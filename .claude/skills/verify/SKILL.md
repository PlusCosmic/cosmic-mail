---
name: verify
description: Verify Cosmic Mail changes end-to-end. Prefer the in-process UI tests and headless Rune renders (deterministic, no display); use the GreenMail E2E for backend/IMAP paths; fall back to the live Hyprland window (wtype/grim) for real focus or visual checks.
---

# Verifying Cosmic Mail changes

The UI is native Rune code, so most verification runs in-process and headless — no
window, no webview, no bridge.

## 1. UI behaviour tests (default)

`src/ui/tests.rs` drives the real view and key handling on Rune's headless `Runtime`
over the demo mailbox (`src/ui/demo.rs`): `rt.find(key)`, `rt.pointer_down/up`,
`rt.key(KeyEvent { .. })`, `rt.elements()` for text, `rt.state` for assertions. Add a
test there for any interaction you change, then `cargo test`.

- Backend results and events reach the UI through `Inbox`; in tests there is no window
  handle, so call `rt.update(|s, tx| { s.poll(now, tx); })` to land them.
- The demo state has no backend: operations apply their optimistic local part and skip
  the network.

## 2. Headless renders (visual)

```sh
cargo run -- --list-scenes
cargo run -- --snap /tmp/out.png --scene settings --scale 2
cargo run -- --tree /tmp/tree.json --scene default     # keys, roles, text, rects
```

Look at the PNG for every UI change (theme, layout, focus rings, dialogs). Add a scene to
`demo::SCENES` for new states. Without a GPU, use lavapipe:
`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json`.

## 3. GreenMail E2E (backend + IMAP)

For sync, body fetch, flags, moves/deletes, search and shipments against a real IMAP
server without touching a real mailbox — see `e2e/README.md`:

```sh
docker compose -f e2e/docker-compose.yml up -d && node e2e/seed.mjs
cargo test --test e2e_greenmail -- --ignored
```

## 4. The live window (last resort)

For genuine keyboard focus / Tab order, compositor behaviour (activation, tray,
notifications) or pixel checks on the real display:

- The installed build may run as `cosmic-mail.service` and owns the single-instance
  socket; a dev launch would just activate it and exit. Stop it first, restart when done:
  `systemctl --user stop cosmic-mail.service` … `systemctl --user start cosmic-mail.service`
- `cargo run` (background, log to a file). Wait for the window:
  `until hyprctl clients -j | jq -e '.[] | select(.class=="cosmic-mail")' ...`
- Focus before every key injection: `hyprctl dispatch focuswindow class:cosmic-mail`.
- Keyboard: `wtype -k j`, `wtype -M ctrl -k k -m ctrl` (palette), `wtype "text"`; use text
  mode (`wtype "G"`) for shifted single characters.
- Screenshots: `grim -g "$(hyprctl clients -j | jq -r '.[] | select(.class=="cosmic-mail") | "\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"')" out.png`
- Under Xvfb (no Hyprland), `xdotool` (`search --name "Cosmic Mail"`, `key j`,
  `mousemove x y click 1`) and `ffmpeg -f x11grab` work against the real window.

## Observing state without the UI

- SQLite cache: `sqlite3 ~/.local/share/cosmic-mail/mail.db` — messages
  (snippet/seen/body_cached/body_text/body_html), folders (unread_count).
- Logs: `RUST_LOG=cosmic_mail=debug cargo run`.

## Environment caveats

- Gmail OAuth token exchange fails in a sandbox-launched dev process
  ("obtaining Gmail access token", red "sync error" status) even though
  `~/.config/cosmic-mail/google-oauth.json` exists — the IMAP account still
  syncs. Don't chase this as an app bug; drive Gmail-dependent flows another way
  or verify at the store layer.
- On the live window this is the user's real mailbox. Selecting an unread message marks
  it read on the server — restore with "Mark unread" or `u`, and verify in sqlite. Never
  drive account removal.

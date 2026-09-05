---
name: verify
description: Verify Cosmic Mail changes end-to-end. Prefer the scripted automation bridge (e2e/) for DOM find/click/assert; fall back to wtype/grim on the live Hyprland session for visual or real keyboard-focus checks.
---

# Verifying Cosmic Mail changes in the running app

## Prefer the scripted bridge (default)

For anything you can assert on — element presence, text, clicking, list/reader/
shipment state — use the **development-only automation bridge** (`internal/automation`,
compiled out of `production` builds) and the `e2e/` harness. It finds DOM elements, clicks
them, and reads text back — deterministic and far less fragile than wtype/grim, and it
works headless. See `e2e/README.md`.

- **Hermetic run (no real mailbox):** `go run ./cmd/fakeimap -ca-out /tmp/ca.pem &` (no
  Docker) or `npm run e2e:env:up` in `frontend/` (GreenMail), then
  `COSMIC_MAIL_EXTRA_CA=… dbus-run-session -- e2e/ci-run.sh` against a dev build
  (`go build -tags gtk3 -o bin/cosmic-mail .`). `go test -tags gtk3 ./internal/sync/`
  covers the engine without the UI.
- **Ad-hoc against the real mailbox:** `wails3 dev`, then drive it with
  `e2e/client.mjs`'s `Bridge` (`await bridge.eval("return …")`, `.click(sel)`, `.waitFor…`).
  Stay read-mostly and restore side effects (see the real-mailbox caveat below).
- Async UI waits are **client-side poll loops** (`bridge.waitFor`) — WebKitGTK evaluates
  each snippet synchronously and does not await a returned Promise.

## When to fall back to wtype/grim (below)

Reserve the live-Hyprland keyboard/screenshot flow for what the bridge can't do:
**pixel/visual confirmation** (theming, layout, focus-ring rendering) and **genuine
keyboard focus / Tab-order behavior** — `Tab` is native focus movement the bridge's DOM
`eval` can't dispatch. Known focus quirks are tracked in papercuts #35 (Escape clears
search) and #36 (message-list listbox focus); confirm against those before filing new ones.

## Before launching

- The promoted daily build runs as a user service and syncs the same accounts. Stop it
  first, restart it when done:
  `systemctl --user stop cosmic-mail.service` … `systemctl --user start cosmic-mail.service`
- Port 9245 must be free (`pkill -f 'vite dev'` for strays). After killing dev
  sessions also check for orphaned `bin/cosmic-mail` processes —
  but beware `pgrep -f` matching its own shell; confirm with `pgrep -a -x cosmic-mail`.

## Launch and drive

- `wails3 dev` (background, log to a file). Wait for the window:
  `until hyprctl clients -j | jq -e '.[] | select(.class=="cosmic-mail")' ...`
- Focus before every key injection (Hyprland 0.56 Lua form; the classic
  `focuswindow class:…` fails with exit 7):
  `hyprctl dispatch 'hl.dsp.focus({ window = "class:cosmic-mail" })'`.
- Keyboard: `wtype -k j`, `wtype -M ctrl -k k -m ctrl` (palette), `wtype "text"`.
  **Gotcha:** `wtype -k G` and `wtype -M shift -k g -m shift` do NOT produce a
  shifted key WebKit reports as `key === "G"` — use text mode `wtype "G"` for
  shifted single characters.
- No pointer injection available (no wlrctl/ydotool). Buttons (filter chips,
  reader actions) are reachable by Tab/Shift+Tab — the focus ring is visible;
  screenshot to confirm which element has focus before pressing Enter/Space.
  Note: the global key handler ignores j/k/etc. while an interactive element
  (button/input) has focus; keyboard-nav keys only work when the list has focus.
- Screenshots: `grim -g "$(hyprctl clients -j | jq -r '.[] | select(.class=="cosmic-mail") | "\(.at[0]),\(.at[1]) \(.size[0])x\(.size[1])"')" out.png`
  (grim takes logical coords; output images are 1.5x hidpi).

## Observing state without the UI

- SQLite cache: `sqlite3 ~/.local/share/cosmic-mail/mail.db` — messages
  (snippet/seen/body_cached/body_text/body_html), folders (unread_count).
  Great for before/after checks of sync and snippet logic.
- App log: the `wails3 dev` log file; `COSMIC_MAIL_LOG=debug wails3 dev` for more.

## Environment caveats

- Gmail OAuth token exchange fails in a sandbox-launched dev process
  ("obtaining Gmail access token", red "sync error" chip) even though
  `~/.config/cosmic-mail/google-oauth.json` exists — the IMAP account still
  syncs. Don't chase this as an app bug; drive Gmail-dependent flows another way
  or verify at the store layer.
- This is the user's real mailbox. Selecting an unread message marks it read on
  the server — restore with the reader's "Mark unread" button (Tab to it) or the
  `u` key, and verify restoration in sqlite. Never drive account removal.

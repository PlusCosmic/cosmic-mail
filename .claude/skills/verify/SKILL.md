---
name: verify
description: Launch and drive Cosmic Mail on the live Hyprland session to verify changes end-to-end (screenshots via grim, keyboard via wtype).
---

# Verifying Cosmic Mail changes in the running app

## Before launching

- The promoted daily build runs as a user service and owns the app's D-Bus name
  (first owner runs sync). Stop it first, restart it when done:
  `systemctl --user stop cosmic-mail.service` … `systemctl --user start cosmic-mail.service`
- Port 1420 must be free (`pkill -f 'vite dev'` for strays). After killing dev
  sessions also check for orphaned `target/debug/cosmic-mail` processes —
  but beware `pgrep -f` matching its own shell; confirm with `pgrep -a -x cosmic-mail`.

## Launch and drive

- `npm run tauri dev` (background, log to a file). Wait for the window:
  `until hyprctl clients -j | jq -e '.[] | select(.class=="cosmic-mail")' ...`
- Focus before every key injection: `hyprctl dispatch focuswindow class:cosmic-mail`.
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
- App log: the tauri-dev log file; `RUST_LOG=cosmic_mail_lib=debug npm run tauri dev` for more.

## Environment caveats

- Gmail OAuth token exchange fails in a sandbox-launched dev process
  ("obtaining Gmail access token", red "sync error" chip) even though
  `~/.config/cosmic-mail/google-oauth.json` exists — the IMAP account still
  syncs. Don't chase this as an app bug; drive Gmail-dependent flows another way
  or verify at the store layer.
- This is the user's real mailbox. Selecting an unread message marks it read on
  the server — restore with the reader's "Mark unread" button (Tab to it) or the
  `u` key, and verify restoration in sqlite. Never drive account removal.

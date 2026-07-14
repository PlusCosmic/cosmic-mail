# Local daily releases

Cosmic Mail's default release channel is deliberately local and manual. It provides a
launcher-visible build for day-to-day use without coupling that build to Vite, Cargo's
debug output, the working tree, or an automatic updater.

## Promote a build

From the repository root:

```sh
npm run promote:local
```

Promotion refuses an uncommitted working tree by default. That makes an installed build
traceable to its Git revision. For a one-off snapshot that is intentionally not
reproducible:

```sh
npm run promote:local -- --allow-dirty
```

The command runs the complete definition-of-done checks, creates a Tauri release binary
with the frontend embedded, and installs it at:

```text
~/.local/share/cosmic-mail-daily/releases/<version-and-revision>/cosmic-mail
```

It then atomically points `~/.local/share/cosmic-mail-daily/current` at that release and
installs `~/.local/share/applications/dev.pluscosmic.mail.desktop` plus the systemd user unit
`~/.config/systemd/user/cosmic-mail.service`. Walker discovers the entry as **Cosmic Mail**.
The desktop ID deliberately matches the app's notification hint.

The service is enabled under `graphical-session.target` and immediately restarted onto the newly
promoted binary. It owns background IMAP IDLE, notifications, and the tray icon with no mapped
window. The Walker entry and tray Open action send an activation to that existing process; closing
the window hides it. The single-instance session D-Bus owner prevents a launcher click from
creating a second sync engine. The service restarts after failures, but a clean tray Quit remains
stopped for the current graphical session.

There is no timer, updater daemon, package-manager hook, or connection to the dev binary.
The daily install changes only when the promotion command succeeds. A failed build or
test leaves the active release untouched.

## Roll back

List installed releases and activate an older one without rebuilding:

```sh
npm run promote:local -- --list
npm run promote:local -- --activate <release-id>
```

The release and dev executables are separate, but use the same application identifier, account
configuration, keyring entries, and SQLite cache. The single-instance owner prevents both from
running simultaneously. Stop the daily unit before a dev session so the dev window—not the daily
build—owns activation:

```sh
systemctl --user stop cosmic-mail.service
npm run tauri dev
```

Restart `cosmic-mail.service` when the dev session ends.

## Later: hosted releases

GitHub Actions can eventually run the same checks and `tauri build` on version tags,
publishing an AppImage or Arch package. That is useful for other machines and users, but
adds signing, artifact retention, and update-policy decisions without improving the local
manual-promotion requirement. Keep application self-update disabled unless the release
policy explicitly changes.

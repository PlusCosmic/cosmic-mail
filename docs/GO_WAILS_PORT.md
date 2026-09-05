# Plan: porting Cosmic Mail to Go + Wails v3

**Status: done.** The port landed on this branch (`claude/rust-tauri-go-wails-port`); what
follows is the plan it was made against, kept for the reasoning, with a record of where
reality differed.

## What actually happened

- **Every package came across with its tests.** 147 Rust tests became ~150 Go tests
  (some merged into table tests, a few added) plus two the Rust build could not have: a
  service round-trip (`app_test.go`) and a full sync-engine run against go-imap's
  in-memory IMAP server over TLS (`internal/sync/integration_test.go`), which covers the
  sweep, folder roles, events, body prefetch with shipment detection, and an IDLE wakeup
  with its notification — the GreenMail case, in-process. The previously `#[ignore]`d
  live-discovery tests are behind `COSMIC_MAIL_LIVE_TESTS=1`.
- **`Window.ExecJS` has no return value**, so the automation bridge posts eval results
  back through Wails' `RawMessageHandler` (a `cosmic-automation:<id>:<json>` message from
  the wrapper). The e2e suite runs unchanged; `cmd/fakeimap` lets it run without Docker.
- **The Wails notification service can't set the app name or the desktop-entry hint** that
  mako rules key on, so `internal/notify` speaks `org.freedesktop.Notifications` over
  godbus directly (already a dependency of Wails).
- **Hyprland 0.56 broke the `focuswindow class:…` fallback** the Rust build shipped;
  `hyprctl dispatch` is Lua now. The port tries `hl.dsp.focus({ window = "class:…" })`
  first, as omarchy's own launch-or-focus helper does.
- **go-message is stricter than mail-parser.** The parser replicates mail-parser's body/
  attachment classification and flat part numbering (so cached `part_index` values stay
  valid) and falls back to "the raw bytes are the text body" for anything go-message
  refuses.
- **Chrono's RFC 3339 wrote `+00:00`, not `Z`**; the store formats dates the same way so
  new rows sort next to the old ones.
- **Keyring entries carry over**: go-keyring uses the same `service` + `username` Secret
  Service attributes the Rust keyring did.
- **Gmail was not re-verified live** in this port (no credentials in the sandbox); the
  OAuth flow, XOAUTH2 client and refresh-token classification are unit-tested and the
  protocol wire format is unchanged. The tray, single-instance activation, hide-on-close
  hook, theme fallback and the full UI e2e suite were exercised on the live Omarchy session.
- **Build time:** production binary ~18 s cold; a dev rebuild under a second.

---

Originally written after doing the same port for RimForge (PlusCosmic/rimforge#3, one
sitting, ~4,500 lines of Go). This is the recipe that worked there, and where
Cosmic Mail differs. Cosmic Mail is a much bigger job: 10,400 lines of Rust and
147 tests against RimForge's 3,500 and 63, with async IMAP, SQLite, OAuth, a
tray, notifications and a background service that RimForge never had. Budget
weeks, not a day.

## The recipe

1. **Scaffold a throwaway Wails project** (`wails3 init -t svelte`) and copy
   its `build/`, `Taskfile.yml` and `.gitignore` in. Do not hand-write them:
   `wails3 update build-assets` then regenerates the plists, desktop file and
   nfpm config from `build/config.yml`. Delete the android/ios/docker dirs and
   their `includes:` lines.
2. **Move the frontend into `frontend/`.** The Taskfiles hardcode it, and
   SvelteKit's default `build/` output collides with Wails' `build/` asset dir.
   `git mv` keeps history. Set adapter-static to `pages: "dist"`, embed
   `frontend/dist` from `main.go`, and set Vite's port to 9245 for `wails3 dev`.
3. **Write `internal/models` first,** then one `App` service in `app.go` whose
   methods are the command table from ARCHITECTURE.md, then run
   `wails3 generate bindings -ts -i` and commit `frontend/bindings`. Point
   `types.ts` at the generated models and `api.ts` at the generated service;
   nothing else in the frontend needs to know. CI regenerates and
   `git diff --exit-code`s the bindings so they can never go stale.
4. **Port package by package, tests first.** Every Rust `#[test]` translates
   almost mechanically; port the tests, then write the Go against them. Rust
   tests that were `#[ignore]`d for touching real state can usually run in Go
   with `t.TempDir()`, `t.Setenv` and a swappable package-level func for the
   side effect (RimForge did this for the trash).
5. **Add one service round-trip test** in package `main` that calls the `App`
   methods against a scratch data dir. It is the only thing that exercises the
   boundary the way the frontend does.
6. **Docs, workflow, PKGBUILD last.** Version of record moves from
   `tauri.conf.json` to `build/config.yml`; the workflow reads it with `sed`.
   Update PlusCosmic/packages *before* merging, because a merge dispatches a
   package build.

## Gotchas from RimForge

- **Linux needs `-tags gtk3`** on every Go command, including
  `go install .../cmd/wails3`. Wails v3 defaults to GTK4 + webkitgtk-6.0;
  the tag keeps webkit2gtk-4.1, which is what the existing package already
  depends on. Default it in the root `Taskfile.yml` so `wails3 build` and
  `wails3 dev` just work. Wails says the tag goes away in v3.1, so plan a GTK4
  switch (install `webkitgtk-6.0`, drop the tag, change package deps) before
  ever upgrading past 3.0.x.
- **`wails3 update build-assets` writes the GTK4 deps into
  `build/linux/nfpm/nfpm.yaml`** regardless of the tag. Codex caught this on
  the RimForge PR; fix it in the same commit as the Taskfile default.
- **A nil Go slice serialises as `null`,** and the generator types every slice
  `T[] | null`. Guarantee non-nil lists in Go (a `NonNil` helper at every
  struct construction site) and narrow the generated types once in `types.ts`
  with a mapped type, rather than null-checking across the frontend.
- **Go source may not contain a literal BOM,** even in a string. Use the `\uFEFF` escape
  in the tests for BOM-tolerant parsing.
- **`dirs::data_dir()` has no Go equivalent on Linux.** `os.UserConfigDir` is
  `~/.config`; write a small helper for `$XDG_DATA_HOME`/`~/.local/share` so
  existing users' data is found where the Rust build left it.
- **Keep the hicolor icon PNGs.** Tauri kept 32/64/128/256 px icons that the
  PKGBUILD installs; Wails keeps one 512 px `appicon.png`. Copy the sized
  ones into `build/linux/icons/` before deleting `src-tauri`.
- The `!lto` PKGBUILD option was for cc-rs picking up makepkg's CFLAGS. cgo
  reads `CGO_CFLAGS`, not `CFLAGS`, so it stops mattering; `!debug` still does.

## What is different about Cosmic Mail

**Things Wails v3 has, so they port rather than get rebuilt:**

| Tauri piece | Wails v3 |
|---|---|
| `tray-icon` feature, `tray.rs` | `app.SystemTray.New()`; Linux speaks StatusNotifierItem over D-Bus itself, no libayatana-appindicator |
| `notify-rust` | `pkg/services/notifications` (org.freedesktop.Notifications over D-Bus) |
| `tauri-plugin-single-instance` | `application.Options.SingleInstance` |
| `visible: false`, `decorations: false` | `WebviewWindowOptions{Hidden: true, Frameless: true}` |
| `app.emit(...)` + `@tauri-apps/api/event` | `application.RegisterEvent[T]("name")` + `app.Event.Emit`; the generator emits typed `Events.On` bindings |
| `automation.rs` evaluating JS in the webview | `window.ExecJS`; keep it behind a build tag the way `debug_assertions` gates it now |
| `tauri-plugin-opener` `open-url` | `app.Browser.OpenURL` |
| `tauri::State<AppState>` | fields on the service struct; construct it in `main` and pass it to `NewService` |

Check the tray on Omarchy early: StatusNotifierItem needs a host (waybar or
omarchy-shell's tray) and RimForge never exercised it.

**Things that need a Go library choice (the actual port risk):**

| Rust crate | Go | Notes |
|---|---|---|
| `async-imap` + `tokio` | `github.com/emersion/go-imap/v2` | Different API shape: goroutines instead of tasks, `context` instead of cancellation tokens. `sync/imap.rs` (2,000 lines) and `sync/mod.rs` (1,200) are where most of the time goes. Unsolicited responses and IDLE both exist in go-imap; the `async-channel` drain trick will need redesigning around its handler API. |
| `lettre` | `github.com/emersion/go-smtp` + `go-message` | Straightforward. |
| `mail-parser` | `github.com/emersion/go-message/mail` | Check charset handling against the fixtures in `e2e/fixtures/mail`; mail-parser is lenient in ways go-message is not. |
| `rusqlite` (bundled) | `modernc.org/sqlite` (pure Go) or `mattn/go-sqlite3` (cgo) | Prefer modernc: one less cgo dependency next to WebKit, no `!lto` concerns. `store.rs` is 2,300 lines; it ports mechanically but is the biggest single file. `Arc<Mutex<Connection>>` becomes `*sql.DB` with `SetMaxOpenConns(1)`, or a mutex around a single conn if the WAL/busy semantics matter. |
| `keyring` (Secret Service) | `github.com/zalando/go-keyring` | Also D-Bus Secret Service, so existing stored credentials should be readable if the service/user names are kept identical. Verify against a real gnome-keyring before trusting that. |
| `oauth2` | `golang.org/x/oauth2` | PKCE and the loopback redirect are built in. |
| `hickory-resolver` | `net.Resolver` | SRV/MX lookups for `autoconfig.rs`. |
| `notify` (inotify) | `github.com/fsnotify/fsnotify` | For the Omarchy theme watcher. |
| `socket2` keepalive | `net.Dialer{KeepAliveConfig: ...}` | Go 1.23+ exposes count/interval/idle directly. |
| `roxmltree` | copy `internal/xmldom` from RimForge | Case-insensitive element tree over `encoding/xml`. |
| `rustls` + `rustls-native-certs` | `crypto/tls` | System roots are the default. The `COSMIC_MAIL_EXTRA_CA` E2E hook is `x509.CertPool.AppendCertsFromPEM` behind the same debug gate. |
| `toml` | `github.com/pelletier/go-toml/v2` | |
| `tracing` | `log/slog` | |
| `anyhow`/`thiserror` | `fmt.Errorf` with `%w` | Errors cross to the frontend as strings either way. |

**`--background` mode and the systemd unit.** The unit runs
`cosmic-mail --background` under `graphical-session.target`. Wails' `app.Run()`
still initialises GTK, the same as Tauri's did, so this keeps working as "run
the app with no window and a tray". Do not reach for Wails' `server` build
tag for it; that mode has no tray and no webview.

**E2E.** `e2e/` drives the real UI through the loopback automation bridge, not
WebDriver, so nothing in it is Tauri-specific except the bridge itself. Port
`automation.rs` early and keep `ci-run.sh` green throughout; it is the only
test that proves IMAP sync end to end against GreenMail.

## Order of work

1. Scaffold, move frontend, models, `App` with every method stubbed to
   return "not ported", bindings generated, `npm run check` green. Half a day.
2. `store` + `settings` + `accounts` with their tests. The rest depends on
   them.
3. `autoconfig`, `auth/oauth`, `wire`, `send`, `attachments`, `shipments`:
   self-contained, test-heavy, parallelisable.
4. `sync/imap` and `sync/mod`: the redesign. Get GreenMail passing before
   touching anything cosmetic.
5. Tray, notifications, Omarchy watcher, automation bridge, `--background`.
6. Docs, workflow, PKGBUILD (`makedepends=('go' 'nodejs' 'npm')`, build with
   `npm ci` in `frontend/` then `go build -tags gtk3,production`).

## Whether to do it

RimForge was worth it for build time and a single toolchain. Cosmic Mail's
case is weaker: the Rust IMAP and mail-parsing stack is mature and already
debugged against real providers, and the port's largest chunk is rewriting
exactly that. Do it if you want Go across both apps and are prepared to
re-earn the IMAP edge cases; not for the build speed alone.

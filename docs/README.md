# Cosmic Mail — Docs

Documentation for agents and humans picking up this project without prior context.
**Read in this order:**

| doc | what it gives you |
|---|---|
| this file | 2-minute project overview |
| [SETUP.md](SETUP.md) | Requirements, account setup, Gmail OAuth, keyboard shortcuts, and local data locations |
| [TERMINOLOGY.md](TERMINOLOGY.md) | Shared names and concise definitions for UI surfaces, mail concepts, sync behavior, and application architecture |
| [OMARCHY.md](OMARCHY.md) | Live theming, Mako notifications, and the background application lifecycle |
| [ARCHITECTURE.md](ARCHITECTURE.md) | **The binding contract** between Go backend and Svelte frontend: commands, events, wire types, DB schema, module layout, sync/notification/oauth/discovery specs. Conform to it exactly; update it in the same change that alters the API surface. |
| [DEVELOPMENT.md](DEVELOPMENT.md) | Build/run/test commands, verification workflow, local state paths, how to test each subsystem |
| [RELEASING.md](RELEASING.md) | manually promote an immutable local daily build into the Omarchy launcher |
| [GOTCHAS.md](GOTCHAS.md) | Hard-won, non-obvious lessons (each one cost real debugging time — read before touching deps, TLS, SASL, or file watchers) |
| [GO_WAILS_PORT.md](GO_WAILS_PORT.md) | The plan the Rust → Go/Wails port followed, and where reality differed |
| [ROADMAP.md](ROADMAP.md) | Frozen archive of pre-issues history (all work tracking — open and done — lives in GitHub issues) |

## What this project is

**Cosmic Mail** — a native Linux mail client for the [omarchy](https://omarchy.org) desktop
(Arch + Hyprland + mako + walker). Wails 3, Go backend, Svelte 5 (runes) + TypeScript
frontend, SvelteKit static adapter (SPA, no SSR) embedded into the binary.

Design goals, in priority order:

1. **Native omarchy citizen** — notifications through mako (freedesktop D-Bus, stable
   app-name, click-to-focus); UI colors live-track the active omarchy theme by watching
   `~/.local/state/omarchy/current/theme/colors.toml`; keyboard-driven, minimal chrome,
   monospace aesthetic (see `prototypes/`).
2. **One sync engine for both account kinds** — plain IMAP (password) and Gmail
   (OAuth2 PKCE → SASL XOAUTH2 over IMAP). No Gmail REST API.
3. **Local-first** — SQLite (WAL) envelope/body cache; lazy body fetch; IMAP IDLE for
   push; secrets only in the Secret Service keyring, never on disk.

## Layout at a glance

```
docs/               you are here (ARCHITECTURE.md = the contract)
main.go, app.go     Wails wiring and the App service (every command is a method)
internal/           Go core: store, accounts, imap, mailparse, sync, oauth, send,
                    autoconfig, shipments, omarchy, notify, desktop, automation
frontend/           Svelte: src/lib/{api,types,theme,pane-layout}.ts,
                    src/lib/stores/mail.svelte.ts, src/lib/components/*.svelte,
                    src/routes/+page.svelte; bindings/ is generated from Go
build/              Wails build assets (config.yml = version of record, icons, nfpm)
e2e/                automation-bridge UI tests + fixtures; cmd/fakeimap serves them
prototypes/         3 static HTML design mockups (open in browser; 03-hybrid is the
                    direction the shell is converging toward)
```

## Working conventions (how changes get made here)

- `docs/ARCHITECTURE.md` is contract-first: for any new command/event/type, extend the
  contract **before** implementing, so parallel work can't drift. Wire types live in
  `internal/models` with camelCase `json` tags; regenerate `frontend/bindings` after
  changing them or `app.go`.
- Definition of done for any change: `gofmt -l .` (empty), `go vet -tags gtk3 ./...`,
  `go test -tags gtk3 ./...`, a fresh `wails3 generate bindings`, and `npm run check`,
  `npm run build`, `npm test` in `frontend/` all green. Live-network discovery tests are
  behind `COSMIC_MAIL_LIVE_TESTS=1`; run them when touching discovery.
- Go style: errors wrapped with `%w`, plain `error` at service boundaries, `log/slog` for
  logs, no panics outside tests/main.
- `wails3 dev` auto-rebuilds on Go and frontend changes. Keep intermediate changes
  buildable when another developer may have the dev process running.
- Open work lives in GitHub issues: feature-sized work is labelled `roadmap`,
  built-but-unconfirmed work is labelled `verification`, and small bugs / UX quirks are
  labelled `papercut` (tracked on the
  [Cosmic Mail Papercuts board](https://github.com/users/PlusCosmic/projects/2)).
  Closed issues are the done history — close them with a short completion comment.
  [ROADMAP.md](ROADMAP.md) is a frozen archive from before the move to issues; don't
  update it.

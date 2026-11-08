# Cosmic Mail — Docs

Documentation for agents and humans picking up this project without prior context.
**Read in this order:**

| doc | what it gives you |
|---|---|
| this file | 2-minute project overview |
| [SETUP.md](SETUP.md) | Requirements, account setup, Gmail OAuth, keyboard shortcuts, and local data locations |
| [TERMINOLOGY.md](TERMINOLOGY.md) | Shared names and concise definitions for UI surfaces, mail concepts, sync behavior, and application architecture |
| [OMARCHY.md](OMARCHY.md) | Live theming, Mako notifications, and the background application lifecycle |
| [ARCHITECTURE.md](ARCHITECTURE.md) | **The binding contract** between the backend and the Rune UI: backend operations, events, wire types, DB schema, module layout, sync/notification/oauth/discovery specs. Conform to it exactly; update it in the same change that alters the API surface. |
| [DEVELOPMENT.md](DEVELOPMENT.md) | Build/run/test commands, verification workflow, local state paths, how to test each subsystem |
| [RELEASING.md](RELEASING.md) | How a merge to `main` becomes an Arch package |
| [GOTCHAS.md](GOTCHAS.md) | Hard-won, non-obvious lessons (each one cost real debugging time — read before touching deps, TLS, SASL, or file watchers) |
| [ROADMAP.md](ROADMAP.md) | Frozen archive of pre-issues history (all work tracking — open and done — lives in GitHub issues) |

## What this project is

**Cosmic Mail** — a native Linux mail client for the [omarchy](https://omarchy.org) desktop
(Arch + Hyprland + mako + walker). One Rust crate: a UI-agnostic backend plus a native
GPU-rendered UI built on [Rune](https://github.com/anomalyco/rune) (wgpu + winit, no
webview, no JavaScript).

Design goals, in priority order:

1. **Native omarchy citizen** — notifications through mako (freedesktop D-Bus, stable
   app-name, click-to-focus); keyboard-driven, minimal chrome. The look is the UI
   foundation's (`rune-theme-foundation`): its colour schemes, light and dark, and its type.
2. **One sync engine for both account kinds** — plain IMAP (password) and Gmail
   (OAuth2 PKCE → SASL XOAUTH2 over IMAP). No Gmail REST API.
3. **Local-first** — SQLite (WAL) envelope/body cache; lazy body fetch; IMAP IDLE for
   push; secrets only in the Secret Service keyring, never on disk.

## Layout at a glance

```
docs/               you are here (ARCHITECTURE.md = the contract)
src/                backend: backend.rs (the operations the UI calls), events.rs (AppEvent),
                    store.rs, accounts.rs, omarchy.rs, notifications.rs, autoconfig.rs,
                    desktop.rs, sync/{mod,imap}.rs, auth/oauth.rs, wire.rs
src/app.rs          startup: single instance, tokio runtime, tray, window
src/ui/             Rune UI: mod.rs (state + Inbox), store.rs (state transitions),
                    view/{mod,chrome,list,reader,modals}.rs, html.rs, images.rs, theme.rs,
                    demo.rs (demo mailbox + headless scenes), tests.rs
tests/              e2e_greenmail.rs (real UI + backend against a GreenMail fixture)
e2e/                the GreenMail fixture (docker compose, seed mail, throwaway TLS)
assets/             icons
```

## Working conventions (how changes get made here)

- `docs/ARCHITECTURE.md` is contract-first: for any new backend operation/event/type,
  extend the contract **before** implementing, so parallel work can't drift. Wire types
  keep `#[serde(rename_all = "camelCase")]` (settings and accounts are persisted as JSON).
- Definition of done for any change: `cargo fmt --check`,
  `cargo clippy --all-targets -- -D warnings` and `cargo test` green; for UI changes, a
  headless `cargo run -- --snap out.png --scene <scene>` of the affected view checked by
  eye. Live-network tests are `#[ignore]`d; run them (and the GreenMail E2E) when
  touching discovery/sync.
- Rust style: edition 2021, no `unwrap()` outside tests/main, `anyhow` internally,
  `AppError` at the `Backend` boundary, `tracing` for logs.
- Open work lives in GitHub issues: feature-sized work is labelled `roadmap`,
  built-but-unconfirmed work is labelled `verification`, and small bugs / UX quirks are
  labelled `papercut` (tracked on the
  [Cosmic Mail Papercuts board](https://github.com/users/PlusCosmic/projects/2)).
  Closed issues are the done history — close them with a short completion comment.
  [ROADMAP.md](ROADMAP.md) is a frozen archive from before the move to issues; don't
  update it.

# Cosmic Mail

This file is shared by Claude and Codex. `AGENTS.md` is a symlink to this file, so
keep instructions tool-agnostic unless a section explicitly names a tool.

Native Linux mail client for the omarchy desktop. Tauri 2 + Rust (src-tauri/) +
Svelte 5/TS (src/). IMAP + Gmail (OAuth2→XOAUTH2), mako notifications, live omarchy
theming.

**Start with `docs/README.md`** (2-min overview + reading order). Non-negotiables:

- `docs/ARCHITECTURE.md` is the **binding contract** for every command, event, wire
  type, and schema. Extend it before changing the API surface. All wire types serialize
  camelCase.
- Read `docs/GOTCHAS.md` before touching deps, TLS/auth, watchers, or discovery — it
  exists because each entry already burned us once. Crate APIs here are newer than
  model memory: read vendored sources in `~/.cargo/registry/src/` before writing
  dep-facing code.
- Definition of done: `cargo check && cargo clippy && cargo test --lib && cargo fmt`
  (src-tauri/) and `npm run check && npm run build` (root) all green.
- No `unwrap()` outside tests/main; `anyhow` internally, `AppError` at command
  boundaries; never log or persist secrets (keyring only).
- Don't run git commits unless asked; the session owner supervises commits.

Dev commands, local state paths, and per-subsystem test recipes: `docs/DEVELOPMENT.md`.
Status and what to build next: `docs/ROADMAP.md`. Small bugs / UX quirks are GitHub
issues labelled `papercut` (board: <https://github.com/users/PlusCosmic/projects/2>);
file new ones there, not in the roadmap.

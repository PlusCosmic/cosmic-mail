# Cosmic Mail

This file is shared by Claude and Codex. `AGENTS.md` is a symlink to this file, so
keep instructions tool-agnostic unless a section explicitly names a tool.

Native Linux mail client for the omarchy desktop. One Rust crate: the backend
(`src/`, UI-agnostic) and a native GPU UI built with [Rune](https://github.com/anomalyco/rune)
(`src/ui/`). IMAP + Gmail (OAuth2→XOAUTH2), mako notifications, live omarchy theming.

**Start with `docs/README.md`** (2-min overview + reading order). Non-negotiables:

- `docs/ARCHITECTURE.md` is the **binding contract** for every `Backend` method
  (command), `AppEvent`, wire type, and schema. Extend it before changing the API
  surface. Persisted JSON (accounts, settings) serializes camelCase.
- Read `docs/GOTCHAS.md` before touching deps, TLS/auth, watchers, discovery, or Rune
  layout — it exists because each entry already burned us once. Crate APIs here are
  newer than model memory: read the sources in `~/.cargo/registry/src/` and Rune's
  checkout in `~/.cargo/git/checkouts/rune-*/` (its `crates/rune/README.md` is the
  manual) before writing dep-facing code. Rune is pinned by rev; the toolchain is
  pinned by `rust-toolchain.toml`.
- Definition of done: `cargo fmt --check && cargo clippy --all-targets -- -D warnings &&
  cargo test` all green; for UI changes also render the affected scene headlessly
  (`cargo run -- --snap out.png --scene NAME`) and look at it.
- No `unwrap()` outside tests/main; `anyhow` internally, `AppError` at the `Backend`
  boundary; never log or persist secrets (keyring only).
- Don't run git commits unless asked; the session owner supervises commits.

Dev commands, local state paths, and per-subsystem test recipes: `docs/DEVELOPMENT.md`.
What to build next: GitHub issues — `roadmap` (features) and `verification` (built,
needs live confirmation) on the [Roadmap board](https://github.com/users/PlusCosmic/projects/3);
`papercut` (small bugs / UX quirks) on the
[Papercuts board](https://github.com/users/PlusCosmic/projects/2). Closed issues are the
done history — close them with a short completion comment; don't maintain `docs/ROADMAP.md`
(frozen archive, pre-dates the move to issues).

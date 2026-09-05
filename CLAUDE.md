# Cosmic Mail

This file is shared by Claude and Codex. `AGENTS.md` is a symlink to this file, so
keep instructions tool-agnostic unless a section explicitly names a tool.

Native Linux mail client for the omarchy desktop. Wails 3 + Go (`main.go`, `app.go`,
`internal/`) + Svelte 5/TS (`frontend/`). IMAP + Gmail (OAuth2→XOAUTH2), mako
notifications, live omarchy theming.

**Start with `docs/README.md`** (2-min overview + reading order). Non-negotiables:

- `docs/ARCHITECTURE.md` is the **binding contract** for every command, event, wire
  type, and schema. Extend it before changing the API surface. All wire types serialize
  camelCase.
- Read `docs/GOTCHAS.md` before touching deps, TLS/auth, watchers, or discovery — it
  exists because each entry already burned us once. Module APIs here are newer than
  model memory: read the sources in `~/go/pkg/mod/` before writing dep-facing code.
- Every Go command takes `-tags gtk3` (Wails 3 links GTK4/webkitgtk-6.0 otherwise).
- Definition of done: `gofmt -l .` empty, `go vet -tags gtk3 ./...`,
  `go test -tags gtk3 ./...`, `wails3 generate bindings -ts -i -clean=true -d
  frontend/bindings -f "-tags gtk3"` leaving no diff, and `npm run check && npm run build
  && npm test` (frontend/) all green.
- No panics outside tests/main; wrap errors with `%w`, plain `error` at service
  boundaries; never log or persist secrets (keyring only).
- Don't run git commits unless asked; the session owner supervises commits.

Dev commands, local state paths, and per-subsystem test recipes: `docs/DEVELOPMENT.md`.
What to build next: GitHub issues — `roadmap` (features) and `verification` (built,
needs live confirmation) on the [Roadmap board](https://github.com/users/PlusCosmic/projects/3);
`papercut` (small bugs / UX quirks) on the
[Papercuts board](https://github.com/users/PlusCosmic/projects/2). Closed issues are the
done history — close them with a short completion comment; don't maintain `docs/ROADMAP.md`
(frozen archive, pre-dates the move to issues).

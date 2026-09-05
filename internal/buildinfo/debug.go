//go:build !production

// Package buildinfo says whether this is a development build. The Rust
// build gated its test hooks (the automation bridge, COSMIC_MAIL_EXTRA_CA,
// COSMIC_MAIL_TEST_IMAP_PASSWORD) on `debug_assertions`; the Go build gates
// them on the absence of Wails' `production` tag, which `wails3 build`
// (and the Arch package) set and `wails3 dev` / `wails3 build DEV=true` do not.
package buildinfo

// Debug is true in development builds only.
const Debug = true

# rune-taffy (patched)

Rune's `rune-taffy` crate at the rev `Cargo.toml` pins, plus one change: `encode_wrap` in
`src/style/mod.rs` has a catch-all arm. blitz-dom (the HTML mail layout) turns on taffy's
`flexbox_balance` feature, which adds `FlexWrap::Balance`/`BalanceReverse`; Rune's exhaustive
match then fails to compile. Rune never sets those values, so they encode as wrapping.

Wired in with `[patch."https://github.com/anomalyco/rune"]` in the root `Cargo.toml`. When the
Rune rev is bumped, copy the crate from the new checkout and re-apply the arm — or drop this
directory once Rune handles the variants upstream. See `docs/GOTCHAS.md` ("Blitz").

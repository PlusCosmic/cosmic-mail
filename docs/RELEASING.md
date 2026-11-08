# Releasing

Builds reach other machines through a hosted Arch package repository: every merge to
`main` that passes the definition of done is packaged and published. (The old local
"daily promotion" script went away with the Tauri build; for a local build, run
`cargo build --release` and use `target/release/cosmic-mail`.)

A release binary is a single self-contained executable: the UI is native Rune code
compiled into it, with no embedded web assets, webview or runtime dependencies beyond
`libxkbcommon`, `wayland` (or X11) and a Vulkan driver.

## Hosted Arch repository

Every merge to `main` is built as an Arch package and published to a pacman repository
hosted on Backblaze B2. Application self-update stays disabled.

### How it works

`.github/workflows/release.yml` runs the definition-of-done suite on every merge to
`main` — `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`,
and a headless `--snap` render of the demo mailbox (on lavapipe), with Rune's build
dependencies installed. Only if all of that passes does it send a `repository_dispatch`
to [PlusCosmic/packages], carrying the package name, this repository, the merged commit
SHA, and the version from `Cargo.toml` (the version of record).

The dispatch also refuses to run for anything but the current tip of `main`. `pkgrel`
is the packaging repository's run number, so *any* dispatch outranks every package
already published — a manual run launched from a feature branch, or a re-run of an
older `main` run, would hand unmerged or stale code a higher `pkgrel` and offer it to
clients as an upgrade. Nothing prunes a bad package from the bucket, so recovering from
one means bumping the version to supersede it.

This repository never writes to the package bucket, and it holds no `PKGBUILD`. Both
live in the packaging repository, which is the sole publisher. That is deliberate: the
pacman database is a read-modify-write over shared object storage, and GitHub's
`concurrency` groups are repository-scoped, so two projects publishing from their own
repositories could each read the old index and silently drop the other's package.
Funnelling every update through one repository gives one concurrency group that actually
serialises them.

The packaging repository builds from `git archive` of the dispatched SHA, so a package
can only contain committed source. It installs the binary plus the desktop entry, the
icons, and the systemd user unit under `/usr/lib/systemd/user/`.

> [!IMPORTANT]
> The `PKGBUILD` in [PlusCosmic/packages] predates the move to Rune and must be updated
> alongside this change: build with `cargo build --release --locked` (no npm, no
> `tauri build`), install `target/release/cosmic-mail`, and replace the `webkit2gtk-4.1`,
> `gtk3`, `librsvg` and `libayatana-appindicator` dependencies with `libxkbcommon`,
> `wayland` and `vulkan-icd-loader` (plus `vulkan-driver`). Its `makedepends` need
> rustup (the toolchain is pinned to 1.99.1 by `rust-toolchain.toml`) and git (Rune is
> a git dependency). The desktop entry should keep `StartupWMClass=cosmic-mail`, the
> window's app id.

`pkgrel` is the packaging repository's Actions run number rather than a hand-maintained
counter, so each build is an upgrade to pacman even when the `Cargo.toml` version is
unchanged. Bump that version for anything users should recognise as a release.

### Setup

This repository needs one secret, `PACKAGES_DISPATCH_TOKEN`: a fine-grained PAT with
`contents: write` on `PlusCosmic/packages`. `GITHUB_TOKEN` cannot dispatch across
repositories.

Everything else — the B2 credentials, the bucket path, the client `pacman.conf` line and
the packaging definitions — lives in [PlusCosmic/packages]. It is private because the
bucket path prefix is what keeps the package repository unlisted, so that is the only
place the repository URL is written down.

Packages are unsigned, and clients set `SigLevel = Optional TrustAll` scoped to this
repository so it does not weaken verification for `core` or `extra`. The tradeoff is
explicit: obscurity hides the URL, but anyone who obtains write access to the bucket can
ship a package that runs as root on every client.

[PlusCosmic/packages]: https://github.com/PlusCosmic/packages

### Running a dev build beside the package

Both builds share one application identifier, account configuration, keyring entries,
SQLite cache and single-instance socket, so a `cargo run` while the packaged
`cosmic-mail.service` runs just activates the packaged window and exits. Stop the
service first (`systemctl --user stop cosmic-mail.service`) and restart it afterwards.

### Rehearsing a packaging change

`scripts/build-local.sh` in the packaging repository runs the same staging and `makepkg`
invocation the workflow does, against a local checkout of this one and without
publishing:

```sh
scripts/build-local.sh cosmic-mail ~/Projects/Mail
```

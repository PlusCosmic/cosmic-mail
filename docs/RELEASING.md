# Releasing

Builds reach other machines through a hosted Arch package repository: every merge to
`main` that passes the definition of done is packaged and published. (The old local
"daily promotion" script went away with the Tauri build; for a local build, run
`cargo build --release` and use `target/release/cosmic-mail`.)

## Versions

Every build on `main` gets its own version, `MAJOR.MINOR.PATCH`, computed by
`scripts/version.sh`: `MAJOR.MINOR` comes from `Cargo.toml`'s `version` (its patch digit
is ignored), and `PATCH` is the number of commits reachable from the built commit. Every
merge to `main` adds commits, so the patch only ever grows, and every workflow (the
Windows and macOS releases, the Arch package) computes the same version for the same
commit with no stored counter and no bot commits. Bump `MAJOR.MINOR` in `Cargo.toml` by hand for a
release people should notice (the patch keeps counting from the commit total).

The build embeds the version: CI and the packages set `COSMIC_MAIL_VERSION`, which
`cosmic_mail::VERSION` reads (falling back to `Cargo.toml`'s version for local builds),
the Settings dialog's About section shows, and `build.rs` writes into the Windows exe's
version info; the macOS bundle's `Info.plist` carries it too. The script needs full history (`fetch-depth: 0` in Actions; it refuses a
shallow clone).

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
SHA, and the version from `scripts/version.sh` (see [Versions](#versions)), which becomes
the package's `pkgver`.

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
> rustup (the toolchain is pinned to 1.99.1 by `rust-toolchain.toml`) and git (Rune and Blitz are
> git dependencies). The desktop entry should keep `StartupWMClass=cosmic-mail`, the
> window's app id.

`pkgver` grows with every merge (the commit count), and `pkgrel` is the packaging
repository's Actions run number, so each build is an upgrade to pacman. The PKGBUILD
exports `COSMIC_MAIL_VERSION="$pkgver"` so the binary reports the same version.

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

## Windows releases (GitHub Releases)

`.github/workflows/windows.yml` builds Windows natively on `windows-latest`:

- **Every pull request and push**: two parallel jobs — `test` (`cargo test --locked --lib`,
  no debug info) and `build` (`cargo build --locked --release`, which then packages `dist/*-setup.exe` (Inno Setup, `packaging/windows/cosmic-mail.iss`) and
  `dist/*-portable.zip`, uploaded as the `cosmic-mail-windows` workflow artifact). A PR
  that breaks the Windows build fails here. Each job keeps its own Rust cache; caches
  saved on `main` are what PRs and later `main` pushes restore, so a cold cache (the
  first run after a dependency bump) costs ~15 min and a warm one a few minutes.
- **Every push to `main`**: once both jobs pass, the `release` job publishes those files
  to the commit's GitHub Release, tagged `v<version>` (see [Versions](#versions)) and
  marked latest. The macOS workflow publishes to the same release; see
  [One release per commit](#one-release-per-commit).

The installer is per-user (`PrivilegesRequired=lowest`: installs under
`%LOCALAPPDATA%\Programs`, no UAC prompt), adds a Start-menu entry, optionally a
desktop icon, and optionally a `HKCU\…\Run` entry that starts `cosmic-mail.exe
--background` at sign-in — the Windows counterpart of `cosmic-mail.service`. It closes a
running Cosmic Mail before upgrading and stops it on uninstall. **Never change the
script's `AppId`**: upgrades find the existing install by it.

The release exe is a GUI-subsystem binary (no console window; `src/main.rs`) with
`assets/icon.ico` embedded by `build.rs` (`winresource`). Builds are not code-signed;
signing would add an Authenticode step with a certificate in Actions secrets before
packaging.

To check the Windows build from Linux without a Windows machine:
`rustup target add x86_64-pc-windows-gnu`, install `gcc-mingw-w64-x86-64`, then
`cargo build --release --target x86_64-pc-windows-gnu` (CI uses the MSVC target).

## macOS releases (GitHub Releases)

`.github/workflows/macos.yml` builds macOS natively on `macos-latest`, the same shape as
the Windows workflow:

- **Every pull request and push**: two parallel jobs — `test` (`cargo test --locked --lib`,
  no debug info) and `build`. `build` compiles both architectures in one
  `cargo build --locked --release --target aarch64-apple-darwin --target x86_64-apple-darwin`,
  packages them with `packaging/macos/package.sh` into
  `dist/cosmic-mail-<version>-macos-universal.dmg` (uploaded as the `cosmic-mail-macos`
  workflow artifact), then mounts that image and checks what ships: both architectures are
  in the binary, the signature is intact, the bundle carries the right version, the binary
  starts on each architecture (the Intel half under Rosetta), and a headless `--snap` of
  the demo mailbox draws (uploaded as `snapshot-macos`).
- **Every push to `main`**: once both jobs pass, the `release` job attaches the disk image
  to the commit's GitHub Release.

`packaging/macos/package.sh VERSION OUT_DIR BINARY...` is the whole packaging step, and
runs by hand on a Mac too (see [DEVELOPMENT.md](DEVELOPMENT.md)). It merges the binaries
with `lipo`, lays out `Cosmic Mail.app` around `packaging/macos/Info.plist`, builds
`AppIcon.icns` from the repository's `icon.png` (`sips` + `iconutil`), signs the bundle
(see [Signing and notarization](#signing-and-notarization)), and writes a compressed disk
image holding the app and an `/Applications` link to drag it onto. The binary is built for macOS 13 or later (`MACOSX_DEPLOYMENT_TARGET` in the
workflow, `LSMinimumSystemVersion` in `Info.plist`; keep the two equal). **Never change
the bundle identifier** (`dev.pluscosmic.mail`): macOS keys the app's Keychain access and
notification settings on it.

### Signing and notarization

Without the secrets below, builds are signed ad hoc and not notarized, which has two
consequences:

- **Gatekeeper blocks the first launch** of a downloaded copy. Open System Settings →
  Privacy & Security and choose **Open Anyway**, or clear the download flag:
  `xattr -dr com.apple.quarantine "/Applications/Cosmic Mail.app"`.
- **Every update is a new app to the Keychain.** An ad-hoc signature identifies one exact
  build, so after each update macOS asks again before Cosmic Mail may read its stored
  passwords and tokens.

Both go away once the repository has a Developer ID certificate and a notary key (an Apple
Developer Program membership provides both). The `build` job then imports the certificate
into a throwaway keychain, `package.sh` signs the app with it and the hardened runtime
(`CODESIGN_IDENTITY`) and signs the image, and the job submits the image to Apple's notary
service, staples the ticket to it, and validates the staple before anything is uploaded.
The switch is the certificate secret: with it set, a build that cannot sign or notarize
fails. Pull requests from forks never see secrets, so they stay ad hoc.

| secret | what it holds |
|---|---|
| `MACOS_CERTIFICATE_P12` | the Developer ID Application certificate and its private key, as a base64-encoded `.p12` |
| `MACOS_CERTIFICATE_PASSWORD` | the `.p12`'s password |
| `MACOS_NOTARY_KEY` | an App Store Connect API key: the whole `AuthKey_<id>.p8` file |
| `MACOS_NOTARY_KEY_ID` | that key's ID |
| `MACOS_NOTARY_ISSUER_ID` | the issuer ID shown above the keys list |

The certificate (any machine with OpenSSL; no Mac needed):

```sh
openssl genrsa -out developer-id.key 2048
openssl req -new -key developer-id.key -out developer-id.csr -subj "/CN=Cosmic Mail Developer ID"
# developer.apple.com → Certificates → + → Developer ID Application: upload the .csr,
# download developerID_application.cer.
openssl x509 -inform DER -in developerID_application.cer -out developer-id.pem
openssl pkcs12 -export -legacy -inkey developer-id.key -in developer-id.pem -out developer-id.p12
base64 -w0 developer-id.p12   # the value of MACOS_CERTIFICATE_P12
```

`-legacy` matters: macOS cannot import the encryption OpenSSL 3 uses for a `.p12` by
default, and reports it as a wrong password. Only the account holder can create a
Developer ID certificate. Keep `developer-id.key` somewhere safe and out of the repository
(`.gitignore` already excludes `*.key`, `*.pem` and `*.p12`); the certificate lasts five
years.

The notary key: App Store Connect → Users and Access → Integrations → App Store Connect
API → Team Keys → generate a key with the **Developer** role. The `.p8` downloads once.

After the first notarized build, drop the "not notarized" sentences from the release notes
in `scripts/publish-release.sh` and from `README.md`.

What the macOS build does not have yet:

- **No menu-bar icon.** Rune's tray has no macOS backend, so the app logs that and carries
  on. Closing the window keeps the app (and sync) running in the Dock; clicking the Dock
  icon brings the window back, and the application menu's Quit ends it.
- **No start at sign-in.** Nothing installs a login item; add Cosmic Mail under System
  Settings → General → Login Items to have it start with the session.

## One release per commit

The Windows and macOS workflows build the same commit separately and publish to one
GitHub Release, `v<version>`. Each calls `scripts/publish-release.sh VERSION FILE...`,
which creates the release with its files if it does not exist yet and otherwise attaches
the files it lacks, so the order the builds finish in does not matter and a re-run
publishes nothing. The two `release` jobs share a concurrency group
(`github-release-<sha>`), so they never run at the same moment for one commit.

A platform whose build fails on `main` simply has no files on that release; the other
platform's still publish. The release notes are written by whichever job creates the
release, so the script's text covers both platforms: update it there when either
platform's files or caveats change.

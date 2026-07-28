# Releasing

Cosmic Mail has two channels: the local daily promotion below, which stays the default,
and a hosted Arch package repository for other machines. The default release channel is
deliberately local and manual. It provides a
launcher-visible build for day-to-day use without coupling that build to Vite, Cargo's
debug output, the working tree, or an automatic updater.

## Promote a build

From the repository root:

```sh
npm run promote:local
```

Promotion refuses an uncommitted working tree by default. That makes an installed build
traceable to its Git revision. For a one-off snapshot that is intentionally not
reproducible:

```sh
npm run promote:local -- --allow-dirty
```

The command runs the complete definition-of-done checks, creates a Tauri release binary
with the frontend embedded, and installs it at:

```text
~/.local/share/cosmic-mail-daily/releases/<version-and-revision>/cosmic-mail
```

It then atomically points `~/.local/share/cosmic-mail-daily/current` at that release and
installs `~/.local/share/applications/dev.pluscosmic.mail.desktop` plus the systemd user unit
`~/.config/systemd/user/cosmic-mail.service`. Walker discovers the entry as **Cosmic Mail**.
The desktop ID deliberately matches the app's notification hint.

The service is enabled under `graphical-session.target` and immediately restarted onto the newly
promoted binary. It owns background IMAP IDLE, notifications, and the tray icon with no mapped
window. The Walker entry and tray Open action send an activation to that existing process; closing
the window hides it. The single-instance session D-Bus owner prevents a launcher click from
creating a second sync engine. The service restarts after failures, but a clean tray Quit remains
stopped for the current graphical session.

There is no timer, updater daemon, package-manager hook, or connection to the dev binary.
The daily install changes only when the promotion command succeeds. A failed build or
test leaves the active release untouched.

## Roll back

List installed releases and activate an older one without rebuilding:

```sh
npm run promote:local -- --list
npm run promote:local -- --activate <release-id>
```

The release and dev executables are separate, but use the same application identifier, account
configuration, keyring entries, and SQLite cache. The single-instance owner prevents both from
running simultaneously. Stop the daily unit before a dev session so the dev window—not the daily
build—owns activation:

```sh
systemctl --user stop cosmic-mail.service
npm run tauri dev
```

Restart `cosmic-mail.service` when the dev session ends.

## Hosted Arch repository

Every merge to `main` is built as an Arch package and published to a pacman repository
hosted on Backblaze B2. This exists to get builds onto machines other than this one; it
does not replace local promotion, and application self-update stays disabled.

### How it works

`.github/workflows/release.yml` runs the definition-of-done suite on every merge to
`main` — `npm run check`, `npm run build`, and `cargo check`, `clippy`, `test --lib` and
`fmt --check` in `src-tauri/`. Only if all of that passes does it send a
`repository_dispatch` to [PlusCosmic/packages], carrying the package name, this
repository, the merged commit SHA, and the version from `src-tauri/tauri.conf.json`.

This repository never writes to the package bucket, and it holds no `PKGBUILD`. Both
live in the packaging repository, which is the sole publisher. That is deliberate: the
pacman database is a read-modify-write over shared object storage, and GitHub's
`concurrency` groups are repository-scoped, so two projects publishing from their own
repositories could each read the old index and silently drop the other's package.
Funnelling every update through one repository gives one concurrency group that actually
serialises them.

The packaging repository builds from `git archive` of the dispatched SHA, so a package
can only contain committed source. It installs the same `--no-bundle` binary promotion
does, plus the desktop entry, the icons, and the systemd user unit under
`/usr/lib/systemd/user/`.

`pkgrel` is the packaging repository's Actions run number rather than a hand-maintained
counter, so each build is an upgrade to pacman even when the `tauri.conf.json` version is
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

### Interaction with the local daily install

A machine can have both, but they collide. `~/.local/share/applications/dev.pluscosmic.mail.desktop`
and `~/.config/systemd/user/cosmic-mail.service` shadow the packaged copies, and both
builds share one application identifier, account configuration, keyring entries, and
SQLite cache — the single-instance owner will stop the second one from starting. On a
machine that uses the package, remove the daily install rather than running both.

### Rehearsing a packaging change

`scripts/build-local.sh` in the packaging repository runs the same staging and `makepkg`
invocation the workflow does, against a local checkout of this one and without
publishing:

```sh
scripts/build-local.sh cosmic-mail ~/Projects/Mail
```

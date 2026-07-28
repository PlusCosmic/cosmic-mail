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

`.github/workflows/publish-arch-repo.yml` builds an Arch package on every merge to
`main` and publishes it to a pacman repository hosted on Backblaze B2. This exists to get
builds onto machines other than this one; it does not replace local promotion, and
application self-update stays disabled.

The package is built from `git archive HEAD`, so it contains committed source only. It
installs the same `--no-bundle` binary that promotion does, plus the desktop entry, the
icons, and the systemd user unit under `/usr/lib/systemd/user/`.

`pkgrel` is the GitHub Actions run number rather than a hand-maintained counter. It
increases on every merge, so each build is an upgrade to pacman even when
`src-tauri/tauri.conf.json`'s version has not changed. Bump that version for anything
users should recognise as a release.

### One-time setup

The repository lives in the public B2 bucket `pluscosmic-packages-bucket`, under the path
prefix `bx2fuafngntes`. Backblaze does not allow directory listing on public buckets, so
that prefix is what keeps the repository unlisted — the bucket name itself is guessable.
Treat the prefix as the secret and do not shorten it.

The bucket is shared by every Cosmic project: one pacman database indexes them all, so a
new application needs a workflow in its own repository pointing at the same `B2_PATH` and
no change on any client. Because GitHub's `concurrency` group cannot see the other
repositories, the publish step re-reads the uploaded index and redoes the merge if a
concurrent publish dropped its entry.

Add the B2 application key to each publishing repository as the `B2_KEY_ID` and
`B2_APP_KEY` secrets.

On each client, in `/etc/pacman.conf`:

```ini
[cosmic]
SigLevel = Optional TrustAll
Server = https://pluscosmic-packages-bucket.s3.eu-central-003.backblazeb2.com/bx2fuafngntes/$arch
```

```sh
sudo pacman -Sy cosmic-mail    # first install
sudo pacman -Syu               # every merge after that
```

Packages are unsigned, and `SigLevel` is scoped to this repository so it does not weaken
verification for `core` or `extra`. The tradeoff is explicit: obscurity hides the URL, but
anyone who obtains write access to the bucket can ship a package that runs as root on
every client. Signing is a `--sign` flag on `makepkg` and `repo-add` plus a GPG key in
Actions secrets if that stops being acceptable.

### Interaction with the local daily install

A machine can have both, but they collide. `~/.local/share/applications/dev.pluscosmic.mail.desktop`
and `~/.config/systemd/user/cosmic-mail.service` shadow the packaged copies, and both
builds share one application identifier, account configuration, keyring entries, and
SQLite cache — the single-instance owner will stop the second one from starting. On a
machine that uses the package, remove the daily install rather than running both.

### Rehearsing a packaging change

`packaging/build-local.sh` runs the same staging and `makepkg` invocation the workflow
does, without publishing. Use it before pushing a `PKGBUILD` change.

Nothing prunes old package files from the bucket. Add a cleanup once they accumulate.

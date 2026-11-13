# Cosmic Mail

Cosmic Mail is a native mail client for Windows, macOS and Linux. It combines a
keyboard-driven interface with desktop notifications and a local-first mail cache, and
looks and behaves the same on all three.

The application is written in Rust, with a native GPU-rendered interface built on
[Rune](https://github.com/anomalyco/rune) (no webview). Gmail and
password-based IMAP accounts share the same IMAP sync engine; Gmail authenticates with
OAuth 2.0 rather than an app password.

> [!NOTE]
> Cosmic Mail is under active development. Reading, syncing, composing, search,
> attachments, and the everyday message actions all work; see the open
> [issues](https://github.com/PlusCosmic/cosmic-mail/issues) for known gaps such as
> STARTTLS IMAP providers and full-history backfill.

## Highlights

- Gmail OAuth 2.0 (PKCE) and implicit-TLS IMAP account support
- Automatic account discovery: autoconfig, ISPDB, MX and SRV lookups
- Fast local SQLite cache with lazy message-body fetching
- IMAP IDLE for near-real-time inbox updates, with polling for servers without it
- Compose, reply, and reply-all over SMTP
- Full-text search across the local cache (SQLite FTS5)
- Attachment download and inline image rendering; remote images blocked by default
- Flag, archive, delete, and move messages
- Delivery-tracking chips for shipments detected in mail
- Unified inbox and account-specific folder views
- Vim-style keyboard navigation, command palette, and resizable three-pane layout
- Five colour schemes in light and dark from the PlusCosmic UI foundation, following the system or fixed
- Native new-mail notifications with sound on every platform (click-to-open on macOS and
  Linux), plus a background mode and a system tray (Windows and Linux)
- Passwords and refresh tokens stored in the platform keyring (Credential Manager,
  Keychain, Secret Service)

Open work is tracked in [GitHub issues](https://github.com/PlusCosmic/cosmic-mail/issues)
(labels `roadmap`, `verification`, `papercut`).

## Install

Every push to `main` publishes a
[GitHub Release](https://github.com/PlusCosmic/cosmic-mail/releases/latest) for Windows and
macOS, and an Arch Linux package to a hosted pacman repository
([RELEASING.md](docs/RELEASING.md)).

- **Windows**: a per-user installer (`*-setup.exe`, no administrator prompt, optional
  start at sign-in) and a portable zip. Builds are unsigned, so SmartScreen warns on first
  run (**More info → Run anyway**). Credentials live in Windows Credential Manager, and
  mail is cached under `%APPDATA%`.
- **macOS**: a disk image (`*-macos-universal.dmg`, Apple Silicon and Intel, macOS 13 or
  later); open it and drag Cosmic Mail to Applications. Builds are not notarized, so macOS
  blocks the first launch: open **System Settings → Privacy & Security** and choose
  **Open Anyway**. Credentials live in the login Keychain, and mail is cached under
  `~/Library/Application Support/cosmic-mail`. There is no menu-bar icon yet: closing the
  window keeps the app running in the Dock.
- **Linux**: the Arch package (with a systemd user service that keeps it running in the
  background), or build from source on any distribution. Credentials live in the Secret
  Service keyring, and mail is cached under `~/.local/share/cosmic-mail`.

## Try it from source

You need Rust (the pinned toolchain in `rust-toolchain.toml` installs itself through
rustup) and Python 3 on `PATH` (a build script needs it). Per platform:

- **macOS**: the Xcode command line tools (`xcode-select --install`).
- **Windows**: the MSVC build tools.
- **Linux**: `libxkbcommon`, `wayland` (or X11) and a Vulkan driver, plus a running Secret
  Service implementation; the [development guide](docs/DEVELOPMENT.md) lists the packages.

```sh
git clone https://github.com/PlusCosmic/cosmic-mail.git
cd cosmic-mail
cargo run
```

`cargo run -- --background` starts without a window (sync and notifications only). Gmail also
requires your own Google OAuth desktop client credentials. The
[setup guide](docs/SETUP.md) covers account configuration, local data, and keyboard
shortcuts.

## Development

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- --snap out.png --scene default   # headless render of the demo mailbox
```

The [development guide](docs/DEVELOPMENT.md) has the complete workflow and subsystem
test recipes. The [architecture contract](docs/ARCHITECTURE.md) documents every backend
operation, event, wire type, and database schema the Rune UI relies on.

## Documentation

- [Setup and usage](docs/SETUP.md)
- [Desktop integration](docs/DESKTOP.md)
- [Development guide](docs/DEVELOPMENT.md)
- [Architecture contract](docs/ARCHITECTURE.md)
- [Issues and roadmap](https://github.com/PlusCosmic/cosmic-mail/issues)
- [Project documentation index](docs/README.md)

## License

Copyright © 2026 Harry Leach. All rights reserved. The source is currently made
available for viewing only; no license to use, modify, or distribute it is granted.
See the [copyright notice](LICENSE) for details.

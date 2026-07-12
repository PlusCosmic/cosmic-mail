# Cosmic Mail

Cosmic Mail is a native Linux mail client built for the
[Omarchy](https://omarchy.org) desktop. It combines a keyboard-driven interface with
live Omarchy theming, desktop notifications, and a local-first mail cache.

The application is built with Tauri 2, Rust, Svelte 5, and TypeScript. Gmail and
password-based IMAP accounts share the same IMAP sync engine; Gmail authenticates with
OAuth 2.0 rather than an app password.

> [!WARNING]
> Cosmic Mail is under active development. Reading and syncing mail works, but sending,
> search, attachments, and several everyday message actions are not implemented yet.

## Highlights

- Gmail OAuth 2.0 and implicit-TLS IMAP account support
- Fast local SQLite cache with lazy message-body fetching
- IMAP IDLE for near-real-time inbox updates
- Unified inbox and account-specific folder views
- Vim-style keyboard navigation and resizable three-pane layout
- Live colour updates when the active Omarchy theme changes
- Native Mako notifications with click-to-focus support
- Passwords and refresh tokens stored in the Secret Service keyring

See the [roadmap](docs/ROADMAP.md) for the current implementation status and known
limitations.

## Try it from source

Cosmic Mail currently targets Omarchy on Arch Linux. You will need Node.js, Rust, the
Tauri development prerequisites, and a running Secret Service implementation.

```sh
git clone https://github.com/PlusCosmic/cosmic-mail.git
cd cosmic-mail
npm install
npm run tauri dev
```

Arch packages required by the application include `webkit2gtk-4.1`, `gtk3`, `librsvg`,
and `openssl`. Gmail also requires your own Google OAuth desktop client credentials.
The [setup guide](docs/SETUP.md) covers account configuration, local data, and keyboard
shortcuts.

## Development

```sh
npm run check
npm run build

cd src-tauri
cargo check
cargo clippy
cargo test --lib
cargo fmt --check
```

The [development guide](docs/DEVELOPMENT.md) has the complete workflow and subsystem
test recipes. The [architecture contract](docs/ARCHITECTURE.md) documents every command,
event, wire type, and database schema shared by the Rust backend and Svelte frontend.

## Documentation

- [Setup and usage](docs/SETUP.md)
- [Omarchy integration](docs/OMARCHY.md)
- [Development guide](docs/DEVELOPMENT.md)
- [Architecture contract](docs/ARCHITECTURE.md)
- [Roadmap and known limitations](docs/ROADMAP.md)
- [Project documentation index](docs/README.md)

## License

Copyright © 2026 Harry Leach. All rights reserved. The source is currently made
available for viewing only; no license to use, modify, or distribute it is granted.
See the [copyright notice](LICENSE) for details.

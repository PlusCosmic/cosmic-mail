# Cosmic Mail

Cosmic Mail is a native Linux mail client built for the
[Omarchy](https://omarchy.org) desktop. It combines a keyboard-driven interface with
live Omarchy theming, desktop notifications, and a local-first mail cache.

The application is built with Go, Wails 3, Svelte 5, and TypeScript. Gmail and
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
- Live colour updates when the active Omarchy theme changes
- Native Mako notifications with click-to-focus, plus a system tray and background mode
- Passwords and refresh tokens stored in the Secret Service keyring

Open work is tracked in [GitHub issues](https://github.com/PlusCosmic/cosmic-mail/issues)
(labels `roadmap`, `verification`, `papercut`).

## Try it from source

Cosmic Mail currently targets Omarchy on Arch Linux. You will need Go 1.25+, Node.js,
the `wails3` CLI, the `webkit2gtk-4.1` and `gtk3` development packages, and a running
Secret Service implementation.

```sh
git clone https://github.com/PlusCosmic/cosmic-mail.git
cd cosmic-mail
go install -tags gtk3 github.com/wailsapp/wails/v3/cmd/wails3@v3.0.0-beta.16
(cd frontend && npm install)
wails3 dev
```

Gmail also requires your own Google OAuth desktop client credentials. The
[setup guide](docs/SETUP.md) covers account configuration, local data, and keyboard
shortcuts.

## Development

The checks that gate a release, in the order the workflow runs them:

```sh
cd frontend && npm run check && npm run build && npm test && cd ..
wails3 generate bindings -ts -i -clean=true -d frontend/bindings -f "-tags gtk3"
gofmt -l .                      # must print nothing
go vet -tags gtk3 ./...
go test -tags gtk3 ./...
go build -tags gtk3,production -trimpath -ldflags="-w -s" -o bin/cosmic-mail .
```

`frontend/bindings` is generated from `app.go` and `internal/models` and is committed,
so `npm run check` works without a Go toolchain; regenerate it whenever the service or a
model changes (the workflow fails if it is stale). On Linux, Wails 3 defaults to
GTK4/WebKitGTK 6.0; every Go command here passes `gtk3` to link against webkit2gtk-4.1
instead, which is what the Arch package depends on.

The [development guide](docs/DEVELOPMENT.md) has the complete workflow and subsystem
test recipes. The [architecture contract](docs/ARCHITECTURE.md) documents every command,
event, wire type, and database schema shared by the Go backend and Svelte frontend.

## Documentation

- [Setup and usage](docs/SETUP.md)
- [Omarchy integration](docs/OMARCHY.md)
- [Development guide](docs/DEVELOPMENT.md)
- [Architecture contract](docs/ARCHITECTURE.md)
- [Issues and roadmap](https://github.com/PlusCosmic/cosmic-mail/issues)
- [Project documentation index](docs/README.md)

## License

Copyright © 2026 Harry Leach. All rights reserved. The source is currently made
available for viewing only; no license to use, modify, or distribute it is granted.
See the [copyright notice](LICENSE) for details.

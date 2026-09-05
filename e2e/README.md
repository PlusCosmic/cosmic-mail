# Scripted end-to-end tests

These tests drive the **real** Cosmic Mail UI — find DOM elements, click them,
assert on text — through a development-build-only automation bridge inside the app
(`internal/automation`), against a **hermetic fixture mailbox** instead of a real one.
No WebDriver: Arch's `webkit2gtk-4.1` ships no `WebKitWebDriver`, so the app opens a
loopback HTTP listener in builds without the `production` tag and evaluates JS in the
webview on request. The bridge and the test affordances below are compiled out of
production builds entirely.

## Pieces

- `docker-compose.yml` — a disposable **GreenMail** server: IMAPS on
  `localhost:3993` (serving our `localhost` cert), plaintext SMTP on `3025`,
  management API on `8080`, with the `test` user pre-created. CI uses this.
- `cmd/fakeimap` (repo root) — the Docker-free alternative: go-imap's in-memory
  server on `127.0.0.1:3993`, seeded with the same fixtures, with a self-signed
  certificate it writes wherever `-ca-out` points.
- `fixtures/mail/*.eml` — the seeded messages (fixed subjects/bodies, incl. a UPS
  shipment). `seed.mjs` delivers them over SMTP for GreenMail; `fakeimap` appends them
  directly.
- `fixtures/tls/` — a **committed, throwaway** `localhost` cert chain for GreenMail.
  `ca.pem` is trusted by the app (via `COSMIC_MAIL_EXTRA_CA`); `localhost.p12` is
  GreenMail's keystore. Test-only, protects nothing real; regenerate with `generate.sh`.
- `profile/accounts.json` — the isolated app profile's account, pointing at the
  fixture. Copied into a throwaway `$XDG_CONFIG_HOME` so a run never touches your
  real accounts or mail DB.
- `client.mjs` — zero-dep `Bridge` client. `*.test.mjs` — Node's built-in test
  runner (`node --test`), run through `npm run e2e` in `frontend/`.

The sync engine itself is also covered without any of this: `go test -tags gtk3
./internal/sync/` runs it against the in-memory server in-process.

## Development-only env hooks (inert unless set, compiled out of production)

| Env var | Effect |
|---|---|
| `COSMIC_MAIL_EXTRA_CA` | Add this PEM's certs to the TLS trust store (trust the fixture's CA). |
| `COSMIC_MAIL_TEST_IMAP_PASSWORD` | Return this as the IMAP password instead of the keyring (no Secret Service needed). |
| `COSMIC_MAIL_AUTOMATION_PORT` | Bridge port (default `4127`). |

## Running locally

Build a development binary first (a production build has no bridge):

```sh
(cd frontend && npm run build) && go build -tags gtk3 -o bin/cosmic-mail .
```

Without Docker, using the in-memory fixture server:

```sh
go run ./cmd/fakeimap -ca-out /tmp/fakeimap-ca.pem &   # IMAPS on 127.0.0.1:3993
COSMIC_MAIL_EXTRA_CA=/tmp/fakeimap-ca.pem dbus-run-session -- e2e/ci-run.sh
kill %1
```

With Docker (what CI does):

```sh
cd frontend && npm run e2e:env:up && cd ..
dbus-run-session -- e2e/ci-run.sh    # isolated bus: no clash with the daily
cd frontend && npm run e2e:env:down
```

`dbus-run-session` matters for the tray and notifications only: on an isolated bus there
is no StatusNotifier host (the app logs a tray error and carries on) and no mako. The
single-instance owner is a different D-Bus name from the Rust daily's, so the two never
collide, but a running daily would still sync the same real accounts — stop it when
testing ownership.

For an interactive dev-server session instead (hot reload, real terminal logs),
stop the daily and drive it manually:

```sh
systemctl --user stop cosmic-mail.service

# 1. Start the fixture (either flavour above).

# 2. Launch the app against an isolated profile pointed at the fixture
#    (port 9245 must be free). Run in its own terminal and leave it up:
PROF="$(mktemp -d)"; mkdir -p "$PROF/config/cosmic-mail" "$PROF/data"
cp e2e/profile/accounts.json "$PROF/config/cosmic-mail/accounts.json"
XDG_CONFIG_HOME="$PROF/config" XDG_DATA_HOME="$PROF/data" \
  COSMIC_MAIL_EXTRA_CA="$PWD/e2e/fixtures/tls/ca.pem" \
  COSMIC_MAIL_TEST_IMAP_PASSWORD="test-pass" \
  wails3 dev

# 3. Run the suite against it (in another terminal).
(cd frontend && npm run e2e)

# 4. Tear the fixture down; restart the promoted service.
systemctl --user start cosmic-mail.service
```

CI runs the GreenMail one-shot under `dbus-run-session -- xvfb-run` — see
`.github/workflows/e2e.yml`.

## Notes

- **Disposable inbox.** Tests may freely open/mark messages; the fixture is wiped on
  teardown. No real mailbox is ever touched (isolated XDG profile).
- **Async UI waits** are client-side poll loops (`bridge.waitFor`) — WebKitGTK
  evaluates each snippet synchronously and does not await a returned Promise.

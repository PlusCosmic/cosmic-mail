# Scripted end-to-end tests

These tests drive the **real** Cosmic Mail UI — find DOM elements, click them,
assert on text — through a debug-only automation bridge inside the app
(`src-tauri/src/automation.rs`), against a **hermetic GreenMail fixture** instead
of a real mailbox. No WebDriver: Arch's `webkit2gtk-4.1` ships no
`WebKitWebDriver`, so the app opens a loopback HTTP listener in `debug_assertions`
builds and evaluates JS in the webview on request. The bridge and the test
affordances below are compiled out of release builds entirely.

## Pieces

- `docker-compose.yml` — a disposable **GreenMail** server: IMAPS on
  `localhost:3993` (serving our `localhost` cert), plaintext SMTP on `3025`,
  management API on `8080`, with the `test` user pre-created.
- `fixtures/mail/*.eml` — the seeded messages (fixed subjects/bodies, incl. a UPS
  shipment). `seed.mjs` delivers them over SMTP; GreenMail files them into INBOX.
- `fixtures/tls/` — a **committed, throwaway** `localhost` cert chain. `ca.pem` is
  trusted by the app (via `COSMIC_MAIL_EXTRA_CA`); `localhost.p12` is GreenMail's
  keystore. Test-only, protects nothing real; regenerate with `generate.sh`.
- `profile/accounts.json` — the isolated app profile's account, pointing at the
  fixture. Copied into a throwaway `$XDG_CONFIG_HOME` so a run never touches your
  real accounts or mail DB.
- `client.mjs` — zero-dep `Bridge` client. `*.test.mjs` — Node's built-in test
  runner (`node --test`).

## Debug-only env hooks (inert unless set, debug builds only)

| Env var | Effect |
|---|---|
| `COSMIC_MAIL_EXTRA_CA` | Add this PEM's certs to the TLS trust store (trust the fixture's CA). |
| `COSMIC_MAIL_TEST_IMAP_PASSWORD` | Return this as the IMAP password instead of the keyring (no Secret Service needed). |
| `COSMIC_MAIL_AUTOMATION_PORT` | Bridge port (default `4127`). |

## Running locally

Prerequisite: Docker. The promoted daily owns the app's D-Bus name, so stop it first.

```sh
systemctl --user stop cosmic-mail.service        # release the single-instance name

# 1. Start + seed the fixture mail server.
npm run e2e:env:up

# 2. Launch a debug build against an isolated profile pointed at the fixture.
#    (port 1420 must be free). Run in its own terminal and leave it up:
PROF="$(mktemp -d)"; mkdir -p "$PROF/config/cosmic-mail" "$PROF/data"
cp e2e/profile/accounts.json "$PROF/config/cosmic-mail/accounts.json"
XDG_CONFIG_HOME="$PROF/config" XDG_DATA_HOME="$PROF/data" \
  COSMIC_MAIL_EXTRA_CA="$PWD/e2e/fixtures/tls/ca.pem" \
  COSMIC_MAIL_TEST_IMAP_PASSWORD="test-pass" \
  npm run tauri dev

# 3. Run the suite against it (in another terminal).
npm run e2e

# 4. Tear the fixture down; restart the promoted service.
npm run e2e:env:down
systemctl --user start cosmic-mail.service
```

`e2e/ci-run.sh` automates steps 2–3 in one shot against a prebuilt debug binary —
that's what CI uses (see `.github/workflows/e2e.yml`).

## Notes

- **Disposable inbox.** Tests may freely open/mark messages; `e2e:env:down` wipes
  everything. No real mailbox is ever touched (isolated XDG profile).
- **Async UI waits** are client-side poll loops (`bridge.waitFor`) — WebKitGTK
  evaluates each snippet synchronously and does not await a returned Promise.

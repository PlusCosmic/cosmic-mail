# Scripted end-to-end tests

`tests/e2e_greenmail.rs` drives the **real** Cosmic Mail UI over the **real** backend
against a **hermetic GreenMail fixture** instead of a real mailbox. Nothing is mocked
between the keystrokes and IMAP: the test builds the app's `Backend` and `Ui`, runs the
actual Rune view on Rune's headless `Runtime`, and clicks, types and reads the element
tree the way a person would (`Runtime::find`, `pointer_down`/`up`, `key`, `elements`).
No window, display, GPU or WebDriver is needed.

It covers: the fixture inbox syncing into the unified list, opening a message (body
fetched over IMAP, marked read), the UPS shipment chip with its tracking number, a flag
toggle surviving a refetch from the database, an optimistic delete rolling back when the
server refuses it (the fixture has no Trash folder), full-cache search, the account's
folder column, and the command palette.

## Pieces

- `docker-compose.yml` — a disposable **GreenMail** server: IMAPS on
  `localhost:3993` (serving our `localhost` cert), plaintext SMTP on `3025`,
  management API on `8080`, with the `test` user pre-created.
- `fixtures/mail/*.eml` — the seeded messages (fixed subjects/bodies, incl. a UPS
  shipment). `seed.mjs` (zero-dependency Node) delivers them over SMTP; GreenMail
  files them into INBOX.
- `fixtures/tls/` — a **committed, throwaway** `localhost` cert chain. `ca.pem` is
  trusted by the app (via `COSMIC_MAIL_EXTRA_CA`); `localhost.p12` is GreenMail's
  keystore. Test-only, protects nothing real; regenerate with `generate.sh`.
- `profile/accounts.json` — the isolated app profile's account, pointing at the
  fixture. The test copies it into a throwaway `$XDG_CONFIG_HOME` (with a throwaway
  `$XDG_DATA_HOME`) so a run never touches your real accounts or mail DB.

## Debug-only env hooks (inert unless set, debug builds only)

| Env var | Effect |
|---|---|
| `COSMIC_MAIL_EXTRA_CA` | Add this PEM's certs to the TLS trust store (trust the fixture's CA). |
| `COSMIC_MAIL_TEST_IMAP_PASSWORD` | Return this as the IMAP password instead of the keyring (no Secret Service needed). |

The test sets both itself.

## Running locally

Prerequisites: Docker and Node (for the seeder).

```sh
docker compose -f e2e/docker-compose.yml up -d
node e2e/seed.mjs
cargo test --test e2e_greenmail -- --ignored
docker compose -f e2e/docker-compose.yml down -v
```

The test is `#[ignore]`d so a plain `cargo test` never needs the fixture. It runs in its
own process with an isolated profile, so it doesn't clash with a running Cosmic Mail.
Re-running against the same fixture works (it toggles the flag rather than asserting an
absolute state); `down -v` resets the mailbox.

To watch the real window against the fixture instead, run the binary with the same
environment:

```sh
PROF="$(mktemp -d)"; mkdir -p "$PROF/config/cosmic-mail" "$PROF/data"
cp e2e/profile/accounts.json "$PROF/config/cosmic-mail/accounts.json"
XDG_CONFIG_HOME="$PROF/config" XDG_DATA_HOME="$PROF/data" \
  COSMIC_MAIL_EXTRA_CA="$PWD/e2e/fixtures/tls/ca.pem" \
  COSMIC_MAIL_TEST_IMAP_PASSWORD="test-pass" \
  cargo run
```

(Stop `cosmic-mail.service` first if it runs: a second launch otherwise just activates
the running instance and exits.) CI runs the fixture and the test — see
`.github/workflows/e2e.yml`.

## Notes

- **Disposable inbox.** Tests may freely open/mark messages; `down -v` wipes
  everything. No real mailbox is ever touched (isolated XDG profile).
- **Async waits** are poll loops: each `Harness::tick` runs `Ui::poll` (landing backend
  results) and a frame, until a condition on the state holds or 90 s pass. The failure
  message includes any toasts, which carry backend errors.
- **Sending is not covered**: the SMTP client requires STARTTLS (or implicit TLS on 465),
  and the fixture's SMTP port is plaintext.

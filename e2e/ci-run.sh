#!/usr/bin/env bash
# One-shot E2E orchestrator: launch a prebuilt development app binary against
# an isolated profile pointed at the fixture IMAP server, wait for the bridge,
# run the test suite, and tear the app down. Used by CI (under dbus-run-session
# + xvfb-run); also runnable locally once you've built the binary with
# `wails3 task build DEV=true` (a production build has no automation bridge).
#
# Assumes the fixture is already up + seeded: either GreenMail
# (`npm run e2e:env:up` in frontend/) or the Docker-free `go run ./cmd/fakeimap
# -ca-out <path>` with COSMIC_MAIL_EXTRA_CA pointed at that path.
set -euo pipefail
cd "$(dirname "$0")/.."   # repo root

APP_BIN="${APP_BIN:-bin/cosmic-mail}"
PORT="${COSMIC_MAIL_AUTOMATION_PORT:-4127}"

if [ ! -x "$APP_BIN" ]; then
  echo "app binary not found at $APP_BIN — build it with 'wails3 task build DEV=true'" >&2
  exit 1
fi

# Isolated, throwaway profile so the run never touches real accounts or mail DB.
PROF="$(mktemp -d)"
mkdir -p "$PROF/config/cosmic-mail" "$PROF/data"
cp e2e/profile/accounts.json "$PROF/config/cosmic-mail/accounts.json"

APP_PID=""
cleanup() {
  [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null || true
  rm -rf "$PROF"
}
trap cleanup EXIT

XDG_CONFIG_HOME="$PROF/config" \
XDG_DATA_HOME="$PROF/data" \
COSMIC_MAIL_EXTRA_CA="${COSMIC_MAIL_EXTRA_CA:-$PWD/e2e/fixtures/tls/ca.pem}" \
COSMIC_MAIL_TEST_IMAP_PASSWORD="test-pass" \
COSMIC_MAIL_AUTOMATION_PORT="$PORT" \
  "$APP_BIN" &
APP_PID=$!

echo "· waiting for the automation bridge (pid $APP_PID)…"
for _ in $(seq 1 60); do
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "app process exited during startup" >&2
    exit 1
  fi
  if node -e "fetch('http://127.0.0.1:${PORT}/health').then(r=>process.exit(r.ok?0:1)).catch(()=>process.exit(1))" 2>/dev/null; then
    echo "· bridge ready"
    break
  fi
  sleep 1
done

(cd frontend && npm run e2e)

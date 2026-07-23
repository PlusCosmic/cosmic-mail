#!/usr/bin/env bash
# One-shot E2E orchestrator: launch a prebuilt debug app binary against an
# isolated profile pointed at the GreenMail fixture, wait for the bridge, run
# the test suite, and tear the app down. Used by CI (under dbus-run-session +
# xvfb-run); also runnable locally once you've built the binary with
# `npm run tauri build -- --debug --no-bundle`.
#
# Assumes the fixture is already up + seeded (`npm run e2e:env:up`).
set -euo pipefail
cd "$(dirname "$0")/.."   # repo root

APP_BIN="${APP_BIN:-src-tauri/target/debug/cosmic-mail}"
PORT="${COSMIC_MAIL_AUTOMATION_PORT:-4127}"

if [ ! -x "$APP_BIN" ]; then
  echo "app binary not found at $APP_BIN — build it with 'npm run tauri build -- --debug --no-bundle'" >&2
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
COSMIC_MAIL_EXTRA_CA="$PWD/e2e/fixtures/tls/ca.pem" \
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

npm run e2e

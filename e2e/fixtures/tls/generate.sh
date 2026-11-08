#!/usr/bin/env bash
# Regenerate the throwaway TLS fixture for the GreenMail E2E mail server.
#
# These are TEST-ONLY credentials for a local `localhost` IMAPS fixture. The key
# protects nothing real and is trusted only when a DEBUG build is explicitly
# pointed at ca.pem via COSMIC_MAIL_EXTRA_CA. Committed on purpose so E2E runs
# are deterministic and need no cert tooling at run time (see e2e/README.md).
#
# A proper CA -> leaf chain is used (not a bare self-signed cert): rustls/webpki
# validates the leaf against the trusted CA and requires a serverAuth EKU + SAN.
#
# Outputs (committed): ca.pem (app trusts this), localhost.p12 (GreenMail serves
# this; password "changeit"). Run from this directory: ./generate.sh
set -euo pipefail
cd "$(dirname "$0")"

DAYS=36500                      # ~100 years; the fixture must never rot
P12_PASS=changeit               # matches e2e/docker-compose.yml GREENMAIL_OPTS
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# 1. A self-signed CA.
openssl req -x509 -newkey rsa:2048 -nodes -days "$DAYS" \
  -keyout "$tmp/ca.key" -out "$tmp/ca.crt" \
  -subj "/CN=Cosmic Mail E2E Test CA" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign"

# 2. A leaf cert for localhost, signed by the CA, with a serverAuth EKU + SAN.
openssl req -newkey rsa:2048 -nodes \
  -keyout "$tmp/localhost.key" -out "$tmp/localhost.csr" \
  -subj "/CN=localhost"
openssl x509 -req -in "$tmp/localhost.csr" -days "$DAYS" \
  -CA "$tmp/ca.crt" -CAkey "$tmp/ca.key" -CAcreateserial \
  -extfile <(printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\nextendedKeyUsage=serverAuth\nbasicConstraints=CA:FALSE\n') \
  -out "$tmp/localhost.crt"

# 3. Emit the two committed artifacts: the CA (app trust) and the leaf keystore
#    with the CA in its chain (GreenMail server).
cp "$tmp/ca.crt" ca.pem
openssl pkcs12 -export -name greenmail \
  -inkey "$tmp/localhost.key" -in "$tmp/localhost.crt" -certfile "$tmp/ca.crt" \
  -passout "pass:$P12_PASS" -out localhost.p12

# The GreenMail container runs as a non-root user and mounts these read-only;
# make them world-readable so it can load the keystore. Test-only material.
chmod 644 ca.pem localhost.p12

echo "Wrote ca.pem and localhost.p12"

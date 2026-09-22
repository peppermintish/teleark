#!/bin/bash
# Validate the release secret early, without logging private material.
set +x
set -euo pipefail
umask 077
root="$(cd "$(dirname "$0")/.." && pwd)"
stage="$(mktemp -d -t teleark-sign-check.XXXXXXXX)"
trap 'rm -rf "$stage"' EXIT
if [[ -z "${TELEARK_MACOS_SIGNING_P12_BASE64:-}" ]]; then
  echo 'Missing TELEARK_MACOS_SIGNING_P12_BASE64 release secret.' >&2; exit 1
fi
printf '%s' "$TELEARK_MACOS_SIGNING_P12_BASE64" | openssl base64 -d -A > "$stage/identity.p12"
unset TELEARK_MACOS_SIGNING_P12_BASE64
openssl pkcs12 -legacy -in "$stage/identity.p12" -clcerts -nokeys -passin pass: -out "$stage/certificate.pem" 2>/dev/null
expected="$(openssl x509 -in "$root/scripts/macos/release-certificate.pem" -noout -fingerprint -sha256)"
actual="$(openssl x509 -in "$stage/certificate.pem" -noout -fingerprint -sha256)"
[[ "$expected" == "$actual" ]] || { echo 'Release identity does not match the pinned public certificate.' >&2; exit 1; }
openssl x509 -in "$stage/certificate.pem" -checkend 2592000 -noout >/dev/null
# Verify the private key belongs to this certificate without printing either key.
openssl pkcs12 -legacy -in "$stage/identity.p12" -nocerts -noenc -passin pass: -out "$stage/private.pem" 2>/dev/null
openssl pkey -in "$stage/private.pem" -pubout -out "$stage/key-public.pem" 2>/dev/null
openssl x509 -in "$stage/certificate.pem" -pubkey -noout > "$stage/cert-public.pem"
cmp -s "$stage/key-public.pem" "$stage/cert-public.pem"
echo 'macOS release signing identity is valid, current and matches the pinned certificate.'

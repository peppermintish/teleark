#!/bin/bash
# Reuse the pinned self-signed identity; never generate a new release identity here.
set +x
set -euo pipefail
umask 077
app="${1:?App bundle required}"
standalone="${2:?Standalone output required}"
repository_root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d -t teleark-sign)"
cleanup() {
  security delete-keychain "$stage/signing.keychain-db" >/dev/null 2>&1 || true
  rm -rf "$stage"
}
trap cleanup EXIT
if [[ -n "${TELEARK_MACOS_SIGNING_P12_BASE64:-}" ]]; then
  printf '%s' "$TELEARK_MACOS_SIGNING_P12_BASE64" | /usr/bin/base64 -D > "$stage/identity.p12"
elif [[ -n "${TELEARK_MACOS_SIGNING_P12:-}" && -f "$TELEARK_MACOS_SIGNING_P12" ]]; then
  cp "$TELEARK_MACOS_SIGNING_P12" "$stage/identity.p12"
else
  echo 'Missing persistent macOS signing identity. Configure TELEARK_MACOS_SIGNING_P12_BASE64 in CI or TELEARK_MACOS_SIGNING_P12 locally.' >&2
  exit 1
fi
unset TELEARK_MACOS_SIGNING_P12_BASE64
chmod 600 "$stage/identity.p12"
# Empty temporary password carries no secret in process arguments; directory is 0700.
security create-keychain -p '' "$stage/signing.keychain-db"
security unlock-keychain -p '' "$stage/signing.keychain-db"
# Import PEM material because Apple's PKCS#12 decoder rejects some OpenSSL envelopes.
openssl pkcs12 -legacy -in "$stage/identity.p12" -nocerts -noenc -passin pass: -out "$stage/private.pem"
openssl pkcs12 -legacy -in "$stage/identity.p12" -clcerts -nokeys -passin pass: -out "$stage/public.pem"
openssl rsa -in "$stage/private.pem" -traditional -out "$stage/import-key.pem" 2>/dev/null
security import "$stage/import-key.pem" -k "$stage/signing.keychain-db" -T /usr/bin/codesign >/dev/null
security import "$stage/public.pem" -k "$stage/signing.keychain-db" >/dev/null
security set-key-partition-list -S apple-tool:,apple: -s -k '' "$stage/signing.keychain-db" >/dev/null
certificate="$repository_root/scripts/macos/release-certificate.pem"
fingerprint="$(openssl x509 -in "$certificate" -noout -fingerprint -sha1 | cut -d= -f2 | tr -d ':')"
requirement="designated => identifier \"app.teleark.desktop\" and certificate leaf = H\"$fingerprint\""
codesign --force --sign "$fingerprint" --keychain "$stage/signing.keychain-db" \
  --identifier app.teleark.desktop --requirements "=$requirement" --timestamp=none "$app"
codesign --verify --strict --verbose=2 "$app"
# Preserve the same signing identity in both distribution forms.
cp "$app/Contents/MacOS/teleark" "$standalone"
codesign --force --sign "$fingerprint" --keychain "$stage/signing.keychain-db" \
  --identifier app.teleark.desktop --requirements "=$requirement" --timestamp=none "$standalone"
codesign --verify --strict --verbose=2 "$standalone"
codesign --verify -R "=identifier \"app.teleark.desktop\" and certificate leaf = H\"$fingerprint\"" "$app"
printf 'Verified stable TeleArk signing identity for app and standalone executable.\n'

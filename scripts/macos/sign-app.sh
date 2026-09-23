#!/bin/bash
# Reuse the pinned self-signed identity; never generate a new release identity here.
set +x
set -euo pipefail
umask 077
app="${1:?App bundle required}"
standalone="${2:?Standalone output required}"
repository_root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d -t teleark-sign)"
keychain="$stage/signing.keychain-db"
saved_keychains=()
restore_keychain_list=0
cleanup() {
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  if [[ "$restore_keychain_list" == 1 ]]; then
    security list-keychains -d user -s "${saved_keychains[@]}" >/dev/null 2>&1 || \
      echo 'macOS signing: could not restore the prior Keychain search list.' >&2
  fi
  rm -rf "$stage"
}
trap cleanup EXIT
keychain_list="$(security list-keychains -d user)"
while IFS= read -r listed_keychain; do
  listed_keychain="${listed_keychain#"${listed_keychain%%[![:space:]]*}"}"
  [[ -n "$listed_keychain" ]] || continue
  if [[ "$listed_keychain" != \"*\" ]]; then
    echo 'Cannot parse the current macOS Keychain search list.' >&2
    exit 1
  fi
  listed_keychain="${listed_keychain#\"}"
  listed_keychain="${listed_keychain%\"}"
  saved_keychains+=("$listed_keychain")
done <<< "$keychain_list"
phase() {
  printf 'macOS signing: %s\n' "$1"
}
if [[ -n "${TELEARK_MACOS_SIGNING_P12_BASE64:-}" ]]; then
  phase 'decoding the CI signing identity'
  printf '%s' "$TELEARK_MACOS_SIGNING_P12_BASE64" | openssl base64 -d -A > "$stage/identity.p12"
elif [[ -n "${TELEARK_MACOS_SIGNING_P12:-}" && -f "$TELEARK_MACOS_SIGNING_P12" ]]; then
  phase 'copying the local signing identity'
  cp "$TELEARK_MACOS_SIGNING_P12" "$stage/identity.p12"
else
  echo 'Missing persistent macOS signing identity. Configure TELEARK_MACOS_SIGNING_P12_BASE64 in CI or TELEARK_MACOS_SIGNING_P12 locally.' >&2
  exit 1
fi
unset TELEARK_MACOS_SIGNING_P12_BASE64
chmod 600 "$stage/identity.p12"
certificate="$repository_root/scripts/macos/release-certificate.pem"
fingerprint="$(openssl x509 -in "$certificate" -noout -fingerprint -sha1 | cut -d= -f2 | tr -d ':')"
[[ -n "$fingerprint" ]] || { echo 'The pinned macOS signing certificate has no SHA-1 fingerprint.' >&2; exit 1; }

create_keychain() {
  # Empty temporary password carries no secret in process arguments; directory is 0700.
  security create-keychain -p '' "$keychain" >/dev/null
  security set-keychain-settings -lut 21600 "$keychain" >/dev/null
  security unlock-keychain -p '' "$keychain" >/dev/null
}

identity_summary() {
  # -v filters out untrusted self-signed identities, even when their key is present.
  # Pin the certificate/key pair here; codesign verifies the resulting signatures below.
  security find-identity -p codesigning "$keychain" 2>&1 || true
}

verify_identity() {
  local summary
  summary="$(identity_summary)"
  if ! grep -Fqi "$fingerprint" <<< "$summary"; then
    echo 'The temporary keychain does not contain the pinned macOS signing identity.' >&2
    printf '%s\n' "$summary" >&2
    return 1
  fi
  printf 'macOS signing: verified identity %s in the temporary keychain\n' "$fingerprint"
}

import_pem_identity() {
  phase 'importing the identity through explicit PEM formats'
  # The PEM path handles older macOS releases that reject some OpenSSL PKCS#12 envelopes.
  openssl pkcs12 -legacy -in "$stage/identity.p12" -nocerts -noenc -passin pass: -out "$stage/private.pem"
  openssl pkcs12 -legacy -in "$stage/identity.p12" -clcerts -nokeys -passin pass: -out "$stage/public-with-attributes.pem"
  openssl rsa -in "$stage/private.pem" -traditional -out "$stage/import-key.pem" 2>/dev/null
  openssl x509 -in "$stage/public-with-attributes.pem" -out "$stage/public.pem"
  # Apple's certtool imports the certificate and its private key as one identity.
  # Separate security imports can be listed by find-identity but fail in codesign.
  certtool i "$stage/public.pem" "k=$keychain" "r=$stage/import-key.pem" >/dev/null
}

phase 'creating the isolated temporary keychain'
create_keychain
phase 'importing the PKCS#12 signing identity'
if security import "$stage/identity.p12" -f pkcs12 -P '' -k "$keychain" -T /usr/bin/codesign >/dev/null 2>&1; then
  verify_identity
else
  phase "PKCS#12 import failed (status $?); retrying with PEM"
  security delete-keychain "$keychain" >/dev/null 2>&1 || true
  create_keychain
  import_pem_identity
  verify_identity
fi

phase 'granting codesign partition access to the private key'
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k '' "$keychain" >/dev/null
phase 'registering the temporary keychain for codesign'
restore_keychain_list=1
security list-keychains -d user -s "$keychain" "${saved_keychains[@]}" >/dev/null

requirement="designated => identifier \"app.teleark.desktop\" and certificate leaf = H\"$fingerprint\""
phase 'signing the app bundle'
codesign --force --sign "$fingerprint" --keychain "$keychain" \
  --identifier app.teleark.desktop --requirements "=$requirement" --timestamp=none "$app"
codesign --verify --strict --verbose=2 "$app"
# Preserve the same signing identity in both distribution forms.
phase 'signing the standalone executable'
cp "$app/Contents/MacOS/teleark" "$standalone"
codesign --force --sign "$fingerprint" --keychain "$keychain" \
  --identifier app.teleark.desktop --requirements "=$requirement" --timestamp=none "$standalone"
codesign --verify --strict --verbose=2 "$standalone"
codesign --verify -R "=identifier \"app.teleark.desktop\" and certificate leaf = H\"$fingerprint\"" "$app"
printf 'Verified stable TeleArk signing identity for app and standalone executable.\n'

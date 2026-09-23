#!/bin/bash
# Native signing regression with a fresh, untrusted identity and synthetic files only.
set +x
set -euo pipefail
umask 077
root="$(cd "$(dirname "$0")/.." && pwd)"
original_keychain_list="$(security list-keychains -d user)"
stage="$(mktemp -d -t teleark-sign-import-test)"
trap 'rm -rf "$stage"' EXIT
fixture="$stage/repository"
mkdir -p "$fixture/scripts/macos" "$fixture/crates/teleark-gui/assets/macos" "$stage/bin"
cp "$root/scripts/macos/sign-app.sh" "$root/scripts/macos/verify-keychain-signing.sh" \
  "$root/scripts/macos/keychain-signing-probe.swift" "$fixture/scripts/macos/"
cp "$root/scripts/test-macos-signing.sh" "$fixture/scripts/"
cp "$root/crates/teleark-gui/assets/macos/Info.plist" "$fixture/crates/teleark-gui/assets/macos/"
bash "$root/scripts/macos/create-signing-identity.sh" "$stage/identity"
cp "$stage/identity/certificate.pem" "$fixture/scripts/macos/release-certificate.pem"
unset TELEARK_MACOS_SIGNING_P12_BASE64
export TELEARK_MACOS_SIGNING_P12="$stage/identity/identity.p12"

# An untrusted self-signed certificate must still provide a pinned signing identity.
if security verify-cert -c "$stage/identity/certificate.pem" -p codeSign -N -L >"$stage/trust.log" 2>&1; then
  echo 'Synthetic certificate unexpectedly has system trust.' >&2
  exit 1
fi
bash "$fixture/scripts/test-macos-signing.sh"
bash "$fixture/scripts/macos/verify-keychain-signing.sh"

# Force the compatibility path even on hosts whose PKCS#12 import succeeds.
cat > "$stage/bin/security" <<'SH'
#!/bin/bash
if [[ "$1" == import && "$2" == *.p12 ]]; then
  exit 1
fi
exec /usr/bin/security "$@"
SH
chmod +x "$stage/bin/security"
PATH="$stage/bin:$PATH" bash "$fixture/scripts/test-macos-signing.sh" >"$stage/fallback.log" 2>&1
grep -Fq 'importing the identity through explicit PEM formats' "$stage/fallback.log"
grep -Fq 'Verified stable update identity, standalone code equivalence and tamper rejection.' "$stage/fallback.log"

# Importing a complete but different identity must fail before signing.
bash "$root/scripts/macos/create-signing-identity.sh" "$stage/other-identity"
if TELEARK_MACOS_SIGNING_P12="$stage/other-identity/identity.p12" \
  bash "$fixture/scripts/test-macos-signing.sh" >"$stage/mismatch.log" 2>&1; then
  echo 'Signing accepted an identity that did not match the pinned certificate.' >&2
  exit 1
fi
grep -Fq 'does not contain the pinned macOS signing identity' "$stage/mismatch.log"
if grep -Fq 'signing the app bundle' "$stage/mismatch.log"; then
  echo 'Signing started before rejecting the mismatched identity.' >&2
  exit 1
fi
if [[ "$(security list-keychains -d user)" != "$original_keychain_list" ]]; then
  echo 'Signing did not restore the original Keychain search list.' >&2
  exit 1
fi
echo 'PASS: untrusted pinned identity, PEM fallback, Keychain continuity and identity mismatch.'

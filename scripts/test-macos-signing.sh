#!/bin/bash
# Native, synthetic signing regression. Does not open the login Keychain or user data.
set +x
set -euo pipefail
stage="$(mktemp -d -t teleark-sign-test)"
trap 'rm -rf "$stage"' EXIT
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$stage/TeleArk.app/Contents/MacOS"
cp /bin/echo "$stage/TeleArk.app/Contents/MacOS/teleark"
cp "$root/crates/teleark-gui/assets/macos/Info.plist" "$stage/TeleArk.app/Contents/Info.plist"
"$root/scripts/macos/sign-app.sh" "$stage/TeleArk.app" "$stage/teleark.bin"
codesign -d -r- "$stage/TeleArk.app" > "$stage/first-requirement" 2>&1
# An update changes sealed resources but must retain the same designated requirement.
/usr/libexec/PlistBuddy -c 'Set :CFBundleVersion 99.0.0' "$stage/TeleArk.app/Contents/Info.plist"
"$root/scripts/macos/sign-app.sh" "$stage/TeleArk.app" "$stage/teleark.bin"
codesign -d -r- "$stage/TeleArk.app" > "$stage/second-requirement" 2>&1
diff "$stage/first-requirement" "$stage/second-requirement"
cp "$stage/TeleArk.app/Contents/MacOS/teleark" "$stage/bundle-code"
cp "$stage/teleark.bin" "$stage/standalone-code"
codesign --remove-signature "$stage/bundle-code"
codesign --remove-signature "$stage/standalone-code"
cmp "$stage/bundle-code" "$stage/standalone-code"
# An altered resource must fail; the pinned signer is not permission to change bytes.
printf 'tampered' >> "$stage/TeleArk.app/Contents/Info.plist"
if codesign --verify --strict "$stage/TeleArk.app" >/dev/null 2>&1; then
  echo 'Tampered app unexpectedly verified.' >&2; exit 1
fi
printf 'Verified stable update identity, standalone code equivalence and tamper rejection.\n'

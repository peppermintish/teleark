#!/bin/bash
# Real native Keychain ACL qualification with exclusively temporary synthetic data.
set +x
set -euo pipefail
umask 077
test "$(uname -s)" = Darwin || { echo 'This qualification requires native macOS.' >&2; exit 1; }
repository_root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$(mktemp -d -t teleark-keychain-qualification)"
cleanup() {
  security delete-keychain "$stage/qualification.keychain-db" >/dev/null 2>&1 || true
  rm -rf "$stage"
}
trap cleanup EXIT
for build in first second; do
  app="$stage/$build.app"
  mkdir -p "$app/Contents/MacOS"
  cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>app.teleark.desktop</string>
<key>CFBundleExecutable</key><string>teleark</string>
<key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>
PLIST
  if [[ "$build" = second ]]; then
    xcrun swiftc -suppress-warnings -D PROBE_TWO "$repository_root/scripts/macos/keychain-signing-probe.swift" -o "$app/Contents/MacOS/teleark"
  else
    xcrun swiftc -suppress-warnings "$repository_root/scripts/macos/keychain-signing-probe.swift" -o "$app/Contents/MacOS/teleark"
  fi
  "$repository_root/scripts/macos/sign-app.sh" "$app" "$stage/$build-standalone"
done
# The builds must differ, while preserving their explicit designated requirement.
if cmp -s "$stage/first-standalone" "$stage/second-standalone"; then
  echo 'Qualification builds unexpectedly have identical bytes.' >&2
  exit 1
fi
mv "$stage/first.app" "$stage/installed.app"
"$stage/installed.app/Contents/MacOS/teleark" create "$stage/qualification.keychain-db"
rm -rf "$stage/installed.app"
mv "$stage/second.app" "$stage/installed.app"
"$stage/installed.app/Contents/MacOS/teleark" read "$stage/qualification.keychain-db"
cp -R "$stage/installed.app" "$stage/restored.app"
# Preserve the public identifier but remove the pinned certificate identity.
codesign --force --sign - --identifier app.teleark.desktop "$stage/installed.app"
codesign --verify --strict "$stage/installed.app"
"$stage/installed.app/Contents/MacOS/teleark" denied "$stage/qualification.keychain-db"
rm -rf "$stage/installed.app"
mv "$stage/restored.app" "$stage/installed.app"
"$stage/installed.app/Contents/MacOS/teleark" read "$stage/qualification.keychain-db"
printf 'PASS: isolated native Keychain signing continuity and identity rejection.\n'

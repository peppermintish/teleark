#!/bin/bash
set -euo pipefail

version="${1:?version is required}"
label="${2:?artifact label is required}"
architecture=universal
base="teleark-${label}-macos-${architecture}"
app="dist/${base}/TeleArk.app"
binary="dist/${base}.bin"
package="dist/TeleArk-${label}-macos-${architecture}.pkg"

for path in "$binary" "dist/${base}.tar.gz" "$package" dist/SHA256SUMS; do test -s "$path"; done
for required_architecture in arm64 x86_64; do
  lipo -archs "$binary" | tr ' ' '\n' | grep -Fxq "$required_architecture"
done
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" = "$version"
cmp "$binary" "$app/Contents/MacOS/teleark"
expanded="$(mktemp -d -t teleark-expand)"
tar -tzf "dist/${base}.tar.gz" > "$expanded/archive-list"
grep -Fq "${base}/TeleArk.app/Contents/MacOS/teleark" "$expanded/archive-list"
grep -Fq "${base}/THIRD_PARTY_NOTICES.md" "$expanded/archive-list"

# Check both Mach-O slices for non-system dynamic libraries.
bash scripts/verify-macos-libraries.sh "$binary"

pkgutil --expand "$package" "$expanded/product"
grep -Fq "version=\"${version}\"" "$expanded/product/Distribution"
grep -Fq 'checkTeleArkVersion()' "$expanded/product/Distribution"
(cd dist && shasum -a 256 -c SHA256SUMS)

installed_app=/Applications/TeleArk.app
test ! -e "$installed_app"
trap 'sudo rm -rf /Applications/TeleArk.app' EXIT
sudo mkdir -p "$installed_app/Contents"
sudo cp "$app/Contents/Info.plist" "$installed_app/Contents/Info.plist"
sudo /usr/libexec/PlistBuddy -c 'Set :CFBundleShortVersionString 0.0.1' "$installed_app/Contents/Info.plist"
sudo installer -pkg "$package" -target /
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$installed_app/Contents/Info.plist")" = "$version"
cmp "$installed_app/Contents/MacOS/teleark" "$binary"
sudo /usr/libexec/PlistBuddy -c 'Set :CFBundleShortVersionString 99.0.0' "$installed_app/Contents/Info.plist"
before="$(shasum -a 256 "$installed_app/Contents/MacOS/teleark" | cut -d ' ' -f 1)"
if sudo installer -pkg "$package" -target / >"${RUNNER_TEMP:-/tmp}/teleark-downgrade.log" 2>&1; then
  echo 'macOS installer accepted a downgrade.' >&2
  exit 1
fi
grep -Fq 'Downgrading is not permitted' "${RUNNER_TEMP:-/tmp}/teleark-downgrade.log"
test "$(shasum -a 256 "$installed_app/Contents/MacOS/teleark" | cut -d ' ' -f 1)" = "$before"
echo 'Verified universal macOS artifacts, system-only dylibs, in-place upgrade and downgrade refusal.'

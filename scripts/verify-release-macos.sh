#!/bin/bash
set -euo pipefail

version="${1:?version is required}"
label="${2:?artifact label is required}"
architecture=universal
base="teleark-${label}-macos-${architecture}"
app="dist/${base}/TeleArk.app"
binary="dist/${base}.bin"
package="dist/TeleArk-${label}-macos-${architecture}.pkg"
installed_app=/Applications/TeleArk.app
install_started=false
stage="$(mktemp -d -t teleark-release-check)"
cleanup() {
  if "$install_started"; then
    sudo rm -rf "$installed_app"
  fi
  rm -rf "$stage"
}
trap cleanup EXIT

for path in "$binary" "dist/${base}.tar.gz" "$package" dist/SHA256SUMS; do test -s "$path"; done
pwsh -NoProfile -File scripts/check-release-checksums.ps1 -Directory dist \
  "${base}.bin" "${base}.tar.gz" "TeleArk-${label}-macos-${architecture}.pkg"
for required_architecture in arm64 x86_64; do
  lipo -archs "$binary" | tr ' ' '\n' | grep -Fxq "$required_architecture"
done
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")" = "$version"
# App and standalone signatures seal different resources; compare executable code separately.
compare_dir="$stage/code"
mkdir "$compare_dir"
cp "$binary" "$compare_dir/standalone"
cp "$app/Contents/MacOS/teleark" "$compare_dir/bundle"
codesign --remove-signature "$compare_dir/standalone"
codesign --remove-signature "$compare_dir/bundle"
cmp "$compare_dir/standalone" "$compare_dir/bundle"
codesign --verify --strict "$app"
codesign --verify --strict "$binary"
fingerprint="$(openssl x509 -in scripts/macos/release-certificate.pem -noout -fingerprint -sha1 | cut -d= -f2 | tr -d ':')"
requirement="identifier \"app.teleark.desktop\" and certificate leaf = H\"$fingerprint\""
codesign --verify -R "=$requirement" "$app"
codesign --verify -R "=$requirement" "$binary"
expanded="$stage/expanded"
mkdir "$expanded"
tar -tzf "dist/${base}.tar.gz" > "$expanded/archive-list"
grep -Fq "${base}/TeleArk.app/Contents/MacOS/teleark" "$expanded/archive-list"
grep -Fq "${base}/THIRD_PARTY_NOTICES.md" "$expanded/archive-list"

# Check both Mach-O slices for non-system dynamic libraries.
bash scripts/verify-macos-libraries.sh "$binary"

pkgutil --expand "$package" "$expanded/product"
grep -Fq "version=\"${version}\"" "$expanded/product/Distribution"
grep -Fq 'checkTeleArkVersion()' "$expanded/product/Distribution"
if [[ -e "$installed_app" || -L "$installed_app" ]]; then
  echo 'TeleArk is already installed or its application path is occupied on this release runner.' >&2
  exit 1
fi
install_started=true
sudo mkdir -p "$installed_app/Contents"
sudo cp "$app/Contents/Info.plist" "$installed_app/Contents/Info.plist"
sudo /usr/libexec/PlistBuddy -c 'Set :CFBundleShortVersionString 0.0.1' "$installed_app/Contents/Info.plist"
sudo installer -pkg "$package" -target /
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$installed_app/Contents/Info.plist")" = "$version"
cmp "$installed_app/Contents/MacOS/teleark" "$app/Contents/MacOS/teleark"
sudo /usr/libexec/PlistBuddy -c 'Set :CFBundleShortVersionString 99.0.0' "$installed_app/Contents/Info.plist"
before="$(shasum -a 256 "$installed_app/Contents/MacOS/teleark" | cut -d ' ' -f 1)"
# The invoking user owns the temporary log; the installer alone needs elevation.
# shellcheck disable=SC2024
if sudo installer -pkg "$package" -target / >"$stage/downgrade.log" 2>&1; then
  echo 'macOS installer accepted a downgrade.' >&2
  exit 1
fi
grep -Fq 'Downgrading is not permitted' "$stage/downgrade.log"
test "$(shasum -a 256 "$installed_app/Contents/MacOS/teleark" | cut -d ' ' -f 1)" = "$before"
echo 'Verified universal macOS artifacts, system-only dylibs, in-place upgrade and downgrade refusal.'

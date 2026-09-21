#!/bin/bash
# Assemble a portable app and a macOS Installer package.
# Run cargo build before invoking this script.
set -euo pipefail

dry_run=false
paths=()
for argument in "$@"; do
  case "$argument" in
    --dry-run) dry_run=true ;;
    --*) echo "Usage: scripts/package-macos.sh [--dry-run] [binary] [destination_app] [destination_dist] [version] [artifact_label] [architecture]" >&2; exit 2 ;;
    *) paths+=("$argument") ;;
  esac
done

if (( ${#paths[@]} > 6 )); then
  echo "Usage: scripts/package-macos.sh [--dry-run] [binary] [destination_app] [destination_dist] [version] [artifact_label] [architecture]" >&2
  exit 2
fi

repository_root="$(cd "$(dirname "$0")/.." && pwd)"
binary="${paths[0]:-${repository_root}/target/release/teleark}"
destination="${paths[1]:-${repository_root}/dist/TeleArk.app}"
dist_dir="${paths[2]:-${repository_root}/dist}"
if (( ${#paths[@]} >= 4 )); then
  version="${paths[3]}"
else
  version="$(grep -m1 '^version = ' "${repository_root}/Cargo.toml" | cut -d '"' -f 2)"
fi
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Installer version must be three numeric components: $version" >&2; exit 2; }
artifact_label="${paths[4]:-$version}"
[[ "$artifact_label" =~ ^[A-Za-z0-9][A-Za-z0-9.-]*$ ]] || { echo "Invalid artifact label: $artifact_label" >&2; exit 2; }
architecture="${paths[5]:-$(uname -m)}"
if [[ "$architecture" != "arm64" && "$architecture" != "x86_64" && "$architecture" != "universal" ]]; then
  echo "Unsupported macOS architecture: $architecture" >&2
  exit 2
fi

pkg_path="${dist_dir}/TeleArk-${artifact_label}-macos-${architecture}.pkg"
standalone_path="${dist_dir}/teleark-${artifact_label}-macos-${architecture}.bin"
if "$dry_run"; then
  printf 'Would assemble portable macOS app: %s -> %s\n' "$binary" "$destination"
  printf 'Would create standalone executable: %s\n' "$standalone_path"
  printf 'Would create Installer package: %s\n' "$pkg_path"
  exit 0
fi

test -f "$binary"
for tool in pkgbuild productbuild plutil lipo ditto /usr/libexec/PlistBuddy; do
  command -v "$tool" >/dev/null 2>&1 || { echo "Required macOS packaging tool is missing: $tool" >&2; exit 1; }
done
if [[ "$architecture" == "universal" ]]; then
  for required_architecture in arm64 x86_64; do
    if ! lipo -archs "$binary" | tr ' ' '\n' | grep -Fxq "$required_architecture"; then
      echo "The built executable does not contain $required_architecture." >&2
      exit 1
    fi
  done
elif ! lipo -archs "$binary" | tr ' ' '\n' | grep -Fxq "$architecture"; then
  echo "The built executable does not contain the expected $architecture architecture." >&2
  exit 1
fi

mkdir -p "$destination/Contents/MacOS" "$destination/Contents/Resources" "$dist_dir"
cp "$binary" "$destination/Contents/MacOS/teleark"
cp "$binary" "$standalone_path"
chmod +x "$destination/Contents/MacOS/teleark" "$standalone_path"
cp "$repository_root/crates/teleark-gui/assets/macos/Info.plist" "$destination/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$destination/Contents/Info.plist"
/usr/libexec/PlistBuddy -c "Set :CFBundleVersion $version" "$destination/Contents/Info.plist"
plutil -lint "$destination/Contents/Info.plist"

stage="$(mktemp -d -t teleark-pkg)"
trap 'rm -rf "$stage"' EXIT
if [ -f "$repository_root/crates/teleark-gui/assets/macos/TeleArk.icns" ]; then
  cp "$repository_root/crates/teleark-gui/assets/macos/TeleArk.icns" "$destination/Contents/Resources/TeleArk.icns"
else
  mkdir "$stage/TeleArk.iconset"
  for icon_size in 16 32 128 256 512; do
    sips -z "$icon_size" "$icon_size" "$repository_root/crates/teleark-gui/assets/icons/teleark-master.png" --out "$stage/TeleArk.iconset/icon_${icon_size}x${icon_size}.png" >/dev/null
    retina_size=$((icon_size * 2))
    sips -z "$retina_size" "$retina_size" "$repository_root/crates/teleark-gui/assets/icons/teleark-master.png" --out "$stage/TeleArk.iconset/icon_${icon_size}x${icon_size}@2x.png" >/dev/null
  done
  iconutil -c icns "$stage/TeleArk.iconset" -o "$destination/Contents/Resources/TeleArk.icns"
fi
cp "$repository_root/README.md" "$repository_root/LICENSE-MIT" "$repository_root/LICENSE-APACHE" "$repository_root/THIRD_PARTY_NOTICES.md" "$destination/Contents/Resources/"

# Root payloads install at a fixed path and cannot relocate to an older app copy.
mkdir -p "$stage/payload" "$stage/scripts"
ditto "$destination" "$stage/payload/TeleArk.app"
sed "s/APP_VERSION_PLACEHOLDER/${version}/g" "$repository_root/scripts/macos/preinstall" > "$stage/scripts/preinstall"
chmod +x "$stage/scripts/preinstall"
pkgbuild --root "$stage/payload" \
  --install-location /Applications \
  --identifier app.teleark.desktop \
  --version "$version" \
  --scripts "$stage/scripts" \
  "$stage/TeleArkComponent.pkg"

# The distribution check shows the downgrade reason in Installer.app. The
# component preinstall repeats the check for command-line installers.
sed "s/APP_VERSION_PLACEHOLDER/${version}/g" "$repository_root/scripts/macos/Distribution.xml" > "$stage/Distribution.xml"
productbuild --distribution "$stage/Distribution.xml" \
  --package-path "$stage" \
  "$pkg_path"
test -s "$pkg_path"
echo "Created macOS installer package: $pkg_path"

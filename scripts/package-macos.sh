#!/bin/bash
# Assemble an unsigned app bundle and optional .pkg installer.
# Run cargo build before invoking this script.
set -euo pipefail

dry_run=false
paths=()
for argument in "$@"; do
  case "$argument" in
    --dry-run) dry_run=true ;;
    --*) echo "Usage: scripts/package-macos.sh [--dry-run] [binary] [destination_app] [destination_dist] [version]" >&2; exit 2 ;;
    *) paths+=("$argument") ;;
  esac
done

if (( ${#paths[@]} > 4 )); then
  echo "Usage: scripts/package-macos.sh [--dry-run] [binary] [destination_app] [destination_dist] [version]" >&2
  exit 2
fi

repository_root="$(cd "$(dirname "$0")/.." && pwd)"
binary="${paths[0]:-${repository_root}/target/release/teleark}"
destination="${paths[1]:-${repository_root}/dist/TeleArk.app}"
dist_dir="${paths[2]:-${repository_root}/dist}"

if (( ${#paths[@]} >= 4 )); then
  version="${paths[3]}"
else
  version="$(grep -m1 '^version = ' "${repository_root}/Cargo.toml" | cut -d '"' -f 2 || echo 'manual')"
fi
version="${version//\//-}"
architecture="$(uname -m 2>/dev/null || echo 'aarch64')"
pkg_path="${dist_dir}/TeleArk-${version}-macos-${architecture}.pkg"

if "$dry_run"; then
  printf 'Would assemble unsigned macOS app: %s -> %s\n' "$binary" "$destination"
  printf 'Would create installer package: %s\n' "$pkg_path"
  echo 'Would copy the binary, Info.plist, and licenses; generate icons; validate with plutil; and create .pkg with downgrade protection.'
  exit 0
fi

test -f "$binary"
mkdir -p "$destination/Contents/MacOS" "$destination/Contents/Resources"
cp "$binary" "$destination/Contents/MacOS/teleark"
chmod +x "$destination/Contents/MacOS/teleark"
cp "$repository_root/crates/teleark-gui/assets/macos/Info.plist" "$destination/Contents/Info.plist"

if [ -f "$repository_root/crates/teleark-gui/assets/macos/TeleArk.icns" ]; then
  cp "$repository_root/crates/teleark-gui/assets/macos/TeleArk.icns" "$destination/Contents/Resources/TeleArk.icns"
else
  icon_directory="$(mktemp -d -t teleark-icon)"
  trap 'rm -rf "$icon_directory"' EXIT
  mkdir "$icon_directory/TeleArk.iconset"
  for icon_size in 16 32 128 256 512; do
    sips -z "$icon_size" "$icon_size" "$repository_root/crates/teleark-gui/assets/icons/teleark-master.png" --out "$icon_directory/TeleArk.iconset/icon_${icon_size}x${icon_size}.png" >/dev/null
    retina_size=$((icon_size * 2))
    sips -z "$retina_size" "$retina_size" "$repository_root/crates/teleark-gui/assets/icons/teleark-master.png" --out "$icon_directory/TeleArk.iconset/icon_${icon_size}x${icon_size}@2x.png" >/dev/null
  done
  iconutil -c icns "$icon_directory/TeleArk.iconset" -o "$destination/Contents/Resources/TeleArk.icns"
fi

cp "$repository_root/README.md" "$repository_root/LICENSE-MIT" "$repository_root/LICENSE-APACHE" "$repository_root/THIRD_PARTY_NOTICES.md" "$destination/Contents/Resources/"
plutil -lint "$destination/Contents/Info.plist"

# Build .pkg installer with downgrade protection if pkgbuild is available
if command -v pkgbuild >/dev/null 2>&1; then
  echo "Building macOS installer package with downgrade protection..."
  scripts_stage="$(mktemp -d -t teleark-pkg-scripts)"
  trap 'rm -rf "$scripts_stage"' EXIT
  
  sed "s/APP_VERSION_PLACEHOLDER/${version}/g" "$repository_root/scripts/macos/preinstall" > "$scripts_stage/preinstall"
  chmod +x "$scripts_stage/preinstall"
  
  mkdir -p "$dist_dir"
  pkgbuild --component "$destination" \
    --install-location "/Applications" \
    --identifier "app.teleark.desktop" \
    --version "$version" \
    --scripts "$scripts_stage" \
    "$pkg_path"
  echo "Created macOS installer package: $pkg_path"
fi

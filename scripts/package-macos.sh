#!/bin/bash
# Assemble an unsigned app bundle. Run cargo build before invoking this script.
set -euo pipefail
binary="${1:-target/release/teleark}"
destination="${2:-dist/TeleArk.app}"
repository_root="$(cd "$(dirname "$0")/.." && pwd)"
test -f "$binary"
mkdir -p "$destination/Contents/MacOS" "$destination/Contents/Resources"
cp "$binary" "$destination/Contents/MacOS/teleark"
cp "$repository_root/crates/teleark-gui/assets/macos/Info.plist" "$destination/Contents/Info.plist"
icon_directory="$(mktemp -d -t teleark-icon)"
trap 'rm -rf "$icon_directory"' EXIT
mkdir "$icon_directory/TeleArk.iconset"
for icon_size in 16 32 128 256 512; do
  sips -z "$icon_size" "$icon_size" "$repository_root/crates/teleark-gui/assets/icons/teleark-master.png" --out "$icon_directory/TeleArk.iconset/icon_${icon_size}x${icon_size}.png" >/dev/null
  retina_size=$((icon_size * 2))
  sips -z "$retina_size" "$retina_size" "$repository_root/crates/teleark-gui/assets/icons/teleark-master.png" --out "$icon_directory/TeleArk.iconset/icon_${icon_size}x${icon_size}@2x.png" >/dev/null
done
iconutil -c icns "$icon_directory/TeleArk.iconset" -o "$destination/Contents/Resources/TeleArk.icns"
cp "$repository_root/LICENSE-MIT" "$repository_root/LICENSE-APACHE" "$repository_root/THIRD_PARTY_NOTICES.md" "$destination/Contents/Resources/"
plutil -lint "$destination/Contents/Info.plist"

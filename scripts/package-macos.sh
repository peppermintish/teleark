#!/bin/bash
# Assemble an unsigned app bundle. Run cargo build before invoking this script.
set -euo pipefail
dry_run=false
paths=()
for argument in "$@"; do
  case "$argument" in
    --dry-run) dry_run=true ;;
    --*) echo "Usage: scripts/package-macos.sh [--dry-run] [binary] [destination]" >&2; exit 2 ;;
    *) paths+=("$argument") ;;
  esac
done
if (( ${#paths[@]} > 2 )); then
  echo "Usage: scripts/package-macos.sh [--dry-run] [binary] [destination]" >&2
  exit 2
fi
binary="${paths[0]:-target/release/teleark}"
destination="${paths[1]:-dist/TeleArk.app}"
if "$dry_run"; then
  printf 'Would assemble unsigned app: %s -> %s\n' "$binary" "$destination"
  echo 'Would copy the binary, Info.plist and licenses; generate icons with sips/iconutil; validate with plutil.'
  exit 0
fi
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

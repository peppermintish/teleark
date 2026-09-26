#!/bin/bash
# Assemble Linux release packages (AppImage, portable tarball and Debian package).
# Run cargo build before invoking this script.
set -euo pipefail

dry_run=false
paths=()
for argument in "$@"; do
  case "$argument" in
    --dry-run) dry_run=true ;;
    --*) echo "Usage: scripts/package-linux.sh [--dry-run] [binary] [destination_dir] [version] [artifact_label] [target]" >&2; exit 2 ;;
    *) paths+=("$argument") ;;
  esac
done

if (( ${#paths[@]} > 5 )); then
  echo "Usage: scripts/package-linux.sh [--dry-run] [binary] [destination_dir] [version] [artifact_label] [target]" >&2
  exit 2
fi

repository_root="$(cd "$(dirname "$0")/.." && pwd)"
binary="${paths[0]:-${repository_root}/target/release/teleark}"
dist_dir="${paths[1]:-${repository_root}/dist}"

if (( ${#paths[@]} >= 3 )); then
  version="${paths[2]}"
else
  version="$(grep -m1 '^version = ' "${repository_root}/Cargo.toml" | cut -d '"' -f 2)"
fi
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Installer version must be three numeric components: $version" >&2; exit 2; }
artifact_label="${paths[3]:-$version}"
[[ "$artifact_label" =~ ^[A-Za-z0-9][A-Za-z0-9.-]*$ ]] || { echo "Invalid artifact label: $artifact_label" >&2; exit 2; }
target="${paths[4]:-${CARGO_BUILD_TARGET:-x86_64-unknown-linux-gnu}}"
architecture="$(uname -m)"
case "$architecture:$target" in
  x86_64:x86_64-unknown-linux-gnu)
    deb_architecture=amd64 ;;
  *)
    echo "Unsupported Linux runner/target combination: $architecture:$target" >&2
    exit 2 ;;
esac
package_name="teleark-${artifact_label}-linux-${architecture}"
tarball_path="${dist_dir}/${package_name}.tar.gz"
appimage_path="${dist_dir}/${package_name}.AppImage"
deb_path="${dist_dir}/teleark_${artifact_label}_${deb_architecture}.deb"

if "$dry_run"; then
  printf 'Would assemble Linux release packages:\n'
  printf '  Binary: %s\n' "$binary"
  printf '  Standalone AppImage: %s\n' "$appimage_path"
  printf '  Portable tarball: %s\n' "$tarball_path"
  printf '  Debian package: %s\n' "$deb_path"
  echo 'Would bundle runtime libraries, desktop entry, icons and licenses into AppImage and tarball, then build a Debian installer with a downgrade guard.'
  exit 0
fi

test -f "$binary"
command -v cargo-deb >/dev/null 2>&1 || { echo 'cargo-deb is required to build the release installer.' >&2; exit 1; }
linuxdeploy="${LINUXDEPLOY:-linuxdeploy}"
command -v "$linuxdeploy" >/dev/null 2>&1 || { echo 'linuxdeploy is required to bundle the portable Linux release.' >&2; exit 1; }
command -v sha256sum >/dev/null 2>&1 || { echo 'sha256sum is required to verify the Linux release outputs.' >&2; exit 1; }

mkdir -p "$dist_dir"
dist_dir="$(cd "$dist_dir" && pwd)"
appimage_path="${dist_dir}/${package_name}.AppImage"
tarball_path="${dist_dir}/${package_name}.tar.gz"
deb_path="${dist_dir}/teleark_${artifact_label}_${deb_architecture}.deb"
stage_root="$(mktemp -d "${dist_dir}/.teleark-stage.XXXXXX")"
trap 'rm -rf "$stage_root"' EXIT
staging_dir="${stage_root}/${package_name}"
mkdir -p "$staging_dir/usr/share/doc/teleark"
cp "$repository_root/README.md" "$repository_root/LICENSE-MIT" "$repository_root/LICENSE-APACHE" "$repository_root/THIRD_PARTY_NOTICES.md" "$staging_dir/usr/share/doc/teleark/"

# linuxdeploy copies the executable and its shared runtime libraries into AppDir.
# Both portable outputs use that same AppDir, so neither needs build SDK packages.
APPIMAGE_EXTRACT_AND_RUN=1 "$linuxdeploy" \
  --appdir "$staging_dir" \
  --executable "$binary" \
  --desktop-file "$repository_root/crates/teleark-gui/assets/linux/com.teleark.desktop.desktop" \
  --icon-file "$repository_root/crates/teleark-gui/assets/icons/teleark.png"
test -x "$staging_dir/AppRun"
test -x "$staging_dir/usr/bin/teleark"

# Keep the distribution copyright notices for shared libraries resolved on the
# baseline runner alongside the portable payload.
mkdir -p "$staging_dir/usr/share/doc/teleark/system-libraries"
while IFS= read -r library; do
  resolved_library="$(readlink -f "$library")"
  package="$(dpkg-query -S "$library" "$resolved_library" 2>/dev/null | head -n 1 | cut -d: -f1 || true)"
  package="${package%%,*}"
  if [[ -n "$package" && -f "/usr/share/doc/$package/copyright" ]]; then
    cp "/usr/share/doc/$package/copyright" "$staging_dir/usr/share/doc/teleark/system-libraries/${package}.copyright"
  fi
done < <(ldd "$binary" | grep -oE '/[^[:space:]]+' | sort -u)
compgen -G "$staging_dir/usr/share/doc/teleark/system-libraries/*.copyright" >/dev/null || {
  echo 'No Linux runtime-library copyright notices were collected.' >&2
  exit 1
}

LDAI_OUTPUT="$appimage_path" APPIMAGE_EXTRACT_AND_RUN=1 "$linuxdeploy" \
  --appdir "$staging_dir" --output appimage
test -s "$appimage_path"
chmod +x "$appimage_path"
echo "Created standalone AppImage: $appimage_path"

tar -C "$stage_root" -czf "$tarball_path" "$package_name"
echo "Created portable archive: $tarball_path"

# Use the exact input executable for Debian too. Cargo-deb resolves the manifest's
# target/release asset under CARGO_TARGET_DIR; leaving it at the normal build
# directory could silently package a different, stale executable.
deb_target_dir="$stage_root/cargo-target"
mkdir -p "$deb_target_dir/$target/release"
cp "$binary" "$deb_target_dir/$target/release/teleark"
CARGO_TARGET_DIR="$deb_target_dir" cargo deb -p teleark-gui --no-build --locked \
  --manifest-path "$repository_root/crates/teleark-gui/Cargo.toml" \
  --target "$target" --deb-version "$version" --output "$deb_path"
test -s "$deb_path"
echo "Created Debian package with downgrade protection: $deb_path"

(
  cd "$dist_dir"
  sha256sum "${package_name}.AppImage" "${package_name}.tar.gz" \
    "teleark_${artifact_label}_${deb_architecture}.deb" > "$stage_root/SHA256SUMS"
  mv "$stage_root/SHA256SUMS" SHA256SUMS
)
echo "Generated checksums in ${dist_dir}/SHA256SUMS"

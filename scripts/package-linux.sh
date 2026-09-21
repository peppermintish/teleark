#!/bin/bash
# Assemble Linux release packages (standalone executable, portable tarball and Debian package).
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

# Determine version
if (( ${#paths[@]} >= 3 )); then
  version="${paths[2]}"
else
  version="$(grep -m1 '^version = ' "${repository_root}/Cargo.toml" | cut -d '"' -f 2 || echo 'manual')"
fi
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "Installer version must be three numeric components: $version" >&2; exit 2; }
artifact_label="${paths[3]:-$version}"
[[ "$artifact_label" =~ ^[A-Za-z0-9][A-Za-z0-9.-]*$ ]] || { echo "Invalid artifact label: $artifact_label" >&2; exit 2; }
target="${paths[4]:-}"
if [[ -n "$target" && "$target" != "x86_64-unknown-linux-gnu" ]]; then
  echo "Unsupported Linux release target: $target" >&2
  exit 2
fi
architecture="$(uname -m)"
if [[ "$architecture" != "x86_64" ]]; then
  echo "The Linux release package requires an x86_64 runner." >&2
  exit 2
fi
package_name="teleark-${artifact_label}-linux-${architecture}"
tarball_path="${dist_dir}/${package_name}.tar.gz"
standalone_path="${dist_dir}/${package_name}"
deb_path="${dist_dir}/teleark_${artifact_label}_amd64.deb"

if "$dry_run"; then
  printf 'Would assemble Linux release packages:\n'
  printf '  Binary: %s\n' "$binary"
  printf '  Standalone executable: %s\n' "$standalone_path"
  printf '  Portable tarball: %s\n' "$tarball_path"
  printf '  Debian package: %s\n' "$deb_path"
  echo 'Would copy binary, desktop entry, icons, licenses, compress tarball, and build a Debian installer with a downgrade guard.'
  exit 0
fi

test -f "$binary"
command -v cargo-deb >/dev/null 2>&1 || { echo 'cargo-deb is required to build the release installer.' >&2; exit 1; }

mkdir -p "$dist_dir"
stage_root="$(mktemp -d "${dist_dir}/.teleark-stage.XXXXXX")"
trap 'rm -rf "$stage_root"' EXIT
staging_dir="${stage_root}/${package_name}"
mkdir "$staging_dir"
cp "$binary" "$staging_dir/teleark"
cp "$binary" "$standalone_path"
chmod +x "$staging_dir/teleark" "$standalone_path"

# Copy desktop entry and main icon
cp "$repository_root/crates/teleark-gui/assets/linux/com.teleark.desktop.desktop" "$staging_dir/"
cp "$repository_root/crates/teleark-gui/assets/icons/teleark.png" "$staging_dir/"
cp "$repository_root/README.md" "$repository_root/LICENSE-MIT" "$repository_root/LICENSE-APACHE" "$repository_root/THIRD_PARTY_NOTICES.md" "$staging_dir/"

# Create portable tarball
tar -C "$stage_root" -czf "$tarball_path" "$package_name"
echo "Created portable archive: $tarball_path"

# The preinst receives the package's real control version from dpkg. Cargo-deb
# resolves target/release assets to the selected target directory itself.
if [[ -n "$target" ]]; then
  cargo deb -p teleark-gui --no-build --target "$target" --output "$deb_path"
else
  cargo deb -p teleark-gui --no-build --output "$deb_path"
fi
test -s "$deb_path"
echo "Created Debian package with downgrade protection: $deb_path"

# Generate SHA256 checksums
(
  cd "$dist_dir"
  rm -f SHA256SUMS
  for file in "${package_name}" "${package_name}.tar.gz" "teleark_${artifact_label}_amd64.deb"; do
    if [ -f "$file" ]; then
      if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" >> SHA256SUMS
      elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$file" >> SHA256SUMS
      fi
    fi
  done
)
echo "Generated checksums in ${dist_dir}/SHA256SUMS"

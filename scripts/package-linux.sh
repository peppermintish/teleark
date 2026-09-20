#!/bin/bash
# Assemble Linux release packages (portable tarball and Debian package).
# Run cargo build before invoking this script.
set -euo pipefail

dry_run=false
paths=()
for argument in "$@"; do
  case "$argument" in
    --dry-run) dry_run=true ;;
    --*) echo "Usage: scripts/package-linux.sh [--dry-run] [binary] [destination_dir] [version]" >&2; exit 2 ;;
    *) paths+=("$argument") ;;
  esac
done

if (( ${#paths[@]} > 3 )); then
  echo "Usage: scripts/package-linux.sh [--dry-run] [binary] [destination_dir] [version]" >&2
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
version="${version//\//-}"
architecture="$(uname -m 2>/dev/null || echo 'x86_64')"
package_name="teleark-${version}-linux-${architecture}"
staging_dir="${dist_dir}/${package_name}"
tarball_path="${dist_dir}/${package_name}.tar.gz"
deb_path="${dist_dir}/teleark_${version}_amd64.deb"

if "$dry_run"; then
  printf 'Would assemble Linux release packages:\n'
  printf '  Binary: %s\n' "$binary"
  printf '  Staging directory: %s\n' "$staging_dir"
  printf '  Portable tarball: %s\n' "$tarball_path"
  printf '  Debian package: %s\n' "$deb_path"
  echo 'Would copy binary, desktop entry, icons, licenses, compress tarball, and invoke cargo-deb if available.'
  exit 0
fi

test -f "$binary"

mkdir -p "$staging_dir"
cp "$binary" "$staging_dir/teleark"
chmod +x "$staging_dir/teleark"

# Copy desktop entry and main icon
cp "$repository_root/crates/teleark-gui/assets/linux/com.teleark.desktop.desktop" "$staging_dir/"
cp "$repository_root/crates/teleark-gui/assets/icons/teleark.png" "$staging_dir/"
cp "$repository_root/README.md" "$repository_root/LICENSE-MIT" "$repository_root/LICENSE-APACHE" "$repository_root/THIRD_PARTY_NOTICES.md" "$staging_dir/"

# Create portable tarball
tar -C "$dist_dir" -czf "$tarball_path" "$package_name"
echo "Created portable archive: $tarball_path"

# Build Debian package if cargo-deb is installed
if command -v cargo-deb >/dev/null 2>&1; then
  echo "Building Debian package with cargo-deb..."
  cargo deb -p teleark-gui --no-build --output "$deb_path"
  echo "Created Debian package: $deb_path"
else
  echo "cargo-deb not found on PATH. Skipping .deb generation." >&2
fi

# Generate SHA256 checksums
(
  cd "$dist_dir"
  rm -f SHA256SUMS
  for file in "${package_name}.tar.gz" "teleark_${version}_amd64.deb"; do
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

#!/bin/bash
set -euo pipefail

version="${1:?version is required}"
label="${2:?artifact label is required}"
architecture="$(uname -m)"
case "$architecture" in
  x86_64) deb_architecture=amd64 ;;
  *) echo "Unsupported Linux architecture: $architecture" >&2; exit 2 ;;
esac
base="teleark-${label}-linux-${architecture}"
appimage="$(pwd)/dist/${base}.AppImage"
tarball="$(pwd)/dist/${base}.tar.gz"
deb="$(pwd)/dist/teleark_${label}_${deb_architecture}.deb"
for path in "$appimage" "$tarball" "$deb" dist/SHA256SUMS; do test -s "$path"; done
file "$appimage" | grep -F 'ELF 64-bit' >/dev/null
test "$(dpkg-deb -f "$deb" Package)" = teleark
test "$(dpkg-deb -f "$deb" Architecture)" = "$deb_architecture"
package_version="$(dpkg-deb -f "$deb" Version)"
case "$package_version" in "$version"|"$version"-*) ;; *) echo "Unexpected Debian version: $package_version" >&2; exit 1;; esac
dependencies="$(dpkg-deb -f "$deb" Depends)"
if grep -Eiq '(^|[, ])[^, ]*(-dev|pkg-config|cmake|clang)([, (]|$)' <<< "$dependencies"; then
  echo "Debian installer requires build-time SDK packages: $dependencies" >&2
  exit 1
fi

stage="$(mktemp -d)"
if dpkg-query -W -f='${Status}' teleark 2>/dev/null | grep -Fxq 'install ok installed'; then
  echo 'TeleArk is already installed on this release runner.' >&2
  exit 1
fi
trap 'sudo dpkg -r teleark >/dev/null 2>&1 || true; rm -rf "$stage"' EXIT
dpkg-deb -e "$deb" "$stage/control"
dpkg-deb -x "$deb" "$stage/deb-payload"
test -x "$stage/control/preinst"
if "$stage/control/preinst" upgrade "${package_version}+newer" "$package_version" >"$stage/downgrade.log" 2>&1; then
  echo 'Debian preinst accepted a downgrade.' >&2
  exit 1
fi
grep -Fq 'Downgrading is not permitted' "$stage/downgrade.log"
"$stage/control/preinst" upgrade 0.0.1 "$package_version"

tar -C "$stage" -xzf "$tarball"
(cd "$stage" && "$appimage" --appimage-extract >/dev/null)
bash scripts/verify-linux-payloads.sh "$stage/$base" "$stage/squashfs-root" "$stage/deb-payload"
for payload in "$stage/$base" "$stage/squashfs-root" "$stage/deb-payload"; do
  if ! file "$payload/usr/bin/teleark" | grep -Eq 'ELF 64-bit LSB .*x86-64'; then
    echo "Linux package has an invalid x64 ELF executable: $payload/usr/bin/teleark" >&2
    exit 1
  fi
done
linked_libraries="$(ldd "$stage/$base/usr/bin/teleark")"
if grep -Fq 'not found' <<< "$linked_libraries"; then
  echo 'Portable Linux binary has an unresolved runtime library on the baseline runner.' >&2
  exit 1
fi

(cd dist && sha256sum -c SHA256SUMS)

sudo apt-get install -y "$deb"
test -x /usr/bin/teleark
cmp /usr/bin/teleark "$stage/deb-payload/usr/bin/teleark"
echo "Verified Linux $architecture AppImage, portable archive, Debian install and downgrade refusal."

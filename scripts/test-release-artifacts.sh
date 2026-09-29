#!/usr/bin/env bash
set -euo pipefail

repository_root="$(cd "$(dirname "$0")/.." && pwd)"
assembler="$repository_root/scripts/assemble-release-assets.sh"
temp_root="${TMPDIR:-/tmp}"
test_directory="$(mktemp -d "$temp_root/teleark-release-artifacts.XXXXXX")"

cleanup() {
  case "$test_directory" in
    "$temp_root"/teleark-release-artifacts.*) rm -rf -- "$test_directory" ;;
    *) echo 'Refusing to remove test data outside the designated temporary directory.' >&2; exit 1 ;;
  esac
}
trap cleanup EXIT

label='1.2.3-manual-abc1234'
artifact_root="$test_directory/artifacts"
destination="$test_directory/dist"
mkdir -p "$artifact_root/release-windows-x86_64" "$artifact_root/release-macos-universal" "$artifact_root/release-linux-x86_64"

windows=("TeleArk-${label}-windows-x86_64.msix")
macos=("teleark-${label}-macos-universal.bin" "TeleArk-${label}-macos-universal.pkg" "teleark-${label}-macos-universal.tar.gz")
linux=("teleark-${label}-linux-x86_64.AppImage" "teleark-${label}-linux-x86_64.tar.gz" "teleark_${label}_amd64.deb")

write_group() {
  local group="$1"
  shift
  local directory="$artifact_root/$group"
  : > "$directory/SHA256SUMS"
  for filename in "$@"; do
    printf 'synthetic package contents for %s\n' "$filename" > "$directory/$filename"
    (cd "$directory" && sha256sum "$filename" >> SHA256SUMS)
  done
}

write_group release-windows-x86_64 "${windows[@]}"
write_group release-macos-universal "${macos[@]}"
write_group release-linux-x86_64 "${linux[@]}"

# Model the CRLF output from [IO.File]::WriteAllLines on a Windows runner.
windows_manifest="$artifact_root/release-windows-x86_64/SHA256SUMS"
windows_manifest_line="$(cat "$windows_manifest")"
printf '%s\r\n' "$windows_manifest_line" > "$windows_manifest"
if ! grep -q $'\r' "$windows_manifest"; then
  echo 'Synthetic Windows checksum manifest must contain CRLF.' >&2
  exit 1
fi

if ! "$assembler" "$artifact_root" "$destination" "$label" >/dev/null; then
  echo 'Valid platform artifacts should assemble.' >&2
  exit 1
fi
actual_release_files="$(find "$destination" -mindepth 1 -maxdepth 1 -type f -printf '%f\n' | LC_ALL=C sort)"
expected_release_files="$(printf '%s\n' SHA256SUMS "${windows[@]}" "${macos[@]}" "${linux[@]}" LICENSE-MIT LICENSE-APACHE THIRD_PARTY_NOTICES.md | LC_ALL=C sort)"
if [[ "$actual_release_files" != "$expected_release_files" ]]; then
  echo 'Assembled release did not contain seven package files and the four required accompanying assets.' >&2
  exit 1
fi
(cd "$destination" && sha256sum --quiet -c SHA256SUMS)

case "$destination" in
  "$test_directory"/*) rm -rf -- "$destination" ;;
  *) echo 'Refusing to remove a destination outside the designated test directory.' >&2; exit 1 ;;
esac
printf 'unexpected file\n' > "$artifact_root/release-linux-x86_64/unexpected.txt"
if "$assembler" "$artifact_root" "$destination" "$label" >/dev/null 2>&1; then
  echo 'Unexpected files in downloaded artifacts must be rejected.' >&2
  exit 1
fi
if [[ -e "$destination" ]]; then
  echo 'A rejected artifact set must not create a release destination.' >&2
  exit 1
fi
rm -f -- "$artifact_root/release-linux-x86_64/unexpected.txt"

printf 'tampered package\n' >> "$artifact_root/release-windows-x86_64/${windows[0]}"
if "$assembler" "$artifact_root" "$destination" "$label" >/dev/null 2>&1; then
  echo 'Packages with an invalid downloaded checksum must be rejected.' >&2
  exit 1
fi
if [[ -e "$destination" ]]; then
  echo 'A checksum failure must not create a release destination.' >&2
  exit 1
fi

printf 'Release artifact verification passed: seven packages, CRLF manifest, three legal notices, unified checksums, and rejection cases.\n'

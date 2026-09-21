#!/bin/bash
set -euo pipefail

repository_root="$(cd "$(dirname "$0")/.." && pwd)"
temporary_root="$(cd "${TMPDIR:-/tmp}" && pwd -P)"
stage="$(mktemp -d "$temporary_root/teleark-linux-payloads.XXXXXX")"
stage="$(cd "$stage" && pwd -P)"
case "$stage" in
  "$temporary_root"/teleark-linux-payloads.*) ;;
  *) echo "Unsafe test directory: $stage" >&2; exit 1 ;;
esac
trap 'rm -rf "$stage"' EXIT

# Byte fixtures isolate the relationship between package formats. Native ELF,
# library resolution and apt installation remain in verify-release-linux.sh.
for format in portable appimage debian; do
  payload="$stage/$format"
  mkdir -p "$payload/usr/bin" "$payload/usr/share/doc/teleark/system-libraries"
  printf '#!/bin/sh\nexit 0\n' > "$payload/usr/bin/teleark"
  chmod +x "$payload/usr/bin/teleark"
  for name in LICENSE-MIT LICENSE-APACHE THIRD_PARTY_NOTICES.md; do
    printf 'Fixture notice\n' > "$payload/usr/share/doc/teleark/$name"
  done
done
for format in portable appimage; do
  payload="$stage/$format"
  printf '# portable RPATH and strip transformation\n' >> "$payload/usr/bin/teleark"
  cp "$payload/usr/bin/teleark" "$payload/AppRun"
  chmod +x "$payload/AppRun"
  printf 'Runtime notice\n' > "$payload/usr/share/doc/teleark/system-libraries/runtime.copyright"
done

verify() {
  bash "$repository_root/scripts/verify-linux-payloads.sh" "$stage/portable" "$stage/appimage" "$stage/debian"
}
expect_failure() {
  local expected="$1"
  if verify > "$stage/failure.log" 2>&1; then
    echo "Linux payload check unexpectedly passed: $expected" >&2
    exit 1
  fi
  grep -Fq "$expected" "$stage/failure.log"
}

# This is the v0.4.8 failure: portable bytes differ legitimately from Debian.
if cmp -s "$stage/portable/usr/bin/teleark" "$stage/debian/usr/bin/teleark"; then
  echo 'The regression fixture does not reproduce the package transformation.' >&2
  exit 1
fi
verify

printf 'corruption\n' >> "$stage/appimage/usr/bin/teleark"
expect_failure 'AppImage and portable archive contain different TeleArk executables.'
cp "$stage/portable/usr/bin/teleark" "$stage/appimage/usr/bin/teleark"

: > "$stage/debian/usr/bin/teleark"
expect_failure 'Missing or empty Linux package file:'
printf '#!/bin/sh\nexit 0\n' > "$stage/debian/usr/bin/teleark"

mv "$stage/portable/usr/share/doc/teleark/system-libraries/runtime.copyright" "$stage/runtime.notice"
expect_failure 'Missing runtime-library copyright notices:'
mv "$stage/runtime.notice" "$stage/portable/usr/share/doc/teleark/system-libraries/runtime.copyright"
verify
echo 'Linux payload checks passed: transformed Debian bytes accepted; portable corruption and missing files rejected.'

#!/bin/bash
# Verify extracted payloads before the native ELF, dependency and install checks.
set -euo pipefail

portable="${1:?portable payload directory is required}"
appimage="${2:?AppImage payload directory is required}"
debian="${3:?Debian payload directory is required}"

for payload in "$portable" "$appimage" "$debian"; do
  for file in usr/bin/teleark usr/share/doc/teleark/LICENSE-MIT usr/share/doc/teleark/LICENSE-APACHE usr/share/doc/teleark/THIRD_PARTY_NOTICES.md; do
    if [[ ! -s "$payload/$file" ]]; then
      echo "Missing or empty Linux package file: $payload/$file" >&2
      exit 1
    fi
  done
  test -x "$payload/usr/bin/teleark"
done

for payload in "$portable" "$appimage"; do
  test -x "$payload/AppRun"
  compgen -G "$payload/usr/share/doc/teleark/system-libraries/*.copyright" >/dev/null || {
    echo "Missing runtime-library copyright notices: $payload" >&2
    exit 1
  }
done

# linuxdeploy strips the portable ELF and rewrites its RPATH. cargo-deb packages
# and strips the original ELF independently, so equality with Debian is invalid.
# AppImage and tarball come from the same final AppDir and must match each other.
if ! cmp -s "$portable/usr/bin/teleark" "$appimage/usr/bin/teleark"; then
  echo 'AppImage and portable archive contain different TeleArk executables.' >&2
  exit 1
fi

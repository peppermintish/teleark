#!/bin/bash
# Create once, keep the private directory outside the checkout, reuse on every release.
set +x
set -euo pipefail
umask 077
identity_dir="${1:?Pass a private directory outside the repository}"
repository_root="$(cd "$(dirname "$0")/../.." && pwd)"
mkdir -p "$identity_dir"
chmod 700 "$identity_dir"
identity_dir="$(cd "$identity_dir" && pwd)"
case "$identity_dir/" in "$repository_root/"*) echo 'Signing secrets must be outside the checkout.' >&2; exit 1 ;; esac
for file in private-key.pem certificate.pem identity.p12; do
  test ! -e "$identity_dir/$file" || { echo 'An identity already exists; refusing to replace it.' >&2; exit 1; }
done
openssl req -x509 -newkey rsa:3072 -sha256 -days 7300 -noenc \
  -subj '/CN=TeleArk Release Signing/O=TeleArk' \
  -addext 'basicConstraints=critical,CA:TRUE' \
  -addext 'keyUsage=critical,digitalSignature,keyCertSign' \
  -addext 'extendedKeyUsage=codeSigning' \
  -keyout "$identity_dir/private-key.pem" -out "$identity_dir/certificate.pem" 2>/dev/null
# macOS security import accepts the interoperable legacy PKCS#12 envelope.
# Its empty passphrase is deliberate: the filesystem and CI secret store protect it.
openssl pkcs12 -export -legacy -macalg sha1 -inkey "$identity_dir/private-key.pem" \
  -in "$identity_dir/certificate.pem" -out "$identity_dir/identity.p12" -passout pass:
chmod 600 "$identity_dir"/*
printf 'Created a private reusable signing identity in %s\n' "$identity_dir"
printf 'Only certificate.pem is public. Keep private-key.pem and identity.p12 private.\n'

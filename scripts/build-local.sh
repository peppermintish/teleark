#!/bin/bash
# Build a local release using only the trusted, private repository configuration.
set +x
set -euo pipefail
repository_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repository_root"
if [[ ! -f .env.local ]]; then
  echo "Missing .env.local; copy .env.example and replace its sample values locally." >&2
  exit 1
fi
# Do not let inherited values fill gaps in the local configuration.
unset TELEARK_DISTRIBUTION_TELEGRAM_API_ID TELEARK_DISTRIBUTION_TELEGRAM_API_HASH
set -a
. ./.env.local
set +a
api_id="${TELEARK_DISTRIBUTION_TELEGRAM_API_ID:-}"
api_hash="${TELEARK_DISTRIBUTION_TELEGRAM_API_HASH:-}"
if [[ ! "$api_id" =~ ^[1-9][0-9]{0,9}$ ]] || (( api_id > 2147483647 )) ||
   [[ ! "$api_hash" =~ ^[[:xdigit:]]{32}$ ]]; then
  echo ".env.local must define a valid Telegram API ID and Hash; values were not logged." >&2
  exit 1
fi
if [[ "$api_id" == 17349 ]]; then
  echo "Replace the public TEST ONLY sample in .env.local with your own application credentials." >&2
  exit 1
fi
echo "Building release with .env.local application identifiers."
cargo build --release -p teleark-gui --bin teleark --locked

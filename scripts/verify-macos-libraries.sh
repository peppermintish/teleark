#!/bin/bash
set -euo pipefail

binary="${1:?binary is required}"
for architecture in arm64 x86_64; do
  # Inspect one slice at a time: universal otool output repeats the binary
  # heading for each architecture, and headings are not dependencies.
  linked_libraries="$(otool -arch "$architecture" -L "$binary")"
  dependencies="$(printf '%s\n' "$linked_libraries" | awk '/^[[:space:]]+[^[:space:]]/ {print $1}')"
  if [[ -z "$dependencies" ]]; then
    echo "Could not inspect macOS library dependencies for $architecture." >&2
    exit 1
  fi
  while IFS= read -r dependency; do
    case "$dependency" in
      /usr/lib/*|/System/Library/*) ;;
      *) echo "Unbundled macOS library for $architecture: $dependency" >&2; exit 1 ;;
    esac
  done <<< "$dependencies"
done

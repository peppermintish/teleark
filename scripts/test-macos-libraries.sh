#!/bin/bash
set -euo pipefail

stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
cat > "$stage/otool" <<'EOF'
#!/bin/bash
[[ "$1" == -arch && "$3" == -L ]] || exit 2
architecture="$2"
binary="$4"
printf '%s (architecture %s):\n' "$binary" "$architecture"
case "${TEST_LIBRARY_CASE:-system}:$architecture" in
  missing:x86_64) exit 0 ;;
  external:x86_64) printf '\t@rpath/libExternal.dylib (compatibility version 1.0.0)\n' ;;
  *) printf '\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)\n' ;;
esac
EOF
chmod +x "$stage/otool"
export PATH="$stage:$PATH"

bash scripts/verify-macos-libraries.sh dist/teleark-universal.bin

if TEST_LIBRARY_CASE=external bash scripts/verify-macos-libraries.sh dist/teleark-universal.bin >"$stage/external.log" 2>&1; then
  echo 'The macOS library check accepted a non-system dependency.' >&2
  exit 1
fi
grep -Fq 'Unbundled macOS library for x86_64: @rpath/libExternal.dylib' "$stage/external.log"

if TEST_LIBRARY_CASE=missing bash scripts/verify-macos-libraries.sh dist/teleark-universal.bin >"$stage/missing.log" 2>&1; then
  echo 'The macOS library check accepted an uninspected architecture.' >&2
  exit 1
fi
grep -Fq 'Could not inspect macOS library dependencies for x86_64.' "$stage/missing.log"
echo 'macOS universal library checks passed.'

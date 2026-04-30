#!/usr/bin/env bash
# build.sh — Build the hello_world example and convert to DOL
#
# Usage:
#   ./build.sh                 # debug build
#   ./build.sh --release       # optimised build
#   ./build.sh --release --run # build + launch in Dolphin

set -euo pipefail

PROFILE="debug"
DOLPHIN=0

for arg in "$@"; do
  case "$arg" in
    --release) PROFILE="release" ;;
    --run)     DOLPHIN=1 ;;
  esac
done

TARGET="targets/powerpc-gekko-eabi.json"
PROFILE_FLAG=""
[ "$PROFILE" = "release" ] && PROFILE_FLAG="--release"

echo "==> Building hello_world ($PROFILE)..."
cargo +nightly build \
  -Z build-std=core,compiler_builtins \
  -Z build-std-features=compiler-builtins-mem \
  --target "$TARGET" \
  -p hello_world \
  $PROFILE_FLAG

ELF="target/powerpc-gekko-eabi/$PROFILE/hello_world"
DOL="hello_world.dol"

echo "==> Converting ELF → DOL..."
cargo run -p elf2dol -- "$ELF" "$DOL"

echo "==> Output: $DOL"

if [ "$DOLPHIN" = "1" ]; then
  echo "==> Launching Dolphin..."
  dolphin-emu -e "$DOL" &
fi

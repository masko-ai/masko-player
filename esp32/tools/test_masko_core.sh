#!/usr/bin/env bash
#
# Host tests for masko_core. Uses the system compiler, so this runs in seconds
# without the ESP-IDF toolchain or hardware attached.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(dirname "$SCRIPT_DIR")"
PACK="${1:-}"
[ -n "$PACK" ] || { echo "usage: $0 <pack.mkpk>"; exit 2; }

BIN="$(mktemp -d)/test_masko_pack"

cc -std=c11 -Wall -Wextra -Werror \
   -I "$ROOT/components/masko_core/include" \
   -DPACK_PATH="\"$PACK\"" \
   "$ROOT/test/test_masko_pack.c" \
   "$ROOT/components/masko_core/masko_pack.c" \
   -o "$BIN"

"$BIN"
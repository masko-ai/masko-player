#!/usr/bin/env bash
#
# Every host test in the SDK, in one command. No hardware, no ESP-IDF toolchain.
#
#   tools/test_all.sh [pack.mkpk]
#
# The pack tests need a real .mkpk to read; build one with tools/masko_pack.py if
# you do not have one. The graph tests build against the cJSON source inside
# ESP-IDF, so point IDF_PATH at your checkout if it is not in the usual place.
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(dirname "$SCRIPT_DIR")"
PACK="${1:-}"

# A pack to test against: an explicit argument, else any built example asset.
if [ -z "$PACK" ]; then
    PACK="$(ls -1 "$ROOT"/examples/*/assets/*.mkpk 2>/dev/null | head -1 || true)"
fi

rc=0

echo "###############################################################"
echo "# masko_core -- pack reader"
echo "###############################################################"
if [ -n "$PACK" ]; then
    "$SCRIPT_DIR/test_masko_core.sh" "$PACK" || rc=1
else
    echo "  (skipped -- no pack given and no examples/*/assets/*.mkpk found)"
    echo "   build one: tools/masko_pack.py build clip.mov -o /tmp/loop.mkpk"
fi
echo

echo "###############################################################"
echo "# masko_graph -- canvas state machine and JSON loader"
echo "###############################################################"
"$SCRIPT_DIR/test_masko_graph.sh" || rc=1
echo

if [ "$rc" -eq 0 ]; then
    echo "ALL HOST TESTS PASSED"
else
    echo "SOME HOST TESTS FAILED"
fi
exit "$rc"

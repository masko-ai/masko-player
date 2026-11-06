#!/usr/bin/env bash
#
# Write a .mkpk into the 'masko' data partition over USB/serial.
# The partition is mmap'd at runtime, so the pack is not compiled into the app.
#
set -euo pipefail

PACK="${1:?usage: $0 <pack.mkpk> [port]}"
PORT="${2:-/dev/cu.usbmodem101}"

if [ ! -f "$PACK" ]; then
  echo "no such pack: $PACK" >&2
  exit 1
fi

. "${IDF_PATH:-$HOME/esp/esp-idf}/export.sh" >/dev/null 2>&1

SIZE=$(wc -c < "$PACK" | tr -d ' ')
echo "flashing $PACK ($SIZE bytes) to 'masko' on $PORT"

python "$IDF_PATH/components/partition_table/parttool.py" \
  --port "$PORT" write_partition --partition-name masko --input "$PACK"

echo "done"
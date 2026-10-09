#!/bin/sh
# Fetch LeRobot's X-series control table (`src/lerobot/motors/dynamixel/tables.py`, Apache-2.0, which cites ROBOTIS's e-manual) at the commit pinned below, and check it against the SHA-256 pinned below:
# `sh sim/fetch-vendor.sh <directory>`. It is read by `layout_check.py` as a second statement of the register map, never imported by the driver and never committed. `LEROBOT_COMMIT=HEAD` (the nightly's second run)
# takes the file at the tip of the maker's default branch instead, in which case the hash is not checked and the run says so.
set -eu

DIR=$1
COMMIT=${LEROBOT_COMMIT:-a855570097e6233b23dd42341d02c64f3ccc9267}
SHA256=683b400f6199415df32beae1146a662015b6d4ad90bc6af3bdf8ddc8222324b9

mkdir -p "$DIR"
if [ "$COMMIT" = HEAD ]; then
  COMMIT=main
  echo "== LeRobot's table at the tip of main; it is not checked against a pin"
  PINNED=0
else
  PINNED=1
fi
curl -sSLf -o "$DIR/lerobot_tables.py" "https://raw.githubusercontent.com/huggingface/lerobot/$COMMIT/src/lerobot/motors/dynamixel/tables.py"
if [ "$PINNED" = 1 ]; then
  echo "$SHA256  $DIR/lerobot_tables.py" | sha256sum -c -
fi
echo "== $DIR/lerobot_tables.py"

#!/bin/sh
# KUKA's RSI simulator stopped under the bridge and KUKA's own plugin (DRY-COVERAGE-PLAN.md item 12.C.4; `frozen.py` is the driver's half), run inside `kuka/Dockerfile`'s image by
# `make frozen-rsi-docker`.
#
# The simulator and the plugin share one container here (RSI is a UDP exchange on the container's loopback, as in `run-rsi.sh`), so Docker's freezer, which stops a whole container, would
# stop the driver too. The simulator's own processes are stopped with SIGSTOP instead: the same thing as far as the plugin can tell (the controller's cycle telegrams stop and nothing
# is sent back), and undone by SIGCONT. The two talk through files, as the other runners' two halves do.
#
#   sh kuka/run-frozen.sh <vendor-prefix>
set -eu

PREFIX=$1
OUT=${TMPDIR:-/tmp}/skynet-rsi-frozen
HAND=${TMPDIR:-/tmp}/skynet-rsi-frozen.d

set +u
. /opt/ros/jazzy/setup.sh
. "$PREFIX/setup.sh"
set -u

stop_loop() {
  kill -TERM "$LOOP" 2>/dev/null || true
  wait "$LOOP" 2>/dev/null || true
  pkill -CONT -f 'kuka_rsi_simulator/rsi_simulator' 2>/dev/null || true
  pkill -TERM -f 'kuka_rsi_simulator/rsi_simulator' 2>/dev/null || true
  trap - EXIT INT TERM
}

rm -rf "$HAND" "$OUT.log"
mkdir -p "$HAND"
(
  while :; do
    ros2 run kuka_rsi_simulator rsi_simulator >> "$OUT.log" 2>&1 || true
    echo "== the simulator exited" >> "$OUT.log"
    sleep 0.2
  done
) &
LOOP=$!
trap 'stop_loop' EXIT INT TERM
waited=0
while ! grep -q "Successfully created socket" "$OUT.log" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 150 ]; then
    echo "::error::KUKA's RSI simulator never started. It said:"; cat "$OUT.log"; exit 1
  fi
  sleep 0.1
done

make --no-print-directory frozen-rsi VENDOR_PREFIX="$PREFIX" FROZEN_DIR="$HAND" &
DRIVER=$!

waiting_for() {   # 0 when the file came, 1 when the driver ended without it
  while [ ! -e "$HAND/$1" ]; do
    kill -0 "$DRIVER" 2>/dev/null || return 1
    sleep 0.1
  done
}

if waiting_for ready; then
  pkill -STOP -f 'kuka_rsi_simulator/rsi_simulator' && echo "== the RSI simulator stopped $(date +%T)"
  touch "$HAND/frozen"
  if waiting_for measured; then
    pkill -CONT -f 'kuka_rsi_simulator/rsi_simulator' && echo "== the RSI simulator let go $(date +%T)"
    touch "$HAND/thawed"
  fi
fi
STATUS=0
wait "$DRIVER" || STATUS=$?
echo "== what the controller said"
grep -a -c "Socket timed out" "$OUT.log" | sed 's/^/   telegrams with no reply inside the cycle: /' || true
grep -a "Timeout count of 100 exceeded" "$OUT.log" | head -1 || true
exit "$STATUS"

#!/bin/sh
# The MAVLink row against ArduPilot's own firmware (DRY-COVERAGE-PLAN.md Phase 11, item 11.1), run inside
# `sim/Dockerfile`'s image by `make check-sim-docker`:
#
#   1. `layout_check.py` -- this driver's MAVLink against the reference implementation's;
#   2. one vehicle of each family the firmware builds -- a multirotor, a rover, a fixed wing, a rover built
#      as a boat, and a submarine -- each a separate process of the vehicle's own flight code, on its own
#      TCP ports (SERIAL0 for the driver, SERIAL1 for the reference implementation, five ports apart);
#   3. `live.py` for each: the driver against the firmware, in real time, with the reference reading the
#      same vehicle as a second opinion;
#   4. the sim half of SCTS through the real ABI against the multirotor, the rover, the fixed wing, the boat and the submarine.
#
# The container has NET_ADMIN so that `live.py` can put delay, jitter and loss on its loopback (`tc netem`) and
# watch the driver on a link that is not one; without the capability that block says it did not run.
#
# A script rather than a recipe for the reason every run-*.sh in drivers/ gives: it runs suites against
# processes and reads what they did. `sh sim/run-sim.sh <skynet-binary>`, never `./`.
set -eu

SKYNET=$1
HERE=$(cd "$(dirname "$0")" && pwd)
MAVLINK=$(dirname "$HERE")
REPO=$(cd "$MAVLINK/../.." && pwd)
PY=/opt/venv/bin/python
export PYTHONWARNINGS=ignore
export PYTHONPATH="$REPO/bindings/python:$MAVLINK"
HOME_LOCATION=-35.363261,149.165230,584,353
DEFAULTS=/opt/ardupilot/default_params
SECOND=$DEFAULTS/skynet-second.parm
BIN=/opt/ardupilot/bin

echo "ArduPilot at $(cat /opt/ardupilot/commit)"
status=0

echo "== the reference implementation"
(cd "$MAVLINK" && $PY layout_check.py | tail -3) || status=1

# One process per vehicle, one working directory each (the firmware writes its parameter store and logs to
# the current directory). Instance N listens on 5760 + 10N.
mkdir -p /tmp/sitl
start() {
  name=$1; instance=$2; shift 2
  mkdir -p "/tmp/sitl/$name"
  (cd "/tmp/sitl/$name" && exec "$@" -I"$instance" --home "$HOME_LOCATION" --speedup 1 >log 2>&1) &
}
start copter 0 $BIN/arducopter --model + --defaults $DEFAULTS/copter.parm,$SECOND
start rover  1 $BIN/ardurover  --model rover --defaults $DEFAULTS/rover.parm,$SECOND
start plane  2 $BIN/arduplane  --model plane --defaults $DEFAULTS/skynet-plane.parm,$SECOND
start boat   3 $BIN/ardurover  --model motorboat --defaults $DEFAULTS/motorboat.parm,$SECOND,$HERE/boat.parm
start sub    4 $BIN/ardusub    --model vectored --defaults $DEFAULTS/sub.parm,$SECOND

waited=0
for port in 5760 5770 5780 5790 5800; do
  until $PY - "$port" <<'PROBE' 2>/dev/null
import socket, sys
socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=1).close()
PROBE
  do
    waited=$((waited + 1))
    if [ $waited -gt 60 ]; then echo "::error::the vehicle on $port never listened."; tail -3 /tmp/sitl/*/log; exit 1; fi
    sleep 1
  done
done
sleep 3

for spec in "copter 5760 5762" "rover 5770 5772" "plane 5780 5782" "boat 5790 5792" "sub 5800 5802"; do
  set -- $spec
  echo "== $1, against the firmware"
  # Not a pipeline: a pipe's status is its last command's, and this script must fail when the suite does.
  (cd "$MAVLINK" && $PY live.py "$1" "tcp:127.0.0.1:$2" ${3:+"tcp:127.0.0.1:$3"} > "/tmp/skynet-mavlink-live-$1.txt" 2>&1) || status=1
  cat "/tmp/skynet-mavlink-live-$1.txt"
done

echo "== the sim half of SCTS, through the real ABI"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
# The third word is a configuration key the family needs: a submarine does not open without a `stop_mode`, which is the operator's decision.
for spec in "copter 5760" "rover 5770" "plane 5780" "boat 5790" "sub 5800 alt_hold"; do
  set -- $spec
  SKYNET_PYTHON=$PY SKYNET_PYTHON_DRIVER=skynet_mavlink \
    "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
    --config "{\"link\":\"tcp:127.0.0.1:$2\",\"open_timeout_ms\":30000${3:+,\"stop_mode\":\"$3\"}}" > "/tmp/skynet-mavlink-scts-$1.txt" 2>&1 || status=1
  echo "-- $1"
  sed 's/\x1b\[[0-9;]*m//g' "/tmp/skynet-mavlink-scts-$1.txt" | grep -E "FAIL|passed|guest|no tier" | head -20
done

exit $status

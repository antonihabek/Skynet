#!/bin/sh
# The Reachy Mini row against the maker's own daemon in simulation (DRY-COVERAGE-PLAN.md item 12.E.12), run by `make check-sim-docker` inside `sim/Dockerfile`'s image:
#
#   1. `layout_check.py`: this driver's routes, models and limits against the maker's source and its own pydantic models, run;
#   2. `reachy-mini-daemon --sim --headless --no-media` started, and waited for until its status says it is running;
#   3. `live.py`: the driver against it, with the maker's own models reading the same robot as the second opinion;
#   4. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> the daemon).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-sim.sh <skynet binary>`, never `./`.
set -eu

SKYNET=$1
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)

export PYTHONPATH="$REPO/bindings/python:$BASE:${PYTHONPATH:-}"
export PYTHONWARNINGS=ignore
PY=/opt/venv/bin/python
status=0

echo "== $(cat /opt/reachy_mini.txt)"
echo "== layout: this driver's routes, models and limits against the maker's"
(cd "$BASE" && REACHY_MINI=/opt/reachy_mini $PY layout_check.py | tail -4) || status=1

PORT=8000
/opt/venv/bin/reachy-mini-daemon --sim --headless --no-media --no-wake-up-on-start --no-goto-sleep-on-stop --no-preload-datasets --fastapi-host 127.0.0.1 --fastapi-port $PORT >/tmp/daemon.log 2>&1 &
DAEMON=$!
trap 'kill $DAEMON 2>/dev/null || true' EXIT
waited=0
until $PY -c "import json,urllib.request,sys; d=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/api/daemon/status', timeout=2)); sys.exit(0 if d.get('state')=='running' else 1)" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 120 ]; then echo "::error::the daemon never reached state 'running'."; tail -30 /tmp/daemon.log; exit 1; fi
  sleep 1
done
sleep 2
echo "== the maker's daemon is running ($($PY -c "import json,urllib.request; d=json.load(urllib.request.urlopen('http://127.0.0.1:$PORT/api/daemon/status')); print('version', d.get('version'), 'simulation_enabled', d.get('simulation_enabled'))"))"

(cd "$BASE" && REACHY_DAEMON_LOG=/tmp/daemon.log $PY live.py 127.0.0.1 $PORT) || status=1

echo ""
echo "== the sim half of SCTS, through the real ABI (the maker's daemon, simulated)"
# The operator's step, with the maker's own route: the driver refuses a robot outside the ranges it is given, and `live.py` left this one wherever its last check did.
$PY - <<EOF
import json, time, urllib.request
base = "http://127.0.0.1:$PORT"
urllib.request.urlopen(urllib.request.Request(base + "/api/move/play/wake_up", data=b"", method="POST"), timeout=5).read()
deadline = time.monotonic() + 15
while json.load(urllib.request.urlopen(base + "/api/move/running", timeout=5)) and time.monotonic() < deadline:
    time.sleep(0.1)
time.sleep(0.5)
EOF
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
CONFIG='{"host":"127.0.0.1","port":8000,"stop_mode":"hold","limits":{"head_x":[-0.01,0.01],"head_y":[-0.01,0.01],"head_z":[-0.01,0.01],"antenna_right":[-1.0,1.0],"antenna_left":[-1.0,1.0]},"speeds":{"head_x":0.05,"head_y":0.05,"head_z":0.05,"head_roll":0.5,"head_pitch":0.5,"head_yaw":0.5,"body_yaw":0.5,"antenna_right":1.0,"antenna_left":1.0},"roles":{"head_x":"head.platform.slide#0","head_y":"head.platform.slide#1","head_z":"head.platform.slide#2","head_roll":"head.platform.roll","head_pitch":"head.platform.pitch","head_yaw":"head.platform.yaw","body_yaw":"torso.body.yaw","antenna_right":"aux.antenna_right.rotate","antenna_left":"aux.antenna_left.rotate"}}'
SKYNET_PYTHON=$PY SKYNET_PYTHON_DRIVER=skynet_reachy_mini \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" --config "$CONFIG" > /tmp/skynet-reachy-scts.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-reachy-scts.txt | grep -E "FAIL|passed|guest|no tier" | head -12

exit $status

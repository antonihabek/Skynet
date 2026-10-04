#!/bin/sh
# The SCHUNK row against the maker's own protocol simulator (DRY-COVERAGE-PLAN.md item 12.E.9), run by `make check-sim-docker` inside `sim/Dockerfile`'s image:
#
#   1. `layout_check.py`: this driver's bytes against the maker's own client, run;
#   2. three copies of `schunk_gripper_dummy` started through `dummy_recorder.py` (an EGU 60 and an EGK 25 on EtherNet/IP, an EGU 60 on PROFINET), and waited for until each answers;
#   3. `live.py`: the driver against them, with the maker's own client reading the same modules as the second opinion and the recorder's log as the record of what the driver put on the wire;
#   4. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> the maker's simulator).
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

echo "== $(cat /opt/schunk.txt)"
echo "== layout: this driver's bytes against the maker's own client"
(cd "$BASE" && SCHUNK_REPO=/opt/schunk_mechatronic_gripper $PY layout_check.py | tail -4) || status=1

rm -f /tmp/frames-egu.jsonl /tmp/frames-egk.jsonl /tmp/frames-pn.jsonl
SCHUNK_DUMMY_LOG=/tmp/frames-egu.jsonl $PY "$HERE/dummy_recorder.py" --port 8000 > /tmp/dummy-egu.log 2>&1 &
EGU=$!
SCHUNK_DUMMY_LOG=/tmp/frames-egk.jsonl $PY "$HERE/dummy_recorder.py" --port 8001 --gripper EGK_25_EI_M_B > /tmp/dummy-egk.log 2>&1 &
EGK=$!
SCHUNK_DUMMY_LOG=/tmp/frames-pn.jsonl $PY "$HERE/dummy_recorder.py" --port 8002 --gripper EGU_60_PN_M_B > /tmp/dummy-pn.log 2>&1 &
PN=$!
trap 'kill $EGU $EGK $PN 2>/dev/null || true' EXIT
for port in 8000 8001 8002; do
  waited=0
  until $PY -c "import urllib.request,sys; urllib.request.urlopen('http://127.0.0.1:$port/adi/data.json?inst=0x0040&count=1', timeout=2)" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 30 ]; then echo "::error::the maker's simulator on port $port never answered."; tail -20 /tmp/dummy-*.log; exit 1; fi
    sleep 1
  done
done
echo "== the maker's simulators are up (EGU 60 EtherNet/IP :8000, EGK 25 EtherNet/IP :8001, EGU 60 PROFINET :8002)"

(cd "$BASE" && SCHUNK_FRAMES=/tmp/frames-egu.jsonl $PY live.py 127.0.0.1 8000 8001 8002) || status=1

echo ""
echo "== the sim half of SCTS, through the real ABI (the maker's simulator)"
# The operator's step, with the maker's own client: the simulator drops 'ready for operation' on every write (F254) and the driver does not bring a module out of that, so the module live.py left is acknowledged
# first, as a person would where the gripper is.
$PY - <<EOF
import time
from schunk_gripper_library.driver import Driver
client = Driver()
client.connect(host="127.0.0.1", port=8000, update_cycle=0.05)
client.acknowledge()
deadline = time.monotonic() + 5
while time.monotonic() < deadline and not (client.get_status_bit(0) == 1 and client.get_status_bit(7) == 0):
    time.sleep(0.05)
client.disconnect()
EOF
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
CONFIG='{"host":"127.0.0.1","port":8000,"stop_mode":"controlled","speed":0.02,"simulator":true,"roles":{"jaw":"right_hand.gripper.grip"}}'
SKYNET_PYTHON=$PY SKYNET_PYTHON_DRIVER=skynet_schunk \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" --config "$CONFIG" > /tmp/skynet-schunk-scts.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-schunk-scts.txt | grep -E "FAIL|passed|guest|no tier" | head -12

exit $status

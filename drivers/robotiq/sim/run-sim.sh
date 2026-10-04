#!/bin/sh
# The Robotiq row against the maker's own fake gripper (DRY-COVERAGE-PLAN.md item 12.E.8), run by `make check-sim-docker` inside `sim/Dockerfile`'s image:
#
#   1. `sim/build.sh`: the maker's SDK, unmodified, and four programs against it;
#   2. `layout_check.py`: this driver's bytes against the maker's own client, run, and the maker's own client driving the simulated gripper written here;
#   3. `live.py`: the driver against the maker's own fake gripper on a TCP port and a pseudo-terminal, with the maker's own client reading the same gripper;
#   4. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> the maker's fake, on its pseudo-terminal).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-sim.sh <skynet binary>`, never `./`. `ROBOTIQ_REPO` and `ROBOTIQ_BUILD` say where the maker's checkout is and where to
# build (the image's /opt/robotiq_grippers and a temporary directory), so that the same script runs outside the image.
set -eu

SKYNET=$1
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)
ROBOTIQ_REPO=${ROBOTIQ_REPO:-/opt/robotiq_grippers}
ROBOTIQ_BUILD=${ROBOTIQ_BUILD:-/tmp/robotiq-sim}

export PYTHONPATH="$REPO/bindings/python:$BASE:${PYTHONPATH:-}"
export PYTHONWARNINGS=ignore
export ROBOTIQ_SIM="$ROBOTIQ_BUILD"
status=0

echo "== $(cat /opt/robotiq.txt 2>/dev/null || git -C "$ROBOTIQ_REPO" log -1 --format='robotiq grippers %H %cs')"
sh "$HERE/build.sh" "$ROBOTIQ_REPO" "$ROBOTIQ_BUILD" || exit 1

echo "== layout: this driver's bytes against the maker's own client"
(cd "$BASE" && python3 layout_check.py | grep -E "FAIL|checks,") || status=1

echo ""
echo "== live: the driver against the maker's own fake gripper, with the maker's own client looking on"
(cd "$BASE" && python3 live.py | grep -E "FAIL|checks,") || status=1

echo ""
echo "== the sim half of SCTS, through the real ABI (the maker's fake gripper on a pseudo-terminal)"
rm -f /tmp/robotiq-fake.out
"$ROBOTIQ_BUILD/robotiq-fake-server" --tcp 0 --pty < /dev/null > /tmp/robotiq-fake.out 2>&1 &
FAKE=$!
trap 'kill $FAKE 2>/dev/null || true' EXIT
waited=0
until grep -q '^pty ' /tmp/robotiq-fake.out 2>/dev/null && grep -q '^tcp ' /tmp/robotiq-fake.out 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 50 ]; then echo "::error::the maker's fake gripper never announced itself."; cat /tmp/robotiq-fake.out; exit 1; fi
  sleep 0.1
done
PTY=$(sed -n 's/^pty //p' /tmp/robotiq-fake.out | head -1)
PORT=$(sed -n 's/^tcp 127.0.0.1:\([0-9]*\)$/\1/p' /tmp/robotiq-fake.out | head -1)
# The operator's step, with the maker's own client: the maker's fake starts as a gripper that has not been activated, and activating one is a decision for a person (a calibration sweep).
"$ROBOTIQ_BUILD/robotiq-client-probe" 127.0.0.1 "$PORT" 9 > /tmp/robotiq-activate.txt 2>&1 || { echo "::error::the maker's own client could not activate the maker's fake:"; cat /tmp/robotiq-activate.txt; exit 1; }
CONFIG="{\"device\":\"$PTY\",\"model\":\"2F-85\",\"stop_mode\":\"halt\",\"speed\":0.1,\"effort\":0.5,\"simulator\":true,\"roles\":{\"jaw\":\"right_hand.gripper.grip\"}}"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_robotiq \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" --config "$CONFIG" > /tmp/skynet-robotiq-scts.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-robotiq-scts.txt | grep -E "FAIL|passed|guest|no tier" | head -12

exit $status

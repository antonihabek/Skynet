#!/bin/sh
# The client half of `sim/run-px4.sh`, run inside `sim/px4-client.Dockerfile`'s image, in the network namespace of a PX4 SIH container:
#
#   1. `live_px4.py`: the driver against the firmware in real time, with `pymavlink` reading the same vehicle on PX4's second MAVLink
#      instance (UDP 14030) as a second opinion;
#   2. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> the firmware).
#
# PX4 sends MAVLink to 127.0.0.1:14540 by itself, so the driver listens there (`udpin`) and replies to whatever it hears from. A script rather than
# a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-px4-client.sh <copter|plane|rover> <skynet binary>`, never `./`.
set -eu

FAMILY=$1
SKYNET=$2
HERE=$(cd "$(dirname "$0")" && pwd)
MAVLINK=$(dirname "$HERE")
REPO=$(cd "$MAVLINK/../.." && pwd)
PY=/opt/venv/bin/python
export PYTHONWARNINGS=ignore
export PYTHONPATH="$REPO/bindings/python:$MAVLINK"
status=0

(cd "$MAVLINK" && $PY live_px4.py "$FAMILY" udpin:0.0.0.0:14540 udpin:0.0.0.0:14030) || status=1

echo "== the sim half of SCTS, through the real ABI"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=$PY SKYNET_PYTHON_DRIVER=skynet_mavlink \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
  --config '{"link":"udpin:0.0.0.0:14540","open_timeout_ms":30000}' > "/tmp/skynet-px4-scts-$FAMILY.txt" 2>&1 || status=1
echo "-- $FAMILY"
sed 's/\x1b\[[0-9;]*m//g' "/tmp/skynet-px4-scts-$FAMILY.txt" | grep -E "FAIL|passed|guest|no tier" | head -20

exit $status

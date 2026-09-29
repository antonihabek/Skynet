#!/bin/sh
# The xArm row against UFACTORY's controller firmware (DRY-COVERAGE-PLAN.md Phase 10, item 10.5), run
# inside `sim/Dockerfile`'s image, in the simulator container's network namespace, by
# `make check-sim-docker`:
#
#   1. `layout_check.py` — this driver's constants against the vendor's own SDK source;
#   2. the suite's live block — the driver against the firmware, with the vendor's own `XArmAPI` as a
#      second opinion on every state the driver claims;
#   3. the sim half of SCTS through the real ABI, against the same firmware.
#
# A script rather than a recipe for the reason every run-*.sh in drivers/ gives: it runs suites against
# a process and reads what it did. `sh sim/run-sim.sh <skynet-binary>`, never `./`: a file's execute bit
# is not carried by every checkout.
set -eu

SKYNET=$1
# The arm the firmware was started as: `xarm_start.sh AXIS TYPE`. UFACTORY's image carries the xArm 5 (5 5), the
# xArm 6 (6 6), the Lite 6 (6 9), the 850 (6 12) and the xArm 7 (7 7).
AXIS=${2:-6}
TYPE=${3:-6}
HERE=$(cd "$(dirname "$0")" && pwd)
XARM=$(dirname "$HERE")
REPO=$(cd "$XARM/../.." && pwd)
export XARM_SDK=/opt/xarm-sdk
export PYTHONWARNINGS=ignore

echo "xArm SDK at $(cat /opt/xarm-sdk.commit)"
status=0

echo "== the vendor's source"
(cd "$XARM" && PYTHONPATH="$XARM" python3 layout_check.py | tail -4) || status=1

echo "== the operator's step, with UFACTORY's own client"
# The driver refuses to start over a latched error (see the header of clear_startup_error.py); a person clears it, and here that is
# the vendor's client, out loud. A controller that still holds one afterwards is a failure.
python3 "$HERE/clear_startup_error.py" 127.0.0.1 || status=1

echo "== the live block, against the firmware"
(cd "$XARM" && PYTHONPATH="$REPO/bindings/python:$XARM" python3 selftest.py --live 127.0.0.1 "$AXIS" "$TYPE") || status=1

# Roles are a human's assertion (section 63.4); these are a fixture's, one per axis count, in the order the arms
# are built (the base yaw, the shoulder and elbow bends, the wrist).
case "$AXIS" in
  5) JOINTS='[{"name":"joint1","role":"right_arm.shoulder.yaw"},{"name":"joint2","role":"right_arm.shoulder.pitch"},{"name":"joint3","role":"right_arm.elbow.flex"},{"name":"joint4","role":"right_arm.wrist.pitch"},{"name":"joint5","role":"right_arm.wrist.roll"}]' ;;
  6) JOINTS='[{"name":"joint1","role":"right_arm.shoulder.yaw"},{"name":"joint2","role":"right_arm.shoulder.pitch"},{"name":"joint3","role":"right_arm.elbow.flex"},{"name":"joint4","role":"right_arm.wrist.pitch"},{"name":"joint5","role":"right_arm.wrist.yaw"},{"name":"joint6","role":"right_arm.wrist.roll"}]' ;;
  7) JOINTS='[{"name":"joint1","role":"right_arm.shoulder.yaw"},{"name":"joint2","role":"right_arm.shoulder.pitch"},{"name":"joint3","role":"right_arm.upper_arm.roll"},{"name":"joint4","role":"right_arm.elbow.flex"},{"name":"joint5","role":"right_arm.forearm.roll"},{"name":"joint6","role":"right_arm.wrist.pitch"},{"name":"joint7","role":"right_arm.wrist.roll"}]' ;;
  *) echo "::error::no fixture for an xArm of $AXIS axes."; exit 1 ;;
esac

echo "== the sim half of SCTS, through the real ABI"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_xarm \
  PYTHONPATH="$REPO/bindings/python:$XARM" \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
  --config "{\"host\":\"127.0.0.1\",\"joints\":$JOINTS}" \
  > /tmp/skynet-xarm-scts.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-xarm-scts.txt | grep -E "FAIL|passed|guest|no tier" | head -20

exit $status

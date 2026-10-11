#!/bin/sh
# The Tello row against djitellopy (DRY-COVERAGE-PLAN.md item 12.E.11), run by `make check-sdk-docker` inside `sim/Dockerfile`'s image:
#
#   1. `layout_check.py`: this driver's text and state string against djitellopy, run, and the two moving a simulated drone written here, each reading where the other put it;
#   2. the driver's own suite;
#   3. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> a simulated drone in the same process).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-sdk.sh <skynet binary>`, never `./`. `TELLO_PYTHON` says where the image's interpreter is, so that the same script runs
# outside the image.
set -eu

SKYNET=$1
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)
TELLO_PYTHON=${TELLO_PYTHON:-/opt/tello-venv/bin/python}

export PYTHONPATH="$REPO/bindings/python:$BASE:${PYTHONPATH:-}"
export PYTHONWARNINGS=ignore
status=0

echo "== $(tr '\n' ' ' < /opt/tello.txt 2>/dev/null || echo 'djitellopy (version not recorded)')"

echo "== layout: this driver's text and state string against djitellopy"
(cd "$BASE" && "$TELLO_PYTHON" layout_check.py | grep -E "FAIL|checks,") || status=1

echo ""
echo "== the driver's own suite"
(cd "$BASE" && python3 selftest.py | grep -E "FAIL|checks,") || status=1

echo ""
echo "== the sim half of SCTS, through the real ABI (a simulated drone in the host's process)"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_tello SKYNET_TELLO_SIM=1 \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" --config '{}' > /tmp/skynet-tello-scts.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-tello-scts.txt | grep -E "FAIL|passed|guest|no tier" | head -12

exit $status

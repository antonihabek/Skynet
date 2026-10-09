#!/bin/sh
# The LEAP Hand row against ROBOTIS's own SDK (DRY-COVERAGE-PLAN.md item 12.E), run by `make check-sdk-docker` inside `sim/Dockerfile`'s image:
#
#   1. `layout_check.py`: this driver's wire and registers against the SDK, run, and the two moving a simulated hand written here, each reading where the other put it;
#   2. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> a simulated hand in the same process).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-sdk.sh <skynet binary>`, never `./`. `LEAP_PYTHON` and `LEROBOT_TABLES` say where the image's interpreter and table are,
# so that the same script runs outside the image.
set -eu

SKYNET=$1
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)
LEAP_PYTHON=${LEAP_PYTHON:-/opt/leap-venv/bin/python}
LEROBOT_TABLES=${LEROBOT_TABLES:-/opt/leap-vendor/lerobot_tables.py}

export PYTHONPATH="$REPO/bindings/python:$BASE:${PYTHONPATH:-}"
export PYTHONWARNINGS=ignore
export LEROBOT_TABLES
status=0

echo "== $(tr '\n' ' ' < /opt/leap.txt 2>/dev/null || echo 'dynamixel-sdk (version not recorded)')"

echo "== layout: this driver's wire and registers against ROBOTIS's own SDK"
(cd "$BASE" && "$LEAP_PYTHON" layout_check.py | grep -E "FAIL|checks,") || status=1

echo ""
echo "== the driver's own suite"
(cd "$BASE" && python3 selftest.py | grep -E "FAIL|checks,") || status=1

echo ""
echo "== the sim half of SCTS, through the real ABI (a simulated hand in the host's process)"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_leap SKYNET_LEAP_SIM=1 \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" --config '{}' > /tmp/skynet-leap-scts.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-leap-scts.txt | grep -E "FAIL|passed|guest|no tier" | head -12

exit $status

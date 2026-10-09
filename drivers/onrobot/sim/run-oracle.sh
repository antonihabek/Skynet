#!/bin/sh
# The OnRobot row against the maker's own client library (DRY-COVERAGE-PLAN.md item 12.E.8), run by `make check-oracle-docker` inside `sim/Dockerfile`'s image:
#
#   1. `layout_check.py`: this driver's registers against the maker's own client, run, and the two moving a simulated gripper written here, each reading where the other put it;
#   2. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> a simulated gripper in the same process).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-oracle.sh <skynet binary>`, never `./`. `ONROBOT_ORACLE` says where the built oracle is (the image's), so that the same
# script runs outside the image.
set -eu

SKYNET=$1
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)
ONROBOT_ORACLE=${ONROBOT_ORACLE:-/opt/onrobot-oracle/onrobot-oracle}

export PYTHONPATH="$REPO/bindings/python:$BASE:${PYTHONPATH:-}"
export PYTHONWARNINGS=ignore
export ONROBOT_ORACLE
status=0

echo "== $(cat /opt/onrobot.txt 2>/dev/null || echo 'onrobot-tool-api (release not recorded)')"

echo "== layout: this driver's registers against the maker's own client"
(cd "$BASE" && python3 layout_check.py | grep -E "FAIL|checks,") || status=1

echo ""
echo "== the sim half of SCTS, through the real ABI (a simulated RG2 in the host's process)"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_onrobot SKYNET_ONROBOT_SIM=1 \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" --config '{}' > /tmp/skynet-onrobot-scts.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-onrobot-scts.txt | grep -E "FAIL|passed|guest|no tier" | head -12

exit $status

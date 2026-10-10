#!/bin/sh
# The Dobot row against Dobot's own client (DRY-COVERAGE-PLAN.md item 12.E.13), run by `make check-sdk-docker` inside `sim/Dockerfile`'s image:
#
#   1. `layout_check.py`: this driver's text and packet layout against the client, run, and the two moving a simulated controller written here, each reading where the other put it;
#   2. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> a simulated controller in the same process).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-sdk.sh <skynet binary>`, never `./`. `DOBOT_PYTHON` and `DOBOT_SDK` say where the image's interpreter and the client
# are, so that the same script runs outside the image.
set -eu

SKYNET=$1
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)
DOBOT_PYTHON=${DOBOT_PYTHON:-/opt/dobot-venv/bin/python}
DOBOT_SDK=${DOBOT_SDK:-/opt/dobot-vendor/sdk}

export PYTHONPATH="$REPO/bindings/python:$BASE:${PYTHONPATH:-}"
export PYTHONWARNINGS=ignore
export DOBOT_SDK
status=0

echo "== $(tr '\n' ' ' < /opt/dobot.txt 2>/dev/null || echo 'numpy and the client (version not recorded)')"

echo "== layout: this driver's text and packets against Dobot's own client"
(cd "$BASE" && "$DOBOT_PYTHON" layout_check.py | grep -E "FAIL|checks,") || status=1

echo ""
echo "== the driver's own suite"
(cd "$BASE" && python3 selftest.py | grep -E "FAIL|checks,") || status=1

echo ""
echo "== the sim half of SCTS, through the real ABI (a simulated controller in the host's process)"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_dobot SKYNET_DOBOT_SIM=1 \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" --config '{}' > /tmp/skynet-dobot-scts.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-dobot-scts.txt | grep -E "FAIL|passed|guest|no tier" | head -12

exit $status

#!/bin/sh
# The Rainbow row against Rainbow Robotics' own client (DRY-COVERAGE-PLAN.md item 12.E.14), run by `make check-sdk-docker` inside `sim/Dockerfile`'s image:
#
#   1. `layout_check.py`: this driver's text and packet layout against rbpodo, run, and the two moving a simulated control box written here, each reading where the other put it;
#   2. the sim half of SCTS through the real ABI (`skynet driver check` -> python-host -> this driver -> a simulated control box in the same process).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-sdk.sh <skynet binary>`, never `./`. `RBPODO_PYTHON` says where the image's interpreter is, so that the same script runs outside the image.
set -eu

SKYNET=$1
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)
RBPODO_PYTHON=${RBPODO_PYTHON:-/opt/rainbow-vendor/venv/bin/python}

export PYTHONPATH="$REPO/bindings/python:$BASE:${PYTHONPATH:-}"
export PYTHONWARNINGS=ignore
status=0

echo "== $(tr '\n' ' ' < /opt/rainbow.txt 2>/dev/null || echo 'rbpodo and numpy (version not recorded)')"

echo "== layout: this driver's text and packets against Rainbow's own client"
# The exit status is the check's own, and the summary is read from the file: a pipe into grep would report grep's.
(cd "$BASE" && "$RBPODO_PYTHON" layout_check.py > /tmp/rainbow-layout.txt 2>&1) || status=1
grep -E "FAIL|checks," /tmp/rainbow-layout.txt || status=1

echo ""
echo "== the driver's own suite"
(cd "$BASE" && python3 selftest.py > /tmp/rainbow-selftest.txt 2>&1) || status=1
grep -E "FAIL|checks," /tmp/rainbow-selftest.txt || status=1

echo ""
echo "== the sim half of SCTS, through the real ABI (a simulated control box in the host's process)"
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_rainbow SKYNET_RAINBOW_SIM=1 \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" --config '{}' > /tmp/skynet-rainbow-scts.txt 2>&1 || status=1
sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-rainbow-scts.txt | grep -E "FAIL|passed|guest|no tier" | head -12

exit $status

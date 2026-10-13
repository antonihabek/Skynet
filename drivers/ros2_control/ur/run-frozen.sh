#!/bin/sh
# URSim under the UR5e's bridge, and Docker's freezer between them (DRY-COVERAGE-PLAN.md item 12.C.4; `frozen.py` is the driver's half, `frozen-host.sh` the shared host half).
#
#   sh ur/run-frozen.sh            # from drivers/ros2_control; `make frozen-ursim-docker` calls it
#
# Variables, with the defaults the Makefile gives them: URSIM_IMAGE, URSIM_NAME, URSIM_MODEL, UR_IMAGE; REPO is the repository root (the directory two above this one).
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=${REPO:-$(cd "$BASE/../.." && pwd)}
: "${URSIM_IMAGE:?URSIM_IMAGE is not set; run this through make frozen-ursim-docker}"
: "${URSIM_NAME:=skynet-ursim}"
: "${URSIM_MODEL:=UR5}"
: "${UR_IMAGE:=skynet-ur-ros}"
. "$BASE/frozen-host.sh"

cleanup() {
  docker unpause "$URSIM_NAME" >/dev/null 2>&1
  docker rm -f "$URSIM_NAME" "${URSIM_NAME}-driver" >/dev/null 2>&1
}
trap cleanup EXIT INT TERM

docker rm -f "$URSIM_NAME" "${URSIM_NAME}-driver" >/dev/null 2>&1
docker run -d --rm --name "$URSIM_NAME" -e ROBOT_MODEL="$URSIM_MODEL" "$URSIM_IMAGE" >/dev/null || { echo "::error::URSim did not start. Nothing was measured."; exit 1; }
frozen_cycle "$URSIM_NAME" "$UR_IMAGE" "$REPO" 'make clean >/dev/null && make frozen VENDOR_PREFIX=/opt/ur_ws/install FROZEN_DIR=/handshake'
exit $?

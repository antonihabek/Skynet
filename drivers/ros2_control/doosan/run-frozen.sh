#!/bin/sh
# DRCF, Doosan's controller emulator, under the bridge and Doosan's own plugin, with Docker's freezer between them (DRY-COVERAGE-PLAN.md item 12.C.4; `frozen.py` is the driver's half,
# `frozen-host.sh` the shared host half).
#
#   sh doosan/run-frozen.sh        # from drivers/ros2_control; `make frozen-drcf-docker` calls it
#
# The emulator is started as `check-drcf-docker` starts it (`-dit`, because its entrypoint backgrounds DRCF and then runs `su dsr`, which returns at once without a terminal), and the
# run waits for DRCF's port and for the emulator's own account of the arm it is, so that the body opened is the arm named.
# Variables, with the defaults the Makefile gives them: DRCF_EMULATOR, DRCF_IMAGE, DRCF_ROBOT_MODEL, DRCF_MODEL; REPO is the repository root.
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=${REPO:-$(cd "$BASE/../.." && pwd)}
: "${DRCF_EMULATOR:?DRCF_EMULATOR is not set; run this through make frozen-drcf-docker}"
: "${DRCF_IMAGE:=skynet-doosan-drcf}"
: "${DRCF_ROBOT_MODEL:=M1013}"
: "${DRCF_MODEL:=m1013}"
NAME=${DRCF_NAME:-skynet-drcf}
. "$BASE/frozen-host.sh"

cleanup() {
  docker unpause "$NAME" >/dev/null 2>&1
  docker rm -f "$NAME" "${NAME}-driver" >/dev/null 2>&1
}
trap cleanup EXIT INT TERM

docker rm -f "$NAME" "${NAME}-driver" >/dev/null 2>&1
docker run -dit --privileged --rm --name "$NAME" --env ROBOT_MODEL="$DRCF_ROBOT_MODEL" "$DRCF_EMULATOR" >/dev/null || { echo "::error::DRCF did not start. Nothing was measured."; exit 1; }
waited=0
until docker exec "$NAME" grep -qi ':3039 ' /proc/net/tcp 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 60 ]; then
    echo "::error::DRCF never listened on 12345 inside $NAME. Its log:"; docker logs "$NAME" 2>&1 | tail -20; exit 1
  fi
  sleep 1
done
waited=0
until docker logs "$NAME" 2>&1 | grep -q "Robot Model Number: $DRCF_ROBOT_MODEL[[:space:]]*$"; do
  waited=$((waited + 1))
  if [ "$waited" -gt 30 ]; then
    echo "::error::DRCF never said it is a $DRCF_ROBOT_MODEL. A URDF alone would open against any arm, so the emulator's own account of itself is what says this run met the arm it names."; exit 1
  fi
  sleep 1
done
echo "DRCF says: $(docker logs "$NAME" 2>&1 | grep 'Robot Model Number' | tail -1)"
frozen_cycle "$NAME" "$DRCF_IMAGE" "$REPO" "make clean >/dev/null && make frozen-drcf VENDOR_PREFIX=/opt/doosan_ws/install FROZEN_DIR=/handshake DRCF_MODEL=$DRCF_MODEL"
exit $?

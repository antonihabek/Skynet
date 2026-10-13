#!/bin/sh
# ARCS, AUBO's controller software, under the bridge and AUBO's own plugin, with Docker's freezer between them (DRY-COVERAGE-PLAN.md item 12.C.4; `frozen.py` is the driver's half,
# `frozen-host.sh` the shared host half).
#
#   sh aubo/run-frozen.sh          # from drivers/ros2_control; `make frozen-arcs-docker` calls it
#
# ARCS is started as `check-arcs-docker` starts it and waited for on the two ports it serves (30004 and 30010). Variables, with the defaults the Makefile gives them: ARCS_IMAGE, AUBO_IMAGE;
# REPO is the repository root.
set -u

HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=${REPO:-$(cd "$BASE/../.." && pwd)}
: "${ARCS_IMAGE:=skynet-aubo-arcs}"
: "${AUBO_IMAGE:=skynet-aubo-ros}"
NAME=${ARCS_NAME:-skynet-arcs}
. "$BASE/frozen-host.sh"

cleanup() {
  docker unpause "$NAME" >/dev/null 2>&1
  docker rm -f "$NAME" "${NAME}-driver" >/dev/null 2>&1
}
trap cleanup EXIT INT TERM

docker rm -f "$NAME" "${NAME}-driver" >/dev/null 2>&1
docker run -d --rm --name "$NAME" --ulimit rtprio=99 "$ARCS_IMAGE" >/dev/null || { echo "::error::ARCS did not start. Nothing was measured."; exit 1; }
waited=0
until docker exec "$NAME" sh -c 'grep -qi ":7534 " /proc/net/tcp && grep -qi ":753A " /proc/net/tcp' 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 90 ]; then
    echo "::error::ARCS never listened on 30004 and 30010 inside $NAME. Its log:"; docker logs "$NAME" 2>&1 | tail -20; exit 1
  fi
  sleep 1
done
echo "ARCS: $(docker exec "$NAME" cat /opt/arcs/installer)"
frozen_cycle "$NAME" "$AUBO_IMAGE" "$REPO" "make clean >/dev/null && make frozen-arcs VENDOR_PREFIX=/opt/aubo_ws/install FROZEN_DIR=/handshake"
exit $?

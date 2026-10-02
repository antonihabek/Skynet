#!/bin/sh
# The MAVLink row against PX4's own firmware (DRY-COVERAGE-PLAN.md item 12.E.1), run on the host by `make check-px4-docker`:
#
#   for each of PX4's SIH vehicles — a multirotor, a fixed wing and a rover —
#     1. start PX4's published image with its interactive shell off (`px4 -d`: with it on and no terminal the shell redraws its prompt
#        as fast as it can and the container's log is gigabytes in seconds) and its own entrypoint bypassed (on Docker Desktop the
#        entrypoint redirects PX4's MAVLink to the host, and this is a loopback measurement);
#     2. run `sim/run-px4-client.sh` in a second container in the same network namespace, so the driver dials 127.0.0.1 and nothing is published.
#
# The image is pinned by digest: `PX4_IMAGE` and `PX4_COMMIT` (the abbreviated hash it was built from, which the live block holds the vehicle's
# own reported build to) are overridden together by the nightly's run against PX4's newest image, with `PX4_COMMIT=` empty.
#
#   sh sim/run-px4.sh <the repository, as docker mounts it> <the skynet binary, as the container sees it>
set -eu

REPO=$1
SKYNET=$2
PX4_IMAGE=${PX4_IMAGE:-px4io/px4-sitl-sih@sha256:357f7c1fb2f6cd37e5040a73f6bcb72aac42c9c6aaf536f6332bef15e33e3fb0}
PX4_COMMIT=${PX4_COMMIT-dd03e18fee}
CLIENT_IMAGE=${PX4_CLIENT_IMAGE:-skynet-px4-client}
status=0

echo "PX4 image $PX4_IMAGE"
for spec in "copter sihsim_quadx" "plane sihsim_airplane" "rover sihsim_rover_ackermann"; do
  set -- $spec
  family=$1
  name="skynet-px4-sim-$family"
  docker rm -f "$name" >/dev/null 2>&1 || true
  docker run -d --rm --name "$name" --entrypoint /opt/px4/bin/px4 -e "PX4_SIM_MODEL=$2" "$PX4_IMAGE" -d >/dev/null
  # PX4 says when it can arm; the live block waits for that itself, so this only has to be long enough for the MAVLink ports to exist.
  waited=0
  until docker logs --tail 40 "$name" 2>&1 | grep -q "Startup script returned successfully"; do
    waited=$((waited + 1))
    if [ "$waited" -gt 60 ]; then echo "::error::PX4's $family never finished starting."; docker logs --tail 20 "$name" 2>&1 || true; status=1; break; fi
    sleep 1
  done
  sleep 3
  echo "== $family, against PX4's firmware ($2)"
  # --cap-add NET_ADMIN so that the live block can put delay, jitter and loss on the loopback (`tc netem`); without it that block says it did not run.
  docker run --rm --cap-add NET_ADMIN --network "container:$name" -v "$REPO":/src -w /src/drivers/mavlink \
    -e "PX4_COMMIT=$PX4_COMMIT" "$CLIENT_IMAGE" sh sim/run-px4-client.sh "$family" "$SKYNET" || status=1
  docker rm -f "$name" >/dev/null 2>&1 || true
done

exit $status

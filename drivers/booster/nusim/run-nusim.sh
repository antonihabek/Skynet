#!/bin/sh
# The Booster row's live run against NUbots' simulator of the K1 (DRY-COVERAGE-PLAN.md item 12.D), run by `make check-nusim-docker`:
#
#   sh nusim/run-nusim.sh <skynet binary> <Booster SDK checkout>
#
#   1. NUSim (MIT) is fetched at its pinned commit by its SHA-256 (`fetch-nusim.sh`), its toolchain image is built from its own Dockerfile (Fast-DDS from source: minutes the first time) and the simulator is built
#      in that image. Nothing of it is committed, and nothing built leaves the containers or the work directory.
#   2. The simulator runs headless in a container on the Docker host's network, DDS domain 0. The driver runs in a second container on the same network (`nusim/Dockerfile`) with the repository and the SDK mounted.
#   3. The harness (`nusim_live.py`) moves each of the K1's 22 joints alone and reads the simulator's answer, then stops the body. The simulator is started again, fresh, and `skynet driver check` runs the sim half of
#      the conformance suite through the real ABI against it.
#
# `NUSIM_REF=main` takes the tip of the simulator's default branch (the nightly's second run). `NUSIM_WORK` is where the simulator's source and build live (default /tmp/skynet-nusim).
set -eu

SKYNET=$1
SDK=$2
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)
WORK=${NUSIM_WORK:-/tmp/skynet-nusim}
SIM_IMAGE=${NUSIM_SIM_IMAGE:-skynet-k1sim}
CLIENT_IMAGE=${NUSIM_CLIENT_IMAGE:-skynet-booster-nusim}
CONTAINER=skynet-nusim
CPUS=${NUSIM_CPUS:-}

cleanup() { docker rm -f "$CONTAINER" > /dev/null 2>&1 || true; }
trap cleanup EXIT INT TERM
cleanup

limit() { if [ -n "$CPUS" ]; then echo "--cpuset-cpus $CPUS"; fi; }

echo "== NUSim, fetched"
sh "$HERE/fetch-nusim.sh" "$WORK"
SRC="$WORK/NUSim"

echo "== NUSim, built in its own toolchain image"
# shellcheck disable=SC2046
docker build -q $(limit) -t "$SIM_IMAGE" -f "$SRC/mujoco/docker/Dockerfile" "$SRC/mujoco" > /dev/null
# shellcheck disable=SC2046
docker run --rm $(limit) -v "$SRC:/workspace/NUSim" -w /workspace/NUSim/mujoco --user "$(id -u):$(id -g)" "$SIM_IMAGE" \
  sh -c 'cmake -S . -B build-docker -GNinja > /dev/null && cmake --build build-docker -j"$(nproc)" > /dev/null && ls build-docker/bin/sim/soccer'

echo "== the client image"
docker build -q -t "$CLIENT_IMAGE" -f "$HERE/Dockerfile" "$HERE" > /dev/null

start_sim() {
  cleanup
  # shellcheck disable=SC2046
  docker run -d --name "$CONTAINER" --network host --ipc host $(limit) -v "$SRC:/workspace/NUSim" -w /workspace/NUSim/mujoco --user "$(id -u):$(id -g)" \
    "$SIM_IMAGE" ./build-docker/bin/sim/soccer --headless --field no-field > /dev/null
  waited=0
  until docker logs "$CONTAINER" 2>&1 | grep -q "controller attached"; do
    waited=$((waited + 1))
    if [ "$waited" -gt 100 ]; then
      echo "::error::the simulator never reported a controller attached. It said:"
      docker logs "$CONTAINER" 2>&1 | tail -20
      exit 1
    fi
    sleep 0.2
  done
  # the DDS endpoints are up a moment after the physics is
  sleep 2
}

client() {
  docker run --rm --network host --ipc host -v "$REPO:/src" -v "$SDK:/sdk:ro" -w /src/drivers/booster "$CLIENT_IMAGE" sh nusim/run-client.sh "$@"
}

echo "== the K1, joint by joint"
start_sim
HARNESS=0
client harness /sdk || HARNESS=$?

echo ""
echo "== the sim half of the conformance suite, through the real ABI, against the simulator"
start_sim
ABI=0
# the skynet binary is mounted from the checkout: it must be at a path under the repository
case "$SKYNET" in
  "$REPO"/*) INSIDE="/src${SKYNET#"$REPO"}" ;;
  *) echo "::error::the skynet binary must be inside the checkout ($REPO) to be seen from the client container: $SKYNET"; exit 2 ;;
esac
client abi /sdk "$INSIDE" || ABI=$?

echo ""
echo "== the simulator, as it ended"
docker logs "$CONTAINER" 2>&1 | grep -E "ERROR|WARN|fall" | sort | uniq -c | head -8 || true

[ "$HARNESS" -eq 0 ] || { echo "::error::the K1 harness failed ($HARNESS)."; exit "$HARNESS"; }
[ "$ABI" -eq 0 ] || { echo "::error::the conformance suite failed against the simulator ($ABI)."; exit "$ABI"; }

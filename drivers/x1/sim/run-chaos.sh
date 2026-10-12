#!/bin/sh
# The X1 driver under the link-chaos contract (DRY-COVERAGE-PLAN.md item 12.C.4), built and run from the tree mounted at the working directory, inside `skynet-x1-sim`:
#
#   sh sim/run-chaos.sh [--only profile]...
#
# The driver and the fake are built here, against the vendor's message types in the image (`/opt/src/x1_infer`, `/opt/ros/jazzy`); then `tools/faultproxy/chaos.py drivers/x1/chaos.py` puts the
# kernel's own link emulation on this container's loopback, which the driver and the fake share, for each profile in turn.
#
# The container's network is its own: `tc` here changes this container's loopback and nothing of the machine's, which is what `SKYNET_FAULTPROXY_NETEM_OK` says to the relay (outside a user
# namespace of its own it refuses, because the interface may be the machine's). It needs `--cap-add NET_ADMIN` and a kernel with `sch_netem`.
#
# The build is quiet on purpose, for `seed-run.sh`'s reason: the ROS and AimRT headers produce more warnings than anyone reads, so a build's output is hidden unless it fails, and then only
# its errors are shown. A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-chaos.sh`, never `./`.
set -eu

HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)
cd "$BASE"

export X1_INFER=${X1_INFER:-/opt/src/x1_infer}
export ROS_ROOT=${ROS_ROOT:-/opt/ros/jazzy}
# This container runs as root on a checkout that is somebody else's: no bytecode is written into it.
export PYTHONDONTWRITEBYTECODE=1

SCRATCH=$(mktemp -d)
trap 'rm -rf "$SCRATCH"' EXIT INT TERM

rm -f libskynet_x1.so fake_x1
if ! make -s lib fake >"$SCRATCH/build.log" 2>&1; then
  grep -E ': error:|\*\*\* |undefined reference' "$SCRATCH/build.log" | head -20
  echo "::error::the driver or the fake did not build. Nothing was run."
  exit 1
fi

export AMENT_PREFIX_PATH=$ROS_ROOT:$X1_INFER/build/install
export ROS_DISTRO=jazzy
export LD_LIBRARY_PATH=$ROS_ROOT/lib:$ROS_ROOT/lib/x86_64-linux-gnu:$X1_INFER/build:$X1_INFER/build/install/lib
export SKYNET_FAULTPROXY_NETEM_OK=1

if ! tc qdisc show dev lo >/dev/null 2>&1; then
  echo "::error::this container cannot use tc on its loopback. Run it with --cap-add NET_ADMIN, on a kernel with sch_netem. Nothing was run."
  exit 1
fi

python3 "$REPO/tools/faultproxy/chaos.py" "$BASE/chaos.py" "$@"

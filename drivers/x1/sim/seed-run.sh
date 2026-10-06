#!/bin/sh
# The X1's hermetic block, built and run from the tree mounted at the working directory — for the seeding campaign
# (`tools/seed-defects`, DRY-COVERAGE-PLAN.md item 12.C.3), which runs it once per seeded defect inside `skynet-x1-sim`.
#
#   sh seed-run.sh <path to a built skynet>
#
# Phase 1 of `run-check.sh` and nothing more: the fake on DDS domain 42, the driver's own suite against it, `skynet driver check`
# through the real ABI, and the fake's own count of the malformed, out-of-range and partial commands it received. Phase 2 is the
# vendor's simulator, which is the rung above (`make check-sim-docker`), and a defect only the simulator sees is not counted here.
#
# The build is quiet on purpose, for `drivers/ros2_control/mock/seed-run.sh`'s reason: the ROS and AimRT headers produce more
# warnings than a campaign keeps, so a build's output is hidden unless it fails, and then only its errors are shown, in the
# `file.cpp:12:3: error:` form the campaign reads as "the seeded source did not compile".
#
# Run as the caller's own user (`docker run --user`), so the campaign can remove the tree afterwards; the image's sim and its message
# types are where `make` expects them (`/opt/src/x1_infer`, `/opt/ros/jazzy`).
set -u

SKYNET=${1:?usage: sh seed-run.sh <path to a built skynet>}
export X1_INFER=${X1_INFER:-/opt/src/x1_infer}
export ROS_ROOT=${ROS_ROOT:-/opt/ros/jazzy}
SCRATCH=$(mktemp -d)
trap 'rm -rf "$SCRATCH"' EXIT INT TERM

rm -f libskynet_x1.so fake_x1 selftest sim_motion
if ! make -s lib fake suite >"$SCRATCH/build.log" 2>&1; then
  grep -E ': error:|\*\*\* |undefined reference' "$SCRATCH/build.log" | head -20
  exit 1
fi
TMPDIR="$SCRATCH" SKYNET="$SKYNET" X1_PHASE1_ONLY=1 sh ./run-check.sh

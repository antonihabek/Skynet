#!/bin/sh
# The mock-components block, built and run from the tree mounted at the working directory — for the seeding campaign
# (`tools/seed-defects`, DRY-COVERAGE-PLAN.md item 12.C.3), which runs it once per seeded defect inside `skynet-ros2-control-mock`.
#
#   sh seed-run.sh <path to a built skynet>
#
# Three steps, and the first one is quiet on purpose: the ROS headers produce a hundred kilobytes of deprecation warnings per build and
# a campaign keeps the tail of what a step printed, so a build's output is hidden unless it fails — and then only its errors are shown,
# in the `file.cpp:12:3: error:` form the campaign reads as "the seeded source did not compile", which is not a defect noticed.
#
#   1. build the suite and the library from the (patched) sources;
#   2. `make check` — the suite against mock_components, 447 checks on a tree with no defect in it;
#   3. `make check-conformance` — the sim half of SCTS through the real ABI, with the `skynet` it was given.
#
# Run as the caller's own user (`docker run --user`), so the campaign can remove the tree afterwards.
set -u

SKYNET=${1:?usage: sh seed-run.sh <path to a built skynet>}
LOG=${TMPDIR:-/tmp}/seed-run-build.log

rm -f selftest libskynet_ros2_control.so
if ! make -s selftest libskynet_ros2_control.so >"$LOG" 2>&1; then
  grep -E ': error:|\*\*\* ' "$LOG" | head -20
  exit 1
fi
make -s check || exit 1
make -s check-conformance SKYNET="$SKYNET" || exit 1

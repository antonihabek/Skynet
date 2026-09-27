#!/bin/sh
# The AUBO row against ARCS (DRY-COVERAGE-PLAN.md Phase 8, item 8.15), run inside `aubo/Dockerfile`'s
# image by `make check-arcs-docker`, in the network namespace of a fresh ARCS container: a session
# against the arm as ARCS starts it, powered off; the operator's power-on; the suite's ARCS block;
# then the sim half of SCTS; and after each, what the controller says about itself.
#
# A script rather than a recipe for the reason every run-check.sh in drivers/ gives: it drives a
# process it did not start, waits for it, runs suites against it and reads what it reports.
#
# The operator is AUBO's own SDK example, `example_startup.cpp`, compiled unmodified in the image:
# it powers the arm on and releases its brakes, which is what a person does at the pendant before a
# `ros2_control` session — AUBO's plugin commands nothing unless the robot is `Running`, and never
# readies it itself. The state reads are `arcs_state`, the one program in this row written here.
#
#   sh aubo/run-arcs.sh <aubo-workspace> [skynet-binary]
set -eu

WS=$1
SKYNET=${2:-}
STATE=$WS/bin/arcs_state

# AUBO's startup example takes the arm to `Running`, waiting through `Idle` and `BrakeReleasing`
# as it goes; anything else afterwards is a controller the next session cannot command.
power_on() {
  echo "== the operator: AUBO's example_startup (power on, release brakes)"
  "$WS/bin/aubo_example_startup" 2>&1 | tail -3
  "$STATE"
  "$STATE" | grep -q " mode=RobotModeType.Running " || {
    echo "::error::ARCS did not reach Running after AUBO's own startup example."; exit 1; }
}

# What the controller says after a phase. Servo mode on after the bridge has closed every session
# would be a controller left in the mode only the closed session's plugin could command.
after() {
  echo "== what ARCS says after $1"
  line=$("$STATE")
  echo "   $line"
  case "$line" in
    *" servo=0 "*) echo "   servo mode is off: the session left the controller as it found it" ;;
    *) echo "::error::$1: ARCS is still in servo mode after every session closed."; return 1 ;;
  esac
}

"$STATE" >/dev/null 2>&1 || { echo "::error::no controller answers at 127.0.0.1:30004."; exit 1; }
echo "SDK:  $(cat "$WS/sdk-version")"
sed 's/^/pin:  /' "$WS/commits"

# First the controller as it starts, before anybody readies the arm: what a session meets when
# the operator's step was skipped. The block checks what the bridge reports; ARCS says whether
# anything reached the arm.
echo ""
echo "== the arm as ARCS starts it"
"$STATE" | grep -q " mode=RobotModeType.PowerOff " || {
  echo "::error::a fresh ARCS should start with the arm powered off, and this one says:"; "$STATE"; exit 1; }
SUITE=0
make --no-print-directory check-arcs-unready VENDOR_PREFIX="$WS/install" || SUITE=$?
line=$("$STATE")
echo "   $line"
case "$line" in
  *" mode=RobotModeType.PowerOff "*" servo=0 "*)
    echo "   still powered off, servo mode off: the refused session reached nothing" ;;
  *) echo "::error::the unready session left ARCS changed."; SUITE=1 ;;
esac

echo ""
power_on
echo ""
echo "== the suite's ARCS block"
make --no-print-directory check-arcs-suite VENDOR_PREFIX="$WS/install" || SUITE=$?
after "the suite" || SUITE=$?

if [ -z "$SKYNET" ]; then
  echo "no skynet binary given, so the sim half of SCTS against ARCS was not run."
  exit "$SUITE"
fi
echo ""
power_on
echo ""
echo "== skynet driver check, against ARCS"
CHECK=0
make --no-print-directory check-arcs-conformance VENDOR_PREFIX="$WS/install" SKYNET="$SKYNET" || CHECK=$?
after "SCTS" || CHECK=$?
[ "$SUITE" -eq 0 ] || exit "$SUITE"
exit "$CHECK"

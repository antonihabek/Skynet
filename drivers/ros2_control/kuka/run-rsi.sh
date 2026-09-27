#!/bin/sh
# The KUKA row over RSI (DRY-COVERAGE-PLAN.md Phase 8, item 8.14), run inside `kuka/Dockerfile`'s
# image by `make check-rsi-docker`: KUKA's own RSI simulator up, the suite's RSI block, then the
# sim half of SCTS against a fresh simulator, and after each what the simulator's own log says.
#
# A script rather than a recipe for the reason every run-check.sh in drivers/ gives: it starts a
# process, waits for it, runs suites against it, stops it and reads what it counted.
#
# The simulator runs in a loop. An RSI-only session ends when the client sends RSI's stop flag,
# and the RSI program on the controller ends with it; KUKA's own wiki leaves the next session to an
# operator, who starts the program again. The loop is that operator, so the second session SCTS
# opens has a controller to hear it — and every pass of the loop is a session the simulator ended,
# which is what is counted below.
#
#   sh kuka/run-rsi.sh <vendor-prefix> [skynet-binary]
set -eu

PREFIX=$1
SKYNET=${2:-}
OUT=${TMPDIR:-/tmp}/skynet-rsi

# `ros2 run` needs the vendor's workspace on the path; the suite itself is run by make with its own
# environment and needs nothing from here. ROS's setup scripts read variables they never set, so
# they are sourced with `-u` off.
set +u
. /opt/ros/jazzy/setup.sh
. "$PREFIX/setup.sh"
set -u

start_controller() {
  rm -f "$OUT.$1.log"
  (
    while :; do
      ros2 run kuka_rsi_simulator rsi_simulator >> "$OUT.$1.log" 2>&1 || true
      echo "== the simulator exited" >> "$OUT.$1.log"
      sleep 0.2
    done
  ) &
  LOOP=$!
  trap 'stop_loop' EXIT INT TERM
  waited=0
  while ! grep -q "Successfully created socket" "$OUT.$1.log" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 150 ]; then
      echo "::error::KUKA's RSI simulator never started. It said:"; cat "$OUT.$1.log"; exit 1
    fi
    sleep 0.1
  done
}

stop_loop() {
  kill -TERM "$LOOP" 2>/dev/null || true
  wait "$LOOP" 2>/dev/null || true
  pkill -TERM -f 'kuka_rsi_simulator/rsi_simulator' 2>/dev/null || true
  trap - EXIT INT TERM
}

# What the controller saw, from its own log. `Socket closed.` is what the simulator prints when a
# reply carries RSI's stop flag and it ends the session; `Timeout count of 100 exceeded` is it
# giving up on a client that stopped answering. A session that ended the second way is one whose
# controller was never told to stop.
stop_controller() {
  phase=$1
  sessions=$2
  stop_loop
  stops=$(grep -c "Socket closed." "$OUT.$phase.log" || true)
  gave_up=$(grep -c "Timeout count of 100 exceeded" "$OUT.$phase.log" || true)
  late=$(grep -c "Packet is late" "$OUT.$phase.log" || true)
  silent=$(grep -c "Socket timed out" "$OUT.$phase.log" || true)
  echo "== what the controller saw ($phase)"
  echo "   stop flags received: $stops, of $sessions session(s) closed"
  echo "   sessions it abandoned after 100 missed replies: $gave_up"
  echo "   late replies: $late; telegrams with no reply inside the 40 ms cycle: $silent"
  status=0
  if [ "$stops" -ne "$sessions" ]; then
    echo "::error::$phase: the controller received $stops stop flag(s) for $sessions closed session(s)."
    echo "  An RSI session the client leaves without the stop flag is one the controller ends by"
    echo "  timing out — on a KRC, a halted RSI program and an error the operator acknowledges."
    status=1
  fi
  if [ "$gave_up" -ne 0 ]; then
    echo "::error::$phase: the controller abandoned $gave_up session(s) after 100 missed replies."
    status=1
  fi
  return $status
}

echo "== the suite's RSI block"
start_controller suite
SUITE=0
make --no-print-directory check-rsi-suite VENDOR_PREFIX="$PREFIX" || SUITE=$?
# The block closes its one session and nothing reopens it; give the reply that carries the stop
# flag a cycle to land before the loop is stopped.
sleep 1
stop_controller suite 1 || SUITE=$?

if [ -z "$SKYNET" ]; then
  echo "no skynet binary given, so the sim half of SCTS over RSI was not run."
  exit "$SUITE"
fi
echo ""
echo "== skynet driver check, over RSI"
start_controller scts
CHECK=0
make --no-print-directory check-rsi-conformance VENDOR_PREFIX="$PREFIX" SKYNET="$SKYNET" || CHECK=$?
sleep 1
# SCTS opens two sessions and closes both (the second after the first was stopped and closed).
stop_controller scts 2 || CHECK=$?
[ "$SUITE" -eq 0 ] || exit "$SUITE"
exit "$CHECK"

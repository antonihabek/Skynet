#!/bin/sh
# The FAIRINO row's whole path, in the order it has to happen in.
#
# A script rather than a `make` recipe because it starts a process, runs suites against it, stops
# it and then reads what it counted — and `make` gives each recipe line a shell of its own.
#
# Four claims, each measured by something different:
#
#   1. the sizes and the constants  `layout_check`, against the vendor's struct and source
#   2. the driver                   `selftest`, which runs the controller in-process
#   3. the wire                     `cross_check`, the vendor's own FRRobot against the fake
#   4. the tier                     `skynet driver check` through the real ABI, against the fake
#
# and then the fake's own report both times, because a conformance run that passed while
# commanding a controller that was refusing everything would otherwise be a green tick.
set -eu

SDK=${1:-}
SUITE_XMLRPC=${2:-31003}
SUITE_CMD=${3:-31080}
SUITE_STATE=${4:-31005}
CHECK_XMLRPC=${5:-32003}
CHECK_CMD=${6:-32080}
CHECK_STATE=${7:-32005}
REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-fairino-check

# The vendor's library has these three compiled in, so the cross-check runs on them and everything
# else runs beside them.
VENDOR_XMLRPC=20003
VENDOR_CMD=8080
VENDOR_STATE=20005

wait_for_controller() {
  waited=0
  while ! grep -q "listening for commands" "$1" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 100 ]; then
      echo "::error::the fake controller never announced itself. It said:"
      cat "$1"
      exit 1
    fi
    sleep 0.1
  done
}

echo "== this tree's sizes against the vendor's struct, and its ports against the vendor's source"
./layout_check "$SDK"

echo ""
echo "== the driver's own suite, which runs the controller in-process"
SUITE=0
./selftest "$SUITE_XMLRPC" "$SUITE_CMD" "$SUITE_STATE" || SUITE=$?

echo ""
echo "== the wire cross-check: the vendor's client, unmodified, against the fake controller"
rm -f "$OUT.cross.txt"
./fake_controller "$VENDOR_XMLRPC" "$VENDOR_CMD" "$VENDOR_STATE" \
  > "$OUT.cross.txt" 2>"$OUT.cross.err" &
FAKE=$!
trap 'kill -TERM $FAKE 2>/dev/null || true' EXIT INT TERM
wait_for_controller "$OUT.cross.err"
CROSS=0
./cross_check 127.0.0.1 || CROSS=$?
# Let the controller drain what is already on its sockets before stopping it. Without this the
# last request or two of a run are read after the TERM or not at all, and the counts below come
# out different from one run to the next — a substrate whose report is not reproducible is a
# substrate nobody can gate on (the Lite3 row learned this the same way).
sleep 1
kill -TERM $FAKE 2>/dev/null || true
wait $FAKE 2>/dev/null || true
trap - EXIT INT TERM

echo ""
echo "== what the controller counted under the vendor's own client"
cat "$OUT.cross.txt"
python3 - "$OUT.cross.txt" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    counted = json.loads(handle.read().strip().splitlines()[-1])

problems = []
# Under the vendor's own client every shape this tree transcribed should arrive as itself. A verb
# name, a parameter shape or a field name that were wrong here would land as one of these.
if counted.get("unknown_method", 0):
    problems.append(f"{counted['unknown_method']} XML-RPC call(s) from the vendor's own client "
                    "named a verb this tree does not implement")
if counted.get("malformed_request", 0):
    problems.append(f"{counted['malformed_request']} request(s) from the vendor's own client did "
                    "not parse — this tree's reading of XML-RPC and theirs disagree")
if counted.get("cnde_unknown_field", 0):
    problems.append(f"{counted['cnde_unknown_field']} field(s) in the vendor's default state "
                    "configuration are missing from this tree's table of their own field names")
if counted.get("cnde_malformed", 0):
    problems.append(f"{counted['cnde_malformed']} CNDE frame(s) from the vendor's client were "
                    "not the framing this tree expects")
for name, what in (("cnde_configs", "a state configuration"),
                   ("cnde_starts", "a stream start"),
                   ("servo_j", "a ServoJ"),
                   ("servo_jt", "a ServoJT"),
                   ("stop_frames", "a StopMotion frame on port 8080")):
    if not counted.get(name, 0):
        problems.append(f"the vendor's client sent no {what} — that path was not exercised, so "
                        "nothing about it was checked")
if counted.get("requested_fields", 0) < 70:
    problems.append("the vendor's client asked for fewer fields than its own default "
                    f"configuration has ({counted.get('requested_fields', 0)}), so most of the "
                    "field table was never exercised")

for problem in problems:
    print(f"::error::{problem}")
sys.exit(1 if problems else 0)
PY
CROSS_COUNT=$?

echo ""
echo "== the tier, through the real ABI"
rm -f "$OUT.check.txt"
./fake_controller "$CHECK_XMLRPC" "$CHECK_CMD" "$CHECK_STATE" \
  > "$OUT.check.txt" 2>"$OUT.check.err" &
FAKE=$!
trap 'kill -TERM $FAKE 2>/dev/null || true' EXIT INT TERM
wait_for_controller "$OUT.check.err"
CHECK=0
cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check ./libskynet_fairino.so \
  --config "{\"host\":\"127.0.0.1\",\"xmlrpc_port\":$CHECK_XMLRPC,\"command_port\":$CHECK_CMD,\"state_port\":$CHECK_STATE}" \
  || CHECK=$?
sleep 1  # as above: drain before stopping, or the report is not the same twice
kill -TERM $FAKE 2>/dev/null || true
wait $FAKE 2>/dev/null || true
trap - EXIT INT TERM

echo ""
echo "== what the controller counted under the conformance run"
cat "$OUT.check.txt"
python3 - "$OUT.check.txt" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    counted = json.loads(handle.read().strip().splitlines()[-1])

problems = []
if counted.get("xmlrpc_requests", 0) == 0:
    problems.append("the fake controller was never called — the driver never reached it, so "
                    "nothing below was measured")
if counted.get("unknown_method", 0):
    problems.append(f"{counted['unknown_method']} call(s) named a verb no FR controller has. "
                    "A stop sent as XML-RPC would land here, because StopMotion is not one")
if counted.get("malformed_request", 0):
    problems.append(f"{counted['malformed_request']} request(s) did not parse as XML-RPC")
if counted.get("commanded_without_window", 0):
    problems.append(
        f"{counted['commanded_without_window']} servo command(s) arrived outside a servo window. "
        "A real controller refuses those, so a driver that skipped ServoMoveStart presents as a "
        "robot that ignores commands")
if counted.get("commanded_while_disabled", 0):
    problems.append(f"{counted['commanded_while_disabled']} command(s) arrived at drives that "
                    "were not enabled")
if counted.get("beyond_travel", 0):
    problems.append(f"{counted['beyond_travel']} setpoint(s) were outside the soft limits this "
                    "controller reported, which the driver's own descriptor declares")
if counted.get("cnde_unknown_field", 0) or counted.get("cnde_malformed", 0):
    problems.append("the driver's state configuration or framing was refused by the controller")
if counted.get("stop_frames", 0) == 0:
    problems.append("no stop frame arrived on port 8080 — the conformance run's emergency stop "
                    "went somewhere else, or nowhere")

print(f"   state frames streamed: {counted.get('cnde_frames_sent', 0)} at "
      f"{counted.get('requested_period_ms', 0)} ms, {counted.get('requested_fields', 0)} fields")
print(f"   plant clamps: velocity {counted.get('clamped_velocity', 0)}, "
      f"position {counted.get('clamped_position', 0)}")

for problem in problems:
    print(f"::error::{problem}")
sys.exit(1 if problems else 0)
PY
COUNT=$?

if [ "$SUITE" -ne 0 ]; then
  echo "::error::the driver's own suite failed ($SUITE). The lines above say which checks."
  exit "$SUITE"
fi
if [ "$CROSS" -ne 0 ]; then
  echo "::error::the wire cross-check failed ($CROSS): the vendor's own client and this tree"
  echo "disagree about the wire."
  exit "$CROSS"
fi
if [ "$CROSS_COUNT" -ne 0 ]; then
  echo "::error::the controller counted a defect under the vendor's client (see above)."
  exit "$CROSS_COUNT"
fi
if [ "$COUNT" -ne 0 ]; then
  echo "::error::the controller counted a defect under the conformance run (see above)."
  exit "$COUNT"
fi
exit "$CHECK"

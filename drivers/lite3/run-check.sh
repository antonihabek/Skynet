#!/bin/sh
# The Lite3 row's whole path, in the order it has to happen in.
#
# A script rather than a `make` recipe because it starts a process, runs suites against it, stops
# it and then reads what it counted — and `make` gives each recipe line a shell of its own.
#
# Four claims, each measured by something different:
#
#   1. the structs        `layout_check`, compiled against the vendor's own headers
#   2. the driver         `selftest`, which is its own motion host and reads the bytes back
#   3. the wire           `cross_check`, the vendor's own Sender and Receiver against the fake
#   4. the tier           `skynet driver check` through the real ABI, against the fake
#
# and then the fake's own report, because a conformance run that passed while commanding a robot
# that was not listening would otherwise be a green tick.
set -eu

SUITE_COMMAND_PORT=${1:-44893}
SUITE_STATE_PORT=${2:-44897}
HZ=${3:-500}
REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-lite3-check

# The vendor's `Receiver` has its port fixed inside the prebuilt library, so the cross-check runs
# on MotionSDK's real ports and everything else runs beside them.
VENDOR_COMMAND_PORT=43893
VENDOR_STATE_PORT=43897

echo "== this tree's wire against MotionSDK's own headers"
./layout_check

echo ""
echo "== the driver's own suite, which is its own motion host"
SUITE=0
./selftest "$SUITE_COMMAND_PORT" "$SUITE_STATE_PORT" || SUITE=$?

echo ""
echo "== the wire cross-check: the vendor's client, unmodified, against the fake motion host"
rm -f "$OUT.cross.txt"
./fake_motion_host "$VENDOR_COMMAND_PORT" 127.0.0.1 "$VENDOR_STATE_PORT" "$HZ" 0 \
  > "$OUT.cross.txt" 2>"$OUT.cross.err" &
FAKE=$!
trap 'kill -TERM $FAKE 2>/dev/null || true' EXIT INT TERM
waited=0
while ! grep -q "listening for commands" "$OUT.cross.err" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 100 ]; then
    echo "::error::the fake motion host never announced itself. It said:"
    cat "$OUT.cross.err"
    exit 1
  fi
  sleep 0.1
done
CROSS=0
./cross_check 127.0.0.1 "$VENDOR_COMMAND_PORT" || CROSS=$?
# Let the fake drain what is already on its socket before stopping it. Without this the last
# datagram or two of a run are read after the TERM or not at all, and the counts below come out
# different from one run to the next — a substrate whose report is not reproducible is a substrate
# nobody can gate on.
sleep 1
kill -TERM $FAKE 2>/dev/null || true
wait $FAKE 2>/dev/null || true
trap - EXIT INT TERM

echo ""
echo "== what the fake counted under the vendor's own client"
cat "$OUT.cross.txt"
python3 - "$OUT.cross.txt" vendor <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    counted = json.loads(handle.read().strip().splitlines()[-1])

problems = []
# Under the vendor's client every shape this tree transcribed should arrive as itself. A code read
# out of a disassembly that were wrong would land here as `unknown_code`, which is the whole point
# of running their Sender against this fake.
if counted.get("unknown_code", 0):
    problems.append(f"{counted['unknown_code']} datagram(s) from the vendor's own Sender carried "
                    "a code this tree does not know. `lite3_wire.hpp` has one wrong")
if counted.get("malformed", 0):
    problems.append(f"{counted['malformed']} datagram(s) from the vendor's own Sender did not "
                    "match the header shape this tree expects")
for name, what in (("control_taken", "0x0114, control right taken"),
                   ("control_returned", "0x0113, control right returned"),
                   ("joints_to_zero", "0x31010c05, all joints to zero"),
                   ("joint_commands", "0x0111, a twelve-joint command")):
    if not counted.get(name, 0):
        problems.append(f"the vendor's client sent no {what} — that path was not exercised, so "
                        "nothing about it was checked")

for problem in problems:
    print(f"::error::{problem}")
sys.exit(1 if problems else 0)
PY
CROSS_COUNT=$?

echo ""
echo "== the tier, through the real ABI"
rm -f "$OUT.check.txt"
./fake_motion_host "$SUITE_COMMAND_PORT" 127.0.0.1 "$SUITE_STATE_PORT" "$HZ" 0 \
  > "$OUT.check.txt" 2>"$OUT.check.err" &
FAKE=$!
trap 'kill -TERM $FAKE 2>/dev/null || true' EXIT INT TERM
waited=0
while ! grep -q "listening for commands" "$OUT.check.err" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 100 ]; then
    echo "::error::the fake motion host never announced itself. It said:"
    cat "$OUT.check.err"
    exit 1
  fi
  sleep 0.1
done
CHECK=0
# Gains in the config, because this driver refuses a position command without them (§63.4 through
# a gain): a Lite3 joint takes an impedance command and a stiffness is the operator's to choose.
# `handback_ms` at zero so two sessions do not spend four seconds damping a fake that has no
# gravity to settle under; the real path is the default and `selftest` exercises it.
cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check ./libskynet_lite3.so \
  --config "{\"host\":\"127.0.0.1\",\"command_port\":$SUITE_COMMAND_PORT,\"state_port\":$SUITE_STATE_PORT,\"kp\":60.0,\"kd\":5.0,\"handback_ms\":0}" || CHECK=$?
sleep 1  # as above: drain before stopping, or the report is not the same twice
kill -TERM $FAKE 2>/dev/null || true
wait $FAKE 2>/dev/null || true
trap - EXIT INT TERM

echo ""
echo "== what the fake counted under the conformance run"
cat "$OUT.check.txt"
python3 - "$OUT.check.txt" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    counted = json.loads(handle.read().strip().splitlines()[-1])

problems = []
if counted.get("datagrams", 0) == 0:
    problems.append("the fake motion host received nothing at all — the driver never reached it, "
                    "so nothing below was measured")
if counted.get("malformed", 0):
    problems.append(f"{counted['malformed']} datagram(s) arrived malformed — a short header, the "
                    "wrong type octet, or a payload that is not 240 bytes")
if counted.get("unknown_code", 0):
    problems.append(f"{counted['unknown_code']} datagram(s) carried a code no vendor method sends")
if counted.get("commanded_without_control", 0):
    problems.append(
        f"{counted['commanded_without_control']} joint command(s) arrived while the robot's own "
        "controller held the legs. A real Lite3 ignores those, so a driver that took control "
        "right once and assumed it for ever presents as a robot that ignores commands")
if counted.get("beyond_travel", 0):
    problems.append(f"{counted['beyond_travel']} setpoint(s) were outside the travel Deep "
                    "Robotics publishes, which the driver's own descriptor declares")

# §24's hold is a stiffness (DRY-COVERAGE-PLAN.md F121): a masked joint carries its setpoint with
# the operator's gains, so a joint commanded with every field zero is a slack leg nobody asked for.
# It was 24 per run before the driver carried F121, when this line was a note rather than a check.
if counted.get("masked_free", 0):
    problems.append(f"{counted['masked_free']} joint command(s) had every gain and the feed-forward "
                    "at zero — a slack leg. A masked joint is held at its setpoint (§24), so the "
                    "hold has regressed or something commanded zero torque unasked")

# Not defects, and worth reading: whether the fake's plant ever hit a rating.
print(f"   plant clamps: torque {counted.get('clamped_torque', 0)}, "
      f"velocity {counted.get('clamped_velocity', 0)}, position {counted.get('clamped_position', 0)}")

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
  echo "::error::the wire cross-check failed ($CROSS): the vendor's own client and this tree "
  echo "disagree about the wire."
  exit "$CROSS"
fi
if [ "$CROSS_COUNT" -ne 0 ]; then
  echo "::error::the fake counted a defect under the vendor's client (see above)."
  exit "$CROSS_COUNT"
fi
if [ "$COUNT" -ne 0 ]; then
  echo "::error::the fake counted a defect under the conformance run (see above)."
  exit "$COUNT"
fi
exit "$CHECK"

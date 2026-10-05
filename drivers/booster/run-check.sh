#!/bin/sh
# The Booster row's whole path, in the order it has to happen in.
#
# A script rather than a `make` recipe because it starts a process, waits for DDS discovery, runs a
# conformance suite against it, stops it and then reads what it counted — and `make` gives each
# recipe line a shell of its own.
set -eu

IFACE=${1:-lo}
HZ=${2:-500}
# The joint and the vendor number the faulty fake reports (F205): the third joint, and a number that
# does not fit sixteen bits, so the word's packing is held rather than its presence.
FAULT_JOINT=2
FAULT_VALUE=70000
REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-booster-check

# `SKYNET` names a built `skynet` binary to use instead of building one with cargo: the seeding campaign
# (`tools/seed-defects`) runs this script in a copy of the driver with no Cargo workspace beside it.
skynet_cli() {
  if [ -n "${SKYNET:-}" ]; then "$SKYNET" "$@"; else cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- "$@"; fi
}

rm -f "$OUT.fake.txt"
./fake_robot "$IFACE" "$HZ" 60 > "$OUT.fake.txt" 2>"$OUT.fake.err" &
FAKE=$!
trap 'kill $FAKE 2>/dev/null || true' EXIT INT TERM

# DDS discovery is not instant and is not a fixed cost. Waiting for the fake's own first line is
# the signal that its publisher exists, which is what the driver is about to look for.
waited=0
while ! grep -q "publishing rt/low_state" "$OUT.fake.err" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 100 ]; then
    echo "::error::the fake robot never announced itself. It said:"
    cat "$OUT.fake.err"
    exit 1
  fi
  sleep 0.1
done
echo "== the fake robot is publishing on $IFACE at $HZ Hz"

# The driver's own suite first: the mode mapping, the layout, the view and every refusal, read
# off the wire by a second subscriber rather than inferred from a return code.
echo ""
echo "== the driver's own suite"
# The exit code is kept rather than allowed to abort the script. Both halves of this run measure
# different things — the suite reads the message off the wire, the fake counts what it would have
# done with it — and a failure in the first is exactly when the second is worth reading.
SUITE=0
# `FAKE_PID`: the suite stops the robot's publishing (SIGSTOP) and starts it again (SIGCONT) to give the driver a
# robot that has gone quiet, which is how it holds the stamp and the sequence of a state to the state's own (F278).
FAKE_PID=$FAKE ./selftest "$IFACE" || SUITE=$?

# Gains in the config, because this driver refuses a position command without them (§63.4 through a
# gain): a Booster drive takes an impedance command and a stiffness is the operator's to choose.
skynet_cli \
  driver check ./libskynet_booster.so \
  --config "{\"interface\":\"$IFACE\",\"kp\":60.0,\"kd\":5.0}" || CHECK=$?

# CONT first: a stopped process holds the TERM pending, and `wait` would be for ever if the suite died while it stopped the robot.
kill -CONT $FAKE 2>/dev/null || true
kill -TERM $FAKE 2>/dev/null || true
wait $FAKE 2>/dev/null || true
trap - EXIT INT TERM

# A robot that reports trouble on one joint (F205), in the maker's own `lost` field. A second fake,
# started only after the first has stopped — two publishers on `rt/low_state` would give the driver
# whichever message came last — with a vendor number wider than sixteen bits so the word's packing
# is held and not only its presence.
echo ""
echo "== a joint the robot reports trouble on (joint $FAULT_JOINT = $FAULT_VALUE)"
rm -f "$OUT.faulty.err"
./fake_robot "$IFACE" "$HZ" 60 "$FAULT_JOINT=$FAULT_VALUE" > "$OUT.faulty.txt" 2>"$OUT.faulty.err" &
FAULTY=$!
trap 'kill $FAULTY 2>/dev/null || true' EXIT INT TERM
waited=0
while ! grep -q "publishing rt/low_state" "$OUT.faulty.err" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 100 ]; then
    echo "::error::the faulty fake robot never announced itself. It said:"
    cat "$OUT.faulty.err"
    exit 1
  fi
  sleep 0.1
done
TROUBLE=0
./selftest "$IFACE" "$FAULT_JOINT=$FAULT_VALUE" || TROUBLE=$?
kill -TERM $FAULTY 2>/dev/null || true
wait $FAULTY 2>/dev/null || true
trap - EXIT INT TERM

# A robot that publishes another body's layout on the same topic: from the first sample (a K1 or T2
# where a T1 was expected), and from a moment mid-session (one that starts publishing after the
# driver has opened). The driver has to refuse the first at open and notice the second on a read.
# A fake of its own each time, started after the last has stopped, for the reason above.
LAYOUT=0
for MODE in 0 2; do
  rm -f "$OUT.layout.err"
  ./fake_robot "$IFACE" "$HZ" 12 - "layout=$MODE" > "$OUT.layout.txt" 2>"$OUT.layout.err" &
  ODD=$!
  trap 'kill $ODD 2>/dev/null || true' EXIT INT TERM
  waited=0
  while ! grep -q "publishing rt/low_state" "$OUT.layout.err" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 100 ]; then
      echo "::error::the odd-layout fake robot never announced itself. It said:"
      cat "$OUT.layout.err"
      exit 1
    fi
    sleep 0.1
  done
  echo ""
  echo "== another body's layout, from $MODE s in"
  ./selftest "$IFACE" "layout=$MODE" || LAYOUT=$?
  kill -TERM $ODD 2>/dev/null || true
  wait $ODD 2>/dev/null || true
  trap - EXIT INT TERM
done

echo ""
echo "== what the fake robot counted"
cat "$OUT.fake.txt"

# The three refusals the substrate exists for. A conformance run that passed while sending the
# wrong layout would be a green tick on a driver the robot rejects.
python3 - "$OUT.fake.txt" <<'PY'
import json, sys

# The SDK's ChannelSubscriber announces itself on stdout (not stderr), so the report file
# carries vendor chatter above the JSON. The report is the last brace-opened line.
with open(sys.argv[1], encoding="utf-8") as handle:
    lines = [line for line in handle.read().splitlines() if line.startswith("{")]
    counted = json.loads(lines[-1] if lines else "{}")

problems = []
if counted.get("commands", 0) == 0:
    problems.append("the fake robot received no command at all — the driver's publisher never "
                    "reached it, so nothing below was measured")
if counted.get("bad_size", 0):
    problems.append(f"{counted['bad_size']} command(s) carried the wrong joint count. The SDK "
                    "header warns such a command 'can address the wrong actuator or cause the "
                    "robot to reject the message'")
if counted.get("wrong_view", 0):
    problems.append(f"{counted['wrong_view']} command(s) were PARALLEL. The driver promised "
                    "SERIAL everywhere — joints as joints, the view it reads")
if counted.get("blended", 0):
    problems.append(f"{counted['blended']} joint-command(s) had weight < 1, handing part of the "
                    "joint to the internal controller. The driver's contract is explicit "
                    "full-user on every joint of every command")
if counted.get("idle_mode", 0):
    problems.append(f"{counted['idle_mode']} command(s) carried a motor mode other than the PMSM "
                    "servo mode. Mode 0 is idle: a drive told to be idle is unpowered, so a command "
                    "that means to drive the joint leaves it limp")

for problem in problems:
    print(f"::error::{problem}")
sys.exit(1 if problems else 0)
PY

if [ "$SUITE" -ne 0 ]; then
  echo "::error::the driver's own suite failed ($SUITE). The lines above say which checks."
  exit "$SUITE"
fi
if [ "$TROUBLE" -ne 0 ]; then
  echo "::error::the driver's reading of a joint the robot reports trouble on failed ($TROUBLE): F205."
  exit "$TROUBLE"
fi
if [ "$LAYOUT" -ne 0 ]; then
  echo "::error::the driver's handling of a state with another body's layout failed ($LAYOUT)."
  exit "$LAYOUT"
fi
exit ${CHECK:-0}

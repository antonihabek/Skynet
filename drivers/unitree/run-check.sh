#!/bin/sh
# The Unitree row's whole path, in the order it has to happen in, once per body.
#
# A script rather than a `make` recipe because it starts a process, waits for DDS discovery, runs a
# conformance suite against it, stops it and then reads what it counted — and `make` gives each
# recipe line a shell of its own.
#
# Seven bodies in sequence, not in parallel: `go2` and `hg` share topic names with different types,
# so DDS would keep them apart, but a parallel run would still interleave seven fakes' reports and
# seven suites' outputs into one unreadable log. Sequence is slower and legible.
set -eu

IFACE=${1:-lo}
HZ=${2:-500}
# The joint and the vendor number the faulty fake reports (F205): the third joint, and a number that
# does not fit sixteen bits, so the word's packing is held rather than its presence.
FAULT_JOINT=2
FAULT_VALUE=70000
REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-unitree-check

# `SKYNET` names a built `skynet` binary to use instead of building one with cargo: the seeding campaign
# (`tools/seed-defects`) runs this script in a copy of the driver with no Cargo workspace beside it.
skynet_cli() {
  if [ -n "${SKYNET:-}" ]; then "$SKYNET" "$@"; else cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- "$@"; fi
}

run_model() {
  MODEL=$1
  echo ""
  echo "###### model $MODEL ######"
  rm -f "$OUT.$MODEL.fake.txt"
  ./fake_robot "$IFACE" "$HZ" 120 "$MODEL" > "$OUT.$MODEL.fake.txt" 2>"$OUT.$MODEL.fake.err" &
  FAKE=$!
  trap 'kill $FAKE 2>/dev/null || true' EXIT INT TERM

  # DDS discovery is not instant and is not a fixed cost. Waiting for the fake's own first line is
  # the signal that its publisher exists, which is what the driver is about to look for.
  waited=0
  while ! grep -q "publishing rt/lowstate" "$OUT.$MODEL.fake.err" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 100 ]; then
      echo "::error::the fake $MODEL never announced itself. It said:"
      cat "$OUT.$MODEL.fake.err"
      exit 1
    fi
    sleep 0.1
  done
  echo "== the fake robot is publishing on $IFACE at $HZ Hz"

  # The driver's own suite first: the mode mapping, the sentinels, the CRC and every refusal, read
  # off the wire by a second subscriber rather than inferred from a return code.
  echo ""
  echo "== the driver's own suite ($MODEL)"
  # The exit code is kept rather than allowed to abort the script. Both halves of this run measure
  # different things — the suite reads the message off the wire, the fake counts what it would have
  # done with it — and a failure in the first is exactly when the second is worth reading.
  SUITE=0
  ./selftest "$IFACE" "$MODEL" || SUITE=$?

  # Gains in the config, because this driver refuses a position command without them (§63.4 through
  # a gain): a Unitree drive takes an impedance command and a stiffness is the operator's to choose.
  CHECK=0
  skynet_cli \
    driver check ./libskynet_unitree.so \
    --config "{\"interface\":\"$IFACE\",\"model\":\"$MODEL\",\"kp\":60.0,\"kd\":5.0}" || CHECK=$?

  kill -TERM $FAKE 2>/dev/null || true
  wait $FAKE 2>/dev/null || true
  trap - EXIT INT TERM

  # A robot that reports trouble on one joint (F205), in the maker's own field: `lost` on the go2
  # message, `motorstate` on the hg one. A second fake, started only after the first has stopped —
  # two publishers on `rt/lowstate` would give the driver whichever message came last — with a vendor
  # number wider than sixteen bits so the word's packing is held and not only its presence.
  echo ""
  echo "== a joint the robot reports trouble on ($MODEL, joint $FAULT_JOINT = $FAULT_VALUE)"
  rm -f "$OUT.$MODEL.faulty.err"
  ./fake_robot "$IFACE" "$HZ" 60 "$MODEL" "$FAULT_JOINT=$FAULT_VALUE" > "$OUT.$MODEL.faulty.txt" 2>"$OUT.$MODEL.faulty.err" &
  FAULTY=$!
  trap 'kill $FAULTY 2>/dev/null || true' EXIT INT TERM
  waited=0
  while ! grep -q "publishing rt/lowstate" "$OUT.$MODEL.faulty.err" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 100 ]; then
      echo "::error::the faulty fake $MODEL never announced itself. It said:"
      cat "$OUT.$MODEL.faulty.err"
      exit 1
    fi
    sleep 0.1
  done
  TROUBLE=0
  ./selftest "$IFACE" "$MODEL" "$FAULT_JOINT=$FAULT_VALUE" || TROUBLE=$?
  kill -TERM $FAULTY 2>/dev/null || true
  wait $FAULTY 2>/dev/null || true
  trap - EXIT INT TERM

  echo ""
  echo "== what the fake robot counted ($MODEL)"
  cat "$OUT.$MODEL.fake.txt"

  # The three refusals the substrate exists for. A conformance run that passed while writing zeros
  # where Unitree's sentinels belong would be a green tick on a driver that commands a joint to zero
  # radians every cycle.
  python3 - "$OUT.$MODEL.fake.txt" "$MODEL" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    counted = json.loads(handle.read().strip() or "{}")
model = sys.argv[2]

problems = []
if counted.get("commands", 0) == 0:
    problems.append("the fake robot received no command at all — the driver's publisher never "
                    "reached it, so nothing below was measured")
if counted.get("bad_crc", 0):
    problems.append(f"{counted['bad_crc']} command(s) had a CRC the robot would drop silently")
if counted.get("zero_where_sentinel", 0):
    problems.append(
        f"{counted['zero_where_sentinel']} command(s) wrote 0.0 where Unitree's PosStopF/VelStopF "
        "sentinel belongs. That is not 'say nothing about this field' — it is a command to go to "
        "zero, and with any gain at all it is a motion")
if counted.get("phantom_slot", 0):
    problems.append(
        f"{counted['phantom_slot']} command(s) drove a motor slot the {model} does not have")
if counted.get("idle_mode", 0):
    problems.append(
        f"{counted['idle_mode']} command(s) carried a motor mode other than the PMSM servo mode. Mode 0 is "
        "idle: a drive told to be idle is unpowered, so a command that means to drive the joint leaves it limp")

for problem in problems:
    print(f"::error::{problem}")
sys.exit(1 if problems else 0)
PY
  COUNT=$?

  if [ "$SUITE" -ne 0 ]; then
    echo "::error::the driver's own suite failed for $MODEL ($SUITE). The lines above say which checks."
    return "$SUITE"
  fi
  if [ "$TROUBLE" -ne 0 ]; then
    echo "::error::the driver's reading of a joint the robot reports trouble on failed for $MODEL ($TROUBLE): F205."
    return "$TROUBLE"
  fi
  if [ "$COUNT" -ne 0 ]; then
    echo "::error::the fake $MODEL counted a defect (see above)."
    return "$COUNT"
  fi
  return "$CHECK"
}

FAILED=0
for MODEL in go2 b2 h1 g1 h1_2 h2 r1; do
  run_model "$MODEL" || FAILED=$?
done

if [ "$FAILED" -ne 0 ]; then
  echo ""
  echo "::error::the Unitree matrix failed (first failure: $FAILED)."
  exit "$FAILED"
fi
echo ""
echo "all seven Unitree bodies passed: the suites, the fakes' counts and the conformance runs."

#!/bin/sh
# The Unitree row's whole path, in the order it has to happen in.
#
# A script rather than a `make` recipe because it starts a process, waits for DDS discovery, runs a
# conformance suite against it, stops it and then reads what it counted — and `make` gives each
# recipe line a shell of its own.
set -eu

IFACE=${1:-lo}
HZ=${2:-500}
REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-unitree-check

rm -f "$OUT.fake.txt"
./fake_robot "$IFACE" "$HZ" 60 > "$OUT.fake.txt" 2>"$OUT.fake.err" &
FAKE=$!
trap 'kill $FAKE 2>/dev/null || true' EXIT INT TERM

# DDS discovery is not instant and is not a fixed cost. Waiting for the fake's own first line is
# the signal that its publisher exists, which is what the driver is about to look for.
waited=0
while ! grep -q "publishing rt/lowstate" "$OUT.fake.err" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 100 ]; then
    echo "::error::the fake robot never announced itself. It said:"
    cat "$OUT.fake.err"
    exit 1
  fi
  sleep 0.1
done
echo "== the fake robot is publishing on $IFACE at $HZ Hz"

# The driver's own suite first: the mode mapping, the sentinels, the CRC and every refusal, read
# off the wire by a second subscriber rather than inferred from a return code.
echo ""
echo "== the driver's own suite"
# The exit code is kept rather than allowed to abort the script. Both halves of this run measure
# different things — the suite reads the message off the wire, the fake counts what it would have
# done with it — and a failure in the first is exactly when the second is worth reading.
SUITE=0
./selftest "$IFACE" || SUITE=$?

# Gains in the config, because this driver refuses a position command without them (§63.4 through a
# gain): a Unitree drive takes an impedance command and a stiffness is the operator's to choose.
cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check ./libskynet_unitree.so \
  --config "{\"interface\":\"$IFACE\",\"kp\":60.0,\"kd\":5.0}" || CHECK=$?

kill -TERM $FAKE 2>/dev/null || true
wait $FAKE 2>/dev/null || true

echo ""
echo "== what the fake robot counted"
cat "$OUT.fake.txt"

# The three refusals the substrate exists for. A conformance run that passed while writing zeros
# where Unitree's sentinels belong would be a green tick on a driver that commands a leg to zero
# radians every cycle.
python3 - "$OUT.fake.txt" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    counted = json.loads(handle.read().strip() or "{}")

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
        f"{counted['phantom_slot']} command(s) drove a motor slot a Go2 does not have (12..19)")

for problem in problems:
    print(f"::error::{problem}")
sys.exit(1 if problems else 0)
PY

if [ "$SUITE" -ne 0 ]; then
  echo "::error::the driver's own suite failed ($SUITE). The lines above say which checks."
  exit "$SUITE"
fi
exit ${CHECK:-0}

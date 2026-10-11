#!/bin/sh
# The Booster row's K1 path (DRY-COVERAGE-PLAN.md item 12.D, `booster`): a K1 fake on DDS, the K1 suite, the driver through the real ABI told `model: k1`, and the fake's own report; then the layouts in both directions.
#
#   sh run-check-k1.sh [interface] [hz]
set -eu

IFACE=${1:-lo}
HZ=${2:-500}
REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-booster-k1-check

skynet_cli() {
  if [ -n "${SKYNET:-}" ]; then "$SKYNET" "$@"; else cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- "$@"; fi
}

# A fake of its own each time, started after the last has stopped: two publishers on `rt/low_state` would give the driver whichever message came last.
start_fake() {
  name=$1
  shift
  rm -f "$OUT.$name.err"
  ./fake_robot "$IFACE" "$HZ" 60 "$@" > "$OUT.$name.txt" 2>"$OUT.$name.err" &
  FAKE=$!
  trap 'kill $FAKE 2>/dev/null || true' EXIT INT TERM
  waited=0
  while ! grep -q "publishing rt/low_state" "$OUT.$name.err" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 100 ]; then
      echo "::error::the $name fake robot never announced itself. It said:"
      cat "$OUT.$name.err"
      exit 1
    fi
    sleep 0.1
  done
}

stop_fake() {
  kill -TERM $FAKE 2>/dev/null || true
  wait $FAKE 2>/dev/null || true
  trap - EXIT INT TERM
}

echo "== a K1 fake is publishing 22 joints on $IFACE at $HZ Hz"
start_fake k1 model=k1
SUITE=0
./selftest_k1 "$IFACE" || SUITE=$?
CHECK=0
skynet_cli driver check ./libskynet_booster.so --config "{\"interface\":\"$IFACE\",\"model\":\"k1\",\"kp\":60.0,\"kd\":5.0}" || CHECK=$?
stop_fake

echo ""
echo "== a T1 fake, and a session that speaks the K1"
start_fake t1
AGAINST=0
./selftest_k1 "$IFACE" against=t1 || AGAINST=$?
stop_fake

echo ""
echo "== a K1 fake that starts publishing the T1's layout 2 s in"
start_fake odd model=k1 - layout=2
LAYOUT=0
./selftest_k1 "$IFACE" layout=2 || LAYOUT=$?
stop_fake

echo ""
echo "== what the K1 fake counted"
cat "$OUT.k1.txt"
python3 - "$OUT.k1.txt" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    lines = [line for line in handle.read().splitlines() if line.startswith("{")]
    counted = json.loads(lines[-1] if lines else "{}")
problems = []
if counted.get("commands", 0) == 0:
    problems.append("the fake robot received no command at all")
for key, what in (("bad_size", "the wrong joint count (the K1's is 22)"), ("wrong_view", "the PARALLEL view"), ("blended", "a weight below 1"), ("idle_mode", "a motor mode other than the servo mode")):
    if counted.get(key, 0):
        problems.append(f"{counted[key]} command(s) carried {what}")
for problem in problems:
    print(f"::error::{problem}")
sys.exit(1 if problems else 0)
PY

[ "$SUITE" -eq 0 ] || { echo "::error::the K1 suite failed ($SUITE)."; exit "$SUITE"; }
[ "$AGAINST" -eq 0 ] || { echo "::error::a K1 session against the T1's layout was not refused ($AGAINST)."; exit "$AGAINST"; }
[ "$LAYOUT" -eq 0 ] || { echo "::error::a K1 session that meets the T1's layout mid-session was not refused ($LAYOUT)."; exit "$LAYOUT"; }
exit "$CHECK"

#!/bin/sh
# The ABB row over EGM (DRY-COVERAGE-PLAN.md Phase 8, item 8.4), run inside `abb/Dockerfile`'s image
# by `make check-egm-docker`: the fake controller up, the suite's EGM block, then the sim half of
# SCTS against a fresh controller, and after each what the controller counted.
#
# A script rather than a recipe for the reason every run-check.sh in drivers/ gives: it starts a
# process, waits for it, runs suites against it, stops it and reads what it counted.
#
#   sh abb/run-egm.sh <vendor-prefix> [skynet-binary]
set -eu

PREFIX=$1
SKYNET=${2:-}
OUT=${TMPDIR:-/tmp}/skynet-egm
# The IRB 1200 5/90's axis ranges in degrees, from the `<limit>` of abb_irb1200_support's own
# description (urdf/abb_irb1200_egm.urdf) — the controller refuses a reference outside them.
LOWER=-170,-100,-200,-270,-130,-360
UPPER=170,130,70,270,130,360
START=0,10,20,0,30,0

start_controller() {
  python3 abb/fake_egm.py --port 6511 --hz 250 --start=$START --lower=$LOWER --upper=$UPPER \
    > "$OUT.$1.txt" 2> "$OUT.$1.err" &
  FAKE=$!
  trap 'kill -TERM $FAKE 2>/dev/null || true' EXIT INT TERM
  waited=0
  while ! grep -q "sending EgmRobot" "$OUT.$1.err" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 100 ]; then
      echo "::error::the fake EGM controller never started. It said:"; cat "$OUT.$1.err"; exit 1
    fi
    sleep 0.1
  done
}

stop_controller() {
  kill -TERM $FAKE 2>/dev/null || true
  wait $FAKE 2>/dev/null || true
  trap - EXIT INT TERM
  echo "== what the controller counted ($1)"
  cat "$OUT.$1.txt"
  python3 - "$OUT.$1.txt" "$1" <<'PY'
import json, sys
counted = json.loads(open(sys.argv[1], encoding="utf-8").read().strip().splitlines()[-1])
problems = [f"{counted[key]} {what}" for key, what in (
    ("malformed", "reply(ies) that did not parse as an EgmSensor"),
    ("wrong_type", "reply(ies) not marked MSGTYPE_CORRECTION"),
    ("wrong_count", "reference(s) without exactly six joints"),
    ("not_finite", "reference(s) with a NaN or an infinity"),
    ("beyond_range", "reference(s) outside the axis ranges"),
) if counted.get(key, 0)]
if not counted.get("corrections", 0):
    problems.append("no correction reached the controller at all, so nothing above was measured")
for problem in problems:
    print(f"::error::{sys.argv[2]}: {problem}")
sys.exit(1 if problems else 0)
PY
}

echo "== the suite's EGM block"
start_controller suite
SUITE=0
make --no-print-directory check-egm-suite VENDOR_PREFIX="$PREFIX" || SUITE=$?
stop_controller suite || SUITE=$?

if [ -z "$SKYNET" ]; then
  echo "no skynet binary given, so the sim half of SCTS over EGM was not run."
  exit "$SUITE"
fi
echo ""
echo "== skynet driver check, over EGM"
start_controller scts
CHECK=0
make --no-print-directory check-egm-conformance VENDOR_PREFIX="$PREFIX" SKYNET="$SKYNET" || CHECK=$?
stop_controller scts || CHECK=$?
[ "$SUITE" -eq 0 ] || exit "$SUITE"
exit "$CHECK"

#!/bin/sh
# The Franka row against franka-sim (DRY-COVERAGE-PLAN.md Phase 8, item 8.2): an FCI server this
# repository did not write, in three phases, each against a fresh server with its own log.
#
#   1. `franka_sim_check.py` — the Python driver, motion by motion, judged by where the arm is;
#   2. `skynet driver check` on the Python driver, through `drivers/python-host`;
#   3. `skynet driver check` on the Rust port, `drivers/franka-rs`.
#
# After phase 1 the server's own log is read for what it flagged: FCI motion-limit violations and
# lost command cycles. The server runs with enforcement off — the default, so a Python-paced loop's
# jitter does not abort a check that is about the protocol — and every violation it *would* have
# enforced is counted and printed, because on a Control box each one is a reflex.
#
# `FRANKA_SIM` names a virtualenv with `franka-sim` installed; the Makefile says how to make one.
set -eu

VENV=${FRANKA_SIM:-}
if [ -z "$VENV" ] || [ ! -x "$VENV/bin/run-franka-sim-server" ]; then
  echo "::error::FRANKA_SIM does not name a virtualenv with franka-sim installed:"
  echo ""
  echo "  python3 -m venv /tmp/franka-sim && /tmp/franka-sim/bin/pip install franka-sim==1.1.7"
  echo "  make check-franka-sim FRANKA_SIM=/tmp/franka-sim"
  echo ""
  echo "Refusing rather than reporting the row unmeasured as passed."
  exit 1
fi

PYTHON=${PYTHON:-python3}
REPO=$(cd ../.. && pwd)
HOST=127.0.0.1
PORT=1337
OUT=${TMPDIR:-/tmp}/skynet-franka-sim
echo "== franka-sim $("$VENV/bin/pip" show franka-sim 2>/dev/null | sed -n 's/^Version: //p')"

start_server() {
  LOG="$OUT.$1.log"
  "$VENV/bin/run-franka-sim-server" --no-gripper > "$LOG" 2>&1 &
  SERVER=$!
  trap 'kill $SERVER 2>/dev/null || true' EXIT INT TERM
  # The listening socket opens before the model is loaded — on a first run the server clones
  # MuJoCo Menagerie first, which takes most of a minute — so an open port is not a ready server.
  # Its own "simulation loop" line is.
  waited=0
  while ! grep -q "Starting MuJoCo simulation loop" "$LOG" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 1800 ]; then
      echo "::error::franka-sim never started its simulation loop. It said:"
      tail -20 "$LOG"
      exit 1
    fi
    sleep 0.1
  done
}

stop_server() {
  # The server's own Ctrl+C path can wedge in its viewer's exit hooks (its `SHUTDOWN_WATCHDOG_S`
  # says so), and there is no viewer here, so TERM and then KILL.
  kill -TERM $SERVER 2>/dev/null || true
  sleep 1
  kill -KILL $SERVER 2>/dev/null || true
  wait $SERVER 2>/dev/null || true
  trap - EXIT INT TERM
}

echo ""
echo "###### 1. the Python driver, motion by motion ######"
start_server check
CHECK=0
PYTHONPATH="$REPO/bindings/python:." "$PYTHON" franka_sim_check.py --host "$HOST" --port "$PORT" \
  || CHECK=$?
stop_server
VIOLATIONS=$(grep -c "motion limit violated" "$OUT.check.log" || true)
LOST=$(grep -c "consecutive lost command cycles" "$OUT.check.log" || true)
echo ""
echo "== what franka-sim flagged during phase 1: $VIOLATIONS motion-limit violation(s), $LOST run(s) of lost command cycles"
grep "motion limit violated" "$OUT.check.log" | sed 's/^.*violated: /     /' | head -10 || true

echo ""
echo "###### 2. the Python driver, the conformance suite ######"
start_server scts-python
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON="$PYTHON" >/dev/null
PY_SCTS=0
SKYNET_PYTHON="$PYTHON" SKYNET_PYTHON_DRIVER=skynet_franka \
  SKYNET_FRANKA_HOST="$HOST" SKYNET_FRANKA_PORT="$PORT" \
  PYTHONPATH="$REPO/bindings/python:." \
  cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
  --config "{\"host\":\"$HOST\",\"port\":$PORT,\"simulator\":true}" || PY_SCTS=$?
stop_server

echo ""
echo "###### 3. the Rust port, the conformance suite ######"
cargo build --manifest-path "$REPO/Cargo.toml" --release -q -p skynet-franka-rs
start_server scts-rust
RS_SCTS=0
cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check "$REPO/target/release/libskynet_franka_rs.so" \
  --config "{\"host\":\"$HOST\",\"port\":$PORT,\"simulator\":true}" || RS_SCTS=$?
stop_server

FAILED=0
[ "$CHECK" -ne 0 ] && { echo "::error::franka_sim_check.py failed ($CHECK)."; FAILED=$CHECK; }
[ "$PY_SCTS" -ne 0 ] && { echo "::error::the Python driver's conformance run failed ($PY_SCTS)."; [ "$FAILED" -ne 0 ] || FAILED=$PY_SCTS; }
[ "$RS_SCTS" -ne 0 ] && { echo "::error::the Rust port's conformance run failed ($RS_SCTS)."; [ "$FAILED" -ne 0 ] || FAILED=$RS_SCTS; }
if [ "$FAILED" -ne 0 ]; then
  exit "$FAILED"
fi
echo ""
echo "both Franka drivers passed against franka-sim: the motions, the hold, the stop and both conformance runs."

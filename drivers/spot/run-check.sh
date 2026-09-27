#!/bin/sh
# The Spot row's whole path, in the order it has to happen in.
#
# A script rather than a `make` recipe because it starts a process, waits for it to announce its
# port, runs a suite, a cross-check and a conformance run against it, stops it and then reads what
# it counted — and `make` gives each recipe line a shell of its own.
#
# Needs the project environment: the wire is gRPC over TLS and no stdlib speaks either.
set -eu

REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-spot-check
SPOT_PYTHON=${SPOT_PYTHON:-$REPO/.venv/bin/python}

if [ ! -x "$SPOT_PYTHON" ]; then
  echo "::error::no project environment at $SPOT_PYTHON. This row needs bosdyn-client and"
  echo "grpcio, which live in the repo-root .venv (untracked by design):"
  echo ""
  echo "  python3 -m venv .venv   # from the repo root (see drivers/spot/README.md)"
  echo "  .venv/bin/pip install 'bosdyn-client==5.2.0'"
  echo "  make check              # from drivers/spot"
  exit 1
fi
if ! "$SPOT_PYTHON" -c "import grpc, bosdyn.client" 2>/dev/null; then
  echo "::error::$SPOT_PYTHON has no grpcio/bosdyn-client. Install them into it:"
  echo ""
  echo "  $SPOT_PYTHON -m pip install 'bosdyn-client==5.2.0'"
  exit 1
fi

rm -f "$OUT.fake.txt"
PYTHONPATH="$REPO/bindings/python:$REPO/drivers/spot" "$SPOT_PYTHON" -m skynet_spot.fake_spot 0 \
  > "$OUT.fake.txt" 2>"$OUT.fake.err" &
FAKE=$!
trap 'kill -TERM $FAKE 2>/dev/null || true' EXIT INT TERM

# The fake announces its ephemeral port on stderr — the signal that it is serving, and the
# address the driver is about to be pointed at.
waited=0
while ! grep -q "listening on" "$OUT.fake.err" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 100 ]; then
    echo "::error::the fake Spot never announced itself. It said:"
    cat "$OUT.fake.err"
    exit 1
  fi
  sleep 0.1
done
PORT=$(sed -n 's/.*listening on 127.0.0.1:\([0-9]*\).*/\1/p' "$OUT.fake.err" | head -1)
echo "== the fake Spot is serving on 127.0.0.1:$PORT"

# The driver's own suite: direct against its own in-process fake, then through the pipe.
echo ""
echo "== the driver's own suite"
SUITE=0
PYTHONPATH="$REPO/bindings/python:$REPO/drivers/spot" "$SPOT_PYTHON" selftest.py || SUITE=$?

# The wire cross-check: the vendor's own client, unmodified, against the fake.
echo ""
echo "== the wire cross-check (the vendor's client)"
CROSS=0
PYTHONPATH="$REPO/bindings/python:$REPO/drivers/spot" "$SPOT_PYTHON" cross_check.py || CROSS=$?

# Through the real ABI: python-host spawns this driver, the driver points at the fake Spot.
echo ""
echo "== driver check through python-host"
CHECK=0
SKYNET_PYTHON_DRIVER=skynet_spot \
SKYNET_PYTHON="$SPOT_PYTHON" \
PYTHONPATH="$REPO/bindings/python:$REPO/drivers/spot" \
cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
  --config "{\"host\":\"127.0.0.1\",\"port\":$PORT,\"username\":\"admin\",\"password\":\"password\",\"ca\":\"$REPO/drivers/spot/certs/ca.pem\"}" || CHECK=$?

kill -TERM $FAKE 2>/dev/null || true
wait $FAKE 2>/dev/null || true
trap - EXIT INT TERM

echo ""
echo "== what the fake Spot counted"
cat "$OUT.fake.txt"

# The three refusals the substrate exists for. A conformance run that passed while the fake
# counted them would be a green tick on a driver the robot answers with rejection.
python3 - "$OUT.fake.txt" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    counted = json.loads(handle.read().strip().splitlines()[-1])

counts = counted.get("counts", {})
problems = []
if counts.get("requests", 0) == 0:
    problems.append("the fake Spot received no request at all — the driver never reached it, "
                    "so nothing below was measured")
if counts.get("bad_auth", 0):
    problems.append(f"{counts['bad_auth']} request(s) carried a wrong or missing token. "
                    "The robot answers those UNAUTHENTICATED, so a driver that misauths "
                    "moves nothing")
if counts.get("lease_refused", 0):
    problems.append(f"{counts['lease_refused']} request(s) carried a lease the robot never "
                    "issued. Motion without the live lease is refused")
if counts.get("challenge_refused", 0):
    problems.append(f"{counts['challenge_refused']} estop check-in(s) echoed the challenge "
                    "wrong. The handshake is the deadman's integrity")

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
  echo "::error::the wire cross-check failed ($CROSS)."
  exit "$CROSS"
fi
if [ "$COUNT" -ne 0 ]; then
  echo "::error::the fake Spot counted a defect (see above)."
  exit "$COUNT"
fi
exit "$CHECK"

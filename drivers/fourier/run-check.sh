#!/bin/sh
# The Fourier row's whole path, in the order it has to happen in.
#
# A script rather than a `make` recipe because it starts a process, waits for it to announce its
# port, runs two suites and a conformance run against it, stops it and then reads what it counted —
# and `make` gives each recipe line a shell of its own.
#
# Needs FOURIER_SDK: a checkout of the vendor's client repo (fftai/rocs_client_py), which the
# wire cross-check imports unmodified. It is not vendored here. The vendor client needs
# `websocket-client` importable too: an unpacked checkout beside the SDK, passed as WSCLIENT
# (or installed — the cross-check only needs the import to resolve).
set -eu

REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-fourier-check
# The vendor's client repo, for the wire cross-check. Defaults to checkouts beside the SDKs.
FOURIER_SDK=${FOURIER_SDK:-/tmp/rocs_client_py}
WSCLIENT=${WSCLIENT:-/tmp/websocket-client-1.9.0}

if [ ! -f "$FOURIER_SDK/rocs_client/__init__.py" ]; then
  echo "::error::FOURIER_SDK does not point at the vendor's client repo ($FOURIER_SDK). The wire"
  echo "claim needs their construction beside this tree's, so this gate refuses without it:"
  echo ""
  echo "  git clone --depth 1 https://github.com/fftai/rocs_client_py /tmp/rocs_client_py"
  echo "  make check FOURIER_SDK=/tmp/rocs_client_py"
  exit 1
fi

rm -f "$OUT.fake.txt"
PYTHONPATH="$REPO/bindings/python:." python3 -m skynet_fourier.fake_rocs 0 > "$OUT.fake.txt" 2>"$OUT.fake.err" &
FAKE=$!
trap 'kill -TERM $FAKE 2>/dev/null || true' EXIT INT TERM

# The fake announces its ephemeral port on stderr — the signal that it is serving, and the URL
# the driver is about to be pointed at.
waited=0
while ! grep -q "listening on" "$OUT.fake.err" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 100 ]; then
    echo "::error::the fake RoCS never announced itself. It said:"
    cat "$OUT.fake.err"
    exit 1
  fi
  sleep 0.1
done
PORT=$(sed -n 's/.*listening on 127.0.0.1:\([0-9]*\).*/\1/p' "$OUT.fake.err" | head -1)
echo "== the fake RoCS is serving on 127.0.0.1:$PORT"

# The driver's own suite: direct against its own in-process fake, then through the pipe.
echo ""
echo "== the driver's own suite"
SUITE=0
PYTHONPATH="$REPO/bindings/python:." python3 selftest.py || SUITE=$?

# The wire cross-check: the vendor's own client, unmodified, against this fake.
echo ""
echo "== the wire cross-check (the vendor's construction)"
CROSS=0
PYTHONPATH="$REPO/bindings/python:." python3 cross_check.py "$FOURIER_SDK" "$WSCLIENT" || CROSS=$?

# Through the real ABI: python-host spawns this driver, the driver points at the fake RoCS.
echo ""
echo "== driver check through python-host"
CHECK=0
SKYNET_PYTHON_DRIVER=skynet_fourier \
SKYNET_PYTHON=python3 \
PYTHONPATH="$REPO/bindings/python:$REPO/drivers/fourier" \
cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
  --config "{\"host\":\"127.0.0.1\",\"port\":$PORT}" || CHECK=$?

kill -TERM $FAKE 2>/dev/null || true
wait $FAKE 2>/dev/null || true
trap - EXIT INT TERM

echo ""
echo "== what the fake RoCS counted"
cat "$OUT.fake.txt"

# The refusals the substrate exists for. A conformance run that passed while commanding wrong
# would be a green tick on a driver the robot answers with motion it never ordered.
python3 - "$OUT.fake.txt" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    counted = json.loads(handle.read().strip().splitlines()[-1])

problems = []
if counted.get("requests", 0) == 0:
    problems.append("the fake RoCS received no request at all — the driver never reached it, "
                    "so nothing below was measured")
# No ws_commands floor: `driver check` commands all-masked holds — the safest command
# that is still well-formed — and a hold correctly sends nothing. The suite proves
# commands cross, with waits on the fake's own receipt.
if counted.get("unknown_endpoint", 0):
    problems.append(f"{counted['unknown_endpoint']} request(s) hit paths the API does not define")
if counted.get("malformed", 0):
    problems.append(f"{counted['malformed']} message(s) arrived malformed — bad JSON, a frame "
                    f"that was not text, or a move for no motor")
if counted.get("clamped", 0):
    problems.append(f"{counted['clamped']} move(s) exceeded the fake's ranges. The driver "
                    f"pre-clamps to the live limits, so a clamp here is a limit the driver "
                    f"misread")

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
  echo "::error::the fake RoCS counted a defect (see above)."
  exit "$COUNT"
fi
exit "$CHECK"

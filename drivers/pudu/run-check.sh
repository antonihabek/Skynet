#!/bin/sh
# The Pudu row's whole path, in the order it has to happen in.
#
# A script rather than a `make` recipe because it starts a process, waits for it to announce its
# port, runs two suites and a conformance run against it, stops it and then reads what it counted —
# and `make` gives each recipe line a shell of its own.
#
# Needs PUDU_SKILL: a checkout of the vendor's skill repo (pudu-robotics/skills), which the
# signing cross-check imports unmodified. It is not vendored here.
set -eu

REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-pudu-check
# The vendor's skill repo, for the signing cross-check. Default to a checkout beside the SDKs.
PUDU_SKILL=${PUDU_SKILL:-/tmp/pudu_skills}

if [ ! -f "$PUDU_SKILL/skills/pudu-openapi-skill/scripts/pudu-request.py" ]; then
  echo "::error::PUDU_SKILL does not point at the vendor's skill repo ($PUDU_SKILL). The signing"
  echo "claim needs their construction beside this tree's, so this gate refuses without it:"
  echo ""
  echo "  git clone --depth 1 https://github.com/pudu-robotics/skills /tmp/pudu_skills"
  echo "  make check PUDU_SKILL=/tmp/pudu_skills"
  exit 1
fi

rm -f "$OUT.fake.txt"
PYTHONPATH="$REPO/bindings/python:." python3 -m skynet_pudu.fake_cloud 0 > "$OUT.fake.txt" 2>"$OUT.fake.err" &
FAKE=$!
trap 'kill -TERM $FAKE 2>/dev/null || true' EXIT INT TERM

# The fake announces its ephemeral port on stderr — the signal that it is serving, and the URL
# the driver is about to be pointed at.
waited=0
while ! grep -q "listening on" "$OUT.fake.err" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 100 ]; then
    echo "::error::the fake cloud never announced itself. It said:"
    cat "$OUT.fake.err"
    exit 1
  fi
  sleep 0.1
done
PORT=$(sed -n 's/.*listening on 127.0.0.1:\([0-9]*\).*/\1/p' "$OUT.fake.err" | head -1)
echo "== the fake cloud is serving on 127.0.0.1:$PORT"

# The driver's own suite: direct against its own in-process fake, then through the pipe.
echo ""
echo "== the driver's own suite"
SUITE=0
PYTHONPATH="$REPO/bindings/python:." python3 selftest.py || SUITE=$?

# The signing cross-check: the vendor's own script, captured, against this fake's verifier.
echo ""
echo "== the signing cross-check (the vendor's construction)"
CROSS=0
PYTHONPATH="$REPO/bindings/python:." python3 cross_check.py "$PUDU_SKILL" || CROSS=$?

# Through the real ABI: python-host spawns this driver, the driver points at the fake cloud.
echo ""
echo "== driver check through python-host"
CHECK=0
SKYNET_PYTHON_DRIVER=skynet_pudu \
SKYNET_PYTHON=python3 \
PYTHONPATH="$REPO/bindings/python:$REPO/drivers/pudu" \
cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
  --config "{\"base_url\":\"http://127.0.0.1:$PORT\",\"sn\":\"SN-CHECK-1\",\"app_key\":\"test-key\",\"app_secret\":\"test-secret\"}" || CHECK=$?

kill -TERM $FAKE 2>/dev/null || true
wait $FAKE 2>/dev/null || true
trap - EXIT INT TERM

echo ""
echo "== what the fake cloud counted"
cat "$OUT.fake.txt"

# The three refusals the substrate exists for. A conformance run that passed while signing wrong
# would be a green tick on a driver the cloud answers with 401.
python3 - "$OUT.fake.txt" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    counted = json.loads(handle.read().strip().splitlines()[-1])

problems = []
if counted.get("requests", 0) == 0:
    problems.append("the fake cloud received no request at all — the driver never reached it, "
                    "so nothing below was measured")
if counted.get("bad_auth", 0):
    problems.append(f"{counted['bad_auth']} request(s) failed signature verification. The cloud "
                    "answers those with 401, so a driver that signs wrong moves nothing")
if counted.get("unknown_endpoint", 0):
    problems.append(f"{counted['unknown_endpoint']} request(s) hit paths the API does not define")
if counted.get("malformed", 0):
    problems.append(f"{counted['malformed']} request(s) arrived malformed — bad JSON, a missing "
                    "serial, or a motion without its documented fields")

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
  echo "::error::the signing cross-check failed ($CROSS)."
  exit "$CROSS"
fi
if [ "$COUNT" -ne 0 ]; then
  echo "::error::the fake cloud counted a defect (see above)."
  exit "$COUNT"
fi
exit "$CHECK"

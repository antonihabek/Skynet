#!/bin/sh
# The port's measurement, run on a kernel CAN bus — `make check` runs this under
# `drivers/canopen/vcan.sh`, which supplies `vcan0` (the machine's, or one in an unprivileged
# network namespace).
#
#   1. The crate's tests with the live half *required*: a run that cannot reach the bus fails.
#   2. The sim half of SCTS against the release .so, with the Python driver's simulated drives
#      serving on the same bus in a process of their own.
#
# `sh check.sh`, never `./check.sh`: a file's execute bit is not carried by every checkout.
set -eu

PYTHON=${PYTHON:-python3}
CARGO=${CARGO:-cargo}
REPO=../..

SKYNET_CANOPEN_REQUIRE_VCAN=1 "$CARGO" test --manifest-path "$REPO/Cargo.toml" -q -p skynet-canopen-rs

# The drives read their shutdown from stdin, so it is a FIFO this shell holds open on fd 3:
# closing it is the signal, and a shell that dies closes it too — no orphans left on the bus.
work=$(mktemp -d)
trap 'exec 3>&- 2>/dev/null; rm -rf "$work"' EXIT
mkfifo "$work/in"
PYTHONPATH=../canopen:$REPO/bindings/python "$PYTHON" -m skynet_canopen.drives vcan0 1 2 3 \
  < "$work/in" > "$work/out" &
drives=$!
exec 3> "$work/in"
for _ in $(seq 1 50); do
  grep -q '^ready$' "$work/out" 2>/dev/null && break
  sleep 0.1
done
if ! grep -q '^ready$' "$work/out"; then
  echo "::error::the simulated drives did not come up on vcan0"
  exit 1
fi

status=0
"$CARGO" run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check "$REPO/target/release/libskynet_canopen_rs.so" \
  --config '{"joints":[{"node":1,"name":"J1","role":"right_arm.shoulder.pitch"},{"node":2,"name":"J2","role":"right_arm.elbow.flex"},{"node":3,"name":"J3","role":"right_arm.wrist.roll"}],"interface":"vcan0"}' \
  || status=$?

exec 3>&-
wait "$drives" || true
exit "$status"

#!/bin/sh
# The port's measurement over a real interface: a veth pair in an unprivileged network namespace,
# the Python slave stack on one end (`python3 -m skynet_ethercat.wire`) and this master on the
# other — `drivers/ethercat/soem.sh`'s arrangement, with this port where SOEM is.
#
#   1. The crate's tests with the live half *required*.
#   2. The sim half of SCTS against the release .so, with the ring served for it.
#
# `sh check.sh`, never `./check.sh`: a file's execute bit is not carried by every checkout.
set -eu

if [ "${SKYNET_ETHERCAT_REEXEC:-}" != "1" ]; then
  if ! command -v unshare >/dev/null 2>&1 || ! unshare -rn /bin/true 2>/dev/null; then
    echo "::error::no unprivileged user and network namespace here, so there is nowhere to put a"
    echo "veth pair without root. Nothing was measured."
    exit 1
  fi
  SKYNET_ETHERCAT_REEXEC=1 exec unshare -rn /bin/sh "$0" "$@"
fi

PYTHON=${PYTHON:-python3}
CARGO=${CARGO:-cargo}
REPO=../..

ip link add ecat0 type veth peer name ecat1
ip link set ecat0 up
ip link set ecat1 up

SKYNET_ETHERCAT_REQUIRE=1 "$CARGO" test --manifest-path "$REPO/Cargo.toml" -q -p skynet-ethercat-rs

work=$(mktemp -d)
PYTHONPATH="../ethercat:../canopen:$REPO/bindings/python" \
  "$PYTHON" -m skynet_ethercat.wire --interface ecat1 --drives 3 > "$work/ring" 2>&1 &
ring=$!
trap 'kill "$ring" 2>/dev/null || true; rm -rf "$work"' EXIT
for _ in $(seq 1 50); do
  grep -q '^serving' "$work/ring" 2>/dev/null && break
  sleep 0.1
done
if ! grep -q '^serving' "$work/ring"; then
  echo "::error::the slave stack did not start serving on ecat1:"
  cat "$work/ring"
  exit 1
fi

"$CARGO" run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check "$REPO/target/release/libskynet_ethercat_rs.so" \
  --config '{"interface":"ecat0","joints":[{"name":"J1","role":"right_arm.shoulder.pitch"},{"name":"J2","role":"right_arm.elbow.flex"},{"name":"J3","role":"right_arm.wrist.roll"}]}'

#!/bin/sh
# Both CANopen drivers against Lely's CiA 402 slave (DRY-COVERAGE-PLAN.md Phase 10, item 10.7), run
# inside `lely/Dockerfile`'s image by `make check-lely-docker`:
#
#   1. the Python driver's own suite with its Lely block — the drives read back by a second master;
#   2. the sim half of SCTS against the Python driver, through `drivers/python-host`;
#   3. the sim half of SCTS against the Rust port's shared object;
#
# each against slaves started fresh for it, and after each what the slaves' own logs say they
# received. Fresh, because `ros2_canopen`'s slave starts a mode's thread once per mode and lets it
# die when the state machine leaves `operation enabled` — a second session on the same slave opens,
# is commanded and reads, and does not move. That is the mock's, it is why the suite's movement
# check is always the first session, and the script does not hide it by reusing a slave.
#
# A script rather than a recipe for the reason every run-*.sh in drivers/ gives: it starts
# processes, waits for them, runs suites against them, stops them and reads what they counted.
#
#   sh lely/run-lely.sh <skynet-binary> <rust-driver.so>
set -eu

SKYNET=$1
RUST_SO=$2
PREFIX=/opt/lely_ws/install
HERE=$(cd "$(dirname "$0")" && pwd)
CANOPEN=$(dirname "$HERE")
REPO=$(cd "$CANOPEN/../.." && pwd)
OUT=${TMPDIR:-/tmp}/skynet-lely
mkdir -p "$OUT"

# ROS's setup scripts read variables they never set, so they are sourced with `-u` off. The slave's
# helper library is installed beside it with no run path (`libmotion_generator.so`), so it is named
# here, as it must be for the apt package too.
set +u
. /opt/ros/jazzy/setup.sh
. "$PREFIX/setup.sh"
set -u
export LD_LIBRARY_PATH="$PREFIX/lib/canopen_fake_slaves:${LD_LIBRARY_PATH:-}"

cat "/opt/lely_ws/commits"

if ! ip link add dev vcan0 type vcan 2>/dev/null; then
  echo "::error::this kernel will not create a vcan interface. On the Docker host: modprobe vcan,"
  echo "and run the container with --cap-add NET_ADMIN. Nothing was measured."
  exit 1
fi
ip link set up vcan0

# The shipped description, and the two configurations `make_dcf.py` derives from it.
EDS="$PREFIX/share/canopen_fake_slaves/config/cia402_slave.eds"
python3 "$HERE/make_dcf.py" "$EDS" "$OUT/configured.dcf"
python3 "$HERE/make_dcf.py" --keep-mapping "$EDS" "$OUT/keep-mapping.dcf"

start_slave() { # phase node configuration
  ros2 launch canopen_fake_slaves cia402_slave.launch.py \
    node_id:="$2" node_name:="s$2" slave_config:="$3" can_interface_name:=vcan0 \
    > "$OUT.$1.$2.log" 2>&1 &
}

# Slaves 1 to 3 are the configured drive; the suite also wants 4 as the slave ships and 5 with its
# units and its own mapping.
start_slaves() { # phase [extra]
  rm -f "$OUT.$1".*.log
  for node in 1 2 3; do start_slave "$1" "$node" "$OUT/configured.dcf"; done
  if [ "${2:-}" = "extra" ]; then
    start_slave "$1" 4 "$EDS"
    start_slave "$1" 5 "$OUT/keep-mapping.dcf"
    nodes="1 2 3 4 5"
  else
    nodes="1 2 3"
  fi
  waited=0
  for node in $nodes; do
    until grep -q "Now sending Boot-Up frame" "$OUT.$1.$node.log" 2>/dev/null; do
      waited=$((waited + 1))
      if [ "$waited" -gt 300 ]; then
        echo "::error::Lely's slave for node $node never sent its boot-up frame. It said:"
        cat "$OUT.$1.$node.log"
        exit 1
      fi
      sleep 0.1
    done
  done
}

stop_slaves() {
  pkill -TERM -f 'ros2 launch' 2>/dev/null || true
  pkill -TERM -f cia402_slave_node 2>/dev/null || true
  sleep 0.5
  pkill -KILL -f 'ros2 launch' 2>/dev/null || true
  pkill -KILL -f cia402_slave_node 2>/dev/null || true
  sleep 0.2
}
trap stop_slaves EXIT INT TERM

# What the slaves saw, from their own logs: the state machine's own account of each drive. Counted
# per node, because a stop that reached two of three drives is the failure this is for.
count() { grep -c "$2" "$OUT.$1.$3.log" 2>/dev/null || true; }

# Exact, not at-least: a stop that reached a drive twice or a session closed without its disable is
# as wrong as one that never arrived, and a count that drifts is a driver that changed. The figures
# are each run's own — a session's close is one disable-voltage and a stop is one quick stop.
report() { # phase quick-stops disable-voltages
  phase=$1
  expected_stops=$2
  expected_disables=$3
  echo "== what the slaves saw ($phase)"
  status=0
  for node in 1 2 3; do
    stops=$(count "$phase" "Received Quick Stop" "$node")
    disables=$(count "$phase" "Received Disable Voltage" "$node")
    enabled=$(count "$phase" "Received Enable Operation" "$node")
    cyclic=$(count "$phase" "run_cyclic_position_mode" "$node")
    echo "   node $node: quick stops $stops, disable voltages $disables, enable operations $enabled," \
         "cyclic-position threads $cyclic"
    if [ "$stops" -ne "$expected_stops" ]; then
      echo "::error::$phase: node $node's state machine received $stops quick stop(s), expected"
      echo "  $expected_stops. A stop the drive never saw is a stop that stopped nothing."
      status=1
    fi
    if [ "$disables" -ne "$expected_disables" ]; then
      echo "::error::$phase: node $node received $disables disable-voltage(s), expected"
      echo "  $expected_disables — one for each session closed."
      status=1
    fi
  done
  # The launch prints two lifecycle errors for every slave — the node reaches `active` before the
  # launch's own transition asks — and neither is a fault, so only a slave that died is one.
  for node in 1 2 3; do
    if grep -q "process has died\|Segmentation\|terminate called\|Aborted" "$OUT.$phase.$node.log"; then
      echo "::error::$phase: node $node's slave died:"; cat "$OUT.$phase.$node.log"
      status=1
    fi
  done
  return $status
}

status=0
JOINTS='"joints":[{"node":1,"name":"J1","role":"right_arm.shoulder.pitch"},{"node":2,"name":"J2","role":"right_arm.elbow.flex"},{"node":3,"name":"J3","role":"right_arm.wrist.roll"}]'

echo "== the suite's Lely block, against slaves as configured (Python driver)"
start_slaves suite extra
(cd "$CANOPEN" && PYTHONPATH="$REPO/bindings/python:$CANOPEN" python3 selftest.py --lely) || status=1
report suite 1 1 || status=1
stop_slaves

echo "== the sim half of SCTS, Python driver"
start_slaves scts-python
make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null
SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_canopen \
  PYTHONPATH="$REPO/bindings/python:$CANOPEN" \
  "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
  --config "{$JOINTS,\"interface\":\"vcan0\"}" > "$OUT.scts-python.txt" 2>&1 || status=1
grep -E "passed|FAIL|guest|no tier" "$OUT.scts-python.txt" | sed 's/\x1b\[[0-9;]*m//g' | head -20
report scts-python 1 2 || status=1
stop_slaves

echo "== the sim half of SCTS, Rust port"
start_slaves scts-rust
"$SKYNET" driver check "$RUST_SO" \
  --config "{$JOINTS,\"interface\":\"vcan0\"}" > "$OUT.scts-rust.txt" 2>&1 || status=1
grep -E "passed|FAIL|guest|no tier" "$OUT.scts-rust.txt" | sed 's/\x1b\[[0-9;]*m//g' | head -20
report scts-rust 1 2 || status=1
stop_slaves

exit $status

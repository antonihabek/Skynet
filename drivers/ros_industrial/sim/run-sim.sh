#!/bin/sh
# The ROS-Industrial row against a server built from the maker's own library (DRY-COVERAGE-PLAN.md item 12.E.6), run by `make check-sim-docker` inside `sim/Dockerfile`'s image:
#
#   1. `layout_check.py`: this driver's constants and bytes against the maker's library, built from the pinned checkout in the image, in its four builds;
#   2. for each build of the maker's server (little- and big-endian, 4- and 8-byte reals): the server is started, `live.py` holds the driver to it, and the sim half of SCTS runs through the
#      real ABI (`skynet driver check` -> python-host -> this driver -> the server).
#
# A script rather than a recipe, for the reason every run-*.sh in drivers/ gives. `sh sim/run-sim.sh <skynet binary>`, never `./`.
set -eu

SKYNET=$1
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)

export PYTHONPATH="$REPO/bindings/python:$BASE:${PYTHONPATH:-}"
export PYTHONWARNINGS=ignore
status=0

echo "== $(cat /opt/industrial_core.txt)"
echo "== layout: this driver's bytes against the maker's, built here"
(cd "$BASE" && INDUSTRIAL_CORE=/opt/industrial_core python3 layout_check.py | tail -4) || status=1

make --no-print-directory -C "$REPO/drivers/python-host" libskynet_python_host.so PYTHON=python3 >/dev/null

JOINTS='[{"name":"joint_1","role":"right_arm.shoulder.yaw","min_rad":-2.9,"max_rad":2.9,"max_speed_rad_s":3.0},{"name":"joint_2","role":"right_arm.shoulder.pitch","min_rad":-1.9,"max_rad":1.9,"max_speed_rad_s":3.0},{"name":"joint_3","role":"right_arm.elbow.flex","min_rad":-2.5,"max_rad":2.5,"max_speed_rad_s":3.0},{"name":"joint_4","role":"right_arm.wrist.pitch","min_rad":-3.1,"max_rad":3.1,"max_speed_rad_s":3.0},{"name":"joint_5","role":"right_arm.wrist.yaw","min_rad":-2.0,"max_rad":2.0,"max_speed_rad_s":3.0},{"name":"joint_6","role":"right_arm.wrist.roll","min_rad":-6.2,"max_rad":6.2,"max_speed_rad_s":3.0}]'

base_port=11100
for variant in "little 4" "big 4" "little 8" "big 8"; do
  set -- $variant
  order=$1; real=$2
  binary=/opt/rosi-src/rosi_server-$order-$real
  motion=$base_port; state=$((base_port + 1)); control=$((base_port + 2))
  base_port=$((base_port + 10))
  echo ""
  echo "===== the maker's server, $order-endian, $real-byte reals"
  "$binary" --motion $motion --state $state --control $control --log /tmp/rosi-$order-$real.log >/dev/null 2>&1 &
  server=$!
  waited=0
  until (echo > /dev/tcp/127.0.0.1/$control) 2>/dev/null || python3 -c "import socket,sys; socket.create_connection(('127.0.0.1',$control),timeout=1).close()" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 30 ]; then echo "::error::the maker's server never listened."; kill $server 2>/dev/null || true; exit 1; fi
    sleep 0.2
  done
  # `live.py` starts a server of its own on free ports: this one is for SCTS, which is given addresses and joints in its configuration.
  (cd "$BASE" && python3 live.py "$binary" "$order" "$real" | grep -E "^\s+FAIL|checks,") || status=1
  CONFIG="{\"host\":\"127.0.0.1\",\"ports\":{\"motion\":$motion,\"state\":$state},\"simulator\":true,\"byte_order\":\"$order\",\"real_bytes\":$real,\"rate_hz\":10,\"point_duration_s\":0.2,\"joints\":$JOINTS}"
  SKYNET_PYTHON=python3 SKYNET_PYTHON_DRIVER=skynet_ros_industrial \
    "$SKYNET" driver check "$REPO/drivers/python-host/libskynet_python_host.so" --config "$CONFIG" > /tmp/skynet-rosi-scts-$order-$real.txt 2>&1 || status=1
  sed 's/\x1b\[[0-9;]*m//g' /tmp/skynet-rosi-scts-$order-$real.txt | grep -E "FAIL|passed|guest|no tier" | head -12
  kill $server 2>/dev/null || true
  wait $server 2>/dev/null || true
done

exit $status

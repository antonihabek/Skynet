#!/bin/sh
# Inside the client container (`nusim/Dockerfile`), on the simulator's host network: build the driver and the CUSTOM-mode helper against the Booster SDK, put the simulator in CUSTOM mode, and run one of the two
# things the live run does with it:
#
#   sh nusim/run-client.sh harness <sdk>            nusim_live.py: the K1's 22 joints, each moved alone, and the stop
#   sh nusim/run-client.sh abi <sdk> <skynet>       `skynet driver check`, the sim half of the conformance suite, through the real ABI against the simulator
#
# It runs from drivers/booster, and leaves the library and the helper beside the sources (build products the checkout ignores).
set -eu

WHAT=$1
SDK=$2
HERE=$(cd "$(dirname "$0")" && pwd)
BASE=$(dirname "$HERE")
REPO=$(cd "$BASE/../.." && pwd)
IFACE=${IFACE:-lo}

cd "$BASE"
make -s -B lib BOOSTER_SDK="$SDK" CXX=g++
g++ -std=c++17 -O2 -isystem "$SDK/include" -o /tmp/mode_custom "$HERE/mode_custom.cpp" "$SDK/lib/$(uname -m)/libbooster_robotics_sdk.a" -lpthread -ldl -lrt
echo "== the simulator is asked for CUSTOM mode"
/tmp/mode_custom "$IFACE"

case "$WHAT" in
  harness)
    NUSIM_LIB="$BASE/libskynet_booster.so" python3 "$HERE/nusim_live.py" "$IFACE"
    ;;
  abi)
    SKYNET=$3
    "$SKYNET" driver check "$BASE/libskynet_booster.so" --config "{\"interface\":\"$IFACE\",\"model\":\"k1\",\"kp\":150.0,\"kd\":3.0,\"simulator\":true}"
    ;;
  *)
    echo "run-client.sh: harness or abi" >&2
    exit 2
    ;;
esac

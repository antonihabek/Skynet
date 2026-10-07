#!/bin/sh
# Build the maker's own client as a process (sim/oracle.cpp) against OnRobot's own packages, unpacked and not installed: `sh sim/build.sh <SDK dir> <output dir>`.
#
# <SDK dir> is where `dpkg -x` put `libonrobot-tool-api1_*.deb` and `libonrobot-tool-api-dev_*.deb` (it holds `usr/include/onrobot_tool_api` and `usr/lib/libonrobot_tool_api.so.*`); `sim/fetch.sh` makes it, checking the
# packages' SHA-256 against the pins. libmodbus (Ubuntu's `libmodbus5` and `libmodbus-dev`) is what the maker's library links.
set -eu

SDK=$1
OUT=$2
HERE=$(cd "$(dirname "$0")" && pwd)
LIB=$(ls "$SDK"/usr/lib/libonrobot_tool_api.so.[0-9]*.[0-9]*.[0-9]* | head -1)
test -f "$LIB" || { echo "::error::no libonrobot_tool_api in $SDK/usr/lib. Nothing was built."; exit 1; }
mkdir -p "$OUT"
g++ -std=c++17 -O1 -Wall -I"$SDK/usr/include" "$HERE/oracle.cpp" "$LIB" -lmodbus -Wl,-rpath,"$(dirname "$LIB")" -o "$OUT/onrobot-oracle"
echo "built $OUT/onrobot-oracle against $(basename "$LIB")"

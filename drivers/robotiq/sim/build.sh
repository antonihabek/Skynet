#!/bin/sh
# Build Robotiq's own SDK (`github.com/Robotiq/grippers`, BSD-3-Clause) and the four harnesses in this directory against it, with nothing of the maker's changed (DRY-COVERAGE-PLAN.md item 12.E.8).
#
#   sh sim/build.sh <checkout of the maker's repository> <output directory>
#
# The maker's CMake project is built as a static library without its libserialport transport (the harnesses bring a TCP one: the transport extension point the maker documents), tests and examples. The harnesses are
# compiled against the maker's public headers and, for the fake gripper, the two internal ones the maker's own `makeFakeGripper()` is built from (`src/fake/`), which the library contains.
#
#   robotiq-fake-server       the maker's fake gripper, on a TCP port and a pseudo-terminal         (server.cpp)
#   robotiq-codec-dump        what the maker's client writes, reads and converts                     (codec_dump.cpp)
#   robotiq-second-opinion    the maker's client reading a gripper's status                          (second_opinion.cpp)
#   robotiq-client-probe      the maker's whole client driving a gripper                             (client_probe.cpp)
set -eu

SDK=$(cd "$1" && pwd)
OUT=$2
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)

cmake -S "$SDK/sdk_cpp" -B "$OUT/sdk" -DCMAKE_BUILD_TYPE=Release -DGRIPPERS_BUILD_DEFAULT_SERIAL=OFF -DGRIPPERS_BUILD_TESTS=OFF -DGRIPPERS_BUILD_EXAMPLES=OFF >"$OUT/cmake.log" 2>&1 \
  || { echo "::error::cmake could not configure the maker's SDK:"; tail -20 "$OUT/cmake.log"; exit 1; }
cmake --build "$OUT/sdk" -j "$(nproc 2>/dev/null || echo 2)" >>"$OUT/cmake.log" 2>&1 \
  || { echo "::error::the maker's SDK did not build:"; tail -30 "$OUT/cmake.log"; exit 1; }

CXXFLAGS="-std=c++17 -O1 -Wall -Wextra -DGRIPPERS_HOSTED=1 -DGRIPPERS_BUILD_DEFAULT_SERIAL=0 -I$SDK/sdk_cpp/include -I$SDK/sdk_cpp/src -I$HERE"
for pair in server:robotiq-fake-server codec_dump:robotiq-codec-dump second_opinion:robotiq-second-opinion client_probe:robotiq-client-probe; do
  source_name=${pair%%:*}
  binary=${pair##*:}
  g++ $CXXFLAGS "$HERE/$source_name.cpp" "$OUT/sdk/libgrippers_sdk.a" -pthread -o "$OUT/$binary" \
    || { echo "::error::$source_name.cpp did not compile against the maker's SDK"; exit 1; }
done
git -C "$SDK" log -1 --format='robotiq grippers %H %cs' > "$OUT/robotiq.txt" 2>/dev/null || echo "robotiq grippers (not a git checkout)" > "$OUT/robotiq.txt"
echo "built: $(cd "$OUT" && ls robotiq-* | tr '\n' ' ')($(cat "$OUT/robotiq.txt"))"

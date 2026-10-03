#!/bin/sh
# Build ROS-Industrial's own `simple_message` library, without ROS, with `codec_dump.cpp` (and, when asked, `server.cpp`) into one binary.
#
#   sh sim/build-reference.sh <industrial_core checkout> <output> <source file> [-D flags...]
#
# The library is the maker's: `ros-industrial/industrial_core`, branch `melodic-devel`, BSD-3-Clause, fetched by `git clone` and never copied into this repository (`INDUSTRIAL_CORE`
# in `layout_check.py` and the `Makefile` names where it is). Its own CMake build wants catkin and ROS, which this does not: the library's `log_wrapper.h` compiles its logging out when neither
# `ROS` nor `STDIOLOG` is defined (a controller with no ROS), and its sources compile unchanged with three standard headers it forgets to include, which are added on the command line and not
# by editing the maker's files. Pass `-DSTDIOLOG` to have it print its own debug lines, which at a feedback rate of 50 Hz is a great many.
#
# `-DBYTE_SWAPPING` and `-DFLOAT64` are the maker's own switches for a controller whose CPU is big-endian and one whose `shared_real` is a double (`shared_types.h`).
set -eu

CORE=$1
OUT=$2
MAIN=$3
shift 3

SRC=$CORE/simple_message
FILES=$(ls "$SRC"/src/*.cpp "$SRC"/src/messages/*.cpp "$SRC"/src/socket/*.cpp)

# shellcheck disable=SC2086
g++ -std=c++11 -O1 -pthread -DSIMPLE_MESSAGE_LINUX "$@" -include cstdio -include cstring -include cstdlib \
  -I"$SRC/include" -I"$(dirname "$0")" $FILES "$MAIN" -o "$OUT"

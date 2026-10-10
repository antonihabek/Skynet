#!/bin/sh
# Fetch Rainbow Robotics' own client for its control boxes (`RainbowRobotics/rbpodo`, Apache-2.0) at the commit pinned below, check the tarball against the SHA-256 pinned below, and build its Python module into a virtual
# environment of its own: `sh sim/fetch-sdk.sh <directory>`.
#
# The directory it prints (`<directory>/venv/bin/python`) is a Python with `rbpodo` and `numpy` installed: it is what `layout_check.py` is run with (RBPODO_PYTHON), and the client is run and never imported by the driver and
# never committed. Building it needs `python3 -m venv`, CMake (3.15 or newer), a C++17 compiler and a network: the build downloads Eigen 3.4.0 and pybind11 3.0.0rc1 itself, each by the MD5 its own CMake files pin, and its
# build backend (scikit-build-core) and numpy from PyPI. `RBPODO_COMMIT=main` (the nightly's second run) takes the tip of the maker's default branch instead, in which case the hash is not checked and the run says so.
set -eu

DIR=$1
COMMIT=${RBPODO_COMMIT:-f6ef41adf629dd27c96b0d3e7ebf3d3fc0cc53c4}
SHA256=aa01c8cdc730889fc667648d5909c773dd2ed382d05ed69fdc743cd440d9ab9b
HERE=$(cd "$(dirname "$0")/.." && pwd)

mkdir -p "$DIR"
if [ "$COMMIT" = main ]; then
  echo "== rbpodo at the tip of main; it is not checked against a pin"
  PINNED=0
else
  PINNED=1
fi
curl -sSLf -o "$DIR/sdk.tar.gz" "https://github.com/RainbowRobotics/rbpodo/archive/$COMMIT.tar.gz"
if [ "$PINNED" = 1 ]; then
  echo "$SHA256  $DIR/sdk.tar.gz" | sha256sum -c -
fi
rm -rf "$DIR/sdk" "$DIR/venv"
mkdir -p "$DIR/sdk"
tar xzf "$DIR/sdk.tar.gz" -C "$DIR/sdk" --strip-components=1
python3 -m venv "$DIR/venv"
"$DIR/venv/bin/python" -m pip install --quiet --upgrade pip
if [ "$PINNED" = 1 ] && [ -f "$HERE/requirements-layout.txt" ]; then
  "$DIR/venv/bin/python" -m pip install --quiet --require-hashes -r "$HERE/requirements-layout.txt"
fi
"$DIR/venv/bin/python" -m pip install --quiet "$DIR/sdk"
"$DIR/venv/bin/python" -c "import rbpodo; print('== rbpodo', rbpodo.__file__)"
echo "== $DIR/venv/bin/python (the interpreter layout_check.py is run with)"

#!/bin/sh
# Fetch NUbots' MuJoCo simulator of the Booster K1 (`NUbots/NUSim`, MIT) at the commit pinned below, and check the tarball against the SHA-256 pinned below: `sh nusim/fetch-nusim.sh <directory>`.
# The directory it prints (`<directory>/NUSim`) is the simulator's source tree: `run-nusim.sh` builds it and runs it in a container, and nothing of it is vendored or committed here.
# `NUSIM_REF=main` (the nightly's second run) takes the tip of the maker's default branch instead, in which case the hash is not checked and the run says so.
set -eu

DIR=$1
REF=${NUSIM_REF:-c70591e668f33b5451348c9c724287a3d6f16c7e}
SHA256=20bd609f05d162bdaa629a2024788c492c89c2b90b34538bf223bcd43a62aaaf

mkdir -p "$DIR"
if [ "$REF" = main ]; then
  echo "== NUSim at the tip of main; it is not checked against a pin"
  PINNED=0
else
  PINNED=1
fi
curl -sSLf -o "$DIR/nusim.tar.gz" "https://github.com/NUbots/NUSim/archive/$REF.tar.gz"
if [ "$PINNED" = 1 ]; then
  echo "$SHA256  $DIR/nusim.tar.gz" | sha256sum -c -
fi
rm -rf "$DIR/NUSim"
mkdir -p "$DIR/NUSim"
tar xzf "$DIR/nusim.tar.gz" -C "$DIR/NUSim" --strip-components=1
echo "== $DIR/NUSim (the simulator's source)"

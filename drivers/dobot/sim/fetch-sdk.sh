#!/bin/sh
# Fetch Dobot's own Python client for the V4 controllers (`Dobot-Arm/TCP-IP-Python-V4`, MIT) at the commit pinned below, and check the tarball against the SHA-256 pinned below: `sh sim/fetch-sdk.sh <directory>`.
# The directory it prints (`<directory>/sdk`) holds the `dobot_sdk` package: it is what `layout_check.py` is told by DOBOT_SDK, run and never imported by the driver and never committed.
# `DOBOT_SDK_COMMIT=main` (the nightly's second run) takes the tip of the maker's default branch instead, in which case the hash is not checked and the run says so.
set -eu

DIR=$1
COMMIT=${DOBOT_SDK_COMMIT:-55ec1ec82201aaf2e6d54aa47b6272bbaa14c815}
SHA256=7d5d041e553ae237bd1731a137f2e9663a2a8bdee6d41d6695bd314c1be0fd46

mkdir -p "$DIR"
if [ "$COMMIT" = main ]; then
  echo "== Dobot's client at the tip of main; it is not checked against a pin"
  PINNED=0
else
  PINNED=1
fi
curl -sSLf -o "$DIR/sdk.tar.gz" "https://github.com/Dobot-Arm/TCP-IP-Python-V4/archive/$COMMIT.tar.gz"
if [ "$PINNED" = 1 ]; then
  echo "$SHA256  $DIR/sdk.tar.gz" | sha256sum -c -
fi
rm -rf "$DIR/TCP-IP-Python-V4-$COMMIT" "$DIR/sdk"
mkdir -p "$DIR/sdk"
tar xzf "$DIR/sdk.tar.gz" -C "$DIR/sdk" --strip-components=1
echo "== $DIR/sdk (the directory that holds dobot_sdk)"

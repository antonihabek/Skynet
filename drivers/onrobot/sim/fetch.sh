#!/bin/sh
# Fetch OnRobot's own client packages from the maker's GitHub release and unpack them, without installing: `sh sim/fetch.sh <directory>`. The release and the SHA-256 of each package are pinned below; `ONROBOT_RELEASE`
# names another release, or `latest` (the nightly's second run), in which case the hashes are not checked and the run says so.
set -eu

DIR=$1
RELEASE=${ONROBOT_RELEASE:-1.2.0}
RUNTIME=libonrobot-tool-api1_1.2.0-1_amd64.deb
DEV=libonrobot-tool-api-dev_1.2.0-1_amd64.deb
RUNTIME_SHA256=723e3900f7be0beede8e0721e5b284888e8316aa875e768f32851612bd0700a5
DEV_SHA256=f7c16a971769d0eee96677fe5b74ec673d773c33c743a025b43bb1d3f0ec8ddd
BASE=https://github.com/OnRobot/onrobot-tool-api/releases/download

mkdir -p "$DIR"
cd "$DIR"
PINNED=0
if [ "$RELEASE" = latest ]; then
  RELEASE=$(curl -sSf https://api.github.com/repos/OnRobot/onrobot-tool-api/releases/latest | python3 -c "import json, sys; print(json.load(sys.stdin)['tag_name'])")
  echo "== OnRobot's newest release is $RELEASE; its packages are not checked against a pin"
elif [ "$RELEASE" = 1.2.0 ]; then
  PINNED=1
fi
if [ "$PINNED" = 0 ]; then
  RUNTIME=libonrobot-tool-api1_${RELEASE}-1_amd64.deb
  DEV=libonrobot-tool-api-dev_${RELEASE}-1_amd64.deb
fi
for deb in "$RUNTIME" "$DEV"; do
  curl -sSLf -o "$deb" "$BASE/$RELEASE/$deb"
done
if [ "$PINNED" = 1 ]; then
  echo "$RUNTIME_SHA256  $RUNTIME" | sha256sum -c -
  echo "$DEV_SHA256  $DEV" | sha256sum -c -
fi
rm -rf sdk
mkdir sdk
dpkg -x "$RUNTIME" sdk
dpkg -x "$DEV" sdk
echo "== unpacked into $DIR/sdk"

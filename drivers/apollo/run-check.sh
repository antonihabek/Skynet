#!/bin/sh
# The Apollo row's whole path, in the order it has to happen in.
#
# A script rather than a `make` recipe because it fetches a pinned substrate, verifies it
# two ways, then runs a suite, a cross-check and a conformance run against it — and `make`
# gives each recipe line a shell of its own.
#
# Needs MuJoCo in the project environment plus a GL backend MuJoCo can load at import
# (MUJOCO_GL=osmesa in CI where libosmesa6 is installed, =egl on a machine with Mesa).
set -eu

REPO=$(cd ../.. && pwd)
HERE=$(pwd)
OUT=${TMPDIR:-/tmp}/skynet-apollo-check
APOLLO_PYTHON=${APOLLO_PYTHON:-$REPO/.venv/bin/python}
MENAGERIE_COMMIT=${MENAGERIE_COMMIT:-822c2d8f877dd166c5b7d3c9f7e3c3b6589473b7}
MODEL_SHA=${MODEL_SHA256:-28513f17a37f1cfa384a68e7fb379085561bab34ade8f96fb8ad77bf473087a6}
SCENE_SHA=${SCENE_SHA256:-d57694726b430548fc2719d5f08440dd6a152cda0f04283a64a4dd4edf546499}
MODE=${1:-check}

if ! "$APOLLO_PYTHON" -c "import mujoco" 2>/dev/null; then
  echo "::error::no MuJoCo under $APOLLO_PYTHON (or its GL backend is missing — try"
  echo "MUJOCO_GL=egl). The sim driver has no body without it:"
  echo ""
  echo "  uv venv .venv && uv pip install --python .venv/bin/python mujoco  # repo root"
  echo "  MUJOCO_GL=egl make check                                          # drivers/apollo"
  exit 1
fi

if [ -n "${MENAGERIE_DIR:-}" ]; then
  MODEL_DIR="$MENAGERIE_DIR/apptronik_apollo"
else
  MODEL_DIR="$OUT/menagerie/apptronik_apollo"
  if [ ! -d "$OUT/menagerie/.git" ]; then
    rm -rf "$OUT/menagerie"
    git init -q "$OUT/menagerie"
    git -C "$OUT/menagerie" remote add origin \
      https://github.com/google-deepmind/mujoco_menagerie.git
    git -C "$OUT/menagerie" sparse-checkout set apptronik_apollo
    git -C "$OUT/menagerie" fetch -q --depth 1 origin "$MENAGERIE_COMMIT"
    git -C "$OUT/menagerie" checkout -q FETCH_HEAD
  fi
fi

# Pinned twice: the commit, then the bytes. A model that drifted without the pin moving
# would be a body the suite's hard numbers no longer describe.
GOT_COMMIT=$(git -C "$(dirname "$MODEL_DIR")" rev-parse HEAD 2>/dev/null || echo "unpinned")
if [ "$GOT_COMMIT" != "$MENAGERIE_COMMIT" ]; then
  echo "::error::menagerie checkout is $GOT_COMMIT, want $MENAGERIE_COMMIT. Delete it"
  echo "or pass MENAGERIE_DIR at the pin; a suite run against other bytes proves nothing."
  exit 1
fi
echo "$MODEL_SHA  $MODEL_DIR/apptronik_apollo.xml" | sha256sum -c - >/dev/null
echo "$SCENE_SHA  $MODEL_DIR/scene.xml" | sha256sum -c - >/dev/null
echo "== substrate verified: menagerie $MENAGERIE_COMMIT, model bytes match"

export MENAGERIE_DIR SCENE_PATH="$MODEL_DIR/scene.xml"
export PYTHONPATH="$REPO/bindings/python:$REPO/drivers/mujoco:$HERE"

echo ""
echo "== the driver's own suite"
SUITE=0
"$APOLLO_PYTHON" selftest.py || SUITE=$?
if [ "$MODE" = "suite" ]; then
  exit "$SUITE"
fi

echo ""
echo "== the layout cross-check (served descriptor vs pinned MJCF)"
CROSS=0
"$APOLLO_PYTHON" cross_check.py || CROSS=$?

echo ""
echo "== driver check through python-host"
CHECK=0
make -C "$REPO/drivers/python-host" >/dev/null 2>&1 || true
ROLES=$(tr -d '\n ' < roles.json)
SKYNET_PYTHON_DRIVER=skynet_mujoco \
SKYNET_PYTHON="$APOLLO_PYTHON" \
PYTHONPATH="$REPO/bindings/python:$REPO/drivers/mujoco" \
cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
  driver check "$REPO/drivers/python-host/libskynet_python_host.so" \
  --config "{\"scene\":\"$MODEL_DIR/scene.xml\",\"base\":\"legged\",\"robot_slug\":\"apollo\",\"keyframe\":\"stand\",\"rate_hz\":100,\"role_overrides\":$ROLES}" || CHECK=$?

if [ "$SUITE" -ne 0 ]; then
  echo "::error::the driver's own suite failed ($SUITE). The lines above say which checks."
  exit "$SUITE"
fi
if [ "$CROSS" -ne 0 ]; then
  echo "::error::the layout cross-check failed ($CROSS)."
  exit "$CROSS"
fi
exit "$CHECK"

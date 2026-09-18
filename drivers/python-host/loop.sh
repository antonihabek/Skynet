#!/bin/sh
# The control loop closed on a camera, across two processes and a real driver.
#
# `DRY-COVERAGE-PLAN.md`'s Phase 1 gate. Everything else in this repository exercises one half:
# `make check-capture` is `skynetk` reading a camera through the C shim and an interpreter into
# §47.6's pool, and `cargo test -p skynet-policy-host --test handoff` is a policy in another process
# reading a pool this process filled. Neither is the loop. This is:
#
#   skynetk  — dlopens the C host, which forks python3, which serves a Python driver over a pipe.
#              Reads its wrist camera with `read_sensor` at 10 Hz, publishes each frame into the
#              pool, offers the pool and both rings on a Unix socket, and turns §7's guards over
#              whatever the command ring holds.
#   skynetp  — connects to that socket, maps the two rings and the pool, and runs §23.3's inference
#              backend: joint positions and the pooled frame in, a setpoint per joint out.
#   skynet   — reads the window `skynetk` sealed on the way out and replays every recorded cycle
#              through this edition's Safety Kernel, which must reach the verdicts the recording holds.
#
# What makes it a test rather than a demonstration is the model. Its weight matrix is zero except for
# one 1.0 per row, at the pooled cell with the same index — so joint *j* is commanded to the mean
# intensity of cell *j* of the image, and a policy that never saw a pixel commands nothing at all.
# Both sides are then required to report what they did: frames published, cycles not idle, and zero
# overruns on either side. Then the replay is required to reproduce every verdict, and to say that the
# window recorded every field it evaluated — a replay that will not report its own completeness is
# §15's most damaging shape, and §26.4 is why it does.
#
# Needs: Linux, a C compiler, python3, and — for `BODY=lerobot`, the default — nothing installed, the
# bridge running against its own fake. It does **not** need a robot, and does not claim to be one: the
# frames are a fake's synthetic image and the joints are a fake's encoders, so what this measures is
# the pipeline, not a body.
#
# `BODY=mujoco` puts a physics simulator on the other side of the same ABI, rendering its own wrist
# camera offscreen. That is a *different* claim and a smaller gap: a simulator is not a machine
# (§35.3 caps it at guest whatever it reports), but the pixels are a render of a scene rather than a
# pattern the fake drew, and the joint readings are an integrator rather than a counter. It needs
# `pip install mujoco` and an offscreen GL. Its camera is declared `concurrent_read` (§89.1), so
# `skynetk` reads it on a capture thread of its own and a software render — slower than the period
# on its own, which is F24 — costs the control loop nothing. This is the measurement §89 was
# ratified to close.
#
# `POLICY=onnx` replaces `skynet-infer`'s dense runner with §23.3's named one — an ONNX Runtime
# session, loaded at run time by `skynet-ort` — running the same weights over the same pixels. It
# needs `pip install onnx numpy onnxruntime`. The two axes are independent, so `BODY=mujoco
# POLICY=onnx` is a simulator's render through the production runner and is the widest claim this
# script can make without a robot.
#
# # SKYNET_LOOP_OFFLINE=1 — §11, measured rather than asserted
#
# §11 says *"no control decision may require a network round trip"*, and §15 makes that a **bench**
# test — `offline.control.independent` — because it needs a machine rather than a model. With this set
# the whole run is re-executed inside a network namespace holding nothing but loopback: no route, no
# resolver, no Hub. If any step between a driver's sensor and a guard's verdict reaches out, it cannot
# succeed there, and the assertions below fail.
#
# And then it is **recorded**, which for three sessions it could not be. §12 binds a result to the
# digest of the bytes it was taken against; `skynet bench record` is what binds one, and the run ends
# by writing a record over the three binaries that decided and reading it back through
# `skynet bench award`. The award is `none` and is expected to be — the sim half of guest is not in
# evidence on this path — so what the run asserts is the one thing it changed: the bench test §15
# requires from guest is no longer among the missing.
#
# The witness is this machine and nobody, which `DRY-COVERAGE-PLAN.md` *Decisions* 4 allows for this
# test and for no other: `offline.control.independent` needs a real machine rather than a real body.
set -eu

# Re-executed rather than wrapped per command, so the namespace covers the interpreter, the shim and
# both binaries. `unshare -r` maps this user to root inside, which is what makes `-n` unprivileged.
if [ "${SKYNET_LOOP_OFFLINE:-}" = "1" ] && [ "${SKYNET_LOOP_REEXEC:-}" != "1" ]; then
  if ! command -v unshare >/dev/null 2>&1; then
    echo "::error::SKYNET_LOOP_OFFLINE=1 and there is no \`unshare\`. §11's test needs a network"
    echo "namespace; running the loop with the network up would measure nothing and report a pass,"
    echo "which is the failure §15 spends a rule on. Install util-linux, or drop the variable."
    exit 1
  fi
  # Through `sh` rather than `exec unshare -rn "$0"`, which needs this file's execute bit — and this
  # file was committed without one. `make check-offline` runs it as `sh ./loop.sh`, so the missing
  # bit was invisible until the re-exec tried to run it directly and every offline run on a
  # checkout with real file modes died as `unshare: failed to execute ./loop.sh: Permission
  # denied`. The bit is set now as well; running through the interpreter is what makes it stop
  # mattering, on a zip, a Windows checkout or a filesystem that does not carry modes at all.
  SKYNET_LOOP_REEXEC=1 exec unshare -rn /bin/sh "$0" "$@"
fi

cd "$(dirname "$0")"
LIB=${LIB:-libskynet_python_host.so}
PYTHON=${PYTHON:-python3}
REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-vision-loop
TARGET=${CARGO_TARGET_DIR:-$REPO/target}

# ── the body ─────────────────────────────────────────────────────────────────
#
# One gate, two bodies. Every assertion below is the same for both, and what changes is the driver
# on the other side of the ABI and the numbers its camera reports — which is the point: a loop that
# closes on one body and not the other is a loop that was measuring the body.
#
# `lerobot` is the fake: a synthetic 32x24 image and a fake's encoders, and it needs nothing
# installed. `mujoco` is a physics simulator rendering its own 128x96 wrist camera offscreen, which
# needs `pip install mujoco` and an offscreen GL — `MUJOCO_GL=osmesa` with `libosmesa6`, or `egl`.
#
# The geometry is **not** read from the descriptor here on purpose. The pool's slot size is declared
# on `skynetk`'s command line and the model's vision block is written below, so the two are two
# independent statements about the same camera — and §47.6's `Hold::FrameShape` is what refuses when
# they disagree (F16). A script that derived both from one query would be a loop that could not
# produce that disagreement, and it is the disagreement that has to be a refusal rather than a
# picture of something else read at whatever stride happens to fit.
BODY=${BODY:-lerobot}
case "$BODY" in
  lerobot)
    DRIVER=skynet_lerobot
    DRIVER_PATH="$REPO/bindings/python:$REPO/drivers/lerobot"
    STREAM=wrist
    WIDTH=32
    HEIGHT=24
    COMPONENTS=3
    CHANNELS=6
    ;;
  mujoco)
    DRIVER=skynet_mujoco
    DRIVER_PATH="$REPO/bindings/python:$REPO/drivers/mujoco"
    SKYNET_MUJOCO_SCENE=${SKYNET_MUJOCO_SCENE:-$REPO/drivers/mujoco/scenes/so_arm_shaped.xml}
    export SKYNET_MUJOCO_SCENE
    MUJOCO_GL=${MUJOCO_GL:-osmesa}
    export MUJOCO_GL
    STREAM=wrist_cam
    WIDTH=128
    HEIGHT=96
    COMPONENTS=3
    CHANNELS=6
    ;;
  *)
    echo "::error::BODY=$BODY is not one this gate knows. It is \`lerobot\` or \`mujoco\`."
    exit 1
    ;;
esac
SLOT_BYTES=$((WIDTH * HEIGHT * COMPONENTS))
# Pooled into a 4x4 grid, so sixteen cells reach the model whatever the camera's resolution is.
POOL=4

# ── the runner ───────────────────────────────────────────────────────────────
#
# One gate, two runners, and the same arithmetic in both. `mlp` is `skynet-infer`'s own dense
# network and needs nothing installed; `onnx` is §23.3's named runner — `skynet-ort`, ONNX Runtime
# loaded at run time — and needs `pip install onnx numpy onnxruntime`.
#
# It is a separate axis from `BODY` on purpose. The pixels a body produces and the runner that reads
# them are independent, and a gate that could only vary them together could not say which of the two
# a difference came from.
POLICY=${POLICY:-mlp}
case "$POLICY" in
  mlp | onnx) ;;
  *)
    echo "::error::POLICY=$POLICY is not one this gate knows. It is \`mlp\` or \`onnx\`."
    exit 1
    ;;
esac

rm -rf "$OUT.sock" "$OUT.k.txt" "$OUT.p.txt" "$OUT.r.txt" "$OUT.box"
mkdir -p "$(dirname "$OUT")" "$OUT.box"

# What the network looks like from in here, said before anything runs. In offline mode the namespace is
# required to be empty — a run that reported "offline" with a default route would be measuring the
# opposite of what it claims.
#
# Read out of `/proc`, which is the kernel's own answer, rather than out of `ip route show`. That is
# not a preference: `ip` is `iproute2` and is absent from plenty of minimal images, `command not
# found` writes nothing to stdout, and `wc -l` of nothing is **0** — so the guard below reported an
# empty namespace on a machine with a default route and no `iproute2`, and passed. A check that
# cannot fail for the reason it exists is the exact shape §15 spends a rule on, and this one was
# guarding the sentence that says so.
#
# Two facts rather than one, because they fail independently: a namespace with a route in it was
# never created, and an interface other than `lo` is something to reach even before a route exists.
if [ ! -r /proc/net/route ] || [ ! -r /proc/net/dev ]; then
  echo "::error::/proc/net/route or /proc/net/dev is unreadable, so this script cannot say whether"
  echo "the network is up. Refusing rather than counting zero: an unmeasured network reported as"
  echo "absent is §11 measured with the network up (§15)."
  exit 1
fi
routes=$(awk 'NR > 1 { n++ } END { print n + 0 }' /proc/net/route)
links=$(awk -F: 'NR > 2 { gsub(/ /, "", $1); if ($1 != "lo") n++ } END { print n + 0 }' /proc/net/dev)
if [ "${SKYNET_LOOP_OFFLINE:-}" = "1" ]; then
  if [ "$routes" != "0" ] || [ "$links" != "0" ]; then
    echo "::error::SKYNET_LOOP_OFFLINE=1 and this namespace has $routes route(s) and $links"
    echo "non-loopback interface(s). The namespace was not created, so §11 would be measured with"
    echo "the network up."
    exit 1
  fi
  echo "== network: none — $routes route(s) and $links non-loopback interface(s) in this namespace,"
  echo "   so §11 is measured rather than assumed"
else
  echo "== network: as this machine has it ($routes route(s), $links non-loopback interface(s)) —"
  echo "   set SKYNET_LOOP_OFFLINE=1 for §11"
fi

echo "== building the host and the three binaries"
make --no-print-directory "$LIB" PYTHON="$PYTHON" >/dev/null
cargo build --manifest-path "$REPO/Cargo.toml" -q \
  -p skynet-kernel-host -p skynet-policy-host -p skynet-cli

# The model, written here so the rule and the numbers that assert it are in one place. Row j is zero
# except for a 1.0 at the column of pooled cell j, and the position block in front of it is zero — so
# every commanded setpoint is a pixel measurement and nothing else.
#
# `POLICY=onnx` writes the **same arithmetic** as an ONNX graph and an artefact naming it, so
# §23.3's production runner drives the loop instead of `skynet-infer`'s own. That is the whole of
# the difference: the weights, the pooling, the assertions below and the numbers they check are
# identical, which is what makes a disagreement between the two runners visible here rather than in
# a robot. `skynet-ort`'s twenty tests measure the runner; this measures the runner **in the loop**,
# which is the claim DRY-COVERAGE-PLAN.md's seventh session left open and named as one line.
echo "== writing the model ($POLICY)"
if [ "$POLICY" = "onnx" ]; then
  "$PYTHON" - "$OUT.json" "$OUT.onnx" "$CHANNELS" "$POOL" "$WIDTH" "$HEIGHT" "$COMPONENTS" <<'ONNXPY'
import json, os, sys

try:
    import numpy as np
    import onnx
    from onnx import helper, numpy_helper, TensorProto
except ImportError as missing:
    # Refusing rather than falling back to `mlp`: a gate that quietly ran a different runner than
    # the one it was asked for would report a pass for a claim nobody measured (§15).
    sys.exit(
        f"POLICY=onnx needs the `onnx` and `numpy` packages to write the graph ({missing}).\n"
        "  pip install onnx numpy    — and `onnxruntime` for the library itself"
    )

artefact, model_path, channels, pool = sys.argv[1], sys.argv[2], int(sys.argv[3]), int(sys.argv[4])
image_w, image_h, image_c = int(sys.argv[5]), int(sys.argv[6]), int(sys.argv[7])
cells = pool * pool
width = channels + cells

# The same matrix as the `mlp` branch below, transposed: `skynet-infer` multiplies a row per output
# and ONNX's MatMul takes `[1, width] x [width, channels]`. One 1.0 per column, at the pooled cell
# with the same index, and zeros across the whole position block in front of it.
weights = np.zeros((width, channels), dtype=np.float32)
for j in range(channels):
    weights[channels + j, j] = 1.0

graph = helper.make_graph(
    [helper.make_node("MatMul", ["input", "W"], ["action"])],
    "skynet-vision-loop",
    [helper.make_tensor_value_info("input", TensorProto.FLOAT, [1, width])],
    [helper.make_tensor_value_info("action", TensorProto.FLOAT, [1, channels])],
    [numpy_helper.from_array(weights, name="W")],
)
model = helper.make_model(graph, opset_imports=[helper.make_opsetid("", 13)])
# Pinned rather than taken from whatever `onnx` was installed: a writer newer than the runtime
# produces a file the runtime refuses to load, and the refusal names an IR version rather than
# anything about this loop.
model.ir_version = 9
onnx.checker.check_model(model)
onnx.save(model, model_path)

with open(artefact, "w") as out:
    json.dump(
        {
            "kind": "onnx",
            "version": 1,
            "inputs": {
                "channels": channels,
                "reads": ["position"],
                "vision": {
                    "width": image_w,
                    "height": image_h,
                    "components": image_c,
                    "pool": pool,
                },
            },
            # Relative, which is what `skynet-ort` resolves against the artefact's own directory —
            # the property that lets a policy directory be mounted somewhere else unedited.
            "model": os.path.basename(model_path),
        },
        out,
        indent=2,
    )
ONNXPY
  # The library, in `skynet-ort`'s own order: an explicit `SKYNET_ORT_LIBRARY` wins, and otherwise
  # this asks the interpreter where its `onnxruntime` wheel put the shared object. The wheel ships
  # `libonnxruntime.so.1.30.0` and no `libonnxruntime.so`, so the platform default name that
  # `skynet-ort` falls back to does not resolve against a pip install — which is a property of the
  # wheel rather than of the runner, and is why this looks rather than assumes.
  if [ -z "${SKYNET_ORT_LIBRARY:-}" ]; then
    SKYNET_ORT_LIBRARY=$("$PYTHON" - <<'LIBPY' || true
import glob, os.path, sys
try:
    import onnxruntime
except ImportError:
    sys.exit(0)
capi = os.path.join(os.path.dirname(onnxruntime.__file__), "capi")
found = sorted(glob.glob(os.path.join(capi, "libonnxruntime.so*")))
print(found[-1] if found else "")
LIBPY
)
    if [ -n "$SKYNET_ORT_LIBRARY" ]; then
      export SKYNET_ORT_LIBRARY
      echo "   ONNX Runtime: $SKYNET_ORT_LIBRARY"
    else
      # Not an error here: `skynet-ort` resolves the platform's default name next and refuses by
      # name if that fails too, which is a better message than anything this script could write.
      echo "   ONNX Runtime: not found through python; leaving it to the system loader"
    fi
  else
    echo "   ONNX Runtime: $SKYNET_ORT_LIBRARY (from the environment)"
  fi
else
"$PYTHON" - "$OUT.json" "$CHANNELS" "$POOL" "$WIDTH" "$HEIGHT" "$COMPONENTS" <<'MLPPY'
import json, sys

path, channels, pool = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
# `image_*`, not `width`: the matrix's own width is computed below from the channels and the cells,
# and two things called `width` in nine lines is how a model ends up declaring a 22-pixel camera.
image_w, image_h, image_c = int(sys.argv[4]), int(sys.argv[5]), int(sys.argv[6])
cells = pool * pool
width = channels + cells
rows = []
for j in range(channels):
    row = [0.0] * width
    row[channels + j] = 1.0
    rows.append(row)
model = {
    "kind": "mlp",
    "version": 1,
    # `position` only: the LeRobot bridge reports relative encoder positions and writes §24's `NaN`
    # for the velocity it does not measure, so a model reading velocity would hold on every cycle.
    "inputs": {
        "channels": channels,
        "reads": ["position"],
        "vision": {
            "width": image_w,
            "height": image_h,
            "components": image_c,
            "pool": pool,
        },
    },
    "layers": [{"activation": "identity", "weights": rows, "bias": [0.0] * channels}],
}
with open(path, "w") as out:
    json.dump(model, out, indent=2)
MLPPY
fi

echo "== starting skynetk ($BODY: the driver, the guards, the ${WIDTH}x${HEIGHT} camera and the pool)"
SKYNET_PYTHON="$PYTHON" \
SKYNET_PYTHON_DRIVER="$DRIVER" \
SKYNET_LEROBOT_FAKE=1 \
PYTHONPATH="$DRIVER_PATH" \
"$TARGET/debug/skynetk" run \
  --driver "./$LIB" \
  --hz 50 --cycles 250 \
  --policy-socket "$OUT.sock" \
  --frame-pool "$STREAM:2x$SLOT_BYTES@10" \
  --blackbox "$OUT.box" > "$OUT.k.txt" 2>&1 &
KERNEL=$!

# Wait for the socket rather than sleeping a guess: the interpreter's start-up is the slow part and
# it is not a fixed cost. A run that gave up here would look like a policy that could not connect.
waited=0
while [ ! -S "$OUT.sock" ]; do
  waited=$((waited + 1))
  if [ "$waited" -gt 300 ]; then
    echo "::error::skynetk never bound $OUT.sock. It said:"
    cat "$OUT.k.txt"
    kill "$KERNEL" 2>/dev/null || true
    exit 1
  fi
  sleep 0.1
done

echo "== starting skynetp (§23.3's inference backend, against that pool)"
"$TARGET/debug/skynetp" run \
  --hz 50 --cycles 200 \
  --channels "$CHANNELS" \
  --backend inference \
  --policy "$OUT.json" \
  --ring-socket "$OUT.sock" > "$OUT.p.txt" 2>&1 || true

wait "$KERNEL" || true

echo ""
echo "── skynetk ──────────────────────────────────────────────────────────────"
cat "$OUT.k.txt"
echo "── skynetp ──────────────────────────────────────────────────────────────"
cat "$OUT.p.txt"
echo "─────────────────────────────────────────────────────────────────────────"

fail=0
say() {
  echo "::error::$1"
  fail=1
}

# The capture half: frames were read through the ABI and reached the pool.
grep -Eq '^published +[1-9][0-9]* to the pool' "$OUT.k.txt" ||
  say "no frame reached §47.6's pool. The camera half of the loop did not run."

# The hand-over: the policy mapped the pool and its backend was shown frames.
grep -Eq "frame pool \"$STREAM\" received with the rings" "$OUT.p.txt" ||
  say "skynetp never reported the pool. The hand-off did not happen."
grep -Eq '[1-9][0-9]* of [0-9]+ turn\(s\) were shown a frame' "$OUT.p.txt" ||
  say "skynetp was shown no frame. The pool crossed and the pixels did not."

# The decision: the model commanded rather than holding. This is the line that fails if a pixel never
# reaches the model — every hold reason is printed beside it, so the failure says which one it was.
grep -Eq 'the model commanded on [1-9][0-9]* of' "$OUT.p.txt" ||
  say "the model held on every turn. The reasons are on the skynetp line above."

# And that the runner asked for is the runner that ran. `skynetp` chooses from the file rather than
# from a flag, so a malformed artefact is refused rather than silently read by the other runner —
# but a run that fell back would still print every line above, and the gate would pass having
# measured the thing it was not asked to measure.
grep -Eq "skynetp: model — $POLICY\[" "$OUT.p.txt" ||
  say "POLICY=$POLICY and skynetp did not report a $POLICY model. The line above says what it ran."

# The far side: the kernel read those commands. `idle` counts cycles that found the ring empty, so a
# run where every cycle was idle is a run where nothing crossed.
cycles=$(sed -n 's/^cycles  *\([0-9]*\)$/\1/p' "$OUT.k.txt" | head -1)
idle=$(sed -n 's/^idle  *\([0-9]*\).*/\1/p' "$OUT.k.txt" | head -1)
if [ -z "${cycles:-}" ] || [ -z "${idle:-}" ]; then
  say "skynetk's summary did not report cycles and idle."
elif [ "$idle" -ge "$cycles" ]; then
  say "every one of skynetk's $cycles cycle(s) found the command ring empty ($idle idle)."
fi

# §23.3: inference is never in the real-time path. Both sides count the periods they missed, and both
# are required to have missed none — which is the part of the gate that is about the clock rather than
# about the pixels.
overruns=$(sed -n 's/^overruns  *\([0-9]*\).*/\1/p' "$OUT.k.txt" | head -1)
reads=$(sed -n 's/^frames read  *\([0-9]*\) of.*/\1/p' "$OUT.k.txt" | head -1)
if [ "${overruns:-1}" != "0" ]; then
  say "skynetk missed its control period ${overruns:-?} time(s). §7's loop is the one that may not."
  # The diagnosis, when the numbers give one. A stream the driver does not declare `concurrent_read`
  # is read between cycles on the control thread (§89.2), so a sensor whose read costs more than the
  # period makes the loop miss on exactly the cycles that read — and one overrun per frame is that,
  # rather than a scheduler or a load spike. §89.2 then suspends the stream on its third miss, which
  # is why this is three overruns and not one per frame for the whole run.
  if [ -n "${reads:-}" ] && [ "$overruns" = "$reads" ]; then
    echo "          every one of the $reads cycle(s) that read a frame overran, and no other cycle did."
    echo "          That is not jitter: the driver does not declare \`concurrent_read\` on this stream,"
    echo "          so it was read on §7's control thread and a camera slower than the period delayed"
    echo "          the guards by construction (DRY-COVERAGE-PLAN.md F24). §89.2 suspends such a stream"
    echo "          rather than let it keep costing the period. The fix is the driver's: render off the"
    echo "          control thread and declare it (§89.1), as drivers/mujoco does."
  fi
fi
grep -Eq 'skynetp: 0 overrun\(s\)' "$OUT.p.txt" ||
  say "skynetp missed its control period on at least one turn."

# ── and then the recording, replayed ─────────────────────────────────────────
#
# The last clause of the gate, and the one that makes the rest of it evidence rather than a run: the
# window `skynetk` sealed on the way out is read back and every recorded cycle is recomputed by this
# edition's Safety Kernel. `skynetk` writes the limits beside the window, so the replay needs no second
# argument — a recording carries what happened, not the ceilings it happened under.
SEALED=$(sed -n 's/^sealed  *\([^ ]*\) .*/\1/p' "$OUT.k.txt" | head -1)
if [ -z "${SEALED:-}" ]; then
  say "skynetk sealed no window, so there is nothing to replay."
else
  echo "── skynet blackbox replay ───────────────────────────────────────────────"
  "$TARGET/debug/skynet" blackbox replay "$SEALED" > "$OUT.r.txt" 2>&1 || true
  cat "$OUT.r.txt"
  echo "─────────────────────────────────────────────────────────────────────────"
  grep -Eq 'all [1-9][0-9]* evaluated cycles reproduced' "$OUT.r.txt" ||
    say "the replay did not reproduce every verdict in the window it read."
  # §26.4. A replay of a partial window is presented exactly like a replay of a whole one unless it
  # says so, and this is the line that says so.
  grep -Fq 'every field this replay evaluates was recorded on every cycle' "$OUT.r.txt" ||
    say "the replay would not say the window was complete, so its verdicts are worth less than they look."
fi

rm -f "$OUT.sock"
if [ "$fail" -ne 0 ]; then
  echo ""
  echo "the loop did not close. The two summaries above are what to read."
  exit 1
fi

# ── and then the record, which is what makes the offline run evidence ────────
#
# Everything above this line is a measurement. §12's rule is that a result is bound to the digest of
# the bytes it was taken against, and `skynet bench record` is what binds one — so the artefacts are
# named here, hashed there, and the result travels with them or not at all.
#
# The subject is the three things that decide: the kernel host that turns §7's guards, the policy
# host that produces the setpoints, and the C shim that is the driver from the ABI's side. The
# interpreter and the Python bridge are the body, and a body is not what §11 is about.
#
# Witnessed by the **machine**, not by a person, and that is the one carve-out `DRY-COVERAGE-PLAN.md`
# *Decisions* 4 makes: `offline.control.independent` needs a real machine rather than a real body, so
# a machine may witness it and the record says which it was.
if [ "${SKYNET_LOOP_OFFLINE:-}" = "1" ]; then
  echo ""
  echo "── skynet bench record ──────────────────────────────────────────────────"
  case "$TARGET" in
    "$REPO"/*)
      RECORD="$OUT.bench.json"
      rm -f "$RECORD"
      RELTARGET=${TARGET#"$REPO"/}
      "$TARGET/debug/skynet" bench record \
        "$RELTARGET/debug/skynetk" \
        "$RELTARGET/debug/skynetp" \
        "drivers/python-host/$LIB" \
        --root "$REPO" \
        --what "the control loop that closed with no network: skynetk, skynetp and the C host" \
        --test offline.control.independent \
        --outcome pass \
        --machine "$(uname -sr) $(uname -m)" \
        --detail "250 kernel cycles and 200 policy turns inside a network namespace holding only loopback — 0 routes, 0 non-loopback interfaces — with 0 overruns on either side and every recorded cycle reproduced by a replay." \
        --out "$RECORD" ||
        say "the bench record could not be written, so this run measured §11 and recorded nothing."

      if [ "$fail" -eq 0 ]; then
        echo ""
        echo "── skynet bench award ───────────────────────────────────────────────────"
        # Exit code ignored on purpose: the award is `none` and is *expected* to be, because the sim
        # half of guest is not in evidence here. What is asserted is narrower and is the only thing
        # this run changed — the bench test §15 requires from guest is no longer among the missing.
        # `NO_COLOR=1` because this output is grepped below, and `term` colours whenever `TERM` is
        # set — it has no `isatty` to tell a pipe from a terminal. An escape code between the
        # indent and the word `missing` is the difference between an assertion and a no-op.
        NO_COLOR=1 "$TARGET/debug/skynet" bench award "$RECORD" --claim guest --root "$REPO" \
          > "$OUT.a.txt" 2>&1 || true
        cat "$OUT.a.txt"
        grep -Eq '^ *missing' "$OUT.a.txt" ||
          say "the award reported nothing missing, which cannot be right with no sim evidence here."
        if grep -E '^ *missing' "$OUT.a.txt" | grep -Fq 'offline.control.independent'; then
          say "offline.control.independent is still missing after the record was written. The record did not reach the award."
        fi
      fi
      ;;
    *)
      echo "CARGO_TARGET_DIR is outside the repository ($TARGET), and a record's artefact paths are"
      echo "relative to one root. No record was written — re-run with the default target directory."
      ;;
  esac
  echo "─────────────────────────────────────────────────────────────────────────"
fi

if [ "$fail" -ne 0 ]; then
  echo ""
  echo "the run measured §11 and did not record it. The lines above are what to read."
  exit 1
fi

echo ""
echo "The loop closed: a camera the driver rendered reached a policy in another process, the setpoints"
echo "it produced reached the kernel's guards with neither side missing its period, and the sealed"
echo "window replayed to the same verdicts through the same kernel."
if [ "${SKYNET_LOOP_OFFLINE:-}" = "1" ]; then
  echo ""
  echo "And it closed with **no network at all** — no route, no resolver, nothing to reach. That is"
  echo "§11's invariant measured on a machine rather than argued about, and it is now **recorded**:"
  echo "$RECORD is a bench record bound to the digest of the three binaries that decided, and"
  echo "\`skynet bench award\` reads it back only against those same bytes. Rebuild any of them and"
  echo "the record stops applying, which is §12's rule and the whole of the mechanism."
  echo ""
  echo "It is witnessed by this machine and by nobody, which is legitimate for exactly this test and"
  echo "for no other (DRY-COVERAGE-PLAN.md Decisions 4). Nothing signed it: §19 forbids a robot"
  echo "holding a key, so the Hub awards and this is a report (§15.1)."
fi
echo ""
echo "What this is not: a robot. The body is $BODY — a fake or a simulator, either way not a machine —"
echo "so the other 18 bench tests in §15 are still \`missing\` and no tier is awarded here."

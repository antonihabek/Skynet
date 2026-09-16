#!/bin/sh
# The control loop closed on a camera, across two processes and a real driver.
#
# `DRY-COVERAGE-PLAN.md`'s Phase 1 gate. Everything else in this repository exercises one half:
# `make check-capture` is `skynetk` reading a camera through the C shim and an interpreter into
# §47.6's pool, and `cargo test -p skynet-policy-host --test handoff` is a policy in another process
# reading a pool this process filled. Neither is the loop. This is:
#
#   skynetk  — dlopens the C host, which forks python3, which serves the LeRobot bridge over a pipe.
#              Reads the fake's 32x24 wrist camera with `read_sensor` at 10 Hz, publishes each frame
#              into the pool, offers the pool and both rings on a Unix socket, and turns §7's guards
#              over whatever the command ring holds.
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
# Needs: Linux, a C compiler, python3, and nothing installed — the LeRobot bridge runs against its own
# fake. It does **not** need a robot, and does not claim to be one: the frames are a fake's synthetic
# image and the joints are a fake's encoders, so what this measures is the pipeline, not a body.
#
# # SKYNET_LOOP_OFFLINE=1 — §11, measured rather than asserted
#
# §11 says *"no control decision may require a network round trip"*, and §15 makes that a **bench**
# test — `offline.control.independent` — because it needs a machine rather than a model. With this set
# the whole run is re-executed inside a network namespace holding nothing but loopback: no route, no
# resolver, no Hub. If any step between a driver's sensor and a guard's verdict reaches out, it cannot
# succeed there, and the assertions below fail.
#
# It is **not** recorded as bench evidence, and this script has no way to record any: §12 binds a
# result to the digest of the bytes it was taken against, and nothing in this workspace produces or
# consumes such a record. So this measures the property and says it measured it; the dry ceiling stays
# where `DRY-COVERAGE-PLAN.md`'s F1 puts it.
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
  SKYNET_LOOP_REEXEC=1 exec unshare -rn "$0" "$@"
fi

cd "$(dirname "$0")"
LIB=${LIB:-libskynet_python_host.so}
PYTHON=${PYTHON:-python3}
REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-vision-loop
TARGET=${CARGO_TARGET_DIR:-$REPO/target}

# 24 rows, 32 columns, 3 components: 2,304 bytes a frame, which is what the pool's slots are declared
# to hold. Pooled into a 4x4 grid, so sixteen cells reach the model.
CHANNELS=6
POOL=4

rm -rf "$OUT.sock" "$OUT.k.txt" "$OUT.p.txt" "$OUT.r.txt" "$OUT.box"
mkdir -p "$(dirname "$OUT")" "$OUT.box"

# What the network looks like from in here, said before anything runs. In offline mode the namespace is
# required to be empty — a run that reported "offline" with a default route would be measuring the
# opposite of what it claims.
routes=$(ip route show 2>/dev/null | wc -l || echo unknown)
if [ "${SKYNET_LOOP_OFFLINE:-}" = "1" ]; then
  if [ "$routes" != "0" ]; then
    echo "::error::SKYNET_LOOP_OFFLINE=1 and this namespace has $routes route(s)."
    echo "The namespace was not created, so §11 would be measured with the network up."
    exit 1
  fi
  echo "== network: none — $routes route(s) in this namespace, so §11 is measured rather than assumed"
else
  echo "== network: as this machine has it ($routes route(s)) — set SKYNET_LOOP_OFFLINE=1 for §11"
fi

echo "== building the host and the three binaries"
make --no-print-directory "$LIB" PYTHON="$PYTHON" >/dev/null
cargo build --manifest-path "$REPO/Cargo.toml" -q \
  -p skynet-kernel-host -p skynet-policy-host -p skynet-cli

# The model, written here so the rule and the numbers that assert it are in one place. Row j is zero
# except for a 1.0 at the column of pooled cell j, and the position block in front of it is zero — so
# every commanded setpoint is a pixel measurement and nothing else.
echo "== writing the model"
"$PYTHON" - "$OUT.json" "$CHANNELS" "$POOL" <<'PY'
import json, sys

path, channels, pool = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
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
        "vision": {"width": 32, "height": 24, "components": 3, "pool": pool},
    },
    "layers": [{"activation": "identity", "weights": rows, "bias": [0.0] * channels}],
}
with open(path, "w") as out:
    json.dump(model, out, indent=2)
PY

echo "== starting skynetk (the driver, the guards, the camera and the pool)"
SKYNET_PYTHON="$PYTHON" \
SKYNET_PYTHON_DRIVER=skynet_lerobot \
SKYNET_LEROBOT_FAKE=1 \
PYTHONPATH="$REPO/bindings/python:$REPO/drivers/lerobot" \
"$TARGET/debug/skynetk" run \
  --driver "./$LIB" \
  --hz 50 --cycles 250 \
  --policy-socket "$OUT.sock" \
  --frame-pool "wrist:2x2304@10" \
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
grep -Eq 'frame pool "wrist" received with the rings' "$OUT.p.txt" ||
  say "skynetp never reported the pool. The hand-off did not happen."
grep -Eq '[1-9][0-9]* of [0-9]+ turn\(s\) were shown a frame' "$OUT.p.txt" ||
  say "skynetp was shown no frame. The pool crossed and the pixels did not."

# The decision: the model commanded rather than holding. This is the line that fails if a pixel never
# reaches the model — every hold reason is printed beside it, so the failure says which one it was.
grep -Eq 'the model commanded on [1-9][0-9]* of' "$OUT.p.txt" ||
  say "the model held on every turn. The reasons are on the skynetp line above."

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
grep -Eq '^overruns *0 ' "$OUT.k.txt" ||
  say "skynetk missed its control period. §7's loop is the one that may not."
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

echo ""
echo "The loop closed: a camera the driver rendered reached a policy in another process, the setpoints"
echo "it produced reached the kernel's guards with neither side missing its period, and the sealed"
echo "window replayed to the same verdicts through the same kernel."
if [ "${SKYNET_LOOP_OFFLINE:-}" = "1" ]; then
  echo ""
  echo "And it closed with **no network at all** — no route, no resolver, nothing to reach. That is"
  echo "§11's invariant measured on a machine rather than argued about. It is still not §15's"
  echo "\`offline.control.independent\`: that is a bench result, and nothing here can record one,"
  echo "because §12 binds a result to the digest of the bytes it was taken against and no producer or"
  echo "consumer of such a record exists yet. See DRY-COVERAGE-PLAN.md."
fi
echo ""
echo "What this is not: a robot. The body is the LeRobot bridge's fake and the image is synthetic, so"
echo "every bench test in §15 is still \`missing\` and the dry ceiling is unchanged."

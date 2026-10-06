#!/bin/sh
# The X1 row's whole path, in the order it has to happen in, in two phases.
#
# A script rather than a `make` recipe because it starts processes, waits for DDS discovery and
# a simulator boot, runs conformance suites against them, stops them and then reads what they
# counted — and `make` gives each recipe line a shell of its own.
#
# Phase 1 is the fake on DDS domain 42: the driver's own suite (the discovery, the impedance
# mapping, the clamp, every refusal), `skynet driver check` through the real ABI, and the
# fake's own count of malformed, out-of-range and partial commands. Phase 2 is the vendor's
# sim on domain 0: the motion proof, then `skynet driver check` against live physics. Two
# phases in sequence, not in parallel: the sim owns the GPU (however software) and the fake
# owns the suite's timing, and a parallel run would interleave both into one unreadable log.
# Sequence is slower and legible.
set -eu

if [ -z "${X1_INFER:-}" ]; then
  echo "::error::X1_INFER is not set. Run through \`make check X1_INFER=... ROS_ROOT=...\`."
  exit 1
fi
if [ -z "${ROS_ROOT:-}" ]; then
  echo "::error::ROS_ROOT is not set. Run through \`make check X1_INFER=... ROS_ROOT=...\`."
  exit 1
fi

REPO=$(cd ../.. && pwd)

# `skynet driver check`, through a `skynet` already built when SKYNET names one (the seeding campaign's container has no Rust
# toolchain, DRY-COVERAGE-PLAN.md 12.C.3), and through cargo otherwise, as it always did.
skynet_cli() {
  if [ -n "${SKYNET:-}" ]; then
    "$SKYNET" "$@"
  else
    cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- "$@"
  fi
}
OUT=${TMPDIR:-/tmp}/skynet-x1-check
MODEL=$X1_INFER/build/cfg/sim_module/model/mjcf/robot/xyber_x1/xyber_x1_serial.xml

export AMENT_PREFIX_PATH=$ROS_ROOT:$X1_INFER/build/install
export ROS_DISTRO=jazzy
export LD_LIBRARY_PATH=$ROS_ROOT/lib:$ROS_ROOT/lib/x86_64-linux-gnu:$X1_INFER/build:$X1_INFER/build/install/lib
# Headless GL for the sim's MuJoCo render thread: software rasterising, no GPU claimed.
export LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe MESA_LOADER_DRIVER_OVERRIDE=swrast
export __GLX_VENDOR_LIBRARY_NAME=mesa

if [ ! -f "$MODEL" ]; then
  echo "::error::no MJCF at $MODEL. The vendor tree is unbuilt or older than the driver."
  exit 1
fi

echo ""
echo "###### phase 1: the fake on domain 42 ######"
rm -f "$OUT.fake.txt" "$OUT.fake.err"
./fake_x1 42 "$MODEL" > "$OUT.fake.txt" 2> "$OUT.fake.err" &
FAKE=$!
trap 'kill $FAKE 2>/dev/null || true' EXIT INT TERM

# DDS discovery is not instant and is not a fixed cost. Waiting for the fake's own first line is
# the signal that its publisher exists, which is what the driver is about to look for.
waited=0
while ! grep -q "serving 4 joints" "$OUT.fake.err" 2>/dev/null; do
  waited=$((waited + 1))
  if [ "$waited" -gt 100 ]; then
    echo "::error::the fake never announced itself. It said:"
    cat "$OUT.fake.err"
    exit 1
  fi
  sleep 0.1
done
echo "== the fake is serving 4 joints on domain 42"

# The driver's own suite first: the discovery, the mode mapping read off the wire, the mask,
# the clamp and every refusal — then the conformance run through the real ABI. Gains in the
# config, because this driver refuses a position command without them (§63.4 through a gain):
# an X1 drive takes an impedance frame and a stiffness is the operator's to choose.
echo ""
echo "== the driver's own suite"
SUITE=0
./selftest 42 "$MODEL" || SUITE=$?

echo ""
echo "== skynet driver check, against the fake"
CHECK_FAKE=0
skynet_cli driver check ./libskynet_x1.so \
  --config "{\"domain\":42,\"model\":\"$MODEL\",\"kp\":50.0,\"kd\":2.0}" || CHECK_FAKE=$?

kill -TERM $FAKE 2>/dev/null || true
wait $FAKE 2>/dev/null || true
trap - EXIT INT TERM

echo ""
echo "== what the fake counted"
cat "$OUT.fake.txt"

# The three refusals the substrate exists for. A conformance run that passed while naming three
# joints, or commanding past the MJCF, would be a green tick on a driver the sim answers with
# motion it never ordered — the index-0 default is silent.
python3 - "$OUT.fake.txt" <<'PY'
import json, sys

with open(sys.argv[1], encoding="utf-8") as handle:
    counted = json.loads(handle.read().strip() or "{}")

problems = []
if counted.get("commands", 0) == 0:
    problems.append("the fake received no command at all — the driver's publisher never "
                    "reached it, so nothing below was measured")
if counted.get("malformed", 0):
    problems.append(f"{counted['malformed']} command(s) were short, ragged or misnamed. The "
                    "sim builds its index map from the command's own names, so a short "
                    "command drives silent joints from the first joint's setpoint")
if counted.get("out_of_range", 0):
    problems.append(f"{counted['out_of_range']} position target(s) left the wire outside the "
                    "MJCF travel the driver clamps to")
if counted.get("partial", 0):
    problems.append(f"{counted['partial']} command(s) omitted a joint the body has")

for problem in problems:
    print(f"::error::{problem}")
sys.exit(1 if problems else 0)
PY
COUNT=$?

if [ "$SUITE" -ne 0 ]; then
  echo "::error::the driver's own suite failed ($SUITE). The lines above say which checks."
  exit "$SUITE"
fi
if [ "$COUNT" -ne 0 ]; then
  echo "::error::the fake counted a defect (see above)."
  exit "$COUNT"
fi
if [ "$CHECK_FAKE" -ne 0 ]; then
  echo "::error::skynet driver check failed against the fake ($CHECK_FAKE)."
  exit "$CHECK_FAKE"
fi
echo ""
echo "phase 1 passed: the suite, the fake's counts and the conformance run."

# The seeding campaign stops here: the vendor's sim (phase 2) is the rung above the one it measures, and a defect only the sim sees
# is not counted at this rung. Everything before this line is the hermetic suite.
if [ "${X1_PHASE1_ONLY:-}" = 1 ]; then
  exit 0
fi

echo ""
echo "###### phase 2: the vendor's sim on domain 0 ######"
# The substrate config, generated from the vendor's own sim config at check time — never a stray
# file in /tmp, which a fresh vendor checkout would not have. Three changes, all load-bearing:
# the subscribe backend gains `ros2` (so SimModule hears /joint_cmd from outside AimRT), the
# ControlModule goes away (its policy would fight the driver for the same joints), and so does
# the JoyStickModule, which exists to feed that policy — and which, with no joystick attached,
# indexes an empty button array and takes the whole process down with a segfault
# (`joy_stick_module.cc:109`, DRY-COVERAGE-PLAN.md F132).
python3 - "$X1_INFER/build/cfg/x1_cfg_sim.yaml" "$OUT.substrate.yaml" <<'PY'
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    lines = handle.read().split("\n")

sub = next(i for i, line in enumerate(lines)
             if line.strip().startswith("sub_topics_options:"))
backend = next(i for i in range(sub, len(lines)) if "enable_backends: [local]" in lines[i])
lines[backend] = lines[backend].replace("[local]", "[local, ros2]")
for name, cfg in (("ControlModule", "control_module"), ("JoyStickModule", "joy_stick_module")):
    lines = [line for line in lines if line != f"          - {name}"]
    module = next(i for i, line in enumerate(lines) if line == f"      - name: {name}")
    assert lines[module + 1].strip() == "log_lvl: INFO", lines[module + 1]
    assert cfg in lines[module + 2], lines[module + 2]
    del lines[module:module + 3]

with open(sys.argv[2], "w", encoding="utf-8") as handle:
    handle.write("\n".join(lines))
print("substrate config written")
PY

rm -f "$OUT.sim.out" "$OUT.sim.err"
(cd "$X1_INFER/build" && ROS_DOMAIN_ID=0 ./aimrt_main --cfg_file_path="$OUT.substrate.yaml" \
  > "$OUT.sim.out" 2> "$OUT.sim.err") &
SIM=$!
trap 'kill $SIM 2>/dev/null || true' EXIT INT TERM

# AimRT's own startup line is the readiness signal — not a sleep, which would be a guess about
# MuJoCo's load time, and not the first JointState, which needs a command before it exists.
waited=0
while ! grep -q "AimRT startup completed" "$OUT.sim.out" 2>/dev/null; do
  waited=$((waited + 1))
  # A sim that died is not a slow one: say so now rather than after two minutes of waiting.
  if ! kill -0 $SIM 2>/dev/null; then
    wait $SIM; status=$?
    echo "::error::the sim exited (status $status) before it started. It said:"
    tail -30 "$OUT.sim.out"
    tail -30 "$OUT.sim.err"
    exit 1
  fi
  if [ "$waited" -gt 240 ]; then
    echo "::error::the sim never started. It said:"
    tail -30 "$OUT.sim.out"
    tail -30 "$OUT.sim.err"
    exit 1
  fi
  sleep 0.5
done
echo "== the sim is up on domain 0"

# The motion proof: a live elbow displaced 0.3 rad, asserted to have travelled. Then the same
# conformance run as phase 1, against physics instead of the fake — primed, because the sim
# publishes once per command and the checker opens cold.
echo ""
echo "== the motion proof"
MOTION=0
./sim_motion 0 "$MODEL" || MOTION=$?

echo ""
echo "== skynet driver check, against the sim"
CHECK_SIM=0
skynet_cli driver check ./libskynet_x1.so \
  --config "{\"domain\":0,\"model\":\"$MODEL\",\"kp\":50.0,\"kd\":2.0,\"prime\":true}" || CHECK_SIM=$?

# AimRT swallows SIGTERM once it is spinning (observed 2026-09-20): TERM politely, then KILL
# what is still there, so a green run never leaves a MuJoCo render thread behind.
kill -TERM $SIM 2>/dev/null || true
sleep 3
kill -KILL $SIM 2>/dev/null || true
wait $SIM 2>/dev/null || true
trap - EXIT INT TERM

if [ "$MOTION" -ne 0 ]; then
  echo "::error::the motion proof failed ($MOTION) — the sim did not move."
  exit "$MOTION"
fi
if [ "$CHECK_SIM" -ne 0 ]; then
  echo "::error::skynet driver check failed against the sim ($CHECK_SIM)."
  exit "$CHECK_SIM"
fi
echo ""
echo "phase 2 passed: the sim moved, and the conformance run holds against physics."
echo ""
echo "the X1 row passed: the suite, the fake's counts, the motion proof and both conformance runs."

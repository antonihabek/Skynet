#!/bin/sh
# The Unitree row against unitree_mujoco, the vendor's own simulator, once per body
# (DRY-COVERAGE-PLAN.md Phase 8, item 8.1). `run-check.sh` is the same shape against the fake.
#
# Runs where the simulator is built — `sim/Dockerfile` is that place, and `make check-sim-docker`
# runs this inside it. `UNITREE_MUJOCO` names the simulator's checkout; the image sets it.
#
# Per body: write the simulator's own configuration file, read the vendor model's actuator order
# out of its XML, start the simulator headless, run `sim_check` against it, run the conformance
# suite against it, stop it. Sequence rather than parallel for `run-check.sh`'s reason: seven
# simulators' logs interleaved into one is a log nobody can read.
set -eu

SIM=${UNITREE_MUJOCO:-}
if [ -z "$SIM" ] || [ ! -x "$SIM/simulate/build/unitree_mujoco" ]; then
  echo "::error::UNITREE_MUJOCO does not name a built unitree_mujoco checkout."
  echo "The simulator is built by sim/Dockerfile; run this through it:"
  echo ""
  echo "  make check-sim-docker"
  echo ""
  echo "Refusing rather than reporting the row unmeasured as passed."
  exit 1
fi

IFACE=lo
REPO=$(cd ../.. && pwd)
OUT=${TMPDIR:-/tmp}/skynet-unitree-sim
echo "== unitree_mujoco $(cat "$SIM.commit" 2>/dev/null || echo '(commit not recorded)')"

# The simulator reads `config.yaml` beside its build directory and nothing else; its command line
# overrides robot, scene, domain and interface but not the elastic band. So the file is written
# per body, whole, from the keys the vendor's own `load_from_yaml` requires. Domain 0 on loopback:
# the robot's domain, on an interface no robot is on.
write_config() {
  cat > "$SIM/simulate/config.yaml" <<EOF
robot: "$1"
robot_scene: "$2"
domain_id: 0
interface: "$IFACE"
idl_type: -1
use_joystick: 0
joystick_type: "xbox"
joystick_device: "/dev/input/js0"
joystick_bits: 16
print_scene_information: 0
enable_elastic_band: $3
EOF
}

# The vendor model's wiring, one line per motor slot: the joint the slot's actuator drives, then
# the joints its position, velocity and torque sensors measure. The simulator's bridge maps slot i
# to actuator i and reads slot i's state from sensors i, i+n and i+2n, so these four columns are
# what a slot *is* in the simulator. Read from the XML the scene includes, not from a list kept
# here, so `sim_check`'s comparisons are against the vendor's file — and a sensor wired to the
# wrong joint (F123) is named rather than silently read as the right one.
write_actuators() {
  python3 - "$SIM/unitree_robots/$1/$2" > "$3" <<'PY'
import pathlib, re, sys

scene = pathlib.Path(sys.argv[1])
included = re.search(r'<include\s+file="([^"]+)"', scene.read_text())
model = scene.parent / included.group(1) if included else scene
text = model.read_text()
block = re.search(r"<actuator>(.*?)</actuator>", text, re.S)
if block is None:
    sys.exit(f"{model} has no <actuator> block")
actuators = re.findall(r'<motor\b[^>]*?\bjoint="([^"]+)"', block.group(1))
sensors = re.search(r"<sensor>(.*?)</sensor>", text, re.S)
if sensors is None:
    sys.exit(f"{model} has no <sensor> block")
entries = re.findall(r"<(\w+)\b([^>]*)/?>", sensors.group(1))
n = len(actuators)

def measured(index, kind):
    if index >= len(entries) or entries[index][0] != kind:
        return "-"
    found = re.search(r'\bjoint="([^"]+)"', entries[index][1])
    return found.group(1) if found else "-"

for slot, joint in enumerate(actuators):
    print("\t".join([joint, measured(slot, "jointpos"), measured(slot + n, "jointvel"),
                     measured(slot + 2 * n, "jointactuatorfrc")]))
PY
}

run_model() {
  MODEL=$1 ROBOT=$2 SCENE=$3 BAND=$4 KP=$5 KD=$6
  echo ""
  echo "###### $MODEL in unitree_mujoco ($ROBOT/$SCENE, elastic band $BAND) ######"
  write_config "$ROBOT" "$SCENE" "$BAND"
  write_actuators "$ROBOT" "$SCENE" "$OUT.$MODEL.actuators"

  xvfb-run -a "$SIM/simulate/build/unitree_mujoco" > "$OUT.$MODEL.sim.log" 2>&1 &
  SIM_PID=$!
  trap 'kill $SIM_PID 2>/dev/null || true; pkill -x unitree_mujoco 2>/dev/null || true' EXIT INT TERM

  # The bridge thread announces which IDL it chose once the model is loaded and DDS is up; that
  # line is the signal the robot's topics exist.
  waited=0
  while ! grep -q "DDS IDL for robot" "$OUT.$MODEL.sim.log" 2>/dev/null; do
    waited=$((waited + 1))
    if [ "$waited" -gt 300 ]; then
      echo "::error::unitree_mujoco never started its bridge for $MODEL. It said:"
      cat "$OUT.$MODEL.sim.log"
      exit 1
    fi
    sleep 0.1
  done
  grep "DDS IDL for robot" "$OUT.$MODEL.sim.log"

  SIMCHECK=0
  ./sim_check "$IFACE" "$MODEL" "$OUT.$MODEL.actuators" || SIMCHECK=$?

  echo ""
  echo "== the conformance suite against the simulator ($MODEL)"
  CHECK=0
  cargo run --manifest-path "$REPO/Cargo.toml" -q -p skynet-cli -- \
    driver check ./libskynet_unitree.so \
    --config "{\"interface\":\"$IFACE\",\"model\":\"$MODEL\",\"kp\":$KP,\"kd\":$KD}" || CHECK=$?

  kill -TERM $SIM_PID 2>/dev/null || true
  pkill -x unitree_mujoco 2>/dev/null || true
  wait $SIM_PID 2>/dev/null || true
  trap - EXIT INT TERM

  if [ "$SIMCHECK" -ne 0 ]; then
    echo "::error::sim_check failed for $MODEL ($SIMCHECK). The lines above say which checks."
    return "$SIMCHECK"
  fi
  return "$CHECK"
}

# The gains are the vendor's, cited in `sim_check.cpp`'s `BODIES` table; the conformance run
# uses the same pair so the two halves command the same stiffness.
FAILED=0
record() { if [ "$FAILED" -eq 0 ]; then FAILED=$1; fi; }
run_model go2 go2 scene.xml 0 50.0 3.5 || record $?
run_model b2 b2 scene.xml 0 1000.0 10.0 || record $?
run_model h1 h1 scene.xml 1 80.0 2.0 || record $?
run_model g1 g1 scene_23dof.xml 1 40.0 1.0 || record $?
run_model h1_2 h1_2 scene.xml 1 80.0 2.0 || record $?
run_model h2 h2 scene.xml 1 60.0 1.0 || record $?
run_model r1 r1 scene.xml 1 100.0 2.0 || record $?

if [ "$FAILED" -ne 0 ]; then
  echo ""
  echo "::error::the Unitree bodies in unitree_mujoco failed (first failure: $FAILED)."
  exit "$FAILED"
fi
echo ""
echo "all seven Unitree bodies passed in the vendor's simulator: the slot maps against its models,"
echo "the stand, the arm, the hold, the stop, and the conformance runs."

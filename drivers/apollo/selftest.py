"""skynet-apollo's own suite: substrate, bring-up, the body, commands, the pipe.

The driver under test is skynet_mujoco pointed at the menagerie Apollo scene — this suite
is what makes that a *row* rather than a config: the joint set in order, the ranges off
the file, the stand, the motion proof, and every refusal this body must produce.

Needs MuJoCo plus a GL backend it can load (MUJOCO_GL=osmesa in CI, =egl here), and the
pinned substrate: SCENE_PATH at menagerie MENAGERIE_COMMIT, verified by run-check.sh.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import subprocess
import sys
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "bindings", "python"))
sys.path.insert(0, os.path.join(HERE, "..", "mujoco"))
sys.path.insert(0, HERE)

MODEL_SHA = "28513f17a37f1cfa384a68e7fb379085561bab34ade8f96fb8ad77bf473087a6"
SCENE_SHA = "d57694726b430548fc2719d5f08440dd6a152cda0f04283a64a4dd4edf546499"

# Actuator order in apptronik_apollo.xml, which is the RHAL joint order: bindings are
# built actuator by actuator, and each of these drives exactly one hinge.
JOINTS = [
    "torso_yaw", "torso_roll", "torso_pitch",
    "neck_yaw", "neck_roll", "neck_pitch",
    "l_shoulder_aa", "l_shoulder_ie", "l_shoulder_fe", "l_elbow_fe",
    "l_wrist_roll", "l_wrist_yaw", "l_wrist_pitch",
    "r_shoulder_aa", "r_shoulder_ie", "r_shoulder_fe", "r_elbow_fe",
    "r_wrist_roll", "r_wrist_yaw", "r_wrist_pitch",
    "l_hip_ie", "l_hip_aa", "l_hip_fe", "l_knee_fe", "l_ankle_ie", "l_ankle_pd",
    "r_hip_ie", "r_hip_aa", "r_hip_fe", "r_knee_fe", "r_ankle_ie", "r_ankle_pd",
]
ELBOW = JOINTS.index("l_elbow_fe")
CAMERAS = {"track", "torso_oak_d_pro_w_front", "torso_oak_d_pro_w_rear"}

CHECKS = 0
FAILURES: list[str] = []


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    print(("  ok    " if held else "  FAIL  ") + what, flush=True)
    if not held:
        FAILURES.append(what)


def section(title: str) -> None:
    print(f"\n{title}", flush=True)


def main() -> int:
    try:
        import mujoco  # noqa: F401
    except ImportError:
        print("::error::skynet-apollo's suite needs MuJoCo: MUJOCO_GL names its backend, "
              "so an unloadable backend fails the import, not a check. See README.md.")
        return 1
    scene = os.environ.get("SCENE_PATH") or os.path.join(
        os.environ.get("MENAGERIE_DIR", ""), "apptronik_apollo", "scene.xml")
    if not scene or not os.path.isfile(scene):
        print("::error::no pinned Apollo scene (SCENE_PATH). run-check.sh fetches and "
              "verifies it; the suite refuses to run against other bytes.")
        return 1

    import skynet_rhal as rhal
    from skynet_mujoco.bridge import MujocoBridge

    with open(os.path.join(HERE, "roles.json"), encoding="utf-8") as handle:
        roles = json.load(handle)
    model_xml = os.path.join(os.path.dirname(scene), "apptronik_apollo.xml")

    def config(**overrides) -> str:
        base = {"scene": scene, "base": "legged", "robot_slug": "apollo",
                "keyframe": "stand", "rate_hz": 100, "role_overrides": roles}
        base.update(overrides)
        return json.dumps(base)

    section("substrate: the pinned model, and nothing else")
    with open(model_xml, "rb") as handle:
        check(hashlib.sha256(handle.read()).hexdigest() == MODEL_SHA,
              "the model bytes are the pin")
    with open(scene, "rb") as handle:
        check(hashlib.sha256(handle.read()).hexdigest() == SCENE_SHA,
              "the scene bytes are the pin")
    with open(os.path.join(os.path.dirname(scene), "LICENSE"), encoding="utf-8") as handle:
        check("Apache License" in handle.read(200), "Apache-2.0 travels with the model")
    root = ET.parse(model_xml).getroot()
    acts = [a for a in root.iter("position")]
    check(len(acts) == 32 and all(a.get("joint") == a.get("name") for a in acts),
          "32 position servos, each named for its joint")

    section("open: fail fast on config, refuse a body it cannot name how to move")
    saved_env = os.environ.pop("SKYNET_MUJOCO_SCENE", None)
    try:
        check(MujocoBridge().open("not json") == rhal.SKR_EINVAL, "malformed config")
        check(MujocoBridge().open(json.dumps({"base": "legged"})) == rhal.SKR_EINVAL,
              "no scene and no SKYNET_MUJOCO_SCENE")
        check(MujocoBridge().open(config(scene="/nonexistent.xml")) == rhal.SKR_ENODEV,
              "an unreadable scene is ENODEV, not a body")
        bad = os.path.join("/tmp", "skynet-apollo-bad.xml")
        with open(bad, "w", encoding="utf-8") as handle:
            handle.write("not xml at all")
        check(MujocoBridge().open(config(scene=bad)) == rhal.SKR_EINVAL,
              "a scene MuJoCo will not compile")
        no_base = json.loads(config())
        del no_base["base"]
        check(MujocoBridge().open(json.dumps(no_base)) == rhal.SKR_EINVAL,
              "no base on a free-root body: how it moves is not in the file")
        check(MujocoBridge().open(config(base="submarine")) == rhal.SKR_EINVAL,
              "a base outside the six kinds")
        check(MujocoBridge().open(config(rate_hz=0)) == rhal.SKR_EINVAL,
              "a zero rate is a config error, not the default")
        check(MujocoBridge().open(config(keyframe="walk")) == rhal.SKR_EINVAL,
              "an unknown keyframe names what the scene has")
    finally:
        if saved_env is not None:
            os.environ["SKYNET_MUJOCO_SCENE"] = saved_env

    driver = MujocoBridge()
    check(driver.open(config()) == rhal.SKR_OK, "open() on the pinned scene")
    check(driver.open(config()) == rhal.SKR_OK, "a second open() reloads rather than "
          "refusing: scenes are cheap and exclusive opens are for hardware")

    section("describe: 32 hinges in actuator order, roled, ranged, unenveloped")
    descriptor = json.loads(driver.describe())
    check(descriptor["robot_slug"] == "apollo", "the slug is the robot")
    check(descriptor["dof"] == 32, "32 joints")
    check([j["name"] for j in descriptor["joints"]] == JOINTS, "named, in actuator order")
    check(all(j["type"] == "revolute" for j in descriptor["joints"]), "all hinges, limited")
    by_name = {j["name"]: j for j in descriptor["joints"]}
    check((by_name["torso_yaw"]["limits"]["position_min_rad"],
           by_name["torso_yaw"]["limits"]["position_max_rad"]) == (-0.829031, 0.829031),
          "torso travel is the file's")
    check((by_name["l_knee_fe"]["limits"]["position_min_rad"],
           by_name["l_knee_fe"]["limits"]["position_max_rad"]) == (0, 2.61799),
          "the knee stops at straight, from the file")
    check(by_name["l_hip_aa"]["limits"]["effort_max_nm"] == 494,
          "and the hip's force ceiling is the servo's forcerange, not a rating")
    check(all("position_min_rad" in j["limits"] for j in descriptor["joints"]),
          "every joint carries travel")
    check(all("effort_max_nm" in j["limits"] for j in descriptor["joints"]),
          "every joint carries the force its servo applies")
    check([j["role"] for j in descriptor["joints"]] == [roles[n] for n in JOINTS],
          "roled by the operator's mapping, 32 of 32")
    check(all(j["control_modes"] == ["position"] for j in descriptor["joints"]),
          "position servos, every one")
    check(descriptor["base"] == "legged", "the base is legged, declared by the operator")
    check(descriptor["envelope"] is None, "and no envelope: a scene mints no safety figure")
    check(descriptor["simulator"] is True, "simulator: true rides every result")
    check(descriptor["verified"] is False and descriptor["digest"] is None,
          "unsigned, and says so")
    check(descriptor["control"]["modes"] == ["position"], "one control mode")
    check(descriptor["control"]["rate_hz_max"] == descriptor["control"]["rate_hz_min"] == 100,
          "the suite's rate, min and max")
    check(descriptor["control"]["command_latency_ms"] == 10.0,
          "one period of latency, by construction")
    check(descriptor["capabilities"] == ["safety.estop"], "the software stop it has")
    check(descriptor["end_effectors"] == [], "no declared hands: the ability_hand meshes "
          "are kinematics, not a declared effector")
    check(descriptor["provenance"]["scene"] == "scene.xml", "the scene is cited")
    check(descriptor["provenance"]["scene_sha256"] == SCENE_SHA, "by its hash")
    check(descriptor["provenance"]["driver_ceiling"] == "guest", "guest, twice over")
    served_cams = {s["name"] for s in descriptor["sensors"]}
    check(served_cams <= CAMERAS, f"cameras are the file's three, or none: {sorted(served_cams)}")
    check(all(s["modality"] == "rgb" and s["fps"] is None for s in descriptor["sensors"]),
          "rgb, with no frame rate claimed")
    renderable = len(served_cams) == 3
    if not renderable:
        print("  ..    no GL context: the camera checks below are not run, not passed",
              flush=True)

    section("read_state: the reader moves the clock")
    out = rhal.State()
    check(driver.read_state(out, 0) == rhal.SKR_OK, "the first read answers")
    check(len(out.position_rad) == 32 and all(math.isfinite(p) for p in out.position_rad),
          "32 finite positions")
    check(all(math.isfinite(v) for v in out.velocity_rad_s), "32 finite velocities")
    check(all(math.isfinite(e) for e in out.effort_nm), "32 finite efforts: what the "
          "servos applied, not what was asked")
    check(out.seq == 1 and out.stamp_ns == 10_000_000, "seq 1 at exactly one period")
    driver.read_state(out, 0)
    check(out.seq == 2 and out.stamp_ns == 20_000_000, "seq 2 at exactly two")
    check(out.flags & rhal.HOMING_NOT_REQUIRED and not (out.flags & rhal.FLAG_ESTOP),
          "homing not required, no stop latched")
    check(out.fault_code == 0, "no fault")
    check(len(out.base_pose) == 7 and all(math.isfinite(p) for p in out.base_pose),
          "a 7-scalar base pose, ground truth and labelled as such")
    check(0.5 < out.base_pose[2] < 1.5, "standing height, not the floor and not the sky")
    check(len(out.base_twist) == 6, "a 6-scalar base twist off the free joint")

    section("commands: full-vector position, held masks, refused modes")
    cmd = rhal.Command()
    cmd.mode = rhal.MODE_POSITION
    cmd.values = list(out.position_rad)
    cmd.mask = [1] * 32
    check(driver.write_command(cmd, 0) == rhal.SKR_OK, "a full-vector position command")
    cmd.mode = rhal.MODE_VELOCITY
    check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL, "velocity is refused, not "
          "approximated onto position servos")
    cmd.mode = rhal.MODE_TORQUE
    check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL, "torque likewise")
    cmd.mode = rhal.MODE_POSITION
    cmd.values = cmd.values[:31]
    check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL, "a short vector")
    cmd.values = list(out.position_rad)
    cmd.mask = [1] * 31
    check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL, "a short mask")
    cmd.mask = [1] * 32
    cmd.values[0] = float("nan")
    check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL, "a non-finite setpoint")
    check(MujocoBridge().write_command(cmd, 0) == rhal.SKR_EPERM, "a command before open")
    driver.close()
    driver = MujocoBridge()
    driver.open(config())
    for _ in range(20):
        driver.read_state(out, 0)
    cmd.values = list(out.position_rad)
    held_before = list(out.position_rad)
    cmd.mask = [0] * 32
    cmd.mask[ELBOW] = 1
    cmd.values[ELBOW] = held_before[ELBOW] - 0.15
    check(driver.write_command(cmd, 0) == rhal.SKR_OK, "one joint commanded, 31 masked")
    for _ in range(10):
        driver.read_state(out, 0)
    held = [abs(a - b) for a, b in zip(out.position_rad, held_before)]
    # Selectivity, not stillness: masked servos hold their setpoint and the loaded knees
    # still sag under gravity (PD tracking error, tens of mrad), so the assertion is that
    # the commanded joint moves clearly more than the worst uncommanded drift. Deterministic
    # sim, same steps every run: 94 mrad against 44 on MuJoCo 3.13.
    check(all(h < 0.06 for i, h in enumerate(held) if i != ELBOW),
          "masked joints drift less than 60 mrad over ten cycles")
    check(abs(out.position_rad[ELBOW] - held_before[ELBOW]) > 0.06,
          "the commanded joint moves more than any masked one")

    section("stand, then the motion proof")
    driver.close()
    driver = MujocoBridge()
    check(driver.open(config()) == rhal.SKR_OK, "reopen onto the stand keyframe")
    driver.read_state(out, 0)
    import mujoco

    model = mujoco.MjModel.from_xml_path(scene)
    stand = [float(v) for v in model.key_qpos[0][7:]]
    check(all(abs(a - b) < 1e-2 for a, b in zip(out.position_rad, stand)),
          "the first read is the stand, to 10 mrad (one step of contact settle)")
    lo = by_name["l_elbow_fe"]["limits"]["position_min_rad"]
    target = stand[ELBOW] - 0.3
    target = target if target > lo + 0.05 else stand[ELBOW] + 0.3
    cmd = rhal.Command()
    cmd.mode = rhal.MODE_POSITION
    cmd.values = list(stand)
    cmd.values[ELBOW] = target
    cmd.mask = [1] * 32
    check(driver.write_command(cmd, 0) == rhal.SKR_OK, "the elbow commanded 0.3 rad off")
    lo_all = [by_name[n]["limits"]["position_min_rad"] for n in JOINTS]
    hi_all = [by_name[n]["limits"]["position_max_rad"] for n in JOINTS]
    inside = True
    for _ in range(100):
        driver.read_state(out, 0)
        inside = inside and all(lo - 1e-6 <= p <= hi + 1e-6
                               for p, lo, hi in zip(out.position_rad, lo_all, hi_all))
    check(abs(out.position_rad[ELBOW] - stand[ELBOW]) > 0.2, "the elbow travelled")
    check(inside, "no joint left the file's travel doing it")

    if renderable:
        section("read_sensor: the torso cameras render")
        frame = rhal.Frame()
        check(driver.read_sensor("torso_oak_d_pro_w_front", frame) == rhal.SKR_OK,
              "the front camera renders")
        check(frame.width > 0 and frame.height > 0 and len(frame.data) > 0,
              "into nonzero pixels")
        check(driver.read_sensor("no_such_camera", rhal.Frame()) == rhal.SKR_ENODEV,
              "an unknown camera is ENODEV")

    section("emergency_stop: latch, refuse, recover in a new instance")
    check(driver.emergency_stop() == rhal.SKR_OK, "the stop latches")
    check(driver.write_command(cmd, 0) == rhal.SKR_EPERM, "later commands are refused")
    driver.read_state(out, 0)
    check((out.flags & rhal.FLAG_ESTOP) != 0, "later reads carry the flag")
    check(driver.close() == rhal.SKR_OK, "close after stop is quiet")
    risen = MujocoBridge()
    check(risen.open(config()) == rhal.SKR_OK, "a new instance recovers")
    risen.read_state(out, 0)
    check(not (out.flags & rhal.FLAG_ESTOP), "and reads no flag")
    risen.close()
    driver.close()

    section("the pipe half: python-host's verbs against a subprocess")
    env = dict(os.environ)
    env["PYTHONPATH"] = (os.path.join(HERE, "..", "..", "bindings", "python") +
                         os.pathsep + os.path.join(HERE, "..", "mujoco") +
                         os.pathsep + env.get("PYTHONPATH", ""))
    env["SKYNET_MUJOCO_SCENE"] = scene
    child = subprocess.Popen(
        [sys.executable, "-m", "skynet_mujoco"], stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1,
        env=env, cwd=HERE)

    def recv() -> str:
        assert child.stdout is not None
        line = child.stdout.readline()
        if not line:
            raise RuntimeError("the child went quiet")
        return line.rstrip("\n")

    hello = recv()
    check(hello.startswith("H ") and "mujoco-bridge" in hello,
          f"the child announces itself: {hello}")
    body = json.dumps({"base": "legged", "robot_slug": "apollo", "keyframe": "stand",
                       "rate_hz": 100, "role_overrides": roles})
    assert child.stdin is not None
    child.stdin.write(f"O {len(body)}\n{body}\n")
    child.stdin.flush()
    check(recv() == "R 0", "O opens through the pipe, scene from the environment")
    child.stdin.write("D\n")
    child.stdin.flush()
    answer = recv()
    check(answer.startswith("D "), "D describes")
    described = child.stdout.read(int(answer.split()[1]))
    child.stdout.readline()
    check(json.loads(described)["dof"] == 32, "32 joints over the pipe")
    child.stdin.write("S 0\n")
    child.stdin.flush()
    fields = recv().split()
    check(fields[0] == "S" and fields[1] == "0" and fields[6] == "32", "S reads 32 joints")
    child.stdin.write("T vendor-token c3RhbmQ=\n")
    child.stdin.flush()
    parts = recv().split()
    check(parts[0] == "R" and parts[1] != "0", "T is refused: this driver has no extensions")
    if len(parts) == 3:
        recv()
    child.stdin.write("X\n")
    child.stdin.flush()
    child.wait(timeout=15)
    check(child.returncode == 0, "X closes the child cleanly")
    child.stderr.close()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed", flush=True)
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())

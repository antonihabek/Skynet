"""skynet-apollo's layout check: the served descriptor against the pinned MJCF.

Both sides live, neither trusted: the file is parsed with the standard library and the
descriptor is served by a fresh bridge, and the two must agree joint for joint. A model
that drifted without the pin moving fails here rather than in the suite's hard numbers.

The check this file exists for is the negative one: the file carries a forcerange on all
32 servos and the served envelope is still null, because a modeller's servo tuning is not
a torque rating and this row must never learn to cite it as one.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import xml.etree.ElementTree as ET

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "bindings", "python"))
sys.path.insert(0, os.path.join(HERE, "..", "mujoco"))

CHECKS = 0
FAILURES: list[str] = []


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    print(("  ok    " if held else "  FAIL  ") + what, flush=True)
    if not held:
        FAILURES.append(what)


def main() -> int:
    scene = os.environ.get("SCENE_PATH")
    if not scene or not os.path.isfile(scene):
        print("::error::no SCENE_PATH. run-check.sh sets it to the verified pin.")
        return 1
    import skynet_rhal as rhal
    from skynet_mujoco.bridge import MujocoBridge

    model_xml = os.path.join(os.path.dirname(scene), "apptronik_apollo.xml")
    root = ET.parse(model_xml).getroot()
    with open(os.path.join(HERE, "roles.json"), encoding="utf-8") as handle:
        roles = json.load(handle)

    driver = MujocoBridge()
    rc = driver.open(json.dumps({"scene": scene, "base": "legged", "robot_slug": "apollo",
                                 "keyframe": "stand", "rate_hz": 100,
                                 "role_overrides": roles}))
    if rc != rhal.SKR_OK:
        print("::error::the bridge would not open the pinned scene.")
        return 1
    served = json.loads(driver.describe())
    driver.close()

    file_acts = [(a.get("name"), a.get("joint")) for a in root.iter("position")]
    check(len(file_acts) == 32, "the file drives 32 position servos")
    check(all(n == j for n, j in file_acts), "each named for its joint")
    check([n for n, _ in file_acts] == [j["name"] for j in served["joints"]],
          "served joints are the file's actuators, in order")

    file_joints = {j.get("name"): j.get("range") for j in root.iter("joint")
                   if j.get("name")}
    served_joints = {j["name"]: j for j in served["joints"]}
    ranges_hold = True
    for name, span in file_joints.items():
        if name == "floating_base" or span is None or name not in served_joints:
            continue
        lo, hi = (float(v) for v in span.split())
        got = served_joints[name]["limits"]
        ranges_hold = ranges_hold and abs(got["position_min_rad"] - lo) < 1e-12 \
            and abs(got["position_max_rad"] - hi) < 1e-12
    check(ranges_hold, "served travel is the file's ranges, to 1e-12")

    forces_hold = True
    for actuator in root.iter("position"):
        span = actuator.get("forcerange")
        if span is None:
            forces_hold = False
            break
        lo, hi = (float(v) for v in span.split())
        forces_hold = forces_hold and abs(
            served_joints[actuator.get("joint")]["limits"]["effort_max_nm"]
            - min(abs(lo), abs(hi))) < 1e-9
    check(forces_hold, "served force ceilings are the file's forceranges")
    check(served["envelope"] is None, "and the envelope is null although all 32 carry "
          "one: servo tuning is not a torque rating")

    keys = list(root.iter("key"))
    check(len(keys) == 1 and keys[0].get("name") == "stand", "one keyframe, named stand")
    check(len(keys[0].get("qpos", "").split()) == 39, "39 qpos: 7 free, 32 hinge")
    check(len(keys[0].get("ctrl", "").split()) == 32, "32 ctrls")

    scene_root = ET.parse(scene).getroot()
    includes = [i.get("file") for i in scene_root.iter("include")]
    check(includes == ["apptronik_apollo.xml"], "the scene includes the model, only it")
    option = next(scene_root.iter("option"))
    check(option.get("timestep") == "0.005", "200 Hz physics under a 100 Hz control rate")

    free = [j.get("name") for j in root.iter("joint") if j.get("type") == "free"]
    check(free == ["floating_base"], "the file's root is free-jointed")
    check(served["base"] == "legged", "the served base is the operator's word, legged")

    file_cams = {c.get("name") for c in root.iter("camera")}
    served_cams = {s["name"] for s in served["sensors"]}
    check(len(file_cams) == 3, "the file carries three cameras")
    check(served_cams <= file_cams, "served cameras are the file's, or none")
    if not served_cams:
        check(bool(served["provenance"]["cameras"]), "no GL context is a note, not a gap")
    check([j["role"] for j in served["joints"]] == [roles[j["name"]] for j in served["joints"]],
          "32 served roles are the mapping file's")
    with open(scene, "rb") as handle:
        check(hashlib.sha256(handle.read()).hexdigest() == served["provenance"]["scene_sha256"],
              "the served scene hash is the scene's bytes")

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed", flush=True)
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())

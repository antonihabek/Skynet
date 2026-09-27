#!/usr/bin/env python3
"""Every public robot description, through the OS — DRY-COVERAGE-PLAN.md Phase 10.

For each entry in `catalogue.json` this runs the stages below and writes one row of
`reference/reports/fleet.json`. Every stage that runs executes the code a robot runs: the Rust URDF
importer, the MuJoCo driver behind the C host that `skynetd` would load, `skynet driver check`
through the real ABI, and `skynetk` turning §7's guards against a policy in another process. Nothing
is re-implemented here; this file only arranges the runs and reads what they print.

| Stage | What runs | What a pass means |
|---|---|---|
| `source` | the pinned file, fetched and hashed | the bytes are the ones the catalogue pins |
| `import` | `skynet descriptor from-urdf` (§63.4) | Skynet's own importer reads the maker's file |
| `body` | MuJoCo, and `body.py` for a URDF | the description is a body physics can step |
| `conformance` | `skynet driver check` through python-host | the sim half of §15, at guest, on that body |
| `loop` | `skynetk --source` + `skynetp`, then `skynet blackbox replay` | a policy pushing past every limit is clamped by the kernel, and the recording reproduces every verdict |
| `travel` | the same, asking past the *narrow* end of every asymmetric joint | the kernel holds both ends of travel — which it does not (F159), so this stage fails wherever a joint's travel is not symmetric |
| `transport` | `skynet transport` of a reference policy of the robot's class | a skill trained on another robot binds to this one (§6) — or the plan says which channel has no equivalent |

A stage that could not run for a reason that belongs to the file (a template, a joint with no name)
is `refused` with the importer's or MuJoCo's own sentence. A stage that did not run because an
earlier one failed is `not_run`, never `pass` — §15's standing rule, applied to a sweep.

**What none of it is.** It is not a driver test: every body here is the MuJoCo driver, and the
robot's own wire is untouched. `drivers/models.json` is where a driver's stage lives, and this report
reads it rather than adding to it. A robot that passes every stage is a robot whose *description*
the OS accepts, bounds and replays — which is the precondition for a driver being worth writing,
and says nothing about whether one exists.

    python3 fleet/sweep.py                      # the whole catalogue, into reference/reports/fleet.json
    python3 fleet/sweep.py --only go2_description,panda_mj_description --out /tmp/fleet.json
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime
import hashlib
import importlib
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from typing import Any

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import robots as robots_mod  # noqa: E402

ANSI = re.compile(r"\x1b\[[0-9;]*m")
LOOP_HZ = 100
LOOP_CYCLES = 300


def sha256_of(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def run(argv: list[str], timeout: float, env: dict[str, str] | None = None, cwd: str | None = None) -> tuple[int, str]:
    """Run a command, return its exit status and its combined output with colour stripped."""
    try:
        done = subprocess.run(
            argv, capture_output=True, text=True, timeout=timeout, env=env, cwd=cwd, errors="replace"
        )
    except subprocess.TimeoutExpired as expired:
        out = (expired.stdout or "") + (expired.stderr or "")
        if isinstance(out, bytes):
            out = out.decode("utf-8", "replace")
        return 124, ANSI.sub("", out) + f"\n[timed out after {timeout:.0f} s]"
    return done.returncode, ANSI.sub("", (done.stdout or "") + (done.stderr or ""))


# ── the source ───────────────────────────────────────────────────────────────


def _menagerie_root() -> str:
    module = importlib.import_module("robot_descriptions.panda_mj_description")
    return module.REPOSITORY_PATH


def resolve(entry: dict[str, Any]) -> dict[str, Any]:
    """The file an entry names, on disk, and whether its bytes are the pinned ones."""
    kind = entry["source"]["kind"]
    if kind == "robot_descriptions":
        module = importlib.import_module(f"robot_descriptions.{entry['source']['module']}")
        root = module.REPOSITORY_PATH
        if entry["source"]["format"] == "mjcf":
            path = module.MJCF_PATH
        elif hasattr(module, "URDF_PATH"):
            path = module.URDF_PATH
        else:
            from robot_descriptions._xacro import get_urdf_path

            path = get_urdf_path(module)
        pinned_file = os.path.join(root, entry["source"]["file"])
    elif kind == "menagerie":
        root = _menagerie_root()
        path = os.path.join(root, entry["source"]["file"])
        pinned_file = path
    else:  # this repository
        root = os.path.join(REPO, "drivers", "ros2_control")
        path = os.path.join(REPO, entry["source"]["file"])
        pinned_file = path
    if not os.path.isfile(pinned_file):
        return {"outcome": "fail", "detail": f"{entry['source']['file']} is not in the checkout"}
    got = sha256_of(pinned_file)
    if got != entry["source"]["sha256"]:
        return {
            "outcome": "fail",
            "detail": f"sha256 {got} where the catalogue pins {entry['source']['sha256']}: the file drifted",
        }
    return {"outcome": "pass", "path": path, "root": root}


# ── the stages ───────────────────────────────────────────────────────────────


def stage_import(entry: dict[str, Any], urdf: str, work: str, skynet: str) -> dict[str, Any]:
    """`skynet descriptor from-urdf`, the way an integrator would run it on this robot."""
    base = robots_mod.BASE_BY_CLASS[entry["class"]]
    out = os.path.join(work, "import.json")
    argv = [skynet, "descriptor", "from-urdf", urdf, "--base", base, "--accept-suggested-roles", "-o", out]
    # A single arm's file has no reason to say which side it is on, and §63.4's answer is that the
    # operator asserts it. The sweep asserts `right`, as the specification's own example does, and
    # every role it produces that way is `inferred` in the descriptor rather than `named`.
    if entry["class"] == "arm":
        argv += ["--assume-side", "right"]
    status, text = run(argv, timeout=120)
    if status != 0:
        reason = next(
            (line.strip() for line in text.splitlines() if line.strip().startswith("error")), text.strip()[:300]
        )
        # The importer's sentence continues over indented lines; carry the whole of it.
        lines = text.splitlines()
        for index, line in enumerate(lines):
            if line.strip().startswith("error"):
                tail = [line.strip()]
                for follow in lines[index + 1 :]:
                    if not follow.startswith("    "):
                        break
                    tail.append(follow.strip())
                reason = " ".join(tail)
                break
        return {"outcome": "refused", "detail": reason}
    descriptor = json.load(open(out, encoding="utf-8"))
    joints = descriptor.get("joints") or []
    roles = {joint["name"]: joint["role"] for joint in joints if joint.get("role")}
    return {
        "outcome": "pass",
        "joints": len(joints),
        "dof": descriptor.get("dof"),
        "roles": len(roles),
        "frames": bool(descriptor.get("frames")),
        "_roles": roles,
    }


def stage_body(entry: dict[str, Any], source: dict[str, Any], work: str) -> dict[str, Any]:
    """A scene the MuJoCo driver can open: the MJCF itself, or one `body.py` writes for a URDF."""
    import mujoco

    path = source["path"]
    if entry["source"]["format"] == "mjcf":
        # The menagerie's convention: `scene.xml` beside the robot includes it and adds a floor, and
        # a free-rooted body with no floor under it falls for ever. Used when it includes this file.
        scene = os.path.join(os.path.dirname(path), "scene.xml")
        chosen = path
        if os.path.isfile(scene):
            text = open(scene, encoding="utf-8", errors="replace").read()
            if f'file="{os.path.basename(path)}"' in text:
                chosen = scene
        try:
            model = mujoco.MjModel.from_xml_path(chosen)
        except Exception as error:  # noqa: BLE001
            return {"outcome": "fail", "detail": f"MuJoCo will not load it: {error}"}
        keyframes = [mujoco.mj_id2name(model, mujoco.mjtObj.mjOBJ_KEY, k) for k in range(model.nkey)]
        keyframe = next((k for k in ("home", "stand", "standing", "init") if k in keyframes), None)
        free_root = any(
            int(model.jnt_type[j]) == int(mujoco.mjtJoint.mjJNT_FREE) and int(model.body_parentid[model.jnt_bodyid[j]]) == 0
            for j in range(model.njnt)
        )
        return {
            "outcome": "pass",
            "scene": chosen,
            "generated": False,
            "actuators": int(model.nu),
            "keyframe": keyframe,
            "free_root": free_root,
            "_hold_gains": _torque_hold_gains(mujoco, model, keyframe),
        }
    import body

    scene = os.path.join(work, "body.xml")
    try:
        built = body.build_from_urdf(mujoco, path, source["root"], scene)
    except body.BuildError as error:
        # A fact about the file — MuJoCo's own sentence, or a body with nothing to drive — rather
        # than a fault in this run, so it is a refusal and the report names it as one.
        return {"outcome": "refused", "detail": str(error)}
    model = mujoco.MjModel.from_xml_path(scene)
    free_root = any(
        int(model.jnt_type[j]) == int(mujoco.mjtJoint.mjJNT_FREE) and int(model.body_parentid[model.jnt_bodyid[j]]) == 0
        for j in range(model.njnt)
    )
    return {
        "outcome": "pass",
        "scene": scene,
        "generated": True,
        # A URDF with a `floating` joint keeps it: MuJoCo makes it free, and the driver then wants
        # the operator's word on the base (F164's converse), which the class supplies.
        "free_root": free_root,
        "actuators": len(built["actuated"]),
        "skipped": built["skipped"],
        "notes": built["notes"],
        "keyframe": None,
    }


def _torque_hold_gains(mujoco: Any, model: Any, keyframe: str | None) -> dict[str, dict[str, float]]:
    """Hold gains for every `<motor>` joint, from the model's own mass matrix — `body.py`'s rule.

    §25 holds a masked torque joint at a stiffness, and the stiffness is the operator's: the MuJoCo
    driver refuses a torque hold without one (F162). The sweep is the operator here, and it chooses
    the way it chooses a URDF's servos — critically damped at `body.SERVO_HZ` against the inertia each
    joint sees at the model's own stand pose — and the report says so on every row that used them.
    """
    import math

    import numpy

    import body

    data = mujoco.MjData(model)
    if keyframe is not None:
        mujoco.mj_resetDataKeyframe(model, data, mujoco.mj_name2id(model, mujoco.mjtObj.mjOBJ_KEY, keyframe))
    mujoco.mj_forward(model, data)
    dense = numpy.zeros((model.nv, model.nv))
    try:
        mujoco.mj_fullM(model, data, dense)
    except TypeError:
        mujoco.mj_fullM(model, dense, data.qM)
    omega = 2.0 * math.pi * body.SERVO_HZ
    gains: dict[str, dict[str, float]] = {}
    for index in range(model.nu):
        if int(model.actuator_biastype[index]) != 0 or int(model.actuator_dyntype[index]) != 0:
            continue
        if int(model.actuator_trntype[index]) not in (0, 1):
            continue
        joint = int(model.actuator_trnid[index][0])
        if int(model.jnt_type[joint]) not in (int(mujoco.mjtJoint.mjJNT_HINGE), int(mujoco.mjtJoint.mjJNT_SLIDE)):
            continue
        dof = int(model.jnt_dofadr[joint])
        inertia = max(float(dense[dof, dof]), 1e-6)
        name = mujoco.mj_id2name(model, mujoco.mjtObj.mjOBJ_JOINT, joint)
        gains[name] = {"kp": round(inertia * omega * omega, 6), "kd": round(2.0 * inertia * omega, 6)}
    return gains


def _driver_env(scene: str) -> dict[str, str]:
    env = dict(os.environ)
    env.update(
        {
            "SKYNET_PYTHON": sys.executable,
            "SKYNET_PYTHON_DRIVER": "skynet_mujoco",
            "SKYNET_MUJOCO_SCENE": scene,
            "PYTHONPATH": os.pathsep.join(
                [os.path.join(REPO, "bindings", "python"), os.path.join(REPO, "drivers", "mujoco")]
            ),
        }
    )
    return env


def _driver_config(entry: dict[str, Any], body_result: dict[str, Any], roles: dict[str, str]) -> str:
    config: dict[str, Any] = {}
    # A base is the operator's word about a root that moves, and only such a root takes one (F164).
    if body_result.get("free_root"):
        config["base"] = robots_mod.BASE_BY_CLASS[entry["class"]]
    if body_result.get("_hold_gains"):
        config["hold_gains"] = body_result["_hold_gains"]
    if body_result.get("keyframe"):
        config["keyframe"] = body_result["keyframe"]
    if roles:
        config["role_overrides"] = roles
    return json.dumps(config, sort_keys=True)


SUMMARY = re.compile(r"(\d+) passed, (\d+) failed(?:, (\d+) not measured)?(?:, (\d+) not applicable)?")


def stage_conformance(
    entry: dict[str, Any], body_result: dict[str, Any], roles: dict[str, str], work: str, skynet: str, host: str
) -> dict[str, Any]:
    """The sim half of §15, through the C host `skynetd` would load, against this body."""
    results = os.path.join(work, "results.json")
    descriptor = os.path.join(work, "descriptor.json")
    config = _driver_config(entry, body_result, roles)
    status, text = run(
        [skynet, "driver", "check", host, "--config", config, "--results", results, "--descriptor", descriptor],
        timeout=300,
        env=_driver_env(body_result["scene"]),
    )
    found = SUMMARY.search(text)
    failures: list[dict[str, str]] = []
    lines = text.splitlines()
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("FAIL"):
            parts = stripped.split(None, 2)
            detail = []
            for follow in lines[index + 1 :]:
                if not follow.startswith("          "):
                    break
                detail.append(follow.strip())
            failures.append(
                {
                    "test": parts[1] if len(parts) > 1 else "?",
                    "assertion": parts[2] if len(parts) > 2 else "",
                    "detail": " ".join(detail)[:400],
                }
            )
    outcome = "guest" if status == 0 else "none"
    if not found:
        # The driver would not open this body — the scene's actuators are all tendons or sites,
        # say — and its own sentence is the detail. A body the RHAL cannot carry is a refusal about
        # the body, not a failed check.
        refusal = next(
            (line.strip() for line in text.splitlines() if "skynet-mujoco:" in line and ("refusing" in line or "no actuator" in line)),
            None,
        )
        if refusal:
            return {"outcome": "none", "detail": refusal[:500], "passed": 0, "failed": 0, "failures": []}
        return {"outcome": "fail", "detail": text.strip()[-600:], "config": json.loads(config)}
    row: dict[str, Any] = {
        "outcome": outcome,
        "passed": int(found.group(1)),
        "failed": int(found.group(2)),
        "not_measured": int(found.group(3) or 0),
        "not_applicable": int(found.group(4) or 0),
        "failures": failures,
    }
    if os.path.isfile(descriptor):
        described = json.load(open(descriptor, encoding="utf-8"))
        joints = described.get("joints") or []
        modes = sorted({mode for joint in joints for mode in (joint.get("control_modes") or [])})
        row["joints"] = len(joints)
        row["modes"] = modes
        row["roles"] = sum(1 for joint in joints if joint.get("role"))
        row["_descriptor"] = descriptor
    return row


LOOP_LINE = re.compile(r"^(cycles|interventions|idle|read failures|write failures|overruns|stops)\s+(\d+)", re.M)
BOX_LINE = re.compile(r"^\s+(guard trips|clamps|estops)\s+(\d+)", re.M)


def _policy(descriptor: dict[str, Any], mode: str) -> dict[str, Any]:
    """A model that asks every joint for more than the body allows, whatever it reads.

    Position: past the upper end of the declared travel. Torque: twice the declared ceiling.
    Velocity: past the declared speed, or one radian a second where none is declared. The weights
    are zero, so the model is its bias — the same request every cycle, and every one of them over a
    limit the kernel was given.
    """
    targets = []
    for joint in descriptor["joints"]:
        limits = joint.get("limits") or {}
        mode = (joint.get("control_modes") or [mode])[0]
        if mode == "position":
            high, low = limits.get("position_max_rad"), limits.get("position_min_rad")
            if high is not None and low is not None:
                targets.append(float(high) + max(0.25 * (float(high) - float(low)), 0.1))
            else:
                targets.append(0.3)
        elif mode == "torque":
            effort = limits.get("effort_max_nm")
            targets.append(2.0 * float(effort) if effort else 1.0)
        else:
            speed = limits.get("velocity_max_rad_s")
            targets.append(1.5 * float(speed) if speed else 1.0)
    count = len(targets)
    return {
        "kind": "mlp",
        "version": 1,
        "inputs": {"channels": count, "reads": ["position"]},
        "layers": [{"activation": "identity", "weights": [[0.0] * count for _ in range(count)], "bias": targets}],
    }


def _direct_source(descriptor: dict[str, Any], mode: str) -> dict[str, Any]:
    """A policy trained on this very body: one channel per joint, by exact name, in joint order.

    Each channel commands its joint in the joint's own mode, so on a body whose joints sit in two
    modes the plan binds each in its own (§86.2) and `skynetk` sends the per-joint array (§86.1).
    The plan it resolves to is every joint `direct`, and its clamp carries each joint's rating in
    that joint's mode — the per-channel ceilings `skynetk --source` hands the envelope guard (F158).
    """
    return {
        "robot_slug": descriptor.get("robot_slug") or "fleet",
        "dof": len(descriptor["joints"]),
        "control_mode": mode,
        "control_hz": LOOP_HZ,
        "action_channels": [
            {"index": index, "kind": f"joint_{(joint.get('control_modes') or [mode])[0]}", "joint": joint["name"]}
            for index, joint in enumerate(descriptor["joints"])
        ],
        "observations": [],
    }


def _close_loop(
    tag: str, entry: dict[str, Any], body_result: dict[str, Any], roles: dict[str, str], work: str,
    tools: dict[str, str], source: str, model: dict[str, Any], mode: str, kernel_extra: list[str],
) -> tuple[dict[str, Any], list[str]]:
    """One run of `skynetk` and `skynetp` across the real ring, then the replay of its window."""
    model_path = os.path.join(work, f"{tag}.policy.json")
    json.dump(model, open(model_path, "w", encoding="utf-8"))
    socket_path = os.path.join(work, f"{tag}.sock")
    box = os.path.join(work, f"{tag}.box")
    os.makedirs(box, exist_ok=True)
    log_path = os.path.join(work, f"{tag}.skynetk.txt")
    kernel_log = open(log_path, "w", encoding="utf-8")
    kernel = subprocess.Popen(
        [
            tools["skynetk"], "run", "--driver", tools["host"], "--config", _driver_config(entry, body_result, roles),
            "--hz", str(LOOP_HZ), "--cycles", str(LOOP_CYCLES + 100), "--policy-socket", socket_path,
            "--blackbox", box, "--source", source, "--watchdog-ms", "200", *kernel_extra,
        ],
        stdout=kernel_log, stderr=subprocess.STDOUT, env=_driver_env(body_result["scene"]),
    )
    deadline = time.monotonic() + 120
    while not os.path.exists(socket_path):
        if kernel.poll() is not None or time.monotonic() > deadline:
            break
        time.sleep(0.05)
    _, policy_text = run(
        [
            tools["skynetp"], "run", "--hz", str(LOOP_HZ), "--cycles", str(LOOP_CYCLES), "--channels",
            str(len(model["layers"][0]["bias"])), "--mode", mode, "--backend", "inference", "--policy",
            model_path, "--ring-socket", socket_path,
        ],
        timeout=180,
    )
    try:
        kernel.wait(timeout=180)
    except subprocess.TimeoutExpired:
        kernel.send_signal(signal.SIGINT)
        kernel.wait(timeout=30)
    kernel_log.close()
    kernel_text = ANSI.sub("", open(log_path, encoding="utf-8", errors="replace").read())
    row: dict[str, Any] = {key.replace(" ", "_"): int(value) for key, value in LOOP_LINE.findall(kernel_text)}
    commanded = re.search(r"the model commanded on (\d+) of (\d+)", policy_text)
    row["policy_commanded"] = int(commanded.group(1)) if commanded else 0
    plan_line = re.search(r"^plan\s+(.+)$", kernel_text, re.M)
    problems: list[str] = []
    diverged = kernel_text.count("the simulation diverged")
    explained = kernel.returncode == 1 and re.search(r"^read failures\s+(\d+)", kernel_text, re.M) and int(
        re.search(r"^read failures\s+(\d+)", kernel_text, re.M).group(1)
    ) <= diverged and "write failures  0" in kernel_text
    if kernel.returncode not in (0, None) and not explained:
        problems.append(f"skynetk exited {kernel.returncode}: " + kernel_text.strip()[-400:])
    if not plan_line:
        problems.append("skynetk resolved no plan from --source")
    if not row.get("cycles"):
        problems.append("skynetk reported no cycles")
    if row["policy_commanded"] == 0:
        problems.append("the policy commanded nothing: " + policy_text.strip()[-300:])
    # A policy driving a floating body to every limit can make the physics diverge, and the driver
    # reports each such step as a failed read (§26.4) rather than as a state. That is the driver
    # doing its job under a policy no robot would run; it is counted, and it is not the kernel's.
    row["diverged"] = kernel_text.count("the simulation diverged")
    unexplained_reads = max(0, row.get("read_failures", 0) - row["diverged"])
    if unexplained_reads or row.get("write_failures"):
        first = re.search(r"^write failures\s+\d+ — (.+)$", kernel_text, re.M)
        problems.append(
            f"{row.get('read_failures', 0)} read and {row.get('write_failures', 0)} write failure(s)"
            + (f"; {first.group(1)}" if first else "")
        )
    sealed = re.search(r"^sealed\s+(\S+)", kernel_text, re.M)
    if sealed:
        _, box_text = run([tools["skynetk"], "blackbox", box], timeout=60)
        row.update({key.replace(" ", "_"): int(value) for key, value in BOX_LINE.findall(box_text)})
        _, replay_text = run([tools["skynet"], "blackbox", "replay", sealed.group(1)], timeout=120)
        reproduced = re.search(r"all (\d+) evaluated cycles reproduced", replay_text)
        row["replayed"] = int(reproduced.group(1)) if reproduced else 0
        row["complete"] = "every field this replay evaluates was recorded on every cycle" in replay_text
        if not reproduced:
            problems.append("the replay did not reproduce every verdict: " + replay_text.strip()[-300:])
        if not row["complete"]:
            problems.append("the replay would not say the window was complete")
    else:
        problems.append("skynetk sealed no window")
    return row, problems


def _single_mode(descriptor: dict[str, Any]) -> str | None:
    modes = {tuple(joint.get("control_modes") or []) for joint in descriptor["joints"]}
    if len(modes) != 1:
        return None
    only = next(iter(modes))
    return only[0] if len(only) == 1 else None


def stage_loop(
    entry: dict[str, Any], body_result: dict[str, Any], conformance: dict[str, Any], roles: dict[str, str],
    work: str, tools: dict[str, str],
) -> dict[str, Any]:
    """A policy asking every joint for more than the body allows, through the kernel, replayed.

    Position: a quarter of the travel past the upper end. Torque: twice the rating. Velocity: half
    again past the rating. With the plan in force every one of those is over a ceiling the kernel
    holds, so a run with no clamp is a kernel that let a declared limit through.
    """
    descriptor = json.load(open(conformance["_descriptor"], encoding="utf-8"))
    mode = _single_mode(descriptor)
    mixed = False
    if mode is None:
        # Joints in two modes, each in exactly one: the plan binds each in its own and `skynetk`
        # sends §86.1's per-joint array, so the frame's own mode only names what inherits.
        if not all(len(joint.get("control_modes") or []) == 1 for joint in descriptor["joints"]):
            return {"outcome": "not_run", "detail": "a joint offers more than one mode, and the sweep does not choose for it"}
        mode, mixed = descriptor["joints"][0]["control_modes"][0], True
    source = os.path.join(work, "source.json")
    json.dump(_direct_source(descriptor, mode), open(source, "w", encoding="utf-8"))
    row, problems = _close_loop(
        "loop", entry, body_result, roles, work, tools, source, _policy(descriptor, mode), mode, ["--jerk", "0.2"]
    )
    row = {"mode": "per joint" if mixed else mode, **row}
    # Only a rating in the commanded mode's own units bounds the command: a travel range says
    # nothing about a torque. A body with none in its mode cannot be clamped, and says so.
    units = {"position": "position_max_rad", "torque": "effort_max_nm", "velocity": "velocity_max_rad_s"}
    rated = sum(
        1
        for joint in descriptor["joints"]
        if (joint.get("limits") or {}).get(units[(joint.get("control_modes") or [mode])[0]]) is not None
    )
    row["rated_joints"] = rated
    if rated and not row.get("clamps"):
        problems.append("a policy asking past every declared limit was never clamped")
    if not rated:
        row["note"] = "no joint declares a rating in its own mode's units, so nothing here could be clamped"
    row["outcome"] = "fail" if problems else "pass"
    if problems:
        row["detail"] = "; ".join(problems)[:800]
    return row


def stage_travel(
    entry: dict[str, Any], body_result: dict[str, Any], conformance: dict[str, Any], roles: dict[str, str],
    work: str, tools: dict[str, str],
) -> dict[str, Any]:
    """Both ends of travel. F159: the kernel holds a position channel to one magnitude.

    A joint whose travel is not symmetric about zero has a narrow end, and a command past it is as
    far outside the body's travel as one past the wide end. This asks every asymmetric joint for a
    tenth of its travel past its narrow end — and every symmetric joint for the middle of its travel,
    which is inside — with no rate limit, so a clamp in this run can only be the envelope guard
    catching a narrow end. It passes only when every one of those requests was clamped.
    """
    descriptor = json.load(open(conformance["_descriptor"], encoding="utf-8"))
    if _single_mode(descriptor) != "position":
        return {"outcome": "not_applicable", "detail": "not a position body; travel is a position bound"}
    targets, narrow = [], []
    for joint in descriptor["joints"]:
        limits = joint.get("limits") or {}
        low, high = limits.get("position_min_rad"), limits.get("position_max_rad")
        if low is None or high is None:
            targets.append(0.0)
            continue
        low, high = float(low), float(high)
        margin = max(0.1 * (high - low), 0.05)
        if abs(abs(low) - abs(high)) < 1e-6:
            targets.append(0.5 * (low + high))
        elif abs(low) < abs(high):
            targets.append(low - margin)
            narrow.append(joint["name"])
        else:
            targets.append(high + margin)
            narrow.append(joint["name"])
    if not narrow:
        return {"outcome": "not_applicable", "detail": "every joint's travel is symmetric about zero"}
    count = len(targets)
    model = {
        "kind": "mlp",
        "version": 1,
        "inputs": {"channels": count, "reads": ["position"]},
        "layers": [{"activation": "identity", "weights": [[0.0] * count for _ in range(count)], "bias": targets}],
    }
    source = os.path.join(work, "source.json")
    json.dump(_direct_source(descriptor, "position"), open(source, "w", encoding="utf-8"))
    row, problems = _close_loop("travel", entry, body_result, roles, work, tools, source, model, "position", [])
    requested = len(narrow) * row.get("policy_commanded", 0)
    clamps = row.get("clamps", 0)
    result = {
        "asymmetric_joints": len(narrow),
        "past_the_narrow_end": requested,
        "clamped": clamps,
        "replayed": row.get("replayed", 0),
    }
    if problems:
        return {"outcome": "fail", "detail": "; ".join(problems)[:600], **result}
    if clamps < requested:
        return {
            "outcome": "fail",
            "detail": (
                f"{len(narrow)} joint(s) asked past the narrow end of their travel on "
                f"{row.get('policy_commanded', 0)} cycles; the kernel clamped {clamps} of {requested} (F159)"
            ),
            **result,
        }
    return {"outcome": "pass", **result}


# A policy trained on LeRobot's SO-101, the arm most of its community's datasets were recorded on,
# as the LeRobot bridge declares its roles (drivers/lerobot/README.md, §67.2). The gripper is
# optional: a policy that grasps still moves an arm with no hand fitted, and says so.
SO101_POLICY = {
    "robot_slug": "so-101 (lerobot)",
    "dof": 6,
    "control_mode": "position",
    "control_hz": 30,
    "action_channels": [
        {"index": 0, "kind": "joint_position", "joint": "shoulder_pan", "role": "right_arm.shoulder.yaw"},
        {"index": 1, "kind": "joint_position", "joint": "shoulder_lift", "role": "right_arm.shoulder.pitch"},
        {"index": 2, "kind": "joint_position", "joint": "elbow_flex", "role": "right_arm.elbow.flex"},
        {"index": 3, "kind": "joint_position", "joint": "wrist_flex", "role": "right_arm.wrist.flex"},
        {"index": 4, "kind": "joint_position", "joint": "wrist_roll", "role": "right_arm.wrist.roll"},
        {"index": 5, "kind": "joint_position", "joint": "gripper", "role": "right_hand.gripper.grip", "optional": True},
    ],
    "observations": [],
}

# A whole-body policy trained on Unitree's G1, with the roles Skynet's own importer suggests for its
# URDF — twelve leg joints, three waist, fourteen arm. Nothing is optional: a locomotion policy
# needs every leg joint it was trained with, and a plan that dropped one would not walk.
G1_JOINTS = [
    ("left_hip_pitch_joint", "left_leg.hip.pitch"), ("left_hip_roll_joint", "left_leg.hip.roll"),
    ("left_hip_yaw_joint", "left_leg.hip.yaw"), ("left_knee_joint", "left_leg.knee.pitch"),
    ("left_ankle_pitch_joint", "left_leg.ankle.pitch"), ("left_ankle_roll_joint", "left_leg.ankle.roll"),
    ("right_hip_pitch_joint", "right_leg.hip.pitch"), ("right_hip_roll_joint", "right_leg.hip.roll"),
    ("right_hip_yaw_joint", "right_leg.hip.yaw"), ("right_knee_joint", "right_leg.knee.pitch"),
    ("right_ankle_pitch_joint", "right_leg.ankle.pitch"), ("right_ankle_roll_joint", "right_leg.ankle.roll"),
    ("waist_yaw_joint", "torso.waist.yaw"), ("waist_roll_joint", "torso.waist.roll"),
    ("waist_pitch_joint", "torso.waist.pitch"),
    ("left_shoulder_pitch_joint", "left_arm.shoulder.pitch"), ("left_shoulder_roll_joint", "left_arm.shoulder.roll"),
    ("left_shoulder_yaw_joint", "left_arm.shoulder.yaw"), ("left_elbow_joint", "left_arm.elbow.pitch"),
    ("left_wrist_roll_joint", "left_arm.wrist.roll"), ("left_wrist_pitch_joint", "left_arm.wrist.pitch"),
    ("left_wrist_yaw_joint", "left_arm.wrist.yaw"),
    ("right_shoulder_pitch_joint", "right_arm.shoulder.pitch"), ("right_shoulder_roll_joint", "right_arm.shoulder.roll"),
    ("right_shoulder_yaw_joint", "right_arm.shoulder.yaw"), ("right_elbow_joint", "right_arm.elbow.pitch"),
    ("right_wrist_roll_joint", "right_arm.wrist.roll"), ("right_wrist_pitch_joint", "right_arm.wrist.pitch"),
    ("right_wrist_yaw_joint", "right_arm.wrist.yaw"),
]
G1_POLICY = {
    "robot_slug": "unitree-g1",
    "dof": len(G1_JOINTS),
    "control_mode": "position",
    "control_hz": 50,
    "action_channels": [
        {"index": index, "kind": "joint_position", "joint": joint, "role": role}
        for index, (joint, role) in enumerate(G1_JOINTS)
    ],
    "observations": [],
}

TRANSPORT_POLICIES = {
    "arm": ("LeRobot SO-101", SO101_POLICY),
    "humanoid": ("Unitree G1", G1_POLICY),
}


def stage_transport(entry: dict[str, Any], conformance: dict[str, Any], work: str, tools: dict[str, str]) -> dict[str, Any]:
    """Would a skill trained on another robot of this class run on this one? `skynet transport`.

    §6's planner, over the descriptor the driver served — so a role in it is one the importer
    suggested and the sweep accepted, or none. The answer is the plan's verdict and how each
    channel bound; `refused` is a real answer, and for most bodies it is the one this platform
    gives today (see the report's summary).
    """
    policy = TRANSPORT_POLICIES.get(entry["class"])
    if policy is None:
        return {"outcome": "not_applicable", "detail": f"no reference policy for class {entry['class']}"}
    name, source = policy
    path = os.path.join(work, "transport.source.json")
    json.dump(source, open(path, "w", encoding="utf-8"))
    status, text = run([tools["skynet"], "transport", path, "--robot", conformance["_descriptor"], "--json"], timeout=60)
    try:
        plan = json.loads(text[text.index("{"):])
    except (ValueError, json.JSONDecodeError):
        return {"outcome": "fail", "policy": name, "detail": text.strip()[-400:]}
    methods: dict[str, int] = {}
    for binding in plan.get("bindings") or []:
        methods[binding.get("method", "?")] = methods.get(binding.get("method", "?"), 0) + 1
    verdict = plan.get("verdict")
    return {
        "outcome": "refused" if verdict == "refused" else "bound",
        "policy": name,
        "verdict": verdict,
        "methods": dict(sorted(methods.items())),
    }


# ── one robot ───────────────────────────────────────────────────────────────


def sweep_one(entry: dict[str, Any], tools: dict[str, str], keep: str | None) -> dict[str, Any]:
    work = tempfile.mkdtemp(prefix="skynet-fleet-")
    stages: dict[str, Any] = {}
    try:
        source = resolve(entry)
        stages["source"] = {k: v for k, v in source.items() if k in ("outcome", "detail")}
        if source["outcome"] != "pass":
            for name in ("import", "body", "conformance", "loop", "travel", "transport"):
                stages[name] = {"outcome": "not_run"}
            return {"id": entry["id"], "stages": stages}

        roles: dict[str, str] = {}
        if entry["source"]["format"] in ("urdf", "xacro"):
            imported = stage_import(entry, source["path"], work, tools["skynet"])
            roles = imported.pop("_roles", {}) or {}
            stages["import"] = imported
        else:
            stages["import"] = {"outcome": "not_applicable", "detail": "an MJCF file; the importer reads URDF"}

        built = stage_body(entry, source, work)
        stages["body"] = {k: v for k, v in built.items() if k != "scene" and not k.startswith("_")}
        if built.get("_hold_gains"):
            stages["body"]["hold_gains"] = f"{len(built['_hold_gains'])} torque joint(s), from the model's mass matrix"
        if built["outcome"] != "pass":
            for name in ("conformance", "loop", "travel", "transport"):
                stages[name] = {"outcome": "not_run"}
            return {"id": entry["id"], "stages": stages}

        # Roles from the importer bind to the body only when the body carries the file's own joint
        # names — which a body generated from that URDF does, and an MJCF written separately does not.
        body_roles = roles if built.get("generated") else {}
        checked = stage_conformance(entry, built, body_roles, work, tools["skynet"], tools["host"])
        stages["conformance"] = {k: v for k, v in checked.items() if not k.startswith("_")}
        if "_descriptor" not in checked:
            for name in ("loop", "travel", "transport"):
                stages[name] = {"outcome": "not_run", "detail": "the driver never described the body"}
            return {"id": entry["id"], "stages": stages}
        stages["loop"] = stage_loop(entry, built, checked, body_roles, work, tools)
        stages["travel"] = stage_travel(entry, built, checked, body_roles, work, tools)
        stages["transport"] = stage_transport(entry, checked, work, tools)
        return {"id": entry["id"], "stages": stages}
    except Exception as error:  # noqa: BLE001 - one robot's crash is a row, not the sweep's end
        stages["error"] = {"outcome": "fail", "detail": f"{type(error).__name__}: {error}"[:600]}
        return {"id": entry["id"], "stages": stages}
    finally:
        if keep:
            target = os.path.join(keep, entry["id"].replace(":", "_").replace("/", "_"))
            shutil.rmtree(target, ignore_errors=True)
            shutil.copytree(work, target, ignore=shutil.ignore_patterns("ring", "*.sock"), dirs_exist_ok=True)
        shutil.rmtree(work, ignore_errors=True)


# ── the report ───────────────────────────────────────────────────────────────


def summarise(catalogue: list[dict[str, Any]], rows: list[dict[str, Any]]) -> dict[str, Any]:
    by_id = {row["id"]: row for row in rows}
    stage_names = ("source", "import", "body", "conformance", "loop", "travel", "transport")
    counts: dict[str, dict[str, int]] = {name: {} for name in stage_names}
    for row in rows:
        for name in stage_names:
            outcome = row["stages"].get(name, {}).get("outcome", "not_run")
            counts[name][outcome] = counts[name].get(outcome, 0) + 1
    robots = {(entry["maker"], entry["model"]) for entry in catalogue}
    through = {
        (entry["maker"], entry["model"])
        for entry in catalogue
        if by_id.get(entry["id"], {}).get("stages", {}).get("loop", {}).get("outcome") == "pass"
        and by_id.get(entry["id"], {}).get("stages", {}).get("conformance", {}).get("outcome") == "guest"
    }
    by_class: dict[str, dict[str, int]] = {}
    for entry in catalogue:
        cls = by_class.setdefault(entry["class"], {"descriptions": 0, "through": 0})
        cls["descriptions"] += 1
        stages = by_id.get(entry["id"], {}).get("stages", {})
        if stages.get("loop", {}).get("outcome") == "pass" and stages.get("conformance", {}).get("outcome") == "guest":
            cls["through"] += 1
    return {
        "descriptions": len(catalogue),
        "robots": len(robots),
        "makers": len({entry["maker"] for entry in catalogue}),
        "robots_through_every_stage": len(through),
        "stages": counts,
        "by_class": dict(sorted(by_class.items())),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--catalogue", default=os.path.join(HERE, "catalogue.json"))
    parser.add_argument("--out", default=os.path.join(REPO, "reference", "reports", "fleet.json"))
    parser.add_argument("--only", default="")
    parser.add_argument("--jobs", type=int, default=max(1, (os.cpu_count() or 2) - 1))
    parser.add_argument("--keep", default="", help="copy each robot's working directory here")
    parser.add_argument("--target", default=os.path.join(REPO, "target", "release"))
    parser.add_argument(
        "--compare",
        default="",
        help="a committed report: exit non-zero naming every row whose stage is worse tonight",
    )
    args = parser.parse_args()

    tools = {
        "skynet": os.path.join(args.target, "skynet"),
        "skynetk": os.path.join(args.target, "skynetk"),
        "skynetp": os.path.join(args.target, "skynetp"),
        "host": os.path.join(REPO, "drivers", "python-host", "libskynet_python_host.so"),
    }
    missing = [path for path in tools.values() if not os.path.isfile(path)]
    if missing:
        print(
            "fleet: these are not built: " + ", ".join(missing) + "\n  cargo build --release -p skynet-cli "
            "-p skynet-kernel-host -p skynet-policy-host && make -C drivers/python-host",
            file=sys.stderr,
        )
        return 2
    catalogue = json.load(open(args.catalogue, encoding="utf-8"))["entries"]
    wanted = [entry for entry in catalogue if not args.only or entry["id"] in args.only.split(",")]
    if args.keep:
        os.makedirs(args.keep, exist_ok=True)

    # Every module import happens once, here, so the workers find the clones in place rather than
    # racing each other to make them.
    for entry in wanted:
        if entry["source"]["kind"] == "robot_descriptions":
            importlib.import_module(f"robot_descriptions.{entry['source']['module']}")
    _menagerie_root()

    rows: list[dict[str, Any]] = []
    started = time.monotonic()
    with concurrent.futures.ProcessPoolExecutor(max_workers=args.jobs) as pool:
        futures = {pool.submit(sweep_one, entry, tools, args.keep or None): entry for entry in wanted}
        for future in concurrent.futures.as_completed(futures):
            row = future.result()
            rows.append(row)
            stages = row["stages"]
            print(
                f"{len(rows):4d}/{len(wanted)} {row['id']:48s} "
                + " ".join(f"{name}={stages.get(name, {}).get('outcome', '-')}" for name in ("import", "body", "conformance", "loop", "travel", "transport")),
                flush=True,
            )
    rows.sort(key=lambda row: row["id"])
    by_id = {entry["id"]: entry for entry in catalogue}
    for row in rows:
        entry = by_id[row["id"]]
        row.update({key: entry[key] for key in ("maker", "model", "class", "route")})
    import mujoco
    import robot_descriptions

    report = {
        "note": (
            "Written by fleet/sweep.py (DRY-COVERAGE-PLAN.md Phase 10). Each row is one public robot description "
            "through Skynet's own importer, the MuJoCo driver behind the C host, `skynet driver check`, and "
            "`skynetk` with a policy that asks past every limit, replayed. A description is not a robot and the "
            "MuJoCo driver is not the robot's driver: `route` names the driver in this tree that speaks the "
            "robot's own interface, and its stage is in drivers/models.json, not here. No row is evidence about "
            "a machine; every body declares simulator: true."
        ),
        "environment": {
            "mujoco": mujoco.__version__,
            "robot_descriptions": robot_descriptions.__version__,
            "skynet": run(["git", "-C", REPO, "rev-parse", "--short", "HEAD"], timeout=10)[1].strip(),
            "date": datetime.date.today().isoformat(),
            "loop": {"hz": LOOP_HZ, "cycles": LOOP_CYCLES},
        },
        "summary": summarise([e for e in catalogue if e["id"] in {r["id"] for r in rows}], rows),
        "robots": rows,
    }
    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as out:
        json.dump(report, out, indent=1, sort_keys=False, ensure_ascii=False)
        out.write("\n")
    print(f"fleet: {len(rows)} description(s) in {time.monotonic() - started:.0f} s → {args.out}")
    print(json.dumps(report["summary"], indent=1))
    if args.compare:
        return compare(json.load(open(args.compare, encoding="utf-8")), report)
    return 0


GOOD = {"pass", "guest", "bound"}


def compare(committed: dict[str, Any], tonight: dict[str, Any]) -> int:
    """Every row whose stage was good in the committed report and is not tonight — a regression.

    Better is printed and passes: the committed report is regenerated by a person, and a nightly
    job that failed because something started working would be one people learn to ignore.
    """
    before = {row["id"]: row["stages"] for row in committed["robots"]}
    worse, better = [], []
    for row in tonight["robots"]:
        old = before.get(row["id"])
        if old is None:
            continue
        for stage, result in row["stages"].items():
            was, now = old.get(stage, {}).get("outcome"), result.get("outcome")
            if was in GOOD and now not in GOOD:
                worse.append(f"{row['id']} {stage}: {was} → {now} — {result.get('detail', '')[:300]}")
            elif was not in GOOD and now in GOOD and was is not None:
                better.append(f"{row['id']} {stage}: {was} → {now}")
    for line in better:
        print(f"fleet: better than the committed report: {line}")
    for line in worse:
        print(f"fleet: REGRESSION against the committed report: {line}", file=sys.stderr)
    return 1 if worse else 0


if __name__ == "__main__":
    sys.exit(main())

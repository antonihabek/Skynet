#!/usr/bin/env python3
"""The hold probe — what a zero mask does to a body, measured in MuJoCo. DRY-COVERAGE-PLAN.md item 12.C.7 (layer 6).

§24 defines a zero in a command's mask as "hold this joint at its current setpoint", §25 says "a zero mask holds position", and the kernel
writes an all-zero mask on every cycle it has nothing to forward — so what a body does under one is what a robot does whenever a policy is
quiet. F121 is the class: two drivers read the zero as "go limp", and the vendor's own simulator showed a standing Go2 fold from 0.385 m to
0.106 m the first time the kernel held it. A suite that never lets a body stand on its own in physics cannot see that.

Three ways of holding a body, each stepped for a few seconds from the same start and measured the same way. The measure is printed as one
JSON object on stdout; this file judges nothing (`sweep.py` holds the bars and says them on the row):

    (default)   the MuJoCo driver, written an all-zero mask at its control rate, with the operator's hold gains from the configuration —
                the hold the kernel gets. `--gain-scale S` multiplies those gains' stiffness by S (damping by √S, critical damping kept).
    --reference an independent hold of the same law, written here and not through the driver: a torque joint is given
                `kp·(q₀ − q) − kd·q̇` at every physics step, a position servo its start position as a constant setpoint — what the maker's own
                model does with its own home pose. It is what tells a body that cannot stand under *any* joint-space hold (a biped on point
                feet needs balance control) from a driver whose hold is wrong: the driver's hold is held to it, never to a number.
    --let-go    the control: the driver's hold replaced by what F121 describes (a torque joint's actuator at zero, a position servo sent half
                a radian elsewhere). It must fail the bars on a body that otherwise stands.

A joint that begins outside its own range (a URDF's zero pose often is: Unitree's A1 calf runs from -2.78 to -0.88 rad and the file's zero is
0) is put at the nearest end of it before the run, because the limit constraint cannot resist a servo's pull from outside. What the measure
reports:

    drift_rad       the most any joint moved from where it began, and which one
    speed_rad_s     the fastest joint at the end: a body still moving is not standing, even inside the drift bar
    base_drop_m     how far the root fell, and tilt_deg how far it tipped from upright; base_start_m and tilt_start_deg where it began
    floor           whether the scene has any geometry fixed to the world: a body with none falls for ever and a hold cannot stand it
    refused         the driver's own sentence, when it would not take the hold (a torque joint with no gains, F162)

The environment is the driver's (`SKYNET_MUJOCO_SCENE`, `PYTHONPATH`).

    SKYNET_MUJOCO_SCENE=scene.xml python3 fleet/hold.py --config '{"keyframe": "home"}'
"""

from __future__ import annotations

import argparse
import json
import math

#: Long enough for a body that is going to fold to have folded (a Go2 does it in well under a second) and for a slow sag to show.
SECONDS = 3.0
#: What "let go" asks of a position servo: far enough that no hold gain could be mistaken for it.
LET_GO_OFFSET_RAD = 0.5


def scaled(config: str, scale: float) -> str:
    """The configuration with every hold gain's stiffness times `scale` and its damping times its square root."""
    if scale == 1.0:
        return config
    parsed = json.loads(config)
    gains = parsed.get("hold_gains")

    def one(entry: dict) -> dict:
        return {"kp": entry["kp"] * scale, "kd": entry.get("kd", 0.0) * math.sqrt(scale)}

    if isinstance(gains, dict) and "kp" in gains:
        parsed["hold_gains"] = one(gains)
    elif isinstance(gains, dict):
        parsed["hold_gains"] = {name: one(entry) for name, entry in gains.items()}
    return json.dumps(parsed, sort_keys=True)


def probe(config: str, seconds: float = SECONDS, mode: str = "driver", gain_scale: float = 1.0) -> dict:
    import mujoco
    import skynet_rhal as rhal
    from skynet_mujoco.bridge import MujocoBridge

    bridge = MujocoBridge()
    rc = bridge.open(scaled(config, gain_scale))
    if rc != rhal.SKR_OK:
        return {"opened": False, "refused": f"the driver would not open this body (rc {rc})"}
    try:
        model, data, joints = bridge._model, bridge._data, bridge._joints  # noqa: SLF001
        if not joints:
            return {"opened": True, "refused": "the body has no joints to hold"}
        read_q = [float(data.qpos[b.qpos_adr]) for b in joints]
        start_q = _in_range(model, joints, read_q)
        outside = sum(1 for a, b in zip(read_q, start_q) if abs(a - b) > 1e-6)
        if outside:
            for b, q in zip(joints, start_q):
                data.qpos[b.qpos_adr] = q
            mujoco.mj_forward(model, data)
        floor = any(int(model.geom_bodyid[g]) == 0 for g in range(model.ngeom))
        before = _snapshot(bridge)
        state = rhal.State()
        rc = bridge.read_state(state, 0)
        if rc != rhal.SKR_OK:
            return {"opened": True, "refused": f"the first read failed (rc {rc})"}
        count = len(joints)
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[0.0] * count, mask=[0] * count)
        steps = max(1, int(round(seconds / (bridge._substeps * float(model.opt.timestep)))))  # noqa: SLF001
        failed_reads = 0
        for _ in range(steps):
            if mode == "reference":
                _reference_step(bridge, start_q)
                continue
            command.seq += 1
            if mode == "let-go":
                _let_go(bridge, start_q)
            else:
                rc = bridge.write_command(command, 0)
                if rc != rhal.SKR_OK:
                    return {"opened": True, "refused": f"the driver refused the all-zero mask (rc {rc}); its own sentence is on stderr"}
            rc = bridge.read_state(state, 0)
            if rc != rhal.SKR_OK:
                failed_reads += 1
                if failed_reads > 5:
                    return {"opened": True, "refused": f"{failed_reads} reads failed under the hold (rc {rc}): the simulation diverged"}
        after = _snapshot(bridge)
        drifts = [abs(a - b) for a, b in zip(after["q"], before["q"])]
        worst = max(range(count), key=lambda i: drifts[i])
        names = list(bridge.joint_names)  # a property of the driver
        return {
            "opened": True,
            "mode": mode,
            "gain_scale": gain_scale,
            "joints": count,
            "outside_range": outside,
            "floor": floor,
            "seconds": round(after["time"] - before["time"], 3),
            "drift_rad": round(drifts[worst], 5),
            "drift_joint": names[worst],
            "speed_rad_s": round(max(abs(v) for v in after["qd"]), 5),
            "base_start_m": round(before["pose"][2], 5),
            "base_drop_m": round(before["pose"][2] - after["pose"][2], 5),
            "tilt_start_deg": round(_tilt_deg(before["pose"]), 3),
            "tilt_deg": round(_tilt_deg(after["pose"]), 3),
        }
    finally:
        bridge.close()


def _in_range(model, joints, positions: list[float]) -> list[float]:
    reference = []
    for position, binding in zip(positions, joints):
        value = position
        if bool(model.jnt_limited[binding.joint_id]):
            low, high = (float(x) for x in model.jnt_range[binding.joint_id])
            value = min(max(position, low), high)
        reference.append(value)
    return reference


def _snapshot(bridge) -> dict:
    """Joint positions and speeds, the root's pose and the clock, read from the simulator's own state."""
    data, joints = bridge._data, bridge._joints  # noqa: SLF001
    body = bridge._describe.root_body  # noqa: SLF001
    w, x, y, z = (float(v) for v in data.xquat[body])
    return {
        "q": [float(data.qpos[b.qpos_adr]) for b in joints],
        "qd": [float(data.qvel[b.dof_adr]) for b in joints],
        "pose": [float(v) for v in data.xpos[body]] + [x, y, z, w],
        "time": float(data.time),
    }


def _tilt_deg(pose: list[float]) -> float:
    """Degrees between the root's up axis and the world's, from the root's quaternion `x y z w`."""
    qx, qy = pose[3], pose[4]
    up = 1.0 - 2.0 * (qx * qx + qy * qy)
    return math.degrees(math.acos(max(-1.0, min(1.0, up))))


def _reference_step(bridge, start_q: list[float]) -> None:
    """One control period of the independent hold: written here, through `MjData` and not through the driver.

    A torque joint gets `kp·(q₀ − q) − kd·q̇` at every physics step (the law §25 describes for an impedance drive), clipped to the actuator's range;
    a position servo gets its start position as a constant setpoint, which is what the maker's model does with its own home pose.
    """
    import mujoco

    model, data, joints = bridge._model, bridge._data, bridge._joints  # noqa: SLF001
    for _ in range(bridge._substeps):  # noqa: SLF001
        for index, binding in enumerate(joints):
            if binding.mode == "torque":
                gains = bridge._gains_for(binding.name)  # noqa: SLF001
                if gains is None:
                    continue
                kp, kd = gains
                torque = kp * (start_q[index] - float(data.qpos[binding.qpos_adr])) - kd * float(data.qvel[binding.dof_adr])
                ctrl = torque / binding.gear
                if bool(model.actuator_ctrllimited[binding.actuator_id]):
                    low, high = model.actuator_ctrlrange[binding.actuator_id]
                    ctrl = min(max(ctrl, float(low)), float(high))
                data.ctrl[binding.actuator_id] = ctrl
            elif binding.mode == "position":
                data.ctrl[binding.actuator_id] = start_q[index] * binding.gear
            else:
                data.ctrl[binding.actuator_id] = 0.0
        mujoco.mj_step(model, data)


def _let_go(bridge, start_q: list[float]) -> None:
    """The control: F121's hold. A torque joint's actuator is written zero, a position servo is sent somewhere else.

    Reaches into the driver's own `MjData` — a control that goes round the driver is the point of one, and it is never used on a real run.
    """
    data = bridge._data  # noqa: SLF001
    for index, binding in enumerate(bridge._joints):  # noqa: SLF001
        if binding.mode == "torque":
            data.ctrl[binding.actuator_id] = 0.0
        elif binding.mode == "position":
            data.ctrl[binding.actuator_id] = (start_q[index] + LET_GO_OFFSET_RAD) * binding.gear


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--config", default="{}", help="the driver's configuration, as the sweep gives it")
    parser.add_argument("--seconds", type=float, default=SECONDS)
    parser.add_argument("--gain-scale", type=float, default=1.0, help="times the stiffness of every hold gain (damping by its square root)")
    group = parser.add_mutually_exclusive_group()
    group.add_argument("--reference", action="store_true", help="an independent hold of the same law, not through the driver")
    group.add_argument("--let-go", action="store_true", help="the control: replace the hold with what F121 describes")
    options = parser.parse_args(argv)
    mode = "reference" if options.reference else "let-go" if options.let_go else "driver"
    try:
        measure = probe(options.config, options.seconds, mode, options.gain_scale)
    except Exception as error:  # noqa: BLE001 — a body the probe could not run is a row, with the cause
        measure = {"opened": False, "refused": f"{type(error).__name__}: {error}"[:400]}
    print(json.dumps(measure, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

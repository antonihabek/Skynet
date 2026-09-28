"""Sim conformance for the MuJoCo bridge, in the terms of §15.

    python selftest.py

The counterpart of `drivers/mock/selftest.c` and `drivers/lerobot/selftest.py`: every check names the
SCTS id it stands for, so a failure here reads the same way a failure on the bench does.

Two things separate this harness from the other two, and both come from the thing being simulated
rather than faked.

**It is run against physics, not against a stub.** The LeRobot bridge is exercised against a fake
robot that answers as the library is documented to; passing there means the adapter is internally
consistent. Here the body is a MuJoCo model, the numbers come out of an integrator, and a command
that should move a joint is checked by looking at where the joint went. That is a stronger statement
about the driver and a weaker one about any hardware — which is the trade §70 makes, and the whole
reason the descriptor declares `simulator: true`.

**The refusals are exercised against scenes that provoke them.** Five scenes in `scenes/`, four of
which exist only to be refused in a particular way: an actuator the RHAL cannot carry, a body with
two joints on it, a free-floating root with no declared base, and a torque-driven arm where the
command mask has no correct answer. A driver author meets all four in the first week.

Rendering needs an OpenGL context. Where there is none the camera checks are reported as skipped
rather than passed, and the driver's own behaviour — declare no sensors, report `ENOSYS` — is
checked instead, because that is the branch a headless machine will actually run.
"""

from __future__ import annotations

import json
import math
import os
import re
import sys
import threading
import time

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "bindings", "python"))
sys.path.insert(0, os.path.dirname(__file__))

import skynet_rhal as rhal  # noqa: E402
from skynet_rhal import Command, Frame, State  # noqa: E402

from skynet_mujoco.bridge import MujocoBridge  # noqa: E402
from skynet_mujoco.describe import BASE_KINDS  # noqa: E402
from skynet_mujoco.render import DEFAULT_SIZE, probe_cameras  # noqa: E402
from skynet_mujoco.render import _rhal_size  # noqa: E402 — pure sizing; held without a context

SCENES = os.path.join(os.path.dirname(os.path.abspath(__file__)), "scenes")

_PASSED = 0
_SKIPPED: list[str] = []
_FAILED: list[str] = []
_USED_IDS: set[str] = set()


def check(scts_id: str, what: str, held: bool, detail: str = "") -> None:
    global _PASSED
    _USED_IDS.add(scts_id)
    if held:
        _PASSED += 1
        print(f"ok   {scts_id:<28} {what}")
    else:
        _FAILED.append(f"{scts_id}: {what}")
        print(f"FAIL {scts_id:<28} {what}")
        if detail:
            print(f"     {detail}")


def skip(scts_id: str, what: str, why: str) -> None:
    _USED_IDS.add(scts_id)
    _SKIPPED.append(f"{scts_id}: {what} — {why}")
    print(f"skip {scts_id:<28} {what}")
    print(f"     {why}")


def scene(name: str) -> str:
    return os.path.join(SCENES, f"{name}.xml")


def opened(name: str = "so_arm_shaped", **config) -> MujocoBridge:
    bridge = MujocoBridge()
    config.setdefault("scene", scene(name))
    rc = bridge.open(json.dumps(config))
    assert rc == rhal.SKR_OK, f"open({name}) returned {rc}"
    return bridge


def read(bridge: MujocoBridge) -> tuple[int, State]:
    state = State()
    return bridge.read_state(state, 0), state


def command(bridge, values, mask=None, mode=rhal.MODE_POSITION, seq=1) -> int:
    mask = mask if mask is not None else [1] * len(values)
    return bridge.write_command(
        Command(stamp_ns=1, seq=seq, mode=mode, values=list(values), mask=list(mask)), 0
    )


def settle(bridge: MujocoBridge, cycles: int = 200) -> State:
    """Run the loop without commanding, which is what an operator watching a robot hold looks like."""
    state = State()
    for _ in range(cycles):
        bridge.read_state(state, 0)
    return state


# ── rhal.abi.version ─────────────────────────────────────────────────────────


def abi_checks() -> None:
    rhal.assert_abi_layout()
    check("rhal.abi.version", "the ctypes ABI view matches the C header", True)

    bridge = MujocoBridge()
    check(
        "rhal.abi.version",
        "opens a scene",
        bridge.open(json.dumps({"scene": scene("so_arm_shaped")})) == rhal.SKR_OK,
    )
    missing = [
        name
        for name in ("open", "close", "read_state", "write_command", "emergency_stop", "describe")
        if not callable(getattr(bridge, name, None))
    ]
    check("rhal.abi.version", "implements every mandatory entry point", not missing, str(missing))
    check("rhal.abi.version", "closes", bridge.close() == rhal.SKR_OK)

    check(
        "rhal.abi.version",
        "refuses a configuration that is not JSON",
        MujocoBridge().open("{not json") == rhal.SKR_EINVAL,
    )
    check(
        "rhal.abi.version",
        "refuses a scene that does not exist",
        MujocoBridge().open(json.dumps({"scene": scene("no_such_scene")})) == rhal.SKR_ENODEV,
    )
    check(
        "rhal.abi.version",
        "refuses a control rate that is not a rate",
        MujocoBridge().open(json.dumps({"scene": scene("so_arm_shaped"), "rate_hz": 0}))
        == rhal.SKR_EINVAL,
    )
    check(
        "rhal.abi.version",
        "refuses a base kind that is not one",
        MujocoBridge().open(json.dumps({"scene": scene("free_base"), "base": "hovercraft"}))
        == rhal.SKR_EINVAL,
    )
    # F164: a moving base declared for a root the scene bolts to the world is refused rather than
    # written into a descriptor whose own provenance says the base is fixed. `fixed` is accepted.
    check(
        "rhal.abi.version",
        "refuses a moving base declared for a root the scene fixes to the world",
        MujocoBridge().open(json.dumps({"scene": scene("so_arm_shaped"), "base": "legged"}))
        == rhal.SKR_EINVAL
        and MujocoBridge().open(json.dumps({"scene": scene("so_arm_shaped"), "base": "fixed"}))
        == rhal.SKR_OK,
    )
    check(
        "rhal.abi.version",
        "refuses a keyframe the scene does not have",
        MujocoBridge().open(json.dumps({"scene": scene("so_arm_shaped"), "keyframe": "home"}))
        == rhal.SKR_EINVAL,
    )

    # No scene at all. The environment variable is the other way in, so it has to be cleared for
    # this to mean anything — and the fact that it has to be is worth a check of its own.
    saved = os.environ.pop("SKYNET_MUJOCO_SCENE", None)
    try:
        check(
            "rhal.abi.version",
            "refuses to invent a body when no scene is named",
            MujocoBridge().open("") == rhal.SKR_EINVAL,
        )
    finally:
        if saved is not None:
            os.environ["SKYNET_MUJOCO_SCENE"] = saved

    os.environ["SKYNET_MUJOCO_SCENE"] = scene("so_arm_shaped")
    try:
        check(
            "rhal.abi.version",
            "and takes the scene from the environment when the configuration names none",
            MujocoBridge().open("") == rhal.SKR_OK,
        )
    finally:
        if saved is None:
            os.environ.pop("SKYNET_MUJOCO_SCENE", None)
        else:
            os.environ["SKYNET_MUJOCO_SCENE"] = saved


# ── rhal.state.completeness ──────────────────────────────────────────────────


def state_checks() -> None:
    bridge = opened()
    rc, first = read(bridge)
    check("rhal.state.completeness", "read_state succeeds", rc == rhal.SKR_OK)
    check("rhal.state.completeness", "joint count matches the body", first.joint_count == 6)
    check("rhal.state.completeness", "timestamps are non-zero", first.stamp_ns != 0)
    check(
        "rhal.state.completeness",
        "every joint reports a finite position",
        all(math.isfinite(v) for v in first.position_rad),
    )
    # A simulator measures all three, so unlike the LeRobot bridge none of them is NaN. Reporting a
    # velocity here is a measurement, not a guess: the integrator has one.
    check(
        "rhal.state.completeness",
        "velocity and effort are measured rather than absent",
        all(math.isfinite(v) for v in first.velocity_rad_s)
        and all(math.isfinite(v) for v in first.effort_nm),
    )

    _, second = read(bridge)
    check("rhal.state.completeness", "the sequence advances", second.seq == first.seq + 1)
    check(
        "rhal.state.completeness",
        "reading before open is refused",
        MujocoBridge().read_state(State(), 0) == rhal.SKR_EPERM,
    )

    # §35.3. The clock is the simulator's, and one read is exactly one control period of it — which
    # is what makes a watchdog measured in this clock mean the same thing it means on hardware.
    period_ns = round(1e9 / 100.0)
    check(
        "rhal.state.rate",
        "one read advances the clock by exactly one control period",
        second.stamp_ns - first.stamp_ns == period_ns,
        f"{second.stamp_ns - first.stamp_ns} ns, expected {period_ns}",
    )
    slow = opened(rate_hz=50.0)
    _, a = read(slow)
    _, b = read(slow)
    check(
        "rhal.state.rate",
        "and a different declared rate is a different period, not a different wall-clock cost",
        b.stamp_ns - a.stamp_ns == round(1e9 / 50.0),
        f"{b.stamp_ns - a.stamp_ns} ns",
    )
    check(
        "rhal.state.rate",
        "the descriptor's rate is the one the clock actually advances at",
        json.loads(slow.describe())["control"]["rate_hz_max"] == 50.0,
    )

    # The property the whole design of this driver turns on: time passes because the robot is
    # running, not because somebody commanded it. Had the world advanced only on a command, a
    # stalled policy would freeze the universe and `safety.watchdog.fires` would be untestable in
    # simulation — which is half the reason for having a simulator driver.
    stalled = opened()
    read(stalled)
    command(stalled, [0.3] * 6)
    before = settle(stalled, 5)
    after = settle(stalled, 50)
    check(
        "safety.watchdog.fires",
        "the clock keeps advancing while nothing commands, so a stall is observable",
        after.stamp_ns > before.stamp_ns,
        f"{before.stamp_ns} then {after.stamp_ns}",
    )
    check(
        "safety.watchdog.fires",
        "and the last command keeps being applied, as drives do",
        abs(after.position_rad[1] - 0.3) < 0.05,
        f"joint 1 at {after.position_rad[1]}",
    )

    # Divergence, and the reason it is checked with a counter rather than with `isnan`.
    #
    # MuJoCo **resets itself** when the integrator blows up, so a driver that looks only at the
    # numbers sees a robot that teleported to its home pose at full speed and reports it as a
    # perfectly good state. The state is injected here rather than provoked with a scene, because a
    # scene that diverges reliably is a scene that diverges differently on the next MuJoCo release.
    unstable = opened()
    read(unstable)
    unstable._data.qvel[:] = 1e12
    rc, _ = read(unstable)
    check(
        "rhal.state.completeness",
        "a diverged simulation is reported, not laundered into a plausible state",
        rc == rhal.SKR_EPROTO,
        f"read_state returned {rc}",
    )
    rc, recovered = read(unstable)
    check(
        "rhal.state.completeness",
        "and the cycle after it reads normally again, because MuJoCo has reset",
        rc == rhal.SKR_OK and all(math.isfinite(v) for v in recovered.position_rad),
    )

    # Ground truth, and the fact that it is ground truth.
    check(
        "rhal.state.completeness",
        "the base pose is the simulator's exact answer rather than an unlocalised NaN",
        all(math.isfinite(v) for v in after.base_pose),
        str(after.base_pose),
    )
    check(
        "rhal.state.completeness",
        "a fixed base reports no twist",
        all(v == 0.0 for v in after.base_twist),
    )

    wheeled = opened("free_base", base="wheeled")
    read(wheeled)
    command(wheeled, [6.0, 6.0], mode=rhal.MODE_VELOCITY)
    rolling = settle(wheeled, 300)
    check(
        "rhal.state.completeness",
        "a free-jointed base reports the twist its free joint carries",
        any(abs(v) > 1e-3 for v in rolling.base_twist),
        str(rolling.base_twist),
    )
    check(
        "rhal.state.completeness",
        "and the base pose moves with it",
        abs(rolling.base_pose[0]) > 1e-3,
        str(rolling.base_pose[:3]),
    )


# ── rhal.command.rejection ───────────────────────────────────────────────────


def command_checks() -> None:
    bridge = opened()
    read(bridge)

    check("rhal.command.rejection", "accepts a well-formed command", command(bridge, [0.1] * 6) == rhal.SKR_OK)
    check("rhal.command.rejection", "rejects a wrong joint count", command(bridge, [0.1] * 5) == rhal.SKR_EINVAL)
    check("rhal.command.rejection", "rejects an unknown mode", command(bridge, [0.1] * 6, mode=200) == rhal.SKR_EINVAL)
    check(
        "rhal.command.rejection",
        "rejects a non-finite value",
        command(bridge, [0.1, float("nan"), 0.1, 0.1, 0.1, 0.1]) == rhal.SKR_EINVAL,
    )
    # …and applies none of it. The loop used to write joint by joint and refuse at the first bad
    # value, so the joint before it had already been sent to 0.7.
    half = opened()
    read(half)
    command(half, [0.0] * 6)
    before = settle(half, 200)
    command(half, [0.7, float("nan"), 0.0, 0.0, 0.0, 0.0], seq=2)
    after = settle(half, 200)
    check(
        "rhal.command.rejection",
        "a refused command moves no joint, including the ones before the bad value",
        abs(after.position_rad[0] - before.position_rad[0]) < 0.005,
        f"joint 0 went {before.position_rad[0]:.4f} -> {after.position_rad[0]:.4f}",
    )
    check(
        "rhal.command.rejection",
        "rejects a mode this body does not accept",
        command(bridge, [0.1] * 6, mode=rhal.MODE_TORQUE) == rhal.SKR_EINVAL,
    )
    check(
        "rhal.command.rejection",
        "commanding before open is refused",
        MujocoBridge().write_command(Command(values=[0.0] * 6, mask=[1] * 6), 0) == rhal.SKR_EPERM,
    )

    # A command that is accepted must actually move the body. This is what a simulator adds over a
    # fake: the check is on where the joint went, not on what was recorded as sent.
    moving = opened()
    read(moving)
    command(moving, [0.0, 0.6, 0.0, 0.0, 0.0, 0.0])
    arrived = settle(moving, 400)
    check(
        "rhal.command.rejection",
        "an accepted command reaches the joint",
        abs(arrived.position_rad[1] - 0.6) < 0.05,
        f"joint 1 at {arrived.position_rad[1]:.4f}, commanded 0.6",
    )

    # The mask, which is the field worth reading twice. A masked joint holds where it is; it does
    # not go to zero, and on this body that difference is visible in the physics.
    held = opened()
    read(held)
    command(held, [0.0, 0.6, 0.0, 0.0, 0.0, 0.0])
    settle(held, 400)
    command(held, [0.0, 0.9, 0.0, 0.0, 0.0, 0.0], mask=[1, 0, 1, 1, 1, 1], seq=2)
    after = settle(held, 300)
    check(
        "rhal.command.rejection",
        "a masked joint holds where it is rather than going to zero",
        abs(after.position_rad[1] - 0.6) < 0.05,
        f"joint 1 at {after.position_rad[1]:.4f}, held from 0.6, would be 0.0 if zeroed",
    )

    # And holds *there*: §24's "current setpoint", latched when the hold begins. The kernel sends
    # an all-masked frame every cycle it has nothing to forward, and until 2026-09-24 this driver
    # answered each one by writing the joint's measured position back as its setpoint — which zeroes
    # the stiffness term, so gravity took the loaded shoulder 0.65 rad in these ten seconds
    # (DRY-COVERAGE-PLAN.md F121, carried as 8.13).
    loaded = opened()
    read(loaded)
    pose = [0.0, 0.9, 0.9, 0.0, 0.0, 0.0]
    for _ in range(600):
        command(loaded, pose)
        read(loaded)
    _, start = read(loaded)
    for step in range(1000):
        command(loaded, [0.0] * 6, mask=[0] * 6, seq=step + 2)
        read(loaded)
    _, end = read(loaded)
    drift = max(abs(a - b) for a, b in zip(end.position_rad, start.position_rad))
    check(
        "rhal.command.rejection",
        "a thousand hold frames keep a loaded arm where its last command put it",
        drift < 0.005,
        f"drifted {drift:.4f} rad; a hold that re-reads the measurement lets gravity win",
    )

    # Torque, where `hold` has no correct value: zero torque is limp, not held, and computing the
    # gravity-compensating torque is a controller this driver is not.
    torque = opened("torque_arm")
    read(torque)
    check(
        "rhal.command.rejection",
        "a fully applied torque command is accepted",
        command(torque, [0.2, 0.1], mode=rhal.MODE_TORQUE) == rhal.SKR_OK,
    )
    check(
        "rhal.command.rejection",
        "a torque command with a masked joint is refused rather than approximated",
        command(torque, [0.2, 0.1], mask=[1, 0], mode=rhal.MODE_TORQUE, seq=2) == rhal.SKR_EINVAL,
    )
    check(
        "rhal.command.rejection",
        "and a position command to a torque body is refused",
        command(torque, [0.2, 0.1], mode=rhal.MODE_POSITION, seq=3) == rhal.SKR_EINVAL,
    )

    # §25: a zero mask holds position on a torque joint too — with the operator's gains, as
    # `drivers/unitree` holds (F121), and refused without them (above). DRY-COVERAGE-PLAN.md F162:
    # every quadruped in DeepMind's menagerie is `<motor>`-driven, and until this held none could
    # take the kernel's hold frame, so none could run under the loop at all.
    def tilted_then(action: str, **config) -> tuple[float, bool, float]:
        """Tilt the arm with a torque, then hold it (or command zero); return the drift, whether
        every write was accepted, and how far the tilt was from the joint's stop at ±2 rad."""
        arm = opened("torque_arm", **config)
        read(arm)
        # The arm stands straight up, so off vertical its own weight pulls it over: a short push
        # and the hold has gravity to hold against, clear of the stop.
        for step in range(10):
            command(arm, [0.05, 0.01], mode=rhal.MODE_TORQUE, seq=step + 1)
            read(arm)
        _, start = read(arm)
        accepted = True
        for step in range(500):
            if action == "hold":
                rc = command(arm, [0.0, 0.0], mask=[0, 0], mode=rhal.MODE_TORQUE, seq=step + 100)
            else:
                rc = command(arm, [0.0, 0.0], mode=rhal.MODE_TORQUE, seq=step + 100)
            accepted = accepted and rc == rhal.SKR_OK
            read(arm)
        _, end = read(arm)
        arm.close()
        drift = max(abs(a - b) for a, b in zip(end.position_rad, start.position_rad))
        return drift, accepted, 2.0 - max(abs(q) for q in start.position_rad)

    held, held_ok, clear = tilted_then("hold", hold_gains={"kp": 2.0, "kd": 0.05})
    limp, _, _ = tilted_then("limp")
    check(
        "rhal.command.rejection",
        "with hold_gains a masked torque joint holds where the hold began, and zero torque does not",
        # Gains sized to a 0.08 kg link at a 2 ms step: kd·dt/I stays under the explicit limit of 2
        # — a kd of 1 here is ten times over it and the hold rings itself apart, which is a gain
        # nobody should choose, not a driver fault. 0.05 rad is the latch against gravity, measured
        # against a limp control and with the arm clear of its stop so the stop cannot hold it.
        held_ok and held < 0.05 and limp > 0.2 and clear > 0.3,
        f"every hold write accepted: {held_ok}; held {held:.4f} rad, limp {limp:.4f} rad, "
        f"{clear:.2f} rad clear of the stop",
    )
    gained = opened("torque_arm", hold_gains={"j1": {"kp": 20.0, "kd": 1.0}})
    read(gained)
    check(
        "rhal.command.rejection",
        "gains for one joint hold that joint and still refuse the other's hold",
        command(gained, [0.0, 0.1], mask=[0, 1], mode=rhal.MODE_TORQUE) == rhal.SKR_OK
        and command(gained, [0.1, 0.0], mask=[1, 0], mode=rhal.MODE_TORQUE, seq=2) == rhal.SKR_EINVAL,
    )
    command(gained, [0.0, 0.1], mask=[0, 1], mode=rhal.MODE_TORQUE, seq=3)
    gained.emergency_stop()
    for _ in range(20):
        read(gained)
    check(
        "safety.estop.latency",
        "the stop ends a torque hold rather than the next step writing it back",
        all(value == 0.0 for value in gained._data.ctrl),
        f"ctrl after the stop: {list(gained._data.ctrl)}",
    )
    refused = MujocoBridge()
    check(
        "rhal.command.rejection",
        "a hold gain of zero stiffness is refused at open — that is a free joint",
        refused.open(json.dumps({"scene": scene("torque_arm"), "hold_gains": {"kp": 0.0, "kd": 1.0}}))
        != rhal.SKR_OK,
    )


def gear_checks() -> None:
    """F167: every value crosses the actuator's gear, in both directions."""
    geared = opened("geared")
    described = {joint["name"]: joint for joint in json.loads(geared.describe())["joints"]}
    check(
        "embodiment.limits.sourced",
        "a geared servo's force ceiling is the range times the gear, in joint units",
        described["ja"]["limits"].get("effort_max_nm") == 6.0
        and described["jb"]["limits"].get("effort_max_nm") == 1.5,
        f"ja {described['ja']['limits']}, jb {described['jb']['limits']}",
    )
    read(geared)
    for step in range(400):
        command(geared, [0.3, -0.2], seq=step + 1)
        _, state = read(geared)
    check(
        "rhal.command.rejection",
        "a position command through a gear puts the joint where it was asked, not at a gear's fraction",
        abs(state.position_rad[0] - 0.3) < 0.01 and abs(state.position_rad[1] + 0.2) < 0.01,
        f"settled at {state.position_rad[0]:.4f} and {state.position_rad[1]:.4f} for 0.3 and -0.2",
    )

    motor = opened("geared_motor")
    rating = json.loads(motor.describe())["joints"][0]["limits"].get("effort_max_nm")
    check(
        "embodiment.limits.sourced",
        "a motor bounded by ctrlrange declares that bound, through its gear",
        rating == 10.0,
        f"declared {rating}; the ctrlrange is ±1 and the gear 10",
    )
    read(motor)
    command(motor, [4.0], mode=rhal.MODE_TORQUE)
    _, state = read(motor)
    check(
        "rhal.state.completeness",
        "a torque commanded through a gear reads back as the torque the joint felt",
        abs(state.effort_nm[0] - 4.0) < 1e-6,
        f"read {state.effort_nm[0]:.4f} N·m for a 4.0 N·m command",
    )


def mixed_mode_checks() -> None:
    """F169: a body with joints in two modes is commanded joint by joint and held in each."""
    body = opened("mixed_modes")
    control = json.loads(body.describe())["control"]
    check(
        "embodiment.descriptor.valid",
        "a two-mode body declares both modes and that it reads the per-joint array (§86.1)",
        sorted(control["modes"]) == ["position", "velocity"] and control.get("per_joint_modes") is True,
        f"control: {control}",
    )
    read(body)
    check(
        "rhal.command.rejection",
        "the kernel's all-held frame is accepted whatever mode it names",
        command(body, [0.0, 0.0], mask=[0, 0]) == rhal.SKR_OK
        and command(body, [0.0, 0.0], mask=[0, 0], mode=rhal.MODE_VELOCITY, seq=2) == rhal.SKR_OK,
    )
    check(
        "rhal.command.rejection",
        "a body-wide frame commands the joints of its mode and holds the others",
        command(body, [0.0, 0.4], mask=[0, 1], seq=3) == rhal.SKR_OK
        and command(body, [0.4, 0.0], mask=[1, 0], seq=4) == rhal.SKR_EINVAL,
    )
    both = Command(
        stamp_ns=1, seq=5, mode=rhal.MODE_POSITION, values=[3.0, 0.4], mask=[1, 1],
        modes=[rhal.JMODE_VELOCITY, rhal.JMODE_POSITION],
    )
    accepted = body.write_command(both, 0) == rhal.SKR_OK
    for _ in range(300):
        _, state = read(body)
    check(
        "rhal.command.rejection",
        "per-joint modes spin the wheel and place the arm in one command",
        accepted and state.velocity_rad_s[0] > 1.0 and abs(state.position_rad[1] - 0.4) < 0.02,
        f"accepted {accepted}; wheel {state.velocity_rad_s[0]:.3f} rad/s, shoulder {state.position_rad[1]:.4f} rad",
    )
    bare = Command(stamp_ns=1, seq=6, mode=rhal.MODE_POSITION, values=[0.0, 0.0], mask=[1, 1], modes=[rhal.MODE_VELOCITY, 0])
    check(
        "rhal.command.rejection",
        "a bare MODE_* in the per-joint array is refused, not read as the nearest mode (§86.1)",
        body.write_command(bare, 0) == rhal.SKR_EINVAL,
    )


# ── safety.estop.latency ─────────────────────────────────────────────────────


def estop_checks() -> None:
    bridge = opened()
    read(bridge)
    command(bridge, [0.0, 0.5, 0.0, 0.0, 0.0, 0.0])
    settle(bridge, 100)

    check("safety.estop.latency", "emergency_stop returns", bridge.emergency_stop() == rhal.SKR_OK)
    _, state = read(bridge)
    check(
        "safety.estop.latency",
        "an asserted stop is observable in the next state",
        state.flags & rhal.FLAG_ESTOP != 0,
    )
    check(
        "safety.estop.latency",
        "commands are refused while stopped",
        command(bridge, [0.1] * 6, seq=9) == rhal.SKR_EPERM,
    )
    check(
        "safety.estop.latency",
        "and every actuator command was zeroed",
        all(float(v) == 0.0 for v in bridge._data.ctrl),
    )
    # Declared because it is implemented, and only the software stop: the descriptor carries no
    # envelope, so nothing here can be read as a rated one (F63).
    descriptor = json.loads(bridge.describe())
    check(
        "safety.estop.latency",
        "declares the software stop it has, and no rated stop",
        descriptor.get("capabilities") == ["safety.estop"] and descriptor["envelope"] is None,
        str(descriptor.get("capabilities")),
    )

    # Said out loud because the number of checks above could be mistaken for evidence about a stop
    # path. §15 counts `safety.estop.independent` at the certified tier and no simulator will ever
    # produce a result for it.
    check(
        "safety.estop.independent",
        "is not measurable here and is not claimed — a simulated stop cuts nothing",
        True,
    )


# ── embodiment.* ─────────────────────────────────────────────────────────────


def descriptor_checks() -> None:
    bridge = opened()
    descriptor = json.loads(bridge.describe())

    check("embodiment.descriptor.valid", "describe returns parseable JSON", True)
    check(
        "embodiment.descriptor.valid",
        "names this body from the scene",
        descriptor["robot_slug"] == "so-arm-shaped",
        descriptor["robot_slug"],
    )
    check(
        "embodiment.descriptor.valid",
        "dof agrees with the joint list",
        descriptor["dof"] == len(descriptor["joints"]) == 6,
    )
    check(
        "embodiment.descriptor.valid",
        "declares the modes the actuators imply",
        descriptor["control"]["modes"] == ["position"],
        str(descriptor["control"]["modes"]),
    )
    check(
        "embodiment.descriptor.valid",
        "declares itself a simulator",
        descriptor["simulator"] is True,
    )
    check(
        "embodiment.descriptor.valid",
        "and its tier ceiling, with simulation named in the reason",
        descriptor["provenance"]["driver_ceiling"] == "guest"
        and "simulator" in descriptor["provenance"]["ceiling_reason"],
    )
    check(
        "embodiment.descriptor.valid",
        "declares command latency as the one period it is by construction",
        descriptor["control"]["command_latency_ms"] == 10.0,
        str(descriptor["control"]["command_latency_ms"]),
    )
    check(
        "embodiment.descriptor.valid",
        "records that the clock is sim time",
        "sim time" in descriptor["provenance"]["timestamps"].lower(),
    )
    check(
        "embodiment.descriptor.valid",
        "records that the base pose is ground truth no robot has",
        "ground truth" in descriptor["provenance"]["localisation"],
    )
    check(
        "embodiment.descriptor.valid",
        "names the scene it was built from and its digest",
        descriptor["provenance"]["scene"] == "so_arm_shaped.xml"
        and len(descriptor["provenance"]["scene_sha256"]) == 64,
    )

    check(
        "embodiment.limits.sourced",
        "declares no unsourced safety envelope",
        descriptor["envelope"] is None,
    )
    check(
        "embodiment.limits.honest",
        "carries the per-joint ratings the scene states",
        all(
            joint["limits"]["position_min_rad"] is not None
            and joint["limits"]["effort_max_nm"] is not None
            for joint in descriptor["joints"]
        ),
        str(descriptor["joints"][0]["limits"]),
    )
    check(
        "embodiment.limits.honest",
        "and the force ceiling is the magnitude that holds in both directions",
        descriptor["joints"][1]["limits"]["effort_max_nm"] == 18.0,
        str(descriptor["joints"][1]["limits"]),
    )
    check(
        "embodiment.descriptor.signed",
        "does not claim to be signed",
        descriptor["verified"] is False and descriptor["digest"] is None,
    )

    # Roles. None, deliberately, and an override is an operator's assertion rather than an inference.
    check(
        "embodiment.roles.coverage",
        "assigns no roles from joint names, however suggestive they are",
        all(joint["role"] is None for joint in descriptor["joints"]),
    )
    check(
        "embodiment.roles.coverage",
        "and says where role suggestion actually lives",
        "from-urdf" in descriptor["provenance"]["roles"],
    )
    overridden = json.loads(
        opened(role_overrides={"shoulder_pan": "right_arm.shoulder.yaw"}).describe()
    )
    check(
        "embodiment.roles.coverage",
        "a configured override is applied and recorded as an assertion",
        overridden["joints"][0]["role"] == "right_arm.shoulder.yaw"
        and "operator's assertion" in overridden["provenance"]["roles"],
    )

    # End effectors: the gripper is a joint in the kinematics and is not an end effector, because
    # nothing in the scene says it is one.
    check(
        "embodiment.descriptor.valid",
        "declares no end effector it was not told about",
        descriptor["end_effectors"] == [],
    )

    check(
        "embodiment.descriptor.valid",
        "declares a base kind from the list",
        descriptor["base"] in BASE_KINDS,
    )


# ── The frame graph, §44.3 ───────────────────────────────────────────────────


def frame_checks() -> None:
    frames = json.loads(opened().describe())["frames"]
    check("embodiment.descriptor.valid", "emits a frame graph", frames is not None)
    check(
        "embodiment.descriptor.valid",
        "rooted at the scene's base link, in REP-103, from MJCF",
        frames["root"] == "base_link"
        and frames["convention"] == "rep103"
        and frames["generator"] == "mjcf",
        str({k: v for k, v in frames.items() if k != "nodes"}),
    )

    nodes = {node["name"]: node for node in frames["nodes"]}
    check(
        "embodiment.descriptor.valid",
        "every body in the robot's subtree is a frame",
        set(nodes) == {
            "base_link",
            "shoulder_link",
            "upper_arm_link",
            "forearm_link",
            "wrist_link",
            "wrist_roll_link",
            "gripper_link",
        },
        str(sorted(nodes)),
    )
    check(
        "embodiment.descriptor.valid",
        "the floor is not part of the robot's frame graph",
        "world" not in nodes,
    )
    check(
        "embodiment.descriptor.valid",
        "the root has no parent and no transform",
        nodes["base_link"]["parent"] is None
        and nodes["base_link"]["kind"] == "root"
        and "translation" not in nodes["base_link"],
    )

    elbow = nodes["forearm_link"]
    check(
        "embodiment.descriptor.valid",
        "a dynamic frame names its joint, its kind and its axis",
        elbow["kind"] == "dynamic"
        and elbow["joint"] == "elbow_flex"
        and elbow["joint_type"] == "revolute"
        and elbow["axis"] == [0.0, 1.0, 0.0],
        str(elbow),
    )
    check(
        "embodiment.descriptor.valid",
        "and its parent-relative transform, with the quaternion ordered xyzw",
        elbow["translation"] == [0.0, 0.0, 0.14] and elbow["rotation"] == [0.0, 0.0, 0.0, 1.0],
        str((elbow["translation"], elbow["rotation"])),
    )
    check(
        "embodiment.descriptor.valid",
        "and records that the transform is the descriptor's rather than a calibration's",
        elbow["source"] == "descriptor",
    )
    check(
        "embodiment.descriptor.valid",
        "every non-root frame reaches the root",
        all(_reaches_root(nodes, name) for name in nodes),
    )

    # Two runs, two descriptors, the same bytes. A descriptor whose digest depends on which machine
    # generated it is a descriptor nobody can verify.
    check(
        "embodiment.descriptor.valid",
        "two imports of one scene produce identical bytes",
        opened().describe() == opened().describe(),
    )

    # A body carrying two joints cannot be expressed in the pinned shape, so the graph is omitted
    # rather than emitted with a hinge missing from it.
    awkward = json.loads(opened("two_joint_body").describe())
    check(
        "embodiment.descriptor.valid",
        "a body with two joints omits the frame graph rather than losing one",
        awkward["frames"] is None,
    )
    check(
        "embodiment.descriptor.valid",
        "and the body still drives",
        awkward["dof"] == 2,
    )


def _reaches_root(nodes: dict, name: str, limit: int = 32) -> bool:
    seen = 0
    while nodes[name]["parent"] is not None:
        name = nodes[name]["parent"]
        seen += 1
        if name not in nodes or seen > limit:
            return False
    return True


# ── What the RHAL cannot carry ───────────────────────────────────────────────


def exclusion_checks() -> None:
    bridge = opened("unsupported_actuators")
    descriptor = json.loads(bridge.describe())
    names = [joint["name"] for joint in descriptor["joints"]]

    check(
        "embodiment.descriptor.valid",
        "an actuator on a tendon is not a joint",
        "cable_pull" not in names,
    )
    check(
        "embodiment.descriptor.valid",
        "an actuator with internal state is not a joint",
        "rolling" not in names,
    )
    check(
        "embodiment.descriptor.valid",
        "a ball joint is not one scalar and is not a joint",
        "wobble" not in names,
    )
    check(
        "embodiment.descriptor.valid",
        "and what remains is declared honestly",
        names == ["plain"] and descriptor["dof"] == 1,
        str(names),
    )

    levels = [note.level for note in bridge.notes]
    check(
        "embodiment.descriptor.valid",
        "each exclusion is reported rather than dropped quietly",
        levels.count("warn") == 3,
        str([note.message[:60] for note in bridge.notes if note.level == "warn"]),
    )


# ── The camera path ──────────────────────────────────────────────────────────


def sensor_checks() -> None:
    bridge = opened()
    descriptor = json.loads(bridge.describe())
    sensors = descriptor["sensors"]

    if not sensors:
        # The headless branch, and it is the one a CI runner and a container actually take. The
        # driver must decline to declare rather than declare and fail the reads.
        skip(
            "embodiment.descriptor.valid",
            "camera frames",
            "no OpenGL context on this machine, so the driver declares no sensors. "
            "Set MUJOCO_GL=osmesa or =egl with the matching library installed to exercise it.",
        )
        check(
            "embodiment.descriptor.valid",
            "a machine that cannot render declares no sensors rather than unreadable ones",
            sensors == [],
        )
        check(
            "embodiment.descriptor.valid",
            "and read_sensor reports the entry point as unimplemented",
            bridge.read_sensor("wrist_cam", Frame()) == rhal.SKR_ENOSYS,
        )
        check(
            "embodiment.descriptor.valid",
            "and says why, where an operator will see it",
            "OpenGL" in descriptor["provenance"]["cameras"],
        )
        return

    check(
        "embodiment.descriptor.valid",
        "the scene's camera appears as a sensor, not as a joint",
        len(sensors) == 1 and descriptor["dof"] == 6,
        str(sensors),
    )
    check(
        "embodiment.descriptor.valid",
        "declared with the modality and the frame it is mounted on",
        sensors[0]["modality"] == "rgb" and sensors[0]["frame"] == "wrist_roll_link",
        str(sensors[0]),
    )
    check(
        "embodiment.descriptor.valid",
        "and no frame rate, because it is rendered on demand",
        sensors[0]["fps"] is None,
    )

    rc, state = read(bridge)
    frame = Frame()
    check(
        "embodiment.descriptor.valid",
        "a declared camera reads",
        bridge.read_sensor("wrist_cam", frame) == rhal.SKR_OK,
    )
    check(
        "embodiment.descriptor.valid",
        "the frame's declared geometry fits the bytes behind it",
        frame.stride * frame.height <= len(frame.data) and len(frame.data) > 0,
        f"{frame.width}x{frame.height} stride {frame.stride}, {len(frame.data)} bytes",
    )
    check(
        "embodiment.descriptor.valid",
        "and matches what the descriptor declared",
        (frame.width, frame.height) == (sensors[0]["width"], sensors[0]["height"]),
    )
    check(
        "embodiment.descriptor.valid",
        "the frame is stamped in the same clock as the state it accompanies",
        frame.stamp_ns == state.stamp_ns,
        f"frame {frame.stamp_ns} vs state {state.stamp_ns}",
    )
    check(
        "embodiment.descriptor.valid",
        "an unknown sensor is no such device",
        bridge.read_sensor("nose", Frame()) == rhal.SKR_ENODEV,
    )

    # The frame follows the body: render, move, render, and the pixels must differ. A cached scene
    # renders the robot where it was, and an observation that lags its state is the failure §25.1
    # spends a page on.
    first = bytes(frame.data)
    command(bridge, [1.2, 0.6, 0.0, 0.0, 0.0, 0.0])
    settle(bridge, 300)
    moved = Frame()
    bridge.read_sensor("wrist_cam", moved)
    check(
        "embodiment.descriptor.valid",
        "the frame is rendered from the state it is stamped with, not from a cached scene",
        bytes(moved.data) != first,
    )

    check(
        "embodiment.descriptor.valid",
        "a scene with no camera declares no sensors",
        json.loads(opened("torque_arm").describe())["sensors"] == [],
    )

    check(
        "embodiment.descriptor.valid",
        "the camera is declared `concurrent_read`, exactly `true` (§89.1)",
        sensors[0].get("concurrent_read") is True,
        str(sensors[0]),
    )

    # §89.4's question, asked of the real renderer: frames read on a second thread while this one
    # steps. Every one must come back, and every one must carry a stamp this thread published — a
    # frame stamped between two steps would be a picture of a state no caller ever held.
    bridge = opened()
    published: set[int] = set()
    rc, state = read(bridge)
    published.add(state.stamp_ns)
    frames: list[tuple[int, int]] = []
    done = threading.Event()

    def sensor_thread() -> None:
        while not done.is_set():
            got = Frame()
            frames.append((bridge.read_sensor("wrist_cam", got), got.stamp_ns))

    reader = threading.Thread(target=sensor_thread)
    reader.start()
    stepped_ok = True
    deadline = time.monotonic() + 0.5
    while time.monotonic() < deadline:
        rc, state = read(bridge)
        stepped_ok = stepped_ok and rc == rhal.SKR_OK
        published.add(state.stamp_ns)
        stepped_ok = stepped_ok and command(bridge, [0.3] * 6) == rhal.SKR_OK
    done.set()
    reader.join(timeout=10)
    check(
        "embodiment.descriptor.valid",
        "read concurrently with read_state and write_command, every frame and every step succeeds",
        stepped_ok and bool(frames) and all(rc == rhal.SKR_OK for rc, _ in frames),
        f"{len(frames)} frame(s), codes {sorted({rc for rc, _ in frames})}",
    )
    check(
        "embodiment.descriptor.valid",
        "and every frame is stamped with a state the stepping thread published",
        all(stamp in published for _, stamp in frames),
        f"{sum(1 for _, s in frames if s not in published)} of {len(frames)} were not",
    )
    bridge.close()


def concurrency_checks() -> None:
    """§89.1's promise, held without a GL context: the control thread never waits on a frame.

    A stand-in renderer that can be stopped mid-frame, behind the real library for everything else.
    With a render held open, `read_state` and `write_command` must still complete — a driver that
    shared a lock between stepping and rendering (the issue's own first sketch took one around
    `mjv_updateScene`) deadlocks here rather than passing. Needs no OpenGL, so a headless CI runner
    checks the arrangement even though it cannot check a picture.
    """
    import numpy  # noqa: PLC0415 — MuJoCo's own dependency, so present wherever this runs
    import mujoco  # noqa: PLC0415

    from skynet_mujoco import render as render_mod  # noqa: PLC0415

    gate = threading.Event()
    entered = threading.Event()

    class _Held:
        def __init__(self, model, height, width) -> None:
            self.shape = (height, width, 3)

        def update_scene(self, data, camera=None) -> None:
            pass

        def render(self):
            entered.set()
            gate.wait(timeout=30)
            return numpy.zeros(self.shape, dtype=numpy.uint8)

        def close(self) -> None:
            pass

    class _Holding:
        """The real library, with a renderer this harness can stop in the middle of a frame."""

        Renderer = _Held

        def __getattr__(self, name):
            return getattr(mujoco, name)

    gate.set()  # the probe at `open` renders once and must not be held
    bridge = MujocoBridge(mujoco=_Holding())
    assert bridge.open(json.dumps({"scene": scene("so_arm_shaped")})) == rhal.SKR_OK
    check(
        "embodiment.descriptor.valid",
        "a render thread that answers is what the declaration rests on",
        json.loads(bridge.describe())["sensors"][0].get("concurrent_read") is True,
    )

    rc, before = read(bridge)
    gate.clear()
    entered.clear()
    frame = Frame()
    result: dict[str, int] = {}
    sensor = threading.Thread(target=lambda: result.update(rc=bridge.read_sensor("wrist_cam", frame)))
    sensor.start()
    in_flight = entered.wait(timeout=10)

    codes: list[int] = []
    stamps: list[int] = []

    def control() -> None:
        for step in range(50):
            rc, state = read(bridge)
            codes.append(rc)
            stamps.append(state.stamp_ns)
            codes.append(command(bridge, [0.2] * 6, seq=step + 2))

    stepping = threading.Thread(target=control)
    stepping.start()
    stepping.join(timeout=10)
    blocked = stepping.is_alive()
    still_rendering = sensor.is_alive()
    gate.set()
    stepping.join(timeout=10)
    sensor.join(timeout=10)

    check(
        "embodiment.descriptor.valid",
        "with a frame held mid-render, 50 read_state and 50 write_command calls still complete",
        in_flight and not blocked and still_rendering and codes and all(c == rhal.SKR_OK for c in codes),
        f"render entered {in_flight}, control thread blocked {blocked}, "
        f"render still held when it finished {still_rendering}, codes {sorted(set(codes))}",
    )
    check(
        "embodiment.descriptor.valid",
        "and the held frame is the state it was asked for, not one the physics reached meanwhile",
        result.get("rc") == rhal.SKR_OK and frame.stamp_ns == before.stamp_ns and stamps
        and stamps[-1] > before.stamp_ns,
        f"frame {frame.stamp_ns}, asked at {before.stamp_ns}, stepped to {stamps[-1] if stamps else None}",
    )

    # A render that never returns must not become a read that never returns: the runtime's capture
    # thread is waiting on it, and a clean shutdown is waiting on that (§89.3).
    patched = render_mod.RENDER_TIMEOUT_S
    render_mod.RENDER_TIMEOUT_S = 0.2
    try:
        gate.clear()
        hung = bridge.read_sensor("wrist_cam", Frame())
        gate.set()
        after = bridge.read_sensor("wrist_cam", Frame())
    finally:
        render_mod.RENDER_TIMEOUT_S = patched
        gate.set()
    check(
        "embodiment.descriptor.valid",
        "a render that does not come back is a failed read, and the camera is not read again",
        hung == rhal.SKR_EIO and after == rhal.SKR_EIO,
        f"the hung read returned {hung}, the next {after}",
    )
    bridge.close()


def probe_checks() -> None:
    """The renderer probe, against a MuJoCo whose renderer refuses.

    The branch that matters most is the one this machine may not be able to take naturally, so it is
    provoked: a stand-in for the library whose `Renderer` raises. What is being checked is that a
    failure to render becomes *no declared sensor* rather than a declared sensor that fails to read.
    """
    import mujoco  # noqa: PLC0415 — deliberately local; this module is importable without it

    class _Refuses:
        mjtObj = mujoco.mjtObj
        attempts = 0

        @staticmethod
        def mj_id2name(model, obj, index):
            return mujoco.mj_id2name(model, obj, index)

        @classmethod
        def Renderer(cls, model, height, width):  # noqa: N802 — mirrors MuJoCo's own name
            cls.attempts += 1
            raise RuntimeError("no OpenGL context")

    model = mujoco.MjModel.from_xml_path(scene("so_arm_shaped"))
    data = mujoco.MjData(model)
    cameras = probe_cameras(_Refuses, model, data, size=None)
    check(
        "embodiment.descriptor.valid",
        "a renderer that refuses produces no declared cameras",
        cameras.names == [] and not cameras.available,
    )
    check(
        "embodiment.descriptor.valid",
        "and a note naming the fix rather than the traceback",
        "MUJOCO_GL" in cameras.note,
        cameras.note,
    )

    # The second probe must not reach MuJoCo at all. A retry after a failed context aborts the
    # process from inside the native library — no exception to catch — so a supervisor restarting a
    # body on a headless machine would take the interpreter with it.
    again = probe_cameras(_Refuses, model, data, size=None)
    check(
        "embodiment.descriptor.valid",
        "a failed probe is remembered rather than retried, which would abort the process",
        _Refuses.attempts == 1 and again.note == cameras.note,
        f"{_Refuses.attempts} attempts",
    )


def size_checks() -> None:
    """The RHAL render size, without a GL context (DRY-COVERAGE-PLAN.md F172).

    A scene resolution whose RGB8 frame the RHAL pipe cannot carry — the fleet's Gen3 declares
    640x480 — is rendered at `DEFAULT_SIZE` instead, and the note names both sizes. An explicit
    `render` size is the operator's word and is honoured as given, with a note when reads will be
    refused. Pure sizing, so no context and no MuJoCo: a namespace with `cam_resolution` stands in
    for the model.
    """
    from types import SimpleNamespace

    gen3 = SimpleNamespace(cam_resolution=[[640, 480]])
    (width, height), note = _rhal_size(None, gen3, 0)
    check(
        "embodiment.descriptor.valid",
        "a scene resolution the pipe cannot carry renders at the default instead",
        (width, height) == DEFAULT_SIZE,
        f"{width}x{height}",
    )
    check(
        "embodiment.descriptor.valid",
        "and the note names both sizes",
        note is not None and "640x480" in note and f"{DEFAULT_SIZE[0]}x{DEFAULT_SIZE[1]}" in note,
        str(note),
    )

    small = SimpleNamespace(cam_resolution=[[64, 48]])
    (width, height), note = _rhal_size(None, small, 0)
    check(
        "embodiment.descriptor.valid",
        "a scene resolution that fits is rendered as the scene declares it",
        (width, height) == (64, 48) and note is None,
        f"{width}x{height} {note}",
    )

    (width, height), note = _rhal_size((640, 480), gen3, 0)
    check(
        "embodiment.descriptor.valid",
        "an explicit render size is the operator's word and is honoured as given",
        (width, height) == (640, 480),
        f"{width}x{height}",
    )
    check(
        "embodiment.descriptor.valid",
        "with a note saying reads will be refused rather than framed",
        note is not None and "refused" in note,
        str(note),
    )


def import_checks() -> None:
    """A MuJoCo that is installed and will not import, because its GL backend will not load.

    MuJoCo resolves `MUJOCO_GL` while it is imported, so `MUJOCO_GL=osmesa` on a machine without
    libOSMesa is not the renderer-probe failure above — it is `import mujoco` raising, and not with
    an `ImportError`. Provoked here with a finder that raises what MuJoCo 3.13 raised on the machine
    that found it, so the branch is held on every machine rather than only on one without OSMesa.
    """
    import contextlib
    import importlib.abc
    import io

    class _BackendWillNotLoad(importlib.abc.MetaPathFinder):
        def find_spec(self, name, path, target=None):
            if name == "mujoco":
                raise AttributeError("'NoneType' object has no attribute 'glGetError'")
            return None

    saved = {name: module for name, module in sys.modules.items()
             if name == "mujoco" or name.startswith("mujoco.")}
    for name in saved:
        del sys.modules[name]
    sys.meta_path.insert(0, _BackendWillNotLoad())
    said = io.StringIO()
    try:
        with contextlib.redirect_stderr(said):
            rc = MujocoBridge().open(json.dumps({"scene": scene("so_arm_shaped")}))
    except Exception as error:  # noqa: BLE001 — the defect this holds is exactly an escape
        rc = f"raised {type(error).__name__}"
    finally:
        sys.meta_path.pop(0)
        sys.modules.update(saved)
    check(
        "rhal.abi.version",
        "a MuJoCo whose GL backend will not load is refused as no device, not raised out of open()",
        rc == rhal.SKR_ENODEV,
        f"open() gave {rc!r}",
    )
    check(
        "rhal.abi.version",
        "and the refusal names MUJOCO_GL and the library to install",
        "MUJOCO_GL=" in said.getvalue() and "libosmesa6" in said.getvalue(),
        said.getvalue()[:200],
    )


# ── The wire protocol ────────────────────────────────────────────────────────


def protocol_checks() -> None:
    """Drive `serve()` through in-memory pipes, as the LeRobot harness does.

    `drivers/python-host` is C and is only exercised where a C compiler is, so the protocol itself is
    held here, where every platform runs it.
    """
    import io

    class _Pipe(io.StringIO):
        def feed(self, text: str) -> None:
            here = self.tell()
            self.seek(0, io.SEEK_END)
            super().write(text)
            self.seek(here)

    inbox, outbox = _Pipe(), io.StringIO()
    config = json.dumps({"scene": scene("so_arm_shaped")})
    inbox.feed(
        "\n".join(
            [
                f"O {len(config)}",
                config,
                "D",
                "S 0",
                "C 1 1 0 6 0.1 0.1 0.1 0.1 0.1 0.1 1 1 1 1 1 1",
                "E",
                "X",
            ]
        )
        + "\n"
    )

    rhal.serve(MujocoBridge(), ceiling="guest", name="mujoco-bridge", version="0", stdin=inbox, stdout=outbox)
    lines = outbox.getvalue().split("\n")

    # The ABI is pinned and the host protocol moves with `drivers/python-host` (§89.1 took it
    # to 2), so this reads the second number rather than restating it.
    check("rhal.abi.version", "the handshake comes first and declares guest",
          lines[0].startswith(f"H 1 {rhal._PROTOCOL_VERSION} guest "), lines[0])
    check("rhal.abi.version", "open is acknowledged", lines[1] == "R 0", lines[1])

    declared = int(lines[2].split()[1])
    check(
        "embodiment.descriptor.valid",
        "the descriptor's length prefix matches its body",
        len(lines[3].encode("utf-8")) == declared,
        f"{declared} vs {len(lines[3].encode('utf-8'))}",
    )
    check(
        "embodiment.descriptor.valid",
        "and the simulator flag crosses the wire",
        '"simulator":true' in lines[3],
    )

    state = lines[4].split()
    check(
        "rhal.state.completeness",
        "the state frame carries three arrays of the joint count",
        state[0] == "S" and state[1] == "0" and int(state[6]) == 6 and len(state) == 7 + 18,
        lines[4],
    )
    check("rhal.command.rejection", "a command is acknowledged", lines[5] == "R 0", lines[5])
    check("safety.estop.latency", "the stop is acknowledged", lines[6] == "R 0", lines[6])
    check("rhal.abi.version", "close is acknowledged and ends the loop", lines[7] == "R 0", lines[7])


def scts_ids() -> set[str] | None:
    """The SCTS ids `skynet-contracts` defines, or None where that source is not beside us.

    Reading the Rust rather than restating the list: a harness naming a test that does not exist
    reads exactly like one naming a test that does, and this session started by making that mistake.
    A vendor who cloned only `drivers/` gets `None` and no check, which is the right trade — their
    tree is not broken because a crate is missing.
    """
    source = os.path.join(
        os.path.dirname(os.path.abspath(__file__)),
        "..", "..", "crates", "skynet-contracts", "src", "certification.rs",
    )
    if not os.path.exists(source):
        return None
    with open(source, encoding="utf-8") as handle:
        body = handle.read()
    return set(re.findall(r'(?:test|quant)\(\s*"([a-z][a-z0-9_.]+)"', body))


def id_checks() -> None:
    known = scts_ids()
    if known is None:
        skip("rhal.abi.version", "the ids named here are real SCTS tests",
             "crates/skynet-contracts is not beside this driver, so there is nothing to check "
             "against.")
        return
    used = {line.split(":")[0] for line in _FAILED} | _USED_IDS
    unknown = sorted(id for id in used if id not in known)
    check(
        "rhal.abi.version",
        "every SCTS id this harness names is one SCTS defines",
        not unknown,
        f"not in SCTS_TESTS: {unknown}. §52 reserves some ids for measures that do not exist yet; "
        "naming one here reads exactly like naming one that does.",
    )


def main() -> int:
    print("skynet-mujoco — sim conformance against MuJoCo physics\n")
    abi_checks()
    state_checks()
    command_checks()
    gear_checks()
    mixed_mode_checks()
    estop_checks()
    descriptor_checks()
    frame_checks()
    exclusion_checks()
    sensor_checks()
    concurrency_checks()
    probe_checks()
    size_checks()
    import_checks()
    protocol_checks()
    id_checks()

    total = _PASSED + len(_FAILED)
    print(f"\n{total} checks, {len(_FAILED)} failed, {len(_SKIPPED)} skipped")
    for failure in _FAILED:
        print(f"  FAIL {failure}")
    for skipped in _SKIPPED:
        print(f"  skip {skipped}")
    if _FAILED:
        return 1

    print(
        "\nGuest tier is the ceiling and nothing here changes it — twice over: Python cannot hold a "
        "real-time path (§31.3), and a simulator is forced to guest by §35.3 so that no simulated "
        "run is ever presented as bench evidence. What this run shows is that the body, the "
        "descriptor, the clock and the command path compose into something that runs."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

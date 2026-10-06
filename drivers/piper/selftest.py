"""skynet-piper's own suite: the protocol, the simulated arm, and every refusal the bridge makes.

No test framework and no kernel bus. `drivers/odrive/selftest.py`'s shape and emphasis: nearly everything asserted here
is a **refusal**, and every refusal is written beside the nearly-identical input that succeeds. A Piper driver's failure
mode is not a crash — it is an arm that reports a plausible angle and moves somewhere else, or one that is told to move
and ignores it because it is in another control mode — so a suite that only drove the happy path would be checking the
one thing that cannot go wrong quietly.

    make selftest     # this file
    make check        # and then the whole path through the real ABI
    make check-vcan   # the same driver on a real SocketCAN interface, where there is one
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import struct
import subprocess
import sys

import skynet_rhal as rhal
from skynet_canopen.bus import Bus, LoopbackBus, SocketCanBus, can_available
from skynet_canopen.frames import Frame

from skynet_piper import protocol as p
from skynet_piper.arm import SimulatedPiper
from skynet_piper.bridge import PiperBridge

FAILURES: list[str] = []
CHECKS = 0
#: Every frame any bridge in this suite ever sent, so one check at the end can say the maker's reset byte never went out.
EVERYTHING_SENT: list[Frame] = []


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")


def section(title: str) -> None:
    print(f"\n{title}")


class RecordingEndpoint(Bus):
    """A loopback endpoint that remembers every frame the bridge sent, in order: what the bridge actually put on the wire,
    which no arm's final state can say (an arm that ignored a frame ends in the same state as one never sent it)."""

    synchronous = True

    def __init__(self, inner: Bus) -> None:
        self._inner = inner
        self.sent: list[Frame] = []

    def send(self, frame: Frame) -> None:
        self.sent.append(frame)
        EVERYTHING_SENT.append(frame)
        self._inner.send(frame)

    def recv(self, timeout_s: float = 0.0) -> Frame | None:
        return self._inner.recv(timeout_s)

    def backlog(self) -> int | None:
        return self._inner.backlog()

    def ids(self) -> list[int]:
        return [frame.arbitration_id for frame in self.sent]

    def of(self, ident: int) -> list[bytes]:
        return [bytes(frame.data) for frame in self.sent if frame.arbitration_id == ident]

    def clear(self) -> None:
        self.sent.clear()


class ManualClock:
    """A clock the test owns, for the staleness bound."""

    def __init__(self) -> None:
        self.now = 1000.0

    def __call__(self) -> float:
        return self.now

    def advance(self, seconds: float) -> None:
        self.now += seconds


def build(gripper: bool = True, clock=None, **arm_args) -> tuple[PiperBridge, SimulatedPiper, RecordingEndpoint, LoopbackBus]:
    loopback = LoopbackBus()
    master = RecordingEndpoint(loopback.endpoint())
    arm = SimulatedPiper(loopback.endpoint(), gripper=gripper, **arm_args)
    loopback.attach(arm.poll)
    bridge = PiperBridge(master, clock=clock) if clock is not None else PiperBridge(master)
    return bridge, arm, master, loopback


def config(**overrides) -> str:
    settings: dict = {"speed_percent": 30, "gripper": True, "gripper_torque_nm": 1.0}
    settings.update(overrides)
    return json.dumps({key: value for key, value in settings.items() if value is not None})


def opened(gripper: bool = True, clock=None, **overrides) -> tuple[PiperBridge, SimulatedPiper, RecordingEndpoint, LoopbackBus]:
    bridge, arm, master, loopback = build(gripper=gripper, clock=clock)
    settings = {"gripper": gripper}
    if not gripper:
        settings["gripper_torque_nm"] = None
    settings.update(overrides)
    result = bridge.open(config(**settings))
    assert result == rhal.SKR_OK, f"open returned {result}"
    return bridge, arm, master, loopback


def command(values: list[float], mask: list[int] | None = None, mode: int = rhal.MODE_POSITION, modes: list[int] | None = None) -> rhal.Command:
    return rhal.Command(mode=mode, values=list(values), mask=list(mask if mask is not None else [1] * len(values)), modes=list(modes or []))


def read(bridge: PiperBridge) -> tuple[int, rhal.State]:
    state = rhal.State()
    return bridge.read_state(state, 0), state


def settle(bridge: PiperBridge, reads: int = 80) -> rhal.State:
    state = rhal.State()
    for _ in range(reads):
        bridge.read_state(state, 0)
    return state


def near(a: float, b: float, tolerance: float = 2e-3) -> bool:
    return abs(a - b) <= tolerance


def said(call) -> tuple[object, str]:
    """What a call returned and what it told the operator on stderr: a refusal is its reason as much as its code, and two
    different reasons for the same code are two different next actions."""
    captured = io.StringIO()
    with contextlib.redirect_stderr(captured):
        result = call()
    return result, captured.getvalue()


GOOD = [0.2, 0.5, -0.5, 0.1, 0.1, 0.1, 0.03]


def main() -> int:
    print("skynet-piper selftest\n")

    # ── the wire ─────────────────────────────────────────────────────────────
    section("the wire — big-endian, in the maker's own layouts")
    check(p.motion_ctrl_1(emergency_stop=1) == bytes([1, 0, 0, 0, 0, 0, 0, 0]), "0x150 carries the fast stop in its first byte")
    check(p.motion_ctrl_2(p.CTRL_CAN, p.MOVE_J, 30) == bytes([1, 1, 30, 0, 0, 0, 0, 0]), "0x151 is control mode, move mode, speed percent")
    check(p.joint_ctrl(1000, -1000) == bytes.fromhex("000003e8fffffc18"), "a joint pair is two big-endian int32 counts of 0.001°")
    check(p.gripper_ctrl(70000, 1000, 1) == bytes.fromhex("00011170" + "03e8" + "0100"), "0x159 is stroke int32, torque uint16, enable code, zero command")
    check(p.motor_enable(0xFF, 2) == bytes([0xFF, 2, 0, 0, 0, 0, 0, 0]), "0x471 enables every motor with a 2")
    check(p.limit_query(3) == bytes([3, 1, 0, 0, 0, 0, 0, 0]), "0x472 asks a joint for its angle and speed limits")
    status = p.parse_status(bytes([1, 0, 1, 0, 0, 0, 0x12, 0x03]))
    check(status.ctrl_mode == 1 and status.arm_status == 0 and status.err_code == 0x1203, "0x2A1 reads the error word as one big-endian uint16")
    check(status.limit_bits() == 0x12 and status.comms_bits() == 0x03, "…with the angle-limit bits in byte 6 and the communication bits in byte 7")
    check(p.parse_joint_pair(bytes.fromhex("00000064ffffff9c")) == (100, -100), "a joint pair reads two signed counts")
    check(p.parse_gripper(bytes.fromhex("0000271000c8" + "40" + "00")) == (10000, 200, 0x40), "the gripper frame is stroke, torque and a status byte")
    check(p.parse_high_speed(bytes.fromhex("fff6000a" + "00000064")) == p.HighSpeed(-10, 10, 100), "a driver's fast report is speed, current, position — signed")
    low = p.parse_low_speed(bytes.fromhex("00f0" + "0023" + "28" + "40" + "0000"))
    check(low == p.LowSpeed(240, 35, 40, 0x40, 0) and low.enabled() and low.faults() == 0, "a driver's slow report: volts, two temperatures, status, current")
    check(p.parse_low_speed(bytes.fromhex("00f0" + "0023" + "fb" + "c0" + "0000")) == p.LowSpeed(240, 35, -5, 0xC0, 0),
          "a motor temperature is signed and the status byte is not: -5 °C is -5, and 0xC0 is 0xC0 and not -64")
    check(p.parse_low_speed(bytes.fromhex("00f0" + "0023" + "28" + "25" + "0000")).faults() == (p.DRIVER_VOLTAGE_LOW | p.DRIVER_OVERCURRENT | p.DRIVER_ERROR),
          "the status byte's fault bits are read as the maker names them")
    report = p.parse_limit_report(bytes.fromhex("01" + "05dc" + "fa24" + "0bb8" + "00"))
    check(report == p.LimitReport(1, 1500, -1500, 3000), "a limit report is joint, max and min angle in 0.1° (signed), speed")
    check(near(report.max_rad(), math.radians(150.0), 1e-9), "…and 1500 is 150°")
    check(near(report.min_rad(), math.radians(-150.0), 1e-9), "…and its minimum, -1500, is -150°: both ends are in 0.1°")
    for builder, label in ((lambda: p.parse_status(bytes(7)), "status"), (lambda: p.parse_joint_pair(bytes(9)), "a joint pair"),
                           (lambda: p.parse_gripper(bytes(4)), "gripper"), (lambda: p.parse_low_speed(bytes(0)), "low speed"),
                           (lambda: p.parse_high_speed(bytes(7)), "high speed"), (lambda: p.parse_limit_report(bytes(8)[:6]), "limit report")):
        try:
            builder()
            refused = False
        except ValueError:
            refused = True
        check(refused, f"a {label} frame of the wrong length is refused, not truncated")
    check(p.rad_to_mdeg(math.pi) == 180000 and p.rad_to_mdeg(-math.pi / 2) == -90000, "radians become 0.001° counts")
    check(near(p.mdeg_to_rad(180000), math.pi, 1e-12), "…and back")
    for bad in (math.nan, math.inf, -math.inf, 1e9):
        try:
            p.rad_to_mdeg(bad)
            refused = False
        except ValueError:
            refused = True
        check(refused, f"{bad!r} is not an angle the wire can carry")
    check(p.metres_to_counts(0.07) == 70000 and p.counts_to_metres(35000) == 0.035, "a gripper stroke is 0.001 mm counts")
    try:
        p.metres_to_counts(math.nan)
        refused = False
    except ValueError:
        refused = True
    check(refused, "a NaN stroke is not a number on the wire")
    try:
        p.motion_ctrl_2(1, 1, 101)
        refused = False
    except ValueError:
        refused = True
    check(refused, "a speed above 100 % is refused before it is a byte")
    check(not hasattr(p, "RESET") and not hasattr(p, "RESUME") and p._RESUME_IS_A_RESET == 2,
          "the maker's resume byte is named for what it is and exported to nobody")
    check(p.JOINT_LIMITS_RAD == ((-2.6179, 2.6179), (0.0, 3.14), (-2.967, 0.0), (-1.745, 1.745), (-1.22, 1.22), (-2.09439, 2.09439))
          and p.GRIPPER_RANGE_M == (0.0, 0.07),
          "the published travel is the maker's table, joint by joint, written out here and not read back from the constant it is checked against")

    # The identifiers every other check here is written in terms of. A wrong constant agrees with itself everywhere, so they are pinned as
    # literals; `layout_check.py` holds the same values to the maker's own codec, which needs the SDK this suite does without.
    check((p.ID_STATUS, p.ID_JOINTS_12, p.ID_JOINTS_34, p.ID_JOINTS_56, p.ID_GRIPPER) == (0x2A1, 0x2A5, 0x2A6, 0x2A7, 0x2A8),
          "what the arm broadcasts is on 0x2A1 (status), 0x2A5 to 0x2A7 (the joint pairs) and 0x2A8 (the gripper)")
    check((p.ID_HIGH_SPEED, p.ID_LOW_SPEED) == (0x251, 0x261), "…each driver's fast report on 0x251 to 0x256 and its slow one, the one that carries the health byte, on 0x261 to 0x266")
    check((p.ID_MOTION_1, p.ID_MOTION_2, p.ID_JOINT_CTRL, p.ID_GRIPPER_CTRL) == (0x150, 0x151, (0x155, 0x156, 0x157), 0x159),
          "what a master sends is on 0x150 (the stop), 0x151 (the mode), 0x155 to 0x157 (the joint pairs) and 0x159 (the gripper)")
    check((p.ID_ENABLE, p.ID_LIMIT_QUERY, p.ID_LIMIT_REPORT) == (0x471, 0x472, 0x473), "…and the enable, the limit query and its answer on 0x471, 0x472 and 0x473")
    for torque, accepted in ((0, True), (5000, True), (5001, False), (50_000, False), (-1, False)):
        try:
            p.gripper_ctrl(0, torque, 1)
            built = True
        except ValueError:
            built = False
        check(built == accepted, f"a gripper torque of {torque} (0.001 N·m) is {'built into a frame' if accepted else 'refused, as the maker library refuses a value outside 0 to 5000'}")

    # ── the simulated arm ────────────────────────────────────────────────────
    section("the simulated arm — what it broadcasts, and what it does with a frame")
    loop = LoopbackBus()
    me = loop.endpoint()  # not a recording one: this section sends the maker's reset byte on purpose, to the model, and no bridge does
    arm = SimulatedPiper(loop.endpoint())
    loop.attach(arm.poll)
    me.recv()
    seen = set()
    for _ in range(40):
        frame = me.recv()
        if frame is None:
            break
        seen.add(frame.arbitration_id)
        if len(seen) >= 17:
            break
    check({p.ID_STATUS, p.ID_JOINTS_12, p.ID_JOINTS_34, p.ID_JOINTS_56, p.ID_GRIPPER} <= seen, "one cycle carries the status, the three joint pairs and the gripper")
    check(all(p.ID_HIGH_SPEED + j in seen and p.ID_LOW_SPEED + j in seen for j in range(6)), "…and each driver's two reports")
    me.send(Frame(p.ID_JOINT_CTRL[0], p.joint_ctrl(10_000, 20_000)))
    arm.poll()
    check(arm.target[:2] == [0, 0], "a target before the arm is in CAN control with its drivers enabled is ignored")
    me.send(Frame(p.ID_ENABLE, p.motor_enable(0xFF, 2)))
    me.send(Frame(p.ID_MOTION_2, p.motion_ctrl_2(p.CTRL_CAN, p.MOVE_J, 50)))
    arm.poll()
    me.send(Frame(p.ID_JOINT_CTRL[0], p.joint_ctrl(10_000, 20_000)))
    arm.poll()
    check(arm.target[:2] == [10_000, 20_000], "…and it is taken once it is")
    me.send(Frame(p.ID_JOINT_CTRL[0], p.joint_ctrl(400_000, 0)))
    arm.poll()
    check(arm.arm_status == 0x04 and arm.err_code & (1 << p.ERR_LIMIT_SHIFT) and arm.target[0] == 10_000,
          "a target past the arm's own limit is refused by the arm: status 4, the joint's limit bit, no motion")
    me.send(Frame(p.ID_MOTION_1, p.motion_ctrl_1(emergency_stop=1)))
    arm.poll()
    check(arm.arm_status == p.ARM_STATUS_EMERGENCY_STOP and arm.target == list(arm.mdeg), "the fast stop latches arm status 1 and stops every joint where it is")
    me.send(Frame(p.ID_MOTION_1, p.motion_ctrl_1(emergency_stop=2)))
    arm.poll()
    check(arm.resets == 1 and arm.dropped and not any(arm.enabled), "the maker's resume byte is a reset that drops the arm: every driver disabled — which only the model in this file knows")

    # ── open: every refusal, beside the input that succeeds ──────────────────
    section("open — a body that does not come up says why")
    bridge, arm, master, loop = build()
    check(bridge.open("not json") == rhal.SKR_EINVAL, "a configuration that is not JSON is refused")
    check(bridge.open("[1, 2]") == rhal.SKR_EINVAL, "…or JSON that is not an object")
    for bad in (None, 0, 101, True, "30", 12.5):
        bridge, arm, master, loop = build()
        check(bridge.open(config(speed_percent=bad)) == rhal.SKR_ENODEV and not master.sent,
              f"`speed_percent` of {bad!r} is refused with nothing sent: a speed has no default")
    bridge, arm, master, loop = build()
    check(bridge.open(json.dumps({"gripper": True, "gripper_torque_nm": 1.0})) == rhal.SKR_ENODEV, "no `speed_percent` at all is refused")
    for bad in (None, 1, "yes"):
        bridge, arm, master, loop = build()
        check(bridge.open(config(gripper=bad)) == rhal.SKR_ENODEV, f"`gripper` of {bad!r} is refused: whether there is a hand is not on the wire")
    for bad in (None, 0, -1, 5.01, True, "1"):
        bridge, arm, master, loop = build()
        check(bridge.open(config(gripper_torque_nm=bad)) == rhal.SKR_ENODEV, f"`gripper_torque_nm` of {bad!r} is refused with a hand declared: a grip force has no default")
    bridge, arm, master, loop = build(gripper=False)
    check(bridge.open(config(gripper=False, gripper_torque_nm=None)) == rhal.SKR_OK, "…while an arm declared without a hand needs no torque")
    for bad in (0, -5, 1001, True, "50"):
        bridge, arm, master, loop = build()
        check(bridge.open(config(window_ms=bad)) == rhal.SKR_ENODEV, f"`window_ms` of {bad!r} is not a window")
    bridge, arm, master, loop = build()
    check(bridge.open(config(roles="oops")) == rhal.SKR_ENODEV, "`roles` that is not an object is refused")
    check(bridge.open(config(roles={"joint9": "right_arm.shoulder.yaw"})) == rhal.SKR_ENODEV, "a role for a joint the arm does not have is refused — a typo, loudly")
    check(bridge.open(config(roles={"joint1": "nonsense"})) == rhal.SKR_ENODEV, "a role outside §5.1's grammar is refused")
    for bad_role, reason in (("hovercraft.hull", "a chain §5.1 does not define"), ("right_arm.shoulder.twist", "an axis §5.1 does not define"),
                             ("right_arm.shoulder.yaw.extra", "a fourth part"), ("right_arm.shoulder#", "an index with no number")):
        bridge, arm, master, loop = build()
        check(bridge.open(config(roles={"joint1": bad_role})) == rhal.SKR_ENODEV and not master.sent, f"a role of `{bad_role}` is refused with nothing sent: {reason}")
    for good_role in ("right_arm.shoulder", "torso.waist.roll#2"):
        bridge, arm, master, loop = build()
        check(bridge.open(config(roles={"joint1": good_role})) == rhal.SKR_OK, f"…while `{good_role}` is a role")
    check(bridge.open(config(roles={"gripper": "right_hand.grip"}, gripper=False, gripper_torque_nm=None)) == rhal.SKR_ENODEV,
          "…and so is the gripper's role on an arm declared without one")
    bridge, arm, master, loop = build(powered=False)
    check(bridge.open(config()) == rhal.SKR_ENODEV, "an arm that is off sends no status, and the body does not come up")
    bridge, arm, master, loop = build()
    arm.mute = {p.ID_JOINTS_34}
    check(bridge.open(config()) == rhal.SKR_ENODEV, "an arm that broadcasts no angles for joints 3 and 4 is refused: a fault would sit unread")
    bridge, arm, master, loop = build()
    arm.mute = {p.ID_LOW_SPEED + 2}
    check(bridge.open(config()) == rhal.SKR_ENODEV, "…and so is one that never sends joint 3's driver report")
    bridge, arm, master, loop = build(gripper=False)
    check(bridge.open(config()) == rhal.SKR_ENODEV, "a configuration that declares a gripper the arm does not broadcast is refused")
    for code in range(1, 16):
        bridge, arm, master, loop = build()
        arm.arm_status = code
        check(bridge.open(config()) == rhal.SKR_ENODEV and p.ID_ENABLE not in master.ids(),
              f"arm status {code:#04x} ({p.ARM_STATUSES[code]}) is refused at open, before any enable")
    bridge, arm, master, loop = build()
    arm.err_code = 1 << p.ERR_COMMS_SHIFT
    check(bridge.open(config()) == rhal.SKR_ENODEV and p.ID_ENABLE not in master.ids(), "a standing error word is refused at open")
    for bit, name in p.GRIPPER_BIT_NAMES.items():
        bridge, arm, master, loop = build()
        arm.gripper_flags = bit
        check(bridge.open(config()) == rhal.SKR_ENODEV and p.ID_ENABLE not in master.ids(),
              f"a gripper reporting `{name}` is refused at open, before any enable: its status byte is health too")
    bridge, arm, master, loop = build()
    arm.gripper_stuck = True
    check(bridge.open(config()) == rhal.SKR_ENODEV, "a gripper that never reports itself enabled is refused: an arm with a hand that will not take commands is not the body that was declared")
    bridge, arm, master, loop = build()
    arm.gripper_flags = p.GRIPPER_ZEROED  # already set by the model: zeroing state is not a fault, in either state
    check(bridge.open(config()) == rhal.SKR_OK, "…while the gripper's zeroing bit is no fault, set or clear")
    for bit in (p.DRIVER_VOLTAGE_LOW, p.DRIVER_MOTOR_OVERHEAT, p.DRIVER_OVERCURRENT, p.DRIVER_OVERHEAT, p.DRIVER_COLLISION_PROTECTION, p.DRIVER_ERROR, p.DRIVER_STALL_PROTECTION):
        bridge, arm, master, loop = build()
        arm.driver_flags[3] = bit
        check(bridge.open(config()) == rhal.SKR_ENODEV and p.ID_ENABLE not in master.ids(),
              f"a driver reporting `{p.DRIVER_BIT_NAMES[bit]}` is refused at open, before any enable")
    bridge, arm, master, loop = build()
    arm.deaf_to = {p.ID_LIMIT_QUERY}
    check(bridge.open(config()) == rhal.SKR_ENODEV, "an arm that does not answer a limit query is refused: it has not said which table is narrower")
    bridge, arm, master, loop = build()
    arm.limits[2] = (-100, 100)
    check(bridge.open(config()) == rhal.SKR_ENODEV, "a reversed angle range is refused, not guessed at")
    for label, limits in (("reversed", (-100, 100)), ("empty", (0, 0))):
        bridge, arm, master, loop = build()
        arm.limits[2] = limits
        code, text = said(lambda: bridge.open(config()))
        check(code == rhal.SKR_ENODEV and "empty or reversed" in text,
              f"a {label} angle range an arm reports is refused as that, by name, and not found out later as a joint outside its travel")

    # the reason a body does not come up is half of the refusal: two reasons for one code are two next actions
    bridge, arm, master, loop = build()
    arm.arm_status = p.ARM_STATUS_EMERGENCY_STOP
    arm.mute = {p.ID_JOINTS_34}
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "arm status 0x01" in text and "joint angles" not in text,
          "an arm that is in a stop and is also missing reports is refused for the stop, the first thing to look up, before anything is waited for")
    bridge, arm, master, loop = build(gripper=False)
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "the gripper (0x2A8)" in text,
          "a hand declared and not broadcast is refused for that, by name, and not for the enable it then never answers")
    bridge, arm, master, loop = build()
    arm.mdeg[1] = -50_000
    check(bridge.open(config()) == rhal.SKR_ENODEV and p.ID_ENABLE not in master.ids(),
          "an arm found outside its own travel is refused before it is enabled (the F246 shape)")
    bridge, arm, master, loop = build()
    arm.deaf_to = {p.ID_ENABLE}
    check(bridge.open(config()) == rhal.SKR_ENODEV, "an arm whose drivers never report enabled is refused")
    check(all(frame[1] != 0x01 for frame in master.of(p.ID_ENABLE)), "…and no disable was sent for it: an arm left as found cannot fall")
    bridge, arm, master, loop = build()
    arm.deaf_to = {p.ID_MOTION_2}
    check(bridge.open(config()) == rhal.SKR_ENODEV, "an arm that does not report CAN control after being asked is refused: its own request is not evidence")

    bridge, arm, master, loop = opened()
    check(True, "the same configuration, against a healthy arm, opens")
    ids = master.ids()
    check(len(master.of(p.ID_LIMIT_QUERY)) == 6, "…after asking each of the six joints for its limits")
    check(max(i for i, x in enumerate(ids) if x == p.ID_LIMIT_QUERY) < ids.index(p.ID_ENABLE) < ids.index(p.ID_MOTION_2),
          "…the limits first, then the enable, then the mode: nothing is enabled before the travel is known")
    enables = master.of(p.ID_ENABLE)
    check([frame[0] for frame in enables][:7] == [1, 2, 3, 4, 5, 6, 7] and all(frame[1] == 0x02 for frame in enables) and all(frame[0] != 0xFF for frame in enables),
          "…enabling joints 1 to 6 by number and the gripper as 7, and never the 0xFF the maker's own sources disagree about (F284)")
    bridge_six, arm_six, master_six, _ = opened(gripper=False)
    check({frame[0] for frame in master_six.of(p.ID_ENABLE)} == {1, 2, 3, 4, 5, 6}, "an arm without a hand is never sent a 7, which on one reading of the maker's documentation would mean all of them")
    check(arm.ctrl_mode == p.CTRL_CAN and arm.move_mode == p.MOVE_J and arm.speed_percent == 30, "…and the arm is in CAN control, MOVE J, at the operator's speed")
    check(all(arm.enabled[:6]), "…with every joint driver enabled")

    # ── the descriptor ───────────────────────────────────────────────────────
    section("the descriptor — what the body says it is")
    descriptor = json.loads(bridge.describe())
    check(descriptor["robot_slug"] == "agilex-piper" and descriptor["dof"] == 7 and descriptor["base"] == "fixed", "a fixed seven-joint body")
    check([joint["name"] for joint in descriptor["joints"]] == list(p.JOINT_NAMES) + ["gripper"], "six joints and the gripper, named as the maker names them")
    check(descriptor["joints"][6]["type"] == "prismatic" and descriptor["joints"][0]["type"] == "revolute", "the gripper is prismatic and the rest revolute")
    check(all(joint["role"] is None for joint in descriptor["joints"]), "no role is declared that the operator did not give (§5.1: roles are never inferred)")
    check(descriptor["control"]["modes"] == ["position"] and descriptor["control"]["per_joint_modes"] is False, "position only, one mode for the body")
    check(all(joint["control_modes"] == ["position"] for joint in descriptor["joints"]), "…on every joint")
    check(descriptor["envelope"] is None, "no envelope: nothing published here is a rated one")
    check(descriptor["capabilities"] == ["safety.estop"], "the software stop it has is declared, and no rated stop")
    check(descriptor["simulator"] is True, "declared simulated: the default is the safe direction (§35.3)")
    expected = list(p.JOINT_LIMITS_RAD)
    check(all(near(descriptor["joints"][i]["limits"]["position_min_rad"], expected[i][0], 1e-3) and near(descriptor["joints"][i]["limits"]["position_max_rad"], expected[i][1], 1e-3) for i in range(6)),
          "each joint's travel is the maker's table, where the arm reports no narrower")
    check(descriptor["joints"][6]["limits"] == {"position_min_m": 0.0, "position_max_m": 0.07}, "the gripper's travel is the maker's 0 to 70 mm")
    bridge2, arm2, master2, loop2 = build()
    arm2.limits[0] = (1000, -1000)  # ±100°, narrower than the published ±150°
    arm2.limits[3] = (2000, -2000)  # ±200°, wider than the published ±100°
    assert bridge2.open(config()) == rhal.SKR_OK
    wide = json.loads(bridge2.describe())
    check(near(wide["joints"][0]["limits"]["position_max_rad"], math.radians(100.0), 1e-3), "a limit the arm reports narrower than the table is the one declared")
    check(near(wide["joints"][3]["limits"]["position_max_rad"], 1.745, 1e-3), "…and one it reports wider is not: the narrower of the two, never the wider (F181)")
    bridge3, arm3, master3, loop3 = build()
    assert bridge3.open(config(roles={"joint1": " Right_Arm.Shoulder.Yaw ", "joint2": "right_arm.shoulder.pitch"}, simulator=False)) == rhal.SKR_OK
    roled = json.loads(bridge3.describe())
    check(roled["joints"][0]["role"] == "right_arm.shoulder.yaw" and roled["joints"][1]["role"] == "right_arm.shoulder.pitch" and roled["joints"][2]["role"] is None,
          "roles the operator gave are normalised and declared, and only those")
    check(roled["simulator"] is True, "a loopback bus is a simulated body whatever the configuration says: there is demonstrably no arm on it")
    bridge4, arm4, master4, loop4 = build(gripper=False)
    assert bridge4.open(config(gripper=False, gripper_torque_nm=None)) == rhal.SKR_OK
    six = json.loads(bridge4.describe())
    check(six["dof"] == 6 and len(six["joints"]) == 6, "an arm declared without its hand has six joints")

    # ── read_state ───────────────────────────────────────────────────────────
    section("read_state — what the body is, and what it will not invent")
    bridge, arm, master, loop = opened()
    arm.poll()  # whatever the bring-up left waiting is the arm's to handle before the test stages it
    arm.mdeg = [30_000, 60_000, -90_000, 10_000, -20_000, 45_000]
    arm.target = list(arm.mdeg)
    arm.gripper_counts = arm.gripper_target = 25_000
    code, state = read(bridge)
    check(code == rhal.SKR_OK, "a read answers")
    check(all(near(a, b, 1e-4) for a, b in zip(state.position_rad, [math.radians(30), math.radians(60), math.radians(-90), math.radians(10), math.radians(-20), math.radians(45), 0.025])),
          "positions are the joint angles in radians and the gripper in metres")
    check(all(math.isnan(v) for v in state.velocity_rad_s), "velocity is NaN: the drivers report motor speed, and the gear ratio is not on the wire (§24)")
    check(all(math.isnan(e) for e in state.effort_nm), "effort is NaN: current is not torque without a torque constant (§24)")
    check(state.fault_code == 0 and state.flags == rhal.HOMING_UNREPORTED, "a healthy arm reads no flag and no fault")
    first_seq, first_stamp = state.seq, state.stamp_ns
    code, state = read(bridge)
    check(state.seq == first_seq + 1 and state.stamp_ns >= first_stamp, "the sequence advances by one a read and the stamp never goes back")
    check(len(state.position_rad) == 7, "seven positions for seven joints")
    bridge, arm, master, loop = opened(gripper=False)
    check(len(read(bridge)[1].position_rad) == 6, "six for an arm without its hand")

    # ── faults, read in the maker's words ────────────────────────────────────
    section("faults — refused, never cleared, and the word is the maker's")
    for code_value in range(1, 16):
        bridge, arm, master, loop = opened()
        arm.arm_status = code_value
        code, state = read(bridge)
        check(code == rhal.SKR_OK and state.flags & rhal.FLAG_FAULT and state.flags & rhal.FLAG_DEGRADED and (state.fault_code >> 16) & 0xFF == code_value,
              f"arm status {code_value:#04x} reads FAULT and DEGRADED with the status in the fault word")
    bridge, arm, master, loop = opened()
    arm.err_code = (1 << (p.ERR_LIMIT_SHIFT + 2)) | (1 << (p.ERR_COMMS_SHIFT + 4))
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_FAULT and state.fault_code & 0xFFFF == arm.err_code, "the error word is carried verbatim in the low sixteen bits")
    for bit in (p.DRIVER_VOLTAGE_LOW, p.DRIVER_MOTOR_OVERHEAT, p.DRIVER_OVERCURRENT, p.DRIVER_OVERHEAT, p.DRIVER_COLLISION_PROTECTION, p.DRIVER_ERROR, p.DRIVER_STALL_PROTECTION):
        bridge, arm, master, loop = opened()
        arm.driver_flags[1] = bit
        code, state = read(bridge)
        check(state.flags & rhal.FLAG_FAULT and (state.fault_code >> 24) == bit, f"a driver's `{p.DRIVER_BIT_NAMES[bit]}` is in the top byte of the fault word and reads FAULT")
    for bit, name in p.GRIPPER_BIT_NAMES.items():
        bridge, arm, master, loop = opened()
        arm.gripper_flags = bit
        code, state = read(bridge)
        check(state.flags & rhal.FLAG_FAULT and state.fault_code & p.FAULT_GRIPPER and (state.fault_code >> 24) == 0,
              f"a gripper's `{name}` reads FAULT with bit 14 of the word, and nothing in the six joint drivers' top byte")
        master.clear()
        check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM and not master.sent, "…and refuses every command, joints included, with nothing sent")
    bridge, arm, master, loop = opened()
    arm.enabled[6] = False
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_FAULT and (state.fault_code >> 24) == p.DRIVER_ENABLED, "a gripper that has disabled itself reads FAULT, with the same 0x40 a disabled joint driver sets")
    bridge, arm, master, loop = opened(gripper=False)
    arm.gripper_flags = p.GRIPPER_FAULT_BITS
    check(read(bridge)[1].flags == rhal.HOMING_UNREPORTED, "an arm declared without a hand reads no gripper byte at all: nothing it broadcasts there is its health")
    bridge, arm, master, loop = opened()
    arm.enabled[2] = False
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_FAULT and (state.fault_code >> 24) == p.DRIVER_ENABLED, "a driver that has disabled itself reads FAULT, with 0x40 in the top byte: a disabled joint is a limp one")
    bridge, arm, master, loop = opened()
    arm.ctrl_mode = 0x05
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_FAULT and state.fault_code & p.FAULT_NOT_CAN_CONTROL,
          "an arm that is no longer in CAN control reads FAULT with a word — the one bit this driver defines — and not a fault flag over a healthy-looking zero")
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM, "…and refuses a command, because the arm is listening to something else")

    # ── write_command ────────────────────────────────────────────────────────
    section("write_command — a setpoint the arm takes, or nothing at all")
    bridge, arm, master, loop = opened()
    master.clear()
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_OK, "a position command on all seven joints is accepted")
    check(master.ids() == [p.ID_MOTION_2, 0x155, 0x156, 0x157, p.ID_GRIPPER_CTRL], "it is the mode frame, the three joint pairs and the gripper — in that order")
    pairs = [struct.unpack(">ii", data) for data in (master.of(0x155)[0], master.of(0x156)[0], master.of(0x157)[0])]
    counts = [c for pair in pairs for c in pair]
    check(all(abs(c - round(math.degrees(v) * 1000)) <= 1 for c, v in zip(counts, GOOD[:6])), "each joint's angle is in its own slot of its own frame, in 0.001°")
    check(p.gripper_ctrl_fields(master.of(p.ID_GRIPPER_CTRL)[0]) == (30_000, 1000, 1, 0), "the gripper frame carries 30 mm, the operator's 1 N·m, enable, and no zero command")
    check(p.motion_ctrl_2_fields(master.of(p.ID_MOTION_2)[0])[:3] == (p.CTRL_CAN, p.MOVE_J, 30), "…and every command restates the mode and the operator's speed")
    state = settle(bridge)
    check(all(near(a, b, 1e-3) for a, b in zip(state.position_rad, GOOD)), "the arm gets there, and a read says so")

    for count in (6, 8, 0):
        master.clear()
        check(bridge.write_command(command([0.1] * count), 0) == rhal.SKR_EINVAL and not master.sent, f"a command for {count} joints is refused with nothing sent")
    master.clear()
    check(bridge.write_command(command(GOOD, modes=[0x13] + [0] * 6), 0) == rhal.SKR_EINVAL and not master.sent, "a per-joint mode the vocabulary does not define is refused with nothing sent")
    check(bridge.write_command(command(GOOD, modes=[rhal.MODE_TORQUE] + [0] * 6), 0) == rhal.SKR_EINVAL and not master.sent,
          "a bare MODE_* written where an offset per-joint mode belongs is refused: the transcription error §86.1's offset exists to catch")
    check(bridge.write_command(command(GOOD, modes=[rhal.JMODE_INHERIT] + [rhal.JMODE_POSITION] * 6), 0) == rhal.SKR_OK, "…while per-joint modes naming position, or inheriting it, are accepted")
    master.clear()
    for mask in ([1] * 7, [0] * 7, [1, 0, 1, 0, 1, 0, 1]):
        check(bridge.write_command(command(GOOD, mode=200, mask=mask), 0) == rhal.SKR_EINVAL and not master.sent,
              f"a mode the ABI does not define is malformed whatever the mask says ({sum(mask)} joints applied): refused with nothing sent")
    for mode in (rhal.MODE_VELOCITY, rhal.MODE_TORQUE):
        check(bridge.write_command(command(GOOD, mode=mode), 0) == rhal.SKR_EPERM and not master.sent,
              f"a {'velocity' if mode == rhal.MODE_VELOCITY else 'torque'} command is refused as EPERM with nothing sent: this wire has no such verb in CAN control mode")
    check(bridge.write_command(command(GOOD, mask=[1, 1, 1, 1, 1, 1, 0], mode=rhal.MODE_VELOCITY), 0) == rhal.SKR_EPERM, "…and a velocity command that masks a joint is refused too")
    check(bridge.write_command(command(GOOD, mode=rhal.MODE_VELOCITY, mask=[0] * 7), 0) == rhal.SKR_OK, "…while a command that applies to no joint at all is a hold, whatever mode it is written in")

    # a setpoint that is not a number, on every joint, in every spelling
    for joint in range(7):
        for bad in (math.nan, math.inf, -math.inf):
            master.clear()
            values = list(GOOD)
            values[joint] = bad
            check(bridge.write_command(command(values), 0) == rhal.SKR_EINVAL and not master.sent,
                  f"a {bad!r} setpoint on {(list(p.JOINT_NAMES) + ['gripper'])[joint]} is refused with nothing sent (F282's shape)")
    masked_nan = list(GOOD)
    masked_nan[2] = math.nan
    check(bridge.write_command(command(masked_nan, mask=[1, 1, 0, 1, 1, 1, 1]), 0) == rhal.SKR_OK, "a NaN in the entry of a joint the mask holds is not read, so the command is accepted")
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_OK, "…and the next ordinary command is too: the refusals above stored nothing")
    not_a_number = list(GOOD)
    not_a_number[3] = math.nan
    code, text = said(lambda: bridge.write_command(command(not_a_number), 0))
    check(code == rhal.SKR_EINVAL and f"the setpoint for {p.JOINT_NAMES[3]} is not a finite number" in text,
          "a refused NaN names the joint and says what it was, and the command is refused whole")

    # travel
    names = list(p.JOINT_NAMES) + ["gripper"]
    declared = json.loads(bridge.describe())["joints"]
    bounds = []
    for joint in range(7):
        limits = declared[joint]["limits"]
        bounds.append((limits.get("position_min_rad", limits.get("position_min_m")), limits.get("position_max_rad", limits.get("position_max_m"))))
    check(all(abs(bounds[j][0] - p.JOINT_LIMITS_RAD[j][0]) < 2e-3 and abs(bounds[j][1] - p.JOINT_LIMITS_RAD[j][1]) < 2e-3 for j in range(6)),
          "the declared travel is the maker's table to the 0.1° the arm reports it in")
    for joint in range(7):
        low, high = bounds[joint]
        margin = 1e-3 if joint < 6 else 1e-4
        for label, value, accepted in (("below", low - margin, False), ("above", high + margin, False), ("at the lower bound", low, True), ("at the upper bound", high, True)):
            values = [(bounds[i][0] + bounds[i][1]) / 2 for i in range(7)]
            values[joint] = value
            master.clear()
            result = bridge.write_command(command(values), 0)
            check((result == rhal.SKR_OK) if accepted else (result == rhal.SKR_EINVAL and not master.sent),
                  f"{names[joint]} {label} its travel is {'accepted' if accepted else 'refused whole, with nothing sent'}")

    # the mask is a hold
    bridge, arm, master, loop = opened()
    settle(bridge, 3)
    outside = list(GOOD)
    outside[1] = 9.0  # joint2's travel is 0 to 3.14 rad
    master.clear()
    check(bridge.write_command(command(outside, mask=[1, 0, 1, 1, 1, 1, 1]), 0) == rhal.SKR_OK,
          "a number outside the travel in the entry of a joint the mask holds is never sent, so it is not held to the travel and the command is accepted")
    held_pair = struct.unpack(">ii", master.of(0x155)[0])
    check(abs(held_pair[1]) <= 1, "…and that joint was held where it is, not sent the number")
    check(bridge.write_command(command(outside), 0) == rhal.SKR_EINVAL, "…while the same number on a joint the mask applies is refused")

    bridge, arm, master, loop = opened()
    settle(bridge)
    assert bridge.write_command(command(GOOD), 0) == rhal.SKR_OK
    settle(bridge)
    master.clear()
    held = [0.0] * 7
    mask = [1, 0, 1, 0, 1, 0, 0]
    held[0], held[2], held[4] = 0.3, -0.6, 0.2
    assert bridge.write_command(command(held, mask=mask), 0) == rhal.SKR_OK
    counts = [c for data in (master.of(0x155)[0], master.of(0x156)[0], master.of(0x157)[0]) for c in struct.unpack(">ii", data)]
    check(abs(counts[0] - round(math.degrees(0.3) * 1000)) <= 1 and abs(counts[2] - round(math.degrees(-0.6) * 1000)) <= 1, "an unmasked joint carries the new setpoint")
    check(abs(counts[1] - round(math.degrees(GOOD[1]) * 1000)) <= 1 and abs(counts[3] - round(math.degrees(GOOD[3]) * 1000)) <= 1 and abs(counts[5] - round(math.degrees(GOOD[5]) * 1000)) <= 1,
          "a masked joint holds the setpoint it was last given (§24: 'current setpoint'), not zero and not a stranger's")
    check(p.gripper_ctrl_fields(master.of(p.ID_GRIPPER_CTRL)[0])[0] == 30_000, "…the gripper too")
    master.clear()
    assert bridge.write_command(command([0.0] * 7, mask=[0] * 7), 0) == rhal.SKR_OK
    counts = [c for data in (master.of(0x155)[0], master.of(0x156)[0], master.of(0x157)[0]) for c in struct.unpack(">ii", data)]
    check(abs(counts[0] - round(math.degrees(0.3) * 1000)) <= 1 and abs(counts[1] - round(math.degrees(GOOD[1]) * 1000)) <= 1, "the kernel's hold frame — every joint masked — holds every joint where it was told to be")
    bridge, arm, master, loop = opened()
    arm.mdeg = [10_000, 20_000, -30_000, 0, 0, 0]
    arm.target = list(arm.mdeg)
    settle(bridge, 3)
    master.clear()
    assert bridge.write_command(command([0.0] * 7, mask=[0] * 7), 0) == rhal.SKR_OK
    counts = [c for data in (master.of(0x155)[0], master.of(0x156)[0], master.of(0x157)[0]) for c in struct.unpack(">ii", data)]
    check(counts[:3] == [10_000, 20_000, -30_000], "a hold on a joint never commanded latches where it is measured, not at the zero the arrays are sized with")
    bridge, arm, master, loop = opened()
    settle(bridge, 3)
    assert bridge.write_command(command(GOOD), 0) == rhal.SKR_OK
    assert bridge.close() == rhal.SKR_OK
    arm.poll()  # the command is still in the arm's inbox until the arm's loop runs
    arm.mdeg = [5_000, 5_000, -5_000, 5_000, 5_000, 5_000]  # inside every joint's travel (joint 3 runs from -170 to 0 degrees)
    arm.target = list(arm.mdeg)
    assert bridge.open(config()) == rhal.SKR_OK
    settle(bridge, 3)
    master.clear()
    assert bridge.write_command(command([0.0] * 7, mask=[0] * 7), 0) == rhal.SKR_OK
    counts = [c for data in (master.of(0x155)[0], master.of(0x156)[0], master.of(0x157)[0]) for c in struct.unpack(">ii", data)]
    check(counts[:6] == [5_000, 5_000, -5_000, 5_000, 5_000, 5_000], "a new session has no setpoint of the last one's: its first hold is where the arm is")

    # faults refuse commands
    for label, stage in (
        ("a standing arm status", lambda a: setattr(a, "arm_status", 0x07)),
        ("a standing error word", lambda a: setattr(a, "err_code", 1)),
        ("a driver's fault bit", lambda a: a.driver_flags.__setitem__(4, p.DRIVER_OVERCURRENT)),
        ("a disabled driver", lambda a: a.enabled.__setitem__(0, False)),
        ("another control mode", lambda a: setattr(a, "ctrl_mode", 0x02)),
    ):
        bridge, arm, master, loop = opened()
        stage(arm)
        read(bridge)
        master.clear()
        check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM and not master.sent, f"{label} refuses every command whole, with nothing sent")
        check(all(frame.arbitration_id != p.ID_ENABLE and frame.arbitration_id != p.ID_MOTION_1 for frame in master.sent), "…and clears nothing: the arm's clear is the person's")

    # ── the stop ─────────────────────────────────────────────────────────────
    section("emergency_stop — the arm's fast stop, and no release that drops the arm")
    bridge, arm, master, loop = opened()
    settle(bridge, 3)
    master.clear()
    check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    check(master.sent and [frame.arbitration_id for frame in master.sent] == [p.ID_MOTION_1] and master.of(p.ID_MOTION_1)[0][0] == p.EMERGENCY_STOP,
          "it sends one 0x150 with the fast stop in its first byte — and nothing else")
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_ESTOP, "the next state says the stop is asserted")
    check(arm.arm_status == p.ARM_STATUS_EMERGENCY_STOP, "…and the arm agrees")
    master.clear()
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM and not master.sent, "commands are refused while the latch is set, with nothing sent")
    check(bridge.release_stop() == rhal.SKR_ENOSYS, "release_stop is ENOSYS: the maker's resume byte is a reset that drops the arm, so there is no software release")
    check(arm.resets == 0 and not arm.dropped, "…and the arm was never sent it")
    check(bridge.close() == rhal.SKR_OK, "close()")
    bridge2 = PiperBridge(master)
    check(bridge2.open(config()) == rhal.SKR_ENODEV, "a new session over an arm that still reports the stop is refused")
    arm.arm_status = p.ARM_STATUS_NORMAL
    arm.enabled = [True] * 7
    check(bridge2.open(config()) == rhal.SKR_OK, "…and one over an arm a person has cleared opens, with the latch gone")
    check(bridge2.write_command(command(GOOD), 0) == rhal.SKR_OK, "…and takes commands")
    bridge3, arm3, master3, loop3 = build()
    check(bridge3.emergency_stop() == rhal.SKR_OK, "a stop before any session is not an error")

    # The driver's own latch, apart from the arm's: an arm that never acts on the frame cannot be what refuses the commands after it.
    bridge, arm, master, loop = opened()
    settle(bridge, 3)
    arm.deaf_to = {p.ID_MOTION_1}
    check(bridge.emergency_stop() == rhal.SKR_OK, "a stop to an arm that does not act on it returns")
    code, state = read(bridge)
    check(arm.arm_status == p.ARM_STATUS_NORMAL and state.flags & rhal.FLAG_ESTOP,
          "the next state reports the stop at once, though the arm never confirms it: the latch is this driver's own, and asserting a stop does not wait to be agreed with")
    master.clear()
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM and not master.sent, "…and commands are refused whole, with nothing sent, while the arm says it is running")

    # A session is over when it is closed: the same bridge, opened again over an arm that has changed.
    bridge, arm, master, loop = opened()
    settle(bridge, 3)
    assert bridge.emergency_stop() == rhal.SKR_OK and bridge.close() == rhal.SKR_OK
    arm.poll()  # the stop is still in the arm's inbox until the arm's loop runs
    assert arm.arm_status == p.ARM_STATUS_EMERGENCY_STOP
    arm.arm_status = p.ARM_STATUS_NORMAL
    arm.enabled = [True] * 7  # a person has reset the arm, with its weight supported
    check(bridge.open(config()) == rhal.SKR_OK, "the same bridge, opened again over an arm a person has cleared, comes up")
    code, state = read(bridge)
    check(not state.flags & rhal.FLAG_ESTOP and bridge.write_command(command(GOOD), 0) == rhal.SKR_OK,
          "…with the stop gone and commands taken: the latch belonged to the session that asserted it, and release is ENOSYS (F76's shape)")
    bridge, arm, master, loop = opened()
    check(near(json.loads(bridge.describe())["joints"][0]["limits"]["position_max_rad"], 2.6179, 1e-3), "a session declares the travel the arm holds: the maker's table where the arm reports no narrower")
    assert bridge.close() == rhal.SKR_OK
    arm.limits[0] = (1000, -1000)  # the arm now holds ±100° on joint 1
    master.clear()
    check(bridge.open(config()) == rhal.SKR_OK and len(master.of(p.ID_LIMIT_QUERY)) == 6,
          "a new session asks every joint for its limits again: what an earlier session was told is not what the arm holds now")
    check(near(json.loads(bridge.describe())["joints"][0]["limits"]["position_max_rad"], math.radians(100.0), 1e-3), "…and declares the narrower travel the arm holds now")
    bridge, arm, master, loop = build()
    stale_stop = struct.pack(">BBBBBBH", p.CTRL_CAN, p.ARM_STATUS_EMERGENCY_STOP, p.MOVE_J, 0, 0, 0, 0)
    loop.endpoint().send(Frame(p.ID_STATUS, stale_stop))
    check(bridge.open(config()) == rhal.SKR_OK, "a stop an earlier session saw, still waiting on the bus when this one opens, is discarded: what is waiting answers a session that is over")

    # ── sessions ─────────────────────────────────────────────────────────────
    section("sessions — a closed body is closed")
    bridge, arm, master, loop = opened()
    master.clear()
    check(bridge.close() == rhal.SKR_OK and not master.sent, "close() sends nothing: a disabled Piper falls and a stood-down one is no longer held, so silence is the hand-back that cannot drop a load")
    check(read(bridge)[0] == rhal.SKR_EPERM, "a read after close is refused")
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM, "…and so is a command")
    check(bridge.read_sensor("x", None) == rhal.SKR_ENOSYS, "this arm has no sensors to read")
    bridge, arm, master, loop = opened()
    arm.powered = False
    check(read(bridge)[0] == rhal.SKR_ETIMEDOUT, "an arm that has gone silent is ETIMEDOUT, not the last angles read as fresh")
    clock = ManualClock()
    bridge, arm, master, loop = opened(clock=clock)
    read(bridge)
    arm.mute = {p.ID_JOINTS_34}
    clock.advance(1.0)
    code, state = read(bridge)
    check(code == rhal.SKR_OK and state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT,
          "a joint pair that has not been heard for a second reads DEGRADED: the angles of joints 3 and 4 are the last thing the arm said")
    arm.mute = set()
    clock.advance(0.0)
    code, state = read(bridge)
    check(not state.flags & rhal.FLAG_DEGRADED, "…and reads healthy again when it is heard")
    for label, muted in (("the status frame", p.ID_STATUS), ("a driver's slow report", p.ID_LOW_SPEED + 2), ("the gripper's frame", p.ID_GRIPPER)):
        clock = ManualClock()
        bridge, arm, master, loop = opened(clock=clock)
        read(bridge)
        arm.mute = {muted}
        clock.advance(1.0)
        code, state = read(bridge)
        check(code == rhal.SKR_OK and state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT,
              f"{label}, not heard for a second, reads DEGRADED and not FAULT: what it last said is no longer health")
        arm.mute = set()
        code, state = read(bridge)
        check(not state.flags & rhal.FLAG_DEGRADED, f"…and reads healthy again when {label} is heard")

    # ── frames it does not speak ─────────────────────────────────────────────
    section("traffic that is not this driver's is counted, not applied")
    bridge, arm, master, loop = opened()
    before = bridge.unexpected_frames
    loop.endpoint().send(Frame(0x7FF, bytes(8)))
    loop.endpoint().send(Frame(p.ID_STATUS, bytes(3)))
    bridge.read_state(rhal.State(), 0)
    check(bridge.unexpected_frames >= before + 2, "an unknown identifier and a status frame of the wrong length are counted")
    check(read(bridge)[0] == rhal.SKR_OK, "…and the read after them is unaffected")

    # ── the entry point ──────────────────────────────────────────────────────
    section("the python-host entry point")
    from skynet_piper import __main__ as entry

    previous = os.environ.get("SKYNET_PIPER_SIM")
    os.environ["SKYNET_PIPER_SIM"] = "bogus"
    try:
        check(entry.main() == 1, "a SKYNET_PIPER_SIM that is neither 1 (with a gripper) nor 0 (without) refuses")
    finally:
        if previous is None:
            del os.environ["SKYNET_PIPER_SIM"]
        else:
            os.environ["SKYNET_PIPER_SIM"] = previous

    # ── vcan ─────────────────────────────────────────────────────────────────
    if "--vcan" in sys.argv[1:]:
        vcan_checks("vcan0")
    elif can_available():
        check(True, "this kernel has CAN support; `make check-vcan` runs the driver against it")
    else:
        print("  ----  this kernel has no CAN support, so SocketCanBus was not exercised at all. `make check-vcan` needs a kernel with it.")

    # ── the maker's reset byte, over the whole suite ─────────────────────────
    section("what no session may ever send")
    check(all(not (frame.arbitration_id == p.ID_MOTION_1 and frame.data[0] == 0x02) for frame in EVERYTHING_SENT),
          "across every session in this suite, not one 0x150 carried the maker's reset byte")

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 0 if not FAILURES else 1


def vcan_checks(interface: str) -> None:
    """The bridge on a kernel CAN bus, against a simulated arm in another process.

    `make check-vcan` is the way in: it creates `vcan0` in an unprivileged network namespace when there is none, which needs no root.
    Asked for and not possible is a failure, never a skip — `--vcan` is a request to measure this, and a run that measured nothing
    must not pass.
    """
    section(f"bridge — on {interface}, against an arm in another process")
    if not can_available():
        check(False, "--vcan was asked for and this kernel will not give out a CAN socket")
        return
    here = os.path.dirname(os.path.abspath(__file__))
    environment = dict(os.environ)
    environment["PYTHONPATH"] = os.pathsep.join([here, os.path.join(here, "..", "canopen"), os.path.join(here, "..", "..", "bindings", "python")])

    def arm(*extra: str) -> subprocess.Popen:
        process = subprocess.Popen([sys.executable, "-m", "skynet_piper.serve", interface, *extra], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   text=True, env=environment)
        assert process.stdout is not None
        if process.stdout.readline().strip() != "ready":
            raise AssertionError("the arm did not come up")
        return process

    def stand_down(process: subprocess.Popen) -> None:
        assert process.stdin is not None
        process.stdin.close()
        process.wait(timeout=10)

    process = arm()
    try:
        bridge = PiperBridge(SocketCanBus(interface))
        check(bridge.open(config()) == rhal.SKR_OK, "an arm in another process comes up: its broadcast, its limits, its enables, CAN control")
        state = rhal.State()
        check(bridge.read_state(state, 0) == rhal.SKR_OK, "the first read answers: a fresh cycle arrives after the drain, and it waits for it")
        goal = command([0.3, 0.8, -0.8, 0.2, 0.3, -0.4, 0.04])
        failures = 0
        for _ in range(120):
            bridge.write_command(goal, 0)
            if bridge.read_state(state, 0) != rhal.SKR_OK:
                failures += 1
        check(failures == 0, "a hundred and twenty commanded cycles, and every read answered")
        check(all(near(a, b, 2e-3) for a, b in zip(state.position_rad, goal.values)), "and the arm reached its setpoints over the kernel's bus")
        check(not state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT, "with no report stale and no fault")
        check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
        bridge.read_state(state, 0)
        check(state.flags & rhal.FLAG_ESTOP != 0, "the stop is visible in the next state")
        check(bridge.write_command(goal, 0) == rhal.SKR_EPERM, "and later commands are refused")
        bridge.close()
    finally:
        stand_down(process)

    for label, extra in (("a standing arm status", ("--status", "7")), ("a standing error word", ("--error", "1")),
                         ("a driver reporting over-current", ("--driver", "4", str(p.DRIVER_OVERCURRENT)))):
        process = arm(*extra)
        try:
            check(PiperBridge(SocketCanBus(interface)).open(config()) == rhal.SKR_ENODEV, f"{label} in another process is refused at open")
        finally:
            stand_down(process)
    process = arm("--no-gripper")
    try:
        check(PiperBridge(SocketCanBus(interface)).open(config()) == rhal.SKR_ENODEV, "a configuration that declares a gripper the arm does not broadcast is refused on a real bus too")
    finally:
        stand_down(process)


if __name__ == "__main__":
    raise SystemExit(main())

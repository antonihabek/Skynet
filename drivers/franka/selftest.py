"""skynet-franka's own suite: the wire, the state machine, and every refusal.

No test framework and no robot. What makes this suite worth more than the sum of its assertions is
its first section: **the layout control.** Every other driver in this repository was measured
against vendor software — URSim, `mock_components`, a CiA 402 slave — and this one cannot be, because
the FCI talks to a Control box and there is no simulator. `DRY-COVERAGE-PLAN.md` F15 is explicit
about what that costs: a protocol transcribed from memory and tested against a fake written from the
same memory proves that two copies of one misunderstanding agree.

So the layout is not transcribed. `layout.json` holds what Franka's own headers compile to —
`sizeof` and `offsetof` computed by a compiler on the vendor's Apache-2.0 definitions — and the
first thing this file does is hold `skynet_franka.protocol` to it, field by field. That does not
make the *behaviour* measured. It does mean a struct this driver reads at the wrong offset fails
here rather than arriving as a joint angle that is somebody else's field.

    make selftest       # this file. Needs python3 and nothing else
    make check          # the whole path through the real ABI, against the fake Control box
    make check-layout   # regenerate layout.json from a libfranka-common checkout
"""

from __future__ import annotations

import json
import math
import os
import socket
import struct
import sys
import threading
import time

import skynet_rhal as rhal

from skynet_franka import protocol as fci
from skynet_franka.bridge import JOINT_LIMITS, JOINT_RATINGS, ROLES, FrankaBridge
from skynet_franka.sim import FakeControlBox

FAILURES: list[str] = []
CHECKS = 0


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


# ── 1. The layout, against the vendor's own compiler ─────────────────────────


def the_layout_is_the_vendors() -> None:
    section("the wire layout, against layout.json (Franka's own headers, compiled)")
    here = os.path.dirname(os.path.abspath(__file__))
    with open(os.path.join(here, "layout.json"), encoding="utf-8") as handle:
        pinned = json.load(handle)
    structs = pinned["structs"]

    check(pinned["constants"]["version"] == fci.VERSION, "the FCI version is the vendor's")
    check(
        pinned["constants"]["command_port"] == fci.COMMAND_PORT,
        "the command port is the vendor's",
    )
    check(
        structs["CommandHeader"]["size"] == fci.HEADER_SIZE,
        f"CommandHeader is {structs['CommandHeader']['size']} bytes",
    )
    check(
        structs["ConnectRequest"]["size"] == fci.CONNECT_REQUEST.size,
        "Connect::Request is four bytes — a version and a port, packed",
    )
    check(
        structs["ConnectResponse"]["size"] == fci.CONNECT_RESPONSE.size,
        "Connect::Response is three bytes: a uint8 status with a uint16 after it and no padding",
    )
    check(
        structs["MoveRequest"]["size"] == fci.MOVE_REQUEST.size,
        f"Move::Request is {structs['MoveRequest']['size']} bytes",
    )
    check(
        structs["RobotState"]["size"] == fci.ROBOT_STATE_SIZE,
        f"RobotState is {structs['RobotState']['size']} bytes",
    )
    check(
        structs["RobotCommand"]["size"] == fci.ROBOT_COMMAND_SIZE,
        f"RobotCommand is {structs['RobotCommand']['size']} bytes",
    )

    state_fields = structs["RobotState"]["fields"]
    for name, ours in (
        ("message_id", fci.STATE_MESSAGE_ID),
        ("q", fci.STATE_Q),
        ("dq", fci.STATE_DQ),
        ("tau_J", fci.STATE_TAU_J),
        ("motion_generator_mode", fci.STATE_MOTION_GENERATOR_MODE),
        ("controller_mode", fci.STATE_CONTROLLER_MODE),
        ("errors", fci.STATE_ERRORS),
        ("reflex_reason", fci.STATE_REFLEX_REASON),
        ("robot_mode", fci.STATE_ROBOT_MODE),
        ("control_command_success_rate", fci.STATE_SUCCESS_RATE),
    ):
        check(state_fields[name] == ours, f"RobotState.{name} is at {state_fields[name]}")

    command_fields = structs["RobotCommand"]["fields"]
    check(command_fields["motion"] == fci.COMMAND_MOTION, "RobotCommand.motion is at 8")
    check(command_fields["control"] == fci.COMMAND_CONTROL, "RobotCommand.control is at 314")
    motion_fields = structs["MotionGeneratorCommand"]["fields"]
    check(motion_fields["q_c"] == fci.MOTION_Q_C, "MotionGeneratorCommand.q_c is first")
    check(motion_fields["dq_c"] == fci.MOTION_DQ_C, "MotionGeneratorCommand.dq_c is at 56")
    check(
        motion_fields["motion_generation_finished"] == fci.MOTION_FINISHED,
        "MotionGeneratorCommand.motion_generation_finished is the last byte",
    )
    control_fields = structs["ControllerCommand"]["fields"]
    check(control_fields["tau_J_d"] == fci.CONTROL_TAU_J_D, "ControllerCommand.tau_J_d is first")

    # The asymmetry a careful reading gets wrong, asserted rather than commented: 56 bytes of
    # doubles on the command side against 28 of floats on the state side, for the same seven joints.
    check(
        motion_fields["dq_c"] - motion_fields["q_c"] == 7 * 8,
        "the command's joint arrays are doubles (56 bytes for seven)",
    )
    check(
        state_fields["dq"] - state_fields["q_d"] == 7 * 4,
        "the state's joint arrays are floats (28 bytes for seven) — the same seven joints",
    )


# ── 2. The wire, encoded and decoded ─────────────────────────────────────────


def the_wire_round_trips() -> None:
    section("encoding")
    header = fci.Header(fci.COMMAND_MOVE, 7, 125)
    again = fci.Header.unpack(header.pack())
    check(
        (again.command, again.command_id, again.size) == (fci.COMMAND_MOVE, 7, 125),
        "a command header round-trips",
    )

    try:
        fci.Header.unpack(struct.pack("<III", fci.COMMAND_MOVE, 1, 3))
        check(False, "a header declaring fewer bytes than itself is refused")
    except fci.ProtocolError:
        check(True, "a header declaring fewer bytes than itself is refused")

    q = [0.1, -0.2, 0.3, -0.4, 0.5, -0.6, 0.7]
    dq = [0.01] * 7
    tau = [1.5] * 7
    raw = fci.pack_robot_state(42, q, dq, tau, robot_mode=fci.ROBOT_MODE_MOVE)
    state = fci.RobotState.unpack(raw)
    check(state.message_id == 42, "a RobotState carries its message id")
    check(
        all(abs(a - b) < 1e-6 for a, b in zip(state.q, q)),
        "the seven joint positions come back where they went in",
    )
    check(state.robot_mode == fci.ROBOT_MODE_MOVE, "the robot mode is the byte at 1372")
    check(not state.faulted(), "no error bit is set on a clean state")
    check(state.fault_code() == 0, "a clean state has fault code zero, which is a measurement")

    faulted = fci.RobotState.unpack(
        fci.pack_robot_state(1, q, dq, tau, errors=[False] * 3 + [True])
    )
    check(faulted.faulted(), "an error bit is seen")
    check(
        faulted.fault_code() == 4,
        "the fault code is the first set bit, one-based, and nothing is interpreted",
    )

    try:
        fci.RobotState.unpack(raw[:-1])
        check(False, "a short RobotState is refused rather than parsed")
    except fci.ProtocolError:
        check(True, "a short RobotState is refused rather than parsed")

    command = fci.unpack_robot_command(fci.pack_robot_command(9, q_c=q))
    check(command["message_id"] == 9, "a RobotCommand carries the state's message id")
    check(
        all(abs(a - b) < 1e-12 for a, b in zip(command["q_c"], q)),
        "the command's positions are doubles and survive exactly",
    )
    check(
        all(v == 0.0 for v in command["tau_J_d"]),
        "a position command leaves the torque field zero rather than stale",
    )


# ── 3. The driver, against the fake Control box ──────────────────────────────


class Harness:
    """A fake box and a bridge connected to it, with the config `open()` would receive."""

    def __init__(self, hz: float = 500.0) -> None:
        self.box = FakeControlBox(hz=hz)
        self.port = self.box.start()
        self.driver = FrankaBridge()

    def open(self) -> int:
        return self.driver.open(json.dumps({"host": "127.0.0.1", "port": self.port}))

    def close(self) -> None:
        self.driver.close()
        self.box.stop()

    @staticmethod
    def state() -> rhal.State:
        # `joint_count` is derived from `position_rad` in the Python view, so a fresh State is what
        # a driver is handed and the driver fills the arrays.
        return rhal.State()


def command(mode: int, values, mask=None) -> rhal.Command:
    return rhal.Command(
        stamp_ns=0,
        seq=1,
        mode=mode,
        values=list(values),
        mask=list(mask if mask is not None else [1] * len(values)),
    )


def the_driver_connects_and_reads() -> None:
    section("connect, and the body it reports")
    harness = Harness()
    try:
        check(harness.open() == rhal.SKR_OK, "open() connects and waits for the first state")

        descriptor = json.loads(harness.driver.describe())
        check(descriptor["dof"] == 7, "the descriptor declares seven joints")
        check(
            descriptor["simulator"] is True,
            "declared simulated: the box on the other end is the fake (§35.3)",
        )
        check(
            [joint["role"] for joint in descriptor["joints"]] == ROLES,
            "every joint carries a canonical §5.1 role",
        )
        # The transcribed ratings, from Franka Robotics' own `franka_description` FR3 table —
        # a second person verifies them against that document, not against this file.
        envelope = descriptor["envelope"]
        check(envelope is not None, "an envelope is declared, with the citation beside it")
        check(
            envelope["joint_velocity_max_rad_s"] == 2.62
            and envelope["joint_torque_max_nm"] == 12.0,
            "the envelope is the narrowest across the arm",
        )
        check(envelope["watchdog_ms"] == 10.0, "the watchdog is ten FCI periods")
        check(
            "franka_description" in envelope["source"],
            "and the source names the vendor's table",
        )
        check(
            envelope["basis"] == "declared",
            "and states the kind beside the citation (§5.3)",
        )
        check(
            "estop" not in envelope,
            "but no hardware stop — the button is on the Control box and this driver "
            "cannot press it",
        )
        check(
            all(
                joint["limits"]["velocity_max_rad_s"] == velocity
                and joint["limits"]["effort_max_nm"] == effort
                for joint, (velocity, effort) in zip(descriptor["joints"], JOINT_RATINGS)
            ),
            "and every joint declares its own rated velocity and effort",
        )
        check(
            "safety.estop" in descriptor["capabilities"],
            "the StopMove the driver has is the capability it declares",
        )
        check(
            all(
                joint["limits"]["position_min_rad"] == low
                and joint["limits"]["position_max_rad"] == high
                for joint, (low, high) in zip(descriptor["joints"], JOINT_LIMITS)
            ),
            "travel limits are declared, because travel is a kinematic fact about the arm",
        )
        check(descriptor["sensors"] == [], "no sensors: a Franka arm publishes no images")
        check(
            descriptor["control"]["rate_hz_min"] == 1000,
            "the FCI's rate is 1 kHz and is not a range",
        )

        out = harness.state()
        check(harness.driver.read_state(out, 0) == rhal.SKR_OK, "read_state succeeds")
        check(out.joint_count == 7, "seven positions come back")
        check(
            all(v == v for v in out.effort_nm),
            "effort is a measurement here, not §24's NaN — the arm reports link-side torque",
        )
        check(out.flags & rhal.HOMING_NOT_REQUIRED != 0, "the arm declares nothing to home")

        first = out.seq
        harness.driver.read_state(out, 0)
        check(out.seq > first, "the sequence advances between reads")

        # The flag follows the substrate, declared by the operator: a real Control box says so
        # deliberately, and the default is the safe direction.
        real = Harness()
        try:
            config = json.dumps(
                {"host": "127.0.0.1", "port": real.port, "simulator": False}
            )
            check(real.driver.open(config) == rhal.SKR_OK, "a declared-real box opens")
            check(
                json.loads(real.driver.describe())["simulator"] is False,
                "and declares real when the operator says so",
            )
        finally:
            real.close()
    finally:
        harness.close()


def the_driver_refuses_what_it_should() -> None:
    section("refusals, each beside the nearly identical thing that works")
    harness = Harness()
    try:
        out = harness.state()
        check(
            harness.driver.read_state(out, 0) == rhal.SKR_EPERM,
            "reading before open is refused",
        )
        check(
            harness.driver.write_command(command(rhal.MODE_POSITION, [0.0] * 7), 0)
            == rhal.SKR_EPERM,
            "commanding before open is refused",
        )
        check(harness.open() == rhal.SKR_OK, "…and open() then succeeds")

        harness.driver.read_state(out, 0)
        here = list(out.position_rad)
        check(
            harness.driver.write_command(command(rhal.MODE_POSITION, here), 0) == rhal.SKR_OK,
            "a well-formed position command is accepted",
        )
        check(
            harness.driver.write_command(command(rhal.MODE_POSITION, here[:3]), 0)
            == rhal.SKR_EINVAL,
            "a command with three joints on a seven-joint arm is refused",
        )
        check(
            harness.driver.write_command(command(200, here), 0) == rhal.SKR_EINVAL,
            "a mode this ABI does not define is malformed, not merely refused",
        )

        # A fault latched on the robot, which is what a collision or a reflex does.
        harness.box.latch_error(3)
        time.sleep(0.05)
        harness.driver.read_state(out, 0)
        check(out.flags & rhal.FLAG_FAULT != 0, "a latched error reaches §24's fault flag")
        check(
            out.fault_code == 4,
            "and the vendor's bit index reaches fault_code verbatim, uninterpreted",
        )
        check(
            harness.driver.write_command(command(rhal.MODE_POSITION, here), 0) == rhal.SKR_EPERM,
            "a faulted arm is refused rather than commanded into the fault (F26's rule again)",
        )
        check(
            not harness.box.errors[3] is False,
            "and the driver did not clear the error to get past it",
        )

        harness.box.clear_errors()
        time.sleep(0.05)
        harness.driver.read_state(out, 0)
        check(
            harness.driver.write_command(command(rhal.MODE_POSITION, here), 0) == rhal.SKR_OK,
            "…and after the operator's recovery it is accepted again",
        )
    finally:
        harness.close()


def the_stop_is_a_stop() -> None:
    section("emergency_stop (§24's second rule)")
    harness = Harness()
    try:
        harness.open()
        out = harness.state()
        harness.driver.read_state(out, 0)
        here = list(out.position_rad)
        harness.driver.write_command(command(rhal.MODE_POSITION, here), 0)
        time.sleep(0.05)
        check(
            harness.box.motion_mode != fci.MOTION_IDLE,
            "a motion is running before the stop",
        )

        check(harness.driver.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
        time.sleep(0.1)
        check(
            harness.box.motion_mode == fci.MOTION_IDLE,
            "the Control box stopped the motion — StopMove, not a torque of zero",
        )
        check(
            harness.driver.write_command(command(rhal.MODE_POSITION, here), 0) == rhal.SKR_EPERM,
            "commands are refused while the latch is set",
        )
        harness.driver.read_state(out, 0)
        check(out.flags & rhal.FLAG_ESTOP != 0, "and the stop is visible in the next state")

        # F76: the latch outlived its session, so a driver closed and opened again after a stop
        # refused every command for the life of the process.
        harness.driver.close()
        check(harness.open() == rhal.SKR_OK, "the same driver opens again after the stop")
        check(
            harness.driver.write_command(command(rhal.MODE_POSITION, here), 0) == rhal.SKR_OK,
            "and commands again: the latch belonged to the session it was asserted in (F76)",
        )
    finally:
        harness.close()


def the_modes_switch_motions() -> None:
    section("the motion state machine: a mode change is a Move, not a field")
    harness = Harness()
    try:
        harness.open()
        out = harness.state()
        harness.driver.read_state(out, 0)
        here = list(out.position_rad)

        harness.driver.write_command(command(rhal.MODE_POSITION, here), 0)
        time.sleep(0.05)
        check(
            harness.box.motion_mode == fci.MOTION_JOINT_POSITION,
            "a position command starts a joint-position motion",
        )
        harness.driver.write_command(command(rhal.MODE_VELOCITY, [0.0] * 7), 0)
        time.sleep(0.05)
        check(
            harness.box.motion_mode == fci.MOTION_JOINT_VELOCITY,
            "a velocity command stops that motion and starts a velocity one",
        )
        harness.driver.write_command(command(rhal.MODE_TORQUE, [0.0] * 7), 0)
        time.sleep(0.05)
        check(
            harness.box.controller_mode == fci.CONTROLLER_EXTERNAL,
            "a torque command asks for the external controller, which is what §23.3's loop needs",
        )
    finally:
        harness.close()


def the_mask_holds_rather_than_zeroing() -> None:
    section("§24's mask, per mode")
    harness = Harness()
    try:
        harness.open()
        out = harness.state()
        harness.driver.read_state(out, 0)
        here = list(out.position_rad)
        target = [v + 0.05 for v in here]
        mask = [1, 0, 1, 0, 1, 0, 1]
        harness.driver.write_command(command(rhal.MODE_POSITION, target, mask), 0)
        time.sleep(0.1)
        sent = harness.box.last_command
        check(sent is not None, "the command reached the box")
        if sent is not None:
            check(
                abs(sent["q_c"][0] - target[0]) < 1e-3,
                "an unmasked joint carries the setpoint",
            )
            check(
                abs(sent["q_c"][1] - here[1]) < 1e-3,
                "a masked joint with no setpoint carries its own reading — held, not zeroed",
            )

        # The arm moves under the hold (the box is told so directly); the target must not follow.
        # Re-reading `q` every cycle zeroes the position error, which is F121: on a loaded joint
        # without the Control box's gravity compensation, a hold that follows the joint holds
        # nothing.
        harness.box.q[1] = here[1] + 0.2
        time.sleep(0.1)
        harness.driver.read_state(out, 0)
        harness.driver.write_command(command(rhal.MODE_POSITION, target, mask), 0)
        time.sleep(0.1)
        sent = harness.box.last_command
        if sent is not None:
            check(
                abs(sent["q_c"][1] - here[1]) < 1e-3,
                "the hold target is latched, not re-read: the joint was pushed 0.2 rad and the "
                "hold still asks for where it began",
            )

        # The kernel's hold frame after a position command: every joint held at the setpoint the
        # command left — §24's "current setpoint" — not at the zeros the frame carries.
        harness.driver.write_command(command(rhal.MODE_POSITION, target), 0)
        time.sleep(0.05)
        harness.driver.write_command(command(rhal.MODE_POSITION, [0.0] * 7, [0] * 7), 0)
        time.sleep(0.1)
        sent = harness.box.last_command
        if sent is not None:
            check(
                max(abs(a - b) for a, b in zip(sent["q_c"], target)) < 1e-3,
                "the hold frame holds every joint at the last position setpoint",
            )

        harness.driver.write_command(command(rhal.MODE_TORQUE, [5.0] * 7, mask), 0)
        time.sleep(0.1)
        sent = harness.box.last_command
        if sent is not None:
            check(abs(sent["tau_J_d"][0] - 5.0) < 1e-6, "an unmasked torque is applied")
            check(
                sent["tau_J_d"][1] == 0.0,
                "a masked torque is zero — which under the FCI's gravity compensation floats "
                "the joint rather than dropping it",
            )

        # Back to position after the torque command: the old setpoints are void — the arm may
        # have moved under a torque command — so a hold latches where each joint now is.
        harness.box.q[3] = -2.0
        time.sleep(0.1)
        harness.driver.read_state(out, 0)
        now = list(out.position_rad)
        harness.driver.write_command(command(rhal.MODE_POSITION, [0.0] * 7, [0] * 7), 0)
        time.sleep(0.1)
        sent = harness.box.last_command
        if sent is not None and "q_c" in sent:
            check(
                abs(sent["q_c"][3] - now[3]) < 1e-3,
                "after a torque command a position hold latches where the joint is, not at a "
                "setpoint from before the torque command",
            )
    finally:
        harness.close()


def the_version_gate_refuses() -> None:
    section("the handshake")
    box = FakeControlBox(hz=200.0)
    port = box.start()
    try:
        # A client claiming a version the box does not speak. Everything after `Connect` is a
        # fixed-width struct whose meaning is the version's, so this is the one refusal that has to
        # work before anything else can be trusted.
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(2.0)
        sock.connect(("127.0.0.1", port))
        sock.sendall(fci.pack_command(fci.COMMAND_CONNECT, 1, fci.CONNECT_REQUEST.pack(3, 5000)))
        reply = sock.recv(64)
        header = fci.Header.unpack(reply)
        status = reply[fci.HEADER_SIZE]
        check(
            header.command == fci.COMMAND_CONNECT and header.command_id == 1,
            "the reply names the exchange it answers",
        )
        check(
            status == fci.CONNECT_INCOMPATIBLE_LIBRARY_VERSION,
            "a wrong FCI version is refused with the box's own version in the reply",
        )
        sock.close()
    finally:
        box.stop()


def a_stale_datagram_is_never_read() -> None:
    section("latest-wins on the state stream (F27, from a second vendor)")
    harness = Harness(hz=1000.0)
    try:
        harness.open()
        out = harness.state()
        # Let the socket fill: at 1 kHz a fifth of a second is two hundred datagrams, and a client
        # that read one per cycle would be two hundred messages behind by the next read.
        time.sleep(0.2)
        harness.driver.read_state(out, 0)
        first = harness.driver._state.message_id
        time.sleep(0.2)
        harness.driver.read_state(out, 0)
        second = harness.driver._state.message_id
        check(
            second - first > 50,
            f"a read after 200 ms of stream advances by the stream and not by one "
            f"({first} then {second})",
        )
    finally:
        harness.close()


def a_silent_stream_is_not_a_state_that_is_still_arriving() -> None:
    """DRY-COVERAGE-PLAN.md F279, found by the link-chaos contract on the kernel's link emulation (`chaos.py`): with the link blackholed, `read_state` went on returning the last state it had received as a
    successful read, so a Control box that had stopped publishing was an arm that was still reporting. The Rust port answered `SKR_EIO` ("the stream is silent") and the Python driver, which the port was written
    from, returned its cached state: a stale state read as fresh, which §24's rule about a stalled producer exists to rule out."""
    section("a stream that has stopped is not a state that is still arriving (F279)")
    harness = Harness(hz=1000.0)
    try:
        check(harness.open() == rhal.SKR_OK, "a driver opens against a stream that is flowing")
        out = harness.state()
        check(harness.driver.read_state(out, 0) == rhal.SKR_OK, "and reads it")
        harness.box.silent = True
        # What was in flight arrives, and then nothing does.
        time.sleep(0.1)
        harness.driver.read_state(out, 0)
        began = time.monotonic()
        rc = harness.driver.read_state(out, 0)
        took = time.monotonic() - began
        check(rc == rhal.SKR_EIO, f"a read of a stream that has gone quiet is EIO, and not the last state it received as if it were new (it returned {rc})")
        check(took < 0.2, f"and it waits for the stream for no more than a fifth of a second ({took * 1000:.0f} ms)")
        harness.box.silent = False
        recovered = False
        end = time.monotonic() + 1.0
        while time.monotonic() < end and not recovered:
            recovered = harness.driver.read_state(out, 0) == rhal.SKR_OK
        check(recovered, "and when the stream flows again the next read is OK")
    finally:
        harness.close()


# ── DRY-COVERAGE-PLAN.md 12.C.3: what a seeded-defect run found this suite not checking ──────────
#
# Seventeen of thirty-four realistic defects survived the suite above. Three shapes account for them:
# a state read with every field zero or equal (so a sign, a swap and a wrong channel all agree with the
# truth), a stop path exercised in one order only (`release_stop` was never called), and tables the suite
# compared with themselves (`JOINT_LIMITS` against `JOINT_LIMITS`). Each is closed below on the property,
# not on the mutant.


def near(a, b, tolerance: float = 1e-5) -> bool:
    return len(a) == len(b) and all(abs(x - y) < tolerance for x, y in zip(a, b))


def the_state_is_read_from_the_right_fields() -> None:
    section("each channel of the state is its own field, with its own sign")
    harness = Harness()
    try:
        # Seven distinct, nonzero values, in every channel that has one: a joint whose sign is flipped,
        # two that are swapped and a channel wired to its neighbour all agree with a column of zeros.
        harness.box.q = [0.3, -0.5, 0.7, -1.9, 0.2, 1.4, -0.6]
        harness.box.dq = [0.01 * (i + 1) for i in range(7)]
        check(harness.open() == rhal.SKR_OK, "open() with an arm that is not at its home pose")
        time.sleep(0.05)
        out = harness.state()
        harness.driver.read_state(out, 0)
        check(near(out.position_rad, harness.box.q), "position is each joint's own slot, in order and with its sign")
        check(near(out.velocity_rad_s, harness.box.dq), "velocity is each joint's own slot: not its neighbour's")
        gravity = [round(2.0 * math.sin(q), 6) for q in harness.box.q]
        check(near(out.effort_nm, gravity), "effort is the torque channel and not the velocity one")

        # The two robot modes that are a stop and a fault without an error bit set.
        harness.box.robot_mode = fci.ROBOT_MODE_USER_STOPPED
        time.sleep(0.05)
        harness.driver.read_state(out, 0)
        check(out.flags & rhal.FLAG_ESTOP != 0, "a person's stop at the Control box (robot mode user-stopped) is a stop in the state")
        check(out.flags & rhal.FLAG_FAULT == 0, "and not a fault")
        harness.box.robot_mode = fci.ROBOT_MODE_REFLEX
        time.sleep(0.05)
        harness.driver.read_state(out, 0)
        check(out.flags & rhal.FLAG_FAULT != 0, "a reflex with no error bit yet is a fault in the state")
        check(out.flags & rhal.FLAG_ESTOP == 0, "and not a stop")
        harness.box.robot_mode = fci.ROBOT_MODE_IDLE
        time.sleep(0.05)
        harness.driver.read_state(out, 0)
        check(out.flags & (rhal.FLAG_ESTOP | rhal.FLAG_FAULT) == 0, "and an idle arm is neither")
    finally:
        harness.close()


def the_stop_path_is_walked_in_every_order() -> None:
    section("the stop: released, sent on a dead socket, and left behind by close()")
    harness = Harness()
    try:
        harness.open()
        out = harness.state()
        harness.driver.read_state(out, 0)
        here = list(out.position_rad)
        harness.driver.write_command(command(rhal.MODE_POSITION, here), 0)
        time.sleep(0.05)
        harness.driver.emergency_stop()
        time.sleep(0.1)
        check(harness.driver.release_stop() == rhal.SKR_OK, "release_stop returns")
        harness.driver.read_state(out, 0)
        check(out.flags & rhal.FLAG_ESTOP == 0, "and the stop is no longer in the state")
        check(harness.driver.write_command(command(rhal.MODE_POSITION, here), 0) == rhal.SKR_OK, "a command is accepted again")
        time.sleep(0.1)
        check(harness.box.motion_mode == fci.MOTION_JOINT_POSITION, "and starts a new motion: the stop ended the one that was running, and the driver knows it")
    finally:
        harness.close()

    # A stop that cannot be sent is not a stop that was: the latch is set, and the answer says so.
    harness = Harness()
    try:
        harness.open()
        out = harness.state()
        harness.driver.read_state(out, 0)
        harness.driver._tcp.close()  # the command connection is gone; the stop has nowhere to go
        check(harness.driver.emergency_stop() == rhal.SKR_EIO, "a stop that could not be sent says so and does not claim the box stopped")
        check(harness.driver.write_command(command(rhal.MODE_POSITION, list(out.position_rad)), 0) == rhal.SKR_EPERM, "while the latch is set all the same")
    finally:
        harness.close()

    # A session that ends with a motion running ends the motion.
    harness = Harness()
    try:
        harness.open()
        out = harness.state()
        harness.driver.read_state(out, 0)
        harness.driver.write_command(command(rhal.MODE_POSITION, list(out.position_rad)), 0)
        time.sleep(0.05)
        check(harness.box.motion_mode != fci.MOTION_IDLE, "a motion is running before close()")
        harness.driver.close()
        time.sleep(0.1)
        check(harness.box.motion_mode == fci.MOTION_IDLE, "and close() ends it on the Control box, rather than leaving it streaming into nothing")
    finally:
        harness.close()


def a_refused_move_is_not_a_started_one() -> None:
    section("a Move the box refuses, and a command the wire fails to carry")
    harness = Harness()
    try:
        harness.box.motion_mode = fci.MOTION_JOINT_POSITION  # somebody else's motion is running: the box refuses a second Move
        harness.open()
        out = harness.state()
        harness.driver.read_state(out, 0)
        check(
            harness.driver.write_command(command(rhal.MODE_POSITION, list(out.position_rad)), 0) != rhal.SKR_OK,
            "a command whose Move the box rejected is not acknowledged as taken",
        )
        time.sleep(0.05)
        check(harness.box.commands_seen == 0, "and nothing was streamed into the motion that never began")
    finally:
        harness.close()

    # F74's shape: a hold is committed when its command is on the wire, not before.
    class Failing:
        """A UDP socket whose sends fail, and which is otherwise the real one."""

        def __init__(self, real):
            self._real = real

        def sendto(self, *args):
            raise OSError("network is unreachable")

        def __getattr__(self, name):
            return getattr(self._real, name)

    harness = Harness()
    try:
        harness.open()
        out = harness.state()
        harness.driver.read_state(out, 0)
        first = list(out.position_rad)
        real = harness.driver._udp
        harness.driver._udp = Failing(real)
        failed = harness.driver.write_command(command(rhal.MODE_POSITION, [0.0] * 7, [0] * 7), 0)
        harness.driver._udp = real
        check(failed == rhal.SKR_EIO, "a command that could not be sent is a transport failure")
        harness.box.q[3] = first[3] + 0.3  # the arm moved while the send was failing
        time.sleep(0.1)
        harness.driver.read_state(out, 0)
        harness.driver.write_command(command(rhal.MODE_POSITION, [0.0] * 7, [0] * 7), 0)
        time.sleep(0.1)
        sent = harness.box.last_command
        check(
            sent is not None and abs(sent["q_c"][3] - (first[3] + 0.3)) < 1e-3,
            "so the hold latches where the joint is when a command is first carried, not where it was when one failed",
        )
    finally:
        harness.close()

    # The command echoes the state it answers: the box counts one that does not as not received.
    harness = Harness(hz=500.0)
    try:
        harness.open()
        time.sleep(0.3)
        out = harness.state()
        harness.driver.read_state(out, 0)
        for name, mode, values in (
            ("position", rhal.MODE_POSITION, list(out.position_rad)),
            ("velocity", rhal.MODE_VELOCITY, [0.0] * 7),
            ("torque", rhal.MODE_TORQUE, [0.0] * 7),
        ):
            harness.driver.write_command(command(mode, values), 0)
            time.sleep(0.1)
            harness.driver.read_state(out, 0)
            sent = harness.box.last_command
            check(sent is not None and sent["message_id"] > 10, f"a {name} command carries the message id of the state it answers, not zero")
    finally:
        harness.close()


# The vendor's tables, typed here and not imported from the driver: a suite that compares `JOINT_LIMITS` with
# itself checks nothing. Franka Robotics' own `franka_description`, `robots/fr3/joint_limits.yaml` (read
# 2026-09-30): the arm's rated ranges, velocities and torques. The driver's travel is deliberately narrower
# than the range (its header says so), so travel is held to *sit inside* the vendor's range and not to equal it.
VENDOR_RANGE = [(-2.9007, 2.9007), (-1.8361, 1.8361), (-2.9007, 2.9007), (-3.0770, -0.1169), (-2.8763, 2.8763), (0.4398, 4.6216), (-3.0508, 3.0508)]
VENDOR_VELOCITY = [2.62, 2.62, 2.62, 2.62, 5.26, 4.18, 5.26]
VENDOR_EFFORT = [87.0, 87.0, 87.0, 87.0, 12.0, 12.0, 12.0]
FRANKA_ROLES = [
    "right_arm.shoulder.yaw",
    "right_arm.shoulder.pitch",
    "right_arm.upper_arm.roll",
    "right_arm.elbow.flex",
    "right_arm.forearm.roll",
    "right_arm.wrist.pitch",
    "right_arm.wrist.roll",
]


def the_declared_body_is_the_vendors() -> None:
    section("the descriptor's tables, against the vendor's own (typed here, not imported)")
    descriptor = json.loads(FrankaBridge().describe())
    joints = descriptor["joints"]
    for index, (joint, (low, high)) in enumerate(zip(joints, VENDOR_RANGE), start=1):
        travel = (joint["limits"]["position_min_rad"], joint["limits"]["position_max_rad"])
        check(low <= travel[0] < travel[1] <= high, f"joint {index}'s travel {travel} sits inside the vendor's range ({low}, {high})")
        check(high - travel[1] < 0.5 and travel[0] - low < 0.5, f"and is not narrowed to nothing: within half a radian of it at both ends")
    check([j["limits"]["velocity_max_rad_s"] for j in joints] == VENDOR_VELOCITY, "each joint's velocity rating is the vendor's, joint for joint")
    check([j["limits"]["effort_max_nm"] for j in joints] == VENDOR_EFFORT, "and each joint's torque rating")
    check([j["role"] for j in joints] == FRANKA_ROLES, "and each joint's role is the axis it turns about: joint 3 rolls the upper arm and joint 4 is the elbow")
    check(
        descriptor["control"]["rate_hz_max"] == 1000 and descriptor["control"]["rate_hz_min"] == 1000,
        "the declared control rate is the FCI's 1 kHz at both ends: it is not a range",
    )


def main() -> int:
    the_layout_is_the_vendors()
    the_wire_round_trips()
    the_driver_connects_and_reads()
    the_driver_refuses_what_it_should()
    the_stop_is_a_stop()
    the_modes_switch_motions()
    the_mask_holds_rather_than_zeroing()
    the_version_gate_refuses()
    a_stale_datagram_is_never_read()
    a_silent_stream_is_not_a_state_that_is_still_arriving()
    the_state_is_read_from_the_right_fields()
    the_stop_path_is_walked_in_every_order()
    a_refused_move_is_not_a_started_one()
    the_declared_body_is_the_vendors()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

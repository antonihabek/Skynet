"""skynet-odrive's own suite: the protocol, the simulated axis, and every refusal the bridge makes.

No test framework and no kernel bus. `drivers/canopen/selftest.py`'s shape, with the same
emphasis: nearly everything asserted here is a **refusal**, and every refusal is written beside
the nearly-identical input that succeeds. An ODrive driver's failure mode is not a crash — it
is a shaft that reports a plausible number and moves somewhere else — so a suite that only
drove the happy path would be checking the one thing that cannot go wrong quietly.

    make selftest     # this file
    make check        # and then the whole path through the real ABI
    make check-vcan   # the same driver on a real SocketCAN interface, where there is one
"""

from __future__ import annotations

import json
import math
import os
import struct
import subprocess
import sys
import time

import skynet_rhal as rhal
from skynet_canopen.bus import Bus, LoopbackBus, SocketCanBus, can_available
from skynet_canopen.frames import Frame

from skynet_odrive import cansimple
from skynet_odrive.axis import SimulatedAxis
from skynet_odrive.bridge import BridgeRefusal, ODriveBridge

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


def build(count: int = 2, **axis_args) -> tuple[ODriveBridge, list[SimulatedAxis], LoopbackBus]:
    bus = LoopbackBus()
    master = bus.endpoint()
    axes = []
    for node in range(count):
        axis = SimulatedAxis(bus.endpoint(), node, **axis_args)
        axes.append(axis)
        bus.attach(axis.poll)
    return ODriveBridge(master), axes, bus


def config(count: int = 2) -> str:
    roles = ["right_arm.shoulder.pitch", "right_arm.elbow.flex"]
    return json.dumps(
        {
            "joints": [
                {"node": n, "name": f"J{n + 1}", "role": roles[n % len(roles)]}
                for n in range(count)
            ]
        }
    )


def opened(count: int = 2, **axis_args) -> tuple[ODriveBridge, list[SimulatedAxis], LoopbackBus]:
    bridge, axes, bus = build(count, **axis_args)
    assert bridge.open(config(count)) == rhal.SKR_OK
    return bridge, axes, bus


class ManualClock:
    """A clock the test owns, for the heartbeat-staleness bound."""

    def __init__(self) -> None:
        self.now = 1000.0

    def __call__(self) -> float:
        return self.now

    def advance(self, seconds: float) -> None:
        self.now += seconds


class RecordingEndpoint(Bus):
    """A loopback endpoint that remembers every frame the bridge sent, in order: what the
    stop actually put on the wire, which no axis's final state can say (an axis that ignored
    the frame ends in the same state as one that was never sent it)."""

    synchronous = True

    def __init__(self, inner: Bus) -> None:
        self._inner = inner
        self.sent: list[Frame] = []

    def send(self, frame: Frame) -> None:
        self.sent.append(frame)
        self._inner.send(frame)

    def recv(self, timeout_s: float = 0.0) -> Frame | None:
        return self._inner.recv(timeout_s)

    def cmds(self) -> list[tuple[int, int]]:
        """Every sent frame as (node, command), in order."""
        return [(cansimple.node_of(f.arbitration_id), cansimple.cmd_of(f.arbitration_id)) for f in self.sent]

    def clear(self) -> None:
        self.sent.clear()


class DroppingEndpoint(Bus):
    """A loopback endpoint that never delivers one identifier — an axis whose reply is lost."""

    synchronous = True

    def __init__(self, inner: Bus, lost: int) -> None:
        self._inner = inner
        self._lost = lost

    def send(self, frame: Frame) -> None:
        self._inner.send(frame)

    def recv(self, timeout_s: float = 0.0) -> Frame | None:
        while True:
            frame = self._inner.recv(timeout_s)
            if frame is None or frame.arbitration_id != self._lost:
                return frame


def main() -> int:
    print("skynet-odrive selftest\n")

    # ── the wire ─────────────────────────────────────────────────────────────
    section("cansimple — identifiers, commands, payloads, units")

    check(cansimple.msg_id(0, cansimple.CMD_HEARTBEAT) == 0x01, "node 0 heartbeats on 0x001")
    check(cansimple.msg_id(63, cansimple.CMD_HEARTBEAT) == 0x7E1, "node 63 heartbeats on 0x7e1")
    check(cansimple.msg_id(2, cansimple.CMD_SET_INPUT_POS) == (2 << 5 | 0x0C),
          "a setpoint goes to node << 5 | command")
    for bad_node in (-1, 64, 1000):
        try:
            cansimple.msg_id(bad_node, cansimple.CMD_HEARTBEAT)
            check(False, f"node {bad_node} is refused")
        except ValueError:
            check(True, f"node {bad_node} is refused")
    for bad_cmd in (-1, 0x20, 0x700):
        try:
            cansimple.msg_id(0, bad_cmd)
            check(False, f"command {bad_cmd:#x} is refused")
        except ValueError:
            check(True, f"command {bad_cmd:#x} is refused")
    check(cansimple.node_of(0x7E1) == 63 and cansimple.cmd_of(0x7E1) == 0x01,
          "an identifier splits back into its node and command")

    heartbeat = struct.pack("<IBBBB", 0x4000, 8, 0, 1, 0x81)
    parsed = cansimple.parse_heartbeat(heartbeat)
    check(parsed == (0x4000, 8, 0, 1, 0x81), "a heartbeat parses to its error, state and flags")
    try:
        cansimple.parse_heartbeat(b"\x01\x02\x03")
        check(False, "a short heartbeat is refused")
    except ValueError:
        check(True, "a short heartbeat is refused")

    estimates = struct.pack("<ff", 1.5, -0.25)
    check(cansimple.parse_estimates(estimates) == (1.5, -0.25), "estimates parse to turns and turns/s")
    try:
        cansimple.parse_estimates(b"\x01\x02\x03\x04")
        check(False, "short estimates are refused")
    except ValueError:
        check(True, "short estimates are refused")

    check(cansimple.parse_error_u32(struct.pack("<I", 0x105) + bytes(4)) == 0x105,
          "a 32-bit error word reads first, the tail is not read")
    check(cansimple.parse_error_u64(struct.pack("<Q", 0x800000000)) == 0x800000000,
          "a 64-bit motor error word reads whole (F280: the DBC models 32)")
    try:
        cansimple.parse_error_u64(struct.pack("<I", 1))
        check(False, "a 4-byte motor error is refused")
    except ValueError:
        check(True, "a 4-byte motor error is refused")

    check(cansimple.rad_to_turns(math.pi) == 0.5, "half a turn is pi radians")
    check(cansimple.turns_to_rad(0.5) == math.pi, "and back")
    for bad in (math.inf, math.nan):
        try:
            cansimple.rad_to_turns(bad)
            check(False, f"{bad} rad is refused")
        except ValueError:
            check(True, f"{bad} rad is refused")
        try:
            cansimple.turns_to_rad(bad)
            check(False, f"{bad} turns is refused")
        except ValueError:
            check(True, f"{bad} turns is refused")
    try:
        cansimple.rad_to_turns(1e40)
        check(False, "an angle past float32 is refused")
    except ValueError:
        check(True, "an angle past float32 is refused")

    ident, payload = cansimple.request(3, cansimple.CMD_GET_ENCODER_ESTIMATES)
    check(ident == (3 << 5 | 0x09) and payload == b"", "a request is a zero-length frame, never RTR")
    check(cansimple.set_requested_state(8) == struct.pack("<i", 8), "a requested state is one int32")
    check(cansimple.set_controller_modes(3, 1) == struct.pack("<ii", 3, 1),
          "modes are two int32s, control first")
    check(cansimple.set_input_pos(1.0) == struct.pack("<fhh", 1.0, 0, 0),
          "a position setpoint carries zero feed-forwards")
    check(cansimple.set_input_pos(0.0, 0.5, -1.25) == struct.pack("<fhh", 0.0, 500, -1250),
          "and nonzero ones scale at 0.001")
    try:
        cansimple.set_input_pos(0.0, 40.0, 0.0)
        check(False, "a feed-forward past int16 is refused")
    except ValueError:
        check(True, "a feed-forward past int16 is refused")
    check(cansimple.set_input_vel(2.0, 0.5) == struct.pack("<ff", 2.0, 0.5),
          "a velocity setpoint is turns/s and Nm")
    check(cansimple.set_input_torque(1.5) == struct.pack("<f", 1.5), "a torque setpoint is one float")
    check(cansimple.estop() == b"", "the stop frame's payload is empty: the id is the message")

    # ── the simulated axis ───────────────────────────────────────────────────
    section("axis — the simulated ODrive axis")

    bus = LoopbackBus()
    axis = SimulatedAxis(bus.endpoint(), 3)
    bus.attach(axis.poll)
    peer = bus.endpoint()
    check(axis.state == cansimple.STATE_IDLE, "an axis boots idle with no error")

    def ask(cmd: int, payload: bytes = b"") -> Frame | None:
        # The bus does not echo to the sender, so everything received is the axis's: drain
        # the stale heartbeats, ask, and read until the reply — which the axis sends before
        # its heartbeat, so it arrives first.
        while peer.recv() is not None:
            pass
        peer.send(Frame(cansimple.msg_id(3, cmd), payload))
        deadline = time.monotonic() + 1.0
        while time.monotonic() < deadline:
            frame = peer.recv()
            if frame is not None and frame.arbitration_id == cansimple.msg_id(3, cmd):
                return frame
        return None

    reply = ask(cansimple.CMD_GET_ENCODER_ESTIMATES)
    check(reply is not None and cansimple.parse_estimates(reply.data) == (0.0, 0.0),
          "it answers estimates at zero")
    reply = ask(cansimple.CMD_GET_ENCODER_COUNT)
    check(reply is not None and struct.unpack("<ii", reply.data) == (0, 0), "and counts at zero")
    reply = ask(cansimple.CMD_GET_MOTOR_ERROR)
    check(reply is not None and cansimple.parse_error_u64(reply.data) == 0, "and a zero motor word")

    def tell(cmd: int, payload: bytes = b"") -> Frame | None:
        # A set with no reply: drain, send, and read the heartbeat the poll ends with —
        # its arrival proves the axis handled the frame.
        while peer.recv() is not None:
            pass
        peer.send(Frame(cansimple.msg_id(3, cmd), payload))
        return peer.recv()

    beat = tell(cansimple.CMD_SET_REQUESTED_STATE, cansimple.set_requested_state(8))
    check(axis.state == 8, "closed loop is granted to a healthy axis")
    check(beat is not None and cansimple.parse_heartbeat(beat.data)[1] == 8,
          "and the next heartbeat says so")
    tell(cansimple.CMD_SET_INPUT_POS, cansimple.set_input_pos(2.0))
    check(axis.input_pos_turns == 2.0, "a setpoint into a closed-loop axis is taken")
    reply = ask(cansimple.CMD_GET_ENCODER_ESTIMATES)
    pos, vel = cansimple.parse_estimates(reply.data) if reply is not None else (0.0, 0.0)
    check(reply is not None and 0.0 < pos < 2.0, "and the shaft moves toward it")
    check(vel > 0.0, "with a positive velocity while it travels")
    tell(cansimple.CMD_SET_REQUESTED_STATE, cansimple.set_requested_state(1))
    check(axis.state == 1, "idle is granted")
    before = axis.input_pos_turns
    tell(cansimple.CMD_SET_INPUT_POS, cansimple.set_input_pos(9.0))
    check(axis.input_pos_turns == before and axis.dropped_setpoints == 1,
          "a setpoint into an idle axis is dropped and counted")

    tell(cansimple.CMD_SET_REQUESTED_STATE, cansimple.set_requested_state(4))
    check(axis.axis_error == 0x1 and axis.refused_states == 1,
          "a calibration state it cannot run sets INVALID_STATE")
    axis.axis_error = 0
    tell(cansimple.CMD_ESTOP)
    check(axis.axis_error == 0x4000 and axis.state == 1, "the stop latches ESTOP_REQUESTED and idles")
    tell(cansimple.CMD_SET_REQUESTED_STATE, cansimple.set_requested_state(8))
    check(axis.state == 1 and axis.refused_states == 2, "a faulted axis does not arm")
    tell(cansimple.CMD_CLEAR_ERRORS)
    check(axis.axis_error == 0, "clearing wipes the word")
    tell(cansimple.CMD_SET_REQUESTED_STATE, cansimple.set_requested_state(8))
    check(axis.state == 8, "and the axis arms again after it")

    ignored = axis.ignored_commands
    tell(0x1F, b"\x01")
    check(axis.ignored_commands == ignored + 1, "an undefined command is ignored and counted")
    peer.send(Frame(cansimple.msg_id(4, cansimple.CMD_GET_ENCODER_ESTIMATES), b""))
    while peer.recv() is not None:
        pass
    check(axis.ignored_commands == ignored + 1, "and another node's frames are not its business")

    axis.motor_error = 0x800000000
    axis.encoder_error = 0x105
    axis.controller_error = 0x2
    reply = ask(cansimple.CMD_GET_MOTOR_ERROR)
    check(reply is not None and cansimple.parse_error_u64(reply.data) == 0x800000000,
          "a staged motor error reads back whole, past 32 bits")
    reply = ask(cansimple.CMD_GET_ENCODER_ERROR)
    check(reply is not None and cansimple.parse_error_u32(reply.data) == 0x105,
          "and the staged encoder word reads back")
    beat = tell(cansimple.CMD_SET_CONTROLLER_MODES, cansimple.set_controller_modes(2, 1))
    check(axis.control_mode == 2 and axis.input_mode == 1, "controller modes are stored")
    check(beat is not None and cansimple.parse_heartbeat(beat.data)[2:5] == (1, 1, 1),
          "and the heartbeat flags every subsystem that holds an error")

    # ── open ─────────────────────────────────────────────────────────────────
    section("bridge — open, and every way it refuses")

    bridge, axes, _bus = build(2)
    check(bridge.open(config(2)) == rhal.SKR_OK, "two healthy axes come up")
    check(all(a.state == 8 for a in axes), "in closed loop, read back and not assumed")
    check(all(a.control_mode == 3 and a.input_mode == 1 for a in axes),
          "in position passthrough, the baseline every command starts from")
    check(bridge.open(config(2)) == rhal.SKR_OK, "opening twice re-arms rather than doubling")
    check(len(bridge.axes) == 2, "two joints, not four")
    bridge.close()
    check(bridge.open(config(2)) == rhal.SKR_OK, "close then open comes back up")
    bridge.close()

    def refuses(settings: dict, fragment: str, what: str) -> None:
        b, _, _ = build(0)
        try:
            b._open(settings)
        except BridgeRefusal as refusal:
            check(fragment in str(refusal), what)
        else:
            check(False, what)

    refuses({}, "no joints", "no joints is refused")
    refuses({"joints": [{"node": 64, "name": "J1"}]}, "0 to 63", "node 64 is refused")
    refuses({"joints": [{"node": -1, "name": "J1"}]}, "0 to 63", "node -1 is refused")
    refuses({"joints": [{"node": True, "name": "J1"}]}, "0 to 63", "node true is refused")
    refuses({"joints": [{"node": "0", "name": "J1"}]}, "0 to 63", 'node "0" is refused')
    refuses({"joints": [{"node": 0}]}, "no `name`", "a joint with no name is refused")
    refuses({"joints": [{"node": 0, "name": "J1"}, {"node": 0, "name": "J2"}]}, "two joints",
            "two joints on one node are refused")
    refuses({"joints": [{"node": 0, "name": "J1", "role": "arm.elbow"}]}, "§5.1",
            "a bad chain is refused")
    refuses({"joints": [{"node": 0, "name": "J1", "role": "right_arm.elbow.bend"}]}, "§5.1",
            "a bad axis is refused")
    refuses({"joints": [{"node": 0, "name": "J1", "role": "right_arm"}]}, "§5.1",
            "a chain alone is refused")
    refuses({"joints": [{"node": 0, "name": "J1", "role": 42}]}, "§5.1",
            "a non-string role is refused")
    refuses({"joints": [{"node": 0, "name": "J1"}], "window_ms": 0}, "not a window",
            "a zero window is refused")
    refuses({"joints": [{"node": 0, "name": "J1"}], "window_ms": True}, "not a window",
            "a boolean window is refused")

    bridge, _, _ = build(0)
    check(bridge.open("{not json") == rhal.SKR_EINVAL, "malformed JSON is EINVAL, not a crash")

    bridge, _, _ = build(2)
    bridge.axes = []  # no sim running: the bus is empty
    lonely = LoopbackBus()
    bridge = ODriveBridge(lonely.endpoint())
    check(bridge.open(config(1)) == rhal.SKR_ENODEV, "an axis that is not there is ENODEV")

    bridge, axes, _bus = build(2, silent=True)
    check(bridge.open(config(2)) == rhal.SKR_ENODEV, "a silent bus is ENODEV")

    bridge, axes, _bus = build(2, heartbeat=False)
    check(bridge.open(config(2)) == rhal.SKR_ENODEV,
          "answers without a heartbeat are ENODEV: unmonitored motion is not commanded")

    bridge, axes, _bus = build(2)
    axes[1].axis_error = 0x4000
    check(bridge.open(config(2)) == rhal.SKR_ENODEV, "a latched fault at open is ENODEV")
    fresh, fresh_axes, _bus = build(2)
    fresh_axes[1].axis_error = 0x4000
    try:
        fresh._open(json.loads(config(2)))
        check(False, "the refusal names the word")
    except BridgeRefusal as refusal:
        check("0x00004000" in str(refusal), "the refusal names the word")

    bridge, axes, _bus = build(2)
    bridge.open(json.dumps({"joints": [{"node": 0, "name": "J1", "role": " Right_Arm.Elbow.Flex#01 "}]}))
    check(bridge.axes[0].role == "right_arm.elbow.flex#1", "a role is normalised the way it parses")
    bridge.close()

    # ── read ─────────────────────────────────────────────────────────────────
    section("bridge — reads")

    bridge, axes, bus = opened(2)
    state = rhal.State()
    check(bridge.read_state(state, 0) == rhal.SKR_OK, "the first read answers")
    check(state.position_rad == [0.0, 0.0], "both shafts at zero")
    check(state.velocity_rad_s == [0.0, 0.0], "both still")
    check(all(math.isnan(e) for e in state.effort_nm), "effort is NaN: amps are not torque")
    check(state.seq == 1 and state.stamp_ns > 0, "the sequence advances under a fresh stamp")
    check(state.flags == rhal.HOMING_UNREPORTED, "no flags on a healthy body")
    check(state.fault_code == 0, "and a zero word")

    command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION,
                           values=[0.5, -0.25], mask=[1, 0])
    check(bridge.write_command(command, 0) == rhal.SKR_OK, "a masked position command is taken")
    bridge.read_state(state, 0)
    check(state.velocity_rad_s[0] > 0.0, "the commanded shaft travels with velocity")
    check(state.position_rad[1] == 0.0, "the masked shaft does not move")
    for _ in range(60):
        bridge.write_command(command, 0)
        bridge.read_state(state, 0)
    check(abs(state.position_rad[0] - 0.5) < 1e-3, "sixty cycles land the shaft on its setpoint")

    axes[1].axis_error = 0x105
    bridge.read_state(state, 0)
    check(state.flags & (rhal.FLAG_FAULT | rhal.FLAG_DEGRADED) != 0, "a staged error faults the body")
    check(state.fault_code == 0x105, "and the word is the maker's, verbatim")
    axes[0].axis_error = 0x200
    bridge.read_state(state, 0)
    check(state.fault_code == 0x200, "with two faults the first speaker is carried")
    axes[0].axis_error = 0
    axes[1].axis_error = 0
    axes[0].state = cansimple.STATE_IDLE
    bridge.read_state(state, 0)
    check(state.flags & rhal.FLAG_FAULT == 0 and state.flags & rhal.FLAG_DEGRADED != 0,
          "idle without an error is degraded, not faulted")
    check(state.fault_code == 0, "with a zero word")
    axes[0].state = cansimple.STATE_CLOSED_LOOP_CONTROL

    clocked_bus = LoopbackBus()
    clock = ManualClock()
    clocked = ODriveBridge(clocked_bus.endpoint(), clock=clock)
    clocked_axes = [SimulatedAxis(clocked_bus.endpoint(), 0)]
    clocked_bus.attach(clocked_axes[0].poll)
    assert clocked.open(json.dumps({"joints": [{"node": 0, "name": "J1"}]})) == rhal.SKR_OK
    clocked_axes[0].heartbeat_enabled = False
    clock.advance(1.5)
    clocked.read_state(state, 0)
    check(state.flags & rhal.FLAG_DEGRADED != 0, "a heartbeat unheard for a second degrades the body")

    bridge, axes, bus = opened(2)
    inner = bridge._bus
    assert inner is not None
    bridge._bus = DroppingEndpoint(inner, cansimple.msg_id(1, cansimple.CMD_GET_ENCODER_ESTIMATES))
    bridge.read_state(state, 0)
    check(state.flags & rhal.FLAG_DEGRADED != 0, "one silent axis of two degrades the body")
    for axis in axes:
        axis.silent = True
    before_seq = state.seq
    check(bridge.read_state(state, 0) == rhal.SKR_ETIMEDOUT, "all silent is ETIMEDOUT, not old data")
    check(state.seq == before_seq + 1, "but the sequence still advances: the read did happen")

    bridge, axes, bus = opened(2)
    rogue = bus.endpoint()
    rogue.send(Frame(cansimple.msg_id(0, cansimple.CMD_GET_ENCODER_ESTIMATES), b"\x01\x02"))
    bridge.read_state(state, 0)
    check(bridge.unexpected_frames == 1, "short estimates are counted, not applied")
    check(state.position_rad == [0.0, 0.0], "and the cycle still answers from the real replies")
    rogue.send(Frame(cansimple.msg_id(0, cansimple.CMD_HEARTBEAT), b"\x01"))
    bridge.read_state(state, 0)
    check(bridge.unexpected_frames == 2, "a short heartbeat is counted too")
    check(bridge.axes[0].axis_state == cansimple.STATE_CLOSED_LOOP_CONTROL,
          "and the known state is kept, not overwritten by garbage")
    rogue.send(Frame(0x123, b"\x01"))
    bridge.read_state(state, 0)
    check(bridge.unexpected_frames == 3, "traffic for nobody is counted")

    # ── write ────────────────────────────────────────────────────────────────
    section("bridge — writes, modes, and what is refused whole")

    def recording(count: int = 2) -> tuple[ODriveBridge, list[SimulatedAxis], RecordingEndpoint]:
        recording_bus = LoopbackBus()
        rec = RecordingEndpoint(recording_bus.endpoint())
        bridge = ODriveBridge(rec)
        axes = []
        for node in range(count):
            axis = SimulatedAxis(recording_bus.endpoint(), node)
            axes.append(axis)
            recording_bus.attach(axis.poll)
        assert bridge.open(config(count)) == rhal.SKR_OK
        rec.clear()
        return bridge, axes, rec

    bridge, axes, rec = recording(2)
    pos = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[0.5, -0.25], mask=[1, 1])
    check(bridge.write_command(pos, 0) == rhal.SKR_OK, "a position command is taken")
    check(rec.cmds() == [(0, cansimple.CMD_SET_INPUT_POS), (1, cansimple.CMD_SET_INPUT_POS)],
          "as one setpoint frame per axis, with no mode frame: position is the baseline")
    expect, _, _ = struct.unpack("<fhh", rec.sent[0].data)
    check(abs(expect - 0.5 / cansimple.TURNS_TO_RAD) < 1e-6, "in turns, little-endian")
    bridge.read_state(rhal.State(), 0)  # writes are fire and forget; the axis acts on the next pump
    check(axes[0].input_pos_turns == expect, "and the axis holds what the frame carried")

    vel = rhal.Command(stamp_ns=1, seq=2, mode=rhal.MODE_VELOCITY, values=[1.0, 2.0], mask=[1, 1])
    rec.clear()
    check(bridge.write_command(vel, 0) == rhal.SKR_OK, "a velocity command is taken")
    check(rec.cmds() == [(0, cansimple.CMD_SET_CONTROLLER_MODES), (0, cansimple.CMD_SET_INPUT_VEL),
                         (1, cansimple.CMD_SET_CONTROLLER_MODES), (1, cansimple.CMD_SET_INPUT_VEL)],
          "with a mode frame ahead of each setpoint on the switch")
    modes = struct.unpack("<ii", rec.sent[0].data)
    check(modes == (cansimple.CONTROL_MODE_VELOCITY, cansimple.INPUT_MODE_PASSTHROUGH),
          "velocity passthrough, the kernel's setpoint unshaped")
    rec.clear()
    check(bridge.write_command(vel, 0) == rhal.SKR_OK, "the same mode again is taken")
    check(rec.cmds() == [(0, cansimple.CMD_SET_INPUT_VEL), (1, cansimple.CMD_SET_INPUT_VEL)],
          "with no mode frame: the mode is set once, not every cycle")

    mixed = rhal.Command(stamp_ns=1, seq=3, mode=rhal.MODE_POSITION, values=[0.1, 3.0],
                         mask=[1, 1], modes=[rhal.JMODE_POSITION, rhal.JMODE_VELOCITY])
    rec.clear()
    check(bridge.write_command(mixed, 0) == rhal.SKR_OK, "a mixed-mode command is taken")
    check(rec.cmds() == [(0, cansimple.CMD_SET_CONTROLLER_MODES), (0, cansimple.CMD_SET_INPUT_POS),
                         (1, cansimple.CMD_SET_INPUT_VEL)],
          "one axis switching back, the other already there")
    inherit = rhal.Command(stamp_ns=1, seq=4, mode=rhal.MODE_TORQUE, values=[0.5, 0.5],
                           mask=[1, 1], modes=[rhal.JMODE_INHERIT, rhal.JMODE_INHERIT])
    rec.clear()
    check(bridge.write_command(inherit, 0) == rhal.SKR_OK, "inherit reads as the body's mode")
    check(rec.cmds() == [(0, cansimple.CMD_SET_CONTROLLER_MODES), (0, cansimple.CMD_SET_INPUT_TORQUE),
                         (1, cansimple.CMD_SET_CONTROLLER_MODES), (1, cansimple.CMD_SET_INPUT_TORQUE)],
          "switching both axes to torque")
    rec.clear()
    check(bridge.write_command(inherit, 0) == rhal.SKR_OK, "torque again is taken")
    check(rec.cmds() == [(0, cansimple.CMD_SET_INPUT_TORQUE), (1, cansimple.CMD_SET_INPUT_TORQUE)],
          "as one torque frame per axis")
    bridge.read_state(rhal.State(), 0)
    check(axes[0].input_torque_nm == 0.5, "in newton-metres, as commanded")

    bad_mode = rhal.Command(stamp_ns=1, seq=5, mode=99, values=[0.0, 0.0], mask=[1, 1])
    check(bridge.write_command(bad_mode, 0) == rhal.SKR_EINVAL, "mode 99 is EINVAL, not coerced")
    bare = rhal.Command(stamp_ns=1, seq=6, mode=rhal.MODE_POSITION, values=[0.0, 0.0],
                        mask=[1, 1], modes=[rhal.MODE_TORQUE, rhal.JMODE_INHERIT])
    check(bridge.write_command(bare, 0) == rhal.SKR_EINVAL,
          "a bare MODE_TORQUE in the modes array is EINVAL (§86.1's offset)")
    short = rhal.Command(stamp_ns=1, seq=7, mode=rhal.MODE_POSITION, values=[0.0], mask=[1])
    check(bridge.write_command(short, 0) == rhal.SKR_EINVAL, "one value for two joints is EINVAL")
    for bad_value in (math.nan, math.inf):
        poisoned = rhal.Command(stamp_ns=1, seq=8, mode=rhal.MODE_POSITION,
                                values=[bad_value, 0.0], mask=[1, 1])
        check(bridge.write_command(poisoned, 0) == rhal.SKR_EINVAL, f"{bad_value} is EINVAL")

    rec.clear()
    held = rhal.Command(stamp_ns=1, seq=9, mode=rhal.MODE_POSITION, values=[9.0, 9.0], mask=[1, 0])
    check(bridge.write_command(held, 0) == rhal.SKR_OK, "a half-masked command is taken")
    check(rec.cmds() == [(0, cansimple.CMD_SET_CONTROLLER_MODES), (0, cansimple.CMD_SET_INPUT_POS)],
          "and the masked joint is sent nothing: the axis holds its last input")
    bridge.read_state(rhal.State(), 0)
    check(axes[1].input_pos_turns != 9.0 / cansimple.TURNS_TO_RAD, "which the axis confirms")

    before = axes[0].input_pos_turns
    rec.clear()
    poisoned = rhal.Command(stamp_ns=1, seq=10, mode=rhal.MODE_POSITION,
                            values=[0.5, math.nan], mask=[1, 1])
    check(bridge.write_command(poisoned, 0) == rhal.SKR_EINVAL, "a bad second joint refuses the command")
    check(rec.sent == [], "with no frame sent: the first joint was not commanded either")
    check(axes[0].input_pos_turns == before, "and the axis confirms it heard nothing")

    bridge.write_command(vel, 0)
    bridge.read_state(rhal.State(), 0)
    check(all(a.control_mode == cansimple.CONTROL_MODE_VELOCITY for a in axes),
          "both axes in velocity (setup for the skew check)")
    rec.clear()
    skewed = rhal.Command(stamp_ns=1, seq=11, mode=rhal.MODE_POSITION,
                          values=[0.5, math.nan], mask=[1, 1])
    check(bridge.write_command(skewed, 0) == rhal.SKR_EINVAL, "a mode switch with a bad joint is EINVAL")
    check(all(a.control_mode == cansimple.CONTROL_MODE_VELOCITY for a in bridge.axes),
          "and the driver still believes velocity: nothing was stored before the refusal")
    rec.clear()
    fixed = rhal.Command(stamp_ns=1, seq=12, mode=rhal.MODE_POSITION,
                         values=[0.5, 0.5], mask=[1, 1])
    check(bridge.write_command(fixed, 0) == rhal.SKR_OK, "the fixed command is taken")
    check([cmd for _, cmd in rec.cmds()].count(cansimple.CMD_SET_CONTROLLER_MODES) == 2,
          "resending both mode frames the refused one never sent")

    axes[1].axis_error = 0x105
    bridge.read_state(rhal.State(), 0)
    rec.clear()
    check(bridge.write_command(pos, 0) == rhal.SKR_EPERM, "a faulted body refuses every command")
    check(rec.sent == [], "with no frame sent: a partial body does not move")
    axes[1].axis_error = 0
    axes[0].state = cansimple.STATE_IDLE
    bridge.read_state(rhal.State(), 0)
    rec.clear()
    check(bridge.write_command(pos, 0) == rhal.SKR_EPERM, "an idle axis refuses this cycle")
    check(rec.cmds() == [(0, cansimple.CMD_SET_REQUESTED_STATE)],
          "but is asked for closed loop again: the re-arm a cleared stop needs")
    bridge.read_state(rhal.State(), 0)
    check(bridge.write_command(pos, 0) == rhal.SKR_OK, "and takes commands once it reports back")

    # ── stop ─────────────────────────────────────────────────────────────────
    section("bridge — the stop, the release, and what the firmware keeps")

    bridge, axes, rec = recording(2)
    check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    check(rec.cmds() == [(0, cansimple.CMD_ESTOP), (1, cansimple.CMD_ESTOP)],
          "one stop frame per axis, fire and forget")
    state = rhal.State()
    bridge.read_state(state, 0)
    check(state.flags & rhal.FLAG_ESTOP != 0, "the stop is visible in the next state")
    check(state.fault_code == 0x4000, "with the firmware's ESTOP_REQUESTED in the word")
    check(bridge.write_command(pos, 0) == rhal.SKR_EPERM, "and later commands are refused")
    check(bridge.release_stop() == rhal.SKR_ENOSYS,
          "release is ENOSYS: a stopped body is renewed by a new session, not released in place")
    check(bridge.write_command(pos, 0) == rhal.SKR_EPERM, "and the session stays refused after it")
    bridge.close()
    check(bridge.open(config(2)) == rhal.SKR_ENODEV,
          "a new session over the uncleared drive refuses too")
    for axis in axes:
        axis._clear_errors()  # the person's act at the drive, never the driver's
    check(bridge.release_stop() == rhal.SKR_ENOSYS, "release is ENOSYS even with nothing latched")
    check(bridge.open(config(2)) == rhal.SKR_OK,
          "after the person's clear a new session comes up")
    check(bridge.write_command(pos, 0) == rhal.SKR_OK, "and commands flow again")
    rec.clear()
    bridge.close()
    check(rec.cmds() == [(0, cansimple.CMD_SET_REQUESTED_STATE), (1, cansimple.CMD_SET_REQUESTED_STATE)],
          "close parks every axis in idle")
    check(all(struct.unpack("<i", f.data)[0] == cansimple.STATE_IDLE for f in rec.sent),
          "idle, not closed loop and not a guess")
    check(bridge.read_state(rhal.State(), 0) == rhal.SKR_EPERM, "a closed body refuses reads too")

    bridge, axes, rec = recording(2)
    session = rhal.State()
    bridge.read_state(session, 0)
    bridge.write_command(pos, 0)
    bridge.emergency_stop()
    bridge.read_state(session, 0)
    bridge.release_stop()
    bridge.close()
    check(all(cmd != cansimple.CMD_CLEAR_ERRORS for _, cmd in rec.cmds()),
          "no path in the session cleared an error: that frame is never sent")

    # ── describe ─────────────────────────────────────────────────────────────
    section("bridge — the descriptor")

    bridge, _, _ = opened(2)
    descriptor = json.loads(bridge.describe())
    check(descriptor["robot_slug"] == "odrive-cansimple-axes", "the slug names the protocol")
    check(descriptor["dof"] == 2 and descriptor["base"] == "fixed", "two joints on a fixed base")
    check(descriptor["simulator"] is True, "simulated on a loopback bus")
    check(all(j["type"] == "continuous" for j in descriptor["joints"]),
          "continuous joints: the wire publishes no travel")
    check(all(j["limits"] == {} for j in descriptor["joints"]),
          "with no limits: no rating for the motor on the axis")
    check(all(j["control_modes"] == ["position", "velocity", "torque"] for j in descriptor["joints"]),
          "all three modes per joint")
    check(descriptor["control"]["modes"] == ["position", "velocity", "torque"],
          "and on the body")
    check(descriptor["control"]["per_joint_modes"] is True, "mixed in one command")
    check(descriptor["envelope"] is None, "no envelope: configuration is not a safety claim")
    check(descriptor["capabilities"] == ["safety.estop"],
          "one capability: the software stop the driver has")
    check([j["role"] for j in descriptor["joints"]] == ["right_arm.shoulder.pitch", "right_arm.elbow.flex"],
          "roles carried from the configuration")
    bridge.close()

    bridge, _, _ = build(2)
    bridge.open(json.dumps({"joints": [{"node": 0, "name": "J1"}], "simulator": False}))
    check(json.loads(bridge.describe())["simulator"] is True,
          "loopback forces simulated even when the configuration claims hardware")
    bridge.close()

    check(bridge.read_sensor("camera", rhal.Frame()) == rhal.SKR_ENOSYS, "no sensors to read")

    from skynet_odrive import __main__ as entry

    previous = os.environ.get("SKYNET_ODRIVE_SIM")
    os.environ["SKYNET_ODRIVE_SIM"] = "bogus"
    try:
        check(entry.main() == 1, "a non-numeric SKYNET_ODRIVE_SIM count refuses")
    finally:
        if previous is None:
            del os.environ["SKYNET_ODRIVE_SIM"]
        else:
            os.environ["SKYNET_ODRIVE_SIM"] = previous

    # ── vcan ─────────────────────────────────────────────────────────────────
    if "--vcan" in sys.argv[1:]:
        vcan_checks("vcan0")
    elif can_available():
        check(True, "this kernel has CAN support; `make check-vcan` runs the driver against it")
    else:
        print(
            "  ----  this kernel has no CAN support, so SocketCanBus was not exercised at all.\n"
            "        `vcan` needs `modprobe`, a kernel with CONFIG_CAN, and the privilege to load\n"
            "        a module. What ran above is the loopback bus: the identical protocol with no\n"
            "        arbitration, no bit timing and no bandwidth — so it says nothing about whether\n"
            "        eight axes fit in a millisecond at 1 Mbit. That row stays in\n"
            "        DRY-COVERAGE-PLAN.md's `not dry-verifiable` column."
        )

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 1 if FAILURES else 0


def vcan_checks(interface: str) -> None:
    """The bridge on a kernel CAN bus, against simulated axes in another process.

    `make check-vcan` is the way in: it creates `vcan0` in an unprivileged network namespace when
    there is none, which needs no root. Asked for and not possible is a failure, never a skip —
    `--vcan` is a request to measure this, and a run that measured nothing must not pass.
    """
    section(f"bridge — on {interface}, against axes in another process")
    if not can_available():
        check(False, "--vcan was asked for and this kernel will not give out a CAN socket")
        return

    here = os.path.dirname(os.path.abspath(__file__))
    environment = dict(os.environ)
    environment["PYTHONPATH"] = os.pathsep.join(
        [here, os.path.join(here, "..", "canopen"), os.path.join(here, "..", "..", "bindings", "python")]
    )

    def axes(*extra: str) -> subprocess.Popen:
        process = subprocess.Popen(
            [sys.executable, "-m", "skynet_odrive.serve", interface, "0", "1", *extra],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            env=environment,
        )
        assert process.stdout is not None
        if process.stdout.readline().strip() != "ready":
            raise AssertionError("the axes did not come up")
        return process

    def stop(process: subprocess.Popen) -> None:
        assert process.stdin is not None
        process.stdin.close()
        process.wait(timeout=10)

    process = axes()
    try:
        bridge = ODriveBridge(SocketCanBus(interface))
        check(bridge.open(config(2)) == rhal.SKR_OK, "two axes in another process come up")
        state = rhal.State()
        check(
            bridge.read_state(state, 0) == rhal.SKR_OK,
            "the first read answers: the replies arrive after the requests, and it waits",
        )
        command = rhal.Command(
            stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[0.5, -0.25], mask=[1, 0]
        )
        failures = 0
        for _ in range(60):
            bridge.write_command(command, 0)
            if bridge.read_state(state, 0) != rhal.SKR_OK:
                failures += 1
        check(failures == 0, "sixty commanded cycles, and every read answered")
        check(
            abs(state.position_rad[0] - 0.5) < 1e-3 and state.position_rad[1] == 0.0,
            "and the commanded shaft reached its setpoint over the kernel's bus",
        )
        check(state.flags & rhal.FLAG_DEGRADED == 0, "with no axis missing a cycle")
        check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
        bridge.read_state(state, 0)
        check(state.flags & rhal.FLAG_ESTOP != 0, "the stop is visible in the next state")
        check(bridge.write_command(command, 0) == rhal.SKR_EPERM, "and later commands are refused")
        bridge.close()
    finally:
        stop(process)

    process = axes("--error", "1", "0x4000")
    try:
        bridge = ODriveBridge(SocketCanBus(interface))
        check(
            bridge.open(config(2)) == rhal.SKR_ENODEV,
            "a body with an axis that latched a fault does not come up",
        )
    finally:
        stop(process)


if __name__ == "__main__":
    raise SystemExit(main())

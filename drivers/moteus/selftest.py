"""skynet-moteus's own suite: the wire, the simulated controllers, and every refusal the bridge makes.

No test framework and no kernel bus. `drivers/piper/selftest.py`'s shape and emphasis: nearly everything asserted here is a **refusal**, and every refusal is
written beside the nearly-identical input that succeeds. A moteus driver's failure mode is not a crash — it is a controller that reports a plausible position and
is not doing what it was told: held in a fault, fallen back on its watchdog, stopped by somebody else, or driving a joint to a position the driver believes it
declared. So a suite that only drove the happy path would be checking the one thing that cannot go wrong quietly.

    make selftest     # this file
    make check        # and then the whole path through the real ABI
    make check-vcan   # the same driver on a kernel CAN-FD bus, controllers in another process
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import signal
import struct
import subprocess
import sys

import skynet_rhal as rhal

from skynet_moteus import protocol as p
from skynet_moteus.bridge import MoteusBridge
from skynet_moteus.bus import Bus, FdFrame, LoopbackBus, SocketCanFdBus, can_fd_available
from skynet_moteus.sim import SimulatedController, SimulatedMoteus

FAILURES: list[str] = []
CHECKS = 0
TWO_PI = 2.0 * math.pi


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


def near(a: float, b: float, tolerance: float = 2e-3) -> bool:
    return abs(a - b) <= tolerance


def said(call) -> tuple[object, str]:
    """What a call returned and what it told the operator on stderr: a refusal is its reason as much as its code, and two reasons for one code are two next actions."""
    captured = io.StringIO()
    with contextlib.redirect_stderr(captured):
        result = call()
    return result, captured.getvalue()


def refused(call) -> bool:
    try:
        call()
    except ValueError:
        return True
    return False


def refused_within(call, seconds: float = 2.0) -> bool:
    """`refused`, for a call a defect could make loop for ever (a varuint loop that never reaches zero also grows the list it appends to): a hang is a failed
    check here, in two seconds, and not a suite that sits until the harness gives up on it."""
    if not hasattr(signal, "setitimer"):
        return refused(call)

    def give_up(signum, frame):
        raise TimeoutError(f"still running after {seconds} s")

    previous = signal.signal(signal.SIGALRM, give_up)
    signal.setitimer(signal.ITIMER_REAL, seconds)
    try:
        return refused(call)
    except TimeoutError:
        return False
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


def why_refused(call) -> str:
    """Why a call refused with ValueError, and an empty string when it did not: two refusals for one frame are two things to tell the operator."""
    try:
        call()
    except ValueError as error:
        return str(error)
    return ""


class Recording(Bus):
    """A loopback endpoint that remembers every frame the bridge sent, in order: what the bridge actually put on the wire, which no controller's final state can say."""

    synchronous = True

    def __init__(self, inner: Bus) -> None:
        self._inner = inner
        self.sent: list[FdFrame] = []

    def send(self, frame: FdFrame) -> None:
        self.sent.append(frame)
        self._inner.send(frame)

    def recv(self, timeout_s: float = 0.0) -> FdFrame | None:
        return self._inner.recv(timeout_s)

    def backlog(self) -> int | None:
        return self._inner.backlog()

    def clear(self) -> None:
        self.sent.clear()

    def to(self, controller: int) -> list[FdFrame]:
        return [f for f in self.sent if (f.arbitration_id & 0x7F) == controller]

    def commands(self, controller: int) -> list[list]:
        """The subframes of every frame to ``controller`` that is a command (it writes the mode register)."""
        out = []
        for frame in self.to(controller):
            items = p.parse_frame(frame.data)
            if any(isinstance(i, p.Register) and i.kind == "write" and i.register == p.MODE for i in items):
                out.append(items)
        return out


class Waiting(Recording):
    """A bus that, like a kernel socket and unlike the in-process loopback, does not say at once that nothing more is coming: the bridge has to wait out its own
    timeouts, and with a ``SteppingClock`` a wait of half a second is five hundred looks and not half a second."""

    synchronous = False


def late(master: Recording, loop: LoopbackBus, frame: FdFrame) -> None:
    """Put ``frame`` on the bus right after the next frame the bridge receives: it arrives while a cycle is being collected, behind what that cycle asked for."""
    original = master.recv

    def recv(timeout_s: float = 0.0) -> FdFrame | None:
        got = original(timeout_s)
        if got is not None:
            master.recv = original
            loop.endpoint().send(frame)
        return got

    master.recv = recv


class ManualClock:
    """A clock the test owns, for staleness."""

    def __init__(self) -> None:
        self.now = 1000.0

    def __call__(self) -> float:
        return self.now

    def advance(self, seconds: float) -> None:
        self.now += seconds


class SteppingClock(ManualClock):
    """A clock that moves a millisecond every time it is read: a wait for a deadline ends after a thousand looks and not after a second of real time."""

    def __call__(self) -> float:
        self.now += 0.001
        return self.now


IDS = (1, 2)


def build(ids=IDS, clock=None, waiting=False, **controller_args) -> tuple[MoteusBridge, SimulatedMoteus, Recording, LoopbackBus]:
    loop = LoopbackBus()
    master = (Waiting if waiting else Recording)(loop.endpoint())
    controller_args.setdefault("position_min", -0.25)
    controller_args.setdefault("position_max", 0.25)
    sim = SimulatedMoteus(loop.endpoint(), list(ids), **controller_args)
    loop.attach(sim.poll)
    bridge = MoteusBridge(master, clock=clock) if clock is not None else MoteusBridge(master)
    return bridge, sim, master, loop


def settings(ids=IDS, **overrides) -> dict:
    joints = [{"id": ident, "name": f"J{index + 1}"} for index, ident in enumerate(ids)]
    base: dict = {"joints": joints, "stop_mode": "zero_velocity"}
    base.update(overrides)
    return {key: value for key, value in base.items() if value is not None}


def config(ids=IDS, **overrides) -> str:
    return json.dumps(settings(ids, **overrides))


def opened(ids=IDS, clock=None, sim_args=None, **overrides) -> tuple[MoteusBridge, SimulatedMoteus, Recording, LoopbackBus]:
    bridge, sim, master, loop = build(ids, clock=clock, **(sim_args or {}))
    result = bridge.open(config(ids, **overrides))
    assert result == rhal.SKR_OK, f"open returned {result}"
    return bridge, sim, master, loop


def command(values: list[float], mask: list[int] | None = None, mode: int = rhal.MODE_POSITION, modes: list[int] | None = None) -> rhal.Command:
    return rhal.Command(mode=mode, values=list(values), mask=list(mask if mask is not None else [1] * len(values)), modes=list(modes or []))


def read(bridge: MoteusBridge) -> tuple[int, rhal.State]:
    state = rhal.State()
    return bridge.read_state(state, 0), state


def settle(bridge: MoteusBridge, reads: int = 80) -> rhal.State:
    state = rhal.State()
    for _ in range(reads):
        bridge.read_state(state, 0)
    return state


def drive(bridge: MoteusBridge, values: list[float], cycles: int = 80, **kwargs) -> rhal.State:
    """What the loop does: command, then read, every cycle."""
    state = rhal.State()
    for _ in range(cycles):
        bridge.write_command(command(values, **kwargs), 0)
        bridge.read_state(state, 0)
    return state


GOOD = [0.5, -0.3]


def ask(controller: SimulatedController, payload: bytes, reply: bool = True, source: int = 0) -> list:
    """One frame to one simulated controller, and the subframes of what it answered."""
    frame = controller.handle(payload, reply, source)
    return [] if frame is None else p.parse_frame(frame.data)


def registers(subframes: list) -> dict[int, float | int]:
    return {item.register: item.value for item in subframes if isinstance(item, p.Register)}


def main() -> int:
    print("skynet-moteus selftest\n")

    # ── the wire ─────────────────────────────────────────────────────────────
    section("the wire — registers, quantities and frames, in the maker's own layouts")
    doc = p.write_registers(p.MODE, p.INT8, [10]) + p.write_registers(0x20, p.INT16, [96, 288, -176]) + p.read_registers(0, p.INT16, 4) + p.read_registers(0x0D, p.INT8, 3)
    check(doc == bytes.fromhex("01000a07206000200150ff140400130d"), "the maker's own example command (docs/protocol/can.md) is reproduced byte for byte")
    reply = registers(p.parse_frame(bytes.fromhex("2404000a005000000170ff230d181400")))
    check(reply == {0: 10, 1: 80, 2: 256, 3: -144, 13: 24, 14: 20, 15: 0}, "…and so is its documented reply read: mode 10, 80, 256, -144 and a clean 24, 20, 0")
    check(near(p.decode(80, p.INT16, p.POSITION), 0.008, 1e-12) and near(p.decode(256, p.INT16, p.VELOCITY), 0.064, 1e-12) and near(p.decode(-144, p.INT16, p.TORQUE), -1.44, 1e-12),
          "…as the maker decodes it: 2.88 degrees, 23.04 degrees a second, -1.44 N·m")
    check(near(p.decode(24, p.INT8, p.VOLTAGE), 12.0, 1e-12) and p.decode(20, p.INT8, p.TEMPERATURE) == 20.0, "…and 24 volts counts at half a volt is 12, and 20 is 20 °C")
    check(p.varuint(0) == b"\x00" and p.varuint(127) == b"\x7f" and p.varuint(128) == b"\x80\x01" and p.varuint(16384) == b"\x80\x80\x01" and p.varuint(0xFFFFFFFF) == bytes.fromhex("ffffffff0f"),
          "a varuint is seven bits a byte, least significant first, with the top bit meaning more follow")
    check(all(p.read_varuint(p.varuint(v), 0) == (v, len(p.varuint(v))) for v in (0, 1, 127, 128, 300, 16383, 16384, 2**21, 2**28, 0xFFFFFFFF)), "…and reads back every length it writes")
    check(refused_within(lambda: p.varuint(-1)) and refused_within(lambda: p.varuint(2**32)), "a varuint is a uint32 and nothing wider or negative")
    check(refused(lambda: p.read_varuint(b"\x80", 0)) and refused(lambda: p.read_varuint(bytes([0x80] * 6), 0)), "…a varuint that runs off the frame, or past five bytes, is refused and not truncated")
    check(p.can_id(0, 1, True) == 0x8001 and p.can_id(0, 1, False) == 0x0001 and p.can_id(1, 0x7F, False) == 0x017F, "a conversation id is the sender over the controller, the sender's top bit meaning reply")
    check(p.split_id(0x0100) == (1, 0, False) and p.split_id(0x8001) == (0, 1, True), "…and the reply to 0x8001 is 0x0100: from controller 1, to sender 0")
    check(refused(lambda: p.can_id(0, 0, True)) and refused(lambda: p.can_id(0, 128, True)) and refused(lambda: p.can_id(128, 1, True)), "controller 0 is the broadcast address, and neither id is wider than seven bits")
    check([len(p.pad(bytes(n))) for n in (0, 1, 8, 9, 12, 13, 16, 17, 20, 21, 24, 25, 32, 33, 48, 49, 64)] == [0, 1, 8, 12, 12, 16, 16, 20, 20, 24, 24, 32, 32, 48, 48, 64, 64],
          "a payload is padded up to a length CAN-FD carries")
    check(p.pad(b"\x01\x02\x03\x04\x05\x06\x07\x08\x09") == b"\x01\x02\x03\x04\x05\x06\x07\x08\x09\x50\x50\x50" and refused(lambda: p.pad(bytes(65))),
          "…with the no-op subframe 0x50, and past 64 bytes it is refused, not split into two commands")
    check(p.read_registers(p.MODE, p.INT8, 1) == bytes.fromhex("1100") and p.read_registers(1, p.F32, 3) == bytes.fromhex("1f01") and p.read_registers(0x0C, p.INT8, 4) == bytes.fromhex("10040c"),
          "a read of one to three registers folds its count into the command byte, and four or more write it out")
    check(p.query() == bytes.fromhex("11001f0110040c"), "the query asks for mode, position, velocity, torque, home state, voltage, temperature and fault, in the runs the maker's own client groups them in")
    check(p.position_command(0.25, 0.0, 0.0, 1.0, 1.0, math.nan, None) == bytes.fromhex("01000a0c06200000803e00000000000000000000803f0000803f0000c07f"),
          "a position command is the mode write, then registers 0x020 to 0x025 as one run of six floats")
    check(p.position_command(0.25, 0.0, 0.0, 1.0, 1.0, 5.0, 0.2).endswith(bytes.fromhex("0000a0400d27cdcc4c3e")), "…the ceiling is its sixth register and the watchdog follows as a register of its own, 0x027")
    check(p.stop_command(0) == bytes.fromhex("010000") and p.stop_command(12) == bytes.fromhex("01000c") and p.stop_command(15) == bytes.fromhex("01000f"),
          "a stop is one write of the mode register: stopped, zero velocity or brake")
    check(all(refused(lambda m=m: p.stop_command(m)) for m in (p.MODE_POSITION, p.MODE_FAULT, p.MODE_TIMEOUT, p.MODE_CURRENT, 99)), "…and nothing else is a stop")
    check(p.diagnostic_write(1, b"conf get x\n") == bytes.fromhex("40010b636f6e662067657420780a") and p.diagnostic_poll(1) == bytes.fromhex("420130"),
          "the diagnostic channel is a client write and a poll of at most 48 bytes (0x40 and 0x42)")

    # quantities at every width
    check(p.encode(0.00965, p.INT16, p.POSITION) == 96 and p.encode(0.0721, p.INT16, p.VELOCITY) == 288 and p.encode(1.765, p.INT16, p.TORQUE) == 176,
          "the maker's own example values map to its own integers (a hair above each, so a float's last bit cannot move the truncation): 3.456 degrees is 96, 25.92 dps is 288, 1.76 N·m is 176")
    check(p.encode(0.00976, p.INT16, p.POSITION) == 97 and p.encode(-0.00976, p.INT16, p.POSITION) == -97 and p.encode(0.0757, p.INT16, p.TORQUE) == 7,
          "the integer mapping truncates toward zero as the maker's `saturate` does, and does not round: 97.6 counts is 97, and -97.6 is -97")
    check(p.encode(0.505, p.INT8, p.POSITION) == 50 and p.encode(0.505005, p.INT32, p.POSITION) == 50500 and p.encode(1.05, p.INT8, p.VOLTAGE) == 2 and p.encode(10.0, p.INT8, p.TIME) == 127,
          "a position is 0.01, 0.0001 or 0.00001 of a revolution by width, a voltage 0.5, 0.1 or 0.001 volts, and a value past the range saturates")
    check(p.encode(math.nan, p.INT8, p.POSITION) == -128 and p.encode(math.nan, p.INT16, p.POSITION) == -32768 and p.encode(math.inf, p.INT32, p.POSITION) == -2147483648
          and math.isnan(p.encode(math.nan, p.F32, p.POSITION)), "NaN is the most negative integer at each width, and a float's own")
    check(all(math.isnan(p.decode(raw, res, p.POSITION)) for raw, res in ((-128, p.INT8), (-32768, p.INT16), (-2147483648, p.INT32))) and p.decode(-127, p.INT8, p.POSITION) == -1.27,
          "…and the most negative integer reads back as NaN where the one above it is a value")
    for name, scales in (("POSITION", p.POSITION), ("VELOCITY", p.VELOCITY), ("TORQUE", p.TORQUE), ("VOLTAGE", p.VOLTAGE), ("TEMPERATURE", p.TEMPERATURE),
                         ("TIME", p.TIME), ("ACCEL", p.ACCEL), ("CURRENT", p.CURRENT), ("POWER", p.POWER)):
        expected = {"POSITION": (0.01, 0.0001, 0.00001), "VELOCITY": (0.1, 0.00025, 0.00001), "TORQUE": (0.5, 0.01, 0.001), "VOLTAGE": (0.5, 0.1, 0.001),
                    "TEMPERATURE": (1.0, 0.1, 0.001), "TIME": (0.01, 0.001, 0.000001), "ACCEL": (0.05, 0.001, 0.00001), "CURRENT": (1.0, 0.1, 0.001),
                    "POWER": (10.0, 0.05, 0.0001)}[name]
        check(tuple(scales) == expected, f"{name.lower()} counts {expected[0]}, {expected[1]} and {expected[2]} at int8, int16 and int32 (registers.md, Mappings)")
    check(near(p.PWM[0], 1 / 127, 1e-12) and near(p.PWM[1], 1 / 32767, 1e-12) and near(p.PWM[2], 1 / 2147483647, 1e-15), "a scale factor counts 1/127, 1/32767 and 1/2147483647")

    # the table of registers, modes and faults, as literals
    check((p.MODE, p.POSITION_REG, p.VELOCITY_REG, p.TORQUE_REG, p.HOME_STATE, p.VOLTAGE_REG, p.TEMPERATURE_REG, p.FAULT) == (0x000, 0x001, 0x002, 0x003, 0x00C, 0x00D, 0x00E, 0x00F),
          "the state registers are 0x000 (mode), 0x001 to 0x003 (position, velocity, torque), 0x00C (home state) and 0x00D to 0x00F (voltage, temperature, fault)")
    check((p.COMMAND_POSITION, p.COMMAND_VELOCITY, p.COMMAND_FEEDFORWARD_TORQUE, p.COMMAND_KP_SCALE, p.COMMAND_KD_SCALE, p.COMMAND_MAX_TORQUE, p.COMMAND_WATCHDOG)
          == (0x020, 0x021, 0x022, 0x023, 0x024, 0x025, 0x027), "…and the command registers 0x020 to 0x025 and the watchdog 0x027")
    check({m: n for m, n in p.MODES.items()} == {0: "stopped", 1: "fault", 2: "enabling", 3: "calibrating", 4: "calibration complete", 5: "pwm", 6: "voltage", 7: "voltage foc", 8: "voltage dq",
                                                  9: "current", 10: "position", 11: "timeout", 12: "zero velocity", 13: "stay within", 14: "measure inductance", 15: "brake"},
          "the mode numbers are the firmware's: 0 stopped, 1 fault, 10 position, 11 timeout, 12 zero velocity, 15 brake")
    check(sorted(p.FAULTS) == list(range(32, 51)) and p.FAULTS[32] == "calibration fault" and p.FAULTS[39] == "outside limit" and p.FAULTS[50] == "velocity control error",
          "the fault codes are 32 to 50, from the calibration fault to the velocity control error")
    check(sorted(p.LIMITING) == list(range(96, 106)) and p.fault_name(99) == "servo.max_current_A" and p.fault_name(104) == "flux braking is active" and p.fault_name(77) == "code 77",
          "…and 96 to 105 are the functions that limit the output, which are not faults (the register reference stops at 103; the firmware sends 104 and 105 too); any other number is only a number")

    # frames
    check(p.parse_frame(b"\x50\x50\x01\x00\x0a\x50") == [p.Register("write", 0, p.INT8, 10)], "a frame's no-op padding is skipped wherever it falls")
    check(p.parse_frame(p.error_subframe("write", 0x25, 1) + p.error_subframe("read", 0x400, 2)) == [p.RegisterError("write", 0x25, 1), p.RegisterError("read", 0x400, 2)],
          "a write error and a read error name the register and the code")
    check(p.parse_frame(p.stream_server_data(1, b"-0.5\r\n")) == [p.StreamData("server", 1, b"-0.5\r\n")] and p.parse_frame(p.stream_poll(1, 48)) == [p.StreamData("poll", 1, bytes([48]))],
          "a stream subframe carries its channel and its bytes; a poll carries the most the client will take")
    check(refused(lambda: p.parse_frame(b"\x55")) and refused(lambda: p.parse_frame(b"\x60")), "a command byte the format does not define is refused, where the maker's parser stops at it quietly")
    check(refused(lambda: p.parse_frame(b"\x01\x00")) and refused(lambda: p.parse_frame(bytes.fromhex("0c0620000000"))) and refused(lambda: p.parse_frame(b"\x40\x01\x05ab")),
          "a subframe cut off mid-register, or mid-stream, is refused and not read as the registers that did arrive")
    check(refused(lambda: p.parse_frame(b"\x00\x00\x20")) and refused(lambda: p.parse_frame(b"\x10\x00\x00")), "a run of zero registers is refused")
    check(p.parse_frame(p.write_registers(0x20, p.F32, [1.0, 2.0, 3.0, 4.0, 5.0])) == [p.Register("write", 0x20 + i, p.F32, float(i + 1)) for i in range(5)],
          "a run of five floats is written once and read back as five registers in order")

    # status
    def status_frame(mode=10, position=0.25, velocity=0.0, torque=0.0, home=2, voltage=24.0, temperature=30.0, fault=0) -> bytes:
        return (p.reply_registers(0, p.INT8, [mode]) + p.reply_registers(1, p.F32, [position, velocity, torque])
                + p.reply_registers(0x0C, p.INT8, [home, p.encode(voltage, p.INT8, p.VOLTAGE), p.encode(temperature, p.INT8, p.TEMPERATURE), fault]))

    status = p.parse_status(status_frame())
    check(status == p.Status(10, 0.25, 0.0, 0.0, 2, 24.0, 30.0, 0), "the answer to the query is a status: mode, position, velocity, torque, home state, voltage, temperature, fault")
    check(refused(lambda: p.parse_status(status_frame()[:-3])), "…a status cut off before its last register is refused, not a status of what arrived")
    check(refused(lambda: p.parse_status(status_frame() + p.reply_registers(0x20, p.F32, [0.0]))), "…and so is one carrying a register nobody asked for")
    check(refused(lambda: p.parse_status(p.reply_registers(0, p.INT16, [10]) + p.reply_registers(1, p.F32, [0.0, 0.0, 0.0]) + p.reply_registers(0x0C, p.INT8, [2, 48, 30, 0]))),
          "…and one that answers a register in another resolution than it was asked in")
    check(refused(lambda: p.parse_status(status_frame() + p.error_subframe("read", 0x0F, 1))) and refused(lambda: p.parse_status(p.write_registers(0, p.INT8, [10]))), "…and one that carries an error, or is not a reply at all")

    # ── the simulated controllers ────────────────────────────────────────────
    section("the simulated controller — what it does with a frame (each from the firmware's mode machine)")
    c = SimulatedController(1, position_min=-0.25, position_max=0.25)
    check(ask(c, p.query(), reply=False) == [] and c.received[-1] == p.query(), "a frame whose sender byte has no reply bit is acted on in silence, query included")
    check(registers(ask(c, p.query())) == {0: 0, 1: 0.0, 2: 0.0, 3: 0.0, 12: 2, 13: 48, 14: 30, 15: 0}, "…and one that has it is answered: stopped, at rest, 24 volts, 30 degrees, no fault")
    errors = [i for i in ask(c, p.query() + p.read_registers(0x400, p.INT8, 1)) if isinstance(i, p.RegisterError)]
    check(errors == [p.RegisterError("read", 0x400, 1)], "a read of a register the controller does not have is answered with a read error naming it")
    errors = [i for i in ask(c, p.write_registers(0x400, p.INT8, [1]) + p.query()) if isinstance(i, p.RegisterError)]
    check(errors == [p.RegisterError("write", 0x400, 1)], "…and a write of one with a write error")
    errors = [i for i in ask(c, p.write_registers(p.MODE, p.INT8, [1]) + p.query()) if isinstance(i, p.RegisterError)]
    check(errors == [p.RegisterError("write", 0, 2)] and c.mode == p.MODE_STOPPED, "fault is the controller's own to enter: a write of mode 1 is refused, and so is each of the preparing modes")
    check(all(any(isinstance(i, p.RegisterError) for i in ask(c, p.write_registers(p.MODE, p.INT8, [m]) + p.query())) for m in (2, 3, 4)), "…2, 3 and 4 too")

    def hold(position: float) -> bytes:
        """A position command with a watchdog that does not run out inside a test."""
        return p.position_command(position, 0.0, 0.0, 1.0, 1.0, math.nan, 5.0)

    c = SimulatedController(1, position_min=-0.25, position_max=0.25)
    seen_modes = []
    ask(c, hold(0.1) + p.query())
    seen_modes.append(c.mode)
    for _ in range(4):
        c.step()
        seen_modes.append(c.mode)
    check(seen_modes == [p.MODE_ENABLING, p.MODE_CALIBRATING, p.MODE_CALIBRATION_COMPLETE, p.MODE_POSITION, p.MODE_POSITION],
          "the first command out of stopped goes through enabling, calibrating and calibration complete before it is taken: three cycles with the driver not yet on")
    c.step(200)
    check(near(c.position, 0.1, 2e-3) and c.mode == p.MODE_POSITION, "…and then the shaft goes to the position it was told")
    ask(c, p.stop_command(p.MODE_ZERO_VELOCITY))
    check(c.mode == p.MODE_ZERO_VELOCITY, "a stop to zero velocity is taken at once")
    ask(c, hold(0.2))
    check(c.mode == p.MODE_POSITION, "…and a position command from it is taken at once too, with no interval of preparing: the driver was never off")
    ask(c, p.stop_command(p.MODE_BRAKE))
    check(c.mode == p.MODE_BRAKE, "a stop to brake is taken at once")
    ask(c, hold(0.2))
    check(c.mode == p.MODE_POSITION, "…and so is the command from it")
    ask(c, p.stop_command(p.MODE_STOPPED))
    ask(c, hold(0.2))
    check(c.mode == p.MODE_ENABLING, "…but from stopped it goes back through preparing, with the driver off for it")

    c = SimulatedController(1, position_min=-0.25, position_max=0.25)
    ask(c, p.position_command(0.0, 0.0, 0.0, 1.0, 1.0, math.nan, None))
    c.step(10)
    c.inject_fault(33)
    ask(c, p.position_command(0.1, 0.0, 0.0, 1.0, 1.0, math.nan, None))
    check(c.mode == p.MODE_FAULT and c.fault == 33, "a fault ignores every command that is not a stop")
    check(registers(ask(c, p.query()))[15] == 33, "…and the fault register names it")
    ask(c, p.stop_command(p.MODE_ZERO_VELOCITY))
    check(c.mode == p.MODE_FAULT, "…even a stop to zero velocity")
    ask(c, p.stop_command(p.MODE_STOPPED))
    check(c.mode == p.MODE_STOPPED and c.fault == 0, "stopped clears it: the only way out")

    c = SimulatedController(1, position_min=-0.25, position_max=0.25)
    ask(c, p.position_command(0.0, 0.0, 0.0, 1.0, 1.0, math.nan, 0.06))
    c.step(10)  # three preparing cycles, then position; the watchdog is 30 cycles and counts from the command
    check(c.mode == p.MODE_POSITION, "a command is held for its watchdog")
    c.step(30)
    check(c.mode == p.MODE_TIMEOUT, "…and when it runs out the controller falls back to timeout")
    ask(c, p.position_command(0.0, 0.0, 0.0, 1.0, 1.0, math.nan, None))
    check(c.mode == p.MODE_TIMEOUT, "…which no command leaves")
    ask(c, p.stop_command(p.MODE_STOPPED))
    check(c.mode == p.MODE_STOPPED, "…only a stop")
    c = SimulatedController(1)
    ask(c, p.position_command(0.0, 0.0, 0.0, 1.0, 1.0, math.nan, math.nan))
    c.step(2000)
    check(c.mode == p.MODE_POSITION, "a watchdog of NaN is no timeout, which is why this driver never writes one")
    c = SimulatedController(1, default_timeout_s=0.06)
    ask(c, p.position_command(0.0, 0.0, 0.0, 1.0, 1.0, math.nan, None))
    c.step(20)
    check(c.mode == p.MODE_POSITION, "an unwritten watchdog is the controller's configured default (here 60 ms, which has not run out)")
    c.step(30)
    check(c.mode == p.MODE_TIMEOUT, "…and it does run out")

    c = SimulatedController(1, position_min=-0.25, position_max=0.25)
    c.position = 0.5
    ask(c, p.position_command(0.0, 0.0, 0.0, 1.0, 1.0, math.nan, None))
    c.step(10)
    check(c.mode == p.MODE_FAULT and c.fault == 39, "position control started outside the bounds faults as outside limit (39), and does not move")
    ask(c, p.stop_command(p.MODE_STOPPED))
    c.position = 0.1
    ask(c, p.position_command(0.0, 0.0, 0.0, 1.0, 1.0, math.nan, None))
    c.step(5)
    check(c.mode == p.MODE_POSITION, "…while the same command from inside them is taken")

    c = SimulatedController(1, position_min=-0.25, position_max=0.25)
    ask(c, hold(0.0))
    c.step(10)
    ask(c, p.write_registers(0x20, p.F32, [0.1]))
    ask(c, p.write_registers(p.MODE, p.INT8, [p.MODE_POSITION]) + p.write_registers(p.COMMAND_WATCHDOG, p.F32, [5.0]))
    c.step(100)
    check(near(c.position, 0.0, 2e-3), "register writes in a frame that does not open with the mode are not a command (the mode goes first), and the next command's unwritten position is 0.0")
    ask(c, hold(0.4))
    c.step(400)
    check(near(c.position, 0.25, 2e-3), "a position past the bounds is clamped to them by the controller itself: the declared travel is what it enforces")

    c = SimulatedController(1)
    ask(c, p.position_command(math.nan, 0.5, 0.0, 1.0, 1.0, math.nan, 5.0))
    c.step(10)
    ask(c, p.position_command(math.nan, 0.5, 0.0, 1.0, 1.0, math.nan, 5.0))
    before = c.position
    for _ in range(100):
        ask(c, p.position_command(math.nan, 0.5, 0.0, 1.0, 1.0, math.nan, 5.0))
        c.step(1)
    check(c.position > before + 0.05, "a velocity with no position is the controller advancing its own target at that speed")
    c = SimulatedController(1)
    for _ in range(200):
        ask(c, p.position_command(math.nan, 0.0, 0.5, 0.0, 0.0, math.nan, 5.0))
        c.step(1)
    check(c.velocity > 0.1 and c.position > 0.01 and near(c.torque, 0.5, 1e-9), "torque is both gains at zero and the feedforward: it accelerates, and the torque it reports is the one it was told")

    c = SimulatedController(1, position_min=-0.25, position_max=0.25)
    c.config["servopos.position_min"] = -0.25
    diag = ask(c, p.diagnostic_write(1, b"conf get servopos.position_min\n") + p.diagnostic_poll(1, 48))
    check(p.StreamData("server", 1, b"-0.25\r\n") in diag, "`conf get` over the diagnostic channel answers one line, the value")
    diag = ask(c, p.diagnostic_write(1, b"conf get servopos.nothing\n") + p.diagnostic_poll(1, 48))
    check(any(isinstance(i, p.StreamData) and i.data.startswith(b"ERR") for i in diag), "…and `ERR` for a name it does not hold")
    c = SimulatedController(1)
    diag = ask(c, p.diagnostic_write(1, b"conf get servopos.position_max\n") + p.diagnostic_poll(1, 48))
    check(p.StreamData("server", 1, b"nan\r\n") in diag, "…and `nan` for a bound that is not set")
    c.refuse_diagnostic = True
    check(not any(isinstance(i, p.StreamData) for i in ask(c, p.diagnostic_write(1, b"conf get servopos.position_max\n") + p.diagnostic_poll(1, 48))), "a controller without the channel says nothing at all")

    # ── open ─────────────────────────────────────────────────────────────────
    section("open — a body that does not come up says why")
    bridge, sim, master, loop = build()
    check(bridge.open("not json") == rhal.SKR_EINVAL and bridge.open("[1, 2]") == rhal.SKR_EINVAL, "a configuration that is not JSON, or is JSON but not an object, is refused")
    for bad in (None, "stop", "release", 0, True, "ZERO_VELOCITY"):
        bridge, sim, master, loop = build()
        code, text = said(lambda: bridge.open(config(stop_mode=bad) if bad is not None else json.dumps({"joints": settings()["joints"]})))
        check(code == rhal.SKR_ENODEV and not master.sent and "stop_mode" in text and "no default" in text,
              f"`stop_mode` of {bad!r} is refused with nothing sent, and the refusal says it has no default")
    for good in ("zero_velocity", "brake", "stopped"):
        bridge, sim, master, loop = build()
        check(bridge.open(config(stop_mode=good)) == rhal.SKR_OK, f"…while `stop_mode` {good!r} is taken")
    bridge, sim, master, loop = build()
    check(bridge.open(json.dumps({"stop_mode": "brake"})) == rhal.SKR_ENODEV and not master.sent, "a body with no joints is refused: nothing on the bus says which joint of which body a controller is")
    for bad in (0, 128, -1, True, "1", 1.5, None):
        bridge, sim, master, loop = build()
        check(bridge.open(json.dumps({"joints": [{"id": bad, "name": "J"}], "stop_mode": "brake"})) == rhal.SKR_ENODEV and not master.sent, f"a controller id of {bad!r} is refused with nothing sent")
    for bad_joints, why in (([{"id": 1}], "no name"), ([{"id": 1, "name": "J"}, {"id": 1, "name": "K"}], "a controller named by two joints"),
                            ([{"id": 1, "name": "J"}, {"id": 2, "name": "J"}], "two joints with one name")):
        bridge, sim, master, loop = build()
        check(bridge.open(json.dumps({"joints": bad_joints, "stop_mode": "brake"})) == rhal.SKR_ENODEV and not master.sent, f"{why} is refused with nothing sent")
    for bad_role, reason in (("nonsense", "a role outside §5.1's grammar"), ("hovercraft.hull", "a chain §5.1 does not define"), ("right_arm.shoulder.twist", "an axis §5.1 does not define")):
        bridge, sim, master, loop = build()
        check(bridge.open(json.dumps({"joints": [{"id": 1, "name": "J", "role": bad_role}], "stop_mode": "brake"})) == rhal.SKR_ENODEV and not master.sent, f"{reason} (`{bad_role}`) is refused")
    bridge, sim, master, loop = build()
    check(bridge.open(json.dumps({"joints": [{"id": 1, "name": "J", "role": " Right_Arm.Shoulder.Pitch "}], "stop_mode": "brake"})) == rhal.SKR_OK
          and json.loads(bridge.describe())["joints"][0]["role"] == "right_arm.shoulder.pitch", "…while a role is normalised and declared")
    for key, bad in (("window_ms", 0), ("window_ms", 1001), ("window_ms", True), ("watchdog_ms", 0.5), ("watchdog_ms", 10001), ("watchdog_ms", "100"), ("max_torque_nm", 0), ("max_torque_nm", -1),
                     ("max_torque_nm", math.inf), ("source", 128), ("source", -1), ("source", True)):
        bridge, sim, master, loop = build()
        bad_text = json.dumps(settings(**{key: bad}), allow_nan=True)
        check(bridge.open(bad_text) == rhal.SKR_ENODEV and not master.sent, f"`{key}` of {bad!r} is refused with nothing sent")
    for bad in ([1], [0.5, 0.5], [1.0, -1.0], "x", [math.nan, 1.0], [True, 2]):
        bridge, sim, master, loop = build()
        check(bridge.open(json.dumps({"joints": [{"id": 1, "name": "J", "limits": bad}], "stop_mode": "brake"}, allow_nan=True)) == rhal.SKR_ENODEV and not master.sent,
              f"`limits` of {bad!r} is refused with nothing sent")

    bridge, sim, master, loop = build()
    sim.controllers[2].mute = True
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "controller 2" in text and "did not answer" in text, "a controller that never answers is refused by number: an id with nothing behind it and one that is off look the same from here")
    bridge, sim, master, loop = build()
    sim.powered = False
    check(bridge.open(config()) == rhal.SKR_ENODEV, "…and so is a bus with nothing on it")
    bridge, sim, master, loop = build(ids=(1,))
    check(bridge.open(config(ids=(1, 2))) == rhal.SKR_ENODEV, "a joint whose controller is not on the bus is refused")
    for code_value in sorted(p.FAULTS):
        bridge, sim, master, loop = build()
        sim.controllers[1].inject_fault(code_value)
        code, text = said(lambda: bridge.open(config()))
        check(code == rhal.SKR_ENODEV and p.FAULTS[code_value] in text and not [f for f in master.sent if p.parse_frame(f.data)[0].kind == "write"],
              f"fault {code_value} ({p.FAULTS[code_value]}) is refused at open by name, before anything is written")
    bridge, sim, master, loop = build()
    sim.controllers[1].mode = p.MODE_TIMEOUT
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "timeout" in text and "stop it deliberately" in text and not master.commands(1), "a controller in timeout is refused, naming the stop only a person should write")
    for mode in (p.MODE_PWM, p.MODE_VOLTAGE, p.MODE_VOLTAGE_FOC, p.MODE_VOLTAGE_DQ, p.MODE_CURRENT, p.MODE_STAY_WITHIN, p.MODE_MEASURE_INDUCTANCE, p.MODE_ENABLING, p.MODE_CALIBRATING):
        bridge, sim, master, loop = build()
        sim.controllers[1].mode = mode
        code, text = said(lambda: bridge.open(config()))
        check(code == rhal.SKR_ENODEV and "another master's mode" in text and not master.commands(1), f"a controller in {p.MODES[mode]} ({mode}) is another master's, and is refused")
    for mode in (p.MODE_STOPPED, p.MODE_POSITION, p.MODE_ZERO_VELOCITY, p.MODE_BRAKE):
        bridge, sim, master, loop = build()
        sim.controllers[1].mode = mode
        check(bridge.open(config()) == rhal.SKR_OK, f"…while one that is {p.MODES[mode]} is ours to take")

    bridge, sim, master, loop = build()
    sim.controllers[1].position = 0.4
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "outside the travel" in text and "fault 39" in text and not master.commands(1), "an axis found outside its own bounds is refused before it is told to hold: the controller would fault on it")
    bridge, sim, master, loop = build(position_min=math.nan, position_max=math.nan)
    sim.controllers[1].position = 3.0
    check(bridge.open(config()) == rhal.SKR_OK, "…and one with no bounds has none to be outside of")
    bridge, sim, master, loop = build(clock=SteppingClock())
    for controller in sim.controllers.values():
        controller.refuse = {p.MODE}
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "told to hold and reports `stopped`" in text, "an axis that never takes the hold is refused with the mode it reports: read back, never assumed")
    bridge, sim, master, loop = build(ids=(1,), clock=SteppingClock())
    sim.controllers[1].refuse = {p.COMMAND_POSITION}
    code, text = said(lambda: bridge.open(config(ids=(1,))))
    check(code == rhal.SKR_ENODEV and "told to hold and reports" in text, "…and one that answers the hold with an error on a register of it is refused too: a status carrying an error is not a status, and the mode is never read from it")

    # the travel
    bridge, sim, master, loop = opened()
    descriptor = json.loads(bridge.describe())
    check(all(near(j["limits"]["position_min_rad"], -0.25 * TWO_PI, 1e-6) and near(j["limits"]["position_max_rad"], 0.25 * TWO_PI, 1e-6) for j in descriptor["joints"]) and all(j["type"] == "revolute" for j in descriptor["joints"]),
          "the declared travel is the controller's own, in radians: servopos.position_min and position_max are -0.25 and 0.25 revolutions")
    check(any(f.data.startswith(p.diagnostic_write(1, b"conf get servopos.position_min\n")) for f in master.to(1))
          and any(f.data.startswith(p.diagnostic_write(1, b"conf get servopos.position_max\n")) for f in master.to(1)), "…read over the diagnostic channel, by the maker's own names")
    bridge, sim, master, loop = build(position_min=-0.5, position_max=0.5)
    sim.controllers[1].position = 0.0
    assert bridge.open(json.dumps({"joints": [{"id": 1, "name": "J1", "limits": [-0.5, 0.5]}, {"id": 2, "name": "J2"}], "stop_mode": "brake"})) == rhal.SKR_OK
    descriptor = json.loads(bridge.describe())
    check(near(descriptor["joints"][0]["limits"]["position_max_rad"], 0.5, 1e-9) and near(descriptor["joints"][1]["limits"]["position_max_rad"], 0.5 * TWO_PI, 1e-9),
          "an operator's `limits` narrow the controller's travel where they are narrower")
    bridge, sim, master, loop = build(position_min=-0.1, position_max=0.1)
    assert bridge.open(json.dumps({"joints": [{"id": 1, "name": "J1", "limits": [-3.0, 3.0]}], "stop_mode": "brake"})) == rhal.SKR_OK
    check(near(json.loads(bridge.describe())["joints"][0]["limits"]["position_max_rad"], 0.1 * TWO_PI, 1e-9), "…and never widen them: the narrower of the two on each side (F181)")
    bridge, sim, master, loop = build(position_min=-0.1, position_max=0.1)
    code, text = said(lambda: bridge.open(json.dumps({"joints": [{"id": 1, "name": "J1", "limits": [1.0, 2.0]}], "stop_mode": "brake"})))
    check(code == rhal.SKR_ENODEV and "do not overlap" in text, "an operator's limits that do not meet the controller's are refused as that")
    bridge, sim, master, loop = build(position_min=math.nan, position_max=math.nan)
    check(bridge.open(config()) == rhal.SKR_OK and all(j["type"] == "continuous" and j["limits"] == {} for j in json.loads(bridge.describe())["joints"]),
          "a controller with no bounds at all is a continuous joint with no limits: nothing declares a travel that is not there")
    bridge, sim, master, loop = build(position_min=-0.25, position_max=math.nan)
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "one side only" in text, "a controller that bounds one side only is refused, naming what to give")
    bridge, sim, master, loop = build(position_min=-0.25, position_max=math.nan)
    assert bridge.open(json.dumps({"joints": [{"id": 1, "name": "J1", "limits": [-3.0, 1.0]}, {"id": 2, "name": "J2", "limits": [-3.0, 1.0]}], "stop_mode": "brake"})) == rhal.SKR_OK
    check(near(json.loads(bridge.describe())["joints"][0]["limits"]["position_min_rad"], -0.25 * TWO_PI, 1e-9) and near(json.loads(bridge.describe())["joints"][0]["limits"]["position_max_rad"], 1.0, 1e-9),
          "…while an operator closing the open side leaves the controller's closed side as it is where that is the narrower")
    bridge, sim, master, loop = build(position_min=0.1, position_max=-0.1)
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "empty or reversed" in text, "a reversed range a controller reports is refused as that, not guessed at")
    bridge, sim, master, loop = build()
    for controller in sim.controllers.values():
        controller.refuse_diagnostic = True
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "diagnostic channel" in text and "no `limits` were given" in text, "controllers that do not answer the diagnostic channel are refused, naming `limits` as the way through")
    bridge, sim, master, loop = build()
    for controller in sim.controllers.values():
        controller.refuse_diagnostic = True
    check(bridge.open(json.dumps({"joints": [{"id": 1, "name": "J1", "limits": [-1.0, 1.0]}, {"id": 2, "name": "J2", "limits": [-2.0, 2.0]}], "stop_mode": "brake"})) == rhal.SKR_OK
          and near(json.loads(bridge.describe())["joints"][1]["limits"]["position_max_rad"], 2.0, 1e-9), "…and `limits` are taken where the controller will not say")

    # backlog
    bridge, sim, master, loop = build()
    stale = FdFrame(0x0100, p.pad(p.reply_registers(0, p.INT8, [1]) + p.reply_registers(1, p.F32, [0.0, 0.0, 0.0]) + p.reply_registers(0x0C, p.INT8, [2, 48, 30, 33])))
    loop.endpoint().send(stale)
    check(bridge.open(config()) == rhal.SKR_OK, "a fault an earlier session saw, still waiting on the bus when this one opens, is discarded: what is waiting answers a session that is over")

    bridge, sim, master, loop = opened()
    check(bridge.open(config()) == rhal.SKR_OK, "a session can be opened again over the same bus")
    bridge2, sim2, master2, loop2 = opened(ids=(1,))
    check(master2.sent and all(frame.extended == (frame.arbitration_id > 0x7FF) for frame in master2.sent) and any(frame.extended for frame in master2.sent) and not all(frame.extended for frame in master2.sent),
          "a frame is sent extended exactly when its identifier does not fit eleven bits: those that ask for a reply (0x8001) are, those that do not (0x0001) are not")
    check(all(len(frame.data) in p.FD_LENGTHS for frame in master2.sent) and all((frame.arbitration_id & 0x7F) == 1 for frame in master2.sent),
          "…every payload this driver sends is a length CAN-FD carries, addressed to the controller it speaks to")
    check(all(((frame.arbitration_id >> 8) & 0x7F) == 0 for frame in master2.sent), "…from sender 0 unless it is told another")
    bridge2, sim2, master2, loop2 = opened(ids=(1,), source=5)
    check(master2.sent and all(((frame.arbitration_id >> 8) & 0x7F) == 5 for frame in master2.sent) and read(bridge2)[0] == rhal.SKR_OK,
          "a configured sender id is the one on every frame, and the replies to it are the ones read")
    bridge2, sim2, master2, loop2 = opened(ids=(1,))
    check(bridge2.unexpected_frames == 0, "a body that opens and reads counts no frame as not its own")

    # ── the descriptor ───────────────────────────────────────────────────────
    section("the descriptor — what the body says it is")
    bridge, sim, master, loop = opened()
    descriptor = json.loads(bridge.describe())
    check(descriptor["robot_slug"] == "mjbots-moteus-axes" and descriptor["dof"] == 2 and descriptor["base"] == "fixed", "a fixed body with one joint per controller")
    check([j["name"] for j in descriptor["joints"]] == ["J1", "J2"] and all(j["role"] is None for j in descriptor["joints"]), "named as the operator named them, with no role the operator did not give (§5.1: roles are never inferred)")
    check(descriptor["control"]["modes"] == ["position", "velocity", "torque"] and descriptor["control"]["per_joint_modes"] is True and all(j["control_modes"] == ["position", "velocity", "torque"] for j in descriptor["joints"]),
          "position, velocity and torque, per joint")
    check(descriptor["envelope"] is None and descriptor["capabilities"] == ["safety.estop"] and descriptor["simulator"] is True,
          "no envelope: nothing published here is a rated one; the software stop it has is declared, and no rated stop; declared simulated (§35.3)")
    bridge, sim, master, loop = build()
    assert bridge.open(config(simulator=False)) == rhal.SKR_OK
    check(json.loads(bridge.describe())["simulator"] is True, "a loopback bus is a simulated body whatever the configuration says: there is demonstrably no controller on it")

    # ── read_state ───────────────────────────────────────────────────────────
    section("read_state — what the body is, and what it will not invent")
    bridge, sim, master, loop = opened()
    sim.controllers[1].position, sim.controllers[1].velocity, sim.controllers[1].torque = 0.1, 0.2, 0.3
    sim.controllers[2].position, sim.controllers[2].velocity, sim.controllers[2].torque = -0.05, -0.1, -0.4
    sim.controllers[1].mode = p.MODE_POSITION
    code, state = read(bridge)
    check(code == rhal.SKR_OK and len(state.position_rad) == 2, "a read answers, one position a joint")
    check(near(state.position_rad[0], 0.1 * TWO_PI, 5e-3) and near(state.position_rad[1], -0.05 * TWO_PI, 5e-3), "positions are revolutions of the output shaft, in radians")
    check(near(state.velocity_rad_s[0], 0.2 * TWO_PI, 5e-2) and near(state.velocity_rad_s[1], -0.1 * TWO_PI, 5e-2), "velocities are hertz at the output shaft, in radians a second")
    check(not any(math.isnan(e) for e in state.effort_nm) and near(state.effort_nm[0], 0.3, 0.3) and state.effort_nm[1] < 0.0,
          "effort is the controller's torque at the output shaft in newton metres — a real number, where a driver whose wire reports amps has none (§24)")
    check(state.fault_code == 0 and state.flags == rhal.HOMING_COMPLETE, "a healthy body reads no flag, no fault, and homed: the controllers report their position referenced to the output")
    first_seq, first_stamp = state.seq, state.stamp_ns
    code, state = read(bridge)
    check(state.seq == first_seq + 1 and state.stamp_ns >= first_stamp, "the sequence advances by one a read and the stamp never goes back")
    for home, expected in ((0, rhal.HOMING_PENDING), (1, rhal.HOMING_PENDING), (2, rhal.HOMING_COMPLETE)):
        bridge, sim, master, loop = opened()
        sim.controllers[2].home_state = home
        code, state = read(bridge)
        check(state.flags & rhal.HOMING_MASK == expected, f"home state {home} ({'relative only' if home == 0 else 'rotor' if home == 1 else 'output'}) reads as homing {rhal.HOMING_NAMES[expected]}: positions that are not referenced to the output are not trustworthy")

    section("faults — refused, never cleared, and the word is the maker's")
    for code_value in sorted(p.FAULTS):
        bridge, sim, master, loop = opened()
        sim.controllers[2].inject_fault(code_value)
        code, state = read(bridge)
        check(code == rhal.SKR_OK and state.flags & rhal.FLAG_FAULT and state.flags & rhal.FLAG_DEGRADED and state.fault_code == (2 << 16) | (p.MODE_FAULT << 8) | code_value,
              f"fault {code_value} ({p.FAULTS[code_value]}) reads FAULT and DEGRADED with joint 2, mode 1 and the code in the word")
    bridge, sim, master, loop = opened()
    sim.controllers[2].inject_fault(33)
    read(bridge)
    master.clear()
    code, text = said(lambda: bridge.write_command(command(GOOD), 0))
    check(code == rhal.SKR_EPERM and not master.sent and "J2" in text and "motor driver fault" in text, "…and a command is refused whole, with nothing sent — not even to the healthy joint — and the refusal names the fault")
    check(not any(item.register == p.MODE and item.value == p.MODE_STOPPED for frame in master.sent for item in p.parse_frame(frame.data) if isinstance(item, p.Register) and item.kind == "write"),
          "…and clears nothing: the controller's clear is the person's")
    for limit in sorted(p.LIMITING):
        bridge, sim, master, loop = opened()
        sim.controllers[1].limiting = limit
        code, state = read(bridge)
        check(state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT and not state.flags & rhal.FLAG_ESTOP and state.fault_code == (1 << 16) | (p.MODE_POSITION << 8) | limit
              and bridge.write_command(command(GOOD), 0) == rhal.SKR_OK,
              f"code {limit} ({p.LIMITING[limit]}) in position mode reads DEGRADED alone, with the code in the word for the black box, and the command is taken: a limit is not a fault")
    bridge, sim, master, loop = opened()
    sim.controllers[1].limiting = 77
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT and state.fault_code == (1 << 16) | (p.MODE_POSITION << 8) | 77,
          "a non-zero code in position mode that is in no table is never read as healthy, and not read as a fault either, and is carried as it is")
    bridge, sim, master, loop = opened()
    sim.controllers[1].limiting = 99
    sim.controllers[2].inject_fault(34)
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_FAULT and state.fault_code == (2 << 16) | (p.MODE_FAULT << 8) | 34, "…and a fault on a later joint takes the word from a limit on an earlier one: the word names the worse of the two")
    bridge, sim, master, loop = opened()
    sim.controllers[1].step(100)  # the default watchdog (100 ms) runs out with nobody commanding
    code, state = read(bridge)
    check(sim.controllers[1].mode == p.MODE_TIMEOUT and state.flags & rhal.FLAG_FAULT and state.flags & rhal.FLAG_DEGRADED and state.fault_code == (1 << 16) | (p.MODE_TIMEOUT << 8),
          "a controller that fell back on its watchdog reads FAULT, with mode 11 in the word and no fault code: a driver that stops commanding is told so")
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM, "…and commands are refused: only a stop leaves timeout, and a stop is the person's")
    for mode in (p.MODE_STOPPED, p.MODE_ENABLING, p.MODE_CALIBRATING, p.MODE_CALIBRATION_COMPLETE, p.MODE_PWM, p.MODE_VOLTAGE, p.MODE_CURRENT, p.MODE_ZERO_VELOCITY, p.MODE_STAY_WITHIN, p.MODE_BRAKE):
        bridge, sim, master, loop = opened()
        sim.controllers[1].mode = mode
        master.clear()
        code, state = read(bridge)
        refusal, text = said(lambda: bridge.write_command(command(GOOD), 0))
        check(state.flags & rhal.FLAG_FAULT and state.fault_code >> 16 == 1 and refusal == rhal.SKR_EPERM and not master.commands(1) and not master.commands(2),
              f"an axis found in {p.MODES[mode]} ({mode}) in a session is somebody else's doing: FAULT, commands refused, and no frame sent to bring it back")
    bridge, sim, master, loop = opened()
    sim.controllers[1].mute = True
    code, state = read(bridge)
    check(code == rhal.SKR_OK and state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT, "one controller silent for a cycle reads DEGRADED: its reading is a period old under a fresh stamp")
    for controller in sim.controllers.values():
        controller.mute = True
    check(read(bridge)[0] == rhal.SKR_ETIMEDOUT, "…and all of them silent is ETIMEDOUT, not the last positions read as fresh")
    clock = ManualClock()
    bridge, sim, master, loop = opened(clock=clock)
    read(bridge)
    sim.controllers[2].mute = True
    clock.advance(2.0)
    code, state = read(bridge)
    check(code == rhal.SKR_OK and state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT, "a status not heard for a second reads DEGRADED: what a controller last said is not health")
    sim.controllers[2].mute = False
    code, state = read(bridge)
    check(not state.flags & rhal.FLAG_DEGRADED, "…and reads healthy again when it is heard")
    bridge, sim, master, loop = opened()
    before = bridge.unexpected_frames
    loop.endpoint().send(FdFrame(0x0123, b"\x00"))
    loop.endpoint().send(FdFrame(0x0100, p.pad(b"\x21\x00\x0a")))  # from controller 1 to sender 0, but a status that is not one
    code, state = read(bridge)
    check(bridge.unexpected_frames >= before + 2 and code == rhal.SKR_OK, "traffic that is not a controller's status is counted and not applied, and the read after it is unaffected")

    # ── write_command ────────────────────────────────────────────────────────
    section("write_command — a setpoint the controller takes, or nothing at all")
    bridge, sim, master, loop = opened()
    master.clear()
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_OK, "a position command on both joints is accepted")
    sent = master.commands(1)
    check(len(sent) == 1 and len(master.commands(2)) == 1, "…one frame to each controller, each opening with the mode")
    values = [i for i in sent[0] if isinstance(i, p.Register) and i.kind == "write"]
    by_register = {i.register: i.value for i in values}
    check(by_register[0] == p.MODE_POSITION and near(by_register[p.COMMAND_POSITION], GOOD[0] / TWO_PI, 1e-6), "…position mode, with the setpoint in revolutions")
    check(by_register[p.COMMAND_VELOCITY] == 0.0 and by_register[p.COMMAND_FEEDFORWARD_TORQUE] == 0.0 and by_register[p.COMMAND_KP_SCALE] == 1.0 and by_register[p.COMMAND_KD_SCALE] == 1.0,
          "…with no velocity, no feedforward and both gains at one: the controller's own control law")
    check(math.isnan(by_register[p.COMMAND_MAX_TORQUE]) and p.COMMAND_WATCHDOG not in by_register, "…and no torque ceiling and no watchdog of its own: the controller's configured ones")
    state = drive(bridge, GOOD)
    check(near(state.position_rad[0], GOOD[0], 0.02) and near(state.position_rad[1], GOOD[1], 0.02), "the controllers get there, and a read says so")

    bridge, sim, master, loop = opened(watchdog_ms=250, max_torque_nm=1.5)
    master.clear()
    bridge.write_command(command(GOOD), 0)
    by_register = {i.register: i.value for i in master.commands(2)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(near(by_register[p.COMMAND_WATCHDOG], 0.25, 1e-6) and near(by_register[p.COMMAND_MAX_TORQUE], 1.5, 1e-6), "a configured watchdog and torque ceiling are written with every command, to every controller")
    sim.controllers[1].step(60)
    check(sim.controllers[1].mode == p.MODE_POSITION, "…and the watchdog is the one that was written, not the controller's (a 250 ms watchdog has not run out after 120 ms)")

    for count in (1, 3, 0):
        bridge, sim, master, loop = opened()
        master.clear()
        check(bridge.write_command(command([0.1] * count), 0) == rhal.SKR_EINVAL and not master.sent, f"a command for {count} joints is refused with nothing sent")
    bridge, sim, master, loop = opened()
    master.clear()
    check(bridge.write_command(command(GOOD, modes=[0x13, 0]), 0) == rhal.SKR_EINVAL and not master.sent, "a per-joint mode the vocabulary does not define is refused with nothing sent")
    check(bridge.write_command(command(GOOD, modes=[rhal.MODE_TORQUE, 0]), 0) == rhal.SKR_EINVAL and not master.sent, "a bare MODE_* written where an offset per-joint mode belongs is refused: the transcription error §86.1's offset exists to catch")
    for mask in ([1, 1], [0, 0], [1, 0]):
        check(bridge.write_command(command(GOOD, mode=200, mask=mask), 0) == rhal.SKR_EINVAL and not master.sent, f"a mode the ABI does not define is malformed whatever the mask says ({sum(mask)} joints applied)")
    check(bridge.write_command(command(GOOD, modes=[rhal.JMODE_INHERIT, rhal.JMODE_POSITION]), 0) == rhal.SKR_OK, "…while per-joint modes naming position, or inheriting it, are accepted")

    for joint in range(2):
        for bad in (math.nan, math.inf, -math.inf):
            bridge, sim, master, loop = opened()
            values = list(GOOD)
            values[joint] = bad
            master.clear()
            code, text = said(lambda: bridge.write_command(command(values), 0))
            check(code == rhal.SKR_EINVAL and not master.sent and f"the setpoint for J{joint + 1} is not a finite number" in text,
                  f"a {bad!r} setpoint on J{joint + 1} is refused with nothing sent, and the refusal names the joint (F282's shape; a NaN position would mean 'hold where you are' to a moteus)")
    bridge, sim, master, loop = opened()
    masked_nan = [math.nan, 0.1]
    check(bridge.write_command(command(masked_nan, mask=[0, 1]), 0) == rhal.SKR_OK, "a NaN in the entry of a joint the mask holds is not read, so the command is accepted")
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_OK, "…and the next ordinary command is too: the refusals above stored nothing")

    # travel
    limit = 0.25 * TWO_PI
    for joint in range(2):
        for label, value, accepted in (("below", -limit - 1e-3, False), ("above", limit + 1e-3, False), ("at the lower bound", -limit, True), ("at the upper bound", limit, True)):
            bridge, sim, master, loop = opened()
            values = [0.0, 0.0]
            values[joint] = value
            master.clear()
            result, text = said(lambda: bridge.write_command(command(values), 0))
            check((result == rhal.SKR_OK) if accepted else (result == rhal.SKR_EINVAL and not master.sent and "outside its travel" in text),
                  f"J{joint + 1} {label} its travel is {'accepted' if accepted else 'refused whole, with nothing sent, and not clamped'}")
    bridge, sim, master, loop = opened()
    check(bridge.write_command(command([limit + 1.0, 0.0], mask=[0, 1]), 0) == rhal.SKR_OK, "a number outside the travel in the entry of a joint the mask holds is never sent, so it is not held to the travel")
    check(bridge.write_command(command([limit + 1.0, 0.0], modes=[rhal.JMODE_VELOCITY, rhal.JMODE_POSITION]), 0) == rhal.SKR_OK, "…and a velocity is not a position: the travel bounds a setpoint of position, and the controller's own bounds stop the shaft")
    bridge, sim, master, loop = build(position_min=math.nan, position_max=math.nan)
    assert bridge.open(config()) == rhal.SKR_OK
    check(bridge.write_command(command([50.0, -50.0]), 0) == rhal.SKR_OK, "a continuous joint has no travel to be outside of")

    # velocity and torque
    bridge, sim, master, loop = opened()
    master.clear()
    check(bridge.write_command(command([1.0, 0.4], modes=[rhal.JMODE_VELOCITY, rhal.JMODE_TORQUE]), 0) == rhal.SKR_OK, "a mixed command, velocity on one joint and torque on the other, is one frame each")
    v = {i.register: i.value for i in master.commands(1)[0] if isinstance(i, p.Register) and i.kind == "write"}
    t = {i.register: i.value for i in master.commands(2)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(v[0] == p.MODE_POSITION and math.isnan(v[p.COMMAND_POSITION]) and near(v[p.COMMAND_VELOCITY], 1.0 / TWO_PI, 1e-6) and v[p.COMMAND_KP_SCALE] == 1.0 and v[p.COMMAND_KD_SCALE] == 1.0,
          "a velocity is position mode with no position (NaN: the controller advances its own target) and the gains kept, in revolutions a second")
    check(t[0] == p.MODE_POSITION and math.isnan(t[p.COMMAND_POSITION]) and t[p.COMMAND_KP_SCALE] == 0.0 and t[p.COMMAND_KD_SCALE] == 0.0 and near(t[p.COMMAND_FEEDFORWARD_TORQUE], 0.4, 1e-6),
          "…and a torque is the same mode with both gains at zero and the torque as the feedforward, in newton metres")
    for _ in range(30):
        bridge.write_command(command([1.0, 0.4], modes=[rhal.JMODE_VELOCITY, rhal.JMODE_TORQUE]), 0)
        code, state = read(bridge)
    check(state.position_rad[0] > 0.02 and state.velocity_rad_s[1] > 1.0 and near(state.effort_nm[1], 0.4, 1e-6) and not state.flags & rhal.FLAG_FAULT,
          "…and the controllers do what they were told: one turns forward, the other accelerates under the torque it was given and reports it")

    # the mask is a hold
    bridge, sim, master, loop = opened()
    settle(bridge, 3)
    drive(bridge, GOOD, cycles=60)
    master.clear()
    check(bridge.write_command(command([0.2, 0.0], mask=[1, 0]), 0) == rhal.SKR_OK, "a command that masks a joint is accepted")
    held = [i for i in master.commands(2)[0] if isinstance(i, p.Register) and i.kind == "write"]
    held_values = {i.register: i.value for i in held}
    check(len(master.commands(2)) == 1 and near(held_values[p.COMMAND_POSITION], GOOD[1] / TWO_PI, 1e-6) and held_values[0] == p.MODE_POSITION,
          "…and a masked joint is still commanded, every cycle: a position command at the setpoint it was last given (§24: 'current setpoint'), because a joint that is skipped runs its watchdog down")
    master.clear()
    assert bridge.write_command(command([0.0, 0.0], mask=[0, 0]), 0) == rhal.SKR_OK
    c1 = {i.register: i.value for i in master.commands(1)[0] if isinstance(i, p.Register) and i.kind == "write"}
    c2 = {i.register: i.value for i in master.commands(2)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(near(c1[p.COMMAND_POSITION], 0.2 / TWO_PI, 1e-6) and near(c2[p.COMMAND_POSITION], GOOD[1] / TWO_PI, 1e-6), "the kernel's hold frame, every joint masked, holds every joint where it was told to be")
    bridge, sim, master, loop = opened()
    sim.controllers[1].position, sim.controllers[2].position = 0.1, -0.1
    settle(bridge, 3)
    master.clear()
    assert bridge.write_command(command([0.0, 0.0], mask=[0, 0]), 0) == rhal.SKR_OK
    c1 = {i.register: i.value for i in master.commands(1)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(near(c1[p.COMMAND_POSITION], 0.1, 5e-3), "a hold on a joint never commanded latches where it is measured, not at a zero")
    bridge, sim, master, loop = opened()
    settle(bridge, 3)
    drive(bridge, [0.5, 0.0], cycles=3, modes=[rhal.JMODE_VELOCITY, rhal.JMODE_POSITION])
    sim.controllers[1].position = 0.07
    settle(bridge, 3)
    master.clear()
    assert bridge.write_command(command([0.0, 0.0], mask=[0, 1]), 0) == rhal.SKR_OK
    c1 = {i.register: i.value for i in master.commands(1)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(near(c1[p.COMMAND_POSITION], 0.07, 5e-3), "…and so does a joint last commanded in velocity: a velocity is not a position to go back to")
    bridge, sim, master, loop = opened()
    settle(bridge, 3)
    assert bridge.write_command(command(GOOD), 0) == rhal.SKR_OK
    assert bridge.close() == rhal.SKR_OK
    sim.poll()  # what the last session sent is still in the controllers' inbox until their loop runs
    for controller in sim.controllers.values():
        controller.mode = p.MODE_STOPPED  # a person stopped them, so the shaft stays where it is put
        controller.velocity = 0.0
        controller.position = 0.05
    assert bridge.open(config()) == rhal.SKR_OK
    settle(bridge, 3)
    master.clear()
    assert bridge.write_command(command([0.0, 0.0], mask=[0, 0]), 0) == rhal.SKR_OK
    c1 = {i.register: i.value for i in master.commands(1)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(near(c1[p.COMMAND_POSITION], 0.05, 5e-3), "a new session has no setpoint of the last one's: its first hold is where the axis is")

    # ── the stop ─────────────────────────────────────────────────────────────
    section("emergency_stop — the mode the operator named, and a latch of the driver's own")
    for name, mode in (("zero_velocity", p.MODE_ZERO_VELOCITY), ("brake", p.MODE_BRAKE), ("stopped", p.MODE_STOPPED)):
        bridge, sim, master, loop = opened(stop_mode=name)
        settle(bridge, 3)
        master.clear()
        check(bridge.emergency_stop() == rhal.SKR_OK, f"emergency_stop with stop_mode {name!r} returns")
        check(all(len(master.to(i)) == 1 and p.parse_frame(master.to(i)[0].data) == [p.Register("write", 0, p.INT8, mode)] for i in IDS),
              f"…one frame to each controller and nothing else: a write of mode {mode}, no query, no reply asked for")
        code, state = read(bridge)
        check(state.flags & rhal.FLAG_ESTOP and all(c.mode == mode for c in sim.controllers.values()), "the next state says the stop is asserted, and the controllers agree")
        master.clear()
        check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM and not master.sent, "commands are refused while the latch is set, with nothing sent")
        check(not state.flags & rhal.FLAG_FAULT, "…and an axis in the stop it was told to enter is not a fault")
    bridge, sim, master, loop = opened(stop_mode="stopped")
    check(bridge.emergency_stop() == rhal.SKR_OK and bridge.release_stop() == rhal.SKR_ENOSYS, "release_stop is ENOSYS from `stopped`: a release would take the axis back through calibrating with the driver off")
    master.clear()
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM and not master.sent, "…and the latch stays")
    for name in ("zero_velocity", "brake"):
        bridge, sim, master, loop = opened(stop_mode=name)
        settle(bridge, 3)
        drive(bridge, GOOD, cycles=40)
        bridge.emergency_stop()
        read(bridge)
        check(bridge.release_stop() == rhal.SKR_OK, f"release_stop is honest from {name!r}: the firmware switches straight to position, with no interval in which the driver is off")
        master.clear()
        code, state = read(bridge)
        check(state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT and not state.flags & rhal.FLAG_ESTOP, "…the latch is gone, and the axes, still in the stop mode, read DEGRADED and not FAULT until a command takes them back")
        check(bridge.write_command(command(GOOD), 0) == rhal.SKR_OK, "…and the next command is taken")
        code, state = read(bridge)
        check(all(c.mode == p.MODE_POSITION for c in sim.controllers.values()) and not state.flags & rhal.FLAG_DEGRADED, "…the axes are back in position, and the body reads healthy")
    bridge, sim, master, loop = opened(stop_mode="zero_velocity")
    bridge.emergency_stop()
    bridge.release_stop()
    read(bridge)
    sim.controllers[1].inject_fault(33)
    read(bridge)
    check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM, "a released stop does not hide a fault the axis took meanwhile: commands are still refused")
    bridge, sim, master, loop = opened(stop_mode="zero_velocity")
    bridge.emergency_stop()
    read(bridge)  # the controllers take the stop
    bridge.release_stop()
    sim.controllers[1].refuse = {p.MODE}  # this one will not take the command that is meant to bring it back
    assert bridge.write_command(command(GOOD), 0) == rhal.SKR_OK
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_FAULT and sim.controllers[1].mode == p.MODE_ZERO_VELOCITY and sim.controllers[2].mode == p.MODE_POSITION,
          "…and an axis that is still in the stop after a release and a command has not taken the command: FAULT, and not 'waiting' for ever")
    bridge, sim, master, loop = opened()
    bridge.emergency_stop()
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_ESTOP and bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM, "the stop is the driver's own latch: it holds whatever the controllers say")
    bridge3, sim3, master3, loop3 = build()
    check(bridge3.emergency_stop() == rhal.SKR_OK and not master3.sent, "a stop before any session is not an error, and there is nothing to send it to")

    # sessions
    section("sessions — a closed body is closed")
    bridge, sim, master, loop = opened()
    master.clear()
    check(bridge.close() == rhal.SKR_OK and not master.sent, "close() sends nothing: a stopped controller coasts, one put in a stop is no longer held, and silence is the hand-back that leaves the decision to the controller's own configured fallback")
    check(read(bridge)[0] == rhal.SKR_EPERM and bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM, "a read after close is refused, and so is a command")
    check(bridge.read_sensor("x", None) == rhal.SKR_ENOSYS, "this body has no sensors to read")
    bridge, sim, master, loop = opened()
    settle(bridge, 3)
    assert bridge.emergency_stop() == rhal.SKR_OK and bridge.close() == rhal.SKR_OK
    sim.poll()  # the stop is still in the controllers' inbox until their loop runs
    assert all(c.mode == p.MODE_ZERO_VELOCITY for c in sim.controllers.values())
    for controller in sim.controllers.values():
        controller.mode = p.MODE_POSITION  # a person has put them back
    assert bridge.open(config()) == rhal.SKR_OK
    code, state = read(bridge)
    check(not state.flags & rhal.FLAG_ESTOP and bridge.write_command(command(GOOD), 0) == rhal.SKR_OK, "a stop does not outlive its session: opened again, the same bridge takes commands")
    bridge, sim, master, loop = opened()
    sim.controllers[1].inject_fault(33)
    sim.controllers[2].inject_fault(33)
    bridge.close()
    check(bridge.open(config()) == rhal.SKR_ENODEV, "…while a fault does: the next session is refused over it until a person has cleared it")

    section("the python-host entry point")
    from skynet_moteus import __main__ as entry

    previous = os.environ.get("SKYNET_MOTEUS_SIM")
    for bogus in ("bogus", "0", "17", "-1"):
        os.environ["SKYNET_MOTEUS_SIM"] = bogus
        try:
            result, text = said(entry.main)
            check(result == 1 and "number of simulated controllers" in text, f"a SKYNET_MOTEUS_SIM of {bogus!r} refuses, naming what it must be")
        finally:
            if previous is None:
                del os.environ["SKYNET_MOTEUS_SIM"]
            else:
                os.environ["SKYNET_MOTEUS_SIM"] = previous

    # ── what the first seeding campaign found nobody had written ────────────
    section("what seeding defects into this driver showed was missing (DRY-COVERAGE-PLAN.md, 12.E.10): each check was red against the defect before it was kept")

    # the wire: byte order, signedness, range, and what a refusal says
    check(p.pack_value(1, p.INT32) == bytes.fromhex("01000000") and p.pack_value(-2, p.INT16) == bytes.fromhex("feff") and p.pack_value(-1, p.INT8) == b"\xff"
          and p.pack_value(0.25, p.F32) == bytes.fromhex("0000803e"), "a value is packed little-endian at its own width, and signed: the int32 1 is 01 00 00 00, the int16 -2 is fe ff, the int8 -1 is ff")
    check(p.parse_frame(bytes.fromhex("2100ff")) == [p.Register("reply", 0, p.INT8, -1)] and p.parse_frame(bytes.fromhex("2500feff")) == [p.Register("reply", 0, p.INT16, -2)]
          and p.parse_frame(bytes.fromhex("290001000000")) == [p.Register("reply", 0, p.INT32, 1)], "…and read back from the wire's own bytes the same way: nothing here is read by the function that wrote it")
    check(p.encode(10.0, p.INT16, p.POSITION) == 32767 and p.encode(-10.0, p.INT16, p.POSITION) == -32767 and p.encode(1e6, p.INT32, p.POSITION) == 2147483647
          and p.encode(-1e6, p.INT32, p.POSITION) == -2147483647 and p.encode(-10.0, p.INT8, p.TIME) == -127,
          "a value past the range saturates at each width, one short of the integer that is the wire's NaN")
    check(refused(lambda: p.read_varuint(bytes([0x80] * 5 + [0x00]), 0)) and refused(lambda: p.read_varuint(bytes([0xFF, 0xFF, 0xFF, 0xFF, 0x1F]), 0))
          and p.read_varuint(bytes([0xFF, 0xFF, 0xFF, 0xFF, 0x0F]), 0) == (0xFFFFFFFF, 5), "a varuint is five bytes at most, even when the sixth would be zero, and its fifth carries only the uint32's last four bits")
    check(refused(lambda: p.split_id(0x10000)) and refused(lambda: p.split_id(-1)), "an identifier is sixteen bits: a longer one is not a conversation id")
    check(refused(lambda: p.write_registers(0, p.INT8, [])) and refused(lambda: p.read_registers(0, p.F32, 0)), "a run of no registers is neither written nor asked for")
    check(refused(lambda: p.parse_frame(b"\x10\x00")) and refused(lambda: p.parse_frame(b"\x00\x00")), "…nor accepted from a frame that ends at its count of none")
    said_error = why_refused(lambda: p.parse_status(status_frame() + p.error_subframe("read", 0x0F, 1)))
    check("register 0xf" in said_error and "error (1)" in said_error, "a status that carries an error says which register and which code: it is a controller's refusal, not a malformed frame")
    check("something other than register replies" in why_refused(lambda: p.parse_status(p.write_registers(0, p.INT8, [10]) + status_frame()[3:])),
          "…and a frame in which a register of the status is written and not replied is no status: a command echoed back is not what a controller said")
    check(p.position_command(0.25, 0.5, 0.125, 0.5, 0.75, 2.0, None) == bytes.fromhex("01000a0c0620" "0000803e" "0000003f" "0000003e" "0000003f" "0000403f" "00000040"),
          "the six command registers go out in the maker's order, each its own number: position 0.25, velocity 0.5, feedforward 0.125, the two gains 0.5 then 0.75, the ceiling 2.0")

    # open
    for bad in ([-1.0, 0.0, 1.0], [-1.0, math.inf], [-math.inf, 1.0]):
        bridge, sim, master, loop = build()
        check(bridge.open(json.dumps({"joints": [{"id": 1, "name": "J", "limits": bad}], "stop_mode": "brake"}, allow_nan=True)) == rhal.SKR_ENODEV and not master.sent,
              f"`limits` of {bad!r} is refused with nothing sent: three numbers are not a range, and an infinite bound is not one the operator set")
    bridge, sim, master, loop = build(ids=(1,))
    loop.endpoint().send(FdFrame(0x0100, p.pad(status_frame(mode=p.MODE_FAULT, fault=33))))
    check(bridge.open(config(ids=(1,))) == rhal.SKR_OK, "a fault an earlier session saw is discarded at open even when it is the only thing waiting: the first frame read would be taken for the answer (one axis)")
    bridge, sim, master, loop = build(position_min=0.1, position_max=0.1)
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "empty or reversed" in text, "a range a controller reports with its minimum equal to its maximum is empty, and is refused as that")
    bridge, sim, master, loop = build(ids=(1,))
    sim.controllers[1].unanswered = {"servopos.position_max"}
    code, text = said(lambda: bridge.open(config(ids=(1,))))
    check(code == rhal.SKR_ENODEV and "did not answer" in text and "no `limits` were given" in text,
          "a controller that answers one bound of its travel and not the other has not said whether the joint is bounded: refused, naming `limits` as the way through")
    bridge, sim, master, loop = build(ids=(1,))
    sim.controllers[1].unanswered = {"servopos.position_max"}
    assert bridge.open(json.dumps({"joints": [{"id": 1, "name": "J1", "limits": [-1.0, 1.0]}], "stop_mode": "brake"})) == rhal.SKR_OK
    travel = json.loads(bridge.describe())["joints"][0]["limits"]
    check(near(travel["position_min_rad"], -1.0, 1e-9) and near(travel["position_max_rad"], 1.0, 1e-9), "…and with `limits` given the operator's range is the joint's, not the one bound the controller did answer")
    bridge, sim, master, loop = build()
    for controller in sim.controllers.values():
        controller.diag_chunk = 2
    check(bridge.open(config()) == rhal.SKR_OK and all(near(j["limits"]["position_max_rad"], 0.25 * TWO_PI, 1e-6) and near(j["limits"]["position_min_rad"], -0.25 * TWO_PI, 1e-6)
                                                      for j in json.loads(bridge.describe())["joints"]),
          "a configuration value that arrives in pieces of two bytes is read to the end of its line: a value cut off mid-number is a different number")
    bridge, sim, master, loop = build()
    sim.controllers[1].position = -0.4
    code, text = said(lambda: bridge.open(config()))
    check(code == rhal.SKR_ENODEV and "outside the travel" in text and not master.commands(1), "an axis found below its travel is refused as well as one above it, before it is told to hold")
    bridge, sim, master, loop = build()
    sim.controllers[1].position = 0.2
    sim.controllers[2].position = -0.2
    assert bridge.open(config(watchdog_ms=250, max_torque_nm=1.5)) == rhal.SKR_OK
    first = {i.register: i.value for i in master.commands(1)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(math.isnan(first[p.COMMAND_POSITION]) and near(first[p.COMMAND_MAX_TORQUE], 1.5, 1e-6) and near(first[p.COMMAND_WATCHDOG], 0.25, 1e-6),
          "the first command a controller is given holds where it is (no position, which the firmware reads as 'here'), under the operator's torque ceiling and watchdog, not the controller's own")
    settle(bridge, 20)
    check(near(sim.controllers[1].position, 0.2, 5e-3) and near(sim.controllers[2].position, -0.2, 5e-3), "…and the axes are where they were put once the session is up: bringing a body up moves nothing")
    bridge, sim, master, loop = opened(stop_mode="zero_velocity")
    settle(bridge, 3)
    bridge.emergency_stop()
    read(bridge)
    assert bridge.release_stop() == rhal.SKR_OK
    assert bridge.open(config()) == rhal.SKR_OK
    sim.controllers[1].mode = p.MODE_ZERO_VELOCITY  # somebody else's doing, in a session that released nothing
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_FAULT, "a release does not outlive its session: an axis in the stop mode in a session that released nothing is a fault, not an axis waiting to resume")
    bridge, sim, master, loop = opened()
    sim.controllers[2].mute = True
    assert said(lambda: bridge.open(config()))[0] == rhal.SKR_ENODEV
    master.clear()
    code, text = said(lambda: bridge.write_command(command(GOOD), 0))
    check(code == rhal.SKR_EPERM and not master.sent and "J2 has not reported" in text,
          "a re-open that was refused leaves an axis that has never reported in this session, and a command is refused over it, naming it, with nothing sent")

    # the waits, on a bus that does not say at once that nothing is coming
    def timed(ids=IDS, **sim_args):
        clock = SteppingClock()
        bridge, sim, master, loop = build(ids, clock=clock, waiting=True, **sim_args)
        return bridge, sim, master, loop, clock

    bridge, sim, master, loop, clock = timed()
    sim.controllers[2].mute = True
    started = clock.now
    code, text = said(lambda: bridge.open(config()))
    spent = clock.now - started
    check(code == rhal.SKR_ENODEV and "controller 2" in text and 0.45 < spent < 0.6, f"a controller that does not answer is waited for half a second and no longer ({spent:.3f} s on the test's clock)")
    bridge, sim, master, loop, clock = timed()
    assert bridge.open(config()) == rhal.SKR_OK
    sim.controllers[2].mute = True
    started = clock.now
    code, state = read(bridge)
    spent = clock.now - started
    check(code == rhal.SKR_OK and state.flags & rhal.FLAG_DEGRADED and 0.045 < spent < 0.07, f"a read waits for a silent controller for the default window, 50 ms, and no longer ({spent:.3f} s)")
    bridge, sim, master, loop, clock = timed()
    assert bridge.open(config(window_ms=20)) == rhal.SKR_OK
    sim.controllers[2].mute = True
    started = clock.now
    read(bridge)
    spent = clock.now - started
    check(0.019 < spent < 0.035, f"…and for the window the configuration gives, in milliseconds ({spent:.3f} s for 20 ms)")
    bridge, sim, master, loop, clock = timed(ids=(1,))
    sim.controllers[1].refuse_diagnostic = True
    started = clock.now
    code = bridge.open(json.dumps({"joints": [{"id": 1, "name": "J1", "limits": [-1.0, 1.0]}], "stop_mode": "brake"}))
    spent = clock.now - started
    check(code == rhal.SKR_OK and 0.9 < spent < 1.2, f"a diagnostic channel that says nothing is waited for half a second a value, and no longer ({spent:.3f} s for the two bounds)")
    bridge, sim, master, loop, clock = timed(ids=(1,))
    sim.controllers[1].refuse = {p.MODE}
    started = clock.now
    code, text = said(lambda: bridge.open(config(ids=(1,))))
    spent = clock.now - started
    check(code == rhal.SKR_ENODEV and "told to hold and reports `stopped`" in text and 0.95 < spent < 1.15, f"an axis that never takes the hold is waited for one second and no longer ({spent:.3f} s)")
    bridge, sim, master, loop, clock = timed()
    assert bridge.open(config()) == rhal.SKR_OK
    check(json.loads(bridge.describe())["simulator"] is True, "a body on a bus that is not the in-process loopback is declared simulated unless the configuration says it is not (§35.3)")
    bridge, sim, master, loop, clock = timed()
    assert bridge.open(config(simulator=False)) == rhal.SKR_OK
    check(json.loads(bridge.describe())["simulator"] is False, "…and is declared real when the configuration says so")

    # read: what is waiting, and what arrives during the cycle
    bridge, sim, master, loop = opened()
    for ident in IDS:
        loop.endpoint().send(FdFrame(ident << 8, p.pad(status_frame(position=0.2 if ident == 1 else -0.2))))
    code, state = read(bridge)
    check(code == rhal.SKR_OK and near(state.position_rad[0], 0.0, 1e-6) and near(state.position_rad[1], 0.0, 1e-6),
          "statuses already waiting when a read begins are taken before the queries go out, so the replies the read waits for are the ones it asked for")
    bridge, sim, master, loop = opened()
    late(master, loop, FdFrame(0x0100, p.pad(status_frame(mode=p.MODE_FAULT, fault=33))))
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_FAULT and state.fault_code == (1 << 16) | (p.MODE_FAULT << 8) | 33, "a status that arrives while the cycle is being collected is in that cycle's state: a fault does not read a cycle late")
    bridge, sim, master, loop = opened()
    late(master, loop, FdFrame(0x0107, p.pad(status_frame(position=0.2))))
    before = bridge.unexpected_frames
    code, state = read(bridge)
    check(bridge.unexpected_frames == before + 1 and near(state.position_rad[0], 0.0, 1e-6), "a status addressed to another sender is counted and not applied: another master's conversation is not this body's state")
    bridge, sim, master, loop = opened()
    late(master, loop, FdFrame(0x10100, b"\x00", extended=True))
    before = bridge.unexpected_frames
    code, state = read(bridge)
    check(bridge.unexpected_frames == before + 1 and code == rhal.SKR_OK, "a frame whose identifier is past sixteen bits is counted and not read as a conversation")
    bridge, sim, master, loop = opened()
    sim.controllers[1].inject_fault(33)
    sim.controllers[2].inject_fault(34)
    code, state = read(bridge)
    check(state.fault_code == (1 << 16) | (p.MODE_FAULT << 8) | 33, "with two axes faulted the word names the first, the one a person looks at first")
    bridge, sim, master, loop = opened()
    sim.controllers[1].limiting = 99
    sim.controllers[2].limiting = 100
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT and state.fault_code == (1 << 16) | (p.MODE_POSITION << 8) | 99, "…and with two axes limiting, the first")

    # write
    bridge, sim, master, loop = opened()
    master.clear()
    check(bridge.write_command(command(GOOD, mode=200, modes=[rhal.JMODE_POSITION, rhal.JMODE_POSITION]), 0) == rhal.SKR_EINVAL and not master.sent,
          "a body mode the ABI does not define is malformed even when every joint names its own")
    bridge, sim, master, loop = opened(watchdog_ms=250, max_torque_nm=1.5)
    settle(bridge, 3)
    master.clear()
    assert bridge.write_command(command([0.2, 0.0], mask=[1, 0]), 0) == rhal.SKR_OK
    held = {i.register: i.value for i in master.commands(2)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(near(held[p.COMMAND_MAX_TORQUE], 1.5, 1e-6) and near(held[p.COMMAND_WATCHDOG], 0.25, 1e-6), "a hold is written under the operator's torque ceiling and watchdog like any other command: a joint held with the controller's own is the one that is off its leash")
    bridge, sim, master, loop = opened()
    settle(bridge, 3)
    assert bridge.write_command(command([1.0, 0.0]), 0) == rhal.SKR_OK
    assert bridge.write_command(command([0.05, 0.0], modes=[rhal.JMODE_TORQUE, rhal.JMODE_POSITION]), 0) == rhal.SKR_OK
    sim.controllers[1].position = 0.07
    settle(bridge, 3)
    master.clear()
    assert bridge.write_command(command([0.0, 0.0], mask=[0, 1]), 0) == rhal.SKR_OK
    c1 = {i.register: i.value for i in master.commands(1)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(near(c1[p.COMMAND_POSITION], 0.07, 5e-3), "a joint last commanded in torque is held where it is measured, not at the position it was last told to go to before the torque")
    bridge, sim, master, loop = opened()
    settle(bridge, 3)
    assert bridge.write_command(command([0.2, 0.0], mask=[1, 0]), 0) == rhal.SKR_OK
    assert bridge.write_command(command([0.2, 0.3]), 0) == rhal.SKR_OK
    master.clear()
    assert bridge.write_command(command([0.2, 0.0], mask=[1, 0]), 0) == rhal.SKR_OK
    c2 = {i.register: i.value for i in master.commands(2)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(near(c2[p.COMMAND_POSITION], 0.3 / TWO_PI, 1e-6), "a joint that was held and is commanded again is held at the new setpoint the next time it is masked, not at the hold it had before")
    bridge, sim, master, loop = opened(stop_mode="zero_velocity")
    settle(bridge, 3)
    assert bridge.write_command(command([0.2, 0.0], mask=[1, 0]), 0) == rhal.SKR_OK
    bridge.emergency_stop()
    read(bridge)
    sim.controllers[2].position = 0.1  # moved by hand while stopped
    assert bridge.release_stop() == rhal.SKR_OK
    read(bridge)
    master.clear()
    assert bridge.write_command(command([0.2, 0.0], mask=[1, 0]), 0) == rhal.SKR_OK
    c2 = {i.register: i.value for i in master.commands(2)[0] if isinstance(i, p.Register) and i.kind == "write"}
    check(near(c2[p.COMMAND_POSITION], 0.1, 5e-3), "a release latches the hold again from where the axis is now: a joint moved by hand while stopped is not taken back to where it was before")

    # the stop, and what the judgement makes of an axis that did not take it
    bridge, sim, master, loop = opened(stop_mode="zero_velocity")
    settle(bridge, 3)
    sim.controllers[1].refuse = {p.MODE}
    bridge.emergency_stop()
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_ESTOP and state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT
          and sim.controllers[1].mode == p.MODE_POSITION and sim.controllers[2].mode == p.MODE_ZERO_VELOCITY,
          "an axis still in position after the stop was asked for reads DEGRADED: the stop did not take on it, and nothing else would say so — and it is not a fault, which is what every axis reads for a cycle after a stop")
    bridge, sim, master, loop = opened(stop_mode="zero_velocity")
    bridge.emergency_stop()
    read(bridge)
    sim.controllers[1].inject_fault(33)
    code, state = read(bridge)
    check(state.flags & rhal.FLAG_FAULT and state.flags & rhal.FLAG_ESTOP, "a fault an axis takes after the stop is a fault: the latch makes the stop mode expected and nothing else")

    # ── vcan ─────────────────────────────────────────────────────────────────
    if "--vcan" in sys.argv[1:]:
        vcan_checks("vcan0")
    elif can_fd_available():
        check(True, "this kernel has CAN-FD support; `make check-vcan` runs the driver against it")
    else:
        print("  ----  this kernel has no CAN-FD support, so SocketCanFdBus was not exercised at all. `make check-vcan` needs a kernel with it.")

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 0 if not FAILURES else 1


def vcan_checks(interface: str) -> None:
    """The bridge on a kernel CAN-FD bus, against simulated controllers in another process.

    `make check-vcan` is the way in: it creates `vcan0` with the MTU an FD frame needs (72) in an unprivileged network namespace when there is none, which needs no root.
    Asked for and not possible is a failure, never a skip — `--vcan` is a request to measure this, and a run that measured nothing must not pass.
    """
    section(f"bridge — on {interface}, against controllers in another process")
    if not can_fd_available():
        check(False, "--vcan was asked for and this kernel will not give out a CAN socket that carries FD frames")
        return
    here = os.path.dirname(os.path.abspath(__file__))
    environment = dict(os.environ)
    environment["PYTHONPATH"] = os.pathsep.join([here, os.path.join(here, "..", "canopen"), os.path.join(here, "..", "..", "bindings", "python")])

    def controllers(*extra: str) -> subprocess.Popen:
        process = subprocess.Popen([sys.executable, "-m", "skynet_moteus.serve", interface, *extra], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=environment)
        assert process.stdout is not None
        if process.stdout.readline().strip() != "ready":
            raise AssertionError("the controllers did not come up")
        return process

    def stand_down(process: subprocess.Popen) -> None:
        assert process.stdin is not None
        process.stdin.close()
        process.wait(timeout=10)

    process = controllers("--ids", "1", "2")
    try:
        bridge = MoteusBridge(SocketCanFdBus(interface))
        check(bridge.open(config(simulator=True)) == rhal.SKR_OK, "controllers in another process come up: queried, their travel read over the diagnostic channel, told to hold, read back in position")
        state = rhal.State()
        check(bridge.read_state(state, 0) == rhal.SKR_OK and len(state.position_rad) == 2, "the first read answers")
        failures = 0
        for _ in range(150):
            bridge.write_command(command(GOOD), 0)
            if bridge.read_state(state, 0) != rhal.SKR_OK:
                failures += 1
        check(failures == 0, "a hundred and fifty commanded cycles, and every read answered")
        check(near(state.position_rad[0], GOOD[0], 0.03) and near(state.position_rad[1], GOOD[1], 0.03), "and the controllers reached their setpoints over the kernel's bus")
        check(not state.flags & rhal.FLAG_DEGRADED and not state.flags & rhal.FLAG_FAULT, "with no report stale and no fault")
        check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
        time_left = 0
        while time_left < 100:
            bridge.read_state(state, 0)
            if state.flags & rhal.FLAG_ESTOP and not state.flags & rhal.FLAG_FAULT:
                break
            time_left += 1
        check(state.flags & rhal.FLAG_ESTOP != 0, "the stop is visible in the next state")
        check(bridge.write_command(command(GOOD), 0) == rhal.SKR_EPERM, "and later commands are refused")
        check(bridge.release_stop() == rhal.SKR_OK and bridge.write_command(command(GOOD), 0) == rhal.SKR_OK, "the stop is released from zero velocity and the next command is taken")
        bridge.close()
    finally:
        stand_down(process)

    process = controllers("--ids", "1", "--fault", "1", "33")
    try:
        check(MoteusBridge(SocketCanFdBus(interface)).open(config(ids=(1,))) == rhal.SKR_ENODEV, "a controller in a standing fault in another process is refused at open")
    finally:
        stand_down(process)
    process = controllers("--ids", "1", "--mode", "1", "11")
    try:
        check(MoteusBridge(SocketCanFdBus(interface)).open(config(ids=(1,))) == rhal.SKR_ENODEV, "…and one in timeout")
    finally:
        stand_down(process)
    process = controllers("--ids", "1", "--no-diagnostic")
    try:
        check(MoteusBridge(SocketCanFdBus(interface)).open(config(ids=(1,))) == rhal.SKR_ENODEV, "…and one whose diagnostic channel does not answer, with no `limits`")
    finally:
        stand_down(process)
    process = controllers("--ids", "1", "--no-diagnostic")
    try:
        check(MoteusBridge(SocketCanFdBus(interface)).open(json.dumps({"joints": [{"id": 1, "name": "J1", "limits": [-1.0, 1.0]}], "stop_mode": "brake"})) == rhal.SKR_OK,
              "…which comes up when the operator gave `limits`")
    finally:
        stand_down(process)
    process = controllers("--ids", "3")
    try:
        check(MoteusBridge(SocketCanFdBus(interface)).open(config(ids=(1,))) == rhal.SKR_ENODEV, "a configuration naming a controller that is not on the bus is refused on a real bus too")
    finally:
        stand_down(process)


if __name__ == "__main__":
    raise SystemExit(main())

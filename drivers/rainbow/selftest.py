"""The Rainbow bridge against a simulated controller written here (DRY-COVERAGE-PLAN.md item 12.E.14): the frames, the two links, what it refuses to open on, the state, the refusals, the faults, the stop, the motions and
the lines.

    PYTHONPATH=../../bindings/python:. python3 selftest.py

Needs python3 on Linux and nothing of the maker's. The simulated controller agrees with whoever wrote it (F15); `layout_check.py` holds the driver's text and packets to Rainbow's own client (`rbpodo`), which is run, and what
this shows is the driver's own logic.

Every wait is for a state (`until`), never for a count of polls or a sleep that something else is supposed to have finished inside: a loaded machine stretches every sleep (F210, `tools/flake-hunt.sh`). Private state
is set only on a bridge that has no thread alive to change it.
"""

from __future__ import annotations

import contextlib
import importlib
import io
import json
import math
import os
import re
import select
import socket
import struct
import sys
import threading
import time
from unittest import mock

import skynet_rhal as rhal

from skynet_rainbow import bridge as bridge_module, frames, link
from skynet_rainbow.bridge import RainbowBridge, TOKEN
from skynet_rainbow.controller_sim import SimulatedController

CHECKS = 0
FAILURES: list[str] = []
SCALE = 20.0           # the simulated joints move this much faster than their speed says
LIMITS = [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]]
DEG = math.pi / 180.0


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")
        if detail:
            print(f"        what the driver said: {detail!r}")


def section(title: str) -> None:
    print(f"\n{title}")


def until(predicate, timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # noqa: BLE001 - a predicate that cannot be evaluated yet is not true yet
            pass
        time.sleep(0.005)
    return bool(predicate())


def raises(kind: type, function, *args, **kwargs) -> str | None:
    """The message of the `kind` the call raises, or None when it raises nothing (or something else)."""
    try:
        function(*args, **kwargs)
    except kind as error:
        return str(error)
    except Exception:  # noqa: BLE001
        return None
    return None


def new_sim(**kw) -> SimulatedController:
    kw.setdefault("time_scale", SCALE)
    return SimulatedController("RB5-850", **kw).start()


def settings(sim: SimulatedController, **extra) -> dict:
    out = {"host": "127.0.0.1", "port": sim.command_port, "data_port": sim.data_port, "model": sim.model, "simulator": sim.simulation, "limits": LIMITS, "rate_hz": 100, "timeout_ms": 120,
           "roles": {"j2": "right_arm.shoulder.pitch", "j6": "right_arm.wrist.roll"}}
    out.update(extra)
    return {k: v for k, v in out.items() if v is not None}


def config(sim: SimulatedController, **extra) -> str:
    return json.dumps(settings(sim, **extra))


def opened(sim: SimulatedController, **extra) -> RainbowBridge:
    until(lambda: not sim.robot_state != frames.ROBOT_IDLE, 3.0)
    bridge = RainbowBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge


def opens_with(sim: SimulatedController, **extra) -> tuple[int, str, RainbowBridge]:
    bridge = RainbowBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    return code, said.getvalue(), bridge


def refused(text: str) -> tuple[int, str]:
    bridge = RainbowBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(text)
    bridge.close()
    return code, said.getvalue()


def state(bridge: RainbowBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def cmd(values: list[float], mask: list[int] | None = None, mode: int = rhal.MODE_POSITION, count: int | None = None) -> rhal.Command:
    n = count if count is not None else len(values)
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=list(values), mask=mask if mask is not None else [1] * n)


def pose(value: float | list[float]) -> list[float]:
    return [value] * 6 if isinstance(value, (int, float)) else list(value)


def write(bridge: RainbowBridge, command: rhal.Command) -> tuple[int, str]:
    bridge._last_refusal = None  # noqa: SLF001 - a refusal is said once for a run of the same cause; each check asks to hear it
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def ext(bridge: RainbowBridge, text: str) -> tuple[int, object]:
    code, body = bridge.extension(TOKEN, text.encode())
    try:
        return code, json.loads(body)
    except json.JSONDecodeError:
        return code, body.decode()


def ready(bridge: RainbowBridge, timeout: float = 5.0) -> bool:
    return until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, timeout)


def said_by(function, *args) -> tuple[object, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        result = function(*args)
    return result, said.getvalue()


def fds() -> int:
    return len(os.listdir("/proc/self/fd"))


def settled_fds() -> int:
    """The number of open descriptors once nothing is closing them: the simulated controller's own threads close their side of a connection a moment after the driver closes its side."""
    last, since, deadline = fds(), time.monotonic(), time.monotonic() + 3.0
    while time.monotonic() < deadline:
        time.sleep(0.02)
        now = fds()
        if now != last:
            last, since = now, time.monotonic()
        elif time.monotonic() - since >= 0.25:
            break
    return last


def holding_the_command_port(bridge: RainbowBridge, seconds: float) -> threading.Thread:
    """A thread that owns the command port's lock for `seconds`, as a write in flight does."""
    taken = threading.Event()

    def hold() -> None:
        with bridge._command_lock:  # noqa: SLF001
            taken.set()
            time.sleep(seconds)

    thread = threading.Thread(target=hold, daemon=True)
    thread.start()
    taken.wait(1.0)
    return thread


def packet_of(**fields) -> bytes:
    return frames.encode_state(**fields)


def servo_texts(sim: SimulatedController) -> list[str]:
    with sim._lock:  # noqa: SLF001
        return [c for c in sim.commands if c.startswith("move_servo_j")]


def lifeless_bridge(**over) -> RainbowBridge:
    """A bridge with no line and no thread, holding a reading made here: for what `read_state`, `write_command` and the waits make of one, with nothing else able to change it."""
    bridge = RainbowBridge()
    bridge._opened = True  # noqa: SLF001
    bridge._command = link.CommandChannel("127.0.0.1", 1)  # noqa: SLF001 - not open
    bridge._data = link.DataChannel("127.0.0.1", 1)  # noqa: SLF001 - not open
    bridge._limits = [(a, b) for a, b in LIMITS]  # noqa: SLF001
    bridge._servo = {"t1": 0.01, "t2": 0.03, "gain": 0.5, "alpha": 0.5}  # noqa: SLF001
    set_reading(bridge, **over)
    return bridge


def set_reading(bridge: RainbowBridge, *, errors: list[int] | None = None, **fields) -> frames.State:
    reading = frames.decode_state(packet_of(**fields))
    with bridge._lock:  # noqa: SLF001
        bridge._latest, bridge._stamp_ns, bridge._dead, bridge._errors = reading, rhal.acquisition_ns(), None, list(errors or [])  # noqa: SLF001
    return reading


def main_frames() -> None:
    section("frames — the commands, byte for byte")
    check(frames.servo_j([0.0] * 6, 0.01, 0.1, 1.0, 0.5) == "move_servo_j(jnt[0, 0, 0, 0, 0, 0],0.01,0.1,1,0.5)", "a servo target is `move_servo_j(jnt[...],t1,t2,gain,alpha)`, the joints separated by a comma and a space as the maker's client writes them")
    check(frames.servo_j([10.5, -20.25, 30, 45.123456, -90, 180], 0.002, 0.03, 0.5, 0.5) == "move_servo_j(jnt[10.5, -20.25, 30, 45.123456, -90, 180],0.002,0.03,0.5,0.5)", "with the numbers as they are: no trailing zeros, six decimals kept")
    for value, text in ((0.0, "0"), (-0.0, "0"), (1e-9, "0"), (-1e-9, "0"), (2.0, "2"), (0.01, "0.01"), (123.456789, "123.456789"), (123.4567891, "123.456789"), (-0.5, "-0.5"), (360.0, "360"), (0.0000006, "0.000001"), (100, "100")):
        check(frames.number(value) == text, f"{value!r} is written `{text}`")
    for bad in (math.nan, math.inf, -math.inf, True, "1", None):
        check(raises(frames.ProtocolError, frames.number, bad) is not None, f"{bad!r} is not a number the controller can be sent")
    check(frames.servo_j([360.0, -360.0, 0, 0, 0, 0], 0.002, 0.021, 1e-6, 0.999999).startswith("move_servo_j(jnt[360, -360, 0, 0, 0, 0],0.002,0.021,0.000001,"), "the edges of every range are sent: an angle of plus or minus 360, the least t1, a t2 just inside, a gain just above 0")
    for text, call in (("-360 to 360", lambda: frames.servo_j([361.0] + [0.0] * 5, 0.01, 0.1, 1, 0.5)), ("-360 to 360", lambda: frames.servo_j([0.0] * 5 + [-360.001], 0.01, 0.1, 1, 0.5)),
                       ("six angles", lambda: frames.servo_j([0.0] * 5, 0.01, 0.1, 1, 0.5)), ("six angles", lambda: frames.servo_j("abcdef", 0.01, 0.1, 1, 0.5)), ("not a finite number", lambda: frames.servo_j([math.nan] + [0.0] * 5, 0.01, 0.1, 1, 0.5)),
                       ("t1", lambda: frames.servo_j([0.0] * 6, 0.0019, 0.1, 1, 0.5)), ("t2", lambda: frames.servo_j([0.0] * 6, 0.01, 0.02, 1, 0.5)), ("t2", lambda: frames.servo_j([0.0] * 6, 0.01, 0.2, 1, 0.5)),
                       ("gain", lambda: frames.servo_j([0.0] * 6, 0.01, 0.1, 0, 0.5)), ("gain", lambda: frames.servo_j([0.0] * 6, 0.01, 0.1, -1, 0.5)), ("alpha", lambda: frames.servo_j([0.0] * 6, 0.01, 0.1, 1, 0)),
                       ("alpha", lambda: frames.servo_j([0.0] * 6, 0.01, 0.1, 1, 1.0)), ("not a finite number", lambda: frames.servo_j([0.0] * 6, math.nan, 0.1, 1, 0.5))):
        check(text in (raises(frames.ProtocolError, call) or ""), f"a servo target outside the manual's range is refused, saying `{text}`")
    check(frames.move_j([0, 0, 90, 0, 90, 0], 60, 80) == "move_j(jnt[0, 0, 90, 0, 90, 0],60,80)", "a joint move is `move_j(jnt[...],speed,acceleration)`")
    check(all(raises(frames.ProtocolError, frames.move_j, [0.0] * 6, *args) is not None for args in ((0, 80), (60, 0), (-1, 80), (math.nan, 80), (60, math.inf))) and raises(frames.ProtocolError, frames.move_j, [400.0] + [0.0] * 5, 60, 80) is not None,
          "and refuses a speed or an acceleration that is not above 0, and an angle out of range")
    check((frames.task_stop(), frames.halt(), frames.task_pause(), frames.task_resume(False), frames.task_resume(True), frames.activate(), frames.pgmode(True), frames.pgmode(False)) ==
          ("task stop", "halt", "task pause", "task resume_a", "task resume_b", "mc jall init", "pgmode real", "pgmode simulation"), "the scripts of Appendix E are those words")
    check(frames.get_control_box_info() == "get_control_box_info()" and frames.print_variable("SD_J0_ANG", "abc") == 'print(SD_J0_ANG,"abc[", "]")', "and the information and variable requests are the maker's client's")
    check(raises(frames.ProtocolError, frames.print_variable, "SD J0", "abc") is not None and raises(frames.ProtocolError, frames.print_variable, "SD_J0", 'a"b') is not None, "a variable name or a tag with a space or a quote in it cannot be sent")
    check(frames.print_variable("SD_INIT_STATE_INFO", "#az_9Q") == 'print(SD_INIT_STATE_INFO,"#az_9Q[", "]")', "and a tag may hold `#` and `_`, which the maker's client's random tags do")
    check(raises(frames.ProtocolError, frames.print_variable, "SD_J0", "a b#") is not None and raises(frames.ProtocolError, frames.print_variable, "SD#J0", "abc") is not None and raises(frames.ProtocolError, frames.print_variable, "SD_J0", "") is not None, "but a variable name may not, and a tag may not be empty or hold a space")
    parsed = SimulatedController("RB5-850")._handle('print(SD_INIT_STATE_INFO,"#az_9Q[", "]")')  # noqa: SLF001
    check(parsed == ["info[print_var][#az_9Q[0.000]]"], "the simulated controller answers a variable asked for with such a tag, as the control box answers the client", str(parsed))
    check(frames.set_payload_info(1.5, (0.0, 0.0, 50.0)) == "set_payload_info(1.5,0,0,50)", "a payload is `set_payload_info(kg,x,y,z)`")
    check(raises(frames.ProtocolError, frames.set_payload_info, -0.1, (0, 0, 0)) is not None and raises(frames.ProtocolError, frames.set_payload_info, 1.0, (0, 0)) is not None and raises(frames.ProtocolError, frames.set_payload_info, math.nan, (0, 0, 0)) is not None,
          "and a negative payload, a centre of two numbers and a payload that is not a number are refused")

    section("frames — what the controller says")
    ack = frames.parse_event("The command was executed")
    check((ack.kind, ack.category, ack.message) == ("ack", "", "") and frames.parse_event(" The command was executed \r").kind == "ack", "the acknowledgement is its own event, whatever whitespace is round it")
    check(frames.parse_event("The command is not allowed.").kind == "rejected", "a line the control box does not allow is `rejected` (Appendix E: the first word is filtered)")
    event = frames.parse_event("info[motion_changed][7100]")
    check((event.kind, event.category, event.message, event.code) == ("info", "motion_changed", "7100", None), "`info[motion_changed][7100]` is an info of that category, whose message is the motion's number")
    event = frames.parse_event("warn[code][562]")
    check((event.kind, event.category, event.message, event.code) == ("warn", "code", "562", 562), "`warn[code][562]` is a warning with a code")
    check(frames.parse_event("error[code][3]").code == 3 and frames.parse_event("error[code][ 485 ]").code == 485 and frames.parse_event("error[code][abc]").code is None and frames.parse_event("error[code][]").code is None
          and frames.parse_event("info[print_var][12]").code is None, "an error's code is the whole number in its message, whatever is round it, and a message that is not one, or is not a code's, has none")
    event = frames.parse_event("info[print_var][id[1234.000]]")
    check((event.category, event.message) == ("print_var", "id[1234.000]"), "a message may hold brackets of its own, and they are not the end of it")
    event = frames.parse_event("info[controlbox][26042604,0]")
    check((event.category, event.message) == ("controlbox", "26042604,0"), "the control box's information is its software version and a type")
    syntax = frames.parse_event("error[msg][Parsing Err[V:26042604]\n0: syntax error at ' ( ' in this line:\nnonsense_command(1,2)\n]")
    check(syntax.kind == "error" and syntax.category == "msg" and syntax.message.startswith("Parsing Err[V:26042604]") and "nonsense_command(1,2)" in syntax.message, "the controller's syntax error, spread over four lines, is one event whose message is all of it")
    event = frames.parse_event("error[load][nofile]")
    check((event.kind, event.category, event.message, event.code) == ("error", "load", "nofile", None), "an error of another category is an error with no code")
    event = frames.parse_event("info[system]")
    check((event.kind, event.category, event.message) == ("info", "system", ""), "a message that is absent is empty")
    event = frames.parse_event("info[a][b")
    check((event.kind, event.category, event.message) == ("info", "a", "b"), "a message whose bracket is not closed is what there is of it")
    for text in ("", "garbage", "hello[world]", "info", "INFO[a][b]", "[a][b]"):
        check(frames.parse_event(text).kind == "unknown", f"{text!r} is unknown: not a thing the controller says, and never an exception")
    check(frames.bracket_depth("info[a][b]") == 0 and frames.bracket_depth("error[msg][Parsing Err[V:1]") == 1 and frames.bracket_depth("]") == -1 and frames.bracket_depth("a[[b") == 2 and frames.bracket_depth("") == 0,
          "bracket depth counts the open minus the closed")

    section("frames — the state packet")
    raw = packet_of(ref=(1.0, 2.0, 3.0, 4.0, 5.0, 6.0), ang=(1.5, 2.5, 3.5, 4.5, 5.5, 6.5), cur=(0.1,) * 6, temperature=(41.0,) * 6, task_state=frames.TASK_PAUSED, robot_state=3, speed_bar=0.5, chunk1=(1 << 6) | (1 << 12), joint_info=(0b110, 0, 0, 0, 0, 0b100000000110),
                    mode=frames.MODE_REAL, init_state=6, collision=1, sos=7, ems=3, paused_flag=1, freedrive=1, time_raw=21950)
    decoded = frames.decode_state(raw)
    check(len(raw) == frames.PACKET_BYTES == 580 and raw[:4] == bytes([0x24, 0x40, 0x02, 0x03]), "a packet is 580 bytes and begins `$`, 576 (its size without the header, little-endian) and 3")
    check(decoded.ref == (1.0, 2.0, 3.0, 4.0, 5.0, 6.0) and decoded.ang == (1.5, 2.5, 3.5, 4.5, 5.5, 6.5) and all(abs(c - 0.1) < 1e-6 for c in decoded.cur) and decoded.temperature == (41.0,) * 6, "the reference, the encoder angles, the currents and the temperatures are each at their own offset")
    check((decoded.task_state, decoded.robot_state, decoded.mode, decoded.init_state, decoded.collision, decoded.sos, decoded.ems, decoded.paused_flag, decoded.freedrive, decoded.time_raw) == (2, 3, 0, 6, 1, 7, 3, 1, 1, 21950) and abs(decoded.speed_bar - 0.5) < 1e-6,
          "and so is every integer the driver reads")
    check(decoded.joint_info == (6, 0, 0, 0, 0, 0b100000000110) and decoded.power_applied and decoded.sos_state and decoded.estop_bits == (0, 0), "a joint's information is its word, and the arm's power and SOS state are bits 6 and 12 of the first chunk")
    check(frames.decode_state(packet_of(chunk1=(1 << 3) | (1 << 5))).estop_bits == (1, 1) and frames.decode_state(packet_of(chunk1=1 << 3)).estop_bits == (1, 0) and not frames.decode_state(packet_of()).power_applied,
          "the stop bits are bits 3 and 5, reported as sent")
    check(frames.decode_state(packet_of(chunk1=(1 << 31) | 1)).chunk1 == (1 << 31) | 1, "the first chunk is unsigned: its top bit is not a sign")
    masked = bytearray(raw)
    for name, value in (("real_vs_simulation_mode", 0x21), ("init_state_info", 0x46), ("op_stat_collision_occur", 5), ("op_stat_sos_flag", 0x47), ("op_stat_ems_flag", 0x43), ("op_stat_soft_estop_occur", 3), ("is_freedrive_mode", 7)):
        struct.pack_into("<i", masked, frames.OFFSETS[name], value)
    wide = frames.decode_state(bytes(masked))
    check((wide.mode, wide.init_state, wide.collision, wide.sos, wide.ems, wide.paused_flag, wide.freedrive) == (1, 6, 1, 7, 3, 1, 3), "each is masked to the bits the document says are valid: 4, 6, 2, 6, 6, 1 and 2")
    check(frames.OFFSETS["jnt_ang"] == 32 and frames.OFFSETS["jnt_info"] == 372 and frames.OFFSETS["real_vs_simulation_mode"] == 404 and frames.OFFSETS["op_stat_sos_flag"] == 448 and frames.OFFSETS["safety_board_stat_info"] + 4 == frames.PACKET_BYTES
          and list(frames.OFFSETS.values()) == sorted(frames.OFFSETS.values()), "the offsets are the document's, in order, and the last field ends the packet")
    short, header = bytearray(raw[:-1]), bytearray(raw)
    check("580" in (raises(frames.ProtocolError, frames.decode_state, bytes(short)) or "") and raises(frames.ProtocolError, frames.decode_state, raw + b"\x00") is not None, "a packet of any other length is refused, saying it is 580")
    for index, value in ((0, 0x23), (1, 0x3F), (2, 0x03), (3, 0x04)):
        broken = bytearray(raw)
        broken[index] = value
        check("header" in (raises(frames.ProtocolError, frames.decode_state, bytes(broken)) or ""), f"a packet whose header byte {index} is {value:#04x} is refused: a `$`, its own size and the state's type")
    for name in ("jnt_ref", "jnt_ang"):
        broken = bytearray(raw)
        struct.pack_into("<f", broken, frames.OFFSETS[name] + 8, math.nan)
        check(name in (raises(frames.ProtocolError, frames.decode_state, bytes(broken)) or ""), f"{name} with a value that is not a number is refused, naming it")
    broken = bytearray(raw)
    struct.pack_into("<f", broken, frames.OFFSETS["default_speed"], math.nan)
    check(math.isnan(frames.decode_state(bytes(broken)).speed_bar), "a speed bar that is not a number is not a reason to refuse the packet: it is reported as it is")
    check(len(frames.CODES) == 606 and set(frames.CODES) == set(range(606)) and frames.describe_code(562) == "Joint angle of the target posture exceeds the specified range." and "does not list" in frames.describe_code(9999),
          "the maker's message table has its 606 codes, and a code that is not in it is said not to be")
    check(frames.ESTOP_CODES <= set(frames.CODES) and {485, 486, 487, 488} <= frames.ESTOP_CODES and 3 not in frames.ESTOP_CODES and all("Safety" in frames.CODES[c] or "Switch" in frames.CODES[c] for c in frames.ESTOP_CODES),
          "the codes read as a stop are in the table, are the ones whose words are about a safety switch, and do not take in a collision")

def tcp_server(script) -> tuple[int, threading.Thread]:
    """A listener on a loopback port that hands its first connection to `script(connection)`: a controller scripted by the test."""
    server = socket.socket()
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", 0))
    server.listen(1)

    def serve() -> None:
        connection, _ = server.accept()
        try:
            script(connection)
        finally:
            connection.close()
            server.close()

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    return server.getsockname()[1], thread


def collect(channel: link.CommandChannel, count: int, timeout: float = 3.0) -> list[frames.Event]:
    """The first `count` events the channel yields."""
    events: list[frames.Event] = []
    deadline = time.monotonic() + timeout
    while len(events) < count and time.monotonic() < deadline:
        got, _ = channel.poll(0.05)
        events.extend(got)
    return events


def main_links() -> None:
    section("the command port — a line out, the events in, and what comes in pieces")
    wrote: list[bytes] = []
    release = threading.Event()

    def hears(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        wrote.append(connection.recv(1024))
        connection.sendall(b"The command was executed\ninfo[motion_changed][7100]\n\n\nwarn[code][562]\r\n")
        release.wait(3.0)

    port, _ = tcp_server(hears)
    channel = link.CommandChannel("127.0.0.1", port)
    channel.open()
    check(channel.is_open and channel.target == f"127.0.0.1:{port}", "the channel is named by host and port")
    channel.send("task stop", time.monotonic() + 1.0)
    events = collect(channel, 3)
    release.set()
    check(wrote == [b"task stop\n"], "a line is written with a newline after it and nothing else", str(wrote))
    check([(e.kind, e.category, e.message) for e in events] == [("ack", "", ""), ("info", "motion_changed", "7100"), ("warn", "code", "562")], "the events come in the order they were said, blank lines and a carriage return are not events", str(events))
    check("one line" in (raises(frames.ProtocolError, channel.send, "task\nstop", time.monotonic() + 1.0) or "") and raises(frames.ProtocolError, channel.send, "task\rstop", time.monotonic() + 1.0) is not None and wrote == [b"task stop\n"],
          "a script with a line break in it is refused, and nothing was written")
    channel.close()
    check(not channel.is_open and "is not open" in (raises(link.LinkError, channel.send, "task stop", time.monotonic() + 1.0) or "") and "is not open" in (raises(link.LinkError, channel.poll, 0.0) or ""),
          "a channel that is closed carries nothing and polls nothing")

    pieces = [b"The comm", b"and was exec", b"uted\r\ninfo[sys", b"tem][halt]\ninfo[motion_changed][0]\nwar", b"n[code][568]\n"]
    gate = threading.Semaphore(0)

    def trickles(connection: socket.socket) -> None:
        for piece in pieces:
            gate.acquire(timeout=3.0)
            connection.sendall(piece)
        gate.acquire(timeout=3.0)

    port, _ = tcp_server(trickles)
    channel = link.CommandChannel("127.0.0.1", port)
    channel.open()
    seen: list[list[str]] = []
    for _ in pieces:
        gate.release()
        time.sleep(0.05)
        seen.append([f"{e.kind}:{e.category}:{e.message}" for e in collect(channel, 99, 0.15)])
    gate.release()
    check(seen[0] == [] and seen[1] == [] and seen[2] == ["ack::"] and seen[3] == ["info:system:halt", "info:motion_changed:0"] and seen[4] == ["warn:code:568"],
          "a line that arrives in pieces is one event, said when its end arrives and not before", str(seen))
    channel.close()

    section("the command port — a message whose brackets are not closed goes on to the next line")
    chunks = [b"error[msg][Parsing Err[V:26042604]\n", b"0: syntax error at ' ( ' in this line:\n", b"nonsense_command(1,2)\n", b"]\nThe command was executed\n"]
    gate = threading.Semaphore(0)

    def syntax(connection: socket.socket) -> None:
        for chunk in chunks:
            gate.acquire(timeout=3.0)
            connection.sendall(chunk)
        gate.acquire(timeout=3.0)

    port, _ = tcp_server(syntax)
    channel = link.CommandChannel("127.0.0.1", port)
    channel.open()
    after = []
    for _ in chunks[:3]:
        gate.release()
        time.sleep(0.02)
        after.append(collect(channel, 99, 0.06))
    gate.release()
    final = collect(channel, 2)
    gate.release()
    check(after == [[], [], []], "the first lines of the controller's syntax error are not events: the message is not whole")
    check([e.kind for e in final] == ["error", "ack"] and final[0].category == "msg" and final[0].message.startswith("Parsing Err[V:26042604]") and "nonsense_command(1,2)" in final[0].message and "syntax error" in final[0].message,
          "and when its last bracket closes they are one error, whose message is all four lines, followed by the next thing the controller says", str(final))
    channel.close()

    gate = threading.Semaphore(0)

    def never_closes(connection: socket.socket) -> None:
        gate.acquire(timeout=3.0)
        connection.sendall(b"error[msg][never closed\nThe command was executed\ninfo[system][halt]\n")
        gate.acquire(timeout=3.0)
        connection.sendall(b"info[a][open\nstill open\n")
        gate.acquire(timeout=3.0)

    port, _ = tcp_server(never_closes)
    channel = link.CommandChannel("127.0.0.1", port)
    channel.open()
    gate.release()
    events = collect(channel, 3)
    check([e.kind for e in events] == ["unknown", "ack", "info"] and "never closed" in events[0].raw, "an acknowledgement is not a part of a message: the message that was being joined is unknown, and the acknowledgement is an acknowledgement", str(events))
    with mock.patch.object(link, "PARTIAL_S", 0.1):
        gate.release()
        time.sleep(0.02)
        first = collect(channel, 99, 0.05)
        time.sleep(0.15)
        later, _ = channel.poll(0.0)
    gate.release()
    check(first == [] and len(later) == 1 and later[0].kind == "unknown" and "open" in later[0].raw, "a message that does not close in the time allowed is unknown, and is not waited for for ever")
    channel.close()
    gate = threading.Semaphore(0)

    def long_message(connection: socket.socket) -> None:
        gate.acquire(timeout=3.0)
        connection.sendall(b"error[msg][a\n" + b"x\n" * 8 + b"]\nThe command was executed\n")
        gate.acquire(timeout=3.0)
        connection.sendall(b"info[a][" + b"y" * 100 + b"\nz\n")
        gate.acquire(timeout=3.0)

    port, _ = tcp_server(long_message)
    channel = link.CommandChannel("127.0.0.1", port)
    channel.open()
    with mock.patch.object(link, "MAX_MESSAGE_LINES", 5), mock.patch.object(link, "MAX_MESSAGE_BYTES", 64):
        gate.release()
        events = collect(channel, 99, 0.4)
        gate.release()
        more = collect(channel, 1, 0.4)
    gate.release()
    check(events[0].kind == "unknown" and any(e.kind == "ack" for e in events), "a message of more lines than a controller sends is unknown, and what follows it is still read")
    check(len(more) == 1 and more[0].kind == "unknown", "and so is one of more bytes")
    channel.close()

    section("the command port — a line that does not end, a controller that hangs up, and a wait that something else ends")
    port, _ = tcp_server(lambda connection: (connection.sendall(b"a" * 20000), time.sleep(1.0)))
    channel = link.CommandChannel("127.0.0.1", port)
    channel.open()
    message = ""
    deadline = time.monotonic() + 3.0
    while not message and time.monotonic() < deadline:
        message = raises(link.LinkError, channel.poll, 0.1) or ""
    check("no line break" in message and not channel.is_open, "a controller that sends 8,192 bytes with no newline is not the command port, and the channel is closed", message)
    port, _ = tcp_server(lambda connection: connection.sendall(b"The command was executed\n"))
    channel = link.CommandChannel("127.0.0.1", port)
    channel.open()
    got = collect(channel, 1)
    message = ""
    deadline = time.monotonic() + 3.0
    while not message and time.monotonic() < deadline:
        message = raises(link.LinkError, channel.poll, 0.1) or ""
    check(len(got) == 1 and "closed the connection" in message and not channel.is_open, "a controller that hangs up is a channel that closes: what it said before is read first", message)
    sim = new_sim()
    try:
        channel = link.CommandChannel("127.0.0.1", sim.command_port)
        channel.open()
        events = channel.query(frames.get_control_box_info(), lambda e: e.kind == "info" and e.category == "controlbox", 1.0)
        check([e.kind for e in events] == ["ack", "info"] and events[-1].message == "26042604,0", "a query is the events up to the one that answers it, the acknowledgement among them")
        began = time.monotonic()
        message = raises(link.LinkError, channel.query, "get_control_box_info()", lambda e: e.kind == "error", 0.2) or ""
        check("did not answer get_control_box_info in 200 ms" in message and 0.15 < time.monotonic() - began < 1.0, "and a query that is not answered in its budget is a LinkError that names the command and the time", message)
        read_end, write_end = os.pipe()
        began = time.monotonic()
        threading.Timer(0.1, os.write, (write_end, b"x")).start()
        events, woke = channel.poll(2.0, (read_end,))
        took = time.monotonic() - began
        check(woke and events == [] and took < 1.0, f"a descriptor that becomes readable ends a wait, and says so ({took:.2f} s of a possible 2)")
        events, woke = channel.poll(0.0, (read_end,))
        check(woke, "(and stays readable until someone reads it)")
        os.close(read_end)
        os.close(write_end)
        channel.close()
        held = settled_fds()
        for _ in range(4):
            channel.open()
        check(settled_fds() <= held + 2, "a channel opened again lets go of the one it had: four reopenings leave one open (and the controller's end of it), not five")
        channel.close()
        sim.stop()
        check("could not be opened" in (raises(link.LinkError, channel.open, 0.3) or ""), "a controller that is not listening is a channel that could not be opened")
    finally:
        sim.stop()
    with mock.patch.object(socket, "create_connection", side_effect=OSError("no route to host")) as patched:
        message = raises(link.LinkError, link.CommandChannel("gateway.example", 5000).open, 0.7) or ""
    check("gateway.example:5000 could not be opened" in message and patched.call_args.args == (("gateway.example", 5000),) and patched.call_args.kwargs == {"timeout": 0.7}, "a controller is waited for as long as the caller says, and named by host and port when it cannot be reached", message)

    section("the data port — one request, one packet, and nothing older")
    sim = new_sim()
    try:
        data = link.DataChannel("127.0.0.1", sim.data_port)
        data.open()
        sim.ref[0] = 12.5
        packet = data.request(1.0)
        check(len(packet) == 580 and abs(frames.decode_state(packet).ref[0] - 12.5) < 1e-6 and sim.data_requests == 1, "a request is answered with the state packet, and the controller was asked once")
        sim.ref[0] = -3.0
        check(abs(frames.decode_state(data.request(1.0)).ref[0] + 3.0) < 1e-6 and sim.data_requests == 2, "the next request is answered with the state as it is now")
        sim.data_extra_packet = True
        check(abs(frames.decode_state(data.request(1.0)).ref[0] + 3.0) < 1e-6, "a packet of another type in front of the state is skipped by its own size")
        sim.data_extra_packet = False
        sim.data_garbage = True
        check(abs(frames.decode_state(data.request(1.0)).ref[0] + 3.0) < 1e-6, "so is noise that does not begin with `$`")
        sim.data_garbage = False
        sim.data_stalled = True
        began = time.monotonic()
        message = raises(link.LinkError, data.request, 0.05) or ""
        took = time.monotonic() - began
        check("did not answer reqdata in 50 ms" in message and took < 0.3 and not data.is_open, f"a request that is not answered in its budget is a LinkError that names the request and the time, and closes the line ({took:.2f} s)", message)
        sim.data_stalled = False
        check("is not open" in (raises(link.LinkError, data.request, 0.1) or ""), "a request on a line that is closed is refused")
        data.open()
        check(len(data.request(1.0)) == 580, "and the line opened again carries the next")
        sim.drop_connections("data")
        check(until(lambda: raises(link.LinkError, data.request, 0.3) is not None and not data.is_open, 3), "a controller that hangs up is a line that closes")
        data.close()
    finally:
        sim.stop()
    state_packet = packet_of(ref=(7.0,) * 6)
    stale = packet_of(ref=(99.0,) * 6)
    ready_to_answer = threading.Event()

    def sends_early(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.sendall(stale)                       # said before anything was asked: an answer to an earlier request
        ready_to_answer.wait(3.0)
        connection.recv(1024)
        connection.sendall(state_packet)

    port, _ = tcp_server(sends_early)
    data = link.DataChannel("127.0.0.1", port)
    data.open()
    time.sleep(0.15)
    ready_to_answer.set()
    check(frames.decode_state(data.request(1.0)).ref[0] == 7.0, "what is waiting when a request is sent is an answer to an earlier one, and is thrown away")
    data.close()

    other_sizes = bytes([0x24, 0x10, 0x00, 0x05]) + bytes(16) + bytes([0x24, 0xFF, 0xFF, 0x03]) + state_packet

    def odd(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        connection.sendall(other_sizes)

    port, _ = tcp_server(odd)
    data = link.DataChannel("127.0.0.1", port)
    data.open()
    check(frames.decode_state(data.request(1.0)).ref[0] == 7.0, "a packet of another type and size is skipped, and a header with a size no packet has is noise: the state is found behind them")
    data.close()
    halves = [state_packet[:100], state_packet[100:300], state_packet[300:]]

    def trickle(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        for half in halves:
            connection.sendall(half)
            time.sleep(0.05)

    port, _ = tcp_server(trickle)
    data = link.DataChannel("127.0.0.1", port)
    data.open()
    check(data.request(2.0) == state_packet, "a packet that arrives in three pieces is one packet")
    data.close()
    port, _ = tcp_server(lambda connection: (connection.recv(1024), connection.sendall(bytes([0x24, 0x40, 0x02, 0x03]) + bytes(50)), time.sleep(2.0)))
    data = link.DataChannel("127.0.0.1", port)
    data.open()
    began = time.monotonic()
    message = raises(link.LinkError, data.request, 0.2) or ""
    check("did not answer reqdata in 200 ms" in message and time.monotonic() - began < 1.0 and not data.is_open, "a packet that stops half way is a request that was not answered, and the line is closed so that its rest is not the next one's", message)
    port, _ = tcp_server(lambda connection: (connection.recv(1024), connection.sendall(b"\x00" * 200), time.sleep(2.0)))
    data = link.DataChannel("127.0.0.1", port)
    data.open()
    with mock.patch.object(link, "MAX_NOISE", 64):
        message = raises(link.LinkError, data.request, 1.0) or ""
    check("none of them begins a state packet" in message and not data.is_open, "a line that sends only noise is not the data port: it is closed, saying so", message)
    port, _ = tcp_server(lambda connection: (connection.recv(1024), None)[1])
    data = link.DataChannel("127.0.0.1", port)
    data.open()
    check("closed the connection" in (raises(link.LinkError, data.request, 1.0) or "") and not data.is_open, "a controller that hangs up instead of answering is a LinkError that says so")
    woken: list[float] = []
    hold = threading.Event()

    def slow(connection: socket.socket) -> None:
        connection.settimeout(3.0)
        connection.recv(1024)
        hold.wait(3.0)
        connection.sendall(state_packet)

    port, _ = tcp_server(slow)
    data = link.DataChannel("127.0.0.1", port)
    data.open()
    read_end, write_end = os.pipe()
    os.set_blocking(read_end, False)

    def on_wake() -> None:
        woken.append(time.monotonic())
        os.read(read_end, 64)
        hold.set()

    threading.Timer(0.1, os.write, (write_end, b"x")).start()
    packet = data.request(2.0, read_end, on_wake)
    check(packet == state_packet and len(woken) == 1, "a descriptor that becomes readable while the reply is awaited calls back, and the wait goes on to the reply")
    os.close(read_end)
    os.close(write_end)
    data.close()
    data = link.DataChannel("127.0.0.1", 1)
    data._buffer = bytes([0x24, 0xFF, 0xFF, 0x03]) + bytes([0x24, 0x40, 0x02, 0x03])  # noqa: SLF001
    packet, dropped = data._pop()  # noqa: SLF001
    check(packet is None and dropped == 4 and data._buffer == bytes([0x24, 0x40, 0x02, 0x03]), "a header whose size is no packet's is noise, dropped a byte at a time, and the start of a packet behind it is kept for the rest to arrive")  # noqa: SLF001
    data._buffer = state_packet[:300]  # noqa: SLF001
    check(data._pop() == (None, 0) and len(data._buffer) == 300, "a packet that has not all arrived is not taken")  # noqa: SLF001
    data._buffer = bytes([0x24, 0x00, 0x00, 0x03]) + state_packet  # noqa: SLF001
    packet, dropped = data._pop()  # noqa: SLF001
    check(packet == state_packet and dropped == 0, "a packet of size 0 is skipped as one of another size and not as noise")

def main_open() -> None:
    section("open — what a configuration may not say")
    sim = new_sim()
    try:
        base = settings(sim)
        code, said = refused("not json")
        check(code == rhal.SKR_EINVAL and "not JSON" in said, "a configuration that is not JSON is refused as invalid, saying so", said)
        code, said = refused("[1]")
        check(code == rhal.SKR_EINVAL and "not a JSON object" in said, "and one that is not an object", said)
        code, said = refused("")
        check(code == rhal.SKR_ENODEV and "`model`" in said, "an empty configuration is an arm with no model: refused, saying what is missing", said)
        code, said = refused(json.dumps({**base, "stop_mode": "stop"}))
        check(code == rhal.SKR_ENODEV and "'stop_mode'" in said and "keys it reads" in said, "a key this driver does not read is refused and named, and the keys it reads are listed (a Dobot's `stop_mode` is not here: the stop is `task stop`)", said)
        check(sim.commands == [], "none of these touched the controller")
        for model in (None, "", "UR5", "rb5-850", "RB", "RB5 850", 5, "RB5-850\n", "R5"):
            code, said = refused(json.dumps({**base, "model": model}))
            check(code == rhal.SKR_ENODEV and "`model`" in said and "does not say which arm" in said, f"a model of {model!r} is refused: the controller does not report which arm it is on, so the operator names it", said)
        for model in ("RB5-850", "RB10-1300E", "RB3-730ES-U", "RB16-900", "RB10_1300RE", "RB1-500ES-U"):
            bridge = RainbowBridge()
            with contextlib.redirect_stderr(io.StringIO()):
                opened_ok = bridge.open(json.dumps({**base, "model": model})) == rhal.SKR_OK
            slug = json.loads(bridge.describe())["robot_slug"] if opened_ok else None
            bridge.close()
            check(opened_ok and slug == "rainbow-" + model.lower().replace("_", "-"), f"{model} is a model, and the descriptor's slug is its name in lower case ({slug})")
        for host in (None, "", 5, ["127.0.0.1"]):
            code, said = refused(json.dumps({**base, "host": host}))
            check(code == rhal.SKR_ENODEV and "`host`" in said and "does not scan" in said, f"a host of {host!r} is refused: no arm is scanned for", said)
        for key in ("port", "data_port"):
            for value in (True, 0, -1, 65536, "5000", 5000.5, None):
                code, said = refused(json.dumps({**base, key: value}))
                # None is "absent": the factory port, which nothing listens on here, so the open fails for another reason
                if value is None:
                    continue
                check(code == rhal.SKR_ENODEV and f"`{key}`" in said and "1 to 65535" in said, f"a {key} of {value!r} is refused, saying what a port is", said)
        for claim in (None, "true", 1, 0):
            code, said = refused(json.dumps({**base, "simulator": claim}))
            check(code == rhal.SKR_ENODEV and "`simulator`" in said and "true or false" in said, f"a simulator claim of {claim!r} is refused: it is true or false", said)
        for given, fragment in ((None, "is not in the configuration"), ([[-1, 1]] * 5, "six [min, max] pairs"), ([[-1, 1, 2]] * 6, "six [min, max] pairs"), ("abc", "six [min, max] pairs"), ([[-1, 1]] * 5 + [[-1, "x"]], "six [min, max] pairs"),
                                ([[-1, 1]] * 5 + [[-1, True]], "six [min, max] pairs"), ([[-1, 1]] * 5 + [[-1, None]], "six [min, max] pairs"), ([[-1, 1]] * 5 + [[1, 1]], "j6 is [1, 1], which is not a range"), ([[-1, 1]] * 5 + [[2, 1]], "j6 is [2, 1]"),
                                ([[-1, 1]] * 5 + [[-7, 1]], "past a turn"), ([[-1, 1]] * 5 + [[-1, 7]], "past a turn")):
            code, said = refused(json.dumps({**base, "limits": given}))
            check(code == rhal.SKR_ENODEV and fragment in said, f"limits of {str(given)[:40]} are refused, saying why", said)
        for given in ([[-1, 1]] * 5 + [[-1, math.nan]], [[-1, 1]] * 5 + [[-1, math.inf]]):
            code, said = refused(json.dumps({**base, "limits": given}))
            check(code in (rhal.SKR_ENODEV, rhal.SKR_EINVAL) and ("six [min, max] pairs" in said or "not JSON" in said), "limits that are not finite numbers are refused", said)
        code, said = refused(json.dumps({**base, "limits": None}))
        check("maker's own URDF" in said, "and what is missing is explained: the maker's own URDF gives every joint the same placeholder, and the manual's is a picture", said)
        edge = [[-2 * math.pi, 2 * math.pi]] * 6
        bridge = RainbowBridge()
        with contextlib.redirect_stderr(io.StringIO()):
            check(bridge.open(json.dumps({**base, "limits": edge})) == rhal.SKR_OK, "a turn either way is the widest range there is, and is accepted")
        bridge.close()
        for kg, com in ((True, None), (-0.1, None), (100.1, None), (math.nan, None), ("2", None), (1.0, [0, 0]), (1.0, [0, 0, 2001]), (1.0, [0, 0, True]), (1.0, "abc"), (None, [0, 0, 0])):
            code, said = refused(json.dumps({**base, "payload_kg": kg, "payload_com_mm": com}))
            check(code == rhal.SKR_ENODEV and ("payload" in said), f"a payload of {kg!r} and a centre of {com!r} is refused", said)
        for kg, com in ((0.0, None), (100.0, None), (2.5, [10, -20, 30.5])):
            bridge = RainbowBridge()
            with contextlib.redirect_stderr(io.StringIO()):
                check(bridge.open(json.dumps({**base, "payload_kg": kg, "payload_com_mm": com})) == rhal.SKR_OK, f"a payload of {kg} kg with a centre of {com} is accepted, and is not told to the controller by an open")
            bridge.close()
        for key, bad in (("rate_hz", 0.9), ("rate_hz", 100.1), ("rate_hz", "50"), ("rate_hz", True), ("timeout_ms", 9.9), ("timeout_ms", 5001), ("stale_ms", 19.9), ("stale_ms", 30001), ("open_timeout_ms", 99), ("open_timeout_ms", 60001)):
            code, said = refused(json.dumps({**base, key: bad}))
            check(code == rhal.SKR_ENODEV and f"`{key}`" in said and "is not a number from" in said, f"{key} of {bad!r} is outside what it may be, and refused", said)
        for key, good_value in (("rate_hz", 1.0), ("rate_hz", 100.0), ("timeout_ms", 10.0), ("timeout_ms", 5000.0), ("stale_ms", 20.0), ("stale_ms", 30000.0), ("open_timeout_ms", 100.0), ("open_timeout_ms", 60000.0)):
            bridge = RainbowBridge()
            said = io.StringIO()
            with contextlib.redirect_stderr(said):
                code = bridge.open(json.dumps({**base, key: good_value}))
            bridge.close()
            if key == "rate_hz" and good_value == 1.0:
                # a second between commands is longer than any t2 the manual allows: refused for that, not as out of range
                check(code == rhal.SKR_ENODEV and "not longer than the control period" in said.getvalue() and "is not a number from" not in said.getvalue(), "rate_hz of 1 is within its bounds, and is refused because no t2 spans a second", said.getvalue())
            else:
                check(code == rhal.SKR_OK, f"{key} of {good_value} is within its bounds, and accepted", said.getvalue())
        for roles in ("abc", {"j7": "x"}, ["j1"]):
            code, said = refused(json.dumps({**base, "roles": roles}))
            check(code == rhal.SKR_ENODEV and "`roles`" in said and "j1, j2, j3, j4, j5, j6" in said, f"roles of {roles!r} are refused: a mapping from a joint's name", said)
        check(set(sim.commands) <= {"get_control_box_info()"}, "none of these wrote anything to the controller but the information request of the opens that succeeded", str(set(sim.commands)))

        section("open — the servo parameters are the manual's, and the maker's own ROS 2 package's where they are not said")
        for given, fragment in (("abc", "mapping from t1, t2, gain, alpha"), ({"t3": 1}, "mapping from t1, t2, gain, alpha"), ({"t1": "x"}, "`servo` t1 of 'x' is not a number"), ({"gain": True}, "is not a number"), ({"t1": 0.0019}, "outside the manual's range"),
                                ({"t2": 0.02}, "outside the manual's range"), ({"t2": 0.2}, "outside the manual's range"), ({"gain": 0}, "outside the manual's range"), ({"alpha": 0}, "outside the manual's range"), ({"alpha": 1}, "outside the manual's range"),
                                ({"t2": 0.025, "t1": 0.01}, "not longer than the control period"), ({"t2": 0.03}, "not longer than the control period"), ({"t2": 0.05}, "")):
            bridge = RainbowBridge()
            said = io.StringIO()
            with contextlib.redirect_stderr(said):
                code = bridge.open(json.dumps({**base, "servo": given, "rate_hz": 25}))
            bridge.close()
            if fragment == "":
                check(code == rhal.SKR_OK, f"servo of {given!r} at 25 Hz is accepted: its t2 spans the 40 ms between two commands", said.getvalue())
            else:
                check(code == rhal.SKR_ENODEV and fragment in said.getvalue(), f"servo of {given!r} at 25 Hz is refused, saying why", said.getvalue())
        for rate, t2 in ((100.0, 0.03), (50.0, 0.04), (25.0, 0.08), (10.0, 0.19)):
            bridge = RainbowBridge()
            bridge._rate_hz = rate  # noqa: SLF001
            servo = bridge._read_servo(None)  # noqa: SLF001
            check(servo["t1"] == 1.0 / rate and abs(servo["t2"] - t2) < 1e-12 and servo["gain"] == 0.5 and servo["alpha"] == 0.5, f"at {rate:g} Hz the defaults are t1 the period ({servo['t1']}), t2 {servo['t2']}, and the maker's own ROS 2 package's gain and alpha of 0.5")
        bridge = RainbowBridge()
        bridge._rate_hz = 5.0  # noqa: SLF001
        message = raises(bridge_module.BridgeRefusal, bridge._read_servo, None) or ""  # noqa: SLF001
        check("not longer than the control period" in message and "0.2000" in message, "a rate so low that no t2 the manual allows spans the period is refused, saying so", message)
        bridge._rate_hz = 1000.0  # noqa: SLF001
        check(bridge._read_servo(None)["t1"] == frames.MIN_T1, "and a period shorter than the manual's least t1 is t1 of 0.002")  # noqa: SLF001
        sim.commands.clear()
        bridge = opened(sim, rate_hz=50, servo={"t1": 0.02, "t2": 0.05, "gain": 2.0, "alpha": 0.25})
        check(ready(bridge), "ready")
        code, _ = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 1) and servo_texts(sim)[0].endswith("],0.02,0.05,2,0.25)"), "the parameters the operator gave are the ones sent, in the order t1, t2, gain, alpha", str(servo_texts(sim)))
        bridge.close()
    finally:
        sim.stop()

    section("open — the controller must answer, be in the mode the operator said, and be a Rainbow control box")
    sim = new_sim()
    try:
        began = time.monotonic()
        port = sim.data_port
        sim.stop()
        code, said, bridge = opens_with(sim, data_port=port, open_timeout_ms=400)
        took = time.monotonic() - began
        check(code == rhal.SKR_EIO and "data port could not be opened" in said and "Is the control box powered" in said and took < 3.0, f"a control box whose data port nobody listens on is EIO, asking whether it is powered and whether that is its data port ({took:.1f} s)", said)
    finally:
        sim.stop()
    sim = new_sim()
    try:
        sim.data_stalled = True
        code, said, bridge = opens_with(sim, open_timeout_ms=300)
        check(code == rhal.SKR_EIO and "no state packet arrived" in said and "Is the control box powered" in said, "a control box that accepts and never answers `reqdata` is EIO, and the message asks whether it is powered and whether that is its data port", said)
        sim.data_stalled = False
        sim.data_garbage = True
        sim.data_extra_packet = True
        code, said, bridge = opens_with(sim)
        bridge.close()
        check(code == rhal.SKR_OK, "noise and a packet of another type in front of the state are not a reason to refuse it")
        sim.data_garbage = sim.data_extra_packet = False
        silent = threading.Event()
        port, _ = tcp_server(lambda connection: (connection.settimeout(3.0), connection.recv(1024), silent.wait(3.0)))
        code, said, bridge = opens_with(sim, port=port, open_timeout_ms=500)
        silent.set()
        check(code == rhal.SKR_EIO and "did not answer" in said and "command port" in said, "a command port that takes the line and never answers get_control_box_info() is EIO: it is not a Rainbow control box's", said)
        port = sim.command_port
        sim.drop_connections()
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        closed_port = listener.getsockname()[1]
        listener.close()
        code, said, bridge = opens_with(sim, port=closed_port, open_timeout_ms=300)
        check(code == rhal.SKR_EIO and "could not be opened" in said, "a command port nobody listens on is EIO, saying it could not be opened", said)
        port, _ = tcp_server(lambda connection: (connection.settimeout(3.0), connection.recv(1024), connection.sendall(b"The command was executed\ninfo[controlbox][abc,0]\n"), time.sleep(0.5)))
        code, said, bridge = opens_with(sim, port=port, open_timeout_ms=500)
        check(code == rhal.SKR_ENODEV and "is not its software version" in said, "a control box that answers with something that is not a software version is refused", said)
        sim.commands.clear()
        sim.simulation = False
        code, said, bridge = opens_with(sim, simulator=True)
        check(code == rhal.SKR_ENODEV and "is in real mode and `simulator` is true" in said and "would move an arm" in said and sim.commands == [], "a controller in its real mode is not a simulation, and an open that says it is is refused, with nothing written", said)
        sim.simulation = True
        code, said, bridge = opens_with(sim, simulator=False)
        check(code == rhal.SKR_ENODEV and "is in simulation mode and `simulator` is false" in said and "moves nothing" in said and sim.commands == [], "and a controller in its simulation mode moves nothing, so an open that says it is real is refused, with nothing written", said)
        sim.mode_override = 7
        code, said, bridge = opens_with(sim, simulator=True)
        check(code == rhal.SKR_ENODEV and "mode 7, which the document does not list" in said, "a mode that is neither is not guessed at", said)
        sim.mode_override = None
    finally:
        sim.stop()

    section("open — the descriptor, and what an open writes")
    sim = new_sim()
    try:
        bridge = RainbowBridge()
        check(bridge.describe() == "{}", "a bridge that has not been opened describes nothing")
        code, said, bridge = opens_with(sim, model="RB10-1300E", roles={"j2": "right_arm.shoulder.pitch", "j6": "right_arm.wrist.roll"})
        check(code == rhal.SKR_OK and said == "" and sim.commands == ["get_control_box_info()"], "an open writes one line, the control box's own information request, and says nothing", f"{code} {said!r} {sim.commands}")
        descriptor = json.loads(bridge.describe())
        check(descriptor["robot_slug"] == "rainbow-rb10-1300e" and descriptor["simulator"] is True and descriptor["base"] == "fixed" and descriptor["dof"] == 6 and descriptor["verified"] is False and descriptor["digest"] is None,
              "the descriptor says which arm, that it is a simulation (the operator's claim, checked against the controller's mode), a fixed base, six degrees of freedom, and not verified")
        joints = descriptor["joints"]
        check([j["name"] for j in joints] == ["j1", "j2", "j3", "j4", "j5", "j6"] and all(j["type"] == "revolute" and j["control_modes"] == ["position"] for j in joints), "six revolute joints in the controller's order, each taking a position")
        check([j["limits"]["position_min_rad"] for j in joints] == [r[0] for r in LIMITS] and [j["limits"]["position_max_rad"] for j in joints] == [r[1] for r in LIMITS], "each with the range the operator gave")
        check([j["role"] for j in joints] == [None, "right_arm.shoulder.pitch", None, None, None, "right_arm.wrist.roll"], "and the roles the operator asserted, where they asserted one")
        check(descriptor["control"] == {"modes": ["position"], "rate_hz_max": 100.0, "rate_hz_min": 1.0} and descriptor["envelope"] is None and descriptor["capabilities"] == ["safety.estop"] and descriptor["extensions"] == [TOKEN] and descriptor["solvers"] == [],
              "the control block is position at the configured rate, no envelope is claimed, the one capability is the software stop, and the one extension is this driver's")
        check([m["name"] for m in descriptor["motions"]] == ["status", "set_mode", "activate", "resume", "clear_faults", "set_payload", "move"], "seven motions are declared")
        by_name = {m["name"]: m["parameters"] for m in descriptor["motions"]}
        check(by_name["status"] == [] and by_name["clear_faults"] == [] and by_name["set_mode"] == [{"name": "mode", "unit": "real|simulation", "required": True}] and by_name["resume"] == [{"name": "after", "unit": "pause|collision", "required": False}] and
              by_name["activate"] == [{"name": "timeout", "unit": "s", "required": False}] and [p["name"] for p in by_name["set_payload"]] == ["kg", "x", "y", "z"] and by_name["set_payload"][0]["required"] is True,
              "and each says what it takes")
        move = by_name["move"]
        check([p["name"] for p in move] == ["j1", "j2", "j3", "j4", "j5", "j6", "speed", "accel", "timeout"] and all(p["unit"] == "rad" for p in move[:6]) and [p["unit"] for p in move[6:]] == ["deg/s", "deg/s^2", "s"] and not any(p["required"] for p in move),
              "a move's joints are in radians, its speed and acceleration are the controller's degrees, and its timeout is in seconds")
        bridge.close()
        code, said, bridge = opens_with(sim, simulator=False)
        check(code == rhal.SKR_ENODEV, "(a controller in its simulation mode is not a real arm)")
        sim.simulation = False
        code, said, bridge = opens_with(sim, simulator=False)
        check(code == rhal.SKR_OK and json.loads(bridge.describe())["simulator"] is False, "a real controller's descriptor says it is not a simulation")
        bridge.close()
        sim.simulation = True
        bridge = opened(sim)
        first = bridge
        check(ready(first), "ready")
        again, said = said_by(first.open, config(sim))
        check(again == rhal.SKR_OK and ready(first), "an open on an open driver is a new session: the first one's thread and lines are let go and the arm is read again", said)
        first.close()
        code, said = refused(json.dumps({**settings(sim), "limits": [[-1, 1]] * 6}))
        check(code == rhal.SKR_OK and said == "", "an open and a close in a row leave a controller that can be opened again", said)
    finally:
        sim.stop()

def main_state() -> None:
    section("state — what is read, in which units, and what each flag means")
    bridge = RainbowBridge()
    out = rhal.State()
    check(bridge.read_state(out, 0) == rhal.SKR_EPERM, "a bridge that has not been opened is not read: EPERM")
    check(bridge.read_sensor("anything", rhal.Frame()) == rhal.SKR_ENOSYS, "and it has no sensors: ENOSYS")
    check(bridge.emergency_stop() == rhal.SKR_OK and bridge._estop, "a stop on a bridge that has not been opened still latches, and returns", str(bridge._estop))  # noqa: SLF001
    check(bridge.release_stop() == rhal.SKR_OK and not bridge._estop, "and with no line to have seen it, the release clears the latch (a session that was never opened has nothing to wait for)")  # noqa: SLF001

    bridge = lifeless_bridge()
    with bridge._lock:  # noqa: SLF001
        bridge._latest = None
    code, out = state(bridge)
    check(code == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED and all(math.isnan(v) for v in out.position_rad) and out.fault_code == 0, "an arm that has not been read is ETIMEDOUT and degraded, with no position claimed", f"{code} {out.flags}")
    check(all(math.isnan(v) for v in out.velocity_rad_s) and all(math.isnan(v) for v in out.effort_nm), "and the controller sends no velocity and no torque, so none is claimed")

    reading = set_reading(bridge, ref=(10.0, -20.0, 30.0, 40.0, 50.0, 60.0))
    code, out = state(bridge)
    check(code == rhal.SKR_OK and out.flags == rhal.HOMING_NOT_REQUIRED and out.fault_code == 0, "a healthy arm reads OK, with no flag but 'there is nothing to home' (the encoders are absolute)", f"{code} {out.flags}")
    check(all(abs(a - b * DEG) < 1e-6 for a, b in zip(out.position_rad, reading.ref)), "the positions are the controller's degrees in radians", str(list(out.position_rad)))
    check(all(math.isnan(v) for v in out.velocity_rad_s) and all(math.isnan(v) for v in out.effort_nm), "and no velocity and no torque are claimed: the state packet has neither")
    seq = out.seq
    code, again = state(bridge)
    check(again.seq == seq + 1 and again.stamp_ns == out.stamp_ns, "a second read is the next sequence number, with the first packet's stamp: the stamp is when the packet arrived, not when it was asked for", f"{seq} {again.seq}")
    with bridge._lock:  # noqa: SLF001
        stamp = bridge._stamp_ns  # noqa: SLF001
    check(out.stamp_ns == stamp, "the stamp is the one taken when the packet was read")

    section("state — the simulation mode reads the reference, the real mode the encoders")
    both = {"ref": (1.0, 2.0, 3.0, 4.0, 5.0, 6.0), "ang": (11.0, 12.0, 13.0, 14.0, 15.0, 16.0)}
    bridge = lifeless_bridge()
    set_reading(bridge, mode=frames.MODE_SIMULATION, **both)
    code, out = state(bridge)
    check(code == rhal.SKR_OK and abs(out.position_rad[0] - 1.0 * DEG) < 1e-6 and abs(out.position_rad[5] - 6.0 * DEG) < 1e-6, "in the controller's simulation mode the position is the reference, which is what the arm would do (`jnt_ang` does not change there)", str(list(out.position_rad)))
    bridge = lifeless_bridge()
    bridge._simulated = False  # noqa: SLF001
    set_reading(bridge, mode=frames.MODE_REAL, init_state=frames.ACTIVATED, **both)
    code, out = state(bridge)
    check(code == rhal.SKR_OK and out.flags == rhal.HOMING_NOT_REQUIRED and abs(out.position_rad[0] - 11.0 * DEG) < 1e-6 and abs(out.position_rad[5] - 16.0 * DEG) < 1e-6, "in the real mode it is the encoders' angle, an activated arm with nothing wrong reads OK", f"{list(out.position_rad)} {out.flags}")

    section("state — an arm that takes no command is degraded, and says why in `status`; it holds no fault")
    cases = [
        ("paused", dict(task_state=frames.TASK_PAUSED), True, "paused"),
        ("a pause flag", dict(paused_flag=1), True, "paused"),
        ("a running task", dict(task_state=frames.TASK_RUNNING), True, None),
        ("not activated in the real mode", dict(mode=frames.MODE_REAL, init_state=3), False, "not activated"),
        ("activated in the real mode", dict(mode=frames.MODE_REAL, init_state=frames.ACTIVATED), False, None),
        ("the real mode when the operator said simulation", dict(mode=frames.MODE_REAL, init_state=frames.ACTIVATED), True, "is in real mode and `simulator` is true"),
        ("the simulation mode when the operator said real", dict(mode=frames.MODE_SIMULATION), False, "is in simulation mode and `simulator` is false"),
    ]
    for what, fields, simulated, reason in cases:
        bridge = lifeless_bridge()
        bridge._simulated = simulated  # noqa: SLF001
        set_reading(bridge, **fields)
        code, out = state(bridge)
        _, body = ext(bridge, "status")
        shown = body.get("not_commandable") if isinstance(body, dict) else None
        if reason is None:
            check(code == rhal.SKR_OK and not out.flags & (rhal.FLAG_DEGRADED | rhal.FLAG_FAULT | rhal.FLAG_ESTOP) and shown is None, f"{what}: nothing is wrong", f"{out.flags} {shown}")
        else:
            check(code == rhal.SKR_OK and out.flags & rhal.FLAG_DEGRADED and not out.flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP) and out.fault_code == 0 and reason in (shown or ""),
                  f"{what}: degraded, with no fault and no fault code, and `status` says why ({reason})", f"{out.flags} {shown}")

    section("state — what the controller reports is a fault, a stop, or neither, in a fixed order")
    quiet = frames.decode_state(packet_of())
    band = bridge_module
    table = [
        ("a device error", dict(sos=3), [], "fault", band.SOS_BAND | 3),
        ("a software emergency stop", dict(ems=2), [], "fault", band.EMS_BAND | 2),
        ("a collision", dict(collision=1), [], "fault", band.COLLISION_BAND | 1),
        ("a collision of the second kind", dict(collision=2), [], "fault", band.COLLISION_BAND | 2),
        ("a joint's JAM bit on j3", dict(joint_info=(0b110, 0b110, 0b110 | 1 << 8, 0b110, 0b110, 0b110)), [], "fault", band.JOINT_BAND | 2 << 8 | 8),
        ("a joint's PS2 bit on j6", dict(joint_info=(0b110,) * 5 + (0b110 | 1 << 15,)), [], "fault", band.JOINT_BAND | 5 << 8 | 15),
        ("a joint's encoder bit 4 on j1", dict(joint_info=(0b110 | 1 << 4,) + (0b110,) * 5), [], "fault", band.JOINT_BAND | 4),
        ("a mode the document does not list", dict(mode=7), [], "fault", band.MODE_BAND | 7),
        ("the SOS state", dict(chunk1=1 << 12), [], "fault", band.SOS_STATE),
        ("the control box's stop-chain error 253", {}, [253], "estop", 253),
        ("the GP switch 60", {}, [60], "estop", 60),
        ("a safety switch having powered the arm off, 488", {}, [488], "estop", 488),
        ("a code that is not about the stop chain", {}, [5], "fault", 5),
        ("a code at the edge of the table, 605", {}, [605], "fault", 605),
        ("a stop after a fault: the stop first", {}, [5, 253], "estop", 253),
        ("a fault after a stop: the stop first", {}, [253, 5], "estop", 253),
        ("two faults: the first", {}, [6, 5], "fault", 6),
        ("an error and a device error: the error", dict(sos=3), [5], "fault", 5),
        ("a device error and a software stop: the device error", dict(sos=3, ems=2), [], "fault", band.SOS_BAND | 3),
        ("a software stop and a collision: the stop", dict(ems=2, collision=1), [], "fault", band.EMS_BAND | 2),
        ("a collision and a joint bit: the collision", dict(collision=1, joint_info=(0b110 | 1 << 8,) + (0b110,) * 5), [], "fault", band.COLLISION_BAND | 1),
        ("a joint bit and a bad mode: the joint", dict(mode=7, joint_info=(0b110 | 1 << 8,) + (0b110,) * 5), [], "fault", band.JOINT_BAND | 8),
        ("a bad mode and the SOS state: the mode", dict(mode=7, chunk1=1 << 12), [], "fault", band.MODE_BAND | 7),
    ]
    for what, fields, errors, kind, word in table:
        verdict = band._classify(frames.decode_state(packet_of(**fields)), errors)  # noqa: SLF001
        check(verdict == (kind, word), f"{what} is {kind} {word}", str(verdict))
    check(band._classify(quiet, []) is None, "an arm that reports nothing holds no fault")  # noqa: SLF001
    for what, fields in (("the joint words the simulator sends", dict(joint_info=(0b110,) * 6)), ("the low bits of a joint word, which are not errors", dict(joint_info=(0b1111,) * 6)),
                         ("the real mode", dict(mode=frames.MODE_REAL)), ("a pause", dict(task_state=frames.TASK_PAUSED, paused_flag=1)), ("the power bit and the stop-state bits of the information word", dict(chunk1=(1 << 6) | (1 << 3) | (1 << 5)))):
        check(band._classify(frames.decode_state(packet_of(**fields)), []) is None, f"{what}: not a fault")  # noqa: SLF001

    for what, fields, errors, estop in (("a stop-chain code", {}, [253], True), ("another code", {}, [5], False), ("a device error", dict(sos=3), [], False)):
        bridge = lifeless_bridge()
        set_reading(bridge, errors=errors, **fields)
        code, out = state(bridge)
        want = rhal.FLAG_ESTOP if estop else rhal.FLAG_FAULT
        other = rhal.FLAG_FAULT if estop else rhal.FLAG_ESTOP
        check(code == rhal.SKR_OK and out.flags & want and out.flags & rhal.FLAG_DEGRADED and not out.flags & other and out.fault_code == (errors[0] if errors else band.SOS_BAND | 3),
              f"{what}: {'the emergency-stop flag' if estop else 'the fault flag'} and degraded, and the fault word", f"{out.flags} {out.fault_code}")
    bridge = lifeless_bridge()
    bridge._estop = True  # noqa: SLF001
    code, out = state(bridge)
    check(code == rhal.SKR_OK and out.flags & rhal.FLAG_ESTOP and out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_FAULT and out.fault_code == 0, "this session's own stop latch is the emergency-stop flag and degraded, with no fault word: nothing is wrong with the arm", f"{out.flags}")

    section("state — a warning degrades the state for a while, once, and a fault outranks it")
    now = time.monotonic()
    for what, warns, degraded, word in (("a fresh warning", [(1, now, 562)], True, 562), ("one a second old", [(1, now - 1.0, 568)], True, 568), ("one that is old", [(1, now - 3.0, 562)], False, 0),
                                        ("two fresh: the later", [(1, now, 562), (2, now, 568)], True, 568), ("an old one and a fresh one", [(1, now - 3.0, 562), (2, now, 568)], True, 568), ("none", [], False, 0)):
        bridge = lifeless_bridge()
        with bridge._lock:  # noqa: SLF001
            bridge._warns, bridge._warn_seq = list(warns), len(warns)  # noqa: SLF001
        code, out = state(bridge)
        check(code == rhal.SKR_OK and bool(out.flags & rhal.FLAG_DEGRADED) == degraded and out.fault_code == word and not out.flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP), f"{what}: {'degraded, and the code of the warning' if degraded else 'not degraded'}", f"{out.flags} {out.fault_code}")
    bridge = lifeless_bridge()
    with bridge._lock:  # noqa: SLF001
        bridge._warns, bridge._warn_seq = [(1, now, 562)], 1  # noqa: SLF001
    set_reading(bridge, sos=3)
    with bridge._lock:  # noqa: SLF001
        bridge._warns, bridge._warn_seq = [(1, time.monotonic(), 562)], 1  # noqa: SLF001
    code, out = state(bridge)
    check(out.flags & rhal.FLAG_FAULT and out.fault_code == band.SOS_BAND | 3, "a fault outranks a warning: the fault's word is the one reported", f"{out.fault_code}")
    bridge = lifeless_bridge()
    set_reading(bridge, task_state=frames.TASK_PAUSED)
    with bridge._lock:  # noqa: SLF001
        bridge._warns, bridge._warn_seq = [(1, time.monotonic(), 562)], 1  # noqa: SLF001
    code, out = state(bridge)
    check(out.flags & rhal.FLAG_DEGRADED and out.fault_code == 0, "and so does a pause: a warning is not told over it")

    section("state — a reading that is stale, a link that is dead, a controller that has stopped acknowledging")
    bridge = lifeless_bridge()
    with bridge._lock:  # noqa: SLF001
        bridge._stamp_ns = rhal.acquisition_ns() - int(0.8e9)  # noqa: SLF001
    code, out = state(bridge)
    check(code == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED and not math.isnan(out.position_rad[0]), "a reading 0.8 s old, when 0.5 s is stale, is ETIMEDOUT and degraded, with the last position still given", f"{code} {out.flags}")
    bridge = lifeless_bridge()
    with bridge._lock:  # noqa: SLF001
        bridge._stamp_ns = rhal.acquisition_ns() - int(0.3e9)  # noqa: SLF001
    code, out = state(bridge)
    check(code == rhal.SKR_OK and not out.flags & rhal.FLAG_DEGRADED, "and one 0.3 s old is not stale", f"{code} {out.flags}")
    bridge = lifeless_bridge()
    bridge._stale_s = 0.1  # noqa: SLF001
    with bridge._lock:  # noqa: SLF001
        bridge._stamp_ns = rhal.acquisition_ns() - int(0.2e9)  # noqa: SLF001
    check(state(bridge)[0] == rhal.SKR_ETIMEDOUT, "the limit is the configured one (`stale_ms`)")
    bridge = lifeless_bridge()
    with bridge._lock:  # noqa: SLF001
        bridge._dead = "the data port reset"  # noqa: SLF001
    code, out = state(bridge)
    check(code == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED, "a link the reading thread has given up on is ETIMEDOUT and degraded", f"{code} {out.flags}")
    bridge = lifeless_bridge()
    with bridge._lock:  # noqa: SLF001
        bridge._pending.append(time.monotonic() - 1.6)  # noqa: SLF001
    code, out = state(bridge)
    check(code == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED, "a command that has waited 1.6 s for its acknowledgement: the controller is hung, ETIMEDOUT and degraded", f"{code} {out.flags}")
    bridge = lifeless_bridge()
    with bridge._lock:  # noqa: SLF001
        bridge._pending.append(time.monotonic() - 0.4)  # noqa: SLF001
    code, out = state(bridge)
    check(code == rhal.SKR_OK and not out.flags & rhal.FLAG_DEGRADED, "and one that has waited 0.4 s is a backlog, not a hang", f"{code} {out.flags}")

    section("state — against the simulated controller")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, out = state(bridge)
        check(code == rhal.SKR_OK and out.flags == rhal.HOMING_NOT_REQUIRED and all(abs(v) < 1e-6 for v in out.position_rad), "an arm at rest reads OK, at zero", f"{code} {out.flags} {list(out.position_rad)}")
        with sim._lock:  # noqa: SLF001
            sim.ref = [30.0, -45.0, 10.0, 0.0, 90.0, -90.0]
        check(until(lambda: abs(state(bridge)[1].position_rad[0] - 30.0 * DEG) < 1e-3 and abs(state(bridge)[1].position_rad[5] + 90.0 * DEG) < 1e-3), "a pose the controller reports is read, in radians", str(list(state(bridge)[1].position_rad)))
        first = state(bridge)[1].stamp_ns
        check(until(lambda: state(bridge)[1].stamp_ns > first), "and the stamp advances with each packet")
        sim.data_stalled = True
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and state(bridge)[1].flags & rhal.FLAG_DEGRADED, 6.0), "a data port that stops answering makes the state ETIMEDOUT and degraded")
        sim.data_stalled = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 6.0), "and when it answers again the state is healthy again, with no restart")
        sim.drop_connections("data")
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 8.0), "a data connection the controller hangs up is opened again, and the arm is read again")
        before = len(sim.seen("get_control_box_info"))
        sim.drop_connections("command")
        check(until(lambda: bool(write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK), 8.0), "a command connection the controller hangs up is opened again, and a command is taken after it")
        del before
        # A write that the old, hung-up connection took into its buffer is also "a command taken": the controller says a line to the connections it has, so it waits for the driver's new one before it speaks.
        until(lambda: _command_connections(sim) == 1, 6.0)

        sim.raise_error(253)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_ESTOP and state(bridge)[1].fault_code == 253), "an `error[code][253]` the controller says is the emergency-stop flag with its code", str(state(bridge)[1].flags))
        check(not state(bridge)[1].flags & rhal.FLAG_FAULT, "and not the fault flag")
        sim.raise_error(5)
        check(until(lambda: _errors_of(bridge) == [253, 5]), "a second code is remembered after the first, in the order said")
        _, body = ext(bridge, "status")
        check(isinstance(body, dict) and body["fault"] == {"class": "estop", "code": 253, "text": bridge_module._say_code(253)} and [e["code"] for e in body["errors"]] == [253, 5], "`status` gives the first, and lists every code with the maker's words", str(body)[:200])  # noqa: SLF001

        # The contract (`faultvocab.py`) injects all 606 codes, and takes minutes; these are the edges of the table's classes, so that this suite holds them by itself (the seeded-defect campaign runs the contract on the packet's fields only).
        def says(code: int, stop: bool) -> bool:
            until(lambda: ext(bridge, "clear_faults")[0] == rhal.SKR_OK and not state(bridge)[1].flags & (rhal.FLAG_ESTOP | rhal.FLAG_FAULT) and not _errors_of(bridge))
            sim.raise_error(code)
            raised, other = (rhal.FLAG_ESTOP, rhal.FLAG_FAULT) if stop else (rhal.FLAG_FAULT, rhal.FLAG_ESTOP)
            return until(lambda: bool(state(bridge)[1].flags & raised) and state(bridge)[1].fault_code == code) and bool(state(bridge)[1].flags & rhal.FLAG_DEGRADED) and not state(bridge)[1].flags & other

        stops = {code: says(code, True) for code in sorted(frames.ESTOP_CODES)}
        check(all(stops.values()), "each of the eleven codes the maker's words make about the stop chain is the emergency-stop flag, with its code and degraded", str([c for c, ok in stops.items() if not ok]))
        faults = {code: says(code, False) for code in (0, 3, 5, 59, 61, 252, 259, 484, 489, 605, 606, 65535)}
        check(all(faults.values()), "and the codes beside them, a code with a stop-like neighbour and a code the table does not list are the fault flag, with their codes and degraded", str([c for c, ok in faults.items() if not ok]))
        bridge.close()
    finally:
        sim.stop()

    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.raise_error(5)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT and state(bridge)[1].fault_code == 5), "a code that is not about the stop chain is the fault flag with its code")
        check(not state(bridge)[1].flags & rhal.FLAG_ESTOP, "and not the emergency-stop flag")
        bridge.close()
        sim.set_mode_state(simulation=False, activated=True)
        sim.joint_info = [0b110, 0b110, 0b110 | 1 << 9, 0b110, 0b110, 0b110]
        bridge = opened(sim, simulator=False)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT and state(bridge)[1].fault_code == (band.JOINT_BAND | 2 << 8 | 9), 4.0), "a joint's error bit in the packet is the fault flag with that joint and bit in its word", str(state(bridge)[1].fault_code))
        sim.joint_info = [0b110] * 6
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_FAULT), "and it ends when the packet no longer carries it")
        sim.sos = 4
        check(until(lambda: state(bridge)[1].fault_code == (band.SOS_BAND | 4)), "a device error in the packet is a fault with its number")
        sim.sos = 0
        sim.collision = 1
        check(until(lambda: state(bridge)[1].fault_code == (band.COLLISION_BAND | 1)), "and a collision")
        sim.collision = 0
        sim.mode_override = 9
        check(until(lambda: state(bridge)[1].fault_code == (band.MODE_BAND | 9)), "and a mode the document does not list")
        sim.mode_override = None
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_FAULT), "and they end when the controller says they have")
        sim.raise_warn(562)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED and state(bridge)[1].fault_code == 562), "a warning the controller says degrades the state, with its code")
        bridge.close()
    finally:
        sim.stop()


def _errors_of(bridge: RainbowBridge) -> list[int]:
    with bridge._lock:  # noqa: SLF001
        return list(bridge._errors)  # noqa: SLF001

def angles_of(text: str) -> list[float]:
    """The six joint angles (degrees) of a `move_servo_j` or `move_j` line, read back from its text."""
    inside = text[text.index("jnt[") + 4:text.index("]")]
    return [float(v) for v in inside.split(",")]


def parameters_of(text: str) -> list[float]:
    return [float(v) for v in text[text.index("]") + 2:-1].split(",")]


def quiet_write(bridge: RainbowBridge, command: rhal.Command) -> tuple[int, str]:
    """A write without the helper's reset of the said-once memory: for what repeats."""
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def main_commands() -> None:
    section("commands — what is refused before anything is sent")
    bridge = RainbowBridge()
    check(write(bridge, cmd(pose(0.0)))[0] == rhal.SKR_EPERM, "a bridge that is not open takes no command: EPERM")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        before = sim.servo_commands
        code, said = write(bridge, cmd([0.0] * 5))
        check(code == rhal.SKR_EINVAL and "5 joints" in said and "six (j1, j2, j3, j4, j5, j6)" in said and "nothing was sent" in said, "a command of five joints is invalid, saying how many and which six", said)
        code, said = write(bridge, cmd([0.0] * 7))
        check(code == rhal.SKR_EINVAL and "7 joints" in said, "and one of seven", said)
        for mode in (rhal.MODE_VELOCITY, rhal.MODE_TORQUE, 7):
            code, said = write(bridge, cmd(pose(0.0), mode=mode))
            check(code == rhal.SKR_EINVAL and f"mode {mode}" in said and "takes positions" in said and "no velocity or torque route" in said, f"a command in mode {mode} is invalid: this arm takes positions", said)
        for value in (math.nan, math.inf, -math.inf):
            values = pose(0.0)
            values[3] = value
            code, said = write(bridge, cmd(values))
            check(code == rhal.SKR_EINVAL and "j4" in said and "not finite" in said and "refused whole" in said, f"{value} for j4 is refused, naming the joint and saying the whole command is refused", said)
        for joint, value in ((0, 6.001), (0, -6.001), (1, 3.001), (1, -3.001), (2, 2.51), (2, -2.51), (5, 6.5)):
            values = pose(0.0)
            values[joint] = value
            code, said = write(bridge, cmd(values))
            low, high = LIMITS[joint]
            check(code == rhal.SKR_EINVAL and f"j{joint + 1}" in said and f"{value:.6f}" in said and f"[{low:.6f}, {high:.6f}]" in said and "refused whole and nothing was sent" in said, f"j{joint + 1} at {value} is outside {LIMITS[joint]}: refused whole, naming the joint, the value and the range", said)
        values = pose(0.1)
        values[4] = 9.0
        check(write(bridge, cmd(values))[0] == rhal.SKR_EINVAL, "a command with one joint out of range and five in is refused whole")
        masked = pose(0.1)
        masked[1], masked[3] = math.nan, 99.0
        code, _ = write(bridge, cmd(masked, mask=[1, 0, 1, 0, 1, 1]))
        check(code == rhal.SKR_OK, "a value that is not applied is not checked: NaN and 99 on joints the mask leaves out do not refuse the command")
        check(until(lambda: len(servo_texts(sim)) == 1), "(it was sent)")
        check(servo_texts(sim)[0].count("nan") == 0 and "99" not in servo_texts(sim)[0], "and what was sent holds neither", servo_texts(sim)[0])
        check(sim.servo_commands == before + 1, "of everything above only that one reached the controller", f"{sim.servo_commands} {before}")
        code, _ = write(bridge, cmd(pose(0.2), mask=[0] * 6))
        check(code == rhal.SKR_OK and sim.servo_commands == before + 1, "a command that applies no joint is OK and writes nothing")
        bridge.close()
    finally:
        sim.stop()

    section("commands — what is written")
    sim = new_sim()
    try:
        bridge = opened(sim, servo={"t1": 0.02, "t2": 0.05, "gain": 2.0, "alpha": 0.25})
        check(ready(bridge), "ready")
        target = [0.1, -0.2, 0.3, -0.4, 0.5, -0.6]
        check(write(bridge, cmd(target))[0] == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 1), "a command is one `move_servo_j`")
        text = servo_texts(sim)[0]
        check(text.startswith("move_servo_j(jnt[") and text.endswith("],0.02,0.05,2,0.25)"), "of the six angles, then t1, t2, gain and alpha, as the operator said", text)
        check(all(abs(a - b / DEG) < 1e-4 for a, b in zip(angles_of(text), target)), "the angles are the radians as degrees", text)
        check(until(lambda: all(abs(a - b / DEG) < 0.01 for a, b in zip(sim.joints_deg, target)), 5.0), "and the arm goes there", str(sim.joints_deg))
        check(until(lambda: all(abs(a - b) < 1e-3 for a, b in zip(state(bridge)[1].position_rad, target))), "and is read there", str(list(state(bridge)[1].position_rad)))
        code, _ = write(bridge, cmd(pose(3.0), mask=[0, 1, 0, 0, 0, 0]))
        check(code == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 2), "j2 at its upper limit, 3.0, is accepted")
        check(abs(angles_of(servo_texts(sim)[1])[1] - 3.0 / DEG) < 1e-3, "and sent as 171.89 degrees", servo_texts(sim)[1])
        code, _ = write(bridge, cmd(pose(3.0 + 5e-10), mask=[0, 1, 0, 0, 0, 0]))
        check(code == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 3) and abs(angles_of(servo_texts(sim)[2])[1] - 3.0 / DEG) < 1e-4, "a value a rounding error past the limit is accepted, and clamped to it")
        code, _ = write(bridge, cmd(pose(-6.0), mask=[1, 0, 0, 0, 0, 0]))
        check(code == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 4) and abs(angles_of(servo_texts(sim)[3])[0] + 6.0 / DEG) < 1e-3, "and a lower limit")
        bridge.close()
    finally:
        sim.stop()

    section("commands — a joint the command leaves out is held where the arm was asked to be")
    sim = new_sim()
    try:
        with sim._lock:  # noqa: SLF001
            sim.ref = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0]
        bridge = opened(sim)
        check(ready(bridge) and until(lambda: abs(state(bridge)[1].position_rad[2] - 30.0 * DEG) < 1e-4), "ready, with the arm at 10 20 30 40 50 60 degrees")
        code, _ = write(bridge, cmd(pose(0.5), mask=[1, 0, 0, 0, 0, 0]))
        check(code == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 1), "j1 alone")
        first = angles_of(servo_texts(sim)[0])
        check(abs(first[0] - 0.5 / DEG) < 1e-4 and all(abs(a - b) < 1e-3 for a, b in zip(first[1:], [20.0, 30.0, 40.0, 50.0, 60.0])), "goes to its target and the other five are asked to stay where they are", str(first))
        check(until(lambda: abs(sim.joints_deg[0] - 0.5 / DEG) < 0.05), "(j1 gets there)")
        code, _ = write(bridge, cmd(pose(0.25), mask=[0, 0, 1, 0, 0, 0]))
        check(code == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 2), "then j3 alone")
        second = angles_of(servo_texts(sim)[1])
        check(abs(second[0] - 0.5 / DEG) < 1e-3 and abs(second[2] - 0.25 / DEG) < 1e-3 and abs(second[1] - 20.0) < 1e-3 and abs(second[5] - 60.0) < 1e-3, "keeps j1 at what it was last asked for, not where it was read", str(second))
        bridge.close()
        with sim._lock:  # noqa: SLF001
            sim.ref = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0]
        bridge = opened(sim)
        check(ready(bridge), "ready")
        write(bridge, cmd(pose(0.5), mask=[1, 0, 0, 0, 0, 0]))
        check(until(lambda: len(servo_texts(sim)) == 3), "a new session")
        check(abs(angles_of(servo_texts(sim)[2])[1] - 20.0) < 1e-3, "starts again from where the arm is", servo_texts(sim)[2])
        bridge.close()
    finally:
        sim.stop()

    section("commands — a stream")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        codes = [write(bridge, cmd(pose(0.001 * (n + 1)), mask=[1, 0, 0, 0, 0, 0]))[0] for n in range(60)]
        check(all(c == rhal.SKR_OK for c in codes), "sixty commands are taken")
        check(until(lambda: len(servo_texts(sim)) == 60, 8.0), "and all sixty reach the controller")
        sent = [angles_of(t)[0] for t in servo_texts(sim)]
        check(all(a < b for a, b in zip(sent, sent[1:])) and abs(sent[-1] - 0.06 / DEG) < 1e-3, "in the order they were given", str(sent[-3:]))
        check(until(lambda: _acked(bridge) == 60, 4.0), "and each is acknowledged", str(_acked(bridge)))
        bridge.close()
    finally:
        sim.stop()

    section("commands — the gate: an arm that is not reading, not mode-right, not active, paused, faulted or hung takes nothing")
    for what, fields, errors, dead, lag, code_wanted, fragments in (
        ("an arm whose link is dead", {}, [], "the data port reset", 0.0, rhal.SKR_EIO, ("not being read", "the data port reset", "not sent")),
        ("a controller that has not acknowledged for 1.5 s", {}, [], None, 1.5, rhal.SKR_EIO, ("not acknowledged a command for 1.5 s", "not sent")),
        ("a fault", dict(sos=3), [], None, 0.0, rhal.SKR_EPERM, ("a fault", "device error 3", "clear_faults", "not sent")),
        ("an error code", {}, [6], None, 0.0, rhal.SKR_EPERM, ("a fault", "message code 6", "clear_faults")),
        ("a stop-chain code", {}, [253], None, 0.0, rhal.SKR_EPERM, ("an emergency stop", "message code 253", "released at the controller")),
        ("a pause", dict(task_state=frames.TASK_PAUSED), [], None, 0.0, rhal.SKR_EPERM, ("paused", "`resume`", "not sent")),
        ("a pause flag", dict(paused_flag=1), [], None, 0.0, rhal.SKR_EPERM, ("paused",)),
        ("a real controller under a simulation claim", dict(mode=frames.MODE_REAL, init_state=frames.ACTIVATED), [], None, 0.0, rhal.SKR_EPERM, ("is in real mode and `simulator` is true", "`set_mode`")),
    ):
        bridge = lifeless_bridge()
        set_reading(bridge, errors=errors, **fields)
        with bridge._lock:  # noqa: SLF001
            bridge._dead = dead  # noqa: SLF001
            if lag:
                bridge._pending.append(time.monotonic() - lag)  # noqa: SLF001
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == code_wanted and all(f in said for f in fragments) and bridge._goal is None, f"{what}: {code_wanted} and why, and no goal is kept", f"{code} {said}")  # noqa: SLF001
    bridge = lifeless_bridge()
    bridge._simulated = False  # noqa: SLF001
    set_reading(bridge, mode=frames.MODE_REAL, init_state=3)
    code, said = write(bridge, cmd(pose(0.1)))
    check(code == rhal.SKR_EPERM and "not activated (activation stage 3 of 6)" in said and "`activate`" in said, "a real arm that is not activated takes nothing, saying how far the activation is", said)
    bridge = lifeless_bridge()
    with bridge._lock:  # noqa: SLF001
        bridge._latest = None  # noqa: SLF001
    code, said = write(bridge, cmd(pose(0.1)))
    check(code == rhal.SKR_ETIMEDOUT and "not been read yet" in said, "an arm that has not been read takes nothing: ETIMEDOUT", said)
    bridge = lifeless_bridge()
    code, said = write(bridge, cmd(pose(0.1)))
    check(code == rhal.SKR_EIO and "did not take the command" in said and "not a refusal by the controller" in said, "a command the line cannot carry is EIO, and says it is the line and not the controller", said)
    check(bridge._goal is None, "and no goal is kept for it")  # noqa: SLF001
    bridge = lifeless_bridge()
    bridge._estop = True  # noqa: SLF001
    check(write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_EPERM, "a latched stop takes nothing: EPERM")

    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        for what, set_it, unset_it, fragment in (
            ("a pause", lambda: setattr(sim, "task_state", frames.TASK_PAUSED), lambda: setattr(sim, "task_state", frames.TASK_IDLE), "paused"),
            ("a pause flag", lambda: setattr(sim, "paused_flag", 1), lambda: setattr(sim, "paused_flag", 0), "paused"),
            ("a device error", lambda: setattr(sim, "sos", 4), lambda: setattr(sim, "sos", 0), "device error 4"),
            ("a collision", lambda: setattr(sim, "collision", 1), lambda: setattr(sim, "collision", 0), "collision"),
            ("a controller that has changed its mode under the session", lambda: sim.set_mode_state(simulation=False), lambda: sim.set_mode_state(simulation=True), "mode was changed under this session"),
        ):
            set_it()
            seen = {"code": None, "said": ""}

            def refused_now() -> bool:
                seen["code"], seen["said"] = write(bridge, cmd(pose(0.1)))
                return seen["code"] == rhal.SKR_EPERM

            check(until(refused_now, 4.0) and fragment in seen["said"], f"{what} in the live state refuses the next command", seen["said"])
            sent = sim.servo_commands
            write(bridge, cmd(pose(0.1)))
            check(sim.servo_commands == sent, "and nothing is sent")
            unset_it()
            check(until(lambda: write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, 4.0), "and when it is over the arm takes commands again")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(simulation=False, activated=False)
    try:
        bridge = opened(sim, simulator=False)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK), "ready (a real controller that is not activated)")
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EPERM and "not activated (activation stage 0 of 6)" in said and sim.servo_commands == 0, "a real arm that was never activated takes no command, and none is written", said)
        sim.set_mode_state(activated=True)
        check(until(lambda: write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, 4.0) and until(lambda: sim.servo_commands >= 1), "and once it is activated, it does")
        bridge.close()
    finally:
        sim.stop()

    section("commands — what the controller says about the last one is told once, by the next")
    sim = new_sim(limits_deg=((-10.0, 10.0),) * 6)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        check(write(bridge, cmd(pose(0.5)))[0] == rhal.SKR_OK, "a target the controller's own limits refuse is sent: the operator's range is wider")
        check(until(lambda: _warn_seq(bridge) >= 1), "and the controller says so (a warning, after its acknowledgement)")
        code, said = write(bridge, cmd(pose(0.05)))
        check(code == rhal.SKR_EINVAL and "the controller refused the last command" in said and "code 562" in said and frames.describe_code(562) in said and "not sent" in said, "the next command is refused with the controller's words for it, and is not sent", said)
        sent = sim.servo_commands
        code, said = write(bridge, cmd(pose(0.05)))
        check(code == rhal.SKR_OK and until(lambda: sim.servo_commands == sent + 1) and said == "", "and the one after is taken: a refusal is said once", said)
        check(until(lambda: _acked(bridge) == 2), "(that one was in range, and the controller said nothing)")
        check(_warn_seq(bridge) == 1, "one warning was said in all", str(_warn_seq(bridge)))
        write(bridge, cmd(pose(0.5)))
        check(until(lambda: _warn_seq(bridge) == 2), "a second one, for the next that was not")
        code, said = write(bridge, cmd(pose(0.05)))
        check(code == rhal.SKR_EINVAL and said.count("code 562") == 1, "is told again, once", said)
        bridge.close()
    finally:
        sim.stop()
    bridge = lifeless_bridge()
    now = time.monotonic()
    with bridge._lock:  # noqa: SLF001
        bridge._warns = [(n + 1, now, 560 + n) for n in range(5)]  # noqa: SLF001
        bridge._warn_seq = 5  # noqa: SLF001
        told = bridge._take_warnings()  # noqa: SLF001
        again = bridge._take_warnings()  # noqa: SLF001
        bridge._warns.append((6, now, 570))  # noqa: SLF001
        bridge._warn_seq = 6  # noqa: SLF001
        next_one = bridge._take_warnings()  # noqa: SLF001
    check(told.count("code ") == 3 and "code 562 (" in told and "code 564 (" in told and "code 560" not in told, "of five warnings the last three are told, in the order said", told)
    check(again == "", "and they are told once")
    check(next_one.startswith("code 570 (") and next_one.count("code ") == 1, "and the next is the one that came after", next_one)

    section("commands — a refusal is said once for a run of the same cause")
    bridge = lifeless_bridge()
    first = quiet_write(bridge, cmd([0.0] * 5))[1]
    second = quiet_write(bridge, cmd([0.0] * 5))[1]
    third = quiet_write(bridge, cmd([0.0] * 6, mode=rhal.MODE_VELOCITY))[1]
    fourth = quiet_write(bridge, cmd([0.0] * 5))[1]
    check(first != "" and second == "" and third != "" and fourth != "", "five joints twice: said once; then another cause: said; then the first again: said again", f"{first!r} {second!r} {third!r} {fourth!r}")
    gated = lifeless_bridge()
    set_reading(gated, sos=3)
    one = quiet_write(gated, cmd(pose(0.1)))[1]
    two = quiet_write(gated, cmd(pose(0.1)))[1]
    set_reading(gated, ems=2)
    three = quiet_write(gated, cmd(pose(0.1)))[1]
    check(one != "" and two == "" and three != "", "a refused gate is said once, and again when the cause is another", f"{one!r} {two!r} {three!r}")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        quiet_write(bridge, cmd([0.0] * 5))
        check(quiet_write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK and quiet_write(bridge, cmd([0.0] * 5))[1] != "", "a command taken between two refusals of one cause makes the second heard")
        bridge.close()
    finally:
        sim.stop()

    section("commands — a write is bounded, and a stop outranks it")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        holder = holding_the_command_port(bridge, 1.5)
        began = time.monotonic()
        code, said = write(bridge, cmd(pose(0.1)))
        took = time.monotonic() - began
        check(code == rhal.SKR_EIO and "did not take the command" in said and 0.1 < took < 0.9, f"a command that waits on a busy line is EIO after about a quarter of a second ({took:.2f} s)", said)
        holder.join(3.0)
        check(until(lambda: write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, 4.0), "and once the line is free, commands go")
        holder = holding_the_command_port(bridge, 1.5)
        result: dict[str, object] = {}

        def attempt() -> None:
            result["took"] = time.monotonic()
            result["code"], result["said"] = write(bridge, cmd(pose(0.1)))
            result["took"] = time.monotonic() - result["took"]

        thread = threading.Thread(target=attempt, daemon=True)
        thread.start()
        time.sleep(0.05)
        bridge.emergency_stop()
        thread.join(3.0)
        check(result.get("code") == rhal.SKR_EPERM, "a stop latched while the command waits for the line makes the answer EPERM, not a slow line's EIO", str(result))
        holder.join(3.0)
        bridge.close()
    finally:
        sim.stop()


def _acked(bridge: RainbowBridge) -> int:
    with bridge._lock:  # noqa: SLF001
        return bridge._acks  # noqa: SLF001


def _warn_seq(bridge: RainbowBridge) -> int:
    with bridge._lock:  # noqa: SLF001
        return bridge._warn_seq  # noqa: SLF001

def stops_seen(sim: SimulatedController) -> int:
    return len(sim.seen("task stop"))


def main_stop() -> None:
    section("stop — a latch that returns at once, and a `task stop` that follows on the wire")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        check(write(bridge, cmd(pose(0.2)))[0] == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 1), "a command is taken")
        check(bridge.emergency_stop() == rhal.SKR_OK, "a stop returns OK")
        code, out = state(bridge)
        check(out.flags & rhal.FLAG_ESTOP and out.flags & rhal.FLAG_DEGRADED, "and the state says so at once, before the line has carried anything")
        check(until(lambda: stops_seen(sim) == 1), "the controller is told `task stop`, by the reading thread", str(sim.commands[-3:]))
        check(until(lambda: _delivered(bridge)), "and the driver knows it was written")
        code, said = write(bridge, cmd(pose(0.3)))
        check(code == rhal.SKR_EPERM, "from the instant of the latch a command is EPERM", said)
        sent = sim.servo_commands
        write(bridge, cmd(pose(0.3)))
        time.sleep(0.05)
        check(sim.servo_commands == sent, "and none reaches the controller")
        code, body = ext(bridge, "move j1=0.1")
        check(code == rhal.SKR_EPERM and "stop is latched" in str(body) and "`status`" in str(body), "a motion is refused too, and the one that is answered is named", str(body))
        code, body = ext(bridge, "status")
        check(code == rhal.SKR_OK and body["stop_latched"] is True and body["stop_delivered"] is True, "`status` says the stop is latched and that it was written", str(body)[:160])
        check(until(lambda: ext(bridge, "status")[1]["halts_seen"] == 1), "and counts the halt the controller reported")
        check(bridge.release_stop() == rhal.SKR_OK, "a release, once the arm has been read idle after the stop, is OK")
        code, out = state(bridge)
        check(not out.flags & rhal.FLAG_ESTOP and code == rhal.SKR_OK, "and the state no longer says stopped")
        check(until(lambda: write(bridge, cmd(pose(0.3)))[0] == rhal.SKR_OK, 3.0), "and commands are taken again (the controller does not latch the stop, so the driver's latch was the stop)")
        bridge.close()
    finally:
        sim.stop()

    section("stop — never blocks, and is written once the line is free")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        holder = holding_the_command_port(bridge, 3.0)
        began = time.monotonic()
        code = bridge.emergency_stop()
        took = time.monotonic() - began
        check(code == rhal.SKR_OK and took < 0.2, f"a stop on a line that is busy for 3 s returns at once ({took * 1000:.1f} ms)")
        check(write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_EPERM, "and refuses commands from that moment")
        check(stops_seen(sim) == 0, "(nothing has been written yet)")
        code, said = said_by(bridge.release_stop)
        check(code == rhal.SKR_EIO and "the stop stays latched" in said and "no reading has shown the arm take it" in said, "a release before the stop was written is refused: EIO, and the latch stays", said)
        check(bridge._estop, "(it does)")  # noqa: SLF001
        holder.join(3.0)
        check(until(lambda: stops_seen(sim) == 1, 4.0), "once the line is free the stop is written")
        check(bridge.release_stop() == rhal.SKR_OK and not bridge._estop, "and then it can be released")  # noqa: SLF001
        bridge.close()
    finally:
        sim.stop()

    section("stop — two stops are two writes, and none is lost")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        bridge.emergency_stop()
        check(until(lambda: stops_seen(sim) == 1 and _delivered(bridge)), "a stop")
        bridge.emergency_stop()
        check(until(lambda: stops_seen(sim) == 2), "and another, which is written again: the first one's delivery is not the second's")
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        bridge.close()
    finally:
        sim.stop()

    section("stop — on the wire it follows every command in flight, and no command follows it")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        running = threading.Event()
        halt = threading.Event()
        counts = {"sent": 0, "refused": 0}

        def stream() -> None:
            running.set()
            n = 0
            while not halt.is_set():
                n += 1
                code, _ = write(bridge, cmd(pose(0.0005 * (n % 200)), mask=[1, 0, 0, 0, 0, 0]))
                counts["sent" if code == rhal.SKR_OK else "refused"] += 1

        thread = threading.Thread(target=stream, daemon=True)
        thread.start()
        running.wait(2.0)
        check(until(lambda: counts["sent"] > 20, 4.0), "a stream of commands is running")
        bridge.emergency_stop()
        check(until(lambda: stops_seen(sim) == 1 and counts["refused"] > 0, 4.0), "a stop in the middle of it is written, and the stream is refused")
        halt.set()
        thread.join(3.0)
        with sim._lock:  # noqa: SLF001
            lines = list(sim.commands)
        at = lines.index("task stop")
        check(not any(line.startswith("move_servo_j") for line in lines[at + 1:]), "and no command follows the stop on the wire", str(lines[at - 1:at + 3]))
        check(any(line.startswith("move_servo_j") for line in lines[:at]), "(there were commands before it)")
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        check(until(lambda: write(bridge, cmd(pose(0.1), mask=[1, 0, 0, 0, 0, 0]))[0] == rhal.SKR_OK, 3.0), "and a command after the release is taken")
        check(until(lambda: _servo_after_stop(sim)), "after it on the wire")
        bridge.close()
    finally:
        sim.stop()

    section("stop — what the stop ends, and what the arm is asked to hold afterwards")
    sim = new_sim(time_scale=0.1)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        check(write(bridge, cmd(pose(5.0), mask=[1, 0, 0, 0, 0, 0]))[0] == rhal.SKR_OK, "a far target for a slow arm")
        check(until(lambda: sim.robot_state != frames.ROBOT_IDLE), "and the arm is moving")
        check(until(lambda: state(bridge)[1].position_rad[0] > 0.05), "and read moving")
        bridge.emergency_stop()
        check(until(lambda: sim.robot_state == frames.ROBOT_IDLE), "a stop ends the motion at the controller")
        stopped_at = sim.joints_deg[0]
        check(0.0 < stopped_at < 5.0 / DEG, "short of where it was going", str(stopped_at))
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        check(until(lambda: abs(state(bridge)[1].position_rad[0] * 1 - stopped_at * DEG) < 1e-3), "the arm is read where it stopped")
        check(write(bridge, cmd(pose(0.5), mask=[0, 1, 0, 0, 0, 0]))[0] == rhal.SKR_OK and until(lambda: len(servo_texts(sim)) == 2), "a command for j2 alone")
        held = angles_of(servo_texts(sim)[1])
        check(abs(held[0] - stopped_at) < 0.5, "holds j1 where the arm is, and not at the 5.0 rad it was once asked for", str(held))
        bridge.close()
    finally:
        sim.stop()

    section("stop — a controller that does not stop is not called stopped")
    sim = new_sim(time_scale=0.1)
    try:
        sim.ignore_stop = True
        bridge = opened(sim)
        check(ready(bridge), "ready")
        write(bridge, cmd(pose(5.5), mask=[1, 0, 0, 0, 0, 0]))
        check(until(lambda: sim.robot_state != frames.ROBOT_IDLE), "the arm is moving")
        bridge.emergency_stop()
        check(until(lambda: stops_seen(sim) == 1 and _delivered(bridge)), "the stop is written, and acknowledged")
        code, said = said_by(bridge.release_stop)
        check(code == rhal.SKR_EIO and "the stop stays latched" in said and "no reading has shown the arm take it" in said and sim.robot_state != frames.ROBOT_IDLE, "but the arm goes on, so the release is refused: the latch is not cleared on a stop only written", said)
        check(bridge._estop and write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_EPERM, "and commands are still refused")  # noqa: SLF001
        sim.ignore_stop = False
        bridge.emergency_stop()
        check(until(lambda: sim.robot_state == frames.ROBOT_IDLE), "a stop the controller takes ends it")
        check(bridge.release_stop() == rhal.SKR_OK, "and then it releases")
        bridge.close()
    finally:
        sim.stop()

    section("stop — a stop after a connection that was lost is written on the next one")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.drop_connections("command")
        check(until(lambda: _command_connections(sim) == 1, 6.0), "a command connection the controller hangs up is opened again by the driver")
        bridge.emergency_stop()
        check(until(lambda: stops_seen(sim) >= 1, 6.0), "and the stop is written on the new one")
        check(until(lambda: _delivered(bridge), 3.0), "and the driver knows it was")
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        bridge.close()
    finally:
        sim.stop()

    section("stop — a session starts with no stop")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        bridge.emergency_stop()
        check(until(lambda: _delivered(bridge)), "stopped")
        check(bridge.close() == rhal.SKR_OK and bridge._estop, "a close writes nothing and leaves the latch")  # noqa: SLF001
        check(stops_seen(sim) == 1, "(the stop was written once, before)")
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.open(config(sim))
        check(code == rhal.SKR_OK and not bridge._estop and not _delivered(bridge), "an open is a new session, and has none", said.getvalue())  # noqa: SLF001
        check(ready(bridge) and not state(bridge)[1].flags & rhal.FLAG_ESTOP, "ready, not stopped")
        check(until(lambda: write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, 3.0), "and commands are taken")
        check(stops_seen(sim) == 1, "and nothing wrote another stop")
        bridge.close()
    finally:
        sim.stop()


def _delivered(bridge: RainbowBridge) -> bool:
    with bridge._lock:  # noqa: SLF001
        return bridge._stop_delivered  # noqa: SLF001


def _command_connections(sim: SimulatedController) -> int:
    with sim._lock:  # noqa: SLF001
        return len([c for c in sim._connections if sim._is_command(c)])  # noqa: SLF001


def _servo_after_stop(sim: SimulatedController) -> bool:
    with sim._lock:  # noqa: SLF001
        lines = list(sim.commands)
    return "task stop" in lines and any(line.startswith("move_servo_j") for line in lines[lines.index("task stop"):])

def run_in_thread(function, *args) -> tuple[threading.Thread, dict]:
    """A call on another thread, and where its answer goes."""
    box: dict[str, object] = {}

    def body() -> None:
        box["began"] = time.monotonic()
        box["result"] = function(*args)
        box["took"] = time.monotonic() - box["began"]  # type: ignore[operator]

    thread = threading.Thread(target=body, daemon=True)
    thread.start()
    return thread, box


def main_motions() -> None:
    section("motions — the request: a token, text, a name and k=v words")
    bridge = RainbowBridge()
    check(bridge.extension(TOKEN, b"status")[0] == rhal.SKR_EPERM, "a bridge that is not open answers no request: EPERM")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        check(RainbowBridge.stop_during_extension is True, "a stop may be written to the host while a motion runs")
        code, body = bridge.extension("vendor.other.thing", b"status")
        check(code == rhal.SKR_ENOSYS and TOKEN in body.decode() and "'vendor.other.thing'" in body.decode(), "another token is ENOSYS, naming this driver's", body.decode())
        code, body = bridge.extension(TOKEN, b"\xff\xfe")
        check(code == rhal.SKR_EINVAL and b"UTF-8" in body, "a request that is not UTF-8 is EINVAL", body.decode())
        for text in ("", "   "):
            code, body = ext(bridge, text)
            check(code == rhal.SKR_EINVAL and "empty request" in body and "Motions: status, set_mode, activate, resume, clear_faults, set_payload, move" in body, f"{text!r} is an empty request, and the motions are listed", str(body))
        for word in ("move", "move j1", "move =5", "move j1=0.1 j2"):
            code, body = ext(bridge, word)
            check(code == rhal.SKR_EINVAL and ("malformed argument" in body or "at least one joint" in body), f"{word!r} is refused, as malformed or as nothing to do", str(body))
        code, body = ext(bridge, "move j1")
        check(code == rhal.SKR_EINVAL and "malformed argument 'j1': wanted k=v" in body, "a word with no `=` is malformed, and quoted", str(body))
        code, body = ext(bridge, "fly")
        check(code == rhal.SKR_EINVAL and "unknown motion 'fly'" in body and "status, set_mode" in body, "an unknown motion is EINVAL, and the motions are listed", str(body))
        code, body = ext(bridge, "status extra=1")
        check(code == rhal.SKR_EINVAL and "`status` does not take extra" in body, "`status` takes no argument", str(body))
        bridge.close()
    finally:
        sim.stop()

    section("motions — status")
    sim = new_sim()
    try:
        bridge = opened(sim, payload_kg=1.5, payload_com_mm=[1, 2, 3], limits=LIMITS)
        check(ready(bridge), "ready")
        with sim._lock:  # noqa: SLF001
            sim.ref = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0]
        check(until(lambda: abs(state(bridge)[1].position_rad[5] - 60.0 * DEG) < 1e-3), "an arm at 10 20 30 40 50 60 degrees")
        code, body = ext(bridge, "status")
        check(code == rhal.SKR_OK and isinstance(body, dict), "status is a JSON object", str(body)[:100])
        check(body["model"] == "RB5-850" and body["simulator"] is True and body["software_version"] == 26042604, "which arm, whether it is a simulation, and the control box's software version", str(body)[:200])
        check(body["link"] == f"127.0.0.1:{sim.command_port}" and body["data"] == f"127.0.0.1:{sim.data_port}", "and the two addresses", f"{body['link']} {body['data']}")
        check(body["mode"] == frames.MODE_SIMULATION and body["mode_name"] == "simulation" and body["init_state"] == 0 and body["activated"] is False and body["power_applied"] is False, "its mode, and how far its activation is")
        check(body["task_state"] == frames.TASK_IDLE and body["paused"] is False and body["moving"] is False and body["robot_state"] == frames.ROBOT_IDLE, "the task, and whether anything is moving")
        check(body["fault"] is None and body["errors"] == [] and body["warnings"] == [] and body["messages"] == [] and body["not_commandable"] is None, "no fault, no code, nothing that keeps it from taking a command")
        check(body["sos"] == 0 and body["ems"] == 0 and body["collision"] == 0 and body["joint_info"] == [0b110] * 6 and body["sos_state"] is False and body["estop_bits"] == [0, 0] and body["freedrive"] == 0, "and every field of the packet that bears on one")
        check(all(abs(a - b) < 1e-3 for a, b in zip(body["position_deg"], [10, 20, 30, 40, 50, 60])) and all(abs(a - b * DEG) < 1e-5 for a, b in zip(body["position_rad"], [10, 20, 30, 40, 50, 60])), "the position in both units", str(body["position_deg"]))
        check(body["reference_deg"] == body["encoder_deg"] and body["goal_deg"] is None and body["temperatures"] == [30.0] * 6 and body["speed_bar"] == 1.0, "the reference and the encoders, no goal yet, the temperatures and the speed bar")
        check(body["payload_configured"] == {"kg": 1.5, "com_mm": [1.0, 2.0, 3.0]}, "the payload the operator said (it is never told to the controller by an open)", str(body["payload_configured"]))
        check(body["packets"] > 0 and body["link_dead"] is None and body["unacknowledged"] == 0 and body["refused_lines"] == 0 and body["unknown_lines"] == 0 and body["halts_seen"] == 0 and body["stop_latched"] is False and body["stop_delivered"] is False, "and the counters", str(body)[-300:])
        write(bridge, cmd(pose(0.1), mask=[1, 0, 0, 0, 0, 0]))
        check(until(lambda: ext(bridge, "status")[1]["acknowledged"] == 1), "an acknowledged line is counted")
        check(ext(bridge, "status")[1]["goal_deg"] is not None and abs(ext(bridge, "status")[1]["goal_deg"][0] - 0.1 / DEG) < 1e-3, "and the goal the driver last asked for is given, in degrees")
        sim.say("what is this")
        check(until(lambda: ext(bridge, "status")[1]["unknown_lines"] == 1), "a line the driver does not know is counted, not guessed at")
        sim.say("error[msg][a thing went wrong]")
        check(until(lambda: ext(bridge, "status")[1]["messages"] == ["error[msg]: a thing went wrong"]), "and an error with a message but no code is kept in the controller's words")
        sim.raise_warn(562)
        check(until(lambda: len(ext(bridge, "status")[1]["warnings"]) == 1), "a warning is listed")
        warn = ext(bridge, "status")[1]["warnings"][0]
        check(warn["code"] == 562 and warn["text"] == frames.describe_code(562) and 0 <= warn["age_s"] < 3, "with its code, the maker's words and its age", str(warn))
        sim.raise_error(6)
        check(until(lambda: ext(bridge, "status")[1]["fault"] is not None), "an error is listed")
        body = ext(bridge, "status")[1]
        check(body["fault"] == {"class": "fault", "code": 6, "text": bridge_module._say_code(6)} and body["errors"] == [{"code": 6, "text": frames.describe_code(6)}], "as a fault of that code, in the maker's words", str(body["fault"]))  # noqa: SLF001
        bridge.close()
    finally:
        sim.stop()

    section("motions — clear_faults")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, body = ext(bridge, "clear_faults")
        check(code == rhal.SKR_OK and body["result"] == "nothing_to_clear" and body["codes"] == [], "with nothing reported there is nothing to clear", str(body))
        code, body = ext(bridge, "clear_faults now=1")
        check(code == rhal.SKR_EINVAL and "does not take now" in body, "and it takes no argument", str(body))
        sim.raise_error(253)
        sim.raise_error(5)
        sim.raise_warn(562)
        sim.say("error[msg][a thing went wrong]")
        check(until(lambda: _errors_of(bridge) == [253, 5] and _warn_seq(bridge) == 1 and ext(bridge, "status")[1]["messages"]), "an emergency stop code, another, a warning and a message")
        check(state(bridge)[1].flags & rhal.FLAG_ESTOP, "the state is an emergency stop")
        sim.sos = 3
        check(until(lambda: ext(bridge, "status")[1]["sos"] == 3), "and the packet carries a device error")
        code, body = ext(bridge, "clear_faults")
        check(code == rhal.SKR_EPERM and "still reports a device error (big error)" in body and "Remove it at the controller" in body and _errors_of(bridge) == [253, 5], "while the packet still carries a cause nothing is acknowledged: EPERM, saying what it is", str(body))
        sim.sos = 0
        sim.ems = 3
        check(until(lambda: ext(bridge, "status")[1]["ems"] == 3), "(a software emergency stop)")
        code, body = ext(bridge, "clear_faults")
        check(code == rhal.SKR_EPERM and "software emergency stop (joint limit)" in body, "is refused too", str(body))
        sim.ems = 0
        sim.collision = 1
        check(until(lambda: ext(bridge, "status")[1]["collision"] == 1), "(a collision)")
        code, body = ext(bridge, "clear_faults")
        check(code == rhal.SKR_EPERM and "external collision" in body, "and a collision", str(body))
        sim.collision = 0
        sim.joint_info = [0b110, 0b110, 0b110, 0b110 | 1 << 10, 0b110, 0b110]
        check(until(lambda: ext(bridge, "status")[1]["joint_info"][3] != 0b110), "(a joint's error bit)")
        code, body = ext(bridge, "clear_faults")
        check(code == rhal.SKR_EPERM and "j4: BIG error" in body, "and a joint's bit, naming the joint and the bit", str(body))
        sim.joint_info = [0b110] * 6
        sim.chunk1 = 1 << 12
        check(until(lambda: ext(bridge, "status")[1]["sos_state"] is True), "(the SOS state)")
        code, body = ext(bridge, "clear_faults")
        check(code == rhal.SKR_EPERM and "the SOS state" in body, "and the SOS state", str(body))
        sim.chunk1 = 0
        sim.mode_override = 9
        check(until(lambda: ext(bridge, "status")[1]["mode"] == 9), "(a mode the document does not list)")
        code, body = ext(bridge, "clear_faults")
        check(code == rhal.SKR_EPERM and "a mode the document does not list (9)" in body, "and a mode", str(body))
        sim.mode_override = None
        check(until(lambda: ext(bridge, "status")[1]["mode"] == frames.MODE_SIMULATION), "(over)")
        code, body = ext(bridge, "clear_faults")
        check(code == rhal.SKR_OK and body["result"] == "cleared" and body["codes"] == [253, 5], "with no cause in the packet the codes are acknowledged, and listed", str(body))
        after = ext(bridge, "status")[1]
        check(after["errors"] == [] and after["warnings"] == [] and after["messages"] == [] and after["fault"] is None, "the codes, the warnings and the messages are gone")
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & (rhal.FLAG_ESTOP | rhal.FLAG_FAULT | rhal.FLAG_DEGRADED)), "and the state is healthy again")
        check(write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, "and a command is taken")
        sim.raise_warn(562)
        check(until(lambda: _warn_seq(bridge) == 2), "a warning after the acknowledgement")
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EINVAL and "code 562" in said, "is told by the next command: the acknowledgement did not turn the count back", said)
        bridge.close()
    finally:
        sim.stop()

    section("motions — resume")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, body = ext(bridge, "resume")
        check(code == rhal.SKR_OK and body == {"result": "not_paused"} and not sim.seen("task resume"), "an arm that is not paused is not told to resume", str(body))
        for bad in ("resume after=never", "resume after=", "resume foo=1"):
            check(ext(bridge, bad)[0] == rhal.SKR_EINVAL, f"{bad!r} is refused")
        code, body = ext(bridge, "resume after=never")
        check("after=pause" in body and "after=collision" in body, "saying what it takes", str(body))
        sim.task_state = frames.TASK_PAUSED
        check(until(lambda: ext(bridge, "status")[1]["paused"] is True), "a paused arm")
        code, body = ext(bridge, "resume")
        check(code == rhal.SKR_OK and body["result"] == "resumed" and body["task_state"] == frames.TASK_IDLE and sim.seen("task resume_a") == ["task resume_a"], "is resumed with `task resume_a`", str(body))
        sim.paused_flag = 1
        check(until(lambda: ext(bridge, "status")[1]["paused"] is True), "an arm whose pause flag is up")
        code, body = ext(bridge, "resume after=pause")
        check(code == rhal.SKR_OK and body["result"] == "resumed" and sim.paused_flag == 0, "is resumed too, and the flag comes down", str(body))
        sim.task_state = frames.TASK_PAUSED
        sim.collision = 1
        check(until(lambda: ext(bridge, "status")[1]["paused"] is True and ext(bridge, "status")[1]["collision"] == 1), "an arm paused by a collision")
        code, body = ext(bridge, "resume after=collision")
        check(code == rhal.SKR_OK and body["result"] == "resumed" and sim.seen("task resume_b") == ["task resume_b"] and sim.collision == 0, "is resumed with `task resume_b`", str(body))
        sim.task_state = frames.TASK_PAUSED
        check(until(lambda: ext(bridge, "status")[1]["paused"] is True), "paused again")
        sim.raise_error(5)
        code, body = ext(bridge, "resume")
        check(code == rhal.SKR_OK, "a resume does not look at the fault: it is not a command", str(body))
        bridge.close()
    finally:
        sim.stop()

    section("motions — set_mode")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        for bad in ("set_mode", "set_mode mode=fast", "set_mode mode=", "set_mode mode=real extra=1"):
            code, body = ext(bridge, bad)
            check(code == rhal.SKR_EINVAL, f"{bad!r} is refused")
        code, body = ext(bridge, "set_mode mode=fast")
        check("`mode=real` or `mode=simulation`" in body, "saying what it takes", str(body))
        code, body = ext(bridge, "set_mode mode=real")
        check(code == rhal.SKR_EPERM and "`simulator: true`" in body and "open the driver again" in body and not sim.seen("pgmode"), "a session that said simulation is not turned real by a motion: EPERM, and nothing is written", str(body))
        sim.set_mode_state(simulation=False)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED), "a controller put in its real mode under the session")
        code, body = ext(bridge, "set_mode mode=simulation")
        check(code == rhal.SKR_OK and body == {"result": "set", "mode": "simulation"} and sim.seen("pgmode") == ["pgmode simulation"] and sim.simulation, "is put back by `pgmode simulation`", str(body))
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED), "and the state is healthy")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(time_scale=0.05)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        write(bridge, cmd(pose(5.0), mask=[1, 0, 0, 0, 0, 0]))
        check(until(lambda: sim.robot_state != frames.ROBOT_IDLE and state(bridge)[1].position_rad[0] > 0.01), "an arm in motion")
        code, body = ext(bridge, "set_mode mode=simulation")
        check(code == rhal.SKR_EPERM and "executing a motion command" in body and not sim.seen("pgmode"), "has its mode left alone: EPERM, and nothing is written", str(body))
        bridge.emergency_stop()
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(simulation=False, activated=True)
    try:
        sim.allow_real = False
        bridge = opened(sim, simulator=False)
        check(ready(bridge), "ready (a real controller)")
        sim.set_mode_state(simulation=True)
        check(until(lambda: ext(bridge, "status")[1]["mode"] == frames.MODE_SIMULATION), "put in its simulation mode under the session")
        began = time.monotonic()
        code, body = ext(bridge, "set_mode mode=real")
        check(code == rhal.SKR_EIO and "`pgmode real` was written" in body and "no reading has shown the controller in that mode" in body and "no real mode" in body and sim.seen("pgmode") == ["pgmode real"], f"a controller that does not go real (the Virtual Control Box has no real mode) is EIO, after about {time.monotonic() - began:.1f} s", str(body))
        sim.allow_real = True
        code, body = ext(bridge, "set_mode mode=real")
        check(code == rhal.SKR_OK and body == {"result": "set", "mode": "real"} and not sim.simulation, "and one that does is set", str(body))
        code, body = ext(bridge, "set_mode mode=simulation")
        check(code == rhal.SKR_EPERM and "`simulator: false`" in body, "while a real session is not turned to simulation", str(body))
        bridge.close()
    finally:
        sim.stop()

    section("motions — activate")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, body = ext(bridge, "activate")
        check(code == rhal.SKR_OK and body["result"] == "not_needed" and not sim.seen("mc "), "a simulation needs no activation, and none is written", str(body))
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(simulation=False, activated=False)
    try:
        bridge = opened(sim, simulator=False, payload_kg=2.5, payload_com_mm=[10, -20, 30.5])
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK), "ready (a real controller that is not activated)")
        for bad, fragment in (("activate timeout=0.5", "outside 1.0 to 600.0"), ("activate timeout=601", "outside 1.0 to 600.0"), ("activate timeout=soon", "not a number"), ("activate now=1", "does not take now"), ("activate timeout=nan", "outside")):
            code, body = ext(bridge, bad)
            check(code == rhal.SKR_EINVAL and fragment in body and not sim.seen("mc "), f"{bad!r} is refused ({fragment}), and nothing is written", str(body))
        code, body = ext(bridge, "activate timeout=20")
        check(code == rhal.SKR_OK and body["result"] == "activated" and body["payload_told"] == {"kg": 2.5, "com_mm": [10.0, -20.0, 30.5]}, "an arm is activated with `mc jall init`, and told the payload the operator gave", str(body))
        check(until(lambda: len(sim.seen("set_payload_info")) == 1), "(the payload line reaches the controller)")
        check(sim.seen("mc ") == ["mc jall init"] and sim.seen("set_payload_info") == [frames.set_payload_info(2.5, (10.0, -20.0, 30.5))] and sim.payload == (2.5, 10.0, -20.0, 30.5), "in that order, and nothing else", str(sim.commands))
        check(sim.commands.index("mc jall init") < sim.commands.index(frames.set_payload_info(2.5, (10.0, -20.0, 30.5))), "the activation first")
        code, body = ext(bridge, "activate")
        check(code == rhal.SKR_OK and body == {"result": "already_activated"} and len(sim.seen("mc ")) == 1, "an arm that is activated is not activated again", str(body))
        check(ext(bridge, "status")[1]["activated"] is True and ext(bridge, "status")[1]["power_applied"] is True, "`status` says so, and that the arm has power")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(simulation=False, activated=False, activation_s=1000.0, time_scale=1.0)
    try:
        bridge = opened(sim, simulator=False)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK), "ready (an activation that never finishes)")
        began = time.monotonic()
        code, body = ext(bridge, "activate timeout=1")
        took = time.monotonic() - began
        check(code == rhal.SKR_EIO and "`mc jall init` was written and no reading has shown the activation done" in body and 0.8 < took < 3.0, f"is EIO after the time given ({took:.1f} s)", str(body))
        thread, box = run_in_thread(ext, bridge, "activate timeout=20")
        check(until(lambda: len(sim.seen("mc ")) == 2), "a second try is written")
        sim.sos = 3
        thread.join(5.0)
        code, body = box.get("result", (None, None))
        check(code == rhal.SKR_EIO and "device error 3 (big error)" in body and "during its activation" in body, "an arm that reports a device error while activating ends it: EIO, saying what", str(body))
        sim.sos = 0
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(simulation=False, activated=False)
    try:
        bridge = opened(sim, simulator=False)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK), "ready")
        code, body = ext(bridge, "activate")
        check(code == rhal.SKR_OK and body["result"] == "activated" and body["payload_told"] is None and not sim.seen("set_payload_info"), "an activation with no payload configured tells the controller none", str(body))
        bridge.close()
    finally:
        sim.stop()

    section("motions — set_payload")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        for bad, fragment in (("set_payload", "needs `kg=`"), ("set_payload x=1", "needs `kg=`"), ("set_payload kg=-1", "outside 0.0 to 100.0"), ("set_payload kg=100.5", "outside"), ("set_payload kg=abc", "not a number"),
                              ("set_payload kg=1 x=2001", "outside -2000.0 to 2000.0"), ("set_payload kg=1 z=-2001", "outside"), ("set_payload kg=1 w=3", "does not take w"), ("set_payload kg=nan", "outside")):
            code, body = ext(bridge, bad)
            check(code == rhal.SKR_EINVAL and fragment in body and not sim.seen("set_payload_info"), f"{bad!r} is refused ({fragment}), and nothing is written", str(body))
        code, body = ext(bridge, "set_payload kg=2.5 x=10 y=-20 z=30.5")
        check(code == rhal.SKR_OK and body["result"] == "told" and body["kg"] == 2.5 and body["com_mm"] == [10.0, -20.0, 30.5], "a payload is told to the controller", str(body))
        check(until(lambda: sim.seen("set_payload_info") == [frames.set_payload_info(2.5, (10.0, -20.0, 30.5))] and sim.payload == (2.5, 10.0, -20.0, 30.5)), "as `set_payload_info(kg, x, y, z)`", str(sim.commands[-2:]))
        check(ext(bridge, "status")[1]["payload_configured"] == {"kg": 2.5, "com_mm": [10.0, -20.0, 30.5]}, "and `status` shows what was told")
        code, body = ext(bridge, "set_payload kg=0")
        check(code == rhal.SKR_OK and body["kg"] == 0.0 and body["com_mm"] == [0.0, 0.0, 0.0], "a payload of nothing, with the centre at the flange, is allowed", str(body))
        code, body = ext(bridge, "set_payload kg=100 x=-2000 y=2000")
        check(code == rhal.SKR_OK and body["com_mm"] == [-2000.0, 2000.0, 0.0], "and so are the ends of the ranges", str(body))
        sim.sos = 4
        check(until(lambda: ext(bridge, "status")[1]["sos"] == 4), "a device error")
        sent = len(sim.seen("set_payload_info"))
        code, body = ext(bridge, "set_payload kg=1")
        check(code == rhal.SKR_EPERM and "a fault" in body and len(sim.seen("set_payload_info")) == sent, "is not an arm to tell a payload: it is gated like a command, and nothing is written", str(body))
        bridge.close()
    finally:
        sim.stop()

    section("motions — move")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        for bad, fragment in (("move", "at least one joint"), ("move speed=10", "at least one joint"), ("move j7=0.1", "`j7` is not a joint"), ("move j1=abc", "not a number"), ("move j1=9", "`j1` of 9 is outside -6.0 to 6.0"), ("move j2=-3.5", "`j2` of -3.5 is outside -3.0 to 3.0"),
                              ("move j1=nan", "outside"), ("move j1=0.1 timeout=0.4", "outside 0.5 to 300.0"), ("move j1=0.1 timeout=301", "outside"), ("move j1=0.1 speed=0.05", "outside 0.1 to 360.0"), ("move j1=0.1 speed=361", "outside"),
                              ("move j1=0.1 accel=0.05", "outside 0.1 to 3600.0"), ("move j1=0.1 accel=3601", "outside")):
            code, body = ext(bridge, bad)
            check(code == rhal.SKR_EINVAL and fragment in body and not sim.seen("move_j("), f"{bad!r} is refused ({fragment}), and nothing is written", str(body))
        with sim._lock:  # noqa: SLF001
            sim.ref = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0]
        check(until(lambda: abs(state(bridge)[1].position_rad[0] - 10.0 * DEG) < 1e-3), "an arm at 10 20 30 40 50 60 degrees")
        code, body = ext(bridge, "move j1=0.5 j3=-0.25")
        check(code == rhal.SKR_OK and body["result"] == "arrived", "a move to a pose arrives", str(body))
        text = sim.seen("move_j(")[0]
        sent = angles_of(text)
        check(abs(sent[0] - 0.5 / DEG) < 1e-4 and abs(sent[2] + 0.25 / DEG) < 1e-4 and all(abs(a - b) < 1e-3 for a, b in zip((sent[1], sent[3], sent[4], sent[5]), (20.0, 40.0, 50.0, 60.0))), "the joints named go where they are told in degrees, and the others stay where they are", text)
        check(text.endswith("],20,40)"), "at the controller's speed and acceleration of the move, 20 and 40 degrees by default", text)
        check(abs(body["goal_deg"][0] - 0.5 / DEG) < 1e-3 and abs(body["position_deg"][0] - 0.5 / DEG) < 0.5 and abs(body["position_deg"][2] + 0.25 / DEG) < 0.5, "and the answer says where it was told to go and where it is", str(body))
        check(abs(ext(bridge, "status")[1]["goal_deg"][0] - 0.5 / DEG) < 1e-3, "and the goal is the driver's, for the next command to hold")
        code, body = ext(bridge, "move j2=0.2 speed=45 accel=90 timeout=20")
        check(code == rhal.SKR_OK and body["result"] == "arrived" and sim.seen("move_j(")[1].endswith("],45,90)"), "with a speed and an acceleration given, they are the ones written", sim.seen("move_j(")[1])
        code, body = ext(bridge, "move j2=0.2")
        check(code == rhal.SKR_OK and body["result"] == "arrived", "a move to where it already is arrives", str(body))
        bridge.close()
    finally:
        sim.stop()

    section("motions — a move that does not arrive")
    sim = new_sim(time_scale=0.1)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        thread, box = run_in_thread(ext, bridge, "move j1=5.0 timeout=30")
        check(until(lambda: sim.robot_state != frames.ROBOT_IDLE), "a long slow move starts")
        bridge.emergency_stop()
        thread.join(5.0)
        code, body = box.get("result", (None, None))
        check(code == rhal.SKR_EPERM and "stop was latched" in body, "a stop ends it: EPERM, saying the stop was latched", str(body))
        check(box.get("took", 99) < 3.0, f"within moments of the stop ({box.get('took', 99):.2f} s)")
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(time_scale=0.1)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        thread, box = run_in_thread(ext, bridge, "move j1=5.0 timeout=30")
        check(until(lambda: sim.robot_state != frames.ROBOT_IDLE and state(bridge)[1].position_rad[0] > 0.02), "a move starts")
        with sim._lock:  # noqa: SLF001
            sim._halt()  # noqa: SLF001 - what the pendant's stop button does
        thread.join(6.0)
        code, body = box.get("result", (None, None))
        check(code == rhal.SKR_OK and body["result"] == "stopped_short" and body["position_deg"][0] < body["goal_deg"][0] - 5.0, "a move that something else stopped has stopped short, and says so with where", str(body))
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(time_scale=0.1)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        thread, box = run_in_thread(ext, bridge, "move j1=5.0 timeout=30")
        check(until(lambda: sim.robot_state != frames.ROBOT_IDLE), "a move starts")
        sim.sos = 3
        thread.join(6.0)
        code, body = box.get("result", (None, None))
        check(code == rhal.SKR_EIO and "fault during the move" in body, "a fault during it ends it: EIO", str(body))
        sim.sos = 0
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim()
    try:
        sim.freeze = True
        bridge = opened(sim)
        check(ready(bridge), "ready (a controller that takes a move and does nothing)")
        began = time.monotonic()
        code, body = ext(bridge, "move j1=0.5 timeout=0.6")
        took = time.monotonic() - began
        check(code == rhal.SKR_EIO and "never started moving after move_j" in body and 0.4 < took < 3.0, f"is EIO, saying the arm never started ({took:.1f} s)", str(body))
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(time_scale=0.1)
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        began = time.monotonic()
        code, body = ext(bridge, "move j1=5.0 timeout=0.8")
        took = time.monotonic() - began
        check(code == rhal.SKR_EIO and "no reading showed what was waited for" in body and "never started" not in body and 0.6 < took < 3.0, f"a move that is still going at its timeout is EIO, and is not said never to have started ({took:.1f} s)", str(body))
        bridge.emergency_stop()
        check(until(lambda: sim.robot_state == frames.ROBOT_IDLE), "(stopped)")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.task_state = frames.TASK_PAUSED
        check(until(lambda: ext(bridge, "status")[1]["paused"] is True), "a paused arm")
        code, body = ext(bridge, "move j1=0.2")
        check(code == rhal.SKR_EPERM and "paused" in body and not sim.seen("move_j("), "takes no move: gated like a command, and nothing is written", str(body))
        sim.task_state = frames.TASK_IDLE
        sim.sos = 5
        check(until(lambda: ext(bridge, "status")[1]["sos"] == 5), "an arm with a device error")
        code, body = ext(bridge, "move j1=0.2")
        check(code == rhal.SKR_EPERM and "a fault" in body and not sim.seen("move_j("), "takes none either", str(body))
        bridge.close()
    finally:
        sim.stop()

def exchange_threads() -> list[threading.Thread]:
    return [t for t in threading.enumerate() if t.name == "rainbow-exchange" and t.is_alive()]


def main_session() -> None:
    section("session — open, close and open again leave nothing behind")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        bridge.close()
        base_fds = settled_fds()
        base_threads = len(exchange_threads())
        for round_ in range(5):
            bridge = opened(sim)
            if round_ == 0:
                check(len(exchange_threads()) == base_threads + 1, "an open starts the reading thread, and one", str(len(exchange_threads())))
                check(ready(bridge), "ready")
            before = len(sim.commands)
            check(bridge.close() == rhal.SKR_OK, f"close {round_ + 1} is OK")
            check(len(exchange_threads()) == base_threads, f"and ends the reading thread ({round_ + 1})")
        check(settled_fds() <= base_fds, f"five sessions leave the same number of descriptors ({base_fds} before, {settled_fds()} after)")
        check(len(sim.commands) == before, "a close writes nothing to the controller: no stop, no request, not a byte")
        check(bridge.close() == rhal.SKR_OK, "a second close is OK")
        check(state(bridge)[0] == rhal.SKR_EPERM and write(bridge, cmd(pose(0.0)))[0] == rhal.SKR_EPERM and bridge.extension(TOKEN, b"status")[0] == rhal.SKR_EPERM, "and a closed bridge answers nothing: EPERM from the state, a command and a request")
        for refusal in ('{"model": "RB5-850"}', "not json", json.dumps({**settings(sim), "limits": None})):
            bridge = RainbowBridge()
            with contextlib.redirect_stderr(io.StringIO()):
                bridge.open(refusal)
            check(len(exchange_threads()) == base_threads, "a refused open starts no thread")
            bridge.close()
        closed_port = sim.data_port
        sim.stop()
        bridge = RainbowBridge()
        with contextlib.redirect_stderr(io.StringIO()):
            code = bridge.open(json.dumps(settings(sim, data_port=closed_port, open_timeout_ms=300)))
        check(code == rhal.SKR_EIO and len(exchange_threads()) == base_threads, "and neither does an open that fails on the line")
        check(settled_fds() <= base_fds, f"or leaves a descriptor ({settled_fds()})")
        bridge.close()
    finally:
        sim.stop()

    section("session — the reading thread runs at the configured rate")
    sim = new_sim()
    try:
        bridge = opened(sim, rate_hz=20)
        check(ready(bridge), "ready")
        first = sim.data_requests
        time.sleep(1.0)
        asked = sim.data_requests - first
        check(5 <= asked <= 30, f"at 20 Hz the state is asked for about twenty times a second ({asked})")
        bridge.close()
    finally:
        sim.stop()

    section("session — a controller that goes away, comes back, or is slow")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.ack_delay_s = 1.6
        check(write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, "a command is taken")
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and state(bridge)[1].flags & rhal.FLAG_DEGRADED, 4.0), "a controller that does not acknowledge it for a second is ETIMEDOUT and degraded")
        sim.ack_delay_s = 0.0
        code, said = write(bridge, cmd(pose(0.1)))
        check(code == rhal.SKR_EIO and "has not acknowledged a command for" in said and "it is hung or the line is" in said, "and takes no more commands: EIO, saying it is hung or the line is", said)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 4.0), "when it answers, it is healthy again")
        check(until(lambda: write(bridge, cmd(pose(0.1)))[0] == rhal.SKR_OK, 3.0), "and takes commands")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.stop()
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and state(bridge)[1].flags & rhal.FLAG_DEGRADED, 6.0), "a controller that goes away is ETIMEDOUT and degraded")
        code, said = write(bridge, cmd(pose(0.1)))
        check(code in (rhal.SKR_EIO, rhal.SKR_ETIMEDOUT) and "not sent" in said or "did not take the command" in said, "and takes no command", said)
        status = ext(bridge, "status")
        check(status[0] == rhal.SKR_OK and status[1]["link_dead"], "`status` still answers, and says the link is dead", str(status)[:200])
        began = time.monotonic()
        check(bridge.close() == rhal.SKR_OK and time.monotonic() - began < 3.0, f"a close does not wait for a controller that is gone ({time.monotonic() - began:.2f} s)")
    finally:
        sim.stop()

    section("session — what the controller says is bounded")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        for code in range(100, 200):
            sim.raise_error(code)
        check(until(lambda: _errors_of(bridge)[-1:] == [199]), "a hundred error codes arrive")
        errors = _errors_of(bridge)
        check(len(errors) == 64 and errors[0] == 136 and errors[-1] == 199, "of which the last sixty-four are kept", f"{len(errors)} {errors[:2]}")
        check(len(ext(bridge, "status")[1]["errors"]) == 64, "`status` lists them all")
        for number in range(50):
            sim.raise_warn(500 + number)
        check(until(lambda: _warn_seq(bridge) == 50), "fifty warnings arrive")
        with bridge._lock:  # noqa: SLF001
            kept = list(bridge._warns)  # noqa: SLF001
        check(len(kept) == 32 and kept[0][0] == 19 and kept[-1][0] == 50 and kept[-1][2] == 549, "of which the last thirty-two are kept, with their sequence numbers", f"{len(kept)} {kept[:1]} {kept[-1:]}")
        check(len(ext(bridge, "status")[1]["warnings"]) == 5, "`status` shows the last five")
        for number in range(20):
            sim.say(f"error[msg][message {number}]")
        check(until(lambda: len(ext(bridge, "status")[1]["messages"]) == 16 and ext(bridge, "status")[1]["messages"][-1] == "error[msg]: message 19"), "twenty messages with no code arrive, and the last sixteen are kept")
        sim.say("error[msg][" + "x" * 500 + "]")
        check(until(lambda: len(ext(bridge, "status")[1]["messages"][-1]) == len("error[msg]: ") + 200), "one of them is cut at two hundred characters")
        bridge.close()
    finally:
        sim.stop()

    section("session — noise on the data port is skipped")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.data_garbage = True
        sim.data_extra_packet = True
        began = state(bridge)[1].seq
        check(until(lambda: state(bridge)[1].seq > began + 20), "bytes that are not a packet, and a packet of another type, before every state packet")
        check(all(state(bridge)[0] == rhal.SKR_OK for _ in range(20)) or until(lambda: state(bridge)[0] == rhal.SKR_OK), "do not stop the state being read")
        with sim._lock:  # noqa: SLF001
            sim.ref = [5.0] * 6
        check(until(lambda: abs(state(bridge)[1].position_rad[0] - 5.0 * DEG) < 1e-4), "or being right", str(list(state(bridge)[1].position_rad)))
        bridge.close()
    finally:
        sim.stop()

    section("session — commands, reads and requests from several threads")
    sim = new_sim()
    try:
        bridge = opened(sim)
        check(ready(bridge), "ready")
        errors: list[str] = []
        results = {"ok": 0}
        lock = threading.Lock()
        stop_reading = threading.Event()

        def writer(joint: int) -> None:
            mask = [1 if j == joint else 0 for j in range(6)]
            for n in range(60):
                try:
                    code = bridge.write_command(cmd(pose(0.001 * (n + 1)), mask=mask), 0)
                except Exception as error:  # noqa: BLE001
                    errors.append(f"write {joint}: {error!r}")
                    return
                if code == rhal.SKR_OK:
                    with lock:
                        results["ok"] += 1

        def reader() -> None:
            while not stop_reading.is_set():
                try:
                    state(bridge)
                    bridge.extension(TOKEN, b"status")
                except Exception as error:  # noqa: BLE001
                    errors.append(f"read: {error!r}")
                    return

        with contextlib.redirect_stderr(io.StringIO()):
            threads = [threading.Thread(target=writer, args=(j,), daemon=True) for j in range(4)]
            watcher = threading.Thread(target=reader, daemon=True)
            watcher.start()
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join(20.0)
            stop_reading.set()
            watcher.join(5.0)
        check(not errors, "four threads writing and one reading raise nothing", "; ".join(errors[:3]))
        check(results["ok"] == 240, "and every command is taken", str(results))
        check(until(lambda: len(servo_texts(sim)) == 240, 8.0), "and every one reaches the controller")
        shape = re.compile(r"^move_servo_j\(jnt\[[^\],]+(,[^\],]+){5}\],[^,)]+,[^,)]+,[^,)]+,[^,)]+\)$")
        check(all(shape.match(line) for line in servo_texts(sim)), "whole, with no two interleaved on the line", str([l for l in servo_texts(sim) if not shape.match(l)][:2]))
        bridge.close()
    finally:
        sim.stop()

    section("session — the entry point serves a simulated controller only when asked to")
    entry = importlib.import_module("skynet_rainbow.__main__")
    check(tuple(entry.ROLES) == frames.JOINTS and all(entry.ROLES.values()) and entry.LIMITS == LIMITS, "the fixture asserts a role for every joint, and a range")
    sim = new_sim()
    try:
        bridge = entry._SimulatedBridge(sim)  # noqa: SLF001
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.open("")
        check(code == rhal.SKR_OK, "an empty configuration is filled in with the simulated controller's address, a model, ranges and roles, and opens", said.getvalue())
        description = json.loads(bridge.describe())
        check(description["simulator"] is True and description["robot_slug"] == "rainbow-rb5-850" and description["joints"][1]["role"] == "right_arm.shoulder.pitch" and description["joints"][1]["limits"] == {"position_min_rad": -3.0, "position_max_rad": 3.0},
              "as a simulation, with the fixture's roles and ranges", str(description)[:200])
        bridge.close()
        with contextlib.redirect_stderr(io.StringIO()):
            code = bridge.open(json.dumps({"model": "RB10-1300", "limits": [[-1, 1]] * 6, "roles": {}}))
        check(code == rhal.SKR_OK and json.loads(bridge.describe())["robot_slug"] == "rainbow-rb10-1300" and json.loads(bridge.describe())["joints"][0]["limits"]["position_max_rad"] == 1.0, "what the configuration says is kept")
        bridge.close()
        for text, wanted in (("not json", rhal.SKR_EINVAL), ("[1]", rhal.SKR_EINVAL)):
            with contextlib.redirect_stderr(io.StringIO()):
                check(bridge.open(text) == wanted, f"{text!r} is refused as the base bridge refuses it")
            bridge.close()
    finally:
        sim.stop()
    with mock.patch.dict(os.environ, {"SKYNET_RAINBOW_SIM": ""}), mock.patch.object(entry.rhal, "serve", lambda bridge, **kw: (kw, type(bridge))) as served:
        result = entry.main()
        check(result[1] is RainbowBridge and result[0]["ceiling"] == "guest" and result[0]["name"] == "rainbow", "without SKYNET_RAINBOW_SIM the bridge served is the real one, at the guest tier", str(result))
        del served
    for value in ("0", "true", " "):
        with mock.patch.dict(os.environ, {"SKYNET_RAINBOW_SIM": value}), mock.patch.object(entry.rhal, "serve", lambda bridge, **kw: type(bridge)):
            check(entry.main() is RainbowBridge, f"SKYNET_RAINBOW_SIM={value!r} is not the opt-in: only 1 is")
    started: list[SimulatedController] = []
    original = entry.SimulatedController

    class Recording(original):  # type: ignore[misc, valid-type]
        def start(self):
            started.append(self)
            return super().start()

    with mock.patch.dict(os.environ, {"SKYNET_RAINBOW_SIM": "1"}), mock.patch.object(entry, "SimulatedController", Recording), mock.patch.object(entry.rhal, "serve", lambda bridge, **kw: bridge), contextlib.redirect_stderr(io.StringIO()) as notice:
        served_bridge = entry.main()
        try:
            check(isinstance(served_bridge, entry._SimulatedBridge) and len(started) == 1 and "SKYNET_RAINBOW_SIM=1" in notice.getvalue() and "not an arm" in notice.getvalue(), "SKYNET_RAINBOW_SIM=1 serves the simulated controller and says on stderr that it is not an arm", notice.getvalue())  # noqa: SLF001
        finally:
            for controller in started:
                controller.stop()

def main() -> int:
    only = os.environ.get("SELFTEST_ONLY", "")
    for name in ("main_frames", "main_links", "main_open", "main_state", "main_commands", "main_stop", "main_motions", "main_session"):
        if name in globals() and (not only or only == name[5:]):
            globals()[name]()
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAILED: {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

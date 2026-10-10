"""The Tello bridge against a simulated drone written here (DRY-COVERAGE-PLAN.md item 12.E.11): the frames, the sockets, what it refuses to open on, the state, the refusals, the motions, the stop and the session.

    PYTHONPATH=../../bindings/python:. python3 selftest.py

Needs python3 on Linux and nothing of the maker's. The simulated drone agrees with whoever wrote it (F15); `layout_check.py` holds the driver's commands and its reading of the state string to djitellopy, which is run, and
what this shows is the driver's own logic.

Every wait is for a state (`until`), never for a count of polls or a sleep that something else is supposed to have finished inside: a loaded machine stretches every sleep (F210, `tools/flake-hunt.sh`). Private state
is set only on a bridge that has no thread alive to change it.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import random
import socket
import sys
import threading
import time
from unittest import mock

import skynet_rhal as rhal

from skynet_tello import bridge as bridge_module, frames, link
from skynet_tello.bridge import TelloBridge, TOKEN
from skynet_tello.drone_sim import DroneSimulator

CHECKS = 0
FAILURES: list[str] = []
SCALE = 20.0           # the simulated drone flies this much faster than its speed says


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


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def new_sim(**kw) -> DroneSimulator:
    kw.setdefault("time_scale", SCALE)
    kw.setdefault("state_port", free_port())
    return DroneSimulator(**kw).start()


def settings(sim: DroneSimulator, **extra) -> dict:
    out = {"host": "127.0.0.1", "port": sim.command_port, "state_port": sim.state_port, "simulator": True, "stop_mode": "hover", "keepalive_s": 0, "reply_timeout_ms": 1500, "takeoff_timeout_ms": 5000, "stale_ms": 1000}
    out.update(extra)
    return {k: v for k, v in out.items() if v is not None}


def config(sim: DroneSimulator, **extra) -> str:
    return json.dumps(settings(sim, **extra))


def opened(sim: DroneSimulator, **extra) -> TelloBridge:
    bridge = TelloBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge


def opens_with(sim: DroneSimulator, **extra) -> tuple[int, str, TelloBridge]:
    bridge = TelloBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    return code, said.getvalue(), bridge


def refused(text: str) -> tuple[int, str]:
    bridge = TelloBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(text)
    bridge.close()
    return code, said.getvalue()


def state(bridge: TelloBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def cmd(values: list[float], mask: list[int] | None = None, mode: int = rhal.MODE_POSITION, count: int | None = None) -> rhal.Command:
    n = count if count is not None else len(values)
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=list(values), mask=mask if mask is not None else [1] * n)


def write(bridge: TelloBridge, command: rhal.Command) -> tuple[int, str]:
    bridge._last_refusal = None  # noqa: SLF001 - a refusal is said once for a run of the same cause; each check asks to hear it
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def ext(bridge: TelloBridge, text: str) -> tuple[int, object]:
    code, body = bridge.extension(TOKEN, text.encode())
    try:
        return code, json.loads(body)
    except json.JSONDecodeError:
        return code, body.decode()


def sent(sim: DroneSimulator, prefix: str) -> int:
    return len(sim.seen(prefix))


def flying(sim: DroneSimulator) -> bool:
    return sim.flying and sim.z > 10.0


def threads_named(prefix: str) -> list[threading.Thread]:
    """The bridge's threads (the simulator's are `tello-sim-...`)."""
    return [t for t in threading.enumerate() if t.name.startswith(prefix) and not t.name.startswith("tello-sim") and t.is_alive()]


def real_state(**changes) -> frames.State:
    base = dict(pitch=0, roll=0, yaw=0, vgx=0, vgy=0, vgz=0, templ=60, temph=62, tof=10, h=0, bat=87, baro=-80.25, time=0, agx=0.0, agy=0.0, agz=-1000.0)
    base.update(changes)
    return frames.State(**base)


# ── the frames ───────────────────────────────────────────────────────────────────────────────────

def main_frames() -> None:
    section("frames — the commands")
    check(frames.command() == "command" and frames.takeoff() == "takeoff" and frames.land() == "land" and frames.emergency() == "emergency" and frames.stop() == "stop", "the commands that take nothing are the guide's words")
    for direction in frames.DIRECTIONS:
        check(frames.move(direction, 20) == f"{direction} 20" and frames.move(direction, 500) == f"{direction} 500" and frames.move(direction, 137) == f"{direction} 137", f"`{direction} x` is the direction, a space and the whole number")
        check(raises(frames.ProtocolError, frames.move, direction, 19) is not None and raises(frames.ProtocolError, frames.move, direction, 501) is not None, f"`{direction}` is refused below 20 and above 500 cm")
    check(frames.DIRECTIONS == ("up", "down", "left", "right", "forward", "back"), "the six directions are the guide's")
    check("direction" in (raises(frames.ProtocolError, frames.move, "sideways", 50) or "") and raises(frames.ProtocolError, frames.move, "forward ", 50) is not None, "a word that is not a direction is refused")
    for bad in (True, False, 50.0, "50", None, 50.5, float("nan")):
        check("whole number" in (raises(frames.ProtocolError, frames.move, "up", bad) or ""), f"a distance of {bad!r} is not a whole number and is refused, not rounded")
    check(frames.rotate(True, 1) == "cw 1" and frames.rotate(True, 360) == "cw 360" and frames.rotate(False, 1) == "ccw 1" and frames.rotate(False, 360) == "ccw 360" and frames.rotate(False, 90) == "ccw 90", "`cw x` and `ccw x` from 1 to 360")
    check(all(raises(frames.ProtocolError, frames.rotate, sense, value) is not None for sense in (True, False) for value in (0, 361, -1, -90)), "an angle of 0, or above 360, or negative is refused for both senses")
    check(raises(frames.ProtocolError, frames.rotate, True, 90.0) is not None and raises(frames.ProtocolError, frames.rotate, True, True) is not None, "an angle that is a float or a bool is refused")
    check([frames.flip(d) for d in "lrfb"] == ["flip l", "flip r", "flip f", "flip b"] and raises(frames.ProtocolError, frames.flip, "x") is not None and raises(frames.ProtocolError, frames.flip, "left") is not None, "`flip x` takes l, r, f or b")
    check(frames.speed(10) == "speed 10" and frames.speed(100) == "speed 100" and frames.speed(55) == "speed 55" and raises(frames.ProtocolError, frames.speed, 9) is not None and raises(frames.ProtocolError, frames.speed, 101) is not None, "`speed x` from 10 to 100")
    check(frames.rc(0, 0, 0, 0) == "rc 0 0 0 0" and frames.rc(-100, 100, -100, 100) == "rc -100 100 -100 100" and frames.rc(1, 2, 3, 4) == "rc 1 2 3 4", "`rc a b c d`: left/right, forward/backward, up/down, yaw")
    for index in range(4):
        values = [0, 0, 0, 0]
        values[index] = 101
        high = list(values)
        values[index] = -101
        check(raises(frames.ProtocolError, frames.rc, *high) is not None and raises(frames.ProtocolError, frames.rc, *values) is not None, f"channel {index + 1} of `rc` is refused past 100 either way")
    check(raises(frames.ProtocolError, frames.rc, 0, 0, 0, 1.5) is not None and raises(frames.ProtocolError, frames.rc, True, 0, 0, 0) is not None, "an `rc` channel that is a float or a bool is refused")
    check(frames.READS == {"speed": "speed?", "battery": "battery?", "time": "time?", "wifi": "wifi?", "sdk": "sdk?", "serial": "sn?"}, "the read commands are the guide's")
    check(all(frames.read(key) == text for key, text in frames.READS.items()) and raises(frames.ProtocolError, frames.read, "height") is not None, "a read is looked up in that table and nothing else")
    check((frames.COMMAND_PORT, frames.STATE_PORT, frames.FACTORY_HOST) == (8889, 8890, "192.168.10.1"), "the ports and the address are the guide's")
    check((frames.MOVE_CM, frames.ROTATE_DEG, frames.SPEED_CM_S, frames.RC_RANGE) == ((20, 500), (1, 360), (10, 100), (-100, 100)), "the ranges are the guide's")

    section("frames — what the drone answers")
    for raw, kind in ((b"ok", "ok"), (b"ok\r\n", "ok"), (b"OK", "ok"), (b" ok \r\n", "ok"), (b"error", "error"), (b"error Not joystick", "error"), (b"ERROR", "error"), (b"error\r\n", "error"), (b"unknown command: foo", "error"),
                      (b"Unknown command", "error"), (b"out of range", "error"), (b"87", "value"), (b"87\r\n", "value"), (b"0TQDG9RED00000", "value"), (b"20", "value"), (b"12s", "value"), (b"", "value"), (b"okay", "value")):
        reply = frames.parse_reply(raw)
        check(reply.kind == kind, f"{raw!r} is {kind}", reply.kind)
    check(frames.parse_reply(b"87\r\n").text == "87" and frames.parse_reply(b"error Motor stop\r\n").text == "error Motor stop", "the text of a reply loses its line end and nothing else")
    check(frames.parse_reply(b"\xff\xfe").kind == "error" and frames.parse_reply(b"ok\xff").kind == "error", "bytes that are not text are an error reply, and never raise")

    section("frames — the state string")
    text = "pitch:0;roll:0;yaw:-12;vgx:0;vgy:0;vgz:0;templ:60;temph:62;tof:10;h:0;bat:87;baro:-80.25;time:0;agx:-1.00;agy:2.00;agz:-999.00;\r\n"
    parsed = frames.parse_state(text)
    check((parsed.pitch, parsed.roll, parsed.yaw, parsed.bat, parsed.h, parsed.tof, parsed.templ, parsed.temph, parsed.time) == (0, 0, -12, 87, 0, 10, 60, 62, 0) and parsed.baro == -80.25 and (parsed.agx, parsed.agy, parsed.agz) == (-1.0, 2.0, -999.0),
          "a state string the way a Tello streams it", str(parsed))
    check(parsed.pad is None and parsed.extra == {}, "with no mission pad and nothing else in it")
    check(frames.parse_state(text.encode()) == parsed, "bytes and text read alike")
    check(frames.parse_state(text.replace("baro:-80.25;time", "baro:-80.25; time")) == parsed, "the guide's space before `time` is no matter")
    padded = frames.parse_state("mid:3;x:10;y:-20;z:30;mpry:1,2,3;" + text)
    check(padded.pad == (3, 10, -20, 30) and padded.extra == {"mpry": "1,2,3"} and padded.h == 0, "with a mission pad detected, its id and coordinates, and a field newer than the guide kept as text")
    check(frames.parse_state("mid:-1;x:0;y:0;z:0;" + text).pad == (-1, 0, 0, 0), "a pad that is not detected is `mid:-1` and zeros")
    check(frames.parse_state(text + "future:abc;").extra == {"future": "abc"}, "a field the guide does not list is kept and not read")
    for name in frames.STATE_INTS + frames.STATE_FLOATS:
        without = ";".join(part for part in text.strip().split(";") if not part.startswith(name + ":")) + ";"
        check(f"`{name}`" in (raises(frames.ProtocolError, frames.parse_state, without) or ""), f"a state string with no `{name}` is refused, naming it")
    check("`vgy`" in (raises(frames.ProtocolError, frames.parse_state, text.replace("vgy:0", "vgy0")) or ""), "the guide's own `vgy%d`, with no colon, is not what a Tello sends and is not read")
    for name, wrong in (("h", "tall"), ("bat", "87.5"), ("baro", "low"), ("yaw", ""), ("agz", "1e")):
        mangled = ";".join(f"{name}:{wrong}" if part.startswith(name + ":") else part for part in text.split(";"))
        check(f"`{name}` is not a number" in (raises(frames.ProtocolError, frames.parse_state, mangled) or ""), f"a `{name}` of {wrong!r} is not a number and is refused")
    for bad in ("", "ok", "no fields here", ";;;", b"\xc3\x28"):
        check(raises(frames.ProtocolError, frames.parse_state, bad) is not None, f"{bad!r} is not a state string")
    check("mission pad" in (raises(frames.ProtocolError, frames.parse_state, "mid:3;x:a;y:0;z:0;" + text) or "") and "mission pad" in (raises(frames.ProtocolError, frames.parse_state, "mid:3;x:0;y:0;" + text) or ""),
          "a mission pad whose fields are not all whole numbers is refused")
    rng = random.Random(7)
    for trial in range(40):
        original = frames.State(pitch=rng.randint(-90, 90), roll=rng.randint(-90, 90), yaw=rng.randint(-180, 180), vgx=rng.randint(-100, 100), vgy=rng.randint(-100, 100), vgz=rng.randint(-100, 100), templ=rng.randint(0, 90),
                                temph=rng.randint(0, 90), tof=rng.randint(0, 3000), h=rng.randint(0, 3000), bat=rng.randint(0, 100), baro=round(rng.uniform(-100, 100), 2), time=rng.randint(0, 5000),
                                agx=round(rng.uniform(-1000, 1000), 2), agy=round(rng.uniform(-1000, 1000), 2), agz=round(rng.uniform(-1000, 1000), 2), pad=(rng.randint(-1, 8), rng.randint(-50, 50), rng.randint(-50, 50), rng.randint(0, 50)) if trial % 2 else None)
        again = frames.parse_state(frames.encode_state(original))
        if again != original:
            check(False, "a state string made by encode_state is read back whole", f"{original} -> {again}")
            break
    else:
        check(True, "forty random state strings, with and without a mission pad, are read back whole")
    check(frames.encode_state(real_state()).endswith(";\r\n") and "vgy:0;" in frames.encode_state(real_state()) and "baro:-80.25;" in frames.encode_state(real_state()), "the string made has the colon after `vgy`, two decimals on `baro` and the guide's line end")
    check(raises(AttributeError, setattr, parsed, "h", 5) is not None or raises(Exception, setattr, parsed, "h", 5) is not None, "a state is not changed once it is read")


# ── the sockets ──────────────────────────────────────────────────────────────────────────────────

def main_links() -> None:
    section("links — the command socket and the state socket")
    drone = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    drone.bind(("127.0.0.1", 0))
    drone.settimeout(2.0)
    state_port = free_port()
    udp = link.UdpLink("127.0.0.1", drone.getsockname()[1], state_port)
    check(not udp.is_open and udp.local_port == 0 and udp.target == f"127.0.0.1:{drone.getsockname()[1]}", "a link that is not open says so, has no local port and names its target")
    check("not open" in (raises(link.LinkError, udp.send, "command") or ""), "a send on a link that is not open is a LinkError")
    check(udp.poll(0.0) == ([], [], False), "a poll of a link that is not open returns nothing and does not raise")
    udp.open()
    try:
        check(udp.is_open and udp.local_port > 0, "an open link has both sockets and a local port of its own")
        udp.send("command")
        data, address = drone.recvfrom(1024)
        check(data == b"command" and address[1] == udp.local_port, "a send is one datagram of the text, from the local port")
        udp.send("rc 0 50 0 0")
        check(drone.recvfrom(1024)[0] == b"rc 0 50 0 0", "and the next is another")
        drone.sendto(b"ok\r\n", address)
        replies, states, woke = udp.poll(1.0)
        check(replies == [b"ok\r\n"] and states == [] and woke is False, "a reply from the drone's address comes back from the command socket")
        drone.sendto(b"pitch:0;", ("127.0.0.1", state_port))
        replies, states, woke = udp.poll(1.0)
        check(replies == [] and states == [b"pitch:0;"], "a state string comes back from the state socket, apart from the replies")
        drone.sendto(b"one", address)
        drone.sendto(b"two", address)
        drone.sendto(b"three", ("127.0.0.1", state_port))
        time.sleep(0.05)
        replies, states, _ = udp.poll(1.0)
        check(replies == [b"one", b"two"] and states == [b"three"], "several datagrams that have arrived are all returned, in order")
        started = time.monotonic()
        check(udp.poll(0.15) == ([], [], False) and time.monotonic() - started >= 0.12, "a poll with nothing to read waits as long as it was given and returns nothing")
        started = time.monotonic()
        udp.poll(0.0)
        check(time.monotonic() - started < 0.05, "a poll of nothing is a look")
        # a datagram from an address that is not the drone's
        stranger = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        stranger.bind(("127.0.0.2", 0))
        stranger.sendto(b"ok", ("127.0.0.1", udp.local_port))
        stranger.sendto(b"pitch:0;", ("127.0.0.1", state_port))
        time.sleep(0.05)
        check(udp.poll(0.5) == ([], [], False) and udp.foreign == 2, "a datagram from any other address is dropped and counted, on both sockets", str(udp.foreign))
        stranger.close()
        drone.sendto(b"x" * 1024, address)
        drone.sendto(b"y" * 1025, address)
        drone.sendto(b"z" * 5000, ("127.0.0.1", state_port))
        time.sleep(0.05)
        replies, states, _ = udp.poll(1.0)
        check(replies == [b"x" * 1024] and states == [] and udp.oversize == 2, "a datagram of 1,024 bytes is read whole and one of 1,025 or more is dropped and counted, not cut", f"{[len(r) for r in replies]} {udp.oversize}")
        read_end, write_end = os.pipe()
        try:
            os.write(write_end, b"x")
            started = time.monotonic()
            replies, states, woke = udp.poll(2.0, (read_end,))
            check(woke is True and replies == [] and time.monotonic() - started < 0.5, "a readable wake descriptor ends the wait at once and is reported")
            os.read(read_end, 8)
            check(udp.poll(0.05, (read_end,)) == ([], [], False), "and one that is not readable is not")
        finally:
            os.close(read_end)
            os.close(write_end)
    finally:
        udp.close()
    check(not udp.is_open and udp.local_port == 0, "a closed link is not open")
    udp.close()
    check(not udp.is_open, "a link that is closed twice is closed")
    check("not open" in (raises(link.LinkError, udp.send, "command") or ""), "a send on a closed link is a LinkError")
    udp.open()
    first = udp.local_port
    udp.open()
    check(udp.is_open and udp.local_port > 0, "an open on an open link closes the sockets it replaces and makes new ones")
    udp.close()
    del first

    section("links — a port that is taken, a port of the operator's, a network that will not take it")
    holder = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    holder.bind(("", 0))
    taken = holder.getsockname()[1]
    busy = link.UdpLink("127.0.0.1", 8889, taken)
    check("state port" in (raises(link.LinkError, busy.open) or "") and not busy.is_open, "a state port that is taken is a LinkError that names it, and leaves nothing open")
    busy = link.UdpLink("127.0.0.1", 8889, free_port(), taken)
    check("local port" in (raises(link.LinkError, busy.open) or ""), "and so is a local port that is taken")
    holder.close()
    fixed = free_port()
    pinned = link.UdpLink("127.0.0.1", 8889, free_port(), fixed)
    pinned.open()
    check(pinned.local_port == fixed, "a local port of the operator's is the one bound")
    pinned.close()
    unreachable = link.UdpLink("127.0.0.1", 9, free_port())
    unreachable.open()
    try:
        unreachable.send("command")
        check(True, "a send to a port nothing listens on is not an error: it is a datagram")
    except link.LinkError:
        check(False, "a send to a port nothing listens on is not an error")
    finally:
        unreachable.close()
    nowhere = link.UdpLink("127.0.0.1", 99999, free_port())
    nowhere.open()
    check(raises(link.LinkError, nowhere.send, "command") is not None, "a send the network refuses (a port that is not one) is a LinkError and not an exception of the socket's")
    nowhere.close()
    drone.close()


# ── the open ─────────────────────────────────────────────────────────────────────────────────────

def main_open() -> None:
    section("open — what it refuses to open on")
    code, said = refused("not json")
    check(code == rhal.SKR_EINVAL and "not JSON" in said, "a configuration that is not JSON is EINVAL")
    code, said = refused("[1]")
    check(code == rhal.SKR_EINVAL and "JSON object" in said, "a configuration that is not an object is EINVAL")
    sim = new_sim()
    try:
        good = settings(sim)
        for key, value, word in (("host", None, "host"), ("host", "tello.local", "IPv4"), ("host", "192.168.10", "IPv4"), ("host", "300.1.1.1", "IPv4"), ("host", 12, "IPv4"), ("host", "", "IPv4"),
                                 ("port", 0, "port"), ("port", 70000, "port"), ("port", True, "port"), ("port", "8889", "port"), ("port", 88.5, "port"),
                                 ("state_port", 0, "state_port"), ("state_port", 65536, "state_port"), ("state_port", False, "state_port"), ("local_port", -1, "local_port"), ("local_port", 65536, "local_port"), ("local_port", True, "local_port"),
                                 ("simulator", None, "simulator"), ("simulator", "yes", "simulator"), ("simulator", 1, "simulator"),
                                 ("stop_mode", None, "stop_mode"), ("stop_mode", "stop", "stop_mode"), ("stop_mode", "Hover", "stop_mode"), ("stop_mode", "", "stop_mode"),
                                 ("keepalive_s", -1, "keepalive_s"), ("keepalive_s", 11, "keepalive_s"), ("keepalive_s", True, "keepalive_s"), ("keepalive_s", "5", "keepalive_s"),
                                 ("reply_timeout_ms", 99, "reply_timeout_ms"), ("reply_timeout_ms", 60001, "reply_timeout_ms"), ("reply_timeout_ms", True, "reply_timeout_ms"),
                                 ("takeoff_timeout_ms", 999, "takeoff_timeout_ms"), ("takeoff_timeout_ms", 120001, "takeoff_timeout_ms"), ("stale_ms", 99, "stale_ms"), ("stale_ms", 30001, "stale_ms"),
                                 ("low_battery_percent", 0, "low_battery_percent"), ("low_battery_percent", 51, "low_battery_percent"), ("open_timeout_ms", 99, "open_timeout_ms"), ("open_timeout_ms", 60001, "open_timeout_ms")):
            body = dict(good)
            if value is None:
                body.pop(key, None)
            else:
                body[key] = value
            code, said = refused(json.dumps(body))
            check(code == rhal.SKR_ENODEV and word in said, f"`{key}` of {value!r} is refused, naming it", said[:120])
        code, said = refused(json.dumps({**good, "joints": 6}))
        check(code == rhal.SKR_ENODEV and "'joints'" in said and "does not read" in said, "a key the driver does not read is refused, naming it")
        check(sent(sim, "") == 0, "and none of those refusals sent the drone a thing")
        for key, value in (("keepalive_s", 0), ("keepalive_s", 10), ("keepalive_s", 0.5), ("reply_timeout_ms", 100), ("stale_ms", 100), ("low_battery_percent", 50), ("local_port", free_port()), ("stop_mode", "land"), ("stop_mode", "emergency")):
            drone = new_sim()
            try:
                code, said, bridge = opens_with(drone, **{key: value})
                check(code == rhal.SKR_OK, f"`{key}` of {value!r} is a value it accepts", said[:120])
                bridge.close()
            finally:
                drone.stop()

        section("open — the handshake")
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_OK and said == "", "a drone that answers is opened without a word")
        check(sim.seen("command") == ["command"] and sim.commands[:4] == ["command", "sdk?", "sn?", "speed?"], "`command` is the first thing it says, and then what the drone says of itself", str(sim.commands[:5]))
        _, body = ext(bridge, "status")
        check(isinstance(body, dict) and body["sdk"] == "20" and body["serial"] == sim.serial and body["speed_cm_s"] == 10 and body["host"] == f"127.0.0.1:{sim.command_port}", "and the answers are kept", str(body)[:200])
        bridge.close()
    finally:
        sim.stop()

    silent = new_sim()
    try:
        silent.silent = True
        started = time.monotonic()
        code, said, bridge = opens_with(silent, open_timeout_ms=900)
        check(code == rhal.SKR_EIO and "did not take `command`" in said and time.monotonic() - started < 6.0, "a drone that says nothing is EIO that names the command, inside the time given", said[:160])
        check(sent(silent, "command") == 3, "after three attempts of the handshake and no more", str(silent.commands))
        check(not threads_named("tello-"), "and no thread is left behind")
        bridge.close()
    finally:
        silent.stop()
    for lost, wanted in ((1, rhal.SKR_OK), (2, rhal.SKR_OK), (3, rhal.SKR_EIO)):
        drone = new_sim()
        try:
            drone.drop_replies = lost
            code, said, bridge = opens_with(drone, open_timeout_ms=1500)
            check(code == wanted, f"{lost} lost repl{'y' if lost == 1 else 'ies'} to `command`: {'opened on the next attempt' if wanted == rhal.SKR_OK else 'refused'}", said[:140])
            if wanted == rhal.SKR_OK:
                check(sent(drone, "command") == lost + 1, "the handshake is sent again, because it is idempotent", str(drone.commands))
            bridge.close()
        finally:
            drone.stop()
    drone = new_sim()
    try:
        drone.command_reply = "error"
        code, said, bridge = opens_with(drone)
        check(code == rhal.SKR_EIO and "answered `command` with 'error'" in said and sent(drone, "command") == 1, "a drone that answers `command` with an error is EIO that says what it said, and is not asked again", said[:160])
        bridge.close()
    finally:
        drone.stop()
    drone = new_sim()
    try:
        drone.state_stalled = True
        code, said, bridge = opens_with(drone, open_timeout_ms=700)
        check(code == rhal.SKR_EIO and "no state string" in said and f"port {drone.state_port}" in said, "a drone that never streams its state is EIO that names the port", said[:200])
        bridge.close()
    finally:
        drone.stop()
    drone = new_sim()
    try:
        drone.state_garbage = True
        code, said, bridge = opens_with(drone, open_timeout_ms=700)
        check(code == rhal.SKR_EIO and "no state string" in said, "a drone whose state strings are not state strings is the same")
        bridge.close()
    finally:
        drone.stop()
    holder = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    holder.bind(("", 0))
    drone = new_sim(state_port=holder.getsockname()[1])
    try:
        code, said, bridge = opens_with(drone)
        check(code == rhal.SKR_EIO and "state port" in said and not threads_named("tello-"), "a state port that is taken is EIO that names it, with nothing left running", said[:160])
        bridge.close()
    finally:
        drone.stop()
        holder.close()
    with mock.patch.object(link.UdpLink, "send", side_effect=link.LinkError("Network is unreachable")):
        drone = new_sim()
        try:
            code, said, bridge = opens_with(drone, open_timeout_ms=500)
            check(code == rhal.SKR_EIO and "unreachable" in said.lower(), "a network that will not take a datagram is EIO that says so", said[:160])
            bridge.close()
        finally:
            drone.stop()

    section("open — the descriptor, and a session that starts clean")
    sim = new_sim()
    try:
        bridge = opened(sim)
        description = json.loads(bridge.describe())
        check(description["robot_slug"] == "ryze-tello" and description["base"] == "aerial" and description["dof"] == 0 and description["joints"] == [] and description["simulator"] is True and description["verified"] is False and description["digest"] is None,
              "the body is an aerial one with no joints, a simulator by the operator's word, and unsigned", str(description)[:200])
        check(description["capabilities"] == ["safety.estop"] and description["extensions"] == [TOKEN] and description["envelope"] is None and description["control"]["modes"] == [], "it has a stop, one extension, no envelope it made up and no control mode", str(description)[:200])
        check([m["name"] for m in description["motions"]] == ["status", "takeoff", "land", "hover", "move", "rotate", "speed", "drive"], "seven motions and the status are declared")
        by_name = {m["name"]: m["parameters"] for m in description["motions"]}
        check(by_name["status"] == [] and by_name["takeoff"] == [] and by_name["land"] == [] and by_name["hover"] == [], "those that take nothing say so")
        check([p["name"] for p in by_name["move"]] == ["up", "down", "left", "right", "forward", "back", "timeout"] and all(p["unit"] == "m" and p["min"] == 0.2 and p["max"] == 5.0 for p in by_name["move"][:6]), "`move` takes a distance in metres, from the guide's 20 cm to its 500")
        check([p["name"] for p in by_name["rotate"]] == ["cw", "ccw", "timeout"] and by_name["speed"] == [{"name": "v", "unit": "m/s", "required": True, "min": 0.1, "max": 1.0}], "`rotate` takes degrees and `speed` metres a second")
        check([p["name"] for p in by_name["drive"]] == ["left", "forward", "up", "yaw"] and all(p["unit"] == "%" and p["min"] == -100 and p["max"] == 100 for p in by_name["drive"]), "`drive` takes the four sticks in percent")
        bridge.close()
        code, said, flagged = opens_with(sim, simulator=False)
        check(code == rhal.SKR_OK and json.loads(flagged.describe())["simulator"] is False, "the claim that it is a real drone is carried into the descriptor, because nothing a Tello says tells the two apart")
        flagged.close()
        bridge = opened(sim)
        bridge.emergency_stop()
        until(lambda: bridge._stop_delivered)    # noqa: SLF001
        bridge.close()
        check(bridge.read_state(rhal.State(), 0) == rhal.SKR_EPERM, "a closed driver reads EPERM")
        with contextlib.redirect_stderr(io.StringIO()):
            check(bridge.open(config(sim)) == rhal.SKR_OK and not bridge._estop and not bridge._stop_delivered and bridge._stop_sent == 0, "a session opened after a stop starts with no stop, delivered or owed")    # noqa: SLF001
        _, body = ext(bridge, "status")
        check(isinstance(body, dict) and body["stop_latched"] is False and body["stop_sent"] == 0 and body["stray_replies"] >= 0, "and `status` says so")
        bridge.close()
        bridge = opened(sim)
        first_threads = len(threads_named("tello-"))
        with contextlib.redirect_stderr(io.StringIO()):
            check(bridge.open(config(sim)) == rhal.SKR_OK, "an open on an open driver is a new session")
        check(len(threads_named("tello-")) == first_threads, "that ends the old session's threads, and leaves one set", str([t.name for t in threads_named("tello-")]))
        bridge.close()
        check(until(lambda: not threads_named("tello-"), 3.0), "a close ends every thread")
        failed = TelloBridge()
        with contextlib.redirect_stderr(io.StringIO()):
            code = failed.open(json.dumps(settings(sim, simulator="maybe")))
        check(code == rhal.SKR_ENODEV and failed.read_state(rhal.State(), 0) == rhal.SKR_EPERM, "a driver whose open was refused is not open")
    finally:
        sim.stop()


# ── the state ────────────────────────────────────────────────────────────────────────────────────

def main_state() -> None:
    section("state — what a reading says")
    sim = new_sim()
    try:
        bridge = TelloBridge()
        check(bridge.read_state(rhal.State(), 0) == rhal.SKR_EPERM, "a driver that is not open reads EPERM")
        bridge = opened(sim, stale_ms=600)
        rc, reading = state(bridge)
        check(rc == rhal.SKR_OK and reading.flags == rhal.HOMING_NOT_REQUIRED and reading.fault_code == 0, "a drone on the ground that streams its state reads healthy, with no flag and no fault word", f"{rc} {reading.flags} {reading.fault_code}")
        check(len(reading.position_rad) == 0 and len(reading.velocity_rad_s) == 0 and len(reading.effort_nm) == 0, "and has no joints to report")
        check(len(reading.base_pose) == 7 and reading.base_pose[2] == 0.0 and all(math.isnan(v) for v in reading.base_pose[:2] + reading.base_pose[3:]), "its pose is the height beneath a position and an orientation it does not have")
        check(len(reading.base_twist) == 6 and all(math.isnan(v) for v in reading.base_twist), "and its twist is not known: the guide gives the speeds no unit")
        before = reading.seq
        _, again = state(bridge)
        check(again.seq == before + 1, "the sequence number advances at every read")
        check(again.stamp_ns >= reading.stamp_ns > 0, "and the stamp is the state string's arrival, not the read's")
        check(ext(bridge, "takeoff")[0] == rhal.SKR_OK and until(lambda: state(bridge)[1].base_pose[2] > 0.5), "after a takeoff the height is in the pose, in metres")
        check(until(lambda: abs(state(bridge)[1].base_pose[2] - sim.z / 100.0) < 0.05, 3.0), "and is the drone's own", f"{state(bridge)[1].base_pose[2]} {sim.z}")

        section("state — stale, dead, low and noisy")
        sim.state_stalled = True
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and state(bridge)[1].flags & rhal.FLAG_DEGRADED, 4.0), "a state that stops arriving is ETIMEDOUT and degraded inside the staleness bound")
        _, reading = state(bridge)
        check(not reading.flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP) and reading.fault_code == 0, "and is not a fault or a stop: the drone said nothing")
        sim.state_stalled = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 4.0), "and when it arrives again the state is healthy again, with no restart")
        bridge._dead = "Network is unreachable"    # noqa: SLF001 - what a failed send sets
        check(state(bridge)[0] == rhal.SKR_ETIMEDOUT and state(bridge)[1].flags & rhal.FLAG_DEGRADED, "a link that could not carry a datagram is ETIMEDOUT and degraded however fresh the state is")
        bridge._dead = None    # noqa: SLF001
        sim.battery = 9.0
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0) and state(bridge)[0] == rhal.SKR_OK, "a battery at 9% is degraded and the read is still OK: the drone is flying and saying so")
        _, reading = state(bridge)
        check(reading.fault_code == 0 and not reading.flags & rhal.FLAG_FAULT, "with no fault word: the guide gives the drone none")
        sim.battery = 50.0
        check(until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0), "and a battery at 50% is not")
        sim.battery = 10.0
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0), "10% is the floor and is degraded (the operator's `low_battery_percent`, 10 unless said)")
        sim.battery = 80.0
        until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0)
        sim.state_garbage = True
        check(until(lambda: bridge._bad_states >= 3, 3.0), "state strings that are not state strings are counted")    # noqa: SLF001
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 4.0), "and do not keep the state fresh: garbage is not a reading")
        sim.state_garbage = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0), "the stream comes back when the drone sends sense")
        sim.state_override = {"pitch": 0, "roll": 0, "yaw": 0}
        check(until(lambda: bridge._bad_states >= 6, 3.0), "a state string with fields missing is garbage too")    # noqa: SLF001
        sim.state_override = None
        until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0)
        stranger = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        stranger.bind(("127.0.0.2", 0))
        stranger.sendto(frames.encode_state(real_state(h=3000, bat=1)).encode(), ("127.0.0.1", sim.state_port))
        check(until(lambda: bridge._link.foreign >= 1, 2.0), "a state string from an address that is not the drone's is dropped and counted")    # noqa: SLF001
        rc, reading = state(bridge)
        check(rc == rhal.SKR_OK and reading.base_pose[2] < 29.0 and not reading.flags & rhal.FLAG_DEGRADED, "and is not believed: the height is not 30 m and the battery is not at 1%")
        stranger.close()
        _, body = ext(bridge, "status")
        check(isinstance(body, dict) and body["foreign_datagrams"] >= 1 and body["bad_states"] >= 6 and body["states"] > 10, "`status` counts what was dropped", str(body)[:240])
        bridge.close()

        section("state — the operator's battery floor")
        sim.battery = 25.0
        bridge = opened(sim, low_battery_percent=30)
        check(until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0), "a battery below the operator's own floor is degraded")
        sim.battery = 35.0
        check(until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0), "and above it is not")
        bridge.close()
    finally:
        sim.stop()

    section("state — what a state string is")
    sim = new_sim()
    try:
        bridge = opened(sim)
        sim.state_override = {"pitch": 0, "roll": 0, "yaw": 0, "vgx": 0, "vgy": 0, "vgz": 0, "templ": 60, "temph": 62, "tof": 10, "h": 123, "bat": 87, "baro": "-80.25", "time": 0, "agx": "0.00", "agy": "0.00", "agz": "-1000.00"}
        check(until(lambda: abs(state(bridge)[1].base_pose[2] - 1.23) < 1e-9, 3.0), "the height is `h` in centimetres, as metres")
        sim.state_override = {**sim.state_override, "h": -5}
        check(until(lambda: abs(state(bridge)[1].base_pose[2] + 0.05) < 1e-9, 3.0), "and is what the drone said, even where that is below zero")
        _, body = ext(bridge, "status")
        check(isinstance(body, dict) and body["height_cm"] == -5 and body["battery_percent"] == 87 and body["temp_low_c"] == 60 and body["temp_high_c"] == 62 and body["baro_cm"] == -80.25 and body["tof_cm"] == 10, "`status` gives the rest of the fields as they came", str(body)[:260])
        sim.state_override = {**sim.state_override, "mid": 4, "x": 11, "y": -12, "z": 40, "mpry": "1,2,3"}
        check(until(lambda: isinstance(ext(bridge, "status")[1], dict) and ext(bridge, "status")[1].get("mission_pad") == [4, 11, -12, 40], 3.0), "a mission pad is reported as the id and the three coordinates")
        check(ext(bridge, "status")[1]["other_fields"] == {"mpry": "1,2,3"}, "and a field newer than the guide is reported as text")
        bridge.close()
    finally:
        sim.stop()


# ── the commands and the extension ───────────────────────────────────────────────────────────────

def main_commands() -> None:
    section("commands — a joint command is not how a drone is moved")
    sim = new_sim()
    try:
        bridge = TelloBridge()
        check(write(bridge, cmd([]))[0] == rhal.SKR_EPERM, "a driver that is not open reads EPERM for a command")
        bridge = opened(sim)
        before = len(sim.commands)
        code, said = write(bridge, cmd([]))
        check(code == rhal.SKR_EPERM and "no joints" in said and TOKEN in said and "drive" in said, "a command with no joints is refused as EPERM, saying where a drone is moved", said[:200])
        code, said = write(bridge, cmd([], mode=rhal.MODE_VELOCITY))
        check(code == rhal.SKR_EPERM and TOKEN in said, "and so is a velocity one")
        check(write(bridge, cmd([], mode=rhal.MODE_TORQUE))[0] == rhal.SKR_EPERM, "and a torque one")
        check(write(bridge, cmd([0.1, 0.2, 0.3]))[0] == rhal.SKR_EINVAL, "a command that has joints is the wrong shape for this body: EINVAL")
        check(write(bridge, cmd([], mode=99))[0] == rhal.SKR_EINVAL, "a command in a mode there is not is EINVAL")
        code, said = write(bridge, cmd([]))
        check(code == rhal.SKR_EPERM and said != "", "the reason is said again after a different one")
        sim_commands = len(sim.commands)
        check(sim_commands == before, "none of them sent the drone a datagram", str(sim.commands[before:]))
        bridge.close()

        section("extension — the request")
        bridge = TelloBridge()
        check(bridge.extension(TOKEN, b"status")[0] == rhal.SKR_EPERM, "a driver that is not open reads EPERM for a motion")
        bridge = opened(sim)
        code, body = bridge.extension("vendor.other", b"status")
        check(code == rhal.SKR_ENOSYS and TOKEN.encode() in body, "a token that is not this driver's is ENOSYS, naming this driver's")
        check(bridge.extension(TOKEN, b"\xff\xfe")[0] == rhal.SKR_EINVAL, "a request that is not text is EINVAL")
        code, body = bridge.extension(TOKEN, b"   ")
        check(code == rhal.SKR_EINVAL and b"takeoff" in body and b"drive" in body, "an empty request is EINVAL and lists the motions")
        code, body = bridge.extension(TOKEN, b"move forward")
        check(code == rhal.SKR_EINVAL and b"k=v" in body, "an argument with no value is EINVAL")
        check(bridge.extension(TOKEN, b"move =3")[0] == rhal.SKR_EINVAL, "and so is one with no key")
        code, body = bridge.extension(TOKEN, b"loop")
        check(code == rhal.SKR_EINVAL and b"unknown motion" in body, "a motion there is not is EINVAL")
        for motion in ("status", "takeoff", "land", "hover"):
            code, body = bridge.extension(TOKEN, f"{motion} extra=1".encode())
            check(code == rhal.SKR_EINVAL and b"does not take" in body, f"`{motion}` takes no arguments")
        check(sent(sim, "takeoff") == 0 and sent(sim, "land") == 0 and sent(sim, "stop") == 0, "and none of those refusals sent the drone anything")
        bridge.close()
    finally:
        sim.stop()


# ── the motions ──────────────────────────────────────────────────────────────────────────────────

def main_motions() -> None:
    section("motions — takeoff, hover and land")
    sim = new_sim()
    try:
        bridge = opened(sim)
        code, body = ext(bridge, "takeoff")
        check(code == rhal.SKR_OK and isinstance(body, dict) and body["result"] == "airborne" and sim.flying and sim.z > 70, "a takeoff is the guide's `takeoff`, answered when the drone is up", str(body))
        check(sim.seen("takeoff") == ["takeoff"], "once")
        code, body = ext(bridge, "takeoff")
        check(code == rhal.SKR_EPERM and "answered `takeoff` with 'error'" in str(body) and sent(sim, "takeoff") == 2, "a second takeoff is refused by the drone, and the refusal says what it said; it is not sent again", str(body))
        code, body = ext(bridge, "hover")
        check(code == rhal.SKR_OK and body == {"result": "hovering"} and "stop" in sim.commands, "`hover` is the guide's `stop`")
        code, body = ext(bridge, "land")
        check(code == rhal.SKR_OK and body == {"result": "landed"} and not sim.flying and sim.z < 1, "`land` is the guide's `land`, answered when the drone is down")
        code, body = ext(bridge, "land")
        check(code == rhal.SKR_EPERM and "'error'" in str(body), "a landing of a drone that is down is refused by the drone")
        sim.error_next = "error Not ready"
        code, body = ext(bridge, "takeoff")
        check(code == rhal.SKR_EPERM and "'error Not ready'" in str(body), "the words the drone gave are the words in the refusal")
        sim.error_next = "87"
        code, body = ext(bridge, "takeoff")
        check(code == rhal.SKR_EIO and "neither `ok` nor an error" in str(body), "a value in answer to a control command is not an acknowledgement")
        bridge.close()

        section("motions — the battery, and a drone that is not being read")
        sim.battery = 5.0
        bridge = opened(sim)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0)
        count = len(sim.commands)
        for motion in ("takeoff", "move forward=0.5", "rotate cw=90", "speed v=0.5"):
            code, body = ext(bridge, motion)
            check(code == (rhal.SKR_EPERM if motion != "speed v=0.5" else rhal.SKR_OK) and (motion == "speed v=0.5" or "battery is at 5%" in str(body)), f"`{motion}` {'is refused' if motion != 'speed v=0.5' else 'is not refused'} at a battery of 5%", str(body))
        check(len(sim.commands) == count + 1 and sim.commands[-1] == "speed 50", "and the refusals sent nothing")
        sim.flying, sim.motors, sim.z = True, True, 50.0
        check(ext(bridge, "hover")[0] == rhal.SKR_OK and ext(bridge, "land")[0] == rhal.SKR_OK, "`hover` and `land` are never refused for a low battery")
        bridge.close()
        sim.battery = 90.0
        bridge = opened(sim)
        sim.state_stalled = True
        until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 4.0)
        count = len(sim.commands)
        for motion in ("takeoff", "land", "hover", "move forward=0.5", "rotate cw=10", "speed v=0.5", "drive forward=10"):
            code, body = ext(bridge, motion)
            check(code == rhal.SKR_EIO and "stale" in str(body), f"`{motion}` is refused while the state is stale", str(body))
        check(len(sim.commands) == count, "and nothing was sent")
        check(ext(bridge, "status")[0] == rhal.SKR_OK, "`status` is still answered")
        bridge.close()
    finally:
        sim.stop()

    section("motions — move")
    sim = new_sim()
    try:
        bridge = opened(sim)
        ext(bridge, "takeoff")
        x0, y0, z0, yaw0 = sim.position
        for text, wire, delta in (("forward=0.5", "forward 50", (50, 0, 0)), ("back=0.2", "back 20", (-20, 0, 0)), ("left=0.3", "left 30", (0, 30, 0)), ("right=0.25", "right 25", (0, -25, 0)), ("up=0.4", "up 40", (0, 0, 40)),
                                  ("down=0.2", "down 20", (0, 0, -20)), ("forward=5", "forward 500", (500, 0, 0)), ("up=0.2", "up 20", (0, 0, 20))):
            before = sim.position
            code, body = ext(bridge, f"move {text}")
            after = sim.position
            check(code == rhal.SKR_OK and sim.commands[-1] == wire and isinstance(body, dict) and body["result"] == "arrived" and body["cm"] == int(wire.split()[1]), f"`move {text}` is `{wire}`", str(body))
            if wire.startswith(("down", "up")):
                continue
            check(all(abs((after[i] - before[i]) - delta[i]) < 1.0 for i in range(3)), f"and the drone went {wire}", f"{before} -> {after}")
        del x0, y0, z0, yaw0
        count = len(sim.commands)
        for text, word in (("forward=0.455", "whole number of centimetres"), ("forward=0.19", "from 20 to 500"), ("forward=5.01", "from 20 to 500"), ("up=0.001", "whole number of centimetres"), ("left=-0.5", "from 20 to 500"), ("left=0", "from 20 to 500"),
                           ("forward=abc", "not a number"), ("forward=nan", "outside"), ("forward=inf", "outside"), ("forward=1e999", "outside")):
            code, body = ext(bridge, f"move {text}")
            check(code == rhal.SKR_EINVAL and word in str(body), f"`move {text}` is refused, as EINVAL, saying {word!r}", str(body))
        for text, word in (("", "exactly one"), ("timeout=5", "exactly one"), ("forward=0.5 back=0.5", "exactly one"), ("up=0.3 down=0.3 left=0.3", "exactly one"), ("sideways=0.5", "exactly one"), ("forward=0.5 sideways=1", "does not take"),
                           ("forward=0.5 timeout=0.5", "outside"), ("forward=0.5 timeout=301", "outside"), ("forward=0.5 timeout=x", "not a number")):
            code, body = ext(bridge, f"move {text}")
            check(code == rhal.SKR_EINVAL and word in str(body), f"`move {text}` is refused, saying {word!r}", str(body))
        check(len(sim.commands) == count, "and none of them sent the drone a thing")
        check(ext(bridge, "move forward=0.5 timeout=20")[0] == rhal.SKR_OK, "a timeout of the operator's is accepted")
        # a motion whose answer is lost is not sent again: the drone did it
        before = sent(sim, "left")
        sim.drop_replies = 1
        started = time.monotonic()
        code, body = ext(bridge, "move left=0.2 timeout=1.2")
        check(code == rhal.SKR_EIO and "did not answer `left 20`" in str(body) and "hover" in str(body) and time.monotonic() - started < 4.0, "a motion whose answer is lost is EIO that says the drone may still be doing it", str(body))
        check(sent(sim, "left") == before + 1, "and was not sent again: a retry would fly it twice", str(sim.commands[-4:]))
        check(ext(bridge, "move left=0.2")[0] == rhal.SKR_OK, "and the next motion is answered")
        sim.silent = True
        code, body = ext(bridge, "move left=0.2 timeout=1")
        check(code == rhal.SKR_EIO and "did not answer" in str(body), "a drone that is silent is the same")
        sim.silent = False
        until(lambda: ext(bridge, "move left=0.2")[0] == rhal.SKR_OK, 4.0)
        sim.error_next = "error"
        code, body = ext(bridge, "move forward=0.5")
        check(code == rhal.SKR_EPERM and "answered `forward 50` with 'error'" in str(body), "a drone that refuses a move says so, as EPERM")
        # one at a time
        results: dict[str, tuple[int, object]] = {}
        first = threading.Thread(target=lambda: results.__setitem__("a", ext(bridge, "move forward=2")))
        mark = len(sim.commands)
        first.start()
        until(lambda: "forward 200" in sim.commands[mark:], 3.0)
        results["b"] = ext(bridge, "rotate cw=10")
        first.join(10.0)
        check(results["a"][0] == rhal.SKR_OK and results["b"][0] == rhal.SKR_OK and sim.commands[mark:mark + 2] == ["forward 200", "cw 10"], "two motions asked at once are flown one after the other, the second sent after the first was answered", str(sim.commands[mark:]))
        bridge.close()

        section("motions — rotate and speed")
        bridge = opened(sim)
        ext(bridge, "takeoff")
        yaw = sim.position[3]
        code, body = ext(bridge, "rotate cw=90")
        check(code == rhal.SKR_OK and sim.commands[-1] == "cw 90" and body == {"result": "turned", "sense": "cw", "degrees": 90} and abs(sim.position[3] - yaw - 90) < 1.0, "`rotate cw=90` is `cw 90`, and the drone turned", str(body))
        code, body = ext(bridge, "rotate ccw=45")
        check(code == rhal.SKR_OK and sim.commands[-1] == "ccw 45" and body["sense"] == "ccw" and abs(sim.position[3] - yaw - 45) < 1.0, "`rotate ccw=45` is `ccw 45`")
        check(ext(bridge, "rotate cw=360")[0] == rhal.SKR_OK and ext(bridge, "rotate ccw=1")[0] == rhal.SKR_OK and sim.commands[-2:] == ["cw 360", "ccw 1"], "from 1 to 360")
        count = len(sim.commands)
        for text, word in (("cw=0", "from 1 to 360"), ("cw=361", "from 1 to 360"), ("ccw=-5", "from 1 to 360"), ("cw=90.5", "whole number of degrees"), ("cw=abc", "not a number"), ("", "exactly one"), ("cw=10 ccw=10", "exactly one"), ("left=10", "exactly one"),
                           ("cw=10 turn=2", "does not take"), ("cw=10 timeout=0", "outside")):
            code, body = ext(bridge, f"rotate {text}")
            check(code == rhal.SKR_EINVAL and word in str(body), f"`rotate {text}` is refused, as EINVAL, saying {word!r}", str(body))
        check(len(sim.commands) == count, "and none of them sent the drone a thing")
        code, body = ext(bridge, "speed v=0.5")
        check(code == rhal.SKR_OK and body == {"result": "set", "cm_s": 50} and sim.commands[-1] == "speed 50" and sim.speed == 50, "`speed v=0.5` is `speed 50`", str(body))
        check(ext(bridge, "status")[1]["speed_cm_s"] == 50, "and `status` gives it")
        check(ext(bridge, "speed v=0.1")[0] == rhal.SKR_OK and sim.commands[-1] == "speed 10" and ext(bridge, "speed v=1")[0] == rhal.SKR_OK and sim.commands[-1] == "speed 100", "from 0.1 to 1 metre a second")
        count = len(sim.commands)
        for text, word in (("v=0.09", "from 10 to 100"), ("v=1.01", "from 10 to 100"), ("v=0", "from 10 to 100"), ("v=0.123", "whole number of centimetres"), ("", "needs"), ("v=x", "not a number"), ("v=0.5 w=1", "does not take")):
            code, body = ext(bridge, f"speed {text}")
            check(code == rhal.SKR_EINVAL and word in str(body), f"`speed {text}` is refused, as EINVAL, saying {word!r}", str(body))
        check(len(sim.commands) == count, "and none of them sent the drone a thing")
        sim.error_next = "error"
        before_speed = ext(bridge, "status")[1]["speed_cm_s"]
        check(ext(bridge, "speed v=0.3")[0] == rhal.SKR_EPERM and ext(bridge, "status")[1]["speed_cm_s"] == before_speed, "a speed the drone refused is not the speed `status` gives")
        started = time.monotonic()
        for _ in range(5):
            ext(bridge, "speed v=0.5")
        check(time.monotonic() - started >= 0.4, "commands that follow each other are a tenth of a second apart: the drone does not answer ones that come closer")
        bridge.close()

        section("motions — drive, the sticks")
        bridge = opened(sim)
        ext(bridge, "takeoff")
        started = time.monotonic()
        code, body = ext(bridge, "drive forward=50")
        check(code == rhal.SKR_OK and body["result"] == "sent" and body["rc"] == [0, 50, 0, 0] and "does not answer" in body["note"] and time.monotonic() - started < 0.5, "`drive forward=50` is `rc 0 50 0 0`, sent and not waited for", str(body))
        check(until(lambda: "rc 0 50 0 0" in sim.commands, 2.0), "it reached the drone")
        code, body = ext(bridge, "drive left=-30 up=20 yaw=10")
        check(code == rhal.SKR_OK and body["rc"] == [-30, 0, 20, 10] and until(lambda: "rc -30 0 20 10" in sim.commands, 2.0), "the channels are left, forward, up and yaw, in the guide's order")
        code, body = ext(bridge, "drive")
        check(code == rhal.SKR_OK and body["rc"] == [0, 0, 0, 0] and until(lambda: sim.commands[-1] == "rc 0 0 0 0", 2.0), "`drive` with nothing is the sticks let go")
        check(ext(bridge, "drive left=100 forward=-100 up=-100 yaw=100")[1]["rc"] == [100, -100, -100, 100], "from -100 to 100")
        count = len(sim.commands)
        for text, word in (("forward=101", "outside"), ("up=-101", "outside"), ("left=0.5", "whole percent"), ("yaw=abc", "not a number"), ("spin=1", "does not take"), ("forward=nan", "outside")):
            code, body = ext(bridge, f"drive {text}")
            check(code == rhal.SKR_EINVAL and word in str(body), f"`drive {text}` is refused, as EINVAL, saying {word!r}", str(body))
        check(len(sim.commands) == count, "and none of them sent the drone a thing")
        bridge.close()

        section("motions — status")
        bridge = opened(sim)
        code, body = ext(bridge, "status")
        keys = {"host", "simulator", "sdk", "serial", "speed_cm_s", "stop_mode", "stop_latched", "stop_sent", "stop_acked", "stop_delivered", "states", "bad_states", "stray_replies", "foreign_datagrams", "oversize_datagrams", "link_dead",
                "keepalive_s", "low_battery_percent", "height_cm", "tof_cm", "baro_cm", "battery_percent", "pitch_deg", "roll_deg", "yaw_deg", "speed_xyz", "accel_xyz", "temp_low_c", "temp_high_c", "motor_time_s", "mission_pad", "other_fields"}
        check(code == rhal.SKR_OK and isinstance(body, dict) and keys <= set(body), "`status` gives the link, the stop and every field of the state", str(sorted(keys - set(body))) if isinstance(body, dict) else str(body))
        check(body["stop_mode"] == "hover" and body["simulator"] is True and body["battery_percent"] >= 95 and body["mission_pad"] is None and body["link_dead"] is None, "with the configuration's values and the drone's")
        bridge.close()
    finally:
        sim.stop()


# ── the stop ─────────────────────────────────────────────────────────────────────────────────────

def stop_words(sim: DroneSimulator, word: str) -> int:
    return len([c for c in sim.commands if c == word])


def main_stop() -> None:
    section("stop — hover: the guide's `stop`")
    sim = new_sim()
    try:
        bridge = opened(sim, stop_mode="hover")
        check(TelloBridge.stop_during_extension is True, "a stop may be called while a motion runs")
        ext(bridge, "takeoff")
        started = time.monotonic()
        rc = bridge.emergency_stop()
        elapsed = time.monotonic() - started
        check(rc == rhal.SKR_OK and elapsed < 0.05, "`emergency_stop` returns at once, OK", f"{elapsed * 1000:.1f} ms")
        _, reading = state(bridge)
        check(reading.flags & rhal.FLAG_ESTOP and reading.flags & rhal.FLAG_DEGRADED, "and the state says stopped from that instant")
        code, body = ext(bridge, "move forward=0.5")
        check(code == rhal.SKR_EPERM and "stop is latched" in str(body) and sent(sim, "forward") == 0, "a motion is refused while it is latched, and nothing is sent")
        for text in ("takeoff", "land", "hover", "rotate cw=10", "speed v=0.5", "drive forward=10"):
            check(ext(bridge, text)[0] == rhal.SKR_EPERM, f"`{text}` too")
        check(ext(bridge, "status")[0] == rhal.SKR_OK, "`status` is answered, to say whether the stop got through")
        check(write(bridge, cmd([]))[0] == rhal.SKR_EPERM, "a command is refused as it always is")
        check(until(lambda: bridge._stop_delivered, 3.0), "the stop is seen delivered: a state string after it shows the drone at rest")    # noqa: SLF001
        check(stop_words(sim, "stop") == 3, "after three datagrams, a datagram can be lost and each is the same", str(sim.commands[-6:]))
        _, body = ext(bridge, "status")
        check(body["stop_latched"] is True and body["stop_sent"] == 3 and body["stop_acked"] is True and body["stop_delivered"] is True, "and `status` says what was sent and what was seen", str(body)[:240])
        check(bridge.release_stop() == rhal.SKR_OK, "a release after that is OK")
        _, reading = state(bridge)
        check(not reading.flags & rhal.FLAG_ESTOP and ext(bridge, "status")[1]["stop_latched"] is False, "and the latch is gone")
        check(ext(bridge, "move forward=0.2")[0] == rhal.SKR_OK, "and the drone is commanded again")

        section("stop — during a motion")
        results: dict[str, tuple[int, object]] = {}
        mark = len(sim.commands)
        runner = threading.Thread(target=lambda: results.__setitem__("move", ext(bridge, "move forward=5")))
        runner.start()
        until(lambda: "forward 500" in sim.commands[mark:] and sim.position[0] > 60, 3.0)
        bridge.emergency_stop()
        runner.join(5.0)
        code, body = results["move"]
        check(code == rhal.SKR_EPERM and "stop was latched" in str(body), "a motion that is running when the stop is latched ends at once, as EPERM, whatever the drone says of it", str(body))
        check(until(lambda: bridge._stop_delivered, 3.0), "the stop is delivered while the motion's answer is outstanding")    # noqa: SLF001
        stopped_at = sim.position[0]
        time.sleep(0.2)
        check(abs(sim.position[0] - stopped_at) < 1.0 and stopped_at < 400, "and the drone stopped where it was", f"{stopped_at}")
        check(sim.commands[mark:].count("forward 500") == 1 and not [c for c in sim.commands[mark:] if c.startswith("forward") and c != "forward 500"], "the motion was sent once, and nothing else of the kind", str(sim.commands[mark:]))
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        time.sleep(0.15)
        stray = ext(bridge, "status")[1]["stray_replies"]
        check(ext(bridge, "move back=0.2")[0] == rhal.SKR_OK and ext(bridge, "status")[1]["stray_replies"] >= stray, "the interrupted motion's late answer, if there is one, is not taken for the next motion's")
        check(sim.commands[-1] == "back 20", "which was sent after the stop was released")

        section("stop — twice, and a stop that is called while another is on the wire")
        bridge.emergency_stop()
        until(lambda: bridge._stop_delivered, 3.0)    # noqa: SLF001
        first = stop_words(sim, "stop")
        bridge.emergency_stop()
        check(not bridge._stop_delivered or until(lambda: bridge._stop_sent >= 3, 3.0), "a second stop is owed its own delivery")    # noqa: SLF001
        check(until(lambda: bridge._stop_delivered and stop_words(sim, "stop") >= first + 3, 3.0), "and is sent three times in its turn", f"{first} {stop_words(sim, 'stop')}")    # noqa: SLF001
        bridge.release_stop()
        # a stop on the ground has nothing to stop and is delivered at once
        ext(bridge, "land")
        bridge.emergency_stop()
        check(until(lambda: bridge._stop_delivered, 3.0) and bridge.release_stop() == rhal.SKR_OK, "a stop on the ground is delivered and released")    # noqa: SLF001
        bridge.close()

        section("stop — land")
        bridge = opened(sim, stop_mode="land")
        ext(bridge, "takeoff")
        bridge.emergency_stop()
        check(until(lambda: bridge._stop_delivered, 4.0), "the stop is delivered when a state string shows the ground")    # noqa: SLF001
        check(stop_words(sim, "land") >= 3 and not sim.flying and sim.z < 6, "`land` was sent three times and the drone is down", f"{stop_words(sim, 'land')} {sim.z}")
        check(sent(sim, "stop") == stop_words(sim, "stop"), "and it was not the guide's `stop`")
        check(bridge.release_stop() == rhal.SKR_OK and ext(bridge, "takeoff")[0] == rhal.SKR_OK, "released, and the drone is not told to fly again until a person says so: that is a takeoff")
        bridge.close()

        section("stop — emergency")
        bridge = opened(sim, stop_mode="emergency")
        before = stop_words(sim, "emergency")
        bridge.emergency_stop()
        check(until(lambda: bridge._stop_delivered, 3.0), "the stop is delivered once it has been sent three times: the drone does not answer it")    # noqa: SLF001
        check(stop_words(sim, "emergency") == before + 3 and not sim.motors, "`emergency` was sent three times and the motors are off", str(sim.commands[-5:]))
        _, body = ext(bridge, "status")
        check(body["stop_acked"] is False and body["stray_replies"] == 0, "with no answer expected, and none counted")
        check(until(lambda: not sim.flying, 3.0), "the drone fell")
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        bridge.close()
    finally:
        sim.stop()

    section("stop — a stop that did not get through")
    sim = new_sim()
    try:
        bridge = opened(sim, stop_mode="hover", reply_timeout_ms=100)
        ext(bridge, "takeoff")
        ext(bridge, "drive forward=50")
        until(lambda: sim.rc == (0, 50, 0, 0), 2.0)
        sim.silent = True
        bridge.emergency_stop()
        time.sleep(0.4)
        check(not bridge._stop_delivered and stop_words(sim, "stop") >= 3, "a drone that does not hear the stop is not called stopped, and the stop is sent again")    # noqa: SLF001
        started = time.monotonic()
        check(bridge.release_stop() == rhal.SKR_EIO and bridge._estop, "and a release says so: EIO, the latch stays")    # noqa: SLF001
        check(time.monotonic() - started < 8.0, "inside a bound")
        before = stop_words(sim, "stop")
        time.sleep(1.1)
        check(stop_words(sim, "stop") >= before + 2, "the stop is sent again every half second while it is owed", f"{before} {stop_words(sim, 'stop')}")
        sim.silent = False
        check(until(lambda: bridge._stop_delivered, 3.0), "and when the drone hears, it is delivered")    # noqa: SLF001
        check(sim.rc == (0, 0, 0, 0), "the sticks were let go by the drone's own stop")
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        bridge.close()

        section("stop — a drone that says ok and goes on")
        bridge = opened(sim, stop_mode="hover", reply_timeout_ms=100)
        ext(bridge, "drive forward=50")
        until(lambda: sim.rc == (0, 50, 0, 0), 2.0)
        sim.ignore_stop = True
        bridge.emergency_stop()
        check(until(lambda: ext(bridge, "status")[1]["stop_acked"], 3.0), "the drone's `ok` is seen")
        time.sleep(0.4)
        check(not bridge._stop_delivered, "and is not enough: a state string still shows it moving")    # noqa: SLF001
        check(bridge.release_stop() == rhal.SKR_EIO and bridge._estop, "so the release is EIO and the latch stays")    # noqa: SLF001
        sim.ignore_stop = False
        check(until(lambda: bridge._stop_delivered, 3.0) and sim.rc == (0, 0, 0, 0), "until the drone does stop")    # noqa: SLF001
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        bridge.close()

        section("stop — a link that will not carry it")
        bridge = opened(sim, stop_mode="hover")
        ext(bridge, "drive forward=50")
        until(lambda: sim.rc == (0, 50, 0, 0), 2.0)
        with mock.patch.object(link.UdpLink, "send", side_effect=link.LinkError("Network is unreachable")):
            started = time.monotonic()
            bridge.emergency_stop()
            elapsed = time.monotonic() - started
            check(elapsed < 0.05, "a stop on a link that is down returns at once, all the same", f"{elapsed * 1000:.1f} ms")
            time.sleep(0.4)
            check(not bridge._stop_delivered and bridge._dead is not None, "and is not called delivered; the link is called dead")    # noqa: SLF001
            check(state(bridge)[0] == rhal.SKR_ETIMEDOUT and state(bridge)[1].flags & rhal.FLAG_ESTOP, "the state is ETIMEDOUT and still says stopped")
        check(until(lambda: bridge._stop_delivered, 4.0), "when the link carries a datagram again the stop goes out and is delivered")    # noqa: SLF001
        check(bridge.release_stop() == rhal.SKR_OK and bridge._dead is None, "and the link is no longer dead")    # noqa: SLF001
        bridge.close()

        section("stop — what the latch keeps from the drone")
        bridge = opened(sim, stop_mode="hover", keepalive_s=0.1)
        ext(bridge, "takeoff")
        until(lambda: sent(sim, "battery?") >= 1, 3.0)
        bridge.emergency_stop()
        until(lambda: bridge._stop_delivered, 3.0)    # noqa: SLF001
        mark = len(sim.commands)
        for text in ("move forward=0.5", "rotate cw=10", "drive forward=10", "speed v=0.3", "land", "takeoff", "hover"):
            ext(bridge, text)
        time.sleep(0.5)
        check(set(sim.commands[mark:]) <= {"stop"}, "while it is latched nothing but the stop is sent: no motion, no stick, no keepalive", str(sim.commands[mark:]))
        bridge.release_stop()
        check(until(lambda: "battery?" in sim.commands[mark:], 3.0), "and the keepalive goes on after the release")
        # a takeoff in flight when the stop is latched
        bridge.close()
        sim.flying, sim.z, sim.motors = False, 0.0, False
        bridge = opened(sim, stop_mode="hover")
        slow = new_sim(time_scale=2.0)
        try:
            other = opened(slow, stop_mode="hover")
            outcome: dict[str, tuple[int, object]] = {}
            runner = threading.Thread(target=lambda: outcome.__setitem__("takeoff", ext(other, "takeoff")))
            runner.start()
            until(lambda: "takeoff" in slow.commands, 2.0)
            other.emergency_stop()
            runner.join(5.0)
            check(outcome["takeoff"][0] == rhal.SKR_EPERM and "stop was latched" in str(outcome["takeoff"][1]), "a takeoff that is in the air's way when the stop is latched ends as EPERM", str(outcome))
            check(until(lambda: other._stop_delivered, 3.0), "and the stop is delivered")    # noqa: SLF001
            other.close()
        finally:
            slow.stop()
        bridge.close()
    finally:
        sim.stop()

    section("stop — a close that comes while a motion waits")
    slow = new_sim(time_scale=2.0)
    try:
        bridge = opened(slow, stop_mode="hover")
        outcome = {}
        runner = threading.Thread(target=lambda: outcome.__setitem__("takeoff", ext(bridge, "takeoff")))
        runner.start()
        until(lambda: "takeoff" in slow.commands, 2.0)
        bridge.close()
        runner.join(5.0)
        check(not runner.is_alive() and outcome["takeoff"][0] in (rhal.SKR_EPERM, rhal.SKR_EIO), "a close ends a motion that is waiting; it does not wait out the takeoff", str(outcome))
        check(sent(slow, "stop") == 0 and sent(slow, "land") == 0 and sent(slow, "emergency") == 0, "and a close sends the drone nothing: what stopping it does is the integrator's to decide")
    finally:
        slow.stop()


# ── the session ──────────────────────────────────────────────────────────────────────────────────

def main_session() -> None:
    section("session — the keepalive and the drone's own watchdog")
    sim = new_sim(time_scale=100.0, watchdog_s=0.2)
    try:
        bridge = opened(sim, keepalive_s=0)
        ext(bridge, "takeoff")
        check(until(lambda: not sim.flying, 3.0), "with no keepalive, a drone nobody speaks to lands itself (the guide's 15 seconds, here a fifth of a second)")
        check(sent(sim, "battery?") == 0, "and the driver sent it nothing to prevent that")
        bridge.close()
    finally:
        sim.stop()
    sim = new_sim(time_scale=100.0, watchdog_s=0.2)
    try:
        bridge = opened(sim, keepalive_s=0.05)
        ext(bridge, "takeoff")
        time.sleep(1.0)
        check(sim.flying and sim.z > 50, "with a keepalive, the same drone is still up a second later, five times the watchdog", f"{sim.z}")
        check(sent(sim, "battery?") >= 8, "because the driver reads the battery now and then", str(sent(sim, "battery?")))
        check(ext(bridge, "status")[1]["keepalive_s"] == 0.05, "`status` says how often")
        bridge.close()
        count = sent(sim, "battery?")
        time.sleep(0.3)
        check(sent(sim, "battery?") == count, "and a closed driver sends nothing more")
        check(until(lambda: not sim.flying, 3.0), "so a drone whose driver has gone lands itself")
    finally:
        sim.stop()
    sim = new_sim()
    try:
        bridge = opened(sim, keepalive_s=0.1)
        ext(bridge, "takeoff")
        mark = len(sim.commands)
        outcome: dict[str, tuple[int, object]] = {}
        slow = threading.Thread(target=lambda: outcome.__setitem__("move", ext(bridge, "move forward=5")))
        slow.start()
        until(lambda: "forward 500" in sim.commands[mark:], 3.0)
        time.sleep(0.6)
        slow.join(10.0)
        check(outcome["move"][0] == rhal.SKR_OK, "a long motion is not disturbed by the keepalive", str(outcome))
        between = [c for c in sim.commands[mark:] if c == "battery?"]
        check(len(between) <= 1 + sim.commands[mark:].index("forward 500") and sim.commands[mark:].count("forward 500") == 1, "which does not speak while a command is in flight", str(sim.commands[mark:]))
        stray = ext(bridge, "status")[1]["stray_replies"]
        check(stray == 0, "and no answer of a keepalive is taken for a motion's or left over", str(stray))
        bridge.close()
        sim.silent = True
        bridge = TelloBridge()
        with contextlib.redirect_stderr(io.StringIO()):
            check(bridge.open(config(sim, keepalive_s=0.05, open_timeout_ms=400)) == rhal.SKR_EIO, "a silent drone is not opened")
    finally:
        sim.stop()

    section("session — stray answers")
    sim = new_sim()
    try:
        bridge = opened(sim)
        extra = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        extra.bind(("127.0.0.1", 0))
        target = ("127.0.0.1", bridge._link.local_port)    # noqa: SLF001
        sim_sock = sim._sock    # noqa: SLF001
        count = ext(bridge, "status")[1]["stray_replies"]
        sim_sock.sendto(b"ok\r\n", target)
        sim_sock.sendto(b"87\r\n", target)
        check(until(lambda: ext(bridge, "status")[1]["stray_replies"] == count + 2, 2.0), "an answer to nothing is counted and dropped")
        results: dict[str, tuple[int, object]] = {}
        runner = threading.Thread(target=lambda: results.__setitem__("hover", ext(bridge, "hover")))
        sim.reply_delay_s = 0.3
        runner.start()
        time.sleep(0.1)
        runner.join(5.0)
        check(results["hover"][0] == rhal.SKR_OK, "a delayed answer is waited for")
        sim.reply_delay_s = 0.0
        extra.close()
        bridge.close()
    finally:
        sim.stop()

    section("session — the entry point")
    import skynet_tello.__main__ as entry
    with mock.patch.dict(os.environ, {"SKYNET_TELLO_SIM": ""}), mock.patch.object(entry.rhal, "serve", lambda bridge, **kw: (kw, type(bridge))):
        result = entry.main()
        check(result[1] is TelloBridge and result[0]["ceiling"] == "guest" and result[0]["name"] == "tello", "without SKYNET_TELLO_SIM the bridge served is the real one, at the guest tier", str(result))
    for value in ("0", "true", " "):
        with mock.patch.dict(os.environ, {"SKYNET_TELLO_SIM": value}), mock.patch.object(entry.rhal, "serve", lambda bridge, **kw: type(bridge)):
            check(entry.main() is TelloBridge, f"SKYNET_TELLO_SIM={value!r} is not the opt-in: only 1 is")
    started: list[DroneSimulator] = []
    original = entry.DroneSimulator

    class Recording(original):    # type: ignore[misc, valid-type]
        def start(self):
            started.append(self)
            return super().start()

    with mock.patch.dict(os.environ, {"SKYNET_TELLO_SIM": "1"}), mock.patch.object(entry, "DroneSimulator", Recording), mock.patch.object(entry.rhal, "serve", lambda bridge, **kw: bridge), contextlib.redirect_stderr(io.StringIO()) as notice:
        served_bridge = entry.main()
        try:
            check(isinstance(served_bridge, entry._SimulatedBridge) and len(started) == 1 and "SKYNET_TELLO_SIM=1" in notice.getvalue() and "not a drone" in notice.getvalue(),    # noqa: SLF001
                  "SKYNET_TELLO_SIM=1 serves the simulated drone, and says on stderr that it is not one", notice.getvalue())
            with contextlib.redirect_stderr(io.StringIO()):
                check(served_bridge.open("{}") == rhal.SKR_OK, "and an empty configuration opens it: the address, the ports, the claim and the stop mode are filled in")
            description = json.loads(served_bridge.describe())
            check(description["simulator"] is True and description["robot_slug"] == "ryze-tello", "as a simulation")
            served_bridge.close()
            with contextlib.redirect_stderr(io.StringIO()):
                code = served_bridge.open(json.dumps({"stop_mode": "land", "keepalive_s": 0}))
            check(code == rhal.SKR_OK and served_bridge._stop_mode == "land", "what the configuration says is kept")    # noqa: SLF001
            served_bridge.close()
            for text, wanted in (("not json", rhal.SKR_EINVAL), ("[1]", rhal.SKR_EINVAL)):
                with contextlib.redirect_stderr(io.StringIO()):
                    check(served_bridge.open(text) == wanted, f"{text!r} is refused as the base bridge refuses it")
                served_bridge.close()
        finally:
            for drone in started:
                drone.stop()
    check(isinstance(entry._free_udp_port(), int) and entry._free_udp_port() > 0, "the simulator is given a state port that is free")    # noqa: SLF001


def main() -> int:
    only = os.environ.get("SELFTEST_ONLY", "")
    for name in ("main_frames", "main_links", "main_open", "main_state", "main_commands", "main_motions", "main_stop", "main_session"):
        if name in globals() and (not only or only == name[5:]):
            globals()[name]()
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAILED: {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

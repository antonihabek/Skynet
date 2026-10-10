"""This driver's text and packet layout, held to Rainbow Robotics' own client, which is run (DRY-COVERAGE-PLAN.md item 12.E.14).

    RBPODO_PYTHON=/path/to/venv/bin/python   (make fetch-sdk: a Python with `rbpodo` and `numpy`, built from the maker's source at a pinned commit)
    PYTHONPATH=../../bindings/python:. $RBPODO_PYTHON layout_check.py

A Rainbow control box is a text port and a packet port, and its maker publishes a client for both (`RainbowRobotics/rbpodo`, Apache-2.0): C++ with a Python module over it. It writes a script line the way the controller
expects it, reads the controller's lines into typed responses, and reads the 580-byte state packet into a struct of the document's table, so it is what this driver's bytes are compared with. What is compared, with the
client on one side and `frames.py` on the other:

* **the packet's layout**: a probe packet whose every four-byte word holds its own index (once as a float, once as an integer) is read by the client, and every field it has is at the offset `frames.OFFSETS` gives it, with
  the count that fills the table to 580 bytes and no gap;
* **the packets**: random ones read by the client and by `frames.decode_state`, field by field (the masks this driver applies are the document's), and random ones made by `frames.encode_state` read by the client;
* **every command this driver sends**: `move_servo_j`, `move_j`, `task stop`, `task pause`, `task resume_a` and `_b`, `pgmode`, `set_payload_info`, `get_control_box_info()`, a system variable's `print(...)` and
  `mc jall init`, byte for byte as the client writes them (random angles and parameters, with the digits the client keeps);
* **the controller's lines**: the client's own typing of every kind of line this driver's simulator says, against `frames.parse_event`, and the maker's table of 606 messages against this driver's;
* **the two together**: the client connects to the simulated controller, moves it with `move_j` and `move_servo_j`, activates it, and reads its state packets; and this driver, opened on the same controller, reads where the
  client put it and moves it for the client to read.

**Where the client and this driver differ, on purpose, each is pinned here and not papered over**: the client writes six *significant* digits (C++'s default stream: 123.456789 is sent as 123.457, 0.00001 as `1e-05`, and a
negative zero as `-0`) where this driver keeps six *decimals* in plain notation; the client turns the continuation lines of a multi-line message (`error[msg][...]` is four lines) into lines of an unknown kind where this
driver keeps the message whole; and the client's `time` is a float where the Virtual Control Box was seen to send an integer.

It cannot show that a real controller answers any of this: the controller is the simulated one (F15), written from the maker's documents and from what the maker's Virtual Control Box was seen to do, and it is accepted by
the client because it was made to be.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import random
import re
import socket
import struct
import sys
import threading
import time

import skynet_rhal as rhal

from skynet_rainbow import bridge as bridge_module, codes, frames
from skynet_rainbow.bridge import RainbowBridge
from skynet_rainbow.controller_sim import SimulatedController

CHECKS = 0
FAILURES: list[str] = []
LIMITS = [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]]
DEG = math.pi / 180.0

try:
    import numpy as np
    import rbpodo
except ImportError as error:
    print(f"layout_check: {error} (make fetch-sdk builds a Python with rbpodo and numpy; run this with that interpreter): nothing was compared.")
    sys.exit(2)


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")
        if detail:
            print(f"        {detail}")


def section(title: str) -> None:
    print(f"\n{title}")


def until(predicate, timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # noqa: BLE001 - not true yet
            pass
        time.sleep(0.005)
    return bool(predicate())


class Port:
    """A loopback server that reads lines and answers each with what `answer(line)` returns, as bytes; it keeps the lines it was sent."""

    def __init__(self, answer) -> None:
        self.answer = answer
        self.lines: list[str] = []
        self._server = socket.socket()
        self._server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._server.bind(("127.0.0.1", 0))
        self._server.listen(4)
        self.port = self._server.getsockname()[1]
        self._closing = False
        threading.Thread(target=self._accept, daemon=True).start()

    def _accept(self) -> None:
        while not self._closing:
            try:
                connection, _ = self._server.accept()
            except OSError:
                return
            threading.Thread(target=self._serve, args=(connection,), daemon=True).start()

    def _serve(self, connection: socket.socket) -> None:
        buffer = b""
        with contextlib.suppress(OSError):
            while True:
                chunk = connection.recv(4096)
                if not chunk:
                    return
                buffer += chunk
                while b"\n" in buffer:
                    line, buffer = buffer.split(b"\n", 1)
                    text = line.decode().rstrip("\r")
                    self.lines.append(text)
                    connection.sendall(self.answer(text))

    def close(self) -> None:
        self._closing = True
        with contextlib.suppress(OSError):
            self._server.close()


def command_answer(line: str) -> bytes:
    """What a controller says to the lines the client sends: the acknowledgement, and what each one is waited on for."""
    if line.startswith("print("):
        tag = re.search(r'"([^"\[]+)\[', line)
        return acked(after=[f"info[print_var][{tag.group(1) if tag else 'x'}[0.000]]"])
    if line == "mc jall init":
        return acked(after=["info[mc][done]"])
    if line == "get_control_box_info()":
        return acked(after=["info[controlbox][26042604,11]"])
    return acked()


def acked(before: list[str] | None = None, after: list[str] | None = None) -> bytes:
    return ("\n".join((before or []) + [frames.ACK] + (after or [])) + "\n").encode()


def probe_packet(pack) -> bytes:
    packet = bytearray(frames.PACKET_BYTES)
    packet[0], packet[1], packet[2], packet[3] = frames.PACKET_START, (frames.PACKET_BYTES - 4) & 0xFF, (frames.PACKET_BYTES - 4) >> 8, frames.PACKET_TYPE_STATE
    for word in range(1, frames.PACKET_BYTES // 4):
        packet[4 * word:4 * word + 4] = pack(word)
    return bytes(packet)


def read_with_client(packet: bytes):
    """What the client makes of one packet: a data port that answers `reqdata` with it."""
    port = Port(lambda line: packet if line.strip() == "reqdata" else b"")
    try:
        client = rbpodo.CobotData("127.0.0.1", port.port)
        state = client.request_data(2.0)
    finally:
        port.close()
    return None if state is None else state.sdata


def key_of(name: str) -> tuple[str, int]:
    """The `frames.OFFSETS` entry for one of the client's field names, and the index in it."""
    if name.startswith("eft_"):
        return "eft", ["fx", "fy", "fz", "mx", "my", "mz"].index(name[4:])
    return name, 0


def fields_of(sdata) -> list[str]:
    return sorted(n for n in dir(sdata) if not n.startswith("_"))


def main() -> int:
    section("the client's table of messages is this driver's")
    theirs = {int(code): message.en for code, message in rbpodo.ErrorCodeMessage.items()}
    check(len(theirs) == len(codes.CODES) == 606, f"606 messages in each ({len(theirs)} and {len(codes.CODES)})")
    plain = {code: " ".join(text.replace("\\n", " ").replace("<br>", " ").split()) for code, text in theirs.items()}
    broken = [code for code, text in theirs.items() if "\\n" in text or "<br>" in text]
    check(plain == codes.CODES, "and they are the same, code by code and word by word, once what stands in the client's table for a line break (the two characters backslash and n, or `<br>`) is read as a space", str([c for c in plain if plain[c] != codes.CODES.get(c)][:5]))
    check(len(broken) == 57 and all(codes.CODES[c] == plain[c] for c in broken), f"(the client's table has one in {len(broken)} messages; this driver gives the one-line form)")
    check(all(frames.describe_code(code) == text for code, text in plain.items()), "and `describe_code` gives the client's words, in one line")

    section("the packet's layout — a probe whose every word holds its own index, read by the client")
    floats = read_with_client(probe_packet(lambda k: struct.pack("<f", float(k))))
    ints = read_with_client(probe_packet(lambda k: struct.pack("<i", k)))
    check(floats is not None and ints is not None, "the client reads a 580-byte state packet")
    names = fields_of(floats)
    check(len(names) == 52, f"the client's struct has 52 fields ({len(names)})")
    covered: dict[str, int] = {}
    spans: list[tuple[int, int, str]] = []
    for name in names:
        key, index = key_of(name)
        value = np.asarray(getattr(floats, name))
        count = value.size
        base = frames.OFFSETS.get(key)
        if base is None:
            check(False, f"{name} is in the client's struct and in `frames.OFFSETS`", "missing")
            continue
        offset = base + 4 * index
        word = offset // 4
        probe = value.ravel() if value.dtype.kind == "f" else np.asarray(getattr(ints, name)).ravel()
        held = all(abs(float(probe[i]) - (word + i)) < 1e-9 for i in range(count))
        check(held, f"{name} ({count} x {'float' if value.dtype.kind == 'f' else 'int'}) is at byte {offset}, as `frames.OFFSETS['{key}']` has it", str(probe[:3].tolist()))
        covered[key] = covered.get(key, 0) + count
        spans.append((offset, offset + 4 * count, name))
    check(set(covered) == set(frames.OFFSETS), "every offset this driver has is a field of the client's", str(set(frames.OFFSETS) ^ set(covered)))
    spans.sort()
    gaps = [(a[2], b[2]) for a, b in zip(spans, spans[1:]) if a[1] != b[0]]
    check(not gaps and spans[0][0] == 4 and spans[-1][1] == frames.PACKET_BYTES, "the fields fill the packet from byte 4 to byte 580 with no gap and no overlap", str(gaps[:3]))

    section("the packets — random ones read by the client and by this driver")
    kinds = {name: np.asarray(getattr(floats, name)).dtype.kind for name in names}
    sizes = {name: np.asarray(getattr(floats, name)).size for name in names}
    rng = random.Random(20261010)
    mismatches: list[str] = []
    for round_ in range(200):
        packet = bytearray(probe_packet(lambda k: b"\0\0\0\0"))
        for name in names:
            key, index = key_of(name)
            offset = frames.OFFSETS[key] + 4 * index
            for i in range(sizes[name]):
                at = offset + 4 * i
                if kinds[name] == "f":
                    packet[at:at + 4] = struct.pack("<f", rng.uniform(-500.0, 500.0))
                else:
                    packet[at:at + 4] = struct.pack("<i", rng.randint(-(1 << 31), (1 << 31) - 1))
        sd = read_with_client(bytes(packet))
        ours = frames.decode_state(bytes(packet))
        f32 = np.float32
        pairs = [("ref", ours.ref, sd.jnt_ref), ("ang", ours.ang, sd.jnt_ang), ("cur", ours.cur, sd.jnt_cur), ("temperature", ours.temperature, sd.jnt_temperature)]
        for label, mine, theirs_ in pairs:
            if [f32(v) for v in mine] != [f32(v) for v in theirs_]:
                mismatches.append(f"{label} in round {round_}")
        ints_ = [("task_state", ours.task_state, int(sd.task_state)), ("robot_state", ours.robot_state, int(sd.robot_state)), ("chunk1", ours.chunk1, int(sd.information_chunk_1) & 0xFFFFFFFF),
                 ("mode", ours.mode, int(sd.real_vs_simulation_mode) & 0xF), ("init_state", ours.init_state, int(sd.init_state_info) & 0x3F), ("collision", ours.collision, int(sd.op_stat_collision_occur) & 3),
                 ("sos", ours.sos, int(sd.op_stat_sos_flag) & 0x3F), ("ems", ours.ems, int(sd.op_stat_ems_flag) & 0x3F), ("paused_flag", ours.paused_flag, int(sd.op_stat_soft_estop_occur) & 1),
                 ("freedrive", ours.freedrive, int(sd.is_freedrive_mode) & 3)]
        for label, mine, theirs_ in ints_:
            if mine != theirs_:
                mismatches.append(f"{label} in round {round_}: {mine} and {theirs_}")
        if tuple(ours.joint_info) != tuple(int(v) for v in sd.jnt_info):
            mismatches.append(f"jnt_info in round {round_}")
        if f32(ours.speed_bar) != f32(sd.default_speed):
            mismatches.append(f"speed_bar in round {round_}")
    check(not mismatches, "200 random packets: every field this driver reads is what the client reads, with the document's masks", "; ".join(mismatches[:4]))
    mismatches = []
    for round_ in range(200):
        fields = dict(ref=tuple(rng.uniform(-360, 360) for _ in range(6)), ang=tuple(rng.uniform(-360, 360) for _ in range(6)), cur=tuple(rng.uniform(0, 5) for _ in range(6)), temperature=tuple(rng.uniform(20, 90) for _ in range(6)),
                      task_state=rng.randint(1, 3), robot_state=rng.choice([1, 3, 5, 61, 561]), speed_bar=rng.uniform(0, 1), chunk1=rng.randint(0, (1 << 32) - 1), joint_info=tuple(rng.randint(0, 0xFFFF) for _ in range(6)),
                      mode=rng.randint(0, 1), init_state=rng.randint(0, 6), collision=rng.randint(0, 1), sos=rng.randint(0, 12), ems=rng.randint(0, 4), paused_flag=rng.randint(0, 1), freedrive=rng.randint(0, 2), time_raw=rng.randint(0, 100000))
        packet = frames.encode_state(**fields)
        sd = read_with_client(packet)
        f32 = np.float32
        held = (
            [f32(v) for v in fields["ref"]] == [f32(v) for v in sd.jnt_ref] and [f32(v) for v in fields["ang"]] == [f32(v) for v in sd.jnt_ang] and [f32(v) for v in fields["cur"]] == [f32(v) for v in sd.jnt_cur]
            and [f32(v) for v in fields["temperature"]] == [f32(v) for v in sd.jnt_temperature] and f32(fields["speed_bar"]) == f32(sd.default_speed) and fields["task_state"] == int(sd.task_state)
            and fields["robot_state"] == int(sd.robot_state) and fields["chunk1"] == int(sd.information_chunk_1) & 0xFFFFFFFF and tuple(fields["joint_info"]) == tuple(int(v) for v in sd.jnt_info)
            and fields["mode"] == int(sd.real_vs_simulation_mode) and fields["init_state"] == int(sd.init_state_info) and fields["collision"] == int(sd.op_stat_collision_occur) and fields["sos"] == int(sd.op_stat_sos_flag)
            and fields["ems"] == int(sd.op_stat_ems_flag) and fields["paused_flag"] == int(sd.op_stat_soft_estop_occur) and fields["freedrive"] == int(sd.is_freedrive_mode) and fields["time_raw"] == int(float(sd.time) * 0 + int.from_bytes(packet[4:8], "little")))
        if not held:
            mismatches.append(f"round {round_}")
    check(not mismatches, "200 packets made by `encode_state` are read by the client as they were made", "; ".join(mismatches[:4]))
    check(floats.time == 1.0 and ints.time < 1e-40, "the client's `time` is a float (the Virtual Control Box was seen to send an integer, which the client reads as a denormal: this driver keeps the integer and does not use it)")

    section("every command this driver sends — byte for byte as the client writes it")
    port = Port(command_answer)
    robot = rbpodo.Cobot("127.0.0.1", port.port)
    rc = rbpodo.ResponseCollector()
    sent = port.lines
    differ: list[str] = []
    for round_ in range(60):
        angles = [round(rng.uniform(-359.0, 359.0), 3) for _ in range(6)]
        t1 = rng.choice([0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.5, 1.0])
        t2 = rng.choice([0.03, 0.05, 0.1, 0.15, 0.19])
        gain, alpha = round(rng.uniform(0.01, 5.0), 2), round(rng.uniform(0.01, 0.99), 2)
        robot.move_servo_j(rc, np.array(angles), t1, t2, gain, alpha, 2.0)
        if sent[-1] != frames.servo_j(angles, t1, t2, gain, alpha):
            differ.append(f"servo: {sent[-1]!r} and {frames.servo_j(angles, t1, t2, gain, alpha)!r}")
        speed, accel = round(rng.uniform(0.1, 360.0), 2), round(rng.uniform(0.1, 3600.0), 1)
        robot.move_j(rc, np.array(angles), speed, accel, 2.0)
        if sent[-1] != frames.move_j(angles, speed, accel):
            differ.append(f"move_j: {sent[-1]!r} and {frames.move_j(angles, speed, accel)!r}")
        kg, x, y, z = round(rng.uniform(0, 100), 2), round(rng.uniform(-2000, 2000), 1), round(rng.uniform(-2000, 2000), 1), round(rng.uniform(-2000, 2000), 1)
        robot.set_payload_info(rc, kg, x, y, z, 2.0)
        if sent[-1] != frames.set_payload_info(kg, (x, y, z)):
            differ.append(f"payload: {sent[-1]!r} and {frames.set_payload_info(kg, (x, y, z))!r}")
    check(not differ, "60 random `move_servo_j`, 60 `move_j` and 60 `set_payload_info` lines are the client's bytes", "; ".join(differ[:3]))
    for what, call, ours in (("task stop", lambda: robot.task_stop(rc, 2.0), frames.task_stop()), ("task pause", lambda: robot.task_pause(rc, 2.0), frames.task_pause()),
                             ("task resume_a", lambda: robot.task_resume(rc, False, 2.0), frames.task_resume(False)), ("task resume_b", lambda: robot.task_resume(rc, True, 2.0), frames.task_resume(True)),
                             ("pgmode real", lambda: robot.set_operation_mode(rc, rbpodo.OperationMode.Real, 2.0), frames.pgmode(True)),
                             ("pgmode simulation", lambda: robot.set_operation_mode(rc, rbpodo.OperationMode.Simulation, 2.0), frames.pgmode(False))):
        result = call()
        check(sent[-1] == ours and result.is_success(), f"{what!r} is what the client writes, and the client takes the acknowledgement", f"{sent[-1]!r} and {ours!r}")
    result, value = robot.get_system_variable(rc, rbpodo.SystemVariable.SD_INIT_STATE_INFO, 2.0)
    line = sent[-1]
    found = re.search(r'"([^"\[]+)\[', line)
    check(found is not None and line == frames.print_variable("SD_INIT_STATE_INFO", found.group(1)) and result.is_success() and value == 0.0, "a system variable is asked for with `print(NAME,\"tag[\", \"]\")`, as the client writes it, and read from `info[print_var][tag[value]]`", line)
    result, info = robot.get_control_box_info(rc, 2.0)
    check(sent[-1] == frames.get_control_box_info() and result.is_success() and info.system_version == 26042604 and info.robot_box_type == 11, "`get_control_box_info()` is the client's, and its answer is read as `info[controlbox][version,type]`", sent[-1])
    result = robot.activate(rc, 2.0)
    check("mc jall init" in sent and sent[-1] == frames.activate() and result.is_success(), "an activation is `mc jall init`, which the client sends after reading the activation stage, and waits for `info[...][done]`", str(sent[-3:]))
    del value, result
    port.close()

    section("the digits — where the client and this driver differ, pinned")
    port = Port(command_answer)
    robot = rbpodo.Cobot("127.0.0.1", port.port)
    rc = rbpodo.ResponseCollector()
    robot.move_servo_j(rc, np.array([1.5, 2.0, 3.0, 4.0, 5.0, 123.456789]), 0.01, 0.03, 0.5, 0.5, 2.0)
    theirs_text, ours_text = port.lines[-1], frames.servo_j([1.5, 2.0, 3.0, 4.0, 5.0, 123.456789], 0.01, 0.03, 0.5, 0.5)
    check("123.457," not in ours_text and "123.456789" in ours_text and "123.457]" in theirs_text, "123.456789 degrees: the client keeps six significant digits (123.457), this driver six decimals", f"{theirs_text} / {ours_text}")
    robot.move_servo_j(rc, np.array([-0.0, 2.0, 3.0, 4.0, 5.0, 0.00001]), 0.002, 0.021, 1e-5, 0.999999, 2.0)
    theirs_text, ours_text = port.lines[-1], frames.servo_j([-0.0, 2.0, 3.0, 4.0, 5.0, 0.00001], 0.002, 0.021, 1e-5, 0.999999)
    check("jnt[-0," in theirs_text and "1e-05" in theirs_text and "jnt[0," in ours_text and "0.00001" in ours_text and "e-" not in ours_text, "and a negative zero is `-0` and a hundred-thousandth `1e-05` to the client, `0` and `0.00001` to this driver", f"{theirs_text} / {ours_text}")
    port.close()

    section("the controller's lines — typed by the client and by this driver")
    cases = ["info[motion_changed][7100]", "info[system][halt]", "info[system][pause]", "info[controlbox][26042604,0]", "info[print_var][SD_INIT_STATE_INFO[6.000]]", "info[mc][done]", "warn[code][562]", "warn[code][568]",
             "error[code][253]", "error[code][48]", "error[code][605]", "something the controller never says"]
    port = Port(lambda line: ("\n".join(cases) + "\n" + frames.ACK + "\n").encode())
    robot = rbpodo.Cobot("127.0.0.1", port.port)
    collector = rbpodo.ResponseCollector()
    robot.task_stop(collector, 2.0)
    responses = list(collector.info(True)) + list(collector.warn(True)) + list(collector.error(True)) + [r for r in collector.type_filter(rbpodo.Response.Type.Unknown, True)]
    by_raw = {r.raw(): r for r in responses}
    for line in cases:
        event = frames.parse_event(line)
        response = by_raw.get(line)
        theirs_kind = {rbpodo.Response.Type.Info: "info", rbpodo.Response.Type.Warn: "warn", rbpodo.Response.Type.Error: "error", rbpodo.Response.Type.Unknown: "unknown", rbpodo.Response.Type.ACK: "ack"}.get(response.type()) if response else None
        if event.kind == "unknown":
            check(theirs_kind == "unknown" and event.kind == "unknown", f"{line!r} is of no kind to the client or to this driver")
        else:
            check(response is not None and theirs_kind == event.kind and response.category() == event.category and response.msg() == event.message, f"{line!r} is {event.kind}[{event.category}][{event.message}] to both", f"{response and (theirs_kind, response.category(), response.msg())} / {(event.kind, event.category, event.message)}")
    port.close()
    block = ["error[msg][Parsing Err[V:26042604]", "0: syntax error at ' ( ' in this line:", "bad(", "]"]
    port = Port(lambda line: ("\n".join(block) + "\n" + frames.ACK + "\n").encode())
    robot = rbpodo.Cobot("127.0.0.1", port.port)
    collector = rbpodo.ResponseCollector()
    robot.task_stop(collector, 2.0)
    errors = list(collector.error(True))
    unknown = list(collector.type_filter(rbpodo.Response.Type.Unknown, True))
    port.close()
    depth = frames.bracket_depth(block[0])
    whole = frames.parse_event("\n".join(block))
    check(len(errors) == 1 and errors[0].category() == "msg" and errors[0].msg() == "Parsing Err[V:26042604]", "a four-line syntax error is, to the client, one error on its first line (the client reads the line whole, its brackets and all)", str([e.msg() for e in errors]))
    check(len(unknown) == 3, "and three lines of no kind", str(len(unknown)))
    check(depth > 0 and whole.kind == "error" and whole.category == "msg" and "syntax error" in whole.message and "bad(" in whole.message, "where this driver sees an unfinished bracket on the first line and reads on to the fourth, and keeps one message", f"{depth} {whole}")

    section("the two together — the client against the simulated controller, and this driver on the same one")
    sim = SimulatedController("RB5-850", time_scale=20.0, simulation=True).start()
    try:
        robot = rbpodo.Cobot("127.0.0.1", sim.command_port)
        data = rbpodo.CobotData("127.0.0.1", sim.data_port)
        rc = rbpodo.ResponseCollector()
        result, info = robot.get_control_box_info(rc, 2.0)
        check(result.is_success() and info.system_version == 26042604, "the client asks the simulator for its software version and is answered", str(info.system_version))
        goal = np.array([10.0, 20.0, 30.0, 40.0, 50.0, 60.0])
        result = robot.move_j(rc, goal, 90, 180, 2.0)
        check(result.is_success(), "the client moves it with `move_j`, and it is acknowledged")
        check(until(lambda: np.allclose(np.asarray(data.request_data(1.0).sdata.jnt_ref), goal, atol=0.01), 6.0), "and reads, from the simulator's data port, the reference it went to", str(np.asarray(data.request_data(1.0).sdata.jnt_ref)))
        check(until(lambda: data.request_data(1.0).sdata.robot_state == 1), "and the arm is idle again when it is there")
        bridge = RainbowBridge()
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.open(json.dumps({"host": "127.0.0.1", "port": sim.command_port, "data_port": sim.data_port, "model": "RB5-850", "simulator": True, "limits": LIMITS, "rate_hz": 100, "timeout_ms": 200}))
        check(code == rhal.SKR_OK, "this driver opens on the same controller", said.getvalue())
        out = rhal.State()
        check(until(lambda: bridge.read_state(out, 0) == rhal.SKR_OK and all(abs(a - b * DEG) < 1e-4 for a, b in zip(out.position_rad, goal))), "and reads where the client put it", str(list(out.position_rad)))
        target = [0.2, -0.3, 0.4, -0.5, 0.6, -0.7]
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=target, mask=[1] * 6)
        check(until(lambda: bridge.write_command(command, 0) == rhal.SKR_OK), "this driver streams a target with `move_servo_j`")
        check(until(lambda: np.allclose(np.asarray(data.request_data(1.0).sdata.jnt_ref), np.array(target) / DEG, atol=0.01), 6.0), "and the client reads it there", str(np.asarray(data.request_data(1.0).sdata.jnt_ref)))
        result = robot.move_servo_j(rc, np.array([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]), 0.01, 0.03, 0.5, 0.5, 2.0)
        check(result.is_success() and until(lambda: np.allclose(np.asarray(data.request_data(1.0).sdata.jnt_ref), [1, 2, 3, 4, 5, 6], atol=0.01), 6.0), "the client streams a target too, and this driver reads where it went", str(np.asarray(data.request_data(1.0).sdata.jnt_ref)))
        check(until(lambda: bridge.read_state(out, 0) == rhal.SKR_OK and abs(out.position_rad[5] - 6.0 * DEG) < 1e-3), "(this driver reads it)", str(list(out.position_rad)))
        bridge.emergency_stop()
        check(until(lambda: "task stop" in sim.commands), "this driver's stop reaches the simulator as the client writes it")
        result = robot.set_operation_mode(rc, rbpodo.OperationMode.Real, 2.0)
        check(result.is_success() and until(lambda: data.request_data(1.0).sdata.real_vs_simulation_mode & 0xF == 0), "the client puts the simulator in its real mode, and reads it so")
        result = robot.activate(rc, 5.0)
        check(result.is_success() and until(lambda: (data.request_data(1.0).sdata.init_state_info & 0x3F) == 6), "and activates it, which the simulator does with `info[mc][done]`", str(result))
        bridge.close()
    finally:
        sim.stop()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAILED: {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

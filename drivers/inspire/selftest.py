"""The Inspire hand bridge against a simulated hand written here (DRY-COVERAGE-PLAN.md item 12.E.8): the frames, what it refuses to open on, the state, the refusals, the faults, the stop, the motions and the line.

    PYTHONPATH=../../bindings/python:. python3 selftest.py

Needs python3 on Linux (the simulated hand is a pseudo-terminal) and nothing of the maker's. The simulated hand agrees with whoever wrote it (F15); `layout_check.py` holds the driver's bytes to the maker's worked
frames and to two independent transcriptions of the manuals, and what this shows is the driver's own logic.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import sys
import threading
import time

import skynet_rhal as rhal

from skynet_inspire import bridge as bridge_module, frames, link
from skynet_inspire.bridge import InspireBridge, TOKEN
from skynet_inspire.hand_sim import SimulatedHand

CHECKS = 0
FAILURES: list[str] = []
SPEED = 0.5
FORCE = 300
SCALE = 20.0           # the simulated fingers move this much faster than their speed says
JOINTS = frames.JOINTS
RH56 = frames.PROFILES["RH56"]
DFTP = frames.PROFILES["RH56DFTP"]


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


def raises(kind: type, function, *args) -> str | None:
    """The message of the `kind` the call raises, or None when it raises nothing (or something else)."""
    try:
        function(*args)
    except kind as error:
        return str(error)
    except Exception:  # noqa: BLE001
        return None
    return None


def new_sim(**kw) -> SimulatedHand:
    kw.setdefault("time_scale", SCALE)
    return SimulatedHand(**kw).start()


def settings(sim: SimulatedHand, **extra) -> dict:
    out = {"protocol": "private", "model": "RH56", "stop_mode": "hold", "speed": SPEED, "force_g": FORCE, "simulator": True, "rate_hz": 50, "timeout_ms": 80,
           "roles": {"index": "right_hand.finger.flex#0", "thumb_rotation": "right_hand.thumb.rotate"}}
    out.update({"host": "127.0.0.1", "port": sim.port} if sim.port else {"device": sim.path})
    out.update(extra)
    return {k: v for k, v in out.items() if v is not None}


def config(sim: SimulatedHand, **extra) -> str:
    return json.dumps(settings(sim, **extra))


def opened(sim: SimulatedHand, **extra) -> InspireBridge:
    bridge = InspireBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge


def opens_with(sim: SimulatedHand, **extra) -> tuple[int, str, InspireBridge]:
    bridge = InspireBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    return code, said.getvalue(), bridge


def refused(text: str) -> tuple[int, str]:
    bridge = InspireBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(text)
    bridge.close()
    return code, said.getvalue()


def state(bridge: InspireBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def cmd(values: list[float], mask: list[int] | None = None, mode: int = rhal.MODE_POSITION, count: int | None = None) -> rhal.Command:
    n = count if count is not None else len(values)
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=list(values), mask=mask if mask is not None else [1] * n)


def write(bridge: InspireBridge, command: rhal.Command) -> tuple[int, str]:
    bridge._last_refusal = None  # noqa: SLF001 - a refusal is said once for a run of the same cause; each check asks to hear it
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def ext(bridge: InspireBridge, text: str) -> tuple[int, object]:
    code, body = bridge.extension(TOKEN, text.encode())
    try:
        return code, json.loads(body)
    except json.JSONDecodeError:
        return code, body.decode()


def ready(bridge: InspireBridge) -> bool:
    return until(lambda: state(bridge)[0] == rhal.SKR_OK)


def deg(*values: float) -> list[float]:
    return [math.radians(v) for v in values]


def pose(registers: list[int], profile: frames.Profile = RH56) -> list[float]:
    return [frames.register_to_angle(r, j, profile) for j, r in enumerate(registers)]


def said_by(call) -> tuple[object, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        result = call()
    return result, said.getvalue()


def hex_(text: str) -> bytes:
    return bytes.fromhex(text.replace(" ", ""))


def fds() -> int:
    return len(os.listdir("/proc/self/fd"))


def holding_the_line(bridge: InspireBridge, seconds: float) -> threading.Thread:
    """A thread that owns the line's lock for `seconds`, as a transaction in flight does."""
    taken = threading.Event()

    def hold() -> None:
        with bridge._link_lock:  # noqa: SLF001
            taken.set()
            time.sleep(seconds)

    thread = threading.Thread(target=hold, daemon=True)
    thread.start()
    taken.wait(1.0)
    return thread


def triage() -> None:
    """What the first seeding campaign (DRY-COVERAGE-PLAN.md item 12.C.3, the fifty-ninth session) showed nothing here would notice: 132 of 448 seeded defects survived, and each section below is one of the groups they fell into."""
    # ── frames ──
    section("frames — the ends of a register range, an answer of the wrong length, a write acknowledged with a result that is not 1")
    low, high = RH56.angle_deg[3]
    step = (high - low) / 1000.0
    check(frames.angle_to_register(math.radians(low - 0.4 * step), 3, RH56) == 0 and frames.angle_to_register(math.radians(low - 0.6 * step), 3, RH56) is None,
          "an angle 0.4 of a register below the range is register 0, and 0.6 below it has no register (-1 is 'no response' and is never produced)")
    check(frames.angle_to_register(math.radians(high + 0.4 * step), 3, RH56) == 1000 and frames.angle_to_register(math.radians(high + 0.6 * step), 3, RH56) is None,
          "an angle 0.4 of a register above the range is register 1000, and 0.6 above it has no register (1001 does not exist)")
    body = bytes([1, 11 + 3, 0x11, 0x0A, 0x06]) + b"\x00" * 11
    short_answer = b"\x90\xEB" + body + bytes([frames.checksum(body)])
    check("a read of 12 bytes is 20" in (raises(frames.ProtocolError, frames.parse_response, short_answer, 1, 0x11, 1546, 12) or ""), "an answer that is well formed for eleven bytes is refused for a read of twelve, with its length byte and checksum right")
    for result in (0, 2, 255):
        ack = bytearray(hex_("90 EB 01 04 12 CE 05 01 EB"))
        ack[7] = result
        ack[-1] = frames.checksum(bytes(ack[2:-1]))
        check("result byte is" in (raises(frames.ProtocolError, frames.parse_response, bytes(ack), 1, 0x12, 1486) or ""), f"a write acknowledged with a result of {result} is refused: the maker's success is 1 and nothing else")

    # ── the defaults the README states ──
    section("the bounds — the numbers the README gives")
    check((bridge_module._TRANSACTION_S, bridge_module._STALE_S, bridge_module._COMMAND_BUDGET_S, bridge_module._FAULT_THRESHOLD, bridge_module._REOPEN_S, bridge_module._INITIAL_READ_ATTEMPTS,  # noqa: SLF001
           bridge_module._RATE_HZ, bridge_module._MAX_RATE_HZ, bridge_module._ECHO_GRACE_S, bridge_module._ARRIVAL_TOLERANCE, bridge_module._CALIBRATION_S) ==
          (0.08, 0.5, 0.25, 3, 0.5, 3, 20.0, 50.0, 0.5, 50, 8.0), "80 ms a transaction, 0.5 s before a reading is stale, 0.25 s a command, three lost frames to a dead line, half a second between reopenings, three first reads, 20 Hz (50 at most), "
          "half a second of grace for a command to be taken, 50 registers to arrive and 8 s of calibration")
    check(TOKEN == "vendor.inspire.task" and InspireBridge.stop_during_extension is True, "the extension is vendor.inspire.task, and a stop may arrive while a motion runs")

    # ── the link itself ──
    section("the link — an answer that stops half way, noise, a late answer, a header in two pieces, bytes after an answer, a gateway that goes away and an adapter that is pulled")
    line = new_sim()
    try:
        request = frames.request_read(1, frames.ADDR_ANGLE_ACT, 12)
        lk = link.Link(device=line.path)
        lk.open()
        line.truncate = 4
        began = time.monotonic()
        text = raises(link.LinkError, lk.transact, request, 1, frames.CMD_READ, frames.ADDR_ANGLE_ACT, 12, 1.0) or ""
        check("stopped sending" in text and time.monotonic() - began < 0.3, f"an answer that stops half way is given up on after the byte timeout and says so, not after the whole budget ({time.monotonic() - began:.2f} s)", text)
        line.truncate = 0
        line.noise = 700
        began = time.monotonic()
        text = raises(link.LinkError, lk.transact, request, 1, frames.CMD_READ, frames.ADDR_ANGLE_ACT, 12, 1.0) or ""
        check("wrong baud rate" in text and time.monotonic() - began < 0.5, "a line that sends only noise is called noise, naming the baud rate, as soon as it has sent more than it could be a header", text)
        line.noise = 0
        line.delay_s = 0.15
        raises(link.LinkError, lk.transact, request, 1, frames.CMD_READ, frames.ADDR_ANGLE_ACT, 12, 0.05)
        line.delay_s = 0.0
        time.sleep(0.3)
        other = frames.request_read(1, frames.ADDR_ANGLE_SET, 12)
        try:
            got = lk.transact(other, 1, frames.CMD_READ, frames.ADDR_ANGLE_SET, 12, 0.5)
            check(len(got) == 12, "a late answer to an earlier request is thrown away before the next request is written, and is not read as its answer")
        except (link.LinkError, frames.ProtocolError) as error:
            check(False, "a late answer to an earlier request is thrown away before the next request is written, and is not read as its answer", str(error))
        line.split_gap_s, line.split_at = 0.03, 1
        try:
            got = lk.transact(request, 1, frames.CMD_READ, frames.ADDR_ANGLE_ACT, 12, 0.5)
            check(len(got) == 12, "an answer whose two-byte header arrives in two pieces (90, then EB) is found")
        except (link.LinkError, frames.ProtocolError) as error:
            check(False, "an answer whose two-byte header arrives in two pieces (90, then EB) is found", str(error))
        line.split_gap_s, line.split_at = 0.0, 3
        line.trailer = b"\x01\x02\x03"
        try:
            got = lk.transact(request, 1, frames.CMD_READ, frames.ADDR_ANGLE_ACT, 12, 0.5)
            check(len(got) == 12, "bytes that follow an answer are not part of it")
        except (link.LinkError, frames.ProtocolError) as error:
            check(False, "bytes that follow an answer are not part of it", str(error))
        line.trailer = b""
        line.pull()
        time.sleep(0.1)
        raises(link.LinkError, lk.transact, request, 1, frames.CMD_READ, frames.ADDR_ANGLE_ACT, 12, 0.3)
        check(not lk.is_open, "a line that failed with an OS error (the adapter was pulled) is closed, so that whoever drives it knows to open it again")
        lk.close()
    finally:
        line.stop()
    gateway = new_sim(tcp=True)
    try:
        lk = link.Link(host="127.0.0.1", port=gateway.port)
        lk.open()
        until(lambda: gateway._open_connections, 2)  # noqa: SLF001
        gateway.hang_up()
        time.sleep(0.05)
        raises(link.LinkError, lk.transact, frames.request_read(1, frames.ADDR_ANGLE_ACT, 12), 1, frames.CMD_READ, frames.ADDR_ANGLE_ACT, 12, 0.3)
        check(not lk.is_open, "a gateway that closed the connection is a line that is closed")
        lk.close()
    finally:
        gateway.stop()

    # ── what a command may take ──
    section("write_command — the quarter of a second a command is given includes waiting for the line")
    sim = new_sim()
    try:
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge), "ready")
        owner = holding_the_line(bridge, 0.6)
        began = time.monotonic()
        code, said = write(bridge, cmd(pose([600] * 6)))
        took = time.monotonic() - began
        owner.join()
        check(code == rhal.SKR_EIO and 0.2 <= took <= 0.4, f"a command that cannot have the line is EIO after its quarter of a second ({took:.2f} s), not when the line is free again (F299)", said)
        bridge.close()
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge), "ready")
        owner = holding_the_line(bridge, 0.2)
        sim.delay_s = 0.07
        began = time.monotonic()
        code, said = write(bridge, cmd(pose([600] * 6)))
        took = time.monotonic() - began
        sim.delay_s = 0.0
        owner.join()
        check(code == rhal.SKR_EIO and took <= 0.3, f"and what is left of the quarter of a second is what the transaction gets: 50 ms is not enough for a hand that answers in 70 ({took:.2f} s)", said)
        bridge.close()

        # ── readings that are old, and a line that is dead ──
        section("read_state — a reading that is old, a line that is dead, and one lost frame that is neither")
        bridge = opened(sim)
        check(ready(bridge), "ready")
        owner = holding_the_line(bridge, 1.0)
        began = time.monotonic()
        stale = until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 1.5)
        took = time.monotonic() - began
        owner.join()
        check(stale and 0.3 <= took <= 0.9, f"a reading older than the staleness bound (half a second) is ETIMEDOUT though no read has failed and the line is not called dead ({took:.2f} s)")
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 2), "and it is read again when the line is free")
        bridge.close()
        bridge = opened(sim, stale_ms=100)
        check(ready(bridge), "ready")
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 1), "with a staleness bound of 100 ms a hand that is read every 20 ms is current")
        sim.drop = True
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 0.25), "and one that goes quiet is stale in a quarter of a second, before three reads can have failed")
        sim.drop = False
        bridge.close()
        bridge = opened(sim, stale_ms=30000)
        check(ready(bridge), "ready")
        sim.drop = True
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 3), "a line whose reads fail three times is ETIMEDOUT however recent its last good reading is")
        first = state(bridge)[1].stamp_ns
        time.sleep(0.2)
        check(state(bridge)[1].stamp_ns == first, "and the state keeps the stamp of the last reading that arrived, not the time it was asked for")
        sim.drop = False
        until(lambda: state(bridge)[0] == rhal.SKR_OK, 3)
        dead_seen: list[object] = []
        watching = threading.Event()

        def watch() -> None:
            while not watching.is_set():
                dead_seen.append(bridge._dead)  # noqa: SLF001
                time.sleep(0.001)

        watcher = threading.Thread(target=watch, daemon=True)
        watcher.start()
        for _ in range(4):
            sim.skip_requests = 1
            time.sleep(0.25)
        watching.set()
        watcher.join()
        check(all(d is None for d in dead_seen), "four lost frames, each followed by good readings, never make the line dead")
        bridge.close()

        # ── a reading that is not there ──
        section("a driver with no reading yet says so, and does not command or move a hand it has not read")
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge), "ready")
        with bridge._lock:  # noqa: SLF001
            bridge._latest = None  # noqa: SLF001
        rc, out = state(bridge)
        check(rc == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED, "a state with no reading is ETIMEDOUT and DEGRADED")
        code, said = write(bridge, cmd(pose([600] * 6)))
        check(code == rhal.SKR_ETIMEDOUT and "not been read" in said, "a command is refused until the hand has been read", said)
        code, text = ext(bridge, "status")
        check(code == rhal.SKR_ETIMEDOUT and "not been read" in str(text), "and so is a status")
        bridge.close()

        # ── what a command is compared with ──
        section("read_state — a command that has just been written is not a command the hand did not take")
        sim.place([1000] * 6)
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge), "ready")
        write(bridge, cmd(pose([600] * 6)))
        flags = [state(bridge)[1].flags & rhal.FLAG_DEGRADED for _ in range(150) if time.sleep(0.002) is None]
        check(not any(flags), "for the half second the hand is given, a target that the last reading does not show yet is not DEGRADED")
        bridge.close()
        sim.place([500] * 6)
        sim.set_registers(frames.ADDR_ANGLE_SET, [500, 500, -1, 500, 500, 500])
        bridge = opened(sim, rate_hz=10)
        check(ready(bridge) and until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 1) and ext(bridge, "status")[1]["image"][2] == 500, "a joint the hand has no target for (-1) is where it is, and is not called to have missed its command")
        bridge.close()
        sim.place([1000] * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        write(bridge, cmd(pose([600, 1000, 1000, 1000, 1000, 1000]), mask=[1, 0, 0, 0, 0, 0]))
        time.sleep(0.8)
        check(not state(bridge)[1].flags & rhal.FLAG_DEGRADED and ext(bridge, "status")[1]["image"] == [600, 1000, 1000, 1000, 1000, 1000], "a command that applies to one joint leaves the others' targets as they were, and the hand is not called degraded for them")
        write(bridge, cmd(pose([700] * 6), mask=[]))
        check(until(lambda: sim.angles == [700] * 6, 3), "a command with no mask applies to every joint")
        bridge.close()

        # ── what the state says of the hand's own parameters ──
        section("read_state — a hand that does not hold the operator's speed or force, or that has not been told them yet, is DEGRADED")
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge), "ready")
        with bridge._lock:  # noqa: SLF001
            reading = bridge._latest  # noqa: SLF001
            bridge._latest = frames.Reading(reading.angle, reading.target, reading.force_limit, (100,) * 6, reading.error, reading.status)  # noqa: SLF001
        check(state(bridge)[1].flags & rhal.FLAG_DEGRADED, "a hand whose speed differs from the operator's is DEGRADED")
        with bridge._lock:  # noqa: SLF001
            bridge._latest = frames.Reading(reading.angle, reading.target, (50,) * 6, reading.speed, reading.error, reading.status)  # noqa: SLF001
        check(state(bridge)[1].flags & rhal.FLAG_DEGRADED, "so is one whose force limit differs")
        with bridge._lock:  # noqa: SLF001
            bridge._latest = reading  # noqa: SLF001
            bridge._stamp_ns = rhal.acquisition_ns()  # noqa: SLF001
        check(not state(bridge)[1].flags & rhal.FLAG_DEGRADED, "and one that holds both is not")
        bridge._params_dirty = True  # noqa: SLF001
        check(state(bridge)[1].flags & rhal.FLAG_DEGRADED, "a hand whose speed and force are still owed to it is DEGRADED, whatever its own record says")
        sim.writes.clear()
        write(bridge, cmd(pose([600] * 6)))
        order = [address for _, address, _ in sim.writes]
        check(frames.ADDR_ANGLE_SET in order and frames.ADDR_FORCE_SET in order and frames.ADDR_SPEED_SET in order and order.index(frames.ADDR_ANGLE_SET) > max(order.index(frames.ADDR_FORCE_SET), order.index(frames.ADDR_SPEED_SET)),
              f"a command that finds the parameters owed tells the hand the force and the speed first and its target after (writes at {order})")
        with bridge._lock:  # noqa: SLF001
            bridge._latest = frames.Reading(reading.angle, reading.target, (1,) * 6, reading.speed, reading.error, reading.status)  # noqa: SLF001
        said_status = ext(bridge, "status")[1]
        check(said_status["force_limit_configured_g"] == [FORCE] * 6 and said_status["force_limit_g"] == [1] * 6, "the status says the force the operator configured and, apart, the force limit the hand holds", str(said_status))
        bridge.close()

        # ── a dead line gives the speed and force back, whatever the hand says ──
        section("the line — after a dead line the speed and force are written again, and the image is made from the hand once")
        sim.place([1000] * 6)
        sim.set_registers(frames.ADDR_ANGLE_SET, [1000, 1000, -1, 1000, 1000, 1000])
        bridge = opened(sim, rate_hz=50)
        check(ready(bridge), "ready")
        sim.drop = True
        check(until(lambda: bridge._dead is not None, 3), "the line is called dead")  # noqa: SLF001
        sim.drop = False
        check(until(lambda: ext(bridge, "status")[1]["parameters_reasserted"] >= 1, 3), "and the speed and force are told to a hand that comes back even when its own record shows them: it may have been power-cycled in between")
        check(ext(bridge, "status")[1]["image"][2] == 1000, "a joint with no target is where it is in the image the driver makes again")
        sim.deaf = True
        write(bridge, cmd(pose([300] * 6)))
        time.sleep(0.25)
        check(ext(bridge, "status")[1]["image"] == [300] * 6, "and that image is made once: what the driver has been asked to do is not overwritten by what the hand echoes")
        sim.deaf = False
        bridge.close()

        # ── a wrong answer in operation ──
        section("read_state — a hand that sends wrong answers for a while and then right ones is read again")
        sim.place([1000] * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.garble = True
        time.sleep(0.7)
        status_code, body = ext(bridge, "status")
        check(status_code == rhal.SKR_OK and "read_error" in body and "checksum" in body["read_error"], "a status asked for while the answers are wrong still answers, and says the on-demand reads failed")
        sim.garble = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3), "a hand whose answers had the wrong checksum is read again when they are right")
        counted = ext(bridge, "status")[1]["exchanges"]
        time.sleep(0.3)
        check(ext(bridge, "status")[1]["exchanges"] > counted + 3 and bridge._loop.is_alive(), "and the reading thread did not die of them: its exchanges go on")  # noqa: SLF001
        for what, corrupt, mend in (("an angle of 4000", lambda: setattr(sim, "report_angles", [4000] * 6), lambda: setattr(sim, "report_angles", None)),
                                    ("a target of -2", lambda: sim.set_registers(frames.ADDR_ANGLE_SET, [-2] * 6), lambda: sim.set_registers(frames.ADDR_ANGLE_SET, [1000] * 6)),
                                    ("a speed of 1500", lambda: sim.set_registers(frames.ADDR_SPEED_SET, [1500] * 6), lambda: sim.set_registers(frames.ADDR_SPEED_SET, [500] * 6)),
                                    ("a force limit of -5", lambda: sim.set_registers(frames.ADDR_FORCE_SET, [-5] * 6), lambda: sim.set_registers(frames.ADDR_FORCE_SET, [FORCE] * 6))):
            corrupt()
            check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 3), f"{what} is not a position or a record the manual allows, and is not believed")
            mend()
            until(lambda: state(bridge)[0] == rhal.SKR_OK, 3)
        bridge.close()

        # ── a gateway that stays shut ──
        section("the line — a gateway that accepts and hangs up is tried about twice a second, and one that is not listening is said to be so")
        gateway = new_sim(tcp=True)
        try:
            bridge = opened(gateway)
            check(ready(bridge), "ready")
            gateway.refuse_connections = True
            before = gateway.connections
            gateway.hang_up()
            time.sleep(1.4)
            attempts = gateway.connections - before
            check(1 <= attempts <= 4, f"a gateway that is up and will not talk is opened again at most twice a second ({attempts} attempts in 1.4 s)")
            gateway.refuse_connections = False
            gateway.stop_listening()
            gateway.hang_up()
            check(until(lambda: bridge._dead is not None and "could not be opened" in bridge._dead, 3), "and one that is not listening is reported as a line that could not be opened, not as the read that failed last")  # noqa: SLF001
            gateway.resume_listening()
            check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 5), "and is read again when it listens")
            bridge.close()
        finally:
            gateway.stop()

        # ── open and close ──
        section("open and close — the first read may be lost, a refused open leaves nothing open, and a close ends the thread and the line")
        sim.skip_requests = 1
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_OK, "a hand whose first answer was lost is opened: the first read is tried three times", said)
        bridge.close()
        sim.heal()
        silent = new_sim()
        silent.silent = True
        mid = fds()
        code, said, bridge = opens_with(silent)
        check(code == rhal.SKR_EIO and "no hand answered" in said and fds() == mid, "a hand that does not answer is refused as one that did not answer, and its line is closed again", said)
        silent.stop()
        garbled = new_sim()
        garbled.garble = True
        code, said, bridge = opens_with(garbled)
        check(code == rhal.SKR_EIO and "no hand answered" in said, "and so is one whose first answers are wrong, in the same words", said)
        garbled.stop()
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.delay_s = 0.1
        thread = bridge._loop  # noqa: SLF001
        began = time.monotonic()
        bridge.close()
        check(thread is not None and not thread.is_alive() and time.monotonic() - began < 1.0, "a close does not return until the reading thread has ended, and does not take its two seconds")
        sim.delay_s = 0.0
        base = fds()
        for _ in range(5):
            opened(sim).close()
        check(fds() == base, "five opens and closes leave no line open")

        # ── the same bridge, a second session ──
        section("open — a session starts with nothing of the last one's")
        sim.place([1000] * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        bridge.emergency_stop()
        until(lambda: ext(bridge, "status")[1]["stop_delivered"])
        bridge._calibrating_until = time.monotonic() + 100  # noqa: SLF001
        bridge._reasserted = 7  # noqa: SLF001
        bridge._last_refusal = "an old refusal"  # noqa: SLF001
        text = config(sim)
        bridge.close()
        said_open, _ = said_by(lambda: bridge.open(text))
        status = ext(bridge, "status")[1]
        check(said_open == rhal.SKR_OK and ready(bridge) and not state(bridge)[1].flags & rhal.FLAG_ESTOP, "the same driver opened again is not stopped")
        check(status["stop_delivered"] is False and status["stop_hold"] is None and status["calibrating"] is False and status["parameters_reasserted"] == 0 and bridge._last_refusal is None, "and has no stop owed or delivered, no hold, no calibration, no count of parameters told and no refusal remembered")  # noqa: SLF001
        check(write(bridge, cmd(pose([800] * 6)))[0] == rhal.SKR_OK, "and takes a command")
        bridge.close()
        released = InspireBridge()
        released._estop = True  # noqa: SLF001
        check(released.release_stop() == rhal.SKR_OK and released._estop is False, "a stop released before the driver was opened is released")  # noqa: SLF001

        # ── the stop, again ──
        section("the stop — a second stop is written again, a hand that did not take the hold is not released, and what the status says of the hold")
        slow = new_sim(time_scale=1.0)
        try:
            bridge = opened(slow, speed=0.05)
            check(ready(bridge), "ready")
            write(bridge, cmd(pose([0] * 6)))
            time.sleep(0.4)
            slow.deaf = True
            bridge.emergency_stop()
            check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2), "the hold is written")
            said_release, said = said_by(bridge.release_stop)
            check(said_release == rhal.SKR_EIO and "stays latched" in said and bridge._estop, "and a hand that acknowledged it and did not take it (its own record of its targets is not the hold) is not released", said)  # noqa: SLF001
            slow.deaf = False
            bridge.close()
            bridge = opened(slow, speed=0.05)
            check(ready(bridge), "ready")
            write(bridge, cmd(pose([0] * 6)))
            time.sleep(0.3)
            slow.place([900, 800, 700, 600, 500, 400])
            bridge.emergency_stop()
            check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2), "the hold is written")
            status = ext(bridge, "status")[1]
            check(status["stop_hold"] == [900, 800, 700, 600, 500, 400] == slow.targets, "and the status says the hold: the angles the hand was at, each joint's own", str(status["stop_hold"]))
            check(status["image"] == status["stop_hold"], "and the image is the hold")
            slow.target_writes.clear()
            bridge.emergency_stop()
            check(until(lambda: len(slow.target_writes) >= 1, 2), "a second stop writes the hold again")
            slow.silent = True
            bridge.emergency_stop()
            check(ext(bridge, "status")[1]["stop_hold"] is None and ext(bridge, "status")[1]["stop_delivered"] is False, "and a stop that has not been written does not show the last one's hold")
            slow.silent = False
            check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3), "the hold goes out when the line is back")
            check(bridge.release_stop() == rhal.SKR_OK, "released")
            status = ext(bridge, "status")[1]
            check(status["stop_hold"] is None and status["stop_delivered"] is False, "and a released stop leaves no hold and nothing delivered")
            bridge.close()
            bridge = opened(slow, speed=0.05)
            check(ready(bridge), "ready")
            slow.report_angles = [4000] * 6
            until(lambda: slow._load_shorts(frames.ADDR_ANGLE_ACT, 6) == [4000] * 6, 1)  # noqa: SLF001
            slow.requests.clear()
            bridge.emergency_stop()
            time.sleep(0.3)
            check(not any(command == frames.CMD_WRITE and address == frames.ADDR_ANGLE_SET for command, address, _ in slow.requests), "a hold is never written from angles that are not angles")
            slow.report_angles = None
            bridge.close()
        finally:
            slow.stop()
        sim.place([1000] * 6)
        bridge = opened(sim, timeout_ms=300)
        check(ready(bridge), "ready")
        sim.target_writes.clear()
        owner = holding_the_line(bridge, 0.15)
        results: list[int] = []
        sender = threading.Thread(target=lambda: results.append(write(bridge, cmd(pose([100] * 6)))[0]))
        sender.start()
        time.sleep(0.05)
        bridge.emergency_stop()
        owner.join()
        sender.join(3.0)
        until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3)
        check(results == [rhal.SKR_EPERM] and not any(values == (100,) * 6 for _, values in sim.target_writes),
              f"a command that was waiting for the line when the stop was called is refused, and never writes its target after the stop ({results})")
        bridge.release_stop()
        bridge.close()
        sim.place([1000] * 6)

        # ── the stop and the other motions ──
        bridge = opened(sim)
        check(ready(bridge), "ready")
        bridge.emergency_stop()
        until(lambda: ext(bridge, "status")[1]["stop_delivered"])
        sim.writes.clear()
        cleared, calibrated = ext(bridge, "clear_errors"), ext(bridge, "calibrate_force confirm=unloaded")
        check(cleared[0] == rhal.SKR_EPERM and calibrated[0] == rhal.SKR_EPERM and not any(address in (frames.ADDR_CLEAR_ERROR, frames.ADDR_FORCE_CALIBRATION) for _, address, _ in sim.writes),
              "while the stop is latched `clear_errors` and `calibrate_force` are refused and write nothing: only `status` is answered")
        bridge.release_stop()
        bridge.close()

        # ── motions ──
        section("extension — the arguments of a move, and how it ends")
        sim.place([1000] * 6)
        bridge = opened(sim, limits={"index": [1.0, 2.0]})
        check(ready(bridge), "ready")
        code, text = ext(bridge, "move index=2.5")
        check(code == rhal.SKR_EINVAL and "outside" in text, "a move beyond the operator's narrower limit is refused, though the profile reaches it", text)
        code, text = ext(bridge, "move index=nan")
        check(code == rhal.SKR_EINVAL and "outside" in text, "a target that is not a number is refused as outside the limits", text)
        code, text = ext(bridge, "move index=1.5 timeout=121")
        check(code == rhal.SKR_EINVAL and "outside" in text, "a timeout of 121 s is refused: 120 s is the most", text)
        code, text = ext(bridge, "move index=1.5 timeout=nan")
        check(code == rhal.SKR_EINVAL and "outside" in text, "and a timeout that is not a number is refused", text)
        bridge.close()

        section("extension — a move that starts from a stalled status, and one that goes the other way")
        for from_registers, to_registers, what in ((1000, 400, "closing"), (200, 800, "opening")):
            lagged = new_sim()
            try:
                lagged.place([from_registers] * 6)
                bridge = opened(lagged)
                check(ready(bridge), "ready")
                lagged.echo_lag_s = 0.3
                began = time.monotonic()
                code, body = ext(bridge, f"move index={pose([to_registers] * 6)[3]} timeout=5")
                took = time.monotonic() - began
                check(code == rhal.SKR_OK and body["result"] == "arrived" and lagged.angles[3] == to_registers and took >= 0.25 and abs(body["joints"]["index"]["angle_register"] - to_registers) <= 50,
                      f"{what}: a status and an angle that show the old state for 0.3 s do not end the move early, and the angle it reports is where the finger is ({took:.2f} s)", str(body))
                bridge.close()
                lagged.echo_lag_s = 0.0
            finally:
                lagged.stop()
        held = new_sim()
        try:
            held.blocks[3] = 600
            held.place([1000] * 6)
            bridge = opened(held)
            check(ready(bridge), "ready")
            code, body = ext(bridge, f"move index={pose([600] * 6)[3]} timeout=5")
            check(code == rhal.SKR_OK and body["result"] == "stopped_on_an_object" and held.statuses[3] == 3, "a finger that closes on an object stops on its force limit, and the move says so")
            held.deaf = True
            code, text = ext(bridge, f"move index={pose([1000] * 6)[3]} timeout=1")
            check(code == rhal.SKR_EIO, "a hand that acknowledges a target and does not take it is not a move that arrived, whatever status the last move left in it", str(text))
            held.deaf = False
            held.place([1000] * 6)
            code, body = ext(bridge, f"move index={pose([300] * 6)[3]} ring={pose([300] * 6)[1]} timeout=5")
            check(code == rhal.SKR_OK and body["result"] == "stopped_on_an_object", "a move of two joints, one of which stops on an object, is not reported as arrived", str(body))
            bridge.close()
        finally:
            held.stop()
        offset = new_sim()
        try:
            offset.place([1000] * 6)
            offset.rest_offset = 30
            bridge = opened(offset)
            check(ready(bridge), "ready")
            code, body = ext(bridge, f"move index={pose([500] * 6)[3]} timeout=5")
            check(code == rhal.SKR_OK and body["result"] == "arrived" and abs(offset.angles[3] - 500) == 30, "a finger that rests 30 registers short of its target (inside the 50 the driver allows) has arrived", str(body))
            bridge.close()
        finally:
            offset.stop()

        section("extension — a motion that is waiting when the driver is closed or stopped ends at once")
        sim.place([1000] * 6)
        sim.deaf = True
        bridge = opened(sim)
        check(ready(bridge), "ready")
        outcomes: list[tuple[int, object]] = []
        mover = threading.Thread(target=lambda: outcomes.append(ext(bridge, f"move index={pose([300] * 6)[3]} timeout=8")))
        mover.start()
        time.sleep(0.3)
        bridge.emergency_stop()
        mover.join(1.5)
        check(not mover.is_alive() and outcomes and outcomes[0][0] == rhal.SKR_EPERM and "stop was latched" in str(outcomes[0][1]), "a move that is waiting for a hand that never arrives ends at once when the stop is latched", str(outcomes))
        bridge.close()
        bridge = opened(sim)
        check(ready(bridge), "ready")
        outcomes.clear()
        mover = threading.Thread(target=lambda: outcomes.append(ext(bridge, f"move index={pose([300] * 6)[3]} timeout=8")))
        mover.start()
        time.sleep(0.3)
        bridge.close()
        mover.join(1.5)
        check(not mover.is_alive() and outcomes and outcomes[0][0] == rhal.SKR_EPERM and "closed" in str(outcomes[0][1]), "and when the driver is closed", str(outcomes))
        sim.deaf = False

        section("extension — clear_errors and calibrate_force when the line fails, the hand refuses, or a fault comes up")
        sim.place([1000] * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.reject_writes = 1
        code, text = ext(bridge, "clear_errors")
        check(code == rhal.SKR_EINVAL and "does not fit the wire" in text, "a hand that refuses the clear is EINVAL, and the motion says the request did not fit", text)
        sim.mute_reads = True
        mover = threading.Thread(target=lambda: outcomes.append(ext(bridge, "clear_errors")))
        outcomes.clear()
        mover.start()
        mover.join(4.0)
        sim.mute_reads = False
        check(outcomes and outcomes[0][0] == rhal.SKR_EIO and "no reading has shown" in str(outcomes[0][1]), "a clear that was written and that no reading then confirmed is EIO, not done", str(outcomes))
        until(lambda: state(bridge)[0] == rhal.SKR_OK, 3)
        sim.silent = True
        code, text = ext(bridge, "clear_errors")
        sim.silent = False
        check(code == rhal.SKR_EIO and "did not carry" in text, "a request the line does not carry is EIO", text)
        until(lambda: state(bridge)[0] == rhal.SKR_OK, 3)
        original = bridge_module._CALIBRATION_S
        bridge_module._CALIBRATION_S = 1.0
        try:
            sim.reject_writes = 1
            code, _ = ext(bridge, "calibrate_force confirm=unloaded")
            check(code != rhal.SKR_OK and write(bridge, cmd(pose([900] * 6)))[0] == rhal.SKR_OK, "a calibration the hand refused to start does not leave the driver refusing commands for its eight seconds")
            outcomes.clear()
            mover = threading.Thread(target=lambda: outcomes.append(ext(bridge, "calibrate_force confirm=unloaded")))
            mover.start()
            check(until(lambda: ext(bridge, "status")[1]["calibrating"], 2), "the calibration is under way")
            time.sleep(0.45)                                    # the simulated hand's own calibration is over by now, and the driver's wait is not
            bridge.emergency_stop()
            mover.join(2.0)
            until(lambda: ext(bridge, "status")[1]["stop_delivered"], 2)
            check(not mover.is_alive() and outcomes and outcomes[0][0] == rhal.SKR_EPERM, "a stop ends a calibration's wait")
            check(bridge.release_stop() == rhal.SKR_OK and write(bridge, cmd(pose([800] * 6)))[0] == rhal.SKR_OK, "and commands are accepted at once after it is released, not when the calibration's time is up")
            sim.heal()
            until(lambda: state(bridge)[0] == rhal.SKR_OK and not ext(bridge, "status")[1]["fault"], 3)
            outcomes.clear()
            mover = threading.Thread(target=lambda: outcomes.append(ext(bridge, "calibrate_force confirm=unloaded")))
            mover.start()
            check(until(lambda: ext(bridge, "status")[1]["calibrating"], 2), "the calibration is under way")
            time.sleep(0.2)
            sim.set_fault(0, error=0b100)
            mover.join(3.0)
            check(outcomes and outcomes[0][0] == rhal.SKR_OK and outcomes[0][1]["result"] == "ended_with_a_fault", "a calibration that ends with a fault says so", str(outcomes))
            sim.heal()
            until(lambda: state(bridge)[0] == rhal.SKR_OK and not ext(bridge, "status")[1]["fault"], 3)
            outcomes.clear()
            bridge_module._CALIBRATION_S = 4.0
            mover = threading.Thread(target=lambda: outcomes.append(ext(bridge, "calibrate_force confirm=unloaded")))
            mover.start()
            check(until(lambda: ext(bridge, "status")[1]["calibrating"], 2), "the calibration is under way")
            began = time.monotonic()
            bridge.close()
            mover.join(2.0)
            check(not mover.is_alive() and time.monotonic() - began < 1.0 and outcomes and outcomes[0][0] == rhal.SKR_EPERM, "and one that is waiting when the driver is closed ends at once, not when its four seconds are up")
        finally:
            bridge_module._CALIBRATION_S = original
        sim.heal()

        section("extension — what the status says of the hand")
        sim.place([100, 200, 300, 400, 500, 600])
        sim.report_forces = [-100, 0, 50, 0, 0, -3]
        until(lambda: sim._load_shorts(frames.ADDR_FORCE_ACT, 6) == [-100, 0, 50, 0, 0, -3], 1)  # noqa: SLF001
        bridge = opened(sim)
        check(ready(bridge), "ready")
        status = ext(bridge, "status")[1]
        check(status["force_g"] == [-100, 0, 50, 0, 0, -3], "the measured force is signed: a finger pulling reads negative", str((status["force_g"], sim.report_forces, sim._load_shorts(frames.ADDR_FORCE_ACT, 6))))  # noqa: SLF001
        check(status["current_ma"] == sim._load_shorts(frames.ADDR_CURRENT, 6, signed=False), "the current is the CURRENT registers, not the force's")  # noqa: SLF001
        check(all(abs(a - b) < 1e-9 for a, b in zip(status["position_rad"], pose([100, 200, 300, 400, 500, 600]))), "every joint's position is converted with its own range")
        sim.report_forces = None
        sim.hold_status(1, 4)
        sim.set_fault(0, error=0b100)
        until(lambda: ext(bridge, "status")[1]["fault"], 2)
        status = ext(bridge, "status")[1]
        check("pinky" in str(status["fault"]) and "over-current" in str(status["fault"]) and "does not name" in status["status"][1], "a fault is described in the status, and a status the maker does not name says so")
        bridge.close()
        sim.heal()
        sim.place([1000] * 6)

        section("the descriptor — the motions' parameters, and a joint whose profile runs the other way")
        custom = {"angle_deg": [[100.0, 20.0], [20, 100], [20, 100], [20, 100], [-13, 50], [90, 160]], "full_range_s": 0.8, "force_set_max_g": 1000}
        reversed_sim = new_sim()
        try:
            code, said, bridge = opens_with(reversed_sim, model="custom", profile=custom)
            descriptor = json.loads(bridge.describe())
            pinky = descriptor["joints"][0]["limits"]
            check(code == rhal.SKR_OK and pinky["position_min_rad"] < pinky["position_max_rad"] and pinky["velocity_max_rad_s"] > 0, "a custom joint whose register 0 is the larger angle has a range from its smaller angle to its larger, and a speed that is positive", said)
            bridge.close()
        finally:
            reversed_sim.stop()
        bridge = opened(sim)
        descriptor = json.loads(bridge.describe())
        motions = {m["name"]: m["parameters"] for m in descriptor["motions"]}
        check(motions["calibrate_force"] == [{"name": "confirm", "required": True}] and {"name": "index", "unit": "rad", "required": False} in motions["move"] and {"name": "timeout", "unit": "s", "required": False} in motions["move"],
              "the descriptor says that a calibration needs the person's word and that a move's joints are in radians and its timeout in seconds")
        bridge.close()

        section("open — what a configuration may not say")
        for key, value, wanted in (("port", True, "gateway"), ("host", 5, "gateway"), ("rate_hz", True, "rate_hz")):
            code, said, _ = opens_with(sim, **({"device": None, "host": "127.0.0.1", "port": 1} | {key: value}))
            check(code == rhal.SKR_ENODEV and wanted in said, f"`{key}` of {value!r} is refused, and the refusal says why", said)
        speed_sim = new_sim()
        try:
            bridge = opened(speed_sim, speed=0.2009)
            check(speed_sim.speeds == [201] * 6, "a speed of 0.2009 is told to the hand as register 201, rounded and not cut")
            bridge.close()
        finally:
            speed_sim.stop()
    finally:
        sim.stop()


def main() -> int:
    # ── the maker's own worked frames ────────────────────────────────────────────────────────────────────
    section("frames — the maker's worked examples, byte for byte (series manual §2.2.1 and §2.2.2)")
    example_read = hex_("EB 90 01 04 11 0A 06 0C 32")
    example_answer = hex_("90 EB 01 0F 11 0A 06 64 00 64 00 64 00 64 00 E8 03 00 00 AC")
    example_write = hex_("EB 90 01 0F 12 CE 05 64 00 64 00 64 00 64 00 E8 03 00 00 70")
    example_ack = hex_("90 EB 01 04 12 CE 05 01 EB")
    check(frames.request_read(1, 1546, 12) == example_read, "reading the twelve bytes of ANGLE_ACT from hand 1 is the manual's frame, checksum 0x32")
    check(frames.parse_response(example_answer, 1, frames.CMD_READ, 1546, 12) == hex_("64 00 64 00 64 00 64 00 E8 03 00 00"), "the manual's answer parses to the manual's twelve bytes, checksum 0xAC")
    check(frames.shorts(frames.parse_response(example_answer, 1, frames.CMD_READ, 1546, 12)) == [100, 100, 100, 100, 1000, 0], "and those twelve bytes are the angles 100, 100, 100, 100, 1000 and 0")
    check(frames.request_write_shorts(1, 1486, [100, 100, 100, 100, 1000, 0]) == example_write, "writing 100, 100, 100, 100, 1000, 0 to ANGLE_SET is the manual's frame, checksum 0x70")
    check(frames.parse_response(example_ack, 1, frames.CMD_WRITE, 1486) == b"", "the manual's acknowledgement parses, checksum 0xEB, and carries no data")
    check(frames.HEADER_REQUEST == b"\xEB\x90" and frames.HEADER_RESPONSE == b"\x90\xEB" and frames.CMD_READ == 0x11 and frames.CMD_WRITE == 0x12, "a request starts EB 90, an answer 90 EB, a read is 0x11 and a write 0x12")
    check(frames.pack_shorts([1000]) == b"\xE8\x03" and frames.pack_shorts([-1]) == b"\xFF\xFF" and frames.pack_shorts([100]) == b"\x64\x00", "sixteen-bit values go low byte first, and -1 is 0xFFFF (§2.2: 100 is 0x6400, 1000 is 0xE803)")
    check(frames.shorts(b"\xFF\xFF", signed=True) == [-1] and frames.shorts(b"\xFF\xFF") == [65535] and frames.shorts(b"\x00\x80", signed=True) == [-32768], "a register reads signed or unsigned as it is asked to")

    section("frames — the maker's register map, pinned as the numbers the manual prints")
    check((frames.ADDR_CLEAR_ERROR, frames.ADDR_FORCE_CALIBRATION) == (1004, 1009), "CLEAR_ERROR is 1004 and GESTURE_FORCE_CLB is 1009")
    check((frames.ADDR_ANGLE_SET, frames.ADDR_FORCE_SET, frames.ADDR_SPEED_SET) == (1486, 1498, 1522), "ANGLE_SET is 1486, FORCE_SET 1498 and SPEED_SET 1522")
    check((frames.ADDR_ANGLE_ACT, frames.ADDR_FORCE_ACT, frames.ADDR_CURRENT) == (1546, 1582, 1594), "ANGLE_ACT is 1546, FORCE_ACT 1582 and CURRENT 1594")
    check((frames.ADDR_ERROR, frames.ADDR_STATUS, frames.ADDR_TEMP) == (1606, 1612, 1618), "ERROR is 1606, STATUS 1612 and TEMP 1618")
    check(frames.ADDR_STATUS - frames.ADDR_ERROR == frames.BLOCK_BYTES and frames.BLOCK_SHORTS == 12 and frames.BLOCK_BYTES == 6, "ERROR and STATUS are six bytes each and adjacent, and a group of six shorts is twelve bytes")
    check(frames.COMMAND_BLOCK_ADDRESS == 1486 and frames.COMMAND_BLOCK_BYTES == 48, "the three command groups are read as the 48 bytes from 1486 to 1533")
    check(JOINTS == ("pinky", "ring", "middle", "index", "thumb_bend", "thumb_rotation"), "the six joints are in the manual's order: little finger first, thumb rotation last")
    check(frames.STATUS_NAMES.keys() == {0, 1, 2, 3, 5, 6, 7} and frames.ERROR_BITS.keys() == {0, 1, 2, 3, 4}, "the manual names status codes 0 to 3 and 5 to 7 (not 4) and error bits 0 to 4")
    check(frames.CLEARABLE_ERROR_BITS == 0b11101, "every error bit but over-temperature (bit 1) is clearable (§2.4.3)")

    section("frames — what it refuses to build")
    for hand in (0, 255, -1, True, 1.0, "1"):
        check(raises(frames.ProtocolError, frames.request_read, hand, 1546, 12) is not None, f"a read for hand {hand!r} is refused (the manual's range is 1 to 254)")
    check(raises(frames.ProtocolError, frames.request_read, 254, 1546, 12) is None and raises(frames.ProtocolError, frames.request_read, 1, 1546, 12) is None, "hands 1 and 254 are accepted")
    for address in (-1, 65536, True, 1.5):
        check(raises(frames.ProtocolError, frames.request_read, 1, address, 12) is not None, f"a read at address {address!r} is refused")
    check(frames.request_read(1, 0, 1)[5:7] == b"\x00\x00" and frames.request_read(1, 65535, 255)[5:7] == b"\xFF\xFF", "addresses 0 and 65535 are accepted and go low byte first")
    for length in (0, 256, -1, True, 1.0):
        check(raises(frames.ProtocolError, frames.request_read, 1, 1546, length) is not None, f"a read of {length!r} bytes is refused (the length is one byte)")
    check(len(frames.request_read(1, 1546, 255)) == 9 and frames.request_read(1, 1546, 255)[7] == 255, "a read is nine bytes whatever it asks for, and 255 is accepted")
    check(raises(frames.ProtocolError, frames.request_write, 1, 1486, b"") is not None, "a write of no data is refused")
    check(raises(frames.ProtocolError, frames.request_write, 1, 1486, b"\x00" * 253) is not None and raises(frames.ProtocolError, frames.request_write, 1, 1486, b"\x00" * 252) is None, "a write of 252 bytes is the most: 253 would overflow the length byte")
    check(raises(frames.ProtocolError, frames.request_write, 1, 1486, "ab") is not None, "a write of text is refused")
    for value in (32768, -32769, True, 1.5, "1"):
        check(raises(frames.ProtocolError, frames.pack_shorts, [value]) is not None, f"{value!r} does not fit a signed sixteen-bit register")
    check(raises(frames.ProtocolError, frames.shorts, b"\x00") is not None, "an odd number of bytes is not a run of registers")
    check(frames.request_write(1, 1004, b"\x01") == hex_("EB 90 01 04 12 EC 03 01 07"), "CLEAR_ERROR's one byte is a write of length 4: EB 90 01 04 12 EC 03 01 07")

    section("frames — an answer is checked completely")
    good = example_answer
    check(raises(frames.ProtocolError, frames.parse_response, good[:-1], 1, 0x11, 1546, 12) is not None, "an answer a byte short is refused")
    check(raises(frames.ProtocolError, frames.parse_response, good + b"\x00", 1, 0x11, 1546, 12) is not None, "an answer a byte long is refused")
    check(raises(frames.ProtocolError, frames.parse_response, b"\xEB\x90" + good[2:], 1, 0x11, 1546, 12) is not None, "an answer that starts like a request (EB 90) is refused")
    bad_length = bytearray(good)
    bad_length[3] = 0x10
    check("length byte" in (raises(frames.ProtocolError, frames.parse_response, bytes(bad_length), 1, 0x11, 1546, 12) or ""), "an answer whose length byte disagrees with its size is refused, naming the length byte")
    bad_sum = bytearray(good)
    bad_sum[-1] ^= 1
    check("checksum" in (raises(frames.ProtocolError, frames.parse_response, bytes(bad_sum), 1, 0x11, 1546, 12) or ""), "a wrong checksum is refused, and said")
    check("hand 1" in (raises(frames.ProtocolError, frames.parse_response, good, 2, 0x11, 1546, 12) or ""), "an answer from another hand is refused, naming both")
    other = bytearray(good)
    other[2] = 2
    other[-1] = frames.checksum(bytes(other[2:-1]))
    check("hand 2" in (raises(frames.ProtocolError, frames.parse_response, bytes(other), 1, 0x11, 1546, 12) or ""), "an answer from hand 2 to a request for hand 1 is refused with a valid checksum")
    wrong_cmd = bytearray(good)
    wrong_cmd[4] = 0x12
    wrong_cmd[-1] = frames.checksum(bytes(wrong_cmd[2:-1]))
    check("command" in (raises(frames.ProtocolError, frames.parse_response, bytes(wrong_cmd), 1, 0x11, 1546, 12) or ""), "an answer to the wrong command is refused")
    wrong_addr = bytearray(good)
    wrong_addr[5] = 0x0B
    wrong_addr[-1] = frames.checksum(bytes(wrong_addr[2:-1]))
    check("register 1547" in (raises(frames.ProtocolError, frames.parse_response, bytes(wrong_addr), 1, 0x11, 1546, 12) or ""), "an answer for another register is refused with a valid checksum, naming it")
    check(raises(frames.ProtocolError, frames.parse_response, good, 1, 0x13, 1546, 12) is not None, "a command that is neither read nor write is refused")
    refusal = bytearray(example_ack)
    refusal[7] = 0
    refusal[-1] = frames.checksum(bytes(refusal[2:-1]))
    check("refused the write to register 1486" in (raises(frames.ProtocolError, frames.parse_response, bytes(refusal), 1, 0x12, 1486) or ""), "a write acknowledged with result 0 is a refused write, and says which register")
    check(raises(frames.ProtocolError, frames.parse_response, example_ack, 1, 0x12, 1486, 0) is None, "a good write acknowledgement is accepted")
    check(raises(frames.ProtocolError, frames.parse_response, example_ack + b"\x00", 1, 0x12, 1486) is not None, "a write acknowledgement of ten bytes is refused")
    check(frames.response_length(b"\x90\xEB\x01") is None and frames.response_length(b"\x90\xEB\x01\x0F") == 20 and frames.response_length(b"\x90\xEB\x01\x04") == 9, "an answer is its length byte plus five, once that byte has arrived")
    check(frames.find_response(b"\x01\x02\x90\xEB\x01") == 2 and frames.find_response(b"\xEB\x90\x01") is None and frames.find_response(b"") is None, "an answer starts at the first 90 EB, and a request's echo (EB 90) is not one")

    section("frames — units")
    check(RH56.angle_deg == ((19.0, 176.7),) * 4 + ((-13.0, 53.6), (90.0, 165.0)) and RH56.full_range_s == 0.8 and RH56.force_set_max_g == 1000, "the series manual's ranges, 0.8 s at speed 1000 and 1000 g, as the manual prints them")
    check(DFTP.angle_deg == ((20.0, 176.0),) * 4 + ((-13.0, 70.0), (90.0, 165.0)) and DFTP.full_range_s == 0.6 and DFTP.force_set_max_g == 3000, "the DFTP manual's ranges, 0.6 s at speed 1000 and 3000 g, as the manual prints them")
    for joint in range(6):
        low, high = RH56.angle_deg[joint]
        check(abs(frames.register_to_angle(0, joint, RH56) - math.radians(low)) < 1e-12 and abs(frames.register_to_angle(1000, joint, RH56) - math.radians(high)) < 1e-12 and
              abs(frames.register_to_angle(500, joint, RH56) - math.radians((low + high) / 2)) < 1e-12, f"{JOINTS[joint]}: register 0 is {low} degrees, 1000 is {high} and 500 is the middle")
    check(all(frames.angle_to_register(frames.register_to_angle(r, j, p), j, p) == r for p in (RH56, DFTP) for j in range(6) for r in range(0, 1001)), "every register of every joint of both models converts to an angle and back to itself")
    low5, high5 = RH56.angle_deg[4]
    check(frames.angle_to_register(math.radians(low5 + (high5 - low5) * 0.5004), 4, RH56) == 500 and frames.angle_to_register(math.radians(low5 + (high5 - low5) * 0.5006), 4, RH56) == 501, "an angle is rounded to the nearest register, not truncated")
    check(frames.angle_to_register(math.radians(18.0), 0, RH56) is None and frames.angle_to_register(math.radians(177.0), 0, RH56) is None, "an angle outside the joint's range has no register")
    check(frames.angle_to_register(math.radians(19.0), 0, RH56) == 0 and frames.angle_to_register(math.radians(176.7), 0, RH56) == 1000, "the two ends of the range are registers 0 and 1000")
    check(frames.angle_to_register(math.nan, 0, RH56) is None and frames.angle_to_register(math.inf, 0, RH56) is None, "a number that is not one has no register")
    check(frames.register_to_angle(500, 4, DFTP) != frames.register_to_angle(500, 4, RH56), "the same register is a different thumb bend on the two models")

    section("frames — the maker's statuses and errors")
    check([frames.classify_status(c) for c in range(0, 4)] == ["ok"] * 4 and all(frames.classify_status(c) == "fault" for c in (4, 5, 6, 7, 8, 9, 100, 255)), "status 0 to 3 are a finger doing its job; 5 to 7 are a stop for cause; 4 and above 7 are not in the table and fail closed")
    check(frames.classify_error(0) == "ok" and all(frames.classify_error(1 << bit) == "fault" for bit in range(8)), "any error bit is a fault, a bit the manual does not define included")
    check(frames.joint_fault([0] * 6, [2] * 6) is None, "a hand with no error and every finger at rest holds no fault")
    check(frames.joint_fault([0, 0, 4, 0, 0, 0], [2, 2, 5, 2, 2, 2]) == (2, 5, 4), "the first joint with a fault is returned with its status and error")
    check(frames.joint_fault([0, 8, 4, 0, 0, 0], [2, 2, 5, 2, 2, 2]) == (1, 2, 8), "and the first is the lowest-numbered, whichever is worse")
    check(frames.joint_fault([0] * 6, [2, 2, 2, 2, 2, 9]) == (5, 9, 0), "a status the manual does not name is a fault on its own")
    text = frames.describe_joint_fault(3, 6, 0b00000110)
    check("index" in text and "over-temperature error" in text and "over-current error" in text and "locked rotor" in text and "status 6" in text and "0x06" in text, "a fault is described by joint, bit names, status name and the raw bytes", text)
    check("does not define" in frames.describe_joint_fault(0, 2, 0x80) and "does not name" in frames.describe_joint_fault(0, 4, 0), "a bit or a status the maker does not define says so")

    sim = new_sim()
    try:
        # ── what it refuses to open on ───────────────────────────────────────────────────────────────────
        section("open — what it refuses, and the words it gives")
        base = settings(sim)

        def refuse(what: str, expect: tuple[str, ...], code: int = rhal.SKR_ENODEV, **change) -> None:
            body = dict(base)
            for key, value in change.items():
                if value is None:
                    body.pop(key, None)
                else:
                    body[key] = value
            got, text = refused(json.dumps(body))
            check(got == code and all(word in text for word in expect), what, f"{got}: {text}")

        code, text = refused("{ not json")
        check(code == rhal.SKR_EINVAL and "not JSON" in text, "a configuration that is not JSON is refused as invalid", text)
        code, text = refused("[1, 2]")
        check(code == rhal.SKR_EINVAL and "not a JSON object" in text, "a configuration that is not an object is refused as invalid", text)
        refuse("a key the driver does not read is refused, naming the keys it does", ("'speeds'", "keys it reads"), speeds=1)
        refuse("no protocol is refused, and the driver says what it speaks", ("`protocol`", "private", "RS-485"), protocol=None)
        refuse("Modbus is refused by name, with the reason the byte registers give", ("'modbus_rtu'", "Modbus RTU and Modbus TCP", "byte order", "guessing"), protocol="modbus_rtu")
        refuse("Modbus TCP is refused the same way", ("'modbus_tcp'", "refuses them by name"), protocol="modbus_tcp")
        refuse("neither device nor host is refused, and it says it does not scan", ("neither a serial `device` nor a `host`", "does not scan"), device=None)
        refuse("both device and host is refused", ("neither a serial `device` nor a `host`", "or names both"), host="127.0.0.1", port=5)
        refuse("a device that is not a path is refused", ("`device` of 5",), device=5)
        refuse("an empty device is refused", ("`device` of ''",), device="")
        refuse("a baud rate the hand cannot be set to is refused, naming those it can", ("`baud` of 9600", "115200", "57600", "19200", "921600"), baud=9600)
        refuse("a baud rate that is a bool is refused", ("`baud` of True",), baud=True)
        refuse("a host with no port is refused", ("`host` of 'gateway'", "`port` of None"), device=None, host="gateway", port=None)
        refuse("a port of zero is refused", ("`port` of 0",), device=None, host="gateway", port=0)
        refuse("a port past 65535 is refused", ("`port` of 65536",), device=None, host="gateway", port=65536)
        refuse("a hand id of 0 is refused, with the factory setting", ("`hand_id` of 0", "1 to 254", "factory setting is 1"), hand_id=0)
        refuse("a hand id of 255 is refused", ("`hand_id` of 255",), hand_id=255)
        refuse("a hand id that is a bool is refused", ("`hand_id` of True",), hand_id=True)
        refuse("no model is refused, saying why a model is the operator's", ("`model`", "identifies its model", "series manual", "DFTP"), model=None)
        refuse("a model the driver does not know is refused", ("`model` of 'RH56X'",), model="RH56X")
        refuse("a profile with a named model is refused", ("`profile` was given", "`model: \"custom\"`"), profile={"angle_deg": []})
        refuse("a custom model with no profile is refused, naming what it needs", ("angle_deg", "full_range_s", "force_set_max_g"), model="custom")
        custom = {"angle_deg": [[19, 176.7]] * 4 + [[-13, 53.6], [90, 165]], "full_range_s": 0.7, "force_set_max_g": 2000}
        refuse("a custom profile with a key too many is refused", ("exactly",), model="custom", profile={**custom, "extra": 1})
        refuse("a custom profile with five angle pairs is refused", ("six [degrees",), model="custom", profile={**custom, "angle_deg": custom["angle_deg"][:5]})
        refuse("a custom profile with an angle that is not a number is refused", ("six [degrees",), model="custom", profile={**custom, "angle_deg": [[19, "x"]] + custom["angle_deg"][1:]})
        refuse("a custom profile with a pair that is not a range is refused", ("not a range of at most 360",), model="custom", profile={**custom, "angle_deg": [[19, 19]] + custom["angle_deg"][1:]})
        refuse("a custom profile with a range past 360 degrees is refused", ("not a range of at most 360",), model="custom", profile={**custom, "angle_deg": [[0, 400]] + custom["angle_deg"][1:]})
        refuse("a custom profile with a zero full-range time is refused", ("`profile.full_range_s` of 0",), model="custom", profile={**custom, "full_range_s": 0})
        refuse("a custom profile with a full-range time of a minute and a bit is refused", ("`profile.full_range_s` of 61",), model="custom", profile={**custom, "full_range_s": 61})
        refuse("a custom profile with a fractional force range is refused", ("`profile.force_set_max_g` of 1000.5",), model="custom", profile={**custom, "force_set_max_g": 1000.5})
        refuse("a custom profile with a force range of zero is refused", ("`profile.force_set_max_g` of 0",), model="custom", profile={**custom, "force_set_max_g": 0})
        refuse("no stop_mode is refused, with what `hold` is and what is not offered", ("`stop_mode`", "document no stop", "`hold`", "speed of zero"), stop_mode=None)
        refuse("a stop_mode that is not `hold` is refused", ("`stop_mode`",), stop_mode="release")
        refuse("no simulator claim is refused, saying why", ("`simulator`", "costs a tier"), simulator=None)
        refuse("a simulator claim that is a string is refused", ("`simulator`",), simulator="yes")
        refuse("no speed is refused, saying what the register means and that zero is undocumented", ("`speed`", "SPEED_SET 1000", "0.8 s", "zero is not documented"), speed=None)
        refuse("a speed of zero is refused", ("`speed`", "0 "), speed=0)
        refuse("a speed above 1 is refused", ("`speed`",), speed=1.5)
        refuse("a speed that is not finite is refused", ("`speed`",), speed=float("nan"))
        refuse("a speed that is a bool is refused", ("`speed`",), speed=True)
        refuse("no force is refused, in grams and with the model's top", ("`force_g`", "grams from 0 to 1000", "FORCE_SET"), force_g=None)
        refuse("a force above the model's range is refused", ("`force_g` in the configuration, or 1001",), force_g=1001)
        refuse("the same force is allowed on the DFTP, whose range is three times the size", ("grams from 0 to 3000",), model="RH56DFTP", force_g=3001)
        refuse("a negative force is refused", ("`force_g`",), force_g=-1)
        refuse("a fractional force is refused", ("`force_g`",), force_g=10.5)
        refuse("a force list of five is refused", ("`force_g`",), force_g=[1, 2, 3, 4, 5])
        refuse("a force list with a bool is refused", ("`force_g`",), force_g=[1, 2, 3, 4, 5, True])
        refuse("limits that are not a mapping are refused", ("`limits` is a mapping",), limits=[0, 1])
        refuse("limits for a joint that does not exist are refused", ("`limits` is a mapping",), limits={"wrist": [0, 1]})
        refuse("a limit that is not two numbers is refused", ("`limits.index` is [min, max]",), limits={"index": [0.5]})
        refuse("a limit that is not a range is refused", ("`limits.index` of [1.0, 1.0] is not a range",), limits={"index": [1.0, 1.0]})
        refuse("a limit wider than the profile is refused, saying a configuration may narrow and never widen", ("wider than the joint's range", "never widen"), limits={"index": [0.0, 3.0]})
        refuse("a limit below the profile is refused", ("wider than the joint's range",), limits={"thumb_bend": [-0.3, 0.5]})
        refuse("roles that are not a mapping are refused", ("`roles` is a mapping",), roles=[1])
        refuse("a role for a joint that does not exist is refused", ("`roles` is a mapping",), roles={"wrist": "x"})
        refuse("a rate above the ceiling is refused", ("`rate_hz` of 51",), rate_hz=51)
        refuse("a rate of zero is refused", ("`rate_hz` of 0",), rate_hz=0)
        refuse("a transaction budget below ten milliseconds is refused", ("`timeout_ms` of 5",), timeout_ms=5)
        refuse("a staleness bound of a bool is refused", ("`stale_ms` of True",), stale_ms=True)
        refuse("an open timeout above a minute is refused", ("`open_timeout_ms` of 60001",), open_timeout_ms=60001)

        # ── open, and what it declares ───────────────────────────────────────────────────────────────────
        section("open — the descriptor and the first reading")
        sim.place([900, 800, 700, 600, 500, 400])
        before = len(sim.target_writes)
        bridge = opened(sim)
        d = json.loads(bridge.describe())
        check(d["robot_slug"] == "inspire-rh56" and d["simulator"] is True and d["base"] == "fixed" and d["dof"] == 6 and d["verified"] is False and d["digest"] is None, "the descriptor says which hand, that it is a simulation, a fixed base, six degrees of freedom and not verified")
        check([j["name"] for j in d["joints"]] == list(JOINTS) and all(j["type"] == "revolute" and j["control_modes"] == ["position"] for j in d["joints"]), "six revolute joints in the manual's order, each taking a position")
        check([j["role"] for j in d["joints"]] == [None, None, None, "right_hand.finger.flex#0", None, "right_hand.thumb.rotate"], "a role is the operator's and is carried where it was given and nowhere else")
        low0, high0 = d["joints"][0]["limits"]["position_min_rad"], d["joints"][0]["limits"]["position_max_rad"]
        check(abs(low0 - math.radians(19.0)) < 1e-9 and abs(high0 - math.radians(176.7)) < 1e-9, "the pinky's limits are the manual's 19 and 176.7 degrees in radians")
        check(abs(d["joints"][4]["limits"]["position_min_rad"] - math.radians(-13.0)) < 1e-9 and abs(d["joints"][4]["limits"]["position_max_rad"] - math.radians(53.6)) < 1e-9, "the thumb bend's limits are -13 and 53.6 degrees")
        check(abs(d["joints"][5]["limits"]["position_min_rad"] - math.radians(90.0)) < 1e-9 and abs(d["joints"][5]["limits"]["position_max_rad"] - math.radians(165.0)) < 1e-9, "the thumb rotation's limits are 90 and 165 degrees")
        expected_speed = SPEED * math.radians(176.7 - 19.0) / 0.8
        check(abs(d["joints"][0]["limits"]["velocity_max_rad_s"] - expected_speed) < 1e-9, "a joint's velocity bound is its range over the manual's no-load time, at the operator's fraction of top speed")
        check(abs(d["joints"][4]["limits"]["velocity_max_rad_s"] - SPEED * math.radians(53.6 + 13.0) / 0.8) < 1e-9, "and the thumb bend's is its own range's")
        check(d["end_effectors"] == [{"name": "hand", "role": None, "kind": "dexterous_hand", "dof": 6, "max_grip_force_n": None, "max_opening_m": None}], "one end effector, a dexterous hand, with no grip force in newtons claimed")
        check(d["envelope"] is None and d["capabilities"] == ["safety.estop"] and d["extensions"] == [TOKEN] and d["sensors"] == [], "no envelope is claimed, the one capability is the software stop, and the one extension is this driver's")
        check([m["name"] for m in d["motions"]] == ["status", "move", "clear_errors", "calibrate_force"], "four motions are declared")
        check(d["control"] == {"modes": ["position"], "rate_hz_max": 50.0, "rate_hz_min": 1.0}, "the control block is position at the configured rate")
        check(len(sim.target_writes) == before, "opening writes no target: a hand that was mid-motion, or holding, is not disturbed")
        check(sim.speeds == [500] * 6 and sim.force_limits == [FORCE] * 6, "opening tells the hand the operator's speed and force limits")
        check(ready(bridge), "the first reading arrives")
        rc, out = state(bridge)
        check(rc == rhal.SKR_OK and [round(math.degrees(p), 3) for p in out.position_rad] == [round(math.degrees(p), 3) for p in pose([900, 800, 700, 600, 500, 400])], "the state is the hand's angles in radians")
        check((out.flags & rhal.HOMING_MASK) == rhal.HOMING_NOT_REQUIRED and not out.flags & (rhal.FLAG_FAULT | rhal.FLAG_ESTOP | rhal.FLAG_DEGRADED) and out.fault_code == 0, "a healthy hand: no homing required, no fault, no stop, not degraded")
        check(all(math.isnan(v) for v in list(out.velocity_rad_s) + list(out.effort_nm)), "velocity and torque are not measured, and are not zero")
        check(out.seq > 0 and out.stamp_ns > 0, "the state carries a sequence and a stamp")
        rc2, out2 = state(bridge)
        check(out2.seq == out.seq + 1, "the sequence counts every read")
        bridge.close()
        sim.place([1000] * 6)

        section("open — a hand that is not there, and a hand that is somebody else's")
        gone = new_sim()
        gone.silent = True
        code, text, _bridge = opens_with(gone, open_timeout_ms=400)
        check(code == rhal.SKR_EIO and "no hand answered at id 1" in text and "115200 and 1 from the factory" in text, "a hand that does not answer is EIO, with the id, the line and the factory settings", text)
        gone.stop()
        other_hand = new_sim(hand=2)
        code, text, _bridge = opens_with(other_hand, open_timeout_ms=400)
        check(code == rhal.SKR_EIO and "id 1" in text, "a hand with another id is not answered for", text)
        other_hand.stop()
        other_hand = new_sim(hand=2)
        bridge = opened(other_hand, hand_id=2)
        check(ready(bridge) and ext(bridge, "status")[1]["hand_id"] == 2, "and one with id 2 is opened as id 2")
        bridge.close()
        other_hand.stop()
        code, text, _bridge = opens_with(sim, device="/dev/null/not-a-port", open_timeout_ms=300)
        check(code == rhal.SKR_EIO and "could not be opened" in text, "a device that does not exist is EIO", text)
        code, text, _bridge = opens_with(sim, device=None, host="127.0.0.1", port=1, open_timeout_ms=300)
        check(code == rhal.SKR_EIO and "could not be opened" in text, "a gateway that is not listening is EIO", text)

        section("open — a hand that holds a fault, or is not where the operator wants it")
        sim.set_fault(2, error=0b100)
        code, text, bridge = opens_with(sim)
        check(code == rhal.SKR_OK and "middle" in text and "over-current error" in text and "clear_errors" in text, "a hand holding a fault opens (so that it can be cleared) and the driver says which joint and what to do", text)
        check(ready(bridge), "and it is read")
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_FAULT and out.flags & rhal.FLAG_DEGRADED and out.fault_code == (3 << 16 | 5 << 8 | 4), "its fault flags are set and the word is joint 3, status 5, error 0x04")
        code, said = write(bridge, cmd(pose([1000] * 6)))
        check(code == rhal.SKR_EPERM and "middle" in said and "clear_errors" in said and "was not sent" in said, "a command is refused naming the joint, and the motion that clears it", said)
        check(len(sim.target_writes) == 0, "and nothing reached the hand")
        bridge.close()
        sim.cool(2)
        sim.set_fault(2, error=0)
        sim.hold_status(2, 2)
        sim.writes.clear()
        sim.heal()

        section("open — a hand that was already moving is not disturbed, and a hand with no target is where it is")
        sim.place([1000] * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        write(bridge, cmd(pose([200] * 6)))
        time.sleep(0.01)
        bridge.close()
        moving = sim.angles
        sim.writes.clear()
        sim.target_writes.clear()
        again = opened(sim)
        check(ready(again) and not any(a for a in sim.target_writes), "a driver that opens on a hand that is on its way writes no target")
        check(until(lambda: sim.angles == [200] * 6, 3), "and the hand arrives where the first driver sent it")
        status = ext(again, "status")[1]
        check(status["image"] == [200] * 6, "the image is the hand's own record of its targets, so a joint that is not commanded keeps it", str(status["image"]))
        again.close()
        sim.place([500, 150, 600, 120, 700, 130])
        sim.set_registers(frames.ADDR_ANGLE_SET, [-1, 150, -1, 120, -1, 130])
        blind = opened(sim)
        check(ready(blind) and ext(blind, "status")[1]["image"] == [500, 150, 600, 120, 700, 130], "a target of -1 in the hand's record is not carried: that joint is where it is")
        blind.close()
        sim.place([1000] * 6)
        sim.target_writes.clear()
        sim.set_registers(frames.ADDR_ANGLE_SET, [1000] * 6)  # noqa: SLF001

        # ── commands ─────────────────────────────────────────────────────────────────────────────────────
        section("write_command — what it refuses, whole, and without touching the hand")
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.target_writes.clear()
        sim.writes.clear()
        full = pose([800] * 6)
        for what, command, words in (
            ("a command of five joints", cmd(full[:5]), ("5 joints", "six", "nothing was sent")),
            ("a command of seven joints", cmd(full + [0.0]), ("7 joints",)),
            ("a velocity command", cmd(full, mode=rhal.MODE_VELOCITY), ("mode 1", "takes positions")),
            ("a torque command", cmd(full, mode=rhal.MODE_TORQUE), ("mode 2",)),
        ):
            code, said = write(bridge, command)
            check(code == rhal.SKR_EINVAL and all(w in said for w in words), f"{what} is refused as invalid", said)
        bad = list(full)
        bad[2] = math.nan
        code, said = write(bridge, cmd(bad))
        check(code == rhal.SKR_EINVAL and "middle" in said and "not finite" in said, "a NaN on one joint refuses the whole command, naming the joint", said)
        bad[2] = math.inf
        code, said = write(bridge, cmd(bad))
        check(code == rhal.SKR_EINVAL and "not finite" in said, "an infinity refuses it too", said)
        bad = list(full)
        bad[5] = math.radians(166.0)
        code, said = write(bridge, cmd(bad))
        check(code == rhal.SKR_EINVAL and "thumb_rotation" in said and "outside its range" in said and "refused whole" in said, "an angle outside one joint's range refuses the whole command, naming it", said)
        bad = list(full)
        bad[4] = math.radians(-14.0)
        code, said = write(bridge, cmd(bad))
        check(code == rhal.SKR_EINVAL and "thumb_bend" in said, "an angle below a joint's range is refused", said)
        bad = list(full)
        bad[1] = math.nan
        code, said = write(bridge, cmd(bad, mask=[1, 0, 1, 1, 1, 1]))
        check(code == rhal.SKR_OK, "a NaN in a joint the mask holds is not read, and the command is taken")
        until(lambda: sim.angles[0] == 800 and sim.angles[2] == 800)
        sim.target_writes.clear()
        bad = list(full)
        bad[5] = math.nan
        bad[0] = math.radians(100)
        code, said = write(bridge, cmd(bad))
        check(code == rhal.SKR_EINVAL and not sim.target_writes, "a refused command's valid first half is not written (F281: the refusal covers every joint before any is claimed)")
        write(bridge, cmd([math.radians(100), math.radians(100), math.radians(100), math.radians(100), math.radians(20), math.radians(100)], mask=[0] * 6))
        check(not sim.target_writes, "a command whose mask holds every joint writes nothing and is accepted")
        check(write(bridge, cmd(full, mask=[0] * 6))[0] == rhal.SKR_OK, "a command with an all-zero mask is accepted, as an empty command")
        said_twice = io.StringIO()
        with contextlib.redirect_stderr(said_twice):
            bridge.write_command(cmd(full[:5]), 0)
            bridge.write_command(cmd(full[:5]), 0)
        check(said_twice.getvalue().count("5 joints") == 1, "a refusal that repeats is said once, not at the control rate")
        bridge.close()
        check(write(bridge, cmd(full))[0] == rhal.SKR_EPERM, "a command to a driver that is not open is EPERM")
        check(state(bridge)[0] == rhal.SKR_EPERM, "a state read of a driver that is not open is EPERM")
        check(bridge.read_sensor("x", rhal.Frame()) == rhal.SKR_ENOSYS, "a sensor read is not implemented")

        section("write_command — what it writes")
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.target_writes.clear()
        sim.writes.clear()
        target = [700, 650, 600, 550, 300, 200]
        code, said = write(bridge, cmd(pose(target)))
        check(code == rhal.SKR_OK and said == "", "a command inside every range is taken, and nothing is said")
        check(sim.target_writes and sim.target_writes[-1][1] == tuple(target), f"the hand was told the six registers, in the manual's order (told {sim.target_writes[-1][1] if sim.target_writes else None})")
        check(len([w for w in sim.writes if w[1] == frames.ADDR_ANGLE_SET]) == 1, "all six joints went in one write")
        check(until(lambda: sim.angles == target, 3), "the fingers go there")
        check(until(lambda: state(bridge)[1].position_rad[0] == pose(target)[0] and not state(bridge)[1].flags & rhal.FLAG_DEGRADED), "and the state follows without being degraded")
        sim.target_writes.clear()
        sim.writes.clear()
        write(bridge, cmd(pose([500, 0, 0, 500, 0, 0]), mask=[1, 0, 0, 1, 0, 0]))
        check([w for w in sim.writes if w[1] < 1534 and w[1] >= 1486] == [(w[0], 1486, frames.pack_shorts([500])) for w in sim.writes if w[1] == 1486] + [(w[0], 1492, frames.pack_shorts([500])) for w in sim.writes if w[1] == 1492],
              "a command that applies the pinky and the index writes those two registers, each alone, and nothing between them", str([(w[1], w[2].hex()) for w in sim.writes]))
        sim.writes.clear()
        write(bridge, cmd(pose([400, 400, 400, 0, 0, 0]), mask=[1, 1, 1, 0, 0, 0]))
        check([(w[1], w[2]) for w in sim.writes] == [(1486, frames.pack_shorts([400, 400, 400]))], "a run of neighbours is one write of exactly their registers")
        sim.writes.clear()
        write(bridge, cmd(pose([0, 0, 0, 0, 0, 300]), mask=[0, 0, 0, 0, 0, 1]))
        check([(w[1], w[2]) for w in sim.writes] == [(1496, frames.pack_shorts([300]))], "the last joint alone is written at its own address, 1496")
        check(sim.target_writes and all(-1 not in t[1] for t in sim.target_writes), "-1 is never sent")
        check(until(lambda: sim.angles[5] == 300 and sim.angles[0] == 400, 3), "and the joints a command did not apply were not touched")
        sim.place([1000] * 6)
        write(bridge, cmd(pose([1000] * 6)))
        # the limit is inclusive, and the edge registers are reached exactly
        sim.target_writes.clear()
        code, _ = write(bridge, cmd([bridge._limits[j][0] for j in range(6)]))  # noqa: SLF001
        check(code == rhal.SKR_OK and sim.target_writes and sim.target_writes[-1][1] == (0, 0, 0, 0, 0, 0), "the lower limit of every joint is accepted and is register 0")
        code, _ = write(bridge, cmd([bridge._limits[j][1] for j in range(6)]))  # noqa: SLF001
        check(code == rhal.SKR_OK and sim.target_writes[-1][1] == (1000,) * 6, "the upper limit of every joint is accepted and is register 1000")
        bridge.close()

        section("write_command — the operator's limits narrow a joint and never widen it")
        bridge = opened(sim, limits={"index": [1.0, 2.0], "thumb_bend": [0.0, 0.5]})
        check(ready(bridge), "ready")
        d = json.loads(bridge.describe())
        check(d["joints"][3]["limits"]["position_min_rad"] == 1.0 and d["joints"][3]["limits"]["position_max_rad"] == 2.0, "the descriptor declares the narrowed limits")
        check(d["joints"][0]["limits"]["position_min_rad"] == math.radians(19.0), "a joint with no limit keeps the profile's")
        inside = pose([800] * 6)
        inside[3], inside[4] = 1.5, 0.25
        check(write(bridge, cmd(inside))[0] == rhal.SKR_OK, "a command inside the narrowed limits is taken")
        outside = list(inside)
        outside[3] = 2.5
        code, said = write(bridge, cmd(outside))
        check(code == rhal.SKR_EINVAL and "index" in said and "outside its range" in said, "a command outside the narrowed limits, though inside the hand's range, is refused", said)
        bridge.close()

        section("the DFTP model: another range, another speed")
        dftp = new_sim(profile="RH56DFTP")
        bridge = opened(dftp, model="RH56DFTP", force_g=2500)
        check(ready(bridge), "ready")
        d = json.loads(bridge.describe())
        check(d["robot_slug"] == "inspire-rh56dftp" and abs(d["joints"][4]["limits"]["position_max_rad"] - math.radians(70.0)) < 1e-9 and abs(d["joints"][0]["limits"]["position_min_rad"] - math.radians(20.0)) < 1e-9, "the descriptor carries the DFTP manual's ranges")
        check(abs(d["joints"][0]["limits"]["velocity_max_rad_s"] - SPEED * math.radians(156.0) / 0.6) < 1e-9, "and its 0.6 s for the whole range")
        check(dftp.force_limits == [2500] * 6, "a force the DFTP allows and the series does not is written")
        write(bridge, cmd([frames.register_to_angle(250, j, DFTP) for j in range(6)]))
        check(until(lambda: dftp.angles == [250] * 6, 3), "a command lands on the register the DFTP's own range gives")
        bridge.close()
        dftp.stop()

        section("a custom profile")
        profile = {"angle_deg": [[10, 100]] * 4 + [[0, 50], [0, 90]], "full_range_s": 0.5, "force_set_max_g": 400}
        bridge = opened(sim, model="custom", profile=profile, force_g=400)
        check(ready(bridge), "ready")
        d = json.loads(bridge.describe())
        check(d["robot_slug"] == "inspire-custom" and abs(d["joints"][0]["limits"]["position_max_rad"] - math.radians(100)) < 1e-9 and abs(d["joints"][0]["limits"]["velocity_max_rad_s"] - SPEED * math.radians(90) / 0.5) < 1e-9, "the descriptor carries the operator's profile")
        write(bridge, cmd([math.radians(55)] * 4 + [math.radians(25), math.radians(45)]))
        check(until(lambda: sim.angles == [500] * 6, 3), "and a command is converted through it")
        bridge.close()
        sim.place([1000] * 6)

        # ── the state, and what degrades it ──────────────────────────────────────────────────────────────
        section("read_state — a hand that was told something and did not take it, and a hand that forgot its parameters")
        sim.place([1000] * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.deaf = True
        write(bridge, cmd(pose([300] * 6)))
        time.sleep(0.8)
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_FAULT, "a target the hand acknowledged and does not hold, longer than the grace, is DEGRADED and not a fault (F175)")
        sim.deaf = False
        write(bridge, cmd(pose([300] * 6)))
        check(until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3), "and it clears when the hand takes the target")
        sim.set_registers(frames.ADDR_SPEED_SET, [500] * 6)  # noqa: SLF001
        sim.set_registers(frames.ADDR_FORCE_SET, [FORCE] * 6)  # noqa: SLF001
        sim.set_registers(frames.ADDR_SPEED_SET, [1000, 500, 500, 500, 500, 500])  # noqa: SLF001
        check(until(lambda: sim.speeds == [500] * 6, 3), "a hand whose speed was changed behind the driver's back is told the operator's again")
        sim.set_registers(frames.ADDR_FORCE_SET, [0, FORCE, FORCE, FORCE, FORCE, FORCE])  # noqa: SLF001
        check(until(lambda: sim.force_limits == [FORCE] * 6, 3), "and so is one whose force limit was")
        check(ext(bridge, "status")[1]["parameters_reasserted"] >= 2, "and the driver counts how often it had to")
        bridge.close()

        section("read_state — every fault, and what the word says")
        for bit, name in frames.ERROR_BITS.items():
            sim.place([1000] * 6)
            sim.heal()
            bridge = opened(sim)
            sim.set_fault(1, error=1 << bit)
            until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT)
            rc, out = state(bridge)
            status = 6 if bit == 0 else 5 if bit == 2 else 7
            check(out.flags & rhal.FLAG_FAULT and out.flags & rhal.FLAG_DEGRADED and out.fault_code == (2 << 16 | status << 8 | (1 << bit)), f"error bit {bit} ({name}) on the ring finger is a fault, and the word is joint 2, status {status}, error 0x{1 << bit:02X}", hex(out.fault_code))
            code, said = write(bridge, cmd(pose([800] * 6)))
            check(code == rhal.SKR_EPERM and "ring" in said and name in said, f"a command over {name} is refused naming the joint and the bit", said)
            bridge.close()
        sim.heal()
        bridge = opened(sim)
        sim.hold_status(0, 4)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT)
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_FAULT and out.fault_code == (1 << 16 | 4 << 8), "status 4, which the manual does not name, is a fault: the driver does not command what it cannot read")
        code, said = write(bridge, cmd(pose([800] * 6)))
        check(code == rhal.SKR_EPERM and "does not name" in said, "and the refusal says the maker does not name it", said)
        sim.hold_status(0, 2)
        sim.set_fault(5, error=0x20, status=2)
        until(lambda: state(bridge)[1].fault_code == (6 << 16 | 2 << 8 | 0x20))
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_FAULT and out.fault_code == (6 << 16 | 2 << 8 | 0x20), "an error bit the manual does not define is a fault too")
        sim.heal()
        sim.set_fault(0, error=0, status=2)
        sim.set_fault(3, error=0b110)
        until(lambda: state(bridge)[1].fault_code == (4 << 16 | 5 << 8 | 0b110))
        check(state(bridge)[1].fault_code == (4 << 16 | 5 << 8 | 0b110), "two bits at once are one error byte, and the status of the stop is the lower of the two reasons the simulated hand gives")
        sim.heal()
        sim.set_fault(0, error=1)
        sim.set_fault(4, error=4)
        until(lambda: state(bridge)[1].fault_code >> 16 == 1)
        check(state(bridge)[1].fault_code >> 16 == 1, "the word names the lowest-numbered joint that holds a fault")
        bridge.close()
        sim.heal()

        section("read_state — a line that dies, and one that comes back")
        sim.place([1000] * 6)
        bridge = opened(sim, stale_ms=300)
        check(ready(bridge), "ready")
        sim.drop = True
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 3), "a hand that stops answering is ETIMEDOUT")
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_DEGRADED, "and DEGRADED")
        check(all(not math.isnan(p) for p in out.position_rad), "and the last angles it did report are still there, flagged and not blanked")
        sim.drop = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3), "and it is read again once the hand answers")
        status = ext(bridge, "status")[1]
        check(status["link_dead"] is None, "the status no longer calls the line dead")
        bridge.close()

        # ── the stop ─────────────────────────────────────────────────────────────────────────────────────
        section("the stop — latched, written as the hand's own angles, and not released by a hand that has not taken it")
        slow = new_sim(time_scale=1.0)
        try:
            bridge = opened(slow, speed=0.05)
            check(ready(bridge), "ready")
            write(bridge, cmd(pose([0] * 6)))
            time.sleep(0.4)
            began = time.monotonic()
            check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.02, "emergency_stop returns at once, without touching the line")
            check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2), "and the reading thread writes the hold")
            at_stop = slow.angles
            hold = slow.targets
            check(all(abs(h - a) <= 2 for h, a in zip(hold, at_stop)), f"the targets are where the fingers were (angles {at_stop}, targets {hold})")
            time.sleep(0.5)
            check(slow.angles == at_stop or all(abs(a - b) <= 1 for a, b in zip(slow.angles, at_stop)), "and the fingers stopped there")
            check(until(lambda: state(bridge)[1].flags & rhal.FLAG_ESTOP), "the state says the stop is latched")
            code, said = write(bridge, cmd(pose([700] * 6)))
            check(code == rhal.SKR_EPERM, "a command while latched is EPERM")
            check(ext(bridge, "move index=1.0")[0] == rhal.SKR_EPERM and "stop is latched" in ext(bridge, "move index=1.0")[1], "so is a move, and it says why")
            check(ext(bridge, "status")[0] == rhal.SKR_OK and ext(bridge, "status")[1]["stop_latched"] is True and ext(bridge, "status")[1]["stop_delivered"] is True, "a status is still answered, and says the stop was delivered")
            check(bridge.release_stop() == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_ESTOP, "release_stop clears the latch once the hand's record shows the hold")
            code, _ = write(bridge, cmd(pose([700] * 6)))
            check(code == rhal.SKR_OK, "and commands are accepted again")
            bridge.close()
        finally:
            slow.stop()

        section("the stop — a line that is down when it is called")
        sim.place([1000] * 6)
        bridge = opened(sim, stale_ms=300)
        check(ready(bridge), "ready")
        write(bridge, cmd(pose([100] * 6)))
        sim.drop = True
        began = time.monotonic()
        check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.02, "a stop with the line down still returns OK within its budget: the latch is the stop (F298: the call is 20 ms at most, and was 101 ms behind a blackhole)")
        check(ext(bridge, "status")[1]["stop_delivered"] is False, "and the status says it has not been delivered")
        said_release, said = said_by(bridge.release_stop)
        check(said_release == rhal.SKR_EIO and "stays latched" in said, "release_stop on a hand that has not taken the stop is EIO and the latch stays", said)
        check(write(bridge, cmd(pose([700] * 6)))[0] == rhal.SKR_EPERM, "and a command is still refused")
        sim.drop = False
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3), "the loop writes the stop as soon as the line carries it")
        check(bridge.release_stop() == rhal.SKR_OK, "and then the stop can be released")
        bridge.close()

        section("the stop — a command that is waiting while it is called does not follow it")
        sim.place([1000] * 6)
        bridge = opened(sim, timeout_ms=300)
        check(ready(bridge), "ready")
        sim.delay_s = 0.25
        results: list[int] = []
        sender = threading.Thread(target=lambda: results.append(write(bridge, cmd(pose([100] * 6)))[0]))
        sender.start()
        time.sleep(0.05)
        bridge.emergency_stop()
        sender.join(3.0)
        sim.delay_s = 0.0
        until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3)
        check(not sender.is_alive() and sim.targets != [100] * 6, "a command in flight when the stop arrives is followed by the hold, and the stop's targets are what the hand holds", str(sim.targets))
        check(bridge.release_stop() == rhal.SKR_OK, "released")
        bridge.close()

        section("the stop — a close writes nothing, and a new open is a new session")
        sim.place([1000] * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        bridge.emergency_stop()
        until(lambda: ext(bridge, "status")[1]["stop_delivered"])
        bridge.close()
        sim.writes.clear()
        bridge2 = opened(sim)
        check(ready(bridge2) and not state(bridge2)[1].flags & rhal.FLAG_ESTOP, "a session that was latched is not latched in the next")
        check(write(bridge2, cmd(pose([900] * 6)))[0] == rhal.SKR_OK, "and takes commands")
        bridge2.close()
        sim.writes.clear()
        sim.target_writes.clear()
        bridge3 = opened(sim)
        sim.writes.clear()
        bridge3.close()
        time.sleep(0.2)
        check(not sim.writes, "a close writes nothing to the hand")

        # ── motions ──────────────────────────────────────────────────────────────────────────────────────
        section("extension — the request, and what it refuses")
        sim.place([1000] * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        check(bridge.extension("vendor.other", b"status")[0] == rhal.SKR_ENOSYS, "a token that is not this driver's is ENOSYS")
        check(ext(bridge, "")[0] == rhal.SKR_EINVAL and "Motions" in ext(bridge, "")[1], "an empty request lists the motions")
        check(ext(bridge, "dance")[0] == rhal.SKR_EINVAL and "unknown motion 'dance'" in ext(bridge, "dance")[1], "an unknown motion is refused by name")
        check(ext(bridge, "move index")[0] == rhal.SKR_EINVAL and "malformed argument" in ext(bridge, "move index")[1], "an argument that is not k=v is refused")
        check(ext(bridge, "status index=1")[0] == rhal.SKR_EINVAL and "does not take index" in ext(bridge, "status index=1")[1], "status takes no arguments")
        check(ext(bridge, "clear_errors x=1")[0] == rhal.SKR_EINVAL, "clear_errors takes none")
        check(ext(bridge, "move")[0] == rhal.SKR_EINVAL and "at least one joint" in ext(bridge, "move")[1], "a move of nothing is refused")
        check(ext(bridge, "move wrist=1")[0] == rhal.SKR_EINVAL and "not a joint" in ext(bridge, "move wrist=1")[1], "a move of a joint that does not exist is refused")
        check(ext(bridge, "move index=abc")[0] == rhal.SKR_EINVAL and "not a number" in ext(bridge, "move index=abc")[1], "a move to a non-number is refused")
        check(ext(bridge, "move index=9")[0] == rhal.SKR_EINVAL, "a move outside the joint's range is refused")
        check(ext(bridge, "move index=nan")[0] == rhal.SKR_EINVAL, "a move to NaN is refused")
        check(ext(bridge, "move index=1.0 timeout=0.1")[0] == rhal.SKR_EINVAL, "a timeout under half a second is refused")
        code, body = bridge.extension(TOKEN, b"\xff\xfe")
        check(code == rhal.SKR_EINVAL and b"UTF-8" in body, "a request that is not UTF-8 is refused")
        bridge.close()
        code, body = bridge.extension(TOKEN, b"status")
        check(code == rhal.SKR_EPERM and b"not open" in body, "a motion on a driver that is not open is EPERM")

        section("extension — status")
        sim.place([1000] * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, st = ext(bridge, "status")
        check(code == rhal.SKR_OK and st["model"] == "RH56" and st["hand_id"] == 1 and st["joints"] == list(JOINTS) and st["simulator"] is True, "the status names the model, the id and the joints, and says it is a simulation")
        check(st["angle_register"] == [1000] * 6 and st["status_code"] == [2] * 6 and st["error"] == ["0x00"] * 6 and st["fault"] is None, "and the registers as the hand reports them")
        check(st["force_g"] == [0] * 6 and len(st["current_ma"]) == 6 and st["temperature_c"] == [31] * 6, "and, read on demand, the force, current and temperature")
        check(st["speed_register"] == [500] * 6 and st["force_limit_g"] == [FORCE] * 6 and st["speed"] == SPEED and st["force_limit_configured_g"] == [FORCE] * 6, "and the speed and force limits, the hand's and the operator's")
        check(st["mapping"]["linear"] == "assumed" and st["mapping"]["profile"] == "RH56" and "series user manual" in st["mapping"]["source"] and "assumed" in st["mapping"]["thumb_orientation"], "the status says which parts of the angle mapping are assumed and where the ranges come from")
        check(st["stop_latched"] is False and st["calibrating"] is False and st["link_dead"] is None, "and that there is no stop, no calibration and no dead line")
        sim.drop = True
        code, st = ext(bridge, "status")
        sim.drop = False
        check(code == rhal.SKR_OK and "read_error" in st and "force_g" not in st, "a status when the hand does not answer the extra reads still answers, and says what failed")
        bridge.close()

        section("extension — move")
        sim.place([1000] * 6)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        code, body = ext(bridge, f"move index={math.radians(100)} middle={math.radians(100)}")
        check(code == rhal.SKR_OK and body["result"] == "arrived" and set(body["joints"]) == {"index", "middle"} and abs(body["joints"]["index"]["angle_register"] - frames.angle_to_register(math.radians(100), 3, RH56)) <= bridge_module._ARRIVAL_TOLERANCE, "a move of two joints waits for both and says they arrived", str(body))
        check(sim.angles[0] == 1000 and sim.angles[1] == 1000, "and the others were not moved")
        sim.blocks[3] = 400
        code, body = ext(bridge, f"move index={math.radians(30)}")
        check(code == rhal.SKR_OK and body["result"] == "stopped_on_an_object" and body["joints"]["index"]["angle_register"] == 400 and "force control value" in body["joints"]["index"]["status"], "a finger that meets an object stops at the force limit and the move says it stopped on an object", str(body))
        check(sim.forces[3] == FORCE, "and the force at the fingertip is the limit the operator set")
        sim.blocks[3] = None
        code, body = ext(bridge, f"move index={math.radians(170)}")
        check(code == rhal.SKR_OK and body["result"] == "arrived", "and it opens again from the object")
        code, body = ext(bridge, f"move ring={math.radians(100)} timeout=0.5")
        sim.place([1000] * 6)
        slow = new_sim(time_scale=1.0)
        try:
            bridge_slow = opened(slow, speed=0.01)
            ready(bridge_slow)
            code, body = ext(bridge_slow, f"move ring={math.radians(25)} timeout=0.6")
            check(code == rhal.SKR_EIO and "no reading showed what was waited for" in body, "a move that does not arrive in its timeout is EIO and says so", str(body))
            bridge_slow.close()
        finally:
            slow.stop()
        sim.set_fault(1, error=0b1)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT)
        code, body = ext(bridge, f"move ring={math.radians(100)}")
        check(code == rhal.SKR_EPERM and "ring" in body, "a move on a hand holding a fault is refused before it is sent", str(body))
        bridge.close()
        sim.heal()

        section("extension — a hand whose status lags its target does not make a move arrive early")
        lagged = new_sim()
        lagged.echo_lag_s = 0.3
        bridge_lag = opened(lagged)
        ready(bridge_lag)
        began = time.monotonic()
        code, body = ext(bridge_lag, f"move index={math.radians(100)} timeout=5")
        took = time.monotonic() - began
        check(code == rhal.SKR_OK and body["result"] == "arrived" and abs(body["joints"]["index"]["angle_register"] - 514) <= bridge_module._ARRIVAL_TOLERANCE and took >= 0.25,
              f"the target is the new one at once and the status and angle show the old for 0.3 s: the move waits for an angle near the target, and not for a status alone ({took:.2f} s)", str(body))
        check(lagged.angles[3] == 514, "and the finger really is there")
        bridge_lag.close()
        lagged.stop()

        section("extension — a move whose finger stalls")
        sim.place([1000] * 6)
        bridge = opened(sim)
        ready(bridge)
        sim.blocks[2] = None
        threading.Timer(0.02, lambda: sim.set_fault(2, error=1)).start()
        slow = new_sim(time_scale=2.0)
        try:
            bridge2 = opened(slow, speed=0.2)
            ready(bridge2)
            threading.Timer(0.15, lambda: slow.set_fault(2, error=1)).start()
            began = time.monotonic()
            code, body = ext(bridge2, f"move middle={math.radians(25)}")
            check(code == rhal.SKR_EIO and "locked-rotor error" in body and "during the move" in body and time.monotonic() - began < 3.0, "a finger that stalls during a move ends it with the fault named, and the move is not left to wait out its timeout", str(body))
            bridge2.close()
        finally:
            slow.stop()
        bridge.close()
        sim.heal()

        section("extension — clear_errors")
        sim.place([1000] * 6)
        bridge = opened(sim)
        ready(bridge)
        sim.set_fault(0, error=0b1)
        sim.set_fault(4, error=0b100)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT)
        code, body = ext(bridge, "clear_errors")
        check(code == rhal.SKR_OK and body["result"] == "cleared" and body["before"][0] == "0x01" and body["after"] == ["0x00"] * 6 and body["remaining"] is None, "clear_errors writes 1 to CLEAR_ERROR and reports what it cleared", str(body))
        check(any(w[1] == 1004 and w[2] == b"\x01" for w in sim.writes), "the write was a single byte at 1004")
        check(write(bridge, cmd(pose([900] * 6)))[0] == rhal.SKR_OK, "and commands are accepted afterwards")
        sim.set_fault(1, error=0b10)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT)
        code, body = ext(bridge, "clear_errors")
        check(code == rhal.SKR_OK and body["result"] == "not_cleared" and "over-temperature" in body["remaining"] and "clears itself" in body["note"], "an over-temperature error is not cleared by it, and the driver says it clears itself when the actuator cools", str(body))
        check(write(bridge, cmd(pose([800] * 6)))[0] == rhal.SKR_EPERM, "and commands stay refused")
        sim.cool(1)
        check(until(lambda: write(bridge, cmd(pose([800] * 6)))[0] == rhal.SKR_OK, 3), "until the actuator has cooled")
        bridge.close()

        section("extension — calibrate_force")
        sim.place([1000] * 6)
        fast = new_sim(time_scale=SCALE)
        bridge = opened(fast)
        ready(bridge)
        code, body = ext(bridge, "calibrate_force")
        check(code == rhal.SKR_EINVAL and "confirm=unloaded" in body and "six seconds" in body, "calibrate_force refuses without the operator's word that the hand is unloaded", str(body))
        check(not any(w[1] == 1009 for w in fast.writes), "and writes nothing")
        code, body = ext(bridge, "calibrate_force confirm=yes")
        check(code == rhal.SKR_EINVAL and not any(w[1] == 1009 for w in fast.writes), "any word but `unloaded` is refused")
        code, body = ext(bridge, "calibrate_force confirm=unloaded extra=1")
        check(code == rhal.SKR_EINVAL and "does not take extra" in body, "an argument it does not take is refused")
        fast.set_fault(0, error=1)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_FAULT)
        code, body = ext(bridge, "calibrate_force confirm=unloaded")
        check(code == rhal.SKR_EPERM and "holds a fault" in body and "clear_errors" in body, "a hand that holds a fault is not calibrated", str(body))
        check(not any(w[1] == 1009 for w in fast.writes), "and nothing was written")
        bridge.close()
        fast.heal()
        fast.stop()
        slow_cal = new_sim(time_scale=1.0)
        bridge = opened(slow_cal)
        ready(bridge)
        results = []
        runner = threading.Thread(target=lambda: results.append(ext(bridge, "calibrate_force confirm=unloaded")))
        runner.start()
        time.sleep(0.5)
        check(any(w[1] == 1009 and w[2] == b"\x01" for w in slow_cal.writes), "the calibration is one byte, 1, at 1009")
        check(ext(bridge, "status")[1]["calibrating"] is True and state(bridge)[1].flags & rhal.FLAG_DEGRADED, "while it runs the status says so and the state is DEGRADED")
        code, said = write(bridge, cmd(pose([500] * 6)))
        check(code == rhal.SKR_EPERM and "calibrating" in said and "was not sent" in said, "a command while the hand moves by itself is refused", said)
        check(not slow_cal.target_writes, "and nothing reached the hand")
        runner.join(12.0)
        check(not runner.is_alive() and results and results[0][0] == rhal.SKR_OK and results[0][1]["result"] == "calibrated" and results[0][1]["seconds"] >= 7.9, "a calibration ends after the eight seconds it is given, and says it calibrated", str(results))
        check(ext(bridge, "status")[1]["calibrating"] is False and write(bridge, cmd(pose([500] * 6)))[0] == rhal.SKR_OK, "and commands are accepted again")
        slow_cal.place([1000] * 6)
        results = []
        runner = threading.Thread(target=lambda: results.append(ext(bridge, "calibrate_force confirm=unloaded")))
        runner.start()
        time.sleep(0.4)
        bridge.emergency_stop()
        runner.join(5.0)
        check(not runner.is_alive() and results and results[0][0] == rhal.SKR_EPERM and "stop was latched" in results[0][1] and "may still be moving" in results[0][1], "a stop during a calibration ends the wait, and says the hand may still be moving by itself", str(results))
        bridge.close()
        slow_cal.stop()

        # ── the line ─────────────────────────────────────────────────────────────────────────────────────
        section("the line — a half-duplex adapter that hears its own request")
        echoing = new_sim()
        echoing.echo_request = True
        bridge = opened(echoing)
        check(ready(bridge), "the request's echo before the answer is skipped, and the hand is read")
        check(write(bridge, cmd(pose([600] * 6)))[0] == rhal.SKR_OK and until(lambda: echoing.angles == [600] * 6, 3), "and commanded")
        bridge.close()
        echoing.stop()

        section("the line — an answer in two pieces, and a corrupt one")
        split = new_sim()
        split.split_gap_s = 0.01
        bridge = opened(split)
        check(ready(bridge), "an answer that arrives in two pieces is read")
        bridge.close()
        split.stop()
        garbled = new_sim()
        garbled.garble = True
        code, text, bridge = opens_with(garbled, open_timeout_ms=500)
        check(code == rhal.SKR_EIO and "checksum" in text, "a hand whose answers have a wrong checksum is not opened, and the driver says why", text)
        garbled.stop()
        short = new_sim()
        short.truncate = 3
        code, text, bridge = opens_with(short, open_timeout_ms=500)
        check(code == rhal.SKR_EIO and ("stopped sending" in text or "did not finish" in text), "a hand whose answers stop short is not opened", text)
        short.stop()
        liar = new_sim()
        liar.reply_hand = 7
        code, text, bridge = opens_with(liar, open_timeout_ms=500)
        check(code == rhal.SKR_EIO and "hand 7" in text, "a hand that answers as another is not opened, and the driver says which", text)
        liar.stop()
        refusing = new_sim()
        refusing.reject_writes = 1
        code, text, bridge = opens_with(refusing, open_timeout_ms=500)
        check(code == rhal.SKR_EIO and "refused the write" in text, "a hand that refuses the first write at open is not opened", text)
        refusing.stop()

        section("the line — a write the hand refuses is EIO, and nothing is remembered as written")
        sim.place([1000] * 6)
        bridge = opened(sim)
        ready(bridge)
        sim.reject_writes = 1
        code, said = write(bridge, cmd(pose([600] * 6)))
        check(code == rhal.SKR_EIO and "refused the write" in said, "a refused write is EIO and says the hand refused", said)
        check(ext(bridge, "status")[1]["image"] == [1000] * 6, "and the image is as it was")
        code, said = write(bridge, cmd(pose([600] * 6)))
        check(code == rhal.SKR_OK, "and the next is taken")
        bridge.close()

        section("the line — a slow line is EIO within the command's budget")
        sim.place([1000] * 6)
        bridge = opened(sim, timeout_ms=80)
        ready(bridge)
        sim.delay_s = 0.5
        began = time.monotonic()
        code, said = write(bridge, cmd(pose([600] * 6)))
        took = time.monotonic() - began
        sim.delay_s = 0.0
        check(code == rhal.SKR_EIO and took < 0.5, f"a hand that answers in half a second is EIO in {took:.2f} s, not after waiting for it", said)
        bridge.close()

        section("the line — a hand that lost its power behind the driver gets the operator's speed and force back")
        sim.place([1000] * 6)
        bridge = opened(sim, stale_ms=300)
        ready(bridge)
        sim.drop = True
        until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 3)
        sim.set_registers(frames.ADDR_SPEED_SET, [100] * 6)  # noqa: SLF001
        sim.set_registers(frames.ADDR_FORCE_SET, [50] * 6)  # noqa: SLF001
        sim.set_registers(frames.ADDR_ANGLE_SET, [700] * 6)  # noqa: SLF001
        sim.place([700] * 6)
        sim.drop = False
        check(until(lambda: sim.speeds == [500] * 6 and sim.force_limits == [FORCE] * 6, 3), "the speed and force limits are written again")
        check(until(lambda: ext(bridge, "status")[1]["image"] == [700] * 6, 3), "and the image is made again from the hand's own record, not from what the driver last said")
        bridge.close()

        section("a hand behind a TCP gateway")
        gateway = new_sim(tcp=True)
        bridge = opened(gateway)
        check(ready(bridge), "a hand behind a raw RS-485 gateway is read")
        check(write(bridge, cmd(pose([600] * 6)))[0] == rhal.SKR_OK and until(lambda: gateway.angles == [600] * 6, 3), "and commanded")
        gateway.hang_up()
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 5), "a gateway that drops the connection is reconnected and the hand is read again")
        check(until(lambda: write(bridge, cmd(pose([700] * 6)))[0] == rhal.SKR_OK, 5) and until(lambda: gateway.angles == [700] * 6, 3), "and commanded again (a command in the moment before the line is back is EIO, and the next one is taken)")
        bridge.close()
        gateway.stop()

        section("a close does not stop the hand")
        slow = new_sim(time_scale=1.0)
        try:
            bridge = opened(slow, speed=0.05)
            ready(bridge)
            write(bridge, cmd(pose([0] * 6)))
            time.sleep(0.3)
            bridge.close()
            at_close = slow.angles
            time.sleep(0.5)
            check(slow.angles[0] < at_close[0], f"the fingers go on closing after a close, as a hand that is told nothing does (angle {at_close[0]}, then {slow.angles[0]})")
        finally:
            slow.stop()
    finally:
        sim.stop()

    triage()
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    try:
        code = main()
    except BaseException:
        # Several checks capture what the driver says with `redirect_stderr`, from threads as well as the main one, and a redirect that is left half undone leaves `sys.stderr` pointing at a buffer:
        # the traceback of a failure would then be written to nowhere, and the run would end with an exit status and no word.
        sys.stderr = sys.__stderr__
        raise
    raise SystemExit(code)

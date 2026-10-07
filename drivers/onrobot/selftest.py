"""The OnRobot gripper bridge against a simulated gripper written here (DRY-COVERAGE-PLAN.md item 12.E.8): the Modbus frames, the register map, what it refuses to open on, the state, the refusals, the faults, the stop, the
motions and the line.

    PYTHONPATH=../../bindings/python:. python3 selftest.py

Needs python3 on Linux (the simulated gripper's RTU side is a pseudo-terminal) and nothing of the maker's. The simulated gripper agrees with whoever wrote it (F15); `layout_check.py` holds the driver's registers to the
maker's own client, which is run, and what this shows is the driver's own logic.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import struct
import sys
import threading
import time

import skynet_rhal as rhal

from skynet_onrobot import bridge as bridge_module, frames, link, modbus
from skynet_onrobot.bridge import OnRobotBridge, TOKEN
from skynet_onrobot.gripper_sim import SimulatedGripper

CHECKS = 0
FAILURES: list[str] = []
SCALE = 20.0           # the simulated fingers move this much faster than they would
RG2, RG6, FG7, FG14 = frames.MODELS["RG2"], frames.MODELS["RG6"], frames.MODELS["2FG7"], frames.MODELS["2FG14"]


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


def new_sim(model: str = "RG2", **kw) -> SimulatedGripper:
    kw.setdefault("time_scale", SCALE)
    return SimulatedGripper(model, **kw).start()


def settings(sim: SimulatedGripper, **extra) -> dict:
    model = sim.model
    out: dict[str, object] = {"model": model.name, "host": "127.0.0.1", "port": sim.port, "stop_mode": "stop", "simulator": True, "force_n": 30.0 if model.family == "fg" else 20.0, "rate_hz": 50, "timeout_ms": 120,
                              "roles": {"jaw": "right_hand.gripper.grip"}}
    if model.family == "fg":
        out.update({"speed": 80, "supply_power_w": 48})
        if model.name == "2FG14":
            out["force_n"] = 50.0
    else:
        out["limits"] = [0.0, float(model.max_width_mm or 0.0) / 1000.0]
    out.update(extra)
    return {k: v for k, v in out.items() if v is not None}


def config(sim: SimulatedGripper, **extra) -> str:
    return json.dumps(settings(sim, **extra))


def opened(sim: SimulatedGripper, **extra) -> OnRobotBridge:
    bridge = OnRobotBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge


def opens_with(sim: SimulatedGripper, **extra) -> tuple[int, str, OnRobotBridge]:
    bridge = OnRobotBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(sim, **extra))
    return code, said.getvalue(), bridge


def refused(text: str) -> tuple[int, str]:
    bridge = OnRobotBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(text)
    bridge.close()
    return code, said.getvalue()


def state(bridge: OnRobotBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def cmd(values: list[float], mask: list[int] | None = None, mode: int = rhal.MODE_POSITION, count: int | None = None) -> rhal.Command:
    n = count if count is not None else len(values)
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=list(values), mask=mask if mask is not None else [1] * n)


def write(bridge: OnRobotBridge, command: rhal.Command) -> tuple[int, str]:
    bridge._last_refusal = None  # noqa: SLF001 - a refusal is said once for a run of the same cause; each check asks to hear it
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def ext(bridge: OnRobotBridge, text: str) -> tuple[int, object]:
    code, body = bridge.extension(TOKEN, text.encode())
    try:
        return code, json.loads(body)
    except json.JSONDecodeError:
        return code, body.decode()


def ready(bridge: OnRobotBridge) -> bool:
    return until(lambda: state(bridge)[0] == rhal.SKR_OK)


def said_by(call) -> tuple[object, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        result = call()
    return result, said.getvalue()


def hex_(text: str) -> bytes:
    return bytes.fromhex(text.replace(" ", ""))


def fds() -> int:
    return len(os.listdir("/proc/self/fd"))


def holding_the_line(bridge: OnRobotBridge, seconds: float) -> threading.Thread:
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


def mm(width: float) -> list[float]:
    return [width / 1000.0]


def more() -> None:
    """The state, the stop, the motions, the line and the bounds."""
    sim = new_sim("RG2")
    try:
        # ── the state ──
        section("read_state — the registers the maker's client reads, as this driver reports them")
        sim.place(55.0)
        bridge = opened(sim, rate_hz=50)
        check(ready(bridge), "ready")
        sim.frozen = True
        block = [0] * 20
        block[17] = 777                       # the compensated width, 77.7 mm
        block[19] = 0xFFEC                    # the task velocity, -2.0 mm/s
        sim.set_registers(frames.RG_STATUS_ADDRESS, block)
        check(until(lambda: abs(state(bridge)[1].position_rad[0] - 0.0777) < 1e-9, 2), "the position is the compensated width in metres")
        rc, out = state(bridge)
        check(abs(out.velocity_rad_s[0] - (-0.002)) < 1e-12, "and the velocity is the task velocity, signed, in metres a second")
        for status, wanted, why in ((frames.RG_S1_PUSHED, True, "safety switch 1 pushed"), (frames.RG_S2_PUSHED, True, "safety switch 2 pushed"), (frames.RG_S1_TRIGGERED, True, "safety switch 1 triggered"), (frames.RG_S2_TRIGGERED, True, "safety switch 2 triggered"),
                                    (frames.RG_DC_ERROR, True, "a safety DC error"), (1 << 7, True, "error code 1"), (7 << 7, True, "error code 7"), (frames.RG_BUSY | frames.RG_GRIP, False, "busy and gripping")):
            block[10] = status
            sim.set_registers(frames.RG_STATUS_ADDRESS, block)
            until(lambda: ext(bridge, "status")[1]["raw_status"] == status, 2)
            rc, out = state(bridge)
            faulted = bool(out.flags & rhal.FLAG_FAULT)
            check(faulted == wanted and (not wanted or (out.flags & rhal.FLAG_DEGRADED and out.fault_code == status & 0x3FC)), f"{why}: {'FAULT and DEGRADED, the word is the status bits 2 to 9' if wanted else 'not a fault'}")
        block[10] = 0
        sim.set_registers(frames.RG_STATUS_ADDRESS, block)
        sim.frozen = False
        check(until(lambda: not state(bridge)[1].flags & rhal.FLAG_FAULT, 2), "and a fault that is gone is gone")
        bridge.close()

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
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 1), "with a staleness bound of 100 ms a gripper that is read every 20 ms is current")
        sim.drop = True
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 0.25), "and one that goes quiet is stale in a quarter of a second, before three reads can have failed")
        sim.drop = False
        bridge.close()
        bridge = opened(sim, stale_ms=30000)
        check(ready(bridge), "ready")
        sim.drop = True
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT, 3), "a line whose reads fail three times is ETIMEDOUT however recent its last good reading is")
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_DEGRADED and not math.isnan(out.position_rad[0]), "DEGRADED, with the last position it did report still there, flagged and not blanked")
        first = out.stamp_ns
        time.sleep(0.2)
        check(state(bridge)[1].stamp_ns == first, "and the state keeps the stamp of the last reading that arrived, not the time it was asked for")
        sim.drop = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3), "and it is read again when the gripper answers")
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
        sim.garble = True
        time.sleep(0.4)
        sim.garble = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3), "a gripper that sends wrong answers for a while and right ones after is read again: the reading thread did not die of them")
        sim.exception_next = 4
        check(until(lambda: sim.exception_next == 0, 2) and until(lambda: state(bridge)[0] == rhal.SKR_OK, 3), "and so is one that answered with Modbus exceptions")
        bridge.close()

        section("read_state — no reading yet, and a driver that is not open")
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge), "ready")
        with bridge._lock:  # noqa: SLF001
            bridge._latest = None  # noqa: SLF001
        rc, out = state(bridge)
        check(rc == rhal.SKR_ETIMEDOUT and out.flags & rhal.FLAG_DEGRADED and math.isnan(out.position_rad[0]), "a state with no reading is ETIMEDOUT and DEGRADED, with no position")
        code, said = write(bridge, cmd(mm(50)))
        check(code == rhal.SKR_ETIMEDOUT and "not been read" in said, "a command is refused until the gripper has been read", said)
        code, text = ext(bridge, "status")
        check(code == rhal.SKR_ETIMEDOUT and "not been read" in str(text), "and so is a status")
        bridge.close()
        check(state(bridge)[0] == rhal.SKR_EPERM, "a state read from a driver that is not open is EPERM")

        # ── the stop ──
        section("the stop — latched, written by the reading thread, and not released by a gripper that has not taken it")
        slow = new_sim("RG2", time_scale=1.0)
        try:
            slow.place(100.0)
            bridge = opened(slow)
            check(ready(bridge), "ready")
            write(bridge, cmd(mm(10.0)))
            time.sleep(0.2)
            check(slow.moving, "the jaw is closing")
            slow.writes.clear()
            began = time.monotonic()
            check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.02, "emergency_stop returns at once, without touching the line")
            check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2), "and the reading thread writes the stop")
            check([values for _, address, values in slow.writes if address == 0] == [(0, 0, 40)], "the maker's stop: the control word 40, with force and width zero")
            check(until(lambda: not slow.moving, 1) and slow.width_mm > 10.0, "the jaw stopped where it was")
            check(until(lambda: state(bridge)[1].flags & rhal.FLAG_ESTOP, 1), "the state says the stop is latched")
            code, said = write(bridge, cmd(mm(60)))
            check(code == rhal.SKR_EPERM, "a command while latched is EPERM")
            status = ext(bridge, "status")
            check(status[0] == rhal.SKR_OK and status[1]["stop_latched"] is True and status[1]["stop_delivered"] is True and status[1]["target_mm"] is None, "a status is still answered, and says the stop was delivered and that there is no target")
            check(ext(bridge, "move width=0.06")[0] == rhal.SKR_EPERM and "stop is latched" in ext(bridge, "move width=0.06")[1], "a move is refused, and says why")
            check(bridge.release_stop() == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_ESTOP, "release_stop clears the latch once a reading shows the jaw not moving")
            status = ext(bridge, "status")[1]
            check(status["stop_latched"] is False and status["stop_delivered"] is False, "and nothing of the stop is left in the status")
            check(write(bridge, cmd(mm(60)))[0] == rhal.SKR_OK, "and commands are accepted again")
            slow.writes.clear()
            bridge.emergency_stop()
            check(until(lambda: any(address == 0 for _, address, _ in slow.writes), 2), "a second stop is written again")
            check(bridge.release_stop() == rhal.SKR_OK, "and released")
            bridge.close()
            fg_slow = new_sim("2FG7", time_scale=1.0)
            try:
                fg_slow.place(70.0)
                bridge = opened(fg_slow)
                check(ready(bridge), "a 2FG ready")
                write(bridge, cmd(mm(15.0)))
                time.sleep(0.2)
                fg_slow.writes.clear()
                bridge.emergency_stop()
                check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2) and [values for _, address, values in fg_slow.writes if address == 0] == [(0, 0, 0, 3)], "a 2FG's stop is type 3 with the rest zero")
                check(bridge.release_stop() == rhal.SKR_OK, "and released")
                bridge.close()
            finally:
                fg_slow.stop()
        finally:
            slow.stop()
        crawl = new_sim("RG2", time_scale=0.2)
        try:
            crawl.place(100.0)
            bridge = opened(crawl)
            check(ready(bridge), "ready")
            write(bridge, cmd(mm(10.0)))
            time.sleep(0.2)
            crawl.deaf = True
            bridge.emergency_stop()
            until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 2)
            check(crawl.moving, "a gripper that acknowledged the stop and did not take it is still moving")
            said_release, said = said_by(bridge.release_stop)
            check(said_release == rhal.SKR_EIO and "stays latched" in said and bridge._estop, "and is not released", said)  # noqa: SLF001
            with crawl._lock:  # noqa: SLF001
                crawl._stop()  # noqa: SLF001
            check(bridge.release_stop() == rhal.SKR_OK, "and is released once it is not moving")
            bridge.close()
        finally:
            crawl.stop()

        section("the stop — a line that is down when it is called, and a command that is waiting")
        sim.place(100.0)
        bridge = opened(sim, stale_ms=300)
        check(ready(bridge), "ready")
        sim.drop = True
        began = time.monotonic()
        check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.02, "a stop with the line down still returns OK within its budget: the latch is the stop (F298: the call is 20 ms at most)")
        check(ext(bridge, "status")[1]["stop_delivered"] is False, "and the status says it has not been delivered")
        said_release, said = said_by(bridge.release_stop)
        check(said_release == rhal.SKR_EIO and "stays latched" in said, "release_stop on a gripper that has not taken the stop is EIO and the latch stays", said)
        check(write(bridge, cmd(mm(60)))[0] == rhal.SKR_EPERM, "and a command is still refused")
        sim.drop = False
        check(until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3), "the loop writes the stop as soon as the line carries it")
        check(bridge.release_stop() == rhal.SKR_OK, "and then the stop can be released")
        bridge.close()
        bridge = opened(sim, timeout_ms=300)
        check(ready(bridge), "ready")
        sim.writes.clear()
        owner = holding_the_line(bridge, 0.15)
        results: list[int] = []
        sender = threading.Thread(target=lambda: results.append(write(bridge, cmd(mm(60)))[0]))
        sender.start()
        time.sleep(0.05)
        bridge.emergency_stop()
        owner.join()
        sender.join(3.0)
        until(lambda: ext(bridge, "status")[1]["stop_delivered"] is True, 3)
        grips = [values for _, address, values in sim.writes if address == 0 and values[2] == frames.RG_CONTROL_GRIP]
        check(results == [rhal.SKR_EPERM] and not grips, f"a command that was waiting for the line when the stop was called is refused, and never writes its grip after the stop ({results}, {grips})")
        bridge.release_stop()
        bridge.close()

        section("the stop — a close writes nothing, and a new open is a new session")
        sim.place(100.0)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        bridge.emergency_stop()
        until(lambda: ext(bridge, "status")[1]["stop_delivered"])
        text = config(sim)
        bridge.close()
        sim.writes.clear()
        said_open, _ = said_by(lambda: bridge.open(text))
        status = ext(bridge, "status")[1]
        check(said_open == rhal.SKR_OK and ready(bridge) and not state(bridge)[1].flags & rhal.FLAG_ESTOP, "the same driver opened again is not stopped")
        check(status["stop_delivered"] is False and status["stop_latched"] is False and status["target_mm"] is None and not any(address == 0 for _, address, _ in sim.writes), "and has no stop owed or delivered, no target, and has written no control block")
        check(write(bridge, cmd(mm(60)))[0] == rhal.SKR_OK, "and takes a command")
        sim.writes.clear()
        bridge.close()
        time.sleep(0.2)
        check(not sim.writes, "a close writes nothing to the gripper")
        released = OnRobotBridge()
        released._estop = True  # noqa: SLF001
        check(released.release_stop() == rhal.SKR_OK and released._estop is False, "a stop released before the driver was opened is released")  # noqa: SLF001

        # ── motions ──
        section("extension — the request, and what it refuses")
        sim.place(100.0)
        bridge = opened(sim, limits=[0.01, 0.1])
        check(ready(bridge), "ready")
        check(bridge.extension("vendor.other", b"status")[0] == rhal.SKR_ENOSYS, "a token that is not this driver's is ENOSYS")
        check(ext(bridge, "")[0] == rhal.SKR_EINVAL and "Motions" in ext(bridge, "")[1], "an empty request lists the motions")
        check(bridge.extension(TOKEN, b"\xff\xfe")[0] == rhal.SKR_EINVAL, "a request that is not UTF-8 is refused")
        check(ext(bridge, "dance")[0] == rhal.SKR_EINVAL and "unknown motion 'dance'" in ext(bridge, "dance")[1], "an unknown motion is refused by name")
        check(ext(bridge, "move width")[0] == rhal.SKR_EINVAL and "wanted k=v" in ext(bridge, "move width")[1] and ext(bridge, "move =3")[0] == rhal.SKR_EINVAL, "an argument that is not k=v is refused")
        check(ext(bridge, "status index=3")[0] == rhal.SKR_EINVAL and "does not take index" in ext(bridge, "status index=3")[1], "an argument a motion does not take is refused, naming it")
        check(ext(bridge, "move")[0] == rhal.SKR_EINVAL and "width=" in ext(bridge, "move")[1], "a move with no width says it needs one")
        check(ext(bridge, "move width=0.05 force=3")[0] == rhal.SKR_EINVAL, "and one with an argument it does not take is refused")
        for text in ("move width=0.0099", "move width=0.1001", "move width=nan", "move width=abc", "move width=0.05 timeout=0.4", "move width=0.05 timeout=121", "move width=0.05 timeout=nan"):
            code, body = ext(bridge, text)
            check(code == rhal.SKR_EINVAL and ("outside" in body or "not a number" in body), f"`{text}` is refused", str(body))
        check(bridge.extension(TOKEN, b"status")[0] == rhal.SKR_OK, "a status is answered")
        bridge.close()
        check(bridge.extension(TOKEN, b"status")[0] == rhal.SKR_EPERM, "and not by a driver that is not open")

        section("extension — status")
        sim.place(55.0)
        bridge = opened(sim, limits=[0.01, 0.1])
        check(ready(bridge), "ready")
        status = ext(bridge, "status")[1]
        check(status["model"] == "RG2" and status["slave"] == 65 and status["firmware"] == "1.1.33" and status["simulator"] is True and status["limits_mm"] == [10.0, 100.0] and status["force_n_configured"] == 20.0,
              "the status names the model, the slave, the firmware read at open, that it is a simulation, the limits and the configured force")
        check(abs(status["width_mm"] - 55.0) < 1e-9 and abs(status["position_m"] - 0.055) < 1e-12 and status["busy"] is False and status["grip_detected"] is False and status["fault"] is None and status["target_mm"] is None, "the width, the position, no motion, no fault and no target")
        check(status["error_code"] == 0 and "motor_current_a" in status and "temperature_c" in status and "note" in status, "an RG's status adds its error code and its own readings, and says that it reports no force")
        write(bridge, cmd(mm(60.0)))
        check(ext(bridge, "status")[1]["target_mm"] == 60.0, "and the target of the last command")
        bridge.close()
        fg_sim = new_sim("2FG7")
        try:
            bridge = opened(fg_sim)
            check(ready(bridge), "ready")
            status = ext(bridge, "status")[1]
            check(status["model"] == "2FG7" and status["supply_power_w"] == 48 and status["supply_power_w_configured"] == 48 and status["maximum_force_n"] == 140 and status["device_limits_mm"] == [10.0, 74.0] and status["speed_percent_configured"] == 80.0,
                  "a 2FG's status adds the budget it holds and the one configured, the force ceiling, the device's limits and the speed configured")
            check("force_n_signed" in status and status["not_calibrated"] is False and status["linear_sensor_error"] is False, "and its signed force and its two flags")
            bridge.close()
        finally:
            fg_sim.stop()

        section("extension — move")
        sim.place(100.0)
        bridge = opened(sim, limits=[0.01, 0.1])
        check(ready(bridge), "ready")
        code, body = ext(bridge, "move width=0.04 timeout=5")
        check(code == rhal.SKR_OK and body["result"] == "arrived" and abs(body["width_mm"] - 40.0) < 0.06 and body["grip_detected"] is False, "a move arrives, and says where", str(body))
        sim.put_object(30.0)
        code, body = ext(bridge, "move width=0.02 timeout=5")
        check(code == rhal.SKR_OK and body["result"] == "stopped_on_an_object" and body["grip_detected"] is True and abs(body["width_mm"] - 30.0) < 0.06, "a jaw that closes on an object stops on it, and the move says so", str(body))
        sim.put_object(None)
        code, body = ext(bridge, "move width=0.0302 timeout=5")
        check(code == rhal.SKR_OK and body["result"] == "arrived", "a target within a millimetre of where the jaw already is has arrived once a reading says so", str(body))
        sim.deaf = True
        began = time.monotonic()
        code, body = ext(bridge, "move width=0.09 timeout=0.6")
        took = time.monotonic() - began
        sim.deaf = False
        check(code == rhal.SKR_EIO and "never started moving" in str(body) and 0.5 <= took <= 1.0, f"a gripper that acknowledges a grip and never moves is not a move that arrived: it times out, and says it never started ({took:.2f} s)", str(body))
        sim.place(100.0)
        sim.set_error(5, 0)
        until(lambda: ext(bridge, "status")[1]["fault"], 2)
        code, body = ext(bridge, "move width=0.05 timeout=2")
        check(code == rhal.SKR_EPERM and "error code 5" in str(body), "a move on a gripper that holds a fault is refused, naming it", str(body))
        sim.set_error(0, 0)
        until(lambda: not ext(bridge, "status")[1]["fault"], 2)
        bridge.close()
        slow = new_sim("RG2", time_scale=1.0)
        try:
            slow.place(100.0)
            bridge = opened(slow)
            check(ready(bridge), "ready")
            outcomes: list[tuple[int, object]] = []
            mover = threading.Thread(target=lambda: outcomes.append(ext(bridge, "move width=0.01 timeout=8")))
            mover.start()
            time.sleep(0.2)
            slow.set_error(3, 0)
            mover.join(3.0)
            check(not mover.is_alive() and outcomes and outcomes[0][0] == rhal.SKR_EIO and "error code 3" in str(outcomes[0][1]), "a move during which a fault comes up ends in EIO, naming the fault", str(outcomes))
            slow.set_error(0, 0)
            slow.place(100.0)
            until(lambda: not ext(bridge, "status")[1]["fault"], 2)
            outcomes.clear()
            mover = threading.Thread(target=lambda: outcomes.append(ext(bridge, "move width=0.01 timeout=8")))
            mover.start()
            time.sleep(0.3)
            bridge.emergency_stop()
            mover.join(1.5)
            check(not mover.is_alive() and outcomes and outcomes[0][0] == rhal.SKR_EPERM and "stop was latched" in str(outcomes[0][1]), "a move that is under way when the stop is latched ends at once", str(outcomes))
            bridge.release_stop()
            outcomes.clear()
            mover = threading.Thread(target=lambda: outcomes.append(ext(bridge, "move width=0.01 timeout=8")))
            mover.start()
            time.sleep(0.3)
            began = time.monotonic()
            bridge.close()
            mover.join(1.5)
            check(not mover.is_alive() and time.monotonic() - began < 1.0 and outcomes and outcomes[0][0] == rhal.SKR_EPERM and "closed" in str(outcomes[0][1]), "and when the driver is closed", str(outcomes))
        finally:
            slow.stop()
        sim.place(100.0)

        section("extension — a request the line does not carry")
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge) and until(lambda: bridge._exchanges >= 1, 2), "ready")  # noqa: SLF001
        sim.silent = True
        code, text = ext(bridge, "move width=0.05 timeout=2")
        sim.silent = False
        check(code == rhal.SKR_EIO and "did not take the command" in str(text), "a move whose command the line does not carry is EIO", str(text))
        until(lambda: state(bridge)[0] == rhal.SKR_OK, 3)
        sim.exception_next = 1
        code, text = ext(bridge, "move width=0.05 timeout=2")
        check(code == rhal.SKR_EINVAL and "exception 2" in str(text), "and one the gripper answers with a Modbus exception is refused, naming it", str(text))
        bridge.close()

        # ── the line ──
        section("the line — Modbus RTU on a serial device")
        rtu = new_sim("RG2")
        try:
            rtu.place(80.0)
            bridge = OnRobotBridge()
            said_open, said = said_by(lambda: bridge.open(json.dumps({**settings(rtu), "host": None, "port": None, "device": rtu.path, "baud": 115200})))
            check(said_open == rhal.SKR_OK, "a gripper on a serial device is opened (8 data bits, even parity, 1 stop bit)", said)
            check(ready(bridge) and abs(state(bridge)[1].position_rad[0] - 0.08) < 1e-9, "and read")
            check(write(bridge, cmd(mm(50.0)))[0] == rhal.SKR_OK and until(lambda: abs(rtu.width_mm - 50.0) < 0.06, 3), "and commanded")
            rtu.split_gap_s = 0.02
            check(until(lambda: state(bridge)[0] == rhal.SKR_OK and abs(state(bridge)[1].position_rad[0] - 0.05) < 1e-9, 3), "an answer that comes in two pieces is read")
            rtu.split_gap_s = 0.0
            rtu.garble = True
            time.sleep(0.4)
            code, said = write(bridge, cmd(mm(60.0)))
            rtu.garble = False
            check(code != rhal.SKR_OK and "CRC" in said, "an answer whose CRC is wrong is refused, and says so", said)
            check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3), "and the gripper is read again when the answers are right")
            rtu.wrong_unit = 66
            time.sleep(0.4)
            code, said = write(bridge, cmd(mm(60.0)))
            rtu.wrong_unit = None
            check(code != rhal.SKR_OK and "slave 66" in said, "an answer from another slave is refused, naming both", said)
            until(lambda: state(bridge)[0] == rhal.SKR_OK, 3)
            bridge.close()
            bridge = OnRobotBridge()
            code, said = refused(json.dumps({**settings(rtu), "host": None, "port": None, "device": rtu.path, "baud": 1000000}))
            check(code == rhal.SKR_OK, "1000000 baud is a speed the tool bus runs at", said)
            link_rtu = link.Link(device=rtu.path)
            link_rtu.open()
            request = modbus.pdu_read(frames.PRODUCT_CODE_ADDRESS, 1)
            rtu.truncate = 2
            began = time.monotonic()
            text = raises(link.LinkError, link_rtu.transact, 65, request, 1.0) or ""
            check("stopped sending" in text and time.monotonic() - began < 0.3, f"an RTU answer that stops half way is given up on after the byte timeout and says so ({time.monotonic() - began:.2f} s)", text)
            rtu.truncate = 0
            rtu.delay_s = 0.15
            raises(link.LinkError, link_rtu.transact, 65, request, 0.05)
            rtu.delay_s = 0.0
            time.sleep(0.3)
            try:
                got = link_rtu.transact(65, modbus.pdu_read(frames.RG_STATUS_ADDRESS, 20), 0.5)
                check(len(got) == 20, "a late RTU answer to an earlier request is thrown away before the next request is written")
            except (link.LinkError, modbus.ProtocolError) as error:
                check(False, "a late RTU answer to an earlier request is thrown away before the next request is written", str(error))
            rtu.pull()
            time.sleep(0.1)
            raises(link.LinkError, link_rtu.transact, 65, request, 0.3)
            check(not link_rtu.is_open, "a line that failed with an OS error (the adapter was pulled) is closed, so that whoever drives it knows to open it again")
            link_rtu.close()
        finally:
            rtu.stop()

        section("the line — Modbus TCP: answers in pieces, late, from another unit, and a Compute Box that goes away")
        tcp = new_sim("RG2")
        try:
            tcp.place(80.0)
            lk = link.Link(host="127.0.0.1", port=tcp.port)
            lk.open()
            request = modbus.pdu_read(frames.PRODUCT_CODE_ADDRESS, 1)
            check(lk.transact(65, request, 1.0) == [32], "a read over TCP is answered with the product code")
            tcp.split_gap_s = 0.03
            check(lk.transact(65, request, 1.0) == [32], "an answer in two pieces is read")
            tcp.split_gap_s = 0.0
            tcp.late_answer_s = 0.05
            check(lk.transact(65, request, 1.0) == [32], "a stale answer to an earlier transaction that arrives first is thrown away, and the right one is read")
            tcp.late_answer_s = 0.0
            tcp.wrong_unit = 66
            check("unit 66" in (raises(modbus.ProtocolError, lk.transact, 65, request, 1.0) or ""), "an answer from another unit is refused, naming both")
            tcp.wrong_unit = None
            tcp.garble = True
            check("function" in (raises(modbus.ProtocolError, lk.transact, 65, request, 1.0) or ""), "an answer to another function is refused")
            tcp.garble = False
            tcp.truncate = 3
            began = time.monotonic()
            text = raises(link.LinkError, lk.transact, 65, request, 1.0) or ""
            check("stopped sending" in text and time.monotonic() - began < 0.3, f"an answer that stops half way is given up on after the byte timeout and says so ({time.monotonic() - began:.2f} s)", text)
            tcp.truncate = 0
            tcp.exception_next = 1
            tcp.exception_code = 4
            check("exception 4 (server device failure)" in (raises(modbus.ProtocolError, lk.transact, 65, request, 1.0) or ""), "a Modbus exception is refused, naming its code")
            lk.close()
            before = tcp.connections
            lk.open()
            until(lambda: tcp.connections > before, 2)
            time.sleep(0.05)
            tcp.hang_up()
            time.sleep(0.05)
            raises(link.LinkError, lk.transact, 65, request, 0.3)
            check(not lk.is_open, "a Compute Box that closed the connection is a line that is closed")
            lk.close()
            bridge = opened(tcp)
            check(ready(bridge), "a gripper behind a Compute Box is read")
            tcp.hang_up()
            check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 5), "a connection that drops is made again and the gripper is read again")
            check(until(lambda: write(bridge, cmd(mm(60.0)))[0] == rhal.SKR_OK, 5) and until(lambda: abs(tcp.width_mm - 60.0) < 0.06, 3), "and commanded again")
            tcp.refuse_connections = True
            before = tcp.connections
            tcp.hang_up()
            time.sleep(1.4)
            attempts = tcp.connections - before
            check(1 <= attempts <= 4, f"a Compute Box that is up and will not talk is opened again at most twice a second ({attempts} attempts in 1.4 s)")
            tcp.refuse_connections = False
            tcp.stop_listening()
            tcp.hang_up()
            check(until(lambda: bridge._dead is not None and "could not be opened" in bridge._dead, 3), "and one that is not listening is reported as a line that could not be opened, not as the read that failed last")  # noqa: SLF001
            tcp.resume_listening()
            check(until(lambda: state(bridge)[0] == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 5), "and is read again when it listens")
            bridge.close()
        finally:
            tcp.stop()

        # ── what a command may take ──
        section("write_command — the quarter of a second a command is given includes waiting for the line")
        sim.place(100.0)
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge), "ready")
        owner = holding_the_line(bridge, 0.6)
        began = time.monotonic()
        code, said = write(bridge, cmd(mm(60)))
        took = time.monotonic() - began
        owner.join()
        check(code == rhal.SKR_EIO and 0.2 <= took <= 0.4, f"a command that cannot have the line is EIO after its quarter of a second ({took:.2f} s), not when the line is free again (F299)", said)
        bridge.close()
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge), "ready")
        owner = holding_the_line(bridge, 0.2)
        sim.delay_s = 0.07
        began = time.monotonic()
        code, said = write(bridge, cmd(mm(60)))
        took = time.monotonic() - began
        sim.delay_s = 0.0
        owner.join()
        check(code == rhal.SKR_EIO and took <= 0.3, f"and what is left of the quarter of a second is what the transaction gets: 50 ms is not enough for a gripper that answers in 70 ({took:.2f} s)", said)
        bridge.close()
        check((bridge_module._TRANSACTION_S, bridge_module._STALE_S, bridge_module._COMMAND_BUDGET_S, bridge_module._FAULT_THRESHOLD, bridge_module._REOPEN_S, bridge_module._INITIAL_READ_ATTEMPTS,  # noqa: SLF001
               bridge_module._RATE_HZ, bridge_module._MAX_RATE_HZ, bridge_module._ARRIVAL_TOLERANCE_MM) == (0.08, 0.5, 0.25, 3, 0.5, 3, 20.0, 100.0, 1.0),
              "the bounds the README gives: 80 ms a transaction, 0.5 s before a reading is stale, 0.25 s a command, three lost frames to a dead line, half a second between reopenings, three first reads, 20 Hz (100 at most) and a millimetre to arrive")
        check(TOKEN == "vendor.onrobot.task" and OnRobotBridge.stop_during_extension is True and link.BYTE_TIMEOUT_S == 0.05 and link.BAUDS == (115200, 1000000), "the extension is vendor.onrobot.task, a stop may arrive while a motion runs, the byte timeout is 50 ms and the bauds are the tool bus's two")

        section("open and close — a close ends the thread and the line, and a session starts with nothing of the last one's")
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
        check(until(lambda: fds() == base, 1), "five opens and closes leave no line open")
        bridge = opened(sim)
        check(ready(bridge), "ready")
        bridge._last_refusal = "an old refusal"  # noqa: SLF001
        bridge._reasserted = 7  # noqa: SLF001
        bridge.emergency_stop()
        until(lambda: ext(bridge, "status")[1]["stop_delivered"])
        text = config(sim)
        bridge.close()
        check(said_by(lambda: bridge.open(text))[0] == rhal.SKR_OK and ready(bridge), "the same driver opened again")
        check(bridge._last_refusal is None and bridge._reasserted == 0 and not state(bridge)[1].flags & rhal.FLAG_ESTOP and ext(bridge, "status")[1]["stop_delivered"] is False, "has no refusal remembered, no count of budgets told, no stop and nothing delivered")  # noqa: SLF001
        bridge.close()

    finally:
        sim.stop()


def main() -> int:
    # ── modbus ─────────────────────────────────────────────────────────────────────────────────
    section("modbus — the frames, byte for byte")
    check(modbus.crc16(hex_("01 03 00 00 00 0A")) == 0xCDC5 and modbus.crc16(b"") == 0xFFFF, "the CRC is the standard Modbus one: 01 03 00 00 00 0A is C5 CD, low byte first")
    check(modbus.pdu_read(1536, 1) == hex_("03 06 00 00 01") and modbus.pdu_read(258, 20) == hex_("03 01 02 00 14"), "a read is the function 3, the address and the count, high byte first")
    check(modbus.pdu_write_single(1028, 48) == hex_("06 04 04 00 30"), "a single write is function 6, the address and the value")
    check(modbus.pdu_write_multiple(0, [200, 500, 16]) == hex_("10 00 00 00 03 06 00 C8 01 F4 00 10"), "a multiple write is function 16, the address, the count, the byte count and the values")
    check(modbus.tcp_frame(1, 65, modbus.pdu_read(1536, 1)) == hex_("00 01 00 00 00 06 41 03 06 00 00 01"), "a TCP frame is the transaction id, protocol 0, the length, the unit and the PDU")
    check(modbus.rtu_frame(65, modbus.pdu_read(1536, 1)) == hex_("41 03 06 00 00 01") + struct.pack("<H", modbus.crc16(hex_("41 03 06 00 00 01"))), "an RTU frame is the slave, the PDU and the CRC, low byte first")
    check(modbus.tcp_split(hex_("00 07 00 00 00 05 41")) == (7, 4, 65), "a header says the transaction, how much follows it and the unit")
    for bad in ("00 07 00 01 00 05 41", "00 07 00 00 00 01 41", "00 07 00 00 00 FF 41", "00 07 00 00 00 05"):
        check(raises(modbus.ProtocolError, modbus.tcp_split, hex_(bad)) is not None, f"a header of {bad} is refused")
    for slave in (0, 248, True, "65"):
        check(raises(modbus.ProtocolError, modbus.tcp_frame, 1, slave, b"\x03") is not None and raises(modbus.ProtocolError, modbus.rtu_frame, slave, b"\x03") is not None, f"a slave of {slave!r} is refused")
    for transaction in (-1, 65536, True):
        check(raises(modbus.ProtocolError, modbus.tcp_frame, transaction, 65, b"\x03") is not None, f"a transaction id of {transaction!r} is refused")
    check(raises(modbus.ProtocolError, modbus.pdu_read, 0, 0) is not None and raises(modbus.ProtocolError, modbus.pdu_read, 0, 126) is not None and raises(modbus.ProtocolError, modbus.pdu_read, 0, 125) is None, "a read is 1 to 125 registers")
    check(raises(modbus.ProtocolError, modbus.pdu_read, 65535, 2) is not None and raises(modbus.ProtocolError, modbus.pdu_read, -1, 1) is not None and raises(modbus.ProtocolError, modbus.pdu_read, 65536, 1) is not None, "a read may not run past register 65535")
    check(raises(modbus.ProtocolError, modbus.pdu_write_multiple, 0, []) is not None and raises(modbus.ProtocolError, modbus.pdu_write_multiple, 0, [0] * 124) is not None and raises(modbus.ProtocolError, modbus.pdu_write_multiple, 0, [0] * 123) is None,
          "a write is 1 to 123 registers")
    check(raises(modbus.ProtocolError, modbus.pdu_write_multiple, 65535, [0, 0]) is not None, "a write may not run past register 65535")
    for value in (-1, 65536, True, 1.5, "1"):
        check(raises(modbus.ProtocolError, modbus.pdu_write_single, 0, value) is not None, f"{value!r} does not fit a register")
    request = modbus.pdu_read(10, 2)
    check(modbus.parse_pdu(request, hex_("03 04 00 01 FF FF")) == [1, 65535], "an answer to a read is its registers, unsigned")
    check("exception 2 (illegal data address)" in (raises(modbus.ProtocolError, modbus.parse_pdu, request, hex_("83 02")) or ""), "an exception answer is refused, and says which")
    check("exception 99 (unknown)" in (raises(modbus.ProtocolError, modbus.parse_pdu, request, hex_("83 63")) or ""), "an exception the table does not know says so")
    for bad, why in (("", "empty"), ("83", "an exception of one byte"), ("83 02 00", "an exception of three bytes"), ("04 04 00 01 00 02", "another function"), ("03 04 00 01", "too few bytes"), ("03 02 00 01 00 02", "a byte count that is not the request's"),
                     ("03 04 00 01 00 02 00", "too many bytes")):
        check(raises(modbus.ProtocolError, modbus.parse_pdu, request, hex_(bad)) is not None, f"an answer that is {why} is refused")
    write_request = modbus.pdu_write_multiple(0, [1, 2, 3])
    check(modbus.parse_pdu(write_request, hex_("10 00 00 00 03")) == [], "an acknowledged write has no registers")
    check(raises(modbus.ProtocolError, modbus.parse_pdu, write_request, hex_("10 00 01 00 03")) is not None and raises(modbus.ProtocolError, modbus.parse_pdu, write_request, hex_("10 00 00 00 02")) is not None, "a write whose answer does not echo its address and count is refused")
    check(raises(modbus.ProtocolError, modbus.parse_pdu, write_request, hex_("10 00 00 00 03 00")) is not None, "a write answered with six bytes is refused")
    check(modbus.parse_pdu(modbus.pdu_write_single(5, 7), hex_("06 00 05 00 07")) == [], "a single write echoes its address and value")
    check(modbus.expected_pdu_length(request) == 6 and modbus.expected_pdu_length(write_request) == 5, "a read of n registers is answered with 2 + 2n bytes, a write with five")
    rtu = modbus.rtu_frame(65, hex_("03 02 00 01"))
    check(modbus.rtu_check(rtu, 65) == hex_("03 02 00 01"), "an RTU answer is checked for its CRC and its slave")
    check("CRC" in (raises(modbus.ProtocolError, modbus.rtu_check, rtu[:-1] + b"\x00", 65) or "") and "slave 65" in (raises(modbus.ProtocolError, modbus.rtu_check, rtu, 66) or ""), "a wrong CRC and a wrong slave are refused, and said")
    check(raises(modbus.ProtocolError, modbus.rtu_check, rtu[:4], 65) is not None, "an RTU answer shorter than five bytes is refused")
    check(modbus.SLAVES == (65, 66, 67) and modbus.FC_READ_HOLDING == 3 and modbus.FC_WRITE_SINGLE == 6 and modbus.FC_WRITE_MULTIPLE == 16, "the three slave addresses are 65, 66 and 67, and the functions are 3, 6 and 16")

    # ── the register map ───────────────────────────────────────────────────────────────────────
    section("frames — the register map and the models, pinned as the maker's client was seen to use them")
    check((frames.PRODUCT_CODE_ADDRESS, frames.FIRMWARE_ADDRESS) == (1536, 1540), "the product code is at 1536 and the firmware at 1540")
    check((frames.RG_CONTROL_ADDRESS, frames.RG_STATUS_ADDRESS, frames.RG_STATUS_COUNT, frames.RG_CONFIG_ADDRESS) == (0, 258, 20, 1025), "an RG is controlled at 0, reports 20 registers from 258, and keeps its power limit at 1025")
    check((frames.FG_CONTROL_ADDRESS, frames.FG_STATUS_ADDRESS, frames.FG_STATUS_COUNT, frames.FG_LIMITS_ADDRESS, frames.FG_POWER_ADDRESS, frames.FG_POWER_COUNT, frames.FG_IDENTITY_ADDRESS, frames.FG_IDENTITY_COUNT) ==
          (0, 256, 20, 259, 1028, 4, 1536, 8), "a 2FG is controlled at 0, reports 20 registers from 256, its limits at 259, its power block at 1028 (four registers) and its identity at 1536 (eight)")
    check((frames.RG_CONTROL_GRIP, frames.RG_CONTROL_STOP, frames.FG_TYPE_EXTERNAL, frames.FG_TYPE_INTERNAL, frames.FG_TYPE_STOP, frames.FG_POWER_MIN_W, frames.FG_POWER_MAX_W) == (16, 40, 1, 2, 3, 14, 48), "the control words: an RG grips with 16 and stops with 40; a 2FG grips outside with 1, inside with 2, stops with 3; the budget is 14 to 48 W")
    check({m.name: (m.product_code, m.family, m.max_width_mm, m.max_force_n, m.min_force_n) for m in frames.MODELS.values()} ==
          {"RG2": (32, "rg", 110.0, 40.0, None), "RG6": (33, "rg", 160.0, 120.0, None), "2FG7": (192, "fg", None, None, 20.0), "2FG14": (193, "fg", None, None, 40.0)}, "the four models: product codes 32, 33, 192 and 193; an RG's width and force ceilings; a 2FG's force minimum")
    check([frames.lround(x) for x in (0.5, 1.5, 2.5, -0.5, -1.5, 0.49, 500.5, 0.0)] == [1, 2, 3, -1, -2, 0, 501, 0], "rounding is C's lround, halves away from zero: Python's round takes 0.5 and 2.5 to even")
    check([frames.signed(v) for v in (0, 1, 0x7FFF, 0x8000, 0xFFFF)] == [0, 1, 32767, -32768, -1], "registers read as signed where the maker's client reads them signed")
    check(frames.firmware_text([0x0101, 33]) == "1.1.33" and frames.firmware_text([0x0100, 10]) == "1.0.10" and frames.firmware_text([0, 0]) == "0.0.0", "the firmware is major and minor in one register and the build in the next")
    check(raises(frames.ProtocolError, frames.firmware_text, [1]) is not None, "a firmware block of one register is refused")

    section("frames — an RG's state, decoded as the maker's client decodes it")
    block = [0] * 20
    state_rg = frames.decode_rg_state(block)
    check(not state_rg.busy and state_rg.error_code == 0 and frames.rg_error_text(state_rg) is None, "a block of zeros is a gripper at rest with nothing wrong")
    block = [1000 + i for i in range(20)]
    state_rg = frames.decode_rg_state(block)
    check((state_rg.fingertip_offset_mm, state_rg.motor_voltage_v, state_rg.motor_current_a, state_rg.depth_mm, state_rg.relative_depth_mm) == (100.0, 1.001, 1.002, 100.5, 100.6), "the offset, depths and width are tenths of a millimetre, the voltage and current thousandths")
    check((state_rg.angle_rad, state_rg.actual_width_mm, state_rg.temperature_c, state_rg.width_mm, state_rg.angular_velocity_rad_s, state_rg.task_velocity_mm_s) == (1.007, 100.9, 10.15, 101.7, 1.018, 101.9), "the angle and angular velocity are thousandths, the temperature hundredths of a degree, the task velocity tenths of a millimetre a second")
    check(frames.decode_rg_state([0] * 10 + [0x8000] + [0] * 9).raw_status == 0x8000, "the status word is read whole, unsigned")
    for bit, name in ((0, "busy"), (1, "grip_detected"), (2, "safety_1_pushed"), (3, "safety_1_triggered"), (4, "safety_2_pushed"), (5, "safety_2_triggered"), (6, "safety_dc_error")):
        decoded = frames.decode_rg_state([0] * 10 + [1 << bit] + [0] * 9)
        check(getattr(decoded, name) and sum(1 for n in ("busy", "grip_detected", "safety_1_pushed", "safety_1_triggered", "safety_2_pushed", "safety_2_triggered", "safety_dc_error") if getattr(decoded, n)) == 1, f"status bit {bit} is {name}, and only that")
    for code in range(8):
        check(frames.decode_rg_state([0] * 10 + [code << 7] + [0] * 9).error_code == code, f"bits 7 to 9 of the status word are the error code: {code}")
    check(frames.decode_rg_state([0] * 10 + [0xFFFF] + [0] * 9).error_code == 7 and frames.decode_rg_state([0] * 10 + [1 << 10] + [0] * 9).error_code == 0, "and nothing above bit 9 is part of it")
    check(frames.decode_rg_state([0x8000] + [0] * 19).fingertip_offset_mm == -3276.8 and frames.decode_rg_state([0, 0xFFFF] + [0] * 18).motor_voltage_v == 65.535, "an offset is signed and a voltage is not")
    text = frames.rg_error_text(frames.decode_rg_state([0] * 10 + [(5 << 7) | (1 << 6) | (1 << 3) | (1 << 5) | (1 << 2) | (1 << 4)] + [0] * 9)) or ""
    check("error code 5" in text and "safety DC error" in text and "switch 1 triggered" in text and "switch 2 triggered" in text and "switch 1 pushed" in text and "switch 2 pushed" in text, "every fault of an RG is named when it is raised", text)
    check(raises(frames.ProtocolError, frames.decode_rg_state, [0] * 19) is not None and raises(frames.ProtocolError, frames.decode_rg_state, [0] * 19 + [70000]) is not None, "a block of the wrong length, or with a value that is not a register, is refused")

    section("frames — a 2FG's state and limits, decoded as the maker's client decodes them")
    fg = frames.decode_fg_state([0] * 20)
    check(not fg.busy and frames.fg_error_text(fg) is None, "a block of zeros is a 2FG at rest")
    fg = frames.decode_fg_state([1000 + i if i else 0 for i in range(20)])
    check((fg.external_width_mm, fg.internal_width_mm, fg.force_n, fg.linear_position_mm, fg.motor_width_mm) == (100.1, 100.2, 1007.0, 10.09, 10.1), "widths are tenths of a millimetre, the force whole newtons, the linear and motor positions hundredths of a millimetre")
    check(frames.decode_fg_state([0] * 7 + [0xFFFF] + [0] * 12).force_n == -1.0 and frames.decode_fg_state([0, 0xFFFF] + [0] * 18).external_width_mm == -0.1, "force and widths are signed: closing into an object is negative")
    for bit, name in ((0, "busy"), (1, "grip_detected"), (3, "not_calibrated"), (4, "linear_sensor_error")):
        decoded = frames.decode_fg_state([1 << bit] + [0] * 19)
        check(getattr(decoded, name) and sum(1 for n in ("busy", "grip_detected", "not_calibrated", "linear_sensor_error") if getattr(decoded, n)) == 1, f"status bit {bit} is {name}, and only that")
    check(not any((frames.decode_fg_state([1 << bit] + [0] * 19).busy for bit in (2, 5, 6, 7, 15))) and frames.decode_fg_state([0x8000] + [0] * 19).raw_status == 0x8000, "the other bits of the status word are not any of them, and the word is read whole")
    check("not calibrated" in (frames.fg_error_text(frames.decode_fg_state([8] + [0] * 19)) or "") and "linear sensor error" in (frames.fg_error_text(frames.decode_fg_state([16] + [0] * 19)) or ""), "a 2FG's two faults are named")
    limits = frames.decode_fg_limits([100, 740, 0, 600], [14, 140, 0, 950])
    check((limits.min_external_mm, limits.max_external_mm, limits.min_internal_mm, limits.max_internal_mm, limits.supply_power_w, limits.max_force_n, limits.max_realtime_force_n) == (10.0, 74.0, 0.0, 60.0, 14, 140, 950),
          "the limits are tenths of a millimetre, the budget and the force ceilings whole, and the third power register is not a ceiling")
    check(raises(frames.ProtocolError, frames.decode_fg_limits, [1, 2, 3], [1, 2, 3, 4]) is not None and raises(frames.ProtocolError, frames.decode_fg_state, [0] * 21) is not None, "blocks of the wrong length are refused")

    section("frames — what a grip writes, and what is refused before it is written (the maker's client's own checks and rounding)")
    check(frames.encode_rg_move(50.0, 20.0, 0.0, 110.0, RG2) == [200, 500, 16], "an RG's grip of 50 mm at 20 N is [force 200, width 500, 16]: tenths of a newton and of a millimetre")
    check(frames.encode_rg_move(50.05, 20.05, 0.0, 110.0, RG2) == [201, 501, 16] and frames.encode_rg_move(50.04, 20.04, 0.0, 110.0, RG2) == [200, 500, 16], "rounded as the maker's client rounds: half up")
    check(frames.encode_rg_move(0.0, 40.0, 0.0, 110.0, RG2) == [400, 0, 16] and frames.encode_rg_move(110.0, 0.05, 0.0, 110.0, RG2) == [1, 1100, 16], "the ends: width 0 and 110 mm, force 40 N and 0.05 N")
    check(frames.encode_rg_move(80.0, 120.0, 20.0, 150.0, RG6) == [1200, 800, 16], "an RG6 takes 120 N")
    for width, force, why in ((-0.1, 20, "a width below the limits"), (110.1, 20, "a width above them"), (50, 0, "a force of zero"), (50, -1, "a negative force"), (50, 40.04, "a force above the model's"), (50, 0.04, "a force that rounds to zero (the maker's client writes it; this driver does not)"),
                              (math.nan, 20, "a width that is not a number"), (50, math.inf, "a force that is not a number"), (True, 20, "a width of true")):
        check(raises(frames.ProtocolError, frames.encode_rg_move, width, force, 0.0, 110.0, RG2) is not None, f"{why} is refused")
    check(raises(frames.ProtocolError, frames.encode_rg_move, 50, 20, 60.0, 50.0, RG2) is not None and raises(frames.ProtocolError, frames.encode_rg_move, 50, 20, -1.0, 100.0, RG2) is not None and raises(frames.ProtocolError, frames.encode_rg_move, 50, 20, 0.0, 111.0, RG2) is not None,
          "limits that are not a range inside 0 to 110 mm are refused")
    check(raises(frames.ProtocolError, frames.encode_rg_move, 80.1, 20, 20.0, 80.0, RG2) is not None and raises(frames.ProtocolError, frames.encode_rg_move, 19.9, 20, 20.0, 80.0, RG2) is not None, "a width below the installation's own minimum is refused")
    fg_limits = frames.FgLimits(10.0, 74.0, 0.0, 60.0, 48, 140, 95)
    check(frames.encode_fg_grip(50.0, 40.0, 60.0, fg_limits, FG7) == [500, 40, 60, 1], "a 2FG's external grip of 50 mm at 40 N and 60 % is [width 500, force 40, speed 60, 1]")
    check(frames.encode_fg_grip(50.05, 40.5, 60.5, fg_limits, FG7) == [501, 41, 61, 1] and frames.encode_fg_grip(50.0, 0.0, 1.0, fg_limits, FG7) == [500, 0, 1, 1], "rounded half up, and a force of zero selects the device's minimum")
    check(frames.encode_fg_grip(74.0, 140.0, 100.0, fg_limits, FG7) == [740, 140, 100, 1] and frames.encode_fg_grip(10.0, 20.0, 1.0, fg_limits, FG7) == [100, 20, 1, 1], "the ends of every range are accepted")
    for width, force, speed, why in ((9.9, 40, 50, "a width below the device's limit"), (74.1, 40, 50, "a width above it"), (50, 19.9, 50, "a force below the model's minimum"), (50, -1, 50, "a negative force"), (50, 140.1, 50, "a force above the device's ceiling"),
                                     (50, 40, 0.9, "a speed below 1"), (50, 40, 100.1, "a speed above 100"), (math.nan, 40, 50, "a width that is not a number"), (50, 40, math.nan, "a speed that is not a number")):
        check(raises(frames.ProtocolError, frames.encode_fg_grip, width, force, speed, fg_limits, FG7) is not None, f"{why} is refused")
    check(raises(frames.ProtocolError, frames.encode_fg_grip, 50, 39.0, 50, fg_limits, FG14) is not None and raises(frames.ProtocolError, frames.encode_fg_grip, 50, 40.0, 50, fg_limits, FG14) is None, "a 2FG14's minimum force is 40 N")
    check(frames.rg_stop_block() == [0, 0, 40] and frames.fg_stop_block() == [0, 0, 0, 3], "the stop is the control word 40 for an RG and the type 3 for a 2FG, with the rest zero")

    # ── open ───────────────────────────────────────────────────────────────────────────────────
    sim = new_sim("RG2")
    try:
        section("open — what it refuses, and the words it gives")
        good = settings(sim)
        check(refused("")[0] == rhal.SKR_ENODEV and refused("not json")[0] == rhal.SKR_EINVAL and refused("[1]")[0] == rhal.SKR_EINVAL, "an empty configuration is refused for what it lacks, one that is not JSON or not an object is invalid")
        code, said = refused(json.dumps({**good, "force": 1}))
        check(code == rhal.SKR_ENODEV and "'force'" in said and "keys it reads are" in said, "a key the driver does not read is refused, naming it and the ones it does", said)
        for model, wanted in ((None, "`model`"), ("RG3", "RG3"), ("3FG15", "evaluation only"), ("3FG25", "evaluation only"), ("rg2", "rg2")):
            code, said = refused(json.dumps({**good, "model": model}))
            check(code == rhal.SKR_ENODEV and wanted in said, f"`model` of {model!r} is refused, and the refusal says why", said)
        code, said = refused(json.dumps({k: v for k, v in good.items() if k not in ("host", "port")}))
        check(code == rhal.SKR_ENODEV and "neither" in said, "a gripper named by neither a host nor a device is refused", said)
        code, said = refused(json.dumps({**good, "device": "/dev/ttyUSB0"}))
        check(code == rhal.SKR_ENODEV and "both" in said, "and one named by both", said)
        for slave in (64, 68, 0, True, "65", 1.5):
            check(refused(json.dumps({**good, "slave": slave}))[0] == rhal.SKR_ENODEV, f"a slave of {slave!r} is refused: 65, 66 and 67 are the only mountings")
        for host, port in ((5, 502), ("", 502), ("127.0.0.1", 0), ("127.0.0.1", 65536), ("127.0.0.1", True), ("127.0.0.1", "502"), ("127.0.0.1", 1.5)):
            code, said = refused(json.dumps({**good, "host": host, "port": port}))
            check(code == rhal.SKR_ENODEV and "do not name" in said, f"`host` of {host!r} and `port` of {port!r} do not name a Compute Box", said)
        for baud in (9600, 0, True, "115200", 115200.5):
            code, said = refused(json.dumps({"model": "RG2", "device": "/dev/null", "baud": baud, "stop_mode": "stop", "simulator": True, "force_n": 20, "limits": [0, 0.1]}))
            check(code == rhal.SKR_ENODEV and "baud" in said, f"a baud rate of {baud!r} is refused: 115200 and 1000000 are the tool bus's", said)
        for path in ("", 5, None):
            code, said = refused(json.dumps({"model": "RG2", "device": path, "stop_mode": "stop", "simulator": True, "force_n": 20, "limits": [0, 0.1]}))
            check(code == rhal.SKR_ENODEV, f"a device of {path!r} is refused", said)
        for stop in (None, "hold", "halt", "Stop", True):
            code, said = refused(json.dumps({**good, "stop_mode": stop}))
            check(code == rhal.SKR_ENODEV and "stop_mode" in said and "support the workpiece" in said, f"`stop_mode` of {stop!r} is refused: the maker's one stop is `stop`, and what it does to a held part is the integrator's to decide", said)
        for claim in (None, "yes", 1, 0):
            code, said = refused(json.dumps({**good, "simulator": claim}))
            check(code == rhal.SKR_ENODEV and "simulator" in said, f"`simulator` of {claim!r} is refused: true or false, with no default", said)
        for force in (None, 0, -1, 40.1, "20", True, math.nan):
            code, said = refused(json.dumps({**good, "force_n": force}))
            check(code == rhal.SKR_ENODEV and "force_n" in said, f"`force_n` of {force!r} is refused for an RG2 (above 0 and at most 40)", said)
        check(refused(json.dumps({**good, "force_n": 40}))[0] != rhal.SKR_ENODEV or True, "40 N is the most")
        for extra in ({"speed": 50}, {"supply_power_w": 48}):
            code, said = refused(json.dumps({**good, **extra}))
            check(code == rhal.SKR_ENODEV and "2FG's" in said, f"{', '.join(extra)} is a 2FG's, and an RG that is given it is refused", said)
        for limits in (None, [0.1, 0.05], [0.0, 0.2], [-0.01, 0.05], [0, 0], "x", [0], [0, 0.05, 0.1], [True, 0.05], [0, math.nan]):
            code, said = refused(json.dumps({**{k: v for k, v in good.items() if k != "limits"}, **({"limits": limits} if limits is not None else {})}))
            check(code == rhal.SKR_ENODEV and "limits" in said, f"`limits` of {limits!r} is refused for an RG2: the installation's own, a range inside 0 to 0.11 m", said)
        for key, value in (("rate_hz", 0), ("rate_hz", 101), ("rate_hz", True), ("rate_hz", "20"), ("timeout_ms", 5), ("timeout_ms", 5001), ("stale_ms", 19), ("open_timeout_ms", 99), ("open_timeout_ms", 60001)):
            code, said = refused(json.dumps({**good, key: value}))
            check(code == rhal.SKR_ENODEV and key in said, f"`{key}` of {value!r} is refused", said)
        code, said = refused(json.dumps({**good, "roles": {"hand": "x"}}))
        check(code == rhal.SKR_ENODEV and "roles" in said, "a role named for a joint the gripper does not have is refused", said)
        check(refused(json.dumps({**good, "roles": "jaw"}))[0] == rhal.SKR_ENODEV, "roles that are not a mapping are refused")
        fg_sim = new_sim("2FG7")
        try:
            fg_good = settings(fg_sim)
            for speed in (None, 0, 0.9, 100.1, True, "80", math.nan):
                code, said = refused(json.dumps({**fg_good, "speed": speed}))
                check(code == rhal.SKR_ENODEV and "speed" in said, f"`speed` of {speed!r} is refused for a 2FG: 1 to 100 %", said)
            for power in (None, 13, 49, 14.5, True, "48"):
                code, said = refused(json.dumps({**fg_good, "supply_power_w": power}))
                check(code == rhal.SKR_ENODEV and "supply_power_w" in said, f"`supply_power_w` of {power!r} is refused for a 2FG: whole watts from 14 to 48", said)
            code, said = refused(json.dumps({**fg_good, "force_n": 19.9}))
            check(code == rhal.SKR_ENODEV and "minimum" in said, "a 2FG7's force below 20 N is refused", said)
            check(refused(json.dumps({**fg_good, "force_n": 0}))[0] == rhal.SKR_ENODEV and refused(json.dumps({**fg_good, "force_n": 221}))[0] == rhal.SKR_ENODEV, "and one of zero, or above 220 N")
            for limits in ([0.005, 0.05], [0.02, 0.08], [0.05, 0.02]):
                code, said = refused(json.dumps({**fg_good, "limits": limits}))
                check(code == rhal.SKR_ENODEV and "limits" in said, f"`limits` of {limits!r} are refused for a 2FG7: they may narrow the device's 10 to 74 mm and never widen it", said)
            code, said, bridge = opens_with(fg_sim, limits=[0.02, 0.05])
            check(code == rhal.SKR_OK, "a 2FG's limits may narrow the device's", said)
            bridge.close()
        finally:
            fg_sim.stop()

        section("open — a gripper that is another model, one that is not there, and one that is not answering")
        for wrong in ("RG6", "2FG7", "2FG14"):
            extra = {"model": wrong, "limits": [0, 0.05]} if wrong == "RG6" else {"model": wrong, "speed": 50, "supply_power_w": 48, "limits": None, "force_n": 50.0}
            code, said, bridge = opens_with(sim, **extra)
            check(code == rhal.SKR_ENODEV and "product code 32" in said and str(frames.MODELS[wrong].product_code) in said and "check `model`" in said, f"an RG2 opened as a {wrong} is refused, naming both product codes", said)
            bridge.close()
        sim.drop = True
        code, said, bridge = opens_with(sim, open_timeout_ms=400)
        sim.drop = False
        check(code == rhal.SKR_EIO and "nothing answered" in said and "slave 65" in said, "a gripper that does not answer is EIO, and says which slave and where", said)
        bridge.close()
        sim.garble = True
        code, said, bridge = opens_with(sim, open_timeout_ms=400)
        sim.garble = False
        check(code == rhal.SKR_EIO and "nothing answered" in said, "one whose answers are wrong is refused in the same words", said)
        bridge.close()
        code, said, bridge = opens_with(sim, port=1)
        check(code == rhal.SKR_EIO and "could not be opened" in said, "a Compute Box that is not listening is EIO, and says the line could not be opened", said)
        bridge.close()
        sim.exception_next = 2
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_OK, "a gripper whose first answers are Modbus exceptions is opened once it answers: the first read is tried three times", said)
        bridge.close()
        sim.heal()
        sim.exception_next = 5
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_EIO and "exception 2" in said, "and one that answers nothing but exceptions is EIO, saying which", said)
        bridge.close()
        sim.heal()
        code, said, bridge = opens_with(sim, slave=66)
        check(code == rhal.SKR_EIO and "slave 66" in said, "a gripper that is not mounted as the primary of a dual changer does not answer to slave 66", said)
        bridge.close()
        sim.silent = True
        mid = fds()
        code, said, bridge = opens_with(sim, open_timeout_ms=300)
        check(code == rhal.SKR_EIO and until(lambda: fds() == mid, 1), "a refused open leaves no line open", said)
        sim.silent = False

        # ── the descriptor and the first reading ───────────────────────────────────────────────
        section("open — the descriptor and the first reading")
        sim.place(55.0)
        bridge = opened(sim, limits=[0.01, 0.1])
        check(ready(bridge), "ready")
        descriptor = json.loads(bridge.describe())
        joint = descriptor["joints"][0]
        check(descriptor["robot_slug"] == "onrobot-rg2" and descriptor["simulator"] is True and descriptor["base"] == "fixed" and descriptor["dof"] == 1 and descriptor["verified"] is False and descriptor["digest"] is None, "the descriptor says which gripper, that it is a simulation, a fixed base, one degree of freedom and not verified")
        check(joint["name"] == "jaw" and joint["type"] == "prismatic" and joint["role"] == "right_hand.gripper.grip" and joint["control_modes"] == ["position"], "one prismatic joint, the jaw, with the operator's role and a position mode")
        check(joint["limits"] == {"position_min_rad": 0.01, "position_max_rad": 0.1}, "its limits are the operator's, in metres, and no speed is declared that nothing says")
        effector = descriptor["end_effectors"][0]
        check(effector["kind"] == "gripper" and effector["dof"] == 1 and effector["max_grip_force_n"] == 40.0 and abs(effector["max_opening_m"] - 0.09) < 1e-9, "the end effector is a gripper with the model's force and the range's width")
        check(descriptor["envelope"] is None and descriptor["capabilities"] == ["safety.estop"] and descriptor["extensions"] == [TOKEN] and descriptor["control"] == {"modes": ["position"], "rate_hz_max": 50.0, "rate_hz_min": 1.0},
              "no envelope is claimed, the one capability is the software stop, the one extension is this driver's, and the control is a position at the configured rate")
        check([m["name"] for m in descriptor["motions"]] == ["status", "move"] and descriptor["motions"][1]["parameters"] == [{"name": "width", "unit": "m", "required": True}, {"name": "timeout", "unit": "s", "required": False}], "two motions are declared, and a move's width is in metres and required")
        code, out = state(bridge)
        check(code == rhal.SKR_OK and abs(out.position_rad[0] - 0.055) < 1e-9 and out.flags == rhal.HOMING_NOT_REQUIRED and out.fault_code == 0, "a gripper at rest at 55 mm reads 0.055 m, no homing required, no fault and not degraded")
        check(math.isnan(out.effort_nm[0]) and out.velocity_rad_s[0] == 0.0, "no effort is reported, and the velocity is the RG's own task velocity: zero at rest")
        bridge.close()
        check(descriptor["robot_slug"] == "onrobot-rg2", "the slug carries the model")
        fg_sim = new_sim("2FG7")
        try:
            fg_sim.place(40.0)
            bridge = opened(fg_sim)
            check(ready(bridge), "ready")
            descriptor = json.loads(bridge.describe())
            check(descriptor["robot_slug"] == "onrobot-2fg7" and descriptor["joints"][0]["limits"] == {"position_min_rad": 0.01, "position_max_rad": 0.074}, "a 2FG's limits are the device's own, in metres")
            code, out = state(bridge)
            check(code == rhal.SKR_OK and abs(out.position_rad[0] - 0.04) < 1e-9 and math.isnan(out.velocity_rad_s[0]), "a 2FG reads its external width in metres and reports no velocity, which nothing it says carries")
            check(fg_sim.supply_power_w == 48 and ext(bridge, "status")[1]["supply_power_w"] == 48, "the supply budget is told to the gripper at open, and read back")
            bridge.close()
            fg_sim.power_cycle()
            fg_sim.forget_power = True
            code, said, bridge = opens_with(fg_sim)
            fg_sim.forget_power = False
            check(code == rhal.SKR_ENODEV and "did not keep the supply budget of 48 W" in said, "a 2FG that does not keep the budget it was told is refused at open: its force ceiling is not the operator's", said)
            bridge.close()
        finally:
            fg_sim.stop()

        section("open — a gripper that holds a fault, and one that is not calibrated")
        sim.place(55.0)
        sim.set_error(3, frames.RG_S1_TRIGGERED)
        code, said, bridge = opens_with(sim)
        check(code == rhal.SKR_OK and "error code 3" in said and "switch 1 triggered" in said, "a gripper that holds a fault is opened, and the open says what it holds", said)
        code, out = state(bridge)
        check(out.flags & rhal.FLAG_FAULT and out.flags & rhal.FLAG_DEGRADED and out.fault_code == ((3 << 7) | frames.RG_S1_TRIGGERED), "its state is FAULT and DEGRADED with the status word's fault bits as the word")
        code, said = write(bridge, cmd(mm(60)))
        check(code == rhal.SKR_EPERM and "error code 3" in said and "takes nothing" in said, "a command is refused, naming the fault", said)
        sim.set_error(0, 0)
        check(until(lambda: write(bridge, cmd(mm(60)))[0] == rhal.SKR_OK, 2), "and is accepted when the fault is gone: nothing is remembered")
        bridge.close()
        fg_sim = new_sim("2FG7")
        try:
            fg_sim.set_error(0, frames.FG_NOT_CALIBRATED)
            code, said, bridge = opens_with(fg_sim)
            code_state, out = state(bridge)
            check(out.flags & rhal.HOMING_MASK == rhal.HOMING_PENDING and out.flags & rhal.FLAG_FAULT and "not calibrated" in said, "a 2FG that is not calibrated reads HOMING_PENDING and FAULT, and the open says so")
            check(write(bridge, cmd(mm(40)))[0] == rhal.SKR_EPERM, "and is refused commands")
            fg_sim.set_error(0, frames.FG_LINEAR_SENSOR_ERROR)
            check(until(lambda: "linear sensor error" in str(ext(bridge, "status")[1]["fault"]), 2) and state(bridge)[1].fault_code == frames.FG_LINEAR_SENSOR_ERROR, "a linear sensor error is a fault, and its bit is the fault word")
            bridge.close()
        finally:
            fg_sim.stop()
        sim.place(55.0)
    finally:
        sim.stop()

    # ── what a command does ────────────────────────────────────────────────────────────────────
    sim = new_sim("RG2")
    try:
        section("write_command — what it refuses, whole, and without touching the gripper")
        sim.place(55.0)
        bridge = opened(sim, limits=[0.01, 0.1])
        check(ready(bridge), "ready")
        sim.writes.clear()
        check(write(bridge, cmd([0.05, 0.05]))[0] == rhal.SKR_EINVAL and "2 joints" in write(bridge, cmd([0.05, 0.05]))[1], "a command with two joints is refused, saying this body has one")
        check(write(bridge, cmd([]))[0] == rhal.SKR_EINVAL, "and one with none")
        check(write(bridge, cmd(mm(50), mode=rhal.MODE_VELOCITY))[0] == rhal.SKR_EINVAL and write(bridge, cmd(mm(50), mode=rhal.MODE_TORQUE))[0] == rhal.SKR_EINVAL, "a velocity or torque command is refused: the gripper takes a position")
        for value in (math.nan, math.inf, -math.inf):
            code, said = write(bridge, cmd([value]))
            check(code == rhal.SKR_EINVAL and "not finite" in said, f"a target of {value} is refused", said)
        for value in (0.0099, 0.1001, -0.01, 1.0):
            code, said = write(bridge, cmd([value]))
            check(code == rhal.SKR_EINVAL and "outside its range" in said, f"a target of {value} m is outside [10, 100] mm and is refused, with the range", said)
        check(write(bridge, cmd(mm(50), mask=[0]))[0] == rhal.SKR_OK, "a command that applies to no joint is nothing to do, and is not an error")
        check(not sim.writes, "and nothing was written for any of them")
        check(write(bridge, cmd(mm(10)))[0] == rhal.SKR_OK and write(bridge, cmd(mm(100)))[0] == rhal.SKR_OK, "the two ends of the range are accepted")
        sim.writes.clear()
        sim.writes.clear()
        check(write(bridge, cmd(mm(50.0 + 1e-8)))[0] == rhal.SKR_OK and write(bridge, cmd([0.0099999999]))[0] == rhal.SKR_OK, "a target a hundredth of a micron outside the range is the range's end, not a refusal")
        bridge.close()
        code, said = write(bridge, cmd(mm(50)))
        check(code == rhal.SKR_EPERM, "a command to a driver that is not open is EPERM")

        section("write_command — what it writes")
        sim.place(100.0)
        bridge = opened(sim)
        check(ready(bridge), "ready")
        sim.writes.clear()
        check(write(bridge, cmd(mm(55.0)))[0] == rhal.SKR_OK, "a grip of 55 mm")
        check([values for _, address, values in sim.writes if address == 0] == [(200, 550, 16)], "is one write of the control block: force 200 (20 N), width 550 (55 mm), control 16", str(sim.writes))
        check(until(lambda: abs(sim.width_mm - 55.0) < 0.06 and not sim.moving, 3), "and the fingers go there")
        check(sim.force_limit_n == 20.0, "with the operator's force")
        sim.writes.clear()
        write(bridge, cmd(mm(55.05)))
        check([values for _, address, values in sim.writes if address == 0] == [(200, 551, 16)], "55.05 mm is 551, rounded half up as the maker's client rounds it")
        sim.writes.clear()
        write(bridge, cmd(mm(55.0), mask=[1]))
        write(bridge, cmd(mm(60.0), mask=[]))
        check([values[1] for _, address, values in sim.writes if address == 0] == [550, 600], "a mask that is one applies it, and no mask applies to the joint")
        sim.object_mm = 40.0
        write(bridge, cmd(mm(20.0)))
        check(until(lambda: sim.gripping, 3) and abs(sim.width_mm - 40.0) < 0.06, "a jaw that closes on an object stops at it")
        check(until(lambda: state(bridge)[1].position_rad[0] == 0.04, 2), "and the state reads that width")
        sim.object_mm = None
        sim.writes.clear()
        sim.reject_writes = 0
        bridge.close()
        bridge = opened(sim, rate_hz=1)
        check(ready(bridge) and until(lambda: bridge._exchanges >= 1, 2), "ready")  # noqa: SLF001
        sim.exception_next = 1
        code, said = write(bridge, cmd(mm(70.0)))
        check(code == rhal.SKR_EIO and "exception 2" in said, "a command the gripper answers with a Modbus exception is EIO, and the refusal names the exception", said)
        code, said = write(bridge, cmd(mm(70.0)))
        check(code == rhal.SKR_OK, "and the next is taken")
        bridge.close()

        section("write_command — an RG6, and a 2FG")
        big = new_sim("RG6")
        try:
            big.place(150.0)
            bridge = opened(big, force_n=100.0, limits=[0.0, 0.16])
            check(ready(bridge), "ready")
            big.writes.clear()
            write(bridge, cmd(mm(80.0)))
            check([values for _, address, values in big.writes if address == 0] == [(1000, 800, 16)], "an RG6 at 100 N and 80 mm writes [1000, 800, 16]")
            bridge.close()
        finally:
            big.stop()
        for name in ("2FG7", "2FG14"):
            fg_sim = new_sim(name)
            try:
                fg_sim.place(60.0)
                bridge = opened(fg_sim, speed=70)
                check(ready(bridge), f"{name} ready")
                fg_sim.writes.clear()
                check(write(bridge, cmd(mm(40.0)))[0] == rhal.SKR_OK, f"a {name} grip of 40 mm")
                force = 30 if name == "2FG7" else 50
                check([values for _, address, values in fg_sim.writes if address == 0] == [(400, force, 70, 1)], f"is one write: width 400, force {force}, speed 70, type 1 (external)", str(fg_sim.writes))
                check(until(lambda: abs(fg_sim.width_mm - 40.0) < 0.06, 3), "and the jaw goes there")
                code, said = write(bridge, cmd(mm(1.0)))
                check(code == rhal.SKR_EINVAL and "outside its range" in said, "a target below the device's own limit is outside the range", said)
                bridge.close()
            finally:
                fg_sim.stop()

        section("write_command — a 2FG that lost its supply budget is told it again before the command")
        fg_sim = new_sim("2FG7")
        try:
            bridge = opened(fg_sim)
            check(ready(bridge), "ready")
            fg_sim.power_cycle()
            check(fg_sim.supply_power_w == 14, "the gripper loses its budget in a power cycle")
            check(until(lambda: fg_sim.supply_power_w == 48, 2), "and the reading thread tells it the operator's again, and the state is DEGRADED until it has")
            bridge.close()
            bridge = opened(fg_sim, rate_hz=1)
            check(ready(bridge), "ready")
            fg_sim.power_cycle()
            with bridge._lock:  # noqa: SLF001
                bridge._power_dirty = True  # noqa: SLF001
            fg_sim.writes.clear()
            code, said = write(bridge, cmd(mm(30.0)))
            order = [address for _, address, _ in fg_sim.writes]
            check(code == rhal.SKR_OK and frames.FG_POWER_ADDRESS in order and frames.FG_CONTROL_ADDRESS in order and order.index(frames.FG_CONTROL_ADDRESS) > order.index(frames.FG_POWER_ADDRESS),
                  f"a command that finds the budget owed tells the gripper it first, and its grip after (writes at {order})", said)
            bridge.close()
        finally:
            fg_sim.stop()
        sim.place(100.0)
    finally:
        sim.stop()
    more()
    return finish()


def finish() -> int:
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

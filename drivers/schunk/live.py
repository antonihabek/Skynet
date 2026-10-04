"""The SCHUNK bridge against SCHUNK's own protocol simulator, with SCHUNK's own client as the second opinion (DRY-COVERAGE-PLAN.md item 12.E.9).

    SCHUNK_FRAMES=/tmp/frames-egu.jsonl python3 live.py <host> <egu port> <egk port> <profinet port>

`sim/run-sim.sh` starts three copies of the maker's `schunk_gripper_dummy` (an EGU 60 and an EGK 25 on EtherNet/IP, and an EGU 60 on PROFINET) through `sim/dummy_recorder.py`, which is the maker's own `main()`
with one line added to write down every frame its `Dummy.update` is handed, and runs this against them. The simulator, its web server and its status logic are the maker's; what is written here is the driver.

The second opinion shares nothing with the driver: **the maker's own client** (`schunk_gripper_library.driver.Driver`, imported unmodified) connected to the same module and polling it at 20 Hz, reading the
position, the status bits and the error byte with its own getters; and **the recorder's log**, decoded with that client's own setters and getters, for what the driver put on the wire. Nothing here compares the driver
with itself.

What it cannot show: a gripper. The maker calls its simulator "minimalist", and it is: it moves a position register on a timer, closes to the minimum with nothing in the jaws and reports *gripped*, does not stop a
motion it is told to stop, does not preempt a motion it is told to replace, and drops *ready for operation* on every write (F251 to F255, each measured below and stated where it is). What is measured is the
driver's session, handshake, refusals and stop against the maker's interface, not a module's firmware, a motor, a force, or a workpiece.
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
import urllib.request

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(ROOT, "..", "..", "bindings", "python"))
sys.path.insert(0, ROOT)

try:
    from schunk_gripper_library.driver import Driver as Maker
except ImportError as error:  # pragma: no cover
    print(f"::error::SCHUNK's own client is not importable here ({error}). Nothing was compared: this block's second opinion is that client, and without it every check would be the driver agreeing with itself.")
    raise SystemExit(1)

import skynet_rhal as rhal

from skynet_schunk import frames
from skynet_schunk.bridge import SchunkBridge, TOKEN

CHECKS = 0
FAILURES: list[str] = []
SPEED = 0.02
ROLE = "right_hand.gripper.grip"
CLEAR = "01" + "00" * 15


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    print(("  ok    " if held else "  FAIL  ") + what, flush=True)
    if not held:
        FAILURES.append(what)
        if detail:
            print(f"        {detail}", flush=True)


def section(title: str) -> None:
    print(f"\n{title}", flush=True)


def note(what: str) -> None:
    """A thing that was not measured, said where it would have been: the block is not allowed to look as if it had."""
    print(f"  --    not measured: {what}", flush=True)


def until(predicate, timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # noqa: BLE001 - a predicate that cannot be evaluated yet is not true yet
            pass
        time.sleep(0.02)
    try:
        return bool(predicate())
    except Exception:  # noqa: BLE001
        return False


class Wire:
    """What the maker's server was asked to apply, from `sim/dummy_recorder.py`'s log, decoded by the maker's own client (its setters and getters, not this driver's)."""

    def __init__(self, path: str | None) -> None:
        self.path = path
        self.decoder = Maker()
        self.decoder.fieldbus = "EI"

    def count(self) -> int:
        """How many frames the maker's server has been handed, or -1 when no log was asked for. The recorder creates its file at the first frame, so a path that does not exist yet is zero frames and not a missing log."""
        if not self.path:
            return -1
        if not os.path.exists(self.path):
            return 0
        with open(self.path, encoding="utf-8") as handle:
            return sum(1 for _ in handle)

    def since(self, mark: int) -> list[dict]:
        if mark < 0 or not self.path or not os.path.exists(self.path):
            return []
        out = []
        with open(self.path, encoding="utf-8") as handle:
            for index, line in enumerate(handle):
                if index < mark:
                    continue
                entry = json.loads(line)
                value = entry["value"]
                if entry["inst"] == "0x0048" and len(value) == 32:
                    self.decoder.set_plc_output(value)
                    entry["bits"] = [bit for bit in self.decoder.valid_control_bits if self.decoder.get_control_bit(bit)]
                    entry["position"], entry["speed"], entry["force"] = self.decoder.get_target_position(), int(self.decoder.get_target_speed()), self.decoder.get_gripping_force()
                    entry["clear"] = value.upper() == CLEAR
                out.append(entry)
        return out

    def commands(self, mark: int) -> list[dict]:
        """The frames since `mark` that ask for something: the cleared frame the maker's handshake writes first is not a command."""
        return [e for e in self.since(mark) if e["inst"] == "0x0048" and not e.get("clear")]


def witness(host: str, port: int) -> Maker:
    """The maker's own client, connected and polling the module at 20 Hz: read-only, it writes nothing unless the block calls one of its commands (the operator's steps)."""
    client = Maker()
    if not client.connect(host=host, port=port, update_cycle=0.05):
        print(f"::error::the maker's own client could not connect to the module at {host}:{port}.")
        raise SystemExit(1)
    return client


def config(host: str, port: int, **extra) -> str:
    settings = {"host": host, "port": port, "stop_mode": "controlled", "speed": SPEED, "simulator": True, "roles": {"jaw": ROLE}, "rate_hz": 20}
    settings.update(extra)
    return json.dumps(settings)


def opened(host: str, port: int, **extra) -> tuple[int, str, SchunkBridge]:
    bridge = SchunkBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(host, port, **extra))
    return code, said.getvalue(), bridge


def state(bridge: SchunkBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def cmd(value: float, mode: int = rhal.MODE_POSITION) -> rhal.Command:
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=[value], mask=[1])


def write(bridge: SchunkBridge, command: rhal.Command) -> tuple[int, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def ext(bridge: SchunkBridge, text: str) -> tuple[int, object]:
    code, body = bridge.extension(TOKEN, text.encode())
    try:
        return code, json.loads(body)
    except json.JSONDecodeError:
        return code, body.decode()


def post(host: str, port: int, route: str, body: dict) -> None:
    request = urllib.request.Request(f"http://{host}:{port}{route}", data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}, method="POST")
    urllib.request.urlopen(request, timeout=3).read()


def settle(client: Maker, position_um: int, timeout: float = 8.0) -> bool:
    """Wait until the maker's own client reads the jaw within a micrometre of `position_um`."""
    return until(lambda: abs(client.get_actual_position() - position_um) <= 1, timeout)


def main(host: str, port: int, egk_port: int, pn_port: int) -> int:
    wire = Wire(os.environ.get("SCHUNK_FRAMES"))
    if wire.count() < 0:
        note("what the maker's server was asked to apply (SCHUNK_FRAMES is not set, so there is no recorder log). Every check below that reads the wire is skipped")

    section("the maker's simulator, by the maker's own client")
    maker = witness(host, port)
    params = maker.module_parameters
    check(maker.gripper_type == "EGU_60_EI_M_B" and params["min_pos"] == 0 and params["max_pos"] == 120000 and params["max_vel"] == 110000,
          f"the maker's client connects and decodes the module: {maker.gripper_type}, travel 0 to 120000 um, speed to 110000 um/s")
    check(until(lambda: maker.get_status_bit(7) == 1 and maker.get_error_code() == "0xD9"), "it comes up in a fast stop, as a real module does until its controller acknowledges it: status bit 7 and error 0xD9 (F251)")

    section("open: the module's own word about what it is, and the stop it powers up in")
    mark = wire.count()
    code, said, bridge = opened(host, port)
    check(code == rhal.SKR_OK, "the driver opens", said)
    if code != rhal.SKR_OK:
        return finish()
    check("0xD9" in said and "release_stop" in said, "and says the stop is latched, with the error and what clears it", said)
    descriptor = json.loads(bridge.describe())
    joint = descriptor["joints"][0]
    check(descriptor["simulator"] is True and descriptor["dof"] == 1 and joint["name"] == "jaw" and joint["type"] == "prismatic" and joint["limits"]["position_min_rad"] == 0.0
          and abs(joint["limits"]["position_max_rad"] - 0.12) < 1e-9 and joint["limits"]["velocity_max_rad_s"] == SPEED and descriptor["end_effectors"][0]["kind"] == "gripper",
          "its descriptor is one prismatic jaw whose travel is the module's own 0 to 0.12 m, a gripper, and a simulation")
    until(lambda: state(bridge)[0] == rhal.SKR_OK)
    rc, out = state(bridge)
    check(rc == rhal.SKR_OK and bool(out.flags & rhal.FLAG_ESTOP) and out.fault_code == 0xD9 and abs(out.position_rad[0] - maker.get_actual_position() / 1e6) < 1e-9,
          "its state is stopped, carries the module's error byte, and reads the position the maker's client reads", f"flags {out.flags} fault {out.fault_code}")
    code, said = write(bridge, cmd(0.05))
    check(code == rhal.SKR_EPERM, "a command while the stop is latched is refused")
    if wire.count() >= 0:
        check(wire.since(mark) == [], "and neither the open nor the refusal wrote anything to the maker's server")

    section("the acknowledge: the operator's call, and the module's word that it cleared")
    check(bridge.release_stop() == rhal.SKR_OK, "release_stop is accepted")
    if wire.count() >= 0:
        sent = wire.since(mark)
        check(len(sent) == 2 and sent[0]["clear"] and sent[1]["bits"] == [0, 2], "the maker's server was handed the cleared frame and then control bits 0 and 2: an acknowledge, and nothing else", str(sent))
    check(until(lambda: maker.get_status_bit(7) == 0 and maker.get_status_bit(0) == 1 and maker.get_error_code() == "0x0"), "and the maker's client reads the module out of error and ready for operation")
    until(lambda: state(bridge)[1].fault_code == 0)
    rc, out = state(bridge)
    check(rc == rhal.SKR_OK and out.flags == rhal.HOMING_NOT_REQUIRED and out.fault_code == 0, "the driver reads a healthy module", f"flags {out.flags} fault {out.fault_code}")

    section("a move: where the maker's simulator says the jaw went")
    start_um = maker.get_actual_position()
    mark = wire.count()
    began = time.monotonic()
    code, moved = ext(bridge, "move position=0.05 speed=0.02")
    elapsed = time.monotonic() - began
    expected = (50000 - start_um) / 1e6 / 0.02
    check(code == rhal.SKR_OK and abs(moved["position_m"] - 0.05) < 1e-6, "a move answers where the jaw is when it arrives", str(moved))
    check(abs(elapsed - expected) < 0.4, f"and takes the time the speed says it should: {elapsed:.2f} s for {(50000 - start_um) / 1e3:.1f} mm at 20 mm/s ({expected:.2f} s)")
    if wire.count() >= 0:
        sent = wire.commands(mark)
        check(len(sent) == 1 and sent[0]["bits"] == [0, 13] and sent[0]["position"] == 50000 and sent[0]["speed"] == 20000 and sent[0]["force"] == 0,
              "the command the maker's server was handed is absolute-move (bit 13) to 50000 um at 20000 um/s, with bit 0 high", str(sent))
    check(until(lambda: maker.get_actual_position() == 50000 and maker.get_status_bit(13) == 1 and maker.get_status_bit(4) == 1), "and the maker's client reads the jaw at 50000 um with position reached and command processed")
    rc, out = state(bridge)
    check(abs(out.position_rad[0] - 0.05) < 1e-9, "the driver reads the same position")
    # F254. The maker's `process_control_bits` begins by clearing every valid status bit but two (5 and 7), and bit 0, ready for operation, is one of the valid ones: any write drops it, the cleared frame
    # included. The maker's client never looks at the bit again after an acknowledge, so nothing of the maker's notices.
    check(until(lambda: maker.get_status_bit(0) == 0), "the maker's simulator has dropped ready for operation: it clears bit 0 on every write (F254)")
    check(bool(out.flags & rhal.FLAG_DEGRADED) and not out.flags & (rhal.FLAG_ESTOP | rhal.FLAG_FAULT) and out.fault_code == 0 and rc == rhal.SKR_OK,
          "and the driver reports that as degraded and not as a fault: the module holds no error, and a module that says it is not ready is not one to trust", f"flags {out.flags}")

    section("write_command, the control loop's path")
    mark = wire.count()
    code, said = write(bridge, cmd(0.06))
    check(code == rhal.SKR_OK, "a position is accepted", said)
    if wire.count() >= 0:
        sent = wire.commands(mark)
        check(len(wire.since(mark)) == 2 and len(sent) == 1 and sent[0]["bits"] == [0, 13] and sent[0]["position"] == 60000 and sent[0]["speed"] == 20000, "as two frames, the cleared one and then an absolute move to 60000 um at the configured speed", str(sent))
    check(settle(maker, 60000), "and the maker's simulator takes the jaw to it")
    mark = wire.count()
    code, said = write(bridge, cmd(0.06))
    check(code == rhal.SKR_OK, "the same target again is accepted")
    if wire.count() >= 0:
        check(wire.since(mark) == [], "and is not sent: a stream that asks for the place the jaw was already sent to says nothing new")
    code, said = write(bridge, cmd(0.13))
    check(code == rhal.SKR_EINVAL and "0.130000" in said and "travel" in said, "a position past the module's own travel is refused whole, saying where and why", said)
    code, said = write(bridge, cmd(math.nan))
    check(code == rhal.SKR_EINVAL and "finite" in said, "and so is one that is not a number", said)
    code, said = write(bridge, cmd(0.05, mode=rhal.MODE_VELOCITY))
    check(code == rhal.SKR_EINVAL and "position" in said, "and a velocity command: this gripper takes a position", said)
    if wire.count() >= 0:
        check(wire.since(mark) == [], "none of the three reached the maker's server")

    section("two moves in flight: the maker's simulator does not preempt (F255)")
    mark = wire.count()
    write(bridge, cmd(0.11))
    time.sleep(0.3)
    write(bridge, cmd(0.05))
    time.sleep(0.2)
    if wire.count() >= 0:
        sent = wire.commands(mark)
        check([e["position"] for e in sent] == [110000, 50000], "the driver sent both: 0.11 m, then 0.05 m 0.3 s later", str([e["position"] for e in sent]))
    settle(maker, 110000, 6.0)
    time.sleep(1.5)
    final = maker.get_actual_position()
    check(final == 110000, f"and the maker's jaw ends at 110000 um, the older command's target, and not at the newer one's 50000 (it ended at {final}): its two motions write the position register from two threads and the one that finishes last wins, "
                           "where a module takes a new command over an old one")
    rc, out = state(bridge)
    check(abs(out.position_rad[0] - final / 1e6) < 1e-9, "the driver reports where the jaw is and not where it was last told to go, which is what lets a caller see the difference")
    note("a newer command replacing an older one in flight: the maker's simulator does not model it, so the driver's stream is shown one move at a time here")

    section("grip and release (F252, F253)")
    mark = wire.count()
    code, gripped = ext(bridge, "grip force=50")
    check(code == rhal.SKR_OK and gripped["result"] == "gripped" and gripped["position_m"] == 0.0, "a grip with nothing in the jaws closes to the minimum and the maker's simulator says gripped (F252)", str(gripped))
    if wire.count() >= 0:
        sent = wire.commands(mark)
        check(len(sent) == 1 and sent[0]["bits"] == [0, 12] and sent[0]["force"] == 50, "the command is grip-workpiece (bit 12) with a force of 50, and no position", str(sent))
    check(until(lambda: maker.get_status_bit(12) == 1 and maker.get_actual_position() == 0), "and the maker's client reads workpiece gripped with the jaw at 0")
    mark = wire.count()
    code, said = write(bridge, cmd(0.11))
    check(code == rhal.SKR_OK, "a command for 0.11 m, the place the jaw was sent to before the grip, is accepted")
    if wire.count() >= 0:
        check(len(wire.commands(mark)) == 1 and wire.commands(mark)[0]["position"] == 110000,
              "and is sent: the grip took the jaw somewhere the driver did not choose, so the target it was last given is not where it is (F253: it was swallowed as already issued)")
    check(settle(maker, 110000, 8.0), "and the jaw goes there")
    mark = wire.count()
    code, released = ext(bridge, "release")
    check(code == rhal.SKR_OK, "a release is accepted", str(released))
    if wire.count() >= 0:
        sent = wire.commands(mark)
        check(len(sent) == 1 and sent[0]["bits"] == [0, 11], "as release-workpiece (bit 11)", str(sent))
    mark = wire.count()
    code, outward = ext(bridge, "grip force=30 outward=1")
    check(code == rhal.SKR_OK and outward["result"] == "gripped", "a grip from the inside outwards is accepted", str(outward))
    if wire.count() >= 0:
        sent = wire.commands(mark)
        check(len(sent) == 1 and sent[0]["bits"] == [0, 7, 12] and sent[0]["force"] == 30, "with the direction bit (7) as well as the grip bit", str(sent))
    check(until(lambda: maker.get_actual_position() == round(outward["position_m"] * 1e6)), "and the position the driver answers is the one the maker's client reads")
    reached = maker.get_actual_position()
    check(reached > 120000, f"which is {reached} um, past the 120000 the module itself reports as its travel: the maker's simulator reads the travel parameters as integers (F252), and the driver reports what it is told and clamps nothing it reads")
    note("a grip force in newtons, and a workpiece: the simulator has neither (force is a percentage the module rates, and nothing is between the jaws)")
    code, said = write(bridge, cmd(0.05))
    check(code == rhal.SKR_OK and settle(maker, 50000, 8.0), "after both, a command moves the jaw again")

    section("the stop: the maker's server was handed the frame, and the latch held at once")
    bridge.close()
    maker.acknowledge()
    until(lambda: maker.get_status_bit(0) == 1)
    code, said, bridge = opened(host, port, stop_mode="fast")
    check(code == rhal.SKR_OK, "a new session opens on the module once the operator has acknowledged it (the maker's client, which is the operator here)", said)
    ext(bridge, "move position=0.0 speed=0.02")
    check(settle(maker, 0, 10.0), "the jaw is at 0")
    maker.acknowledge()
    until(lambda: maker.get_status_bit(0) == 1)
    outcome: list = []
    worker = threading.Thread(target=lambda: outcome.append(ext(bridge, "move position=0.08 speed=0.02")), daemon=True)
    worker.start()
    check(until(lambda: maker.get_actual_position() > 5000, 3.0), "a move of 80 mm at 20 mm/s is under way")
    mark = wire.count()
    began = time.monotonic()
    check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.05, f"the stop returns in {(time.monotonic() - began) * 1000:.1f} ms with the jaw moving")
    worker.join(3.0)
    check(not worker.is_alive() and outcome and outcome[0][0] == rhal.SKR_EPERM and "stop" in str(outcome[0][1]), "and ends the motion that was waiting, saying the stop took it", str(outcome))
    if wire.count() >= 0:
        check(until(lambda: len(wire.commands(mark)) >= 1, 3.0), "the maker's server was handed a frame")
        sent = wire.commands(mark)
        check(bool(sent) and sent[0]["bits"] == [], "with control bit 0 LOW and nothing else: the fast stop triggers on a low signal", str(sent))
    check(until(lambda: maker.get_status_bit(7) == 1 and maker.get_error_code() == "0xD9"), "and the maker's client reads the module in error 0xD9")
    rc, out = state(bridge)
    check(bool(out.flags & rhal.FLAG_ESTOP) and out.fault_code == 0xD9, "the driver's state says stopped, with the module's error byte")
    code, said = write(bridge, cmd(0.02))
    check(code == rhal.SKR_EPERM, "a command after the stop is refused")
    # F255, the second half: the maker's `move` loop never looks at a stop. The status says error; the jaw goes on to its target.
    settle(maker, 80000, 8.0)
    check(maker.get_actual_position() == 80000, f"the maker's simulator does not stop the jaw: it goes on to 80000 um with the module in error (it reads {maker.get_actual_position()}) (F255)")
    note("the jaw halting on a stop: the maker's simulator does not model it, so what is held is the frame the module was handed and the error it reports, never the deceleration")
    check(bridge.release_stop() == rhal.SKR_OK, "release_stop acknowledges")
    check(until(lambda: maker.get_status_bit(7) == 0 and maker.get_error_code() == "0x0"), "and the maker's client reads the error cleared")
    bridge.close()

    section("the controlled stop: the stop bit, no error")
    until(lambda: maker.get_status_bit(0) == 1)
    code, said, bridge = opened(host, port, stop_mode="controlled")
    check(code == rhal.SKR_OK, "a session opens with `controlled`", said)
    mark = wire.count()
    check(bridge.emergency_stop() == rhal.SKR_OK, "the stop is accepted")
    if wire.count() >= 0:
        check(until(lambda: len(wire.commands(mark)) >= 1, 3.0) and wire.commands(mark)[0]["bits"] == [0, 1], "and the maker's server was handed control bits 0 and 1: stop, with the fast stop not asked for", str(wire.since(mark)))
    check(until(lambda: maker.get_status_bit(13) == 1 and maker.get_status_bit(4) == 1) and maker.get_status_bit(7) == 0, "the maker's client reads the stop processed and the module not in error")
    rc, out = state(bridge)
    check(bool(out.flags & rhal.FLAG_ESTOP) and out.fault_code == 0, "the driver's latch is held all the same, without a fault code: the stop is the session's and the module has nothing wrong")
    check(write(bridge, cmd(0.02))[0] == rhal.SKR_EPERM, "and a command is refused until it is released")
    check(bridge.release_stop() == rhal.SKR_OK, "release_stop acknowledges")
    check(write(bridge, cmd(0.02))[0] == rhal.SKR_OK, "and a command is accepted again")
    settle(maker, 20000, 8.0)

    section("the connection: the maker's own `lose_connection_sec` event")
    post(host, port, "/adi/events.json", {"lose_connection_sec": 1.5})
    began = time.monotonic()
    time.sleep(0.6)
    rc, out = state(bridge)
    check(rc == rhal.SKR_ETIMEDOUT and bool(out.flags & rhal.FLAG_DEGRADED), "while the module answers 503, the driver's read is late and degraded, and says so", f"rc {rc} flags {out.flags}")
    code, said = write(bridge, cmd(0.03))
    check(code == rhal.SKR_EIO and "503" in said, "and a command is EIO, saying what the module answered: nothing is latched for an outage", said)
    time.sleep(max(0.0, 1.5 - (time.monotonic() - began)) + 0.5)
    rc, out = state(bridge)
    check(rc == rhal.SKR_OK and not out.flags & (rhal.FLAG_ESTOP | rhal.FLAG_FAULT), "when it answers again the driver reads it again, with no stop and no fault the outage did not cause", f"rc {rc} flags {out.flags}")
    check(until(lambda: maker.connected, 6.0), "and the maker's own client has reconnected too")
    check(write(bridge, cmd(0.03))[0] == rhal.SKR_OK, "a command is accepted after the outage")
    bridge.close()

    section("the other modules the maker's simulator offers")
    egk = witness(host, egk_port)
    check(egk.gripper_type == "EGK_25_EI_M_B" and until(lambda: egk.get_status_bit(7) == 1 and egk.get_error_code() == "0xEF"), "an EGK 25 comes up with error 0xEF, not the 0xD9 the EGU does (F251)", f"{egk.gripper_type} {egk.get_error_code()}")
    code, said, bridge = opened(host, egk_port)
    check(code == rhal.SKR_OK and "0xEF" in said and "does not name" in said, "the driver opens it latched and says it holds an error the maker's client does not name", said)
    low, high = egk.module_parameters["min_pos"] / 1e6, egk.module_parameters["max_pos"] / 1e6
    descriptor = json.loads(bridge.describe())
    check(descriptor["robot_slug"] == "schunk-egk-25-m-b" and descriptor["joints"][0]["limits"]["position_min_rad"] == low and descriptor["joints"][0]["limits"]["position_max_rad"] == high,
          f"its travel is the module's own, {low:g} to {high:g} m, as the maker's client decodes it")
    check(bridge.release_stop() == rhal.SKR_OK, "release_stop acknowledges it")
    check(until(lambda: egk.get_status_bit(7) == 0 and egk.get_error_code() == "0x0"), "and the maker's client reads it cleared")
    code, moved = ext(bridge, "move position=0.04 speed=0.02")
    check(code == rhal.SKR_OK and abs(moved["position_m"] - 0.04) < 1e-6, "a move inside its travel is made", str(moved))
    check(ext(bridge, "move position=0.06")[0] == rhal.SKR_EINVAL and write(bridge, cmd(0.01))[0] == rhal.SKR_EINVAL, "and one past it, on either side, is refused: the EGU's 0.12 m is not this module's")
    bridge.close()
    egk.disconnect()

    pn = witness(host, pn_port)
    until(lambda: pn.get_plc_input()[30:32] == "D9")
    raw = pn.get_plc_input()
    check(pn.gripper_type.endswith("PN_M_B") and raw[30:32] == "D9" and pn.get_error_code() == "0x0",
          f"the maker's PROFINET simulator puts error 0xD9 in byte 15 of the input frame ({raw}) while the maker's client reads error {pn.get_error_code()} from byte 12 (F249)")
    code, said, bridge = opened(host, pn_port)
    check(code == rhal.SKR_ENODEV and "PROFINET" in said and "byte 15" in said, "so the driver does not drive a PROFINET module, and says why", said)
    pn.disconnect()

    section("sessions")
    mark = wire.count()
    code, said, bridge = opened(host, port)
    check(code != rhal.SKR_OK and "not ready" in said, "a new session on the module the maker's simulator left not-ready is refused, naming it: the driver does not bring a module out of that (F254's consequence)", said)
    maker.acknowledge()
    until(lambda: maker.get_status_bit(0) == 1 and maker.get_status_bit(7) == 0)
    code, said, bridge = opened(host, port)
    check(code == rhal.SKR_OK, "and opens once the operator has acknowledged it")
    mark = wire.count()
    check(bridge.close() == rhal.SKR_OK, "a close is OK")
    if wire.count() >= 0:
        check(wire.since(mark) == [], "and writes nothing: a held workpiece is not let go by a session ending")
    maker.disconnect()

    return finish()


def finish() -> int:
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    args = sys.argv[1:]
    sys.exit(main(args[0] if args else "127.0.0.1", int(args[1]) if len(args) > 1 else 8000, int(args[2]) if len(args) > 2 else 8001, int(args[3]) if len(args) > 3 else 8002))

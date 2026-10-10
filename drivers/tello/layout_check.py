"""This driver's text and state string, held to djitellopy, which is run (DRY-COVERAGE-PLAN.md item 12.E.11).

    TELLO_PYTHON=/path/to/venv/bin/python   (make fetch-sdk: a Python with `djitellopy` 2.5.0 and its two imports, `numpy` and `av`, installed by hash)
    PYTHONPATH=../../bindings/python:. $TELLO_PYTHON layout_check.py

A Tello is a UDP socket that takes the SDK 2.0 User Guide's text commands and streams a state string, and Ryze published the guide and a sample client of it (`Tello3.py`, Python 2-era code on a download address that no longer
answers). **djitellopy** (MIT, DJITelloPy, https://github.com/damiafuentes/DJITelloPy) is the client of the same guide that everyone else uses: it writes the commands the way the guide gives them, reads the state string into a typed
dict, and decides what a reply means. It is *not* the maker's, and that is what this check is worth: a second implementation of the same document, written by someone else, run. What is compared, with djitellopy on one side and
`frames.py` on the other:

* **every command this driver sends**, byte for byte as djitellopy writes it (random distances, angles, speeds and stick values), by running djitellopy's own methods against the simulated drone and reading what arrived;
* **the state string**: strings made by `frames.encode_state`, read by djitellopy's `parse_state` and by `frames.parse_state`, field by field, with and without a mission pad;
* **what a reply means**: djitellopy's own decision about a reply (`send_control_command`), against `frames.parse_reply`;
* **the two together**: djitellopy connects to the simulated drone, enters SDK mode, takes off, moves, turns and lands, and reads its state; and this driver, opened on the same drone, reads where djitellopy put it and lands it.

**Where djitellopy and this driver differ, on purpose, each is pinned here and not papered over**: djitellopy sends a distance or an angle the guide's range does not allow and lets the drone refuse it (this driver refuses it before the
wire); it *retries a control command whose reply was lost*, up to three more times, so a motion that was flown and whose `ok` was lost is flown again (this driver never retries a motion); and it takes any reply with `ok` in it for success.

It cannot show that a real Tello answers any of this: the drone is the simulated one (F15), written from the guide and accepted by djitellopy because it was made to be.
"""

from __future__ import annotations

import contextlib
import io
import json
import logging
import random
import socket
import sys
import threading
import time

import djitellopy.tello as dj

import skynet_rhal as rhal

from skynet_tello import bridge as bridge_module, frames
from skynet_tello.bridge import TelloBridge
from skynet_tello.drone_sim import DroneSimulator

CHECKS = 0
FAILURES: list[str] = []
SCALE = 20.0


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
        except Exception:  # noqa: BLE001
            pass
        time.sleep(0.01)
    return bool(predicate())


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


STATE_PORT = free_port()


def new_sim(**kw) -> DroneSimulator:
    kw.setdefault("time_scale", SCALE)
    kw.setdefault("state_port", STATE_PORT)
    return DroneSimulator(**kw).start()


_STARTED = False


def client(sim: DroneSimulator) -> dj.Tello:
    """A djitellopy `Tello` pointed at the simulated drone. The library keeps one response socket and one state socket for the whole process, bound to the drone's own port numbers (8889 and 8890) on every address, which a drone on the same
    machine cannot share; so the first client is set up the way `Tello.__init__` sets up the first (the socket, and the library's own two receiver functions in threads), with the ports its class constants name changed to the simulated
    drone's, and nothing of the library's is replaced."""
    global _STARTED
    dj.Tello.LOGGER.setLevel(logging.CRITICAL)
    dj.Tello.CONTROL_UDP_PORT = sim.command_port
    dj.Tello.STATE_UDP_PORT = STATE_PORT
    if not _STARTED:
        dj.client_socket = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        dj.client_socket.bind(("127.0.0.1", 0))
        threading.Thread(target=dj.Tello.udp_response_receiver, daemon=True).start()
        threading.Thread(target=dj.Tello.udp_state_receiver, daemon=True).start()
        dj.threads_initialized = True
        _STARTED = True
        time.sleep(0.1)
    return dj.Tello(host="127.0.0.1", retry_count=0)


def main() -> int:
    rng = random.Random(11)

    section("commands — every command this driver sends, as djitellopy writes it")
    sim = new_sim()
    try:
        drone = client(sim)
        drone.connect()
        check(sim.commands[0] == "command" and frames.command() == "command", "`connect()` enters SDK mode with `command`")
        count = len(sim.commands)
        drone.takeoff()
        drone.land()
        check(sim.commands[count:] == [frames.takeoff(), frames.land()], "`takeoff()` and `land()`", str(sim.commands[count:]))
        drone.takeoff()
        for method, direction in ((drone.move_up, "up"), (drone.move_down, "down"), (drone.move_left, "left"), (drone.move_right, "right"), (drone.move_forward, "forward"), (drone.move_back, "back")):
            for _ in range(8):
                cm = rng.randint(*frames.MOVE_CM)
                mark = len(sim.commands)
                sim.speed = 100
                method(cm)
                if sim.commands[mark:] != [frames.move(direction, cm)]:
                    check(False, f"`move_{direction}({cm})` is {frames.move(direction, cm)!r}", str(sim.commands[mark:]))
                    break
            else:
                check(True, f"`move_{direction}(x)`: eight random distances of 20 to 500, as `{direction} x`")
            if direction in ("down",):
                drone.move_up(100)
        for method, clockwise in ((drone.rotate_clockwise, True), (drone.rotate_counter_clockwise, False)):
            for _ in range(8):
                degrees = rng.randint(*frames.ROTATE_DEG)
                mark = len(sim.commands)
                method(degrees)
                if sim.commands[mark:] != [frames.rotate(clockwise, degrees)]:
                    check(False, f"{method.__name__}({degrees})", str(sim.commands[mark:]))
                    break
            else:
                check(True, f"`{method.__name__}(x)`: eight random angles of 1 to 360, as `{'cw' if clockwise else 'ccw'} x`")
        sim.battery = 100.0
        for method, word in ((drone.flip_left, "l"), (drone.flip_right, "r"), (drone.flip_forward, "f"), (drone.flip_back, "b")):
            mark = len(sim.commands)
            method()
            check(sim.commands[mark:] == [frames.flip(word)], f"`{method.__name__}()` is `flip {word}`", str(sim.commands[mark:]))
        for _ in range(8):
            cm_s = rng.randint(*frames.SPEED_CM_S)
            mark = len(sim.commands)
            drone.set_speed(cm_s)
            if sim.commands[mark:] != [frames.speed(cm_s)]:
                check(False, f"`set_speed({cm_s})`", str(sim.commands[mark:]))
                break
        else:
            check(True, "`set_speed(x)`: eight random speeds of 10 to 100, as `speed x`")
        for _ in range(20):
            sticks = [rng.randint(-100, 100) for _ in range(4)]
            mark = len(sim.commands)
            drone.last_rc_control_timestamp = 0
            drone.send_rc_control(*sticks)
            until(lambda: len(sim.commands) > mark, 1.0)
            if sim.commands[mark:] != [frames.rc(*sticks)]:
                check(False, f"`send_rc_control{tuple(sticks)}`", str(sim.commands[mark:]))
                break
        else:
            check(True, "`send_rc_control(a, b, c, d)`: twenty random sticks, as `rc a b c d`, in the order left/right, forward/backward, up/down, yaw")
        for query, key in ((drone.query_battery, "battery"), (drone.query_speed, "speed"), (drone.query_sdk_version, "sdk"), (drone.query_serial_number, "serial"), (drone.query_wifi_signal_noise_ratio, "wifi")):
            mark = len(sim.commands)
            query()
            check(sim.commands[mark:] == [frames.read(key)], f"`{query.__name__}()` is `{frames.read(key)}`", str(sim.commands[mark:]))
        mark = len(sim.commands)
        drone.emergency()
        until(lambda: len(sim.commands) > mark, 1.0)
        check(sim.commands[mark:] == [frames.emergency()], "`emergency()` is `emergency`")
        check(frames.stop() == "stop", "`stop`, which djitellopy has no method for, is the guide's word for a hover")

        section("commands — where djitellopy sends what the guide's range does not allow")
        sim.flying, sim.motors, sim.z = True, True, 50.0
        for method, args, wire in ((drone.move_forward, (10,), "forward 10"), (drone.move_forward, (501,), "forward 501"), (drone.rotate_clockwise, (0,), "cw 0"), (drone.set_speed, (5,), "speed 5")):
            mark = len(sim.commands)
            try:
                method(*args)
            except dj.TelloException:
                pass
            check(sim.commands[mark:] == [wire], f"djitellopy sends {wire!r}, which the drone refuses", str(sim.commands[mark:]))
        check(all((raises_protocol(frames.move, "forward", 10), raises_protocol(frames.move, "forward", 501), raises_protocol(frames.rotate, True, 0), raises_protocol(frames.speed, 5))), "and this driver refuses all four before the wire")
        sim.flying = False

        section("commands — a reply that is lost")
        sim.flying, sim.motors, sim.z, sim.x, sim.y, sim.yaw = True, True, 100.0, 0.0, 0.0, 0.0
        sim.speed = 100
        sim.drop_replies = 1
        retrying = dj.Tello(host="127.0.0.1", retry_count=3)
        retrying.RESPONSE_TIMEOUT = 1
        mark = len(sim.commands)
        with contextlib.suppress(dj.TelloException):
            retrying.send_control_command("forward 100", timeout=1)
        until(lambda: sim.commands[mark:].count("forward 100") == 2 and not sim._busy, 5.0)    # noqa: SLF001
        check(sim.commands[mark:].count("forward 100") == 2 and sim.position[0] > 150, "djitellopy sends a control command again when its reply is lost, and the drone flew `forward 100` twice", f"{sim.commands[mark:]} x={sim.position[0]:.0f}")
        sim.flying = False
    finally:
        sim.stop()

    section("state — one string, read by djitellopy and by this driver")
    different = 0
    for trial in range(60):
        original = frames.State(pitch=rng.randint(-90, 90), roll=rng.randint(-90, 90), yaw=rng.randint(-180, 180), vgx=rng.randint(-100, 100), vgy=rng.randint(-100, 100), vgz=rng.randint(-100, 100), templ=rng.randint(0, 90),
                                temph=rng.randint(0, 90), tof=rng.randint(0, 3000), h=rng.randint(0, 3000), bat=rng.randint(0, 100), baro=round(rng.uniform(-100, 100), 2), time=rng.randint(0, 5000),
                                agx=round(rng.uniform(-1000, 1000), 2), agy=round(rng.uniform(-1000, 1000), 2), agz=round(rng.uniform(-1000, 1000), 2), pad=(rng.randint(-1, 8), rng.randint(-50, 50), rng.randint(-50, 50), rng.randint(0, 50)) if trial % 2 else None)
        text = frames.encode_state(original)
        theirs = dj.Tello.parse_state(text)
        ours = frames.parse_state(text)
        mine = {name: getattr(ours, name) for name in frames.STATE_INTS + frames.STATE_FLOATS}
        if ours.pad is not None:
            mine.update(dict(zip(("mid", "x", "y", "z"), ours.pad)))
        if theirs != mine or any(type(theirs[name]) is not type(value) for name, value in mine.items()):
            different += 1
            if different == 1:
                check(False, "the same fields, with the same values and types", f"{text!r}: djitellopy {theirs}, this driver {mine}")
    check(different == 0, "sixty random state strings, half of them with a mission pad, give the same fields and the same values in both, with the same types (whole numbers, and `baro` and the accelerations as floats)")
    real = "mid:-1;x:0;y:0;z:0;mpry:0,0,0;pitch:3;roll:-2;yaw:-12;vgx:0;vgy:0;vgz:0;templ:62;temph:64;tof:10;h:0;bat:87;baro:-80.25;time:0;agx:-1.00;agy:2.00;agz:-999.00;\r\n"
    theirs, ours = dj.Tello.parse_state(real), frames.parse_state(real)
    check(theirs["mpry"] == "0,0,0" and ours.extra == {"mpry": "0,0,0"} and theirs["mid"] == ours.pad[0] == -1, "the field newer than the guide (`mpry`) is text in both, and a pad that is not detected is `mid:-1`")
    check(theirs["bat"] == ours.bat == 87 and theirs["yaw"] == ours.yaw == -12 and theirs["baro"] == ours.baro == -80.25, "a string as a Tello is described to send it")
    missing = real.replace("h:0;", "")
    check("h" not in dj.Tello.parse_state(missing) and raises_protocol(frames.parse_state, missing), "a string with a field missing: djitellopy keeps what is there and this driver refuses it (a reading with no height is not a reading)")
    check(dj.Tello.parse_state("ok") == {} and raises_protocol(frames.parse_state, "ok"), "`ok` where a state was expected: an empty dict for djitellopy, a refusal here")
    check(dj.Tello.parse_state(real.replace("h:0", "h:tall")).get("h") is None and raises_protocol(frames.parse_state, real.replace("h:0", "h:tall")), "a field that is not a number: left out by djitellopy and refused here")
    check(dj.Tello.parse_state(real.replace("vgy:0", "vgy0")).get("vgy") is None and raises_protocol(frames.parse_state, real.replace("vgy:0", "vgy0")), "the guide's own `vgy%d` with no colon: the field is lost to both")

    section("replies — what djitellopy makes of one, and what this driver makes of it")
    probe = dj.Tello(host="127.0.0.1", retry_count=0)
    table = (("ok", True), ("OK", True), ("ok\r\n", True), ("error", False), ("error Not joystick", False), ("Unknown command: foo", False), ("out of range", False), ("87", False), ("", False), ("okay", True), ("error ok", True))
    for reply, accepted in table:
        probe.send_command_with_return = lambda command, timeout=7, reply=reply: reply    # noqa: B023 - the library's own classifier, fed a canned reply
        try:
            probe.send_control_command("takeoff")
            success = True
        except dj.TelloException:
            success = False
        check(success == accepted, f"djitellopy takes {reply!r} for {'success' if accepted else 'a failure'}")
    agreed = sum(1 for reply, accepted in table if (frames.parse_reply(reply.encode()).kind == "ok") == accepted)
    check(agreed == len(table) - 2, "the two agree on all the replies but the two with `ok` inside another word", f"{agreed} of {len(table)}")

    section("the two together — djitellopy moves the simulated drone and this driver reads and lands it")
    sim = new_sim()
    try:
        drone = client(sim)
        drone.connect()
        drone.takeoff()
        check(until(lambda: drone.get_height() > 60, 3.0) and abs(drone.get_height() - sim.z) < 12, "djitellopy takes the simulated drone up and reads its height back", f"{drone.get_height()} {sim.z}")
        sim.speed = 100
        drone.move_forward(100)
        drone.rotate_clockwise(90)
        check(abs(sim.position[0] - 100) < 2 and abs(sim.position[3] - 90) < 2, "and flies it forward and turns it", str(sim.position))
        check(until(lambda: drone.get_battery() == int(sim.battery), 3.0) and drone.get_yaw() == 90, "its battery and yaw are the drone's", f"{drone.get_battery()} {sim.battery} {drone.get_yaw()}")
        height, battery = drone.get_height(), drone.get_battery()
        port = free_port()
        sim.state_port = port
        bridge = TelloBridge()
        with contextlib.redirect_stderr(io.StringIO()) as said:
            code = bridge.open(json.dumps({"host": "127.0.0.1", "port": sim.command_port, "state_port": port, "simulator": True, "stop_mode": "hover", "keepalive_s": 0, "reply_timeout_ms": 2000}))
        check(code == rhal.SKR_OK, "this driver opens on the same drone", said.getvalue())
        out = rhal.State()
        until(lambda: bridge.read_state(out, 0) == rhal.SKR_OK and abs(out.base_pose[2] * 100 - sim.z) < 12, 3.0)
        check(abs(out.base_pose[2] * 100 - height) < 12 and abs(out.base_pose[2] * 100 - sim.z) < 12, "and reads the height djitellopy had read", f"{out.base_pose[2] * 100} {height}")
        body = json.loads(bridge.extension(bridge_module.TOKEN, b"status")[1])
        check(body["battery_percent"] in (battery, battery - 1) and body["yaw_deg"] == 90 and body["serial"] == drone.query_serial_number(), "its battery, its yaw and its serial number are the ones djitellopy read", str(body)[:200])
        code, text = bridge.extension(bridge_module.TOKEN, b"land")
        check(code == rhal.SKR_OK and not sim.flying and sim.z < 1, "and lands it")
        bridge.close()
    finally:
        sim.stop()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAILED: {failure}")
    return 1 if FAILURES else 0


def raises_protocol(function, *args) -> bool:
    try:
        function(*args)
    except frames.ProtocolError:
        return True
    return False


if __name__ == "__main__":
    sys.exit(main())

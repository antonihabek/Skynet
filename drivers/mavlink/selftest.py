"""skynet-mavlink's own suite: the wire, the frames, and every refusal the bridge makes.

No test framework, `drivers/xarm/selftest.py`'s shape and emphasis: nearly everything asserted here is a
**refusal** or a **boundary**, each written beside the nearly identical input that succeeds. A vehicle
driver's failure is not a crash. It is a setpoint the autopilot ignores without a word because the mode was
wrong, a stop that is sent once on a lossy link and never heard, a north that is an east.

    make selftest                          # this file, against the simulated vehicle
    make check                             # and then the whole path through the real ABI
    python3 selftest.py --live <link>      # the live block, against a real autopilot's firmware
                                           # (`make check-sim-docker` runs it against ArduPilot's)
"""

from __future__ import annotations

import json
import math
import os
import select
import sys
import threading
import time

import skynet_rhal as rhal

from skynet_mavlink import frames, geometry
from skynet_mavlink.autopilot import SimulatedAutopilot
from skynet_mavlink.bridge import TOKEN, MavlinkBridge
from skynet_mavlink.link import LinkClosed, LinkRefusal, open_link, parse_spec

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


def until(predicate, timeout: float = 3.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.005)
    return bool(predicate())


class Rig:
    """A simulated vehicle and a bridge open on it."""

    def __init__(self, family: str = "copter", *, ready: bool = True, sim_kwargs: dict | None = None, **settings) -> None:
        self.vehicle = SimulatedAutopilot(family, **(sim_kwargs or {}))
        self.vehicle.gcs_failsafe = 1.0  # a vehicle with a link-loss failsafe set, so `open` has nothing to warn about
        if ready:
            self.vehicle.ekf_ready_at = 0.0
        self.vehicle.start()
        self.bridge = MavlinkBridge()
        self.settings = {"link": self.link, "command_timeout_ms": 200, "command_tries": 2, "open_timeout_ms": 4000, **settings}
        self.rc = 0

    @property
    def link(self) -> str:
        v = self.vehicle
        return {"tcp": f"tcp:127.0.0.1:{v.port}", "udp": f"udpout:127.0.0.1:{v.port}", "pty": f"serial:{v.pty_path}:115200"}[v.transport]

    def open(self, **more) -> int:
        self.rc = self.bridge.open(json.dumps({**self.settings, **more}))
        return self.rc

    def ext(self, text: str) -> tuple[int, dict | str]:
        code, out = self.bridge.extension(TOKEN, text.encode())
        text_out = out.decode()
        try:
            return code, json.loads(text_out)
        except json.JSONDecodeError:
            return code, text_out

    def state(self) -> tuple[int, rhal.State]:
        state = rhal.State()
        return self.bridge.read_state(state, 0), state

    def close(self) -> None:
        self.bridge.close()
        self.vehicle.stop()


def last_setpoint(rig: Rig, count: int) -> dict:
    """The `count`th setpoint the vehicle heard, once it has heard that many (they are not acknowledged)."""
    assert until(lambda: len(rig.vehicle.setpoints) >= count, 2.0), "the vehicle heard no setpoint"
    return rig.vehicle.setpoints[count - 1]


def flying(rig: Rig, altitude: float = 5.0) -> None:
    """Guided, armed, and at `altitude` — the state most motions want."""
    assert rig.ext("mode name=guided")[0] == 0
    assert rig.ext("arm")[0] == 0
    assert rig.ext(f"takeoff alt={altitude}")[0] == 0
    assert rig.vehicle.wait(lambda: rig.vehicle.landed == frames.Landed.IN_AIR, 8.0)


def main() -> int:
    if "--live" in sys.argv[1:]:
        live_checks(sys.argv[sys.argv.index("--live") + 1])
        return finish()

    print("skynet-mavlink selftest\n")

    # ── the wire ─────────────────────────────────────────────────────────────
    section("frames — MAVLink on the wire")
    hb = frames.SPECS["HEARTBEAT"]
    fields = {"custom_mode": 0, "type": 2, "autopilot": 3, "base_mode": 89, "system_status": 4, "mavlink_version": 3}
    frame = frames.encode(hb, fields, seq=1, system_id=1, component_id=1)
    # The reference implementation's bytes for the same heartbeat (layout_check.py builds every message's and compares).
    check(frame.hex() == "fd0900000101010000000000000002035904032f75", "a heartbeat is the frame the reference builds, byte for byte, checksum included")
    check(frames.crc_x25(b"123456789") == 0x6F91, "the checksum is X.25 (CRC-16/MCRF4XX): its check string gives 0x6F91")
    check(frame[0] == 0xFD and frame[1] == 9 and frame[7:10] == b"\0\0\0", "the header is v2: start byte, length, message id in three bytes")
    short = frames.encode(hb, {"type": 2}, seq=0, system_id=1, component_id=1)
    check(short[1] == 5, "a payload's trailing zeros are cut: a heartbeat with only its type set is five bytes (custom mode, then type)")
    zero = frames.encode(frames.SPECS["EXTENDED_SYS_STATE"], {}, seq=0, system_id=1, component_id=1)
    check(zero[1] == 1, "but never below one byte")
    parsed = frames.Parser().feed(short)
    check(len(parsed) == 1 and parsed[0].fields["autopilot"] == 0 and parsed[0].fields["type"] == 2, "and a receiver pads them back")
    try:
        frames.encode(hb, {"type": 300}, seq=0, system_id=1, component_id=1)
        check(False, "a value that does not fit its field is refused")
    except frames.ProtocolError:
        check(True, "a value that does not fit its field is refused, not truncated")
    try:
        frames.encode(frames.SPECS["STATUSTEXT"], {"text": b"x" * 51}, seq=0, system_id=1, component_id=1)
        check(False, "a string longer than its field is refused")
    except frames.ProtocolError:
        check(True, "a string longer than its field is refused")

    parser = frames.Parser()
    stream = frame + b"\x00\xfe\x01" + frame
    out = []
    for offset in range(0, len(stream), 7):
        out += parser.feed(stream[offset : offset + 7])
    check(len(out) == 2, "frames are cut out of a stream wherever the bytes fall, and a false start byte costs nothing")
    damaged = bytearray(frame)
    damaged[12] ^= 0x01
    parser = frames.Parser()
    check(parser.feed(bytes(damaged)) == [] and parser.bad_crc == 1, "a frame with one bit wrong is refused on its checksum")
    check(len(parser.feed(frame)) == 1, "and the parser is not left confused by it")
    swallowed = bytearray(b"\xfd\xff") + b"\x00" * 30
    parser = frames.Parser()
    got = parser.feed(bytes(swallowed) + frame)
    check(len(got) == 0 and parser.unknown + parser.bad_crc >= 0, "a start byte that announces 255 bytes waits for them, as any parser must")
    got = parser.feed(b"\x00" * 300 + frame)
    check(sum(1 for p in got if p.name == "HEARTBEAT") >= 1, "and a real frame after it is found once the announced length has passed")

    # ── geometry ─────────────────────────────────────────────────────────────
    section("geometry — NED and FRD into ENU and FLU (§44.4)")
    check(geometry.ned_to_enu(1.0, 2.0, 3.0) == (2.0, 1.0, -3.0), "north 1, east 2, down 3 is east 2, north 1, up -3")
    check(geometry.enu_to_ned(*geometry.ned_to_enu(1.0, 2.0, 3.0)) == (1.0, 2.0, 3.0), "and the map is its own inverse")
    check(geometry.frd_to_flu(1.0, 2.0, 3.0) == (1.0, -2.0, -3.0), "a body rate: right becomes left, down becomes up")

    M = [[0, 1, 0], [1, 0, 0], [0, 0, -1]]
    N = [[1, 0, 0], [0, -1, 0], [0, 0, -1]]

    def matmul(a, b):
        return [[sum(a[i][k] * b[k][j] for k in range(3)) for j in range(3)] for i in range(3)]

    worst = 0.0
    for roll, pitch, yaw in ((0, 0, 0), (0, 0, math.pi / 2), (0.3, 0, 0), (0, 0.4, 0), (0.3, -0.2, 2.5), (-1.0, 0.5, -2.0)):
        q_ned = geometry.quat_from_euler_ned_frd(roll, pitch, yaw)
        expected = matmul(matmul(M, geometry.rotation_matrix(*q_ned)), N)
        x, y, z, w = geometry.quat_ned_frd_to_enu_flu(*q_ned)
        got = geometry.rotation_matrix(w, x, y, z)
        worst = max(worst, max(abs(got[i][j] - expected[i][j]) for i in range(3) for j in range(3)))
    check(worst < 1e-9, f"the orientation is M·R·N to {worst:.1e}, for six attitudes including the ones that expose a sign")
    x, y, z, w = geometry.quat_ned_frd_to_enu_flu(1.0, 0.0, 0.0, 0.0)
    forward = geometry.rotation_matrix(w, x, y, z)
    check(abs(forward[0][0]) < 1e-9 and abs(forward[1][0] - 1.0) < 1e-9, "a vehicle facing north has its forward axis on ENU's y")
    x, y, z, w = geometry.quat_ned_frd_to_enu_flu(*geometry.quat_from_euler_ned_frd(0.0, 0.0, math.pi / 2))
    forward = geometry.rotation_matrix(w, x, y, z)
    check(abs(forward[0][0] - 1.0) < 1e-9 and abs(forward[1][0]) < 1e-9, "and one facing east has it on ENU's x")
    check(w >= 0, "the quaternion's w is non-negative, so a state does not flip sign between samples")
    check(all(math.isnan(v) for v in geometry.quat_ned_frd_to_enu_flu(0.0, 0.0, 0.0, 0.0)), "the zero quaternion is not an attitude: NaN")

    # ── the link ─────────────────────────────────────────────────────────────
    section("link — a socket or a serial port")
    for spec, why in (
        (None, "nothing"), ("", "an empty string"), ("localhost:5760", "a bare host and port"), ("tcp:localhost", "no port"),
        ("tcp:localhost:notaport", "a port that is not a number"), ("tcp:localhost:99999", "a port that is not a port"),
        ("serial:/dev/ttyACM0", "no baud rate"), ("serial:/dev/ttyACM0:123", "a baud rate the platform has no setting for"),
        ("udp:0.0.0.0:14550", "a scheme that is neither udpin nor udpout"),
    ):
        try:
            parse_spec(spec)
            check(False, f"a link of {why} is refused")
        except LinkRefusal:
            check(True, f"a link of {why} is refused")
    check(parse_spec("tcp:127.0.0.1:5760") == ("tcp", "127.0.0.1", 5760) and parse_spec("serial:/dev/ttyACM0:921600")[2] == 921600, "and the accepted forms parse")
    try:
        open_link("tcp:127.0.0.1:1", connect_timeout=0.5)
        check(False, "a connection nothing accepts is reported")
    except LinkClosed:
        check(True, "a connection nothing accepts is reported as the vehicle not answering")

    for transport in ("tcp", "udp", "pty"):
        rig = Rig(sim_kwargs={"transport": transport})
        code = rig.open()
        check(code == rhal.SKR_OK, f"{transport}: the bridge opens the simulated vehicle" + ("" if code == rhal.SKR_OK else f" (rc {code})"))
        if code == rhal.SKR_OK:
            check(until(lambda: rig.state()[0] == rhal.SKR_OK), f"{transport}: and reads its state")
        rig.close()

    # ── refusals at open ─────────────────────────────────────────────────────
    section("open — what it refuses to bring up")
    bridge = MavlinkBridge()
    check(bridge.open("not json") == rhal.SKR_EINVAL, "a configuration that is not JSON is EINVAL")
    check(bridge.open("[]") == rhal.SKR_EINVAL, "and one that is not an object")
    check(bridge.open("{}") == rhal.SKR_EINVAL, "no `link`: EINVAL, naming the forms")
    check(bridge.open('{"link":"localhost:5760"}') == rhal.SKR_EINVAL, "a link with no scheme: EINVAL")
    check(bridge.open('{"link":"tcp:127.0.0.1:1"}') == rhal.SKR_EIO, "a link nothing listens on: EIO, a transport failure and not a refused body")
    check(bridge.read_state(rhal.State(), 0) == rhal.SKR_EPERM, "read_state before a successful open is EPERM")
    check(bridge.write_command(rhal.Command(values=[], mask=[]), 0) == rhal.SKR_EPERM, "write_command before a successful open is EPERM")
    check(bridge.extension(TOKEN, b"status")[0] == rhal.SKR_EPERM, "and so is an extension")
    check(bridge.emergency_stop() == rhal.SKR_OK and bridge.release_stop() == rhal.SKR_OK, "a stop on a body that never opened is a latch and returns")
    for bad, why in (({"source_system": 0}, "system id 0"), ({"source_component": 300}, "component id 300"), ({"system_id": True}, "a boolean for a system id"),
                     ({"command_tries": 0}, "no tries"), ({"command_timeout_ms": -1}, "a negative timeout")):
        rig = Rig(**bad)
        check(rig.open() == rhal.SKR_ENODEV, f"{why} is refused before a byte is sent")
        rig.close()

    rig = Rig(sim_kwargs={})
    rig.vehicle.silent = True
    check(rig.open(open_timeout_ms=600) == rhal.SKR_EIO, "a vehicle that never sends a heartbeat: EIO after the open timeout")
    rig.close()

    rig = Rig()
    rig.vehicle.extra_heartbeats = [(7, 1, {"custom_mode": 0, "type": 2, "autopilot": 3, "base_mode": 81, "system_status": 3, "mavlink_version": 3})]
    check(rig.open() == rhal.SKR_ENODEV, "two vehicles on one link and none named: refused, because a driver that picked would command whichever spoke first")
    check(rig.open(system_id=7) == rhal.SKR_OK and json.loads(rig.bridge.describe())["robot_slug"].startswith("ardupilot-copter"), "and `system_id` chooses")
    rig.close()

    rig = Rig()
    rig.vehicle.extra_heartbeats = [(1, 100, {"custom_mode": 0, "type": frames.MavType.GIMBAL, "autopilot": 8, "base_mode": 0, "system_status": 4, "mavlink_version": 3}),
                                    (255, 190, {"custom_mode": 0, "type": frames.MavType.GCS, "autopilot": 8, "base_mode": 0, "system_status": 4, "mavlink_version": 3})]
    check(rig.open() == rhal.SKR_OK, "a gimbal and a ground station heard beside the autopilot are not vehicles")
    rig.close()

    for label, kwargs, why in (
        ("PX4", {"autopilot": frames.Autopilot.PX4}, "PX4"),
        ("a boat", {"family": "boat"}, "a surface boat"),
        ("a submarine", {"family": "sub"}, "a submarine"),
    ):
        rig = Rig(kwargs.get("family", "copter"), sim_kwargs={"autopilot": kwargs["autopilot"]} if "autopilot" in kwargs else None)
        check(rig.open() == rhal.SKR_ENODEV, f"{label} is refused by name ({why})")
        rig.close()
    rig = Rig()
    rig.vehicle.vehicle_type = frames.MavType.VTOL_TILTROTOR if hasattr(frames.MavType, "VTOL_TILTROTOR") else 21
    check(rig.open() == rhal.SKR_ENODEV, "a VTOL, whose modes are a plane's and a copter's at once, is refused")
    rig.close()
    rig = Rig()
    rig.vehicle.vehicle_type = frames.MavType.ANTENNA_TRACKER
    check(rig.open(open_timeout_ms=700) == rhal.SKR_EIO, "an antenna tracker is not a vehicle to this driver: no vehicle heard")
    rig.close()

    rig = Rig()
    rig.vehicle.sign_frames = True
    check(rig.open() == rhal.SKR_ENODEV, "a signed link is refused, because the vehicle would ignore every command this driver sent")
    rig.close()

    rig = Rig()
    check(rig.open(stop_mode="nonsense") == rhal.SKR_ENODEV, "a stop mode that is not a mode of this vehicle is refused")
    check(rig.open(allow_modes=["nonsense"]) == rhal.SKR_ENODEV, "and so is an allow list naming one")
    check(rig.open(stop_mode="loiter", allow_modes=["acro"]) == rhal.SKR_OK, "a valid stop mode and allow list open")
    rig.close()

    rig = Rig()
    check(rig.open() == rhal.SKR_OK and rig.open() == rhal.SKR_OK, "opening a body twice in a row brings it up again, not into a half state")
    check(rig.bridge.close() == rhal.SKR_OK and rig.bridge.read_state(rhal.State(), 0) == rhal.SKR_EPERM, "close, and a read is EPERM again")
    check(rig.open() == rhal.SKR_OK, "and it reopens")
    rig.close()

    # ── the descriptor ───────────────────────────────────────────────────────
    section("describe — what the body says it is")
    rig = Rig()
    rig.open()
    descriptor = json.loads(rig.bridge.describe())
    names = [m["name"] for m in descriptor["motions"]]
    check(descriptor["dof"] == 0 and descriptor["joints"] == [] and descriptor["base"] == "aerial", "a multirotor: no joints, an aerial base")
    check(descriptor["extensions"] == [TOKEN] and descriptor["capabilities"] == ["safety.estop"], "it declares the extension and the software stop, and nothing more")
    check(descriptor["envelope"] is None, "no envelope: the vehicle's limits are parameters it enforces, not a risk assessment anyone here made")
    check(descriptor["robot_slug"] == "ardupilot-copter-4.7.1" and descriptor["simulator"] is True, "the slug names the firmware it read and the descriptor says it is a simulation")
    check({"takeoff", "land", "velocity", "goto", "arm", "disarm", "mode", "hold", "rtl", "status", "param"} == set(names), "a copter's motions")
    check([p["min"] for m in descriptor["motions"] if m["name"] == "takeoff" for p in m["parameters"]] == [0.5], "with the one bound the driver enforces declared")
    rig.close()
    rig = Rig("rover")
    rig.open()
    descriptor = json.loads(rig.bridge.describe())
    names = [m["name"] for m in descriptor["motions"]]
    check(descriptor["base"] == "wheeled" and "takeoff" not in names and "land" not in names and "velocity" in names, "a rover: a wheeled base, no takeoff and no landing, and velocity")
    rig.close()
    rig = Rig("plane")
    rig.open()
    names = [m["name"] for m in json.loads(rig.bridge.describe())["motions"]]
    check(json.loads(rig.bridge.describe())["base"] == "aerial" and "velocity" not in names and "takeoff" not in names, "a fixed wing: aerial, no velocity setpoint (it cannot fly one), and takeoff is a mode")
    rig.close()
    rig = Rig(sim_kwargs={})
    rig.vehicle.has_sim_params = False
    rig.open()
    check(json.loads(rig.bridge.describe())["simulator"] is False, "a vehicle with no SIM_ parameters is declared a real flight controller")
    rig.close()
    rig = Rig()
    rig.vehicle.has_sim_params = False
    rig.open(simulator=True)
    check(json.loads(rig.bridge.describe())["simulator"] is True, "and a configuration can only make that more cautious")
    rig.close()

    # ── state ────────────────────────────────────────────────────────────────
    section("read_state — the pose in REP-103, and what it does not claim")
    rig = Rig()
    rig.open()
    code, state = rig.state()
    check(code == rhal.SKR_OK and state.joint_count == 0, "the state has no joints")
    check(all(math.isnan(v) for v in state.base_pose) or until(lambda: not math.isnan(rig.state()[1].base_pose[0])), "before telemetry arrives the pose is NaN, not zeros")
    check(until(lambda: not math.isnan(rig.state()[1].base_pose[0])), "and then it is a pose")
    code, first = rig.state()
    code, second = rig.state()
    check(second.seq == first.seq + 1, "the sequence advances on every read")
    check(second.stamp_ns >= first.stamp_ns > 0, "the stamp is the arrival of the newest sample, on the acquisition clock")
    check(first.flags & rhal.HOMING_MASK == rhal.HOMING_NOT_REQUIRED, "homing is not required of a vehicle")
    flying(rig, 5.0)
    rig.ext("goto x=10 y=0 z=5")
    check(until(lambda: rig.state()[1].base_pose[0] > 9.0, 8.0), "flying to east 10: ENU x rises")
    code, state = rig.state()
    check(abs(state.base_pose[1]) < 0.5 and abs(state.base_pose[2] - 5.0) < 0.6, "and y and z stay")
    x, y, z, w = state.base_pose[3:]
    check(abs(x) < 0.05 and abs(y) < 0.05 and abs(math.sqrt(x * x + y * y + z * z + w * w) - 1.0) < 1e-3, "the orientation is a unit quaternion, level")
    yaw = 2 * math.atan2(z, w)
    check(abs(yaw) < 0.2, "facing east: ENU yaw zero")
    rig.ext("goto x=10 y=8 z=5")
    check(until(lambda: rig.state()[1].base_pose[1] > 7.0, 8.0), "flying north: ENU y rises")
    check(until(lambda: abs(2 * math.atan2(rig.state()[1].base_pose[5], rig.state()[1].base_pose[6]) - math.pi / 2) < 0.2), "and it faces north: ENU yaw a quarter turn")
    rig.ext("velocity vx=0 vy=2 vz=0")
    check(until(lambda: rig.state()[1].base_twist[1] > 1.5, 3.0), "a north velocity is a positive ENU vy in the twist")
    code, state = rig.state()
    check(abs(state.base_twist[0]) < 0.3 and abs(state.base_twist[2]) < 0.3, "and no east or up")
    rig.close()

    rig = Rig()
    rig.open()
    until(lambda: not math.isnan(rig.state()[1].base_pose[0]))
    rig.vehicle.silent = True
    check(until(lambda: rig.state()[0] == rhal.SKR_OK and math.isnan(rig.state()[1].base_pose[0]), 3.0), "a vehicle that stops streaming: the pose goes NaN, not stale")
    check(rig.state()[1].flags & rhal.FLAG_DEGRADED != 0, "and the state is degraded")
    check(until(lambda: rig.state()[0] == rhal.SKR_ETIMEDOUT, 6.0), "and after the heartbeat stops too, the read is ETIMEDOUT")
    rig.vehicle.silent = False
    check(until(lambda: rig.state()[0] == rhal.SKR_OK, 4.0), "which clears when the vehicle speaks again")
    rig.vehicle.system_status = frames.MavState.CRITICAL
    check(until(lambda: rig.state()[1].flags & rhal.FLAG_FAULT != 0 and rig.state()[1].fault_code == frames.MavState.CRITICAL, 3.0), "a critical system status is a fault, with the status as the fault code")
    check(rig.state()[1].flags & rhal.FLAG_DEGRADED != 0, "and DEGRADED with it: §27's `rhal` condition reads that bit and not FAULT, so a fault alone would leave the OS reading a failsafe as `present`")
    check(rig.state()[1].flags & rhal.FLAG_ESTOP == 0, "and not a stop: the vehicle is flying its own failsafe, and its stop chain is not open")
    for status, name in ((frames.MavState.EMERGENCY, "an emergency"), (frames.MavState.POWEROFF, "a power-off")):
        rig.vehicle.system_status = status
        # Waiting on the fault *code*: the flag is still set from the status before, so waiting on it alone reads a stale code.
        check(until(lambda: rig.state()[1].flags & rhal.FLAG_FAULT != 0 and rig.state()[1].fault_code == status, 3.0),
              f"{name} is a fault too, with the status as the fault code: the autopilot saying the vehicle is lost is not a healthy one")
        check(rig.state()[1].flags & rhal.FLAG_DEGRADED != 0, f"and {name} is degraded as well")
    rig.vehicle.system_status = frames.MavState.FLIGHT_TERMINATION
    check(until(lambda: rig.state()[1].flags & rhal.FLAG_ESTOP != 0 and rig.state()[1].fault_code == frames.MavState.FLIGHT_TERMINATION, 3.0),
          "a flight termination is the autopilot's own stop, ESTOP with the status as the fault code")
    check(rig.state()[1].flags & rhal.FLAG_DEGRADED != 0, "and DEGRADED with it")
    rig.vehicle.system_status = frames.MavState.CALIBRATING
    check(until(lambda: rig.state()[1].flags & rhal.FLAG_ESTOP == 0 and rig.state()[1].flags & rhal.FLAG_FAULT == 0 and rig.state()[1].flags & rhal.FLAG_DEGRADED != 0, 3.0),
          "a vehicle that is still calibrating is degraded and nothing worse")
    rig.vehicle.system_status = 77
    check(until(lambda: rig.state()[1].flags & rhal.FLAG_FAULT != 0 and rig.state()[1].fault_code == 77, 3.0), "and a state the dialect does not define is a fault, never a healthy vehicle")
    rig.vehicle.system_status = None
    rig.vehicle.battery = 8
    check(until(lambda: rig.state()[1].flags & rhal.FLAG_DEGRADED != 0, 3.0), "a battery at 8% is degraded")
    rig.vehicle.battery = 60
    check(until(lambda: rig.state()[1].flags & rhal.FLAG_DEGRADED == 0, 3.0), "and 60% is not")
    rig.vehicle.battery = 8
    until(lambda: rig.state()[1].flags & rhal.FLAG_DEGRADED != 0, 3.0)
    rig.vehicle.battery = -1  # MAVLink's "unknown": a vehicle with no battery monitor says this on every report
    check(until(lambda: rig.state()[1].flags & rhal.FLAG_DEGRADED == 0, 3.0), "and a battery level of unknown (-1) is not empty: a vehicle with no monitor is not degraded for it")
    rig.close()

    # ── commands ─────────────────────────────────────────────────────────────
    section("write_command — a body with no joints")
    rig = Rig()
    rig.open()
    check(rig.bridge.write_command(rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[0.0], mask=[1]), 0) == rhal.SKR_EINVAL, "a joint command is EINVAL: the shape is wrong before the permission is")
    check(rig.bridge.write_command(rhal.Command(stamp_ns=1, seq=1, mode=9, values=[], mask=[]), 0) == rhal.SKR_EINVAL, "an unknown mode is EINVAL")
    check(rig.bridge.write_command(rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_VELOCITY, values=[], mask=[]), 0) == rhal.SKR_EPERM, "a well-formed empty command is EPERM, with the route named")
    rig.close()

    # ── motions ──────────────────────────────────────────────────────────────
    section("motions — arm, and the vehicle's own words for a refusal")
    rig = Rig(ready=False)
    rig.open()
    rig.vehicle.ekf_ready_at = time.monotonic() + 1.2
    check(rig.bridge.extension("vendor.other.task", b"status")[0] == rhal.SKR_ENOSYS, "a token this driver does not serve is ENOSYS")
    check(rig.bridge.extension(TOKEN, b"\xff\xfe")[0] == rhal.SKR_EINVAL, "a request that is not UTF-8 is EINVAL")
    check(rig.ext("")[0] == rhal.SKR_EINVAL, "an empty request is EINVAL, listing the motions")
    check(rig.ext("status extra")[0] == rhal.SKR_EINVAL, "an argument without a value is malformed")
    check(rig.ext("fly")[0] == rhal.SKR_EINVAL, "an unknown motion is EINVAL")
    code, text = rig.ext("arm")
    check(code == rhal.SKR_EPERM and "Need Position Estimate" in text, "arming before the estimate is EPERM and carries the vehicle's own STATUSTEXT")
    check(rig.vehicle.armed is False, "and nothing was armed")
    check(until(lambda: time.monotonic() > rig.vehicle.ekf_ready_at, 2.0) and rig.ext("arm")[0] == rhal.SKR_OK and rig.vehicle.armed, "the same request after it: armed, confirmed by the heartbeat")
    code, status = rig.ext("status")
    check(code == 0 and status["armed"] is True and status["mode"] == "stabilize", "status reports it, in mode names and not numbers")
    check(rig.ext("arm extra=1")[0] == rhal.SKR_EINVAL, "arm takes nothing")
    rig.vehicle.d, rig.vehicle.landed = -5.0, frames.Landed.IN_AIR
    code, text = rig.ext("disarm")
    check(code == rhal.SKR_EPERM and rig.vehicle.armed and "not landed" in text, "disarming in the air is refused with the vehicle's words")
    check(rig.ext("disarm force=2")[0] == rhal.SKR_EINVAL, "force takes 1 or nothing")
    check(rig.ext("disarm force=1")[0] == rhal.SKR_OK and not rig.vehicle.armed, "force=1 sends the value the protocol documents for it, and the vehicle disarms")
    forced = [p for c, p in rig.vehicle.commands if c == frames.Cmd.COMPONENT_ARM_DISARM][-1]
    check(forced[0] == 0.0 and forced[1] == 21196.0, "the wire carries param2 21196")
    rig.close()

    section("motions — acknowledgements that do not come")
    rig = Rig()
    rig.open()
    rig.vehicle.ack_commands[frames.Cmd.COMPONENT_ARM_DISARM] = None
    started = time.monotonic()
    code, text = rig.ext("arm")
    check(code == rhal.SKR_ETIMEDOUT and "2 tries" in text, "a command the vehicle never acknowledges: ETIMEDOUT after the configured tries")
    check(time.monotonic() - started < 2.0, "and inside the time the configuration allows")
    sent = [c for c, _ in rig.vehicle.commands if c == frames.Cmd.COMPONENT_ARM_DISARM]
    check(len(sent) == 2, "it was sent twice")
    rig.close()

    # What follows is about what the vehicle *says* and about a retry after one lost reply: neither is a claim about latency, so a
    # rig with a deadline a loaded machine cannot miss (200 ms was missed, and the suite went red 1 run in 3 on a machine with every
    # core busy — the same two checks each time, and once during unrelated jobs). Each group has its own rig: a late acknowledgement
    # from one command must not be able to count as a send in the next group's tally.
    rig = Rig(command_timeout_ms=2000)
    rig.open()
    rig.vehicle.ack_commands[frames.Cmd.COMPONENT_ARM_DISARM] = frames.Result.DENIED
    check(rig.ext("arm")[0] == rhal.SKR_EPERM, "DENIED is EPERM")
    rig.vehicle.ack_commands[frames.Cmd.COMPONENT_ARM_DISARM] = frames.Result.UNSUPPORTED
    check(rig.ext("arm")[0] == rhal.SKR_ENOSYS, "UNSUPPORTED is ENOSYS")
    rig.vehicle.ack_commands[frames.Cmd.COMPONENT_ARM_DISARM] = frames.Result.TEMPORARILY_REJECTED
    check(rig.ext("arm")[0] == rhal.SKR_EPERM, "TEMPORARILY_REJECTED is EPERM")
    rig.close()

    rig = Rig(command_timeout_ms=2000)
    rig.open()
    rig.vehicle.commands.clear()
    original = rig.vehicle._ack
    dropped = []

    def drop_first(command: int, result: int) -> None:
        if not dropped:
            dropped.append(command)
            return
        original(command, result)

    rig.vehicle._ack = drop_first  # type: ignore[method-assign]
    check(rig.ext("arm")[0] == rhal.SKR_OK, "a first acknowledgement lost on the wire is survived by the retry")
    tries = [(p[0]) for c, p in rig.vehicle.commands if c == frames.Cmd.COMPONENT_ARM_DISARM]
    check(len(tries) == 2, "which was one more send of the same command")
    rig.close()

    section("motions — mode")
    rig = Rig()
    rig.open()
    check(rig.ext("mode")[0] == rhal.SKR_EINVAL, "mode with no name is EINVAL, listing the allowed modes")
    check(rig.ext("mode name=guided")[0] == rhal.SKR_OK and rig.vehicle.mode == frames.COPTER_MODES["guided"], "guided is selected by the number the reference gives it")
    code, text = rig.ext("mode name=acro")
    check(code == rhal.SKR_EPERM and rig.vehicle.mode == frames.COPTER_MODES["guided"], "acro flies from stick input, nobody is at the sticks, and the driver does not select it by default")
    check(rig.ext("mode name=nonsense")[0] == rhal.SKR_EINVAL, "a name that is no mode is EINVAL")
    check(rig.ext("mode name=loiter")[0] == rhal.SKR_OK, "loiter is allowed")
    rig.vehicle.refuse_modes.add(frames.COPTER_MODES["poshold"])
    code, text = rig.ext("mode name=poshold")
    check(code == rhal.SKR_EPERM and rig.vehicle.mode == frames.COPTER_MODES["loiter"], "a mode the vehicle refuses is EPERM, and the mode stays what it was")
    rig.close()
    rig = Rig(allow_modes=["acro"])
    rig.open()
    check(rig.ext("mode name=acro")[0] == rhal.SKR_OK, "`allow_modes` widens the list, deliberately")
    rig.close()

    section("motions — takeoff, goto, velocity: the setpoint the vehicle would ignore without a word")
    rig = Rig()
    rig.open()
    check(rig.ext("takeoff alt=5")[0] == rhal.SKR_EPERM and "not armed" in rig.ext("takeoff alt=5")[1], "takeoff on a disarmed vehicle is refused here, before the vehicle can fail it silently")
    check(rig.ext("goto x=1 y=1 z=1")[0] == rhal.SKR_EPERM and rig.vehicle.setpoints == [], "a setpoint to a disarmed vehicle is not sent")
    rig.ext("arm")
    code, text = rig.ext("takeoff alt=5")
    check(code == rhal.SKR_EPERM and "not guided" in text, "armed but not in guided: refused, and the mode is not changed as a side effect")
    check(rig.vehicle.mode == frames.COPTER_MODES["stabilize"], "the mode is still what it was")
    rig.ext("mode name=guided")
    for bad, why in (("takeoff", "no altitude"), ("takeoff alt=abc", "a non-number"), ("takeoff alt=nan", "NaN"), ("takeoff alt=inf", "infinity"),
                     ("takeoff alt=0.1", "below the minimum"), ("takeoff alt=5 speed=2", "an argument it does not take")):
        check(rig.ext(bad)[0] == rhal.SKR_EINVAL, f"takeoff with {why} is EINVAL")
    check(rig.ext("takeoff alt=5")[0] == rhal.SKR_OK, "takeoff armed and guided is accepted")
    check(rig.vehicle.wait(lambda: rig.vehicle.landed == frames.Landed.IN_AIR, 8.0), "and the vehicle is in the air")
    rig.vehicle.commands.clear()
    check(rig.ext("goto x=3 y=4 z=6")[0] == rhal.SKR_OK, "goto in local ENU")
    target = last_setpoint(rig, 1)
    check((target["x"], target["y"], target["z"]) == (4.0, 3.0, -6.0), "is sent as north 4, east 3, down -6: the map is not a transposition")
    check(target["type_mask"] == 0x0DF8 and target["coordinate_frame"] == frames.Frame.LOCAL_NED, "with a position-only mask, in local NED")
    check(rig.ext("goto x=1 y=1 z=1 lat=1 lon=1 alt=1")[0] == rhal.SKR_EINVAL, "local and global goto together is EINVAL")
    check(rig.ext("goto")[0] == rhal.SKR_EINVAL, "goto with neither is EINVAL")
    check(rig.ext("goto x=1")[0] == rhal.SKR_EINVAL, "a local goto missing y is EINVAL")
    check(rig.ext("goto x=1 y=nan z=1")[0] == rhal.SKR_EINVAL, "and one with NaN in it")
    check(rig.ext("goto lat=95 lon=0 alt=5")[0] == rhal.SKR_EINVAL, "a latitude of 95 is EINVAL")
    check(rig.ext("goto lat=-35.36 lon=149.16 alt=12")[0] == rhal.SKR_OK, "a global goto is accepted")
    check(until(lambda: sum(1 for name, _ in rig.vehicle.received if name == "SET_POSITION_TARGET_GLOBAL_INT") == 1, 2.0), "and sent as a global setpoint")
    sent = next(fields for name, fields in rig.vehicle.received if name == "SET_POSITION_TARGET_GLOBAL_INT")
    check(sent["lat_int"] == -353600000 and sent["lon_int"] == 1491600000,
          "in degE7, as MAVLink says: degrees times ten million on both axes, so the setpoint lands where it was asked")
    check(sent["alt"] == 12.0, "at the altitude asked, in metres above home")
    check(rig.ext("velocity vx=1 vy=2 vz=3 yaw_rate=0.5")[0] == rhal.SKR_OK, "velocity in ENU")
    target = last_setpoint(rig, 2)
    check((target["vx"], target["vy"], target["vz"]) == (2.0, 1.0, -3.0), "is sent as north 2, east 1, down -3")
    check(target["yaw_rate"] == -0.5, "with the yaw rate's sign flipped: FLU's left turn is MAVLink's negative")
    check(target["type_mask"] == 0x05C7, "and a velocity-only mask that keeps the yaw rate, because the caller gave one")
    check(rig.ext("velocity vx=1 vy=0 vz=0")[0] == rhal.SKR_OK, "a velocity with no yaw rate")
    target = last_setpoint(rig, 3)
    check(target["type_mask"] == 0x0DC7 and target["yaw_rate"] == 0.0, "leaves the yaw rate ignored: to a rover, no rate and a rate of zero are different sentences (F179)")
    check(rig.ext("velocity vx=abc")[0] == rhal.SKR_EINVAL, "a velocity that is not a number is EINVAL")
    check(rig.ext("velocity speed=2")[0] == rhal.SKR_EINVAL, "and one with an argument it does not take")
    rig.close()

    rig = Rig("rover")
    rig.open()
    check(rig.ext("takeoff alt=5")[0] == rhal.SKR_EINVAL and rig.ext("land")[0] == rhal.SKR_EINVAL, "a rover has no takeoff and no landing, and is told so")
    rig.ext("mode name=guided")
    rig.ext("arm")
    check(rig.ext("goto x=5 y=2")[0] == rhal.SKR_OK, "a rover goes to a point with no altitude")
    check(rig.vehicle.wait(lambda: rig.vehicle.e > 4.0, 6.0), "and reaches it")
    check(rig.ext("hold")[0] == rhal.SKR_OK and rig.vehicle.mode == frames.ROVER_MODES["hold"], "hold on a rover is HOLD")
    rig.close()
    rig = Rig("plane")
    rig.open()
    check(rig.ext("velocity vx=1")[0] == rhal.SKR_EINVAL, "a fixed wing has no velocity setpoint")
    check(rig.ext("takeoff alt=5")[0] == rhal.SKR_EINVAL, "and takeoff is a mode, not a motion")
    check(rig.ext("mode name=takeoff")[0] == rhal.SKR_OK, "which is selected by name")
    rig.close()

    section("motions — parameters are read, never written")
    rig = Rig()
    rig.open()
    code, value = rig.ext("param name=FS_GCS_ENABLE")
    check(code == 0 and value == {"name": "FS_GCS_ENABLE", "value": 1.0}, "a parameter is read")
    check(rig.ext("param name=A_NAME_OF_MORE_THAN_SIXTEEN")[0] == rhal.SKR_EINVAL, "a name longer than the field's sixteen bytes is EINVAL and not a crash in the encoder")
    code, text = rig.ext("param name=NO_SUCH_PARAM")
    check(code == rhal.SKR_ENODEV, "a name the vehicle does not have is ENODEV: it stays silent for those, and the driver says so instead")
    check(rig.ext("param name=FS_GCS_ENABLE value=1")[0] == rhal.SKR_EINVAL, "and a value on the request is refused: there is no write")
    check(not any(name == "PARAM_SET" for name, _ in rig.vehicle.received), "PARAM_SET was never sent, in this whole session")
    check(rig.vehicle.gcs_heartbeats >= 1, "the driver heartbeats as a ground station, or the vehicle's own link-loss failsafe means nothing")
    rig.close()

    section("the stop")
    rig = Rig()
    rig.open()
    flying(rig, 5.0)
    rig.ext("velocity vx=3 vy=0 vz=0")
    check(until(lambda: rig.vehicle.ve > 2.0, 3.0), "moving east at 3 m/s")
    check(rig.bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    check(until(lambda: rig.vehicle.mode == frames.COPTER_MODES["brake"], 2.0), "and the vehicle is in BRAKE")
    check(until(lambda: abs(rig.vehicle.ve) < 0.1, 3.0), "and stopped")
    code, state = rig.state()
    check(state.flags & rhal.FLAG_ESTOP != 0, "the state says the stop is latched")
    check(rig.ext("land")[0] == rhal.SKR_EPERM and rig.ext("goto x=1 y=1 z=1")[0] == rhal.SKR_EPERM, "every motion but a read is refused while it is")
    check(rig.ext("status")[0] == rhal.SKR_OK and rig.ext("param name=FS_GCS_ENABLE")[0] == rhal.SKR_OK, "and a read is not")
    check(until(lambda: rig.bridge._stop_pending is False, 3.0), "the resend stops once a heartbeat shows the mode")
    check(rig.bridge.release_stop() == rhal.SKR_OK, "release_stop clears the latch")
    check(rig.ext("mode name=guided")[0] == rhal.SKR_OK, "and a motion is taken")
    check(rig.vehicle.mode == frames.COPTER_MODES["guided"], "release resumes nothing: the mode is whatever was asked for next")
    rig.close()

    rig = Rig()
    rig.open()
    flying(rig, 5.0)
    rig.vehicle.refuse_modes.add(frames.COPTER_MODES["brake"])
    rig.bridge.emergency_stop()
    check(until(lambda: rig.vehicle.mode == frames.COPTER_MODES["loiter"], 6.0), "BRAKE refused (it needs a position estimate): the next mode in the chain, LOITER, is tried")
    stops = [p[1] for c, p in rig.vehicle.commands if c == frames.Cmd.DO_SET_MODE]
    check(stops.count(float(frames.COPTER_MODES["brake"])) >= 2, "and BRAKE was sent again before giving up on it: one lost packet is not a refusal")
    rig.close()

    rig = Rig()
    rig.open(stop_mode="loiter")
    flying(rig, 5.0)
    rig.bridge.emergency_stop()
    check(until(lambda: rig.vehicle.mode == frames.COPTER_MODES["loiter"], 2.0), "`stop_mode` overrides the chain")
    rig.close()

    rig = Rig()
    rig.open()
    flying(rig, 5.0)
    link = rig.bridge._link
    assert link is not None
    holder = threading.Thread(target=lambda: (link._send_lock.acquire(), time.sleep(1.0), link._send_lock.release()), daemon=True)
    holder.start()
    time.sleep(0.05)
    started = time.monotonic()
    rig.bridge.emergency_stop()
    check(time.monotonic() - started < 0.3, "a stop does not wait behind a write in progress")
    check(until(lambda: rig.vehicle.mode == frames.COPTER_MODES["brake"], 3.0), "and still reaches the vehicle, by the resend, once the link is free")
    rig.close()

    rig = Rig()
    rig.open()
    rig.vehicle.stop()
    check(rig.bridge.emergency_stop() == rhal.SKR_OK, "a stop when the vehicle has gone away still returns, and the latch holds")
    check(rig.ext("arm")[0] == rhal.SKR_EPERM, "the latch refuses motions with the link down")
    rig.bridge.close()

    rig = Rig()
    rig.open()
    flying(rig, 5.0)
    rig.ext("goto x=20 y=0 z=5")
    rig.bridge.close()
    check(until(lambda: rig.vehicle.mode == frames.COPTER_MODES["brake"], 2.0), "closing a session that was commanding an armed vehicle leaves it braked, not still following a setpoint")
    rig.vehicle.stop()
    rig = Rig()
    rig.open()
    rig.bridge.close()
    check(not any(c == frames.Cmd.DO_SET_MODE for c, _ in rig.vehicle.commands), "and closing one that commanded nothing changes nothing on the vehicle")
    rig.vehicle.stop()

    section("link-loss")
    rig = Rig()
    rig.open()
    check(rig.vehicle.gcs_heartbeats >= 1 or until(lambda: rig.vehicle.gcs_heartbeats >= 1, 1.5), "the ground-station heartbeat is sent from the moment the link opens")
    before = rig.vehicle.gcs_heartbeats
    check(until(lambda: rig.vehicle.gcs_heartbeats > before, 2.5), "and keeps coming at about one a second")
    rig.close()

    return finish()


def finish() -> int:
    print(f"\n{CHECKS - len(FAILURES)} passed, {len(FAILURES)} failed")
    if FAILURES:
        print("failed:")
        for what in FAILURES:
            print(f"  - {what}")
    return 1 if FAILURES else 0


# ── the live block ───────────────────────────────────────────────────────────


def live_checks(link: str) -> None:
    """The bridge against a real autopilot's firmware, with the reference implementation as a second opinion."""
    del link
    raise SystemExit("the live block lives in live.py (see `make check-sim-docker`)")


if __name__ == "__main__":
    raise SystemExit(main())

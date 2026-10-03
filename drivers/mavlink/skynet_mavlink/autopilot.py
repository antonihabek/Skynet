"""A simulated ArduPilot or PX4 vehicle, written here. The dry substrate — and not the evidence.

`make check-sim-docker` and `make check-px4-docker` run this driver against ArduPilot's and PX4's own firmware (`sim/`). This module
exists for what those cannot give on demand, and for machines with no container:

* **refusals**: a vehicle that never acknowledges, one that answers with a frame whose checksum is wrong,
  one that goes silent mid-session, a second vehicle on the same link, a boat, a signed link;
* **timing**: an arming check that fails for a while and then passes, a stop mode that will not engage;
* **speed**: a suite that finishes in seconds.

It is a point mass in NED with the autopilot's *vocabulary* — the same mode numbers or packed words, the same acknowledgement
results, a refusal explained by a STATUSTEXT, a velocity setpoint that times out — and none of its flight
control, EKF or failsafes. Every behaviour in it was read off the firmware first (`selftest.py --live`
shows the two side by side), and where they differ the firmware is the one that is right. PX4's, in particular: a
mode it will not enter is acknowledged anyway (F224), an integer parameter is four bytes in the float field (F225), and a
stream that stops while it is in offboard mode sends it home with a failsafe status (F226).
"""

from __future__ import annotations

import math
import os
import pty
import socket
import struct
import threading
import time
import tty

from . import geometry
from .frames import (
    SPECS,
    Autopilot,
    Cmd,
    COPTER_MODES,
    Landed,
    MavState,
    MavType,
    ModeFlag,
    Parser,
    PX4_MODES,
    ROVER_MODES,
    PLANE_MODES,
    Result,
    SUB_MODES,
    crc_x25,
    encode,
)

__all__ = ["SimulatedAutopilot"]

_FAMILIES = {
    "copter": (MavType.QUADROTOR, COPTER_MODES, 0),
    "plane": (MavType.FIXED_WING, PLANE_MODES, 0),
    "rover": (MavType.GROUND_ROVER, ROVER_MODES, 0),
    "boat": (MavType.SURFACE_BOAT, ROVER_MODES, 0),
    "sub": (MavType.SUBMARINE, SUB_MODES, 0),
}


class SimulatedAutopilot:
    """One vehicle on a loopback TCP port (or UDP, or a pseudo-terminal).

    Every knob a test turns is a plain attribute, set before or after `start()`.
    """

    def __init__(self, family: str = "copter", *, system_id: int = 1, transport: str = "tcp", autopilot: int = Autopilot.ARDUPILOTMEGA) -> None:
        self.family = family
        self.system_id = system_id
        self.transport = transport
        self.autopilot = autopilot
        self.px4 = autopilot == Autopilot.PX4
        self.vehicle_type, self.modes, _ = _FAMILIES[family]
        if self.px4:
            self.modes = dict(PX4_MODES)
        self._numbers = {v: k for k, v in self.modes.items()}
        # ── what a test may set ──
        self.has_sim_params = True          # a SITL build has SIM_* parameters, a flight controller's does not (ArduPilot)
        self.autostart = 10040              # PX4's SYS_AUTOSTART: 10040 to 10049 are the airframes of its SIH build, and only that build has them
        self.gcs_failsafe = 0.0             # FS_GCS_ENABLE (ArduPilot) or NAV_DLL_ACT (PX4)
        self.px4_integers: dict[str, int] = {}  # further integer parameters PX4 answers for, each by its own four bytes
        self.offboard_loss_s = 1.0          # PX4: how long a stream may be silent before it flies home (COM_OF_LOSS_T)
        self.origin_silent = False          # PX4: never answer a request for the local frame's origin (no position estimate yet)
        self.ekf_ready_at = 0.0             # arming fails ("Need Position Estimate") until this monotonic time
        self.silent = False                 # send nothing at all
        self.ack_commands: dict[int, int | None] = {}   # command -> result to answer with; None = never answer
        self.refuse_modes: set[int] = set() # DO_SET_MODE to these is answered FAILED
        self.quiet_modes: set[int] = set()  # DO_SET_MODE to these is answered ACCEPTED and not made (ArduPilot family; PX4 does this by itself, F224)
        self.lose_mode_requests: dict[int, int] = {}  # mode word -> how many DO_SET_MODE to it never arrive (a datagram lost on the link: no change, no answer, not recorded)
        self.corrupt_next = 0               # the next N frames go out with a bad CRC
        self.sign_frames = False            # send every frame with the signed flag and a signature, and ignore unsigned messages (but heartbeats)
        self.accept_unsigned = False        # ...unless this is set: PX4's SIH build signs what it sends and acts on unsigned commands all the same (F228)
        self.extra_heartbeats: list[tuple[int, int, dict]] = []  # (system, component, fields) also heard
        self.guid_timeout = 3.0
        # ArduSub: the vehicle keeps the last `MANUAL_CONTROL` for `RC_OVERRIDE_TIME` (3 s) after the stream stops, then reads neutral sticks.
        self.rc_override_timeout = 3.0
        self.sub_speed = 1.0                # m/s at a full stick (the pilot's gain scales it on the real vehicle; here the gain is one)
        self.sub_yaw_rate = 1.0             # rad/s at a full stick
        self.heartbeat_period = 1.0         # seconds between heartbeats; a suite that waits on a state a heartbeat carries shortens it
        self.streams_only_on_request = True
        # ── the world ──
        self.n = self.e = self.d = 0.0
        self.vn = self.ve = self.vd = 0.0
        self.yaw = 0.0
        self.armed = False
        # PX4 boots in hold (AUTO.LOITER); ArduPilot's copter in stabilize and its rover in manual.
        self.mode = self.modes["hold"] if self.px4 else self.modes.get("stabilize", self.modes.get("manual", 0))
        self.landed = Landed.UNDEFINED if family == "sub" else Landed.ON_GROUND   # ArduSub has no ground, and says nothing about one
        self.battery = 100
        self.gps_fix = 3
        #: PX4's local frame's origin, as it reports it (degrees times 1e7, millimetres): the SIH build's own default home, in Zurich.
        self.origin = (473977433, 85455943, 489370)
        self._goto: tuple[float, float, float] | None = None
        self._takeoff_alt: float | None = None
        self._setpoint: tuple[float, float, float, float, float] | None = None  # vn, ve, vd, yaw_rate, time
        self._last_setpoint_at = 0.0        # PX4: when the last setpoint of any kind arrived, which offboard mode is entered and kept on
        self._failsafe = False              # PX4: the vehicle is flying a failsafe of its own (reported as MAV_STATE_CRITICAL)
        # ── the log a test reads ──
        self.received: list[tuple[str, dict]] = []
        self.commands: list[tuple[int, list[float]]] = []
        self.integer_commands: list[dict] = []   # every COMMAND_INT, whole (its x and y are integers a COMMAND_LONG's are not)
        self.command_times: list[float] = []     # when each entry of `commands` arrived (monotonic), for the order things were said in
        self.setpoints: list[dict] = []
        self.setpoint_times: list[float] = []    # and each setpoint
        self.sticks: list[dict] = []             # every MANUAL_CONTROL, whole
        self.stick_times: list[float] = []       # and when each arrived
        self._stick: tuple[int, int, int, int, float] | None = None   # x, y, z, r and when: the override the vehicle is obeying
        self.gcs_heartbeats = 0
        self.intervals: dict[int, float] = {}
        self.parse_errors = 0
        # ── plumbing ──
        self._parser = Parser()
        self._quit = threading.Event()
        self._threads: list[threading.Thread] = []
        self._lock = threading.RLock()
        self._client: socket.socket | None = None
        self._peer: tuple | None = None
        self._udp: socket.socket | None = None
        self._pty_master: int | None = None
        self.pty_path: str | None = None
        self.port = 0
        self._tx_seq = 0
        self._next_emit: dict[int, float] = {}
        self._statustexts: list[tuple[int, str]] = []
        self._last_gcs = time.monotonic()
        self._stopped_by_failsafe = False

    # ── lifecycle ────────────────────────────────────────────────────────────

    def start(self) -> "SimulatedAutopilot":
        if self.transport == "tcp":
            server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            server.bind(("127.0.0.1", 0))
            server.listen(1)
            server.settimeout(0.2)
            self.port = server.getsockname()[1]
            self._server = server
            self._spawn(self._accept_loop)
        elif self.transport == "udp":
            udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            udp.bind(("127.0.0.1", 0))
            udp.settimeout(0.05)
            self._udp = udp
            self.port = udp.getsockname()[1]
            self._spawn(self._udp_loop)
        elif self.transport == "pty":
            master, slave = pty.openpty()
            tty.setraw(master)
            tty.setraw(slave)
            self._pty_master = master
            self.pty_path = os.ttyname(slave)
            self._slave_fd = slave  # kept open so the master never sees a hangup
            self._spawn(self._pty_loop)
        else:
            raise ValueError(f"unknown transport {self.transport!r}")
        self._spawn(self._world_loop)
        return self

    def stop(self) -> None:
        self._quit.set()
        for thread in self._threads:
            thread.join(timeout=1.0)
        for sock in (getattr(self, "_server", None), self._client, self._udp):
            if sock is not None:
                try:
                    sock.close()
                except OSError:
                    pass
        for fd in (self._pty_master, getattr(self, "_slave_fd", None)):
            if fd is not None:
                try:
                    os.close(fd)
                except OSError:
                    pass

    def _spawn(self, target) -> None:
        thread = threading.Thread(target=target, daemon=True)
        thread.start()
        self._threads.append(thread)

    # ── the wire ─────────────────────────────────────────────────────────────

    def _accept_loop(self) -> None:
        while not self._quit.is_set():
            try:
                client, _ = self._server.accept()
            except (socket.timeout, OSError):
                continue
            client.settimeout(0.05)
            self._client = client
            self._parser = Parser()
            while not self._quit.is_set():
                try:
                    data = client.recv(4096)
                except socket.timeout:
                    continue
                except OSError:
                    break
                if not data:
                    break
                self._ingest(data)
            self._client = None

    def _udp_loop(self) -> None:
        assert self._udp is not None
        while not self._quit.is_set():
            try:
                data, peer = self._udp.recvfrom(4096)
            except (socket.timeout, OSError):
                continue
            self._peer = peer
            self._ingest(data)

    def _pty_loop(self) -> None:
        assert self._pty_master is not None
        import select

        while not self._quit.is_set():
            ready, _, _ = select.select([self._pty_master], [], [], 0.05)
            if not ready:
                continue
            try:
                data = os.read(self._pty_master, 4096)
            except OSError:
                return
            if data:
                self._ingest(data)

    def _write(self, data: bytes) -> None:
        if self.silent:
            return
        try:
            if self.transport == "tcp" and self._client is not None:
                self._client.sendall(data)
            elif self.transport == "udp" and self._udp is not None and self._peer is not None:
                self._udp.sendto(data, self._peer)
            elif self.transport == "pty" and self._pty_master is not None:
                os.write(self._pty_master, data)
        except OSError:
            pass

    def _emit(self, name: str, values: dict, *, system: int | None = None, component: int = 1) -> None:
        with self._lock:
            self._tx_seq = (self._tx_seq + 1) & 0xFF
            frame = bytearray(encode(SPECS[name], values, seq=self._tx_seq, system_id=system or self.system_id,
                                     component_id=component, incompat=1 if self.sign_frames else 0))
            if self.sign_frames:
                frame += b"\0" * 13  # a signature nothing here could verify, which is the point
            if self.corrupt_next > 0:
                self.corrupt_next -= 1
                frame[-1 if not self.sign_frames else -14] ^= 0xFF
            self._write(bytes(frame))

    def _text(self, severity: int, text: str) -> None:
        self._statustexts.append((severity, text))
        self._emit("STATUSTEXT", {"severity": severity, "text": text.encode()[:50]})

    # ── what it hears ────────────────────────────────────────────────────────

    def _ingest(self, data: bytes) -> None:
        for packet in self._parser.feed(data):
            if packet.name is None:
                continue
            self.received.append((packet.name, packet.fields))
            if self.sign_frames and not self.accept_unsigned and packet.name != "HEARTBEAT":
                continue  # signing on: an unsigned message is ignored, nothing answered and nothing done (ArduPilot's behaviour)
            handler = getattr(self, f"_on_{packet.name.lower()}", None)
            if handler is not None:
                handler(packet.fields)

    def _on_heartbeat(self, fields: dict) -> None:
        self.gcs_heartbeats += 1
        self._last_gcs = time.monotonic()

    def _ack(self, command: int, result: int) -> None:
        override = self.ack_commands.get(command, result)
        if override is None:
            return
        self._emit("COMMAND_ACK", {"command": command, "result": override, "target_system": 255, "target_component": 190})

    def _on_command_long(self, f: dict) -> None:
        command = int(f["command"])
        params = [f[f"param{i}"] for i in range(1, 8)]
        if command == Cmd.DO_SET_MODE:
            word = ((int(params[1]) << 16) | (int(params[2]) << 24)) if self.px4 else int(params[1])
            if self.lose_mode_requests.get(word, 0) > 0:
                self.lose_mode_requests[word] -= 1
                return
        self.commands.append((command, params))
        self.command_times.append(time.monotonic())
        if f["target_system"] not in (0, self.system_id):
            return
        if command == Cmd.SET_MESSAGE_INTERVAL:
            self.intervals[int(params[0])] = params[1]
            self._next_emit[int(params[0])] = 0.0
            self._ack(command, Result.ACCEPTED)
        elif command == Cmd.REQUEST_MESSAGE:
            if int(params[0]) == SPECS["AUTOPILOT_VERSION"].msg_id:
                self._send_version()
            elif int(params[0]) == SPECS["GPS_GLOBAL_ORIGIN"].msg_id and self.px4 and not self.origin_silent:
                self._emit("GPS_GLOBAL_ORIGIN", {"latitude": self.origin[0], "longitude": self.origin[1], "altitude": self.origin[2], "time_usec": 628000})
            self._ack(command, Result.ACCEPTED)
        elif command == Cmd.DO_SET_MODE and self.px4:
            self._px4_set_mode(int(params[1]), int(params[2]))
        elif command == Cmd.DO_SET_MODE:
            number = int(params[1])
            if number in self.refuse_modes or number not in self._numbers:
                self._ack(command, Result.FAILED)
            elif number in self.quiet_modes:
                self._ack(command, Result.ACCEPTED)
            else:
                self.mode = number
                self._ack(command, Result.ACCEPTED)
        elif command == Cmd.COMPONENT_ARM_DISARM:
            if params[0] >= 0.5:
                if time.monotonic() < self.ekf_ready_at:
                    self._text(2, "Arm: Need Position Estimate")
                    self._ack(command, Result.FAILED)
                else:
                    self.armed = True
                    self._text(6, "Arming motors")
                    self._ack(command, Result.ACCEPTED)
            else:
                if self.family != "sub" and self.landed != Landed.ON_GROUND and params[1] != 21196.0:
                    self._text(2, "Disarm: not landed")
                    # ArduPilot answers FAILED; PX4 answers TEMPORARILY_REJECTED (measured against each).
                    self._ack(command, Result.TEMPORARILY_REJECTED if self.px4 else Result.FAILED)
                else:
                    self.armed = False
                    self._ack(command, Result.ACCEPTED)
        elif command == Cmd.NAV_TAKEOFF and self.px4:
            # PX4 takes the altitude above sea level, and acknowledges a takeoff to a disarmed vehicle: it carries nothing out and
            # enters its takeoff mode anyway (F227).
            self.mode = self.modes["takeoff"]
            if self.armed:
                self._takeoff_alt = params[6] - self.origin[2] / 1000.0
                self.landed = Landed.TAKEOFF
            self._ack(command, Result.ACCEPTED)
        elif command == Cmd.NAV_TAKEOFF and self.family == "sub":
            self._ack(command, Result.UNSUPPORTED)   # a submarine has no takeoff (ArduSub does not handle the command)
        elif command == Cmd.NAV_TAKEOFF:
            if not self.armed or self.modes.get("guided") != self.mode:
                self._ack(command, Result.FAILED)
            else:
                self._takeoff_alt = params[6]
                self.landed = Landed.TAKEOFF
                self._ack(command, Result.ACCEPTED)
        elif command == Cmd.DO_REPOSITION and self.px4:
            self._px4_reposition(params[4], params[5], params[6])
            self._ack(command, Result.ACCEPTED)
        else:
            self._ack(command, Result.UNSUPPORTED)

    def _on_command_int(self, f: dict) -> None:
        command = int(f["command"])
        self.commands.append((command, [f["param1"], f["param2"], f["param3"], f["param4"], f["x"], f["y"], f["z"]]))
        self.command_times.append(time.monotonic())
        self.integer_commands.append(dict(f))
        if f["target_system"] not in (0, self.system_id):
            return
        if command == Cmd.DO_REPOSITION and self.px4 and f["frame"] == 0:
            self._px4_reposition(f["x"] / 1e7, f["y"] / 1e7, f["z"])
            self._ack(command, Result.ACCEPTED)
        else:
            self._ack(command, Result.UNSUPPORTED)

    def _px4_set_mode(self, main: int, sub: int) -> None:
        """PX4's DO_SET_MODE: a mode it will not enter is acknowledged all the same and not made (F224). Offboard is entered only while
        a stream is running (measured on its rover; its copter did not insist while disarmed, so the stream the driver sends first is
        what the check relies on)."""
        word = (main << 16) | (sub << 24)
        enter = word in self._numbers and word not in self.refuse_modes
        if enter and word == self.modes["offboard"] and time.monotonic() - self._last_setpoint_at > 0.5:
            enter = False
        if enter:
            self.mode = word
            self._failsafe = False
        self._ack(Cmd.DO_SET_MODE, Result.ACCEPTED)

    def _px4_reposition(self, lat: float, lon: float, amsl: float) -> None:
        """PX4's DO_REPOSITION: it goes to hold, and flies there if armed; a disarmed vehicle is told and does nothing. The local
        offset is PX4's own projection, a sphere of 6,371,000 m."""
        self.mode = self.modes["hold"]
        if not self.armed:
            return
        lat0, lon0, alt0 = self.origin[0] / 1e7, self.origin[1] / 1e7, self.origin[2] / 1000.0
        n = math.radians(lat - lat0) * 6371000.0
        e = math.radians(lon - lon0) * 6371000.0 * math.cos(math.radians(lat0))
        d = self.d if math.isnan(amsl) else -(amsl - alt0)
        self._goto = (n, e, d)
        self._setpoint = None

    def _on_param_request_read(self, f: dict) -> None:
        name = f["param_id"]
        if self.px4:
            # PX4 copies an integer's bytes into the float field, with the integer's type beside it (F225): 10040 is 1.4e-41 on the wire.
            integers = {"SYS_AUTOSTART": self.autostart, "NAV_DLL_ACT": int(self.gcs_failsafe), "COM_OBL_RC_ACT": 0, **self.px4_integers}
            reals = {"SIH_MASS": 1.0}
            if name in integers:
                value, kind = struct.pack("<i", integers[name]), 6   # the integer's own four bytes, not a float made of them
            elif name in reals:
                value, kind = reals[name], 9
            else:
                return  # an unknown name is not answered: that is what the firmware does
            self._emit("PARAM_VALUE", {"param_value": value, "param_count": len(integers) + len(reals), "param_index": 0,
                                       "param_id": name.encode(), "param_type": kind})
            return
        table = {"FS_GCS_ENABLE": self.gcs_failsafe, "GUID_TIMEOUT": self.guid_timeout}
        if self.has_sim_params:
            table["SIM_SPEEDUP"] = 1.0
        if name in table:
            self._emit("PARAM_VALUE", {"param_value": float(table[name]), "param_count": len(table), "param_index": 0,
                                       "param_id": name.encode(), "param_type": 9})
        # an unknown name is not answered: that is what the firmware does

    def _on_set_position_target_local_ned(self, f: dict) -> None:
        self.setpoints.append(dict(f))
        self.setpoint_times.append(time.monotonic())
        self._last_setpoint_at = time.monotonic()
        if self.px4:
            if self.armed and self.mode == self.modes["offboard"]:
                self._goto = None
                self._setpoint = (f["vx"], f["vy"], f["vz"], f["yaw_rate"], time.monotonic())
            return
        if not self.armed or self.mode != self.modes.get("guided"):
            return  # ignored without a word, as the firmware does
        mask = f["type_mask"]
        if not mask & 0x7 == 0x7:  # a position is asked for
            self._goto = (f["x"], f["y"], f["z"])
            self._setpoint = None
        else:
            self._goto = None
            self._setpoint = (f["vx"], f["vy"], f["vz"], f["yaw_rate"], time.monotonic())

    def _on_manual_control(self, f: dict) -> None:
        self.sticks.append(dict(f))
        self.stick_times.append(time.monotonic())
        self._stick = (f["x"], f["y"], f["z"], f["r"], time.monotonic())

    def _send_version(self) -> None:
        if self.px4:
            # 1.17.0, and the hash the SIH image the live block pins was built from (dd03e18fee), least significant byte first.
            self._emit("AUTOPILOT_VERSION", {"flight_sw_version": (1 << 24) | (17 << 16),
                                             "flight_custom_version": [0, 0, 0, 0xEE, 0x8F, 0xE1, 0x03, 0xDD]})
            return
        self._emit("AUTOPILOT_VERSION", {"flight_sw_version": (4 << 24) | (7 << 16) | (1 << 8) | 0xFF,
                                         "flight_custom_version": list(b"simulate")})

    # ── what it says ─────────────────────────────────────────────────────────

    def _heartbeat_fields(self) -> dict:
        base = ModeFlag.CUSTOM_MODE_ENABLED | (ModeFlag.SAFETY_ARMED if self.armed else 0)
        status = getattr(self, "system_status", None)
        if status is None:
            status = MavState.ACTIVE if self.armed else MavState.STANDBY
            if self._failsafe:
                status = MavState.CRITICAL
        return {"custom_mode": self.mode, "type": self.vehicle_type, "autopilot": self.autopilot, "base_mode": base,
                "system_status": status, "mavlink_version": 3}

    def _emit_streams(self, now: float) -> None:
        q = self._quaternion()
        for msg_id, name, values in (
            (SPECS["ATTITUDE_QUATERNION"].msg_id, "ATTITUDE_QUATERNION",
             lambda: {"q1": q[0], "q2": q[1], "q3": q[2], "q4": q[3]}),
            (SPECS["LOCAL_POSITION_NED"].msg_id, "LOCAL_POSITION_NED",
             lambda: {"x": self.n, "y": self.e, "z": self.d, "vx": self.vn, "vy": self.ve, "vz": self.vd}),
            (SPECS["GLOBAL_POSITION_INT"].msg_id, "GLOBAL_POSITION_INT", self._global_position),
            (SPECS["SYS_STATUS"].msg_id, "SYS_STATUS",
             lambda: {"onboard_control_sensors_present": 0x25, "onboard_control_sensors_enabled": 0x25,
                      "onboard_control_sensors_health": 0x25, "battery_remaining": self.battery}),
            (SPECS["GPS_RAW_INT"].msg_id, "GPS_RAW_INT", lambda: {"fix_type": self.gps_fix, "satellites_visible": 10}),
            (SPECS["EXTENDED_SYS_STATE"].msg_id, "EXTENDED_SYS_STATE", lambda: {"landed_state": self.landed}),
        ):
            interval = self.intervals.get(msg_id)
            if interval is None or interval <= 0:
                continue
            if now >= self._next_emit.get(msg_id, 0.0):
                self._next_emit[msg_id] = now + interval / 1e6
                self._emit(name, values())

    def _global_position(self) -> dict:
        if self.px4:
            # Altitude above sea level is the origin's plus the height above it, and relative altitude is the height: so `alt - relative_alt` is home's.
            lat0, lon0, alt0 = self.origin
            return {"lat": lat0 + int(round(math.degrees(self.n / 6371000.0) * 1e7)),
                    "lon": lon0 + int(round(math.degrees(self.e / (6371000.0 * math.cos(math.radians(lat0 / 1e7)))) * 1e7)),
                    "alt": alt0 + int(-self.d * 1000), "relative_alt": int(-self.d * 1000)}
        return {"lat": int(-353632610 + self.n / 0.0111 * 1), "lon": 1491652300 + int(self.e / 0.0091), "alt": int(-self.d * 1000), "relative_alt": int(-self.d * 1000)}

    def _quaternion(self) -> tuple[float, float, float, float]:
        """MAVLink's attitude quaternion for `self.yaw` (about down, from north), level."""
        half = self.yaw / 2
        return (math.cos(half), 0.0, 0.0, math.sin(half))

    # ── the world ────────────────────────────────────────────────────────────

    def _world_loop(self) -> None:
        last = time.monotonic()
        last_beat = 0.0
        while not self._quit.is_set():
            time.sleep(0.01)
            now = time.monotonic()
            dt, last = now - last, now
            self._step(dt, now)
            if now - last_beat >= self.heartbeat_period:
                last_beat = now
                if not self.silent:
                    self._emit("HEARTBEAT", self._heartbeat_fields())
                    for system, component, fields in self.extra_heartbeats:
                        self._emit("HEARTBEAT", fields, system=system, component=component)
            if not self.silent:
                self._emit_streams(now)

    def _step_px4(self, dt: float, now: float) -> None:
        """PX4's flight, as far as the vocabulary goes: takeoff to an altitude and then hold, a reposition in hold, a velocity in offboard
        for as long as the stream lasts, and a stream that stops sends it home (F226)."""
        modes = self.modes
        if self._failsafe and self.mode != modes["rtl"]:
            self._failsafe = False
        if not self.armed:
            self.vn = self.ve = self.vd = 0.0
            if self.landed != Landed.ON_GROUND:
                self.landed = Landed.ON_GROUND
            return
        target_v = (0.0, 0.0, 0.0)
        if self.mode == modes["offboard"] and now - self._last_setpoint_at > self.offboard_loss_s:
            self.mode, self._failsafe, self._goto, self._setpoint = modes["rtl"], True, (0.0, 0.0, self.d), None
            self._text(2, "Failsafe activated")
        if self.mode == modes["offboard"] and self._setpoint is not None:
            vn, ve, vd, yaw_rate, _ = self._setpoint
            target_v = (vn, ve, vd)
            self.yaw += (yaw_rate if math.isfinite(yaw_rate) else 0.0) * dt
        elif self.mode == modes["takeoff"] and self._takeoff_alt is not None:
            remaining = -self._takeoff_alt - self.d
            if abs(remaining) < 0.15:
                self._takeoff_alt, self.landed, self.mode = None, Landed.IN_AIR, modes["hold"]
            else:
                target_v = (0.0, 0.0, math.copysign(2.5, remaining))
        elif self.mode in (modes["hold"], modes["rtl"]) and self._goto is not None:
            dn, de, dd = self._goto[0] - self.n, self._goto[1] - self.e, self._goto[2] - self.d
            distance = math.sqrt(dn * dn + de * de + dd * dd)
            if distance > 0.1:
                speed = min(5.0, distance)
                target_v = (dn / distance * speed, de / distance * speed, dd / distance * speed)
                if math.hypot(dn, de) > 1.0:
                    self.yaw = math.atan2(de, dn)
            elif self.mode == modes["rtl"]:
                self.mode, self._goto = modes["land"], None
        elif self.mode == modes["land"]:
            target_v = (0.0, 0.0, 1.0)
            self.landed = Landed.LANDING
        self._move(target_v, dt)

    def _step(self, dt: float, now: float) -> None:
        if self.px4:
            with self._lock:
                self._step_px4(dt, now)
            return
        with self._lock:
            if self.gcs_failsafe and self.armed and now - self._last_gcs > 5.0 and not self._stopped_by_failsafe:
                self._stopped_by_failsafe = True
                self.mode = self.modes.get("brake", self.modes.get("hold", self.mode))
                self._text(1, "GCS Failsafe")
            if not self.armed:
                self.vn = self.ve = self.vd = 0.0
                if self.landed != Landed.ON_GROUND and self.family != "sub":
                    self.landed = Landed.ON_GROUND
                return
            guided = self.mode == self.modes.get("guided")
            land = self.mode == self.modes.get("land")
            target_v = (0.0, 0.0, 0.0)
            if guided and self._takeoff_alt is not None:
                remaining = -self._takeoff_alt - self.d
                if abs(remaining) < 0.15:
                    self._takeoff_alt = None
                    self.landed = Landed.IN_AIR
                else:
                    target_v = (0.0, 0.0, math.copysign(2.5, remaining))
            elif guided and self._goto is not None:
                dn, de, dd = self._goto[0] - self.n, self._goto[1] - self.e, self._goto[2] - self.d
                distance = math.sqrt(dn * dn + de * de + dd * dd)
                if distance > 0.1:
                    speed = min(5.0, distance)
                    target_v = (dn / distance * speed, de / distance * speed, dd / distance * speed)
                    if math.hypot(dn, de) > 1.0:
                        self.yaw = math.atan2(de, dn)
            elif guided and self._setpoint is not None:
                vn, ve, vd, yaw_rate, at = self._setpoint
                if now - at <= self.guid_timeout:
                    target_v = (vn, ve, vd)
                    self.yaw += (yaw_rate if math.isfinite(yaw_rate) else 0.0) * dt
            elif land:
                target_v = (0.0, 0.0, 1.0)
                self.landed = Landed.LANDING
            if self.family == "sub" and not guided:
                target_v = self._sub_velocity(now, dt)
            self._move(target_v, dt)

    #: ArduSub's modes that take the sticks, and the one that climbs on its own.
    _SUB_STICK_MODES = ("stabilize", "manual", "alt_hold", "poshold", "surftrak")

    def _sub_velocity(self, now: float, dt: float) -> tuple[float, float, float]:
        """What a submarine does in a mode that is not guided: the sticks it was last given, for as long as the override lasts (`RC_OVERRIDE_TIME`), and `surface` climbs.

        The sticks are MAVLink's (x forward, y right, z up from 500, r clockwise) and the point mass is neutrally buoyant, so neutral sticks are a vehicle that drifts nowhere:
        the real thing's depth and position holds are not modelled, which is the firmware's."""
        if self.mode == self.modes["surface"]:
            if self.d <= 0.0:
                # ArduSub's `ModeSurface::run`: "already at surface, hold depth at surface", which is a switch to alt_hold (reason SURFACE_COMPLETE), not a mode that stays.
                self.mode = self.modes["alt_hold"]
                return (0.0, 0.0, 0.0)
            return (0.0, 0.0, -0.5)
        if self.mode not in (self.modes[name] for name in self._SUB_STICK_MODES) or self._stick is None:
            return (0.0, 0.0, 0.0)
        x, y, z, r, at = self._stick
        if now - at > self.rc_override_timeout:
            return (0.0, 0.0, 0.0)
        forward, right, up = x / 1000.0 * self.sub_speed, y / 1000.0 * self.sub_speed, (z - 500) / 500.0 * self.sub_speed
        self.yaw += r / 1000.0 * self.sub_yaw_rate * dt
        return (forward * math.cos(self.yaw) - right * math.sin(self.yaw), forward * math.sin(self.yaw) + right * math.cos(self.yaw), -up)

    def _move(self, target_v: tuple[float, float, float], dt: float) -> None:
        # Accelerate toward the target at 4 m/s^2: a stop is not instantaneous.
        step = 4.0 * dt
        self.vn, self.ve, self.vd = (self._toward(v, t, step) for v, t in zip((self.vn, self.ve, self.vd), target_v))
        self.n += self.vn * dt
        self.e += self.ve * dt
        self.d += self.vd * dt
        if self.family in ("rover", "boat"):
            self.d, self.vd = 0.0, 0.0
        elif self.family == "sub":
            if self.d < 0.0:
                self.d, self.vd = 0.0, 0.0   # a submarine cannot rise out of the water: the surface is depth zero
        elif self.d >= 0.0 and self.landed in (Landed.LANDING, Landed.IN_AIR):
            self.d, self.vd = 0.0, 0.0
            self.landed = Landed.ON_GROUND
            self.armed = False  # a copter disarms itself after landing
        elif self.d < -0.2 and self.landed == Landed.ON_GROUND:
            self.landed = Landed.TAKEOFF

    @staticmethod
    def _toward(value: float, target: float, step: float) -> float:
        if abs(target - value) <= step:
            return target
        return value + math.copysign(step, target - value)

    # ── things a test wants without reaching in ──────────────────────────────

    def forget(self) -> None:
        """Empty every log a test reads, together: `commands` and `command_times` are parallel lists, and a test that cleared one would misalign the other."""
        for log in (self.commands, self.command_times, self.integer_commands, self.setpoints, self.setpoint_times, self._statustexts):
            del log[:]

    def position_enu(self) -> tuple[float, float, float]:
        return geometry.ned_to_enu(self.n, self.e, self.d)

    def wait(self, predicate, timeout: float = 5.0) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return True
            time.sleep(0.01)
        return bool(predicate())

    def garbage(self, data: bytes) -> None:
        """Bytes that are not a frame, straight onto the wire."""
        self._write(data)

    def raw_frame_with_id(self, msg_id: int, payload: bytes, *, crc_extra: int) -> None:
        """A frame for a message id this driver does not know (its seed supplied by the test)."""
        header = bytes([len(payload), 0, 0, 0, self.system_id, 1]) + msg_id.to_bytes(3, "little")
        crc = crc_x25(bytes([crc_extra]), crc_x25(header + payload))
        self._write(bytes([0xFD]) + header + payload + crc.to_bytes(2, "little"))

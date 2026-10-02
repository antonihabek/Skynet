"""The RHAL driver: an ArduPilot vehicle — multirotor, fixed wing, ground rover or surface boat — over MAVLink.

This is a **command-level** body (§59.2): the flight controller owns every servo loop, the EKF and the
failsafes, and there is no joint here for the OS to command. It reads the vehicle's pose and twist and
faults, and it moves it by named motions under `vendor.mavlink.task` (§84.1), the way `drivers/pudu` and
`drivers/spot` do. Guest tier is the ceiling by mechanism; the kernel never sees these commands.

# What it reads off the vehicle rather than out of a file

| From | Message | Used for |
|---|---|---|
| the heartbeat | `HEARTBEAT.type`, `autopilot` | which vehicle, and whether it is one this driver knows |
| the version | `AUTOPILOT_VERSION.flight_sw_version`, the build hash | the descriptor's slug, and the proof of which firmware answered |
| the parameter table | `SIM_SPEEDUP` | **whether it is a simulation.** A SITL build has `SIM_*` parameters and a flight controller's build does not |
| the pose | `LOCAL_POSITION_NED`, `ATTITUDE_QUATERNION` (else `ATTITUDE`) | `base_pose` in REP-103, ENU and FLU (§44.4) |
| the twist | the same two | `base_twist`: linear in the world frame, angular in the body frame |
| the health | `SYS_STATUS`, `HEARTBEAT.system_status` | the fault word, and whether the state is degraded |

Nothing about the vehicle is typed into a configuration file but what a person must assert: the link.

# What it refuses to bring up

* **A link with no vehicle on it**, and **a link with more than one** unless `system_id` names one — a
  driver that picked would command whichever vehicle spoke first.
* **PX4, and anything that is not ArduPilot.** PX4's modes are a packed union and its offboard mode wants a
  setpoint stream before it will engage; that is another driver's work, and no PX4 has been run.
* **A submarine.** §97 gave §5 the word for it (`submerged`), so the refusal is no longer about the vocabulary: it
  is that ArduSub has a mode set of its own (`SUB_MODES`), no takeoff, no ground, and a stop that is a person's
  decision — hold depth, or come up — and that nothing here has driven one. It is refused by name, with that reason,
  until the marine work in DRY-COVERAGE-PLAN.md 12.E.4 has.
* **A signed link.** MAVLink 2 signing means the vehicle will ignore what this driver sends, and this driver
  holds no key.

# A boat

A surface boat is ArduRover built with a boat frame: it reports MAV_TYPE 11, takes Rover's modes, and moves under
the same two motions a rover does (`goto`, `velocity`). Its descriptor says `base: "surface"` (§97) where a rover's
says `wheeled`, which is the whole of what a hull being a hull changes here — §87.2 then asks it for a safe-state
basis before host, as it asks a wheeled base. Its stop is HOLD, which stops the thrust and does not stop the hull: a
boat has no brake, and it comes to rest as the water and the wind allow. That is what a software stop means on one,
and `emergency_stop`'s caller should read it that way.

It changes nothing on the vehicle at `open()`: it asks for telemetry rates (a per-link setting the vehicle
forgets) and reads parameters, and never writes one.

# The stop

`emergency_stop` latches, then puts the vehicle in the mode that stops it: BRAKE on a multirotor (LOITER if
BRAKE will not engage), HOLD on a rover, LOITER on a fixed wing — which circles, because a wing cannot stand
still. It does not kill the motors; a multirotor that loses its rotors in the air is a falling object, and
that is a decision for a person with a separate button. The command is resent every second until the
vehicle's own heartbeat says it took it. It is a **software** stop over the same link as the commands.
"""

from __future__ import annotations

import collections
import json
import math
import sys
import threading
import time

import skynet_rhal as rhal

from . import geometry
from .frames import (
    SPECS,
    Autopilot,
    Cmd,
    FAMILY_MODES,
    Frame,
    Landed,
    MavState,
    MavType,
    ModeFlag,
    Packet,
    Parser,
    ProtocolError,
    Result,
    Sensor,
    encode,
)
from .link import Link, LinkClosed, LinkRefusal, open_link

__all__ = ["MavlinkBridge", "BridgeRefusal", "TOKEN"]

TOKEN = "vendor.mavlink.task"

#: What a heartbeat is not: a ground station, a companion computer, a gimbal, an ADS-B receiver, an antenna
#: tracker. Their `autopilot` field is normally `INVALID`, and this is the belt beside those braces.
_NOT_A_VEHICLE = frozenset({MavType.GCS, MavType.ONBOARD_CONTROLLER, MavType.GIMBAL, MavType.ADSB, MavType.ANTENNA_TRACKER})

#: Telemetry this driver asks for, `(message, Hz)`. A per-link setting: the vehicle forgets it on reboot,
#: and the keepalive asks again when the data stops.
_STREAMS = (("ATTITUDE_QUATERNION", 25), ("LOCAL_POSITION_NED", 25), ("GLOBAL_POSITION_INT", 5),
            ("SYS_STATUS", 2), ("GPS_RAW_INT", 2), ("EXTENDED_SYS_STATE", 2))

#: The modes the extension will select by name, per family. A mode that flies from stick input (stabilize,
#: acro, flip, throw, autotune, ...) has nobody at the sticks, and this driver would be the thing that put the
#: vehicle in it. `allow_modes` in the configuration widens this deliberately.
_ALLOWED_MODES = {
    "copter": ("guided", "guided_nogps", "loiter", "poshold", "alt_hold", "brake", "land", "rtl", "smart_rtl", "auto", "circle"),
    "rover": ("guided", "hold", "loiter", "auto", "rtl", "smart_rtl"),
    "plane": ("guided", "loiter", "rtl", "auto", "circle", "takeoff", "qloiter", "qland", "qrtl"),
}
#: What stops each family, most preferred first (see the module docstring).
_STOP_CHAIN = {"copter": ("brake", "loiter"), "rover": ("hold",), "plane": ("loiter",)}
#: The modes in which the vehicle takes a position or velocity setpoint from a companion computer.
_GUIDED = {"copter": ("guided", "guided_nogps"), "rover": ("guided",), "plane": ("guided",)}

#: The parameter that turns on each family's ground-station link-loss failsafe. ArduPlane spells it without the
#: final E (ArduPilot's own name, held by the live block, which reads it off the firmware).
_LINK_FAILSAFE_PARAM = {"copter": "FS_GCS_ENABLE", "rover": "FS_GCS_ENABLE", "plane": "FS_GCS_ENABL"}

#: What each `MAV_STATE` the heartbeat can carry is *for the vehicle*, in the classes `tools/fault-vocab/contract.py` holds every driver to
#: (`drivers/fault-vocabulary.json`, `mav_state`, which says why): `ok`; `degraded` (DEGRADED alone, motions taken); `fault` (FAULT and DEGRADED —
#: §27's `rhal` condition reads that bit and not FAULT — and the motions that take the vehicle somewhere new refused); `estop` (ESTOP and DEGRADED,
#: every motion but `status` and `param` refused). A state in no row is `fault`: a vehicle whose autopilot reports what nobody has described is not
#: read as well.
_STATE_CLASS = {
    MavState.UNINIT: "degraded",
    MavState.BOOT: "degraded",
    MavState.CALIBRATING: "degraded",
    MavState.STANDBY: "ok",
    MavState.ACTIVE: "ok",
    MavState.CRITICAL: "fault",
    MavState.EMERGENCY: "fault",
    MavState.POWEROFF: "fault",
    MavState.FLIGHT_TERMINATION: "estop",
}
#: The motions a vehicle in a failsafe still answers: the way out. A person in a failsafe wants `land` and `rtl` most, so a driver that refused
#: them because the heartbeat said CRITICAL would take the way out away at the moment it matters (the table's `keeps`).
_FAILSAFE_MOTIONS = ("status", "param", "hold", "rtl", "land", "disarm")

_COMMAND_TRIES = 3
_COMMAND_TIMEOUT_S = 1.0
_PROGRESS_TIMEOUT_S = 10.0
_HEARTBEAT_STALE_S = 3.5
_POSE_STALE_S = 1.0
_SEVERITY_ERROR = 3  # MAV_SEVERITY_ERROR and worse are the ones a refusal is explained by


class BridgeRefusal(Exception):
    """A reason this body did not come up, in the terms an integrator acts on."""


class Unreachable(BridgeRefusal):
    """The vehicle did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""


class _Refused(Exception):
    """A motion that did not happen: the code it answers with, and the words."""

    def __init__(self, code: int, message: str) -> None:
        super().__init__(message)
        self.code = code
        self.message = message


class _Ack:
    def __init__(self) -> None:
        self.event = threading.Event()
        self.result: int | None = None


class MavlinkBridge:
    """A body made of one ArduPilot vehicle."""

    def __init__(self) -> None:
        self._lock = threading.RLock()
        self._opened = False
        self._reset()

    def _reset(self) -> None:
        self._link: Link | None = None
        self._parser = Parser()
        self._quit = threading.Event()
        self._threads: list[threading.Thread] = []
        self._latest: dict[str, tuple[dict, int]] = {}
        self._heartbeats: dict[tuple[int, int], tuple[dict, int]] = {}
        self._params: dict[str, float] = {}
        self._param_event = threading.Event()
        self._texts: collections.deque[tuple[int, int, str]] = collections.deque(maxlen=64)
        self._partial: dict[int, list[str]] = {}
        self._pending: dict[int, list[_Ack]] = {}
        self._pending_lock = threading.Lock()
        self._signed_seen = False
        self._dead: str | None = None
        self._seq = 0
        self._tx_seq = 0
        self._our_system = 255
        self._our_component = 190
        self._target_system = 0
        self._target_component = 0
        self._family = ""
        #: `surface` for a boat, which shares Rover's modes under another base kind (§97); empty for the rest.
        self._hull = ""
        self._vehicle_type = 0
        self._firmware = ""
        self._git = ""
        self._simulated = True
        self._estop = False
        self._stop_pending = False
        self._stop_sent_at = 0.0
        self._stop_index = 0
        self._stop_index_at = 0.0
        self._stop_chain: list[int] = []
        self._commanded = False
        self._allowed_modes: tuple[str, ...] = ()
        self._descriptor: str | None = None
        self._mode_numbers: dict[str, int] = {}
        self._link_failsafe: float | None = None
        self._command_timeout = _COMMAND_TIMEOUT_S
        self._command_tries = _COMMAND_TRIES

    # ── §4's entry points ────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError as error:
            _say(f"the configuration is not JSON: {error}")
            return rhal.SKR_EINVAL
        if not isinstance(settings, dict):
            _say("the configuration is not a JSON object")
            return rhal.SKR_EINVAL
        # A body, and the stop latched on it, belong to one session (DRY-COVERAGE-PLAN.md F73, F76).
        self._teardown()
        self._reset()
        try:
            self._open(settings)
        except LinkRefusal as refusal:
            self._teardown()
            _say(str(refusal))
            return rhal.SKR_EINVAL
        except Unreachable as refusal:
            self._teardown()
            _say(str(refusal))
            return rhal.SKR_EIO
        except BridgeRefusal as refusal:
            self._teardown()
            _say(str(refusal))
            return rhal.SKR_ENODEV
        except LinkClosed as error:
            self._teardown()
            _say(f"the vehicle could not be reached: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _open(self, settings: dict) -> None:
        self._our_system = _byte(settings, "source_system", 255)
        self._our_component = _byte(settings, "source_component", 190)
        wanted_system = settings.get("system_id")
        if wanted_system is not None:
            wanted_system = _byte(settings, "system_id", 1)
        open_timeout = _seconds(settings, "open_timeout_ms", 8.0)
        self._command_timeout = _seconds(settings, "command_timeout_ms", _COMMAND_TIMEOUT_S)
        tries = settings.get("command_tries", _COMMAND_TRIES)
        if isinstance(tries, bool) or not isinstance(tries, int) or not 1 <= tries <= 10:
            raise BridgeRefusal(f"`command_tries` of {tries!r} is not a whole number from 1 to 10.")
        self._command_tries = tries
        self._link = open_link(settings.get("link"))
        self._start_threads()

        vehicle = self._discover(wanted_system, open_timeout)
        self._target_system, self._target_component = vehicle
        heartbeat, _ = self._heartbeats[vehicle]
        self._classify(heartbeat)
        if self._signed_seen:
            raise BridgeRefusal(
                "the vehicle is sending MAVLink 2 signed frames. Signing means it will ignore an unsigned command, "
                "and this driver holds no signing key: a session that could read the vehicle and move nothing "
                "would look like a working link. Turn signing off on the vehicle, or give this driver a key."
            )

        stop_name = settings.get("stop_mode")
        table = FAMILY_MODES[self._family]
        self._mode_numbers = dict(table)
        chain = (stop_name,) if stop_name else _STOP_CHAIN[self._family]
        for name in chain:
            if name not in table:
                raise BridgeRefusal(f"`stop_mode` {name!r} is not a {self._family} mode. The modes are {', '.join(sorted(table))}.")
        self._stop_chain = [table[name] for name in chain]
        extra = settings.get("allow_modes") or ()
        if not isinstance(extra, (list, tuple)) or any(name not in table for name in extra):
            raise BridgeRefusal(f"`allow_modes` must list {self._family} mode names from {', '.join(sorted(table))}.")
        self._allowed_modes = tuple(dict.fromkeys(_ALLOWED_MODES[self._family] + tuple(extra)))

        self._request_streams()
        self._read_version()
        self._simulated = bool(settings.get("simulator", False)) or self._read_param("SIM_SPEEDUP", 1.0) is not None
        failsafe = _LINK_FAILSAFE_PARAM[self._family]
        self._link_failsafe = self._read_param(failsafe, 1.0)
        if self._link_failsafe == 0:
            _say(
                f"this vehicle's ground-station link-loss failsafe is off ({failsafe}=0). If this process dies the "
                "vehicle keeps the mode it was last given until a setpoint times out; this driver reads parameters and "
                "never writes one, so that is yours to set."
            )
        self._wait_for(lambda: "LOCAL_POSITION_NED" in self._latest and ("ATTITUDE_QUATERNION" in self._latest or "ATTITUDE" in self._latest), 3.0)
        self._descriptor = self._build_descriptor()

    def close(self) -> int:
        if self._link is not None and self._commanded and not self._estop:
            heartbeat = self._latest_heartbeat()
            if heartbeat is not None and heartbeat["base_mode"] & ModeFlag.SAFETY_ARMED:
                # A session ends with the vehicle stopped where it is, not still following a setpoint.
                try:
                    self._send_mode(self._stop_chain[0], wait=0.05)
                except (LinkClosed, IndexError):
                    pass
        self._teardown()
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._link is None:
            return rhal.SKR_EPERM
        now = rhal.acquisition_ns()
        with self._lock:
            latest = dict(self._latest)
            heartbeat = self._heartbeats.get((self._target_system, self._target_component))
        self._seq += 1
        out.seq = self._seq
        out.flags = rhal.HOMING_NOT_REQUIRED
        out.fault_code = 0
        out.position_rad = []
        out.velocity_rad_s = []
        out.effort_nm = []
        out.base_pose = [math.nan] * 7
        out.base_twist = [0.0] * 6
        if heartbeat is None:
            return rhal.SKR_ETIMEDOUT
        hb, hb_ns = heartbeat
        stamp = hb_ns
        pose_ok = False
        position = latest.get("LOCAL_POSITION_NED")
        quaternion = _orientation(latest)
        if position is not None and quaternion is not None:
            fields, position_ns = position
            attitude, attitude_ns = quaternion
            if now - position_ns <= _POSE_STALE_S * 1e9 and now - attitude_ns <= _POSE_STALE_S * 1e9:
                east, north, up = geometry.ned_to_enu(fields["x"], fields["y"], fields["z"])
                out.base_pose = [east, north, up, *attitude["q"]]
                vx, vy, vz = geometry.ned_to_enu(fields["vx"], fields["vy"], fields["vz"])
                wx, wy, wz = geometry.frd_to_flu(attitude["rates"][0], attitude["rates"][1], attitude["rates"][2])
                out.base_twist = [vx, vy, vz, wx, wy, wz]
                stamp = max(position_ns, attitude_ns)
                pose_ok = True
        out.stamp_ns = stamp
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP
        status = hb["system_status"]
        klass = _STATE_CLASS.get(status, "fault")
        if klass != "ok":
            out.flags |= rhal.FLAG_DEGRADED
            out.fault_code = status
        if klass == "fault":
            out.flags |= rhal.FLAG_FAULT
        elif klass == "estop":
            out.flags |= rhal.FLAG_ESTOP
        degraded = not pose_ok or self._unhealthy(latest)
        battery = latest.get("SYS_STATUS")
        if battery is not None and 0 <= battery[0]["battery_remaining"] <= 10:
            degraded = True
        if degraded:
            out.flags |= rhal.FLAG_DEGRADED
        if self._dead or now - hb_ns > _HEARTBEAT_STALE_S * 1e9:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        # The shape before the permission, the way `drivers/pendant` and `drivers/pudu` check it.
        if cmd.joint_count != 0:
            return rhal.SKR_EINVAL
        if cmd.mode not in (rhal.MODE_POSITION, rhal.MODE_VELOCITY, rhal.MODE_TORQUE):
            return rhal.SKR_EINVAL
        _say("joint commands are refused: this body has no joints, and the only route that moves the vehicle is "
             f"`extension` under {TOKEN}.")
        return rhal.SKR_EPERM

    def emergency_stop(self) -> int:
        self._estop = True
        self._stop_pending = True
        self._stop_index = 0
        self._stop_index_at = time.monotonic()
        self._stop_sent_at = 0.0
        if self._link is not None and self._stop_chain:
            try:
                # Never waits behind a command in flight, and never writes into the middle of one.
                self._send_mode(self._stop_chain[0], wait=0.05)
                self._stop_sent_at = time.monotonic()
            except LinkClosed:
                pass  # the latch is set, and the keepalive keeps trying while the link lasts
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch (§48.2). The vehicle stays in the stop mode; nothing is resumed."""
        self._estop = False
        self._stop_pending = False
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── the extension: motions ───────────────────────────────────────────────

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != TOKEN:
            return rhal.SKR_ENOSYS, f"unknown token {token!r}. This driver serves {TOKEN}.".encode()
        if not self._opened or self._link is None:
            return rhal.SKR_EPERM, b"the driver is not open."
        try:
            words = data.decode("utf-8").split()
        except UnicodeDecodeError:
            return rhal.SKR_EINVAL, b"the request is text: `<motion> k=v k=v` in UTF-8."
        if not words:
            return rhal.SKR_EINVAL, f"empty request. Motions: {', '.join(self._motion_names())}.".encode()
        motion, args = words[0], {}
        for word in words[1:]:
            key, sep, value = word.partition("=")
            if not sep or not key:
                return rhal.SKR_EINVAL, f"malformed argument {word!r}: wanted k=v.".encode()
            args[key] = value
        if motion not in self._motion_names():
            return rhal.SKR_EINVAL, f"unknown motion {motion!r}. Motions: {', '.join(self._motion_names())}.".encode()
        if self._estop and motion not in ("status", "param"):
            return rhal.SKR_EPERM, (
                b"refused: the emergency stop is latched. Only `status` and `param` are answered until it is released."
            )
        refusal = self._failsafe_refusal(motion)
        if refusal is not None:
            return rhal.SKR_EPERM, refusal.encode()
        try:
            handler = getattr(self, f"_motion_{motion}")
            return rhal.SKR_OK, json.dumps(handler(args), separators=(",", ":")).encode()
        except _Refused as refusal:
            return refusal.code, refusal.message.encode()
        except ProtocolError as error:
            # A value that does not fit its field reached the encoder: the request was wrong, and the link is fine.
            return rhal.SKR_EINVAL, f"the request does not fit the wire: {error}".encode()
        except LinkClosed as error:
            return rhal.SKR_EIO, f"the link failed: {error}".encode()

    def _failsafe_refusal(self, motion: str) -> str | None:
        """Why the vehicle's own state says this motion is not made, or `None`. The way out stays open in a failsafe; nothing moves a terminated one."""
        heartbeat = self._latest_heartbeat()
        if heartbeat is None:
            return None
        status = heartbeat["system_status"]
        klass = _STATE_CLASS.get(status, "fault")
        name = MavState.NAMES.get(status, f"state {status}")
        if klass == "estop" and motion not in ("status", "param"):
            return f"refused: the vehicle reports {name}, the autopilot ending the flight. Only `status` and `param` are answered; nothing moves it."
        if klass == "fault" and motion not in _FAILSAFE_MOTIONS:
            return (
                f"refused: the vehicle reports {name}, a failsafe. A motion that takes it somewhere new would fight it; "
                f"the way out is open: {', '.join(m for m in _FAILSAFE_MOTIONS if m not in ('status', 'param'))}."
            )
        return None

    def _kind(self) -> str:
        """What the vehicle is called to a person: the mode family, except that a hull is a boat."""
        return "boat" if self._hull == "surface" else self._family

    def _motion_names(self) -> tuple[str, ...]:
        common = ("status", "param", "arm", "disarm", "mode", "hold", "rtl", "goto")
        if self._family == "copter":
            return common + ("takeoff", "land", "velocity")
        if self._family == "rover":
            return common + ("velocity",)
        return common

    def _motion_status(self, args: dict) -> dict:
        self._no_arguments("status", args)
        hb = self._latest_heartbeat()
        assert hb is not None
        sys_status = self._latest.get("SYS_STATUS", ({}, 0))[0]
        gps = self._latest.get("GPS_RAW_INT", ({}, 0))[0]
        landed = self._latest.get("EXTENDED_SYS_STATE", ({}, 0))[0].get("landed_state")
        return {
            "family": self._family,
            "kind": self._kind(),
            "firmware": self._firmware,
            "build": self._git,
            "simulator": self._simulated,
            "mode": self._mode_name(hb["custom_mode"]),
            "armed": bool(hb["base_mode"] & ModeFlag.SAFETY_ARMED),
            "state": MavState.NAMES.get(hb["system_status"], str(hb["system_status"])),
            "landed": Landed.NAMES.get(landed) if landed is not None else None,
            "gps_fix": gps.get("fix_type"),
            "satellites": gps.get("satellites_visible"),
            "battery_percent": sys_status.get("battery_remaining"),
            "link_failsafe": self._link_failsafe,
            "stop_latched": self._estop,
            "messages": [{"severity": s, "text": t} for _, s, t in list(self._texts)[-5:]],
        }

    def _motion_param(self, args: dict) -> dict:
        name = args.get("name")
        if not name or set(args) != {"name"}:
            raise _Refused(rhal.SKR_EINVAL, "param needs name=NAME (and nothing else): a parameter to read. This driver never writes one.")
        if len(name) > 16 or not name.isascii():
            raise _Refused(rhal.SKR_EINVAL, f"{name!r} is not a parameter name: they are ASCII and at most 16 characters (the field is 16 bytes).")
        value = self._read_param(name, 2.0)
        if value is None:
            raise _Refused(rhal.SKR_ENODEV, f"the vehicle does not answer for a parameter named {name!r} (it ignores names it does not have).")
        return {"name": name, "value": value}

    def _motion_arm(self, args: dict) -> dict:
        self._no_arguments("arm", args)
        self._run_command(Cmd.COMPONENT_ARM_DISARM, [1.0], "arm")
        if not self._wait_for(lambda: self._is_armed(), 4.0):
            raise _Refused(rhal.SKR_EIO, "the vehicle accepted the command and its heartbeat still says disarmed after four seconds.")
        return {"armed": True}

    def _motion_disarm(self, args: dict) -> dict:
        force = args.pop("force", "0")
        self._no_arguments("disarm", args)
        if force not in ("0", "1"):
            raise _Refused(rhal.SKR_EINVAL, "disarm takes force=1 to disarm in the air (motors stop; the vehicle falls), or nothing.")
        # 21196 is the value ArduPilot and MAVLink both document for "disarm regardless of state".
        self._run_command(Cmd.COMPONENT_ARM_DISARM, [0.0, 21196.0 if force == "1" else 0.0], "disarm")
        if not self._wait_for(lambda: not self._is_armed(), 4.0):
            raise _Refused(rhal.SKR_EIO, "the vehicle accepted the command and its heartbeat still says armed after four seconds.")
        return {"armed": False}

    def _motion_mode(self, args: dict) -> dict:
        name = args.pop("name", None)
        self._no_arguments("mode", args)
        if not name:
            raise _Refused(rhal.SKR_EINVAL, f"mode needs name=MODE. Allowed here: {', '.join(self._allowed_modes)}.")
        return self._set_mode_by_name(name)

    def _motion_hold(self, args: dict) -> dict:
        self._no_arguments("hold", args)
        name = next(n for n, v in self._mode_numbers.items() if v == self._stop_chain[0])
        return self._set_mode_by_name(name, allow_any=True)

    def _motion_rtl(self, args: dict) -> dict:
        self._no_arguments("rtl", args)
        return self._set_mode_by_name("rtl", allow_any=True)

    def _motion_land(self, args: dict) -> dict:
        self._no_arguments("land", args)
        return self._set_mode_by_name("land", allow_any=True)

    def _motion_takeoff(self, args: dict) -> dict:
        altitude = self._number(args, "alt", required=True, minimum=0.5)
        self._no_arguments("takeoff", args)
        hb = self._latest_heartbeat()
        assert hb is not None
        if not hb["base_mode"] & ModeFlag.SAFETY_ARMED:
            raise _Refused(rhal.SKR_EPERM, "the vehicle is not armed: `arm` first. A takeoff command to a disarmed vehicle is refused by the vehicle and would be answered here with its words, not a fix.")
        if self._mode_name(hb["custom_mode"]) not in _GUIDED["copter"]:
            raise _Refused(rhal.SKR_EPERM, f"the vehicle is in {self._mode_name(hb['custom_mode'])}, not guided: `mode name=guided` first. This driver does not change a vehicle's mode as a side effect of a motion.")
        self._run_command(Cmd.NAV_TAKEOFF, [0.0, 0.0, 0.0, math.nan, math.nan, math.nan, altitude], "takeoff")
        return {"takeoff_alt_m": altitude}

    def _motion_goto(self, args: dict) -> dict:
        has_local = any(k in args for k in ("x", "y", "z"))
        has_global = any(k in args for k in ("lat", "lon", "alt"))
        if has_local == has_global:
            raise _Refused(rhal.SKR_EINVAL, "goto takes either x= y= z= (metres, east-north-up from the EKF origin) or lat= lon= alt= (degrees, and metres above home) — not both, and one of them.")
        self._require_guided()
        if has_local:
            east = self._number(args, "x", required=True)
            north = self._number(args, "y", required=True)
            up = self._number(args, "z", required=self._family != "rover", default=0.0)
            self._no_arguments("goto", args)
            n, e, d = geometry.enu_to_ned(east, north, up)
            # Position only: ignore velocity (bits 3-5), acceleration (6-8), yaw (10) and yaw rate (11).
            self._send("SET_POSITION_TARGET_LOCAL_NED", {
                "x": n, "y": e, "z": d, "type_mask": 0x0DF8, "coordinate_frame": Frame.LOCAL_NED,
                "target_system": self._target_system, "target_component": self._target_component})
            self._commanded = True
            return {"target_enu_m": [east, north, up]}
        lat = self._number(args, "lat", required=True, minimum=-90.0, maximum=90.0)
        lon = self._number(args, "lon", required=True, minimum=-180.0, maximum=180.0)
        alt = self._number(args, "alt", required=self._family != "rover", default=0.0)
        self._no_arguments("goto", args)
        self._send("SET_POSITION_TARGET_GLOBAL_INT", {
            "lat_int": int(round(lat * 1e7)), "lon_int": int(round(lon * 1e7)), "alt": alt, "type_mask": 0x0DF8,
            "coordinate_frame": Frame.GLOBAL_RELATIVE_ALT_INT, "target_system": self._target_system,
            "target_component": self._target_component})
        self._commanded = True
        return {"target": {"lat": lat, "lon": lon, "alt_m": alt}}

    def _motion_velocity(self, args: dict) -> dict:
        east = self._number(args, "vx", default=0.0)
        north = self._number(args, "vy", default=0.0)
        up = self._number(args, "vz", default=0.0)
        turning = "yaw_rate" in args
        yaw_rate = self._number(args, "yaw_rate", default=0.0)
        self._no_arguments("velocity", args)
        self._require_guided()
        vn, ve, vd = geometry.enu_to_ned(east, north, up)
        # Velocity only: ignore position (bits 0-2), acceleration (6-8) and yaw (10). The yaw *rate* (bit 11) is
        # ignored unless the caller gave one, because "no yaw rate" and "a yaw rate of zero" are different
        # sentences to a rover: ArduRover turns toward a velocity vector whose rate is ignored and drives
        # straight along its heading when the rate is zero (measured against its firmware, F179). A rate is in
        # the body's FLU convention (left is positive), and MAVLink's is about down (right is positive).
        self._send("SET_POSITION_TARGET_LOCAL_NED", {
            "vx": vn, "vy": ve, "vz": vd, "yaw_rate": -yaw_rate if turning else 0.0,
            "type_mask": 0x05C7 if turning else 0x0DC7, "coordinate_frame": Frame.LOCAL_NED,
            "target_system": self._target_system, "target_component": self._target_component})
        self._commanded = True
        return {"velocity_enu_m_s": [east, north, up], "yaw_rate_rad_s": yaw_rate if turning else None,
                "note": "one setpoint: the vehicle stops following it when GUID_TIMEOUT passes, so repeat it while it should continue"}

    # ── helpers for motions ──────────────────────────────────────────────────

    def _no_arguments(self, motion: str, args: dict) -> None:
        if args:
            raise _Refused(rhal.SKR_EINVAL, f"{motion} does not take {', '.join(sorted(args))}.")

    def _number(self, args: dict, key: str, *, required: bool = False, default: float | None = None,
                minimum: float | None = None, maximum: float | None = None) -> float:
        raw = args.pop(key, None)
        if raw is None:
            if required:
                raise _Refused(rhal.SKR_EINVAL, f"{key}= is required.")
            return default if default is not None else 0.0
        try:
            value = float(raw)
        except ValueError:
            raise _Refused(rhal.SKR_EINVAL, f"{key}={raw!r} is not a number.") from None
        if not math.isfinite(value):
            raise _Refused(rhal.SKR_EINVAL, f"{key}={raw!r} is not finite.")
        if minimum is not None and value < minimum:
            raise _Refused(rhal.SKR_EINVAL, f"{key}={value} is below {minimum}.")
        if maximum is not None and value > maximum:
            raise _Refused(rhal.SKR_EINVAL, f"{key}={value} is above {maximum}.")
        return value

    def _require_guided(self) -> None:
        hb = self._latest_heartbeat()
        assert hb is not None
        mode = self._mode_name(hb["custom_mode"])
        if not hb["base_mode"] & ModeFlag.SAFETY_ARMED:
            raise _Refused(rhal.SKR_EPERM, "the vehicle is not armed. It ignores a setpoint without saying so, so this driver says it: `arm` first.")
        if mode not in _GUIDED[self._family]:
            raise _Refused(rhal.SKR_EPERM, f"the vehicle is in {mode}, not guided. It ignores a setpoint in any other mode without saying so: `mode name=guided` first.")

    def _set_mode_by_name(self, name: str, allow_any: bool = False) -> dict:
        if name not in self._mode_numbers:
            raise _Refused(rhal.SKR_EINVAL, f"{name!r} is not a {self._family} mode. Allowed here: {', '.join(self._allowed_modes)}.")
        if not allow_any and name not in self._allowed_modes:
            raise _Refused(rhal.SKR_EPERM, f"{name} is a mode this driver does not select by default: it flies from stick input or does something the caller did not ask for. Allowed here: {', '.join(self._allowed_modes)}. `allow_modes` in the configuration widens the list.")
        number = self._mode_numbers[name]
        self._run_command(Cmd.DO_SET_MODE, [float(ModeFlag.CUSTOM_MODE_ENABLED), float(number)], f"mode {name}")
        if not self._wait_for(lambda: (self._latest_heartbeat() or {}).get("custom_mode") == number, 4.0):
            raise _Refused(rhal.SKR_EIO, f"the vehicle accepted mode {name} and its heartbeat still reports another after four seconds.")
        return {"mode": name}

    def _run_command(self, command: int, params: list[float], what: str) -> None:
        started = rhal.acquisition_ns()
        result = self._command(command, params)
        if result == Result.ACCEPTED:
            self._commanded = True
            return
        why = self._explain(started)
        if result is None:
            raise _Refused(rhal.SKR_ETIMEDOUT, f"the vehicle did not acknowledge {what} in {self._command_tries} tries.{why}")
        name = Result.NAMES.get(result, f"result {result}")
        if result == Result.UNSUPPORTED:
            raise _Refused(rhal.SKR_ENOSYS, f"the vehicle does not support {what} ({name}).{why}")
        raise _Refused(rhal.SKR_EPERM, f"the vehicle refused {what} ({name}).{why}")

    def _explain(self, since_ns: int) -> str:
        """The vehicle's own words for a refusal: the STATUSTEXT it sent while the command was outstanding."""
        said = [t for ns, severity, t in list(self._texts) if ns >= since_ns and severity <= _SEVERITY_ERROR + 1]
        return f" It said: {'; '.join(said)}." if said else ""

    # ── the vehicle ──────────────────────────────────────────────────────────

    def _start_threads(self) -> None:
        for name, target in (("reader", self._read_loop), ("keepalive", self._keepalive_loop)):
            thread = threading.Thread(target=target, name=f"mavlink-{name}", daemon=True)
            thread.start()
            self._threads.append(thread)

    def _read_loop(self) -> None:
        link = self._link
        assert link is not None
        while not self._quit.is_set():
            try:
                data = link.recv(0.25)
            except LinkClosed as error:
                if not self._quit.is_set():
                    self._dead = str(error)
                return
            if not data:
                continue
            arrived = rhal.acquisition_ns()
            for packet in self._parser.feed(data):
                self._handle(packet, arrived)

    def _handle(self, packet: Packet, arrived: int) -> None:
        if packet.signed:
            self._signed_seen = True
        if packet.name is None or packet.system_id == self._our_system:
            return
        fields = packet.fields
        if packet.name == "HEARTBEAT":
            with self._lock:
                self._heartbeats[(packet.system_id, packet.component_id)] = (fields, arrived)
            return
        if self._target_system and (packet.system_id != self._target_system):
            return
        if packet.name == "COMMAND_ACK":
            target = fields.get("target_system", 0)
            if target not in (0, self._our_system):
                return
            with self._pending_lock:
                for waiter in self._pending.get(fields["command"], []):
                    waiter.result = fields["result"]
                    waiter.event.set()
        elif packet.name == "STATUSTEXT":
            self._statustext(fields, arrived)
        elif packet.name == "PARAM_VALUE":
            with self._lock:
                self._params[fields["param_id"]] = fields["param_value"]
            self._param_event.set()
        elif packet.name in ("ATTITUDE_QUATERNION", "ATTITUDE", "LOCAL_POSITION_NED", "GLOBAL_POSITION_INT", "SYS_STATUS",
                             "GPS_RAW_INT", "EXTENDED_SYS_STATE", "AUTOPILOT_VERSION"):
            if self._target_component and packet.component_id != self._target_component:
                return
            with self._lock:
                self._latest[packet.name] = (fields, arrived)

    def _statustext(self, fields: dict, arrived: int) -> None:
        text, ident = fields["text"], fields["id"]
        if ident:
            chunks = self._partial.setdefault(ident, [])
            chunks.append(text)
            if len(text) >= 50 and len(chunks) < 8:
                return
            text = "".join(self._partial.pop(ident))
        self._texts.append((arrived, fields["severity"], text))

    def _keepalive_loop(self) -> None:
        last_beat = 0.0
        last_streams = time.monotonic()
        while not self._quit.wait(0.1):
            now = time.monotonic()
            try:
                if now - last_beat >= 1.0:
                    last_beat = now
                    self._send("HEARTBEAT", {"type": MavType.GCS, "autopilot": Autopilot.INVALID, "base_mode": 0,
                                             "custom_mode": 0, "system_status": MavState.ACTIVE, "mavlink_version": 3})
                if self._stop_pending and now - self._stop_sent_at >= 1.0 and self._stop_chain:
                    if self._stop_confirmed():
                        self._stop_pending = False
                    else:
                        # Resend every second; move to the next mode in the chain only after a mode has
                        # had time to show in a heartbeat (1 Hz) and still has not.
                        if len(self._stop_chain) > 1 and now - self._stop_index_at >= 2.5:
                            self._stop_index = (self._stop_index + 1) % len(self._stop_chain)
                            self._stop_index_at = now
                        self._send_mode(self._stop_chain[self._stop_index], wait=0.05)
                        self._stop_sent_at = now
                if self._target_system and now - last_streams >= 5.0:
                    last_streams = now
                    fresh = self._latest.get("LOCAL_POSITION_NED")
                    if fresh is None or rhal.acquisition_ns() - fresh[1] > 3e9:
                        for name, hz in _STREAMS:
                            self._send_interval(name, hz)
            except (LinkClosed, ProtocolError):
                return

    def _stop_confirmed(self) -> bool:
        hb = self._latest_heartbeat()
        if hb is None:
            return False
        return hb["custom_mode"] in self._stop_chain or not hb["base_mode"] & ModeFlag.SAFETY_ARMED

    def _discover(self, wanted: int | None, timeout: float) -> tuple[int, int]:
        """Wait for the vehicle's heartbeat, and for a second one that would make the choice ambiguous."""
        deadline = time.monotonic() + timeout
        settle: float | None = None
        while time.monotonic() < deadline:
            candidates = self._candidates(wanted)
            if candidates and settle is None:
                # One vehicle heard. Listen a moment longer: a second changes the answer.
                settle = time.monotonic() + (0.0 if wanted is not None else 1.6)
            if settle is not None and time.monotonic() >= settle:
                break
            if self._dead:
                raise Unreachable(f"the link ended before a vehicle answered: {self._dead}")
            time.sleep(0.02)
        candidates = self._candidates(wanted)
        if self._signed_seen and not candidates:
            raise BridgeRefusal("frames arrive but none parses as a MAVLink 2 message this driver can verify: the link is signed. This driver holds no signing key.")
        if not candidates:
            raise Unreachable(
                f"no vehicle heartbeat arrived within {timeout:.0f}s. A vehicle that is off, a link on the wrong port or "
                "baud rate and a `udpin` link the vehicle was never told to send to all look like this."
            )
        systems = sorted({system for system, _ in candidates})
        if len(systems) > 1:
            raise BridgeRefusal(
                f"vehicles {systems} answer on this link. Name one with `system_id`: a driver that picked would command "
                "whichever heartbeat arrived first."
            )
        system = systems[0]
        components = sorted(component for s, component in candidates if s == system)
        if 1 in components:
            return system, 1
        if len(components) > 1:
            raise BridgeRefusal(f"vehicle {system} has autopilots at components {components}, and none of them is 1. This driver does not choose between them.")
        return system, components[0]

    def _candidates(self, wanted: int | None) -> list[tuple[int, int]]:
        with self._lock:
            return [key for key, (hb, _) in self._heartbeats.items()
                    if hb["autopilot"] != Autopilot.INVALID and hb["type"] not in _NOT_A_VEHICLE
                    and key[0] != self._our_system and (wanted is None or key[0] == wanted)]

    def _classify(self, hb: dict) -> None:
        autopilot, vehicle = hb["autopilot"], hb["type"]
        self._vehicle_type = vehicle
        self._hull = ""
        if autopilot != Autopilot.ARDUPILOTMEGA:
            raise BridgeRefusal(
                f"the autopilot is MAV_AUTOPILOT {autopilot}"
                f"{' (PX4)' if autopilot == Autopilot.PX4 else ''}. This driver is written for ArduPilot's modes, whose "
                "custom-mode numbers it holds against the reference. PX4 packs a main and a sub mode into one word and "
                "wants a setpoint stream before it will engage offboard: that is another driver, and no PX4 has been run."
            )
        if vehicle in MavType.MULTIROTORS:
            self._family = "copter"
        elif vehicle == MavType.FIXED_WING:
            self._family = "plane"
        elif vehicle == MavType.GROUND_ROVER:
            self._family = "rover"
        elif vehicle == MavType.SURFACE_BOAT:
            # ArduRover built with a boat frame: Rover's modes and Rover's motions, and §97's `surface` for a base.
            self._family = "rover"
            self._hull = "surface"
        elif vehicle == MavType.SUBMARINE:
            raise BridgeRefusal(
                f"the vehicle reports MAV_TYPE {vehicle}, a submarine. §5 has had the word for it since §97 (`submerged`), so this is not "
                "the vocabulary: ArduSub has a mode set of its own, no takeoff and no ground, and the stop that is right under the "
                "surface — hold depth, or come up — is a person's decision, not a table entry. Nothing here has driven one. Refused by "
                "name until the marine work (DRY-COVERAGE-PLAN.md 12.E.4) has; the vehicle was not commanded."
            )
        elif vehicle in MavType.VTOLS:
            raise BridgeRefusal(f"the vehicle is a VTOL (MAV_TYPE {vehicle}). Its modes are a plane's and a copter's at once; this driver holds one family per vehicle and has met neither.")
        else:
            raise BridgeRefusal(f"the vehicle reports MAV_TYPE {vehicle}, which is not a multirotor, a fixed wing or a ground rover.")

    def _request_streams(self) -> None:
        for name, hz in _STREAMS:
            self._send_interval(name, hz)

    def _send_interval(self, name: str, hz: float) -> None:
        self._send("COMMAND_LONG", {
            "param1": float(SPECS[name].msg_id), "param2": 1e6 / hz, "command": Cmd.SET_MESSAGE_INTERVAL,
            "target_system": self._target_system, "target_component": self._target_component})

    def _read_version(self) -> None:
        self._command(Cmd.REQUEST_MESSAGE, [float(SPECS["AUTOPILOT_VERSION"].msg_id)], tries=2, timeout=1.0)
        self._wait_for(lambda: "AUTOPILOT_VERSION" in self._latest, 2.0)
        version = self._latest.get("AUTOPILOT_VERSION")
        if version is None:
            self._firmware, self._git = "unknown", ""
            return
        fields = version[0]
        packed = fields["flight_sw_version"]
        self._firmware = f"{packed >> 24}.{(packed >> 16) & 0xFF}.{(packed >> 8) & 0xFF}"
        self._git = bytes(fields["flight_custom_version"]).rstrip(b"\0").decode("ascii", "replace")

    def _read_param(self, name: str, timeout: float) -> float | None:
        """A parameter's value, or `None` when the vehicle does not answer for it (it ignores unknown names)."""
        with self._lock:
            self._params.pop(name, None)
        self._param_event.clear()
        self._send("PARAM_REQUEST_READ", {"param_index": -1, "target_system": self._target_system,
                                          "target_component": self._target_component, "param_id": name.encode("ascii")})
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self._lock:
                if name in self._params:
                    return self._params[name]
            self._param_event.wait(max(0.0, min(0.1, deadline - time.monotonic())))
            self._param_event.clear()
        return None

    def _command(self, command: int, params: list[float], *, tries: int | None = None, timeout: float | None = None) -> int | None:
        """Send a COMMAND_LONG and wait for its acknowledgement; retry with a rising confirmation counter."""
        tries = self._command_tries if tries is None else tries
        timeout = self._command_timeout if timeout is None else timeout
        waiter = _Ack()
        with self._pending_lock:
            self._pending.setdefault(command, []).append(waiter)
        padded = list(params) + [0.0] * (7 - len(params))
        try:
            for attempt in range(tries):
                self._send("COMMAND_LONG", {
                    **{f"param{i + 1}": padded[i] for i in range(7)}, "command": command, "confirmation": attempt,
                    "target_system": self._target_system, "target_component": self._target_component})
                deadline = time.monotonic() + timeout
                while (remaining := deadline - time.monotonic()) > 0:
                    if not waiter.event.wait(remaining):
                        break
                    waiter.event.clear()
                    if waiter.result == Result.IN_PROGRESS:
                        deadline = time.monotonic() + _PROGRESS_TIMEOUT_S
                        continue
                    return waiter.result
            return None
        finally:
            with self._pending_lock:
                self._pending[command].remove(waiter)

    def _send(self, name: str, values: dict) -> None:
        link = self._link
        if link is None:
            raise LinkClosed("the link is closed")
        self._tx_seq = (self._tx_seq + 1) & 0xFF
        link.send(encode(SPECS[name], values, seq=self._tx_seq, system_id=self._our_system, component_id=self._our_component))

    def _send_mode(self, number: int, *, wait: float) -> None:
        """DO_SET_MODE, once, without waiting for an answer and without queueing behind a write in progress."""
        link = self._link
        if link is None:
            raise LinkClosed("the link is closed")
        self._tx_seq = (self._tx_seq + 1) & 0xFF
        frame = encode(SPECS["COMMAND_LONG"], {
            "param1": float(ModeFlag.CUSTOM_MODE_ENABLED), "param2": float(number), "command": Cmd.DO_SET_MODE,
            "target_system": self._target_system, "target_component": self._target_component},
            seq=self._tx_seq, system_id=self._our_system, component_id=self._our_component)
        link.try_send(frame, wait)

    def _wait_for(self, predicate, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return True
            time.sleep(0.02)
        return bool(predicate())

    def _latest_heartbeat(self) -> dict | None:
        with self._lock:
            entry = self._heartbeats.get((self._target_system, self._target_component))
        return entry[0] if entry else None

    def _is_armed(self) -> bool:
        hb = self._latest_heartbeat()
        return bool(hb and hb["base_mode"] & ModeFlag.SAFETY_ARMED)

    def _mode_name(self, number: int) -> str:
        for name, value in self._mode_numbers.items():
            if value == number:
                return name
        return f"mode {number}"

    def _unhealthy(self, latest: dict) -> bool:
        """A sensor this vehicle has, uses, and reports unhealthy. What is absent is not reported as broken."""
        entry = latest.get("SYS_STATUS")
        if entry is None:
            return False
        fields = entry[0]
        critical = Sensor.GYRO | Sensor.ACCEL | Sensor.MAG | Sensor.ABSOLUTE_PRESSURE | Sensor.GPS | Sensor.AHRS
        mask = fields["onboard_control_sensors_present"] & fields["onboard_control_sensors_enabled"] & critical
        return bool(mask & ~fields["onboard_control_sensors_health"])

    def _teardown(self) -> None:
        self._quit.set()
        link, self._link = self._link, None
        if link is not None:
            link.close()
        for thread in self._threads:
            if thread is not threading.current_thread():
                thread.join(timeout=1.0)
        self._threads = []

    # ── the descriptor ───────────────────────────────────────────────────────

    def _build_descriptor(self) -> str:
        motions = [
            {"name": "status", "parameters": []},
            {"name": "param", "parameters": [{"name": "name", "required": True}]},
            {"name": "arm", "parameters": []},
            {"name": "disarm", "parameters": [{"name": "force", "required": False}]},
            {"name": "mode", "parameters": [{"name": "name", "required": True}]},
            {"name": "hold", "parameters": []},
            {"name": "rtl", "parameters": []},
            {"name": "goto", "parameters": [
                {"name": "x", "unit": "m", "required": False}, {"name": "y", "unit": "m", "required": False},
                {"name": "z", "unit": "m", "required": False}, {"name": "lat", "unit": "deg", "required": False},
                {"name": "lon", "unit": "deg", "required": False}, {"name": "alt", "unit": "m", "required": False}]},
        ]
        if self._family == "copter":
            motions.append({"name": "takeoff", "parameters": [{"name": "alt", "unit": "m", "required": True, "min": 0.5}]})
            motions.append({"name": "land", "parameters": []})
        if self._family in ("copter", "rover"):
            motions.append({"name": "velocity", "parameters": [
                {"name": "vx", "unit": "m/s", "required": False}, {"name": "vy", "unit": "m/s", "required": False},
                {"name": "vz", "unit": "m/s", "required": False}, {"name": "yaw_rate", "unit": "rad/s", "required": False}]})
        descriptor = {
            "schema_version": 1,
            "robot_slug": f"ardupilot-{self._kind()}-{self._firmware}".rstrip("-"),
            "digest": None,
            "verified": False,
            "simulator": self._simulated,
            "base": self._hull or ("aerial" if self._family in ("copter", "plane") else "wheeled"),
            "dof": 0,
            "joints": [],
            "end_effectors": [],
            "sensors": [],
            "control": {"modes": [], "rate_hz_max": 25, "rate_hz_min": 1, "command_latency_ms": None},
            # Null, not an empty object: a vehicle's speed and altitude limits are parameters the flight controller
            # enforces (and this driver only reads), not a risk assessment anybody here made (§5.2).
            "envelope": None,
            "capabilities": ["safety.estop"],
            "motions": motions,
            "extensions": [TOKEN],
            "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))


# ── module helpers ───────────────────────────────────────────────────────────


def _orientation(latest: dict) -> tuple[dict, int] | None:
    """`{"q": [qx, qy, qz, qw], "rates": [p, q, r]}`: the orientation already in Skynet's frame (FLU to ENU),
    and the body rates still as MAVLink reports them (FRD), which `read_state` converts."""
    quaternion = latest.get("ATTITUDE_QUATERNION")
    if quaternion is not None:
        fields, ns = quaternion
        q = geometry.quat_ned_frd_to_enu_flu(fields["q1"], fields["q2"], fields["q3"], fields["q4"])
        return {"q": list(q), "rates": [fields["rollspeed"], fields["pitchspeed"], fields["yawspeed"]]}, ns
    euler = latest.get("ATTITUDE")
    if euler is not None:
        fields, ns = euler
        w, x, y, z = geometry.quat_from_euler_ned_frd(fields["roll"], fields["pitch"], fields["yaw"])
        q = geometry.quat_ned_frd_to_enu_flu(w, x, y, z)
        return {"q": list(q), "rates": [fields["rollspeed"], fields["pitchspeed"], fields["yawspeed"]]}, ns
    return None


def _byte(settings: dict, key: str, default: int) -> int:
    value = settings.get(key, default)
    if isinstance(value, bool) or not isinstance(value, int) or not 1 <= value <= 255:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a MAVLink system or component id (1 to 255).")
    return value


def _seconds(settings: dict, key: str, default: float) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not 0 < value <= 120000:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number of milliseconds above zero and at most 120000.")
    return float(value) / 1000.0


def _say(message: str) -> None:
    print(f"skynet-mavlink: {message}", file=sys.stderr, flush=True)

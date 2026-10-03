"""One autopilot's vocabulary for a vehicle, and the motions that are not a mode change.

`MavlinkBridge` is the link, the telemetry, the commands and the stop, and none of that differs between autopilots. What does is here:

* the **mode table** — ArduPilot numbers its modes, PX4 packs a main and a sub mode into one word;
* how a mode is **requested** (`DO_SET_MODE`'s parameters), how a **parameter** value is read, how the build is read out of
  `AUTOPILOT_VERSION`;
* what **stops** the vehicle, which modes take a setpoint from a companion computer, which link-loss failsafe the vehicle has;
* **takeoff, goto and velocity**, which each autopilot does its own way;
* how it knows whether the vehicle is a **simulation**.

A motion set holds the family it was made for and nothing else: the link, the telemetry and the latch it asks the bridge for. The bridge
methods it calls (`_number`, `_no_arguments`, `_send`, `_run_command`, `_latest_heartbeat`, `_wait_for`, `_mode_name`, `_is_armed`,
`_commanded`) are the toolkit; they are underscored because they are the bridge's own, and used here because the two modules are one
package and say so.
"""

from __future__ import annotations

import math
import struct
import time

import skynet_rhal as rhal

from . import geometry
from .errors import BridgeRefusal, Refused
from .frames import SPECS, Autopilot, Cmd, FAMILY_MODES, Frame, MavType, ModeFlag, PX4_MODES
from .link import LinkClosed

__all__ = ["Motions", "ArdupilotMotions", "ArdusubMotions", "Px4Motions", "select_motions"]


class Motions:
    """What one autopilot makes of one kind of vehicle. Subclasses fill in the tables and the three motions."""

    #: The word in the descriptor's slug and in `status`.
    dialect = ""

    def __init__(self, bridge, family: str, hull: str = "") -> None:
        self.bridge = bridge
        self.family = family
        #: `surface` for a boat, which shares Rover's modes under another base kind (§97); empty otherwise.
        self.hull = hull

    # ── the tables ───────────────────────────────────────────────────────────

    @property
    def table(self) -> dict[str, int]:
        """Mode name to the vehicle's custom-mode word."""
        raise NotImplementedError

    @property
    def allowed(self) -> tuple[str, ...]:
        """The modes `mode name=` will select without `allow_modes`: not the ones that fly from stick input."""
        raise NotImplementedError

    @property
    def link_failsafe_param(self) -> str:
        """The parameter that turns on the vehicle's ground-station link-loss failsafe."""
        raise NotImplementedError

    def stop_chain(self, settings: dict) -> tuple[str, ...]:
        """The modes that stop the vehicle, most preferred first, from the configuration's `stop_mode` or the family's own."""
        raise NotImplementedError

    @property
    def kind(self) -> str:
        """What the vehicle is called to a person."""
        return self.family

    @property
    def base(self) -> str:
        """§5's base kind for the descriptor."""
        raise NotImplementedError

    # ── the codecs ───────────────────────────────────────────────────────────

    def mode_params(self, number: int) -> tuple[float, float, float]:
        """`DO_SET_MODE`'s first three parameters for a custom-mode word."""
        raise NotImplementedError

    def mode_label(self, number: int) -> str:
        """A mode word no table has, as a person would read it."""
        return f"mode {number}"

    def decode_param(self, raw: bytes, param_type: int) -> float:
        """A parameter's value, from the four bytes of `PARAM_VALUE`'s float field as the frame carried them and the type that came with it."""
        raise NotImplementedError

    def firmware(self, version: dict) -> tuple[str, str]:
        """`(version, build)` from `AUTOPILOT_VERSION`'s fields."""
        raise NotImplementedError

    def simulated(self, settings: dict, read_param) -> bool:
        """Whether the vehicle is a simulation, from what it says and from what the configuration asserts."""
        raise NotImplementedError

    def on_open(self) -> None:
        """Anything this autopilot's motions need asked of the vehicle once the link is up. Nothing is written to it."""

    def after_stop_request(self) -> None:
        """What makes a stop true on this vehicle beyond the mode it was put in, sent each time the stop is (re)asked for. It never waits behind a write in progress
        (the stop path's rule, `Link.try_send`), and `LinkClosed` is the caller's to swallow."""

    # ── the motions ──────────────────────────────────────────────────────────

    def names(self) -> tuple[str, ...]:
        raise NotImplementedError

    def motion_descriptors(self) -> list[dict]:
        """The descriptor's `motions` (§84.1), in the order `names()` gives them."""
        return [_DESCRIPTORS[name] for name in self.names()]

    def takeoff(self, args: dict) -> dict:
        raise NotImplementedError

    def goto(self, args: dict) -> dict:
        raise NotImplementedError

    def velocity(self, args: dict) -> dict:
        raise NotImplementedError

    def drive(self, args: dict) -> dict:
        """The sticks, for the vehicles that are moved by them. Only a motion set whose `names()` lists `drive` is asked."""
        raise NotImplementedError

    def surface(self, args: dict) -> dict:
        """Come up. Only a motion set whose `names()` lists `surface` is asked."""
        raise NotImplementedError


#: One entry per motion name; a motion set whose parameters differ builds its own.
_DESCRIPTORS: dict[str, dict] = {
    "status": {"name": "status", "parameters": []},
    "param": {"name": "param", "parameters": [{"name": "name", "required": True}]},
    "arm": {"name": "arm", "parameters": []},
    "disarm": {"name": "disarm", "parameters": [{"name": "force", "required": False}]},
    "mode": {"name": "mode", "parameters": [{"name": "name", "required": True}]},
    "hold": {"name": "hold", "parameters": []},
    "rtl": {"name": "rtl", "parameters": []},
    "goto": {"name": "goto", "parameters": [
        {"name": "x", "unit": "m", "required": False}, {"name": "y", "unit": "m", "required": False},
        {"name": "z", "unit": "m", "required": False}, {"name": "lat", "unit": "deg", "required": False},
        {"name": "lon", "unit": "deg", "required": False}, {"name": "alt", "unit": "m", "required": False}]},
    "takeoff": {"name": "takeoff", "parameters": [{"name": "alt", "unit": "m", "required": True, "min": 0.5}]},
    "land": {"name": "land", "parameters": []},
    "velocity": {"name": "velocity", "parameters": [
        {"name": "vx", "unit": "m/s", "required": False}, {"name": "vy", "unit": "m/s", "required": False},
        {"name": "vz", "unit": "m/s", "required": False}, {"name": "yaw_rate", "unit": "rad/s", "required": False}]},
    "surface": {"name": "surface", "parameters": []},
    # A fraction of a full stick, not a speed: ArduSub scales the sticks by the pilot's joystick gain (`JS_GAIN_DEFAULT`), and a thruster is not a velocity source.
    "drive": {"name": "drive", "parameters": [
        {"name": "surge", "unit": "stick", "required": False, "min": -1.0, "max": 1.0}, {"name": "sway", "unit": "stick", "required": False, "min": -1.0, "max": 1.0},
        {"name": "heave", "unit": "stick", "required": False, "min": -1.0, "max": 1.0}, {"name": "yaw", "unit": "stick", "required": False, "min": -1.0, "max": 1.0},
        {"name": "duration", "unit": "s", "required": True, "min": 0.1, "max": 5.0}]},
}

_COMMON = ("status", "param", "arm", "disarm", "mode", "hold", "rtl", "goto")


# ── ArduPilot ────────────────────────────────────────────────────────────────


class ArdupilotMotions(Motions):
    """ArduPilot's multirotor, fixed wing, ground rover and surface boat: a custom mode is a number, a setpoint is followed in guided."""

    dialect = "ardupilot"

    #: The modes the extension will select by name, per family. A mode that flies from stick input (stabilize, acro, flip, throw,
    #: autotune, ...) has nobody at the sticks, and this driver would be the thing that put the vehicle in it. `allow_modes` in the
    #: configuration widens this deliberately.
    _ALLOWED = {
        "copter": ("guided", "guided_nogps", "loiter", "poshold", "alt_hold", "brake", "land", "rtl", "smart_rtl", "auto", "circle"),
        "rover": ("guided", "hold", "loiter", "auto", "rtl", "smart_rtl"),
        "plane": ("guided", "loiter", "rtl", "auto", "circle", "takeoff", "qloiter", "qland", "qrtl"),
    }
    #: What stops each family, most preferred first: BRAKE on a multirotor (LOITER if BRAKE will not engage), HOLD on a rover, LOITER on a
    #: fixed wing — which circles, because a wing cannot stand still.
    _STOP = {"copter": ("brake", "loiter"), "rover": ("hold",), "plane": ("loiter",)}
    #: The modes in which the vehicle takes a position or velocity setpoint from a companion computer.
    _GUIDED = {"copter": ("guided", "guided_nogps"), "rover": ("guided",), "plane": ("guided",)}
    #: ArduPlane spells the link-loss parameter without the final E (ArduPilot's own name, held by the live block, which reads it off the firmware).
    _LINK_FAILSAFE = {"copter": "FS_GCS_ENABLE", "rover": "FS_GCS_ENABLE", "plane": "FS_GCS_ENABL"}

    @property
    def table(self) -> dict[str, int]:
        return FAMILY_MODES[self.family]

    @property
    def allowed(self) -> tuple[str, ...]:
        return self._ALLOWED[self.family]

    @property
    def link_failsafe_param(self) -> str:
        return self._LINK_FAILSAFE[self.family]

    def stop_chain(self, settings: dict) -> tuple[str, ...]:
        name = settings.get("stop_mode")
        chain = (name,) if name else self._STOP[self.family]
        for each in chain:
            if each not in self.table:
                raise BridgeRefusal(f"`stop_mode` {each!r} is not a {self.family} mode. The modes are {', '.join(sorted(self.table))}.")
        return chain

    @property
    def kind(self) -> str:
        return "boat" if self.hull == "surface" else self.family

    @property
    def base(self) -> str:
        return self.hull or ("aerial" if self.family in ("copter", "plane") else "wheeled")

    def mode_params(self, number: int) -> tuple[float, float, float]:
        return float(ModeFlag.CUSTOM_MODE_ENABLED), float(number), 0.0

    def decode_param(self, raw: bytes, param_type: int) -> float:
        # ArduPilot converts: an integer parameter's value is the number, in the float field.
        del param_type
        return struct.unpack("<f", raw)[0]

    def firmware(self, version: dict) -> tuple[str, str]:
        packed = version["flight_sw_version"]
        return (f"{packed >> 24}.{(packed >> 16) & 0xFF}.{(packed >> 8) & 0xFF}",
                bytes(version["flight_custom_version"]).rstrip(b"\0").decode("ascii", "replace"))

    def simulated(self, settings: dict, read_param) -> bool:
        # A SITL build has `SIM_*` parameters and a flight controller's build does not; a configuration can only make this more cautious.
        return bool(settings.get("simulator", False)) or read_param("SIM_SPEEDUP", 1.0) is not None

    def names(self) -> tuple[str, ...]:
        if self.family == "copter":
            return _COMMON + ("takeoff", "land", "velocity")
        if self.family == "rover":
            return _COMMON + ("velocity",)
        return _COMMON

    def takeoff(self, args: dict) -> dict:
        bridge = self.bridge
        altitude = bridge._number(args, "alt", required=True, minimum=0.5)
        bridge._no_arguments("takeoff", args)
        hb = bridge._latest_heartbeat()
        assert hb is not None
        if not hb["base_mode"] & ModeFlag.SAFETY_ARMED:
            raise Refused(rhal.SKR_EPERM, "the vehicle is not armed: `arm` first. A takeoff command to a disarmed vehicle is refused by the vehicle and would be answered here with its words, not a fix.")
        if bridge._mode_name(hb["custom_mode"]) not in self._GUIDED["copter"]:
            raise Refused(rhal.SKR_EPERM, f"the vehicle is in {bridge._mode_name(hb['custom_mode'])}, not guided: `mode name=guided` first. This driver does not change a vehicle's mode as a side effect of a motion.")
        bridge._run_command(Cmd.NAV_TAKEOFF, [0.0, 0.0, 0.0, math.nan, math.nan, math.nan, altitude], "takeoff")
        return {"takeoff_alt_m": altitude}

    def goto(self, args: dict) -> dict:
        bridge = self.bridge
        has_local = any(k in args for k in ("x", "y", "z"))
        has_global = any(k in args for k in ("lat", "lon", "alt"))
        if has_local == has_global:
            raise Refused(rhal.SKR_EINVAL, "goto takes either x= y= z= (metres, east-north-up from the EKF origin) or lat= lon= alt= (degrees, and metres above home) — not both, and one of them.")
        self._require_guided()
        if has_local:
            east = bridge._number(args, "x", required=True)
            north = bridge._number(args, "y", required=True)
            up = bridge._number(args, "z", required=self.family != "rover", default=0.0)
            bridge._no_arguments("goto", args)
            n, e, d = geometry.enu_to_ned(east, north, up)
            # Position only: ignore velocity (bits 3-5), acceleration (6-8), yaw (10) and yaw rate (11).
            bridge._send("SET_POSITION_TARGET_LOCAL_NED", {
                "x": n, "y": e, "z": d, "type_mask": 0x0DF8, "coordinate_frame": Frame.LOCAL_NED,
                "target_system": bridge._target_system, "target_component": bridge._target_component})
            bridge._commanded = True
            return {"target_enu_m": [east, north, up]}
        lat = bridge._number(args, "lat", required=True, minimum=-90.0, maximum=90.0)
        lon = bridge._number(args, "lon", required=True, minimum=-180.0, maximum=180.0)
        alt = bridge._number(args, "alt", required=self.family != "rover", default=0.0)
        bridge._no_arguments("goto", args)
        bridge._send("SET_POSITION_TARGET_GLOBAL_INT", {
            "lat_int": int(round(lat * 1e7)), "lon_int": int(round(lon * 1e7)), "alt": alt, "type_mask": 0x0DF8,
            "coordinate_frame": Frame.GLOBAL_RELATIVE_ALT_INT, "target_system": bridge._target_system,
            "target_component": bridge._target_component})
        bridge._commanded = True
        return {"target": {"lat": lat, "lon": lon, "alt_m": alt}}

    def velocity(self, args: dict) -> dict:
        bridge = self.bridge
        east = bridge._number(args, "vx", default=0.0)
        north = bridge._number(args, "vy", default=0.0)
        up = bridge._number(args, "vz", default=0.0)
        turning = "yaw_rate" in args
        yaw_rate = bridge._number(args, "yaw_rate", default=0.0)
        bridge._no_arguments("velocity", args)
        self._require_guided()
        vn, ve, vd = geometry.enu_to_ned(east, north, up)
        # Velocity only: ignore position (bits 0-2), acceleration (6-8) and yaw (10). The yaw *rate* (bit 11) is
        # ignored unless the caller gave one, because "no yaw rate" and "a yaw rate of zero" are different
        # sentences to a rover: ArduRover turns toward a velocity vector whose rate is ignored and drives
        # straight along its heading when the rate is zero (measured against its firmware, F179). A rate is in
        # the body's FLU convention (left is positive), and MAVLink's is about down (right is positive).
        bridge._send("SET_POSITION_TARGET_LOCAL_NED", {
            "vx": vn, "vy": ve, "vz": vd, "yaw_rate": -yaw_rate if turning else 0.0,
            "type_mask": 0x05C7 if turning else 0x0DC7, "coordinate_frame": Frame.LOCAL_NED,
            "target_system": bridge._target_system, "target_component": bridge._target_component})
        bridge._commanded = True
        return {"velocity_enu_m_s": [east, north, up], "yaw_rate_rad_s": yaw_rate if turning else None,
                "note": "one setpoint: the vehicle stops following it when GUID_TIMEOUT passes, so repeat it while it should continue"}

    def _require_guided(self) -> None:
        bridge = self.bridge
        hb = bridge._latest_heartbeat()
        assert hb is not None
        mode = bridge._mode_name(hb["custom_mode"])
        if not hb["base_mode"] & ModeFlag.SAFETY_ARMED:
            raise Refused(rhal.SKR_EPERM, "the vehicle is not armed. It ignores a setpoint without saying so, so this driver says it: `arm` first.")
        if mode not in self._GUIDED[self.family]:
            raise Refused(rhal.SKR_EPERM, f"the vehicle is in {mode}, not guided. It ignores a setpoint in any other mode without saying so: `mode name=guided` first.")


# ── ArduSub ──────────────────────────────────────────────────────────────────

#: The modes that take the pilot's sticks, which is what `MANUAL_CONTROL` is (ArduSub's joystick handler, `transform_manual_control_to_rc_override`). Of these `manual` has no depth
#: control (a neutral stick leaves the thrusters off and buoyancy decides), `stabilize` levels the vehicle, `alt_hold` holds depth and `poshold` holds position as well, which needs a position source.
_ARDUSUB_STICK_MODES = ("manual", "stabilize", "alt_hold", "poshold", "surftrak")
#: What may be named as a stop. Not `guided`, `auto` or `circle`, which go on moving, and not `acro` or `motordetect`, which are for a person at the sticks or a bench.
_ARDUSUB_STOP_MODES = ("alt_hold", "poshold", "manual", "stabilize", "surface")


class ArdusubMotions(ArdupilotMotions):
    """ArduSub: a submarine or a remotely operated vehicle, under §97's `submerged`.

    What differs from ArduPilot's other vehicles, each of it read out of ArduSub's own source at the pinned commit and held by the live block against its firmware:

    * there is **no takeoff, land or return to launch** — there is a ground nobody lands on, and the way out is `surface`;
    * the vehicle is moved by **sticks**: `MANUAL_CONTROL` carries x, y, r in -1000..1000 and z in 0..1000 (500 neutral), the vehicle keeps the last one for `RC_OVERRIDE_TIME`
      (3 s by default) after it stops, and so a stream that ends must end in neutral. That is `drive`, a stream of bounded length that always finishes with neutral frames;
    * `guided` needs a position estimate (a DVL, a USBL or the simulator's GPS), and an ROV without one has `drive` and nothing else;
    * **the stop is a decision the operator makes**, so it has no default: `alt_hold` keeps the depth and lets the vehicle drift, `poshold` keeps the place and needs a position
      source, `manual` and `stabilize` stop the thrusters and leave buoyancy to decide whether it rises or sinks, and `surface` ascends, which under ice or a tether is not a stop.
      A configuration without `stop_mode` is refused at open, naming the four, rather than choosing one for somebody who cannot be asked while the vehicle is under water.
    """

    #: The longest a `drive` may last. The call blocks for this long, so it is bounded where the caller can see it.
    MAX_DRIVE_S = 5.0
    _DRIVE_PERIOD_S = 0.05
    #: Neutral frames sent after a drive and after a stop: the vehicle holds the last override for seconds, and one datagram can be lost.
    _NEUTRAL_FRAMES = 3
    #: `MANUAL_CONTROL`'s z for a neutral throttle (0..1000, ArduSub's own).
    _Z_NEUTRAL = 500
    #: How far down still counts as at the surface: a vehicle's depth estimate wanders by a few centimetres at rest (measured: the position's z is not zero at the surface).
    _AT_SURFACE_M = 0.3

    _ALLOWED = {"sub": ("alt_hold", "poshold", "stabilize", "manual", "guided", "surface", "auto")}
    _GUIDED = {"sub": ("guided",)}
    _LINK_FAILSAFE = {"sub": "FS_GCS_ENABLE"}

    def stop_chain(self, settings: dict) -> tuple[str, ...]:
        name = settings.get("stop_mode")
        if not name:
            raise BridgeRefusal(
                "a submarine's stop is a decision this driver will not make for you, and `stop_mode` is not in the configuration. Name one of: "
                "`alt_hold` (keeps the depth, lets the vehicle drift), `poshold` (keeps the place; needs a position source such as a DVL), "
                "`manual` or `stabilize` (stops the thrusters; buoyancy decides whether it rises or sinks) or `surface` (ascends: not a stop under ice or on a tether). "
                "The vehicle was not commanded.")
        if name not in _ARDUSUB_STOP_MODES:
            raise BridgeRefusal(f"`stop_mode` {name!r} is not one a submarine can be stopped in. It is one of {', '.join(_ARDUSUB_STOP_MODES)}.")
        return (name,)

    @property
    def kind(self) -> str:
        return "submarine"

    def names(self) -> tuple[str, ...]:
        return ("status", "param", "arm", "disarm", "mode", "hold", "surface", "goto", "velocity", "drive")

    def takeoff(self, args: dict) -> dict:
        raise Refused(rhal.SKR_ENOSYS, "a submarine has no takeoff: `drive` it down, or `goto` a depth in guided.")

    def after_stop_request(self) -> None:
        # The mode is the stop; the sticks are what the vehicle is still obeying. Neutral replaces the last override at once, where waiting for it to expire is seconds of thrust.
        self.bridge._try_send("MANUAL_CONTROL", self._frame(0.0, 0.0, 0.0, 0.0), wait=0.05)

    def _frame(self, surge: float, sway: float, heave: float, yaw: float) -> dict:
        """The sticks, from the body's own frame (FLU, §44.4) to MAVLink's: x forward, y *right*, z up from neutral 500, r *clockwise*."""
        def stick(fraction: float) -> int:
            return int(round(max(-1.0, min(1.0, fraction)) * 1000))
        return {"x": stick(surge), "y": -stick(sway), "z": self._Z_NEUTRAL + int(round(max(-1.0, min(1.0, heave)) * 500)), "r": -stick(yaw),
                "buttons": 0, "target": self.bridge._target_system}

    def surface(self, args: dict) -> dict:
        """The vehicle is asked to come up, and the answer is where it is.

        ArduSub's `ModeSurface::run` ends the mode itself the moment the vehicle is at the surface ("already at surface, hold depth at surface": a switch to `alt_hold`, reason
        SURFACE_COMPLETE), so a vehicle that was there already leaves surface mode before its next heartbeat and is never seen in it. A driver that waited for the heartbeat to
        say `surface` would answer an error for a vehicle that did what it was asked, found by the live block's reference reading alt_hold and never surface. So `alt_hold` is an
        answer too, when the vehicle is at the surface, and an error when it is not (a request that the vehicle acknowledged and did nothing about leaves it where it was).

        **What the vehicle is in is what a heartbeat says that was sent after it answered** (F244). The heartbeat the driver already held was sent before the mode changed, and on a
        vehicle that sends one a second it is up to a second stale: reading it called a request that took one that did not, with the vehicle ascending, whenever it was at depth."""
        bridge = self.bridge
        bridge._no_arguments("surface", args)
        number, holding = self.table["surface"], self.table["alt_hold"]
        bridge._run_command(Cmd.DO_SET_MODE, list(self.mode_params(number)), "mode surface")
        answered = rhal.acquisition_ns()
        if not bridge._wait_for(lambda: self._heartbeat_after(answered) is not None, 4.0):
            raise Refused(rhal.SKR_EIO, "the vehicle accepted mode surface and sent no heartbeat after it answered, in four seconds.")
        reported = (self._heartbeat_after(answered) or {}).get("custom_mode")
        if reported == number:
            return {"mode": "surface"}
        if reported != holding:
            raise Refused(rhal.SKR_EIO, f"the vehicle accepted mode surface and its next heartbeat reports {bridge._mode_name(reported)}, which is neither surface nor the alt_hold ArduSub ends it in.")
        depth = self._depth_m()
        if depth is not None and depth <= self._AT_SURFACE_M:
            return {"mode": "alt_hold", "at_surface": True, "depth_m": round(depth, 2),
                    "note": "ArduSub ends surface mode itself when the vehicle is at the surface, and holds the depth there"}
        raise Refused(rhal.SKR_EIO, "the vehicle accepted mode surface and reports alt_hold, which ArduSub switches to when it is at the surface, but the vehicle is not at the surface "
                      + ("(its depth is unknown: there is no position estimate)." if depth is None else f"(it is {depth:.1f} m down). The request did not take; read `status` and ask again."))

    def _heartbeat_after(self, since_ns: int) -> dict | None:
        """The vehicle's heartbeat, if one arrived after `since_ns`: what it says it is in now, and not what it was in when the last one was sent."""
        bridge = self.bridge
        with bridge._lock:
            entry = bridge._heartbeats.get((bridge._target_system, bridge._target_component))
        return entry[0] if entry is not None and entry[1] > since_ns else None

    def _depth_m(self) -> float | None:
        """How far down the vehicle is, from its position estimate, or `None` when the estimate is absent or not recent (an ROV with no position source has none)."""
        held = self.bridge._latest.get("LOCAL_POSITION_NED")
        if held is None or rhal.acquisition_ns() - held[1] > 1.5e9:
            return None
        return held[0]["z"]

    def drive(self, args: dict) -> dict:
        bridge = self.bridge
        surge, sway, heave, yaw = (bridge._number(args, key, default=0.0, minimum=-1.0, maximum=1.0) for key in ("surge", "sway", "heave", "yaw"))
        duration = bridge._number(args, "duration", required=True, minimum=0.1, maximum=self.MAX_DRIVE_S)
        bridge._no_arguments("drive", args)
        self._require_sticks()
        command = self._frame(surge, sway, heave, yaw)
        neutral = self._frame(0.0, 0.0, 0.0, 0.0)
        sent = 0
        deadline = time.monotonic() + duration
        try:
            while not bridge._estop and time.monotonic() < deadline:
                bridge._send("MANUAL_CONTROL", command)
                bridge._commanded = True
                sent += 1
                time.sleep(self._DRIVE_PERIOD_S)
        finally:
            # Neutral, more than once, and whatever ended the stream: the vehicle keeps the last override for seconds, so a drive that stopped sending is not a drive that stopped.
            for _ in range(self._NEUTRAL_FRAMES):
                try:
                    bridge._send("MANUAL_CONTROL", neutral)
                except LinkClosed:
                    break
                time.sleep(self._DRIVE_PERIOD_S)
        if bridge._estop:
            raise Refused(rhal.SKR_EPERM, "the emergency stop was asserted while the drive was streaming. The stream has ended in neutral, and the stop is holding the vehicle.")
        return {"stick": {"surge": surge, "sway": sway, "heave": heave, "yaw": yaw}, "duration_s": duration, "frames": sent,
                "note": "a fraction of a full stick, scaled by the vehicle's joystick gain; the stream ended in neutral and the vehicle stays in its mode"}

    def _require_sticks(self) -> None:
        bridge = self.bridge
        hb = bridge._latest_heartbeat()
        assert hb is not None
        mode = bridge._mode_name(hb["custom_mode"])
        if not hb["base_mode"] & ModeFlag.SAFETY_ARMED:
            raise Refused(rhal.SKR_EPERM, "the vehicle is not armed, so its thrusters will not answer a stick: `arm` first.")
        if mode not in _ARDUSUB_STICK_MODES:
            raise Refused(rhal.SKR_EPERM, f"the vehicle is in {mode}, which does not take the sticks. `mode name=` one of {', '.join(_ARDUSUB_STICK_MODES)} first; "
                          "this driver does not change a vehicle's mode as a side effect of a motion.")


# ── PX4──────────────────────────────────────────────────────────────────────

#: `MAV_PARAM_TYPE` to the struct format of the integer PX4 copies, byte for byte, into the four bytes of `PARAM_VALUE`'s float field (F225).
_PX4_INTEGER_FORMATS = {1: "<B", 2: "<b", 3: "<H", 4: "<h", 5: "<I", 6: "<i"}
_PARAM_REAL32 = 9

_PX4_VELOCITY = {"name": "velocity", "parameters": [
    {"name": "vx", "unit": "m/s", "required": False}, {"name": "vy", "unit": "m/s", "required": False},
    {"name": "vz", "unit": "m/s", "required": False}, {"name": "yaw_rate", "unit": "rad/s", "required": False},
    {"name": "duration", "unit": "s", "required": True, "min": 0.1, "max": 5.0}]}


class Px4Motions(Motions):
    """PX4's multirotor, fixed wing and ground rover.

    What differs from ArduPilot, each of it measured against PX4's own SIH build (`DRY-COVERAGE-PLAN.md` item 12.E.1):

    * a custom mode is a packed word, and `DO_SET_MODE` carries its halves in two parameters;
    * a `DO_SET_MODE` to a mode PX4 will not enter is **acknowledged as accepted** and not made (F224), so the heartbeat is the only answer;
    * an integer parameter arrives as its own bytes in the float field (F225);
    * there is no guided mode to be in: a position is a `DO_REPOSITION`, which PX4 turns into a hold-and-go by itself, and which it ignores
      without a word on a disarmed vehicle;
    * a takeoff altitude is above sea level, not above home;
    * a velocity is only followed in offboard mode, which PX4 enters only while a setpoint stream is already running and **leaves for its
      own return-to-launch the moment the stream stops** (F226). So a velocity here is a stream of bounded length that ends by handing the
      vehicle to hold *before* it stops, and never a single setpoint.
    """

    dialect = "px4"

    #: The modes selected by name without `allow_modes`: the autonomous ones. The rest fly from stick input, and `offboard` from a stream only
    #: the velocity motion provides.
    _ALLOWED = {
        "copter": ("hold", "takeoff", "land", "rtl", "mission"),
        "plane": ("hold", "takeoff", "land", "rtl", "mission"),
        "rover": ("hold", "rtl", "mission"),
    }
    #: PX4's own radius of the earth (`CONSTANTS_RADIUS_OF_EARTH`, lib/geo): its local frame is a projection on a sphere of this size, so a local
    #: offset turned into a latitude and longitude with it lands where PX4's estimator would call that local point.
    _EARTH_RADIUS_M = 6371000.0
    #: The airframes that exist only in PX4's SITL build (`init.d-posix/airframes/10040_sihsim_*`). A vehicle running one is a simulation, whatever
    #: a configuration says; any other airframe may be a flight controller, and the configuration has to say which.
    _SIMULATION_AIRFRAMES = range(10040, 10050)
    #: The longest a velocity may last. The call blocks for this long (and a little more), so it is bounded where the caller can see it.
    MAX_VELOCITY_S = 5.0
    _STREAM_PERIOD_S = 0.05
    #: Setpoints sent before the switch to offboard, which PX4 will not make without a stream already running.
    _OFFBOARD_LEAD_S = 1.0
    _OFFBOARD_ENTER_S = 3.0
    #: How long the hand-over to hold may take, with the stream still running: the heartbeat that shows it comes once a second.
    _HANDOVER_S = 4.0
    #: A mode request is one datagram, so it is asked again this often until the heartbeat shows the mode (F231).
    _ASK_AGAIN_S = 1.0
    _FRESH_S = 3.0

    @property
    def table(self) -> dict[str, int]:
        return PX4_MODES

    @property
    def allowed(self) -> tuple[str, ...]:
        return self._ALLOWED[self.family]

    @property
    def link_failsafe_param(self) -> str:
        return "NAV_DLL_ACT"

    def stop_chain(self, settings: dict) -> tuple[str, ...]:
        name = settings.get("stop_mode")
        if name is None:
            return ("hold",)
        if name not in self.table or name == "offboard":
            raise BridgeRefusal(f"`stop_mode` {name!r} is not a PX4 mode that stops a vehicle. The modes are {', '.join(sorted(m for m in self.table if m != 'offboard'))}.")
        return (name,)

    @property
    def base(self) -> str:
        return "aerial" if self.family in ("copter", "plane") else "wheeled"

    def mode_params(self, number: int) -> tuple[float, float, float]:
        return float(ModeFlag.CUSTOM_MODE_ENABLED), float((number >> 16) & 0xFF), float((number >> 24) & 0xFF)

    def mode_label(self, number: int) -> str:
        return f"px4 mode main={(number >> 16) & 0xFF} sub={(number >> 24) & 0xFF}"

    def decode_param(self, raw: bytes, param_type: int) -> float:
        if param_type == _PARAM_REAL32:
            return struct.unpack("<f", raw)[0]
        fmt = _PX4_INTEGER_FORMATS.get(param_type)
        if fmt is None:
            raise Refused(rhal.SKR_ENOSYS, f"a parameter of MAV_PARAM_TYPE {param_type} does not fit the four bytes PX4 gives a value, so this driver cannot read it.")
        # The bytes, not a float made of them: -1 is 0xFFFFFFFF, a NaN, and a signalling NaN loses a bit on the way through a double (F229).
        return float(struct.unpack(fmt, raw[: struct.calcsize(fmt)])[0])

    def firmware(self, version: dict) -> tuple[str, str]:
        packed = version["flight_sw_version"]
        # PX4 packs the abbreviated git hash into the custom-version bytes least significant first; the tag's hash is those bytes reversed.
        return (f"{packed >> 24}.{(packed >> 16) & 0xFF}.{(packed >> 8) & 0xFF}",
                bytes(reversed(version["flight_custom_version"])).hex())

    def simulated(self, settings: dict, read_param) -> bool:
        claim = settings.get("simulator")
        if claim is not None and not isinstance(claim, bool):
            raise Refused(rhal.SKR_EINVAL, f"`simulator` of {claim!r} is not true or false.")
        airframe = read_param("SYS_AUTOSTART", 2.0)
        if airframe is not None and int(airframe) in self._SIMULATION_AIRFRAMES:
            if claim is False:
                raise Refused(
                    rhal.SKR_EINVAL,
                    f"`simulator` is false and the vehicle's airframe is {int(airframe)}, one that exists only in PX4's simulation build. "
                    "Say what the vehicle is: this driver will not describe a simulation as a flight controller.")
            return True
        if claim is None:
            raise Refused(
                rhal.SKR_EINVAL,
                f"PX4 does not say whether it is a simulation: its airframe (SYS_AUTOSTART {None if airframe is None else int(airframe)}) is not one of the "
                "simulation-only ones this driver recognises, and a flight controller and a Gazebo vehicle look alike on the wire. Set `simulator` "
                "to true or false. A bench run described as a simulation costs a tier it could have had; a simulation described as a bench run is a "
                "false claim, so there is no default.")
        return claim

    def on_open(self) -> None:
        """Ask for the origin of the vehicle's local frame, which a local goto needs and PX4 does not stream."""
        bridge = self.bridge
        bridge._command(Cmd.REQUEST_MESSAGE, [float(SPECS["GPS_GLOBAL_ORIGIN"].msg_id)], tries=2, timeout=1.0)
        bridge._wait_for(lambda: "GPS_GLOBAL_ORIGIN" in bridge._latest, 2.0)

    def names(self) -> tuple[str, ...]:
        if self.family == "copter":
            return _COMMON + ("takeoff", "land", "velocity")
        if self.family == "plane":
            return _COMMON + ("takeoff",)
        return _COMMON + ("velocity",)

    def motion_descriptors(self) -> list[dict]:
        return [_PX4_VELOCITY if name == "velocity" else _DESCRIPTORS[name] for name in self.names()]

    # ── takeoff, goto, velocity ──────────────────────────────────────────────

    def takeoff(self, args: dict) -> dict:
        bridge = self.bridge
        altitude = bridge._number(args, "alt", required=True, minimum=0.5)
        bridge._no_arguments("takeoff", args)
        self._require_armed("takeoff")
        amsl = self._home_amsl() + altitude
        # PX4 reads param7 as an altitude above mean sea level, takes the position (param5, param6) as where it is when they are NaN, puts itself
        # in its takeoff mode and returns to hold on arrival. It is the one motion that changes the mode as a side effect, and it is PX4 that does.
        bridge._run_command(Cmd.NAV_TAKEOFF, [math.nan, 0.0, 0.0, math.nan, math.nan, math.nan, amsl], "takeoff")
        return {"takeoff_alt_m": altitude, "takeoff_amsl_m": amsl,
                "note": "PX4 enters its own takeoff mode and returns to hold when it arrives"}

    def goto(self, args: dict) -> dict:
        bridge = self.bridge
        has_local = any(k in args for k in ("x", "y", "z"))
        has_global = any(k in args for k in ("lat", "lon", "alt"))
        if has_local == has_global:
            raise Refused(rhal.SKR_EINVAL, "goto takes either x= y= z= (metres, east-north-up from the EKF origin) or lat= lon= alt= (degrees, and metres above home) — not both, and one of them.")
        if has_local:
            east = bridge._number(args, "x", required=True)
            north = bridge._number(args, "y", required=True)
            up = self._altitude(args, "z")
            bridge._no_arguments("goto", args)
            self._require_armed("goto")
            lat, lon, amsl = self._from_local(east, north, up)
            self._reposition(lat, lon, amsl)
            return {"target_enu_m": [east, north, math.nan if up is None else up]}
        lat = bridge._number(args, "lat", required=True, minimum=-90.0, maximum=90.0)
        lon = bridge._number(args, "lon", required=True, minimum=-180.0, maximum=180.0)
        alt = self._altitude(args, "alt")
        bridge._no_arguments("goto", args)
        self._require_armed("goto")
        self._reposition(lat, lon, math.nan if alt is None else self._home_amsl() + alt)
        return {"target": {"lat": lat, "lon": lon, "alt_m": alt}}

    def _altitude(self, args: dict, key: str) -> float | None:
        """A goto's altitude: required of a vehicle that flies, and of a rover only if it gave one (it has none to give up)."""
        if self.family == "rover" and key not in args:
            return None
        return self.bridge._number(args, key, required=True)

    def velocity(self, args: dict) -> dict:
        bridge = self.bridge
        east = bridge._number(args, "vx", default=0.0)
        north = bridge._number(args, "vy", default=0.0)
        up = bridge._number(args, "vz", default=0.0)
        turning = "yaw_rate" in args
        yaw_rate = bridge._number(args, "yaw_rate", default=0.0)
        duration = bridge._number(args, "duration", required=True, minimum=0.1, maximum=self.MAX_VELOCITY_S)
        bridge._no_arguments("velocity", args)
        self._require_armed("velocity")
        vn, ve, vd = geometry.enu_to_ned(east, north, up)
        move = self._velocity_setpoint(vn, ve, vd, -yaw_rate if turning else 0.0, turning)
        halt = self._velocity_setpoint(0.0, 0.0, 0.0, 0.0, False)
        offboard, hold = self.table["offboard"], self.table["hold"]

        # PX4 enters offboard only with a stream already running, and answers a switch it will not make as if it had made it (F224): so lead with
        # the stream, ask, and believe the heartbeat.
        self._stream_or_stopped(move, self._OFFBOARD_LEAD_S)
        if not self._stream_into(move, offboard, self._OFFBOARD_ENTER_S):
            bridge._send_mode(hold, wait=0.2)
            raise Refused(rhal.SKR_EIO, f"PX4 acknowledged offboard mode and its heartbeat did not show it after {self._OFFBOARD_ENTER_S:.0f} seconds. "
                          "The vehicle was not moved and was asked for hold.")
        bridge._commanded = True
        self._stream_or_stopped(move, duration)
        # The hand-over: hold is asked for while the stream is still running, because a stream that simply stops is what PX4 answers by flying
        # home (F226). Zero velocity until the heartbeat shows hold.
        if not self._stream_into(halt, hold, self._HANDOVER_S):
            raise Refused(rhal.SKR_EIO, f"the vehicle was asked for hold and its heartbeat still shows offboard after {self._HANDOVER_S:.0f} seconds. The stream has stopped, "
                          "so PX4's own offboard-loss failsafe acts: `hold` or `rtl` now, and read `status`.")
        return {"velocity_enu_m_s": [east, north, up], "yaw_rate_rad_s": yaw_rate if turning else None, "duration_s": duration,
                "note": "streamed for the duration, then handed to hold"}

    # ── what the three share ─────────────────────────────────────────────────

    def _require_armed(self, motion: str) -> None:
        if not self.bridge._is_armed():
            raise Refused(rhal.SKR_EPERM, f"the vehicle is not armed. PX4 acknowledges a {motion} to a disarmed vehicle, carries nothing out, and changes its mode as it does "
                          "(measured: a takeoff leaves it in takeoff mode, a reposition in hold), so this driver refuses instead of sending it: `arm` first (F227).")

    def _home_amsl(self) -> float:
        """The altitude of home above sea level, which PX4's altitude is measured from, as the vehicle's own global position gives it."""
        bridge = self.bridge
        with bridge._lock:
            entry = bridge._latest.get("GLOBAL_POSITION_INT")
        if entry is None or rhal.acquisition_ns() - entry[1] > self._FRESH_S * 1e9:
            raise Refused(rhal.SKR_EPERM, "the vehicle has no recent global position (GLOBAL_POSITION_INT). PX4 takes altitude above sea level and this driver "
                          "turns the caller's metres above home into that from the position the vehicle reports: without a fix there is nothing to turn it with.")
        fields = entry[0]
        return (fields["alt"] - fields["relative_alt"]) / 1000.0

    def _from_local(self, east: float, north: float, up: float | None) -> tuple[float, float, float]:
        """A point given east, north and up of the origin of PX4's local frame, as latitude, longitude and altitude above sea level."""
        bridge = self.bridge
        with bridge._lock:
            entry = bridge._latest.get("GPS_GLOBAL_ORIGIN")
        if entry is None:
            raise Refused(rhal.SKR_EPERM, "the vehicle has not said where its local frame's origin is (GPS_GLOBAL_ORIGIN), so x= y= z= cannot be turned into a position PX4 "
                          "understands. Use lat= lon= alt=, or try again once the vehicle has a position estimate.")
        origin = entry[0]
        lat0, lon0, alt0 = origin["latitude"] / 1e7, origin["longitude"] / 1e7, origin["altitude"] / 1000.0
        lat = lat0 + math.degrees(north / self._EARTH_RADIUS_M)
        lon = lon0 + math.degrees(east / (self._EARTH_RADIUS_M * math.cos(math.radians(lat0))))
        return lat, lon, math.nan if up is None else alt0 + up

    def _reposition(self, lat: float, lon: float, amsl: float) -> None:
        # `DO_REPOSITION` over COMMAND_INT, because a latitude in a COMMAND_LONG is a float and has a metre's resolution at these latitudes.
        # Speed -1 is the vehicle's default, and bit 0 of the flags asks it to change mode to carry the reposition out.
        self.bridge._run_command(Cmd.DO_REPOSITION, [-1.0, 1.0, 0.0, math.nan], "goto",
                                 position=(Frame.GLOBAL, int(round(lat * 1e7)), int(round(lon * 1e7)), amsl))

    def _velocity_setpoint(self, vn: float, ve: float, vd: float, yaw_rate: float, turning: bool) -> dict:
        bridge = self.bridge
        # Velocity only: ignore position (bits 0-2), acceleration (6-8) and yaw (10); the yaw rate (bit 11) only when the caller gave one (F179).
        return {"vx": vn, "vy": ve, "vz": vd, "yaw_rate": yaw_rate, "type_mask": 0x05C7 if turning else 0x0DC7,
                "coordinate_frame": Frame.LOCAL_NED, "target_system": bridge._target_system, "target_component": bridge._target_component}

    def _in_mode(self, word: int) -> bool:
        return (self.bridge._latest_heartbeat() or {}).get("custom_mode") == word

    def _stream_into(self, setpoint: dict, word: int, seconds: float) -> bool:
        """Stream `setpoint` and ask for the mode `word`, until the heartbeat shows it or `seconds` have passed; whether it showed.

        The request is asked again every `_ASK_AGAIN_S`. It is one datagram with nothing behind it (PX4 acknowledges a mode it will not enter, F224, so an
        acknowledgement is not waited for), and on a link that loses one in thirty a lost request for hold left the vehicle in offboard with nothing
        streaming, which PX4 answers by flying home. Asking again is harmless to a mode already entered, and bounded by the window."""
        next_ask = 0.0

        def asked_until_shown() -> bool:
            nonlocal next_ask
            now = time.monotonic()
            if now >= next_ask:
                self.bridge._send_mode(word, wait=0.2)
                next_ask = now + self._ASK_AGAIN_S
            return self._in_mode(word)

        return self._stream_or_stopped(setpoint, seconds, until=asked_until_shown)

    def _stream_or_stopped(self, setpoint: dict, seconds: float, until=None) -> bool:
        """`_stream`, except that a stop latched while it ran is an error: the stop owns the vehicle from then on."""
        held = self._stream(setpoint, seconds, until)
        if self.bridge._estop:
            raise Refused(rhal.SKR_EPERM, "the emergency stop was asserted while the velocity was being streamed. The stream has ended, and the stop is holding the vehicle.")
        return held

    def _stream(self, setpoint: dict, seconds: float, until=None) -> bool:
        """Send `setpoint` every 50 ms for `seconds`, or until `until()` holds; whether it held (true for a plain stream that ran its time).
        It returns at once when the stop is latched, so a stop never waits behind a stream."""
        bridge = self.bridge
        deadline = time.monotonic() + seconds
        while not bridge._estop:
            bridge._send("SET_POSITION_TARGET_LOCAL_NED", setpoint)
            if until is not None and until():
                return True
            if time.monotonic() >= deadline:
                return until is None
            time.sleep(self._STREAM_PERIOD_S)
        return False


# ── choosing ─────────────────────────────────────────────────────────────────

def select_motions(bridge, heartbeat: dict) -> Motions:
    """The motion set for the vehicle that sent `heartbeat`, or a `BridgeRefusal` that says why this driver does not drive it."""
    autopilot, vehicle = heartbeat["autopilot"], heartbeat["type"]
    if autopilot == Autopilot.PX4:
        if vehicle in MavType.MULTIROTORS:
            return Px4Motions(bridge, "copter")
        if vehicle == MavType.FIXED_WING:
            return Px4Motions(bridge, "plane")
        if vehicle == MavType.GROUND_ROVER:
            return Px4Motions(bridge, "rover")
        if vehicle in MavType.VTOLS:
            raise BridgeRefusal(
                f"the vehicle is a VTOL (MAV_TYPE {vehicle}) running PX4. Its modes are a plane's and a copter's at once and the transition between them is not a mode "
                "this driver sets; it holds one family per vehicle and has driven neither.")
        raise BridgeRefusal(
            f"the vehicle reports MAV_TYPE {vehicle} under PX4, which is not a multirotor, a fixed wing or a ground rover — the three this driver has driven against "
            "PX4's own simulation. A boat or a submarine under PX4 is a different frame with different modes, and neither has been run.")
    if autopilot != Autopilot.ARDUPILOTMEGA:
        raise BridgeRefusal(
            f"the autopilot is MAV_AUTOPILOT {autopilot}. This driver speaks ArduPilot (whose custom-mode numbers it holds against the reference) and "
            "PX4 (whose packed mode word it holds against the same): it has not met this one, and a driver that guessed at its modes would be the one that set them wrong."
        )
    if vehicle in MavType.MULTIROTORS:
        return ArdupilotMotions(bridge, "copter")
    if vehicle == MavType.FIXED_WING:
        return ArdupilotMotions(bridge, "plane")
    if vehicle == MavType.GROUND_ROVER:
        return ArdupilotMotions(bridge, "rover")
    if vehicle == MavType.SURFACE_BOAT:
        # ArduRover built with a boat frame: Rover's modes and Rover's motions, and §97's `surface` for a base.
        return ArdupilotMotions(bridge, "rover", hull="surface")
    if vehicle == MavType.SUBMARINE:
        # ArduSub: its own modes, sticks to move it, and a stop the operator names (`ArdusubMotions`). §97's `submerged` is the base.
        return ArdusubMotions(bridge, "sub", hull="submerged")
    if vehicle in MavType.VTOLS:
        raise BridgeRefusal(f"the vehicle is a VTOL (MAV_TYPE {vehicle}). Its modes are a plane's and a copter's at once; this driver holds one family per vehicle and has met neither.")
    raise BridgeRefusal(f"the vehicle reports MAV_TYPE {vehicle}, which is not a multirotor, a fixed wing or a ground rover.")

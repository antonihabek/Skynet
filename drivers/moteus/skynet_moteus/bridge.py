"""mjbots moteus controllers over CAN-FD: one Skynet joint per controller, on one bus.

The wire is moteus's register protocol (:mod:`skynet_moteus.protocol`): a command is a frame that opens with the mode and writes the control law's
fields, a query is a frame that reads, and a controller answers only when the sender asks it to. What the driver speaks of it:

| Asked, every cycle | mode, position, velocity, torque, home state, voltage, temperature and the fault register |
| Read at open | each controller's own travel (`conf get servopos.position_min` / `position_max`, over the diagnostic channel) |
| Commanded, every cycle | *position* mode with all six control fields written; velocity is that mode with no position, torque is that mode with both gains at zero |
| Stopped by | the mode the operator named: *zero velocity*, *brake* or *stopped* (`stop_mode`, required) |

# What it reads, and what it will not invent

**A command stream is a lease.** A moteus holds a command for its watchdog and then falls back (mode 11, *timeout*), whatever the host is doing. So every
cycle sends every axis a command — a masked joint is *held*, by a position command at the target it was last given, never skipped — and the watchdog is
the controller's own (`watchdog_ms`, or the controller's configured default when it is left out; this driver never writes "no timeout"). The hand-back is
silence for that reason, as it is for Piper: `close` sends nothing, and a driver that stops talking leaves the controller to its configured fallback.

**Health is the mode register and the fault register together.** Mode 1 carries a fault code (32 to 50); mode 11 is the watchdog having run out; the
non-zero codes 96 to 105 in a valid mode are *which function is limiting the output* and not faults (the register reference lists 96 to 103; the firmware sends 104 and 105 too). In a session only *position* is the mode this driver put an
axis in, so anything else — stopped, preparing, zero velocity, another master's voltage or current command — is something done to the axis by someone
else, and it is **refused, never repaired**: a driver that re-enabled an axis a person had stopped, by writing a position command into it, would be starting
a machine whose reason for stopping nobody had looked at. `open` is the person's act and the only place an axis is brought up.

**The stop is the operator's choice, and none of the three holds a load.** `zero velocity` is a derivative-only controller (it brings velocity to zero and holds
nothing against gravity), `brake` shorts the phases, `stopped` coasts. Which is right depends on the robot, so `stop_mode` has no default. The first two leave the
motor driver on, and a position command switches back from them with no interval in which the axis is limp (`ISR_MaybeChangeMode`, read for this driver), so
`release_stop` clears the latch; `stopped` goes back through the controller's calibrating phase with the driver off, and `release_stop` answers ENOSYS for it.

**Travel is the controller's own, narrowed by the operator, never widened.** `servopos.position_min` and `position_max` are what the controller itself enforces
(it faults on starting position control outside them and clamps its target to them), so they are the declared travel; the operator may narrow them with `limits`
(radians) and may not widen them (F181's rule). A controller with neither bound and an operator with no `limits` is a continuous joint: nothing declares a travel
that is not there. A controller that does not answer the diagnostic channel and an operator who gave no `limits` is refused, not guessed at.

**Effort is the controller's torque, at the output shaft, in newton metres.** That is what register 0x003 is (`registers.md`); it is not NaN here, unlike a
driver whose wire reports amps.

# Modes

Position, velocity and torque, per joint (§86.1). All three are the same command with different fields: position is `position`; velocity writes no position
(NaN: the controller advances its own target at the velocity) and keeps the gains; torque writes both gains as zero and the torque as the feedforward. Every
command carries every field, so there is no mode to remember and no stale one to send.
"""

from __future__ import annotations

import json
import math
import time

import skynet_rhal as rhal

from . import protocol as p
from .bus import Bus, FdFrame, SocketCanFdBus

__all__ = ["MoteusBridge", "Axis", "BridgeRefusal"]

_TWO_PI = 2.0 * math.pi

#: How long to wait for one controller to answer one query before giving up on the body.
_REQUEST_TIMEOUT_S = 0.5
#: How long `open()` waits for an axis it has told to hold to report position mode: the controller's preparing phase is a few milliseconds, and a second is margin.
_ENABLE_TIMEOUT_S = 1.0
#: How long one `conf get` waits for its line.
_CONF_TIMEOUT_S = 0.5
#: How long `read_state` waits, after its queries, for every axis's reply: the default, which `window_ms` replaces.
_WINDOW_S = 0.050
#: How old a status may be before the axis it speaks for reads degraded.
_STALE_S = 1.0
#: The most frames one wait applies before giving up whatever the clock says (a clock a test owns does not advance).
_WAIT_FRAME_CAP = 20_000
#: Slack on the travel check at open, in revolutions: the position is a float and a joint at its limit reads a hair either side.
_TRAVEL_SLACK_REV = 1e-3

#: What `stop_mode` may be, and the register value each writes.
_STOP_MODES = {"zero_velocity": p.MODE_ZERO_VELOCITY, "brake": p.MODE_BRAKE, "stopped": p.MODE_STOPPED}
#: The modes in which an axis may be found at open: it is ours to take over (stopped) or already holding (position, and the two stops this driver writes).
_OPENABLE = (p.MODE_STOPPED, p.MODE_POSITION, p.MODE_ZERO_VELOCITY, p.MODE_BRAKE)

#: §5.1's canonical chains and axes (the ODrive and Piper drivers' validator, with its pointer).
_CHAINS = frozenset({"left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand", "torso", "neck", "head", "base", "tail", "aux"})
_AXES = frozenset({"pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip"})


def _normalise_role(role: object) -> str | None:
    """The canonical form of a §5.1 role, or None when it does not parse."""
    if not isinstance(role, str):
        return None
    text = role.strip().lower()
    if not text:
        return None
    head, _, tail = text.partition("#")
    index: str | None = None
    if "#" in text:
        if not tail or len(tail) > 10 or not tail.isascii() or not tail.isdigit() or int(tail) > 0xFFFFFFFF:
            return None
        index = str(int(tail))
    parts = head.split(".")
    if len(parts) not in (2, 3):
        return None
    if parts[0] not in _CHAINS:
        return None
    if not parts[1] or any(not piece for piece in parts[2:]):
        return None
    if len(parts) == 3 and parts[2] not in _AXES:
        return None
    canonical = ".".join(parts)
    if index is not None:
        canonical += "#" + index
    return canonical


class BridgeRefusal(Exception):
    """A reason this body did not come up, in the terms an integrator acts on."""


class Axis:
    """One moteus controller, and the last thing it said."""

    def __init__(self, ident: int, name: str, role: str | None, operator_limits: tuple[float, float] | None) -> None:
        self.ident = ident
        self.name = name
        self.role = role
        self.operator_limits = operator_limits
        #: The declared travel in radians, or None for a continuous joint.
        self.travel: tuple[float, float] | None = None
        self.status: p.Status | None = None
        self.status_at: float | None = None
        #: §24's hold: the last position setpoint (radians) and whether there is one, and the latched hold target.
        self.setpoint = 0.0
        self.have_setpoint = False
        self.hold = 0.0
        self.holding = False

    def mode_name(self) -> str:
        if self.status is None:
            return "no status yet"
        return p.MODES.get(self.status.mode, f"mode {self.status.mode}")


class MoteusBridge:
    """A body made of moteus controllers."""

    def __init__(self, bus: Bus | None = None, clock=time.monotonic) -> None:
        self._bus = bus
        self._owns_bus = bus is None
        self._clock = clock
        self._simulated = True
        self.axes: list[Axis] = []
        self._descriptor: str | None = None
        self._seq = 0
        self._estop = False
        self._awaiting_resume = False
        self._opened = False
        self._source = 0
        self._stop_mode = p.MODE_ZERO_VELOCITY
        self._watchdog_s: float | None = None
        self._max_torque = math.nan
        self._window_s = _WINDOW_S
        #: Frames that were not a controller's reply to this driver, or that did not parse. Counted, never dropped in silence.
        self.unexpected_frames = 0

    # ── §4's entry points ────────────────────────────────────────────────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError as error:
            _say(f"the configuration is not JSON: {error}")
            return rhal.SKR_EINVAL
        if not isinstance(settings, dict):
            _say("the configuration is JSON but not an object.")
            return rhal.SKR_EINVAL
        # A body, and the stop latched on it, belong to one session.
        self.axes = []
        self._descriptor = None
        self._estop = False
        self._awaiting_resume = False
        try:
            self._open(settings)
        except BridgeRefusal as refusal:
            _say(str(refusal))
            return rhal.SKR_ENODEV
        except OSError as error:
            _say(f"the bus could not be opened: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _open(self, settings: dict) -> None:
        stop = settings.get("stop_mode")
        if stop not in _STOP_MODES:
            raise BridgeRefusal(
                f"`stop_mode` is {stop!r} and has no default. What a stop does to a moteus axis is the integrator's risk assessment, not this driver's: "
                "`zero_velocity` brings velocity to zero and holds nothing against a load, `brake` shorts the phases, `stopped` lets the shaft coast, and none of "
                "the three holds a loaded joint. Give one of \"zero_velocity\", \"brake\", \"stopped\"."
            )
        self._stop_mode = _STOP_MODES[stop]
        window_ms = settings.get("window_ms", _WINDOW_S * 1000.0)
        if isinstance(window_ms, bool) or not isinstance(window_ms, (int, float)) or not 0.0 < window_ms <= 1000.0:
            raise BridgeRefusal(
                f"`window_ms` of {window_ms!r} is not a window. It bounds how long a read waits for every axis to answer its query, in milliseconds, above zero "
                "and at most a second — a longer one would be a control loop waiting on a bus that has gone."
            )
        self._window_s = float(window_ms) / 1000.0
        watchdog_ms = settings.get("watchdog_ms")
        if watchdog_ms is not None and (isinstance(watchdog_ms, bool) or not isinstance(watchdog_ms, (int, float)) or not 1.0 <= watchdog_ms <= 10_000.0):
            raise BridgeRefusal(
                f"`watchdog_ms` of {watchdog_ms!r} is not a watchdog. It is how long a controller keeps a command before it falls back to its configured "
                "timeout behaviour, 1 ms to 10 s; leave it out to use each controller's own configured default. \"No timeout\" is not an option this driver offers."
            )
        self._watchdog_s = None if watchdog_ms is None else float(watchdog_ms) / 1000.0
        max_torque = settings.get("max_torque_nm")
        if max_torque is not None and (isinstance(max_torque, bool) or not isinstance(max_torque, (int, float)) or not math.isfinite(max_torque) or max_torque <= 0.0):
            raise BridgeRefusal(f"`max_torque_nm` of {max_torque!r} is not a torque ceiling: give a number above zero, or leave it out for each controller's own.")
        self._max_torque = math.nan if max_torque is None else float(max_torque)
        source = settings.get("source", 0)
        if isinstance(source, bool) or not isinstance(source, int) or not 0 <= source <= 0x7F:
            raise BridgeRefusal(f"`source` of {source!r} is not a 7-bit sender id (0 to 127).")
        self._source = source

        joints = settings.get("joints") or []
        if not joints:
            raise BridgeRefusal(
                "no joints in the configuration. This driver cannot discover them: an id says which controller on the bus, and nothing on the bus says which joint of "
                'which body it is, nor what it should be called. Give `{"joints": [{"id": 1, "name": "J1", "role": "right_arm.shoulder.pitch"}], '
                '"stop_mode": "zero_velocity"}`.'
            )
        for entry in joints:
            ident = entry.get("id")
            name = entry.get("name")
            if isinstance(ident, bool) or not isinstance(ident, int) or not 1 <= ident <= 127:
                raise BridgeRefusal(f"`{ident}` is not a moteus id (1 to 127; 0 is the broadcast address and no controller has it).")
            if not name:
                raise BridgeRefusal(f"controller {ident} has no `name`; a joint the OS cannot name is one no policy channel can bind to.")
            if any(axis.ident == ident for axis in self.axes):
                raise BridgeRefusal(f"controller {ident} is named by two joints. One controller is one joint: two bound to it would send it two setpoints a cycle.")
            if any(axis.name == name for axis in self.axes):
                raise BridgeRefusal(f"two joints are called `{name}`; a joint is addressed by its name.")
            role = entry.get("role")
            if role is not None and _normalise_role(role) is None:
                raise BridgeRefusal(
                    f"`{role}` is not a §5.1 role (chain.link[.axis][#index] over the canonical chains and axes). Roles are the operator's and are never inferred."
                )
            limits = entry.get("limits")
            operator_limits: tuple[float, float] | None = None
            if limits is not None:
                if (not isinstance(limits, (list, tuple)) or len(limits) != 2 or any(isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) for v in limits)
                        or not limits[0] < limits[1]):
                    raise BridgeRefusal(f"`limits` of {limits!r} on {name} is not [min, max] in radians with min below max.")
                operator_limits = (float(limits[0]), float(limits[1]))
            self.axes.append(Axis(ident, str(name), _normalise_role(role) if role is not None else None, operator_limits))

        if self._bus is None:
            self._bus = SocketCanFdBus(settings.get("interface", "vcan0"))
        # What is already on the bus answers an earlier session: discarded, never applied. A stale status would refuse this body for the last session's fault, or
        # clear it for the last session's health.
        waiting = self._bus.backlog()
        for _ in range(_WAIT_FRAME_CAP if waiting is None else waiting):
            if self._bus.recv() is None:
                break
        # §35.3: the dangerous direction is declaring `false` about something that is not real, and a moteus and a simulator of one answer identically.
        self._simulated = bool(settings.get("simulator", True)) or self._bus.synchronous

        # Order matters: everything is read before anything is commanded.
        for axis in self.axes:
            self._refuse_if_absent(axis)
        for axis in self.axes:
            self._refuse_if_not_ours(axis)
        for axis in self.axes:
            self._read_travel(axis)
        for axis in self.axes:
            self._refuse_if_outside_travel(axis)
        self._enable()
        for axis in self.axes:
            position = self._rad(axis)
            axis.setpoint = position
            axis.hold = position
        self._descriptor = self._build_descriptor()

    def close(self) -> int:
        # Nothing is sent. A moteus that is told to stop coasts, one that is put in a stop mode is no longer held to anything, and one that is simply no longer spoken to
        # runs its own watchdog down and falls back to the behaviour its owner configured: the hand-back that decides nothing this driver has no business deciding.
        if self._bus is not None and self._owns_bus:
            self._bus.close()
            self._bus = None
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        assert self._bus is not None
        # What is waiting answers an earlier cycle: applied, because a late reading is newer than none, and taken before this cycle's queries so the replies this
        # read waits for are the ones it asked for.
        self._drain()
        # §24: stamped at acquisition: the instant the first query leaves.
        acquired = rhal.acquisition_ns()
        for axis in self.axes:
            self._send(axis, p.query(), reply=True)
        seen = self._collect()
        self._drain()
        self._seq += 1

        out.stamp_ns = acquired
        out.seq = self._seq
        out.flags = 0
        out.fault_code = 0
        out.position_rad = []
        out.velocity_rad_s = []
        out.effort_nm = []
        now = self._clock()
        faulted = False
        unwell = False
        homed = True
        fault_word = 0
        degraded_word = 0
        for index, axis in enumerate(self.axes):
            status = axis.status
            if status is None:
                out.position_rad.append(math.nan)
                out.velocity_rad_s.append(math.nan)
                out.effort_nm.append(math.nan)
                faulted = True
                continue
            out.position_rad.append(status.position * _TWO_PI)
            out.velocity_rad_s.append(status.velocity * _TWO_PI)
            out.effort_nm.append(status.torque)
            if status.home_state != 2:
                homed = False
            verdict = self._judge(axis)
            # The maker's own numbers, composed and not translated (§24): which joint, the mode and the fault register, so the black box says what a person looks
            # up. One word fits one axis: the first that is faulted, else the first that is only degraded (a limiting code names which function is limiting).
            word = ((index + 1) << 16) | ((status.mode & 0xFF) << 8) | (status.fault & 0xFF)
            if verdict == "fault":
                faulted = True
                if fault_word == 0:
                    fault_word = word
            elif verdict == "degraded":
                unwell = True
                if degraded_word == 0:
                    degraded_word = word
            if axis.status_at is not None and now - axis.status_at > _STALE_S:
                unwell = True
        out.fault_code = fault_word or degraded_word
        out.flags = rhal.HOMING_COMPLETE if homed and self.axes else rhal.HOMING_PENDING
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP
        if faulted:
            out.flags |= rhal.FLAG_DEGRADED | rhal.FLAG_FAULT
        elif unwell:
            out.flags |= rhal.FLAG_DEGRADED
        if 0 < seen < len(self.axes):
            # Some axes answered this cycle and some did not: some readings above are a period or more old under a fresh stamp. Reported, not smoothed over.
            out.flags |= rhal.FLAG_DEGRADED
        if seen == 0:
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        count = len(self.axes)
        if cmd.joint_count != count:
            return rhal.SKR_EINVAL
        try:
            modes = [cmd.mode_of(index) for index in range(count)]
        except ValueError:
            return rhal.SKR_EINVAL
        known = (rhal.MODE_POSITION, rhal.MODE_VELOCITY, rhal.MODE_TORQUE)
        if cmd.mode not in known or any(mode not in known for mode in modes):
            return rhal.SKR_EINVAL
        applied = [bool(cmd.mask[index]) if index < len(cmd.mask) else True for index in range(count)]
        for index in range(count):
            if applied[index] and not math.isfinite(cmd.values[index]):
                _say(f"the setpoint for {self.axes[index].name} is not a finite number. The whole command is refused; send a number (§24). A NaN position would mean "
                     "'hold where you are' to a moteus, which is not what a caller who sent one asked for.")
                return rhal.SKR_EINVAL
        if self._estop:
            return rhal.SKR_EPERM
        problems = self._problems()
        if problems:
            _say("refused — " + "; ".join(problems) + ". Nothing this driver sends will move the body until that is cleared at the controller, and `open` is the act that brings an axis up.")
            return rhal.SKR_EPERM
        for index, axis in enumerate(self.axes):
            if not applied[index] or modes[index] != rhal.MODE_POSITION or axis.travel is None:
                continue
            low, high = axis.travel
            value = cmd.values[index]
            if value < low or value > high:
                _say(f"{axis.name} commanded to {value:.4f} outside its travel ({low:.4f} to {high:.4f}). Refused rather than clamped: a clamp would move the joint "
                     "somewhere nobody asked for.")
                return rhal.SKR_EINVAL

        # §24's hold, resolved on copies and committed once the frames are on the bus.
        positions = [self._rad(axis) for axis in self.axes]
        frames: list[tuple[Axis, bytes]] = []
        updates: list[tuple[float, bool, float, bool]] = []
        for index, axis in enumerate(self.axes):
            setpoint, have, hold, holding = axis.setpoint, axis.have_setpoint, axis.hold, axis.holding
            if applied[index]:
                holding = False
                value = cmd.values[index]
                if modes[index] == rhal.MODE_POSITION:
                    setpoint, have = value, True
                    data = p.position_command(value / _TWO_PI, 0.0, 0.0, 1.0, 1.0, self._max_torque, self._watchdog_s)
                elif modes[index] == rhal.MODE_VELOCITY:
                    have = False  # a velocity is not a position to return to
                    data = p.position_command(math.nan, value / _TWO_PI, 0.0, 1.0, 1.0, self._max_torque, self._watchdog_s)
                else:
                    have = False
                    data = p.position_command(math.nan, 0.0, value, 0.0, 0.0, self._max_torque, self._watchdog_s)
            else:
                if not holding:
                    hold = setpoint if have else positions[index]
                    holding = True
                data = p.position_command(hold / _TWO_PI, 0.0, 0.0, 1.0, 1.0, self._max_torque, self._watchdog_s)
            frames.append((axis, data))
            updates.append((setpoint, have, hold, holding))
        try:
            for axis, data in frames:
                self._send(axis, data, reply=False)
        except ValueError:
            return rhal.SKR_EINVAL
        for axis, (setpoint, have, hold, holding) in zip(self.axes, updates):
            axis.setpoint, axis.have_setpoint, axis.hold, axis.holding = setpoint, have, hold, holding
        self._awaiting_resume = False  # the next read must find the axes in position, or say they are not
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        self._estop = True
        if self._bus is not None:
            for axis in self.axes:
                self._send(axis, p.stop_command(self._stop_mode), reply=False)
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Release a stop that left the motor driver on; ENOSYS for one that did not.

        From *zero velocity* and *brake* the firmware switches straight to another mode (`ISR_MaybeChangeMode`: the driver is never off in between), so the
        next command takes the axis back with no interval in which it is limp, and the latch can honestly be cleared. From *stopped* the controller re-enters through
        its calibrating phase with the driver off; a release that did that would drop a loaded joint for it and report a clean return, so it is ENOSYS and the session is
        renewed over an axis a person has looked at (the Piper and ODrive precedent)."""
        if self._stop_mode == p.MODE_STOPPED:
            return rhal.SKR_ENOSYS
        self._estop = False
        # The axes are in the stop mode until a command takes them back, and until one has, that is expected and not a fault. The hold is re-latched from where they are.
        self._awaiting_resume = True
        for axis in self.axes:
            axis.holding = False
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── what is judged ───────────────────────────────────────────────────────────────────────────────────────────────────

    def _judge(self, axis: Axis) -> str:
        """``fault``, ``degraded`` or ``ok`` for one axis, from what it last said. Only *position* is healthy in a session (and the stop this driver wrote, after
        one)."""
        status = axis.status
        assert status is not None
        mode = status.mode
        if mode == p.MODE_POSITION:
            if self._estop:
                # Still in position after a stop was asked for: the frame is a cycle behind this read, or it did not take. ESTOP is already set and commands are refused; DEGRADED says the
                # axis has not confirmed it, and stays set for as long as it has not.
                return "degraded"
            if status.fault:
                return "degraded"  # 96 to 105 are limits, listed or not: an unlisted non-zero code is never read as healthy
            return "ok"
        if self._estop and mode == self._stop_mode:
            return "ok"
        if self._awaiting_resume and mode == self._stop_mode:
            return "degraded"  # released from a stop that left the driver on: the next command takes the axis back, and until it has the axis is not in position
        return "fault"

    def _problems(self) -> list[str]:
        out: list[str] = []
        for axis in self.axes:
            if axis.status is None:
                out.append(f"{axis.name} has not reported")
            elif self._judge(axis) == "fault":
                if axis.status.mode == p.MODE_FAULT:
                    out.append(f"{axis.name} (controller {axis.ident}) is in fault: {p.fault_name(axis.status.fault)} ({axis.status.fault})")
                else:
                    out.append(f"{axis.name} (controller {axis.ident}) is in {axis.mode_name()} ({axis.status.mode}), not position")
        return out

    # ── the bring-up ─────────────────────────────────────────────────────────────────────────────────────────────────────

    def _refuse_if_absent(self, axis: Axis) -> None:
        """Ask the controller for its status, and refuse one that never answers."""
        assert self._bus is not None
        self._send(axis, p.query(), reply=True)
        deadline = self._clock() + _REQUEST_TIMEOUT_S
        for _ in range(_WAIT_FRAME_CAP):
            frame = self._bus.recv(max(deadline - self._clock(), 0.0))
            if frame is None:
                if self._bus.synchronous or self._clock() >= deadline:
                    break
                continue
            self._apply(frame)
            if axis.status is not None:
                return
        raise BridgeRefusal(
            f"controller {axis.ident} did not answer a query within {_REQUEST_TIMEOUT_S:.1f}s. A controller that is not on the bus, one that is powered off and one at "
            "another id look the same from here; `candump -f` on the interface (CAN-FD, 1 Mbit with a 5 Mbit data phase) is what tells them apart."
        )

    def _refuse_if_not_ours(self, axis: Axis) -> None:
        status = axis.status
        assert status is not None
        if status.mode == p.MODE_FAULT:
            raise BridgeRefusal(
                f"controller {axis.ident} ({axis.name}) is in fault: {p.fault_name(status.fault)} ({status.fault}). This body does not come up over a latched fault. Look the "
                "reason up (`moteus_tool --target %d --info`, the maker's table is docs/protocol/registers.md, 0x00f), and clear it at the controller deliberately: a driver "
                "that cleared it here would restart a machine whose reason for stopping nobody had looked at." % axis.ident
            )
        if status.mode == p.MODE_TIMEOUT:
            raise BridgeRefusal(
                f"controller {axis.ident} ({axis.name}) is in timeout: its watchdog ran out, from this driver's last session or someone else's, and it is applying its "
                "configured fallback. Only a stop leaves that mode, and a stop is the person's act here: stop it deliberately (`moteus_tool --target %d --stop`) with its load "
                "supported, then open again." % axis.ident
            )
        if status.mode not in _OPENABLE:
            raise BridgeRefusal(
                f"controller {axis.ident} ({axis.name}) is in {axis.mode_name()} ({status.mode}), which is another master's mode: this driver puts an axis in position and finds "
                "it stopped, in position, or in a stop it wrote. Whatever is driving it is still driving it; stop that first."
            )

    def _conf_get(self, axis: Axis, name: str) -> float | None:
        """One configuration value over the diagnostic channel, or None when the controller does not answer it (or answers `ERR`)."""
        assert self._bus is not None
        self._send(axis, p.diagnostic_write(1, f"conf get {name}\n".encode("latin1")), reply=False)
        deadline = self._clock() + _CONF_TIMEOUT_S
        text = b""
        for _ in range(_WAIT_FRAME_CAP):
            self._send(axis, p.diagnostic_poll(1), reply=True)
            progressed = False
            while True:
                frame = self._bus.recv(0.0 if self._bus.synchronous else 0.01)
                if frame is None:
                    break
                progressed = True
                try:
                    source, _, _ = p.split_id(frame.arbitration_id) if frame.arbitration_id <= 0xFFFF else (None, None, None)
                    if source == axis.ident:
                        text += p.parse_diagnostic_reply(bytes(frame.data), 1)
                    else:
                        self._apply(frame)
                except ValueError:
                    self.unexpected_frames += 1
            if b"\n" in text:
                break
            if self._clock() >= deadline or (self._bus.synchronous and not progressed):
                break
        line = text.split(b"\n", 1)[0].strip().decode("latin1")
        if not line or line.startswith("ERR"):
            return None
        try:
            return float(line)
        except ValueError:
            return None

    def _read_travel(self, axis: Axis) -> None:
        low = self._conf_get(axis, "servopos.position_min")
        high = self._conf_get(axis, "servopos.position_max")
        answered = low is not None and high is not None
        if not answered and axis.operator_limits is None:
            raise BridgeRefusal(
                f"controller {axis.ident} ({axis.name}) did not answer `conf get servopos.position_min` / `position_max` over the diagnostic channel, and no `limits` were given. "
                "This driver declares the controller's own travel, and a controller that will not say what it holds has not said whether the joint is bounded: give `limits` "
                "in radians for it, or fix the channel (diagnostic streaming needs a firmware that has it)."
            )
        # NaN is the controller's "no bound on this side": an open side is minus or plus infinity until the operator closes it
        lo_c, hi_c = -math.inf, math.inf
        if answered:
            assert low is not None and high is not None
            if not math.isnan(low) and not math.isnan(high) and not low < high:
                raise BridgeRefusal(
                    f"controller {axis.ident} ({axis.name}) reports a position range of {low} to {high} revolutions, which is empty or reversed. Refused rather than guessed at; "
                    "the controller's limits are its own to set."
                )
            lo_c = -math.inf if math.isnan(low) else low * _TWO_PI
            hi_c = math.inf if math.isnan(high) else high * _TWO_PI
        lo_o, hi_o = axis.operator_limits if axis.operator_limits is not None else (-math.inf, math.inf)
        lo, hi = max(lo_c, lo_o), min(hi_c, hi_o)  # the narrower of the two on each side, never the wider (F181)
        if math.isinf(lo) and math.isinf(hi):
            axis.travel = None  # nothing bounds this joint: it is continuous, and nothing declares a travel that is not there
            return
        if math.isinf(lo) or math.isinf(hi):
            raise BridgeRefusal(
                f"{axis.name} (controller {axis.ident}) is bounded on one side only (from the controller's position_min {low}, position_max {high} and the `limits` given). "
                "A joint with one open side has no travel this driver can declare: give `limits` for the other side or set both on the controller."
            )
        if not lo < hi:
            raise BridgeRefusal(
                f"`limits` on {axis.name} ({lo_o:.4f} to {hi_o:.4f} rad) do not overlap the controller's own travel ({lo_c:.4f} to {hi_c:.4f} rad)."
            )
        axis.travel = (lo, hi)

    def _refuse_if_outside_travel(self, axis: Axis) -> None:
        if axis.travel is None:
            return
        position = self._rad(axis)
        low, high = axis.travel
        slack = _TRAVEL_SLACK_REV * _TWO_PI
        if not low - slack <= position <= high + slack:
            raise BridgeRefusal(
                f"{axis.name} reads {position:.4f} rad, outside the travel this driver declares ({low:.4f} to {high:.4f}). A moteus refuses to start position control "
                "outside its bounds (fault 39, outside limit), so it is refused here instead: bring it inside by hand."
            )

    def _enable(self) -> None:
        """Tell every axis to hold where it is, and wait until each reports position mode: read back, never assumed. The controller's own preparing phase (modes 2 to 4,
        a few cycles of ADC calibration) is what is waited out; a position command with no position written means *use the current position*."""
        assert self._bus is not None
        deadline = self._clock() + _ENABLE_TIMEOUT_S
        for _ in range(_WAIT_FRAME_CAP):
            for axis in self.axes:
                self._send(axis, p.position_command(math.nan, 0.0, 0.0, 1.0, 1.0, self._max_torque, self._watchdog_s) + p.query(), reply=True)
            self._collect()
            self._drain()
            if all(axis.status is not None and axis.status.mode == p.MODE_POSITION for axis in self.axes):
                return
            if any(axis.status is not None and axis.status.mode in (p.MODE_FAULT, p.MODE_TIMEOUT) for axis in self.axes):
                break
            if self._clock() >= deadline:
                break
        for axis in self.axes:
            status = axis.status
            if status is not None and status.mode == p.MODE_POSITION:
                continue
            if status is not None and status.mode == p.MODE_FAULT:
                advice = f"It faulted on the way up: {p.fault_name(status.fault)} ({status.fault}). Look it up and clear it at the controller."
            else:
                advice = ("It is still preparing, or another master holds it, or it never heard the command. The mode is read back rather than assumed, because a master that "
                          "trusted its own request would write setpoints into an axis that is not listening.")
            raise BridgeRefusal(f"controller {axis.ident} ({axis.name}) was told to hold and reports `{axis.mode_name()}`. {advice}")

    # ── the bus ──────────────────────────────────────────────────────────────────────────────────────────────────────────

    def _rad(self, axis: Axis) -> float:
        return 0.0 if axis.status is None else axis.status.position * _TWO_PI

    def _send(self, axis: Axis, data: bytes, reply: bool) -> None:
        assert self._bus is not None
        ident = p.can_id(self._source, axis.ident, reply)
        self._bus.send(FdFrame(ident, p.pad(data), extended=ident > 0x7FF))

    def _drain(self) -> None:
        """Apply everything already waiting, without waiting for more."""
        assert self._bus is not None
        waiting = self._bus.backlog()
        for _ in range(_WAIT_FRAME_CAP if waiting is None else waiting):
            frame = self._bus.recv()
            if frame is None:
                return
            self._apply(frame)

    def _collect(self) -> int:
        """How many axes answered this cycle's query, waiting up to the window for the rest."""
        assert self._bus is not None
        waiting = {axis.ident for axis in self.axes}
        deadline = self._clock() + self._window_s
        for _ in range(_WAIT_FRAME_CAP):
            if not waiting:
                break
            remaining = deadline - self._clock()
            frame = self._bus.recv(max(remaining, 0.0))
            if frame is None:
                if self._bus.synchronous or remaining <= 0.0:
                    break
                continue
            # Whatever was waiting was taken before the queries went out, so a reply that parses here is the answer to one of them.
            ident = self._apply(frame)
            if ident is not None:
                waiting.discard(ident)
        return len(self.axes) - len(waiting)

    def _apply(self, frame: FdFrame) -> int | None:
        """Fold one frame into the axis it speaks for. Returns the axis id that spoke, or None."""
        if frame.arbitration_id > 0xFFFF:
            self.unexpected_frames += 1
            return None
        source, destination, _ = p.split_id(frame.arbitration_id)
        axis = next((a for a in self.axes if a.ident == source), None)
        if axis is None or destination != self._source:
            self.unexpected_frames += 1
            return None
        try:
            axis.status = p.parse_status(bytes(frame.data))
        except ValueError:
            self.unexpected_frames += 1
            return None
        axis.status_at = self._clock()
        return axis.ident

    # ── the descriptor ───────────────────────────────────────────────────────────────────────────────────────────────────

    def _build_descriptor(self) -> str:
        joints = []
        for axis in self.axes:
            if axis.travel is None:
                joint_type, limits = "continuous", {}
            else:
                joint_type, limits = "revolute", {"position_min_rad": axis.travel[0], "position_max_rad": axis.travel[1]}
            joints.append({"name": axis.name, "role": axis.role, "type": joint_type, "limits": limits, "control_modes": ["position", "velocity", "torque"]})
        descriptor = {
            "schema_version": 1,
            "robot_slug": "mjbots-moteus-axes",
            "digest": None,
            "verified": False,
            "simulator": self._simulated,
            "base": "fixed",
            "dof": len(self.axes),
            "joints": joints,
            "end_effectors": [],
            "sensors": [],
            "control": {"modes": ["position", "velocity", "torque"], "per_joint_modes": True, "rate_hz_max": 1000, "rate_hz_min": 1},
            # Null: no rated envelope is published for what is attached to a controller, and the controller's own limits are configuration, not a risk assessment (§63.4).
            "envelope": None,
            "capabilities": ["safety.estop"],
            "extensions": [],
            "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))


def _say(message: str) -> None:
    import sys

    print(f"skynet-moteus: {message}", file=sys.stderr, flush=True)

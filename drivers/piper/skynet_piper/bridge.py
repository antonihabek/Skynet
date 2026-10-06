"""An AgileX Piper arm over CAN: six revolute joints and, optionally, the gripper, as one Skynet body.

The wire is the maker's own (``skynet_piper.protocol``): big-endian eight-byte frames the arm *broadcasts* — its
status, its joint angles, each driver's report — and a master answers with a mode and joint targets. What this
driver does with it:

| Read, every cycle | status (0x2A1), the joint pairs (0x2A5–0x2A7), the gripper (0x2A8), each driver's report (0x261–0x266) |
| Asked at open | each joint's angle limits (0x472, answered by 0x473) |
| Commanded | ``0x151`` (CAN control, MOVE J, the operator's speed) and the six targets (``0x155``–``0x157``), the gripper's (``0x159``) |
| Stopped by | ``0x150`` ``emergency_stop`` 1 — the arm's fast stop |

# What it reads, and what it will not invent

**Health is the status frame and the drivers' reports together.** The arm status (sixteen values), the error word
(a joint's angle-limit and communication bits) and every driver's status byte (supply voltage, motor and driver
temperature, over-current, collision protection, driver error, stall protection and *driver enabled*) are all on
the wire. A body that does not send them does not come up: without them a fault would sit unread while the driver
reported angles. The fault word is the maker's own, composed and not translated: the error word in the low sixteen
bits, the arm status above it, and the union of every driver's fault bits in the top byte, with ``0x40`` there
meaning *a driver is disabled* (a disabled Piper joint is a limp one). Two bits are this driver's, both
in the byte the maker lists as reserved: bit 15, set while the arm is in a control mode other than CAN control and is ignoring
this master, and bit 14, set while the gripper's status byte holds a fault bit (a disabled gripper sets the top byte's 0x40, as
a disabled joint does).

**Faults are refused, never cleared.** While any of that stands every command is refused, and the driver sends no
frame that clears one. The arm's clear is the person's act at the arm.

**The stop is a stop, and its release is not the SDK's.** ``emergency_stop`` sends the arm's fast stop and latches.
The maker's *resume* (``0x150`` ``emergency_stop`` 2) is, in the same SDK, ``ResetPiper``: *the arm loses power and
falls*. A software release that dropped a loaded arm is not a release, so ``release_stop`` answers ENOSYS, nothing
in this driver encodes that byte, and ``open`` refuses while the arm reports the stop. The session is renewed over
an arm a person reset deliberately, supporting the weight (§4, the ODrive and Spot precedent).

**Travel is the narrower of two tables.** The maker publishes each joint's range (``C_PiperParamManager``) and the
arm reports the limits it holds (0x473). The descriptor declares the intersection, never the wider (F181), and a
setpoint outside it is refused whole, never clamped.

**Velocity and effort are NaN.** The drivers report *motor* speed and current; the gear ratio and torque constant
that turn them into a joint's velocity and torque are not on the wire (§24).

**The gripper is the operator's to declare.** ``gripper`` and ``gripper_torque_nm`` have no default: an arm sold
without a hand has no seventh joint, and a hand's grip force is the integrator's risk assessment (§63.4).

# Modes

Position only. The wire's joint-level verb is *move to this angle at this percentage of the arm's own speed*; there
is no velocity or torque command in the CAN control mode this driver speaks (MIT mode would carry them, with gains
the operator chooses, and is not built). A command in another mode is refused.
"""

from __future__ import annotations

import json
import math
import time

import skynet_rhal as rhal
from skynet_canopen.bus import Bus, SocketCanBus
from skynet_canopen.frames import Frame

from . import protocol as p

__all__ = ["PiperBridge", "BridgeRefusal"]

#: How long `open()` waits for the arm's first status frame. The arm broadcasts without being asked, at a rate in the
#: hundreds of hertz; half a second of silence is an arm that is off, in another mode of broadcast, or not on this bus.
_STATUS_TIMEOUT_S = 0.5
#: How long `open()` waits for every report it needs to see before it commands anything.
_TELEMETRY_TIMEOUT_S = 1.0
#: How long `open()` waits for one joint's limit report.
_LIMIT_TIMEOUT_S = 0.5
#: How long `open()` waits for the drivers to report themselves enabled, and the arm to report CAN control.
_ENABLE_TIMEOUT_S = 1.0
#: The most frames a drain applies on a bus that cannot say how many are waiting: a few cycles of the arm's broadcast (about
#: twenty frames each), which is more than a control period holds and less than a flood that would never let the read begin.
_DRAIN_LIMIT = 512
#: The most frames one wait applies before it gives up whatever the clock says (see `_frames`): about a second of a real arm's broadcast
#: at its full rate, and a few hundred cycles of a simulated one.
_WAIT_FRAME_CAP = 20_000
#: How often `open()` repeats a request the arm has not yet shown it acted on.
_REASK_S = 0.1
#: How long `read_state` waits for a fresh cycle's reports — the default, which `window_ms` replaces.
_WINDOW_S = 0.050
#: How old a report may be before what it says is not health any more but the last thing it said.
_STALE_S = 0.5

_CHAINS = frozenset({"left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand",
                     "torso", "neck", "head", "base", "tail", "aux"})
_AXES = frozenset({"pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip"})


def _normalise_role(role: object) -> str | None:
    """The canonical form of a §5.1 role, or None when it does not parse (the ODrive driver's validator, with its pointer)."""
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


class PiperBridge:
    """An AgileX Piper arm."""

    def __init__(self, bus: Bus | None = None, clock=time.monotonic) -> None:
        self._bus = bus
        self._owns_bus = bus is None
        self._clock = clock
        self._simulated = True
        self._opened = False
        self._estop = False
        self._seq = 0
        self._descriptor: str | None = None
        self._window_s = _WINDOW_S
        self._speed_percent = 0
        self._has_gripper = False
        self._gripper_torque = 0
        self._names: list[str] = []
        self._roles: dict[str, str] = {}
        #: The travel declared, per joint, radians (and the gripper's, metres): the narrower of the maker's table and the arm's report.
        self._travel: list[tuple[float, float]] = []
        #: Latest of everything the arm said, and when it arrived on `self._clock`.
        self._status: p.Status | None = None
        self._status_at: float | None = None
        self._pairs: dict[int, tuple[int, int]] = {}
        self._pairs_at: dict[int, float] = {}
        self._gripper: tuple[int, int, int] | None = None
        self._gripper_at: float | None = None
        self._low: dict[int, p.LowSpeed] = {}
        self._low_at: dict[int, float] = {}
        self._reports: dict[int, p.LimitReport] = {}
        #: §24's hold, per joint: the last position setpoint and whether there is one, and the latched hold target.
        self._setpoint: list[float] = []
        self._have_setpoint: list[bool] = []
        self._hold: list[float] = []
        self._holding: list[bool] = []
        #: Frames on an identifier this driver does not speak, or of the wrong length. Counted, never dropped in silence.
        self.unexpected_frames = 0

    # ── §4's entry points ────────────────────────────────────────────────────

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
        self._reset_session()
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

    def _reset_session(self) -> None:
        self._estop = False
        self._descriptor = None
        self._status = None
        self._status_at = None
        self._pairs = {}
        self._pairs_at = {}
        self._gripper = None
        self._gripper_at = None
        self._low = {}
        self._low_at = {}
        self._reports = {}
        self._travel = []
        self._names = []
        self._roles = {}
        self._setpoint = []
        self._have_setpoint = []
        self._hold = []
        self._holding = []

    def _open(self, settings: dict) -> None:
        speed = settings.get("speed_percent")
        if isinstance(speed, bool) or not isinstance(speed, int) or not 1 <= speed <= 100:
            raise BridgeRefusal(
                f"`speed_percent` is {speed!r} and has no default. A joint move is 'to this angle at this percentage of the "
                "arm's own maximum speed', and how fast an arm may move among people is the integrator's risk assessment, not "
                "this driver's: give a whole number of percent from 1 to 100."
            )
        has_gripper = settings.get("gripper")
        if not isinstance(has_gripper, bool):
            raise BridgeRefusal(
                "`gripper` is not declared. An arm sold without a hand has six joints and one with it has seven, and nothing on "
                'the wire says which this is: give `"gripper": true` or `"gripper": false`.'
            )
        torque = settings.get("gripper_torque_nm")
        if has_gripper:
            if isinstance(torque, bool) or not isinstance(torque, (int, float)) or not 0.0 < torque <= 5.0:
                raise BridgeRefusal(
                    f"`gripper_torque_nm` is {torque!r} and has no default. How hard a hand closes is the integrator's risk "
                    "assessment (§63.4); the wire takes 0 to 5 N·m, so give a number above 0 and at most 5."
                )
        window_ms = settings.get("window_ms", _WINDOW_S * 1000.0)
        if isinstance(window_ms, bool) or not isinstance(window_ms, (int, float)) or not 0.0 < window_ms <= 1000.0:
            raise BridgeRefusal(
                f"`window_ms` of {window_ms!r} is not a window. It bounds how long a read waits for a fresh cycle of the arm's "
                "reports, in milliseconds, above zero and at most a second — a longer one would be a control loop waiting on a bus "
                "that has gone."
            )
        names = list(p.JOINT_NAMES) + ([p.GRIPPER_NAME] if has_gripper else [])
        roles: dict[str, str] = {}
        given = settings.get("roles") or {}
        if not isinstance(given, dict):
            raise BridgeRefusal("`roles` maps a joint name to a §5.1 role, as an object.")
        for name, role in given.items():
            if name not in names:
                raise BridgeRefusal(f"`roles` names `{name}`, which is not one of this arm's joints ({', '.join(names)}).")
            canonical = _normalise_role(role)
            if canonical is None:
                raise BridgeRefusal(
                    f"`{role}` is not a §5.1 role (chain.link[.axis][#index] over the canonical chains and axes). Roles are "
                    "the operator's and are never inferred."
                )
            roles[name] = canonical

        if self._bus is None:
            self._bus = SocketCanBus(settings.get("interface", "vcan0"))
        # Whatever is already waiting answers an earlier session: discarded, not applied.
        waiting = self._bus.backlog()
        for _ in range(_DRAIN_LIMIT if waiting is None else waiting):
            if self._bus.recv() is None:
                break
        self._simulated = bool(settings.get("simulator", True)) or self._bus.synchronous
        self._window_s = float(window_ms) / 1000.0
        self._speed_percent = speed
        self._has_gripper = has_gripper
        self._gripper_torque = round(float(torque) * 1000.0) if has_gripper else 0
        self._names = names
        self._roles = roles

        self._await_status()
        self._refuse_if_faulted()
        self._await_telemetry()
        self._refuse_if_faulted()
        self._read_limits()
        self._refuse_if_outside_travel()
        self._enable_and_take_control()
        positions = self._positions()
        self._setpoint = list(positions)
        self._hold = list(positions)
        self._have_setpoint = [False] * len(names)
        self._holding = [False] * len(names)
        self._descriptor = self._build_descriptor()

    def close(self) -> int:
        # Nothing is sent to the arm. A Piper that is disabled loses power and falls, one that is put in standby is no longer
        # held by the control loop that was holding it, and one that is simply no longer spoken to holds the last target it
        # was given: the one hand-back that cannot drop a load is silence.
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
        # What is waiting answers an earlier cycle; it is applied, because a late reading is newer than none, and the cycle
        # this read waits for is the one that arrives after it.
        self._drain()
        # §24: stamped at acquisition. The arm samples when it broadcasts, so a stamp taken here — before the fresh cycle is
        # collected — can only be older than the sample, never newer.
        acquired = rhal.acquisition_ns()
        seen = self._collect_cycle()
        self._seq += 1

        out.stamp_ns = acquired
        out.seq = self._seq
        out.flags = rhal.HOMING_UNREPORTED
        if self._estop or (self._status is not None and self._status.arm_status == p.ARM_STATUS_EMERGENCY_STOP):
            out.flags |= rhal.FLAG_ESTOP
        positions = self._positions()
        out.position_rad = list(positions)
        # NaN, not zero: see the module docstring.
        out.velocity_rad_s = [math.nan] * len(self._names)
        out.effort_nm = [math.nan] * len(self._names)
        word = self._fault_word()
        out.fault_code = word
        faulted = bool(self._problems())
        unwell = self._unwell()
        if faulted:
            out.flags |= rhal.FLAG_DEGRADED | rhal.FLAG_FAULT
        elif unwell:
            out.flags |= rhal.FLAG_DEGRADED
        if seen == 0:
            # Nothing arrived for a whole window: the arm is silent. The sequence still advances — the watchdog is watching
            # the stamp and this read happened — but the caller is told rather than handed the last angles as fresh.
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        count = len(self._names)
        if cmd.joint_count != count:
            return rhal.SKR_EINVAL
        try:
            modes = [cmd.mode_of(index) for index in range(count)]
        except ValueError:
            return rhal.SKR_EINVAL
        # A mode the ABI does not define is malformed whatever the mask says: the mask decides which joints move, not which entries are
        # well-formed, and an unrecognised mode quietly treated as position is a torque command that became a position command.
        known = (rhal.MODE_POSITION, rhal.MODE_VELOCITY, rhal.MODE_TORQUE)
        if cmd.mode not in known or any(mode not in known for mode in modes):
            return rhal.SKR_EINVAL
        applied = [bool(cmd.mask[index]) if index < len(cmd.mask) else True for index in range(count)]
        # A setpoint that is not a number is refused with the other malformed commands, before anything is built or stored.
        for index in range(count):
            if applied[index] and not math.isfinite(cmd.values[index]):
                _say(f"the setpoint for {self._names[index]} is not a finite number. The whole command is refused; send a number (§24).")
                return rhal.SKR_EINVAL
        if self._estop:
            return rhal.SKR_EPERM
        for index in range(count):
            if applied[index] and modes[index] != rhal.MODE_POSITION:
                _say(
                    f"{self._names[index]} is commanded in a mode this wire does not carry. The Piper's joint-level verb in CAN "
                    "control mode is 'move to this angle'; there is no velocity or torque command in it (MIT mode would carry "
                    "them, with gains the operator chooses). Refused rather than converted."
                )
                return rhal.SKR_EPERM
        problems = self._problems()
        if problems:
            _say("refused — " + "; ".join(problems) + ". Nothing this driver sends will move the arm until that is cleared at the arm.")
            return rhal.SKR_EPERM
        for index in range(count):
            if not applied[index]:
                continue
            low, high = self._travel[index]
            value = cmd.values[index]
            if value < low or value > high:
                _say(
                    f"{self._names[index]} commanded to {value:.4f} outside its travel ({low:.4f} to {high:.4f}), the narrower of "
                    "the maker's published range and the limit the arm reports. Refused rather than clamped: a clamp would move "
                    "the joint somewhere nobody asked for."
                )
                return rhal.SKR_EINVAL

        # §24's hold, resolved per joint on copies, committed only once the frames are on the bus.
        setpoint = list(self._setpoint)
        have_setpoint = list(self._have_setpoint)
        hold = list(self._hold)
        holding = list(self._holding)
        positions = self._positions()
        targets: list[float] = []
        for index in range(count):
            if applied[index]:
                holding[index] = False
                have_setpoint[index] = True
                setpoint[index] = cmd.values[index]
                targets.append(cmd.values[index])
                continue
            if not holding[index]:
                hold[index] = setpoint[index] if have_setpoint[index] else positions[index]
                holding[index] = True
            targets.append(hold[index])
        try:
            frames = [Frame(p.ID_MOTION_2, p.motion_ctrl_2(p.CTRL_CAN, p.MOVE_J, self._speed_percent))]
            counts = [p.rad_to_mdeg(value) for value in targets[:6]]
            for pair, ident in enumerate(p.ID_JOINT_CTRL):
                frames.append(Frame(ident, p.joint_ctrl(counts[2 * pair], counts[2 * pair + 1])))
            if self._has_gripper:
                frames.append(Frame(p.ID_GRIPPER_CTRL, p.gripper_ctrl(p.metres_to_counts(targets[6]), self._gripper_torque, 0x01)))
        except ValueError:
            return rhal.SKR_EINVAL
        assert self._bus is not None
        for frame in frames:
            self._bus.send(frame)
        self._setpoint, self._have_setpoint, self._hold, self._holding = setpoint, have_setpoint, hold, holding
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        self._estop = True
        if self._bus is not None:
            self._bus.send(Frame(p.ID_MOTION_1, p.motion_ctrl_1(emergency_stop=p.EMERGENCY_STOP)))
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """No software release: ENOSYS, honestly (the ODrive and Spot precedent). The maker's resume is a reset that drops the arm."""
        return rhal.SKR_ENOSYS

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── the bring-up ─────────────────────────────────────────────────────────

    def _await_status(self) -> None:
        deadline = self._clock() + _STATUS_TIMEOUT_S
        if self._status is None:
            for _ in self._frames(deadline):
                if self._status is not None:
                    break
        if self._status is None:
            raise BridgeRefusal(
                f"the arm sent no status frame within {_STATUS_TIMEOUT_S:.1f}s. A Piper broadcasts its status (0x2A1) as soon as it is "
                "powered; silence is an arm that is off, a bus at the wrong bit rate (it speaks 1 Mbit/s), or an interface that is "
                "not the arm's. `candump` is what tells them apart."
            )

    def _await_telemetry(self) -> None:
        wanted = {p.ID_JOINTS_12, p.ID_JOINTS_34, p.ID_JOINTS_56}
        deadline = self._clock() + _TELEMETRY_TIMEOUT_S

        def complete() -> bool:
            return len(self._low) == 6 and wanted <= set(self._pairs) and ((not self._has_gripper) or self._gripper is not None)

        if complete():
            return
        for _ in self._frames(deadline):
            if complete():
                return
        missing = []
        if not wanted <= set(self._pairs):
            missing.append("the joint angles (0x2A5–0x2A7)")
        if len(self._low) < 6:
            missing.append(f"the drivers' reports (0x261–0x266: {len(self._low)} of 6)")
        if self._has_gripper and self._gripper is None:
            missing.append("the gripper (0x2A8) — the configuration says there is one")
        raise BridgeRefusal(
            "the arm answers with a status but not with " + " or ".join(missing) + ". This driver reads health from the drivers' "
            "reports and positions from the angle frames, so an arm that does not broadcast them is one whose faults would sit "
            "unread while the driver reported angles."
        )

    def _refuse_if_faulted(self) -> None:
        problems = self._problems(opening=True)
        if problems:
            raise BridgeRefusal(
                "the arm reports " + "; ".join(problems) + ". This body does not come up over a standing fault or a stop. Look the "
                "reason up, and clear it at the arm deliberately — with the arm's weight supported where the maker's reset would "
                "drop it. A driver that cleared it here would restart a machine whose reason for stopping nobody had looked at."
            )

    def _read_limits(self) -> None:
        assert self._bus is not None
        for joint in range(1, 7):
            self._bus.send(Frame(p.ID_LIMIT_QUERY, p.limit_query(joint)))
            deadline = self._clock() + _LIMIT_TIMEOUT_S
            if joint not in self._reports:
                for _ in self._frames(deadline):
                    if joint in self._reports:
                        break
            if joint not in self._reports:
                raise BridgeRefusal(
                    f"joint {joint} did not answer a limit query (0x472) within {_LIMIT_TIMEOUT_S:.1f}s. This driver declares the "
                    "narrower of the maker's published travel and the limit the arm holds, and an arm that will not say what it "
                    "holds has not told it which of the two is narrower."
                )
        travel: list[tuple[float, float]] = []
        for joint, (published_low, published_high) in enumerate(p.JOINT_LIMITS_RAD):
            report = self._reports[joint + 1]
            low, high = report.min_rad(), report.max_rad()
            if not low < high:
                raise BridgeRefusal(
                    f"joint {joint + 1} reports an angle range of {report.min_angle / 10:.1f}° to {report.max_angle / 10:.1f}°, which "
                    "is empty or reversed. Refused rather than guessed at; the arm's limits are its own to set."
                )
            travel.append((max(published_low, low), min(published_high, high)))
        if self._has_gripper:
            travel.append(p.GRIPPER_RANGE_M)
        self._travel = travel

    def _refuse_if_outside_travel(self) -> None:
        positions = self._positions()
        for index, (low, high) in enumerate(self._travel):
            # The tolerance is the wire's own resolution: a joint at its limit reads a count either side of it.
            slack = 2.0 / p.MDEG_PER_RAD if index < 6 else 2.0 / p.COUNTS_PER_METRE
            if not low - slack <= positions[index] <= high + slack:
                raise BridgeRefusal(
                    f"{self._names[index]} reads {positions[index]:.4f}, outside the travel this driver declares ({low:.4f} to "
                    f"{high:.4f}). An arm found outside its range would refuse every command and say nothing, so it is refused here "
                    "instead (the F246 shape): bring it inside by hand."
                )

    def _ask_until(self, frames: list[Frame], done) -> None:
        """Send ``frames``, then apply what the arm broadcasts until ``done()``, asking again every `_REASK_S` and giving up after
        `_ENABLE_TIMEOUT_S`. The arm broadcasts hundreds of frames a second, so asking per frame received would be a flood."""
        assert self._bus is not None
        deadline = self._clock() + _ENABLE_TIMEOUT_S
        for frame in frames:
            self._bus.send(frame)
        asked_at = self._clock()
        if done():
            return
        for _ in self._frames(deadline):
            if done():
                return
            if self._clock() - asked_at >= _REASK_S:
                for frame in frames:
                    self._bus.send(frame)
                asked_at = self._clock()

    def _enable_and_take_control(self) -> None:
        assert self._bus is not None
        # One enable per motor, never the 0xFF the message class documents or the 7 the interface documents as "all": the maker's own
        # sources disagree about both (F284), `EnablePiper` is `EnableArm(7)`, and enabling joints 1 to 6 by name and the gripper by its
        # own number means the same on either reading. 7 is sent only when there is a hand: on the reading where it is "all" it would
        # enable a motor nobody declared.
        enables = [Frame(p.ID_ENABLE, p.motor_enable(joint, 0x02)) for joint in range(1, 7)]
        if self._has_gripper:
            enables.append(Frame(p.ID_ENABLE, p.motor_enable(7, 0x02)))
        def gripper_up() -> bool:
            return (not self._has_gripper) or (self._gripper is not None and bool(self._gripper[2] & p.GRIPPER_ENABLED))

        self._ask_until(
            enables,
            lambda: len(self._low) == 6 and all(low.enabled() for low in self._low.values()) and gripper_up(),
        )
        low_now = self._low_by_joint()
        disabled = [str(joint + 1) for joint in range(6) if joint not in low_now or not low_now[joint].enabled()]
        if not gripper_up():
            disabled.append("the gripper")
        if disabled:
            raise BridgeRefusal(
                f"the driver(s) of {', '.join(('joint ' + d) if d.isdigit() else d for d in disabled)} did not report themselves enabled within {_ENABLE_TIMEOUT_S:.1f}s of "
                "an enable. The arm is left as found — a disable here would let it fall. A joint that will not enable has a fault "
                "the status byte names, a brake that is not released, or a supply that is too low."
            )
        self._ask_until(
            [Frame(p.ID_MOTION_2, p.motion_ctrl_2(p.CTRL_CAN, p.MOVE_J, self._speed_percent))],
            lambda: self._status is not None and self._status.ctrl_mode == p.CTRL_CAN,
        )
        if self._status is None or self._status.ctrl_mode != p.CTRL_CAN:
            mode = p.CTRL_MODES.get(self._status.ctrl_mode, f"mode {self._status.ctrl_mode}") if self._status else "no status"
            raise BridgeRefusal(
                f"the arm was asked for CAN control and reports `{mode}`. It is read back rather than assumed: a master that "
                "trusted its own request would write targets into an arm that is listening to something else — a teach pendant, "
                "a remote controller, another host."
            )

    # ── what the arm says ────────────────────────────────────────────────────

    def _low_by_joint(self) -> dict[int, p.LowSpeed]:
        return {ident - p.ID_LOW_SPEED: report for ident, report in self._low.items()}

    def _positions(self) -> list[float]:
        counts = [0] * 6
        for pair, ident in enumerate((p.ID_JOINTS_12, p.ID_JOINTS_34, p.ID_JOINTS_56)):
            first, second = self._pairs.get(ident, (0, 0))
            counts[2 * pair], counts[2 * pair + 1] = first, second
        positions = [p.mdeg_to_rad(value) for value in counts]
        if self._has_gripper:
            positions.append(p.counts_to_metres(self._gripper[0]) if self._gripper is not None else 0.0)
        return positions

    def _problems(self, opening: bool = False) -> list[str]:
        """What stands against commanding this arm, in words — empty when nothing does."""
        out: list[str] = []
        status = self._status
        if status is None:
            return ["no status has been heard"]
        if status.arm_status != p.ARM_STATUS_NORMAL:
            out.append(f"arm status {status.arm_status:#04x} ({p.ARM_STATUSES.get(status.arm_status, 'unknown')})")
        if status.err_code:
            out.append(f"error word {status.err_code:#06x}")
        low = self._low_by_joint()
        for joint in range(6):
            report = low.get(joint)
            if report is None:
                continue
            bits = report.faults()
            if bits:
                names = ", ".join(name for bit, name in p.DRIVER_BIT_NAMES.items() if bits & bit)
                out.append(f"joint {joint + 1}'s driver reports {names}")
            if not opening and not report.enabled():
                out.append(f"joint {joint + 1}'s driver is disabled")
        if self._has_gripper and self._gripper is not None:
            gripper_status = self._gripper[2]
            bits = gripper_status & p.GRIPPER_FAULT_BITS
            if bits:
                names = ", ".join(name for bit, name in p.GRIPPER_BIT_NAMES.items() if bits & bit)
                out.append(f"the gripper reports {names}")
            if not opening and not gripper_status & p.GRIPPER_ENABLED:
                out.append("the gripper's driver is disabled")
        if not opening and status.ctrl_mode != p.CTRL_CAN:
            out.append(f"the arm is in {p.CTRL_MODES.get(status.ctrl_mode, 'mode ' + str(status.ctrl_mode))}, not CAN control")
        return out

    def _unwell(self) -> bool:
        """Running, but something is wrong that is not a fault: a report that has gone stale, or a cycle that did not complete."""
        now = self._clock()
        if self._status_at is None or now - self._status_at > _STALE_S:
            return True
        for ident in (p.ID_JOINTS_12, p.ID_JOINTS_34, p.ID_JOINTS_56):
            at = self._pairs_at.get(ident)
            if at is None or now - at > _STALE_S:
                return True
        if self._has_gripper and (self._gripper_at is None or now - self._gripper_at > _STALE_S):
            return True
        for joint in range(6):
            at = self._low_at.get(p.ID_LOW_SPEED + joint)
            if at is None or now - at > _STALE_S:
                return True
        return False

    def _fault_word(self) -> int:
        """The maker's words, composed: the error word low, the arm status above it, the union of the drivers' fault bits on top."""
        status = self._status
        word = 0
        if status is not None:
            word = ((status.arm_status & 0xFF) << 16) | (status.err_code & 0xFFFF)
        if status is not None and status.ctrl_mode != p.CTRL_CAN:
            word |= p.FAULT_NOT_CAN_CONTROL
        union = 0
        for report in self._low.values():
            union |= report.faults()
            if not report.enabled():
                union |= p.DRIVER_ENABLED
        if self._has_gripper and self._gripper is not None:
            if self._gripper[2] & p.GRIPPER_FAULT_BITS:
                word |= p.FAULT_GRIPPER
            if not self._gripper[2] & p.GRIPPER_ENABLED:
                union |= p.DRIVER_ENABLED
        return word | (union << 24)

    # ── the bus ──────────────────────────────────────────────────────────────

    def _frames(self, deadline: float):
        """Apply frames as they arrive and yield each one, until ``deadline``, until a synchronous bus has nothing more (it never will),
        or until `_WAIT_FRAME_CAP` frames. The last is what keeps a *broadcasting* arm on a bus whose clock a test owns from being
        waited on for ever: the broadcast has no end, and a clock that does not advance has no deadline."""
        assert self._bus is not None
        for _ in range(_WAIT_FRAME_CAP):
            remaining = deadline - self._clock()
            if remaining <= 0.0:
                return
            frame = self._bus.recv(remaining)
            if frame is None:
                if self._bus.synchronous:
                    return
                continue
            self._apply(frame)
            yield frame

    def _drain(self) -> None:
        """Apply what is already waiting, without waiting for more. The arm broadcasts, so on a bus that runs its peers when its
        inbox is empty ('read until empty' never ends) the bus says how many are waiting; on a real socket a bounded read of what
        the kernel holds is the same thing."""
        assert self._bus is not None
        waiting = self._bus.backlog()
        for _ in range(_DRAIN_LIMIT if waiting is None else waiting):
            frame = self._bus.recv()
            if frame is None:
                return
            self._apply(frame)

    def _collect_cycle(self) -> int:
        """How many of the position frames arrived fresh, waiting up to the window for the rest."""
        wanted = {p.ID_STATUS, p.ID_JOINTS_12, p.ID_JOINTS_34, p.ID_JOINTS_56}
        got: set[int] = set()
        for frame in self._frames(self._clock() + self._window_s):
            if frame.arbitration_id in wanted:
                got.add(frame.arbitration_id)
            if got == wanted:
                break
        # Whatever else came with them is this cycle's, too: the drivers' reports follow the angles on the arm's loop.
        self._drain()
        return len(got)

    def _apply(self, frame: Frame) -> None:
        ident, data = frame.arbitration_id, bytes(frame.data)
        now = self._clock()
        try:
            if ident == p.ID_STATUS:
                self._status = p.parse_status(data)
                self._status_at = now
            elif ident in (p.ID_JOINTS_12, p.ID_JOINTS_34, p.ID_JOINTS_56):
                self._pairs[ident] = p.parse_joint_pair(data)
                self._pairs_at[ident] = now
            elif ident == p.ID_GRIPPER:
                self._gripper = p.parse_gripper(data)
                self._gripper_at = now
            elif p.ID_LOW_SPEED <= ident < p.ID_LOW_SPEED + 6:
                self._low[ident] = p.parse_low_speed(data)
                self._low_at[ident] = now
            elif p.ID_HIGH_SPEED <= ident < p.ID_HIGH_SPEED + 6:
                p.parse_high_speed(data)  # read for its length: this driver does not report motor-side speed
            elif ident == p.ID_LIMIT_REPORT:
                report = p.parse_limit_report(data)
                self._reports[report.motor_num] = report
            else:
                self.unexpected_frames += 1
        except ValueError:
            self.unexpected_frames += 1

    # ── the descriptor ───────────────────────────────────────────────────────

    def _build_descriptor(self) -> str:
        joints = []
        for index, name in enumerate(self._names):
            low, high = self._travel[index]
            gripper = index == 6
            joints.append({
                "name": name,
                "role": self._roles.get(name),
                "type": "prismatic" if gripper else "revolute",
                "limits": {
                    ("position_min_m" if gripper else "position_min_rad"): low,
                    ("position_max_m" if gripper else "position_max_rad"): high,
                },
                "control_modes": ["position"],
            })
        descriptor = {
            "schema_version": 1,
            "robot_slug": "agilex-piper",
            "digest": None,
            "verified": False,
            "simulator": self._simulated,
            "base": "fixed",
            "dof": len(self._names),
            "joints": joints,
            "end_effectors": [],
            "sensors": [],
            "control": {
                "modes": ["position"],
                "per_joint_modes": False,
                "rate_hz_max": 200,
                "rate_hz_min": 1,
            },
            # Null: no rated envelope is published for this arm that a driver could cite, and the limits above are the
            # travel, not a risk assessment (§63.4).
            "envelope": None,
            # The fast stop is a software stop — a frame, a latch and refused commands — and the descriptor says so; the
            # rated stop is the arm's own button and a wire has no way to reach it.
            "capabilities": ["safety.estop"],
            "extensions": [],
            "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))


def _say(message: str) -> None:
    import sys

    print(f"skynet-piper: {message}", file=sys.stderr, flush=True)

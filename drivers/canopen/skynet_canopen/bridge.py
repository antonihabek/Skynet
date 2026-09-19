"""The RHAL driver: CiA 402 axes on a CANopen bus, as one body.

§4's five mandatory entry points over :mod:`skynet_canopen.frames`, one CANopen node per joint.

# What it reads out of the drives rather than out of a configuration file

Four things, and each of them is a refusal when the drive will not say:

| From | Object | Used for |
|---|---|---|
| encoder resolution | `0x608F:1`, `0x608F:2` | increments to radians |
| gear ratio | `0x6091:1`, `0x6091:2` | motor revolutions to shaft revolutions |
| software position limits | `0x607D:1`, `0x607D:2` | the descriptor's travel |
| max motor speed | `0x6080` | the descriptor's velocity ceiling |

The first two are the ones worth the paragraph. A position in increments means nothing without
them, and a driver that assumed the common 65536 counts per revolution and a direct drive would
report a joint angle wrong by the gearbox — a factor of fifty on an industrial axis — **in radians,
to a Safety Kernel that bounds travel with it**. §5.2's rule is that a limit with no provenance is a
rumour; a unit with no provenance is worse, because it is silently applied to every number the body
reports. So `open()` refuses a drive that answers `0x608F` with an abort, and the body does not come
up.

What it does **not** read is a torque rating. `0x6076` is the *motor's* rated torque, which is a
design value from a data sheet and not a joint safety rating (§63.4) — the same distinction that
makes `skynet-urdf` write a literal null rather than a URDF's effort limit. So the descriptor
declares no `effort_max_nm` and `envelope: null`, and the guards that need one report unavailable.
That is the correct and slightly annoying answer.

# What it verifies before it commands anything

**The PDO mapping.** This driver speaks TPDO2 and RPDO2 under CiA 402's default mapping —
`statusword + position actual` out, `controlword + target position` in. A drive that has been
remapped will read a target position as something else entirely, and the frame is well-formed
either way. So `open()` reads `0x1A01` and `0x1601` back and refuses a drive whose mapping is not
the one this driver speaks. Assuming the default is the industry's most common source of an axis
that moves to a number nobody sent.

**The state.** The bridge walks each drive `shutdown → switch on → enable operation` and reads the
statusword back at each step rather than trusting the transition it just requested. A drive that
does not reach `operation enabled` is reported by name, with the state it did reach — and a master
that skipped the read-back would write setpoints into a disabled drive, see them acknowledged on the
bus, and see the axis stand still.

# What it does not pretend about the stop

`emergency_stop()` sends CiA 402 quick-stop to every axis and then NMT pre-operational, which stops
the drives it can reach over the bus it has, latches, and refuses every later command. **That is a
software stop and the descriptor declares it as one** — the `safety.estop` capability, in the
tradition `drivers/ur_rtde` sets. What it is not is a safety-rated stop: a rated stop is an STO
input wired to a relay, and it is not reachable from a CAN frame, so the envelope carries no rated
stop and §15's `safety.estop.independent` is a bench test nobody can pass from here.
"""

from __future__ import annotations

import json
import math
import time

import skynet_rhal as rhal

from .bus import Bus, SocketCanBus
from .cia402 import OD, Command, Mode, State, controlword_for, increments_to_rad, rad_to_increments
from .cia402 import state_of
from .frames import (
    NMT_ID,
    NMT_PRE_OPERATIONAL,
    NMT_START,
    SYNC_ID,
    Frame,
    SdoAbort,
    parse_sdo_response,
    parse_tpdo2,
    rpdo2,
    sdo_download_request,
    sdo_request_id,
    sdo_upload_request,
    sdo_response_id,
    tpdo2_id,
)

__all__ = ["CanOpenBridge", "Axis", "BridgeRefusal"]

#: The default mapping this driver speaks, as CiA 301 packs a mapping entry.
_TPDO2_EXPECTED = ((0x6041 << 16) | 16, (0x6064 << 16) | 32)
_RPDO2_EXPECTED = ((0x6040 << 16) | 16, (0x607A << 16) | 32)

#: How long to wait for a drive to answer an SDO before giving up on the body.
_SDO_TIMEOUT_S = 0.5

#: How long `read_state` waits, after its SYNC, for every axis's TPDO2 — the default, which
#: `sync_window_ms` in the configuration replaces. A TPDO2 under the default mapping is a six-byte
#: frame, about 111 bits or 0.11 ms at 1 Mbit/s, so five milliseconds is some forty axes answering
#: back to back on a fast bus, or a handful on a slow one. It is a bound, not a sleep: the wait ends
#: the moment the last axis has answered.
_SYNC_WINDOW_S = 0.005


class BridgeRefusal(Exception):
    """A reason this body did not come up, in the terms an integrator acts on."""


class Axis:
    """One drive, and the numbers read off it."""

    def __init__(self, node: int, name: str, role: str | None) -> None:
        self.node = node
        self.name = name
        self.role = role
        self.increments = 0
        self.revolutions = 0
        self.gear_motor = 0
        self.gear_shaft = 0
        self.limit_min = 0
        self.limit_max = 0
        self.max_motor_rpm = 0
        self.position = 0
        self.statusword = 0
        self.target = 0
        #: Set once the drive has reported `operation enabled` itself.
        self.enabled = False

    def to_rad(self, increments_value: int) -> float:
        return increments_to_rad(
            increments_value, self.increments, self.revolutions, self.gear_motor, self.gear_shaft
        )

    def to_increments(self, radians: float) -> int:
        return rad_to_increments(
            radians, self.increments, self.revolutions, self.gear_motor, self.gear_shaft
        )

    def state(self) -> State:
        return state_of(self.statusword)


class CanOpenBridge:
    """A body made of CiA 402 axes."""

    def __init__(self, bus: Bus | None = None) -> None:
        self._bus = bus
        self._owns_bus = bus is None
        #: Whether this body is simulated (§35.3). See `_build_descriptor`.
        self._simulated = True
        self.axes: list[Axis] = []
        self._descriptor: str | None = None
        self._seq = 0
        self._estop = False
        self._opened = False
        #: Frames that arrived on an identifier this driver does not speak. Counted rather than
        #: dropped in silence: on a shared bus a rising count is somebody else's traffic, and on a
        #: dedicated one it is a drive doing something nobody asked for.
        self.unexpected_frames = 0
        #: `read_state`'s bound on waiting for this SYNC's TPDO2s. See `_SYNC_WINDOW_S`.
        self._sync_window_s = _SYNC_WINDOW_S

    # ── §4's entry points ────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError as error:
            _say(f"the configuration is not JSON: {error}")
            return rhal.SKR_EINVAL
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
        joints = settings.get("joints") or []
        if not joints:
            raise BridgeRefusal(
                "no joints in the configuration. This driver cannot discover them: CANopen's "
                "scan tells you which node ids answer, and nothing on the bus says which axis of "
                "which arm a node is, nor what it should be called. Give "
                '`{"joints": [{"node": 1, "name": "J1", "role": "right_arm.shoulder.pitch"}]}`.'
            )
        if self._bus is None:
            interface = settings.get("interface", "vcan0")
            self._bus = SocketCanBus(interface)
        # §35.3 caps a simulated body at guest, so the dangerous direction is declaring `false`
        # about something that is not real — and this driver genuinely cannot tell: a CiA 402 drive
        # and a simulator of one answer identically, which is what makes the simulator useful. So
        # the default is `true` and a real integration says `{"simulator": false}` deliberately. A
        # loopback bus overrides even that, because there is demonstrably no machine on it.
        self._simulated = bool(settings.get("simulator", True)) or self._bus.synchronous
        window_ms = settings.get("sync_window_ms", _SYNC_WINDOW_S * 1000.0)
        if (
            isinstance(window_ms, bool)
            or not isinstance(window_ms, (int, float))
            or not 0.0 < window_ms <= 1000.0
        ):
            raise BridgeRefusal(
                f"`sync_window_ms` of {window_ms!r} is not a window. It bounds how long a read "
                "waits for every axis to answer its SYNC, in milliseconds, above zero and at most "
                "a second — a longer one would be a control loop waiting on a bus that has gone."
            )
        self._sync_window_s = float(window_ms) / 1000.0

        for entry in joints:
            node = entry.get("node")
            name = entry.get("name")
            if not isinstance(node, int) or not 1 <= node <= 127:
                raise BridgeRefusal(f"`{node}` is not a CANopen node id (1 to 127).")
            if not name:
                raise BridgeRefusal(f"node {node} has no `name`; a joint the OS cannot name is one "
                                    "no policy channel can bind to.")
            self.axes.append(Axis(node, name, entry.get("role")))

        # Order matters here and the suite found out why. See `_start`: a latched fault is read
        # *before* anything that could clear it.
        for axis in self.axes:
            self._start(axis)
        for axis in self.axes:
            self._refuse_if_faulted(axis)
        for axis in self.axes:
            self._read_units(axis)
            self._check_mapping(axis)
            self._read_limits(axis)
        for axis in self.axes:
            self._set_mode(axis, Mode.CSP)
            self._enable(axis)
        self._descriptor = self._build_descriptor()

    def close(self) -> int:
        if self._bus is not None:
            for axis in self.axes:
                try:
                    self._sdo_write(axis, OD.CONTROLWORD, controlword_for(Command.DISABLE_VOLTAGE))
                except (BridgeRefusal, SdoAbort):
                    pass  # closing a body whose drives have gone away is still closing it
            self._bus.send(Frame(NMT_ID, bytes([NMT_PRE_OPERATIONAL, 0])))
            if self._owns_bus:
                self._bus.close()
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        assert self._bus is not None
        # Anything still on the bus answers an *earlier* SYNC, and reading it after this one would
        # report the body as it was a period ago under this read's stamp. So it is taken first —
        # still applied, because a late position is newer than none — and the SYNC after it is the
        # one whose answers this read waits for.
        self._drain()
        # §24: stamped at acquisition. A CiA 402 drive samples its synchronous PDOs on SYNC, so the
        # instant the SYNC leaves is the instant the positions below were taken.
        acquired = rhal.acquisition_ns()
        self._bus.send(Frame(SYNC_ID, b""))
        seen = self._collect_tpdo()
        self._seq += 1

        out.stamp_ns = acquired
        out.seq = self._seq
        out.flags = rhal.HOMING_NOT_REQUIRED
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP
        out.fault_code = 0
        out.position_rad = []
        out.velocity_rad_s = []
        out.effort_nm = []
        faulted = False
        for axis in self.axes:
            out.position_rad.append(axis.to_rad(axis.position))
            # **NaN, not zero.** TPDO2's default mapping carries the statusword and the position and
            # nothing else, so velocity and torque are quantities this driver does not measure on
            # this cycle. §24's rule, and a zero would read as "this joint is still" every cycle.
            out.velocity_rad_s.append(math.nan)
            out.effort_nm.append(math.nan)
            if axis.state() in (State.FAULT, State.FAULT_REACTION_ACTIVE):
                faulted = True
                out.fault_code = axis.node
        if faulted:
            out.flags |= rhal.FLAG_DEGRADED | rhal.FLAG_FAULT
        if 0 < seen < len(self.axes):
            # Some axes answered this SYNC and some did not, so some of the positions above are a
            # period or more old under a fresh stamp. Reported rather than smoothed over: the body
            # is running, and something on its bus is wrong.
            out.flags |= rhal.FLAG_DEGRADED
        if seen == 0:
            # Every drive silent for a whole cycle. The sequence still advances — the OS's watchdog
            # is watching the *stamp* and this read did happen — but a body that answered nothing is
            # a bus problem and the caller is told so rather than handed the previous positions.
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        if cmd.joint_count != len(self.axes):
            return rhal.SKR_EINVAL
        if cmd.mode != rhal.MODE_POSITION:
            # Only CSP is wired. CSV and CST are one object write and a different RPDO mapping away,
            # and shipping them untested against a drive that has never run them is how a torque
            # command becomes a position command.
            return rhal.SKR_EINVAL
        if self._estop:
            return rhal.SKR_EPERM
        assert self._bus is not None

        enable = controlword_for(Command.ENABLE_OPERATION)
        for index, axis in enumerate(self.axes):
            if not axis.enabled:
                continue
            applied = cmd.mask[index] if index < len(cmd.mask) else 1
            if applied:
                value = cmd.values[index]
                if not math.isfinite(value):
                    return rhal.SKR_EINVAL
                axis.target = axis.to_increments(value)
            # A held joint keeps its previous target, which is what a hold *is*: the RPDO still goes
            # out, because a drive in cyclic synchronous position mode treats a missing setpoint as
            # a communication fault rather than as a hold.
            self._bus.send(rpdo2(axis.node, enable, axis.target))
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        self._estop = True
        if self._bus is None:
            return rhal.SKR_OK
        quick_stop = controlword_for(Command.QUICK_STOP)
        for axis in self.axes:
            self._bus.send(rpdo2(axis.node, quick_stop, axis.target))
        self._bus.send(Frame(NMT_ID, bytes([NMT_PRE_OPERATIONAL, 0])))
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── CANopen ──────────────────────────────────────────────────────────────

    def _start(self, axis: Axis) -> None:
        """Bring the node to NMT operational so its PDOs flow. **Nothing more.**

        There is no `NMT reset node` here, and the omission is the point. A reset returns the drive's
        state machine to `switch on disabled` — which clears a latched **fault**, silently, before
        anybody has read why it latched. A driver that reset every axis at open would erase the one
        piece of evidence about why a robot stopped, on every restart, and would present a faulted
        machine as a healthy one that had simply needed bringing up.
        
        The suite is what established this rather than the design: `a drive that has latched a fault
        is refused` failed while a reset was here, and it failed because the reset had already
        cleared the fault the test had just set.
        """
        assert self._bus is not None
        self._bus.send(Frame(NMT_ID, bytes([NMT_START, axis.node])))

    def _refuse_if_faulted(self, axis: Axis) -> None:
        """Read the state before touching anything, and refuse a drive that has latched a fault."""
        axis.statusword = self._sdo_read(axis, OD.STATUSWORD)
        state = axis.state()
        if state in (State.FAULT, State.FAULT_REACTION_ACTIVE):
            raise BridgeRefusal(
                f"node {axis.node} is in `{state.value}` (statusword {axis.statusword:#06x}). This "
                "body does not come up over a latched fault. Read 0x603F for the error code and "
                "clear it at the drive deliberately — a driver that sent a fault reset here would "
                "restart a machine whose reason for stopping nobody had looked at, and would do it "
                "again on the next power cycle."
            )

    def _sdo_read(self, axis: Axis, obj) -> int:
        assert self._bus is not None
        self._bus.send(sdo_upload_request(axis.node, obj.index, obj.subindex))
        value = self._await_sdo(axis, obj)
        if value is None:
            raise BridgeRefusal(
                f"node {axis.node} acknowledged a read of {obj.name} as a write, which is a drive "
                "whose SDO server does not follow CiA 301."
            )
        return _sign(value, obj.size, obj.signed)

    def _sdo_write(self, axis: Axis, obj, value: int) -> None:
        assert self._bus is not None
        self._bus.send(
            sdo_download_request(axis.node, obj.index, obj.subindex, value, obj.size)
        )
        self._await_sdo(axis, obj)

    def _await_sdo(self, axis: Axis, obj) -> int | None:
        assert self._bus is not None
        deadline = time.monotonic() + _SDO_TIMEOUT_S
        while time.monotonic() <= deadline:
            frame = self._bus.recv(0.01)
            if frame is None:
                # A synchronous bus delivers inside `recv`, so nothing arriving means nothing will;
                # a real one answers some microseconds later and the deadline is what waits for it.
                # Without the distinction this is either half a second burned on every test or a
                # real drive given up on after one empty poll.
                if self._bus.synchronous:
                    break
                continue
            if frame.arbitration_id != sdo_response_id(axis.node):
                self._stash(frame)
                continue
            try:
                return parse_sdo_response(frame.data, obj.index, obj.subindex)
            except SdoAbort as abort:
                raise BridgeRefusal(f"node {axis.node}, {obj.name}: {abort.explain()}") from abort
            except ValueError as error:
                raise BridgeRefusal(f"node {axis.node}, {obj.name}: {error}") from error
        raise BridgeRefusal(
            f"node {axis.node} did not answer a read of {obj.name} "
            f"({obj.index:#06x}:{obj.subindex:02x}) within {_SDO_TIMEOUT_S:.1f}s. A node that is "
            "not on the bus and a node that is powered off look the same from here; `candump` is "
            "what tells them apart."
        )

    def _stash(self, frame: Frame) -> None:
        """A frame that arrived while waiting for an SDO response."""
        for axis in self.axes:
            if frame.arbitration_id == tpdo2_id(axis.node):
                axis.statusword, axis.position = parse_tpdo2(frame.data)
                return
            if frame.arbitration_id == sdo_response_id(axis.node):
                return  # another axis's transfer; its own wait will not see it, and says so
            if frame.arbitration_id == 0x700 + axis.node or frame.arbitration_id == 0x080 + axis.node:
                return  # heartbeat and emergency objects are expected traffic
        self.unexpected_frames += 1

    def _read_units(self, axis: Axis) -> None:
        try:
            axis.increments = self._sdo_read(axis, OD.ENCODER_INCREMENTS)
            axis.revolutions = self._sdo_read(axis, OD.ENCODER_REVOLUTIONS)
            axis.gear_motor = self._sdo_read(axis, OD.GEAR_MOTOR_REVOLUTIONS)
            axis.gear_shaft = self._sdo_read(axis, OD.GEAR_SHAFT_REVOLUTIONS)
        except BridgeRefusal as refusal:
            raise BridgeRefusal(
                f"{refusal}\n\nThis body does not come up without it. An increment is not an angle "
                "until the drive says how many of them make a turn, and a driver that assumed a "
                "common value would report every joint's position in radians, wrong by whatever "
                "the real drive uses, to a kernel that bounds travel with it (§5.2)."
            ) from refusal
        if axis.increments <= 0 or axis.revolutions <= 0 or axis.gear_shaft <= 0:
            raise BridgeRefusal(
                f"node {axis.node} reports an encoder resolution of {axis.increments}/"
                f"{axis.revolutions} and a gear ratio of {axis.gear_motor}/{axis.gear_shaft}. A "
                "zero in either is not a ratio, and dividing by it would produce an angle nobody "
                "measured."
            )

    def _check_mapping(self, axis: Axis) -> None:
        for count_obj, entries, expected, which in (
            (OD.TPDO2_MAPPING_COUNT, (OD.TPDO2_MAPPING_1, OD.TPDO2_MAPPING_2), _TPDO2_EXPECTED, "TPDO2"),
            (OD.RPDO2_MAPPING_COUNT, (OD.RPDO2_MAPPING_1, OD.RPDO2_MAPPING_2), _RPDO2_EXPECTED, "RPDO2"),
        ):
            count = self._sdo_read(axis, count_obj)
            found = tuple(self._sdo_read(axis, entry) for entry in entries)
            if count != 2 or found != expected:
                raise BridgeRefusal(
                    f"node {axis.node}'s {which} maps "
                    + ", ".join(f"{value:#010x}" for value in found[:count])
                    + f" and this driver speaks the CiA 402 default, "
                    + ", ".join(f"{value:#010x}" for value in expected)
                    + ". A frame under another mapping is well-formed and means something else, so "
                    "it is refused rather than sent: an axis that moves to a number nobody wrote is "
                    "the failure this check exists for. Remap the drive, or use a driver that reads "
                    "its mapping."
                )

    def _read_limits(self, axis: Axis) -> None:
        axis.limit_min = self._sdo_read(axis, OD.POSITION_LIMIT_MIN)
        axis.limit_max = self._sdo_read(axis, OD.POSITION_LIMIT_MAX)
        axis.max_motor_rpm = self._sdo_read(axis, OD.MAX_MOTOR_SPEED)

    def _set_mode(self, axis: Axis, mode: Mode) -> None:
        self._sdo_write(axis, OD.MODES_OF_OPERATION, mode.value)
        display = self._sdo_read(axis, OD.MODES_DISPLAY)
        if display != mode.value:
            raise BridgeRefusal(
                f"node {axis.node} was asked for mode {mode.value} ({mode.name}) and reports mode "
                f"{display}. A drive running a profile mode generates its own trajectory between "
                "setpoints, which is a second interpolator that does not know the Safety Kernel "
                "exists."
            )

    def _enable(self, axis: Axis) -> None:
        for command, expected in (
            (Command.SHUTDOWN, State.READY_TO_SWITCH_ON),
            (Command.SWITCH_ON, State.SWITCHED_ON),
            (Command.ENABLE_OPERATION, State.OPERATION_ENABLED),
        ):
            self._sdo_write(axis, OD.CONTROLWORD, controlword_for(command))
            axis.statusword = self._sdo_read(axis, OD.STATUSWORD)
            reached = axis.state()
            if reached is not expected:
                raise BridgeRefusal(
                    f"node {axis.node} was asked to {command.value} and reports "
                    f"`{reached.value}` rather than `{expected.value}` (statusword "
                    f"{axis.statusword:#06x}). "
                    + (
                        "A drive in `fault` has latched something; read 0x603F for the code and "
                        "clear it at the drive before the body will come up."
                        if reached in (State.FAULT, State.FAULT_REACTION_ACTIVE)
                        else "The state is read back rather than assumed, because a master that "
                        "trusted its own transition would write setpoints into a disabled drive "
                        "and watch the axis stand still."
                    )
                )
        axis.enabled = True

    def _drain(self) -> None:
        """Apply everything already waiting, without waiting for more."""
        assert self._bus is not None
        while True:
            frame = self._bus.recv()
            if frame is None:
                return
            self._stash(frame)

    def _collect_tpdo(self) -> int:
        """How many axes answered this SYNC, waiting up to the window for the rest.

        It used to read whatever was waiting and return, which is right on the loopback bus — the
        drives answer inside `recv` — and wrong on every other: on a kernel bus the answers arrive
        microseconds after the SYNC, so the first read of a session found nothing and returned
        `ETIMEDOUT`, and every read after it reported the *previous* SYNC's positions under its own
        stamp. `make check-vcan` is what found it (DRY-COVERAGE-PLAN.md F71): the suite had only
        ever run on a bus that cannot be late.
        """
        assert self._bus is not None
        waiting = {axis.node for axis in self.axes}
        deadline = time.monotonic() + self._sync_window_s
        while waiting:
            remaining = deadline - time.monotonic()
            frame = self._bus.recv(max(remaining, 0.0))
            if frame is None:
                if self._bus.synchronous or remaining <= 0.0:
                    break
                continue
            for axis in self.axes:
                if frame.arbitration_id == tpdo2_id(axis.node):
                    axis.statusword, axis.position = parse_tpdo2(frame.data)
                    waiting.discard(axis.node)
                    break
            else:
                self._stash(frame)
        return len(self.axes) - len(waiting)

    # ── the descriptor ───────────────────────────────────────────────────────

    def _build_descriptor(self) -> str:
        joints = []
        for axis in self.axes:
            limits = {
                "position_min_rad": axis.to_rad(axis.limit_min),
                "position_max_rad": axis.to_rad(axis.limit_max),
            }
            if axis.max_motor_rpm > 0 and axis.gear_motor > 0:
                # rpm at the motor to rad/s at the shaft.
                limits["velocity_max_rad_s"] = (
                    axis.max_motor_rpm / 60.0 * 2.0 * math.pi * (axis.gear_shaft / axis.gear_motor)
                )
            # **No `effort_max_nm`.** `0x6076` is the motor's rated torque: a design value from a
            # data sheet, not a joint safety rating (§63.4). The same distinction that makes
            # `skynet-urdf` write a literal null rather than a URDF's effort limit.
            joints.append(
                {
                    "name": axis.name,
                    "role": axis.role,
                    "type": "revolute",
                    "limits": limits,
                    "control_modes": ["position"],
                }
            )
        descriptor = {
            "schema_version": 1,
            "robot_slug": "canopen-cia402-axes",
            "digest": None,
            "verified": False,
            # A simulated bus is a simulated body. On a real `vcan0` with real drives this is
            # `False`, and the driver has no way to tell — which is why it is configuration rather
            # than a guess: `{"simulator": true}` is what the harness sets, and a real integration
            # leaves it out.
            "simulator": self._simulated,
            "base": "fixed",
            "dof": len(self.axes),
            "joints": joints,
            "end_effectors": [],
            "sensors": [],
            "control": {
                "modes": ["position"],
                "rate_hz_max": 1000,
                "rate_hz_min": 1,
            },
            # Null, not an empty object: nobody has declared a safety envelope for this body, and
            # the drives' own limits are the drives' rather than a risk assessment's.
            "envelope": None,
            # `emergency_stop` quick-stops every axis, drops the bus to pre-operational, latches,
            # and refuses later commands — the software stop the driver actually has. The rated
            # stop stays undeclared for the module docstring's reason.
            "capabilities": ["safety.estop"],
            "extensions": [],
            "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))


def _sign(value: int, width: int, signed: bool) -> int:
    if not signed:
        return value
    bits = 8 * width
    return value - (1 << bits) if value >= (1 << (bits - 1)) else value


def _say(message: str) -> None:
    import sys

    print(f"skynet-canopen: {message}", file=sys.stderr, flush=True)

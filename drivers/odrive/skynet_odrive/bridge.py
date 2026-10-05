"""An ODrive body over CANSimple: one Skynet joint per axis, on one CAN bus.

The wire is ODrive Robotics' CANSimple protocol (``Firmware/communication/can``): an 11-bit
identifier with the node id up top and the command below, heartbeat and encoder estimates
cyclic, everything else asked for. What the driver speaks of it:

| Asked, every cycle | ``GET_ENCODER_ESTIMATES`` (position and velocity, in turns) |
| Asked at open | a heartbeat (axis error and state — the only health on the wire) |
| Commanded | ``SET_INPUT_POS`` / ``SET_INPUT_VEL`` / ``SET_INPUT_TORQUE``, after ``SET_CONTROLLER_MODES`` when the mode changed |
| Stopped by | ``ESTOP`` (latches ``AXIS_ERROR_ESTOP_REQUESTED`` in the firmware) |

# What it reads, and what it will not invent

**Health is the heartbeat.** Every heartbeat carries the axis error word, the axis state, and
whether the motor, encoder and controller hold an error of their own. A body whose axes do
not send one does not come up: without it an error would sit in the firmware unread while
the driver reported positions. The cyclic rates are the integrator's configuration and this
driver never assumes them — the estimates are *requested* every cycle rather than awaited,
so an axis with its cyclic estimates switched off still answers.

**Errors are refused, never cleared.** A nonzero axis error reads ``FAULT`` and ``DEGRADED``
with the word verbatim, and every command is refused while it stands — a setpoint an axis
with a latched error drops is success the robot did not have. Clearing is the person's act
at the drive (``CLEAR_ERRORS`` is board-wide in the firmware, which is one more reason this
driver never sends it). The stop is the exception that proves it: ``ESTOP`` latches an
error in the firmware by design, so after a stop there is no software release —
``release_stop`` answers ENOSYS and a new session returns the body (§4, the Spot precedent).
``open`` refuses while any axis holds the error, so the session is renewed over a drive the
person cleared deliberately, never over one the software cleared on the way in.

**No rating is declared.** The wire publishes no travel, no velocity rating and no torque
rating: nothing on it says what motor is attached to an axis, and the limits the firmware
holds (``vel_limit``, ``current_lim``) are integrator configuration the protocol cannot even
read back. So the joints are ``continuous`` with no limits and the envelope is null — the
``canopen`` precedent — and the guards that need a figure report unavailable. A driver that
transcribed the ODrive's own current limit as a joint torque rating would be enforcing the
integrator's configuration as a safety claim.

**Effort is NaN.** ``GET_IQ`` reports amps, and amps become newton-metres through the
motor's torque constant, which is configuration and not on the wire. §24's rule.

# Modes

Position, velocity and torque, per joint (§86.1): each axis is independent and the three
setpoints are three commands, so a mixed command is one frame per joint rather than a mode
the body has to share. The input mode is always passthrough — the kernel's setpoint reaches
the controller unshaped. Anything else the command carries is refused, not coerced.
"""

from __future__ import annotations

import json
import math
import struct
import time

import skynet_rhal as rhal
from skynet_canopen.bus import Bus, SocketCanBus
from skynet_canopen.frames import Frame

from . import cansimple

__all__ = ["ODriveBridge", "Axis", "BridgeRefusal"]

#: How long to wait for one axis to answer one request before giving up on the body.
_REQUEST_TIMEOUT_S = 0.5

#: How long `open()` waits for an axis's heartbeat. The firmware's default cyclic rate is
#: 100 ms; five of those is margin for a loaded bus, and an axis that sends none in half a
#: second has its cyclic health switched off — the body this driver does not command.
_HEARTBEAT_TIMEOUT_S = 0.5

#: How long `open()` keeps asking one axis for closed loop before it refuses the body.
_STATE_TIMEOUT_S = 1.0
_STATE_POLL_S = 0.01

#: How long `read_state` waits, after its requests, for every axis's estimates — the default,
#: which `window_ms` in the configuration replaces. A request and its reply are two short
#: frames, about a quarter of a millisecond at 1 Mbit/s, so fifty milliseconds is margin for
#: a busy bus, not a sleep: the wait ends the moment the last axis has answered.
_WINDOW_S = 0.050

#: How old a heartbeat may be before the axis it speaks for reads degraded. Health that has
#: not been heard from in a second is not health; it is the last thing the axis said.
_HEARTBEAT_STALE_S = 1.0

#: §5.1's canonical chains and axes, mirrored from `skynet-contracts`' roles table so
#: config-supplied roles are validated the way `parse_role` will read them — including its
#: trim-and-lowercase normalization. (Copied from `skynet_fourier`'s, which states the same;
#: a private validator is duplicated with its pointer rather than imported.)
_CHAINS = frozenset({"left_arm", "right_arm", "left_leg", "right_leg",
                     "left_hand", "right_hand", "torso", "neck", "head",
                     "base", "tail", "aux"})
_AXES = frozenset({"pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate",
                   "grip"})


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
        # ASCII digits only (`str.isdigit` admits superscripts), and at most
        # ten of them — a u32, the way `parse_role` bounds it.
        if not tail or len(tail) > 10 or not tail.isascii() \
                or not tail.isdigit() or int(tail) > 0xFFFFFFFF:
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
    """One ODrive axis, and the last thing it said."""

    def __init__(self, node: int, name: str, role: str | None) -> None:
        self.node = node
        self.name = name
        self.role = role
        self.pos_turns = 0.0
        self.vel_turns_s = 0.0
        #: The last heartbeat's state and error, or None until the first heartbeat arrives.
        self.axis_state: int | None = None
        self.axis_error = 0
        #: When the last heartbeat arrived, on the bridge's clock — None until the first.
        self.last_heartbeat: float | None = None
        #: The control mode the driver last set, or None until it sets one.
        self.control_mode: int | None = None

    def state_name(self) -> str:
        """The last heartbeat's state in the maker's words, or what is known without one."""
        if self.axis_state is None:
            return "no heartbeat yet"
        return cansimple.STATES.get(self.axis_state, f"state {self.axis_state}")


class ODriveBridge:
    """A body made of ODrive axes."""

    def __init__(self, bus: Bus | None = None, clock=time.monotonic) -> None:
        self._bus = bus
        self._owns_bus = bus is None
        self._clock = clock
        #: Whether this body is simulated (§35.3). See `_open`.
        self._simulated = True
        self.axes: list[Axis] = []
        self._descriptor: str | None = None
        self._seq = 0
        self._estop = False
        self._opened = False
        #: Frames that arrived on an identifier this driver does not speak, or too short to
        #: parse. Counted rather than dropped in silence: on a shared bus a rising count is
        #: somebody else's traffic, and on a dedicated one it is an axis doing something
        #: nobody asked for.
        self.unexpected_frames = 0
        #: `read_state`'s bound on waiting for this cycle's estimates. See `_WINDOW_S`.
        self._window_s = _WINDOW_S

    # ── §4's entry points ────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError as error:
            _say(f"the configuration is not JSON: {error}")
            return rhal.SKR_EINVAL
        # A body, and the stop latched on it, belong to one session. Reopening clears both:
        # the axes below are re-armed by the enable sequence, and a latch from an earlier
        # session is not evidence about this one.
        self.axes = []
        self._descriptor = None
        self._estop = False
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
                "no joints in the configuration. This driver cannot discover them: a node id "
                "says which axis on the bus, and nothing on the bus says which joint of which "
                "body an axis is, nor what it should be called. Give "
                '`{"joints": [{"node": 0, "name": "J1", "role": "right_arm.shoulder.pitch"}]}`.'
            )
        if self._bus is None:
            interface = settings.get("interface", "vcan0")
            self._bus = SocketCanBus(interface)
        # Anything already on the bus answers an earlier session — including this bridge's
        # own previous one, whose close handshake may never have been pumped and whose
        # heartbeats would otherwise detonate on this session's first wait. It is discarded,
        # not applied: a stale heartbeat's error would refuse this body for the last
        # session's fault, and a stale clean bill would clear it for the last session's
        # health. Fresh beats arrive with the presence replies below.
        while self._bus.recv() is not None:
            pass
        # §35.3 caps a simulated body at guest, so the dangerous direction is declaring `false`
        # about something that is not real — and this driver genuinely cannot tell: a CANSimple
        # axis and a simulator of one answer identically, which is what makes the simulator
        # useful. So the default is `true` and a real integration says `{"simulator": false}`
        # deliberately. A loopback bus overrides even that, because there is demonstrably no
        # machine on it.
        self._simulated = bool(settings.get("simulator", True)) or self._bus.synchronous
        window_ms = settings.get("window_ms", _WINDOW_S * 1000.0)
        if (
            isinstance(window_ms, bool)
            or not isinstance(window_ms, (int, float))
            or not 0.0 < window_ms <= 1000.0
        ):
            raise BridgeRefusal(
                f"`window_ms` of {window_ms!r} is not a window. It bounds how long a read "
                "waits for every axis to answer its estimates request, in milliseconds, above "
                "zero and at most a second — a longer one would be a control loop waiting on "
                "a bus that has gone."
            )
        self._window_s = float(window_ms) / 1000.0

        for entry in joints:
            node = entry.get("node")
            name = entry.get("name")
            if isinstance(node, bool) or not isinstance(node, int) or not 0 <= node <= 63:
                raise BridgeRefusal(f"`{node}` is not a CANSimple node id (0 to 63).")
            if not name:
                raise BridgeRefusal(f"node {node} has no `name`; a joint the OS cannot name is one "
                                    "no policy channel can bind to.")
            if any(axis.node == node for axis in self.axes):
                raise BridgeRefusal(
                    f"node {node} is named by two joints. One axis is one joint: two joints "
                    "bound to it would send it two setpoints a cycle and report one shaft twice."
                )
            role = entry.get("role")
            if role is not None and _normalise_role(role) is None:
                raise BridgeRefusal(
                    f"`{role}` is not a §5.1 role (chain.link[.axis][#index] over the canonical "
                    "chains and axes). A joint with an unparseable role can only ever match by "
                    "exact name — but that is the planner's fallback for a body the vocabulary "
                    "does not describe, not a spelling this driver accepts in configuration."
                )
            self.axes.append(Axis(node, name, _normalise_role(role) if role is not None else None))

        # Order matters: a latched fault is read *before* anything that could move the axis.
        for axis in self.axes:
            self._refuse_if_absent(axis)
            self._await_heartbeat(axis)
            self._refuse_if_faulted(axis)
        for axis in self.axes:
            self._set_modes(axis, cansimple.CONTROL_MODE_POSITION)
            self._request_closed_loop(axis)
        self._descriptor = self._build_descriptor()

    def close(self) -> int:
        if self._bus is not None:
            for axis in self.axes:
                try:
                    self._bus.send(Frame(
                        cansimple.msg_id(axis.node, cansimple.CMD_SET_REQUESTED_STATE),
                        cansimple.set_requested_state(cansimple.STATE_IDLE),
                    ))
                except OSError:
                    pass  # closing a body whose bus has gone away is still closing it
            if self._owns_bus:
                self._bus.close()
                # Dropped, so the next `open` binds a socket rather than sending on a closed one.
                self._bus = None
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        assert self._bus is not None
        # Anything still on the bus answers an *earlier* cycle, and applying it after this
        # one's requests would report the body as it was a period ago under this read's stamp.
        # So it is taken first — still applied, because a late reading is newer than none —
        # and the requests after it are the ones whose answers this read waits for.
        self._drain()
        # §24: stamped at acquisition. The estimates below are sampled when the axis reads
        # each request, so the instant the first request leaves is the instant they were taken.
        acquired = rhal.acquisition_ns()
        for axis in self.axes:
            ident, payload = cansimple.request(axis.node, cansimple.CMD_GET_ENCODER_ESTIMATES)
            self._bus.send(Frame(ident, payload))
        seen = self._collect_estimates()
        # And everything that arrived while the estimates were being collected — the flags
        # below are this cycle's verdict on the body, and a heartbeat that arrived during
        # the wait is this cycle's knowledge. Without this second drain a fault (or a clear)
        # would report a cycle late: the estimates retire their axes out from under the
        # heartbeats behind them.
        self._drain()
        self._seq += 1

        out.stamp_ns = acquired
        out.seq = self._seq
        out.flags = rhal.HOMING_UNREPORTED
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP
        out.fault_code = 0
        out.position_rad = []
        out.velocity_rad_s = []
        out.effort_nm = []
        faulted = False
        unwell = False
        now = self._clock()
        for axis in self.axes:
            out.position_rad.append(cansimple.turns_to_rad(axis.pos_turns))
            out.velocity_rad_s.append(cansimple.turns_to_rad(axis.vel_turns_s))
            # **NaN, not zero.** The wire reports current in amps, and amps become torque
            # through the motor's torque constant, which is configuration and not on the wire
            # — so effort is a quantity this driver does not measure. §24's rule, and a zero
            # would read as "this joint is unloaded" every cycle.
            out.effort_nm.append(math.nan)
            if axis.axis_error:
                faulted = True
                if out.fault_code == 0:
                    # The maker's word, verbatim (§24): the black box is where a person looks
                    # the code up, so a node number or a count in its place would be a fault
                    # whose cause was thrown away. One word fits one axis; the first axis with
                    # something to say is the one carried.
                    out.fault_code = axis.axis_error & 0xFFFFFFFF
            elif axis.axis_state != cansimple.STATE_CLOSED_LOOP_CONTROL:
                unwell = True
            elif axis.last_heartbeat is not None and now - axis.last_heartbeat > _HEARTBEAT_STALE_S:
                unwell = True
        if faulted:
            out.flags |= rhal.FLAG_DEGRADED | rhal.FLAG_FAULT
        elif unwell:
            out.flags |= rhal.FLAG_DEGRADED
        if 0 < seen < len(self.axes):
            # Some axes answered this cycle and some did not, so some of the readings above
            # are a period or more old under a fresh stamp. Reported rather than smoothed
            # over: the body is running, and something on its bus is wrong.
            out.flags |= rhal.FLAG_DEGRADED
        if seen == 0:
            # Every axis silent for a whole cycle. The sequence still advances — the OS's
            # watchdog is watching the *stamp* and this read did happen — but a body that
            # answered nothing is a bus problem and the caller is told so rather than handed
            # the previous readings.
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        if cmd.joint_count != len(self.axes):
            return rhal.SKR_EINVAL
        try:
            modes = [cmd.mode_of(index) for index in range(len(self.axes))]
        except ValueError:
            return rhal.SKR_EINVAL
        if self._estop:
            return rhal.SKR_EPERM
        assert self._bus is not None
        for axis in self.axes:
            if axis.axis_error:
                # An axis that holds an error drops every setpoint, and `SKR_OK` for a command
                # it drops is success the robot did not have. Refused whole: the other axes'
                # targets would move part of a body that has lost one of its joints. It is the
                # person's clear at the drive that brings it back, never a frame of this
                # driver's — which sends no `CLEAR_ERRORS` on any path.
                return rhal.SKR_EPERM
        unarmed = [axis for axis in self.axes
                   if axis.axis_state != cansimple.STATE_CLOSED_LOOP_CONTROL]
        if unarmed:
            # The re-arm every idle axis needs after its error was cleared at the drive:
            # without it an axis sits in `IDLE`, ignoring setpoints, until somebody asks.
            # Asked again every command, the way the CANopen driver re-sends its enable, and
            # to every axis that needs it — serialising the asks over one cycle per axis
            # would make a two-axis recovery take two cycles for no reason. This cycle is
            # still refused, because no axis has reported the state yet and setpoints sent on
            # trust would be the same false success as above.
            for axis in unarmed:
                self._bus.send(Frame(
                    cansimple.msg_id(axis.node, cansimple.CMD_SET_REQUESTED_STATE),
                    cansimple.set_requested_state(cansimple.STATE_CLOSED_LOOP_CONTROL),
                ))
            return rhal.SKR_EPERM

        # Every setpoint is converted and checked before any frame goes out, and nothing is
        # stored until all of them pass. Deciding axis by axis while sending would refuse a bad
        # second axis after the first had been commanded — and storing the mode before its frame
        # goes out would be worse: a later refusal would leave the driver believing an axis it
        # never told, and the next command would send a setpoint the axis reads in the wrong
        # mode.
        frames: list[tuple[Frame, Axis | None, int | None]] = []
        for index, axis in enumerate(self.axes):
            control = _CONTROL_OF.get(modes[index])
            if control is None:
                return rhal.SKR_EINVAL
            applied = cmd.mask[index] if index < len(cmd.mask) else 1
            if not applied:
                continue
            value = cmd.values[index]
            try:
                setpoint: bytes | None = None
                if control == cansimple.CONTROL_MODE_POSITION:
                    setpoint = cansimple.set_input_pos(cansimple.rad_to_turns(value))
                elif control == cansimple.CONTROL_MODE_VELOCITY:
                    setpoint = cansimple.set_input_vel(cansimple.rad_to_turns(value))
                elif control == cansimple.CONTROL_MODE_TORQUE:
                    if not math.isfinite(value):
                        return rhal.SKR_EINVAL
                    setpoint = cansimple.set_input_torque(value)
            except (ValueError, struct.error):
                return rhal.SKR_EINVAL
            assert setpoint is not None
            if axis.control_mode != control:
                frames.append((
                    Frame(
                        cansimple.msg_id(axis.node, cansimple.CMD_SET_CONTROLLER_MODES),
                        cansimple.set_controller_modes(control, cansimple.INPUT_MODE_PASSTHROUGH),
                    ),
                    axis,
                    control,
                ))
            frames.append((Frame(
                cansimple.msg_id(axis.node, _CMD_OF[control]), setpoint,
            ), None, None))
        for frame, axis, control in frames:
            self._bus.send(frame)
            if axis is not None:
                assert control is not None
                axis.control_mode = control
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        self._estop = True
        if self._bus is None:
            return rhal.SKR_OK
        for axis in self.axes:
            self._bus.send(Frame(
                cansimple.msg_id(axis.node, cansimple.CMD_ESTOP), cansimple.estop(),
            ))
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """No software release: ENOSYS, honestly (the Spot precedent).

        The stop set ``AXIS_ERROR_ESTOP_REQUESTED`` in every axis, and returning from that
        needs the person's clear at the drive plus a new session — a latch this method
        cleared would report a body that still refuses every command, the observable half
        returned and the commandable half not. The OS treats ENOSYS as "no software
        release" and renews the session instead (§4), which is exactly what a body whose
        stop latches in firmware is for.
        """
        return rhal.SKR_ENOSYS

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── CANSimple ──────────────────────────────────────────────────────────

    def _refuse_if_absent(self, axis: Axis) -> None:
        """Ask the axis for its estimates, and refuse a node that never answers."""
        assert self._bus is not None
        ident, payload = cansimple.request(axis.node, cansimple.CMD_GET_ENCODER_ESTIMATES)
        self._bus.send(Frame(ident, payload))
        deadline = self._clock() + _REQUEST_TIMEOUT_S
        while True:
            frame = self._bus.recv(0.01)
            if frame is None:
                if self._bus.synchronous or self._clock() >= deadline:
                    break
                continue
            if frame.arbitration_id == cansimple.msg_id(axis.node, cansimple.CMD_GET_ENCODER_ESTIMATES):
                try:
                    axis.pos_turns, axis.vel_turns_s = cansimple.parse_estimates(frame.data)
                except ValueError:
                    self.unexpected_frames += 1
                    continue
                return
            self._apply(frame)
        raise BridgeRefusal(
            f"node {axis.node} did not answer an estimates request within "
            f"{_REQUEST_TIMEOUT_S:.1f}s. A node that is not on the bus and a node that is "
            "powered off look the same from here; `candump` is what tells them apart."
        )

    def _await_heartbeat(self, axis: Axis) -> None:
        """Wait for one heartbeat: the proof the axis volunteers its health."""
        assert self._bus is not None
        deadline = self._clock() + _HEARTBEAT_TIMEOUT_S
        while axis.axis_state is None:
            frame = self._bus.recv(0.01)
            if frame is None:
                if self._bus.synchronous or self._clock() >= deadline:
                    break
                continue
            self._apply(frame)
        if axis.axis_state is None:
            raise BridgeRefusal(
                f"node {axis.node} answers requests but sends no heartbeat. This driver reads "
                "the axis error and state from the heartbeat alone — there is no request for "
                "them — so an axis with its cyclic heartbeat switched off is an axis whose "
                "faults would sit unread while the driver reported positions. Enable "
                "`can.heartbeat_rate_ms` on the axis, or command it some other way."
            )

    def _refuse_if_faulted(self, axis: Axis) -> None:
        """Read the latched error before touching anything, and refuse an axis that holds one."""
        if axis.axis_error:
            raise BridgeRefusal(
                f"node {axis.node} holds axis error {axis.axis_error:#010x} "
                f"({axis.state_name()}). This body does not come up over a latched fault. Read "
                "the motor, encoder and controller words for why, and clear it at the drive "
                "deliberately — a driver that cleared it here would restart a machine whose "
                "reason for stopping nobody had looked at, and would do it again on the next "
                "power cycle."
            )

    def _set_modes(self, axis: Axis, control: int) -> None:
        assert self._bus is not None
        self._bus.send(Frame(
            cansimple.msg_id(axis.node, cansimple.CMD_SET_CONTROLLER_MODES),
            cansimple.set_controller_modes(control, cansimple.INPUT_MODE_PASSTHROUGH),
        ))
        axis.control_mode = control

    def _request_closed_loop(self, axis: Axis) -> None:
        """Ask for closed loop and wait until the axis reports it — read back, never assumed."""
        assert self._bus is not None
        self._bus.send(Frame(
            cansimple.msg_id(axis.node, cansimple.CMD_SET_REQUESTED_STATE),
            cansimple.set_requested_state(cansimple.STATE_CLOSED_LOOP_CONTROL),
        ))
        deadline = self._clock() + _STATE_TIMEOUT_S
        while axis.axis_state != cansimple.STATE_CLOSED_LOOP_CONTROL:
            if axis.axis_error:
                break
            frame = self._bus.recv(_STATE_POLL_S)
            if frame is None:
                if self._bus.synchronous or self._clock() >= deadline:
                    break
                continue
            self._apply(frame)
        if axis.axis_state != cansimple.STATE_CLOSED_LOOP_CONTROL:
            if axis.axis_error:
                advice = (
                    "A faulted axis does not arm; read the motor, encoder and controller "
                    "words for why and clear it at the drive."
                )
            else:
                advice = (
                    "An axis that stays out of closed loop is still initialising, has an "
                    "interlock no requested state overrides, or never heard the request. The "
                    "state is read back rather than assumed, because a master that trusted its "
                    "own request would write setpoints into a disabled axis and watch the "
                    "shaft stand still."
                )
            raise BridgeRefusal(
                f"node {axis.node} was asked for closed loop and reports "
                f"`{axis.state_name()}`. {advice}"
            )

    def _drain(self) -> None:
        """Apply everything already waiting, without waiting for more."""
        assert self._bus is not None
        while True:
            frame = self._bus.recv()
            if frame is None:
                return
            self._apply(frame)

    def _collect_estimates(self) -> int:
        """How many axes answered this cycle's requests, waiting up to the window for the rest.

        Right on the loopback bus — the axes answer inside `recv` — and right on a kernel bus
        only because of the deadline: the answers arrive microseconds after the requests, and
        reading whatever is waiting would report the *previous* cycle's shafts under this
        read's stamp (the CANopen driver's F71, avoided here by writing the wait first).
        """
        assert self._bus is not None
        waiting = {axis.node for axis in self.axes}
        deadline = self._clock() + self._window_s
        while waiting:
            remaining = deadline - self._clock()
            frame = self._bus.recv(max(remaining, 0.0))
            if frame is None:
                if self._bus.synchronous or remaining <= 0.0:
                    break
                continue
            self._apply_cycle(frame, waiting)
        return len(self.axes) - len(waiting)

    def _apply_cycle(self, frame: Frame, waiting: set[int]) -> None:
        """Fold one frame of a cycle into its axis. An estimates reply moves the shaft and
        retires the axis from the wait — but only when it parses: a short reply is counted,
        not applied, and the axis stays waited for. Parsed once, so a short reply is counted
        once and not once per reader."""
        for axis in self.axes:
            if frame.arbitration_id == cansimple.msg_id(axis.node, cansimple.CMD_GET_ENCODER_ESTIMATES):
                try:
                    axis.pos_turns, axis.vel_turns_s = cansimple.parse_estimates(frame.data)
                except ValueError:
                    self.unexpected_frames += 1
                    return
                waiting.discard(axis.node)
                return
        self._apply(frame)

    def _apply(self, frame: Frame) -> None:
        """Fold one frame into the axis it speaks for. Heartbeats move state; estimates move
        the shaft; anything else on a known identifier is counted, not parsed."""
        for axis in self.axes:
            ident = frame.arbitration_id
            if ident == cansimple.msg_id(axis.node, cansimple.CMD_HEARTBEAT):
                try:
                    error, state, _, _, _ = cansimple.parse_heartbeat(frame.data)
                except ValueError:
                    self.unexpected_frames += 1
                    return
                axis.axis_error = error
                axis.axis_state = state
                axis.last_heartbeat = self._clock()
                return
            if ident == cansimple.msg_id(axis.node, cansimple.CMD_GET_ENCODER_ESTIMATES):
                try:
                    axis.pos_turns, axis.vel_turns_s = cansimple.parse_estimates(frame.data)
                except ValueError:
                    self.unexpected_frames += 1
                return
        self.unexpected_frames += 1

    # ── the descriptor ─────────────────────────────────────────────────────

    def _build_descriptor(self) -> str:
        joints = []
        for axis in self.axes:
            joints.append(
                {
                    "name": axis.name,
                    "role": axis.role,
                    # Continuous, with no limits: the wire publishes no travel and no rating
                    # for the motor on the axis (see the module docstring), and a joint without
                    # travel limits is described as continuous, not revolute.
                    "type": "continuous",
                    "limits": {},
                    "control_modes": ["position", "velocity", "torque"],
                }
            )
        descriptor = {
            "schema_version": 1,
            "robot_slug": "odrive-cansimple-axes",
            "digest": None,
            "verified": False,
            # A simulated bus is a simulated body. On a real bus with real axes this is
            # `False`, and the driver has no way to tell — which is why it is configuration
            # rather than a guess: `{"simulator": true}` is what the harness sets, and a real
            # integration leaves it out.
            "simulator": self._simulated,
            "base": "fixed",
            "dof": len(self.axes),
            "joints": joints,
            "end_effectors": [],
            "sensors": [],
            "control": {
                "modes": ["position", "velocity", "torque"],
                "per_joint_modes": True,
                "rate_hz_max": 1000,
                "rate_hz_min": 1,
            },
            # Null, not an empty object: nobody has declared a safety envelope for this body,
            # and the drives' own limits are configuration rather than a risk assessment's.
            "envelope": None,
            # `emergency_stop` sends every axis its stop frame and latches, and refuses later
            # commands — the software stop the driver actually has. The rated stop stays
            # undeclared: a rated stop is an STO input wired to a relay, and it is not
            # reachable from a CAN frame.
            "capabilities": ["safety.estop"],
            "extensions": [],
            "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))


#: RHAL body modes to the control mode each commands.
_CONTROL_OF = {
    rhal.MODE_POSITION: cansimple.CONTROL_MODE_POSITION,
    rhal.MODE_VELOCITY: cansimple.CONTROL_MODE_VELOCITY,
    rhal.MODE_TORQUE: cansimple.CONTROL_MODE_TORQUE,
}

#: Control modes to the setpoint command each is commanded with.
_CMD_OF = {
    cansimple.CONTROL_MODE_POSITION: cansimple.CMD_SET_INPUT_POS,
    cansimple.CONTROL_MODE_VELOCITY: cansimple.CMD_SET_INPUT_VEL,
    cansimple.CONTROL_MODE_TORQUE: cansimple.CMD_SET_INPUT_TORQUE,
}


def _say(message: str) -> None:
    import sys

    print(f"skynet-odrive: {message}", file=sys.stderr, flush=True)

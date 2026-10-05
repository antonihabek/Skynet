"""An ODrive axis, simulated — the dry substrate this driver is measured against.

One instance is one axis: it answers ``GET_*`` requests, runs ``IDLE`` and
``CLOSED_LOOP_CONTROL``, integrates its shaft toward the input in position and velocity mode,
and sends a heartbeat behind every frame it handles. ``layout_check.py`` holds its frame ids
and payload layouts to the maker's own DBC; what follows is everything else it is and is not.

# What makes it a substrate rather than a mirror

* **It refuses.** A setpoint written while the axis is not in closed loop is dropped and
  counted; a state it cannot run sets ``INVALID_STATE``; a command id the firmware does not
  define is ignored and counted. Those are the paths a real axis takes and the ones a mirror
  would never exercise.
* **It can be made to fault**, which is the state no amount of correct driver code can avoid
  and the one an integrator meets first: stage an error word and the next heartbeat carries
  it, the next closed-loop request is refused, and only an explicit clear — the person's
  act, never the driver's — brings the axis back.
* **It can be made silent**, in two halves: no answers at all (a bus problem), or answers
  but no heartbeat (an axis whose cyclic health the integrator switched off, which this
  driver refuses to command).

# What it is not

An axis. It has no current loop, no thermal model, no calibration sequence and no bus. It
moves its shaft toward the input by a fixed fraction per estimates request, which is an
interpolator no real servo has, and its velocity is that motion divided by a virtual ten
milliseconds — the firmware's default estimates rate — because a loopback bus has no clock.
In torque mode the shaft does not move at all: integrating torque needs an inertia the wire
does not carry, and an invented one would be physics the tests could pass against and a
robot would not have. Nothing it reports is evidence about a motor, and ``describe()`` on
the bridge above it declares ``simulator: true`` so that §35.3 caps the tier whatever the
numbers say.
"""

from __future__ import annotations

import struct

from skynet_canopen.bus import Bus
from skynet_canopen.frames import Frame

from . import cansimple

__all__ = ["SimulatedAxis"]

#: ``AXIS_ERROR_INVALID_STATE`` and ``AXIS_ERROR_ESTOP_REQUESTED`` (the DBC's ``Axis_Error``
#: value table): the two bits the sim sets itself. Every other bit is staged by the test.
_INVALID_STATE = 0x00000001
_ESTOP_REQUESTED = 0x00004000


class SimulatedAxis:
    """One ODrive axis on the bus, at ``node``."""

    def __init__(
        self,
        bus: Bus,
        node: int,
        *,
        heartbeat: bool = True,
        silent: bool = False,
        cpr: int = 8192,
        virtual_dt_s: float = 0.01,
    ) -> None:
        self.bus = bus
        self.node = node
        #: A heartbeat behind every handled frame is the cyclic health the firmware sends at
        #: 100 ms by default. Switched off, the axis answers requests and never volunteers its
        #: state — the body the driver refuses to command.
        self.heartbeat_enabled = heartbeat
        #: Answers nothing at all: a powered-off axis, or one that is not on the bus.
        self.silent = silent
        #: Encoder counts per revolution, for the count reply. The driver's motion never reads
        #: counts — estimates carry turns — so this is shape, not physics.
        self.cpr = cpr
        #: The sim's virtual timebase: one estimates request is this many seconds of shaft motion.
        self.virtual_dt_s = virtual_dt_s
        self.state = cansimple.STATE_IDLE
        self.axis_error = 0
        self.motor_error = 0
        self.encoder_error = 0
        self.controller_error = 0
        self.sensorless_error = 0
        self.control_mode = cansimple.CONTROL_MODE_POSITION
        self.input_mode = cansimple.INPUT_MODE_PASSTHROUGH
        self.pos_turns = 0.0
        self.vel_turns_s = 0.0
        self.input_pos_turns = 0.0
        self.input_vel_turns_s = 0.0
        self.input_torque_nm = 0.0
        #: What the driver sent that went nowhere, by cause. A counter that never moves in the
        #: suite is a refusal path nothing exercised.
        self.dropped_setpoints = 0
        self.refused_states = 0
        self.ignored_commands = 0

    # ── the bus ────────────────────────────────────────────────────────────

    def poll(self) -> None:
        """Answer what is waiting, with a heartbeat behind every handled frame.

        The firmware volunteers its heartbeat on time; a loopback bus has no time, so here the
        heartbeat rides on traffic instead — and only on traffic. A heartbeat per poll whether
        or not anything arrived would livelock every drain: each empty look would mint the
        next frame, and a reader draining the bus would never see it empty.
        """
        handled = 0
        while True:
            frame = self.bus.recv()
            if frame is None:
                break
            if self._handle(frame):
                handled += 1
        if handled and self.heartbeat_enabled:
            self._send(cansimple.CMD_HEARTBEAT, self._heartbeat())

    def _send(self, cmd: int, payload: bytes) -> None:
        self.bus.send(Frame(cansimple.msg_id(self.node, cmd), payload))

    def _handle(self, frame: Frame) -> bool:
        """Handle one frame. True when it was addressed to this axis and not silent."""
        if self.silent or cansimple.node_of(frame.arbitration_id) != self.node:
            return False
        cmd = cansimple.cmd_of(frame.arbitration_id)
        data = frame.data
        if cmd == cansimple.CMD_SET_REQUESTED_STATE:
            self._request_state(struct.unpack_from("<i", data)[0] if len(data) >= 4 else -1)
        elif cmd == cansimple.CMD_SET_CONTROLLER_MODES:
            if len(data) >= 8:
                self.control_mode, self.input_mode = struct.unpack_from("<ii", data)
        elif cmd == cansimple.CMD_SET_INPUT_POS:
            self._input_pos(struct.unpack_from("<f", data)[0] if len(data) >= 4 else 0.0)
        elif cmd == cansimple.CMD_SET_INPUT_VEL:
            self._input_vel(struct.unpack_from("<f", data)[0] if len(data) >= 4 else 0.0)
        elif cmd == cansimple.CMD_SET_INPUT_TORQUE:
            self._input_torque(struct.unpack_from("<f", data)[0] if len(data) >= 4 else 0.0)
        elif cmd == cansimple.CMD_ESTOP:
            self.axis_error |= _ESTOP_REQUESTED
            self.state = cansimple.STATE_IDLE
        elif cmd == cansimple.CMD_CLEAR_ERRORS:
            self._clear_errors()
        elif cmd == cansimple.CMD_GET_ENCODER_ESTIMATES and len(data) == 0:
            self._integrate()
            self._send(cmd, struct.pack("<ff", self.pos_turns, self.vel_turns_s))
        elif cmd == cansimple.CMD_GET_ENCODER_COUNT and len(data) == 0:
            shadow = int(self.pos_turns * self.cpr)
            self._send(cmd, struct.pack("<ii", shadow, shadow % self.cpr))
        elif cmd == cansimple.CMD_GET_MOTOR_ERROR and len(data) == 0:
            self._send(cmd, struct.pack("<Q", self.motor_error))
        elif cmd == cansimple.CMD_GET_ENCODER_ERROR and len(data) == 0:
            self._send(cmd, struct.pack("<I", self.encoder_error) + bytes(4))
        elif cmd == cansimple.CMD_GET_CONTROLLER_ERROR and len(data) == 0:
            self._send(cmd, struct.pack("<I", self.controller_error) + bytes(4))
        elif cmd == cansimple.CMD_GET_SENSORLESS_ERROR and len(data) == 0:
            self._send(cmd, struct.pack("<I", self.sensorless_error) + bytes(4))
        elif cmd == cansimple.CMD_GET_IQ and len(data) == 0:
            self._send(cmd, struct.pack("<ff", 0.0, 0.0))
        elif cmd == cansimple.CMD_GET_BUS_VOLTAGE_CURRENT and len(data) == 0:
            self._send(cmd, struct.pack("<ff", 24.0, 0.0))
        elif cmd == cansimple.CMD_GET_ADC_VOLTAGE:
            self._send(cmd, struct.pack("<f", 0.0) + bytes(4))
        elif cmd in (
            cansimple.CMD_GET_SENSORLESS_ESTIMATES,
            cansimple.CMD_SET_NODE_ID,
            cansimple.CMD_SET_LIMITS,
            cansimple.CMD_SET_TRAJ_VEL_LIMIT,
            cansimple.CMD_SET_TRAJ_ACCEL_LIMITS,
            cansimple.CMD_SET_TRAJ_INERTIA,
            cansimple.CMD_SET_LINEAR_COUNT,
            cansimple.CMD_SET_POS_GAIN,
            cansimple.CMD_SET_VEL_GAINS,
            cansimple.CMD_START_ANTICOGGING,
            cansimple.CMD_RESET,
            cansimple.CMD_NMT,
            cansimple.CMD_HEARTBEAT,
            cansimple.CMD_SET_STARTUP_CONFIG,
        ):
            # Defined by the firmware and never sent by this driver: heard and left alone.
            self.ignored_commands += 1
        else:
            self.ignored_commands += 1
        return True

    # ── the behaviour ──────────────────────────────────────────────────────

    def _request_state(self, state: int) -> None:
        if state == cansimple.STATE_IDLE:
            self.state = cansimple.STATE_IDLE
        elif state == cansimple.STATE_CLOSED_LOOP_CONTROL:
            if self.axis_error:
                # A faulted axis does not arm. The firmware's own refusal is louder than
                # silence; the count is what the suite reads.
                self.refused_states += 1
            else:
                self.state = cansimple.STATE_CLOSED_LOOP_CONTROL
        else:
            # No calibration sequence runs here, and no lock-in, homing or dir-find: asking
            # for one is a state this axis cannot reach, said the way the firmware says it.
            self.axis_error |= _INVALID_STATE
            self.refused_states += 1

    def _input_pos(self, pos_turns: float) -> None:
        if self.state != cansimple.STATE_CLOSED_LOOP_CONTROL:
            self.dropped_setpoints += 1
            return
        self.input_pos_turns = pos_turns

    def _input_vel(self, vel_turns_s: float) -> None:
        if self.state != cansimple.STATE_CLOSED_LOOP_CONTROL:
            self.dropped_setpoints += 1
            return
        self.input_vel_turns_s = vel_turns_s

    def _input_torque(self, torque_nm: float) -> None:
        if self.state != cansimple.STATE_CLOSED_LOOP_CONTROL:
            self.dropped_setpoints += 1
            return
        self.input_torque_nm = torque_nm

    def _clear_errors(self) -> None:
        # The firmware's ``clear_errors`` is board-wide (``can_simple.cpp``'s TODO says it
        # might want to clear axis errors only); with one axis per instance the two are one.
        self.axis_error = 0
        self.motor_error = 0
        self.encoder_error = 0
        self.controller_error = 0
        self.sensorless_error = 0

    def _integrate(self) -> None:
        before = self.pos_turns
        if self.state == cansimple.STATE_CLOSED_LOOP_CONTROL:
            if self.control_mode == cansimple.CONTROL_MODE_POSITION:
                self.pos_turns += (self.input_pos_turns - self.pos_turns) * 0.25
            elif self.control_mode == cansimple.CONTROL_MODE_VELOCITY:
                self.pos_turns += self.input_vel_turns_s * self.virtual_dt_s
            # Torque mode holds: integrating it needs an inertia the wire does not carry
            # (see the module docstring), so the shaft stays where it is.
        self.vel_turns_s = (self.pos_turns - before) / self.virtual_dt_s

    def _heartbeat(self) -> bytes:
        return struct.pack(
            "<IBBBB",
            self.axis_error & 0xFFFFFFFF,
            self.state & 0xFF,
            1 if self.motor_error else 0,
            1 if self.encoder_error else 0,
            (1 if self.controller_error else 0),
        )

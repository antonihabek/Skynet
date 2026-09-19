"""The RHAL driver: a segment of CiA 402 drives over EtherCAT.

§24's five entry points onto :mod:`skynet_ethercat.master`. Almost everything this driver refuses,
it refuses for a reason `drivers/canopen` established first — the two speak the same profile over
different transports, and the profile is where the traps are.

## What it reads off the drives rather than out of a file

| Object | For |
|---|---|
| `0x608F:1`, `0x608F:2` | encoder resolution — increments per motor revolution |
| `0x6091:1`, `0x6091:2` | gear ratio — motor revolutions per shaft revolution |
| `0x607D:1`, `0x607D:2` | software position limits → the descriptor's travel |

The first two are what the driver **refuses a body over**. A position in increments means nothing
without them, and a driver that assumed 65536 counts per revolution and a direct drive would report
a joint angle wrong by the gearbox — a factor of fifty on an industrial axis, in radians, to a
Safety Kernel that bounds travel with it. §5.2's rule is that a limit with no provenance is a
rumour; a unit with no provenance is worse, because it is silently applied to every number the body
reports.

## What it will not read

A torque rating. `0x6076` is the *motor's* rated torque — a design value from a datasheet — and
§63.4 is explicit that a design value is not a safety rating. `envelope` is null and no joint
declares `effort_max_nm`, exactly as in `drivers/canopen`, `drivers/franka` and `skynet-urdf`.

## And one thing that is this transport's alone

Every cycle is **one** `LRW` datagram carrying every drive's process data, and the master asserts its
working counter is three per slave. A drive that dropped off the segment mid-run does not raise
anything: the frame comes back the right length with one fewer increment, and `read_state` answers
`SKR_EIO` because of that number and nothing else.
"""

from __future__ import annotations

import json
import math
import os
import struct
import sys

import skynet_rhal as rhal
from skynet_rhal import Command, Driver, Frame, State

from skynet_canopen import cia402

from .master import EtherCatMaster, MasterError
from .segment import LoopbackSegment, RawSocketSegment, SegmentError
from .slave import EtherCatDrive


class BridgeRefusal(Exception):
    """A body this driver will not present as a robot."""


class EtherCatBridge(Driver):
    """One EtherCAT segment as one §24 body."""

    def __init__(self) -> None:
        self._master: EtherCatMaster | None = None
        self._segment = None
        self._drives: list[dict] = []
        self._opened = False
        self._estopped = False
        self._seq = 0
        self._positions: list[float] = []
        self._descriptor: str | None = None
        self._slug = "ethercat-segment"
        self._outputs = bytearray()
        #: The last process-data image the segment returned. `write_command` walks each drive's
        #: CiA 402 state machine from what the drive *reported*, never from what this driver
        #: predicted — which is the rule `drivers/canopen` states and the reason the two agree.
        self._last_image = b""

    # ── open / close ─────────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        # A fresh session. The C++ wrapper's singleton problem has a Python twin: `serve()` holds
        # one driver for the process, so a second `open()` lands on the object the first one left.
        self._estopped = False
        self._seq = 0
        try:
            settings = json.loads(config) if config.strip() else {}
        except ValueError:
            return rhal.SKR_EINVAL

        joints = settings.get("joints")
        if not isinstance(joints, list) or not joints:
            print(
                "skynet-ethercat: this driver needs a `joints` list in its config, naming each "
                "drive in segment order with the role it carries. Auto-increment addressing is "
                "positional and §5.1's roles are never inferred from a position — `shoulder` is a "
                "pitch on one robot and a yaw on the next.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EINVAL
        self._slug = settings.get("robot_slug", self._slug)

        try:
            self._segment = self._make_segment(settings, len(joints))
        except SegmentError as error:
            print(f"skynet-ethercat: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO

        self._master = EtherCatMaster(self._segment)
        try:
            self._master.bring_up(len(joints))
        except MasterError as error:
            print(f"skynet-ethercat: the segment did not come up: {error}",
                  file=sys.stderr, flush=True)
            return rhal.SKR_EIO

        try:
            self._drives = [self._read_drive(index, joints[index]) for index in range(len(joints))]
            self._descriptor = self._build_descriptor()
        except (MasterError, BridgeRefusal) as error:
            print(f"skynet-ethercat: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_EPROTO

        self._outputs = bytearray(self._master.logical_length)
        self._positions = [0.0] * len(self._drives)
        self._last_image = b""
        self._opened = True
        return rhal.SKR_OK

    def _make_segment(self, settings: dict, expected: int):
        interface = settings.get("interface") or os.environ.get("SKYNET_ETHERCAT_INTERFACE")
        if interface:
            # The real thing. It refuses to construct where there is nothing to construct against,
            # and there is deliberately no fallback: a master that quietly ran against a loopback
            # segment would report a ring that is not there.
            return RawSocketSegment(interface)
        simulated = int(settings.get("sim", os.environ.get("SKYNET_ETHERCAT_SIM", 0) or 0))
        if simulated <= 0:
            raise SegmentError(
                "no `interface` and no `sim` count. This driver does not guess: name a NIC with "
                "an EtherCAT segment on it, or say how many simulated drives to bring up."
            )
        if simulated != expected:
            raise SegmentError(
                f"`sim` asks for {simulated} drive(s) and `joints` describes {expected}. "
                f"Auto-increment addressing is positional, so the two have to be the same number "
                f"or every address after the first mismatch names a different machine."
            )
        drives = [
            EtherCatDrive(position, name=f"drive{position}") for position in range(simulated)
        ]
        return LoopbackSegment(drives)

    def _read_drive(self, position: int, declared: dict) -> dict:
        """Everything about one drive that the drive itself has to tell us."""
        master = self._master
        try:
            increments = master.sdo_upload(position, 0x608F, 1, 4, False)
            revolutions = master.sdo_upload(position, 0x608F, 2, 4, False)
            gear_motor = master.sdo_upload(position, 0x6091, 1, 4, False)
            gear_shaft = master.sdo_upload(position, 0x6091, 2, 4, False)
        except MasterError as error:
            raise BridgeRefusal(
                f"drive {position} would not report its encoder resolution or its gear ratio "
                f"({error}). A position in increments means nothing without both: a driver that "
                f"assumed a direct drive would report the motor's angle as the joint's, wrong by "
                f"the gearbox, in radians, to a Safety Kernel that bounds travel with it."
            ) from error
        if revolutions <= 0 or gear_shaft <= 0:
            raise BridgeRefusal(
                f"drive {position} reports a zero in a ratio denominator "
                f"({increments}/{revolutions}, {gear_motor}/{gear_shaft}). Refusing rather than "
                f"dividing: the result would be a joint angle nobody measured."
            )
        try:
            travel_min = master.sdo_upload(position, 0x607D, 1, 4, True)
            travel_max = master.sdo_upload(position, 0x607D, 2, 4, True)
        except MasterError:
            travel_min = travel_max = None

        return {
            "position": position,
            "name": declared.get("name", f"J{position + 1}"),
            "role": declared.get("role"),
            "increments": increments,
            "revolutions": revolutions,
            "gear_motor": gear_motor,
            "gear_shaft": gear_shaft,
            "travel": (travel_min, travel_max),
        }

    def close(self) -> int:
        if self._opened and self._master is not None:
            try:
                self._quick_stop()
            except MasterError:
                pass
        if self._segment is not None:
            self._segment.close()
        self._opened = False
        self._master = None
        self._segment = None
        return rhal.SKR_OK

    # ── units ────────────────────────────────────────────────────────────────

    def _to_rad(self, drive: dict, increments_value: int) -> float:
        return cia402.increments_to_rad(
            increments_value,
            drive["increments"],
            drive["revolutions"],
            drive["gear_motor"],
            drive["gear_shaft"],
        )

    def _to_increments(self, drive: dict, radians: float) -> int:
        return cia402.rad_to_increments(
            radians,
            drive["increments"],
            drive["revolutions"],
            drive["gear_motor"],
            drive["gear_shaft"],
        )

    # ── §24 ──────────────────────────────────────────────────────────────────

    def read_state(self, out: State, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        try:
            image = self._master.cycle(bytes(self._outputs))
        except MasterError as error:
            print(f"skynet-ethercat: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        self._last_image = image

        positions = []
        flags = 0
        fault_code = 0
        for drive in self._drives:
            statusword, increments = struct.unpack_from(
                "<Hi", image, self._master.input_at[drive["position"]]
            )
            positions.append(self._to_rad(drive, increments))
            state = cia402.state_of(statusword)
            if state in cia402.FAULT_REACTION_STATES:
                flags |= rhal.FLAG_FAULT
                if fault_code == 0:
                    fault_code = drive["position"] + 1
        self._positions = positions
        self._seq += 1

        if self._estopped:
            flags |= rhal.FLAG_ESTOP
        # §27's homing. A CiA 402 drive with an absolute encoder needs none and one with an
        # incremental encoder does, and nothing in the objects this driver reads says which — so
        # the field is `unreported`, which is the state §24 added to be able to say exactly this.
        flags |= rhal.HOMING_UNREPORTED

        out.stamp_ns = rhal.acquisition_ns()
        out.seq = self._seq
        out.flags = flags
        out.fault_code = fault_code
        out.position_rad = positions
        # Velocity and effort are §24's NaN rather than zero: this driver maps neither
        # `0x606C` nor `0x6077` into the cyclic image, so nobody measured them. A zero would read
        # as *this joint is still*, which is the one answer that is both wrong and reassuring.
        out.velocity_rad_s = [math.nan] * len(positions)
        out.effort_nm = [math.nan] * len(positions)
        return rhal.SKR_OK

    def write_command(self, cmd: Command, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        if self._estopped:
            return rhal.SKR_EPERM
        if cmd.mode not in (rhal.MODE_POSITION, rhal.MODE_VELOCITY, rhal.MODE_TORQUE):
            return rhal.SKR_EINVAL
        if len(cmd.values) != len(self._drives) or len(cmd.mask) != len(self._drives):
            return rhal.SKR_EINVAL
        if cmd.mode != rhal.MODE_POSITION:
            # CSP only, and the descriptor says so. Velocity and torque are `0x60FF` and `0x6071`
            # with a different PDO map, and a driver that accepted the mode while sending a
            # position would move the joint to a number in the wrong units.
            return rhal.SKR_EPERM

        for index, drive in enumerate(self._drives):
            at = self._master.output_at[drive["position"]]
            controlword, target = struct.unpack_from("<Hi", self._outputs, at)
            if cmd.mask[index]:
                try:
                    target = self._to_increments(drive, float(cmd.values[index]))
                except (ValueError, OverflowError):
                    return rhal.SKR_EINVAL
            # The CiA 402 walk, one transition per cycle, driven by what the drive reports rather
            # than by what this driver hoped: `next_state` is a prediction and the statusword is
            # the fact. The bridge never commands until the drive says it is enabled.
            controlword = self._advance(index, drive)
            struct.pack_into("<Hi", self._outputs, at, controlword, target)
        return rhal.SKR_OK

    def _advance(self, index: int, drive: dict) -> int:
        """One step of the CiA 402 walk towards `operation enabled`."""
        state = self._state_of(drive)
        if state is cia402.State.OPERATION_ENABLED:
            return cia402.controlword_for(cia402.Command.ENABLE_OPERATION)
        if state is cia402.State.SWITCHED_ON:
            return cia402.controlword_for(cia402.Command.ENABLE_OPERATION)
        if state is cia402.State.READY_TO_SWITCH_ON:
            return cia402.controlword_for(cia402.Command.SWITCH_ON)
        if state is cia402.State.SWITCH_ON_DISABLED:
            return cia402.controlword_for(cia402.Command.SHUTDOWN)
        if state in cia402.FAULT_REACTION_STATES:
            # Not reset here. `drivers/canopen`'s F26: a driver that clears a latched fault on its
            # own erases why the robot stopped, and the reset is an operator's to ask for.
            return 0
        return 0

    def _state_of(self, drive: dict) -> cia402.State:
        image_at = self._master.input_at[drive["position"]]
        try:
            statusword, _ = struct.unpack_from("<Hi", self._last_image, image_at)
        except struct.error:
            # Nothing has been read yet. `switch on disabled` is the state a drive comes up in and
            # is the one that makes the walk start from the beginning — guessing `operation
            # enabled` would have the bridge command a drive it has never heard from.
            return cia402.State.SWITCH_ON_DISABLED
        return cia402.state_of(statusword)

    def emergency_stop(self) -> int:
        """§24's second rule: callable from a signal handler and from a dying process.

        The latch is set first and unconditionally. Then one cycle with every controlword at
        `quick stop` — which is CiA 402's own emergency deceleration, a state the drive enters and
        holds rather than a torque this driver computed.

        What it is **not** is a power cut. An EtherCAT segment's safety stop is FSoE — Safety over
        EtherCAT, a second protocol on the same wire with its own certified stack — and this driver
        speaks none of it. A driver reporting `SKR_OK` for a controlword it merely wrote would be
        claiming a category of stop it does not have.
        """
        self._estopped = True
        if not self._opened or self._master is None:
            return rhal.SKR_OK
        try:
            self._quick_stop()
        except MasterError:
            return rhal.SKR_EIO
        return rhal.SKR_OK

    def _quick_stop(self) -> None:
        word = cia402.controlword_for(cia402.Command.QUICK_STOP)
        for drive in self._drives:
            at = self._master.output_at[drive["position"]]
            _, target = struct.unpack_from("<Hi", self._outputs, at)
            struct.pack_into("<Hi", self._outputs, at, word, target)
        self._last_image = self._master.cycle(bytes(self._outputs))

    def read_sensor(self, name: str, out: Frame) -> int:
        # A servo segment publishes no images. §24 reports the capability as absent rather than
        # answering an empty frame, and the descriptor declares no sensors to match.
        return rhal.SKR_ENODEV

    # ── the body ─────────────────────────────────────────────────────────────

    def describe(self) -> str:
        if self._descriptor is None:
            raise RuntimeError("describe() before open()")
        return self._descriptor

    def _build_descriptor(self) -> str:
        joints = []
        for drive in self._drives:
            limits: dict = {}
            low, high = drive["travel"]
            if low is not None and high is not None:
                limits["position_min_rad"] = round(self._to_rad(drive, low), 6)
                limits["position_max_rad"] = round(self._to_rad(drive, high), 6)
            joints.append(
                {
                    "name": drive["name"],
                    "role": drive["role"],
                    "type": "revolute",
                    # Travel only. `0x6076` is the motor's rated torque — a design value — and
                    # §63.4 keeps a design value out of a safety envelope.
                    "limits": limits,
                    "control_modes": ["position"],
                }
            )
        return json.dumps(
            {
                "schema_version": 1,
                "robot_slug": self._slug,
                "digest": None,
                "verified": False,
                "simulator": isinstance(self._segment, LoopbackSegment),
                "base": "fixed",
                "dof": len(joints),
                "joints": joints,
                "end_effectors": [],
                "sensors": [],
                "control": {
                    "modes": ["position"],
                    "rate_hz_max": 1000,
                    "rate_hz_min": 50,
                    "command_latency_ms": None,
                },
                "envelope": None,
                # `emergency_stop` writes quick-stop to every drive, latches, and refuses later
                # commands — the software stop the driver actually has, in the tradition
                # `drivers/ur_rtde` sets. FSoE is the stop it does not have, and the envelope
                # carries no rated stop to match.
                "capabilities": ["safety.estop"],
                "solvers": [],
            }
        )

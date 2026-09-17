"""A CiA 402 drive, simulated — the dry substrate this driver is measured against.

`DRY-COVERAGE-PLAN.md`'s Phase 2 table asks for a *"CiA402 slave sim"* beside `vcan`, and this is
it. One instance is one axis: it holds an object dictionary, answers expedited SDO, follows the
state machine in :mod:`skynet_canopen.cia402`, and publishes TPDO2 on every SYNC.

# What makes it a substrate rather than a mirror

The temptation with a simulator on both sides of a protocol is to make it agree with the driver by
construction, at which point the pair proves nothing. Three things are arranged against that:

* **It is written from the profile, not from the bridge.** The state machine it runs is
  `cia402.next_state`, which is also what the bridge *predicts* — and the bridge never assumes its
  prediction: it reads the statusword back and refuses to command until the drive reports
  `operation enabled` itself.
* **It refuses.** A target written while the drive is not enabled is dropped and counted; an unknown
  object aborts with CiA 301's own code; a read-only object aborts rather than accepting. Those are
  the paths a real drive takes and the ones a mirror would never exercise.
* **It can be made to fault**, which is the state no amount of correct driver code can avoid and the
  one an integrator meets first.

# What it is not

A drive. It has no current loop, no thermal model, no following error and no bus. It moves its
position toward the target by a fixed fraction each SYNC, which is an interpolator that no real
servo has. Nothing it reports is evidence about a motor, and `describe()` on the bridge above it
declares `simulator: true` so that §35.3 caps the tier whatever the numbers say.
"""

from __future__ import annotations

from .bus import Bus
from .cia402 import OD, Command, Mode, State, command_of, next_state, state_of, statusword_for
from .frames import (
    HEARTBEAT_BOOT,
    HEARTBEAT_OPERATIONAL,
    HEARTBEAT_PRE_OPERATIONAL,
    HEARTBEAT_STOPPED,
    NMT_ID,
    NMT_PRE_OPERATIONAL,
    NMT_RESET_NODE,
    NMT_START,
    NMT_STOP,
    SYNC_ID,
    ABORT_LENGTH,
    ABORT_NO_SUCH_OBJECT,
    ABORT_NO_SUCH_SUBINDEX,
    ABORT_READ_ONLY,
    Frame,
    heartbeat_id,
    parse_rpdo2,
    parse_sdo_request,
    rpdo2_id,
    sdo_abort_response,
    sdo_download_response,
    sdo_upload_response,
    tpdo2,
)

__all__ = ["SimulatedDrive"]

#: What a PDO mapping entry looks like: index, subindex, bit length, packed as CiA 301 packs it.
def _mapping(index: int, subindex: int, bits: int) -> int:
    return (index << 16) | (subindex << 8) | bits


class SimulatedDrive:
    """One axis on the bus, at ``node``."""

    def __init__(
        self,
        bus: Bus,
        node: int,
        *,
        increments: int = 1 << 17,
        revolutions: int = 1,
        gear_motor: int = 50,
        gear_shaft: int = 1,
        rated_torque_mnm: int = 2_000,
        travel_increments: tuple[int, int] = (-(1 << 22), 1 << 22),
        report_units: bool = True,
    ) -> None:
        self.bus = bus
        self.node = node
        self.state = State.SWITCH_ON_DISABLED
        self.mode = Mode.CSP
        self.position = 0
        self.velocity = 0
        self.torque = 0
        self.target = 0
        self.previous_controlword = 0
        self.nmt = "pre-operational"
        #: Targets written while the drive was not in `operation enabled`. A real drive drops these
        #: too; the count is what lets a test show the drop happened rather than infer it.
        self.targets_dropped = 0
        self.faulted = False
        #: A drive that will not report its units, which the profile permits and which the bridge
        #: must refuse a body over. `report_units=False` is how the selftest provokes it.
        self._report_units = report_units
        self._objects: dict[tuple[int, int], tuple[int, int, bool]] = {}
        self._read_only: set[tuple[int, int]] = set()
        self._install_objects(
            increments, revolutions, gear_motor, gear_shaft, rated_torque_mnm, travel_increments
        )
        self.bus.send(Frame(heartbeat_id(node), bytes([HEARTBEAT_BOOT])))

    # ── the object dictionary ────────────────────────────────────────────────

    def _install_objects(
        self,
        increments: int,
        revolutions: int,
        gear_motor: int,
        gear_shaft: int,
        rated_torque_mnm: int,
        travel: tuple[int, int],
    ) -> None:
        def put(obj, value: int, read_only: bool = False) -> None:
            self._objects[(obj.index, obj.subindex)] = (value, obj.size, obj.signed)
            if read_only:
                self._read_only.add((obj.index, obj.subindex))

        put(OD.MOTOR_RATED_TORQUE, rated_torque_mnm, read_only=True)
        put(OD.POSITION_LIMIT_MIN, travel[0])
        put(OD.POSITION_LIMIT_MAX, travel[1])
        put(OD.MAX_MOTOR_SPEED, 3000)
        if self._report_units:
            put(OD.ENCODER_INCREMENTS, increments, read_only=True)
            put(OD.ENCODER_REVOLUTIONS, revolutions, read_only=True)
            put(OD.GEAR_MOTOR_REVOLUTIONS, gear_motor, read_only=True)
            put(OD.GEAR_SHAFT_REVOLUTIONS, gear_shaft, read_only=True)
        # The default mapping this driver asserts. A drive whose mapping differs is a drive the
        # bridge refuses rather than commands — see `frames.py`.
        put(OD.TPDO2_MAPPING_COUNT, 2, read_only=True)
        put(OD.TPDO2_MAPPING_1, _mapping(0x6041, 0, 16), read_only=True)
        put(OD.TPDO2_MAPPING_2, _mapping(0x6064, 0, 32), read_only=True)
        put(OD.RPDO2_MAPPING_COUNT, 2, read_only=True)
        put(OD.RPDO2_MAPPING_1, _mapping(0x6040, 0, 16), read_only=True)
        put(OD.RPDO2_MAPPING_2, _mapping(0x607A, 0, 32), read_only=True)

    def _live(self, index: int, subindex: int) -> int | None:
        """Objects whose value is state rather than storage."""
        if (index, subindex) == (OD.STATUSWORD.index, 0):
            return statusword_for(self.state, target_reached=self.position == self.target)
        if (index, subindex) == (OD.CONTROLWORD.index, 0):
            return self.previous_controlword
        if (index, subindex) == (OD.POSITION_ACTUAL.index, 0):
            return self.position
        if (index, subindex) == (OD.VELOCITY_ACTUAL.index, 0):
            return self.velocity
        if (index, subindex) == (OD.TORQUE_ACTUAL.index, 0):
            return self.torque
        if (index, subindex) == (OD.MODES_DISPLAY.index, 0):
            return self.mode.value
        if (index, subindex) == (OD.MODES_OF_OPERATION.index, 0):
            return self.mode.value
        if (index, subindex) == (OD.TARGET_POSITION.index, 0):
            return self.target
        return None

    # ── the bus ──────────────────────────────────────────────────────────────

    def poll(self) -> int:
        """Handle everything waiting. Returns how many frames were consumed."""
        handled = 0
        while True:
            frame = self.bus.recv()
            if frame is None:
                return handled
            handled += 1
            self._handle(frame)

    def _handle(self, frame: Frame) -> None:
        if frame.arbitration_id == NMT_ID:
            self._nmt(frame)
        elif frame.arbitration_id == SYNC_ID:
            self.step()
        elif frame.arbitration_id == rpdo2_id(self.node):
            self._rpdo2(frame)
        elif frame.arbitration_id == 0x600 + self.node:
            self._sdo(frame)

    def _nmt(self, frame: Frame) -> None:
        if len(frame.data) != 2:
            return
        command, node = frame.data[0], frame.data[1]
        if node not in (0, self.node):
            return
        if command == NMT_START:
            self.nmt = "operational"
            self.bus.send(Frame(heartbeat_id(self.node), bytes([HEARTBEAT_OPERATIONAL])))
        elif command == NMT_STOP:
            self.nmt = "stopped"
            self.bus.send(Frame(heartbeat_id(self.node), bytes([HEARTBEAT_STOPPED])))
        elif command == NMT_PRE_OPERATIONAL:
            self.nmt = "pre-operational"
            self.bus.send(Frame(heartbeat_id(self.node), bytes([HEARTBEAT_PRE_OPERATIONAL])))
        elif command == NMT_RESET_NODE:
            self.state = State.SWITCH_ON_DISABLED
            self.previous_controlword = 0
            self.faulted = False
            self.nmt = "pre-operational"
            self.bus.send(Frame(heartbeat_id(self.node), bytes([HEARTBEAT_BOOT])))

    def _sdo(self, frame: Frame) -> None:
        try:
            kind, index, subindex, value, size = parse_sdo_request(frame.data)
        except ValueError:
            return
        key = (index, subindex)
        live = self._live(index, subindex)
        stored = self._objects.get(key)

        if live is None and stored is None:
            # CiA 301 distinguishes "no such object" from "no such subindex", and an integrator
            # acts on the two differently: the first is the wrong device, the second is the wrong
            # subindex on the right one.
            exists = any(idx == index for idx, _ in self._objects) or any(
                self._live(index, sub) is not None for sub in range(0, 3)
            )
            code = ABORT_NO_SUCH_SUBINDEX if exists else ABORT_NO_SUCH_OBJECT
            self.bus.send(sdo_abort_response(self.node, index, subindex, code))
            return

        if kind == "upload":
            if live is not None:
                width, signed = self._width_of(index, subindex)
                self.bus.send(
                    sdo_upload_response(self.node, index, subindex, self._raw(live, width), width)
                )
            else:
                stored_value, width, _signed = stored
                self.bus.send(
                    sdo_upload_response(
                        self.node, index, subindex, self._raw(stored_value, width), width
                    )
                )
            return

        # download
        if key in self._read_only:
            self.bus.send(sdo_abort_response(self.node, index, subindex, ABORT_READ_ONLY))
            return
        width, signed = self._width_of(index, subindex)
        if size != width:
            self.bus.send(sdo_abort_response(self.node, index, subindex, ABORT_LENGTH))
            return
        self._apply(index, subindex, self._signed(value, width, signed))
        self.bus.send(sdo_download_response(self.node, index, subindex))

    def _width_of(self, index: int, subindex: int) -> tuple[int, bool]:
        for obj in vars(OD).values():
            if getattr(obj, "index", None) == index and getattr(obj, "subindex", None) == subindex:
                return obj.size, obj.signed
        stored = self._objects.get((index, subindex))
        return (stored[1], stored[2]) if stored else (4, True)

    @staticmethod
    def _raw(value: int, width: int) -> int:
        return int(value) & ((1 << (8 * width)) - 1)

    @staticmethod
    def _signed(value: int, width: int, signed: bool) -> int:
        if not signed:
            return value
        bits = 8 * width
        return value - (1 << bits) if value >= (1 << (bits - 1)) else value

    def _apply(self, index: int, subindex: int, value: int) -> None:
        if (index, subindex) == (OD.CONTROLWORD.index, 0):
            self._controlword(value)
        elif (index, subindex) == (OD.MODES_OF_OPERATION.index, 0):
            try:
                self.mode = Mode(value)
            except ValueError:
                pass  # an unsupported mode leaves the drive in the one it has, as a real one does
        elif (index, subindex) in (
            (OD.TARGET_POSITION.index, 0),
            (OD.TARGET_VELOCITY.index, 0),
            (OD.TARGET_TORQUE.index, 0),
        ):
            self._target(value)
        else:
            width, _ = self._width_of(index, subindex)
            self._objects[(index, subindex)] = (value, width, True)

    def _rpdo2(self, frame: Frame) -> None:
        try:
            controlword, target = parse_rpdo2(frame.data)
        except ValueError:
            return
        self._controlword(controlword)
        self._target(target)

    def _controlword(self, controlword: int) -> None:
        command = command_of(controlword, self.previous_controlword)
        self.previous_controlword = controlword
        if command is None:
            return
        before = self.state
        self.state = next_state(self.state, command)
        if before is State.FAULT and self.state is State.SWITCH_ON_DISABLED:
            self.faulted = False

    def _target(self, target: int) -> None:
        # A target is accepted only in `operation enabled`. A real drive does the same and it is the
        # reason the bridge reads the statusword back before it commands anything: a master that
        # wrote setpoints into a disabled drive would see them acknowledged on the bus and see the
        # axis stand still.
        if self.state is not State.OPERATION_ENABLED:
            self.targets_dropped += 1
            return
        self.target = target

    # ── the axis ─────────────────────────────────────────────────────────────

    def step(self) -> None:
        """One SYNC: move toward the target, then publish TPDO2."""
        if self.state is State.OPERATION_ENABLED:
            gap = self.target - self.position
            step = int(gap * 0.25)
            if step == 0 and gap != 0:
                step = 1 if gap > 0 else -1
            self.position += step
            self.velocity = step
            self.torque = max(-1000, min(1000, step))
        elif self.state is State.QUICK_STOP_ACTIVE:
            self.velocity = 0
            self.torque = 0
        else:
            self.velocity = 0
            self.torque = 0
        statusword = statusword_for(self.state, target_reached=self.position == self.target)
        self.bus.send(tpdo2(self.node, statusword, self.position))

    def fault(self, code: int = 0x7305) -> None:
        """Latch a fault, as a drive does when an encoder stops answering.

        The EMCY goes out as well as the state change, because the two carry different things: the
        statusword says *this axis has faulted* and the emergency object says *why*, and a driver
        that only watched the statusword would report a fault with no cause on every axis.
        """
        self.faulted = True
        self.state = State.FAULT
        self.velocity = 0
        self.torque = 0
        self.bus.send(
            Frame(
                0x080 + self.node,
                bytes([code & 0xFF, (code >> 8) & 0xFF, 0x01, 0, 0, 0, 0, 0]),
            )
        )

"""skynet-canopen's own suite: the protocol, the profile, and every refusal the bridge makes.

No test framework and no bus. `drivers/lerobot/selftest.py`'s shape, with one difference in emphasis:
nearly everything asserted here is a **refusal**, and every refusal is written beside the
nearly-identical input that succeeds. A CANopen driver's failure mode is not a crash — it is an axis
that reports a plausible number and moves somewhere else — so a suite that only drove the happy path
would be checking the one thing that cannot go wrong quietly.

    make selftest         # this file
    make check            # and then the whole path through the real ABI
    make check-vcan       # the same driver on a real SocketCAN interface, where there is one
    make check-lely-docker  # and against Lely's CiA 402 slave, which nobody here wrote (10.7)
"""

from __future__ import annotations

import json
import math
import os
import subprocess
import sys
import time

import skynet_rhal as rhal

from skynet_canopen import cia402, frames
from skynet_canopen.bridge import BridgeRefusal, CanOpenBridge
from skynet_canopen.bus import Bus, LoopbackBus, SocketCanBus, can_available
from skynet_canopen.slave import SimulatedDrive

FAILURES: list[str] = []
CHECKS = 0


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")


def section(title: str) -> None:
    print(f"\n{title}")


def build(count: int = 3, **drive_args) -> tuple[CanOpenBridge, list[SimulatedDrive], LoopbackBus]:
    bus = LoopbackBus()
    master = bus.endpoint()
    drives = []
    for node in range(1, count + 1):
        drive = SimulatedDrive(bus.endpoint(), node, **drive_args)
        drives.append(drive)
        bus.attach(drive.poll)
    return CanOpenBridge(master), drives, bus


def config(count: int = 3) -> str:
    roles = ["right_arm.shoulder.pitch", "right_arm.elbow.flex", "right_arm.wrist.roll"]
    return json.dumps(
        {
            "joints": [
                {"node": n, "name": f"J{n}", "role": roles[(n - 1) % len(roles)]}
                for n in range(1, count + 1)
            ]
        }
    )


class LateEndpoint(Bus):
    """A loopback endpoint that never has anything on the first look.

    The shape of every real bus, which the loopback bus cannot have: the master sends a SYNC and
    looks, and the drives' answers arrive a moment after that look. Every other `recv` finds
    nothing whatever is waiting, and `synchronous` is false, as it is on a kernel bus. The bridge's
    reads were only ever exercised on a bus that cannot be late until F71.
    """

    synchronous = False

    def __init__(self, inner: Bus) -> None:
        self._inner = inner
        self._ready = False

    def send(self, frame: frames.Frame) -> None:
        self._inner.send(frame)

    def recv(self, timeout_s: float = 0.0) -> frames.Frame | None:
        del timeout_s
        self._ready = not self._ready
        return self._inner.recv() if self._ready else None


class DroppingEndpoint(Bus):
    """A loopback endpoint that never delivers one identifier — an axis whose TPDO is lost."""

    synchronous = True

    def __init__(self, inner: Bus, lost: int) -> None:
        self._inner = inner
        self._lost = lost

    def send(self, frame: frames.Frame) -> None:
        self._inner.send(frame)

    def recv(self, timeout_s: float = 0.0) -> frames.Frame | None:
        while True:
            frame = self._inner.recv(timeout_s)
            if frame is None or frame.arbitration_id != self._lost:
                return frame


class RecordingEndpoint(Bus):
    """A loopback endpoint that remembers every frame the bridge sent, in order: what the stop
    actually put on the wire, which no drive's final state can say (a drive that ignored the
    frame ends in the same state as one that was never sent it)."""

    synchronous = True

    def __init__(self, inner: Bus) -> None:
        self._inner = inner
        self.sent: list[frames.Frame] = []
        self.fail = False

    def send(self, frame: frames.Frame) -> None:
        if self.fail:
            raise OSError("the bus is down")
        self.sent.append(frame)
        self._inner.send(frame)

    def recv(self, timeout_s: float = 0.0) -> frames.Frame | None:
        return self._inner.recv(timeout_s)


class DuplicatingEndpoint(Bus):
    """A loopback endpoint that delivers every TPDO2 twice: the answer to an earlier SYNC is
    still waiting when the next read begins, as on a bus where a drive is a moment slow."""

    synchronous = True

    def __init__(self, inner: Bus) -> None:
        self._inner = inner
        self._queue: list[frames.Frame] = []

    def send(self, frame: frames.Frame) -> None:
        self._inner.send(frame)

    def recv(self, timeout_s: float = 0.0) -> frames.Frame | None:
        fresh = []
        while (frame := self._inner.recv(0.0)) is not None:
            fresh.append(frame)
        # What is new, then a second copy of each TPDO2 behind it: the copies are still waiting
        # when the reader has taken the originals and stopped.
        self._queue.extend(fresh)
        self._queue.extend(f for f in fresh if 0x281 <= f.arbitration_id <= 0x2FF)
        return self._queue.pop(0) if self._queue else None


class WrongModeDisplay(SimulatedDrive):
    """A drive that says it runs a profile mode whatever it was asked for."""

    def _live(self, index: int, subindex: int):
        if (index, subindex) == (cia402.OD.MODES_DISPLAY.index, 0):
            return 1
        return super()._live(index, subindex)


def main() -> int:
    print("skynet-canopen selftest\n")

    # ── the wire ─────────────────────────────────────────────────────────────
    section("frames — CANopen on the wire")

    request = frames.sdo_download_request(3, 0x6040, 0, 0x000F, 2)
    check(request.arbitration_id == 0x603, "an SDO request goes to 0x600 + node")
    check(
        frames.parse_sdo_request(request.data) == ("download", 0x6040, 0, 0x000F, 2),
        "and parses back to the object and value it carried",
    )
    response = frames.sdo_upload_response(3, 0x6041, 0, 0x0237, 2)
    check(response.arbitration_id == 0x583, "an SDO response comes from 0x580 + node")
    check(
        frames.parse_sdo_response(response.data, 0x6041, 0) == 0x0237,
        "and yields the value when the object matches",
    )

    # The refusal that stops a stale reply being read as this one's answer.
    try:
        frames.parse_sdo_response(response.data, 0x6064, 0)
        check(False, "a reply about another object is refused")
    except ValueError as error:
        check("answered about" in str(error), "a reply about another object is refused, by name")

    abort = frames.sdo_abort_response(3, 0x608F, 1, frames.ABORT_NO_SUCH_OBJECT)
    try:
        frames.parse_sdo_response(abort.data, 0x608F, 1)
        check(False, "an abort raises")
    except frames.SdoAbort as raised:
        check(raised.code == frames.ABORT_NO_SUCH_OBJECT, "an abort raises with CiA 301's own code")
        check("no such object" in raised.explain(), "and explains it in a sentence")

    # A negative target has to survive the round trip as two's complement, in both directions.
    for value in (-1, -2**31, 2**31 - 1, 0, 12345):
        frame = frames.rpdo2(1, 0x000F, value)
        check(frames.parse_rpdo2(frame.data) == (0x000F, value), f"RPDO2 round-trips {value}")

    # ── the profile ──────────────────────────────────────────────────────────
    section("cia402 — the drive profile")

    for state in cia402.State:
        word = cia402.statusword_for(state)
        check(cia402.state_of(word) is state, f"statusword {word:#06x} reads back as {state.value}")

    check(
        cia402.state_of(0x0031) is cia402.State.READY_TO_SWITCH_ON,
        "a real drive's statusword decodes",
    )
    # The masking order the module header warns about: `0x4F & 0x27` is `0x07`, so a wrongly ordered
    # decode reads `operation enabled` as `fault reaction active` — a robot the operator is told has
    # faulted while it is moving.
    check(
        cia402.state_of(cia402.statusword_for(cia402.State.OPERATION_ENABLED))
        is cia402.State.OPERATION_ENABLED,
        "and `operation enabled` is not swallowed by a narrower mask",
    )

    # Fault reset is an edge, not a level.
    reset = cia402.controlword_for(cia402.Command.FAULT_RESET)
    check(
        cia402.command_of(reset, previous=0x0000) is cia402.Command.FAULT_RESET,
        "bit 7 rising is a fault reset",
    )
    check(
        cia402.command_of(reset, previous=reset) is not cia402.Command.FAULT_RESET,
        "and bit 7 held is not — a master that held it would clear every fault as it appeared",
    )

    # Units, and the refusal that is the whole reason they are read off the drive.
    rad = cia402.increments_to_rad(131072, 131072, 1, 50, 1)
    check(
        abs(rad - (2 * math.pi / 50)) < 1e-12,
        "131072 increments through a 50:1 gearbox is one motor turn at the shaft",
    )
    check(
        cia402.rad_to_increments(rad, 131072, 1, 50, 1) == 131072,
        "and the inverse returns the increments it came from",
    )
    try:
        cia402.increments_to_rad(1, 131072, 0, 50, 1)
        check(False, "a zero denominator is refused")
    except ValueError as error:
        check("not a ratio" in str(error), "a zero in a ratio is refused rather than divided by")

    # ── the bridge, against simulated drives ─────────────────────────────────
    section("bridge — three axes on a bus")

    bridge, drives, _bus = build(3)
    check(bridge.open(config(3)) == rhal.SKR_OK, "three drives come up")
    check(
        all(drive.state is cia402.State.OPERATION_ENABLED for drive in drives),
        "and every one of them reports `operation enabled` itself",
    )

    descriptor = json.loads(bridge.describe())
    check(descriptor["dof"] == 3, "the descriptor declares three degrees of freedom")
    check(descriptor["simulator"] is True, "and declares itself simulated (§35.3)")
    check(descriptor["envelope"] is None, "and declares no safety envelope")
    check(
        descriptor["capabilities"] == ["safety.estop"],
        "and declares the software stop it has — quick-stop plus NMT pre-op, latched",
    )
    check(
        "effort_max_nm" not in descriptor["joints"][0]["limits"],
        "and no torque rating — 0x6076 is the motor's design value, not a joint safety rating",
    )
    check(
        descriptor["joints"][0]["limits"]["position_min_rad"] < 0
        < descriptor["joints"][0]["limits"]["position_max_rad"],
        "travel comes from the drive's own software limits, converted with its own units",
    )

    state = rhal.State()
    check(bridge.read_state(state, 0) == rhal.SKR_OK, "read_state answers")
    check(len(state.position_rad) == 3, "with one position per axis")
    check(all(math.isfinite(p) for p in state.position_rad), "every position is finite")
    # §24: a quantity the driver does not measure is NaN. TPDO2's default mapping carries the
    # statusword and the position and nothing else.
    check(
        all(math.isnan(v) for v in state.velocity_rad_s),
        "velocity is NaN, because the default PDO mapping does not carry it",
    )

    first = state.seq
    bridge.read_state(state, 0)
    check(state.seq != first, "and the sequence advances between reads")

    # A command reaches the axis, and the axis gets there.
    command = rhal.Command(
        stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[0.5, 0.0, 0.0], mask=[1, 0, 0]
    )
    check(bridge.write_command(command, 0) == rhal.SKR_OK, "a position command is accepted")
    for _ in range(80):
        bridge.write_command(command, 0)
        bridge.read_state(state, 0)
    check(abs(state.position_rad[0] - 0.5) < 1e-3, "and the axis reaches the setpoint")
    check(abs(state.position_rad[1]) < 1e-9, "while a masked-out joint stays where it was")

    check(
        bridge.write_command(
            rhal.Command(mode=rhal.MODE_POSITION, values=[0.0] * 2, mask=[1, 1]), 0
        )
        == rhal.SKR_EINVAL,
        "a command with the wrong joint count is refused",
    )
    check(
        bridge.write_command(
            rhal.Command(mode=rhal.MODE_TORQUE, values=[0.0] * 3, mask=[1, 1, 1]), 0
        )
        == rhal.SKR_EINVAL,
        "and so is a mode this driver has not wired",
    )
    check(
        bridge.write_command(
            rhal.Command(mode=rhal.MODE_POSITION, values=[math.nan, 0, 0], mask=[1, 1, 1]), 0
        )
        == rhal.SKR_EINVAL,
        "and so is a setpoint that is not a number",
    )
    # F73: a refusal used to be decided axis by axis while the frames went out, so a bad value on
    # the second axis was refused after the first axis had already been sent its setpoint.
    before = [drive.target for drive in drives]
    rc = bridge.write_command(
        rhal.Command(mode=rhal.MODE_POSITION, values=[0.25, math.nan, 0.0], mask=[1, 1, 1]), 0
    )
    bridge.read_state(state, 0)  # lets the drives consume whatever was sent
    check(
        rc == rhal.SKR_EINVAL and [drive.target for drive in drives] == before,
        "a command refused for one axis moves none of them — validated whole before a frame "
        "goes out (F73)",
    )
    try:
        rc = bridge.write_command(
            rhal.Command(mode=rhal.MODE_POSITION, values=[1.0e6, 0.0, 0.0], mask=[1, 1, 1]), 0
        )
    except Exception as error:  # the failure this check exists for is an exception, so it is caught
        rc = f"{type(error).__name__}: {error}"
    check(
        rc == rhal.SKR_EINVAL,
        "and a setpoint beyond RPDO2's 32-bit target is refused, not raised out of the driver (F73)",
    )

    check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    bridge.read_state(state, 0)
    check(state.flags & rhal.FLAG_ESTOP != 0, "and the next state says the stop is asserted")
    check(
        bridge.write_command(command, 0) == rhal.SKR_EPERM,
        "and a command after it is refused",
    )
    check(bridge.close() == rhal.SKR_OK, "close")
    # F73: `open` appended to the axes a previous session had left and never reset the latch, so
    # the same bridge opened twice was a body of six joints that refused every command.
    check(bridge.open(config(3)) == rhal.SKR_OK, "the same bridge opens again")
    check(
        json.loads(bridge.describe())["dof"] == 3,
        "as the same three axes rather than six (F73)",
    )
    check(
        bridge.write_command(command, 0) == rhal.SKR_OK,
        "and the stop's latch belonged to the session it was asserted in (F73)",
    )
    check(bridge.close() == rhal.SKR_OK, "close again")

    # ── F71: a bus where the drives answer after the master has looked ────────
    section("bridge — a bus that answers later than the first look (F71)")

    late_bus = LoopbackBus()
    late_drives = []
    for node in (1, 2, 3):
        drive = SimulatedDrive(late_bus.endpoint(), node)
        late_drives.append(drive)
        late_bus.attach(drive.poll)
    bridge = CanOpenBridge(LateEndpoint(late_bus.endpoint()))
    check(bridge.open(config(3)) == rhal.SKR_OK, "three drives come up on a late bus")
    state = rhal.State()
    check(
        bridge.read_state(state, 0) == rhal.SKR_OK,
        "the first read answers — it waits for this SYNC's TPDO2s rather than taking an empty "
        "look for a silent body",
    )
    command = rhal.Command(
        stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[0.5, 0.0, 0.0], mask=[1, 0, 0]
    )
    bridge.write_command(command, 0)
    bridge.read_state(state, 0)
    reported = state.position_rad[0]
    check(
        reported == bridge.axes[0].to_rad(late_drives[0].position) and reported != 0.0,
        "and a read reports the position the drive published for *this* SYNC, not the one before",
    )
    check(state.flags & rhal.FLAG_DEGRADED == 0, "with every axis answering, nothing is degraded")
    bridge.close()

    lossy_bus = LoopbackBus()
    for node in (1, 2, 3):
        drive = SimulatedDrive(lossy_bus.endpoint(), node)
        lossy_bus.attach(drive.poll)
    bridge = CanOpenBridge(DroppingEndpoint(lossy_bus.endpoint(), frames.tpdo2_id(2)))
    check(bridge.open(config(3)) == rhal.SKR_OK, "a body whose second axis's TPDO2 is lost opens")
    check(bridge.read_state(state, 0) == rhal.SKR_OK, "and reads, because two axes answered")
    check(
        state.flags & rhal.FLAG_DEGRADED != 0,
        "and says it is degraded: one position under this stamp is older than the stamp",
    )
    bridge.close()

    bridge, _drives, _bus = build(1)
    check(
        bridge.open(json.dumps({**json.loads(config(1)), "sync_window_ms": 0}))
        == rhal.SKR_ENODEV,
        "a SYNC window of zero is refused rather than read as no waiting",
    )

    # ── F174: drives that take time to change state ──────────────────────────
    section("bridge — drives that take time to change state (F174)")

    from skynet_canopen import bridge as bridge_module

    # A drive spends the seconds after power-up in `not ready to switch on`, which is the drive's own
    # transition and is not read as a command. `open` used to write `shutdown` once, read the state
    # once, and refuse a healthy drive for reporting the state it was in.
    bridge, drives, _bus = build(2, boot_reads=4)
    check(
        drives[0].state is cia402.State.NOT_READY_TO_SWITCH_ON,
        "a drive still initialising reports `not ready to switch on` when the bus finds it",
    )
    check(bridge.open(config(2)) == rhal.SKR_OK, "and comes up once it has finished initialising")
    check(
        all(drive.state is cia402.State.OPERATION_ENABLED for drive in drives),
        "with every drive `operation enabled` by its own report",
    )

    # A drive that evaluates its controlword on its own control cycle acknowledges the SDO before its
    # state machine has run, so the statusword read straight after is the state it was in.
    for reads in (1, 3):
        bridge, drives, _bus = build(2, cycle_reads=reads)
        check(
            bridge.open(config(2)) == rhal.SKR_OK
            and all(d.state is cia402.State.OPERATION_ENABLED for d in drives),
            f"a drive that acts on a controlword {reads} of its own cycles later comes up",
        )

    # The control: waiting is bounded, and a drive that never finishes is still refused by name.
    saved = bridge_module._STATE_TIMEOUT_S
    bridge_module._STATE_TIMEOUT_S = 0.05
    try:
        bridge, _drives, _bus = build(1, boot_reads=10**9)
        rc = bridge.open(config(1))
    finally:
        bridge_module._STATE_TIMEOUT_S = saved
    check(rc == rhal.SKR_ENODEV, "a drive that never leaves `not ready to switch on` is refused")

    # And a fault is not waited on: a drive that latches one at the first controlword ends the walk
    # at once, however long the wait would otherwise be.
    class FaultsOnFirstCommand(SimulatedDrive):
        def _execute(self, controlword: int) -> None:
            super()._execute(controlword)
            if self.state is not cia402.State.FAULT:
                self.fault()

    bus = LoopbackBus()
    drive = FaultsOnFirstCommand(bus.endpoint(), 1)
    bus.attach(drive.poll)
    bridge = CanOpenBridge(bus.endpoint())
    bridge_module._STATE_TIMEOUT_S = 30.0
    started = time.monotonic()
    try:
        rc = bridge.open(config(1))
    finally:
        bridge_module._STATE_TIMEOUT_S = saved
    check(
        rc == rhal.SKR_ENODEV and time.monotonic() - started < 5.0,
        "a drive that faults during the walk is refused at once and not after the wait",
    )

    # ── the refusals that are the point ──────────────────────────────────────
    section("bridge — what it refuses to bring up")

    bridge, _drives, _bus = build(1, report_units=False)
    rc = bridge.open(config(1))
    check(rc == rhal.SKR_ENODEV, "a drive that will not report its encoder resolution is refused")

    # The control: the same drive, reporting its units, comes up.
    bridge, _drives, _bus = build(1)
    check(bridge.open(config(1)) == rhal.SKR_OK, "and the same drive reporting them comes up")

    # F73: the zero check covered three of the four figures, and the fourth was divided by.
    bridge, _drives, _bus = build(1, gear_motor=0)
    try:
        rc = bridge.open(config(1))
    except Exception as error:  # the failure this check exists for is an exception, so it is caught
        rc = f"{type(error).__name__}: {error}"
    check(
        rc == rhal.SKR_ENODEV,
        "a drive reporting a gear ratio of 0/1 is refused rather than divided by (F73)",
    )

    bridge, drives, _bus = build(1)
    drives[0]._objects[(0x1A01, 1)] = ((0x6041 << 16) | 16, 4, False)
    drives[0]._objects[(0x1A01, 2)] = ((0x606C << 16) | 32, 4, False)  # velocity, not position
    check(
        bridge.open(config(1)) == rhal.SKR_ENODEV,
        "a drive whose TPDO2 maps velocity where this driver reads position is refused",
    )

    bridge, drives, _bus = build(1)
    drives[0].fault()
    check(
        bridge.open(config(1)) == rhal.SKR_ENODEV,
        "a drive that has latched a fault is refused, rather than commanded into one",
    )

    bridge, _drives, _bus = build(1)
    check(bridge.open("{}") == rhal.SKR_ENODEV, "a configuration with no joints is refused")
    check(bridge.open("not json") == rhal.SKR_EINVAL, "and one that is not JSON is refused")
    bridge, _drives, _bus = build(1)
    check(
        bridge.open(json.dumps({"joints": [{"node": 200, "name": "J1"}]})) == rhal.SKR_ENODEV,
        "and a node id outside CANopen's range is refused",
    )
    bridge, _drives, _bus = build(1)
    check(
        bridge.open(json.dumps({"joints": [{"node": 1}]})) == rhal.SKR_ENODEV,
        "and a joint with no name is refused",
    )
    bridge, _drives, _bus = build(1)
    check(
        bridge.open(json.dumps({"joints": [{"node": 1, "name": "J1"}, {"node": 1, "name": "J2"}]}))
        == rhal.SKR_ENODEV,
        "and so are two joints on one node, which would be one drive given two setpoints (F73)",
    )

    # ── the drive the bridge talks to ────────────────────────────────────────
    section("slave — the substrate refuses too, which is what makes it one")

    bus = LoopbackBus()
    master = bus.endpoint()
    drive = SimulatedDrive(bus.endpoint(), 1)
    bus.attach(drive.poll)

    master.send(frames.sdo_upload_request(1, 0x9999, 0))
    reply = _await(master, 0x581)
    try:
        frames.parse_sdo_response(reply.data, 0x9999, 0)
        check(False, "an unknown object aborts")
    except frames.SdoAbort as raised:
        check(raised.code == frames.ABORT_NO_SUCH_OBJECT, "an unknown object aborts with 0x06020000")

    master.send(frames.sdo_download_request(1, 0x608F, 1, 42, 4))
    reply = _await(master, 0x581)
    try:
        frames.parse_sdo_response(reply.data, 0x608F, 1)
        check(False, "a read-only object aborts on a write")
    except frames.SdoAbort as raised:
        check(raised.code == frames.ABORT_READ_ONLY, "a read-only object aborts on a write")

    master.send(frames.sdo_download_request(1, 0x607A, 0, 1000, 4))
    _await(master, 0x581)
    check(
        drive.targets_dropped == 1,
        "a target written to a drive that is not enabled is dropped and counted",
    )
    check(drive.target == 0, "and the drive's target is unchanged")

    seeded_gaps()

    # ── the bus this machine does not have ───────────────────────────────────
    section("bus — SocketCAN, where there is one")

    if "--lely" in sys.argv[1:]:
        lely_checks("vcan0")
    elif "--vcan" in sys.argv[1:]:
        vcan_checks("vcan0")
    elif can_available():
        check(True, "this kernel has CAN support; `make check-vcan` runs the driver against it")
    else:
        print(
            "  ----  this kernel has no CAN support, so SocketCanBus was not exercised at all.\n"
            "        `vcan` needs `modprobe`, a kernel with CONFIG_CAN, and the privilege to load\n"
            "        a module. What ran above is the loopback bus: the identical protocol with no\n"
            "        arbitration, no bit timing and no bandwidth — so it says nothing about whether\n"
            "        eight axes fit in a millisecond at 1 Mbit. That row stays in\n"
            "        DRY-COVERAGE-PLAN.md's `not dry-verifiable` column."
        )

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 1 if FAILURES else 0


def seeded_gaps() -> None:
    """What `tools/seed-defects` found this suite could not see (DRY-COVERAGE-PLAN.md 12.C.3): each
    block is a defect of a kind that has happened to a driver here, seeded into this one and let
    through by the checks above. Every block was shown failing against its defect before it was
    kept."""

    section("seeded gaps: the wire, written out from CiA 301 and not read back from the module")
    check(
        (frames.tpdo2_id(1), frames.rpdo2_id(1), frames.SYNC_ID, frames.NMT_ID)
        == (0x281, 0x301, 0x080, 0x000),
        "TPDO2 is 0x280 + node, RPDO2 0x300 + node, SYNC 0x080 and NMT 0x000: the default allocation",
    )
    check(
        frames.sdo_upload_request(2, 0x6041, 0).arbitration_id == 0x602
        and frames.sdo_upload_response(2, 0x6041, 0, 1, 2).arbitration_id == 0x582
        and frames.heartbeat_id(2) == 0x702,
        "an SDO request goes to 0x600 + node, its reply comes from 0x580 + node, a heartbeat from 0x700",
    )
    check(
        cia402.controlword_for(cia402.Command.QUICK_STOP) == 0x0002
        and cia402.controlword_for(cia402.Command.SHUTDOWN) == 0x0006
        and cia402.controlword_for(cia402.Command.ENABLE_OPERATION) == 0x000F,
        "quick stop is controlword 0x0002, shutdown 0x0006, enable operation 0x000F: CiA 402's",
    )
    per_count = 2 * math.pi / (131072 * 50)
    check(
        [
            cia402.rad_to_increments(n * per_count, 131072, 1, 50, 1)
            for n in (1.4, 1.6, 2.5, -1.4, -1.6, -2.5)
        ]
        == [1, 2, 3, -1, -2, -3],
        "a setpoint rounds half away from zero: truncating biases every command toward zero, and a "
        "control loop integrates a bias",
    )

    section("seeded gaps: a state that says only what was measured")
    bridge, drives, _bus = build(3)
    bridge.open(config(3))
    state = rhal.State()
    bridge.read_state(state, 0)
    check(all(math.isnan(e) for e in state.effort_nm), "torque is NaN: TPDO2 does not carry it")
    drives[1].state = cia402.State.FAULT_REACTION_ACTIVE
    bridge.read_state(state, 0)
    check(
        state.flags & rhal.FLAG_FAULT != 0 and state.flags & rhal.FLAG_DEGRADED != 0 and state.fault_code >> 16 == 2,
        "a drive still reacting to its fault is a fault, DEGRADED beside it for §27, and the fault word names its node in the upper half",
    )
    drives[1].state = cia402.State.FAULT
    bridge.read_state(state, 0)
    check(
        state.flags & rhal.FLAG_FAULT != 0 and state.fault_code >> 16 == 2,
        "as is one that has latched it",
    )
    check(
        bridge.write_command(rhal.Command(mode=rhal.MODE_POSITION, values=[0.0] * 3, mask=[1] * 3), 0) == rhal.SKR_EPERM,
        "and a command is refused while a drive holds a fault: the enable controlword would be sent to a drive that drops it, and answered OK",
    )
    # 12.C.6: the drive's own account of why. A fault's emergency object carries CiA 301's code, and the fault word carries it verbatim in
    # its lower half — the node alone is not the maker's code, and the black box is where a person looks the code up.
    drives[1].fault(0x7305)
    bridge.read_state(state, 0)
    check(state.fault_code == (2 << 16 | 0x7305), f"a drive's emergency code reaches the fault word verbatim, beside its node: {state.fault_code:#x}")
    drives[1].state = cia402.State.OPERATION_ENABLED
    drives[1].faulted = False
    bridge.read_state(state, 0)
    check(state.fault_code == 0 and not state.flags & (rhal.FLAG_FAULT | rhal.FLAG_DEGRADED), "and a drive that leaves its fault is read as well again, its code with it")
    bridge.close()

    bus = LoopbackBus()
    for node in (1, 2, 3):
        drive = SimulatedDrive(bus.endpoint(), node)
        bus.attach(drive.poll)
    silent = bus.endpoint()
    for node in (1, 2, 3):
        silent = DroppingEndpoint(silent, frames.tpdo2_id(node))
    bridge = CanOpenBridge(silent)
    bridge.open(config(3))
    check(
        bridge.read_state(state, 0) == rhal.SKR_ETIMEDOUT,
        "a bus that answered nothing is ETIMEDOUT, not a healthy read of the previous positions",
    )
    bridge.close()

    bus = LoopbackBus()
    drives = []
    for node in (1, 2, 3):
        drive = SimulatedDrive(bus.endpoint(), node)
        drives.append(drive)
        bus.attach(drive.poll)
    bridge = CanOpenBridge(DuplicatingEndpoint(bus.endpoint()))
    bridge.open(config(3))
    bridge.read_state(state, 0)
    bridge.write_command(
        rhal.Command(mode=rhal.MODE_POSITION, values=[0.5, 0.0, 0.0], mask=[1, 0, 0]), 0
    )
    bridge.read_state(state, 0)
    check(
        state.position_rad[0] == bridge.axes[0].to_rad(drives[0].position)
        and state.position_rad[0] != 0.0,
        "the answer to an earlier SYNC that is still waiting is drained before the next one: a read "
        "reports this SYNC's position, not the one before under this read's stamp (F71)",
    )
    bridge.close()

    section("seeded gaps: a drive that does not do what it was asked")
    bus = LoopbackBus()
    drive = WrongModeDisplay(bus.endpoint(), 1)
    bus.attach(drive.poll)
    check(
        CanOpenBridge(bus.endpoint()).open(config(1)) == rhal.SKR_ENODEV,
        "a drive that reports a profile mode after it was asked for cyclic synchronous is refused: "
        "a profile mode plans its own trajectory and does not know the Safety Kernel exists",
    )

    section("seeded gaps: what the stop puts on the wire, and what release_stop undoes")
    bus = LoopbackBus()
    drives = []
    for node in (1, 2, 3):
        drive = SimulatedDrive(bus.endpoint(), node)
        drives.append(drive)
        bus.attach(drive.poll)
    recording = RecordingEndpoint(bus.endpoint())
    bridge = CanOpenBridge(recording)
    bridge.open(config(3))
    command = rhal.Command(
        stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[0.4, -0.3, 0.2], mask=[1, 1, 1]
    )
    for _ in range(40):
        bridge.write_command(command, 0)
        bridge.read_state(state, 0)
    held = [axis.target for axis in bridge.axes]
    recording.sent.clear()
    check(bridge.emergency_stop() == rhal.SKR_OK, "the stop returns")
    stop = [(f.arbitration_id, bytes(f.data)) for f in recording.sent]
    want = [
        (
            frames.rpdo2_id(node),
            (0x0002).to_bytes(2, "little") + held[node - 1].to_bytes(4, "little", signed=True),
        )
        for node in (1, 2, 3)
    ] + [(0x000, bytes([0x80, 0x00]))]
    check(
        stop == want,
        "and it is one quick-stop RPDO2 per axis, carrying that axis's held target (never zero), "
        "then NMT pre-operational to every node: the drives stop under control and stop listening",
    )
    bridge.read_state(state, 0)  # the loopback bus delivers when the master next looks
    check(
        all(drive.state is cia402.State.QUICK_STOP_ACTIVE for drive in drives),
        "every drive is in `quick stop active`, not switched off",
    )
    recording.sent.clear()
    check(bridge.release_stop() == rhal.SKR_OK, "release_stop returns")
    check(
        [(f.arbitration_id, bytes(f.data)) for f in recording.sent]
        == [(0x000, bytes([0x01, 0x00]))],
        "and it is one NMT start to every node: the bus is operational again, or the drives ignore "
        "every PDO while the latch says they may be commanded",
    )
    check(
        bridge.write_command(command, 0) == rhal.SKR_OK,
        "and the latch is clear: the stop belonged to the session that asserted it",
    )
    bridge.emergency_stop()
    recording.fail = True
    check(
        bridge.release_stop() == rhal.SKR_EIO
        and bridge.write_command(command, 0) == rhal.SKR_EPERM,
        "a release that could not reach the bus is EIO and leaves the latch set: the bus never left "
        "pre-operational, and a cleared latch would claim the body commandable",
    )
    recording.fail = False
    bridge.close()


def vcan_checks(interface: str) -> None:
    """The bridge on a kernel CAN bus, against simulated drives in another process.

    `make check-vcan` is the way in: it creates `vcan0` in an unprivileged network namespace when
    there is none, which needs no root. Asked for and not possible is a failure, never a skip —
    `--vcan` is a request to measure this, and a run that measured nothing must not pass.
    """
    section(f"bridge — on {interface}, against drives in another process (F71)")
    if not can_available():
        check(False, "--vcan was asked for and this kernel will not give out a CAN socket")
        return

    here = os.path.dirname(os.path.abspath(__file__))
    environment = dict(os.environ)
    environment["PYTHONPATH"] = os.pathsep.join(
        [here, os.path.join(here, "..", "..", "bindings", "python")]
    )

    def drives(*extra: str) -> subprocess.Popen:
        process = subprocess.Popen(
            [sys.executable, "-m", "skynet_canopen.drives", interface, "1", "2", "3", *extra],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            env=environment,
        )
        assert process.stdout is not None
        if process.stdout.readline().strip() != "ready":
            raise AssertionError("the drives did not come up")
        return process

    def stop(process: subprocess.Popen) -> None:
        assert process.stdin is not None
        process.stdin.close()
        process.wait(timeout=5)

    process = drives()
    try:
        bridge = CanOpenBridge(SocketCanBus(interface))
        check(bridge.open(config(3)) == rhal.SKR_OK, "three drives in another process come up")
        state = rhal.State()
        check(
            bridge.read_state(state, 0) == rhal.SKR_OK,
            "the first read answers: the drives' TPDO2s arrive after the SYNC, and it waits",
        )
        command = rhal.Command(
            stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[0.5, -0.25, 0.0], mask=[1, 1, 0]
        )
        failures = 0
        for _ in range(60):
            bridge.write_command(command, 0)
            if bridge.read_state(state, 0) != rhal.SKR_OK:
                failures += 1
        check(failures == 0, "sixty commanded cycles, and every read answered")
        check(
            abs(state.position_rad[0] - 0.5) < 1e-3 and abs(state.position_rad[1] + 0.25) < 1e-3,
            "and both commanded axes reached their setpoints over the kernel's bus",
        )
        check(state.flags & rhal.FLAG_DEGRADED == 0, "with no axis missing a SYNC")
        check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
        bridge.read_state(state, 0)
        check(state.flags & rhal.FLAG_ESTOP != 0, "the stop is visible in the next state")
        check(bridge.write_command(command, 0) == rhal.SKR_EPERM, "and later commands are refused")
        bridge.close()
    finally:
        stop(process)

    process = drives("--fault", "2")
    try:
        bridge = CanOpenBridge(SocketCanBus(interface))
        check(
            bridge.open(config(3)) == rhal.SKR_ENODEV,
            "a body with a drive that latched a fault does not come up",
        )
    finally:
        stop(process)


def lely_checks(interface: str) -> None:
    """The bridge on a kernel CAN bus, against Lely's CiA 402 slave (Phase 10, item 10.7).

    `make check-lely-docker` is the way in, and it brings up what this expects on `interface`:
    nodes 1, 2 and 3 as `lely/make_dcf.py` configures `ros2_canopen`'s `canopen_fake_slaves` —
    a CANopen stack and a CiA 402 state machine that nobody in this repository wrote — node 4 as the
    slave's own device description ships, and node 5 with its units declared and its PDO mapping
    left as shipped. Asked for and not there is a failure, never a skip.

    Every claim about a drive's state is read off the drive over a second socket, not out of the
    bridge: the bridge reading its own writes back would agree with itself. What this cannot say is
    written in the drive's own limits — `ros2_canopen`'s slave is a mock: it moves an axis in one
    step, in units of a thousand increments, and starts a mode's thread once per mode, so the first
    session on a slave is the one that moves.
    """
    section(f"bridge — on {interface}, against Lely's CiA 402 slave (10.7)")
    if not can_available():
        check(False, "--lely was asked for and this kernel will not give out a CAN socket")
        return

    probe = SocketCanBus(interface)

    def ask(node: int, obj) -> int:
        # The probe hears the whole bus, including the bridge's own SDO transfers with the same node,
        # which use the same two identifiers. What was queued before this question was somebody
        # else's answer, and a reply about another object is skipped rather than read as this one's.
        while probe.recv(0.0) is not None:
            pass
        probe.send(frames.sdo_upload_request(node, obj.index, obj.subindex))
        deadline = time.monotonic() + 1.0
        while time.monotonic() < deadline:
            frame = probe.recv(0.05)
            if frame is None or frame.arbitration_id != frames.sdo_response_id(node):
                continue
            try:
                return frames.parse_sdo_response(frame.data, obj.index, obj.subindex)
            except ValueError:
                continue
        raise AssertionError(f"node {node} did not answer a read of {obj.name}")

    def state_of(node: int) -> cia402.State:
        return cia402.state_of(ask(node, cia402.OD.STATUSWORD))

    def refusal(nodes: list[int]) -> str:
        try:
            CanOpenBridge(SocketCanBus(interface))._open(
                {"joints": [{"node": n, "name": f"J{n}"} for n in nodes], "interface": interface}
            )
        except BridgeRefusal as raised:
            return str(raised)
        return ""

    bridge = CanOpenBridge(SocketCanBus(interface))
    opened = bridge.open(config(3))
    check(opened == rhal.SKR_OK, "three of Lely's drives come up")
    if opened != rhal.SKR_OK:
        return
    check(
        all(state_of(node) is cia402.State.OPERATION_ENABLED for node in (1, 2, 3)),
        "and every one of them reports `operation enabled` to a second master",
    )

    # The figures come from the drive's own objects and the units from its own resolution and gear —
    # 2^22 increments of travel at 2^17 a motor turn through 50:1 — worked out here from the
    # configuration `make_dcf.py` writes, which the driver has no access to.
    joint = json.loads(bridge.describe())["joints"][0]["limits"]
    turns = (1 << 22) / ((1 << 17) * 50)
    check(
        abs(joint["position_max_rad"] - turns * 2 * math.pi) < 1e-9
        and abs(joint["position_min_rad"] + turns * 2 * math.pi) < 1e-9,
        "the descriptor's travel is what the drive's software limits, resolution and gear say",
    )
    check(
        abs(joint["velocity_max_rad_s"] - 3000 / 60 * 2 * math.pi / 50) < 1e-9,
        "and its speed ceiling is 3000 rpm at the motor seen at the shaft",
    )

    state = rhal.State()
    check(bridge.read_state(state, 0) == rhal.SKR_OK, "read_state answers on the first SYNC")
    first = state.seq
    bridge.read_state(state, 0)
    check(state.seq != first, "and the sequence advances")
    check(
        state.flags & rhal.FLAG_DEGRADED == 0,
        "with every axis's TPDO2 arriving inside the SYNC window",
    )
    check(
        bridge.unexpected_frames > 0,
        "and the slave's other PDOs, which this driver does not speak, are counted rather than lost",
    )

    # ros2_canopen's cyclic-position thread quantises a target to a thousand increments, which at
    # 6.5 million a shaft turn is a millionth of a radian short of anything asked for.
    command = rhal.Command(
        stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[0.3, -0.2, 0.1], mask=[1, 1, 0]
    )
    for _ in range(60):
        bridge.write_command(command, 0)
        time.sleep(0.01)
        bridge.read_state(state, 0)
    check(
        abs(state.position_rad[0] - 0.3) < 2e-3 and abs(state.position_rad[1] + 0.2) < 2e-3,
        "both commanded axes reach their setpoints, read back from the slave's own TPDO2",
    )
    check(abs(state.position_rad[2]) < 1e-9, "while the masked-out axis stays where it was")
    check(
        bridge.write_command(
            rhal.Command(mode=rhal.MODE_POSITION, values=[math.nan, 0, 0], mask=[1, 1, 1]), 0
        )
        == rhal.SKR_EINVAL,
        "and a setpoint that is not a number is refused before a frame goes out",
    )

    check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    check(
        all(state_of(node) is cia402.State.QUICK_STOP_ACTIVE for node in (1, 2, 3)),
        "and every drive's own state machine is in `quick stop active`",
    )
    bridge.read_state(state, 0)
    check(state.flags & rhal.FLAG_ESTOP != 0, "the stop is visible in the next state")
    check(bridge.write_command(command, 0) == rhal.SKR_EPERM, "and later commands are refused")

    check(bridge.release_stop() == rhal.SKR_OK, "release_stop returns")
    check(bridge.write_command(command, 0) == rhal.SKR_OK, "and the session commands again")
    check(
        all(state_of(node) is cia402.State.OPERATION_ENABLED for node in (1, 2, 3)),
        "which walks every drive back from `quick stop active` to `operation enabled` itself",
    )
    check(bridge.close() == rhal.SKR_OK, "close")
    check(
        all(state_of(node) is cia402.State.SWITCH_ON_DISABLED for node in (1, 2, 3)),
        "and leaves every drive `switch on disabled`, which is what disable voltage is",
    )

    # The drives this driver must refuse, each for the reason a second reading of the profile gives.
    said = refusal([4])
    check(
        "0x6091" in said and "no such object" in said,
        "a drive with no gear-ratio object — Lely's slave as it ships — is refused for it, by name",
    )
    said = refusal([5])
    check(
        "TPDO2 maps 0x60410010, 0x60610008" in said,
        "a drive with its units but another PDO mapping is refused for the mapping, by name",
    )
    check(
        "did not answer" in refusal([9]),
        "and a node that is not on the bus is refused as one that did not answer",
    )


def _await(endpoint, arbitration_id: int):
    for _ in range(8):
        frame = endpoint.recv()
        if frame is not None and frame.arbitration_id == arbitration_id:
            return frame
    raise AssertionError(f"nothing arrived on {arbitration_id:#05x}")


if __name__ == "__main__":
    raise SystemExit(main())

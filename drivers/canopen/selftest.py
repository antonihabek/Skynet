"""skynet-canopen's own suite: the protocol, the profile, and every refusal the bridge makes.

No test framework and no bus. `drivers/lerobot/selftest.py`'s shape, with one difference in emphasis:
nearly everything asserted here is a **refusal**, and every refusal is written beside the
nearly-identical input that succeeds. A CANopen driver's failure mode is not a crash — it is an axis
that reports a plausible number and moves somewhere else — so a suite that only drove the happy path
would be checking the one thing that cannot go wrong quietly.

    make selftest         # this file
    make check            # and then the whole path through the real ABI
    make check-vcan       # the same driver on a real SocketCAN interface, where there is one
"""

from __future__ import annotations

import json
import math
import sys

import skynet_rhal as rhal

from skynet_canopen import cia402, frames
from skynet_canopen.bridge import BridgeRefusal, CanOpenBridge
from skynet_canopen.bus import LoopbackBus, can_available
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

    check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    bridge.read_state(state, 0)
    check(state.flags & rhal.FLAG_ESTOP != 0, "and the next state says the stop is asserted")
    check(
        bridge.write_command(command, 0) == rhal.SKR_EPERM,
        "and a command after it is refused",
    )
    check(bridge.close() == rhal.SKR_OK, "close")

    # ── the refusals that are the point ──────────────────────────────────────
    section("bridge — what it refuses to bring up")

    bridge, _drives, _bus = build(1, report_units=False)
    rc = bridge.open(config(1))
    check(rc == rhal.SKR_ENODEV, "a drive that will not report its encoder resolution is refused")

    # The control: the same drive, reporting its units, comes up.
    bridge, _drives, _bus = build(1)
    check(bridge.open(config(1)) == rhal.SKR_OK, "and the same drive reporting them comes up")

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

    # ── the bus this machine does not have ───────────────────────────────────
    section("bus — SocketCAN, where there is one")

    if can_available():
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


def _await(endpoint, arbitration_id: int):
    for _ in range(8):
        frame = endpoint.recv()
        if frame is not None and frame.arbitration_id == arbitration_id:
            return frame
    raise AssertionError(f"nothing arrived on {arbitration_id:#05x}")


if __name__ == "__main__":
    raise SystemExit(main())

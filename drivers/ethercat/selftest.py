"""skynet-ethercat's own suite: the frames, the bring-up, the profile and every refusal.

No test framework and no hardware. What this suite is built around is the thing that makes EtherCAT
different from every other bus in this repository: **nothing refuses.** A datagram addressed to a
slave that is absent, mis-addressed or in the wrong state comes back the right length, full of what
the master sent, with a working counter one lower than it should be. So most of what is asserted
here is a *number*, and the interesting half of the suite is the set of cases where that number is
the only thing that went wrong.

    make selftest   # this file
    make check      # and then the whole path through the real ABI
"""

from __future__ import annotations

import json
import struct
import sys

import skynet_rhal as rhal

from skynet_canopen import cia402
from skynet_ethercat import frames, segment, slave
from skynet_ethercat.bridge import EtherCatBridge
from skynet_ethercat.master import EtherCatMaster, MasterError
from skynet_ethercat.segment import LoopbackSegment, SegmentError

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


def drives(count: int, **kwargs) -> list[slave.EtherCatDrive]:
    return [slave.EtherCatDrive(i, name=f"J{i + 1}", **kwargs) for i in range(count)]


# ── the wire ─────────────────────────────────────────────────────────────────


def the_frames_encode() -> None:
    section("frames")
    datagram = frames.Datagram(frames.APRD, 3, (0, slave.REG_AL_STATUS), b"\x00\x00")
    frame = frames.Frame([datagram])
    raw = frame.pack()
    check(len(raw) >= 60, "a short frame is padded to Ethernet's minimum")
    check(
        struct.unpack_from(">H", raw, 12)[0] == frames.ETHERTYPE,
        "the EtherType is EtherCAT's 0x88A4",
    )
    back = frames.Frame.unpack(raw)
    check(len(back.datagrams) == 1, "the padding is not read as a second datagram")
    check(back.datagrams[0].command == frames.APRD, "the command survives")

    two = frames.Frame(
        [
            frames.Datagram(frames.BRD, 1, (0, 0), b"\x00"),
            frames.Datagram(frames.BWR, 2, (0, slave.REG_AL_CONTROL), b"\x02\x00"),
        ]
    )
    back = frames.Frame.unpack(two.pack())
    check(len(back.datagrams) == 2, "two datagrams in one frame round-trip")
    check(
        back.datagrams[0].more and not back.datagrams[1].more,
        "the `more` bit is set on every datagram but the last, and is not the caller's to set",
    )

    try:
        frames.Frame.unpack(b"\x00" * 12 + b"\x08\x00" + b"\x00" * 4)
        check(False, "a frame with the wrong EtherType is refused")
    except frames.FrameError:
        check(True, "a frame with the wrong EtherType is refused")

    try:
        frames.Frame([]).pack()
        check(False, "a frame with no datagram is refused")
    except frames.FrameError:
        check(True, "a frame with no datagram is refused")

    try:
        frames.Datagram(frames.LWR, 1, (0, 0), b"\x00" * 2000).pack()
        check(False, "a datagram past the eleven-bit length is refused")
    except frames.FrameError:
        check(True, "a datagram past the eleven-bit length is refused")


def the_working_counter_is_the_answer() -> None:
    section("the working counter, which is the only thing that says a slave was there")
    bus = LoopbackSegment(drives(3))
    master = EtherCatMaster(bus)
    check(master.scan() == 3, "a broadcast read counts the segment")

    empty = EtherCatMaster(LoopbackSegment([]))
    check(empty.scan() == 0, "an empty segment counts zero rather than raising")

    # The case this whole design exists for: a datagram for a station nobody has. It comes back the
    # right length, full of what was sent, and only the counter says so.
    bus = LoopbackSegment(drives(2))
    master = EtherCatMaster(bus)
    master.configure(2)
    answer = bus.cycle(
        frames.Frame(
            [frames.Datagram(frames.FPRD, 9, (0x7777, slave.REG_AL_STATUS), b"\x00\x00")]
        )
    )
    check(
        answer.datagrams[0].working_counter == 0,
        "a read of a station nobody has returns zero increments — not an error",
    )
    check(
        len(answer.datagrams[0].data) == 2,
        "…and the right number of bytes, which is why a master that ignored the counter would "
        "read them as a measurement",
    )
    try:
        master.exchange(
            frames.Datagram(frames.FPRD, 10, (0x7777, slave.REG_AL_STATUS), b"\x00\x00"), 1
        )
        check(False, "the master refuses a working counter that is not what it asserted")
    except MasterError:
        check(True, "the master refuses a working counter that is not what it asserted")


def auto_increment_is_positional() -> None:
    section("auto-increment addressing")
    ring = drives(3)
    bus = LoopbackSegment(ring)
    # Each slave increments the position as the frame passes and acts only on zero. Slave 1 is
    # addressed by 0xFFFF, which is the master counting backwards from its own port.
    answer = bus.cycle(
        frames.Frame([frames.Datagram(frames.APWR, 1, (0xFFFF, slave.REG_STATION_ADDRESS), b"\x05\x00")])
    )
    check(answer.datagrams[0].working_counter == 1, "exactly one slave serviced it")
    check(ring[1].station_address == 5, "…and it was the second one, by position")
    check(
        ring[0].station_address == 0 and ring[2].station_address == 0,
        "the others were passed through untouched",
    )


def the_bring_up_is_the_protocols() -> None:
    section("Init → Pre-Op → Safe-Op → Op")
    ring = drives(2)
    master = EtherCatMaster(LoopbackSegment(ring))
    master.configure(2)
    check(all(d.station_address for d in ring), "every slave has a station address after Init")
    master.request_state(slave.AL_PREOP, 2)
    check(all(d.al_state == slave.AL_PREOP for d in ring), "Pre-Op is reached")
    master.request_state(slave.AL_SAFEOP, 2)
    master.request_state(slave.AL_OP, 2)
    check(all(d.al_state == slave.AL_OP for d in ring), "and Op")

    # Op without the sync managers configured: refused, with a code, and left where it could hold.
    bare = drives(1)
    raw = LoopbackSegment(bare)
    raw.cycle(
        frames.Frame(
            [frames.Datagram(frames.BWR, 1, (0, slave.REG_AL_CONTROL), struct.pack("<H", slave.AL_OP))]
        )
    )
    check(bare[0].al_error, "Op straight from Init sets the AL error bit")
    check(
        bare[0].al_status_code == slave.ALSTATUS_INVALID_SM_CFG,
        "…with the sync-manager configuration as the reason (0x001D)",
    )
    check(bare[0].al_state != slave.AL_OP, "and the slave is not in Op")

    try:
        EtherCatMaster(LoopbackSegment(drives(4))).configure(3)
        check(False, "a segment with more slaves than the configuration describes is refused")
    except MasterError:
        check(True, "a segment with more slaves than the configuration describes is refused")


def coe_reads_and_refuses() -> None:
    section("CoE: the profile over the mailbox")
    ring = drives(2)
    master = EtherCatMaster(LoopbackSegment(ring))
    master.bring_up(2)
    check(master.sdo_upload(0, 0x608F, 1, 4, False) == 1 << 16, "an encoder resolution reads back")
    check(master.sdo_upload(0, 0x6091, 1, 4, False) == 50, "a gear ratio reads back")
    check(
        cia402.state_of(master.sdo_upload(0, 0x6041, 0, 2, False))
        is cia402.State.SWITCH_ON_DISABLED,
        "the statusword decodes through the same profile the CANopen driver uses",
    )
    try:
        master.sdo_upload(0, 0x1234, 0, 4, False)
        check(False, "an object the drive does not have is an abort, not a zero")
    except MasterError as error:
        check("aborted" in str(error), "an object the drive does not have is an abort, not a zero")

    master.sdo_download(0, 0x6060, 0, cia402.Mode.CSP.value, 1)
    check(
        master.sdo_upload(0, 0x6061, 0, 1, True) == cia402.Mode.CSP.value,
        "a mode written over SDO reads back from the display object",
    )


def the_cycle_carries_everything() -> None:
    section("one LRW, every drive")
    ring = drives(3)
    bus = LoopbackSegment(ring)
    master = EtherCatMaster(bus)
    master.bring_up(3)
    outputs = bytearray(master.logical_length)
    image = master.cycle(bytes(outputs))
    check(len(image) == master.logical_length, "the image comes back the size of the map")

    for word in (0x06, 0x07, 0x0F):
        for index in range(3):
            struct.pack_into("<Hi", outputs, master.output_at[index], word, 0)
        image = master.cycle(bytes(outputs))
    check(
        all(d.state is cia402.State.OPERATION_ENABLED for d in ring),
        "three cycles walk every drive to operation enabled",
    )

    for index in range(3):
        struct.pack_into("<Hi", outputs, master.output_at[index], 0x0F, 40000)
    for _ in range(4):
        image = master.cycle(bytes(outputs))
    _, position = struct.unpack_from("<Hi", image, master.input_at[1])
    check(position == 40000, "a setpoint in the output window reaches the right drive")

    # The failure this transport has that the others do not: a drive that leaves the segment. It is
    # taken out of the segment's own list rather than the caller's copy — which is a detail the
    # first version of this test got wrong, and the symptom was a check that could not fail.
    bus.slaves.pop()
    try:
        master.cycle(bytes(outputs))
        check(False, "a drive that drops off the segment is a refusal, not a short read")
    except MasterError as error:
        check(
            "working counter" in str(error),
            "a drive that drops off the segment is caught by the working counter and nothing else",
        )


# ── the driver ───────────────────────────────────────────────────────────────


def config(count: int = 3, **extra) -> str:
    roles = [
        "right_arm.shoulder.pitch",
        "right_arm.elbow.flex",
        "right_arm.wrist.roll",
    ]
    body = {
        "sim": count,
        "joints": [{"name": f"J{i + 1}", "role": roles[i % len(roles)]} for i in range(count)],
    }
    body.update(extra)
    return json.dumps(body)


def command(mode: int, values, mask=None) -> rhal.Command:
    return rhal.Command(
        stamp_ns=0,
        seq=1,
        mode=mode,
        values=list(values),
        mask=list(mask if mask is not None else [1] * len(values)),
    )


def the_driver_reports_a_body() -> None:
    section("the body the driver reports")
    bridge = EtherCatBridge()
    check(bridge.open(config()) == rhal.SKR_OK, "open() brings up the segment and reads the drives")
    descriptor = json.loads(bridge.describe())
    check(descriptor["dof"] == 3, "three joints")
    check(
        descriptor["envelope"] is None,
        "no envelope: 0x6076 is the motor's rated torque, a design value (§63.4)",
    )
    check(
        all("effort_max_nm" not in joint["limits"] for joint in descriptor["joints"]),
        "and no joint declares a torque ceiling either",
    )
    check(
        all("position_min_rad" in joint["limits"] for joint in descriptor["joints"]),
        "travel is declared, read from 0x607D and converted with the drive's own units",
    )
    check(
        descriptor["control"]["modes"] == ["position"],
        "position only — CSV and CST need a different PDO map, and a driver that accepted the "
        "mode while sending a position would move the joint in the wrong units",
    )
    check(descriptor["simulator"] is True, "a loopback segment declares itself simulated (§35.3)")

    state = rhal.State()
    check(bridge.read_state(state, 0) == rhal.SKR_OK, "read_state succeeds")
    check(len(state.position_rad) == 3, "three positions come back")
    check(
        all(value != value for value in state.velocity_rad_s),
        "velocity is NaN: this driver maps 0x606C into nothing, so nobody measured it — and a "
        "zero would read as 'this joint is still' (§24)",
    )
    first = state.seq
    bridge.read_state(state, 0)
    check(state.seq > first, "the sequence advances between reads")
    bridge.close()


def the_driver_refuses_a_body_with_no_units() -> None:
    section("§5.2's rule: a unit with no provenance")
    bridge = EtherCatBridge()
    # A drive that will not report 0x608F or 0x6091 — which the profile permits, and which makes
    # every position it reports a number in units nobody knows.
    bridge._make_segment = lambda settings, expected: LoopbackSegment(  # noqa: SLF001
        drives(expected, report_units=False)
    )
    check(
        bridge.open(config(2)) == rhal.SKR_EPROTO,
        "a drive that will not report its encoder resolution is refused, not assumed",
    )
    bridge.close()


def the_driver_walks_and_stops() -> None:
    section("the CiA 402 walk, and the stop")
    bridge = EtherCatBridge()
    bridge.open(config(2))
    state = rhal.State()
    target = [0.3, -0.2]
    for _ in range(12):
        bridge.read_state(state, 0)
        check_rc = bridge.write_command(command(rhal.MODE_POSITION, target), 0)
        if check_rc != rhal.SKR_OK:
            break
    bridge.read_state(state, 0)
    check(
        all(abs(a - b) < 1e-3 for a, b in zip(state.position_rad, target)),
        f"the drives reach the commanded positions ({[round(v, 4) for v in state.position_rad]})",
    )

    check(
        bridge.write_command(command(rhal.MODE_TORQUE, target), 0) == rhal.SKR_EPERM,
        "a torque command is refused rather than sent as a position",
    )
    check(
        bridge.write_command(command(200, target), 0) == rhal.SKR_EINVAL,
        "a mode this ABI does not define is malformed, not merely refused",
    )
    check(
        bridge.write_command(command(rhal.MODE_POSITION, [0.1]), 0) == rhal.SKR_EINVAL,
        "a one-joint command to a two-joint body is refused",
    )

    check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    check(
        bridge.write_command(command(rhal.MODE_POSITION, target), 0) == rhal.SKR_EPERM,
        "commands are refused while the latch is set",
    )
    bridge.read_state(state, 0)
    check(state.flags & rhal.FLAG_ESTOP != 0, "and the stop is visible in the next state")
    bridge.close()


def a_latched_fault_is_not_cleared() -> None:
    section("F26's rule, at a second vendor's transport")
    ring = drives(2)
    bridge = EtherCatBridge()
    bridge._make_segment = lambda settings, expected: LoopbackSegment(ring)  # noqa: SLF001
    bridge.open(config(2))
    state = rhal.State()
    ring[0].latch_fault(0x2310)
    bridge.read_state(state, 0)
    check(state.flags & rhal.FLAG_FAULT != 0, "a latched fault reaches §24's fault flag")
    check(state.fault_code == 1, "and names which drive, verbatim rather than interpreted")
    for _ in range(6):
        bridge.read_state(state, 0)
        bridge.write_command(command(rhal.MODE_POSITION, [0.1, 0.1]), 0)
    check(
        ring[0].fault_code == 0x2310,
        "six cycles later the fault is still latched — the driver did not reset it to get past it, "
        "because the erased thing is why the robot stopped",
    )
    bridge.close()


def a_real_segment_refuses_rather_than_falling_back() -> None:
    section("the transport is named, never guessed")
    bridge = EtherCatBridge()
    check(
        bridge.open(json.dumps({"joints": [{"name": "J1", "role": None}]})) == rhal.SKR_EIO,
        "a config with neither an interface nor a `sim` count is refused rather than simulated",
    )
    try:
        segment.RawSocketSegment("definitely-not-an-interface")
        check(False, "a raw segment on an interface that is not there refuses to construct")
    except SegmentError:
        check(True, "a raw segment on an interface that is not there refuses to construct")


def main() -> int:
    the_frames_encode()
    the_working_counter_is_the_answer()
    auto_increment_is_positional()
    the_bring_up_is_the_protocols()
    coe_reads_and_refuses()
    the_cycle_carries_everything()
    the_driver_reports_a_body()
    the_driver_refuses_a_body_with_no_units()
    the_driver_walks_and_stops()
    a_latched_fault_is_not_cleared()
    a_real_segment_refuses_rather_than_falling_back()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

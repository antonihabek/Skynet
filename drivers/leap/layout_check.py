"""This driver's wire and registers, held to ROBOTIS's own SDK, which is run (DRY-COVERAGE-PLAN.md item 12.E).

    LEAP_PYTHON=/path/to/venv/bin/python (pip install --require-hashes -r requirements-layout.txt)
    LEROBOT_TABLES=/path/to/lerobot_tables.py   (make fetch-vendor; optional)
    LEAP_API=/path/to/LEAP_Hand_API             (optional, and see README.md: its licence is non-commercial)
    PYTHONPATH=../../bindings/python:. python layout_check.py

A LEAP Hand has no controller of its own: it is sixteen Dynamixel servos, and the maker of the servos publishes the client that every program for them is built on, `dynamixel_sdk` (ROBOTIS, Apache-2.0, PyPI).
It builds a packet the way a servo expects it and reads one the way a servo sends it, so it is what this driver's bytes are compared with. What is compared, with the SDK on one side and `frames.py` on the other:

* **the CRC and the byte stuffing**, on random buffers and parameters (with `FF FF FD` runs in them);
* **every request this driver sends**: pings, reads, writes, synchronised reads and writes and reboots, byte for byte, as the SDK builds them (random servo ids, addresses and data);
* **every answer this driver reads**: random status packets, and corrupt ones, through the SDK's own `rxPacket` and this driver's `parse_status`, field by field, and the same with a bad CRC, a truncation and noise before the header;
* **the register map**, against LeRobot's `X_SERIES_CONTROL_TABLE` (Apache-2.0, which cites the e-manual) where `LEROBOT_TABLES` points at it;
* **the two together**: this driver brings the simulated hand up and commands it, and the SDK reads back what it did to the registers (torque, mode, gains, goal current, goals), through `GroupSyncRead` both ways (the plain one and
  `fastSyncRead`, which the hand's authors' own client prefers); then the SDK moves the hand with `GroupSyncWrite` and this driver reads where it went and does not move the hand it finds;
* **the hand's authors' own client**, `dynamixel_client.py` and `leap_hand_utils.py` of the LEAP_Hand_API repository, run against the simulated hand, where `LEAP_API` points at a checkout of it. That repository's
  licence is CC BY-NC 4.0, so nothing here fetches or runs it unless a person says so.

It cannot show that a real hand answers any of this: the hand is the simulated one (F15), written from the e-manual and from what the SDK reads, and it is accepted by the SDK because it was made to be.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import random
import runpy
import sys
import time

import skynet_rhal as rhal

from skynet_leap import frames
from skynet_leap.bridge import LeapBridge
from skynet_leap.hand_sim import SimulatedHand

CHECKS = 0
FAILURES: list[str] = []
LEROBOT_TABLES = os.environ.get("LEROBOT_TABLES", "")
LEAP_API = os.environ.get("LEAP_API", "")

try:
    import dynamixel_sdk as dxl
except ImportError:
    print("layout_check: dynamixel_sdk is not installed (pip install --require-hashes -r requirements-layout.txt in a virtual environment, and run this with that interpreter): nothing was compared.")
    sys.exit(2)


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")
        if detail:
            print(f"        {detail}")


def section(title: str) -> None:
    print(f"\n{title}")


def until(predicate, timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # noqa: BLE001 - not true yet
            pass
        time.sleep(0.005)
    return bool(predicate())


class FakePort:
    """What the SDK's packet handler needs of a port, without a port: what it writes is kept, and what it reads is what was put in `rx`."""

    def __init__(self, incoming: bytes = b"") -> None:
        self.is_using = False
        self.written: list[bytes] = []
        self.rx = bytearray(incoming)

    def clearPort(self) -> None:  # noqa: N802 - the SDK's name
        pass

    def writePort(self, packet) -> int:  # noqa: N802
        self.written.append(bytes(packet))
        return len(packet)

    def readPort(self, length: int) -> list[int]:  # noqa: N802
        out = list(self.rx[:length])
        del self.rx[:length]
        return out

    def setPacketTimeout(self, packet_length) -> None:  # noqa: N802
        pass

    def isPacketTimeout(self) -> bool:  # noqa: N802
        return not self.rx


def sdk_bytes(call) -> bytes:
    """What the SDK writes for `call(port, handler)`."""
    port = FakePort()
    handler = dxl.PacketHandler(2.0)
    call(port, handler)
    port.is_using = False
    assert len(port.written) == 1, port.written
    return port.written[0]


def noisy(rng: random.Random, count: int) -> bytes:
    """Random bytes, thick with the ones a packet is made of."""
    return bytes(rng.choice((0xFF, 0xFD, 0x00, 0xFF, 0xFD, rng.randrange(256))) for _ in range(count))


def main_wire() -> None:
    rng = random.Random(0x1EA9)
    section("the CRC and the byte stuffing, against the SDK's")
    handler = dxl.PacketHandler(2.0)
    bad = [length for length in range(0, 120) for _ in range(20) if (lambda data: handler.updateCRC(0, list(data), len(data)) != frames.crc16(data))(noisy(rng, length))]
    check(not bad, f"the CRC of 2,400 random buffers of 0 to 119 bytes is the SDK's (polynomial 0x8005, no reflection): {len(bad)} differ")
    mismatches = 0
    for _ in range(2000):
        params = noisy(rng, rng.randrange(0, 40))
        txpacket = [0xFF, 0xFF, 0xFD, 0x00, rng.randrange(0, 253), (len(params) + 3) & 0xFF, (len(params) + 3) >> 8, 0x03] + list(params) + [0, 0]
        stuffed = bytes(handler.addStuffing(list(txpacket)))
        ours = frames.stuff(params)
        theirs = stuffed[8:-2]
        length = stuffed[5] | stuffed[6] << 8
        if theirs != ours or length != len(ours) + 3:
            mismatches += 1
    check(mismatches == 0, f"byte stuffing of 2,000 random parameter blocks is the SDK's, and its length counts the stuffed bytes: {mismatches} differ")
    own_wrong = clean_wrong = sdk_wrong = 0
    for _ in range(3000):
        params = noisy(rng, rng.randrange(0, 40))
        stuffed = frames.stuff(params)
        packet = [0xFF, 0xFF, 0xFD, 0x00, 1, (len(stuffed) + 3) & 0xFF, (len(stuffed) + 3) >> 8, 0x55] + list(stuffed) + [0, 0]
        removed = bytes(handler.removeStuffing(list(packet)))
        out = removed[8:8 + len(params)]
        if frames.unstuff(stuffed) != params:
            own_wrong += 1
        if out != params:
            sdk_wrong += 1
            if b"\xff\xff\xfd" not in params:
                clean_wrong += 1
    check(own_wrong == 0, f"this driver takes the stuffing out of what it put in, for 3,000 random parameter blocks: {own_wrong} differ")
    check(clean_wrong == 0, f"and the SDK takes it out as this driver does wherever there is none to take out ({sdk_wrong} of the 3,000 differ, and every one of them had a `FF FF FD` stuffed before the difference)")
    pinned = bytes.fromhex("fffffdfffdfd")
    pinned_stuffed = frames.stuff(pinned)
    sdk_out = bytes(handler.removeStuffing([0xFF, 0xFF, 0xFD, 0x00, 1, (len(pinned_stuffed) + 3) & 0xFF, 0, 0x55] + list(pinned_stuffed) + [0, 0]))[8:8 + len(pinned)]
    check(pinned_stuffed == bytes.fromhex("fffffdfdfffdfd") and frames.unstuff(pinned_stuffed) == pinned and sdk_out != pinned,
          f"the shortest block the SDK mis-reads, pinned (F308): `{pinned.hex()}` is sent as `{pinned_stuffed.hex()}`, this driver reads it back whole, and the SDK's `removeStuffing` gives `{sdk_out.hex()}`: it looks two bytes back in a "
          "buffer it has already been shifting left, so the FD of an FF FD FD that follows an earlier removal is taken for one it should remove")

    section("every request this driver sends, as the SDK builds it")
    for name, trials, build_ours, build_theirs in (
            ("ping", 300, lambda r: (frames.ping(r[0]),), lambda r: (lambda port, ph: ph.ping(port, r[0]),)),
            ("reboot", 300, lambda r: (frames.reboot(r[0]),), lambda r: (lambda port, ph: ph.reboot(port, r[0]),)),
            ("read", 800, lambda r: (frames.read(r[0], r[1], r[2]),), lambda r: (lambda port, ph: ph.readTx(port, r[0], r[1], r[2]),)),
    ):
        wrong = 0
        for _ in range(trials):
            r = (rng.randrange(0, 253), rng.randrange(0, 256), rng.randrange(1, 256))
            if build_ours(r)[0] != sdk_bytes(build_theirs(r)[0]):
                wrong += 1
        check(wrong == 0, f"{trials} random {name}s are byte for byte the SDK's: {wrong} differ")
    wrong = stuffed_blocks = 0
    for _ in range(1500):
        servo, address, data = rng.randrange(0, 253), rng.randrange(0, 256), noisy(rng, rng.randrange(1, 12))
        theirs = sdk_bytes(lambda port, ph: ph.writeTxOnly(port, servo, address, len(data), list(data)))
        if b"\xff\xff\xfd" in bytes([address & 0xFF, 0]) + data:
            stuffed_blocks += 1
        elif frames.write(servo, address, data) != theirs:
            wrong += 1
    check(wrong == 0, f"1,500 random writes of 1 to 11 bytes are byte for byte the SDK's wherever the parameters hold no `FF FF FD`: {wrong} differ")
    sdk_unstuffed = sdk_bytes(lambda port, ph: ph.writeTxOnly(port, 5, 0x74, 3, [0xFF, 0xFF, 0xFD]))
    mine = frames.write(5, 0x74, b"\xff\xff\xfd")
    check(sdk_unstuffed[5] == 8 and sdk_unstuffed[8:-2] == bytes([0x74, 0x00, 0xFF, 0xFF, 0xFD]) and mine[5] == 9 and mine[8:-2] == bytes([0x74, 0x00, 0xFF, 0xFF, 0xFD, 0xFD]),
          f"and where they do ({stuffed_blocks} of those 1,500) the SDK sends them unstuffed, pinned (F307): `FF FF FD` in the data goes out as `FF FF FD` with the length unchanged, and the e-manual says an FD must follow it. "
          "The SDK stuffs into a new list and its `txPacket` ignores what `addStuffing` returns. This driver sends the FD")
    wrong = 0
    for _ in range(800):
        address, length, ids = rng.randrange(0, 256), rng.randrange(1, 40), rng.sample(range(0, 253), rng.randrange(1, 17))
        if frames.sync_read(address, length, ids) != sdk_bytes(lambda port, ph: ph.syncReadTx(port, address, length, ids, len(ids), False)):
            wrong += 1
        if frames.fast_sync_read(address, length, ids) != sdk_bytes(lambda port, ph: ph.syncReadTx(port, address, length, ids, len(ids), True)):
            wrong += 1
    check(wrong == 0, f"800 random synchronised reads, plain and fast, are the SDK's: {wrong} differ")
    wrong = 0
    for _ in range(1200):
        address, length = rng.randrange(0, 256), rng.choice((1, 2, 4))
        ids = rng.sample(range(0, 253), rng.randrange(1, 17))
        entries = {servo: noisy(rng, length) for servo in ids}
        param = [b for servo in ids for b in [servo, *entries[servo]]]
        theirs = sdk_bytes(lambda port, ph: ph.syncWriteTxOnly(port, address, length, param, len(param)))
        if b"\xff\xff\xfd" not in bytes([address & 0xFF, address >> 8, length, 0]) + bytes(param) and frames.sync_write(address, length, entries) != theirs:
            wrong += 1
    check(wrong == 0, f"1,200 random synchronised writes are the SDK's wherever the parameters hold no `FF FF FD`: {wrong} differ")
    wrong = 0
    for _ in range(300):
        ids = rng.sample(range(0, 16), rng.randrange(1, 17))
        goals = {servo: rng.randrange(-(1 << 20), 1 << 20) for servo in ids}
        mine = frames.sync_write(frames.ADDR_GOAL_POSITION, 4, {servo: frames.pack_u32(value) for servo, value in goals.items()})

        def group(port, ph):
            writer = dxl.GroupSyncWrite(port, ph, frames.ADDR_GOAL_POSITION, 4)
            for servo, value in goals.items():
                writer.addParam(servo, (value & 0xFFFFFFFF).to_bytes(4, "little"))
            writer.txPacket()
        if mine != sdk_bytes(group):
            wrong += 1
    check(wrong == 0, f"and 300 goal writes the way `GroupSyncWrite` makes them, with signed goals: {wrong} differ")
    wrong = 0
    for _ in range(300):
        ids = rng.sample(range(0, 16), rng.randrange(1, 17))
        address, length = rng.choice((frames.SPAN_STATE, frames.SPAN_GAINS, frames.SPAN_MOTION))

        def group_read(port, ph):
            reader = dxl.GroupSyncRead(port, ph, address, length)
            for servo in ids:
                reader.addParam(servo)
            reader.txPacket()
        if frames.sync_read(address, length, sorted(ids, key=ids.index)) != sdk_bytes(group_read):
            wrong += 1
    check(wrong == 0, f"and 300 reads of the three spans a cycle reads, the way `GroupSyncRead` makes them: {wrong} differ")

    section("every answer this driver reads, as the SDK reads it")
    wrong = disagree = clean = 0
    for _ in range(3000):
        servo, error, params = rng.randrange(0, 253), rng.choice((0, 0, 0, 0x80, 0x01, 0x84, 0x07)), noisy(rng, rng.randrange(0, 30))
        packet = frames.packet(servo, frames.INST_STATUS, bytes([error]) + params)
        port = FakePort(packet)
        data, result, got_error = dxl.PacketHandler(2.0).readRx(port, servo, len(params))
        parsed = frames.parse_status(packet)
        if result != dxl.COMM_SUCCESS or got_error != error or bytes(data) != params:
            wrong += 1
            if b"\xff\xff\xfd" not in params:
                clean += 1
        if parsed is None or parsed[0].servo != servo or parsed[0].error != error or parsed[0].params != params or parsed[1] != len(packet):
            disagree += 1
    check(clean == 0, f"the SDK reads 3,000 random status packets (the alert flag, every error number) as this driver built them wherever the data holds no `FF FF FD` ({wrong} that do it misreads, F308)")
    check(disagree == 0, f"and this driver reads all 3,000 back whole, stuffed data included: {disagree} differ")
    wrong = 0
    for _ in range(600):
        servo, params = rng.randrange(0, 253), noisy(rng, rng.randrange(1, 20))
        packet = bytearray(frames.packet(servo, frames.INST_STATUS, b"\x00" + params))
        packet[rng.randrange(8, len(packet))] ^= 1 << rng.randrange(8)
        sdk_ok = dxl.PacketHandler(2.0).readRx(FakePort(bytes(packet)), servo, len(params))[1] == dxl.COMM_SUCCESS
        try:
            ours_ok = frames.parse_status(bytes(packet)) is not None
        except frames.ProtocolError:
            ours_ok = False
        if sdk_ok != ours_ok:
            wrong += 1
    check(wrong == 0, f"a status packet with one bit flipped is refused by the SDK exactly when it is refused here (600 packets): {wrong} differ")
    wrong = 0
    for _ in range(300):
        servo, params = rng.randrange(0, 253), noisy(rng, rng.randrange(1, 20))
        packet = frames.packet(servo, frames.INST_STATUS, b"\x00" + params)
        cut = packet[:rng.randrange(1, len(packet))]
        result = dxl.PacketHandler(2.0).readRx(FakePort(cut), servo, len(params))[1]
        if result == dxl.COMM_SUCCESS or frames.parse_status(cut) is not None:
            wrong += 1
    check(wrong == 0, f"and a status packet that stops short is waited for by this driver and never accepted by the SDK (300 packets): {wrong} accepted")
    wrong = 0
    for _ in range(300):
        servo, params = rng.randrange(0, 253), noisy(rng, rng.randrange(1, 20))
        packet = frames.packet(servo, frames.INST_STATUS, b"\x00" + params)
        prefix = bytes(rng.choice((0x00, 0x11, 0xAA, 0xFF, 0xFD)) for _ in range(rng.randrange(1, 30)))
        parsed = frames.parse_status(prefix + packet)
        theirs = dxl.PacketHandler(2.0).readRx(FakePort(prefix + packet), servo, len(params))
        if theirs[1] == dxl.COMM_SUCCESS and (parsed is None or parsed[0].params != params):
            wrong += 1
    check(wrong == 0, f"noise before a status packet is skipped by this driver whenever the SDK skips it (300 packets): {wrong} differ")


def main_table() -> None:
    section("the register map, against LeRobot's X-series control table")
    if not LEROBOT_TABLES:
        print("  (LEROBOT_TABLES is not set: `make fetch-vendor` fetches the file, pinned by SHA-256. The register map was not compared.)")
        return
    tables = runpy.run_path(LEROBOT_TABLES)
    table = tables["X_SERIES_CONTROL_TABLE"]
    ours = {"Model_Number": (frames.ADDR_MODEL_NUMBER, 2), "Operating_Mode": (frames.ADDR_OPERATING_MODE, 1), "Current_Limit": (frames.ADDR_CURRENT_LIMIT, 2), "Shutdown": (frames.ADDR_SHUTDOWN, 1),
            "Torque_Enable": (frames.ADDR_TORQUE_ENABLE, 1), "Hardware_Error_Status": (frames.ADDR_HARDWARE_ERROR, 1), "Position_D_Gain": (frames.ADDR_POSITION_D, 2), "Position_I_Gain": (frames.ADDR_POSITION_I, 2),
            "Position_P_Gain": (frames.ADDR_POSITION_P, 2), "Goal_Current": (frames.ADDR_GOAL_CURRENT, 2), "Goal_Position": (frames.ADDR_GOAL_POSITION, 4), "Moving": (frames.ADDR_MOVING, 1),
            "Present_Current": (frames.ADDR_PRESENT_CURRENT, 2), "Present_Velocity": (frames.ADDR_PRESENT_VELOCITY, 4), "Present_Position": (frames.ADDR_PRESENT_POSITION, 4),
            "Present_Input_Voltage": (frames.ADDR_PRESENT_VOLTAGE, 2), "Present_Temperature": (frames.ADDR_PRESENT_TEMPERATURE, 1)}
    for name, (address, size) in ours.items():
        check(tuple(table.get(name, (None, None))) == (address, size), f"{name} is at {address} and {size} byte(s) wide, as LeRobot's table has it ({table.get(name)})")
    start, length = frames.SPAN_STATE
    check(table["Torque_Enable"][0] == start and table["Hardware_Error_Status"][0] == start + length - 1, "the first span a cycle reads runs from Torque_Enable to Hardware_Error_Status")
    start, length = frames.SPAN_GAINS
    check(table["Position_D_Gain"][0] == start and table["Goal_Current"][0] + 2 == start + length, "the second from Position_D_Gain to the end of Goal_Current")
    start, length = frames.SPAN_MOTION
    check(table["Goal_Position"][0] == start and table["Present_Position"][0] + 4 == start + length, "and the third from Goal_Position to the end of Present_Position")
    check(tables["MODEL_NUMBER_TABLE"]["xl330-m288"] == frames.VARIANTS["lite"].model_number, "the Lite's servo, XL330-M288, is model number 1200 in LeRobot's table too")
    check("xc330-m288" not in tables["MODEL_NUMBER_TABLE"], "the Full's servo, XC330-M288, is not in LeRobot's table at all (it has the XC330-T181 and XC330-T288): its model number, 1240, is the e-manual's and nothing here confirms it (F304)")
    check(5 in tables["MODEL_OPERATING_MODES"]["xl330-m288"], "current-based position control (mode 5) is one of the XL330-M288's modes")
    check(tables["MODEL_RESOLUTION"]["xl330-m288"] == 4096 and abs(frames.RAD_PER_TICK - 2 * math.pi / 4096) < 1e-15, "with 4096 ticks to a turn")
    check(tables["X_SERIES_BAUDRATE_TABLE"][4_000_000] == 6, "and 4 Mbaud is the servos' baud code 6")


def configuration(sim: SimulatedHand, **extra) -> str:
    settings = {"variant": "full", "stop_mode": "hold", "current_ma": 350, "gains": {"kp": 600, "ki": 0, "kd": 200}, "side_scale": 0.75, "simulator": True, "rate_hz": 50, "timeout_ms": 150, "device": sim.path}
    settings.update(extra)
    return json.dumps(settings)


def sdk_session(path: str):
    port = dxl.PortHandler(path)
    assert port.openPort(), "the SDK could not open the pseudo-terminal"
    assert port.setBaudRate(4000000), "the SDK could not set 4 Mbaud on it"
    return port, dxl.PacketHandler(2.0)


def retried(call, attempts: int = 4):
    """The SDK's packet timeout is its latency timer's 16 ms twice over, which a loaded machine can miss; a call that did not succeed is made again."""
    result = None
    for _ in range(attempts):
        result = call()
        status = result[1] if isinstance(result, tuple) else result
        if status == dxl.COMM_SUCCESS:
            return result
        time.sleep(0.01)
    return result


def sdk_span(port, handler, address: int, length: int, fast: bool) -> dict[int, bytes] | None:
    reader = dxl.GroupSyncRead(port, handler, address, length)
    for servo in frames.IDS:
        reader.addParam(servo)
    for _ in range(4):
        result = reader.fastSyncRead() if fast else reader.txRxPacket()
        if result == dxl.COMM_SUCCESS:
            return {servo: bytes(reader.data_dict[servo]) for servo in frames.IDS}
        time.sleep(0.01)
    return None


def main_live() -> None:
    section("the two together: this driver brings the hand up, and the SDK reads what it did")
    start = tuple(frames.position_to_register(a) for a in (0.1, 0.2, 0.3, 0.4, 0.0, 0.1, 0.2, 0.3, -0.2, 0.0, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0))
    sim = SimulatedHand("full", time_scale=20.0, start_registers=start).start()
    try:
        said = io.StringIO()
        bridge = LeapBridge()
        with contextlib.redirect_stderr(said):
            code = bridge.open(configuration(sim))
        check(code == rhal.SKR_OK, "this driver opens the simulated hand", said.getvalue())
        target = [lo + (hi - lo) * (j + 1) / 18 for j, (lo, hi) in enumerate(zip(frames.LIMITS_MIN, frames.LIMITS_MAX))]
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=target, mask=[1] * 16)
        until(lambda: bridge.write_command(command, 0) == rhal.SKR_OK, 5)
        want = [frames.position_to_register(v) for v in target]
        check(until(lambda: sim.positions == want, 5), "and commands it")
        bridge.close()

        port, handler = sdk_session(sim.path)
        models = [retried(lambda i=i: handler.ping(port, i))[0] for i in frames.IDS]
        check(models == [1240] * 16, "the SDK pings sixteen servos and every one is a 1240, the model the driver was told to expect")
        spans = {name: (sdk_span(port, handler, *span, False), sdk_span(port, handler, *span, True)) for name, span in (("state", frames.SPAN_STATE), ("gains", frames.SPAN_GAINS), ("motion", frames.SPAN_MOTION))}
        for name, (plain, fast) in spans.items():
            check(plain is not None and fast is not None and plain == fast, f"the SDK's plain and fast synchronised reads of the {name} span return the same bytes from all sixteen servos")
        state, gains, motion = (spans[n][0] for n in ("state", "gains", "motion"))
        check(state is not None and all(state[i][0] == 1 for i in frames.IDS) and all(state[i][frames.ADDR_HARDWARE_ERROR - 64] == 0 for i in frames.IDS), "it reads torque on and no hardware error on every servo, as this driver left them")
        modes = [retried(lambda i=i: handler.read1ByteTxRx(port, i, frames.ADDR_OPERATING_MODE))[0] for i in frames.IDS]
        check(modes == [5] * 16, "and operating mode 5 (current-based position control)")
        expected_p = [round(600 * (0.75 if i in (0, 4, 8) else 1)) for i in frames.IDS]
        expected_d = [round(200 * (0.75 if i in (0, 4, 8) else 1)) for i in frames.IDS]
        check(gains is not None and [frames.u16(gains[i], 4) for i in frames.IDS] == expected_p and [frames.u16(gains[i], 0) for i in frames.IDS] == expected_d and [frames.u16(gains[i], 2) for i in frames.IDS] == [0] * 16, "the P, I and D gains the operator asked for, the side-to-side joints at three quarters of P and D")
        check(gains is not None and [frames.i16(gains[i], frames.ADDR_GOAL_CURRENT - frames.ADDR_POSITION_D) for i in frames.IDS] == [350] * 16, "and the goal current")
        check(motion is not None and [frames.i32(motion[i], 0) for i in frames.IDS] == want, "and every servo's goal position is the register this driver computed from the angle")
        check(motion is not None and [frames.i32(motion[i], frames.ADDR_PRESENT_POSITION - frames.ADDR_GOAL_POSITION) for i in frames.IDS] == want, "and the servos are there")

        section("the two together: the SDK moves the hand, and this driver reads it and does not move it")
        moved = [frames.position_to_register(a) for a in (0.5, 0.0, 0.2, -0.1, 0.0, 0.4, 0.1, 0.0, 0.3, 0.2, 0.1, 0.0, 1.0, 1.2, 0.5, -0.5)]
        writer = dxl.GroupSyncWrite(port, handler, frames.ADDR_GOAL_POSITION, 4)
        for servo, value in zip(frames.IDS, moved):
            writer.addParam(servo, (value & 0xFFFFFFFF).to_bytes(4, "little"))
        check(writer.txPacket() == dxl.COMM_SUCCESS, "the SDK writes a goal to every servo with `GroupSyncWrite`")
        check(until(lambda: sim.positions == moved, 5), "and the servos go there")
        port.closePort()
        sim.goal_writes.clear()
        said = io.StringIO()
        bridge = LeapBridge()
        with contextlib.redirect_stderr(said):
            code = bridge.open(configuration(sim))
        check(code == rhal.SKR_OK and not sim.goal_writes, "this driver opens a hand that holds the SDK's goals, and writes no goal", said.getvalue())
        out = rhal.State()
        check(until(lambda: bridge.read_state(out, 0) == rhal.SKR_OK and not out.flags & rhal.FLAG_DEGRADED, 5), "and reads it")
        check(all(abs(out.position_rad[j] - frames.register_to_position(moved[j])) < 1e-9 for j in range(16)), "at the positions the SDK moved it to")
        bridge.emergency_stop()
        check(until(lambda: json.loads(bridge.extension("vendor.leap.task", b"status")[1])["stop_delivered"], 5), "the stop is written")
        bridge.close()
        port, handler = sdk_session(sim.path)
        goals = retried(lambda: (sdk_span(port, handler, *frames.SPAN_MOTION, False), dxl.COMM_SUCCESS))[0]
        check(goals is not None and [frames.i32(goals[i], 0) for i in frames.IDS] == sim.positions, "and the SDK reads every goal where its servo is: the hold")
        port.closePort()
    finally:
        sim.stop()


def main_maker() -> None:
    section("the hand's authors' own client and utilities, run")
    if not LEAP_API:
        print("  (LEAP_API is not set: that repository's licence is CC BY-NC 4.0, so nothing fetches or runs it unless a person points this at a checkout. The authors' client was not run.)")
        return
    sys.path.insert(0, os.path.join(LEAP_API, "python"))
    try:
        import numpy as np
        from leap_hand_utils import dynamixel_client as client_module
        from leap_hand_utils import leap_hand_utils as utils
    except ImportError as error:
        check(False, "the authors' client imports", str(error))
        return
    low, high = utils.LEAPsim_limits("regular")
    check(tuple(low) == frames.LIMITS_MIN and tuple(high) == frames.LIMITS_MAX, "the joint ranges in `frames.py` are the authors' `LEAPsim_limits`")
    check(abs(float(utils.LEAPsim_to_LEAPhand(np.zeros(1))[0]) - frames.OFFSET_RAD) < 1e-12, "and the offset between the servo's angle and the hand's is the authors' 3.14159")
    truncation = rounding = 0
    rng = random.Random(7)
    for _ in range(2000):
        angle = rng.uniform(-1.3, 2.4)
        theirs = int((angle + 3.14159) / client_module.DEFAULT_POS_SCALE)
        mine = frames.position_to_register(angle)
        if mine != theirs:
            rounding += 1
            if abs(mine - theirs) != 1:
                truncation += 1
    check(truncation == 0, f"a goal differs from the authors' client's (which cuts an angle to a whole tick toward zero) by at most one tick, and by one in {rounding} of 2,000 angles, this driver rounding to the nearest (F305)")
    check(client_module.DEFAULT_CUR_SCALE == 1.34 and frames.CURRENT_MA_PER_UNIT == 1.0, "the authors' client scales a Present_Current unit by 1.34 mA, which is another servo's figure: the e-manual says 1.0 mA for the XC330-M288 and XL330-M288, and this driver reports 1.0 (F306)")
    check(abs(client_module.DEFAULT_VEL_SCALE - frames.VELOCITY_RAD_S_PER_UNIT) < 1e-12, "and the velocity unit, 0.229 rev/min, is the same")
    check(client_module.ADDR_GOAL_POSITION == frames.ADDR_GOAL_POSITION and client_module.ADDR_PRESENT_POSITION == frames.ADDR_PRESENT_POSITION and client_module.ADDR_PRESENT_VELOCITY == frames.ADDR_PRESENT_VELOCITY
          and client_module.ADDR_PRESENT_CURRENT == frames.ADDR_PRESENT_CURRENT and client_module.ADDR_TORQUE_ENABLE == frames.ADDR_TORQUE_ENABLE, "and its registers are the same ones")

    sim = SimulatedHand("full", time_scale=20.0).start()
    try:
        original = client_module.DynamixelClient
        main_module = runpy.run_path(os.path.join(LEAP_API, "python", "main.py"), run_name="not_main")
        node_class = main_module["LeapNode"]

        class Pointed(original):                      # the authors' node opens /dev/ttyUSB0; this opens the simulated hand's pseudo-terminal in its place
            def __init__(self, motor_ids, port="", baudrate=1000000, **kw):
                super().__init__(motor_ids, sim.path, baudrate, **kw)
        main_module["DynamixelClient"] = Pointed
        node_class.__init__.__globals__["DynamixelClient"] = Pointed
        node = node_class()
        check(sim.operating_modes == [5] * 16 and sim.torque == [1] * 16, "the authors' own `LeapNode` brings the simulated hand up: current-based position control, torque on")
        check(sim.goal_currents == [node.curr_lim] * 16 and sim.gains[1][2] == node.kP and sim.gains[0][2] == int(node.kP * 0.75), "with its gains and current limit (three quarters of P and D on the side-to-side joints)")
        node.set_allegro(np.full(16, 0.3))
        check(until(lambda: all(abs(p - 0.3) < 0.01 for p in (np.array(node.read_pos()) - 3.14159)), 5), "and moves it, and reads where it went, with `fastSyncRead`")
        check(any(i == frames.INST_FAST_SYNC_READ for i, *_ in sim.requests), "(the reads were Fast Sync Reads, so the simulated hand answers the instruction the authors' client uses)")
        positions = list(sim.positions)
        node.dxl_client.disconnect()
        said = io.StringIO()
        bridge = LeapBridge()
        with contextlib.redirect_stderr(said):
            code = bridge.open(configuration(sim, current_ma=node.curr_lim, gains={"kp": node.kP, "ki": node.kI, "kd": node.kD}))
        out = rhal.State()
        check(code == rhal.SKR_OK and until(lambda: bridge.read_state(out, 0) == rhal.SKR_OK, 5), "this driver opens the hand the authors' client left, and reads it", said.getvalue())
        check(all(abs(out.position_rad[j] - frames.register_to_position(positions[j])) < 1e-9 for j in range(16)), "at the positions the authors' client moved it to")
        bridge.close()
    finally:
        sim.stop()


def main() -> int:
    main_wire()
    main_table()
    main_live()
    main_maker()
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAILED: {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

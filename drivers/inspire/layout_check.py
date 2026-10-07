"""This driver's bytes, held to the maker's worked frames and to two independent transcriptions of the manuals (DRY-COVERAGE-PLAN.md item 12.E.8).

    PYTHONPATH=../../bindings/python:. python3 layout_check.py

Needs `pyrh56==0.3.0` (module `rh56_sdk`, MIT; a third party's transcription of the series manual V1.08, with a serial driver) and `inspire-rh56dftp==0.1.0` (module `inspire_rh56dftp`; a third party's Modbus TCP
client written from the DFTP manual) importable, and `pyserial` for the first's serial transport. Neither is the maker's, and neither is a hand: what is compared is **what each of three readings of the same manuals
makes of the same facts**, and a disagreement is either a transcription error in one of them or a place where the manuals are not clear, and is pinned below rather than resolved by taste.

What it checks, and what it cannot:

* every frame this driver builds is byte-for-byte the frame `pyrh56` builds for the same request (500 random reads, 500 random batch writes, 200 random one-byte writes), and the manual's four worked frames, which
  `selftest.py` already holds as literals, are built by both;
* every answer `pyrh56`'s mock transport synthesises parses here, and every answer this driver's simulated hand writes parses in `pyrh56`'s strict parser;
* the register map, status table, error bits and joint order agree with both packages;
* `pyrh56`'s own driver, opened on the simulated hand's pseudo-terminal, reads it, moves it, sets its speed and force and clears its errors, and this driver then reads the same hand and agrees.

It cannot show that a real hand answers any of this. The simulated hand was written from the same manuals as the driver (F15).
"""

from __future__ import annotations

import importlib.metadata
import random
import sys
import time

CHECKS = 0
FAILURES: list[str] = []


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


def main() -> int:
    try:
        import rh56_sdk
        import inspire_rh56dftp
        from rh56_sdk import registers as p_registers
        from rh56_sdk.constants import SERVO_COUNT
        from rh56_sdk.driver import RH56Driver
        from rh56_sdk.enums import ErrorFlag, Finger, StatusCode
        from rh56_sdk.protocol import RH56Protocol
        from rh56_sdk.transport import SerialTransport
        from inspire_rh56dftp import registers as d_registers
    except ImportError as error:
        print(f"layout_check: {error}. This check imports the two independent transcriptions of the manuals and refuses to run without them: `pip install pyrh56==0.3.0 pyserial inspire-rh56dftp==0.1.0`, "
              "and run it with that interpreter (PYRH56_PYTHON in the Makefile). Nothing was compared.", file=sys.stderr)
        return 2

    from skynet_inspire import frames
    from skynet_inspire.hand_sim import SimulatedHand
    from skynet_inspire.bridge import InspireBridge
    import contextlib
    import io
    import json
    import skynet_rhal as rhal

    section("the packages, as installed")
    versions = {name: importlib.metadata.version(name) for name in ("pyrh56", "inspire-rh56dftp", "pyserial")}
    check(versions["pyrh56"] == "0.3.0" and versions["inspire-rh56dftp"] == "0.1.0", f"pyrh56 0.3.0 and inspire-rh56dftp 0.1.0, the versions this check was written against ({versions})")
    del rh56_sdk

    section("the register map: this driver, pyrh56 and the DFTP client")
    pairs = [("ANGLE_SET", frames.ADDR_ANGLE_SET, p_registers.REG_ANGLE_SET), ("FORCE_SET", frames.ADDR_FORCE_SET, p_registers.REG_FORCE_SET), ("SPEED_SET", frames.ADDR_SPEED_SET, p_registers.REG_SPEED_SET),
             ("ANGLE_ACT", frames.ADDR_ANGLE_ACT, p_registers.REG_ANGLE_ACT), ("FORCE_ACT", frames.ADDR_FORCE_ACT, p_registers.REG_FORCE_ACT), ("CURRENT", frames.ADDR_CURRENT, p_registers.REG_CURRENT),
             ("ERROR", frames.ADDR_ERROR, p_registers.REG_ERROR), ("STATUS", frames.ADDR_STATUS, p_registers.REG_STATUS), ("TEMP", frames.ADDR_TEMP, p_registers.REG_TEMP),
             ("CLEAR_ERROR", frames.ADDR_CLEAR_ERROR, p_registers.REG_CLEAR_ERROR), ("GESTURE_FORCE_CLB", frames.ADDR_FORCE_CALIBRATION, p_registers.REG_GESTURE_FORCE_CLB)]
    for name, ours, theirs in pairs:
        check(ours == theirs, f"{name}: this driver {ours}, pyrh56 {theirs}")
    check((frames.ADDR_ANGLE_SET, frames.ADDR_ANGLE_ACT, frames.ADDR_FORCE_ACT, frames.ADDR_SPEED_SET) == (d_registers.ANGLE_SET, d_registers.ANGLE_ACT, d_registers.FORCE_ACT, d_registers.SPEED_SET),
          "the DFTP client's four registers (ANGLE_SET, ANGLE_ACT, FORCE_ACT, SPEED_SET) are this driver's, in its Modbus register numbering, which the manual says is the byte address")
    check(list(frames.JOINTS) == list(d_registers.DOF_ORDER), "the joints are the DFTP client's DOF_ORDER, name for name and in the same order")
    check([int(f) for f in Finger] == [0, 1, 2, 3, 4, 5] and [f.name.lower() for f in Finger][:4] == ["pinky", "ring", "middle", "index"] and Finger.THUMB_FLEX == 4 and Finger.THUMB_ROTATION == 5,
          "and pyrh56's finger order is the same six")
    check(set(frames.STATUS_NAMES) == {int(c) for c in StatusCode} == set(p_registers.STATUS_TEXT), "the status codes are pyrh56's: 0 to 3 and 5 to 7")
    check(set(frames.ERROR_BITS) == set(p_registers.ERROR_BITS) and [1 << b for b in sorted(frames.ERROR_BITS)] == [int(f) for f in ErrorFlag if f], "the error bits are pyrh56's: 0 to 4")

    section("what the three readings disagree about (pinned, not resolved)")
    check(inspire_rh56dftp.DEFAULT_IP == "192.168.123.211" and inspire_rh56dftp.DEFAULT_PORT == 6000,
          f"the DFTP client's default address is {inspire_rh56dftp.DEFAULT_IP}:{inspire_rh56dftp.DEFAULT_PORT}; the DFTP manual §1.3.2 says 192.168.11.210, port 6000 (F296: the driver reaches nothing by IP and takes no default address)")
    check(p_registers.REG_GESTURE_NO_SET == 1008, "pyrh56 names register 1008 GESTURE_NO_SET; the series manual calls it \"reserve\" (this driver never writes it)")
    from rh56_sdk import constants as p_constants
    check(p_constants.RH56_THUMB_FLEX_CLOSE_LIMIT == 200 and p_constants.RH56_THUMB_FLEX_OPEN_LIMIT == 700,
          "pyrh56 limits the thumb bend to registers 200 to 700 by its own policy, which neither manual states: this driver limits to the manual's range and the operator's narrowing (F297)")

    section("requests: this driver's bytes against pyrh56's builders")
    rng = random.Random(1)
    equal = True
    for _ in range(500):
        hand, address, length = rng.randint(1, 254), rng.randint(0, 65535), rng.randint(1, 255)
        equal = equal and frames.request_read(hand, address, length) == RH56Protocol.build_read_reg16(hand, address, length)
    check(equal, "500 random reads (hand, address, length) are byte-for-byte pyrh56's")
    equal = True
    for _ in range(500):
        hand, address = rng.randint(1, 254), rng.randint(0, 65000)
        values = [rng.randint(0, 1000) for _ in range(rng.randint(1, 20))]
        equal = equal and frames.request_write_shorts(hand, address, values) == RH56Protocol.build_reg16_frame(hand, address, values)
    check(equal, "500 random batch writes of 1 to 20 registers are byte-for-byte pyrh56's")
    check(frames.request_write_shorts(1, 1486, [-1, 0, 1000, 5, 6, 7]) == RH56Protocol.build_reg16_frame(1, 1486, [0xFFFF, 0, 1000, 5, 6, 7]), "a -1 is the sixteen-bit 0xFFFF pyrh56 would send for it")
    equal = True
    for _ in range(200):
        hand, address = rng.randint(1, 254), rng.randint(0, 65535)
        data = bytes(rng.randint(0, 255) for _ in range(rng.randint(1, 252)))
        equal = equal and frames.request_write(hand, address, data) == RH56Protocol.build_write_bytes_frame(hand, address, data)
    check(equal, "200 random byte-register writes of 1 to 252 bytes are byte-for-byte pyrh56's")
    check(frames.request_write(1, frames.ADDR_CLEAR_ERROR, b"\x01") == RH56Protocol.build_write_bytes_frame(1, 1004, b"\x01"), "and CLEAR_ERROR's frame is pyrh56's")
    check(frames.request_write(1, frames.ADDR_FORCE_CALIBRATION, b"\x01") == RH56Protocol.build_write_bytes_frame(1, 1009, b"\x01"), "and so is the force calibration's")
    check(RH56Protocol.build_read_reg16(1, 1546, 12) == bytes.fromhex("EB90010411 0A060C32".replace(" ", "")) and RH56Protocol.build_reg16_frame(1, 1486, [100, 100, 100, 100, 1000, 0]) == bytes.fromhex("EB90010F12CE056400640064006400E803000070"),
          "pyrh56 builds the manual's two worked requests too")
    check(RH56Protocol.checksum(bytes.fromhex("010F110A06")) == 49 and frames.checksum(bytes.fromhex("010F110A06")) == 49, "both sum the bytes after the header and keep the low eight bits")

    section("answers: each reading's answers through the other's parser")
    equal = True
    for _ in range(300):
        hand, address, length = rng.randint(1, 254), rng.randint(0, 60000), rng.choice([6, 12, 18, 48])
        request = RH56Protocol.build_read_reg16(hand, address, length)
        mock = SerialTransport("COM_MOCK")
        mock.connect()
        answer = mock.request(request)
        equal = equal and frames.parse_response(answer, hand, frames.CMD_READ, address, length) == b"\x00" * length
    check(equal, "300 answers pyrh56's mock transport synthesises parse here, to the zeros it puts in them")
    mock = SerialTransport("COM_MOCK")
    mock.connect()
    check(frames.parse_response(mock.request(RH56Protocol.build_reg16_frame(3, 1486, [1, 2, 3, 4, 5, 6])), 3, frames.CMD_WRITE, 1486) == b"", "and so does its write acknowledgement")
    hand_sim = SimulatedHand(time_scale=20.0).start()
    try:
        hand_sim.place([111, 222, 333, 444, 555, 666])
        hand_sim.set_fault(2, error=0b101)
        transport = SerialTransport(hand_sim.path, 115200, 0.5)
        transport.connect()
        data = RH56Protocol.parse_read_response(transport.request(RH56Protocol.build_read_reg16(1, frames.ADDR_ANGLE_ACT, 12), 0.5), 1, frames.ADDR_ANGLE_ACT, 12)
        check(RH56Protocol.parse_u16_le(data) == [111, 222, 333, 444, 555, 666], "an answer this driver's simulated hand writes passes pyrh56's strict parser, with the angles in it")
        data = RH56Protocol.parse_read_response(transport.request(RH56Protocol.build_read_reg16(1, frames.ADDR_ERROR, 12), 0.5), 1, frames.ADDR_ERROR, 12)
        check(list(data[:6]) == [0, 0, 0b101, 0, 0, 0] and list(data[6:]) == [2, 2, 6, 2, 2, 2], "and so do the error and status bytes, six and six, in the order the manual gives")
        check(RH56Protocol.parse_write_ack(transport.request(RH56Protocol.build_reg16_frame(1, frames.ADDR_SPEED_SET, [123] * 6), 0.5), 1, frames.ADDR_SPEED_SET) is True and hand_sim.speeds == [123] * 6,
              "and its write acknowledgement passes the strict parser, and the write was taken")
        transport.disconnect()

        section("pyrh56's own driver, on the simulated hand")
        hand_sim.heal()
        hand_sim.place([1000] * 6)
        hand = RH56Driver(hand_sim.path, 115200, 1, 0.5)
        hand.connect()
        check(hand.read_angle() == [1000] * 6 and hand.read_status() == [2] * 6 and hand.read_error() == [0] * 6, "it connects, and reads the angles, the statuses and the errors the hand holds")
        check(hand.read_temperature() == [31] * 6 and len(hand.read_current()) == 6 and hand.read_force() == [0] * 6, "and the temperature, the current and the force")
        hand.set_speed([400] * 6)
        hand.set_force_threshold([250] * 6)
        check(hand_sim.speeds == [400] * 6 and hand_sim.force_limits == [250] * 6, "it sets the speed and the force limit, and the hand holds them")
        target = [900, 800, 700, 600, 500, 400]
        hand.move_to(target)
        check(hand_sim.targets == target, "it writes a frame of six targets, and the hand's record of them is the frame")
        check(until(lambda: hand_sim.angles == target, 3), "and the fingers go there")
        hand_sim.set_fault(1, error=0b1)
        check(hand.read_error()[1] == 1 and hand.read_status()[1] == 6, "it reads an error the hand raises")
        try:
            hand.clear_error(verify=True)
            cleared = hand_sim.errors == [0] * 6
        except Exception:  # noqa: BLE001 - what is compared is the hand's state
            cleared = False
        check(cleared, "and its clear_error clears it")
        hand.disconnect()

        section("this driver, on the hand pyrh56 just moved")
        bridge = InspireBridge()
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.open(json.dumps({"device": hand_sim.path, "protocol": "private", "model": "RH56", "stop_mode": "hold", "speed": 0.5, "force_g": 300, "simulator": True}))
        check(code == rhal.SKR_OK, "this driver opens the same hand", said.getvalue())
        state = rhal.State()
        until(lambda: bridge.read_state(state, 0) == rhal.SKR_OK)
        angles = [round(frames.angle_to_register(state.position_rad[j], j, frames.PROFILES["RH56"]) or -1) for j in range(6)]
        check(angles == target, f"and reads the angles pyrh56 left it at (registers {angles})")
        check(hand_sim.speeds == [500] * 6 and hand_sim.force_limits == [300] * 6, "and tells it the operator's speed and force, over what pyrh56 had set")
        bridge.close()
    finally:
        hand_sim.stop()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())

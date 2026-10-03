"""The emulated Dynamixel bus, held to ROBOTIS's own SDK — item 12.D / 12.E.5.

Every assertion here is made by `dynamixel_sdk`, the maker's Python library, talking to the pseudo-terminal exactly as it talks to a serial port: if
the framing, the byte stuffing or the CRC were wrong in the emulator, the library would call the packet corrupt, and that is a failed check rather than a
quiet one. The CRC and the stuffing are also compared with the SDK's own functions directly, so a defect in both ends that cancelled would still show. Then the
same bus is read through LeRobot's own `DynamixelMotorsBus`, the layer the bridge's robot class sits on.

    ~/lerobot-venv/bin/python drivers/lerobot/sim/test_dynamixel_bus.py

Needs `lerobot`, `dynamixel-sdk` and `pyserial`: `lerobot[dynamixel]`, which `check-wire-venv` installs.
"""

from __future__ import annotations

import os
import random
import sys
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import dynamixel_sdk as dxl  # noqa: E402

from dynamixel_bus import ALERT, DynamixelBus, HW_OVERLOAD, crc16, stuff, unstuff  # noqa: E402

# Addresses of the X-series control table, which `dynamixel_bus` reads from LeRobot's own tables; they are written out here, from ROBOTIS's e-manual, so that
# a table the emulator mis-read would disagree with this file rather than agree with itself.
ID, OPERATING_MODE, HOMING_OFFSET, MIN_LIMIT, MAX_LIMIT, TORQUE, HW_ERROR, GOAL_POSITION, PRESENT_POSITION = 7, 11, 20, 52, 48, 64, 70, 116, 132
XL430_W250, XL330_M288 = 1060, 1200


def wait_until(predicate, timeout: float = 3.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.01)
    return predicate()


class FramingAgainstTheSDK(unittest.TestCase):
    """The two functions the packet format rests on, compared with the SDK's own."""

    def setUp(self) -> None:
        self.handler = dxl.PacketHandler(2.0)

    def test_the_crc_is_the_sdks_over_random_buffers_and_the_headers_pattern(self) -> None:
        generator = random.Random(2)
        for size in (1, 2, 7, 8, 33, 120):
            data = bytes(generator.randrange(256) for _ in range(size))
            self.assertEqual(crc16(data), self.handler.updateCRC(0, list(data), len(data)), f"{size} bytes")
        header = bytes([0xFF, 0xFF, 0xFD, 0x00, 0x01, 0x03, 0x00, 0x01])
        self.assertEqual(crc16(header), self.handler.updateCRC(0, list(header), len(header)))

    @staticmethod
    def packet(params: bytes) -> list[int]:
        """An instruction packet as the SDK holds one before it stuffs it: header, id, length, instruction, parameters, two CRC bytes."""
        length = len(params) + 3
        return list(b"\xff\xff\xfd\x00\x01" + length.to_bytes(2, "little") + b"\x03" + params + b"\x00\x00")

    def test_stuffing_is_the_sdks_both_ways(self) -> None:
        for params in (b"", b"\x01\x02", b"\xff\xff\xfd", b"\x00\xff\xff\xfd\x00", b"\xff\xff\xfd\xff\xff\xfd", b"\xff\xff\xfd\xfd"):
            theirs = self.handler.addStuffing(self.packet(params))
            wanted = stuff(params)
            self.assertEqual(bytes(theirs[8:8 + len(wanted)]), wanted, f"the SDK stuffs {params!r} as this does")
            self.assertEqual(theirs[5] | (theirs[6] << 8), len(params) + 3 + len(wanted) - len(params), "and counts the stuffed bytes in the length")
            self.assertEqual(unstuff(wanted), params, f"and this removes it again: {params!r}")
            # The SDK removing what this stuffed: a packet whose length counts the stuffing.
            stuffed = list(b"\xff\xff\xfd\x00\x01" + (len(wanted) + 3).to_bytes(2, "little") + b"\x03" + wanted + b"\x00\x00")
            removed = self.handler.removeStuffing(stuffed)
            self.assertEqual(bytes(removed[8:8 + len(params)]), params, f"the SDK removes this's stuffing from {params!r}")


class SDKBus(unittest.TestCase):
    def setUp(self) -> None:
        self.bus = DynamixelBus()
        self.path = self.bus.start()
        self.port = dxl.PortHandler(self.path)
        self.assertTrue(self.port.openPort())
        self.port.setBaudRate(1_000_000)
        self.packets = dxl.PacketHandler(2.0)

    def tearDown(self) -> None:
        self.port.closePort()
        self.bus.stop()

    def read4(self, ident: int, address: int) -> int:
        value, result, error = self.packets.read4ByteTxRx(self.port, ident, address)
        self.assertEqual((result, error), (dxl.COMM_SUCCESS, 0))
        return value - (1 << 32) if value & (1 << 31) else value

    def test_every_servo_answers_a_ping_with_its_own_models_number(self) -> None:
        expected = {1: XL430_W250, 2: XL430_W250, 3: XL330_M288, 4: XL330_M288, 5: XL330_M288, 6: XL330_M288}
        for ident, model in expected.items():
            number, result, error = self.packets.ping(self.port, ident)
            self.assertEqual((result, error, number), (dxl.COMM_SUCCESS, 0, model), f"servo {ident}")

    def test_a_broadcast_ping_is_answered_by_every_servo_in_id_order(self) -> None:
        found, result = self.packets.broadcastPing(self.port)
        self.assertEqual(result, dxl.COMM_SUCCESS)
        self.assertEqual({ident: data[0] for ident, data in found.items()},
                         {1: XL430_W250, 2: XL430_W250, 3: XL330_M288, 4: XL330_M288, 5: XL330_M288, 6: XL330_M288})

    def test_an_id_nobody_has_is_a_timeout_not_an_answer(self) -> None:
        _, result, _ = self.packets.ping(self.port, 9)
        self.assertEqual(result, dxl.COMM_RX_TIMEOUT)

    def test_a_goal_in_extended_position_mode_is_followed_past_one_turn(self) -> None:
        self.packets.write1ByteTxRx(self.port, 3, OPERATING_MODE, 4)
        self.assertEqual(self.packets.write1ByteTxRx(self.port, 3, TORQUE, 1), (dxl.COMM_SUCCESS, 0))
        self.packets.write4ByteTxRx(self.port, 3, GOAL_POSITION, 6000)
        self.assertTrue(wait_until(lambda: abs(self.read4(3, PRESENT_POSITION) - 6000) <= 3, 6.0), "a goal beyond 4095 is reached, because extended mode does not wrap")

    def test_a_goal_in_position_mode_stays_inside_the_travel_limits(self) -> None:
        self.bus.calibrate(1, homing_offset=0, range_min=1000, range_max=3000)
        self.packets.write1ByteTxRx(self.port, 1, TORQUE, 1)
        self.packets.write4ByteTxRx(self.port, 1, GOAL_POSITION, 3900)
        self.assertTrue(wait_until(lambda: abs(self.read4(1, PRESENT_POSITION) - 3000) <= 3, 6.0))
        time.sleep(0.3)
        self.assertLessEqual(self.read4(1, PRESENT_POSITION), 3003)

    def test_a_goal_with_torque_off_goes_nowhere(self) -> None:
        before = self.read4(2, PRESENT_POSITION)
        self.packets.write4ByteTxRx(self.port, 2, GOAL_POSITION, before + 700)
        time.sleep(0.3)
        self.assertEqual(self.read4(2, PRESENT_POSITION), before)

    def test_eeprom_cannot_be_written_while_torque_is_on_and_says_access_error(self) -> None:
        self.packets.write1ByteTxRx(self.port, 4, TORQUE, 1)
        result, error = self.packets.write4ByteTxRx(self.port, 4, MIN_LIMIT, 123)
        self.assertEqual((result, error), (dxl.COMM_SUCCESS, 7))
        self.assertIn("not available to target address", self.packets.getRxPacketError(error).lower(), "the maker's own words for error 7")
        self.packets.write1ByteTxRx(self.port, 4, TORQUE, 0)
        self.assertEqual(self.packets.write4ByteTxRx(self.port, 4, MIN_LIMIT, 123), (dxl.COMM_SUCCESS, 0))
        self.assertEqual(self.read4(4, MIN_LIMIT), 123)

    def test_a_register_that_is_read_only_refuses_a_write(self) -> None:
        self.assertEqual(self.packets.write4ByteTxRx(self.port, 5, PRESENT_POSITION, 1)[1], 7)
        self.assertEqual(self.packets.write2ByteTxRx(self.port, 5, 0, 1)[1], 7, "the model number is the maker's")

    def test_an_operating_mode_the_model_does_not_have_is_a_data_range_error(self) -> None:
        self.assertEqual(self.packets.write1ByteTxRx(self.port, 1, OPERATING_MODE, 5)[1], 4, "an XL430 has no current-based position mode")
        self.assertEqual(self.packets.write1ByteTxRx(self.port, 6, OPERATING_MODE, 5), (dxl.COMM_SUCCESS, 0), "an XL330 does")

    def test_the_homing_offset_is_added_to_what_is_reported(self) -> None:
        raw = self.read4(4, PRESENT_POSITION)
        self.packets.write4ByteTxRx(self.port, 4, HOMING_OFFSET, 100)
        self.assertEqual(self.read4(4, PRESENT_POSITION), raw + 100)

    def test_a_negative_homing_offset_is_twos_complement(self) -> None:
        raw = self.read4(4, PRESENT_POSITION)
        self.packets.write4ByteTxRx(self.port, 4, HOMING_OFFSET, (-100) & 0xFFFFFFFF)
        self.assertEqual(self.read4(4, HOMING_OFFSET), -100)
        self.assertEqual(self.read4(4, PRESENT_POSITION), raw - 100)

    def test_a_reply_whose_parameters_hold_the_header_pattern_is_stuffed_and_the_sdk_unstuffs_it(self) -> None:
        # Homing_Offset 0x00FDFFFF is the bytes FF FF FD 00, the pattern a header starts with.
        self.packets.write4ByteTxRx(self.port, 1, HOMING_OFFSET, 0x00FDFFFF)
        self.assertEqual(self.read4(1, HOMING_OFFSET), 0x00FDFFFF)
        self.assertGreater(self.bus.bytes_stuffed, 0, "the emulator stuffed a byte the SDK had to remove")
        self.assertEqual(self.bus.servo(1).peek("Homing_Offset"), 0x00FDFFFF, "and the SDK stuffed the write, which the emulator removed")

    def test_a_reply_with_a_bad_crc_is_called_corrupt_by_the_makers_library(self) -> None:
        self.bus.corrupt(1)
        _, result, _ = self.packets.ping(self.port, 1)
        self.assertEqual(result, dxl.COMM_RX_CORRUPT)
        _, result, _ = self.packets.ping(self.port, 1)
        self.assertEqual(result, dxl.COMM_SUCCESS)

    def test_a_servo_that_goes_silent_times_out_and_the_others_still_answer(self) -> None:
        self.bus.silence(5)
        self.assertEqual(self.packets.ping(self.port, 5)[1], dxl.COMM_RX_TIMEOUT)
        self.assertEqual(self.packets.ping(self.port, 6)[1], dxl.COMM_SUCCESS)

    def test_a_hardware_error_raises_the_alert_flag_and_the_shutdown_register_takes_the_torque_off(self) -> None:
        self.packets.write1ByteTxRx(self.port, 1, TORQUE, 1)
        self.bus.fault(1, HW_OVERLOAD)
        _, result, error = self.packets.ping(self.port, 1)
        self.assertEqual((result, error & ALERT), (dxl.COMM_SUCCESS, ALERT))
        self.assertIn("hardware", self.packets.getRxPacketError(error).lower())
        self.assertEqual(self.packets.read1ByteTxRx(self.port, 1, HW_ERROR)[0], HW_OVERLOAD)
        self.assertEqual(self.packets.read1ByteTxRx(self.port, 1, TORQUE)[0], 0, "overload is in the factory Shutdown mask, so the servo let go")
        self.assertEqual(self.packets.ping(self.port, 2)[2] & ALERT, 0, "and only that servo")

    def test_a_reboot_clears_the_error_and_the_torque(self) -> None:
        self.bus.fault(2, HW_OVERLOAD)
        self.packets.reboot(self.port, 2)
        self.assertEqual(self.packets.read1ByteTxRx(self.port, 2, HW_ERROR)[0], 0)
        self.assertEqual(self.packets.ping(self.port, 2)[2] & ALERT, 0)

    def test_sync_write_moves_several_servos_and_sync_read_reads_them_back(self) -> None:
        for ident in (1, 2, 3):
            self.packets.write1ByteTxRx(self.port, ident, OPERATING_MODE, 4)
            self.packets.write1ByteTxRx(self.port, ident, TORQUE, 1)
        writer = dxl.GroupSyncWrite(self.port, self.packets, GOAL_POSITION, 4)
        goals = {1: 1800, 2: 2200, 3: 2600}
        for ident, goal in goals.items():
            writer.addParam(ident, [dxl.DXL_LOBYTE(dxl.DXL_LOWORD(goal)), dxl.DXL_HIBYTE(dxl.DXL_LOWORD(goal)),
                                    dxl.DXL_LOBYTE(dxl.DXL_HIWORD(goal)), dxl.DXL_HIBYTE(dxl.DXL_HIWORD(goal))])
        self.assertEqual(writer.txPacket(), dxl.COMM_SUCCESS)
        reader = dxl.GroupSyncRead(self.port, self.packets, PRESENT_POSITION, 4)
        for ident in goals:
            reader.addParam(ident)

        def arrived() -> bool:
            if reader.txRxPacket() != dxl.COMM_SUCCESS:
                return False
            return all(reader.isAvailable(ident, PRESENT_POSITION, 4) and abs(reader.getData(ident, PRESENT_POSITION, 4) - goal) <= 3 for ident, goal in goals.items())

        self.assertTrue(wait_until(arrived, 6.0))

    def test_a_stalled_servo_answers_and_does_not_move(self) -> None:
        self.bus.stall(1)
        self.packets.write1ByteTxRx(self.port, 1, TORQUE, 1)
        before = self.read4(1, PRESENT_POSITION)
        self.packets.write4ByteTxRx(self.port, 1, GOAL_POSITION, before + 500)
        time.sleep(0.4)
        self.assertEqual(self.read4(1, PRESENT_POSITION), before)

    def test_a_pushed_shaft_with_torque_off_stays_pushed(self) -> None:
        before = self.read4(6, PRESENT_POSITION)
        self.bus.push(6, 200)
        self.assertEqual(self.read4(6, PRESENT_POSITION), (before + 200) % 4096)

    def test_a_status_return_level_of_zero_silences_everything_but_a_ping(self) -> None:
        self.packets.write1ByteTxRx(self.port, 3, 68, 0)  # the last answered write: a level of 0 stops the acknowledgements
        self.assertEqual(self.packets.write1ByteTxRx(self.port, 3, 65, 1)[0], dxl.COMM_RX_TIMEOUT, "a write is no longer acknowledged")
        self.assertEqual(self.packets.ping(self.port, 3)[1], dxl.COMM_SUCCESS, "a ping always is")
        self.assertEqual(self.bus.servo(3).regs[65], 1, "and it was carried out")


class LeRobotBus(unittest.TestCase):
    """The same bus through LeRobot's own `DynamixelMotorsBus`."""

    def setUp(self) -> None:
        from lerobot.motors import Motor, MotorNormMode
        from lerobot.motors.dynamixel import DynamixelMotorsBus

        self.bus = DynamixelBus()
        self.path = self.bus.start()
        models = ("xl430-w250", "xl430-w250", "xl330-m288", "xl330-m288", "xl330-m288", "xl330-m288")
        names = ("shoulder_pan", "shoulder_lift", "elbow_flex", "wrist_flex", "wrist_roll", "gripper")
        self.motors = {n: Motor(i + 1, models[i], MotorNormMode.DEGREES) for i, n in enumerate(names)}
        self.lerobot = DynamixelMotorsBus(port=self.path, motors=self.motors)

    def tearDown(self) -> None:
        if self.lerobot.is_connected:
            self.lerobot.disconnect(disable_torque=False)
        self.bus.stop()

    def test_the_handshake_finds_six_motors_whose_models_are_the_ones_expected(self) -> None:
        self.lerobot.connect()
        self.assertTrue(self.lerobot.is_connected)

    def test_a_motor_that_does_not_answer_is_named_by_the_makers_own_error(self) -> None:
        self.bus.silence(4)
        with self.assertRaises(Exception) as caught:
            self.lerobot.connect()
        message = str(caught.exception)
        self.assertIn("Missing motor IDs", message)
        self.assertIn("4 (expected model: 1200)", message)

    def test_configure_motors_lowers_the_return_delay_as_lerobot_says(self) -> None:
        self.lerobot.connect()
        self.lerobot.disable_torque()
        self.lerobot.configure_motors()
        for ident in range(1, 7):
            self.assertEqual(self.bus.servo(ident).peek("Return_Delay_Time"), 0)

    def test_a_calibration_written_is_the_calibration_read(self) -> None:
        from lerobot.motors import MotorCalibration

        self.lerobot.connect()
        self.lerobot.disable_torque()
        calibration = {n: MotorCalibration(id=m.id, drive_mode=0, homing_offset=(-1) ** m.id * 40 * m.id, range_min=500 + m.id, range_max=3500 - m.id)
                       for n, m in self.motors.items()}
        self.lerobot.write_calibration(calibration)
        self.assertEqual(self.lerobot.read_calibration(), calibration)
        self.assertTrue(self.lerobot.is_calibrated)

    def test_a_sync_write_of_a_goal_and_a_sync_read_of_the_position_agree(self) -> None:
        from lerobot.motors import MotorCalibration

        self.lerobot.connect()
        self.lerobot.write_calibration({n: MotorCalibration(id=m.id, drive_mode=0, homing_offset=0, range_min=500, range_max=3500) for n, m in self.motors.items()})
        self.lerobot.enable_torque()
        self.lerobot.sync_write("Goal_Position", {"shoulder_pan": 20.0}, normalize=True)
        self.assertTrue(wait_until(lambda: abs(self.lerobot.sync_read("Present_Position", ["shoulder_pan"])["shoulder_pan"] - 20.0) < 0.5, 6.0))


if __name__ == "__main__":
    unittest.main(verbosity=1)

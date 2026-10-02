"""The emulated Feetech bus, held to Feetech's own SDK — item 12.D / 12.E.5.

Every assertion here is made by `scservo_sdk`, the maker's Python library, talking to the pseudo-terminal exactly as it talks to a
serial port: if the framing or the checksum were wrong in the emulator, the library would call the packet corrupt, and that is a
failed check rather than a quiet one. Then the same bus is read through LeRobot's own `FeetechMotorsBus`, which is the layer the
bridge's robot class sits on.

    ~/lerobot-venv/bin/python drivers/lerobot/sim/test_feetech_bus.py

Needs `lerobot`, `feetech-servo-sdk` (`scservo_sdk`) and `pyserial`, which `~/lerobot-venv` has.
"""

from __future__ import annotations

import os
import sys
import time
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import scservo_sdk as scs  # noqa: E402

from feetech_bus import FeetechBus  # noqa: E402


def wait_until(predicate, timeout: float = 3.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.01)
    return predicate()


class SDKBus(unittest.TestCase):
    def setUp(self) -> None:
        self.bus = FeetechBus()
        self.path = self.bus.start()
        self.port = scs.PortHandler(self.path)
        self.assertTrue(self.port.openPort())
        self.port.setBaudRate(1_000_000)
        self.packets = scs.PacketHandler(0)

    def tearDown(self) -> None:
        self.port.closePort()
        self.bus.stop()

    def test_every_servo_answers_a_ping_with_the_models_number(self) -> None:
        for ident in range(1, 7):
            model, result, error = self.packets.ping(self.port, ident)
            self.assertEqual((result, error, model), (scs.COMM_SUCCESS, 0, 777), f"servo {ident}")

    def test_an_id_nobody_has_is_a_timeout_not_an_answer(self) -> None:
        _, result, _ = self.packets.ping(self.port, 9)
        self.assertEqual(result, scs.COMM_RX_TIMEOUT)

    def test_a_goal_is_followed_and_the_present_position_reports_it(self) -> None:
        self.assertEqual(self.packets.write1ByteTxRx(self.port, 1, 40, 1), (scs.COMM_SUCCESS, 0))  # Torque_Enable
        self.packets.write2ByteTxRx(self.port, 1, 42, 2500)  # Goal_Position
        self.assertTrue(wait_until(lambda: abs(self.packets.read2ByteTxRx(self.port, 1, 56)[0] - 2500) <= 2))

    def test_a_goal_with_torque_off_goes_nowhere(self) -> None:
        before = self.packets.read2ByteTxRx(self.port, 1, 56)[0]
        self.packets.write2ByteTxRx(self.port, 1, 42, before + 700)
        time.sleep(0.3)
        self.assertEqual(self.packets.read2ByteTxRx(self.port, 1, 56)[0], before)

    def test_the_servo_cannot_be_asked_past_its_travel(self) -> None:
        self.bus.calibrate(2, homing_offset=0, range_min=1000, range_max=3000)
        self.packets.write1ByteTxRx(self.port, 2, 55, 0)  # Lock off, as LeRobot does before it writes anything
        self.packets.write1ByteTxRx(self.port, 2, 40, 1)
        self.packets.write2ByteTxRx(self.port, 2, 42, 3900)
        self.assertTrue(wait_until(lambda: abs(self.packets.read2ByteTxRx(self.port, 2, 56)[0] - 3000) <= 2))
        time.sleep(0.3)
        self.assertLessEqual(self.packets.read2ByteTxRx(self.port, 2, 56)[0], 3002)

    def test_eeprom_is_write_protected_while_lock_is_set_and_acknowledged_anyway(self) -> None:
        self.packets.write1ByteTxRx(self.port, 3, 55, 1)  # Lock on
        result, error = self.packets.write2ByteTxRx(self.port, 3, 9, 123)  # Min_Position_Limit
        self.assertEqual((result, error), (scs.COMM_SUCCESS, 0))
        self.assertEqual(self.packets.read2ByteTxRx(self.port, 3, 9)[0], 0)
        self.packets.write1ByteTxRx(self.port, 3, 55, 0)  # Lock off
        self.packets.write2ByteTxRx(self.port, 3, 9, 123)
        self.assertEqual(self.packets.read2ByteTxRx(self.port, 3, 9)[0], 123)

    def test_the_homing_offset_is_subtracted_from_what_is_reported(self) -> None:
        self.packets.write1ByteTxRx(self.port, 4, 55, 0)
        raw = self.packets.read2ByteTxRx(self.port, 4, 56)[0]
        self.packets.write2ByteTxRx(self.port, 4, 31, 100)  # Homing_Offset +100, sign bit 11
        self.assertEqual(self.packets.read2ByteTxRx(self.port, 4, 56)[0], raw - 100)

    def test_a_negative_homing_offset_is_sign_magnitude(self) -> None:
        self.packets.write1ByteTxRx(self.port, 4, 55, 0)
        raw = self.packets.read2ByteTxRx(self.port, 4, 56)[0]
        self.packets.write2ByteTxRx(self.port, 4, 31, (1 << 11) | 100)  # −100
        self.assertEqual(self.packets.read2ByteTxRx(self.port, 4, 56)[0], raw + 100)

    def test_a_reply_with_a_bad_checksum_is_called_corrupt_by_the_makers_library(self) -> None:
        self.bus.corrupt(1)
        _, result, _ = self.packets.ping(self.port, 1)
        self.assertEqual(result, scs.COMM_RX_CORRUPT)
        _, result, _ = self.packets.ping(self.port, 1)
        self.assertEqual(result, scs.COMM_SUCCESS)

    def test_a_servo_that_goes_silent_times_out_and_the_others_still_answer(self) -> None:
        self.bus.silence(5)
        self.assertEqual(self.packets.ping(self.port, 5)[1], scs.COMM_RX_TIMEOUT)
        self.assertEqual(self.packets.ping(self.port, 6)[1], scs.COMM_SUCCESS)

    def test_a_fault_is_reported_in_the_error_byte_with_the_makers_own_bit(self) -> None:
        self.bus.fault(1, self.bus.errors["overload"])
        _, result, error = self.packets.ping(self.port, 1)
        self.assertEqual((result, error), (scs.COMM_SUCCESS, scs.ERRBIT_OVERLOAD))
        self.assertIn("overload", self.packets.getRxPacketError(error).lower())

    def test_sync_write_moves_several_servos_and_sync_read_reads_them_back(self) -> None:
        for ident in (1, 2, 3):
            self.packets.write1ByteTxRx(self.port, ident, 40, 1)
        writer = scs.GroupSyncWrite(self.port, self.packets, 42, 2)
        for ident, goal in ((1, 1800), (2, 2200), (3, 2600)):
            writer.addParam(ident, [goal & 0xFF, goal >> 8])
        self.assertEqual(writer.txPacket(), scs.COMM_SUCCESS)
        reader = scs.GroupSyncRead(self.port, self.packets, 56, 2)
        for ident in (1, 2, 3):
            reader.addParam(ident)

        def arrived() -> bool:
            if reader.txRxPacket() != scs.COMM_SUCCESS:
                return False
            return all(
                reader.isAvailable(ident, 56, 2) and abs(reader.getData(ident, 56, 2) - goal) <= 3
                for ident, goal in ((1, 1800), (2, 2200), (3, 2600))
            )

        self.assertTrue(wait_until(arrived))

    def test_a_stalled_servo_answers_and_does_not_move(self) -> None:
        self.bus.stall(1)
        self.packets.write1ByteTxRx(self.port, 1, 40, 1)
        before = self.packets.read2ByteTxRx(self.port, 1, 56)[0]
        self.packets.write2ByteTxRx(self.port, 1, 42, before + 500)
        time.sleep(0.4)
        self.assertEqual(self.packets.read2ByteTxRx(self.port, 1, 56)[0], before)

    def test_a_pushed_shaft_with_torque_off_stays_pushed(self) -> None:
        before = self.packets.read2ByteTxRx(self.port, 6, 56)[0]
        self.bus.push(6, 200)
        self.assertEqual(self.packets.read2ByteTxRx(self.port, 6, 56)[0], (before + 200) % 4096)


class LeRobotBus(unittest.TestCase):
    """The same bus through LeRobot's own `FeetechMotorsBus`."""

    def setUp(self) -> None:
        from lerobot.motors import Motor, MotorNormMode
        from lerobot.motors.feetech import FeetechMotorsBus

        self.bus = FeetechBus()
        self.path = self.bus.start()
        names = ("shoulder_pan", "shoulder_lift", "elbow_flex", "wrist_flex", "wrist_roll", "gripper")
        self.motors = {n: Motor(i + 1, "sts3215", MotorNormMode.DEGREES) for i, n in enumerate(names)}
        self.lerobot = FeetechMotorsBus(port=self.path, motors=self.motors)

    def tearDown(self) -> None:
        if self.lerobot.is_connected:
            self.lerobot.disconnect(disable_torque=False)
        self.bus.stop()

    def test_the_handshake_finds_six_motors_of_one_firmware(self) -> None:
        self.lerobot.connect()  # `_handshake`: every id exists, one firmware version
        self.assertTrue(self.lerobot.is_connected)

    def test_a_motor_that_does_not_answer_is_named_by_the_makers_own_error(self) -> None:
        self.bus.silence(4)
        with self.assertRaises(Exception) as caught:
            self.lerobot.connect()
        # LeRobot names the id and the model it expected (it does not know the bus has silenced one); that is the maker's sentence.
        message = str(caught.exception)
        self.assertIn("Missing motor IDs", message)
        self.assertIn("4 (expected model: 777)", message)

    def test_configure_motors_does_what_lerobot_says_it_does(self) -> None:
        self.lerobot.connect()
        self.lerobot.disable_torque()
        self.lerobot.configure_motors()
        for ident in range(1, 7):
            servo = self.bus.servo(ident)
            self.assertEqual(servo.peek("Return_Delay_Time"), 0)
            self.assertEqual(servo.peek("Acceleration"), 254)
            self.assertEqual(servo.peek("Phase") & 0x10, 0, "bit 4 of Phase must be cleared, as LeRobot's comment requires")

    def test_a_sync_write_of_a_goal_and_a_sync_read_of_the_position_agree(self) -> None:
        self.lerobot.connect()
        self.lerobot.write_calibration(
            {
                n: __import__("lerobot.motors", fromlist=["MotorCalibration"]).MotorCalibration(
                    id=m.id, drive_mode=0, homing_offset=0, range_min=500, range_max=3500
                )
                for n, m in self.motors.items()
            }
        )
        self.lerobot.enable_torque()
        self.lerobot.sync_write("Goal_Position", {"shoulder_pan": 20.0}, normalize=True)
        self.assertTrue(
            wait_until(lambda: abs(self.lerobot.sync_read("Present_Position", ["shoulder_pan"])["shoulder_pan"] - 20.0) < 0.5)
        )


if __name__ == "__main__":
    unittest.main(verbosity=1)

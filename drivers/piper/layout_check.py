"""This driver's constants and frame layouts against AgileX's own SDK.

    make check-layout PIPER_SDK=/path/to/piper_sdk PIPER_PYTHON=/path/to/python-with-python-can

`PIPER_SDK` is a checkout of AgileX's SDK (`agilexrobotics/piper_sdk`, MIT), read unmodified and not vendored here, and
`PIPER_PYTHON` an interpreter with `python-can` (the SDK's one dependency) installed. The check imports the maker's own
codec — `piper_sdk.protocol.protocol_v2.piper_protocol_v2.C_PiperParserV2`, the code that turns every frame into named
fields and back — and holds `skynet_piper.protocol` to it in **both directions**:

* what the arm sends: this driver's bytes for a status, a joint pair, the gripper, a driver's two reports and a limit
  report are decoded by the maker's `DecodeMessage`, and every field must be the one this module reads;
* what a master sends: the maker's `EncodeMessage` makes the bytes of a stop, a mode, a joint pair, a gripper command, an
  enable and a limit query, and they must be this module's, byte for byte;
* and the frames this driver sends are decoded by the same `DecodeMessage` (the SDK decodes a master's frames too, for
  its linkage mode), so the arm's side of each is read by the maker's code and not this repository's.

Random vectors, 300 of each, from a fixed seed. Then the enumerations (control mode, arm status, move mode, the error word's
bits, a driver's status byte), the published travel (`C_PiperParamManager`), and four behaviours out of the SDK's own source
the driver leans on, held as source text: if the maker rewords them the check fails and a person re-reads the file.

The checkout must be at the pinned commit. A newer SDK may change the protocol, and a verdict about a different SDK than the
pin names would be a verdict about nothing.
"""

from __future__ import annotations

import inspect
import math
import os
import random
import subprocess
import sys

from skynet_piper import protocol as p

#: The SDK this driver's constants were read from. `git rev-parse HEAD` at the clone (2026-07-31).
PIN = "c9e8a28174e71eeaac448593cb65f8ab258a92fe"

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


def die(message: str) -> int:
    print(f"::error::{message}")
    return 1


def main(argv: list[str]) -> int:
    sdk = argv[0] if argv else os.environ.get("PIPER_SDK", "")
    if not sdk:
        return die("PIPER_SDK is not set. This check holds the driver to AgileX's own SDK and has nothing to hold it to without one: "
                   "`git clone https://github.com/agilexrobotics/piper_sdk`, check out the pin in layout_check.py, and "
                   "`make check-layout PIPER_SDK=<that checkout>`. Nothing was measured.")
    if not os.path.isdir(os.path.join(sdk, "piper_sdk")):
        return die(f"{sdk} has no piper_sdk directory: it is not a checkout of agilexrobotics/piper_sdk. Nothing was measured.")
    head = subprocess.run(["git", "-C", sdk, "rev-parse", "HEAD"], capture_output=True, text=True).stdout.strip()
    if head != PIN:
        return die(f"{sdk} is at {head or 'no commit'} and this check was written against {PIN}. A newer SDK may change the protocol, "
                   "and a verdict about a different SDK than the pin names would be a verdict about nothing: check out the pin.")
    sys.path.insert(0, sdk)
    try:
        import can
        from piper_sdk.piper_msgs.msg_v2 import (
            ArmMessageMapping, ArmMsgFeedbackStatus, ArmMsgFeedbackStatusEnum, ArmMsgGripperCtrl, ArmMsgJointCtrl,
            ArmMsgMotionCtrl_1, ArmMsgMotionCtrl_2, ArmMsgMotorEnableDisableConfig, ArmMsgSearchMotorMaxAngleSpdAccLimit,
            ArmMsgType, CanIDPiper, PiperMessage,
        )
        from piper_sdk.piper_param.piper_param_manager import C_PiperParamManager
        from piper_sdk.protocol.protocol_v2.piper_protocol_v2 import C_PiperParserV2
        from piper_sdk.interface.piper_interface_v2 import C_PiperInterface_V2
    except ImportError as error:
        return die(f"the maker's SDK did not import ({error}). It needs `python-can`: run this under a Python that has it (PIPER_PYTHON).")

    parser = C_PiperParserV2()
    rng = random.Random(0x91B3)

    def decode(ident: int, data: bytes) -> PiperMessage:
        message = PiperMessage()
        frame = can.Message(arbitration_id=ident, data=bytearray(data), is_extended_id=False, timestamp=1.0)
        assert parser.DecodeMessage(frame, message), f"the maker's decoder does not know {ident:#x}"
        return message

    def encode(message: PiperMessage) -> tuple[int, bytes]:
        frame = can.Message()
        assert parser.EncodeMessage(message, frame), f"the maker's encoder does not know {message.type_}"
        return frame.arbitration_id, bytes(frame.data)

    def s32() -> int:
        return rng.randint(-(2**31), 2**31 - 1)

    def s16() -> int:
        return rng.randint(-(2**15), 2**15 - 1)

    # ── identifiers ──────────────────────────────────────────────────────────
    section("identifiers — the maker's CanIDPiper")
    pairs = {
        "ARM_STATUS_FEEDBACK": p.ID_STATUS, "ARM_JOINT_FEEDBACK_12": p.ID_JOINTS_12, "ARM_JOINT_FEEDBACK_34": p.ID_JOINTS_34,
        "ARM_JOINT_FEEDBACK_56": p.ID_JOINTS_56, "ARM_GRIPPER_FEEDBACK": p.ID_GRIPPER, "ARM_MOTION_CTRL_1": p.ID_MOTION_1,
        "ARM_MOTION_CTRL_2": p.ID_MOTION_2, "ARM_JOINT_CTRL_12": p.ID_JOINT_CTRL[0], "ARM_JOINT_CTRL_34": p.ID_JOINT_CTRL[1],
        "ARM_JOINT_CTRL_56": p.ID_JOINT_CTRL[2], "ARM_GRIPPER_CTRL": p.ID_GRIPPER_CTRL, "ARM_MOTOR_ENABLE_DISABLE_CONFIG": p.ID_ENABLE,
        "ARM_SEARCH_MOTOR_MAX_SPD_ACC_LIMIT": p.ID_LIMIT_QUERY, "ARM_FEEDBACK_CURRENT_MOTOR_ANGLE_LIMIT_MAX_SPD": p.ID_LIMIT_REPORT,
    }
    for name, ours in pairs.items():
        check(CanIDPiper[name].value == ours, f"{name} is {CanIDPiper[name].value:#x}, and this driver says {ours:#x}")
    for joint in range(6):
        check(CanIDPiper[f"ARM_INFO_HIGH_SPD_FEEDBACK_{joint + 1}"].value == p.ID_HIGH_SPEED + joint
              and CanIDPiper[f"ARM_INFO_LOW_SPD_FEEDBACK_{joint + 1}"].value == p.ID_LOW_SPEED + joint,
              f"joint {joint + 1}'s two driver reports are at {p.ID_HIGH_SPEED + joint:#x} and {p.ID_LOW_SPEED + joint:#x}")

    # ── what the arm sends, decoded by the maker's code ──────────────────────
    section("what the arm sends — this driver's bytes, read by the maker's DecodeMessage")
    ok_status = ok_pairs = ok_gripper = ok_high = ok_low = ok_limits = True
    for _ in range(300):
        ctrl, arm, mode, teach, motion, trajectory = (rng.choice(list(p.CTRL_MODES)), rng.randrange(16), rng.randrange(6), rng.randrange(8),
                                                      rng.randrange(2), rng.randrange(256))
        err = rng.randrange(65536)
        data = bytes([ctrl, arm, mode, teach, motion, trajectory]) + err.to_bytes(2, "big")
        ours = p.parse_status(data)
        theirs = decode(p.ID_STATUS, data).arm_status_msgs
        ok_status &= (ours.ctrl_mode, ours.arm_status, ours.mode_feed, ours.teach_status, ours.motion_status, ours.trajectory_num, ours.err_code) == (
            int(theirs.ctrl_mode), int(theirs.arm_status), int(theirs.mode_feed), int(theirs.teach_status), int(theirs.motion_status),
            theirs.trajectory_num, theirs.err_code)
        first, second = s32(), s32()
        pair_data = p.joint_ctrl(first, second)
        message = decode(p.ID_JOINTS_12, pair_data)
        ok_pairs &= p.parse_joint_pair(pair_data) == (message.arm_joint_feedback.joint_1, message.arm_joint_feedback.joint_2)
        stroke, torque, status = s32(), s16(), rng.randrange(256)
        gripper_data = stroke.to_bytes(4, "big", signed=True) + torque.to_bytes(2, "big", signed=True) + bytes([status, 0])
        theirs_gripper = decode(p.ID_GRIPPER, gripper_data).gripper_feedback
        ok_gripper &= p.parse_gripper(gripper_data) == (theirs_gripper.grippers_angle, theirs_gripper.grippers_effort, theirs_gripper.status_code)
        joint = rng.randrange(6)
        speed, current, pos = s16(), s16(), s32()
        high_data = speed.to_bytes(2, "big", signed=True) + current.to_bytes(2, "big", signed=True) + pos.to_bytes(4, "big", signed=True)
        high = getattr(decode(p.ID_HIGH_SPEED + joint, high_data), f"arm_high_spd_feedback_{joint + 1}")
        ours_high = p.parse_high_speed(high_data)
        ok_high &= (ours_high.motor_speed, ours_high.current, ours_high.pos) == (high.motor_speed, high.current, high.pos)
        vol, temp, motor_temp, low_status, bus = rng.randrange(65536), s16(), rng.randint(-128, 127), rng.randrange(256), rng.randrange(65536)
        low_data = (vol.to_bytes(2, "big") + temp.to_bytes(2, "big", signed=True) + motor_temp.to_bytes(1, "big", signed=True)
                    + bytes([low_status]) + bus.to_bytes(2, "big"))
        low = getattr(decode(p.ID_LOW_SPEED + joint, low_data), f"arm_low_spd_feedback_{joint + 1}")
        ours_low = p.parse_low_speed(low_data)
        ok_low &= (ours_low.vol, ours_low.foc_temp, ours_low.motor_temp, ours_low.status, ours_low.bus_current) == (
            low.vol, low.foc_temp, low.motor_temp, low.foc_status_code, low.bus_current)
        motor, high_angle, low_angle, max_speed = rng.randrange(1, 7), s16(), s16(), rng.randrange(65536)
        limit_data = (bytes([motor]) + high_angle.to_bytes(2, "big", signed=True) + low_angle.to_bytes(2, "big", signed=True)
                      + max_speed.to_bytes(2, "big") + bytes([0]))
        limits = decode(p.ID_LIMIT_REPORT, limit_data).arm_feedback_current_motor_angle_limit_max_spd
        ours_limits = p.parse_limit_report(limit_data)
        ok_limits &= (ours_limits.motor_num, ours_limits.max_angle, ours_limits.min_angle, ours_limits.max_speed) == (
            limits.motor_num, limits.max_angle_limit, limits.min_angle_limit, limits.max_joint_spd)
    check(ok_status, "300 random 0x2A1 status frames: control mode, arm status, move mode, teach, motion, trajectory and the 16-bit error word")
    check(ok_pairs, "300 random joint pairs: two signed 32-bit counts")
    check(ok_gripper, "300 random gripper frames: signed stroke, signed torque, status byte")
    check(ok_high, "300 random driver fast reports: signed speed, signed current, signed position")
    check(ok_low, "300 random driver slow reports: unsigned volts, signed temperatures, status byte, unsigned bus current")
    check(ok_limits, "300 random limit reports: signed angle limits, unsigned speed (see the pinned disagreement below)")

    # ── what a master sends, made by the maker's code ────────────────────────
    section("what a master sends — the maker's EncodeMessage, byte for byte against this driver's")
    ok = True
    for _ in range(300):
        stop, track, teach = rng.randrange(3), rng.randrange(9), rng.randrange(8)
        ident, data = encode(PiperMessage(type_=ArmMsgType.PiperMsgMotionCtrl_1, arm_motion_ctrl_1=ArmMsgMotionCtrl_1(stop, track, teach)))
        ok &= (ident, data) == (p.ID_MOTION_1, p.motion_ctrl_1(stop, track, teach))
    check(ok, "300 random 0x150 frames: the stop byte, the trajectory byte, the teach byte")
    ok = True
    for _ in range(300):
        ctrl, move, speed, mit, residence, install = (rng.choice([0, 1, 3, 4, 7]), rng.randrange(6), rng.randrange(101), rng.choice([0, 0xAD, 0xFF]),
                                                      rng.randrange(256), rng.randrange(4))
        ident, data = encode(PiperMessage(type_=ArmMsgType.PiperMsgMotionCtrl_2, arm_motion_ctrl_2=ArmMsgMotionCtrl_2(ctrl, move, speed, mit, residence, install)))
        ok &= (ident, data) == (p.ID_MOTION_2, p.motion_ctrl_2(ctrl, move, speed, mit, residence, install))
    check(ok, "300 random 0x151 frames: control mode, move mode, speed percent, MIT, residence time, installation position")
    for pair, ident_expected in enumerate(p.ID_JOINT_CTRL):
        ok = True
        for _ in range(300):
            values = [s32() for _ in range(6)]
            kind = (ArmMsgType.PiperMsgJointCtrl_12, ArmMsgType.PiperMsgJointCtrl_34, ArmMsgType.PiperMsgJointCtrl_56)[pair]
            ident, data = encode(PiperMessage(type_=kind, arm_joint_ctrl=ArmMsgJointCtrl(*values)))
            ok &= (ident, data) == (ident_expected, p.joint_ctrl(values[2 * pair], values[2 * pair + 1]))
        check(ok, f"300 random {ident_expected:#x} frames: joints {2 * pair + 1} and {2 * pair + 2}, signed 32-bit counts")
    ok = True
    for _ in range(300):
        stroke, torque, code, zero = s32(), rng.randrange(5001), rng.randrange(4), rng.choice([0, 0xAE])
        ident, data = encode(PiperMessage(type_=ArmMsgType.PiperMsgGripperCtrl, arm_gripper_ctrl=ArmMsgGripperCtrl(stroke, torque, code, zero)))
        ok &= (ident, data) == (p.ID_GRIPPER_CTRL, p.gripper_ctrl(stroke, torque, code, zero))
    check(ok, "300 random 0x159 frames: signed stroke, unsigned torque 0 to 5000, enable code, zero command")
    try:
        p.gripper_ctrl(0, 5001, 1)
        refused = False
    except ValueError:
        refused = True
    check(refused, "a gripper torque above 5000 is refused by this driver as it is by the maker's own constructor")
    ok = True
    for _ in range(100):
        motor, flag = rng.choice([1, 2, 3, 4, 5, 6, 7, 0xFF]), rng.choice([1, 2])
        ident, data = encode(PiperMessage(type_=ArmMsgType.PiperMsgMotorEnableDisableConfig, arm_motor_enable=ArmMsgMotorEnableDisableConfig(motor, flag)))
        ok &= (ident, data) == (p.ID_ENABLE, p.motor_enable(motor, flag))
    check(ok, "100 random 0x471 frames: the motor number (1-6, 7, 0xFF for all) and the enable flag")
    ok = True
    for motor in range(1, 7):
        for content in (1, 2):
            ident, data = encode(PiperMessage(type_=ArmMsgType.PiperMsgSearchMotorMaxAngleSpdAccLimit,
                                              arm_search_motor_max_angle_spd_acc_limit=ArmMsgSearchMotorMaxAngleSpdAccLimit(motor, content)))
            ok &= (ident, data) == (p.ID_LIMIT_QUERY, p.limit_query(motor, content))
    check(ok, "every 0x472 query: joint 1 to 6, angle and speed or acceleration")

    # ── what this driver sends, read by the maker's decoder ──────────────────
    section("what this driver sends — read back by the maker's DecodeMessage")
    ok = True
    for _ in range(300):
        ctrl, move, speed = rng.choice([0, 1]), rng.randrange(6), rng.randrange(101)
        decoded = decode(p.ID_MOTION_2, p.motion_ctrl_2(ctrl, move, speed)).arm_motion_ctrl_2
        ok &= (int(decoded.ctrl_mode), int(decoded.move_mode), decoded.move_spd_rate_ctrl) == (ctrl, move, speed)
    check(ok, "this driver's 0x151 is read by the maker's decoder as the mode, move mode and speed it was built from")
    ok = True
    for pair, ident in enumerate(p.ID_JOINT_CTRL):
        first, second = s32(), s32()
        decoded = decode(ident, p.joint_ctrl(first, second)).arm_joint_ctrl
        names = (f"joint_{2 * pair + 1}", f"joint_{2 * pair + 2}")
        ok &= (getattr(decoded, names[0]), getattr(decoded, names[1])) == (first, second)
    check(ok, "this driver's three joint-target frames are read by the maker's decoder as the angles they were built from")
    decoded = decode(p.ID_GRIPPER_CTRL, p.gripper_ctrl(70_000, 1000, 1)).arm_gripper_ctrl
    check((decoded.grippers_angle, decoded.grippers_effort, decoded.status_code, decoded.set_zero) == (70_000, 1000, 1, 0),
          "this driver's gripper command is read as 70 mm, 1 N·m, enable, no zero command")

    # ── the enumerations ─────────────────────────────────────────────────────
    section("the enumerations the arm reports")
    enums = ArmMsgFeedbackStatusEnum
    check({int(m) for m in enums.CtrlMode if m.name != "UNKNOWN"} == set(p.CTRL_MODES), "the eight control modes are the maker's, by value")
    check({int(m) for m in enums.ArmStatus if m.name != "UNKNOWN"} == set(p.ARM_STATUSES), "the sixteen arm statuses are the maker's, by value")
    check({int(m) for m in enums.ModeFeed if m.name != "UNKNOWN"} == set(p.MOVE_MODES), "the six move modes are the maker's, by value")
    check(enums.ArmStatus.EMERGENCY_STOP == p.ARM_STATUS_EMERGENCY_STOP and enums.ArmStatus.NORMAL == p.ARM_STATUS_NORMAL
          and enums.CtrlMode.CAN_CTRL == p.CTRL_CAN and enums.ModeFeed.MOVE_J == p.MOVE_J, "NORMAL, EMERGENCY_STOP, CAN_CTRL and MOVE_J are the values this driver uses")
    status = ArmMsgFeedbackStatus()
    for bit in range(16):
        status.err_code = 1 << bit
        flags = status.err_status
        read = {
            "comms": [getattr(flags, f"communication_status_joint_{j}") for j in range(1, 7)],
            "limit": [getattr(flags, f"joint_{j}_angle_limit") for j in range(1, 7)],
        }
        expected_comms = [bool(p.parse_status(bytes(6) + (1 << bit).to_bytes(2, "big")).comms_bits() & (1 << j)) for j in range(6)]
        expected_limit = [bool(p.parse_status(bytes(6) + (1 << bit).to_bytes(2, "big")).limit_bits() & (1 << j)) for j in range(6)]
        if read["comms"] != expected_comms or read["limit"] != expected_limit:
            check(False, f"error-word bit {bit}: the maker reads comms {read['comms']} limit {read['limit']}")
            break
    else:
        check(True, "all sixteen bits of the error word mean to this driver what they mean to the maker: communication in 0–5, angle limit in 8–13")
    check(p.FAULT_NOT_CAN_CONTROL & 0x3F3F == 0, "the bit this driver defines (15) is one the maker defines no flag for")
    low = decode(p.ID_LOW_SPEED, bytes(5) + bytes([0xFF]) + bytes(2)).arm_low_spd_feedback_1.foc_status
    flags = (low.voltage_too_low, low.motor_overheating, low.driver_overcurrent, low.driver_overheating, low.collision_status, low.driver_error_status,
             low.driver_enable_status, low.stall_status)
    check(all(flags), "a driver status byte of 0xFF sets all eight of the maker's flags")
    for index, name in enumerate(("voltage_too_low", "motor_overheating", "driver_overcurrent", "driver_overheating", "collision_status",
                                  "driver_error_status", "driver_enable_status", "stall_status")):
        single = decode(p.ID_LOW_SPEED, bytes(5) + bytes([1 << index]) + bytes(2)).arm_low_spd_feedback_1.foc_status
        check(getattr(single, name) and sum(bool(getattr(single, other)) for other in ("voltage_too_low", "motor_overheating", "driver_overcurrent",
              "driver_overheating", "collision_status", "driver_error_status", "driver_enable_status", "stall_status")) == 1,
              f"status bit {index} is the maker's `{name}`, and this driver's `{p.DRIVER_BIT_NAMES[1 << index]}`")

    names = ("voltage_too_low", "motor_overheating", "driver_overcurrent", "driver_overheating", "sensor_status", "driver_error_status",
             "driver_enable_status", "homing_status")
    for index, name in enumerate(names):
        single = decode(p.ID_GRIPPER, bytes(6) + bytes([1 << index, 0])).gripper_feedback.foc_status
        only = getattr(single, name) and sum(bool(getattr(single, other)) for other in names) == 1
        mine = (p.GRIPPER_FAULT_BITS & (1 << index) != 0) if index < 6 else None
        meaning = {6: p.GRIPPER_ENABLED, 7: p.GRIPPER_ZEROED}.get(index, 1 << index)
        check(only and meaning == 1 << index and (mine is None or p.GRIPPER_BIT_NAMES[1 << index]),
              f"gripper status bit {index} is the maker's `{name}`, and this driver's " + (f"`{p.GRIPPER_BIT_NAMES[1 << index]}`" if index < 6 else ("enabled" if index == 6 else "zeroed (homing_status)")))
    check(p.GRIPPER_FAULT_BITS == 0x3F and not p.GRIPPER_FAULT_BITS & (p.GRIPPER_ENABLED | p.GRIPPER_ZEROED),
          "the gripper's fault bits are the six the maker names as faults, and neither the enable bit nor the zeroing bit")

    # ── the published tables ─────────────────────────────────────────────────
    section("the travel the maker publishes")
    manager = C_PiperParamManager()
    table = [manager.GetJointLimitParam(f"j{joint}") for joint in range(1, 7)]
    check([tuple(row) for row in table] == [tuple(row) for row in p.JOINT_LIMITS_RAD], "the six joint ranges are C_PiperParamManager's table")
    check(tuple(manager.GetGripperRangeParam()) == p.GRIPPER_RANGE_M, "the gripper's stroke is the maker's 0 to 0.07 m")

    # ── behaviours out of the SDK's own source ───────────────────────────────
    section("what the SDK says it does — held as source text")
    source = inspect.getsource(C_PiperInterface_V2)
    reset = source.split("def ResetPiper", 1)[1].split("def MotionCtrl_2", 1)[0]
    check("lose power and fall" in reset and "self.MotionCtrl_1(0x02, 0x00, 0x00)" in reset,
          "ResetPiper is 0x150 with emergency_stop = 2, and the maker's own words for it are that the arm loses power and falls")
    stop = source.split("def EmergencyStop", 1)[1].split("def ResetPiper", 1)[0]
    check("self.MotionCtrl_1(emergency_stop, 0x00, 0x00)" in stop and "0x01: Emergency stop" in stop and "0x02: Resume" in stop,
          "EmergencyStop sends 0x150 with the stop byte, and its documented values are 1 stop and 2 resume — the same byte ResetPiper sends")
    enable = source.split("def EnableArm", 1)[1].split("def EnablePiper", 1)[0]
    check("enable_flag: Literal[0x01, 0x02] = 0x02" in enable and "ArmMsgMotorEnableDisableConfig(motor_num, enable_flag)" in enable and "0x471" in enable,
          "EnableArm sends 0x471 with the enable flag 2 for the motor it is given")
    mode = source.split("def MotionCtrl_2", 1)[1][:3000]
    check("0x151" in mode and "0x01: CAN command control mode" in mode.replace("CAN 指令控制模式", "CAN command control mode") or "CAN command control mode" in mode,
          "MotionCtrl_2 is 0x151, and control mode 1 is CAN command control")

    # ── the disagreements the maker's own sources have, pinned ───────────────
    section("pinned: where the maker's own sources disagree with each other (DRY-COVERAGE-PLAN.md F284)")
    from piper_sdk.piper_msgs.msg_v2.feedback import ArmMsgFeedbackCurrentMotorAngleLimitMaxSpd
    limit_source = inspect.getsource(ArmMsgFeedbackCurrentMotorAngleLimitMaxSpd)
    codec_source = inspect.getsource(C_PiperParserV2.DecodeMessage)
    negative = decode(p.ID_LIMIT_REPORT, bytes([1]) + (-1500).to_bytes(2, "big", signed=True) + (-1500).to_bytes(2, "big", signed=True) + bytes(3))
    check("uint16, 单位 0.1度" in limit_source and negative.arm_feedback_current_motor_angle_limit_max_spd.max_angle_limit == -1500,
          "0x473's maximum angle is documented as `uint16` and decoded as a signed 16-bit count by the maker's own codec; this driver follows the code, "
          "which is what a joint whose range is negative needs")
    check("ConvertToNegative_16bit(self.ConvertBytesToInt(can_data,6,8),False)" in codec_source.replace(" ", "").replace("\n", "") or "ConvertBytesToInt(can_data,6,8)" in codec_source,
          "the error word is bytes 6 and 7 read as one big-endian uint16, though the documentation lists them as two separate bytes")
    enable_doc = inspect.getsource(ArmMsgMotorEnableDisableConfig)
    check("7: Represents gripper motor" in enable_doc and "0xFF: Represents all joint motors" in enable_doc
          and "7 represents all motors" in source.split("def EnableArm", 1)[1].split("def EnablePiper", 1)[0]
          and "self.EnableArm(7)" in source.split("def EnablePiper", 1)[1].split("def DisablePiper", 1)[0],
          "0x471's motor number 7 is the gripper in the message class's documentation and *all motors* in the interface's, and EnablePiper is EnableArm(7); "
          "this driver enables joints 1 to 6 by number and the gripper as 7 only when there is one, and never sends 0xFF, so it means the same on either reading")
    gripper_doc = inspect.getsource(ArmMsgGripperCtrl)
    check("Byte 0-3 grippers_angle: int32, unit: 0.001°" in gripper_doc and "Byte 0 grippers_angle: int32, 单位 0.001mm" in gripper_doc,
          "the gripper command's stroke unit is documented in one place as 0.001 mm and in the other as 0.001°; this driver follows the first, "
          "which the maker's own parameter table (a stroke in metres, 0 to 0.07) agrees with")

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 0 if not FAILURES else 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

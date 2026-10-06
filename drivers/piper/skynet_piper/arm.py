"""A simulated Piper arm: the dry substrate, answering the way the maker's SDK says the firmware does.

**What this is, and what it is not.** It is a counterpart written in this repository (rung 4): it consumes the
frames a master sends, moves six joints and a gripper toward their targets, and broadcasts the arm's cyclic
reports, in the maker's own layouts. ``layout_check.py`` holds every frame it speaks to the maker's codec, so
the *bytes* are not this file's invention. Its behaviour is: what the firmware does when a target is outside a
joint's limits, when the motors are disabled, when the fast stop arrives or when the maker's ``ResetPiper`` byte
does, is modelled here from the SDK's own documentation of those commands and is not measured on any arm. An
agreement between this model and the driver is a driver agreeing with its author.

The behaviours it models, each with the SDK sentence it comes from:

* **The broadcast.** Once powered the arm sends 0x2A1, 0x2A5–0x2A8 and each driver's 0x251–0x256 and
  0x261–0x266 on its own. Here one :meth:`poll` is one cycle of that: the frames are handled, the joints take
  one step, and a full set of reports goes out.
* **A command needs a mode and enabled motors.** Joint targets (0x155–0x157) move the arm only in CAN control
  mode (0x151 ``ctrl_mode`` 1) with MOVE J (``move_mode`` 1) and every joint driver enabled (0x471). Otherwise
  they are ignored, which is what ``SKR_FLAG_FAULT``'s *commands are being ignored* is about.
* **A target outside the arm's own limits is refused by the arm** (arm status 4, *target position exceeds
  limit*, and the joint's limit bit in the error word) and is not moved to.
* **The fast stop** (0x150 ``emergency_stop`` 1) latches arm status 1 and stops every joint where it is.
* **The reset** (0x150 ``emergency_stop`` 2) is the SDK's ``ResetPiper``: *the arm loses power and falls,
  clearing every error and internal flag* — modelled as every driver disabled, every error cleared, and
  :attr:`dropped` set, so a test can say a driver sent it.
"""

from __future__ import annotations

import math
import struct

from skynet_canopen.bus import Bus
from skynet_canopen.frames import Frame

from . import protocol as p

__all__ = ["SimulatedPiper"]

#: How far a joint moves in one cycle at 100 % speed, in 0.001° — fast enough that a test is a few polls.
_STEP_MDEG_AT_FULL_SPEED = 20_000
#: How far the gripper moves in one cycle, in 0.001 mm.
_GRIPPER_STEP = 20_000


class SimulatedPiper:
    """One arm on one bus. ``poll()`` is its whole life: a cycle of the firmware's loop."""

    def __init__(self, bus: Bus, *, gripper: bool = True, powered: bool = True) -> None:
        self.bus = bus
        self.has_gripper = gripper
        self.powered = powered
        #: Joint angles, 0.001°, and the targets they move toward.
        self.mdeg = [0] * 6
        self.target = [0] * 6
        self.gripper_counts = 0
        self.gripper_target = 0
        self.ctrl_mode = p.CTRL_STANDBY
        self.move_mode = 0
        self.speed_percent = 0
        self.arm_status = p.ARM_STATUS_NORMAL
        self.err_code = 0
        #: Joint driver enable state (index 0–5) and the gripper's (index 6).
        self.enabled = [False] * 7
        #: Extra status bits a test can stage on each joint driver (the ``DRIVER_*`` flags).
        self.driver_flags = [0] * 6
        #: Extra status bits a test can stage on the gripper (the ``GRIPPER_*`` fault bits).
        self.gripper_flags = 0
        #: A gripper that never enables, whatever it is told (0x471 for motor 7, or 0x159's enable code).
        self.gripper_stuck = False
        #: The limits the arm holds and reports (0x473), 0.1°, and the joint speed ceiling, 0.001 rad/s.
        self.limits = [
            (round(math.degrees(hi) * 10), round(math.degrees(lo) * 10))
            for lo, hi in p.JOINT_LIMITS_RAD
        ]
        self.max_speed = [3000] * 6
        #: Every frame the arm received, in order: ``(id, bytes)``.
        self.received: list[tuple[int, bytes]] = []
        #: How many times the SDK's ``ResetPiper`` byte arrived, and whether the arm dropped.
        self.resets = 0
        self.dropped = False
        #: How many feedback cycles went out.
        self.cycles = 0
        #: Frames a test wants held back, by id: they are consumed and ignored.
        self.deaf_to: set[int] = set()
        #: Reports a test wants silenced, by id: the arm does not send them.
        self.mute: set[int] = set()

    # ── the firmware's loop ──────────────────────────────────────────────────

    def poll(self) -> None:
        if not self.powered:
            while self.bus.recv() is not None:
                pass
            return
        while True:
            frame = self.bus.recv()
            if frame is None:
                break
            self.received.append((frame.arbitration_id, bytes(frame.data)))
            if frame.arbitration_id in self.deaf_to:
                continue
            self._handle(frame)
        self._step()
        self._broadcast()

    # ── what the arm does with a frame ───────────────────────────────────────

    def _handle(self, frame: Frame) -> None:
        ident, data = frame.arbitration_id, bytes(frame.data)
        try:
            if ident == p.ID_MOTION_1:
                self._motion_1(*p.motion_ctrl_1_fields(data))
            elif ident == p.ID_MOTION_2:
                ctrl, move, speed, *_ = p.motion_ctrl_2_fields(data)
                self.ctrl_mode, self.move_mode, self.speed_percent = ctrl, move, speed
            elif ident in p.ID_JOINT_CTRL:
                self._joint_targets(p.ID_JOINT_CTRL.index(ident), *p.joint_ctrl_pair(data))
            elif ident == p.ID_GRIPPER_CTRL:
                self._gripper(*p.gripper_ctrl_fields(data))
            elif ident == p.ID_ENABLE:
                self._enable(*p.motor_enable_fields(data))
            elif ident == p.ID_LIMIT_QUERY:
                self._report_limits(*p.limit_query_fields(data))
        except ValueError:
            # A frame of the wrong length: the firmware ignores what it cannot parse.
            return

    def _motion_1(self, emergency_stop: int, track_ctrl: int, teach_ctrl: int) -> None:
        del track_ctrl, teach_ctrl
        if emergency_stop == p.EMERGENCY_STOP:
            self.arm_status = p.ARM_STATUS_EMERGENCY_STOP
            self.target = list(self.mdeg)
            self.gripper_target = self.gripper_counts
        elif emergency_stop == p._RESUME_IS_A_RESET:
            # `ResetPiper`: power is lost, the arm falls, every error and internal flag is cleared.
            self.resets += 1
            self.dropped = True
            self.arm_status = p.ARM_STATUS_NORMAL
            self.err_code = 0
            self.enabled = [False] * 7
            self.ctrl_mode = p.CTRL_STANDBY
            self.target = list(self.mdeg)
            self.gripper_target = self.gripper_counts

    def _motion_ready(self) -> bool:
        return (
            self.ctrl_mode == p.CTRL_CAN
            and self.move_mode == p.MOVE_J
            and all(self.enabled[:6])
            and self.arm_status == p.ARM_STATUS_NORMAL
        )

    def _joint_targets(self, pair: int, first: int, second: int) -> None:
        if not self._motion_ready():
            return
        for joint, counts in ((2 * pair, first), (2 * pair + 1, second)):
            high, low = self.limits[joint]
            if not low * 100 <= counts <= high * 100:
                # The arm refuses a target past its own limit: status 4, the joint's limit bit, no motion.
                self.arm_status = 0x04
                self.err_code |= 1 << (p.ERR_LIMIT_SHIFT + joint)
                return
        self.target[2 * pair] = first
        self.target[2 * pair + 1] = second

    def _gripper(self, stroke: int, torque: int, status_code: int, set_zero: int) -> None:
        del torque, set_zero
        if status_code in (0x01, 0x03):
            self.enabled[6] = not self.gripper_stuck
        elif status_code in (0x00, 0x02):
            self.enabled[6] = False
        if self.enabled[6] and self.arm_status == p.ARM_STATUS_NORMAL:
            self.gripper_target = max(0, min(int(p.GRIPPER_RANGE_M[1] * p.COUNTS_PER_METRE), stroke))

    def _enable(self, motor_num: int, flag: int) -> None:
        on = flag == 0x02
        if flag not in (0x01, 0x02):
            return
        if motor_num == 0xFF:
            self.enabled = [on] * 7
        elif 1 <= motor_num <= 7:
            self.enabled[motor_num - 1] = on
        if self.gripper_stuck:
            self.enabled[6] = False

    def _report_limits(self, motor_num: int, content: int) -> None:
        if content != 0x01 or not 1 <= motor_num <= 6:
            return
        high, low = self.limits[motor_num - 1]
        payload = struct.pack(">BhhH", motor_num, high, low, self.max_speed[motor_num - 1]) + bytes(1)
        self.bus.send(Frame(p.ID_LIMIT_REPORT, payload))

    # ── one cycle ────────────────────────────────────────────────────────────

    def _step(self) -> None:
        if self.arm_status != p.ARM_STATUS_NORMAL or not self._motion_ready():
            return
        step = max(1, _STEP_MDEG_AT_FULL_SPEED * self.speed_percent // 100)
        for joint in range(6):
            delta = self.target[joint] - self.mdeg[joint]
            self.mdeg[joint] += max(-step, min(step, delta))
        if self.enabled[6]:
            delta = self.gripper_target - self.gripper_counts
            self.gripper_counts += max(-_GRIPPER_STEP, min(_GRIPPER_STEP, delta))

    def _broadcast(self) -> None:
        self.cycles += 1
        moving = any(abs(self.target[j] - self.mdeg[j]) > 0 for j in range(6))
        status = struct.pack(
            ">BBBBBBH",
            self.ctrl_mode,
            self.arm_status,
            self.move_mode,
            0,
            p.MOTION_NOT_REACHED if moving else p.MOTION_REACHED,
            0,
            self.err_code,
        )
        self._send(p.ID_STATUS, status)
        for pair, ident in enumerate((p.ID_JOINTS_12, p.ID_JOINTS_34, p.ID_JOINTS_56)):
            self._send(ident, p.joint_ctrl(self.mdeg[2 * pair], self.mdeg[2 * pair + 1]))
        if self.has_gripper:
            self._send(p.ID_GRIPPER, struct.pack(">ihBB", self.gripper_counts, 0, self._gripper_status(), 0))
        for joint in range(6):
            self._send(p.ID_HIGH_SPEED + joint, struct.pack(">hhi", 0, 0, self.mdeg[joint]))
            flags = self.driver_flags[joint] | (p.DRIVER_ENABLED if self.enabled[joint] else 0)
            self._send(p.ID_LOW_SPEED + joint, struct.pack(">HhbBH", 240, 35, 40, flags, 0))

    def _gripper_status(self) -> int:
        return (p.GRIPPER_ENABLED if self.enabled[6] else 0) | p.GRIPPER_ZEROED | self.gripper_flags

    def _send(self, ident: int, data: bytes) -> None:
        if ident in self.mute:
            return
        self.bus.send(Frame(ident, data))

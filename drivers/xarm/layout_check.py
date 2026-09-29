"""This driver's constants against the vendor's own source.

    XARM_SDK=/path/to/xArm-Python-SDK python3 layout_check.py

`skynet_xarm.frames` transcribes ports, register numbers, a framing and the offsets of fields in
UFACTORY's report structures, each with the file it came from named beside it. A transcription is
right on the day it is made. This is the check that it is still right on the day the vendor ships a
release: it imports the SDK's own modules — the `XCONF` tables, `ReportDataStructure`, and the
byte-packing helpers `UxbusCmd` sends with — and compares.

What it holds, and what that would catch:

* **The ports** (`XCONF.SocketConf`) — the control socket and the two reports this driver reads.
* **The registers** (`XCONF.UxbusReg`) — all nine this driver sends. A register renumbered under it is
  a driver that starts an arm's motor enable where it meant to read its state.
* **The header** — the protocol identifier `UxbusCmdTcp` frames with, and the bits
  `check_private_protocol` reads out of a reply's state byte.
* **The report offsets** — every field of the rich and fast reports this driver parses, against the
  offset ctypes computes from `x3/report.py`'s structure, so a field the vendor inserts above one
  moves the number the driver reads and this says so.
* **The servo frame** — the bytes `BytesData.from_fp32_list` produces for a `MOVE_SERVOJ`, against
  `frames.servoj_data`'s.
* **The joint table** — `XCONF.Robot.JOINT_LIMITS`: the driver's copy of every arm's ranges (a controller's own are
  held against it, F181), and the arm the simulated controller stands for.

Exits non-zero on the first disagreement it cannot explain, and says what was compared.
"""

from __future__ import annotations

import ctypes
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from skynet_xarm import frames  # noqa: E402
from skynet_xarm.controller import XARM6_LIMITS  # noqa: E402

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


def main() -> int:
    sdk = os.environ.get("XARM_SDK")
    if not sdk or not os.path.isdir(os.path.join(sdk, "xarm")):
        print(
            "::error::XARM_SDK does not name an xArm-Python-SDK checkout. This check opens the vendor's own\n"
            "files and has nothing to compare without them:\n"
            "  git clone --depth 1 https://github.com/xArm-Developer/xArm-Python-SDK\n"
            "  XARM_SDK=$PWD/xArm-Python-SDK python3 layout_check.py"
        )
        return 2
    sys.path.insert(0, sdk)
    from xarm.core.config.x_config import XCONF  # noqa: PLC0415
    from xarm.core.utils.bytes_data import BytesData  # noqa: PLC0415
    from xarm.x3 import report  # noqa: PLC0415

    print(f"skynet-xarm layout check, against the SDK at {sdk}\n")

    print("the ports")
    check(frames.CONTROL_PORT == XCONF.SocketConf.TCP_CONTROL_PORT, "the control port is TCP_CONTROL_PORT")
    check(frames.REPORT_RICH_PORT == XCONF.SocketConf.TCP_REPORT_RICH_PORT, "the rich report is TCP_REPORT_RICH_PORT")
    check(frames.REPORT_REAL_PORT == XCONF.SocketConf.TCP_REPORT_REAL_PORT, "and the fast one is TCP_REPORT_REAL_PORT")

    print("\nthe registers")
    for name in ("GET_VERSION", "MOTION_EN", "SET_STATE", "GET_STATE", "GET_ERROR", "CLEAN_ERR", "CLEAN_WAR", "SET_MODE", "MOVE_SERVOJ"):
        check(getattr(frames.Fun, name) == getattr(XCONF.UxbusReg, name), f"{name} is UxbusReg.{name}")

    print("\nthe header")
    sys.path.insert(0, sdk)
    from xarm.core.wrapper import uxbus_cmd_tcp  # noqa: PLC0415

    check(frames.PROTOCOL_ID == uxbus_cmd_tcp.PRIVATE_MODBUS_TCP_PROTOCOL, "the protocol identifier is PRIVATE_MODBUS_TCP_PROTOCOL")

    class _Port:
        def __init__(self) -> None:
            self.sent = b""

        def flush(self) -> None:
            pass

        def write(self, data: bytes) -> int:
            self.sent = data
            return 0

    port = _Port()
    cmd = uxbus_cmd_tcp.UxbusCmdTcp(port)
    cmd.send_modbus_request(XCONF.UxbusReg.SET_STATE, bytes([4]), 1, t_id=1)
    check(port.sent == frames.request(1, frames.Fun.SET_STATE, bytes([4])), "a request is byte for byte what the vendor's send_modbus_request writes")

    source = open(os.path.join(sdk, "xarm/core/wrapper/uxbus_cmd_tcp.py"), encoding="utf-8").read()
    for bit, name in ((frames.StateByte.INVALID, "0x08"), (frames.StateByte.NOT_READY, "0x10"), (frames.StateByte.WARNING, "0x20"), (frames.StateByte.ERROR, "0x40")):
        check(f"state & {name}" in source, f"check_private_protocol still tests state & {name} ({bit:#04x})")

    print("\nthe servo frame")
    values = [0.1, -0.2, 0.3, -0.4, 0.5, -0.6, 0.0]
    vendor = BytesData.from_fp32_list(values + [0.0, 0.0, 0.0], 10)
    check(bytes(vendor) == frames.servoj_data(values), "a MOVE_SERVOJ's ten float32 are the bytes BytesData.from_fp32_list makes")
    uxbus = open(os.path.join(sdk, "xarm/core/wrapper/uxbus_cmd.py"), encoding="utf-8").read()
    check("txdata += [mvvelo, mvacc, mvtime]" in uxbus and "MOVE_SERVOJ, txdata, 10" in uxbus, "and move_servoj still sends seven angles then speed, acceleration and time")

    print("\nthe report offsets")
    fast = report._Report30003DataStructure
    rich = report._Report30002DataStructure

    def offset(cls, field: str) -> int:
        return getattr(cls, f"_{field}").offset

    check(offset(fast, "state_mode") == frames._STATE_MODE, "state_mode (fast)")
    check(offset(fast, "cmd_num") == frames._CMD_NUM, "cmd_num (fast)")
    check(offset(fast, "actual_joint_angle") == frames._ANGLES, "actual_joint_angle (fast)")
    check(offset(fast, "actual_tcp_pose") == frames._HEAD_END, "and the head ends where the tcp pose begins")
    for field, ours in (
        ("state_mode", frames._STATE_MODE),
        ("actual_joint_angle", frames._ANGLES),
        ("motor_enable_status", frames._ENABLE),
        ("error_code", frames._ERROR),
        ("warn_code", frames._WARN),
        ("robot_type", frames._ROBOT_TYPE),
        ("robot_axis", frames._ROBOT_AXIS),
        ("version", frames._VERSION),
        ("joint_max_speed", frames._JOINT_MAX_SPEED),
        ("is_simulation_robot", frames._IS_SIMULATION),
        ("is_reduced_mode", frames._IS_REDUCED),
        ("reduced_joint_limits", frames._JOINT_LIMITS),
    ):
        check(offset(rich, field) == ours, f"{field} (rich) is at {ours}")
    check(ctypes.sizeof(dict(rich._fields_)["_reduced_joint_limits"]) == 56, "the joint ranges are seven pairs of float32")
    check(ctypes.sizeof(dict(rich._fields_)["_version"]) == 30, "and the version is thirty bytes")
    check(frames._RICH_END <= ctypes.sizeof(rich), "and the fields this driver reads end inside the structure")

    print("\nthe joint table")
    table = XCONF.Robot.JOINT_LIMITS[6][XCONF.Robot.Type.XARM6_X4]
    check(
        all(abs(lo - a) < 1e-9 and abs(hi - b) < 1e-9 for (lo, hi), (a, b) in zip(XARM6_LIMITS, table)),
        "the simulated controller's xArm 6 ranges are the vendor's JOINT_LIMITS[6][XARM6_X4]",
    )

    ours = frames.VENDOR_JOINT_LIMITS
    theirs = {(axis, arm_type): tuple(ranges) for axis, types in XCONF.Robot.JOINT_LIMITS.items() for arm_type, ranges in types.items()}
    check(set(ours) == set(theirs), f"the driver carries the vendor's table for every arm the SDK lists ({len(theirs)}), and no other")
    check(
        all(
            len(ours[key]) == key[0] and all(abs(a - x) < 1e-12 and abs(b - y) < 1e-12 for (a, b), (x, y) in zip(ours[key], theirs[key]))
            for key in ours
            if key in theirs
        ),
        "and every range in it is the vendor's, to the last digit",
    )

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())

"""This driver's constants and bytes against ROS-Industrial's own `simple_message` library.

    INDUSTRIAL_CORE=/path/to/industrial_core python3 layout_check.py

`skynet_ros_industrial.frames` transcribes a framing, the numbers of the message types and the layout of five message bodies, with the maker's file beside each. A transcription is right on the day it is
made; this is the check that it is still right on the day the maker ships a change, and it does not stop at reading text. It **builds the maker's library** (without ROS: `sim/build-reference.sh`) into `sim/codec_dump.cpp` and holds the two implementations to each other in both directions, for each of the four ways a controller's build can spell an int and a real:

* **Constants**, read out of the maker's headers: the ten joints of a message, the 1,024-byte socket buffer, every message type, comm type, reply code, special sequence number, valid-field bit,
  tri-state and robot mode this driver names.
* **The bytes the maker makes**: each message the driver speaks, encoded by the maker's `toByteArray` and `SmplMsgConnection::sendMsg`'s own steps, against `frames.encode` and its body builders,
  **byte for byte**, in the default build and in `BYTE_SWAPPING`, `FLOAT64` and both.
* **The bytes the maker reads**: every frame this driver makes, read by the maker's `SimpleMessage::init` and the typed message's `unload`, and the fields it finds against the fields that went in. A driver whose
  bytes the maker's code reads differently has a defect the other direction cannot show.
* **The maker's bytes through this driver's reader**: the maker's frames, cut by `Stream` and parsed by the `parse_*` functions, against the values that went in.

Exits non-zero on the first disagreement it cannot explain, and says what was compared. It needs `g++` and a checkout of the maker's repository (`git clone --depth 1 https://github.com/ros-industrial/industrial_core`).
"""

from __future__ import annotations

import os
import re
import shutil
import struct
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

from skynet_ros_industrial import frames

FAILURES: list[str] = []
CHECKS = 0

POSITIONS = [0.1, -0.2, 0.3, -0.4, 0.5, -0.6]
VELOCITIES = [0.01, 0.02, -0.03, 0.04, -0.05, 0.06]
ACCELERATIONS = [1.0, -1.0, 0.5, -0.5, 0.25, -0.25]

#: The maker's four builds: (name, compiler flags, the Format this driver uses for it).
BUILDS = [
    ("the default build", [], frames.Format()),
    ("BYTE_SWAPPING (a big-endian controller)", ["-DBYTE_SWAPPING"], frames.Format(byte_order="big")),
    ("FLOAT64 (a controller whose real is a double)", ["-DFLOAT64"], frames.Format(real_bytes=8)),
    ("BYTE_SWAPPING and FLOAT64", ["-DBYTE_SWAPPING", "-DFLOAT64"], frames.Format(byte_order="big", real_bytes=8)),
]


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")


def read(core: str, relative: str) -> str:
    with open(os.path.join(core, "simple_message", relative), encoding="utf-8") as handle:
        return handle.read()


def enumerator(text: str, name: str) -> int | None:
    """`NAME = 12` or `NAME = 0x01` in an enum body, with a trailing comment ignored. The first such line wins."""
    found = re.search(r"\b" + re.escape(name) + r"\s*=\s*(-?0x[0-9a-fA-F]+|-?\d+)", text)
    return None if found is None else int(found.group(1), 0)


def build(core: str, out: str, flags: list[str]) -> str:
    target = os.path.join(out, "dump" + "".join(flag.lower().replace("-d", "-") for flag in flags))
    done = subprocess.run(["sh", os.path.join(HERE, "sim", "build-reference.sh"), core, target, os.path.join(HERE, "sim", "codec_dump.cpp"), *flags], capture_output=True, text=True, check=False)
    if done.returncode != 0:
        print(done.stdout[-1500:], done.stderr[-3000:])
        raise SystemExit(f"::error::the maker's library did not build with {flags}")
    return target


def cases(fmt: frames.Format) -> dict[str, bytes]:
    """What this driver makes for each message the maker's tool makes, in this build's spelling."""
    request, reply = frames.Comm.SERVICE_REQUEST, frames.Comm.SERVICE_REPLY
    body = frames.joint_traj_pt(3, POSITIONS, 0.25, 0.5, fmt)
    return {
        "traj_pt_request": frames.encode(frames.MsgType.JOINT_TRAJ_PT, request, frames.Reply.INVALID, body, fmt),
        "traj_pt_reply_success": frames.encode(frames.MsgType.JOINT_TRAJ_PT, reply, frames.Reply.SUCCESS, body, fmt),
        "traj_pt_reply_failure": frames.encode(frames.MsgType.JOINT_TRAJ_PT, reply, frames.Reply.FAILURE, body, fmt),
        "traj_pt_stop": frames.encode(frames.MsgType.JOINT_TRAJ_PT, request, frames.Reply.INVALID, frames.joint_traj_pt(frames.Seq.STOP_TRAJECTORY, POSITIONS, 0.0, 0.0, fmt), fmt),
        "feedback": frames.encode(
            frames.MsgType.JOINT_FEEDBACK, frames.Comm.TOPIC, frames.Reply.INVALID,
            frames.joint_feedback(7, frames.Valid.TIME | frames.Valid.POSITION | frames.Valid.VELOCITY, 12.5, POSITIONS, VELOCITIES, ACCELERATIONS, fmt), fmt),
        "joint": frames.encode(frames.MsgType.JOINT, frames.Comm.TOPIC, frames.Reply.INVALID, frames.joint_position(0, POSITIONS, fmt), fmt),
        "status": frames.encode(
            frames.MsgType.STATUS, frames.Comm.TOPIC, frames.Reply.INVALID,
            frames.robot_status(frames.Status(1, 0, 4021, 0, 1, frames.RobotMode.AUTO, 1), fmt), fmt),
        "ping_request": frames.ping(fmt),
        "ping_reply": frames.encode(frames.MsgType.PING, reply, frames.Reply.SUCCESS, b"", fmt),
    }


def maker_cases(binary: str) -> dict[str, bytes]:
    done = subprocess.run([binary, "encode"], capture_output=True, text=True, check=True)
    found: dict[str, bytes] = {}
    for line in done.stdout.splitlines():
        parts = line.split(" ")
        # A frame line is a name and hex and nothing else; anything else on standard output (the maker's logging, if it was built in) is not one.
        if len(parts) == 2 and re.fullmatch(r"[a-z_]+", parts[0]) and re.fullmatch(r"(?:[0-9a-f]{2})+", parts[1]):
            found[parts[0]] = bytes.fromhex(parts[1])
    return found


def maker_reads(binary: str, frame: bytes) -> dict[str, list[float]]:
    done = subprocess.run([binary, "decode", frame.hex()], capture_output=True, text=True, check=False)
    fields: dict[str, list[float]] = {}
    for line in done.stdout.splitlines():
        parts = line.split(" ")
        if re.fullmatch(r"[a-z_]+", parts[0]) and all(re.fullmatch(r"-?[0-9.e+-]+|nan|inf", p) for p in parts[1:]) and len(parts) > 1:
            fields[parts[0]] = [float(p) for p in parts[1:]]
        elif parts[0] == "error":
            fields["error"] = [1.0]
    return fields


def single(value: float) -> float:
    """What a 32-bit real holds of a number: the maker prints its float widened to a double, so 0.1 comes back as 0.10000000149."""
    return struct.unpack("<f", struct.pack("<f", value))[0]


def close(left: list[float], right: list[float], fmt: frames.Format) -> bool:
    if len(left) != len(right):
        return False
    if fmt.real_bytes == 4:
        return all(single(a) == single(b) for a, b in zip(left, right))
    return all(abs(a - b) <= 1e-15 * max(1.0, abs(a)) for a, b in zip(left, right))


def main() -> int:
    core = os.environ.get("INDUSTRIAL_CORE")
    if not core or not os.path.isdir(os.path.join(core, "simple_message", "include")):
        print(
            "::error::INDUSTRIAL_CORE does not name an industrial_core checkout. This check builds the maker's own library and has nothing to compare without it:\n"
            "  git clone --depth 1 https://github.com/ros-industrial/industrial_core\n"
            "  INDUSTRIAL_CORE=$PWD/industrial_core python3 layout_check.py"
        )
        return 2
    if shutil.which("g++") is None:
        print("::error::g++ is not installed; the maker's library is C++ and this check builds it.")
        return 2

    print(f"skynet-ros-industrial layout check, against the library at {core}\n")

    print("the constants, out of the maker's headers")
    joint_data = read(core, "include/simple_message/joint_data.h")
    check(frames.MAX_JOINTS == enumerator(joint_data, "MAX_NUM_JOINTS"), "a message carries MAX_NUM_JOINTS joints")
    socket_header = read(core, "include/simple_message/socket/simple_socket.h")
    check(frames.MAX_MESSAGE == enumerator(socket_header, "MAX_BUFFER_SIZE"), "and the socket reads MAX_BUFFER_SIZE bytes")
    check(frames.Port.MOTION == enumerator(socket_header, "MOTION") and frames.Port.SYSTEM == enumerator(socket_header, "SYSTEM") and frames.Port.STATE == enumerator(socket_header, "STATE")
          and frames.Port.IO == enumerator(socket_header, "IO"), "the standard ports are the maker's: motion 11000, system 11001, state 11002, io 11003")
    simple = read(core, "include/simple_message/simple_message.h")
    for name in ("PING", "JOINT_POSITION", "JOINT_TRAJ_PT", "JOINT_TRAJ", "STATUS", "JOINT_TRAJ_PT_FULL", "JOINT_FEEDBACK", "READ_INPUT", "WRITE_OUTPUT"):
        check(getattr(frames.MsgType, name) == enumerator(simple, name), f"message type {name} is StandardMsgTypes::{name}")
    check(frames.MsgType.JOINT == enumerator(simple, "JOINT"), "message type JOINT is StandardMsgTypes::JOINT")
    for name in ("TOPIC", "SERVICE_REQUEST", "SERVICE_REPLY"):
        check(getattr(frames.Comm, name) == enumerator(simple, name), f"comm type {name} is CommTypes::{name}")
    for name in ("SUCCESS", "FAILURE"):
        check(getattr(frames.Reply, name) == enumerator(simple, name), f"reply code {name} is ReplyTypes::{name}")
    check(frames.MsgType.INVALID == enumerator(simple, "INVALID") == frames.Comm.INVALID == frames.Reply.INVALID, "INVALID is 0 in all three")
    pt = read(core, "include/simple_message/joint_traj_pt.h")
    for name, ours in (("START_TRAJECTORY_DOWNLOAD", frames.Seq.START_TRAJECTORY_DOWNLOAD), ("START_TRAJECTORY_STREAMING", frames.Seq.START_TRAJECTORY_STREAMING),
                       ("END_TRAJECTORY", frames.Seq.END_TRAJECTORY), ("STOP_TRAJECTORY", frames.Seq.STOP_TRAJECTORY)):
        check(ours == enumerator(pt, name), f"special sequence number {name} is {ours}")
    feedback_header = read(core, "include/simple_message/joint_feedback.h")
    for name in ("TIME", "POSITION", "VELOCITY", "ACCELERATION"):
        check(getattr(frames.Valid, name) == enumerator(feedback_header, name), f"feedback valid-field bit {name} is ValidFieldTypes::{name}")
    status_header = read(core, "include/simple_message/robot_status.h")
    check(frames.RobotMode.MANUAL == enumerator(status_header, "MANUAL") and frames.RobotMode.AUTO == enumerator(status_header, "AUTO") and frames.RobotMode.UNKNOWN == enumerator(status_header, "UNKNOWN"),
          "robot modes are UNKNOWN -1, MANUAL 1, AUTO 2")
    check(frames.TriState.TRUE == enumerator(status_header, "TS_TRUE") and frames.TriState.FALSE == enumerator(status_header, "TS_FALSE") and frames.TriState.UNKNOWN == enumerator(status_header, "TS_UNKNOWN"),
          "and a tri-state is unknown -1, false 0, true 1")
    shared = read(core, "include/simple_message/shared_types.h")
    check("typedef float shared_real;" in shared and "typedef double shared_real;" in shared and "FLOAT64" in shared, "a real is a float, and a double under FLOAT64")
    check("typedef int shared_int;" in shared, "and an int is an int")

    with tempfile.TemporaryDirectory() as out:
        for name, flags, fmt in BUILDS:
            print(f"\n{name}")
            binary = build(core, out, flags)
            theirs = maker_cases(binary)
            ours = cases(fmt)
            check(set(theirs) == set(ours) and len(theirs) == 9, f"the maker makes the {len(ours)} messages this driver knows")
            for key in ours:
                check(theirs.get(key) == ours[key], f"{key}: the bytes are the maker's, all {len(ours[key])} of them")

            # The maker's bytes through this driver's reader.
            stream = frames.Stream(fmt)
            parsed = {key: stream.feed(frame) for key, frame in theirs.items()}
            point = frames.parse_joint_traj_pt(parsed["traj_pt_request"][0].body, fmt)
            check(point.seq == 3 and all(abs(a - b) < 1e-6 for a, b in zip(point.joints[:6], POSITIONS)) and point.joints[6:] == (0.0,) * 4 and abs(point.velocity - 0.25) < 1e-9
                  and abs(point.duration - 0.5) < 1e-9, "the maker's point reads back as the point that went in")
            check(frames.parse_joint_traj_pt(parsed["traj_pt_stop"][0].body, fmt).seq == frames.Seq.STOP_TRAJECTORY, "and its stop reads as STOP_TRAJECTORY")
            check((parsed["traj_pt_reply_success"][0].comm, parsed["traj_pt_reply_success"][0].reply) == (frames.Comm.SERVICE_REPLY, frames.Reply.SUCCESS)
                  and parsed["traj_pt_reply_failure"][0].reply == frames.Reply.FAILURE, "and a reply says whether it succeeded")
            feedback = frames.parse_joint_feedback(parsed["feedback"][0].body, fmt)
            check(feedback.robot_id == 7 and feedback.valid == 7 and abs(feedback.time - 12.5) < 1e-9 and all(abs(a - b) < 1e-6 for a, b in zip(feedback.positions[:6], POSITIONS))
                  and all(abs(a - b) < 1e-6 for a, b in zip(feedback.velocities[:6], VELOCITIES)) and all(abs(a - b) < 1e-6 for a, b in zip(feedback.accelerations[:6], ACCELERATIONS)),
                  "the maker's feedback reads back: the robot, the valid fields, the time and the three arrays")
            older = frames.parse_joint_position(parsed["joint"][0].body, fmt)
            check(older[0] == 0 and all(abs(a - b) < 1e-6 for a, b in zip(older[1][:6], POSITIONS)), "and so does its older positions message, with its sequence number")
            check(frames.parse_robot_status(parsed["status"][0].body, fmt) == frames.Status(1, 0, 4021, 0, 1, frames.RobotMode.AUTO, 1), "and its status: the seven fields in the maker's order")
            check(len(parsed["ping_request"]) == 1 and parsed["ping_request"][0].body == b"", "and its ping is a message with no body")

            # This driver's bytes through the maker's reader.
            read_back = maker_reads(binary, ours["traj_pt_request"])
            check(read_back.get("seq") == [3.0] and close(read_back.get("joints", []), POSITIONS, fmt) and close(read_back.get("velocity", []), [0.25], fmt) and close(read_back.get("duration", []), [0.5], fmt)
                  and read_back.get("type") == [11.0] and read_back.get("comm") == [2.0], "this driver's point is read by the maker's code as the point that went in")
            check(maker_reads(binary, ours["traj_pt_stop"]).get("seq") == [float(frames.Seq.STOP_TRAJECTORY)], "and its stop is the maker's STOP_TRAJECTORY")
            read_back = maker_reads(binary, ours["feedback"])
            check(read_back.get("robot_id") == [7.0] and read_back.get("valid") == [7.0] and close(read_back.get("time", []), [12.5], fmt) and close(read_back.get("positions", []), POSITIONS, fmt)
                  and close(read_back.get("velocities", []), VELOCITIES, fmt) and close(read_back.get("accelerations", []), ACCELERATIONS, fmt), "and a feedback message made here is read by the maker's code the same way")
            check(maker_reads(binary, ours["status"]).get("status") == [1.0, 0.0, 4021.0, 0.0, 1.0, 2.0, 1.0], "and so is a status")
            check("error" in maker_reads(binary, ours["traj_pt_request"][:-1]), "and the maker's reader refuses a frame whose length is not the bytes that came (so the check can fail)")

    print(f"\n{CHECKS} passed, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

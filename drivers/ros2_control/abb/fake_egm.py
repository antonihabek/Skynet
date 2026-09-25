"""A fake ABB robot controller speaking EGM, for the ABB row's no-RobotStudio substrate.

`DRY-COVERAGE-PLAN.md` Phase 8, item 8.4, and `docs/robotstudio-runbook.md` section 6: the controller
side of Externally Guided Motion, written here from ABB's own message definitions. `egm.proto` ships
in `ros-industrial/abb_libegm` under BSD-3-Clause and in RobotWare's own installation folder; the
image `abb/Dockerfile` builds compiles it with `protoc`, and this file imports the result. So the
two ends of the wire share ABB's definition and nothing else: the client on the other end is
PickNik's `abb_hardware_interface` over ros-industrial's `abb_libegm`, unmodified, hosted by the
`ros2_control` bridge.

What EGM is, as far as this fake models it. The **controller** is the UDP client: at its sample
rate (4 ms by default) it sends an `EgmRobot` — feedback joints in degrees, the planned joints, the
motor, EGM and RAPID states — to the sensor's port, and the sensor answers each one with an
`EgmSensor` whose `planned.joints` is the position reference, in degrees. This fake moves its six
joints toward the last reference at a bounded speed and reports where they are. It does **not**
model what a real controller does between the reference and the motor — `EGMActJoint`'s
`PosCorrGain`, its filters, the speed reference's meaning, the comm timeout that stops EGM — and a
green run is therefore a statement about the bytes and the loop, not about an ABB arm.

One field is a constant rather than a measurement, and it is said here because it looks like one.
A real controller sends the tool pose beside the joints, and `abb_libegm` refuses a six-axis
message without it (`parse(Feedback)` and `parse(Planned)` require `cartesian`). This fake has no
kinematics, so it sends the origin with the identity rotation. Nothing on the other end reads it:
PickNik's interface exports joint interfaces only, and the bridge reads nothing else.

It counts what a controller would reject or never see, and prints the counts as one JSON line on
SIGTERM for `run-egm.sh` to read:

* `malformed` — a datagram that does not parse as an `EgmSensor`;
* `wrong_type` — a reply whose header is not `MSGTYPE_CORRECTION`;
* `wrong_count` — a reference that does not carry exactly six joints;
* `not_finite` — a reference with a NaN or an infinity in it;
* `beyond_range` — a reference outside the axis range the controller was started with;
* `unanswered` — a message the sensor did not answer before the next one went out.

Usage (inside the image):

    python3 fake_egm.py --port 6511 --hz 250 --start=0,10,20,0,30,0 \
        --lower=-170,-100,-200,-270,-130,-360 --upper=170,130,70,270,130,360

Licence: Apache-2.0 (this file). `egm_pb2` is generated from ABB's BSD-3 `egm.proto` and is not
vendored here.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import select
import signal
import socket
import sys
import time

sys.path.insert(0, os.environ.get("EGM_PY", "/opt/abb_ws/egm_py"))
import egm_pb2  # noqa: E402  (generated in the image from abb_libegm's proto/egm.proto)


def floats(text: str) -> list[float]:
    values = [float(part) for part in text.split(",")]
    if len(values) != 6:
        raise argparse.ArgumentTypeError(f"six comma-separated values, got {len(values)}")
    return values


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--host", default="127.0.0.1", help="where the sensor listens")
    parser.add_argument("--port", type=int, default=6511, help="the sensor's EGM port")
    parser.add_argument("--hz", type=float, default=250.0, help="the controller's EGM rate")
    parser.add_argument("--start", type=floats, default=[0.0] * 6, help="initial joints, degrees")
    parser.add_argument("--lower", type=floats, required=True, help="axis lower bounds, degrees")
    parser.add_argument("--upper", type=floats, required=True, help="axis upper bounds, degrees")
    parser.add_argument("--speed", type=float, default=90.0, help="joint speed cap, degrees/s")
    args = parser.parse_args()

    joints = list(args.start)
    target = list(args.start)
    counts = {
        "sent": 0, "replies": 0, "corrections": 0, "malformed": 0, "wrong_type": 0,
        "wrong_count": 0, "not_finite": 0, "beyond_range": 0, "unanswered": 0,
    }
    stopping = False

    def stop(_signum, _frame):
        nonlocal stopping
        stopping = True

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.bind(("127.0.0.1", 0))
    sensor = (args.host, args.port)
    period = 1.0 / args.hz
    started = time.monotonic()
    seqno = 0
    answered = True
    print(f"fake_egm: sending EgmRobot to {sensor[0]}:{sensor[1]} at {args.hz:g} Hz",
          file=sys.stderr, flush=True)

    next_tick = time.monotonic()
    while not stopping:
        now = time.monotonic()
        if now >= next_tick:
            if seqno > 0 and not answered:
                counts["unanswered"] += 1
            # Move toward the last reference, bounded — the one piece of plant this fake has.
            step = args.speed * period
            for i in range(6):
                delta = target[i] - joints[i]
                joints[i] += max(-step, min(step, delta))
            robot = egm_pb2.EgmRobot()
            robot.header.seqno = seqno
            robot.header.tm = int((now - started) * 1000.0) & 0xFFFFFFFF
            robot.header.mtype = egm_pb2.EgmHeader.MSGTYPE_DATA
            robot.feedBack.joints.joints.extend(joints)
            wall = time.time()
            robot.feedBack.time.sec = int(wall)
            robot.feedBack.time.usec = int((wall % 1.0) * 1e6)
            # The constant pose the header explains: present because the client requires it,
            # and never a measurement.
            for pose in (robot.feedBack.cartesian, robot.planned.cartesian):
                pose.pos.x = pose.pos.y = pose.pos.z = 0.0
                pose.orient.u0 = 1.0
                pose.orient.u1 = pose.orient.u2 = pose.orient.u3 = 0.0
            robot.planned.joints.joints.extend(target)
            robot.planned.time.sec = robot.feedBack.time.sec
            robot.planned.time.usec = robot.feedBack.time.usec
            robot.motorState.state = egm_pb2.EgmMotorState.MOTORS_ON
            robot.mciState.state = egm_pb2.EgmMCIState.MCI_RUNNING
            robot.mciConvergenceMet = True
            robot.rapidExecState.state = egm_pb2.EgmRapidCtrlExecState.RAPID_RUNNING
            robot.testSignals.signals.extend([0.0] * 6)
            robot.measuredForce.force.extend([0.0] * 6)
            robot.utilizationRate = 0.0
            try:
                sock.sendto(robot.SerializeToString(), sensor)
                counts["sent"] += 1
            except OSError:
                pass
            seqno = (seqno + 1) & 0xFFFFFFFF
            answered = False
            next_tick += period
            if next_tick < now:  # fell behind: skip ahead rather than burst
                next_tick = now + period

        timeout = max(0.0, next_tick - time.monotonic())
        ready, _, _ = select.select([sock], [], [], timeout)
        if not ready:
            continue
        try:
            data, _peer = sock.recvfrom(65535)
        except OSError:
            continue
        counts["replies"] += 1
        answered = True
        reply = egm_pb2.EgmSensor()
        try:
            reply.ParseFromString(data)
        except Exception:  # noqa: BLE001 — protobuf raises its own DecodeError
            counts["malformed"] += 1
            continue
        if reply.header.mtype != egm_pb2.EgmHeader.MSGTYPE_CORRECTION:
            counts["wrong_type"] += 1
            continue
        reference = list(reply.planned.joints.joints)
        if len(reference) != 6:
            counts["wrong_count"] += 1
            continue
        if not all(math.isfinite(value) for value in reference):
            counts["not_finite"] += 1
            continue
        if any(value < low - 1e-6 or value > high + 1e-6
               for value, low, high in zip(reference, args.lower, args.upper)):
            counts["beyond_range"] += 1
            continue
        counts["corrections"] += 1
        target = reference

    print(json.dumps(counts), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())

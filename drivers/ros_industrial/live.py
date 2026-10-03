"""The ROS-Industrial bridge against a server built from ROS-Industrial's own `simple_message` library (DRY-COVERAGE-PLAN.md item 12.E.6).

    python3 live.py <server binary> [<byte_order> <real_bytes>]

`sim/run-sim.sh` builds the maker's library without ROS into `sim/server.cpp`, in each of the four ways a controller's build can spell an int and a real, and runs this against each. The server's
sockets, framing, codecs, message dispatch, ping handler and reconnect after a client goes are the maker's code; what is written here is the arm behind them. So the other end of the wire is an
independent implementation of the protocol, written by the people who wrote it, and not this repository's.

The second opinion has two parts that share nothing with the driver: **the server's log**, in which every message the motion server received is decoded by the maker's `JointTrajPt::unload` and
written as one JSON object (what the driver put on the wire, as the maker's code reads it), and **the server's own arm**, asked over a control port of its own for where the arm is.

What it cannot show: a robot. The arm is a straight-line interpolator behind the maker's sockets, and what is measured is the driver's session, framing, sequence discipline and stop against the
maker's stack, not any controller's motion planner, safety circuit or alarm table.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import socket
import subprocess
import sys
import tempfile
import time

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(ROOT, "..", "..", "bindings", "python"))
sys.path.insert(0, ROOT)

import skynet_rhal as rhal

from skynet_ros_industrial.bridge import RosIndustrialBridge
from skynet_ros_industrial.controller import SIX_JOINT_LIMITS, SIX_JOINT_MAX_SPEED

CHECKS = 0
FAILURES: list[str] = []
ROLES = ("right_arm.shoulder.yaw", "right_arm.shoulder.pitch", "right_arm.elbow.flex", "right_arm.wrist.pitch", "right_arm.wrist.yaw", "right_arm.wrist.roll")


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    print(("  ok    " if held else "  FAIL  ") + what, flush=True)
    if not held:
        FAILURES.append(what)
        if detail:
            print(f"        {detail}", flush=True)


def section(title: str) -> None:
    print(f"\n{title}", flush=True)


def until(predicate, timeout: float = 3.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.01)
    return bool(predicate())


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class Server:
    """The maker's server as a subprocess, its log, and its control port."""

    def __init__(self, binary: str, *extra: str) -> None:
        self.ports = {"motion": free_port(), "state": free_port(), "control": free_port()}
        self.log_path = tempfile.mktemp(prefix="rosi-", suffix=".log")
        self.process = subprocess.Popen(
            [binary, "--motion", str(self.ports["motion"]), "--state", str(self.ports["state"]), "--control", str(self.ports["control"]), "--log", self.log_path, *extra],
            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            try:
                socket.create_connection(("127.0.0.1", self.ports["control"]), timeout=0.2).close()
                break
            except OSError:
                time.sleep(0.05)
        else:
            raise SystemExit("live: the maker's server never listened on its control port")
        self._control = socket.create_connection(("127.0.0.1", self.ports["control"]), timeout=2.0)
        self._buffer = b""

    def ask(self, line: str) -> str:
        self._control.sendall(line.encode() + b"\n")
        while b"\n" not in self._buffer:
            self._buffer += self._control.recv(512)
        reply, _, self._buffer = self._buffer.partition(b"\n")
        return reply.decode()

    def arm(self) -> tuple[list[float], bool]:
        reply = self.ask("positions")
        text, _, moving = reply.rpartition(" ")
        return json.loads(text), moving == "1"

    def events(self) -> list[dict]:
        try:
            with open(self.log_path, encoding="utf-8") as handle:
                return [json.loads(line) for line in handle if line.strip()]
        except FileNotFoundError:
            return []

    def points(self) -> list[dict]:
        return [e for e in self.events() if e["event"] == "point"]

    def stop(self) -> None:
        try:
            self.ask("quit")
        except OSError:
            pass
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.kill()


def config(server: Server, byte_order: str, real_bytes: int, **extra) -> str:
    settings = {
        "host": "127.0.0.1", "ports": {"motion": server.ports["motion"], "state": server.ports["state"]}, "simulator": True, "byte_order": byte_order, "real_bytes": real_bytes,
        "rate_hz": 10, "point_duration_s": 0.2,
        "joints": [{"name": f"joint_{i + 1}", "role": ROLES[i], "min_rad": SIX_JOINT_LIMITS[i][0], "max_rad": SIX_JOINT_LIMITS[i][1], "max_speed_rad_s": SIX_JOINT_MAX_SPEED} for i in range(6)],
    }
    settings.update(extra)
    return json.dumps(settings)


def cmd(values: list[float]) -> rhal.Command:
    return rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=values, mask=[1] * len(values))


def state(bridge: RosIndustrialBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def write(bridge: RosIndustrialBridge, values: list[float]) -> tuple[int, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(cmd(values), 0)
    return code, said.getvalue()


def single(value: float) -> float:
    import struct
    return struct.unpack("<f", struct.pack("<f", value))[0]


def run(binary: str, byte_order: str, real_bytes: int) -> None:
    flags = []
    label = f"{byte_order}-endian, {real_bytes}-byte reals"
    server = Server(binary)
    try:
        section(f"the maker's server, built for a {label} controller")
        bridge = RosIndustrialBridge()
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.open(config(server, byte_order, real_bytes))
        check(code == rhal.SKR_OK, "the driver opens: the maker's ping handler answered, and its feedback and status arrived and were read", said.getvalue())
        if code != rhal.SKR_OK:
            return
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        code, first = state(bridge)
        truth, _ = server.arm()
        check(code == rhal.SKR_OK and all(abs(a - b) < 1e-3 for a, b in zip(first.position_rad, truth)), "the driver's positions are the server's arm, which the maker's codec encoded and this driver decoded")
        check(len(first.velocity_rad_s) == 6 and not any(math.isnan(v) for v in first.velocity_rad_s) and first.flags == rhal.HOMING_NOT_REQUIRED, "with the velocities the feedback said were valid, and a healthy state")
        descriptor = json.loads(bridge.describe())
        check(descriptor["dof"] == 6 and descriptor["simulator"] is True and descriptor["control"]["rate_hz_max"] == 10.0, "and the descriptor is what the configuration asserted")

        section("a move: what the maker's code reads off the wire")
        target = [0.3, -0.2, 0.4, -0.3, 0.2, -0.1]
        began = len(server.points())
        goals = [[t * (i + 1) / 10 for t in target] for i in range(10)]
        codes = []
        for goal in goals:
            codes.append(write(bridge, goal)[0])
            time.sleep(0.1)
        check(codes == [rhal.SKR_OK] * 10, "ten points at 10 Hz, each a step inside a joint's speed ceiling, are accepted")
        until(lambda: len(server.points()) >= began + 10)
        points = server.points()[began:began + 10]
        check([p["seq"] for p in points] == list(range(10)), "the maker's code reads them as a trajectory: sequence 0 to 9, in order")
        check(all(abs(p["joints"][j] - single(goals[i][j])) < 1e-6 or abs(p["joints"][j] - goals[i][j]) < 1e-9 for i, p in enumerate(points) for j in range(6)), "each carrying the six joint targets that were commanded")
        check(all(abs(p["duration"] - 0.2) < 1e-6 for p in points) and all(0.0 < p["velocity"] <= 1.0 for p in points), "and the configured duration and a velocity ratio between zero and one")
        expected = max(abs(goals[1][j] - goals[0][j]) for j in range(6)) / (0.2 * SIX_JOINT_MAX_SPEED)
        check(abs(points[1]["velocity"] - expected) < 1e-4, f"the ratio is the maker's own arithmetic: the largest step over the duration and the speed ceiling ({points[1]['velocity']:.4f})")
        check(until(lambda: all(abs(a - b) < 0.01 for a, b in zip(server.arm()[0], target)), 4.0), "and the server's arm ends at the target")
        until(lambda: all(abs(a - b) < 0.01 for a, b in zip(state(bridge)[1].position_rad, target)))
        check(all(abs(a - b) < 0.01 for a, b in zip(state(bridge)[1].position_rad, target)), "which the driver reads")
        before = len(server.points())
        for what, values in (("above a joint's range", [3.0, 0, 0, 0, 0, 0]), ("a step no joint can make", [0.3 + 1.5, -0.2, 0.4, -0.3, 0.2, -0.1])):
            check(write(bridge, values)[0] == rhal.SKR_EINVAL, f"a setpoint {what} is refused")
        time.sleep(0.2)
        check(len(server.points()) == before, "and the maker's code never saw either: nothing was sent")

        section("a gap: the maker's server answers FAILURE, and the driver starts a new trajectory")
        # The queue is eight deep and a point lasts at least its duration, so a burst of long-duration points fills it.
        bridge.close()
        server.ask("place 0 0 0 0 0 0")
        said = io.StringIO()
        began_open = time.monotonic()
        with contextlib.redirect_stderr(said):
            code = bridge.open(config(server, byte_order, real_bytes, point_duration_s=2.0, rate_hz=1))
        waited = time.monotonic() - began_open
        check(code == rhal.SKR_OK and waited < 8.0, f"a new session opens on the same server after the last one's stop and close, {waited:.1f} s later: the maker's own server sleeps five seconds before it accepts the next client (F241)", said.getvalue())
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        told, results = "", []
        gap_from = len(server.points())
        for i in range(14):
            code, said_ = write(bridge, [0.01 * (i + 1), 0, 0, 0, 0, 0])
            results.append(code)
            told += said_
            time.sleep(0.05)
        check(rhal.SKR_EIO in results and "FAILURE" in told, "a full queue is answered with FAILURE by the maker's handler and told to the driver's next command")
        time.sleep(0.3)
        seqs = [p["seq"] for p in server.points()[gap_from:]]
        restart = seqs.index(0, 1) if 0 in seqs[1:] else -1
        check(seqs[0] == 0 and restart > 0 and seqs[:restart] == list(range(restart)), "and what the driver sent next is sequence 0 again: a new trajectory, with the old one's gap behind it", f"{seqs}")

        section("the stop, as the maker's code reads it")
        bridge.close()
        server.ask("place 0 0 0 0 0 0")
        with contextlib.redirect_stderr(io.StringIO()):
            code = bridge.open(config(server, byte_order, real_bytes, point_duration_s=1.0, rate_hz=1))
        check(code == rhal.SKR_OK, "a third session opens too")
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        write(bridge, [0.5, 0, 0, 0, 0, 0])
        until(lambda: server.arm()[1])
        began_at = time.monotonic()
        check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began_at < 0.05, f"the stop returns in {(time.monotonic() - began_at) * 1000:.1f} ms with the arm moving")
        check(until(lambda: any(e["event"] == "stop" for e in server.events())), "and the maker's code reads it as STOP_TRAJECTORY")
        stop_events = [e for e in server.events() if e["event"] == "point" and e["seq"] == -4]
        check(bool(stop_events) and stop_events[-1]["velocity"] == 0.0, "(sequence -4, at no speed)")
        check(until(lambda: not server.arm()[1], 1.0), "the server flushed its queue and the arm is not moving")
        rest, _ = server.arm()
        time.sleep(0.4)
        check(all(abs(a - b) < 1e-6 for a, b in zip(rest, server.arm()[0])), "and it stays where it stopped")
        code, held = state(bridge)
        check(bool(held.flags & rhal.FLAG_ESTOP), "the driver's state says stopped")
        check(write(bridge, [0.1, 0, 0, 0, 0, 0])[0] == rhal.SKR_EPERM, "a command after the stop is refused")
        check(bridge.release_stop() == rhal.SKR_OK, "and release_stop runs the arm again")
        mark = len(server.points())
        check(write(bridge, [rest[0] + 0.01, 0, 0, 0, 0, 0])[0] == rhal.SKR_OK, "a command is accepted")
        until(lambda: len(server.points()) > mark)
        check(server.points()[-1]["seq"] == 0, "as a new trajectory, from where the arm stopped")

        section("the controller's own state, which the driver reads and never clears")
        server.ask("fault 4021")
        until(lambda: state(bridge)[1].fault_code == 4021)
        code, faulted = state(bridge)
        check(faulted.fault_code == 4021 and faulted.flags & rhal.FLAG_FAULT and faulted.flags & rhal.FLAG_DEGRADED, "an error on the controller is FAULT and DEGRADED, with the controller's code")
        check(write(bridge, [0.0] * 6)[0] == rhal.SKR_EPERM, "and takes no command")
        bridge.close()
        with contextlib.redirect_stderr(io.StringIO()) as said:
            code = RosIndustrialBridge().open(config(server, byte_order, real_bytes))
        check(code == rhal.SKR_ENODEV, "a new session is refused over the error, which is the evidence of why the arm stopped")
        server.ask("clear")
        server.ask("estop on")
        time.sleep(0.3)
        with contextlib.redirect_stderr(io.StringIO()) as said:
            code = RosIndustrialBridge().open(config(server, byte_order, real_bytes))
        check(code == rhal.SKR_ENODEV, "and over an e-stop")
        server.ask("estop off")
        server.ask("drives off")
        time.sleep(0.3)
        with contextlib.redirect_stderr(io.StringIO()) as said:
            code = RosIndustrialBridge().open(config(server, byte_order, real_bytes))
        check(code == rhal.SKR_ENODEV, "and over drives that are off")
        server.ask("drives on")
        time.sleep(0.3)

        section("a session, and the next one on the same server (the maker's fault handler reconnects)")
        bridge = RosIndustrialBridge()
        with contextlib.redirect_stderr(io.StringIO()):
            check(bridge.open(config(server, byte_order, real_bytes)) == rhal.SKR_OK, "the driver opens again, on the connection the maker's server accepted after the last one went")
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        stops = sum(1 for e in server.events() if e["event"] == "stop")
        bridge.close()
        check(until(lambda: sum(1 for e in server.events() if e["event"] == "stop") > stops), "closing a session ends it with a stop trajectory the maker's code reads")
        flags.append(0)
    finally:
        server.stop()


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: live.py <server binary> [<byte_order> <real_bytes>]")
        return 2
    binary = sys.argv[1]
    byte_order = sys.argv[2] if len(sys.argv) > 2 else "little"
    real_bytes = int(sys.argv[3]) if len(sys.argv) > 3 else 4
    run(binary, byte_order, real_bytes)
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

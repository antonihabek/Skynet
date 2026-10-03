"""The Reachy Mini bridge against Pollen Robotics' own daemon running its own simulator (DRY-COVERAGE-PLAN.md item 12.E.12).

    python3 live.py <host> <port>

`sim/run-sim.sh` starts `reachy-mini-daemon --sim --headless --no-media` in the substrate image, whose `reachy_mini` package is the maker's own at a pinned commit, and runs this against it. The daemon, its
REST interface, its inverse kinematics and its MuJoCo model are the maker's; what is written here is the driver.

The second opinion shares nothing with the driver: **the daemon's own models**. Every state the driver reads is read again over plain HTTP and validated by the maker's `FullState`, and every claim about where the
head is goes through the daemon's own `with_target_head_pose` and present pose, which is the physics' idea of where it is and the daemon's idea of where it was told to go.

What it cannot show: a robot. The simulator is MuJoCo with the daemon's own kinematics, with no motor that overheats, no cable and no hand on the head. What is measured is the driver's session, commands and stop
against the maker's REST interface, not a robot's firmware, torque or safety.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import os
import sys
import time
import urllib.error
import urllib.request

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(ROOT, "..", "..", "bindings", "python"))
sys.path.insert(0, ROOT)

import skynet_rhal as rhal

from skynet_reachy_mini import frames
from skynet_reachy_mini.bridge import ReachyMiniBridge

CHECKS = 0
FAILURES: list[str] = []
NAMES = [name for name, _ in frames.JOINTS]
LIMITS = {"head_x": [-0.01, 0.01], "head_y": [-0.01, 0.01], "head_z": [-0.01, 0.01], "antenna_right": [-1.0, 1.0], "antenna_left": [-1.0, 1.0]}
SPEEDS = {"head_x": 0.05, "head_y": 0.05, "head_z": 0.05, "head_roll": 0.5, "head_pitch": 0.5, "head_yaw": 0.5, "body_yaw": 0.5, "antenna_right": 1.0, "antenna_left": 1.0}
RATE_HZ = 20.0


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


def until(predicate, timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # noqa: BLE001 - a predicate that cannot be evaluated yet is not true yet
            pass
        time.sleep(0.02)
    return bool(predicate())


class Daemon:
    """The maker's daemon over plain HTTP, and its own pydantic models when they can be imported."""

    def __init__(self, host: str, port: int) -> None:
        self.base = f"http://{host}:{port}"
        try:
            from reachy_mini.daemon.app.models import FullState
            from reachy_mini.io.protocol import DaemonStatus
        except Exception:  # noqa: BLE001
            self.FullState = self.DaemonStatus = None
        else:
            self.FullState, self.DaemonStatus = FullState, DaemonStatus

    def get(self, path: str):
        with urllib.request.urlopen(self.base + path, timeout=5) as response:
            return json.loads(response.read())

    def post(self, path: str, body=None):
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(self.base + path, data=data, method="POST", headers={"Content-Type": "application/json"})
        with urllib.request.urlopen(request, timeout=5) as response:
            return json.loads(response.read() or b"null")

    def present(self) -> list[float]:
        document = self.get("/api/state/full?with_target_head_pose=true&with_target_body_yaw=true&with_target_antenna_positions=true")
        if self.FullState is not None:
            self.FullState.model_validate(document)
        return list(frames.parse_state(document).positions)

    def target(self) -> dict:
        return self.get("/api/state/full?with_target_head_pose=true&with_target_body_yaw=true&with_target_antenna_positions=true")


def config(host: str, port: int, **extra) -> str:
    settings = {
        "host": host, "port": port, "stop_mode": "hold", "limits": LIMITS, "speeds": SPEEDS, "rate_hz": RATE_HZ,
        "roles": {"head_roll": "head.platform.roll", "head_pitch": "head.platform.pitch", "head_yaw": "head.platform.yaw", "body_yaw": "torso.body.yaw", "antenna_right": "aux.antenna_right.rotate", "antenna_left": "aux.antenna_left.rotate"},
    }
    settings.update(extra)
    return json.dumps(settings)


def cmd(values: list[float], mask: list[int] | None = None) -> rhal.Command:
    return rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=values, mask=mask or [1] * len(values))


def state(bridge: ReachyMiniBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def at(**given: float) -> list[float]:
    return [given.get(name, 0.0) for name in NAMES]


def drive_to(bridge: ReachyMiniBridge, goal: list[float], seconds: float) -> list[int]:
    """Stream commands at the configured rate towards `goal`, each step inside every joint's speed, and return the codes."""
    codes = []
    period = 1.0 / RATE_HZ
    held = list(bridge._held)
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        step = [max(h - SPEEDS[name] * period * 0.9, min(h + SPEEDS[name] * period * 0.9, g)) for name, h, g in zip(NAMES, held, goal)]
        out = io.StringIO()
        with contextlib.redirect_stderr(out):
            codes.append(bridge.write_command(cmd(step), 0))
        held = list(bridge._held)
        time.sleep(period)
    return codes


def main(host: str, port: int) -> int:
    daemon = Daemon(host, port)

    section("the maker's daemon, by its own models")
    status = daemon.get("/api/daemon/status")
    if daemon.DaemonStatus is not None:
        daemon.DaemonStatus.model_validate(status)
    check(status.get("state") == "running" and status.get("simulation_enabled") is True, f"it says it is running and a simulation (version {status.get('version')})")
    check(daemon.FullState is not None, "and the maker's own models are importable here, so what the daemon says is validated by them")
    start = daemon.present()
    check(len(start) == 9 and all(math.isfinite(v) for v in start), "its state is nine finite numbers in this driver's order", str(start))

    section("open: the daemon's own word about what it is")
    bridge = ReachyMiniBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(host, port))
    check(code == rhal.SKR_OK, "the driver opens", said.getvalue())
    if code != rhal.SKR_OK:
        return finish()
    descriptor = json.loads(bridge.describe())
    check(descriptor["simulator"] is True and descriptor["dof"] == 9, "its descriptor says simulator, from the daemon's own `simulation_enabled`, and nine joints")
    until(lambda: state(bridge)[0] == rhal.SKR_OK)
    code, first = state(bridge)
    truth = daemon.present()
    check(code == rhal.SKR_OK and all(abs(a - b) < 5e-3 for a, b in zip(first.position_rad, truth)), "the driver's positions are the daemon's, read again over HTTP and validated by the maker's FullState")
    check(first.flags == rhal.HOMING_NOT_REQUIRED and first.fault_code == 0, "a healthy robot reads clean")

    section("a move: where the maker's daemon and its physics say the head went")
    goal = at(head_pitch=0.25, head_yaw=0.2, body_yaw=0.3, antenna_right=0.4, antenna_left=-0.4, head_z=0.005)
    codes = drive_to(bridge, goal, 4.0)
    check(all(c == rhal.SKR_OK for c in codes), f"{len(codes)} commands at {RATE_HZ:g} Hz, each inside every speed, are accepted", str(codes[:10]))
    sent = daemon.target()
    pose = sent.get("target_head_pose") or {}
    check(abs(pose.get("pitch", 9) - goal[4]) < 1e-6 and abs(sent.get("target_body_yaw", 9) - goal[6]) < 1e-6 and list(sent.get("target_antennas_position") or [9, 9]) == [goal[7], goal[8]],
          "the daemon's own target is the one the driver sent: head pitch, body yaw and the antennas, right then left")
    settled = until(lambda: all(abs(a - b) < 0.03 for a, b in zip(daemon.present(), goal)), 5.0)
    check(settled, "and the daemon's simulated robot gets there", f"{[round(v, 3) for v in daemon.present()]} against {goal}")
    until(lambda: all(abs(a - b) < 0.03 for a, b in zip(state(bridge)[1].position_rad, goal)), 3.0)
    check(all(abs(a - b) < 0.03 for a, b in zip(state(bridge)[1].position_rad, goal)), "which the driver reads")

    section("refusals the daemon would have turned into clamps")
    before = daemon.target()
    out = io.StringIO()
    with contextlib.redirect_stderr(out):
        beyond = bridge.write_command(cmd(at(head_pitch=0.25 + 0.05, head_roll=math.radians(41), head_yaw=0.2, body_yaw=0.3, antenna_right=0.4, antenna_left=-0.4, head_z=0.005)), 0)
    check(beyond == rhal.SKR_EINVAL, "a roll past the documented 40 degrees is refused by the driver")
    after = daemon.target()
    check(before.get("target_head_pose") == after.get("target_head_pose"), "and the daemon never saw it: its target is where it was")
    with contextlib.redirect_stderr(out):
        yaw = bridge.write_command(cmd(at(head_pitch=0.25, head_yaw=0.2 + 0.0, body_yaw=0.3 - 0.45, antenna_right=0.4, antenna_left=-0.4, head_z=0.005)), 0)
    check(yaw == rhal.SKR_EINVAL, "a step the body yaw's speed does not allow in one period is refused")

    section("the stop: told to the daemon, read back from the daemon")
    drive_to(bridge, at(head_pitch=-0.2, body_yaw=-0.3), 0.6)
    began = time.monotonic()
    check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.05, f"the stop returns in {(time.monotonic() - began) * 1000:.1f} ms with the head moving")
    check(until(lambda: _target_is_present(daemon), 3.0), "and the daemon's own target is where its robot is: it was told to hold")
    rest = daemon.present()
    time.sleep(0.6)
    check(all(abs(a - b) < 2e-3 for a, b in zip(rest, daemon.present())), "and the robot stays there")
    check(bool(state(bridge)[1].flags & rhal.FLAG_ESTOP), "the driver's state says stopped")
    check(bridge.write_command(cmd(rest), 0) == rhal.SKR_EPERM, "a command after the stop is refused")
    check(bridge.release_stop() == rhal.SKR_OK, "and release_stop runs the robot again")
    until(lambda: state(bridge)[0] == rhal.SKR_OK)
    check(bridge.write_command(cmd([min(max(v, -1.0), 1.0) for v in rest]), 0) == rhal.SKR_OK, "a command is accepted")

    section("somebody else's move: the daemon ignores targets while one runs")
    daemon.post("/api/move/goto", {"head_pose": {"x": 0, "y": 0, "z": 0, "roll": 0, "pitch": 0.1, "yaw": 0}, "antennas": [0, 0], "body_yaw": 0, "duration": 6.0})
    check(until(lambda: bool(daemon.get("/api/move/running")), 2.0), "a move another client started is running on the daemon")
    out = io.StringIO()
    with contextlib.redirect_stderr(out):
        code = bridge.write_command(cmd(rest), 0)
    check(code == rhal.SKR_EPERM and "ignored" in out.getvalue(), "the daemon answers a target with `ignored`, and the driver tells the caller the command did not happen")
    bridge.emergency_stop()
    check(until(lambda: not daemon.get("/api/move/running"), 4.0), "and the stop cancels the move the daemon is running: a head being sent somewhere is not stopped by a target it ignores")
    bridge.release_stop()
    bridge.close()
    with contextlib.redirect_stderr(out):
        daemon.post("/api/move/goto", {"head_pose": {"x": 0, "y": 0, "z": 0, "roll": 0, "pitch": -0.1, "yaw": 0}, "antennas": [0, 0], "body_yaw": 0, "duration": 6.0})
        refused = ReachyMiniBridge().open(config(host, port))
    check(refused == rhal.SKR_ENODEV, "a new session is refused while somebody else's move runs: two masters is one that is ignored without a word")
    for move in daemon.get("/api/move/running"):
        daemon.post("/api/move/stop", move)

    section("limp: the stop that lets go, and the release that takes hold again")
    limp = ReachyMiniBridge()
    with contextlib.redirect_stderr(out):
        limp.open(config(host, port, stop_mode="limp"))
    until(lambda: state(limp)[0] == rhal.SKR_OK)
    limp.emergency_stop()
    check(until(lambda: daemon.get("/api/motors/status").get("mode") == "disabled", 3.0), "`limp` takes the motors' torque away, as the maker's own motor status says")
    check(limp.release_stop() == rhal.SKR_OK and daemon.get("/api/motors/status").get("mode") == "enabled", "and a release enables them again")
    limp.close()
    check(daemon.get("/api/motors/status").get("mode") == "enabled", "a close never leaves them limp")

    section("a session, and the next one")
    again = ReachyMiniBridge()
    with contextlib.redirect_stderr(out):
        check(again.open(config(host, port)) == rhal.SKR_OK, "the driver opens again on the same daemon")
    again.close()
    return finish()


def _target_is_present(daemon: Daemon) -> bool:
    document = daemon.target()
    head, present = document.get("target_head_pose") or {}, document.get("head_pose") or {}
    return all(abs(head.get(k, 9) - present.get(k, -9)) < 0.01 for k in ("x", "y", "z", "roll", "pitch", "yaw")) and abs(document.get("target_body_yaw", 9) - document.get("body_yaw", -9)) < 0.01


def finish() -> int:
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1", int(sys.argv[2]) if len(sys.argv) > 2 else 8000))

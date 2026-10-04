"""The Reachy Mini bridge against Pollen Robotics' own daemon running its own simulator (DRY-COVERAGE-PLAN.md item 12.E.12).

    python3 live.py <host> <port>

`sim/run-sim.sh` starts `reachy-mini-daemon --sim --headless --no-media` in the substrate image, whose `reachy_mini` package is the maker's own at a pinned commit, and runs this against it. The daemon, its
REST interface, its inverse kinematics and its MuJoCo model are the maker's; what is written here is the driver.

The second opinion shares nothing with the driver: **the daemon's own models, physics and web server**. Every state the driver reads is read again over plain HTTP and validated by the maker's `FullState`; every claim about
where the head is is the physics' present pose; and every claim about **what the driver sent** is the maker's own access log (`REACHY_DAEMON_LOG`), because the daemon's REST interface cannot be asked what it was told
to go to (F245: `with_target_head_pose` answers 500 before any command and, after one, returns nothing, as `FullState` has no target fields).

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
from skynet_reachy_mini.__main__ import ROLES
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


def note(what: str) -> None:
    """A thing that was not measured, said where it would have been: the block is not allowed to look as if it had."""
    print(f"  --    not measured: {what}", flush=True)


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
        """Where the robot is: the route's own default (control mode, head pose, body yaw, antennas), which the daemon serves from the moment it runs.

        It must not ask for the target too. A daemon that has not been commanded has none, and `GET /api/state/full?with_target_head_pose=true` then answers 500 (`assert target_pose is not None` in the maker's
        `routers/state.py`): found by the first run against the image whose meshes were real, when this helper did ask and the block died before its first check.
        """
        document = self.get("/api/state/full")
        if self.FullState is not None:
            self.FullState.model_validate(document)
        return list(frames.parse_state(document).positions)

    def requests(self, method: str, path: str) -> int:
        """How many requests the maker's server answered 200 for a route, from its own access log (`REACHY_DAEMON_LOG`, which `sim/run-sim.sh` sets), or -1 when no log was given.

        This is the second opinion on **what the driver sent**. The daemon's REST interface cannot be asked what it was last told to go to: `GET /api/state/full?with_target_head_pose=true` is the route that
        looks as if it could, and it answers 500 before any command (`assert target_pose is not None`) and, after one, drops every `target_*` field, because the maker's `FullState` model has none
        (DRY-COVERAGE-PLAN.md F245). What the maker's own web server received is what is left that shares nothing with the driver.
        """
        log = os.environ.get("REACHY_DAEMON_LOG")
        if not log or not os.path.exists(log):
            return -1
        needle = f'"{method} {path} HTTP/1.1"'
        with open(log, errors="replace") as handle:
            return sum(1 for line in handle if "uvicorn.access" in line and needle in line and line.rstrip().endswith(" 200"))

    def last_motor_mode(self) -> str | None:
        """The motor mode the maker's server was last asked for, in the order its access log has them, or None if it was never asked (or there is no log)."""
        log = os.environ.get("REACHY_DAEMON_LOG")
        if not log or not os.path.exists(log):
            return None
        last = None
        marker = '"POST /api/motors/set_mode/'
        with open(log, errors="replace") as handle:
            for line in handle:
                if "uvicorn.access" in line and marker in line and line.rstrip().endswith(" 200"):
                    last = line.split(marker, 1)[1].split(" ", 1)[0]
        return last


def config(host: str, port: int, **extra) -> str:
    settings = {
        "host": host, "port": port, "stop_mode": "hold", "limits": LIMITS, "speeds": SPEEDS, "rate_hz": RATE_HZ,
        "roles": dict(ROLES),
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


def still(daemon: "Daemon", timeout: float = 8.0, tolerance: float = 1e-3, gap: float = 0.3) -> bool:
    """Wait until the maker's simulated robot has stopped creeping: two reads `gap` apart agree to `tolerance` on every joint. The maker's physics goes on settling for a while after a move ends."""
    deadline = time.monotonic() + timeout
    last = daemon.present()
    while time.monotonic() < deadline:
        time.sleep(gap)
        now = daemon.present()
        if all(abs(a - b) < tolerance for a, b in zip(last, now)):
            return True
        last = now
    return False


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

    section("the robot as the maker's daemon starts it: asleep, which is outside the ranges a session is given")
    asleep = ReachyMiniBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = asleep.open(config(host, port))
    asleep.close()
    check(code == rhal.SKR_ENODEV and "head_z" in said.getvalue() and "antenna_right" in said.getvalue() and "wake_up" in said.getvalue(),
          "a session whose ranges are narrower than the sleeping pose is refused at open, naming the joints and the maker's own wake-up (before this, every command was refused and not a word said)", said.getvalue())
    daemon.post("/api/move/play/wake_up")
    check(until(lambda: not daemon.get("/api/move/running"), 10.0), "the operator wakes it with the maker's own route, and the move ends")
    check(still(daemon), "and the maker's simulated robot settles")
    awake = daemon.present()
    check(all(LIMITS[name][0] <= awake[NAMES.index(name)] <= LIMITS[name][1] for name in LIMITS), "and the awake pose is inside every range the session declares", str([round(v, 3) for v in awake]))

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
    posts = daemon.requests("POST", "/api/move/set_target")
    codes = drive_to(bridge, goal, 4.0)
    check(all(c == rhal.SKR_OK for c in codes), f"{len(codes)} commands at {RATE_HZ:g} Hz, each inside every speed, are accepted", str(codes[:10]))
    time.sleep(0.3)
    if posts >= 0:
        logged = daemon.requests("POST", "/api/move/set_target") - posts
        check(logged == len(codes), f"and the maker's own server logged {logged} `set_target` requests for {len(codes)} accepted commands: one each, and nothing else")
    else:
        note("what the maker's server received (no access log: REACHY_DAEMON_LOG is not set)")
    settled = until(lambda: all(abs(a - b) < 0.03 for a, b in zip(daemon.present(), goal)), 5.0)
    check(settled, "and the daemon's simulated robot gets there", f"{[round(v, 3) for v in daemon.present()]} against {goal}")
    until(lambda: all(abs(a - b) < 0.03 for a, b in zip(state(bridge)[1].position_rad, goal)), 3.0)
    check(all(abs(a - b) < 0.03 for a, b in zip(state(bridge)[1].position_rad, goal)), "which the driver reads")

    section("refusals the daemon would have turned into clamps")
    posts = daemon.requests("POST", "/api/move/set_target")
    out = io.StringIO()
    with contextlib.redirect_stderr(out):
        beyond = bridge.write_command(cmd(at(head_pitch=0.25 + 0.05, head_roll=math.radians(41), head_yaw=0.2, body_yaw=0.3, antenna_right=0.4, antenna_left=-0.4, head_z=0.005)), 0)
    check(beyond == rhal.SKR_EINVAL, "a roll past the documented 40 degrees is refused by the driver")
    check("head_roll" in out.getvalue() and "range" in out.getvalue(), "and it says which joint and why, where it used to say nothing", out.getvalue())
    out = io.StringIO()
    with contextlib.redirect_stderr(out):
        yaw = bridge.write_command(cmd(at(head_pitch=0.25, head_yaw=0.2 + 0.0, body_yaw=0.3 - 0.45, antenna_right=0.4, antenna_left=-0.4, head_z=0.005)), 0)
    check(yaw == rhal.SKR_EINVAL, "a step the body yaw's speed does not allow in one period is refused")
    check("body_yaw" in out.getvalue() and "one period" in out.getvalue(), "saying which joint and what its speed allows", out.getvalue())
    time.sleep(0.3)
    if posts >= 0:
        check(daemon.requests("POST", "/api/move/set_target") == posts, "and the maker's server never saw either of them: a daemon that would have clamped a request was never sent one")
    else:
        note("that the maker's server never saw them (no access log)")

    section("the stop: told to the daemon, and the robot where the maker's physics put it")
    drive_to(bridge, at(head_pitch=-0.2, body_yaw=-0.3), 0.6)
    posts = daemon.requests("POST", "/api/move/set_target")
    began = time.monotonic()
    check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.05, f"the stop returns in {(time.monotonic() - began) * 1000:.1f} ms with the head moving")
    if posts >= 0:
        check(until(lambda: daemon.requests("POST", "/api/move/set_target") > posts, 3.0), "and the maker's server logged the hold: a target at the head's own pose, which is what `hold` is")
    else:
        note("that the maker's server logged the hold (no access log)")
    time.sleep(1.0)
    rest = daemon.present()
    check(rest[4] > -0.1, "the head did not go on to the pitch the stream was driving it to (-0.2): the stop interrupted the move", f"pitch {rest[4]:.3f}")
    check(still(daemon, timeout=8.0, tolerance=2e-3, gap=0.6), "and the robot stays where it stopped: it settles, and is not still going anywhere", str([round(v, 4) for v in daemon.present()]))
    check(all(abs(a - b) < 0.03 for a, b in zip(rest, daemon.present())), "within 0.03 of where it was a second after the stop", str([round(b - a, 4) for a, b in zip(rest, daemon.present())]))
    rest = daemon.present()
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
    disabled = daemon.requests("POST", "/api/motors/set_mode/disabled")
    limp.emergency_stop()
    if disabled >= 0:
        check(until(lambda: daemon.requests("POST", "/api/motors/set_mode/disabled") > disabled, 3.0), "`limp` asks the maker's server to disable the motors: it logged `POST /api/motors/set_mode/disabled`")
    else:
        note("that the maker's server was asked to disable the motors (no access log)")
    # What the maker's simulator cannot show. `MujocoBackend.get_motor_control_mode` returns `Enabled` whatever was asked and `set_motor_control_mode` is `pass` (`daemon/backend/mujoco/backend.py`, at the
    # pinned commit), so the status route reads `enabled` through a limp stop. The state of a limp head is the real robot's backend's to report (`robot/backend.py` implements it) and is not measured here: what is
    # held is the request the server received. If this reads `disabled`, the simulator has learnt to model torque and this block should check the state itself.
    check(daemon.get("/api/motors/status").get("mode") == "enabled", "the maker's simulator does not model torque, so its motor status cannot show a limp head: it reads `enabled` through the stop")
    enabled = daemon.requests("POST", "/api/motors/set_mode/enabled")
    check(limp.release_stop() == rhal.SKR_OK, "a release is accepted")
    if enabled >= 0:
        check(until(lambda: daemon.requests("POST", "/api/motors/set_mode/enabled") > enabled, 3.0), "and the maker's server logged the request to enable the motors again")
    limp.close()
    if disabled >= 0:
        check(daemon.last_motor_mode() == "enabled", "a close never leaves them limp: the last motor mode the maker's server was asked for is `enabled`", str(daemon.last_motor_mode()))

    section("a session, and the next one")
    again = ReachyMiniBridge()
    with contextlib.redirect_stderr(out):
        check(again.open(config(host, port)) == rhal.SKR_OK, "the driver opens again on the same daemon")
    again.close()
    return finish()


def finish() -> int:
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1", int(sys.argv[2]) if len(sys.argv) > 2 else 8000))

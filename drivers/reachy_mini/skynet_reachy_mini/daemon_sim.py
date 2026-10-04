"""A simulated Reachy Mini daemon, written here: the dry substrate for `selftest.py` and for `skynet driver check`.

It speaks the routes `frames.py` lists, with the shapes the maker's models give them, and moves its nine values towards their targets at the speeds a test sets. What is *this file's choice* and not the
maker's: the dynamics (a straight line at a speed ceiling per joint, nothing of a Stewart platform's workspace or a motor's torque), what `disabled` does to a pose (nothing: it stops following), and that
the daemon clamps a target to the ranges below, as the maker's documentation says the SDK does and the daemon's own clamp was not run. `drop`, `error`, `external_move` and `delay_s` are how a test makes it behave the
way the real one does when something is wrong.
"""

from __future__ import annotations

import json
import math
import threading
import time
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from . import frames

__all__ = ["SimulatedDaemon"]

#: The ranges this daemon clamps to (this file's choice, for the ones the maker's documentation does not give: the head's translation and the antennas').
RANGES = {
    "head_x": (-0.03, 0.03), "head_y": (-0.03, 0.03), "head_z": (-0.02, 0.04),
    **{name: limits for name, limits in frames.DOCUMENTED_LIMITS.items()},
    "antenna_right": (-3.0, 3.0), "antenna_left": (-3.0, 3.0),
}
#: How fast each value follows its target, per second (this file's choice).
SPEEDS = {"head_x": 0.2, "head_y": 0.2, "head_z": 0.2, "head_roll": 3.0, "head_pitch": 3.0, "head_yaw": 3.0, "body_yaw": 3.0, "antenna_right": 6.0, "antenna_left": 6.0}


class SimulatedDaemon:
    def __init__(self) -> None:
        self.port = 0
        self._lock = threading.Lock()
        names = [name for name, _ in frames.JOINTS]
        self._names = names
        self._present = {name: 0.0 for name in names}
        self._target = dict(self._present)
        self._mode = frames.MotorMode.ENABLED
        self._stopping = False
        self._server: ThreadingHTTPServer | None = None
        self._connections: list = []
        #: What a test turns on: the daemon's own state and error, a move some other client started (its uuid), a drop of every request, a delay before every answer.
        self.state = frames.DaemonState.RUNNING
        self.error: str | None = None
        self.external_move: str | None = None
        self.drop = False
        #: The next this-many POSTs are lost on the way (the connection is closed and nothing is done): a link that loses a request, where `drop` is a daemon that has gone and `delay_s` is one that is slow. A request the
        #: daemon only answers late still arrives, which is why a retry is not shown by a delay.
        self.drop_posts = 0
        self.delay_s = 0.0
        self.simulation_enabled: bool | None = True
        self.mockup_sim_enabled = False
        self.requests: list[tuple[str, str, object]] = []
        self.targets: list[dict] = []
        self.stopped_moves: list[str] = []
        self.modes: list[str] = []

    # ── what a test does to it ───────────────────────────────────────────────

    def __enter__(self) -> SimulatedDaemon:
        return self.start()

    def __exit__(self, *exc: object) -> None:
        self.stop()

    def start(self) -> SimulatedDaemon:
        daemon = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args: object) -> None:  # the suite's output is its own
                pass

            def setup(self) -> None:
                super().setup()
                daemon._connections.append(self.connection)

            def _answer(self, status: int, document: object) -> None:
                body = json.dumps(document).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def _body(self) -> object:
                length = int(self.headers.get("Content-Length") or 0)
                return json.loads(self.rfile.read(length) or b"null") if length else None

            def do_GET(self) -> None:
                daemon._handle(self, "GET")

            def do_POST(self) -> None:
                daemon._handle(self, "POST")

        class Quiet(ThreadingHTTPServer):
            def handle_error(self, request, client_address) -> None:  # a client that resets its connection is what the suite does on purpose
                pass

        self._server = Quiet(("127.0.0.1", 0), Handler)
        self._server.daemon_threads = True
        self.port = self._server.server_address[1]
        threading.Thread(target=self._server.serve_forever, daemon=True, name="reachy-sim-http").start()
        threading.Thread(target=self._physics, daemon=True, name="reachy-sim-physics").start()
        return self

    def stop(self) -> None:
        self._stopping = True
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()

    def reset_connections(self) -> None:
        """Every connection a client holds is closed from this end, as a daemon that crashed and came back leaves them."""
        for connection in self._connections:
            try:
                connection.shutdown(2)
                connection.close()
            except OSError:
                pass
        self._connections = []

    def place(self, values: list[float]) -> None:
        with self._lock:
            for name, value in zip(self._names, values):
                self._present[name] = self._target[name] = value

    def present(self) -> list[float]:
        with self._lock:
            return [self._present[name] for name in self._names]

    def target(self) -> list[float]:
        with self._lock:
            return [self._target[name] for name in self._names]

    @property
    def mode(self) -> str:
        return self._mode

    def set_mode(self, mode: str) -> None:
        """A person (or the maker's app) setting the motors' mode at the robot: not a request this driver made, so it is not in `modes`."""
        self._mode = mode

    # ── the routes ───────────────────────────────────────────────────────────

    def _handle(self, handler: BaseHTTPRequestHandler, method: str) -> None:
        path = handler.path.split("?", 1)[0]
        body = handler._body() if method == "POST" else None  # type: ignore[attr-defined]
        self.requests.append((method, path, body))
        if self.drop:
            handler.close_connection = True
            return
        if method == "POST" and self.drop_posts > 0:
            self.drop_posts -= 1
            handler.close_connection = True
            return
        if self.delay_s:
            time.sleep(self.delay_s)
        answer = handler._answer  # type: ignore[attr-defined]
        if method == "GET" and path == frames.Route.DAEMON_STATUS:
            answer(200, {
                "type": "daemon_status", "robot_name": "reachy_mini", "state": self.state, "wireless_version": False, "desktop_app_daemon": False,
                "simulation_enabled": self.simulation_enabled, "mockup_sim_enabled": self.mockup_sim_enabled, "no_media": True,
                "backend_status": {"motor_control_mode": self._mode, "error": self.error}, "error": self.error, "version": "simulated"})
        elif method == "GET" and path == frames.Route.STATE_FULL:
            with self._lock:
                p = self._present
                answer(200, {
                    "control_mode": self._mode, "head_pose": {k: p[f"head_{k}"] for k in ("x", "y", "z", "roll", "pitch", "yaw")}, "body_yaw": p["body_yaw"],
                    "antennas_position": [p["antenna_right"], p["antenna_left"]], "timestamp": datetime.now(timezone.utc).isoformat()})
        elif method == "POST" and path == frames.Route.SET_TARGET:
            if self.external_move is not None:
                answer(200, {"status": "ignored", "reason": "move_running"})
                return
            self.targets.append(body if isinstance(body, dict) else {})
            with self._lock:
                self._apply(body)
            answer(200, {"status": "ok"})
        elif method == "GET" and path == frames.Route.RUNNING:
            answer(200, [] if self.external_move is None else [{"uuid": self.external_move}])
        elif method == "POST" and path == frames.Route.STOP_MOVE:
            uuid = body.get("uuid") if isinstance(body, dict) else None
            if uuid is not None and uuid == self.external_move:
                self.stopped_moves.append(uuid)
                self.external_move = None
                answer(200, {"message": f"Stopped move with UUID: {uuid}"})
            else:
                answer(404, {"detail": f"Running move with UUID {uuid} not found"})
        elif method == "GET" and path == frames.Route.MOTOR_STATUS:
            answer(200, {"mode": self._mode})
        elif method == "POST" and path.startswith(frames.API + "/motors/set_mode/"):
            mode = path.rsplit("/", 1)[1]
            if mode not in (frames.MotorMode.ENABLED, frames.MotorMode.DISABLED, frames.MotorMode.GRAVITY_COMPENSATION):
                answer(422, {"detail": "not a motor mode"})
                return
            self.modes.append(mode)
            self._mode = mode
            answer(200, {"status": f"motors changed to {mode} mode"})
        else:
            answer(404, {"detail": "not found"})

    def _apply(self, body: object) -> None:
        if not isinstance(body, dict):
            return
        pose = body.get("target_head_pose")
        if isinstance(pose, dict):
            for key in ("x", "y", "z", "roll", "pitch", "yaw"):
                if key in pose:
                    self._target[f"head_{key}"] = self._clamp(f"head_{key}", float(pose[key]))
        antennas = body.get("target_antennas")
        if isinstance(antennas, list) and len(antennas) == 2:
            self._target["antenna_right"] = self._clamp("antenna_right", float(antennas[0]))
            self._target["antenna_left"] = self._clamp("antenna_left", float(antennas[1]))
        if isinstance(body.get("target_body_yaw"), (int, float)):
            self._target["body_yaw"] = self._clamp("body_yaw", float(body["target_body_yaw"]))

    @staticmethod
    def _clamp(name: str, value: float) -> float:
        low, high = RANGES[name]
        return max(low, min(high, value))

    # ── the arm ──────────────────────────────────────────────────────────────

    def _physics(self) -> None:
        last = time.monotonic()
        while not self._stopping:
            time.sleep(0.005)
            now = time.monotonic()
            step, last = now - last, now
            with self._lock:
                if self._mode == frames.MotorMode.DISABLED:
                    continue
                for name in self._names:
                    gap = self._target[name] - self._present[name]
                    move = SPEEDS[name] * step
                    self._present[name] += gap if abs(gap) <= move else math.copysign(move, gap)

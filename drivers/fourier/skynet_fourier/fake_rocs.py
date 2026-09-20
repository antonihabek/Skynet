"""A fake RoCS server: the endpoints this driver speaks, on loopback HTTP+WebSocket.

Run by `run-check.sh` as a subprocess (ephemeral port, announced on stderr), or imported
by `selftest.py` and served on a thread. Either way the driver reaches it over real
sockets — `urllib` and `wscodec` to `http.server` — so the bytes the driver sends are
the bytes the fake verifies.

The wire shapes are the vendor's: endpoint paths, the `{code, msg, data}` envelope, the
WebSocket `{command, data}` messages and the string-typed PVC fields all come from
`fftai/rocs_client_py` (MIT), read — not copied — the way the Pudu fake reads the skill
repo. What is the fake's own is the motor set: six entries (`1`–`3` × `left`/`right`)
with distinct ranges and nonzero starts, documented here because no Fourier publication
names the GR-1's motor inventory. The driver discovers joints live off
`/robot/motor/limit/list`, so on a real robot it exposes the real set; the suite pins
behavior on the fake's.

Two vendor-doc examples are served verbatim, attributed: `get_joint_limit`'s four leg
joints (the docstring's radians) and `get_joint_states`' status shape. The pushed debug
states reuse the docstring's `jointStates` member names (`qa`, `qdota`, `taua`, `qc`,
`qdotc`, `tauc`). Where the vendor publishes nothing — the `sdk_ctrl/status` body, the
hand state — the fake serves a fixed documented shape and says so.

`move_joint` clamps to the fake's ranges and counts the clamp: the vendor's client
pre-clamps (`_cover_param`), the driver pre-clamps to the live limits, so a run in
which `clamped` is nonzero is a run in which somebody sent what the robot would not
take. `POST /robot/stop` counts separately: it is the driver's emergency path, and the
fake's count of it is the suite's proof the driver called it.

Counts print as one JSON line on SIGTERM, the unitree shape — or read live off the
server object when imported.
"""

from __future__ import annotations

import json
import signal
import socket
import sys
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from skynet_fourier.wscodec import WsError, accept_key, close as ws_close
from skynet_fourier.wscodec import recv_text as ws_recv
from skynet_fourier.wscodec import send_text as ws_send

#: The fake's motor set. (no, orientation, min_deg, max_deg, start_deg) — ranges
#: distinct per motor so the suite's clamping checks are per-motor meaningful,
#: starts nonzero so a driver writing zeros would not look correct.
MOTORS: tuple[tuple[str, str, float, float, float], ...] = (
    ("1", "left", -150.0, 150.0, 10.0),
    ("1", "right", -150.0, 150.0, -20.0),
    ("2", "left", -120.0, 120.0, 30.0),
    ("2", "right", -120.0, 120.0, -40.0),
    ("3", "left", -90.0, 90.0, 50.0),
    ("3", "right", -90.0, 90.0, -60.0),
)

#: `get_joint_limit`'s docstring example, verbatim: four leg joints in radians.
DOC_JOINT_LIMITS = [
    {"name": "left_hip_roll", "qaMax": 0.523598775598299,
     "qaMin": -0.087266462599716, "qdotaMax": 12.56637061435917, "tauaMax": 82.5},
    {"name": "left_hip_yaw", "qaMax": 0.392699081698724,
     "qaMin": -0.392699081698724, "qdotaMax": 12.56637061435917, "tauaMax": 82.5},
    {"name": "left_hip_pitch", "qaMax": 0.698131700797732,
     "qaMin": -1.221730476396031, "qdotaMax": 22.441443522143093, "tauaMax": 200},
    {"name": "left_knee_pitch", "qaMax": 2.094395102393195,
     "qaMin": -0.087266462599716, "qdotaMax": 22.441443522143093, "tauaMax": 200},
]


class FakeRocsState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests = 0
        self.ws_commands = 0
        self.malformed = 0
        self.unknown_endpoint = 0
        self.clamped = 0
        self.stops = 0
        self.positions = {(no, ori): start for no, ori, _, _, start in MOTORS}
        self.enabled = {(no, ori): True for no, ori, _, _, _ in MOTORS}
        self.move_joints: list[dict] = []
        self.last_move: dict | None = None
        self.last_head: dict | None = None
        self.last_waist: dict | None = None
        self.upper_bodies: list[dict] = []
        self.listening = False
        self.push_conns: list[socket.socket] = []

    def counts(self) -> dict:
        with self.lock:
            return {"requests": self.requests, "ws_commands": self.ws_commands,
                    "malformed": self.malformed,
                    "unknown_endpoint": self.unknown_endpoint,
                    "clamped": self.clamped, "stops": self.stops}

    def limits(self) -> list[dict]:
        # The vendor client's `_move_joint` merges each command with the limit
        # entry by (no, orientation) and pops min/max_angle and ip — so entries
        # carry all three, the key names its own.
        return [{"no": no, "orientation": ori, "min_angle": lo, "max_angle": hi,
                 "ip": "192.168.12.1"}
                for no, ori, lo, hi, _ in MOTORS]

    def pvc(self, no: str, orientation: str) -> dict | None:
        with self.lock:
            if (no, orientation) not in self.positions:
                return None
            position = self.positions[(no, orientation)]
        # Strings, in the docstring's shapes: position to 2 decimals ("85.00"),
        # velocity to 4, current to 5. The fake is static, so velocity reads
        # zero and current a small nonzero hold.
        return {"no": no, "orientation": orientation,
                "position": f"{position:.2f}", "velocity": "0.0000",
                "current": "0.12000"}

    def apply_move(self, motors: list[dict]) -> int:
        """Apply a `move_joint` command list. Returns the clamped count."""
        clamped = 0
        ranges = {(no, ori): (lo, hi) for no, ori, lo, hi, _ in MOTORS}
        with self.lock:
            for motor in motors:
                key = (str(motor.get("no")), str(motor.get("orientation")))
                if key not in self.positions:
                    continue
                try:
                    angle = float(motor.get("angle"))
                except (TypeError, ValueError):
                    continue
                lo, hi = ranges[key]
                if angle < lo:
                    angle, clamped = lo, clamped + 1
                elif angle > hi:
                    angle, clamped = hi, clamped + 1
                self.positions[key] = angle
                self.move_joints.append({"no": key[0], "orientation": key[1],
                                         "angle": angle})
            self.clamped += clamped
        return clamped

    def push_states(self) -> dict:
        # One debug-states push in the docstring's shape: `jointStates` with the
        # six member names, `basestate`, `fsmstatename`, `timestamp`. Positions
        # mirror the fake's motors in degrees — the docstring's example shows
        # radians for `qa`, and the mismatch is the fake's documented
        # simplification: the push proves the frame path, not the units.
        with self.lock:
            joints = [
                {"name": f"{no}_{ori}", "qa": pos, "qdota": 0.0, "taua": 0.0,
                 "qc": pos, "qdotc": 0.0, "tauc": 0.0}
                for (no, ori), pos in sorted(self.positions.items())]
        return {"data": {
            "states": {
                "basestate": {"a": 0.0, "b": 0.0, "c": 0.0, "va": 0.0,
                              "vb": 0.0, "vc": 0.0, "vx": 0.0, "vy": 0.0,
                              "vz": 0.0, "x": 0.0, "y": 0.0, "z": 0.0},
                "fsmstatename": {"currentstatus": "Stand"},
                "jointStates": joints,
                "stanceindex": {},
                "contactforce": {"fxL": 0, "fyL": 0, "fzL": 0, "mxL": 0,
                                 "myL": 0, "mzL": 0, "fxR": 0, "fyR": 0,
                                 "fzR": 0, "mxR": 0, "myR": 0, "mzR": 0}},
            "timestamp": {"nanos": 0, "seconds": "0"}},
            "function": "SonnieGetStates"}


def _ok(data: object = None) -> bytes:
    return json.dumps({"code": 0, "msg": "ok", "data": data or {}}).encode()


def _fail(message: str) -> bytes:
    return json.dumps({"code": -1, "msg": message, "data": None}).encode()


class _Handler(BaseHTTPRequestHandler):
    server_version = "FakeRoCS/1"

    @property
    def _state(self) -> FakeRocsState:
        return self.server.fake_state  # type: ignore[attr-defined]

    def log_message(self, *args: object) -> None:
        pass

    def _send_json(self, body: bytes, status: int = 200) -> None:
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _read_json(self) -> tuple[dict | None, bool]:
        try:
            length = int(self.headers.get("Content-Length", "0"))
        except ValueError:
            return None, False
        raw = self.rfile.read(length) if length else b"{}"
        try:
            decoded = json.loads(raw.decode("utf-8") or "{}")
        except (UnicodeDecodeError, json.JSONDecodeError):
            return None, False
        return decoded if isinstance(decoded, dict) else None, True

    def _count(self) -> None:
        with self._state.lock:
            self._state.requests += 1

    # -- HTTP ----------------------------------------------------------
    def do_GET(self) -> None:  # noqa: N802 (http.server's name)
        if self.path == "/ws" and \
                self.headers.get("Upgrade", "").lower() == "websocket":
            self._serve_ws()
            return
        self._count()
        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path
        if path == "/robot/motor/limit/list":
            self._send_json(_ok(self._state.limits()))
        elif path == "/robot/joint_limit":
            self._send_json(_ok({"function": "SonnieGetStatesLimit",
                                 "data": {"jointlimit": DOC_JOINT_LIMITS}}))
        elif path == "/robot/joint_states":
            self._send_json(_ok({
                "data": {
                    "bodyandlegstate": {
                        "currentstatus": "StartComplete",
                        "log": {"logBuffer": [{"log": "init complete"}]}},
                    "leftarmstate": {"armstatus": "Swing"},
                    "rightarmstate": {"armstatus": "Swing"}},
                "function": "SonnieGetSystemStates"}))
        elif path == "/robot/enable_states_listen":
            with self._state.lock:
                self._state.listening = True
                conns = list(self._state.push_conns)
            push = json.dumps(self._state.push_states())
            for conn in conns:
                try:
                    ws_send(conn, push, client=False)
                except OSError:
                    pass
            self._send_json(_ok({}))
        elif path == "/robot/disable_states_listen":
            with self._state.lock:
                self._state.listening = False
            self._send_json(_ok({}))
        elif path == "/robot/sdk_ctrl/status":
            # The vendor publishes no body for this; the fake's fixed shape.
            self._send_json(_ok({"running": True}))
        elif path == "/robot/motor/hand/enable":
            self._send_json(_ok({}))
        elif path == "/robot/motor/hand/disable":
            self._send_json(_ok({}))
        elif path == "/robot/sdk_ctrl/close":
            self._send_json(_ok({}))
        elif path == "/control/camera_status":
            # The vendor client's constructor probes this (`Camera` reads
            # `data is True`); the driver never calls it. Served so the
            # vendor's construction runs unmodified against the fake.
            self._send_json(_ok(True))
        else:
            with self._state.lock:
                self._state.unknown_endpoint += 1
            self._send_json(_fail(f"no such endpoint: {path}"), 404)

    def do_POST(self) -> None:  # noqa: N802 (http.server's name)
        self._count()
        parsed = urllib.parse.urlparse(self.path)
        path = parsed.path
        body, valid = self._read_json()
        if not valid or body is None:
            with self._state.lock:
                self._state.malformed += 1
            self._send_json(_fail("malformed JSON body"), 400)
            return
        if path == "/robot/motor/pvc":
            no, ori = str(body.get("no", "")), str(body.get("orientation", ""))
            pvc = self._state.pvc(no, ori)
            if pvc is None:
                with self._state.lock:
                    self._state.malformed += 1
                self._send_json(_fail(f"unknown motor: {no}-{ori}"))
            else:
                self._send_json(_ok(pvc))
        elif path == "/robot/motor/hand/state":
            self._send_json(_ok({"position": "0.00"}))
        elif path in ("/robot/stand", "/robot/start", "/robot/reset"):
            self._send_json(_ok({}))
        elif path == "/robot/stop":
            with self._state.lock:
                self._state.stops += 1
            self._send_json(_ok({}))
        elif path == "/robot/upper_body":
            with self._state.lock:
                self._state.upper_bodies.append(dict(body))
            self._send_json(_ok({}))
        else:
            with self._state.lock:
                self._state.unknown_endpoint += 1
            self._send_json(_fail(f"no such endpoint: {path}"), 404)

    # -- WebSocket -----------------------------------------------------
    def _serve_ws(self) -> None:
        key = self.headers.get("Sec-WebSocket-Key", "")
        if not key:
            self.send_error(400, "missing Sec-WebSocket-Key")
            return
        self.send_response(101)
        self.send_header("Upgrade", "websocket")
        self.send_header("Connection", "Upgrade")
        self.send_header("Sec-WebSocket-Accept", accept_key(key))
        self.end_headers()
        sock = self.request
        # Idle hold: the vendor client parks this connection for the session's
        # life, so a timeout here is a heartbeat, not a failure.
        sock.settimeout(5.0)
        self.close_connection = True
        with self._state.lock:
            self._state.push_conns.append(sock)
        try:
            while True:
                try:
                    text = ws_recv(sock, client=False)
                except TimeoutError:
                    continue
                except WsError:
                    with self._state.lock:
                        self._state.malformed += 1
                    break
                except OSError:
                    break
                if text is None:
                    break
                self._handle_ws(text)
        finally:
            with self._state.lock:
                if sock in self._state.push_conns:
                    self._state.push_conns.remove(sock)
            # The handler thread owns this socket until close; http.server
            # closes it on return.
            try:
                ws_close(sock, client=False)
            except OSError:
                pass

    def _handle_ws(self, text: str) -> None:
        try:
            message = json.loads(text)
        except json.JSONDecodeError:
            with self._state.lock:
                self._state.malformed += 1
            return
        if not isinstance(message, dict) or "command" not in message:
            with self._state.lock:
                self._state.malformed += 1
            return
        command = message["command"]
        data = message.get("data", {})
        inner = data.get("command", data) if isinstance(data, dict) else data
        with self._state.lock:
            self._state.ws_commands += 1
        if command == "move_joint" and isinstance(inner, list):
            self._state.apply_move(inner)
        elif command in ("enable_motor", "disable_motor") and isinstance(inner, dict):
            key = (str(inner.get("no")), str(inner.get("orientation")))
            with self._state.lock:
                if key in self._state.enabled:
                    self._state.enabled[key] = command == "enable_motor"
        elif command in ("check_motor_for_flag", "check_motor_for_set_pd"):
            pass
        elif command == "move" and isinstance(data, dict):
            with self._state.lock:
                self._state.last_move = dict(data)
        elif command == "head" and isinstance(data, dict):
            with self._state.lock:
                self._state.last_head = dict(data)
        elif command == "waist" and isinstance(data, dict):
            with self._state.lock:
                self._state.last_waist = dict(data)
        else:
            with self._state.lock:
                self._state.malformed += 1


class FakeRocsServer(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True

    def __init__(self, port: int = 0) -> None:
        self.fake_state = FakeRocsState()
        super().__init__(("127.0.0.1", port), _Handler)


def serve(port: int = 0) -> tuple[FakeRocsServer, FakeRocsState]:
    """A server and its state, for the suite's in-process half."""
    server = FakeRocsServer(port)
    return server, server.fake_state


def serve_forever(port: int = 0) -> None:
    """Serve until SIGTERM, then print the counts as one JSON line (stdout)."""
    server = FakeRocsServer(port)
    announced = server.server_address[1]
    print(f"fake-rocs: listening on 127.0.0.1:{announced}", file=sys.stderr,
          flush=True)

    def _report(_signum: int, _frame: object) -> None:
        print(json.dumps(server.fake_state.counts()), flush=True)
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, _report)
    server.serve_forever()


if __name__ == "__main__":
    serve_forever(int(sys.argv[1]) if len(sys.argv) > 1 else 0)

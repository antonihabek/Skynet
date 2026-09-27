"""A fake Pudu cloud: the OpenAPI endpoints this driver speaks, on loopback HTTP.

Run by `run-check.sh` as a subprocess (ephemeral port, announced on stderr), or imported by
`selftest.py` and served on a thread. Either way the driver reaches it over a real socket —
`urllib` to `http.server` — so the bytes the driver sends are the bytes the fake verifies.

What it verifies, on every request, is the signature: it recomputes the HMAC-SHA1 over the
string-to-sign the skill documents and compares with `hmac.compare_digest`. A driver that signs
wrong gets 401 and a `bad_auth` count, not data. `run-check.sh` additionally runs the vendor's
own `pudu-request.py` against it — the fake accepting the vendor's signature is the independent
half of the signing argument (the driver and the fake sharing one construction would otherwise
prove only that they agree with each other).

Task state is a counter and a dict: deliveries, cruises and calls are accepted, queued with an
id, and reported RUNNING. Nothing moves — a fake cloud has no robot — and the position is fixed
(`TestMap`, 1F, x=1.5/y=2.5/z=0.0): nonzero, so a driver writing zeros would not look correct.

Counts (`requests`, `bad_auth`, `unknown_endpoint`, `malformed`) print as one JSON line on
SIGTERM, the unitree shape — or read live off the server object when imported.
"""

from __future__ import annotations

import base64
import hashlib
import hmac
import json
import re
import signal
import sys
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

#: The test credentials. Fixed, documented, and accepted only by this fake: the driver under
#: test signs with them, and anything else gets 401.
TEST_KEY = "test-key"
TEST_SECRET = "test-secret"

_position = {"mapName": "TestMap", "floor": "1F",
             "position": {"x": 1.5, "y": 2.5, "z": 0.0}}


class FakeCloudState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.requests = 0
        self.bad_auth = 0
        self.unknown_endpoint = 0
        self.malformed = 0
        self.counter = 0
        self.tasks: dict[str, dict] = {}

    def counts(self) -> dict:
        with self.lock:
            return {"requests": self.requests, "bad_auth": self.bad_auth,
                    "unknown_endpoint": self.unknown_endpoint, "malformed": self.malformed}

    def next_id(self, prefix: str) -> str:
        with self.lock:
            self.counter += 1
            return f"{prefix}-{self.counter:04d}"


def _verify(method: str, path: str, query: str, body: bytes, headers) -> bool:
    """Recompute the skill's signature over what arrived and compare in constant time."""
    date = headers.get("x-date", "")
    auth = headers.get("Authorization", "")
    match = re.fullmatch(r'hmac id="([^"]+)", algorithm="hmac-sha1", headers="x-date", '
                         r'signature="([^"]+)"', auth or "")
    if not date or not match or match.group(1) != TEST_KEY:
        return False
    md5 = ""
    if method == "POST":
        md5 = base64.b64encode(hashlib.md5(body).hexdigest().encode("ascii")).decode("ascii")
    # The same environment-prefix strip the driver signs with (duplicated, not imported: the
    # fake and the driver must agree the way two implementations agree, not by sharing code).
    base = path
    for prefix in ("/release", "/test", "/prepub"):
        if base == prefix:
            base = "/"
        elif base.startswith(prefix + "/"):
            base = base[len(prefix):]
    sign_path = base if not query else base + "?" + urllib.parse.unquote(query)
    signing = "\n".join(["x-date: " + date, method, "application/json", "application/json",
                         md5, sign_path])
    expect = base64.b64encode(
        hmac.new(TEST_SECRET.encode("utf-8"), signing.encode("utf-8"),
                 hashlib.sha1).digest()).decode("ascii")
    return hmac.compare_digest(expect, match.group(2))


def _send(handler: BaseHTTPRequestHandler, code: int, payload: dict) -> None:
    raw = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    handler.send_response(code)
    handler.send_header("Content-Type", "application/json")
    handler.send_header("Content-Length", str(len(raw)))
    handler.end_headers()
    handler.wfile.write(raw)


class _Handler(BaseHTTPRequestHandler):
    server_version = "FakePuduCloud/0.1.0"

    def log_message(self, *args) -> None:  # keep stderr for the port line and the report
        pass

    def _dispatch(self) -> None:
        state: FakeCloudState = self.server.fake_state  # type: ignore[attr-defined]
        parsed = urllib.parse.urlparse(self.path)
        path, query = parsed.path, parsed.query
        length = int(self.headers.get("Content-Length", "0") or "0")
        body = self.rfile.read(length) if length else b""
        with state.lock:
            state.requests += 1
        if not _verify(self.command, path, query, body, self.headers):
            with state.lock:
                state.bad_auth += 1
            _send(self, 401, {"message": "UNAUTHORIZED"})
            return
        try:
            data = json.loads(body.decode("utf-8")) if body else {}
        except (json.JSONDecodeError, UnicodeDecodeError):
            with state.lock:
                state.malformed += 1
            _send(self, 400, {"message": "MALFORMED"})
            return
        params = dict(urllib.parse.parse_qsl(query, keep_blank_values=True))

        if self.command == "GET" and path == "/open-platform-service/v1/robot/get_position":
            if "sn" not in params:
                return self._malformed(state)
            return _send(self, 200, {"message": "SUCCESS", "data": _position})
        if self.command == "GET" and path == "/open-platform-service/v1/robot/task/state/get":
            if "sn" not in params:
                return self._malformed(state)
            with state.lock:
                tasks = [{"name": t["kind"], "type": t["kind"], "state": "RUNNING"}
                         for t in state.tasks.values()]
            return _send(self, 200, {"message": "SUCCESS",
                                     "data": {"code": 0, "message": "SUCCESS",
                                              "data": {"tasks": tasks}}})
        if self.command == "GET" and path == "/open-platform-service/v2/status/get_by_sn":
            if "sn" not in params:
                return self._malformed(state)
            return _send(self, 200, {"message": "SUCCESS", "data": {"online": True}})
        if self.command == "POST" and path in (
                "/open-platform-service/v1/delivery_task",
                "/open-platform-service/v1/cruise_task",
                "/open-platform-service/v1/custom_call",
                "/open-platform-service/v1/custom_call/cancel"):
            return self._task(state, path, data)
        with state.lock:
            state.unknown_endpoint += 1
        _send(self, 404, {"message": "UNKNOWN"})

    def _malformed(self, state: FakeCloudState):
        with state.lock:
            state.malformed += 1
        _send(self, 400, {"message": "MALFORMED"})

    def _task(self, state: FakeCloudState, path: str, data: dict):
        """One task endpoint: shape-check the documented fields, queue, answer queued."""
        kind = {"delivery_task": "delivery", "cruise_task": "cruise",
                "custom_call": "call", "custom_call/cancel": "cancel_call"}
        key = path.rsplit("/", 1)[-1] if not path.endswith("cancel") else "custom_call/cancel"
        if not isinstance(data, dict) or "sn" not in data:
            return self._malformed(state)
        if key == "delivery_task":
            trays = (data.get("payload") or {}).get("trays") or []
            points = [d.get("points") for t in trays for d in t.get("destinations", [])]
            if not trays or not all(points):
                return self._malformed(state)
        elif key == "cruise_task":
            if not (data.get("payload") or {}).get("mapCruiseName"):
                return self._malformed(state)
        elif key == "custom_call":
            if not data.get("point"):
                return self._malformed(state)
        elif key == "custom_call/cancel":
            if not data.get("taskId"):
                return self._malformed(state)
            with state.lock:
                if data["taskId"] not in state.tasks:
                    return _send(self, 200, {"message": "NOT_FOUND"})
                del state.tasks[data["taskId"]]
            return _send(self, 200, {"message": "SUCCESS", "data": {"cancelled": True}})
        task_id = state.next_id("task")
        with state.lock:
            state.tasks[task_id] = {"kind": kind[key]}
        return _send(self, 200, {"message": "SUCCESS",
                                 "data": {"code": 0, "message": "SUCCESS", "taskId": task_id,
                                          "taskStatus": "RUNNING", "queueNo": 1}})

    do_GET = _dispatch
    do_POST = _dispatch


def serve(port: int = 0) -> tuple[ThreadingHTTPServer, FakeCloudState]:
    """Serve on 127.0.0.1 (an ephemeral port when 0), returning server and state."""
    server = ThreadingHTTPServer(("127.0.0.1", port), _Handler)
    server.daemon_threads = True
    server.fake_state = FakeCloudState()  # type: ignore[attr-defined]
    return server, server.fake_state  # type: ignore[attr-defined]


def main(argv: list[str]) -> int:
    port = int(argv[1]) if len(argv) > 1 else 0
    server, state = serve(port)
    print(f"fake-pudu-cloud: listening on 127.0.0.1:{server.server_port}", file=sys.stderr,
          flush=True)

    def stop(signum, frame):
        print(json.dumps(state.counts()), flush=True)
        raise SystemExit(0)

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    server.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))

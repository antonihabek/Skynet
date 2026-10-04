"""A simulated SCHUNK gripper module, written here from the maker's diagrams and from reading its client and its simulator: the dry substrate of `drivers/schunk` (DRY-COVERAGE-PLAN.md item 12.E.9).

It serves the two routes of the module's web interface (`GET /adi/data.json`, `POST /adi/update.json`) over stdlib HTTP, keeps the 16-byte input and output frames, and moves a jaw at the speed it was told to. **It agrees
with whoever wrote it** (F15): what it can show is the driver's own handshake, refusals and stop path, and what it cannot is how a real module behaves. SCHUNK's own simulator (`schunk_gripper_dummy`) is the
other substrate, and `live.py` runs the driver against it; this is not a copy of that, and where they differ the difference is named below:

* **Time.** The maker's simulator does a move *inside* the request that asked for it, so the status frame does not change until the jaw has arrived. This one moves the jaw in a thread of its own, so the toggle flips at
  once and the done bits when the jaw is there, which is what a module with a motor does. `time_scale` makes it faster (a test of a 1.5 s move takes 0.15 s); it is never 1 for a measurement of the driver's timing.
* **A grip with nothing in the jaws.** The maker's simulator closes to the minimum position and reports *gripped*. This one stops where `workpiece_um` is (gripped), and with no workpiece closes to the minimum and
  reports *no workpiece detected*, which is what the maker's client expects of a module that finds none (`Driver.GripResult.NO_WORKPIECE_DETECTED`).
* **Power-up.** A module comes up with its fast stop active (status bit 7, error byte 0xD9) in the maker's simulator, and this one can (`start_ready=False`), because that is the case a driver must handle.

Parameters are an EGU 60 (and an EGK 25) as the maker's simulator reports them: read, not copied.
"""

from __future__ import annotations

import json
import struct
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

from . import frames
from .frames import Bit, Status

__all__ = ["SimulatedGripper", "MODULES"]

#: Parameter sets, in the module's own units (mm, mm/s, %).
MODULES = {
    "EGU_60_M_B": {"type": 11, "min_pos": 0.0, "max_pos": 120.0, "min_vel": 10.0, "max_vel": 110.0, "max_grp_vel": 25.0, "wp_release_delta": 5.0, "max_phys_stroke": 120.0, "max_grp_force": 650.0, "serial": "NSNS0667", "sw": "5.3.0.90971"},
    "EGK_25_M_B": {"type": 17, "min_pos": 20.0, "max_pos": 53.0, "min_vel": 5.0, "max_vel": 120.0, "max_grp_vel": 20.0, "wp_release_delta": 10.0, "max_phys_stroke": 53.0, "max_grp_force": 50.0, "serial": "BBKT3207", "sw": "5.3.1.116408"},
}
FIELDBUS_CODES = {"EI": 2, "EC": 3, "PN": 1}


class SimulatedGripper:
    def __init__(self, module: str = "EGU_60_M_B", fieldbus: str = "EI", *, start_ready: bool = True, time_scale: float = 1.0, workpiece_um: int | None = None) -> None:
        if module not in MODULES:
            raise ValueError(f"no such simulated module {module!r}: {', '.join(MODULES)}")
        self.module, self.fieldbus = module, fieldbus
        self.params = dict(MODULES[module])
        self.time_scale = time_scale
        self.workpiece_um = workpiece_um
        self.port = 0
        self._lock = threading.RLock()
        self._input = bytearray(16)
        self._output = bytearray(frames.clear_output())
        self._data: dict[str, str] = {}
        self._motion: dict | None = None
        self._position = float(self.params["min_pos"] * 1000.0 + 250.0)
        self._stopping = False
        self._server: ThreadingHTTPServer | None = None
        self._connections: list = []
        #: What a test turns on.
        self.drop = False                 # every request is lost on the way: a module that has gone
        self.drop_posts = 0               # the next this-many POSTs are lost
        self.delay_s = 0.0                # every answer waits
        self.reachable = True             # False: 503 (the maker's simulator's `lose_connection_sec` event)
        self.requests: list[tuple[str, str, object]] = []
        self.commands: list[dict] = []    # every output frame that triggered something: what the module was asked
        self.not_feasible_above_um: int | None = None
        self.sticky_error: int | None = None   # an error whose cause is still there: an acknowledge does not clear it
        self.error_on_command: int | None = None   # a module that takes a command and raises this error byte with it (not an acknowledge): found by the seeded-defect run, 12.C.3
        self.refuse_writes = False        # every write to the output frame is answered `result: 1` and not acted on
        self.garble_reads = False         # every read of an instance is answered with something that is not a list of hex text
        self.jammed = False               # the jaw does not move whatever it is told: a motion that never completes
        self._build_data()
        if start_ready:
            self._set_bit(Status.READY, True)
        else:
            self._set_bit(Status.ERROR, True)
            self._input[12] = frames.ERROR_FAST_STOP
        self._put_position()

    # ── the module's data ────────────────────────────────────────────────────

    def _float(self, value: float) -> str:
        raw = struct.pack("<f", value)
        return (raw[::-1] if self.fieldbus == "PN" else raw).hex().upper()

    def _build_data(self) -> None:
        p = self.params
        self._data = {
            "0x0500": struct.pack("<h", p["type"]).hex().upper()[:2], "0x1130": f"{FIELDBUS_CODES[self.fieldbus]:02X}",
            "0x0540": self._float(p["wp_release_delta"]), "0x0588": self._float(p["max_phys_stroke"]), "0x0600": self._float(p["min_pos"]), "0x0608": self._float(p["max_pos"]),
            "0x0628": self._float(p["min_vel"]), "0x0630": self._float(p["max_vel"]), "0x0650": self._float(p["max_grp_vel"]), "0x0660": self._float(p["max_grp_force"]),
            "0x1000": p["serial"].encode().ljust(16).hex().upper(), "0x1118": p["sw"].encode().ljust(22).hex().upper(),
            "0x0380": "0000",  # the pre-hold time, in ms: the maker's client reads it at the end of every grip
        }

    # ── the input frame ──────────────────────────────────────────────────────

    def _bit(self, bit: int) -> int:
        return self._input[bit // 8] >> (bit % 8) & 1

    def _set_bit(self, bit: int, value: bool) -> None:
        if value:
            self._input[bit // 8] |= 1 << (bit % 8)
        else:
            self._input[bit // 8] &= ~(1 << (bit % 8)) & 0xFF

    def _put_position(self) -> None:
        raw = struct.pack("<i", int(round(self._position)))
        self._input[4:8] = raw[::-1] if self.fieldbus == "PN" else raw

    @property
    def position_um(self) -> int:
        with self._lock:
            return int(round(self._position))

    def place(self, position_um: int) -> None:
        with self._lock:
            self._position = float(position_um)
            self._motion = None
            self._put_position()

    def status_bits(self) -> list[int]:
        with self._lock:
            return [bit for bit in Status.VALID if self._bit(bit)]

    @property
    def error_code(self) -> int:
        return self._input[12]

    def set_error(self, code: int | None) -> None:
        """A fault the module raises by itself (not by a command): the error bit and byte, and any motion ends."""
        with self._lock:
            self._motion = None
            self._set_bit(Status.ERROR, code is not None)
            self._input[12] = code or 0

    def set_warning(self, code: int | None) -> None:
        with self._lock:
            self._set_bit(Status.WARNING, code is not None)
            self._input[14] = code or 0

    # ── the commands ─────────────────────────────────────────────────────────

    def _control(self) -> tuple[int, int, int, int]:
        word = struct.unpack("<I", bytes(self._output[0:4]))[0]
        fields = []
        for start in (4, 8, 12):
            raw = bytes(self._output[start:start + 4])
            fields.append(struct.unpack("<i", raw[::-1] if self.fieldbus == "PN" else raw)[0])
        return word, fields[0], fields[1], fields[2]

    def _process(self) -> None:
        """What the module does with an output frame, in the order the maker's simulator does it (acknowledge, manual release, errors, stops, then motion)."""
        word, position, speed, force = self._control()
        bit = lambda n: word >> n & 1  # noqa: E731
        if bytes(self._output) == frames.clear_output():
            return
        self.commands.append({"word": word, "position": position, "speed": speed, "force": force, "hex": bytes(self._output).hex().upper()})
        for transient in (Status.NOT_FEASIBLE, Status.SUCCESS, Status.NO_WORKPIECE, Status.GRIPPED, Status.POSITION_REACHED, Status.PRE_GRIP, Status.WORKPIECE_LOST, Status.WRONG_WORKPIECE, Status.SOFT_LIMIT):
            self._set_bit(transient, False)
        self._set_bit(Status.TOGGLE, not self._bit(Status.TOGGLE))
        if bit(Bit.ACKNOWLEDGE):
            self._motion = None
            if self.sticky_error is not None:
                self._set_bit(Status.READY, False)
                self._set_bit(Status.ERROR, True)
                self._input[12] = self.sticky_error
                return
            self._set_bit(Status.READY, True)
            self._set_bit(Status.ERROR, False)
            self._input[12] = 0
            return
        if self.error_on_command is not None:
            self._motion = None
            self._set_bit(Status.ERROR, True)
            self._set_bit(Status.READY, False)
            self._input[12] = self.error_on_command
            return
        if self._bit(Status.ERROR):
            return                                                    # a module in error takes nothing but an acknowledge
        if not bit(Bit.FAST_STOP):
            self._motion = None
            self._set_bit(Status.ERROR, True)
            self._set_bit(Status.READY, False)
            self._input[12] = frames.ERROR_FAST_STOP
            return
        if bit(Bit.STOP):
            self._motion = None
            self._set_bit(Status.SUCCESS, True)
            self._set_bit(Status.POSITION_REACHED, True)
            return
        if bit(Bit.RELEASE_WORKPIECE):
            self._start(self._position + self.params["wp_release_delta"] * 1000.0, self.params["max_grp_vel"] * 1000.0, "release")
            return
        if bit(Bit.MOVE_ABSOLUTE) or bit(Bit.MOVE_RELATIVE):
            target = position if bit(Bit.MOVE_ABSOLUTE) else int(round(self._position)) + position
            low, high = self.params["min_pos"] * 1000.0, self.params["max_pos"] * 1000.0
            if speed <= 0 or not low - 1 <= target <= high + 1 or (self.not_feasible_above_um is not None and target > self.not_feasible_above_um):
                self._set_bit(Status.NOT_FEASIBLE, True)
                return
            self._start(float(target), float(speed), "move")
            return
        if bit(Bit.GRIP_WORKPIECE):
            outward = bool(bit(Bit.GRIP_DIRECTION))
            end = self.params["max_pos"] * 1000.0 if outward else self.params["min_pos"] * 1000.0
            if self.workpiece_um is not None and ((not outward and self.workpiece_um > self._position) or (outward and self.workpiece_um < self._position)):
                self._set_bit(Status.NOT_FEASIBLE, True)
                return
            target = float(self.workpiece_um) if self.workpiece_um is not None else end
            self._start(target, (speed if speed > 0 else self.params["max_grp_vel"] * 1000.0), "grip")
            return
        self._set_bit(Status.NOT_FEASIBLE, True)                       # a command this simulator does not model (jogging, brake test, a grip at a position, shutdown, soft reset)

    def _start(self, target: float, speed: float, kind: str) -> None:
        self._motion = {"target": target, "speed": max(1.0, speed), "kind": kind}

    def _finish(self, kind: str) -> None:
        self._set_bit(Status.SUCCESS, kind != "grip" or self.workpiece_um is not None)
        if kind == "grip":
            self._set_bit(Status.GRIPPED if self.workpiece_um is not None else Status.NO_WORKPIECE, True)
        else:
            self._set_bit(Status.POSITION_REACHED, True)

    def _physics(self) -> None:
        last = time.monotonic()
        while not self._stopping:
            time.sleep(0.005)
            now = time.monotonic()
            dt, last = now - last, now
            with self._lock:
                motion = self._motion
                if motion is None or self.jammed:
                    continue
                step = motion["speed"] * dt * self.time_scale
                gap = motion["target"] - self._position
                if abs(gap) <= step:
                    self._position = motion["target"]
                    self._motion = None
                    self._finish(motion["kind"])
                else:
                    self._position += step if gap > 0 else -step
                self._put_position()

    # ── the web interface ────────────────────────────────────────────────────

    def start(self) -> SimulatedGripper:
        gripper = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *args: object) -> None:
                pass

            def setup(self) -> None:
                super().setup()
                gripper._connections.append(self.connection)

            def _answer(self, status: int, document: object) -> None:
                body = json.dumps(document).encode()
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def do_GET(self) -> None:
                gripper._handle(self, "GET")

            def do_POST(self) -> None:
                gripper._handle(self, "POST")

        class Quiet(ThreadingHTTPServer):
            def handle_error(self, request, client_address) -> None:
                pass

        self._server = Quiet(("127.0.0.1", 0), Handler)
        self._server.daemon_threads = True
        self.port = self._server.server_address[1]
        threading.Thread(target=self._server.serve_forever, daemon=True, name="schunk-sim-http").start()
        threading.Thread(target=self._physics, daemon=True, name="schunk-sim-physics").start()
        return self

    def stop(self) -> None:
        self._stopping = True
        if self._server is not None:
            self._server.shutdown()
            self._server.server_close()

    def __enter__(self) -> SimulatedGripper:
        return self.start()

    def __exit__(self, *exc: object) -> None:
        self.stop()

    def reset_connections(self) -> None:
        for connection in self._connections:
            try:
                connection.shutdown(2)
                connection.close()
            except OSError:
                pass
        self._connections = []

    def _handle(self, handler: BaseHTTPRequestHandler, method: str) -> None:
        split = urlsplit(handler.path)
        body = b""
        if method == "POST":
            length = int(handler.headers.get("Content-Length") or 0)
            body = handler.rfile.read(length) if length else b""
        form = {k: v[0] for k, v in parse_qs(body.decode("ascii", errors="replace")).items()}
        self.requests.append((method, split.path, form if method == "POST" else parse_qs(split.query)))
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
        if method == "GET" and split.path == frames.Route.DATA:
            if not self.reachable:
                handler.send_response(503)
                handler.send_header("Content-Length", "0")
                handler.end_headers()
                return
            if self.garble_reads:
                answer(200, {"result": "not a list"})
                return
            query = {k: v[0] for k, v in parse_qs(split.query).items()}
            inst, count = query.get("inst"), query.get("count")
            if count != "1":
                answer(200, [])
            elif inst == frames.Instance.INPUT:
                with self._lock:
                    answer(200, [bytes(self._input).hex().upper()])
            elif inst == frames.Instance.OUTPUT:
                with self._lock:
                    answer(200, [bytes(self._output).hex().upper()])
            elif inst in self._data:
                answer(200, [self._data[inst]])
            else:
                answer(200, [])
        elif method == "POST" and split.path == frames.Route.UPDATE:
            inst, value = form.get("inst"), form.get("value", "")
            if self.refuse_writes:
                answer(200, {"result": 1})
                return
            if inst != frames.Instance.OUTPUT or len(value) != 32 or any(c not in "0123456789abcdefABCDEF" for c in value):
                answer(200, {"result": 1})
                return
            with self._lock:
                self._output[:] = bytes.fromhex(value)
                self._process()
            answer(200, {"result": 0})
        else:
            answer(404, {"detail": "not found"})

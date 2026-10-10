"""A Tello written here, for the tests: the dry substrate (DRY-COVERAGE-PLAN.md item 12.E.11).

It is the SDK 2.0 User Guide as a program: a UDP socket that takes the guide's text commands and answers them (`ok` when a command is done, which for a motion is when the drone has arrived), a state string 10 times a second
to the host that spoke to it last, a battery that runs down, and the guide's one safety feature: "If there is no command for 15 seconds, the Tello will land automatically."

**It agrees with whoever wrote it** (F15). What the guide does not say, this does by assumption, and each is a lever or a comment: a drone that has not been sent `command` says nothing; a command that interrupts another one
(`stop`, `emergency`) is answered and the interrupted one is not; `rc` and `emergency` get no reply (the maker's own sample client, and every other, sends them without waiting for one); a motion sent while another runs is an `error`.
Distances are centimetres and its motions run `time_scale` times faster than the wall (its watchdog does not).
"""

from __future__ import annotations

import math
import queue
import select
import socket
import threading
import time

from . import frames

__all__ = ["DroneSimulator"]

#: How far the simulated drone climbs on a takeoff (centimetres), and how long a takeoff and a landing take at time_scale 1.
TAKEOFF_CM = 80
TAKEOFF_S = 4.0
LAND_S = 3.0
ROTATE_DEG_S = 90.0
RC_CM_S = 100.0
WATCHDOG_S = 15.0
STATE_INTERVAL_S = 0.1
STEP_S = 0.02


class DroneSimulator:
    def __init__(self, *, time_scale: float = 1.0, state_port: int = frames.STATE_PORT, bind: str = "127.0.0.1", battery: int = 100, watchdog_s: float = WATCHDOG_S) -> None:
        self.time_scale = time_scale
        #: The guide's 15 seconds, on the wall's clock: the drone's watchdog is its own, and `time_scale` only hurries its motions.
        self.watchdog_s = watchdog_s
        self.state_port = state_port
        self.bind = bind
        self.command_port = 0
        self._lock = threading.RLock()
        self._sock: socket.socket | None = None
        self._state_sock: socket.socket | None = None
        self._threads: list[threading.Thread] = []
        self._closing = threading.Event()
        self._motions: queue.Queue = queue.Queue()
        self._cancel = threading.Event()
        self._busy = False
        # What the drone is doing.
        self.sdk_mode = False
        self.flying = False
        self.motors = False
        self.x = self.y = 0.0
        self.z = 0.0                 # height, cm
        self.yaw = 0.0               # degrees
        self.speed = 10              # cm/s, "speed?"
        self.battery = float(battery)
        self.flight_s = 0.0
        self.templ, self.temph = 60, 62
        self.rc = (0, 0, 0, 0)
        self._velocity = (0.0, 0.0, 0.0)
        self._client: tuple[str, int] | None = None
        self._last_command = time.monotonic()
        # What a test injects.
        self.silent = False
        self.drop_replies = 0
        self.reply_delay_s = 0.0
        self.ignore_stop = False
        self.state_stalled = False
        self.state_garbage = False
        self.state_override: dict | None = None
        self.interrupted_reply: str | None = None
        self.error_next: str | None = None
        self.command_reply = "ok"
        self.serial = "0TQDG9RED00000"
        # What the driver did.
        self.commands: list[str] = []
        self.states_sent = 0

    # ── lifecycle ────────────────────────────────────────────────────────────

    def start(self) -> DroneSimulator:
        self._closing.clear()
        self._sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self._sock.bind((self.bind, 0))
        self.command_port = self._sock.getsockname()[1]
        self._state_sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        for target in (self._receive_loop, self._motion_loop, self._physics_loop, self._state_loop):
            thread = threading.Thread(target=target, daemon=True, name=f"tello-sim-{target.__name__}")
            thread.start()
            self._threads.append(thread)
        return self

    def stop(self) -> None:
        self._closing.set()
        self._cancel.set()
        self._motions.put(None)
        for thread in self._threads:
            thread.join(timeout=1.0)
        self._threads = []
        for sock in (self._sock, self._state_sock):
            if sock is not None:
                try:
                    sock.close()
                except OSError:
                    pass
        self._sock = self._state_sock = None

    # ── what a test reads ────────────────────────────────────────────────────

    def seen(self, prefix: str) -> list[str]:
        with self._lock:
            return [c for c in self.commands if c.startswith(prefix)]

    @property
    def position(self) -> tuple[float, float, float, float]:
        with self._lock:
            return self.x, self.y, self.z, self.yaw

    def state(self) -> frames.State:
        with self._lock:
            vx, vy, vz = self._velocity
            return frames.State(pitch=0, roll=0, yaw=int(round(((self.yaw + 180.0) % 360.0) - 180.0)), vgx=int(vx), vgy=int(vy), vgz=int(vz), templ=self.templ, temph=self.temph,
                                tof=int(self.z) if self.flying else 10, h=int(self.z), bat=max(0, int(self.battery)), baro=-80.0 + self.z / 100.0, time=int(self.flight_s), agx=0.0, agy=0.0, agz=-1000.0)

    # ── the command socket ───────────────────────────────────────────────────

    def _receive_loop(self) -> None:
        sock = self._sock
        assert sock is not None
        while not self._closing.is_set():
            try:
                readable, _, _ = select.select([sock], [], [], 0.05)
            except (OSError, ValueError):
                return
            if not readable:
                continue
            try:
                data, address = sock.recvfrom(1024)
            except OSError:
                return
            try:
                text = data.decode("utf-8").strip()
            except UnicodeDecodeError:
                continue
            with self._lock:
                self.commands.append(text)
                silent = self.silent
            if silent:
                continue
            self._handle(text, address)

    def _reply(self, address: tuple[str, int], text: str) -> None:
        with self._lock:
            if self.drop_replies > 0:
                self.drop_replies -= 1
                return
            delay = self.reply_delay_s
        if delay:
            time.sleep(delay)
        sock = self._sock
        if sock is not None:
            try:
                sock.sendto((text + "\r\n").encode(), address)
            except OSError:
                pass

    def _handle(self, text: str, address: tuple[str, int]) -> None:
        words = text.split()
        if not words:
            return
        verb = words[0]
        if verb == "command":
            with self._lock:
                self.sdk_mode = True
                self._client = (address[0], self.state_port)
                self._last_command = time.monotonic()
                answer = self.command_reply
            self._reply(address, answer)
            return
        with self._lock:
            if not self.sdk_mode:
                return
            self._last_command = time.monotonic()
        if verb == "emergency":
            with self._lock:
                self.motors = False
                self.rc = (0, 0, 0, 0)
            self._cancel.set()
            return
        if verb == "stop":
            with self._lock:
                keep = self.ignore_stop
                if not keep:
                    self.rc = (0, 0, 0, 0)
            if not keep:
                self._cancel.set()
            self._reply(address, "ok")
            return
        if verb == "rc":
            try:
                values = tuple(int(w) for w in words[1:5])
            except ValueError:
                return
            if len(values) == 4 and all(-100 <= v <= 100 for v in values):
                with self._lock:
                    if self.flying and self.motors:
                        self.rc = values
            return
        reads = {"speed?": lambda: str(self.speed), "battery?": lambda: str(max(0, int(self.battery))), "time?": lambda: f"{int(self.flight_s)}s", "wifi?": lambda: "90", "sdk?": lambda: "20", "sn?": lambda: self.serial,
                 "height?": lambda: f"{int(self.z // 10)}dm", "temp?": lambda: f"{self.templ}~{self.temph}C", "tof?": lambda: f"{int(self.z) * 10}mm"}
        if verb in reads:
            with self._lock:
                answer = reads[verb]()
            self._reply(address, answer)
            return
        if verb in ("takeoff", "land", "up", "down", "left", "right", "forward", "back", "cw", "ccw", "flip", "speed"):
            with self._lock:
                busy = self._busy
                forced = self.error_next
                self.error_next = None
            if forced is not None:
                self._reply(address, forced)
                return
            if verb == "speed":
                if len(words) == 2 and words[1].lstrip("-").isdigit() and 10 <= int(words[1]) <= 100:
                    with self._lock:
                        self.speed = int(words[1])
                    self._reply(address, "ok")
                else:
                    self._reply(address, "error")
                return
            if busy:
                self._reply(address, "error")
                return
            self._motions.put((words, address))
            return
        self._reply(address, f"unknown command: {verb}")

    # ── the motions ──────────────────────────────────────────────────────────

    def _wait(self, seconds: float) -> bool:
        """Sleep `seconds` of the drone's time (the wall's divided by time_scale), and say whether it was left in peace."""
        end = time.monotonic() + seconds / self.time_scale
        while time.monotonic() < end:
            if self._cancel.wait(min(STEP_S, max(0.0, end - time.monotonic()))):
                return False
            if self._closing.is_set():
                return False
        return True

    def _motion_loop(self) -> None:
        while not self._closing.is_set():
            item = self._motions.get()
            if item is None:
                return
            words, address = item
            self._cancel.clear()
            with self._lock:
                self._busy = True
            try:
                answer = self._run(words)
            finally:
                with self._lock:
                    self._busy = False
                    self._velocity = (0.0, 0.0, 0.0)
            if answer is None:
                if self.interrupted_reply is not None:
                    self._reply(address, self.interrupted_reply)
            else:
                self._reply(address, answer)

    def _run(self, words: list[str]) -> str | None:
        """The reply to a motion, or None when it was interrupted."""
        verb = words[0]
        with self._lock:
            flying, motors, battery = self.flying, self.motors, self.battery
        if verb == "takeoff":
            if flying or battery < 10:
                return "error"
            with self._lock:
                self.motors = True
            return self._climb(TAKEOFF_CM, TAKEOFF_S, takeoff=True)
        if verb == "land":
            if not flying:
                return "error"
            return self._climb(0, LAND_S, takeoff=False)
        if not flying or not motors:
            return "error"
        if verb == "flip":
            if len(words) == 2 and words[1] in frames.FLIPS and battery > 50:
                return "ok" if self._wait(1.0) else None
            return "error"
        try:
            amount = int(words[1])
        except (IndexError, ValueError):
            return "error"
        if verb in ("cw", "ccw"):
            if not 1 <= amount <= 360:
                return "error"
            sign = 1 if verb == "cw" else -1
            steps = max(1, int(amount / ROTATE_DEG_S / self.time_scale / STEP_S))
            for _ in range(steps):
                if not self._wait(STEP_S * self.time_scale):
                    return None
                with self._lock:
                    self.yaw += sign * amount / steps
            return "ok"
        if not 20 <= amount <= 500:
            return "error"
        body = {"forward": (1, 0, 0), "back": (-1, 0, 0), "left": (0, 1, 0), "right": (0, -1, 0), "up": (0, 0, 1), "down": (0, 0, -1)}[verb]
        with self._lock:
            speed = self.speed
        seconds = amount / speed
        steps = max(1, int(seconds / self.time_scale / STEP_S))
        for _ in range(steps):
            if not self._wait(STEP_S * self.time_scale):
                return None
            with self._lock:
                radians = math.radians(self.yaw)
                dx, dy, dz = body[0] * amount / steps, body[1] * amount / steps, body[2] * amount / steps
                self.x += dx * math.cos(radians) - dy * math.sin(radians)
                self.y += dx * math.sin(radians) + dy * math.cos(radians)
                self.z = max(0.0, self.z + dz)
                self._velocity = (body[0] * speed, body[1] * speed, body[2] * speed)
        return "ok"

    def _climb(self, target_cm: float, seconds: float, *, takeoff: bool) -> str | None:
        with self._lock:
            start = self.z
            if takeoff:
                self.flying = True
        steps = max(1, int(seconds / self.time_scale / STEP_S))
        for index in range(1, steps + 1):
            if not self._wait(STEP_S * self.time_scale):
                return None
            with self._lock:
                self.z = start + (target_cm - start) * index / steps
        with self._lock:
            if not takeoff:
                self.flying = False
                self.motors = False
                self.rc = (0, 0, 0, 0)
        return "ok"

    # ── the world ────────────────────────────────────────────────────────────

    def _physics_loop(self) -> None:
        last = time.monotonic()
        while not self._closing.is_set():
            time.sleep(STEP_S)
            now = time.monotonic()
            dt = (now - last) * self.time_scale
            last = now
            with self._lock:
                if self.flying:
                    self.flight_s += dt
                    self.battery = max(0.0, self.battery - dt / 60.0)
                    if not self.motors:
                        # `emergency`: the motors are cut and the drone falls.
                        self.z = max(0.0, self.z - 400.0 * dt)
                        if self.z <= 0.0:
                            self.flying = False
                    elif any(self.rc):
                        a, b, c, d = self.rc
                        radians = math.radians(self.yaw)
                        forward, left = b * RC_CM_S / 100.0, -a * RC_CM_S / 100.0
                        self.x += (forward * math.cos(radians) - left * math.sin(radians)) * dt
                        self.y += (forward * math.sin(radians) + left * math.cos(radians)) * dt
                        self.z = max(0.0, self.z + c * RC_CM_S / 100.0 * dt)
                        self.yaw += d * ROTATE_DEG_S / 100.0 * dt
                        self._velocity = (b * RC_CM_S / 100.0, -a * RC_CM_S / 100.0, c * RC_CM_S / 100.0)
                    elif not self._busy:
                        self._velocity = (0.0, 0.0, 0.0)
                if self.sdk_mode and self.flying and now - self._last_command > self.watchdog_s and not self._busy:
                    # "If there is no command for 15 seconds, the Tello will land automatically."
                    self.rc = (0, 0, 0, 0)
                    self.z = max(0.0, self.z - 40.0 * dt)
                    if self.z <= 0.0:
                        self.flying = False
                        self.motors = False

    def _state_loop(self) -> None:
        while not self._closing.is_set():
            time.sleep(STATE_INTERVAL_S)
            with self._lock:
                client, stalled, garbage, override = self._client, self.state_stalled, self.state_garbage, self.state_override
            sock = self._state_sock
            if client is None or stalled or sock is None:
                continue
            if garbage:
                payload = b"this is not a state string\r\n"
            else:
                state = self.state()
                text = frames.encode_state(state)
                if override:
                    text = ";".join(f"{key}:{value}" for key, value in override.items()) + ";\r\n"
                payload = text.encode()
            try:
                sock.sendto(payload, client)
                self.states_sent += 1
            except OSError:
                pass

"""The ROS 2 base driver's subject for `tools/faultproxy/chaos.py`: DDS on one interface, with the kernel's own link emulation on it.

    make -C drivers/ros2_base chaos-docker      # in the ROS 2 image, which has ROS, `tc` and a loopback of its own

ROS 2 speaks DDS, and DDS names its peers in discovery messages and sends to ports nobody configured, so there is no address to put a relay at. The flow is `("netem", "lo")`: the queue of the
container's own loopback, which the driver (this process, in-process, with its own `rclpy`) and the controller share. Every packet either sends meets the profile, in both directions at once;
`tools/faultproxy/relays.py`'s `netem_arguments` says what a kernel queue cannot do (`truncate`, `stall`, a fault scoped to one direction, `reset`), which `profiles` leaves out.

Two things have to be true for that to measure anything, and the first was measured before this file was written (`DRY-COVERAGE-PLAN.md`, the sixty-second session):

  * **The driver's DDS is made to use the network.** Between two processes of one host, DDS's default is shared memory, which is not an interface, and a profile on `lo` would then be on
    nothing. `FASTDDS_BUILTIN_TRANSPORTS=UDPv4` (Jazzy's Fast DDS) is set for this process and *not* for the controller's: the controller offers both and the driver takes UDP. With `lo` delayed
    200 ms the UDP-only listener saw the odometry 0.200 s late.
  * **The truth does not travel the faulty path.** `sim/chaos_tap.py` is a participant on the default transports, i.e. shared memory, which the same measurement found 0.000 s late through the
    same delay. Where the base really is, and how many odometry samples had it moving, come from there, by a pipe.

The substrate is `ros2_control`'s own `diff_drive_controller` on mock hardware, started by `sim/chaos-controller.sh` once per profile, and the driver is the one `live.py` runs against it. It is the
ROS project's implementation of the interface and not any base maker's software, and its hardware is never late and has no inertia: **what this shows is the driver's own waits, staleness and stop
path against a link that delays, loses, repeats, reorders, corrupts and blackholes DDS traffic**, and nothing about a Husky, a TurtleBot or what their computers do on a dead link (F15).

The body moves by streaming a velocity, so the contract's "positions" are the pose (x, y) and "a command" is the forward velocity the subject keeps asking for: the base is always on its way,
a late pose is a wrong one, and the age of a read is the distance it is behind divided by the speed. A write that finds no motion running starts one on a thread (a `velocity` is a call that lasts as
long as its `duration`) and returns at once; what the driver says to a refused start is what the write returns. `diff_drive_controller` zeroes its target 0.5 s after the last command it heard, so a
link that stops carrying commands stops the base, and the truth stops with it.
"""

from __future__ import annotations

import json
import os
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(ROOT, "bindings", "python"))

# The driver's DDS over UDP, set before anything of ROS is imported in this process; children get the environment without it (`_child_environment`).
os.environ["FASTDDS_BUILTIN_TRANSPORTS"] = "UDPv4"

import skynet_rhal as rhal  # noqa: E402

from skynet_ros2_base.__main__ import _ros  # noqa: E402
from skynet_ros2_base.bridge import TOKEN, Ros2BaseDriver  # noqa: E402

#: What `sim/chaos_tap.py` writes: x, y, yaw and the forward speed of every odometry message.
RECORD = struct.Struct("<4d")

#: The forward velocity every write asks for, in metres a second: the base is always on its way, so an old pose is a wrong one.
SPEED = 0.3
#: How long one motion streams before the next is started. The driver's longest is five seconds.
MOTION_S = 5.0
#: A forward speed above this is the base moving, for the count of commands it has acted on (the controller's own odometry, from the tap).
MOVING = 0.02

CONTROLLERS = os.environ.get("CHAOS_CONTROLLERS", os.path.join(HERE, "sim", "controllers.yaml"))
COMMAND_TOPIC = os.environ.get("CHAOS_COMMAND_TOPIC", "/diff_drive_controller/cmd_vel")
ODOMETRY_TOPIC = os.environ.get("CHAOS_ODOMETRY_TOPIC", "/diff_drive_controller/odom")


def _child_environment() -> dict[str, str]:
    environment = dict(os.environ)
    environment.pop("FASTDDS_BUILTIN_TRANSPORTS", None)
    return environment


class Subject:
    name = "ros2_base (ros2_control's diff_drive_controller on mock hardware, DDS over UDP on one interface, the kernel's link emulation on it)"
    flows = [("netem", "lo")]
    #: The profiles a kernel queue can carry (`tools/faultproxy/profiles.json` marks them `netem`): `slow-link` is scoped to one direction, `stall` holds and releases, `truncate` and `reset` are not a packet's.
    profiles = ["delay-50ms", "jitter-30ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct", "corrupt-5pct"]
    speed = SPEED
    #: The pose is refused (`ETIMEDOUT`, DEGRADED) after this long without an odometry sample: the driver's own bound, its configuration's `stale_after_s` (default 1 s).
    stale_s = 1.0

    def __init__(self) -> None:
        self.driver = Ros2BaseDriver(_ros)
        self._controller: subprocess.Popen | None = None
        self._tap: subprocess.Popen | None = None
        self._tap_read = -1
        self._tap_thread: threading.Thread | None = None
        self._truth = [0.0, 0.0]
        self._moving = 0
        self._records = 0
        self._motion: threading.Thread | None = None
        self._motion_rc = rhal.SKR_OK
        self._lock = threading.Lock()

    # ── the substrate ──────────────────────────────────────────────────────────────────────────────────────────────────────────────────

    def start(self) -> None:
        """The controller, active, and then the tap, which has to be discovered before any profile is set."""
        environment = _child_environment()
        self._controller = subprocess.Popen(
            ["sh", os.path.join(HERE, "sim", "chaos-controller.sh"), CONTROLLERS],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, start_new_session=True, env=environment,
        )
        lines: list[str] = []
        ready = threading.Event()

        def watch() -> None:
            assert self._controller is not None and self._controller.stdout is not None
            for line in self._controller.stdout:
                lines.append(line.rstrip())
                if line.startswith("controllers up"):
                    ready.set()
            ready.set()

        threading.Thread(target=watch, daemon=True).start()
        if not ready.wait(120) or not any(line.startswith("controllers up") for line in lines):
            said = "\n".join(lines[-12:]) or "(nothing)"
            self.stop()
            raise SystemExit(f"chaos: the controller did not come up. It said:\n{said}")
        self._tap_read, tap_write = os.pipe()
        self._records = 0
        self._tap = subprocess.Popen(
            [sys.executable, os.path.join(HERE, "sim", "chaos_tap.py"), str(tap_write)],
            stdout=subprocess.DEVNULL, stderr=tempfile.TemporaryFile(), pass_fds=(tap_write,), start_new_session=True, env={**environment, "TAP_TOPIC": ODOMETRY_TOPIC},
        )
        os.close(tap_write)
        self._tap_thread = threading.Thread(target=self._listen_to_the_tap, args=(self._tap_read,), daemon=True)
        self._tap_thread.start()
        end = time.monotonic() + 30
        while self._records == 0 and time.monotonic() < end and self._tap.poll() is None:
            time.sleep(0.05)
        if self._records == 0:
            self.stop()
            raise SystemExit("chaos: the second reader of the base never heard an odometry sample, so there is no truth to hold the driver to.")

    def _listen_to_the_tap(self, descriptor: int) -> None:
        buffer = b""
        while True:
            try:
                data = os.read(descriptor, 4096)
            except OSError:
                return
            if not data:
                return
            buffer += data
            while len(buffer) >= RECORD.size:
                x, y, _yaw, speed = RECORD.unpack(buffer[: RECORD.size])
                buffer = buffer[RECORD.size:]
                with self._lock:
                    self._truth = [x, y]
                    if abs(speed) > MOVING:
                        self._moving += 1
                    self._records += 1

    def stop(self) -> None:
        for process in (self._tap, self._controller):
            if process is None:
                continue
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except (ProcessLookupError, PermissionError):
                pass
        for process in (self._tap, self._controller):
            if process is None:
                continue
            try:
                process.wait(timeout=6)
            except subprocess.TimeoutExpired:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except (ProcessLookupError, PermissionError):
                    pass
                process.wait()
        self._tap = self._controller = None
        if self._tap_read >= 0:
            try:
                os.close(self._tap_read)
            except OSError:
                pass
            self._tap_read = -1

    def config(self, addresses) -> str:
        del addresses
        return json.dumps({"command_topic": COMMAND_TOPIC, "odometry_topic": ODOMETRY_TOPIC, "max_linear": 0.5, "max_angular": 1.0, "simulator": True, "open_wait_s": 15.0})

    # ── the driver's four entry points ─────────────────────────────────────────────────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        state = rhal.State()
        rc = self.driver.read_state(state, 0)
        return rc, state.flags, [state.base_pose[0], state.base_pose[1]]

    def target(self, index: int):
        del index  # a velocity is the same one every time: the pose is what moves
        return [0.0, 0.0]

    def _run_motion(self) -> None:
        rc, _ = self.driver.extension(TOKEN, f"velocity vx={SPEED} duration={MOTION_S}".encode())
        with self._lock:
            self._motion_rc = rc

    def write(self, values) -> int:
        """Keep a forward velocity going. A motion lasts as long as its `duration`, so this starts one when none is running and returns at once; while one runs it asks the driver whether the stop is latched."""
        del values
        thread = self._motion
        if thread is not None and thread.is_alive():
            rc, text = self.driver.extension(TOKEN, b"status")
            if rc == rhal.SKR_OK and json.loads(text).get("stop_latched"):
                return rhal.SKR_EPERM
            return rhal.SKR_OK
        with self._lock:
            self._motion_rc = rhal.SKR_OK
        self._motion = thread = threading.Thread(target=self._run_motion, daemon=True)
        thread.start()
        # A refusal is quick and a motion is not: a start that has not come back in a tenth of a second is under way.
        thread.join(0.1)
        if thread.is_alive():
            return rhal.SKR_OK
        with self._lock:
            return self._motion_rc

    def stop_call(self) -> int:
        return self.driver.emergency_stop()

    def release(self) -> None:
        self.driver.release_stop()

    def close(self) -> None:
        self.driver.close()
        thread = self._motion
        if thread is not None:
            thread.join(timeout=3.0)

    # ── what the substrate really did ──────────────────────────────────────────────────────────────────────────────────────────────────

    def truth(self):
        with self._lock:
            return list(self._truth)

    def commands_taken(self) -> int:
        """How many odometry samples had the base moving: it moves only on commands it heard, `diff_drive_controller` zeroing its target 0.5 s after the last."""
        with self._lock:
            return self._moving


def subject() -> Subject:
    return Subject()

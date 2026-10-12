"""The X1 driver's subject for `tools/faultproxy/chaos.py`: the vendor's two ROS 2 topics over DDS on one interface, with the kernel's own link emulation on it.

    make -C drivers/x1 chaos-docker      # in the pinned image: the vendor's message types, ROS 2, `tc` and a loopback of its own

ROS 2 speaks DDS, and DDS names its peers in discovery messages and sends to ports nobody configured, so there is no address to put a relay at. The flow is `("netem", "lo")`: the queue of the
container's own loopback, which the driver (this process, through `rhal_ctypes`) and the fake robot share. Every packet either sends meets the profile, in both directions at once;
`tools/faultproxy/relays.py`'s `netem_arguments` says what a kernel queue cannot do (`truncate`, `stall`, a fault scoped to one direction, `reset`), which `profiles` leaves out.

The driver's DDS is made to use the network: between two processes of one host Fast DDS's default is shared memory, which is not an interface, so `FASTDDS_BUILTIN_TRANSPORTS=UDPv4` is set for this
process and not for the fake's (the fake offers both and the driver takes UDP; measured for the ROS 2 base driver, `drivers/ros2_base/chaos.py`, where the UDP-only listener saw a 200 ms delay in
full). Where the body really is, and how many well-formed commands it has received, come from the fake's `tap=<fd>`: a pipe, because everything on the interface meets the profile and a socket
would be a tap the faults corrupt.

The substrate is `fake_x1` run with `slew=`: the vendor's own message types at both ends, four joints that follow what they are commanded at a fixed speed, so that a late reading is a wrong one.
**The fake agrees with whoever wrote it** (F15): what this shows is the driver's own waits, staleness and stop path against a link that delays, loses, repeats, reorders, corrupts and blackholes
DDS traffic, and nothing about an X1 or what its computer does on a dead link. The vendor's simulator is the rung above and is not run here.

`CHAOS_DRIVER`, `CHAOS_FAKE` and `CHAOS_MODEL` name other binaries and another MJCF.
"""

from __future__ import annotations

import json
import os
import re
import signal
import struct
import subprocess
import sys
import tempfile
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))

# The driver's DDS over UDP, set before the driver is loaded; the fake gets the environment without it.
os.environ["FASTDDS_BUILTIN_TRANSPORTS"] = "UDPv4"

from rhal_ctypes import Driver  # noqa: E402

#: The four joints `fake_x1` serves, in its order, which is the order the driver reports them in.
JOINTS = ("lumbar_yaw_joint", "left_shoulder_pitch_joint", "left_elbow_pitch_joint", "left_knee_pitch_joint")

#: What the fake's tap writes: the four joints' positions as doubles, then the count of well-formed commands it has received, little-endian, nothing between.
RECORD = struct.Struct("<4dQ")

#: How fast the fake's joints follow a command, in radians a second: the contract divides how far behind a reading is by this.
SLEW = 2.0

#: How long one pose is held before the other is asked for: a pose change of 0.6 rad takes 0.3 s at SLEW, so most of the run the body is on its way.
HOLD_S = 0.6

DOMAIN = 42
KP, KD = 50.0, 2.0


def _model_path() -> str:
    if os.environ.get("CHAOS_MODEL"):
        return os.environ["CHAOS_MODEL"]
    infer = os.environ.get("X1_INFER", "/opt/src/x1_infer")
    return os.path.join(infer, "build", "cfg", "sim_module", "model", "mjcf", "robot", "xyber_x1", "xyber_x1_serial.xml")


def _ranges(path: str) -> list[tuple[float, float]]:
    """The MJCF's own range for each of the four joints: the driver clamps to it, so a pose outside it would be tested as a clamp and not as a link."""
    with open(path, encoding="utf-8") as handle:
        xml = handle.read()
    out = []
    for joint in JOINTS:
        at = xml.find(f'name="{joint}"')
        end = xml.find(">", at) if at >= 0 else -1
        found = re.search(r'range="\s*([-+0-9.eE]+)\s+([-+0-9.eE]+)\s*"', xml[at:end]) if at >= 0 and end > at else None
        if found is None:
            raise SystemExit(f"chaos: {path} has no range for {joint}")
        out.append((float(found.group(1)), float(found.group(2))))
    return out


def _poses(ranges):
    """Two far-apart poses, each joint a swing of up to 0.3 rad either side of the middle of its range and no more than two fifths of the way to either end of it."""
    low, high = [], []
    for lower, upper in ranges:
        middle, half = (lower + upper) / 2.0, (upper - lower) / 2.0
        swing = min(0.3, 0.4 * half)
        low.append(middle - swing)
        high.append(middle + swing)
    return (low, high)


class Subject:
    name = "x1 (the vendor's ROS 2 topics over DDS on one interface, the kernel's link emulation on it)"
    flows = [("netem", "lo")]
    #: The profiles a kernel queue can carry (`tools/faultproxy/profiles.json` marks them `netem`): `slow-link` is scoped to one direction, `stall` holds and releases, `truncate` and `reset` are not a packet's.
    profiles = ["delay-50ms", "jitter-30ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct", "corrupt-5pct"]
    speed = SLEW
    #: The longest the driver may go without a new state before a read says so: the watchdog window it declares in its envelope (`X1_WATCHDOG_MS`).
    stale_s = 0.1

    def __init__(self) -> None:
        self.driver = Driver(os.environ.get("CHAOS_DRIVER", os.path.join(HERE, "libskynet_x1.so")))
        self._fake_path = os.environ.get("CHAOS_FAKE", os.path.join(HERE, "fake_x1"))
        if not os.path.exists(self._fake_path):
            raise SystemExit(f"chaos: no fake robot at {self._fake_path}. Build it first: make -C drivers/x1 fake X1_INFER=... ROS_ROOT=...")
        self._model = _model_path()
        self._poses = _poses(_ranges(self._model))
        self._process: subprocess.Popen | None = None
        self._tap_read = -1
        self._tap_thread: threading.Thread | None = None
        self._errors = None
        self._truth = [0.0] * len(JOINTS)
        self._taken = 0
        self._records = 0
        self._origin = time.monotonic()

    def start(self) -> None:
        """The fake robot, publishing on the loopback, and the tap that says where it is. Its first record is how this knows it is publishing."""
        self._tap_read, tap_write = os.pipe()
        self._errors = tempfile.TemporaryFile()
        self._records = 0
        environment = dict(os.environ)
        environment.pop("FASTDDS_BUILTIN_TRANSPORTS", None)
        self._process = subprocess.Popen(
            [self._fake_path, str(DOMAIN), self._model, f"slew={SLEW}", f"tap={tap_write}"],
            stdout=subprocess.DEVNULL, stderr=self._errors, pass_fds=(tap_write,), env=environment,
        )
        os.close(tap_write)
        self._tap_thread = threading.Thread(target=self._listen_to_the_tap, args=(self._tap_read,), daemon=True)
        self._tap_thread.start()
        end = time.monotonic() + 15
        while self._records == 0 and time.monotonic() < end and self._process.poll() is None:
            time.sleep(0.02)
        if self._records == 0:
            self._errors.seek(0)
            said = self._errors.read().decode(errors="replace").strip()
            self.stop()
            raise SystemExit(f"chaos: the fake robot never wrote to its tap. It said: {said or '(nothing)'}")

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
                *positions, taken = RECORD.unpack(buffer[: RECORD.size])
                buffer = buffer[RECORD.size:]
                self._truth, self._taken = positions, taken
                self._records += 1

    def stop(self) -> None:
        if self._process is not None:
            self._process.send_signal(signal.SIGTERM)
            try:
                self._process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._process.kill()
                self._process.wait()
            self._process = None
        if self._tap_thread is not None:
            self._tap_thread.join(timeout=2)
            self._tap_thread = None
        if self._tap_read >= 0:
            os.close(self._tap_read)
            self._tap_read = -1
        if self._errors is not None:
            self._errors.close()
            self._errors = None

    def config(self, addresses) -> str:
        del addresses
        return json.dumps({"domain": DOMAIN, "model": self._model, "kp": KP, "kd": KD, "simulator": True})

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        return self.driver.read(len(JOINTS), with_seq=True)

    def target(self, index: int):
        # Held for HOLD_S at a time whatever the caller's own cadence is, so that the body is on its way from one pose to the other for most of the run.
        del index
        return self._poses[int((time.monotonic() - self._origin) / HOLD_S) & 1]

    def write(self, values) -> int:
        return self.driver.write(values)

    def stop_call(self) -> int:
        return self.driver.emergency_stop()

    def release(self) -> None:
        self.driver.release_stop()

    def close(self) -> None:
        self.driver.close()

    def truth(self):
        return list(self._truth)

    def commands_taken(self) -> int:
        """How many well-formed commands the fake has received: ones that reached it whole, naming all four joints."""
        return self._taken


def subject() -> Subject:
    return Subject()

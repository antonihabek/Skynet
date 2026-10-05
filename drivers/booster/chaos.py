"""The Booster driver's subject for `tools/faultproxy/chaos.py`: DDS, on one interface, with the kernel's own link emulation on that interface.

    make -C drivers/booster chaos BOOSTER_SDK=/path/to/booster_robotics_sdk   # builds the driver and the fake, then runs this in a network namespace of its own

booster_robotics_sdk speaks DDS, and DDS names its peers in discovery messages, sends to multicast groups and to ports nobody configured, so there is no address to point a relay at. The flow is
`("netem", "lo")`: the queue of the namespace's own loopback, which the driver (this process, through `rhal_ctypes`) and the substrate share. Every packet either sends meets the profile, in both
directions at once, and `tools/faultproxy/relays.py`'s `netem_arguments` says what a kernel queue cannot do (`truncate`, `stall`, a fault scoped to one direction, `reset`), which `profiles` leaves out.
`drivers/unitree/chaos.py` is the same subject for the other DDS driver, and the first one the kernel's link emulation was built for.

The substrate is `fake_robot`, the process `make check` runs: the vendor's own channel classes on its side of `rt/low_state` and `rt/joint_ctrl`, so the CDR encoding, the discovery and the topic types
are the maker's code at both ends. Where the body really is, and how many commands it has acted on, come from the fake's `tap=<fd>`: a pipe, because everything on the interface meets the profile and a
socket would be a tap the faults corrupt. **The fake agrees with whoever wrote it** (F15): what this shows is the driver's own waits, staleness and stop path against a link that delays, loses, repeats,
reorders, corrupts and blackholes DDS traffic, and nothing about Booster's robot or what its onboard computer does on a dead link.

The gains are the operator's (the driver refuses a position command without them) and are chosen here for a plant that moves in a way a late reading shows: the fake integrates a unit inertia, so `kp` 600
and `kd` 40 cross a pose change of six tenths of a radian in about two tenths of a second and settle. The poses are held for `HOLD_S` and not changed with every call, because the contract asks for a new
target every few milliseconds and a plant that follows a target flipping at fifty hertz moves a hundredth of a radian, which no stale reading could be told from.

`CHAOS_DRIVER` and `CHAOS_FAKE` name other binaries.
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
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))

from rhal_ctypes import Driver  # noqa: E402

JOINTS = 23

#: What the fake's tap writes (`fake_robot.cpp`'s `Tap`): the twenty-three joints of the serial view as doubles, then the count of commands acted on, little-endian, nothing between.
RECORD = struct.Struct(f"<{JOINTS}dQ")

#: The driver's published travel, radians, in its joint order (the manual's limit table; `booster_bridge.cpp`'s `T1_TRAVEL`).
TRAVEL = [
    (-1.0122910, 1.0122910), (-0.3141593, 0.8203047),
    (-3.2812190, 1.1868239), (-1.6406095, 1.5358897), (-2.2340214, 2.2340214), (-2.0943951, 0.0349066),
    (-3.2812190, 1.1868239), (-1.5358897, 1.6406095), (-2.2340214, 2.2340214), (-0.0349066, 2.0943951),
    (-1.0122910, 1.0122910),
    (-2.0594885, 2.0594885), (-0.3665191, 1.5358897), (-1.0122910, 1.0122910), (0.0000000, 2.1467550), (-0.4014257, 0.8552113), (-0.4188790, 0.7853982),
    (-2.0594885, 2.0594885), (-1.5358897, 0.3665191), (-1.0122910, 1.0122910), (0.0000000, 2.1467550), (-0.4014257, 0.8552113), (-0.4188790, 0.7853982),
]


def _poses():
    """Two far-apart poses, each joint a swing of up to 0.3 rad either side of the middle of its travel and no more than two fifths of the way to either end of it."""
    low, high = [], []
    for lower, upper in TRAVEL:
        middle, half = (lower + upper) / 2.0, (upper - lower) / 2.0
        swing = min(0.3, 0.4 * half)
        low.append(middle - swing)
        high.append(middle + swing)
    return (low, high)


POSES = _poses()

#: How long one pose is held before the other is asked for (see the file's last paragraph but one).
HOLD_S = 0.6

KP, KD = 600.0, 40.0


class Subject:
    name = "booster t1 (DDS on one interface, the kernel's link emulation on it)"
    flows = [("netem", "lo")]
    #: The profiles a kernel queue can carry (`tools/faultproxy/profiles.json` marks them `netem`): `slow-link` is scoped to one direction, `stall` holds and releases, `truncate` and `reset` are
    #: not a packet's.
    profiles = ["delay-50ms", "jitter-30ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct", "corrupt-5pct"]
    #: How fast a joint moves under the gains above between the poses, in radians a second: the contract divides how far behind a reading is by this.
    speed = 2.0
    #: The longest the driver may go without a new state before a read says so (a held sequence, or a flag).
    stale_s = 0.1

    def __init__(self) -> None:
        self.driver = Driver(os.environ.get("CHAOS_DRIVER", os.path.join(ROOT, "drivers", "booster", "libskynet_booster.so")))
        self._fake_path = os.environ.get("CHAOS_FAKE", os.path.join(ROOT, "drivers", "booster", "fake_robot"))
        if not os.path.exists(self._fake_path):
            raise SystemExit(f"chaos: no fake robot at {self._fake_path}. Build it first: make -C drivers/booster fake BOOSTER_SDK=...")
        self._process: subprocess.Popen | None = None
        self._tap_read = -1
        self._tap_thread: threading.Thread | None = None
        self._errors = None
        self._truth = [0.0] * JOINTS
        self._acted_on = 0
        self._records = 0
        self._origin = time.monotonic()

    def start(self) -> None:
        """The fake robot, publishing on the loopback, and the tap that says where it is. Its first record is how this knows it is publishing."""
        self._tap_read, tap_write = os.pipe()
        self._errors = tempfile.TemporaryFile()
        self._records = 0
        # A fake that stops of itself after five minutes, so that one orphaned by a runner that died does not publish for ever in a namespace nobody is in.
        self._process = subprocess.Popen(
            [self._fake_path, "lo", "500", "300", f"tap={tap_write}"],
            stdout=subprocess.DEVNULL,
            stderr=self._errors,
            pass_fds=(tap_write,),
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
                *positions, acted_on = RECORD.unpack(buffer[: RECORD.size])
                buffer = buffer[RECORD.size :]
                self._truth, self._acted_on = positions, acted_on
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
        return json.dumps({"interface": "lo", "kp": KP, "kd": KD, "simulator": True})

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        return self.driver.read(JOINTS, with_seq=True)

    def target(self, index: int):
        # Held for HOLD_S at a time whatever the caller's own cadence is, so that the body is on its way from one pose to the other for most of the run.
        return POSES[int((time.monotonic() - self._origin) / HOLD_S) & 1]

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
        """How many commands the fake has acted on: the ones that reached it whole, in the serial view, with the T1's layout."""
        return self._acted_on


def subject() -> Subject:
    return Subject()

"""The Lite3 driver's subject for `tools/faultproxy/chaos.py`: the two UDP flows between it and a motion host, each through a fault-injecting relay.

    make -C drivers/lite3 chaos            # builds the driver and the fake, then runs this in a network namespace of its own

The substrate is `fake_motion_host`, the process the cross-check and `skynet driver check` run: it listens for MotionSDK's command datagrams and pushes `RobotData` to a port it is
given, 500 times a second, from a clock of its own, so the plant moves whatever the link does. The two flows are the commands (the driver's socket is *connected* to the host's command
port, which here is a relay's) and the state push (the host sends to the port it is told, which here is a relay's, and the relay sends on to the port the driver has bound). That
second flow is the host's own and the driver cannot name it in a message, so the host is started in `attach`, once the relays exist and their addresses are known.

Where the arm really is, and whether the host is *obeying*, come from the fake's optional sixth argument: a tap, to which it sends the twelve joint positions as doubles and the number of
joint commands it has acted on (the ones it received while the SDK held control right) with every push. The tap is not relayed and the driver never sees it. A command is a datagram nothing
answers, so the driver's return code says the socket took it and cannot say the machine did: `commands_taken` is how the contract's sixth question (*effective*) asks. **The fake agrees with whoever wrote it** (F15): what this shows is the driver's own waits, staleness and stop path against a link that delays, loses, repeats,
reorders, cuts short and corrupts datagrams, and nothing about Deep Robotics' motion host.

`CHAOS_DRIVER` and `CHAOS_FAKE` name other binaries. The configuration leaves everything but the gains and the hand-back at the driver's defaults on purpose; the hand-back (what `close`
waits between damping the legs and returning control, two seconds in the vendor's own order) is shortened because a contract that closes the session ten times has no use for it.
"""

from __future__ import annotations

import json
import os
import signal
import socket
import struct
import subprocess
import sys
import threading
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))

from rhal_ctypes import Driver  # noqa: E402

#: Where the fake listens for commands, where the driver binds for the push, and where the fake taps the plant's true positions. Not the vendor's 43893 and 43897: a developer running this
#: outside a network namespace may have a motion host (or the cross-check's) on those.
COMMAND_PORT, STATE_PORT, TAP_PORT = 45893, 45897, 45899

JOINTS = 12

#: What the fake's tap carries (`fake_motion_host.cpp`'s `Tap`): twelve doubles and a counter, little-endian, nothing between them.
TAP_FORMAT = f"<{JOINTS}dQ"
TAP_BYTES = struct.calcsize(TAP_FORMAT)

#: Two far-apart poses for all four legs, each inside every joint's published travel (hip +-0.53, thigh -3.5 to +0.32, knee 0.349 to 2.8): the leg is always on its way from one to
#: the other, so a late reading is a wrong one.
POSES = (
    [0.3, -0.8, 1.2] * 4,
    [-0.3, -0.2, 2.0] * 4,
)


class Subject:
    name = "lite3 (two UDP flows: the commands, and the state the motion host pushes back)"
    flows: list = []
    #: The profiles that are faults of a datagram link (`tools/faultproxy/profiles.json`): `slow-link` and `reset` are a byte stream's.
    profiles = ["delay-50ms", "jitter-30ms", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct", "truncate-5pct", "corrupt-5pct"]
    #: What a joint does under the gains below, in radians a second, measured on a clean link with the poses above (the contract divides how far behind a reading is by this).
    speed = 3.0
    #: The longest `read_state` waits for a push newer than the last it returned (twice the measured interval, held inside 1..100 ms); after that it returns the old one under the old
    #: sequence, which is how it says the push has stopped.
    stale_s = 0.1

    def __init__(self) -> None:
        self.driver = Driver(os.environ.get("CHAOS_DRIVER", os.path.join(ROOT, "drivers", "lite3", "libskynet_lite3.so")))
        self._fake_path = os.environ.get("CHAOS_FAKE", os.path.join(ROOT, "drivers", "lite3", "fake_motion_host"))
        if not os.path.exists(self._fake_path):
            raise SystemExit(f"chaos: no fake motion host at {self._fake_path}. Build it first: make -C drivers/lite3 fake")
        self._process: subprocess.Popen | None = None
        self._tap: socket.socket | None = None
        self._tap_thread: threading.Thread | None = None
        self._truth = [0.0] * JOINTS
        self._acted_on = 0
        self._running = False

    def start(self) -> None:
        self._tap = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self._tap.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._tap.bind(("127.0.0.1", TAP_PORT))
        self._tap.settimeout(0.1)
        self._running = True
        self._tap_thread = threading.Thread(target=self._listen_to_the_tap, daemon=True)
        self._tap_thread.start()
        self.flows = [("udp", ("127.0.0.1", COMMAND_PORT)), ("udp", ("127.0.0.1", STATE_PORT))]

    def _listen_to_the_tap(self) -> None:
        while self._running:
            try:
                data = self._tap.recv(JOINTS * 8 + 16)
            except socket.timeout:
                continue
            except OSError:
                return
            if len(data) >= TAP_BYTES:
                *positions, acted_on = struct.unpack(TAP_FORMAT, data[:TAP_BYTES])
                self._truth, self._acted_on = positions, acted_on

    def attach(self, addresses) -> None:
        """The relays exist: start the motion host pushing to the state relay."""
        state_relay = addresses[1]
        self._process = subprocess.Popen(
            [self._fake_path, str(COMMAND_PORT), "127.0.0.1", str(state_relay[1]), "500", "0", str(TAP_PORT)],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )

    def stop(self) -> None:
        if self._process is not None:
            self._process.send_signal(signal.SIGTERM)
            try:
                self._process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self._process.kill()
                self._process.wait()
            self._process = None
        self._running = False
        if self._tap is not None:
            self._tap.close()
            self._tap = None
        if self._tap_thread is not None:
            self._tap_thread.join(timeout=2)
            self._tap_thread = None

    def config(self, addresses) -> str:
        command_relay = addresses[0]
        return json.dumps({"host": "127.0.0.1", "command_port": command_relay[1], "state_port": STATE_PORT, "kp": 60.0, "kd": 5.0, "handback_ms": 100, "simulator": True})

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        return self.driver.read(JOINTS, with_seq=True)

    def target(self, index: int):
        return POSES[index & 1]

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
        """How many joint commands the motion host has acted on, which counts none it received while the underlying controller held the legs."""
        return self._acted_on


def subject() -> Subject:
    return Subject()

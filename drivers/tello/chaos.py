"""The Tello driver's subject for `tools/faultproxy/chaos.py`: the two UDP flows between it and a drone, each through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/tello python3 tools/faultproxy/chaos.py drivers/tello/chaos.py        # or `make -C drivers/tello chaos`, in a network namespace of its own

The substrate is `DroneSimulator`, the drone the suite uses, flying: the drone answers each command on the socket the command came from (the driver sends to a relay, so it answers the relay), and
pushes its state string ten times a second to the address that last spoke to it and the port it is told (here a relay's, which sends on to the port the driver has bound). That second flow is the
drone's own and the driver cannot name it in a message, so the simulator learns the relay's port in `attach`, once the relays exist.

This body has no joints, so the contract's "positions" are the height (the one thing the state string says in a unit the guide defines) and "a command" is the stick the subject sends: up when the drone
is below the pose it is asked for, down when above. `rc` has no reply, so the driver's return code says the socket took it; `commands_taken` is the simulator's count of the sticks it acted on, which is how the
contract's sixth question (*effective*) asks. **The drone agrees with whoever wrote it** (F15): what this shows is the driver's own waits, staleness and stop path against a link that delays, loses, repeats,
reorders, cuts short and corrupts datagrams, and nothing about a Tello. It also has no way to show that a real Tello obeys an `rc` it receives after a gap, which the guide does not say.
"""

from __future__ import annotations

import json
import socket

import skynet_rhal as rhal

from skynet_tello import bridge as bridge_module
from skynet_tello.bridge import TOKEN, TelloBridge
from skynet_tello.drone_sim import RC_CM_S, DroneSimulator

#: The stick, in percent of the sticks' range, and the speed it gives in metres a second: the drone is always on its way, so an old height is a wrong one.
STICK = 50
SPEED = STICK * RC_CM_S / 100.0 / 100.0
#: Two far-apart heights, in metres, both above the simulated takeoff's 0.8 m.
LOW, HIGH = 1.1, 2.1


class CountingDrone(DroneSimulator):
    """The simulated drone, counting the sticks it acted on: an `rc` it received while it was flying with its motors on (the only ones it takes)."""

    acted = 0

    def _handle(self, text: str, address: tuple[str, int]) -> None:
        if text.startswith("rc "):
            with self._lock:
                if self.flying and self.motors:
                    self.acted += 1
        super()._handle(text, address)


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


class Subject:
    name = "tello (two UDP flows: the commands, and the state string the drone pushes back)"
    #: The profiles that are faults of a datagram link (`tools/faultproxy/profiles.json`): `slow-link` and `reset` are a byte stream's.
    profiles = ["delay-50ms", "jitter-30ms", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct", "truncate-5pct", "corrupt-5pct"]
    #: A stick is one datagram nothing answers, so it is as quick as a joint command is.
    write_bound_s = 0.25
    speed = SPEED

    def __init__(self) -> None:
        self.drone: CountingDrone | None = None
        self.bridge = TelloBridge()
        self.flows: list = []
        #: The state string older than this is not believed: the bound the driver keeps a reading to.
        self.stale_s = bridge_module._STALE_S    # noqa: SLF001
        self._state_port = 0

    def start(self) -> None:
        # The guide's own clock (`time_scale` hurries the physics as well as the takeoff, so the speed this file declares would be wrong by the same factor).
        self.drone = CountingDrone(time_scale=1.0).start()
        self._state_port = free_port()
        self.flows = [("udp", ("127.0.0.1", self.drone.command_port)), ("udp", ("127.0.0.1", self._state_port))]
        self.bridge = TelloBridge()

    def attach(self, addresses) -> None:
        """The relays exist: the drone pushes its state to the state relay, which sends it on to the port the driver binds."""
        assert self.drone is not None
        self.drone.state_port = addresses[1][1]

    def stop(self) -> None:
        if self.drone is not None:
            self.drone.stop()
            self.drone = None

    def config(self, addresses) -> str:
        return json.dumps({"host": "127.0.0.1", "port": addresses[0][1], "state_port": self._state_port, "simulator": True, "stop_mode": "hover", "keepalive_s": 1.0, "reply_timeout_ms": 400, "open_timeout_ms": 4000})

    def open(self, config: str) -> int:
        rc = self.bridge.open(config)
        if rc != rhal.SKR_OK:
            return rc
        assert self.drone is not None
        # Flying: a stick has somewhere to go. A reopened driver finds the drone already up, and `takeoff` is then refused by the drone and not asked again.
        if not self.drone.flying:
            code, said = self.bridge.extension(TOKEN, b"takeoff")
            if code != rhal.SKR_OK:
                print(f"chaos: takeoff was refused ({code}): {said.decode(errors='replace')}")
                return code
        return rhal.SKR_OK

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, [state.base_pose[2]]

    def target(self, index: int):
        return [LOW if index % 2 == 0 else HIGH]

    def write(self, values) -> int:
        assert self.drone is not None
        stick = STICK if values[0] > self.drone.position[2] / 100.0 else -STICK
        code, _ = self.bridge.extension(TOKEN, f"drive up={stick}".encode())
        return code

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        self.bridge.close()

    def truth(self):
        assert self.drone is not None
        return [self.drone.position[2] / 100.0]

    def commands_taken(self) -> int:
        """How many sticks the drone has acted on."""
        return self.drone.acted if self.drone is not None else 0


def subject() -> Subject:
    return Subject()

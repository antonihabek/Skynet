"""The MAVLink bridge's subject for `tools/faultproxy/chaos.py`: one UDP flow between the driver and a vehicle, through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/mavlink python3 tools/faultproxy/chaos.py drivers/mavlink/chaos.py

The substrate is `SimulatedAutopilot`, the vehicle the suite uses, flying in guided mode at a velocity the subject keeps asking for. The driver dials `udpout:` at a
relay, and the relay is what a radio link is to it: it delays, drops, duplicates, reorders, truncates and corrupts datagrams, and it goes silent. **The vehicle agrees
with whoever wrote it** (F15): what this shows is the driver's own timeout, staleness, parser and stop path, and nothing about ArduPilot's or PX4's.

This body has no joints, so the contract's "positions" are the pose (east, north, up) and "a command" is the velocity setpoint every write repeats; the vehicle keeps flying
east at `SPEED`, so a late pose is a wrong one and the age of a read is the distance it is behind divided by the speed.

What the `blackhole` profile's recovery means here: the vehicle has its ground-station link-loss failsafe set (`FS_GCS_ENABLE`, which `open` would otherwise warn about) and
after five seconds without the driver's heartbeat it leaves guided mode for BRAKE, as a real one does. When the link is healed the driver reads again, and the next velocity is
refused because the vehicle is not in guided: the driver does not put a vehicle back into a mode its own failsafe took it out of, and the contract's allowance for "a named
refusal that a re-open clears" is what this row rests on (`open` below asks for guided, arms and takes off again). Measured, 2026-10-03: 58 checks, none failed.
"""

from __future__ import annotations

import json
import math

import skynet_rhal as rhal

from skynet_mavlink import bridge as bridge_module
from skynet_mavlink import frames
from skynet_mavlink.autopilot import SimulatedAutopilot
from skynet_mavlink.bridge import TOKEN, MavlinkBridge

#: The velocity every write asks for, east, in metres a second: the vehicle is always on its way, so an old pose is a wrong one.
SPEED = 2.0
ALTITUDE = 8.0


class Subject:
    name = "mavlink (one UDP flow, both ways, to a simulated ArduPilot multirotor in guided flight)"
    #: The profiles that are faults of a datagram link and that the contract applies to one (`tools/faultproxy/profiles.json`): `slow-link` and `reset` are a byte stream's.
    profiles = ["delay-50ms", "jitter-30ms", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "duplicate-20pct", "reorder-20pct", "truncate-5pct", "corrupt-5pct"]
    #: A velocity setpoint is one datagram with nothing waiting for an answer, so it is as quick as a joint command is.
    write_bound_s = 0.25

    def __init__(self) -> None:
        self.vehicle: SimulatedAutopilot | None = None
        self.bridge = MavlinkBridge()
        self.flows: list = []
        self.speed = SPEED
        #: The pose goes NaN and the state DEGRADED after this long without a position: the bound the driver keeps a pose to.
        self.stale_s = bridge_module._POSE_STALE_S

    def start(self) -> None:
        self.vehicle = SimulatedAutopilot("copter", transport="udp")
        self.vehicle.gcs_failsafe = 1.0  # a vehicle with a link-loss failsafe set, so `open` has nothing to warn about
        self.vehicle.ekf_ready_at = 0.0
        self.vehicle.heartbeat_period = 0.1
        self.vehicle.start()
        self.flows = [("udp", ("127.0.0.1", self.vehicle.port))]
        self.bridge = MavlinkBridge()

    def stop(self) -> None:
        if self.vehicle is not None:
            self.vehicle.stop()
            self.vehicle = None

    def config(self, addresses) -> str:
        host, port = addresses[0]
        return json.dumps({"link": f"udpout:{host}:{port}", "command_timeout_ms": 200, "command_tries": 2, "open_timeout_ms": 4000})

    def open(self, config: str) -> int:
        rc = self.bridge.open(config)
        if rc != rhal.SKR_OK:
            return rc
        assert self.vehicle is not None
        # Flying, in guided mode: a velocity setpoint has somewhere to go. A reopened driver finds the vehicle already up, and each of these is then a no-op.
        for request in ("mode name=guided", "arm", f"takeoff alt={ALTITUDE}"):
            code, _ = self.bridge.extension(TOKEN, request.encode())
            if code != rhal.SKR_OK and not (request == "arm" and self.vehicle.armed):
                return code
        if not self.vehicle.wait(lambda: self.vehicle.landed == frames.Landed.IN_AIR, 12.0):
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.base_pose[:3])

    def target(self, index: int):
        del index  # a velocity is the same one every time: the pose is what moves
        return [0.0]

    def write(self, values) -> int:
        del values
        code, _ = self.bridge.extension(TOKEN, f"velocity vx={SPEED} vy=0 vz=0".encode())
        return code

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        self.bridge.close()

    def truth(self):
        assert self.vehicle is not None
        east, north, up = self.vehicle.position_enu()
        return [east, north, up]


def subject() -> Subject:
    return Subject()

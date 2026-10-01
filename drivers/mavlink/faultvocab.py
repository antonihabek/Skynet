"""The MAVLink bridge's subject for `tools/fault-vocab/contract.py`: the vehicle's own state word, `HEARTBEAT.system_status` (`MAV_STATE`).

    PYTHONPATH=bindings/python:drivers/mavlink python3 tools/fault-vocab/contract.py drivers/mavlink/faultvocab.py

The substrate is `SimulatedAutopilot`, the vehicle the suite uses, armed and flying in guided mode, so that a motion has somewhere to go. It is
made to say what a vehicle says — a heartbeat with another state in it — and **it agrees with whoever wrote it** (F15): what this holds is what
the bridge does with a state a vehicle reports, never what a firmware reports when, or in which of these states a real vehicle would be.

This body is moved by motions (`vendor.mavlink.task`), not joint commands, so "a command" here is the motion that takes the vehicle somewhere new
(`velocity`) and the vocabulary's `keeps` are the motions a person most wants in a failsafe (`drivers/fault-vocabulary.json`).
"""

from __future__ import annotations

import json

import skynet_rhal as rhal

from skynet_mavlink import frames
from skynet_mavlink.autopilot import SimulatedAutopilot
from skynet_mavlink.bridge import MavlinkBridge, TOKEN

#: The request each motion in `keeps` is made with. `disarm` in the air needs `force=1`, the driver's own rule, which a person in a failsafe
#: is the one to say.
REQUESTS = {"hold": "hold", "rtl": "rtl", "land": "land", "disarm": "disarm force=1"}
ALTITUDE = 5.0


class Subject:
    driver = "mavlink"
    name = "mavlink (the bridge against a simulated ArduPilot multirotor, armed and flying in guided mode)"
    #: The vehicle sends a heartbeat every 0.1 s here (the real one, every second); a changed state is in the next read or two.
    settle_s = 1.5

    def __init__(self) -> None:
        self.vehicle: SimulatedAutopilot | None = None
        self.bridge: MavlinkBridge | None = None

    def start(self) -> None:
        self.vehicle = SimulatedAutopilot("copter")
        self.vehicle.gcs_failsafe = 1.0  # a vehicle with its link-loss failsafe set, so `open` has nothing to warn about
        self.vehicle.ekf_ready_at = 0.0
        self.vehicle.heartbeat_period = 0.1
        self.vehicle.start()
        self.bridge = MavlinkBridge()
        config = {"link": f"tcp:127.0.0.1:{self.vehicle.port}", "command_timeout_ms": 200, "command_tries": 2, "open_timeout_ms": 4000}
        rc = self.bridge.open(json.dumps(config))
        if rc != rhal.SKR_OK:
            raise SystemExit(f"faultvocab: the bridge did not open against the simulated vehicle (rc {rc})")
        for request in ("mode name=guided", "arm", f"takeoff alt={ALTITUDE}"):
            code, text = self._ask(request)
            if code != rhal.SKR_OK:
                raise SystemExit(f"faultvocab: `{request}` was refused at the start (rc {code}: {text})")
        if not self.vehicle.wait(lambda: self.vehicle.landed == frames.Landed.IN_AIR, 8.0):
            raise SystemExit("faultvocab: the simulated vehicle did not reach its altitude")

    def restart(self) -> None:
        self.stop()
        self.start()

    def stop(self) -> None:
        if self.bridge is not None:
            self.bridge.close()
            self.bridge = None
        if self.vehicle is not None:
            self.vehicle.stop()
            self.vehicle = None

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        assert self.vehicle is not None
        self.vehicle.system_status = code

    def clear(self, vocabulary: str) -> None:
        assert self.vehicle is not None
        self.vehicle.system_status = None
        self.restore()

    def restore(self) -> None:
        """What the world is put back to after a motion: armed and level in the air in guided mode, the vehicle's own bookkeeping reset."""
        vehicle = self.vehicle
        assert vehicle is not None
        with vehicle._lock:  # noqa: SLF001 — the simulator's own world, which a test reaches into by design
            vehicle.mode = vehicle.modes["guided"]
            vehicle.armed = True
            vehicle.landed = frames.Landed.IN_AIR
            vehicle.n = vehicle.e = 0.0
            vehicle.d = -ALTITUDE
            vehicle.vn = vehicle.ve = vehicle.vd = 0.0
            vehicle._goto = vehicle._setpoint = vehicle._takeoff_alt = None  # noqa: SLF001

    def _ask(self, request: str) -> tuple[int, str]:
        assert self.bridge is not None
        code, out = self.bridge.extension(TOKEN, request.encode())
        return code, out.decode(errors="replace")

    def read(self) -> tuple[int, int, int]:
        assert self.bridge is not None
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, state.fault_code

    def write(self) -> int:
        """The motion that takes the vehicle somewhere new."""
        return self._ask("velocity vx=0 vy=1 vz=0")[0]

    def motion(self, name: str) -> int:
        return self._ask(REQUESTS[name])[0]


def subject() -> Subject:
    return Subject()

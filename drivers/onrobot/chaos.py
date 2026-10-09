"""The OnRobot bridge's subject for `tools/faultproxy/chaos.py`: the gripper's own link, Modbus TCP to a Compute Box or Modbus RTU on the tool's RS-485, through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/onrobot python3 tools/faultproxy/chaos.py drivers/onrobot/chaos.py                          # Modbus TCP (the default)
    ONROBOT_CHAOS_LINK=serial PYTHONPATH=... python3 tools/faultproxy/chaos.py drivers/onrobot/chaos.py                            # Modbus RTU on a pseudo-terminal pair

The substrate is `SimulatedGripper`, the one the suite uses, served on the substrate's end of the relay: Modbus frames, a gripper that moves at the speed it is told, and the registers `frames.py` documents. The driver
is opened on the other end, which is what a cable, a Compute Box or a USB RS-485 adapter is to it. **The simulator agrees with whoever wrote it** (F15): what this shows is the driver's own transaction budget,
exchange cycle, staleness bound and stop path, and nothing about the maker's firmware.

The profiles are the ones that apply to each link. TCP delivers every byte or none, so it has no truncated or corrupted frame to show, and a serial line (unlike TCP) has no connection to reset. Nothing in the driver
may wait on any of them for longer than its budgets.
"""

from __future__ import annotations

import json
import os

import skynet_rhal as rhal

from skynet_onrobot import bridge as bridge_module
from skynet_onrobot.bridge import OnRobotBridge
from skynet_onrobot.gripper_sim import RG_SPEED_MM_S, SimulatedGripper

#: Two far-apart widths, each inside an RG2's stroke (metres): the fingers are always on their way from one to the other, so a late reading is a wrong one.
POSES = ([0.02], [0.09])
LINK = os.environ.get("ONROBOT_CHAOS_LINK", "tcp")
if LINK not in ("tcp", "serial"):
    raise SystemExit(f"ONROBOT_CHAOS_LINK is {LINK!r}: it is tcp or serial.")


class Subject:
    name = f"onrobot ({'the Compute Box link: Modbus TCP' if LINK == 'tcp' else 'the tool bus: Modbus RTU on a serial line'})"
    #: `reset` is not in the TCP list, for the reason `drivers/schunk/chaos.py` gives: a connection the Compute Box dropped is made again by the next transaction while the box is still there, so the link carries data
    #: throughout and "outage reported" has no outage to report (0 of 10 reads were not OK, which is the driver doing its job). What `selftest.py` holds is that a dropped connection is made again and the gripper read
    #: and commanded again, that a Compute Box which accepts and will not talk is retried at most twice a second, and that one which is not listening is reported as a line that could not be opened; a persistent
    #: outage is `blackhole`'s, and it is here.
    profiles = (["delay-2ms", "jitter-2ms", "slow-link", "stall-1500ms", "blackhole"] if LINK == "tcp"
                else ["delay-2ms", "jitter-2ms", "slow-link", "stall-1500ms", "blackhole", "loss-10pct", "burst-loss", "truncate-5pct", "corrupt-5pct"])
    #: A metre a second is what the simulator's RG2 crosses at (100 mm/s), the figure the simulator chose and the maker does not publish.
    speed = RG_SPEED_MM_S["RG2"] / 1000.0

    def __init__(self) -> None:
        self.gripper: SimulatedGripper | None = None
        self.bridge = OnRobotBridge()
        self.flows: list = [("serial", None)] if LINK == "serial" else []
        self.stale_s = bridge_module._STALE_S

    def start(self) -> None:
        self.gripper = SimulatedGripper("RG2", time_scale=1.0).start()
        if LINK == "tcp":
            self.flows = [("tcp", ("127.0.0.1", self.gripper.port))]
        self.bridge = OnRobotBridge()

    def attach(self, addresses) -> None:
        assert self.gripper is not None
        if LINK == "serial":
            self.gripper.attach(addresses[0][1])

    def stop(self) -> None:
        if self.gripper is not None:
            self.gripper.stop()
            self.gripper = None

    def config(self, addresses) -> str:
        link = {"host": "127.0.0.1", "port": addresses[0][1]} if LINK == "tcp" else {"device": addresses[0][0]}
        return json.dumps({**link, "model": "RG2", "stop_mode": "stop", "simulator": True, "force_n": 20.0, "limits": [0.0, 0.11], "rate_hz": 50.0, "open_timeout_ms": 3000,
                           "roles": {"jaw": "right_hand.gripper.grip"}})

    def open(self, config: str) -> int:
        return self.bridge.open(config)

    def read(self):
        state = rhal.State()
        rc = self.bridge.read_state(state, 0)
        return rc, state.flags, list(state.position_rad)

    def target(self, index: int):
        return POSES[index & 1]

    def write(self, values) -> int:
        command = rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=list(values), mask=[1])
        return self.bridge.write_command(command, 0)

    def stop_call(self) -> int:
        return self.bridge.emergency_stop()

    def release(self) -> None:
        self.bridge.release_stop()

    def close(self) -> None:
        self.bridge.close()

    def truth(self):
        return [self.gripper.width_mm / 1000.0] if self.gripper is not None else [0.0]


def subject() -> Subject:
    return Subject()

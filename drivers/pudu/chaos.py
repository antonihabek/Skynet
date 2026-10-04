"""The Pudu driver's subject for `tools/faultproxy/chaos.py`: the cloud's one HTTP flow, through a fault-injecting relay.

    PYTHONPATH=bindings/python:drivers/pudu python3 tools/faultproxy/chaos.py drivers/pudu/chaos.py

The substrate is `fake_cloud`, the one the suite uses: the OpenAPI endpoints the driver speaks on loopback HTTP, with the signature checked on every request. The driver is pointed at the relay with a `base_url`.
The robot does not move (a cloud has no robot to move here), so there is no freshness question, and the driver has no joints, so the "command" the contract sends is the one route that moves this body: a task
under `vendor.pudu.task`. **The fake agrees with whoever wrote it** (F15): what this shows is the driver's own timeout and stop path against a link that is worse than a datacentre's, and nothing about Pudu's cloud.
"""

from __future__ import annotations

import json
import threading

import skynet_rhal as rhal

from skynet_pudu.bridge import TOKEN, PuduDriver
from skynet_pudu.fake_cloud import TEST_KEY, TEST_SECRET, serve


class Subject:
    name = "pudu (the cloud's HTTP API through one TCP relay)"
    #: Not `reset`, for the reason `drivers/reachy_mini/chaos.py` gives: every call here opens its own connection, so a reset is a failed request and the next call is a new one; there is no connection to be
    #: wedged. A persistent outage is `blackhole`'s.
    profiles = ["delay-50ms", "jitter-30ms", "slow-link", "stall-1500ms", "blackhole"]
    #: The base does not move and the cloud publishes a fixed pose, so a late reading is not a wrong one.
    speed = 0.0
    #: The driver keeps no state between calls (every read is two cloud requests), so its staleness bound is the read's own: how long a call may take before it says the link is down (`_READ_BUDGET_S`, 3 s).
    stale_s = 3.0
    #: No loop is closed over a cloud, so a read is bounded by its budget and not by the control loop's two seconds; and the one route that moves this body is a task request, which is not shortened to meet a
    #: quarter of a second: a timeout does not say the task was not made, so it waits as long as the driver's ten seconds. These are the driver's declared bounds, and the contract holds it to them.
    read_bound_s = 3.5
    write_bound_s = 10.5

    def __init__(self) -> None:
        self.server = None
        self.thread: threading.Thread | None = None
        self.driver = PuduDriver()
        self.flows: list = []

    def start(self) -> None:
        self.server, _ = serve(0)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.flows = [("tcp", ("127.0.0.1", self.server.server_port))]
        self.driver = PuduDriver()

    def stop(self) -> None:
        if self.server is not None:
            self.server.shutdown()
            self.server.server_close()
            self.server = None

    def config(self, addresses) -> str:
        return json.dumps({"base_url": f"http://127.0.0.1:{addresses[0][1]}", "sn": "CHAOS-0001", "app_key": TEST_KEY, "app_secret": TEST_SECRET, "simulator": True})

    def open(self, config: str) -> int:
        return self.driver.open(config)

    def read(self):
        state = rhal.State()
        rc = self.driver.read_state(state, 0)
        return rc, state.flags, list(state.base_pose[:3]) if rc == rhal.SKR_OK else []

    def target(self, index: int):
        return [float(index & 1)]

    def write(self, values) -> int:
        # The only route that moves this body is a task: a call to a map point.
        code, _ = self.driver.extension(TOKEN, f"call point=P{int(values[0])}".encode())
        return code

    def stop_call(self) -> int:
        return self.driver.emergency_stop()

    def release(self) -> None:
        self.driver.release_stop()

    def close(self) -> None:
        self.driver.close()

    def truth(self):
        return [1.5, 2.5, 0.0]


def subject() -> Subject:
    return Subject()

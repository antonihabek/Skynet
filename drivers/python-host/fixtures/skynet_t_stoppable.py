"""A fixture child whose extension runs until it is stopped, for the host's stop-during-extension checks (DRY-COVERAGE-PLAN.md F234).

Run by `stop_check` through `drivers/python-host` (never by hand):

    SKYNET_PYTHON_DRIVER=skynet_t_stoppable PYTHONPATH=bindings/python:fixtures ...

One token, `test.run`, which runs for `SKYNET_FIXTURE_RUN_S` seconds (default 1.5), as a streamed motion does, and answers `OK` and "ran" if it was left to finish, or `EPERM` and
"stopped" if `emergency_stop` was called while it ran. `SKYNET_FIXTURE_DECLARES=1` is the driver saying that its `emergency_stop` is safe to call from another thread while an
extension is running (`stop_during_extension`), which is what lets the serve loop act on the host's `E` at once instead of when the extension returns; unset, the child is one that has
not said so, and the stop waits for the run to end, which is what every child did before.
"""

from __future__ import annotations

import os
import threading
import time

import skynet_rhal as rhal


class StoppableDriver:
    def __init__(self) -> None:
        self._stopped = threading.Event()
        self.stop_calls = 0
        if os.environ.get("SKYNET_FIXTURE_DECLARES") == "1":
            self.stop_during_extension = True

    def open(self, config: str) -> int:
        return rhal.SKR_OK

    def close(self) -> int:
        return rhal.SKR_OK

    def read_state(self, out, deadline_ns: int) -> int:
        out.seq += 1
        out.stamp_ns = rhal.acquisition_ns()
        return rhal.SKR_OK

    def write_command(self, cmd, deadline_ns: int) -> int:
        return rhal.SKR_EPERM

    def emergency_stop(self) -> int:
        self.stop_calls += 1
        self._stopped.set()
        return rhal.SKR_OK

    def release_stop(self) -> int:
        self._stopped.clear()
        return rhal.SKR_OK

    def describe(self) -> str:
        return '{"schema_version":1,"robot_slug":"stoppable-fixture","dof":0,"joints":[],"extensions":["test.run"]}'

    def read_sensor(self, name: str, out) -> int:
        return rhal.SKR_ENOSYS

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != "test.run":
            return (rhal.SKR_ENOSYS, b"unknown token " + token.encode("utf-8"))
        if self._stopped.is_set():
            return (rhal.SKR_EPERM, b"stopped before it began")
        run_s = float(os.environ.get("SKYNET_FIXTURE_RUN_S", "1.5"))
        began = time.monotonic()
        while time.monotonic() - began < run_s:
            if self._stopped.wait(0.005):
                return (rhal.SKR_EPERM, b"stopped")
        return (rhal.SKR_OK, b"ran")


def main() -> int:
    return rhal.serve(StoppableDriver(), ceiling="guest", name="stoppable-fixture", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())

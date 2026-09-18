"""A one-joint Python body with a camera slower than a control period.

The Python analogue of `crates/skynet-kernel-host/tests/fixtures/slow_camera.c`, and it exists for
the same reason that one does: to put §89 on something real rather than argue it. This is the half
the C fixture cannot reach — the *host*, which forwards every RHAL call to an interpreter, and which
before protocol 2 had one pipe for all of them.

Not a driver anybody should copy. What it is for is the two claims `Makefile`'s `check-concurrent`
target makes:

* a Python driver **can** declare §89.1's promise and `skynet driver check` exercises it (§89.4);
* a Python driver that declares it falsely **fails** that check.

``--config`` decides which:

    {"render_ms": 30}                     undeclared: read on the control thread (§89.2)
    {"render_ms": 30, "concurrent": true} declared, and true
    {"render_ms": 30, "lie": true}        declared, and false — the control

── Why the declaration is honest when it is made ───────────────────────────────

§89.1's promise is that ``read_sensor`` for this sensor may run on a second thread while
``read_state`` and ``write_command`` run, without making them wait. Two things have to hold and both
are visible in twenty lines here.

**No shared mutable state.** ``read_sensor`` touches ``self._camera`` and nothing else;
``read_state`` and ``write_command`` touch ``self._joint`` and nothing else. That is exactly what
the drivers in this repository cannot say — the LeRobot bridge's camera frames are a by-product of
the ``get_observation()`` that reads the joints, and MuJoCo renders the same ``mjData`` the step
writes — which is DRY-COVERAGE-PLAN.md F46.

**And the GIL.** Two threads in one CPython process do not run bytecode at the same time, so a
``read_sensor`` that spends its time in Python delays the control thread whatever it declares. The
render here is ``time.sleep``, which releases the GIL, so the promise holds for the same reason it
holds for a real driver whose render is inside a C extension that releases it. A driver whose
expensive part is Python may not declare this, and nothing but the vendor can tell.
"""

from __future__ import annotations

import json
import sys
import threading
import time

import skynet_rhal as rhal
from skynet_rhal import Driver, Frame, State, Command

WIDTH = 8
HEIGHT = 8
CHANNELS = 3


class SlowCamera(Driver):
    """One revolute joint and one deliberately expensive camera."""

    def __init__(self) -> None:
        self._render_ms = 30
        self._concurrent = False
        self._lie = False
        self._opened = False
        # The joint's, touched only by read_state and write_command.
        self._joint = {"position": 0.0, "setpoint": 0.0, "seq": 0}
        # The camera's, touched only by read_sensor.
        self._camera = {"frames": 0, "pixels": bytes(WIDTH * HEIGHT * CHANNELS)}
        # The lie: one scratch both paths use, held across a render. A lock rather than a plain
        # flag because the point is to reproduce what a wrapper driver does — reach for the one
        # object it has and make the other path wait or fail on it.
        self._scratch = threading.Lock()

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except ValueError:
            return rhal.SKR_EINVAL
        self._render_ms = int(settings.get("render_ms", 30))
        self._lie = settings.get("lie") is True
        self._concurrent = self._lie or settings.get("concurrent") is True
        self._opened = True
        return rhal.SKR_OK

    def close(self) -> int:
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: State, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        if self._lie:
            # The camera is holding the buffer this path would fill, so there is no state to
            # return. Answering rather than waiting keeps §24's first rule — wait-free — and makes
            # the defect visible instead of turning it into a missed period.
            if not self._scratch.acquire(blocking=False):
                return rhal.SKR_ETIMEDOUT
            self._scratch.release()
        self._joint["seq"] += 1
        self._joint["position"] = self._joint["setpoint"]
        out.stamp_ns = time.monotonic_ns()
        out.seq = self._joint["seq"]
        out.flags = rhal.HOMING_NOT_REQUIRED
        out.fault_code = 0
        out.position_rad = [self._joint["position"]]
        out.velocity_rad_s = [0.0]
        out.effort_nm = [float("nan")]
        return rhal.SKR_OK

    def write_command(self, cmd: Command, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        # Shape before permission, and an unknown mode before either: a mode this ABI does not
        # define is malformed (SKR_EINVAL) and a mode this body does not offer is refused
        # (SKR_EPERM), and `skynet driver check` tells the two apart because a driver that folded
        # them would let an unrecognised torque command arrive as a position command.
        if cmd.mode not in (rhal.MODE_POSITION, rhal.MODE_VELOCITY, rhal.MODE_TORQUE):
            return rhal.SKR_EINVAL
        if len(cmd.values) != 1 or len(cmd.mask) != 1:
            return rhal.SKR_EINVAL
        if cmd.mode != rhal.MODE_POSITION:
            return rhal.SKR_EPERM
        if cmd.mask[0]:
            self._joint["setpoint"] = float(cmd.values[0])
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        return rhal.SKR_OK

    def read_sensor(self, name: str, out: Frame) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        if name != "wrist":
            return rhal.SKR_ENODEV
        acquired = time.monotonic_ns()
        if self._lie:
            with self._scratch:
                time.sleep(self._render_ms / 1000.0)
        else:
            time.sleep(self._render_ms / 1000.0)
        self._camera["frames"] += 1
        fill = self._camera["frames"] & 0xFF
        self._camera["pixels"] = bytes([fill]) * (WIDTH * HEIGHT * CHANNELS)
        out.stamp_ns = acquired
        out.width = WIDTH
        out.height = HEIGHT
        out.stride = WIDTH * CHANNELS
        out.format = 0  # SKR_FMT_RGB8
        out.data = self._camera["pixels"]
        return rhal.SKR_OK

    def describe(self) -> str:
        sensor = {
            "name": "wrist",
            "role": None,
            "modality": "rgb",
            "width": WIDTH,
            "height": HEIGHT,
            "fps": None,
            "frame": None,
        }
        if self._concurrent:
            sensor["concurrent_read"] = True
        return json.dumps(
            {
                "schema_version": 1,
                "robot_slug": "slow-camera-py",
                "digest": None,
                "verified": False,
                "simulator": True,
                "base": "fixed",
                "dof": 1,
                "joints": [
                    {
                        "name": "J1",
                        "role": "right_arm.wrist.roll",
                        "type": "continuous",
                        "limits": {"position_min_rad": -3.14, "position_max_rad": 3.14},
                        "control_modes": ["position"],
                    }
                ],
                "end_effectors": [],
                "sensors": [sensor],
                "control": {
                    "modes": ["position"],
                    "rate_hz_max": 1000,
                    "rate_hz_min": 10,
                    "command_latency_ms": None,
                },
                "envelope": None,
                "solvers": [],
            }
        )


def main() -> int:
    return rhal.serve(SlowCamera(), name="slow-camera-py", version="0.1.0")


if __name__ == "__main__":
    sys.exit(main())

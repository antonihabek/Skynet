"""A fixture child with a frame the pipe cannot carry, for the host's `F` verb checks.

Run by `frame_check` through `drivers/python-host` (never by hand):

    SKYNET_PYTHON_DRIVER=skynet_big_frame PYTHONPATH=bindings/python:fixtures ...

Two sensors: `big` answers 100,000 bytes — past the 64 KiB line both sides honour — and
`small` answers 100. The big read must arrive as `EINVAL` with the stream still in sync, so a
small read and a `read_state` straight after it still work (DRY-COVERAGE-PLAN.md F172).
"""

from __future__ import annotations

import skynet_rhal as rhal


class BigFrameDriver:
    def open(self, config: str) -> int:
        return rhal.SKR_OK

    def close(self) -> int:
        return rhal.SKR_OK

    def read_state(self, out, deadline_ns: int) -> int:
        out.seq = getattr(self, "_seq", 0) + 1
        self._seq = out.seq
        return rhal.SKR_OK

    def write_command(self, cmd, deadline_ns: int) -> int:
        return rhal.SKR_EPERM

    def emergency_stop(self) -> int:
        return rhal.SKR_OK

    def release_stop(self) -> int:
        return rhal.SKR_OK

    def describe(self) -> str:
        return (
            '{"schema_version":1,"robot_slug":"frame-fixture","dof":0,"joints":[],'
            '"sensors":[{"name":"big","modality":"rgb","width":400,"height":250},'
            '{"name":"small","modality":"rgb","width":10,"height":10}]}'
        )

    def read_sensor(self, name: str, out) -> int:
        if name == "big":
            out.stamp_ns = 1
            out.width = 400
            out.height = 250
            out.stride = 1200
            out.format = 0
            out.data = bytes(100_000)
            return rhal.SKR_OK
        if name == "small":
            out.stamp_ns = 2
            out.width = 10
            out.height = 10
            out.stride = 30
            out.format = 0
            out.data = bytes(100)
            return rhal.SKR_OK
        return rhal.SKR_ENODEV


def main() -> int:
    return rhal.serve(BigFrameDriver(), ceiling="guest", name="frame-fixture", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())

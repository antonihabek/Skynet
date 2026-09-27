"""A fixture child that implements `extension`, for the host's `T` verb checks.

Run by `extension_check` through `drivers/python-host` (never by hand):

    SKYNET_PYTHON_DRIVER=skynet_t_extension PYTHONPATH=bindings/python:fixtures ...

Three behaviours behind one token (`test.echo`): echo the request bytes back, answer `ENOSYS`
with words on any other token, and raise on the request `boom` — which must arrive as an in-frame
`EIO` rather than a desynchronised stream.
"""

from __future__ import annotations

import sys

import skynet_rhal as rhal


class EchoDriver:
    def open(self, config: str) -> int:
        return rhal.SKR_OK

    def close(self) -> int:
        return rhal.SKR_OK

    def read_state(self, out, deadline_ns: int) -> int:
        return rhal.SKR_OK

    def write_command(self, cmd, deadline_ns: int) -> int:
        return rhal.SKR_EPERM

    def emergency_stop(self) -> int:
        return rhal.SKR_OK

    def release_stop(self) -> int:
        return rhal.SKR_OK

    def describe(self) -> str:
        return (
            '{"schema_version":1,"robot_slug":"t-fixture","dof":0,"joints":[],'
            '"extensions":["test.echo"]}'
        )

    def read_sensor(self, name: str, out) -> int:
        return rhal.SKR_ENOSYS

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != "test.echo":
            return (rhal.SKR_ENOSYS, b"unknown token " + token.encode("utf-8"))
        if data == b"boom":
            raise RuntimeError("the fixture explodes on demand")
        return (rhal.SKR_OK, b"echo:" + data)


def main() -> int:
    return rhal.serve(EchoDriver(), ceiling="guest", name="t-fixture", version="0.1.0")


if __name__ == "__main__":
    raise SystemExit(main())

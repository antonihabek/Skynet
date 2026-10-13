"""`frozen.py` asked of a driver that does what it is told, so that its questions can be seen to fail (DRY-COVERAGE-PLAN.md item 12.C.4, F332).

    make -C drivers/ros2_control frozen-selftest

`frozen.py` is run by `frozen-ursim-docker` and `frozen-drcf-docker` against two makers' plugins, and the question those two runs ask the other way round (*outage reported*, with
`FROZEN_OUTAGE_REPORTED=no`) passes while the plugin stays silent. A check that passes in the only two places it is run has not been shown able to fail, so this runs the script's
`main()` against a stand-in for the driver that is silent, or says the controller has gone and keeps saying it, or says it once and then goes quiet again, with the controller's half of
the handshake already in place, and reads which checks failed from what the script prints.

Nothing here starts a container or loads a library.
"""

from __future__ import annotations

import contextlib
import io
import os
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import frozen  # noqa: E402

OK, BUSY = 0, 16


class StandIn:
    """What `rhal_ctypes.Driver` is to `frozen.py`: open, read, write, stop, close. `behaviour` is what a read says once the controller has been frozen
    (the script's `ready` file exists and its `measured` file does not yet): `silent` is a plugin that returns OK with the last values, `reports` one that returns an error from then on, `flaps` one that
    says it once and then returns OK again."""

    behaviour = "silent"
    slow_write = False
    directory = ""
    reads_after_freeze = 0

    def __init__(self, _lib: str) -> None:
        self.name = "stand-in"
        self.stopped = False
        self.seq = 0

    def open(self, _config: str) -> int:
        return OK

    def frozen(self) -> bool:
        return os.path.exists(os.path.join(self.directory, "ready")) and not os.path.exists(os.path.join(self.directory, "measured"))

    def read(self, joints: int, with_seq: bool = False):
        self.seq += 1
        rc = OK
        if self.frozen():
            StandIn.reads_after_freeze += 1
            if self.behaviour == "reports":
                rc = BUSY
            elif self.behaviour == "flaps":
                rc = BUSY if StandIn.reads_after_freeze == 3 else OK
        return rc, 0, [0.1] * joints, self.seq

    def write(self, _values) -> int:
        if self.slow_write and self.frozen():
            time.sleep(0.3)
        return 1 if self.stopped else OK

    def emergency_stop(self) -> int:
        self.stopped = True
        return OK

    def release_stop(self) -> int:
        self.stopped = False
        return OK

    def close(self) -> int:
        return OK


def run(behaviour: str, outage_reported: str | None, slow_write: bool = False, write_bounded: str | None = None) -> tuple[int, str]:
    """`frozen.main()` against a stand-in that behaves as said; returns (the number of failed checks, what it printed)."""
    with tempfile.TemporaryDirectory() as directory:
        for word in ("frozen", "thawed"):
            open(os.path.join(directory, word), "w", encoding="utf-8").close()
        StandIn.behaviour = behaviour
        StandIn.slow_write = slow_write
        StandIn.directory = directory
        StandIn.reads_after_freeze = 0
        frozen.Driver = StandIn
        frozen.failures = frozen.checks = 0
        frozen.FROZEN_FOR_S = 0.6
        os.environ.update(FROZEN_LIB="none", FROZEN_CONFIG="none", FROZEN_DIR=directory, FROZEN_JOINTS="6")
        os.environ.pop("FROZEN_WAKE", None)
        os.environ.pop("FROZEN_OUTAGE_REPORTED", None)
        os.environ.pop("FROZEN_WRITE_BOUNDED", None)
        if outage_reported is not None:
            os.environ["FROZEN_OUTAGE_REPORTED"] = outage_reported
        if write_bounded is not None:
            os.environ["FROZEN_WRITE_BOUNDED"] = write_bounded
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            frozen.main()
        return frozen.failures, out.getvalue()


def failed_checks(text: str) -> list[str]:
    return [line.split(":", 1)[0].replace("FAIL", "").strip() for line in text.splitlines() if line.lstrip().startswith("FAIL")]


def main() -> int:
    wrong = 0

    def expect(name: str, behaviour: str, outage_reported: str | None, failing: list[str], mentions: str = "", slow_write: bool = False, write_bounded: str | None = None) -> None:
        nonlocal wrong
        count, text = run(behaviour, outage_reported, slow_write, write_bounded)
        got = failed_checks(text)
        ok = got == failing and (mentions in text)
        wrong += 0 if ok else 1
        print(f"  {'ok  ' if ok else 'FAIL'}  {name}: failed {got or 'nothing'}" + ("" if ok else f" (wanted {failing or 'nothing'}{', and the text ' + repr(mentions) if mentions else ''})"))
        if not ok:
            print(text)

    # The contract as it is asked of a plugin that is expected to say the outage.
    expect("a silent plugin fails the question the contract asks", "silent", None, ["outage reported"])
    expect("a plugin that says it and keeps saying it passes", "reports", None, [], "and it stayed said")
    expect("a plugin that says it once and goes quiet again fails", "flaps", None, ["outage reported"])
    # The same question asked the other way round for a plugin measured not to say it (F332).
    expect("a measured gap passes while the plugin stays silent", "silent", "no", [], "KNOWN GAP F332:")
    expect("a measured gap fails the day the plugin says the controller has gone", "reports", "no", ["outage reported"], "HAS CLOSED")
    # And the bound on a write, which AUBO's plugin does not keep (F333): its `write()` waits a second for the controller's reply.
    expect("a write that waits for a frozen controller fails the bound", "reports", None, ["bounded"], slow_write=True)
    expect("a measured gap passes while a write waits", "reports", None, [], "KNOWN GAP F333:", slow_write=True, write_bounded="no")
    expect("a measured gap fails the day no write waits", "reports", None, ["bounded"], "HAS CLOSED", write_bounded="no")
    print(f"\n{8 - wrong} of 8 behaved as wanted")
    return 1 if wrong else 0


if __name__ == "__main__":
    raise SystemExit(main())

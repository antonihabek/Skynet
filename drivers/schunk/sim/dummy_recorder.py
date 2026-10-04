"""SCHUNK's own protocol simulator, unmodified, with one addition: it writes down every frame it is asked to apply (DRY-COVERAGE-PLAN.md item 12.E.9).

    SCHUNK_DUMMY_LOG=/tmp/dummy-frames.jsonl python3 dummy_recorder.py --port 8000 --gripper EGU_60_EI_M_B

The maker's web server answers a write before it applies it and its access log carries no form fields, so the only way to know **what the driver put on the wire** with the maker's own server as the witness is to
look at what the maker's `Dummy.update` was handed. This wraps that one method, appends `{"t", "inst", "value"}` as a line of JSON for each call, and runs the maker's `main()` as it is. The arguments are the maker's
(`--port`, `--gripper`). Nothing of the maker's is copied here: it is imported from the package the image installed from the maker's repository at the pinned commit.
"""

from __future__ import annotations

import json
import os
import threading
import time

from schunk_gripper_dummy import dummy as maker_dummy
from schunk_gripper_dummy import main as maker_main

LOG = os.environ.get("SCHUNK_DUMMY_LOG", "")
if not LOG:
    raise SystemExit("::error::SCHUNK_DUMMY_LOG names no file. The recorder has nowhere to write, and a simulator that was asked to record and did not would let a run pass on frames nobody saw.")

_LOCK = threading.Lock()
_ORIGINAL = maker_dummy.Dummy.update


def _update(self, msg):
    with _LOCK, open(LOG, "a", encoding="utf-8") as handle:
        handle.write(json.dumps({"t": time.time(), "inst": msg["inst"], "value": msg["value"]}) + "\n")
    return _ORIGINAL(self, msg)


maker_dummy.Dummy.update = _update

if __name__ == "__main__":
    maker_main.main()

"""What the wire selftests share: the counters, the retry loop, and the guard that every LeRobot robot class has been decided about.

`selftest_wire.py` (SO-101, Feetech) and `selftest_wire_dynamixel.py` (Koch and OpenMANIPULATOR-X, Dynamixel) both report through `check`, and both hold the bridge to the
same fact about LeRobot: a robot class it ships is either in `skynet_lerobot.roles.ROLE_TABLES` or is named here, with a reason, as deliberately without one (F219). The
list lives in one place so that writing a table is one edit, here and in `roles.py`, and a class nobody decided about fails both files rather than neither.
"""

from __future__ import annotations

import re
import time
from pathlib import Path

import lerobot

from skynet_lerobot.roles import ROLE_TABLES

__all__ = ["NO_TABLE_BY_DECISION", "check", "failed", "finish", "holds_for", "roles_cover_every_lerobot_robot", "wait_until"]

_PASSED = 0
_FAILED: list[str] = []

#: LeRobot robot classes the bridge has no role table for, by decision rather than by oversight (F219's guard). A class LeRobot adds that is in
#: neither this list nor `ROLE_TABLES` fails `roles_cover_every_lerobot_robot`.
NO_TABLE_BY_DECISION = {
    "bi_so_follower": "two arms on one bus object; the motor names are prefixed by side and there is no table that is not a guess about which is which",
    "bi_openarm_follower": "bimanual, as above",
    "bi_rebot_b601_follower": "bimanual, as above",
    "openarm_follower": "no role table has been written",
    "rebot_b601_follower": "no role table has been written",
    "earthrover_mini_plus": "a wheeled base with no arm; nothing in §5.1's chain vocabulary to say",
    "unitree_g1": "driven through drivers/unitree, not through this bridge",
    "hope_jr_hand": "a dexterous hand; no finger table has been written",
}


def check(scts_id: str, what: str, held: bool, detail: str = "") -> None:
    global _PASSED
    if held:
        _PASSED += 1
        print(f"ok   {scts_id:<28} {what}")
    else:
        _FAILED.append(f"{scts_id}: {what}")
        print(f"FAIL {scts_id:<28} {what}")
        if detail:
            print(f"     {detail}")


def failed() -> list[str]:
    return list(_FAILED)


def wait_until(predicate, timeout: float = 4.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.02)
    return predicate()


def holds_for(predicate, seconds: float) -> bool:
    """Whether `predicate` is true at every look for `seconds`: for a claim about what does NOT happen, which `wait_until` cannot make — it returns at the first true look,
    and a state that has not yet had time to change is a true look (a seeded defect survived exactly that, F232)."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if not predicate():
            return False
        time.sleep(0.02)
    return predicate()


def roles_cover_every_lerobot_robot() -> None:
    """F219's guard: every robot class installed LeRobot ships either has a role table or is on the list of those that deliberately do not."""
    package = Path(lerobot.__file__).parent / "robots"
    names = {}
    for source in package.rglob("*.py"):
        for match in re.finditer(r'^\s+name = "([a-z0-9_]+)"\s*$', source.read_text(encoding="utf-8"), re.M):
            names[match.group(1)] = source.relative_to(package).as_posix()
    check("embodiment.roles.coverage", f"found the {len(names)} robot class names this LeRobot ships", len(names) >= 10, str(sorted(names)))
    undecided = sorted(name for name in names if name not in ROLE_TABLES and name not in NO_TABLE_BY_DECISION)
    check(
        "embodiment.roles.coverage",
        "every one has a role table or is listed as deliberately without one",
        not undecided,
        f"undecided: {undecided} — add a table to skynet_lerobot/roles.py, or a reason to NO_TABLE_BY_DECISION in wire_common.py",
    )
    stale = sorted(name for name in NO_TABLE_BY_DECISION if name not in names)
    check("embodiment.roles.coverage", "and the list of exceptions names no class this LeRobot does not have", not stale, str(stale))
    both = sorted(name for name in NO_TABLE_BY_DECISION if name in ROLE_TABLES)
    check("embodiment.roles.coverage", "and no class is both on that list and in the role table", not both, str(both))


def finish(footer: str) -> int:
    print(f"\n{_PASSED + len(_FAILED)} checks, {len(_FAILED)} failed")
    for failure in _FAILED:
        print(f"  FAIL {failure}")
    print("\n" + footer)
    return 1 if _FAILED else 0

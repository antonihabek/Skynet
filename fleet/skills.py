"""The skills library, as the fleet sweep reads it (DRY-COVERAGE-PLAN.md item 12.F.7).

The 24 skills live in the Hub (`skynet-skills/skills/*/manifest.json`), not in this repository, so
the sweep's `skill` stage needs a checkout of them and says so when it has none. Nothing here decides
whether a skill is good: `skynet skill check` decides what the robot's gate says, and `skynetk` with
the skill's own entry point decides what the loop does with its taught path. This module only finds
the packages and reads what those two print.

# What a row of the stage can and cannot say

Three questions are asked of every (skill, body) pair, and they are separate because they fail for
separate reasons (`docs/issues/skill-clauses-name-one-bodys-joints.md`, `gripper-has-no-open-end.md`):

| Question | Where it is answered | What a failure means |
|---|---|---|
| **binds** — does the skill's action space reach this body (§6, §37.1)? | `skill.transport.resolves` | no channel/role of the skill has an equivalent joint here |
| **names** — is every joint a clause names a joint of this body? | `skill.context.on_body` | the clause would be refused with its path named (§54.2's first rule) |
| **supplied** — does the daemon fill every path a clause reads? | `skill.context.supplied` | the skill never starts on *any* robot (today: `robot.state.gripper`) |

A skill **starts** on a body only when all three hold. When the action space binds, the skill's own
`bin/run` is also driven through `skynetk` against the body: that says what the kernel does with the
taught numbers (how many rows fired, how many were refused, how many commands the kernel had to
clamp), and it is the only place a taught angle that exceeds a body's limit shows. It does **not**
say the skill would succeed, because `success_criteria` are judged by the daemon (§54.3) over a
state this sweep does not hold, and a pose inside a body's limits is not the pose the skill meant
(the pose is a property of the cell and the body it was taught on).
"""

from __future__ import annotations

import json
import os
import re
from typing import Any

# The three checks that separate "reaches" from "starts". Spelled once here; the CLI's own ids.
CHECK_BINDS = "skill.transport.resolves"
CHECK_NAMES = "skill.context.on_body"
CHECK_SUPPLIED = "skill.context.supplied"
CHECKS = (CHECK_BINDS, CHECK_NAMES, CHECK_SUPPLIED)

# `    pass  skill.transport.resolves   the action space ...` and its siblings: pass, FAIL, miss, n/a. A verdict is indented exactly four
# spaces; the prose that explains one is indented ten, and a refusal's sentence that happens to start with `pass` is not a verdict.
CHECK_LINE = re.compile(r"^ {4}(pass|FAIL|miss|n/a)\s+([a-z_]+(?:\.[a-z_]+)+)\s", re.M)

# What the kernel host says when it cannot execute a plan the planner accepted (`skynet-kernel-host/src/plan.rs`): a loop that
# writes channel *i* to joint *i* and commands every joint on every cycle. Two sentences, two causes, both the host's own words.
HOST_CANNOT = (
    ("reorders joints", re.compile(r"a plan that reorders joints needs a channel map\s+this host does not have yet", re.I)),
    ("leaves a joint out", re.compile(r"the plan binds \d+ channel\(s\) and the body declares \d+ joint\(s\)")),
)


def host_cannot(kernel_text: str) -> str | None:
    """Why the kernel host would not execute a plan its planner accepted, in a few words — or `None` if it did not refuse for that."""
    flat = " ".join(kernel_text.split())
    for reason, pattern in HOST_CANNOT:
        if pattern.search(flat):
            return reason
    return None


# What `skynetp run` prints at the end of a trajectory: `... — 6 commanded, 0 refused ...`.
COMMANDED = re.compile(r"—\s*(\d+) commanded")
REFUSED = re.compile(r",\s*(\d+) refused")


def hub_names(repo: str) -> list[str]:
    """The directory names the Hub has been cloned under — read from `tools/hub-path.mjs`, not retyped.

    §78: one fact, one list. A second copy of this array would agree with the first until a clone was
    renamed, and then a sweep would report "no skills checkout" beside a checkout that is there.
    """
    with open(os.path.join(repo, "tools", "hub-path.mjs"), encoding="utf-8") as handle:
        text = handle.read()
    block = re.search(r"HUB_DIRECTORY_NAMES\s*=\s*\[(.*?)\]", text, re.S)
    if block is None:
        raise RuntimeError("tools/hub-path.mjs no longer lists HUB_DIRECTORY_NAMES")
    names = re.findall(r"'([^']+)'", block.group(1))
    if not names:
        raise RuntimeError("tools/hub-path.mjs lists no Hub directory names")
    return names


def find_skills(repo: str, explicit: str = "") -> str | None:
    """The directory holding the skill packages, or `None` — never a guess.

    `explicit` (the `--skills` flag) or `SKYNET_SKILLS` win; otherwise the Hub checkout beside this
    one. `None` is a real answer: a vendor who cloned only this repository has no skills library, and
    the stage says `not_run` with that reason rather than passing on nothing.
    """
    for given in (explicit, os.environ.get("SKYNET_SKILLS", "")):
        if given:
            return given if os.path.isdir(given) else None
    parent = os.path.dirname(os.path.abspath(repo))
    for name in hub_names(repo):
        candidate = os.path.join(parent, name, "skynet-skills", "skills")
        if os.path.isdir(candidate):
            return candidate
    return None


def packages(skills_dir: str) -> list[dict[str, Any]]:
    """Every package under `skills_dir`: its directory, name and manifest, sorted by name."""
    found = []
    for name in sorted(os.listdir(skills_dir)):
        directory = os.path.join(skills_dir, name)
        path = os.path.join(directory, "manifest.json")
        if os.path.isfile(path):
            with open(path, encoding="utf-8") as handle:
                found.append({"name": name, "dir": directory, "manifest": json.load(handle)})
    return found


def parse_checks(text: str) -> dict[str, str]:
    """`skynet skill check`'s report as `{check id: pass|FAIL|miss|n/a}`."""
    return {check: outcome for outcome, check in CHECK_LINE.findall(text)}


def argument(args: list[str], flag: str) -> str | None:
    """The value after `flag` in a manifest's `runtime.args`, or `None`."""
    return args[args.index(flag) + 1] if flag in args and args.index(flag) + 1 < len(args) else None


def trajectory(manifest: dict[str, Any]) -> dict[str, Any] | None:
    """What a commanding skill's entry point will do: its rate, cycle bound, mode and number of taught rows.

    `None` for a package that commands nothing (no `source_embodiment`, no `--setpoints`), such as
    `shift-readiness`, which is a dwell: it has nothing to drive and the stage says so.
    """
    if manifest.get("source_embodiment") is None:
        return None
    args = [str(a) for a in (manifest.get("runtime") or {}).get("args") or []]
    setpoints = argument(args, "--setpoints")
    if setpoints is None:
        return None
    rows = [row for row in setpoints.split(";") if row.strip()]
    hz, cycles = argument(args, "--hz"), argument(args, "--cycles")
    if hz is None or cycles is None:
        return None
    return {
        "args": args,
        "hz": int(float(hz)),
        "cycles": int(cycles),
        "mode": argument(args, "--mode") or "position",
        "channels": int(argument(args, "--channels") or 0),
        "rows": len(rows),
    }


def skill_summary(package: dict[str, Any]) -> dict[str, Any]:
    """The one-line identity of a package, for the report's `skills` block."""
    manifest = package["manifest"]
    shape = trajectory(manifest)
    return {
        "name": package["name"],
        "id": manifest.get("id"),
        "version": manifest.get("version"),
        "task": manifest.get("task"),
        "commands": shape is not None,
        "rows": shape["rows"] if shape else 0,
        "seconds": manifest.get("expected_duration_seconds"),
        "source": (manifest.get("source_embodiment") or {}).get("robot_slug"),
        "verified_on": (manifest.get("embodiment") or {}).get("verified_on") or [],
    }


def hand_for(manifest: dict[str, Any], descriptor: dict[str, Any]) -> dict[str, Any] | None:
    """The joint of this body that the skill's `gripper` channel means — the integrator's assertion, or `None`.

    A MuJoCo-served body declares no end effector (the driver will not guess a hand from a joint's name),
    and §6 routes a `gripper` channel to a declared effector, so on such a body the 21 skills that close a
    hand are refused whatever else is true of them. The sweep is the integrator here, as it is when it
    accepts a role table (§63.4), and it declares a hand on two grounds, in this order, recorded with the
    row so neither passes for something the scene said:

    1. **By role.** A joint carries the very role the channel names (`right_hand.gripper.grip`).
    2. **By the skill's own word.** The channel names its joint (`"joint": "gripper"`) and the body has a joint of
       exactly that name — the rule the planner applies to every other channel (an exact name binds before a
       role). It is the skill saying which joint its hand is, not the sweep reading a hand off a name.

    One joint: declaring a body's *other* hand would route a right-arm skill's gripper channel to the left
    hand, because the planner takes the first effector of the kind.
    """
    channels = (manifest.get("source_embodiment") or {}).get("action_channels") or []
    grippers = [channel for channel in channels if channel.get("kind") == "gripper"]
    joints = descriptor.get("joints") or []
    for channel in grippers:
        for joint in joints:
            if channel.get("role") and joint.get("role") == channel["role"]:
                return {"joint": joint["name"], "kind": "gripper", "role": channel["role"], "by": "role"}
    for channel in grippers:
        for joint in joints:
            if channel.get("joint") and joint["name"] == channel["joint"]:
                return {"joint": joint["name"], "kind": "gripper", "role": joint.get("role"), "by": "name"}
    return None


def with_hand(descriptor: dict[str, Any], hand: dict[str, Any]) -> dict[str, Any]:
    """A copy of `descriptor` that declares `hand` as its end effector, as the driver would serve it given the same word."""
    declared = dict(descriptor)
    declared["end_effectors"] = [{"name": hand["joint"], "role": hand["role"], "kind": hand["kind"], "dof": 1.0}]
    return declared


def taught_ranges(setpoints: str) -> list[tuple[float, float]]:
    """The lowest and highest value each channel is ever asked for in a `--setpoints "tick:v0,v1,..;tick:..."` table."""
    low: list[float] = []
    high: list[float] = []
    for row in (r for r in setpoints.split(";") if r.strip()):
        _, _, values = row.partition(":")
        for index, text in enumerate(values.split(",")):
            value = float(text)
            if index == len(low):
                low.append(value)
                high.append(value)
            else:
                low[index] = min(low[index], value)
                high[index] = max(high[index], value)
    return list(zip(low, high))


def past_travel(plan: dict[str, Any], ranges: list[tuple[float, float]], tolerance: float = 1e-9) -> list[str]:
    """Where the taught path asks a joint for more than the body's travel allows — the sentences, one per joint, or `[]`.

    `plan` is `skynet transport --json`'s output: each binding's `source_index` is the taught table's column,
    its `target` the joint the planner bound it to, and `clamp.travel` both ends of that joint's position
    travel as the kernel will hold them (§92). A channel with no travel (a gripper effector, a velocity
    joint) is not compared: nothing in the plan says where its ends are. This is what the loop would clamp,
    found without running the loop, so it holds for the bodies the host will not run a plan on (§93.2).
    """
    travel = (plan.get("clamp") or {}).get("travel") or {}
    found: list[str] = []
    for binding in plan.get("bindings") or []:
        target, index = binding.get("target"), binding.get("source_index")
        if target is None or target not in travel or not isinstance(index, int) or index >= len(ranges):
            continue
        low_end, high_end = travel[target]
        low, high = ranges[index]
        if high > high_end + tolerance:
            found.append(f"{target}: taught {high:.3g} past {high_end:.3g}")
        elif low < low_end - tolerance:
            found.append(f"{target}: taught {low:.3g} past {low_end:.3g}")
    return found


def verdict(checks: dict[str, str]) -> dict[str, bool]:
    """Binds / names / supplied as booleans. A check the report did not carry is *not* a pass."""
    return {key: checks.get(check) == "pass" for key, check in (("binds", CHECK_BINDS), ("names", CHECK_NAMES), ("supplied", CHECK_SUPPLIED))}


def first_refusal(checks: dict[str, str]) -> str | None:
    """The first of the three checks that did not pass, in the order they are asked, or `None`."""
    for check in CHECKS:
        if checks.get(check) != "pass":
            return check
    return None

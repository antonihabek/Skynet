#!/usr/bin/env python3
"""Check the corpus runner's own accounting.

    python editions/python/self_check.py

The runner is the only thing that says how far this edition has got, and after §31.3's per-file
``Unimplemented`` signal it has four buckets rather than two. A miscount there is worse than a
failing case: a failing case is loud, and a number that quietly reads 100 untranscribed cases as 100
wrong answers — or, in the other direction, 100 wrong answers as untranscribed ones — is a green run
that means nothing.

So this runs the runner against **synthetic** corpora with known answers, in a temporary directory,
with the dispatch table swapped out. Four of the fifteen checks test the *detector* rather than the
runner's happy path:

* that a corpus with a declined file is refused the name ``complete`` **even though its other file
  passes** — the property §31.3's next step depends on, since ``fuzz`` is one corpus over three
  files and only one of them is being written;
* that a handler declining *some* of a file's cases is reported as **failures**, because declining
  is a property of a file and a handler doing it case by case is a defect rather than a gap;
* that only the declined case of such a file fails. Writing this check the other way round is what
  found the rule: a sibling case that produced the right bytes **did** produce them, and failing it
  to punish the handler would be a false claim in the opposite direction. Nothing can be hidden this
  way either, because the declined case is already a failure and the run is red regardless;
* that the guard is what does the excluding — the same corpus is asserted to become complete once
  the declining file is removed, so an assertion passing for some other reason would show up.
"""

from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))

from skynet_os import corpus as corpus_module  # noqa: E402
from skynet_os.canonical import canonical  # noqa: E402
from skynet_os.corpus import Unimplemented, run_corpus  # noqa: E402

FAILURES: list[str] = []


def check(name: str, condition: bool, detail: str = "") -> None:
    suffix = f" — {detail}" if detail else ""
    if condition:
        print(f"  ok    {name}")
        return
    FAILURES.append(f"{name}{suffix}")
    print(f"  FAIL  {name}{suffix}")


def write_corpus(root: Path, kind: str, label: str, cases: list[dict[str, Any]]) -> None:
    directory = root / kind
    directory.mkdir(parents=True, exist_ok=True)
    (directory / f"{label}.json").write_text(
        json.dumps({"corpus": kind, "cases": cases}, indent=2), encoding="utf-8"
    )


def case(case_id: str, value: Any, expected: Any = ...) -> dict[str, Any]:
    """A case whose ``expected`` is the canonical form of ``value`` unless told otherwise."""
    return {
        "id": case_id,
        "note": "",
        "input": {"value": value},
        "expected": canonical(value if expected is ... else expected),
    }


def echo(_label: str, case_input: dict[str, Any]) -> Any:
    return case_input["value"]


def decline_whole_file(label: str, case_input: dict[str, Any]) -> Any:
    if label == "written":
        return case_input["value"]
    raise Unimplemented("spanning", label)


def decline_some_cases(_label: str, case_input: dict[str, Any]) -> Any:
    if case_input["value"] == "decline-me":
        raise Unimplemented("inconsistent", "mixed")
    return case_input["value"]


def explode(_label: str, _case_input: dict[str, Any]) -> Any:
    raise ValueError("the driver is on fire")


def build(root: Path) -> None:
    # A corpus with no handler at all — three cases.
    write_corpus(root, "unhandled", "only", [case("a", 1), case("b", 2), case("c", 3)])

    # A corpus that passes outright — two cases.
    write_corpus(root, "answered", "only", [case("a", "x"), case("b", "y")])

    # A corpus over two files where one is written and one is declined whole. This is `fuzz`.
    write_corpus(root, "spanning", "written", [case("a", 10), case("b", 11)])
    write_corpus(root, "spanning", "pending", [case("a", 20), case("b", 21), case("c", 22)])

    # A handler that declines only part of a file.
    write_corpus(
        root,
        "inconsistent",
        "mixed",
        [case("a", "fine"), case("b", "decline-me"), case("c", "also-fine")],
    )

    # A handler that raises, and a case that simply answers wrongly.
    write_corpus(root, "raising", "only", [case("a", "anything")])
    write_corpus(root, "wrong", "only", [case("a", "produced", expected="something-else")])


def main() -> int:
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        build(root)

        handlers = {
            "answered": echo,
            "spanning": decline_whole_file,
            "inconsistent": decline_some_cases,
            "raising": explode,
            "wrong": echo,
        }
        original = corpus_module.HANDLERS
        corpus_module.HANDLERS = handlers
        try:
            report = run_corpus(root)

            print("report    " + report.summary())
            print()

            # -- the four buckets ---------------------------------------------------------------
            check(
                "a corpus with no handler is counted, not skipped",
                report.no_handler_cases == 3,
                f"no_handler_cases={report.no_handler_cases}, want 3",
            )
            check(
                "a corpus with no handler is named",
                report.missing_handlers == ["unhandled"],
                f"missing_handlers={report.missing_handlers}",
            )
            check(
                "a wholly declined file lands in the same bucket, kept separate",
                report.declined_cases == 3,
                f"declined_cases={report.declined_cases}, want 3",
            )
            check(
                "the declined file is named by corpus and label",
                report.declined_files == ["spanning/pending"],
                f"declined_files={report.declined_files}",
            )

            # -- the detector: a partial corpus is not a complete one ---------------------------
            check(
                "a corpus holding a declined file is refused `complete`",
                "spanning" not in report.complete,
                f"complete={report.complete}",
            )
            check(
                "...and is named `partial` instead, so it is not silently absent",
                report.partial == ["spanning"],
                f"partial={report.partial}",
            )
            check(
                "a corpus whose every case passed is `complete`",
                "answered" in report.complete,
                f"complete={report.complete}",
            )

            # -- the detector: declining case by case is a defect, not a gap --------------------
            inconsistent_failures = [f for f in report.failures if f.corpus == "inconsistent"]
            check(
                "a handler declining part of a file reports failures, not `unimplemented`",
                report.declined_cases == 3 and len(inconsistent_failures) > 0,
                f"declined_cases={report.declined_cases}, "
                f"inconsistent failures={len(inconsistent_failures)}",
            )
            check(
                "...and only the declined case fails: its siblings answered correctly and are "
                "counted as passes, because failing them would invent a claim the other way",
                len(inconsistent_failures) == 1,
                f"got {len(inconsistent_failures)}, want 1",
            )
            check(
                "...and the reason says why rather than repeating an exception",
                inconsistent_failures[0].reason is not None
                and "property of a file" in inconsistent_failures[0].reason,
                f"reason={inconsistent_failures[0].reason!r}",
            )
            check(
                "...and the corpus is not complete either",
                "inconsistent" not in report.complete,
                f"complete={report.complete}",
            )

            # -- ordinary failures still behave -------------------------------------------------
            raising = next(f for f in report.failures if f.corpus == "raising")
            check(
                "a raising handler is a failing case, named by exception type",
                raising.reason == "ValueError: the driver is on fire",
                f"reason={raising.reason!r}",
            )
            wrong = next(f for f in report.failures if f.corpus == "wrong")
            check(
                "a wrong answer reports where it diverged rather than a reason",
                wrong.reason is None and wrong.diverged_at is not None,
                f"reason={wrong.reason!r} diverged_at={wrong.diverged_at}",
            )

            # -- the numbers add up to what is on disk ------------------------------------------
            on_disk = sum(
                len(json.loads(path.read_text(encoding="utf-8"))["cases"])
                for path in sorted(root.glob("*/*.json"))
            )
            check(
                "passed + failed + not-implemented is every case on disk",
                report.total == on_disk,
                f"total={report.total}, on disk={on_disk}",
            )

            # -- the guard is what excludes it --------------------------------------------------
            # Same corpus, same handler, with the declining file deleted. If `spanning` were still
            # refused `complete` here, the assertion above was passing for some other reason.
            (root / "spanning" / "pending.json").unlink()
            second = run_corpus(root)
            check(
                "removing the declined file makes the same corpus complete — so the exclusion "
                "above was the guard, not an accident",
                "spanning" in second.complete and second.partial == [],
                f"complete={second.complete}, partial={second.partial}",
            )
        finally:
            corpus_module.HANDLERS = original

    divergences()
    transport_divergences()
    admission_divergences()

    print()
    total = 15 + 8 + 4 + 2
    if FAILURES:
        print(f"self-check FAILED — {len(FAILURES)} of {total}")
        return 1
    print(f"self-check ok — {total} checks")
    return 0


# -- The traps the corpus cannot reach ----------------------------------------
#
# `safety.py` carries guards against JavaScript-versus-Python divergences, and a mutation run proved
# the corpus exercises none of them: deleting the `bool` guard changed no case's answer. That is not
# a gap in the corpus. Every edition must pass the corpus, and Rust cannot represent these inputs at
# all -- `joint_torque_max_nm` is an `Option<f64>` and serde refuses `true` before any rule runs. The
# traps are properties of a *dynamically typed transcription*, so they are checked here, against
# expectations read from `reference/typescript/safety.ts` rather than from `safety.py`.

ENVELOPE = {
    "joint_velocity_max_rad_s": 2,
    "joint_torque_max_nm": 80,
    "base_velocity_max_m_s": 1.5,
    "watchdog_ms": 100,
    "estop": True,
    "source": "https://example.invalid/datasheet.pdf",
}

CLAMP = {
    "joint_velocity_max_rad_s": 2,
    "joint_torque_max_nm": 80,
    "base_velocity_max_m_s": 1.5,
    "min_control_hz": 10,
    "per_channel": {"elbow": 80, "shoulder": 60},
    "source": "https://example.invalid/datasheet.pdf",
}

AVAILABLE = {"imu": True, "people_detection": True, "legged": True}

TWO_BODIES = {
    "root": "base_link",
    "convention": "rep103",
    "generator": "vendor",
    "nodes": [
        {"parent": None, "kind": "root", "name": "base_link"},
        {
            "parent": "base_link", "kind": "static", "name": "left",
            "translation": [0, 0.5, 0], "rotation": [0, 0, 0, 1], "source": "descriptor",
            "collision": True,
            "collision_shape": {
                "kind": "capsule", "a": [0, 0, 0], "b": [0, 0, 0.2], "radius_m": 0.05,
            },
        },
        {
            "parent": "base_link", "kind": "static", "name": "right",
            "translation": [0, -0.5, 0], "rotation": [0, 0, 0, 1], "source": "descriptor",
            "collision": True,
            "collision_shape": {
                "kind": "capsule", "a": [0, 0, 0], "b": [0, 0, 0.2], "radius_m": 0.05,
            },
        },
    ],
}


def host_params(**overrides: Any) -> dict[str, Any]:
    params: dict[str, Any] = {
        "tier": "host",
        "envelope": dict(ENVELOPE),
        "transport": dict(CLAMP),
        "profile": {"name": "cell-a", "max_delta_per_cycle": 0.25},
        "available": dict(AVAILABLE),
    }
    params.update(overrides)
    return params


def codes(limits: dict[str, Any]) -> list[str]:
    return [notice["code"] for notice in limits["notices"]]


def divergences() -> None:
    from skynet_os import safety

    print()
    print("the traps the corpus cannot reach")

    # 1. `typeof true === 'number'` is false; `isinstance(True, int)` is true. The reference
    #    discards the boolean and takes 80 from the transport clamp. An edition that let it through
    #    would clamp every joint to 1 Nm and report it as an operator's own setting.
    limits = safety.resolve_limits(
        host_params(envelope={**ENVELOPE, "joint_torque_max_nm": True})
    )
    check(
        "a boolean limit is not a number: the reference reads 80 from the transport clamp",
        limits["joint_torque_max_nm"] == 80 and limits["joint_torque_max_nm"] is not True,
        f"joint_torque_max_nm={limits['joint_torque_max_nm']!r}",
    )

    # 2, 3 and 4. `self_collision_margin_m === undefined` is strict. Present-and-null takes the
    #    other branch: no notice, and `?? 0` still resolves the clearance to zero.
    frames = safety._flatten_or_throw(TWO_BODIES)
    with_frames = safety.resolve_limits({**host_params(), "frames": frames})
    check(
        "an absent self-collision margin emits the notice, once the guard is actually active",
        "self_collision" in with_frames["active_guards"]
        and "no_collision_margin" in codes(with_frames),
        f"active={with_frames['active_guards']}, notices={codes(with_frames)}",
    )
    null_margin = safety.resolve_limits(
        {
            **host_params(
                profile={
                    "name": "cell-a",
                    "max_delta_per_cycle": 0.25,
                    "self_collision_margin_m": None,
                }
            ),
            "frames": frames,
        }
    )
    check(
        "a margin present and null emits no notice -- `=== undefined` is strict, and `.get` is not",
        "self_collision" in null_margin["active_guards"]
        and "no_collision_margin" not in codes(null_margin),
        f"notices={codes(null_margin)}",
    )
    check(
        "...and still resolves to a null margin rather than to a missing key",
        null_margin["self_collision_margin_m"] is None,
        f"self_collision_margin_m={null_margin['self_collision_margin_m']!r}",
    )

    # 5 and 6. `cycle.capability !== undefined`. Absent means the runtime did not say and the gate
    #    does not evaluate; present-and-null means it said null, which is not a granted token.
    granted = safety.with_grants(safety.resolve_limits(host_params()), ["motion.arm"])
    cycle = {"now_ms": 1000, "command_ms": 995, "sequence": 42, "desired": {"elbow": 10}}
    absent_cap = safety.evaluate_cycle(cycle, granted, ENVELOPE, 41)
    check(
        "an absent capability does not evaluate the mode gate",
        "mode_gate" not in absent_cap["tripped"],
        f"tripped={absent_cap['tripped']}",
    )
    null_cap = safety.evaluate_cycle({**cycle, "capability": None}, granted, ENVELOPE, 41)
    check(
        "a capability present and null trips it -- null is not a granted token",
        "mode_gate" in null_cap["tripped"] and null_cap["response"] == "hold",
        f"tripped={null_cap['tripped']}, response={null_cap['response']}",
    )

    # 7 and 8. `Object.entries` emits array-index keys first, ascending, then the rest in insertion
    #    order. Python preserves insertion order throughout, so the `clamped` array would come out
    #    in a different order for a body whose channels are numbered.
    numbered = safety.with_grants(
        safety.resolve_limits(
            host_params(transport={**CLAMP, "per_channel": {"10": 1, "2": 1, "base": 1}})
        ),
        [],
    )
    verdict = safety.evaluate_cycle(
        {
            "now_ms": 1000, "command_ms": 995, "sequence": 42,
            "desired": {"10": 50, "base": 50, "2": 50},
        },
        numbered,
        ENVELOPE,
        41,
    )
    order = [entry["channel"] for entry in verdict["clamped"]]
    check(
        "numeric channel names clamp in JavaScript's order, not the order they were written",
        order == ["2", "10", "base"],
        f"clamped order={order}, want ['2', '10', 'base']",
    )
    leading_zero = safety.evaluate_cycle(
        {
            "now_ms": 1000, "command_ms": 995, "sequence": 42,
            "desired": {"09": 50, "2": 50},
        },
        safety.with_grants(
            safety.resolve_limits(
                host_params(transport={**CLAMP, "per_channel": {"09": 1, "2": 1}})
            ),
            [],
        ),
        ENVELOPE,
        41,
    )
    zero_order = [entry["channel"] for entry in leading_zero["clamped"]]
    check(
        "...and a channel named `09` is not an array index, so it keeps its place",
        zero_order == ["2", "09"],
        f"clamped order={zero_order}, want ['2', '09']",
    )


# -- The transport traps, for the same reason -----------------------------------------------------
#
# Two more mutations of `transport.py` survive the corpus, and both survive it correctly: Rust's
# `allowRetarget` is an `Option<bool>` and cannot hold a truthy non-boolean at all, and serde reads
# an absent field and an explicit null into the same `None`. Neither distinction is expressible in
# every edition, so neither belongs in a corpus every edition must pass.

ARM_JOINTS = [
    {
        "name": "J1", "role": "right_arm.shoulder.pitch", "type": "revolute",
        "limits": {
            "position_min_rad": -3, "position_max_rad": 3,
            "velocity_max_rad_s": 2, "effort_max_nm": 80,
        },
        "control_modes": ["torque", "position"],
    },
    {
        "name": "J2", "role": "right_arm.elbow.flex", "type": "revolute",
        # No `effort_max_nm`: this joint has no rating for a torque channel, which is what makes
        # the `unscalable_channel` branch reachable.
        "limits": {"position_min_rad": -3, "position_max_rad": 3, "velocity_max_rad_s": 2},
        "control_modes": ["torque", "position"],
    },
]


def arm(**overrides: Any) -> dict[str, Any]:
    body: dict[str, Any] = {
        "schema_version": 1,
        "robot_slug": "test-arm",
        "digest": "sha256:arm",
        "verified": True,
        "base": "fixed",
        "dof": 3,
        "joints": ARM_JOINTS,
        "end_effectors": [],
        "sensors": [],
        "control": {
            "modes": ["position", "torque"], "rate_hz_max": 200,
            "rate_hz_min": 10, "command_latency_ms": 4,
        },
        "envelope": {
            "joint_velocity_max_rad_s": 2, "joint_torque_max_nm": 80,
            "watchdog_ms": 100, "estop": True, "source": "https://example.invalid/arm.pdf",
        },
        "solvers": ["trac_ik"],
    }
    body.update(overrides)
    return body


def trainer(channels: list[dict[str, Any]], **overrides: Any) -> dict[str, Any]:
    source: dict[str, Any] = {
        "robot_slug": "trainer",
        "digest": "sha256:policy",
        "dof": 3,
        "control_mode": "torque",
        "control_hz": 50,
        "action_channels": channels,
        "observations": [],
    }
    source.update(overrides)
    return source


def transport_divergences() -> None:
    from skynet_os import transport

    print()
    print("the transport traps the corpus cannot reach")

    # 1 and 2. `options.allowRetarget === true` is strict. A truthy non-boolean does *not* grant
    #    retargeting, and — this is the part worth pinning — the message it produces then blames a
    #    missing solver on a body that has one, because the message branch is a truthy test while
    #    the grant is an identity test. Transcribed as it stands; it is the reference's sentence.
    #    A chain that exists with a joint that does not is what needs task space at all.
    needs_task_space = trainer(
        [{"index": 0, "kind": "joint_torque", "role": "right_arm.wrist.roll"}]
    )
    loose = transport.resolve_transport(needs_task_space, arm(), {"allowRetarget": 1})
    check(
        "a truthy-but-not-true allowRetarget does not grant retargeting",
        loose["verdict"] == "refused"
        and all(b["method"] != "retarget" for b in loose["bindings"]),
        f"verdict={loose['verdict']}, methods={[b['method'] for b in loose['bindings']]}",
    )
    check(
        "...and still takes the message branch that blames a solver this body has",
        any(
            w["code"] == "retarget_without_solver" and "no inverse-kinematics solver" in w["message"]
            for w in loose["warnings"]
        ),
        f"warnings={[w['code'] for w in loose['warnings']]}",
    )

    # 3 and 4. `channel.peak !== undefined` decides whether an unscalable channel is warned about.
    #    J2 declares no torque rating, so `scaleFor` returns 1 either way and the *only* observable
    #    difference is the warning — which is the whole point of it.
    absent_peak = transport.resolve_transport(
        trainer([{"index": 0, "kind": "joint_torque", "role": "right_arm.elbow.flex"}]), arm(), {}
    )
    check(
        "a channel with no declared peak raises no unscalable warning",
        not any(w["code"] == "unscalable_channel" for w in absent_peak["warnings"]),
        f"warnings={[w['code'] for w in absent_peak['warnings']]}",
    )
    null_peak = transport.resolve_transport(
        trainer(
            [{"index": 0, "kind": "joint_torque", "role": "right_arm.elbow.flex", "peak": None}]
        ),
        arm(),
        {},
    )
    check(
        "a peak present and null does raise it -- `!== undefined` is strict, and `.get` is not",
        any(w["code"] == "unscalable_channel" for w in null_peak["warnings"]),
        f"warnings={[w['code'] for w in null_peak['warnings']]}",
    )


# -- The admission trap ---------------------------------------------------------------------------
#
# `capacity.simulator !== true` is strict, and Rust's field is an `Option<bool>` which cannot hold a
# truthy non-boolean at all -- so the corpus cannot reach the difference between a strict test and a
# truthy one. §55.1's whole point is that an environment package installed against a robot with no
# simulator driver is inert, and the warning is the only thing that says so.

MANIFEST = {
    "schema_version": 1,
    "id": "acme/world",
    "version": "1.0.0",
    "kind": "environment",
    "digest": "sha256:pkg",
    "isolation": "sandboxed",
    "budget": {"ram_mb": 512, "vram_mb": 2048, "cpu_cores": 2, "disk_mb": 4096},
    "embodiment": {"capabilities": [], "control_mode": "torque"},
    "requests": [],
}

CAPACITY = {
    "ram_mb": 8192,
    "vram_mb": 8192,
    "cpu_cores": 6,
    "disk_mb": 65536,
    "os_version": "1.0.0",
    "capabilities": [],
    "granted": [],
    "installed": [],
}


def admit(**capacity_overrides: Any) -> dict[str, Any]:
    from skynet_os import admission

    capacity = dict(CAPACITY)
    capacity.update(capacity_overrides)
    return admission.admit_package(
        {
            "manifest": MANIFEST,
            "capacity": capacity,
            "signature": "verified",
            "observed_digest": "sha256:pkg",
        }
    )


def admission_divergences() -> None:
    print()
    print("the admission trap the corpus cannot reach")

    truthy = admit(simulator=1)
    check(
        "a truthy-but-not-true `simulator` is not a simulator: the environment is still inert",
        any(f["code"] == "environment_inert" for f in truthy["findings"]),
        f"findings={[f['code'] for f in truthy['findings']]}",
    )
    real = admit(simulator=True)
    check(
        "...and a declared simulator raises no such warning",
        not any(f["code"] == "environment_inert" for f in real["findings"]),
        f"findings={[f['code'] for f in real['findings']]}",
    )


if __name__ == "__main__":
    raise SystemExit(main())

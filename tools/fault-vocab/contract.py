#!/usr/bin/env python3
"""The fault-vocabulary contract: what a driver must make of the codes its robot's maker defined.

DRY-COVERAGE-PLAN.md item 12.C.6, the dry proxy for layer 9. Nothing here says a driver understands a robot's faults: layer 9 is the
robot's own, and no dry test reaches it. What this holds is narrower and still the thing a driver gets wrong in practice — the
**translation**. A maker's controller reports a state or an alarm as a number; §24 gives the driver three flags and one word to say it
in; and the OS acts on the flags (§27 leaves `present` only on `RHAL_DEGRADED`, and `stop` only on `RHAL_ESTOP`) and on nothing else,
because §24 makes `fault_code` "vendor-specific, surfaced verbatim to the black box" — the OS is told never to interpret one. So a
driver that reads *protective stop* and reports "degraded" has told the OS the robot is merely impaired, and a driver that reads a code
it has never heard of and reports nothing has told it the robot is well. Both are the worst class of bug a robot has: they look like
success.

`drivers/fault-vocabulary.json` states, per driver and per vocabulary the driver reads, what each code the maker documents is *for the
robot*, in four classes, and what to do with a code that is in no table:

  ok        healthy. No flag; the fault word is zero; commands are accepted.
  degraded  running with something wrong (a reduced mode). DEGRADED, not FAULT or ESTOP; commands are accepted.
  fault     the robot is not doing what it is told (§24: "the drive has faulted; commands are being ignored"). FAULT and DEGRADED —
            DEGRADED because it is the only one of the two §27 reads — and not ESTOP; and **commands are refused**, because a driver
            that answers a command the controller is ignoring with `SKR_OK` has reported success it did not have.
  estop     the maker's stop chain is open. ESTOP and DEGRADED; commands are refused.

and, for any code a vocabulary does not list, the class it **escalates** to (`unmapped`, `fault` or `estop`, with the reason): an unknown
state of a safety system is read as the most restrictive thing the driver can honestly say, and never as healthy. A driver is held to it
by `probes`, codes chosen outside every table.

A body that is moved by **motions** and not by joint commands (a vehicle: MAVLink's `land`, `rtl`, `hold`) cannot be held to "a faulted body
refuses commands" without taking away the commands a person most wants in a failsafe. A vocabulary of such a body says what stays
available (`keeps`, with `keeps_because`): in `fault` those motions are **still answered** while the one that takes the body somewhere new
(`write`) is refused — a driver that refuses `land` because the vehicle has reported a failsafe has made the failsafe worse — and in `estop`
they are refused with the rest, because the stop chain is open and nothing moves the body.

The fault word is held too: `verbatim` means it is the injected code (the black box is where a person looks the code up, so a node number
or a count in its place is a fault whose cause was thrown away), and a driver whose word is derived says how in the subject.

A driver supplies a **subject** (`drivers/<name>/faultvocab.py`): a class with

    name                                    for the report
    settle_s                                how long the substrate takes to show a changed code (a stream's period, plus margin)
    start() / stop()                        the substrate and the driver, opened and healthy
    can_inject(vocabulary, code) -> str|None   None if the substrate can emit it, else the reason it cannot (reported, never skipped silently)
    inject(vocabulary, code) / clear(vocabulary)
    read() -> (rc, flags, fault_code)       one `read_state`
    write() -> rc                           one valid command: the kind that takes the body somewhere new
    motion(name) -> rc                      only if the vocabulary has `keeps`: one of the named motions, as a person would ask for it
    restore()                               only with `motion`: put the body back where it was, for the next check
    restart()                               a fresh session, for the substrate that holds a latch a person clears
    expected_code(vocabulary, code) -> int  only if `fault_code` is `derived`

    PYTHONPATH=bindings/python:drivers/ur_rtde python3 tools/fault-vocab/contract.py drivers/ur_rtde/faultvocab.py
    python3 tools/fault-vocab/contract.py drivers/ur_rtde/faultvocab.py --only safety_status
    python3 tools/fault-vocab/contract.py --list

`selftest.py` shows the contract against drivers written to be wrong in each way it names.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(REPO, "bindings", "python"))

import skynet_rhal as rhal  # noqa: E402

RC_OK = rhal.SKR_OK
ESTOP, FAULT, DEGRADED = rhal.FLAG_ESTOP, rhal.FLAG_FAULT, rhal.FLAG_DEGRADED
VOCABULARY_FILE = os.path.join(REPO, "drivers", "fault-vocabulary.json")


class Rule:
    """What a class asks of a flags word and of a command."""

    def __init__(self, required: int, forbidden: int, commands: str, why: str) -> None:
        self.required, self.forbidden, self.commands, self.why = required, forbidden, commands, why

    def holds(self, flags: int) -> bool:
        return flags & self.required == self.required and not flags & self.forbidden


#: The four classes. `fault` and `estop` both require DEGRADED: §27's `rhal` condition reads that bit and not FAULT
#: (`skynetd-io/src/blackbox.rs`, `RhalSighting::of`), so a faulted drive that sets only FAULT leaves the operating state at `present`.
CLASSES = {
    "ok": Rule(0, ESTOP | FAULT | DEGRADED, "accepted", "healthy: no flag, a zero fault word, commands taken"),
    "degraded": Rule(DEGRADED, ESTOP | FAULT, "accepted", "running with something wrong: DEGRADED alone, commands taken"),
    "fault": Rule(FAULT | DEGRADED, ESTOP, "refused", "not doing what it is told: FAULT and DEGRADED, commands refused"),
    "estop": Rule(ESTOP | DEGRADED, 0, "refused", "the maker's stop chain is open: ESTOP and DEGRADED, commands refused"),
}
#: What an unlisted code may be treated as. For a state or an alarm — a channel whose codes say what the robot is doing — `ok` and `degraded` are
#: not on the list, and that is the contract. For a **warning** channel (`"kind": "warning"`: codes the maker documents as advisory, which the
#: robot runs on through) the floor is `degraded`: an unlisted warning is never read as healthy, and is not read as a stop either.
ESCALATIONS = ("fault", "estop")
WARNING_ESCALATIONS = ("degraded", "fault", "estop")


def escalations(spec: dict) -> tuple[str, ...]:
    return WARNING_ESCALATIONS if spec.get("kind") == "warning" else ESCALATIONS


def flag_names(flags: int) -> str:
    names = [name for bit, name in ((ESTOP, "ESTOP"), (FAULT, "FAULT"), (DEGRADED, "DEGRADED")) if flags & bit]
    return "+".join(names) if names else "no flag"


def load_spec(path: str = VOCABULARY_FILE) -> dict:
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


class Result:
    def __init__(self) -> None:
        self.count = 0
        self.failed = 0
        self.injected = 0
        self.not_injectable: list[str] = []
        self.failures: list[str] = []

    def check(self, condition: bool, what: str) -> bool:
        self.count += 1
        print(f"  {'ok  ' if condition else 'FAIL'}  {what}", flush=True)
        if not condition:
            self.failed += 1
            self.failures.append(what)
        return bool(condition)


def observe(subject, rule: Rule, wait_s: float) -> tuple[int, int, int, bool]:
    """Read until a read satisfies `rule` or the substrate has had `wait_s` to show it; then insist it holds three reads running.

    Returns the last read and whether it held. A transition is allowed time; a driver that flaps between healthy and faulted once the
    substrate has settled is not allowed to pass on the read that happened to agree.
    """
    end = time.monotonic() + wait_s
    rc, flags, code = subject.read()
    while not (rc == RC_OK and rule.holds(flags)) and time.monotonic() < end:
        time.sleep(0.02)
        rc, flags, code = subject.read()
    held = rc == RC_OK and rule.holds(flags)
    for _ in range(3):
        if not held:
            break
        time.sleep(0.02)
        rc, flags, code = subject.read()
        held = rc == RC_OK and rule.holds(flags)
    return rc, flags, code, held


def wanted_code(subject, vocabulary: str, spec: dict, code: int, klass: str) -> tuple[int | None, str]:
    """The fault word a read should carry for `code`, and a phrase for it. `None` means only that it must not be zero."""
    if klass == "ok":
        return 0, "zero"
    kind = spec.get("fault_code", "verbatim")
    if kind == "verbatim":
        return code, f"the maker's code {code}, verbatim"
    if kind == "derived":
        return subject.expected_code(vocabulary, code), "the value the subject derives from it"
    if kind == "absent":
        return -1, ""
    return None, "something other than zero"


def _not_seen(flags: int) -> str:
    """Why a missing bit matters, said only as far as it is true: §27 reads DEGRADED for `present` and ESTOP for `stop`, and nothing else."""
    if flags & (ESTOP | DEGRADED):
        return " — §27's `rhal` condition reads DEGRADED, and a fault that sets another bit alone is read only through that"
    return " — the OS would see nothing: §27 reads DEGRADED and ESTOP, and this sets neither"


def check_one(t: Result, subject, vocabulary: str, spec: dict, code: int, klass: str, label: str, settle_s: float) -> None:
    rule = CLASSES[klass]
    reason = subject.can_inject(vocabulary, code)
    if reason is not None:
        t.not_injectable.append(f"{vocabulary} {code} ({label}): {reason}")
        print(f"  --    {label}: not injectable here — {reason}", flush=True)
        return
    t.injected += 1
    subject.inject(vocabulary, code)
    rc, flags, fault_word, held = observe(subject, rule, settle_s)
    at = f"{vocabulary} {code} {label} → {klass}"
    t.check(
        held,
        f"{at}: reads {flag_names(flags)} (rc {rc}) and the class wants {flag_names(rule.required)}"
        + (f", never {flag_names(rule.forbidden)}" if rule.forbidden else "")
        + (_not_seen(flags) if klass in ("fault", "estop") and rc == RC_OK else ""),
    )
    want, phrase = wanted_code(subject, vocabulary, spec, code, klass)
    if want == -1:
        pass  # `absent`: the vocabulary has no code of its own to carry, so the fault word says nothing about it (the flags do)
    elif want is None:
        t.check(fault_word != 0, f"{at}: the fault word is {fault_word}, and a fault with no code is one nobody can look up")
    else:
        t.check(fault_word == want, f"{at}: the fault word is {fault_word}, wanted {phrase}")
    command = subject.write()
    if rule.commands == "refused":
        t.check(command != RC_OK, f"{at}: a command is refused (rc {command}); `SKR_OK` would report success the robot did not have")
    else:
        t.check(command == RC_OK, f"{at}: a command is taken (rc {command})")
    if klass in ("fault", "estop"):
        check_keeps(t, subject, spec, at, klass)
    subject.clear(vocabulary)
    _, flags, _, healthy = observe(subject, CLASSES["ok"], settle_s + 2.0)
    if not t.check(healthy, f"{at}: when the substrate clears it the driver reads healthy again (reads {flag_names(flags)}) — a fault is not sticky"):
        subject.restart()


def check_keeps(t: Result, subject, spec: dict, at: str, klass: str) -> None:
    """The motions a vocabulary says stay available in a failsafe: answered in `fault`, refused in `estop`."""
    for motion in spec.get("keeps", []):
        rc = subject.motion(motion)
        if klass == "fault":
            t.check(
                rc == RC_OK,
                f"{at}: `{motion}` stays available (rc {rc}) — the way out of a failsafe is not refused because the body has reported one",
            )
        else:
            t.check(rc != RC_OK, f"{at}: `{motion}` is refused (rc {rc}) — the stop chain is open and nothing moves the body")
        restore = getattr(subject, "restore", None)
        if restore is not None:
            restore()


def check_vocabulary(t: Result, subject, name: str, spec: dict) -> None:
    settle_s = float(getattr(subject, "settle_s", 1.0))
    print(f"\n{name} — {spec['channel']}", flush=True)
    for key in sorted(spec["codes"], key=int):
        entry = spec["codes"][key]
        check_one(t, subject, name, spec, int(key), entry["class"], entry["name"], settle_s)
    escalation = spec["unmapped"]
    for code in spec["probes"]:
        check_one(t, subject, name, spec, int(code), escalation, f"unlisted ({code})", settle_s)


def run(subject, entry: dict, only: list[str] | None = None) -> Result:
    t = Result()
    print(f"fault vocabulary: {subject.name}", flush=True)
    subject.start()
    try:
        for name, spec in entry["vocabularies"].items():
            if only and name not in only:
                continue
            check_vocabulary(t, subject, name, spec)
    finally:
        try:
            subject.stop()
        except Exception:  # noqa: BLE001 — stopping a body whose substrate has gone is still stopping it
            pass
    print(
        f"\n{t.count} checks, {t.failed} failed; {t.injected} codes injected, {len(t.not_injectable)} not injectable here",
        flush=True,
    )
    return t


def list_spec(spec: dict) -> None:
    for driver, entry in spec["drivers"].items():
        for name, vocabulary in entry.get("vocabularies", {}).items():
            counts: dict[str, int] = {}
            for code in vocabulary["codes"].values():
                counts[code["class"]] = counts.get(code["class"], 0) + 1
            listed = ", ".join(f"{count} {klass}" for klass, count in sorted(counts.items()))
            keeps = f"; keeps {', '.join(vocabulary['keeps'])} in a failsafe" if vocabulary.get("keeps") else ""
            print(f"{driver:12} {name:18} {listed}; unlisted → {vocabulary['unmapped']}; {len(vocabulary['probes'])} probes{keeps}")
        for gap in entry.get("not_covered", []):
            print(f"{driver:12} not covered: {gap['what']} — {gap['why']}")
        if entry.get("table_of"):
            print(f"{driver:12} holds to the table of {entry['table_of']}, by a test of its own")
        elif not entry.get("vocabularies"):
            print(f"{driver:12} pending: {entry.get('pending', '')[:110]}…")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("subject", nargs="?", help="a Python file defining `subject()`, e.g. drivers/ur_rtde/faultvocab.py")
    parser.add_argument("--only", action="append", help="a vocabulary by name; repeatable")
    parser.add_argument("--list", action="store_true", help="print the vocabulary file's contents and stop")
    options = parser.parse_args(argv)
    spec = load_spec()
    if options.list:
        list_spec(spec)
        return 0
    if not options.subject:
        parser.error("name a subject, or --list")
    loaded = importlib.util.spec_from_file_location("faultvocab_subject", options.subject)
    module = importlib.util.module_from_spec(loaded)
    sys.path.insert(0, os.path.dirname(os.path.abspath(options.subject)))
    loaded.loader.exec_module(module)
    subject = module.subject()
    entry = spec["drivers"].get(subject.driver)
    if entry is None:
        print(f"fault vocabulary: `{subject.driver}` has no entry in {os.path.relpath(VOCABULARY_FILE, REPO)}. Add one, or say why it has none.")
        return 2
    result = run(subject, entry, options.only)
    return 1 if result.failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

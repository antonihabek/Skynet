#!/usr/bin/env python3
"""What the fault-vocabulary contract does, shown — against drivers written to be wrong in each way it names, and one written to be right.

    python3 tools/fault-vocab/selftest.py

A contract nobody has watched fail proves only that it runs (CONTRIBUTING.md: a check that has never failed has proved nothing). So each way a
driver can mistranslate a maker's code is built here as a toy and the contract is run against it, and the run must go red *for that reason*:

  silent             reads a code and says nothing: no flag, a zero fault word          — the robot is reported well
  degraded-only      a stopped robot is reported DEGRADED and no more                   — §27 sees an impaired robot, not a stopped one
  fault-only         FAULT without DEGRADED                                             — §27 reads DEGRADED, not FAULT: the OS sees `present`
  commands-taken     the flags are right and a command is still answered OK             — success the robot did not have
  unlisted-ok        a code in no table is read as healthy                              — the code nobody thought of
  unlisted-degraded  a code in no table is read as merely degraded, commands taken      — below the floor
  code-dropped       the flags are right and the fault word is zero                     — a fault nobody can look up
  code-replaced      the fault word is something else (a count, a position)             — not the maker's code, so not verbatim
  estop-for-fault    a fault reported as a stop chain open                              — a name put on an event that has none
  sticky             a cleared fault is still reported                                  — F142's shape
  flapping           right on one read in two                                           — a pass on the read that happened to agree
  keeps-nothing      a failsafe refuses the motion that gets the body out of it         — the way out taken away by the report of the failsafe
  keeps-in-stop      a stop chain open and a motion still answered                      — a body moved while its stop is asserted

and the careful toy, which must pass everything, and the vocabulary file itself, which must be one the contract can hold anybody to.
"""

from __future__ import annotations

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import contract  # noqa: E402
from contract import CLASSES, DEGRADED, ESTOP, FAULT, RC_OK  # noqa: E402

#: A vocabulary shaped like a real one, small enough to read: two codes in every class and a table that says what an unlisted code escalates to.
ENTRY = {
    "vocabularies": {
        "state": {
            "channel": "a toy's state word",
            "codes": {
                "1": {"name": "NORMAL", "class": "ok"},
                "2": {"name": "REDUCED", "class": "degraded"},
                "3": {"name": "STOPPED", "class": "fault"},
                "4": {"name": "HALTED", "class": "fault"},
                "5": {"name": "ESTOP", "class": "estop"},
            },
            "unmapped": "estop",
            "probes": [40, 255],
            "fault_code": "verbatim",
            "keeps": ["out"],
        }
    }
}


class Toy:
    """A driver that translates a state word the way `defect` says. `None` is the careful one."""

    driver = "toy"
    settle_s = 0.05

    def __init__(self, defect: str | None = None) -> None:
        self.defect = defect
        self.name = f"toy ({defect or 'careful'})"
        self.code = 1
        self.reads = 0
        self.latched = False

    def start(self) -> None:
        self.code, self.reads, self.latched = 1, 0, False

    def restart(self) -> None:
        self.start()

    def stop(self) -> None:
        pass

    def can_inject(self, vocabulary: str, code: int) -> str | None:
        return None

    def inject(self, vocabulary: str, code: int) -> None:
        self.code = code
        self.latched = True

    def clear(self, vocabulary: str) -> None:
        self.code = 1
        if self.defect != "sticky":
            self.latched = False

    def klass(self) -> str:
        table = {1: "ok", 2: "degraded", 3: "fault", 4: "fault", 5: "estop"}
        listed = table.get(self.code)
        if listed is not None:
            if self.defect == "silent" and listed != "ok":
                return "ok"
            if self.defect == "degraded-only" and listed in ("fault", "estop"):
                return "degraded"
            if self.defect == "estop-for-fault" and listed == "fault":
                return "estop"
            return listed
        if self.defect == "unlisted-ok":
            return "ok"
        if self.defect == "unlisted-degraded":
            return "degraded"
        return "estop"

    def read(self) -> tuple[int, int, int]:
        self.reads += 1
        klass = self.klass()
        if self.defect == "sticky" and self.latched and self.code == 1:
            klass = "fault"
        if self.defect == "flapping" and self.reads % 2 == 0:
            klass = "ok"
        rule = CLASSES[klass]
        flags = rule.required
        if self.defect == "fault-only" and klass == "fault":
            flags = FAULT
        word = 0 if klass == "ok" else self.code
        if self.defect == "code-dropped":
            word = 0
        if self.defect == "code-replaced" and klass != "ok":
            word = 7
        return RC_OK, flags, word

    def motion(self, name: str) -> int:
        """The way out: answered in a failsafe, refused with the stop chain open — and the two toys that get it wrong."""
        klass = self.klass()
        if self.defect == "keeps-nothing" and klass == "fault":
            return -1
        if self.defect == "keeps-in-stop" and klass == "estop":
            return RC_OK
        return -1 if klass == "estop" else RC_OK

    def write(self) -> int:
        refused = CLASSES[self.klass()].commands == "refused"
        if self.defect == "commands-taken":
            return RC_OK
        if self.defect in ("silent", "unlisted-ok", "unlisted-degraded") and not refused:
            return RC_OK
        return -1 if refused else RC_OK


def run_toy(defect: str | None) -> contract.Result:
    sink = open(os.devnull, "w", encoding="utf-8")
    saved, sys.stdout = sys.stdout, sink
    try:
        return contract.run(Toy(defect), ENTRY)
    finally:
        sys.stdout = saved
        sink.close()


class Checks:
    def __init__(self) -> None:
        self.count = 0
        self.failed = 0

    def check(self, condition: bool, what: str) -> bool:
        self.count += 1
        print(f"  {'ok  ' if condition else 'FAIL'}  {what}", flush=True)
        if not condition:
            self.failed += 1
        return bool(condition)


#: Each defect, and a phrase every one of the contract's complaints about it must be able to contain — so the run goes red for the right reason
#: and not merely goes red.
DEFECTS = {
    "silent": "wants",
    "degraded-only": "wants FAULT+DEGRADED",
    "fault-only": "wants FAULT+DEGRADED",
    "commands-taken": "a command is refused",
    "unlisted-ok": "unlisted",
    "unlisted-degraded": "unlisted",
    "code-dropped": "the fault word",
    "code-replaced": "the fault word",
    "estop-for-fault": "never ESTOP",
    "sticky": "not sticky",
    "flapping": "reads",
    "keeps-nothing": "stays available",
    "keeps-in-stop": "is refused",
}


def toys(t: Checks) -> None:
    t.check(run_toy(None).failed == 0, "the careful toy passes every check the contract has")
    careful = run_toy(None)
    t.check(careful.count >= 5 * 4 + 2 * 4, f"and it was asked a great deal: {careful.count} checks over five listed codes and two probes")
    for defect, phrase in DEFECTS.items():
        result = run_toy(defect)
        t.check(result.failed > 0, f"{defect}: the contract goes red ({result.failed} of {result.count} checks)")
        t.check(
            any(phrase in failure for failure in result.failures),
            f"{defect}: and one of its complaints says why — `{phrase}`",
        )


def fault_only_is_the_one_the_os_cannot_see(t: Checks) -> None:
    """§27 reads DEGRADED and ESTOP; the class rules must therefore never let FAULT stand alone."""
    for klass in ("fault", "estop"):
        t.check(bool(CLASSES[klass].required & DEGRADED), f"the {klass} class requires DEGRADED, the one bit of the pair §27 reads")
    t.check(CLASSES["estop"].required & ESTOP == ESTOP, "and the estop class requires ESTOP, which §27 reads for `stop`")
    t.check(not CLASSES["fault"].required & ESTOP and CLASSES["fault"].forbidden & ESTOP == ESTOP, "while a fault may not name a stop chain it has not seen open")


def the_file(t: Checks) -> None:
    """A vocabulary file the contract can hold somebody to: classes it knows, an escalation that is not a shrug, probes outside the table."""
    spec = contract.load_spec()
    t.check(bool(spec["drivers"]), "the file names drivers")
    for driver, entry in spec["drivers"].items():
        for name, vocabulary in entry.get("vocabularies", {}).items():
            at = f"{driver} {name}"
            classes = {code["class"] for code in vocabulary["codes"].values()}
            t.check(classes <= set(CLASSES), f"{at}: every code is in one of the four classes ({sorted(classes)})")
            t.check(
                vocabulary["unmapped"] in contract.escalations(vocabulary),
                f"{at}: an unlisted code escalates to {' or '.join(contract.escalations(vocabulary))}, never to healthy ({vocabulary['unmapped']})",
            )
            t.check(not set(map(str, vocabulary["probes"])) & set(vocabulary["codes"]), f"{at}: no probe is a code the table lists")
            if vocabulary.get("no_probes"):
                t.check(not vocabulary["probes"] and len(vocabulary["no_probes"]) > 40, f"{at}: no probes, and it says why none can arrive")
            else:
                t.check(len(vocabulary["probes"]) >= 3, f"{at}: at least three probes, so one lucky value is not the evidence")
            t.check(0 not in vocabulary["probes"], f"{at}: no probe is zero, §24's healthy")
            if "keeps" in vocabulary:
                t.check(
                    bool(vocabulary["keeps"]) and all(isinstance(m, str) and m for m in vocabulary["keeps"]),
                    f"{at}: `keeps` names the motions that stay available in a failsafe",
                )
                t.check(len(vocabulary.get("keeps_because", "")) >= 40, f"{at}: and says why those and no others")
                t.check(
                    any(code["class"] == "fault" for code in vocabulary["codes"].values()),
                    f"{at}: and has a fault class for them to stay available in",
                )
            t.check(
                all(isinstance(entry_, dict) and entry_.get("name") for entry_ in vocabulary["codes"].values()),
                f"{at}: every code carries the maker's own name for it",
            )
            t.check(
                "NOT measured" in vocabulary["source"] or "measured" in vocabulary["source"],
                f"{at}: the source says what was measured against the maker's software and what was only read",
            )
        if "vocabularies" not in entry:
            shares = isinstance(entry.get("table_of"), str) and entry["table_of"] in spec["drivers"]
            says_why = len(entry.get("pending", "")) >= 40
            t.check(shares or says_why, f"{driver}: no vocabulary of its own, and the entry says whose table it holds to or why it has none")
        for gap in entry.get("not_covered", []):
            t.check(bool(gap.get("what")) and len(gap.get("why", "")) > 40, f"{driver}: a vocabulary left uncovered states a reason ({gap.get('what')})")
    json.dumps(spec)


def main() -> int:
    t = Checks()
    print("the contract, against drivers wrong in each way it names")
    toys(t)
    print("\nthe classes, against what §27 reads")
    fault_only_is_the_one_the_os_cannot_see(t)
    print("\nthe vocabulary file")
    the_file(t)
    print(f"\n{t.count} checks, {t.failed} failed", flush=True)
    return 1 if t.failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

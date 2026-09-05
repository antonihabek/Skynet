"""The conformance corpus runner, Python edition.

    An edition that has not passed the corpus is not an edition. It cannot carry the name, appear
    in the matrix, or be installed by ``skynetd``. (§31.4)

This is that mechanism for the Python edition. It reads ``corpus/**/*.json``, runs each case's input
through this edition's modules, serialises the result in canonical form, and compares it to the
case's ``expected`` **string** — byte for byte, not field by field.

Why byte-identical and not "equivalent": because equivalence is where drift hides. Two
implementations that emit the same warnings in a different order are equivalent today and are
computing a different clamp the day somebody writes code that reads the first warning.

# An unimplemented corpus is a failure, not a skip

The Rust runner refuses to silently skip a corpus it does not recognise, and this one refuses
harder, because it is the runner of an **incomplete** edition. Every corpus with no handler is
reported as ``unimplemented`` and counted against the total, so the honest sentence about this
edition is always available as a number. A runner that quietly passed 4 of 31 corpora and printed
"OK" would be the single most misleading artifact in this repository.

A corpus is not the unit, though, and that took a second reading of the corpus to see. ``fuzz`` is
one corpus spanning three files — ``cycle``, ``observation`` and ``transport`` — so the day one of
them is written the other two arrive at a handler never written for them. Counting those as
**failures** would be a different claim from the true one, and a louder one: a hundred cases nobody
has transcribed yet would read as a hundred cases this edition gets wrong. So a handler may decline
a file by raising :class:`Unimplemented`, the runner counts it in the same bucket as a corpus with
no handler at all, and a corpus holding a declined file is never named ``complete``.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

from .canonical import canonical
from . import admission as admission_module
from . import calibration as calibration_module
from . import cell as cell_module
from . import certification as certification_module
from . import checkin as checkin_module
from . import birth as birth_module
from . import blackbox as blackbox_module
from . import digest as digest_module
from . import embodiment as embodiment_module
from . import identity as identity_module
from . import intent as intent_module
from . import kinematics as kinematics_module
from . import live as live_module
from . import observation as observation_module
from . import opstate as opstate_module
from . import ota as ota_module
from . import power as power_module
from . import predicate as predicate_module
from . import provenance as provenance_module
from . import readiness as readiness_module
from . import roles as roles_module
from . import watchdog as watchdog_module
from . import safety as safety_module
from . import semver as semver_module
from . import skill as skill_module
from . import telemetry as telemetry_module
from . import transfer as transfer_module
from . import transport as transport_module
from . import vocabulary as vocabulary_module

__all__ = [
    "Case",
    "CorpusFile",
    "Failure",
    "Report",
    "Unimplemented",
    "run_corpus",
    "load_corpus",
]


class Unimplemented(Exception):
    """Raised by a handler for a corpus file this edition has not transcribed yet.

    Distinct from a failure, and the distinction is the whole point. A handler is dispatched on the
    corpus kind, and several kinds span several files: implementing ``fuzz/cycle`` would otherwise
    send ``fuzz/observation`` and ``fuzz/transport`` into an arm never written for them and count
    108 untranscribed cases as *wrong answers*. That number is not merely unhelpful, it is false —
    and it gets worse exactly when the edition is making progress, which is the opposite of what a
    progress number is for.

    Declining is a property of a **file**. A handler that declines some of a file's cases and
    answers others is a defect in this edition rather than a gap in it, and the runner reports it as
    failures for that reason.
    """

    def __init__(self, corpus: str, label: str, note: str = "") -> None:
        self.corpus = corpus
        self.label = label
        self.note = note
        detail = f" — {note}" if note else ""
        super().__init__(f"{corpus}/{label} is not implemented in this edition{detail}")


@dataclass
class Case:
    id: str
    note: str
    input: dict[str, Any]
    expected: str


@dataclass
class CorpusFile:
    """One corpus file: its declared kind, the label taken from the path, and its cases."""

    path: Path
    corpus: str
    #: The file stem, which the dispatch uses to tell `semver/compare` from `semver/satisfies`.
    label: str
    cases: list[Case]


@dataclass
class Failure:
    corpus: str
    label: str
    case: str
    note: str
    expected: str
    produced: str
    #: Index of the first differing byte, or ``None`` when the case did not produce output at all.
    diverged_at: int | None
    reason: str | None = None

    def describe(self) -> str:
        if self.reason is not None:
            return f"{self.corpus}/{self.label} :: {self.case} — {self.reason}"
        marker = "" if self.diverged_at is None else f" (first difference at byte {self.diverged_at})"
        return (
            f"{self.corpus}/{self.label} :: {self.case}{marker}\n"
            f"    note     {self.note}\n"
            f"    expected {self.expected}\n"
            f"    produced {self.produced}"
        )


@dataclass
class Report:
    passed: int = 0
    failed: int = 0
    #: Cases in corpora this edition has no handler for at all. Counted, never hidden.
    no_handler_cases: int = 0
    #: Cases in files a handler exists for and declined by name. The same bucket, kept separate
    #: because the two say different things about where the edition stands.
    declined_cases: int = 0
    failures: list[Failure] = field(default_factory=list)
    #: Corpus kinds with no handler, in the order first seen.
    missing_handlers: list[str] = field(default_factory=list)
    #: ``corpus/label`` for every file a handler declined, in the order first seen.
    declined_files: list[str] = field(default_factory=list)
    #: Corpus kinds where every case passed **and** no file was declined.
    complete: list[str] = field(default_factory=list)
    #: Corpus kinds holding at least one declined file. Never named complete, however many of
    #: their other files pass — a corpus is complete or it is not.
    partial: list[str] = field(default_factory=list)

    @property
    def unimplemented(self) -> int:
        return self.no_handler_cases + self.declined_cases

    @property
    def total(self) -> int:
        return self.passed + self.failed + self.unimplemented

    def summary(self) -> str:
        return (
            f"{self.passed} passed, {self.failed} failed, "
            f"{self.unimplemented} not implemented "
            f"({self.no_handler_cases} in corpora with no handler, "
            f"{self.declined_cases} in files a handler declined), "
            f"{self.total} total"
        )


def load_corpus(root: Path) -> list[CorpusFile]:
    """Read every corpus file under ``root``, sorted so a run is reproducible."""
    files: list[CorpusFile] = []
    for path in sorted(root.glob("*/*.json")):
        document = json.loads(path.read_text(encoding="utf-8"))
        cases = [
            Case(
                id=case.get("id", "<unnamed>"),
                note=case.get("note", ""),
                input=case.get("input", {}),
                expected=case["expected"],
            )
            for case in document.get("cases", [])
        ]
        files.append(
            CorpusFile(path=path, corpus=document["corpus"], label=path.stem, cases=cases)
        )
    return files


# ── Dispatch ─────────────────────────────────────────────────────────────────
#
# Keyed on the corpus kind and, where one kind spans several files with different inputs, the file
# label. The Rust runner learned this the hard way: a catch-all arm meant a new fuzz family arrived
# not as an unsupported corpus but as forty-eight vectors dispatched into an arm never written for
# them. Every entry here says which files it is for.

Handler = Callable[[str, dict[str, Any]], Any]


def _provenance(label: str, case_input: dict[str, Any]) -> Any:
    """§56's two questions: what a run is worth, and whether two roll-ups of it agree."""
    if label == "classify":
        return provenance_module.classify_to_json(case_input)
    if label == "scores":
        return provenance_module.scores_to_json(case_input)
    raise Unimplemented("provenance", label)


def _readiness(label: str, case_input: dict[str, Any]) -> Any:
    """One robot, and the registry it sits in."""
    if label == "robot":
        return readiness_module.readiness_to_json(case_input)
    if label == "fleet":
        return readiness_module.fleet_to_json(case_input)
    raise Unimplemented("readiness", label)


def _roles(_label: str, case_input: dict[str, Any]) -> Any:
    return roles_module.role_to_json(roles_module.parse_role(case_input.get("role")))


def _vocabulary(_label: str, case_input: dict[str, Any]) -> Any:
    return vocabulary_module.closed_set_to_json(case_input.get("set"))


def _digest(_label: str, case_input: dict[str, Any]) -> Any:
    return digest_module.digest_to_json(
        case_input.get("kind", ""),
        case_input.get("version", ""),
        case_input.get("manifest"),
        case_input.get("files", []),
    )


def _frames(label: str, case_input: dict[str, Any]) -> Any:
    if "bindings" in label:
        return embodiment_module.frame_fault_to_json(
            embodiment_module.validate_frame_bindings(case_input["graph"], case_input.get("joints", []))
        )
    if "validate" in label:
        return embodiment_module.frame_fault_to_json(
            embodiment_module.validate_frames(case_input["graph"])
        )
    raise Unimplemented("frames", label)


def _identity(label: str, case_input: dict[str, Any]) -> Any:
    """Three files: what somebody typed, what credential is held, and what is running."""
    if label == "pairing":
        return identity_module.code_to_json(case_input)
    if label == "credential":
        return identity_module.identity_to_json(case_input)
    if label == "attestation":
        return identity_module.attestation_to_json(case_input)
    raise Unimplemented("identity", label)


def _blackbox(label: str, case_input: dict[str, Any]) -> Any:
    """Four files, and each holds a different kind of thing.

    `schemas` pins the six MCAP channel schemas — §31.2 keeps them single-source across editions —
    `records` pins the ring's record layout, `header` pins the ring header and its two tables, and
    `seal` pins the framing of a whole file by its digest. This is the one corpus whose subject is
    bytes rather than a verdict.
    """
    if label == "schemas":
        return blackbox_module.schemas_to_json(case_input)
    if label == "records":
        return blackbox_module.records_to_json(case_input)
    if label == "header":
        return blackbox_module.header_to_json(case_input)
    if label == "seal":
        return blackbox_module.seal_to_json(case_input)
    raise Unimplemented("blackbox", label)


def _calibration(label: str, case_input: dict[str, Any]) -> Any:
    """§45's validity, over whatever a robot has measured and whatever its body can hold."""
    if label == "sets":
        return calibration_module.sets_to_json(case_input)
    return calibration_module.calibration_to_json(case_input)


def _cell(label: str, case_input: dict[str, Any]) -> Any:
    """Two files, because the document has two consumers and an edition could agree about one while
    disagreeing about the other: `validate` is what an operator sees when they save it, `context` is
    what a precondition and the workspace guard read out of it afterwards.
    """
    if label == "validate":
        return cell_module.validate_to_json(case_input)
    if label == "context":
        return cell_module.context_to_json(case_input)
    raise Unimplemented("cell", label)


def _checkin(label: str, case_input: dict[str, Any]) -> Any:
    """Three files, because a check-in has three separable ways to be wrong.

    `claim` is the field §17's rule lives on — the platform never claims a robot is running
    something it has not heard from — `health` is §33.6's age rule, and `document` is the bytes that
    actually cross to the Hub. An edition could reproduce the first two and still assemble them
    differently, which is the case only the third one catches.
    """
    if label == "claim":
        return checkin_module.claim_to_json(case_input)
    if label == "health":
        return checkin_module.health_to_json(case_input)
    if label == "document":
        return checkin_module.document_to_json(case_input)
    raise Unimplemented("checkin", label)


def _intent(_label: str, case_input: dict[str, Any]) -> Any:
    return intent_module.offer_to_json(case_input)


def _kinematics(_label: str, case_input: dict[str, Any]) -> Any:
    return kinematics_module.solved_to_json(
        case_input.get("frames", {}),
        case_input.get("joints", []),
        case_input.get("command", {}),
    )


def _opstate(label: str, case_input: dict[str, Any]) -> Any:
    """Three files: one evaluation, and two whole-table freezes of the predicates it uses."""
    if label == "evaluate":
        return opstate_module.transition_to_json(case_input)
    if label == "properties":
        return opstate_module.properties_to_json(case_input)
    if label == "warning":
        return opstate_module.warning_matrix_to_json(case_input)
    raise Unimplemented("opstate", label)


def _ota(label: str, case_input: dict[str, Any]) -> Any:
    """Two of §12's three rules: may this image be installed, and is the slot it booted into good."""
    if label == "bundle":
        return ota_module.bundle_to_json(case_input)
    if label == "boot":
        return ota_module.boot_to_json(case_input)
    raise Unimplemented("ota", label)


def _power(_label: str, case_input: dict[str, Any]) -> Any:
    """Five files, one shape: §36's assessment plus every predicate a caller reads off it."""
    return power_module.power_to_json(case_input)


def _predicate(_label: str, case_input: dict[str, Any]) -> Any:
    return predicate_module.predicate_to_json(case_input.get("predicate"), case_input.get("context"))


def _semver(label: str, case_input: dict[str, Any]) -> Any:
    if "satisfies" in label:
        return semver_module.satisfies_to_json(case_input["version"], case_input["range"])
    if "compare" in label:
        return semver_module.compare_to_json(case_input["a"], case_input["b"])
    raise Unimplemented("semver", label)


def _live(_label: str, case_input: dict[str, Any]) -> Any:
    """§54.2's live context. One file, because there is one function.

    Every case in it is really a case about a single rule: a fact the robot does not know is
    *absent* rather than defaulted, because §54.2 refuses a precondition it cannot evaluate and
    "assume true" is what free-text preconditions do by construction.
    """
    return live_module.context_to_json(case_input)


def _observation(label: str, case_input: dict[str, Any]) -> Any:
    """Two files with different inputs: a frame to assemble, and the instant to assemble it at."""
    if label == "frames":
        return observation_module.frame_to_json(case_input)
    if label == "target":
        return observation_module.target_to_json(case_input)
    raise Unimplemented("observation", label)


def _limits(_label: str, case_input: dict[str, Any]) -> Any:
    return safety_module.limits_to_json(case_input)


def _admission(_label: str, case_input: dict[str, Any]) -> Any:
    return admission_module.admission_to_json(case_input)


def _birth(_label: str, case_input: dict[str, Any]) -> Any:
    return birth_module.attested_to_json(case_input)


def _certification(_label: str, case_input: dict[str, Any]) -> Any:
    return certification_module.certification_to_json(case_input)


def _cycle(_label: str, case_input: dict[str, Any]) -> Any:
    return safety_module.verdict_to_json(case_input)


def _fuzz(label: str, case_input: dict[str, Any]) -> Any:
    """One corpus, three files, and not all of them written.

    A file with no handler declines by name rather than raising, so its cases are counted as *not
    implemented* instead of as wrong answers. The corpus is reported `partial` for exactly as long
    as that is true of any of the three.
    """
    if label == "cycle":
        return safety_module.verdict_to_json(case_input)
    if label == "observation":
        return observation_module.frame_to_json(case_input)
    if label == "transport":
        return transport_module.plan_to_json(case_input)
    raise Unimplemented("fuzz", label)


def _skill(label: str, case_input: dict[str, Any]) -> Any:
    """Six files, six questions: §54.2 twice, §54.3, §54.4 twice and §54.5."""
    if label == "preconditions":
        return skill_module.preconditions_to_json(case_input)
    if label == "success":
        return skill_module.success_to_json(case_input)
    if label == "timeout":
        return skill_module.timeout_to_json(case_input)
    if label == "bounded-deadline":
        return skill_module.bounded_deadline_to_json(case_input)
    if label == "composition":
        return skill_module.composition_to_json(case_input)
    if label == "lifecycle":
        return skill_module.lifecycle_to_json(case_input)
    raise Unimplemented("skill", label)


def _telemetry(label: str, case_input: dict[str, Any]) -> Any:
    """§74's telemetry. Two files, because a window and a set of them are two questions.

    An edition could agree about the first while disagreeing about the second — and the second is
    the one §15's field conformance divides by, which is where a gap becomes visible.
    """
    if label == "window":
        return telemetry_module.window_to_json(case_input)
    if label == "coverage":
        return telemetry_module.coverage_to_json(case_input)
    raise Unimplemented("telemetry", label)


def _transfer(label: str, case_input: dict[str, Any]) -> Any:
    """Four files: a fetch and its arrival, an upload and its receipt. Mirrors, not duplicates."""
    if label == "fetch":
        return transfer_module.fetch_to_json(case_input)
    if label == "arrival":
        return transfer_module.arrival_to_json(case_input)
    if label == "upload":
        return transfer_module.upload_to_json(case_input)
    if label == "receipt":
        return transfer_module.receipt_to_json(case_input)
    raise Unimplemented("transfer", label)


def _transport(_label: str, case_input: dict[str, Any]) -> Any:
    """Seven files, one input shape: a source policy, a target body, and the caller's options."""
    return transport_module.plan_to_json(case_input)


def _watchdog(_label: str, case_input: dict[str, Any]) -> Any:
    return watchdog_module.watchdog_to_json(
        case_input.get("on_command_loss"),
        case_input.get("windows", {}),
    )


HANDLERS: dict[str, Handler] = {
    "admission": _admission,
    "birth": _birth,
    "blackbox": _blackbox,
    "calibration": _calibration,
    "cell": _cell,
    "certification": _certification,
    "checkin": _checkin,
    "cycle": _cycle,
    "digest": _digest,
    "fuzz": _fuzz,
    "limits": _limits,
    "live": _live,
    "observation": _observation,
    "opstate": _opstate,
    "ota": _ota,
    "power": _power,
    "frames": _frames,
    "identity": _identity,
    "intent": _intent,
    "kinematics": _kinematics,
    "predicate": _predicate,
    "provenance": _provenance,
    "readiness": _readiness,
    "roles": _roles,
    "vocabulary": _vocabulary,
    "semver": _semver,
    "skill": _skill,
    "telemetry": _telemetry,
    "transfer": _transfer,
    "transport": _transport,
    "watchdog": _watchdog,
}


def run_corpus(root: Path) -> Report:
    """Run every case and report. Never raises for a failing case — that is what the report is."""
    report = Report()
    per_corpus: dict[str, list[bool]] = {}

    for corpus_file in load_corpus(root):
        handler = HANDLERS.get(corpus_file.corpus)

        if handler is None:
            report.no_handler_cases += len(corpus_file.cases)
            if corpus_file.corpus not in report.missing_handlers:
                report.missing_handlers.append(corpus_file.corpus)
            continue

        # The whole file is evaluated before anything is recorded, because declining is a property
        # of a file and cannot be acted on one case at a time: whether a declined case is *not yet
        # written* or *an inconsistency in the handler* depends on what the file's other cases did.
        evaluated: list[tuple[str, Case, Any]] = []
        for case in corpus_file.cases:
            try:
                evaluated.append(
                    ("produced", case, canonical(handler(corpus_file.label, case.input)))
                )
            except Unimplemented:
                evaluated.append(("declined", case, None))
            except Exception as error:  # noqa: BLE001 — a raising case is a failing case, reported
                evaluated.append(("raised", case, error))

        declined = [entry for entry in evaluated if entry[0] == "declined"]

        if declined and len(declined) == len(evaluated):
            report.declined_cases += len(declined)
            name = f"{corpus_file.corpus}/{corpus_file.label}"
            if name not in report.declined_files:
                report.declined_files.append(name)
            if corpus_file.corpus not in report.partial:
                report.partial.append(corpus_file.corpus)
            continue

        outcomes = per_corpus.setdefault(corpus_file.corpus, [])

        for kind, case, payload in evaluated:
            if kind == "declined":
                # Some of this file answered and some declined. Counting the declined ones as *not
                # implemented* would hide a live inconsistency behind a number that reads as honest
                # progress, which is the one thing this runner exists to prevent.
                report.failed += 1
                outcomes.append(False)
                report.failures.append(
                    Failure(
                        corpus=corpus_file.corpus,
                        label=corpus_file.label,
                        case=case.id,
                        note=case.note,
                        expected=case.expected,
                        produced="",
                        diverged_at=None,
                        reason=(
                            f"handler declined {len(declined)} of {len(evaluated)} cases in this "
                            "file — Unimplemented is a property of a file, not of a case"
                        ),
                    )
                )
                continue

            if kind == "raised":
                report.failed += 1
                outcomes.append(False)
                report.failures.append(
                    Failure(
                        corpus=corpus_file.corpus,
                        label=corpus_file.label,
                        case=case.id,
                        note=case.note,
                        expected=case.expected,
                        produced="",
                        diverged_at=None,
                        reason=f"{type(payload).__name__}: {payload}",
                    )
                )
                continue

            if payload == case.expected:
                report.passed += 1
                outcomes.append(True)
                continue

            report.failed += 1
            outcomes.append(False)
            report.failures.append(
                Failure(
                    corpus=corpus_file.corpus,
                    label=corpus_file.label,
                    case=case.id,
                    note=case.note,
                    expected=case.expected,
                    produced=payload,
                    diverged_at=_first_difference(case.expected, payload),
                )
            )

    report.complete = sorted(
        kind
        for kind, outcomes in per_corpus.items()
        if outcomes and all(outcomes) and kind not in report.partial
    )
    return report


def _first_difference(expected: str, produced: str) -> int:
    left = expected.encode("utf-8")
    right = produced.encode("utf-8")
    for index in range(min(len(left), len(right))):
        if left[index] != right[index]:
            return index
    return min(len(left), len(right))

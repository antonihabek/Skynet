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
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Callable

from .canonical import canonical
from . import roles as roles_module
from . import semver as semver_module
from . import vocabulary as vocabulary_module

__all__ = ["Case", "CorpusFile", "Failure", "Report", "run_corpus", "load_corpus"]


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
    #: Cases in corpora this edition has no handler for. Counted, never hidden.
    unimplemented: int = 0
    failures: list[Failure] = field(default_factory=list)
    #: Corpus kinds with no handler, in the order first seen.
    missing_handlers: list[str] = field(default_factory=list)
    #: Corpus kinds where every case passed.
    complete: list[str] = field(default_factory=list)

    @property
    def total(self) -> int:
        return self.passed + self.failed + self.unimplemented

    def summary(self) -> str:
        return (
            f"{self.passed} passed, {self.failed} failed, "
            f"{self.unimplemented} in corpora with no handler, {self.total} total"
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


def _roles(_label: str, case_input: dict[str, Any]) -> Any:
    return roles_module.role_to_json(roles_module.parse_role(case_input.get("role")))


def _vocabulary(_label: str, case_input: dict[str, Any]) -> Any:
    return vocabulary_module.closed_set_to_json(case_input.get("set"))


def _semver(label: str, case_input: dict[str, Any]) -> Any:
    if "satisfies" in label:
        return semver_module.satisfies_to_json(case_input["version"], case_input["range"])
    if "compare" in label:
        return semver_module.compare_to_json(case_input["a"], case_input["b"])
    raise KeyError(f"no semver handler for `{label}`")


HANDLERS: dict[str, Handler] = {
    "roles": _roles,
    "vocabulary": _vocabulary,
    "semver": _semver,
}


def run_corpus(root: Path) -> Report:
    """Run every case and report. Never raises for a failing case — that is what the report is."""
    report = Report()
    per_corpus: dict[str, list[bool]] = {}

    for corpus_file in load_corpus(root):
        handler = HANDLERS.get(corpus_file.corpus)

        if handler is None:
            report.unimplemented += len(corpus_file.cases)
            if corpus_file.corpus not in report.missing_handlers:
                report.missing_handlers.append(corpus_file.corpus)
            continue

        outcomes = per_corpus.setdefault(corpus_file.corpus, [])

        for case in corpus_file.cases:
            try:
                produced = canonical(handler(corpus_file.label, case.input))
            except Exception as error:  # noqa: BLE001 — a raising case is a failing case, reported
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
                        reason=f"{type(error).__name__}: {error}",
                    )
                )
                continue

            if produced == case.expected:
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
                    produced=produced,
                    diverged_at=_first_difference(case.expected, produced),
                )
            )

    report.complete = sorted(kind for kind, outcomes in per_corpus.items() if all(outcomes))
    return report


def _first_difference(expected: str, produced: str) -> int:
    left = expected.encode("utf-8")
    right = produced.encode("utf-8")
    for index in range(min(len(left), len(right))):
        if left[index] != right[index]:
            return index
    return min(len(left), len(right))

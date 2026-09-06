#!/usr/bin/env python3
"""Run the conformance corpus against the Python edition.

    python editions/python/run_corpus.py [--corpus-root corpus] [--verbose]

Exit code 0 only when every case in every corpus **that has a handler** passed, and the summary
always says how many cases are not implemented — split into the corpora with no handler at all and
the individual files a handler declined. Those facts together are the honest sentence about this
edition, and none of them is useful without the others: §31.4 says an edition that has not passed
the corpus is not an edition, so "8 of 31 corpora, all passing" has to be as visible as the zero
failures, and a corpus this edition answers *part* of has to be visibly neither.

``--min-passed`` is the ratchet, and it exists because everything above is a **floor of zero**. An
edition whose handler was deleted answers nothing, fails nothing, and exits 0: the cases move from
*passed* into *not implemented*, which is the bucket that reads as honest progress. That is the one
regression this runner's own design cannot see, so the number a CI step passes here is the number
this edition has reached, and lowering it is a decision somebody makes in a diff.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from skynet_os.corpus import run_corpus  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--corpus-root",
        default=None,
        help="Directory holding the corpus (default: `corpus/` beside the repository root).",
    )
    parser.add_argument("--verbose", action="store_true", help="Print every failure in full.")
    parser.add_argument(
        "--json",
        action="store_true",
        help=(
            "Print the summary as one JSON object instead of the human report. Used by "
            "`tools/edition-report.mjs`, which assembles what the Hub renders at "
            "/skynet/os/editions: a number on that page has to come from a run rather than from "
            "somebody typing it into a template."
        ),
    )
    parser.add_argument(
        "--min-passed",
        type=int,
        default=None,
        help="Fail if fewer than this many cases passed. The ratchet — see the module docstring.",
    )
    args = parser.parse_args()

    root = (
        Path(args.corpus_root)
        if args.corpus_root
        else Path(__file__).resolve().parents[2] / "corpus"
    )
    if not root.is_dir():
        print(f"no corpus at {root}", file=sys.stderr)
        return 2

    report = run_corpus(root)

    if args.json:
        import json

        # Deliberately the same fields the human report prints, and no derived ones: a summary that
        # computed a percentage here and a page that computed it differently would be two answers
        # to one question.
        json.dump(
            {
                "passed": report.passed,
                "failed": report.failed,
                "unimplemented": report.unimplemented,
                "total": report.total,
                "complete": sorted(report.complete),
                "partial": sorted(report.partial),
                "missing_handlers": sorted(report.missing_handlers),
                "declined_files": sorted(report.declined_files),
            },
            sys.stdout,
            indent=2,
            sort_keys=True,
        )
        print()
        return 1 if report.failed else 0

    print(f"corpus     {root}")
    print(f"result     {report.summary()}")
    if report.complete:
        print(f"complete   {', '.join(report.complete)}")
    if report.missing_handlers:
        print(f"no handler {', '.join(sorted(report.missing_handlers))}")
    if report.declined_files:
        print(f"declined   {', '.join(sorted(report.declined_files))}")
    if report.partial:
        # Named separately from `complete` on purpose: these are corpora this edition answers part
        # of, and a reader who saw only the passing files would take them for finished ones.
        print(f"partial    {', '.join(sorted(report.partial))}")

    if report.failures:
        print()
        shown = report.failures if args.verbose else report.failures[:10]
        for failure in shown:
            print(failure.describe())
            print()
        if len(shown) < len(report.failures):
            print(f"... and {len(report.failures) - len(shown)} more (pass --verbose)")

    if args.min_passed is not None and report.passed < args.min_passed:
        print()
        print(
            f"regression: {report.passed} passed, and this edition had reached "
            f"{args.min_passed}. Cases that stop being answered are counted as *not implemented* "
            "rather than as failures, so nothing above this line went red — which is why the "
            "floor is checked separately from the failures."
        )
        return 1

    return 1 if report.failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

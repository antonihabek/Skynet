#!/usr/bin/env python3
"""Run the conformance corpus against the Python edition.

    python editions/python/run_corpus.py [--corpus-root corpus] [--verbose]

Exit code 0 only when every case in every corpus **that has a handler** passed, and the summary
always says how many cases are not implemented — split into the corpora with no handler at all and
the individual files a handler declined. Those facts together are the honest sentence about this
edition, and none of them is useful without the others: §31.4 says an edition that has not passed
the corpus is not an edition, so "8 of 31 corpora, all passing" has to be as visible as the zero
failures, and a corpus this edition answers *part* of has to be visibly neither.
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

    return 1 if report.failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

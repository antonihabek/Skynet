# The C++ edition

> **It passes the corpus — every case in every corpus.** §31.4: *"An edition that has not passed the
> corpus is not an edition."* This one does. The figure is deliberately not written here: the runner
> prints it on every run, and `reference/reports/editions.json` holds it, regenerated and checked in
> CI. A README that states a passing number is a claim with nothing behind it (finding 158).

§31.3 gives C++ a **Certified** ceiling and a **Full** standing — *"the largest population in
robotics; ROS 2 is C++ and every vendor SDK is."* This is §42's **L2 — Contracts**: every contract in
`corpus/`, computed natively and held byte for byte to the oracle. It is not a kernel (L4) and not a
runtime (L3), and it says so rather than implying either.

## Build and check

```bash
make -C editions/cpp check
```

`check` is three things, all blocking:

| Step | What it holds |
|---|---|
| `json_roundtrip` | Every `expected` string in the corpus parses and re-serialises to identical bytes — the canonical writer, proven before anything is built on it |
| `self_check` | The runner's four-bucket accounting over synthetic corpora; the case tables against 219 whole strings the reference's engine converted; the JavaScript helpers at their edge inputs; and a scan refusing locale-dependent `<cctype>` calls in `src/` |
| the corpus | Every case, at the ratchet `MIN_PASSED` in the `Makefile` |

Warnings are errors (`-Wall -Wextra -Wpedantic -Wconversion -Wshadow -Werror`), objects build in
parallel with compiler-recorded header dependencies, and the runner links statically so it runs
wherever it is copied. On Windows, `mingw32-make` is the same Makefile.

## How it was written

From `reference/typescript/*` only — never from `corpus/`, which would agree by construction, and
never from the Python edition, which would prove only that one hand did both. Every contract file
opens with the places where a line that reads the same in both languages means something different;
`include/skynet_os/js.hpp` is the one module that holds those transcriptions.

## The generated file

`src/unicode_case.inc` (and `tests/case_vectors.inc`) are the full `toUpperCase`/`toLowerCase`
mappings and final-sigma properties, **read out of the engine the reference runs on** by
`tools/unicode-case.ts`. C++ has no standard facility that maps `ß` to `SS` or U+212A to `k`, and
both reach the corpus. The files record their Unicode version; regenerate them with

```bash
node tools/unicode-case.ts
```

and CI checks them under the Node that produced them (`case-tables`). The generator is shared
with the Go edition, which needs the same tables for the same reason: one derivation, two renderings.

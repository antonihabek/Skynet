# The C# / .NET edition

> **This edition is partial, and the runner is what says so.** §31.4: *"An edition that has not passed the
> corpus is not an edition."* This one has not, yet. The figure is deliberately not written here — the runner
> prints it on every run, and `reference/reports/editions.json` holds the figures for the editions that are
> complete, which this is not (finding 158).

§31.3 gives C# / .NET a **Guest** ceiling and a **Community** standing — *"GC. Windows-based cells and existing
industrial tooling."* This is §42's **L2 — Contracts**.

The Guest ceiling is a property of a garbage collector, not a judgement about C#: a collector cannot promise a
clamp inside a 1 ms deadline, so this edition is an **observer**. It computes the same verdicts as the Rust
edition and enforces none of them.

## Build and check

```bash
make -C editions/csharp check
```

| Step | What it holds |
|---|---|
| `round trip` | Every `expected` string in the corpus parses and re-serialises to identical bytes |
| `self check` | 117 checks: rule 3 at its edges, rules 2, 4 and 5 as facts, the documents the parser must refuse, the runner's four-bucket accounting, 219 whole strings the reference's engine case-converted, the fused-multiply-add ban, and the closed-vocabulary table |
| `corpus` | Every case this edition answers, at the ratchet `MIN_PASSED` in the `Makefile` |

`TreatWarningsAsErrors` lives in `SkynetOs.csproj` rather than in the `Makefile`, so an editor and a CI step see
the same gate: a rule only the Makefile enforces is a rule that passes in an IDE and fails in CI, which teaches a
contributor to distrust the gate rather than the code. No `PackageReference`: §31.4 requires byte-identical
output, so the number conversion and the duplicate-key rule have to be ours, and a build that needs a package
index is one that cannot run on the machine an edition is being reviewed on.

## `ToString("R")` has the right digits and the wrong shape

This is the one place where .NET is *better* off than the JVM, and it changes the implementation. `Double.toString`
in Java gives `4.9E-324` for the smallest subnormal where `5E-324` round-trips, so the Java edition cannot use its
library's renderer at all and runs a precision loop. .NET Core 3.0 onwards, `"R"` **is** the shortest decimal that
round-trips — so this edition takes its digits and reshapes them, and runs no search.

What it still has to fix is the shape, in two directions:

| Input | `ToString("R")` | Rule 3 |
|---|---|---|
| `double.Epsilon` | `5E-324` | `0.000…0005` — the same digit, positionally |
| `1e-7` | `1E-07` | `0.0000001` |
| `1e-6` | `1E-06` | `0.000001` |
| `1e20` | `1E+20` | `100000000000000000000` |
| `1e21` | `1E+21` | `1000000000000000000000` |
| `-0.0` | `-0` | `0` |

Every row is asserted in `selfcheck` — both what the framework writes and what rule 3 requires — so a reader who
wonders why `FormatNumber` reshapes a string it already trusts finds the answer in a test rather than a comment.

Note the third and fourth rows against the Java table: .NET turns exponential at a *narrower* band than Java does,
so an edition that reshaped only the two exponents the Java edition has to reshape would be wrong here.

## `Trim()` is the exact inverse of ECMAScript's on both characters that matter

Not a subset, not a superset — the inverse, on precisely the two characters that distinguish the sets:

| Character | ECMAScript whitespace | `char.IsWhiteSpace` |
|---|---|---|
| U+FEFF zero-width no-break space | **yes** | no |
| U+0085 next line | no | **yes** |

The corpus turns on both directions: a semver range written with a byte-order mark before its version, and a
predicate case whose string is blank to one set and not to the other. `selfcheck` asserts the framework's answer
beside this edition's on each, so the difference is a recorded fact rather than a claim.

## `ToUpperInvariant()` is the *simple* mapping, and there is no final sigma at all

One code point in, one out, no context. So the sharp s is **unchanged** where the reference writes `SS`, and
`"ΟΣ".ToLowerInvariant()` gives a medial sigma where the reference gives a final one. This is not a version
disagreement of the kind the Java edition has — it is a different function. The tables therefore come from
`tools/unicode-case.ts`, read out of the engine the reference runs on, like every other edition's.

`InvariantGlobalization` is set in the project file besides, so a locale cannot reach `ToUpper()` or
`double.ToString()` at all. The property is there to make a locale-dependent call impossible rather than merely
unused.

## Rule 2 is not `string.CompareOrdinal`

Keys sort by their UTF-8 **bytes**. `CompareOrdinal` compares UTF-16 code units, and the two disagree above the
basic multilingual plane: an astral character is a surrogate pair, and a surrogate code unit (0xD800…) sorts
*below* U+E000…U+FFFF while its UTF-8 encoding (0xF0…) sorts *above*. So `CompareOrdinal` puts an astral key first
and rule 2 puts it last. `selfcheck` asserts both orders on the same pair of keys.

## `char.ConvertToUtf32` throws on a string the oracle answers

A JavaScript string is a sequence of UTF-16 code units and may hold an **unpaired surrogate**; the reference's
`codePointAt` returns it. `char.ConvertToUtf32` raises `ArgumentException` instead, and `char.ConvertFromUtf32`
refuses to write one back. So every walk over a string in `Js` goes through its own `CodePointAt` and
`AppendCodePoint`, which answer the code unit itself when there is no pair. Without that, a lone surrogate
anywhere in a role or a version string is an exception where the oracle answers `null` — a red corpus case caused
entirely by the host library. Three checks in `selfcheck` cover it.

## `System.Text.Json` is not used, and the surrogate rule is one of the reasons

It rejects a document whose `\uXXXX` escape is an unpaired surrogate, which the reference accepts, and its
`GetDouble` is not the engine's conversion. A parser that refused documents the oracle can write would fail the
round trip on the corpus's own files.

## No fused multiply-add, enforced by a scan with a positive control

The C, C++ and Zig editions switch contraction off at the compiler and then scan the object code, because those
compilers may fuse on their own. **The CLR may not** — its `mul` and `add` on float64 are separate IEEE-754
operations — so there is no flag here and nothing to find in the IL.

What there *is* on this platform is `System.Math.FusedMultiplyAdd`, which fuses when a caller asks for it. §31.2
pins the kernel's arithmetic as a fixed sequence of correctly rounded operations, and a fused operation rounds
once where that sequence rounds twice. So the rule is that nothing asks, and `selfcheck` scans the sources to say
so — **with a positive control**, which is finding 171: a scan that finds nothing proves nothing unless something
would have been found. The control is arithmetic rather than a compiled probe:

```
a = 0.1, b = 0.2, c = -(a * b)
(a * b) + c                     == 0                        — the stepwise sequence cancels exactly
Math.FusedMultiplyAdd(a, b, c)  == -1.6653345369377347e-18  — the fused one keeps the bits rounding away
```

If a future runtime stopped fusing that call, the check fails rather than quietly passing for the wrong reason.

## One project, three subcommands

`roundtrip`, `selfcheck` and `corpus`, dispatched in `src/Program.cs`. .NET allows one entry point per assembly,
and three assemblies sharing a source tree is three project files to keep in step for no reader's benefit.

The `Makefile` runs the built `.dll` directly rather than through `dotnet run`, and that is a correctness fix
rather than a speed one: **`dotnet run` forwards any option it does not recognise to the program**, so `--nologo`
arrived as this edition's first argument and the runner answered with its usage line. Right of the runner, and a
trap in the Makefile.

## What MSBuild decides that no other edition's build does

**An XML comment cannot contain two consecutive hyphens.** `SkynetOs.csproj` therefore cannot quote a `dotnet run
--` command line at all — `MSB4025: An XML comment cannot contain '--'` — so the invocations live in the
`Makefile` and the project file points at it. This is the third per-language comment rule the editions have run
into: a C comment cannot contain the two characters that open one, a Java comment cannot contain a unicode
escape, and an XML comment cannot contain a long option.

`AnalysisLevel` is `latest-recommended` rather than `latest-all`. The difference is not strictness about
correctness — every analyser that reports a defect is on either way — it is that `all` enables the API-design and
performance advisories, several of which are wrong about this code on purpose: the value model exposes a mutable
list because a contract builds documents by appending, and the corpus runner catches broad exceptions because
finding 164 says a runner that crashes describing a failure hides the failure. Choosing the analyser set is a
decision; suppressing a rule that found something would not be. The one suppression in the tree is exactly that
broad catch, `#pragma`-scoped to the four lines it covers and justified where it sits.

## How it was written

From the modules under `reference/typescript` only — never from `corpus/`, which would agree by construction, and
never from the Python, C++, Go, C, Zig or Java editions, which would prove only that one hand did both. One class
under `src/contracts/` per module there, named for it. `Dispatch` is the only one with no counterpart: the
reference has no such table, because in TypeScript the corpus generator calls the functions directly.

There is a second class with no counterpart: `contracts/Vocabulary`, the seventy-four closed string sets.
They are declared across the reference's modules as exported constants rather than computed by one of them,
so this edition holds them as its own table. A collection initialiser with a duplicate key throws at type
initialisation, which is the same gate the Java edition spells with an explicit `put` — a duplicate would
replace the earlier set in place and the corpus would still pass on whichever of the two survived.

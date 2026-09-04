# The Python edition

> **This is not yet an edition.** §31.4: *"An edition that has not passed the corpus is not an
> edition. It cannot carry the name, appear in the matrix, or be installed by `skynetd`."* It
> currently passes **8 of 31 corpora**. The runner prints that number on every run and this file
> will keep saying so until it is 31.

§73's last column has carried one entry against §31 since the table was written: *"The second
edition, which is what the corpus is for."* This is the start of it.

## What a Python edition is, and the ceiling it declares

§31.3's matrix gives Python a **guest** ceiling and no kernel:

> GIL and GC. Cannot host an enforcing kernel and should never claim to. Remains the best edition
> for simulator drivers, bench harnesses, research and the SDK.

The three rules that keep the matrix honest apply here in full:

1. **An edition below its ceiling reports advisory enforcement.** This edition is an *observer*. It
   computes the same verdicts as the Rust edition and enforces none of them, and it says so in those
   words on every surface it ever grows.
2. **An edition may not raise its own ceiling.** Guest is a property of the language's runtime — a
   garbage collector cannot promise a clamp inside a 1 ms deadline — not a configuration flag.
3. **A ceiling is not a judgement about the language.** Python's ceiling is about a GC, not about
   Python.

## Running the corpus

```
python editions/python/run_corpus.py
python editions/python/run_corpus.py --verbose      # every failure in full
```

No dependencies beyond the standard library, deliberately: an edition whose conformance run needs a
package index is an edition that cannot be checked on the machine it is being reviewed on.

## What passes today

| Corpus | Cases | |
|---|---|---|
| `predicate` | 83 | §54.2's three-valued outcome, and the boolean derived from it |
| `vocabulary` | 73 | every closed set in the system |
| `semver` | 61 | version ordering and range satisfaction |
| `roles` | 38 | the canonical role grammar |
| `digest` | 19 | §37.1's version identity, framing included |
| `frames` | 27 | §44.3's graph rules, and the joint names they cannot check |
| `kinematics` | 14 | forward kinematics, poses to thirty digits |
| `watchdog` | 13 | §46's chain, the host refusal, and window nesting |
| | **328** | of 1,055 |

The other 23 corpora have no handler and their 727 cases are counted as such in every run. They are
not skipped and not hidden: a runner that quietly passed eight corpora and printed "OK" would be the
most misleading artifact in this repository.

## Where the values come from

**From `reference/typescript/`, never from `corpus/`.**

This is the whole discipline of §31.4 and it is easy to lose. A corpus exists to catch a divergence
between an edition and the reference; an edition generated *from* the corpus agrees with it by
construction and proves nothing. So every constant here is transcribed from the reference edition's
own source, and the corpus then judges the transcription.

It earned its keep twice on the first day:

* A first pass extracted `reserved_capabilities` as **eight** members from a regex that had run past
  the end of a declaration. The real value is one token, `hri.warning`, and both the Rust and
  TypeScript editions say so. A corpus-generated table would have had eight and been wrong in the
  same direction as the tool that built it.
* The `large-major` semver case failed on the first run, and the divergence was not a typo: **Python
  has arbitrary-precision integers and the specification is IEEE-754.** `int("9007199254740993")` is
  exact, `Number("9007199254740993")` rounds, and this edition ordered two versions the reference
  calls equal — while *serialising* them identically, because canonical form renders both as the
  rounded value. A disagreement about ordering that is invisible in the output is the worst shape a
  divergence can take, and nothing but a byte-for-byte corpus would have found it.

## Layout

```
skynet_os/
  canonical.py     canonical JSON — the rule every other module's output passes through
  vocabulary.py    all 73 closed sets
  roles.py         §5's role grammar
  semver.py        §54.2's version ordering and ranges
  predicate.py     §54.2's evaluator: satisfied, unsatisfied, and unevaluable
  digest.py        §37.1's version identity
  embodiment.py    §44.3's frame graph, and the joint names the graph cannot check
  kinematics.py    forward kinematics, and the sine, cosine and square root it is written on
  watchdog.py      §46's chain and its window nesting
  corpus.py        the runner and its dispatch
run_corpus.py      the CLI
```

`canonical.py` is a deliberate third implementation of a rule that already exists in
`crates/skynet-json` and `reference/typescript/canonical.ts`, and the duplication is the point: a
corpus whose expected values were produced by the code that checks them would prove nothing.

## What the second round found

Four transcriptions, and each carried a way for Python to be quietly wrong that the reference gets
free from its own language. All four are written down where they are handled rather than here.

* **`predicate`** — `True == 1` is true in Python and `true === 1` is false in JavaScript, so
  `_strict_equal` compares types first. And the comparison operators coerce strings the way
  `Number()` does, not the way `float()` does: `0x1f` is 31, `1e3` is 1000, a blank string refuses,
  and `Infinity` parses and is then discarded as non-finite. The corpus carries a case for each.
* **`digest`** — paths are ordered by **UTF-16 code unit**, which is not Python's code-point order
  for anything above U+FFFF. A naive edition would hash a file list in a different order and produce
  a different digest rather than an error.
* **`kinematics`** — `math.trunc` returns an arbitrary-precision `int` where `Math.trunc` returns a
  double, so the range reduction converts back immediately. All fourteen poses match to thirty
  digits, which is what holds two hand-written implementations of sine, cosine and square root to
  each other.
* **`watchdog`** — a window declared as `True` is a finite `int` in Python and would nest inside
  every real window, reporting a chain as sound on the strength of a value that is not a duration.

One of them was a mistake this file had already warned about. `digest`'s separator is a NUL, and the
reference's own header explains that writing it by eye is how the TypeScript edition came to use a
space. This edition then wrote a **raw** NUL — correct, and invisible in every editor and terminal.
It is `_NUL = chr(0)` now, and named, which is the fix the reference's comment was asking for.

## What is next, in dependency order

The four that carry the weight — `cycle` (§7's eight guards), `transport` (§6), `admission` (§8)
and `certification` (§15) — and then the rest. Everything after the foundation is mechanical; the
foundation is canonical serialisation and a runner that refuses to lie about what it has not done.

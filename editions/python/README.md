# The Python edition

> **This is not yet an edition.** §31.4: *"An edition that has not passed the corpus is not an
> edition. It cannot carry the name, appear in the matrix, or be installed by `skynetd`."* It
> currently passes **13 of 31 corpora**. The runner prints that number on every run and this
> file will keep saying so until it is 31.

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
python editions/python/self_check.py                # the runner's own accounting
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
| `limits` | 19 | §7.6's three composition rules, and the guards a body cannot run |
| `cycle` | 35 | the eight guards in evaluation order, pose guards included |
| `fuzz/cycle` | 40 | seeded vectors through the same path |
| `observation` | 18 | §25.1's frame assembly, its target instant, and §26's record |
| `fuzz/observation` | 48 | seeded vectors through the same path |
| `transport` | 44 | §6's planner: binding, scaling, retargeting, rate and observations |
| `fuzz/transport` | 60 | seeded vectors through the same path |
| | **592** | of 1,061 |

The other 18 corpora have no handler at all — 469 cases, counted in every run. They are not
skipped and not hidden: a runner that quietly passed ten corpora and printed "OK" would be the
most misleading artifact in this repository.

### `partial` is a state, and `fuzz` was in it

`fuzz` is one corpus over three files, and it is `complete` now that all three are written.
It spent three rounds `partial`, which is the state the runner exists to be able to report:
without it, implementing one file of three would have counted the other 108 cases as *wrong
answers* rather than as *untranscribed*, and the number would have got worse precisely as the
edition improved. A corpus is complete or it is not, however many of its files are done.

`self_check.py` holds the runner to this against synthetic corpora with known answers: **27
checks**, of which four test the detector and twelve test the language traps below.

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
  safety.py        §7's eight guards, §7.6's limit resolution, and the per-cycle verdict
  observation.py   §25.1's frame assembly, and the record §26 keeps of it
  transport.py     §6's planner: how a policy's action space reaches a body
  corpus.py        the runner and its dispatch
run_corpus.py      the CLI
self_check.py      the runner's own accounting, and the traps the corpus cannot reach
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

## What the third round found: the traps a corpus cannot reach

`safety.py` was transcribed and passed all 94 of its cases on the first run, which is a claim that
has to be earned rather than reported. Five mutations of it broke between 1 and 17 cases each.

**A sixth broke nothing**, and that is the finding. `typeof true === 'number'` is false in
JavaScript and `isinstance(True, int)` is true in Python, so an envelope declaring
`joint_torque_max_nm: true` clamps every joint to **1 Nm** in a naive transcription and is
discarded outright by the reference — and no corpus case reaches it.

That is not a gap in the corpus, and the fix is not to add a case. Every edition must pass the
corpus and **Rust cannot represent the input at all**: `joint_torque_max_nm` is an `Option<f64>`
and serde refuses `true` before any rule runs. These traps exist only for a dynamically typed
edition, so they live in `self_check.py`, with expectations read from `safety.ts`:

| Trap | The reference, versus a naive transcription |
|---|---|
| a boolean limit | Discarded, so 80 comes from the transport clamp — against **1 Nm on every joint** |
| `self_collision_margin_m === undefined` | Strict. Present-and-null emits **no** notice and still clamps at zero clearance; `.get` collapses the two |
| `capability !== undefined` | Absent means the runtime did not say, and the gate does not evaluate. Present-and-null means it said null, which is not a granted token — so the gate **trips** |
| `Object.entries` | Array-index keys first in ascending order, *then* insertion order. `"2"` clamps before `"10"`, and `"09"` is not an index at all |
| `Math.sign` | Returns `-0` for `-0` and `NaN` for `NaN`. `copysign` maps `NaN` onto `±1`, turning an unrepresentable command into a confident clamp |
| `null > number` | Coerced to `0 > number`. Transcribed rather than tidied — a sensible reading of the specification is a divergence from it |

## What the fourth round found: two gaps in the corpus itself

`observation.py` also passed all 64 of its cases on the first run, and mutating it found
something better than a bug in the transcription — **two behaviours the reference documents in
prose and the corpus could not tell apart**. Neither is a Python-only trap; both are
expressible in every edition, so both became corpus cases rather than notes.

| Mutation | Before | After |
|---|---|---|
| half-away-from-zero replaced with Python's half-to-even `round` | **0 cases** | 1 |
| the staleness budget made two-sided | **0 cases** | 1 |

The first is the sharper one. The reference's own comment says two editions rounding a
half-nanosecond differently is *"a different chosen sample the day two candidates are
equidistant"* — and no case in the corpus had a value that landed on a half, including
`sub-millisecond-latency`, whose note claims to pin exactly this and whose 0.35 ms multiplies
out to 350000.00000000006. `exactly-half-a-microsecond` lands on 0.5 and 2.5 µs, where
half-away gives 1 and 3 and half-to-even gives 0 and 2.

The second: the budget bounds how far *behind* the target a sample may be, and a sample from
ahead of it is a clock fault rather than a stale one. Every existing case with a negative age
had one *inside* its budget, so a two-sided reading agreed with the one-sided one everywhere.

The corpus is 1,055 cases → **1,057**, and the Rust edition reproduces both new ones byte for
byte without a change: they pin behaviour that was already correct in both editions and
unprotected in both.

### And one divergence in the reference, left alone deliberately

`roundHalfAway` is defined **twice** in `reference/typescript/` — in `kinematics.ts` as
`Math.trunc(v ± 0.5)` and in `observation.ts` as a fraction test — and the two disagree at the
largest double below a half, where the sum rounds up to exactly `1.0`:

| input | `kinematics.ts` | `observation.ts` |
|---|---|---|
| `0.49999999999999994` | 1 | 0 |
| `-0.49999999999999994` | −1 | 0 |
| `NaN`, `±Infinity` | `NaN`, `±Inf` | 0 |

This edition transcribes **each file's own version**, because a transcription that quietly
unified them would be an edition disagreeing with the reference in order to be right. Neither
input is reachable from a real latency, and it is still one function with two behaviours —
the shape of defect §7.4's redundant channel exists to catch. Unifying it is a change to the
TypeScript, the Rust and the corpus together, and belongs in its own round.

## What the fifth round found: six more gaps, and a defect all three editions shared

`transport.py` passed all 100 of its cases first run. Ten mutations; **four caught, six not**.
The largest catch is worth stating on its own: walking a warning's `subjects` as a Python
`set` instead of in insertion order breaks **51 cases**. A Python `set` has no promised order
at all, so that edition would produce correct plans whose warnings are shuffled *differently
between runs of the same input*.

Four of the six became corpus cases, because every edition can express them:

| New case | The rule it pins |
|---|---|
| `duplicate-role-first-wins` | A body declaring one role on two joints keeps the **first**. Last-wins produced identical bytes everywhere |
| `scaled-to-an-exact-half` | A 128 Nm peak on an 80 Nm rating is exactly 0.625, so the warning reads `63%`. `toFixed(0)` takes the larger integer on a tie; half-to-even gives `62` |
| `sensor-declares-no-width` | `candidate.width ?? 0` makes an undeclared dimension a **zero**, not a match — so the stream downscales rather than passing as direct |
| `zero-watchdog-is-no-watchdog` | A watchdog of zero yields no `min_control_hz` and no `rate_below_watchdog`. The `> 0` guard is in two places and neither was covered |

The other two stayed here, for the same reason §19's did: Rust's `allowRetarget` is an
`Option<bool>` and cannot hold a truthy non-boolean, and serde reads an absent field and an
explicit null into one `None`.

### The defect a corpus is structurally unable to catch

`duplicate-role-first-wins` was the first case in the corpus with exactly **one** unmapped
joint, and it printed:

> `1 joint on this body **are** not driven by the policy`

The plural was on the noun and not on the verb. Seventeen existing cases carried it, and every
edition reproduced it byte for byte, because **all three had the same bug**. A corpus detects
editions disagreeing; it is structurally unable to detect them agreeing about something wrong.
Fixed in all three — the reference first, then the Rust, then this edition — and the corpus
regenerated, which is the only reason the change is visible at all.

## What is next, in dependency order

`admission` (34) and `certification` (18) — the last two of the four §31.3 calls the ones that
carry the weight. Then `skill` (85), `identity` (42), `opstate` (36) and the rest. Everything after the foundation is mechanical; the foundation is canonical
serialisation and a runner that refuses to lie about what it has not done.

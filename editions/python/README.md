# The Python edition

> **This is not yet an edition.** §31.4: *"An edition that has not passed the corpus is not an
> edition. It cannot carry the name, appear in the matrix, or be installed by `skynetd`."* It
> currently passes **28 of 31 corpora**. The runner prints that number on every run and this
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
| `admission` | 36 | §8's install gate: signatures, budgets, grants and §49.1's reserved token |
| `certification` | 20 | §15's award: absence is not a pass, and the evidence caps the claim |
| `skill` | 86 | §54.2–54.5: preconditions, success, timeout, composition and the lifecycle |
| `opstate` | 37 | §27's machine, and the two predicate tables it is read through |
| `identity` | 44 | §10, §19 and §50: pairing codes, credentials and what is running |
| `transfer` | 32 | §12 and §79's four decisions: fetch, arrival, upload, receipt |
| `power` | 28 | §36's four states plus the fifth, and the faults beside them |
| `ota` | 29 | §12's second and third rules: the install gate and health-gated rollback |
| `birth` | 11 | §50's vendor certificate, and the key it is about |
| `intent` | 11 | §11's pull, and the two refusals that are not symmetric |
| `readiness` | 12 | the tier a registry subject can attain, and what blocks the next one |
| `checkin` | 25 | §17's claim, §33.6's age rule, and the document that crosses the boundary |
| `cell` | 28 | §55.2's descriptor: every fault in it, and the two projections read out of it |
| `blackbox` | 21 | §26's record layout, the ring header, the six schemas, and a sealed file |
| `calibration` | 31 | §45's validity, the guards a set feeds, and which sets a body can hold |
| | **1,043** | of 1,091 |

The other 3 corpora have no handler at all — 48 cases, counted in every run. They are not
skipped and not hidden: a runner that quietly passed most corpora and printed "OK" would be the
most misleading artifact in this repository.

### `partial` is a state, and `fuzz` was in it

`fuzz` is one corpus over three files, and it is `complete` now that all three are written.
It spent three rounds `partial`, which is the state the runner exists to be able to report:
without it, implementing one file of three would have counted the other 108 cases as *wrong
answers* rather than as *untranscribed*, and the number would have got worse precisely as the
edition improved. A corpus is complete or it is not, however many of its files are done.

`self_check.py` holds the runner to this against synthetic corpora with known answers: **41
checks**, of which four test the detector and the rest test the language traps below.

Both editions run on every commit. §31.4 asks for differential checking in CI and until the
ninth round the `corpus` job had one row — this edition passed nowhere but on the machine of
whoever last touched it. It has two rows now, and the second carries `--min-passed`, because
every other check here has a floor of zero: an edition whose handler was deleted answers
nothing, fails nothing, and exits 0, since the cases move into the bucket that reads as honest
progress.

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
  admission.py     §8's install gate, and §49.1's one reserved capability
  certification.py §15's award, and the evidence that caps it
  skill.py         §54's five sections: preconditions, success, timeout, composition, lifecycle
  opstate.py       §27's machine, which four callers described in prose before it was declared
  identity.py      pairing codes, device credentials, and §10's attestation sentence
  transfer.py      §12's fetch and §79's upload, and what each check means
  power.py         §36's states, reserve, thermal ladder and faults
  ota.py           §12's install gate and its health-gated rollback
  birth.py         §50's vendor certificate, and what a robot may conclude from one
  intent.py        §11's pull: may this response become the desired state
  readiness.py     the tier a subject can attain, and the blockers between it and the next
  checkin.py       §17's claim, §33.6's health, and the document that crosses to the Hub
  cell.py          §55.2's descriptor, its faults, and the two projections a robot reads
  blackbox.py      §26's bytes: the ring record, the header, the schemas and the MCAP file
  calibration.py   §45's validity, expiry and the guards a parameter set feeds
  js.py            the four JavaScript expressions Python has no equivalent for
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

The largest catch is worth stating on its own, because the *number* is the finding. Walking a
warning's `subjects` as a Python `set` rather than in insertion order breaks — six identical
runs of the identical mutated code:

```
41, 44, 48, 52, 58, 69 cases
```

CPython randomises string hashing per process, so a `set` there does not merely order the
warnings wrongly: it orders them **differently on every start**. An edition that reached for
the obvious type would produce entirely correct plans whose warnings shuffle between runs,
and a suite that happened to pass once would fail the next time nobody changed anything.

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

## What the sixth round found: a bug in the Rust edition, and a question for the owner

`admission.py` and `certification.py` completed the four §31.3 calls *the ones that carry the
weight*. Both passed first run; sixteen mutations between them, and three of the survivors
became corpus cases. Two of those three found something bigger than a gap.

### The Rust edition awarded a tier on a stale pass

`duplicate-results-keep-the-last` reports one test twice — a pass, then the failing re-run that
replaced it. The reference builds a `Map`, so the **last** wins and the award is refused.
`skynet-contracts` used `.iter().find(..)`, which is **first** wins, and awarded `guest` on the
stale pass.

That is the same answer on every input where a test is reported once, which is every input the
corpus had. A vendor re-running a failed conformance test and submitting both results would have
been certified by the edition that runs **on the robot**. Fixed in
`crates/skynet-contracts/src/certification.rs`.

### A version comparison the two editions answer differently — left open

§8.2's `min_os_version` gate splits on `.` and compares segments as numbers. The reference maps
`Number`, so `"0-rc1"` is **NaN** — and `NaN < 0` is false, so the robot reads as *newer* and the
package installs. `skynet-contracts` parses with a fallback of **0**, so the same robot reads as
*older* and it is refused. A robot on `1.2.0-rc1` is told by the Hub that a package needing
`1.2.1` will install, and refused it by the robot.

This one is **not** fixed, and deliberately. The two answers are not equally safe: the reference
**fails open**, so any robot with a non-numeric segment satisfies *every* `min_os_version` —
which is the one thing that field exists to prevent. Making the Rust edition agree would ship
that permissiveness to the robot; making the reference agree is a specification change that the
Hub's mirrored copy inherits. It is recorded in §80 as an owner's decision, and
`os-version-with-an-empty-segment` pins the neighbouring case the two **do** agree on, so the
agreement cannot rot while the disagreement is open.

### And a hazard in the mutation harness itself

A mutation whose replacement has the *same byte length* as the original can leave a stale
`__pycache__` after the revert — CPython keys the cache on `(mtime, size)`, and a revert inside
the same second changes neither. It produced one false failure here, against code that was no
longer on disk. Every mutation figure in this file was re-measured with the cache cleared before
each run: **34 of 34 caught**.

## What the seventh round found: the default the module warns about, untested

`skill`, `opstate` and `identity` — 163 cases, all three first run. Thirty-nine mutations,
**thirty caught**, and the sharpest survivor is one sentence away from where it lives.

`opstate.py`'s header says of its inputs:

> a state machine that read "nobody told me about the kernel" as "the kernel is live" would be
> **the single most dangerous line in this repository**.

Defaulting `kernel` to `live` broke **no case**. Twenty-five cases reach that rule and every one
names the kernel explicitly; the only case that omits it omits *everything*, and is refused four
rules earlier by the RHAL default. `a-kernel-nobody-observed-is-down` observes every condition
except that one.

Three more became cases — `attested` needing **both** halves, an empty scope list not being an
absent one, and `root === false` being strict, so an invocation that never said it was nested
acquires its own leases rather than running holding nothing. 1,066 → **1,069**.

### And a third divergence between the editions, left open

Eight astral characters typed as a pairing code are `too_long` to the reference — *"a pairing
code is 8 characters and this is 16"* — and `not_in_alphabet` at position 1 to the Rust edition.
`.length` counts UTF-16 code units; `chars().count()` counts code points. Both refuse, so nothing
unsafe is shipping; what is wrong is that one operator gets two sentences about one typo, and the
reference's is false as English. Recorded in §80, and the probe case **withdrawn** rather than
left red — pinning either answer would settle it by whichever edition was easier to edit.

## What the eighth round found: two boundaries nothing stood on

`transfer`, `power` and `ota` — 87 cases, all first run. Twenty-five mutations, twenty-three
caught, and both survivors were the same shape: a comparison whose boundary no case sat exactly
on.

Every fetch case had a partial well under the declared size or well over it, so `partial >= size`
was never tested at the equals — where the answer matters, because a partial that is exactly the
whole length is either a finished transfer nobody recorded or somebody else's bytes, and an
edition testing `>` alone resumes from the end of a file and sends nothing. And no bundle
declared more than one gated component, so the **order** `gated_components` is reported in — a
sentence an operator reads, and a fault's subjects — was unpinned.

1,069 → **1,071**, and Rust reproduces both without a change.

## What the ninth round found: a defect in Rust, and this edition running nowhere

`checkin` and `cell` — 51 cases, both first run. Thirty-eight mutations, **thirty caught**, and the
survivors split into the two kinds this file has been sorting them into since the third round: the
ones a typed corpus cannot express, and the ones it simply had not looked at.

Four became corpus cases. **1,071 → 1,075**, and one of them went red in Rust.

### A digest on a slot nobody read

A measure carries a `state` and a `digest`, and both are well-typed independently. Nothing had ever
put `{ state: 'unmeasured', digest: 'sha256:aaaa' }` together — a document saying nobody read the
slot and carrying a reading of it in the next field — so what a consumer sees was unpinned, and the
two editions had answered it differently for as long as the field has existed. `shape.ts` gates the
digest on the state; `skynet-contracts` passed it through.

The reference is right, and the argument is the query rather than the tidiness. §32.1 puts a fleet
column on *"which robots are running image `sha256:…`"* — the question an incident is run through —
and Rust's answer put a robot whose image slot was **never read** into that result. The prose half of
the same document already says `an unmeasured OS image (the slot could not be read)`, so passing the
digest through had one report contradicting itself in the machine-readable half, where nobody is
reading English to catch it. Fixed in `crates/skynet-contracts/src/identity.rs`.

### Three more, each a list nobody had made long enough

| New case | The rule it pins |
|---|---|
| `two-stages-left-and-the-order-nobody-pinned` | `pending` is a **second** list of `blocking`'s facts, sorted separately, and no document case had two entries. §34.4 collapses check-ins, so two documents differing only in a list's order are two a queue cannot tell are duplicates |
| `a-fixture-named-and-not-measured` | `volume: null` is a fixture nobody measured. The check is truthy for exactly this document; a key test reports an operator who wrote the field out and left it empty as one who measured it wrongly |
| `a-duplicate-fixture-measured-wrongly` | One fault, not two. The duplicate is reported and the walk **stops** — the copy that is about to be deleted is not a second thing to tell the operator about |

### And the two the corpus structurally cannot carry

A `deferred` of `1` and an unrecognised readiness word are inputs no typed edition can produce, so
they went to `self_check.py` with the rest. The cell trap is the more interesting one, because this
repository had already written down why it cannot be a case: `generate.ts` says at the frame-bindings
rule that it writes every input through `JSON.stringify`, which renders an infinity as `null` — so a
frozen case would hold a *different* document from the one its expectation was computed against,
which is worse than no case at all. A box with an infinite corner is checked here instead, beside a
coordinate of `true`, which JSON can carry and no typed edition can produce.

### The finding that was not in any transcription

**This edition was in no CI job.** §31.4 asks for differential checking on every commit and the
`corpus` job's own comment said *"One row today"* — written when that was true, and left standing
through eight rounds in which a second edition was built. A change to `reference/typescript/`, to the
corpus, or to a shared module would have broken it silently until somebody next ran it by hand. Both
editions run now, and the Python row carries a ratchet for the regression the runner's own design
cannot see: cases that stop being answered are counted as *not implemented*, which is the bucket that
reads as progress.

## What the tenth round found: a case that did not reach the rule it is named for

`blackbox` — 18 cases, first run, and it is the one corpus whose subject is **bytes**. A third
implementation of the ring record, the header, the six MCAP schemas and the sealed file produced the
identical SHA-256 for a 19,805-byte file on the first attempt, which is the strongest single piece of
evidence in this repository that §31.2's single-source list is actually single-source.

Twenty-two mutations, nineteen caught. Three survivors, and the first is the finding:

### `frame-astral-stream-name` never cuts inside a character

Its note says a cut inside a multi-byte sequence *"would put invalid UTF-8 in the ring and every
reader would have to guess"*, and the case is nine **two-byte** characters in a sixteen-byte field.
Sixteen is even, so the boundary lands exactly between the eighth and the ninth: the back-off loop
looks at a lead byte, exits without doing anything, and an edition that had never written one agreed
about every byte of the record. Deleting the loop entirely broke nothing.

`frame-a-stream-name-cut-inside-a-character` is six **three-byte** characters. The boundary falls
inside the sixth, the cut walks back to fifteen, and the field holds five characters and a NUL —
one byte shorter than it could be, which is exactly the point.

### Two more boundaries nobody had sat on

`cycle-every-guard-tripped` is all eight. `MAX_TRIPPED` is eight, the guard table is eight bytes at
offsets 104–111, and `capability` is at 112 — so the count byte reads `08`, the table reads
`0102030405060708`, and the next two bytes are `ffff`. No case had gone past **one** tripped guard.

`three-messages-on-one-channel` writes three messages to channel 6 where the existing seal case
writes one each to two channels, so the per-channel sequence was 1 on every message and a constant
produced the same file. A reader uses that number to detect a message it did not receive. The third
message is also out of order in time — 3000 ns then 2000 ns, which is what writing from a ring looks
like — so the summary's `message_start_time` is pinned as the minimum rather than the first seen.

**1,075 → 1,078**, and Rust reproduces all three without a change.

### And one inconsistency in the format, recorded rather than pinned

The last survivor cannot be closed with a case, and the reason is worth more than the mutation.
`encodeCycle` checks seven arrays against the ring's shape — `desired`, `command`, `mask`,
`clamped`, `position`, `velocity`, `effort` — and **throws** when one does not match. `tripped` is
the eighth array and is silently truncated to eight instead.

A ninth guard is not something a producer emits, so the clamp is defensive rather than reachable,
and a corpus case carrying one would freeze *truncation* as the specified answer for a malformed
cycle — when the answer the other seven arrays give is a refusal. That is a question about the
format rather than a gap in an edition, so it is written down here and in `BUILD-LOG.md` rather than
settled by whichever behaviour was easier to transcribe.

## What the eleventh round found: nine of nineteen, and the function nothing was measuring

`calibration` — 18 cases, first run, and then the **weakest mutation run of the project**: nine of
nineteen caught. Ten survivors, eight of them real, and they say something about how this corpus was
written rather than about this transcription. Every case sits comfortably inside or comfortably
outside its rule; nothing sits **on** one.

**1,078 → 1,091.** Thirteen new cases, and Rust reproduces all of them without a change.

### `setsFor` had no case at all

The two `events` cases call `setsFor(…)` and freeze its **output** into their `applicable` field.
So the function that decides which parameter sets a body can hold — the one that keeps a bolted-down
arm from reporting `balance` unavailable for want of an IMU it will never have — was measured by
nothing: two editions could disagree about it and the corpus would compare them on the answer one of
them had already given.

That is `corpus/README.md`'s own diagnosis, pointed at a function instead of a field:

> a corpus grows a blind spot wherever an *input* field is missing, not only where an
> implementation is.

`calibration/sets.json` is seven cases against it directly, and four of them reach branches nothing
had: `sensing.depth` and `sensing.force` on their own, an IMU on a fixed base, and a declared
capability that matches none of the sensing rules. The Rust adapter gained the same branch.

### Four boundaries, and two notes that claimed what their inputs did not reach

| New case | The rule it pins |
|---|---|
| `a-residual-exactly-at-its-threshold` | 1.0 against 1.0 is **valid**. `>=` fails every calibration that hit its target precisely |
| `an-age-exactly-at-the-limit` | 3,600 s against 3,600 s is valid. On a fleet recalibrating on a schedule, `>` and `>=` disagree about every robot on the day its policy comes due |
| `a-robot-that-got-colder` | `temperature-delta`'s note says *"the direction does not matter"* and every temperature case ran **warmer**. Without the absolute value, −18 °C reads as inside a 5 °C budget |
| `a-policy-with-no-timestamp` | §45's rule that a missing reading never extends validity, for the **taken** reading. `temperature-unreadable` pins it for the current one |

Two more: `an-empty-method` — §45.3's check is falsy, not a null test, and no record had ever carried
`""`, which is the shape of every form somebody tabbed past — and `an-age-on-a-rounding-tie`, half a
second, where `toFixed` takes the larger and Python's formatting takes the even one.

And `arm-without-an-imu`'s note said *"seven sets exist, four apply"*. Five apply. Corrected.

### One consolidation, and it was a latent fix

`toFixed` existed **twice** in this edition — `certification.py` through `Decimal`, and
`transport.py` as `floor(x + 0.5)` with a comment arguing the domain made it safe. It is not the
same rule: at `0.49999999999999994` the sum rounds up to exactly `1.0` in binary and the shortcut
answers `1` where the nearest integer to the value itself is `0`. That is the identical input on
which `roundHalfAway` is defined twice and two ways in `reference/typescript`, which this file has
been recording since the fourth round.

One `to_fixed` now, in `js.py`, on the `Decimal` rule. The corpus was green before and after, so
nothing reachable moved — which is exactly what makes it worth doing now rather than after somebody
finds the input.

## What is next, in dependency order

`provenance` (17 cases), `telemetry` (16), `live` (15). Three corpora, 48 cases, and none of them is a foundation any of the others needs. Everything after the
foundation is mechanical; the foundation is canonical serialisation and a runner that refuses to lie
about what it has not done.

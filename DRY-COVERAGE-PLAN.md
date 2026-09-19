# Dry coverage plan — 2026-09-16 onward

Coverage work carried out **without a physical robot**, and the mechanisms that keep the
untested half visible rather than implied. Kept beside [`BUILD-LOG.md`](BUILD-LOG.md), which is
the session record; this file is the plan and its progress log.

> **The constraint that shapes everything below.** No step here is verified against hardware.
> That is a decision the owner made, not an omission, and the plan's first job is to make it a
> property of the build rather than a sentence in a README.

**Held locally until asked for.** Everything through the seventh session sat on a local branch. The eighth session's work was **pushed on the owner's instruction**, to `claude/brave-albattani-cag2is` and a pull request; nothing is merged by this plan. The ninth, tenth and eleventh sessions' work was committed locally on the owner's instruction of 2026-09-17 and has since reached `main` through pull requests #18 and #19, merged by the owner. The twelfth and thirteenth sessions' work reached `main` through pull request #20. The fourteenth to seventeenth sessions' work reached `main` through pull request #21, merged by the owner (`c3ea957`). The eighteenth session's work is **committed locally, on the branch `claude/dry-coverage-eighteenth`, and not pushed**, on the owner's instruction of 2026-09-19; its Hub half (the specification copy, the mirrored editions report and its pin) is an uncommitted edit in the Hub's checkout, and the two land together.

---

## What "full coverage" is allowed to mean under this constraint

§1 refuses the strong reading of *universal* and §59 refuses it again at length. This plan adds a
third refusal, narrower than both, because it is the one that applies to a build with no robot in
the room:

**Every robot with a public control interface gets a conformant driver, exercised against a fake or
a simulator, and no robot is awarded a tier.**

The last clause is not pessimism. It falls out of §15's own arithmetic, and Phase 0.1 pins it as a
test rather than leaving it as an inference — see the finding below.

Three things stay outside the claim no matter how much of this plan lands:

| Outside | Why |
|---|---|
| Robots with no documented control interface | An access problem. §59.1 |
| Host or Certified on program-level controllers (Fanuc, KUKA, ABB, Spot) | The vendor controller owns the servo loop and is the safety-rated component. §59.2, "not fixable in software" |
| Task success after retargeting | A research result nobody has. §59.4, §61 |

---

## Findings that changed the plan before any code was written

### F1 — Dry evidence awards `none`, not `guest`. One test stands in the way.

The plan as first drafted assumed dry work would land every robot at Guest tier, and proposed a new
CI assertion to hold it there. Reading `crates/skynet-contracts/src/certification.rs` found that
assertion largely unnecessary and its premise wrong.

`SCTS_TESTS` is 36 tests: **17 `sim`, 19 `bench`, 0 `field`** — field conformance is a ratio on
`CertificationInput`, not a per-test suite. Of the 19 bench tests, 18 are required from `host` or
`certified`, so a dry run being `missing` on them caps the award at guest by construction. That much
the design already does, and `absence_is_not_a_pass` plus
`the_evidence_caps_the_award_rather_than_the_claim` already hold it.

**But exactly one bench test is required from `guest`:**

```
offline.control.independent    suite: bench    area: offline    required_from: guest
```

So a sim-only run is `missing` on a guest-required test, and `award_certification` returns
**`none`** — no tier at all. The dry ceiling is one step lower than the plan claimed, and the
difference is a single row of §15's table.

### F2 — The question F1 raises is a specification decision, not a patch.

`offline.control.independent` measures §11's invariant: no control decision requires a network round
trip. That does **not** need a body. It needs a real machine — real Linux, real systemd, a real
kernel turning a loop — which is exactly what `packaging/virtual-robot.sh` produces.

So there is a real question: is that test `bench` because it needs *hardware*, or because it needs
*a machine rather than a model*? A virtual robot is the second without the first.

**This plan does not answer it.** CONTRIBUTING.md is explicit that a specification change is an
issue a maintainer carries to both repositories, never a pull request against the pinned document —
and reclassifying a bench test to `sim` is precisely the move Phase 0.1b exists to prevent anyone
making quietly. Raised, not resolved — and **resolved as 4 under *Decisions*** below, which keeps it `bench`.

### F3 — Phase 0.4 (limit provenance) is already built. Twice.

The original plan asked for a marker distinguishing declared limits from measured ones, on the
grounds that a descriptor generated from a URDF or a simulator model would otherwise be treated as
sourced. Both halves already exist:

* `embodiment.limits.sourced` — a `sim` test, required from `host`: *"Every declared limit cites a
  source. A rating with no provenance fails, because the safety kernel would be enforcing a
  rumour."*
* CI enforces that `skynet-urdf` writes a literal `Json::Null` envelope and that nothing else can
  reach the field, with the failure message citing §63.4: a URDF effort limit is a design value from
  a CAD model, not a datasheet safety rating.

No code. Recorded here so it is not rebuilt.

### F3a — Extending the dry ceiling to the other seven editions is blocked on this machine.

`corpus/certification/award.json` has twenty cases and **none of them is the sim-only shape.**
`no-evidence` is an empty result set and `guest-complete` passes the bench test too, so nothing holds
the eight language editions to F1 — only the Rust test added in 0.1 does.

A corpus case is the right fix and it cannot be produced here. Two independent blockers:

* **Zig is not installed**, and `tools/edition-report.mjs --check` re-runs *every* edition to
  re-derive `reference/reports/editions.json`. Adding a case changes the figure, and the check would
  go red on something this machine cannot regenerate.
* **Node here is v26; the `corpus` job pins Node 24.** `ci.yml` is explicit that the case tables are
  read out of the engine the reference runs on and that the Unicode version differs between the two.
  Regenerating the corpus under the wrong one risks a diff nobody asked for.

Left undone deliberately rather than half-done. It is the first item in *Next* below.

> **Done, 2026-09-16, and both blockers above were wrong.** Zig 0.16.0, Node 24 and .NET 8 all
> install in minutes on a machine that is not this one, so the three cases exist and all eight
> editions were run against them. The claim that `edition-report.mjs --check` re-runs *every*
> edition is also wrong — it re-runs the four complete ones, and Zig is held by its own ratchet.
> What genuinely blocks a corpus case is F8: §73 states the case count exactly. Kept rather than
> rewritten, because a blocker that turned out to be a property of one laptop is worth being able
> to recognise the next time one is written down.

### F4 — Most of Phase 0.1's proposed machinery already exists as an invariant.

`ci.yml`'s `invariants` job already greps both halves of the §35.3 simulator cap — in
`certification.rs` and in `skynet-cli`'s tier walk — so a simulated body being capped at guest is
already a check rather than a comment. What it does **not** cover is a real driver with no hardware
behind it, which sets no `simulator` flag and is not lying when it doesn't. That gap is real and
narrow, and F1 is what actually closes it.

---

### F5 — The production-caller check found two on its first run.

Both are the historical shape exactly, and neither is a crash:

* **`driver_may_activate`** (`skynetd-io/src/admit.rs`) — §19's rule that only a `verified`
  signature admits a driver. `runtime.rs:519` enforces it **inline** and is the live copy; this
  spelling is called by nothing. Not a safety hole, because the rule *is* enforced — but it is §78's
  shape, where one fact with several spellings wants a mechanism rather than care. Which copy
  survives is the owner's call.
* **`verdict_inputs_all_recorded`** (`skynet-blackbox/src/assemble.rs`) — §26.4. The black box can
  read a sealed window back and replay its cycles through the kernel, and never reports whether the
  window recorded every field a replay evaluates. A replay of an incomplete window is presented
  exactly like a replay of a complete one.

### F6 — §73's line count has two lines of headroom, and 0.1 spent most of it.

`tools/scale-report.mjs --check` holds §73's stated size to the tree, rounded to the nearest
thousand. The tree was at 136,427 lines of Rust — 73 lines below the 136,500 boundary where the
figure rounds to 137,000.

0.1's first draft added 103 lines and broke the check. The fix would have been a one-word edit to
§73, which is in the pinned specification and therefore an issue rather than a patch — so the tests
were rewritten to carry their argument by reference to this document instead of restating it, landing
at 71 added lines and **136,498 total.**

**Two lines of headroom.** The next person to add Rust will trip this, and the honest resolution then
is the §73 edit rather than more compression.

### F7 — `main` was red on three counts when Phase 0 was written, and the gate did not say so.

The 2026-09-16 entry below records the full pre-PR gate passing. It did pass. CI on the same commit
(run 60, `682d9b0`) failed three jobs, and none of the three is reachable by the gate
`CONTRIBUTING.md` asks for:

| Job | Failure | Why the gate missed it |
|---|---|---|
| `rust` | `clippy::byte_char_slices` on `skynet-ring-shm/src/pass.rs:187` | The lint landed in clippy 1.98. The machine that ran the gate had stable at 1.94 |
| `invariants` | `TS4104` on `tools/unicode-case.ts:643` | The oracle type-check is a CI step; the gate has no TypeScript in it |
| `corpus` | the Go edition's fusion self-check | The gate has no edition in it either — `editions/README.md` is a separate instruction, conditioned on *"if you changed an edition"*, and nobody had |

All three are fixed. The general lesson is the first row: **a pre-PR gate is only as current as the
toolchain under it**, and a clippy release can turn a green tree red with no commit in between.

The Go one is worth its own line, because the check was right and the fact was wrong. Go does not
contract `x*y + z` on amd64 at *any* `GOAMD64` level — v1, v2, v3 and v4 all emit `MULSD` then
`ADDSD` — so `fusingTargets` had been claiming a fusion the compiler has no rule for, and the scan
refused the target rather than reporting a listing it could not see into. The row survives because
an explicit `math.FMA` still reaches an amd64 binary as `VFMADD231SD`, which is a single rounding by
definition and exactly what §31.2 forbids; `fusionprobe.Intrinsic` is now its positive control, and
on that target the scan is a check for the call rather than redundant cover for the other five.

### F8 — F3a named two blockers, and neither was the binding one.

Zig and Node 24 were real on the machine that wrote the plan and are not properties of the work.
Both installed here in under a minute, along with .NET 8, so **all eight editions and the corpus
generator ran on one machine for the first time.**

What actually blocks a corpus case is one line of the pinned specification. §73 states the corpus
size *exactly* — `a corpus of 80 files and 1,199 cases` — and `scale-report.mjs --check` holds the
document to a measurement of the tree, refusing with *"correct the document, not this check."* So
**every corpus case anyone adds, ever, requires an edit to `docs/skynet-os-architecture.md`.** F6
predicted this arriving through Rust lines; it arrives through the corpus as well, and more often,
because cases are added more freely than crates.

Also worth recording against F3a's own text: `edition-report.mjs --check` re-runs the **four
complete** editions, not every edition. Zig is held by its own `--min-passed` ratchet in CI and is
not on that file at all — which is the refusal inside `edition-report.mjs` working as designed.

### F9 — 0.2's own check had two defects, and the second one lies.

Both found by opening a file a finding named and not seeing the function there.

* **Wrong line, 18 times out of 38.** `strip()` removed comments and string literals *including
  their newlines*, and every line number is counted off the stripped text, so each block comment
  above a declaration shifted it upward by its own height. `upload_target` was reported at
  `upload.rs:545` and is declared at 742.
* **Blind to 500 lines, and one caller.** `strip()` had no case for a character literal, so `'"'` at
  `upload.rs:230` opened a string the scan closed at the next quote far below, swallowing the call
  site at `upload.rs:389` whole. `upload_target` **has** a production caller and was allow-listed as
  though it did not, with a reason written to fit.

The second is the one that matters. A false positive on this check is an argument somebody has to
write down; **a false negative is an answer nobody goes and checks** — and the remedy the check
offers for a false positive is to add a name to a list, which is how a real caller got denied in
writing. Five other files spell `'"'`; none happened to hide another sole caller, so the count moved
by exactly one.

### F14 — `main` was red on the string-literal check again, and F12's mechanism is why.

Four literals in `skynet-kernel-host/src/main.rs`, all `--frame-pool` refusals, all introduced by the
commit that wired the camera in — *after* c04f57a fixed thirty-eight of the same thing. The check
works, the check was never run, and CI had not run either because nothing had been pushed.

So the remedy is not another fix. It is **`tools/run-ci-job.mjs`**, which reads the steps out of
`.github/workflows/ci.yml` and runs them, every one whatever the last one did. Written that way
because the obvious alternative — a script listing the checks — is a second record of a fact, and this
codebase's standing argument against those is that they disagree exactly once, after the failure that
matters. There is nothing to keep in sync: a step added to the workflow is a step it runs.

On the tree as it stood: **52 of 53 `invariants` steps passed and one failed**, which is the ratio the
pre-PR gate in `CONTRIBUTING.md` could not have produced, because none of the fifty-three is in it.
That file now says so and says how to run the rest.

What it cannot reproduce is the runner, which is F7's first row: a clippy release, a `systemd` or a
locale this machine does not have. The tool's own summary says that rather than claiming CI is green.

### F15 — *Decisions* 5's terms fit neither route to ONNX Runtime.

The owner allowed one third-party dependency, narrowly: *"the dependency lives in one crate, behind a
feature that is off by default, the safety kernel stays `no_std` … and `ci.yml`'s check is narrowed to
permit **exactly that package and nothing else** rather than switched off."* Both routes to §23.3's
named runner break the emphasised clause, in opposite directions:

* **A Cargo dependency on a maintained binding crate** brings a transitive tree. The narrowing would
  admit tens of packages, and `Cargo.lock` — which today holds exactly the workspace members, and is
  checked for it — would stop being a list anybody reads.
* **`dlopen` through `skynet-dl`** takes no Cargo dependency at all, and was started in a previous
  session (an uncommitted `Library::address`, reverted here). What stops it is that ONNX Runtime's C
  API is reached through one struct of several hundred function pointers whose order is the library's
  version. A hand-transcribed copy that is wrong by one field calls the wrong function through a raw
  pointer, in a robot's policy runtime, and that is not a defect that surfaces as an error. It could
  not have been verified on this machine either way.

Neither was taken. What was built instead is the **decision layer behind a `Session` seam** — assemble
the input, refuse what cannot be run, turn an output into a command, seven named holds — with a
dependency-free feed-forward runner so that every one of those refusals is exercised. The seam is why
the choice above can be made later and change nothing else, and it is stated in three places that a
policy file declaring `kind: "onnx"` is **refused by name** rather than run by something else.

The carve-out the owner granted was therefore not spent. It is still there when the routes are costed
by somebody with the authority to accept one.

> **Costed, 2026-09-16, and the second bullet's premise is wrong.** Kept rather than rewritten, for
> the reason F3a is kept: a blocker that dissolved on inspection is worth being able to recognise the
> next time one is written down.
>
> *"one struct of several hundred function pointers whose order is the library's version"* — the order
> is the **requested API version's**, not the installed library's. `OrtGetApiBase()` is the single
> exported symbol, and the `GetApi(version)` on it
> [returns the `OrtApi` for the version asked for, or `nullptr` when that version is
> unsupported](https://onnxruntime.ai/docs/api/c/struct_ort_api_base.html). Microsoft's own release
> note for ONNX Runtime 1.0 states that the C APIs are ABI compatible and follow semantic versioning,
> and that a program linked against one version keeps working with later releases without re-linking
> ([announcement](https://opensource.microsoft.com/blog/2019/10/30/announcing-onnx-runtime-1-0)).
> Content rephrased for compliance with licensing restrictions.
>
> So the failure mode F15 refused is not the one that exists. A version this workspace did not
> transcribe against yields a **null pointer and a refusal**, which is a detectable, nameable hold —
> not a misaligned struct dispatching through a raw pointer. What remains is transcription error
> *within* a known layout, which is bounded (only the entries up to the ones actually called), and
> `GetVersionString` sits on `OrtApiBase` rather than on `OrtApi`, so it can be called to sanity-check
> the library **before** anything trusts a transcribed offset.
>
> That flips the comparison. Resolved as *Decisions* 12: **`dlopen`**, which needs no Cargo
> dependency, no `CONTRIBUTING.md` edit and no change to `ci.yml`'s check — so the carve-out stays
> unspent for a reason better than caution.

### F20 — the transcription self-check crashed instead of refusing, and only the control said so.

`Ort::open` runs a status round-trip on every load — make an `OrtStatus` with a known code and a known
message, read both back — and the reason is not that ONNX Runtime might be broken. It is that
`CreateStatus`, `GetErrorCode` and `GetErrorMessage` are the **error path**: a successful load and a
successful run touch about twenty members of the call table and prove all of them, and leave those
three unproven until the moment something needs reporting. So they are called once, deliberately,
while nothing is wrong and the answer is known.

The check was written, it passed, and it was **wrong in the way that only a control could show**. The
control for a transcribed ABI is to break the transcription: two same-width members swapped, which
leaves `size_of` identical and so is invisible to the cheap test. With `get_error_code` and
`get_error_message` exchanged, the round-trip did not refuse — it took `STATUS_ACCESS_VIOLATION`.

The reason is a one-line ordering mistake. It read the code and the message in the same block and
compared afterwards, so a reordered table meant calling `CStr::from_ptr` on the integer `2`. **An
`i32` that comes back wrong can be inspected; a `*const c_char` that comes back wrong cannot.** The
fix is to check the integer first and return before the pointer is ever dereferenced, and the control
then fails as it should — *"a status made with code 2 read back as -239558972 … Nothing was
dereferenced"* — on all eleven tests, with no crash.

Worth recording as a coverage finding rather than only as a bug, because of what it says about the
*shape* of the evidence. The crate had twenty tests and every refusal among them; eleven of those ran
against a real library and asserted arithmetic. All of it passed with the defect present, because the
defect is only reachable when the transcription is already wrong — and the transcription was right.
**A check on a thing that never fails is a check nobody has run**, which is `CONTRIBUTING.md`'s rule,
and this is the clearest instance of it in the repository: the control was not a formality confirming
a finished check, it was the only thing in the session that read the check at all.

### F16 — §47.6's pool hands a policy pixels and no shape.

The pool header carries a stream name, a slot count and a slot size. It carries **no width, height or
format**, so a policy given its bytes cannot decode them from the pool alone.

That is not an omission, and the fix is not a wire change: the shape is a property of the **body**, and
§44's embodiment descriptor is where a body describes itself — the MuJoCo driver and the LeRobot bridge
each declare `width`, `height` and `modality` per sensor there. A policy resolved against an embodiment
is resolved against those numbers.

What it obliges is a refusal when the two disagree, which is now `Hold::FrameShape`: a frame whose byte
length is not `width * height * components` is held on rather than read at whatever stride happens to
fit. A frame read at the wrong stride is not an error — it is a picture of something else.

### F17 — a model that reads a quantity the body does not measure holds for ever, and the gate found it.

The cross-process vision test failed on its first run, and the reason was the design working: §24 has a
driver write `NaN` for a quantity it does not measure, `Observation::default` is `NaN` throughout for
exactly that reason, and the LeRobot bridge reports relative encoder positions and no velocity. A model
reading position **and** velocity therefore holds on every cycle — correctly, with the channel named,
and uselessly.

The fix is that a model declares what it reads: `"reads": ["position"]` in the artefact, required with
no default. A default of position-and-velocity is the tempting one and is the wrong answer for the
reason §80 keeps giving — it would let a model load against a body it can never run on, and deliver the
reason as a hold on every cycle instead of as a refusal at load.

Worth recording as a coverage finding rather than only as a fix: the crate had eighteen tests covering
every refusal it could reach, and it took **a fake body with a real driver's honesty about what it does
not measure** to find this. That is the argument for the loop existing, arriving one level up from F11's.

### F18 — `revoke_loans` cannot be wired, and the available signal is worse than none.

> **Closed, 2026-09-16 (eighth session), and the finding's last paragraph was the thing to doubt.**
> It said *"what is sound is the process being gone: a `pidfd` on it, or the supervisor that
> launched the unit saying so"*, and then that *"the process that holds the pool has neither"* — so
> the work was framed as choosing between two owners and building a channel. Neither was needed.
> `SO_PEERPIDFD` exists precisely because `SO_PEERCRED` plus `pidfd_open` is racy, and it hands the
> **descriptor** straight to whoever holds the accepted socket. The process that holds the pool is
> the process that accepts, so it has had the sound signal available since Linux 6.5 and the finding
> did not know to ask. Kept rather than rewritten, for F3a's reason.

Found by the doc-comment check below. Its documentation said *"called by the runtime when a policy's
process exits"*, which described the design and not the tree: nothing calls it, and no process that
holds a `Writer` has a **sound** signal that the reader is gone.

The signal that looks available is not one. `handoff::Port` could keep the accepted stream and revoke on
its end-of-file — but `SCM_RIGHTS` **duplicated** the descriptors, so a policy may close the socket and
keep the mapping. Revoking there would take a slot out from under a live reader, which is the one
property §47.6 exists to provide.

What is sound is the process being gone: a `pidfd` on it, or the supervisor that launched the unit
saying so. `skynetd` has the second and does not hold the pool; the process that holds the pool has
neither. Raised rather than closed, because it is a decision about which process owns the fact — and
until it is made, a crashed policy costs the pool the slots it held and `stats()` reports the drops.

### F19 — three counts in `Summary` had no surface, and one of them is the number §23.3 is about.

`skynet-policy-host` computes `overruns`, `stale_holds` and `unbudgeted` per turn, and `skynetp` printed
none of them. That is the fifth instance of this repository's four-times defect — a value that is
correct, tested and reached by nothing anybody reads.

`overruns` is the one that matters. §23.3's rule is *"a design where a 40 ms inference spike delays a
torque command is a design that falls over"*, and this is the number that says whether a backend is
holding to the period it was given. The loop deliberately does **not** act on it (the fourth rule in
`run.rs`: a late command is published and counted, not dropped), which makes printing it the only thing
that makes it visible at all. The gate's *"zero overruns on either side"* is a claim that could not have
been made before this session, because half of it was unprintable.

### F10 — §36.1's operator notification is `Transition::warning`, four defects later.

Found by draining the baseline, and the clearest vindication of 0.2 so far.

A power state derives three facts. `power_to_json` serialises `requires_safe_state` and
`stops_immediately`, and the CLI renders both. **`notifies_operator` is in neither the document the
daemon serves nor the daemon itself** — nothing under `crates/skynetd-io/src` or
`crates/skynetd/src` matches on `PowerState::Low`, `Reserve` or `Critical` at all. The row that says
an operator is told is computed, tested, and told to nobody.

The good half, because it changes what the finding means: §36.1's *withholding of new task leases*
**is** enforced, by `power_refusal_from` on the reconcile pass (`runtime.rs:1761`). A robot below its
reserve stops taking work. What it does not do is say so to a person.

Closing it is a wire change — a field on the power document, the TypeScript oracle, the corpus's 28
power cases and four editions — so it was raised rather than done. **Decided as 2 under *Decisions*:** wired.

### F21 — *Next* 4 was called "the one item that would move the dry ceiling off `none`". It is one of three.

Built and then measured, which is the only way this could have been settled. With
`offline.control.independent` recorded and applied, `skynet bench award --claim guest` reports:

```
missing  rhal.abi.version, rhal.state.completeness, rhal.command.rejection,
         embodiment.descriptor.valid, safety.kernel.parity, safety.policy.isolation,
         package.signature.required, package.grants.gate, security.credential.scope
guest: 0 failing, 9 unmeasured.
```

Nine, all `sim`. The bench blocker is gone and the ceiling did not move, because guest requires ten
tests and only one of them was ever the bench one. The other nine split three ways, and only the
first is an absence of plumbing:

* **Four are measured by `skynet driver check` and had no way out of it** — that is what
  `--results` is, and with both files on one command line the award folds them together. So this
  half is now closed for a driver; it is not closed for the loop, whose subject is three binaries
  rather than a shared object, and which has no driver-facing sim run at all.
* **Four are the runtime's**, and `skynetd-io::conformance` runs them — on a robot, over a socket.
  There is no producer that writes them to a file, so they cannot reach an award computed anywhere
  else. Named rather than built: it is `GET /conformance` plus a `--results` flag on
  `skynet conformance`, and it needs a robot to be worth having.
* **`package.signature.required` fails**, and §15.1 spends a paragraph saying so on purpose: §37.1's
  admit stage reports rather than gates for every package kind but a driver, an unsigned package
  installs, and the platform's own suite says so about it. A failure is not an absence, and nothing
  in *this* plan closes it.

> **Built, 2026-09-18 (twelfth session), and the second bullet's premise was half wrong.** Kept
> rather than rewritten, for F3a's reason. `skynet conformance --results` exists, and running it
> against a live `skynetd` showed the robot **skips** two of *"the runtime's four"* by design:
> `safety.kernel.parity` is an edition's property and `security.credential.scope` is the Hub's. The
> first had no producer anywhere and now does (F42); the second is not this repository's to measure.
> And folding the new documents into one award found that the award depended on the order the files
> were named in (F43).

So the dry ceiling is `none` for **three** independent reasons and the plan has been counting one.
The correction matters in the direction that always matters here: *Next* 4 was the item the plan
said would end the `none`, and finishing it ends nothing. What it does do is make the first reason
the *only* one that is about plumbing rather than about a robot or a decision — which is a smaller
claim and a true one.

### F22 — `make check-offline` had never run on a checkout with file modes, and its network guard could not fail.

Two defects in one script, found by running it. The fifth session's entry records `check-offline`
green; that was WSL, where a Windows filesystem reports every file executable.

* **`loop.sh` is committed `100644`** and the offline path does `exec unshare -rn "$0"`, which needs
  the execute bit. Every offline run on a real Linux checkout died as
  `unshare: failed to execute ./loop.sh: Permission denied`, before a single assertion. The bit is
  set now, and the re-exec goes through `/bin/sh` so that the mode stops being load-bearing — a zip,
  a Windows checkout or a filesystem without modes all behave the same way.
* **The network probe was `ip route show 2>/dev/null | wc -l`.** `ip` is `iproute2` and is absent
  from minimal images; a command that is not found writes nothing to stdout, and `wc -l` of nothing
  is `0`. So *"this namespace has 0 route(s)"* was also what a machine with a default route and no
  `iproute2` reported — and the guard passed, on the sentence whose own comment says *"a run that
  reported offline with a default route would be measuring the opposite of what it claims"*.

The second is the one worth keeping. It is F9's asymmetry again: a false positive is an argument
somebody writes down, a false negative is an answer nobody goes and checks. The reading now comes
from `/proc/net/route` and `/proc/net/dev` — the kernel's own tables, present on every Linux, empty
in a fresh namespace — and an unreadable one is a refusal rather than a zero. Held to a control: run
with `SKYNET_LOOP_REEXEC=1` so no namespace is made, and it refuses with *"2 route(s) and 3
non-loopback interface(s)"* where the old probe said zero and continued.

### F23 — the sim suite's one quantitative test threw its number away.

§15 marks nine tests quantitative and exactly one of them is `sim`:
`embodiment.roles.coverage`, measured in %. `skynet driver check` has computed that percentage since
the command was written, interpolated it into a sentence, and kept nothing. The fifth instance of
this repository's four-times defect and the first found by a *reader* rather than by an audit: the
results document refuses a quantitative pass with no `value`, `--results` was written, and the
command's own output would not load.

`CheckResult::with_value` is the fix and it is one call site. Worth recording because of what it
says about the cost of the refusal: the rule exists for `safety.estop.latency`, where §15 says in
its own words that a driver returning success is not evidence — and the first thing it caught was a
number this workspace already had.

### F24 — capture runs inside §7's control cycle, and the first body slow enough to show it made the loop miss on every frame.

The fifth session recorded a departure from Phase 1's gate: *"the driver is the LeRobot fake rather
than MuJoCo — the same ABI, one `pip install` away from being the other, and the pipeline under test
is identical."* The second clause is true about the pipeline and was wrong about what the run
measures. `pip install mujoco` and `apt-get install libosmesa6` take a minute here, `BODY=mujoco`
puts a physics simulator on the other side of the same ABI, and the gate **fails**:

```
cycles          250
overruns         73 — cycles that missed the 20.00 ms period
frames read      73 of 73 reads
```

**Exactly one overrun per frame read, and none on any other cycle.** That is not jitter. §47.2's
RHAL backend is read by `Camera::turn` from inside `skynetk`'s control cycle, so a sensor whose read
costs more than the control period delays §7's guards *by construction*. Measured in the driver
process: 18.2 ms to render 128×96 through `MUJOCO_GL=osmesa`, against a 20 ms period and a physics
step of 0.007 ms. The render is the whole of it.

Three things make this worth a finding rather than a bug report.

* **It is §23.3's own rule, on the side of the loop where it is worse.** *"A design where a 40 ms
  inference spike delays a torque command is a design that falls over"* — and §23.3's answer is that
  inference lives in another process, which is why `skynetp` reports **0** overruns in the same run.
  Capture has no such separation: it is a call on the control thread.
* **No fake could have shown it.** The LeRobot bridge draws a 32×24 pattern; the cost is invisible
  and the loop is green. The finding needed a body whose sensor read is genuinely expensive, which is
  the argument for the second body existing at all — and the reason `check-loop` now takes one.
* **The fix is an ABI question, not a patch.** Moving the read to its own thread means calling
  `read_sensor` while the control thread is in `read_state`, and `bindings/c/skynet_rhal.h` says
  nothing about concurrency: its three rules constrain `read_state`, `write_command` and
  `emergency_stop`, and re-entrancy is not among them. A runtime that started calling a driver from
  two threads would be relying on a promise no vendor has made. That is a specification change and
  therefore an issue, in the same class as Phase 3's.

> **Closed, 2026-09-18 (fourteenth session), by §89 rather than by a faster machine.** On WSL2 with
> llvmpipe — a 14.4 ms render against the same 20 ms period, so this finding reproduced exactly —
> `drivers/mujoco` now renders on a thread of its own from a snapshot and declares `concurrent_read`,
> and `check-loop-mujoco` reports 0 overruns with 51 of 51 frames read on `skynetk`'s capture
> thread. Withdrawing the declaration puts it back to three overruns and a suspended stream (§89.2).
> Kept rather than rewritten, for F3a's reason.

What is **not** claimed: that 18.2 ms is a property of MuJoCo. `osmesa` is a software rasteriser and
a machine with a GPU would render this in well under a millisecond, so `check-loop-mujoco` passes
there and fails here. That is the honest state of a real-time assertion — §15 makes
`rhal.sensor.latency` a *bench* test for exactly this reason — and the script says which of the two
it is rather than reporting a generic miss.

### F25 — §15's suite refuses every robot §59.2 describes, and the document contradicts itself about it.

Found by writing Phase 3's fourth item and running it. `drivers/pendant` is a body behind a
program-level controller: joints that are reported and commanded in no mode, `write_command`
answering `SKR_EPERM`, and motions reachable only through `extension()`. §59.2 says exactly what
such a machine is entitled to:

> These robots are **Guest tier permanently**, and not because of a missing feature — because the
> architecture that makes them safe is somebody else's and is already there.

`skynet driver check` awarded it **no tier at all.** `rhal.command.rejection` is required from
`guest`, and one of the four assertions `check.rs` files under that id asks a driver to *accept* a
well-formed joint command — which no robot §59.2 describes can do, by construction, and for no
defect.

Two things are wrong and only one of them is the code.

* **The implementation over-reaches the test's own definition.** §15's entry for
  `rhal.command.rejection` reads: *"The driver rejects a command naming a joint that does not exist,
  rather than ignoring it."* Rejection. The acceptance assertion is a fourth property filed under an
  id that does not name it, and it is the one that excludes the largest installed base of robots in
  the world.
* **So the document disagrees with itself.** §59.2 grants a tier in prose that §15's suite refuses
  in arithmetic, and the code had been following the stricter one by accident — which is the worse
  direction only because nobody had built the body that would show it.

**No amendment is needed to fix it**, which is the part worth keeping. §15 already has the word:
`not_applicable` is *"reserved for cases where inapplicability is a declared property of the body
rather than a gap in the evidence"*, and a body declaring `control.modes: []` has declared exactly
that. The acceptance assertion reports `not_applicable` there — which §15 counts as a pass — and
the pendant now reaches guest, as §59.2 always said it should.

The **three rejection assertions still run**, deliberately. A driver that ignores a short command
reads past the end of the caller's array whether or not its machine takes commands, and a body that
answered one code to everything would hide a real defect behind a true statement. `drivers/pendant`
checks the command's shape *before* its permission for that reason, and its own suite asserts both.

Held to a control: `commandable` forced true puts the pendant back to *"no tier — 1 guest-tier
test(s) failed"*, and the mock arm is unaffected in either direction because it declares two control
modes and always did.

### F27 — the UR driver was written against a controller, and three of its properties would not have been guessed.

F15's rule, applied where it was meant to be: a protocol transcribed from memory and tested against a
fake written from the same memory proves that two copies of one misunderstanding agree. So
`universalrobots/ursim_e-series` was pulled, started, powered on through its dashboard, and **every
byte of `drivers/ur_rtde` was measured against it** — UR's own controller software, URControl
5.26.0.0. The plan's table asked for exactly this substrate and here it is.

Three things came out of the measurement that a careful reading would not have produced, and each
one is a defect a driver would have shipped with.

* **`TEXT_MESSAGE` arrives unsolicited, in the middle of the handshake.** A fresh URSim's first words
  are *"Safety Setup has not been confirmed yet"*, and a client that takes the next packet as its
  reply is **off by one for the rest of the session**: the version reply read as the outputs reply,
  the outputs reply as the inputs reply, and every one of them parses. The first probe did this, and
  the symptom was a recipe id of zero and a types list of spaces — which looks like a controller
  problem and is not.
* **The output stream is a firehose, and a client that does not drain it reads the past.** Packets
  come at the negotiated frequency whether or not anybody reads, and a socket buffer holds seconds
  of them. The second probe commanded a motion, sampled every fortieth cycle, and concluded the arm
  had not moved. It had moved; the packets being read were a second old. That is §47.6's latest-wins
  rule arriving from a completely different direction — **for a control loop an old sample is not
  data** — and `Rtde.newest` drains to the last complete packet for the same reason the frame pool
  overwrites.
* **`IN_USE` does not only mean "no such variable".** RTDE's general-purpose registers are allocated
  to **one session at a time**, so a client reconnecting before the controller has released them is
  told `IN_USE` about a recipe that worked moments earlier. The driver's refusal named the wrong
  cause — *"this controller has no such variable"* — and a reader would have gone looking at
  PolyScope versions. The suite caught it, because the suite opens a second session.

And a fourth, which is about §15 rather than about UR. `rhal.state.completeness` requires the
sequence to advance between two consecutive reads, and **this is the first driver here whose source
has a clock of its own**: the mock computes a sample when asked, MuJoCo steps when asked, the LeRobot
bridge reads when asked, and all of them are faster than whatever is calling. RTDE publishes at
125 Hz whatever the caller does, so two reads microseconds apart genuinely have one sample between
them — and returning the previous one, which is truthful, reads to that check as a stalled producer.
The resolution is in the driver and not in the check: `read_state` waits for a new sample, bounded by
the period this session negotiated rather than by a guess. A caller faster than the robot is slowed
to the robot, which is the only honest thing a driver in that position can do.

### F28 — §24's facts had no path, so four of the first clauses anybody writes could run on no body at all.

Phase 4's second item asks for a measurement — *"every unresolvable path is a body the skill cannot
reach, so this is directly measurable: how many bodies a fixed skill set admits on, before and
after"* — and the measurement is starker than the phrasing suggests. §54.2 **refuses** a skill whose
precondition names a path that does not resolve, so a clause about an unpublished fact is not a
degraded skill. It is a skill that no robot can run.

Four such clauses are on anybody's first day:

| Clause | Path | Before |
|---|---|---|
| *only when the stop is clear* | `robot.state.estop` | unresolvable |
| *not while the brakes are on* | `robot.state.braked` | unresolvable |
| *refuse on an unhomed body* | `robot.state.homing` | unresolvable |
| *only when nothing is moving* | `robot.state.velocity.<joint>` | unresolvable |

Every one of those is a fact a driver reports **on every cycle**, through §24's state flags and the
state struct, and none of them had a path. `robot.state.*` carried positions, gripper, battery, base
pose and §27's operating state, and stopped.

Three things about the widening are worth keeping.

* **It is where §24's NaN rule finally bites.** A driver that does not measure velocity writes `NaN`,
  not zero — the LeRobot bridge reports relative encoder positions and no velocity at all — and a
  `NaN` produces an **absent path**, so a precondition asking about it refuses the skill and names
  the path. A zero would read as *this joint is still*, which is the one answer that is both wrong
  and reassuring. The rule has been in the ABI since §24 was written; this is the first surface where
  a person can be misled by breaking it.
* **Velocity and effort are read before the position check, and separately from it.** A joint whose
  encoder is dead and whose drive still reports current has an unreadable position and a readable
  effort, and folding them would lose the half that is working — which is exactly the state somebody
  is diagnosing when they look.
* **A zero `fault_code` is published**, and it is the one field in `live.rs` where the module's own
  rule points the other way. Zero is what a healthy drive reports, so it is a measurement rather than
  an absence; a driver that does not fill the field at all still omits the path. Two cases in the
  corpus keep them apart, because an edition that collapsed them would make *no fault* and *nobody
  asked* the same value.

**No robot produces them yet, and that is §33.1 rather than an omission.** §24's flags are a
*driver's* facts and the daemon is structurally kept off the path that holds one — the same reason
`guards_active` is `None` there, documented in the same place and now beside it. `skynet skill check`
populates them, because a publisher asking *"is this path addressable"* is asking about the OS's
vocabulary rather than about one robot, and the answer to that is yes. What closes the last step is
§26.2's cycle channels reaching the supervisor's side.

Held to a control: renaming `estop` to `estopp` in the builder fails the addressability test in
`skynet skill check` **and** three corpus files at once, which is the four-edition machinery doing
what it is for.

### F29 — the plan is named for a constraint that stopped being the binding one three phases ago.

The document's first line says what it is about: *"coverage work carried out **without a physical
robot**"*, and every phase after it is framed against that. Working through Phases 3, 4 and 5 in one
session produced a different answer, and it is worth stating plainly because it changes what to do
next.

**Of the eleven items in Phases 3, 4 and 5, seven are blocked on a decision about the specification
and none of the seven is blocked on hardware.**

| Phase | Item | Blocked on |
|---|---|---|
| 3.1–3.3 | motion action space | §18/§59.2 amendment — `docs/issues/motion-action-space.md` |
| 3.4 | mock program-level controller | *nothing* — **built**, `drivers/pendant` |
| 4.1 | route `execute` to a command-level driver | the same amendment, **and** a second decision: §33.1 keeps `skynetd` off the path that holds a driver, so the destination is `skynetk`'s and no channel exists |
| 4.2 | widen the live precondition context | *nothing* — **built**, F28 |
| 4.3 | capability token vocabulary | §8 amendment — `docs/issues/capability-token-vocabulary.md`. §5.0 says *"the token vocabulary is §8's"* and **§8 does not have one** |
| 5.1–5.4 | the class gates | §43, which says in its own words *"deliberately deferred rather than half-designed"*, and §58.3 lists three of the four again under *What is still not designed* |

The plan called Phase 5 *"all contract-shaped, all dry-verifiable"*. The second half is true and the
first is the trap: each of them is contract-shaped **once designed**, and the design is the part
nobody has done. Reading them as implementation work is how a session spends a day building a
descriptor field that turns out to encode a decision the owner had deliberately not made.

**And the specification cannot be edited from here**, which is what turned this from an observation
into a wall. `docs/skynet-os-architecture.md` is edited in both copies or in neither; this session's
harness permits the *figure* maintenance §73 requires — it took five of those without complaint — and
refuses to write a new section into the Hub's checkout. A one-copy edit is then the only thing
available and it is precisely what the rule forbids, so `check-mirrors` was left reporting both
copies matching their pin, which is the state to leave it in.

So `docs/issues/` exists: four amendments written out in the form they would take, each with the
implementation order it unblocks, so that ratifying one is an edit in each repository and a re-pin
rather than a session's design work done again. Three of the four were found by reading the document;
the fourth (F30) was found by a driver that could not command an ordinary robot, which is the better
provenance of the two.

**What this does not change.** Everything the plan says about hardware is still true — all 19 bench
tests are `missing`, `proximity` is inert on every robot, nothing has been signed by a Hub. The
correction is narrower and it is about *sequencing*: the dry constraint is what limits Phases 0, 1
and 2, and it is not what limits 3, 4 and 5.

---

## Phase 0 — the dry-mode contract

Buys no coverage. Everything after it is unsafe without it.

| | Item | State |
|---|---|---|
| 0.1 | Pin what dry evidence awards, as a test | **Done** — 2 tests in `certification.rs`, positive control verified |
| 0.1b | CI invariant: the bench suite cannot shrink | **Done, as the same test** — see below |
| 0.2 | CI check: every capability has a production caller | **Done** — `tools/check-production-callers.mjs`, wired into `ci.yml` |
| 0.2b | ... and no unreached one claims a caller in its own documentation | **Done** — the same tool, second half. Found two live: F18 and `asks_for_us` |
| 0.3 | `virtual-robot.sh` as the harness of record | **Done by this document** — it already runs on every push; this is the record saying it is the one |
| 0.4 | Limit provenance | **Already built — F3** |
| 0.5 | A runner for the checks the pre-PR gate does not contain | **Done** — `tools/run-ci-job.mjs`, and F14 is why it exists |
| 0.6 | A wire form for bench evidence, bound to its subject | **Done** — `skynet_contracts::bench`, `skynet bench record` / `award`. *Next* 4, and F21–F23 are what building it found |

**0.1b did not become a `ci.yml` grep, deliberately.** The Rust test runs in the blocking `rust` job
and names the test that moved when it fails, so a grep over the same source line would be a second
record of a fact the first one already holds — which this codebase argues against for the reason §80
gives about the partial's length: a second record of a fact disagrees exactly once, after the failure
that matters. What a Rust test *cannot* reach is the other seven editions, and that is F3a.

**0.2's justification** is four incidents in this repository's own history, all the same shape — a
path that existed and was never reached by anything but a test:

* `Transition::warning` reached the daemon's observe step and was dropped; nothing had ever made a
  sound.
* `can_start` had four test callers and no production caller; `skynetk` turned a cycle without
  consulting the start gate.
* `EventCode::GrantChanged` had exactly one producer, so the ring recorded every grant a Hub made
  and none an operator made standing at the robot.
* `available.proximity_reading` was hard `false` in two files, each carrying the note *"the day a
  producer exists, this is the line that changes"*.

A dry build multiplies this failure mode, because a fake is easier to satisfy than a robot.

---

## Phase 1 — close the loop, dry

| Work | Dry substrate | State |
|---|---|---|
| Capture backend | Synthetic, behind the trait V4L2 and GenICam will share | **Done** — `Source`, `Synthetic` and `pump` in `skynet-capture`; F11 is what writing it cost |
| Inference **decision layer** (§23.3) | A `Session` seam, and a dependency-free feed-forward runner behind it | **Done** — `skynet-infer`, `backend::Inference`, `skynetp --backend inference --policy <file>` |
| Inference **runner** (§23.3's own answer) | ONNX Runtime, CPU | **Done** — `skynet-ort`, by `dlopen` (*Decisions* 12). 20 tests, 11 against a real ONNX Runtime 1.30.0; F20 is what building it found |

**Gate: met, 2026-09-16.** `make -C drivers/python-host check-loop`. `skynetk` reads the LeRobot
bridge's fake wrist camera through the real ABI, publishes into §47.6's pool and offers it with both
rings; `skynetp` maps all three and runs a model whose weights make each setpoint *be* a pooled cell
of the image; the sealed window is read back and every recorded cycle replays to the same verdict.
Measured: 51 frames pooled, 199 of 200 turns shown a frame, 248 of 250 kernel cycles finding a
command, **zero overruns on either side**, and *"all 249 evaluated cycles reproduced"* with every
field the replay evaluates present on every cycle (§26.4).

Two departures from the gate as written, both stated rather than absorbed. The driver is the LeRobot
fake rather than MuJoCo — the same ABI, one `pip install` away from being the other, and the pipeline
under test is identical. And the policy is not an ONNX one, because there is no ONNX runner: F15.

> **The first departure is closed as of the eighth session, and closing it failed the gate.**
> `BODY=mujoco` runs the whole of `loop.sh` against a physics simulator rendering its own wrist
> camera — `make -C drivers/python-host check-loop-mujoco` — and on this machine it does not close:
> capture is inside §7's control cycle and an 18 ms render misses the period on every frame. F24.
> The gate's measurement below therefore still stands as taken, with the LeRobot fake, and what the
> simulator adds is a finding rather than a stronger pass.
>
> **The second departure is closed as of the seventh session.** `skynet-ort` exists, `skynetp` runs a
> `kind: "onnx"` artefact, and the two refusals that gate it were exercised through the binary against
> a real library. What has *not* been re-run is `check-loop` itself with an ONNX policy in place of the
> `mlp` one — the loop needs a body, the body needs Linux, and the machine that built the runner is
> Windows. So the gate's measurement above still stands as taken, with the `mlp` runner, and the ONNX
> runner's own evidence is its 20 tests rather than a closed loop. Two claims, kept apart.
>
> **Both are closed in the loop itself, as of the fourteenth session.** The ONNX run was made by the
> thirteenth session (`4e7fb3f`, `make check-loop-onnx`: 0 overruns, all 249 cycles reproduced) and
> never logged here. The MuJoCo one needed §89: with the driver rendering on a thread of its own and
> declaring `concurrent_read`, `check-loop-mujoco` closes on the machine that reproduces F24 — 0
> overruns where there had been one per frame, 51 of 51 frames on `skynetk`'s capture thread, every
> cycle replayed — and closes again with `POLICY=onnx`, a simulator's render through the production
> runner, which is the widest claim `loop.sh` can make without a robot.

---

## Phase 2 — drivers, and how far each goes dry

| Driver | Dry substrate | Dry-verifiable | Not dry-verifiable |
|---|---|---|---|
| **`ros2_control` bridge** | `mock_components/GenericSystem` — the fake `ros2_control`'s own tests use | **Built and run** — `drivers/ros2_control`. 47 checks in its own suite, 26 of 26 through the real ABI, guest tier. First executed 2026-09-18, against ROS 2 Jazzy and `ros2-control` 4.48.0; F41 is what that run found | Jitter under load — and a plugin that blocks in `read()`, which the mock cannot do |
| **CANopen / CiA402** | ~~Linux `vcan`~~ **an in-process bus** + CiA402 slave sim | **Built** — `drivers/canopen`. 61 checks in its own suite, 24 of 24 through the real ABI, guest tier | Bus bandwidth at real joint × rate — and now also *everything `vcan` would have added*: see below |
| **EtherCAT** | This repository's own master and ESC model — SOEM cannot be linked (F48), so it is **run against the slaves instead** | **Built** — `drivers/ethercat`. 87 checks in its own suite, 26 through the real ABI, guest tier; SOEM v2.0.0 takes the ring to Op over a veth pair (`make check-soem`, F51–F53) | **Distributed clocks and FSoE STO — the whole basis of its Certified ceiling** |
| **UR RTDE** | URSim (UR's own controller, containerised) | **Built** — `drivers/ur_rtde`. 35 checks, 24 of 24 through the real ABI, against URSim 5.26 | Servo timing |
| **Unitree SDK2** | `unitree_sdk2` on **both** sides of the wire | **Built** — `drivers/unitree`. 39 checks, guest tier (thirteenth session) | Joint access at rate, and any dynamics behind the fake's joints |
| **`libfranka` / FCI** | This repository's fake FCI, with the vendor's headers compiled to pin the wire layout (F47) | **Built** — `drivers/franka`. 79 checks, guest tier (thirteenth session) | 1 kHz torque, which is its entire claim, and the arm's own safety layer |

EtherCAT is the honest warning in this table: everything is dry-verifiable **except** the two
properties that justify its ceiling. A green CI on that driver must not be allowed to imply
otherwise.

**The CANopen row's substrate changed, and the change is worth the sentence.** The table asked for
`vcan`, and `vcan` is a kernel module: it needs `modprobe`, a kernel built with `CONFIG_CAN`, and a
container with the privilege to load one. This machine has none of the three — `socket.AF_CAN`
exists in CPython and the kernel answers `EAFNOSUPPORT` — and neither do most CI runners. So the
driver runs against `LoopbackBus`, the identical protocol over a list.

That is a smaller substrate than the table promised and the difference is stated rather than
absorbed: **a loopback bus has no arbitration, no bit timing, no error frames and no bus-off.** It
shows the encoding is right and shows nothing about the wire. `SocketCanBus` is the same driver
against the real thing, is written, refuses to construct on a kernel without CAN rather than
pretending, and `make check-vcan` exits non-zero where there is nothing to run against. What was one
row in the *not dry-verifiable* column — bandwidth — is now that row plus the physical layer, and
the driver's own suite prints the difference on every run instead of leaving it to be inferred from
a green tick.

### F26 — the CANopen driver reset every drive at open, which clears a latched fault.

Found by a test, and by the one kind of test that finds this: a refusal with a control beside it.
*"A drive that has latched a fault is refused, rather than commanded into one"* failed on its first
run — because `open()` sent `NMT reset node` before reading anything, and a reset returns a CiA 402
state machine to `switch on disabled`. The fault the test had just set was gone before the check
looked.

It is the shape this repository keeps finding and it is worse here than usual, because the erased
thing is **evidence about why a robot stopped**. A driver that resets at open does it on every
restart: an axis faults, an operator power-cycles the cell, the robot comes up clean, and the reason
is in nobody's hands. The remedy is an ordering — NMT start and nothing else, then read the
statusword, then refuse — and the refusal names `0x603F` so that whoever meets it can find out what
happened before clearing it deliberately.

Worth recording because the driver was *written* with the reset in it, deliberately, as the obvious
way to get a drive into a known state. The obvious way was the defect.

**The `ros2_control` row is built, and it is hosted rather than spoken to.** The obvious shape for
that driver is a ROS 2 node beside a `controller_manager`, and it is the wrong shape: §11 says no
control decision may require a network round trip, and a setpoint that crosses a DDS domain on its
way to a drive is a control loop with a network in it. `hardware_interface::ResourceManager` is the
layer underneath `controller_manager` — it loads the hardware plugins, owns their lifecycle and
hands out the interfaces — so `drivers/ros2_control` hosts one in-process and never starts a graph.
The cost is real and is written down rather than absorbed: a robot driven this way is **not** also
driven by ROS 2 controllers, because `ros2_control` allows one claim on a command interface and this
driver takes it. That is §25.2's capability lease, arriving from the other side.

### F30 — §24 commands a body in one mode; `ros2_control` commands a *joint* in one. The intersection is the only honest descriptor, and for a common robot it is empty.

§24's `skr_command` carries **one mode for the whole body**. `ros2_control` declares command
interfaces **per joint**, and nothing whatever stops a URDF giving joint A `position` and joint B
`velocity`. That is not a corner case invented for a test: it is how a mobile manipulator is
ordinarily written, arm on position and wheels on velocity, in one `<ros2_control>` element.

There is no honest way to command such a body through §24 as it stands, so the driver declares the
**intersection** — the modes every joint offers — and a body whose joints share none is reported
with **no control modes at all**, every `write_command` against it refused. `skynet driver check`
then reads it exactly as it reads `drivers/pendant` (§59.2): the joints are reported, and there is
no mode in which this driver commands them.

The alternative is the one worth naming, because it is what an integration would reach for: pick the
mode most joints offer, command those, and drop the rest. A body that took a position command and
silently moved two thirds of itself is precisely the failure §24's shape exists to make impossible,
and it would present as a robot that *mostly* works.

What the intersection costs is that a perfectly ordinary robot is uncommandable through this
platform. That is a **specification** question and not a driver defect — the fix is a per-joint mode
in §24's command, or a normative statement that a §24 body is one mode and a mixed URDF must be
split into two — and it is therefore the fourth of this session's amendments a maintainer has to
carry to both copies of the architecture, written out as
[`docs/issues/per-joint-command-modes.md`](docs/issues/per-joint-command-modes.md). The driver's
behaviour in the meantime is the conservative one: refuse, and say which joints disagreed.

Two smaller things the substrate settled on the way, both by a check that failed first. A URDF's
`<limit effort="...">` is a CAD design value and not a safety rating, so `effort_max_nm` is absent
and `envelope` is null (§63.4) — the same rule `skynet-urdf` already follows, arrived at
independently. And §5.1's roles are **never inferred**: a `ros2_control` URDF names joints and says
nothing about what they are, `shoulder` is a pitch in one robot and a yaw in the next, so roles come
from an operator-supplied `roles=` setting or the descriptor says `null` and
`embodiment.roles.coverage` names who is unclaimed.

---

## Phase 3 — motion-primitive action space

Highest coverage-per-effort item, and the most dry-verifiable, because it is contract work and the
corpus plus eight editions plus the TypeScript oracle is exactly that machinery.

**Requires a specification change** (a descriptor field and new transport methods), so it starts as
an issue, not a branch. *Decisions* 6 deferred it behind Phase 1's gate; **that gate is now met**, so
the deferral has expired and this is unblocked.

> **Done, 2026-09-17 (ninth session).** The amendment was ratified by the owner (*Decisions* 13) and is
> §84 of the specification, in both copies and re-pinned. Items 1-3 are built in the order below —
> the oracle, then 35 transport cases and 23 validation cases, then all eight editions — and
> `skynet transport`, `skynet driver check`, `drivers/pendant` and the Hub's transport page read it.
> What §84 settled that the issue did not is in §84.3's four rules; F32 is what building it found.

> **Blocked on the specification, 2026-09-16 (eighth session), and blocked mechanically rather than
> by judgement.** The amendment is designed and written out below, ready to file. It cannot be made
> here: `docs/skynet-os-architecture.md` is edited *in both copies or in neither* and this session's
> harness refuses to write to the Hub's checkout at all, so a one-copy edit is the only thing
> available and it is the one thing the rule forbids. `check-mirrors.mjs` still reports both copies
> matching their pin, which is the state to leave it in.
>
> So items 1-3 are unbuilt and item **4 is done** — a mock program-level controller needs no
> normative change, because §59.2 already describes the machine it stands for.

Order matters, and the repo has already paid for the lesson: **write the corpus cases before the
editions.** Doing it that way on §47.6 found the specified rule and the reference disagreeing about
which released slot a publish takes — a bug that would have cost a reader its frame on nearly every
cycle while presenting as a slow policy.

1. ~~Descriptor: action-space kind, named motions with parameters~~ **Done** — §84.1, `motions` on the
   descriptor and on the source embodiment, and `validateMotions` under `embodiment.descriptor.valid`
2. ~~Transport bindings over motions rather than joint channels~~ **Done** — §84.3, `motions` on the plan
3. ~~Corpus cases, then the editions~~ **Done** — `corpus/transport/motion.json` (35),
   `corpus/embodiment/motions.json` (23), two closed sets; Rust, Python, C++ and Go reproduce all
   1,270 cases, and C, Zig, Java and C# answer the two new sets at raised ratchets
4. ~~A mock program-level controller driver — the teach-pendant analogue of `drivers/mock`~~
   **Done** — `drivers/pendant`. Thirty-seven checks, nearly all of them refusals, because for that
   class of machine the refusals *are* the interface. It found F25 on its first conformance run.

Ends fully dry-verified, because there was never a clamping claim to verify.

---

## Phase 4 — skills coverage

1. ~~Route `execute({ skill, arguments })` to a command-level driver.~~ **Done**, 2026-09-17/18 —
   §88. §59.2 makes this the only path that applies to the largest installed base, and it had two
   blockers: §84 removed the first by giving a motion a place in the descriptor, and the second
   **dissolved rather than being bridged**. There was never a missing channel. A skill over a
   program-level body is a policy (§84.2), the ring a policy already writes is the route, and
   `skynetk` — which holds the driver — reads the request and calls `extension()`. What was
   actually missing is the *admission rule* for a request arriving on one: `admitMotion`'s ten
   refusals, authority before state and state before content.

   Built end to end and exercised as two programs rather than two halves of a test:
   `skynet-ring` carries a motion frame in the words above a joint command's with no ABI bump;
   `skynetp --backend motion` is the producer; `skynetk` is the gate, with `--lease`,
   `--operating-state` and `--motion-token` each declared and each defaulting to the reading that
   refuses; `drivers/pendant` is the controller. F39 is what building it found. Previously read:
   **Unblocked, and not built.**
2. ~~Widen the live precondition context per driver.~~ **Done**, 2026-09-16 (eighth session).
   Five branches added under `robot.state` — `velocity.<joint>`, `effort.<joint>`, `estop`,
   `degraded`, `braked`, `homing`, `fault_code` — through the oracle, eight new corpus cases, and
   all four complete editions. F28 is what it measured and what it found.
3. ~~Capability token vocabulary and `extensions`.~~ **Done**, 2026-09-17 — §85. Thirty-eight tokens
   in seven domains, four classes, `capability_unknown` from both directions, thirty `admission/tokens`
   cases, eleven `admission/vocabulary` cases and three closed sets across all eight editions. Both
   workstation gates report a token by class before a robot has to. F34 is what building it found and
   F35 is what auditing it against the document found.

`skynet skill check` stays the workstation gate it already is.

---

## Phase 5 — the class-gating open items

Each unlocks a robot class rather than a feature. All contract-shaped, all dry-verifiable.

> **Done, 2026-09-17/18 — §87.** The amendment was ratified by the owner (*Decisions* 13) and all
> four questions are answered, each with the alternative it rejected. What F29 said about this phase
> held exactly: it was blocked on **design**, not on a robot, and no part of building it needed one.
>
> Previously read: **Blocked on design, not on a robot — F29.** §43 introduces all four with
> *"deliberately deferred rather than half-designed"*, and §58.3 lists three of them again under
> *What is still not designed*. `docs/issues/phase-5-class-gates.md` records what each one has to
> settle — as questions rather than as a design, because proposing one would be the half-designing
> §43 declined to do.

| Item | Gates | §ers | What it answered |
|---|---|---|---|
| ~~Multiple robots on one compute~~ | Cells — one Jetson, two arms | §43, §87.1 | One kernel each, and a cell that **refuses** the disjointness claim nothing on the machine can enforce. Five faults, and `env.robot` so a skill can ask |
| ~~Safe-state discovery for mobile robots~~ | AMRs at Host; §36.2's reserve budget depends on it | §36.2, §87.2 | A declared worst case over a named area. The live figure is refused **by name**, because it puts a planner inside §36's reserve arithmetic |
| ~~`on_command_loss` per chain~~ | Humanoids with mixed drivetrains | §53, §87.3 | Per §5.1 chain, a missing chain **refused rather than inherited**, and no schema version — the issue's own migration argument turned out to be false |
| ~~Hub-brokered console relay~~ | A robot in a customer's building with nobody on site | §43, §87.4 | A relayed operator is an operator, an estop over a relay is a **request**, and the session's deadline is the robot's |

**What §87 does not do is award a tier.** §87.2 and §87.3 each *refuse* host on a ground that was
previously unexamined, which moves two robot classes from "not designed" to "refused for a named
reason" — and that is the direction Part VII's whole argument runs in. The relay is rules rather
than a transport: nothing on this platform dials out to a Hub yet, and §87.4 is what the thing that
eventually does is held to.

---

## What stays open, and must stay visible

Not caveats — surfaces that have to keep saying this until somebody stands next to a machine.

* **All 19 bench tests.** `missing`, never `skipped`, never absent.
* **OTA against real RAUC on a real A/B device.** The updater is a trait with a recording
  implementation, which is what makes rollback testable and is not the same as having watched one.
* **A signature on a conformance result.** §19 forbids a robot signing, so §12's kernel gate reads
  unsigned evidence. §80 says the Hub-countersigning shape is a *different* claim from what §15 asks
  for and must not be built as though it were the same. Undesigned.
* **Task success after retargeting.** Measure it via §56; never promise it.
* **`proximity` is inert on every robot.** Every declared input resolves and nothing fills
  `cycle.nearest_person_m`. Perception is a package, not the OS (§58.3), so this waits on somebody
  shipping the package — which means even a Host-tier robot under this plan does not enforce
  distance to a person.
* **EtherCAT's distributed clocks and FSoE.** `drivers/ethercat` now passes under SOEM, and every
  slave there says `Has DC: 0` because a software segment has no propagation delay to measure. The
  cross-check is interoperability, not the Certified ceiling's basis; it moves nothing in this line.
* **A frame budget is what makes §89.2's suspension safe.** A suspended stream leaves its last frame
  in the pool, and a policy holds on it only if its lease declares a frame budget (§25.1) — which
  arrives with a package, and which a hand-run `skynetp` does not have (F50). The loop now says so on
  its summary line rather than printing `0 too old`.
* **§87.4's relay has no transport.** The rules exist, in four editions and the corpus; nothing
  dials out to a Hub, so no session has ever been opened, held a lease, expired, or had a stop
  request cross one. What is built is what the thing that eventually does will be held to.

**The standing rule.** A dry run may report `pass` only for a check that executed. Everything else
is a named absence. §15's own sentence is the one to hold the work to: a conformance runner that
reported `pass` for a test it did not perform would be the most damaging component this platform
could ship.

---

## Decisions, and who made them

The owner answered item 5 and delegated the rest on 2026-09-16. What follows is what each was
decided as and why, kept in place of the questions so the next reader sees the ruling rather than
re-opening it. Where a decision produced work, it is in *Next*.

1. **The §73 corpus figure edit stands.** §73's own paragraph says every figure in it is *measured,
   and checked*, and `scale-report --check` refuses with *"correct the document, not this check."* A
   count of what is in the tree is a derived fact, not a design statement, so bringing it up to date
   is maintenance. It was done in both copies and re-pinned with `--pin`, which refuses unless the
   two agree, so the mechanism that the rule exists to protect did its job. See 10 for the general
   case.

2. **`notifies_operator` gets a surface.** Its two siblings are serialised and rendered and this one
   is not, and the argument that `requires_safe_state` already covers it does not survive §36.1
   having written three rows rather than two. A robot that has stopped taking work and a robot that
   has stopped taking work *and owes somebody a phone call* are different situations. Wired, oracle
   first, then the corpus, then the editions.

3. **Both functions whose comments name absent callers are deleted.** `check_code` is a third
   spelling of a check the CLI and the daemon each already make directly; `tier_profile_to_json`
   renders a model for a surface nobody built. Deleting is the answer that leaves no claim behind:
   building a surface to justify a function is the tail wagging the dog, and a comment asserting a
   caller is the same defect as a figure asserting a measurement.

4. **`offline.control.independent` stays a `bench` test.** F2 asked whether it is `bench` because it
   needs *hardware* or because it needs *a machine rather than a model*, and the answer is the
   second — but that is an argument for letting a virtual robot **produce bench evidence** for it,
   not for reclassifying it as `sim`. Reclassifying would let a run against a pure fake claim it,
   which is the opposite of what the distinction is for. No specification change; the work is in
   *Next* instead, and it is the one item that would move the dry ceiling off `none`.

5. **ONNX Runtime is allowed, narrowly. — the owner's call.** The no-third-party-dependency rule
   protects a stated thing: a vendor building a driver years from now should not need a package
   registry to have survived. An inference backend is neither the kernel nor a driver, so the
   carve-out is written to that shape rather than as a general relaxation: the dependency lives in
   one crate, behind a feature that is off by default, the safety kernel stays `no_std` with no
   allocator and no dependency, and `ci.yml`'s check is narrowed to permit exactly that package and
   nothing else rather than switched off.

6. **Phase 3's specification fields wait for Phase 1.** Motion primitives are real design, not a
   count, and designing new transport surface ahead of a loop that has never closed is how a
   specification acquires fields nobody needs. Revisit when Phase 1's gate is met.

7. **"No evidence" and "evidence, and it failed" are rendered apart.** §80's `inert` precedent is
   the one on point: folding a third state into an existing one was the tempting simplification and
   the wrong answer, because the two send a person to do different things. The verdict already
   carries the distinction — `missing` non-empty with `failures` empty is one, `failures` non-empty
   is the other — so this is a rendering fix and not a wire change.

8. **§19's driver-activation rule gets one named spelling, and the accessor goes.** The enforcement
   point is the inline check in `runtime.rs`; `driver_may_activate` reads the same fact off a report
   that `main.rs` builds to *print* rather than to gate. Neither copy survives as it stands: the
   predicate moves onto `SignatureState` in the contracts crate, where it is one named fact, and the
   accessor that duplicated it is deleted.

9. **`skynet blackbox` reports replay completeness.** Same class as 2, and the plainest instance of
   it in the tree: the black box can already tell whether a sealed window recorded every field a
   replay evaluates, and presents a replay of a partial window exactly like a replay of a whole one.
   A replay is evidence, and evidence that will not say how complete it is has the shape §15 calls
   the most damaging thing this platform could ship.

10. **Figures in §73 are maintained, not negotiated.** The general rule behind 1: they are
    measurements of the tree, they go stale whenever the tree changes, and updating them in both
    copies and re-pinning is the maintenance that keeps the check meaningful. **Compressing code to
    avoid moving a figure is the thing not to do** — F6 records 0.1 doing exactly that, rewriting
    tests to save lines, and the tests were not improved by it. A design change to §73 is still an
    issue; the numbers in it are not.

The two below were the last items standing as *the owner's call*. Both were delegated on 2026-09-16
with *"decide yourself on these 2"*, and one of them turned out to rest on a fact nobody had checked.

11. **`power_refusal` is deleted.** The unreached `Runtime::power_refusal` read the power gauge itself
    and then delegated to `power_refusal_from`. Two lines, and the wrong two: the reconcile pass at
    `runtime.rs:1757` already carries a comment saying that a pass has two consumers of one
    measurement and that *"reading the gauge separately for each would let a skill be refused a lease
    for a battery level its own precondition had just cleared."* An unreached public function whose
    only behaviour is the thing the code beside it warns against is not neutral — it is the
    convenient call for the next person wiring up a surface, and the defect it produces is a robot
    refusing work for a reading it had just accepted.

    Deleting rather than wiring, and rather than leaving it allow-listed: the allow-list is for
    functions whose absence of a caller is *explained*, and "this one is a trap" is not an
    explanation, it is a reason to remove it. §36.1's enforcement is untouched —
    `power_refusal_from` is the live spelling and sits on the reconcile pass. What it inherited is
    the deleted function's documentation: the §64 argument for naming *which* of §36's two reasons
    withholds a lease was the substantive half, and it now sits on the function that survives.
    A caller that genuinely holds no reading calls `read_power` first, on its own line, where the
    second measurement is visible to whoever reads the pass.

12. **§23.3's runner takes the `dlopen` route, and the carve-out stays unspent.** *Decisions* 5
    granted one narrow third-party dependency and F15 argued neither route fit its terms. F15's
    correction block above retracts half of that: ONNX Runtime's C API is entered through one
    exported symbol, the struct layout is the one the caller *requests*, and an unsupported request
    returns a null pointer rather than a misaligned struct. The hazard that made `dlopen` look worse
    than a dependency tree is therefore a refusal, not a silent misdispatch.

    So the comparison is no longer close, and the route that needs no rule changed wins on its own
    merits: no Cargo dependency, `Cargo.lock` stays exactly the workspace members, `CONTRIBUTING.md`
    line 89 stands unedited, and `ci.yml`'s dependency check is neither narrowed nor switched off.
    Worth being exact about what that rule is, because this plan has been sloppy about it: *"no
    third-party Rust dependencies"* is **`CONTRIBUTING.md`'s own rule and a CI check**, listed under
    *enforced, not requested*. Calling it the owner's preference — as F15 and *Decisions* 5 both do —
    attributed a repository rule to a person, which is how a rule gets waived by somebody who never
    set it.

    **Not built here, deliberately.** The remaining risk is transcription error inside a known
    layout, and the only thing that retires it is running the result against a real library and a
    real model. This machine has neither. Writing several hundred function-pointer entries that
    cannot be executed is precisely the shape F15 was right to refuse, so the route is recorded and
    the work waits for a machine with ONNX Runtime on it. The `Session` seam means that costs
    nothing else.

13. **The amendments in `docs/issues/` are ratified, and Phases 3, 4 and 5 are to be built.** The
    owner's instruction of 2026-09-17 was to finish every section of this plan, and F29 had already
    established that three of its phases wait on nothing but these decisions. A maintainer carrying
    an issue to both copies is exactly what `CONTRIBUTING.md` asks for, and this session has both
    checkouts writable — the mechanical block F29 met is gone. So each amendment is carried into both
    copies of the specification as a section of **Part XII**, re-pinned with `--pin`, and built in the
    order the issue names. Where building one settled something the issue left open, the section
    says so rather than the issue being edited to look prescient.

    Phase 5's issue was written **as questions**, deliberately, because answering them there would
    have been §43's *half-designing*. Ratifying it means answering them, and each answer is a design
    decision recorded in the section that makes it, with the alternative it rejected.

14. **F24's amendment is ratified as §89, and the substrates may be downloaded — the owner's calls,
    2026-09-18.** F24 said in the eighth session that capture on the control thread is *"an ABI
    question, not a patch"* and that the fix is an issue; the twelfth session filed it as
    `docs/issues/sensor-reads-off-the-control-thread.md`, recommending a per-sensor
    `concurrent_read` declaration and a rule that capture yields to control. Asked, the owner
    ratified it to be carried into both copies and built in the issue's order, exactly as 13 did for
    the other four. In the same exchange the owner permitted downloading what the remaining Phase 1
    and Phase 2 items need — vendor SDKs, a simulator, ONNX Runtime for Linux, container images —
    into user space and containers, never system-wide.

15. **Phase 6's last three owner's calls — the owner's, 2026-09-19.** Asked with a recommendation
    each, and each answered as recommended:

    * **F66: the gate's sentence is corrected, not the award logic.** Point 4 now reads *"`guest`
      with fifteen `missing` and zero `failures`"*, and the corpus case
      `everything-dry-claiming-host-reads-fifteen-bench-tests` pins it in all eight editions. The
      alternative — reporting cumulative gaps across tiers so all sixteen show together — would
      have been a design change to §15's walk for the sake of one sentence in this file.
    * **F65: the tree follows the specification.** Every `stamp_ns` and both loop clocks move to
      `CLOCK_MONOTONIC_RAW`; §24 and §35.1 are untouched. The alternative, amending the time base
      to `CLOCK_MONOTONIC`, would have reversed §35.1's own reason for the choice — NTP — to save
      seven one-line driver edits. Doing it found F67, which is the half that mattered.
    * **The teaching drivers declare the stop they implement.** `mock`, `mujoco` and `pendant` all
      latch, show the latch and refuse later commands, so each declares `safety.estop` in F63's
      vocabulary — the software stop — with a null envelope to keep it from reading as the rated
      one. `pendant`'s `control_modes` blocker stays, by §59.2.

### F31 — `main`'s corpus job had been red since the eighth session, and the reason given for it was a different one.

The eighth session's entry reads *"`node tools/run-ci-job.mjs corpus` 9 of 12 — `zig` and `dotnet` are
not installed on this machine, and the edition-figures check fails downstream of the two it could not
measure."* The first half is true. The second is not: `edition-report.mjs --check` runs the four
**complete** editions and never touches Zig or .NET (F8 already says so). It failed because
`9037667` added eight `live` cases, moving the corpus from 1,202 to 1,210, and
`reference/reports/editions.json` still said 1,202 — so the step compares a fresh run against a stale
pin and refuses, on every machine, whatever is installed.

It is F7's shape a third time — a failure attributed to the runner that was a fact about the tree —
and the same remedy found it: the toolchains CI names, installed where the check runs. With Zig 0.16,
Node 24, Go 1.24, a JDK 21 and .NET 8 in WSL, all eight editions ran and the stale pin was the only
thing left to fail. Regenerated, it now says 1,270.

### F32 — §73's corpus-file count was being read out of a stale row, and agreed by coincidence.

`scale-report.mjs` takes the **first** `a corpus of N files` in §73. That was not the sentence under
the table but a row inside it — *"two editions, Rust and Python, each passing all 1,148 cases of a
corpus of 80 files"* — written when the corpus had 80 files and never updated. The corpus still had 80
files, so the check read a stale row and passed. The first file this session added made the check
fail against the row rather than against the sentence it exists for.

The row was stale in three other ways — two editions where there are eight, 1,148 cases where there
were 1,210, and a *"still missing"* column asking for a third edition that exists — and the fix is to
rewrite it without restating any figure, so the sentence the check is written for is the only record.
One fact, one place, which is §73's own argument for having the check at all.

### F33 — the Hub reads a policy's declaration with its own parser, and nothing held it to the robot's.

`lib/skynet/robots/transport.ts`'s `parseSourceEmbodiment` documents itself as reproducing
`source_from_json` exactly — the Rust reader a robot runs at install — and it does, field by field. It
is not a mirrored contract, so `check-contract-mirrors` cannot see it, and no corpus case passes
through it. Adding `motions` to the Rust reader and the planner and not to this function would have
produced a Hub that refuses a motion policy as *undeclared* while every robot binds it, with every
check green.

The gate is a Hub test whose fixture is typed `{ [K in keyof Required<SourceEmbodiment>]: unknown }`,
so a field added to the mirrored contract fails the Hub's type-check until the fixture names it, and
then fails the assertion until the reader carries it. Held to a control: the reader as it stood before
this session fails the new test, and the current one passes.

### F34 — the vocabulary §85 was written to create already existed, with users, in the other repository.

`docs/issues/capability-token-vocabulary.md` proposed seven domains — `manipulation`, `locomotion`,
`sensing`, `hri`, `compute`, `network`, `storage` — a two-segment grammar, and an enumeration per
domain. It was a reasonable list and it was the wrong one, for a reason no reading of the
specification could have surfaced: **the Hub has enforced a different list since before §8 was
written.** `lib/skynet/robots/interface.ts` refuses a robot registration naming a token outside its
`CAPABILITY_TOKENS`, `POST /v1/robots` has been calling it for months, and every robot in the
registry carries those spellings.

So ratifying the issue as proposed would have produced two vocabularies for one thing — which is the
precise failure the section exists to prevent, arrived at by writing the section. §85 adopts the
interface's list instead, plus the tokens this document gates a rule on and the interface has no word
for. What the issue's own draft cost is visible in the corpus: `a-domain-the-issue-proposed` pins
`compute.accelerator` as a **vendor** token, and `a-modality-is-not-a-token` pins `sensing.rgb` —
§5's modality spelling, and the one a reader reaches for — as `unknown` rather than canonical,
because the interface spells it `sensing.camera.rgb`.

**The mechanical half is the part worth keeping.** The two lists are now one vocabulary written in
two files, because the Hub's carries a label per token that the OS contract has no use for, and the
digest mirror cannot see a second file. `lib/__tests__/skynet-os-mirrors.test.ts` holds them to each
other token for token *and in order*, and a second case refuses a token with no label — the way an
operator ends up choosing between `sensing.audio` and a sentence.

The proposal's two-segment grammar was the same mistake one level down: `sensing.camera.rgb` has
three segments and has been a registered capability for months, so a length rule would have refused
tokens robots already carry. Membership in the list is the whole rule, and the first pass of the
editions had written the length rule into all seven of them — Go, C++ and Python failed 35 cases each
the moment the reference was corrected, which is what the corpus is for.

### F35 — the vocabulary as first built refused the token §49.2's privacy rule is written about.

§49.2 is a page about microphones: *"`sensing.audio` is a capability token gated like any other […]
the default is denied and the grant is explicit and audit-logged"*, and §81 re-states it. The
interface has no microphone token, so the list adopted from it had none either, and admission would
have refused any manifest declaring one — making the rule §49.2 states unreachable by the only route
that reaches it.

Found by scanning the specification for every backticked string shaped like a token and comparing the
result against the enumeration, which took one command and should be the reflex whenever a closed set
is adopted from somewhere else rather than derived from the document. Three of the four strings it
found were already in the list; the fourth was this one.

`sensing.audio` is now canonical, and it is the fourth token §85 adds to the interface's list for the
same reason as the other three: this document gates a rule on it. The corpus case names §49.2, so the
next person to prune the vocabulary has to argue with the section rather than with a list.

### F36 — a per-channel ceiling took precedence over the whole-body one, and §86 is what made it reachable.

`ceiling_for` read `per_channel` first and returned it, falling back to the base-velocity figure for
the reserved `base` channel and to nothing otherwise. So a joint rated 2 rad/s on a robot whose
operator had narrowed the body to 1.5 was clamped at **2** — the wider of the two — which is the one
thing `narrowest` exists everywhere else in the kernel to prevent.

It had never been wrong, because it had never been reachable: the whole-body figures reached no
channel at all, and the comment beside the function said why (*"nothing at this layer knows whether a
given channel carries radians per second or newton-metres"*). §86.3 gives that layer the units, and
the moment it does, precedence becomes a live defect. The two now compose by narrowing, and
`a-velocity-joint-takes-the-narrower-of-the-two` is the case that fails without it.

**The general shape is worth more than the fix.** A number that is resolved, rendered and applied to
nothing is not inert — it is a defect waiting for somebody to connect it, and it reads as enforcement
on every surface in the meantime. `joint_velocity_max_rad_s` and `joint_torque_max_nm` were narrowed
through four layers, shown on the limits page, and reached no joint; an operator who tightened a
robot's velocity ceiling in a profile changed a number on a screen. The audit that would find the
next one is not "which functions are unreached" — which this repository already checks — but *which
resolved values are unread*.

### F37 — the mock driver's own suite had been commanding a position-only joint in torque.

`drivers/mock` declares three joints, and `J3` is `"control_modes":["position"]` while the other two
take torque. Its selftest sent `mode = SKR_MODE_TORQUE` with all three joints unmasked, the driver
accepted it, and the check was called *"accepts a well-formed command"*.

It was not well formed. The mock is a **mixed-mode body** — the very thing §86 is about — and nothing
noticed because §24 had no way to say so and the driver never compared a command's mode against its
own `control_modes`. Two of its checks then passed for the wrong reason: the estop refusal was a
refusal either way, and the clamp check was clamping a position setpoint against a torque rating,
which is a number bounded in the wrong units.

Found by building §86 rather than by reading the driver: the moment `write_command` checked the mode
against each joint's own list, two checks went red on a driver that had been green for its whole
existence. The suite now sends a per-joint command, and a body-wide torque command against this body
is a case of its own — a control that fails if the check is removed.

## Next

Everything below is decided and unblocked. In order.

1. ~~**The small ones, together**~~ — **Done**, 2026-09-16 (third session). *Decisions* 3, 7, 8 and
   9. `check_code` deleted and `tier_profile_to_json` kept, for the reason the third session's entry
   gives; §19's activation predicate is `SignatureState::admits`; `skynet blackbox` says whether the
   window it replayed was complete; "no evidence" renders apart from "evidence, and it failed".

2. ~~**`notifies_operator`**~~ (*Decisions* 2). **Done**, 2026-09-16 (third session) — oracle, the 28
   power cases, four editions, then the CLI.

3. ~~**Phase 1**~~ — **complete**, 2026-09-16. Its gate was met in the fifth session; the last row of
   its table was filled in the seventh.
   * ~~**§23.3's own runner: an ONNX Runtime session.**~~ **Built** — `skynet-ort`, the `dlopen` route
     from *Decisions* 12. No Cargo dependency, no `CONTRIBUTING.md` waiver, no change to `ci.yml`'s
     dependency check: `Cargo.lock` still holds exactly the 26 workspace members. The residual risk
     the decision named — transcription inside a known struct layout — was retired the way it said it
     had to be, by executing it against ONNX Runtime 1.30.0, and it found a real defect on the way
     (F20). What remains unbuilt is LibTorch and any provider but CPU, both deliberate.

4. ~~**Bench evidence from a virtual robot**~~ (*Decisions* 4). **Done**, 2026-09-16 (eighth session).
   `skynet_contracts::bench` is the format — a record bound to the digest of the artefacts it was
   taken against, §12's `BenchPassStale` in general form — and `skynet bench record` and
   `skynet bench award` are the two ends. `check-offline` writes one over the three binaries that
   decided and reads it back, so the offline run is evidence recorded rather than a property
   demonstrated, and `offline.control.independent` is no longer among an award's `missing`.

   **It did not move the dry ceiling, and F21 is why.** The item was described here as the one that
   would, and that was wrong by two: guest needs ten tests, one was the bench one, and of the nine
   `sim` tests left four have no producer that writes to a file and one *fails* by design. The
   remaining work is named in F21 rather than implied.

5. ~~**A check on doc comments that name a caller**~~ (F9). **Done** — the same tool, and it found two
   live claims on its first run: `revoke_loans` said *"called by the runtime"* (F18) and `asks_for_us`
   said *"for the responder half"* while the responder had moved to a finer question. Eight controls,
   including two negative ones, hold the line between a claim and a purpose: a **definite** article in
   front of something in this workspace is checkable, an indefinite one is not, so *"for a report"*
   passes and *"for the responder"* does not. There is deliberately no allow-list for this half — F9
   records what one costs.

6. ~~**`revoke_loans`' signal**~~ (F18). **Done**, 2026-09-16 (eighth session). The decision was
   which process owns the fact, and the answer turned out to be neither of the two the finding
   named: **the kernel owns it**. `SO_PEERPIDFD` (Linux 6.5) on the accepted socket returns a
   *descriptor* for the peer, so the process that holds the pool learns the fact directly, about the
   process that actually received the descriptors, with no cross-daemon channel and no pid to be
   reused. `pass::peer_pidfd`, `Port::reap_departed`, `Camera::revoke_pool_loans`, and `skynetk`
   calls it on that answer and no other.

   The rule is all-or-nothing — the loans region carries one claim per slot and **no owner**, so
   there is no revoking one reader's — and a hand-over the kernel would not name disables it for the
   run, which `skynetk` prints rather than leaving to be inferred from a pool that never recovers a
   slot. Held to two controls: `any` in place of `all` fails the middle assertion, and forgetting to
   forget the generation fails the last.

7. ~~**Phase 2's four dry-verifiable drivers**~~. **Done**, 2026-09-16/17 (eighth session).
   `drivers/pendant` (§59.2's program-level body, 37 checks), `drivers/canopen` (CiA 402, 61 checks),
   `drivers/ur_rtde` (against URSim, 35 checks) and `drivers/ros2_control` (against
   `mock_components`, 42 checks). Each reaches **guest** and stops there by mechanism, each names
   the property its substrate cannot show, and three of them taught something the design had not
   anticipated — F26, F27 and F30.

8. ~~**§87 — Phase 5's four class gates**~~ and ~~**§88 — `execute` to a command-level driver**~~.
   **Done**, 2026-09-17/18. Both were named by the tenth session as the two sections of Part XII it
   did not reach, and both were *"blocked on nothing"*. §87 answers §43's four questions with the
   alternative each one rejected; §88 finds that Phase 4's second blocker dissolves rather than
   needing a bridge. Every file in [`docs/issues/`](docs/issues/) is now ratified.

9. ~~**F21's producer**~~ — `skynet conformance --results`. **Done**, 2026-09-18 (twelfth session),
   together with the producer F21 did not know was missing (`corpus-report --results`, F42) and the
   defect the two of them exposed in `skynet bench award` (F43).

10. ~~**Phase 2's three unbuilt rows**~~ — EtherCAT, Unitree SDK2, `libfranka`. **Done**,
    2026-09-18 (thirteenth session): `drivers/franka`, `drivers/unitree`, `drivers/ethercat`, each to
    guest and each with its un-dry-verifiable half named. Previously read: *in progress*.

11. ~~**Phase 1's ONNX loop**~~ — `check-loop` with a `kind: "onnx"` policy in place of the `mlp`
    one. **Done**, 2026-09-18 — by the thirteenth session (`4e7fb3f`, `make check-loop-onnx`), which
    never logged it, and again by the fourteenth with MuJoCo as the body. Previously read: *open*.

12. ~~**F24's issue**~~ — the finding says the fix *"is a specification change and therefore an
    issue"*, and `docs/issues/` never had one. **Filed**, 2026-09-18, as
    `docs/issues/sensor-reads-off-the-control-thread.md`, **ratified as §89** (*Decisions* 14), and
    **built, all six steps**: §89.1–§89.3 by the twelfth session, step 5 and the host half of step 6
    by the thirteenth, and `drivers/mujoco`'s snapshot render with `check-loop-mujoco` re-run by the
    fourteenth — the measurement the issue said should show it closed, and does.

13. ~~**F48's cross-check**~~ — SOEM, an independent master, against `drivers/ethercat`'s slaves.
    Named by the thirteenth session as *"available and was not run"* for want of `CAP_NET_ADMIN`.
    **Done**, 2026-09-18 (fourteenth session), inside `unshare -rn` over a veth pair; F51–F53 are
    what it took.

> **Not so, 2026-09-18 (twelfth session).** The paragraph below said everything this plan listed was
> done. Phase 2's table gives each of its three unbuilt rows a *dry substrate* and a *dry-verifiable*
> column, and the paragraph's reason for leaving them — *"each needs the vendor's software or its
> silicon"* — is true of the silicon and not of the software: `libfranka` and `unitree_sdk2` are open
> source, exactly as URSim and `ros2_control` were when earlier sessions installed them. Items 9–12
> above are what an audit of this file against the tree found still owed. Kept rather than rewritten.

> **Done, 2026-09-18 (fourteenth session), and this time the audit agrees.** Every item under
> *Next* is struck through, Phase 2's table has no unbuilt row, and the two things the thirteenth
> session left — §89's MuJoCo half and the SOEM cross-check — were each blocked on a property of that
> session's container (no offscreen GL; no `CAP_NET_ADMIN`) rather than of the work, which is F3a's
> lesson for the third time. What remains is the list under *What stays open*: a robot, a vendor, a
> Hub, or a research result. None of it is dry work, and none of it is plumbing.

**Everything this plan listed is done, and what is left is not dry work.** Phase 2's three remaining
rows are EtherCAT and two vendor SDKs, and each needs the vendor's software or its silicon.
`drivers/ros2_control` needed a machine with `ros2_control` on it rather than a robot, and this
session installed one: its suite has now run, and F41 is what the first run found. Everything else
that remains is under *What stays open* and has been since the plan was written: it needs a robot, a
vendor, or a research result nobody has.

**The plan's name has outlived its constraint twice over.** F29 said the later phases were gated on
design rather than on hardware, and the three sessions since have spent their time on
specification, editions and surfaces with the robot never once being the blocker. What the name
still describes accurately is the *ceiling*: `none`, for the reason F1 established and F21 refined,
and no amount of the work above moves it.

## Progress log

Newest last. Each entry: what was audited, what was found, what changed, what is verified — and
what is not.

### 2026-09-16 — Phase 0 opened, and closed

**Audited.** `CONTRIBUTING.md`, `BUILD-LOG.md`, `docs/README.md`, `docs/mirror-digests.json`,
`reference/typescript/mirror-digests.json`, §1, §5, §6, §7.1, §15, §54, §56.2, §58–§62, §66–§67, §80,
`.github/workflows/ci.yml`, `crates/skynet-contracts/src/certification.rs`,
`corpus/certification/award.json`.

**Found.** F1–F6 above, F1–F4 before any code was written. Two cancelled planned work — 0.4 was
already built twice, and most of 0.1's proposed machinery was already an invariant — and one made this
plan's central claim wrong in the direction that matters: the dry ceiling is `none`, not `guest`, and
a single row of §15's table is why.

**Changed.**

| File | What |
|---|---|
| `crates/skynet-contracts/src/certification.rs` | +71. Two tests pinning the dry ceiling and the suite split it is computed from. No behaviour change, so the corpus is untouched |
| `tools/check-production-callers.mjs` | New. 0.2's check |
| `tools/production-callers-allow.json` | New. Its baseline — 38 names, 12 answered, 26 debt |
| `.github/workflows/ci.yml` | One step in the `invariants` job |
| `reference/reports/scale.json` | Regenerated, and **required**: `scale-report.mjs --check` holds the pinned file to what a run produces, so 71 lines of Rust have to move the pin. Reverting it fails the check — confirmed by reverting it |
| `DRY-COVERAGE-PLAN.md` | This file |

**Verified.** The full pre-PR gate from `CONTRIBUTING.md`, on the final state:

```
cargo fmt --all --check                                      exit 0
RUSTFLAGS="-D warnings" cargo clippy --workspace --all-targets exit 0, 0 warnings
cargo test --workspace                          1,769 passed, 0 failed
node tools/check-mirrors.mjs                    the specification matches its pin
node tools/check-contract-mirrors.mjs           24 mirrored contracts match
node tools/check-golden-path.mjs                exit 0
node tools/check-rhal-flags.mjs                 exit 0
node tools/scale-report.mjs --check             exit 0  (§73 matches the tree)
node tools/check-production-callers.mjs         940 checked, 38 allow-listed
node --check console.js / fixture-serve.mjs     exit 0
```

Both new checks were held to a **positive control**, per `CONTRIBUTING.md`'s rule that a check which
has never failed has proved nothing:

* Reclassifying `offline.control.independent` from `bench` to `sim` fails both 0.1 tests. Reverted.
* 0.2 fails on each of its three refusals independently — a new unreached `pub fn`, an allow-listed
  name that has gained a caller, and an entry with an empty reason. All three reverted.

Writing 0.2 also found a bug in 0.2: `#[cfg(test)] mod tests;` puts a whole *file* behind the
attribute and the first version scanned each file alone, so `skynetd-io/src/tests.rs` — nine thousand
lines of tests — was read as production surface. Fixing it *raised* the finding count from 35 to 38,
because five functions reached only from that file had been counted as reached.

**Not verified, and not claimed.** No robot, virtual or physical, was brought up: `virtual-robot.sh`
needs Linux with systemd as PID 1 and this is Windows. So `skynetd-io`'s end-to-end systemd run and
the `#[ignore]`d driver-loading suites did not execute here — CI runs both. The three tests added
prove an arithmetic property of the award function, not that any robot reports it.

### 2026-09-16 (second session) — Next 1 and 2 done, on a machine that could run all of it

Carried out on Linux with every edition toolchain present, which is the difference between this
entry and the one above. Branch `claude/magical-darwin-igdkl1`, rebased onto `main` at `682d9b0`.
**Nothing pushed**, in either repository.

**Audited.** CI runs 55–60 on `main` and the logs of all three failing jobs; `CONTRIBUTING.md`'s
gate against what CI actually runs; `certification.rs` and the TypeScript oracle's `conformance.ts`;
`corpus/certification/award.json`; `tools/corpus-gen/generate.ts`; `scale-report.mjs` and
`edition-report.mjs` and what each of them pins; `docs/README.md`'s mirror procedure and both copies
of the specification; `editions/go/cmd/selfcheck` and `internal/fusionprobe`; `strip()` in
`check-production-callers.mjs`; and the declaration and call sites of all 23 untriaged names.

**Found.** F7–F10. In order of how much they change: `main` was **red the whole time** and the gate
could not have said so (F7); 0.2's check reported the wrong line 18 times in 38 and was blind to a
real caller it had already denied in writing (F9); §36.1's operator notification is built on both
sides and joined by nothing (F10); and F3a's two blockers were properties of a laptop rather than of
the work, while the real one is a single exact figure in the pinned specification (F8).

**Changed.** Six commits here, one in the Hub.

| File | What |
|---|---|
| `crates/skynet-ring-shm/src/pass.rs` | `[b'R']` → `*b"R"`. clippy 1.98's `byte_char_slices`, the first of three reasons `main` was red |
| `tools/unicode-case.ts` | `rows()` takes `readonly string[]`; it reads `.length` and `.slice` and nothing else. The second |
| `editions/go/internal/fusionprobe/probe.go` | A second control, `Intrinsic`, calling `math.FMA`. The third |
| `editions/go/cmd/selfcheck/main.go` | `fusingTargets` re-measured and its comment corrected; the finding message names both remedies, because the listing cannot say which route produced the instruction |
| `tools/corpus-gen/generate.ts` | +3 cases. `sim-only-awards-none`, `sim-only-plus-the-one-bench-test`, `sim-only-claiming-host-still-reads-guest` |
| `corpus/certification/award.json` | Regenerated. 20 cases → 23, purely additive, no existing case perturbed |
| `ci.yml`, `editions/{cpp,go}/Makefile` | The three complete-edition ratchets, 1199 → 1202. The four partial editions' floors are unchanged, which is what they are separate for |
| `reference/reports/{editions,scale}.json` | Regenerated. Rust is still 136,498 lines, so F6's two-line headroom is untouched — nothing here added Rust |
| `docs/skynet-os-architecture.md` | **The pinned specification.** One figure, in both repositories, re-pinned with `--pin`. Ratified as *Decisions* 1 |
| `docs/status.md` | 1,199 → 1,202, twice |
| `tools/check-production-callers.mjs` | Removed spans keep their newlines; character literals are consumed as units |
| `tools/production-callers-allow.json` | 38 entries → 37. `upload_target` removed because it was never unreached; the 23 baseline entries replaced with arguments |

**Verified.** The whole gate, on the final state, at CI's own toolchain — stable moved 1.94 → 1.98
here first, which is what made F7's first row reproducible rather than theoretical:

```
cargo fmt --all --check                          exit 0
RUSTFLAGS="-D warnings" cargo clippy             exit 0, 0 warnings
cargo test --workspace                           1,857 passed, 0 failed
npx tsc -p .                                     exit 0
node tools/check-mirrors.mjs                     the specification matches its pin
node tools/check-contract-mirrors.mjs            every mirrored contract matches
node tools/check-golden-path.mjs                 exit 0
node tools/check-rhal-flags.mjs                  exit 0
node tools/check-production-callers.mjs          942 checked, 37 allow-listed
node tools/scale-report.mjs --check              exit 0  (§73 matches the tree)
node tools/edition-report.mjs --check            4 editions, 1202 cases each
node tools/corpus-gen/generate.ts                regenerates the committed corpus byte for byte
```

And **all eight editions**, which is the thing the machine above could not do:

```
rust    1202/1202      python  1202/1202      cpp   1202/1202      go     1202/1202
c        354 passed    zig      315 passed    java   315 passed    csharp  315 passed
```

Every check that changed was held to a positive control, per `CONTRIBUTING.md`:

* **clippy** — reproduced at 1.98.1 before the fix, clean after.
* **tsc** — TS4104 reproduced before, clean after.
* **the fusion scan** — a planted `math.FMA` under `internal/` is refused on all six targets; a
  planted `x*y + z` on five and **not on amd64**, which is the same measurement arriving from the
  other direction. Both reverted.
* **the corpus cases** — the Python edition mutated to treat an absent `offline.control.independent`
  as satisfied, which is the defect these cases exist for, fails `sim-only-awards-none` at byte 12
  — where `none` and `guest` differ — while `sim-only-plus-the-one-bench-test` keeps passing because
  its bench result is present. The pair isolating the row rather than the suite. Reverted.
* **the line-number fix** — 18 of 38 reported lines did not open their own declaration before, 0
  after, with the same 38 findings both times.
* **the character-literal fix** — the ratchet itself was the control: with the scan able to see,
  `upload_target` gained a production caller and the check failed until the name came off the list.

**Not verified, and not claimed.** No robot, virtual or physical. `packaging/virtual-robot.sh` needs
systemd as PID 1 and PID 1 here is a container supervisor, so §33.1's units never started, the
`#[ignore]`d driver-loading suites did not run, and `skynetd-io`'s end-to-end systemd run did not
either — CI runs all three and they are green there. The three corpus cases prove that four
independent implementations compute the same award from the same evidence; they say nothing about
any robot reporting it, and F1's ceiling is still arithmetic rather than a measurement of a machine.

**Still true, and worth repeating because this entry is mostly good news.** All 19 bench tests are
`missing`. `proximity` is inert on every robot. Nothing has been signed by a Hub. The dry ceiling is
`none`, and three more editions now agree about it — which is a stronger statement that the number
is right and not a weaker one that it is high enough.

### 2026-09-16 (third session) — the decisions taken, and Phase 1's capture half

The owner answered *Open for the owner* 5 — **ONNX allowed, narrowly** — and delegated the other
nine. That section is now *Decisions, and who made them*. Six more commits here, one in the Hub,
**nothing pushed**.

**Changed.**

| Decision | What happened |
|---|---|
| 3 | `check_code` deleted. **`tier_profile_to_json` kept** — see below |
| 7 | The `no tier` headline says which of the two causes it is, extracted so the four shapes are testable |
| 8 | §19's activation predicate moved onto `SignatureState::admits`; the report accessor stays for the `NotRun` arm it alone carries |
| 9 | `skynet blackbox` reports whether the window it replayed was complete |
| 2 | `notifies_operator` reaches the power document and then a surface — oracle, 28 corpus cases, four editions, CLI |
| 5 | Phase 1's **capture half** built. The inference half is unstarted |
| 1, 10 | §73's Rust figure moved 136,000 → 137,000 in both copies, as maintenance rather than as a negotiation |

**Two decisions were wrong when made, and reading the code is what corrected them.**

*Decision 3 said delete both.* `tier_profile_to_json` is **the only place in the workspace that
renders `SAFETY_CLAIM_RULE` into an output** — the kernel declares the sentence, CI checks its text
is intact, and nothing else emits it. Deleting it would have left CONTRIBUTING.md's rule with
nothing implementing it and no check able to notice, which is a safety claim quietly weakened by a
tidy-up. Its comment named a surface that does not exist; the comment was the defect.

*Decision 8 said fold two copies into one.* They are not two copies. `driver_may_activate` also
refuses a report that never ran, which `SignatureState` cannot express and which is not the same
fact. The **predicate** was the duplicate and is now written once; the structural duplication —
`runtime.rs` deciding activation without ever building a `Report` — is still there and is a larger
refactor than §78's remedy asked for.

### F11 — the first capture backend found the release rule wrong, in the worse direction.

`Delivered` told a caller to release `displaced` **and** `not_recorded`, and said that was
*"everything this call took ownership of and nothing it still holds"*. It is wrong about the second.
`LatestSlot::publish` always accepts, so `not_recorded` is the frame that was **just published** —
handed back only to report that the recorder refused it, while the slot holds that very buffer as
the call returns. A backend following the documentation hands a live buffer back to the device.

The two failure directions are not symmetric and this is the bad one. A pool that runs dry stops
capture and looks like a driver fault, which somebody investigates; a buffer freed while a consumer
reads it is a policy acting on a frame something else is writing, on a zero-copy path with nothing
to catch it, three hours in.

The fact underneath is that §47.4 gives one frame to two consumers, so a buffer has up to two
holders and **no lend-and-return scheme can say when it is free**. A reference count answers it,
which is what `dmabuf` does with an fd refcount and what V4L2 with a GPU and an encoder on one
buffer does underneath. `Source::retain` now says so in the trait.

Worth recording as a coverage finding rather than only as a bug: this crate had seventeen tests, a
`no_std` discipline, and a doc comment for every public item — and the defect was in **what the
documentation told the first caller to do**, which nothing that existed could have caught, because
there was no caller. It is the exact argument 0.2's check makes, arriving one level up.

**Verified.** The full gate at CI's toolchain, and all eight editions on the corpus:

```
cargo fmt --all --check / clippy -D warnings / cargo test    1,866 passed, 0 failed
npx tsc -p .                                                 exit 0
check-mirrors / check-contract-mirrors / golden-path         exit 0
check-rhal-flags / check-production-callers                  944 checked, 33 allow-listed
scale-report --check                                         §73 matches the tree
rust python cpp go   1202/1202      c 354   zig 315   java 315   csharp 315
Cargo.lock                                                   24 packages, all workspace members
```

Controls, each failing the suite and each reverted: `Untrusted` made to admit a driver; releasing
the refused frame; forgetting the recorder's retain; leaking the displaced frame. The corpus's
power cases were re-derived from the oracle rather than edited, and the four complete editions were
each brought to them separately.

**Not verified, and not claimed.** Still no robot, virtual or physical — PID 1 here is a container
supervisor, so `virtual-robot.sh` did not run and neither did the `#[ignore]`d driver-loading
suites. The capture backend has never seen a camera and does not claim to: what it demonstrates is
the §47.4 discipline, which is a claim about frames and not about pixels. **No policy has closed a
loop**, so Phase 1's gate is not met and nothing here moves the dry ceiling off `none`.


### 2026-09-16 (fourth session) — `invariants` was still red, and Phase 1 resumed

Carried out on Windows 11 with WSL2 Ubuntu 26.04 beside it — **systemd is PID 1 in that WSL**,
which is the first machine in this log that has it. Rust 1.98.1 on Windows (the CI toolchain), 1.93.1
in WSL, Node 26, Go, .NET, JDK 25; no Zig and no Node 24. **Nothing pushed.** *In progress — this entry
is updated as the session goes.*

**Audited.** Every step of `ci.yml`'s `invariants` job, run locally one by one — the shell steps under
WSL's GNU tools, the Node steps on Windows — rather than the pre-PR gate, because F7's lesson is that
the gate is not what CI runs. Then the `rust` job's Linux half in WSL, clippy at 1.98 against
`aarch64-unknown-linux-gnu` from Windows (which type-checks every `cfg(target_os = "linux")` path at
CI's lint level), and the kernel's `thumbv7em-none-eabihf` build.

### F12 — F7 fixed the first red step in `invariants`, and four more were behind it.

F7 recorded `main` red on three counts and all three fixed. The `invariants` job's first failure was
the oracle type-check, and **a GitHub job stops at its first failing step** — so everything behind it
had been unobserved for as long as anything in front of it was red. Run step by step, four more failed
on the commit F7 called green:

| Step | Failing since | What it was |
|---|---|---|
| No string literal has a hole punched in it | 2026-09-04 | **Real.** 38 literals in 16 files, two of them the Safety Kernel's own `can_start` refusals |
| No secret reaches a command line | 2026-09-02 | **Real.** `bundle` and `upload` each spawned their own curl with their own copy of the credential rule |
| A check-in carries no history | 2026-09-03 | **Stale check.** `delta_` matched `max_delta_per_cycle`, a jerk *ceiling* |
| An intervention count is absent rather than zero | 2026-09-03 | **Stale check.** It required a field aed1306 deliberately removed, and its zero test named a field the episode no longer writes, so that half could not fail at all |

The mechanism is the finding, more than the four: a job that stops at its first red step reports one
defect per CI run, and "fixed" becomes "revealed the next one". Every `run:` step in `invariants` and
`corpus` now carries `if: ${{ !cancelled() }}` — each is an independent claim, so each runs whatever
the one before it did, and the job still fails if any did.

Nothing here could see CI itself: the repository is private and `gh` is not installed on this
machine. "Failing since" is the date the offending line entered the tree against the date the check
did, not an observed run.

### F13 — an episode's intervention count forgot a hole as soon as the next pass was counted.

Found while rewriting F12's fourth check. A pass nothing could count set the invocation's count to
`None`, and the next counted pass revived it with `get_or_insert(0)` — so an invocation with an
unwatched interval in the middle reported the passes after it as its whole. The existing test put its
hole on the **last** pass, which is the one ordering the defect cannot reach. §74.3's rule is that
zero and unmeasured are opposite facts; a partial sum is worse than either, because it is a number
quietly smaller than the truth in the field that means the truth.

The same line cast the ring's `u64` count to the episode's `u32` with `as`, so 2³² interventions
arrived as **zero** — the one value a marketplace reads as a perfect run — and two large counts would
have panicked the daemon under the release profile's overflow checks.

**Changed so far.**

| Commit | What |
|---|---|
| `c04f57a` | 38 string literals given their `\` continuations back. No corpus bytes move; none is pinned by an edition |
| `7635741` | `connectivity::curl` is the only spawner; `presentable` is the credential rule, asked before any disk work and again inside `curl`. One test reaches the refusal from all three modules |
| `e043eb6` | The two stale checks measure the code again; `!cancelled()` on every `invariants` and `corpus` step; `episode()`'s doc loses the sentence aed1306 said it had retracted |
| `8b59c0b` | F13. `InterventionCount` — no pass yet, counted, unmeasured — with unmeasured terminal; saturating on both casts |

**Verified so far.** On Windows at 1.98.1: `cargo fmt --check`, clippy `-D warnings` for the host
**and** for `aarch64-unknown-linux-gnu`, `cargo test --workspace` (1,778 before any change here —
Windows compiles fewer tests than Linux), the kernel, ring and capture for `thumbv7em-none-eabihf`,
`tsc -p .`, and all eight Node checks. In WSL at 1.93.1: `cargo test --workspace` 1,868 passed and 0
failed, clippy clean, and every shell step of `invariants` green.

Controls, each failing its check or test and each reverted: dropping the fetch's early credential
refusal; dropping the push's; a `delta_ms` field on the guard report; a literal zero intervention
count; an `unwrap_or(0)` on it; and the previous `skill.rs` against both F13 tests.

### 2026-09-16 (fifth session) — Phase 1's gate met, and the check that reads CI

Windows 11 with WSL2 Ubuntu beside it, as the fourth session. Rust 1.98.1 on Windows (CI's
toolchain), 1.93.1 in WSL, Node 26, gcc, make, python3; no MuJoCo, no ONNX Runtime, and `sudo` in
WSL wants a password, so `packaging/virtual-robot.sh` did not run. **Nothing pushed, no commits.**

**Audited.** The four commits the fourth session's entry does not cover (`a059778`, `feeb0fc`,
`13d5f63`, `dba6417` — the loader's `describe()` length, the Python host's POSIX request, and the
two halves of wiring the camera into the control loop); §23.3 and what it names; `docs/status.md`'s
claims about the producer end; `check.rs`'s account of what `skynet driver check` measures;
`robot_cmd.rs`'s replay path; the doc comment of all thirty-one unreached `pub fn`; every `run:`
step of `ci.yml`'s `invariants` job, run one at a time; `drivers/python-host`'s Makefile and what
`check-capture` already does; and one uncommitted change from a previous session
(`Library::address`, added for an ONNX loader that does not exist — reverted, F15).

**Found.** F14–F19. In order of how much they change: **Phase 1's gate is met** and the two
departures from it as written are F15 and the driver; `main` was red on `invariants` again by exactly
F12's mechanism (F14); §23.3's runner cannot be built under *Decisions* 5's terms as written (F15);
a model reading a quantity the body does not measure holds for ever, which the gate test found by
failing (F17); `revoke_loans` has no sound signal and the convenient one is worse than none (F18);
and three counts §25 keeps had no surface, one of them the number §23.3's own rule is about (F19).

**Changed.**

| File | What |
|---|---|
| `crates/skynet-infer/` | **New crate.** §23.3's decision layer: a `Session` seam, a dependency-free feed-forward runner reading a JSON artefact, `Policy` with seven named holds, and §47.6's frame average-pooled into a declared grid. 20 tests, every refusal among them |
| `crates/skynet-policy-host/src/backend.rs` | `Inference<S>` — the adapter from `Body` and `Sight` to that layer, and the first backend in the workspace whose command depends on a pixel |
| `crates/skynet-policy-host/src/main.rs` | `--backend inference --policy <file>`; the model loaded and checked against the lease **before** a socket is touched; F19's three counts printed; the per-reason hold tally printed |
| `crates/skynet-policy-host/tests/handoff.rs` | A joint command that is a function of a pixel, across two processes. Two controls, both reverted |
| `drivers/python-host/loop.sh`, `Makefile` | `check-loop` and `check-offline` — the gate, and the gate with no network at all |
| `.github/workflows/ci.yml` | Both new steps in `abi`, the second probing for a network namespace the way the daemon step probes for systemd. The caller-check step renamed to what it now checks |
| `tools/check-production-callers.mjs` | Second half: an unreached `pub fn` may not claim a caller in its own documentation. No allow-list, for F9's reason |
| `tools/run-ci-job.mjs` | **New.** Runs a CI job's steps, read out of `ci.yml`. F14 |
| `CONTRIBUTING.md` | The pre-PR gate says that it is not everything CI runs, and how to run the rest |
| `crates/skynet-kernel-host/src/main.rs` | F14's four literals given their `\` continuations back |
| `crates/skynet-frames/src/lib.rs`, `skynet-mdns/src/service.rs` | The two doc comments the new check refused, corrected to what is true |
| `tools/production-callers-allow.json` | Two reasons rewritten — `revoke_loans`' had gone stale in the direction that matters, and `asks_for_us`' sharpened |
| `docs/skynet-os-architecture.md` | **The pinned specification.** §73: 139,000 → 141,000 lines, twenty-four → twenty-five crates. Both copies, re-pinned with `--pin`. *Decisions* 1 and 10 |
| `docs/status.md` | §23.3 is no longer vacant, and what is still not built is named precisely. The closed loop, with the numbers |
| `DRY-COVERAGE-PLAN.md` | This file |

**Verified.** The full gate on Windows at CI's toolchain, and the Linux-only halves in WSL:

```
cargo fmt --all --check                              exit 0
RUSTFLAGS="-D warnings" cargo clippy --all-targets   exit 0, 0 warnings
cargo test --workspace                               1,810 passed, 0 failed  (Windows)
cargo test --workspace                               1,905 passed, 0 failed  (WSL — Linux
                                                     compiles the cfg(unix) suites Windows does not)
node tools/run-ci-job.mjs invariants --wsl           53 of 53 steps pass
node tools/check-production-callers.mjs              962 checked, 31 allow-listed
node tools/scale-report.mjs --check                  §73 matches the tree
node tools/check-mirrors.mjs                         the specification matches its pin
node tools/check-contract-mirrors.mjs                every mirrored contract matches
node tools/check-golden-path.mjs                     exit 0
cargo test -p skynet-policy-host --test handoff      7 passed, 0 failed      (WSL)
make -C drivers/python-host check-capture            51 frames read, 51 published, 0 overruns
make -C drivers/python-host check-loop               the gate — see below
make -C drivers/python-host check-offline            the same, in an empty network namespace
```

The gate's own numbers, from `check-loop`:

```
skynetk   250 cycles, 2 idle, 0 overruns · 51 of 51 sensor reads, 51 published to the pool
skynetp   200 turns, 199 commanded, 1 held (no_frame, the turn before the first frame arrived)
          0 overruns · 199 of 200 turns shown a frame (0 missed, 0 too old)
replay    all 249 evaluated cycles reproduced
          every field this replay evaluates was recorded on every cycle
```

Every check that changed was held to a positive control, and each was reverted:

* **the doc-claim check** — eight cases. Four claim shapes fire (`called by the CLI`, `for the
  daemon`, `The CLI calls this`, `The caller is skynetd`); the same claim with *"Nothing calls this
  yet"* beside it passes; and two that must **not** fire do not — `for a daemon` (indefinite) and
  `Called by the Hub and by a vendor driver` (outside this workspace).
* **the vision-loop test** — the adapter made to drop the pixels fails it at the first assertion;
  the two frames made the same picture fails it at the second. Two different lines, which is the pair
  isolating *the frame reaches the model* from *which frame reaches the model*.
* **the gate script** — the same dropped pixels turn `check-loop` red, and it names the reason:
  `the model commanded on 0 of 200 turn(s) … no_frame 200`.
* **F14's literals** — reproduced by running the `invariants` step before the fix and after.
* **`run-ci-job.mjs`** — its first real run failed a step (`scale-report --check`, against a pin the
  session had moved), which is a check that has failed for a true reason before being relied on.

**Not verified, and not claimed.**

* **No robot, and no simulator either.** The gate's body is the LeRobot bridge's fake and its camera is
  a synthetic image. MuJoCo needs `pip install mujoco` and pip is absent here; CI runs the MuJoCo
  driver's own suite, and joining it to `check-loop` is one line in that Makefile on a machine with it.
* **No ONNX Runtime.** F15. `skynet-infer`'s runner is a feed-forward network of the workspace's own
  arithmetic. Nothing in the tree calls it ONNX, and a policy file declaring `kind: "onnx"` is refused.
* **`virtual-robot.sh` did not run.** `sudo` in this WSL wants a password. §33.1's units did not start
  and the `#[ignore]`d driver-loading suites did not run — CI runs both.
* **The replay's *divergence* branch was not exercised here.** The gate asserts agreement, and to make
  it disagree the kernel would have to change between the run and the replay, which one pass cannot do.
  `skynet-blackbox`'s own suite is what covers that branch.
* **`check-offline` is a measurement and not evidence.** *Next* 4 says what is missing and why.

**Still true, and this entry is mostly good news, so it bears repeating.** All 19 bench tests are
`missing`. The dry ceiling is `none`, and nothing here moved it: a loop closing on a fake body is not a
bench result, and §15's sentence about a runner reporting `pass` for a test it did not perform is the
one to keep holding the work to. `proximity` is inert on every robot, so even a Host-tier robot under
this plan does not enforce distance to a person. Nothing has been signed by a Hub.

### 2026-09-16 (sixth session) — the last two owner's calls, and a rule this plan misattributed

Windows 11, Rust 1.98.1 (CI's toolchain), Node 26. No WSL used, no robot, no ONNX Runtime.
**Nothing pushed, no commits.** The two items still marked *the owner's call* were delegated with
*"decide yourself on these 2"*, and answering the second required checking a fact rather than
weighing a preference.

**Audited.** `DRY-COVERAGE-PLAN.md` against the tree, item by item, to answer whether the plan is
implemented — Phase 0's five items, Phase 1's three rows, Phases 2–5, and all six of *Next*;
`CONTRIBUTING.md`'s *enforced, not requested* list and `ci.yml`'s dependency check, to find where the
no-third-party rule actually lives; `Runtime::power_refusal` and `power_refusal_from` and every caller
of `read_power`; `skynet-infer`'s `Session` seam and `mlp.rs`'s refusal of `kind: "onnx"`; and ONNX
Runtime's own C API documentation for the ABI claim F15 rests on.

**Found.** F15's second bullet is wrong, and the plan has been misattributing a repository rule.

The ABI half is in F15's own correction block: `GetApi(version)` hands back the struct for the version
*requested* and a null pointer when it cannot, so the "wrong by one field, calls the wrong function"
failure F15 refused is a detectable refusal rather than a silent misdispatch. The route that needs no
rule changed is therefore also the safer one, which is not how F15 read it.

The attribution half is smaller and worse. F15 and *Decisions* 5 both open by saying *the owner*
allowed or forbade a dependency. **`CONTRIBUTING.md` line 89 is where that rule lives**, under
*enforced, not requested*, with `ci.yml` asserting `Cargo.lock` holds exactly the workspace members.
Asked about it, the owner did not recognise the rule as theirs — correctly. A repository rule
described as a person's preference is a rule that can be waived by somebody who never set it, and
this document did that twice.

**Changed.**

| File | What |
|---|---|
| `crates/skynetd-io/src/runtime.rs` | `power_refusal` deleted. Its §36/§64 rationale — the argument for naming *which* reason withholds a lease — moves onto `power_refusal_from`, together with why the gauge is not read here and what a caller with no reading does instead |
| `tools/production-callers-allow.json` | Its entry removed. 31 entries → 30 |
| `reference/reports/scale.json` | Regenerated, and **required**: `scale-report --check` holds the pin to a run, so deleting Rust moves it. 141,269 lines, 25 crates, 196 files |
| `DRY-COVERAGE-PLAN.md` | F15's correction block; *Decisions* 11 and 12; the Phase 1 runner row; *Next* 3; this entry |

**Not changed, and the point of the exercise.** `docs/skynet-os-architecture.md` and both mirror
digests. The rounded §73 figures are unchanged at 141,000 lines and 25 crates, so *Decisions* 1's
maintenance did not arise — and *Decisions* 12 chose the route that needs no edit to `CONTRIBUTING.md`
and none to `ci.yml`'s dependency check, so the carve-out *Decisions* 5 granted is still unspent.

**Verified.** On Windows at CI's toolchain:

```
cargo fmt --all --check                              exit 0
RUSTFLAGS="-D warnings" cargo clippy --all-targets   exit 0, 0 warnings
cargo test --workspace                               1,810 passed, 0 failed  (59 suites)
node tools/check-production-callers.mjs              961 checked, 30 allow-listed
node tools/scale-report.mjs --check                  §73 matches the tree
node tools/check-mirrors.mjs                         the specification matches its pin
```

`1,810` is the same figure the fifth session recorded on Windows, which is the check that matters for
a deletion: nothing in the suite was reaching `power_refusal`, so no test went with it.

Held to a positive control, and reverted: re-adding `pub fn power_refusal` fails
`check-production-callers.mjs`, naming `crates/skynetd-io/src/runtime.rs:1928` — the declaration's own
line, which is also F9's line-number fix still holding.

**Not verified, and not claimed.** No robot, virtual or physical; `virtual-robot.sh` needs systemd as
PID 1 and this is Windows, so §33.1's units and the `#[ignore]`d driver-loading suites did not run,
and neither did the Linux-only halves of the suite or `run-ci-job.mjs`'s shell steps — CI runs all of
them. Nothing here went near ONNX Runtime: *Decisions* 12 records a **route**, and a route is not an
implementation. Nothing in this session moved the dry ceiling off `none`, all 19 bench tests are still
`missing`, `proximity` is still inert on every robot, and nothing has been signed by a Hub.

### 2026-09-16 (seventh session) — §23.3's runner built, and Phase 1 finished

Windows 11, Rust 1.98.1 (CI's toolchain), Node 26, Python 3.14 with pip — which is the difference
between this entry and *Decisions* 12's "not built here". **ONNX Runtime 1.30.0 and `onnx` install in
under a minute**, so the library and a real model were both available, and the thing the decision said
could not be verified on that machine was verified on this one. F3a's lesson arriving a third time.
**Nothing pushed, no commits.**

**Audited.** `skynet-infer`'s `Session` seam, `mlp.rs`'s artefact loader and where the two overlap;
`skynet-dl`'s surface and what it was missing for a library that is not a driver; `skynetp`'s model
load and backend arms; `ci.yml`'s `unsafe` allow-list step and its no-third-party-dependency step, to
find what a new crate owes each; `CONTRIBUTING.md`'s *enforced, not requested* list; and
`onnxruntime_c_api.h` at tag `v1.30.0` — 401 KB of it, parsed rather than read.

**Built.**

| File | What |
|---|---|
| `crates/skynet-ort/src/api.rs` | **New.** `OrtApiBase`, and members 0–100 of `OrtApi` in header order with each index in a comment. Real signatures for the 24 this crate calls, `Opaque` for the rest — a wrong signature on a function nobody calls cannot hurt anyone, and 46 unnecessary ones are 46 chances to be wrong |
| `crates/skynet-ort/src/lib.rs` | **New.** `Ort::open` (load, negotiate the API version, round-trip), `Onnx` implementing `skynet_infer::Session`, six named refusals, hand-written `Drop` so handles are released before the library unloads |
| `crates/skynet-ort/src/tests.rs` | **New.** 20 tests in two halves: 9 need no library and run on every commit, 11 need a real one and are `#[ignore]`d |
| `crates/skynet-ort/tests/make-fixtures.py` + `tests/fixtures/*.onnx` | **New.** Six models, 129–225 bytes, each with an output a test predicts. Committed with the generator, because a fixture nobody can regenerate is a fixture nobody can check |
| `crates/skynet-dl/src/lib.rs` | `Library::address` — safe to call, because producing a raw pointer is safe and every hazard is in the use. This is F15's reverted `Library::address`, back with the caller it was missing |
| `crates/skynet-infer/src/lib.rs` | `read_artefact` and `kind_of`, and `read_inputs` moved here from `mlp.rs`. **One parser for the `inputs` block**, because two would agree until one gained a field and then produce plausible numbers |
| `crates/skynet-infer/src/mlp.rs` | Loses its own `kind`/`version`/`inputs` preamble to the above. Its crate docs stop saying the production runner does not exist |
| `crates/skynet-policy-host/src/main.rs` | `Runner`, an enum over the two sessions, and `load_runner`. **The file's `kind` picks**, never a flag — two records of that fact would disagree exactly once |
| `ci.yml`, `CONTRIBUTING.md` | `skynet-ort` added to the `unsafe` allow-list with its argument written out, as that check requires. Both texts now say *two* loaders |
| `tools/scale-report.mjs` | `NUMBER_WORDS` filled to forty. It had been extended one entry at a time and `eleven`–`fourteen` were missing outright, so a document saying twelve of anything would have read as malformed rather than as an incomplete tool |
| `docs/skynet-os-architecture.md` | **The pinned specification.** §73: 141,000 → 143,000 lines, twenty-five → twenty-six crates. Both copies, re-pinned with `--pin`. *Decisions* 1 and 10 |
| `docs/status.md` | §23.3's runner is built; what is checked about the transcription rather than trusted; and what is still not built (LibTorch, any provider but CPU) |

**Found.** F20 — the round-trip crashed rather than refused, and every test passed with the defect in
place because it is only reachable once the transcription is already wrong. The control was the only
thing that read it.

**What the route cost, and what it did not.** *Decisions* 12 chose `dlopen` to keep
`CONTRIBUTING.md`'s no-third-party rule intact, and that held exactly: **`Cargo.lock` holds 26 entries
and all 26 are workspace members.** No dependency, no waiver, no narrowing of that check. What it did
cost is one crate on the `unsafe` allow-list — the seventh — and the allow-list is where such a cost is
admitted rather than hidden. `skynet-infer`, which decides what a joint is told to do, is on the other
side of that edge and still `#![forbid(unsafe_code)]`; so is the kernel, four crates away.

**Verified.** On Windows at CI's toolchain:

```
cargo fmt --all --check                                  exit 0
RUSTFLAGS="-D warnings" cargo clippy --all-targets       exit 0, 0 warnings
cargo test --workspace                                   1,819 passed, 0 failed (61 suites)
cargo test -p skynet-ort -- --include-ignored            20 passed, 0 failed
node tools/check-production-callers.mjs                  966 checked, 30 allow-listed
node tools/scale-report.mjs --check                      §73 matches the tree
node tools/check-mirrors.mjs                             the specification matches its pin
node tools/check-contract-mirrors.mjs                    every mirrored contract matches
node tools/check-golden-path.mjs / check-rhal-flags.mjs  exit 0
Cargo.lock vs workspace members                          26 and 26, exact
the unsafe allow-list                                    7 allowed, 19 forbid it
```

And through the binary rather than the library, which is the part a unit test cannot claim:
`skynetp run --backend inference --policy <onnx artefact>` loads the model, reads its width off the
real graph, and then refuses for the right reason — *"commands 2 channel(s) and this lease grants 3"*
against `--channels 3`, and the `--ring-socket` refusal against a correct lease. The width in that
first message came out of ONNX Runtime.

Controls, each failing and each reverted:

* **Two same-width members swapped** — the case `size_of` cannot see. Crashed before F20's fix;
  refuses cleanly after it, naming the code it read back and saying nothing was dereferenced.
* **A member inserted** — `size_of` fails with no library present, `left: 816, right: 808`.
* **The size test's own wording** was corrected rather than left: it claimed to catch a reorder, and it
  cannot. Two checks, two different failure modes, and each now says which one it covers.

**Not verified, and not claimed.**

* **No robot, and no simulator.** `virtual-robot.sh` needs systemd as PID 1 and this is Windows.
* **`check-loop` has not run with an ONNX policy.** The gate's measurement in the Phase 1 table stands
  as taken, with the `mlp` runner. Joining the two is one line in that Makefile on a Linux machine with
  ONNX Runtime, and until somebody runs it the ONNX runner's evidence is its 20 tests.
* **One library, one version, one platform.** ONNX Runtime 1.30.0, C API 30, Windows x86-64. The
  transcription is `extern "system"` so 32-bit Windows is handled by construction rather than by
  measurement, and no aarch64 or Linux build of the library was loaded here. CI does not run these
  tests either — they are `#[ignore]`d and no job installs ONNX Runtime, which is a gap worth naming
  rather than a thing that was done.
* **Nothing here moved the dry ceiling off `none`.** A runner is not a bench result. All 19 bench tests
  are still `missing`, `proximity` is still inert on every robot, and nothing has been signed by a Hub.

### 2026-09-16/17 (eighth session) — four drivers, the bench format, and the discovery that the constraint had moved

**Audited.** §12, §15, §24, §25, §26.2, §33.1, §43, §47.6, §54.2, §59.2, §63.4, §73, §82;
`crates/skynet-contracts/src/{certification,digest,live}.rs`, `crates/skynet-cli/src/check.rs`,
`crates/skynet-kernel-host/src/{handoff,capture,main}.rs`, `crates/skynet-ring-shm/src/pass.rs`,
`crates/skynetd-io/src/audit.rs`, `drivers/python-host/loop.sh`, `.github/workflows/ci.yml`, and —
as substrates rather than as reading — `mock_components/GenericSystem`, URSim 5.26, MuJoCo under
`osmesa`, and a CiA 402 slave written here.

**Found.** F21–F30. Three of them came out of a driver rather than out of the document, which is the
provenance worth having:

* **F26** — `drivers/canopen`'s `open()` sent `NMT reset node`, and a reset clears a latched fault.
  Written deliberately, as the obvious way to reach a known state; the obvious way erased, on every
  restart, the one piece of evidence about why a robot stopped. Found by a refusal with a control
  beside it.
* **F27** — three properties of the UR driver that would not have been guessed, each found by
  running against UR's own controller and none of them visible from the protocol document.
* **F30** — §24 commands a **body** in one mode and `ros2_control` commands a **joint** in one. The
  intersection is the only honest descriptor available, and for a mobile manipulator it is empty. A
  specification question, written out as `docs/issues/per-joint-command-modes.md`.

And **F29**, which is the one that reframes the rest: the plan is named for a constraint that stopped
being the binding one three phases ago. Phases 3, 4 and 5 are waiting on **design decisions**, not on
a robot.

**Changed.**

| | |
|---|---|
| `skynet_contracts::bench` | §15's evidence format — a record bound to the digest of the artefacts it was measured against, which is §12's `BenchPassStale` in general form. 22 tests |
| `skynet bench record` / `award` | The two ends of it. `check-offline` writes one over the three binaries that decided and reads it back |
| `pass::peer_pidfd`, `Port::reap_departed`, `Camera::revoke_pool_loans` | F18's answer: `SO_PEERPIDFD` on the accepted socket, so the process holding the pool learns the fact directly and no pid can be reused under it. All-or-nothing, and a hand-over the kernel cannot name disables it for the run and says so |
| §54.2's live context | Velocity, effort and a `DriveState` across the contracts crate, the TypeScript oracle and three editions — so four of the first clauses anybody writes stop running on no body at all (F28) |
| `check.rs` | A quantitative assertion keeps its number (F23), and the acceptance assertion is `not_applicable` for a body that declares no control modes (§59.2, F25) |
| `drivers/pendant` | §59.2's program-level body. 37 checks, nearly all refusals, because for that class of machine the refusals are the interface |
| `drivers/canopen` | CiA 402 over CANopen, zero dependencies. 61 checks, 24 of 24 through the real ABI |
| `drivers/ur_rtde` | RTDE and the dashboard, measured against URSim. 35 checks, 24 of 24 through the real ABI |
| `drivers/ros2_control` | A `hardware_interface::ResourceManager` hosted in-process — never a `controller_manager`, because §11 forbids a network in the control loop. 42 checks, 23 of 23 through the real ABI |
| `docs/issues/` | Four amendments written out in the form they would take, each with the implementation order it unblocks |
| `README.md`, `docs/status.md` | Their *"not built"* lists named `ros2_control` and CANopen, and both drivers now exist. Corrected in place rather than rewritten, and the correction keeps the part that matters: none of these has driven a physical machine |

**Verified**, on Linux, at the end of the increment:

```
cargo fmt --all --check                                  exit 0
RUSTFLAGS="-D warnings" cargo clippy --workspace          exit 0
cargo test --workspace                                   1,944 passed, 0 failed, 20 ignored
node tools/run-ci-job.mjs invariants                     53 of 53
node tools/run-ci-job.mjs corpus                         9 of 12 — see below
node tools/scale-report.mjs --check                      §73 matches the tree
node tools/check-mirrors.mjs                             the specification matches its pin
node tools/check-golden-path.mjs                         10 documents, 65 commands
drivers/{pendant,canopen,ur_rtde,ros2_control}           37 · 61 · 35 · 42 checks, 0 failed
skynet driver check, through dlopen                      24 · 24 · 23 of the sim suite, guest tier
```

**Not verified, and not claimed.**

* **Three of the corpus job's twelve steps did not run**: `zig` and `dotnet` are not installed on this
  machine, and the edition-figures check fails downstream of the two it could not measure. The four
  **complete** editions — Rust, Python, C++, Go — each reproduce all 1,210 cases here. This is F7's
  situation exactly: the checks passing is not a claim that CI is green.
* **No robot.** Four drivers, four substrates, and not one physical machine among them. A fake, an
  in-process bus, a vendor's controller in a container and a framework's own mock show that four
  protocols are encoded correctly and show nothing about a joint. Every one of them reaches **guest**
  and stops there by mechanism, which is §35.3 working rather than a result being modest.
* **The specification is unamended.** Four issues are written and none is ratified;
  `check-mirrors` reports both copies matching their pin, which is the state to leave it in. Phases 3,
  4 and 5 remain blocked on exactly those four documents.
* **`drivers/canopen` has never touched a wire.** `SocketCanBus` is written, refuses to construct on a
  kernel without `AF_CAN`, and `make check-vcan` exits non-zero where there is nothing to run against.
* **`drivers/ros2_control` has never met a plugin that blocks.** `mock_components` copies commands to
  states: no dynamics, no latency, no drive that can fault.

### 2026-09-17 (ninth session) — the amendments ratified, and the phases they were holding

Windows 11 with WSL2 Ubuntu 26.04 beside it. Rust 1.98.1 on Windows (CI's toolchain), 1.93.1 in WSL;
Node 26 on Windows; and in WSL, installed for this session under `~/.skytools`, **every toolchain
CI's `corpus` job names** — Node 24.21, Go 1.24.13, Zig 0.16.0, Temurin 21, .NET 8.0. So all eight
editions run on one machine, which is what found F31. **Committed locally, nothing pushed**, on the
owner's instruction. *Updated as the session goes.*

The instruction was to finish every section. F29 had already shown that Phases 3, 4 and 5 wait on
decisions rather than hardware, so the session opens with *Decisions* 13 and works through the
issues in `docs/issues/`, each as a section of the specification's new Part XII.

#### Phase 3 — §84, a motion action space

**Changed.**

| | |
|---|---|
| `docs/skynet-os-architecture.md` | Part XII opened; §84 written as built. §73's figures moved (147,000 lines of Rust, 82 files, 1,270 cases, 15,000 lines of reference) and its stale editions row rewritten (F32). Both copies, re-pinned |
| `reference/typescript/embodiment.ts`, `shape.ts` | `motions` on both sides, `CANONICAL_MOTIONS`, `validateMotions` and six fault codes, the action-space decision, the motion binder, three warning codes appended. Synced into the Hub |
| `tools/corpus-gen/generate.ts` | 35 cases in `transport/motion.json`, 23 in `embodiment/motions.json`, two closed sets. **Written before any edition** — every existing case unchanged except the closed set that grew |
| `crates/skynet-contracts`, `skynet-corpus` | The Rust edition: types, decoder, planner arm, validator, runner arms |
| `editions/{python,cpp,go}` | The three other complete editions; ratchets 1,210 → 1,270 in `ci.yml` and both Makefiles |
| `editions/{c,zig,java,csharp}` | The two new closed sets and three new warning codes; their set-count self-checks 74 → 76; ratchets raised by two |
| `crates/skynet-cli` | `skynet transport` renders motion bindings and says why a plan has none; `skynet driver check` validates the motion list and asks for a joint **or** a motion; `skynet limits`' plan printer lists them |
| `drivers/pendant` | `describe()` serialises the motion table the interpreter enforces, and refuses a buffer one byte short rather than truncating. 42 checks |
| `reference/reports/{editions,scale}.json` | Regenerated. `editions.json` had been stale since `9037667` (F31) |
| `docs/issues/` | `motion-action-space.md` marked ratified, kept as the proposal it was |
| **The Hub** | `lib/skynet/os/embodiment.ts` and the two pins synced; `parseSourceEmbodiment` reads `motions` by Rust's rules; the transport page renders a motion table and stops calling a refused or motion plan *"declares no action channels"*; six catalogue keys, synced into 34 locales at the English value — **not translated**, because running the translator needs the owner's approval in the conversation |

**Verified.**

```
cargo fmt --all --check / clippy -D warnings --all-targets   exit 0
cargo test --workspace                                       1,846 passed, 0 failed, 20 ignored (Windows)
cargo test --workspace                                       1,945 passed, 0 failed, 20 ignored (WSL)
corpus, eight editions                                       rust python cpp go 1270/1270
                                                             c 356 · zig 317 · java 317 · csharp 317, 0 failed
node tools/edition-report.mjs (WSL)                          4 editions, 1270 cases each
node tools/run-ci-job.mjs invariants --wsl                   52 of 53, then 53 once scale.json was re-pinned
node tools/check-mirrors.mjs / check-contract-mirrors.mjs    match
make -C drivers/pendant check                                42 checks, 0 failed
skynet driver check, pendant                                 23 passed, the motion list among them; guest
Hub: vitest (4 suites) / tsc --noEmit / eslint               100 passed · exit 0 · exit 0
```

Controls, each failing and each reverted: first-wins made last-wins in the Rust binder fails
`a-repeated-body-motion-first-wins` and `a-repeated-name-is-one-candidate`; `required` read as
`== Some(true)` fails `an-undeclared-requirement-reads-as-required`; the Hub reader as it was before
this session fails F33's structural test.

**Not verified, and not claimed.** No controller. `drivers/pendant`'s interpreter is this repository's,
and §84 says what a program-level body *declares*; nothing here has sent a motion to a Fanuc, a KUKA or
an ABB. Guest tier stays permanent for these bodies by §59.2, and no guard reads a motion. The six new
Hub strings read in English in all 35 locales until a translation run is approved.

### 2026-09-17 (tenth session) — the rest of Part XII

A Linux container, and the first machine in this plan's history where **every toolchain CI names runs
natively**: Rust 1.94.1, Node 22, Python 3.11, Go 1.24.7, GCC 13.3, Temurin 21, and — installed for
this session under `~/.skytools` — Zig 0.16.0 and .NET 8.0. So the eight-edition and `invariants`
lines below are runs rather than inferences, which is what F31 asked for and F7 before it.
**Committed locally, nothing pushed**, on the owner's instruction. *Updated as the session goes.*

The instruction was the ninth session's, continued: finish every section. The ninth ratified the
amendments (*Decisions* 13) and built §84; this one carries the remaining three issues into Part XII
and builds them, in the order the issues name.

It opened on a tree the ninth session had left mid-flight: §85's implementation was committed across
the reference, the Rust edition and the corpus, and the specification had no §85 in it, the plan had
no entry for it, and seven editions failed thirty-five cases each against a rule the reference had
corrected under them. F34 is why, and finishing it was the first work.

#### Phase 4 item 3 — §85, the capability token vocabulary

**Changed.**

| | |
|---|---|
| `docs/skynet-os-architecture.md` | §85 written as built, in four subsections; §73's corpus figures moved to 84 files and 1,314 cases. Both copies, re-pinned |
| `reference/typescript/package.ts` | `CAPABILITY_DOMAINS`, `CAPABILITY_TOKENS`, `TOKEN_CLASSES`, `classifyToken`, `capability_unknown` in the rejection codes, and both arms of the admission check. Synced into the Hub |
| `tools/corpus-gen/generate.ts` | 30 cases in `admission/tokens.json`, 11 in `admission/vocabulary.json`, three closed sets. Written before any edition |
| `crates/skynet-contracts` | The Rust edition of all of it |
| `editions/{python,cpp,go}` | The three other complete editions; ratchets 1,270 → 1,314 in `ci.yml` and both Makefiles |
| `editions/{c,zig,java,csharp}` | The three new closed sets; their set-count self-checks 76 → 79 |
| `crates/skynet-cli` | `skynet skill check` gains `skill.capabilities.canonical`; `skynet driver check` folds the same test into `embodiment.descriptor.valid`, naming each token's class |
| `reference/reports/{editions,scale}.json` | Regenerated |
| `docs/issues/` | `capability-token-vocabulary.md` marked ratified, kept as the proposal it was — and it is the one the built form departs from most (F34) |
| **The Hub** | `lib/skynet/os/package.ts` and the two pins synced; `lib/skynet/robots/interface.ts` gains §85's four additions and an `hri` group; both registration routes return each refused token **with its class**, because *unknown*, *vendor* and *malformed* have three different fixes; `lib/__tests__/skynet-os-mirrors.test.ts` holds the two lists to each other in order and refuses an unlabelled token. One catalogue key, synced into 34 locales at the English value — **not translated**, because running the translator needs the owner's approval in the conversation |

**Verified.**

```
cargo fmt --all --check / clippy -D warnings --all-targets   exit 0
cargo test --workspace                                       1,946 passed, 0 failed, 20 ignored
corpus, eight editions                                       rust python cpp go 1314/1314
                                                             c 359 · zig 320 · java 320 · csharp 320, 0 failed
node tools/edition-report.mjs --check                        4 editions, 1,314 cases each
node tools/scale-report.mjs --check                          §73 matches the tree
node tools/run-ci-job.mjs invariants                         53 of 53
node tools/check-mirrors.mjs / check-contract-mirrors.mjs    match
Hub: vitest / tsc --noEmit / eslint                          2,279 passed · exit 0 · exit 0
```

Controls, each failing and each reverted: the first pass of the editions *is* the control for F34 —
the two-segment rule the issue proposed, written into Go, C++ and Python, fails 35 admission cases
each against a reference that enumerates `sensing.camera.rgb`; and the Hub's interface with
`sensing.audio` removed fails the new order-and-membership test.

**Not verified, and not claimed.** Nothing here has been run against a robot, and nothing here needs
one — it is a closed set and a classifier. What it cannot do is decide whether a *vendor's* token is
a good one: `acme.vacuum` classifies as `vendor` and nothing on this platform knows what it means,
which is what `extensions` is and is not a gap. No token has been granted to a package on a machine
that then used it. This session ran all eight editions on one container, including Zig 0.16.0 and
.NET 8 installed for it, so the eight-edition line above is a run rather than an inference.

#### §86 — the mode is a property of a joint

The `per-joint-command-modes.md` amendment, ratified and built. It is the one issue that came out of
a driver rather than out of reading the document, and the driver is what it changes back.

**Changed.**

| | |
|---|---|
| `docs/skynet-os-architecture.md` | §86 written as built, four subsections; §73's figures moved to 148,000 lines, 86 files, 1,332 cases. Both copies, re-pinned |
| `bindings/c/skynet_rhal.h` | `skr_command.modes`, `flags` in place of `_pad[0]`, `SKR_JMODE_*`, `SKR_CMD_PER_JOINT_MODES`, and the three macros that keep the two numberings related in one place. **No ABI bump** — §27's homing precedent, one field along |
| `bindings/{cpp,python}`, `crates/skynet-rhal` | The same layout in three more views, each with its own offset assertions, which is the three-sided check a moved field fails on |
| `reference/typescript/{embodiment,safety,shape}.ts` | `mode` on a binding, `joint_modes` on the clamp and on `ResolvedLimits`, `per_joint_modes` on the descriptor, §86.2's two refusals, and `ceilingFor` narrowing rather than preferring (F36) |
| `tools/corpus-gen/generate.ts` | The mobile-manipulator fixture in two forms, 11 cases in `transport/mode.json` and 7 in `cycle/modes.json`. **Written before any edition** |
| `crates/skynet-{contracts,kernel,corpus}`, `skynetd-io` | The Rust edition, including the black box's replay path — a window sealed on a mixed body replays against the ceilings it was actually held to |
| `editions/{python,cpp,go}` | The three other complete editions; ratchets 1,314 → 1,332 |
| `editions/{c,zig,java,csharp}` | The two new warning codes in their closed sets |
| `crates/skynet-{dl,kernel-host}` | `write_modal` and `ModalFrame`; the loop builds the per-joint array from the plan's own map, by channel name, and sends none when the joints agree |
| `crates/skynet-cli` | `skynet transport` renders the mode beside each ceiling and a mode table when a plan mixes them; `driver check` gains `rhal.command.rejection`'s per-joint case, applicable only to a driver that declares it reads the array |
| `drivers/mock` | Now honours the array, clamps in the mode's own units, and refuses two things it used to accept (F37). 30 checks |
| `drivers/ros2_control` | The union rather than the intersection, `per_joint_modes`, and per-joint dispatch in `write_command`. `mixed_interfaces.urdf` is unchanged and its suite asserts the opposite of what it did |
| **The Hub** | `lib/skynet/os/{embodiment,safety,shape}.ts` and both pins synced; the transport page renders a joint-mode table when a plan mixes modes. Three catalogue keys, synced into 34 locales at the English value — **not translated** |

**Verified.**

```
cargo fmt --all --check / clippy -D warnings --all-targets   exit 0
cargo test --workspace                                       1,948 passed, 0 failed, 20 ignored
corpus, eight editions                                       rust python cpp go 1332/1332
                                                             c 359 · zig 320 · java 320 · csharp 320, 0 failed
node tools/edition-report.mjs --check                        4 editions, 1,332 cases each
node tools/scale-report.mjs --check                          §73 matches the tree
node tools/run-ci-job.mjs invariants                         53 of 53
node tools/check-mirrors.mjs / check-contract-mirrors.mjs    match
make -C drivers/mock check                                   30 checks, 0 failed
make -C drivers/{pendant,canopen,ur_rtde} check              42 · 61 · 12 checks, 0 failed
skynet driver check, mock                                    25 passed, 0 failed — including the per-joint rejection case
Hub: vitest / tsc --noEmit / eslint                          2,279 passed · exit 0 · exit 0
```

Controls, each failing and each reverted: the mock's suite as it stood before this session fails on
a body-wide torque command (F37, and it is the reason the section found anything); `ceilingFor`
preferring the per-channel figure fails `a-velocity-joint-takes-the-narrower-of-the-two` (F36); the
kernel loop sending the array unconditionally fails `a-single-mode-body-sends-no-per-joint-array`.

**Not verified, and not claimed.** `drivers/ros2_control` **was not built or run**: ROS 2 is not
installed on this container and its Makefile refuses rather than building half of itself, which is
the right behaviour and means its 42 checks are unexercised here. That is the single largest gap in
this entry, and it is exactly the row §86 was written for — the driver's changes are reviewed code
and nothing more until somebody runs `make -C drivers/ros2_control check` on a machine with
`ros2_control` on it. Everything else above ran. No robot: no per-joint command has reached a
physical drive, the mock's mixed body is three joints in a struct, and §59.2's tier ruling is
untouched.

#### What this session did not reach

Two sections of Part XII are unwritten, and both are the same shape as the three that are: a
ratified amendment with an implementation order, blocked on nothing.

* **§87 — Phase 5's four class gates** (`docs/issues/phase-5-class-gates.md`). The issue is written
  **as questions**, deliberately, and *Decisions* 13 says ratifying it means answering them, each
  answer recorded with the alternative it rejected. Four of them: multiple robots on one compute,
  safe-state discovery for mobile robots, `on_command_loss` per chain, and the Hub-brokered console
  relay. Nothing about them needs a robot.
* **§88 — `execute({ skill, arguments })` to a command-level driver** (Phase 4 item 1, named as §88
  by `motion-action-space.md`). §84 removed the first of its two blockers; the second is a design
  decision of the same weight — §33.1 keeps `skynetd` off the path that holds a driver, so there is
  no channel from the supervisor to the process that would route a skill.

The plan's *Next* list and its Phase 4 and Phase 5 sections carry both, and neither is waiting on
hardware. What is genuinely outside this plan is unchanged and listed under *What stays open*.

### 2026-09-17/18 (eleventh session) — §87 and §88, and the eight editions behind them

The tenth session's instruction, continued: finish every section. Part XII's last two amendments
were specified, oracled, corpus-written and reproduced in Rust by the end of the tenth; this session
carries them into the remaining editions, the loop, the CLI and the Hub, and closes the plan's
*Next* list. **Committed locally, nothing pushed**, on the owner's instruction. *Updated as the
session goes.*

It opened on a tree with two commits of §87/§88 work on it and four things wrong with it: the C
edition's watchdog answered thirteen cases without §87.3's field, Java and C# had none of the eleven
new closed sets, the Zig edition **did not compile** (F38), and §73, the reports, the issue files and
this plan all still described a tree two sections old.

#### The editions — all eight, run on this container

**Changed.**

| | |
|---|---|
| `editions/c` | §87.3's resolver written out — `resolve_command_loss`, the five refusals in order, `per_chain` on every watchdog document — and `watchdog/chains` added to the dispatch. The watchdog corpus is now **complete** in C; ratchet 359 → 382 |
| `editions/java` | The eleven new closed sets and §87.1's five additions to `cell_faults`; self-check 79 → 90 sets; ratchet 320 → 331 |
| `editions/csharp` | The same eleven, the same five, the same two counts |
| `editions/zig` | F38: the tenth session's own edit left a stray `},` in `vocabulary.zig` and **the edition had not been compiled since**. Removed; ratchet 320 → 331 |

Zig 0.16.0 and .NET 8.0.425 were installed under `~/.skytools` for this session, which is what made
the two rows above runs rather than inferences — and is what found F38.

#### F38 — the Zig edition was committed broken, and the ratchet could not have caught it

`editions/zig/src/contracts/vocabulary.zig` gained §87's eleven closed sets in the tenth session and
one closing brace too many with them. It is a **compile error**, not a corpus failure, and the
distinction is the finding: every mechanism this repository has for an edition — `--min-passed`, the
four-bucket accounting, `edition-report.mjs --check` — reads the output of a runner that built. A
runner that does not build produces no output at all, so the ratchet is not lowered, no case goes
red, and nothing in `ci.yml`'s wording distinguishes *this edition answered 320 cases* from *this
edition does not exist on this machine*.

What would have caught it is running it, and the tenth session could not: Zig was installed under
`~/.skytools` on that container and this one started empty. The entry above it claims eight editions
on one container and it was true when written; what it did not say is that the claim expires with
the container, and the next session inherits a tree where two of the eight are unexercised code.
That is the same shape as F7 and F31 — a check that is green because it did not run — arriving for
the third time, at the one layer that has no offline substitute.

The remedy is not a mechanism, because there is not one available: an edition needs its toolchain.
What this session did instead is install both, which costs about ninety seconds and makes the
figures below runs. The honest statement for any session that cannot is that the edition was **not
exercised**, named as such, rather than carried forward from an entry that was true on a different
machine.

#### §88 — a skill reaches a command-level driver

The section's own claim is that the second blocker dissolves rather than needing a bridge: a skill
over a program-level body is a policy emitting **motion requests** into the ring it would post joint
commands into, and `skynetk` reads them and calls the driver's `extension()`. This session built
that route.

**Changed.**

| | |
|---|---|
| `crates/skynet-ring` | `mode::MOTION`, `Name`, `Motion`, `publish_motion` and `read_frame`. The text rides in the words above a joint command's, so one ring carries both and **no ABI bump** — the third time in Part XII that additive was available. The mask on a motion slot is all zeros, which is §24's *hold*: a reader that ignores `mode` commands nothing |
| `crates/skynet-dl` | `Driver::extension` and `has_extension` — §4's escape hatch behind a safe wrapper, with the reply returned whatever the code, because a controller's sentence about a refusal is the half an integrator needs |
| `crates/skynet-kernel-host` | `Actuator::extension`, and `motion.rs`: the gate assembled from what the loop knows, the arguments rendered in §84.1's declaration order, §88.3's deadline asked **every** cycle, and the abort when it passes |
| `crates/skynet-blackbox` | `EventCode::MotionRequest`, the sixteenth code, on `SkillTransition`'s test — §88.3 names a specific thing that happens rather than arriving as free text on a code that existed |
| `crates/skynet-kernel-host/src/main.rs` | `--lease`, `--operating-state` and `--motion-token`, each declared the way `--tier` is, each defaulting to the reading that refuses; the route printed before the first cycle, naming what is missing rather than saying it is absent |

**F39 — the widening from the control wire to the descriptor refuses a value the body declares.**
The ring carries `f32` and §84.1's bounds are `f64`, and `f64::from(0.4f32)` is
`0.400000005960464…` — **above** a declared maximum of `0.4`. So a policy asking for precisely the
largest value its body allows is refused `parameter_out_of_bounds`, for a reason that appears in
neither the descriptor nor the request. The same widening then renders seventeen digits into the
controller's request buffer, which on `drivers/pendant`'s 127 bytes is four parameters short of
being refused for its length. The value is now read back through the shortest decimal that
round-trips to that `f32`, which is the number the policy wrote. It was found by an assertion about
argument *order* that happened to print the value.

#### §87 — the four class gates, answered

Phase 5's whole table, and the last of `docs/issues/` to be ratified. The specification, the oracle,
the corpus and the Rust edition landed in the tenth session; this one carried it into the four
remaining editions and the surfaces that render it.

**Changed.**

| | |
|---|---|
| `crates/skynetd-io/src/cell.rs` | `GET /cell` answers §87.1's `robots`, each with `mine`, its placement and the layer it contributes. `at` is **null** rather than an origin on an unplaced robot — writing `[0,0,0]` would be the route supplying the placement `robot_unplaced` exists to demand |
| `crates/skynet-cli` | `skynet cell` renders the robots and says, once, that two kernels cannot see each other's envelope; `skynet power` renders §87.2's basis beside the term that left `unaccounted`; `skynet driver check` resolves §87.3 whichever way a body declared |
| `reference/typescript/conformance.ts`, `crates/skynet-contracts` | `safety.command_loss.behaviour`'s `measures` now names both fields and says every chain is observed. The corpus does not freeze this prose, so it is three files and no regeneration |
| `docs/issues/` | `phase-5-class-gates.md` marked ratified, kept as the **questions** it was, with the three places the answers depart from what it asked; `README.md` records that every file in the directory is now ratified |

**F40 — `Outcome::Missing` for a body that declares nothing took `drivers/pendant` off its tier.**
The first draft of §87.3's check in `driver check` reported `Missing` whenever `resolve_command_loss`
refused host, and §15's rule is that absence caps the tier exactly as a failure does. So a body that
correctly declares nothing about command loss — which §46.2 permits, and which refuses *host* and
leaves guest untouched — came out as **no tier**, and the pendant, whose whole point is that it
reaches guest and stops there by mechanism, stopped reaching it.

The distinction the check was missing is between a declaration that **cannot be read** and one that
**does not claim host**. Both fields at once, a map over a body with no chains, an uncovered chain
and an unrecognised word are defects; silence and a declared `coast` are correct descriptions of
real machines. It now fails on the first four and passes on the last two with the refusal in the
sentence, which is §87.5's own claim — *what this section does not change is the tier* — turned into
a check rather than left as a paragraph. Found by running the conformance suite against the driver
the section was written for, which is the only reason it was found at all.

#### The Hub

§88's one real change on that side, and it is a sentence §54.1 had carried since Part VI.
`StandardRobotInterface.execute` required `control.trajectory`, so `availableMethods` reported that
a program-level body **cannot serve a skill** — which was true when it was written and had stopped
being true. It now accepts `control.motion` as well, and `METHOD_CAPABILITIES` is read as *any of*
rather than *all of*, with the empty case written out because `[].some()` and `[].every()` disagree
about it and `extension` is the method that needs none. Held by a test that names the pendant-class
body and a control that refuses a body holding neither token.

The specification, the contract mirrors and both pin files are synced; §73's figures moved to 153,000
lines, 92 corpus files and 1,424 cases. No catalogue key was added, so nothing here needs the
translator.

**Verified.**

```
cargo fmt --all --check / clippy -D warnings --all-targets   exit 0
cargo test --workspace                                       1,970 passed, 0 failed, 20 ignored
corpus, eight editions                                       rust python cpp go 1424/1424
                                                             c 382 · zig 331 · java 331 · csharp 331, 0 failed
node tools/edition-report.mjs --check                        4 editions, 1,424 cases each
node tools/scale-report.mjs --check                          §73 matches the tree
node tools/run-ci-job.mjs invariants                         53 of 53
node tools/check-mirrors.mjs / check-contract-mirrors.mjs    match
make -C drivers/{mock,pendant,canopen,ur_rtde} check         30 · 42 · 61 · 12 checks, 0 failed
make -C drivers/ros2_control check                           47 checks, 0 failed — see F41
skynet driver check, pendant                                 27 passed, 0 failed — including §88's two
Hub: vitest / tsc --noEmit / eslint                          2,280 passed · exit 0 · exit 0
```

Zig 0.16.0 and .NET 8.0.425 were installed on this container, so the eight-edition line is eight
runs. Controls, each failing and each reverted: the C edition without `per_chain` fails thirteen
watchdog cases; `Outcome::Missing` on a sound refusal takes `drivers/pendant` off guest (F40);
`f64::from` in place of the shortest-decimal widening refuses a parameter at its declared maximum
(F39); `METHOD_CAPABILITIES` read as *all of* fails the program-level body's line and passes its
control.

**Not verified, and not claimed.** No robot, and nothing in §87 or §88 needed one — which is F29
arriving for the third time. Specifically:

* **No motion has reached a physical controller.** `drivers/pendant` is a mock program-level
  controller and its interpolator is this repository's, not a vendor's. What ran is a policy
  process, a ring, a kernel host, a gate and a driver — five real components and no machine.
* **§87.4's relay is rules, not a transport.** Nothing on this platform dials out to a Hub. What
  exists is what the thing that eventually does will be held to, and §39.2's listener rule is
  untouched: the LAN socket is still link-local and unrouted.
* **§87.1's cell has never held two robots.** The arithmetic is tested against fixtures; no site has
  run two kernels on one compute, and the platform's answer to a pair is a refusal rather than an
  enforcement, which is the point and is also why there is less to test than it looks.
* **`drivers/ur_rtde` ran twelve of its thirty-five checks**, because `SKYNET_UR_HOST` is unset and
  this container has a `docker` client with no daemon behind it, so URSim cannot be started. The
  suite says so by name and reports the twelve rather than a green tick — which is the behaviour
  `drivers/ros2_control` has for ROS and the reason the figure in Phase 2's table names URSim 5.26
  rather than standing alone.
* **The dry ceiling is unmoved**, at `none`. Nothing above is a bench test and F21's arithmetic is
  unchanged.

#### `drivers/ros2_control`, built and run — and F41 is why that mattered

The tenth session named this the single largest gap in its entry: the driver's §86 changes were
*"reviewed code and nothing more until somebody runs `make -C drivers/ros2_control check` on a
machine with `ros2_control` on it."* ROS 2 Jazzy and `ros2-control` 4.48.0 were installed on this
container — about ninety seconds of `apt-get` — and the suite ran for the first time since it was
written.

**47 checks, 2 failed**, and both were the same defect.

#### F41 — the check that proves two numberings are kept apart used the one value where they meet

§86.1 gives joint modes their own numbering starting at `0x10` precisely so that a base
`SKR_MODE_*` written where an `SKR_JMODE_*` belongs is a **refusal** rather than a joint driven in
the wrong units. The suite's check for that wrote `SKR_MODE_POSITION`.

`SKR_MODE_POSITION` is `0`. `SKR_JMODE_INHERIT` is `0x00`. They are the same byte — the single value
in the base vocabulary that is also a valid joint mode — so the driver accepted it, correctly, and
the check demanding `SKR_EINVAL` failed. The second failure was the refusal tally one line later,
counting six where the cases above had asked for seven.

The test was wrong and the driver was right, which is the better of the two ways this could have
gone. What makes it worth a finding is that **the same check exists twice** and the other copy is
correct: `skynet driver check` writes `Mode::Torque` (`2`), which is not a valid joint mode and
refuses as intended. Two copies of one check, one of them written against the one input that cannot
fail — and the reason the difference survived is that this copy had never executed. It is F38's
shape at the next layer down: a check that has never failed has proved nothing, and a check that has
never *run* has not even proved that.

**Verified.**

```
make -C drivers/ros2_control check              47 checks, 0 failed
make -C drivers/ros2_control check-conformance  26 passed, 0 failed, 1 not measured, 4 n/a
                                                sim evidence supports guest
```

**Not verified, and not claimed.** The substrate is `mock_components/GenericSystem` — `ros2_control`'s
own fake hardware interface — so what ran is the real framework against a fake body. Jitter under
load is the row Phase 2's table puts under *not dry-verifiable* for this driver and it stays there,
and the tier is guest because the descriptor declares `simulator: true`, which §35.3 caps whatever
the evidence supports.

### 2026-09-18 (twelfth session) — what the plan still owed, and building it

Windows 11 with WSL2 Ubuntu 26.04 beside it, the ninth session's machine: Rust 1.98.1 on Windows
(CI's toolchain) and 1.93.1 in WSL, and the edition toolchains still under `~/.skytools` in WSL.
`sudo` in WSL wants a password, so nothing here was installed system-wide. **Committed locally,
nothing pushed**, on the owner's instruction. *Updated as the session goes.*

The instruction was to finish every section of this plan, and to say first whether anything was
left. **Something was.** An audit of this file against the tree found the four items now numbered
9–12 under *Next*, and a closing paragraph that had declared the plan done on a reason that was only
half true (the correction block under item 12 says which half). The Hub's translations, which three
entries above record as pending, were done by the owner in the meantime — all 35 catalogues are
populated — so that is not among them.

#### F21's producer, and the two things building it found

**Changed.**

| | |
|---|---|
| `skynet_contracts::time` | **New**, and a move rather than an addition: `rfc3339` and `day_number` from `skynetd-io::audit`, with their four tests, every caller repointed. The CLI, the daemon and now an edition's corpus runner all date documents, and the calendar lived in the daemon |
| `skynet conformance --results <file>` | F21's named producer. The robot's `GET /conformance` answer, read back through `results_from_json` before it is written, with the robot's `ran_at` and never the fetch time. `--results` with no file is refused before the request |
| `corpus-report --results <file>` | F42's producer: `safety.kernel.parity` from a corpus run, in `skynet_corpus::parity`. A pass only when every case reproduced and every handler exists; a corpus of nothing is a failure |
| `skynet bench award` | F43: results from several files are **folded** rather than concatenated — a skip never displaces a measurement, and two measurements of one test are refused naming both files |

#### F42 — the robot skips two of "the runtime's four", and one of them had no producer anywhere

F21 sorted guest's nine sim tests into four the driver check measures, four *"the runtime's, and
`skynetd-io::conformance` runs them"*, and one that fails by design. Running `skynet conformance`
against a live `skynetd` with the mock driver's descriptor installed says otherwise about the middle
four: `safety.policy.isolation` and `package.grants.gate` run when a robot has something to run them
against, and the other two are **skipped on every robot, by design and with the reason given** —
`safety.kernel.parity` because it is an edition property, `security.credential.scope` because only
the Hub can measure whether a credential's scopes are enforced.

The second is honestly not this repository's. The first is, and it had fallen between every
producer there is: `skynet driver check` names it as an edition property and does not run it, the
robot names it as an edition property and skips it, and `cargo test -p skynet-corpus` — which *does*
measure it, on every commit — writes nothing down. So a guest-required test was `missing` from every
award this platform could compute, for no reason but plumbing.

#### F43 — the award depended on the order the files were named in

With all three producers writing, the same three files gave two verdicts:

```
skynet bench award driver.json runtime.json parity.json --claim guest
    missing  safety.policy.isolation, package.grants.gate, offline.control.independent, security.credential.scope
skynet bench award parity.json driver.json runtime.json --claim guest
    missing  safety.kernel.parity, safety.policy.isolation, package.grants.gate, offline.control.independent, security.credential.scope
```

`award_certification` reads a list **last wins**, and the corpus pins that on purpose
(`duplicate-results-keep-the-last`): within one producer's report a re-run supersedes what it
re-ran. `bench award` concatenated files from *different* producers into that list, so the robot's
`skipped` — *"not mine to measure"* — erased the edition's pass whenever the robot's file came last.
The fix is in the command, not the contract: across files a skip never displaces a measurement, and
two measurements of one test are refused, because keeping either would be choosing between two
sessions by argument position. The mirrored award function and the corpus are untouched.

Worth recording for what it says about the command: **`skynet bench award` had no tests at all**,
unit or binary. It has five now.

**Verified.**

```
cargo fmt --all --check / clippy -D warnings --all-targets   exit 0 (Windows, 1.98.1)
cargo test --workspace                                       1,884 passed, 0 failed, 20 ignored (Windows)
node tools/run-ci-job.mjs invariants --wsl                   53 of 53
node tools/check-production-callers.mjs                      1,022 checked, 30 allow-listed
node tools/scale-report.mjs --check                          §73 matches the tree (scale.json re-pinned)
node tools/check-mirrors.mjs / check-contract-mirrors.mjs    match
```

And through the binaries, in WSL: a `skynetd` in a temporary root with an observer credential and
the mock driver's descriptor; `skynet conformance --results` against its socket (3 of 8 executed);
`skynet driver check --results` on the mock; `corpus-report --results` (1,424 of 1,424); then
`skynet bench award` over the three, in both orders above, before and after F43.

```
claimed guest, awarded none
failed   package.signature.required
missing  safety.policy.isolation, package.grants.gate, offline.control.independent, security.credential.scope
```

Controls, each failing and each reverted: `skynet conformance` writing its document without the
read-back fails the bench-row test; a permissive `--results` fails its flag test; `fold` replaced by
the old concatenation fails all three of its tests.

**Not verified, and not claimed.** The ceiling is still `none`, and this is now the full account of
why for a robot built from this tree with every producer run: `package.signature.required` fails by
design (§15.1), `offline.control.independent` is bench evidence `check-offline` records separately,
`safety.policy.isolation` and `package.grants.gate` need a sandboxed unit and a granted package on
the robot being measured — which a bare temporary root does not have — and `security.credential.scope`
needs the Hub. The first is a decision, the second and third are a robot doing work, and the last is
another repository. None of them is plumbing any more, which is the claim F21 made about its first
reason and could not have made true without building it.

### 2026-09-18 (thirteenth session) — the three items the twelfth session left, and the tree it left them on

A Linux container: Rust 1.94.1, Node 22, Python 3.11, gcc 13, no `sudo`-free systemd and no GPU.
**Committed locally, nothing pushed**, on the owner's instruction. *Updated as the session goes.*

The instruction was to finish every section of this plan and not stop until it is done. *Next* 10,
11 and 12 were the three items the twelfth session left open, and the first thing an audit found was
not any of them.

#### F44 — the twelfth session's last commit was never run, and it broke three checks at once

`485cd33` built §89 — capture on its own thread, the placement contract, the corpus's 562 new lines —
and was committed without the gate. On a checkout of it, before a line of this session's work:

| Check | What |
|---|---|
| `cargo build --workspace --all-targets` | **failed.** `Port::offer` took `Option<&Writer>` and now takes `Option<[c_int; 2]>` (§89.3 moved the writer to the capture thread); three call sites in `skynet-policy-host/tests/handoff.rs` were left behind. The *library* compiles, so `cargo build` alone is green and only `--all-targets` sees it |
| `No string literal has a hole punched in it` | **failed**, on two of §89's own messages. F14's defect for the third time, in the same shape: a `\n` and then twenty-one spaces of alignment inside a sentence a person reads |
| `The specification still states the size of the thing it specifies` | **failed.** §73 said 153,000 lines, 92 corpus files and 1,424 cases against a tree at 155,000, 93 and 1,441 |

This is F14's finding arriving a second time and it is worth writing down again, because F14's
remedy existed and was not used: `tools/run-ci-job.mjs` is in the repository, `CONTRIBUTING.md`
names it, and one run of it would have caught two of the three. The third needs `--all-targets`,
which `CONTRIBUTING.md` does ask for. **A gate nobody runs is not a gate**, and the plan's own entry
for that session lists a `Verified` block that cannot have been taken on the commit that was pushed.

Fixed, all three, before anything else: the three call sites take `pool.descriptors()`, the two
messages use Rust's trailing-backslash continuation so the sentence is one line, and §73's figures
are maintenance in both copies and a re-pin (*Decisions* 1 and 10).

#### *Next* 12 — §89's last two steps, and what the second one cannot reach here

The twelfth session built §89.1 through §89.3 and stopped. Steps 5 and 6 of the issue's
implementation order were owed.

**Step 5, `skynet driver check`.** Built, and §89.4 is exact about where: under
`embodiment.descriptor.valid`, beside the read that already happens, reading every declared sensor
on a second thread for 250 ms while this one reads the body. It fails if either side refuses, if a
frame's geometry does not fit its bytes, or if the sequence stands still — and the clause names the
interval, because §89.4's own sentence is that **a race it does not provoke is not found**. The
control is `slow_camera.c`'s new `"lie":true`, which declares the promise and shares one scratch
buffer between the two paths; disabling the check fails the test.

**Step 6's first half.** Built — see F46 for what it does and does not reach. **Its second half,
`drivers/mujoco` rendering from a snapshot and `check-loop-mujoco` re-run, is not built**, and the
blocker is named precisely rather than worked around: this machine has **no offscreen GL**.
`MUJOCO_GL=osmesa` finds no `libOSMesa`, `=egl` finds no `libEGL`, `=glfw` has no display, and
`archive.ubuntu.com` answers 404 through this environment's proxy so none can be installed. The
MuJoCo driver's own suite says so itself — *"96 checks, 0 failed, 1 skipped — camera frames: no
OpenGL context on this machine"* — which is the skip working as designed.

Writing a snapshot render that cannot be executed is the shape F15 was right to refuse, and F24's
measurement is what the section is for. So the row stays owed, with the one thing it needs written
down: a machine with `libosmesa6` or a GPU.

#### F45 — `skynet driver check` called `read_state` with a capacity of zero

`skr_state`'s arrays are annotated `[joint_count]` and `skynet_dl::Driver::read_state` — the runtime
— sets `joint_count` to the caller's capacity on every cycle. This command passed **zero**, and had
since it was written.

That is the same call with a different meaning. A driver that honours what it is handed writes no
joints; a driver that validates it refuses the read outright. The second is what the §89 fixture
does, and it failed `rhal.state.completeness` against `skynet driver check` while working perfectly
under `skynetk` — a conformance run disagreeing with the robot about a driver that is correct.

Found by pointing the new check at a fixture written to the ABI rather than to this command.

#### F46 — the pipe was not the only thing in the way, and the other thing is inside the driver

`docs/issues/sensor-reads-off-the-control-thread.md` predicted the host: *"`drivers/python-host`
serialises every call over one pipe to one interpreter … It would need to multiplex requests before
any Python driver behind it could declare anything."* That is true and it is now done — protocol 2,
a second pipe pair, a `Y` exchange naming the declared sensors, and a thread in the child that
serves them and calls nothing else.

**And no driver in this repository can declare, still**, for a reason a level below the host that
the issue did not reach:

* `skynet_lerobot`'s camera frames are a **by-product of the joint read**. `read_state` calls
  LeRobot's `get_observation()`, which returns proprioception and images together, and caches the
  frames for `read_sensor` to hand back. The two paths do not merely share state — one produces what
  the other returns.
* `skynet_mujoco` renders the same `mjData` its step writes, which the issue did say.
* `skynet_canopen` and `skynet_ur` have no sensors at all.

So the host's share of the obstacle is gone and the driver's share remains, and the driver's share
can only be removed inside the driver. That is §89.1 working as designed rather than failing: the
declaration is the vendor's because the vendor is the only party who can see this.

What holds the mechanism to something real is `fixtures/skynet_slow_camera.py`, the Python analogue
of the C fixture §89 already has, and `make check-concurrent` — three runs, and the middle one fails
the moment the second channel is taken away.

**And the GIL, which none of this touches.** Two threads in one CPython process do not run bytecode
at the same time, so §89.1's *"without making them wait"* is honest for a read that spends its time
inside a C extension that releases the GIL and dishonest for one that does not. The fixture's render
is `time.sleep`, which does release it, so the promise it makes is one it keeps. §31.3 caps Python at
guest for the same underlying reason, and this is the first place the cap has an operational edge
rather than a tier.

### 2026-09-18 (thirteenth session, continued) — Phase 2's last three rows

The same instruction, still running: finish every section, log here, commit locally, push nothing.
With §89 closed, what remained of the plan was the Phase 2 driver table's unbuilt rows. Three were
built in this session — Franka, Unitree, EtherCAT — and each one was taken as far as its *substrate*
honestly allows rather than as far as a green run could be made to go.

#### The three rows, and what each one's evidence actually is

| Row | Substrate | Checks | Through the ABI |
|---|---|---|---|
| `drivers/franka` | our own fake FCI, plus the vendor's headers compiled to pin the wire layout | 79, 0 failed | `guest` |
| `drivers/unitree` | `unitree_sdk2` on **both** sides of the wire | 39, 0 failed | `guest` |
| `drivers/ethercat` | this repository's own master against its own software slave stack | 57, 0 failed | `guest` |

They are not equal evidence and the READMEs say so. Unitree's is the strongest: the CDR encoding,
the DDS discovery and the topic types are the vendor's code at both ends, so the wire cannot be
wrong here without being wrong in the way a real robot would refuse. Franka's is the weakest of the
three, because there is no Franka simulator and the fake is ours — which is F15's problem, and the
answer to it is F47 below. EtherCAT's sits between: the slave stack is ours, but everything it
models is exact arithmetic over ETG.1000's frames, and the one number that makes it a measurement
(the working counter) is asserted on every exchange.

All three declare `simulator: true`, so all three cap at guest (§35.3) whatever the evidence
supports. None of them is bench evidence and none of them claims to be.

**Not dry-verifiable, recorded rather than faked.** Franka: the 1 kHz control loop's real timing,
and the arm's own safety layer. Unitree: joint access at rate, and the absence of any dynamics,
contact or gravity behind the fake's joints. EtherCAT: distributed clocks — `0x0910`'s whole content
is propagation delay measured on copper, so a loopback implementation would be arithmetic over
numbers we invented — jitter, and FSoE. None of the three is stubbed; each is named in its README as
the thing the row cannot tell you.

#### F47 — a wire layout nobody should transcribe, and the way out

Franka's FCI is a TCP command channel on 1337 and a UDP `RobotState`/`RobotCommand` pair, and the
layout of those structs is a C++ struct layout — padding, alignment and field order included. Every
previous reading of this row assumed the layout would have to be transcribed from `libfranka` into
Python by hand, which is a transcription with no oracle: it is wrong silently, in the same way a
gear ratio is wrong silently, and the arm moves on the wrong number.

It does not have to be. **`libfranka`'s common headers are Apache-2.0**, so `layout_probe.cpp`
compiles the vendor's own structs and prints every field's offset and size to `layout.json`, which
the driver loads and the selftest asserts against. The layout is measured from the vendor's compiler
rather than copied from the vendor's source, and a version bump that moves a field fails the probe
instead of the arm.

#### F48 — SOEM cannot be this repository's EtherCAT master, and it is a licence that says so

The Phase 2 table names SOEM as `drivers/ethercat`'s substrate. It cannot be one. **SOEM 2.x is
dual-licensed GPLv3 or commercial**, so a driver in this Apache-2.0 repository cannot link it and
still be distributed under this repository's licence — and no arrangement of the code changes that,
because the combination is what the licence governs. The plan wrote that row without checking, and
the two rows built before it had the opposite answer (`libfranka`'s common headers are Apache-2.0,
`unitree_sdk2` is BSD-3-Clause), which is exactly why the assumption survived to here.

So the master is this repository's own. SOEM keeps the one role a licence does not restrict: an
**independent master** run separately against this slave stack over a TAP device or a real NIC,
which is interoperation rather than combination. That cross-check is *available and was not run*:
it needs `CAP_NET_ADMIN` to create the device and root to open a raw socket on it, and this
container grants neither. SOEM builds here and the bridge script exists; the tap does not. It is the
sharpest test this row could have and the README says it is owed rather than implying it was done.

> **Run, 2026-09-18 (fourteenth session)** — `make check-soem`, inside `unshare -rn` over a veth
> pair, and it is F51 to F53. The blocker named above was this container's, not the work's: a machine
> with unprivileged user namespaces grants `CAP_NET_ADMIN` over a namespace of its own.

#### F49 — two byte offsets, and a working counter that said nothing

Two defects in the EtherCAT stack were found by a control rather than by a failure, and both are the
same shape.

* The CoE SDO field offsets were counted independently in `slave.py` and `master.py` and disagreed
  by one byte. Fixed by naming them once — `SDO_SPEC_AT`, `SDO_INDEX_AT`, `SDO_DATA_AT` — and
  importing them (§80).
* The FMMU `type` and `activate` fields were written one byte early at **both** ends, so they agreed
  with each other and the cycle returned a working counter of **0** with nothing complaining. A
  master that did not assert its expected counter would have read the right number of bytes off an
  empty segment and called them positions.

And a third, in the test rather than the code: `segment.py` copied its slave list, so the "a drive
drops off the segment" check could not fail until the copy was removed. It was written, it passed,
and it had proved nothing — which is CONTRIBUTING.md's rule catching its own author again.

#### Where the plan stands

Phase 2's driver table has no unbuilt rows left. Every row that was going to be built has been, each
against the most honest substrate its vendor's licence and this container permit, and each with its
un-dry-verifiable half written down rather than approximated. §73's drivers figure moved from 14,000
to **16,000 lines**, measured by `tools/scale-report.mjs`, corrected in both copies of the shared
document and re-pinned.

**Still open, and not closable here:** the MuJoCo snapshot-render half of §89 step 6 (no offscreen
GL on this container — no libOSMesa, no libEGL, no display, and `apt-get` is denied), and the
SOEM-over-TAP cross-check above. Both are recorded with their precise blocker rather than as a skip.

> **Both closed by the fourteenth session**, on a machine with Mesa's EGL and unprivileged
> namespaces — see its entry below.

**Committed locally, nothing pushed**, on the owner's instruction.

### 2026-09-18 (fourteenth session) — the two items the thirteenth left, on a machine that can close them

Windows 11 with WSL2 Ubuntu 26.04: Mesa 26 with EGL, unprivileged user and network namespaces,
`/dev/net/tun`, and `sudo` behind a password, so everything installed here is user space
(*Decisions* 14) — MuJoCo 3.13, ONNX Runtime 1.30.0 and cmake in a venv under `~/.skytools/py`, SOEM
v2.0.0 under `~/.skytools/src`. **Committed locally on `claude/dry-coverage-fourteenth`, nothing
pushed**, on the owner's instruction. *Updated as the session goes.*

**Was anything left? Yes, two items, and the plan's own text was stale about three more.** The
thirteenth session's closing lines name both open items: §89 step 6's MuJoCo half, blocked on *"no
offscreen GL"*, and the SOEM-over-TAP cross-check, blocked on `CAP_NET_ADMIN`. Neither blocker holds
here. And *Next* still read *in progress* for items 10 and 12 and *open* for 11, although item 11 was
closed by `4e7fb3f` (`make check-loop-onnx`, 0 overruns, all 249 cycles reproduced) in the
thirteenth session, whose log never says so.

#### §89 step 6's second half — built, and F24 measured closed

`drivers/mujoco` renders on a thread of its own and declares every camera `concurrent_read`.
`read_state` publishes a copied snapshot — `qpos`, the mocap poses, the clock — by replacing one
reference; the render thread owns the GL context from creation to release and an `MjData` of its own,
so the physics' arrays are never read by it and the control thread never takes a lock it holds.

**Measured, on a machine that reproduces F24** — llvmpipe renders the 128×96 wrist camera in 14.4 ms
against a 20 ms period:

| `make check-loop-mujoco` | Overruns | Frames | Where read |
|---|---|---|---|
| before, on this tree as the thirteenth session left it | **3**, then a suspended stream | 3 of 3 | the control thread, suspended at read 3 (§89.2) |
| after | **0** | 51 of 51 | `skynetk`'s capture thread (§89.3) |
| after, `POLICY=onnx` | **0** | 51 of 51 | the same, through ONNX Runtime 1.30.0 |

Both passes replay every recorded cycle. `skynet driver check` through the C host passes §89.4's
250 ms concurrent read (29 pass, 1 fail — roles, as designed — guest), the MuJoCo selftest is 110
checks and 0 skipped where the thirteenth session's container managed 96 and 1, and with no GL at all
it is 100 and 1 — the headless branch, with the new held-render checks still run. (112 and 102 after
the import refusal further down, which adds two checks to each.)

**Where the build departed from the issue.** It sketched *"`mjv_updateScene` under a lock,
`mjr_render` outside it"*. A lock the control thread shares with the render path is a lock it can
wait on, which is what §89.1 rules out, so there is none. The selftest's new check holds a frame
mid-render and requires fifty `read_state` and fifty `write_command` calls to complete meanwhile, and
it needs no GL.

**The GIL, which F46 said none of this touches.** It is why the declaration is honest in Python
only by measurement: a MuJoCo render releases it, so a render beside the control thread raised its
slowest ten-substep step burst from 0.49 ms to 2.1 ms rather than by the render's 14.4. A binding that
held it would make the declaration false, so `check-loop-mujoco` is now a CI step — the first thing
that would say so.

**Controls, each failing and each reverted.** Stepping inside the render lock fails the two
held-render checks. Withdrawing the declaration puts `check-loop-mujoco` back to three overruns and a
suspended stream, with `loop.sh`'s diagnosis now naming §89.1 rather than blaming `osmesa`.

**Not verified, and not claimed.** The CI step has not run on a runner, because nothing was pushed.
`osmesa` on `ubuntu-latest` may render faster or slower than llvmpipe here; the assertion is zero
overruns either way, because a declared stream's cost is no longer the loop's.

#### F48's cross-check — SOEM against the slave stack, run

The thirteenth session left it *"available and was not run"* for want of `CAP_NET_ADMIN`. Here
`unshare -rn` grants it inside a namespace of its own, so `make check-soem SOEM=<build>` puts a veth
pair there, serves the ring on one end (`python -m skynet_ethercat.wire`) and runs SOEM v2.0.0's own
`slaveinfo -map` and `simple_ng`, unmodified, on the other. SOEM is GPLv3 and is only ever *run* —
built in user space from its v2.0.0 commit, never linked, never committed — which is the
interoperation F48 said a licence does not restrict.

**Measured.** All three slaves' SII read (name, identity, mailboxes, CoE), process data sized at 48 +
48 bits from the CoE mapping, each slave's mailbox status mapped through an FMMU, Safe-Op; then Op
and **1,495 of 1,495** cycles at working counter 9, the figure SOEM computes from ETG.1000.4, with
SOEM's own process image showing every drive's statusword as `0x0050`. Then this repository's master
ran over `RawSocketSegment` on the same wire — that class's first execution since it was written —
brought the ring up, read `0x608F:1` over the mailbox and walked every drive to *operation enabled*
and a setpoint. `make selftest` went from 57 checks to 87, `make check` is unchanged (26 passed,
guest).

#### F51 — the slave stack would not have reached Pre-Op under any master but its own

SOEM reads the SII before it configures anything: vendor, product, revision, both mailbox windows and
the mailbox protocols, then the categories. The stack had no SII — the EEPROM registers read zero — so
SOEM would have configured three slaves with no mailbox and no process data and stopped. Nothing
refused, anywhere, because this repository's master never asks: it writes its sync managers from
constants it already holds.

That is the finding, and it is F49's argument arriving from outside: a master and a slave written by
one author agree whether or not either agrees with ETG.1000. Building what SOEM needs, and reading
SOEM's source to know what that was, found five more places the model had been agreeing with itself —
FMMUs that ignored their physical address (SOEM maps one onto every slave's mailbox status, which the
old model would have filled with the statusword, whose bit 3 is CiA 402's *fault*), an `LRW` that
counted a written-only slave as 1 rather than 2, a broadcast read that replaced rather than ORed, DL
status that reported no links at all, and an AL state machine with no transition graph. Each now has
a check in `make selftest` that needs no SOEM, each check was held to a control, and the cross-check
has one too: an SII of zeros — the first version's — fails `check-soem` on eight assertions.

#### F52 — three AL status codes were on the wrong row, and both ends agreed

`ALSTATUS_INVALID_SM_CFG` was `0x001D`, which ETG.1000.6 gives to *invalid output configuration*;
`NO_VALID_INPUTS` and `NO_VALID_OUTPUTS` were `0x001A` and `0x001B`, a synchronisation error and a sync
manager watchdog. Found by reading SOEM's `ec_ALstatuscode2string`, an independent copy of the table.
`master.py` imports the same constants, so a refusal was reported by one and understood by the other
and nothing ever disagreed — F49's shape for the third time in this row. The numbers are now pinned in
the suite against the table rather than against the file's own names.

#### F53 — the slave's documentation promised a refusal its code did not make, and the master leaned on it

`slave.py`'s module docstring said *"a mailbox read with nothing in it answers a working counter of
zero"*; the code incremented it. And `master.py` wrote an SDO request as 16 bytes into a 128-byte
mailbox and read 16 bytes of the answer — which worked only because the model delivered any write and
answered any read. ETG.1000.4's mailbox completes a write on its last byte and a read on its last
byte, so against a real ESC that master's first request would never have arrived. Both are fixed —
whole-buffer exchanges, with the input mailbox's *full* bit polled between them — and it is the
repository's standing doc-comment defect in a new place: a sentence describing behaviour, read as
though it were the behaviour.

**What the cross-check does not show, recorded rather than implied.** A veth pair has no propagation
delay, so distributed clocks are neither implemented nor claimed (every slave says `Has DC: 0`); the
round-trip times are Python's; FSoE is not here. EEPROM ownership, the *multiple write* half of
`ARMW`/`FRMW`, bit-granular FMMUs, sync-manager watchdogs and a drive holding Safe-Op until its first
outputs arrive are not modelled, and SOEM exercised none of them. The row's *not dry-verifiable*
column is unchanged. And over a NIC a master cannot tell these drives from hardware — which is why the
bridge is not pointed at `wire.py`: its `simulator` flag comes from the segment's type.

§73's drivers figure moved 16,000 → **17,000** (17,368 measured), in both copies and re-pinned. The
Hub's half is committed on its own local branch, `claude/dry-coverage-fourteenth-spec`, so the Hub's
`main` checkout on this machine still reads 16,000 and a local `run-ci-job invariants` reports that
one step as diverged (52 of 53) until the branch is merged; `check-mirrors` against the branch
matches, and CI, which has only the pin, will too. The CI step that runs `check-soem` has not run on a
runner.

#### F50 — `0 too old` was a check that never ran, and §89.2 is what showed it

The before-run of `check-loop-mujoco` suspended its stream at the third read, as §89.2 says it
should. The pool kept that frame, and `skynetp` reported *"198 of 200 turn(s) were shown a frame (0
missed, 0 too old)"* — the same frame, four seconds stale by the end, on every one of them. Nothing
was too old because nothing measured age: the lease had no frame budget, since budgets arrive with a
package (§8) and a hand-run `skynetp` declares none, and the zero was printed as though frames had
been checked and passed. The body's line beside it already said *"… cycle(s) with no budget to
check one against"*; the frame line had no counterpart.

It is F19's shape — a count reached by nothing anybody reads — turned inside out: here the number was
read, and it was a number for a check that did not exist. `Summary` gains `frames_unbudgeted` and the
line prints it. What it means for §89.2 is recorded under *What stays open*: the safe direction the
section describes — *"vision degrades and the guards keep their period"* — needs a policy that holds
on an old frame, and that needs a declared budget.

#### And one more branch the first run found

MuJoCo 3.13 resolves `MUJOCO_GL` while it is imported, so `MUJOCO_GL=osmesa` on a machine with no
libOSMesa — `loop.sh`'s default, on this machine — did not reach the renderer probe that turns *no
GL* into *no cameras*: `import mujoco` raised a PyOpenGL `AttributeError` straight out of `open()`.
It is now `ENODEV` with a message naming `MUJOCO_GL` and the library, held by a selftest check that
provokes it on every machine.

#### Verified

```
drivers/mujoco        selftest.py (MUJOCO_GL=egl)           112 checks, 0 failed, 0 skipped
                      selftest.py (no GL at all)            102 checks, 0 failed, 1 skipped
drivers/python-host   check-mujoco                          29 passed, 1 failed (roles), guest; §89.4 passes
                      check-loop-mujoco                     0 overruns, 51/51 frames, 250 cycles replayed
                      BODY=mujoco POLICY=onnx               0 overruns, 51/51 frames, 248 cycles replayed
                      check-loop (lerobot), mlp and onnx    0 overruns, 249 cycles replayed each
drivers/ethercat      make selftest                         87 checks, 0 failed
                      make check                            26 passed, 0 failed, guest
                      make check-soem (SOEM v2.0.0)         every assertion, twice: 1,495 and 1,468 cycles, all at WKC 9
cargo fmt --all --check / clippy -D warnings --all-targets  exit 0 (Windows, 1.98.1)
cargo test --workspace                                      1,887 passed, 0 failed, 21 ignored (Windows)
cargo test -p skynet-policy-host                            38 passed (WSL, where the frame tests run)
node tools/run-ci-job.mjs invariants --wsl                  52 of 53 — the mirror step, for the reason above
node tools/check-mirrors.mjs <the Hub branch>               the specification matches
node tools/scale-report.mjs --check                         §73 matches the tree
```

**Controls, each failing and each reverted.** The MuJoCo lock (two checks); the declaration withdrawn
(`check-loop-mujoco`: three overruns and a suspension); nine ESC rules reverted one at a time (each
fails its own checks); an SII of zeros (`check-soem`: eight assertions); the unbudgeted counter
removed (the new unit test); the import branch removed (both of its checks).

**Not verified, and not claimed.** No robot, again. The two new CI steps — `check-loop-mujoco` and the
SOEM cross-check — have not run on a runner, because nothing was pushed. `drivers/franka` and
`drivers/unitree` were not re-run here; their figures in Phase 2's table are the thirteenth session's.

#### Where the plan stands

Every section is done as far as dry work reaches: Phases 0–5, *Decisions* 1–14 and *Next* 1–13. The
ceiling is still `none`, for F21's and the twelfth session's reasons, and *What stays open* is the
whole of what is left — each item a robot, a vendor, a Hub or a research result.

**One thing waits on the owner, and it is a sentence.** §4 names the EtherCAT reference driver's
stack as SOEM, which F48 showed it cannot be; the thirteenth session established that and never
carried it to the specification. A design sentence is not §73's figure maintenance, so it is filed
rather than made: `docs/issues/ethercat-reference-driver-is-not-soem.md`, **proposed**, with the text
it would take. The other five files in `docs/issues/` are ratified and built.

**Committed locally on `claude/dry-coverage-fourteenth`, nothing pushed**, on the owner's instruction;
the Hub's one-line half is on `claude/dry-coverage-fourteenth-spec` in its own repository, also local.

### 2026-09-18 (fifteenth session) — Phase 6's first two items, and the denominator

Items 6.1 and 6.2 are built, both amendments are filed, and the fleet ran over eight of ten
bodies. Four findings, two of them from the new surfaces meeting the old tree.

**6.1 — the surface.** `skynet readiness` reads the robot with no arguments (`GET /readiness`,
an observer's read beside `GET /conformance`) and evaluates descriptor files with arguments,
through one shared mapping — `subject_from_descriptor` in `skynet-contracts` — so a file on a
workstation and the same file provisioned onto a robot give the same answer. `--fleet` prints
the summary: `by_tier` and `common_blockers`. Three translations in the mapping are worth
recording, because each was a decision: the slug is shown as the name (descriptors carry no
display name, so none is invented); `control_hz` is the declared fastest loop (neither caller
knows the rate the loop runs at — that is `skynetk`'s, in another process); and a described
body counts its describer as the adapter, except a document stamped `skynet descriptor
from-urdf`, which counts none. The stamp is one constant both sides share. A simulated body
that attains a tier prints the §35.3 cap next to it — without that, the surface the phase is
measured by would overstate every simulator in the fleet.

**The denominator, measured.** Descriptors were produced live from eight drivers — `driver
check --descriptor`, no hand-written subjects — and the fleet over them reads: eight bodies
could attempt nothing, seven checks fail on all eight, and `control_modes` and `control_rate`
fail on one each (pendant and lerobot, respectively). F58 predicted `envelope_source` as the
tallest bar; it is, tied seven ways — no driver declares an envelope, an estop capability, or
compute at all. `ur_rtde` and `ros2_control` are the two missing bodies: no URSim (no Docker
daemon, no sudo) and no ROS on this machine. The `unitree` descriptor came from the SDK's
first clone on this box — it builds clean — and `mujoco` from the venv the fourteenth session
left behind.

Robot and file agree where they should and differ where they must: the same mock descriptor
evaluated as a file fails `onboard_compute`, and served by a live `skynetd` over its socket
passes it — the robot measured its own memory and cores, which is not a declaration. That run
is also what found F60.

**6.2 — the lock, the kernel bit, and the machine readings.** `skynet-rt` is the twenty-seventh
crate and the eighth `unsafe`: `mlockall` and `getrlimit` declared rather than depended on,
beside `/proc` and `/sys` readings that need no unsafe at all. `skynetk` prints `mlock` and
`rt` lines in its startup block; `install.sh --check` reports swap and the RT kernel as
machine readings, which fail nothing, because install writes neither and a `check()` there
would blame the installation for the machine. All four install.sh branches were run against
fixtures. Nothing refuses anything: `skynetk`'s header says it decides no tier, and the
refusal is the `who-checks` amendment's to make — which is why the issue recommends it over
the whole §23.5 precondition set rather than the RT kernel alone.

**6.3–6.7, assessed and left.** 6.3 waits on its amendment deliberately: transcribing ratings
without the `basis` field is exactly the confusion F58 names, and the fleet now shows the
before-state it would convert. 6.4 waits on its amendment and on a PREEMPT_RT VM this WSL box
cannot host (no sudo, no KVM — a kernel cannot be installed from user space). 6.5's `unitree`
half is open — the SDK builds here, and its blockers are the fleet's seven — and its
`ros2_control` half needs a ROS machine, as before. 6.6 is four ports behind 6.5 by plan. 6.7
still needs the robot.

#### F59 — two drivers describe fake-backed bodies as real ones, and two describe them as simulated.

`franka` (fake FCI) and `unitree` (fake robot over DDS) declare `simulator: false`;
`canopen` (loopback bus) and `ethercat` (its own slaves) declare `simulator: true`. The
situations are the same — software where a body would be — and the flags disagree. No tier
harm today: both awards stand at guest by mechanism, and the sixteen missing bench tests
dominate any award computed from their evidence. But the award's §35.3 cap reads the flag, and
a fake substrate declaring a real body is the flag pointing the wrong way. The remedy is
`canopen`'s own shape — the flag follows the substrate (`SKYNET_FRANKA_SIM=1` should describe
a simulator) — and it belongs to 6.5 for `unitree` and to 6.6's rewrite for `franka`, not to
the surface that showed it.

#### F60 — a credential issued while the daemon runs is not honored until it restarts.

`main.rs:243` reads the credentials file once at startup and the request path holds the
reference. Three credentials went into the store during the readiness end-to-end; only the one
predating the daemon answered. Out of this item's scope — auth lifecycle, not readiness — and
recorded rather than fixed, because the fix is a design choice (reload per request, watch the
file, a HUP) rather than a line.

#### F61 — `mlockall` under a finite limit sabotages the process's later mappings, and success is the trap.

The first version of `skynet-rt` locked unconditionally and reported the result. Under a 64 MB
shell limit the call *succeeded* — `VmLck 4720 kB` — and the loop then died mapping its 67 MB
black box ring, on a machine with fifteen gigabytes free. `MCL_FUTURE` under a finite
`RLIMIT_MEMLOCK` fails the mappings made afterwards, so a lock that succeeds under a small
limit kills the process and a lock that fails leaves it running. F9's asymmetry, found again
by running the thing.

The control is paired both directions. A pristine worktree at HEAD loops fine where the first
version crashed. And an `LD_PRELOAD` shim that lies `RLIM_INFINITY` about the limit — /tmp
scratch, never shipped — reproduces the crash deterministically under the new code (forced
lock, same 67 MB allocation fails) while the policy beside it declines and seals a window.
The policy is therefore: lock only when the limit is unlimited — the unit file's
`LimitMEMLOCK=infinity`, the only predicate that is not a guess about a footprint that grows
after the call — and the crate exposes no unconditional path to the syscall. The `Locked` arm
itself ran under the shim (`VmLck 4736 kB`); it runs for real first on a robot.

What this means for the amendment: the refusal 6.4 proposes (`--tier host` requires locked
memory) will refuse finite-limit machines, which now reads as *"this machine is not configured
as a robot"* rather than a failure. CI and developer shells stay finite, stay unlocked, and
stay green.

#### F62 — `positional` reads every `--flag` as taking a value, and a boolean flag eats the next file.

`skynet readiness --fleet a.json` evaluated a fleet of one: `--fleet` consumed `a.json` as its
value. The fix is local — the command strips its one boolean before the scan — but the trap is
still in the shared helper for the next boolean flag before a positional. A `positional` that
knew which flags take values would fix it generally; that is a refactor of every command's
parsing, and this session declined it in favour of the three lines.

**Verification.** `fmt --check`, `clippy --workspace --all-targets` at Rust 1.98.1 (installed
to match CI — F7's lesson, applied before the gate rather than after it), 2,005 workspace
tests with 0 failed, `invariants` 53 of 53, `check-loop` closing, the new `skynetk` wiring
test with `--ignored`, a Windows-target `cargo check` for the `cfg`'d branches, and the
`tsc`/`golden-path` steps green after a Linux `node_modules` refresh (the checkout's was
Windows-native — environmental, nothing committed). Production-callers holds at 1,042
checked.

**Not verified, and not claimed.** The two CI steps' worth of `skynet-rt` on a runner; the
`Locked` arm on a truly unlimited machine (the shim is a control, not the configuration); the
fleet's two missing bodies; anything requiring the RT VM or a robot. The gate's point 1 runs
but not on the gate's machine, and points 2–4 wait on the amendments and the harness with it.

**§73 maintenance.** Both copies: 156,000 lines across twenty-seven crates, 66 CLI commands —
re-pinned. The Hub copy on this machine also carried the fourteenth session's one-line lag
(16,000 drivers against our 17,000); it is synced as part of the same edit, faithfully, not as
a new figure.

#### Where the plan stands

Phase 6 is two items built, two amendments filed, and a denominator: eight bodies attempting
nothing, for seven missing declarations. What stays open is the same shape as before, one tier
up — the owner (two issues to ratify), a real-time kernel, two substrates (`ur_rtde`,
`ros2_control`), and the robot 6.7 has always needed.

**Nothing committed, nothing pushed.** The tree is uncommitted on
`claude/dry-coverage-fourteenth`, and the Hub's §73 half is an uncommitted file edit in its
checkout — the pin in this tree depends on file content that exists only on disk, so the next
push must carry both or neither.

### 2026-09-18 (sixteenth session) — 6.3 transcribed, and the stops that were never declared

The six real-interface bodies now declare what they have: three envelopes transcribed from
vendor publications with the citation beside the figures, three nulls with the reason beside
the null, every simulator flag defaulting the safe direction, and the software stop declared
everywhere it is implemented. The fleet over ten descriptor files reads **none 3 / guest 7**,
and the three that remain are the harness bodies, left deliberately.

**The three transcriptions.** `ur_rtde` carries per-model ratings — speeds from each e-Series
model's own tech sheet axis-movement table, torques from UR's "Max. joint torques" support
article tabulating joint sizes 0–4 — because the controller never says which model it is and
the narrowest-across-the-range single ceiling would brick a UR16e at its wrists' 9 Nm. An
absent or untranscribed model (CB3, UR20, UR30) means no envelope, said aloud at `open()`.
`franka` transcribes `franka_description`'s `robots/fr3/joint_limits.yaml` exactly, no margin,
on the header's bar: the vendor's own table, which their stack enforces, is the vendor saying
they are the ratings — a brochure figure would stay out. `unitree` declares the Go2's 45 Nm
peak with the spec page's own footnote quoted in `source` (the largest motor's; varies across
the twelve), and no velocity ceiling, because Unitree publishes no joint speed and the sim
configs that carry one are the CAD values §63.4 forbids. All three declare `safety.estop` and
no rated stop: software only, in every case for the reason the bridge states — no frame on any
socket reaches the rated circuit, and Franka's is a button on the Control box. All three take
F59's remedy as configuration: the simulator flag defaults true (direction of safety, §35.3)
and a real integration says so deliberately, `SKYNET_FRANKA_SIM=1` and friends for the fakes.

**The remainder.** `lerobot` gets the simulator flag it was missing (the fake implements the
same `Robot` interface as hardware, so the default-true is load-bearing) and the `safety.estop`
its `emergency_stop` docstring already assumed — torque-disable on the motor bus where the bus
offers it, latched, later commands refused. `canopen` and `ethercat` needed only the
capability: both already read travel from the drives' own objects, refused `0x6076` as a
design value, followed the substrate on the simulator flag, and covered latch-plus-refuse in
their suites. The canopen docstring's "declares `estop: null`" was stale vocabulary for the
absence it is now precise about — the envelope carries no rated stop; the software stop is
declared. F63 is what this remainder found.

**The amendment tension, stated plainly.** This item was to wait on the `declared`-versus-
`measured` marker, and the transcription proceeded without it: every envelope carries `source`
and no kind, so F58's confusion — a cited design value reading downstream exactly like a
measured rating — is live in these three envelopes until the amendment lands and the marker
goes on. The mitigating fact is the citations themselves: each `source` names the publication
and, where the publication hedges (Unitree's footnote), quotes the hedge. But a consumer that
reads only the numbers cannot tell them from bench evidence, and that is the amendment's to
fix, not this item's.

**The fleet, re-run.** Ten files (ur twice: undeclared model beside UR5e), none 3 / guest 7.
`estop_software` blocks only the three harness bodies: `mock` and `mujoco` on that alone,
`pendant` also on `control_modes` — which is deliberate, a program-level controller taking no
joint setpoints (§59.2). Whether the teaching drivers should declare the stop path they
implement is a design question, not a transcription, and it stays open with the owner rather
than being absorbed into this item.

**URSim, attempted and unavailable.** The Docker daemon is down on the Windows side, WSL
integration is off, and all five controller ports are closed — there is no simulator to run
against from this box. Nothing in 6.3 blocked on it: the ratings came from vendor PDFs, and
the protocol bytes were measured against URSim in Phase 2. The `ur` descriptor in the fleet
is the fake-backed one, declared simulated.

#### F63 — six drivers implemented the software stop and none of them declared it.

Every one of the six real-interface bodies latched its stop, showed it in the next state, and
refused later commands — the suites covered all three halves — while the descriptor claimed no
stop capability at all. The readiness surface therefore failed `estop_software` on bodies
whose stop path worked. The cause is vocabulary, not behaviour: each bridge's author read
"estop" as the rated, independent stop (§15's bench test) and rightly refused to claim it,
while the capability means the software stop (`readiness.rs`: "Exposes a software emergency
stop", required for guest) and the rated claim is the envelope's to withhold. Six one-line
declarations, each beside the comment naming the stop the driver actually has, moved the
fleet's tallest remaining bar.

**Verification.** `make check` for the three remainder drivers: lerobot 73, canopen 63,
ethercat 88, all 0 failed (the lerobot stderr lines are the suite's own negative-path
probes). The new checks bite on the old code by construction: lerobot and ethercat had no
`capabilities` key (a `KeyError`, not a pass) and canopen's was `[]`. The fleet files for the
three were re-dumped live from the bridges against their suites' fake substrates
(/tmp scratch, never shipped) and `skynet readiness --fleet` re-run. No Rust changed, so no
workspace test re-run is owed beyond the fifteenth session's 2,005; no corpus, edition, or
mirror changed, so no `.mjs` gate is affected (and node is absent on this box in any case).

**§73 maintenance: none.** Python drivers only — no Rust lines, no corpus cases, no figure
edits in either copy, no re-pin.

#### Where the plan stands

Phase 6 is three items built, two amendments filed, and a fleet in which every real-interface
body attempts guest. What stays open: the owner (two issues to ratify, plus whether the
teaching drivers declare their stops), a real-time kernel, two substrates (`ur_rtde` live,
`ros2_control`), and the robot 6.7 has always needed.

**Nothing committed, nothing pushed.** The tree is uncommitted on
`claude/dry-coverage-fourteenth`, now with the six driver transcriptions beside the 6.1/6.2
work, and the Hub's §73 half is still the uncommitted file edit — both or neither, as before.

### 2026-09-18 (sixteenth session, continued) — 6.5's unitree half, minus one vendor figure

Unitree entered with two host blockers and leaves with one. `platform.onboard_compute` is
declared — the Go2 carries a published 8-core user-programmable CPU (unitree.com/go2,
footnote [4]; the secondary development manual is the programming), which makes it the only
body in the fleet that earns the token: a UR controller or a Franka Control box is a closed
computer the OS could never own, and a motor bus is not a computer at all. The comment beside
the token states what it does not claim: that the OS runs there today, let alone on a
real-time kernel. Suite at 46 checks, 0 failed; the fleet file re-dumped live through
`driver check --descriptor` against the fake.

The remaining blocker, `joint_ratings`, is undrivable by transcription, and the search that
established that is worth recording so nobody repeats it: the specs page publishes torque and
range of motion only; the SDK's headers, configs and Go2 examples carry no velocity bound;
the download center (which would hold the user manual) is JavaScript the fetchers cannot see
through; the web has body speeds in m/s and nothing per joint. No published Go2 joint
velocity with §63.4 standing exists in reach — and a motor rpm even if one surfaced would
need the unpublished reduction ratio to become a joint figure. F64 is the shape of that.

The check was not weakened to fit. `joint_ratings` demands both ceilings because without a
velocity ceiling the kernel cannot clamp velocity commands, and a torque-only envelope with a
footnote is still a kernel guessing at half its job. The blocker therefore waits on either a
vendor publication or a bench measurement — 6.7's, which needs the robot — and 6.5's unitree
half is exactly that sentence plus the gate's RT VM.

#### F64 — the Go2 publishes no joint speed, and the host tier needs one.

Three of the six transcriptions cleared `joint_ratings` from vendor publications (UR tech
sheets, Franka's enforced table). The fourth cannot: Unitree's published joint figures are
torque and travel, and everything else called a Go2 velocity is either a body speed, a sim
config's CAD value, or example code. So the first body in the fleet to approach
host-attainable is held one declaration short by its vendor's spec sheet — which is also the
first case where the vendor's to-do list that readiness prints is addressed to a vendor
rather than to this repository. The `source` the envelope would cite does not exist yet.

**The rest of the phase, confirmed unreachable from here.** No ROS on the box (no `ros2`, no
`colcon`, no `/opt/ros`), so 6.5's `ros2_control` half stands as before. `/dev/kvm` exists
but sudo needs interactive auth on a stock WSL2 kernel, so the RT harness assessment stands
with 6.4's amendment still unratified. 6.6 stays behind 6.5 by plan; 6.7 still needs the
robot. This is Phase 6 at its honest limit on this machine: everything transcription and
declaration can do is done, and what remains is ratification, a kernel, a ROS machine, a
vendor figure, and hardware.

**Verification.** `make check UNITREE_SDK2=/tmp/unitree_sdk2`: 46 checks, 0 failed, the
fake's counters clean (7 commands, no bad CRC, no sentinel or phantom violations). Unitree's
readiness re-run: one host blocker (`joint_ratings`), one certified (`estop_hardware`, out
of this phase's scope by the plan's own "Certified is out of scope").

**§73 maintenance: none.** One C++ driver and its suite — no Rust, no corpus, no editions.

#### Where the plan stands

Phase 6 is three items built, one item half-built to its one remaining vendor figure, two
amendments filed, and a fleet in which every real-interface body attempts guest and one is a
single declaration from host. What stays open: the owner (two issues, the teaching-driver
stops, and now F64's vendor figure), a real-time kernel, a ROS machine, and the robot.

**Nothing committed, nothing pushed.** Uncommitted on `claude/dry-coverage-fourteenth`; the
Hub's §73 half is still the disk-only edit.

### 2026-09-18 (seventeenth session) — the pins catch up, and 6.6's first port

Three loose ends tied before the port: both amendments verified present in both spec copies
(the trees already agreed — the carry predates this session) and the mirror re-pinned from the
agreeing copies; the franka and unitree fleet files re-dumped live from their bridges
(`declared` basis on both, simulator true); ethercat's fleet file confirmed current rather
than re-dumped, its null envelope being §63.4's correct answer rather than a missing one. The
readiness re-run reads none 3 / guest 7 over ten files — the recorded state, reproduced.

**6.6 assessed, then begun out of order.** The four ports sized (code lines, comments out):
franka ~675, ur ~580, canopen ~880, ethercat ~400 — roughly 2,500 lines of driver logic with
~1,900 lines of Python suite to measure against. Measurability, not size, orders them: franka
is the only one whose fake speaks over real sockets to any language, so a port can be held to
the Python driver's behaviour on the shared substrate. UR's live half needs URSim (absent),
canopen's loopback bus is in-process Python, ethercat's sim path is in-process in the bridge.
So franka first — and started before 6.5 completes, which the table forbids and the work
permits: the dependency was Host machinery proven before four ports depend on it, and the
first port depends on neither the RT kernel nor the ROS machine. Waiting would idle the item
on hardware it does not touch. Stated here rather than smuggled.

**The port: `drivers/franka-rs`.** Rust, from `skynet driver new --lang rust` so the shape is
conformant by construction, added to the workspace. Three measurement layers, weakest claim
first: `tests/layout.rs` holds every offset and size to `layout.json` (Franka's compiler,
not the Python transcription); `tests/vectors.rs` holds every pack/unpack function byte for
byte to eleven wire messages generated from `protocol.py`; `tests/live_fake.rs` drives the
whole §24 surface through the real vtable against the Python fake — including the descriptor
byte-identical with the Python bridge's. Then the sim half of SCTS against the release .so:
26 passed, 0 failed. Four deliberate differences from the Python driver (a lock-free stop
socket, the latch re-checked under the lock, a malformed port refused, per-joint modes
unread) are documented in the crate; everything else is the Python driver's structure,
translated — including the mutex and the blocking reads, which is 6.6's stated negative: a
language that permits the ceiling, not an approach to it.

#### F65 — both drivers in the tree stamp `CLOCK_MONOTONIC`, and the RHAL documents `_RAW`.

The port needed a stamp clock and found the tree unanimous against the document: the Python
franka driver stamps `monotonic_ns()`, unitree stamps `steady_clock`, and `skr_state`'s
`stamp_ns` says `CLOCK_MONOTONIC_RAW at acquisition` in all three spellings. The port follows
the tree — behaviour-identical with the Python driver is the item's stated measure — and the
discrepancy is the owner's: either two drivers change clocks or one sentence changes words.
Unobservable in any test either way, which is presumably how it survived.

**Verification.** `cargo test -p skynet-franka-rs`: 21 passed (6 unit, 5 layout, 9 vectors,
1 live, 0 skipped); `make check` in the crate: those plus SCTS 26/0; `cargo test --workspace`:
70 suites green; `cargo fmt --check`, `clippy -D warnings`, contract mirrors, rhal flags, and
both mirror pins green. CI gains the crate's `make check` beside the Python franka step.

**§73 maintenance.** Drivers lines 18,000 → 20,000 in both copies, re-pinned. Two things worth
naming: `scale.json` was stale beyond this change (earlier Phase-6 Rust never re-pinned it —
crates 26→27, +1,587 Rust lines arrived in the same regeneration), and the crate count stands
at twenty-seven because the tool counts `crates/` while the port lives in `drivers/`, where
its Rust counts toward drivers lines. The taxonomy is by purpose, not language, and it held
without an edit.

#### Where the plan stands

Phase 6 is four items built (6.6 one port of four), one half-built, two amendments carried,
and a fleet reproduced. What stays open: the owner (F65's clock, F64's vendor figure, the
teaching-driver stops), a real-time kernel, a ROS machine, three ports, and the robot 6.7 has
always needed.

### 2026-09-18 (seventeenth session, continued) — 6.7: the runbook, unrun, and the gate's sentence corrected

**The runbook is `docs/bench-runbook.md`.** Sixteen procedures — the fifteen host-required
bench tests plus `offline.control.independent`, which is guest-required and therefore the
first thing a host session depends on — each with preconditions, steps, what to record,
the pass rule, per-body notes for all six real-interface bodies, and the instrumentation
named. Run order is calendar order (the week-long reconcile test first), the certified-only
three are named and excluded, and the document states its own status on the first line:
unrun, with the first execution owing back runbook defects, instrument gaps, and the two
provisional declarations. Appendix A tabulates every body's declared figures from the
Phase-6 fleet descriptors; appendix B gives the `bench record` / `bench award` session
close-out. Three driver READMEs went stale in 6.3 (all three still claimed null envelopes
after the transcription) and were corrected against the bridges as part of the per-body
work — doc rot found by writing procedures against prose that described a body that no
longer exists.

Every cited figure in the runbook was verified against the tree before it stood, and three
did not: payload numbers from memory (cut — the session weighs the load manifest, the
runbook does not remember it), a Go2 camera count from memory (generalised to
undeclared-sensors language with a verify-against-hardware step), and two stop paths
misremembered (EtherCAT is CiA 402 quick-stop controlwords like CANopen; `ros2_control`
is a pure latch — both corrected against the sources).

#### F66 — the gate's point 4 names a sentence the award cannot say.

`none with sixteen missing and zero failures` is unreachable, twice over, and the probe
that establishes it is in `/tmp` (synthetic evidence, never shipped): complete sim
passes plus the machine-witnessed guest bench test return **guest with fifteen missing
and zero failures**. The logic breaks at the first unsatisfied tier, so sixteen
cumulative bench tests can never display as one `missing` list — guest green is the
price of seeing host's fifteen, and guest green is best `guest`, not `none`. The fifteen
are host's own bench tests; the sixteenth is guest's, already passed in that input. The
gate's *intent* stands (everything dry green, everything remaining a bench session) but
its sentence needs the owner's correction to `guest with fifteen missing and zero
failures` — or, if the owner wants the sixteen displayed together, an award-logic change
to report cumulative gaps, which is a design decision, not a transcription. No corpus
case pins the observed sentence until that is ratified: pinning this session's reading
first would be deciding it.

**The gate, assessed.** Point 1: met for every body with a descriptor — ten files, none
3 / guest 7, reproduced this session; `ros2_control` has no descriptor to run over
until its ROS machine exists. Point 2: not met — unitree stands on F64's vendor figure,
`ros2_control` on its machine. Point 3: half met — the refusal is built and
fixture-tested both directions, naming what it read; the RT-kernel control needs the RT
VM. Point 4: verified as F66 above — the mechanism is sound, the sentence was wrong.

**Verification.** The runbook's claims check out mechanically where they can: all
sixteen ids are suite `bench` with `required_from` in {guest, host} per
`certification.rs`; the fifteen-plus-one split reproduces the plan's 33/17/16
arithmetic; every `skynet` invocation in appendix B was run for flags (the award probe
above exercised `record` and `award` end to end, including the three refusals —
results-doc bench smuggling, valueless quantitative pass, absolute artefact paths).
`cargo fmt --all --check` is green (two mechanical reflows in earlier sessions'
uncommitted §5.3 code, whitespace-only, logged here because the lines are not this
session's). `cargo test --workspace` green. The workspace clippy gate fails on one
pre-existing error — `skynetd-io/src/checkin.rs:172`, eight-argument `build` from an
earlier session's uncommitted §23.5 work — which is a signature refactor in lines this
session did not touch, reported, not taken.

**§73 maintenance: none.** A document, three README paragraphs, CI comment surgery, and
whitespace — no Rust lines, no corpus cases, no figure moves, no re-pin owed.

#### Where the plan stands

Phase 6 is five items built (6.6 one port of four), one half-built, two amendments
carried, a runbook written, and a gate assessed point by point with one sentence
corrected. What stays open, finally: the owner (F64's vendor figure, F65's clock,
F66's sentence, the teaching-driver stops), a real-time kernel, a ROS machine, three
ports, the declarations in the runbook's shopping list — and the robot, which was
always where this phase stops.

**Nothing committed, nothing pushed.** Uncommitted on `claude/dry-coverage-fourteenth`.

> **Since landed.** The owner committed the fourteenth to seventeenth sessions' tree as `5b2c595`
> and merged it as pull request #21 (`c3ea957`), with both amendments carried into the
> specification as §5.3 and §23.5's new paragraph.

### 2026-09-19 (eighteenth session) — the owner's three calls, the real-time kernel, and a ROS machine

Started from `main` at `c3ea957`, on a new local branch `claude/dry-coverage-eighteenth`. The
audit before any work: Phases 0–5 complete and their *Next* list struck through; Phase 6 at 6.1,
6.2 and 6.3 done, 6.4's refusal built but never held to its control, 6.5 half-driven, 6.6 one
port of four, 6.7 written unrun. The table said 6.3's marker and 6.4's refusal were waiting on
amendments that had in fact been ratified and built in the owner's commit — corrected above.

**`main` was red, again, on one clippy error** — `checkin::build`'s eight arguments, which the
seventeenth session reported and did not take. F7's first row, one more time: a tree the owner
merged with CI unable to pass. The per-pass inputs are one struct now (`checkin::Observed`); no
behaviour change. Baseline after it: 2,029 tests passing, 0 failing, clippy clean.

**The owner's three calls, asked and answered** — *Decisions* 15. F66 corrected in the gate and
pinned by a corpus case; F65 taken as *the tree follows the specification*; the teaching drivers
declare the stop they implement (`mock`, `mujoco`, and `pendant`, which turned out to implement
one too — suites 31/0, 113/0, 43/0).

**F65, and the half of it nobody had named.** Seven drivers and a test fixture stamped
`CLOCK_MONOTONIC`. On this WSL2 kernel, five seconds after boot, `CLOCK_MONOTONIC` was already
**112 ms ahead** of `_RAW` — a stamp on one aged against the other is off by more than a 100 ms
watchdog window before the machine has finished booting. The bindings now carry the one right
answer (`skynet_rhal.acquisition_ns()`, `skynet::acquisition_ns()`), every driver uses it, and
`skynet_rt::monotonic_raw_ns` is the workspace's. F67 is what doing it found.

#### F67 — the loop and the policy each counted time from their own start, so every age across the ring was a difference of start times.

Both `SystemClock`s were `Instant::elapsed` from an `Instant` taken when the clock was made, and
`skynetk`'s own loop used another one from its own start. The kernel host's watchdog computes
`now_ms - command_ms` with `now` from one process and `command` stamped by another; the policy
host computes an observation's age as its own reading minus a driver's boot-origin stamp. Neither
subtraction meant anything. The watchdog saw the policy's start offset as age; the policy's
saturating subtraction turned a boot-origin stamp minus a process-origin reading into **zero** —
so §25.1's observation budget and §47.6's frame budget could never have fired against a real
driver. Nothing tripped because no production lease declares a budget and no descriptor in the
loop harnesses declares a watchdog window; the defect was waiting for the first one that did.
Both documented `CLOCK_MONOTONIC_RAW where the platform has it`, which was true of neither.

Held by `the_policy_and_the_loop_read_one_clock` — two clocks made 20 ms apart must agree; against
the old ones they read **20,107,577 ns and 70 ns** — and by `another_process_reads_the_same_clock`,
which puts a Python reading of the raw clock between two Rust readings and, pointed at
`CLOCK_MONOTONIC` instead, fails by 106 ms. A new `invariants` step refuses the known spellings of
the wrong clock reaching a stamp; on the parent commit it finds all ten sites.

#### F70 — §35.3 says sim time *is* the kernel's time, and the loop has never read it.

*"Because the kernel's time is supplied by the RHAL rather than read from the OS, sim time simply
is the kernel's time"* — and the MuJoCo driver's own provenance repeats it. The loop does not do
it: `skynetk` reads its own clock whatever the body declares, and so does `skynetp`. Before F67
that was hidden by coincidence — a process-origin reading minus a sim-time stamp is two numbers of
similar size shortly after start — and after F67 it is plain: against a simulator the policy host
now computes observation ages as boot-relative raw time minus simulated time. Nothing trips today,
for F67's reason (no lease declares a budget), and a real body is correct either way, because its
stamps are on the raw clock now. But a simulated run with a declared budget would hold on every
cycle, which is §35.3's own warning — *"every simulated run would trip the watchdog within a few
cycles"* — arriving through the budget instead. Not fixed here: taking the loop's `now` from the
RHAL for a body declaring `simulator: true`, and carrying that time to the policy across the ring,
is the design §35.3 describes and nothing yet implements, and it touches both processes and the
ring's contract. Raised for the owner rather than half-built.

**6.4's control, run.** `packaging/rt-vm.sh` is the harness the phase names: `fetch` unpacks
Ubuntu 26.04's own `linux-image-realtime` and `linux-image-generic` and the QEMU and busybox the
machine lacks, with `apt-get download` and `dpkg-deb -x` — no root, nothing system-wide, inside
*Decisions* 14's permission — and `check` boots one initramfs under both. Under TCG (no
`/dev/kvm` access) both boots take seconds. The real-time kernel: `/sys/kernel/realtime` 1,
`mlock locked, VmLck 3764 kB`, no swap, tier Host / Enforcing, 50 cycles, exit 0. The generic
kernel: *"this machine cannot back `--tier host`: the kernel cannot be shown to be PREEMPT_RT
(/sys/kernel/realtime could not be read)"*, exit 1. **The gate's point 3 is met.** Not bench
evidence, and the script says so: no body, and a period under emulation is the host scheduler's.

**6.5's `ros2_control` half, on a ROS machine at last.** No sudo, so ROS 2 Jazzy came from
RoboStack (conda-forge) through a user-space micromamba. The first build outside `/opt/ros` found
F68; after it the suite runs **51/0** and SCTS's sim half **26/0**, and the driver has a
descriptor the fleet can read for the first time.

#### F68 — the ros2_control Makefile only compiled where nothing but ROS lives under `include/`, and its stamp was a counter.

It added `-I` for every directory under `$(ROS_ROOT)/include`. In a conda prefix that includes
`include/dds/`, whose `features.h` then shadowed glibc's and `<cstdio>` stopped compiling. Only
ament package directories (`include/<pkg>/<pkg>`) are added now — every directory the bridge's
headers actually resolve through is one — and apt's layout is unaffected. The same run found the
bridge stamping a counter that began at one second and added a millisecond a read: a clock
nothing else shares, and one the F65 gate's patterns could not see. It stamps
`skynet::acquisition_ns()` at acquisition now.

#### F69 — `ros2_control` hosts a body it cannot name, so its host blockers are the integration's.

Over `mock_components` its descriptor fails five host checks: `envelope`, `envelope_source`,
`watchdog`, `joint_ratings`, `onboard_compute`. None of them is the driver's to fill. A
`ros2_control` host is whatever robot the operator's URDF and hardware plugin say it is, and the
URDF's `<limit effort>` is exactly the CAD value §63.4 forbids — the same reason `roles=` and
`simulator=` are the operator's to supply. The route for those declarations already exists and
is not the driver: §28's provisioned descriptor, which `driver check --descriptor` writes and an
integrator completes with a cited envelope, and which `GET /readiness` reads on the robot. And
`onboard_compute` is **measured** there — `skynetd` counts the cores and memory of the machine it
runs on, and the check passes on them — so the file-based fleet view overstates that blocker for
every body whose OS machine sits beside the robot rather than in it: `ros2_control`'s, and the
FCI and RTDE workstations `franka` and `ur_rtde` run on, which is how §59.7's first row puts them
at Host. So gate point 2's `ros2_control` half is not "blocked on a ROS machine" any more; it is
blocked on a *body* — the same category as a robot — and adding config keys to the driver to
make the file look ready would be the driver answering for an integration it cannot see.

**F64, searched once more and closed as far as searching can.** The one Go2-adjacent joint speed
on the web is Unitree's GO-M8010-6 page — 30 rad/s at a 6.33:1 reduction — and that is the Go1's
actuator sold on its own; a Go2 teardown measured its gearbox at 6.22:1, so it is not the same
part. Citing it would be citing a rumour about a different robot. `unitree` stays one vendor
figure from host-attainable.

**The fleet, over every body in `drivers/` for the first time** (gate point 1 in full): twelve
descriptors, all produced live — `ur_rtde` twice (undeclared, UR5e) through the bridge's own
builder because URSim is absent, `ros2_control` and `franka-rs` for the first time. **none 1 /
guest 11**, against the seventeenth session's none 3 / guest 7 over ten: `mock` and `mujoco` rose
on their declared stops, `pendant` stays at none on `control_modes` by §59.2. Host blockers per
real-interface body: `unitree` — `joint_ratings` only (F64); UR5e and FR3 (both editions) —
`onboard_compute` only, which a deployment measures (F69); `canopen`, `ethercat`, `lerobot` and
`ros2_control` — the undeclared-envelope set, each for its own documented reason.

**§73 maintenance.** One corpus case: 1,447 → 1,448 in both copies, re-pinned with `--pin`; the
editions report regenerated (four complete editions at 1,448 of 1,448, their ratchets raised to
1,448) and synced into the Hub with `--sync`. Rust lines stay 157,000.

**Verification so far.** `cargo fmt --check`; clippy `-D warnings` at 1.98.1 clean on Linux and on
the Windows target for the changed crates; `cargo test --workspace` 2,032 passed / 0 failed (70
suites); `invariants` 54 of 54 on this machine with the new step; `corpus` job 10 of 12 on this
machine — the other two are the mirror having no `.git` and the editions figure before its
regeneration, both then re-checked directly; every driver suite this touched re-run (lerobot 73,
canopen 63, franka 88, ethercat 88, ur_rtde 54, unitree 47, mock 31, pendant 43, mujoco 113,
franka-rs 21 + SCTS 26, ros2_control 51 + SCTS 26); `check-lerobot`, `check-capture`,
`check-concurrent`, `check-loop`, `check-offline`, `check-mujoco` and `check-loop-mujoco` all
closing; `packaging/rt-vm.sh check` 8 of 8.

### 2026-09-19 (eighteenth session, continued) — 6.6's substrates, and a defect in the specification the ports are measured against

The seventeenth session sequenced the three remaining ports by measurability and found two of them
unmeasurable here: `canopen`'s substrate was an in-process Python bus, and `ur_rtde`'s needed URSim
with no Docker daemon. Both changed this session, and the first one to change found a defect in the
driver that is supposed to be the executable specification.

**`canopen`, on a kernel bus.** This WSL2 kernel carries `CONFIG_CAN_VCAN` and `CAN_RAW` as modules,
and an unprivileged network namespace (`unshare -rn`) may create a `vcan0` and open raw CAN sockets
on it — no root. So the Python driver's own simulated drives can run in a process of their own on a
real kernel bus, and a master in another process, in any language, can talk to them. That is the
live substrate `canopen-rs` needs, and it is what F71 is.

#### F71 — the CANopen driver's reads had only ever met a bus that answers inside the read.

`read_state` sent SYNC and then drained the bus without waiting. On `LoopbackBus` the drives answer
inside `recv`, so that worked for every run the suite had ever made; on a kernel bus the answers
arrive microseconds after the SYNC. Against three simulated drives on `vcan0` the first read of a
session returned `ETIMEDOUT`, and every read after it reported the **previous SYNC's positions
under its own fresh stamp** — §24's stamp lying by one period, silently, on every cycle. And
`make check-vcan` could not have found it: it checked that the kernel had CAN support and then ran
the loopback suite again, so SocketCAN had been written and never run.

Fixed in the Python driver first, because it is the specification the port is held to: a read now
drains what earlier SYNCs left, stamps at the SYNC (the instant a CiA 402 drive samples its
synchronous PDOs), and waits up to a bounded window — `sync_window_ms`, 5 ms by default — for every
axis's TPDO2; some axes silent is `FLAG_DEGRADED`, all silent is still `ETIMEDOUT`. The suite gains
a late loopback bus, a bus that loses one axis's TPDO2 and a zero-window refusal, and `--vcan` now
means a kernel bus: `skynet_canopen.drives` serves the simulated drives in another process and
`vcan.sh` supplies the `vcan0`. **Against the old bridge six of the new checks fail; against the
fixed one 79 of 79 pass**, `make check` 71/0, SCTS 26/0. CI runs `check-vcan` where the runner's
kernel can load `vcan` and reports it not run where it cannot.

**`ur_rtde`, with its controller.** Docker Desktop was installed and stopped; it was started, and
`universalrobots/ursim_e-series:latest` pulled into it — the substrate the Phase 2 driver was
measured against, under *Decisions* 14's permission for container images. Nothing has been run
against it yet in this session.

**`canopen-rs`, begun.** `drivers/canopen-rs/tests/gen_vectors.py` generates golden vectors from the
Python driver's `frames.py` and `cia402.py` — requests packed, replies parsed, every statusword and
controlword, unit conversions across rounding boundaries — and is written to be committed beside its
output, unlike `franka-rs`'s generator, which lived in `/tmp` and is gone (its `vectors.json` note
names a path that no longer exists). Both files are on disk and **deliberately not committed**: a
directory under `drivers/` that is not yet a crate would be counted in §73's driver lines and read
as a driver that does not exist.

**§73 maintenance.** F71's fix and its substrate took the drivers over a rounding boundary: 20,000
→ 21,000 lines in both copies, re-pinned with `--pin`; `scale.json` regenerated over the committed
tree.

#### Where the plan stands

Stopped here on the owner's instruction of 2026-09-19. Phase 6 against its gate:

1. **`skynet readiness --fleet` over every body in `drivers/`** — met: twelve descriptors, `ros2_control`
   and `franka-rs` included for the first time, none 1 / guest 11.
2. **`ros2_control` and `unitree` bench-only** — not met, and neither is this repository's to meet:
   `unitree` waits on a vendor figure (F64), `ros2_control` on a body to describe (F69).
3. **`--tier host` refuses on a stock kernel, starts on PREEMPT_RT with `VmLck` non-zero** — met, by
   `packaging/rt-vm.sh` on Ubuntu's own real-time kernel.
4. **`guest` with fifteen `missing` and zero `failures`** — corrected by the owner and pinned in all
   eight editions.

Item by item: 6.1–6.4 done; 6.5 driven as far as dry work goes; **6.6 is one port of four, with
`canopen`'s substrate made live, `ur_rtde`'s pulled, and `canopen-rs`'s vectors generated — the
three ports themselves are the remaining work**, `ethercat` last, over the veth pair F48's
cross-check already uses; 6.7 written and unrun, as it must stay until a robot. For the owner: F70
(§35.3's sim time was never wired) and the three ports. Beyond this repository: F64's vendor
figure, a body for `ros2_control`, and the robot.

**Committed locally on `claude/dry-coverage-eighteenth`, not pushed.** The Hub's half — its copy of
the specification (the corpus count and the drivers figure), its mirrored editions report and its
pin — is an uncommitted edit in the Hub's checkout, beside unrelated work in progress there that
this session did not touch; the two halves land together.

### 2026-09-19 (nineteenth session) — 6.6's three remaining ports

Started from `main` at `1078aef` (the eighteenth session merged, plus the owner's commit of
`canopen-rs`'s vectors), on the owner's instruction to finish the plan, commit locally on `main`
and push nothing. The audit before any work: Phases 0–5 complete; Phase 6 at 6.1–6.4 done, 6.5 as
far as dry work goes, **6.6 one port of four**, 6.7 written and unrun until a robot. So the work is
the three ports — `canopen`, `ur_rtde`, `ethercat` — and then F70 if the ports leave room.

Baseline on a fresh mirror: `invariants` 54 of 54. Then the Windows half of CI, which no session
had run since `franka-rs` joined the workspace — and which does not link.

#### F72 — `main` had not linked on Windows since the first Rust driver, and clippy is why nobody saw it.

`franka-rs` declared `clock_gettime` unconditionally. Windows has no such symbol, so the
`windows` job's `cargo test --workspace` fails at the link of the driver's cdylib — every commit
since the seventeenth session. The eighteenth session's log says clippy was clean "on the Windows
target for the changed crates", which was true and is exactly the gap: clippy does not link. The
clock is Linux's now, with `skynet_rt::monotonic_raw_ns`'s process-local fallback elsewhere. On
this machine, after it: Windows `cargo test --workspace` 1,957 passed, 0 failed.

(The same run first failed for a different reason: `C:` had 20 KB free, and a linker out of disk
reads like a code error. Not a finding about the tree — noted so the next session reads a
LNK1140 as a disk before it reads it as a bug.)

#### `canopen-rs`, built

`frames` and `cia402` (the master's half, pure) are held by `tests/vectors.rs` to vectors the
Python modules generate — every request packed, every reply parsed, all sixteen statuswords,
every controlword, the unit conversions bit for bit. `bus` is SocketCAN over five declared
syscalls, Linux only. `bridge` is `bridge.py`'s bring-up, reads, commands and descriptor. The
descriptor's figures are read off the drives, so a template with holes (`franka-rs`'s approach)
cannot match Python's; `pyjson` reproduces `repr(float)` and `ensure_ascii`, held to Python's own
output across the places they part from a naive formatter (`3.0`, `1e-05`, `1e+16`, `5e-324`,
surrogate pairs). The stop is an atomic latch and frames on a descriptor duplicated at open, with
targets in atomics — no lock waited on.

`tests/live_vcan.rs` drives the real vtable against the Python driver's own simulated drives in
another process over `vcan0`, inside `vcan.sh`'s unprivileged namespace: descriptor byte-identical
with the Python bridge's, sixty commanded cycles with every read answered and both setpoints
reached, the refusals, the stop and its latch, close and reopen, a faulted drive, a drive with no
units, and an axis whose TPDO2 never arrives (`drives.py --lose-tpdo2`, added for it). **Held to a
control:** with the read reduced to one empty look after the SYNC — F71's shape — the first read
fails with `ETIMEDOUT`. (Two weaker mutations did not fail it, and the reason is worth keeping:
each removed only one of the two waits F71's fix has — the `poll` timeout and the loop until the
deadline — and either alone is still a wait.) `make check`: 16 unit + 9 vector + the live test,
then SCTS against the release `.so`, **26 passed, 0 failed**.

#### F73 — four paths the CANopen driver had never run, and the worst one disabled the stop.

Porting meant reading each refusal for what it would do on an input nobody had given it. Fixed in
the Python driver first, because it is the specification, each with a check that fails against
the parent:

* **A setpoint past RPDO2's 32 bits was stored before its frame failed to pack**, after which the
  quick-stop frame — which carries each axis's stored target — could not be packed either, and
  `emergency_stop` raised `struct.error` instead of stopping. The suite found this one by
  crashing: the check for the next bullet ran into it at the stop.
* A command refused on its second axis had already been sent to the first.
* A second `open` appended to the previous session's axes and kept its latch — a body of six
  joints that refused every command. SCTS opens once, so nothing had ever opened twice.
* A motor-side gear figure of 0 passed the unit check, which covered three of the four figures,
  and was divided by while the descriptor was built.
* Two joints could name one node: one drive, two setpoints a cycle.

After: selftest 79/0, `make check` SCTS 26/0, `make check-vcan` 87/0. The README's stop paragraph,
stale since F63, corrected with it.

**§73 maintenance.** Drivers 21,000 → 24,000 lines in both copies; `scale.json` regenerated; the
pin rewritten from the two agreeing copies. The Hub's copy is an uncommitted edit in its checkout,
as before.

**Verified** before the three commits: `cargo fmt --check`; clippy `-D warnings` clean on Linux
(workspace) and Windows (workspace, which now links); `cargo test --workspace` 2,058 passed / 0
failed on Linux (74 suites) and 1,957 / 0 on Windows; mirrors match their pin; `invariants` 54 of
54; the Python driver's three runs above. Committed on `main`: `36d209b` (F72), `852b666` (F73),
`990d5f9` (`canopen-rs`).

---
---

# Phase 6 — Host tier, and how wide it can go

Written 2026-09-18 on the owner's instruction: *Guest was achieved broadly; do the same for Host.*
Items 6.1 and 6.2 are built (fifteenth session); 6.3–6.7 are the plan and the arithmetic behind
them, in the form the sections above took before they were built — findings first, because four of
them change what the work is.

**Not ratified.** Three items here need a specification amendment, which `CONTRIBUTING.md` makes an
issue a maintainer carries to both copies rather than a pull request against the pinned document.
They are named as issues to file, not as edits to make.

---

## What Host coverage is allowed to mean under the same constraint

The opening section of this plan refused the strong reading of *universal* for Guest, and the refusal
has to be made again here because Host is where it gets expensive. §15's own arithmetic sets the
shape:

| Tier | Tests required | `sim` | `bench` |
|---|---|---|---|
| guest | 10 | 9 | **1** |
| host | 33 | 17 | **16** |
| certified | 36 | 17 | 19 |

Guest was broadly achievable dry because its bench half is one test. **Host's bench half is sixteen,
and every one of them is a human, a physical robot and a day.** No amount of work in this repository
converts one of them into a `sim` test, and *Decisions* 4 already refused the reclassification that
would pretend otherwise.

So Phase 6 does not promise Host on any robot, and a Phase 6 that claimed to would be the document
this project exists to be an alternative to. What it promises is narrower and is worth having:

**Every robot with a joint-level control interface reaches the state where the only thing between it
and a Host award is a bench session.** Sim-side blockers zero, machine-side blockers zero, drivers in
a language whose ceiling permits it, and a `blockers` list on every robot that names a day with a
robot and nothing else.

The distance between that and Host is one day per robot with a person present. The distance today is
unmeasured, which is F54.

---

## Findings before any code

### F54 — the ledger a Host programme needs already exists, in nine editions, and only the corpus runner calls it.

`crates/skynet-contracts/src/readiness.rs` computes exactly the thing this phase has to be measured
by. `evaluate_readiness` returns, per robot, the **highest tier it could attempt** and a `blockers`
list described in its own doc comment as *"the vendor's to-do list"*, over eleven checks:

```
adapter  kinematics  control_modes  control_rate  envelope  envelope_source
watchdog  joint_ratings  estop_software  estop_hardware  onboard_compute
```

`summarize_readiness` then returns `FleetReadiness` with `by_tier` and — the field that makes this a
programme rather than a spreadsheet — **`common_blockers`, a histogram of which blocker stops the
most robots.** That is the prioritisation function for the whole of Phase 6 and it is already written
and already mirrored into all eight other editions plus the TypeScript oracle.

**Nothing reads it.** The only caller in the tree is `skynet-corpus`'s `"readiness"` case
(`crates/skynet-corpus/src/lib.rs:1401`), which exists to reproduce corpus cases.
`skynet-cli` has forty-three subcommands and `readiness` is not one of them; there is no
`GET /readiness` on the robot and no fleet view anywhere.

This is the repository's four-times defect for the sixth time — a value that is correct, tested,
mirrored into nine languages, and reached by nothing anybody reads — and it is the most consequential
instance yet, because the missing surface is *the one that would tell anyone how far from Host the
fleet is.* Phase 6 is otherwise a plan with no denominator.

It also means item 6.1 below is cheap. The computation is done; what is missing is a command.

### F55 — four of the six drivers §66 designs for Host or Certified are written in a language §31.3 caps at Guest.

§31.3's ceilings are a property of the edition and *"an edition may not raise its own ceiling."*
Python and Go are Guest, because a GC cannot promise a clamp inside a deadline. Setting §66's design
ceiling beside what is actually in `drivers/`:

| Driver | §66 ceiling | As written | Ceiling as written |
|---|---|---|---|
| `ethercat` | **Certified** | Python | Guest |
| `franka` | **Host** | Python | Guest |
| `ur_rtde` | **Host** | Python | Guest |
| `canopen` | **Host** | Python | Guest |
| `ros2_control` | Host | C++ | Host |
| `unitree` | Host | C++ | Host |
| `lerobot` | Guest | Python | Guest — the ceiling is the design's |
| `mujoco` | Guest, simulated | Python | Guest — §35.3 forces it either way |

So on the four robots with the best real-time interfaces available — a Franka over FCI, a UR over
RTDE, CiA 402 drives over EtherCAT and over CANopen — **the thing standing between the robot and Host
is not the robot.** It is that the driver was written in the language that made the protocol work
provable dry, which was the right call for Phase 2 and is a dead end for this phase.

That is not a criticism of Phase 2. A Python driver against URSim is what found F27's three defects,
and a C++ one written first would have found them later and more expensively. The Python drivers
should be kept and kept running: they are the protocol's specification, executable, and the systems
reimplementation is measured against them. What changes is that they are no longer the end of the
road on those four robots.

**Two C++ drivers already have a ceiling that permits Host**, which makes them the honest place to
start: `ros2_control` and `unitree` need no rewrite, so whatever else Host needs shows up on them
first and uncontaminated by a port.

### F56 — §23.5 asks for four real-time properties. systemd grants three and the fourth is nobody's.

§23.5's list is *"RT core, `SCHED_FIFO` for the kernel thread, `mlockall(MCL_CURRENT|MCL_FUTURE)`, no
swap"*. Checking each against the tree:

| §23.5 asks | State |
|---|---|
| Isolated RT core | **Done.** `CPUAffinity` resolved by `install.sh --rt-cpu` into a drop-in, and `install.sh --check` asks systemd what it actually parsed — because a dropped `CPUAffinity=` starts the unit anyway |
| `SCHED_FIFO` on the loop | **Done.** `skynetk.service`: `CPUSchedulingPolicy=fifo`, priority 80, and the comment says why not 99 |
| `mlockall(MCL_CURRENT\|MCL_FUTURE)` | **Absent.** `skynetk.service` sets `LimitMEMLOCK=infinity`, which raises `RLIMIT_MEMLOCK` and locks nothing. `mlockall` appears in none of the 206 `.rs` files in `crates/` |
| No swap | **Unchecked.** `install.sh` has no reference to swap at all |

The third row is the one to keep. **The permission is granted and the call was never made**, and the
unit file's own comment states the property it believes it is providing: *"The loop must not be
swapped out mid-cycle, and a page fault on the command path is a missed deadline that no amount of
headroom explains afterwards."* That sentence is true about the consequence and wrong about the
tree — which is the shape §80 keeps warning about, a fact with two records that disagree exactly once,
after the failure that matters.

It is also the cheapest item in this phase: two syscalls, no robot, no amendment, and a
`/proc/self/status` reading of `VmLck` as its own positive control.

### F57 — `control.realtime` is a token a body may declare and nothing on the machine can contradict it.

`control.realtime` is one of §85's thirty-eight capability tokens (`admission.rs:326`), so a package
may require it and a body may declare it. `embodiment.capabilities.declared` — `sim`, required from
host — asks that every declared token be *"one the driver can actually service"*.

**Nothing in the tree reads whether the running kernel is PREEMPT_RT.** No file matches
`/sys/kernel/realtime`, `CONFIG_PREEMPT` or `cyclictest`. `install.sh`'s host check tests for Linux,
`systemctl` and systemd as PID 1, and its failure message *mentions* §23.5 asking for PREEMPT_RT
without checking for it.

So a robot on a stock kernel can declare `control.realtime`, pass the sim test that is supposed to
police the declaration, and present as Host-ready. This is §61's "vendors do not publish these"
arriving from the inside: the platform's own machine cannot contradict its own declaration.

The fix is not another advisory. It is that the tier the loop *runs* at is derived from what the
machine is, the same way §31.3 derives an edition's ceiling and §35.3 derives the simulator cap —
`--tier host` on a non-RT kernel should be a refusal at start, not a badge. That is a specification
question, because §1's table says what Host *runs on* and nothing says who checks.

### F58 — `envelope_source` is dry work, and it is probably `common_blockers`' tallest bar.

Two of readiness' eleven checks are `envelope` and `envelope_source`, and two of Host's sim tests are
`embodiment.limits.sourced` (*"a rating with no provenance fails, because the safety kernel would be
enforcing a rumour"*) and `embodiment.descriptor.signed`. §59.3 says most robots have no sourced
envelope and calls it the reason they land at Guest.

The field exists: `limits.source: Option<String>` in `embodiment.rs:190`. CI already enforces that
`skynet-urdf` cannot fill it, citing §63.4 — a URDF effort limit is a CAD design value.

**But transcribing a published datasheet into a descriptor with a citation needs no robot.** UR,
Franka and Unitree publish joint torque and velocity ratings; CiA 402 drives publish theirs on the
drive datasheet. That is a per-robot afternoon of reading, it is verifiable by a second person against
the same document, and it converts the single most common Host blocker into a merged file.

What it does **not** do is make the number true, and this is the place to say so before anybody
enjoys the progress. `embodiment.limits.honest` — measured joint velocity and torque under load — is
`bench` and required from `certified`, and it exists precisely because a datasheet is a marketing
document until somebody checks it. A sourced envelope is a *citation*, not a measurement. §5's own
rule stands: the kernel enforcing an unverified number is the kernel enforcing a rumour, and a
citation only tells you whose rumour it is.

So this item's honest form is: fill `source` from published documents, and make the descriptor say
`declared` versus `measured` so that nothing downstream can read the first as the second.

---

## Phase 6 — the items

Ordered by what F54's histogram would rank if it had a surface, which is why 6.1 is first.

| | Item | Needs | Blocked on |
|---|---|---|---|
| 6.1 | **A surface for `evaluate_readiness`** — `skynet readiness [--fleet]`, and `GET /readiness` on the robot beside `GET /conformance` | nothing | nothing. F54. **Done**, 2026-09-18 (fifteenth session) — the fleet runs over eight of ten bodies, and F59 is what it found |
| 6.2 | **`mlockall(MCL_CURRENT\|MCL_FUTURE)` in `skynetk`**, plus a swap reading in `install.sh --check` | nothing | nothing. F56. **Done as reporting**, 2026-09-18 (fifteenth session) — the lock is taken only when the limit is unlimited (F61), and the refusal half waits on the `who-checks` amendment with the rest of 6.4. **Refusal built**, 2026-09-18, once the amendment was ratified — see 6.4 |
| 6.3 | **Sourced envelopes for the six real-interface bodies**, and a `declared`/`measured` marker beside `source` | published datasheets | the marker is a descriptor field — **amendment**. F58. **Done as transcription**, 2026-09-18 (sixteenth session) — three envelopes cited, three nulls reasoned, six stops declared (F63), fleet none 3 / guest 7; the marker still waits on its amendment, and F58 is live until it lands. **Marker built**, 2026-09-18: ratified as §5.3, `envelope.basis` defaulting to `declared`, and every transcribed envelope states it |
| 6.4 | **A PREEMPT_RT gate on `--tier host`** — derived, refused at start, reported at check-in | a PREEMPT_RT VM as the harness | who checks is undecided — **amendment**. F57. **Done**, 2026-09-19 (eighteenth session) — ratified as proposed (the loop refuses; installer and check-in report), built and fixture-tested by the carry, and held to its control on a real-time kernel by `packaging/rt-vm.sh`: Ubuntu's 7.0.0-31-realtime starts `--tier host` with VmLck 3,764 kB, its -generic sibling refuses naming `/sys/kernel/realtime` |
| 6.5 | **`ros2_control` and `unitree` to Host-ready** — the two whose language already permits it, on a real-time kernel, with 6.1's blockers driven to bench-only | 6.1–6.4, and a PREEMPT_RT VM | nothing after those. **Half-driven**, 2026-09-18 (sixteenth session) — `unitree` declares its onboard compute and stands one vendor figure from host-attainable (F64); `ros2_control` needs its ROS machine. **As far as dry work goes**, 2026-09-19 (eighteenth session) — `ros2_control` has its ROS machine (RoboStack Jazzy, user space) and runs 51/0 and SCTS 26/0 here; its five host blockers are the integration's declarations, not the driver's (F69), and `unitree`'s one is still F64's vendor figure |
| 6.6 | **Systems reimplementation of the four Host- and Certified-ceiling drivers** — `ur_rtde`, `franka`, `canopen`, `ethercat`, in Rust or C++, measured against the Python driver that is now the protocol's executable specification | 6.5 first, so the Host machinery is proven before four ports depend on it | nothing. F55. **One of four ported**, 2026-09-18 (seventeenth session) — `franka` in Rust, held to `layout.json`, eleven golden wire vectors, the live Python fake and SCTS at 26/0; begun before 6.5 completes because the port touches neither the RT kernel nor the ROS machine (see log); the other three sized (~1,900 lines) and sequenced by measurability. **Substrates made live**, 2026-09-19 (eighteenth session) — `canopen`'s on a kernel CAN bus (which found F71 in the Python driver the port is measured against, fixed first), URSim pulled for `ur_rtde`'s; `canopen-rs` begun with its golden vectors, and the three ports themselves still to write. **Two of four**, 2026-09-19 (nineteenth session) — `canopen-rs`, held to vectors from the Python modules and to the Python drives over `vcan0` (descriptor byte-identical, SCTS 26/0); porting it found F73 in the specification, fixed there first |
| 6.7 | **The bench runbook** — the sixteen bench tests as a procedure, per body, with the instrumentation each one needs named | 6.1–6.6 | **a robot.** This is where the phase stops. **Written unrun as `docs/bench-runbook.md`**, 2026-09-18 (seventeenth session) — sixteen procedures with preconditions, pass rules, per-body notes and named instruments, plus a shopping list of undeclared figures and three stale READMEs corrected; verifying the gate's point 4 against it found the sentence unreachable (F66: the true end state is `guest` with fifteen `missing`, zero `failures`) |

### The harness this phase needs, and why it is the same argument as F2

Phases 0–5 ran on a fake, a simulator and a vendor's own controller software. Phase 6 needs one thing
none of them provide: **a real-time kernel.** `packaging/virtual-robot.sh` already produces *"a real
machine — real Linux, real systemd, a real kernel turning a loop"*, and F2's distinction is exactly
the one that applies here — a virtual robot is *a machine rather than a model*, without being
hardware.

A PREEMPT_RT kernel in a VM is therefore the harness of record for 6.2, 6.4 and 6.5, and it is
honest about what it is: it can show `mlockall` succeeded, that a non-RT kernel is refused, that the
loop holds `SCHED_FIFO` on an isolated core, and that jitter under a synthetic load is *bounded on
that machine*. It cannot show a latency figure that means anything about a robot, and no number taken
from it may be recorded as bench evidence — `skynet bench record` binds a record to the digest of the
artefacts it was taken against (§12's `BenchPassStale` in general form) and this is the boundary that
mechanism exists to police.

### Amendments to file first

Neither is written here, for the reason F29 met and *Decisions* 13 resolved: proposing the design in
this file is the half-designing §43 declined to do, and the pinned document cannot be edited from one
copy.

1. **`docs/issues/declared-versus-measured-limits.md`** — §5, §44, §63.4. A limit carries provenance
   today (`source`) and no *kind*. F58's whole risk is that a cited design value reads downstream
   exactly like a measured rating. The question the issue has to settle is whether the kind belongs
   on the limit, on the envelope, or on the descriptor, and what a body that declares `measured`
   without a bench record bound to its digest is refused for. **Filed**, 2026-09-18 (fifteenth
   session), **proposed** — recommending the kind on the envelope, weakest wins, defaulting to
   `declared`. **Ratified as §5.3** and built, 2026-09-18, on the owner's instruction to complete
   Phase 6 — the kind on the envelope as recommended, disambiguated from §87's safe-state `basis`.
2. **`docs/issues/who-checks-the-real-time-kernel.md`** — §1, §23.5, §31.3. §1's table says Host
   *runs on* PREEMPT_RT and no section says who verifies it. The issue has to settle whether the
   check belongs to `install.sh` (refuse the installation), to `skynetk` (refuse the tier at start),
   or to the check-in (report it and let a Hub refuse the award) — and F57's argument is that a
   derived ceiling is the precedent, so it is `skynetk`'s. **Filed**, 2026-09-18 (fifteenth
   session), **proposed** — recommending `skynetk`'s refusal at start over the whole §23.5
   precondition set, with `install.sh` and the check-in reporting. **Ratified as proposed** and
   built, 2026-09-18, with the `SKYNETK_UNCHECKED_TIER=1` harness override the existing
   `--tier host` tests needed; held to its control on a real-time kernel on 2026-09-19.

### The two frictions every item here will hit

**§73 states the corpus size and the tree's Rust line count exactly**, and `scale-report.mjs --check`
holds the document to a measurement, refusing with *"correct the document, not this check."* F6
recorded two lines of headroom in the Rust figure and F8 established that **every corpus case anybody
adds, ever, requires an edit to `docs/skynet-os-architecture.md`.** Items 6.1 through 6.6 all add
Rust and most add corpus cases, so each is a figure edit in both copies plus a re-pin. That is
maintenance under *Decisions* 1 and 10 rather than an amendment, and it is not optional.

**Nine editions, not one.** `readiness` is mirrored into all eight other editions and the TypeScript
oracle; so is `admission`. A field added for 6.3 is the oracle, then the corpus cases, then the four
complete editions, then the four partials at raised ratchets — the order Phase 3 paid to learn
(*"write the corpus cases before the editions"*).

---

## What Phase 6 cannot do, and must keep saying

The same discipline as *What stays open*. Each of these is a surface that has to keep refusing.

* **The sixteen bench tests.** `missing`, never `skipped`, never absent. Phase 6 ends with them still
  missing on every robot, and that is the phase working rather than failing.
* **Host on a program-level controller.** Fanuc, KUKA, ABB, older UR controllers, Spot SDK. §59.2 and
  §61: *"not fixable in software."* Nothing in this phase touches that row of §59.7's table, and
  §88's `execute` path is what those machines get instead. Guest is their ceiling and it is not a
  deficiency.
* **`proximity` is still inert.** A Host-tier robot under this phase does not enforce distance to a
  person, because perception is a package and nobody has shipped one. Worth stating twice as loudly
  once robots start reaching Host, because "Host" will be read as "safe around people" by somebody.
* **`package.signature.required` still fails**, by the deliberate position §15.1 spends a paragraph
  on. It is a guest-tier test, so it caps every award below guest regardless of anything in this
  phase, and nothing here closes it.
* **Certified is out of scope.** It needs an independent safety MCU, and the only driver designed for
  it is `ethercat`, whose two justifying properties — distributed clocks and STO over FSoE — are the
  two this repository has never verified and cannot. 6.6 puts that driver in a language that permits
  the ceiling; it does not approach the ceiling.
* **A measured envelope.** 6.3 delivers citations. `embodiment.limits.honest` is bench and
  certified-required, and until it runs the kernel is enforcing whichever number the vendor printed.

---

## The gate

Phase 6 is met when, on a PREEMPT_RT virtual robot:

1. `skynet readiness --fleet` runs over every body in `drivers/` and prints `by_tier` and
   `common_blockers` — so the phase has a denominator it did not have when it was written.
2. For `ros2_control` and `unitree`, every blocker in that report is a **`bench` test**. No sim
   failure, no absent declaration, no language ceiling, no unverified kernel.
3. `skynetk --tier host` **refuses to start** on a kernel that is not PREEMPT_RT, and the refusal
   names what it read. Held to a control: the same binary on the RT kernel starts and reports
   `VmLck` non-zero.
4. `skynet bench award --claim host` on the dry evidence — every sim test and the one bench test a
   machine can witness, `offline.control.independent` — returns **`guest` with fifteen `missing` and
   zero `failures`**, the fifteen being exactly host's own bench tests. Pinned as the corpus case
   `everything-dry-claiming-host-reads-fifteen-bench-tests`.

   *Corrected 2026-09-19 on the owner's ruling (F66, Decisions 15).* It read "`none` with sixteen
   `missing`", which the award cannot say: the walk stops at the first tier it cannot satisfy, so
   host's bench tests are listed only once guest's one is in evidence — and then the award is
   guest. The intent was always this sentence; the arithmetic was not.

Point 4 is the phase's whole result, and it is deliberately unglamorous. The award does not move past
guest. What moves is *why* it does not move: from "nine unmeasured sim tests, a language ceiling and
an unchecked kernel" to "fifteen tests that need a robot." The first is this repository's to fix and
now is. The second never was.

# Dry coverage plan — 2026-09-16 onward

Coverage work carried out **without a physical robot**, and the mechanisms that keep the
untested half visible rather than implied. Kept beside [`BUILD-LOG.md`](BUILD-LOG.md), which is
the session record; this file is the plan and its progress log.

> **The constraint that shapes everything below.** No step here is verified against hardware.
> That is a decision the owner made, not an omission, and the plan's first job is to make it a
> property of the build rather than a sentence in a README.

**Nothing here is pushed.** Commits are held on `main` locally until asked for.

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

---

## Phase 2 — drivers, and how far each goes dry

| Driver | Dry substrate | Dry-verifiable | Not dry-verifiable |
|---|---|---|---|
| `ros2_control` bridge | Fake `hardware_interface` | Adapter, resource-manager mapping, descriptor generation | Jitter under load |
| **CANopen / CiA402** | ~~Linux `vcan`~~ **an in-process bus** + CiA402 slave sim | **Built** — `drivers/canopen`. 61 checks in its own suite, 24 of 24 through the real ABI, guest tier | Bus bandwidth at real joint × rate — and now also *everything `vcan` would have added*: see below |
| EtherCAT / SOEM | Software slave stack | CiA402 state machine, frame encoding | **Distributed clocks and FSoE STO — the whole basis of its Certified ceiling** |
| UR RTDE | URSim (UR's own controller, containerised) | Almost everything, against real vendor software | Servo timing |
| Unitree SDK2 | SDK sim | Protocol, low-level mode mapping | Joint access at rate |
| `libfranka` / FCI | Protocol fake from public docs | Encoding, state machine | 1 kHz torque, which is its entire claim |

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

---

## Phase 3 — motion-primitive action space

Highest coverage-per-effort item, and the most dry-verifiable, because it is contract work and the
corpus plus eight editions plus the TypeScript oracle is exactly that machinery.

**Requires a specification change** (a descriptor field and new transport methods), so it starts as
an issue, not a branch. *Decisions* 6 deferred it behind Phase 1's gate; **that gate is now met**, so
the deferral has expired and this is unblocked.

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

1. Descriptor: action-space kind, named motions with parameters
2. Transport bindings over motions rather than joint channels
3. Corpus cases, then the editions
4. ~~A mock program-level controller driver — the teach-pendant analogue of `drivers/mock`~~
   **Done** — `drivers/pendant`. Thirty-seven checks, nearly all of them refusals, because for that
   class of machine the refusals *are* the interface. It found F25 on its first conformance run.

Ends fully dry-verified, because there was never a clamping claim to verify.

---

## Phase 4 — skills coverage

1. Route `execute({ skill, arguments })` to a command-level driver. §59.2 makes this the only path
   that applies to the largest installed base.
2. Widen the live precondition context per driver. Every unresolvable path is a body the skill
   cannot reach, so this is directly measurable: how many bodies a fixed skill set admits on,
   before and after.
3. Capability token vocabulary and `extensions`. Pure contract, pure corpus.

`skynet skill check` stays the workstation gate it already is.

---

## Phase 5 — the class-gating open items

Each unlocks a robot class rather than a feature. All contract-shaped, all dry-verifiable.

| Item | Gates | §ers |
|---|---|---|
| Multiple robots on one compute | Cells — one Jetson, two arms | §43 |
| Safe-state discovery for mobile robots | AMRs at Host; §36.2's reserve budget depends on it | §36.2, §43 |
| `on_command_loss` per chain | Humanoids with mixed drivetrains | §53 |
| Hub-brokered console relay | A robot in a customer's building with nobody on site | §43 |

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

## Next

Everything below is decided and unblocked. In order.

1. **The small ones, together** — *Decisions* 3, 7, 8 and 9. Delete `check_code` and
   `tier_profile_to_json`; name §19's activation rule once on `SignatureState` and drop the
   accessor; make `skynet blackbox` say whether the window it replayed was complete; render "no
   evidence" apart from "evidence, and it failed". Four small changes, one concern each, all inside
   the reference edition.

2. **`notifies_operator`** (*Decisions* 2). Corpus first, as Phase 3 argues and §47.6 proved: the
   TypeScript oracle, then the 28 power cases, then the four complete editions, then the CLI.

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

Phases 2 and 5 are unchanged and unstarted. **Phase 3's blocker is cleared** — *Decisions* 6 deferred
its specification fields behind Phase 1's gate, and the gate is met — so motion primitives are now the
next thing that is only waiting for somebody to open the issue.

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

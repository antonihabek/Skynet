# Proposed amendment — a limit carries provenance but no kind, and a citation reads as a measurement

**Status: ratified and carried 2026-09-18 as §5.3**, on the owner's instruction to complete
Phase 6. Written 2026-09-18 from `DRY-COVERAGE-PLAN.md` F58, by Phase 6's first session; carried
with two relocations the carrying found necessary (`DRY-COVERAGE-PLAN.md` F67): the §44 half
moved into §5.3 itself, because §44 holds coordinate frames and no envelope field list — and
`envelope.basis` is disambiguated there from §87's safe-state `basis`, the second meaning the
same word already carries in this document.

## The gap

`SafetyEnvelope.source` says *where* a figure came from. Nothing says *what kind of thing* it
is. A joint torque ceiling transcribed from a vendor's PDF and a joint torque ceiling measured
under load arrive downstream as the same value in the same field:

* the Safety Kernel enforces both identically (§5);
* `embodiment.limits.sourced` passes on either — it asks for a citation, and a datasheet is one;
* readiness' `envelope_source` passes on either, for the same reason.

F58's whole risk is that sentence: Phase 6 item 6.3 transcribes published ratings into
descriptors, which is real progress against the most common Host blocker, and every file it
touches becomes able to present a design value exactly as a measured rating presents itself.
§5's own rule stands over all of it — *the kernel enforcing an unverified number is the kernel
enforcing a rumour* — and a citation only tells you whose rumour it is.

The measurement exists, one tier up: `embodiment.limits.honest` — measured joint velocity and
torque under load — is `bench` and required from `certified`, and it exists precisely because a
datasheet is a marketing document until somebody checks it. What is missing is the vocabulary
that keeps the two apart below that tier.

## The question

Whether the kind belongs on the limit, on the envelope, or on the descriptor — and what a body
that declares `measured` without a bench record bound to its digest is refused for.

Three placements, narrowest first:

1. **Per limit.** Every figure in the envelope carries its own kind. Most precise: a body whose
   velocity rating was measured and whose torque rating was transcribed says exactly that. Also
   the heaviest: every figure gains a field, and every edition, the oracle and the corpus gain
   the rules for it.
2. **Per envelope.** One kind for the set, weakest wins: a mixed envelope declares `declared`.
   One field, one rule, and the conservative direction is the default — a vendor who measured
   half their envelope and wants the word `measured` measures the other half.
3. **Per descriptor.** Coarsest: the whole body is declared or measured. Simple to the point of
   being wrong the day a body mixes a measured arm with a transcribed base.

The recommendation is (2): a `basis` beside `source` on the envelope, `declared` or `measured`,
defaulting to `declared` when absent — because every descriptor written before this section is a
declaration, and a default of `measured` would award the stronger word to every file that never
heard of it.

## Proposed text

In §5, beside the rule the gap quotes: *a limit carries its basis with its provenance. `declared`
is a figure transcribed from documentation; `measured` is a figure taken on the body. The kernel
enforces both — it has nothing else to enforce — and every surface that renders a limit renders
the basis with it, so that nothing downstream can read the first as the second.*

In §44, on the envelope: *`basis` is `declared` unless stated otherwise. A body that declares
`measured` without a bench record for `embodiment.limits.honest` bound to its descriptor digest
is refused under `embodiment.descriptor.valid`, for claiming a measurement nothing recorded.*

§63.4 needs no new rule — it already refuses to populate the envelope from URDF values — but its
provenance block gains the sentence that this section exists to make sayable: *had it populated
them, their basis would have been `declared`.*

## What this unblocks, in order

Phase 6 item 6.3: transcribe the published ratings (which needs no amendment — `source` exists),
then mark every one of them `declared`, then the oracle, the corpus cases, and the editions in
the order Phase 3 paid to learn. `skynet readiness` renders the basis beside the envelope it
evaluates, so the fleet view cannot show a transcribed figure as a measured one.

## Why this is an issue and not a patch

`CONTRIBUTING.md`: *"If you think the specification is wrong, open an issue rather than a pull
request against it — a maintainer carries the change to both copies."* A descriptor field is a
design change, not §73's figure maintenance (`DRY-COVERAGE-PLAN.md` *Decisions* 1 and 10), and
*Decisions* 13 and 14 each ratified a named amendment rather than a class of them. So it waits
for the owner.

## Amends

§5 (the rule), §44 (the envelope field and the refusal), §63.4 (one sentence in the provenance
block). Corpus cases, then the editions.

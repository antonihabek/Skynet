# Proposed amendment — §59.5 says the Safety Kernel, `skynetd`, the drivers, OTA, the console and the corpus are zero lines, and §73 counts them

**Status: ratified 2026-09-21 and carried to both copies of the specification.** Kept as the
proposal it was, for the reason `DRY-COVERAGE-PLAN.md` keeps its superseded findings; the
specification is the text that holds. One departure: the proposed "sixteen drivers" counted the
tree at filing and was stale by carrying (twenty directories), so the carried text says "the
drivers" — which is this file's own argument about figures nothing checks, arriving before the
ink dried. Recorded in the plan's twenty-fourth session entry.

## The gap

§59.5, in full:

> ### 59.5 None of it is built
>
> This is a design with its contracts implemented: roughly 2,430 lines of TypeScript covering
> transport, admission, certification and readiness, plus the RHAL headers. The Safety Kernel,
> `skynetd`, the policy runtime, every driver, OTA, the console and the conformance corpus are
> **zero lines**. The realistic figure is several engineer-years before a robot moves under this.

§73, in the same document, measured by `tools/scale-report.mjs` and enforced by its `--check`:

> **Built** means running, tested code: **159,000 lines of Rust** across **twenty-seven crates**,
> held by a corpus of 93 files and 1,448 cases to **17,000 lines** of the TypeScript reference
> edition and **18,000 lines** of the Python edition, with **29,000 lines** of drivers and
> **9,000 lines** in the Hub's mirror of the shared contracts.

Every component §59.5 names as zero lines is in that count. The Safety Kernel is `skynet-kernel`;
`skynetd` is `skynetd` and `skynetd-io`; the policy runtime is `skynet-policy-host`; the drivers
are sixteen directories under `drivers/`; OTA is in the daemon with its RAUC slots; the console is
built and §32 describes its four panels; and the conformance corpus is the 93 files and 1,448 cases
§73 counts. §61's last table row carries the same claim in four words — *"Several engineer-years,
unstarted"*.

This is §80's shape — one fact with two records that disagree — and it is in the part of the
document whose entire job is to be trusted about limits. A reader who checks §73 against §59.5 has
found the document contradicting itself on the most basic question there is, in the section that
was written to establish that it does not flatter itself.

## Why it stayed wrong

§73's figures are held to the tree by `scale-report.mjs --check`, which fails the `invariants` job
when the paragraph and the measurement disagree, and which was itself built because that paragraph
had been wrong three times (§73's own note). **§59.5 has no such mechanism.** Nothing measures it,
nothing fails, and it has been false for as long as there has been a kernel.

## What is still true, and is the part worth keeping

The section's *conclusion* survives its facts, and that is the reason to amend it rather than delete
it: **no robot has moved under this.** Every driver in the tree is measured against a fake, a
simulator or the vendor's own software; every bench test in §15 is `missing` on every body; and
`DRY-COVERAGE-PLAN.md`'s whole premise is that none of it is verified against hardware. The
sentence that should be load-bearing here is that one, not a line count.

## Proposed text — §59.5

Retitle to **"It is built, and no robot has run it"**, and replace the body with:

> The contracts, the Safety Kernel, `skynetd`, the policy runtime, sixteen drivers, OTA, the
> console and the conformance corpus are written and tested — §73 counts them, and a check holds
> that paragraph to the tree. This section said for many rounds that they were zero lines, which
> was true when it was written and has not been for a long time.
>
> What has not happened is the thing this section was really about. **No robot has run any of it.**
> Every driver is exercised against a fake, a simulator, or the vendor's own controller software;
> every `bench` test in §15 is `missing` on every body, because each one needs a physical machine
> and a person; and nothing here has commanded an actuator that was attached to anything. The
> distance from where this is to a robot moving under it is a commissioning project per robot
> (§59.6) and a bench day per tier claim (§15) — not several engineer-years of writing the OS, and
> not zero either.
>
> A reader deciding whether to trust this document should weigh the second paragraph, not the
> first. Code that exists and has never met hardware is a stronger position than a design
> document and a much weaker one than a fleet.

## Proposed text — §61's last row

| Requirement | Today |
|---|---|
| The OS itself | Written and tested (§73), and never run on a robot. What remains per robot is commissioning (§59.6) and §15's bench half |

## Amends

§59.5 in full, §61's last table row. No contract change, no corpus case, no edition work. §73's
figures are maintenance and are already correct.

## A second question the owner may want to settle with it

§59.5 was wrong for months because nothing checked it, and the fix above leaves it exactly as
uncheckable. The cheapest mechanism that would hold it is that **`scale-report.mjs --check` already
knows the components exist** — it counts the crates. A check that failed when Part VII asserted zero
lines of something §73 counts is a few lines in a tool that already runs in `invariants`.

That is a separate change from this amendment and it is not proposed here, because it is a tool
rather than a specification, and because the honest version of it is narrow: it can catch *"zero
lines"* against a non-zero count, and it cannot catch a stale argument. Filed as a note rather than
a recommendation.

## Why this is an issue and not a patch

`CONTRIBUTING.md`: *"If you think the specification is wrong, open an issue rather than a pull
request against it — a maintainer carries the change to both copies."* A figure that a check
enforces is maintenance (`DRY-COVERAGE-PLAN.md` *Decisions* 1 and 10); this rewrites a subsection's
argument and retitles it, so it waits for the owner.

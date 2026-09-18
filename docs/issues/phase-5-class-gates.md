# Proposed amendments — §43's four open items, which gate four robot classes

**Status: ratified 2026-09-17 as §87 of the specification, and built.** Kept as the questions it was,
for the reason `DRY-COVERAGE-PLAN.md` keeps its superseded findings. *Decisions* 13 says ratifying
this file means **answering** them, each answer recorded with the alternative it rejected, and §87 is
those four answers. Where the built form departs from what is asked below:

* The **second** question here — a declared worst case or a live figure — is answered *declared*,
  and §87.2 says why the accurate option loses: it puts a planner inside §36's reserve arithmetic and
  makes the navigation stack a safety-relevant component with no tier. `live` is refused **by name**
  rather than reported as an unrecognised word, because a vendor who writes it has proposed the
  design that subsection rejected.
* The **third** proposes the migration as a `schema_version` bump, on the grounds that refusing a
  missing chain *"makes every existing descriptor invalid"*. That is false in the direction that
  matters and §87.3 records it: a descriptor written before the section declares no map and is read
  by §46.2's body-wide rules exactly as it was, and one declaring neither field was **already**
  refused host by *"silence is not `hold`"*. There is no schema version.
* The **first** asks how two robots on one compute share a cell, and §87.1 answers that they do not:
  two kernels cannot see each other's envelope, so the platform **refuses the disjointness claim**
  rather than inventing a cell envelope nobody evaluates.
* The **fourth**'s three decisions are answered in §87.4, and its own last sentence — *"that last one
  is the substance"* — was right: a relayed session declares a lease, the robot enforces it locally,
  and a session with no declared deadline is refused at open.

`DRY-COVERAGE-PLAN.md`'s Phase 5 lists four items and calls them *"all contract-shaped, all
dry-verifiable"*. The second half is true and the first half is the problem: each of them is
contract-shaped **once it has been designed**, and §43 is the section that says none of them has
been. Its own words, about the first:

> Deliberately deferred rather than half-designed.

§58.3 lists three of the four again under *What is still not designed*. So this is not work waiting
for a robot; it is work waiting for a decision, which is `DRY-COVERAGE-PLAN.md` F29.

Each entry below is what the amendment would have to settle, written as questions rather than as a
design — because proposing one here would be the *half-designing* §43 declined to do. What is worth
recording is that each of the four is a small number of decisions rather than an open field.

## Multiple robots on one compute — §43, and Phase 5's first row

Gates: cells. One Jetson, two arms.

The machinery is partly built: `skynet_contracts::cell` and `skynetd_io::cell` exist, §55.2's cell
descriptor is signed and reaches the `workspace` guard and `env.*`. What is undesigned is the
*runtime* shape, and §43 names the four questions exactly — two RHAL drivers, two kernels or one,
two black boxes, and a shared accelerator.

Two of them look answerable from what is already here and one does not:

* **Two black boxes.** §26.2 already gives the supervisor and the control loop separate rings for a
  reason that applies unchanged per robot. Two rings per robot is the shape the existing argument
  produces.
* **Two RHAL drivers.** `skynetk` holds one driver and one pool; two processes is the shape
  §33.1 already forces, and the pool hand-off is per socket.
* **Two kernels or one** is the real decision, and it is a safety argument rather than an
  engineering one: one kernel evaluating two bodies is one process whose failure stops both, and two
  kernels sharing a machine cannot see each other's envelope. §7's guards are per body; whether a
  *cell* has an envelope is the question §43 is deferring.

## Safe-state discovery for mobile robots — §36.2, §43

Gates: AMRs at Host tier, because §36.2's reserve budget is computed from the time to reach a safe
state and a mobile robot's is a navigation problem.

The decision is narrower than the phrasing suggests, and it is this: **may a robot declare a safe
state it cannot reach?** §5.2's rule says a figure with no provenance is a rumour, and
`time_to_safe_state_s` on an AMR is a number that depends on where it is standing. The honest
options are a *declared worst case over the mapped area* — which is a measurement somebody takes
once and which §15's `power.safe_state.reachable` could then check — or a *live* figure the
navigation stack supplies per cycle, which puts a planner inside §36's reserve arithmetic.

## `on_command_loss` per chain — §53

Gates: humanoids with mixed drivetrains, where the right answer for a leg and for an arm differ.

`EmbodimentDescriptor.on_command_loss` is per body today. Per chain means the field becomes a map
keyed by §5.1's chain vocabulary — which already exists, which is what makes this the smallest of
the four — with a body-wide default for a robot that has one behaviour.

The decision is what a **missing** chain means. Inheriting the body-wide value is the convenient
answer and it makes a partially-declared humanoid look fully declared; refusing is the safe one and
it makes every existing descriptor invalid. §5.2's direction is the second, and the migration is a
schema version rather than a default.

## Hub-brokered console relay — §39.2, §43

Gates: a robot in a customer's building with nobody on site.

§39.2 forbids the console over the internet, which §43 calls *"right for security and wrong"* for
exactly that robot. The shape §43 already suggests is a relay the robot **dials out** to, and the
decisions are about what the Hub may then do: whether a relayed session is a distinct role in §5.1's
four rather than the same operator arriving by another route, whether an estop over a relay is
honest when the latency is unbounded, and whether a session that cannot be ended by pulling a cable
may hold a lease at all.

That last one is the substance. Every other console session ends when somebody unplugs something.

## What ratifying any of them costs

The same order, every time, and §47.6 is what taught it: **the corpus cases before the editions.**
The oracle, then the cases, then Rust, Python, C++ and Go, then whatever surface renders it. Every
case added moves §73's corpus count, which `scale-report --check` holds to the tree.

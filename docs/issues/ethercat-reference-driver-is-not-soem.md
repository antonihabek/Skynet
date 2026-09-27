# Proposed amendment — §4 names SOEM as the EtherCAT reference driver's stack, and it cannot be

**Status: ratified and carried 2026-09-19**, on the owner's instruction to finish every section of
`DRY-COVERAGE-PLAN.md`, together with `stop-latch-lifetime.md`, which amends the same paragraph.
Written 2026-09-18 by the plan's fourteenth session, from F48, which established the fact in the
thirteenth and did not carry it to the specification. Carried as proposed, and one step further:
§62.1's driver table said **EtherCAT (SOEM)** as well, the same false parenthetical in a second
place, and it now names the master as the driver's own and points at §4.

## The gap

§4 ends:

> **Reference drivers ship with the OS** for ROS 2 (`ros2_control` resource manager), plain
> `ros2_control`, EtherCAT (SOEM), CANopen, MuJoCo and Isaac Sim …

`drivers/ethercat` is that reference driver, and it does not use SOEM. It cannot: **SOEM 2.x is
dual-licensed GPLv3 or commercial**, so an Apache-2.0 driver that linked it could not be distributed
under this repository's licence, and no arrangement of the code changes that (F48). The master the
driver ships is this repository's own, written against ETG.1000.

So the sentence tells a vendor something false about what they would be building on, and it is the
kind of false that costs somebody a licence review: a reader who takes the parenthetical at its word
links SOEM into a driver and ships it.

## What SOEM is to this repository now

Not the stack, and not nothing. `make check-soem` runs SOEM v2.0.0 **as a separate program** against
the driver's slave stack over a veth pair — interoperation, which the licence does not restrict — and
it is the check that found F51 to F53. That role is worth naming in the specification, because it is
the only evidence this row has that is not this repository agreeing with itself.

## Proposed text

> **Reference drivers ship with the OS** for ROS 2 (`ros2_control` resource manager), plain
> `ros2_control`, EtherCAT (its own master — SOEM's licence rules out linking it, and SOEM is run
> against the driver's slaves as an independent check instead), CANopen, MuJoCo and Isaac Sim …

## Why this is an issue and not a patch

`CONTRIBUTING.md`: *"If you think the specification is wrong, open an issue rather than a pull request
against it — a maintainer carries the change to both copies."* §73's figures are maintenance
(`DRY-COVERAGE-PLAN.md` *Decisions* 1 and 10) and were edited this session in both copies; this is a
sentence about design, and *Decisions* 13 and 14 each ratified a named amendment rather than a class
of them. So it waits for the owner.

## Amends

§4's reference-driver sentence. No figure, no contract, no corpus case.

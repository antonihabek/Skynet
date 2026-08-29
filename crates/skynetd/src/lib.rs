//! `skynetd` — the supervisor's brain.
//!
//! > **`skynetd` is not safety-critical, and it is built that way on purpose.** (§33.1)
//!
//! It never touches an actuator. It supervises the policy runtime, which posts desired commands
//! into a ring the Safety Kernel reads, and the kernel is what writes. So `skynetd` may crash,
//! hang, leak, be killed by the OOM reaper, or be stopped by an operator, and the robot does not
//! become dangerous — the kernel keeps running, sees no fresh commands, and holds.
//!
//! That single property licenses everything else here. This can be a large, allocating,
//! network-facing program with a JSON parser in it, because none of that is in the path between a
//! policy and a joint. The property is worth defending mechanically rather than remembering, so
//! **this crate does not depend on `skynet-rhal`** and CI asserts that it does not: a dependency
//! edge to the hardware abstraction layer is the first step in quietly acquiring the ability to
//! write a joint.
//!
//! # What is here, and what is deliberately not
//!
//! Everything in this crate is **pure**. [`reconcile::plan`] is a function of the desired state and
//! the observed state; it returns actions as *data*, each carrying the [`Gate`] that says what has
//! to be true before it may be applied. Supervision is a state machine that takes an event and a
//! clock reading rather than reading one. Health is an aggregation. Leases are a table with one
//! pre-emption rule.
//!
//! [`Gate`]: reconcile::Gate
//!
//! Nothing here opens a socket, spawns a process, writes a file or reads a clock. That is not
//! minimalism — it is what makes the level-triggered claim in §33.3 testable. "Running the loop
//! twice changes nothing the first run already did" is a property you can only assert about a
//! function; about a program that also talks to systemd, it is a hope.
//!
//! The I/O half — the systemd launcher, the RAUC slot reader, the package store, the Unix socket —
//! is **not written**, and is named as absent here rather than stubbed. A supervisor that reported
//! a plausible fleet status from nothing is the exact failure this project is arranged around.
//!
//! # The shape of one iteration
//!
//! ```text
//! loop:
//!   desired ← cached desired state on disk        (refreshed from the Hub when reachable)
//!   actual  ← observe(): installed packages, running units, image slot, grants, leases
//!   plan    ← diff(desired, actual)               ordered, §33.4
//!   for action in plan:
//!       if action.gate is not open: defer(action); continue
//!       apply(action); observe(); record()
//!   report(generation, actual, health, attestation)
//! ```
//!
//! **Observation is never assumed.** `actual` is read from the system, never from a memory of what
//! `skynetd` believes it did earlier. A supervisor that trusts its own record of the past is a
//! supervisor that disagrees with the machine it is supervising, and on a robot that disagreement
//! is discovered by a person. The types here enforce the shape of that discipline: [`plan`] takes
//! an [`Actual`] and has no way to produce one.
//!
//! [`plan`]: reconcile::plan
//! [`Actual`]: actual::Actual

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod actual;
pub mod desired;
pub mod health;
pub mod lease;
pub mod reconcile;
pub mod supervise;

#[cfg(test)]
mod tests;

pub use actual::{Actual, InstalledPackage, OsSlots, UnitState, UnitStatus};
pub use desired::{DesiredPackage, DesiredState, PackageKind, ProfileLimits, RunState};
pub use health::{Health, PackageHealth, Readiness};
pub use lease::{LeaseDenial, LeaseTable, PreemptionOutcome};
pub use reconcile::{plan, Action, ActionKind, Gate, Plan, SafeState};
pub use supervise::{CrashPolicy, RestartDecision, Supervision, SupervisionState};

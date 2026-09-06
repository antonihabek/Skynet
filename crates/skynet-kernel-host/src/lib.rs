//! The process that owns the control loop.
//!
//! §3's stack diagram has a band in it that the repository did not have:
//!
//! ```text
//! ├══════════════════════════════════════════════════════════════┤
//! │  ███  SAFETY KERNEL  ███                                     │  real-time
//! │  the only writer to an actuator · 8 guards · no allocation   │  no AI
//! ├══════════════════════════════════════════════════════════════┤
//! ```
//!
//! [`skynet_kernel`] is the eight guards and their arithmetic. It is `no_std`, takes no clock, does
//! no I/O, and cannot open a driver or read a ring — deliberately, because that is what makes it
//! small enough to keep identical across editions (§31.5). What it has never had is a **host**: a
//! process that opens the RHAL driver, turns the crank at a declared rate, and puts the kernel's
//! verdict on the wire.
//!
//! The consequence of that absence runs through the whole specification. §73's table has eight rows
//! whose "what is still missing" column says some version of *nothing has run a cycle* — five of
//! §74's telemetry counters report unmeasured because §13's ring has no action channels; seven of
//! §27's eight conditions are unobserved, so no robot ever enters a commanding state, so §49's
//! warning tone has never sounded; `field_conformance` is measurable rather than measured; no guard
//! has ever tripped on a machine. Every one of those is downstream of this loop not existing.
//!
//! # What this is not
//!
//! **It is not the policy runtime.** Nothing here loads a model, assembles an observation frame or
//! produces a desired command; this is the consumer of the ring, never the producer. A robot
//! running this and nothing else is a robot whose kernel is turning, whose guards are evaluating,
//! whose black box is filling — and whose policy side is empty, so every cycle reads
//! [`skynet_ring::Reading::Empty`] and holds. That is a real state and it is worth being able to
//! reach: it is the state §33.1 describes, in which `skynetd` can be killed and the robot holds
//! position safely.
//!
//! **It is not a claim about a tier.** [`skynet_kernel::Tier::enforcement`] derives enforcement from
//! the tier and no caller may pass one in, and running this as an ordinary process on a
//! general-purpose scheduler does not make it a host-tier robot. What decides that is whether the
//! process is scheduled before the next control deadline, which is a property of the kernel
//! configuration underneath it and is reported rather than assumed.
//!
//! # Structure
//!
//! [`cycle`] is pure: one command in, one decision out, no clock and no I/O, so the awkward cases —
//! the ramp, the escalation, the mask — are reachable from a test rather than from a robot.
//! [`driver`] is the RHAL behind a trait, so the loop can be driven against a fake with no shared
//! object present. [`run`] is the loop, and it is the only part with a clock in it.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod cycle;
pub mod driver;

/// §25's hand-off. The mechanism is a Unix socket; the surface exists everywhere and refuses.
pub mod handoff;
pub mod recorder;
pub mod run;

#[cfg(test)]
mod tests;

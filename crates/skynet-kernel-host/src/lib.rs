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
//! §27's eight conditions were unobserved, so no robot ever entered a commanding state, so §49's
//! warning tone has never sounded; `field_conformance` is measurable rather than measured; no guard
//! has ever tripped on a machine. Every one of those is downstream of this loop not existing.
//!
//! Two of them have since moved, and this crate is why: `kernel` and `rhal` are read off the cycles
//! this host records (§26.4), so five of the eight are unobserved rather than seven. The count is
//! not restated anywhere it could go stale — `skynetd_io::observed` derives it, and
//! `tools/scale-report.mjs --check` holds the documents to it.
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
//! [`capture`] is §47's half that only this process can do — a sensor read through the driver it
//! already holds — and it turns inside the cycle, after the write.
//! [`cycle`] is pure: one command in, one decision out, no clock and no I/O, so the awkward cases —
//! the ramp, the escalation, the mask — are reachable from a test rather than from a robot.
//! [`driver`] is the RHAL behind a trait, so the loop can be driven against a fake with no shared
//! object present. [`motion`] is §88's path — the one a body with no joints is commanded over, and
//! the only part of this crate that does not end at `write_command`. [`run`] is the loop, and it is
//! the only part with a clock in it.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod capture;
pub mod cycle;
pub mod driver;
/// Who §25's socket is shared with, when a package runs as its own user (§23.4).
pub mod group;
pub mod motion;

/// §25's hand-off. The mechanism is a Unix socket; the surface exists everywhere and refuses.
pub mod handoff;
pub mod plan;
pub mod post;
pub mod recorder;
pub mod run;
/// What the loop's timing looked like beyond its worst moment (12.C.5).
pub mod timing;

#[cfg(test)]
mod tests;

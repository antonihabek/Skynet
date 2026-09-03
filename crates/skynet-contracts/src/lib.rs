//! The Skynet OS contracts — level L2 of the edition matrix (§42).
//!
//! Four pure functions and the vocabularies they operate over:
//!
//! | Function | Answers |
//! |---|---|
//! | [`transport::resolve_transport`] | For each channel of this policy's action space, what does this body do with it? |
//! | [`admission::admit_package`] | May this package be installed on this robot? |
//! | [`certification::award_certification`] | What tier does this evidence support? |
//! | [`readiness::evaluate_readiness`] | Given what has been *declared*, which tier could this robot attempt? |
//!
//! Plus [`roles`], the canonical vocabulary all four lean on, and [`safety`], which re-exports the
//! Safety Kernel and renders its output.
//!
//! # Pure, and that is the whole point
//!
//! No I/O, no clock, no database, no network. Every input is an argument and every output is a
//! value. That is what lets the same function run **on the Hub**, so a repository page can answer
//! "does this run on my G1?" before anyone downloads forty gigabytes, and **on the robot**, at
//! install time, with no network at all. Neither side is authoritative because both compute the
//! same function over the same declared facts, and so they cannot disagree.
//!
//! # `no_std`
//!
//! `alloc` is used, `std` is not. A robot resolving a transport plan at install time may be a
//! Guest-tier container on a workstation or a Host-tier image with no operating system worth the
//! name underneath it, and the contracts should not be the reason the second is impossible.
//!
//! # The rule underneath all of it
//!
//! **Nothing here ever invents a number.** A body with no declared envelope produces a plan that
//! says so. A channel whose target joint declares no rating is reported unscalable rather than
//! scaled against a plausible guess. A required conformance test with no result is `missing`, never
//! a silent pass. A transport plan that quietly assumed a torque rating would be a safety check
//! that lies, which is the worst artifact this system could produce.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

extern crate alloc;

pub mod admission;
pub mod birth;
pub mod bundle;
pub mod calibration;
pub mod cell;
pub mod certification;
pub mod checkin;
pub mod digest;
pub mod embodiment;
pub mod entitlement;
pub mod identity;
pub mod intent;
pub mod json;
pub mod kinematics;
pub mod live;
pub mod observation;
pub mod opstate;
pub mod ota;
pub mod power;
pub mod predicate;
pub mod provenance;
pub mod readiness;
pub mod roles;
pub mod safety;
pub mod semver;
pub mod skill;
pub mod telemetry;
pub mod transport;
pub mod upload;
pub mod watchdog;

/// The version of the contract surface this crate implements.
///
/// Bumped when a corpus case changes meaning, not when an implementation detail changes. An
/// edition reports it at check-in so a fleet console can say which contracts a robot is actually
/// running, rather than inferring it from an OS version.
pub const CONTRACTS_VERSION: &str = "1.0.0";

pub use admission::{admit_package, AdmissionParams, AdmissionResult};
pub use certification::{award_certification, field_conformance, CertificationInput};
pub use digest::{version_digest, DigestError, DigestFile};
pub use kinematics::{flatten, with_frame_set, FlatFrames};
pub use opstate::{evaluate as evaluate_state, Conditions, OperatingState, Transition};
pub use predicate::{evaluate as evaluate_predicate, PredicateOutcome};
pub use readiness::{evaluate_readiness, summarize_readiness, ReadinessSubject};
pub use roles::{parse_role, ParsedRole};
pub use transport::{resolve_transport, ResolveTransportOptions, TransportPlan};

/// Round half away from zero.
///
/// `f64::round` does this, and it is not available under `no_std` without `libm` — and §31.2's rule
/// about platform maths libraries applies with full force to a rounding used in a latency
/// correction or a deadline: two editions rounding a half differently is a one-unit disagreement
/// today and a different chosen sample the day two candidates are equidistant.
///
/// It lives here rather than in whichever module needed it first, because the second caller is how
/// a private helper becomes two implementations of one decision.
///
/// **Not** JavaScript's `Math.round`, which is half toward positive infinity: the two differ on
/// negative halves, and every caller here is required to have checked its sign first.
pub(crate) fn round_half_away(value: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    let truncated = value as i64 as f64;
    let fraction = value - truncated;
    if fraction >= 0.5 {
        truncated + 1.0
    } else if fraction <= -0.5 {
        truncated - 1.0
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests;

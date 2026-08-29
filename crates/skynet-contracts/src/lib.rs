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
pub mod certification;
pub mod digest;
pub mod embodiment;
pub mod json;
pub mod kinematics;
pub mod readiness;
pub mod roles;
pub mod safety;
pub mod transport;

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
pub use readiness::{evaluate_readiness, summarize_readiness, ReadinessSubject};
pub use roles::{parse_role, ParsedRole};
pub use transport::{resolve_transport, ResolveTransportOptions, TransportPlan};

#[cfg(test)]
mod tests;

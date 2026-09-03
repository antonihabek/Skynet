//! `skynetd`'s I/O half — where the supervisor meets the machine.
//!
//! [`skynetd`] is the brain: `plan` is a function of two documents, supervision is a state machine
//! that is handed a clock reading, health is an aggregation. Nothing in it opens a socket, spawns a
//! process, writes a file or reads a clock, and that is what makes §33.3's two claims — converges
//! from any starting state, idempotent — properties a test can prove rather than hopes.
//!
//! This crate is the other half. It reads the package store, launches units, observes systemd,
//! serves the local API, and runs the loop. It is the part that can be wrong about the world.
//!
//! # Why two crates and not two modules
//!
//! Because the purity of the brain is then mechanical rather than remembered. `cargo tree` shows
//! `skynetd` depending on nothing that can perform I/O, and a future change that reached for
//! `std::fs` inside a `plan` would have to add a dependency somebody would see. The workspace
//! already holds one invariant of this kind — neither crate may depend on `skynet-rhal`, because
//! §33.1's "it never touches an actuator" is what licenses everything else about the daemon — and
//! CI asserts both.
//!
//! # The seam, and what it is for
//!
//! Everything that touches the outside world goes through a trait:
//!
//! | Trait | Real | Fake |
//! |---|---|---|
//! | [`launch::Launcher`] | [`launch::SystemdLauncher`] | [`launch::RecordingLauncher`] |
//! | [`observe::Slots`] | [`observe::RaucSlots`] | [`observe::FixedSlots`] |
//!
//! The fakes are not a testing convenience bolted on afterwards. §33.3 says a reconciler converges
//! *from any starting state, including states nobody anticipated*, and the states worth testing —
//! a half-installed package, a unit that will not stop, an image staged for a version nobody wants
//! — are states you cannot reach on a real machine on purpose. So the suite arranges them.
//!
//! The store is not behind a trait, because a store over a temporary directory *is* the real store:
//! everything it does is `std::fs` against a root it is handed, and a fake would test a different
//! program.
//!
//! # Platform
//!
//! A robot runs Linux (§23.5), and the parts that assume it — the Unix socket, the activation
//! symlink, `systemd-run`, cgroup properties — are compiled under `cfg(unix)`. What is left is
//! portable and is tested everywhere: the HTTP framing, the routing, the authorisation table, the
//! path and digest rules. That split is not a portability ambition. It is so that the half of this
//! crate a person is most likely to get wrong can be exercised without a robot.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod admit;
pub mod api;
pub mod audio;
pub mod audit;
pub mod auth;
pub mod birth;
pub mod birth_vector;
pub mod blackbox;
pub mod bundle;
pub mod calibration;
pub mod capacity;
pub mod cell;
pub mod checkin;
pub mod conformance;
pub mod connectivity;
pub mod console;
pub mod embodiment;
pub mod embodiment_vector;
pub mod entitlement;
pub mod http;
pub mod identity;
pub mod intent;
pub mod launch;
pub mod layout;
pub mod limits;
pub mod notes;
pub mod observe;
pub mod ota;
pub mod outbound;
pub mod post;
pub mod power;
pub mod releases;
pub mod runtime;
pub mod skill;
pub mod store;
pub mod telemetry;
pub mod trust;
pub mod upload;

#[cfg(unix)]
pub mod serve;

#[cfg(test)]
mod tests;

pub use layout::{Layout, PackageId};
pub use store::{StagedPackage, Store, StoreError};

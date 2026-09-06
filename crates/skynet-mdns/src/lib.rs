//! Finding a robot on the LAN (§32.2, §34.2) — DNS-SD over multicast DNS.
//!
//! §32.2 gives the console its reason for existing in one sentence: *"when the network is down the
//! Hub is unreachable, and that is exactly the moment somebody needs to see the robot."* And
//! [`serve`] holds that line properly — the LAN listener **refuses a routable address**, so the
//! console is bound link-local and cannot be reached from outside the robot's own network.
//!
//! Which leaves the console reachable and **unaddressable**. To open it you need the robot's
//! address, and a link-local IPv6 address carries a zone index — `fe80::a1b2:c3d4:e5f6:0708%en0` —
//! which nobody types, guesses, or reads off a robot. The documented answer today is that an
//! operator already knows where the robot is. On a bench with three of them, or on the first boot
//! of a robot whose network configuration is wrong, they do not.
//!
//! Nothing in this repository advertised and nothing discovered: before this crate, `mdns`, `_tcp`
//! and `dns-sd` had zero occurrences in the tree. So the bootstrap hole was not a gap in an
//! implementation, it was a missing subsystem, and it is the one piece of the operator-facing tool
//! that **has no browser substitute** — a web page cannot browse mDNS, and that is not a
//! permissions question, it is that no browser exposes the protocol at all.
//!
//! # What this crate is, and the three things it deliberately is not
//!
//! It is the wire format ([`wire`]), DNS-SD's TXT convention ([`txt`]), the definition of what a
//! Skynet robot advertises ([`service`]), and a one-shot browse ([`browse`]).
//!
//! It is **not** a general mDNS implementation, and the omissions are chosen rather than pending:
//!
//! * **No cache.** RFC 6762's cache is for a resolver answering repeated questions cheaply. This
//!   answers one question — *which robots are on this network right now* — and a cached answer to
//!   that is a robot that may have been unplugged. The tool is for the moment something is wrong;
//!   a stale entry is the failure mode, not the optimisation.
//! * **No probing, no conflict resolution, no announcement state machine.** Those belong to a
//!   responder claiming a name, which is the robot's half.
//! * **No continuous monitoring.** A browse is a question with a deadline. Watching a network over
//!   time is a different shape with a different failure mode, and building it as a side effect of
//!   this would produce a bad version of it.
//!
//! # Both halves are defined here; only one of them is wired
//!
//! §79 is this repository's record of what half a route costs: an address answered, a sender that
//! backed off politely, no bytes moved, and *"the failure was indistinguishable from patience."*
//! A browser shipped against a robot that does not advertise would find nothing and look exactly
//! like a network with no robots on it.
//!
//! So [`service::advertisement`] — the records a robot answers a browse *with* — lives here, beside
//! the parser that reads them, and both halves will share one definition rather than two spellings
//! that agree until they do not (§78). What is **not** done is binding the responder's socket in
//! `skynetd`, which is Unix-only code and the daemon's to own.
//!
//! **Until that lands, a browse on a real network finds nothing, and that is a true report.**
//! [`browse::Browse`] therefore distinguishes *no robot answered* from *this machine could not
//! ask* — see its header, because on a laptop with multicast filtered by a VPN those two look
//! identical and only one of them is about robots.
//!
//! # Every bound is a refusal
//!
//! `http.rs` states the rule this parser inherits: *"a parser that silently drops a header it did
//! not have room for is a parser that can be made to forget an `Authorization`."* Every limit here
//! produces a [`wire::WireError`] rather than a shorter answer, and a malformed packet is discarded
//! whole rather than half-read — because the packets arrive from any host on the LAN, unsolicited
//! and unauthenticated, and the discovery tool is the one an operator runs when they already
//! suspect something is wrong on that network.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod browse;
pub mod responder;
pub mod service;
pub mod txt;
pub mod wire;

//! The producer §25's command ring has never had.
//!
//! ```text
//! Policy backend                       Safety Kernel                     RHAL
//! ──────────────                       ─────────────                     ────
//! observation frame  ──assembler──▶
//!                    ◀── inference ──
//! desired[]  ──▶ ┌──────────────────┐
//!                │ SPSC ring, shm   │ ──▶ 8 guards ──▶ rhal_command ──▶ write_command()
//!                │ fixed 4 KiB slots│      (§7.1)
//!                └──────────────────┘
//! ```
//!
//! `skynet-kernel-host` is everything to the right of the ring, and its own header says what it is
//! not: *"Nothing here loads a model, assembles an observation frame or produces a desired command;
//! this is the consumer of the ring, never the producer."* This crate is the left-hand column.
//!
//! # Why an empty ring was not a small gap
//!
//! §73's table has eight rows whose "what is still missing" column reduces to one sentence — *the
//! ring has no action channels* — and every one of them is downstream of nothing publishing:
//!
//! * five of §74's telemetry counters report `unmeasured` on a shipping robot, because they are
//!   counted off commanded cycles and no cycle has ever been commanded;
//! * §15's `field_conformance` is refused rather than reported, because a ratio over a window
//!   nothing commanded is a perfect `1.0000` by arithmetic and reads on a page as a robot that did
//!   everything right rather than one that did nothing;
//! * §32.1's guard-trip column has a source and no robot that has ever tripped a guard;
//! * no skill has executed, so §54.4's `intervention_count` is attributed to nothing;
//! * and §26.3's open question — what shape a supervisor opens its recording ring at — waits on a
//!   **channel count**, which the specification says is "the policy runtime's, and that runtime is
//!   unwritten".
//!
//! This crate is that runtime, and [`Commanding::channels`] is that number.
//!
//! # What this is, and the three things it is deliberately not
//!
//! **It is not an inference runtime.** §23.3 puts CUDA, PyTorch and ONNX Runtime above the kernel
//! and outside everything that has to stay small; loading a neural network is a [`Backend`] nobody
//! in this repository has written, and the trait exists so that absence is a named vacancy rather
//! than a silent one. What ships here are backends that are honest about being simple: one that
//! commands nothing and one that plays a table of setpoints open-loop.
//!
//! **It never touches the RHAL.** §25: *"The policy cannot reach the kernel except through the
//! ring, and cannot reach the RHAL at all."* This process opens one shared mapping and writes
//! commands into it. It holds no actuator descriptor, and the kernel host — a separate process
//! with a separate lifetime — is the only writer to a joint.
//!
//! **It is not a safety claim.** Nothing here decides a tier, and a guard that refuses one of these
//! commands is the system working. A robot running this and a kernel host is a robot with a
//! complete control path; whether that path is *safe* is decided by the eight guards on the other
//! side of the ring, which is the entire architecture of §7 and is not this crate's to assert.
//!
//! # The distinction the whole design turns on
//!
//! A policy has three things it can be doing, and two of them look identical from the ring if you
//! model them as `Option<Command>`. [`Intent`] makes all three sayable:
//!
//! | | published | sequence | what the kernel's watchdog sees |
//! |---|---|---|---|
//! | [`Intent::Command`] | a command | advances | a live producer |
//! | [`Intent::Hold`] | a command with an all-zero mask | **advances** | a live producer asking for nothing |
//! | [`Intent::Silent`] | **nothing** | **does not advance** | a **stalled producer**, and a controlled stop |
//!
//! `Hold` and `Silent` are the two that must never be confused, and an `Option` would have made
//! them the same value. A policy that has decided to command nothing is alive and says so every
//! cycle; a policy that has stopped producing is a fault, and §24's rule — *"the watchdog treats a
//! repeat as a stalled producer"* — is what turns it into a stop. Publishing nothing is how this
//! crate reports the second, and it is deliberately the cheapest thing it can do, because a runtime
//! in trouble should not need to succeed at anything in order to fail safely.
//!
//! # Structure
//!
//! [`backend`] is the policy interface and the backends that ship with it, all pure: a tick number
//! and a clock in, an [`Intent`] out, no I/O and no ring. [`run`] is the loop and the only part
//! that publishes.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod backend;
pub mod run;

pub use backend::{Approach, Backend, Body, Commanding, Hold, Intent, Trajectory};
pub use run::{Clock, PolicyHost, Refusal, StepClock, Summary, SystemClock, Tick};

#[cfg(test)]
mod tests;

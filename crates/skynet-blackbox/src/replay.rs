//! Deterministic replay (§13, §15, §56.4).
//!
//! > Given the same policy digest, observation stream and embodiment digest, replay reproduces the
//! > same actions. `telemetry.replay.deterministic` requires it. This turns "the robot did
//! > something strange on Tuesday" from an anecdote into a regression test.
//!
//! # What is actually being checked
//!
//! Not "the policy would produce the same output" — that is a claim about a neural network on a
//! particular accelerator, and this crate cannot make it. The policy's output is *recorded*, and
//! what replay verifies is the half of the cycle that is deterministic by construction: **given the
//! recorded desired values, the recorded timing and the same resolved limits, does this edition's
//! Safety Kernel reach the recorded verdict?**
//!
//! That is a narrower claim and a much stronger one, because it is checkable. It fails when:
//!
//! * the kernel changed behaviour between the recording and the replay — an OS upgrade that quietly
//!   altered a clamp, which is precisely the regression nobody catches in the field;
//! * two editions disagree, which §31.4 says is a specification ambiguity to resolve rather than a
//!   bug to hide;
//! * the limits handed to the replay are not the ones that were in force, which is itself worth
//!   knowing: it means the incident report is being read against the wrong configuration.
//!
//! It is also what makes a disputed benchmark answerable (§56.4). A result whose recording replays
//! is a result somebody else can check; one whose recording diverges is a result with a question
//! in it.
//!
//! # `previous` is threaded, and that is where the subtlety is
//!
//! The rate-limit guard compares this cycle's command against what was actually sent last cycle. So
//! replay cannot evaluate a cycle in isolation: it walks the window in order, carrying the previous
//! *command* forward, exactly as the control loop did. Replaying from the middle of a recording
//! therefore has one unavoidable weakness — the first cycle has no predecessor — and
//! [`Replay::warmed`] says how many cycles in the comparison became sound, rather than pretending
//! the first one was.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_kernel::{evaluate_cycle, CommandCycle, Envelope, FaultResponse, Guard, ResolvedLimits};

use crate::record::{Body, Cycle, Record, NO_CAPABILITY};
use crate::ring::Ring;

/// How one cycle's replay came out.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    /// The record's sequence number.
    pub sequence: u64,
    /// The control tick.
    pub tick: u64,
    /// Empty when the recomputed verdict matched the recorded one.
    pub divergences: Vec<Divergence>,
    /// Whether this cycle had a predecessor, and so whether the rate limit could be evaluated as it
    /// was on the robot.
    pub warmed: bool,
}

impl Outcome {
    /// Whether this cycle reproduced.
    pub fn agreed(&self) -> bool {
        self.divergences.is_empty()
    }
}

/// One way a recomputed verdict differed from the recorded one.
#[derive(Debug, Clone, PartialEq)]
pub struct Divergence {
    /// Which part of the verdict.
    pub field: &'static str,
    /// What the recording says happened.
    pub recorded: String,
    /// What this edition computes.
    pub recomputed: String,
}

/// What a whole replay concluded.
#[derive(Debug, Clone, PartialEq)]
pub struct Replay {
    /// Cycles compared.
    pub cycles: usize,
    /// Cycles that reproduced exactly.
    pub agreed: usize,
    /// Every cycle that did not, in order.
    ///
    /// All of them, not the first: a replay that stopped at the first divergence would answer "did
    /// this change" but not "how much of the recording is affected", and the second question is the
    /// one an incident review asks.
    pub diverged: Vec<Outcome>,
    /// Cycles whose predecessor was in the window, so the rate limit was evaluated as it was.
    pub warmed: usize,
    /// Cycles skipped because the recording could not supply an input.
    ///
    /// Reported rather than counted as agreement. A replay that treated an unevaluable cycle as a
    /// pass would be the conformance-suite failure §15 spends a rule on.
    pub skipped: Vec<Skipped>,
}

impl Replay {
    /// Whether every cycle in the window reproduced.
    ///
    /// A skipped cycle is not an agreement, so this is false when anything was skipped. The
    /// alternative — `agreed == cycles` while a third of the window went unevaluated — is a green
    /// light that means nothing.
    pub fn deterministic(&self) -> bool {
        self.diverged.is_empty() && self.skipped.is_empty() && self.cycles > 0
    }
}

/// A cycle that could not be replayed, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    /// The record's sequence number.
    pub sequence: u64,
    /// What was missing.
    pub reason: String,
}

/// Replay every cycle in `records` against `limits`.
///
/// Takes the two name tables rather than a [`Ring`], so that a recording read from a sealed file —
/// which has the names and not the ring — replays through the same function as one read from the
/// disk it was written to. [`replay_ring`] is the convenience for the second case.
///
/// The limits and envelope are supplied by the caller because they are not in the recording: they
/// are resolved from the embodiment descriptor, the operator profile and the transport clamp, all
/// of which the recording *names* — through the embodiment digest in its header — without
/// carrying. Handing them in is therefore also the check that the right ones were used, and a
/// replay run against the wrong profile diverges loudly rather than agreeing quietly.
pub fn replay(
    records: &[Record],
    channels: &[String],
    capabilities: &[String],
    limits: &ResolvedLimits<'_>,
    envelope: Option<&Envelope<'_>>,
) -> Replay {
    let mut result = Replay {
        cycles: 0,
        agreed: 0,
        diverged: Vec::new(),
        warmed: 0,
        skipped: Vec::new(),
    };

    // Carried across cycles, exactly as the control loop carries them.
    let mut previous: Option<Vec<(String, f64)>> = None;
    let mut last_sequence: Option<u64> = None;

    for record in records {
        let Body::Cycle(cycle) = &record.body else {
            continue;
        };
        result.cycles += 1;

        if cycle.desired.len() > channels.len() {
            result.skipped.push(Skipped {
                sequence: record.sequence,
                reason: "the header names fewer channels than the record holds values".to_string(),
            });
            previous = None;
            last_sequence = None;
            continue;
        }

        let desired: Vec<(&str, f64)> = cycle
            .desired
            .iter()
            .enumerate()
            .map(|(index, value)| (channels[index].as_str(), *value as f64))
            .collect();
        let carried: Option<Vec<(&str, f64)>> = previous.as_ref().map(|held| {
            held.iter()
                .map(|(name, value)| (name.as_str(), *value))
                .collect()
        });
        let warmed = carried.is_some();
        if warmed {
            result.warmed += 1;
        }

        let capability = if cycle.capability == NO_CAPABILITY {
            None
        } else {
            capabilities
                .get(cycle.capability as usize)
                .map(String::as_str)
        };
        let nearest_person_m = if cycle.nearest_person_m.is_finite() {
            Some(cycle.nearest_person_m as f64)
        } else {
            None
        };

        let recomputed = evaluate_cycle(
            &CommandCycle {
                // Nanoseconds to milliseconds, in `f64`, which is exact for every value below 2^53
                // nanoseconds — 104 days of uptime. Beyond that the conversion rounds, and a robot
                // that has been running for a hundred days is a robot whose watchdog window is
                // being compared at microsecond precision rather than nanosecond. Recorded here
                // rather than papered over.
                now_ms: record.mono_ns as f64 / 1.0e6,
                command_ms: cycle.command_stamp_ns as f64 / 1.0e6,
                sequence: cycle.command_seq as u64,
                desired: &desired,
                previous: carried.as_deref(),
                capability,
                nearest_person_m,
            },
            limits,
            envelope,
            last_sequence,
        );

        let mut divergences = Vec::new();
        compare(&mut divergences, cycle, &recomputed, channels);

        if divergences.is_empty() {
            result.agreed += 1;
        } else {
            result.diverged.push(Outcome {
                sequence: record.sequence,
                tick: cycle.tick,
                divergences,
                warmed,
            });
        }

        // What was *recorded* as sent, not what was recomputed. Replay reconstructs the robot's
        // history; carrying the recomputed command forward would compare the second cycle against a
        // world the robot never entered, and one divergence would cascade into every cycle after
        // it.
        previous = Some(
            channels
                .iter()
                .take(cycle.command.len())
                .cloned()
                .zip(cycle.command.iter().copied())
                .collect(),
        );
        last_sequence = Some(cycle.command_seq as u64);
    }

    result
}

fn compare(
    into: &mut Vec<Divergence>,
    recorded: &Cycle,
    recomputed: &skynet_kernel::CycleVerdict<'_>,
    channels: &[String],
) {
    let recorded_response = recorded
        .response
        .map(FaultResponse::as_str)
        .unwrap_or("none");
    let recomputed_response = recomputed
        .response
        .map(FaultResponse::as_str)
        .unwrap_or("none");
    if recorded_response != recomputed_response {
        into.push(Divergence {
            field: "response",
            recorded: recorded_response.to_string(),
            recomputed: recomputed_response.to_string(),
        });
    }

    let recorded_tripped = names(&recorded.tripped);
    let recomputed_tripped = names(&recomputed.tripped.iter().copied().collect::<Vec<_>>());
    if recorded_tripped != recomputed_tripped {
        into.push(Divergence {
            field: "tripped",
            recorded: recorded_tripped,
            recomputed: recomputed_tripped,
        });
    }

    if recorded.advisory != recomputed.advisory {
        into.push(Divergence {
            field: "advisory",
            recorded: recorded.advisory.to_string(),
            recomputed: recomputed.advisory.to_string(),
        });
    }

    // The command, channel by channel. A verdict under a stop carries no command at all, which is a
    // different fact from a command of zero — so the two are compared by presence as well as value.
    for (index, name) in channels.iter().enumerate().take(recorded.command.len()) {
        let was = recorded.command[index];
        let now = recomputed
            .command
            .iter()
            .find(|(channel, _)| *channel == name.as_str())
            .map(|(_, value)| *value);
        match now {
            // Bit equality, not `==`: a clamp emits a specific `f64` and a replay that produced a
            // value one unit in the last place away has not reproduced the verdict. `NaN` compares
            // unequal to itself under `==`, and a recording holding one must still be able to
            // agree with a replay that produced one.
            Some(value) if value.to_bits() == was.to_bits() => {}
            Some(value) => into.push(Divergence {
                field: "command",
                recorded: alloc::format!("{name}={was}"),
                recomputed: alloc::format!("{name}={value}"),
            }),
            None if recorded.command.iter().all(|value| *value == 0.0) => {
                // A stop verdict: the kernel sends nothing and the recording holds the zeroed
                // array a fixed-size record leaves behind. Not a divergence.
            }
            None => into.push(Divergence {
                field: "command",
                recorded: alloc::format!("{name}={was}"),
                recomputed: alloc::format!("{name} absent"),
            }),
        }
    }
}

fn names(guards: &[Guard]) -> String {
    guards
        .iter()
        .map(|guard| guard.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// Replay a ring in place: read it, order it, and hand it to [`replay`].
pub fn replay_ring(
    ring: &Ring,
    store: &[u8],
    limits: &ResolvedLimits<'_>,
    envelope: Option<&Envelope<'_>>,
) -> Replay {
    let held = ring.ordered(store);
    replay(
        &held,
        &ring.meta().channels,
        ring.capabilities(),
        limits,
        envelope,
    )
}

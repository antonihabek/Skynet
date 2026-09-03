//! §74's counters, counted off §13's ring.
//!
//! §74 gives telemetry a route and eight counters, and five of them — `cycles`, `interventions`,
//! `guard_trips`, `clamps` and `estops` — were reported unmeasured on the grounds that they *"need
//! a kernel"*. They do not need a kernel to be **counted**. They need a kernel to *happen*, and
//! when one happens every one of them is already written down: §13's ring is the record of what the
//! kernel did, cycle by cycle, and §26 puts an estop on its event channel. The daemon opens that
//! ring, holds it, seals it and serves it.
//!
//! So the source was never missing. Nothing had walked the ring and counted.
//!
//! # A window, not the recording
//!
//! [`crate::seal::seal`] counts cycles and interventions over the **whole** ring, which is the
//! right window for an incident and the wrong one for §74: a telemetry window is the interval
//! between two reconcile passes, and the ring holds minutes or years. So a tally is taken from a
//! sequence number — the ring's [`Ring::next_sequence`] at the instant the window opened — and
//! covers every record at or after it.
//!
//! # Under-counting is the failure this has to avoid, and it is detectable
//!
//! A ring is circular. A window whose opening sequence has been overwritten still yields a number,
//! and that number is a **floor** presented as a count — which is exactly §47.4's *"a gap that
//! looks like a quiet period"*, in the one place §74.1 chose its whole shape to avoid it. A robot
//! logging at 1 kHz into a ring that holds seventy seconds overwrites a five-minute window four
//! times over, and the tally would report the tail as the whole.
//!
//! [`Tally::complete`] is therefore not advisory. A tally that is not complete is one whose counts
//! must be reported as **unmeasured**, so the window says nothing rather than something small, and
//! the Hub's own coverage arithmetic (§74.5) sees the hole it actually has. Losing a window is
//! cheap by construction: §74.1 made the windows tile so that a Hub can see precisely which ones it
//! does not have.
//!
//! # What is counted, and at which granularity
//!
//! Three different granularities, one per counter, each the granularity of the thing its name
//! names:
//!
//! | Counter | Counts | Why not per cycle |
//! |---|---|---|
//! | `cycles` | control cycles | — |
//! | `interventions` | cycles the kernel intervened in | §15 defines field conformance over cycles |
//! | `guard_trips` | **guards** that tripped | a cycle in which three guards fire is three trips |
//! | `clamps` | **channels** whose command was clamped | a clamp is applied to a channel |
//! | `estops` | assertions, not clearances | §26's `estop` event |
//!
//! `interventions` and `guard_trips` would otherwise be the same number under two names, and a
//! console dividing by both would draw one line twice. The distinction is the useful one: how often
//! the kernel had to act, against how much acting it did.

use alloc::vec::Vec;

use crate::record::{Body, EventCode, Record};
use crate::ring::Ring;

/// What one telemetry window's worth of ring holds (§74).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Tally {
    /// Control cycles in the window.
    pub cycles: u64,
    /// Cycles the kernel intervened in — the numerator §15's field conformance is computed from.
    pub interventions: u64,
    /// Guards that tripped, summed over the window's cycles.
    pub guard_trips: u64,
    /// Channels whose command was clamped, summed over the window's cycles.
    pub clamps: u64,
    /// Emergency stops asserted (§26's `estop`). Clearances are not counted.
    pub estops: u64,
    /// Whether this ring can hold a control cycle at all.
    ///
    /// A ring created with no action channels is a robot with **no command path**: the policy
    /// runtime and observation assembly (§25) do not exist on this edition, so §26's state,
    /// desired, verdict and command channels have no producer, and neither has the safety path that
    /// asserts a stop. Every count above is then zero because nothing writes them, which is not the
    /// same fact as a robot that commanded a body and tripped nothing — and it is the second fact a
    /// fleet page would render.
    ///
    /// The recorder's own index already says this in a sentence, for the same reason. This is the
    /// machine-readable half of it, and it is a property of the *ring* rather than of the edition:
    /// the day a control loop opens a ring with channels in it, these counters begin reporting with
    /// no further change here.
    pub commands: bool,
    /// The window's first sequence was no longer in the ring: it wrapped, and records were lost.
    pub truncated: bool,
    /// Sequence numbers missing from inside the window.
    ///
    /// A hole is a record that was written and is not here, which is a defect. Records older than
    /// the window are not holes: the ring is a window and the window moving on is what a ring is
    /// for — that case is [`Tally::truncated`].
    pub holes: u64,
    /// Records the producer itself said it dropped, over every gap inside the window (§47.4).
    pub dropped: u64,
    /// Slots in the ring whose CRC did not match.
    ///
    /// Over the **whole ring** rather than the window, and it cannot be otherwise: a torn record's
    /// sequence number is part of what was torn, so there is nothing to place it in a window by.
    /// Any damage at all therefore puts the window's completeness in doubt, which is the safe
    /// direction and the honest one.
    pub damaged: u64,
}

impl Tally {
    /// Whether these counts are a count rather than a floor.
    ///
    /// A tally that is not complete must be reported as unmeasured. See the module header: a floor
    /// presented as a count is the one failure §74.1 picked its shape to make impossible, and
    /// getting it back by under-counting inside a window would be a poor trade.
    pub fn complete(&self) -> bool {
        !self.truncated && self.holes == 0 && self.dropped == 0 && self.damaged == 0
    }

    /// Whether these counts may be reported at all.
    ///
    /// Both halves: something must have been able to write what is counted, and what was written
    /// must all still be here.
    pub fn measured(&self) -> bool {
        self.commands && self.complete()
    }
}

/// Count one window off a ring (§74).
///
/// `from_sequence` is the ring's [`Ring::next_sequence`] at the instant the window opened, so the
/// window is every record at or after it. Sequences are dense and start at one, so a
/// `from_sequence` of zero counts the whole recording — which is what a caller with no opening
/// reading should get, rather than nothing.
///
/// **Zero is never truncated**, and the distinction is not a special case dodged: a caller with no
/// opening sequence has no window boundary, so there is nothing it could have lost. Records older
/// than the ring's first are not a defect — the ring is a window and the window moving on is what a
/// ring is for, which is [`Continuity::holes`](crate::Continuity)' own argument. Truncation is the
/// different fact that a boundary somebody *named* has been overwritten.
pub fn tally(ring: &Ring, store: &[u8], from_sequence: u64) -> Tally {
    let held = ring.ordered(store);
    let damaged = ring
        .records(store)
        .filter(|(_, record)| matches!(record, Err(crate::record::RecordError::Torn)))
        .count() as u64;

    // The oldest sequence still in the ring. If it is *past* where the window opened, the ring
    // wrapped over the window's own records and every count below is a floor.
    //
    // Equality is not truncation: `from_sequence` is where the next record will go, so a ring whose
    // oldest record *is* that sequence has lost nothing of this window.
    let truncated = from_sequence > 0
        && held
            .first()
            .is_some_and(|oldest| oldest.sequence > from_sequence);

    let mut out = tally_of(&held, from_sequence, ring.geometry().shape.channels > 0);
    out.truncated = truncated;
    out.damaged = damaged;
    out
}

/// Count a window off records already read back, for a caller that has them (§74).
///
/// The ring is read whole in order to seal it, so a caller that has just sealed holds the records
/// and should not read the file twice. `commands` is the ring's channel count being non-zero.
///
/// [`Tally::truncated`] and [`Tally::damaged`] are **not** set here: both are properties of the
/// ring rather than of a list of records, and a caller using this directly has to supply them.
pub fn tally_of(held: &[Record], from_sequence: u64, commands: bool) -> Tally {
    let mut out = Tally {
        commands,
        ..Tally::default()
    };

    let mut previous: Option<u64> = None;
    for record in held.iter().filter(|r| r.sequence >= from_sequence) {
        // Holes inside the window, found the way `continuity_of` finds them: the records arrive in
        // sequence order and dense, so any step of more than one is what is not here.
        if let Some(last) = previous {
            out.holes = out
                .holes
                .saturating_add(record.sequence.saturating_sub(last) - 1);
        }
        previous = Some(record.sequence);

        match &record.body {
            Body::Cycle(cycle) => {
                out.cycles += 1;
                if cycle.intervened() {
                    out.interventions += 1;
                }
                out.guard_trips += cycle.tripped.len() as u64;
                out.clamps += cycle.clamped.iter().filter(|clamped| **clamped).count() as u64;
            }
            // Asserted, not cleared. A window that counted both would report two for one stop and
            // make the rate depend on whether the operator had got round to clearing it.
            Body::Event(event) if event.code == EventCode::Estop => out.estops += 1,
            // §47.4: the producer saying what it lost is the only reason the loss is visible at all,
            // so it is summed rather than noted.
            Body::Gap(gap) => out.dropped = out.dropped.saturating_add(gap.dropped),
            _ => {}
        }
    }

    out
}

/// The records a ring holds, oldest first — for a caller that wants both a tally and a seal.
///
/// A thin re-export of [`Ring::ordered`] so a caller does not have to name the ring module to get
/// the argument [`tally_of`] wants.
pub fn ordered(ring: &Ring, store: &[u8]) -> Vec<Record> {
    ring.ordered(store)
}

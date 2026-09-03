//! §74's windows, on a real machine: what this robot can count, and what it refuses to guess.
//!
//! [`skynet_contracts::telemetry`] decides what a window *is* and takes no clock. This is the other
//! half — reading two clocks, counting what there is to count, and putting the result in §34.4's
//! queue as the class that is dropped first and aggressively.
//!
//! # Where each counter comes from
//!
//! §74.3 keeps zero and unmeasured apart for §33.6's reason, and every counter here has to say
//! which of the two it is. They come from three places:
//!
//! * **This loop.** `reconcile_passes` is what it did, `queue_dropped` is §34.4's own counter — the
//!   one the section is written around, *"this robot dropped forty thousand telemetry items last
//!   month"* — and `package_restarts` is §33.5's supervisor, which decides every restart on this
//!   machine.
//! * **§13's ring.** `cycles`, `interventions`, `guard_trips`, `clamps` and `estops` are counted
//!   off the black box by [`skynet_blackbox::tally`], because that is where the kernel's verdicts
//!   already are. See below.
//! * **Nowhere.** Nothing, now. There is no counter left in §74's set with no source.
//!
//! ## The five were never waiting on a kernel to count them
//!
//! This module used to say the five *"need a kernel, which this daemon does not host"*, and that
//! they needed one to **happen** is true. It does not follow that they needed one to be counted:
//! §13's ring is the record of what the kernel did, cycle by cycle, §26 puts an estop on its event
//! channel, and this daemon opens that ring, holds it, seals it and serves it over §33.7. The
//! source was in the process the whole time and nothing had walked it.
//!
//! One of the five was wrong for a second reason. `interventions` was recorded as needing
//! *teleoperation*, and §15 is explicit that field conformance is *"the ratio of cycles the kernel
//! did not have to intervene in"* — a kernel intervention, which `Cycle::intervened` has computed
//! and named as `field_conformance`'s input since the ring was written. A human taking over is
//! §54.4's `intervention_count` on an **episode**, which is a different number on a different
//! route. Two facts with one word between them, and the wrong one had been written down.
//!
//! ## What the ring cannot tell you is reported as unmeasured, not as zero
//!
//! [`skynet_blackbox::tally::Tally::measured`] has two halves and both matter here. A ring created
//! with no action channels is a robot with **no command path** — the policy runtime and observation
//! assembly (§25) are unwritten on this edition — so its zero cycles are the absence of a producer
//! rather than a measurement, and reporting them would put a green guard column over a robot that
//! has never commanded a joint. And a ring that **wrapped over the window it was asked about**
//! yields a floor rather than a count, which is §47.4's *"gap that looks like a quiet period"* in
//! the one place §74.1 chose its whole shape to avoid it.
//!
//! So on the edition that ships today these five are still absent from a window, and the reason is
//! now a property of the ring rather than a sentence in this file. The day a control loop opens a
//! ring with channels in it, they begin reporting, and nothing here changes.
//!
//! # The window is closed by the same pass that opens the next one
//!
//! There is no timer. A window covers the interval between two reconcile passes, so a robot that
//! stops reconciling stops producing windows — which is the correct behaviour and not a bug: the
//! gap is then visible on the Hub as a gap, which is exactly what §47.4 asks for and what a
//! heartbeat produced on a separate timer would hide.

use skynet_contracts::telemetry::{assess_window, Window};
use skynet_json::Json;

use crate::layout::Layout;
use crate::outbound::{Class, Queue};

/// What this robot has counted since the last window closed.
///
/// Held in memory rather than on disk, deliberately. A counter that survived a reboot would be a
/// running total in disguise, and §74.1 rules those out: the drop becomes invisible because the
/// next total absorbs it. A reboot ends a window — the counts in the open one are lost, and the
/// *gap* is what the Hub sees, which is the honest report of a robot that restarted.
#[derive(Debug, Clone, Default)]
pub struct Counting {
    /// Wall-clock milliseconds when this window opened. §35.1's correlation clock.
    pub opened_wall_ms: f64,
    /// Monotonic milliseconds when it opened. §35.1's measurement clock.
    pub opened_monotonic_ms: f64,
    /// Reconcile passes since it opened.
    pub passes: u64,
    /// Queue drops observed when it opened, across every class.
    pub dropped_at_open: u64,
    /// Restarts §33.5's supervisor decided on since it opened (§74's `package_restarts`).
    ///
    /// A count since the window opened rather than a reading taken at both ends, which is the
    /// difference between this and `dropped_at_open` beside it: the queue keeps a running total
    /// this module can subtract, and the supervisor keeps a bounded failure list whose entries
    /// expire out of §33.5's window. Subtracting two readings of that would report a *negative*
    /// number of restarts on a robot that had been quiet for ten minutes.
    pub restarts: u64,
    /// The black box's next sequence number when this window opened.
    ///
    /// The window's boundary in the ring, and the only thing that has to be remembered for the five
    /// counters the ring supplies: everything else is read back out of the recording at close.
    ///
    /// Zero is a window opened with **no recorder**, which is a robot whose black box would not
    /// open — [`crate::blackbox::Recorder::open`] recreates a damaged ring rather than refusing to
    /// start, so this is rare and is not the ordinary case. It is carried as zero rather than as an
    /// `Option` because [`skynet_blackbox::tally::tally`] already gives zero the meaning "no
    /// boundary was named", and a second spelling of the same absence is how the two drift apart.
    pub opened_sequence: u64,
}

impl Counting {
    /// Open a window now.
    ///
    /// `sequence` is the black box's next sequence number, or zero when there is no recorder.
    pub fn opened(wall_ms: f64, monotonic_ms: f64, dropped: u64, sequence: u64) -> Counting {
        Counting {
            opened_wall_ms: wall_ms,
            opened_monotonic_ms: monotonic_ms,
            passes: 0,
            dropped_at_open: dropped,
            restarts: 0,
            opened_sequence: sequence,
        }
    }

    /// Close it, producing the window that crosses to the Hub.
    ///
    /// A counter this robot could not measure is **left absent** rather than set to zero. That is
    /// §74.3, and it is the difference between a fleet page saying "this robot has tripped no
    /// guards" and "nothing on this robot has ever counted a guard trip" — the first of which is a
    /// claim about a machine nobody has measured.
    ///
    /// `tally` is what §13's ring holds for this window, or `None` when there is no ring to ask or
    /// it could not be read. A tally that is present but not
    /// [`measured`](skynet_blackbox::tally::Tally::measured) is treated exactly like its absence,
    /// and the module header says why: a ring with no command path and a ring that wrapped over the
    /// window are different reasons to say nothing, and both of them are reasons to say nothing.
    pub fn close(
        &self,
        wall_ms: f64,
        monotonic_ms: f64,
        dropped: u64,
        tally: Option<skynet_blackbox::tally::Tally>,
    ) -> Window {
        let window = Window {
            from_ms: Some(self.opened_wall_ms),
            to_ms: Some(wall_ms),
            elapsed_ms: Some(monotonic_ms - self.opened_monotonic_ms),
            counters: Vec::new(),
        }
        .with(
            "queue_dropped",
            Some(dropped.saturating_sub(self.dropped_at_open) as f64),
        )
        .with("reconcile_passes", Some(self.passes as f64))
        .with("package_restarts", Some(self.restarts as f64));

        // All five or none of them. They come from one walk of one recording, so a window carrying
        // some of them would be describing a ring that was partly readable — which is not a state
        // the ring has, and would leave a consumer dividing a measured numerator by an absent
        // denominator.
        let Some(tally) = tally.filter(|tally| tally.measured()) else {
            return window;
        };

        window
            .with("cycles", Some(tally.cycles as f64))
            .with("interventions", Some(tally.interventions as f64))
            .with("guard_trips", Some(tally.guard_trips as f64))
            .with("clamps", Some(tally.clamps as f64))
            .with("estops", Some(tally.estops as f64))
    }
}

/// The window a robot sends, as JSON.
///
/// The five fields the contract reads and nothing else. A window carrying the robot's extras would
/// be inviting a Hub to start depending on one, and telemetry is the class designed to be lost.
pub fn window_to_json(window: &Window) -> Json {
    let mut counters = Json::obj();
    for (name, count) in &window.counters {
        match count {
            Some(value) => counters.set(name, Json::Num(*value)),
            // Present-and-null rather than absent, for §74.3: a consumer that has to discover which
            // keys this robot happened to fill in is one that gets it wrong on the robot that
            // filled in none.
            None => counters.set(name, Json::Null),
        };
    }

    let mut out = Json::obj();
    out.set("counters", counters)
        .set(
            "elapsed_ms",
            match window.elapsed_ms {
                Some(value) => Json::Num(value),
                None => Json::Null,
            },
        )
        .set(
            "from_ms",
            match window.from_ms {
                Some(value) => Json::Num(value),
                None => Json::Null,
            },
        )
        .set(
            "to_ms",
            match window.to_ms {
                Some(value) => Json::Num(value),
                None => Json::Null,
            },
        );
    out
}

/// Why a window was not queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotQueued {
    /// This robot is not paired, so there is nowhere to address one (§10).
    Unpaired,
    /// The window is not one the contract would count. Carries the first refusal.
    Unusable(String),
    /// The queue refused it. Carries the sentence.
    Refused(String),
}

/// Put a window in §34.4's queue, or say why not (§74).
///
/// **An unusable window is not queued**, and that is the one place this differs from the check-in,
/// which is enqueued whatever holes it has. The reason is the drop policy: a check-in is
/// level-triggered state that a fleet needs even when incomplete, and a telemetry window with no
/// duration is a row that can produce no rate and no coverage — so queueing it would spend the
/// budget of the class that is dropped first on an item that could never be worth anything.
///
/// An unpaired robot queues nothing, for [`crate::checkin::enqueue`]'s reason.
pub fn enqueue(
    queue: &Queue,
    layout: &Layout,
    window: &Window,
) -> Result<Option<String>, NotQueued> {
    if crate::identity::credential_for_sending(layout).is_none() {
        return Err(NotQueued::Unpaired);
    }

    let report = assess_window(window);
    if !report.usable {
        let first = report
            .faults
            .iter()
            .find(|fault| fault.severity == skynet_contracts::admission::Severity::Error)
            .map(|fault| fault.message.clone())
            .unwrap_or_default();
        return Err(NotQueued::Unusable(first));
    }

    let Ok(body) = window_to_json(window).to_canonical() else {
        return Err(NotQueued::Refused(
            "the window could not be serialised".to_string(),
        ));
    };

    match queue.enqueue(Class::Telemetry, &body) {
        Ok(enqueued) => Ok(Some(enqueued.id.clone())),
        Err(error) => Err(NotQueued::Refused(error.reason().to_string())),
    }
}

/// The open window, as an API answer, for `GET /telemetry` and the console.
///
/// Reports the window **being counted** rather than the last one sent, because on a robot this
/// edition runs the second is very often nothing and a route that answered `null` would leave an
/// operator unable to see whether their robot is counting at all.
pub fn to_json(
    counting: &Counting,
    wall_ms: f64,
    monotonic_ms: f64,
    dropped: u64,
    tally: Option<skynet_blackbox::tally::Tally>,
) -> Json {
    let window = counting.close(wall_ms, monotonic_ms, dropped, tally);
    let report = assess_window(&window);

    let mut out = Json::obj();
    out.set("open", window_to_json(&window)).set(
        "report",
        skynet_contracts::telemetry::window_report_to_json(&report),
    );

    // Why the ring-derived counters are absent, when they are — as a sentence, because the answer
    // is not "this edition cannot" but one of three specific things about this robot's recording,
    // and an operator looking at five nulls should not have to guess which.
    if let Some(note) = unmeasured_note(tally) {
        out.set("note", Json::Str(note.to_string()));
    }
    out
}

/// Why §74's ring-derived counters are not in the window, or `None` when they are.
///
/// Separate from [`to_json`] so the console and any future consumer name the reason the same way,
/// and so the three cases stay visibly distinct: no recorder, no command path, and a window the
/// recording no longer covers are three different things to go and fix.
pub fn unmeasured_note(tally: Option<skynet_blackbox::tally::Tally>) -> Option<&'static str> {
    match tally {
        None => Some(
            "§74's cycles, interventions, guard_trips, clamps and estops are counted off §13's \
             black box, and this robot's ring could not be read. They are reported unmeasured \
             rather than as zero (§74.3).",
        ),
        Some(tally) if !tally.commands => Some(
            "§74's cycles, interventions, guard_trips, clamps and estops are counted off §13's \
             black box, and this recording has no action channels: there is no control loop on \
             this robot, so §26's cycle channels have no producer. Zero here would be a claim \
             about a machine that has never commanded a joint, so they are reported unmeasured \
             (§74.3).",
        ),
        Some(tally) if !tally.complete() => Some(
            "§74's cycles, interventions, guard_trips, clamps and estops are counted off §13's \
             black box, and this window is no longer whole in it — the ring wrapped over it, or it \
             holds a gap. The counts would be a floor rather than a count, so they are reported \
             unmeasured and this window is one the Hub can see it does not have (§47.4, §74.1).",
        ),
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TempRoot;
    use skynet_blackbox::tally::Tally;

    /// A tally that may be reported: a ring with a command path, whole over the window.
    fn measured(
        cycles: u64,
        interventions: u64,
        guard_trips: u64,
        clamps: u64,
        estops: u64,
    ) -> Tally {
        Tally {
            cycles,
            interventions,
            guard_trips,
            clamps,
            estops,
            commands: true,
            ..Tally::default()
        }
    }

    #[test]
    fn a_closed_window_carries_what_the_loop_counted() {
        let mut counting = Counting::opened(1_000.0, 500.0, 7, 0);
        counting.passes = 12;
        counting.restarts = 3;
        let window = counting.close(301_000.0, 300_500.0, 9, None);

        assert_eq!(window.from_ms, Some(1_000.0));
        assert_eq!(window.elapsed_ms, Some(300_000.0));
        // Two drops happened inside the window, not the nine the queue has seen for its life.
        assert_eq!(window.counter("queue_dropped"), Some(2.0));
        assert_eq!(window.counter("reconcile_passes"), Some(12.0));
        // §33.5's supervisor, counted since the window opened rather than read off a total.
        assert_eq!(window.counter("package_restarts"), Some(3.0));
    }

    #[test]
    fn the_rings_five_counters_reach_the_window() {
        // §74's five, off §13's ring. The point of the whole arrangement: what the kernel did is
        // already written down, and this is the walk that counts it.
        let counting = Counting::opened(1_000.0, 500.0, 0, 1);
        let window = counting.close(
            301_000.0,
            300_500.0,
            0,
            Some(measured(30_000, 12, 19, 41, 1)),
        );

        assert_eq!(window.counter("cycles"), Some(30_000.0));
        assert_eq!(window.counter("interventions"), Some(12.0));
        assert_eq!(window.counter("guard_trips"), Some(19.0));
        assert_eq!(window.counter("clamps"), Some(41.0));
        assert_eq!(window.counter("estops"), Some(1.0));

        // And with all eight present, nothing in §74's set is unmeasured.
        let report = assess_window(&window);
        assert!(report.usable);
        assert!(report.unmeasured.is_empty(), "{:?}", report.unmeasured);
    }

    #[test]
    fn a_ring_with_no_command_path_leaves_the_five_absent_rather_than_zero() {
        // The edition that ships today. §74.3: zero and unmeasured are opposite facts, and a fleet
        // page showing "0 guard trips" over a robot that has never commanded a joint is the one
        // that gets misread.
        let counting = Counting::opened(1_000.0, 500.0, 0, 1);
        let window = counting.close(
            301_000.0,
            300_500.0,
            0,
            Some(Tally {
                commands: false,
                ..Tally::default()
            }),
        );

        let report = assess_window(&window);
        for name in ["cycles", "guard_trips", "clamps", "interventions", "estops"] {
            assert!(report.unmeasured.contains(&name.to_string()), "{name}");
        }
        // The three this loop keeps are still there. A robot with no control loop still reports
        // that it is up, reconciling and dropping nothing.
        assert!(
            !report.unmeasured.contains(&"reconcile_passes".to_string()),
            "{:?}",
            report.unmeasured
        );
    }

    #[test]
    fn a_window_the_ring_wrapped_over_is_reported_as_unmeasured_rather_than_small() {
        // The counts exist and are a floor. Reporting them would put a plausible small number where
        // a fleet needs a hole it can see — §47.4, and the reason §74.1 made windows tile.
        let counting = Counting::opened(1_000.0, 500.0, 0, 1);
        let window = counting.close(
            301_000.0,
            300_500.0,
            0,
            Some(Tally {
                cycles: 400,
                interventions: 3,
                commands: true,
                truncated: true,
                ..Tally::default()
            }),
        );

        assert_eq!(
            window.counter("cycles"),
            None,
            "a floor must not be sent as a count"
        );
        let report = assess_window(&window);
        assert!(report.unmeasured.contains(&"cycles".to_string()));
    }

    #[test]
    fn the_five_arrive_together_or_not_at_all() {
        // One walk of one recording produces all five, so a window carrying some of them would be
        // describing a ring that was partly readable — a state the ring does not have, and one that
        // would leave a consumer dividing a measured numerator by an absent denominator.
        let counting = Counting::opened(0.0, 0.0, 0, 1);
        let full = counting.close(1_000.0, 1_000.0, 0, Some(measured(10, 1, 1, 0, 0)));
        let empty = counting.close(1_000.0, 1_000.0, 0, None);

        let present = |window: &Window| {
            ["cycles", "interventions", "guard_trips", "clamps", "estops"]
                .iter()
                .filter(|name| window.counter(name).is_some())
                .count()
        };
        assert_eq!(present(&full), 5);
        assert_eq!(present(&empty), 0);
    }

    /// A quiet robot reports **zero** restarts, not an absence, and the two are the whole of §74.3.
    ///
    /// A window carrying no `package_restarts` used to mean *nothing here counts restarts*. It now
    /// means nothing else could mean it, so a robot that restarted nothing has to say so — and a
    /// fleet page filtering for churn has a robot to compare against.
    #[test]
    fn a_window_with_no_restarts_reports_zero_rather_than_nothing() {
        let window = Counting::opened(0.0, 0.0, 0, 0).close(1_000.0, 1_000.0, 0, None);
        assert_eq!(window.counter("package_restarts"), Some(0.0));
    }

    /// A robot that ran cycles and tripped nothing reports **zero** trips, which is the fact.
    ///
    /// The other half of §74.3, and the one this change makes reachable: with a command path and a
    /// whole window, "no guard tripped" is a measurement rather than an absence, and it is the
    /// number a certification's field suite needs to see.
    #[test]
    fn a_robot_that_tripped_nothing_says_so_rather_than_saying_nothing() {
        let window = Counting::opened(0.0, 0.0, 0, 1).close(
            1_000.0,
            1_000.0,
            0,
            Some(measured(500, 0, 0, 0, 0)),
        );
        assert_eq!(window.counter("guard_trips"), Some(0.0));
        assert_eq!(window.counter("interventions"), Some(0.0));
        assert_eq!(window.counter("cycles"), Some(500.0));
    }

    #[test]
    fn an_unpaired_robot_queues_nothing() {
        let root = TempRoot::new("telemetry-unpaired");
        let layout = root.layout();
        let queue = Queue::of(&layout);
        let window = Counting::opened(0.0, 0.0, 0, 0).close(1_000.0, 1_000.0, 0, None);
        assert_eq!(enqueue(&queue, &layout, &window), Err(NotQueued::Unpaired));
    }

    #[test]
    fn a_window_with_no_duration_is_refused_rather_than_queued() {
        // The one place this differs from a check-in, which is queued whatever holes it has. A
        // window with no duration can produce no rate and no coverage, so queueing it would spend
        // the budget of the class §34.4 drops first on an item that can never be worth anything.
        let root = TempRoot::new("telemetry-unusable");
        let layout = root.layout();
        let queue = Queue::of(&layout);
        let window = Counting::opened(0.0, 500.0, 0, 0).close(1_000.0, 500.0, 0, None);
        match enqueue(&queue, &layout, &window) {
            Err(NotQueued::Unpaired) => {}
            Err(NotQueued::Unusable(_)) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_open_window_is_reported_rather_than_the_last_one_sent() {
        let counting = Counting::opened(1_000.0, 0.0, 0, 0);
        let document = to_json(&counting, 61_000.0, 60_000.0, 3, None);
        assert_eq!(
            document
                .get("open")
                .and_then(|open| open.get("elapsed_ms"))
                .and_then(Json::as_f64),
            Some(60_000.0)
        );
        assert!(document
            .get("report")
            .and_then(|report| report.get("usable"))
            .and_then(Json::as_bool)
            .unwrap_or(false));
    }

    #[test]
    fn the_route_says_which_of_the_three_silences_this_is() {
        // Three different things to go and fix, and five nulls look identical. An operator reading
        // this route should not have to guess whether the disk failed, the robot has no control
        // loop, or the window fell out of the ring.
        assert!(unmeasured_note(None).unwrap().contains("could not be read"));
        assert!(unmeasured_note(Some(Tally::default()))
            .unwrap()
            .contains("no action channels"));
        assert!(unmeasured_note(Some(Tally {
            commands: true,
            truncated: true,
            ..Tally::default()
        }))
        .unwrap()
        .contains("no longer whole"));
        // And nothing at all when the five are in the window, because a note explaining an absence
        // that is not there is the kind of text a reader learns to skip.
        assert!(unmeasured_note(Some(measured(1, 0, 0, 0, 0))).is_none());
    }
}

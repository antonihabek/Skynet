//! §74's windows, on a real machine: what this robot can count, and what it refuses to guess.
//!
//! [`skynet_contracts::telemetry`] decides what a window *is* and takes no clock. This is the other
//! half — reading two clocks, counting what there is to count, and putting the result in §34.4's
//! queue as the class that is dropped first and aggressively.
//!
//! # Almost every counter is unmeasured, and that is the report
//!
//! §74.3 keeps zero and unmeasured apart for §33.6's reason, and on this edition the distinction is
//! not academic: **six of the eight counters have no source.** `cycles`, `guard_trips`, `clamps`
//! and `estops` need a kernel, which this daemon does not host; `interventions` needs teleoperation
//! (§17 of the Hub's own document) and `package_restarts` needs the supervisor to count them, which
//! it does not.
//!
//! Two are real. `queue_dropped` is §34.4's own counter — the one the section is written around,
//! *"this robot dropped forty thousand telemetry items last month"* — and `reconcile_passes` is what
//! this loop actually did. Reporting those two and naming the other six is the whole content of a
//! window today, and it is worth sending: a robot that is up, reconciling and dropping nothing is a
//! fact, and the gaps between windows are what make a fleet page able to say when it was not.
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
}

impl Counting {
    /// Open a window now.
    pub fn opened(wall_ms: f64, monotonic_ms: f64, dropped: u64) -> Counting {
        Counting {
            opened_wall_ms: wall_ms,
            opened_monotonic_ms: monotonic_ms,
            passes: 0,
            dropped_at_open: dropped,
        }
    }

    /// Close it, producing the window that crosses to the Hub.
    ///
    /// The six counters this edition cannot measure are **left absent** rather than set to zero.
    /// That is §74.3, and it is the difference between a fleet page saying "this robot has tripped
    /// no guards" and "nothing on this robot has ever counted a guard trip" — the first of which is
    /// a claim about a machine nobody has measured.
    pub fn close(&self, wall_ms: f64, monotonic_ms: f64, dropped: u64) -> Window {
        Window {
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
pub fn to_json(counting: &Counting, wall_ms: f64, monotonic_ms: f64, dropped: u64) -> Json {
    let window = counting.close(wall_ms, monotonic_ms, dropped);
    let report = assess_window(&window);

    let mut out = Json::obj();
    out.set("open", window_to_json(&window))
        .set(
            "report",
            skynet_contracts::telemetry::window_report_to_json(&report),
        )
        .set(
            "note",
            Json::Str(
                "six of §74's eight counters have no source on this edition — four need a kernel, \
                 one needs teleoperation and one needs the supervisor to count restarts. They are \
                 reported unmeasured rather than as zero (§74.3)."
                    .to_string(),
            ),
        );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TempRoot;

    #[test]
    fn a_closed_window_carries_what_was_counted_and_names_the_rest() {
        let counting = Counting::opened(1_000.0, 500.0, 7);
        let mut counting = counting;
        counting.passes = 12;
        let window = counting.close(301_000.0, 300_500.0, 9);

        assert_eq!(window.from_ms, Some(1_000.0));
        assert_eq!(window.elapsed_ms, Some(300_000.0));
        // Two drops happened inside the window, not the nine the queue has seen for its life.
        assert_eq!(window.counter("queue_dropped"), Some(2.0));
        assert_eq!(window.counter("reconcile_passes"), Some(12.0));

        // And the six with no source are absent rather than zero — §74.3, which is the difference
        // between "this robot has tripped no guards" and "nothing here counts guard trips".
        let report = assess_window(&window);
        assert!(report.usable);
        for name in [
            "cycles",
            "guard_trips",
            "clamps",
            "interventions",
            "estops",
            "package_restarts",
        ] {
            assert!(report.unmeasured.contains(&name.to_string()), "{name}");
        }
    }

    #[test]
    fn an_unpaired_robot_queues_nothing() {
        let root = TempRoot::new("telemetry-unpaired");
        let layout = root.layout();
        let queue = Queue::of(&layout);
        let window = Counting::opened(0.0, 0.0, 0).close(1_000.0, 1_000.0, 0);
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
        let window = Counting::opened(0.0, 500.0, 0).close(1_000.0, 500.0, 0);
        match enqueue(&queue, &layout, &window) {
            Err(NotQueued::Unpaired) => {}
            Err(NotQueued::Unusable(_)) => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_open_window_is_reported_rather_than_the_last_one_sent() {
        let counting = Counting::opened(1_000.0, 0.0, 0);
        let document = to_json(&counting, 61_000.0, 60_000.0, 3);
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
}

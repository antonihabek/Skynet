//! This process's own counters: passes, overruns, requests (§64).
//!
//! Every other number this daemon serves is about the robot — the queue, the units, the ring.
//! These are about the daemon itself: how many reconcile passes it has run, how many of those
//! overran their interval, how long the last one took, how many requests it has served. They
//! exist for the question *"is the supervisor well"* asked of a process rather than of a robot,
//! and `GET /metrics` serves them beside the queue and unit counters so one route answers it.
//!
//! # In memory, and reset by a restart
//!
//! A counter that survived a reboot would be a running total in disguise, and §74.1 rules those
//! out for the telemetry windows for the reason that applies here too: a drop becomes invisible
//! because the next total absorbs it. These reset with the process, and `uptime_ms` on the route
//! says what interval they cover. A fleet console joins them across restarts; the robot does not
//! pretend they are joinable by itself.
//!
//! # What counts as an overrun, and what does not
//!
//! A pass overruns when it took longer than the reconcile interval — the loop's one timing
//! contract (§33.2). Two exemptions, both deliberate. An interval of zero means *reconcile
//! continuously*, so the concept is meaningless and nothing is counted; and `once` has no
//! interval at all, so its single pass sets `last_pass_ms` and counts nothing. An overrun
//! counter that fired on either would be a number nobody could act on.

/// This process's own counters, kept on the daemon and served by `GET /metrics`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Metrics {
    /// Reconcile passes this process has run.
    pub passes: u64,
    /// Passes that took longer than the interval. See [`Metrics::record_pass`].
    pub pass_overruns: u64,
    /// How long the last pass took, in milliseconds. `None` until a pass has run — §33.6's
    /// rule, which applies to the daemon's own readings no less than the robot's: no pass yet
    /// and a pass that took nothing are opposite facts.
    pub last_pass_ms: Option<u64>,
    /// The reconcile interval, in milliseconds. Zero means no interval is in force — `once`,
    /// or a loop that has not set one — and overruns are then meaningless rather than zero.
    pub interval_ms: u64,
    /// API requests this process has served, on either listener.
    pub requests: u64,
}

impl Metrics {
    /// Record a finished pass. Returns whether it overran — the caller logs that, because a
    /// counter that silently incremented is a diagnosis nobody reads, and the line carries the
    /// two numbers the counter alone cannot hold.
    pub fn record_pass(&mut self, elapsed_ms: u64) -> bool {
        self.passes += 1;
        self.last_pass_ms = Some(elapsed_ms);
        // No interval, no overrun. An interval of zero is *reconcile continuously*, and `once`
        // never sets one — counting either would be a number nobody could act on.
        let overran = self.interval_ms > 0 && elapsed_ms > self.interval_ms;
        if overran {
            self.pass_overruns += 1;
        }
        overran
    }

    /// Record a served request.
    pub fn record_request(&mut self) {
        self.requests += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slow_pass_counts_and_a_fast_one_does_not() {
        let mut metrics = Metrics {
            interval_ms: 10_000,
            ..Metrics::default()
        };
        assert!(!metrics.record_pass(500));
        assert!(metrics.record_pass(10_001));
        assert_eq!(metrics.passes, 2);
        assert_eq!(metrics.pass_overruns, 1);
        assert_eq!(metrics.last_pass_ms, Some(10_001));
    }

    #[test]
    fn exactly_the_interval_is_not_an_overrun() {
        let mut metrics = Metrics {
            interval_ms: 10_000,
            ..Metrics::default()
        };
        assert!(!metrics.record_pass(10_000));
        assert_eq!(metrics.pass_overruns, 0);
    }

    #[test]
    fn without_an_interval_nothing_overruns() {
        let mut metrics = Metrics::default();
        assert!(!metrics.record_pass(u64::MAX));
        assert_eq!(metrics.passes, 1);
        assert_eq!(metrics.pass_overruns, 0);
        assert_eq!(metrics.last_pass_ms, Some(u64::MAX));
    }

    #[test]
    fn a_fresh_process_has_measured_nothing() {
        let metrics = Metrics::default();
        assert_eq!(metrics.passes, 0);
        assert_eq!(metrics.last_pass_ms, None);
    }
}

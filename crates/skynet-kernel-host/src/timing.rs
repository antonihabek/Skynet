//! What the loop's timing looked like, not only its worst moment (DRY-COVERAGE-PLAN.md 12.C.5).
//!
//! §46.3's first window is one number — the worst cycle the kernel has measured — and one number hides what a control engineer
//! asks about first: how often. A loop whose worst cycle is 0.4 ms once in a million and one whose worst is 0.4 ms every tenth
//! cycle report the same maximum. [`Histogram`] keeps the whole distribution in a fixed array, so recording a cycle is one
//! increment — no allocation, no lock, nothing that could itself be the outlier — and the report can say p50, p99, p99.9 and
//! p99.99 beside the worst.
//!
//! Two things are recorded each cycle: the **work** (what `elapsed` already measured: the step and its records, not the sleep),
//! and the **interval** between this cycle's start and the last one's, which is what a driver on the other end of the wire feels
//! as jitter and what divides into an effective rate. A loop asked for 1 kHz that turns at 940 Hz has a worst-case work time
//! that looks perfectly healthy.
//!
//! **What it is not:** a latency of any robot. The clock is the host's, the sample is this process's own view of itself, and a
//! percentile from a virtual machine is the host scheduler's as much as the guest's. It is a host-side budget, reported as one.

/// Sub-buckets per doubling: eight, so a value is reported at most 12.5% high.
const SUB: u32 = 8;
const SUB_SHIFT: u32 = 3;
/// Buckets: values below eight are their own bucket, then eight per power of two up to 2^40 ns (about eighteen minutes).
const BUCKETS: usize = (SUB as usize) * (41 - SUB_SHIFT as usize + 1);

/// A fixed-size log-linear histogram of nanosecond durations.
#[derive(Clone)]
pub struct Histogram {
    counts: [u64; BUCKETS],
    total: u64,
    worst: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram::new()
    }
}

impl Histogram {
    /// An empty histogram.
    pub const fn new() -> Histogram {
        Histogram {
            counts: [0; BUCKETS],
            total: 0,
            worst: 0,
        }
    }

    fn bucket(ns: u64) -> usize {
        if ns < SUB as u64 {
            return ns as usize;
        }
        let power = 63 - ns.leading_zeros();
        let power = power.min(40);
        let shift = power - SUB_SHIFT;
        let sub = if power == 40 && ns >= (1 << 41) {
            SUB as u64 - 1
        } else {
            (ns >> shift) & (SUB as u64 - 1)
        };
        (SUB as usize) + ((power - SUB_SHIFT) as usize) * (SUB as usize) + sub as usize
    }

    /// The largest value that falls in bucket `index`, which is what a quantile reports.
    fn upper(index: usize) -> u64 {
        if index < SUB as usize {
            return index as u64;
        }
        if index == BUCKETS - 1 {
            return u64::MAX; // everything past the last power of two is kept here, and is not above `worst`
        }
        let over = index - SUB as usize;
        let power = (over / SUB as usize) as u32 + SUB_SHIFT;
        let sub = (over % SUB as usize) as u64;
        let shift = power - SUB_SHIFT;
        (((SUB as u64) + sub + 1) << shift) - 1
    }

    /// Note one duration. One increment, and nothing that can fail.
    pub fn record(&mut self, ns: u64) {
        self.counts[Histogram::bucket(ns)] += 1;
        self.total += 1;
        if ns > self.worst {
            self.worst = ns;
        }
    }

    /// How many durations were recorded.
    pub fn count(&self) -> u64 {
        self.total
    }

    /// The largest duration recorded, exactly.
    pub fn worst(&self) -> u64 {
        self.worst
    }

    /// The duration below which `q` of the recorded ones fall (0 < q ≤ 1), at the top of its bucket, never above the worst
    /// actually seen. `None` when nothing was recorded.
    pub fn quantile(&self, q: f64) -> Option<u64> {
        if self.total == 0 {
            return None;
        }
        // The rank that must be covered: the smallest count that makes `q` of them, so p99.99 of a thousand cycles is the worst.
        let wanted = ((q * self.total as f64).ceil() as u64).clamp(1, self.total);
        let mut seen = 0u64;
        for (index, count) in self.counts.iter().enumerate() {
            seen += count;
            if seen >= wanted {
                return Some(Histogram::upper(index).min(self.worst));
            }
        }
        Some(self.worst)
    }
}

/// Fixed-rate pacing without catch-up: when the next cycle is due, taken from a schedule and not from how long the last one took.
///
/// The loop used to sleep `period - elapsed` after each cycle. That makes each cycle's length the work plus the period minus the
/// work plus however late the sleep woke — the period and the overshoot, every time — so a loop asked for a kilohertz turned at
/// 930 Hz on the machine that measured it (`cycle interval` against `loop rate` in `skynetk`'s report, which is how it was found).
/// A deadline that advances by one period each cycle absorbs the overshoot: a late wake-up shortens the next sleep instead of
/// lengthening the next cycle. And a deadline that has *passed* is not chased. A driver that is sent a burst of cycles to make up for
/// a stall receives N commands at once, which is F124's discontinuity produced on purpose; the schedule starts again from now, and
/// the stall is the `late starts` the report counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pacer {
    period_ns: u64,
    next_ns: u64,
}

impl Pacer {
    /// A schedule whose first deadline is one period after `start_ns`.
    pub fn new(period_ns: u64, start_ns: u64) -> Pacer {
        Pacer {
            period_ns,
            next_ns: start_ns + period_ns,
        }
    }

    /// At the end of a cycle, at `now_ns` on the same clock: how long to sleep before the next. Zero when the deadline has already
    /// passed — and then the schedule is re-anchored on `now_ns`, so the cycle after that is one period away and not none.
    pub fn after_cycle(&mut self, now_ns: u64) -> u64 {
        if now_ns < self.next_ns {
            let sleep = self.next_ns - now_ns;
            self.next_ns += self.period_ns;
            sleep
        } else {
            self.next_ns = now_ns + self.period_ns;
            0
        }
    }
}

/// The report's lines for one histogram: `p50 0.012 ms, p99 0.021 ms, p99.9 0.035 ms, p99.99 0.120 ms, worst 0.181 ms`.
pub fn summarise(histogram: &Histogram) -> Option<String> {
    let ms = |ns: u64| ns as f64 / 1_000_000.0;
    Some(format!(
        "p50 {:.3} ms, p99 {:.3} ms, p99.9 {:.3} ms, p99.99 {:.3} ms, worst {:.3} ms",
        ms(histogram.quantile(0.50)?),
        ms(histogram.quantile(0.99)?),
        ms(histogram.quantile(0.999)?),
        ms(histogram.quantile(0.9999)?),
        ms(histogram.worst()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `cycles` cycles of a loop that does `work_ns` of work and whose sleeps wake `overshoot_ns` late, on a clock this test owns,
    /// and return the time it ended at.
    fn simulate(pacer: &mut Pacer, cycles: u64, work_ns: u64, overshoot_ns: u64) -> u64 {
        let mut now = 0;
        for _ in 0..cycles {
            now += work_ns;
            let sleep = pacer.after_cycle(now);
            if sleep > 0 {
                now += sleep + overshoot_ns;
            }
        }
        now
    }

    #[test]
    fn a_late_wake_up_shortens_the_next_sleep_and_the_rate_is_the_one_asked_for() {
        // 1 kHz, 20 µs of work, every sleep waking 180 µs late: the loop that sleeps `period - work` runs at 1 / 1.18 ms.
        let period = 1_000_000;
        let mut pacer = Pacer::new(period, 0);
        let ended = simulate(&mut pacer, 10_000, 20_000, 180_000);
        let rate = 10_000.0 / (ended as f64 / 1e9);
        assert!(
            (995.0..=1005.0).contains(&rate),
            "{rate} Hz against 1000 asked"
        );
        let naive_period = 20_000 + (period - 20_000) + 180_000;
        assert!(
            naive_period > period * 11 / 10,
            "the old pacing's cycle was {naive_period} ns: the bug this replaces"
        );
    }

    #[test]
    fn a_stall_is_not_made_up_for_with_a_burst() {
        let period = 1_000_000;
        let mut pacer = Pacer::new(period, 0);
        let mut now = 0;
        for _ in 0..5 {
            now += 10_000;
            let sleep = pacer.after_cycle(now);
            now += sleep;
        }
        now += 10 * period; // descheduled for ten periods
        let first = pacer.after_cycle(now);
        assert_eq!(
            first, 0,
            "the deadline has passed, so there is nothing to wait for"
        );
        let mut sleeps = Vec::new();
        for _ in 0..5 {
            now += 10_000;
            let sleep = pacer.after_cycle(now);
            sleeps.push(sleep);
            now += sleep;
        }
        assert!(sleeps.iter().all(|s| *s > period / 2), "after the stall the loop is back on a period each, not running cycles back to back: {sleeps:?}");
    }

    #[test]
    fn work_longer_than_the_period_sleeps_not_at_all_and_starts_again_from_now() {
        let period = 1_000_000;
        let mut pacer = Pacer::new(period, 0);
        assert_eq!(pacer.after_cycle(1_500_000), 0);
        let next = pacer.after_cycle(1_600_000);
        assert_eq!(
            next,
            period - 100_000,
            "one period from the cycle that overran, less the work since"
        );
    }

    #[test]
    fn a_value_is_reported_at_most_an_eighth_high_and_never_low() {
        for ns in [
            0u64,
            1,
            7,
            8,
            9,
            15,
            16,
            100,
            999,
            1_000,
            123_456,
            1_000_000,
            987_654_321,
            1 << 39,
        ] {
            let index = Histogram::bucket(ns);
            let top = Histogram::upper(index);
            assert!(top >= ns, "{ns} fell in a bucket whose top is {top}");
            assert!(
                top as f64 <= ns as f64 * 1.125 + 1.0,
                "{ns} was reported as {top}, which is more than an eighth high"
            );
        }
    }

    #[test]
    fn buckets_are_in_order_and_do_not_overlap() {
        let mut last = 0;
        for ns in (0..100_000u64).chain((100_000..10_000_000).step_by(997)) {
            let index = Histogram::bucket(ns);
            assert!(index >= last, "{ns} went backwards");
            last = index;
        }
    }

    #[test]
    fn quantiles_come_from_the_ranks() {
        let mut histogram = Histogram::new();
        assert_eq!(
            histogram.quantile(0.5),
            None,
            "nothing recorded says nothing"
        );
        for _ in 0..9_990 {
            histogram.record(100_000); // a tenth of a millisecond
        }
        for _ in 0..9 {
            histogram.record(400_000);
        }
        histogram.record(9_000_000); // one slow cycle in ten thousand
        assert_eq!(histogram.count(), 10_000);
        let p50 = histogram.quantile(0.50).unwrap();
        assert!((100_000..=112_500).contains(&p50), "p50 {p50}");
        let p999 = histogram.quantile(0.999).unwrap();
        assert!(
            (100_000..=112_500).contains(&p999),
            "p99.9 {p999}: the 9,990th of 10,000 is still an ordinary cycle"
        );
        let p9999 = histogram.quantile(0.9999).unwrap();
        assert!(
            (400_000..=450_000).contains(&p9999),
            "p99.99 {p9999}: the 9,999th is the last of the nine 0.4 ms cycles"
        );
        assert_eq!(
            histogram.quantile(1.0).unwrap(),
            9_000_000,
            "and the whole of it is the one slow cycle, never above the worst"
        );
        assert_eq!(histogram.worst(), 9_000_000);
    }

    #[test]
    fn a_thousand_cycles_cannot_say_p99_99_is_anything_but_the_worst() {
        let mut histogram = Histogram::new();
        for _ in 0..999 {
            histogram.record(50_000);
        }
        histogram.record(700_000);
        assert_eq!(histogram.quantile(0.9999), Some(700_000));
    }

    #[test]
    fn enormous_values_are_kept_in_the_last_bucket_and_counted() {
        let mut histogram = Histogram::new();
        histogram.record(u64::MAX);
        histogram.record(1 << 50);
        assert_eq!(histogram.count(), 2);
        assert_eq!(histogram.worst(), u64::MAX);
        assert!(histogram.quantile(1.0).is_some());
    }

    #[test]
    fn the_summary_is_one_line_with_the_whole_distribution() {
        let mut histogram = Histogram::new();
        for _ in 0..100 {
            histogram.record(250_000);
        }
        let line = summarise(&histogram).unwrap();
        assert!(
            line.starts_with("p50 0.2")
                && line.contains("p99.99 0.250")
                && line.ends_with("worst 0.250 ms"),
            "{line}"
        );
        assert_eq!(summarise(&Histogram::new()), None);
    }
}

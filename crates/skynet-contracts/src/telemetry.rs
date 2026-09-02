//! §74 — telemetry, as windows rather than samples.
//!
//! The Rust edition of `reference/typescript/telemetry.ts`. Read that file for the design; this one
//! records the port.
//!
//! §34.4 gave telemetry a priority, a drop policy, a counter and a paragraph of its own, and neither
//! §17's eight endpoints nor §57's nine gave it a route. §74 adds the tenth, and this is what
//! crosses it.
//!
//! Two properties decide the shape and both come from rules that already existed:
//!
//! * **A window, not a sample and not a total.** A running total absorbs a drop and leaves a tidy
//!   series, which is what §47.4 forbids; a series of samples produces ten thousand items after a
//!   week offline, which is what §34.4 collapses check-ins to avoid. Counts over a self-describing
//!   interval have neither property, and they make a drop *visible* — the windows stop tiling.
//! * **Two clocks.** §35.1 assigns them: wall time for correlation with the Hub, monotonic for
//!   anything measured. Using wall time as a denominator would let an NTP step turn four hundred
//!   guard trips into a plausible rate with nothing looking wrong.
//!
//! # Pure
//!
//! No clock, no network. Both bounds and the elapsed monotonic time arrive as numbers, so the same
//! functions run on the robot that produced a window and on a Hub deciding what a set of them
//! covers.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

/// What a window may count.
///
/// A closed set for `RECONCILE_STAGES`' reason: a fleet console divides by these, and six spellings
/// of one counter is six columns that each look empty.
pub const TELEMETRY_COUNTERS: [&str; 8] = [
    "cycles",
    "guard_trips",
    "clamps",
    "interventions",
    "estops",
    "package_restarts",
    "queue_dropped",
    "reconcile_passes",
];

/// One counter over a window.
///
/// `None` is **not measured** and `Some(0.0)` is measured-and-zero. §33.6's reason for keeping them
/// apart applies here with more force: "no measurement of guard trips" and "no guard trips" render
/// identically as a zero, and the second is the number an operator relaxes on.
pub type Counted = Option<f64>;

/// One telemetry window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Window {
    /// Wall-clock start, milliseconds since the epoch. §35.1's correlation clock.
    pub from_ms: Option<f64>,
    /// Wall-clock end, exclusive.
    pub to_ms: Option<f64>,
    /// Monotonic milliseconds actually elapsed. §35.1's measurement clock, and every rate's
    /// denominator.
    pub elapsed_ms: Option<f64>,
    /// `(name, count)`, in [`TELEMETRY_COUNTERS`] order. A name absent here is unmeasured.
    pub counters: Vec<(String, Counted)>,
}

impl Window {
    /// The count for one name, or `None` when this window did not measure it.
    pub fn counter(&self, name: &str) -> Counted {
        self.counters
            .iter()
            .find(|(entry, _)| entry == name)
            .and_then(|(_, count)| *count)
    }

    /// Set one counter, replacing any previous value for the same name.
    pub fn with(mut self, name: &str, count: Counted) -> Window {
        self.counters.retain(|(entry, _)| entry != name);
        self.counters.push((name.to_string(), count));
        self
    }
}

/// Why a window cannot be counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowFaultCode {
    /// It does not say when it was.
    Unbounded,
    /// It ends before it starts.
    Inverted,
    /// It carries no monotonic duration, so nothing can be divided by it.
    UnmeasuredDuration,
    /// The two clocks disagree by more than a minute.
    ImplausibleDuration,
}

impl WindowFaultCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            WindowFaultCode::Unbounded => "unbounded",
            WindowFaultCode::Inverted => "inverted",
            WindowFaultCode::UnmeasuredDuration => "unmeasured_duration",
            WindowFaultCode::ImplausibleDuration => "implausible_duration",
        }
    }
}

/// Every window fault, in the order the checks run.
pub const WINDOW_FAULTS: [WindowFaultCode; 4] = [
    WindowFaultCode::Unbounded,
    WindowFaultCode::Inverted,
    WindowFaultCode::UnmeasuredDuration,
    WindowFaultCode::ImplausibleDuration,
];

/// One finding about a window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowFault {
    /// What.
    pub code: WindowFaultCode,
    /// Whether it stops the window being counted.
    pub severity: crate::admission::Severity,
    /// A sentence naming the fix (§64).
    pub message: String,
}

/// What one window says.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowReport {
    /// Whether this window may be counted at all.
    pub usable: bool,
    /// Monotonic milliseconds the rates are over.
    pub elapsed_ms: f64,
    /// `(to − from) − elapsed`. Positive is wall time running ahead. Reported, never corrected.
    pub skew_ms: f64,
    /// `(name, per-second rate)` for every measured counter, in [`TELEMETRY_COUNTERS`] order.
    pub rates: Vec<(String, f64)>,
    /// Counters nobody measured, sorted.
    pub unmeasured: Vec<String>,
    /// Everything found.
    pub faults: Vec<WindowFault>,
}

fn fault(
    code: WindowFaultCode,
    severity: crate::admission::Severity,
    message: String,
) -> WindowFault {
    WindowFault {
        code,
        severity,
        message,
    }
}

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|number| number.is_finite())
}

/// What one window says, and whether it says anything (§74.2, §74.3).
///
/// Every check runs and the result carries all of them, for [`crate::admission::admit_package`]'s
/// reason: an operator looking at a robot whose telemetry is being refused should get one list
/// rather than one problem per window.
pub fn assess_window(window: &Window) -> WindowReport {
    use crate::admission::Severity;

    let mut faults = Vec::new();
    let from = finite(window.from_ms);
    let to = finite(window.to_ms);
    let elapsed = finite(window.elapsed_ms);

    let bounded = from.is_some() && to.is_some();
    if !bounded {
        faults.push(fault(
            WindowFaultCode::Unbounded,
            Severity::Error,
            "This window does not say when it was. §35.1 makes wall time the clock a Hub correlates \
             on, so a window without both bounds cannot be placed beside another robot's — and the \
             gap between windows is how a dropped one is noticed at all (§34.4, §47.4)."
                .to_string(),
        ));
    } else if to < from {
        faults.push(fault(
            WindowFaultCode::Inverted,
            Severity::Error,
            "This window ends before it starts. That is a clock that stepped backwards mid-window \
             rather than a counting error, and the counts in it are over an interval nobody can \
             name."
                .to_string(),
        ));
    }

    let measured = elapsed.filter(|value| *value > 0.0);
    if measured.is_none() {
        faults.push(fault(
            WindowFaultCode::UnmeasuredDuration,
            Severity::Error,
            "This window carries no monotonic duration, so there is nothing to divide the counts \
             by. §35.1 keeps rates off wall time on purpose: an NTP step would otherwise turn four \
             hundred guard trips into a plausible number and nothing would look wrong."
                .to_string(),
        ));
    }

    let skew = match (from, to, measured) {
        (Some(from), Some(to), Some(elapsed)) => to - from - elapsed,
        _ => 0.0,
    };

    // A minute of disagreement over a five-minute window is a machine that was suspended, an NTP
    // correction, or a clock nobody set. A fact about the robot, not a reason to discard counts.
    //
    // Compared without `f64::abs`, which is not available under `no_std` — and the seconds in the
    // message are **truncated toward zero** rather than rounded, because `Math.round` is half
    // toward positive infinity and [`crate::round_half_away`] is half away from zero, and the two
    // disagree on exactly the negative halves a backwards clock produces. Truncation is the one
    // reading both editions can spell identically without a helper.
    // The two comparisons are kept rather than folded into a range, which is the same expression
    // for every number and a different one for `NaN`: a range says *not contained* and raises the
    // fault, two comparisons say *neither greater nor less* and do not. The reference edition
    // spells it with two comparisons, so this one does too — an edition difference reachable only
    // by a value JSON cannot carry is still an edition difference, and the corpus exists to have
    // none.
    #[allow(clippy::manual_range_contains)]
    if skew > 60_000.0 || skew < -60_000.0 {
        faults.push(fault(
            WindowFaultCode::ImplausibleDuration,
            Severity::Warning,
            format!(
                "Wall time and the monotonic clock disagree by {} s over this window. The counts \
                 are kept and the rates are computed from the monotonic duration (§35.1); the \
                 disagreement is worth one line on a fleet page, because it is what a suspended \
                 machine and a corrected clock both look like.",
                (skew / 1000.0) as i64
            ),
        ));
    }

    let mut rates = Vec::new();
    let mut unmeasured = Vec::new();
    for name in TELEMETRY_COUNTERS {
        match window.counter(name) {
            Some(count) if count.is_finite() && count >= 0.0 => {
                if let Some(elapsed) = measured {
                    rates.push((name.to_string(), count / (elapsed / 1000.0)));
                }
            }
            _ => unmeasured.push(name.to_string()),
        }
    }
    unmeasured.sort();

    WindowReport {
        usable: faults.iter().all(|entry| entry.severity != Severity::Error),
        elapsed_ms: measured.unwrap_or(0.0),
        skew_ms: skew,
        rates,
        unmeasured,
        faults,
    }
}

/// An interval no window covers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gap {
    /// Where the coverage stopped.
    pub from_ms: f64,
    /// Where it resumed.
    pub to_ms: f64,
}

/// What a set of windows actually covers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Coverage {
    /// Monotonic milliseconds actually observed, over usable windows.
    pub observed_ms: f64,
    /// Wall-clock milliseconds between the earliest start and the furthest end.
    pub spanned_ms: f64,
    /// Intervals inside the span that no window covers, in order.
    pub gaps: Vec<Gap>,
    /// How many windows [`assess_window`] refused.
    pub unusable: usize,
    /// `(name, total)` over usable windows that measured it, in [`TELEMETRY_COUNTERS`] order.
    pub totals: Vec<(String, f64)>,
    /// Counters no usable window measured, sorted.
    pub unmeasured: Vec<String>,
}

impl Coverage {
    /// The total for one counter, or `None` when nothing measured it.
    pub fn total(&self, name: &str) -> Option<f64> {
        self.totals
            .iter()
            .find(|(entry, _)| entry == name)
            .map(|(_, total)| *total)
    }
}

/// What a set of windows covers (§74.1, §74.5).
///
/// The function §15's field conformance has to be computed through, for one reason: **the
/// denominator is the time that was observed, not the time that elapsed.** A robot that sent windows
/// for one hour out of a day has demonstrated nothing about the other twenty-three, and §15 puts a
/// floor of 0.999 on that figure and suspends an award below it — so the difference between the two
/// denominators is the difference between suspending a robot and not.
pub fn coverage(windows: &[Window]) -> Coverage {
    let mut usable: Vec<(f64, f64, &Window)> = Vec::new();
    let mut unusable = 0usize;
    let mut observed = 0.0f64;

    for window in windows {
        let report = assess_window(window);
        if !report.usable {
            unusable += 1;
            continue;
        }
        observed += report.elapsed_ms;
        usable.push((
            window.from_ms.unwrap_or_default(),
            window.to_ms.unwrap_or_default(),
            window,
        ));
    }

    let mut totals: Vec<(String, f64)> = Vec::new();
    for name in TELEMETRY_COUNTERS {
        let mut sum = 0.0f64;
        let mut seen = false;
        for (_, _, window) in &usable {
            if let Some(count) = window.counter(name) {
                if count.is_finite() && count >= 0.0 {
                    sum += count;
                    seen = true;
                }
            }
        }
        if seen {
            totals.push((name.to_string(), sum));
        }
    }
    let mut unmeasured: Vec<String> = TELEMETRY_COUNTERS
        .iter()
        .filter(|name| !totals.iter().any(|(entry, _)| entry == *name))
        .map(|name| (*name).to_string())
        .collect();
    unmeasured.sort();

    usable.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(core::cmp::Ordering::Equal));

    let mut gaps = Vec::new();
    let mut spanned = 0.0f64;
    if let Some((first_from, first_to, _)) = usable.first().copied() {
        let mut reach = first_to;
        for (from, to, _) in usable.iter().skip(1) {
            if *from > reach {
                gaps.push(Gap {
                    from_ms: reach,
                    to_ms: *from,
                });
            }
            if *to > reach {
                reach = *to;
            }
        }
        let width = reach - first_from;
        spanned = if width > 0.0 { width } else { 0.0 };
    }

    Coverage {
        observed_ms: observed,
        spanned_ms: spanned,
        gaps,
        unusable,
        totals,
        unmeasured,
    }
}

/// §15's field conformance, over what was actually observed.
///
/// `None` when nothing counted cycles, which is not the same as 1.0 and is what
/// [`crate::certification::award_certification`] takes for *"no fleet data exists yet"*. A Hub that
/// substituted 1.0 here would be awarding a certified tier on the strength of a robot that has never
/// reported.
pub fn conformance_of(coverage: &Coverage) -> Option<f64> {
    let cycles = coverage.total("cycles")?;
    if !cycles.is_finite() {
        return None;
    }
    // Absent interventions read as zero *only once there are cycles*, and that is not the usual
    // rule. §74.3 keeps unmeasured and zero apart everywhere else — but a window that counted
    // cycles and did not count interventions is a robot whose kernel intervened in none of them,
    // because the counter that would have recorded one is the same counter that recorded the
    // cycles. Absent `cycles` is still absent, which is the guard above.
    let interventions = coverage.total("interventions").unwrap_or(0.0);
    // **Through `field_conformance`, not beside it.** These two functions computed the same ratio,
    // with the same clamp, in two crates, and neither had a caller — so the day one of them gained
    // a rule about a robot whose counters disagree, the other would have kept the old answer and
    // nothing would have compared them. §15's number has one definition and it belongs to the
    // module that checks it against §15's floor.
    crate::certification::field_conformance(cycles, interventions)
}

/// One window report, as the corpus holds it.
pub fn window_report_to_json(report: &WindowReport) -> Json {
    let mut faults = Json::arr();
    for entry in &report.faults {
        let mut row = Json::obj();
        row.set("code", Json::Str(entry.code.as_str().to_string()))
            .set("message", Json::Str(entry.message.clone()))
            .set("severity", Json::Str(entry.severity.as_str().to_string()));
        faults.push(row);
    }

    let mut rates = Json::obj();
    for (name, rate) in &report.rates {
        rates.set(name, Json::Num(*rate));
    }

    let mut out = Json::obj();
    out.set("elapsed_ms", Json::Num(report.elapsed_ms))
        .set("faults", faults)
        .set("rates", rates)
        .set("skew_ms", Json::Num(report.skew_ms))
        .set(
            "unmeasured",
            Json::Arr(
                report
                    .unmeasured
                    .iter()
                    .map(|name| Json::Str(name.clone()))
                    .collect(),
            ),
        )
        .set("usable", Json::Bool(report.usable));
    out
}

/// One coverage, as the corpus holds it.
pub fn coverage_to_json(coverage: &Coverage) -> Json {
    let mut gaps = Json::arr();
    for gap in &coverage.gaps {
        let mut row = Json::obj();
        row.set("from_ms", Json::Num(gap.from_ms))
            .set("to_ms", Json::Num(gap.to_ms));
        gaps.push(row);
    }

    let mut totals = Json::obj();
    for (name, total) in &coverage.totals {
        totals.set(name, Json::Num(*total));
    }

    let mut out = Json::obj();
    out
        // §15's third suite, over what was actually observed. Nullable rather than optional, and
        // frozen here rather than left to each consumer, because this is the number that decides
        // whether a certificate is suspended — and it had two implementations, in two crates,
        // pinned by nothing. A value both editions compute and neither corpus holds is a value
        // they are free to disagree about.
        .set(
            "conformance",
            match conformance_of(coverage) {
                Some(ratio) => Json::Num(ratio),
                None => Json::Null,
            },
        )
        .set("gaps", gaps)
        .set("observed_ms", Json::Num(coverage.observed_ms))
        .set("spanned_ms", Json::Num(coverage.spanned_ms))
        .set("totals", totals)
        .set(
            "unmeasured",
            Json::Arr(
                coverage
                    .unmeasured
                    .iter()
                    .map(|name| Json::Str(name.clone()))
                    .collect(),
            ),
        )
        .set("unusable", Json::Num(coverage.unusable as f64));
    out
}

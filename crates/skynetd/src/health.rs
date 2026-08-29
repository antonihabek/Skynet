//! Liveness, readiness, and the difference between them (§33.6).
//!
//! > A policy that is alive but not ready is **worse** than one that has died, because a dead
//! > process is obvious and a live-but-stalled one looks fine on every dashboard while the robot
//! > stands still.
//!
//! * **Liveness** — the process exists. Cheap, and nearly worthless on its own.
//! * **Readiness** — the process is producing commands at its declared rate, with an advancing
//!   sequence number.
//!
//! The watchdog catches a stalled producer in the *control* path. Readiness is what catches it in
//! the **reporting** path, so an operator sees "running, not producing" rather than a green tick.
//!
//! # Every field carries its age
//!
//! Literally, here: [`PackageHealth::age_ms`] is computed against the observation instant rather
//! than assumed fresh. A fleet console showing a robot's guard trip rate without saying it is four
//! hours old is a console that will eventually be used to make a decision about a robot that is not
//! in the state shown.
//!
//! And a health field nobody has measured is **absent**, never zero. Zero guard trips and no
//! measurement of guard trips are opposite facts: one says the kernel intervened in nothing, the
//! other says nobody looked.

/// What a package's command production looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Producing at the declared rate with an advancing sequence.
    Ready,
    /// Alive and not producing. The state that looks fine on a dashboard and is not.
    Stalled,
    /// Not running, so the question does not apply.
    NotRunning,
    /// Running, and nothing has reported a sequence yet.
    ///
    /// Distinct from `Stalled`: a policy in its first cycle has legitimately produced nothing, and
    /// reporting that as a stall would make every start look like a fault.
    Unknown,
}

/// One package's health.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageHealth {
    /// Package id.
    pub id: String,
    /// Whether the process exists.
    pub live: bool,
    /// Whether it is producing.
    pub readiness: Readiness,
    /// The last command sequence seen from it, when one has been seen.
    pub last_sequence: Option<u64>,
    /// How stale the reading is, in milliseconds.
    pub age_ms: u64,
}

/// Aggregate health, as reported at check-in and rendered on the console (§33.6).
///
/// Every field is either reported or explicitly absent. Nothing on this structure is inferred, and
/// nothing defaults to a flattering value — the same rule `fieldConformance()` follows when it
/// returns nothing rather than 1.0 for a robot that reported no cycles.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Health {
    /// Per package.
    pub packages: Vec<PackageHealth>,
    /// Generation lag against the Hub: desired minus applied. `None` when the Hub has not been
    /// reached, which is not a failure (§34.1) and is a different fact from a lag of zero.
    pub generation_lag: Option<u64>,
    /// Fraction of the disk still free, 0–1.
    pub storage_headroom: Option<f64>,
    /// Guard trips per thousand cycles over the reporting window.
    pub guard_trip_rate: Option<f64>,
    /// Battery state of charge, 0–1.
    pub battery: Option<f64>,
    /// Hottest thermal zone, degrees Celsius.
    pub temperature_c: Option<f64>,
}

impl Health {
    /// Whether anything here should stop a robot being handed new work.
    ///
    /// Deliberately narrow: a stalled policy and a failed one both count, and an unmeasured field
    /// does **not**. "We could not measure the battery" is not "the battery is fine", and it is
    /// also not "the robot is unhealthy" — it is a gap, and reporting a gap as a fault is how an
    /// operator learns to ignore the health screen.
    pub fn is_degraded(&self) -> bool {
        self.packages
            .iter()
            .any(|package| package.readiness == Readiness::Stalled)
    }

    /// Packages that are alive and producing nothing — the finding §33.6 exists for.
    pub fn stalled(&self) -> impl Iterator<Item = &PackageHealth> {
        self.packages
            .iter()
            .filter(|package| package.readiness == Readiness::Stalled)
    }
}

/// What one package reported about itself since the last observation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Production {
    /// The last command sequence, when one has been seen.
    pub last_sequence: Option<u64>,
    /// When that sequence was seen, monotonic milliseconds.
    pub seen_at_ms: u64,
    /// The rate the package declared it would run at.
    pub declared_hz: Option<f64>,
}

/// Decide readiness from what a package produced.
///
/// The rule, and the reason it is a multiple of the declared period rather than a constant: a
/// policy declaring 10 Hz and one declaring 1 kHz are stalled at very different absolute ages, and
/// a fixed threshold would either call the slow one stalled continuously or let the fast one sit
/// dead for a hundred cycles before anybody noticed.
///
/// A package that declared no rate cannot be judged, and reports [`Readiness::Unknown`] rather than
/// being assumed ready. That is the honest answer and it is also a nudge: §8.1 makes the budget
/// mandatory for exactly this reason, and a rate nobody declared is a rate nobody can hold anyone
/// to.
pub fn readiness(
    live: bool,
    production: Production,
    now_ms: u64,
    tolerance_periods: f64,
) -> Readiness {
    if !live {
        return Readiness::NotRunning;
    }
    let Some(_) = production.last_sequence else {
        return Readiness::Unknown;
    };
    let Some(hz) = production.declared_hz.filter(|hz| *hz > 0.0) else {
        return Readiness::Unknown;
    };

    let period_ms = 1000.0 / hz;
    let age = now_ms.saturating_sub(production.seen_at_ms) as f64;
    if age > period_ms * tolerance_periods {
        Readiness::Stalled
    } else {
        Readiness::Ready
    }
}

/// How many declared periods a package may miss before it is called stalled.
///
/// Three: one for the period itself, one for scheduling noise, and one so a single missed cycle on
/// a busy machine is not an incident. Below that a healthy robot reports faults; far above it the
/// signal arrives after the watchdog has already stopped the robot, which makes it a worse
/// dashboard than none.
pub const DEFAULT_TOLERANCE_PERIODS: f64 = 3.0;

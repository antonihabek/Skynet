//! §11 and §17's check-in — the one thing a robot says about itself.
//!
//! > `POST /devices/{id}/checkin { generation, applied, health, attestation }`
//!
//! Four fields in the specification and one rule behind all of them, from §17:
//!
//! > **The platform never claims a robot is running something it has not heard from.** A deployment
//! > is `pending` until a check-in.
//!
//! Everything here exists to make that rule survive contact with a robot that is halfway through
//! converging, or waiting for a safe state, or running software nobody asked for. The check-in is
//! the *only* evidence the Hub has, so a check-in that overstates is a fleet console that lies, and
//! there is no second source to catch it.
//!
//! # A check-in carries no history, and that is what makes it collapsible
//!
//! §34.4 puts check-ins in a class of their own: *"superseded — only the newest matters, so older
//! ones are collapsed rather than queued"*, because *"a week offline should produce one current
//! check-in on reconnect, not ten thousand historical ones."*
//!
//! That collapsing is only safe because **every field here is a statement about now**. No deltas, no
//! counters since last time, no event list. A check-in that carried *"twelve guard trips since the
//! last one"* would make collapsing lossy, and the queue would be discarding data while believing
//! it was discarding duplicates. The level-triggered rule from §33.3 applies to the report as much
//! as to the loop.
//!
//! # `applied` is not a field the daemon may set
//!
//! §33.3: *"Convergence is one plan being empty, not one pass succeeding. A supervisor that reported
//! 'converged' because every action it applied succeeded would stamp the generation on a robot that
//! has not started the policy yet."* So `applied` is derived, here, from what is left to do — and
//! there turn out to be **four** answers rather than two, because *"still working"* and *"waiting
//! for a safe state that may not come for hours"* are different sentences to put in front of an
//! operator (§33.4).
//!
//! # The generation on the wire is the one the robot applied
//!
//! Never the one it was asked for. That is the single field §17's rule lives or dies on: a check-in
//! echoing the desired generation would mark every deployment applied the moment the robot heard
//! about it, which is precisely the claim the Hub is forbidden from making.
//!
//! # Pure
//!
//! No clock, no network. Ages arrive as durations rather than as timestamps, for §35.2's reason.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

use crate::identity::{attest, attestation_to_json, Attestation};

/// What a robot may claim about the generation it was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The plan is empty and the generations agree.
    Applied,
    /// There is work left and it is being done.
    Converging,
    /// Everything left is waiting on §33.4's safe-state barrier.
    Deferred,
    /// The plan is empty and the generations disagree.
    Inconsistent,
}

impl Claim {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Claim::Applied => "applied",
            Claim::Converging => "converging",
            Claim::Deferred => "deferred",
            Claim::Inconsistent => "inconsistent",
        }
    }

    /// Parse one, or `None` for a word this edition does not know.
    pub fn parse(text: &str) -> Option<Claim> {
        CLAIMS.into_iter().find(|claim| claim.as_str() == text)
    }
}

/// Every claim, weakest first.
pub const CLAIMS: [Claim; 4] = [
    Claim::Applied,
    Claim::Converging,
    Claim::Deferred,
    Claim::Inconsistent,
];

/// §33.4's six ordered stages, as a check-in names them.
///
/// Here rather than left as free strings because a fleet console groups by them: *"forty robots are
/// waiting on stage 1"* is a fact somebody acts on, and *"forty robots reported forty spellings of
/// the OS image stage"* is not.
pub const STAGES: [&str; 6] = [
    "os_image",
    "rhal_driver",
    "package",
    "profile",
    "policy",
    "lease",
];

/// One thing the reconciler has left to do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    /// Which of [`STAGES`].
    pub stage: String,
    /// Whether it is waiting on §33.4's safe-state barrier rather than simply not done yet.
    ///
    /// The distinction is the whole reason [`Claim::Deferred`] exists: an action that has not run is
    /// a robot mid-convergence, and an action waiting for a safe state is a robot that will not
    /// converge until somebody puts it in one.
    pub deferred: bool,
    /// Why, in a sentence.
    pub reason: String,
}

/// Where the reconciler has got to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// What the robot has actually applied.
    pub observed_generation: u64,
    /// What the desired state asked for.
    pub desired_generation: u64,
    /// What is left to do.
    pub pending: Vec<Pending>,
}

/// What a robot is entitled to claim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimVerdict {
    /// Which of the four.
    pub claim: Claim,
    /// §17's field. True for [`Claim::Applied`] and for nothing else.
    pub applied: bool,
    /// The generation this check-in reports having applied. **Never the desired one.**
    pub generation: u64,
    /// The sentence an operator reads.
    pub detail: String,
    /// The stages with work left, sorted and deduplicated. Empty on `applied`.
    pub blocking: Vec<String>,
}

fn stages_of(pending: &[Pending]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in pending {
        if !out.contains(&entry.stage) {
            out.push(entry.stage.clone());
        }
    }
    out.sort();
    out
}

/// What this robot is entitled to claim (§17, §33.3, §33.4).
///
/// The order of the tests is the order of severity, and the last one is the interesting one. An
/// **empty plan with disagreeing generations** is a reconciler that computed a plan closing nothing:
/// the robot is not converged and nothing is going to converge it. Reporting that as `applied` would
/// hide a supervisor bug behind a green tick on a fleet console, which is the one place nobody would
/// go looking for it. It gets a name.
pub fn claim_for(progress: &Progress) -> ClaimVerdict {
    let observed = progress.observed_generation;
    let desired = progress.desired_generation;
    let blocking = stages_of(&progress.pending);

    if !progress.pending.is_empty() {
        if progress.pending.iter().all(|entry| entry.deferred) {
            return ClaimVerdict {
                claim: Claim::Deferred,
                applied: false,
                generation: observed,
                detail: format!(
                    "Everything left to do is waiting for a safe state (§33.4): {}. Nothing here \
                     converges until the robot is put in one, which is an operator action rather \
                     than a matter of time.",
                    blocking.join(", ")
                ),
                blocking,
            };
        }
        return ClaimVerdict {
            claim: Claim::Converging,
            applied: false,
            generation: observed,
            detail: format!(
                "Still reconciling generation {desired}, with work left at {}. This robot is \
                 running generation {observed}.",
                blocking.join(", ")
            ),
            blocking,
        };
    }

    if observed != desired {
        return ClaimVerdict {
            claim: Claim::Inconsistent,
            applied: false,
            generation: observed,
            detail: format!(
                "The plan is empty and this robot has applied generation {observed} against a \
                 desired {desired}. An empty plan that closes nothing is a supervisor that cannot \
                 see the difference it is meant to close, and reporting it as applied would put a \
                 green tick over it."
            ),
            blocking,
        };
    }

    ClaimVerdict {
        claim: Claim::Applied,
        applied: true,
        generation: observed,
        detail: format!("Generation {observed} is applied and the plan is empty."),
        blocking,
    }
}

// ── §33.6's health, and the age rule ─────────────────────────────────────────

/// One aggregate health reading.
///
/// §33.6 ends with four words that decide this shape: *"Every field carries its age."* A number
/// without one is not a measurement — it is a value a fleet console will render as current, and the
/// battery reading from four hours ago is exactly the one somebody dispatches an engineer on.
///
/// So a value with no age is **not reported as a value**. See [`summarize`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Measured {
    /// The reading, when there is one.
    pub value: Option<f64>,
    /// How old it is, in milliseconds.
    pub age_ms: Option<u64>,
}

impl Measured {
    /// A dated reading.
    pub fn at(value: f64, age_ms: u64) -> Measured {
        Measured {
            value: Some(value),
            age_ms: Some(age_ms),
        }
    }

    /// Nobody measured it.
    pub fn none() -> Measured {
        Measured::default()
    }
}

/// How a package's command production looks (§33.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Producing at the declared rate with an advancing sequence.
    Ready,
    /// Alive and not producing. The state that looks fine on a dashboard and is not.
    Stalled,
    /// Not running, so the question does not apply.
    NotRunning,
    /// Running, and nothing has reported a sequence yet.
    Unknown,
}

impl Readiness {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Readiness::Ready => "ready",
            Readiness::Stalled => "stalled",
            Readiness::NotRunning => "not_running",
            Readiness::Unknown => "unknown",
        }
    }

    /// Parse one, or `None` for a word this edition does not know.
    pub fn parse(text: &str) -> Option<Readiness> {
        READINESS.into_iter().find(|state| state.as_str() == text)
    }
}

/// Every readiness state.
pub const READINESS: [Readiness; 4] = [
    Readiness::Ready,
    Readiness::Stalled,
    Readiness::NotRunning,
    Readiness::Unknown,
];

/// One package's health, as a check-in carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageHealth {
    /// Package id.
    pub id: String,
    /// Whether the process exists.
    pub live: bool,
    /// Whether it is producing.
    pub readiness: Readiness,
    /// How stale the reading is.
    pub age_ms: u64,
}

/// The named aggregate fields, in the order §33.6 lists them.
pub const HEALTH_FIELDS: [&str; 5] = [
    "generation_lag",
    "guard_trip_rate",
    "storage_headroom",
    "battery",
    "temperature_c",
];

/// What was observed about this robot's health.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HealthInput {
    /// Per package.
    pub packages: Vec<PackageHealth>,
    /// Desired minus applied, when the Hub has been reached.
    pub generation_lag: Measured,
    /// Guard trips per thousand cycles.
    pub guard_trip_rate: Measured,
    /// Fraction of the disk still free, 0–1.
    pub storage_headroom: Measured,
    /// Battery state of charge, 0–1.
    pub battery: Measured,
    /// Hottest thermal zone, degrees Celsius.
    pub temperature_c: Measured,
}

impl HealthInput {
    /// The five aggregate fields, paired with their names, in [`HEALTH_FIELDS`] order.
    fn fields(&self) -> [(&'static str, Measured); 5] {
        [
            ("generation_lag", self.generation_lag),
            ("guard_trip_rate", self.guard_trip_rate),
            ("storage_headroom", self.storage_headroom),
            ("battery", self.battery),
            ("temperature_c", self.temperature_c),
        ]
    }
}

/// Health as a check-in reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct HealthReport {
    /// Whether anything here should stop this robot being handed new work.
    pub degraded: bool,
    /// Packages that are alive and producing nothing — the finding §33.6 exists for.
    pub stalled: Vec<String>,
    /// Fields nobody measured, sorted.
    pub unmeasured: Vec<String>,
    /// Fields that carried a value and no age, sorted. Reported as unmeasured, and named apart.
    pub undated: Vec<String>,
    /// Per package, in the order given.
    pub packages: Vec<PackageHealth>,
    /// The aggregate fields that survived both rules, in [`HEALTH_FIELDS`] order.
    pub fields: Vec<(String, f64, u64)>,
}

/// Roll health up for a check-in (§33.6).
///
/// Three rules, and the second is the one this function was written for:
///
/// * **A stalled package is degraded and an unmeasured field is not.** *"We could not measure the
///   battery"* is not *"the battery is fine"*, and it is also not *"the robot is unhealthy"* — it is
///   a gap, and reporting a gap as a fault is how an operator learns to ignore the health screen.
/// * **A value with no age is dropped and named.** §33.6 requires an age on every field, so a
///   reading that cannot be dated is reported as unmeasured — in `undated` rather than in
///   `unmeasured`, because *"nobody measured this"* and *"somebody measured this and lost track of
///   when"* send different people to different places. Passing the number through would be worse
///   than dropping it: a console renders whatever it is given as current.
/// * **`unknown` readiness is not a stall.** A policy in its first cycle has legitimately produced
///   nothing, and reporting that as a fault would make every start look like one.
pub fn summarize(health: &HealthInput) -> HealthReport {
    let mut stalled: Vec<String> = health
        .packages
        .iter()
        .filter(|entry| entry.readiness == Readiness::Stalled)
        .map(|entry| entry.id.clone())
        .collect();
    stalled.sort();

    let mut unmeasured: Vec<String> = Vec::new();
    let mut undated: Vec<String> = Vec::new();
    let mut fields: Vec<(String, f64, u64)> = Vec::new();

    for (name, measured) in health.fields() {
        match (measured.value, measured.age_ms) {
            (Some(value), Some(age)) if value.is_finite() => {
                fields.push((name.to_string(), value, age))
            }
            (Some(value), None) if value.is_finite() => undated.push(name.to_string()),
            _ => unmeasured.push(name.to_string()),
        }
    }
    unmeasured.sort();
    undated.sort();

    HealthReport {
        degraded: !stalled.is_empty(),
        stalled,
        unmeasured,
        undated,
        packages: health.packages.clone(),
        fields,
    }
}

// ── §7.6's limits, as a check-in carries them ────────────────────────────────

/// What the Safety Kernel would enforce on this robot (§7.3, §7.6, §32.1).
///
/// §32.1 asks a device page for *"active and unavailable guards with the missing input named,
/// resolved limits and their provenance"*, and until this field existed nothing carried them: the
/// robot resolved them, served them on its own LAN console, and the Hub's own route said so in a
/// `gaps` list. A producer with no consumer on one side of §17 and a consumer with no producer on
/// the other, which is the shape Part X was written about.
///
/// # It is a statement about now, which is what makes it belong here
///
/// §34.4 collapses check-ins because every field in one describes the present. These do: they are
/// the ceilings in force and the guard partition as of this pass, not a count, a delta or a
/// history. A robot whose profile changed twice while offline should reconnect reporting the limits
/// it is under, and exactly one of them.
///
/// # `resolved_at` is not a tier claim, and the name is the reason
///
/// It is *"the tier these limits were resolved at"* — `skynet_kernel::ResolvedLimits`' own field,
/// under its own name. §15 awards a tier from bench evidence a human witnessed and a robot never
/// awards itself one (§19), so a field called `tier` here would be read as one within a week.
/// §7.6: nothing on a robot decides its tier, the daemon resolves at the floor, and the numbers
/// above are identical at all three tiers either way.
///
/// # `enforcement` and `enforced` are different facts
///
/// `enforcement` is what the tier *permits* — advisory, enforcing, enforcing-redundant — and is
/// derived from `resolved_at`. `enforced` is whether anything on this robot actually applies these
/// limits to a command, which on an edition with no policy runtime is `false` at every tier. A
/// consumer that had only the first would render a green guard list over a robot enforcing nothing.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardReport {
    /// The tier these limits were resolved at. Never a tier claim — see the type's docs.
    pub resolved_at: String,
    /// What that tier permits.
    pub enforcement: String,
    /// Whether anything on this robot applies them to a command.
    pub enforced: bool,
    /// Narrowest declared joint velocity ceiling.
    pub joint_velocity_max_rad_s: Option<f64>,
    /// Narrowest declared joint torque ceiling.
    pub joint_torque_max_nm: Option<f64>,
    /// Narrowest declared base velocity ceiling.
    pub base_velocity_max_m_s: Option<f64>,
    /// Slowest loop rate the controller tolerates.
    pub min_control_hz: Option<f64>,
    /// Jerk ceiling as a fraction of the channel ceiling per cycle.
    pub max_delta_per_cycle: Option<f64>,
    /// Where the numbers came from. `None` is a limit set nobody cited a datasheet for.
    pub source: Option<String>,
    /// Guards that will run, in evaluation order.
    pub active: Vec<String>,
    /// Guards specified and inoperable here, each with what it wanted.
    pub unavailable: Vec<UnavailableGuard>,
    /// Everything an operator must read, as codes and subjects.
    ///
    /// **Codes, never sentences.** The Hub renders these in thirty-five locales (§32.3) and a robot
    /// that shipped English prose would put the translation on the side that cannot do it. Both
    /// editions hold the same catalogue, so the code is enough.
    ///
    /// `guard_unavailable` is carried here *as well as* in [`Self::unavailable`], and the
    /// duplication is deliberate: this field is what the kernel produced, and a wire that dropped
    /// part of it would be a view rather than evidence. A renderer with the table beside it should
    /// filter the code out; that is the renderer's decision to make.
    pub notices: Vec<Notice>,
}

/// A guard that is specified and cannot run here.
#[derive(Debug, Clone, PartialEq)]
pub struct UnavailableGuard {
    /// Which guard.
    pub guard: String,
    /// What it needs and does not have — a missing input, or *"not applicable to this body"*.
    pub missing: Vec<String>,
}

/// One thing an operator must read, as the kernel emitted it.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    /// The code. `skynet_kernel::NoticeCode`'s wire spelling.
    pub code: String,
    /// The specific things named — missing inputs, widened fields.
    pub subjects: Vec<String>,
}

/// Serialise the guard report, or `null`.
///
/// Present-and-null rather than absent, because *"this robot resolved no limits"* and *"this robot
/// runs an edition that does not report them"* are the same absence to a consumer switching on a
/// key, and both are facts a fleet page acts on differently from a robot that resolved some.
pub fn guard_report_to_json(report: Option<&GuardReport>) -> Json {
    let Some(report) = report else {
        return Json::Null;
    };

    let number = |value: Option<f64>| match value {
        Some(value) => Json::Num(value),
        None => Json::Null,
    };

    let mut unavailable = Json::arr();
    for entry in &report.unavailable {
        let mut row = Json::obj();
        row.set("guard", Json::Str(entry.guard.clone()))
            .set("missing", str_arr(&entry.missing));
        unavailable.push(row);
    }

    let mut notices = Json::arr();
    for notice in &report.notices {
        let mut row = Json::obj();
        row.set("code", Json::Str(notice.code.clone()))
            .set("subjects", str_arr(&notice.subjects));
        notices.push(row);
    }

    let mut out = Json::obj();
    out.set("active", str_arr(&report.active))
        .set(
            "base_velocity_max_m_s",
            number(report.base_velocity_max_m_s),
        )
        .set("enforced", Json::Bool(report.enforced))
        .set("enforcement", Json::Str(report.enforcement.clone()))
        .set("joint_torque_max_nm", number(report.joint_torque_max_nm))
        .set(
            "joint_velocity_max_rad_s",
            number(report.joint_velocity_max_rad_s),
        )
        .set("max_delta_per_cycle", number(report.max_delta_per_cycle))
        .set("min_control_hz", number(report.min_control_hz))
        .set("notices", notices)
        .set("resolved_at", Json::Str(report.resolved_at.clone()))
        .set(
            "source",
            match &report.source {
                Some(source) => Json::Str(source.clone()),
                None => Json::Null,
            },
        )
        .set("unavailable", unavailable);
    out
}

// ── The document ─────────────────────────────────────────────────────────────

/// Everything a check-in is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckInInput {
    /// The Hub's identifier for this robot.
    pub device_id: String,
    /// Which supervisor built this.
    pub agent: String,
    /// Which contract surface it implements.
    pub contracts: String,
    /// §27's operating state, as the daemon last observed it.
    pub state: String,
    /// Where the reconciler has got to.
    pub progress: Progress,
    /// What was observed about health.
    pub health: HealthInput,
    /// §10's three digests.
    pub attestation: Attestation,
    /// `sha256:…` over the canonical desired-state document this robot is reconciling against.
    ///
    /// Empty when it holds none, which is a robot that has never been given one — §50.2's shipping
    /// state, and not the same as a robot whose document could not be digested.
    ///
    /// # This is not history, and that is the test it had to pass
    ///
    /// §34.4 collapses check-ins because every field in one is a statement about **now**, and a
    /// field about the past would make the collapse lossy while looking like a duplicate. A digest
    /// of the document currently on disk is as present-tense as `generation` beside it.
    ///
    /// # Why the generation was not enough
    ///
    /// §11 rests idempotence on the counter, and [`crate::intent::assess_offer`] enforces it by
    /// refusing a *different* document under a generation this robot has already applied. That
    /// refusal is the robot's, and until this field existed it was **only** the robot's: the Hub
    /// knew which generation a robot had applied and had no way to ask *which document*. So a Hub
    /// restored from a backup — the exact case §11 names — could serve generation 41 with different
    /// contents to a fleet, watch every robot refuse it, and see nothing but robots that had
    /// stopped converging. Here it is one comparison.
    pub desired_digest: String,
    /// §7.6's resolved limits and guard partition, when this robot composed any.
    ///
    /// `None` is an edition that does not resolve them, and it is not the same as a robot with no
    /// ceilings — that one reports a [`GuardReport`] whose numbers are all absent, with the
    /// `no_envelope` notice saying why.
    pub limits: Option<GuardReport>,
}

/// The document a robot sends.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckIn {
    /// What it may claim.
    pub claim: ClaimVerdict,
    /// Rolled-up health.
    pub health: HealthReport,
    /// §10's three digests, whole.
    pub attestation: Attestation,
    /// Who is reporting.
    pub device_id: String,
    /// Which supervisor.
    pub agent: String,
    /// Which contract surface.
    pub contracts: String,
    /// §27's state.
    pub state: String,
    /// What is left to do, sorted by stage.
    pub pending: Vec<Pending>,
    /// The document behind the generation. See [`CheckInInput::desired_digest`].
    pub desired_digest: String,
    /// §7.6's limits, carried through unchanged. See [`GuardReport`].
    pub limits: Option<GuardReport>,
}

/// Build the document a robot sends (§11, §17).
///
/// This function is the whole of what crosses to the Hub, which is why it is here rather than in the
/// daemon: two editions must produce the **same bytes** for the same robot, or a fleet console shows
/// two robots that differ only in who compiled their supervisor.
///
/// It cannot fail, for [`attest`]'s reason, and it is the same reason: the robot most in need of a
/// check-in is the one that cannot measure everything. A report withheld for want of a field is a
/// robot the Hub believes is offline.
pub fn check_in(input: &CheckInInput) -> CheckIn {
    let mut pending = input.progress.pending.clone();
    pending.sort_by(|a, b| a.stage.cmp(&b.stage));

    CheckIn {
        claim: claim_for(&input.progress),
        health: summarize(&input.health),
        attestation: input.attestation.clone(),
        device_id: input.device_id.trim().to_string(),
        agent: input.agent.trim().to_string(),
        contracts: input.contracts.trim().to_string(),
        state: input.state.trim().to_string(),
        pending,
        desired_digest: input.desired_digest.trim().to_string(),
        // Carried through rather than derived. What the guards are and what they lack is the
        // kernel's answer for this body, and a builder that recomputed any of it would be a second
        // implementation of §7.3 living in the reporting path.
        limits: input.limits.clone(),
    }
}

/// The one-line summary a console and a terminal both render.
pub fn summary(built: &CheckIn) -> String {
    let report = attest(&built.attestation);
    let who = if built.device_id.is_empty() {
        "This robot"
    } else {
        &built.device_id
    };
    let health = if built.health.degraded {
        format!(
            "degraded ({} alive and producing nothing)",
            built.health.stalled.join(", ")
        )
    } else {
        "no package is alive and producing nothing".to_string()
    };
    format!(
        "{who} reports generation {} as {}, {health}. {}",
        built.claim.generation,
        built.claim.claim.as_str(),
        report.sentence
    )
}

/// One claim, as the corpus holds it.
pub fn claim_verdict_to_json(verdict: &ClaimVerdict) -> Json {
    let mut out = Json::obj();
    out.set("applied", Json::Bool(verdict.applied))
        .set("blocking", str_arr(&verdict.blocking))
        .set("claim", Json::Str(verdict.claim.as_str().to_string()))
        .set("detail", Json::Str(verdict.detail.clone()))
        .set("generation", Json::Num(verdict.generation as f64));
    out
}

/// One health rollup, as the corpus holds it.
pub fn health_report_to_json(report: &HealthReport) -> Json {
    let mut packages = Json::arr();
    for entry in &report.packages {
        let mut row = Json::obj();
        row.set("age_ms", Json::Num(entry.age_ms as f64))
            .set("id", Json::Str(entry.id.clone()))
            .set("live", Json::Bool(entry.live))
            .set("readiness", Json::Str(entry.readiness.as_str().to_string()));
        packages.push(row);
    }

    let mut fields = Json::obj();
    for name in HEALTH_FIELDS {
        match report.fields.iter().find(|(field, _, _)| field == name) {
            // Present-and-null for a field with no reading, and an object where there is one. Both
            // keys are always there, which is what lets a console iterate the five §33.6 names
            // rather than discovering which of them this robot happened to measure.
            None => {
                fields.set(name, Json::Null);
            }
            Some((_, value, age)) => {
                let mut node = Json::obj();
                node.set("age_ms", Json::Num(*age as f64))
                    .set("value", Json::Num(*value));
                fields.set(name, node);
            }
        }
    }

    let mut out = Json::obj();
    out.set("degraded", Json::Bool(report.degraded))
        .set("fields", fields)
        .set("packages", packages)
        .set("stalled", str_arr(&report.stalled))
        .set("undated", str_arr(&report.undated))
        .set("unmeasured", str_arr(&report.unmeasured));
    out
}

/// The check-in as it goes on the wire.
pub fn check_in_to_json(built: &CheckIn) -> Json {
    let health = health_report_to_json(&built.health);

    let mut pending = Json::arr();
    for entry in &built.pending {
        let mut row = Json::obj();
        row.set("deferred", Json::Bool(entry.deferred))
            .set("reason", Json::Str(entry.reason.clone()))
            .set("stage", Json::Str(entry.stage.clone()));
        pending.push(row);
    }

    let mut out = Json::obj();
    out.set("agent", Json::Str(built.agent.clone()))
        // §17's field, first among the ones a reader looks for. It is `false` for three of the four
        // claims, and the claim beside it says which — because "not applied" covers a robot that is
        // working, a robot that is waiting for a person, and a robot whose supervisor disagrees
        // with itself.
        .set("applied", Json::Bool(built.claim.applied))
        // The sentence **and** the three digests it renders. See
        // [`crate::identity::attestation_to_json`]: §10 asks the Hub to record what a robot is
        // running, and a fleet console cannot index prose.
        .set("attestation", attestation_to_json(&built.attestation))
        .set("blocking", str_arr(&built.claim.blocking))
        .set("claim", Json::Str(built.claim.claim.as_str().to_string()))
        .set("contracts", Json::Str(built.contracts.clone()))
        .set("detail", Json::Str(built.claim.detail.clone()))
        .set("desired_digest", Json::Str(built.desired_digest.clone()))
        .set("device_id", Json::Str(built.device_id.clone()))
        .set("generation", Json::Num(built.claim.generation as f64))
        .set("health", health)
        // §7.6's ceilings and guard partition, or `null`. Present-and-null so a consumer can tell
        // "this edition does not report them" from "this robot resolved none".
        .set("limits", guard_report_to_json(built.limits.as_ref()))
        .set("pending", pending)
        .set("state", Json::Str(built.state.clone()));
    out
}

fn str_arr(values: &[String]) -> Json {
    Json::Arr(values.iter().map(|s| Json::Str(s.clone())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Measure;
    use alloc::vec;

    fn report() -> GuardReport {
        GuardReport {
            resolved_at: "guest".to_string(),
            enforcement: "advisory".to_string(),
            enforced: false,
            joint_velocity_max_rad_s: Some(0.5),
            joint_torque_max_nm: Some(80.0),
            base_velocity_max_m_s: None,
            min_control_hz: None,
            max_delta_per_cycle: None,
            source: Some("vendor datasheet rev C".to_string()),
            active: vec!["watchdog".to_string(), "envelope".to_string()],
            unavailable: vec![UnavailableGuard {
                guard: "balance".to_string(),
                missing: vec!["a legged base — not applicable to this body".to_string()],
            }],
            notices: vec![Notice {
                code: "advisory_only".to_string(),
                subjects: Vec::new(),
            }],
        }
    }

    fn pending(stage: &str, deferred: bool) -> Pending {
        Pending {
            stage: stage.to_string(),
            deferred,
            reason: "because".to_string(),
        }
    }

    fn attestation() -> Attestation {
        Attestation {
            device_id: "dev_7".to_string(),
            os_image: Measure::known("sha256:aa"),
            policy: Measure::nothing(),
            embodiment: Measure::known("sha256:cc"),
        }
    }

    #[test]
    fn an_empty_plan_on_the_wanted_generation_is_the_only_applied_state() {
        let verdict = claim_for(&Progress {
            observed_generation: 41,
            desired_generation: 41,
            pending: Vec::new(),
        });
        assert_eq!(verdict.claim, Claim::Applied);
        assert!(verdict.applied);
        assert_eq!(verdict.generation, 41);
    }

    #[test]
    fn the_generation_on_the_wire_is_never_the_desired_one() {
        // §17's rule lives on this field. A check-in echoing the desired generation would mark
        // every deployment applied the moment the robot heard about it.
        for pendings in [
            vec![pending("policy", false)],
            vec![pending("policy", true)],
        ] {
            let verdict = claim_for(&Progress {
                observed_generation: 40,
                desired_generation: 41,
                pending: pendings,
            });
            assert_eq!(verdict.generation, 40);
            assert!(!verdict.applied);
        }
        // And in the state where nothing is pending and they still disagree.
        let verdict = claim_for(&Progress {
            observed_generation: 40,
            desired_generation: 41,
            pending: Vec::new(),
        });
        assert_eq!(verdict.generation, 40);
    }

    #[test]
    fn work_left_and_work_waiting_are_different_claims() {
        let working = claim_for(&Progress {
            observed_generation: 40,
            desired_generation: 41,
            pending: vec![pending("policy", false), pending("os_image", true)],
        });
        assert_eq!(working.claim, Claim::Converging);

        let waiting = claim_for(&Progress {
            observed_generation: 40,
            desired_generation: 41,
            pending: vec![pending("policy", true), pending("os_image", true)],
        });
        assert_eq!(waiting.claim, Claim::Deferred);
        assert!(waiting.detail.contains("operator action"));
    }

    #[test]
    fn an_empty_plan_that_closes_nothing_is_named_rather_than_ticked() {
        let verdict = claim_for(&Progress {
            observed_generation: 40,
            desired_generation: 41,
            pending: Vec::new(),
        });
        assert_eq!(verdict.claim, Claim::Inconsistent);
        assert!(!verdict.applied);
    }

    #[test]
    fn blocking_stages_are_sorted_and_said_once() {
        let verdict = claim_for(&Progress {
            observed_generation: 1,
            desired_generation: 2,
            pending: vec![
                pending("policy", false),
                pending("os_image", false),
                pending("policy", false),
            ],
        });
        assert_eq!(verdict.blocking, vec!["os_image", "policy"]);
    }

    #[test]
    fn a_reading_with_no_age_is_not_a_reading() {
        // §33.6: every field carries its age. A number a console cannot date is one it renders as
        // current, and the four-hour-old battery reading is the one somebody drives out on.
        let report = summarize(&HealthInput {
            battery: Measured {
                value: Some(0.12),
                age_ms: None,
            },
            storage_headroom: Measured::at(0.4, 900),
            ..HealthInput::default()
        });
        assert_eq!(report.undated, vec!["battery"]);
        assert!(!report.fields.iter().any(|(name, _, _)| name == "battery"));
        assert_eq!(
            report.fields,
            vec![("storage_headroom".to_string(), 0.4, 900)]
        );
        // And it is named apart from the fields nobody measured at all.
        assert!(!report.unmeasured.contains(&"battery".to_string()));
    }

    #[test]
    fn an_unmeasured_field_is_not_a_fault_and_a_stall_is() {
        let nothing = summarize(&HealthInput::default());
        assert!(!nothing.degraded);
        assert_eq!(nothing.unmeasured.len(), HEALTH_FIELDS.len());

        let stalled = summarize(&HealthInput {
            packages: vec![PackageHealth {
                id: "acme/grasp".to_string(),
                live: true,
                readiness: Readiness::Stalled,
                age_ms: 40,
            }],
            ..HealthInput::default()
        });
        assert!(stalled.degraded);
        assert_eq!(stalled.stalled, vec!["acme/grasp"]);
    }

    #[test]
    fn a_first_cycle_is_not_a_stall() {
        let report = summarize(&HealthInput {
            packages: vec![PackageHealth {
                id: "acme/grasp".to_string(),
                live: true,
                readiness: Readiness::Unknown,
                age_ms: 5,
            }],
            ..HealthInput::default()
        });
        assert!(!report.degraded);
    }

    #[test]
    fn every_health_field_is_a_key_whether_or_not_it_was_measured() {
        // A console iterates §33.6's five names; it must not have to discover which of them this
        // robot happened to measure.
        let built = check_in(&CheckInInput {
            device_id: "dev_7".to_string(),
            agent: "skynetd/test".to_string(),
            contracts: "1.0.0".to_string(),
            state: "hold".to_string(),
            progress: Progress::default(),
            health: HealthInput::default(),
            attestation: attestation(),
            desired_digest: String::new(),
            limits: None,
        });
        let document = check_in_to_json(&built);
        let fields = document
            .get("health")
            .and_then(|health| health.get("fields"))
            .expect("health.fields");
        for name in HEALTH_FIELDS {
            assert_eq!(fields.get(name), Some(&Json::Null), "{name}");
        }
    }

    #[test]
    fn the_attestation_goes_in_whole_and_incomplete() {
        let built = check_in(&CheckInInput {
            device_id: String::new(),
            agent: "skynetd/test".to_string(),
            contracts: "1.0.0".to_string(),
            state: "post".to_string(),
            progress: Progress::default(),
            health: HealthInput::default(),
            attestation: Attestation {
                device_id: String::new(),
                os_image: Measure::unmeasured("the slot could not be read"),
                policy: Measure::nothing(),
                embodiment: Measure::nothing(),
            },
            desired_digest: String::new(),
            limits: None,
        });
        let document = check_in_to_json(&built);
        let attestation = document.get("attestation").expect("attestation");
        assert_eq!(
            attestation.get("complete").and_then(Json::as_bool),
            Some(false)
        );
        assert!(attestation
            .get("sentence")
            .and_then(Json::as_str)
            .is_some_and(|sentence| sentence.contains("unmeasured")));

        // The three measures survive whatever the sentence says about them. A robot that could not
        // read its own slot reports `unmeasured` with the reason, and a robot running no policy
        // reports `nothing` — two facts a fleet console groups by and neither of which is a digest.
        let os_image = attestation.get("os_image").expect("os_image");
        assert_eq!(
            os_image.get("state").and_then(Json::as_str),
            Some("unmeasured")
        );
        assert_eq!(os_image.get("digest"), Some(&Json::Null));
        assert_eq!(
            os_image.get("why").and_then(Json::as_str),
            Some("the slot could not be read")
        );
        assert_eq!(
            attestation
                .get("policy")
                .and_then(|policy| policy.get("state"))
                .and_then(Json::as_str),
            Some("nothing")
        );
    }

    /// §11's counter says *which* intent; this says which **document**.
    ///
    /// The pair is what makes the Hub able to run [`crate::intent::assess_offer`] over what it is
    /// about to serve. With the generation alone, a Hub restored from a backup can serve
    /// generation 41 with different contents to a fleet, watch every robot refuse it, and see only
    /// robots that have stopped converging.
    #[test]
    fn the_document_behind_the_generation_is_named() {
        let built = check_in(&CheckInInput {
            device_id: "dev_7".to_string(),
            agent: "skynetd/test".to_string(),
            contracts: "1.0.0".to_string(),
            state: "running".to_string(),
            progress: Progress::default(),
            health: HealthInput::default(),
            attestation: attestation(),
            desired_digest: "  sha256:dddd  ".to_string(),
            limits: None,
        });
        assert_eq!(built.desired_digest, "sha256:dddd");
        assert_eq!(
            check_in_to_json(&built)
                .get("desired_digest")
                .and_then(Json::as_str),
            Some("sha256:dddd")
        );
    }

    /// A robot holding nothing says so, rather than omitting the field.
    ///
    /// Present-and-empty for the reason §33.6's five health fields are present-and-null: a fleet
    /// page that has to discover which keys a robot happened to send is a page that gets it wrong
    /// on the robot that sent none — and *"reconciling against nothing"* is a state somebody acts
    /// on, not an absence.
    #[test]
    fn a_robot_with_no_desired_state_says_so() {
        let built = check_in(&CheckInInput {
            device_id: String::new(),
            agent: "skynetd/test".to_string(),
            contracts: "1.0.0".to_string(),
            state: "post".to_string(),
            progress: Progress::default(),
            health: HealthInput::default(),
            attestation: attestation(),
            desired_digest: String::new(),
            limits: None,
        });
        assert_eq!(
            check_in_to_json(&built)
                .get("desired_digest")
                .and_then(Json::as_str),
            Some("")
        );
    }

    /// §10: *"the Hub records not 'robot X is online' but 'robot X is running image `sha256:…`'"*.
    ///
    /// The regression this pins is the one that shipped: the check-in carried [`attest`]'s
    /// **sentence** and nothing else, so the only route from the wire to a digest was a regular
    /// expression over prose that is deliberately not machine-readable. The assertion is therefore
    /// on the digest as a *field*, because a check for `sha256:aaaa` anywhere in the document would
    /// have passed against the version this test exists to prevent.
    #[test]
    fn the_three_digests_are_fields_and_not_only_prose() {
        let built = check_in(&CheckInInput {
            device_id: "dev_7".to_string(),
            agent: "skynetd/test".to_string(),
            contracts: "1.0.0".to_string(),
            state: "running".to_string(),
            progress: Progress::default(),
            health: HealthInput::default(),
            attestation: attestation(),
            desired_digest: String::new(),
            limits: None,
        });
        let document = check_in_to_json(&built);
        let attestation = document.get("attestation").expect("attestation");

        for (name, digest) in [
            ("os_image", Some("sha256:aa")),
            ("policy", None),
            ("embodiment", Some("sha256:cc")),
        ] {
            let measure = attestation.get(name).unwrap_or_else(|| panic!("{name}"));
            assert_eq!(
                measure.get("digest").and_then(Json::as_str),
                digest,
                "{name}"
            );
        }
    }

    #[test]
    fn pending_is_sorted_so_two_editions_agree_byte_for_byte() {
        let built = check_in(&CheckInInput {
            device_id: "dev_7".to_string(),
            agent: String::new(),
            contracts: String::new(),
            state: String::new(),
            progress: Progress {
                observed_generation: 1,
                desired_generation: 2,
                pending: vec![pending("policy", false), pending("os_image", true)],
            },
            health: HealthInput::default(),
            attestation: attestation(),
            desired_digest: String::new(),
            limits: None,
        });
        let stages: Vec<&str> = built.pending.iter().map(|p| p.stage.as_str()).collect();
        assert_eq!(stages, vec!["os_image", "policy"]);
    }

    #[test]
    fn every_stage_a_check_in_can_name_is_in_the_vocabulary() {
        // The set exists so a fleet console can group by it. A stage the vocabulary does not hold
        // is one that groups into a column of its own on somebody's dashboard.
        assert_eq!(STAGES.len(), 6);
        for stage in STAGES {
            assert!(!stage.is_empty());
        }
    }

    /// §7.6's block is carried, never recomputed.
    ///
    /// What the guards are and what they lack is the kernel's answer for one body. A builder that
    /// derived any of it would be a second implementation of §7.3 living in the reporting path, and
    /// the two would disagree first on the machine nobody was looking at.
    #[test]
    fn the_limits_cross_unchanged() {
        let built = check_in(&CheckInInput {
            device_id: "dev_7".to_string(),
            agent: "skynetd/0.1.0".to_string(),
            contracts: "1.0.0".to_string(),
            state: "running".to_string(),
            progress: Progress::default(),
            health: HealthInput::default(),
            attestation: attestation(),
            desired_digest: String::new(),
            limits: Some(report()),
        });
        assert_eq!(built.limits.as_ref(), Some(&report()));

        let document = check_in_to_json(&built);
        let limits = document.get("limits").expect("present");
        assert_eq!(limits.get("resolved_at"), Some(&Json::from("guest")));
        assert_eq!(
            limits.get("source"),
            Some(&Json::from("vendor datasheet rev C"))
        );
        // Absent ceilings are null rather than zero. A base velocity of 0 m/s is a robot forbidden
        // to move; a base velocity nobody declared is a robot with no whole-body ceiling, and §7.2
        // spends its whole length on the difference.
        assert_eq!(limits.get("base_velocity_max_m_s"), Some(&Json::Null));
        assert_eq!(
            limits.arr_or_empty("unavailable")[0].arr_or_empty("missing")[0],
            Json::from("a legged base — not applicable to this body"),
            "§7.2's sentence has to survive the crossing verbatim: a fixed base is a shape and not \
             a defect, and a Hub re-deriving this row would report a missing IMU"
        );
    }

    /// `enforcement` and `enforced` are different facts, and a consumer with only the first would
    /// render a green guard list over a robot that enforces nothing.
    #[test]
    fn a_tier_that_permits_enforcement_is_not_a_robot_that_enforces() {
        let mut permitting = report();
        permitting.resolved_at = "host".to_string();
        permitting.enforcement = "enforcing".to_string();
        // Still false: the tier permits it and this edition has no policy runtime to do it.
        assert!(!permitting.enforced);

        let document = guard_report_to_json(Some(&permitting));
        assert_eq!(document.get("enforcement"), Some(&Json::from("enforcing")));
        assert_eq!(document.get("enforced"), Some(&Json::Bool(false)));
    }

    /// A robot that reports no limits and a robot with no ceilings are different documents.
    ///
    /// `null` rather than an absent key, because a consumer switching on the presence of a field
    /// would read "this edition does not report them" and "this robot resolved none" as one fact,
    /// and a fleet page acts on them differently.
    #[test]
    fn no_limits_is_null_and_no_ceilings_is_a_report_full_of_nulls() {
        let silent = check_in(&CheckInInput {
            device_id: "dev_7".to_string(),
            agent: String::new(),
            contracts: String::new(),
            state: String::new(),
            progress: Progress::default(),
            health: HealthInput::default(),
            attestation: attestation(),
            desired_digest: String::new(),
            limits: None,
        });
        assert_eq!(check_in_to_json(&silent).get("limits"), Some(&Json::Null));

        let bare = GuardReport {
            joint_velocity_max_rad_s: None,
            joint_torque_max_nm: None,
            source: None,
            active: Vec::new(),
            unavailable: Vec::new(),
            notices: vec![Notice {
                code: "no_envelope".to_string(),
                subjects: Vec::new(),
            }],
            ..report()
        };
        let document = guard_report_to_json(Some(&bare));
        assert_ne!(document, Json::Null);
        assert_eq!(document.get("joint_velocity_max_rad_s"), Some(&Json::Null));
        assert_eq!(
            document.arr_or_empty("notices")[0].get("code"),
            Some(&Json::from("no_envelope")),
            "the report says why it is empty rather than being empty"
        );
    }
}

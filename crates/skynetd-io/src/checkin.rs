//! §11 and §17's check-in on a real machine: what this robot puts in one, and when.
//!
//! [`skynet_contracts::checkin`] decides what a check-in *is* and takes no clock, no network and no
//! filesystem. This is the other half — reading the reconciler's last plan, the health rollup, §10's
//! identity and §27's state off the machine, and putting the result in §34.4's queue.
//!
//! # An unpaired robot does not enqueue one
//!
//! §17's route is `POST /devices/{id}/checkin` and an unpaired robot has no `{id}`. A check-in built
//! anyway would be a document addressed to nobody, and §34.4's queue would then hold it, collapse
//! it, and eventually count it in the drop counters — so the console would report a robot
//! discarding its own state, for want of a Hub it was never given. The honest state is an empty
//! queue and an identity panel saying why.
//!
//! # One per pass, and the queue collapses them
//!
//! §34.4 puts check-ins in the class where *"only the newest matters, so older ones are collapsed
//! rather than queued"*. That makes enqueueing every reconcile pass the correct thing rather than a
//! wasteful one: the queue holds exactly one, it is always current, and a robot that reconnects
//! after a week sends one check-in rather than ten thousand.
//!
//! The cost is one file write per reconcile period, and the alternative — enqueue only when the
//! document changed — is edge-triggered, which is the mistake §33.3 spends its longest paragraph
//! on. A robot whose check-in was dropped for want of disk would then never build another, because
//! nothing changed.
//!
//! # What it cannot measure
//!
//! Two of §33.6's five aggregate fields, down from four. `generation_lag` is arithmetic this daemon
//! can do and `battery` and `temperature_c` are §36's, read from `/sys` by [`crate::power`]. What
//! is left is `guard_trip_rate`, which needs the kernel through a path §33.1 denies this daemon,
//! and `storage_headroom`, which needs a `statvfs` nothing here calls. Both are reported absent,
//! which is a different fact from zero and is the one §33.6 asks for.

use skynet_contracts::checkin::{
    check_in, check_in_to_json, CheckIn, CheckInInput, GuardReport, HealthInput, Measured,
    PackageHealth, Pending, Progress, Readiness, STAGES,
};
use skynet_json::Json;
use skynetd::actual::Actual;
use skynetd::reconcile::{plan, Plan};
use skynetd::SafeState;

use crate::identity::Held;
use crate::layout::Layout;
use crate::outbound::{Class, Queue};

/// The stage name §33.4's table numbers `stage`.
///
/// A number outside 1–6 becomes `stage_<n>` rather than one of the six or a shared `unknown`. It
/// cannot happen from this workspace — `Action::new` is internal to the reconciler and only ever
/// passes 1 to 6 — and if it ever does, a fleet console should group it into a column of its own
/// rather than silently into `lease`.
pub fn stage_name(stage: u8) -> String {
    match STAGES.get(stage.saturating_sub(1) as usize) {
        Some(name) if stage >= 1 => (*name).to_string(),
        _ => alloc_stage(stage),
    }
}

fn alloc_stage(stage: u8) -> String {
    format!("stage_{stage}")
}

/// Where the reconciler has got to, read off a fresh plan rather than remembered (§33.3).
///
/// The plan is recomputed here rather than taken from `last_pass`, and the difference matters: a
/// pass records what was *attempted*, and a check-in has to report what is *left*. A robot that
/// applied every action in its last pass and still has work to do — which is the ordinary case,
/// because §33.3 needs several passes — would otherwise report itself converged one pass early.
pub fn progress(
    desired: &skynetd::desired::DesiredState,
    actual: &Actual,
    safe: SafeState,
) -> Progress {
    let computed: Plan = plan(desired, actual);
    let mut pending = Vec::new();
    for action in &computed.actions {
        let open = action.gate.is_open(safe, actual);
        pending.push(Pending {
            stage: stage_name(action.stage),
            deferred: !open,
            // The action's name and its subject, which is what `GET /plan` already renders —
            // through the same function, so the sentence a Hub reads and the one an operator reads
            // on the console cannot drift into two vocabularies for one plan.
            reason: if open {
                format!(
                    "{} {}, and nothing is in the way",
                    crate::api::action_name(&action.kind),
                    action.subject()
                )
            } else {
                format!(
                    "{} {}: {}",
                    crate::api::action_name(&action.kind),
                    action.subject(),
                    action.gate.reason()
                )
            },
        });
    }

    Progress {
        observed_generation: actual.generation,
        desired_generation: desired.generation,
        pending,
    }
}

/// Roll the daemon's health observation into the shape a check-in carries (§33.6).
///
/// Three of §33.6's five aggregate fields now have a source, and two do not. `generation_lag` is
/// arithmetic this daemon can do, and `battery` and `temperature_c` are §36's, read from `/sys` by
/// [`crate::power`] and arriving here through [`skynetd::health::Health`] — the same struct
/// `GET /health` renders, so a robot cannot show one battery figure on its console and send another
/// to its Hub. `guard_trip_rate` needs the kernel and `storage_headroom` needs a `statvfs` nothing
/// here calls, and both are reported absent, which is a different fact from zero.
///
/// **One age for all three**, because they are one pass: the caller observes, reads the gauge and
/// rolls up in the same turn of the loop, so there is one measurement instant and `age_ms` is its
/// age. §33.6 drops any field carrying a value and no age, so a reading that arrived undated would
/// be reported as unmeasured — correctly by the rule, and baffling on a robot whose battery had
/// just been read.
pub fn health(
    observed: &skynetd::health::Health,
    age_ms: u64,
    generation_lag: Option<u64>,
) -> HealthInput {
    HealthInput {
        packages: observed
            .packages
            .iter()
            .map(|package| PackageHealth {
                id: package.id.clone(),
                live: package.live,
                readiness: match package.readiness {
                    skynetd::health::Readiness::Ready => Readiness::Ready,
                    skynetd::health::Readiness::Stalled => Readiness::Stalled,
                    skynetd::health::Readiness::NotRunning => Readiness::NotRunning,
                    skynetd::health::Readiness::Unknown => Readiness::Unknown,
                },
                age_ms: package.age_ms,
            })
            .collect(),
        generation_lag: dated(generation_lag.map(|lag| lag as f64), age_ms),
        // The two §33.6 names that nothing on this robot measures. Absent rather than zero: no
        // measurement of guard trips and zero guard trips are opposite facts.
        guard_trip_rate: Measured::none(),
        storage_headroom: Measured::none(),
        // §36's two, and the shape of the conversion is the interesting part. `battery` is a
        // *fraction*, which is what §33.6 asks for and what a fleet page renders as a bar — so a
        // gauge that reported watt-hours is turned back into one against the declared capacity, and
        // a body with no declared capacity reports no battery rather than a watt-hour figure in a
        // column labelled with a percentage.
        battery: dated(observed.battery, age_ms),
        temperature_c: dated(observed.temperature_c, age_ms),
    }
}

/// A value and the age of the pass it was measured in, or an absence.
///
/// One helper rather than three `match`es, because §33.6's rule is one rule: a value with no age is
/// reported as unmeasured, and the three fields that have values all have the same one.
fn dated(value: Option<f64>, age_ms: u64) -> Measured {
    match value {
        Some(value) => Measured::at(value, age_ms),
        None => Measured::none(),
    }
}

/// Build the document this robot would send right now.
pub fn build(
    identity: &Held,
    attestation: skynet_contracts::identity::Attestation,
    progress: Progress,
    health: HealthInput,
    state: &str,
    desired_digest: &str,
    limits: Option<GuardReport>,
    machine: Option<skynet_contracts::checkin::MachineReadings>,
) -> CheckIn {
    check_in(&CheckInInput {
        device_id: identity.identity.device_id.clone(),
        agent: format!("skynetd/{}", crate::api::VERSION),
        contracts: skynet_contracts::CONTRACTS_VERSION.to_string(),
        state: state.to_string(),
        progress,
        health,
        attestation,
        // Passed in rather than read here, so this function stays a function of what has already
        // been observed — the same shape `attestation` and `progress` arrive in.
        desired_digest: desired_digest.to_string(),
        // §7.6's ceilings and guard partition, from the same `crate::limits` composition
        // `GET /limits` serves — so the fleet page and the robot's own console can never be
        // describing two different limit sets. `None` is a caller that could not compose them,
        // which is not the same as a robot with no ceilings (see `GuardReport`).
        limits,
        // §23.5's machine readings, read by the caller off the machine both processes share.
        machine,
    })
}

/// Read §23.5's machine half of a check-in off this machine.
///
/// `skynet-rt`'s probes, translated to the contract shape. Always `Some`: the readings inside
/// may be absent, and that absence is the report — an edition that reads nothing is `None` at
/// the call site, and this edition reads.
pub fn read_machine() -> Option<skynet_contracts::checkin::MachineReadings> {
    Some(skynet_contracts::checkin::MachineReadings {
        preempt_rt: skynet_rt::preempt_rt(),
        swap_kb: skynet_rt::swap_kb().map(|kb| kb as f64),
    })
}

/// Why a check-in was not queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotQueued {
    /// This robot is not paired, so there is no route to address one to (§17).
    Unpaired,
    /// The queue refused it. Carries the sentence.
    Refused(String),
}

/// Put one in §34.4's queue, or say why not.
///
/// `Ok(None)` is the unpaired case and is deliberately not an error: a robot nobody has adopted is
/// at a correct point in its lifecycle (§50.2), and a reconcile loop that logged a failure every
/// pass for it would train an operator to ignore the one that matters.
pub fn enqueue(
    queue: &Queue,
    identity: &Held,
    built: &CheckIn,
) -> Result<Option<String>, NotQueued> {
    if built.device_id.trim().is_empty() || !identity.identity.credential {
        return Err(NotQueued::Unpaired);
    }
    let body = check_in_to_json(built)
        .to_canonical()
        .map_err(|error| NotQueued::Refused(format!("{error:?}")))?;
    match queue.enqueue(Class::CheckIn, &body) {
        Ok(enqueued) => Ok(Some(enqueued.id)),
        Err(error) => Err(NotQueued::Refused(error.reason())),
    }
}

/// The check-in as an API answer, for `GET /checkin` and the console.
///
/// The **document that would be sent**, plus whether one is queued and why not. A route that showed
/// only the last one sent would be blank on every robot this edition runs on, and a route that
/// showed only the document would not answer the question an operator actually has, which is
/// whether the Hub is hearing any of this.
pub fn to_json(built: &CheckIn, queued: &Result<Option<String>, NotQueued>) -> Json {
    let mut out = check_in_to_json(built);
    out.set(
        "summary",
        Json::Str(skynet_contracts::checkin::summary(built)),
    );
    let (queued_flag, note) = match queued {
        Ok(Some(_)) => (
            true,
            "queued under §34.4's check-in class, where the newest supersedes the last.".to_string(),
        ),
        Ok(None) => (false, "nothing was queued.".to_string()),
        Err(NotQueued::Unpaired) => (
            false,
            "this robot is not paired (§10), so there is no `/devices/{id}/checkin` to address one \
             to. Nothing is queued, which is why the outbound queue is empty rather than growing."
                .to_string(),
        ),
        Err(NotQueued::Refused(detail)) => (false, detail.clone()),
    };
    out.set("queued", Json::Bool(queued_flag))
        .set("queued_note", Json::Str(note));
    out
}

/// Where a request body may be staged, for the Hub sink.
pub fn scratch(layout: &Layout) -> std::path::PathBuf {
    layout.state()
}

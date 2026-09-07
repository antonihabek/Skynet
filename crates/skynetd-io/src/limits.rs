//! §7.3's resolved limits, composed on the robot (§6.3, §38.2).
//!
//! `skynet_kernel::resolve_limits` is the one place a ceiling is chosen. It has had three callers
//! since it was written — the kernel's own suite, the corpus runner, and `skynet-contracts`'
//! renderer — and **none of them on a robot**. So the function that composes the envelope, the
//! transport plan and the operator profile into the single set the kernel enforces had never once
//! been asked what *this machine's* limits are.
//!
//! That absence was cheap to miss because nothing downstream of it existed either: no policy
//! runtime, so no cycle to enforce over. But two things did depend on it and both were stuck:
//!
//! * §6.3 — *"this module decides what is possible and under what ceilings, and hands the ceilings
//!   to the Safety Kernel"*. The plan computed a clamp at install (§37.1) and handed it to nothing,
//!   which is §73's transport row in one sentence.
//! * `telemetry.replay.deterministic` — replay is a function of the recording **and** the limits
//!   that were in force, and `crate::conformance` reported it skipped for exactly that reason.
//!
//! # The tier is an input here, and it is never invented
//!
//! `resolve_limits` takes a [`Tier`], and **nothing on this robot decides one**: §15 awards a tier
//! from conformance evidence a human witnessed, §11 would deliver it in desired state, and neither
//! path is written. `crate::runtime` already refuses to fill `robot.tier` for that reason.
//!
//! So this module takes the tier from its caller and states which one it resolved at. What makes
//! that safe rather than a fudge is a property of `resolve_limits` itself, asserted in this
//! module's own suite rather than assumed: **the tier changes what is done with the limits, never
//! what they are.** Every ceiling, every active guard and every unavailable guard is identical at
//! all three tiers. What the tier decides is the enforcement mode and two notices —
//! `advisory_only` and `no_independent_stop` — which is precisely the honest report: *these are the
//! numbers, and here is what this integration depth would do with them.*
//!
//! [`ROBOT_TIER`] is guest and the reason is not a preference. §1's table defines guest as the tier
//! that does not own the control loop and enforces nothing, and this edition has no policy runtime
//! and no kernel on a control path — so guest is a description of this build rather than a claim
//! about the machine, and it is the reading that claims least.
//!
//! # Where each input comes from, and the three that have no source
//!
//! | Input | Source |
//! |---|---|
//! | `envelope` | the embodiment descriptor's `envelope` (§5) |
//! | `frames` | the descriptor's frame graph, flattened (§44.3) |
//! | `available.legged` | the descriptor's `base` |
//! | `available.imu` | a sensor whose modality is `imu` |
//! | `profile.*` ceilings | `/data/state/profiles/active.json` (§38.1) |
//! | `profile.workspace` | §55.2's cell, out of the same profile |
//! | `transport` | §37.1's plan, for a named installed package |
//! | `profile.proximity` | the same profile's `limits.proximity` (§11) |
//! | `profile.self_collision_margin_m` | the same profile's `limits` (§11) |
//! | `available.people_detection` | a sensor declaring `detects_people` (§5) |
//!
//! `people_detection` was `false` on every body until 2026-09-07, and that was a measured absence
//! rather than a default: §5's sensor vocabulary has nine modalities and none of them is a person
//! detector. A camera is not one — detection is a perception component, not a sensor — and
//! inferring one from `rgb` would be exactly the *"claim about the body that nothing checks"* that
//! `Available` had two fields removed for.
//!
//! §5 closed it the only way that does not require inference: a **boolean the vendor writes on the
//! sensor**, because only the party that built the sensing pipeline knows whether their depth
//! stream is one a person is measured in. The derivation is
//! [`has_people_detection`](skynet_contracts::embodiment::has_people_detection) — a mirrored
//! contract, not three lines here, so a Hub reading the same descriptor cannot answer differently.
//! A body that declares nothing still leaves the guard unavailable naming a sensor that detects
//! people, which is the guard reporting on the body rather than on the descriptor's author.
//!
//! **What this does not do is make the guard work.** `proximity` acts on `cycle.nearest_person_m`,
//! and nothing on this edition fills that field: no capture backend, no perception component, no
//! path from a declaration to a distance. So a robot whose vendor declares a detector now lists
//! `proximity` as **active** and the guard never trips, which reads as protection to anybody
//! looking at the list. That is a worse failure than the one this closed, it is the same shape as
//! `available.imu` has always had, and it is one step wider because that field has no producer at
//! all. §80 carries the question of whether the state deserves a name of its own; until it does,
//! nothing here should be read as saying a robot is monitoring separation.
//!
//! The other two were gaps in the **delivery path** rather than in the body, and they are closed:
//! §11's profile carries `self_collision_margin_m` and a `proximity` block beside its four
//! ceilings, so both now arrive under the profile's own signature rather than through a second
//! path (§38.1).
//!
//! One of them changes what the machine does. `self_collision_margin_m` is read by §7.1's fifth
//! guard, so a profile that sets it is a robot whose collision bodies stop short of touching
//! instead of at the moment they meet — and `NoCollisionMargin` stops being emitted on every robot.
//! The other did not, then: `proximity` also needs `available.people_detection`, which was `false`
//! on every body until §5 grew the field. What changed first was the **report** — the guard named
//! the sensor it has not got rather than a profile field that could not be delivered, which is the
//! difference between "this platform cannot do it" and "this robot has not got one". What changes
//! now is that a body can answer it.
//!
//! # The cell's volumes stop being reported and start being applied
//!
//! `crate::cell` computes §55.2's workspace layer and says of it:
//!
//! > the layer is computed and **reported**, with the reason it goes no further
//!
//! This is where it goes further. The layer is folded into the operator profile the kernel reads,
//! so a robot with a surveyed cell and a frame graph that reaches the frames it names has
//! `workspace` in its active guards for the first time — and one whose graph does not reach them
//! has the missing frame named, which is §44.5 working rather than a body silently unguarded.

use skynet_contracts::cell::CellLayer;
use skynet_contracts::checkin::{GuardReport, InertGuard, Notice, UnavailableGuard};
use skynet_contracts::embodiment::SafetyEnvelope;
use skynet_contracts::kinematics::{flatten, FlatFrames};
use skynet_contracts::transport::TransportClamp as PlanClamp;
use skynet_json::Json;

// Through `skynet-contracts` rather than through a direct edge to `skynet-kernel`. The daemon's
// dependency set is a thing CI asserts about (§33.1), and every kernel type this module needs is
// already re-exported by the layer that renders them — so the composition and the rendering read
// the same names out of the same place.
use skynet_contracts::safety::kinematics::{Frame, FrameSet, Volume, WorkspaceLayer};
use skynet_contracts::safety::{
    limits_to_json, resolve_limits, Available, Envelope, OperatorProfile,
    Proximity as KernelProximity, ResolveLimitsParams, ResolvedLimits, Tier, TransportClamp,
};

use crate::embodiment::Body;
use crate::layout::Layout;

/// The tier this edition resolves at, and why it is not a choice.
///
/// §1: guest *"runs on a container beside the vendor stack"*, does not own the control loop, and
/// its Safety Kernel is advisory. This build has no policy runtime, so nothing here owns a control
/// loop and no verdict reaches an actuator — which is that row, describing this repository.
///
/// A robot integrated more deeply than this is still reported at guest, and the error is in the
/// safe direction: the numbers are identical at every tier, and what is understated is only what
/// this software would be permitted to *do* with them.
pub const ROBOT_TIER: Tier = Tier::Guest;

/// The limits, and everything they were composed from.
///
/// Owned, because [`skynet_kernel`] borrows: it is `no_std` with no allocator and takes `&str` and
/// `&[(&str, f64)]` throughout. `skynet-corpus` performs the same ceremony for a JSON document and
/// its own module docs predicted this one — *"it is the same ceremony `skynetd` will perform for
/// real"*.
#[derive(Debug, Clone)]
pub struct Owned {
    tier: Tier,
    /// Whether a body was described at all.
    ///
    /// Not derivable from `envelope`, and the difference is what §27's `guards` condition rests on.
    /// A descriptor with no `envelope` block is a body somebody described and did not give
    /// ceilings; no descriptor is a robot nobody has described. Both leave `envelope` at `None`,
    /// and the first is a robot whose guard partition means something.
    described: bool,
    envelope: Option<SafetyEnvelope>,
    profile: Option<ProfileOwned>,
    /// §55.2's cell layer, when the cell draws usable volumes.
    cell: Option<CellLayer>,
    frames: Option<FlatFrames>,
    /// Why there is no frame set, when the descriptor declared one.
    frames_fault: Option<String>,
    available: Available,
    transport: Option<PlanClamp>,
    /// Which package the clamp above came from, when one did.
    transport_of: Option<String>,
}

/// The operator profile in force, as far as this robot receives one (§38.1).
#[derive(Debug, Clone, Default)]
struct ProfileOwned {
    name: String,
    joint_velocity_max_rad_s: Option<f64>,
    joint_torque_max_nm: Option<f64>,
    base_velocity_max_m_s: Option<f64>,
    max_delta_per_cycle: Option<f64>,
    self_collision_margin_m: Option<f64>,
    /// §7.1's separation thresholds, promoted to the kernel's triple only when complete.
    ///
    /// `skynet_kernel::Proximity` requires all three and a profile may declare fewer, so the
    /// promotion happens here and an incomplete set stays `None`. That leaves the guard reporting
    /// `profile.proximity` as its missing input, which is §7.2's rule — a guard with a partial
    /// input reports unavailable and never runs on a default somebody invented for the rest.
    proximity: Option<KernelProximity>,
}

/// Compose the limits this robot would run under.
///
/// Never fails. Every input is optional to `resolve_limits` and every absence produces a notice
/// rather than a refusal — a body nobody described resolves to no ceilings and a `no_envelope`
/// notice, which is the honest answer for an unprovisioned robot and not an error.
///
/// A **malformed** descriptor is the one case worth distinguishing and it is distinguished by the
/// same rule `crate::capacity` applies: nothing is salvaged from it. A malformed body carries no
/// envelope and no frames here, so the limits come out as though nobody had described the body —
/// which is what a document nobody could read amounts to — and `GET /embodiment` is where the
/// reason lives.
pub fn read(
    layout: &Layout,
    body: &Body,
    tier: Tier,
    proximity: &crate::blackbox::ProximitySighting,
) -> Owned {
    let known = body.known();
    let descriptor = known.map(|known| &known.descriptor);

    let (frames, frames_fault) = match descriptor.and_then(|d| d.frames.as_ref()) {
        None => (None, None),
        Some(graph) => match flatten(graph) {
            Ok(flat) => (Some(flat), None),
            // §44.5: a graph that does not flatten leaves both pose guards unavailable, naming
            // `embodiment.frames`, and the fault itself is carried so the route can say which node
            // broke it rather than leaving an operator to diff two panels.
            Err(fault) => (None, Some(format!("{} — {}", fault.code, fault.detail))),
        },
    };

    let available = Available {
        // Declared, not inferred. A body with no `imu` sensor leaves `balance` unavailable naming
        // an IMU, which is the guard reporting on the body rather than on the descriptor's author.
        imu: descriptor.is_some_and(|d| d.sensors.iter().any(|s| s.modality == "imu")),
        // Declared per sensor by the vendor, and derived by the contract rather than here: this
        // was `false` on every body for as long as §5 had no field for it, and the derivation is
        // `has_people_detection` so that a Hub reading the same descriptor cannot answer
        // differently.
        people_detection: descriptor
            .is_some_and(skynet_contracts::embodiment::has_people_detection),
        legged: descriptor.is_some_and(|d| d.base == "legged"),
        // §80's third state, and it is a **reading** off §13's ring rather than the constant
        // `false` it was until 2026-09-07. The constant was the honest answer on this edition —
        // nothing here fills `cycle.nearest_person_m` — and it carried the note *"the day a
        // producer exists, this is the line that changes"*, which is precisely the shape that goes
        // wrong silently: the day a producer existed, a robot would have gone on drawing the guard
        // as inert with a detector filling the field on every cycle.
        //
        // It is counted over an interval rather than taken off the newest cycle, because *does a
        // producer exist* is not a state — see `ProximitySighting`. On a robot with no vision path
        // the answer is still `false` and `proximity` still resolves **inert**; what changed is
        // that the robot is now saying so because it looked.
        proximity_reading: proximity.arriving(),
    };

    Owned {
        tier,
        described: descriptor.is_some(),
        envelope: descriptor.and_then(|d| d.envelope.clone()),
        profile: read_profile(layout),
        cell: crate::cell::active(layout)
            .as_ref()
            .and_then(skynet_contracts::cell::cell_workspace_layer),
        frames,
        frames_fault,
        available,
        // §37.1 resolves a plan per package at install and this robot runs none, so there is no
        // clamp in force. `with_package` is how a caller asks for a specific one.
        transport: None,
        transport_of: None,
    }
}

/// The profile in force, or nothing.
///
/// Reads the file `crate::runtime` writes after a signature verified (§38.1), which is why nothing
/// here re-checks one: a document that reached `active.json` is a document this robot verified, and
/// a second opinion taken from the same bytes would not be a second opinion.
fn read_profile(layout: &Layout) -> Option<ProfileOwned> {
    let document = crate::observe::read_json(&layout.active_profile())?;
    let (_, limits) = crate::observe::read_profile(layout);
    let limits = limits?;
    Some(ProfileOwned {
        name: document
            .get("name")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string(),
        joint_velocity_max_rad_s: limits.joint_velocity_max_rad_s,
        joint_torque_max_nm: limits.joint_torque_max_nm,
        base_velocity_max_m_s: limits.base_velocity_max_m_s,
        max_delta_per_cycle: limits.max_delta_per_cycle,
        self_collision_margin_m: limits.self_collision_margin_m,
        proximity: limits.proximity.and_then(|prox| {
            // All three or none. Two thresholds and no factor is not a proximity guard with a
            // default multiplier — it is a guard nobody finished configuring, and inventing the
            // third number here would be this layer deciding a safety parameter.
            Some(KernelProximity {
                stop_distance_m: prox.stop_distance_m?,
                slow_distance_m: prox.slow_distance_m?,
                slow_factor: prox.slow_factor?,
            })
        }),
    })
}

impl Owned {
    /// Fold in the ceilings §37.1's plan computed for one installed package (§6.3).
    ///
    /// The clamp narrows and can never widen — `resolve_limits` takes the minimum of every source
    /// — so adding it is safe whichever package a caller names. What it cannot do is speak for a
    /// *running* policy, because none runs: this is "what would this package be held to", which is
    /// the question `skynet limits --package` exists to answer.
    pub fn with_package(mut self, id: &str, clamp: PlanClamp) -> Self {
        self.transport = Some(clamp);
        self.transport_of = Some(id.to_string());
        self
    }

    /// The envelope, borrowed.
    fn envelope_ref(&self) -> Option<Envelope<'_>> {
        self.envelope.as_ref().map(|declared| Envelope {
            joint_velocity_max_rad_s: declared.joint_velocity_max_rad_s,
            joint_torque_max_nm: declared.joint_torque_max_nm,
            base_velocity_max_m_s: declared.base_velocity_max_m_s,
            watchdog_ms: declared.watchdog_ms,
            estop: declared.estop,
            source: declared.source.as_deref(),
        })
    }

    /// Per-channel ceilings, sorted by name.
    ///
    /// `TransportClamp::per_channel` is documented as sorted and the planner emits it in binding
    /// order, which nothing has ever noticed because this is the first caller to hand one to the
    /// kernel. Sorted here rather than there because the ordering is this interface's requirement
    /// and the plan is a document with an order of its own.
    fn per_channel_refs(&self) -> Vec<(&str, f64)> {
        let mut out: Vec<(&str, f64)> = self
            .transport
            .as_ref()
            .map(|clamp| {
                clamp
                    .per_channel
                    .iter()
                    .map(|(name, limit)| (name.as_str(), *limit))
                    .collect()
            })
            .unwrap_or_default();
        out.sort_by_key(|(name, _)| *name);
        out
    }

    /// Resolve, and hand the result to `f`.
    ///
    /// A closure because a resolved set borrows four separate views that have to outlive it and
    /// cannot be returned alongside it. The alternative is every caller repeating the same five
    /// lines, which is how two callers end up resolving slightly different limit sets.
    ///
    /// The envelope is passed through as well because `evaluate_cycle` and
    /// `skynet_blackbox::replay` both want it beside the limits: the watchdog guard reads the
    /// declared window from the envelope rather than from the resolved set.
    pub fn with_resolved<T>(
        &self,
        f: impl FnOnce(&ResolvedLimits<'_>, Option<&Envelope<'_>>) -> T,
    ) -> T {
        let per_channel = self.per_channel_refs();
        let frame_refs: Option<Vec<Frame<'_>>> = self.frames.as_ref().map(FlatFrames::frames);
        let volumes: Vec<Volume<'_>> = self
            .cell
            .as_ref()
            .map(CellLayer::volumes)
            .unwrap_or_default();
        let layers: Vec<WorkspaceLayer<'_>> = match &self.cell {
            Some(cell) if !volumes.is_empty() => vec![cell.layer(&volumes)],
            _ => Vec::new(),
        };

        let params = ResolveLimitsParams {
            tier: self.tier,
            envelope: self.envelope_ref(),
            transport: self.transport.as_ref().map(|clamp| TransportClamp {
                joint_velocity_max_rad_s: clamp.joint_velocity_max_rad_s,
                joint_torque_max_nm: clamp.joint_torque_max_nm,
                base_velocity_max_m_s: clamp.base_velocity_max_m_s,
                min_control_hz: clamp.min_control_hz,
                per_channel: &per_channel,
                source: clamp.source.as_deref(),
            }),
            profile: self.profile.as_ref().map(|profile| OperatorProfile {
                name: profile.name.as_str(),
                joint_velocity_max_rad_s: profile.joint_velocity_max_rad_s,
                joint_torque_max_nm: profile.joint_torque_max_nm,
                base_velocity_max_m_s: profile.base_velocity_max_m_s,
                max_delta_per_cycle: profile.max_delta_per_cycle,
                proximity: profile.proximity,
                workspace: &layers,
                self_collision_margin_m: profile.self_collision_margin_m,
            }),
            available: self.available,
            frames: frame_refs
                .as_deref()
                .and_then(|frames| FrameSet::new(frames).ok()),
        };
        let limits = resolve_limits(&params);
        f(&limits, params.envelope.as_ref())
    }

    /// The profile's name, for the widened-limits notice.
    pub fn profile_name(&self) -> Option<&str> {
        self.profile.as_ref().map(|profile| profile.name.as_str())
    }
}

/// What this daemon can see of §27's **`guards`** condition — *"a guard is unavailable"*.
///
/// The fifth of §27's eight to gain a producer, and the first that is **not** read off §13's ring.
/// `observed.rs` had it filed as *"a cycle records what guards did, not which were available"*,
/// which is a true sentence about the ring and the wrong place to have looked: §7.6's partition is
/// resolved **on this daemon**, by `resolve_limits`, and served at `GET /limits` and across §17 to
/// the device page. The answer to *which guards are unavailable* was already being computed,
/// rendered in two places and reported to the Hub, while §27 took it at a default.
///
/// # Two sources, and the field's own documentation names the one that was missed
///
/// §27's `guards_unavailable` is documented as *"§45.5 is the usual source"* — calibration expiry —
/// and that list is [`crate::calibration::Status::unavailable`], which is a **different** list from
/// `resolve_limits`'s. Both are this daemon's, neither was wired to §27, and either alone would be
/// an answer that is right about half the robots: a body that declares no IMU loses `balance`
/// through the resolver, and a body with an expired camera extrinsic loses `proximity` through
/// §45.5 with a perfectly good descriptor. So this is the **union**, and it reports which source
/// each guard came from, because *"fit the sensor"* and *"recalibrate"* are different jobs.
///
/// # It is a resolved partition, not a per-cycle one, and that is stated rather than hidden
///
/// What this observes is which guards **can** run, given what the body declares, what the profile
/// sets and what §45.5 still believes. It is not a reading of which guards the kernel had inputs
/// for on the last cycle. §80's `inert` state exists precisely because those two can differ — a
/// guard whose every declared input resolves and whose per-cycle reading never arrives — and
/// `inert` is carried here separately for that reason rather than folded into unavailable.
///
/// The gap this named — *a declared sensor that fails at runtime; the descriptor goes on declaring
/// it and nothing here notices* — is **half closed** since 2026-09-07, and it closed exactly where
/// this paragraph said it belonged: with the ring readings. `available.proximity_reading` is
/// counted off §13's ring by [`crate::blackbox::ProximitySighting`], so a person detector that was
/// filling `cycle.nearest_person_m` and stops moves `proximity` from active to **inert** within an
/// interval, and a console stops drawing protection the robot no longer has.
///
/// It is half rather than whole because that is the only sensor whose per-cycle reading appears in
/// a record. An IMU that fails is still invisible here: §26 carries the base pose and twist a
/// driver reported, not a per-sensor liveness, and `balance` resolves off the descriptor's `imu`
/// declaration alone. Claiming otherwise would be exactly the overreach this type is arranged
/// against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardsSighting {
    /// No body has been described, so there is no partition to take. Not a reading.
    ///
    /// The distinction that makes this a variant rather than "everything is unavailable": with no
    /// descriptor `resolve_limits` finds nothing declared and reports nearly every guard
    /// unavailable, which is arithmetically correct and would tell an operator their guards are
    /// broken when what is missing is the document describing the robot.
    Unobservable(&'static str),
    /// The partition resolved and every guard this body declares has its inputs.
    AllAvailable,
    /// The partition resolved and at least one guard is unavailable, named with its source.
    Unavailable(Vec<UnavailableFor>),
}

/// One unavailable guard and which of the two sources took it out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnavailableFor {
    /// §7.2's guard name.
    pub guard: String,
    /// Whether §45.5's expiry is what removed it, rather than a missing declaration.
    pub calibration: bool,
}

/// Take §27's `guards` condition off the resolved partition and §45.5's expiry list.
///
/// `expired` is passed in rather than read here for [`report`]'s reason one function up: this stays
/// a function over facts, and one that read the clock and the disk could not be driven by a suite.
pub fn guards(
    owned: &Owned,
    expired: &[skynet_contracts::calibration::GuardUnavailable],
) -> GuardsSighting {
    if !owned.described {
        return GuardsSighting::Unobservable(
            "no embodiment descriptor has been accepted, so there is no guard set to partition \
             (§5, §7.2)",
        );
    }

    let mut out: Vec<UnavailableFor> = owned.with_resolved(|limits, _| {
        limits
            .unavailable_guards
            .iter()
            .map(|entry| UnavailableFor {
                guard: entry.guard.as_str().to_string(),
                calibration: false,
            })
            .collect()
    });

    // §45.5's half, merged rather than appended: a guard can lose its declaration *and* its
    // calibration, and reporting it twice would make one guard read as two problems. Where both
    // are true the calibration is the one an operator can act on today, so it wins the label.
    for entry in expired {
        match out.iter_mut().find(|held| held.guard == entry.guard) {
            Some(held) => held.calibration = true,
            None => out.push(UnavailableFor {
                guard: entry.guard.clone(),
                calibration: true,
            }),
        }
    }

    out.sort_by(|a, b| a.guard.cmp(&b.guard));
    out.dedup_by(|a, b| a.guard == b.guard);

    if out.is_empty() {
        GuardsSighting::AllAvailable
    } else {
        GuardsSighting::Unavailable(out)
    }
}

impl GuardsSighting {
    /// §27's condition. True when any guard is unavailable, from either source.
    pub fn unavailable(&self) -> bool {
        matches!(self, GuardsSighting::Unavailable(_))
    }

    /// Whether this was an observation at all.
    pub fn observed(&self) -> bool {
        !matches!(self, GuardsSighting::Unobservable(_))
    }

    /// What was seen, in a sentence.
    ///
    /// Names the guards and their source, because §45.5's whole requirement is that an unavailable
    /// guard say *why*: "proximity unavailable" sends nobody anywhere and "proximity unavailable:
    /// the calibration expired" sends them to a target board.
    pub fn detail(&self) -> String {
        match self {
            GuardsSighting::Unobservable(why) => (*why).to_string(),
            GuardsSighting::AllAvailable => {
                "every guard this body declares has its inputs, and no applicable calibration has \
                 expired"
                    .to_string()
            }
            GuardsSighting::Unavailable(entries) => {
                let named: Vec<String> = entries
                    .iter()
                    .map(|entry| {
                        if entry.calibration {
                            format!("{} (a calibration expired, §45.5)", entry.guard)
                        } else {
                            format!("{} (an input the body does not declare)", entry.guard)
                        }
                    })
                    .collect();
                format!("{} unavailable: {}", named.len(), named.join(", "))
            }
        }
    }

    /// The guards, for a route that renders them as data rather than as a sentence.
    pub fn entries(&self) -> &[UnavailableFor] {
        match self {
            GuardsSighting::Unavailable(entries) => entries,
            _ => &[],
        }
    }
}

/// §7.6's limits, as §17's check-in carries them.
///
/// A projection rather than the route's document, and the differences are the point. It carries no
/// rendered English — the Hub renders notice codes in thirty-five locales (§32.3) and a robot that
/// shipped sentences would put the translation on the side that cannot do it — no `sources` block,
/// and none of the prose the console needs. What is left is about five hundred bytes, which is what
/// makes it a field on a document §34.4 sends every reconcile pass rather than a route of its own.
///
/// `enforced` is `false` and it is not read from anything: no policy runtime exists on this
/// edition, so nothing carries a command as far as a guard at any tier (§73). It is a constant with
/// a reason rather than a field waiting for one, and the day a runtime exists is the day this line
/// has to become a measurement.
pub fn report(owned: &Owned) -> GuardReport {
    owned.with_resolved(|limits, _| GuardReport {
        resolved_at: limits.tier.as_str().to_string(),
        enforcement: limits.enforcement.as_str().to_string(),
        enforced: false,
        joint_velocity_max_rad_s: limits.joint_velocity_max_rad_s,
        joint_torque_max_nm: limits.joint_torque_max_nm,
        base_velocity_max_m_s: limits.base_velocity_max_m_s,
        min_control_hz: limits.min_control_hz,
        max_delta_per_cycle: limits.max_delta_per_cycle,
        self_collision_margin_m: limits.self_collision_margin_m,
        source: limits.source.map(str::to_string),
        active: limits
            .active_guards
            .iter()
            .map(|guard| guard.as_str().to_string())
            .collect(),
        unavailable: limits
            .unavailable_guards
            .iter()
            .map(|entry| UnavailableGuard {
                guard: entry.guard.as_str().to_string(),
                missing: entry.missing.iter().map(|s| (*s).to_string()).collect(),
            })
            .collect(),
        inert: limits
            .inert_guards
            .iter()
            .map(|entry| InertGuard {
                guard: entry.guard.as_str().to_string(),
                reading: entry.reading.to_string(),
            })
            .collect(),
        notices: limits
            .notices
            .iter()
            .map(|notice| Notice {
                code: notice.code.as_str().to_string(),
                subjects: notice.subjects.iter().map(|s| (*s).to_string()).collect(),
            })
            .collect(),
    })
}

/// What `GET /limits` answers.
///
/// The resolved set as `skynet-contracts` renders it, plus a `sources` block, because the numbers
/// alone do not answer the question an operator actually has — *why is this ceiling that?* A limit
/// set with no provenance is the `unsourced_limits` notice applied to the report instead of to the
/// envelope.
pub fn to_json(owned: &Owned) -> Json {
    owned.with_resolved(|limits, _| {
        let mut out = limits_to_json(limits, owned.profile_name());

        let mut sources = Json::obj();
        sources
            .set("envelope", Json::Bool(owned.envelope.is_some()))
            .set("profile", Json::Bool(owned.profile.is_some()))
            .set(
                "workspace_layers",
                Json::Num(if owned.cell.is_some() { 1.0 } else { 0.0 }),
            )
            .set(
                "frames",
                match &owned.frames {
                    Some(flat) => Json::Num(flat.frames().len() as f64),
                    None => Json::Null,
                },
            )
            .set(
                "frames_fault",
                match &owned.frames_fault {
                    Some(fault) => Json::from(fault.as_str()),
                    None => Json::Null,
                },
            )
            .set(
                "transport_of",
                match &owned.transport_of {
                    Some(id) => Json::from(id.as_str()),
                    None => Json::Null,
                },
            );
        out.set("sources", sources);

        let mut available = Json::obj();
        available
            .set("imu", Json::Bool(owned.available.imu))
            .set("legged", Json::Bool(owned.available.legged))
            .set(
                "people_detection",
                Json::Bool(owned.available.people_detection),
            );
        out.set("available", available);

        // Named absences, never zeros. Each is a field the kernel's `OperatorProfile` accepts and a
        // guard reads, with nothing on this robot able to deliver one.
        out.set(
            "unmeasured",
            Json::Arr(vec![
                Json::from(
                    "profile.proximity — §11's desired state carries no such field, so the \
                     `proximity` guard cannot run on this edition however the body is described",
                ),
                Json::from(
                    "profile.self_collision_margin_m — the same, so two of this robot's own bodies \
                     must meet before the `self_collision` guard holds",
                ),
                // Not `available.people_detection` any more — §5 grew `detects_people` and a
                // body can answer it. What is unmeasured is the reading itself, and it is the
                // sharper entry of the two: a declaration says a stream exists, and the guard
                // acts on a *number*.
                Json::from(
                    "cycle.nearest_person_m — no producer on this edition fills it, so a body \
                     that declares `detects_people` puts `proximity` in the active guards and \
                     nothing will ever trip it. An active guard reads as protection",
                ),
            ]),
        );

        out.set(
            "tier_source",
            Json::from(
                "assumed. Nothing on this robot decides its tier: §15 awards one from conformance \
                 evidence and §11 would deliver it in desired state, and neither path is written. \
                 Resolved at the floor, which claims least — and the ceilings above are identical \
                 at all three tiers, so what the assumption decides is the enforcement mode and \
                 two notices rather than a number.",
            ),
        );
        // The one thing an operator would otherwise have to derive from two fields. `envelope` is
        // in `active_guards` on every robot — it has no missing input — and with an empty
        // `per_channel` it has nothing to compare a joint command against, because §6.3 says a
        // joint's ceiling comes from the transport plan and this robot has none in force. Reading
        // "active" as "enforcing a ceiling" is the wrong reading and it is the natural one.
        if limits.per_channel.is_empty() {
            out.set(
                "per_channel_note",
                Json::from(
                    "No per-channel ceiling is in force. The `envelope` guard is active and \
                     has nothing to clamp a joint command against: §6.3 hands a joint's \
                     ceiling to the kernel from the transport plan, and a plan belongs to a \
                     package (`?package=`). The whole-body figures above are not applied per \
                     channel, because nothing at this layer knows whether a channel carries \
                     radians per second or newton-metres.",
                ),
            );
        }
        out.set("enforced", Json::Bool(false));
        out.set(
            "enforced_note",
            Json::from(
                "These are the limits the Safety Kernel would enforce, composed by the same \
                 `resolve_limits` a control loop would call. Nothing enforces them here: §33.1 \
                 keeps this daemon off the actuator path and no policy runtime exists to carry a \
                 command as far as a guard (§73).",
            ),
        );
        out
    })
}

/// The set, for a terminal (§64).
pub fn to_lines(owned: &Owned) -> Vec<String> {
    owned.with_resolved(|limits, _| {
        let number = |value: Option<f64>| match value {
            Some(value) => format!("{value}"),
            None => "—".to_string(),
        };
        let mut lines = vec![
            format!(
                "limits: resolved at `{}` ({}), and enforced by nothing on this edition",
                limits.tier.as_str(),
                limits.enforcement.as_str()
            ),
            format!(
                "  joint velocity {} rad/s · torque {} Nm · base {} m/s · jerk {}",
                number(limits.joint_velocity_max_rad_s),
                number(limits.joint_torque_max_nm),
                number(limits.base_velocity_max_m_s),
                number(limits.max_delta_per_cycle),
            ),
            format!(
                "  source: {}",
                limits
                    .source
                    .unwrap_or("none cited — a safety limit without provenance is a rumour"),
            ),
        ];
        let active: Vec<&str> = limits.active_guards.iter().map(|g| g.as_str()).collect();
        lines.push(format!(
            "  {} of 8 guards would run: {}",
            active.len(),
            if active.is_empty() {
                "none".to_string()
            } else {
                active.join(", ")
            }
        ));
        for entry in limits.unavailable_guards.iter() {
            let missing: Vec<&str> = entry.missing.iter().copied().collect();
            lines.push(format!(
                "    {:<16} needs {}",
                entry.guard.as_str(),
                missing.join(", ")
            ));
        }
        // §80's third state, said in its own words rather than folded in with the line above. An
        // unavailable guard "needs" something this robot does not have; an inert one has
        // everything it declared and is waiting on a reading nothing produces, and an operator
        // told to fit a sensor they already fitted goes looking in the wrong place.
        for entry in limits.inert_guards.iter() {
            lines.push(format!(
                "    {:<16} inert — every declared input resolves and nothing fills {}",
                entry.guard.as_str(),
                entry.reading
            ));
        }
        for notice in limits.notices.iter() {
            lines.push(format!(
                "  ! {}: {}",
                notice.code.as_str(),
                skynet_contracts::safety::notice_message(notice, owned.profile_name())
            ));
        }
        lines
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An arm that declares an envelope, a jointed frame graph with two collision bodies, and an
    /// IMU. Enough for six of the eight guards to have something to say.
    const ARM: &str = r#"{
        "robot_slug": "bench-arm",
        "base": "fixed",
        "dof": 1,
        "envelope": { "joint_velocity_max_rad_s": 1.0, "joint_torque_max_nm": 80,
                      "watchdog_ms": 100, "source": "vendor datasheet rev C" },
        "sensors": [{"name": "torso_imu", "modality": "imu"},
                    {"name": "head_cam", "modality": "rgb"}],
        "joints": [{"name": "shoulder", "type": "revolute", "control_modes": ["position"]}],
        "frames": {
            "root": "base_link",
            "convention": "rep103",
            "nodes": [
                {"name": "base_link", "parent": null, "kind": "root", "collision": true,
                 "collision_shape": {"kind": "capsule", "radius_m": 0.1, "length_m": 0.2},
                 "translation": [0, 0, 0], "rotation": [0, 0, 0, 1], "source": "descriptor"},
                {"name": "tool0", "parent": "base_link", "kind": "dynamic",
                 "joint": "shoulder", "joint_type": "revolute", "axis": [0, 1, 0],
                 "translation": [0, 0, 0.1], "rotation": [0, 0, 0, 1],
                 "source": "descriptor", "collision": true,
                 "collision_shape": {"kind": "capsule", "radius_m": 0.05, "length_m": 0.3}}
            ]
        }
    }"#;

    /// A profile that narrows two ceilings and surveys a cell around `tool0`.
    const PROFILE: &str = r#"{
        "name": "site-a",
        "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        "limits": { "joint_velocity_max_rad_s": 0.5, "max_delta_per_cycle": 0.1 },
        "cell": {
            "name": "bay-3",
            "facts": { "door_closed": true },
            "allowed": [{ "frame": "tool0",
                          "shape": { "kind": "box", "min": [-1, -1, 0], "max": [1, 1, 2] } }]
        }
    }"#;

    /// The same profile with §11's `proximity` block, which is the guard's other input.
    ///
    /// All three numbers or none (see `read_profile`): two thresholds and no factor is a guard
    /// nobody finished configuring, and this layer inventing the third would be it deciding a
    /// safety parameter.
    const PROFILE_WITH_PROXIMITY: &str = r#"{
        "name": "site-a",
        "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
        "limits": { "joint_velocity_max_rad_s": 0.5, "max_delta_per_cycle": 0.1,
                    "proximity": { "stop_distance_m": 0.3, "slow_distance_m": 1.0,
                                   "slow_factor": 0.25 } }
    }"#;

    struct Bench {
        root: std::path::PathBuf,
    }

    impl Bench {
        fn new(label: &str) -> Bench {
            let root =
                std::env::temp_dir().join(format!("skynetd-limits-{}-{label}", std::process::id()));
            std::fs::remove_dir_all(&root).ok();
            let bench = Bench { root };
            std::fs::create_dir_all(bench.layout().state()).unwrap();
            bench
        }

        fn layout(&self) -> Layout {
            Layout::new(&self.root)
        }

        fn describe(&self, document: &str) -> &Bench {
            std::fs::write(self.layout().embodiment(), document).unwrap();
            self
        }

        fn profile(&self, document: &str) -> &Bench {
            std::fs::create_dir_all(self.layout().profiles()).unwrap();
            std::fs::write(self.layout().active_profile(), document).unwrap();
            self
        }

        fn resolve(&self, tier: Tier) -> Owned {
            self.resolve_seeing(
                tier,
                &crate::blackbox::ProximitySighting::Unobservable("this bench has no ring"),
            )
        }

        /// The same, with §80's proximity reading supplied.
        ///
        /// Separate rather than a fourth argument on `resolve`, because every case but one is
        /// about the descriptor and the profile — and a bench where every call site had to name a
        /// ring reading would bury the one case that is about it.
        fn resolve_seeing(
            &self,
            tier: Tier,
            proximity: &crate::blackbox::ProximitySighting,
        ) -> Owned {
            let layout = self.layout();
            read(&layout, &crate::embodiment::load(&layout), tier, proximity)
        }
    }

    impl Drop for Bench {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    /// §27's `guards`, and the second source the condition's own documentation named.
    ///
    /// `guards_unavailable` is documented in `skynet-contracts` as *"§45.5 is the usual source"*,
    /// and §45.5's list is `calibration::Status::unavailable` — a different list from
    /// `resolve_limits`'s. Reading only the resolver would miss a body whose descriptor is perfect
    /// and whose camera extrinsic expired last month, which is the *usual* case the field names.
    #[test]
    fn the_guard_condition_is_the_union_of_two_lists() {
        use skynet_contracts::calibration::GuardUnavailable;

        let bench = Bench::new("guards-condition");
        bench.describe(ARM);
        let owned = bench.resolve(ROBOT_TIER);

        // Whatever this body leaves unavailable through the resolver, none of it is a calibration.
        let resolved = guards(&owned, &[]);
        for entry in resolved.entries() {
            assert!(
                !entry.calibration,
                "{} came from the resolver and must not be labelled a calibration",
                entry.guard
            );
        }

        // §45.5's half, for a guard the resolver was happy with. It must appear, and it must be
        // labelled as the calibration it is — the two send an operator to different places.
        let expired = [GuardUnavailable {
            guard: "proximity".to_string(),
            set: "camera_extrinsics".to_string(),
            state: "expired",
            detail: "camera_extrinsics expired 40 days ago".to_string(),
        }];
        let merged = guards(&owned, &expired);
        assert!(merged.unavailable());
        let proximity = merged
            .entries()
            .iter()
            .find(|entry| entry.guard == "proximity")
            .expect("§45.5's guard reaches the condition");
        assert!(proximity.calibration);
        assert!(merged.detail().contains("§45.5"));

        // One guard, not two, when both sources name it — and the calibration wins the label,
        // because that is the one an operator can act on today.
        let doubled = guards(&owned, &expired);
        let named = doubled
            .entries()
            .iter()
            .filter(|entry| entry.guard == "proximity")
            .count();
        assert_eq!(
            named, 1,
            "one guard is one problem however many lists name it"
        );
    }

    /// A robot nobody has described is **unobservable**, not a robot with broken guards.
    ///
    /// With no descriptor `resolve_limits` finds nothing declared and reports nearly every guard
    /// unavailable. That is arithmetically right and, as an answer to §27, would tell an operator
    /// their guards are broken when what is missing is the document describing the robot.
    #[test]
    fn a_body_nobody_described_has_no_partition_to_take() {
        let bench = Bench::new("guards-undescribed");
        let owned = bench.resolve(ROBOT_TIER);

        // The resolver does report guards unavailable here, which is exactly why the variant is
        // needed rather than derived from the list being empty.
        let (_, unavailable) = partition(&owned);
        assert!(!unavailable.is_empty(), "the resolver has plenty to say");

        let sighting = guards(&owned, &[]);
        assert!(!sighting.observed());
        assert!(
            !sighting.unavailable(),
            "an unobservable condition is not a positive reading of it"
        );
        assert!(sighting.detail().contains("no embodiment descriptor"));
    }

    /// The resolved partition alone, without §45.5's half.
    ///
    /// Renamed off `guards` when the module gained a public function of that name: a test helper
    /// shadowing the thing under test is how a suite ends up proving something about itself.
    fn partition(owned: &Owned) -> (Vec<String>, Vec<String>) {
        owned.with_resolved(|limits, _| {
            (
                limits
                    .active_guards
                    .iter()
                    .map(|guard| guard.as_str().to_string())
                    .collect(),
                limits
                    .unavailable_guards
                    .iter()
                    .map(|entry| entry.guard.as_str().to_string())
                    .collect(),
            )
        })
    }

    fn notices(owned: &Owned) -> Vec<String> {
        owned.with_resolved(|limits, _| {
            limits
                .notices
                .iter()
                .map(|notice| notice.code.as_str().to_string())
                .collect()
        })
    }

    /// The claim the whole module rests on, checked rather than believed.
    ///
    /// This robot's tier is an assumption (§15 awards one and nothing here does), and the
    /// assumption is only defensible if it decides nothing about the numbers. So: resolve the same
    /// machine three times and assert that every ceiling, every active guard and every unavailable
    /// guard is identical, and that exactly two notices move.
    ///
    /// If this ever fails, `ROBOT_TIER` has stopped being a label on a report and become an input
    /// to a safety-relevant value — which is the moment the assumption has to be replaced by an
    /// awarded tier rather than defended.
    #[test]
    fn the_ceilings_do_not_depend_on_the_tier_and_only_the_enforcement_moves() {
        let bench = Bench::new("tier-invariance");
        bench.describe(ARM).profile(PROFILE);

        let shape = |tier: Tier| {
            bench.resolve(tier).with_resolved(|limits, _| {
                (
                    limits.joint_velocity_max_rad_s,
                    limits.joint_torque_max_nm,
                    limits.base_velocity_max_m_s,
                    limits.max_delta_per_cycle,
                    limits
                        .active_guards
                        .iter()
                        .map(|guard| guard.as_str())
                        .collect::<Vec<_>>(),
                    limits
                        .unavailable_guards
                        .iter()
                        .map(|entry| entry.guard.as_str())
                        .collect::<Vec<_>>(),
                )
            })
        };
        assert_eq!(shape(Tier::Guest), shape(Tier::Host));
        assert_eq!(shape(Tier::Host), shape(Tier::Certified));

        // And the two things that do move, so this is not asserting that the tier is inert.
        let codes = |tier: Tier| notices(&bench.resolve(tier));
        assert!(codes(Tier::Guest).contains(&"advisory_only".to_string()));
        assert!(!codes(Tier::Host).contains(&"advisory_only".to_string()));
        assert!(codes(Tier::Host).contains(&"no_independent_stop".to_string()));
        assert!(!codes(Tier::Certified).contains(&"no_independent_stop".to_string()));
    }

    /// A robot nobody has described has no ceilings, and says so rather than having none quietly.
    #[test]
    fn an_undescribed_body_resolves_to_no_ceilings_and_a_notice_saying_why() {
        let bench = Bench::new("bare");
        let owned = bench.resolve(ROBOT_TIER);
        owned.with_resolved(|limits, envelope| {
            assert!(limits.joint_velocity_max_rad_s.is_none());
            assert!(limits.source.is_none());
            assert!(envelope.is_none());
        });
        assert!(notices(&owned).contains(&"no_envelope".to_string()));

        let document = to_json(&owned);
        assert_eq!(
            document
                .get("sources")
                .and_then(|s| s.get("envelope"))
                .and_then(Json::as_bool),
            Some(false)
        );
        // Not enforced, and the route says so on every answer rather than in a footnote nobody
        // reads: these are limits nothing on this edition applies.
        assert_eq!(document.get("enforced"), Some(&Json::Bool(false)));
    }

    /// §55.2's volumes reach the guard that reads them, which is the whole of this session's
    /// second half. Before it, `cell_workspace_layer` was computed and shown to a human.
    #[test]
    fn the_cells_volumes_make_the_workspace_guard_active() {
        let bench = Bench::new("cell-workspace");
        bench.describe(ARM);
        let (_, unavailable) = partition(&bench.resolve(ROBOT_TIER));
        assert!(
            unavailable.contains(&"workspace".to_string()),
            "with no profile there are no volumes: {unavailable:?}"
        );

        bench.profile(PROFILE);
        let (active, _) = partition(&bench.resolve(ROBOT_TIER));
        assert!(
            active.contains(&"workspace".to_string()),
            "a surveyed cell whose frame the graph reaches: {active:?}"
        );
    }

    /// §44.5, from this module's side: a cell naming a frame the body does not have leaves the
    /// guard unavailable **with the frame named**, rather than silently guarding nothing.
    #[test]
    fn a_cell_naming_a_frame_the_body_does_not_have_names_it() {
        let bench = Bench::new("cell-dangling");
        bench
            .describe(ARM)
            .profile(&PROFILE.replace(r#""frame": "tool0""#, r#""frame": "gripper""#));

        let missing = bench.resolve(ROBOT_TIER).with_resolved(|limits, _| {
            limits
                .unavailable_guards
                .iter()
                .find(|entry| entry.guard.as_str() == "workspace")
                .map(|entry| entry.missing.iter().copied().collect::<Vec<_>>().join(", "))
        });
        assert_eq!(missing.as_deref(), Some("gripper"));
    }

    /// §38.2 only ever narrows, and the attempt to widen is recorded before it is discarded.
    #[test]
    fn a_profile_narrows_and_a_widening_is_recorded_then_discarded() {
        let bench = Bench::new("narrowing");
        bench.describe(ARM).profile(PROFILE);
        bench.resolve(ROBOT_TIER).with_resolved(|limits, _| {
            assert_eq!(limits.joint_velocity_max_rad_s, Some(0.5), "the profile's");
            assert_eq!(limits.joint_torque_max_nm, Some(80.0), "the envelope's");
        });

        bench.profile(&PROFILE.replace("0.5", "9.0"));
        let owned = bench.resolve(ROBOT_TIER);
        owned.with_resolved(|limits, _| {
            assert_eq!(
                limits.joint_velocity_max_rad_s,
                Some(1.0),
                "a profile cannot raise the envelope's ceiling"
            );
        });
        assert!(notices(&owned).contains(&"profile_widened".to_string()));
    }

    /// The three inputs with no source, and the guards that report them.
    ///
    /// A camera is not a person detector, so `proximity` is unavailable on a body that has one and
    /// declares nothing — the vendor says whether their stream measures a person (§5), and this
    /// descriptor does not.
    #[test]
    fn a_camera_is_not_a_person_detector() {
        let bench = Bench::new("proximity");
        bench.describe(ARM).profile(PROFILE);
        let owned = bench.resolve(ROBOT_TIER);
        assert!(!owned.available.people_detection);
        assert!(owned.available.imu, "the descriptor declares one");

        let missing = owned.with_resolved(|limits, _| {
            limits
                .unavailable_guards
                .iter()
                .find(|entry| entry.guard.as_str() == "proximity")
                .map(|entry| entry.missing.iter().copied().collect::<Vec<_>>().join(", "))
        });
        assert_eq!(missing.as_deref(), Some("profile.proximity"));

        let document = to_json(&owned);
        assert_eq!(document.arr_or_empty("unmeasured").len(), 3);
    }

    /// A body whose vendor declared a person detector on a camera.
    ///
    /// Shared by the runtime-reading cases below, which differ only in what the ring counted.
    const SEEING_ARM_FOR_RUNTIME: &str = r#"{
        "robot_slug": "bench-arm",
        "base": "fixed",
        "dof": 1,
        "envelope": { "joint_velocity_max_rad_s": 1.0, "joint_torque_max_nm": 80,
                      "watchdog_ms": 100, "source": "vendor datasheet rev C" },
        "sensors": [{"name": "torso_imu", "modality": "imu"},
                    {"name": "head_cam", "modality": "rgb", "detects_people": true}],
        "joints": [{"name": "shoulder", "type": "revolute", "control_modes": ["position"]}]
    }"#;

    /// A detector that was filling the reading and stops moves the guard to **inert**.
    ///
    /// The case §80 raised and could not test, because `proximity_reading` was a constant `false`
    /// in two files. It is the runtime half of the gap `GuardsSighting`'s own header names — *"a
    /// declared sensor that fails at runtime; the descriptor goes on declaring it and nothing here
    /// notices"* — and the direction is the one that matters: the console stops drawing protection
    /// the robot no longer has, without anybody editing a descriptor.
    ///
    /// Nothing about the body changes between the two resolutions. The only difference is what the
    /// ring counted, which is the whole point.
    #[test]
    fn a_detector_that_stops_filling_the_reading_makes_the_guard_inert() {
        use crate::blackbox::ProximitySighting;

        let bench = Bench::new("proximity-runtime");
        bench
            .describe(SEEING_ARM_FOR_RUNTIME)
            .profile(PROFILE_WITH_PROXIMITY);

        let partition = |sighting: &ProximitySighting| {
            bench
                .resolve_seeing(ROBOT_TIER, sighting)
                .with_resolved(|limits, _| {
                    (
                        limits
                            .active_guards
                            .iter()
                            .any(|guard| guard.as_str() == "proximity"),
                        limits
                            .inert_guards
                            .iter()
                            .any(|entry| entry.guard.as_str() == "proximity"),
                        limits
                            .unavailable_guards
                            .iter()
                            .any(|entry| entry.guard.as_str() == "proximity"),
                    )
                })
        };

        // While it is producing. A detector slower than the control loop fills a minority of
        // cycles, which is the expected shape rather than a fault — 120 of 4,000 is a 30 Hz sensor
        // under a 1 kHz loop.
        let (active, inert, unavailable) = partition(&ProximitySighting::Counted {
            cycles: 4_000,
            filled: 120,
        });
        assert!(active, "a reading that is arriving makes the guard active");
        assert!(!inert);
        assert!(!unavailable);

        // The same robot, the same descriptor, one interval later, and the detector has stopped.
        let (active, inert, unavailable) = partition(&ProximitySighting::Counted {
            cycles: 4_000,
            filled: 0,
        });
        assert!(
            !active,
            "a guard whose reading stopped arriving is not protection"
        );
        assert!(
            inert,
            "and it is inert rather than unavailable — the sensor is still declared and fitted"
        );
        assert!(
            !unavailable,
            "`unavailable` would send an operator to fit a sensor that is already there (§80)"
        );
    }

    /// A reading nobody counted is not a reading that arrived.
    ///
    /// Fail-closed, and worth asserting explicitly because the other direction is available and
    /// wrong: leaving the guard active on no evidence is a robot claiming to watch for people on
    /// the strength of a file it could not open.
    #[test]
    fn a_proximity_reading_nobody_counted_leaves_the_guard_inert() {
        use crate::blackbox::ProximitySighting;

        let bench = Bench::new("proximity-unobserved");
        bench
            .describe(SEEING_ARM_FOR_RUNTIME)
            .profile(PROFILE_WITH_PROXIMITY);

        for sighting in [
            ProximitySighting::Unobservable("no ring"),
            ProximitySighting::Counted {
                cycles: 0,
                filled: 0,
            },
        ] {
            let active = bench
                .resolve_seeing(ROBOT_TIER, &sighting)
                .with_resolved(|limits, _| {
                    limits
                        .active_guards
                        .iter()
                        .any(|guard| guard.as_str() == "proximity")
                });
            assert!(!active, "{sighting:?} must not resolve the guard active");
        }
    }

    /// A vendor declares a detector, and the guard goes active on a robot that cannot see anyone.
    ///
    /// This is the honest half of §5's field and the reason `unmeasured` names
    /// `cycle.nearest_person_m` rather than `available.people_detection`. The declaration is a
    /// claim about the **body**, and it is now answerable; the guard acts on a **number**, and
    /// nothing on this edition produces one. So `proximity` moves out of `unavailable_guards` —
    /// which is correct, because the input it was naming is no longer missing — and into
    /// `active_guards`, where it will never trip.
    ///
    /// The assertion worth having is the last one. A robot that lists the guard as active and does
    /// not say why it is inert would be a robot reporting speed-and-separation monitoring it is
    /// not doing, and that is a worse failure than the one §5 closed.
    #[test]
    fn a_declared_detector_with_no_producer_resolves_inert() {
        const SEEING_ARM: &str = r#"{
            "robot_slug": "bench-arm",
            "base": "fixed",
            "dof": 1,
            "envelope": { "joint_velocity_max_rad_s": 1.0, "joint_torque_max_nm": 80,
                          "watchdog_ms": 100, "source": "vendor datasheet rev C" },
            "sensors": [{"name": "torso_imu", "modality": "imu"},
                        {"name": "head_cam", "modality": "rgb", "detects_people": true}],
            "joints": [{"name": "shoulder", "type": "revolute", "control_modes": ["position"]}]
        }"#;

        let bench = Bench::new("declared-detector");
        bench.describe(SEEING_ARM).profile(PROFILE_WITH_PROXIMITY);
        let owned = bench.resolve(ROBOT_TIER);
        assert!(
            owned.available.people_detection,
            "the vendor declared one on the head camera"
        );

        let (active, missing, inert) = owned.with_resolved(|limits, _| {
            (
                limits
                    .active_guards
                    .iter()
                    .any(|guard| guard.as_str() == "proximity"),
                limits
                    .unavailable_guards
                    .iter()
                    .any(|entry| entry.guard.as_str() == "proximity"),
                limits
                    .inert_guards
                    .iter()
                    .find(|entry| entry.guard.as_str() == "proximity")
                    .map(|entry| entry.reading),
            )
        });

        // The three assertions are the decision of 2026-09-07, and the first one is the one that
        // changed. This test was written when §7.2 had two states, and it asserted **active** —
        // correctly, under a partition that could not express this robot. That is what §80 raised:
        // every input the resolver can see is present, nothing fills the reading, and *active* is
        // what every surface renders as protection.
        assert!(
            !active,
            "a guard whose reading nothing produces is not one that will run"
        );
        assert!(
            !missing,
            "and it is not unavailable either — both of its declared inputs are present, and the \
             remedy for an unavailable guard is a sensor this robot has"
        );
        assert_eq!(
            inert,
            Some("cycle.nearest_person_m"),
            "it is inert, and it names the reading it is waiting for (§64, §80)"
        );

        // And the robot still says so in `unmeasured`, which is the sentence that existed before
        // the state did. Kept rather than replaced: the partition tells a console how to draw the
        // guard, and this tells a person which reading is absent.
        let document = to_json(&owned);
        assert!(
            document
                .arr_or_empty("unmeasured")
                .iter()
                .filter_map(Json::as_str)
                .any(|entry| entry.starts_with("cycle.nearest_person_m")),
            "an inert guard has to name what it is waiting for"
        );
    }

    /// `balance` is not applicable to a fixed base, and the wording says which — so nobody reads a
    /// shape difference as a defect (§7.2).
    #[test]
    fn a_fixed_base_is_not_a_balance_failure() {
        let bench = Bench::new("balance");
        bench.describe(ARM);
        let missing = bench.resolve(ROBOT_TIER).with_resolved(|limits, _| {
            limits
                .unavailable_guards
                .iter()
                .find(|entry| entry.guard.as_str() == "balance")
                .map(|entry| entry.missing.iter().copied().collect::<Vec<_>>().join(", "))
        });
        assert!(
            missing
                .as_deref()
                .is_some_and(|m| m.contains("not applicable")),
            "{missing:?}"
        );
    }

    /// §6.3's hand-off, and the rule that makes it safe to fold in whichever package is named: a
    /// clamp can only narrow.
    #[test]
    fn a_transport_clamp_narrows_and_never_widens() {
        let bench = Bench::new("clamp");
        bench.describe(ARM);
        let clamp = PlanClamp {
            joint_velocity_max_rad_s: Some(9.0),
            joint_torque_max_nm: Some(10.0),
            base_velocity_max_m_s: None,
            min_control_hz: Some(50.0),
            // Deliberately out of order: the plan is a document with an order of its own and the
            // kernel's interface asks for sorted.
            per_channel: vec![("wrist".to_string(), 0.2), ("elbow".to_string(), 0.4)],
            source: Some("plan".to_string()),
        };
        let owned = bench.resolve(ROBOT_TIER).with_package("acme/walk", clamp);
        owned.with_resolved(|limits, _| {
            assert_eq!(limits.joint_velocity_max_rad_s, Some(1.0), "the envelope's");
            assert_eq!(limits.joint_torque_max_nm, Some(10.0), "the plan's");
            assert_eq!(limits.min_control_hz, Some(50.0));
            assert_eq!(
                limits.per_channel,
                &[("elbow", 0.4), ("wrist", 0.2)],
                "sorted by name, as the interface asks"
            );
            assert_eq!(limits.ceiling_for("wrist"), Some(0.2));
        });
        assert_eq!(
            to_json(&owned)
                .get("sources")
                .and_then(|s| s.get("transport_of"))
                .and_then(Json::as_str),
            Some("acme/walk")
        );
    }

    /// The terminal rendering names what each unavailable guard is waiting for, so the output is a
    /// list of things to go and fix rather than a list of things that are wrong.
    #[test]
    fn the_terminal_form_names_what_each_missing_guard_needs() {
        let bench = Bench::new("lines");
        bench.describe(ARM).profile(PROFILE);
        let lines = to_lines(&bench.resolve(ROBOT_TIER)).join("\n");
        assert!(lines.contains("guest"), "{lines}");
        assert!(lines.contains("vendor datasheet rev C"), "{lines}");
        assert!(lines.contains("proximity"), "{lines}");
        assert!(lines.contains("guards would run"), "{lines}");
    }
}

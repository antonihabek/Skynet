//! The Safety Kernel.
//!
//! One invariant holds the entire operating system together:
//!
//! > **The Safety Kernel is the only component with write access to an actuator.**
//!
//! A policy does not command a joint. It writes a *desired* command into a ring the kernel reads,
//! and the kernel decides what reaches the motors. That is not a stylistic preference. The Hub's
//! artifact scanner already reports that most published checkpoints are pickles, and unpickling
//! executes code by construction; a design where loading a model can move an arm makes every
//! published model a remote code execution vulnerability bolted to a two-metre humanoid. Sole
//! writership is what lets Skynet run untrusted policies *at all*, and running untrusted policies
//! is the entire point of having a catalogue.
//!
//! # Why this crate looks the way it does
//!
//! * **`#![no_std]`, and `alloc` is not linked.** §23.1: omitting the allocator makes "this never
//!   allocates" a compile-time property rather than a code-review promise. One source builds for a
//!   Linux RT thread at the host tier and for a bare-metal Cortex-M/R at the certified tier; two
//!   implementations of a safety kernel is two safety kernels, and the one that gets less
//!   attention is the one on the MCU.
//! * **It takes no clock.** `now_ms` is a parameter (§35.2). Three things fall out of that: the
//!   kernel is trivially testable because a cycle vector carries its own time; deterministic replay
//!   works because replay supplies the recorded timestamps; and simulation works with no special
//!   case, because a simulator RHAL driver's clock simply *is* the kernel's clock.
//! * **It is arithmetic on declared numbers.** No model, no network, no I/O. The whole crate is a
//!   few hundred lines because the component that must be identical across editions has to stay
//!   small enough that several teams can keep it identical (§31.5).
//!
//! # The honesty rule
//!
//! A clamp is worth exactly what the scheduler underneath it can promise. Running as an ordinary
//! process beside a vendor stack, the kernel cannot guarantee it is scheduled before the next
//! control cycle, so it **cannot** promise enforcement — and this crate will not let it claim
//! otherwise. [`Tier::enforcement`] derives the mode; no caller can pass one in. An OS that
//! displayed "safety enforced" over a best-effort userspace process would be lying about the one
//! thing it must never lie about.

#![no_std]
#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod fixed;
pub mod kinematics;
pub mod real;

use fixed::FixedVec;

/// Largest number of command channels one cycle may carry.
///
/// A whole-body humanoid is roughly 30–60 actuated joints, so 128 leaves generous headroom while
/// keeping a cycle frame small enough to sit comfortably in cache. Exceeding it is a fault, never
/// a truncation — see [`fixed`].
pub const MAX_CHANNELS: usize = 128;

/// Largest number of distinct guards that can trip in one cycle. There are eight guards.
pub const MAX_TRIPPED: usize = 8;

/// Largest number of notices [`resolve_limits`] will report.
pub const MAX_NOTICES: usize = 24;

/// Largest number of subjects one notice names.
pub const MAX_SUBJECTS: usize = 8;

// ── Deployment tiers ─────────────────────────────────────────────────────────

/// How deeply Skynet OS is installed on a given robot.
///
/// The tiers exist because "universal" has to mean something defensible. There is no single binary
/// that boots on every robot ever built, and claiming one would be the sort of marketing fiction
/// this platform is built to displace. What *is* universal is the contract — the same packages,
/// planner, Hub and conformance suite — reached at three depths of integration, with each tier
/// stating plainly what it can and cannot promise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Runs beside the robot's existing stack. Observes and advises; enforces nothing.
    Guest,
    /// Skynet OS is the robot's onboard operating system and owns the control loop.
    Host,
    /// Host, plus an independent safety processor with its own stop path.
    Certified,
}

impl Tier {
    /// Derived, never supplied. A caller cannot pass in the enforcement mode it would prefer.
    pub fn enforcement(self) -> Enforcement {
        match self {
            Tier::Guest => Enforcement::Advisory,
            Tier::Host => Enforcement::Enforcing,
            Tier::Certified => Enforcement::EnforcingRedundant,
        }
    }

    /// Whether a stop path exists that does not depend on the application processor.
    pub fn independent_stop_path(self) -> bool {
        matches!(self, Tier::Certified)
    }

    /// Whether this tier can be *evidence in* a functional-safety case.
    ///
    /// Never "confers one". See [`SAFETY_CLAIM_RULE`], which is the sentence this product is not
    /// allowed to shorten.
    pub fn supports_safety_case(self) -> bool {
        matches!(self, Tier::Certified)
    }

    /// The wire spelling, for serialisation and logs.
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Guest => "guest",
            Tier::Host => "host",
            Tier::Certified => "certified",
        }
    }

    /// Parse the wire spelling. Unknown values are `None` rather than a default, because
    /// defaulting an unrecognised tier to anything would be picking a safety posture by accident.
    pub fn parse(text: &str) -> Option<Tier> {
        match text {
            "guest" => Some(Tier::Guest),
            "host" => Some(Tier::Host),
            "certified" => Some(Tier::Certified),
            _ => None,
        }
    }
}

/// What the kernel does with a violation.
///
/// [`Enforcement::Advisory`] is not a weaker enforcement setting; it is the *absence* of
/// enforcement, recorded honestly. In advisory mode the kernel computes every verdict, logs every
/// violation and raises every alarm — and the command still reaches the vendor stack unchanged,
/// because there is nothing underneath that Skynet controls. The value is real (an operator learns
/// their policy commands 140% of rated torque *before* trusting it with anything) and it is not a
/// safety function, and every surface says so in those words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enforcement {
    /// Detected and recorded; nothing is enforced.
    Advisory,
    /// Enforced by this kernel.
    Enforcing,
    /// Enforced here and mirrored on an independent safety processor.
    EnforcingRedundant,
}

impl Enforcement {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Enforcement::Advisory => "advisory",
            Enforcement::Enforcing => "enforcing",
            Enforcement::EnforcingRedundant => "enforcing_redundant",
        }
    }
}

/// The sentence the product is not allowed to shorten.
///
/// A tier is an architecture, not a certificate. Skynet OS can be a component of somebody's ISO
/// 10218 or ISO 13482 safety case; it cannot confer one, because conformity assessment is done by
/// a notified body against a specific machine in a specific application, and no amount of software
/// architecture substitutes for that. Every surface mentioning the certified tier carries this.
pub const SAFETY_CLAIM_RULE: &str = concat!(
    "Skynet OS supplies evidence for a safety case. It does not certify a machine. Conformity ",
    "assessment against ISO 10218, ISO 13482 or any other standard is performed by a notified ",
    "body against a specific robot in a specific application."
);

// ── Guards ───────────────────────────────────────────────────────────────────

/// The guard set, in the order the kernel evaluates it each cycle.
///
/// **Order is part of the specification, not an implementation detail.** The watchdog runs first
/// because a stale command must be *discarded*, never clamped and forwarded — clamping it would
/// keep a dead producer's last intention alive at a legal magnitude, which is exactly the failure
/// the watchdog exists to prevent. Rate limiting runs *after* magnitude clamping because clamping
/// creates step changes of its own; getting this order wrong yields a kernel that passes every
/// unit test and still whipcracks an arm the first time it clamps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Guard {
    /// A command arrived inside the window and its sequence advanced.
    Watchdog,
    /// Per-joint position, velocity, torque vs. rating and transport clamp.
    Envelope,
    /// Change per cycle vs. the jerk ceiling.
    RateLimit,
    /// Commanded pose vs. operator-defined allowed volumes.
    Workspace,
    /// Commanded configuration vs. the body's collision model, one step lookahead.
    SelfCollision,
    /// Centre of pressure inside the support polygon.
    Balance,
    /// Commanded speed vs. distance to the nearest detected person.
    Proximity,
    /// The command's capability is one the operator granted this package.
    ModeGate,
}

/// Every guard, in evaluation order. Iterating this is the only sanctioned order.
pub const GUARDS: [Guard; 8] = [
    Guard::Watchdog,
    Guard::Envelope,
    Guard::RateLimit,
    Guard::Workspace,
    Guard::SelfCollision,
    Guard::Balance,
    Guard::Proximity,
    Guard::ModeGate,
];

impl Guard {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Guard::Watchdog => "watchdog",
            Guard::Envelope => "envelope",
            Guard::RateLimit => "rate_limit",
            Guard::Workspace => "workspace",
            Guard::SelfCollision => "self_collision",
            Guard::Balance => "balance",
            Guard::Proximity => "proximity",
            Guard::ModeGate => "mode_gate",
        }
    }

    /// What this guard does when its check fails.
    pub fn response(self) -> FaultResponse {
        match self {
            Guard::Watchdog => FaultResponse::StopCat2,
            Guard::Envelope => FaultResponse::Clamp,
            Guard::RateLimit => FaultResponse::Clamp,
            Guard::Workspace => FaultResponse::Hold,
            Guard::SelfCollision => FaultResponse::Hold,
            Guard::Balance => FaultResponse::StopCat2,
            Guard::Proximity => FaultResponse::StopCat1,
            Guard::ModeGate => FaultResponse::Hold,
        }
    }
}

/// Graduated responses, weakest to strongest, following IEC 60204-1.
///
/// A kernel whose only response is a hard stop is a kernel operators disable, because a humanoid
/// that drops a crate whenever a velocity estimate blips is worse than no policy at all. Most
/// violations are clamped and logged; stopping is reserved for the cases where continuing is the
/// dangerous option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FaultResponse {
    /// Command reduced to the ceiling and forwarded. Logged.
    Clamp,
    /// Last safe command held; the policy is given one cycle to recover.
    Hold,
    /// Controlled ramp to zero velocity, power retained, brakes engaged.
    StopCat2,
    /// Controlled ramp to zero, then drive power removed.
    StopCat1,
    /// Drive power removed immediately, uncontrolled.
    StopCat0,
}

impl FaultResponse {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            FaultResponse::Clamp => "clamp",
            FaultResponse::Hold => "hold",
            FaultResponse::StopCat2 => "stop_cat2",
            FaultResponse::StopCat1 => "stop_cat1",
            FaultResponse::StopCat0 => "stop_cat0",
        }
    }
}

// ── Inputs ───────────────────────────────────────────────────────────────────

/// The robot's declared safety envelope.
///
/// Every field is optional and `None` means **nobody declared it**, which is a different fact from
/// "there is no limit" and is treated as such everywhere. This mirrors the Hub's `SafetyEnvelope`,
/// whose `limits` column is nullable for exactly this reason.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Envelope<'a> {
    /// Per-joint angular velocity ceiling.
    pub joint_velocity_max_rad_s: Option<f64>,
    /// Per-joint torque ceiling.
    pub joint_torque_max_nm: Option<f64>,
    /// Whole-body translation ceiling.
    pub base_velocity_max_m_s: Option<f64>,
    /// Command timeout before the controller faults, in milliseconds.
    pub watchdog_ms: Option<f64>,
    /// Whether a hardware emergency stop is fitted.
    pub estop: Option<bool>,
    /// Where the figures came from. A limit with no provenance is a rumour.
    pub source: Option<&'a str>,
}

/// Speed-and-separation thresholds for the proximity guard.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Proximity {
    /// Below this separation the robot stops.
    pub stop_distance_m: f64,
    /// Below this separation the robot is slowed by `slow_factor`.
    pub slow_distance_m: f64,
    /// Multiplier applied between `stop_distance_m` and `slow_distance_m`.
    pub slow_factor: f64,
}

/// An operator's overrides.
///
/// Only ever downward. A profile may narrow what the vendor rated; it may never widen it, and
/// [`resolve_limits`] enforces that by taking the minimum rather than trusting the input. That is
/// the difference between an operator saying "in this cell, half speed" and an operator being able
/// to talk a robot past its own datasheet.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OperatorProfile<'a> {
    /// Profile name, for the notice text.
    pub name: &'a str,
    /// Narrowed joint velocity ceiling.
    pub joint_velocity_max_rad_s: Option<f64>,
    /// Narrowed joint torque ceiling.
    pub joint_torque_max_nm: Option<f64>,
    /// Narrowed base velocity ceiling.
    pub base_velocity_max_m_s: Option<f64>,
    /// Maximum change per control cycle, as a fraction of the ceiling. Jerk limiting.
    pub max_delta_per_cycle: Option<f64>,
    /// Speed and separation monitoring thresholds.
    pub proximity: Option<Proximity>,
    /// Where the operator will allow named frames to be (§7.1's fourth guard).
    ///
    /// Layers compose by conjunction, so an added layer can only narrow — see
    /// [`kinematics::WorkspaceLayer`], where the reasoning lives with the geometry.
    pub workspace: &'a [kinematics::WorkspaceLayer<'a>],
    /// How close two collision bodies may come before the fifth guard holds, in metres.
    ///
    /// Absent means zero, which is a legal setting and a late one: the surfaces have to actually
    /// meet before anything happens, and by then the command that put them there has been sent.
    /// [`NoticeCode::NoCollisionMargin`] says so, in the same way the missing jerk ceiling does.
    pub self_collision_margin_m: Option<f64>,
}

/// The ceilings a transport plan derived from the body and the policy's declared peaks.
#[derive(Debug, Clone, Copy, Default)]
pub struct TransportClamp<'a> {
    /// Whole-body joint velocity ceiling from the envelope.
    pub joint_velocity_max_rad_s: Option<f64>,
    /// Whole-body joint torque ceiling from the envelope.
    pub joint_torque_max_nm: Option<f64>,
    /// Base velocity ceiling from the envelope.
    pub base_velocity_max_m_s: Option<f64>,
    /// Slowest loop rate the controller's watchdog tolerates.
    pub min_control_hz: Option<f64>,
    /// Per-channel ceilings, keyed by target joint name. Must be sorted by name.
    pub per_channel: &'a [(&'a str, f64)],
    /// Provenance of the envelope figures.
    pub source: Option<&'a str>,
}

/// What this robot can actually supply, for deciding which guards can run.
///
/// Three flags, and it used to be five. `solvers` and `collision_model` were removed when §44's
/// frame graph arrived, and the reason is worth more than the two lines it saves: both were
/// **claims about the body that nothing checked**. A descriptor could say `collision_model: true`
/// with no geometry in it and the guard would report itself active while having nothing to
/// evaluate — which is §7.2's failure in its most damaging form, a guard reported as running that
/// is not. The frame set is the evidence, so the frame set is what is asked.
///
/// `solvers` was worse than unchecked, it was wrong: it named an *inverse*-kinematics solver, and
/// §44.2 gives the `workspace` guard forward kinematics computed in-cycle by the kernel itself.
/// A robot with no IK solver at all can run the guard perfectly well.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Available {
    /// An inertial measurement unit is fitted and reporting.
    pub imu: bool,
    /// Some sensor detects people.
    pub people_detection: bool,
    /// The body has legs, so balance is applicable at all.
    pub legged: bool,
}

/// Everything [`resolve_limits`] needs.
#[derive(Debug, Clone, Copy)]
pub struct ResolveLimitsParams<'a> {
    /// The tier this robot is running at.
    pub tier: Tier,
    /// The robot's declared envelope, or `None` when nobody declared one.
    pub envelope: Option<Envelope<'a>>,
    /// The transport plan's ceilings, or `None` when no plan has been resolved.
    pub transport: Option<TransportClamp<'a>>,
    /// The operator's profile, or `None`.
    pub profile: Option<OperatorProfile<'a>>,
    /// What the robot can supply.
    pub available: Available,
    /// The body's frames, flattened and ordered (§44.3), or `None` where it declares no tree.
    ///
    /// Absent, null and empty are three different bodies in the descriptor and all three arrive
    /// here as `None`, because all three mean the same thing to a guard: there is nothing to
    /// compute a pose from. Which of the three it was is the descriptor validator's story to tell,
    /// and it tells it once rather than in every cycle.
    pub frames: Option<kinematics::FrameSet<'a>>,
}

// ── Output ───────────────────────────────────────────────────────────────────

/// Why an operator is being told something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeCode {
    /// The tier cannot enforce; verdicts are recorded and nothing is stopped.
    AdvisoryOnly,
    /// No safety envelope was declared for this robot.
    NoEnvelope,
    /// No stop path exists that survives the application processor hanging.
    NoIndependentStop,
    /// The declared limits cite no datasheet.
    UnsourcedLimits,
    /// A guard cannot run here; the subjects name the missing inputs.
    GuardUnavailable,
    /// The operator profile asked to raise a limit. The request was recorded and discarded.
    ProfileWidened,
    /// No jerk ceiling is configured.
    NoRateLimit,
    /// No self-collision margin is configured, so surfaces must meet before the guard trips.
    NoCollisionMargin,
}

impl NoticeCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            NoticeCode::AdvisoryOnly => "advisory_only",
            NoticeCode::NoEnvelope => "no_envelope",
            NoticeCode::NoIndependentStop => "no_independent_stop",
            NoticeCode::UnsourcedLimits => "unsourced_limits",
            NoticeCode::GuardUnavailable => "guard_unavailable",
            NoticeCode::ProfileWidened => "profile_widened",
            NoticeCode::NoRateLimit => "no_rate_limit",
            NoticeCode::NoCollisionMargin => "no_collision_margin",
        }
    }
}

/// One thing an operator must read before this runs.
///
/// The notice carries a code and its subjects but no rendered sentence: formatting English is a
/// job for a layer that has an allocator and a translation catalogue, and the kernel has neither.
#[derive(Debug, Clone)]
pub struct SafetyNotice<'a> {
    /// Why.
    pub code: NoticeCode,
    /// The guard this notice is about, when it is about one.
    pub guard: Option<Guard>,
    /// The specific things named — missing inputs, widened fields.
    pub subjects: FixedVec<&'a str, MAX_SUBJECTS>,
}

/// A guard that is specified but cannot run here, with the inputs that are missing.
#[derive(Debug, Clone)]
pub struct UnavailableGuard<'a> {
    /// Which guard.
    pub guard: Guard,
    /// What it needs and does not have.
    pub missing: FixedVec<&'a str, MAX_SUBJECTS>,
}

/// The single set of ceilings the kernel enforces, and what it can and cannot check.
#[derive(Debug, Clone)]
pub struct ResolvedLimits<'a> {
    /// The tier these limits were resolved at.
    pub tier: Tier,
    /// Derived from the tier.
    pub enforcement: Enforcement,
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
    /// Speed-and-separation thresholds, when the operator configured them.
    ///
    /// Carried on the resolved set rather than read from the profile at cycle time, so that
    /// [`resolve_limits`] stays the only place a limit is chosen.
    pub proximity: Option<Proximity>,
    /// Per-channel ceilings, borrowed from the transport clamp.
    pub per_channel: &'a [(&'a str, f64)],
    /// Guards that will actually run.
    pub active_guards: FixedVec<Guard, MAX_TRIPPED>,
    /// Guards specified but inoperable here, with the input that is missing.
    pub unavailable_guards: FixedVec<UnavailableGuard<'a>, MAX_TRIPPED>,
    /// Provenance of the numbers. `None` when nobody cited one.
    pub source: Option<&'a str>,
    /// Things an operator must read.
    pub notices: FixedVec<SafetyNotice<'a>, MAX_NOTICES>,
    /// Capability tokens the operator granted the running package, for the mode gate.
    pub grants: &'a [&'a str],
    /// The body's frames, when it declares a tree the kernel can walk.
    pub frames: Option<kinematics::FrameSet<'a>>,
    /// The operator's allowed volumes, layered.
    pub workspace: &'a [kinematics::WorkspaceLayer<'a>],
    /// The configured self-collision margin, in metres. `None` is zero, with a notice.
    pub self_collision_margin_m: Option<f64>,
}

impl<'a> ResolvedLimits<'a> {
    /// Whether a guard will run.
    pub fn is_active(&self, guard: Guard) -> bool {
        self.active_guards.contains(&guard)
    }

    /// The ceiling that applies to one channel.
    ///
    /// Per-channel first, then the base-velocity ceiling for the reserved `base` channel. A
    /// channel with neither has no ceiling here, and the kernel does not invent one: the
    /// whole-body velocity and torque figures are not applied per channel because nothing at this
    /// layer knows whether a given channel carries radians per second or newton-metres. That
    /// knowledge lives in the transport plan, which is why the plan's `per_channel` is where a
    /// joint's real ceiling comes from.
    pub fn ceiling_for(&self, channel: &str) -> Option<f64> {
        if let Some((_, limit)) = self.per_channel.iter().find(|(name, _)| *name == channel) {
            return Some(*limit);
        }
        if channel == "base" {
            return self.base_velocity_max_m_s;
        }
        None
    }
}

// ── Limit resolution ─────────────────────────────────────────────────────────

/// Narrowest of the values present. Absent inputs never widen the result.
fn narrowest(values: &[Option<f64>]) -> Option<f64> {
    let mut best: Option<f64> = None;
    for value in values.iter().flatten() {
        if !value.is_finite() {
            continue;
        }
        best = Some(match best {
            Some(current) if current <= *value => current,
            _ => *value,
        });
    }
    best
}

/// Append a notice. A free function rather than a closure so the borrow of `notices` ends with
/// each call, which keeps the widened-limits block below able to move its own list in whole.
fn push_notice<'a>(
    notices: &mut FixedVec<SafetyNotice<'a>, MAX_NOTICES>,
    code: NoticeCode,
    guard: Option<Guard>,
    subjects: &[&'a str],
) {
    let mut list: FixedVec<&'a str, MAX_SUBJECTS> = FixedVec::new();
    for subject in subjects {
        list.push(subject);
    }
    notices.push(SafetyNotice {
        code,
        guard,
        subjects: list,
    });
}

/// Compose every source of ceilings into the single set the kernel enforces.
///
/// Three sources — the robot's declared envelope, the transport plan's per-channel limits, and the
/// operator's profile — and the rule between them is **always the narrowest, never a precedence
/// order**. Taking the minimum means no source can raise what another lowered, including a
/// mistaken or malicious operator profile, which is why the profile is folded in here rather than
/// applied afterwards. An attempt to widen is recorded as a [`NoticeCode::ProfileWidened`] notice
/// and then discarded by the minimum: silently ignoring it would leave an operator believing a
/// limit they set is in force.
pub fn resolve_limits<'a>(params: &ResolveLimitsParams<'a>) -> ResolvedLimits<'a> {
    let ResolveLimitsParams {
        tier,
        envelope,
        // Read by `partition_guards`, alongside `available`, for the same reason.
        frames: _,
        transport,
        profile,
        // `available` is read by `partition_guards`, which takes the whole params struct so the
        // guard-availability rules stay in one place rather than being split across two functions.
        available: _,
    } = *params;

    let enforcement = tier.enforcement();
    let mut notices: FixedVec<SafetyNotice<'a>, MAX_NOTICES> = FixedVec::new();

    if enforcement == Enforcement::Advisory {
        push_notice(&mut notices, NoticeCode::AdvisoryOnly, None, &[]);
    }
    if !tier.independent_stop_path() {
        push_notice(&mut notices, NoticeCode::NoIndependentStop, None, &[]);
    }

    match envelope {
        None => push_notice(&mut notices, NoticeCode::NoEnvelope, None, &[]),
        Some(env) if env.source.is_none() => {
            push_notice(&mut notices, NoticeCode::UnsourcedLimits, None, &[])
        }
        Some(_) => {}
    }

    // Widening attempts are recorded, then discarded by the minimum below.
    let mut widened: FixedVec<&'a str, MAX_SUBJECTS> = FixedVec::new();
    let mut check_widen = |label: &'a str, requested: Option<f64>, ceiling: Option<f64>| {
        if let (Some(requested), Some(ceiling)) = (requested, ceiling) {
            if requested > ceiling {
                widened.push(label);
            }
        }
    };
    check_widen(
        "joint_velocity_max_rad_s",
        profile.and_then(|p| p.joint_velocity_max_rad_s),
        envelope.and_then(|e| e.joint_velocity_max_rad_s),
    );
    check_widen(
        "joint_torque_max_nm",
        profile.and_then(|p| p.joint_torque_max_nm),
        envelope.and_then(|e| e.joint_torque_max_nm),
    );
    check_widen(
        "base_velocity_max_m_s",
        profile.and_then(|p| p.base_velocity_max_m_s),
        envelope.and_then(|e| e.base_velocity_max_m_s),
    );
    if !widened.is_empty() {
        notices.push(SafetyNotice {
            code: NoticeCode::ProfileWidened,
            guard: None,
            subjects: widened,
        });
    }

    let joint_velocity = narrowest(&[
        envelope.and_then(|e| e.joint_velocity_max_rad_s),
        transport.and_then(|t| t.joint_velocity_max_rad_s),
        profile.and_then(|p| p.joint_velocity_max_rad_s),
    ]);
    let joint_torque = narrowest(&[
        envelope.and_then(|e| e.joint_torque_max_nm),
        transport.and_then(|t| t.joint_torque_max_nm),
        profile.and_then(|p| p.joint_torque_max_nm),
    ]);
    let base_velocity = narrowest(&[
        envelope.and_then(|e| e.base_velocity_max_m_s),
        transport.and_then(|t| t.base_velocity_max_m_s),
        profile.and_then(|p| p.base_velocity_max_m_s),
    ]);

    let max_delta = profile.and_then(|p| p.max_delta_per_cycle);
    if max_delta.is_none() {
        // No subjects: the sentence already names the setting, and the `rate_limit` guard's own
        // unavailability notice below carries `profile.max_delta_per_cycle` as its missing input.
        // Naming it twice would put the same fact in two places for a reader to reconcile.
        push_notice(&mut notices, NoticeCode::NoRateLimit, None, &[]);
    }

    let (active, unavailable) = partition_guards(params, max_delta);

    // Emitted only when the guard it is about will actually run. A robot with no collision geometry
    // is already being told the guard is unavailable, and adding "and its margin is unset" to that
    // is a second sentence about a thing that is not happening.
    if active.contains(&Guard::SelfCollision)
        && profile.and_then(|p| p.self_collision_margin_m).is_none()
    {
        push_notice(
            &mut notices,
            NoticeCode::NoCollisionMargin,
            Some(Guard::SelfCollision),
            &[],
        );
    }

    for entry in unavailable.iter() {
        let mut list: FixedVec<&'a str, MAX_SUBJECTS> = FixedVec::new();
        for missing in entry.missing.iter() {
            list.push(missing);
        }
        notices.push(SafetyNotice {
            code: NoticeCode::GuardUnavailable,
            guard: Some(entry.guard),
            subjects: list,
        });
    }

    ResolvedLimits {
        tier,
        enforcement,
        joint_velocity_max_rad_s: joint_velocity,
        joint_torque_max_nm: joint_torque,
        base_velocity_max_m_s: base_velocity,
        min_control_hz: transport.and_then(|t| t.min_control_hz),
        max_delta_per_cycle: max_delta,
        proximity: profile.and_then(|p| p.proximity),
        per_channel: transport.map(|t| t.per_channel).unwrap_or(&[]),
        active_guards: active,
        unavailable_guards: unavailable,
        source: envelope
            .and_then(|e| e.source)
            .or_else(|| transport.and_then(|t| t.source)),
        notices,
        grants: &[],
        frames: params.frames,
        workspace: params.profile.map(|p| p.workspace).unwrap_or(&[]),
        self_collision_margin_m: params.profile.and_then(|p| p.self_collision_margin_m),
    }
}

impl<'a> ResolvedLimits<'a> {
    /// Attach the capability grants the mode gate checks against.
    ///
    /// Separate from [`resolve_limits`] because grants are a property of the *package* holding the
    /// lease, not of the body, and one resolved limit set serves every package on the robot.
    pub fn with_grants(mut self, grants: &'a [&'a str]) -> Self {
        self.grants = grants;
        self
    }
}

fn partition_guards<'a>(
    params: &ResolveLimitsParams<'a>,
    max_delta: Option<f64>,
) -> (
    FixedVec<Guard, MAX_TRIPPED>,
    FixedVec<UnavailableGuard<'a>, MAX_TRIPPED>,
) {
    let mut active: FixedVec<Guard, MAX_TRIPPED> = FixedVec::new();
    let mut unavailable: FixedVec<UnavailableGuard<'a>, MAX_TRIPPED> = FixedVec::new();

    for guard in GUARDS {
        let mut missing: FixedVec<&'a str, MAX_SUBJECTS> = FixedVec::new();
        match guard {
            Guard::Watchdog => {
                let has_window = params
                    .envelope
                    .and_then(|e| e.watchdog_ms)
                    .is_some_and(|ms| ms > 0.0);
                if !has_window {
                    missing.push("envelope.watchdog_ms");
                }
            }
            Guard::Envelope => {}
            Guard::RateLimit => {
                if max_delta.is_none() {
                    missing.push("profile.max_delta_per_cycle");
                }
            }
            // §44.5: a body whose descriptor declares no frame graph, or whose graph does not
            // reach the frame the operator constrained, reports these two unavailable **with the
            // missing frame named** — rather than evaluating them against a frame it guessed.
            Guard::Workspace => {
                let volumes: usize = params
                    .profile
                    .map(|profile| {
                        profile
                            .workspace
                            .iter()
                            .map(|layer| layer.volumes.len())
                            .sum()
                    })
                    .unwrap_or(0);
                if volumes == 0 {
                    missing.push("profile.workspace");
                }
                match params.frames {
                    None => {
                        missing.push("embodiment.frames");
                    }
                    Some(set) => {
                        if let Some(profile) = params.profile {
                            for layer in profile.workspace {
                                for volume in layer.volumes {
                                    // Absent, or present and behind a joint one scalar cannot
                                    // describe. Both are "the tree does not reach the frame the
                                    // operator constrained", and both are better said now than by
                                    // holding the robot on every cycle for the life of the lease.
                                    let reachable =
                                        set.index_of(volume.frame).is_some_and(|index| {
                                            set.statically_resolvable(index as usize)
                                        });
                                    if !reachable && !missing.contains(&volume.frame) {
                                        missing.push(volume.frame);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            Guard::SelfCollision => match params.frames {
                None => {
                    missing.push("embodiment.frames");
                }
                Some(set) => {
                    // Two, not one. A single collision body has nothing to collide with, and a
                    // guard reported active with no pair to test is a guard that passes by not
                    // looking — the same failure as a conformance suite treating an absent result
                    // as a pass.
                    if set.collision_bodies() < 2 {
                        missing.push("embodiment.frames[].collision, on two or more frames");
                    }
                }
            },
            Guard::Balance => {
                // Not applicable rather than missing on a wheeled base, and the wording says which,
                // so nobody reads a shape difference as a defect (§7.2).
                if !params.available.legged {
                    missing.push("a legged base — not applicable to this body");
                } else if !params.available.imu {
                    missing.push("an IMU");
                }
            }
            Guard::Proximity => {
                if params.profile.and_then(|p| p.proximity).is_none() {
                    missing.push("profile.proximity");
                } else if !params.available.people_detection {
                    missing.push("a sensor that detects people");
                }
            }
            Guard::ModeGate => {}
        }

        if missing.is_empty() {
            active.push(guard);
        } else {
            unavailable.push(UnavailableGuard { guard, missing });
        }
    }

    (active, unavailable)
}

// ── The per-cycle decision ───────────────────────────────────────────────────

/// One control cycle's worth of intent.
#[derive(Debug, Clone, Copy)]
pub struct CommandCycle<'a> {
    /// Monotonic tick supplied by the caller. The kernel owns no clock (§35.2).
    pub now_ms: f64,
    /// When the command the policy produced was stamped.
    pub command_ms: f64,
    /// Sequence number; must advance, or the command is a repeat of a stalled producer.
    pub sequence: u64,
    /// Desired values keyed by target joint or actuator name, sorted by name.
    pub desired: &'a [(&'a str, f64)],
    /// What was actually sent last cycle, for rate limiting.
    pub previous: Option<&'a [(&'a str, f64)]>,
    /// The capability token this command exercises, when the runtime supplied one.
    ///
    /// `None` means the caller did not say, and the mode gate does not evaluate — it does not
    /// mean "no capability". A guard with no input reports unavailable; it never passes by
    /// default (§7.2).
    pub capability: Option<&'a str>,
    /// Distance to the nearest detected person, in metres, when something is measuring it.
    pub nearest_person_m: Option<f64>,
}

/// One clamped channel, and by how much.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clamped<'a> {
    /// Which channel.
    pub channel: &'a str,
    /// What the policy asked for.
    pub from: f64,
    /// What the kernel will send.
    pub to: f64,
}

/// One pose-guard finding, with the frames it is about.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PoseTrip<'a> {
    /// [`Guard::Workspace`] or [`Guard::SelfCollision`].
    pub guard: Guard,
    /// The frame that left its volume, or the first of the colliding pair.
    pub frame: &'a str,
    /// The second of the pair, on a self-collision. `None` on a workspace excursion.
    pub other: Option<&'a str>,
    /// **Negative is a violation**, in metres — one sign convention across both guards, so a reader
    /// of a verdict does not have to remember which one measures which way.
    ///
    /// `None` when the frame had no pose this cycle. That is a trip in its own right: a guard that
    /// cannot establish where a constrained frame is has not shown the command to be safe, and
    /// §7.2's rule is that it says so rather than passing.
    pub margin_m: Option<f64>,
}

/// What the two pose guards found, and what they were able to look at.
///
/// Present on every verdict, empty or not, and that is deliberate. If it were omitted when nothing
/// tripped, an edition that has not implemented the pose guards at all would emit bytes identical
/// to an edition that ran them and found nothing — which is the "passes by not looking" failure
/// §31.4's corpus exists to make impossible.
#[derive(Debug, Clone, Default)]
pub struct PoseOutcome<'a> {
    /// Whether forward kinematics ran this cycle.
    ///
    /// False when no body was supplied, or when neither pose guard is active — in which case there
    /// is nothing for a few thousand floating-point operations to decide.
    pub evaluated: bool,
    /// Whether every joint value came from this cycle's command.
    ///
    /// §7.1 specifies these guards on the *commanded* configuration, one step ahead. On a body
    /// commanded in velocity or torque there is no such configuration to compute, and what ran was
    /// a check of where the robot is now — a real check, and a weaker claim, so the verdict makes
    /// the difference visible rather than letting a log imply the stronger one.
    pub lookahead: bool,
    /// Whether more trips occurred than [`kinematics::MAX_POSE_TRIPS`] could carry.
    ///
    /// The response is unaffected — one trip and twenty both `hold` — so this qualifies the report
    /// and never the decision.
    pub truncated: bool,
    /// Frames whose pose could not be computed this cycle, named (§44.5).
    pub unresolved: FixedVec<&'a str, { kinematics::MAX_UNRESOLVED }>,
    /// What tripped, in frame order.
    pub trips: FixedVec<PoseTrip<'a>, { kinematics::MAX_POSE_TRIPS }>,
}

/// The body the pose guards read, and the scratch they write into.
///
/// Passed at cycle time rather than carried on [`ResolvedLimits`] because two of its three parts
/// change every cycle: the measured joint state is this cycle's, and [`kinematics::Kinematics`] is
/// a buffer the caller owns so that the kernel allocates nothing. The runtime holds one of these
/// for the life of a lease and hands it back on each cycle.
pub struct Body<'a, 'k> {
    /// How each joint in the frame graph is driven, and where it is now.
    ///
    /// The runtime builds this from the transport plan, which is the only place that knows whether
    /// a channel carries a position, a velocity or a torque. The kernel does not guess.
    pub joints: &'k [kinematics::JointBinding<'a>],
    /// Caller-owned scratch, reused every cycle.
    pub scratch: &'k mut kinematics::Kinematics,
}

/// What the kernel decided this cycle.
#[derive(Debug, Clone)]
pub struct CycleVerdict<'a> {
    /// What the kernel will send. Empty under a hold or a stop.
    pub command: FixedVec<(&'a str, f64), MAX_CHANNELS>,
    /// The strongest response any tripped guard demanded, or `None` when nothing tripped.
    pub response: Option<FaultResponse>,
    /// Guards that tripped, in evaluation order.
    pub tripped: FixedVec<Guard, MAX_TRIPPED>,
    /// Channels whose values were reduced.
    pub clamped: FixedVec<Clamped<'a>, MAX_CHANNELS>,
    /// Whether the command was forwarded unchanged despite a trip, because the tier cannot
    /// enforce.
    ///
    /// Always false at host and certified. The field exists so a guest-tier log cannot be mistaken
    /// for an enforcement record.
    pub advisory: bool,
    /// What the two pose guards found.
    pub pose: PoseOutcome<'a>,
}

/// Evaluate one control cycle.
///
/// The hot path, and the whole of it. Linear in the number of channels, no allocation, no clock,
/// no I/O. The conformance corpus feeds identical cycle vectors to every edition and requires
/// identical verdicts, which is what `safety.kernel.parity` generalises to (§31.4).
pub fn evaluate_cycle<'a>(
    cycle: &CommandCycle<'a>,
    limits: &ResolvedLimits<'a>,
    envelope: Option<&Envelope<'a>>,
    last_sequence: Option<u64>,
) -> CycleVerdict<'a> {
    evaluate_cycle_with_body(cycle, limits, envelope, last_sequence, None)
}

/// Evaluate one control cycle, with the body the pose guards need.
///
/// The same hot path. [`evaluate_cycle`] is this with no body, which is the honest description of
/// a robot whose descriptor declares no frame graph: guards four and five reported themselves
/// unavailable when the lease was granted, and there is nothing here for them to do.
pub fn evaluate_cycle_with_body<'a>(
    cycle: &CommandCycle<'a>,
    limits: &ResolvedLimits<'a>,
    envelope: Option<&Envelope<'a>>,
    last_sequence: Option<u64>,
    body: Option<Body<'a, '_>>,
) -> CycleVerdict<'a> {
    let mut tripped: FixedVec<Guard, MAX_TRIPPED> = FixedVec::new();
    let mut clamped: FixedVec<Clamped<'a>, MAX_CHANNELS> = FixedVec::new();
    let advisory = limits.enforcement == Enforcement::Advisory;

    // The strongest response any guard *demanded this cycle* — not the strongest response any
    // tripped guard is capable of demanding, which is a different and weaker thing.
    //
    // The difference is `proximity`, whose §7.1 row reads `stop_cat1` because that is what it does
    // when separation falls below the stop distance. Between the two distances it does something
    // else: it scales the command and lets it through. Deriving the verdict from the guard's row
    // reported `stop_cat1` on a cycle where the robot kept moving at half speed — a verdict that
    // says "stopped" about a robot that was not is the one thing a black box must never record.
    let mut demanded: Option<FaultResponse> = None;

    // 1. Watchdog. A stale command is discarded, never clamped and forwarded — clamping it would
    //    keep a dead producer's last intention alive at a legal magnitude, which is the failure
    //    the watchdog exists to prevent.
    let stale = envelope
        .and_then(|e| e.watchdog_ms)
        .is_some_and(|window| cycle.now_ms - cycle.command_ms > window);
    let stalled = last_sequence.is_some_and(|last| cycle.sequence <= last);
    if limits.is_active(Guard::Watchdog) && (stale || stalled) {
        tripped.push(Guard::Watchdog);
        // The one guard that still short-circuits. Everything below evaluates a command; a stale
        // one is not a command, and running the pose guards on values the producer has stopped
        // updating would report where a robot would be if it were still being driven.
        return stop_verdict(
            cycle,
            FaultResponse::StopCat2,
            tripped,
            clamped,
            advisory,
            PoseOutcome::default(),
        );
    }

    let mut command: FixedVec<(&'a str, f64), MAX_CHANNELS> = FixedVec::new();

    // 2. Magnitude clamping against the resolved ceilings.
    for (channel, value) in cycle.desired.iter() {
        let ceiling = limits.ceiling_for(channel);
        let mut next = *value;
        if let Some(ceiling) = ceiling {
            if value.abs() > ceiling {
                next = signum(*value) * ceiling;
                tripped.push_unique(Guard::Envelope);
                demand(&mut demanded, FaultResponse::Clamp);
                clamped.push(Clamped {
                    channel,
                    from: *value,
                    to: next,
                });
            }
        }
        command.push((channel, next));
    }

    // 3. Rate limiting, after clamping, because clamping creates step changes of its own.
    if let (Some(max_delta), Some(previous)) = (limits.max_delta_per_cycle, cycle.previous) {
        for index in 0..command.len() {
            let (channel, value) = match command.get(index) {
                Some(entry) => *entry,
                None => continue,
            };
            let Some(prior) = previous
                .iter()
                .find(|(name, _)| *name == channel)
                .map(|(_, v)| *v)
            else {
                continue;
            };
            let max_step = match limits.ceiling_for(channel) {
                Some(ceiling) => ceiling * max_delta,
                None => prior.abs() * max_delta,
            };
            let delta = value - prior;
            if delta.abs() > max_step && max_step > 0.0 {
                let next = prior + signum(delta) * max_step;
                tripped.push_unique(Guard::RateLimit);
                demand(&mut demanded, FaultResponse::Clamp);
                clamped.push(Clamped {
                    channel,
                    from: value,
                    to: next,
                });
                if let Some(slot) = command.get_mut(index) {
                    *slot = (channel, next);
                }
            }
        }
    }

    // 4 and 5. The pose guards, on the configuration the command above would produce.
    //
    //    After clamping and rate limiting, and that ordering is the whole point: the pose that
    //    matters is the one the robot will actually be commanded into, not the one the policy
    //    asked for and the envelope refused. Checking the desired values would report an excursion
    //    the kernel had already prevented, on every cycle a clamp fired.
    let mut pose = PoseOutcome::default();
    let evaluate_pose =
        limits.is_active(Guard::Workspace) || limits.is_active(Guard::SelfCollision);
    if let (true, Some(set), Some(body)) = (evaluate_pose, limits.frames, body) {
        body.scratch.solve(&set, body.joints, |name| {
            command
                .iter()
                .find(|(channel, _)| *channel == name)
                .map(|(_, value)| *value)
        });
        pose.evaluated = true;
        pose.lookahead = body.scratch.lookahead();
        for index in body.scratch.unresolved().iter() {
            pose.unresolved.push(set.frames()[*index as usize].name);
        }
        if limits.is_active(Guard::Workspace) {
            check_workspace(&set, body.scratch, limits.workspace, &mut pose);
        }
        if limits.is_active(Guard::SelfCollision) {
            check_self_collision(
                &set,
                body.scratch,
                limits.self_collision_margin_m.unwrap_or(0.0),
                &mut pose,
            );
        }
        pose.truncated = pose.trips.overflowed();
        for trip in pose.trips.iter() {
            tripped.push_unique(trip.guard);
            demand(&mut demanded, trip.guard.response());
        }
    }

    // 7. Proximity — speed and separation monitoring, when something is measuring separation.
    //
    //    Evaluated after the magnitude and rate guards because it scales the command those two
    //    produced: applying it first would let a clamp raise a speed the separation had already
    //    reduced, and the composition rule everywhere in this system is that a later stage may
    //    only narrow.
    if limits.is_active(Guard::Proximity) {
        if let (Some(distance), Some(prox)) = (cycle.nearest_person_m, limits.proximity) {
            if distance <= prox.stop_distance_m {
                tripped.push_unique(Guard::Proximity);
                return stop_verdict(
                    cycle,
                    FaultResponse::StopCat1,
                    tripped,
                    clamped,
                    advisory,
                    pose,
                );
            }
            if distance <= prox.slow_distance_m && prox.slow_factor < 1.0 {
                tripped.push_unique(Guard::Proximity);
                // A reduction, not a stop. §7.1's row for this guard is what it does *inside* the
                // stop distance; between the two distances it is speed-and-separation monitoring
                // doing exactly its job, and the command it produced goes out.
                demand(&mut demanded, FaultResponse::Clamp);
                for index in 0..command.len() {
                    let (channel, value) = match command.get(index) {
                        Some(entry) => *entry,
                        None => continue,
                    };
                    let next = value * prox.slow_factor;
                    if next != value {
                        clamped.push(Clamped {
                            channel,
                            from: value,
                            to: next,
                        });
                        if let Some(slot) = command.get_mut(index) {
                            *slot = (channel, next);
                        }
                    }
                }
            }
        }
    }

    // 8. Mode gate — the capability this command exercises must be one the operator granted.
    //
    //    Enforced at every cycle rather than at start, so a revocation takes effect on the next
    //    command rather than on the next restart (§8.2).
    if limits.is_active(Guard::ModeGate) {
        if let Some(capability) = cycle.capability {
            if !limits.grants.contains(&capability) {
                tripped.push_unique(Guard::ModeGate);
                demand(&mut demanded, FaultResponse::Hold);
            }
        }
    }

    // A frame carrying more channels than the kernel can evaluate is a fault, not a truncation.
    if command.overflowed() || clamped.overflowed() {
        tripped.push_unique(Guard::Envelope);
        return stop_verdict(
            cycle,
            FaultResponse::StopCat2,
            tripped,
            clamped,
            advisory,
            pose,
        );
    }

    let response = demanded;

    // A hold means the command does not go out. That rule lives here, once, rather than at each
    // guard that demands one: `hold` is defined as the last safe command being held while the
    // policy is given a cycle to recover, and a kernel that forwarded a clamped command alongside
    // a `hold` verdict would be doing the opposite of what it reported. Making it a property of the
    // response also means a later guard can only ever strengthen the outcome — a workspace `hold`
    // no longer hides a proximity `stop_cat1` behind an early return.
    let withheld = response.is_some_and(|response| response >= FaultResponse::Hold);

    CycleVerdict {
        // In advisory mode the desired command is what reaches the robot. Returning the clamped
        // values here would make a guest-tier deployment look enforced in its own logs.
        command: if advisory {
            copy_of(cycle.desired)
        } else if withheld {
            FixedVec::new()
        } else {
            command
        },
        response,
        tripped,
        clamped,
        advisory,
        pose,
    }
}

/// §7.1's fourth guard: every constrained frame inside its operator-defined volumes.
fn check_workspace<'a>(
    set: &kinematics::FrameSet<'a>,
    solved: &kinematics::Kinematics,
    layers: &'a [kinematics::WorkspaceLayer<'a>],
    pose: &mut PoseOutcome<'a>,
) {
    for (index, frame) in set.frames().iter().enumerate() {
        // Layers compose by conjunction: the frame must satisfy every layer that names it, and the
        // reported margin is the worst of them. A layer that does not name this frame says nothing
        // about it, so a site rule about one arm does not confine the other.
        let mut constrained = false;
        let mut worst: Option<f64> = None;
        for layer in layers {
            let mut best: Option<f64> = None;
            for volume in layer.volumes {
                if volume.frame != frame.name {
                    continue;
                }
                constrained = true;
                if let Some(pose) = solved.pose(index) {
                    let margin = volume.shape.margin(&pose.translation);
                    best = Some(match best {
                        Some(current) if current >= margin => current,
                        _ => margin,
                    });
                }
            }
            if let Some(best) = best {
                worst = Some(match worst {
                    Some(current) if current <= best => current,
                    _ => best,
                });
            }
        }

        if !constrained {
            continue;
        }
        match (solved.pose(index), worst) {
            // Constrained and unlocatable. Not a pass: the guard has not shown the command safe,
            // and §7.2 is explicit that a guard whose input is absent reports rather than passes.
            (None, _) => pose.trips.push(PoseTrip {
                guard: Guard::Workspace,
                frame: frame.name,
                other: None,
                margin_m: None,
            }),
            (Some(_), Some(margin)) if margin < 0.0 => pose.trips.push(PoseTrip {
                guard: Guard::Workspace,
                frame: frame.name,
                other: None,
                margin_m: Some(margin),
            }),
            _ => false,
        };
    }
}

/// §7.1's fifth guard: no two collision bodies closer than the operator's margin.
///
/// Quadratic, and the pairs are visited in index order so that the reported list is the same on
/// every edition. Adjacent frames and declared exceptions are skipped before any arithmetic — a
/// humanoid's torso and pelvis overlap by design, and a guard that tripped on the robot standing
/// still is a guard an operator switches off.
fn check_self_collision<'a>(
    set: &kinematics::FrameSet<'a>,
    solved: &kinematics::Kinematics,
    margin_m: f64,
    pose: &mut PoseOutcome<'a>,
) {
    let frames = set.frames();
    let margin_m = if margin_m > 0.0 { margin_m } else { 0.0 };

    for i in 0..frames.len() {
        let Some(first) = frames[i].collision else {
            continue;
        };
        for j in (i + 1)..frames.len() {
            let Some(second) = frames[j].collision else {
                continue;
            };
            if kinematics::adjacent(frames, i, j) || kinematics::ignores(frames, i, j) {
                continue;
            }

            let (Some(pose_i), Some(pose_j)) = (solved.pose(i), solved.pose(j)) else {
                // One of the pair has no pose. Reported once, against the pair, rather than
                // silently skipped — a collision test that quietly excludes a limb is a test that
                // passes because it did not look.
                pose.trips.push(PoseTrip {
                    guard: Guard::SelfCollision,
                    frame: frames[i].name,
                    other: Some(frames[j].name),
                    margin_m: None,
                });
                continue;
            };

            let (a1, b1) = kinematics::capsule_in_world(pose_i, &first);
            let (a2, b2) = kinematics::capsule_in_world(pose_j, &second);
            let squared = kinematics::segment_distance_squared(&a1, &b1, &a2, &b2);
            let threshold = first.radius_m + second.radius_m + margin_m;
            // The decision is taken in squared distance, so no square root can move a verdict. The
            // root is taken only for the number the report carries.
            if squared < threshold * threshold {
                let separation = kinematics::distance(squared);
                pose.trips.push(PoseTrip {
                    guard: Guard::SelfCollision,
                    frame: frames[i].name,
                    other: Some(frames[j].name),
                    margin_m: Some(separation - first.radius_m - second.radius_m - margin_m),
                });
            }
        }
    }
}

fn stop_verdict<'a>(
    cycle: &CommandCycle<'a>,
    response: FaultResponse,
    tripped: FixedVec<Guard, MAX_TRIPPED>,
    clamped: FixedVec<Clamped<'a>, MAX_CHANNELS>,
    advisory: bool,
    pose: PoseOutcome<'a>,
) -> CycleVerdict<'a> {
    CycleVerdict {
        command: if advisory {
            copy_of(cycle.desired)
        } else {
            FixedVec::new()
        },
        response: Some(response),
        tripped,
        clamped,
        advisory,
        pose,
    }
}

fn copy_of<'a>(source: &'a [(&'a str, f64)]) -> FixedVec<(&'a str, f64), MAX_CHANNELS> {
    let mut out: FixedVec<(&'a str, f64), MAX_CHANNELS> = FixedVec::new();
    for entry in source {
        out.push(*entry);
    }
    out
}

/// Record a demand, keeping the strongest.
fn demand(current: &mut Option<FaultResponse>, response: FaultResponse) {
    if current.is_none_or(|held| held < response) {
        *current = Some(response);
    }
}

/// `core::f64::signum` returns 1.0 for +0.0 and -1.0 for -0.0, which would turn a zero command
/// into a full-scale one if a ceiling of zero were ever configured. This returns 0.0 for 0.0.
fn signum(value: f64) -> f64 {
    if value > 0.0 {
        1.0
    } else if value < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Whether a transport plan and a limit set are safe enough to start.
///
/// The kernel refuses two things and only two: a plan that could not be resolved, and enforcement
/// that was promised and cannot be delivered. Everything else — a missing guard, an unsourced
/// limit, a degraded observation — is a notice an operator reads and accepts, because a system
/// that refuses to start on every imperfection is a system that gets bypassed.
pub fn can_start(transport_refused: bool, limits: &ResolvedLimits<'_>) -> Result<(), &'static str> {
    if transport_refused {
        return Err("The policy cannot be transported onto this body.");
    }
    if limits.tier != Tier::Guest && limits.enforcement == Enforcement::Advisory {
        return Err("This tier promises enforcement and the kernel resolved to advisory.");
    }
    Ok(())
}

#[cfg(test)]
mod tests;

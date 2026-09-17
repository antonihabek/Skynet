//! Power and thermal (§36).
//!
//! The Rust edition of `reference/typescript/power.ts`. Read that file for the design; this one
//! records the port.
//!
//! §36 opens with the sentence the whole module is built around:
//!
//! > A laptop at 3% shows a warning. A humanoid at 3% falls over.
//!
//! So power is handled in the register of the safety envelope rather than as a user-experience
//! concern: four states, a reserve computed rather than guessed at, a fixed order in which work is
//! given up under pressure, and one override that does not consult the fuel gauge at all.
//!
//! # What this module had to decide, because §36 did not
//!
//! §36 is four short subsections of prose and this is the first code to consume any of it. Five
//! things had to be settled to make it run, and each is called out where it is defined rather than
//! left as a constant somebody has to reverse-engineer.
//!
//! **The band widths.** §36.1 says `low` is "approaching reserve" and `reserve` is "at the reserve
//! budget", which is a shape rather than a number. [`RESERVE_BAND`] and [`LOW_BAND`] give it one,
//! and both are multiples of the reserve because the reserve is the only energy scale the body
//! declares — a percentage of pack capacity would mean something different on every robot.
//!
//! **A fifth state.** §36.1's table has four and assumes a battery. An arm bolted to a bench and
//! run off mains has no state of charge, and forcing it into one of the four would either
//! immobilise it (`critical`) or claim a headroom nobody measured (`normal`). [`PowerState::
//! Unmetered`] is the honest answer, and the descriptor is what distinguishes a body with no pack
//! from a body whose gauge has stopped answering — see [`assess`].
//!
//! **What a silent gauge is.** Not `critical`. §36.4 overrides the gauge on a **positive**
//! observation from the bus, and missing evidence is never a positive finding anywhere else in
//! this system. A robot that stopped dead on a flaky I²C read would be a robot that stops dead on
//! a flaky I²C read. It is reported, it withholds new leases, and it does not stop what is running.
//!
//! **How the OS recognises a brownout.** §36.4 says voltage sag arrives as `RHAL_DEGRADED` with a
//! vendor fault code — and §24 says a fault code is "vendor-specific, surfaced verbatim to the
//! black box", which is to say the OS is told never to interpret one. Those two sentences cannot
//! both be honoured by an OS that guesses. [`PowerProfile::brownout_fault_codes`] is the resolution:
//! the vendor who defined the codes declares which of them mean sag, and the OS **matches** rather
//! than interprets. Undeclared, the OS cannot tell a sag from a dead encoder, and says so.
//!
//! **How deep the ladder is cut.** §36.3 gives an order and no schedule. [`shed`] gives one, and
//! the dividing line is meaningful rather than arbitrary: throttling gives up everything that costs
//! nothing to give up, and only thermal `critical` touches the two rungs that change what the robot
//! can do.
//!
//! # The reserve is a floor, and the report says so
//!
//! §36.2 computes the reserve from three things: the declared figures, the current payload, and the
//! distance to the nearest safe state. **One of the three exists on this platform.** There is no
//! payload mass anywhere on the descriptor and no distance-to-safe-state anywhere in the
//! observation, so a reserve computed here is the declared energy and nothing else — which is an
//! *under*-estimate, and under-estimating a reserve is the direction that runs a robot past the
//! point where it can still sit down.
//!
//! [`Reserve::unaccounted`] names the missing terms, and a reserve with a non-empty `unaccounted`
//! is reported as a floor rather than as the reserve. Inventing a payload term would have been one
//! line and would have made every downstream number a fabrication — the failure this whole document
//! is written against.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::json::number_text;

// ── The safe state's provenance (§87.2) ──────────────────────────────────────

/// The two bases a `time_to_safe_state_s` may be declared on (§87.2). There is no third.
///
/// **`live` is deliberately absent.** A figure the navigation stack supplies per cycle is the
/// attractive design and §87.2 rejects it for two reasons that have nothing to do with accuracy: it
/// puts a planner inside §36's reserve arithmetic, so a skill admitted at one instant is refused at
/// the next for a reason nobody can audit afterwards; and it makes navigation a safety-relevant
/// component with no tier, when §58.3 keeps perception and navigation as packages that §15 does not
/// test and §12's gate does not hold.
pub const SAFE_STATE_BASES: [&str; 2] = ["fixed", "mapped_worst_case"];

/// Base kinds that have somewhere to travel to. §5's base kinds minus `fixed`.
pub const MOBILE_BASES: [&str; 5] = ["wheeled", "tracked", "legged", "aerial", "rail"];

/// How a body's safe-state figure was arrived at (§87.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SafeStateDeclaration {
    /// `fixed` — it reaches a safe state where it stands — or a worst case over a mapped area.
    pub basis: String,
    /// The area the worst case was taken over. Required by `mapped_worst_case` and nothing else.
    pub area: Option<String>,
    /// When somebody measured it. Not checked, and recorded because a figure with no date is one
    /// nobody will re-take.
    pub measured_at: Option<String>,
}

/// What can be wrong with a safe-state declaration (§87.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeStateFaultCode {
    /// A mobile body whose figure has no stated basis.
    BasisUndeclared,
    /// The design §87.2 rejected, proposed by name.
    BasisRefused,
    /// A word that is neither of the two.
    BasisUnknown,
    /// A mapped worst case over nothing.
    AreaUndeclared,
    /// A mapped worst case on a body with nowhere to travel.
    MappedOnAFixedBase,
    /// A basis with no figure beside it.
    FigureUndeclared,
}

impl SafeStateFaultCode {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            SafeStateFaultCode::BasisUndeclared => "basis_undeclared",
            SafeStateFaultCode::BasisRefused => "basis_refused",
            SafeStateFaultCode::BasisUnknown => "basis_unknown",
            SafeStateFaultCode::AreaUndeclared => "area_undeclared",
            SafeStateFaultCode::MappedOnAFixedBase => "mapped_on_a_fixed_base",
            SafeStateFaultCode::FigureUndeclared => "figure_undeclared",
        }
    }
}

/// Every safe-state fault code, in the reference's declaration order.
pub const SAFE_STATE_FAULTS: [SafeStateFaultCode; 6] = [
    SafeStateFaultCode::BasisUndeclared,
    SafeStateFaultCode::BasisRefused,
    SafeStateFaultCode::BasisUnknown,
    SafeStateFaultCode::AreaUndeclared,
    SafeStateFaultCode::MappedOnAFixedBase,
    SafeStateFaultCode::FigureUndeclared,
];

/// One thing wrong with a safe-state declaration.
#[derive(Debug, Clone, PartialEq)]
pub struct SafeStateFault {
    /// What is wrong.
    pub code: SafeStateFaultCode,
    /// What to tell the vendor who wrote it.
    pub detail: String,
}

/// Whether this base kind has a navigation problem between it and a safe state.
pub fn base_is_mobile(base: Option<&str>) -> bool {
    base.is_some_and(|base| MOBILE_BASES.contains(&base))
}

/// §87.2's checks over the declaration, or `None`.
///
/// The first fault in declaration order, like `validate_motions` and for the same reason: the basis
/// is read before the area, and the area before anything about the figure, because a declaration
/// whose basis is wrong is one whose other fields are being read against the wrong rule.
///
/// The first rule is the one that keeps every descriptor written before §87 valid: **an absent
/// basis on a `fixed` base is `fixed`.** A robot bolted to a bench should not have to declare what
/// it is not, which is §35.3's argument about `simulator` applied to the same shape. On a mobile
/// base the same silence is a rumour in §5.2's sense, and it is the term the reserve is built from.
pub fn validate_safe_state(profile: &PowerProfile, base: Option<&str>) -> Option<SafeStateFault> {
    let has_figure = profile
        .time_to_safe_state_s
        .is_some_and(|value| value.is_finite() && value > 0.0);

    let Some(declaration) = profile.safe_state.as_ref() else {
        if !base_is_mobile(base) {
            return None;
        }
        return Some(SafeStateFault {
            code: SafeStateFaultCode::BasisUndeclared,
            detail: "A figure with no basis is a rumour (§5.2), and on a body that travels the \
                     time to a safe state depends on where it is standing. Declare `fixed` if it \
                     stops where it is, or `mapped_worst_case` with the area the worst case was \
                     taken over."
                .to_string(),
        });
    };

    if declaration.basis == "live" {
        return Some(SafeStateFault {
            code: SafeStateFaultCode::BasisRefused,
            detail: "A figure the navigation stack supplies per cycle is the design §87.2 \
                     rejected: it puts a planner inside §36's reserve arithmetic, so a skill \
                     admitted at one instant is refused at the next for a reason nobody can audit, \
                     and it makes navigation a safety-relevant component with no tier. Declare a \
                     worst case over a mapped area instead."
                .to_string(),
        });
    }

    if !SAFE_STATE_BASES.contains(&declaration.basis.as_str()) {
        return Some(SafeStateFault {
            code: SafeStateFaultCode::BasisUnknown,
            detail: format!(
                "`{}` is not one of fixed or mapped_worst_case, so nothing can read how this \
                 figure was arrived at.",
                declaration.basis
            ),
        });
    }

    if declaration.basis == "mapped_worst_case" {
        if !base_is_mobile(base) {
            return Some(SafeStateFault {
                code: SafeStateFaultCode::MappedOnAFixedBase,
                detail: "This base does not travel, so there is no mapped area to take a worst \
                         case over. It is a claim about a navigation problem this body does not \
                         have, and it arrives by copying a descriptor between machines."
                    .to_string(),
            });
        }
        let named = declaration
            .area
            .as_deref()
            .is_some_and(|area| !crate::js::js_trim(area).is_empty());
        if !named {
            return Some(SafeStateFault {
                code: SafeStateFaultCode::AreaUndeclared,
                detail:
                    "A worst case over nothing is not a worst case. Name the area it was taken \
                         over, so somebody reading the reserve later knows where the figure stops \
                         being true."
                        .to_string(),
            });
        }
    }

    if !has_figure {
        return Some(SafeStateFault {
            code: SafeStateFaultCode::FigureUndeclared,
            detail: "A basis with no `time_to_safe_state_s` beside it describes how a number was \
                     arrived at without carrying the number."
                .to_string(),
        });
    }

    None
}

/// The basis the third reserve term may be accounted on, or `None`.
///
/// `fixed` on a fixed base is the reading a bolted-down arm's silence always had, spelled out;
/// every other combination has to be declared and has to pass [`validate_safe_state`], because
/// accounting a term against a declaration with a fault in it would be the invented measurement
/// §5.2 forbids.
pub fn safe_state_basis(profile: &PowerProfile, base: Option<&str>) -> Option<&'static str> {
    if validate_safe_state(profile, base).is_some() {
        return None;
    }
    if !profile
        .time_to_safe_state_s
        .is_some_and(|value| value.is_finite() && value > 0.0)
    {
        return None;
    }
    match profile.safe_state.as_ref().map(|d| d.basis.as_str()) {
        Some("mapped_worst_case") => Some("mapped_worst_case"),
        Some("fixed") => Some("fixed"),
        _ => {
            if base_is_mobile(base) {
                None
            } else {
                Some("fixed")
            }
        }
    }
}

/// Whether this body may attempt the host tier, given what it says about its safe state (§87.2).
///
/// The AMR-shaped sibling of §46.2's `host_blocked_by_command_loss`, and it has the same shape for
/// the same reason: the tier is refused where the recovery path has not been established, and the
/// refusal names the fix. `None` when it may.
///
/// **A `fixed` base is never refused on this ground**, because it never was the problem — §36.2's
/// assumption that the robot knows where a safe state is holds trivially for an arm bolted down,
/// and §43 says so in the row this answers.
pub fn host_blocked_by_safe_state(profile: &PowerProfile, base: Option<&str>) -> Option<String> {
    if !base_is_mobile(base) {
        return None;
    }
    let fault = validate_safe_state(profile, base)?;
    // Two sentences rather than one, and neither is assembled out of the other. §64 makes a message
    // that misdescribes what happened a defect, and a refusal built by splicing a clause in front
    // of a detail that already opens with its own subject is exactly how one gets written.
    Some(format!(
        "§36.2 computes this body's reserve budget from the time it takes to reach a safe state, \
         and this body travels, so that figure is one nothing can place ({}). {} (§87.2)",
        fault.code.as_str(),
        fault.detail
    ))
}

// ── What the body declares (§36.2, §52) ──────────────────────────────────────

/// The power and thermal figures a vendor declares (§36.2, §52).
///
/// Every field is nullable and every absence produces a named unavailability rather than an
/// assumption, which is §7.2's rule applied here as it is everywhere else. `brownout_fault_codes`
/// is the one list, and empty means *this vendor has not said which of its codes mean sag* — not
/// *this body cannot brown out*.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PowerProfile {
    /// How long this body takes to reach its nearest safe state from full rated load.
    ///
    /// Declared, and measured by `power.safe_state.reachable` — which has existed in the
    /// conformance suite since Part II and had, until this module, no field to measure against.
    pub time_to_safe_state_s: Option<f64>,
    /// The energy that takes, watt-hours. The reserve is built from this figure.
    pub safe_stop_energy_wh: Option<f64>,
    /// The pack's usable capacity, watt-hours.
    ///
    /// Present so a gauge that reports a fraction can be compared against a reserve in watt-hours.
    /// A body that declares no capacity and whose gauge reports only a percentage has a state of
    /// charge and no way to turn it into energy, which [`assess`] reports rather than approximates.
    pub pack_capacity_wh: Option<f64>,
    /// The temperature at which this body begins giving up work (§36.3).
    pub thermal_throttle_c: Option<f64>,
    /// The temperature at which it gives up everything it is allowed to give up.
    pub thermal_critical_c: Option<f64>,
    /// How `time_to_safe_state_s` was arrived at (§87.2), or `None`.
    ///
    /// A block **about** the figure rather than a second copy of it, because §78 is this
    /// repository's record of what two spellings of one fact cost. §36.2's third reserve term is
    /// *distance to the nearest safe state*, and on a mobile robot the figure above is a number
    /// that depends on where the robot is standing — so §87.2's decision is that it stays a
    /// **declaration** with a stated basis, and the live figure a navigation stack could supply is
    /// refused.
    pub safe_state: Option<SafeStateDeclaration>,
    /// The vendor's own fault codes that mean voltage sag (§36.4).
    ///
    /// §24 makes `fault_code` vendor-specific and surfaced verbatim, so the OS is explicitly not in
    /// a position to interpret one — and §36.4 asks it to act on exactly one meaning. The way out
    /// is for the vendor who defined the codes to name the ones that mean sag: the OS then matches
    /// a number against a list rather than interpreting anything.
    ///
    /// **Empty is not "cannot brown out".** It is "nobody has said", and it is reported as such at
    /// the moment it matters — when the RHAL is actually degraded and nothing can say why.
    pub brownout_fault_codes: Vec<u32>,
}

impl PowerProfile {
    /// Whether this body claims a battery at all.
    ///
    /// A pack is claimed by declaring either its capacity or the energy a safe stop costs — the
    /// second because an energy budget in watt-hours is a statement about a pack even when nobody
    /// wrote the capacity down. A body that declares neither is a mains machine as far as this
    /// module is concerned, and [`PowerState::Unmetered`] is the correct reading of its silence.
    pub fn claims_a_pack(&self) -> bool {
        self.pack_capacity_wh.is_some() || self.safe_stop_energy_wh.is_some()
    }
}

// ── What the robot observed ──────────────────────────────────────────────────

/// One cycle's power and thermal observation.
///
/// Every field is what something on the robot actually read. Nothing here is derived, and nothing
/// here is remembered — the same discipline §33.3 imposes on the reconciler's `actual`, for the
/// same reason.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PowerObservation {
    /// State of charge as the fuel gauge reports it, 0–1.
    pub state_of_charge: Option<f64>,
    /// Energy remaining, watt-hours, where the gauge reports energy rather than a fraction.
    ///
    /// Preferred over [`Self::state_of_charge`] when both are present, because it needs no capacity
    /// to be useful and a gauge that reports it is a gauge that has been told the pack's capacity
    /// by somebody closer to the pack than this module is.
    pub energy_remaining_wh: Option<f64>,
    /// The hottest thermal zone, degrees Celsius.
    pub temperature_c: Option<f64>,
    /// How long this body has been above [`PowerProfile::thermal_throttle_c`] without dropping
    /// back under it. Zero when it is not throttling.
    pub throttled_for_ms: u64,
    /// Whether the RHAL raised `RHAL_DEGRADED` this cycle (§24).
    pub rhal_degraded: bool,
    /// The vendor fault code the RHAL surfaced, verbatim (§24).
    pub fault_code: Option<u32>,
}

// ── The reserve (§36.2) ──────────────────────────────────────────────────────

/// §36.2's terms, in the order that section lists them.
///
/// Two of the three have no source anywhere on this platform, which is the fact
/// [`Reserve::unaccounted`] exists to carry rather than to hide.
pub const RESERVE_TERMS: [&str; 3] = ["declared_energy", "payload", "distance_to_safe_state"];

/// The energy this body must keep in hand to reach a safe state (§36.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reserve {
    /// The reserve, watt-hours, or `None` when the body declares nothing to compute one from.
    pub wh: Option<f64>,
    /// §36.2's terms nothing on this robot supplies, in [`RESERVE_TERMS`] order.
    ///
    /// **Non-empty means [`Self::wh`] is a floor**, not the reserve: every missing term can only
    /// add energy, so the figure is an under-estimate and every state derived from it is
    /// optimistic by an unknown amount.
    pub unaccounted: Vec<&'static str>,
    /// The basis `distance_to_safe_state` was accounted on, or `None` when it was not (§87.2).
    ///
    /// The difference between a term **nobody supplied** and a term **bounded** over an area
    /// somebody walked, and a reader who cannot tell the two apart is reading a floor as a
    /// measurement. It is the same shape §7.2 uses for a guard whose input nobody declared: the
    /// value and the reason it is what it is, together, so neither can be read without the other.
    pub safe_state_basis: Option<&'static str>,
}

impl Reserve {
    /// Whether every term §36.2 names was accounted for.
    ///
    /// Today this is `false` on every robot, and it is a method rather than a constant so that the
    /// day a payload mass reaches the descriptor, the consumers do not have to be found again.
    pub fn complete(&self) -> bool {
        self.wh.is_some() && self.unaccounted.is_empty()
    }
}

/// Compute §36.2's reserve budget from what the body declares.
///
/// The arithmetic is deliberately the identity: the reserve *is* `safe_stop_energy_wh`, because the
/// other two terms §36.2 names have no source. Scaling the declared figure by a plausible payload
/// factor would have been one line of arithmetic and a number nobody could defend — and this is the
/// number that decides when a two-metre humanoid stops walking.
pub fn reserve(profile: &PowerProfile, base: Option<Option<&str>>) -> Reserve {
    let mut unaccounted = Vec::new();
    if profile.safe_stop_energy_wh.is_none() {
        unaccounted.push(RESERVE_TERMS[0]);
    }
    // `payload` still has no source on this platform. Listed unconditionally rather than behind a
    // lookup, so the day one arrives this is a compile-time-visible place to change rather than a
    // silent over-report.
    unaccounted.push(RESERVE_TERMS[1]);

    // §87.2. The third term leaves `unaccounted` when the declaration is sound — which is not the
    // same as the figure being measured, and `safe_state_basis` is what keeps the two
    // distinguishable. The outer `Option` is *did the caller say what this body stands on*: `None`
    // gets the reading this function had before §87, because "nobody told me" and "it is bolted
    // down" are the two things §7.2 exists to keep apart.
    let basis = base.and_then(|base| safe_state_basis(profile, base));
    if basis.is_none() {
        unaccounted.push(RESERVE_TERMS[2]);
    }

    Reserve {
        wh: profile
            .safe_stop_energy_wh
            .filter(|value| value.is_finite() && *value > 0.0),
        unaccounted,
        safe_state_basis: basis,
    }
}

// ── The four states, and the fifth (§36.1) ───────────────────────────────────

/// Where the `reserve` band ends, as a multiple of the reserve.
///
/// A quarter of a reserve wide. §36.1 puts `reserve` "at the reserve budget" and `critical` "below
/// reserve", which as written are a single point and everything under it — and a gauge reporting
/// within a percent or two of the boundary would then chatter between *begin a controlled stop* and
/// *stop immediately*, on a robot that is mid-ramp. The band is what makes the two states
/// distinguishable for long enough to act on.
pub const RESERVE_BAND: f64 = 1.25;

/// Where the `low` band ends, as a multiple of the reserve.
///
/// **One whole reserve above the reserve band**, and that width is the argument: `low`'s only
/// behaviour is that a running task may finish and no new one may start, so the warning has to
/// arrive with at least one further safe stop's worth of energy in hand or the promise in §36.1's
/// row is one the robot cannot keep. Anything narrower makes `low` a state the robot passes
/// through rather than one it acts in.
pub const LOW_BAND: f64 = 2.25;

/// §36.1's states, most capable first, plus the one §36.1 does not have.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PowerState {
    /// Above reserve plus margin. Full operation.
    Normal,
    /// Approaching reserve. No new task leases; a running task may finish; the operator is told.
    Low,
    /// At the reserve budget. A controlled stop, then a safe state.
    Reserve,
    /// Below reserve. An immediate controlled stop; drives disabled once at rest.
    Critical,
    /// This body's energy is not metered.
    ///
    /// Not in §36.1's table, which assumes a battery. Two bodies reach it and they are not the same
    /// robot: one declares no pack and is a mains machine behaving correctly, one declares a pack
    /// whose gauge has stopped answering. [`Assessment::faults`] is what tells them apart, and it is
    /// deliberately not a sixth state — the *power* state of both is that nobody knows it.
    #[default]
    Unmetered,
}

impl PowerState {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            PowerState::Normal => "normal",
            PowerState::Low => "low",
            PowerState::Reserve => "reserve",
            PowerState::Critical => "critical",
            PowerState::Unmetered => "unmetered",
        }
    }

    /// Read one back, for a report that crossed a wire. Exact, like every other parser here.
    pub fn parse(text: &str) -> Option<Self> {
        POWER_STATES
            .iter()
            .copied()
            .find(|state| state.as_str() == text)
    }

    /// Whether §25.2 may grant a new task lease in this state (§36.1).
    ///
    /// `Unmetered` grants, and that is the whole reason the state exists: a bench arm on mains must
    /// not be refused work because it has no battery to report on. A *gauge fault* withholds leases
    /// and [`Assessment::grants_new_leases`] is where the two are combined — the state alone is not
    /// enough to answer.
    pub fn grants_new_leases(self) -> bool {
        matches!(self, PowerState::Normal | PowerState::Unmetered)
    }

    /// Whether §36.1 requires the robot to be taken to a safe state.
    pub fn requires_safe_state(self) -> bool {
        matches!(self, PowerState::Reserve | PowerState::Critical)
    }

    /// Whether the stop is immediate rather than after the running task (§36.1's last row).
    pub fn stops_immediately(self) -> bool {
        self == PowerState::Critical
    }

    /// Whether an operator is notified (§36.1's `low` row, and everything below it).
    pub fn notifies_operator(self) -> bool {
        matches!(
            self,
            PowerState::Low | PowerState::Reserve | PowerState::Critical
        )
    }
}

/// Every power state, most capable first.
pub const POWER_STATES: [PowerState; 5] = [
    PowerState::Normal,
    PowerState::Low,
    PowerState::Reserve,
    PowerState::Critical,
    PowerState::Unmetered,
];

/// Why the power state is what it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerReason {
    /// §36.4's override: the bus says sag, and the gauge was not consulted.
    Brownout,
    /// Below the reserve.
    BelowReserve,
    /// Inside the reserve band.
    AtReserve,
    /// Inside the low band.
    ApproachingReserve,
    /// Above the low band.
    AboveReserve,
    /// This body declares no pack.
    NoPack,
    /// This body declares a pack and nothing answered for it.
    NoReading,
    /// A reading arrived and there is no reserve to compare it against.
    NoReserve,
    /// A fraction arrived and no capacity to turn it into energy.
    NoCapacity,
    /// A reading arrived and could not be believed.
    ///
    /// Separate from [`PowerReason::NoCapacity`] because the remedies are in different places: a
    /// missing capacity is a descriptor to fill in, and a state of charge of 1.4 is a gauge to fix.
    /// Both leave the robot with no energy figure and an operator with two different phone calls.
    ReadingUnusable,
}

impl PowerReason {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            PowerReason::Brownout => "brownout",
            PowerReason::BelowReserve => "below_reserve",
            PowerReason::AtReserve => "at_reserve",
            PowerReason::ApproachingReserve => "approaching_reserve",
            PowerReason::AboveReserve => "above_reserve",
            PowerReason::NoPack => "no_pack",
            PowerReason::NoReading => "no_reading",
            PowerReason::NoReserve => "no_reserve",
            PowerReason::NoCapacity => "no_capacity",
            PowerReason::ReadingUnusable => "reading_unusable",
        }
    }
}

/// Every reason, in rule order.
pub const POWER_REASONS: [PowerReason; 10] = [
    PowerReason::Brownout,
    PowerReason::BelowReserve,
    PowerReason::AtReserve,
    PowerReason::ApproachingReserve,
    PowerReason::AboveReserve,
    PowerReason::NoPack,
    PowerReason::NoReading,
    PowerReason::NoReserve,
    PowerReason::NoCapacity,
    PowerReason::ReadingUnusable,
];

// ── What is wrong, as distinct from where the state is ───────────────────────

/// A fault in the power subsystem itself, rather than a level of charge.
///
/// Separate from [`PowerState`] because they answer different questions and an operator needs both:
/// the state says how much energy is left, and a fault says whether that number can be believed.
/// Folding them together would produce a robot reporting `unmetered` for a dead gauge and for a
/// mains socket, which are opposite situations with opposite remedies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerFaultCode {
    /// The body declares a pack and nothing read one.
    GaugeSilent,
    /// The gauge reports a fraction and the body declares no capacity to scale it by.
    CapacityUndeclared,
    /// A pack is claimed and no safe-stop energy is declared, so there is no reserve.
    ReserveUndeclared,
    /// The RHAL is degraded and this vendor has declared no code that means sag (§36.4).
    BrownoutUndeclarable,
    /// The gauge reports a state of charge outside 0–1.
    ChargeOutOfRange,
    /// Above [`PowerProfile::thermal_throttle_c`] for longer than [`SUSTAINED_THROTTLE_MS`].
    ThermalSustained,
    /// Above [`PowerProfile::thermal_critical_c`].
    ThermalCritical,
    /// A temperature arrived and the body declares no threshold to compare it against.
    ThermalUndeclared,
}

impl PowerFaultCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            PowerFaultCode::GaugeSilent => "gauge_silent",
            PowerFaultCode::CapacityUndeclared => "capacity_undeclared",
            PowerFaultCode::ReserveUndeclared => "reserve_undeclared",
            PowerFaultCode::BrownoutUndeclarable => "brownout_undeclarable",
            PowerFaultCode::ChargeOutOfRange => "charge_out_of_range",
            PowerFaultCode::ThermalSustained => "thermal_sustained",
            PowerFaultCode::ThermalCritical => "thermal_critical",
            PowerFaultCode::ThermalUndeclared => "thermal_undeclared",
        }
    }

    /// Whether this fault raises §27's `DEGRADED`.
    ///
    /// The two thermal ones do, because §36.3 says so in as many words. The declaration gaps do
    /// not: a descriptor that is missing a field is a descriptor to fix, and a robot that reported
    /// itself degraded for it would be a robot reporting a paperwork problem in the same column as
    /// a hot motor.
    pub fn degrades(self) -> bool {
        matches!(
            self,
            PowerFaultCode::ThermalSustained | PowerFaultCode::ThermalCritical
        )
    }

    /// Whether this fault withholds new task leases.
    ///
    /// A robot that cannot tell whether it is at reserve should not be handed *new* work, and
    /// should not have taken from it the work it is already doing — which is exactly §36.1's `low`
    /// behaviour, arrived at from the other direction.
    pub fn withholds_leases(self) -> bool {
        matches!(
            self,
            PowerFaultCode::GaugeSilent
                | PowerFaultCode::CapacityUndeclared
                | PowerFaultCode::ReserveUndeclared
                | PowerFaultCode::ChargeOutOfRange
                | PowerFaultCode::ThermalCritical
        )
    }
}

/// Every fault code, in the order [`assess`] can raise them.
pub const POWER_FAULT_CODES: [PowerFaultCode; 8] = [
    PowerFaultCode::GaugeSilent,
    PowerFaultCode::CapacityUndeclared,
    PowerFaultCode::ReserveUndeclared,
    PowerFaultCode::BrownoutUndeclarable,
    PowerFaultCode::ChargeOutOfRange,
    PowerFaultCode::ThermalSustained,
    PowerFaultCode::ThermalCritical,
    PowerFaultCode::ThermalUndeclared,
];

/// One thing wrong, with the sentence that names the remedy (§64).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerFault {
    /// A code a console can translate.
    pub code: PowerFaultCode,
    /// What to do about it.
    pub detail: String,
}

// ── Thermal (§36.3) ──────────────────────────────────────────────────────────

/// How long above the throttle threshold counts as "sustained" (§36.3).
///
/// Thirty seconds. §36.3 raises `DEGRADED` on *sustained* throttling and does not say how long,
/// and both mistakes are real: a threshold of one cycle makes every heavy motion a degraded robot,
/// and a threshold of ten minutes makes `DEGRADED` a state a thermally limited robot never reaches
/// while spending its afternoon at the throttle point.
pub const SUSTAINED_THROTTLE_MS: u64 = 30_000;

/// §36.3's ladder, weakest claim first.
///
/// Data rather than a comment, because the order is the specification's contribution here and a
/// consumer that reordered it would be giving up the policy's loop rate before it gave up an OTA
/// download.
pub const SHED_ORDER: [&str; 5] = [
    "telemetry",
    "ota",
    "blackbox_compression",
    "inference_rate",
    "non_essential_sensors",
];

/// What is never given up, at any pressure (§36.3).
///
/// > and never: RHAL, kernel, control loop
///
/// Present as data so it can be asserted against, which is the only way a list like this stays
/// true: [`shed`] never returns any of these, and a test says so.
pub const NEVER_SHED: [&str; 3] = ["rhal", "kernel", "control_loop"];

/// How many rungs of [`SHED_ORDER`] are given up while throttling but not critical.
///
/// The first three, and the line between three and four is the meaningful one: telemetry, OTA and
/// black-box compression are housekeeping, and giving them up costs the robot nothing it was
/// hired to do. Rungs four and five change what it can *do* — a slower policy and fewer sensors —
/// so they are held back for the temperature at which the alternative is damage.
pub const THROTTLE_DEPTH: usize = 3;

/// Thermal pressure, from a temperature and what the body declares about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThermalPressure {
    /// Below the throttle threshold, or no temperature to compare.
    #[default]
    Nominal,
    /// At or above the throttle threshold, below critical.
    Throttling,
    /// At or above the critical threshold.
    Critical,
}

impl ThermalPressure {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            ThermalPressure::Nominal => "nominal",
            ThermalPressure::Throttling => "throttling",
            ThermalPressure::Critical => "critical",
        }
    }
}

/// Every pressure level.
pub const THERMAL_PRESSURES: [ThermalPressure; 3] = [
    ThermalPressure::Nominal,
    ThermalPressure::Throttling,
    ThermalPressure::Critical,
];

/// The pressure a temperature implies, given what the body declares.
///
/// A temperature with no declared threshold is [`ThermalPressure::Nominal`] and a fault, not a
/// silent pass: nobody has said what hot is on this body, so nothing here is entitled to decide.
/// Guessing 85 °C would be right for a CPU package and wrong for a harmonic drive by fifty degrees.
pub fn pressure(profile: &PowerProfile, temperature_c: Option<f64>) -> ThermalPressure {
    let Some(temperature) = temperature_c.filter(|value| value.is_finite()) else {
        return ThermalPressure::Nominal;
    };
    if let Some(critical) = profile.thermal_critical_c {
        if temperature >= critical {
            return ThermalPressure::Critical;
        }
    }
    if let Some(throttle) = profile.thermal_throttle_c {
        if temperature >= throttle {
            return ThermalPressure::Throttling;
        }
    }
    ThermalPressure::Nominal
}

/// What is given up at this pressure, in [`SHED_ORDER`] order (§36.3).
///
/// Never returns anything in [`NEVER_SHED`], which is the invariant the whole ladder exists to
/// state: there is no thermal situation in which giving up the safety envelope is the right trade,
/// and inference is throttled before control rather than beside it.
pub fn shed(pressure: ThermalPressure) -> Vec<&'static str> {
    let depth = match pressure {
        ThermalPressure::Nominal => 0,
        ThermalPressure::Throttling => THROTTLE_DEPTH,
        ThermalPressure::Critical => SHED_ORDER.len(),
    };
    SHED_ORDER[..depth].to_vec()
}

/// Whether a package's declared inference budget can still be met (§36.3's last sentence).
///
/// Returns the sentence when it cannot, and `None` when it can or when the question cannot be
/// asked. Three ways it cannot be asked, and all three answer `None` rather than blocking:
///
/// * the package declares no budget — nothing to miss;
/// * nothing has measured the loop — a package is not blocked on an absence;
/// * the measurement is not finite.
///
/// The comparison is against the **observed** loop time rather than against the thermal state,
/// which is the honest coupling: thermal throttling is the usual cause of a missed budget and is
/// not the only one, and a robot that blocked a package for being warm while its loop was
/// comfortably inside budget would be enforcing a proxy instead of the thing itself.
pub fn budget_missed(declared_ms: Option<f64>, observed_ms: Option<f64>) -> Option<String> {
    let declared = declared_ms.filter(|value| value.is_finite() && *value > 0.0)?;
    let observed = observed_ms.filter(|value| value.is_finite())?;
    if observed <= declared {
        return None;
    }
    Some(format!(
        "this package declares an inference budget of {} ms and the loop is taking {} ms. §36.3 \
         throttles inference before control, so the loop is late rather than the robot being \
         unsafe — but a policy quietly missing its own declared deadline is what the budget exists \
         to prevent, so it is refused until the measurement comes back inside it.",
        number_text(declared),
        number_text(observed)
    ))
}

// ── The assessment ───────────────────────────────────────────────────────────

/// One evaluation of §36.
#[derive(Debug, Clone, PartialEq)]
pub struct Assessment {
    /// Where the body is on §36.1's ladder.
    pub state: PowerState,
    /// Why.
    pub reason: PowerReason,
    /// The reserve, and which of §36.2's terms went into it.
    pub reserve: Reserve,
    /// The energy the gauge accounts for, watt-hours, where it could be established.
    pub remaining_wh: Option<f64>,
    /// Thermal pressure (§36.3).
    pub thermal: ThermalPressure,
    /// What is being given up at that pressure, in [`SHED_ORDER`] order.
    pub shed: Vec<&'static str>,
    /// Everything wrong with the subsystem, in [`POWER_FAULT_CODES`] order.
    pub faults: Vec<PowerFault>,
}

impl Assessment {
    /// Whether §25.2 may grant a new task lease.
    ///
    /// The state's answer, narrowed by any fault that withholds. Two sources rather than one
    /// because they are two different reasons to say no — *there is not enough energy* and *the
    /// energy cannot be measured* — and an operator reading a refusal needs to know which.
    pub fn grants_new_leases(&self) -> bool {
        self.state.grants_new_leases()
            && !self
                .faults
                .iter()
                .any(|fault| fault.code.withholds_leases())
    }

    /// Whether §27's `DEGRADED` is raised by power or thermal (§36.3).
    pub fn degraded(&self) -> bool {
        self.faults.iter().any(|fault| fault.code.degrades())
    }

    /// Whether §36.1 requires this robot to be taken to a safe state now.
    pub fn requires_safe_state(&self) -> bool {
        self.state.requires_safe_state()
    }

    /// Whether the stop is immediate (§36.1's last row, and §36.4's override).
    pub fn stops_immediately(&self) -> bool {
        self.state.stops_immediately()
    }
}

/// Assess §36 for one cycle.
///
/// The order of the rules is the design, and the first one is the one §36.4 is written for.
///
/// 1. **Brownout.** The bus said sag; the gauge is not consulted, "because a gauge that disagrees
///    with the bus is a gauge that is wrong". This is the only rule that ignores a reading it has.
/// 2. **No pack declared.** A mains machine, correctly reporting that its energy is not metered.
/// 3. **No reading.** A body that claims a pack and got nothing back. Not `critical` — see the
///    module header — and the fault is what makes it distinguishable from rule 2.
/// 4. **No reserve, or no capacity.** A reading arrived and cannot be compared against anything.
/// 5. **The bands.** Which is the only rule that looks at a number, and the last one for that
///    reason.
pub fn assess(
    profile: &PowerProfile,
    observation: &PowerObservation,
    base: Option<Option<&str>>,
) -> Assessment {
    let reserve = reserve(profile, base);
    let thermal = pressure(profile, observation.temperature_c);
    let mut faults: Vec<PowerFault> = Vec::new();

    let remaining = remaining_wh(profile, observation);

    // Rule 1, and it runs before the faults are gathered because §36.4's override does not depend
    // on any of them. A brownout with a silent gauge is still a brownout.
    let brownout = observation.rhal_degraded
        && observation
            .fault_code
            .is_some_and(|code| profile.brownout_fault_codes.contains(&code));

    let (state, reason) = if brownout {
        (PowerState::Critical, PowerReason::Brownout)
    } else if !profile.claims_a_pack() {
        (PowerState::Unmetered, PowerReason::NoPack)
    } else if observation.state_of_charge.is_none() && observation.energy_remaining_wh.is_none() {
        (PowerState::Unmetered, PowerReason::NoReading)
    } else {
        match (remaining, reserve.wh) {
            (Some(remaining), Some(budget)) => {
                (band(remaining, budget), band_reason(remaining, budget))
            }
            // Two ways to arrive with no energy figure, and they send an operator to two different
            // places: a gauge whose reading cannot be believed, and a descriptor with no capacity
            // to scale a believable one by.
            (None, _) if charge_unusable(observation) => {
                (PowerState::Unmetered, PowerReason::ReadingUnusable)
            }
            (None, _) => (PowerState::Unmetered, PowerReason::NoCapacity),
            (Some(_), None) => (PowerState::Unmetered, PowerReason::NoReserve),
        }
    };

    // The faults, in POWER_FAULT_CODES order so a console renders them the same way every time.
    if profile.claims_a_pack()
        && observation.state_of_charge.is_none()
        && observation.energy_remaining_wh.is_none()
    {
        faults.push(PowerFault {
            code: PowerFaultCode::GaugeSilent,
            detail: "this body declares a pack and nothing on this robot read one. The state is \
                     reported as unmetered rather than critical — §36.4 overrides the gauge on a \
                     positive reading from the bus, and a missing reading is not one — and no new \
                     task lease is granted until something answers for the pack (§36.1)."
                .to_string(),
        });
    }
    if profile.claims_a_pack()
        && observation.energy_remaining_wh.is_none()
        && observation.state_of_charge.is_some()
        && profile.pack_capacity_wh.is_none()
    {
        faults.push(PowerFault {
            code: PowerFaultCode::CapacityUndeclared,
            detail: "the gauge reports a fraction and this body declares no `pack_capacity_wh` to \
                     turn it into watt-hours, so there is nothing to compare against the reserve. \
                     Declare the capacity, or report `energy_remaining_wh` from the gauge (§36.2)."
                .to_string(),
        });
    }
    if profile.claims_a_pack() && reserve.wh.is_none() {
        faults.push(PowerFault {
            code: PowerFaultCode::ReserveUndeclared,
            detail: "this body declares a pack and no `safe_stop_energy_wh`, so §36.2's reserve \
                     cannot be computed and the four states have no boundary. It is the figure \
                     `power.safe_state.reachable` measures, and the one that decides when this \
                     robot stops."
                .to_string(),
        });
    }
    if observation.rhal_degraded && profile.brownout_fault_codes.is_empty() {
        faults.push(PowerFault {
            code: PowerFaultCode::BrownoutUndeclarable,
            detail: "the RHAL is reporting `RHAL_DEGRADED` and this body declares no \
                     `brownout_fault_codes`, so nothing here can tell a voltage sag from a dead \
                     encoder. §24 makes a fault code vendor-specific and surfaced verbatim, which \
                     is precisely why the vendor rather than the OS has to name the codes that \
                     mean sag (§36.4)."
                .to_string(),
        });
    }
    if let Some(charge) = observation.state_of_charge {
        // Through the same predicate the reason uses, rather than a second spelling of it beside
        // the first: a fault that fired on a reading the state accepted would be the two halves of
        // one answer disagreeing about what a state of charge is.
        if charge_unusable(observation) {
            faults.push(PowerFault {
                code: PowerFaultCode::ChargeOutOfRange,
                detail: format!(
                    "the gauge reports a state of charge of {}, and a state of charge is a \
                     fraction between 0 and 1. The reading is discarded rather than clamped: a \
                     clamped 1.4 is a full battery, and this one is a gauge nobody should be \
                     trusting yet (§36.1).",
                    number_text(charge)
                ),
            });
        }
    }
    match thermal {
        ThermalPressure::Throttling if observation.throttled_for_ms >= SUSTAINED_THROTTLE_MS => {
            faults.push(PowerFault {
                code: PowerFaultCode::ThermalSustained,
                detail: format!(
                    "this body has been throttling for {} s. §36.3 raises DEGRADED on sustained \
                     throttling, and {} of the five rungs are given up — never the RHAL, the \
                     kernel or the control loop.",
                    number_text((observation.throttled_for_ms / 1000) as f64),
                    THROTTLE_DEPTH
                ),
            });
        }
        ThermalPressure::Critical => {
            faults.push(PowerFault {
                code: PowerFaultCode::ThermalCritical,
                detail: "this body is at or above its declared critical temperature. Every rung \
                         of §36.3's ladder is given up, inference included; the RHAL, the kernel \
                         and the control loop are not, because there is no thermal situation in \
                         which giving up the safety envelope is the right trade."
                    .to_string(),
            });
        }
        _ => {}
    }
    if observation
        .temperature_c
        .is_some_and(|value| value.is_finite())
        && profile.thermal_throttle_c.is_none()
        && profile.thermal_critical_c.is_none()
    {
        faults.push(PowerFault {
            code: PowerFaultCode::ThermalUndeclared,
            detail: "something is reading a temperature and this body declares neither \
                     `thermal_throttle_c` nor `thermal_critical_c`, so nothing here knows what hot \
                     is on this machine. The reading is reported and nothing is shed: 85 °C is \
                     right for a CPU package and fifty degrees wrong for a harmonic drive (§36.3)."
                .to_string(),
        });
    }

    Assessment {
        state,
        reason,
        reserve,
        remaining_wh: remaining,
        thermal,
        shed: shed(thermal),
        faults,
    }
}

/// The energy the gauge accounts for, watt-hours.
///
/// `energy_remaining_wh` wins over a fraction times a capacity, because a gauge reporting energy
/// has been told the pack's capacity by something closer to the pack than a descriptor field. A
/// fraction outside 0–1 produces `None` rather than a clamp — see [`PowerFaultCode::
/// ChargeOutOfRange`].
fn remaining_wh(profile: &PowerProfile, observation: &PowerObservation) -> Option<f64> {
    if let Some(energy) = observation
        .energy_remaining_wh
        .filter(|value| value.is_finite() && *value >= 0.0)
    {
        return Some(energy);
    }
    let charge = observation
        .state_of_charge
        .filter(|value| value.is_finite() && *value >= 0.0 && *value <= 1.0)?;
    let capacity = profile
        .pack_capacity_wh
        .filter(|value| value.is_finite() && *value > 0.0)?;
    Some(charge * capacity)
}

/// Whether the gauge produced a state of charge that is not a state of charge.
///
/// Only reached when there is no energy figure, and only about the fraction: an
/// `energy_remaining_wh` that is negative or not finite is already ignored by [`remaining_wh`], and
/// a body reporting one alongside no usable fraction has a gauge fault of a kind this platform has
/// not seen and would rather not name speculatively.
fn charge_unusable(observation: &PowerObservation) -> bool {
    observation
        .state_of_charge
        .is_some_and(|charge| !charge.is_finite() || !(0.0..=1.0).contains(&charge))
}

/// §36.1's bands.
fn band(remaining: f64, budget: f64) -> PowerState {
    if remaining < budget {
        PowerState::Critical
    } else if remaining < budget * RESERVE_BAND {
        PowerState::Reserve
    } else if remaining < budget * LOW_BAND {
        PowerState::Low
    } else {
        PowerState::Normal
    }
}

/// The reason that goes with [`band`]. A `match` on the state would lose the distinction between
/// *at reserve* and *below it*, which is the one §36.1 spends two rows on.
fn band_reason(remaining: f64, budget: f64) -> PowerReason {
    match band(remaining, budget) {
        PowerState::Critical => PowerReason::BelowReserve,
        PowerState::Reserve => PowerReason::AtReserve,
        PowerState::Low => PowerReason::ApproachingReserve,
        _ => PowerReason::AboveReserve,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn pack() -> PowerProfile {
        PowerProfile {
            time_to_safe_state_s: Some(2.0),
            safe_stop_energy_wh: Some(20.0),
            pack_capacity_wh: Some(500.0),
            safe_state: None,
            thermal_throttle_c: Some(70.0),
            thermal_critical_c: Some(90.0),
            brownout_fault_codes: vec![4097],
        }
    }

    fn at(charge: f64) -> PowerObservation {
        PowerObservation {
            state_of_charge: Some(charge),
            ..Default::default()
        }
    }

    fn codes(assessment: &Assessment) -> Vec<&'static str> {
        assessment
            .faults
            .iter()
            .map(|fault| fault.code.as_str())
            .collect()
    }

    // ── The bands (§36.1) ────────────────────────────────────────────────────

    #[test]
    fn a_full_pack_is_normal_and_grants_leases() {
        let assessment = assess(&pack(), &at(0.9), None);
        assert_eq!(assessment.state, PowerState::Normal);
        assert_eq!(assessment.reason, PowerReason::AboveReserve);
        assert!(assessment.grants_new_leases());
        assert!(!assessment.requires_safe_state());
        assert!(codes(&assessment).is_empty(), "{:?}", codes(&assessment));
    }

    /// The reserve is 20 Wh of a 500 Wh pack, so the bands sit at 4%, 5% and 9% — which is exactly
    /// §36's opening sentence: a humanoid's interesting range is the bottom tenth of the gauge.
    #[test]
    fn the_bands_are_multiples_of_the_reserve_and_not_of_the_pack() {
        let profile = pack();
        // 45 Wh: above 2.25 × 20.
        assert_eq!(assess(&profile, &at(0.09), None).state, PowerState::Normal);
        // 40 Wh: inside the low band.
        assert_eq!(assess(&profile, &at(0.08), None).state, PowerState::Low);
        // 22.5 Wh: inside the reserve band.
        assert_eq!(
            assess(&profile, &at(0.045), None).state,
            PowerState::Reserve
        );
        // 15 Wh: below the reserve.
        assert_eq!(
            assess(&profile, &at(0.03), None).state,
            PowerState::Critical
        );
    }

    #[test]
    fn low_stops_new_leases_and_does_not_stop_the_robot() {
        let assessment = assess(&pack(), &at(0.08), None);
        assert_eq!(assessment.state, PowerState::Low);
        assert_eq!(assessment.reason, PowerReason::ApproachingReserve);
        assert!(!assessment.grants_new_leases(), "§36.1's low row");
        assert!(
            !assessment.requires_safe_state(),
            "a running task may finish"
        );
        assert!(assessment.state.notifies_operator());
    }

    #[test]
    fn reserve_asks_for_a_safe_state_and_critical_asks_for_it_now() {
        let reserve = assess(&pack(), &at(0.045), None);
        assert!(reserve.requires_safe_state());
        assert!(!reserve.stops_immediately());

        let critical = assess(&pack(), &at(0.03), None);
        assert!(critical.requires_safe_state());
        assert!(critical.stops_immediately());
    }

    /// The band boundaries are inclusive at the bottom, so a robot exactly at its reserve is at
    /// reserve rather than below it. The alternative reading makes a body that computed its budget
    /// perfectly the one that gets the harshest state.
    #[test]
    fn a_body_exactly_at_its_reserve_is_at_reserve_and_not_below_it() {
        let profile = PowerProfile {
            pack_capacity_wh: None,
            ..pack()
        };
        let exactly = PowerObservation {
            energy_remaining_wh: Some(20.0),
            ..Default::default()
        };
        assert_eq!(assess(&profile, &exactly, None).state, PowerState::Reserve);
    }

    /// The bands are inclusive at the bottom throughout, and these are the four readings that pin
    /// [`RESERVE_BAND`] and [`LOW_BAND`] rather than merely sitting inside them.
    ///
    /// Worth having for a reason the corpus found first: with only the interior cases, moving the
    /// reserve band from 1.25 to 1.9 left every frozen vector byte-identical. A constant no case
    /// discriminates is a constant two editions can disagree about while both passing.
    #[test]
    fn the_band_boundaries_are_pinned_and_inclusive_at_the_bottom() {
        let profile = pack();
        let at_wh = |wh: f64| PowerObservation {
            energy_remaining_wh: Some(wh),
            ..Default::default()
        };
        // Reserve is 20 Wh, so the boundaries are 25 and 45.
        assert_eq!(
            assess(&profile, &at_wh(24.999), None).state,
            PowerState::Reserve
        );
        assert_eq!(assess(&profile, &at_wh(25.0), None).state, PowerState::Low);
        assert_eq!(
            assess(&profile, &at_wh(44.999), None).state,
            PowerState::Low
        );
        assert_eq!(
            assess(&profile, &at_wh(45.0), None).state,
            PowerState::Normal
        );
    }

    // ── §36.4, the override ──────────────────────────────────────────────────

    #[test]
    fn a_declared_brownout_code_is_critical_whatever_the_gauge_says() {
        let observation = PowerObservation {
            state_of_charge: Some(1.0),
            rhal_degraded: true,
            fault_code: Some(4097),
            ..Default::default()
        };
        let assessment = assess(&pack(), &observation, None);
        assert_eq!(assessment.state, PowerState::Critical);
        assert_eq!(assessment.reason, PowerReason::Brownout);
        assert!(assessment.stops_immediately());
        assert!(
            codes(&assessment).is_empty(),
            "a declared code is not a fault — it is the mechanism working"
        );
    }

    #[test]
    fn a_fault_code_this_vendor_did_not_call_a_brownout_is_not_one() {
        let observation = PowerObservation {
            state_of_charge: Some(1.0),
            rhal_degraded: true,
            fault_code: Some(9999),
            ..Default::default()
        };
        let assessment = assess(&pack(), &observation, None);
        assert_eq!(
            assessment.state,
            PowerState::Normal,
            "the OS matches a declared list; it does not interpret a vendor code"
        );
    }

    #[test]
    fn a_degraded_rhal_with_no_declared_codes_says_it_cannot_tell() {
        let profile = PowerProfile {
            brownout_fault_codes: Vec::new(),
            ..pack()
        };
        let observation = PowerObservation {
            state_of_charge: Some(0.9),
            rhal_degraded: true,
            fault_code: Some(4097),
            ..Default::default()
        };
        let assessment = assess(&profile, &observation, None);
        assert_eq!(assessment.state, PowerState::Normal);
        assert_eq!(codes(&assessment), ["brownout_undeclarable"]);
        assert!(
            assessment.grants_new_leases(),
            "a gap in a vendor's declaration is not a reason to stop a working robot; it is a \
             reason to say so"
        );
    }

    /// The gauge is not consulted, and neither is the absence of one.
    #[test]
    fn a_brownout_on_a_body_with_no_gauge_is_still_a_brownout() {
        let observation = PowerObservation {
            rhal_degraded: true,
            fault_code: Some(4097),
            ..Default::default()
        };
        let assessment = assess(&pack(), &observation, None);
        assert_eq!(assessment.state, PowerState::Critical);
    }

    // ── The bodies that are not humanoids ────────────────────────────────────

    #[test]
    fn a_body_with_no_pack_is_unmetered_and_keeps_working() {
        let profile = PowerProfile {
            safe_stop_energy_wh: None,
            pack_capacity_wh: None,
            ..pack()
        };
        let assessment = assess(&profile, &PowerObservation::default(), None);
        assert_eq!(assessment.state, PowerState::Unmetered);
        assert_eq!(assessment.reason, PowerReason::NoPack);
        assert!(
            assessment.grants_new_leases(),
            "an arm on mains is not a robot in trouble"
        );
        assert!(codes(&assessment).is_empty());
    }

    #[test]
    fn a_declared_pack_with_a_silent_gauge_is_reported_and_withholds_leases() {
        let assessment = assess(&pack(), &PowerObservation::default(), None);
        assert_eq!(assessment.state, PowerState::Unmetered);
        assert_eq!(assessment.reason, PowerReason::NoReading);
        assert_eq!(codes(&assessment), ["gauge_silent"]);
        assert!(!assessment.grants_new_leases());
        assert!(
            !assessment.stops_immediately(),
            "a dead gauge is not a flat battery, and a robot that stopped on one would stop on a \
             flaky bus read"
        );
    }

    #[test]
    fn a_fraction_with_no_declared_capacity_cannot_be_compared() {
        let profile = PowerProfile {
            pack_capacity_wh: None,
            ..pack()
        };
        let assessment = assess(&profile, &at(0.5), None);
        assert_eq!(assessment.state, PowerState::Unmetered);
        assert_eq!(assessment.reason, PowerReason::NoCapacity);
        assert_eq!(codes(&assessment), ["capacity_undeclared"]);
        assert!(!assessment.grants_new_leases());
    }

    #[test]
    fn a_pack_with_no_safe_stop_energy_has_no_boundary_to_compare_against() {
        let profile = PowerProfile {
            safe_stop_energy_wh: None,
            ..pack()
        };
        let assessment = assess(&profile, &at(0.5), None);
        assert_eq!(assessment.state, PowerState::Unmetered);
        assert_eq!(assessment.reason, PowerReason::NoReserve);
        assert_eq!(codes(&assessment), ["reserve_undeclared"]);
    }

    #[test]
    fn a_state_of_charge_outside_zero_to_one_is_discarded_rather_than_clamped() {
        let assessment = assess(&pack(), &at(1.4), None);
        assert_eq!(
            assessment.reason,
            PowerReason::ReadingUnusable,
            "a gauge to fix, not a descriptor to fill in"
        );
        assert!(codes(&assessment).contains(&"charge_out_of_range"));
        assert!(!assessment.grants_new_leases());
    }

    #[test]
    fn energy_reported_directly_beats_a_fraction_times_a_capacity() {
        let observation = PowerObservation {
            state_of_charge: Some(0.9),
            energy_remaining_wh: Some(10.0),
            ..Default::default()
        };
        let assessment = assess(&pack(), &observation, None);
        assert_eq!(assessment.remaining_wh, Some(10.0));
        assert_eq!(assessment.state, PowerState::Critical);
    }

    // ── The reserve (§36.2) ──────────────────────────────────────────────────

    #[test]
    fn the_reserve_names_the_two_terms_nothing_supplies() {
        let computed = reserve(&pack(), None);
        assert_eq!(computed.wh, Some(20.0));
        assert_eq!(computed.unaccounted, ["payload", "distance_to_safe_state"]);
        assert!(
            !computed.complete(),
            "a reserve missing two of §36.2's three terms is a floor, and complete() is the only \
             thing that says so"
        );
    }

    #[test]
    fn a_body_declaring_no_energy_has_no_reserve_and_names_that_too() {
        let profile = PowerProfile {
            safe_stop_energy_wh: None,
            ..pack()
        };
        let computed = reserve(&profile, None);
        assert_eq!(computed.wh, None);
        assert_eq!(computed.unaccounted[0], "declared_energy");
    }

    #[test]
    fn a_nonsense_safe_stop_energy_is_no_reserve_rather_than_a_boundary_at_zero() {
        for value in [0.0, -5.0, f64::NAN] {
            let profile = PowerProfile {
                safe_stop_energy_wh: Some(value),
                ..pack()
            };
            assert_eq!(reserve(&profile, None).wh, None, "{value}");
        }
    }

    // ── Thermal (§36.3) ──────────────────────────────────────────────────────

    #[test]
    fn the_ladder_is_given_up_in_order_and_never_reaches_the_kernel() {
        assert!(shed(ThermalPressure::Nominal).is_empty());
        assert_eq!(
            shed(ThermalPressure::Throttling),
            ["telemetry", "ota", "blackbox_compression"]
        );
        assert_eq!(
            shed(ThermalPressure::Critical),
            [
                "telemetry",
                "ota",
                "blackbox_compression",
                "inference_rate",
                "non_essential_sensors"
            ]
        );
        for pressure in THERMAL_PRESSURES {
            for given_up in shed(pressure) {
                assert!(
                    !NEVER_SHED.contains(&given_up),
                    "{given_up} at {}",
                    pressure.as_str()
                );
            }
        }
    }

    #[test]
    fn inference_is_given_up_before_the_control_loop_and_after_everything_cheap() {
        let inference = SHED_ORDER
            .iter()
            .position(|rung| *rung == "inference_rate")
            .expect("the ladder has an inference rung");
        assert_eq!(inference, THROTTLE_DEPTH, "held back until critical");
        assert!(!SHED_ORDER.contains(&"control_loop"));
    }

    #[test]
    fn a_brief_excursion_above_the_throttle_point_is_not_a_degraded_robot() {
        let observation = PowerObservation {
            state_of_charge: Some(0.9),
            temperature_c: Some(75.0),
            throttled_for_ms: 4_000,
            ..Default::default()
        };
        let assessment = assess(&pack(), &observation, None);
        assert_eq!(assessment.thermal, ThermalPressure::Throttling);
        assert_eq!(assessment.shed.len(), THROTTLE_DEPTH);
        assert!(!assessment.degraded(), "four seconds is not sustained");
        assert!(codes(&assessment).is_empty());
    }

    #[test]
    fn sustained_throttling_raises_degraded() {
        let observation = PowerObservation {
            state_of_charge: Some(0.9),
            temperature_c: Some(75.0),
            throttled_for_ms: SUSTAINED_THROTTLE_MS,
            ..Default::default()
        };
        let assessment = assess(&pack(), &observation, None);
        assert!(assessment.degraded());
        assert_eq!(codes(&assessment), ["thermal_sustained"]);
        assert!(
            assessment.grants_new_leases(),
            "§36.3's DEGRADED is the impaired running state; it is not a lease refusal"
        );
    }

    #[test]
    fn a_critical_temperature_sheds_everything_and_withholds_leases() {
        let observation = PowerObservation {
            state_of_charge: Some(0.9),
            temperature_c: Some(95.0),
            throttled_for_ms: 120_000,
            ..Default::default()
        };
        let assessment = assess(&pack(), &observation, None);
        assert_eq!(assessment.thermal, ThermalPressure::Critical);
        assert_eq!(assessment.shed.len(), SHED_ORDER.len());
        assert!(assessment.degraded());
        assert!(!assessment.grants_new_leases());
        assert_eq!(
            codes(&assessment),
            ["thermal_critical"],
            "critical rather than critical-and-sustained: one temperature is one fault"
        );
    }

    #[test]
    fn a_temperature_with_no_declared_threshold_sheds_nothing_and_says_why() {
        let profile = PowerProfile {
            thermal_throttle_c: None,
            thermal_critical_c: None,
            ..pack()
        };
        let observation = PowerObservation {
            state_of_charge: Some(0.9),
            temperature_c: Some(95.0),
            ..Default::default()
        };
        let assessment = assess(&profile, &observation, None);
        assert_eq!(assessment.thermal, ThermalPressure::Nominal);
        assert!(assessment.shed.is_empty());
        assert_eq!(codes(&assessment), ["thermal_undeclared"]);
        assert!(!assessment.degraded());
    }

    #[test]
    fn no_temperature_at_all_is_not_a_thermal_fault() {
        let assessment = assess(&pack(), &at(0.9), None);
        assert_eq!(assessment.thermal, ThermalPressure::Nominal);
        assert!(codes(&assessment).is_empty());
    }

    // ── §36.3's last sentence ────────────────────────────────────────────────

    #[test]
    fn a_loop_inside_its_declared_budget_is_not_blocked() {
        assert_eq!(budget_missed(Some(20.0), Some(18.0)), None);
        assert_eq!(
            budget_missed(Some(20.0), Some(20.0)),
            None,
            "at is not over"
        );
    }

    #[test]
    fn a_loop_past_its_declared_budget_is_blocked_and_the_message_names_both_numbers() {
        let refusal = budget_missed(Some(20.0), Some(31.0)).expect("blocked");
        assert!(refusal.contains("20 ms"), "{refusal}");
        assert!(refusal.contains("31 ms"), "{refusal}");
        assert!(refusal.contains("§36.3"), "{refusal}");
    }

    #[test]
    fn a_package_is_never_blocked_on_an_absence() {
        assert_eq!(budget_missed(None, Some(400.0)), None, "no declaration");
        assert_eq!(budget_missed(Some(20.0), None), None, "no measurement");
        assert_eq!(budget_missed(Some(20.0), Some(f64::NAN)), None);
        assert_eq!(budget_missed(Some(0.0), Some(400.0)), None, "no budget");
    }

    // ── The vocabularies ─────────────────────────────────────────────────────

    #[test]
    fn every_state_and_reason_round_trips_its_wire_spelling() {
        for state in POWER_STATES {
            assert_eq!(PowerState::parse(state.as_str()), Some(state));
        }
        assert_eq!(PowerState::parse("Normal"), None, "exact, not lenient");
        // Distinct spellings, which is what a console groups by.
        for (index, state) in POWER_STATES.iter().enumerate() {
            for other in &POWER_STATES[index + 1..] {
                assert_ne!(state.as_str(), other.as_str());
            }
        }
        for (index, reason) in POWER_REASONS.iter().enumerate() {
            for other in &POWER_REASONS[index + 1..] {
                assert_ne!(reason.as_str(), other.as_str());
            }
        }
        for (index, code) in POWER_FAULT_CODES.iter().enumerate() {
            for other in &POWER_FAULT_CODES[index + 1..] {
                assert_ne!(code.as_str(), other.as_str());
            }
        }
    }

    /// The default is the least capable reading everywhere else in this repository, and here it is
    /// the one that claims nothing: an unmetered body with no faults is what a caller who has
    /// observed nothing has established.
    #[test]
    fn the_default_state_claims_nothing() {
        assert_eq!(PowerState::default(), PowerState::Unmetered);
        assert_eq!(ThermalPressure::default(), ThermalPressure::Nominal);
    }
}

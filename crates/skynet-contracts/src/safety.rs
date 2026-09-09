//! The Safety Kernel, and how its output is written down.
//!
//! The kernel itself is [`skynet_kernel`] — `no_std`, no `alloc`, no clock, and deliberately unable
//! to format a sentence. This module is the layer above it that has an allocator: it renders a
//! [`ResolvedLimits`] and a [`CycleVerdict`] into canonical JSON, and it holds the English text for
//! the notice codes the kernel emits as bare enum values.
//!
//! Splitting them that way is not tidiness. A notice message is a string that will one day be
//! translated into thirty-five languages and rendered in a browser; a guard verdict is arithmetic
//! that has to complete inside a control cycle on a Cortex-M. Keeping the second free of the first
//! is what lets the same guard code build for both targets.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use skynet_json::Json;

pub use skynet_kernel::{
    can_start, evaluate_cycle, evaluate_cycle_with_body, fixed::FixedVec, kinematics,
    resolve_limits, Available, Body, Clamped, CommandCycle, CycleVerdict, Enforcement, Envelope,
    FaultResponse, Guard, NoticeCode, OperatorProfile, PoseOutcome, PoseTrip, Proximity,
    ResolveLimitsParams, ResolvedLimits, SafetyNotice, Tier, TransportClamp, UnavailableGuard,
    WarningChannel, GUARDS, MAX_CHANNELS, SAFETY_CLAIM_RULE,
};

/// The English for a notice, given its code and what it names.
///
/// The kernel emits a code and its subjects; this is the only place the sentence lives, so a change
/// to the wording is a change in one file rather than in every edition's kernel.
pub fn notice_message(notice: &SafetyNotice<'_>, profile_name: Option<&str>) -> String {
    match notice.code {
        NoticeCode::AdvisoryOnly => concat!(
            "At the guest tier Skynet OS does not own the control loop. Violations are detected ",
            "and recorded; commands still reach the robot unchanged. This is not a safety function."
        )
        .to_string(),
        NoticeCode::NoIndependentStop => concat!(
            "There is no stop path independent of the application processor. If the processor ",
            "hangs, nothing here stops the robot."
        )
        .to_string(),
        NoticeCode::NoEnvelope => concat!(
            "No safety envelope has been declared for this robot. Only per-joint ratings from the ",
            "body description are available, and whole-body ceilings are unenforced."
        )
        .to_string(),
        NoticeCode::UnsourcedLimits => {
            "The declared limits cite no datasheet. A safety limit without provenance is a rumour."
                .to_string()
        }
        NoticeCode::ProfileWidened => format!(
            "The operator profile \"{}\" asks to raise limits above the declared envelope. Those values are ignored.",
            profile_name.unwrap_or("")
        ),
        NoticeCode::NoRateLimit => concat!(
            "No jerk ceiling is configured, so a single-cycle jump from zero to the clamped ",
            "maximum is permitted. Set max_delta_per_cycle before running a policy near people."
        )
        .to_string(),
        NoticeCode::NoCollisionMargin => concat!(
            "No self-collision margin is configured, so two of this robot's own bodies must ",
            "actually meet before the guard holds. Set self_collision_margin_m to stop the command ",
            "that would bring them together."
        )
        .to_string(),
        NoticeCode::GuardUnavailable => format!(
            "The {} guard cannot run on this robot.",
            notice.guard.map(Guard::as_str).unwrap_or("unknown")
        ),
    }
}

fn subjects_json(subjects: &[&str]) -> Json {
    Json::Arr(subjects.iter().map(|s| Json::from(*s)).collect())
}

/// Serialise a resolved limit set.
pub fn limits_to_json(limits: &ResolvedLimits<'_>, profile_name: Option<&str>) -> Json {
    let mut out = Json::obj();
    out.set("tier", Json::from(limits.tier.as_str()));
    out.set("enforcement", Json::from(limits.enforcement.as_str()));
    out.set(
        "joint_velocity_max_rad_s",
        nullable(limits.joint_velocity_max_rad_s),
    );
    out.set("joint_torque_max_nm", nullable(limits.joint_torque_max_nm));
    out.set(
        "base_velocity_max_m_s",
        nullable(limits.base_velocity_max_m_s),
    );
    out.set("min_control_hz", nullable(limits.min_control_hz));
    out.set("max_delta_per_cycle", nullable(limits.max_delta_per_cycle));
    // §7.1's fifth guard holds at this distance, and until §11 could deliver it the field was
    // always absent and not worth a column. It is deliverable now, so it is reported — for the
    // reason every other number here is: an operator who cannot see the margin in force cannot
    // tell a robot holding 5 cm from one holding at contact.
    out.set(
        "self_collision_margin_m",
        nullable(limits.self_collision_margin_m),
    );

    let mut per_channel = Json::obj();
    for (name, limit) in limits.per_channel {
        per_channel.set(name, Json::Num(*limit));
    }
    out.set("per_channel", per_channel);

    out.set(
        "active_guards",
        Json::Arr(
            limits
                .active_guards
                .iter()
                .map(|g| Json::from(g.as_str()))
                .collect(),
        ),
    );

    let mut unavailable = Json::arr();
    for entry in limits.unavailable_guards.iter() {
        let mut item = Json::obj();
        item.set("guard", Json::from(entry.guard.as_str()));
        let missing: Vec<&str> = entry.missing.iter().copied().collect();
        item.set("missing", subjects_json(&missing));
        unavailable.push(item);
    }
    out.set("unavailable_guards", unavailable);

    // §80's third state, always present. A shape that omitted it where empty would let an edition
    // that never produces it agree with every case that has none.
    let mut inert = Json::arr();
    for entry in limits.inert_guards.iter() {
        let mut item = Json::obj();
        item.set("guard", Json::from(entry.guard.as_str()));
        item.set("reading", Json::from(entry.reading));
        inert.push(item);
    }
    out.set("inert_guards", inert);

    out.set(
        "source",
        match limits.source {
            Some(source) => Json::from(source),
            None => Json::Null,
        },
    );

    let mut notices = Json::arr();
    for notice in limits.notices.iter() {
        let mut item = Json::obj();
        item.set("code", Json::from(notice.code.as_str()));
        item.set("message", Json::Str(notice_message(notice, profile_name)));
        if !notice.subjects.is_empty() {
            let subjects: Vec<&str> = notice.subjects.iter().copied().collect();
            item.set("subjects", subjects_json(&subjects));
        }
        notices.push(item);
    }
    out.set("notices", notices);

    out
}

/// Serialise one cycle's verdict.
pub fn verdict_to_json(verdict: &CycleVerdict<'_>) -> Json {
    let mut out = Json::obj();

    let mut command = Json::obj();
    for (channel, value) in verdict.command.iter() {
        command.set(channel, Json::Num(*value));
    }
    out.set("command", command);

    out.set(
        "response",
        match verdict.response {
            Some(response) => Json::from(response.as_str()),
            None => Json::Null,
        },
    );
    out.set(
        "tripped",
        Json::Arr(
            verdict
                .tripped
                .iter()
                .map(|g| Json::from(g.as_str()))
                .collect(),
        ),
    );

    let mut clamped = Json::arr();
    for entry in verdict.clamped.iter() {
        let mut item = Json::obj();
        item.set("channel", Json::from(entry.channel));
        item.set("from", Json::Num(entry.from));
        item.set("to", Json::Num(entry.to));
        clamped.push(item);
    }
    out.set("clamped", clamped);
    out.set("advisory", Json::Bool(verdict.advisory));
    out.set("pose", pose_to_json(&verdict.pose));

    out
}

/// Serialise what the two pose guards found.
///
/// Emitted on every verdict, empty or not. Omitting it when nothing tripped would make an edition
/// that has not implemented §44 byte-identical to one that ran the guards and found nothing, which
/// is the "passes by not looking" failure the corpus exists to make impossible.
pub fn pose_to_json(pose: &PoseOutcome<'_>) -> Json {
    let mut out = Json::obj();
    out.set("evaluated", Json::Bool(pose.evaluated));
    out.set("lookahead", Json::Bool(pose.lookahead));
    out.set("truncated", Json::Bool(pose.truncated));
    let unresolved: Vec<&str> = pose.unresolved.iter().copied().collect();
    out.set("unresolved", subjects_json(&unresolved));

    let mut trips = Json::arr();
    for trip in pose.trips.iter() {
        let mut item = Json::obj();
        item.set("guard", Json::from(trip.guard.as_str()));
        item.set("frame", Json::from(trip.frame));
        // Present and null on a workspace excursion rather than absent: "this finding is about one
        // frame" is a fact, and a reader of an absent key cannot tell it from a producer that
        // forgot the field.
        item.set(
            "other",
            match trip.other {
                Some(other) => Json::from(other),
                None => Json::Null,
            },
        );
        item.set("margin_m", nullable(trip.margin_m));
        trips.push(item);
    }
    out.set("trips", trips);
    out
}

fn nullable(value: Option<f64>) -> Json {
    match value {
        Some(number) => Json::Num(number),
        None => Json::Null,
    }
}

/// The tier model, rendered for a surface that has to explain it.
///
/// Every field is derived from [`Tier`] rather than stored, so a UI cannot show a tier as enforcing
/// when the kernel would resolve it to advisory.
pub fn tier_profile_to_json(tier: Tier) -> Json {
    let mut out = Json::obj();
    out.set("tier", Json::from(tier.as_str()));
    out.set("enforcement", Json::from(tier.enforcement().as_str()));
    out.set(
        "owns_control_loop",
        Json::Bool(!matches!(tier, Tier::Guest)),
    );
    out.set(
        "independent_stop_path",
        Json::Bool(tier.independent_stop_path()),
    );
    out.set(
        "supports_safety_case",
        Json::Bool(tier.supports_safety_case()),
    );
    out.set("summary", Json::from(tier_summary(tier)));
    out.set("substrate", Json::from(tier_substrate(tier)));
    // Carried on every tier, not only the certified one, so a surface cannot render a tier without
    // also having the sentence that qualifies it.
    out.set("safety_claim_rule", Json::from(SAFETY_CLAIM_RULE));
    out
}

fn tier_summary(tier: Tier) -> &'static str {
    match tier {
        Tier::Guest => {
            "Runs beside the robot's existing stack. Observes and advises; enforces nothing."
        }
        Tier::Host => {
            "Skynet OS is the robot's onboard operating system and owns the control loop."
        }
        Tier::Certified => "Host tier plus an independent safety processor with its own stop path.",
    }
}

fn tier_substrate(tier: Tier) -> &'static str {
    match tier {
        Tier::Guest => "A container or process on any compute that can reach the robot over IP, serial or CAN.",
        Tier::Host => "PREEMPT_RT Linux on the robot's application processor, driving hardware through an RHAL driver.",
        Tier::Certified => "PREEMPT_RT Linux for the policy runtime; the kernel mirrored on a separate safety MCU wired into the drive-enable line.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tier_carries_the_claim_rule() {
        for tier in [Tier::Guest, Tier::Host, Tier::Certified] {
            let json = tier_profile_to_json(tier);
            let rule = json.get("safety_claim_rule").unwrap().as_str().unwrap();
            assert!(rule.contains("does not certify a machine"));
        }
    }

    #[test]
    fn a_guard_unavailable_notice_names_the_guard() {
        let limits = resolve_limits(&ResolveLimitsParams {
            tier: Tier::Host,
            envelope: None,
            transport: None,
            profile: None,
            frames: None,
            available: Available::default(),
        });
        let json = limits_to_json(&limits, None);
        let notices = json.get("notices").unwrap().as_arr().unwrap();
        let text: Vec<&str> = notices
            .iter()
            .filter_map(|n| n.get("message").and_then(Json::as_str))
            .collect();
        assert!(text
            .iter()
            .any(|m| m.contains("The watchdog guard cannot run")));
        assert!(text
            .iter()
            .any(|m| m.contains("The balance guard cannot run")));
    }

    #[test]
    fn a_widened_profile_notice_names_the_profile() {
        let envelope = Envelope {
            joint_torque_max_nm: Some(80.0),
            ..Default::default()
        };
        let profile = OperatorProfile {
            name: "night-shift",
            joint_torque_max_nm: Some(500.0),
            ..Default::default()
        };
        let limits = resolve_limits(&ResolveLimitsParams {
            tier: Tier::Host,
            envelope: Some(envelope),
            transport: None,
            profile: Some(profile),
            frames: None,
            available: Available::default(),
        });
        let json = limits_to_json(&limits, Some("night-shift"));
        let rendered = json.to_canonical().unwrap();
        assert!(rendered.contains("night-shift"));
    }

    #[test]
    fn a_verdict_serialises_to_canonical_json() {
        let envelope = Envelope {
            watchdog_ms: Some(100.0),
            ..Default::default()
        };
        let per_channel = [("elbow", 80.0)];
        let clamp = TransportClamp {
            per_channel: &per_channel,
            ..Default::default()
        };
        let limits = resolve_limits(&ResolveLimitsParams {
            tier: Tier::Host,
            envelope: Some(envelope),
            transport: Some(clamp),
            profile: None,
            frames: None,
            available: Available::default(),
        });
        let desired = [("elbow", 120.0)];
        let cycle = CommandCycle {
            now_ms: 10.0,
            command_ms: 10.0,
            sequence: 1,
            desired: &desired,
            previous: None,
            capability: None,
            nearest_person_m: None,
        };
        let verdict = evaluate_cycle(&cycle, &limits, Some(&envelope), None);
        let rendered = verdict_to_json(&verdict).to_canonical().unwrap();
        assert_eq!(
            rendered,
            r#"{"advisory":false,"clamped":[{"channel":"elbow","from":120,"to":80}],"command":{"elbow":80},"pose":{"evaluated":false,"lookahead":false,"trips":[],"truncated":false,"unresolved":[]},"response":"clamp","tripped":["envelope"]}"#
        );
    }
}

// ── Reading a resolved limit set back (§13, §83.5) ───────────────────────────

/// Why a `GET /limits` document could not be turned back into a limit set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LimitsError {
    /// The document was not a JSON object, or a required field was missing or the wrong shape.
    Malformed {
        /// The field that was wrong, or absent.
        field: &'static str,
    },
    /// A tier or enforcement spelling this edition does not know.
    Unknown {
        /// Which field carried it.
        field: &'static str,
    },
}

/// A guard the document says was **active** whose input the document does not carry.
///
/// `GET /limits` is written for an operator reading a console: it renders the guard *partition* and
/// the ceilings, and leaves out two things the kernel evaluates — §7.1's separation thresholds and
/// the operator's capability grants. So a limit set read back from one has those two guards active
/// with nothing behind them.
///
/// This is reported rather than refused, and where the line falls is the whole point. `mode_gate` is
/// active on every ordinary robot, so refusing here would refuse every real document; and it does
/// nothing at all on a cycle that exercised no capability. Whether the absence matters is a fact
/// about the **window**, not about the document, so the decision belongs to whoever holds both. A
/// caller replaying a window in which the input never appears is on safe ground and should say so;
/// one replaying a window in which it does appear is comparing against a robot that is not the one
/// that produced it, and must refuse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreconstructed {
    /// The guard the document listed as active.
    pub guard: Guard,
    /// The input a limits document does not carry.
    pub missing: &'static str,
    /// What in a recording would have engaged it, so a caller can look for exactly that.
    pub engaged_by: &'static str,
}

impl LimitsError {
    /// One sentence, naming the fix (§64).
    pub fn detail(&self) -> String {
        match self {
            LimitsError::Malformed { field } => format!(
                "the limits document has no usable `{field}`. It should be the body of \
                 `GET /limits` from the robot, as `skynet limits --json` prints it."
            ),
            LimitsError::Unknown { field } => format!(
                "the limits document's `{field}` is a spelling this edition does not know. A \
                 newer robot's document may need a newer `skynet`."
            ),
        }
    }
}

/// A limit set read back from a document, holding the strings the borrowed form points at.
///
/// The same shape `skynetd`'s own limits take, and for the same reason: [`ResolvedLimits`] borrows
/// its per-channel table and its provenance string, so something has to own them for as long as the
/// borrow lives. [`OwnedLimits::with_resolved`] is that scope.
#[derive(Debug, Clone, PartialEq)]
pub struct OwnedLimits {
    tier: Tier,
    enforcement: Enforcement,
    joint_velocity_max_rad_s: Option<f64>,
    joint_torque_max_nm: Option<f64>,
    base_velocity_max_m_s: Option<f64>,
    min_control_hz: Option<f64>,
    max_delta_per_cycle: Option<f64>,
    self_collision_margin_m: Option<f64>,
    per_channel: Vec<(String, f64)>,
    active_guards: Vec<Guard>,
    unreconstructed: Vec<Unreconstructed>,
    source: Option<String>,
    envelope: Option<OwnedEnvelope>,
}

/// The declared envelope, when the document carries one. See [`OwnedLimits::with_resolved`].
#[derive(Debug, Clone, PartialEq)]
struct OwnedEnvelope {
    joint_velocity_max_rad_s: Option<f64>,
    joint_torque_max_nm: Option<f64>,
    base_velocity_max_m_s: Option<f64>,
    watchdog_ms: Option<f64>,
    estop: Option<bool>,
    source: Option<String>,
}

impl OwnedLimits {
    /// The tier the document was resolved at.
    pub fn tier(&self) -> Tier {
        self.tier
    }

    /// The enforcement mode the document reported.
    pub fn enforcement(&self) -> Enforcement {
        self.enforcement
    }

    /// The guards the document said would run.
    pub fn active_guards(&self) -> &[Guard] {
        &self.active_guards
    }

    /// Active guards this document could not supply an input for. See [`Unreconstructed`].
    pub fn unreconstructed(&self) -> &[Unreconstructed] {
        &self.unreconstructed
    }

    /// Hand the borrowed form to `f`, with the envelope when the document carried one.
    ///
    /// # The envelope is not decoration, and leaving it out made every real replay wrong
    ///
    /// `resolve_limits` folds an envelope into the ceilings, so the resolved set is complete
    /// for seven of the eight guards and the envelope beside it looks like provenance. It is
    /// not, for one field: [`evaluate_cycle`] reads `watchdog_ms` **off the envelope**, because
    /// staleness is a property of the declaration rather than a resolved ceiling. So a limit
    /// set reconstructed without one has `watchdog` active and no window to measure against,
    /// and every cycle whose command was stale replays as one that was not.
    ///
    /// That was found by replaying a real 300-cycle window against its own `GET /limits`
    /// document and watching two recorded `stop_cat2` verdicts come back as `none` — a replay
    /// saying the robot kept going where the recording says it stopped, which is the one
    /// direction this must never be wrong in.
    pub fn with_resolved<T>(
        &self,
        f: impl FnOnce(&ResolvedLimits<'_>, Option<&Envelope<'_>>) -> T,
    ) -> T {
        let per_channel: Vec<(&str, f64)> = self
            .per_channel
            .iter()
            .map(|(name, value)| (name.as_str(), *value))
            .collect();
        let mut active = FixedVec::new();
        for guard in &self.active_guards {
            active.push(*guard);
        }
        let envelope = self.envelope.as_ref().map(|held| Envelope {
            joint_velocity_max_rad_s: held.joint_velocity_max_rad_s,
            joint_torque_max_nm: held.joint_torque_max_nm,
            base_velocity_max_m_s: held.base_velocity_max_m_s,
            watchdog_ms: held.watchdog_ms,
            estop: held.estop,
            source: held.source.as_deref(),
        });
        let limits = ResolvedLimits {
            tier: self.tier,
            enforcement: self.enforcement,
            joint_velocity_max_rad_s: self.joint_velocity_max_rad_s,
            joint_torque_max_nm: self.joint_torque_max_nm,
            base_velocity_max_m_s: self.base_velocity_max_m_s,
            min_control_hz: self.min_control_hz,
            max_delta_per_cycle: self.max_delta_per_cycle,
            proximity: None,
            per_channel: &per_channel,
            active_guards: active,
            unavailable_guards: FixedVec::new(),
            inert_guards: FixedVec::new(),
            source: self.source.as_deref(),
            notices: FixedVec::new(),
            grants: &[],
            frames: None,
            workspace: &[],
            self_collision_margin_m: self.self_collision_margin_m,
        };
        f(&limits, envelope.as_ref())
    }
}

/// Read back what [`limits_to_json`] wrote.
///
/// The inverse lives beside the forward direction for §78's reason: two spellings of one document
/// in two files agree until somebody changes one of them, and this pair is read by a replay whose
/// whole value is that it is comparing against the limits that were actually in force.
///
/// It is **not** a complete inverse and says so by refusing rather than by filling in. See
/// [`LimitsError::GuardWithoutInput`].
pub fn limits_from_json(document: &Json) -> Result<OwnedLimits, LimitsError> {
    if document.as_obj().is_none() {
        return Err(LimitsError::Malformed { field: "(root)" });
    }

    let tier = document
        .get("tier")
        .and_then(Json::as_str)
        .ok_or(LimitsError::Malformed { field: "tier" })?;
    let tier = Tier::parse(tier).ok_or(LimitsError::Unknown { field: "tier" })?;

    let enforcement =
        document
            .get("enforcement")
            .and_then(Json::as_str)
            .ok_or(LimitsError::Malformed {
                field: "enforcement",
            })?;
    let enforcement = Enforcement::parse(enforcement).ok_or(LimitsError::Unknown {
        field: "enforcement",
    })?;

    let mut per_channel = Vec::new();
    match document.get("per_channel") {
        Some(value) => {
            let Some(table) = value.as_obj() else {
                return Err(LimitsError::Malformed {
                    field: "per_channel",
                });
            };
            for (name, limit) in table {
                let Some(limit) = limit.as_f64() else {
                    return Err(LimitsError::Malformed {
                        field: "per_channel",
                    });
                };
                per_channel.push((name.clone(), limit));
            }
        }
        None => {
            return Err(LimitsError::Malformed {
                field: "per_channel",
            })
        }
    }

    // Optional, and its absence is the ordinary case: `GET /limits` does not carry one, so a
    // document straight off a robot reaches here without it. `skynetk` writes one beside every
    // window it seals precisely so that a replay of that window has the field the watchdog is
    // measured against.
    let envelope = document.get_present("envelope").map(|held| OwnedEnvelope {
        joint_velocity_max_rad_s: number(held, "joint_velocity_max_rad_s"),
        joint_torque_max_nm: number(held, "joint_torque_max_nm"),
        base_velocity_max_m_s: number(held, "base_velocity_max_m_s"),
        watchdog_ms: number(held, "watchdog_ms"),
        estop: held.get("estop").and_then(Json::as_bool),
        source: held
            .get("source")
            .and_then(Json::as_str)
            .map(ToString::to_string),
    });

    let mut active_guards = Vec::new();
    let mut unreconstructed = Vec::new();
    let Some(listed) = document.get("active_guards").and_then(Json::as_arr) else {
        return Err(LimitsError::Malformed {
            field: "active_guards",
        });
    };
    for entry in listed {
        let Some(name) = entry.as_str() else {
            return Err(LimitsError::Malformed {
                field: "active_guards",
            });
        };
        let guard = Guard::parse(name).ok_or(LimitsError::Unknown {
            field: "active_guards",
        })?;
        if guard == Guard::Watchdog && envelope.is_none() {
            unreconstructed.push(Unreconstructed {
                guard,
                missing: "the command timeout it measures staleness against \
                          (`envelope.watchdog_ms`)",
                engaged_by: "a cycle carrying a command",
            });
        }
        if guard == Guard::Proximity {
            unreconstructed.push(Unreconstructed {
                guard,
                missing: "the separation thresholds (§7.1's stop and slow distances, and the \
                          slow factor)",
                engaged_by: "a cycle carrying a proximity reading",
            });
        }
        if guard == Guard::ModeGate {
            unreconstructed.push(Unreconstructed {
                guard,
                missing: "the operator's capability grants (§8.2)",
                engaged_by: "a cycle carrying a capability token",
            });
        }
        active_guards.push(guard);
    }

    Ok(OwnedLimits {
        tier,
        enforcement,
        joint_velocity_max_rad_s: number(document, "joint_velocity_max_rad_s"),
        joint_torque_max_nm: number(document, "joint_torque_max_nm"),
        base_velocity_max_m_s: number(document, "base_velocity_max_m_s"),
        min_control_hz: number(document, "min_control_hz"),
        max_delta_per_cycle: number(document, "max_delta_per_cycle"),
        self_collision_margin_m: number(document, "self_collision_margin_m"),
        per_channel,
        active_guards,
        unreconstructed,
        envelope,
        source: document
            .get("source")
            .and_then(Json::as_str)
            .map(ToString::to_string),
    })
}

/// A ceiling the document writes as a number or as `null`, where `null` is *no ceiling declared*.
fn number(document: &Json, key: &str) -> Option<f64> {
    document.get(key).and_then(Json::as_f64)
}

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
    can_start, evaluate_cycle, evaluate_cycle_with_body, kinematics, resolve_limits, Available,
    Body, Clamped, CommandCycle, CycleVerdict, Enforcement, Envelope, FaultResponse, Guard,
    NoticeCode, OperatorProfile, PoseOutcome, PoseTrip, Proximity, ResolveLimitsParams,
    ResolvedLimits, SafetyNotice, Tier, TransportClamp, UnavailableGuard, WarningChannel, GUARDS,
    MAX_CHANNELS, SAFETY_CLAIM_RULE,
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

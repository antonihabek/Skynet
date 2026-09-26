//! Power-on self-test: §48.2's five kernel checks, run before the loop turns.
//!
//! §48.5 divides POST down the middle. The supervisor owns three checks it can run without
//! touching an actuator; these five need the driver, so the kernel runs them here, records each
//! on §13's ring, and the supervisor's `post::run` reads the verdicts back instead of running
//! the checks itself. What crosses the ring is the verdict and one sentence of evidence — never
//! the authority to have run the check, which stays here.
//!
//! The checks run in §48.2's order, on the session the loop will use:
//!
//! 1. `rhal_abi` — the driver speaks the ABI this OS implements. The load gate (`accepts`)
//!    already refused anything else; this is the record that it did, re-read rather than
//!    assumed.
//! 2. `describe_matches_descriptor` — `describe()` parses, and the joints it claims are the
//!    joints the probe read. A driver disagreeing with itself about the body is not run.
//! 3. `estop_asserted_and_observed` — the stop is asserted, the flag is observed, and the
//!    session is returned: released in place where the driver has a software release, renewed
//!    (close, open) where it has not. Asserting the stop and observing that the RHAL reported it
//!    is the only evidence the stop path works today (§48.2).
//! 4. `encoder_sanity` — every position finite, no joint missing. Velocity and effort may be
//!    NaN where a joint does not measure them (§24); infinity is implausible everywhere.
//! 5. `kernel_corpus_selfcheck` — three vectors through the linked kernel, held to §31.4.
//!
//! A failed check fails the boot: the caller records every verdict first, then exits without
//! turning the loop. A robot that failed its own self-test goes to SAFE (§48.2), and the loop
//! never running is what sends it there — the supervisor reads the failure off the ring and
//! holds it.

use crate::driver::{PostBody, ReleaseFailure, Write};

/// One check's verdict: its wire-stable name, whether it passed, and the evidence in one
/// sentence.
///
/// The names are `GET /post`'s `check` codes, shared with the supervisor's three. Renaming one
/// is a wire change, on the rule the supervisor's reader states where it translates them.
pub struct CheckVerdict {
    /// The §48.2 check, e.g. `rhal_abi`.
    pub name: &'static str,
    /// Whether it passed.
    pub passed: bool,
    /// One sentence of evidence, terse enough for the event channel's 96 bytes.
    pub detail: String,
}

/// What the caller does with the session POST ran on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionAction {
    /// The session was released in place (or never stopped being commandable): run the loop on
    /// it.
    Keep,
    /// The driver has no software release: close and reopen before the loop turns. The software
    /// latch dies with the session (§4); a hardware latch reporting honestly in the new session
    /// holds the robot through §27 rather than through POST.
    Renew,
}

/// The five verdicts, what to do with the session, and two facts the descriptor established.
pub struct Report {
    /// The five checks, in §48.2's order.
    pub checks: Vec<CheckVerdict>,
    /// Whether the loop runs on this session or a renewed one.
    pub action: SessionAction,
    /// Whether the body declares any joint control mode. Absent `control.modes` counts as
    /// commandable: a body that takes commands but declares nothing is the common case for a
    /// minimal descriptor, and only an explicit empty list excuses a refusal.
    pub commandable: bool,
    /// Whether the descriptor's envelope declares a rated stop fitted (`envelope.estop`).
    pub rated_stop: bool,
}

/// Whether every check passed.
pub fn passed(report: &Report) -> bool {
    report.checks.iter().all(|check| check.passed)
}

/// Run the five checks on the session the loop will use.
pub fn run_post(body: &mut impl PostBody) -> Report {
    let joints = body.joints();

    let abi = check_abi(body);
    let (describe, commandable, hold_mode, rated_stop) = check_describe(body, joints);
    let (estop, action) = check_estop(body, joints, commandable, hold_mode, rated_stop);
    let encoder = check_encoders(body, joints);
    let corpus = check_corpus();

    Report {
        checks: vec![abi, describe, estop, encoder, corpus],
        action,
        commandable,
        rated_stop,
    }
}

/// A mode value back to the word the descriptor declared it under, for verdicts.
///
/// The fallback names position rather than panicking: every stored mode came from
/// [`hold_mode_for`], so anything else is unreachable — and a verdict formatter must not be what
/// fails a boot.
fn mode_name(mode: u8) -> &'static str {
    if mode == skynet_rhal::Mode::Position as u8 {
        "position"
    } else if mode == skynet_rhal::Mode::Velocity as u8 {
        "velocity"
    } else if mode == skynet_rhal::Mode::Torque as u8 {
        "torque"
    } else {
        "position"
    }
}

/// A declared control mode this OS can send a hold in.
fn hold_mode_for(name: &str) -> Option<u8> {
    match name {
        "position" => Some(skynet_rhal::Mode::Position as u8),
        "velocity" => Some(skynet_rhal::Mode::Velocity as u8),
        "torque" => Some(skynet_rhal::Mode::Torque as u8),
        _ => None,
    }
}

fn check_abi(body: &impl PostBody) -> CheckVerdict {
    let found = body.abi_version();
    let expected = skynet_rhal::ABI_VERSION;
    CheckVerdict {
        name: "rhal_abi",
        passed: found == expected,
        detail: if found == expected {
            format!("driver speaks RHAL ABI {found}, verified at load and re-read here")
        } else {
            format!("driver reports RHAL ABI {found}; this OS implements {expected}")
        },
    }
}

/// The describe verdict, whether the body takes joint commands, the mode a hold goes out
/// in, and whether a rated stop is fitted.
fn check_describe(
    body: &mut impl PostBody,
    joints: usize,
) -> (CheckVerdict, bool, Option<u8>, bool) {
    // Fail-closed in every direction: a body whose descriptor never parsed is treated as
    // commandable in position mode with no rated stop, so the estop check verifies strictly and
    // grades a stuck flag as a failure rather than excusing it.
    let fail = |detail: String| {
        (
            CheckVerdict {
                name: "describe_matches_descriptor",
                passed: false,
                detail,
            },
            true,
            Some(skynet_rhal::Mode::Position as u8),
            false,
        )
    };
    let text = match body.describe_text() {
        Ok(text) => text,
        Err(error) => return fail(format!("describe() refused: {error}")),
    };
    let parsed = match skynet_json::Json::parse(&text) {
        Ok(parsed) => parsed,
        Err(_) => return fail("describe() output does not parse as JSON".to_string()),
    };
    let described = parsed
        .get("joints")
        .and_then(skynet_json::Json::as_arr)
        .map(|joints| joints.len());
    let Some(described) = described else {
        return fail("describe() names no `joints` array".to_string());
    };
    if described != joints {
        return fail(format!(
            "describe() claims {described} joints but the probe read {joints}"
        ));
    }
    let declared: Option<Vec<&str>> = parsed
        .get("control")
        .and_then(|control| control.get("modes"))
        .and_then(skynet_json::Json::as_arr)
        .map(|modes| modes.iter().filter_map(skynet_json::Json::as_str).collect());
    // An explicit empty list is a body that takes no joint commands (§59.2); a missing list is
    // a minimal descriptor on a body that takes them, verified in position mode.
    let commandable = declared
        .as_ref()
        .map(|modes| !modes.is_empty())
        .unwrap_or(true);
    let hold_mode = match &declared {
        None => Some(skynet_rhal::Mode::Position as u8),
        Some(modes) => modes.iter().filter_map(|mode| hold_mode_for(mode)).next(),
    };
    let rated_stop = parsed
        .get("envelope")
        .and_then(|envelope| envelope.get("estop"))
        .and_then(skynet_json::Json::as_bool)
        .unwrap_or(false);
    (
        CheckVerdict {
            name: "describe_matches_descriptor",
            passed: true,
            detail: format!("describe() parses; {described} joints match the probe"),
        },
        commandable,
        hold_mode,
        rated_stop,
    )
}

/// How many reads the verifier waits for the flag to clear after a release.
///
/// A software latch clears on the next read; a controller walking out of its stopped mode takes
/// a read or two (the Franka box leaves user-stopped once a new motion arrives). Twenty bounds
/// the wait at twenty reads with no sleep — reads are the pace — and a flag still set then is
/// either the hardware reporting honestly or a release that never happened.
const FLAG_SETTLE_READS: usize = 20;

/// Assert the stop, observe the flag, and return the session — in place or renewed.
fn check_estop(
    body: &mut impl PostBody,
    joints: usize,
    commandable: bool,
    hold_mode: Option<u8>,
    rated_stop: bool,
) -> (CheckVerdict, SessionAction) {
    let fail = |detail: String| {
        (
            CheckVerdict {
                name: "estop_asserted_and_observed",
                passed: false,
                detail,
            },
            SessionAction::Keep,
        )
    };
    if !body.has_stop() {
        return fail("this body has no emergency stop: nothing to assert, and nothing runs".into());
    }
    if let Err(error) = body.stop() {
        return fail(format!("emergency_stop refused: {error}"));
    }
    let observed = match body.sample(0) {
        Ok(sample) => sample.flags & skynet_rhal::flags::ESTOP != 0,
        Err(error) => return fail(format!("no state after the stop: {}", error.detail)),
    };
    if !observed {
        return fail("stop asserted but SKR_FLAG_ESTOP never set: the path is not proven".into());
    }

    if !body.has_release() {
        return (
            CheckVerdict {
                name: "estop_asserted_and_observed",
                passed: true,
                detail: "asserted and observed; no software release, so the session is renewed"
                    .to_string(),
            },
            SessionAction::Renew,
        );
    }
    match body.release() {
        Err(ReleaseFailure::Absent) => (
            CheckVerdict {
                name: "estop_asserted_and_observed",
                passed: true,
                detail: "asserted and observed; release answers absent, so the session is renewed"
                    .to_string(),
            },
            SessionAction::Renew,
        ),
        Err(ReleaseFailure::Failed(detail)) => fail(format!("release failed: {detail}")),
        Ok(()) => verify_release(body, joints, commandable, hold_mode, rated_stop),
    }
}

/// The session was released; prove it is commandable again.
fn verify_release(
    body: &mut impl PostBody,
    joints: usize,
    commandable: bool,
    hold_mode: Option<u8>,
    rated_stop: bool,
) -> (CheckVerdict, SessionAction) {
    let fail = |detail: String| {
        (
            CheckVerdict {
                name: "estop_asserted_and_observed",
                passed: false,
                detail,
            },
            SessionAction::Keep,
        )
    };
    // The hold goes out before the flag is judged: a controller that leaves its stopped mode
    // when a new motion arrives (the Franka box) needs the motion first, and a hold over every
    // joint moves nothing. On a body that takes no joint commands there is no motion to send,
    // and the flag alone is the verdict. The hold goes out in the body's own first declared
    // mode — a position hold on a torque-only arm would be refused for the wrong reason.
    if commandable {
        let Some(mode) = hold_mode else {
            return fail("declares control modes but none this OS sends".to_string());
        };
        let values = vec![0.0f32; joints];
        let mask = vec![0u8; joints];
        let rc = body.command(
            Write {
                stamp_ns: 0,
                seq: 1,
                mode,
                values: &values,
                mask: &mask,
                modes: &[],
            },
            0,
        );
        if rc != 0 {
            return fail(format!(
                "release returned but a {} hold over {joints} joints was refused ({rc})",
                mode_name(mode),
            ));
        }
    }
    let mut reads = 0;
    let cleared = loop {
        reads += 1;
        match body.sample(0) {
            Ok(sample) => {
                if sample.flags & skynet_rhal::flags::ESTOP == 0 {
                    break true;
                }
            }
            Err(error) => return fail(format!("no state after release: {}", error.detail)),
        }
        if reads >= FLAG_SETTLE_READS {
            break false;
        }
    };
    if !cleared {
        if rated_stop {
            return (
                CheckVerdict {
                    name: "estop_asserted_and_observed",
                    passed: true,
                    detail: "released; the chain is still open on a rated stop, which the \
                             hardware reports honestly"
                        .to_string(),
                },
                SessionAction::Keep,
            );
        }
        return fail(
            "release returned but SKR_FLAG_ESTOP is still set, with no rated stop declared".into(),
        );
    }
    let detail = if commandable {
        // `hold_mode` is `Some` here: `None` returned above.
        let mode = mode_name(hold_mode.unwrap_or(skynet_rhal::Mode::Position as u8));
        if reads > 1 {
            format!("asserted, observed, released; flag clear after {reads} reads, {mode} hold ok")
        } else {
            format!("asserted, observed, released; a {mode} hold over {joints} joints accepted")
        }
    } else {
        "asserted, observed, released; this body takes no joint commands".to_string()
    };
    (
        CheckVerdict {
            name: "estop_asserted_and_observed",
            passed: true,
            detail,
        },
        SessionAction::Keep,
    )
}

/// Every position finite, no joint missing, nothing infinite anywhere.
fn check_encoders(body: &mut impl PostBody, joints: usize) -> CheckVerdict {
    let fail = |detail: String| CheckVerdict {
        name: "encoder_sanity",
        passed: false,
        detail,
    };
    if let Err(error) = body.sample(0) {
        return fail(format!("no state to judge encoders on: {}", error.detail));
    }
    let (position, velocity, effort) = body.state();
    if position.len() != joints {
        return fail(format!(
            "state carries {} positions for {joints} joints",
            position.len()
        ));
    }
    for (index, value) in position.iter().enumerate() {
        if !value.is_finite() {
            return fail(format!("joint {index} position is {value}: a dead encoder"));
        }
    }
    // NaN velocity or effort is a joint that does not measure it (§24) and passes; infinity is
    // implausible everywhere — no sensor reports it, and no guard can use it.
    for (name, values) in [("velocity", velocity), ("effort", effort)] {
        for (index, value) in values.iter().enumerate() {
            if value.is_infinite() {
                return fail(format!(
                    "joint {index} {name} is {value}: no sensor reports that"
                ));
            }
        }
    }
    CheckVerdict {
        name: "encoder_sanity",
        passed: true,
        detail: format!("{joints} positions finite"),
    }
}

/// Three vectors through the linked kernel, held to §31.4.
///
/// The robot's guarantee is only as good as the kernel on it: a pass-through, a clamp and a
/// watchdog trip evaluate through the real [`skynet_kernel::evaluate_cycle_with_body`] and the
/// verdicts must reproduce the certified ones. The vectors are hand-built — the corpus files are
/// not shipped on a robot — and `selfcheck_vectors_match_the_corpus` pins each to the case it
/// came from, so a corpus regeneration that moves a verdict fails there until the sample is
/// re-derived. The comparison is on the verdict JSON, which is what §31.4's byte-identity is
/// about.
fn check_corpus() -> CheckVerdict {
    let fail = |detail: String| CheckVerdict {
        name: "kernel_corpus_selfcheck",
        passed: false,
        detail,
    };
    for vector in SELFCHECK_VECTORS {
        let verdict = evaluate_vector(vector);
        if verdict != vector.expected {
            return fail(format!(
                "vector {} disagrees with §31.4: this is not the certified kernel",
                vector.case
            ));
        }
    }
    CheckVerdict {
        name: "kernel_corpus_selfcheck",
        passed: true,
        detail: format!(
            "{}/{} kernel vectors reproduce §31.4",
            SELFCHECK_VECTORS.len(),
            SELFCHECK_VECTORS.len()
        ),
    }
}

/// One hand-built case: its corpus source, its cycle inputs, and the certified verdict.
struct SelfcheckVector {
    /// The corpus file and case this was derived from, e.g. `cycle/empty.json:no-channels`.
    case: &'static str,
    now_ms: f64,
    command_ms: f64,
    sequence: u64,
    desired: &'static [(&'static str, f64)],
    /// The certified `verdict_to_json`, pinned by test to the case above.
    expected: &'static str,
}

/// The sample: an empty command that is not an error, a clamp to a rating, and a stale command
/// discarded. All three share the corpus cases' limits; only the cycle differs.
const SELFCHECK_VECTORS: &[SelfcheckVector] = &[
    SelfcheckVector {
        case: "cycle/empty.json:no-channels",
        now_ms: 1000.0,
        command_ms: 995.0,
        sequence: 42,
        desired: &[],
        expected: "{\"advisory\":false,\"clamped\":[],\"command\":{},\"pose\":{\"evaluated\":false,\"lookahead\":false,\"trips\":[],\"truncated\":false,\"unresolved\":[]},\"response\":null,\"tripped\":[]}",
    },
    SelfcheckVector {
        case: "cycle/guards.json:clamped-to-rating",
        now_ms: 1000.0,
        command_ms: 995.0,
        sequence: 42,
        desired: &[("elbow", 120.0)],
        expected: "{\"advisory\":false,\"clamped\":[{\"channel\":\"elbow\",\"from\":120,\"to\":80}],\"command\":{\"elbow\":80},\"pose\":{\"evaluated\":false,\"lookahead\":false,\"trips\":[],\"truncated\":false,\"unresolved\":[]},\"response\":\"clamp\",\"tripped\":[\"envelope\"]}",
    },
    SelfcheckVector {
        case: "cycle/guards.json:stale-command-discarded",
        now_ms: 1000.0,
        command_ms: 800.0,
        sequence: 42,
        desired: &[("elbow", 500.0)],
        expected: "{\"advisory\":false,\"clamped\":[],\"command\":{},\"pose\":{\"evaluated\":false,\"lookahead\":false,\"trips\":[],\"truncated\":false,\"unresolved\":[]},\"response\":\"stop_cat2\",\"tripped\":[\"watchdog\"]}",
    },
];

fn evaluate_vector(vector: &SelfcheckVector) -> String {
    use skynet_kernel::{
        evaluate_cycle_with_body, resolve_limits, Available, Body, CommandCycle, Envelope,
        OperatorProfile, ResolveLimitsParams, Tier, TransportClamp,
    };
    // The corpus cases' limits, hand-built: host tier, the cell-a envelope, transport and
    // profile. Built beside the call rather than in a helper, because the resolved limits
    // borrow these and a helper would have to hand back borrowed locals.
    let envelope = Envelope {
        joint_velocity_max_rad_s: Some(2.0),
        joint_torque_max_nm: Some(80.0),
        base_velocity_max_m_s: Some(1.5),
        watchdog_ms: Some(100.0),
        estop: Some(true),
        // Provenance, unread by every guard — the pin test holds the verdict equal to the corpus
        // case carrying the URL, so the sample need not ship a datasheet link to nowhere.
        source: None,
        basis: None,
    };
    let transport = TransportClamp {
        joint_velocity_max_rad_s: Some(2.0),
        joint_torque_max_nm: Some(80.0),
        base_velocity_max_m_s: Some(1.5),
        min_control_hz: Some(10.0),
        per_channel: &[("elbow", 80.0), ("shoulder", 60.0)],
        joint_modes: &[],
        source: None,
    };
    let profile = OperatorProfile {
        name: "cell-a",
        joint_velocity_max_rad_s: None,
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        max_delta_per_cycle: Some(0.25),
        proximity: None,
        workspace: &[],
        self_collision_margin_m: None,
    };
    let limits = resolve_limits(&ResolveLimitsParams {
        tier: Tier::Host,
        envelope: Some(envelope),
        transport: Some(transport),
        profile: Some(profile),
        session: None,
        available: Available {
            imu: true,
            people_detection: true,
            legged: true,
            proximity_reading: true,
        },
        frames: None,
    });
    let cycle = CommandCycle {
        now_ms: vector.now_ms,
        command_ms: vector.command_ms,
        sequence: vector.sequence,
        desired: vector.desired,
        previous: None,
        capability: None,
        nearest_person_m: None,
    };
    // Empty bindings, as the corpus runner passes for these cases: no `joints` in their input,
    // no pose to evaluate, and the verdicts say `evaluated: false` to prove it.
    let bindings: &[skynet_kernel::kinematics::JointBinding] = &[];
    let mut scratch = skynet_kernel::kinematics::Kinematics::new();
    let verdict = evaluate_cycle_with_body(
        &cycle,
        &limits,
        Some(&envelope),
        None,
        Some(Body {
            joints: bindings,
            scratch: &mut scratch,
        }),
    );
    // Canonical form, which is what §31.4's byte-identity compares. A verdict that will not
    // serialize — non-finite somewhere it must not be — can never equal a certified one, and
    // arrives here as a mismatch rather than a panic.
    skynet_contracts::safety::verdict_to_json(&verdict)
        .to_canonical()
        .unwrap_or_else(|_| "a verdict that does not serialize".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::Fake;

    fn verdict_of<'a>(report: &'a Report, name: &str) -> &'a CheckVerdict {
        report
            .checks
            .iter()
            .find(|check| check.name == name)
            .unwrap_or_else(|| panic!("no check named {name}"))
    }

    #[test]
    fn a_default_fake_passes_all_five_and_keeps_its_session() {
        let mut body = Fake::new(3);
        let report = run_post(&mut body);
        assert_eq!(report.checks.len(), 5);
        for check in &report.checks {
            assert!(check.passed, "{}: {}", check.name, check.detail);
        }
        assert!(passed(&report));
        assert_eq!(report.action, SessionAction::Keep);
        assert_eq!(body.stops, 1, "the stop was asserted");
        assert_eq!(body.releases, 1, "and released in place");
        assert!(!body.latched, "the session is commandable again");
    }

    #[test]
    fn every_detail_fits_the_event_channel() {
        // The evidence crosses on §13's ring, whose detail field is 96 bytes. `put_text` would
        // truncate past that silently; the verdicts stay terse by construction instead, and this
        // is what holds them to it.
        let mut body = Fake::new(3);
        let report = run_post(&mut body);
        for check in &report.checks {
            assert!(
                check.detail.len() <= skynet_blackbox::record::EVENT_DETAIL,
                "{}: {} bytes: {}",
                check.name,
                check.detail.len(),
                check.detail,
            );
        }
    }

    #[test]
    fn a_foreign_abi_fails_the_first_check() {
        let mut body = Fake::new(3);
        body.abi_version = 1;
        let report = run_post(&mut body);
        let abi = verdict_of(&report, "rhal_abi");
        assert!(!abi.passed);
        assert!(!passed(&report));
    }

    #[test]
    fn a_descriptor_that_disagrees_with_the_probe_fails() {
        let mut body = Fake::new(3);
        body.descriptor = Some(
            r#"{"schema_version":1,"joints":[{"name":"j0"}],"control":{"modes":["position"]}}"#
                .to_string(),
        );
        let report = run_post(&mut body);
        let describe = verdict_of(&report, "describe_matches_descriptor");
        assert!(!describe.passed, "{}", describe.detail);
        assert!(describe.detail.contains("1 joints but the probe read 3"));
    }

    #[test]
    fn a_body_with_no_stop_fails_the_estop_check() {
        let mut body = Fake::new(3);
        body.stop_available = false;
        let report = run_post(&mut body);
        let estop = verdict_of(&report, "estop_asserted_and_observed");
        assert!(!estop.passed);
        assert_eq!(body.stops, 0, "nothing was asserted");
    }

    #[test]
    fn no_release_renews_the_session_and_still_passes() {
        let mut body = Fake::new(3);
        body.release_available = false;
        let report = run_post(&mut body);
        let estop = verdict_of(&report, "estop_asserted_and_observed");
        assert!(estop.passed, "{}", estop.detail);
        assert_eq!(report.action, SessionAction::Renew);
        assert!(body.latched, "the old session stays stopped");
    }

    #[test]
    fn a_failed_release_fails_the_estop_check() {
        let mut body = Fake::new(3);
        body.release_error = Some("the bus never answered".to_string());
        let report = run_post(&mut body);
        let estop = verdict_of(&report, "estop_asserted_and_observed");
        assert!(!estop.passed, "{}", estop.detail);
        assert_eq!(report.action, SessionAction::Keep);
    }

    #[test]
    fn a_flag_that_never_clears_fails_without_a_rated_stop() {
        let mut body = Fake::new(3);
        // The latch clears but the flag does not follow it: hardware holding the chain on a body
        // that declares no rated stop.
        body.flags = skynet_rhal::flags::ESTOP;
        let report = run_post(&mut body);
        let estop = verdict_of(&report, "estop_asserted_and_observed");
        assert!(!estop.passed, "{}", estop.detail);
    }

    #[test]
    fn a_flag_that_never_clears_passes_on_a_rated_stop() {
        let mut body = Fake::new(3);
        body.flags = skynet_rhal::flags::ESTOP;
        body.descriptor = Some(
            r#"{"schema_version":1,"joints":[{"name":"j0"},{"name":"j1"},{"name":"j2"}],"control":{"modes":["position"]},"envelope":{"estop":true}}"#
                .to_string(),
        );
        let report = run_post(&mut body);
        let estop = verdict_of(&report, "estop_asserted_and_observed");
        assert!(estop.passed, "{}", estop.detail);
        assert!(report.rated_stop);
    }

    #[test]
    fn a_body_with_no_modes_skips_the_hold() {
        let mut body = Fake::new(3);
        body.descriptor = Some(
            r#"{"schema_version":1,"joints":[{"name":"j0"},{"name":"j1"},{"name":"j2"}],"control":{"modes":[]}}"#
                .to_string(),
        );
        let report = run_post(&mut body);
        assert!(!report.commandable);
        let estop = verdict_of(&report, "estop_asserted_and_observed");
        assert!(estop.passed, "{}", estop.detail);
        assert!(
            estop.detail.contains("no joint commands"),
            "the skip is said aloud: {}",
            estop.detail,
        );
    }

    #[test]
    fn a_dead_encoder_fails_sanity_but_nan_velocity_passes() {
        let mut body = Fake::new(3);
        body.velocity[1] = f32::NAN;
        let report = run_post(&mut body);
        assert!(
            verdict_of(&report, "encoder_sanity").passed,
            "an unmeasured velocity is §24, not a dead encoder"
        );
        body.position[1] = f32::NAN;
        let report = run_post(&mut body);
        let encoder = verdict_of(&report, "encoder_sanity");
        assert!(!encoder.passed, "{}", encoder.detail);
    }

    /// The sample is held to the corpus it was drawn from, four ways: the hand-built vectors
    /// evaluate to the embedded verdicts, the embedded verdicts equal the cases' `expected`, and
    /// the cases' own JSON — built through the corpus runner's builders — evaluates to the same.
    /// A corpus regeneration that moves a verdict fails here until the sample is re-derived.
    #[test]
    fn selfcheck_vectors_match_the_corpus() {
        use skynet_corpus::{CycleOwned, LimitsOwned};
        let root = skynet_corpus::default_root();
        assert!(root.is_dir(), "no corpus at {}", root.display());
        for vector in SELFCHECK_VECTORS {
            let (file, case_id) = vector
                .case
                .split_once(':')
                .expect("every vector names file:case");
            let text = std::fs::read_to_string(root.join(file)).expect("the corpus file reads");
            let document = skynet_json::Json::parse(&text).expect("the corpus file parses");
            let case = document
                .arr_or_empty("cases")
                .iter()
                .find(|case| case.get("id").and_then(skynet_json::Json::as_str) == Some(case_id))
                .unwrap_or_else(|| panic!("{} names no case {case_id}", vector.case));
            let expected = case
                .get("expected")
                .and_then(skynet_json::Json::as_str)
                .expect("every case carries expected");
            assert_eq!(
                vector.expected, expected,
                "{}: the embedded verdict drifted from the corpus",
                vector.case,
            );
            let input = case.get("input").expect("every case carries input");
            let owned = LimitsOwned::from_json(input.get("limits").expect("limits"))
                .expect("the case limits build");
            let cycle_owned =
                CycleOwned::from_json(input.get("cycle").expect("cycle")).expect("cycle builds");
            let last_sequence = input
                .get("last_sequence")
                .and_then(skynet_json::Json::as_f64);
            let per_channel = owned.per_channel_refs();
            let frames = owned.frame_refs();
            let volumes = owned.volume_refs();
            let layers = owned.layer_refs(&volumes);
            let joint_modes = owned.joint_mode_refs();
            let params = owned.params(&per_channel, &joint_modes, frames.as_deref(), &layers);
            let grants = owned.grant_refs();
            let limits = skynet_kernel::resolve_limits(&params).with_grants(&grants);
            let (desired, previous) = cycle_owned.refs();
            let cycle = cycle_owned.cycle(&desired, previous.as_deref());
            let envelope = owned.envelope();
            let bindings = skynet_corpus::BindingsOwned::from_json(input.get("joints"));
            let bindings = bindings.refs();
            let mut scratch = skynet_kernel::kinematics::Kinematics::new();
            let verdict = skynet_kernel::evaluate_cycle_with_body(
                &cycle,
                &limits,
                envelope.as_ref(),
                last_sequence.map(|n| n as u64),
                Some(skynet_kernel::Body {
                    joints: &bindings,
                    scratch: &mut scratch,
                }),
            );
            let rebuilt = skynet_contracts::safety::verdict_to_json(&verdict)
                .to_canonical()
                .expect("the verdict serializes");
            assert_eq!(
                rebuilt, expected,
                "{}: the corpus runners' builders disagree with the case",
                vector.case,
            );
            assert_eq!(
                evaluate_vector(vector),
                expected,
                "{}: the hand-built sample disagrees with the corpus",
                vector.case,
            );
        }
    }
}

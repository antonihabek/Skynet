//! §88 — what admits a motion request on a program-level body.
//!
//! The Rust edition of `reference/typescript/execute.ts`. Read that file for the design; this one
//! records the port.
//!
//! §54.1 ended with two sentences that stood from the moment Part VI was written: *"`execute({
//! skill, arguments, timeout_s })` already existed on the Hub side as the invocation point. Nothing
//! routed it."* For a joint-commanded body that stopped being true once §54.4's lifecycle was
//! driven. For §59.2's machines it was still true, and Phase 4 named two blockers; §84 removed the
//! first by giving a motion a place in the descriptor.
//!
//! # The second blocker dissolved rather than being bridged
//!
//! As written: §33.1 keeps `skynetd` off the path that holds a driver, so a routed skill's
//! destination is `skynetk`'s and **there is no channel from the supervisor to it**. Both obvious
//! remedies spend §33.1's licence — a socket from the supervisor carrying motions puts it on the
//! actuator path, and moving the driver into the supervisor deletes the section outright.
//!
//! §88.1's answer is that the route already exists. A skill over a program-level body is a policy
//! process emitting motion requests into the same ring it would post joint commands into;
//! `skynetk` reads them and calls the driver's `extension()`; and the supervisor admits the
//! invocation, evaluates §54.2's preconditions, takes §25.2's leases and starts a unit — every one
//! of which it already does — and never sees a motion. What was missing was not a channel but the
//! **admission rule** for a request arriving on one.
//!
//! # It is a gate, and calling it a guard would be a claim
//!
//! §7's guards clamp a *value* against a resolved ceiling. There is no ceiling for `move_l` and no
//! clamp that produces a safer program, because the controller owns the servo loop and its safety
//! case belongs to somebody else. What is checkable is **authority, state, and the body's own
//! declared table** — and the last is the half §84.1 created and nothing consumed. [`admit_motion`]
//! is not a ninth guard and this module does not pretend it is.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::embodiment::EmbodimentMotion;

/// §27's states in which a body will take a command.
///
/// `running` and `degraded`, and the second is not a softening: §27 puts `DEGRADED` on a body that
/// is *still there and still answering*, and §36.3's whole ladder is about a robot that keeps
/// working while giving things up. A gate that refused a motion in `stopping` would be queueing
/// work behind a controlled stop.
pub const MOTION_READY_STATES: [&str; 2] = ["running", "degraded"];

/// Why a motion request was refused, in the order [`admit_motion`] evaluates them.
///
/// Declaration order **is** precedence, which is the convention §84.1's fault list already uses,
/// and it is worth the sentence because the first four are about the robot and the last six are
/// about the request: a caller reading a refusal from the first group has nothing to fix in its own
/// bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionRefusalCode {
    /// The request's holder is not who holds the body (§25.2).
    NoLease,
    /// The emergency-stop chain is open (§24).
    Estop,
    /// §27's state is not one in which this body runs a program.
    NotActive,
    /// The controller is executing a motion.
    Busy,
    /// The body declares no motion of this name (§84.1).
    UnknownMotion,
    /// The request carries no completion deadline (§88.3).
    DeadlineUndeclared,
    /// The request carries a parameter the motion does not take.
    ParameterUnknown,
    /// A parameter the motion requires and the request omits.
    ParameterMissing,
    /// A supplied value that is not a finite number.
    ParameterNotFinite,
    /// A supplied value outside a bound the motion **declares**.
    ParameterOutOfBounds,
}

impl MotionRefusalCode {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            MotionRefusalCode::NoLease => "no_lease",
            MotionRefusalCode::Estop => "estop",
            MotionRefusalCode::NotActive => "not_active",
            MotionRefusalCode::Busy => "busy",
            MotionRefusalCode::UnknownMotion => "unknown_motion",
            MotionRefusalCode::DeadlineUndeclared => "deadline_undeclared",
            MotionRefusalCode::ParameterUnknown => "parameter_unknown",
            MotionRefusalCode::ParameterMissing => "parameter_missing",
            MotionRefusalCode::ParameterNotFinite => "parameter_not_finite",
            MotionRefusalCode::ParameterOutOfBounds => "parameter_out_of_bounds",
        }
    }
}

/// Every refusal, in evaluation order.
pub const MOTION_REFUSALS: [MotionRefusalCode; 10] = [
    MotionRefusalCode::NoLease,
    MotionRefusalCode::Estop,
    MotionRefusalCode::NotActive,
    MotionRefusalCode::Busy,
    MotionRefusalCode::UnknownMotion,
    MotionRefusalCode::DeadlineUndeclared,
    MotionRefusalCode::ParameterUnknown,
    MotionRefusalCode::ParameterMissing,
    MotionRefusalCode::ParameterNotFinite,
    MotionRefusalCode::ParameterOutOfBounds,
];

/// One request, as a policy emits it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MotionRequest {
    /// The **body's** own spelling, resolved by the transport plan at install (§84.3).
    pub motion: String,
    /// By name, in the document's own key order, because a controller names its arguments.
    pub parameters: Vec<(String, Option<f64>)>,
    /// How long the controller may take, milliseconds (§88.3). Required.
    pub deadline_ms: Option<f64>,
    /// Who is asking. Compared against the lease, which §25.2 holds by holder string.
    pub holder: Option<String>,
}

/// What the process holding the driver knows when a request arrives.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MotionGate {
    /// The body's declared motions (§84.1), in declaration order.
    pub motions: Vec<EmbodimentMotion>,
    /// §27's operating state.
    pub state: String,
    /// §24's estop bit: the hardware chain is open.
    pub estop: bool,
    /// Who holds the body (§25.2), or `None` when nobody does.
    pub lease_held_by: Option<String>,
    /// The motion the controller is executing, when one is in flight.
    pub executing: Option<String>,
}

/// What [`admit_motion`] concluded.
#[derive(Debug, Clone, PartialEq)]
pub struct MotionVerdict {
    /// Whether the driver is handed this request.
    pub accepted: bool,
    /// Why not, or `None`.
    pub refusal: Option<MotionRefusalCode>,
    /// The sentence.
    pub detail: String,
    /// The argument list the driver is handed, in the motion's **declaration order** (§88.2).
    ///
    /// A program-level controller's motion is a call with positional arguments and the declaration
    /// order is the only order anybody agreed on. A parameter the motion declares, does not
    /// require, and the request omits is **absent rather than zero** — the controller owns its
    /// defaults, and substituting one here would be inventing a command. Empty on a refusal.
    pub arguments: Vec<(String, f64)>,
    /// Parameters whose value nothing checked, because the motion declares no bound (§5.2).
    pub unbounded: Vec<String>,
}

fn refuse(refusal: MotionRefusalCode, detail: String) -> MotionVerdict {
    MotionVerdict {
        accepted: false,
        refusal: Some(refusal),
        detail,
        arguments: Vec::new(),
        unbounded: Vec::new(),
    }
}

/// Whether this request may be handed to the driver, and with what.
///
/// Authority before state, state before content. A request from a holder that does not hold the
/// body is refused **without the platform reading what it asked for**, because reading it would
/// mean answering a question nobody was entitled to ask.
///
/// **`parameter_unknown` is a refusal here and a warning at plan time, and the asymmetry is the
/// point.** §84.3 raises `motion_parameter_unused` on a transport plan and does not refuse, because
/// the controller does take the motion. At command time the same fact is a value about to be handed
/// to a controller that will either interpret it or reject it on a moving robot, and §25.2's
/// argument applies unchanged: commanding a body with the parts of a command it happened to
/// understand is worse than refusing.
pub fn admit_motion(gate: &MotionGate, request: &MotionRequest) -> MotionVerdict {
    let held = gate.lease_held_by.as_deref();
    let holder = request.holder.as_deref();

    match (held, holder) {
        (Some(held), Some(holder)) if held == holder => {}
        (None, _) => {
            return refuse(
                MotionRefusalCode::NoLease,
                "Nothing holds this body, so there is nobody whose motion this would be (§25.2)."
                    .to_string(),
            );
        }
        (Some(held), holder) => {
            return refuse(
                MotionRefusalCode::NoLease,
                format!(
                    "This body is held by `{held}` and the request came from `{}` (§25.2).",
                    holder.unwrap_or("nobody")
                ),
            );
        }
    }

    if gate.estop {
        return refuse(
            MotionRefusalCode::Estop,
            "The emergency-stop chain is open (§24). What opened it is not something the body can \
             say, so the only correct response is to command nothing."
                .to_string(),
        );
    }

    if !MOTION_READY_STATES.contains(&gate.state.as_str()) {
        return refuse(
            MotionRefusalCode::NotActive,
            format!(
                "§27 has this body in `{}`, which is not a state in which it will run a program. \
                 Accepting the request would be queueing work behind whatever the body is doing \
                 instead — a controlled stop, a self-test, or waiting to be homed.",
                gate.state
            ),
        );
    }

    if let Some(executing) = gate.executing.as_deref().filter(|name| !name.is_empty()) {
        return refuse(
            MotionRefusalCode::Busy,
            format!(
                "The controller is executing `{executing}`. A program-level controller owns the \
                 servo loop (§59.2), so it finishes or it is aborted; there is no second motion to \
                 interleave with the first."
            ),
        );
    }

    let Some(motion) = gate
        .motions
        .iter()
        .find(|candidate| candidate.name == request.motion)
    else {
        return refuse(
            MotionRefusalCode::UnknownMotion,
            format!(
                "This body declares no motion called `{}`. The name a request carries is the \
                 **body's** spelling, which §84.3's plan resolved at install — a policy's own name \
                 reaching here unresolved is the defect this refusal usually means.",
                request.motion
            ),
        );
    };

    if !request
        .deadline_ms
        .is_some_and(|value| value.is_finite() && value > 0.0)
    {
        return refuse(
            MotionRefusalCode::DeadlineUndeclared,
            "A motion takes seconds, during which §7.1's watchdog guard has nothing to watch: a \
             correct policy produces no command at all. The deadline is what replaces it (§88.3), \
             so a request without one is a program nothing would ever conclude had failed."
                .to_string(),
        );
    }

    // First wins, as §84.1's reader does with a repeated name: `duplicate_parameter` is the
    // descriptor's fault and `embodiment.descriptor.valid` is where it is reported. A gate that
    // preferred the later one would command against a bound the check never looked at.
    let mut declared: Vec<&crate::embodiment::MotionParameter> = Vec::new();
    for parameter in &motion.parameters {
        if !declared.iter().any(|known| known.name == parameter.name) {
            declared.push(parameter);
        }
    }

    // The request's own names first: a parameter the motion has no slot for means the policy and
    // the body disagree about the motion's *shape*, which subsumes arguing about its values.
    let mut supplied: Vec<&String> = request.parameters.iter().map(|(name, _)| name).collect();
    supplied.sort();
    for name in supplied {
        if declared.iter().any(|known| &known.name == name) {
            continue;
        }
        return refuse(
            MotionRefusalCode::ParameterUnknown,
            format!(
                "`{}` does not take `{name}`. §84.3 warned about this at install — \
                 `motion_parameter_unused` — and declined to refuse a plan over it, because the \
                 controller does take the motion. Here it is a value about to be sent to a \
                 controller that has no slot for it, and §25.2 is clear that commanding with the \
                 parts of a command a machine happened to understand is worse than refusing.",
                motion.name
            ),
        );
    }

    let mut arguments: Vec<(String, f64)> = Vec::new();
    let mut unbounded: Vec<String> = Vec::new();

    for parameter in declared {
        let supplied = request
            .parameters
            .iter()
            .find(|(name, _)| *name == parameter.name);
        let Some((_, value)) = supplied else {
            // §84.1's fail-closed reading: anything but an explicit `false` is required.
            if parameter.required != Some(false) {
                return refuse(
                    MotionRefusalCode::ParameterMissing,
                    format!(
                        "`{}` requires `{}` and the request does not carry it. §84.1 reads a \
                         requirement fail-closed, so a parameter whose `required` the descriptor \
                         never declared is required here too.",
                        motion.name, parameter.name
                    ),
                );
            }
            // Absent rather than zero. §7.2's rule, and the controller owns its own defaults.
            continue;
        };

        let Some(value) = value.filter(|value| value.is_finite()) else {
            return refuse(
                MotionRefusalCode::ParameterNotFinite,
                format!(
                    "`{}` is not a finite number. A controller handed one would either fault or \
                     move somewhere nobody chose.",
                    parameter.name
                ),
            );
        };

        let min = parameter.min.filter(|value| value.is_finite());
        let max = parameter.max.filter(|value| value.is_finite());
        if let Some(min) = min {
            if value < min {
                return refuse(
                    MotionRefusalCode::ParameterOutOfBounds,
                    format!(
                        "`{}` is {} and `{}` declares a minimum of {}.",
                        parameter.name,
                        crate::json::number_text(value),
                        motion.name,
                        crate::json::number_text(min)
                    ),
                );
            }
        }
        if let Some(max) = max {
            if value > max {
                return refuse(
                    MotionRefusalCode::ParameterOutOfBounds,
                    format!(
                        "`{}` is {} and `{}` declares a maximum of {}.",
                        parameter.name,
                        crate::json::number_text(value),
                        motion.name,
                        crate::json::number_text(max)
                    ),
                );
            }
        }
        if min.is_none() && max.is_none() {
            unbounded.push(parameter.name.clone());
        }

        arguments.push((parameter.name.clone(), value));
    }

    let detail = if unbounded.is_empty() {
        format!(
            "`{}` accepted, with every supplied parameter inside a bound this body declared.",
            motion.name
        )
    } else {
        format!(
            "`{}` accepted. {} not checked against anything, because this body declares no bound \
             for {} — which §5.2 reads as *nobody declared one*, never as unbounded.",
            motion.name,
            if unbounded.len() == 1 {
                "One parameter was"
            } else {
                "Some parameters were"
            },
            unbounded.join(", ")
        )
    };

    MotionVerdict {
        accepted: true,
        refusal: None,
        detail,
        arguments,
        unbounded,
    }
}

/// Whether a motion in flight is still inside its deadline (§88.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MotionProgress {
    /// Inside its deadline.
    Running,
    /// Past it, with no completion from the driver.
    Overdue,
}

impl MotionProgress {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            MotionProgress::Running => "running",
            MotionProgress::Overdue => "overdue",
        }
    }
}

/// Both, in the reference's declaration order.
pub const MOTION_PROGRESS: [MotionProgress; 2] = [MotionProgress::Running, MotionProgress::Overdue];

/// What one cycle concluded about a motion already in flight.
#[derive(Debug, Clone, PartialEq)]
pub struct MotionProgressReport {
    /// Running or overdue.
    pub progress: MotionProgress,
    /// Milliseconds past the deadline. Negative is remaining.
    pub overdue_by_ms: f64,
    /// The sentence.
    pub detail: String,
}

/// Whether a motion in flight has run past its deadline (§88.3).
///
/// Separate from [`admit_motion`] because one is asked **once per request** and this is asked
/// **every cycle** until the controller answers. Folding them together would produce a function
/// whose expensive half ran at the control rate, and — worse — one that could not be read without
/// knowing which of its two jobs a caller wanted.
pub fn motion_progress(deadline_ms: f64, started_at_ms: f64, now_ms: f64) -> MotionProgressReport {
    let elapsed = now_ms - started_at_ms;
    let over = elapsed - deadline_ms;
    // `>` rather than `>=`, as §87.4's lease is: the instant the deadline falls is still inside it,
    // and a controller that answers exactly on time has answered on time.
    if over > 0.0 {
        return MotionProgressReport {
            progress: MotionProgress::Overdue,
            overdue_by_ms: over,
            detail: format!(
                "The controller has held this motion for {} ms against a {} ms deadline. §7.1's \
                 watchdog has nothing to watch on this path (§88.3), so this is the bound that \
                 fires: abort the motion, enter §27's fault path, and record it.",
                crate::json::number_text(elapsed),
                crate::json::number_text(deadline_ms)
            ),
        };
    }
    MotionProgressReport {
        progress: MotionProgress::Running,
        overdue_by_ms: over,
        detail: format!(
            "Inside its deadline, with {} ms left.",
            crate::json::number_text(-over)
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embodiment::MotionParameter;
    use alloc::vec;

    fn parameter(
        name: &str,
        required: bool,
        min: Option<f64>,
        max: Option<f64>,
    ) -> MotionParameter {
        MotionParameter {
            name: name.to_string(),
            unit: None,
            required: Some(required),
            min,
            max,
        }
    }

    fn gate() -> MotionGate {
        MotionGate {
            motions: vec![EmbodimentMotion {
                name: "move_l".to_string(),
                motion: Some("move_linear".to_string()),
                parameters: vec![
                    parameter("pose", true, Some(-1.5), Some(1.5)),
                    parameter("speed", false, Some(0.01), Some(0.25)),
                ],
            }],
            state: "running".to_string(),
            estop: false,
            lease_held_by: Some("acme/palletise".to_string()),
            executing: None,
        }
    }

    fn request() -> MotionRequest {
        MotionRequest {
            motion: "move_l".to_string(),
            parameters: vec![
                ("pose".to_string(), Some(0.4)),
                ("speed".to_string(), Some(0.1)),
            ],
            deadline_ms: Some(5_000.0),
            holder: Some("acme/palletise".to_string()),
        }
    }

    /// Authority before state, and the refusal deliberately does not say whether the motion exists:
    /// reading the request would mean answering a question nobody was entitled to ask.
    #[test]
    fn a_request_from_the_wrong_holder_is_not_read() {
        let verdict = admit_motion(
            &gate(),
            &MotionRequest {
                motion: "not_a_motion".to_string(),
                holder: Some("acme/other".to_string()),
                ..request()
            },
        );
        assert_eq!(verdict.refusal, Some(MotionRefusalCode::NoLease));
    }

    /// The argument list is the motion's declaration order, and an omitted optional parameter is
    /// absent rather than zero.
    #[test]
    fn an_omitted_optional_parameter_is_absent() {
        let verdict = admit_motion(
            &gate(),
            &MotionRequest {
                parameters: vec![("pose".to_string(), Some(0.4))],
                ..request()
            },
        );
        assert!(verdict.accepted);
        assert_eq!(verdict.arguments, vec![("pose".to_string(), 0.4)]);
    }

    /// §5.2 on the command path: an undeclared bound does not refuse, and the verdict names the
    /// parameters that went unchecked rather than reporting a check it did not perform.
    #[test]
    fn an_undeclared_bound_is_named_rather_than_assumed() {
        let mut gate = gate();
        gate.motions[0].parameters[0] = parameter("pose", true, None, None);
        let verdict = admit_motion(&gate, &request());
        assert!(verdict.accepted);
        assert_eq!(verdict.unbounded, vec!["pose".to_string()]);
    }

    /// The instant the deadline falls is still inside it.
    #[test]
    fn a_controller_answering_exactly_on_time_answered_on_time() {
        assert_eq!(
            motion_progress(5_000.0, 1_000.0, 6_000.0).progress,
            MotionProgress::Running
        );
        assert_eq!(
            motion_progress(5_000.0, 1_000.0, 6_001.0).progress,
            MotionProgress::Overdue
        );
    }
}

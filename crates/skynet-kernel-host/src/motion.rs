//! §88's motion path: what this process does with a request that is not a joint command.
//!
//! §54.1 ended with two sentences that stood since Part VI was written —
//! `StandardRobotInterface.execute({ skill, arguments, timeout_s })` existed on the Hub side as the
//! invocation point, and **nothing routed it**. For a joint-commanded body that stopped being true
//! when §54.4's lifecycle was driven. For §59.2's machines — the program-level installed
//! base — it was still true until §88, and this module is the half of §88 that runs.
//!
//! # What was missing was never a channel
//!
//! [`crate::run`] already reads a ring a policy writes, and §84.2 says a policy whose action space
//! is motions is a policy. So a skill over a program-level body posts **motion requests** into that
//! same ring, this process reads them, and the driver's `extension()` is where they land.
//! `skynetd` never sees one, which is §33.1 working rather than being worked around.
//!
//! What was actually missing is the **admission rule**. A joint command is admitted by §7.1's eight
//! guards; a motion is not a joint command (§84.4), so nothing had decided what this process should
//! do with one. [`skynet_contracts::execute::admit_motion`] is that decision and this module is its
//! caller: it assembles the gate out of what the loop knows, hands the verdict's argument list to
//! §4's escape hatch in the order §84.1 declared, and keeps asking
//! [`skynet_contracts::execute::motion_progress`] until the controller answers or the deadline
//! passes.
//!
//! # Why the gate's authority half is supplied rather than observed
//!
//! Three of [`MotionGate`]'s five fields are facts this process does not own. The body's declared
//! motions are the descriptor's, §27's operating state is the supervisor's, and who holds the body
//! is §25.2's lease table — which lives in `skynetd`, on the far side of the line §33.1 draws. So
//! [`Authority`] is **given** to the loop by whoever started it, and its default refuses
//! everything: no lease, no state, no motions. That is the fail-closed reading and it is the only
//! honest one — a loop that answered *"whoever holds it"* to §88.2's first question would admit a
//! motion from any process that could reach the ring.
//!
//! The other two are the loop's own and are observed rather than declared: §24's estop bit comes
//! off the sample this cycle read, and `executing` is this module's own record of the motion it
//! last handed over.

use std::ffi::CString;

use skynet_contracts::embodiment::EmbodimentMotion;
use skynet_contracts::execute::{
    admit_motion, motion_progress, MotionGate, MotionProgress, MotionRequest, MotionVerdict,
};
use skynet_ring::Motion as MotionFrame;

use crate::driver::Actuator;

/// What this process is told about authority, because it cannot observe it (§25.2, §27, §84.1).
///
/// The default refuses every motion, and that is the whole design of this type: a robot whose
/// supervisor has not spoken is a robot nothing holds a lease on, and §88.2's first refusal is the
/// right answer to a motion arriving on one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Authority {
    /// The body's declared motions (§84.1), in declaration order — which is the order §88.2 hands
    /// the arguments over in.
    pub motions: Vec<EmbodimentMotion>,
    /// §27's operating state, as the supervisor reports it. Empty is *nobody said*, and every
    /// motion is then refused `not_active`.
    pub state: String,
    /// Who holds the body (§25.2). `None` refuses every motion `no_lease`.
    pub lease_held_by: Option<String>,
    /// §4's capability token this body's controller takes its motions on.
    ///
    /// The token is the **vendor's**, listed under `extensions` in the descriptor, and it is
    /// carried here rather than guessed because a body may declare several and picking one would
    /// be inventing a rule §88 does not have. `None` is a body with no motion route, and a request
    /// to it is delivered nowhere and recorded as undelivered rather than as refused — those are
    /// different facts and an operator needs the second one.
    pub token: Option<String>,
}

/// What one cycle did about motions.
///
/// `None` on a cycle with no request in flight and none arriving, which is every cycle on a
/// joint-commanded body.
#[derive(Debug, Clone, PartialEq)]
pub struct MotionTurn {
    /// The body's own spelling of the motion this turn is about.
    pub motion: String,
    /// The request's sequence, which is the ring's.
    pub seq: u64,
    /// What the gate concluded, on the cycle the request arrived. `None` on a later cycle, which
    /// is a turn about a motion already in flight.
    pub verdict: Option<MotionVerdict>,
    /// The bytes handed to §4's escape hatch, when the request was admitted **and** delivered.
    pub delivered: Option<String>,
    /// The controller's own code and sentence, when it was asked.
    pub reply: Option<(i32, String)>,
    /// Whether this cycle found the motion past its deadline (§88.3).
    pub overdue: bool,
    /// One sentence about what happened, for the record and for an operator.
    pub detail: String,
}

/// The one motion this process has handed the controller and not yet seen finish.
#[derive(Debug, Clone, PartialEq)]
struct InFlight {
    motion: String,
    seq: u64,
    deadline_ms: f64,
    started_at_ms: f64,
}

/// §88's half of the loop's memory: the authority it was given and the request in flight.
#[derive(Debug, Clone, Default)]
pub struct Motions {
    authority: Authority,
    in_flight: Option<InFlight>,
    /// The last request's sequence, so a ring that publishes nothing new is not read as a repeat of
    /// the request it is still holding. §24's rule about a stalled producer is the watchdog's and
    /// does not apply here (§88.3) — what applies is that one request is executed once.
    last_seq: Option<u64>,
    /// Requests refused, which is the counter §88.3's completeness argument turns on: a body whose
    /// record held only the motions that ran could not tell *"the skill asked for nothing"* from
    /// *"the skill was refused ten times"*.
    pub refused: u64,
    /// Requests handed to the controller.
    pub delivered: u64,
    /// Requests the controller held past their deadline.
    pub overdue: u64,
}

impl Motions {
    /// Tell this process who holds the body, what state it is in, and what it can be asked to do.
    ///
    /// Called by whoever started the loop, as often as any of it changes. It is separate from the
    /// cycle because none of it is a per-cycle fact: a lease is taken once and a descriptor is read
    /// at open, and putting them in the step signature would suggest the loop could work them out.
    pub fn authorise(&mut self, authority: Authority) {
        self.authority = authority;
    }

    /// What this process has been told (§88.2's gate, minus the two halves it observes itself).
    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    /// Whether a motion is in flight, by name.
    pub fn executing(&self) -> Option<&str> {
        self.in_flight.as_ref().map(|flight| flight.motion.as_str())
    }

    /// One cycle of §88, given whatever the ring had.
    ///
    /// `fresh` is the request this cycle read, or `None` on a cycle where the ring carried a joint
    /// command, nothing, or a repeat of the request already in flight. Either way the motion in
    /// flight is asked whether it is still inside its deadline, because §88.3's bound is the one
    /// that replaces §7.1's watchdog on this path and it has to fire on a cycle where the policy
    /// said nothing at all.
    pub fn step<A: Actuator>(
        &mut self,
        now_ms: f64,
        estop: bool,
        body: &mut A,
        fresh: Option<(&MotionFrame, u64, &[f32])>,
    ) -> Option<MotionTurn> {
        if let Some((frame, seq, values)) = fresh {
            if self.last_seq != Some(seq) {
                self.last_seq = Some(seq);
                return Some(self.arrived(now_ms, estop, body, frame, seq, values));
            }
        }
        self.in_flight
            .clone()
            .map(|flight| self.progress(now_ms, body, &flight))
    }

    /// A request arrived. Admit it, and hand it over if it was admitted.
    fn arrived<A: Actuator>(
        &mut self,
        now_ms: f64,
        estop: bool,
        body: &mut A,
        frame: &MotionFrame,
        seq: u64,
        values: &[f32],
    ) -> MotionTurn {
        let request = read_request(frame, values);
        let gate = MotionGate {
            motions: self.authority.motions.clone(),
            state: self.authority.state.clone(),
            estop,
            lease_held_by: self.authority.lease_held_by.clone(),
            executing: self.executing().map(str::to_string),
        };
        let verdict = admit_motion(&gate, &request);
        let motion = request.motion.clone();

        if !verdict.accepted {
            self.refused += 1;
            let detail = verdict.detail.clone();
            return MotionTurn {
                motion,
                seq,
                verdict: Some(verdict),
                delivered: None,
                reply: None,
                overdue: false,
                detail,
            };
        }

        // Admitted, and now it has to reach a controller. A body with no token declared, or one
        // whose driver has no `extension` at all, is not a refusal — the request was sound and this
        // platform had nowhere to put it, which is a different thing to record and a different
        // thing to fix.
        let Some(token) = self.authority.token.clone() else {
            let detail = String::from(
                "This motion was admitted and nothing carried it: no capability token is declared \
                 for this body's controller, so there is no route to §4's escape hatch (§88.1). \
                 Declare the token the descriptor lists under `extensions`.",
            );
            return MotionTurn {
                motion,
                seq,
                verdict: Some(verdict),
                delivered: None,
                reply: None,
                overdue: false,
                detail,
            };
        };
        if !body.has_extension() {
            let detail = String::from(
                "This motion was admitted and nothing carried it: this driver implements no \
                 `extension`, so a body declaring motions has no route to its controller (§4, \
                 §88.1).",
            );
            return MotionTurn {
                motion,
                seq,
                verdict: Some(verdict),
                delivered: None,
                reply: None,
                overdue: false,
                detail,
            };
        }

        let request_text = render(&motion, &verdict);
        // A token with a NUL in it cannot cross the ABI, and it arrived from a declaration rather
        // than from the wire — so it is refused with a sentence rather than trimmed to whatever
        // came before the NUL, which would call a different extension.
        let Ok(token_c) = CString::new(token.clone()) else {
            let detail = format!(
                "This motion was admitted and nothing carried it: the declared capability token \
                 `{token}` contains a NUL and cannot cross §4's ABI."
            );
            return MotionTurn {
                motion,
                seq,
                verdict: Some(verdict),
                delivered: None,
                reply: None,
                overdue: false,
                detail,
            };
        };

        let reply = body.extension(&token_c, request_text.as_bytes());
        self.delivered += 1;
        let detail = if reply.accepted() {
            self.in_flight = Some(InFlight {
                motion: motion.clone(),
                seq,
                deadline_ms: request.deadline_ms.unwrap_or_default(),
                started_at_ms: now_ms,
            });
            format!("The controller took `{motion}`: {}", reply.detail)
        } else {
            // The controller refused it. §88.2's ten refusals are this platform's and this is not
            // one of them — it is the vendor's own status word, which is exactly what §4 is for and
            // exactly what an integrator spends an afternoon turning back into a sentence.
            format!(
                "The controller refused `{motion}` with {}: {}",
                reply.code, reply.detail
            )
        };
        MotionTurn {
            motion,
            seq,
            verdict: Some(verdict),
            delivered: Some(request_text),
            reply: Some((reply.code, reply.detail)),
            overdue: false,
            detail,
        }
    }

    /// A motion is in flight. Ask whether it is still inside its deadline (§88.3).
    fn progress<A: Actuator>(
        &mut self,
        now_ms: f64,
        body: &mut A,
        flight: &InFlight,
    ) -> MotionTurn {
        let report = motion_progress(flight.deadline_ms, flight.started_at_ms, now_ms);
        if report.progress == MotionProgress::Running {
            return MotionTurn {
                motion: flight.motion.clone(),
                seq: flight.seq,
                verdict: None,
                delivered: None,
                reply: None,
                overdue: false,
                detail: report.detail,
            };
        }

        // Past it. §88.3: the driver is asked to abort, §27's fault path is entered, and the black
        // box carries the request, the verdict and the outcome. The abort is this body's emergency
        // stop, because it is the only abort §4's ABI has — a `stop_motion` entry point does not
        // exist and inventing one here would be inventing an ABI.
        self.overdue += 1;
        self.in_flight = None;
        let aborted = if body.has_stop() {
            match body.stop() {
                Ok(()) => String::from("The body's emergency stop was triggered."),
                Err(error) => format!("The body's emergency stop failed: {error}"),
            }
        } else {
            String::from(
                "This body has no emergency stop, so nothing aborted the motion — the controller \
                 is still running a program nobody on this side can end.",
            )
        };
        MotionTurn {
            motion: flight.motion.clone(),
            seq: flight.seq,
            verdict: None,
            delivered: None,
            reply: None,
            overdue: true,
            detail: format!("{} {aborted}", report.detail),
        }
    }
}

/// The request as §88.2 reads it, out of the frame the ring carried.
///
/// A name the wire could not spell as UTF-8 becomes the empty string rather than being dropped, and
/// the empty string is a motion no body declares — so it arrives at `unknown_motion`, which is the
/// refusal an operator can act on. Silently skipping it would produce a request with one argument
/// missing, which §88.2 exists to refuse.
fn read_request(frame: &MotionFrame, values: &[f32]) -> MotionRequest {
    let mut parameters = Vec::new();
    for (index, value) in values.iter().enumerate() {
        let Some(name) = frame.parameters.get(index) else {
            break;
        };
        if name.is_empty() {
            continue;
        }
        parameters.push((
            name.as_str().unwrap_or_default().to_string(),
            Some(widen(*value)),
        ));
    }
    MotionRequest {
        motion: frame.name.as_str().unwrap_or_default().to_string(),
        parameters,
        // A zero or non-finite deadline is §88.2's sixth refusal rather than this function's, so it
        // is carried through as it arrived. `None` is what the wire cannot distinguish from zero,
        // and both refuse.
        deadline_ms: Some(f64::from(frame.deadline_ms)),
        holder: frame
            .holder
            .as_str()
            .filter(|text| !text.is_empty())
            .map(str::to_string),
    }
}

/// The `f64` a policy meant when it put an `f32` on the wire.
///
/// **Not `f64::from`**, and the difference is a refusal. §84.1's bounds are `f64` because the
/// descriptor is JSON; the ring is `f32` because it is a control wire. Widening exactly makes
/// `0.4f32` into `0.400000005960464…`, which is **above** a declared maximum of `0.4` — so a policy
/// asking for precisely the largest value its body declares is refused `parameter_out_of_bounds`,
/// for a reason that is in neither the descriptor nor the request. The same widening then renders
/// seventeen digits into a controller's request buffer, which on `drivers/pendant`'s 127 bytes is
/// four parameters away from being refused for its length.
///
/// So the value is read back through the shortest decimal that round-trips to that `f32`, which is
/// what `{}` produces for one and is the number the policy wrote. It allocates, once per request —
/// and a request is a thing that happens every few seconds on this path, not every cycle (§88.3).
fn widen(value: f32) -> f64 {
    format!("{value}").parse().unwrap_or(f64::from(value))
}

/// The bytes handed to §4's escape hatch: the motion, then its arguments in declaration order.
///
/// `<motion> <name>=<value>…`, and the format is the **vendor's** rather than this platform's —
/// `drivers/pendant` is the body §88 was written against and this is what its controller parses.
/// What this platform contributes is the ordering §88.2 decided and the ten refusals that ran
/// first; a parameter the motion declares, does not require and the request omitted is absent here
/// rather than zero, because the controller owns its defaults.
fn render(motion: &str, verdict: &MotionVerdict) -> String {
    let mut out = String::from(motion);
    for (name, value) in &verdict.arguments {
        out.push(' ');
        out.push_str(name);
        out.push('=');
        out.push_str(&skynet_contracts::json::number_text(*value));
    }
    out
}

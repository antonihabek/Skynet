//! §54.2's live precondition context, assembled.
//!
//! The Rust edition of `reference/typescript/live.ts`. Read that file for the design; this one
//! records the port.
//!
//! §54.2 turns `SkillManifest.preconditions` from prose into predicates and then names the seven
//! branches those predicates may read:
//!
//! ```text
//! robot.state.*        joint positions, gripper, battery percent, base pose
//! robot.guards.active  which guards are actually running (§7.2)
//! robot.calibration.*  which parameter sets are valid, and their age (§45)
//! robot.frames.*       whether the frames the skill needs resolve (§44)
//! robot.tier           guest | host | certified
//! skill.grants.*       capability tokens the operator granted this package
//! env.*                operator-declared facts about the cell — fixture present, door state
//! ```
//!
//! [`predicate::evaluate`](crate::predicate::evaluate) has existed since the sixteenth session and
//! [`skill::evaluate_preconditions`](crate::skill::evaluate_preconditions) since the seventeenth.
//! Both take the context as an argument, and until now nothing built one. That is the gap this
//! module closes, and it is a **contract** rather than daemon code for the reason every other file
//! here is one: the path a skill publisher writes into a predicate is a promise the OS makes about
//! what it will supply, and a promise assembled ad hoc inside a daemon is undocumented, untested,
//! and different in the next edition.
//!
//! # The rule that decides every field: absent is not null
//!
//! §54.2's first rule is the one with teeth:
//!
//! > **A precondition that cannot be evaluated refuses the skill**, naming the path that was
//! > unresolvable. Not "assume true", which is what free-text preconditions do by construction.
//!
//! So a fact this robot does not know must be **absent** from the context, never present as `null`
//! and never present as a plausible zero. `skynet-json` already draws that distinction — it is the
//! reason `Json::get_present` exists beside `Json::get` — and this is the module where it is
//! load-bearing rather than tidy. A robot with no battery sensor that reported
//! `robot.state.battery_percent = 0` would refuse every skill; one that reported `100` would run
//! every skill; one that omits the path refuses only the skills that asked, and names the path in
//! the refusal. Only the third is honest, and only the third is useful.
//!
//! # Two branches are lists rather than objects, and the reason is a dot
//!
//! [`cell`](crate::cell) found, from the other side, that a fact key containing a dot is a fact no
//! precondition can address: `{"door.closed": true}` and `{"door": {"closed": true}}` are the same
//! path and different documents. It answers by *refusing* such a key, because an operator can
//! rename a fact.
//!
//! A **capability token contains a dot by construction** — `arm.left`, `sensing.camera`,
//! `locomotion.base` — so `skill.grants.<token>` is unaddressable for every token that exists, and
//! nobody can rename them. The shape therefore has to change rather than the names: grants are a
//! **sorted list** at `skill.grants.tokens`, read with `includes` and `includes_all`, which the
//! predicate language has had since it was ported and which is exactly the operation a precondition
//! about grants wants anyway.
//!
//! Frames are a list for the second half of the same reason. §44 lets a frame id carry a namespace,
//! and §54.2's question about them is *"whether the frames the skill needs resolve"* — a
//! containment question, not a lookup. `robot.frames.resolved` with `includes_all` asks it in one
//! clause; an object keyed by frame id would ask it in one clause per frame and get the empty case
//! wrong.
//!
//! Joint names are *not* moved to a list, because the question about a joint is its **value** and a
//! list cannot answer that. They keep `robot.state.joints.<name>`, and a joint whose name a dot
//! path cannot address is omitted and **reported** — see [`LiveContext::unaddressable`]. That is
//! the one asymmetry in this module and it is the honest one: the OS cannot rename a body's joints,
//! so it says which ones no precondition will ever be able to name rather than pretending the
//! branch is complete.
//!
//! # Pure
//!
//! No clock, no I/O. Ages arrive already computed from a supplied wall-clock reading, for the
//! reason the kernel takes `now_ms` as a parameter (§35.2), and every other field arrives already
//! observed.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

use crate::calibration::Validity;
use crate::embodiment::FrameFault;
use crate::opstate::OperatingState;

/// A joint's measured position, in the units its channel declares.
#[derive(Debug, Clone, PartialEq)]
pub struct JointReading {
    /// The joint's name, as the descriptor spells it.
    pub name: String,
    /// Its position. Non-finite readings are dropped rather than published — see
    /// [`LiveContext::unreadable`].
    pub position: f64,
}

/// What is known about one calibration set (§45), flattened for a precondition to read.
#[derive(Debug, Clone, PartialEq)]
pub struct CalibrationFact {
    /// Which set.
    pub set: String,
    /// The verdict, which decides `robot.calibration.<set>.valid`.
    pub validity: Validity,
    /// Seconds since it was taken, when both the record and a clock reading were available.
    ///
    /// `None` omits the path rather than reporting zero: §54.2 asks for *"which parameter sets are
    /// valid, and their age"*, and a robot with no clock does not know the age of anything.
    pub age_s: Option<f64>,
}

/// Where the base is, when a driver reports it.
///
/// Position and orientation in one struct because §54.2 names *"base pose"* as one fact, and a
/// context that could publish a position with no orientation would let a precondition about a
/// docked robot read true while it faced the wrong way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BasePose {
    /// Metres, in the body's world frame.
    pub xyz: [f64; 3],
    /// Unit quaternion, `x, y, z, w` — the order §44 uses everywhere.
    pub quaternion: [f64; 4],
}

/// Everything the OS knows, as the arguments this context is built from.
///
/// Every field that can be unknown is an [`Option`] or an empty collection, and every one of those
/// omits its path rather than filling it. That is [`live_context`]'s whole contract with §54.2's
/// first rule.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveInputs {
    /// Measured joint positions.
    pub joints: Vec<JointReading>,
    /// The gripper's opening, 0 closed to 1 open, when the body has one.
    pub gripper: Option<f64>,
    /// Battery charge, 0 to 100.
    pub battery_percent: Option<f64>,
    /// Where the base is.
    pub base_pose: Option<BasePose>,
    /// Where §27's machine says the robot is.
    ///
    /// An [`Option`] even though the daemon always has one, because a context assembled by
    /// something that has not observed the machine must not claim `post` by default — that is a
    /// state, and stating it would let a precondition about the operating state read a value
    /// nobody measured.
    pub operating_state: Option<OperatingState>,
    /// Which of §7.2's eight guards are actually evaluating.
    ///
    /// `None` and `Some(vec![])` are different facts and the distinction is the whole module in one
    /// field: an empty list is a kernel reporting that no guard is running, and `None` is nobody
    /// having asked a kernel. §33.1 keeps `skynetd` on the far side of that line, so a daemon with
    /// no kernel channel supplies `None` — an empty list from it would be a claim rather than an
    /// observation.
    pub guards_active: Option<Vec<String>>,
    /// Which are not, and are therefore reporting unavailable (§45.5).
    pub guards_unavailable: Vec<String>,
    /// One entry per applicable calibration set.
    pub calibration: Vec<CalibrationFact>,
    /// Frame ids that resolve against the flattened graph (§44).
    pub frames_resolved: Vec<String>,
    /// Why the graph does not flatten, when it does not.
    ///
    /// The frame validator's own fault rather than a sentence, so a precondition can name the
    /// **code** — *"refuse when the graph has a cycle"* is a clause somebody will write, and a
    /// string forces them to match on prose. §64's rule that every refusal names the fix is
    /// checked mechanically over the corpus, and a bare string under a key called `fault` is
    /// exactly the shape that check refuses.
    pub frames_fault: Option<FrameFault>,
    /// `guest`, `host` or `certified` (§1).
    pub tier: Option<String>,
    /// The capability tokens the operator granted **this package** (§8, §38).
    pub grants: Vec<String>,
    /// The cell's `env.*` branch, from [`cell_environment`](crate::cell::cell_environment).
    ///
    /// Taken already-built rather than as a [`CellDescriptor`](crate::cell::CellDescriptor),
    /// because §55.2's document has one other consumer — the `workspace` guard — and a robot with
    /// no cell profile at all still has the other six branches.
    pub env: Option<Json>,
}

/// The context, plus what it could not say and why.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveContext {
    /// The document a predicate reads.
    pub context: Json,
    /// Joint and set names no dot path can address, so no precondition can ever name them.
    ///
    /// Reported rather than silently dropped, because the symptom otherwise is a precondition that
    /// refuses with `robot.state.joints.arm.1` unresolvable and an operator who can see the joint
    /// on the console.
    pub unaddressable: Vec<String>,
    /// Readings that were not finite, and are therefore not readings.
    ///
    /// A NaN joint is the fault `drivers/mock` injects and §7.2's envelope guard refuses on. It
    /// must not reach a precondition as a number, and it must not reach one as a missing joint with
    /// no explanation either.
    pub unreadable: Vec<String>,
}

/// A key a dot path can address: non-empty, no dot, and no surrounding whitespace.
///
/// The same rule as `cell`'s, deliberately duplicated rather than shared, because they are two
/// rules that happen to agree: this one is about a name the **body** chose and that one is about a
/// name the **operator** chose, and the two could reasonably diverge about whitespace without
/// either being wrong.
fn addressable(key: &str) -> bool {
    !key.is_empty() && !key.contains('.') && key.trim() == key
}

/// Set a path only when there is something finite to say.
fn set_number(target: &mut Json, key: &str, value: Option<f64>) {
    if let Some(number) = value {
        if number.is_finite() {
            target.set(key, Json::Num(number));
        }
    }
}

fn str_list(values: &[String]) -> Json {
    let mut sorted: Vec<&String> = values.iter().collect();
    sorted.sort();
    sorted.dedup();
    Json::Arr(sorted.iter().map(|v| Json::Str((*v).clone())).collect())
}

/// Build §54.2's context.
///
/// Every branch is omitted rather than emitted empty when nothing is known about it, with one
/// deliberate exception: `skill.grants.tokens` is emitted as an empty array.
///
/// The reason is that a grant is a **file the daemon owns**. "This package was granted nothing" is
/// an answer the OS always has, and a precondition should be able to refuse on it and say so. Every
/// other branch is something the OS learns from somewhere else — a driver, a kernel, a clock, an
/// operator's survey of a room — and any of them can be missing, at which point §54.2's first rule
/// is the honest answer and an empty array is a claim.
pub fn live_context(inputs: &LiveInputs) -> LiveContext {
    let mut unaddressable = Vec::new();
    let mut unreadable = Vec::new();

    // ── robot.state ──────────────────────────────────────────────────────────
    let mut joints = Json::obj();
    let mut any_joint = false;
    for reading in &inputs.joints {
        if !addressable(&reading.name) {
            unaddressable.push(reading.name.clone());
            continue;
        }
        if !reading.position.is_finite() {
            unreadable.push(reading.name.clone());
            continue;
        }
        joints.set(&reading.name, Json::Num(reading.position));
        any_joint = true;
    }

    let mut state = Json::obj();
    if any_joint {
        state.set("joints", joints);
    }
    set_number(&mut state, "gripper", inputs.gripper);
    set_number(&mut state, "battery_percent", inputs.battery_percent);
    if let Some(pose) = inputs.base_pose {
        if pose.xyz.iter().all(|v| v.is_finite()) && pose.quaternion.iter().all(|v| v.is_finite()) {
            let mut base = Json::obj();
            base.set("x", Json::Num(pose.xyz[0]));
            base.set("y", Json::Num(pose.xyz[1]));
            base.set("z", Json::Num(pose.xyz[2]));
            base.set("qx", Json::Num(pose.quaternion[0]));
            base.set("qy", Json::Num(pose.quaternion[1]));
            base.set("qz", Json::Num(pose.quaternion[2]));
            base.set("qw", Json::Num(pose.quaternion[3]));
            state.set("base_pose", base);
        } else {
            unreadable.push("base_pose".to_string());
        }
    }
    // §54.2's table says `robot.state.*` and then lists four examples; §27's state is inside that
    // branch and is the fact a precondition most obviously wants — *"only when the robot is
    // running"* is the first precondition anybody writes. It is published under its own name rather
    // than inside a status object so the path stays stable if §27 ever grows a second field.
    if let Some(operating) = inputs.operating_state {
        state.set("operating", Json::Str(operating.as_str().to_string()));
    }

    // ── robot.guards ─────────────────────────────────────────────────────────
    let mut guards = Json::obj();
    if let Some(active) = &inputs.guards_active {
        guards.set("active", str_list(active));
    }
    if !inputs.guards_unavailable.is_empty() {
        guards.set("unavailable", str_list(&inputs.guards_unavailable));
    }

    // ── robot.calibration ────────────────────────────────────────────────────
    let mut calibration = Json::obj();
    let mut any_calibration = false;
    for fact in &inputs.calibration {
        if !addressable(&fact.set) {
            unaddressable.push(fact.set.clone());
            continue;
        }
        let mut row = Json::obj();
        row.set(
            "valid",
            Json::Bool(matches!(fact.validity, Validity::Valid { .. })),
        );
        row.set("state", Json::Str(fact.validity.as_str().to_string()));
        set_number(&mut row, "age_s", fact.age_s);
        calibration.set(&fact.set, row);
        any_calibration = true;
    }

    // ── robot ────────────────────────────────────────────────────────────────
    let mut robot = Json::obj();
    if state.as_obj().is_some_and(|obj| !obj.is_empty()) {
        robot.set("state", state);
    }
    if guards.as_obj().is_some_and(|obj| !obj.is_empty()) {
        robot.set("guards", guards);
    }
    if any_calibration {
        robot.set("calibration", calibration);
    }
    let mut frames = Json::obj();
    if !inputs.frames_resolved.is_empty() {
        frames.set("resolved", str_list(&inputs.frames_resolved));
    }
    if let Some(fault) = &inputs.frames_fault {
        let mut reported = Json::obj();
        reported.set("code", Json::Str(fault.code.clone()));
        reported.set("detail", Json::Str(fault.detail.clone()));
        reported.set("subjects", str_list(&fault.subjects));
        frames.set("fault", reported);
    }
    if frames.as_obj().is_some_and(|obj| !obj.is_empty()) {
        robot.set("frames", frames);
    }
    if let Some(tier) = &inputs.tier {
        robot.set("tier", Json::Str(tier.clone()));
    }

    // ── skill.grants ─────────────────────────────────────────────────────────
    let mut grants = Json::obj();
    grants.set("tokens", str_list(&inputs.grants));
    let mut skill = Json::obj();
    skill.set("grants", grants);

    let mut context = Json::obj();
    context.set("robot", robot);
    context.set("skill", skill);
    if let Some(env) = &inputs.env {
        context.set("env", env.clone());
    }

    unaddressable.sort();
    unaddressable.dedup();
    unreadable.sort();
    unreadable.dedup();

    LiveContext {
        context,
        unaddressable,
        unreadable,
    }
}

/// What a console shows about a context it could not fully build.
///
/// One sentence per omission, naming the path that will be unresolvable, because §54.2's refusal
/// names a path and an operator has to be able to get from that path back to the reason.
pub fn explain_omissions(built: &LiveContext) -> Vec<String> {
    let mut lines = Vec::new();
    for name in &built.unaddressable {
        lines.push(format!(
            "`{name}` carries a dot, so no precondition can address it — a dot path would read the \
             name as two keys. Nothing can be done about that here: the body chose the name."
        ));
    }
    for name in &built.unreadable {
        lines.push(format!(
            "`{name}` did not report a finite value, so it is absent rather than published. §54.2 \
             refuses a precondition that names it, which is the honest answer to a sensor \
             reporting NaN."
        ));
    }
    lines
}

/// Serialise a built context, omissions included.
///
/// The omissions travel **beside** the context rather than inside it, because a path under `robot.`
/// describing the context itself would be a path a precondition could read — and a skill whose
/// precondition asserts something about the completeness of its own evaluation context is a skill
/// asserting something about the OS rather than about the robot.
pub fn live_context_to_json(built: &LiveContext) -> Json {
    let mut out = Json::obj();
    out.set("context", built.context.clone());
    out.set("unaddressable", str_list(&built.unaddressable));
    out.set("unreadable", str_list(&built.unreadable));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::predicate::{evaluate, Outcome};
    use alloc::vec;

    fn path(built: &LiveContext, path: &str) -> Option<Json> {
        crate::predicate::read_path(&built.context, path).cloned()
    }

    /// The property the whole module exists for, asserted against the thing that consumes it: an
    /// unknown fact does not resolve, so §54.2 refuses the clause that named it.
    #[test]
    fn a_fact_nobody_measured_does_not_resolve() {
        let built = live_context(&LiveInputs::default());
        assert!(path(&built, "robot.state.battery_percent").is_none());

        let clause = Json::parse(r#"{"op":"gte","path":"robot.state.battery_percent","value":20}"#)
            .expect("the clause parses");
        let outcome = evaluate(Some(&clause), &built.context);
        assert_eq!(outcome.outcome, Outcome::Unevaluable);
        assert!(
            outcome.path.as_deref() == Some("robot.state.battery_percent"),
            "the refusal names the path, which is §54.2's whole first rule"
        );
    }

    /// And the difference a zero would make, spelled out, because it is one line either way.
    #[test]
    fn a_battery_the_robot_can_read_resolves() {
        let built = live_context(&LiveInputs {
            battery_percent: Some(62.0),
            ..LiveInputs::default()
        });
        let clause = Json::parse(r#"{"op":"gte","path":"robot.state.battery_percent","value":20}"#)
            .expect("the clause parses");
        assert!(evaluate(Some(&clause), &built.context).satisfied());
    }

    /// The one branch that is always present, and the reason: a grant is a file the daemon owns.
    #[test]
    fn grants_are_always_present_even_when_empty() {
        let built = live_context(&LiveInputs::default());
        assert_eq!(path(&built, "skill.grants.tokens"), Some(Json::arr()));

        // Present *and empty* is a different answer from absent: this one is false rather than
        // unevaluable, which is what lets an operator tell "granted nothing" from "the OS could not
        // tell me".
        let clause =
            Json::parse(r#"{"op":"includes","path":"skill.grants.tokens","value":"arm.left"}"#)
                .expect("the clause parses");
        assert_eq!(
            evaluate(Some(&clause), &built.context).outcome,
            Outcome::Unsatisfied
        );
    }

    /// And the branch that is not, which is the correction this module made to itself. An empty
    /// `robot.guards.active` from a daemon with no kernel channel would be a claim, and a claim
    /// about a guard set is exactly the thing §54.2's first rule refuses to let anybody make.
    #[test]
    fn an_unasked_kernel_and_a_kernel_reporting_nothing_are_different_contexts() {
        let unasked = live_context(&LiveInputs::default());
        assert!(path(&unasked, "robot.guards.active").is_none());
        assert!(path(&unasked, "robot.guards").is_none());

        let asked = live_context(&LiveInputs {
            guards_active: Some(Vec::new()),
            ..LiveInputs::default()
        });
        assert_eq!(path(&asked, "robot.guards.active"), Some(Json::arr()));

        let clause =
            Json::parse(r#"{"op":"includes","path":"robot.guards.active","value":"envelope"}"#)
                .expect("the clause parses");
        assert_eq!(
            evaluate(Some(&clause), &unasked.context).outcome,
            Outcome::Unevaluable,
            "nobody asked a kernel, so the clause is refused rather than answered"
        );
        assert_eq!(
            evaluate(Some(&clause), &asked.context).outcome,
            Outcome::Unsatisfied,
            "a kernel said no guard is running, which is an answer"
        );
    }

    #[test]
    fn a_non_finite_joint_is_reported_rather_than_published() {
        let built = live_context(&LiveInputs {
            joints: vec![
                JointReading {
                    name: "elbow".to_string(),
                    position: f64::NAN,
                },
                JointReading {
                    name: "wrist".to_string(),
                    position: 0.1,
                },
            ],
            ..LiveInputs::default()
        });
        assert!(path(&built, "robot.state.joints.elbow").is_none());
        assert_eq!(
            path(&built, "robot.state.joints.wrist"),
            Some(Json::Num(0.1))
        );
        assert_eq!(built.unreadable, vec!["elbow".to_string()]);
        assert!(explain_omissions(&built)[0].contains("NaN"));
    }

    /// An empty object resolves, and `exists` over one would answer *false* about a robot whose
    /// sensors are all failing — the same word it uses for a body that has no such joint.
    #[test]
    fn a_body_whose_every_joint_is_unreadable_publishes_no_joints_branch() {
        let built = live_context(&LiveInputs {
            joints: vec![JointReading {
                name: "elbow".to_string(),
                position: f64::INFINITY,
            }],
            ..LiveInputs::default()
        });
        assert!(path(&built, "robot.state.joints").is_none());
        assert!(path(&built, "robot.state").is_none());
    }

    #[test]
    fn a_dotted_joint_name_is_named_rather_than_dropped() {
        let built = live_context(&LiveInputs {
            joints: vec![JointReading {
                name: "arm.1".to_string(),
                position: 0.5,
            }],
            ..LiveInputs::default()
        });
        assert_eq!(built.unaddressable, vec!["arm.1".to_string()]);
        assert!(
            built.unreadable.is_empty(),
            "it read fine; it cannot be named"
        );
        assert!(explain_omissions(&built)[0].contains("carries a dot"));
    }

    /// The decision this module makes that §54.2 does not, asserted the way a publisher would use
    /// it: `arm.left` under `skill.grants.` would be two keys, so the branch is a list.
    #[test]
    fn a_grant_token_is_reachable_through_the_list_and_not_as_a_path() {
        let built = live_context(&LiveInputs {
            grants: vec!["arm.left".to_string(), "sensing.camera".to_string()],
            ..LiveInputs::default()
        });
        assert!(path(&built, "skill.grants.arm.left").is_none());

        let clause =
            Json::parse(r#"{"op":"includes","path":"skill.grants.tokens","value":"arm.left"}"#)
                .expect("the clause parses");
        assert!(evaluate(Some(&clause), &built.context).satisfied());
    }

    #[test]
    fn a_base_pose_is_all_seven_numbers_or_none() {
        let built = live_context(&LiveInputs {
            base_pose: Some(BasePose {
                xyz: [1.0, f64::NAN, 0.0],
                quaternion: [0.0, 0.0, 0.0, 1.0],
            }),
            ..LiveInputs::default()
        });
        assert!(path(&built, "robot.state.base_pose").is_none());
        assert!(path(&built, "robot.state.base_pose.x").is_none());
        assert_eq!(built.unreadable, vec!["base_pose".to_string()]);
    }

    #[test]
    fn only_a_valid_calibration_sets_the_boolean_and_the_state_carries_the_rest() {
        let built = live_context(&LiveInputs {
            calibration: vec![
                CalibrationFact {
                    set: "joint_zeros".to_string(),
                    validity: Validity::Valid { checked: vec![] },
                    age_s: Some(10.0),
                },
                CalibrationFact {
                    set: "hand_eye".to_string(),
                    validity: Validity::Failed {
                        detail: "residual 4 exceeds 1".to_string(),
                    },
                    age_s: None,
                },
            ],
            ..LiveInputs::default()
        });
        assert_eq!(
            path(&built, "robot.calibration.joint_zeros.valid"),
            Some(Json::Bool(true))
        );
        assert_eq!(
            path(&built, "robot.calibration.joint_zeros.age_s"),
            Some(Json::Num(10.0))
        );
        assert_eq!(
            path(&built, "robot.calibration.hand_eye.valid"),
            Some(Json::Bool(false))
        );
        assert_eq!(
            path(&built, "robot.calibration.hand_eye.state"),
            Some(Json::Str("failed".to_string()))
        );
        // Valid, and of unknown age. Reporting zero would make a stale set read as one taken this
        // second, which is the direction that lets a skill run.
        assert!(path(&built, "robot.calibration.hand_eye.age_s").is_none());
    }

    #[test]
    fn an_unobserved_operating_state_is_absent_rather_than_post() {
        let built = live_context(&LiveInputs::default());
        assert!(path(&built, "robot.state.operating").is_none());

        let built = live_context(&LiveInputs {
            operating_state: Some(OperatingState::Running),
            ..LiveInputs::default()
        });
        assert_eq!(
            path(&built, "robot.state.operating"),
            Some(Json::Str("running".to_string()))
        );
    }

    #[test]
    fn every_list_is_sorted_and_deduplicated() {
        let built = live_context(&LiveInputs {
            guards_active: Some(vec![
                "workspace".to_string(),
                "envelope".to_string(),
                "workspace".to_string(),
            ]),
            ..LiveInputs::default()
        });
        assert_eq!(
            path(&built, "robot.guards.active"),
            Some(Json::Arr(vec![
                Json::Str("envelope".to_string()),
                Json::Str("workspace".to_string()),
            ]))
        );
    }

    /// §64, from the direction the corpus check found it: a `fault` is something an operator can
    /// act on, so it carries a code and a sentence rather than being one.
    #[test]
    fn a_frame_fault_reaches_a_precondition_whole() {
        let built = live_context(&LiveInputs {
            frames_fault: Some(FrameFault {
                code: "cycle".to_string(),
                subjects: alloc::vec!["tool0".to_string()],
                detail: "tool0 is its own ancestor.".to_string(),
            }),
            ..LiveInputs::default()
        });
        assert_eq!(
            path(&built, "robot.frames.fault.code"),
            Some(Json::Str("cycle".to_string()))
        );
        let clause = Json::parse(r#"{"op":"eq","path":"robot.frames.fault.code","value":"cycle"}"#)
            .expect("the clause parses");
        assert!(evaluate(Some(&clause), &built.context).satisfied());
    }

    /// The omissions travel beside the context, never inside it — a skill must not be able to
    /// write a precondition about the completeness of its own evaluation context.
    #[test]
    fn the_omission_lists_are_not_reachable_from_a_predicate() {
        let built = live_context(&LiveInputs {
            joints: vec![JointReading {
                name: "arm.1".to_string(),
                position: 0.5,
            }],
            ..LiveInputs::default()
        });
        assert!(path(&built, "unaddressable").is_none());
        assert!(path(&built, "robot.unaddressable").is_none());
        let document = live_context_to_json(&built);
        assert_eq!(
            document
                .get("unaddressable")
                .and_then(Json::as_arr)
                .map(<[Json]>::len),
            Some(1)
        );
    }
}

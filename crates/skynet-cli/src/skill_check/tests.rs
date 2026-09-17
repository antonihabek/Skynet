//! What `skynet skill check` has to catch, and the two things it must never claim.
//!
//! Every case here writes a real package directory and runs the real [`inspect`], because the
//! failures this command exists for are properties of a *directory* — a manifest naming a file that
//! is not there — and a test that handed `inspect` a `Json` would not be able to have that defect.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "skynet-skill-check-{}-{label}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, relative: &str, body: &str) {
        let target = self.0.join(relative);
        std::fs::create_dir_all(target.parent().expect("a parent")).expect("mkdir");
        std::fs::write(target, body).expect("write");
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A manifest that passes everything, as the text a publisher would write.
///
/// Built as a string rather than a `Json` so that a test changing one field changes exactly what a
/// publisher would change, and so the parser is on the path under test.
fn manifest(extra: &str) -> String {
    format!(
        r#"{{
  "schema_version": 1,
  "id": "openrobot/park",
  "version": "1.0.0",
  "kind": "skill",
  "digest": "",
  "isolation": "sandboxed",
  "task": "Bring the arm to its park pose and hold.",
  "budget": {{ "ram_mb": 64, "cpu_cores": 0.5, "control_hz": 100 }},
  "embodiment": {{ "capabilities": ["manipulation.arm", "control.position"], "control_mode": "position" }},
  "requests": ["manipulation.arm"],
  "runtime": {{ "entrypoint": "bin/run" }},
  "expected_duration_seconds": 12,
  "safety_notes": "The arm sweeps through its full range on the way to park.",
  "preconditions": [
    {{ "statement": "The robot is running.", "predicate": {{ "op": "eq", "path": "robot.state.operating", "value": "running" }} }}
  ],
  "success_criteria": [
    {{ "statement": "Still running at the end.", "predicate": {{ "op": "eq", "path": "robot.state.operating", "value": "running" }} }}
  ]{extra}
}}
"#
    )
}

/// A package with the manifest text given and an entry point beside it.
fn package(label: &str, manifest_text: &str) -> TempDir {
    let dir = TempDir::new(label);
    dir.write("manifest.json", manifest_text);
    dir.write("bin/run", "#!/bin/sh\nexec skynetp run --backend hold\n");
    dir
}

fn result<'a>(report: &'a Report, id: &str) -> &'a CheckResult {
    report
        .results
        .iter()
        .find(|result| result.id == id)
        .unwrap_or_else(|| {
            panic!(
                "no check called {id}; there are: {}",
                report
                    .results
                    .iter()
                    .map(|result| result.id)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

#[test]
fn a_well_formed_package_fails_nothing() {
    let dir = package("good", &manifest(""));
    let report = inspect(dir.path(), None);

    let failures: Vec<&str> = report
        .results
        .iter()
        .filter(|result| result.outcome == Outcome::Fail)
        .map(|result| result.id)
        .collect();
    assert!(failures.is_empty(), "unexpected failures: {failures:?}");

    assert_eq!(report.id.as_deref(), Some("openrobot/park"));
    assert!(
        report.digest.is_some(),
        "a digest should have been computed"
    );
}

/// The exit code is not "did anything fail". §15's rule is that absence is not a pass, and a
/// package whose entry point nobody could run has not been shown to work.
#[test]
fn a_package_that_fails_nothing_still_exits_nonzero_while_something_is_unmeasured() {
    let dir = package("unmeasured", &manifest(""));
    let report = inspect(dir.path(), None);
    assert_eq!(report.tally(Outcome::Fail), 0);
    assert!(
        report.tally(Outcome::Missing) > 0,
        "the entry point and the transport plan are both unmeasurable here"
    );
    assert_eq!(print(&report), 1);
}

/// The defect this command was written for: an entry point that is not in the package. It installs,
/// activates, and fails to start.
#[test]
fn an_entry_point_that_is_not_there_is_a_failure_naming_the_path() {
    let dir = TempDir::new("no-entry");
    dir.write("manifest.json", &manifest(""));
    let report = inspect(dir.path(), None);

    let check = result(&report, "package.entrypoint.present");
    assert_eq!(check.outcome, Outcome::Fail);
    assert!(check.detail.contains("bin/run"), "{}", check.detail);
}

/// The second one: a path that resolves on no robot. The clause is well formed, the operator is
/// real, and the skill is refused for ever with a message naming a path nobody can supply.
#[test]
fn a_precondition_naming_a_path_no_robot_publishes_is_a_failure() {
    let text = manifest("").replace("robot.state.operating", "robot.state.batery_percent");
    let dir = package("typo", &text);
    let report = inspect(dir.path(), None);

    let check = result(&report, "skill.preconditions.decidable");
    assert_eq!(check.outcome, Outcome::Fail);
    assert!(
        check.detail.contains("robot.state.batery_percent"),
        "the refusal has to name the path: {}",
        check.detail
    );
}

/// `env.*` is the operator's, declared in a cell profile this command has not been shown, so a path
/// under it is addressable at any depth. Refusing one would refuse every site-specific precondition
/// §54.2 exists to permit.
#[test]
fn a_path_under_env_is_addressable_at_any_depth() {
    let text = manifest("").replace("robot.state.operating", "env.fixture.door.closed");
    let dir = package("env", &text);
    let report = inspect(dir.path(), None);
    assert_eq!(
        result(&report, "skill.preconditions.decidable").outcome,
        Outcome::Pass
    );
}

/// A joint name is the body's, and this command has not been given a descriptor. Checking two
/// segments deep is what stops it refusing a precondition about a real joint.
#[test]
fn a_joint_name_is_not_checked_because_the_body_decides_those() {
    let text = manifest("").replace(
        r#""op": "eq", "path": "robot.state.operating", "value": "running""#,
        r#""op": "lte", "path": "robot.state.joints.shoulder_pitch", "value": 1.5"#,
    );
    let dir = package("joint", &text);
    let report = inspect(dir.path(), None);
    assert_eq!(
        result(&report, "skill.preconditions.decidable").outcome,
        Outcome::Pass
    );
}

/// An operator nobody implements is a structural defect, visible with no robot at all.
#[test]
fn an_operator_the_os_does_not_implement_is_a_failure() {
    let text = manifest("").replace(
        r#""op": "eq", "path": "robot.state.operating""#,
        r#""op": "approximately", "path": "robot.state.operating""#,
    );
    let dir = package("unknown-op", &text);
    let report = inspect(dir.path(), None);

    let check = result(&report, "skill.preconditions.decidable");
    assert_eq!(check.outcome, Outcome::Fail);
    assert!(check.detail.contains("approximately"), "{}", check.detail);
}

/// `and` short-circuits, so the evaluator alone would never reach the second operand of a satisfied
/// one. Walking the document is what makes this visible, and it is exactly the typo that then
/// refuses the skill on the first robot where the first operand is false.
#[test]
fn a_bad_path_in_the_unreached_arm_of_an_and_is_still_found() {
    let text = manifest("").replace(
        r#"{ "op": "eq", "path": "robot.state.operating", "value": "running" }"#,
        r#"{ "op": "and", "of": [
          { "op": "eq", "path": "robot.state.operating", "value": "running" },
          { "op": "gte", "path": "robot.batery.percent", "value": 30 }
        ] }"#,
    );
    let dir = package("short-circuit", &text);
    let report = inspect(dir.path(), None);

    let check = result(&report, "skill.preconditions.decidable");
    assert_eq!(
        check.outcome,
        Outcome::Fail,
        "the second operand was never evaluated, and it still has to be found"
    );
    assert!(
        check.detail.contains("robot.batery.percent"),
        "{}",
        check.detail
    );
}

/// §8's narrowing rule. Admission calls this `over_request` and refuses the install.
#[test]
fn requesting_a_token_the_package_never_declared_is_a_failure() {
    let text = manifest("").replace(
        r#""requests": ["manipulation.arm"]"#,
        r#""requests": ["manipulation.arm", "locomotion.walk"]"#,
    );
    let dir = package("over-request", &text);
    let report = inspect(dir.path(), None);

    let check = result(&report, "skill.requests.declared");
    assert_eq!(check.outcome, Outcome::Fail);
    assert!(check.detail.contains("locomotion.walk"), "{}", check.detail);
}

/// §85. A misspelt capability is named with its class, where admission would refuse it as
/// `capability_unknown` — and the control, the same manifest spelt right, passes.
#[test]
fn a_token_the_platform_does_not_define_is_named_before_a_robot_refuses_it() {
    let misspelt = manifest("").replace(
        r#""capabilities": ["manipulation.arm", "control.position"]"#,
        r#""capabilities": ["manipulation.arm", "control.positon"], "extensions": ["sensing.lidar"]"#,
    );
    let report = inspect(package("unknown-token", &misspelt).path(), None);
    let check = result(&report, "skill.capabilities.canonical");
    assert_eq!(check.outcome, Outcome::Fail);
    assert!(
        check.detail.contains("control.positon (unknown)"),
        "{}",
        check.detail
    );
    assert!(
        check
            .detail
            .contains("sensing.lidar (an extension, and canonical)"),
        "{}",
        check.detail
    );

    let spelt = inspect(package("known-token", &manifest("")).path(), None);
    assert_eq!(
        result(&spelt, "skill.capabilities.canonical").outcome,
        Outcome::Pass
    );
}

/// A skill is bounded by definition. Without a duration §54.4's timeout has no input.
#[test]
fn a_skill_with_no_declared_duration_has_no_enforceable_timeout() {
    let text = manifest("").replace(r#""expected_duration_seconds": 12,"#, "");
    let dir = package("unbounded", &text);
    let report = inspect(dir.path(), None);
    assert_eq!(
        result(&report, "skill.timeout.declared").outcome,
        Outcome::Fail
    );
}

/// §54.3: a skill that judges its own success always succeeds. One with no criteria at all reports
/// every invocation `unlabeled`, which is honest and is worth telling the publisher about.
#[test]
fn a_skill_with_no_success_criteria_is_a_failure_that_says_unlabeled() {
    let text = manifest("").replace(
        r#""success_criteria": [
    { "statement": "Still running at the end.", "predicate": { "op": "eq", "path": "robot.state.operating", "value": "running" } }
  ]"#,
        r#""success_criteria": []"#,
    );
    let dir = package("unlabeled", &text);
    let report = inspect(dir.path(), None);

    let check = result(&report, "skill.success.independent");
    assert_eq!(check.outcome, Outcome::Fail);
    assert!(check.detail.contains("unlabeled"), "{}", check.detail);
}

/// Only a driver runs unsandboxed, because the Safety Kernel is the only writer to an actuator.
#[test]
fn a_skill_asking_to_run_unsandboxed_is_a_failure() {
    let text = manifest("").replace(r#""isolation": "sandboxed""#, r#""isolation": "native""#);
    let dir = package("native", &text);
    let report = inspect(dir.path(), None);
    assert_eq!(
        result(&report, "package.isolation.sandboxed").outcome,
        Outcome::Fail
    );
}

/// §54.5, decided by the contract's own walker rather than by anything here.
#[test]
fn a_call_graph_with_a_cycle_in_it_is_refused_at_publish_time() {
    let text = manifest(
        r#",
  "composition": { "calls": [
    { "from": "openrobot/park", "to": "openrobot/home" },
    { "from": "openrobot/home", "to": "openrobot/park" }
  ] }"#,
    );
    let dir = package("cycle", &text);
    let report = inspect(dir.path(), None);

    let check = result(&report, "skill.composition.acyclic");
    assert_eq!(check.outcome, Outcome::Fail);
    assert!(check.detail.contains("openrobot/park"), "{}", check.detail);
}

/// A skill that calls nothing is not a skill with a broken graph.
#[test]
fn a_skill_that_calls_nothing_reports_composition_as_inapplicable() {
    let dir = package("no-calls", &manifest(""));
    let report = inspect(dir.path(), None);
    assert_eq!(
        result(&report, "skill.composition.acyclic").outcome,
        Outcome::NotApplicable
    );
}

/// The card may be flat or nested, and both reach a robot. A checker that read one spelling would
/// pass a package whose preconditions the robot then enforces — or the reverse.
#[test]
fn the_skill_card_is_read_nested_as_well_as_flat() {
    let flat = manifest("");
    let nested = r#"{
  "schema_version": 1,
  "id": "openrobot/park",
  "version": "1.0.0",
  "kind": "skill",
  "digest": "",
  "isolation": "sandboxed",
  "budget": { "ram_mb": 64, "cpu_cores": 0.5, "control_hz": 100 },
  "embodiment": { "capabilities": ["manipulation.arm", "control.position"], "control_mode": "position" },
  "requests": ["manipulation.arm"],
  "runtime": { "entrypoint": "bin/run" },
  "skill": {
    "task": "Bring the arm to its park pose and hold.",
    "expected_duration_seconds": 12,
    "safety_notes": "The arm sweeps through its full range on the way to park.",
    "preconditions": [
      { "statement": "The robot is running.", "predicate": { "op": "eq", "path": "robot.state.operating", "value": "running" } }
    ],
    "success_criteria": [
      { "statement": "Still running at the end.", "predicate": { "op": "eq", "path": "robot.state.operating", "value": "running" } }
    ]
  }
}
"#;

    let flat_dir = package("flat", &flat);
    let nested_dir = package("nested", nested);
    let flat_report = inspect(flat_dir.path(), None);
    let nested_report = inspect(nested_dir.path(), None);

    for id in [
        "skill.timeout.declared",
        "skill.success.independent",
        "skill.safety_notes.present",
        "skill.preconditions.decidable",
    ] {
        assert_eq!(
            result(&flat_report, id).outcome,
            result(&nested_report, id).outcome,
            "{id} came out differently for the two spellings of one card"
        );
    }
}

/// A prose precondition is legal — it is what every skill published before §54.2 carries — and it
/// is not something the OS can check. Reported as unmeasured rather than as a pass.
#[test]
fn a_prose_precondition_is_counted_rather_than_refused() {
    let text = manifest("").replace(
        r#"{ "statement": "The robot is running.", "predicate": { "op": "eq", "path": "robot.state.operating", "value": "running" } }"#,
        r#""The cell door is closed.""#,
    );
    let dir = package("prose", &text);
    let report = inspect(dir.path(), None);

    assert_eq!(
        result(&report, "skill.preconditions.decidable").outcome,
        Outcome::Pass,
        "prose is not a malformed predicate"
    );
    let counted = result(&report, "skill.preconditions.enforceable");
    assert_eq!(counted.outcome, Outcome::Missing);
    assert!(counted.detail.contains("1 of 1"), "{}", counted.detail);
}

/// The roots are derived from the live context, not retyped. If that ever stops being true this
/// test is what notices: it asserts the branches §54.2's table names are all present, which is the
/// property the derivation exists to keep.
#[test]
fn the_derived_vocabulary_separates_the_os_names_from_the_body_and_cell_names() {
    let vocabulary = addressable_paths();

    // Fixed vocabulary: the OS chooses these names and they are the same on every robot.
    for path in [
        "robot.state.operating",
        "robot.state.battery_percent",
        "robot.state.gripper",
        "robot.guards.active",
        "robot.frames.resolved",
        "robot.tier",
        "skill.grants.tokens",
    ] {
        assert!(
            addressable(path, &vocabulary),
            "{path} is vocabulary §54.2 publishes and the derivation missed it"
        );
    }

    // Open branches: the body and the cell choose these names, so nothing below them is checked.
    for path in [
        "robot.state.joints.shoulder_pitch",
        "robot.calibration.hand_eye.valid",
        "env.fixture.door.closed",
    ] {
        assert!(
            addressable(path, &vocabulary),
            "{path} names something the descriptor or the cell decides, which this command has not \
             been given"
        );
    }

    // And the typo the two-segment rule used to let through.
    assert!(!addressable("robot.state.batery_percent", &vocabulary));
    assert!(!addressable("robot.sate.operating", &vocabulary));
    assert!(!addressable("nonsense", &vocabulary));
}

/// §24's facts, and the measurement `DRY-COVERAGE-PLAN.md` Phase 4 asks this widening for.
///
/// *"Every unresolvable path is a body the skill cannot reach, so this is directly measurable: how
/// many bodies a fixed skill set admits on, before and after."* Each path below is a clause
/// somebody writes on the first day — *"only when the stop is clear"*, *"not while the brakes are
/// on"*, *"refuse on an unhomed body"*, *"only when nothing is moving"* — and every one of them
/// was unaddressable, which §54.2 turns into a skill that no body can run.
///
/// The velocity and effort branches are **open**, like the joints they are keyed by: the body
/// chooses those names. The flags are fixed vocabulary, because the OS chooses them.
#[test]
fn the_drive_facts_a_precondition_could_not_read_are_addressable() {
    let vocabulary = addressable_paths();

    for path in [
        "robot.state.estop",
        "robot.state.degraded",
        "robot.state.braked",
        "robot.state.homing",
        "robot.state.fault_code",
    ] {
        assert!(
            addressable(path, &vocabulary),
            "{path} is one of §24's flags and a precondition cannot name it"
        );
    }
    for path in ["robot.state.velocity.elbow", "robot.state.effort.elbow"] {
        assert!(
            addressable(path, &vocabulary),
            "{path} is keyed by a joint name the body chooses, so it must be open"
        );
    }
    // And a typo in the new vocabulary is still refused, which is what makes the four above a check
    // rather than a spelling of `true`.
    assert!(!addressable("robot.state.estopp", &vocabulary));
    assert!(!addressable("robot.state.homed", &vocabulary));
}

/// A directory with no manifest is one failure and not eleven. Reporting every downstream check as
/// a separate failure would bury the one fact the publisher has to act on.
#[test]
fn a_directory_with_no_manifest_reports_one_failure() {
    let dir = TempDir::new("empty");
    let report = inspect(dir.path(), None);
    assert_eq!(report.results.len(), 1);
    assert_eq!(report.results[0].outcome, Outcome::Fail);
}

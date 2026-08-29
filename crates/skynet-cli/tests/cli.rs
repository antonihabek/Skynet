//! The CLI, exercised as a command rather than as a library.
//!
//! The unit tests in each module cover the decisions; these cover the thing a developer actually
//! types. §63.5 states the milestones this tool is measured against — *scaffold builds and loads,
//! 5 minutes; descriptor drafted from URDF, 15 minutes* — and a test suite that never runs the
//! binary cannot tell you whether either holds.
//!
//! The test that compiles a driver and loads it is `#[ignore]`d by default and run explicitly in
//! the `abi` CI job, which is the one guaranteed to have a C compiler. That is scheduling rather
//! than skipping: a check that quietly does not run is the failure mode §15 spends a whole rule on.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const ARM_URDF: &str = r#"<?xml version="1.0"?>
<robot name="Bench Arm">
  <link name="base_link"><collision/></link>
  <link name="upper"/>
  <link name="lower"/>
  <link name="tool"/>
  <joint name="shoulder_pitch" type="revolute">
    <parent link="base_link"/><child link="upper"/>
    <origin xyz="0 0 0.1" rpy="0 0 0"/><axis xyz="0 1 0"/>
    <limit lower="-1.5" upper="1.5" effort="80" velocity="2"/>
  </joint>
  <joint name="elbow_flex" type="revolute">
    <parent link="upper"/><child link="lower"/>
    <origin xyz="0.3 0 0"/><axis xyz="0 1 0"/>
    <limit lower="-2.6" upper="2.6" effort="60" velocity="2"/>
  </joint>
  <joint name="tool_mount" type="fixed">
    <parent link="lower"/><child link="tool"/>
  </joint>
</robot>"#;

fn skynet(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_skynet"))
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("the skynet binary should run")
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("skynet-cli-test-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// stderr with its whitespace collapsed.
///
/// The reports are wrapped to a column width for reading, so a phrase a test looks for may have a
/// newline and an indent in the middle of it. Matching against the flattened form asserts what was
/// said rather than where the wrapper happened to break it.
fn said(output: &Output) -> String {
    stderr(output)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The invocation §63.4 writes down, including the redirect: the descriptor goes to stdout and the
/// reasoning goes to stderr, or `> embodiment.json` captures a file full of prose.
#[test]
fn from_urdf_puts_the_descriptor_on_stdout_and_the_reasoning_on_stderr() {
    let dir = scratch("from-urdf");
    let urdf = dir.join("arm.urdf");
    std::fs::write(&urdf, ARM_URDF).unwrap();

    let output = skynet(&["descriptor", "from-urdf", urdf.to_str().unwrap()]);
    assert!(output.status.success(), "{}", stderr(&output));

    let json = stdout(&output);
    assert!(
        json.starts_with('{'),
        "stdout should be the descriptor: {json}"
    );
    assert!(json.contains("\"envelope\":null"));
    assert!(json.contains("\"robot_slug\":\"bench-arm\""));
    assert!(
        json.contains("\"dof\":2"),
        "the fixed joint is not a degree of freedom"
    );

    let report = stderr(&output);
    assert!(!report.contains('{'), "the report should not carry JSON");
    assert!(
        report.contains("envelope"),
        "the report explains the null envelope"
    );
    assert!(
        report.contains("--assume-side"),
        "and names how to resolve the sides"
    );
}

/// The rule with no flag behind it, checked through the command line rather than the library, so
/// that no future flag can quietly reach past it.
#[test]
fn no_combination_of_flags_populates_the_envelope() {
    let dir = scratch("envelope");
    let urdf = dir.join("arm.urdf");
    std::fs::write(&urdf, ARM_URDF).unwrap();
    let path = urdf.to_str().unwrap();

    for flags in [
        vec![],
        vec!["--accept-suggested-roles", "--assume-side", "left"],
        vec!["--control-modes", "position,velocity,torque"],
        vec!["--base", "wheeled", "--slug", "anything"],
    ] {
        let mut args = vec!["descriptor", "from-urdf", path];
        args.extend(flags.iter());
        let output = skynet(&args);
        assert!(output.status.success(), "{:?}: {}", flags, stderr(&output));
        assert!(
            stdout(&output).contains("\"envelope\":null"),
            "{flags:?} produced an envelope"
        );
    }
}

/// Roles are suggested, never assigned, until somebody says otherwise in as many words.
#[test]
fn roles_stay_null_until_accepted_and_the_flag_is_what_accepts_them() {
    let dir = scratch("roles");
    let urdf = dir.join("arm.urdf");
    std::fs::write(&urdf, ARM_URDF).unwrap();
    let path = urdf.to_str().unwrap();

    let suggested = skynet(&["descriptor", "from-urdf", path, "--assume-side", "left"]);
    assert!(stdout(&suggested).contains("\"role\":null"));
    assert!(
        stderr(&suggested).contains("left_arm.shoulder.pitch"),
        "{}",
        stderr(&suggested)
    );

    let accepted = skynet(&[
        "descriptor",
        "from-urdf",
        path,
        "--assume-side",
        "left",
        "--accept-suggested-roles",
    ]);
    assert!(stdout(&accepted).contains("\"role\":\"left_arm.shoulder.pitch\""));
    assert!(!stdout(&accepted).contains("\"role\":null"));
}

/// Regenerating from the same file must produce the same bytes, or the digest a vendor signs
/// depends on which machine ran the tool.
#[test]
fn the_output_is_reproducible() {
    let dir = scratch("reproducible");
    let urdf = dir.join("arm.urdf");
    std::fs::write(&urdf, ARM_URDF).unwrap();
    let args = ["descriptor", "from-urdf", urdf.to_str().unwrap(), "--quiet"];
    assert_eq!(stdout(&skynet(&args)), stdout(&skynet(&args)));
}

#[test]
fn an_unexpanded_xacro_is_refused_by_name_with_the_command_that_fixes_it() {
    let dir = scratch("xacro");
    let urdf = dir.join("arm.urdf.xacro");
    std::fs::write(
        &urdf,
        r#"<robot name="r" xmlns:xacro="http://ros.org/wiki/xacro">
             <xacro:property name="reach" value="0.4"/>
             <link name="a"/><link name="b"/>
             <joint name="j" type="revolute"><parent link="a"/><child link="b"/>
               <limit lower="${-reach}" upper="${reach}" effort="1" velocity="1"/></joint>
           </robot>"#,
    )
    .unwrap();

    let output = skynet(&["descriptor", "from-urdf", urdf.to_str().unwrap()]);
    assert!(!output.status.success());
    let message = said(&output);
    assert!(message.contains("xacro template"), "{message}");
    assert!(message.contains("xacro robot.urdf.xacro"), "{message}");
}

/// The whole loop §18 promises: draft a body from a URDF, then ask what a policy would do on it,
/// with no hardware anywhere in the picture.
#[test]
fn a_descriptor_drafted_from_urdf_can_be_planned_against_immediately() {
    let dir = scratch("transport");
    let urdf = dir.join("arm.urdf");
    std::fs::write(&urdf, ARM_URDF).unwrap();
    let descriptor = dir.join("embodiment.json");

    let import = skynet(&[
        "descriptor",
        "from-urdf",
        urdf.to_str().unwrap(),
        "--assume-side",
        "left",
        "--accept-suggested-roles",
        "--control-modes",
        "position",
        "--quiet",
        "-o",
        descriptor.to_str().unwrap(),
    ]);
    assert!(import.status.success(), "{}", stderr(&import));

    let policy = dir.join("policy.json");
    std::fs::write(
        &policy,
        r#"{"control_mode":"position","control_hz":30,"action_channels":[
             {"index":0,"kind":"joint_position","role":"left_arm.shoulder.pitch","peak":1.0},
             {"index":1,"kind":"joint_position","role":"left_arm.elbow.flex","peak":1.0}]}"#,
    )
    .unwrap();

    let plan = skynet(&[
        "transport",
        policy.to_str().unwrap(),
        "--robot",
        descriptor.to_str().unwrap(),
        "--json",
    ]);
    assert!(plan.status.success(), "{}", stderr(&plan));
    let json = stdout(&plan);
    // `remap` rather than `direct`: the policy declares roles and no joint names, so the two
    // bodies are related through the canonical vocabulary rather than by a name collision — which
    // is the mechanism the whole role system exists for.
    assert!(json.contains("\"verdict\":\"direct\""), "{json}");
    assert!(json.contains("\"method\":\"remap\""), "{json}");
    assert!(!json.contains("\"method\":\"blocked\""), "{json}");
    assert!(json.contains("\"robot_slug\":\"bench-arm\""));
    // A descriptor nobody has signed produces a plan that says so, rather than one that pretends.
    assert!(json.contains("unverified_embodiment"), "{json}");
    assert!(json.contains("no_target_envelope"), "{json}");
}

/// Without `--control-modes` the planner blocks every channel, and the import report says it will.
/// The two have to agree or the advice is wrong.
#[test]
fn the_warning_about_empty_control_modes_is_accurate() {
    let dir = scratch("modes");
    let urdf = dir.join("arm.urdf");
    std::fs::write(&urdf, ARM_URDF).unwrap();
    let descriptor = dir.join("embodiment.json");

    let import = skynet(&[
        "descriptor",
        "from-urdf",
        urdf.to_str().unwrap(),
        "-o",
        descriptor.to_str().unwrap(),
    ]);
    assert!(
        said(&import).contains("control_mode_unsupported"),
        "{}",
        said(&import)
    );

    let policy = dir.join("policy.json");
    std::fs::write(
        &policy,
        r#"{"control_mode":"position","action_channels":[
             {"index":0,"kind":"joint_position","joint":"shoulder_pitch","peak":1.0}]}"#,
    )
    .unwrap();

    let plan = skynet(&[
        "transport",
        policy.to_str().unwrap(),
        "--robot",
        descriptor.to_str().unwrap(),
        "--json",
    ]);
    let json = stdout(&plan);
    assert!(json.contains("control_mode_unsupported"), "{json}");
    assert!(json.contains("\"method\":\"blocked\""), "{json}");
    assert!(!plan.status.success(), "a refused plan exits non-zero");
}

#[test]
fn roles_parse_or_say_they_did_not() {
    let good = skynet(&["role", "left_arm.shoulder.pitch", "base.wheel#2"]);
    assert!(good.status.success());
    assert!(
        stdout(&good).contains("chain=left_arm"),
        "{}",
        stdout(&good)
    );

    let bad = skynet(&["role", "third_arm.shoulder.pitch"]);
    assert!(!bad.status.success());
    assert!(stdout(&bad).contains("does not parse"), "{}", stdout(&bad));
}

#[test]
fn the_scaffold_writes_a_project_and_tells_you_what_will_fail() {
    let dir = scratch("scaffold");
    let target = dir.join("acme-arm");

    let output = skynet(&[
        "driver",
        "new",
        "acme-arm",
        "--lang",
        "c",
        "--dir",
        target.to_str().unwrap(),
    ]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(target.join("acme_arm.c").is_file());
    assert!(target.join("Makefile").is_file());
    assert!(target.join("README.md").is_file());

    let source = std::fs::read_to_string(target.join("acme_arm.c")).unwrap();
    assert!(source.contains("skynet_rhal_entry"));
    assert!(
        source.contains("\\\"envelope\\\":null"),
        "the stub declares no envelope"
    );

    // The scaffold predicts its own conformance result, and `driver check` has to agree.
    let told = stderr(&output);
    let told = told.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(told.contains("embodiment.roles.coverage"), "{told}");
    assert!(told.contains("embodiment.descriptor.signed"), "{told}");

    // Writing into a non-empty directory needs saying so.
    let again = skynet(&[
        "driver",
        "new",
        "acme-arm",
        "--dir",
        target.to_str().unwrap(),
    ]);
    assert!(!again.status.success());
    assert!(said(&again).contains("--force"), "{}", said(&again));
}

#[test]
fn driver_check_refuses_a_file_that_is_not_a_driver() {
    let dir = scratch("not-a-driver");
    let missing = dir.join("nothing.so");
    let output = skynet(&["driver", "check", missing.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(
        said(&output).contains("does not exist"),
        "{}",
        said(&output)
    );

    let text = dir.join("notes.txt");
    std::fs::write(&text, b"not a shared object").unwrap();
    let output = skynet(&["driver", "check", text.to_str().unwrap()]);
    assert!(!output.status.success());
    assert!(said(&output).contains("shared object"), "{}", said(&output));
}

/// §63.5's first two milestones, measured rather than asserted: a scaffold that builds and loads,
/// and a conformance run whose failures are the to-do list its own README promised.
///
/// `#[ignore]` because it needs a C compiler. The `abi` CI job runs it explicitly.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn a_fresh_c_scaffold_builds_loads_and_reaches_guest_tier() {
    let dir = scratch("scaffold-build");
    let target = dir.join("probe-arm");
    let bindings = repository_root().join("bindings/c");

    let created = skynet(&[
        "driver",
        "new",
        "probe-arm",
        "--lang",
        "c",
        "--dir",
        target.to_str().unwrap(),
    ]);
    assert!(created.status.success(), "{}", stderr(&created));

    let library = target.join(if cfg!(windows) {
        "probe_arm.dll"
    } else if cfg!(target_os = "macos") {
        "libprobe_arm.dylib"
    } else {
        "libprobe_arm.so"
    });

    // `CC` first, then the usual names. A vendor's machine has one of these; naming all three in
    // the failure is more use than insisting on the one this developer happens to have.
    let candidates: Vec<String> = std::env::var("CC")
        .into_iter()
        .chain(["cc", "gcc", "clang"].iter().map(|name| name.to_string()))
        .collect();
    let mut build = None;
    for compiler in &candidates {
        let attempt = Command::new(compiler)
            .args([
                "-std=c11",
                "-O2",
                "-Wall",
                "-Wextra",
                "-Wpedantic",
                "-fPIC",
                "-shared",
            ])
            .arg(format!("-I{}", bindings.display()))
            .arg("-o")
            .arg(&library)
            .arg(target.join("probe_arm.c"))
            .output();
        if let Ok(output) = attempt {
            build = Some(output);
            break;
        }
    }
    let build = build.unwrap_or_else(|| {
        panic!(
            "no C compiler found. Tried: {}. Set CC.",
            candidates.join(", ")
        )
    });
    assert!(
        build.status.success(),
        "the scaffold must compile clean under -Wall -Wextra -Wpedantic:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );

    let checked = skynet(&["driver", "check", library.to_str().unwrap()]);
    let report = stderr(&checked);

    // Guest tier, which is what "builds and loads" means in the tier model.
    assert!(
        checked.status.success(),
        "a fresh scaffold should reach guest tier:\n{report}"
    );
    assert!(report.contains("sim evidence supports"), "{report}");

    // And its two blockers are exactly the two the scaffold's README names, in the same words.
    assert!(
        report.contains("embodiment.roles.coverage — failed"),
        "{report}"
    );
    assert!(
        report.contains("embodiment.descriptor.signed — not measured"),
        "{report}"
    );

    // The runtime-level sim tests are named as not covered rather than quietly counted as passes.
    assert!(report.contains("safety.kernel.parity"), "{report}");
    assert!(report.contains("not covered here"), "{report}");
}

fn repository_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/skynet-cli`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the workspace root")
        .to_path_buf()
}

// ── `skynet frames` (§44) ────────────────────────────────────────────────────

/// A two-link planar arm with geometry, written as a URDF so the whole path is exercised: the
/// importer reads it, the descriptor carries §44.3's block, and the kernel walks it.
const GEOMETRY_URDF: &str = r#"<?xml version="1.0"?>
<robot name="Bench Arm">
  <link name="base_link">
    <collision><geometry><cylinder radius="0.06" length="0.1"/></geometry></collision>
  </link>
  <link name="upper">
    <collision><origin xyz="0.25 0 0"/><geometry><box size="0.5 0.08 0.08"/></geometry></collision>
  </link>
  <link name="tool"/>
  <joint name="shoulder" type="revolute">
    <parent link="base_link"/><child link="upper"/>
    <origin xyz="0 0 0.5"/><axis xyz="0 0 1"/>
    <limit lower="-3" upper="3" effort="80" velocity="2"/>
  </joint>
  <joint name="tool_mount" type="fixed">
    <parent link="upper"/><child link="tool"/><origin xyz="0.5 0 0"/>
  </joint>
</robot>"#;

fn descriptor_from(dir: &Path, urdf: &str) -> PathBuf {
    let source = dir.join("robot.urdf");
    std::fs::write(&source, urdf).expect("write urdf");
    let out = skynet(&["descriptor", "from-urdf", source.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let path = dir.join("robot.json");
    std::fs::write(&path, stdout(&out)).expect("write descriptor");
    path
}

#[test]
fn the_whole_path_from_a_urdf_to_a_pose() {
    // The point of the command: a developer with a URDF and no robot can see where a configuration
    // puts the tool, using the same kinematics the kernel will run in cycle.
    let dir = scratch("frames-pose");
    let descriptor = descriptor_from(&dir, GEOMETRY_URDF);

    let out = skynet(&[
        "frames",
        descriptor.to_str().unwrap(),
        "--joints",
        "shoulder=1.5707963267948966",
        "--json",
    ]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));

    // A quarter turn about z takes the tool from (0.5, 0, 0.5) to (0, 0.5, 0.5).
    let json = stdout(&out);
    assert!(json.contains("\"collision_pairs\":1"), "{json}");
    assert!(json.contains("\"lookahead\":true"), "{json}");
    assert!(
        json.contains("\"name\":\"tool\",\"resolved\":true"),
        "the tool resolved: {json}"
    );
    assert!(
        json.contains(",1,0.5]") || json.contains("0.5,0.5]"),
        "{json}"
    );
}

#[test]
fn a_joint_with_no_value_is_unresolved_rather_than_assumed_to_be_at_zero() {
    // A limb assumed straight is a limb in a place it is not, and the report says which frame and
    // why rather than printing a pose nobody supplied the inputs for.
    let dir = scratch("frames-unresolved");
    let descriptor = descriptor_from(&dir, GEOMETRY_URDF);

    let out = skynet(&["frames", descriptor.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let report = stderr(&out);
    assert!(report.contains("no value for shoulder"), "{report}");
    assert!(
        report.contains("upper above it has none"),
        "the frame below names its parent rather than its own joint: {report}"
    );
}

#[test]
fn the_report_says_what_the_pose_guards_will_be_able_to_do() {
    let dir = scratch("frames-guards");
    let descriptor = descriptor_from(&dir, GEOMETRY_URDF);
    let out = skynet(&["frames", descriptor.to_str().unwrap()]);
    let report = stderr(&out);
    assert!(
        report.contains("3 of 3 frames can be constrained"),
        "{report}"
    );
    assert!(report.contains("2 collision bodies, 1 pairs"), "{report}");
}

#[test]
fn one_collision_body_is_reported_as_a_guard_that_cannot_run() {
    // §7.2, on a workstation: a body with one capsule has nothing to collide with, and learning
    // that here is better than learning it from a guard silently doing nothing.
    let dir = scratch("frames-one-body");
    let descriptor = descriptor_from(&dir, ARM_URDF);
    let out = skynet(&["frames", descriptor.to_str().unwrap()]);
    let report = stderr(&out);
    assert!(report.contains("the guard reports unavailable"), "{report}");
}

#[test]
fn a_descriptor_with_no_frame_block_says_which_of_the_two_absences_it_is() {
    let dir = scratch("frames-absent");
    let path = dir.join("bare.json");
    std::fs::write(&path, r#"{"robot_slug":"x","base":"fixed","dof":0}"#).expect("write");
    let out = skynet(&["frames", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("has no `frames` block"),
        "{}",
        stderr(&out)
    );

    let path = dir.join("null.json");
    std::fs::write(
        &path,
        r#"{"robot_slug":"x","base":"fixed","dof":0,"frames":null}"#,
    )
    .expect("write");
    let out = skynet(&["frames", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("could not express this body's tree"),
        "null is the generator saying so, which is a different fact: {}",
        stderr(&out)
    );
}

#[test]
fn a_malformed_graph_is_refused_with_the_frame_named() {
    let dir = scratch("frames-bad");
    let path = dir.join("bad.json");
    std::fs::write(
        &path,
        r#"{"robot_slug":"x","base":"fixed","dof":0,"frames":{"root":"base_link","convention":"rep103","nodes":[{"name":"base_link","parent":null,"kind":"root"},{"name":"arm","parent":"nowhere","kind":"static"}]}}"#,
    )
    .expect("write");
    let out = skynet(&["frames", path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let message = stderr(&out);
    assert!(message.contains("parent_missing"), "{message}");
    assert!(message.contains("arm"), "the frame is named: {message}");
}

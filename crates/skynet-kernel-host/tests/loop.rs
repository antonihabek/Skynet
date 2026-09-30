//! The loop, against a driver that is really a shared object.
//!
//! `src/tests.rs` drives the loop against `driver::Fake`, which is arrays. That covers the
//! decisions and none of the ABI: a body reached through `dlopen`, a vtable checked at load, three
//! caller-allocated arrays a stranger's C fills, and a `write_command` that returns an `errno`.
//! Between the two suites, the same loop is exercised against a model and against a machine.
//!
//! `#[ignore]` because it needs a C compiler, following `skynet-cli`'s scaffold test — the `abi`
//! CI job runs both explicitly. A silent skip on a machine with no compiler is the failure §15
//! spends a rule on, so the test panics with the compilers it tried rather than passing.

use std::path::{Path, PathBuf};
use std::process::Command;

fn repository_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/crates/skynet-kernel-host`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate lives two levels below the workspace root")
        .to_path_buf()
}

fn library_name(stem: &str) -> String {
    if cfg!(windows) {
        format!("{stem}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{stem}.dylib")
    } else {
        format!("lib{stem}.so")
    }
}

/// Build `drivers/mock` as a shared object in a directory of this test's own.
///
/// Per test, not per suite. Cargo runs these in parallel and they first shared one path, which on
/// Windows is `LoadLibraryW` error 32 — one test compiling over the file another has mapped. On
/// Linux it is worse than a failure: the loader would map a half-written object.
fn build_mock(who: &str) -> PathBuf {
    build_driver(
        who,
        "mock_robot",
        &repository_root().join("drivers/mock/mock_robot.c"),
    )
}

/// Build one C source as a driver shared object named `stem`, in a directory of the test's own.
fn build_driver(who: &str, stem: &str, source: &Path) -> PathBuf {
    let root = repository_root();
    let out = std::env::temp_dir().join(format!("skynet-kernel-host-{who}"));
    std::fs::create_dir_all(&out).expect("a scratch directory");
    let library = out.join(library_name(stem));

    let candidates: Vec<String> = std::env::var("CC")
        .into_iter()
        .chain(["cc", "gcc", "clang"].iter().map(|name| name.to_string()))
        .collect();

    let mut build = None;
    for compiler in &candidates {
        let attempt = Command::new(compiler)
            .args(["-std=c11", "-O2", "-Wall", "-Wextra", "-fPIC", "-shared"])
            .arg(format!("-I{}", root.join("bindings/c").display()))
            .arg("-o")
            .arg(&library)
            .arg(source)
            .arg("-lm")
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
        "the mock robot must compile:\n{}",
        String::from_utf8_lossy(&build.stderr)
    );
    library
}

fn skynetk(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_skynetk"))
        .args(args)
        .output()
        .expect("skynetk runs")
}

/// The enforcing loop on a machine that backs nothing — CI's stock runners included. The
/// override is the harness's, explicit per spawn rather than ambient, so the refusal test
/// below runs beside these with the gate closed.
fn skynetk_unchecked(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_skynetk"))
        .args(args)
        .env("SKYNETK_UNCHECKED_TIER", "1")
        .output()
        .expect("skynetk runs")
}

/// The loop turns against a real driver, and every guard says what it can and cannot do.
///
/// This is the run §73's table was waiting on. What it establishes is narrow and worth stating: a
/// cycle is turned, the kernel is asked, a command crosses the RHAL, and the guards that cannot run
/// name the input they lack rather than passing. It does **not** establish that anything moved
/// under enforcement — nothing publishes to the ring, so every cycle holds.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn the_loop_turns_against_a_real_driver_and_names_what_it_cannot_check() {
    let library = build_mock("turns");

    let output = skynetk_unchecked(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--tier",
        "host",
        "--velocity-max",
        "2.0",
        "--jerk",
        "0.1",
        "--watchdog-ms",
        "50",
        "--hz",
        "200",
        "--cycles",
        "40",
    ]);

    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "the run must succeed:\n{said}");

    // Host tier resolves to enforcing, and nothing in the loop chose that — `Tier::enforcement`
    // derives it and takes no argument.
    assert!(said.contains("enforcement Enforcing"), "{said}");

    // §7.2: a guard that cannot run says so, and says what it needs. The mock declares no frame
    // graph and no proximity sensor, so four of the eight are unavailable and each names its input.
    assert!(said.contains("unavailable Workspace"), "{said}");
    assert!(said.contains("embodiment.frames"), "{said}");
    assert!(said.contains("unavailable Proximity"), "{said}");
    assert!(said.contains("profile.proximity"), "{said}");

    // Forty cycles, and the ring nobody filled.
    assert!(said.contains("cycles          40"), "{said}");
    assert!(said.contains("read failures   0"), "{said}");
    assert!(said.contains("write failures  0"), "{said}");
    assert!(
        said.contains("idle            40"),
        "every cycle should be idle: there is no policy runtime to publish a command\n{said}"
    );
}

/// A jerk ceiling on the command line reaches the guard that needs it.
///
/// Regression: `--jerk` was parsed, stored and wired nowhere, and the only thing that noticed was
/// §7.2's own output naming `profile.max_delta_per_cycle` as missing on a run where `--jerk` had
/// been passed. The flag is on the **operator profile**, not the transport clamp.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn the_jerk_ceiling_reaches_the_rate_limit_guard() {
    let library = build_mock("jerk");

    let with = skynetk_unchecked(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--tier",
        "host",
        "--velocity-max",
        "2.0",
        "--jerk",
        "0.1",
        "--watchdog-ms",
        "50",
        "--cycles",
        "2",
    ]);
    let with = String::from_utf8_lossy(&with.stdout).into_owned();
    assert!(
        !with.contains("unavailable RateLimit"),
        "a declared jerk ceiling must make the rate-limit guard available:\n{with}"
    );

    let without = skynetk_unchecked(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--tier",
        "host",
        "--velocity-max",
        "2.0",
        "--watchdog-ms",
        "50",
        "--cycles",
        "2",
    ]);
    let without = String::from_utf8_lossy(&without.stdout).into_owned();
    assert!(
        without.contains("unavailable RateLimit"),
        "and without one it must report itself unavailable rather than passing:\n{without}"
    );
    assert!(
        without.contains("profile.max_delta_per_cycle"),
        "naming the input it lacks:\n{without}"
    );
}

/// A driver that is not a driver is refused at load, not at the first cycle.
#[test]
fn a_file_that_is_not_a_driver_is_refused_before_any_cycle() {
    let path = std::env::temp_dir().join("skynet-kernel-host-not-a-driver.txt");
    std::fs::write(&path, b"this is not a shared object").expect("the scratch file is writable");

    let output = skynetk(&["run", "--driver", path.to_str().unwrap(), "--cycles", "1"]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        said.contains("shared object"),
        "the failure should name what was wrong with the file:\n{said}"
    );
}

/// A channel list that disagrees with the body is refused, because it would command a joint that
/// does not exist.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn a_channel_list_that_disagrees_with_the_body_is_refused() {
    let library = build_mock("channels");

    let output = skynetk(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--channels",
        "only,two",
        "--cycles",
        "1",
    ]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(
        said.contains("command a joint that does not exist"),
        "{said}"
    );
}

/// A conforming driver's descriptor is read, and the return value is taken as the length it is.
///
/// Regression, and the reason it is here rather than in `skynet-dl`: `Driver::describe` read §4's
/// return value — *bytes written* — as a status, so every conforming driver's descriptor was a
/// failure. Nothing noticed because nothing called it until a camera needed to be checked against
/// the body, and the mock is the conforming driver this suite already builds.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn a_conforming_drivers_descriptor_is_read() {
    let library = build_mock("describe");
    let mut body = skynet_kernel_host::driver::Rhal::open(&library, "").expect("the mock opens");
    let text = body
        .describe()
        .expect("a driver that returns its length has returned a descriptor");
    assert!(text.starts_with('{'), "{text}");
    assert!(text.contains("\"robot_slug\":\"mock-arm\""), "{text}");
    assert!(
        !body.has_sensors(),
        "and the mock declares no read_sensor, which the next test relies on"
    );
}

/// A frame pool for a body with no sensors is refused before anything is offered.
///
/// A pool offered for a stream the body cannot produce tells a policy it has a camera. The mock
/// has no `read_sensor`, so the refusal names that — and names it before a socket or a mapping has
/// been made, which is why this runs on any host with a C compiler.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn a_frame_pool_for_a_body_with_no_sensors_is_refused() {
    let library = build_mock("no-sensors");
    let socket = std::env::temp_dir().join("skynet-kernel-host-no-sensors.sock");

    let output = skynetk(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--policy-socket",
        socket.to_str().unwrap(),
        "--frame-pool",
        "wrist:2x64@10",
        "--cycles",
        "1",
    ]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(said.contains("no read_sensor entry point"), "{said}");
    assert!(
        !socket.exists(),
        "refused before the socket was made, so nothing was offered"
    );
}

/// A frame pool declared without a rate is refused, naming the missing part.
#[test]
fn a_frame_pool_with_no_capture_rate_is_refused() {
    let output = skynetk(&[
        "run",
        "--driver",
        "unused",
        "--policy-socket",
        "unused.sock",
        "--frame-pool",
        "wrist:2x64",
    ]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(said.contains("capture rate"), "{said}");
}

// ── §23.4: the socket belongs to one group ─────────────────────────────────────

/// A group with no socket to give it is refused, naming both flags.
///
/// Silently ignoring the flag would leave an operator believing the rings were confined to a group
/// when there were no rings at all, and would hide the mistake until the one package that should
/// have them could not connect.
#[test]
fn a_socket_group_without_a_socket_is_refused() {
    let output = skynetk(&[
        "run",
        "--driver",
        "unused",
        "--policy-socket-group",
        "skynet-ring",
    ]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(said.contains("without --policy-socket"), "{said}");
}

/// An empty group name is refused rather than read as no group.
///
/// A systemd unit spelling the flag `--policy-socket-group ${GROUP}` passes one empty argument when
/// the variable is unset, and a loop that took that for "share with nobody" would open the socket
/// at whatever mode it was created with.
#[test]
fn an_empty_socket_group_is_refused() {
    let output = skynetk(&[
        "run",
        "--driver",
        "unused",
        "--policy-socket",
        "unused.sock",
        "--policy-socket-group",
        "",
    ]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(said.contains("empty name"), "{said}");
}

// ── §89: a sensor read is not the control period's to spend ─────────────────────

/// `tests/fixtures/slow_camera.c`, whose camera takes `render_ms` a frame and declares
/// `concurrent_read` when asked to.
#[cfg(unix)]
fn build_slow_camera(who: &str) -> PathBuf {
    build_driver(
        who,
        "slow_camera",
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/slow_camera.c"),
    )
}

/// The number after `label` at the start of a summary line, e.g. `overruns        3 — …`.
#[cfg(unix)]
fn summary_number(said: &str, label: &str) -> u64 {
    said.lines()
        .find_map(|line| line.strip_prefix(label))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("no `{label}` line in the summary:\n{said}"))
}

/// Run the slow camera for 40 cycles of a 20 ms period with a pool read at the loop's rate.
#[cfg(unix)]
fn run_slow_camera(who: &str, config: &str) -> String {
    let library = build_slow_camera(who);
    let socket = std::env::temp_dir().join(format!("skynet-kernel-host-{who}.sock"));
    let _ = std::fs::remove_file(&socket);
    let output = skynetk(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--config",
        config,
        "--hz",
        "50",
        "--cycles",
        "40",
        "--policy-socket",
        socket.to_str().unwrap(),
        "--frame-pool",
        "wrist:2x192@50",
    ]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "the run must succeed:\n{said}");
    said
}

/// §89.2 through a real driver: a 30 ms render on a 20 ms period, and a driver that does not
/// declare `concurrent_read`. The stream is read on the control thread, costs the period on each of
/// its first three reads, and is then suspended — so the loop overruns three times and not forty,
/// which is `DRY-COVERAGE-PLAN.md` F24's measurement with the rule that closes it in place.
#[cfg(unix)]
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn an_undeclared_slow_camera_is_suspended_and_the_loop_keeps_its_period() {
    let said = run_slow_camera("slow-undeclared", r#"{"render_ms":30}"#);

    assert!(
        said.contains("read between cycles on the control thread"),
        "{said}"
    );
    assert!(said.contains("suspended at read 3"), "{said}");
    assert!(
        said.contains("read on         nothing after read 3"),
        "{said}"
    );
    let overruns = summary_number(&said, "overruns");
    assert!(
        (3..=4).contains(&overruns),
        "the three reads before suspension cost the period, and nothing after them should:\n{said}"
    );
    assert_eq!(summary_number(&said, "frames read"), 3, "{said}");
}

/// §89.3, and the control for the test above: the same camera, declared `concurrent_read`, is read
/// on a thread of its own. It is never suspended, it keeps delivering frames for the whole run, and
/// the control loop does not pay for a single render.
#[cfg(unix)]
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn a_declared_slow_camera_is_read_on_its_own_thread_and_costs_the_loop_nothing() {
    let said = run_slow_camera("slow-declared", r#"{"render_ms":30,"concurrent":true}"#);

    assert!(said.contains("read on a thread of its own"), "{said}");
    assert!(!said.contains("suspended"), "{said}");
    assert!(
        said.contains("read on         a thread of its own"),
        "{said}"
    );
    assert_eq!(summary_number(&said, "overruns"), 0, "{said}");
    assert!(
        summary_number(&said, "frames read") >= 10,
        "0.8 s of 30 ms renders is some twenty frames:\n{said}"
    );
}

/// §23.5's per-process half, in the startup report: the lock and the kernel it runs on.
///
/// The lock is taken, declined or failed loudly — all three are answers, and silence is none
/// of them. Which one it is depends on `RLIMIT_MEMLOCK`, which is the machine's, so the test
/// asserts the *shape* rather than the outcome: an `mlock` line carrying the probe, the limit
/// it declined under, or the failure, and an `rt` line naming what `/sys/kernel/realtime`
/// said.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn the_startup_report_names_the_lock_and_the_kernel() {
    let library = build_mock("rt-lines");

    let output = skynetk(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--cycles",
        "5",
    ]);
    let said = String::from_utf8_lossy(&output.stdout).to_string();
    assert!(output.status.success(), "the run must succeed:\n{said}");

    let mlock = said
        .lines()
        .find(|line| line.starts_with("mlock "))
        .expect("an mlock line");
    assert!(
        mlock.contains("VmLck") || mlock.contains("not locked") || mlock.contains("FAILED"),
        "the lock is reported with its probe, its limit, or its failure: {mlock}"
    );
    assert!(
        said.lines().any(|line| line.starts_with("rt ")),
        "an rt line naming the kernel: {said}"
    );
    assert!(
        said.lines().any(|line| line.starts_with("swap ")),
        "a swap line naming SwapTotal: {said}"
    );
}

/// `--tier host` refuses on a machine that cannot back the claim, and names what it read.
///
/// Machine-aware rather than fixed: the test computes the refusal from the same readings the
/// binary refuses on, so on a PREEMPT_RT robot with locked memory and no swap it starts
/// instead — which is the gate's control, running wherever a robot is. The `abi` job's
/// runners are stock kernels, so there this is the refusal half, every run.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn the_host_claim_is_refused_on_a_machine_that_cannot_back_it() {
    let expected = skynet_rt::refuse_host(&skynet_rt::host_readings());
    let library = build_mock("host-gate");

    let output = skynetk(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--tier",
        "host",
        "--cycles",
        "2",
    ]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if expected.is_empty() {
        assert!(
            output.status.success(),
            "a machine that backs the claim must start:\n{said}"
        );
        return;
    }
    assert!(
        !output.status.success(),
        "a machine that cannot back the claim must refuse:\n{said}"
    );
    for reason in &expected {
        assert!(
            said.contains(reason),
            "the refusal must name what it read ({reason}):\n{said}"
        );
    }
    assert!(
        said.contains("SKYNETK_UNCHECKED_TIER"),
        "the refusal must say how the harness runs it anyway:\n{said}"
    );
}

/// A simulated body's own clock is the loop's, and its recording says it was simulated (§35.3, F70).
///
/// `drivers/mock` declares `simulator: true` and keeps a clock of its own — one second at open, a
/// millisecond a read, the machine's clock nowhere in it. Until F70 `skynetk` asked the kernel at
/// its own reading of `CLOCK_MONOTONIC_RAW` whatever the body declared, recorded that reading, and
/// sealed every window with `simulator: false` — so a mock run's black box said it was a field run
/// on the one field `seal.rs` carries to stop exactly that. This reads the window back rather than
/// trusting the report: the flag in the header, and every cycle recorded at the body's own time.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn a_simulated_body_keeps_its_own_clock_and_says_so_in_the_recording() {
    let library = build_mock("simulated-time");
    let recording = std::env::temp_dir().join("skynet-kernel-host-simulated-time-box");
    // A previous run's ring would be reopened rather than recreated, and its records are not this
    // run's evidence.
    let _ = std::fs::remove_dir_all(&recording);

    let output = skynetk(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--hz",
        "200",
        "--cycles",
        "20",
        "--blackbox",
        recording.to_str().unwrap(),
    ]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "the run must succeed:\n{said}");
    assert!(
        said.contains("time       the simulator's own clock (§35.3)"),
        "the startup report names the clock every number after it is on:\n{said}"
    );

    let store = std::fs::read(recording.join("ring")).expect("the run wrote a ring");
    let ring = skynet_blackbox::ring::Ring::open(&store).expect("a ring this edition reads");
    assert!(
        ring.meta().simulator,
        "the window records that its body declared itself a simulator"
    );
    let cycles: Vec<u64> = ring
        .ordered(&store)
        .into_iter()
        .filter(|record| matches!(record.body, skynet_blackbox::record::Body::Cycle(_)))
        .map(|record| record.mono_ns)
        .collect();
    assert_eq!(cycles.len(), 20, "one record per cycle:\n{said}");
    // From the mock's fifth read: `Rhal::open` takes the first, to learn the joint count from
    // it, and §48.2's self-test takes the next three — observing the asserted stop, verifying the
    // release cleared the flag, and judging the encoders — before the loop turns.
    let expected: Vec<u64> = (5..=24)
        .map(|read| 1_000_000_000 + read * 1_000_000)
        .collect();
    assert_eq!(
        cycles, expected,
        "every cycle is recorded at the mock's own time — one second, and a millisecond a read — \
         because that is the time the kernel was asked at"
    );
}

/// A policy's source embodiment written into a scratch file, for `--source`.
fn source_file(who: &str, channels: &[&str]) -> PathBuf {
    let path = std::env::temp_dir().join(format!("skynet-kernel-host-{who}-source.json"));
    let bound: Vec<String> = (channels.iter().enumerate())
        .map(|(index, joint)| {
            format!(r#"{{"index":{index},"kind":"joint_position","joint":"{joint}"}}"#)
        })
        .collect();
    let text = format!(
        r#"{{"robot_slug":"trainer","dof":{},"control_mode":"position","control_hz":100,"action_channels":[{}],"observations":[]}}"#,
        channels.len(),
        bound.join(",")
    );
    std::fs::write(&path, text).expect("a scratch source embodiment");
    path
}

/// DRY-COVERAGE-PLAN.md F158: with a plan, every channel carries its own joint's rating.
///
/// Before `--source` the loop could hold no joint's rating — the envelope guard ran against an empty
/// table — so the assertion that matters is the plan line: three channels, three ceilings, named by
/// the joints the mock declares. Held to a control: without `--source` the same run names `joint0`.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn a_source_embodiment_gives_every_channel_its_joints_rating() {
    let library = build_mock("source");
    let source = source_file("source", &["J1", "J2", "J3"]);
    let driver = library.to_str().unwrap();
    let run = |extra: &[&str]| {
        let mut args = vec!["run", "--driver", driver, "--hz", "100", "--cycles", "5"];
        args.extend_from_slice(extra);
        let output = skynetk(&args);
        let said = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        (output.status.success(), said)
    };

    let (ok, said) = run(&["--source", source.to_str().unwrap()]);
    assert!(ok, "the run must succeed:\n{said}");
    assert!(said.contains("channels   J1, J2, J3"), "{said}");
    assert!(
        said.contains("3 channel(s) from") && said.contains("3 with a ceiling"),
        "every channel has its joint's rating:\n{said}"
    );

    let (ok, said) = run(&[]);
    assert!(ok, "{said}");
    assert!(said.contains("channels   joint0, joint1, joint2"), "{said}");
    assert!(
        !said.contains("plan       "),
        "no plan, and none claimed:\n{said}"
    );
}

/// The loop writes channel i to joint i, so a plan that binds elsewhere is refused, not obeyed.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn a_plan_that_reorders_the_joints_is_refused() {
    let library = build_mock("reorder");
    let source = source_file("reorder", &["J2", "J1", "J3"]);
    let output = skynetk(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--cycles",
        "5",
        "--source",
        source.to_str().unwrap(),
    ]);
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "a reordering plan must refuse:\n{said}"
    );
    assert!(said.contains("reorders joints"), "{said}");

    let output = skynetk(&[
        "run",
        "--driver",
        library.to_str().unwrap(),
        "--source",
        source.to_str().unwrap(),
        "--channels",
        "a,b,c",
    ]);
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("second answer"),
        "--source and --channels together are refused"
    );
}

/// Read `--limits-report`'s document, once it names a plan whose origin contains `origin`.
#[cfg(unix)]
fn report_naming(path: &Path, origin: &str) -> skynet_json::Json {
    for _ in 0..200 {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(document) = skynet_json::Json::parse(&text) {
                let plan = document
                    .get("plan")
                    .and_then(skynet_json::Json::as_str)
                    .unwrap_or_default();
                if plan.contains(origin) {
                    return document;
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!(
        "{} never named a plan from `{origin}`:\n{}",
        path.display(),
        std::fs::read_to_string(path).unwrap_or_default()
    );
}

/// One entry of the report's `per_channel` or `joint_modes` table.
#[cfg(unix)]
fn entry(document: &skynet_json::Json, table: &str, name: &str) -> skynet_json::Json {
    document
        .get(table)
        .and_then(|table| table.get(name))
        .cloned()
        .unwrap_or(skynet_json::Json::Null)
}

/// Connect for the rings as a policy would, and say whether they were handed over.
///
/// The hand-over carries one byte of payload beside the descriptors; a refused connection is closed
/// with nothing in it, which reads as end of file.
#[cfg(unix)]
fn take_the_rings(socket: &Path) -> (std::os::unix::net::UnixStream, bool) {
    use std::io::Read;
    let mut stream = std::os::unix::net::UnixStream::connect(socket).expect("the loop listens");
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .expect("a read timeout");
    let mut byte = [0u8; 1];
    let handed = matches!(stream.read(&mut byte), Ok(1));
    (stream, handed)
}

/// §93: the plan reaches the loop from the supervisor's rendezvous, when a policy connects.
///
/// Everything is read off the loop's own report rather than off anything this test kept: with no
/// file there, the body's own ratings; a package's source, adopted when a policy connects and held
/// in its own units; a wider plan waiting while an earlier policy is alive (§33.4); a plan the body
/// refuses refusing the rings; and the window sealed with the set it let go of.
#[cfg(unix)]
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn the_rendezvous_plan_reaches_the_loop_when_a_policy_connects() {
    let library = build_mock("rendezvous");
    let dir = std::env::temp_dir().join("skynet-kernel-host-rendezvous-run");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("bb")).expect("a scratch directory");
    let source = dir.join("source.json");
    let report = dir.join("kernel-limits.json");
    // `sun_path` is 108 bytes, and a temporary directory can be long.
    let socket = PathBuf::from(format!("/tmp/skynet-rv-{}.sock", std::process::id()));
    let _ = std::fs::remove_file(&socket);

    let log = std::fs::File::create(dir.join("k.log")).expect("a log");
    let mut kernel = Command::new(env!("CARGO_BIN_EXE_skynetk"))
        .args(["run", "--driver", library.to_str().unwrap()])
        .args(["--hz", "100", "--cycles", "250", "--watchdog-ms", "200"])
        .arg("--source-from")
        .arg(&source)
        .arg("--limits-report")
        .arg(&report)
        .arg("--policy-socket")
        .arg(&socket)
        .arg("--blackbox")
        .arg(dir.join("bb"))
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .expect("skynetk starts");

    // Nothing at the rendezvous: the body's own ratings, J1 held in position.
    // The mock declares J1's travel truncated at ±3.14, so its ceiling is that truncation — not π.
    #[allow(clippy::approx_constant)]
    let truncated = 3.14;
    let own = report_naming(&report, "the body's own ratings");
    assert_eq!(entry(&own, "joint_modes", "J1").as_str(), Some("position"));
    assert_eq!(entry(&own, "per_channel", "J1").as_f64(), Some(truncated));
    for _ in 0..200 {
        if socket.exists() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // A package's source, in torque for the two joints that offer it. A changed mode is a
    // widening, and with no earlier policy alive it applies at once.
    std::fs::write(
        &source,
        r#"{"package":"acme/arm","source_embodiment":{"robot_slug":"trainer","dof":3,"control_mode":"torque","control_hz":100,"action_channels":[{"index":0,"kind":"joint_torque","joint":"J1"},{"index":1,"kind":"joint_torque","joint":"J2"},{"index":2,"kind":"joint_position","joint":"J3"}],"observations":[]}}"#,
    )
    .expect("the rendezvous");
    let (_first, handed) = take_the_rings(&socket);
    assert!(handed, "a plan this body accepts hands the rings over");
    let package = report_naming(&report, "acme/arm");
    assert_eq!(
        entry(&package, "joint_modes", "J1").as_str(),
        Some("torque")
    );
    assert_eq!(
        entry(&package, "per_channel", "J1").as_f64(),
        Some(80.0),
        "J1's ceiling is its torque rating now, not its travel"
    );

    // Back to position while the first policy, which is this process, is still alive: a widening,
    // so it waits, and the report goes on naming the plan in force.
    std::fs::write(
        &source,
        r#"{"package":"acme/reach","source_embodiment":{"robot_slug":"trainer","dof":3,"control_mode":"position","control_hz":100,"action_channels":[{"index":0,"kind":"joint_position","joint":"J1"},{"index":1,"kind":"joint_position","joint":"J2"},{"index":2,"kind":"joint_position","joint":"J3"}],"observations":[]}}"#,
    )
    .expect("the rendezvous");
    let (_second, handed) = take_the_rings(&socket);
    assert!(
        handed,
        "the policy takes the rings, under the narrower plan"
    );

    // A plan the body refuses refuses the rings.
    std::fs::write(
        &source,
        r#"{"package":"acme/other","source_embodiment":{"robot_slug":"trainer","dof":1,"control_mode":"position","control_hz":100,"action_channels":[{"index":0,"kind":"joint_position","joint":"nope"}],"observations":[]}}"#,
    )
    .expect("the rendezvous");
    let (_third, handed) = take_the_rings(&socket);
    assert!(!handed, "a refused plan must not hand the rings over");

    let status = kernel.wait().expect("skynetk exits");
    let said = std::fs::read_to_string(dir.join("k.log")).unwrap_or_default();
    assert!(status.success(), "{said}");
    assert!(said.contains("plan       now"), "{said}");
    assert!(said.contains("waits until every policy"), "{said}");
    assert!(said.contains("refused the rings"), "{said}");
    let held = report_naming(&report, "acme/arm");
    assert_eq!(
        entry(&held, "joint_modes", "J1").as_str(),
        Some("torque"),
        "the wider plan never applied while the first policy was alive"
    );

    // The window, with the set it let go of: the body's own ratings, until the package's took over.
    let sealed = std::fs::read_dir(dir.join("bb").join("sealed"))
        .expect("a sealed window")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.to_string_lossy().ends_with(".limits.json"))
        .expect("the limits beside the window");
    let beside = skynet_json::Json::parse(&std::fs::read_to_string(&sealed).unwrap()).unwrap();
    let earlier = beside.arr_or_empty("earlier");
    assert_eq!(earlier.len(), 1, "one change, one earlier set");
    assert!(earlier[0]
        .get("until_sequence")
        .and_then(skynet_json::Json::as_f64)
        .is_some());
    assert_eq!(
        entry(earlier[0].get("limits").unwrap(), "joint_modes", "J1").as_str(),
        Some("position")
    );
    let _ = std::fs::remove_file(&socket);
}

/// The number after `label` in `skynetk`'s report, in whatever unit follows it.
fn reported(said: &str, label: &str) -> f64 {
    let line = said
        .lines()
        .find(|line| line.starts_with(label))
        .unwrap_or_else(|| panic!("no `{label}` line in:\n{said}"));
    line[label.len()..]
        .split_whitespace()
        .next()
        .and_then(|word| word.parse().ok())
        .unwrap_or_else(|| panic!("no number after `{label}` in `{line}`"))
}

/// F195 (12.C.5): the loop turns at the rate it was asked to, which it did not — it slept `period - work` after each cycle, so every cycle
/// was the period plus however late the sleep woke, and a kilohertz was 930 Hz. Measured from the loop's own report, which divides the
/// cycles it turned by the time it took. A runner can stall for a few milliseconds in a two-second run, so the loop gets three tries to show
/// it *can* keep its rate; all three short is a loop that cannot.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
fn the_loop_turns_at_the_rate_it_was_asked_for() {
    let library = build_mock("rate");
    let mut rates = Vec::new();
    for _ in 0..3 {
        let output = skynetk(&[
            "run",
            "--driver",
            library.to_str().unwrap(),
            "--hz",
            "500",
            "--cycles",
            "1000",
            "--quiet",
        ]);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let said = String::from_utf8_lossy(&output.stdout).into_owned();
        rates.push(reported(&said, "loop rate"));
        if rates.last().is_some_and(|rate| *rate >= 485.0) {
            return;
        }
    }
    panic!("the loop was asked for 500 Hz and turned at {rates:?} — 3% short or more, three runs in a row");
}

/// 12.C.5: a loop descheduled for thirty periods says so. The work time cannot — every cycle's work was fast and only the sleep was long,
/// so the `overruns` counter stays at zero — and a stall that nothing reports is a stall nobody can budget for. `SIGSTOP` and `SIGCONT`
/// are the operating system's own: nothing inside the process is told.
#[test]
#[ignore = "needs a C compiler; run explicitly with --ignored (the abi CI job does)"]
#[cfg(unix)]
fn a_loop_that_was_descheduled_reports_a_late_start_and_does_not_make_up_for_it() {
    let library = build_mock("stall");
    let child = Command::new(env!("CARGO_BIN_EXE_skynetk"))
        .args([
            "run",
            "--driver",
            library.to_str().unwrap(),
            "--hz",
            "100",
            "--cycles",
            "150",
            "--quiet",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("skynetk runs");
    let signal = |name: &str| {
        let status = Command::new("kill")
            .args([name, &child.id().to_string()])
            .status()
            .expect("kill runs");
        assert!(status.success(), "kill {name}");
    };
    std::thread::sleep(std::time::Duration::from_millis(400));
    signal("-STOP");
    std::thread::sleep(std::time::Duration::from_millis(400)); // forty periods
    signal("-CONT");
    let output = child.wait_with_output().expect("skynetk ends");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let said = String::from_utf8_lossy(&output.stdout).into_owned();
    assert!(
        reported(&said, "late starts") >= 1.0,
        "a stall of forty periods is at least one late start:\n{said}"
    );
    let interval = said
        .lines()
        .find(|line| line.starts_with("cycle interval"))
        .expect("the interval line");
    let worst: f64 = interval
        .split("worst ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|word| word.parse().ok())
        .expect("a worst interval");
    assert!(
        worst >= 300.0,
        "the stall is in the distribution as the worst interval, {worst} ms:\n{said}"
    );
    assert_eq!(
        reported(&said, "overruns"),
        0.0,
        "and the work was never slow, which is why the work time could not say so:\n{said}"
    );
    let p99 = interval
        .split("p99 ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|word| word.parse::<f64>().ok())
        .expect("a p99");
    assert!(p99 < 30.0, "no catch-up burst and no slow loop: the ordinary cycle is still a period ({p99} ms at p99)");
}

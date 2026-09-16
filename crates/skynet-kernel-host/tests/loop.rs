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
    let root = repository_root();
    let out = std::env::temp_dir().join(format!("skynet-kernel-host-{who}"));
    std::fs::create_dir_all(&out).expect("a scratch directory");
    let library = out.join(library_name("mock_robot"));

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
            .arg(root.join("drivers/mock/mock_robot.c"))
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

    let output = skynetk(&[
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

    let with = skynetk(&[
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

    let without = skynetk(&[
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
}

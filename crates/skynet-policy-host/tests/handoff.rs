//! §25's hand-off, between two processes that were started independently.
//!
//! Every other test of the command path runs in one process, and says so: *"a single process is
//! what a test can start, and what it exercises is the wire."* That was true until the descriptor
//! had a route. This one starts `skynetp` as a real child, gives it a socket rather than a mapping,
//! and reads what it publishes through a ring it never saw created.
//!
//! What that adds over the in-process test is the only part §80 said was missing: the mapping is
//! made by one process, the writes are made by another, and the page tables are what connect them.
//!
//! Linux and nothing else. `memfd_create` and `SCM_RIGHTS` are the mechanism, and on every other
//! platform the crate answers `Unsupported` — so this file is compiled away rather than skipped at
//! runtime, which keeps a passing run on a developer's Windows machine from reading as evidence.

#![cfg(target_os = "linux")]

use std::process::Command;
use std::time::{Duration, Instant};

use skynet_kernel_host::handoff::Port;
use skynet_obs::Observation;
use skynet_ring::{Command as RingCommand, Reading};

/// Where the socket goes. One per test, because two tests binding one path is a race with itself.
fn socket_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "skynet-handoff-test-{name}-{}.sock",
        std::process::id()
    ))
}

#[test]
fn a_policy_in_another_process_fills_a_ring_this_one_created() {
    let path = socket_path("published");
    let (mapping, body, mut port) = Port::open(&path, 16, 16).expect("the runtime's rings");

    // The policy, as a separate program. `--cycles` bounds it so the test cannot hang on a producer
    // that will not stop, and the setpoints are what this test reads back out of the ring.
    let mut policy = Command::new(env!("CARGO_BIN_EXE_skynetp"))
        .args([
            "run",
            "--hz",
            "200",
            "--cycles",
            "40",
            "--channels",
            "2",
            "--backend",
            "trajectory",
            "--setpoints",
            "0:0.25,-0.5",
            "--ring-socket",
            path.to_str().expect("a utf-8 path"),
            "--quiet",
        ])
        .spawn()
        .expect("skynetp starts");

    // The runtime's side of the hand-off: offer the descriptor until somebody takes it. A policy
    // that has not reached its `connect` yet is the ordinary case for the first few passes, which
    // is why the port is level-triggered rather than a single accept.
    let deadline = Instant::now() + Duration::from_secs(10);
    while port.handed() == 0 && Instant::now() < deadline {
        port.offer(&mapping, &body, None);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(port.handed(), 1, "the policy never took the descriptor");

    // And then read what it wrote. Polled rather than assumed: the producer is a separate process
    // on its own clock, and a test that read once would be asserting about scheduling.
    let ring = mapping.ring().expect("the ring this process created");
    let mut consumer = ring.consumer();
    let mut got = RingCommand::default();
    let mut commanded = None;
    while Instant::now() < deadline {
        if consumer.read(&mut got) == Reading::Fresh && got.mask[0] != 0 {
            commanded = Some(got);
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    let status = policy.wait().expect("skynetp exits");
    assert!(status.success(), "skynetp failed: {status}");

    let commanded = commanded.expect("no commanded slot ever appeared in the ring");
    assert_eq!(commanded.channels, 2);
    assert_eq!(
        &commanded.values[..2],
        &[0.25, -0.5],
        "the values the other process published, read through the mapping this one made",
    );
    assert_eq!(&commanded.mask[..2], &[1, 1]);
}

/// A policy pointed at a socket nobody is listening on.
///
/// It must refuse rather than fall back to a ring in its own memory: a producer that quietly
/// publishes where nothing reads is the failure §79.7 records — *"the address answered, the sender
/// backed off, no bytes moved, and the failure was indistinguishable from patience."*
#[test]
fn a_policy_with_no_runtime_to_join_refuses_rather_than_publishing_into_itself() {
    let path = socket_path("absent");
    let _ = std::fs::remove_file(&path);

    let output = Command::new(env!("CARGO_BIN_EXE_skynetp"))
        .args([
            "run",
            "--cycles",
            "1",
            "--ring-socket",
            path.to_str().expect("a utf-8 path"),
        ])
        .output()
        .expect("skynetp runs");

    assert!(!output.status.success(), "it should have refused");
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(
        said.contains("no runtime is listening"),
        "the refusal has to name what is missing: {said}",
    );
}

/// The loop, closed across two processes.
///
/// The one this file exists for. Every other test of the control path runs in one process; this
/// asserts that a command produced **in another process** is a function of a body published **in
/// this one** — which is the whole of §25's arrangement and the thing that could not be written
/// until both rings crossed a process boundary.
///
/// `approach` closes half the remaining distance each cycle at `--gain 0.5`, so the arithmetic is
/// what makes this a test rather than a demonstration: from 0.0 toward 1.0 the command is 0.5, and
/// from 0.5 it is 0.75. A policy that could not see the body holds and commands nothing at all.
#[test]
fn a_command_from_another_process_is_a_function_of_the_body_this_one_published() {
    let path = socket_path("closed-loop");
    let (commands, body, mut port) = Port::open(&path, 16, 16).expect("the runtime rings");
    let observations = skynet_obs::Ring::open(body.words()).expect("the observation ring");
    let mut publisher = observations.producer();

    // Publish where the body is *before* the policy starts, so its first cycle has something to
    // read: a policy that connects to a ring nobody has written yet reads `Empty` and holds, which
    // is correct and is not what this test is about.
    publisher.publish(&at(0.0)).expect("publish");

    let mut policy = Command::new(env!("CARGO_BIN_EXE_skynetp"))
        .args([
            "run",
            "--hz",
            "200",
            "--cycles",
            "600",
            "--channels",
            "1",
            "--backend",
            "approach",
            "--target",
            "1.0",
            "--gain",
            "0.5",
            "--ring-socket",
            path.to_str().expect("a utf-8 path"),
            "--quiet",
        ])
        .spawn()
        .expect("skynetp starts");

    let deadline = Instant::now() + Duration::from_secs(15);
    while port.handed() == 0 && Instant::now() < deadline {
        port.offer(&commands, &body, None);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(port.handed(), 1, "the policy never took the descriptors");

    let ring = commands.ring().expect("the command ring");
    let mut consumer = ring.consumer();

    let halfway = settle(&mut consumer, deadline).expect("no command arrived from the policy");
    assert!(
        (halfway - 0.5).abs() < 1e-3,
        "half the distance from 0.0 to 1.0, computed in another process: {halfway}",
    );

    // Move the body, and the command has to move with it. This is the assertion that a private ring
    // could never make: the number changes because a different process read what this one wrote.
    publisher.publish(&at(0.5)).expect("publish");
    let mut moved = halfway;
    while Instant::now() < deadline {
        if let Some(value) = settle(&mut consumer, deadline) {
            if (value - 0.75).abs() < 1e-3 {
                moved = value;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(
        (moved - 0.75).abs() < 1e-3,
        "half the distance from 0.5 to 1.0: {moved}",
    );

    let _ = policy.kill();
    let _ = policy.wait();
}

/// An observation of one joint at `position`, with everything else absent rather than zero.
fn at(position: f32) -> Observation {
    let mut sample = Observation {
        stamp_ns: 1,
        seq: 1,
        joints: 1,
        ..Observation::default()
    };
    sample.position[0] = position;
    sample
}

/// Read until a commanded slot appears, and answer with its first value.
fn settle(consumer: &mut skynet_ring::Consumer<'_>, deadline: Instant) -> Option<f32> {
    let mut got = RingCommand::default();
    while Instant::now() < deadline {
        if consumer.read(&mut got) == Reading::Fresh && got.mask[0] != 0 {
            return Some(got.values[0]);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
}

/// The way a policy the **daemon** starts finds the runtime.
///
/// No `--ring-socket` and no `--channels`: a package declares its own `runtime.args` (§8), so
/// `skynetd` passes the rendezvous in the unit's environment instead of editing somebody else's
/// command line. `skynetd_io::shape::rendezvous` decides who is told; this asserts the other end
/// reads it.
#[test]
fn a_policy_finds_the_ring_through_the_environment_when_nothing_is_on_its_command_line() {
    let path = socket_path("environment");
    let (commands, body, mut port) = Port::open(&path, 16, 16).expect("the runtime rings");
    let observations = skynet_obs::Ring::open(body.words()).expect("the observation ring");
    observations.producer().publish(&at(0.0)).expect("publish");

    let mut policy = Command::new(env!("CARGO_BIN_EXE_skynetp"))
        .args([
            "run",
            "--hz",
            "200",
            "--cycles",
            "40",
            "--backend",
            "approach",
            "--target",
            "1.0",
            "--gain",
            "0.5",
            "--quiet",
        ])
        .env("SKYNET_RING_SOCKET", path.to_str().expect("a utf-8 path"))
        .env("SKYNET_RING_CHANNELS", "1")
        .spawn()
        .expect("skynetp starts");

    let deadline = Instant::now() + Duration::from_secs(15);
    while port.handed() == 0 && Instant::now() < deadline {
        port.offer(&commands, &body, None);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(port.handed(), 1, "the policy never took the descriptors");

    let ring = commands.ring().expect("the command ring");
    let commanded = settle(&mut ring.consumer(), deadline).expect("no command arrived");
    assert!(
        (commanded - 0.5).abs() < 1e-3,
        "a policy given nothing but an environment still closed the loop: {commanded}",
    );

    let status = policy.wait().expect("skynetp exits");
    assert!(status.success(), "skynetp failed: {status}");
}

/// §47.6's pool crosses the same socket as the rings, and the other process can address it.
///
/// This is the part §80 recorded as missing after `skynet-frames` existed: the crate's own suite
/// maps a pool in one process, which proves the arithmetic and proves nothing about the hand-off.
/// Here the region is created by this process, the descriptors go over `SCM_RIGHTS` with the two
/// rings in one message, and the geometry is never told to the child — it reads the slot count and
/// the slot size out of the pool's own header, which is the reason `adopt_declared` exists.
///
/// The frame is published **before** the child connects, on purpose. A pool that had published
/// nothing would exercise the same mapping and the same header, and would pass identically if
/// `latest` were hard-coded to `None`; a frame already in it makes the assertion say which slot.
#[test]
fn a_frame_pool_crosses_with_the_rings_and_the_policy_reads_its_shape_from_it() {
    use skynet_frames::{Geometry, Writer};

    let path = socket_path("frames");
    let (commands, body, mut port) = Port::open(&path, 16, 16).expect("the runtime's rings");

    // Three slots and 24 bytes: numbers no default would produce, so a child that reported them
    // read them rather than assumed them.
    let mut pool = Writer::create(
        "test_rgb",
        Geometry {
            slots: 3,
            slot_bytes: 24,
        },
    )
    .expect("a pool");
    pool.publish(1, 5_000, b"pixels").expect("published");

    let policy = Command::new(env!("CARGO_BIN_EXE_skynetp"))
        .args([
            "run",
            "--hz",
            "200",
            "--cycles",
            "20",
            "--channels",
            "1",
            "--backend",
            "trajectory",
            "--setpoints",
            "0:0.25",
            "--ring-socket",
            path.to_str().expect("a utf-8 path"),
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("skynetp starts");

    let deadline = Instant::now() + Duration::from_secs(15);
    while port.handed() == 0 && Instant::now() < deadline {
        port.offer(&commands, &body, Some(&pool));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(port.handed(), 1, "the policy never took the descriptors");

    let output = policy.wait_with_output().expect("skynetp exits");
    assert!(output.status.success(), "skynetp failed: {}", output.status);
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        said.contains(r#"frame pool "test_rgb" received with the rings"#),
        "the policy did not report the pool it was handed. It said:\n{said}",
    );
    // Slot 0, because the pool was empty and §47.6 fills the lowest free slot. What matters is that
    // the child named a slot at all: it could only have done so by reading the header's `latest`.
    assert!(
        said.contains("newest frame in slot 0"),
        "the policy mapped the pool and did not see the frame in it. It said:\n{said}",
    );

    // And the loans region really is the child's to write: it exited, so nothing holds a slot, and
    // the writer can take the one the frame is in.
    pool.publish(2, 6_000, b"again").expect("the slot is free");
}

/// A runtime with no pool hands over two descriptors, and the policy says nothing about frames.
///
/// The other half of the count being the announcement. Without this, "four descriptors means a
/// pool" would be asserted and "two means no pool" would be assumed, and the assumption is the one
/// that fails quietly — a policy that printed a frame line on every robot would be claiming a
/// camera that is not there, which §34.3 is exactly about.
#[test]
fn a_runtime_with_no_pool_hands_over_two_descriptors_and_the_policy_claims_no_camera() {
    let path = socket_path("no-frames");
    let (commands, body, mut port) = Port::open(&path, 16, 16).expect("the runtime's rings");

    let policy = Command::new(env!("CARGO_BIN_EXE_skynetp"))
        .args([
            "run",
            "--hz",
            "200",
            "--cycles",
            "20",
            "--channels",
            "1",
            "--backend",
            "trajectory",
            "--setpoints",
            "0:0.25",
            "--ring-socket",
            path.to_str().expect("a utf-8 path"),
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("skynetp starts");

    let deadline = Instant::now() + Duration::from_secs(15);
    while port.handed() == 0 && Instant::now() < deadline {
        port.offer(&commands, &body, None);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(port.handed(), 1, "the policy never took the descriptors");

    let output = policy.wait_with_output().expect("skynetp exits");
    assert!(output.status.success(), "skynetp failed: {}", output.status);
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        !said.contains("frame pool"),
        "a policy handed no pool talked about one. It said:\n{said}",
    );
}

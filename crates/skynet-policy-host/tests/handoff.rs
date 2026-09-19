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
    let (mapping, body, mut port) =
        Port::open(&path, 16, 16, skynet_obs::TimeBase::Monotonic).expect("the runtime's rings");

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
    let (commands, body, mut port) =
        Port::open(&path, 16, 16, skynet_obs::TimeBase::Monotonic).expect("the runtime rings");
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
    let (commands, body, mut port) =
        Port::open(&path, 16, 16, skynet_obs::TimeBase::Monotonic).expect("the runtime rings");
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
    let (commands, body, mut port) =
        Port::open(&path, 16, 16, skynet_obs::TimeBase::Monotonic).expect("the runtime's rings");

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
        port.offer(&commands, &body, Some(pool.descriptors()));
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
    // And the backend was shown it on every turn, which is the step past naming the slot: the
    // child took a loan, copied the pixels, and gave the loan back before deciding.
    assert!(
        said.contains(
            "20 of 20 turn(s) were shown a frame (0 missed, 0 too old, 20 with no frame budget to \
             check one against)"
        ),
        "the policy named the frame and did not hand it to its backend. It said:\n{said}",
    );

    // And the loans region really is the child's to write: it exited, so nothing holds a slot, and
    // the writer can take the one the frame is in.
    pool.publish(2, 6_000, b"again").expect("the slot is free");
}

/// **A joint command that is a function of a pixel, across two processes.**
///
/// `DRY-COVERAGE-PLAN.md`'s Phase 1 gate, less the driver: the frames here are published by this
/// process rather than rendered by a simulator, and what that costs is stated at the end of this
/// comment. Everything between the pool and the ring is the real thing.
///
/// Until this test there was no point in the workspace where §47's path changed what a joint was told
/// to do. `Camera` published frames and `PolicyHost` copied them out and handed them to a backend,
/// and every backend that existed ignored them: `Trajectory` reads the clock, `Approach` reads the
/// body. So the pool could have been filled with zeros and every test would have passed.
///
/// The model makes the assertion arithmetic rather than a demonstration. Its one weight row is
/// `[0, 1, 0, 0, 0]`: the joint position is multiplied by zero, and the second input — the **top-left
/// cell of the pooled frame** — is multiplied by one. So the commanded setpoint *is* the mean
/// intensity of the top-left quadrant of the image, and a frame of `51`s must command `0.2` while a
/// frame of `204`s must command `0.8`.
///
/// # What this does not do
///
/// No driver and no simulator. `skynetk` reads a camera through `read_sensor` and publishes it into
/// this same pool — `drivers/python-host`'s `check-capture` target is that half, against the LeRobot
/// fake's wrist camera through the real ABI — and joining the two into one run needs a machine with
/// the driver's interpreter and a `skynetk` process, which is `make -C drivers/python-host
/// check-loop`. What is asserted here is the half that can be asserted from `cargo test`: the pool
/// crosses, a policy in another process reads it, and the pixels reach the ring as a number.
#[test]
fn a_command_in_another_process_is_a_function_of_a_pixel_this_one_published() {
    use skynet_frames::{Geometry, Writer};

    let path = socket_path("vision-loop");
    let (commands, body, mut port) =
        Port::open(&path, 16, 16, skynet_obs::TimeBase::Monotonic).expect("the runtime's rings");
    let observations = skynet_obs::Ring::open(body.words()).expect("the observation ring");
    observations.producer().publish(&at(0.0)).expect("publish");

    // A 4x4 RGB frame, 48 bytes, pooled into four cells of 2x2 pixels each.
    let mut pool = Writer::create(
        "wrist",
        Geometry {
            slots: 3,
            slot_bytes: 48,
        },
    )
    .expect("a pool");

    // `reads: ["position"]` and not position-and-velocity, because `at()` above publishes what a real
    // position-only driver publishes: §24's `NaN` for the velocity it does not measure. A model that
    // read one would hold on every cycle, correctly — which is the case `Inputs::reads` exists for.
    let model = std::env::temp_dir().join(format!("skynet-vision-{}.json", std::process::id()));
    std::fs::write(
        &model,
        r#"{
  "kind": "mlp",
  "version": 1,
  "inputs": {
    "channels": 1,
    "reads": ["position"],
    "vision": { "width": 4, "height": 4, "components": 3, "pool": 2 }
  },
  "layers": [
    { "activation": "identity", "weights": [[0.0, 1.0, 0.0, 0.0, 0.0]], "bias": [0.0] }
  ]
}"#,
    )
    .expect("the model file");

    // Dim, then bright. Only the top-left quadrant is read, so the rest of each frame is filled with
    // the complement — a policy reading the wrong cell, or the frame at the wrong stride, lands on it.
    let dim = quadrant_frame(51, 204);
    let bright = quadrant_frame(204, 51);
    pool.publish(1, 5_000, &dim).expect("published");

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
            "inference",
            "--policy",
            model.to_str().expect("a utf-8 path"),
            "--ring-socket",
            path.to_str().expect("a utf-8 path"),
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("skynetp starts");

    let deadline = Instant::now() + Duration::from_secs(20);
    while port.handed() == 0 && Instant::now() < deadline {
        port.offer(&commands, &body, Some(pool.descriptors()));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(port.handed(), 1, "the policy never took the descriptors");

    let ring = commands.ring().expect("the command ring");
    let mut consumer = ring.consumer();

    let dark = settle_at(&mut consumer, 0.2, deadline);
    assert!(
        dark.is_some(),
        "no command carrying the dim frame's top-left mean (0.2) ever arrived",
    );

    // The frame changes, and the command has to change with it. This is the assertion the whole file
    // was building toward: the number moved because a different process read a pixel this one wrote.
    pool.publish(2, 6_000, &bright).expect("published");
    let lit = settle_at(&mut consumer, 0.8, deadline);
    assert!(
        lit.is_some(),
        "the frame changed and the command did not follow it to 0.8",
    );

    let _ = policy.kill();
    let output = policy.wait_with_output().expect("skynetp exits");
    let said = String::from_utf8_lossy(&output.stdout);
    assert!(
        said.contains("skynetp: model — mlp[1x5 identity] from"),
        "the policy did not report the model it loaded. It said:\n{said}",
    );

    let _ = std::fs::remove_file(&model);
}

/// A 4x4 RGB frame whose top-left 2x2 quadrant is `inside` and whose other three are `outside`.
fn quadrant_frame(inside: u8, outside: u8) -> Vec<u8> {
    let mut frame = Vec::with_capacity(4 * 4 * 3);
    for y in 0..4 {
        for x in 0..4 {
            let value = if y < 2 && x < 2 { inside } else { outside };
            frame.extend_from_slice(&[value, value, value]);
        }
    }
    frame
}

/// Read until a commanded slot carries `wanted`, within a tolerance a byte-to-float mean allows.
fn settle_at(
    consumer: &mut skynet_ring::Consumer<'_>,
    wanted: f32,
    deadline: Instant,
) -> Option<f32> {
    let mut got = RingCommand::default();
    while Instant::now() < deadline {
        if consumer.read(&mut got) == Reading::Fresh
            && got.mask[0] != 0
            && (got.values[0] - wanted).abs() < 2e-3
        {
            return Some(got.values[0]);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    None
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
    let (commands, body, mut port) =
        Port::open(&path, 16, 16, skynet_obs::TimeBase::Monotonic).expect("the runtime's rings");

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

/// **A departed policy is noticed, by the kernel, and only once.**
///
/// §47.6 lends a slot to a reader and the writer will not reuse it until the loan comes back. A
/// policy that dies holding one pins that slot for the life of the robot, and
/// `skynet_frames::Writer::revoke_loans` is the remedy — which had no caller for six sessions,
/// because nothing that holds the pool had a **sound** signal that the reader was gone.
/// `DRY-COVERAGE-PLAN.md` F18 is that finding, and this is the test of its answer.
///
/// Two children rather than one, because the rule is all-or-nothing and the interesting half is the
/// refusal. The loans region is one claim per slot and carries no owner, so there is no revoking one
/// reader's loans — and revoking while a second reader is alive would take a slot out from under it,
/// which is the one property the pool exists to provide. So: the short-lived policy exits, and the
/// answer must still be *no*.
///
/// The final assertion is the one a timeout could not give: the answer is `true` exactly once. A
/// second `true` would revoke the loans of whatever policy started next.
#[test]
fn a_pool_is_only_reclaimed_once_every_policy_that_took_it_has_gone() {
    use skynet_frames::{Geometry, Writer};

    let path = socket_path("departed");
    let (commands, body, mut port) =
        Port::open(&path, 16, 16, skynet_obs::TimeBase::Monotonic).expect("the runtime's rings");
    let mut pool = Writer::create(
        "wrist",
        Geometry {
            slots: 2,
            slot_bytes: 8,
        },
    )
    .expect("a pool");
    pool.publish(1, 1_000, b"pixels").expect("published");

    let policy = |cycles: &str| {
        Command::new(env!("CARGO_BIN_EXE_skynetp"))
            .args([
                "run",
                "--hz",
                "50",
                "--cycles",
                cycles,
                "--channels",
                "1",
                "--backend",
                "trajectory",
                "--setpoints",
                "0:0.25",
                "--ring-socket",
                path.to_str().expect("a utf-8 path"),
            ])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("skynetp starts")
    };

    // Half a second and a tenth of a second at 50 Hz. The gap is what makes the middle assertion
    // meaningful rather than lucky.
    let mut long_lived = policy("25");
    let mut short_lived = policy("5");

    let deadline = Instant::now() + Duration::from_secs(30);
    while port.handed() < 2 && Instant::now() < deadline {
        port.offer(&commands, &body, Some(pool.descriptors()));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(port.handed(), 2, "both policies should have taken the pool");
    assert_eq!(
        port.unidentified(),
        0,
        "the kernel named neither peer, so this machine cannot answer the question this test asks",
    );
    assert!(
        !port.reap_departed(),
        "both policies are running and nothing may be reclaimed",
    );

    short_lived.wait().expect("the short one is ours to reap");
    assert!(
        !port.reap_departed(),
        "one policy exited and the other is still reading. Reclaiming here is the bug the whole \
         mechanism exists to avoid",
    );

    long_lived.wait().expect("the long one is ours to reap");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut reclaimed = false;
    while !reclaimed && Instant::now() < deadline {
        reclaimed = port.reap_departed();
        if !reclaimed {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    assert!(
        reclaimed,
        "both policies exited and were reaped, and the port never said so",
    );
    assert!(
        !port.reap_departed(),
        "the answer is once per generation of readers: a second one would revoke the loans of \
         whatever policy connects next",
    );
}

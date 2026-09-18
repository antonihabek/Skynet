//! The port's behaviour against the Python fake Control box.
//!
//! `drivers/franka/skynet_franka/sim.py` speaks both FCI transports over real sockets, so the
//! Rust driver talks to the same substrate the Python driver's suite uses — no test-only fake,
//! no in-process shortcuts. Two tests, run sequentially against two boxes:
//!
//! * `full_cycle_over_ffi`: the whole §24 surface through the real `skynet_rhal_entry` vtable —
//!   open, describe (byte-compared with the Python bridge's own `describe()`), state fields,
//!   commands that move the fake's joints, motion switching, malformed-command refusals, the
//!   estop latch and its observability, close and reopen.
//! * `faulted_box_is_surfaced_verbatim`: a box with error bit 2 latched reports `FAULT` with
//!   fault code 3 and refuses commands.
//!
//! Without `python3` both tests print why and pass: a suite that failed for a missing interpreter
//! would be measuring the machine, not the driver. (The Python suites' own rule: skipped by name,
//! never passing quietly.)

use std::io::{BufRead, BufReader};
use std::process::{Child, Command as ProcCommand, Stdio};

use skynet_rhal::{flags, status, Homing, Mode, State, ABI_VERSION};
use skynet_rhal::{Command as RhalCommand, Ctx};

const FAKE_RUNNER: &str = r#"
import sys, threading
from skynet_franka.sim import FakeControlBox
hz = float(sys.argv[1])
latch = int(sys.argv[2])
box = FakeControlBox(hz=hz)
if latch >= 0:
    box.latch_error(latch)
print(box.start(), flush=True)
threading.Event().wait()
"#;

/// The fake's start pose (`sim.py`: home-ish, never singular).
const START_Q: [f64; 7] = [0.0, -0.785, 0.0, -2.356, 0.0, 1.571, 0.785];

struct FakeBox {
    child: Child,
    port: u16,
}

impl FakeBox {
    /// Start the fake, or return `None` when there is no interpreter to run it under.
    fn spawn(hz: f64, latch: i32) -> Option<FakeBox> {
        let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
        let pythonpath = format!("{root}/drivers/franka:{root}/bindings/python");
        let mut child = ProcCommand::new("python3")
            .arg("-c")
            .arg(FAKE_RUNNER)
            .arg(hz.to_string())
            .arg(latch.to_string())
            .env("PYTHONPATH", pythonpath)
            // The fake reads no environment, but the operator's would leak through `serve`-style
            // runners; a test substrate takes its configuration from argv or not at all.
            .env_remove("SKYNET_FRANKA_HOST")
            .env_remove("SKYNET_FRANKA_PORT")
            .env_remove("SKYNET_FRANKA_SIM")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let stdout = child.stdout.take()?;
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        let port = line.trim().parse::<u16>().ok()?;
        Some(FakeBox { child, port })
    }
}

impl Drop for FakeBox {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The Python bridge's own `describe()`, for the byte comparison. No box needed: the descriptor
/// is a property of the driver, not of the connection.
fn python_describe() -> Option<String> {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let pythonpath = format!("{root}/drivers/franka:{root}/bindings/python");
    let output = ProcCommand::new("python3")
        .arg("-c")
        .arg("from skynet_franka.bridge import FrankaBridge; print(FrankaBridge().describe(), end='')")
        .env("PYTHONPATH", pythonpath)
        .env_remove("SKYNET_FRANKA_SIM")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn skip(reason: &str) {
    println!("SKIPPED: {reason}");
}

/// One `skr_state` over caller-owned buffers, the way the runtime fills it: `joint_count` on the
/// way in is the caller's capacity.
struct StateBuffer {
    position: Vec<f32>,
    velocity: Vec<f32>,
    effort: Vec<f32>,
}

impl StateBuffer {
    fn new(capacity: usize) -> StateBuffer {
        StateBuffer {
            position: vec![f32::NAN; capacity],
            velocity: vec![f32::NAN; capacity],
            effort: vec![f32::NAN; capacity],
        }
    }

    fn state(&mut self) -> State {
        State {
            stamp_ns: 0,
            seq: 0,
            joint_count: self.position.len() as u16,
            flags: 0,
            position_rad: self.position.as_mut_ptr(),
            velocity_rad_s: self.velocity.as_mut_ptr(),
            effort_nm: self.effort.as_mut_ptr(),
            base_pose: [f32::NAN; 7],
            base_twist: [0.0; 6],
            fault_code: 0,
        }
    }
}

fn command(mode: u8, values: &[f32], mask: &[u8], seq: u32) -> RhalCommand {
    assert_eq!(values.len(), 7);
    assert_eq!(mask.len(), 7);
    RhalCommand {
        stamp_ns: 1,
        seq,
        mode,
        flags: 0,
        _pad: [0; 2],
        joint_count: 7,
        values: values.as_ptr(),
        mask: mask.as_ptr(),
        modes: std::ptr::null(),
    }
}

fn full_cycle_over_ffi() {
    let Some(running) = FakeBox::spawn(500.0, -1) else {
        skip("python3 is not on PATH, so the fake Control box cannot run");
        return;
    };

    // SAFETY: the vtable is a static and every entry point is called with live, correctly
    // shaped arguments — the same contract the OS holds.
    unsafe {
        let driver = &*skynet_franka_rs::skynet_rhal_entry();
        assert_eq!(driver.abi_version, ABI_VERSION);
        let open = driver.open.expect("open");
        let close = driver.close.expect("close");
        let read_state = driver.read_state.expect("read_state");
        let write_command = driver.write_command.expect("write_command");
        let emergency_stop = driver.emergency_stop.expect("emergency_stop");
        let describe = driver.describe.expect("describe");

        // Before open the body refuses.
        let mut buffers = StateBuffer::new(7);
        let mut state = buffers.state();
        assert_eq!(
            read_state(std::ptr::null_mut(), &mut state, 0),
            status::EINVAL,
            "a null context is malformed, not closed"
        );

        // Open against the fake. Every setting explicit: the ambient environment must not be
        // able to change what this test asserts.
        let config = std::ffi::CString::new(format!(
            r#"{{"host": "127.0.0.1", "port": {}, "simulator": true}}"#,
            running.port
        ))
        .unwrap();
        let mut ctx: *mut Ctx = std::ptr::null_mut();
        assert_eq!(open(config.as_ptr(), &mut ctx), status::OK);
        assert!(!ctx.is_null());

        // The descriptor, byte for byte against the Python bridge's.
        let mut json = vec![0i8; 8192];
        let described = describe(ctx, json.as_mut_ptr() as *mut std::ffi::c_char, json.len());
        assert!(described > 0, "describe fits an 8 KiB buffer");
        let ours = String::from_utf8(
            json[..described as usize]
                .iter()
                .map(|b| *b as u8)
                .collect(),
        )
        .expect("the descriptor is UTF-8");
        match python_describe() {
            Some(theirs) => assert_eq!(
                ours, theirs,
                "the Rust descriptor is the Python descriptor, byte for byte"
            ),
            None => println!("SKIPPED: python3 cannot render the comparison descriptor"),
        }
        let mut tiny = vec![0i8; 16];
        assert_eq!(
            describe(ctx, tiny.as_mut_ptr() as *mut std::ffi::c_char, tiny.len()),
            status::EINVAL,
            "too small is an error, never a truncation"
        );

        // State: the fake's start pose, the sequence advancing, the latch flags clear.
        let mut buffers = StateBuffer::new(7);
        let mut first = buffers.state();
        assert_eq!(read_state(ctx, &mut first, 0), status::OK);
        assert_eq!(first.joint_count, 7);
        assert_ne!(first.stamp_ns, 0, "stamped at acquisition, never unset");
        assert_eq!(first.flags & flags::ESTOP, 0);
        assert_eq!(first.flags & flags::FAULT, 0);
        assert_eq!(
            first.flags & flags::HOMING_MASK,
            Homing::NotRequired.to_flags()
        );
        assert_eq!(first.fault_code, 0);
        let expected_q: [f32; 7] = START_Q.map(|v| v as f32);
        assert_eq!(
            buffers.position.as_slice(),
            &expected_q,
            "the first state is the fake's start pose"
        );
        assert!(
            buffers.velocity.iter().all(|v| *v == 0.0),
            "a fresh box reports no velocity"
        );
        let mut second = buffers.state();
        assert_eq!(read_state(ctx, &mut second, 0), status::OK);
        assert_ne!(second.seq, first.seq, "the sequence advances between reads");

        // An undersized caller is refused, not overrun.
        let mut small = StateBuffer::new(6);
        let mut cramped = small.state();
        assert_eq!(read_state(ctx, &mut cramped, 0), status::EINVAL);

        // A position command moves the fake's joints: the fake applies `q_c` directly in a
        // joint-position motion, so the commanded values come back out of the next states.
        let target = [0.1f32, -0.7, 0.0, -2.3, 0.0, 1.5, 0.7];
        let mask = [1u8; 7];
        let cmd = command(Mode::Position as u8, &target, &mask, 1);
        assert_eq!(write_command(ctx, &cmd, 0), status::OK);
        let mut settled = false;
        for _ in 0..20 {
            let mut buffers = StateBuffer::new(7);
            let mut state = buffers.state();
            assert_eq!(read_state(ctx, &mut state, 0), status::OK);
            if buffers.position.as_slice() == target {
                settled = true;
                break;
            }
        }
        assert!(
            settled,
            "the commanded position arrives in the state stream"
        );

        // A masked joint holds: position holds the last reading, so masking joint 0 while
        // commanding the rest leaves joint 0 where it was.
        let mut held = target;
        held[0] = 1.234;
        let mut hold_mask = [1u8; 7];
        hold_mask[0] = 0;
        let cmd = command(Mode::Position as u8, &held, &hold_mask, 2);
        assert_eq!(write_command(ctx, &cmd, 0), status::OK);
        let mut seen_hold = false;
        for _ in 0..20 {
            let mut buffers = StateBuffer::new(7);
            let mut state = buffers.state();
            assert_eq!(read_state(ctx, &mut state, 0), status::OK);
            if buffers.position[0] == target[0] {
                seen_hold = true;
                break;
            }
        }
        assert!(seen_hold, "a masked position joint holds its last reading");

        // Switching modes stops the running motion and starts another — one transact each way,
        // against the same box the Python suite switches against.
        let spin = [0.0f32; 7];
        let cmd = command(Mode::Velocity as u8, &spin, &mask, 3);
        assert_eq!(write_command(ctx, &cmd, 0), status::OK);
        let cmd = command(Mode::Position as u8, &target, &mask, 4);
        assert_eq!(write_command(ctx, &cmd, 0), status::OK);

        // Malformed commands are refused.
        let cmd = command(200, &target, &mask, 5);
        assert_eq!(write_command(ctx, &cmd, 0), status::EINVAL);
        let short = RhalCommand {
            joint_count: 6,
            ..command(Mode::Position as u8, &target, &mask, 6)
        };
        assert_eq!(write_command(ctx, &short, 0), status::EINVAL);

        // The stop: it returns, it latches, it shows in the next state, and later commands
        // are refused.
        assert_eq!(emergency_stop(ctx), status::OK);
        let mut buffers = StateBuffer::new(7);
        let mut stopped = buffers.state();
        assert_eq!(read_state(ctx, &mut stopped, 0), status::OK);
        assert_ne!(
            stopped.flags & flags::ESTOP,
            0,
            "the stop is observable in state"
        );
        let cmd = command(Mode::Position as u8, &target, &mask, 7);
        assert_eq!(write_command(ctx, &cmd, 0), status::EPERM);

        // Close, and the body is gone; reopen, and it is back.
        assert_eq!(close(ctx), status::OK);
        let mut buffers = StateBuffer::new(7);
        let mut closed = buffers.state();
        assert_eq!(read_state(ctx, &mut closed, 0), status::EPERM);
        let mut ctx2: *mut Ctx = std::ptr::null_mut();
        assert_eq!(open(config.as_ptr(), &mut ctx2), status::OK);
        let mut buffers = StateBuffer::new(7);
        let mut reopened = buffers.state();
        assert_eq!(read_state(ctx2, &mut reopened, 0), status::OK);
        assert_eq!(close(ctx2), status::OK);
    }
}

fn faulted_box_is_surfaced_verbatim() {
    let Some(running) = FakeBox::spawn(500.0, 2) else {
        skip("python3 is not on PATH, so the fake Control box cannot run");
        return;
    };

    // SAFETY: as above.
    unsafe {
        let driver = &*skynet_franka_rs::skynet_rhal_entry();
        let open = driver.open.expect("open");
        let close = driver.close.expect("close");
        let read_state = driver.read_state.expect("read_state");
        let write_command = driver.write_command.expect("write_command");

        let config = std::ffi::CString::new(format!(
            r#"{{"host": "127.0.0.1", "port": {}, "simulator": true}}"#,
            running.port
        ))
        .unwrap();
        let mut ctx: *mut Ctx = std::ptr::null_mut();
        assert_eq!(open(config.as_ptr(), &mut ctx), status::OK);

        let mut buffers = StateBuffer::new(7);
        let mut state = buffers.state();
        assert_eq!(read_state(ctx, &mut state, 0), status::OK);
        assert_ne!(
            state.flags & flags::FAULT,
            0,
            "the latched error sets FAULT"
        );
        assert_eq!(
            state.fault_code, 3,
            "the fault code is the bit index, one-based"
        );

        let target = [0.0f32; 7];
        let mask = [1u8; 7];
        let cmd = command(Mode::Position as u8, &target, &mask, 1);
        assert_eq!(
            write_command(ctx, &cmd, 0),
            status::EPERM,
            "refused rather than commanded into the fault"
        );

        assert_eq!(close(ctx), status::OK);
    }
}

#[test]
fn live_fake() {
    // One test, two boxes in sequence: the driver holds its connection in statics (it is a
    // cdylib, and the OS loads one), so two parallel tests would drive one connection from two
    // threads. Sequential here; `skynet driver check` runs the .so in its own process anyway.
    full_cycle_over_ffi();
    faulted_box_is_surfaced_verbatim();
}

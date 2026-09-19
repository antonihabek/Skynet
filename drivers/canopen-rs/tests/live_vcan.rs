//! The port's behaviour against the Python driver's simulated drives, over a kernel CAN bus.
//!
//! `python3 -m skynet_canopen.drives` serves `drivers/canopen`'s own `SimulatedDrive`s on a
//! SocketCAN interface in a process of their own, so this port talks to the substrate the Python
//! driver's `make check-vcan` uses — through the kernel, where the drives answer *after* the
//! master looks (`DRY-COVERAGE-PLAN.md` F71). No Rust drive, no in-process shortcut.
//!
//! Everything goes through the real `skynet_rhal_entry` vtable: open, the descriptor
//! (byte-compared with the Python bridge's own `describe()` for the same drives), reads, commands
//! that move the drives, the refusals, the stop and its latch, close and reopen, a faulted drive,
//! a drive without units, and an axis whose TPDO2 never arrives.
//!
//! It needs `vcan0` and `python3`. `make check` runs it under `drivers/canopen/vcan.sh`, which makes
//! the interface in an unprivileged network namespace, and sets `SKYNET_CANOPEN_REQUIRE_VCAN=1` so
//! that a run asked to measure this and unable to is a failure. A plain `cargo test` without the
//! interface prints why and passes — measuring the machine is not measuring the driver — and says
//! so by name, never quietly.

use std::io::{BufRead, BufReader};
use std::process::{Child, ChildStdin, Command as ProcCommand, Stdio};

use skynet_canopen_rs::bus::SocketCan;
use skynet_rhal::{flags, status, Homing, Mode, State, ABI_VERSION};
use skynet_rhal::{Command as RhalCommand, Ctx, RawDriver};

const INTERFACE: &str = "vcan0";

const CONFIG: &str = r#"{"joints": [{"node": 1, "name": "J1", "role": "right_arm.shoulder.pitch"}, {"node": 2, "name": "J2", "role": "right_arm.elbow.flex"}, {"node": 3, "name": "J3", "role": "right_arm.wrist.roll"}], "interface": "vcan0"}"#;

fn root() -> String {
    concat!(env!("CARGO_MANIFEST_DIR"), "/../..").to_string()
}

fn pythonpath() -> String {
    let root = root();
    format!("{root}/drivers/canopen:{root}/bindings/python")
}

/// Three simulated drives in a process of their own, alive until this is dropped: closing their
/// stdin is their shutdown signal, so a test that panics takes them with it.
struct Drives {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Drives {
    fn spawn(extra: &[&str]) -> Drives {
        let mut child = ProcCommand::new("python3")
            .args(["-m", "skynet_canopen.drives", INTERFACE, "1", "2", "3"])
            .args(extra)
            .env("PYTHONPATH", pythonpath())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("python3 runs the drives");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut line)
            .expect("the drives say something");
        assert_eq!(line.trim(), "ready", "the drives came up");
        let stdin = child.stdin.take();
        Drives { child, stdin }
    }
}

impl Drop for Drives {
    fn drop(&mut self) {
        // EOF is the drives' shutdown signal; the kill is for a drive that ignored it.
        drop(self.stdin.take());
        if self.child.wait().is_err() {
            let _killed = self.child.kill();
        }
    }
}

/// The Python bridge's own `describe()` for the same three drives, on its in-process bus: the
/// descriptor is a function of what the drives report, and the drives here are the same class
/// with the same defaults.
fn python_describe() -> String {
    let script = format!(
        "import json\n\
         from skynet_canopen.bridge import CanOpenBridge\n\
         from skynet_canopen.bus import LoopbackBus\n\
         from skynet_canopen.slave import SimulatedDrive\n\
         bus = LoopbackBus()\n\
         master = bus.endpoint()\n\
         for node in (1, 2, 3):\n\
         \x20   drive = SimulatedDrive(bus.endpoint(), node)\n\
         \x20   bus.attach(drive.poll)\n\
         bridge = CanOpenBridge(master)\n\
         assert bridge.open({CONFIG:?}) == 0\n\
         print(bridge.describe(), end='')\n"
    );
    let output = ProcCommand::new("python3")
        .arg("-c")
        .arg(script)
        .env("PYTHONPATH", pythonpath())
        .output()
        .expect("python3 runs");
    assert!(
        output.status.success(),
        "the Python bridge describes: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8")
}

struct Buffers {
    position: Vec<f32>,
    velocity: Vec<f32>,
    effort: Vec<f32>,
}

impl Buffers {
    fn new(capacity: usize) -> Buffers {
        Buffers {
            position: vec![0.0; capacity],
            velocity: vec![0.0; capacity],
            effort: vec![0.0; capacity],
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
            base_pose: [0.0; 7],
            base_twist: [0.0; 6],
            fault_code: 0,
        }
    }
}

fn command(mode: u8, values: &[f32], mask: &[u8]) -> RhalCommand {
    RhalCommand {
        stamp_ns: 1,
        seq: 1,
        mode,
        flags: 0,
        _pad: [0; 2],
        joint_count: values.len() as u16,
        values: values.as_ptr(),
        mask: mask.as_ptr(),
        modes: std::ptr::null(),
    }
}

fn driver() -> &'static RawDriver {
    // SAFETY: the entry point returns a pointer to a static vtable.
    unsafe { &*skynet_canopen_rs::skynet_rhal_entry() }
}

fn open(config: &str) -> (i32, *mut Ctx) {
    let config = std::ffi::CString::new(config).unwrap();
    let mut ctx: *mut Ctx = std::ptr::null_mut();
    // SAFETY: a NUL-terminated config and a live out-pointer, the OS's contract.
    let rc = unsafe { (driver().open.unwrap())(config.as_ptr(), &mut ctx) };
    (rc, ctx)
}

fn read(ctx: *mut Ctx, buffers: &mut Buffers) -> (i32, State) {
    let mut state = buffers.state();
    // SAFETY: a live context and a state whose arrays are `joint_count` long.
    let rc = unsafe { (driver().read_state.unwrap())(ctx, &mut state, 0) };
    (rc, state)
}

fn write(ctx: *mut Ctx, command: &RhalCommand) -> i32 {
    // SAFETY: a live context and a command whose arrays are `joint_count` long.
    unsafe { (driver().write_command.unwrap())(ctx, command, 0) }
}

fn close(ctx: *mut Ctx) -> i32 {
    // SAFETY: a live context.
    unsafe { (driver().close.unwrap())(ctx) }
}

fn describe(ctx: *mut Ctx) -> String {
    let mut json = vec![0u8; 16384];
    // SAFETY: a live context and a buffer of the length passed.
    let n = unsafe {
        (driver().describe.unwrap())(ctx, json.as_mut_ptr() as *mut std::ffi::c_char, json.len())
    };
    assert!(n > 0, "describe fits 16 KiB");
    String::from_utf8(json[..n as usize].to_vec()).expect("UTF-8")
}

/// Whether this process can reach the interface and run the drives, or why it cannot.
fn reachable() -> Result<(), String> {
    SocketCan::open(INTERFACE)
        .map(drop)
        .map_err(|error| error.to_string())?;
    let python = ProcCommand::new("python3").arg("--version").output();
    match python {
        Ok(output) if output.status.success() => Ok(()),
        _ => Err("python3 is not on PATH, so the simulated drives cannot run".into()),
    }
}

fn a_body_of_three_drives() {
    let drives = Drives::spawn(&[]);
    assert_eq!(driver().abi_version, ABI_VERSION);

    let (rc, ctx) = open(CONFIG);
    assert_eq!(rc, status::OK, "three drives in another process come up");
    assert_eq!(
        describe(ctx),
        python_describe(),
        "the descriptor is the Python bridge's, byte for byte"
    );

    let mut buffers = Buffers::new(3);
    let (rc, first) = read(ctx, &mut buffers);
    assert_eq!(
        rc,
        status::OK,
        "the first read answers: the TPDO2s arrive after the SYNC, and it waits (F71)"
    );
    assert_eq!(first.joint_count, 3);
    assert_ne!(first.stamp_ns, 0, "stamped at acquisition");
    assert_eq!(
        first.flags & flags::HOMING_MASK,
        Homing::NotRequired.to_flags()
    );
    assert!(
        buffers.velocity.iter().all(|v| v.is_nan()),
        "velocity is not measured"
    );
    assert!(buffers.effort.iter().all(|v| v.is_nan()), "nor effort");
    let (_, second) = read(ctx, &mut buffers);
    assert_ne!(second.seq, first.seq, "the sequence advances");

    let mut small = Buffers::new(2);
    assert_eq!(
        read(ctx, &mut small).0,
        status::EINVAL,
        "an undersized caller is refused"
    );

    let values = [0.5f32, -0.25, 0.0];
    let mask = [1u8, 1, 0];
    let cmd = command(Mode::Position as u8, &values, &mask);
    let mut failures = 0;
    let mut last = second;
    for _ in 0..60 {
        assert_eq!(write(ctx, &cmd), status::OK);
        let (rc, state) = read(ctx, &mut buffers);
        if rc != status::OK {
            failures += 1;
        }
        last = state;
    }
    assert_eq!(
        failures, 0,
        "sixty commanded cycles, and every read answered"
    );
    assert!(
        (buffers.position[0] - 0.5).abs() < 1e-3 && (buffers.position[1] + 0.25).abs() < 1e-3,
        "both commanded axes reached their setpoints over the kernel's bus: {:?}",
        buffers.position
    );
    assert!(
        buffers.position[2].abs() < 1e-6,
        "and the held axis stayed put"
    );
    assert_eq!(
        last.flags & flags::DEGRADED,
        0,
        "with no axis missing a SYNC"
    );

    let torque = command(Mode::Torque as u8, &values, &mask);
    assert_eq!(
        write(ctx, &torque),
        status::EINVAL,
        "a mode not wired is refused"
    );
    let short = command(Mode::Position as u8, &values[..2], &mask[..2]);
    assert_eq!(
        write(ctx, &short),
        status::EINVAL,
        "and a wrong joint count"
    );
    let nan = [f32::NAN, 0.0, 0.0];
    assert_eq!(
        write(ctx, &command(Mode::Position as u8, &nan, &[1, 1, 1])),
        status::EINVAL,
        "and a setpoint that is not a number"
    );

    // SAFETY: a live context.
    assert_eq!(
        unsafe { (driver().emergency_stop.unwrap())(ctx) },
        status::OK
    );
    let (_, stopped) = read(ctx, &mut buffers);
    assert_ne!(
        stopped.flags & flags::ESTOP,
        0,
        "the stop is visible in the next state"
    );
    assert_eq!(
        write(ctx, &cmd),
        status::EPERM,
        "and later commands are refused"
    );

    assert_eq!(close(ctx), status::OK);
    assert_eq!(
        read(ctx, &mut buffers).0,
        status::EPERM,
        "a closed body refuses"
    );

    let (rc, ctx) = open(CONFIG);
    assert_eq!(rc, status::OK, "the same driver opens again (F73)");
    assert!(
        describe(ctx).contains("\"dof\":3"),
        "as three axes, not six"
    );
    assert_eq!(write(ctx, &cmd), status::OK, "with the latch its session's");
    assert_eq!(close(ctx), status::OK);
    drop(drives);
}

fn the_refusals() {
    let drives = Drives::spawn(&["--fault", "2"]);
    assert_eq!(
        open(CONFIG).0,
        status::ENODEV,
        "a body with a drive that latched a fault does not come up"
    );
    drop(drives);

    let drives = Drives::spawn(&["--no-units", "1"]);
    assert_eq!(
        open(CONFIG).0,
        status::ENODEV,
        "nor one whose drive will not say how many increments make a turn"
    );
    drop(drives);

    let (rc, _) = open(r#"{"joints": [{"node": 1, "name": "J1"}], "interface": "no-such-can"}"#);
    assert_eq!(
        rc,
        status::EIO,
        "an interface that does not exist is a bus failure"
    );
}

fn an_axis_that_misses_every_sync() {
    let drives = Drives::spawn(&["--lose-tpdo2", "2"]);
    let (rc, ctx) = open(CONFIG);
    assert_eq!(
        rc,
        status::OK,
        "a body whose second axis's TPDO2 is lost opens"
    );
    let mut buffers = Buffers::new(3);
    let (rc, state) = read(ctx, &mut buffers);
    assert_eq!(rc, status::OK, "and reads, because two axes answered");
    assert_ne!(
        state.flags & flags::DEGRADED,
        0,
        "and says it is degraded: one position under this stamp is older than the stamp"
    );
    assert_eq!(close(ctx), status::OK);
    drop(drives);
}

#[test]
fn live_vcan() {
    if let Err(reason) = reachable() {
        if std::env::var("SKYNET_CANOPEN_REQUIRE_VCAN").as_deref() == Ok("1") {
            panic!("the kernel-bus half was asked for and cannot run: {reason}");
        }
        println!("SKIPPED: the kernel-bus half did not run — {reason}");
        return;
    }
    // One test, in sequence: the driver holds its body in statics, as the cdylib the OS loads.
    a_body_of_three_drives();
    the_refusals();
    an_axis_that_misses_every_sync();
}

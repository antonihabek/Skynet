//! The port against the Python slave stack, over a real interface.
//!
//! `python3 -m skynet_ethercat.wire` serves `drivers/ethercat`'s simulated drives on one end of a
//! veth pair, and this master runs on the other over `AF_PACKET` — the arrangement SOEM is
//! cross-checked in. Everything goes through the real vtable: open (scan, configure, Init → Op, CoE
//! reads), the descriptor (equal as a value to the Python bridge's for the same drives), the walk to
//! `operation enabled` and the drives reaching their targets, the refusals, F77's refused command
//! leaving the image alone, the stop and its latch, reopen, a faulted drive, a drive without units,
//! and a configuration that miscounts the ring.
//!
//! `make check` runs it under `check.sh`, which makes `ecat0`/`ecat1` in an unprivileged network
//! namespace and sets `SKYNET_ETHERCAT_REQUIRE=1`, so a run asked to measure this and unable to is a
//! failure. A plain `cargo test` without the pair prints why and passes, by name.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command as ProcCommand, Stdio};

use skynet_ethercat_rs::segment::RawSocket;
use skynet_json::Json;
use skynet_rhal::{flags, status, Mode, State, ABI_VERSION};
use skynet_rhal::{Command as RhalCommand, Ctx, RawDriver};

const MASTER_SIDE: &str = "ecat0";
const RING_SIDE: &str = "ecat1";
const JOINTS: &str = r#"[{"name": "J1", "role": "right_arm.shoulder.pitch"}, {"name": "J2", "role": "right_arm.elbow.flex"}, {"name": "J3", "role": "right_arm.wrist.roll"}]"#;

fn pythonpath() -> String {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    format!("{root}/drivers/ethercat:{root}/drivers/canopen:{root}/bindings/python")
}

/// The Python slave stack on the ring side, until dropped.
struct Ring(Child);

impl Ring {
    fn serve(extra: &[&str]) -> Ring {
        let mut child = ProcCommand::new("python3")
            .args([
                "-m",
                "skynet_ethercat.wire",
                "--interface",
                RING_SIDE,
                "--drives",
                "3",
            ])
            .args(extra)
            .env("PYTHONPATH", pythonpath())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("python3 serves the ring");
        let mut line = String::new();
        BufReader::new(child.stdout.take().expect("stdout"))
            .read_line(&mut line)
            .expect("the ring says something");
        assert!(line.starts_with("serving"), "the ring came up: {line}");
        Ring(child)
    }
}

impl Drop for Ring {
    fn drop(&mut self) {
        // SIGKILL: the server holds nothing a clean exit would release. The wait reaps it, so the
        // next ring on the same interface is the only one answering.
        let _killed = self.0.kill();
        let _reaped = self.0.wait();
    }
}

/// The Python bridge's descriptor for the same three drives, on its own in-process segment.
fn python_descriptor() -> Json {
    let script = format!(
        "from skynet_ethercat.bridge import EtherCatBridge\n\
         bridge = EtherCatBridge()\n\
         assert bridge.open('{{\"sim\": 3, \"joints\": {JOINTS}}}') == 0\n\
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
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Json::parse(std::str::from_utf8(&output.stdout).unwrap()).unwrap()
}

fn driver() -> &'static RawDriver {
    // SAFETY: the entry point returns a pointer to a static vtable.
    unsafe { &*skynet_ethercat_rs::skynet_rhal_entry() }
}

fn open(config: &str) -> (i32, *mut Ctx) {
    let config = std::ffi::CString::new(config).unwrap();
    let mut ctx: *mut Ctx = std::ptr::null_mut();
    // SAFETY: a NUL-terminated config and a live out-pointer.
    let rc = unsafe { (driver().open.unwrap())(config.as_ptr(), &mut ctx) };
    (rc, ctx)
}

fn config(joints: &str) -> String {
    format!(r#"{{"interface": "{MASTER_SIDE}", "joints": {joints}}}"#)
}

struct Reading {
    rc: i32,
    flags: u16,
    fault_code: u32,
    seq: u32,
    q: [f32; 3],
    qd: [f32; 3],
}

fn read(ctx: *mut Ctx) -> Reading {
    let (mut q, mut qd, mut effort) = ([0f32; 3], [0f32; 3], [0f32; 3]);
    let mut state = State {
        stamp_ns: 0,
        seq: 0,
        joint_count: 3,
        flags: 0,
        position_rad: q.as_mut_ptr(),
        velocity_rad_s: qd.as_mut_ptr(),
        effort_nm: effort.as_mut_ptr(),
        base_pose: [0.0; 7],
        base_twist: [0.0; 6],
        fault_code: 0,
    };
    // SAFETY: a live context and a state whose arrays are three long.
    let rc = unsafe { (driver().read_state.unwrap())(ctx, &mut state, 0) };
    Reading {
        rc,
        flags: state.flags,
        fault_code: state.fault_code,
        seq: state.seq,
        q,
        qd,
    }
}

fn write(ctx: *mut Ctx, mode: u8, values: &[f32], mask: &[u8]) -> i32 {
    let command = RhalCommand {
        stamp_ns: 1,
        seq: 1,
        mode,
        flags: 0,
        _pad: [0; 2],
        joint_count: values.len() as u16,
        values: values.as_ptr(),
        mask: mask.as_ptr(),
        modes: std::ptr::null(),
    };
    // SAFETY: a live context and a command whose arrays are `joint_count` long.
    unsafe { (driver().write_command.unwrap())(ctx, &command, 0) }
}

fn close(ctx: *mut Ctx) -> i32 {
    // SAFETY: a live context.
    unsafe { (driver().close.unwrap())(ctx) }
}

fn describe(ctx: *mut Ctx) -> Json {
    let mut buffer = vec![0u8; 16384];
    // SAFETY: a live context and a buffer of the length passed.
    let n = unsafe {
        (driver().describe.unwrap())(
            ctx,
            buffer.as_mut_ptr() as *mut std::ffi::c_char,
            buffer.len(),
        )
    };
    assert!(n > 0);
    Json::parse(std::str::from_utf8(&buffer[..n as usize]).unwrap()).unwrap()
}

/// Walk the drives to `operation enabled` and on to `target`: one CiA 402 step per cycle.
fn drive_to(ctx: *mut Ctx, target: &[f32; 3], mask: &[u8; 3]) -> Reading {
    let mut last = read(ctx);
    for _ in 0..16 {
        assert_eq!(write(ctx, Mode::Position as u8, target, mask), status::OK);
        last = read(ctx);
        assert_eq!(last.rc, status::OK);
    }
    last
}

fn a_ring_of_three() {
    let _ring = Ring::serve(&[]);
    assert_eq!(driver().abi_version, ABI_VERSION);
    let (rc, ctx) = open(&config(JOINTS));
    assert_eq!(rc, status::OK, "the ring comes up to Op over a raw socket");
    assert_eq!(
        describe(ctx),
        python_descriptor(),
        "the descriptor is the Python bridge's, value for value"
    );

    let first = read(ctx);
    assert_eq!(first.rc, status::OK);
    assert!(first.q.iter().all(|v| v.is_finite()));
    assert!(
        first.qd.iter().all(|v| v.is_nan()),
        "velocity is not mapped, so NaN"
    );
    assert_eq!(first.flags & flags::HOMING_MASK, 0, "homing is unreported");
    assert_ne!(read(ctx).seq, first.seq, "the sequence advances");

    let target = [0.3f32, -0.2, 0.1];
    let reached = drive_to(ctx, &target, &[1, 1, 1]);
    assert!(
        reached
            .q
            .iter()
            .zip(target)
            .all(|(q, t)| (q - t).abs() < 1e-3),
        "the drives reach the commanded positions: {:?}",
        reached.q
    );

    assert_eq!(
        write(ctx, Mode::Torque as u8, &target, &[1; 3]),
        status::EPERM,
        "torque"
    );
    assert_eq!(
        write(ctx, 200, &target, &[1; 3]),
        status::EINVAL,
        "an undefined mode"
    );
    assert_eq!(
        write(ctx, 0, &target[..2], &[1; 2]),
        status::EINVAL,
        "a short command"
    );
    // F77: refused for its second drive. Holding every drive afterwards keeps the first where it
    // was, not at the refused command's value.
    let refused = [0.9f32, f32::NAN, 0.1];
    assert_eq!(write(ctx, 0, &refused, &[1; 3]), status::EINVAL);
    assert_eq!(
        write(ctx, 0, &[1.0e6, 0.0, 0.0], &[1; 3]),
        status::EINVAL,
        "past 32 bits"
    );
    let held = drive_to(ctx, &target, &[0, 0, 0]);
    assert!(
        (held.q[0] - target[0]).abs() < 1e-3,
        "a refused command left the first drive's target alone (F77): {:?}",
        held.q
    );

    // SAFETY: a live context.
    assert_eq!(
        unsafe { (driver().emergency_stop.unwrap())(ctx) },
        status::OK
    );
    let stopped = read(ctx);
    assert_eq!(
        stopped.rc,
        status::OK,
        "the stop's frame did not desynchronise the cycle"
    );
    assert_ne!(stopped.flags & flags::ESTOP, 0, "the stop is visible");
    assert_eq!(
        write(ctx, 0, &target, &[1; 3]),
        status::EPERM,
        "and later commands refused"
    );
    assert_eq!(close(ctx), status::OK);
    assert_eq!(read(ctx).rc, status::EPERM);

    let (rc, ctx) = open(&config(JOINTS));
    assert_eq!(rc, status::OK, "the same driver opens again");
    assert_eq!(
        write(ctx, 0, &target, &[0; 3]),
        status::OK,
        "with the latch its session's"
    );
    assert_eq!(close(ctx), status::OK);
}

fn the_refusals() {
    {
        let _ring = Ring::serve(&["--fault", "0"]);
        let (rc, ctx) = open(&config(JOINTS));
        assert_eq!(rc, status::OK);
        let reading = read(ctx);
        assert_ne!(
            reading.flags & flags::FAULT,
            0,
            "a latched fault reaches the flag"
        );
        assert_eq!(reading.fault_code, 1, "and names the drive, one-based");
        assert_eq!(close(ctx), status::OK);
    }
    {
        let _ring = Ring::serve(&["--no-units", "1"]);
        assert_eq!(
            open(&config(JOINTS)).0,
            status::EPROTO,
            "a drive that will not report its units is refused, not assumed"
        );
    }
    {
        let _ring = Ring::serve(&[]);
        let two = r#"[{"name": "J1"}, {"name": "J2"}]"#;
        assert_eq!(
            open(&config(two)).0,
            status::EIO,
            "two joints on a ring of three is refused: addressing is positional"
        );
    }
    let absent = r#"{"interface": "no-such-nic", "joints": [{"name": "J1"}]}"#;
    assert_eq!(
        open(absent).0,
        status::EIO,
        "an interface that does not exist"
    );
}

#[test]
fn live_wire() {
    if let Err(error) = RawSocket::open(MASTER_SIDE) {
        if std::env::var("SKYNET_ETHERCAT_REQUIRE").as_deref() == Ok("1") {
            panic!("the live half was asked for and cannot run: {error}");
        }
        println!("SKIPPED: the live half did not run — {error}");
        return;
    }
    // One test, in sequence: the driver holds its body in statics, as the cdylib the OS loads.
    a_ring_of_three();
    the_refusals();
}

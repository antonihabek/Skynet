//! The port against URSim — Universal Robots' own controller software — through the real vtable.
//!
//! The Python driver's suite measures itself against URSim, and this is the same measurement for
//! the port: open (power, brakes, RTDE, the control program), the descriptor (equal as a value to
//! the Python bridge's, from `vectors.json`), a commanded move of joint 0 with the rest held, the
//! refusals, a refused command that must not move the arm when the next one holds (F74), the stop
//! and its latch, and the same driver opened again after it (F76).
//!
//! It needs a controller: `SKYNET_UR_HOST` names one. Without it the test prints why and passes —
//! the Makefile sets `SKYNET_UR_REQUIRE=1` whenever it is given a host, so a run asked to measure
//! this and unable to is a failure, never a quiet pass.

use std::time::{Duration, Instant};

use skynet_json::Json;
use skynet_rhal::{flags, status, Mode, State, ABI_VERSION};
use skynet_rhal::{Command as RhalCommand, Ctx, RawDriver};

fn driver() -> &'static RawDriver {
    // SAFETY: the entry point returns a pointer to a static vtable.
    unsafe { &*skynet_ur_rtde_rs::skynet_rhal_entry() }
}

fn open(config: &str) -> (i32, *mut Ctx) {
    let config = std::ffi::CString::new(config).unwrap();
    let mut ctx: *mut Ctx = std::ptr::null_mut();
    // SAFETY: a NUL-terminated config and a live out-pointer.
    let rc = unsafe { (driver().open.unwrap())(config.as_ptr(), &mut ctx) };
    (rc, ctx)
}

struct Reading {
    rc: i32,
    flags: u16,
    q: [f32; 6],
    qd: [f32; 6],
    effort: [f32; 6],
}

fn read(ctx: *mut Ctx) -> Reading {
    let mut q = [0.0f32; 6];
    let mut qd = [0.0f32; 6];
    let mut effort = [0.0f32; 6];
    let mut state = State {
        stamp_ns: 0,
        seq: 0,
        joint_count: 6,
        flags: 0,
        position_rad: q.as_mut_ptr(),
        velocity_rad_s: qd.as_mut_ptr(),
        effort_nm: effort.as_mut_ptr(),
        base_pose: [0.0; 7],
        base_twist: [0.0; 6],
        fault_code: 0,
    };
    // SAFETY: a live context and a state whose arrays are six long.
    let rc = unsafe { (driver().read_state.unwrap())(ctx, &mut state, 0) };
    Reading {
        rc,
        flags: state.flags,
        q,
        qd,
        effort,
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
    assert!(n > 0, "describe fits 16 KiB");
    Json::parse(std::str::from_utf8(&buffer[..n as usize]).unwrap()).expect("the descriptor parses")
}

/// The Python bridge's descriptor for an undeclared model, simulated, out of `vectors.json`.
fn python_descriptor() -> Json {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors.json");
    let vectors = Json::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    let case = vectors
        .get("descriptors")
        .and_then(Json::as_arr)
        .unwrap()
        .iter()
        .find(|case| {
            case.get("model") == Some(&Json::Null)
                && case.get("simulator").and_then(Json::as_bool) == Some(true)
        })
        .expect("the no-model, simulated descriptor");
    Json::parse(case.get("descriptor").and_then(Json::as_str).unwrap()).unwrap()
}

/// Open, retrying while the controller still holds the registers for the previous session — the
/// finding `drivers/ur_rtde`'s suite records: RTDE's registers belong to one session at a time and
/// are released a second or two after a socket goes.
fn open_when_released(config: &str) -> (i32, *mut Ctx) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let (rc, ctx) = open(config);
        if rc == status::OK || Instant::now() >= deadline {
            return (rc, ctx);
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Feed `values` at the rate `servoj` covers until joint 0 is within 0.01 rad of `target`, or
/// twenty seconds pass.
fn servo_until(ctx: *mut Ctx, values: &[f32; 6], mask: &[u8; 6], target: f32) -> Reading {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert_eq!(write(ctx, Mode::Position as u8, values, mask), status::OK);
        let reading = read(ctx);
        if (reading.q[0] - target).abs() < 0.01 || Instant::now() >= deadline {
            return reading;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
}

#[test]
fn live_ursim() {
    let host = std::env::var("SKYNET_UR_HOST").unwrap_or_default();
    if host.trim().is_empty() {
        if std::env::var("SKYNET_UR_REQUIRE").as_deref() == Ok("1") {
            panic!("the URSim half was asked for and SKYNET_UR_HOST names no controller");
        }
        println!(
            "SKIPPED: SKYNET_UR_HOST is not set, so nothing above the wire format was measured."
        );
        return;
    }
    assert_eq!(driver().abi_version, ABI_VERSION);
    let config = format!(r#"{{"host": "{host}", "simulator": true}}"#);

    let (rc, ctx) = open_when_released(&config);
    assert_eq!(
        rc,
        status::OK,
        "the arm comes up: powered, brakes released, exchange started"
    );
    assert_eq!(
        describe(ctx),
        python_descriptor(),
        "the descriptor is the Python bridge's, value for value"
    );

    let first = read(ctx);
    assert_eq!(first.rc, status::OK, "read_state answers");
    assert!(
        first.q.iter().all(|v| v.is_finite()),
        "six finite positions"
    );
    assert!(
        first.qd.iter().all(|v| v.is_finite()),
        "velocity is measured"
    );
    assert!(
        first.effort.iter().all(|v| v.is_nan()),
        "torque is not: amps are not newton-metres"
    );

    // Joint 0 by +0.2 rad, the rest held.
    let start = first.q;
    let mut target = start;
    target[0] += 0.20;
    let mask = [1u8, 0, 0, 0, 0, 0];
    let moved = servo_until(ctx, &target, &mask, target[0]);
    assert!(
        (moved.q[0] - target[0]).abs() < 0.01,
        "the arm reaches the setpoint (joint 0: {:+.3} → {:+.3})",
        start[0],
        moved.q[0]
    );
    assert!(
        (moved.q[1] - start[1]).abs() < 0.01,
        "while a held joint stays where it was"
    );

    let position = Mode::Position as u8;
    assert_eq!(
        write(ctx, position, &[0.0; 3], &[1; 3]),
        status::EINVAL,
        "wrong joint count"
    );
    assert_eq!(
        write(ctx, Mode::Torque as u8, &target, &[1; 6]),
        status::EINVAL,
        "torque"
    );
    // F74: refused on joint 1, with a new joint 0 in it. Holding every joint afterwards must hold
    // joint 0 where the arm was told to be, not at the refused command's value.
    let mut refused = target;
    refused[0] += 0.3;
    refused[1] = f32::NAN;
    assert_eq!(
        write(ctx, position, &refused, &[1; 6]),
        status::EINVAL,
        "a NaN setpoint"
    );
    let mut held = read(ctx);
    for _ in 0..125 {
        assert_eq!(write(ctx, position, &target, &[0; 6]), status::OK);
        held = read(ctx);
        std::thread::sleep(Duration::from_millis(8));
    }
    assert!(
        (held.q[0] - target[0]).abs() < 0.01,
        "a full second of holds after a refused command stays at {:+.3}, not {:+.3} (F74): read \
         {:+.3}",
        target[0],
        refused[0],
        held.q[0]
    );

    // SAFETY: a live context.
    assert_eq!(
        unsafe { (driver().emergency_stop.unwrap())(ctx) },
        status::OK
    );
    let stopped = read(ctx);
    assert_ne!(
        stopped.flags & flags::ESTOP,
        0,
        "the stop is visible in the next state"
    );
    assert_eq!(
        write(ctx, position, &target, &mask),
        status::EPERM,
        "and later commands refused"
    );
    assert_eq!(close(ctx), status::OK);
    assert_eq!(read(ctx).rc, status::EPERM, "a closed arm refuses");

    // F76: the same driver, opened again after the stop, commands again.
    let (rc, ctx) = open_when_released(&config);
    assert_eq!(rc, status::OK, "the same driver opens again after the stop");
    let here = read(ctx).q;
    assert_eq!(
        write(ctx, position, &here, &[0; 6]),
        status::OK,
        "and commands: the latch belonged to the session it was asserted in (F76)"
    );
    assert_eq!(close(ctx), status::OK);

    assert_eq!(
        open("{}").0,
        status::ENODEV,
        "a configuration with no host is refused"
    );
    assert_eq!(
        open("not json").0,
        status::EINVAL,
        "and one that is not JSON"
    );
}

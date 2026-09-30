//! The port's stop, release and close, through the real vtable on a kernel CAN bus, against scripted drives that keep a log
//! (DRY-COVERAGE-PLAN.md 12.C.3).
//!
//! `live_vcan` shows the stop returns, latches, and is visible in the next state. It cannot say *what the stop put on the bus*:
//! which controlword, which target, in what order, and whether the bus was dropped to pre-operational; what `release_stop`
//! sends to bring it back; what `close` leaves behind; what happens when the frames cannot be sent. The seeded-defect
//! campaign (`tools/seed-defects/`) found that fourteen of this crate's realistic defects on those paths left `cargo test`
//! green — a quick-stop that carries a target of zero, a release that clears the latch and never restarts the bus, a close that
//! leaves the stop path writing to a bus that was closed.
//!
//! The drives are `common::Model`, each serving the interface from a thread and logging every frame it was sent, so a test
//! reads what the master wrote. **They agree with whoever scripted them** — F15's point, and the reason `live_vcan` and the
//! Python drives exist.
//!
//! It needs `vcan0`. `make check` runs it under `drivers/canopen/vcan.sh`, which makes the interface in an unprivileged network
//! namespace and sets `SKYNET_CANOPEN_REQUIRE_VCAN=1` so a run asked to measure this and unable to is a failure; a plain
//! `cargo test` without the interface prints why and passes, by name, as `live_vcan` does.

mod common;

use std::process::Command as Process;
use std::sync::{Mutex, MutexGuard};

use common::{serve_on, Cia, Model, Serving};
use skynet_canopen_rs::bus::SocketCan;
use skynet_canopen_rs::frames;
use skynet_rhal::{flags, status, Command as RhalCommand, Ctx, Mode, RawDriver, State};

const INTERFACE: &str = "vcan0";
const CONFIG: &str =
    r#"{"joints": [{"node": 1, "name": "J1"}, {"node": 2, "name": "J2"}], "interface": "vcan0"}"#;

/// Increments in one turn of the shaft behind the model's 131072-increment encoder and 50:1 gear.
const INCREMENTS_PER_TURN: f64 = 131072.0 * 50.0;

fn increments(radians: f64) -> i32 {
    (radians / std::f64::consts::TAU * INCREMENTS_PER_TURN).round() as i32
}

fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The bus, or why this run cannot have one: skipped by name, never quietly, unless the run was asked to require it.
fn bus() -> bool {
    match SocketCan::open(INTERFACE) {
        Ok(_) => true,
        Err(error) => {
            if std::env::var("SKYNET_CANOPEN_REQUIRE_VCAN").as_deref() == Ok("1") {
                panic!("the kernel-bus half was asked for and cannot run: {error}");
            }
            println!("SKIPPED: the kernel-bus half did not run — {error}");
            false
        }
    }
}

fn driver() -> &'static RawDriver {
    // SAFETY: the entry point returns a pointer to a static vtable.
    unsafe { &*skynet_canopen_rs::skynet_rhal_entry() }
}

fn open() -> (i32, *mut Ctx) {
    let config = std::ffi::CString::new(CONFIG).unwrap();
    let mut ctx: *mut Ctx = std::ptr::null_mut();
    // SAFETY: a NUL-terminated config and a live out-pointer.
    let rc = unsafe { (driver().open.unwrap())(config.as_ptr(), &mut ctx) };
    (rc, ctx)
}

fn opened() -> (Serving, *mut Ctx) {
    let drives = serve_on(INTERFACE, Model::with(&[1, 2]));
    let (rc, ctx) = open();
    assert_eq!(
        rc,
        status::OK,
        "the scripted drives are brought up over the kernel bus"
    );
    (drives, ctx)
}

struct Reading {
    rc: i32,
    flags: u16,
    velocity: Vec<f32>,
    effort: Vec<f32>,
    base_pose: [f32; 7],
}

fn read(ctx: *mut Ctx) -> Reading {
    let mut position = vec![0.0f32; 2];
    let mut velocity = vec![0.0f32; 2];
    let mut effort = vec![0.0f32; 2];
    let mut state = State {
        stamp_ns: 0,
        seq: 0,
        joint_count: 2,
        flags: 0,
        position_rad: position.as_mut_ptr(),
        velocity_rad_s: velocity.as_mut_ptr(),
        effort_nm: effort.as_mut_ptr(),
        base_pose: [0.0; 7],
        base_twist: [0.0; 6],
        fault_code: 0,
    };
    // SAFETY: a live context and a state whose arrays are two long.
    let rc = unsafe { (driver().read_state.unwrap())(ctx, &mut state, 0) };
    Reading {
        rc,
        flags: state.flags,
        velocity,
        effort,
        base_pose: state.base_pose,
    }
}

fn write(ctx: *mut Ctx, values: &[f32; 2]) -> i32 {
    let mask = [1u8; 2];
    let command = RhalCommand {
        stamp_ns: 1,
        seq: 1,
        mode: Mode::Position as u8,
        flags: 0,
        _pad: [0; 2],
        joint_count: 2,
        values: values.as_ptr(),
        mask: mask.as_ptr(),
        modes: std::ptr::null(),
    };
    // SAFETY: a live context and a command whose arrays are two long.
    unsafe { (driver().write_command.unwrap())(ctx, &command, 0) }
}

fn stop(ctx: *mut Ctx) -> i32 {
    // SAFETY: a live context.
    unsafe { (driver().emergency_stop.unwrap())(ctx) }
}

fn release(ctx: *mut Ctx) -> i32 {
    // SAFETY: a live context.
    unsafe { (driver().release_stop.unwrap())(ctx) }
}

fn close(ctx: *mut Ctx) -> i32 {
    // SAFETY: a live context.
    unsafe { (driver().close.unwrap())(ctx) }
}

const TARGETS: [f32; 2] = [0.5, -0.25];

/// Take the interface down and put it back, whatever the test does in between.
struct InterfaceDown;

impl InterfaceDown {
    fn new() -> InterfaceDown {
        let status = Process::new("ip")
            .args(["link", "set", INTERFACE, "down"])
            .status();
        assert!(
            status.map(|s| s.success()).unwrap_or(false),
            "taking {INTERFACE} down needs the privileges of the namespace `drivers/canopen/vcan.sh` makes (and `ip`): run under it, on a machine with no {INTERFACE} of its own"
        );
        InterfaceDown
    }
}

impl Drop for InterfaceDown {
    fn drop(&mut self) {
        let _ = Process::new("ip")
            .args(["link", "set", INTERFACE, "up"])
            .status();
    }
}

#[test]
fn the_stop_quick_stops_every_axis_at_the_target_it_holds_and_then_drops_the_bus_to_pre_operational(
) {
    let _one = one_at_a_time();
    if !bus() {
        return;
    }
    let (drives, ctx) = opened();
    assert_eq!(write(ctx, &TARGETS), status::OK);
    let (t1, t2) = (increments(TARGETS[0] as f64), increments(TARGETS[1] as f64));
    assert_eq!(stop(ctx), status::OK, "emergency_stop returns");
    drives.wait_for("the bus dropped to pre-operational", |m| {
        m.nmt().last() == Some(&(frames::NMT_PRE_OPERATIONAL, 0))
    });
    drives.with(|m| {
        let quick = |node: u8| m.log.iter().position(|f| f.id == frames::rpdo2_id(node) && u16::from_le_bytes([f.data[0], f.data[1]]) == 0x0002).unwrap_or_else(|| panic!("node {node} was never sent a quick-stop"));
        let last = |node: u8| m.rpdos(node).last().copied().unwrap();
        let (q1, q2) = (quick(1), quick(2));
        let preop = m.log.iter().position(|f| f.id == frames::NMT_ID && f.data[0] == frames::NMT_PRE_OPERATIONAL).unwrap();
        assert!(q1 < preop && q2 < preop, "every axis is quick-stopped before the bus is dropped, not after: {q1} {q2} then {preop}");
        let within = |value: i32, want: i32| (value - want).abs() <= 1;
        assert!(last(1).0 == 0x0002 && within(last(1).1, t1), "node 1's quick-stop carries the target it was holding ({t1}), and not zero: {:?}", last(1));
        assert!(last(2).0 == 0x0002 && within(last(2).1, t2), "and so does node 2's ({t2}): {:?}", last(2));
        assert_ne!(t1, t2);
    });
    drives.wait_for("the drives in quick stop", |m| {
        m.drives.iter().all(|d| d.state == Cia::QuickStopActive)
    });
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn release_returns_the_bus_to_operational_clears_the_latch_and_lets_the_drives_be_enabled_again() {
    let _one = one_at_a_time();
    if !bus() {
        return;
    }
    let (drives, ctx) = opened();
    assert_eq!(write(ctx, &TARGETS), status::OK);
    assert_eq!(stop(ctx), status::OK);
    assert_ne!(
        read(ctx).flags & flags::ESTOP,
        0,
        "the next state says the stop is asserted"
    );
    assert_eq!(
        write(ctx, &TARGETS),
        status::EPERM,
        "and a command after it is refused"
    );
    let nmt_before = drives.settled(|m| m.nmt().len());

    assert_eq!(release(ctx), status::OK, "release_stop returns");
    drives.wait_for("NMT start, broadcast", |m| {
        m.nmt().len() > nmt_before && m.nmt().last() == Some(&(frames::NMT_START, 0))
    });
    assert_eq!(
        read(ctx).flags & flags::ESTOP,
        0,
        "the state no longer says stopped"
    );
    assert_eq!(
        write(ctx, &TARGETS),
        status::OK,
        "commands are accepted again"
    );
    drives.wait_for("the drives walked back from quick stop", |m| {
        m.drives.iter().all(|d| d.state == Cia::OperationEnabled)
    });
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn a_stop_whose_frames_could_not_be_sent_does_not_say_they_were() {
    let _one = one_at_a_time();
    if !bus() {
        return;
    }
    let (_drives, ctx) = opened();
    {
        let _down = InterfaceDown::new();
        assert_eq!(stop(ctx), status::EIO, "with the interface down no frame can be written, and saying OK would claim a stop that was not sent");
        assert_eq!(
            write(ctx, &TARGETS),
            status::EPERM,
            "though the latch is set, so the commands that would follow are refused"
        );
    }
    assert_eq!(release(ctx), status::OK);
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn a_close_leaves_the_stop_path_nothing_to_write_to_and_a_reopen_lets_go_of_the_first_body() {
    let _one = one_at_a_time();
    if !bus() {
        return;
    }
    let (drives, ctx) = opened();
    assert_eq!(write(ctx, &TARGETS), status::OK);
    assert_eq!(close(ctx), status::OK);
    drives.wait_for("the close's disable", |m| {
        m.get(1).controlwords.last() == Some(&0x0000)
            && m.nmt().last() == Some(&(frames::NMT_PRE_OPERATIONAL, 0))
    });
    let logged = drives.settled(|m| m.log.len());
    assert_eq!(
        stop(ctx),
        status::OK,
        "a stop after close has nothing to stop and says so"
    );
    assert_eq!(
        drives.settled(|m| m.log.len()),
        logged,
        "and writes nothing to a bus that was closed"
    );

    // A second open is a new session: the first body is disabled, not left enabled behind it.
    let (rc, _first) = open();
    assert_eq!(rc, status::OK);
    let (rc2, ctx2) = open();
    assert_eq!(rc2, status::OK);
    let words = drives.settled(|m| m.get(1).controlwords.clone());
    let enable = [0x0006, 0x0007, 0x000F];
    let second_round = words.windows(3).filter(|w| *w == enable).count();
    assert!(
        second_round >= 3,
        "three enables in all (the first body, the reopen, and its replacement): {words:?}"
    );
    let disables = words.iter().filter(|w| **w == 0x0000).count();
    assert!(disables >= 2, "the body that was replaced was disabled before its replacement was enabled, and the one closed before was too: {words:?}");
    assert_eq!(close(ctx2), status::OK);
}

#[test]
fn the_state_claims_no_velocity_no_torque_and_no_pose_and_a_description_needs_room_for_its_terminator(
) {
    let _one = one_at_a_time();
    if !bus() {
        return;
    }
    let (_drives, ctx) = opened();
    let state = read(ctx);
    assert_eq!(state.rc, status::OK);
    assert!(
        state.velocity.iter().all(|v| v.is_nan()) && state.effort.iter().all(|v| v.is_nan()),
        "velocity and torque are not in TPDO2 and are NaN, never zero"
    );
    assert!(
        state.base_pose.iter().all(|v| v.is_nan()),
        "a fixed base has no pose: NaN and not the origin"
    );
    let describe = |len: usize| -> i32 {
        let mut buffer = vec![0u8; len.max(1)];
        // SAFETY: a live context and a buffer of the length passed.
        unsafe {
            (driver().describe.unwrap())(ctx, buffer.as_mut_ptr() as *mut std::ffi::c_char, len)
        }
    };
    let length = describe(1 << 14);
    assert!(length > 100);
    assert_eq!(describe(length as usize), status::EINVAL, "a buffer exactly as long as the text has no room for the terminator, and is refused rather than truncated");
    assert_eq!(describe(length as usize + 1), length);
    assert_eq!(close(ctx), status::OK);
}

//! The port's stop, release, close, reopen and state, through the real vtable over a kernel interface, against a scripted ring that
//! keeps a log (DRY-COVERAGE-PLAN.md 12.C.3).
//!
//! `live_wire` shows the stop returns, latches and is visible in the next state. It cannot say *what the stop put on the ring*:
//! which controlword, which target, at which index, and whether it was sent at all, since the next cycle holds quick stop
//! whether or not the stop's own frame left; what `close` leaves behind; what a second `open` does to the first body; what
//! happens when the frame cannot be written; what a fixed base's pose reads as. The seeded-defect campaign
//! (`tools/seed-defects/`) found that eleven of this crate's realistic defects on those paths left `make check` green — a stop
//! frame never sent, a stop that cannot be written and says OK, a release that does not clear the latch, a close that leaves the
//! stop path writing to a socket the body let go of, a configuration with no interface that defaults to `eth0`.
//!
//! The ring is `common::Ring`, serving one end of a veth pair from a thread and logging every frame it was sent, so a test reads
//! what the master wrote. **It agrees with whoever scripted it** — F15's point, and the reason `live_wire` and the Python slave
//! stack exist.
//!
//! It needs the pair `ecat0`/`ecat1` and `ip`. `make check` runs it under `check.sh`, which makes the pair in an unprivileged
//! network namespace and sets `SKYNET_ETHERCAT_REQUIRE=1` so a run asked to measure this and unable to is a failure; a plain
//! `cargo test` without the pair prints why and passes, by name, as `live_wire` does.

mod common;

use std::process::Command as Process;
use std::sync::{Mutex, MutexGuard};

use common::*;
use skynet_ethercat_rs::frames::Datagram;
use skynet_ethercat_rs::segment::RawSocket;
use skynet_rhal::{flags, status, Command as RhalCommand, Ctx, Mode, RawDriver, State};

const MASTER_SIDE: &str = "ecat0";
const RING_SIDE: &str = "ecat1";
const CONFIG: &str = r#"{"interface": "ecat0", "joints": [{"name": "J1"}, {"name": "J2"}]}"#;

/// Increments in one turn of the shaft behind the model's 131072-increment encoder and 50:1 gear.
const INCREMENTS_PER_TURN: f64 = 131072.0 * 50.0;

fn increments(radians: f64) -> i32 {
    (radians / std::f64::consts::TAU * INCREMENTS_PER_TURN).round() as i32
}

fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The pair, or why this run cannot have one: skipped by name, never quietly, unless the run was asked to require it.
fn pair() -> bool {
    match RawSocket::open(MASTER_SIDE) {
        Ok(_) => true,
        Err(error) => {
            if std::env::var("SKYNET_ETHERCAT_REQUIRE").as_deref() == Ok("1") {
                panic!("the live half was asked for and cannot run: {error}");
            }
            println!("SKIPPED: the live half did not run — {error}");
            false
        }
    }
}

fn driver() -> &'static RawDriver {
    // SAFETY: the entry point returns a pointer to a static vtable.
    unsafe { &*skynet_ethercat_rs::skynet_rhal_entry() }
}

fn open_with(config: &str) -> (i32, *mut Ctx) {
    let config = std::ffi::CString::new(config).unwrap();
    let mut ctx: *mut Ctx = std::ptr::null_mut();
    // SAFETY: a NUL-terminated config and a live out-pointer.
    let rc = unsafe { (driver().open.unwrap())(config.as_ptr(), &mut ctx) };
    (rc, ctx)
}

fn opened() -> (Serving, *mut Ctx) {
    let ring = serve_on(RING_SIDE, Ring::of(2));
    let (rc, ctx) = open_with(CONFIG);
    assert_eq!(
        rc,
        status::OK,
        "the scripted ring is brought up over the interface"
    );
    (ring, ctx)
}

struct Reading {
    rc: i32,
    flags: u16,
    velocity: Vec<f32>,
    effort: Vec<f32>,
    base_pose: [f32; 7],
    base_twist: [f32; 6],
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
        base_twist: [1.0; 6],
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
        base_twist: state.base_twist,
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

/// Walk both drives to `operation enabled` and on to `TARGETS`: one CiA 402 step per cycle.
fn drive_to_targets(ctx: *mut Ctx) {
    assert_eq!(read(ctx).rc, status::OK);
    for _ in 0..8 {
        assert_eq!(write(ctx, &TARGETS), status::OK);
        assert_eq!(read(ctx).rc, status::OK);
    }
}

/// Every controlword and target a frame carried to the two drives, by the layout the master configured.
fn outputs_of(image: &[u8]) -> [(u16, i32); 2] {
    let one = |at: usize| {
        (
            u16::from_le_bytes([image[at], image[at + 1]]),
            i32::from_le_bytes([image[at + 2], image[at + 3], image[at + 4], image[at + 5]]),
        )
    };
    [one(0), one(6)]
}

/// Take an interface down and put it back, whatever the test does in between.
struct InterfaceDown(&'static str);

impl InterfaceDown {
    fn new(name: &'static str) -> InterfaceDown {
        let status = Process::new("ip")
            .args(["link", "set", name, "down"])
            .status();
        assert!(
            status.map(|s| s.success()).unwrap_or(false),
            "taking {name} down needs the privileges of the namespace `check.sh` makes (and `ip`): run under it, on a machine with no {name} of its own"
        );
        InterfaceDown(name)
    }
}

impl Drop for InterfaceDown {
    fn drop(&mut self) {
        let _ = Process::new("ip")
            .args(["link", "set", self.0, "up"])
            .status();
    }
}

#[test]
fn the_stop_puts_one_quick_stop_frame_on_the_ring_at_index_zero_with_the_targets_it_holds() {
    let _one = one_at_a_time();
    if !pair() {
        return;
    }
    let (ring, ctx) = opened();
    drive_to_targets(ctx);
    let before = ring.with(|r| r.log.len());
    assert_eq!(stop(ctx), status::OK, "emergency_stop returns");
    ring.wait_for("the stop's own frame, an LRW with index 0", |r| {
        r.log[before..]
            .iter()
            .flat_map(|f| f.datagrams.iter())
            .any(|d| d.command == LRW && d.index == 0)
    });
    ring.with(|r| {
        let frames: Vec<&Vec<u8>> = r.log[before..]
            .iter()
            .flat_map(|f| f.datagrams.iter())
            .filter(|d| d.command == LRW && d.index == 0)
            .map(|d| &d.data)
            .collect();
        assert_eq!(frames.len(), 1, "one frame, not one a cycle");
        let [(w1, t1), (w2, t2)] = outputs_of(frames[0]);
        assert_eq!(
            (w1, w2),
            (QUICK_STOP, QUICK_STOP),
            "every drive's controlword at quick stop"
        );
        assert!(
            (t1 - increments(TARGETS[0] as f64)).abs() <= 1,
            "drive 1 holds the target it was at, not zero: {t1}"
        );
        assert!(
            (t2 - increments(TARGETS[1] as f64)).abs() <= 1,
            "and so does drive 2: {t2}"
        );
        assert_ne!(t1, t2);
    });
    let stopped = read(ctx);
    assert_eq!(
        stopped.rc,
        status::OK,
        "the stop's own answer did not desynchronise the next cycle"
    );
    assert_ne!(stopped.flags & flags::ESTOP, 0);
    ring.wait_for("the drives in quick stop", |r| {
        r.slaves.iter().all(|s| s.state == Cia::QuickStopActive)
    });
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn a_stop_whose_frame_could_not_be_written_does_not_say_it_was() {
    let _one = one_at_a_time();
    if !pair() {
        return;
    }
    let (_ring, ctx) = opened();
    {
        let _down = InterfaceDown::new(MASTER_SIDE);
        assert_eq!(
            stop(ctx),
            status::EIO,
            "with the interface down no frame can be written, and saying OK would claim a stop that was not sent"
        );
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
fn release_clears_the_latch_and_the_drives_walk_back_to_operation_enabled() {
    let _one = one_at_a_time();
    if !pair() {
        return;
    }
    let (ring, ctx) = opened();
    drive_to_targets(ctx);
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
    assert_eq!(release(ctx), status::OK, "release_stop returns");
    assert_eq!(release(ctx), status::OK, "and is idempotent");
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
    for _ in 0..8 {
        assert_eq!(write(ctx, &TARGETS), status::OK);
        assert_eq!(read(ctx).rc, status::OK);
    }
    ring.with(|r| {
        assert!(
            r.slaves.iter().all(|s| s.state == Cia::OperationEnabled),
            "the release sent nothing, and the next cyclic frames' enable controlwords walked each drive back"
        );
    });
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn the_state_claims_no_velocity_no_torque_and_no_pose() {
    let _one = one_at_a_time();
    if !pair() {
        return;
    }
    let (_ring, ctx) = opened();
    let state = read(ctx);
    assert_eq!(state.rc, status::OK);
    assert!(
        state.velocity.iter().all(|v| v.is_nan()) && state.effort.iter().all(|v| v.is_nan()),
        "velocity and torque are not in this PDO map and are NaN, never zero"
    );
    assert!(
        state.base_pose.iter().all(|v| v.is_nan()),
        "a fixed base has no pose: NaN and not the origin"
    );
    assert_eq!(
        state.base_twist, [0.0; 6],
        "and no twist, which is a measured zero"
    );
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn a_ring_that_goes_silent_fails_the_read_and_a_ring_that_comes_back_is_read_again() {
    let _one = one_at_a_time();
    if !pair() {
        return;
    }
    let (ring, ctx) = opened();
    assert_eq!(read(ctx).rc, status::OK);
    ring.with(|r| r.deaf = true);
    assert_eq!(
        read(ctx).rc,
        status::EIO,
        "a frame the ring does not return is an I/O error, not whatever the arrays held"
    );
    ring.with(|r| r.deaf = false);
    assert_eq!(
        read(ctx).rc,
        status::OK,
        "and a ring that answers again is read again"
    );
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn a_close_quick_stops_the_drives_and_leaves_the_stop_path_nothing_to_write_to() {
    let _one = one_at_a_time();
    if !pair() {
        return;
    }
    let (ring, ctx) = opened();
    drive_to_targets(ctx);
    assert_eq!(close(ctx), status::OK);
    ring.wait_for("the close's quick stop", |r| {
        r.slaves.iter().all(|s| s.state == Cia::QuickStopActive)
    });
    assert_eq!(
        read(ctx).rc,
        status::EPERM,
        "a read after close has no body"
    );
    let logged = ring.settled(|r| r.log.len());
    assert_eq!(
        stop(ctx),
        status::OK,
        "a stop after close has nothing to stop and says so"
    );
    assert_eq!(
        ring.settled(|r| r.log.len()),
        logged,
        "and writes nothing to an interface the body let go of"
    );
}

#[test]
fn a_second_open_quick_stops_the_first_body_before_it_brings_up_the_second() {
    let _one = one_at_a_time();
    if !pair() {
        return;
    }
    let (ring, first) = opened();
    drive_to_targets(first);
    let before = ring.settled(|r| r.log.len());
    let (rc, second) = open_with(CONFIG);
    assert_eq!(rc, status::OK, "the same driver opens again");
    ring.with(|r| {
        let fresh: Vec<&Datagram> = r.log[before..]
            .iter()
            .flat_map(|f| f.datagrams.iter())
            .collect();
        let quick_stop = fresh
            .iter()
            .position(|d| {
                d.command == LRW
                    && outputs_of(&d.data)
                        .iter()
                        .all(|(word, _)| *word == QUICK_STOP)
            })
            .expect("the body that was replaced was quick-stopped, not abandoned enabled");
        let bring_up = fresh
            .iter()
            .position(|d| d.command == BRD)
            .expect("the second body's scan");
        assert!(
            quick_stop < bring_up,
            "and before the second body's bring-up: {quick_stop} then {bring_up}"
        );
    });
    assert_eq!(close(second), status::OK);
}

/// An interface called `eth0`, with the ring on the other end of it, for as long as the test holds it.
struct Eth0;

impl Eth0 {
    fn new() -> Eth0 {
        let run = |args: &[&str]| {
            Process::new("ip")
                .args(args)
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        };
        assert!(
            run(&["link", "add", "eth0", "type", "veth", "peer", "name", "eth1"])
                && run(&["link", "set", "eth0", "up"])
                && run(&["link", "set", "eth1", "up"]),
            "making a pair called eth0/eth1 needs the privileges of the namespace `check.sh` makes (and `ip`), and a machine with no eth0 of its own"
        );
        Eth0
    }
}

impl Drop for Eth0 {
    fn drop(&mut self) {
        let _ = Process::new("ip").args(["link", "del", "eth0"]).status();
    }
}

#[test]
fn a_configuration_that_names_no_interface_is_refused_and_not_sent_to_eth0() {
    let _one = one_at_a_time();
    if !pair() {
        return;
    }
    std::env::remove_var("SKYNET_ETHERCAT_INTERFACE");
    let _eth0 = Eth0::new();
    let ring = serve_on("eth1", Ring::of(2));
    let (rc, _) = open_with(r#"{"joints": [{"name": "J1"}, {"name": "J2"}]}"#);
    assert_eq!(
        rc,
        status::EIO,
        "no `interface` and no environment variable: the driver does not guess which NIC carries a segment"
    );
    assert!(
        ring.settled(|r| r.log.is_empty()),
        "and sent no raw frame onto an interface nobody named, though eth0 exists and a ring answers behind it"
    );
}

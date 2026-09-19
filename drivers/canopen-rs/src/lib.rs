//! skynet-canopen-rs — CiA 402 drives on a CANopen bus, in Rust.
//!
//! A systems port of `drivers/canopen`'s Python driver (6.6 in `DRY-COVERAGE-PLAN.md`), which is
//! the executable specification: the framing and the profile are held to golden vectors generated
//! from its own modules (`tests/vectors.rs`), and the behaviour to its simulated drives, in their
//! own process, over a kernel CAN bus (`tests/live_vcan.rs`) — descriptor byte-identical with the
//! Python bridge's.
//!
//! ```text
//! cargo build --release
//! sh ../canopen/vcan.sh skynet driver check target/release/libskynet_canopen_rs.so \
//!     --config '{"joints": [{"node": 1, "name": "J1"}]}'
//! ```
//!
//! # Four rules (§4, §89)
//!
//! 1. `read_state` and `write_command` are wait-free and allocate nothing.
//! 2. `emergency_stop` must work from a signal handler and from a process that is dying.
//! 3. `describe` is the source of truth for the body.
//! 4. `read_sensor` is called on the real-time thread unless a sensor says otherwise.
//!
//! Rule 1 is the one this port does not meet, and it says so as `franka-rs` does: `read_state`
//! sends a SYNC and waits up to the SYNC window for the drives' answers, behind the connection's
//! mutex — the Python driver's structure, translated. A CANopen read *is* a wait on a bus; making
//! it wait-free means a thread that owns the SYNC cycle and a ring the RHAL reads, which is the
//! follow-up a systems language permits and no machine in this phase can measure. Neither
//! function allocates. Rule 2 is met in full: the stop is an atomic latch, then one quick-stop
//! RPDO2 per axis and one NMT frame, built on the stack and written on a descriptor duplicated at
//! open and held outside the connection lock, attempted with `try_lock` and never waited for.
//!
//! # Why this crate uses `unsafe`, for the CI allow-list (§23.1)
//!
//! Two boundaries that cannot be written without it. The RHAL: `skynet_rhal_entry` and six
//! `extern "C"` functions dereferencing caller-owned state and command buffers, every pointer
//! null-checked and every array bounded by the count the caller declares. And SocketCAN, in
//! `bus`: `socket`, `if_nametoindex`, `bind`, `poll` and `recv` declared rather than depended on,
//! because the workspace has no libc and one bus does not justify one. `frames`, `cia402`,
//! `pyjson` and `bridge` are safe code behind the two.

pub mod bridge;
pub mod bus;
pub mod cia402;
pub mod frames;
pub mod pyjson;

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use skynet_rhal::{status, Command, Ctx, Frame, Mode, RawDriver, State, ABI_VERSION};

use bridge::{Bridge, Config};
use bus::SocketCan;
use cia402::Command as Cia402Command;

/// The body, behind the mutex the module header owns up to. `None` before open and after close.
fn body() -> &'static Mutex<Option<Bridge<SocketCan>>> {
    static BODY: OnceLock<Mutex<Option<Bridge<SocketCan>>>> = OnceLock::new();
    BODY.get_or_init(|| Mutex::new(None))
}

static OPENED: AtomicBool = AtomicBool::new(false);
static ESTOPPED: AtomicBool = AtomicBool::new(false);

/// What the stop needs, held outside the body's lock: a second descriptor on the bus, and the
/// node ids in joint order — both made at open, so the stop allocates nothing.
struct StopPath {
    socket: SocketCan,
    nodes: Vec<u8>,
}

fn stop_path() -> &'static Mutex<Option<StopPath>> {
    static STOP: OnceLock<Mutex<Option<StopPath>>> = OnceLock::new();
    STOP.get_or_init(|| Mutex::new(None))
}

/// Each node's current target in increments, indexed by node id: the quick-stop RPDO2 carries it,
/// as `bridge.py`'s does, and the stop cannot take the body's lock to read it. Written after every
/// accepted command.
static TARGETS: [AtomicI32; 128] = [const { AtomicI32::new(0) }; 128];

/// Lock the body, or refuse when the lock is poisoned: a thread died holding the bus, the state
/// behind it is suspect, and the honest answer is `EIO`, not a panic across the C boundary.
fn lock_body() -> Result<MutexGuard<'static, Option<Bridge<SocketCan>>>, i32> {
    body().lock().map_err(|_| status::EIO)
}

fn say(message: &str) {
    eprintln!("skynet-canopen: {message}");
}

unsafe extern "C" fn open(config_json: *const c_char, out: *mut *mut Ctx) -> i32 {
    if out.is_null() {
        return status::EINVAL;
    }
    let config_text = if config_json.is_null() {
        String::new()
    } else {
        // SAFETY: the OS passes a NUL-terminated config string, or null for an empty one.
        match unsafe { CStr::from_ptr(config_json) }.to_str() {
            Ok(text) => text.to_string(),
            Err(_) => return status::EINVAL,
        }
    };
    let config = match Config::parse(&config_text) {
        Ok(config) => config,
        Err(refusal) => {
            say(refusal.message());
            return refusal.status();
        }
    };
    let mut guard = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    // A second open is a new session: the previous body is closed first, not stacked under it.
    if let Some(previous) = guard.take() {
        if let Ok(mut stop) = stop_path().lock() {
            *stop = None;
        }
        previous.close();
    }
    OPENED.store(false, Ordering::Release);

    let bus = match SocketCan::open(&config.interface) {
        Ok(bus) => bus,
        Err(error) => {
            say(&format!("the bus could not be opened: {error}"));
            return status::EIO;
        }
    };
    let stop_socket = match bus.try_clone() {
        Ok(socket) => socket,
        Err(error) => {
            say(&format!(
                "the stop path could not have a socket of its own: {error}"
            ));
            return status::EIO;
        }
    };
    let bridge = match Bridge::open(bus, &config) {
        Ok(bridge) => bridge,
        Err(refusal) => {
            say(refusal.message());
            return refusal.status();
        }
    };
    for node in bridge.nodes() {
        TARGETS[node as usize].store(0, Ordering::Release);
    }
    let nodes = bridge.nodes().collect();
    match stop_path().lock() {
        Ok(mut stop) => {
            *stop = Some(StopPath {
                socket: stop_socket,
                nodes,
            })
        }
        Err(_) => return status::EIO,
    }
    *guard = Some(bridge);
    // The latch belongs to the session it was asserted in (F73): the drives were disabled at
    // close, and the enable sequence in `Bridge::open` is what re-armed them.
    ESTOPPED.store(false, Ordering::Release);
    OPENED.store(true, Ordering::Release);
    // SAFETY: `out` was null-checked. The context is opaque to the OS; a static body needs no
    // allocation behind it.
    unsafe { *out = body() as *const Mutex<Option<Bridge<SocketCan>>> as *mut Ctx };
    status::OK
}

unsafe extern "C" fn close(ctx: *mut Ctx) -> i32 {
    if ctx.is_null() {
        return status::EINVAL;
    }
    OPENED.store(false, Ordering::Release);
    if let Ok(mut stop) = stop_path().lock() {
        *stop = None;
    }
    if let Ok(mut guard) = lock_body() {
        if let Some(bridge) = guard.take() {
            bridge.close();
        }
    }
    status::OK
}

unsafe extern "C" fn read_state(ctx: *mut Ctx, out: *mut State, _deadline_ns: u64) -> i32 {
    if ctx.is_null() || out.is_null() {
        return status::EINVAL;
    }
    if !OPENED.load(Ordering::Acquire) {
        return status::EPERM;
    }
    // SAFETY: the OS passes a valid `State` whose three arrays are `joint_count` long on the way
    // in — the caller's capacity, which bounds every write below.
    let state = unsafe { &mut *out };
    let mut guard = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    let Some(bridge) = guard.as_mut() else {
        return status::EPERM;
    };
    let joints = bridge.joint_count();
    if (state.joint_count as usize) < joints
        || state.position_rad.is_null()
        || state.velocity_rad_s.is_null()
        || state.effort_nm.is_null()
    {
        return status::EINVAL;
    }
    // SAFETY: non-null, and the caller's capacity is at least `joints`.
    let positions = unsafe { std::slice::from_raw_parts_mut(state.position_rad, joints) };
    let reading = match bridge.read_state(
        ESTOPPED.load(Ordering::Acquire),
        positions,
        clock::acquisition_ns,
    ) {
        Ok(reading) => reading,
        Err(rc) => return rc,
    };
    state.stamp_ns = reading.stamp_ns;
    state.seq = reading.seq;
    state.joint_count = joints as u16;
    state.flags = reading.flags;
    state.fault_code = reading.fault_code;
    state.base_pose = [f32::NAN; 7]; // a fixed base, not localised
    state.base_twist = [0.0; 6];
    for joint in 0..joints {
        // **NaN, not zero.** TPDO2's default mapping carries the statusword and the position and
        // nothing else, so velocity and torque are not measured this cycle (§24).
        // SAFETY: capacity was checked above and the pointers are non-null.
        unsafe {
            *state.velocity_rad_s.add(joint) = f32::NAN;
            *state.effort_nm.add(joint) = f32::NAN;
        }
    }
    reading.status
}

unsafe extern "C" fn write_command(ctx: *mut Ctx, cmd: *const Command, _deadline_ns: u64) -> i32 {
    if ctx.is_null() || cmd.is_null() {
        return status::EINVAL;
    }
    if !OPENED.load(Ordering::Acquire) {
        return status::EPERM;
    }
    // SAFETY: non-null, and the OS keeps it alive for the duration of the call.
    let command = unsafe { &*cmd };
    if command.values.is_null() || command.mask.is_null() {
        return status::EINVAL;
    }
    // Only cyclic synchronous position is wired, as in Python: CSV and CST are an object write and
    // a different RPDO mapping away, and shipping them untested is how a torque command becomes a
    // position command. §86.1's per-joint array is unread for the same reason — the descriptor
    // declares no `per_joint_modes`, so no conformant producer sends one.
    if Mode::from_u8(command.mode) != Some(Mode::Position) {
        return status::EINVAL;
    }
    if ESTOPPED.load(Ordering::Acquire) {
        return status::EPERM;
    }
    let mut guard = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    let Some(bridge) = guard.as_mut() else {
        return status::EPERM;
    };
    let joints = bridge.joint_count();
    if command.joint_count as usize != joints {
        return status::EINVAL;
    }
    // Re-checked under the lock: a stop that lands between the latch check above and this line
    // refuses the command rather than racing it onto the bus — `franka-rs`'s correction, here too.
    if ESTOPPED.load(Ordering::Acquire) {
        return status::EPERM;
    }
    // SAFETY: both arrays are non-null and `joint_count` long, which equals `joints`.
    let (values, mask) = unsafe {
        (
            std::slice::from_raw_parts(command.values, joints),
            std::slice::from_raw_parts(command.mask, joints),
        )
    };
    let rc = bridge.write_command(values, mask);
    if rc == status::OK {
        for (node, target) in bridge.nodes().zip(bridge.targets()) {
            TARGETS[node as usize].store(target, Ordering::Release);
        }
    }
    rc
}

/// Callable from a signal handler and from a dying process: no allocation, no lock waited on, no
/// reply awaited. The latch is set first, so a stop whose frames cannot be sent still refuses
/// every command after it.
unsafe extern "C" fn emergency_stop(ctx: *mut Ctx) -> i32 {
    if ctx.is_null() {
        return status::EINVAL;
    }
    ESTOPPED.store(true, Ordering::Release);
    let stop = match stop_path().try_lock() {
        Ok(stop) => stop,
        // Contended: another thread is inside open or close. The latch is set, later commands are
        // refused, and saying OK would claim frames that were never sent.
        Err(_) => return status::EIO,
    };
    let Some(path) = stop.as_ref() else {
        // No bus is no drives: before open, or after close. The latch above still holds.
        return status::OK;
    };
    let quick_stop = Cia402Command::QuickStop.controlword();
    let mut delivered = true;
    // Every axis is attempted even when one write fails: a stop that gave up at the first error
    // would leave the axes after it running.
    for &node in &path.nodes {
        let target = TARGETS[node as usize].load(Ordering::Acquire);
        delivered &= path
            .socket
            .write_frame(&frames::rpdo2(node, quick_stop, target))
            .is_ok();
    }
    delivered &= path
        .socket
        .write_frame(&frames::nmt(frames::NMT_PRE_OPERATIONAL, 0))
        .is_ok();
    if delivered {
        status::OK
    } else {
        status::EIO
    }
}

unsafe extern "C" fn describe(ctx: *mut Ctx, json_out: *mut c_char, len: usize) -> i32 {
    if ctx.is_null() || json_out.is_null() {
        return status::EINVAL;
    }
    let guard = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    // Before open there is no body to describe, and `bridge.py` answers `{}`.
    let text = guard.as_ref().map(Bridge::describe).unwrap_or("{}");
    let bytes = text.as_bytes();
    // Too small is an error, never a truncation: half a body description is worse than none, and
    // the OS retries with a larger buffer.
    if len < bytes.len() + 1 {
        return status::EINVAL;
    }
    // SAFETY: `json_out` is writable for `len` bytes and `len > bytes.len()`.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, json_out, bytes.len());
        *json_out.add(bytes.len()) = 0;
    }
    bytes.len() as i32
}

unsafe extern "C" fn read_sensor(_ctx: *mut Ctx, _name: *const c_char, _out: *mut Frame) -> i32 {
    // Drives on a bus publish no images; `bridge.py` answers ENOSYS, and so does this.
    status::ENOSYS
}

/// `CLOCK_MONOTONIC_RAW` in nanoseconds — §35.1's clock, and §24's for every `stamp_ns`. Local,
/// for `franka-rs`'s reason: a driver is a vendor's artefact and should build outside this
/// workspace. Linux only, with `skynet-rt`'s fallback elsewhere (F72).
mod clock {
    /// Zero when the call itself fails, which is the clock saying nothing rather than a guess.
    #[cfg(target_os = "linux")]
    pub fn acquisition_ns() -> u64 {
        use std::ffi::c_int;

        #[repr(C)]
        struct Timespec {
            tv_sec: i64,
            tv_nsec: i64,
        }

        extern "C" {
            fn clock_gettime(clk_id: c_int, tp: *mut Timespec) -> c_int;
        }

        /// `CLOCK_MONOTONIC_RAW` from `linux/time.h`, which NTP does not slew.
        const CLOCK_MONOTONIC_RAW: c_int = 4;

        let mut tp = Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: `clock_gettime` with a valid id writes 16 bytes to the pointer it is given.
        let rc = unsafe { clock_gettime(CLOCK_MONOTONIC_RAW, &mut tp) };
        if rc != 0 {
            return 0;
        }
        (tp.tv_sec.max(0) as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(tp.tv_nsec.max(0) as u64)
    }

    /// Off Linux, a monotonic reading from this process's first call — not a base another
    /// process shares, and a robot runs Linux (§23).
    #[cfg(not(target_os = "linux"))]
    pub fn acquisition_ns() -> u64 {
        static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        ORIGIN
            .get_or_init(std::time::Instant::now)
            .elapsed()
            .as_nanos() as u64
    }
}

/// `extension` is null rather than a stub that returns success, so the OS reports the capability
/// as absent, which is true.
static DRIVER: RawDriver = RawDriver {
    abi_version: ABI_VERSION,
    driver_name: c"skynet-canopen-rs".as_ptr(),
    driver_version: c"0.1.0".as_ptr(),
    open: Some(open),
    close: Some(close),
    read_state: Some(read_state),
    write_command: Some(write_command),
    emergency_stop: Some(emergency_stop),
    describe: Some(describe),
    read_sensor: Some(read_sensor),
    extension: None as Option<
        unsafe extern "C" fn(
            *mut Ctx,
            *const c_char,
            *const c_void,
            usize,
            *mut c_void,
            usize,
        ) -> i32,
    >,
};

/// The one symbol a driver shared object must export.
#[no_mangle]
pub extern "C" fn skynet_rhal_entry() -> *const RawDriver {
    &DRIVER
}

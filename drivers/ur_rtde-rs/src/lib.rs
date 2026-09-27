//! skynet-ur-rtde-rs — a Universal Robots arm over RTDE, in Rust.
//!
//! A systems port of `drivers/ur_rtde`'s Python driver (6.6 in `DRY-COVERAGE-PLAN.md`), which is
//! the executable specification and was measured against URSim, UR's own controller software.
//! The wire is held to the bytes the Python session writes and reads (`tests/vectors.rs`, from
//! `tests/gen_vectors.py`); the behaviour to URSim itself, through the real vtable, when
//! `SKYNET_UR_HOST` names one (`tests/live_ursim.rs`).
//!
//! ```text
//! cargo build --release
//! skynet driver check target/release/libskynet_ur_rtde_rs.so --config '{"host": "127.0.0.1"}'
//! ```
//!
//! # Four rules (§4, §89)
//!
//! 1. `read_state` and `write_command` are wait-free and allocate nothing.
//! 2. `emergency_stop` must work from a signal handler and from a process that is dying.
//! 3. `describe` is the source of truth for the body.
//! 4. `read_sensor` is called on the real-time thread unless a sensor says otherwise.
//!
//! Rule 1 is not met, and the crate says so as its siblings do: `read_state` waits up to one
//! controller period for a *new* sample — RTDE publishes on its own clock, and a read that returned
//! the previous sample at once would read as a stalled producer — behind the connection's mutex,
//! which is the Python structure translated. Rule 2 is met: the stop is an atomic latch, then one
//! hold package built on the stack and `stop` to the dashboard, each on a socket duplicated at open
//! and held outside the connection lock, attempted with `try_lock`, no reply awaited.
//!
//! # Why this crate uses `unsafe`, for the CI allow-list (§23.1)
//!
//! The RHAL boundary only: `skynet_rhal_entry` and six `extern "C"` functions dereferencing
//! caller-owned state and command buffers, every pointer null-checked and every array bounded by
//! the count the caller declares. The three sockets are `std::net`, and `rtde`, `dashboard`,
//! `control` and `bridge` are safe code behind the boundary.

pub mod bridge;
pub mod control;
pub mod dashboard;
pub mod rtde;

use std::ffi::{c_char, c_void, CStr};
use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use skynet_rhal::{status, Command, Ctx, Frame, Mode, RawDriver, State, ABI_VERSION};

use bridge::{Config, Connection};

/// The arm, behind the mutex the module header owns up to. `None` before open and after close.
fn body() -> &'static Mutex<Option<Connection>> {
    static BODY: OnceLock<Mutex<Option<Connection>>> = OnceLock::new();
    BODY.get_or_init(|| Mutex::new(None))
}

static OPENED: AtomicBool = AtomicBool::new(false);
static ESTOPPED: AtomicBool = AtomicBool::new(false);

/// What the stop writes to, outside the connection's lock: the RTDE and dashboard sockets,
/// duplicated at open, and the input recipe's id for the hold package.
struct StopPath {
    rtde: TcpStream,
    dashboard: TcpStream,
    recipe: u8,
}

fn stop_path() -> &'static Mutex<Option<StopPath>> {
    static STOP: OnceLock<Mutex<Option<StopPath>>> = OnceLock::new();
    STOP.get_or_init(|| Mutex::new(None))
}

/// The setpoint the registers hold, as `f64` bits: the hold package carries it, as `bridge.py`'s
/// does, and the stop cannot take the connection's lock to read it.
static SETPOINT: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

fn publish_setpoint(setpoint: [f64; 6]) {
    for (slot, value) in SETPOINT.iter().zip(setpoint) {
        slot.store(value.to_bits(), Ordering::Release);
    }
}

fn lock_body() -> Result<MutexGuard<'static, Option<Connection>>, i32> {
    body().lock().map_err(|_| status::EIO)
}

fn say(message: &str) {
    eprintln!("skynet-ur: {message}");
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
    if let Some(note) = config.model_note() {
        say(&note);
    }
    let mut guard = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    // A second open is a new session: the previous arm is let go of first, not stacked.
    OPENED.store(false, Ordering::Release);
    if let Ok(mut stop) = stop_path().lock() {
        *stop = None;
    }
    if let Some(previous) = guard.take() {
        previous.close();
    }
    let connection = match Connection::open(&config, &say) {
        Ok(connection) => connection,
        Err(refusal) => {
            say(refusal.message());
            return refusal.status();
        }
    };
    let (rtde, dashboard, recipe) = match connection.stop_handles() {
        Ok(handles) => handles,
        Err(error) => {
            say(&format!(
                "the stop path could not have sockets of its own: {error}"
            ));
            connection.close();
            return status::EIO;
        }
    };
    publish_setpoint(connection.setpoint());
    match stop_path().lock() {
        Ok(mut stop) => {
            *stop = Some(StopPath {
                rtde,
                dashboard,
                recipe,
            })
        }
        Err(_) => return status::EIO,
    }
    *guard = Some(connection);
    // The latch belongs to the session it was asserted in (F76): the control program was just
    // uploaded afresh, and nothing will be servoed until a command asks for it.
    ESTOPPED.store(false, Ordering::Release);
    OPENED.store(true, Ordering::Release);
    // SAFETY: `out` was null-checked. The context is opaque; a static body needs no allocation.
    unsafe { *out = body() as *const Mutex<Option<Connection>> as *mut Ctx };
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
        if let Some(connection) = guard.take() {
            connection.close();
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
    // SAFETY: the OS passes a valid `State` whose arrays are `joint_count` long on the way in.
    let state = unsafe { &mut *out };
    if (state.joint_count as usize) < 6
        || state.position_rad.is_null()
        || state.velocity_rad_s.is_null()
        || state.effort_nm.is_null()
    {
        return status::EINVAL;
    }
    let mut guard = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    let Some(connection) = guard.as_mut() else {
        return status::EPERM;
    };
    let reading = match connection.read_state(
        ESTOPPED.load(Ordering::Acquire),
        skynet_rt::monotonic_raw_ns,
    ) {
        Ok(reading) => reading,
        Err(rc) => return rc,
    };
    state.stamp_ns = reading.stamp_ns;
    state.seq = reading.seq;
    state.joint_count = 6;
    state.flags = reading.flags;
    state.fault_code = reading.fault_code;
    state.base_pose = [f32::NAN; 7]; // a fixed base, not localised
    state.base_twist = [0.0; 6];
    for joint in 0..6 {
        // SAFETY: capacity was checked above and the pointers are non-null.
        unsafe {
            *state.position_rad.add(joint) = reading.sample.q[joint] as f32;
            *state.velocity_rad_s.add(joint) = reading.sample.qd[joint] as f32;
            // **NaN.** `actual_current` is amps, and the constant that makes it newton-metres is
            // not published (§5.2).
            *state.effort_nm.add(joint) = f32::NAN;
        }
    }
    status::OK
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
    if command.joint_count != 6 || command.values.is_null() || command.mask.is_null() {
        return status::EINVAL;
    }
    // `servoj` is a joint-position statement. Velocity is `speedj` and torque is not reachable
    // from URScript at all, and wiring either against a controller nobody has run them on is the
    // shape F15 refused. §86.1's per-joint array is unread: the descriptor declares no
    // `per_joint_modes`.
    if Mode::from_u8(command.mode) != Some(Mode::Position) {
        return status::EINVAL;
    }
    if ESTOPPED.load(Ordering::Acquire) {
        return status::EPERM;
    }
    let mut values = [0.0f32; 6];
    let mut mask = [0u8; 6];
    for joint in 0..6 {
        // SAFETY: `joint_count` is 6, and both arrays are that long.
        unsafe {
            values[joint] = *command.values.add(joint);
            mask[joint] = *command.mask.add(joint);
        }
    }
    let mut guard = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    let Some(connection) = guard.as_mut() else {
        return status::EPERM;
    };
    // Re-checked under the lock: a stop that lands between the check above and this line refuses
    // the command rather than racing it onto the wire.
    if ESTOPPED.load(Ordering::Acquire) {
        return status::EPERM;
    }
    let rc = connection.write_command(&values, &mask);
    if rc == status::OK {
        publish_setpoint(connection.setpoint());
    }
    rc
}

/// Callable from a signal handler and from a dying process: the latch, then the hold package and
/// the dashboard's `stop`, each written once on a socket duplicated at open. No allocation, no
/// lock waited on, no reply read. `SKR_OK` when either reached the controller — either one stops
/// `servoj` being fed — and `SKR_EIO` when neither did, rather than claiming a stop that was not
/// sent.
unsafe extern "C" fn emergency_stop(ctx: *mut Ctx) -> i32 {
    if ctx.is_null() {
        return status::EINVAL;
    }
    ESTOPPED.store(true, Ordering::Release);
    let stop = match stop_path().try_lock() {
        Ok(stop) => stop,
        // Contended: open or close is running. The latch refuses later commands; saying OK
        // would claim frames that were never sent.
        Err(_) => return status::EIO,
    };
    let Some(path) = stop.as_ref() else {
        return status::OK; // no controller: before open, or after close
    };
    let mut setpoint = [0.0f64; 6];
    for (value, slot) in setpoint.iter_mut().zip(&SETPOINT) {
        *value = f64::from_bits(slot.load(Ordering::Acquire));
    }
    let hold = rtde::setpoint_package(path.recipe, &setpoint, control::CMD_HOLD);
    let held = (&path.rtde).write_all(&hold).is_ok();
    let stopped = (&path.dashboard).write_all(b"stop\n").is_ok();
    if held || stopped {
        status::OK
    } else {
        status::EIO
    }
}

/// Clear the session latch for §48.2's power-on self-test, and restart the program the stop
/// stopped.
///
/// `dashboard stop` halts the program `open` loaded, and setpoints go nowhere without one — so
/// the latch alone would accept commands the controller never executes. `play` restarts the
/// loaded program, on the stop path's own dashboard socket, fire-and-forget like the stop: the
/// reply sits unread on a socket nothing reads, and the program is turning by the time the loop
/// commands. The latch clears; nothing resumes — the stop's hold setpoint stands until the next
/// command replaces it. Idempotent: `play` on a running program is a no-op the controller
/// absorbs.
unsafe extern "C" fn release_stop(ctx: *mut Ctx) -> i32 {
    if ctx.is_null() {
        return status::EINVAL;
    }
    ESTOPPED.store(false, Ordering::Release);
    let stop = match stop_path().lock() {
        Ok(stop) => stop,
        Err(_) => return status::EIO,
    };
    let Some(path) = stop.as_ref() else {
        return status::OK;
    };
    if (&path.dashboard).write_all(b"play\n").is_ok() {
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
    // Before open there is no arm to describe, and `bridge.py` answers `{}`.
    let text = guard.as_ref().map(Connection::describe).unwrap_or("{}");
    let bytes = text.as_bytes();
    // Too small is an error, never a truncation.
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
    // An arm over RTDE publishes no images; `bridge.py` answers ENOSYS, and so does this.
    status::ENOSYS
}

/// `extension` is null rather than a stub that returns success.
static DRIVER: RawDriver = RawDriver {
    abi_version: ABI_VERSION,
    driver_name: c"skynet-ur-rtde-rs".as_ptr(),
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
    release_stop: Some(release_stop),
};

/// The one symbol a driver shared object must export.
#[no_mangle]
pub extern "C" fn skynet_rhal_entry() -> *const RawDriver {
    &DRIVER
}

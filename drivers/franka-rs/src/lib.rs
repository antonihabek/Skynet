//! skynet-franka-rs — a Franka arm over the FCI, in Rust.
//!
//! A systems port of `drivers/franka`'s Python driver (6.6 in `DRY-COVERAGE-PLAN.md`), which is
//! the executable specification: the framing is byte-identical with it (`tests/vectors.json`),
//! the constants are held to the same `layout.json` (`tests/layout.rs`), and the behaviour is
//! exercised against the same fake Control box (`tests/live_fake.rs`).
//!
//! ```text
//! cargo build --release
//! skynet driver check target/release/libskynet_franka_rs.so --config '{"port": <fake>}'
//! ```
//!
//! # Four rules (§4, §89)
//!
//! 1. `read_state` and `write_command` are wait-free and allocate nothing. **Nothing** in them may
//!    allocate, lock or log — they run inside the real-time path at the joint rate.
//! 2. `emergency_stop` must work from a signal handler and from a process that is dying.
//! 3. `describe` is the source of truth for the body, so it cannot drift from the hardware without
//!    this driver being wrong.
//! 4. `read_sensor` is called on the real-time thread unless `describe` declares
//!    `"concurrent_read": true` on that sensor.
//!
//! Rule 1 is the one this port does not meet yet, and it says so rather than leaning on the
//! scaffold's wording: the connection sits behind a mutex, `read_state` blocks on the socket with
//! a timeout, and `write_command` transacts on the calling thread. That is the Python driver's
//! structure, translated — 6.6 puts the driver in a language that permits the Host ceiling and
//! does not approach it. Rule 2 is met in full: the stop is an atomic latch plus one `send` on a
//! socket held outside the connection lock, attempted with `try_lock` and never waited for.
//!
//! # Why this crate uses `unsafe`, for the CI allow-list (§23.1)
//!
//! A driver is the RHAL from the other side: `skynet_rhal_entry` and five `extern "C"`
//! functions dereferencing caller-owned state and command buffers, and that boundary cannot be
//! written without `unsafe`. The unsafety lives only in this file — `protocol` and `driver`
//! are safe code behind it — and every pointer is null-checked with every array bounded by the
//! capacity the caller declares.

pub mod driver;
pub mod protocol;

use std::ffi::{c_char, c_void, CStr};
use std::io::Write;
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use skynet_rhal::{status, Command, Ctx, Frame, Mode, RawDriver, State, ABI_VERSION};

use driver::{acquisition_ns, Config, Connection};

/// The connection, behind the mutex the module header owns up to.
fn body() -> &'static Mutex<Connection> {
    static BODY: OnceLock<Mutex<Connection>> = OnceLock::new();
    BODY.get_or_init(|| Mutex::new(Connection::new()))
}

static OPENED: AtomicBool = AtomicBool::new(false);
static ESTOPPED: AtomicBool = AtomicBool::new(false);

/// The stop path's own socket, cloned from the command connection at open and held outside the
/// connection lock: `emergency_stop` sends on it without waiting for a transact that may be
/// blocked on a dead box. `None` before open and after close.
fn stop_socket() -> &'static Mutex<Option<TcpStream>> {
    static STOP: OnceLock<Mutex<Option<TcpStream>>> = OnceLock::new();
    STOP.get_or_init(|| Mutex::new(None))
}

/// Lock the connection, or refuse when it is poisoned. A poisoned mutex is a thread that died
/// holding the bus — the state behind it is suspect and the honest answer is `EIO`, not a panic
/// across the C boundary and not proceeding as if nothing happened.
fn lock_body() -> Result<std::sync::MutexGuard<'static, Connection>, i32> {
    body().lock().map_err(|_| status::EIO)
}

unsafe extern "C" fn open(config_json: *const c_char, out: *mut *mut Ctx) -> i32 {
    if out.is_null() {
        return status::EINVAL;
    }
    // SAFETY: the OS passes a NUL-terminated config string, or null for an empty one.
    let config_text = if config_json.is_null() {
        String::new()
    } else {
        match unsafe { CStr::from_ptr(config_json) }.to_str() {
            Ok(text) => text.to_string(),
            Err(_) => return status::EINVAL,
        }
    };
    let config = match Config::parse(&config_text) {
        Ok(config) => config,
        Err(rc) => return rc,
    };
    let mut conn = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    match conn.open(&config) {
        Ok(stop_tcp) => {
            if let Ok(mut stop) = stop_socket().lock() {
                *stop = Some(stop_tcp);
            }
            OPENED.store(true, Ordering::Release);
            ESTOPPED.store(false, Ordering::Release);
            // The context pointer is opaque to the OS; a static body needs no allocation behind it.
            unsafe { *out = body() as *const Mutex<Connection> as *mut Ctx };
            status::OK
        }
        Err(rc) => rc,
    }
}

unsafe extern "C" fn close(ctx: *mut Ctx) -> i32 {
    if ctx.is_null() {
        return status::EINVAL;
    }
    if let Ok(mut stop) = stop_socket().lock() {
        *stop = None;
    }
    if let Ok(mut conn) = lock_body() {
        conn.close();
    }
    OPENED.store(false, Ordering::Release);
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
    if (state.joint_count as usize) < protocol::JOINTS {
        return status::EINVAL;
    }
    if state.position_rad.is_null() || state.velocity_rad_s.is_null() || state.effort_nm.is_null() {
        return status::EINVAL;
    }
    let mut conn = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    let view = match conn.read_state(ESTOPPED.load(Ordering::Acquire)) {
        Ok(view) => view,
        Err(rc) => return rc,
    };
    // Stamped at ACQUISITION, never at return: a timestamp applied on return folds this driver's
    // own latency into every downstream budget. (The acquisition happened inside `receive`, one
    // call down; the stamp is read here because the connection does not own the clock.)
    state.stamp_ns = acquisition_ns();
    state.seq = view.seq;
    state.joint_count = protocol::JOINTS as u16;
    state.flags = view.flags;
    state.base_pose = [f32::NAN; 7]; // not localised
    state.base_twist = [0.0; 6];
    state.fault_code = view.fault_code;
    for joint in 0..protocol::JOINTS {
        // SAFETY: capacity was checked above and the pointers are non-null.
        unsafe {
            *state.position_rad.add(joint) = view.q[joint];
            *state.velocity_rad_s.add(joint) = view.dq[joint];
            // Measured link-side torque, which the arm does report — so this is one of the few
            // bodies here where `effort_nm` is a measurement rather than §24's NaN.
            *state.effort_nm.add(joint) = view.tau_j[joint];
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

    // Shape first, and refuse rather than clamp: a driver that accepts a short command is one
    // that reads past the end of the caller's array.
    if command.joint_count as usize != protocol::JOINTS
        || command.values.is_null()
        || command.mask.is_null()
    {
        return status::EINVAL;
    }
    let Some(mode) = Mode::from_u8(command.mode) else {
        return status::EINVAL;
    };
    // §86.1's per-joint array is unread here, exactly as the Python driver does not read it: the
    // descriptor declares no `per_joint_modes`, so no conformant producer sends one, and the FCI
    // runs one motion generator in one mode. A body-wide command is the only shape this body has.
    if ESTOPPED.load(Ordering::Acquire) {
        return status::EPERM;
    }

    let mut values = [0.0f32; protocol::JOINTS];
    let mut mask = [0u8; protocol::JOINTS];
    for joint in 0..protocol::JOINTS {
        // SAFETY: `joint_count` was checked to equal JOINTS, and both arrays are that long.
        unsafe {
            values[joint] = *command.values.add(joint);
            mask[joint] = *command.mask.add(joint);
        }
    }
    let mut conn = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    // Re-checked under the lock: a stop that lands between the latch check above and this line
    // must refuse the command, not race it onto the wire. The Python driver does not re-check;
    // the port closes the race rather than translating it.
    if ESTOPPED.load(Ordering::Acquire) {
        return status::EPERM;
    }
    conn.write_command(mode, &values, &mask, false)
}

/// Callable from a signal handler and from a dying process. No allocation, no blocking, no locks
/// waited on: the latch is an atomic and the socket is attempted with `try_lock`, so a stop that
/// lands while another thread holds the stop socket's mutex still latches — the message is the
/// second half of the stop, and the latch is the first.
unsafe extern "C" fn emergency_stop(ctx: *mut Ctx) -> i32 {
    if ctx.is_null() {
        return status::EINVAL;
    }
    ESTOPPED.store(true, Ordering::Release);
    let message = protocol::pack_command(protocol::COMMAND_STOP_MOVE, stop_command_id(), &[]);
    let sent = match stop_socket().try_lock() {
        Ok(stop) => match stop.as_ref() {
            // No socket is no box: before open, or after close cleared it. The latch above is
            // still set — a stop asked for on a closed body refuses the commands that would
            // follow an open, which is the safe direction.
            None => return status::OK,
            Some(tcp) => {
                let mut tcp = tcp;
                tcp.write_all(&message).is_ok()
            }
        },
        // Contended: another thread is inside open or close. The latch is set, the later
        // commands are refused, and saying OK would claim a message that was never sent.
        Err(_) => return status::EIO,
    };
    if !sent {
        // The stop is a latch first and a message second. A box that is unreachable is a box
        // this driver cannot stop, and saying SKR_OK would be claiming otherwise.
        return status::EIO;
    }
    if let Ok(mut conn) = body().try_lock() {
        conn.note_stopped();
    }
    status::OK
}

/// Command ids for the stop path, which cannot share the connection's counter without taking its
/// lock. Ids only match replies to requests, the stop waits for no reply, and any collision with
/// a transact id resolves the same way: a reply nobody waits for is dropped.
fn stop_command_id() -> u32 {
    use std::sync::atomic::AtomicU32;
    static NEXT: AtomicU32 = AtomicU32::new(0x8000_0000);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

unsafe extern "C" fn describe(ctx: *mut Ctx, json_out: *mut c_char, len: usize) -> i32 {
    if ctx.is_null() || json_out.is_null() {
        return status::EINVAL;
    }
    let conn = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    let bytes = conn.describe();
    let bytes = bytes.as_bytes();
    // Too small is an error, never a truncation: half a parsed body description is worse than a
    // failed one, and the OS retries with a larger buffer.
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
    // A Franka arm publishes no images. §24 reports the capability as absent rather than
    // answering an empty frame, and the descriptor declares no sensors to match.
    status::ENODEV
}

/// `extension` is null rather than a stub that returns success. The OS reports the
/// capability as absent, which is true, instead of calling through to something that lies.
/// `read_sensor` is a real entry point returning `ENODEV`, exactly as the Python driver answers:
/// an arm with no images, refusing the read rather than absenting the entry.
static DRIVER: RawDriver = RawDriver {
    abi_version: ABI_VERSION,
    driver_name: c"skynet-franka-rs".as_ptr(),
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

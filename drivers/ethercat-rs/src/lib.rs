//! skynet-ethercat-rs — CiA 402 drives on an EtherCAT segment, in Rust.
//!
//! A systems port of `drivers/ethercat`'s Python driver (6.6 in `DRY-COVERAGE-PLAN.md`), which is
//! the executable specification and has been cross-checked against SOEM. The frames are held to
//! golden vectors from `frames.py` (`tests/vectors.rs`); the behaviour to the Python slave stack,
//! served by `python3 -m skynet_ethercat.wire` on one end of a veth pair, with this master on the
//! other over `AF_PACKET` (`tests/live_wire.rs`) — SOEM's own arrangement.
//!
//! # Four rules (§4, §89)
//!
//! Rule 1 (wait-free reads) is not met, and the crate says so as its siblings do: a read is one
//! `LRW` round the ring, waited for, behind the body's mutex. Rule 2 is met: the stop is an atomic
//! latch, then one quick-stop `LRW` built on the stack from atomics and written once on a socket
//! duplicated at open — no lock waited on, no allocation, no answer read. Its index is 0, which no
//! exchange of the master's uses, so its answer is never read as a cycle's. The next `read_state`
//! keeps every controlword at quick stop and asserts the working counter.
//!
//! # Why this crate uses `unsafe`, for the CI allow-list (§23.1)
//!
//! The RHAL boundary — `skynet_rhal_entry` and six `extern "C"` functions over caller-owned
//! buffers, every pointer null-checked and every array bounded by the count the caller declares —
//! and `AF_PACKET` in `segment`: `socket`, `if_nametoindex`, `bind`, `poll` and `recv` declared
//! rather than depended on. `frames`, `master` and `bridge` are safe code behind the two.

pub mod bridge;
pub mod frames;
pub mod master;
pub mod segment;

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};

use skynet_rhal::{status, Command, Ctx, Frame, Mode, RawDriver, State, ABI_VERSION};

use bridge::{Bridge, Config, MAX_DRIVES};
use segment::RawSocket;

fn body() -> &'static Mutex<Option<Bridge<RawSocket>>> {
    static BODY: OnceLock<Mutex<Option<Bridge<RawSocket>>>> = OnceLock::new();
    BODY.get_or_init(|| Mutex::new(None))
}

static OPENED: AtomicBool = AtomicBool::new(false);
static ESTOPPED: AtomicBool = AtomicBool::new(false);

/// The stop's socket and the image's layout, made at open.
struct StopPath {
    socket: RawSocket,
    output_at: Vec<usize>,
    logical_length: usize,
}

fn stop_path() -> &'static Mutex<Option<StopPath>> {
    static STOP: OnceLock<Mutex<Option<StopPath>>> = OnceLock::new();
    STOP.get_or_init(|| Mutex::new(None))
}

/// Each drive's target in the image, by position: the quick-stop frame carries it.
static TARGETS: [AtomicI32; MAX_DRIVES] = [const { AtomicI32::new(0) }; MAX_DRIVES];

fn publish_targets(bridge: &Bridge<RawSocket>) {
    for (slot, target) in TARGETS.iter().zip(bridge.targets()) {
        slot.store(target, Ordering::Release);
    }
}

fn lock_body() -> Result<MutexGuard<'static, Option<Bridge<RawSocket>>>, i32> {
    body().lock().map_err(|_| status::EIO)
}

fn say(message: &str) {
    eprintln!("skynet-ethercat: {message}");
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
    // A fresh session: the previous body is stopped and let go of, and the latch clears (F76).
    OPENED.store(false, Ordering::Release);
    if let Ok(mut stop) = stop_path().lock() {
        *stop = None;
    }
    if let Some(previous) = guard.take() {
        previous.close();
    }
    let Some(interface) = config.interface.clone() else {
        say("no `interface`. This driver does not guess: name the NIC the segment is on.");
        return status::EIO;
    };
    let segment = match RawSocket::open(&interface) {
        Ok(segment) => segment,
        Err(error) => {
            say(&error.to_string());
            return status::EIO;
        }
    };
    let stop_socket = match segment.try_clone() {
        Ok(socket) => socket,
        Err(error) => {
            say(&format!(
                "the stop path could not have a socket of its own: {error}"
            ));
            return status::EIO;
        }
    };
    let bridge = match Bridge::open(segment, &config) {
        Ok(bridge) => bridge,
        Err(refusal) => {
            say(refusal.message());
            return refusal.status();
        }
    };
    publish_targets(&bridge);
    match stop_path().lock() {
        Ok(mut stop) => {
            *stop = Some(StopPath {
                socket: stop_socket,
                output_at: bridge.master().output_at.clone(),
                logical_length: bridge.master().logical_length,
            })
        }
        Err(_) => return status::EIO,
    }
    *guard = Some(bridge);
    ESTOPPED.store(false, Ordering::Release);
    OPENED.store(true, Ordering::Release);
    // SAFETY: `out` was null-checked; the context is opaque and a static body needs no allocation.
    unsafe { *out = body() as *const Mutex<Option<Bridge<RawSocket>>> as *mut Ctx };
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
    // SAFETY: the OS passes a valid `State` whose arrays are `joint_count` long on the way in.
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
    let reading = match bridge.read_state(ESTOPPED.load(Ordering::Acquire), positions) {
        Ok(reading) => reading,
        Err(error) => {
            say(&error.to_string());
            return status::EIO;
        }
    };
    state.stamp_ns = skynet_rt::monotonic_raw_ns();
    state.seq = reading.seq;
    state.joint_count = joints as u16;
    state.flags = reading.flags;
    state.fault_code = reading.fault_code;
    state.base_pose = [f32::NAN; 7];
    state.base_twist = [0.0; 6];
    for joint in 0..joints {
        // NaN, not zero: this PDO map carries neither `0x606C` nor `0x6077` (§24).
        // SAFETY: capacity was checked above and the pointers are non-null.
        unsafe {
            *state.velocity_rad_s.add(joint) = f32::NAN;
            *state.effort_nm.add(joint) = f32::NAN;
        }
    }
    status::OK
}

unsafe extern "C" fn write_command(ctx: *mut Ctx, cmd: *const Command, _deadline_ns: u64) -> i32 {
    if ctx.is_null() || cmd.is_null() {
        return status::EINVAL;
    }
    if !OPENED.load(Ordering::Acquire) || ESTOPPED.load(Ordering::Acquire) {
        return status::EPERM;
    }
    // SAFETY: non-null, and the OS keeps it alive for the duration of the call.
    let command = unsafe { &*cmd };
    let Some(mode) = Mode::from_u8(command.mode) else {
        return status::EINVAL; // a mode this ABI does not define is malformed
    };
    if command.values.is_null() || command.mask.is_null() {
        return status::EINVAL;
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
    if mode != Mode::Position {
        // CSP only: velocity and torque are other objects in another PDO map, and accepting the
        // mode while sending a position would move a joint in the wrong units. Refused as not
        // permitted, as `bridge.py` refuses it.
        return status::EPERM;
    }
    // Re-checked under the lock: a stop between the check above and here refuses the command.
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
        publish_targets(bridge);
    }
    rc
}

/// The largest frame the stop builds: an Ethernet frame without its FCS.
const STOP_FRAME: usize = 1518;

/// Callable from a signal handler and from a dying process: the latch, then one quick-stop `LRW`
/// built on the stack and written once. `SKR_EIO` when it could not be written; no answer is
/// awaited, and the next `read_state` asserts the working counter with the stop still held.
unsafe extern "C" fn emergency_stop(ctx: *mut Ctx) -> i32 {
    if ctx.is_null() {
        return status::EINVAL;
    }
    ESTOPPED.store(true, Ordering::Release);
    let stop = match stop_path().try_lock() {
        Ok(stop) => stop,
        Err(_) => return status::EIO, // open or close is running; the latch holds
    };
    let Some(path) = stop.as_ref() else {
        return status::OK; // no segment: before open, or after close
    };
    let mut raw = [0u8; STOP_FRAME];
    let Some(length) = quick_stop_frame(&mut raw, &path.output_at, path.logical_length) else {
        return status::EIO;
    };
    match path.socket.send_raw(&raw[..length]) {
        Ok(()) => status::OK,
        Err(_) => status::EIO,
    }
}

/// The quick-stop `LRW`, written into `raw` without allocating: every drive's controlword at
/// quick stop and its target from [`TARGETS`], index 0. The byte layout is `frames::Frame::pack`'s,
/// which `tests/vectors.rs` holds to Python's.
fn quick_stop_frame(
    raw: &mut [u8; STOP_FRAME],
    output_at: &[usize],
    length: usize,
) -> Option<usize> {
    let body = 10 + length + 2;
    if length > frames::MAX_DATAGRAM_DATA || 16 + body > STOP_FRAME {
        return None;
    }
    raw[0..6].copy_from_slice(&frames::BROADCAST_MAC);
    raw[6..12].copy_from_slice(&frames::MASTER_MAC);
    raw[12..14].copy_from_slice(&frames::ETHERTYPE.to_be_bytes());
    let header = (body as u16 & 0x7FF) | (frames::FRAME_TYPE_DATAGRAMS << 12);
    raw[14..16].copy_from_slice(&header.to_le_bytes());
    raw[16] = frames::LRW;
    raw[17] = 0; // the stop's index — never an exchange's
    raw[18..22].fill(0); // logical address 0
    raw[22..24].copy_from_slice(&(length as u16 & 0x7FF).to_le_bytes());
    raw[24..26].fill(0); // irq
    let image = &mut raw[26..26 + length];
    image.fill(0);
    let word = skynet_cia402_rs::Command::QuickStop
        .controlword()
        .to_le_bytes();
    for (position, &at) in output_at.iter().enumerate() {
        if at + 6 > length || position >= MAX_DRIVES {
            return None;
        }
        let target = TARGETS[position].load(Ordering::Acquire);
        image[at..at + 2].copy_from_slice(&word);
        image[at + 2..at + 6].copy_from_slice(&target.to_le_bytes());
    }
    raw[26 + length..28 + length].fill(0); // working counter
    let used = 16 + body;
    let padded = used.max(14 + frames::MIN_ETHERNET_PAYLOAD);
    raw[used..padded].fill(0);
    Some(padded)
}

unsafe extern "C" fn describe(ctx: *mut Ctx, json_out: *mut c_char, len: usize) -> i32 {
    if ctx.is_null() || json_out.is_null() {
        return status::EINVAL;
    }
    let guard = match lock_body() {
        Ok(guard) => guard,
        Err(rc) => return rc,
    };
    // Before open there is no body; Python raises there, and the port refuses rather than invent
    // one.
    let Some(text) = guard.as_ref().map(Bridge::describe) else {
        return status::EPERM;
    };
    let bytes = text.as_bytes();
    if len < bytes.len() + 1 {
        return status::EINVAL; // too small is an error, never a truncation
    }
    // SAFETY: `json_out` is writable for `len` bytes and `len > bytes.len()`.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, json_out, bytes.len());
        *json_out.add(bytes.len()) = 0;
    }
    bytes.len() as i32
}

unsafe extern "C" fn read_sensor(_ctx: *mut Ctx, _name: *const c_char, _out: *mut Frame) -> i32 {
    // A servo segment publishes no images; `bridge.py` answers ENODEV, and so does this.
    status::ENODEV
}

static DRIVER: RawDriver = RawDriver {
    abi_version: ABI_VERSION,
    driver_name: c"skynet-ethercat-rs".as_ptr(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stop_frame_is_the_packed_frame() {
        // The stack-built quick-stop LRW must be byte-identical with the same frame packed the
        // ordinary way, index 0 and all.
        let output_at = [0usize, 6];
        let length = 24;
        TARGETS[0].store(1234, Ordering::Release);
        TARGETS[1].store(-5, Ordering::Release);
        let mut raw = [0u8; STOP_FRAME];
        let n = quick_stop_frame(&mut raw, &output_at, length).expect("fits");
        let mut image = vec![0u8; length];
        let word = skynet_cia402_rs::Command::QuickStop
            .controlword()
            .to_le_bytes();
        for (at, target) in [(0usize, 1234i32), (6, -5)] {
            image[at..at + 2].copy_from_slice(&word);
            image[at + 2..at + 6].copy_from_slice(&target.to_le_bytes());
        }
        let packed = frames::Frame::new(vec![frames::Datagram::new(frames::LRW, 0, (0, 0), image)])
            .pack()
            .unwrap();
        assert_eq!(&raw[..n], packed.as_slice());
    }
}

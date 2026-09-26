//! A safe wrapper around a loaded driver.
//!
//! [`Library`](crate::Library) opens the shared object and hands back one function pointer. This is
//! the layer above it: an open driver context, and the four calls a control loop makes on it, with
//! every raw pointer confined to this file.
//!
//! It lives here rather than in `skynet-kernel-host` for the reason the crate header gives. Calling
//! into a driver is calling arbitrary native code, and the argument that the unsafety is contained
//! only holds if it is contained in **one** place. A host crate that declared
//! `#![forbid(unsafe_code)]` and then did its own `unsafe` in a module would be making that claim
//! and breaking it in the same crate.
//!
//! # What each call assumes, and where the assumption comes from
//!
//! The RHAL is a C ABI: the driver fills caller-allocated arrays and never frees, resizes or
//! retains them past the call (§24). [`Buffers`] is those arrays, owned on this side, sized once at
//! open and reused every cycle — so the hot path never reaches an allocator, which is the property
//! §24 asks for and the reason the buffers are a struct rather than a return value.
//!
//! Every entry point is optional in the vtable and a null one is a capability the driver does not
//! have. None is called through without being checked, and the absent ones are reported as absent
//! rather than as failures: a driver with no `emergency_stop` is a real driver with a real
//! limitation, and §7.1's Cat 0 response has to know that rather than discover it.

use std::ffi::{c_char, c_void, CStr, CString};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError, TryLockError, Weak};

use skynet_rhal::{accepts, Ctx, LoadError, RawDriver, State};

use crate::{DlError, Library};

/// Why a driver could not be opened or driven.
#[derive(Debug)]
pub enum DriverError {
    /// The shared object could not be loaded, or exports no entry point.
    Load(DlError),
    /// The vtable is not one this OS accepts.
    Rejected(LoadError),
    /// `open()` returned a non-zero code.
    Open(i32),
    /// The vtable omits an entry point this operation needs.
    Absent(&'static str),
    /// A call returned a non-zero code. Negative is an `errno` (§24).
    Call {
        /// Which entry point.
        entry: &'static str,
        /// What it returned.
        code: i32,
    },
    /// A call succeeded and what it handed back cannot be used as given.
    ///
    /// Distinct from [`DriverError::Call`], because the driver said *yes*: the fault is in what it
    /// wrote, and a person reading a code of zero beside "failed" would be reading a contradiction.
    Malformed {
        /// Which entry point.
        entry: &'static str,
        /// What was wrong with it.
        detail: String,
    },
    /// A sensor frame larger than the buffer declared for its stream.
    ///
    /// Refused rather than truncated, for the reason `skynet-frames` refuses one: a policy handed
    /// nine tenths of an image has no way to know. The capacity is §47.5's declaration, so the fix is
    /// the declaration rather than this process growing a buffer a driver asked it to.
    Oversized {
        /// Bytes the frame occupies.
        needed: usize,
        /// Bytes the stream was declared to hold.
        capacity: usize,
    },
    /// A second caller for a sensor whose descriptor does not declare `concurrent_read` (§89.1).
    ///
    /// Not a fault in the driver: an undeclared sensor is read on the control thread, which is what
    /// every driver written before §89 means. It is the answer to a request this OS must not grant.
    Undeclared {
        /// The sensor asked for.
        sensor: String,
        /// Why it cannot be read off the control thread.
        detail: String,
    },
    /// `read_sensor` was asked for on the control thread while the capture thread was inside it.
    ///
    /// §89.1 promises a driver that `read_sensor` never runs concurrently with itself, and the
    /// control thread does not wait, so this read is not made rather than made in parallel.
    Busy(&'static str),
    /// The driver this handle read through has been closed.
    Closed,
}

impl std::fmt::Display for DriverError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DriverError::Load(error) => write!(f, "{error}"),
            DriverError::Rejected(error) => write!(f, "the driver was rejected: {error:?}"),
            DriverError::Open(code) => write!(
                f,
                "the driver's open() returned {code}. A non-zero return means the driver declined \
                 to attach to this machine; its own logs say why, and the config string is the \
                 usual cause."
            ),
            DriverError::Absent(entry) => write!(
                f,
                "the driver's vtable has no `{entry}`. That entry point is optional in the RHAL, \
                 so this is a limitation of the driver rather than a fault: the OS reports the \
                 capability as absent and never calls through a null pointer."
            ),
            DriverError::Call { entry, code } => {
                write!(f, "the driver's {entry}() returned {code}")
            }
            DriverError::Malformed { entry, detail } => {
                write!(f, "the driver's {entry}() succeeded and {detail}")
            }
            DriverError::Oversized { needed, capacity } => write!(
                f,
                "the driver handed back a {needed}-byte frame for a stream declared to hold \
                 {capacity}. It is refused rather than truncated (§47.5); declare a larger slot."
            ),
            DriverError::Undeclared { sensor, detail } => write!(
                f,
                "`{sensor}` cannot be read off the control thread: {detail}. Only a sensor whose \
                 descriptor declares `\"concurrent_read\": true` may be (§89.1); an undeclared one \
                 is read between cycles."
            ),
            DriverError::Busy(entry) => write!(
                f,
                "the driver's {entry}() was already running on the capture thread, and the control \
                 thread does not wait for it (§89.1)"
            ),
            DriverError::Closed => write!(f, "the driver has been closed"),
        }
    }
}

impl std::error::Error for DriverError {}

/// The three per-joint arrays a driver fills, owned on this side.
///
/// Allocated once and reused, per §24. The width is the driver's declared joint count and does not
/// change while a context is open.
#[derive(Debug)]
pub struct Buffers {
    position: Vec<f32>,
    velocity: Vec<f32>,
    effort: Vec<f32>,
}

impl Buffers {
    /// Room for `joints` joints.
    pub fn new(joints: usize) -> Buffers {
        Buffers {
            position: vec![f32::NAN; joints],
            velocity: vec![f32::NAN; joints],
            effort: vec![f32::NAN; joints],
        }
    }

    /// Measured positions, as of the last [`Driver::read_state`].
    pub fn position(&self) -> &[f32] {
        &self.position
    }

    /// Measured velocities; `NaN` where the joint does not measure it.
    pub fn velocity(&self) -> &[f32] {
        &self.velocity
    }

    /// Measured efforts; `NaN` where unmeasured.
    pub fn effort(&self) -> &[f32] {
        &self.effort
    }
}

/// Everything about one state sample except the per-joint arrays, which stay in [`Buffers`].
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// `CLOCK_MONOTONIC_RAW` at acquisition, never at return (§24).
    pub stamp_ns: u64,
    /// Sample sequence number.
    pub seq: u32,
    /// How many joints the driver reported.
    pub joints: usize,
    /// Bitwise-or of `skynet_rhal::flags`.
    pub flags: u16,
    /// Vendor-specific, surfaced verbatim.
    pub fault_code: u32,
    /// `x y z qx qy qz qw`, all `NaN` when the base is not localised.
    pub base_pose: [f32; 7],
    /// `vx vy vz wx wy wz`.
    pub base_twist: [f32; 6],
}

/// Everything about one sensor frame except its bytes, which were copied into the caller's buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SensorFrame {
    /// `CLOCK_MONOTONIC_RAW` at acquisition (§47.2). Never zero: a frame with no stamp is refused.
    pub stamp_ns: u64,
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    /// One of `skynet_rhal::format`.
    pub format: u32,
    /// How many bytes were copied.
    pub len: usize,
}

/// An open driver context.
///
/// Closing is [`Context`]'s [`Drop`], and the library is held alongside the context so it cannot be
/// unloaded while the context is live — unloading under a live context is what turns a clean
/// failure into a crash in somebody else's `atexit` handler.
// No `Debug`: the vtable is a C struct of raw function pointers and does not implement it, and a
// hand-written one would print addresses that mean nothing to a reader.
pub struct Driver {
    // Copies of the context's two fields, for the calls below. `context` owns them.
    ctx: *mut Ctx,
    vtable: &'static RawDriver,
    /// Shared only with a [`ConcurrentSensor`], and only weakly, so this is the one strong
    /// reference: the context closes when the `Driver` does.
    context: Arc<Context>,
}

/// What a `Driver` owns: the context, the library it came from, and the lock every `read_sensor`
/// call takes.
///
/// Behind an [`Arc`] so that a [`ConcurrentSensor`] can hold a [`Weak`] to it and discover that the
/// driver is gone instead of calling into a closed context.
struct Context {
    ctx: *mut Ctx,
    vtable: &'static RawDriver,
    /// Held for the whole of every `read_sensor` call, whichever thread makes it. §89.1 promises a
    /// driver that `read_sensor` never runs concurrently with itself; this is the promise kept.
    sensor_lock: Mutex<()>,
    // Last, and load-bearing: fields drop after `Drop::drop` runs, so the context is closed before
    // the library it came from is unloaded.
    _library: Library,
}

impl Drop for Context {
    fn drop(&mut self) {
        if let Some(close) = self.vtable.close {
            if !self.ctx.is_null() {
                // SAFETY: `ctx` came from this vtable's `open`, is closed exactly once — `Context` is
                // not `Clone`, the one `Arc` that owns it is the `Driver`'s, and a
                // `ConcurrentSensor`'s `Weak` cannot keep it alive — and nothing is using it by the
                // time the last reference is dropped.
                unsafe { close(self.ctx) };
                self.ctx = std::ptr::null_mut();
            }
        }
    }
}

/// The one `Arc` a context lives in.
///
/// Clippy asks for `Context` to be `Send + Sync` before it is shared, and it deliberately is neither:
/// the `Driver` that holds this `Arc` must stay on the thread that opened it, which is what its raw
/// pointer field keeps true. The only thing that crosses a thread is a [`ConcurrentSensor`]'s `Weak`,
/// and its `Send` is argued where it is declared, clause by clause, rather than granted to every
/// user of the context by marking the context itself.
#[allow(clippy::arc_with_non_send_sync)]
fn shared(context: Context) -> Arc<Context> {
    Arc::new(context)
}

/// A second caller for one sensor's `read_sensor`, from a thread that is not the control thread
/// (§89.3).
///
/// Made only by [`Driver::concurrent_sensor`], and only for a sensor whose descriptor — read from
/// this driver's own `describe()` — declares `"concurrent_read": true` (§89.1). Everything this
/// handle may call is `read_sensor`, for that one sensor.
pub struct ConcurrentSensor {
    context: Weak<Context>,
    name: CString,
}

// SAFETY: this is the one `Send` in this crate, and every clause of it is held here rather than
// asserted by a caller.
//
// * **The driver promised it.** A `ConcurrentSensor` exists only after `concurrent_sensor` read the
//   driver's own descriptor and found `concurrent_read: true` on this sensor — §89.1's declaration
//   that `read_sensor` for it may run on another thread while `read_state` and `write_command` run.
// * **`read_sensor` never runs concurrently with itself.** Both routes to it take
//   `Context::sensor_lock` for the whole call, which is the half of §89.1 this OS promises back.
// * **Nothing else is called through it.** `read` is its only method, and it calls `read_sensor`.
// * **It cannot outlive the context.** It holds a `Weak`, upgraded for the length of one read; once
//   the `Driver` is dropped the upgrade fails and the read is refused as `Closed`. The context's
//   fields it reads through the upgrade — the pointer and the vtable — are never written after
//   `open`, and the lock is a `Mutex`. A `Driver` dropped while a read is in flight closes the
//   context when that read lets go of it: after the read and never during it, though on the capture
//   thread. `skynet-kernel-host` joins its capture thread before its driver drops, so that ordering
//   does not arise there.
unsafe impl Send for ConcurrentSensor {}

impl ConcurrentSensor {
    /// The sensor this handle reads.
    pub fn name(&self) -> &std::ffi::CStr {
        &self.name
    }

    /// Read this sensor's newest frame into `into`, by [`Driver::read_sensor`]'s rules.
    ///
    /// Blocks while the control thread is inside `read_sensor` for another sensor, which is the
    /// right side to wait: the capture thread has no deadline and the control thread does.
    pub fn read(&mut self, into: &mut [u8]) -> Result<SensorFrame, DriverError> {
        let context = self.context.upgrade().ok_or(DriverError::Closed)?;
        let read_sensor = context
            .vtable
            .read_sensor
            .ok_or(DriverError::Absent("read_sensor"))?;
        let _held = context
            .sensor_lock
            .lock()
            // The lock guards no data — only the fact that a call is in progress — so a thread that
            // panicked while holding it left nothing inconsistent behind.
            .unwrap_or_else(PoisonError::into_inner);
        read_frame(read_sensor, context.ctx, &self.name, into)
    }
}

/// One command whose joints are not all in the same mode (§86.1), for [`Driver::write_modal`].
///
/// The three arrays are indexed by the same joint count and the shortest decides it. `modes` holds
/// one [`skynet_rhal::jmode`] value per joint, and [`skynet_rhal::jmode::INHERIT`] is how a joint
/// says it takes `mode`.
pub struct ModalFrame<'a> {
    /// When the command was produced, not when it was sent.
    pub stamp_ns: u64,
    /// Must advance; a repeat is a stalled producer to the watchdog.
    pub seq: u32,
    /// The mode of every joint whose `modes` entry is `INHERIT`.
    pub mode: u8,
    /// Setpoints, in each joint's own units.
    pub values: &'a [f32],
    /// 0 = hold, non-zero = apply.
    pub mask: &'a [u8],
    /// `SKR_JMODE_*` per joint.
    pub modes: &'a [u8],
}

impl Driver {
    /// Load a shared object, check its vtable, and open a context.
    ///
    /// `config` is passed verbatim to the driver's `open()`.
    pub fn open(path: &Path, config: &str) -> Result<Driver, DriverError> {
        let library = Library::open(path).map_err(DriverError::Load)?;

        // SAFETY: the caller is asserting that this library was built against
        // `bindings/c/skynet_rhal.h`, which is what `entry()` documents and what `accepts()`
        // immediately re-checks by reading the ABI version out of the vtable it returns.
        let entry = unsafe { library.entry() }.map_err(DriverError::Load)?;

        // SAFETY: `entry` is a non-null function pointer of the RHAL entry signature. It takes no
        // arguments and returns a pointer to a vtable the driver owns for its own lifetime, which
        // is at least as long as the library's — this is the one call the ABI makes before any
        // context exists.
        let raw = unsafe { entry() };
        if raw.is_null() {
            return Err(DriverError::Load(DlError::MissingSymbol {
                symbol: skynet_rhal::ENTRY_SYMBOL.to_string(),
            }));
        }

        // SAFETY: non-null, and pointing at a `skr_driver` the driver keeps alive for the life of
        // the library. The `'static` is sound only because `_library` is held in the same struct
        // and dropped after this reference stops being used.
        let vtable: &'static RawDriver = unsafe { &*raw };

        accepts(vtable).map_err(DriverError::Rejected)?;

        let open = vtable.open.ok_or(DriverError::Absent("open"))?;
        let config = CString::new(config).unwrap_or_default();
        let mut ctx: *mut Ctx = std::ptr::null_mut();

        // SAFETY: `open` came from a vtable that passed `accepts`, the config pointer is a valid
        // NUL-terminated C string that outlives the call, and `ctx` is a valid out-parameter.
        let code = unsafe { open(config.as_ptr(), &mut ctx) };
        if code != 0 {
            return Err(DriverError::Open(code));
        }

        Ok(Driver {
            ctx,
            vtable,
            context: shared(Context {
                ctx,
                vtable,
                sensor_lock: Mutex::new(()),
                _library: library,
            }),
        })
    }

    /// How many joints the driver's descriptor declares, read once by opening and reading a state.
    ///
    /// A driver that cannot report a state has no joint count, and this returns zero rather than
    /// guessing — a guessed width is a buffer the driver will write past.
    pub fn probe_joints(&mut self) -> usize {
        let mut buffers = Buffers::new(skynet_ring_max_channels());
        self.read_state(&mut buffers, 0)
            .map_or(0, |sample| sample.joints)
    }

    /// Read one state sample into caller-owned buffers.
    pub fn read_state(&mut self, into: &mut Buffers, now_ns: u64) -> Result<Sample, DriverError> {
        let read_state = self
            .vtable
            .read_state
            .ok_or(DriverError::Absent("read_state"))?;

        let mut state = State {
            stamp_ns: 0,
            seq: 0,
            joint_count: into.position.len() as u16,
            flags: 0,
            position_rad: into.position.as_mut_ptr(),
            velocity_rad_s: into.velocity.as_mut_ptr(),
            effort_nm: into.effort.as_mut_ptr(),
            base_pose: [f32::NAN; 7],
            base_twist: [0.0; 6],
            fault_code: 0,
        };

        // SAFETY: `read_state` came from a vtable that passed `accepts`; `ctx` is the context that
        // vtable's `open` produced and has not been closed; and the three pointers address arrays of
        // exactly `joint_count` floats owned by `into`, which outlives the call. §24 forbids the
        // driver retaining them past it.
        let code = unsafe { read_state(self.ctx, &mut state, now_ns) };
        if code != 0 {
            return Err(DriverError::Call {
                entry: "read_state",
                code,
            });
        }

        Ok(Sample {
            stamp_ns: state.stamp_ns,
            seq: state.seq,
            // The driver may report fewer joints than the buffer holds. Trusting its number over
            // the buffer's would be trusting a driver to bound a read of our memory, so it is
            // clamped — a driver reporting more than it was given room for is a driver bug, and the
            // clamp makes it a short read rather than an overrun.
            joints: (state.joint_count as usize).min(into.position.len()),
            flags: state.flags,
            fault_code: state.fault_code,
            base_pose: state.base_pose,
            base_twist: state.base_twist,
        })
    }

    /// Write one command. The return is the driver's own code; negative is an `errno` (§24).
    pub fn write_command(
        &mut self,
        stamp_ns: u64,
        seq: u32,
        mode: u8,
        values: &[f32],
        mask: &[u8],
        now_ns: u64,
    ) -> Result<i32, DriverError> {
        let write_command = self
            .vtable
            .write_command
            .ok_or(DriverError::Absent("write_command"))?;

        // The two arrays are indexed by the same `joint_count`, so a mismatch would have the driver
        // read past the shorter one. Taking the minimum is the only safe reading of a disagreement
        // this side created.
        let joints = values.len().min(mask.len());

        // §86.1. This entry point takes one mode for the whole body and says so by leaving the
        // flag clear and the array null — which is exactly what an ABI-1 command is, and what every
        // driver written before §86 expects. A caller with a mixed body reaches for `write_modal`.
        let command = skynet_rhal::Command {
            stamp_ns,
            seq,
            mode,
            flags: 0,
            _pad: [0; 2],
            joint_count: joints as u16,
            values: values.as_ptr(),
            mask: mask.as_ptr(),
            modes: core::ptr::null(),
        };

        // SAFETY: `write_command` came from a vtable that passed `accepts`; `ctx` is live; and both
        // arrays address at least `joint_count` elements owned by the caller for the duration of
        // the call.
        Ok(unsafe { write_command(self.ctx, &command, now_ns) })
    }

    /// Write one command whose joints are not all in the same mode (§86.1).
    ///
    /// Takes a [`ModalFrame`] rather than six arguments, which is not only a lint: three arrays
    /// indexed by one joint count, passed positionally, is an invitation to swap `mask` and `modes`
    /// — two `&[u8]` of the same length whose meanings are unrelated.
    ///
    /// `modes` is one [`skynet_rhal::jmode`] value per joint, and
    /// [`skynet_rhal::jmode::INHERIT`] is how a joint says it takes `mode`. This is the entry point
    /// for a mobile manipulator — an arm on position, wheels on velocity — which had no
    /// representation at all before §86.
    ///
    /// **Only for a body whose descriptor declares `control.per_joint_modes`.** A driver written
    /// before §86 does not read the array, so a command it cannot see would be applied body-wide in
    /// whatever `mode` says, which is the silent half-commanded robot §86 exists to prevent.
    /// Transport will not produce a mixed plan for such a body, so in the ordinary path this cannot
    /// be reached with the wrong driver; this note is for the caller who builds a command by hand.
    pub fn write_modal(&mut self, frame: ModalFrame<'_>, now_ns: u64) -> Result<i32, DriverError> {
        let write_command = self
            .vtable
            .write_command
            .ok_or(DriverError::Absent("write_command"))?;

        // The minimum of all three, for the reason `write_command` takes the minimum of two: a
        // driver reading `joint_count` entries out of the shortest array is an overrun this side
        // created.
        let joints = frame
            .values
            .len()
            .min(frame.mask.len())
            .min(frame.modes.len());

        let command = skynet_rhal::Command {
            stamp_ns: frame.stamp_ns,
            seq: frame.seq,
            mode: frame.mode,
            flags: skynet_rhal::cmd_flags::PER_JOINT_MODES,
            _pad: [0; 2],
            joint_count: joints as u16,
            values: frame.values.as_ptr(),
            mask: frame.mask.as_ptr(),
            modes: frame.modes.as_ptr(),
        };

        // SAFETY: as `write_command` above, and `modes` addresses at least `joint_count` entries
        // for the duration of the call by the same argument.
        Ok(unsafe { write_command(self.ctx, &command, now_ns) })
    }

    /// Trigger the driver's emergency stop, when it has one.
    pub fn emergency_stop(&mut self) -> Result<(), DriverError> {
        let estop = self
            .vtable
            .emergency_stop
            .ok_or(DriverError::Absent("emergency_stop"))?;

        // SAFETY: from a vtable that passed `accepts`, called on a live context, taking nothing but
        // that context.
        let code = unsafe { estop(self.ctx) };
        if code != 0 {
            return Err(DriverError::Call {
                entry: "emergency_stop",
                code,
            });
        }
        Ok(())
    }

    /// The driver's embodiment descriptor, as JSON, when it publishes one.
    ///
    /// # The return value is a length, and from 2026-09-04 this read it as a status
    ///
    /// §4's contract, in the header's words: *"Returns bytes written, or SKR_EINVAL if `len` is too
    /// small — the OS then retries with a larger buffer."* This function used to treat any non-zero
    /// return as a failure, which is every descriptor a conforming driver has ever written — the
    /// mock returns its length, as the header says to. Nothing noticed because nothing called it:
    /// `skynet driver check` drives the vtable itself and reads the return correctly, and the one
    /// production path that needed a descriptor from a loaded driver did not exist until a control
    /// loop needed to know what its camera was.
    ///
    /// The retry is bounded — 4 KiB, growing fourfold to 1 MiB, the same walk the CLI takes — because
    /// a caller that grew the buffer until it fit would be letting a driver decide how much this
    /// process allocates.
    pub fn describe(&mut self) -> Result<String, DriverError> {
        let describe = self
            .vtable
            .describe
            .ok_or(DriverError::Absent("describe"))?;

        let mut size = 4096usize;
        loop {
            let mut buffer = vec![0u8; size];

            // SAFETY: from a vtable that passed `accepts`, live context, and a writable buffer of
            // exactly the length passed alongside it.
            let written =
                unsafe { describe(self.ctx, buffer.as_mut_ptr() as *mut c_char, buffer.len()) };

            if written == skynet_rhal::status::EINVAL && size < DESCRIBE_CEILING {
                size *= 4;
                continue;
            }
            if written <= 0 {
                return Err(DriverError::Call {
                    entry: "describe",
                    code: written,
                });
            }
            // Clamped, for `read_state`'s reason: a driver claiming to have written more than it
            // was given room for is a driver bug, and trusting its number would be trusting it to
            // bound a read of this process's memory.
            buffer.truncate((written as usize).min(size));
            return String::from_utf8(buffer).map_err(|_| DriverError::Malformed {
                entry: "describe",
                detail: "wrote bytes that are not UTF-8. The descriptor is JSON, and JSON is UTF-8"
                    .to_string(),
            });
        }
    }

    /// Whether the driver can read sensors at all, without calling it.
    pub fn has_sensors(&self) -> bool {
        self.vtable.read_sensor.is_some()
    }

    /// Whether this driver has the vendor escape hatch at all, without calling it.
    ///
    /// Asked before it is needed, for [`Driver::has_emergency_stop`]'s reason: §88's motion route
    /// ends here, and a loop that found out at the moment a motion arrived would have admitted a
    /// request it then could not deliver.
    pub fn has_extension(&self) -> bool {
        self.vtable.extension.is_some()
    }

    /// Call §4's vendor escape hatch, guarded by its capability token.
    ///
    /// §88.1's last step: the process holding the driver hands an **admitted** motion to the
    /// controller, and this is the call that does it. The request's bytes and the token's meaning
    /// are the vendor's — that is what an escape hatch is — and what this side owns is that the
    /// reply is a sentence somebody can read and that a driver's own length is never trusted to
    /// bound a read of this process's memory.
    ///
    /// The reply is returned whatever the code, because a refusal's sentence is the interesting
    /// half: §4 calls this the route a vendor's status word arrives on, and a controller's *"pose
    /// = 9.1 is outside the range this controller declares"* is worth more than the `-EINVAL`
    /// beside it. `Ok` carries the driver's own non-negative code; `Err` carries the negative one
    /// with whatever sentence came back.
    pub fn extension(&mut self, token: &std::ffi::CStr, request: &[u8]) -> ExtensionReply {
        let Some(extension) = self.vtable.extension else {
            return ExtensionReply {
                code: skynet_rhal::status::ENOSYS,
                detail: String::from(
                    "this driver implements no `extension`, so there is no route for a motion to \
                     reach its controller (§4, §88.1). A body commanded by named motions \
                     needs one.",
                ),
            };
        };

        let mut reply = vec![0u8; EXTENSION_REPLY_BYTES];
        // SAFETY: from a vtable that passed `accepts`, live context, a NUL-terminated token, and a
        // request and a reply buffer each passed with exactly its own length.
        let code = unsafe {
            extension(
                self.ctx,
                token.as_ptr(),
                request.as_ptr() as *const c_void,
                request.len(),
                reply.as_mut_ptr() as *mut c_void,
                reply.len(),
            )
        };

        // Whatever the driver wrote, up to the first NUL — the ABI's reply is a C string in a
        // caller's buffer, and a driver that filled the buffer without terminating it is the case
        // this truncation is for.
        let end = reply
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(reply.len());
        reply.truncate(end);
        ExtensionReply {
            code,
            detail: String::from_utf8_lossy(&reply).into_owned(),
        }
    }

    /// Read the named sensor's newest frame, copying its bytes into `into`.
    ///
    /// **Copied, not borrowed**, because the ABI says the driver's buffer is valid only until the
    /// next read of the same sensor. A borrow would be a lifetime this side could not express, and
    /// §47.3's zero-copy path is `dmabuf` — a V4L2 or GPU concern this entry point does not carry.
    /// `into` is the stream's declared slot (§47.5), so the copy never allocates.
    ///
    /// Three things the driver says are checked against what it hands back before any byte is
    /// trusted, each for the reason `skynet driver check` gives: a frame with no bytes, a header
    /// whose `stride × height` does not fit the bytes behind it, and a frame with no acquisition
    /// stamp. The last is §47.2's rule — a zero stamp is not an estimate, it is an absence, and
    /// substituting arrival time for it here is exactly what that section forbids.
    ///
    /// **It does not wait.** When a [`ConcurrentSensor`] is inside `read_sensor` on the capture
    /// thread this answers [`DriverError::Busy`] instead of calling in beside it: §89.1 promises the
    /// driver that `read_sensor` never runs concurrently with itself, and the caller of this method
    /// is the control thread, which has a deadline.
    pub fn read_sensor(
        &mut self,
        name: &CStr,
        into: &mut [u8],
    ) -> Result<SensorFrame, DriverError> {
        let read_sensor = self
            .vtable
            .read_sensor
            .ok_or(DriverError::Absent("read_sensor"))?;
        let _held = match self.context.sensor_lock.try_lock() {
            Ok(held) => held,
            // Guards no data, so a panic while it was held left nothing inconsistent.
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(DriverError::Busy("read_sensor")),
        };
        read_frame(read_sensor, self.ctx, name, into)
    }

    /// A handle that reads one sensor from another thread, **if and only if** this driver's own
    /// descriptor declares `"concurrent_read": true` on it (§89.1).
    ///
    /// The declaration is read here, from `describe()`, through the same decoder a robot uses —
    /// rather than taken from a caller — because it is the whole justification for the `Send` on
    /// [`ConcurrentSensor`], and a justification a caller supplies is one a caller can get wrong. A
    /// sensor whose declaration is absent, `false`, or anything but the boolean is refused as
    /// [`DriverError::Undeclared`], and is read on the control thread instead.
    pub fn concurrent_sensor(&mut self, name: &str) -> Result<ConcurrentSensor, DriverError> {
        if self.vtable.read_sensor.is_none() {
            return Err(DriverError::Absent("read_sensor"));
        }
        let undeclared = |detail: &str| DriverError::Undeclared {
            sensor: name.to_string(),
            detail: detail.to_string(),
        };
        let text = self.describe()?;
        let json = skynet_json::Json::parse(&text)
            .map_err(|_| undeclared("the driver's descriptor is not JSON"))?;
        let descriptor = skynet_contracts::json::descriptor_from_json(&json)
            .map_err(|_| undeclared("the driver's descriptor does not decode"))?;
        let Some(sensor) = descriptor.sensors.iter().find(|sensor| sensor.name == name) else {
            return Err(undeclared(
                "the driver's descriptor declares no sensor by that name",
            ));
        };
        if sensor.concurrent_read != Some(true) {
            return Err(undeclared(
                "the driver's descriptor does not declare it `concurrent_read`",
            ));
        }
        let name = CString::new(name).map_err(|_| undeclared("the name contains a NUL byte"))?;
        Ok(ConcurrentSensor {
            context: Arc::downgrade(&self.context),
            name,
        })
    }

    /// Whether the driver exposes an emergency stop, without calling it.
    pub fn has_emergency_stop(&self) -> bool {
        self.vtable.emergency_stop.is_some()
    }

    /// The ABI version the loaded driver declared, after [`accepts`] verified it.
    ///
    /// Power-on self-test re-reads it rather than assuming the load gate ran: §48.2's first row
    /// is a record that the check happened, and a record of a field nobody read is a claim.
    pub fn abi_version(&self) -> u32 {
        self.vtable.abi_version
    }

    /// Whether the driver can release a stop without renewing the session, without calling it.
    ///
    /// Asked before it is needed, for [`Driver::has_emergency_stop`]'s reason: power-on self-test
    /// asserts the stop either way, and what it does afterwards — release in place, or close and
    /// reopen — depends on this.
    pub fn has_release_stop(&self) -> bool {
        self.vtable.release_stop.is_some()
    }

    /// Clear the session latch `emergency_stop` set, when the driver has a software release.
    ///
    /// `Absent` when the vtable carries no `release_stop`: a body whose stop is latched by its
    /// own circuit or drives, where only a person resetting the hardware returns the session.
    /// That is a fact for the caller to route around — close, open, and read what the new
    /// session reports — and not an error in the driver.
    pub fn release_stop(&mut self) -> Result<(), DriverError> {
        let release = self
            .vtable
            .release_stop
            .ok_or(DriverError::Absent("release_stop"))?;

        // SAFETY: from a vtable that passed `accepts`, called on a live context, taking nothing but
        // that context.
        let code = unsafe { release(self.ctx) };
        if code != 0 {
            return Err(DriverError::Call {
                entry: "release_stop",
                code,
            });
        }
        Ok(())
    }
}

/// The type of the vtable's `read_sensor` entry, once it is known to be there.
type ReadSensor = unsafe extern "C" fn(*mut Ctx, *const c_char, *mut skynet_rhal::Frame) -> i32;

/// One `read_sensor` call and the checks on what it handed back, for both routes to it.
///
/// The caller holds `Context::sensor_lock` for the whole call, which is what makes the copy below
/// sound: the driver's buffer is valid until the next read of the sensor, and no other read can
/// begin until this one lets go of the lock.
fn read_frame(
    read_sensor: ReadSensor,
    ctx: *mut Ctx,
    name: &CStr,
    into: &mut [u8],
) -> Result<SensorFrame, DriverError> {
    let mut frame = skynet_rhal::Frame {
        stamp_ns: 0,
        width: 0,
        height: 0,
        stride: 0,
        format: 0,
        data: std::ptr::null(),
        size: 0,
    };

    // SAFETY: from a vtable that passed `accepts`, called on a live context — the caller holds the
    // `Driver` or an upgraded `Weak` to its context; `name` is NUL-terminated and outlives the call;
    // `frame` is a valid out-parameter the driver fills.
    let code = unsafe { read_sensor(ctx, name.as_ptr(), &mut frame) };
    if code != 0 {
        return Err(DriverError::Call {
            entry: "read_sensor",
            code,
        });
    }

    let malformed = |detail: String| DriverError::Malformed {
        entry: "read_sensor",
        detail,
    };
    if frame.data.is_null() || frame.size == 0 {
        return Err(malformed("returned an empty frame".to_string()));
    }
    let described = (frame.stride as usize).saturating_mul(frame.height as usize);
    if described > frame.size {
        return Err(malformed(format!(
            "described {}x{} at stride {} — {described} bytes — and handed back {}",
            frame.width, frame.height, frame.stride, frame.size
        )));
    }
    if frame.stamp_ns == 0 {
        return Err(malformed(
            "returned a frame with no acquisition stamp (§47.2)".to_string(),
        ));
    }
    if frame.size > into.len() {
        return Err(DriverError::Oversized {
            needed: frame.size,
            capacity: into.len(),
        });
    }

    // SAFETY: `data` is non-null and the driver asserts `size` readable bytes behind it, valid
    // until the next read of this sensor — which cannot begin before this copy finishes, because
    // the caller holds the sensor lock for the whole call. `into` was checked to hold them.
    let bytes = unsafe { std::slice::from_raw_parts(frame.data as *const u8, frame.size) };
    into[..frame.size].copy_from_slice(bytes);

    Ok(SensorFrame {
        stamp_ns: frame.stamp_ns,
        width: frame.width,
        height: frame.height,
        stride: frame.stride,
        format: frame.format,
        len: frame.size,
    })
}

/// The largest descriptor [`Driver::describe`] will make room for: 1 MiB, after four retries.
const DESCRIBE_CEILING: usize = 1 << 20;

/// Bytes a driver's extension reply may occupy.
///
/// Four kilobytes, and fixed rather than grown the way `describe`'s buffer is: `describe` answers a
/// document whose size is a property of the body, and this answers one sentence. A controller that
/// needs more than four kilobytes to say why it refused a motion has a different problem from the
/// one a larger buffer solves.
const EXTENSION_REPLY_BYTES: usize = 4096;

/// What §4's escape hatch answered.
///
/// The code and the sentence together, because on this route they are one fact: a vendor's numeric
/// status is exactly the thing §88's refusals exist to turn back into something an integrator can
/// act on, and a caller handed only the number would be re-deriving the sentence from a table it
/// does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionReply {
    /// The driver's own code. Negative is an `errno` (§24).
    pub code: i32,
    /// Whatever the driver wrote, up to the first NUL. Empty is a driver that said nothing.
    pub detail: String,
}

impl ExtensionReply {
    /// Whether the controller took the motion.
    pub fn accepted(&self) -> bool {
        self.code >= 0
    }
}

/// The widest command frame the ring carries, which is the most joints a probe can make room for.
fn skynet_ring_max_channels() -> usize {
    128
}

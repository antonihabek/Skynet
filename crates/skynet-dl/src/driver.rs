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

use std::ffi::{c_char, CString};
use std::path::Path;

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
/// Closing is [`Drop`], and the library is held alongside the context so it cannot be unloaded
/// while the context is live — unloading under a live context is what turns a clean failure into a
/// crash in somebody else's `atexit` handler.
// No `Debug`: the vtable is a C struct of raw function pointers and does not implement it, and a
// hand-written one would print addresses that mean nothing to a reader.
pub struct Driver {
    // Field order is the drop order, and it is load-bearing: the context is closed before the
    // library it came from is unloaded.
    ctx: *mut Ctx,
    vtable: &'static RawDriver,
    _library: Library,
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
            _library: library,
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

        let command = skynet_rhal::Command {
            stamp_ns,
            seq,
            mode,
            _pad: [0; 3],
            joint_count: joints as u16,
            values: values.as_ptr(),
            mask: mask.as_ptr(),
        };

        // SAFETY: `write_command` came from a vtable that passed `accepts`; `ctx` is live; and both
        // arrays address at least `joint_count` elements owned by the caller for the duration of
        // the call.
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
    pub fn read_sensor(
        &mut self,
        name: &std::ffi::CStr,
        into: &mut [u8],
    ) -> Result<SensorFrame, DriverError> {
        let read_sensor = self
            .vtable
            .read_sensor
            .ok_or(DriverError::Absent("read_sensor"))?;

        let mut frame = skynet_rhal::Frame {
            stamp_ns: 0,
            width: 0,
            height: 0,
            stride: 0,
            format: 0,
            data: std::ptr::null(),
            size: 0,
        };

        // SAFETY: from a vtable that passed `accepts`, called on a live context; `name` is
        // NUL-terminated and outlives the call; `frame` is a valid out-parameter the driver fills.
        let code = unsafe { read_sensor(self.ctx, name.as_ptr(), &mut frame) };
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
        // until the next read of this sensor — which cannot happen before this copy finishes,
        // because `self` is borrowed mutably for the whole call. `into` was checked to hold them.
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

    /// Whether the driver exposes an emergency stop, without calling it.
    pub fn has_emergency_stop(&self) -> bool {
        self.vtable.emergency_stop.is_some()
    }
}

impl Drop for Driver {
    fn drop(&mut self) {
        if let Some(close) = self.vtable.close {
            if !self.ctx.is_null() {
                // SAFETY: `ctx` came from this vtable's `open`, is closed exactly once — `Driver` is
                // not `Clone` and owns it — and nothing else holds a reference to it by this point.
                unsafe { close(self.ctx) };
                self.ctx = std::ptr::null_mut();
            }
        }
    }
}

/// The largest descriptor [`Driver::describe`] will make room for: 1 MiB, after four retries.
const DESCRIBE_CEILING: usize = 1 << 20;

/// The widest command frame the ring carries, which is the most joints a probe can make room for.
fn skynet_ring_max_channels() -> usize {
    128
}

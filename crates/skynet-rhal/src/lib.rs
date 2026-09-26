//! RHAL — the Rust view of the vendor interface.
//!
//! `bindings/c/skynet_rhal.h` is the interface. This crate is not a second definition of it: it is
//! the same struct layout expressed in Rust so that a driver written in Rust, and the OS that
//! loads one, can talk about the same bytes. §31.2 lists the RHAL C ABI among the things that stay
//! single-source no matter how many language editions exist, "because two definitions of a struct
//! layout is a segfault" — so every type here is `#[repr(C)]` and every field is in header order,
//! and [`tests::layout_matches_the_header`] is what keeps that claim true rather than aspirational.
//!
//! # Why this exists at all, when the header does
//!
//! Two audiences.
//!
//! * **A Rust driver author** wants [`Driver`], a safe trait, and [`export_driver!`], which builds
//!   the `skynet_rhal_entry` symbol and the vtable for them. They never write `unsafe`, never see a
//!   raw pointer, and cannot forget to set `abi_version`.
//! * **The OS loading a `.so`** wants [`RawDriver`], the vtable exactly as C declares it, so it can
//!   check `abi_version` before calling a single function through it.
//!
//! # What this crate deliberately does not do
//!
//! It does not load shared objects. `dlopen` is `skynetd`'s job (§33), it needs a filesystem and a
//! signature check, and putting it here would drag std into a crate the kernel-side build wants to
//! keep `no_std`.

#![no_std]

use core::ffi::{c_char, c_int, c_void};

/// The ABI version this crate describes. Mirrors `SKYNET_RHAL_ABI_VERSION`.
///
/// Bumped only for a breaking change. The OS refuses a driver whose major does not match rather
/// than attempting a partial load — half a hardware driver is worse than none.
///
/// v2 appends [`RawDriver::release_stop`]. The bump is forced by the layout: the vtable is the
/// driver's own static struct, so a v1 table read as v2 would have its first word past the end
/// taken for a function pointer. Spare bits and padding carried the last two evolutions (§27's
/// homing field, §86.1's mode array); a vtable has neither.
pub const ABI_VERSION: u32 = 2;

/// Return codes. Negative `errno`-style, mirroring `SKR_*` in the header.
///
/// The distinction between them is recorded in the black box (§26), because "the drive is faulted"
/// and "the bus timed out" lead to completely different investigations.
pub mod status {
    #![allow(missing_docs)]
    use core::ffi::c_int;

    pub const OK: c_int = 0;
    /// Drive disabled, or the command is not permitted in this mode.
    pub const EPERM: c_int = -1;
    /// Bus or transport failure.
    pub const EIO: c_int = -5;
    /// The named joint, sensor or device does not exist.
    pub const ENODEV: c_int = -19;
    /// Malformed request — a wrong `joint_count`, an unknown mode.
    pub const EINVAL: c_int = -22;
    /// The driver does not implement this optional entry point.
    pub const ENOSYS: c_int = -38;
    /// The device answered, and the answer was not valid.
    pub const EPROTO: c_int = -71;
    /// The deadline passed before the operation completed.
    pub const ETIMEDOUT: c_int = -110;
}

/// State flags, mirroring `SKR_FLAG_*`.
pub mod flags {
    #![allow(missing_docs)]
    /// An emergency stop is asserted.
    pub const ESTOP: u16 = 1 << 0;
    /// The drive has faulted; commands are being ignored.
    pub const FAULT: u16 = 1 << 1;
    /// Running, but something is wrong — thermal, a dead encoder.
    pub const DEGRADED: u16 = 1 << 2;
    /// Holding brakes are engaged.
    pub const BRAKED: u16 = 1 << 3;

    /// Where the homing field starts. Mirrors `SKR_HOMING_SHIFT`.
    pub const HOMING_SHIFT: u16 = 4;
    /// The two bits the homing field occupies. Mirrors `SKR_HOMING_MASK`.
    pub const HOMING_MASK: u16 = 0b11 << HOMING_SHIFT;
}

/// §27's homing condition, as a driver reports it in [`State::flags`].
///
/// # Why this is in the flags word and did not need an ABI break
///
/// §73 carried `homing` as the last of §27's eight conditions for as long as this enum did not
/// exist, with the reason *"§24 gives the ABI no flag for it, so it needs a vendor to recompile
/// rather than a daemon to look somewhere else"*. The first half of that was true and the second
/// half was the wrong conclusion drawn from it. `flags` is a `u16` with twelve bits spare and it is
/// already a bitwise-or, so a field can be added to it without moving a byte: no offset changes,
/// [`ABI_VERSION`] does not bump, and every driver ever compiled against v1 keeps loading. What a
/// vendor has to recompile for is *answering*, which is a different and much smaller claim.
///
/// # Four states, because zero is not `Pending`
///
/// A single `SKR_FLAG_HOMED` bit would have been the obvious encoding and it conflates the two
/// facts that matter most: a clear bit would mean *this robot is not homed* and *this driver has
/// never heard of homing* at once. Those send an operator to different places — one to the homing
/// routine, one to the vendor — and §7.2's rule is that a measurement nobody can attribute is not
/// one. Two bits carry the [`Option`] on the wire that the type carries in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Homing {
    /// `SKR_HOMING_NOT_REQUIRED`. Absolute encoders; there is nothing to home.
    NotRequired = 1,
    /// `SKR_HOMING_PENDING`. Joint positions are not yet trustworthy.
    Pending = 2,
    /// `SKR_HOMING_COMPLETE`. Homed, and the positions mean what they say.
    Complete = 3,
}

impl Homing {
    /// Read the field out of a [`State::flags`] word.
    ///
    /// `None` is `SKR_HOMING_UNREPORTED`, which is what every v1 driver reports by doing nothing.
    /// It is deliberately not [`Homing::Pending`]: the OS declines to command on both, and it says
    /// which, and the two have different remedies.
    pub fn from_flags(flags: u16) -> Option<Homing> {
        match flags & flags::HOMING_MASK {
            0 => None,
            value if value == (Homing::NotRequired as u16) << flags::HOMING_SHIFT => {
                Some(Homing::NotRequired)
            }
            value if value == (Homing::Pending as u16) << flags::HOMING_SHIFT => {
                Some(Homing::Pending)
            }
            _ => Some(Homing::Complete),
        }
    }

    /// The bits this state occupies in a flags word, for a driver assembling one.
    pub fn to_flags(self) -> u16 {
        (self as u16) << flags::HOMING_SHIFT
    }

    /// The wire spelling, which is §27's.
    pub fn as_str(self) -> &'static str {
        match self {
            Homing::NotRequired => "not_required",
            Homing::Pending => "pending",
            Homing::Complete => "complete",
        }
    }
}

/// Control modes, mirroring `SKR_MODE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Mode {
    /// Values are radians, or metres for a prismatic joint.
    Position = 0,
    /// Values are rad/s, or m/s.
    Velocity = 1,
    /// Values are Nm, or N.
    Torque = 2,
}

impl Mode {
    /// Decode a wire value. Unknown modes are `None` rather than a default, because defaulting an
    /// unrecognised mode to position would send position values to a torque-controlled joint.
    pub fn from_u8(value: u8) -> Option<Mode> {
        match value {
            0 => Some(Mode::Position),
            1 => Some(Mode::Velocity),
            2 => Some(Mode::Torque),
            _ => None,
        }
    }

    /// The per-joint spelling of this mode, `SKR_JMODE_*` (§86.1).
    pub fn as_joint_mode(self) -> u8 {
        jmode::OF_BASE | (self as u8)
    }

    /// Decode one entry of [`Command::modes`].
    ///
    /// `Ok(None)` is `SKR_JMODE_INHERIT` — take the command's own `mode`. `Err(())` is an entry
    /// this ABI does not define, which a driver reports as `SKR_EINVAL` rather than guessing: the
    /// value it is most likely to be is a bare `SKR_MODE_*` written into the wrong array, and that
    /// is precisely the transcription error the offset exists to catch.
    #[allow(clippy::result_unit_err)]
    pub fn from_joint_mode(value: u8) -> Result<Option<Mode>, ()> {
        if value == jmode::INHERIT {
            return Ok(None);
        }
        if value & 0xf0 != jmode::OF_BASE {
            return Err(());
        }
        Mode::from_u8(value & 0x0f).map(Some).ok_or(())
    }
}

/// Per-joint control-mode values, mirroring `SKR_JMODE_*` (§86.1).
///
/// Deliberately not `SKR_MODE_*`: zero is `INHERIT` because zero is what a `calloc` gives you, and
/// a producer that allocated the array and forgot to fill it must read as *the mode the command
/// already carries* rather than as "every joint on position". The offset is what makes the mistake
/// that arrangement invites loud — a bare `Mode` discriminant written here is not a valid entry at
/// all, so it is refused instead of moving a joint in the wrong units.
pub mod jmode {
    /// Take [`super::Command::mode`].
    pub const INHERIT: u8 = 0x00;
    /// The high nibble every real mode carries.
    pub const OF_BASE: u8 = 0x10;
    /// Radians, or metres for a prismatic joint.
    pub const POSITION: u8 = 0x10;
    /// Radians per second, or metres per second.
    pub const VELOCITY: u8 = 0x11;
    /// Newton-metres, or newtons.
    pub const TORQUE: u8 = 0x12;
}

/// Command flags, mirroring `SKR_CMD_*` (§86.1).
pub mod cmd_flags {
    /// [`super::Command::modes`] is present and readable.
    ///
    /// The byte this lives in was `_pad[0]` — explicit padding at a fixed offset, zero in every
    /// command any producer has ever written — which is §27's homing precedent one field along: no
    /// offset moves, the ABI version does not bump, and a v1 driver keeps working.
    pub const PER_JOINT_MODES: u8 = 1 << 0;
}

/// Sensor frame pixel formats, mirroring `SKR_FMT_*`.
pub mod format {
    #![allow(missing_docs)]
    pub const RGB8: u32 = 0;
    pub const BGR8: u32 = 1;
    pub const DEPTH16: u32 = 2;
    pub const DEPTH32F: u32 = 3;
    pub const JPEG: u32 = 4;
    pub const POINTS32F: u32 = 5;
}

/// Opaque driver context. Only the driver knows what is behind this pointer.
#[repr(C)]
pub struct Ctx {
    _private: [u8; 0],
}

/// One sample of the robot's state. Mirrors `skr_state`.
///
/// Every array is caller-allocated and reused across cycles, so the hot path never reaches an
/// allocator. The driver fills; it never frees, resizes, or retains a pointer past the call.
#[repr(C)]
#[derive(Debug)]
pub struct State {
    /// `CLOCK_MONOTONIC_RAW` **at acquisition**, never at return. A timestamp applied on return
    /// folds the driver's own latency into the measurement and silently corrupts every downstream
    /// budget (§24).
    pub stamp_ns: u64,
    /// Sample sequence number.
    pub seq: u32,
    /// Length of the three per-joint arrays.
    pub joint_count: u16,
    /// Bitwise-or of [`flags`].
    pub flags: u16,
    /// `[joint_count]` positions.
    pub position_rad: *mut f32,
    /// `[joint_count]` velocities; `NaN` where the joint does not measure it.
    pub velocity_rad_s: *mut f32,
    /// `[joint_count]` efforts; `NaN` where unmeasured.
    pub effort_nm: *mut f32,
    /// `x y z qx qy qz qw`; all `NaN` when the base is not localised.
    pub base_pose: [f32; 7],
    /// `vx vy vz wx wy wz`.
    pub base_twist: [f32; 6],
    /// Vendor-specific; surfaced verbatim to the black box.
    pub fault_code: u32,
}

/// One command. Mirrors `skr_command`.
///
/// `mask` is the field worth reading twice. A zero means *hold this joint*, and it is how the OS
/// says nothing rather than saying zero — on a torque-controlled joint that is the difference
/// between holding position and going limp. It is also where a `drop` binding from the transport
/// plan meets the wire (§24).
#[repr(C)]
#[derive(Debug)]
pub struct Command {
    /// When the command was produced, not when it was sent.
    pub stamp_ns: u64,
    /// Must advance. A repeat is treated as a stalled producer by the watchdog guard.
    pub seq: u32,
    /// A [`Mode`] discriminant: the mode of every joint that inherits.
    pub mode: u8,
    /// [`cmd_flags`]. Was `_pad[0]`, and zero in every ABI-1 command (§86.1).
    pub flags: u8,
    /// Explicit padding, so the Rust and C layouts agree without relying on either compiler's
    /// choices.
    pub _pad: [u8; 2],
    /// Length of `values`, `mask` and `modes`.
    pub joint_count: u16,
    /// `[joint_count]`; units follow this joint's mode.
    pub values: *const f32,
    /// `[joint_count]`; 0 = hold, non-zero = apply.
    pub mask: *const u8,
    /// `[joint_count]` of [`jmode`] values (§86.1).
    ///
    /// Read **only** when `flags` carries [`cmd_flags::PER_JOINT_MODES`]: a command from a producer
    /// that predates §86 does not have this field, and reading it would be reading past the struct
    /// that producer laid out.
    pub modes: *const u8,
}

/// One sensor frame. Mirrors `skr_frame`.
///
/// `data` is driver-owned and valid until the next read of the same sensor.
#[repr(C)]
#[derive(Debug)]
pub struct Frame {
    /// Acquisition timestamp, on the same terms as [`State::stamp_ns`].
    pub stamp_ns: u64,
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
    /// Bytes per row.
    pub stride: u32,
    /// One of [`format`].
    pub format: u32,
    /// Frame bytes.
    pub data: *const c_void,
    /// Length of `data` in bytes.
    pub size: usize,
}

/// The driver vtable, exactly as C declares it. Mirrors `skr_driver`.
///
/// Optional entry points may be null; the OS reports the capability as absent rather than calling
/// through a null pointer.
#[repr(C)]
pub struct RawDriver {
    /// Must equal [`ABI_VERSION`]. Checked at load, before anything else is called.
    pub abi_version: u32,
    /// NUL-terminated driver name.
    pub driver_name: *const c_char,
    /// NUL-terminated driver version.
    pub driver_version: *const c_char,

    /// Open the device from a JSON configuration string.
    pub open: Option<unsafe extern "C" fn(*const c_char, *mut *mut Ctx) -> c_int>,
    /// Close and release everything `open` acquired.
    pub close: Option<unsafe extern "C" fn(*mut Ctx) -> c_int>,

    /// Real-time path. Wait-free, no allocation, must respect `deadline_ns`.
    pub read_state: Option<unsafe extern "C" fn(*mut Ctx, *mut State, u64) -> c_int>,
    /// Real-time path, same rules.
    pub write_command: Option<unsafe extern "C" fn(*mut Ctx, *const Command, u64) -> c_int>,

    /// Must be callable from a signal handler and from a failing process. Never blocks.
    pub emergency_stop: Option<unsafe extern "C" fn(*mut Ctx) -> c_int>,

    /// Writes the embodiment descriptor as JSON. Returns bytes written, or `EINVAL` when `len` is
    /// too small — the OS then retries with a larger buffer rather than truncating a body
    /// description, because a half-parsed descriptor is worse than a failed one.
    pub describe: Option<unsafe extern "C" fn(*mut Ctx, *mut c_char, usize) -> c_int>,

    /// Optional. Null when the robot exposes no sensors through the RHAL.
    pub read_sensor: Option<unsafe extern "C" fn(*mut Ctx, *const c_char, *mut Frame) -> c_int>,

    /// Optional. The vendor escape hatch, guarded by a capability token so a policy using a
    /// manufacturer feature declares the dependency rather than reaching for it.
    pub extension: Option<
        unsafe extern "C" fn(
            *mut Ctx,
            *const c_char,
            *const c_void,
            usize,
            *mut c_void,
            usize,
        ) -> c_int,
    >,

    /// Optional since v2. Clears the session latch `emergency_stop` set, so power-on self-test
    /// can prove the stop path and hand the loop a commandable session (§48.2).
    ///
    /// `None` when the driver has no software release — a body whose stop is latched by its own
    /// circuit or drives. The OS then proves the path by asserting and observing, and renews the
    /// session instead; see the header, which carries the whole contract.
    pub release_stop: Option<unsafe extern "C" fn(*mut Ctx) -> c_int>,
}

// A vtable is a table of function pointers to code the loader has already verified and is shared
// immutably across threads. The `*const c_char` name fields point at string literals with static
// lifetime. Neither is mutated after construction.
unsafe impl Sync for RawDriver {}

/// The signature of the one symbol a driver shared object must export.
pub type EntryFn = unsafe extern "C" fn() -> *const RawDriver;

/// The symbol name the OS looks up with `dlsym`.
pub const ENTRY_SYMBOL: &str = "skynet_rhal_entry";

/// Whether a vtable may be called at all.
///
/// Two checks, and the order matters: the version is read first because it is the only field whose
/// meaning is guaranteed across ABI versions. A v2 driver may have moved every other field.
pub fn accepts(driver: &RawDriver) -> Result<(), LoadError> {
    if driver.abi_version != ABI_VERSION {
        return Err(LoadError::AbiMismatch {
            expected: ABI_VERSION,
            found: driver.abi_version,
        });
    }
    // The four entry points with no meaningful fallback. A driver missing `emergency_stop` is one
    // whose stop path does not exist, which is the single worst thing to discover late.
    if driver.open.is_none() {
        return Err(LoadError::MissingEntryPoint("open"));
    }
    if driver.read_state.is_none() {
        return Err(LoadError::MissingEntryPoint("read_state"));
    }
    if driver.write_command.is_none() {
        return Err(LoadError::MissingEntryPoint("write_command"));
    }
    if driver.emergency_stop.is_none() {
        return Err(LoadError::MissingEntryPoint("emergency_stop"));
    }
    if driver.describe.is_none() {
        return Err(LoadError::MissingEntryPoint("describe"));
    }
    Ok(())
}

/// Why a vtable was refused. Both variants name the fix, per §64.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// The driver was built against a different ABI major.
    AbiMismatch {
        /// What this OS speaks.
        expected: u32,
        /// What the driver declared.
        found: u32,
    },
    /// A mandatory entry point is null.
    MissingEntryPoint(&'static str),
}

impl core::fmt::Display for LoadError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LoadError::AbiMismatch { expected, found } => write!(
                f,
                "driver targets RHAL ABI {found}; this OS implements {expected}. Rebuild the driver against skynet_rhal.h v{expected}."
            ),
            LoadError::MissingEntryPoint(name) => {
                write!(f, "driver does not implement the mandatory entry point `{name}`")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{align_of, offset_of, size_of};

    /// The claim in §31.2 is that the ABI is single-source. This is what keeps it true.
    ///
    /// The offsets below are the ones a C compiler produces for `skr_state`, `skr_command` and
    /// `skr_frame` on any LP64 or LLP64 target, computed by hand from the header's field order and
    /// the natural alignment rules. `drivers/mock` asserts the same numbers from the C side with
    /// `offsetof`, so the two halves of the check meet in the middle: if either language's layout
    /// moves, one of the two suites fails.
    #[test]
    fn layout_matches_the_header() {
        // skr_state: u64, u32, u16, u16, three pointers, [f32;7], [f32;6], u32
        assert_eq!(offset_of!(State, stamp_ns), 0);
        assert_eq!(offset_of!(State, seq), 8);
        assert_eq!(offset_of!(State, joint_count), 12);
        assert_eq!(offset_of!(State, flags), 14);
        assert_eq!(offset_of!(State, position_rad), 16);
        assert_eq!(offset_of!(State, velocity_rad_s), 16 + size_of::<usize>());
        assert_eq!(offset_of!(State, effort_nm), 16 + 2 * size_of::<usize>());
        assert_eq!(offset_of!(State, base_pose), 16 + 3 * size_of::<usize>());
        assert_eq!(
            offset_of!(State, base_twist),
            16 + 3 * size_of::<usize>() + 28
        );
        assert_eq!(
            offset_of!(State, fault_code),
            16 + 3 * size_of::<usize>() + 52
        );

        // skr_command: u64, u32, u8, u8, [u8;2], u16, then three pointers on their natural
        // alignment. §86.1 renamed the first padding byte to `flags` and appended `modes`, which
        // moves nothing before it — the point of putting the flag in padding that already existed.
        assert_eq!(offset_of!(Command, stamp_ns), 0);
        assert_eq!(offset_of!(Command, seq), 8);
        assert_eq!(offset_of!(Command, mode), 12);
        assert_eq!(offset_of!(Command, flags), 13);
        assert_eq!(offset_of!(Command, _pad), 14);
        assert_eq!(offset_of!(Command, joint_count), 16);
        assert_eq!(offset_of!(Command, values), 24);
        assert_eq!(offset_of!(Command, mask), 24 + size_of::<usize>());
        assert_eq!(offset_of!(Command, modes), 24 + 2 * size_of::<usize>());

        // skr_frame: u64, four u32, pointer, size_t
        assert_eq!(offset_of!(Frame, stamp_ns), 0);
        assert_eq!(offset_of!(Frame, width), 8);
        assert_eq!(offset_of!(Frame, height), 12);
        assert_eq!(offset_of!(Frame, stride), 16);
        assert_eq!(offset_of!(Frame, format), 20);
        assert_eq!(offset_of!(Frame, data), 24);
        assert_eq!(offset_of!(Frame, size), 24 + size_of::<usize>());

        // The vtable is a version word padded to a pointer, two name pointers, then the nine
        // entry points — twelve pointer-sized slots. `release_stop` is appended after `extension`
        // (v2), so nothing before it moved — which is what lets a C driver initialise positionally
        // and still land every earlier entry point.
        assert_eq!(offset_of!(RawDriver, abi_version), 0);
        assert_eq!(offset_of!(RawDriver, driver_name), size_of::<usize>());
        assert_eq!(offset_of!(RawDriver, open), 3 * size_of::<usize>());
        assert_eq!(offset_of!(RawDriver, extension), 10 * size_of::<usize>());
        assert_eq!(offset_of!(RawDriver, release_stop), 11 * size_of::<usize>());
        assert_eq!(size_of::<RawDriver>(), 12 * size_of::<usize>());
        assert_eq!(align_of::<RawDriver>(), align_of::<usize>());
    }

    /// A null function pointer must occupy the same slot as a real one, or `Option` would change
    /// the layout and the vtable would no longer be the C one.
    #[test]
    fn optional_fn_pointers_are_niche_optimised() {
        assert_eq!(
            size_of::<Option<unsafe extern "C" fn(*mut Ctx) -> c_int>>(),
            size_of::<usize>()
        );
    }

    fn stub_vtable() -> RawDriver {
        unsafe extern "C" fn open(_: *const c_char, _: *mut *mut Ctx) -> c_int {
            status::OK
        }
        unsafe extern "C" fn read_state(_: *mut Ctx, _: *mut State, _: u64) -> c_int {
            status::OK
        }
        unsafe extern "C" fn write_command(_: *mut Ctx, _: *const Command, _: u64) -> c_int {
            status::OK
        }
        unsafe extern "C" fn estop(_: *mut Ctx) -> c_int {
            status::OK
        }
        unsafe extern "C" fn describe(_: *mut Ctx, _: *mut c_char, _: usize) -> c_int {
            status::OK
        }
        RawDriver {
            abi_version: ABI_VERSION,
            driver_name: c"stub".as_ptr(),
            driver_version: c"0.0.0".as_ptr(),
            open: Some(open),
            close: None,
            read_state: Some(read_state),
            write_command: Some(write_command),
            emergency_stop: Some(estop),
            describe: Some(describe),
            read_sensor: None,
            extension: None,
            release_stop: None,
        }
    }

    #[test]
    fn a_complete_v2_driver_is_accepted() {
        assert_eq!(accepts(&stub_vtable()), Ok(()));
    }

    #[test]
    fn a_future_abi_is_refused_rather_than_partially_loaded() {
        let mut driver = stub_vtable();
        driver.abi_version = ABI_VERSION + 1;
        assert_eq!(
            accepts(&driver),
            Err(LoadError::AbiMismatch {
                expected: ABI_VERSION,
                found: ABI_VERSION + 1,
            })
        );
    }

    /// The direction that matters now that v2 exists: a v1 table is one pointer shorter, so
    /// reading its `release_stop` would read past the driver's own struct. Refused, loudly —
    /// which is §4's rule rather than a new one.
    #[test]
    fn a_v1_driver_is_refused_rather_than_read_past_its_end() {
        let mut driver = stub_vtable();
        driver.abi_version = 1;
        assert_eq!(
            accepts(&driver),
            Err(LoadError::AbiMismatch {
                expected: ABI_VERSION,
                found: 1,
            })
        );
    }

    #[test]
    fn a_driver_without_a_stop_path_is_refused() {
        let mut driver = stub_vtable();
        driver.emergency_stop = None;
        assert_eq!(
            accepts(&driver),
            Err(LoadError::MissingEntryPoint("emergency_stop"))
        );
    }

    #[test]
    fn optional_entry_points_stay_optional() {
        let mut driver = stub_vtable();
        driver.read_sensor = None;
        driver.extension = None;
        driver.release_stop = None;
        driver.close = None;
        assert_eq!(accepts(&driver), Ok(()));
    }

    #[test]
    fn unknown_modes_do_not_default() {
        assert_eq!(Mode::from_u8(2), Some(Mode::Torque));
        assert_eq!(Mode::from_u8(7), None);
    }

    /// The claim that makes the homing field not an ABI break: it uses bits nothing else does.
    ///
    /// Asserted rather than read off, because the failure mode is silent in the worst way — a
    /// homing field overlapping `SKR_FLAG_BRAKED` would make a braked robot report itself homed,
    /// and both halves would look correct in isolation.
    #[test]
    fn the_homing_field_occupies_bits_nothing_else_claims() {
        let existing = flags::ESTOP | flags::FAULT | flags::DEGRADED | flags::BRAKED;
        assert_eq!(existing & flags::HOMING_MASK, 0);
        assert_eq!(flags::HOMING_MASK, 0b0011_0000);
    }

    /// A driver that has never heard of homing reports *unreported*, not *pending*.
    ///
    /// This is the whole reason the field is two bits, and it is the case that keeps every v1
    /// driver loading unchanged: it sets flags it knows about and the field stays zero.
    #[test]
    fn a_v1_driver_reports_nothing_rather_than_a_state() {
        assert_eq!(Homing::from_flags(0), None);
        assert_eq!(Homing::from_flags(flags::ESTOP | flags::BRAKED), None);
    }

    #[test]
    fn every_homing_state_survives_a_round_trip_through_the_word() {
        for state in [Homing::NotRequired, Homing::Pending, Homing::Complete] {
            assert_eq!(Homing::from_flags(state.to_flags()), Some(state));
            // And beside the flags a real driver also sets, which is how it arrives.
            assert_eq!(
                Homing::from_flags(state.to_flags() | flags::DEGRADED | flags::ESTOP),
                Some(state)
            );
        }
    }

    /// Setting the field must not disturb the four flags below it, nor they it.
    #[test]
    fn the_two_halves_of_the_word_do_not_disturb_each_other() {
        let word = Homing::Complete.to_flags() | flags::BRAKED;
        assert_eq!(word & flags::BRAKED, flags::BRAKED);
        assert_eq!(word & flags::ESTOP, 0);
        assert_eq!(Homing::from_flags(word), Some(Homing::Complete));
    }
}

//! How much of the volume under a path is free (§33.6, §80).
//!
//! One question, one syscall, one crate. It exists because two named things in this repository were
//! blocked on the same missing measurement and neither could be finished without it:
//!
//! * §33.6's `storage_headroom`, reported **absent** at every check-in since check-in existed —
//!   `crate::checkin`'s own words were *"needs a `statvfs` nothing here calls"*;
//! * §80's retention of sealed windows, whose third condition is *"the disk is under the floor"*
//!   and which therefore never reclaims anything, because `Free::Unmeasured` is what it is handed.
//!
//! # Why this is a crate rather than three lines in the daemon
//!
//! `skynetd-io` carries the `forbid` attribute for unsafe code, and this workspace's precedent for a
//! necessary `unsafe` is a crate of its own with the argument in its header: `skynet-dl` is the
//! loader, `skynet-ring-shm` is §25's mapping, `skynet-claim` is §26.2's counter. This is the
//! **fifth**, and putting a hand-rolled syscall into the daemon instead would have spent that
//! property on a status field.
//!
//! Adding it also found a hole in the check that enforces the rule: the allow-list looked for the
//! string `forbid(unsafe_code)` anywhere in a crate's root file, and the sentence above — which is
//! *about another crate's* attribute — satisfied it. The check now matches the attribute anchored
//! to the start of a line, so a comment cannot stand in for one.
//!
//! # The layout hazard, and how this refuses to get it wrong quietly
//!
//! The obvious call is `statvfs`, and it was rejected: `struct statvfs` is a **libc** structure and
//! its layout is glibc's or musl's rather than the kernel's. `statfs` is the syscall's own, and on
//! **64-bit Linux** it is fifteen `__kernel_long_t` in a row — the same 120 bytes on x86-64 and on
//! aarch64, which take it from `asm-generic`. So there is no `struct` here at all: the buffer is
//! `[u64; 15]` and the fields are named indices into it. A layout that is a struct definition is a
//! layout that can be subtly wrong; a layout that is an index is one a reviewer can count.
//!
//! **And it is checked rather than trusted.** Every answer is validated before it is returned — the
//! block size is a power of two in a plausible range, the total is non-zero, and the available
//! count does not exceed the total. A wrong reading fails those and comes back as
//! [`SpaceError::Implausible`], which every caller turns into *unmeasured*. That matters more here
//! than it would elsewhere: this number authorises **deleting a sealed recording**, and the failure
//! to design against is not "no answer" but "a confident wrong one".
//!
//! On 32-bit Linux the kernel's `statfs` and `statfs64` differ and this returns
//! [`SpaceError::Unsupported`] rather than guessing; on any other platform, likewise. §28's tables
//! describe 64-bit machines and a robot on something else reports the field absent, which is the
//! state everything has been in since the daemon was written.
//!
//! # `f_bavail`, not `f_bfree`
//!
//! `f_bfree` counts every free block; `f_bavail` counts the ones an unprivileged process may
//! actually use, excluding the filesystem's reserve. This reports **available**, and the reason is
//! §28: the disk that matters is the one the black box can still write into. Blocks reserved for
//! root are not space this daemon has, and a robot that believed they were would discover it at the
//! moment the recorder failed — which is exactly when a recording is worth having.

#![warn(missing_docs)]

use std::path::Path;

/// Why a volume could not be measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpaceError {
    /// Not 64-bit Linux. The kernel structure this reads is not the one this platform has.
    Unsupported,
    /// The syscall failed, with its `errno`. A missing path is the ordinary case.
    Call(i32),
    /// The syscall succeeded and returned something that cannot describe a filesystem.
    ///
    /// Reported rather than returned, because the number is used to decide whether a sealed
    /// recording may be deleted.
    Implausible {
        /// The block size read.
        block_size: u64,
        /// The total block count read.
        blocks: u64,
        /// The available block count read.
        available: u64,
    },
}

impl SpaceError {
    /// A short code for a log or a status document.
    pub fn as_str(&self) -> &'static str {
        match self {
            SpaceError::Unsupported => "unsupported",
            SpaceError::Call(_) => "call",
            SpaceError::Implausible { .. } => "implausible",
        }
    }
}

impl core::fmt::Display for SpaceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SpaceError::Unsupported => {
                write!(
                    f,
                    "free space needs 64-bit Linux, where `statfs` has a layout this can read"
                )
            }
            SpaceError::Call(e) => write!(f, "statfs failed: errno {e}"),
            SpaceError::Implausible {
                block_size,
                blocks,
                available,
            } => write!(
                f,
                "statfs returned a filesystem that cannot exist — block size {block_size}, \
                 {blocks} blocks, {available} available — so nothing here is reported rather than \
                 a number being believed",
            ),
        }
    }
}

impl std::error::Error for SpaceError {}

/// What a volume holds and what is left of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    /// Bytes this process may still write.
    pub available_bytes: u64,
    /// Bytes the filesystem holds in total.
    pub total_bytes: u64,
}

impl Space {
    /// The fraction still free, 0–1 — §33.6's `storage_headroom`.
    ///
    /// A total of zero would be a division this cannot perform, and [`space_of`] refuses such a
    /// reading before one is ever constructed, so this is total and does not answer `None`.
    pub fn free_fraction(&self) -> f64 {
        self.available_bytes as f64 / self.total_bytes as f64
    }
}

// ── The syscall ──────────────────────────────────────────────────────────────
//
// Declared rather than depended on, for `skynet-ring-shm`'s reason: a bindings crate would be the
// first third-party dependency in a workspace whose absence of them a vendor can check with
// `cargo build` and no network.

#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
mod sys {
    use core::ffi::{c_char, c_int};

    /// The kernel's `struct statfs` on 64-bit: fifteen `__kernel_long_t`, 120 bytes.
    ///
    /// | Index | Field |
    /// |---|---|
    /// | 0 | `f_type` |
    /// | 1 | `f_bsize` |
    /// | 2 | `f_blocks` |
    /// | 3 | `f_bfree` |
    /// | 4 | `f_bavail` |
    /// | 5 | `f_files` |
    /// | 6 | `f_ffree` |
    /// | 7 | `f_fsid` (two 32-bit halves) |
    /// | 8 | `f_namelen` |
    /// | 9 | `f_frsize` |
    /// | 10 | `f_flags` |
    /// | 11–14 | `f_spare` |
    pub const WORDS: usize = 15;

    /// `f_bsize`.
    pub const BSIZE: usize = 1;
    /// `f_blocks`.
    pub const BLOCKS: usize = 2;
    /// `f_bavail`.
    pub const BAVAIL: usize = 4;

    extern "C" {
        pub fn statfs(path: *const c_char, buf: *mut u64) -> c_int;
        pub fn __errno_location() -> *mut c_int;
    }

    /// The last `errno`, for a call that failed.
    pub fn errno() -> i32 {
        // SAFETY: `__errno_location` returns a pointer to this thread's `errno`, valid for the
        // lifetime of the thread, and it is read immediately after the failed call.
        unsafe { *__errno_location() }
    }
}

/// The largest block size worth believing: a filesystem reporting more than this is one this is
/// reading wrong.
pub const MAX_BLOCK_SIZE: u64 = 1024 * 1024;
/// And the smallest.
pub const MIN_BLOCK_SIZE: u64 = 512;

/// Can these three numbers describe a filesystem?
///
/// The whole of what this crate checks, in one function so that the test exercises the rule rather
/// than a copy of it — a plausibility check tested against its own restatement is a check that
/// passes whatever either says.
///
/// Each clause catches a different way of reading the structure wrongly, and each is sufficient
/// alone: an offset one field out gives a block size that is a byte count or an inode count; one
/// several fields out gives an available count larger than the total.
///
/// Public because it is the crate's actual claim. `space_of` returns a number *or* nothing, and
/// this is the rule that decides which — a caller weighing whether to trust a reading from
/// somewhere else can ask the same question this asks.
pub fn plausible(block_size: u64, blocks: u64, available: u64) -> bool {
    (MIN_BLOCK_SIZE..=MAX_BLOCK_SIZE).contains(&block_size)
        && block_size.is_power_of_two()
        && blocks > 0
        && available <= blocks
}

/// Measure the volume the given path lives on.
///
/// The path must exist; a robot asks about its own black box directory, which §28 preallocates.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
pub fn space_of(path: &Path) -> Result<Space, SpaceError> {
    use std::os::unix::ffi::OsStrExt;

    let mut c_path = Vec::with_capacity(path.as_os_str().as_bytes().len() + 1);
    c_path.extend_from_slice(path.as_os_str().as_bytes());
    c_path.push(0);

    let mut buf = [0u64; sys::WORDS];
    // SAFETY: `c_path` is NUL-terminated and outlives the call; `buf` is 120 bytes, which is the
    // whole of the structure the kernel writes on this platform, and it is initialised.
    let rc = unsafe { sys::statfs(c_path.as_ptr().cast(), buf.as_mut_ptr()) };
    if rc != 0 {
        return Err(SpaceError::Call(sys::errno()));
    }

    let block_size = buf[sys::BSIZE];
    let blocks = buf[sys::BLOCKS];
    let available = buf[sys::BAVAIL];

    // Checked rather than trusted. Any of these failing means the bytes are not the structure this
    // believes they are, and the answer is silence rather than a number.
    if !plausible(block_size, blocks, available) {
        return Err(SpaceError::Implausible {
            block_size,
            blocks,
            available,
        });
    }

    // Saturating, because a filesystem large enough to overflow this multiplication is one whose
    // size is being read wrong — and the checks above have already established it is not.
    Ok(Space {
        available_bytes: available.saturating_mul(block_size),
        total_bytes: blocks.saturating_mul(block_size),
    })
}

/// Measure the volume the given path lives on.
///
/// Not this platform: see [`SpaceError::Unsupported`].
#[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
pub fn space_of(_path: &Path) -> Result<Space, SpaceError> {
    Err(SpaceError::Unsupported)
}

#[cfg(test)]
mod tests;

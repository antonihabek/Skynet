//! The shared sequence a black box ring's writers claim slots from (§26.2).
//!
//! §26.2 states the rule and the reason:
//!
//! > A `Ring` holds its next sequence number **in memory**. Two of them over one file hand out the
//! > same sequence twice, encode two records into one slot, and produce a recording that is intact
//! > by every check a reader applies and wrong in the interval that matters.
//!
//! There are two writers — `skynetd` records events and gaps, `skynet-kernel-host` records cycles —
//! and both belong in one recording. This crate is the counter they agree through: one `AtomicU64`
//! in a file both map `MAP_SHARED`, claimed by `fetch_add`.
//!
//! # It is a claim counter, not a recovery counter, and that distinction is the whole design
//!
//! `Ring::open` rejects a next-sequence field in the header, by name, and it is right to:
//!
//! > Recovery is a scan, and it is a scan on purpose. … The scan is `O(capacity)`, happens once at
//! > start, and is the only way to be right after a crash: the highest sequence number actually on
//! > disk is the truth, whatever any counter claims.
//!
//! Both arguments hold because they are about different counters. Recovery is about a **crash**,
//! where a counter is ahead of or behind the disk and there is no way to tell which. This is about
//! **concurrency**, where two live writers must agree who gets which slot.
//!
//! So this counter is **seeded from the scan** and never consulted after a restart. Three things
//! follow, and the third is why §26.2's fix turned out to be cheap:
//!
//! * It is not in the ring header — a field mutated on every append would invalidate the header's
//!   CRC, and would be the thing most likely to be torn by a power cut.
//! * It is not durable. [`Claim::attach`] seeds it every time the recording is opened, and a stale
//!   file left by a crash is overwritten rather than believed.
//! * **The on-disk ring format therefore does not change.** `FORMAT_VERSION` stays 2, the
//!   conformance corpus is untouched, and no language edition moves.
//!
//! # This is the fourth `unsafe` in the workspace
//!
//! `skynet-rhal` declares a C vtable, `skynet-cli` loads native code, `skynet-ring-shm` maps a
//! `memfd`. This is the fourth, and `skynet-statfs` — §33.6's free-disk read — is the fifth. It
//! takes the same treatment as all of them: as small as a crate can be — five `extern "C"`
//! declarations, one `mmap`, one `AtomicU64::from_ptr` — with everything a reviewer has to check
//! visible in one file.
//!
//! What it deliberately does **not** do is run a seqlock. `skynet-ring` needs one because a reader
//! may race a writer mid-slot; the black box has no concurrent reader, because sealing takes the
//! directory lock. A claimed slot is written by exactly one writer and read by nobody until that
//! writer has finished with it, so the only shared mutable state in this crate is the counter.
//!
//! # Linux, and what happens elsewhere
//!
//! `mmap` is POSIX and the `errno` accessor is not, so this follows `skynet-ring-shm`: on any
//! non-Linux target every function returns [`ClaimError::Unsupported`] rather than failing to
//! compile, and the workspace still builds and tests on a developer's machine.

#![forbid(missing_docs)]

use std::path::{Path, PathBuf};

/// Why a claim counter could not be created or mapped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimError {
    /// Not Linux. This mapping is not available here.
    Unsupported,
    /// The file could not be opened or created, with its `errno`.
    Open(i32),
    /// `ftruncate` failed, with its `errno`.
    Size(i32),
    /// `mmap` failed, with its `errno`.
    Map(i32),
}

impl ClaimError {
    /// A code for a log or a console.
    pub fn as_str(&self) -> &'static str {
        match self {
            ClaimError::Unsupported => "unsupported",
            ClaimError::Open(_) => "open",
            ClaimError::Size(_) => "size",
            ClaimError::Map(_) => "map",
        }
    }
}

impl core::fmt::Display for ClaimError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ClaimError::Unsupported => write!(f, "a shared claim counter needs Linux"),
            ClaimError::Open(e) => write!(f, "opening the claim file failed: errno {e}"),
            ClaimError::Size(e) => write!(f, "sizing the claim file failed: errno {e}"),
            ClaimError::Map(e) => write!(f, "mapping the claim file failed: errno {e}"),
        }
    }
}

impl std::error::Error for ClaimError {}

/// How many bytes the file holds: one `u64`.
pub const CLAIM_BYTES: usize = 8;

/// The file, beside the ring it is about.
pub fn path_in(directory: &Path) -> PathBuf {
    directory.join("claim")
}

// ── The syscalls ─────────────────────────────────────────────────────────────
//
// Declared rather than depended on, for `skynet-ring-shm`'s reason: a bindings crate would be the
// first non-vendored dependency in a workspace whose absence of them a vendor can check with
// `cargo build` and no network.

#[cfg(target_os = "linux")]
mod sys {
    use core::ffi::{c_char, c_int, c_void};

    /// `off_t`, which is **not** always 64 bits — see `skynet-ring-shm` for the 32-bit ARM case.
    #[cfg(target_pointer_width = "64")]
    pub type OffT = i64;
    #[cfg(not(target_pointer_width = "64"))]
    pub type OffT = i32;

    pub const PROT_READ: c_int = 1;
    pub const PROT_WRITE: c_int = 2;
    pub const MAP_SHARED: c_int = 1;

    pub const O_RDWR: c_int = 2;
    pub const O_CREAT: c_int = 64;
    pub const O_CLOEXEC: c_int = 0o2000000;

    extern "C" {
        pub fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        pub fn ftruncate(fd: c_int, length: OffT) -> c_int;
        pub fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            offset: OffT,
        ) -> *mut c_void;
        pub fn munmap(addr: *mut c_void, len: usize) -> c_int;
        pub fn close(fd: c_int) -> c_int;
        pub fn __errno_location() -> *mut c_int;
    }

    /// The last `errno`, for an error that carries one.
    pub fn errno() -> i32 {
        // SAFETY: `__errno_location` returns a pointer to this thread's `errno`, valid for the
        // lifetime of the thread. Reading it immediately after a failed call is the documented way
        // to obtain the error.
        unsafe { *__errno_location() }
    }
}

/// A mapped claim counter.
///
/// Dropping it unmaps and closes. Nothing else in this crate frees anything, so the entire lifetime
/// question is answered by this one type living exactly as long as its mapping.
#[derive(Debug)]
pub struct Claim {
    #[cfg(target_os = "linux")]
    fd: core::ffi::c_int,
    #[cfg(target_os = "linux")]
    addr: *mut core::ffi::c_void,
}

// The mapping is a shared region holding exactly one atomic, so handing one to another thread is
// exactly as safe as handing over the `&AtomicU64` it yields. The raw pointer is what makes the
// compiler ask, and it is only ever used to produce that reference and to unmap.
//
// `Sync` is implemented as well as `Send`, and unlike `skynet_ring_shm::Mapping` that is correct
// here: the entire contents is one atomic, and two threads sharing one is what the type is for.
unsafe impl Send for Claim {}
unsafe impl Sync for Claim {}

impl Claim {
    /// Open the counter beside a ring, seeding it when this caller is the one that creates it.
    ///
    /// `seed` is the sequence the next record should carry — `Ring::next_sequence()`, off the scan.
    ///
    /// # The seed is written unconditionally, and that is the point
    ///
    /// A file left by a previous run holds a sequence that was true for a ring which may since have
    /// been recreated at a different shape (`Recorder::open` does exactly that). Believing it would
    /// hand out sequences the scan disagrees with, which is the failure this counter exists to
    /// prevent, arriving from the other direction.
    ///
    /// **So the caller must serialise this.** §26.2 gives the directory lock its narrowed meaning
    /// for precisely this reason: whoever holds it seeds, and every writer thereafter attaches with
    /// [`Claim::attach_existing`]. A second seeding while a writer is claiming would reset the
    /// counter under it.
    pub fn seed(directory: &Path, seed: u64) -> Result<Claim, ClaimError> {
        let claim = Self::map(&path_in(directory))?;
        claim
            .counter()
            .store(seed, core::sync::atomic::Ordering::SeqCst);
        Ok(claim)
    }

    /// Attach to a counter somebody else seeded.
    ///
    /// Creates the file if it is absent, which is the ordinary race rather than an error: a writer
    /// that arrives between the creator's `open` and its `store` reads a zero, and a zero is not a
    /// sequence any ring hands out — [`Claim::next`] is `fetch_add` on a counter whose first valid
    /// value is 1. A caller that wants to be sure has the lock; see [`Claim::seed`].
    pub fn attach_existing(directory: &Path) -> Result<Claim, ClaimError> {
        Self::map(&path_in(directory))
    }

    /// The counter itself.
    pub fn counter(&self) -> &core::sync::atomic::AtomicU64 {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: `addr` is the result of a successful `mmap` of `CLAIM_BYTES` bytes, which is
            // page-aligned and therefore 8-aligned, and is unmapped only in `Drop`. The region is
            // `MAP_SHARED` and every access to it in this process and every other goes through this
            // one `AtomicU64`, so there is no non-atomic access to alias.
            unsafe { core::sync::atomic::AtomicU64::from_ptr(self.addr.cast()) }
        }
        #[cfg(not(target_os = "linux"))]
        {
            unreachable!("a Claim is never constructed off Linux")
        }
    }

    /// Claim the next sequence number.
    ///
    /// `fetch_add` returns the value **before** the addition, which is the one this writer owns —
    /// so two writers racing here receive two different numbers and neither has to know the other
    /// exists. That is the whole of §26.2.
    pub fn next(&self) -> u64 {
        self.counter()
            .fetch_add(1, core::sync::atomic::Ordering::SeqCst)
    }

    /// What the next claim will return, without claiming it.
    pub fn peek(&self) -> u64 {
        self.counter().load(core::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(target_os = "linux")]
    fn map(path: &Path) -> Result<Claim, ClaimError> {
        use std::os::unix::ffi::OsStrExt;

        let mut c_path = Vec::with_capacity(path.as_os_str().as_bytes().len() + 1);
        c_path.extend_from_slice(path.as_os_str().as_bytes());
        c_path.push(0);

        // SAFETY: `c_path` is NUL-terminated and outlives the call. The mode is only consulted when
        // `O_CREAT` creates the file.
        let fd = unsafe {
            sys::open(
                c_path.as_ptr().cast(),
                sys::O_RDWR | sys::O_CREAT | sys::O_CLOEXEC,
                0o644 as core::ffi::c_uint,
            )
        };
        if fd < 0 {
            return Err(ClaimError::Open(sys::errno()));
        }

        // Sized every time rather than only at creation: a file truncated to zero by anything at
        // all would make the mapping a `SIGBUS` on first touch, and `ftruncate` to the length it
        // already has is a no-op.
        // SAFETY: `fd` is a descriptor this function just opened successfully.
        if unsafe { sys::ftruncate(fd, CLAIM_BYTES as sys::OffT) } != 0 {
            let error = sys::errno();
            // SAFETY: `fd` is open and is not used again.
            unsafe { sys::close(fd) };
            return Err(ClaimError::Size(error));
        }

        // SAFETY: `fd` is open, sized to `CLAIM_BYTES`, and the length matches. A null `addr` lets
        // the kernel choose.
        let addr = unsafe {
            sys::mmap(
                core::ptr::null_mut(),
                CLAIM_BYTES,
                sys::PROT_READ | sys::PROT_WRITE,
                sys::MAP_SHARED,
                fd,
                0,
            )
        };
        if addr as isize == -1 {
            let error = sys::errno();
            // SAFETY: `fd` is open and is not used again.
            unsafe { sys::close(fd) };
            return Err(ClaimError::Map(error));
        }

        Ok(Claim { fd, addr })
    }

    #[cfg(not(target_os = "linux"))]
    fn map(_path: &Path) -> Result<Claim, ClaimError> {
        Err(ClaimError::Unsupported)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: `addr` and `fd` are the results of a successful `mmap` and `open`, neither has
            // been freed, and `self` is being destroyed so neither is used again.
            unsafe {
                sys::munmap(self.addr, CLAIM_BYTES);
                sys::close(self.fd);
            }
        }
    }
}

#[cfg(test)]
mod tests;

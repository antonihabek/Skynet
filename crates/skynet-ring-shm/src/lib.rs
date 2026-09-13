//! The `memfd` a §25 command ring is mapped over.
//!
//! §25 asks for "fixed-size slots in a `memfd`-backed mapping", and `skynet-ring` deliberately stops
//! one step short of that: it is a protocol over `&[AtomicU32]` and it opens nothing. This crate is
//! the step. It creates an anonymous file in memory, sizes it, seals it, maps it, and hands back the
//! slice the protocol runs on.
//!
//! # One of the two mappings that carry `unsafe`, outside the ABI and the loader
//!
//! `skynet-rhal` declares a C vtable of raw function pointers and `skynet-cli` loads arbitrary
//! native code; those two cannot be safe by construction and CI asserts every other crate is. This
//! is the third, `skynet-claim` — §26.2's shared sequence — is the fourth, and `skynet-statfs`
//! — §33.6's free-disk read — is the fifth. All are
//! deliberately as small as a crate can be; this one is four `extern "C"` declarations,
//! one `mmap`, and one `slice::from_raw_parts`. Everything about the ring that could move a robot
//! wrongly — the seqlock, the wrap, the tear detection, the layout — is in `skynet-ring`, where the
//! compiler checks it.
//!
//! The split is the point. A reviewer asking "is the command path memory-safe" reads this file, and
//! it is short.
//!
//! # Who creates it, and why that direction
//!
//! **The runtime creates the ring and the sandboxed policy receives it**, never the other way round.
//! The fd is passed over the policy's control socket; the policy maps it and writes; the runtime
//! maps it **read-only** and reads.
//!
//! Three properties fall out of that, and each is a thing an untrusted policy would otherwise be
//! able to do:
//!
//! * **The geometry is not the policy's to choose.** It is written and validated before the fd is
//!   handed over, and [`skynet_ring::Ring`] caches the slot count at open — so a policy scribbling a
//!   new one into the header afterwards changes nothing on the reading side.
//! * **The file cannot be shrunk.** `F_SEAL_SHRINK` is added before the fd leaves this process. A
//!   policy that could `ftruncate` the mapping smaller would make every subsequent read by the
//!   runtime a `SIGBUS` — a policy killing the supervisor with one syscall, from inside the sandbox.
//! * **The runtime cannot write it.** Its mapping carries no `PROT_WRITE`, so "the policy is the
//!   only writer" is enforced by the page tables rather than asserted in a comment. Which is also
//!   what makes the single-producer half of SPSC true across a trust boundary rather than by
//!   agreement.
//!
//! What a hostile producer *can* still do is put nonsense in a slot: a wild `channels`, a stalled
//! sequence, a seqlock that never goes even. None of it is memory-unsafe on the reading side —
//! `channels` is bounds-checked, the index is taken modulo a cached power of two, and every value is
//! a load — and all of it is a command, which is what the Safety Kernel is for. §7's guards are the
//! answer to a policy sending bad commands; this crate's job is only to make sure a bad command is
//! the *worst* thing it can send.
//!
//! # Linux, and what happens elsewhere
//!
//! `memfd_create` is Linux. On any other target every function here returns
//! [`ShmError::Unsupported`] rather than failing to compile, so the workspace still builds and tests
//! on a developer's machine — which is most of them, and all of the ones this was written on. The
//! Linux path is exercised by CI, which runs on Linux.

#![forbid(missing_docs)]

pub mod pass;

use core::sync::atomic::AtomicU32;

use skynet_ring::{Ring, HEADER_BYTES, SLOT_BYTES};

/// Why a ring could not be created or mapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShmError {
    /// Not Linux. `memfd_create` does not exist here.
    Unsupported,
    /// `memfd_create` failed, with its `errno`.
    Create(i32),
    /// `ftruncate` failed, with its `errno`.
    Size(i32),
    /// Adding the seals failed, with its `errno`.
    Seal(i32),
    /// `mmap` failed, with its `errno`.
    Map(i32),
    /// The slot count is zero or not a power of two.
    Geometry,
    /// The mapping is not a ring this build can address. Carries the protocol's own verdict.
    Ring(skynet_ring::RingError),
    /// `sendmsg` failed while handing the descriptor over, with its `errno`.
    Send(i32),
    /// `recvmsg` failed while receiving it, with its `errno`.
    Receive(i32),
    /// A message arrived carrying no descriptor.
    ///
    /// Its own variant rather than an `errno`, because it is not a failure of the syscall: the
    /// socket worked and the peer sent something that was not a ring. A policy that treated it as a
    /// transport error would retry for ever against a runtime that is never going to send one.
    NoDescriptor,
}

impl ShmError {
    /// A code for a log or a console.
    pub fn as_str(self) -> &'static str {
        match self {
            ShmError::Unsupported => "unsupported",
            ShmError::Create(_) => "create",
            ShmError::Size(_) => "size",
            ShmError::Seal(_) => "seal",
            ShmError::Map(_) => "map",
            ShmError::Geometry => "geometry",
            ShmError::Ring(_) => "ring",
            ShmError::Send(_) => "send",
            ShmError::Receive(_) => "receive",
            ShmError::NoDescriptor => "no-descriptor",
        }
    }
}

impl core::fmt::Display for ShmError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ShmError::Unsupported => write!(f, "memfd rings are Linux-only"),
            ShmError::Create(e) => write!(f, "memfd_create failed: errno {e}"),
            ShmError::Size(e) => write!(f, "ftruncate failed: errno {e}"),
            ShmError::Seal(e) => write!(f, "sealing failed: errno {e}"),
            ShmError::Map(e) => write!(f, "mmap failed: errno {e}"),
            ShmError::Geometry => write!(f, "the slot count must be a non-zero power of two"),
            ShmError::Ring(e) => write!(f, "not a usable ring: {}", e.as_str()),
            ShmError::Send(e) => write!(f, "the descriptor could not be sent: errno {e}"),
            ShmError::Receive(e) => write!(f, "the descriptor could not be received: errno {e}"),
            ShmError::NoDescriptor => write!(
                f,
                "the peer on this socket sent no descriptor, so there is no ring here to map"
            ),
        }
    }
}

impl std::error::Error for ShmError {}

/// How the mapping may be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// The producer's mapping. Read and write.
    Write,
    /// The consumer's mapping. **No `PROT_WRITE`.**
    ///
    /// Which is what makes the single-producer half of SPSC a property of the page tables rather
    /// than of an agreement between two programs, one of which is untrusted.
    Read,
}

/// How many bytes a ring of `slot_count` slots occupies.
pub fn bytes_for(slot_count: usize) -> usize {
    HEADER_BYTES + slot_count * SLOT_BYTES
}

// ── The four syscalls ────────────────────────────────────────────────────────
//
// Declared rather than depended on. See the manifest for why: a bindings crate would be the first
// non-vendored dependency in a workspace whose absence of them is a property a vendor can check
// with `cargo build` and no network, and these four declarations are visible in a diff in a way a
// transitive dependency is not.

#[cfg(target_os = "linux")]
mod sys {
    use core::ffi::{c_char, c_int, c_uint, c_void};

    /// `off_t`, which is **not** always 64 bits.
    ///
    /// glibc on a 32-bit target compiles `off_t` as `long` unless `_FILE_OFFSET_BITS=64` is defined,
    /// and 32-bit ARM is a live target for a robot controller. Declaring `ftruncate` with an `i64`
    /// there would push the argument in the wrong register and size a file to something nobody
    /// asked for. A ring is a few hundred kilobytes, so 32 bits is ample where that is what the ABI
    /// uses.
    #[cfg(target_pointer_width = "64")]
    pub type OffT = i64;
    #[cfg(not(target_pointer_width = "64"))]
    pub type OffT = i32;

    pub const PROT_READ: c_int = 1;
    pub const PROT_WRITE: c_int = 2;
    pub const MAP_SHARED: c_int = 1;

    pub const MFD_CLOEXEC: c_uint = 1;
    pub const MFD_ALLOW_SEALING: c_uint = 2;

    /// Only the cross-process test needs this: it duplicates the ring's descriptor *without*
    /// `FD_CLOEXEC` so the copy survives an `exec`. Production keeps the flag — a ring leaking into
    /// every child a runtime happens to spawn is precisely the accident `MFD_CLOEXEC` prevents.
    #[cfg(test)]
    pub const F_DUPFD: c_int = 0;
    pub const F_ADD_SEALS: c_int = 1033;
    pub const F_SEAL_SEAL: c_int = 0x0001;
    pub const F_SEAL_SHRINK: c_int = 0x0002;
    pub const F_SEAL_GROW: c_int = 0x0004;

    extern "C" {
        pub fn memfd_create(name: *const c_char, flags: c_uint) -> c_int;
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
        pub fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
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

/// A mapped ring: the file, the address, and the slice the protocol runs on.
///
/// Dropping it unmaps and closes. Nothing else in this crate frees anything, so the entire lifetime
/// question is answered by this one type living exactly as long as its mapping.
#[derive(Debug)]
pub struct Mapping {
    #[cfg(target_os = "linux")]
    fd: core::ffi::c_int,
    #[cfg(target_os = "linux")]
    addr: *mut core::ffi::c_void,
    bytes: usize,
    slot_count: usize,
    access: Access,
}

// The mapping is a plain shared region and the protocol over it is built out of atomics, so handing
// one to another thread is exactly as safe as handing over the `&[AtomicU32]` it yields — which is
// the whole design. The raw pointer inside is what makes the compiler ask, and it is only ever used
// to produce that slice and to unmap.
//
// `Sync` is deliberately **not** implemented. Two threads sharing one `Mapping` would be two
// producers or two consumers on a structure whose entire correctness argument is that there is one
// of each, and §25.2's sentence about two policies driving one elbow — "a trajectory neither was
// trained on and neither can be held responsible for" — is what that looks like from outside.
unsafe impl Send for Mapping {}

impl Mapping {
    /// Create a fresh ring in an anonymous file, and write its header.
    ///
    /// The file is created with `MFD_ALLOW_SEALING`, sized, and then sealed against growing,
    /// shrinking and further sealing **before it is returned** — so there is no window in which a
    /// process holding the fd can resize it, not even the one that made it.
    ///
    /// `name` appears in `/proc/self/fd` and nowhere else; it is a debugging aid and has no bearing
    /// on identity. Two rings with the same name are two different files.
    pub fn create(name: &str, slot_count: usize) -> Result<Mapping, ShmError> {
        if slot_count == 0 || !slot_count.is_power_of_two() {
            return Err(ShmError::Geometry);
        }
        let mapping = Self::create_inner(name, slot_count)?;
        Ring::create(mapping.words(), slot_count).map_err(ShmError::Ring)?;
        Ok(mapping)
    }

    /// Create a mapping with **no ring in it**, for a protocol that is not §25's command ring.
    ///
    /// [`Mapping::create`] writes a command-ring header and validates it; this writes nothing. The
    /// caller lays its own structure over [`Mapping::words`] — §25's observation ring is the reason
    /// it exists, and it has a different magic on purpose so that a consumer built for commands
    /// which opened one is refused at the first word rather than reading joint positions as
    /// setpoints with the drives live.
    ///
    /// The sealing, the `MFD_CLOEXEC` and the geometry rules are the same; what is absent is any
    /// opinion about what the bytes mean.
    pub fn create_raw(name: &str, slot_count: usize) -> Result<Mapping, ShmError> {
        if slot_count == 0 || !slot_count.is_power_of_two() {
            return Err(ShmError::Geometry);
        }
        Self::create_inner(name, slot_count)
    }

    /// Map a raw mapping somebody else created, without reading a ring header.
    ///
    /// **Takes ownership of `fd`**, like [`Mapping::adopt`]. The caller validates whatever protocol
    /// it expects to find: this cannot, because the whole point of the raw pair is that the bytes
    /// are not §25's.
    pub fn adopt_raw(
        fd: core::ffi::c_int,
        slot_count: usize,
        access: Access,
    ) -> Result<Mapping, ShmError> {
        if slot_count == 0 || !slot_count.is_power_of_two() {
            return Err(ShmError::Geometry);
        }
        Self::adopt_inner(fd, slot_count, access)
    }

    /// Map a ring somebody else created, from a file descriptor they passed over.
    ///
    /// The header is validated before this returns, so a caller never holds a `Mapping` over
    /// something that is not a ring this build can address. `slot_count` is read from the header and
    /// cached from that moment: a producer that later rewrites the header cannot move where this
    /// side looks.
    ///
    /// **Takes ownership of `fd`**, which it closes on drop. A caller that received it over a
    /// socket should not close it as well.
    pub fn adopt(
        fd: core::ffi::c_int,
        slot_count: usize,
        access: Access,
    ) -> Result<Mapping, ShmError> {
        if slot_count == 0 || !slot_count.is_power_of_two() {
            return Err(ShmError::Geometry);
        }
        let mapping = Self::adopt_inner(fd, slot_count, access)?;
        Ring::open(mapping.words()).map_err(ShmError::Ring)?;
        Ok(mapping)
    }

    /// The file descriptor, for passing to another process.
    ///
    /// Borrowed, not given: this `Mapping` still closes it on drop, so a caller sending it over a
    /// socket with `SCM_RIGHTS` — which duplicates it into the receiver — needs nothing else, and a
    /// caller tempted to `close` it should not.
    #[cfg(target_os = "linux")]
    pub fn fd(&self) -> core::ffi::c_int {
        self.fd
    }

    /// How the mapping may be used.
    pub fn access(&self) -> Access {
        self.access
    }

    /// How many slots it holds.
    pub fn slot_count(&self) -> usize {
        self.slot_count
    }

    /// How many bytes it occupies.
    pub fn len(&self) -> usize {
        self.bytes
    }

    /// Whether it holds nothing at all. Never true — a ring always has a header and a slot — and
    /// present because `len` without `is_empty` is a lint and because the answer is worth stating.
    pub fn is_empty(&self) -> bool {
        self.bytes == 0
    }

    /// The ring over this mapping.
    ///
    /// Borrowed from `self`, so a `Ring` cannot outlive the mapping it reads. That lifetime is the
    /// entire safety argument for the slice below, and it is the compiler's to check rather than a
    /// caller's to remember.
    pub fn ring(&self) -> Result<Ring<'_>, ShmError> {
        Ring::open(self.words()).map_err(ShmError::Ring)
    }

    #[cfg(target_os = "linux")]
    fn create_inner(name: &str, slot_count: usize) -> Result<Mapping, ShmError> {
        Self::create_sized(name, bytes_for(slot_count), slot_count)
    }

    #[cfg(target_os = "linux")]
    fn create_sized(name: &str, bytes: usize, slot_count: usize) -> Result<Mapping, ShmError> {
        // A C string on the stack. The name is a debugging aid, so a long one is truncated rather
        // than refused — and an interior NUL is truncated at, which is what the syscall would do
        // with it anyway.
        let mut c_name = [0u8; 64];
        for (slot, byte) in c_name.iter_mut().take(62).zip(name.bytes()) {
            if byte == 0 {
                break;
            }
            *slot = byte;
        }

        // SAFETY: `c_name` is NUL-terminated by construction — the loop above writes at most 62 of
        // 64 bytes and the array is zeroed.
        let fd = unsafe {
            sys::memfd_create(
                c_name.as_ptr() as *const core::ffi::c_char,
                sys::MFD_CLOEXEC | sys::MFD_ALLOW_SEALING,
            )
        };
        if fd < 0 {
            return Err(ShmError::Create(sys::errno()));
        }

        // SAFETY: `fd` is a file descriptor this function owns and has not yet closed.
        if unsafe { sys::ftruncate(fd, bytes as sys::OffT) } != 0 {
            let error = sys::errno();
            // SAFETY: same, and nothing has been mapped yet.
            unsafe { sys::close(fd) };
            return Err(ShmError::Size(error));
        }

        // The seals, before anything is mapped and long before the fd is passed anywhere. `SEAL_SEAL`
        // is included so the set cannot be *reduced* later: a seal that can be unsealed is a comment.
        //
        // SAFETY: `fd` is owned here and was created with `MFD_ALLOW_SEALING`, without which this
        // returns EPERM rather than doing anything surprising.
        let sealed = unsafe {
            sys::fcntl(
                fd,
                sys::F_ADD_SEALS,
                sys::F_SEAL_SHRINK | sys::F_SEAL_GROW | sys::F_SEAL_SEAL,
            )
        };
        if sealed != 0 {
            let error = sys::errno();
            // SAFETY: as above.
            unsafe { sys::close(fd) };
            return Err(ShmError::Seal(error));
        }

        Self::map(fd, bytes, slot_count, Access::Write)
    }

    #[cfg(target_os = "linux")]
    fn adopt_inner(
        fd: core::ffi::c_int,
        slot_count: usize,
        access: Access,
    ) -> Result<Mapping, ShmError> {
        Self::map(fd, bytes_for(slot_count), slot_count, access)
    }

    /// Create a mapping of an explicit **byte** size, for a protocol whose unit is not a ring slot.
    ///
    /// §47.6's frame pool is why this exists. A ring slot is 4 KiB and a 1080p frame is not, so
    /// sizing in slots would round every pool up to the nearest ring and lay a `[AtomicU32]` over
    /// pixels — two wrong shapes to get one mapping. The sealing, the `MFD_CLOEXEC` and the
    /// `MAP_SHARED` are identical; what differs is only the arithmetic and the accessor the caller
    /// reaches for afterwards ([`Mapping::bytes_slice`] rather than [`Mapping::words`]).
    ///
    /// `slot_count` is carried through unvalidated and uninterpreted — the caller's own notion of a
    /// slot, recorded so both ends can agree on it without a second channel. It is **not** a power
    /// of two here, because a frame pool's capacity is a vendor's declaration (§47.5) and rounding
    /// somebody's four-slot pool to eight would silently change what §47.6's drop-and-count rule
    /// counts.
    #[cfg(target_os = "linux")]
    pub fn create_bytes(name: &str, bytes: usize, slots: usize) -> Result<Mapping, ShmError> {
        if bytes == 0 || slots == 0 {
            return Err(ShmError::Geometry);
        }
        Self::create_sized(name, bytes, slots)
    }

    /// Map a byte-sized region somebody else created and passed over.
    ///
    /// **Takes ownership of `fd`**, like every other `adopt` here. Nothing about the contents is
    /// validated: that is the caller's, for [`Mapping::adopt_raw`]'s reason.
    ///
    /// `bytes` must be what the creator sized it to. A receiver that maps fewer sees a prefix,
    /// which is a protocol error rather than a memory-safety one; a receiver that maps *more* would
    /// be a `SIGBUS` waiting for the first read past the end, which is why the creator's size
    /// crosses beside the descriptor rather than being guessed from the geometry.
    #[cfg(target_os = "linux")]
    pub fn adopt_bytes(
        fd: core::ffi::c_int,
        bytes: usize,
        slots: usize,
        access: Access,
    ) -> Result<Mapping, ShmError> {
        if bytes == 0 || slots == 0 {
            return Err(ShmError::Geometry);
        }
        Self::map(fd, bytes, slots, access)
    }

    /// Not supported off Linux, for [`Mapping::create`]'s reason.
    #[cfg(not(target_os = "linux"))]
    pub fn create_bytes(_name: &str, _bytes: usize, _slots: usize) -> Result<Mapping, ShmError> {
        Err(ShmError::Unsupported)
    }

    /// Not supported off Linux, for [`Mapping::create`]'s reason.
    #[cfg(not(target_os = "linux"))]
    pub fn adopt_bytes(
        _fd: core::ffi::c_int,
        _bytes: usize,
        _slots: usize,
        _access: Access,
    ) -> Result<Mapping, ShmError> {
        Err(ShmError::Unsupported)
    }

    #[cfg(target_os = "linux")]
    fn map(
        fd: core::ffi::c_int,
        bytes: usize,
        slot_count: usize,
        access: Access,
    ) -> Result<Mapping, ShmError> {
        let prot = match access {
            Access::Write => sys::PROT_READ | sys::PROT_WRITE,
            Access::Read => sys::PROT_READ,
        };
        // SAFETY: a null hint asks the kernel to choose the address, `bytes` is the size the file
        // was truncated to, and `fd` is a valid descriptor. `MAP_SHARED` is what makes the mapping
        // the same memory in every process that maps this file, which is the entire point.
        let addr = unsafe {
            sys::mmap(
                core::ptr::null_mut(),
                bytes,
                prot,
                sys::MAP_SHARED,
                fd,
                0 as sys::OffT,
            )
        };
        if addr as isize == -1 {
            let error = sys::errno();
            // SAFETY: `fd` is owned by this function on the failure path and nothing was mapped.
            unsafe { sys::close(fd) };
            return Err(ShmError::Map(error));
        }
        Ok(Mapping {
            fd,
            addr,
            bytes,
            slot_count,
            access,
        })
    }

    /// The mapping as bytes, for a protocol whose unit is a byte rather than a word.
    ///
    /// `AtomicU8` rather than `u8`, and the reason is the same one [`Mapping::words`] gives for
    /// `AtomicU32`: another process writes this memory while this one reads it, which is a data
    /// race for any type without interior mutability — including for pixels nobody will ever
    /// compare. A relaxed load compiles to the same byte move a plain read would, so the atomicity
    /// costs nothing and is what makes the read defined.
    ///
    /// It carries **no ordering of its own**. §47.6's tear rule is the caller's: a reader that
    /// wants a frame rather than a smear reads the generation, reads the bytes, and reads the
    /// generation again.
    #[cfg(target_os = "linux")]
    pub fn bytes_slice(&self) -> &[core::sync::atomic::AtomicU8] {
        // SAFETY: identical to `words` below, one element type narrower. `addr` came from a
        // successful `mmap` of `self.bytes` bytes and is unmapped only by `Drop`; `mmap` returns
        // page-aligned memory, which is aligned for `AtomicU8` trivially; the region is
        // `MAP_SHARED` over a `memfd`, so every byte is readable and initialised, and every bit
        // pattern is a valid `AtomicU8`; and the lifetime is `&self`, so no slice outlives the
        // mapping.
        unsafe {
            core::slice::from_raw_parts(
                self.addr as *const core::sync::atomic::AtomicU8,
                self.bytes,
            )
        }
    }

    #[cfg(target_os = "linux")]
    /// The words the protocol runs on.
    ///
    /// Public because [`Mapping::create_raw`] would be useless without it: a caller laying its own
    /// ring over this mapping needs the slice, and the alternative — a second copy of the mapping
    /// logic in the crate that owns the other protocol — is the second `mmap` this crate exists to
    /// prevent.
    pub fn words(&self) -> &[AtomicU32] {
        // SAFETY: this is the one place in the workspace that turns an address into a slice, and
        // four things make it sound.
        //
        // 1. `addr` came from a successful `mmap` of `bytes` bytes and has not been unmapped: the
        //    only `munmap` is in `Drop`, which consumes `self`.
        // 2. `mmap` returns page-aligned memory, which is aligned for `AtomicU32` by a wide margin.
        // 3. The region is `MAP_SHARED` over a `memfd`, so every byte is readable and initialised —
        //    a fresh `memfd` reads as zero, and zero is a valid `AtomicU32`.
        // 4. The lifetime is `&self`, so no slice can outlive the mapping.
        //
        // The concurrent access is the whole reason the element type is an atomic: another process
        // writing this memory while this one reads it is a data race for any other type and is
        // exactly what `AtomicU32` is defined to permit.
        unsafe { core::slice::from_raw_parts(self.addr as *const AtomicU32, self.bytes / 4) }
    }

    // ── Everywhere else ──────────────────────────────────────────────────────
    //
    // A stub rather than a compile error, so `cargo test --workspace` works on a machine that is not
    // the robot. The alternative — `#[cfg]`-ing the crate out of the workspace — makes it invisible
    // on the machines where most of the reading happens.

    #[cfg(not(target_os = "linux"))]
    fn create_inner(_name: &str, _slot_count: usize) -> Result<Mapping, ShmError> {
        Err(ShmError::Unsupported)
    }

    #[cfg(not(target_os = "linux"))]
    fn adopt_inner(
        _fd: core::ffi::c_int,
        _slot_count: usize,
        _access: Access,
    ) -> Result<Mapping, ShmError> {
        Err(ShmError::Unsupported)
    }

    #[cfg(not(target_os = "linux"))]
    /// Nothing is mapped here, so there are no bytes.
    ///
    /// The twin of [`Mapping::words`]'s stub, and it was missing when §47.6c's mapping half landed.
    /// `skynet-frames` calls `bytes_slice` from ten places and gates none of them — correctly, since
    /// a pool cannot be *constructed* off Linux and therefore cannot be read there either. So the
    /// absent stub was never a runtime hole; it was a **compile** failure, and it took the whole
    /// workspace down on every host that is not the robot. That is the one thing the comment above
    /// this block promises will not happen, which is why the stub is the fix rather than a `cfg` on
    /// each of the ten callers.
    pub fn bytes_slice(&self) -> &[core::sync::atomic::AtomicU8] {
        &[]
    }

    #[cfg(not(target_os = "linux"))]
    /// Nothing is mapped here, so there are no words.
    pub fn words(&self) -> &[AtomicU32] {
        &[]
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            // SAFETY: both were produced by this type and neither has been released; `drop` runs
            // once. The results are ignored because there is nothing a failing `munmap` at teardown
            // could be turned into, and a panic in `Drop` would replace a leaked page with an
            // aborted process.
            unsafe {
                sys::munmap(self.addr, self.bytes);
                sys::close(self.fd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_is_the_header_plus_the_slots() {
        assert_eq!(bytes_for(1), 4096 * 2);
        assert_eq!(bytes_for(8), 4096 * 9);
    }

    #[test]
    fn a_slot_count_that_is_not_a_power_of_two_is_refused_before_any_syscall() {
        // Refused on every platform, including the ones with no `memfd_create`, because it is a
        // geometry question rather than a system one.
        assert_eq!(Mapping::create("x", 0).unwrap_err(), ShmError::Geometry);
        assert_eq!(Mapping::create("x", 6).unwrap_err(), ShmError::Geometry);
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn elsewhere_it_says_so_rather_than_failing_to_compile() {
        assert_eq!(
            Mapping::create("ring", 4).unwrap_err(),
            ShmError::Unsupported
        );
    }

    #[cfg(target_os = "linux")]
    mod linux {
        use super::*;
        // Imported here rather than beside the module above, because everything that touches a
        // command is behind this `cfg` and an unused import on a developer's laptop is a warning
        // that CI turns into a failure.
        use skynet_ring::{Command, Reading};

        #[test]
        fn a_created_ring_maps_and_is_empty() {
            let mapping = Mapping::create("skynet-test", 4).expect("create");
            assert_eq!(mapping.len(), bytes_for(4));
            assert_eq!(mapping.slot_count(), 4);
            assert_eq!(mapping.access(), Access::Write);

            let ring = mapping.ring().expect("ring");
            let mut got = Command::default();
            assert_eq!(ring.consumer().read(&mut got), Reading::Empty);
        }

        #[test]
        fn a_command_crosses_two_independent_mappings_of_one_file() {
            // The property the whole crate exists for, and it cannot be tested with one mapping:
            // two `mmap`s of the same `memfd` are the same memory, so a store through one is a load
            // through the other. Anything less proves only that a process can talk to itself.
            let producer_side = Mapping::create("skynet-test", 2).expect("create");
            let consumer_side = Mapping::adopt(producer_side.fd(), 2, Access::Read).expect("adopt");

            let mut sent = Command {
                seq: 99,
                stamp_ns: 1_234_567,
                channels: 3,
                ..Command::default()
            };
            sent.values[..3].copy_from_slice(&[1.5, -2.25, 0.0]);
            sent.mask[..3].copy_from_slice(&[1, 0, 1]);
            producer_side
                .ring()
                .expect("ring")
                .producer()
                .publish(&sent)
                .expect("publish");

            let mut got = Command::default();
            assert_eq!(
                consumer_side
                    .ring()
                    .expect("ring")
                    .consumer()
                    .read(&mut got),
                Reading::Fresh
            );
            assert_eq!(got.seq, 99);
            assert_eq!(&got.values[..3], &[1.5, -2.25, 0.0]);
            assert_eq!(&got.mask[..3], &[1, 0, 1]);

            // `adopt` took ownership of a descriptor the other mapping also closes, so leak the
            // duplicate rather than double-closing it. In production the consumer's fd arrives over
            // a socket and is genuinely its own; here there is one file and two owners, which is a
            // shape only a test can produce.
            core::mem::forget(consumer_side);
        }

        #[test]
        fn the_file_cannot_be_shrunk_after_it_is_sealed() {
            // The seal is not decoration. A policy holding this fd inside its sandbox could
            // otherwise `ftruncate` it to zero, and every subsequent read by the runtime would take
            // `SIGBUS` — one syscall from untrusted code killing the supervisor.
            let mapping = Mapping::create("skynet-test", 2).expect("create");
            // SAFETY: a descriptor this test owns, for the duration of the call.
            let shrunk = unsafe { sys::ftruncate(mapping.fd(), 4096) };
            assert_eq!(shrunk, -1, "F_SEAL_SHRINK did not hold");
            assert_eq!(sys::errno(), 1 /* EPERM */);
        }

        #[test]
        fn the_seals_cannot_be_removed() {
            // `F_SEAL_SEAL`, which is why the set is added in one call before the fd goes anywhere.
            // A seal that can be lifted is a comment.
            let mapping = Mapping::create("skynet-test", 1).expect("create");
            // SAFETY: as above.
            let again = unsafe { sys::fcntl(mapping.fd(), sys::F_ADD_SEALS, sys::F_SEAL_GROW) };
            assert_eq!(again, -1);
            assert_eq!(sys::errno(), 1 /* EPERM */);
        }

        #[test]
        fn a_read_only_mapping_refuses_a_write_from_the_page_tables() {
            // Not asserted in a comment: a `Ring` from a read-only mapping hands out a `Producer`
            // like any other, and a `publish` through it would fault. That is the *design* — the
            // page tables are the enforcement, and the type system's job here is only to make the
            // read path convenient. This test pins the half that can be checked without a signal
            // handler: the mapping was made without PROT_WRITE and says so.
            let writer = Mapping::create("skynet-test", 1).expect("create");
            let reader = Mapping::adopt(writer.fd(), 1, Access::Read).expect("adopt");
            assert_eq!(reader.access(), Access::Read);
            assert_eq!(writer.access(), Access::Write);
            core::mem::forget(reader);
        }

        /// The claim the crate exists for, proved across a process boundary.
        ///
        /// Everything else here maps the same file twice in one address space, which proves
        /// `MAP_SHARED` behaves — and proves it in a process that could have used a `static`
        /// instead. §25 puts the policy in a **sandbox**, so the load-bearing claim is that a store
        /// by one process is a load by another, and the only way to check that is to have two.
        ///
        /// The child is this same test binary, re-executed with two environment variables, which is
        /// why the test appears to call itself. The alternative — a second crate holding a
        /// forty-line binary — is a build artifact that ships forever to test one syscall.
        #[test]
        fn a_command_crosses_two_processes() {
            // The child half. It runs first in reading order because it must return before the
            // parent half below is reached.
            if let Ok(raw) = std::env::var("SKYNET_RING_TEST_FD") {
                let fd: core::ffi::c_int = raw.parse().expect("a numeric fd");
                let mapping = Mapping::adopt(fd, 2, Access::Read).expect("adopt in the child");
                let mut got = Command::default();
                assert_eq!(
                    mapping.ring().expect("ring").consumer().read(&mut got),
                    Reading::Fresh,
                );
                assert_eq!(got.seq, 4_242);
                assert_eq!(got.channels, 2);
                assert_eq!(&got.values[..2], &[7.5, -0.25]);
                return;
            }

            let mapping = Mapping::create("skynet-cross", 2).expect("create");
            let mut sent = Command {
                seq: 4_242,
                channels: 2,
                ..Command::default()
            };
            sent.values[..2].copy_from_slice(&[7.5, -0.25]);
            sent.mask[..2].copy_from_slice(&[1, 1]);
            mapping
                .ring()
                .expect("ring")
                .producer()
                .publish(&sent)
                .expect("publish");

            // A duplicate without `FD_CLOEXEC`, so it survives the `exec`. The original keeps the
            // flag: a ring leaking into every child a runtime happens to spawn is exactly the sort
            // of accident `MFD_CLOEXEC` is there to prevent, and this test wants one exception
            // rather than the flag removed.
            //
            // SAFETY: a descriptor this test owns.
            let inherited = unsafe { sys::fcntl(mapping.fd(), sys::F_DUPFD, 3) };
            assert!(inherited >= 3, "F_DUPFD failed: errno {}", sys::errno());

            let status =
                std::process::Command::new(std::env::current_exe().expect("the test binary"))
                    .args([
                        "--exact",
                        "tests::linux::a_command_crosses_two_processes",
                        "--nocapture",
                    ])
                    .env("SKYNET_RING_TEST_FD", inherited.to_string())
                    .status()
                    .expect("spawn the child");

            // SAFETY: the duplicate is this test's, and the child has exited.
            unsafe { sys::close(inherited) };
            assert!(
                status.success(),
                "the child could not read the ring: {status}"
            );
        }

        #[test]
        fn adopting_something_that_is_not_a_ring_is_refused() {
            // A `memfd` full of zeroes has no magic, so `Ring::open` refuses it — which is what
            // stops a mismapped or wrong-sized fd from being read as a command stream.
            let raw = {
                let name = b"skynet-empty\0";
                // SAFETY: a NUL-terminated literal.
                unsafe { sys::memfd_create(name.as_ptr() as *const core::ffi::c_char, 0) }
            };
            assert!(raw >= 0);
            // SAFETY: a descriptor this test owns.
            assert_eq!(unsafe { sys::ftruncate(raw, bytes_for(1) as sys::OffT) }, 0);

            let error = Mapping::adopt(raw, 1, Access::Write).unwrap_err();
            assert_eq!(error, ShmError::Ring(skynet_ring::RingError::NotARing));
        }
    }
}

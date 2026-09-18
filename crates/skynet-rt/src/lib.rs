//! §23.5's machine preconditions, read by the process itself.
//!
//! §23.5's list is *"RT core, `SCHED_FIFO` for the kernel thread,
//! `mlockall(MCL_CURRENT|MCL_FUTURE)`, no swap"*. `SCHED_FIFO` belongs to the unit file; the
//! three the running process can read live here:
//!
//! * [`lock_if_unlimited`] — lock this process's address space, current pages and future ones, so
//!   the loop is never swapped out mid-cycle and no page fault lands on the command path;
//! * [`preempt_rt`] — read whether the running kernel is PREEMPT_RT, from `/sys/kernel/realtime`;
//! * [`swap_kb`] — read how much swap the machine has active, from `/proc/meminfo`.
//!
//! Plus [`locked_kb`], which reads `VmLck` back out of `/proc/self/status` — the lock's own
//! positive control, in the reading rather than in a test. A lock that succeeded and a `VmLck`
//! of zero disagree, and the caller prints both rather than trusting either.
//!
//! And [`refuse_host`], which turns the three readings into the refusal `--tier host` carries:
//! empty means the machine backs the claim, and every entry names what was read. Unknown
//! readings refuse — an unreadable probe is not a zero (F22), and a claim the machine cannot
//! back is what a refusal is for.
//!
//! # Why this is a crate rather than three lines in `skynetk`
//!
//! `skynet-kernel-host` carries the `forbid` attribute for unsafe code, and this workspace's
//! precedent for a necessary `unsafe` is a crate of its own with the argument in its header:
//! `skynet-statfs` is §33.6's free-disk measurement the same way. This is the eighth member of
//! the allow-list `ci.yml` holds, for one `mlockall` declared rather than depended on — a
//! bindings crate would be the first third-party dependency in a workspace whose absence of them
//! a vendor checks with `cargo build` and no network.
//!
//! # Why the lock is conditional, and why success is the dangerous branch
//!
//! The first version of this crate called `mlockall` unconditionally and reported the result.
//! Against a developer's shell limit it succeeded — and the loop then died mapping its 67 MB
//! black box ring, on a machine with fifteen gigabytes free. `MCL_FUTURE` under a finite
//! `RLIMIT_MEMLOCK` makes every later mapping that would exceed the limit fail, so a lock that
//! *succeeds* under a small limit sabotages the process and a lock that *fails* leaves it
//! running. `DRY-COVERAGE-PLAN.md` F61 is the full account, with the control.
//!
//! So the crate exposes no unconditional call. [`lock_if_unlimited`] reads the limit first and
//! locks only when it is unlimited — which is exactly `skynetk.service`'s `LimitMEMLOCK=infinity`
//! — and every other case is a named outcome the caller reports. Unlimited is the only safe
//! predicate: any finite threshold is a guess about a footprint that grows after the call.
//!
//! # What this crate does not do
//!
//! It reads and it derives; the exit code belongs to the caller. [`refuse_host`] names every
//! reason `--tier host` may not start on this machine, and `skynetk` refuses on a non-empty
//! list before the driver opens. `SCHED_FIFO` is the one precondition nobody here reads — the
//! amendment's refusal names the three a process can establish about its machine, and the
//! scheduler stays the unit file's to set, the loop's overruns to report, and the bench's to
//! measure.

#![warn(missing_docs)]

/// Lock this process's memory, but only when the limit allows all of it.
///
/// Reads [`memlock_limit`] first. `Unlimited` is the only case that calls `mlockall`
/// (`MCL_CURRENT | MCL_FUTURE` — the flags' values are 1 and 2 from `asm-generic/mman.h`, the
/// same on every architecture); a finite limit, an unreadable one and a non-Linux host all
/// decline, because locking under a finite limit breaks the mappings the process makes
/// afterwards rather than protecting the ones it has.
pub fn lock_if_unlimited() -> LockOutcome {
    match memlock_limit() {
        MemlockLimit::Unlimited => {
            #[cfg(target_os = "linux")]
            {
                match lock_all() {
                    Ok(()) => LockOutcome::Locked,
                    Err(errno) => LockOutcome::Failed(errno),
                }
            }
            // Unreachable: the limit is never `Unlimited` off Linux. Written as a value rather
            // than `unreachable!()`: a platform refactor that made this reachable should
            // decline the lock, not abort the control loop.
            #[cfg(not(target_os = "linux"))]
            {
                LockOutcome::Unknown
            }
        }
        MemlockLimit::Limited(bytes) => LockOutcome::Limited { bytes },
        MemlockLimit::Unknown => LockOutcome::Unknown,
    }
}

/// What asking for the lock produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockOutcome {
    /// Locked, current pages and future ones. [`locked_kb`] is the probe that agrees.
    Locked,
    /// Declined: the limit is finite, and locking under it would fail the process's later
    /// mappings. The limit, in bytes.
    Limited {
        /// `RLIMIT_MEMLOCK`, in bytes.
        bytes: u64,
    },
    /// Declined: the limit could not be read, so the call was not risked. This is also the
    /// answer off Linux, where there is no `mlockall` to call.
    Unknown,
    /// The limit was unlimited and the call still failed, with its `errno`. Bizarre — the
    /// ordinary failures are all limit-shaped — and reported rather than retried.
    Failed(i32),
}

/// This process's `RLIMIT_MEMLOCK`: how much of its address space it may lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemlockLimit {
    /// No limit. This is the robot's configuration (`LimitMEMLOCK=infinity`), and the only one
    /// [`lock_if_unlimited`] locks under.
    Unlimited,
    /// A finite limit, in bytes. Locking is declined, not attempted.
    Limited(u64),
    /// The limit could not be read — not 64-bit Linux, or the call failed.
    Unknown,
}

/// Read `RLIMIT_MEMLOCK`.
///
/// `Unknown` anywhere the reading has no meaning: off Linux, off 64-bit (where `struct rlimit`
/// is not two `u64`), or when the call itself fails. All three decline the lock for the same
/// reason an unreadable probe is not a zero — see `F22` in `DRY-COVERAGE-PLAN.md`.
pub fn memlock_limit() -> MemlockLimit {
    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    {
        let mut rlim = sys::RLimit { cur: 0, max: 0 };
        // SAFETY: `rlim` is a live `struct rlimit` of the platform's own shape, and the call
        // writes exactly that on success.
        let rc = unsafe { sys::getrlimit(sys::RLIMIT_MEMLOCK, &mut rlim) };
        if rc != 0 {
            return MemlockLimit::Unknown;
        }
        if rlim.cur == sys::RLIM_INFINITY {
            MemlockLimit::Unlimited
        } else {
            MemlockLimit::Limited(rlim.cur)
        }
    }
    #[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
    {
        MemlockLimit::Unknown
    }
}

/// Lock this process's memory: current pages and every page faulted from now on.
///
/// Private, deliberately: an unconditional call under a finite limit sabotages the process's
/// later mappings (see the crate docs), so the only path to it is [`lock_if_unlimited`], which
/// reads the limit first.
#[cfg(target_os = "linux")]
fn lock_all() -> Result<(), i32> {
    // SAFETY: `mlockall` takes only flags and touches no pointer this passes it. The only
    // failure modes are a flag the kernel does not know (these two are the oldest ones it has)
    // and a limit exceeded, both reported as a return code rather than UB.
    let rc = unsafe { sys::mlockall(sys::MCL_CURRENT | sys::MCL_FUTURE) };
    if rc == 0 {
        Ok(())
    } else {
        Err(sys::errno())
    }
}

#[cfg(target_os = "linux")]
mod sys {
    use core::ffi::c_int;

    /// `MCL_CURRENT` from `asm-generic/mman.h`.
    pub const MCL_CURRENT: c_int = 1;
    /// `MCL_FUTURE` from `asm-generic/mman.h`.
    pub const MCL_FUTURE: c_int = 2;
    /// `RLIMIT_MEMLOCK` from `asm-generic/resource.h`. 8 on every architecture.
    #[cfg(target_pointer_width = "64")]
    pub const RLIMIT_MEMLOCK: c_int = 8;
    /// `RLIM_INFINITY`: no limit.
    #[cfg(target_pointer_width = "64")]
    pub const RLIM_INFINITY: u64 = u64::MAX;

    /// `struct rlimit` on 64-bit Linux: two `__kernel_ulong_t`.
    #[cfg(target_pointer_width = "64")]
    #[repr(C)]
    pub struct RLimit {
        /// The soft limit: what this process may lock.
        pub cur: u64,
        /// The hard limit: what it may raise the soft limit to.
        pub max: u64,
    }

    extern "C" {
        pub fn mlockall(flags: c_int) -> c_int;
        pub fn __errno_location() -> *mut c_int;
        #[cfg(target_pointer_width = "64")]
        pub fn getrlimit(resource: c_int, rlim: *mut RLimit) -> c_int;
    }

    /// The last `errno`, for a call that failed.
    pub fn errno() -> i32 {
        // SAFETY: `__errno_location` returns a pointer to this thread's `errno`, valid for the
        // lifetime of the thread, and it is read immediately after the failed call.
        unsafe { *__errno_location() }
    }
}

/// Kilobytes of this process's address space currently locked, or `None` when it cannot be read.
///
/// `/proc/self/status` because it is the kernel's own accounting, needs no crate, and answers
/// for exactly the process that locked. `None` on any failure — a missing file, an unreadable
/// one, a unit that is not kilobytes — because a probe that guessed would be worse than silence
/// next to a lock it is supposed to be checking.
pub fn locked_kb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/self/status").ok()?;
    parse_vm_lck(&text)
}

/// Pull `VmLck` out of a `/proc/self/status`.
///
/// Split out so the parsing is testable on any host. The line is `VmLck:` followed by kilobytes
/// — the kernel has printed it that way since forever — but the unit is checked rather than
/// assumed, for `skynet-statfs`'s reason: a unit misread is a budget misread, in the reassuring
/// direction.
pub fn parse_vm_lck(status: &str) -> Option<u64> {
    for line in status.lines() {
        let Some(rest) = line.strip_prefix("VmLck:") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let value: u64 = parts.next()?.parse().ok()?;
        if parts.next() != Some("kB") {
            return None;
        }
        return Some(value);
    }
    None
}

/// Whether the running kernel is PREEMPT_RT (§1, §23.5).
///
/// Three answers, not two. `Some(true)` is `/sys/kernel/realtime` reading `1`; `Some(false)` is
/// it reading `0`; `None` is the file being absent or unreadable, which is "cannot be
/// established" rather than "stock" — a kernel too old for the file is not a kernel proven
/// otherwise, and the distinction is the one a refusal will branch on when the amendment lands.
pub fn preempt_rt() -> Option<bool> {
    let text = std::fs::read_to_string("/sys/kernel/realtime").ok()?;
    parse_realtime(&text)
}

/// Read one sysfs bit. Split out so the parsing is testable on any host.
pub fn parse_realtime(text: &str) -> Option<bool> {
    match text.trim() {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// Kilobytes of swap the machine has active, or `None` when it cannot be read.
///
/// `SwapTotal` out of `/proc/meminfo` — the same line `install.sh --check` reads, so the
/// installer and the loop agree about the machine. Total rather than free: §23.5 wants *no
/// swap*, and free swap is still a pager the loop can land in. `None` on any failure, for
/// [`locked_kb`]'s reason: a probe that guessed zero would be the reassuring answer.
pub fn swap_kb() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_swap_total(&text)
}

/// Pull `SwapTotal` out of a `/proc/meminfo`. Split out so the parsing is testable on any
/// host. The unit is checked for [`parse_vm_lck`]'s reason.
pub fn parse_swap_total(meminfo: &str) -> Option<u64> {
    for line in meminfo.lines() {
        let Some(rest) = line.strip_prefix("SwapTotal:") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let value: u64 = parts.next()?.parse().ok()?;
        if parts.next() != Some("kB") {
            return None;
        }
        return Some(value);
    }
    None
}

/// The three machine readings `--tier host` is refused on (§23.5, as amended).
///
/// Read together by [`host_readings`] so the caller cannot refuse on a stale lock and a fresh
/// kernel. Every field has an unknown state, and every unknown refuses — see [`refuse_host`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostReadings {
    /// What asking for the lock produced. Only [`LockOutcome::Locked`] starts.
    pub lock: LockOutcome,
    /// Whether the running kernel is PREEMPT_RT. Only `Some(true)` starts.
    pub realtime: Option<bool>,
    /// Kilobytes of active swap. Only `Some(0)` starts.
    pub swap_kb: Option<u64>,
}

/// Read the machine `--tier host` would run on.
///
/// The lock is taken here rather than passed in: taking it is part of reading the machine,
/// and a caller that locked earlier and read later would report a lock from before whatever
/// else it did in between.
pub fn host_readings() -> HostReadings {
    HostReadings {
        lock: lock_if_unlimited(),
        realtime: preempt_rt(),
        swap_kb: swap_kb(),
    }
}

/// Every reason `--tier host` may not start on this machine, each naming what was read.
///
/// Empty means the machine backs the claim. Unknown readings are reasons, not passes: a
/// kernel too old for `/sys/kernel/realtime`, an unreadable lock limit and an unreadable
/// meminfo are all "cannot be established", and absence is not evidence of compliance. Pure
/// over its argument, so both directions are testable on any host — including the allow
/// direction, which no machine in this repository's reach can produce live.
pub fn refuse_host(readings: &HostReadings) -> Vec<String> {
    let mut reasons = Vec::new();
    match &readings.lock {
        LockOutcome::Locked => {}
        LockOutcome::Limited { bytes } => reasons.push(format!(
            "memory is not locked: RLIMIT_MEMLOCK is {bytes} bytes, and the lock is taken only \
             when it is unlimited (the robot sets LimitMEMLOCK=infinity)"
        )),
        LockOutcome::Unknown => reasons.push(
            "memory is not locked: the lock limit could not be read, so the call was not risked"
                .to_string(),
        ),
        LockOutcome::Failed(errno) => reasons.push(format!(
            "memory is not locked: mlockall failed (errno {errno}) under an unlimited lock limit"
        )),
    }
    match readings.realtime {
        Some(true) => {}
        Some(false) => {
            reasons.push("the kernel is not PREEMPT_RT (/sys/kernel/realtime reads 0)".to_string())
        }
        None => reasons.push(
            "the kernel cannot be shown to be PREEMPT_RT (/sys/kernel/realtime could not be read)"
                .to_string(),
        ),
    }
    match readings.swap_kb {
        Some(0) => {}
        Some(kb) => reasons.push(format!(
            "swap is active (SwapTotal {kb} kB); §23.5 wants no swap on a robot"
        )),
        None => reasons.push(
            "swap cannot be shown to be off (SwapTotal could not be read from /proc/meminfo)"
                .to_string(),
        ),
    }
    reasons
}

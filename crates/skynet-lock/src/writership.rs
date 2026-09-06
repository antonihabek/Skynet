//! §26.2's two roles: who **creates** a recording, and who **attaches** to one.
//!
//! [`WriterLock`](crate::WriterLock) was written when the rule was *one ring, one writer*, and it
//! enforced it by refusing the second process outright. §26.2's design pass narrowed what the lock
//! means:
//!
//! > The directory lock stays and its meaning narrows: from *one writer* to *one creator of the
//! > claim mapping*, with every writer thereafter attaching to it.
//!
//! This is that narrowing, written once. Both recorders need it — `skynetd` records events and
//! gaps, `skynet-kernel-host` records cycles, both belong in one recording — and neither may depend
//! on the other, which is the same argument that put the lock in a crate of its own.
//!
//! # The two roles, and what each is allowed to do
//!
//! | | Creator | Attacher |
//! |---|---|---|
//! | Holds the directory lock | yes, for its life | no |
//! | Creates or replaces the ring | yes | **never** — it would delete the creator's window |
//! | Seeds the claim counter | yes, from `Ring::open`'s scan | no |
//! | Appends | at claimed sequences | at claimed sequences |
//! | Seals | yes | **no** — see below |
//!
//! # Why an attacher does not seal
//!
//! Sealing reads the whole ring file and writes it out as a window. `skynet-claim` states the
//! property that lets a black box do without a seqlock: *"a claimed slot is written by exactly one
//! writer and read by nobody until that writer has finished with it, so the only shared mutable
//! state in this crate is the counter."* A second process sealing is a second reader, and two
//! processes sealing the same bytes would produce two windows of one recording with two digests.
//!
//! The creator holds the lock for its life, so the rule needs no new mechanism: sealing is the
//! thing the lock still gates, exactly as it did before this module existed. What the creator's own
//! seal may still catch is a slot an attacher is part-way through writing, and that is a torn
//! record with a failing CRC — dropped by [`Ring::ordered`], counted as damaged, never mistaken for
//! data. Losing the in-flight record of a concurrent writer is the honest cost of not putting a
//! lock on the real-time append path.
//!
//! # The race a bounded wait exists for
//!
//! A creator takes the lock, then opens or creates a ring, then seeds. That is not instant — a
//! 64 MiB preallocation is a file write — and an attacher arriving inside that window maps a
//! counter holding zero. Zero is not a sequence any ring hands out, and `Ring::stage_claimed`
//! refuses it rather than letting the arithmetic wrap it onto the ring's last slot.
//!
//! So attaching **waits** for the counter to be seeded, briefly, and refuses if it is not. Two
//! processes started together at boot is the ordinary case and it resolves in milliseconds. A
//! counter that stays zero means the holder took the lock and died before seeding, which is a
//! person's problem and says so.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use skynet_claim::{Claim, ClaimError};

use crate::{LockError, WriterLock};

/// How long [`Attaching::attach`] waits for the creator to seed the counter.
///
/// Long enough for a preallocation of §28's size on a slow disk, short enough that a robot whose
/// creator died mid-start is not held up by a recorder. It is a wait for another process to finish
/// a step it has already begun, not a retry loop.
pub const SEED_WAIT: Duration = Duration::from_secs(2);

/// Why a writer could not attach to a recording somebody else created.
#[derive(Debug)]
pub enum AttachError {
    /// The shared counter could not be mapped.
    ///
    /// On anything but Linux this is [`ClaimError::Unsupported`] and it is not a fault: a mapped
    /// atomic is what makes two writers safe, and without one the second writer is refused exactly
    /// as it was before §26.2 was built.
    Claim {
        /// What the mapping said.
        error: ClaimError,
        /// Who holds the directory, so the refusal names the other process.
        holder: String,
    },
    /// The counter was mapped and stayed zero: a holder that took the lock and did not seed.
    Unseeded {
        /// Who holds the directory.
        holder: String,
        /// How long this waited.
        waited: Duration,
    },
}

impl std::fmt::Display for AttachError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AttachError::Claim { error, holder } => write!(
                f,
                "this black box directory already has a writer ({holder}), and a second one needs \
                 the shared sequence counter §26.2 specifies, which could not be mapped: {error}\n\
                 Two writers over one ring without it would hand out the same sequence number \
                 twice and produce a recording that looks intact and is not. Stop the other \
                 writer, or point this one at another directory.",
            ),
            AttachError::Unseeded { holder, waited } => write!(
                f,
                "this black box directory is held by {holder}, which has not seeded the sequence \
                 counter after {:.1}s.\n\
                 A holder seeds it immediately after opening the ring, so one that has not either \
                 is still preallocating a very large ring or stopped between taking the lock and \
                 seeding. If that process is gone, its lock file is safe to delete.",
                waited.as_secs_f64(),
            ),
        }
    }
}

impl std::error::Error for AttachError {}

/// Which role a process got when it asked for writership.
#[derive(Debug)]
pub enum Joining {
    /// Nobody held the directory. This process creates the recording and seeds the counter.
    Creator(Creating),
    /// Somebody else holds it. This process attaches to the recording that is there.
    Attacher(Attaching),
}

/// A creator that holds the lock and has not seeded yet.
///
/// Two-phase because the seed is a fact about the ring: it is `Ring::open`'s scan, and the scan
/// cannot happen before the lock is taken without touching a file another process owns.
#[derive(Debug)]
pub struct Creating {
    lock: WriterLock,
    dir: PathBuf,
}

impl Creating {
    /// Seed the shared counter from the ring's scan, and become the writership.
    ///
    /// `next_sequence` is `Ring::next_sequence()` — the sequence the next record gets, off the
    /// scan of what is actually on disk.
    ///
    /// # A counter that cannot be mapped is not an error here
    ///
    /// The creator is a legitimate single writer, and a recording with one writer needs no shared
    /// counter at all: the ring's own in-memory sequence is correct, which is what every version of
    /// this code before §26.2 relied on. So a mapping failure leaves this writership without a
    /// claim, [`Writership::next`] answers `None`, and the caller appends the way it always has.
    /// The consequence lands on the *second* writer instead, where it belongs — it is refused, and
    /// the refusal names the mapping as the reason.
    pub fn seed(self, next_sequence: u64) -> Writership {
        let claim = Claim::seed(&self.dir, next_sequence).ok();
        let reclaimed_from = self.lock.reclaimed_from().map(str::to_string);
        Writership {
            _lock: Some(self.lock),
            claim,
            role: Role::Creator,
            reclaimed_from,
        }
    }

    /// Where the lock file is.
    pub fn lock_path(&self) -> &Path {
        self.lock.path()
    }

    /// The note of the dead holder whose lock this creator reclaimed, if it reclaimed one.
    pub fn reclaimed_from(&self) -> Option<&str> {
        self.lock.reclaimed_from()
    }
}

/// An attacher that knows who holds the directory and has not mapped the counter yet.
#[derive(Debug)]
pub struct Attaching {
    dir: PathBuf,
    holder: String,
}

impl Attaching {
    /// Who holds the directory, as the lock file names them.
    pub fn holder(&self) -> &str {
        &self.holder
    }

    /// Map the counter the creator seeded, waiting [`SEED_WAIT`] for it if it has to.
    pub fn attach(self) -> Result<Writership, AttachError> {
        self.attach_within(SEED_WAIT)
    }

    /// [`Attaching::attach`] with the wait supplied, so a test does not spend it.
    pub fn attach_within(self, wait: Duration) -> Result<Writership, AttachError> {
        let claim = Claim::attach_existing(&self.dir).map_err(|error| AttachError::Claim {
            error,
            holder: self.holder.clone(),
        })?;

        let started = Instant::now();
        loop {
            if claim.peek() != 0 {
                return Ok(Writership {
                    _lock: None,
                    claim: Some(claim),
                    role: Role::Attached,
                    reclaimed_from: None,
                });
            }
            if started.elapsed() >= wait {
                return Err(AttachError::Unseeded {
                    holder: self.holder,
                    waited: started.elapsed(),
                });
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Which of §26.2's roles a writership holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Took the lock: creates the ring, seeds the counter, and is the only party that may seal.
    Creator,
    /// Attached to a recording another process created.
    Attached,
}

/// The right to write a black box directory, and the counter the sequences come from.
///
/// Dropping it releases the lock if this is the creator, and unmaps the counter either way.
#[derive(Debug)]
pub struct Writership {
    // Ordered before `claim` so the lock is released last: a creator that dropped its lock while
    // its mapping was still live would let a new creator seed a counter under itself.
    _lock: Option<WriterLock>,
    claim: Option<Claim>,
    role: Role,
    reclaimed_from: Option<String>,
}

impl Writership {
    /// Ask for writership of `dir`, naming this process as `who`.
    ///
    /// The only error is a lock file that could not be created for a reason other than its already
    /// existing — an unwritable directory, in practice. A directory somebody else holds is not an
    /// error, it is [`Joining::Attacher`].
    pub fn join(dir: &Path, who: &str) -> Result<Joining, LockError> {
        match WriterLock::take(dir, who) {
            Ok(lock) => Ok(Joining::Creator(Creating {
                lock,
                dir: dir.to_path_buf(),
            })),
            Err(LockError::Held { holder, .. }) => Ok(Joining::Attacher(Attaching {
                dir: dir.to_path_buf(),
                holder,
            })),
            Err(other) => Err(other),
        }
    }

    /// Which role this is.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The dead holder this writership's lock was reclaimed from, if there was one.
    ///
    /// Worth carrying this far because of what it means on a robot: the previous `skynetd` was
    /// stopped by a signal and left its lock, which is the ordinary way a daemon stops. A line in
    /// the journal saying so is the difference between a fact and a mystery the day somebody is
    /// reading a black box directory to work out what happened.
    pub fn reclaimed_from(&self) -> Option<&str> {
        self.reclaimed_from.as_deref()
    }

    /// True when this writership may seal the window — the creator, and nobody else.
    pub fn may_seal(&self) -> bool {
        self.role == Role::Creator
    }

    /// Claim the next sequence number, or `None` when there is no shared counter.
    ///
    /// `None` means this is a lone creator on a platform without the mapping, and the caller should
    /// append through the ring's own counter. It never means "unavailable" for an attacher: an
    /// attacher without a counter does not exist, because [`Attaching::attach`] refuses.
    pub fn next(&self) -> Option<u64> {
        self.claim.as_ref().map(|claim| claim.next())
    }

    /// What the next claim will return, without taking it.
    pub fn peek(&self) -> Option<u64> {
        self.claim.as_ref().map(|claim| claim.peek())
    }
}

#[cfg(test)]
mod tests;

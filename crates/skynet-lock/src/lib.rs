//! The exclusive writer lock on a black box directory (§26.2).
//!
//! One rule, and it is small enough to state in a sentence: **a ring has one writer.**
//!
//! # Why a lock and not a convention
//!
//! [`skynet_blackbox::ring::Ring`] keeps its next sequence number **in memory**. Two rings over one
//! file hand out the same sequence twice, encode two records into one slot, and produce a recording
//! that is intact by every check a reader applies and wrong in the interval that matters. Nothing
//! detects it. It is found by an investigator reading the file to answer a question, months later,
//! and the answer is quietly false.
//!
//! That is the worst failure a black box has, and it is worth a file on disk to make impossible.
//!
//! # Why a crate
//!
//! Two callers, and the argument only holds if there is one implementation. `skynetd` records §26's
//! event channel — a configuration change, a grant, a dropped frame — and `skynet-kernel-host`
//! records control cycles. Both are recorders, both open a directory, and neither may depend on the
//! other: the daemon must not link the kernel host (§33.1 keeps it off that path) and the host must
//! not link the daemon (it would drag an HTTP server onto a real-time process).
//!
//! `skynet-blackbox` would be the natural home and cannot be: it is `#![no_std]`, argued as such in
//! its own header, and a lock is a file.
//!
//! # What this crate became when §26.2 got its design pass
//!
//! The sentence above is the rule as it was *enforced* — a second writer refused. §26.2's design
//! pass narrowed the lock's meaning from **one writer** to **one creator**: every writer after the
//! first attaches to a shared counter and claims sequences from it, and the lock decides which of
//! them creates the recording rather than which of them exists. [`writership`] is that, and it is
//! what the two recorders use. [`WriterLock`] is the mechanism underneath it and stays exactly as
//! it was.
//!
//! # A lock is not stolen from a process that exists — and a dead holder is not a holder
//!
//! A process killed with a signal leaves its lock behind, and this crate first refused to clear it:
//! *"a recorder that helpfully takes over a directory would take over one another process is still
//! writing, on the day the first one was merely slow rather than dead."* That reasoning is right and
//! the rule built on it was wrong, because of how a daemon actually stops.
//!
//! `skynetd` is stopped by `systemd` with `SIGTERM`. It installs no handler — that needs `unsafe`,
//! which this workspace forbids outside two crates — so `Drop` never runs and the lock is **always**
//! left behind. A stale lock was therefore not the exception it was argued as; it was the state
//! every robot is in after its first restart. The next daemon attached instead of creating, and an
//! attacher may not seal (§26.2), so **from the first restart onwards the robot could never seal a
//! window again** — silently, with the black box still recording into a ring nobody could close.
//! The end-to-end script found it in one run; nothing in the suite could, because a unit test does
//! not restart a daemon.
//!
//! So the rule is now the sharper form of the same sentence: a lock is never taken from a process
//! that **exists**, and the holder's existence is a fact the operating system answers rather than a
//! guess. The note carries the holder's pid *and the moment it started* — field 22 of
//! `/proc/<pid>/stat` — because a pid alone is not an identity: numbers are reused within hours and
//! reset entirely at a reboot, which is the ordinary case here. The pair is an identity, and it is
//! the kernel's answer rather than a heuristic. Three outcomes, and only one of them clears
//! anything:
//!
//! | The holder | What happens |
//! |---|---|
//! | alive | refused, named, exactly as before |
//! | gone — no such pid, or a pid that started at a different moment | the lock is reclaimed, and the reclaim is **reported** rather than silent |
//! | unknown — no `/proc`, or a note without both fields in it | refused. Fail closed: not being able to tell is not the same as knowing |
//!
//! What makes the refusal tolerable is what each caller does with it. `skynetd` starts **without a
//! recorder** and says so — §33.1 means a supervisor that cannot record is not a robot that cannot
//! stop. `skynetk` refuses to run, because a control loop whose cycles go nowhere is the one case
//! where the recording is the point.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod ring_file;
pub mod writership;

pub use writership::{AttachError, Attaching, Creating, Joining, Role, Writership};

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The file a holder writes into the directory.
pub const LOCK_FILE: &str = "writer.lock";

/// Why a lock could not be taken.
#[derive(Debug)]
pub enum LockError {
    /// Somebody else holds it.
    Held {
        /// The lock file.
        path: PathBuf,
        /// What the holder wrote into it, or a note that it wrote nothing.
        holder: String,
    },
    /// The lock file could not be created for a reason other than its existing.
    Io(std::io::Error),
}

impl std::fmt::Display for LockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LockError::Held { path, holder } => write!(
                f,
                "{} already has a writer: {holder}\n\
                 One ring has one writer (§26.2). Two would hand out the same sequence number \
                 twice and produce a recording that looks intact and is not, which is the one \
                 failure a black box must never have. Stop the other writer, or point this one at \
                 another directory. A lock whose holder has exited is reclaimed automatically; this \
                 one was not, which means the pid above is running — or that this machine could not \
                 ask, since not being able to tell is not the same as knowing. If you are certain \
                 the holder is gone, the lock file is safe to delete.",
                path.display()
            ),
            LockError::Io(error) => write!(f, "the writer lock could not be taken: {error}"),
        }
    }
}

impl std::error::Error for LockError {}

/// A held lock. Released on drop.
#[derive(Debug)]
pub struct WriterLock {
    path: PathBuf,
    reclaimed_from: Option<String>,
}

impl WriterLock {
    /// Take the lock on `dir`, naming this process as `who` in the file.
    ///
    /// `who` is what the next process to try will be shown, so it should say which program this is
    /// rather than only that something holds it.
    pub fn take(dir: &Path, who: &str) -> Result<WriterLock, LockError> {
        match Self::create(dir, who, None) {
            Err(LockError::Held { path, holder }) => match liveness(&holder) {
                // The holder is running, or this machine cannot say. Refused, named, and nothing on
                // disk is touched.
                Liveness::Alive | Liveness::Unknown => Err(LockError::Held { path, holder }),
                Liveness::Gone => {
                    // Removed only while it still says what it said when the liveness was decided.
                    // Two processes racing here both re-read, one wins `create_new`, and the loser
                    // is told the lock is held by the winner — which is the correct answer rather
                    // than a retry loop. The window in between is microseconds and needs two daemons
                    // started at once on one robot to reach, which is not a thing `systemd` does
                    // with one unit.
                    if fs::read_to_string(&path).unwrap_or_default().trim() != holder {
                        return Err(LockError::Held { path, holder });
                    }
                    fs::remove_file(&path).map_err(LockError::Io)?;
                    Self::create(dir, who, Some(holder))
                }
            },
            other => other,
        }
    }

    /// Create the lock file, or say who has it.
    fn create(
        dir: &Path,
        who: &str,
        reclaimed_from: Option<String>,
    ) -> Result<WriterLock, LockError> {
        let path = dir.join(LOCK_FILE);

        // `create_new` is atomic on every platform this targets, which is the whole mechanism.
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let note = format!("{who} {}\n", own_identity());
                // Best effort. A lock whose note could not be written is still a lock, and failing
                // to take it because the explanation did not land would be the wrong trade. What
                // that costs is stated rather than hidden: a note with no pid in it reads as
                // `Unknown` to the next process, so an unwritable note makes a lock nobody will
                // ever reclaim.
                let _ = file.write_all(note.as_bytes());
                let _ = file.sync_all();
                Ok(WriterLock {
                    path,
                    reclaimed_from,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let holder = fs::read_to_string(&path)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                Err(LockError::Held {
                    path,
                    holder: if holder.is_empty() {
                        "the lock file says nothing about who holds it".to_string()
                    } else {
                        holder
                    },
                })
            }
            Err(error) => Err(LockError::Io(error)),
        }
    }

    /// The note of the dead holder this lock was reclaimed from, if it was.
    ///
    /// Returned rather than logged here, because this crate has no opinion about where a robot's
    /// journal is — and not returning it at all would make a reclaim invisible, which is the one
    /// property that would make reclaiming as dangerous as the first version of this feared.
    pub fn reclaimed_from(&self) -> Option<&str> {
        self.reclaimed_from.as_deref()
    }

    /// Where the lock file is.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for WriterLock {
    fn drop(&mut self) {
        // Best effort, and the failure mode is a stale lock rather than a corrupted ring — which is
        // the direction this whole mechanism is built to fail in.
        let _ = fs::remove_file(&self.path);
    }
}

/// What the operating system says about the process a lock note names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Liveness {
    /// The pid is running and is still the program that wrote the note.
    Alive,
    /// No such pid, or a pid that now belongs to something else.
    Gone,
    /// This machine cannot say. Treated as alive everywhere it matters.
    Unknown,
}

/// Read a lock note — `"<who> pid <n> start <jiffies>"` — and ask whether that process is there.
///
/// `/proc` is the whole mechanism, and it is the right one here: a robot runs Linux (§23.5), the
/// answer comes from the kernel rather than from a heuristic, and no `unsafe` is needed to ask it.
///
/// # Why the start time and not the name
///
/// A pid on its own is not an identity. Pids are reused within hours on a busy machine and reset to
/// nothing on a reboot — which is the ordinary case here, since a robot that reboots leaves a lock
/// naming a pid that something else is now using. A lock reclaimed from an unrelated process that
/// happens to hold the number is precisely the takeover this must not perform.
///
/// The first version compared `/proc/<pid>/comm` to the `who` in the note, and it was wrong in the
/// dangerous direction: `who` is a label a caller chooses, `comm` is the executable's name, and
/// where they differ — every test in this crate, and any program that embeds a recorder — a
/// perfectly live holder reads as gone. Field 22 of `/proc/<pid>/stat` is the process's start time
/// in jiffies since boot, and *(pid, start time)* is an identity: it distinguishes the holder from
/// a reused pid and from anything at all after a reboot, and it cannot be fooled by a caller's
/// choice of label.
///
/// A note with no start time in it — one written by a version of this crate that did not record it,
/// or by a hand — is [`Liveness::Unknown`], and therefore refused.
fn liveness(note: &str) -> Liveness {
    let (Some(pid), Some(started)) = (field(note, "pid"), field(note, "start")) else {
        return Liveness::Unknown;
    };
    if !Path::new("/proc").is_dir() {
        return Liveness::Unknown;
    }
    match start_time(pid) {
        // No such process. The holder is gone, whatever it was called.
        None => Liveness::Gone,
        Some(now) if now == started => Liveness::Alive,
        // The pid exists and started at a different moment, so it is a different process wearing a
        // reused number.
        Some(_) => Liveness::Gone,
    }
}

/// A `<name> <number>` pair out of a lock note.
fn field(note: &str, name: &str) -> Option<u64> {
    note.split_whitespace()
        .skip_while(|word| *word != name)
        .nth(1)
        .and_then(|digits| digits.parse::<u64>().ok())
}

/// Field 22 of `/proc/<pid>/stat`: when this process started, in jiffies since boot.
///
/// The comm field is field 2, it is wrapped in parentheses, and it may itself contain spaces and
/// parentheses — so the only safe way to read anything after it is to split from the **last** `)`.
/// Everything after that begins at field 3, which puts field 22 at index 19.
fn start_time(pid: u64) -> Option<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(')')?.1;
    after_comm.split_whitespace().nth(19)?.parse().ok()
}

/// This process's own identity, for the note it writes.
fn own_identity() -> String {
    match start_time(u64::from(std::process::id())) {
        Some(started) => format!("pid {} start {started}", std::process::id()),
        // No `/proc`: the pid is written anyway, because it is what a person reads to go and look,
        // and a note without a start time is one no other process will ever reclaim.
        None => format!("pid {}", std::process::id()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("skynet-lock-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    #[test]
    fn one_holder_at_a_time() {
        let dir = scratch("one");
        let first = WriterLock::take(&dir, "skynetd").expect("a fresh directory has no writer");

        match WriterLock::take(&dir, "skynetk") {
            Err(LockError::Held { holder, .. }) => {
                assert!(
                    holder.contains("skynetd"),
                    "the refusal names who holds it, not just that it is held: {holder}",
                );
            }
            Err(other) => panic!("refused for the wrong reason: {other}"),
            Ok(_) => panic!("two writers were allowed onto one directory"),
        }

        drop(first);
        assert!(
            WriterLock::take(&dir, "skynetk").is_ok(),
            "releasing the lock lets the next writer in, so it is a lock and not a refusal",
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// Whether this machine can be asked about a pid at all.
    fn proc_answers() -> bool {
        Path::new("/proc/self/comm").exists()
    }

    /// A pid that certainly does not exist: a child, run and reaped.
    #[cfg(unix)]
    fn a_pid_that_has_exited() -> u32 {
        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .spawn()
            .expect("a shell");
        let pid = child.id();
        let _ = child.wait();
        pid
    }

    /// The lock a signal-stopped daemon leaves behind, on the machine a robot runs.
    ///
    /// This is the case the end-to-end script found: `skynetd` installs no `SIGTERM` handler, so
    /// `Drop` never runs and the lock is left on **every** restart. Refusing it meant a robot that
    /// could not seal a window again for the rest of its life.
    #[cfg(unix)]
    #[test]
    fn a_lock_left_by_a_process_that_has_exited_is_reclaimed_and_says_so() {
        if !proc_answers() {
            return;
        }
        let dir = scratch("exited");
        let dead = a_pid_that_has_exited();
        let note = format!("skynetd pid {dead} start 1");
        fs::write(dir.join(LOCK_FILE), format!("{note}\n")).expect("writable");

        let lock = WriterLock::take(&dir, "skynetd").expect("a dead holder is not a holder");
        assert_eq!(
            lock.reclaimed_from(),
            Some(note.as_str()),
            "a reclaim that nothing can report is a reclaim nobody can audit",
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// A lock whose holder is running is refused, which is the rule that has not changed.
    ///
    /// The note is the one this crate writes — a hand-written one would be missing the start time
    /// and would be refused for the weaker reason. Here the holder is **this** process, running,
    /// and the label on it is deliberately not this executable's name: the identity is the pid and
    /// its start time, and a live holder must not read as gone merely because a caller called
    /// itself something else. That is the shape of the bug this replaced.
    #[test]
    fn a_lock_whose_holder_is_running_is_never_taken() {
        let dir = scratch("alive");
        fs::write(dir.join(LOCK_FILE), format!("skynetd {}\n", own_identity())).expect("writable");

        let error = WriterLock::take(&dir, "skynetk").expect_err("the holder is running");
        let text = error.to_string();
        assert!(
            text.contains("skynetd"),
            "the refusal names who holds it: {text}"
        );
        assert!(text.contains("safe to delete"), "{text}");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A pid that is alive and is not the process that took the lock.
    #[cfg(unix)]
    #[test]
    fn a_reused_pid_is_not_the_holder_that_wrote_the_note() {
        if !proc_answers() {
            return;
        }
        let dir = scratch("reused");
        // This process's pid with somebody else's start time: what a lock file looks like after a
        // reboot handed the number to a different program.
        let started = start_time(u64::from(std::process::id())).expect("a start time");
        fs::write(
            dir.join(LOCK_FILE),
            format!("skynetd pid {} start {}\n", std::process::id(), started + 1),
        )
        .expect("writable");

        let lock = WriterLock::take(&dir, "skynetd").expect("a pid is not an identity");
        assert!(lock.reclaimed_from().is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    /// Not being able to tell is not the same as knowing.
    #[test]
    fn a_note_that_names_no_pid_is_refused_rather_than_reclaimed() {
        let dir = scratch("unknown");
        fs::write(dir.join(LOCK_FILE), "something was here\n").expect("writable");
        assert!(
            WriterLock::take(&dir, "skynetd").is_err(),
            "a lock this crate cannot reason about is one it leaves alone",
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// On a machine with no `/proc` — a developer's Windows box — nothing is ever reclaimed.
    #[test]
    fn without_proc_every_stale_lock_is_refused() {
        if proc_answers() {
            return;
        }
        let dir = scratch("noproc");
        fs::write(dir.join(LOCK_FILE), "skynetd pid 4242\n").expect("writable");
        let error = WriterLock::take(&dir, "skynetk").expect_err("no way to tell, so no takeover");
        assert!(error.to_string().contains("skynetd pid 4242"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// The drop is what releases it, so a lock cannot outlive the process that took it in the
    /// ordinary case.
    #[test]
    fn the_file_is_gone_when_the_lock_is() {
        let dir = scratch("drop");
        let lock = WriterLock::take(&dir, "skynetk").expect("taken");
        let path = lock.path().to_path_buf();
        assert!(path.exists());
        drop(lock);
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

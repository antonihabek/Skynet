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
//! # A stale lock is not stolen
//!
//! A process killed with `SIGKILL` leaves its lock behind. This does **not** clear it, and the
//! reason is that the alternative is worse: a recorder that helpfully takes over a directory would
//! take over one another process is still writing, on the day the first one was merely slow rather
//! than dead. So a stale lock names its holder and clearing it is a person's decision.
//!
//! What makes that tolerable is what each caller does with a refusal. `skynetd` starts **without a
//! recorder** and says so — §33.1 means a supervisor that cannot record is not a robot that cannot
//! stop. `skynetk` refuses to run, because a control loop whose cycles go nowhere is the one case
//! where the recording is the point.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

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
                 another directory. If the holder is gone, the lock file is safe to delete — it is \
                 not cleared automatically, because a recorder that took over a directory would \
                 take over one another process was still writing.",
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
}

impl WriterLock {
    /// Take the lock on `dir`, naming this process as `who` in the file.
    ///
    /// `who` is what the next process to try will be shown, so it should say which program this is
    /// rather than only that something holds it.
    pub fn take(dir: &Path, who: &str) -> Result<WriterLock, LockError> {
        let path = dir.join(LOCK_FILE);

        // `create_new` is atomic on every platform this targets, which is the whole mechanism.
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let note = format!("{who} pid {}\n", std::process::id());
                // Best effort. A lock whose note could not be written is still a lock, and failing
                // to take it because the explanation did not land would be the wrong trade.
                let _ = file.write_all(note.as_bytes());
                let _ = file.sync_all();
                Ok(WriterLock { path })
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

    /// A lock left behind is not cleared, and the message says how to clear it.
    #[test]
    fn a_stale_lock_is_named_rather_than_stolen() {
        let dir = scratch("stale");
        // What a killed process leaves: the file, with nobody holding it.
        fs::write(dir.join(LOCK_FILE), "skynetd pid 4242\n").expect("writable");

        let error = WriterLock::take(&dir, "skynetk").expect_err("the lock is held");
        let text = error.to_string();
        assert!(text.contains("skynetd pid 4242"), "{text}");
        assert!(
            text.contains("safe to delete"),
            "a stale lock has to say how to clear it, or it is a robot nobody can restart: {text}",
        );
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

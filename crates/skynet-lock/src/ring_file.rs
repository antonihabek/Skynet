//! Publishing the ring file, and waiting for one that is being published (§26.2).
//!
//! [`crate::writership`] settles *which* of two processes creates the recording. This settles the
//! handoff that follows it: the creator has to put a ring on disk and the attacher has to read the
//! one it put there, and until a robot existed with both units starting correctly, nothing had ever
//! done those two things at the same time.
//!
//! # What went wrong, on the first host that ever ran both
//!
//! The creator wrote the ring with `File::create` followed by `write_all` at the final path. A ring
//! sized for a robot is 64 MB, and for the whole of that write `<dir>/ring` is a file on disk at
//! every intermediate length. §33.1 starts the control loop **before** the supervisor, so there is
//! always a reader arriving during exactly that window:
//!
//! ```text
//! skynetd: no black box: skynetk pid 8055 start 5970 holds this black box directory and the
//!          ring it created cannot be opened here: Short { expected: 67108864, found: 14319600 }
//! ```
//!
//! The supervisor then ran with no black box for the life of the process — no retry, one log line,
//! and every §26 event it recorded went nowhere. It came back on the next restart, which is why it
//! is invisible on any machine that has booted twice: the ring is already there and already whole.
//! **It reproduces only on a robot whose `/data` is new, which is the state every robot ships in.**
//!
//! # The two halves
//!
//! [`publish`] writes to a neighbouring path and renames, so another process sees either nothing or
//! a complete ring — never a prefix of one. [`read_when_published`] is the other side: a writership
//! that answered `Attacher` has said a **live** creator holds the directory (a dead holder's lock is
//! reclaimed rather than honoured, which is this crate's own rule), so a ring that is not there yet
//! is a ring that is coming, and waiting for it is the correct thing to do rather than a retry loop
//! papering over a race. It is bounded anyway: a robot whose supervisor blocks forever on its
//! recorder is worse than one recording nothing.

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

/// How long an attacher waits for the creator to publish the ring.
///
/// The write it is waiting on is one `write_all` of the ring's whole length — 64 MB on a robot —
/// plus an `fsync`. Five seconds is roughly two orders of magnitude more than that costs on the
/// slowest storage this runs on, and it is a ceiling rather than a delay: the common case returns
/// on the first read.
pub const PUBLISH_WAIT: Duration = Duration::from_secs(5);

/// How often the wait looks again. Short enough to be invisible, long enough not to spin.
const POLL: Duration = Duration::from_millis(25);

/// Write `bytes` to `path` so that no other process can observe a partial one.
///
/// The file is created beside its destination and renamed onto it, which is atomic within a
/// filesystem. `fsync` happens before the rename: a ring that is visible at its name and empty
/// after a power cut would be worse than one that is absent, because absent is a state every reader
/// here already handles.
pub fn publish(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let incoming = incoming_path(path);
    // Truncating rather than refusing: what is at this path is a previous attempt by this same
    // creator — the writership lock means nobody else is writing it — and the previous attempt
    // failed, or the rename would have taken it away.
    let file = fs::File::create(&incoming)?;
    {
        use std::io::Write;
        let mut file = &file;
        file.write_all(bytes)?;
        file.flush()?;
    }
    file.sync_all()?;
    fs::rename(&incoming, path)
}

/// Read `path`, waiting up to `within` for a creator to publish it.
///
/// Only absence is waited on. Every other error is returned at once: a permission error or an
/// unreadable device is not a file that is about to appear, and retrying one for five seconds turns
/// a clear failure into a slow one.
pub fn read_when_published(path: &Path, within: Duration) -> io::Result<Vec<u8>> {
    let deadline = Instant::now() + within;
    loop {
        match fs::read(path) {
            Ok(bytes) => return Ok(bytes),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "{} did not appear within {:?}. The writership named a live creator, \
                             so this is a creator that took the lock and then did not publish a \
                             ring — its own log is where that is.",
                            path.display(),
                            within
                        ),
                    ));
                }
                std::thread::sleep(POLL);
            }
            Err(error) => return Err(error),
        }
    }
}

/// Where [`publish`] stages its write. Beside the destination, because a rename across filesystems
/// is not atomic and `/tmp` is a different one on most robots.
fn incoming_path(path: &Path) -> std::path::PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".incoming");
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("skynet-ring-file-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The defect, in the shape it had on the robot: a reader that opens the destination while a
    /// 64 MB write is in flight sees a prefix. With `publish`, there is no moment at which the
    /// destination holds one.
    #[test]
    fn a_published_file_is_never_visible_at_a_partial_length() {
        let dir = temp("atomic");
        let path = dir.join("ring");
        let bytes = vec![7u8; 4 * 1024 * 1024];

        let writer = {
            let path = path.clone();
            let bytes = bytes.clone();
            std::thread::spawn(move || publish(&path, &bytes).unwrap())
        };

        // Read the destination as fast as this thread can while the write runs. Every read either
        // finds nothing or finds the whole thing; a `Short` is the bug.
        let mut sightings = 0;
        while !writer.is_finished() {
            if let Ok(seen) = fs::read(&path) {
                assert_eq!(
                    seen.len(),
                    bytes.len(),
                    "a partial ring was visible at its name"
                );
                sightings += 1;
            }
        }
        writer.join().unwrap();
        assert_eq!(fs::read(&path).unwrap().len(), bytes.len());
        let _ = sightings;
        assert!(
            !dir.join("ring.incoming").exists(),
            "the staging file is renamed away"
        );
    }

    /// An attacher that arrives before the creator has published waits for it rather than
    /// reporting no black box — which is what the supervisor did on the first robot.
    #[test]
    fn an_attacher_waits_for_a_ring_that_is_still_being_written() {
        let dir = temp("waits");
        let path = dir.join("ring");
        let writer = {
            let path = path.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                publish(&path, &[3u8; 64]).unwrap();
            })
        };
        let seen = read_when_published(&path, PUBLISH_WAIT).expect("the ring arrives");
        assert_eq!(seen, vec![3u8; 64]);
        writer.join().unwrap();
    }

    /// And it is bounded. A creator that takes the lock and never publishes must not hold a
    /// supervisor's start open forever.
    #[test]
    fn the_wait_gives_up_and_says_what_it_was_waiting_for() {
        let dir = temp("bounded");
        let path = dir.join("ring");
        let error = read_when_published(&path, Duration::from_millis(80))
            .expect_err("nothing published it");
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert!(error.to_string().contains("did not appear"));
    }
}

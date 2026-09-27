//! Crash reports: what a dead process died of, in its own words (§64).
//!
//! Two processes write these and one reads them. The supervisor (`skynetd`) and the kernel host
//! (`skynetk`) each install a panic hook at start-up ([`install_hook`]) writing one JSON file
//! with the message and location, because a supervisor that panics would otherwise leave only a
//! journal line, and a journal that rotated is a crash nobody can diagnose. The supervisor's
//! next boot reads its own file before doing anything else — logged at error, cleared, carried
//! into every request context — and also takes the kernel's, because the kernel has no API of
//! its own to carry it on.
//!
//! One crate for both writers, because the alternative was the report format written twice — in
//! the daemon and in the kernel host, which is the arrangement `skynet-lock` exists to prevent.
//! The two hosts cannot share code any other way: the kernel host takes no dependency on the
//! daemon's crate, so the format lives here, below both.
//!
//! Two deliberate limits. First, each file holds one crash — the latest wins, because the file
//! answers *did the previous process die abnormally*, and the journal keeps the full history.
//! Second, only the supervisor clears. The kernel reads its own file at boot to log it, but
//! never clears it: it restarts in two seconds and the supervisor in five, so a kernel that
//! cleared at boot would destroy the report before its only reader woke up.
//!
//! The hook chains to the default hook rather than replacing it: a crash report that swallowed
//! the panic message would trade the journal's evidence for the file's, and both are wanted.

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

/// What one dead process left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrashReport {
    /// The binary that died, e.g. `skynetd`.
    pub binary: String,
    /// Its process id, so two crashes in one boot are distinguishable in the journal.
    pub pid: u32,
    /// Wall milliseconds at the panic (§35.1's correlation clock — the one thing it is for
    /// here is joining this report to the journal lines around it).
    pub wall_ms: u64,
    /// The panic message, verbatim.
    pub message: String,
    /// `file:line`, or the empty string when the panic carried no location.
    pub location: String,
}

impl CrashReport {
    /// Build a report from a panic. The payload downcast is the only fragile line: a panic
    /// with a non-string payload reports that fact rather than an empty message, because an
    /// empty message would read as a crash with nothing to say.
    pub fn from_panic(binary: &str, info: &std::panic::PanicHookInfo<'_>) -> Self {
        let message = if let Some(text) = info.payload().downcast_ref::<&str>() {
            (*text).to_string()
        } else if let Some(text) = info.payload().downcast_ref::<String>() {
            text.clone()
        } else {
            "(non-string panic payload)".to_string()
        };
        let location = info
            .location()
            .map(|at| format!("{}:{}", at.file(), at.line()))
            .unwrap_or_default();
        CrashReport {
            binary: binary.to_string(),
            pid: std::process::id(),
            wall_ms: wall_ms(),
            message,
            location,
        }
    }

    /// Render as JSON. Built by hand over [`skynet_json`], whose string escaping is what makes
    /// a panic message with quotes and newlines safe to carry — a `format!` here would be a
    /// second JSON writer, and the second one is always the one with the bug.
    pub fn to_json(&self) -> String {
        let mut body = skynet_json::Json::obj();
        body.set("binary", skynet_json::Json::from(self.binary.as_str()))
            .set("pid", skynet_json::Json::from(self.pid as f64))
            .set("wall_ms", skynet_json::Json::from(self.wall_ms as f64))
            .set("message", skynet_json::Json::from(self.message.as_str()))
            .set("location", skynet_json::Json::from(self.location.as_str()));
        body.to_canonical()
            .unwrap_or_else(|_| "{\"binary\":\"unknown\"}".to_string())
    }

    /// Parse back what [`CrashReport::to_json`] wrote. `None` on any shape it does not
    /// recognise: a corrupt crash file is deleted by the reader, not repaired, because a
    /// diagnosis reconstructed from half a report is how §64's rule gets broken.
    pub fn parse(text: &str) -> Option<Self> {
        let parsed = skynet_json::Json::parse(text).ok()?;
        Some(CrashReport {
            binary: parsed.get("binary")?.as_str()?.to_string(),
            pid: parsed.get("pid")?.as_f64()? as u32,
            wall_ms: parsed.get("wall_ms")?.as_f64()? as u64,
            message: parsed.get("message")?.as_str()?.to_string(),
            location: parsed.get("location")?.as_str()?.to_string(),
        })
    }
}

/// Wall milliseconds, best effort. Zero when the clock is unavailable — a crash report that
/// refused to write for want of a timestamp would be the file failing at its one job.
fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_millis() as u64)
        .unwrap_or(0)
}

/// Install the panic hook writing to `path`. The hook chains to the default hook, writes
/// best-effort (a hook that panics aborts the process, so every fallible step swallows its
/// error), and stamps the report with `binary`.
pub fn install_hook(path: PathBuf, binary: &'static str) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let report = CrashReport::from_panic(binary, info);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Written to a sibling and renamed, because the supervisor stats the kernel's file on
        // every pass and a reader that caught a half-written report would parse-fail it into
        // oblivion — `take_report` clears what it cannot parse. `rename` is atomic: readers
        // see the old report or the new one, never half of either.
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let _ = std::fs::write(&tmp, report.to_json());
        let _ = std::fs::rename(&tmp, &path);
        previous(info);
    }));
}

/// Read and clear the crash file: the report of the previous process, if it died abnormally.
/// A corrupt file is cleared and forgotten (see [`CrashReport::parse`]).
///
/// Only the supervisor takes. The kernel host reads with [`read_report`] and leaves the file
/// for its reader — see the crate docs for why clearing there would destroy the evidence.
pub fn take_report(path: &Path) -> Option<CrashReport> {
    let text = std::fs::read_to_string(path).ok()?;
    let _ = std::fs::remove_file(path);
    CrashReport::parse(&text)
}

/// Read the crash file without clearing it: the report of the previous process, if it died
/// abnormally. The kernel host's reader — it logs what it finds at boot and leaves the file
/// for the supervisor, the only clearer.
pub fn read_report(path: &Path) -> Option<CrashReport> {
    CrashReport::parse(&std::fs::read_to_string(path).ok()?)
}

/// How the previous process exited, as four states rather than two.
///
/// A crash file answers *did it panic*. It cannot answer *did it stop cleanly*, because a
/// `kill -9`, an OOM kill and an abort leave no report either — and *no crash was recorded*
/// is not *the exit was clean*. The clean-stop marker ([`StopMarker`]) is the other half of
/// the evidence: a process that chose its exit writes one, so the boot that finds neither
/// file knows the exit was unclean rather than merely unrecorded. The fourth state is the
/// honest remainder: on a machine nothing has ever run on, there is no previous exit at
/// all, and *unclean* would be a false alarm on every first boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviousExit {
    /// The previous process wrote its marker: it chose its exit.
    Clean,
    /// The previous process left a crash report, carried here — the state and the report
    /// travel as one, so no caller can hold a crash state with no report to show for it.
    Crashed(CrashReport),
    /// Neither file, on a machine with evidence of a prior run: the previous process was
    /// killed, ran out of memory, aborted, or lost power. Which one is not recorded —
    /// the journal around the death is where that question is answered.
    Unclean,
    /// Neither file, on a machine with no evidence of a prior run: nothing to judge.
    Unknown,
}

impl PreviousExit {
    /// The wire spelling, for `/health` and the surfaces that render it.
    pub fn as_str(&self) -> &'static str {
        match self {
            PreviousExit::Clean => "clean",
            PreviousExit::Crashed(_) => "crashed",
            PreviousExit::Unclean => "unclean",
            PreviousExit::Unknown => "unknown",
        }
    }
}

/// The clean-stop marker: proof the process chose its exit.
///
/// Armed at start-up, dropped at every return — and a drop during unwinding writes nothing
/// ([`std::thread::panicking`]), because a panic that marked itself clean would be the file
/// lying about the one death it exists to distinguish. Signals, aborts and power loss run
/// no destructors, so they leave no marker either, which is exactly the absence the next
/// boot reads as [`PreviousExit::Unclean`].
///
/// Only the supervisor's loop arms one. Single passes and subcommands are foreground — their
/// operator watches them die — and a `once` run against a live root would otherwise plant a
/// marker the daemon's next boot would misread as its own predecessor's clean exit. The
/// kernel host arms none either: it has no graceful stop path (every systemd stop SIGTERMs
/// it), so a kernel marker would read *unclean* after every routine restart — noise, not
/// signal.
pub struct StopMarker {
    path: PathBuf,
    mode: &'static str,
}

impl StopMarker {
    /// Arm the marker: dropping the guard writes it, unless the drop unwinds through a panic.
    pub fn arm(path: PathBuf, mode: &'static str) -> Self {
        StopMarker { path, mode }
    }
}

impl Drop for StopMarker {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        }
        let mut body = skynet_json::Json::obj();
        body.set("mode", skynet_json::Json::from(self.mode))
            .set("wall_ms", skynet_json::Json::from(wall_ms() as f64));
        let Ok(text) = body.to_canonical() else {
            return;
        };
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Atomic for the same reason as the crash file: a reader must see the old marker or
        // the new one, never half of either.
        let mut tmp = self.path.as_os_str().to_owned();
        tmp.push(".tmp");
        let _ = std::fs::write(&tmp, text);
        let _ = std::fs::rename(&tmp, &self.path);
    }
}

/// Take the clean-stop marker: whether the previous process chose its exit.
///
/// Clears whatever it finds — a corrupt marker is cleared and forgotten, like a corrupt
/// crash file, and counts as no marker: only a well-formed `{"mode":"daemon"}` is evidence
/// of anything.
pub fn take_marker(path: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let _ = std::fs::remove_file(path);
    skynet_json::Json::parse(&text)
        .ok()
        .and_then(|parsed| parsed.get("mode")?.as_str().map(str::to_string))
        .as_deref()
        == Some("daemon")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// The panic hook is process-global, so the two tests that panic take this lock: without
    /// it one test's panic lands in the other's temporary hook and both fail mystifyingly.
    static HOOK_LOCK: Mutex<()> = Mutex::new(());

    fn temp_path(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!("skynet-crash-test-{}-{label}", std::process::id()))
    }

    fn sample() -> CrashReport {
        CrashReport {
            binary: "skynetd".to_string(),
            pid: 4242,
            wall_ms: 1_786_000_000_000,
            message: "index out of bounds: the len is 3 but the \"index\" is 4\nsecond line"
                .to_string(),
            location: "src/api.rs:1234".to_string(),
        }
    }

    #[test]
    fn a_report_round_trips_through_the_file() {
        let path = temp_path("roundtrip");
        let report = sample();
        std::fs::write(&path, report.to_json()).expect("write the report");
        let back = take_report(&path).expect("read the report");
        assert_eq!(back, report);
        assert!(!path.exists(), "taking the report clears the file");
    }

    #[test]
    fn reading_leaves_the_file_for_the_clearer() {
        let path = temp_path("read");
        std::fs::write(&path, sample().to_json()).expect("write the report");
        assert_eq!(read_report(&path), Some(sample()));
        assert!(path.exists(), "reading is not taking");
        assert_eq!(take_report(&path), Some(sample()));
        assert!(!path.exists());
    }

    #[test]
    fn a_corrupt_file_is_cleared_and_forgotten() {
        let path = temp_path("corrupt");
        std::fs::write(&path, "{not json").expect("write garbage");
        assert!(take_report(&path).is_none());
        assert!(!path.exists());
    }

    #[test]
    fn a_missing_file_is_no_report() {
        let path = temp_path("missing");
        let _ = std::fs::remove_file(&path);
        assert!(take_report(&path).is_none());
        assert!(read_report(&path).is_none());
    }

    #[test]
    fn a_guard_dropped_cleanly_leaves_a_marker() {
        let path = temp_path("marker");
        drop(StopMarker::arm(path.clone(), "daemon"));
        assert!(take_marker(&path), "the drop wrote the marker");
        assert!(!path.exists(), "taking clears it");
    }

    #[test]
    fn a_guard_dropped_by_a_panic_writes_nothing() {
        // The marker's one lie would be a panic marking itself clean. The guard drops during
        // unwinding with `panicking()` true, so the file is never written — and the next boot
        // reads the crash file rather than a marker.
        let _hook = HOOK_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        let path = temp_path("panic-marker");
        let _ = std::panic::catch_unwind(|| {
            let _guard = StopMarker::arm(path.clone(), "daemon");
            panic!("boom");
        });
        assert!(!path.exists());
        assert!(!take_marker(&path));
    }

    #[test]
    fn a_corrupt_marker_counts_as_no_marker() {
        let path = temp_path("marker-corrupt");
        std::fs::write(&path, "{not json").expect("write garbage");
        assert!(!take_marker(&path));
        assert!(!path.exists());
    }

    #[test]
    fn from_panic_captures_message_and_location() {
        // The hook itself is process-global, so this captures what the hook would see
        // through a temporary hook that is restored before the assertion — under the lock
        // that serialises this test against the other one that panics, because without it
        // the other test's panic lands in this temporary hook.
        let _hook = HOOK_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        use std::sync::Arc;
        let seen: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
        let write_to = Arc::clone(&seen);
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let report = CrashReport::from_panic("test-binary", info);
            *write_to.lock().expect("capture the panic") = Some((report.message, report.location));
        }));
        let _ = std::panic::catch_unwind(|| panic!("a message with \"quotes\""));
        std::panic::set_hook(previous);
        let (message, location) = seen
            .lock()
            .expect("read the capture")
            .clone()
            .expect("the hook saw the panic");
        assert_eq!(message, "a message with \"quotes\"");
        assert!(location.contains("lib.rs"), "{location}");
    }
}

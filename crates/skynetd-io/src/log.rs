//! What this daemon says about itself, and how much of it (§38.1).
//!
//! Every message `skynetd` prints goes through here, so that §38.1's `log_level` setting is a
//! decision made in one place rather than a flag each call site checks for itself.
//!
//! # Why a process-global
//!
//! A log call must not cost a filesystem read. The setting lives on disk, so something has to hold
//! the current value in memory, and threading it to every call site would mean passing a logger
//! into functions whose only reason to take one is that they might complain. So: one atomic, read
//! by [`enabled`] and written by [`refresh`].
//!
//! That has a consequence worth stating rather than discovering. **A change made by editing the
//! file takes effect on the next reconcile pass**, because that is when the daemon re-reads it — up
//! to ten seconds at the default period. A change made through `POST /settings` takes effect
//! immediately, because the route runs in this process and sets the atomic itself. It is the same
//! shape as §38.3's black box mirroring, and it is the right direction: the record that must be
//! immediate is, and the one that can wait does.
//!
//! # The output is unchanged by the level
//!
//! A message that prints at all prints exactly what it printed before this module existed —
//! `skynetd: …`, no level tag, no timestamp. The level decides *whether*, never *what*. Anything
//! reading this daemon's stderr, including the end-to-end suite, was written against that prefix,
//! and a log line's shape is an interface even when nobody wrote it down as one.
//!
//! # JSON, and why it is a flag rather than a setting
//!
//! `--log-format json` prints one JSON object per line instead: the service, a wall timestamp, the
//! level, the operation in force (see below) and the message. It exists for aggregation — a fleet
//! log consumer that parses JSON cannot parse `skynetd: …`, and a human tailing one robot's
//! journal does not want to read JSON, so both shapes survive and the default is the human one.
//!
//! It is a start-up flag and deliberately not §38.1's fourth setting. A format cannot be toggled
//! mid-stream: every line before the toggle would be text in a consumer that parses JSON, which is
//! not a degraded mode but a corrupt one. The closed set of three stays closed, and the stream this
//! process writes is one shape from its first line to its last.
//!
//! # Operations: what each line was said in the course of
//!
//! Start-up, every reconcile pass and every served request each run inside an [`Operation`],
//! and every line printed while one is in force carries its id. The id is a counter within this
//! process — `boot`, `pass-12`, `req-7` — because an operation id is for joining lines to each
//! other, and the journal's timestamps join the process to everything else. Counters are
//! greppable where random ids are not, and determinism here costs nothing: uniqueness is only
//! ever needed within one stream.
//!
//! The current operation is thread-local rather than global, and the guard clears it on drop. Both
//! halves matter. A global would let a log line on one thread carry another thread's operation —
//! this daemon has two threads and only one of them logs, but a second logger is one refactor
//! away — and a set-without-clear would attribute the shutdown lines to whatever request was
//! served last. An operation that ended stops being one.

use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

pub use crate::settings::Level;

/// The level in force, as its ordinal. [`Level::Info`] until something says otherwise.
static LEVEL: AtomicU8 = AtomicU8::new(2);

/// Whether lines print as JSON objects. Set once at start-up from `--log-format`, never after —
/// see the module header for why a mid-stream toggle would corrupt the consumer.
static JSON: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// The operation in force on this thread, if one is. Set only through [`Operation`].
    static CURRENT_OP: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn ordinal(level: Level) -> u8 {
    match level {
        Level::Error => 0,
        Level::Warn => 1,
        Level::Info => 2,
        Level::Debug => 3,
    }
}

/// The level this process is printing at.
pub fn level() -> Level {
    match LEVEL.load(Ordering::Relaxed) {
        0 => Level::Error,
        1 => Level::Warn,
        3 => Level::Debug,
        _ => Level::Info,
    }
}

/// Print at this level, or louder.
pub fn set_level(level: Level) {
    LEVEL.store(ordinal(level), Ordering::Relaxed);
}

/// Take the level from the settings store.
///
/// Called on start-up and on each reconcile pass. Returns what it set, so a caller that wants to
/// say the level changed can.
pub fn refresh(layout: &crate::layout::Layout) -> Level {
    let level = crate::settings::read(layout).settings.log_level;
    set_level(level);
    level
}

/// Whether a message at this level would be printed.
///
/// Public because the macros below expand to a call to it, and because a caller doing real work to
/// build a message — walking a directory to say what is in it — should be able to skip the work
/// rather than only the printing.
pub fn enabled(level: Level) -> bool {
    ordinal(level) <= LEVEL.load(Ordering::Relaxed)
}

/// Print lines as JSON objects rather than `skynetd: …`.
///
/// Called once at start-up from `--log-format`, and never after: a stream that changed shape
/// halfway through would corrupt the consumer it was changed for. Tests set and unset it under
/// the same lock that guards the level.
pub fn set_json(on: bool) {
    JSON.store(on, Ordering::Relaxed);
}

/// Whether lines print as JSON objects.
pub fn json_enabled() -> bool {
    JSON.load(Ordering::Relaxed)
}

/// Render one message on stderr, in whichever shape is in force.
///
/// The text branch is byte-identical to what this printed before the JSON shape existed — the
/// `format_args!` go straight to `eprintln!` with no intermediate string, so the default path
/// allocates nothing it did not allocate before.
pub fn emit(level: Level, args: fmt::Arguments<'_>) {
    if json_enabled() {
        eprintln!(
            "{}",
            render_json(level, &args.to_string(), current_op().as_deref(), wall_ms())
        );
    } else {
        eprintln!("skynetd: {args}");
    }
}

/// One JSON log line: the service, a wall timestamp, the level, the operation and the message.
///
/// Wall time, not monotonic: a log timestamp is a record of when, for joining this line to the
/// journal and the fleet consumer, and §35.1's monotonic rule is about control decisions — no
/// decision reads this. Built over [`skynet_json`], whose string escaping is what makes a message
/// with quotes and newlines safe to carry; a `format!` here would be a second JSON writer.
pub fn render_json(level: Level, message: &str, op: Option<&str>, wall_ms: u64) -> String {
    let mut line = skynet_json::Json::obj();
    line.set("service", skynet_json::Json::from("skynetd"))
        .set("wall_ms", skynet_json::Json::Num(wall_ms as f64))
        .set("level", skynet_json::Json::from(level.as_str()))
        .set(
            "op",
            match op {
                Some(id) => skynet_json::Json::from(id),
                None => skynet_json::Json::Null,
            },
        )
        .set("message", skynet_json::Json::from(message));
    line.to_canonical()
        .unwrap_or_else(|_| "{\"service\":\"skynetd\"}".to_string())
}

/// Wall milliseconds, best effort. Zero when the clock is unavailable — a log line that refused
/// to print for want of a timestamp would be the module failing at its one job.
fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|span| span.as_millis() as u64)
        .unwrap_or(0)
}

/// The operation in force on this thread, if one is.
pub fn current_op() -> Option<String> {
    CURRENT_OP.with(|held| held.borrow().clone())
}

/// An operation in force: every line printed while this is alive carries its id.
///
/// Constructed at the top of start-up, a reconcile pass or a served request, dropped at the end.
/// Entering replaces whatever is in force and dropping clears rather than restores: start-up
/// contains the first pass the way the loop contains all of them, but a line is joined to the
/// innermost operation that printed it, and restoring the outer one afterwards would attribute
/// the lines between operations to an operation that ended. A stack would be machinery for
/// showing two ids where the line carries one.
pub struct Operation;

impl Operation {
    /// Put `id` in force until the returned guard is dropped.
    pub fn enter(id: &str) -> Self {
        CURRENT_OP.with(|held| *held.borrow_mut() = Some(id.to_string()));
        Operation
    }
}

impl Drop for Operation {
    fn drop(&mut self) {
        CURRENT_OP.with(|held| *held.borrow_mut() = None);
    }
}

/// Print one message, if the level in force permits it.
///
/// The level check stays in the macro, so a disabled call site formats nothing; the rendering
/// lives in [`emit`], so the JSON decision is made in one place rather than in four.
#[macro_export]
macro_rules! log_at {
    ($level:expr, $($arg:tt)*) => {
        if $crate::log::enabled($level) {
            $crate::log::emit($level, format_args!($($arg)*));
        }
    };
}

/// Something has gone wrong.
#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => { $crate::log_at!($crate::log::Level::Error, $($arg)*) };
}

/// Something is about to.
#[macro_export]
macro_rules! log_warn {
    ($($arg:tt)*) => { $crate::log_at!($crate::log::Level::Warn, $($arg)*) };
}

/// What this daemon is doing. The default level.
#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => { $crate::log_at!($crate::log::Level::Info, $($arg)*) };
}

/// How it decided. Off unless somebody asked for it.
#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => { $crate::log_at!($crate::log::Level::Debug, $($arg)*) };
}

/// Held by any test that moves the level.
///
/// The level is process-global by design, and `cargo test` runs tests on threads: two of them
/// setting it at once do not fail, they pass or fail depending on scheduling, which is the worst
/// available outcome. Every test that touches it takes this first. It lives outside `mod tests` so
/// that the suite in `crate::tests` — which exercises the route that sets the level — can take the
/// same one.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_admits_itself_and_everything_louder() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        let _restore = Restore(level());

        set_level(Level::Warn);
        assert!(enabled(Level::Error));
        assert!(enabled(Level::Warn));
        assert!(!enabled(Level::Info), "info is quieter than warn");
        assert!(!enabled(Level::Debug));

        set_level(Level::Debug);
        for level in Level::ALL {
            assert!(enabled(level), "{} at debug", level.as_str());
        }

        set_level(Level::Error);
        assert!(enabled(Level::Error));
        assert!(!enabled(Level::Warn));

        // The default a process starts at, which must be the same one `Settings::default()` names
        // — two defaults that can disagree is a daemon whose first messages depend on which of
        // them ran first.
        set_level(crate::settings::Settings::default().log_level);
        assert_eq!(level(), Level::Info);
    }

    #[test]
    fn a_json_line_parses_back_with_all_five_members() {
        let line = render_json(
            Level::Warn,
            "a message with \"quotes\" and\na newline",
            Some("pass-12"),
            1_786_000_000_000,
        );
        let parsed = skynet_json::Json::parse(&line).expect("a JSON line parses as JSON");
        assert_eq!(
            parsed.get("service").and_then(skynet_json::Json::as_str),
            Some("skynetd")
        );
        assert_eq!(
            parsed.get("wall_ms").and_then(skynet_json::Json::as_f64),
            Some(1_786_000_000_000.0)
        );
        assert_eq!(
            parsed.get("level").and_then(skynet_json::Json::as_str),
            Some("warn")
        );
        assert_eq!(
            parsed.get("op").and_then(skynet_json::Json::as_str),
            Some("pass-12")
        );
        assert_eq!(
            parsed.get("message").and_then(skynet_json::Json::as_str),
            Some("a message with \"quotes\" and\na newline"),
            "the message survives quoting and newlines verbatim"
        );
    }

    #[test]
    fn a_line_outside_any_operation_carries_a_null_op() {
        let line = render_json(Level::Info, "starting", None, 0);
        let parsed = skynet_json::Json::parse(&line).expect("a JSON line parses as JSON");
        assert_eq!(parsed.get("op"), Some(&skynet_json::Json::Null));
    }

    #[test]
    fn an_operation_is_in_force_until_its_guard_drops() {
        assert_eq!(current_op(), None);
        {
            let _pass = Operation::enter("pass-12");
            assert_eq!(current_op().as_deref(), Some("pass-12"));
        }
        assert_eq!(
            current_op(),
            None,
            "an operation that ended stops being one — the next line must not carry it"
        );
    }

    #[test]
    fn entering_replaces_and_dropping_clears() {
        // Start-up contains the first pass, but a line carries one id: the innermost. And when
        // the inner operation ends, the outer one is not restored — the lines between operations
        // belong to no operation.
        let _boot = Operation::enter("boot");
        assert_eq!(current_op().as_deref(), Some("boot"));
        {
            let _pass = Operation::enter("pass-1");
            assert_eq!(current_op().as_deref(), Some("pass-1"));
        }
        assert_eq!(current_op(), None);
    }

    #[test]
    fn the_format_is_text_unless_start_up_said_json() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|held| held.into_inner());
        let restore = json_enabled();
        set_json(true);
        assert!(json_enabled());
        set_json(false);
        assert!(!json_enabled());
        set_json(restore);
    }

    struct Restore(Level);

    impl Drop for Restore {
        fn drop(&mut self) {
            set_level(self.0);
        }
    }
}

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

use std::sync::atomic::{AtomicU8, Ordering};

pub use crate::settings::Level;

/// The level in force, as its ordinal. [`Level::Info`] until something says otherwise.
static LEVEL: AtomicU8 = AtomicU8::new(2);

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

/// Print one message, if the level in force permits it.
#[macro_export]
macro_rules! log_at {
    ($level:expr, $($arg:tt)*) => {
        if $crate::log::enabled($level) {
            eprintln!("skynetd: {}", format_args!($($arg)*));
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

    struct Restore(Level);

    impl Drop for Restore {
        fn drop(&mut self) {
            set_level(self.0);
        }
    }
}

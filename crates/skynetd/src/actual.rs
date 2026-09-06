//! Observed state — what the machine actually is (§33.3).
//!
//! > **Observation is never assumed.** `actual` is read from the system — the package store,
//! > systemd, the RAUC slot, the kernel's lease table — never from a memory of what `skynetd`
//! > believes it did earlier. A supervisor that trusts its own record of the past is a supervisor
//! > that disagrees with the machine it is supervising, and on a robot that disagreement is
//! > discovered by a person.
//!
//! This module is the *shape* of that reading. It contains no code that reads anything: the
//! observers are I/O and live outside this crate, which is what keeps the reconciler a pure
//! function of two documents.
//!
//! # Every field carries its age, or does not exist
//!
//! Not implemented here as a timestamp on each field, because there is a better place for it: an
//! observation is taken in one pass, so the whole [`Actual`] carries one `observed_at` and a
//! consumer can reason about the lot. A supervisor holding a package list from one second and a
//! unit list from another has a view of the machine that never existed — the same failure §25.1
//! describes for observation frames, one layer up.

/// What state a supervised unit is in.
///
/// The distinction between `Failed` and `Stopped` is the one that matters: a package that exited
/// because an operator stopped it and one that exited because it crashed five times in ten minutes
/// look identical on a process table and mean opposite things to a reconciler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitState {
    /// The process exists.
    Running,
    /// Not running, and nobody is trying to change that.
    Stopped,
    /// Crash-looped past the threshold and abandoned (§33.5). Not restarted; reported.
    Failed,
    /// Being started or stopped right now. A reconciler does nothing to a unit in this state — the
    /// previous action is still in flight, and acting again is how two starts race.
    Transitioning,
}

/// One supervised unit as the system reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitStatus {
    /// Package id.
    pub id: String,
    /// What the system says it is doing.
    pub state: UnitState,
    /// Restarts since it was last started by an operator, for crash-loop detection.
    pub restarts: u32,
    /// Exit status of the last exit, when there has been one.
    pub last_exit: Option<i32>,
}

/// One package present in the store.
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledPackage {
    /// Package id.
    pub id: String,
    /// The digest actually staged.
    pub digest: String,
    /// Whether the activation symlink points at this digest (§37.1).
    ///
    /// Staged-but-not-active is a real and useful state: fetching and verifying is free and can
    /// happen under a running policy, and only the symlink swap needs a safe state.
    pub activated: bool,
}

/// The A/B image slots (§12).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct OsSlots {
    /// Version running now.
    pub active: String,
    /// Version staged in the inactive slot, when one is staged.
    pub staged: Option<String>,
}

/// The machine, as observed in one pass.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Actual {
    /// Monotonic milliseconds at which this observation was taken.
    ///
    /// Supplied rather than read, for the reason the kernel takes no clock (§35.2): a function that
    /// reads a clock cannot be replayed, and replay is what turns "the robot did something strange
    /// on Tuesday" into a regression test.
    pub observed_at_ms: u64,
    /// The generation the robot last finished applying.
    pub generation: u64,
    /// Image slots.
    pub os: OsSlots,
    /// Everything in the package store.
    pub installed: Vec<InstalledPackage>,
    /// Everything the supervisor is supervising.
    pub units: Vec<UnitStatus>,
    /// The profile in force, by digest. `None` when none is applied.
    pub profile_digest: Option<String>,
    /// The ceilings that profile carries, read from the profile on disk rather than remembered.
    ///
    /// Present so the reconciler can tell a narrowing from a widening (§33.4) rather than assuming
    /// the pessimistic case for both. A digest alone cannot answer that question, and answering it
    /// wrongly in the safe direction still costs an operator a safe state to tighten a limit.
    pub profile_limits: Option<crate::desired::ProfileLimits>,
    /// Capability grants in force, as `(package id, token)`, sorted.
    pub grants: Vec<(String, String)>,
    /// Exclusive capability leases in force, as `(package id, token)` (§25.2).
    ///
    /// Distinct from `grants`: a grant says a package *may* use a capability, a lease says it
    /// currently *is*. Two packages may hold the same grant; only one holds the lease.
    pub leases: Vec<(String, String)>,
    /// Packages with a live (non-terminal) skill invocation (§54.4).
    ///
    /// Read off the daemon's own skill table on the observing pass — the same way `leases`
    /// is read off `leases.json` — because the reconciler cannot otherwise see the second
    /// writer of its process table. A unit in this set is wanted by definition for as long
    /// as its invocation is live, so the plan holds no stop for it; the set empty again, a
    /// leftover unit is reaped rather than supervised. A set rather than a list, because
    /// membership is the only question asked of it.
    pub live_skills: std::collections::BTreeSet<String>,
}

impl Actual {
    /// The store entry for a package, when there is one.
    pub fn installed(&self, id: &str) -> Option<&InstalledPackage> {
        self.installed.iter().find(|entry| entry.id == id)
    }

    /// The unit for a package, when the supervisor knows one.
    pub fn unit(&self, id: &str) -> Option<&UnitStatus> {
        self.units.iter().find(|unit| unit.id == id)
    }

    /// Whether a package is running right now.
    pub fn is_running(&self, id: &str) -> bool {
        matches!(
            self.unit(id).map(|unit| unit.state),
            Some(UnitState::Running)
        )
    }

    /// The tokens granted to a package.
    pub fn grants_for(&self, id: &str) -> Vec<&str> {
        self.grants
            .iter()
            .filter(|(package, _)| package == id)
            .map(|(_, token)| token.as_str())
            .collect()
    }
}

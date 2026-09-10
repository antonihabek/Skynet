//! §38.2's session override — the layer a person standing at the machine sets.
//!
//! The layering is `vendor defaults → fleet profile → site profile → cell profile → session
//! override`, and every layer above this one arrives signed and through desired state (§38.1).
//! This one does not, because it is answering a different question. A profile says *"in this cell,
//! half speed"*; a session override says *"half speed for the next twenty minutes while I have my
//! hands near it"*, and routing that through a Hub, a signing key and a reconcile pass would mean
//! nobody ever used it — they would move the robot at full speed instead, which is the outcome the
//! layer exists to prevent.
//!
//! What makes that safe to leave unsigned is that **it can only narrow**. `resolve_limits` composes
//! it by minimum with everything above it, so the worst a forged or corrupted override can do is
//! slow the robot down. That is the whole of the argument, and it is why this file must never grow
//! a field that could widen anything.
//!
//! ## It expires, and expiry lives here rather than in the kernel
//!
//! §38.2: *"Session overrides are time-boxed and expire — a 'maintenance mode, half speed' that
//! outlives the maintenance is a hazard that looks like a setting."* That is the sharpest sentence
//! in the section and it names the real failure: not a robot that is too slow, but a robot that is
//! quietly too slow for a week and a team that concludes the arm is faulty.
//!
//! The kernel cannot enforce it. It takes no clock — §35.2 makes `now_ms` a parameter to a cycle
//! rather than something the kernel reads — so it has no way to ask whether a deadline has passed.
//! The supervisor has the clock, so the supervisor decides: [`read`] returns `None` for an expired
//! override and reports that it did, and `resolve_limits` is simply never handed one. One clock in
//! the system rather than two that can disagree.
//!
//! **Wall time, not monotonic**, and for §45.4's reason rather than by oversight: a deadline set
//! before a reboot has to still be a deadline after one, and a monotonic reading means nothing
//! across a restart. §35.1 permits exactly this — it is a record's age, not a control decision.
//! An override therefore survives a reboot, which is right in both directions: the work is probably
//! still in progress, and if the machine has been off for a day the override is expired when it
//! comes back rather than reapplying itself.

use std::path::Path;

use skynet_contracts::safety::SessionOverride;
use skynet_json::Json;

use crate::layout::Layout;

/// The longest an override may run for.
///
/// Eight hours: longer than any single piece of bench work and shorter than the interval at which
/// somebody would forget one exists. A cap is not paternalism here, it is what keeps the type
/// honest — an override that could be set for a year is a profile that nobody signed, and §38.1
/// signs profiles precisely because a durable change to what a robot may do is a decision somebody
/// should have to stand behind.
pub const MAX_DURATION_MS: i64 = 8 * 60 * 60 * 1000;

/// A stored override, with the two times that decide whether it is still one.
#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    /// What it is called, for the notice that announces it.
    pub name: String,
    /// Who set it. §38.3 wants provenance on every configuration change.
    pub set_by: String,
    /// Why, in the setter's own words. Free text and optional.
    pub reason: Option<String>,
    /// When it was set, in milliseconds since the epoch.
    pub set_at_ms: i64,
    /// When it stops applying, in milliseconds since the epoch.
    pub expires_at_ms: i64,
    /// Narrowed joint velocity ceiling.
    pub joint_velocity_max_rad_s: Option<f64>,
    /// Narrowed joint torque ceiling.
    pub joint_torque_max_nm: Option<f64>,
    /// Narrowed base velocity ceiling.
    pub base_velocity_max_m_s: Option<f64>,
    /// Narrowed jerk ceiling.
    pub max_delta_per_cycle: Option<f64>,
}

impl Stored {
    /// The kernel's view of this override.
    pub fn as_kernel(&self) -> SessionOverride<'_> {
        SessionOverride {
            name: self.name.as_str(),
            joint_velocity_max_rad_s: self.joint_velocity_max_rad_s,
            joint_torque_max_nm: self.joint_torque_max_nm,
            base_velocity_max_m_s: self.base_velocity_max_m_s,
            max_delta_per_cycle: self.max_delta_per_cycle,
        }
    }

    /// How long is left, in milliseconds, or zero once it has expired.
    pub fn remaining_ms(&self, now_ms: i64) -> i64 {
        (self.expires_at_ms - now_ms).max(0)
    }

    /// Whether this override still applies at `now_ms`.
    pub fn live_at(&self, now_ms: i64) -> bool {
        now_ms < self.expires_at_ms
    }

    /// Whether it narrows anything at all.
    ///
    /// An override with four absent ceilings is not an error and is not a narrowing either: it
    /// would compose to no change and still raise the notice that says the robot is being held
    /// back. [`set`] refuses it for that reason — a sentence saying a robot is slowed, about a
    /// robot that is not, is exactly the kind of false statement §32.2 spends its rule on.
    pub fn narrows_something(&self) -> bool {
        self.joint_velocity_max_rad_s.is_some()
            || self.joint_torque_max_nm.is_some()
            || self.base_velocity_max_m_s.is_some()
            || self.max_delta_per_cycle.is_some()
    }

    fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("name", Json::from(self.name.as_str()));
        out.set("set_by", Json::from(self.set_by.as_str()));
        if let Some(reason) = &self.reason {
            out.set("reason", Json::from(reason.as_str()));
        }
        out.set("set_at_ms", Json::Num(self.set_at_ms as f64));
        out.set("expires_at_ms", Json::Num(self.expires_at_ms as f64));
        // Absent rather than null for an unset ceiling, which is the corpus rule (§31.4) and means
        // the same thing here as it does there: nobody narrowed this one.
        for (key, value) in [
            ("joint_velocity_max_rad_s", self.joint_velocity_max_rad_s),
            ("joint_torque_max_nm", self.joint_torque_max_nm),
            ("base_velocity_max_m_s", self.base_velocity_max_m_s),
            ("max_delta_per_cycle", self.max_delta_per_cycle),
        ] {
            if let Some(number) = value {
                out.set(key, Json::Num(number));
            }
        }
        out
    }

    fn from_json(document: &Json) -> Option<Stored> {
        let number = |key: &str| document.get_present(key).and_then(Json::as_f64);
        Some(Stored {
            name: document.get("name").and_then(Json::as_str)?.to_string(),
            set_by: document
                .get("set_by")
                .and_then(Json::as_str)
                .unwrap_or("unattributed")
                .to_string(),
            reason: document
                .get_present("reason")
                .and_then(Json::as_str)
                .map(str::to_string),
            set_at_ms: number("set_at_ms")? as i64,
            expires_at_ms: number("expires_at_ms")? as i64,
            joint_velocity_max_rad_s: number("joint_velocity_max_rad_s"),
            joint_torque_max_nm: number("joint_torque_max_nm"),
            base_velocity_max_m_s: number("base_velocity_max_m_s"),
            max_delta_per_cycle: number("max_delta_per_cycle"),
        })
    }
}

/// What reading the store found.
#[derive(Debug, Clone, PartialEq)]
pub enum Reading {
    /// Nothing is set.
    None,
    /// An override is set and still applies.
    Live(Stored),
    /// An override is set and its deadline has passed.
    ///
    /// Carried rather than collapsed into [`Reading::None`], because the two are different facts
    /// and the caller has different work to do: an expired override is a file to delete and an
    /// event to record, and a console that showed it as "nothing set" would leave somebody
    /// wondering why the robot sped up.
    Expired(Stored),
    /// A file is there and could not be read as an override.
    ///
    /// Held rather than applied, and reported rather than ignored. The safe direction for an
    /// unreadable *narrowing* is to drop it — a robot runs at its profile's ceilings, which is
    /// where it would be with no override at all — but somebody set that file expecting the robot
    /// to be slowed, and silence would let them believe it was.
    Unreadable(String),
}

/// Read the override in force, if there is one.
///
/// `now_ms` is wall time in milliseconds. Passed in rather than read here for the reason every
/// clock in this daemon is passed in: a function that reads the time cannot be tested against a
/// deadline without waiting for it.
pub fn read(layout: &Layout, now_ms: i64) -> Reading {
    let path = layout.session_override();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Reading::None,
        Err(error) => return Reading::Unreadable(format!("{path:?} could not be read: {error}")),
    };
    let document = match Json::parse(&text) {
        Ok(document) => document,
        Err(error) => {
            return Reading::Unreadable(format!("{path:?} is not readable JSON: {error}"))
        }
    };
    match Stored::from_json(&document) {
        None => Reading::Unreadable(format!(
            "{path:?} is missing a field a session override must have: name, set_at_ms and expires_at_ms"
        )),
        Some(stored) if stored.live_at(now_ms) => Reading::Live(stored),
        Some(stored) => Reading::Expired(stored),
    }
}

/// Why a request to set an override was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// The requested duration is zero, negative, or past [`MAX_DURATION_MS`].
    Duration(String),
    /// Every ceiling was absent, so it would narrow nothing.
    NarrowsNothing,
    /// A ceiling was zero, negative, or not a number.
    Ceiling(String),
    /// It could not be written.
    Write(String),
}

impl Refusal {
    /// The sentence for this refusal — §64's rule that a refusal names its fix.
    pub fn detail(&self) -> String {
        match self {
            Refusal::Duration(said) => said.clone(),
            Refusal::NarrowsNothing => concat!(
                "A session override has to narrow something. Set at least one of ",
                "joint_velocity_max_rad_s, joint_torque_max_nm, base_velocity_max_m_s or ",
                "max_delta_per_cycle."
            )
            .to_string(),
            Refusal::Ceiling(said) => said.clone(),
            Refusal::Write(said) => said.clone(),
        }
    }
}

/// Set the override, replacing whatever was there.
///
/// Returns the stored document, so the caller can record §38.3's audit entry and black box event
/// from the same values that reached the disk rather than from the ones it was asked for.
pub fn set(layout: &Layout, stored: Stored) -> Result<Stored, Refusal> {
    let duration = stored.expires_at_ms - stored.set_at_ms;
    if duration <= 0 {
        return Err(Refusal::Duration(
            "A session override has to expire in the future. Give it a duration.".to_string(),
        ));
    }
    if duration > MAX_DURATION_MS {
        return Err(Refusal::Duration(format!(
            "A session override may run for at most {} hours. Longer than that is a change to how \
             this robot behaves rather than to an afternoon, and belongs in a signed profile.",
            MAX_DURATION_MS / (60 * 60 * 1000)
        )));
    }
    if !stored.narrows_something() {
        return Err(Refusal::NarrowsNothing);
    }
    for (label, value) in [
        ("joint_velocity_max_rad_s", stored.joint_velocity_max_rad_s),
        ("joint_torque_max_nm", stored.joint_torque_max_nm),
        ("base_velocity_max_m_s", stored.base_velocity_max_m_s),
        ("max_delta_per_cycle", stored.max_delta_per_cycle),
    ] {
        // A negative or non-finite ceiling is refused rather than clamped. `narrowest()` would
        // happily take -1 rad/s as the tightest of its inputs and the robot would then be unable to
        // move at all — which is safe, and is also a robot somebody cannot diagnose from its
        // limits page. Zero is refused for the same reason: it is indistinguishable from a stop
        // that nobody commanded.
        if let Some(number) = value {
            if !number.is_finite() || number <= 0.0 {
                return Err(Refusal::Ceiling(format!(
                    "{label} has to be a number greater than zero. A ceiling of {number} is a \
                     robot that cannot move, which is not what a session override is for."
                )));
            }
        }
    }

    let path = layout.session_override();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| Refusal::Write(format!("{parent:?} could not be created: {error}")))?;
    }
    let text = stored
        .to_json()
        .to_canonical()
        .map_err(|error| Refusal::Write(format!("the override could not be encoded: {error}")))?;
    write_atomically(&path, &text)
        .map_err(|error| Refusal::Write(format!("{path:?} could not be written: {error}")))?;
    Ok(stored)
}

/// Remove the override, and say whether there was one.
///
/// Idempotent: clearing nothing is not an error, because the operator's intent — *this robot should
/// not be under an override* — is satisfied either way, and a 404 would send somebody looking for a
/// file whose absence is what they wanted.
pub fn clear(layout: &Layout) -> std::io::Result<bool> {
    match std::fs::remove_file(layout.session_override()) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// Write through a temporary file and rename, so a crash mid-write cannot leave half a document.
///
/// The same care the desired state and the grant store take, and it matters more here than the
/// small size of the file suggests: a torn override reads as [`Reading::Unreadable`], which holds
/// the robot at its profile's ceilings — safe, and still a bench session interrupted for a reason
/// nobody can see.
fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

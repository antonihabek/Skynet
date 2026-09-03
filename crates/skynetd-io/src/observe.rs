//! Reading the machine (§33.3).
//!
//! > **Observation is never assumed.** `actual` is read from the system — the package store,
//! > systemd, the RAUC slot, the kernel's lease table — never from a memory of what `skynetd`
//! > believes it did earlier. A supervisor that trusts its own record of the past is a supervisor
//! > that disagrees with the machine it is supervising, and on a robot that disagreement is
//! > discovered by a person.
//!
//! [`observe`] is that sentence as a function. It takes the store, a launcher and a slot reader,
//! and returns a [`skynetd::Actual`] — the type the reconciler consumes and, deliberately, has no
//! way to construct for itself.
//!
//! # One pass, one timestamp
//!
//! Everything is read in a single call and the whole observation carries one `observed_at_ms`. A
//! supervisor holding a package list from one second and a unit list from another has a view of the
//! machine that never existed — the failure §25.1 describes for observation frames, one layer up.
//! It is not free: a package that stopped between the store read and the systemd read is reported
//! as running, and the next pass corrects it. That is the level-triggered bargain, and it is a
//! better trade than a composite view nobody can reason about.
//!
//! # Grants are read, leases are read, and both are files
//!
//! Grants live in `/data/state/grants.json` because they survive a reboot and an operator has to be
//! able to see them. Leases are the kernel's (§25.2) and this edition has no channel to the kernel,
//! so [`read_leases`] reads the file `skynetd` itself writes — which is a memory of the past
//! wearing a filesystem, and it is named as that rather than presented as an observation.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use skynet_json::Json;
use skynetd::actual::{Actual, InstalledPackage, OsSlots};
use skynetd::desired::ProfileLimits;

use crate::launch::Launcher;
use crate::layout::Layout;
use crate::store::Store;

/// Whatever can say which OS image is running and which is staged (§12).
pub trait Slots {
    /// Read the slots.
    fn read(&self) -> OsSlots;
}

/// RAUC, the A/B update framework the images are built with.
#[derive(Debug, Clone, Default)]
pub struct RaucSlots;

impl Slots for RaucSlots {
    /// Ask `rauc status` which slot is booted and what is in the other one.
    ///
    /// An absent or unparseable answer produces **empty** slots, not a guess. §33.4 gates an OS
    /// apply on a safe state and stages before it; a reconciler told the active version is `""`
    /// emits a stage for whatever the Hub wants and nothing else, which is the harmless direction.
    /// A reconciler told the wrong active version would apply an image the robot is already
    /// running.
    fn read(&self) -> OsSlots {
        let Ok(output) = std::process::Command::new("rauc")
            .args(["status", "--output-format=json"])
            .output()
        else {
            return OsSlots::default();
        };
        parse_rauc_status(&String::from_utf8_lossy(&output.stdout))
    }
}

/// Read RAUC's JSON status into slots.
///
/// Pure, because the shape of somebody else's JSON is exactly the thing that changes between
/// releases and exactly the thing nobody notices has changed until a fleet stops updating.
pub fn parse_rauc_status(text: &str) -> OsSlots {
    let Ok(document) = Json::parse(text) else {
        return OsSlots::default();
    };
    let booted = document
        .get_present("booted")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string();

    let mut active = String::new();
    let mut staged = None;
    for slot in document.arr_or_empty("slots") {
        // RAUC writes slots as one-key objects: `{"rootfs.0": {…}}`.
        let Some(map) = slot.as_obj() else { continue };
        for (name, body) in map {
            let version = body
                .get("slot_status")
                .and_then(|status| status.get_present("bundle_version"))
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string();
            if version.is_empty() {
                continue;
            }
            let boot_name = body
                .get_present("bootname")
                .and_then(Json::as_str)
                .unwrap_or(name.as_str());
            if boot_name == booted || name == &booted {
                active = version;
            } else {
                staged = Some(version);
            }
        }
    }
    OsSlots { active, staged }
}

/// Slots supplied rather than read — for the suite, and for a machine with no RAUC.
#[derive(Debug, Clone, Default)]
pub struct FixedSlots(pub OsSlots);

impl Slots for FixedSlots {
    fn read(&self) -> OsSlots {
        self.0.clone()
    }
}

/// Read the whole machine in one pass.
///
/// `now_ms` is supplied for the reason the kernel takes no clock (§35.2): an observation that read
/// its own timestamp could not be replayed, and replay is what turns "the robot did something
/// strange on Tuesday" into a regression test.
pub fn observe<L: Launcher, S: Slots>(
    store: &Store,
    launcher: &L,
    slots: &S,
    now_ms: u64,
) -> Actual {
    let layout = store.layout().clone();

    // Two reads, and the order between them is load-bearing. A package with a `current` link is
    // activated by construction — the link *is* the activation (§37.1) — and a blob with no link is
    // staged and inert, which §37.1 requires to be a distinct state because fetching and verifying
    // is free under a running policy and only the swap waits for a gate.
    //
    // The active entry is pushed first for each id, because `Actual::installed` answers with the
    // first match and a reconciler shown a stale staged digest for a package that is running would
    // plan an activation of the version it just replaced.
    let active = store.installed();
    let mut installed: Vec<InstalledPackage> = active
        .iter()
        .map(|(id, digest)| InstalledPackage {
            id: id.as_str(),
            digest: digest.clone(),
            activated: true,
        })
        .collect();
    for (id, digest) in store.staged() {
        if active.iter().any(|(active_id, _)| active_id == &id) {
            continue;
        }
        installed.push(InstalledPackage {
            id: id.as_str(),
            digest,
            activated: false,
        });
    }
    installed
        .sort_by(|a, b| (&a.id, !a.activated, &a.digest).cmp(&(&b.id, !b.activated, &b.digest)));

    let units = launcher.observe().unwrap_or_default();
    let grants = read_grants(&layout);
    let leases = read_leases(&layout);
    let (profile_digest, profile_limits) = read_profile(&layout);

    Actual {
        observed_at_ms: now_ms,
        generation: read_generation(&layout),
        os: slots.read(),
        installed,
        units,
        profile_digest,
        profile_limits,
        grants,
        leases,
    }
}

/// The generation this robot last finished applying (§11).
fn read_generation(layout: &Layout) -> u64 {
    read_json(&layout.state().join("generation.json"))
        .and_then(|value| value.get_present("generation").and_then(Json::as_f64))
        .filter(|n| n.is_finite() && *n >= 0.0)
        .map(|n| n as u64)
        .unwrap_or(0)
}

/// Capability grants in force, as `(package, token)`, sorted.
pub fn read_grants(layout: &Layout) -> Vec<(String, String)> {
    read_pairs(&layout.grants())
}

/// Exclusive leases in force.
pub fn read_leases(layout: &Layout) -> Vec<(String, String)> {
    read_pairs(&layout.state().join("leases.json"))
}

fn read_pairs(path: &Path) -> Vec<(String, String)> {
    let Some(document) = read_json(path) else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    if let Some(map) = document.as_obj() {
        for (id, tokens) in map {
            for token in tokens.as_arr().unwrap_or(&[]) {
                if let Some(token) = token.as_str() {
                    pairs.push((id.clone(), token.to_string()));
                }
            }
        }
    }
    pairs.sort();
    pairs.dedup();
    pairs
}

/// Write a `(package, token)` table back.
pub fn write_pairs(path: &Path, pairs: &[(String, String)]) -> std::io::Result<()> {
    let mut grouped: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (id, token) in pairs {
        grouped.entry(id).or_default().push(token);
    }
    let mut document = Json::obj();
    for (id, mut tokens) in grouped {
        tokens.sort();
        tokens.dedup();
        document.set(id, Json::Arr(tokens.into_iter().map(Json::from).collect()));
    }
    let text = document
        .to_canonical()
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    write_atomic(path, text.as_bytes())
}

/// The operator profile in force: its digest and the ceilings it carries (§38).
///
/// The limits are read from the file rather than remembered, because §33.4's asymmetry needs to
/// tell a narrowing from a widening, and a digest alone cannot answer that. Answering it wrongly in
/// the safe direction still costs an operator a safe state to tighten a limit.
pub fn read_profile(layout: &Layout) -> (Option<String>, Option<ProfileLimits>) {
    let Some(document) = read_json(&layout.active_profile()) else {
        return (None, None);
    };
    let digest = document
        .get_present("digest")
        .and_then(Json::as_str)
        .map(str::to_string);
    let limits = document.get("limits").cloned().unwrap_or_else(Json::obj);
    let number = |key: &str| {
        limits
            .get_present(key)
            .and_then(Json::as_f64)
            .filter(|n| n.is_finite())
    };
    (
        digest,
        Some(ProfileLimits {
            joint_velocity_max_rad_s: number("joint_velocity_max_rad_s"),
            joint_torque_max_nm: number("joint_torque_max_nm"),
            base_velocity_max_m_s: number("base_velocity_max_m_s"),
            max_delta_per_cycle: number("max_delta_per_cycle"),
            self_collision_margin_m: number("self_collision_margin_m"),
            proximity: limits.get_present("proximity").map(|prox| {
                let distance = |key: &str| {
                    prox.get_present(key)
                        .and_then(Json::as_f64)
                        .filter(|n| n.is_finite())
                };
                skynetd::desired::ProfileProximity {
                    stop_distance_m: distance("stop_distance_m"),
                    slow_distance_m: distance("slow_distance_m"),
                    slow_factor: distance("slow_factor"),
                }
            }),
        }),
    )
}

/// Read and parse a JSON file, or nothing.
pub fn read_json(path: &Path) -> Option<Json> {
    let text = fs::read_to_string(path).ok()?;
    Json::parse(&text).ok()
}

/// Write a file so that a reader sees either the old contents or the new ones.
///
/// Write-then-rename. The alternative — truncate and write — has a window in which the file is
/// empty, and the file this is used for is the desired state: a robot that lost power there would
/// come back to §33.8's ladder with a corrupt document, which is recoverable, but recoverable is a
/// worse property than impossible.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)
}

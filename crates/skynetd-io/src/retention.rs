//! Retention of sealed windows (§80, decided 2026-09-04).
//!
//! §79.4 keeps every sealed window and says so in as many words — *"the sealed file is not
//! deleted"* — and then names the consequence rather than resolving it: *"retention of sealed
//! windows is a real question and this amendment does not answer it; it is listed in §80 rather
//! than settled in passing."* A robot that seals daily for a year does not survive §28's disk.
//!
//! The owner's rule, and the reasoning it answers:
//!
//! > The Hub's acknowledgement is **another party's word**, and §13 exists precisely for the day
//! > that party is the one being asked questions — so acknowledgement alone never authorises a
//! > deletion. A window an **incident names is never reclaimed**. A routine window may be reclaimed
//! > only when a receipt exists, the receipt's digest **is** the window's digest, and the disk is
//! > **under the floor** — oldest first. A reclaim writes a **tombstone**: digest, seal time, byte
//! > count, reason. The chain of custody is therefore complete for ever and only the bytes are ever
//! > freed, which is the difference between a robot that cannot account for a window and one that
//! > can say what it held and when it let it go.
//!
//! # Three conditions, and none of them is sufficient alone
//!
//! The rule reads as a conjunction and each term is doing separate work:
//!
//! * **A receipt** says another machine has the bytes. On its own it is that machine's word, which
//!   is exactly what §13 exists to be independent of — so it never authorises anything by itself.
//! * **Under the floor** is what makes a deletion necessary rather than tidy. Above it, nothing is
//!   reclaimed however well collected it is, because there is no benefit to weigh against losing
//!   local evidence.
//! * **Not cited** is absolute. A window an incident names is the evidence for that incident, and
//!   the disk filling is not a reason to destroy it — a robot that reclaimed one would free
//!   megabytes and lose the only local copy of the thing somebody is about to ask about.
//!
//! # The measurement this waits on, and why it is a parameter
//!
//! [`plan`] takes `free_bytes` and `floor_bytes` rather than reading a disk, for §45's reason and
//! the kernel's: *the clock and the conditions are supplied*. That is what lets the policy be
//! decided in a test rather than on a machine whose disk happens to be full.
//!
//! It **was** also the honest shape, because nothing on this robot measured free disk: §33.6's
//! `storage_headroom` was absent for the same reason, in [`crate::checkin`]'s own words — *"needs a
//! `statvfs` nothing here calls"* — and `skynetd-io` carries `#![forbid(unsafe_code)]`, so the
//! syscall belonged in a crate of its own with the argument in its header.
//!
//! `skynet-statfs` is that crate, and [`free_of`] now reads a disk. What has not changed is the
//! shape: `plan` still takes the number rather than fetching it, and [`Free::Unmeasured`] is still
//! what a caller supplies on a platform the syscall does not support or a reading the crate refuses
//! to believe — in which case nothing is reclaimed, which is the correct direction for a rule whose
//! failure mode is destroying evidence.
//!
//! # The tombstone is not the audit log
//!
//! §38.3's log is *"small, append-only, and kept for the life of the machine"*, which is the right
//! shape, and it is the wrong file. Its subject is *who changed what*: every entry carries an actor
//! and a previous value, and a reclaim has neither — it is not a decision somebody made, it is a
//! consequence of a disk. Writing one there would put a digest into a `detail` string, where it is
//! prose rather than a field, which is the mistake §10's attestation spent a session undoing.
//!
//! So `reclaimed.jsonl` sits beside the windows it accounts for, in the same JSON Lines shape and
//! for the same reasons §38.3 gives: an append must not require rewriting the file, and a line
//! truncated by a power cut costs that line rather than the history.
//!
//! Its growth is bounded in the way that matters. A tombstone is under two hundred bytes and the
//! window it replaces is megabytes, so ten thousand reclaims cost under two megabytes — and a robot
//! that has reclaimed ten thousand windows is a robot whose accounting is the last thing on its
//! disk worth keeping.

use std::path::PathBuf;

use skynet_json::Json;

use crate::layout::Layout;

/// How much room there is, or that nobody has measured.
///
/// A three-state rather than a `u64` with a sentinel, for §7.2's reason applied to a number: a
/// robot that cannot measure its disk and a robot with no free bytes are different facts, and the
/// second is the one that would authorise a deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Free {
    /// Free bytes on the volume the black box is on.
    Bytes(u64),
    /// Nobody has measured it. Nothing is reclaimed.
    Unmeasured,
}

/// One sealed window, as the policy sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The file name under `<blackbox>/sealed/`.
    pub name: String,
    /// How many bytes it is.
    pub size_bytes: u64,
    /// The monotonic instant in its name (§13). Zero when the name does not carry one.
    pub sealed_at_ns: u64,
    /// Why it was sealed, from its name.
    pub reason: String,
    /// `sha256:…` over its bytes — **hashed, never believed off a manifest**. See [`scan`].
    pub digest: String,
    /// Whether a receipt whose digest is this window's digest sits beside it.
    pub receipted: bool,
    /// Whether an incident this robot raised named it.
    pub cited: bool,
}

/// Why a window was not reclaimed.
///
/// Reported per window rather than summarised, because the three send an operator to three places:
/// one is waiting on a Hub, one is waiting on a disk, and one is never going anywhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// An incident names it. Never reclaimed, at any disk pressure.
    Cited,
    /// No receipt, or a receipt for other bytes. The Hub does not have it.
    Uncollected,
    /// The floor was reached before this one was needed, or was never crossed.
    NotNeeded,
    /// Nothing measured the disk.
    Unmeasured,
}

impl Kept {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Kept::Cited => "cited",
            Kept::Uncollected => "uncollected",
            Kept::NotNeeded => "not_needed",
            Kept::Unmeasured => "unmeasured",
        }
    }
}

/// What a pass would do, before it does any of it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Windows to reclaim, oldest first.
    pub reclaim: Vec<Candidate>,
    /// Everything else, with the reason it stayed.
    pub kept: Vec<(String, Kept)>,
    /// How many bytes reclaiming all of `reclaim` would free.
    pub freed_bytes: u64,
    /// Whether that is enough to reach the floor.
    ///
    /// `false` with a non-empty `reclaim` is a robot that freed what it could and is still under —
    /// which is a fact an operator needs, because the next thing to look at is not the black box.
    pub reaches_floor: bool,
}

/// What a pass would do (§80).
///
/// **Pure.** No disk, no clock. Everything it decides on is supplied, which is what lets the three
/// conditions be tested against each other rather than against a machine.
///
/// The order of the tests inside the loop is the order of finality. The **floor** is first, and it
/// is not a test on a window at all — it is the condition on the pass, asked once per iteration
/// because the answer changes as bytes are freed. Then `cited`, because it is the one answer that
/// never changes and an operator reading *"uncollected"* against a window an incident names would
/// go and collect it to no purpose. Then the receipt.
pub fn plan(candidates: &[Candidate], free: Free, floor_bytes: u64) -> Plan {
    let Free::Bytes(free_bytes) = free else {
        return Plan {
            reclaim: Vec::new(),
            kept: candidates
                .iter()
                .map(|window| (window.name.clone(), Kept::Unmeasured))
                .collect(),
            freed_bytes: 0,
            reaches_floor: false,
        };
    };

    // There is deliberately no early return for a disk already above the floor. One was written
    // and removed: the loop's first test is `free + freed >= floor`, which on the first iteration
    // is exactly `free >= floor`, so the early exit produced an identical plan through a second
    // copy of the rule — and a mutation run proved it by breaking the copy without breaking a
    // single test. The floor is expressed once.
    let mut reclaim = Vec::new();
    let mut kept = Vec::new();
    let mut freed: u64 = 0;

    for window in candidates {
        if free_bytes.saturating_add(freed) >= floor_bytes {
            // Enough. Everything from here on is kept because it was not needed, not because it
            // failed a test — an operator who reclaims a window to reach a floor should not be told
            // the rest are collectable.
            kept.push((window.name.clone(), Kept::NotNeeded));
            continue;
        }
        if window.cited {
            kept.push((window.name.clone(), Kept::Cited));
            continue;
        }
        if !window.receipted {
            kept.push((window.name.clone(), Kept::Uncollected));
            continue;
        }
        freed = freed.saturating_add(window.size_bytes);
        reclaim.push(window.clone());
    }

    Plan {
        reclaim,
        kept,
        freed_bytes: freed,
        reaches_floor: free_bytes.saturating_add(freed) >= floor_bytes,
    }
}

// ── The tombstone ────────────────────────────────────────────────────────────

/// What a reclaim leaves behind (§80).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    /// `sha256:…` over the bytes that were freed.
    pub digest: String,
    /// The monotonic instant the window was sealed at (§13), from its name.
    pub sealed_at_ns: u64,
    /// How many bytes it held.
    pub bytes: u64,
    /// Why it was sealed. **Not** why it was reclaimed — there is only one reason for that, and it
    /// is the floor.
    pub reason: String,
}

impl Tombstone {
    /// The line it is written as.
    pub fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("bytes", Json::Num(self.bytes as f64))
            .set("digest", Json::Str(self.digest.clone()))
            .set("reason", Json::Str(self.reason.clone()))
            .set("sealed_at_ns", Json::Num(self.sealed_at_ns as f64));
        out
    }
}

/// Where the tombstones live.
pub fn tombstones_path(layout: &Layout) -> PathBuf {
    layout.blackbox().join("reclaimed.jsonl")
}

/// Append one, atomically enough: `O_APPEND` and one write, as §38.3's log does.
pub fn record(layout: &Layout, tombstone: &Tombstone) -> std::io::Result<()> {
    use std::io::Write;

    let line = tombstone
        .to_json()
        .to_canonical()
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    let path = tombstones_path(layout);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()
}

/// Every tombstone, oldest first. A line that does not parse is skipped, not fatal.
///
/// §38.3's degradation argument applies unchanged: a line truncated by a power cut costs that line
/// and the ten thousand before it still parse.
pub fn tombstones(layout: &Layout) -> Vec<Tombstone> {
    let Ok(text) = std::fs::read_to_string(tombstones_path(layout)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| {
            let document = Json::parse(line).ok()?;
            Some(Tombstone {
                digest: document.get("digest").and_then(Json::as_str)?.to_string(),
                sealed_at_ns: document
                    .get("sealed_at_ns")
                    .and_then(Json::as_f64)
                    .unwrap_or(0.0)
                    .max(0.0) as u64,
                bytes: document
                    .get("bytes")
                    .and_then(Json::as_f64)
                    .unwrap_or(0.0)
                    .max(0.0) as u64,
                reason: document
                    .get("reason")
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}

// ── The citation marker ──────────────────────────────────────────────────────

/// Mark a window as named by an incident (§80).
///
/// Written at the raise, by the process that put the window's name into the incident document.
///
/// # Why a marker rather than the seal reason
///
/// Every incident this daemon raises seals with [`Reason::Fault`](skynet_blackbox::seal::Reason),
/// so `reason == "fault"` answers the same question today — and it answers it by *inference*, which
/// is one new incident path away from being wrong. A future raise that sealed for
/// `Estop` or `Operator` would produce a window an incident names and the reason does not, and the
/// reclaim would free it. That failure is silent, irreversible, and lands on exactly the window
/// somebody is about to ask about, so the link is recorded rather than derived.
///
/// # Why it is not in the manifest
///
/// The manifest is what the *seal* computed, written by the producer at the moment it computed it,
/// and a consumer that edited one would be rewriting the producer's record. A citation is a
/// different fact by a different party at a later instant, so it is a different file.
pub fn cite(layout: &Layout, recording: &str, digest: &str, kind: &str) -> std::io::Result<()> {
    let mut document = Json::obj();
    document.set("digest", Json::Str(digest.to_string()));
    document.set("kind", Json::Str(kind.to_string()));
    let text = document
        .to_canonical()
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    crate::observe::write_atomic(&citation_path(layout, recording), text.as_bytes())
}

/// Where one window's citation lives.
pub fn citation_path(layout: &Layout, recording: &str) -> PathBuf {
    layout
        .blackbox()
        .join("sealed")
        .join(format!("{recording}.cited.json"))
}

/// Whether an incident named these exact bytes.
///
/// The digest is checked for [`crate::upload::Receipt::read`]'s reason, and it is the more
/// important half here: §13 names a window for a monotonic instant and a reboot restarts that
/// clock, so a citation left beside a reused name would protect the wrong window — and, worse,
/// would leave the window it was actually about looking reclaimable.
///
/// A citation that does **not** match is left on the disk rather than removed. It is a record of
/// something that happened, its window is gone, and a reclaim pass is not the party that should be
/// deciding to delete evidence of a citation.
pub fn cited(layout: &Layout, recording: &str, digest: &str) -> bool {
    let Some(document) = crate::observe::read_json(&citation_path(layout, recording)) else {
        return false;
    };
    document
        .get("digest")
        .and_then(Json::as_str)
        .is_some_and(|held| held.trim() == digest.trim())
}

// ── The disk ─────────────────────────────────────────────────────────────────

/// Read the sealed directory into candidates, oldest first.
///
/// **Every window is hashed.** [`crate::upload::sealed`] reads manifests because it is cheap enough
/// for a status route and nothing it feeds destroys anything. This decides deletions, and the
/// manifest's own doc comment says what it is: *"a cache in the sense that it can be rebuilt, and
/// it is not a second source of truth"*. A window deleted because a sidecar said another window's
/// digest had a receipt is the single worst outcome this module can produce, and it costs one pass
/// over a directory that is only read when the disk is already under the floor.
pub fn scan(layout: &Layout) -> Vec<Candidate> {
    let dir = layout.blackbox().join("sealed");
    let mut found: Vec<(String, u64)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".mcap") {
                return None;
            }
            Some((name, entry.metadata().ok()?.len()))
        })
        .collect();
    // §13's zero-padded monotonic instant makes a plain sort oldest first, which is the order §80
    // reclaims in.
    found.sort();

    found
        .into_iter()
        .filter_map(|(name, size)| {
            let (digest, _) = crate::bundle::hash_file(&dir.join(&name)).ok()?;
            let (sealed_at_ns, reason) = parse_name(&name);
            Some(Candidate {
                receipted: crate::upload::Receipt::read(layout, &name, &digest).is_some(),
                cited: cited(layout, &name, &digest),
                name,
                size_bytes: size,
                sealed_at_ns,
                reason,
                digest,
            })
        })
        .collect()
}

/// The instant and the reason out of a window's file name.
///
/// [`crate::blackbox::write_sealed`] writes `{mono_ns:020}-{reason}.mcap`. A name that does not
/// parse yields `(0, "")` rather than being skipped: a window whose name nobody recognises is still
/// a window, and dropping it from the scan would make it permanently invisible to a policy whose
/// job is to account for everything on the disk.
fn parse_name(name: &str) -> (u64, String) {
    let stem = name.strip_suffix(".mcap").unwrap_or(name);
    let Some((instant, reason)) = stem.split_once('-') else {
        return (0, String::new());
    };
    (instant.parse().unwrap_or(0), reason.to_string())
}

/// Reclaim one window: tombstone first, then the bytes.
///
/// **The tombstone is written before the delete**, and the ordering is the whole of the accounting
/// guarantee. A crash between the two leaves a tombstone for a window that still exists — visible,
/// recoverable, and wrong in the direction that loses nothing. The reverse leaves freed bytes
/// nothing accounts for, which is precisely the state §80 exists to make impossible.
///
/// The sidecars go with the bytes. A manifest and a receipt describe a file that is no longer
/// there, and §13's names repeat across a reboot: leaving them would put a stale digest hint and a
/// stale receipt beside a name a future window may take, and while both are guarded by their own
/// digest checks, a guard that is never exercised is not a guarantee anybody should be spending.
pub fn reclaim(layout: &Layout, window: &Candidate) -> std::io::Result<()> {
    record(
        layout,
        &Tombstone {
            digest: window.digest.clone(),
            sealed_at_ns: window.sealed_at_ns,
            bytes: window.size_bytes,
            reason: window.reason.clone(),
        },
    )?;

    let dir = layout.blackbox().join("sealed");
    std::fs::remove_file(dir.join(&window.name))?;
    // Best effort, and deliberately after the one that matters: a sidecar that will not delete is
    // not a reason to report a reclaim as failed when the bytes are already gone and accounted for.
    let _ = std::fs::remove_file(crate::blackbox::manifest_path(&dir, &window.name));
    let _ = std::fs::remove_file(layout.receipt(&window.name));
    Ok(())
}

/// Run a pass: scan, plan, reclaim what the plan names.
///
/// Returns the plan it acted on, so a caller can report what it did without recomputing it — and
/// so a caller that wants to *show* a plan without acting can call [`scan`] and [`plan`] instead.
pub fn run(layout: &Layout, free: Free, floor_bytes: u64) -> (Plan, Vec<std::io::Error>) {
    // Nothing is reclaimed without a measurement, so nothing is *hashed* without one either. The
    // scan reads every sealed byte on the disk (see its own header for why it must), and running
    // it every pass to reach a conclusion that is fixed would be the most expensive no-op on the
    // robot. This is the state the daemon is in today.
    if matches!(free, Free::Unmeasured) {
        return (plan(&[], free, floor_bytes), Vec::new());
    }
    let candidates = scan(layout);
    let decided = plan(&candidates, free, floor_bytes);
    let mut failures = Vec::new();
    for window in &decided.reclaim {
        if let Err(error) = reclaim(layout, window) {
            failures.push(error);
        }
    }
    (decided, failures)
}

/// The directory a caller must measure the free space of, when something can (§33.6).
///
/// Named here rather than left for a caller to reconstruct, because the volume that matters is the
/// one the *windows* are on and not the one the daemon was started from.
pub fn volume(layout: &Layout) -> PathBuf {
    layout.blackbox()
}

/// What §80's retention would do, for a status route (§39.1).
///
/// **It does not scan.** [`scan`] hashes every sealed window, and a console polling this route
/// would put gigabytes of reading on a loop. What a reader needs from a status route is the
/// *policy's* state — whether a pass would run at all, and what has already been let go — and
/// both are cheap.
///
/// `free` is reported as `null` when nobody measured it, beside a sentence saying so. That is the
/// §33.6 treatment applied one level up: a route that omitted the field would leave an operator
/// unable to tell a robot with plenty of room from one whose disk nothing can see.
pub fn report(layout: &Layout, free: Free, floor_bytes: u64) -> Json {
    let stones = tombstones(layout);
    let mut out = Json::obj();
    out.set(
        "free_bytes",
        match free {
            Free::Bytes(bytes) => Json::Num(bytes as f64),
            Free::Unmeasured => Json::Null,
        },
    );
    out.set("floor_bytes", Json::Num(floor_bytes as f64));
    out.set(
        "would_reclaim",
        Json::Bool(matches!(free, Free::Bytes(bytes) if bytes < floor_bytes)),
    );
    out.set("reclaimed", Json::Num(stones.len() as f64));
    out.set(
        "reclaimed_bytes",
        Json::Num(stones.iter().map(|stone| stone.bytes).sum::<u64>() as f64),
    );
    if matches!(free, Free::Unmeasured) {
        out.set(
            "detail",
            Json::Str(
                "This machine's volume could not be measured — §33.6's `storage_headroom` is \
                 absent for the same reason, and `skynet-statfs` needs 64-bit Linux and \
                 refuses a reading that cannot describe a filesystem — so §80's reclaim \
                 does not run and every sealed window is kept. That is the safe direction \
                 and it is not a state to stay in: a robot that seals daily will \
                 eventually fill this volume, and the operator is the party who finds out."
                    .to_string(),
            ),
        );
    }
    out
}

/// The floor this robot reclaims below, and where it comes from.
///
/// **Not configurable yet, and the number is not the interesting part.** §80 says *"under the
/// floor"* and §28 says why there is one — *a black box that stops recording because a log filled
/// the disk is a black box that fails exactly when something is going wrong* — and neither names a
/// figure. This is a placeholder with the reasoning attached: one gigabyte is enough for a
/// preallocated ring plus an OTA slot on the smallest machine §28's table admits, which is the
/// state that must not be reachable.
///
/// It sits here rather than in §38's configuration because a floor an operator can lower to zero
/// is a floor that authorises deleting everything, and that is a decision for the owner rather than
/// for a profile.
pub const DEFAULT_FLOOR_BYTES: u64 = 1_024 * 1_024 * 1_024;

/// What this robot can say about its own free disk.
///
/// The volume the **black box** is on rather than the root filesystem, because that is the disk
/// §28 reserves and the one a reclaim would free. On a robot where they are the same partition the
/// answer is the same; on one where they are not, this is the question that was being asked.
///
/// An unmeasurable disk is [`Free::Unmeasured`] and never a zero. The distinction is the whole
/// reason this returns an enum: a robot that read *no bytes free* off a failed syscall would run
/// §80's rule at full force against every window it holds.
pub fn free_of(layout: &Layout) -> Free {
    match skynet_statfs::space_of(&layout.blackbox()) {
        Ok(space) => Free::Bytes(space.available_bytes),
        Err(_) => Free::Unmeasured,
    }
}

/// §33.6's `storage_headroom`: the fraction of that volume still free, 0–1.
///
/// `None` for an unmeasurable disk, which is what `Health` already means by an absent field —
/// *nothing on this structure is inferred, and nothing defaults to a flattering value*. A robot
/// that could not measure its disk reports nothing rather than reporting full.
pub fn headroom_of(layout: &Layout) -> Option<f64> {
    skynet_statfs::space_of(&layout.blackbox())
        .ok()
        .map(|space| space.free_fraction())
}

//! §12 on a real machine: the documents, the updater, and the boot that confirms itself.
//!
//! [`skynet_contracts::ota`] decides §12's two decidable rules and takes no clock, no key and no
//! filesystem. This is the other half — reading the release manifest a check-in cached, verifying
//! its signature and the bench pass beside it against the trust store, driving RAUC, and holding
//! the one piece of state that has to survive the reboot an update causes.
//!
//! The split is the same one [`crate::entitlement`] and [`crate::embodiment`] use, and for the same
//! reason: the decision has to run identically on the Hub and on a robot that has been offline for
//! a week, and a decision function that opened files could not.
//!
//! # Both documents are signed, and the second one matters more than it looks
//!
//! A **release manifest** says what a bundle changes. A **bench pass** says that somebody ran the
//! conformance suite against those exact bytes and it passed. §12's third rule is the conjunction
//! of the two — *"an update that changes the Safety Kernel or the envelope is refused without a
//! fresh bench pass"* — so both are inputs to a gate the document calls the single most important
//! test in the suite.
//!
//! An unsigned bench pass on a robot's disk is therefore §38.1's attacker exactly: *"somebody
//! editing limits on a robot's disk"*, one file, no network, and the safety kernel replaced. So a
//! bench pass whose signature does not verify against the trust store is **not a bench pass** — it
//! reaches the contract as `None`, and the gate refuses the update for want of evidence rather than
//! accepting evidence nobody signed.
//!
//! # The flattening is lossy, and the documents say who they are
//!
//! [`Layout::release`] and [`Layout::bench_pass`] flatten a version and a digest into one path
//! component, so `1.4.0` and `1_4_0` name one file. Without a check that would be a way to make one
//! release's manifest answer for another's, and a desired-state document is not a trusted input. So
//! each document declares its own identity and a mismatch is refused — the same rule
//! [`crate::entitlement`] applies to a token's `package` field, and it is what makes the flattening
//! safe rather than clever.
//!
//! # What is not built
//!
//! Nothing **fetches** a bundle. §34's outbound half is written and its inbound counterpart is not,
//! so a robot cannot go and get an image it has been told about. [`stage`] installs a bundle that is
//! already on the disk and reports the honest refusal when it is not, rather than reporting success
//! for a file nobody delivered.

use std::path::{Path, PathBuf};

use skynet_contracts::admission::SignatureState;
use skynet_contracts::ota::{
    admit_bundle, bundle_verdict_to_json, confirm_boot, BenchPass, BootDecision, BootEvidence,
    BootVerdict, BundleManifest, BundleVerdict, UpdateSubject, DEFAULT_BOOT_BUDGET_MS,
};
use skynet_json::Json;

use crate::layout::Layout;
use crate::trust::{bench_message, read_detached, release_message, SignedBy, TrustStore, Verdict};

/// Why an update could not be driven.
#[derive(Debug, Clone)]
pub struct UpdateError {
    /// What was being attempted.
    pub doing: String,
    /// What went wrong, in a sentence an operator can act on.
    pub detail: String,
}

impl std::fmt::Display for UpdateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.doing, self.detail)
    }
}

impl std::error::Error for UpdateError {}

/// Whatever can install an image and tell a bootloader what it thinks of a slot (§12).
///
/// A trait for the reason [`crate::launch::Launcher`] is one: the suite has to be able to drive
/// every path of §12's second rule, including the rollback, and a rollback that only runs on real
/// hardware is a rollback nobody tests until a fleet needs it.
pub trait Updater {
    /// Install a bundle into the inactive slot. Does not reboot.
    fn install(&mut self, bundle: &Path) -> Result<(), UpdateError>;

    /// Tell the bootloader this slot is good, so it stops counting.
    fn mark_good(&mut self) -> Result<(), UpdateError>;

    /// Tell the bootloader this slot is bad, so the next boot returns to the other one.
    fn mark_bad(&mut self) -> Result<(), UpdateError>;
}

/// RAUC, the A/B update framework the images are built with (§12, §23).
#[derive(Debug, Clone, Default)]
pub struct RaucUpdater;

impl RaucUpdater {
    fn run(&self, doing: &str, args: &[&str]) -> Result<(), UpdateError> {
        let output = std::process::Command::new("rauc")
            .args(args)
            .output()
            .map_err(|error| UpdateError {
                doing: doing.to_string(),
                detail: format!(
                    "`rauc` could not be run: {error}. §12's A/B slots are RAUC's; a robot without \
                     it can be updated by a person with an SD card and by nothing else."
                ),
            })?;
        if output.status.success() {
            return Ok(());
        }
        // RAUC's own message, not a summary of it. The failure modes here are somebody else's
        // vocabulary — a slot that will not mount, a bundle signed by a key the bundle store does
        // not have — and paraphrasing them is how a support call loses the one string that names
        // the cause.
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(UpdateError {
            doing: doing.to_string(),
            detail: if stderr.is_empty() {
                format!("`rauc {}` exited {}", args.join(" "), output.status)
            } else {
                stderr
            },
        })
    }
}

impl Updater for RaucUpdater {
    fn install(&mut self, bundle: &Path) -> Result<(), UpdateError> {
        self.run(
            "installing a bundle",
            &["install", &bundle.to_string_lossy()],
        )
    }

    fn mark_good(&mut self) -> Result<(), UpdateError> {
        self.run("marking the booted slot good", &["status", "mark-good"])
    }

    fn mark_bad(&mut self) -> Result<(), UpdateError> {
        self.run("marking the booted slot bad", &["status", "mark-bad"])
    }
}

/// An updater that records rather than acts — for the suite, and for a machine with no RAUC.
#[derive(Debug, Clone, Default)]
pub struct RecordingUpdater {
    /// Every call, in order.
    pub calls: Vec<String>,
    /// When set, every call fails with this detail.
    pub refuse: Option<String>,
}

impl Updater for RecordingUpdater {
    fn install(&mut self, bundle: &Path) -> Result<(), UpdateError> {
        self.calls.push(format!("install {}", bundle.display()));
        self.result("installing a bundle")
    }

    fn mark_good(&mut self) -> Result<(), UpdateError> {
        self.calls.push("mark-good".to_string());
        self.result("marking the booted slot good")
    }

    fn mark_bad(&mut self) -> Result<(), UpdateError> {
        self.calls.push("mark-bad".to_string());
        self.result("marking the booted slot bad")
    }
}

impl RecordingUpdater {
    fn result(&self, doing: &str) -> Result<(), UpdateError> {
        match &self.refuse {
            None => Ok(()),
            Some(detail) => Err(UpdateError {
                doing: doing.to_string(),
                detail: detail.clone(),
            }),
        }
    }
}

/// A release manifest as this robot holds it, with what is known about its signature.
#[derive(Debug, Clone)]
pub struct Release {
    /// The manifest the contract reads.
    pub manifest: BundleManifest,
    /// sha256 over the document's canonical form.
    pub digest: String,
    /// What the trust store concluded.
    pub signature: Verdict,
    /// Where the bundle would be installed from, when the manifest names a file.
    pub bundle_path: Option<PathBuf>,
}

/// Read the cached release manifest for one version, and judge its signature.
///
/// Three outcomes, and the middle one is why this is not `Option`: a missing manifest is a release
/// nobody has told this robot about, and an unreadable one is a release somebody delivered badly.
/// The first is ordinary — most robots are told about one release at a time — and the second is a
/// fact worth an operator's attention.
pub fn read_release(
    layout: &Layout,
    trust: &TrustStore,
    version: &str,
) -> Result<Option<Release>, String> {
    let path = layout.release(version);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };
    let document = Json::parse(&text)
        .map_err(|error| format!("{} does not parse: {error}", path.display()))?;

    let manifest = skynet_contracts::json::bundle_from_json(&document);

    // The flattening is lossy, so the document says who it is. Without this, `1.4.0` and `1_4_0`
    // name one file and a desired state could make one release's manifest answer for another's.
    if manifest.version != version {
        return Err(format!(
            "{} is filed as `{version}` and declares itself `{}`. One of the two is wrong, and \
             until somebody says which, this robot would be judging an update against another \
             release's manifest",
            path.display(),
            manifest.version,
        ));
    }

    let digest = document_digest(&document)?;
    let signed = read_detached(&layout.release_signature(version))
        .map_err(|error| format!("{}: {error}", path.display()))?;
    let signature = trust.check(&release_message(&digest), signed.as_ref());

    // Two ways bytes get here and one of them is new. A manifest may *name* a file, which is how an
    // image was put on a robot before there was a fetcher and how an offline install still works;
    // and the fetcher writes one named by the digest. The named file wins when it exists, because
    // it is somebody's explicit instruction — and neither is trusted on sight: both are hashed
    // against `manifest.digest` before RAUC is handed anything (§12).
    let named = document
        .get_present("bundle")
        .and_then(Json::as_str)
        .filter(|name| !name.is_empty())
        .map(|name| layout.os().join("bundles").join(name));
    let fetched = layout.bundle(&manifest.digest);
    let bundle_path = match named {
        Some(path) if path.is_file() => Some(path),
        Some(path) if !fetched.is_file() => Some(path),
        _ if fetched.is_file() => Some(fetched),
        other => other,
    };

    Ok(Some(Release {
        manifest,
        digest,
        signature,
        bundle_path,
    }))
}

/// Read the bench pass for an image digest, if this robot holds a signed one.
///
/// **An unsigned or untrusted pass is not a pass.** It returns `Ok(None)` with the reason, so the
/// contract refuses the update for want of evidence rather than accepting evidence nobody signed —
/// see the module header. That is the one place in this file where the safe direction and the
/// convenient direction disagree, and the sentence it returns is what keeps the refusal legible: an
/// operator who has a bench pass on the disk and is being told there is none needs to know it is
/// the signature and not the file.
pub fn read_bench_pass(
    layout: &Layout,
    trust: &TrustStore,
    image_digest: &str,
) -> (Option<BenchPass>, Option<String>) {
    if image_digest.is_empty() {
        return (None, None);
    }
    let path = layout.bench_pass(image_digest);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return (None, None),
        Err(error) => {
            return (
                None,
                Some(format!("{} could not be read: {error}", path.display())),
            )
        }
    };
    let Ok(document) = Json::parse(&text) else {
        return (None, Some(format!("{} does not parse", path.display())));
    };

    let declared = document
        .get("image_digest")
        .and_then(Json::as_str)
        .unwrap_or_default();
    if declared != image_digest {
        return (
            None,
            Some(format!(
                "{} is filed under `{image_digest}` and declares itself `{declared}`. A bench pass \
                 is bound to the bytes it was taken against, and this one names other bytes",
                path.display(),
            )),
        );
    }

    let digest = match document_digest(&document) {
        Ok(digest) => digest,
        Err(error) => return (None, Some(error)),
    };
    let signed = match read_detached(&layout.bench_pass_signature(image_digest)) {
        Ok(signed) => signed,
        Err(error) => return (None, Some(format!("{}: {error}", path.display()))),
    };
    let verdict = trust.check(&bench_message(&digest), signed.as_ref());
    if verdict.state != SignatureState::Verified {
        return (
            None,
            Some(format!(
                "the bench pass for `{image_digest}` is `{}` — {}. §12's gate is the reason this is \
                 refused rather than believed: a document that decides whether the Safety Kernel \
                 may be replaced, sitting unsigned on the robot's own disk, is the attacker §38.1 \
                 names",
                verdict.state.as_str(),
                verdict.detail,
            )),
        );
    }

    (
        Some(BenchPass {
            image_digest: declared.to_string(),
            tier: document
                .get("tier")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            passed: document.get("passed") == Some(&Json::Bool(true)),
        }),
        None,
    )
}

fn document_digest(document: &Json) -> Result<String, String> {
    let canonical = document
        .to_canonical()
        .map_err(|error| format!("could not be canonicalised: {error:?}"))?;
    Ok(String::from_utf8_lossy(&skynet_sha256::hex_bytes(canonical.as_bytes())).into_owned())
}

/// Everything §12 concludes about one version, ready for an API answer.
#[derive(Debug, Clone)]
pub struct Assessment {
    /// The version asked about.
    pub version: String,
    /// The verdict, when a manifest was found.
    pub verdict: Option<BundleVerdict>,
    /// Why there is no verdict, when there is not.
    pub because: Option<String>,
    /// What is known about the manifest's own signature.
    pub signature: Option<Verdict>,
    /// Why a bench pass on the disk was not used, when one was not.
    pub bench_note: Option<String>,
    /// Where the bundle would be installed from, when the manifest names a file.
    pub bundle_path: Option<PathBuf>,
    /// `sha256:…` over the bundle, from the manifest.
    ///
    /// Carried out of the assessment rather than re-read, because it is the name of the thing every
    /// gate above was applied to — the signature, the bench pass, the compatibility list. The fetch
    /// and the check on what arrived use *this* digest, so the bytes that get installed are the
    /// bytes the gate ran against and not merely bytes for the same version number.
    pub digest: Option<String>,
}

impl Assessment {
    /// Whether this version may be staged and applied.
    pub fn admitted(&self) -> bool {
        self.verdict.as_ref().is_some_and(|v| v.admitted)
    }

    /// The API shape.
    pub fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("version", Json::Str(self.version.clone()));
        out.set("admitted", Json::Bool(self.admitted()));
        match &self.verdict {
            Some(verdict) => {
                out.set("verdict", bundle_verdict_to_json(verdict));
            }
            None => {
                out.set("verdict", Json::Null);
            }
        }
        out.set(
            "because",
            match &self.because {
                Some(reason) => Json::Str(reason.clone()),
                None => Json::Null,
            },
        );
        out.set(
            "signature",
            match &self.signature {
                Some(verdict) => Json::Str(verdict.state.as_str().to_string()),
                None => Json::Null,
            },
        );
        out.set(
            "signer",
            match self.signature.as_ref().and_then(|v| v.signer.clone()) {
                Some(signer) => Json::Str(signer),
                None => Json::Null,
            },
        );
        out.set(
            "bench_note",
            match &self.bench_note {
                Some(note) => Json::Str(note.clone()),
                None => Json::Null,
            },
        );
        out.set(
            "digest",
            match &self.digest {
                Some(digest) => Json::Str(digest.clone()),
                None => Json::Null,
            },
        );
        out
    }
}

/// §12's third rule, run against the documents this robot actually holds.
///
/// `board` and `channel` come from the caller rather than being read here, because both are facts
/// about the robot and its desired state rather than about the release.
pub fn assess(
    layout: &Layout,
    trust: &TrustStore,
    version: &str,
    running_version: &str,
    board: Option<String>,
    channel: &str,
) -> Assessment {
    let mut assessment = Assessment {
        version: version.to_string(),
        verdict: None,
        because: None,
        signature: None,
        bench_note: None,
        bundle_path: None,
        digest: None,
    };

    let release = match read_release(layout, trust, version) {
        Err(error) => {
            assessment.because = Some(error);
            return assessment;
        }
        Ok(None) => {
            assessment.because = Some(format!(
                "nothing on this robot describes release `{version}`. §57 delivers a release \
                 manifest with the desired state, and §12 will not install an image whose \
                 manifest nobody has seen — because the manifest is what says whether the update \
                 touches the Safety Kernel."
            ));
            return assessment;
        }
        Ok(Some(release)) => release,
    };

    let (bench, bench_note) = read_bench_pass(layout, trust, &release.manifest.digest);
    let subject = UpdateSubject {
        running_version: running_version.to_string(),
        board,
        channel: channel.to_string(),
        signature: release.signature.state,
        bench,
    };

    assessment.verdict = Some(admit_bundle(&release.manifest, &subject));
    assessment.signature = Some(release.signature);
    assessment.bench_note = bench_note;
    assessment.bundle_path = release.bundle_path;
    assessment.digest = Some(release.manifest.digest.clone());
    assessment
}

// ── §12's second rule, on disk ───────────────────────────────────────────────

/// The slot this robot applied and has not yet confirmed.
#[derive(Debug, Clone, PartialEq)]
pub struct Pending {
    /// The version that was applied.
    pub version: String,
    /// The budget the operator set, or [`DEFAULT_BOOT_BUDGET_MS`].
    pub budget_ms: f64,
}

/// Record that an image was applied and its next boot is the one that has to prove itself.
pub fn mark_pending(layout: &Layout, version: &str, budget_ms: f64) -> std::io::Result<()> {
    let mut document = Json::obj();
    document.set("version", Json::Str(version.to_string()));
    document.set("budget_ms", Json::Num(budget_ms));
    let text = document
        .to_canonical()
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    std::fs::create_dir_all(layout.os())?;
    crate::observe::write_atomic(&layout.pending_boot(), text.as_bytes())
}

/// Read it back, or nothing.
pub fn read_pending(layout: &Layout) -> Option<Pending> {
    let document = crate::observe::read_json(&layout.pending_boot())?;
    Some(Pending {
        version: document
            .get("version")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string(),
        budget_ms: document
            .get_present("budget_ms")
            .and_then(Json::as_f64)
            .filter(|budget| budget.is_finite())
            .unwrap_or(DEFAULT_BOOT_BUDGET_MS),
    })
}

/// Forget it, on a confirm or a rollback.
pub fn clear_pending(layout: &Layout) -> std::io::Result<()> {
    match std::fs::remove_file(layout.pending_boot()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Milliseconds since this machine booted, when it can be read.
///
/// `/proc/uptime` rather than the daemon's own uptime, and the difference matters here: §12's budget
/// is a **boot** budget, and a daemon that restarted twice while a driver crash-looped would
/// otherwise reset the clock on a rollback that was supposed to be unattended.
///
/// `None` where there is no `/proc`, and the caller falls back to its own uptime with that fact
/// recorded rather than silently substituted.
pub fn boot_elapsed_ms(root: &Path) -> Option<f64> {
    let text = std::fs::read_to_string(root.join("proc/uptime")).ok()?;
    let seconds: f64 = text.split_whitespace().next()?.parse().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some(seconds * 1000.0)
}

/// What the boot confirmation concluded, and whether the machine was told.
#[derive(Debug, Clone)]
pub struct Confirmation {
    /// The decision.
    pub verdict: BootVerdict,
    /// The version being proved, when one is pending.
    pub version: Option<String>,
    /// Whether the elapsed time came from `/proc/uptime` or from the daemon's own start.
    pub clock: &'static str,
    /// What went wrong telling the bootloader, when something did.
    pub failed: Option<String>,
}

impl Confirmation {
    /// The API shape.
    pub fn to_json(&self) -> Json {
        let mut out = skynet_contracts::ota::boot_verdict_to_json(&self.verdict);
        out.set(
            "version",
            match &self.version {
                Some(version) => Json::Str(version.clone()),
                None => Json::Null,
            },
        );
        out.set("clock", Json::Str(self.clock.to_string()));
        out.set(
            "failed",
            match &self.failed {
                Some(detail) => Json::Str(detail.clone()),
                None => Json::Null,
            },
        );
        out
    }
}

/// The three health inputs §12's second rule reads, grouped.
///
/// A struct rather than three parameters, and it earns its place beyond satisfying a lint: every
/// one of the three is an `Option<bool>` and three adjacent ones at a call site are three chances
/// to pass the self-test result as the package health. Named fields make that mistake impossible to
/// write rather than merely unlikely.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BootHealth {
    /// Whether §48.2's self-test concluded the robot may run.
    pub post_passed: Option<bool>,
    /// Whether every package the desired state wants running is running.
    pub packages_healthy: Option<bool>,
    /// Whether the reconciler converged.
    pub converged: Option<bool>,
}

/// §12's second rule, decided and then acted on.
///
/// The decision is [`confirm_boot`]'s and this function only carries it out: mark the slot, clear
/// the marker, and report. Splitting them is what makes the rollback path testable without a
/// bootloader, which §12 asks for by name — `ota.rollback.works` stages a deliberately broken image
/// and requires unattended recovery, and a rollback nobody can exercise until a fleet needs it is a
/// rollback nobody has exercised.
pub fn confirm(
    layout: &Layout,
    updater: &mut dyn Updater,
    running_version: &str,
    elapsed_ms: f64,
    clock: &'static str,
    health: &BootHealth,
) -> Confirmation {
    let pending = read_pending(layout);

    // A marker for a version this robot is *not* running is the rollback that already happened:
    // the bootloader returned to the previous slot and the marker outlived it. Clearing it is the
    // honest end of that story, and leaving it would make the next ordinary boot look pending.
    if let Some(entry) = &pending {
        if !entry.version.is_empty() && entry.version != running_version {
            let _ = clear_pending(layout);
            return Confirmation {
                verdict: confirm_boot(&BootEvidence::default()),
                version: Some(entry.version.clone()),
                clock,
                failed: None,
            };
        }
    }

    let evidence = BootEvidence {
        pending: pending.is_some(),
        elapsed_ms,
        budget_ms: pending
            .as_ref()
            .map(|p| p.budget_ms)
            .unwrap_or(DEFAULT_BOOT_BUDGET_MS),
        post_passed: health.post_passed,
        packages_healthy: health.packages_healthy,
        converged: health.converged,
    };
    let verdict = confirm_boot(&evidence);

    let failed = match verdict.decision {
        BootDecision::Confirm => match updater.mark_good() {
            Ok(()) => {
                let _ = clear_pending(layout);
                None
            }
            Err(error) => Some(error.to_string()),
        },
        BootDecision::RollBack => match updater.mark_bad() {
            Ok(()) => {
                // The marker is *kept*. The rollback happens on the next boot, and a robot that
                // forgot it had been marked bad would report a healthy pending-free state while
                // still running the image it just condemned.
                None
            }
            Err(error) => Some(error.to_string()),
        },
        BootDecision::Wait | BootDecision::NotPending => None,
    };

    Confirmation {
        verdict,
        version: pending.map(|entry| entry.version),
        clock,
        failed,
    }
}

/// A detached signature, read.
///
/// Re-exported through this module so a caller staging an update does not have to reach into
/// [`crate::trust`] for one piece of a flow that otherwise lives here.
pub fn signature_of(path: &Path) -> Result<Option<SignedBy>, String> {
    read_detached(path)
}

//! §12's three rules, as two decisions.
//!
//! The Rust edition of `reference/typescript/ota.ts`. Read that file for the design; this one
//! records the port.
//!
//! §12 gives OTA three rules that are *"not standard practice"*, and the third is the one the
//! document calls the single most important test in the whole suite:
//!
//! 1. **Never apply mid-task.** An update issued while the robot is holding something is staged and
//!    applied at a declared safe state. `ota.safe_state.respected` measures it.
//! 2. **Health-gated rollback.** The new slot must pass a health check within a boot budget or the
//!    bootloader returns to the previous slot unattended. `ota.rollback.works` measures it.
//! 3. **OTA is not a route around certification.** An update that changes the Safety Kernel or the
//!    envelope is refused without a fresh bench pass. *"Without this rule, every guarantee in §7
//!    and §15 is one silent update away from being void."*
//!
//! **Rule 1 is not here**, and that is worth saying rather than leaving as an absence: it is
//! already enforced, structurally, by `skynetd::reconcile::plan` — staging is `Gate::Open` and
//! applying is `Gate::SafeState`, so an apply cannot be emitted for a robot that is not at one. A
//! second implementation of it here would be a second answer to a question already answered, which
//! is §31.2's failure in miniature.
//!
//! Rules 2 and 3 are decisions, and decisions are functions. [`admit_bundle`] answers rule 3 and
//! [`confirm_boot`] answers rule 2.
//!
//! # Why rule 3 fails closed on a word nobody recognises
//!
//! A bundle declares which components it changes. Two of them — `safety_kernel` and `envelope` —
//! require a bench pass for **these exact bytes** before the update may be applied.
//!
//! An unrecognised component name is treated as though it were one of those two. That is the whole
//! anti-gaming argument in one rule: a publisher who wanted to ship a kernel change past the gate
//! would not write `safety_kernel`, they would write `core` or `rt` or `platform` — and a matcher
//! that only knew two words would wave it through. The failure mode of the strict reading is an
//! update that waits for somebody to fix a typo. The failure mode of the lenient reading is §7's
//! guarantees quietly voided, which is the sentence §12 spends its length on.
//!
//! # And why a bench pass is bound to a digest
//!
//! The same reason §56.3's seeds come from the benchmark: evidence that is not bound to what it is
//! evidence *of* is not evidence. A bench pass for version 1.4.0 says nothing about 1.4.1, and a
//! gate that accepted "this robot has passed a bench test at some point" would be satisfied by
//! every robot that had ever been certified — which is every robot the gate exists to protect.
//!
//! # Pure
//!
//! No clock, no key, no filesystem. The signature arrives decided, as everywhere else in this
//! crate, and the boot budget arrives as elapsed milliseconds rather than as a timestamp (§35.2).

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

use crate::admission::{Severity, SignatureState};

/// The components an OS bundle may declare that it changes.
///
/// Deliberately the stack §3 draws and nothing more. A vocabulary invented wider than the document
/// would be this module deciding what an operating system is made of, and the only two entries that
/// carry a rule are the two §12 names.
///
/// Reproduced into `corpus/vocabulary/closed-sets.json`.
pub const OS_COMPONENTS: [&str; 9] = [
    "bootloader",
    "kernel_image",
    "rhal_abi",
    "rhal_driver",
    "safety_kernel",
    "envelope",
    "skynetd",
    "policy_runtime",
    "userspace",
];

/// The components §12's third rule gates, by name.
///
/// **Not the whole rule.** [`gated`] is, because an unrecognised name is gated too.
pub const CERTIFICATION_GATED: [&str; 2] = ["safety_kernel", "envelope"];

/// Whether changing this component requires a fresh bench pass.
///
/// True for §12's two names, and true for anything this edition does not recognise. See the module
/// header: a component nobody can classify is one nobody has established is safe to change, and the
/// two readings fail in opposite directions.
pub fn gated(component: &str) -> bool {
    if CERTIFICATION_GATED.contains(&component) {
        return true;
    }
    !OS_COMPONENTS.contains(&component)
}

/// What an OS release says about itself (§57's `GET /os/releases`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BundleManifest {
    /// The version this bundle installs.
    pub version: String,
    /// `stable`, `beta`, whatever the fleet uses. Compared against the desired state's channel.
    pub channel: String,
    /// `sha256:…` over the bundle. What a bench pass is bound to.
    pub digest: String,
    /// How many bytes the bundle is, when the publisher declares it.
    ///
    /// **Absent, not zero, on a manifest that says nothing** — and the distinction is load-bearing
    /// twice over. [`crate::bundle::plan_fetch`] refuses to resume from a partial *larger than the
    /// whole*, which is how a leftover from another image is caught before a transfer rather than
    /// after it; with no declared size there is nothing to compare and the digest catches it at the
    /// end instead. And a progress line is a fraction only when the denominator came from the
    /// signed document — a size the *server* claimed describes whatever the server is serving.
    ///
    /// A `0` here would be a publisher declaring an empty image, which is a different statement
    /// from declining to say.
    pub size: Option<u64>,
    /// Machine identifiers this image is built for. Empty means the bundle declares nothing, which
    /// is a warning rather than a match — see [`admit_bundle`].
    pub compatible: Vec<String>,
    /// Which of [`OS_COMPONENTS`] differ from the running image.
    pub changes: Vec<String>,
}

/// A bench pass, and what it is a pass *of*.
#[derive(Debug, Clone, PartialEq)]
pub struct BenchPass {
    /// The bundle digest this pass was taken against.
    pub image_digest: String,
    /// The tier it established, from [`crate::certification`]'s vocabulary.
    pub tier: String,
    /// Whether every required test passed. A recorded run that failed is still a record.
    pub passed: bool,
}

/// The robot this bundle would be applied to.
///
/// **No `Default`, deliberately.** §37.1 has five signature states and none of them is a sensible
/// zero: `absent` would be a caller who forgot the field claiming nobody signed the image, and
/// `verified` would be worse. A caller has to say.
#[derive(Debug, Clone, PartialEq)]
pub struct UpdateSubject {
    /// The version running now.
    pub running_version: String,
    /// What this machine is, when it knows. `None` omits the compatibility check rather than
    /// failing it: a robot that cannot say what board it is has not established a mismatch.
    pub board: Option<String>,
    /// The channel the desired state asked for.
    pub channel: String,
    /// §37.1's five outcomes, decided elsewhere.
    pub signature: SignatureState,
    /// The bench pass this robot holds, when it holds one.
    pub bench: Option<BenchPass>,
}

/// Why a bundle was refused, or worth saying anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleFaultCode {
    /// The bundle names no version.
    Unversioned,
    /// The bundle names no digest, so nothing can be bound to it.
    Undigested,
    /// A signature was present and did not verify (§37.1). Tampering looks like this.
    SignatureFailed,
    /// It verified against a key this robot has not been told to believe.
    SignatureUntrusted,
    /// Nobody signed it.
    SignatureAbsent,
    /// This platform cannot check a signature at all.
    SignatureUnverifiable,
    /// The bundle is for another machine.
    Incompatible,
    /// The bundle declares no compatible machines.
    CompatibilityUndeclared,
    /// A different channel from the one the desired state asked for.
    ChannelMismatch,
    /// §12's third rule: a gated component with no bench pass for these bytes.
    CertificationRequired,
    /// A bench pass exists and is for other bytes.
    BenchPassStale,
    /// A bench pass exists for these bytes and records a failure.
    BenchPassFailed,
    /// The bundle installs the version already running.
    AlreadyRunning,
}

impl BundleFaultCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            BundleFaultCode::Unversioned => "unversioned",
            BundleFaultCode::Undigested => "undigested",
            BundleFaultCode::SignatureFailed => "signature_failed",
            BundleFaultCode::SignatureUntrusted => "signature_untrusted",
            BundleFaultCode::SignatureAbsent => "signature_absent",
            BundleFaultCode::SignatureUnverifiable => "signature_unverifiable",
            BundleFaultCode::Incompatible => "incompatible",
            BundleFaultCode::CompatibilityUndeclared => "compatibility_undeclared",
            BundleFaultCode::ChannelMismatch => "channel_mismatch",
            BundleFaultCode::CertificationRequired => "certification_required",
            BundleFaultCode::BenchPassStale => "bench_pass_stale",
            BundleFaultCode::BenchPassFailed => "bench_pass_failed",
            BundleFaultCode::AlreadyRunning => "already_running",
        }
    }
}

/// Every fault code, in the order the checks run.
pub const BUNDLE_FAULTS: [BundleFaultCode; 13] = [
    BundleFaultCode::Unversioned,
    BundleFaultCode::Undigested,
    BundleFaultCode::SignatureFailed,
    BundleFaultCode::SignatureUntrusted,
    BundleFaultCode::SignatureAbsent,
    BundleFaultCode::SignatureUnverifiable,
    BundleFaultCode::Incompatible,
    BundleFaultCode::CompatibilityUndeclared,
    BundleFaultCode::ChannelMismatch,
    BundleFaultCode::CertificationRequired,
    BundleFaultCode::BenchPassStale,
    BundleFaultCode::BenchPassFailed,
    BundleFaultCode::AlreadyRunning,
];

/// One thing found about a bundle.
#[derive(Debug, Clone, PartialEq)]
pub struct BundleFault {
    /// What.
    pub code: BundleFaultCode,
    /// Whether it stops the update.
    pub severity: Severity,
    /// A sentence naming the fix (§64).
    pub message: String,
    /// The components, machines or digests it is about.
    pub subjects: Vec<String>,
}

/// §12's answer for one bundle.
#[derive(Debug, Clone, PartialEq)]
pub struct BundleVerdict {
    /// Whether this bundle may be staged and applied.
    pub admitted: bool,
    /// Everything found, in the order the checks run.
    pub faults: Vec<BundleFault>,
    /// The declared components that require a bench pass, sorted.
    ///
    /// Reported whether or not one was held, because *"this update touches the safety kernel"* is
    /// a fact an operator wants on the screen before they go and book bench time.
    pub gated_components: Vec<String>,
}

fn fault(
    code: BundleFaultCode,
    severity: Severity,
    message: String,
    subjects: Vec<String>,
) -> BundleFault {
    BundleFault {
        code,
        severity,
        message,
        subjects,
    }
}

/// §12's third rule, and everything else that decides whether an image may be installed.
///
/// Every check runs, and the result carries all of them. An operator fixing four problems one
/// attempt at a time is the failure `admit_package` already refuses to cause, and an OS update is
/// the case where each attempt costs a reboot.
pub fn admit_bundle(bundle: &BundleManifest, subject: &UpdateSubject) -> BundleVerdict {
    let mut faults = Vec::new();

    if bundle.version.trim().is_empty() {
        faults.push(fault(
            BundleFaultCode::Unversioned,
            Severity::Error,
            "This bundle names no version, so nothing can say what the robot would be running \
             afterwards. Rebuild it with a version."
                .to_string(),
            Vec::new(),
        ));
    }

    // Before the certification gate, because that gate binds a bench pass to this digest and a
    // bundle with no digest cannot be bound to anything. Reported separately so the two refusals do
    // not read as one problem.
    if bundle.digest.trim().is_empty() {
        faults.push(fault(
            BundleFaultCode::Undigested,
            Severity::Error,
            "This bundle names no digest. §12's certification gate binds a bench pass to the exact \
             bytes it was taken against, and bytes with no name cannot be bound to."
                .to_string(),
            Vec::new(),
        ));
    }

    // §37.1's five outcomes, kept apart here for the reason they are kept apart there: four
    // different facts with four different people at the end of them.
    match subject.signature {
        SignatureState::Verified => {}
        SignatureState::Failed => faults.push(fault(
            BundleFaultCode::SignatureFailed,
            Severity::Error,
            "A signature was present and did not verify. The bytes are attested to somebody other \
             than the manifest claims, which is what tampering looks like — do not install this, \
             and find out where it came from."
                .to_string(),
            Vec::new(),
        )),
        SignatureState::Untrusted => faults.push(fault(
            BundleFaultCode::SignatureUntrusted,
            Severity::Error,
            "The signature verifies against a key this robot has not been told to believe. Add the \
             key with `skynetd trust add`, or find out why a stranger signed an OS image for this \
             fleet."
                .to_string(),
            Vec::new(),
        )),
        SignatureState::Absent => faults.push(fault(
            BundleFaultCode::SignatureAbsent,
            Severity::Error,
            "Nobody signed this bundle. §12 signs images with the same Ed25519 machinery as \
             releases, and an unsigned root filesystem is a root filesystem anybody on the path \
             could have written."
                .to_string(),
            Vec::new(),
        )),
        SignatureState::Unverifiable => faults.push(fault(
            BundleFaultCode::SignatureUnverifiable,
            Severity::Error,
            "This platform cannot check a signature at all, so it cannot establish that this image \
             is the one the fleet published. That is the operating system's problem rather than \
             the publisher's."
                .to_string(),
            Vec::new(),
        )),
    }

    // Compatibility. A bundle that declares nothing is a **warning**, because §12 does not require
    // the field and refusing on it would stop every image built before it existed — but it is said
    // out loud, because "this image does not know what it is for" is exactly what somebody wants to
    // have been told after flashing the wrong board.
    match (&subject.board, bundle.compatible.is_empty()) {
        (_, true) => faults.push(fault(
            BundleFaultCode::CompatibilityUndeclared,
            Severity::Warning,
            "This bundle declares no compatible machines, so nothing here can tell whether it is \
             built for this one."
                .to_string(),
            Vec::new(),
        )),
        (Some(board), false) if !bundle.compatible.iter().any(|m| m == board) => {
            faults.push(fault(
                BundleFaultCode::Incompatible,
                Severity::Error,
                format!(
                    "This bundle is built for {}, and this machine is `{board}`.",
                    bundle.compatible.join(", ")
                ),
                bundle.compatible.clone(),
            ));
        }
        // A robot that cannot say what board it is has not established a mismatch, and refusing
        // would make an unknown machine unupdatable — which is the state a robot is in exactly when
        // it most needs an update.
        (None, false) => faults.push(fault(
            BundleFaultCode::CompatibilityUndeclared,
            Severity::Warning,
            format!(
                "This machine does not report what it is, so the bundle's compatibility with {} \
                 could not be checked.",
                bundle.compatible.join(", ")
            ),
            bundle.compatible.clone(),
        )),
        _ => {}
    }

    if !subject.channel.is_empty()
        && !bundle.channel.is_empty()
        && subject.channel != bundle.channel
    {
        faults.push(fault(
            BundleFaultCode::ChannelMismatch,
            Severity::Error,
            format!(
                "The desired state asks for the `{}` channel and this bundle is `{}`. A robot \
                 pulled onto another channel by an image rather than by an operator is a robot \
                 nobody moved.",
                subject.channel, bundle.channel
            ),
            Vec::new(),
        ));
    }

    // ── §12's third rule ─────────────────────────────────────────────────────
    let mut gated_components: Vec<String> = bundle
        .changes
        .iter()
        .filter(|component| gated(component))
        .cloned()
        .collect();
    gated_components.sort();
    gated_components.dedup();

    if !gated_components.is_empty() {
        let unrecognised: Vec<String> = gated_components
            .iter()
            .filter(|component| !OS_COMPONENTS.contains(&component.as_str()))
            .cloned()
            .collect();

        match &subject.bench {
            None => faults.push(fault(
                BundleFaultCode::CertificationRequired,
                Severity::Error,
                certification_message(&gated_components, &unrecognised),
                gated_components.clone(),
            )),
            Some(bench) if bench.image_digest != bundle.digest || bundle.digest.is_empty() => {
                faults.push(fault(
                    BundleFaultCode::BenchPassStale,
                    Severity::Error,
                    format!(
                        "The bench pass this robot holds was taken against `{}` and this bundle is \
                         `{}`. Evidence that is not bound to what it is evidence of is not \
                         evidence — book a fresh bench pass for these bytes.",
                        display_digest(&bench.image_digest),
                        display_digest(&bundle.digest),
                    ),
                    gated_components.clone(),
                ));
            }
            Some(bench) if !bench.passed => faults.push(fault(
                BundleFaultCode::BenchPassFailed,
                Severity::Error,
                format!(
                    "A bench pass exists for these bytes and it records a failure. This update \
                     changes {}, and §12 refuses one that has not passed rather than one that has \
                     not been tried.",
                    gated_components.join(", ")
                ),
                gated_components.clone(),
            )),
            Some(_) => {}
        }
    }

    if !subject.running_version.is_empty() && subject.running_version == bundle.version {
        faults.push(fault(
            BundleFaultCode::AlreadyRunning,
            Severity::Warning,
            format!(
                "This robot is already running `{}`. Staging it again is harmless and applying it \
                 costs a reboot for nothing.",
                bundle.version
            ),
            Vec::new(),
        ));
    }

    BundleVerdict {
        admitted: !faults.iter().any(|fault| fault.severity == Severity::Error),
        faults,
        gated_components,
    }
}

fn display_digest(digest: &str) -> String {
    if digest.is_empty() {
        return "nothing".to_string();
    }
    digest.to_string()
}

fn certification_message(gated_components: &[String], unrecognised: &[String]) -> String {
    let mut message = format!(
        "This update changes {}, and §12 refuses one that changes the safety kernel or the \
         envelope without a fresh bench pass for these exact bytes.",
        gated_components.join(", ")
    );
    if !unrecognised.is_empty() {
        message.push_str(&format!(
            " {} is not a component this edition recognises, and an unrecognised component is \
             gated for the same reason the two named ones are: nobody has established what it \
             changes.",
            unrecognised.join(", ")
        ));
    }
    message.push_str(" This robot holds no bench pass at all.");
    message
}

// ── §12's second rule ────────────────────────────────────────────────────────

/// What to do about a slot the robot has just booted into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootDecision {
    /// Nothing to decide: this is not a first boot into a new image.
    NotPending,
    /// Still inside the budget and not yet healthy. Say nothing to the bootloader.
    ///
    /// The default, and the one that must never be skipped: a supervisor that confirmed a slot
    /// before the health check finished would have replaced an unattended recovery with a coin
    /// toss.
    Wait,
    /// Healthy. Mark the slot good, so the bootloader stops counting.
    Confirm,
    /// The budget expired without health. Mark the slot bad and let the bootloader return.
    RollBack,
}

impl BootDecision {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            BootDecision::NotPending => "not_pending",
            BootDecision::Wait => "wait",
            BootDecision::Confirm => "confirm",
            BootDecision::RollBack => "roll_back",
        }
    }
}

/// Every decision, weakest action first.
pub const BOOT_DECISIONS: [BootDecision; 4] = [
    BootDecision::NotPending,
    BootDecision::Wait,
    BootDecision::Confirm,
    BootDecision::RollBack,
];

/// §12's default boot budget, in milliseconds.
///
/// Ten minutes. Long enough for a robot that boots slowly, fetches a desired state over a poor link
/// and starts a driver that probes a bus; short enough that a warehouse at 3 a.m. is not a warehouse
/// with a brick in it until morning. The document does not give a number, so this is a default an
/// operator may narrow rather than a constant anybody derived.
pub const DEFAULT_BOOT_BUDGET_MS: f64 = 600_000.0;

/// What the supervisor knows about this boot.
#[derive(Debug, Clone, PartialEq)]
pub struct BootEvidence {
    /// Whether this boot is the first into a slot nothing has confirmed.
    ///
    /// False makes every other field irrelevant, and that is deliberate: a supervisor that could
    /// mark a slot bad on an ordinary boot would be a supervisor that can roll back a robot nobody
    /// updated.
    pub pending: bool,
    /// Milliseconds since this boot, supplied (§35.2).
    pub elapsed_ms: f64,
    /// The budget, from the operator profile or [`DEFAULT_BOOT_BUDGET_MS`].
    pub budget_ms: f64,
    /// Whether §48.2's self-test concluded the robot may run.
    ///
    /// `None` is *"the self-test has not finished"*, which is not a pass and is not a failure —
    /// it is the ordinary state for the first seconds of a boot, and it is why [`BootDecision::Wait`]
    /// exists.
    pub post_passed: Option<bool>,
    /// Whether every package the desired state wants running is running.
    pub packages_healthy: Option<bool>,
    /// Whether the daemon reconciled at least one pass to convergence.
    pub converged: Option<bool>,
}

impl Default for BootEvidence {
    /// A boot nobody has observed: pending nothing, knowing nothing.
    fn default() -> Self {
        Self {
            pending: false,
            elapsed_ms: 0.0,
            budget_ms: DEFAULT_BOOT_BUDGET_MS,
            post_passed: None,
            packages_healthy: None,
            converged: None,
        }
    }
}

/// One boot decision, with the reason.
#[derive(Debug, Clone, PartialEq)]
pub struct BootVerdict {
    /// What to do.
    pub decision: BootDecision,
    /// Why, in a sentence an operator reads after the fact.
    pub detail: String,
    /// The named checks that have not passed, sorted. Empty on a confirm.
    pub missing: Vec<String>,
}

/// §12's second rule — health-gated rollback.
///
/// > The new slot must pass a health check within a boot budget or the bootloader returns to the
/// > previous slot unattended.
///
/// Three properties, and each is the reason a rule of this shape is written down rather than left
/// to a script:
///
/// * **Absence of evidence is not health.** Every check is an `Option` and `None` never confirms.
///   A supervisor that marked a slot good because it had not yet been told otherwise would replace
///   §12's unattended recovery with a race between the health check and the confirmation.
/// * **The budget is what makes it unattended.** Without it a robot that never becomes healthy
///   waits for ever, in the state the rollback exists to leave. With it, the worst case is a reboot.
/// * **A robot that is not pending decides nothing.** `mark bad` on an ordinary boot is a rollback
///   of an update nobody made.
pub fn confirm_boot(evidence: &BootEvidence) -> BootVerdict {
    if !evidence.pending {
        return BootVerdict {
            decision: BootDecision::NotPending,
            detail: "This boot is not the first into a new slot, so there is nothing to confirm \
                     and nothing to roll back."
                .to_string(),
            missing: Vec::new(),
        };
    }

    let mut missing = Vec::new();
    if evidence.post_passed != Some(true) {
        missing.push("post".to_string());
    }
    if evidence.packages_healthy != Some(true) {
        missing.push("packages".to_string());
    }
    if evidence.converged != Some(true) {
        missing.push("converged".to_string());
    }
    missing.sort();

    if missing.is_empty() {
        return BootVerdict {
            decision: BootDecision::Confirm,
            detail: format!(
                "The self-test passed, every wanted package is running and the reconciler \
                 converged, {} ms into a {} ms budget. The slot is good.",
                crate::json::number_text(evidence.elapsed_ms),
                crate::json::number_text(evidence.budget_ms),
            ),
            missing,
        };
    }

    // A budget that is not a positive number is no budget, and no budget means waiting for ever
    // rather than rolling back on the first pass. The safe direction here is the patient one: an
    // unattended rollback triggered by a misconfigured budget is a working robot rebooted into an
    // older image for no reason.
    let expired = evidence.budget_ms.is_finite()
        && evidence.budget_ms > 0.0
        && evidence.elapsed_ms.is_finite()
        && evidence.elapsed_ms >= evidence.budget_ms;

    if expired {
        return BootVerdict {
            decision: BootDecision::RollBack,
            detail: format!(
                "{} ms have passed and the budget is {} ms, and {} still {} not passed. Marking \
                 the slot bad; the bootloader returns to the previous one.",
                crate::json::number_text(evidence.elapsed_ms),
                crate::json::number_text(evidence.budget_ms),
                missing.join(", "),
                if missing.len() == 1 { "has" } else { "have" },
            ),
            missing,
        };
    }

    BootVerdict {
        decision: BootDecision::Wait,
        detail: format!(
            "{} ms into a {} ms budget, with {} still to pass. Saying nothing to the bootloader \
             until they do or the budget expires.",
            crate::json::number_text(evidence.elapsed_ms),
            crate::json::number_text(evidence.budget_ms),
            missing.join(", "),
        ),
        missing,
    }
}

// ── Wire shapes ──────────────────────────────────────────────────────────────

/// Serialise a bundle verdict.
pub fn bundle_verdict_to_json(verdict: &BundleVerdict) -> Json {
    let mut faults = Json::arr();
    for entry in &verdict.faults {
        let mut row = Json::obj();
        row.set("code", Json::Str(entry.code.as_str().to_string()))
            .set("severity", Json::Str(entry.severity.as_str().to_string()))
            .set("message", Json::Str(entry.message.clone()))
            .set(
                "subjects",
                Json::Arr(
                    entry
                        .subjects
                        .iter()
                        .map(|s| Json::Str(s.clone()))
                        .collect(),
                ),
            );
        faults.push(row);
    }
    let mut out = Json::obj();
    out.set("admitted", Json::Bool(verdict.admitted))
        .set("faults", faults)
        .set(
            "gated_components",
            Json::Arr(
                verdict
                    .gated_components
                    .iter()
                    .map(|c| Json::Str(c.clone()))
                    .collect(),
            ),
        );
    out
}

/// Serialise a boot verdict.
pub fn boot_verdict_to_json(verdict: &BootVerdict) -> Json {
    let mut out = Json::obj();
    out.set("decision", Json::Str(verdict.decision.as_str().to_string()))
        .set("detail", Json::Str(verdict.detail.clone()))
        .set(
            "missing",
            Json::Arr(
                verdict
                    .missing
                    .iter()
                    .map(|m| Json::Str(m.clone()))
                    .collect(),
            ),
        );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn bundle(changes: &[&str]) -> BundleManifest {
        BundleManifest {
            version: "1.4.0".to_string(),
            channel: "stable".to_string(),
            digest: "sha256:aaaa".to_string(),
            size: Some(402_653_184),
            compatible: vec!["bench-arm-v1".to_string()],
            changes: changes.iter().map(|c| (*c).to_string()).collect(),
        }
    }

    fn subject() -> UpdateSubject {
        UpdateSubject {
            running_version: "1.3.0".to_string(),
            board: Some("bench-arm-v1".to_string()),
            channel: "stable".to_string(),
            signature: SignatureState::Verified,
            bench: None,
        }
    }

    fn codes(verdict: &BundleVerdict) -> Vec<&'static str> {
        verdict
            .faults
            .iter()
            .map(|fault| fault.code.as_str())
            .collect()
    }

    #[test]
    fn an_ordinary_update_is_admitted_with_nothing_to_say() {
        let verdict = admit_bundle(&bundle(&["userspace"]), &subject());
        assert!(verdict.admitted);
        assert!(verdict.faults.is_empty());
        assert!(verdict.gated_components.is_empty());
    }

    /// §12's third rule, and the test the document calls the most important in the suite.
    #[test]
    fn a_kernel_change_without_a_bench_pass_is_refused() {
        let verdict = admit_bundle(&bundle(&["safety_kernel", "userspace"]), &subject());
        assert!(!verdict.admitted);
        assert_eq!(codes(&verdict), ["certification_required"]);
        assert_eq!(verdict.gated_components, ["safety_kernel"]);
        assert!(verdict.faults[0].message.contains("safety_kernel"));
        assert!(verdict.faults[0].message.contains("bench pass"));
    }

    #[test]
    fn an_envelope_change_is_gated_for_the_same_reason() {
        let verdict = admit_bundle(&bundle(&["envelope"]), &subject());
        assert!(!verdict.admitted);
        assert_eq!(verdict.gated_components, ["envelope"]);
    }

    /// The way somebody would actually get past this gate: not by writing `safety_kernel`, but by
    /// writing something a two-word matcher has never heard of.
    #[test]
    fn a_component_nobody_recognises_is_gated_and_the_refusal_says_why() {
        let verdict = admit_bundle(&bundle(&["rt-core"]), &subject());
        assert!(!verdict.admitted);
        assert_eq!(verdict.gated_components, ["rt-core"]);
        assert!(
            verdict.faults[0]
                .message
                .contains("not a component this edition recognises"),
            "{}",
            verdict.faults[0].message
        );
        // And the classification itself, independent of the message.
        assert!(gated("rt-core"));
        assert!(gated("safety_kernel"));
        assert!(gated("envelope"));
        assert!(!gated("userspace"));
        assert!(!gated("skynetd"));
    }

    #[test]
    fn a_bench_pass_for_these_bytes_opens_the_gate() {
        let mut robot = subject();
        robot.bench = Some(BenchPass {
            image_digest: "sha256:aaaa".to_string(),
            tier: "host".to_string(),
            passed: true,
        });
        let verdict = admit_bundle(&bundle(&["safety_kernel"]), &robot);
        assert!(verdict.admitted);
        assert_eq!(
            verdict.gated_components,
            ["safety_kernel"],
            "and it still says what was gated, because that is what an operator wants on the screen"
        );
    }

    /// The anti-gaming case: evidence not bound to what it is evidence of is not evidence.
    #[test]
    fn a_bench_pass_for_other_bytes_does_not() {
        let mut robot = subject();
        robot.bench = Some(BenchPass {
            image_digest: "sha256:bbbb".to_string(),
            tier: "host".to_string(),
            passed: true,
        });
        let verdict = admit_bundle(&bundle(&["safety_kernel"]), &robot);
        assert!(!verdict.admitted);
        assert_eq!(codes(&verdict), ["bench_pass_stale"]);
        assert!(verdict.faults[0].message.contains("sha256:bbbb"));
        assert!(verdict.faults[0].message.contains("sha256:aaaa"));
    }

    #[test]
    fn a_recorded_failure_is_not_a_pass() {
        let mut robot = subject();
        robot.bench = Some(BenchPass {
            image_digest: "sha256:aaaa".to_string(),
            tier: "host".to_string(),
            passed: false,
        });
        let verdict = admit_bundle(&bundle(&["safety_kernel"]), &robot);
        assert!(!verdict.admitted);
        assert_eq!(codes(&verdict), ["bench_pass_failed"]);
    }

    /// A bundle with no digest cannot be the subject of evidence, so a pass claiming to cover it
    /// covers nothing.
    #[test]
    fn a_pass_bound_to_no_digest_covers_no_bundle() {
        let mut image = bundle(&["safety_kernel"]);
        image.digest = String::new();
        let mut robot = subject();
        robot.bench = Some(BenchPass {
            image_digest: String::new(),
            tier: "host".to_string(),
            passed: true,
        });
        let verdict = admit_bundle(&image, &robot);
        assert!(!verdict.admitted);
        assert_eq!(codes(&verdict), ["undigested", "bench_pass_stale"]);
    }

    #[test]
    fn every_signature_state_but_verified_refuses_an_image() {
        for state in [
            SignatureState::Failed,
            SignatureState::Untrusted,
            SignatureState::Absent,
            SignatureState::Unverifiable,
        ] {
            let mut robot = subject();
            robot.signature = state;
            let verdict = admit_bundle(&bundle(&["userspace"]), &robot);
            assert!(!verdict.admitted, "{state:?} should refuse");
            assert_eq!(verdict.faults.len(), 1);
        }
    }

    #[test]
    fn an_image_for_another_board_is_refused_and_an_unknown_board_is_not() {
        let mut image = bundle(&["userspace"]);
        image.compatible = vec!["warehouse-amr-v2".to_string()];
        let verdict = admit_bundle(&image, &subject());
        assert!(!verdict.admitted);
        assert_eq!(codes(&verdict), ["incompatible"]);

        // A robot that cannot say what it is has not established a mismatch, and refusing would
        // make an unknown machine unupdatable — which is when it most needs an update.
        let mut robot = subject();
        robot.board = None;
        let verdict = admit_bundle(&image, &robot);
        assert!(verdict.admitted);
        assert_eq!(codes(&verdict), ["compatibility_undeclared"]);
    }

    #[test]
    fn a_channel_the_operator_did_not_ask_for_is_refused() {
        let mut image = bundle(&["userspace"]);
        image.channel = "beta".to_string();
        let verdict = admit_bundle(&image, &subject());
        assert!(!verdict.admitted);
        assert_eq!(codes(&verdict), ["channel_mismatch"]);
    }

    /// Re-converging on a desired state the robot already satisfies is the ordinary case after a
    /// rollback, so it warns rather than refusing.
    #[test]
    fn the_version_already_running_is_a_warning() {
        let mut robot = subject();
        robot.running_version = "1.4.0".to_string();
        let verdict = admit_bundle(&bundle(&["userspace"]), &robot);
        assert!(verdict.admitted);
        assert_eq!(codes(&verdict), ["already_running"]);
    }

    // ── §12's second rule ────────────────────────────────────────────────────

    #[test]
    fn an_ordinary_boot_decides_nothing() {
        let verdict = confirm_boot(&BootEvidence::default());
        assert_eq!(verdict.decision, BootDecision::NotPending);

        // Even a perfectly healthy one. Confirming a slot nobody staged is a supervisor telling
        // the bootloader about an update that never happened.
        let verdict = confirm_boot(&BootEvidence {
            post_passed: Some(true),
            packages_healthy: Some(true),
            converged: Some(true),
            ..BootEvidence::default()
        });
        assert_eq!(verdict.decision, BootDecision::NotPending);
    }

    /// The property the whole function exists for: absence of evidence is not health.
    #[test]
    fn a_check_nobody_has_run_never_confirms() {
        let verdict = confirm_boot(&BootEvidence {
            pending: true,
            elapsed_ms: 1_000.0,
            ..BootEvidence::default()
        });
        assert_eq!(verdict.decision, BootDecision::Wait);
        assert_eq!(verdict.missing, ["converged", "packages", "post"]);
    }

    #[test]
    fn every_check_passing_confirms_the_slot() {
        let verdict = confirm_boot(&BootEvidence {
            pending: true,
            elapsed_ms: 42_000.0,
            post_passed: Some(true),
            packages_healthy: Some(true),
            converged: Some(true),
            ..BootEvidence::default()
        });
        assert_eq!(verdict.decision, BootDecision::Confirm);
        assert!(verdict.missing.is_empty());
    }

    #[test]
    fn the_budget_is_what_makes_the_recovery_unattended() {
        let evidence = |elapsed: f64| BootEvidence {
            pending: true,
            elapsed_ms: elapsed,
            post_passed: Some(false),
            ..BootEvidence::default()
        };
        assert_eq!(
            confirm_boot(&evidence(DEFAULT_BOOT_BUDGET_MS - 1.0)).decision,
            BootDecision::Wait
        );
        assert_eq!(
            confirm_boot(&evidence(DEFAULT_BOOT_BUDGET_MS)).decision,
            BootDecision::RollBack
        );
    }

    /// The patient reading, and it is the safe one: an unattended rollback triggered by a
    /// misconfigured budget is a working robot rebooted into an older image for no reason.
    #[test]
    fn a_budget_that_is_not_a_budget_waits_rather_than_rolling_back() {
        for budget in [0.0, -1.0, f64::NAN] {
            let verdict = confirm_boot(&BootEvidence {
                pending: true,
                elapsed_ms: 5_000.0,
                budget_ms: budget,
                post_passed: Some(false),
                ..BootEvidence::default()
            });
            assert_eq!(verdict.decision, BootDecision::Wait, "budget {budget}");
        }
    }

    #[test]
    fn the_report_names_which_check_is_missing() {
        let verdict = confirm_boot(&BootEvidence {
            pending: true,
            elapsed_ms: 60_000.0,
            post_passed: Some(true),
            packages_healthy: Some(true),
            ..BootEvidence::default()
        });
        assert_eq!(verdict.missing, ["converged"]);
        assert!(verdict.detail.contains("converged"), "{}", verdict.detail);
    }

    #[test]
    fn the_vocabularies_have_no_duplicates_and_the_gated_two_are_components() {
        let mut sorted = OS_COMPONENTS.to_vec();
        sorted.sort_unstable();
        let mut deduped = sorted.clone();
        deduped.dedup();
        assert_eq!(sorted, deduped);
        for name in CERTIFICATION_GATED {
            assert!(
                OS_COMPONENTS.contains(&name),
                "`{name}` is gated and is not a component, so nothing could ever declare it"
            );
        }
    }
}

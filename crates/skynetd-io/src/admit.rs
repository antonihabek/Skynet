//! Admission, on the robot (§8, §37.1).
//!
//! `admit_package` is the shared contract; this runs it against the machine it is on and turns the
//! result into something an operator reads. Since the twelfth session it had every input but one,
//! and the one was the signature: this edition had no Ed25519, so every package reported
//! `signature_unverifiable` and the report could not gate anything, because a gate that refuses
//! everything is a gate somebody takes out.
//!
//! Ed25519 is written ([`skynet_ed25519`]) and the trust store exists ([`crate::trust`]), so the
//! signature is now a fact rather than a gap. What changed with it:
//!
//! * **The state is one of five and it is measured.** `verified`, `failed`, `untrusted`, `absent`,
//!   `unverifiable` — and this edition can produce the first four. `unverifiable` no longer appears
//!   at all, which is the sentence this module existed to stop having to write.
//! * **`untrusted` is new**, and it is the state a verifier discovers the moment there is a trust
//!   store to check against: a signature can be entirely valid and made by a stranger. It is not
//!   `failed`, because nothing went wrong, and the remedy is in `state/trust.json` rather than
//!   anywhere near the package.
//! * **There is no longer an unavoidable rejection.** Every error this reports is one somebody can
//!   act on, which is what makes gating on it defensible for the first time.
//!
//! # What gates, and what still only reports
//!
//! §19 is specific about where the line is: *"a driver runs native and must therefore be signed by
//! a key the operator has trusted."* A driver is the one package kind admitted unsandboxed, so a
//! driver whose signature is anything but `verified` **does not activate** — the refusal is in
//! [`crate::runtime`], at the symlink swap, because a staged blob is inert and the swap is the
//! consequential moment.
//!
//! Everything else still reports. Not because the check is weaker for a policy, but because there
//! is nothing on a robot that can *produce* a signature: the Hub signs releases (§19), and a
//! developer staging a package they built five minutes ago has nothing to sign it with. Refusing
//! those would refuse the golden path in the README and every local install, and the fix would be a
//! flag — which is the failure this module has been avoiding since it was written.
//!
//! That asymmetry is a real gap and it is recorded as one rather than hidden: a policy with no
//! signature installs today, and it says `unsigned` in every report that mentions it.

use skynet_contracts::admission::{
    admit_package, AdmissionParams, AdmissionResult, PackageFinding, Severity, SignatureState,
};
use skynet_json::Json;

use crate::capacity::Capacity;

/// What admission concluded about one package on this robot.
#[derive(Debug, Clone)]
pub enum Report {
    /// It could not be run, and why.
    ///
    /// A robot whose capacity is unknown cannot be reasoned about: every conclusion admission drew
    /// would be about a machine that does not exist. Reported rather than answered with defaults.
    NotRun {
        /// What was missing.
        detail: String,
    },
    /// It ran.
    Ran {
        /// Everything found, in the order the checks run.
        result: AdmissionResult,
        /// What was concluded about the signature, and by whom.
        signature: SignatureNote,
    },
}

/// The signature verdict, carried alongside the findings.
///
/// Admission's finding says *what* is wrong with a signature; this says *which key* was involved
/// and *why* the verdict came out that way, which is the half an operator can act on. A report that
/// says `signature_untrusted` and nothing else has told somebody there is a problem and not where
/// to look for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureNote {
    /// The state given to admission.
    pub state: SignatureState,
    /// The trust store's label for the signer, when it verified against a trusted key.
    pub signer: Option<String>,
    /// A sentence naming what happened. Empty when it verified.
    pub detail: String,
}

impl SignatureNote {
    /// The note for a package nothing accompanied.
    pub fn absent() -> SignatureNote {
        SignatureNote {
            state: SignatureState::Absent,
            signer: None,
            detail: "nothing accompanied these bytes to say who produced them".to_string(),
        }
    }
}

impl From<crate::trust::Verdict> for SignatureNote {
    fn from(verdict: crate::trust::Verdict) -> SignatureNote {
        SignatureNote {
            state: verdict.state,
            signer: verdict.signer,
            detail: verdict.detail,
        }
    }
}

impl Report {
    /// Whether anything at all was wrong.
    pub fn clean(&self) -> bool {
        match self {
            Report::NotRun { .. } => false,
            Report::Ran { result, .. } => result.findings.is_empty(),
        }
    }

    /// Every error finding.
    ///
    /// Was `actionable_errors`, and the name carried an exception: it filtered out the one
    /// rejection this edition could not avoid. There is no longer one — a robot that can verify a
    /// signature has no error to excuse — so the filter went with it, and every error here is one
    /// somebody can do something about.
    pub fn errors(&self) -> Vec<&PackageFinding> {
        match self {
            Report::NotRun { .. } => Vec::new(),
            Report::Ran { result, .. } => result
                .findings
                .iter()
                .filter(|f| f.severity == Severity::Error)
                .collect(),
        }
    }

    /// Whether a driver carrying this report may be activated (§19).
    ///
    /// Only a `verified` signature admits a driver. Not `untrusted` — a stranger's valid signature
    /// on native code is the case this rule exists for — and not `absent`, which is the case
    /// everybody assumes it exists for.
    pub fn driver_may_activate(&self) -> bool {
        match self {
            Report::NotRun { .. } => false,
            Report::Ran { signature, .. } => signature.state == SignatureState::Verified,
        }
    }
}

/// Run admission for one package on this robot.
///
/// `observed_digest` is the digest the bytes actually hash to — the store computes it while
/// verifying, so admission is told what was measured rather than what was claimed. `signature` is
/// what [`crate::trust`] concluded, for the same reason: a measured verdict, never a default.
pub fn run(
    manifest: &skynet_contracts::admission::PackageManifest,
    capacity: &Capacity,
    observed_digest: &str,
    signature: SignatureNote,
) -> Report {
    let Some(capacity) = capacity.known() else {
        let Capacity::Unavailable { detail } = capacity else {
            unreachable!("a capacity is known or unavailable")
        };
        return Report::NotRun {
            detail: detail.clone(),
        };
    };

    let result = admit_package(&AdmissionParams {
        manifest: manifest.clone(),
        capacity: capacity.clone(),
        signature: signature.state,
        observed_digest: observed_digest.to_string(),
        // §37.1 resolves transport at install and a `refused` verdict fails the install. There is
        // no policy to plan for at stage time and no source embodiment to plan from, so `None` —
        // which admission reads as "no transport verdict was reached" rather than as a pass.
        transport_verdict: None,
        // Nothing scans the package. `false` is what this edition can establish, and it is the
        // conservative direction: it does not claim the package is inert.
        executes_on_load: false,
        trust_remote_code: false,
    });

    Report::Ran { result, signature }
}

/// The report, for a terminal.
///
/// Every finding, not the first — §8.2's rule, because fixing four problems one deployment at a
/// time is precisely how people end up granting everything.
pub fn to_lines(report: &Report) -> Vec<String> {
    let mut lines = Vec::new();
    match report {
        Report::NotRun { detail } => {
            lines.push("admission: not run".to_string());
            lines.push(format!("  {detail}"));
            lines.push(
                "  The blob is staged and inert. Nothing has checked whether this robot can feed it."
                    .to_string(),
            );
        }
        Report::Ran { result, signature } => {
            match (&signature.signer, signature.state) {
                (Some(signer), SignatureState::Verified) => {
                    lines.push(format!("signature: verified — signed by `{signer}`"));
                }
                _ => lines.push(format!(
                    "signature: {} — {}",
                    signature.state.as_str(),
                    signature.detail
                )),
            }

            if result.findings.is_empty() {
                lines.push("admission: nothing found".to_string());
            }
            for finding in &result.findings {
                let severity = match finding.severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                };
                let subjects = finding
                    .subjects
                    .as_ref()
                    .map(|s| format!(" [{}]", s.join(", ")))
                    .unwrap_or_default();
                lines.push(format!(
                    "admission {severity}: {} — {}{subjects}",
                    finding.code.as_str(),
                    finding.message
                ));
            }
            if !result.effective_grants.is_empty() {
                lines.push(format!(
                    "admission: capabilities that would be active — {}",
                    result.effective_grants.join(", ")
                ));
            }
            // Said on every run rather than only when it matters, for the same reason it always
            // was: a reader who sees a clean report must not come away believing more was enforced
            // than was.
            lines.push(
                "admission: staging is not gated — the blob is inert until something activates it. \
                 Activation refuses a driver whose signature is not `verified` (§19); every other \
                 kind activates and carries its findings."
                    .to_string(),
            );
        }
    }
    lines
}

/// The report, for the API.
pub fn to_json(report: &Report) -> Json {
    let mut document = Json::obj();
    match report {
        Report::NotRun { detail } => {
            document
                .set("state", Json::from("not_run"))
                .set("detail", Json::from(detail.as_str()));
        }
        Report::Ran { result, signature } => {
            let mut note = Json::obj();
            note.set("state", Json::from(signature.state.as_str()));
            note.set(
                "signer",
                match &signature.signer {
                    Some(label) => Json::from(label.as_str()),
                    None => Json::Null,
                },
            );
            note.set("detail", Json::from(signature.detail.as_str()));

            document
                .set("state", Json::from("ran"))
                // Deliberately **not** `result.admitted`. That field answers "would this install",
                // and staging installs nothing — reporting it would invite a reader to believe a
                // decision was made and acted on.
                .set("would_admit", Json::Bool(result.admitted))
                .set("signature", note)
                .set("gates", Json::Bool(false))
                .set(
                    "gates_note",
                    Json::from(
                        "Staging is not gated: the blob is inert until something activates it. \
                         Activation refuses a driver whose signature is not `verified` (§19). \
                         Other kinds activate carrying their findings, because nothing on a robot \
                         can produce a signature — the Hub signs releases — and refusing them \
                         would refuse every local install.",
                    ),
                )
                .set(
                    "findings",
                    Json::Arr(
                        result
                            .findings
                            .iter()
                            .map(|finding| {
                                let mut item = Json::obj();
                                item.set("code", Json::from(finding.code.as_str()))
                                    .set(
                                        "severity",
                                        Json::from(match finding.severity {
                                            Severity::Error => "error",
                                            Severity::Warning => "warning",
                                        }),
                                    )
                                    .set("message", Json::from(finding.message.as_str()));
                                if let Some(subjects) = &finding.subjects {
                                    item.set(
                                        "subjects",
                                        Json::Arr(
                                            subjects
                                                .iter()
                                                .map(|s| Json::from(s.as_str()))
                                                .collect(),
                                        ),
                                    );
                                }
                                item
                            })
                            .collect(),
                    ),
                )
                .set(
                    "effective_grants",
                    Json::Arr(
                        result
                            .effective_grants
                            .iter()
                            .map(|s| Json::from(s.as_str()))
                            .collect(),
                    ),
                );
        }
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;
    use skynet_contracts::admission::{
        DeviceCapacity, EmbodimentRequirement, PackageRejectionCode, ResourceBudget,
    };

    fn manifest(ram_mb: f64, needs: &[&str]) -> skynet_contracts::admission::PackageManifest {
        skynet_contracts::admission::PackageManifest {
            schema_version: 1,
            id: "openrobot/grasp".to_string(),
            version: "1.0.0".to_string(),
            kind: "policy".to_string(),
            digest: "a".repeat(64),
            isolation: "sandboxed".to_string(),
            budget: ResourceBudget {
                ram_mb,
                vram_mb: None,
                cpu_cores: None,
                disk_mb: None,
                control_hz: None,
                inference_budget_ms: None,
            },
            embodiment: EmbodimentRequirement {
                capabilities: needs.iter().map(|s| s.to_string()).collect(),
                extensions: Vec::new(),
                ..Default::default()
            },
            requests: Vec::new(),
            runtime: None,
            depends_on: Vec::new(),
            min_os_version: None,
        }
    }

    fn capacity(ram_mb: f64, provides: &[&str]) -> Capacity {
        Capacity::Known(Box::new(DeviceCapacity {
            ram_mb,
            os_version: "0.1.0".to_string(),
            capabilities: provides.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }))
    }

    fn signed_by(label: &str) -> SignatureNote {
        SignatureNote {
            state: SignatureState::Verified,
            signer: Some(label.to_string()),
            detail: String::new(),
        }
    }

    fn untrusted() -> SignatureNote {
        SignatureNote {
            state: SignatureState::Untrusted,
            signer: None,
            detail: "signed by `beef…`, which is not among the 1 key(s) this robot trusts"
                .to_string(),
        }
    }

    /// The property the module used to turn on, inverted.
    ///
    /// There was an `UNAVOIDABLE_HERE`: one rejection code this edition produced on every package
    /// because it could not check a signature, and which every caller had to except. With Ed25519
    /// there is nothing to except — a clean package on a fine robot with a trusted signature has no
    /// findings at all, which is the first time that has been true.
    #[test]
    fn a_verified_package_on_a_fine_robot_has_nothing_wrong_with_it() {
        let report = run(
            &manifest(64.0, &[]),
            &capacity(1024.0, &[]),
            &"a".repeat(64),
            signed_by("acme-robotics"),
        );
        assert!(report.clean(), "{report:?}");
        assert!(report.errors().is_empty());
        assert!(report.driver_may_activate());
    }

    /// The four states that are findings, each producing its own code.
    #[test]
    fn each_signature_state_produces_its_own_finding() {
        for (state, code) in [
            (
                SignatureState::Failed,
                PackageRejectionCode::SignatureFailed,
            ),
            (
                SignatureState::Untrusted,
                PackageRejectionCode::SignatureUntrusted,
            ),
            (SignatureState::Absent, PackageRejectionCode::Unsigned),
            (
                SignatureState::Unverifiable,
                PackageRejectionCode::SignatureUnverifiable,
            ),
        ] {
            let note = SignatureNote {
                state,
                signer: None,
                detail: "detail".to_string(),
            };
            let report = run(
                &manifest(64.0, &[]),
                &capacity(1024.0, &[]),
                &"a".repeat(64),
                note,
            );
            let codes: Vec<_> = report.errors().iter().map(|f| f.code).collect();
            assert_eq!(codes, vec![code], "{state:?}");
            assert!(!report.driver_may_activate(), "{state:?}");
        }
    }

    /// §19, and the distinction that made `untrusted` worth a state of its own.
    ///
    /// A stranger's *valid* signature on native code is what this rule is for. The obvious case —
    /// nobody signed it — is the one everybody thinks of, and it is not the dangerous one.
    #[test]
    fn a_driver_needs_a_trusted_signature_and_not_merely_a_valid_one() {
        let verified = run(
            &manifest(64.0, &[]),
            &capacity(1024.0, &[]),
            &"a".repeat(64),
            signed_by("acme-robotics"),
        );
        assert!(verified.driver_may_activate());

        let stranger = run(
            &manifest(64.0, &[]),
            &capacity(1024.0, &[]),
            &"a".repeat(64),
            untrusted(),
        );
        assert!(!stranger.driver_may_activate());
    }

    /// The findings that are worth having today.
    #[test]
    fn a_budget_the_robot_cannot_feed_is_found() {
        let report = run(
            &manifest(4096.0, &[]),
            &capacity(512.0, &[]),
            &"a".repeat(64),
            signed_by("acme-robotics"),
        );
        let errors = report.errors();
        assert!(
            errors
                .iter()
                .any(|f| f.code == PackageRejectionCode::BudgetExceeded),
            "{errors:?}"
        );
    }

    #[test]
    fn a_capability_this_body_does_not_provide_is_found() {
        let report = run(
            &manifest(64.0, &["manipulation.grasp"]),
            &capacity(1024.0, &[]),
            &"a".repeat(64),
            signed_by("acme-robotics"),
        );
        let errors = report.errors();
        assert!(
            errors
                .iter()
                .any(|f| f.code == PackageRejectionCode::CapabilityUngranted),
            "{errors:?}"
        );
    }

    /// §5.0's whole point, from the other side: declaring the token admits the package.
    #[test]
    fn a_body_that_declares_the_capability_provides_it() {
        let report = run(
            &manifest(64.0, &["manipulation.grasp"]),
            &capacity(1024.0, &["manipulation.grasp"]),
            &"a".repeat(64),
            signed_by("acme-robotics"),
        );
        assert!(report.errors().is_empty(), "{:?}", report.errors());
    }

    /// An unknown robot is not one to make an install decision about.
    #[test]
    fn an_unavailable_capacity_does_not_produce_an_admission() {
        let report = run(
            &manifest(64.0, &[]),
            &Capacity::Unavailable {
                detail: "no /proc/meminfo".to_string(),
            },
            &"a".repeat(64),
            signed_by("acme-robotics"),
        );
        assert!(matches!(report, Report::NotRun { .. }));
        assert!(report.errors().is_empty());
        // And a driver does not activate off the back of a report that never ran.
        assert!(!report.driver_may_activate());
        let lines = to_lines(&report).join("\n");
        assert!(lines.contains("not run"), "{lines}");
        assert!(lines.contains("inert"), "{lines}");
    }

    /// The signer's name reaches the report, because a hex string is not an answer.
    #[test]
    fn a_verified_report_names_who_signed_it() {
        let report = run(
            &manifest(64.0, &[]),
            &capacity(1024.0, &[]),
            &"a".repeat(64),
            signed_by("acme-robotics"),
        );
        let lines = to_lines(&report).join("\n");
        assert!(lines.contains("signed by `acme-robotics`"), "{lines}");

        let document = to_json(&report);
        let note = document.get("signature").expect("the note is on the wire");
        assert_eq!(note.get("state").and_then(Json::as_str), Some("verified"));
        assert_eq!(
            note.get("signer").and_then(Json::as_str),
            Some("acme-robotics")
        );
    }

    /// The sentence that must be on every report, including a clean one.
    ///
    /// It used to say the signature was not checked. It now says what *is* enforced and what is
    /// not, and the reason it is still unconditional is unchanged: a reader who sees no findings
    /// must not come away believing more was enforced than was.
    #[test]
    fn every_report_says_what_staging_does_and_does_not_enforce() {
        for report in [
            run(
                &manifest(64.0, &[]),
                &capacity(1024.0, &[]),
                &"a".repeat(64),
                signed_by("acme-robotics"),
            ),
            run(
                &manifest(9999.0, &["nope"]),
                &capacity(16.0, &[]),
                &"a".repeat(64),
                untrusted(),
            ),
        ] {
            let lines = to_lines(&report).join("\n");
            assert!(lines.contains("not gated"), "{lines}");
            assert!(lines.contains("driver"), "{lines}");
            assert_eq!(
                to_json(&report).get("gates"),
                Some(&Json::Bool(false)),
                "the wire says it too"
            );
        }
    }

    /// An untrusted report carries the detail, because the code alone does not say where to look.
    #[test]
    fn an_untrusted_signature_reports_where_to_look() {
        let report = run(
            &manifest(64.0, &[]),
            &capacity(1024.0, &[]),
            &"a".repeat(64),
            untrusted(),
        );
        let lines = to_lines(&report).join("\n");
        assert!(lines.contains("signature: untrusted"), "{lines}");
        assert!(lines.contains("not among the 1 key(s)"), "{lines}");
    }
}

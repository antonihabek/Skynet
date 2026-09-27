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
use skynet_contracts::skill::{
    read_calls, validate_composition, CompositionFault, SkillCall, MAX_SKILL_DEPTH,
};
use skynet_contracts::transport::{TransportMethod, TransportPlan};
use skynet_contracts::{resolve_transport, ResolveTransportOptions};
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
        /// §37.1's `transport` stage, when there was something to plan — see [`transport_plan`].
        ///
        /// Carried beside the result rather than folded into it, because the *verdict* is what
        /// admission acts on and the *plan* is what an operator reads: which channel is blocked,
        /// which joint a torque was scaled against, which chain would need a solver. A refusal that
        /// said only "the policy's action space cannot be transported onto this body" would be
        /// true, useless, and the sort of message §64 exists to prevent.
        ///
        /// Boxed for [`crate::capacity::Capacity::Known`]'s reason: a plan carries a binding per
        /// channel and a body has forty of them, so an unboxed variant would make every `Report`
        /// — including the `NotRun` that carries a sentence — the size of a humanoid's action
        /// space. `Option<Box<_>>` is a null pointer when there was nothing to plan.
        plan: Option<Box<TransportPlan>>,
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

    /// Whether a driver carrying this report may be activated (§19).
    ///
    /// Two refusals, and only one of them is about the signature. The state is asked through
    /// [`SignatureState::admits`], which is where §19's rule is written and where a stranger's
    /// perfectly valid signature is refused — the case the rule exists for, as against the unsigned
    /// case everybody assumes it exists for. **A report that never ran also refuses**, which is this
    /// function's own contribution and not the contract's: admission that could not be assessed is
    /// not admission that passed.
    ///
    /// The daemon does not consult this. `runtime.rs`'s activation arm re-derives the same verdict
    /// from the trust store because it never builds a `Report`, and the two agreed by care until
    /// the predicate moved onto `SignatureState`. That much is now one fact; the two paths deciding
    /// activation separately is the larger duplication, and it is still here.
    pub fn driver_may_activate(&self) -> bool {
        match self {
            Report::NotRun { .. } => false,
            Report::Ran { signature, .. } => signature.state.admits(),
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
}

/// A skill-card field, in the two spellings a manifest may carry it in.
///
/// The Hub's card schema puts `task`, `preconditions`, `success_criteria`,
/// `expected_duration_seconds` and `composition` at the **top level** of the manifest it publishes.
/// A manifest assembled from a repository that nests the card puts the same fields under `skill`.
/// Both shapes reach a robot, so both are read.
///
/// They are read *here*, once, because the alternative is what this module and
/// [`crate::skill`] had between them: the same `or_else` chain written out at five call sites. The
/// failure that shape produces is not a crash — it is the sixth field somebody adds being read in
/// one spelling and silently absent in the other, which on a precondition is a clause that never
/// refuses anything.
pub fn card_field<'a>(manifest: &'a Json, name: &str) -> Option<&'a Json> {
    manifest.get_present(name).or_else(|| {
        manifest
            .get("skill")
            .and_then(|card| card.get_present(name))
    })
}

/// The call graph a skill's manifest declares (§54.5).
///
/// `composition.calls`, flat or under `skill`, like every other field a manifest may carry either
/// way. Absent on every package that is not a composing skill, which is nearly all of them.
pub fn declared_calls(manifest: &Json) -> Vec<SkillCall> {
    read_calls(card_field(manifest, "composition").and_then(|c| c.get_present("calls")))
}

/// §54.5's install-time refusal: *"a cycle is refused at install by walking the declared graph, not
/// discovered at runtime."*
///
/// # This is not the same check the runtime makes
///
/// [`crate::skill::Runtime::execute`] validates the **live ancestry** — the chain of invocations
/// that actually happened — and refuses a call that would close a cycle. That is the right check
/// and it happens at exactly the moment §54.5 says it should not be the only one: at runtime, on a
/// robot, with leases held and a deadline running.
///
/// This walks the graph the manifest **declares**, before anything can invoke it. A publisher whose
/// skill calls itself four levels down finds out when they install it, rather than when a cell is
/// waiting and a chain unwinds.
///
/// One function decides both, which is the reason `validate_composition` lives in the contract: two
/// implementations of *acyclic and depth-limited* is two answers to one question, and only one of
/// them has a corpus behind it.
pub fn composition_fault(id: &str, manifest: &Json) -> Option<CompositionFault> {
    let calls = declared_calls(manifest);
    if calls.is_empty() {
        return None;
    }
    validate_composition(id, &calls, MAX_SKILL_DEPTH)
}

/// The sentence a refusal carries, naming the path (§64).
pub fn composition_refusal(id: &str, fault: &CompositionFault) -> String {
    format!(
        "`{id}` declares a call graph §54.5 refuses: {} — {}. A composition system without depth \
         and cycle limits is a way to exhaust a robot, and the graph is walked at install so that \
         nothing discovers it at runtime with leases held and a deadline running.",
        fault.code.as_str(),
        fault.path.join(" → "),
    )
}

/// §37.1's `transport` stage, which had never run.
///
/// > `transport` — `resolveTransport()` against this body; a `refused` verdict fails the install
///
/// One of six stages in §37.1's pipeline and the only one with no implementation: `resolve_transport`
/// had three callers — the Hub, `skynet transport`, and the corpus — and none of them was an
/// install. `AdmissionParams::transport_verdict` has been an `Option` since admission was written
/// and was `None` on every robot, with a comment saying there was no source embodiment to plan
/// from. There is one now (`PackageManifest::source_embodiment`).
///
/// Returns `None` when there is nothing to plan, and the two cases are different and both correct:
///
/// * **The package declares no action space.** A driver, an environment, a dataset — none of them
///   command joints, and a transport verdict about them would be an answer to a question nobody
///   asked.
/// * **This robot has no usable descriptor.** Planning needs the body, and §7.2's rule applies: the
///   absence is reported by the routes that report descriptors rather than converted into a verdict
///   here. A `refused` invented from a missing body would fail every install on an unprovisioned
///   robot with a sentence about the *policy*.
///
/// # The two options, and why neither is guessed
///
/// `requested_hz` is the package's own `budget.control_hz` — the rate it declares it will run at on
/// this robot — rather than the source embodiment's `control_hz`, which is the rate it was
/// *trained* at. A publisher may declare both and they may differ; the plan is made against the one
/// the loop will actually use, because a plan that cleared a rate the package never intends to run
/// is a plan about a different robot.
///
/// `allow_retarget` is the caller's, and §6.1 is the reason it is a parameter rather than a
/// constant: *"retargeting changes what the policy is doing. That should be a decision somebody
/// made, not a fallback that happens quietly."* The person running `skynetd install` is standing
/// there; a reconcile loop is not, and passes `false`.
pub fn transport_plan(
    manifest: &skynet_contracts::admission::PackageManifest,
    body: &crate::embodiment::Body,
    allow_retarget: bool,
) -> Option<TransportPlan> {
    let source = manifest.source_embodiment.as_ref()?;
    let known = body.known()?;
    Some(resolve_transport(
        source,
        &known.descriptor,
        ResolveTransportOptions {
            allow_retarget,
            requested_hz: manifest.budget.control_hz,
        },
    ))
}

/// Run admission for one package on this robot.
///
/// `observed_digest` is the digest the bytes actually hash to — the store computes it while
/// verifying, so admission is told what was measured rather than what was claimed. `signature` is
/// what [`crate::trust`] concluded, for the same reason: a measured verdict, never a default.
///
/// `body` is here for §37.1's `transport` stage — see [`transport_plan`].
pub fn run(
    manifest: &skynet_contracts::admission::PackageManifest,
    capacity: &Capacity,
    observed_digest: &str,
    signature: SignatureNote,
    body: &crate::embodiment::Body,
    allow_retarget: bool,
) -> Report {
    let Some(capacity) = capacity.known() else {
        let Capacity::Unavailable { detail } = capacity else {
            unreachable!("a capacity is known or unavailable")
        };
        return Report::NotRun {
            detail: detail.clone(),
        };
    };

    let plan = transport_plan(manifest, body, allow_retarget).map(Box::new);

    let result = admit_package(&AdmissionParams {
        manifest: manifest.clone(),
        capacity: capacity.clone(),
        signature: signature.state,
        observed_digest: observed_digest.to_string(),
        // §37.1's `transport` stage, resolved above. `None` is still reachable and still means
        // "no transport verdict was reached" rather than a pass — a package with no action space,
        // or a robot with no descriptor to plan against.
        transport_verdict: plan.as_ref().map(|plan| plan.verdict.as_str().to_string()),
        // Nothing scans the package. `false` is what this edition can establish, and it is the
        // conservative direction: it does not claim the package is inert.
        executes_on_load: false,
        trust_remote_code: false,
    });

    Report::Ran {
        result,
        signature,
        plan,
    }
}

/// The verdict in one line, for a record rather than for a reader.
///
/// The black box takes this and the operator's terminal takes [`to_lines`], and the split is §41.1:
/// the recording is not a log file, so what lands on the event channel is the **decision** — the
/// signature state, and how many errors and warnings — while the findings themselves stay in the
/// report somebody reads. A ring filled with prose would be a ring holding fewer control cycles.
pub fn summary(report: &Report) -> String {
    match report {
        Report::NotRun { detail } => format!("admission not run: {detail}"),
        Report::Ran {
            result,
            signature,
            plan,
        } => {
            let errors = result
                .findings
                .iter()
                .filter(|finding| finding.severity == Severity::Error)
                .count();
            let warnings = result.findings.len() - errors;
            let signer = match (&signature.signer, signature.state) {
                (Some(signer), SignatureState::Verified) => format!("signed by {signer}"),
                _ => format!("signature {}", signature.state.as_str()),
            };
            // The verdict rather than the plan. §41.1: what lands on the event channel is the
            // *decision*, and a ring full of per-channel bindings is a ring holding fewer control
            // cycles. Which channel was blocked stays in the report an operator reads.
            let transport = match plan {
                Some(plan) => format!(", transport {}", plan.verdict.as_str()),
                None => String::new(),
            };
            if result.findings.is_empty() {
                format!("admitted, nothing found, {signer}{transport}")
            } else {
                format!("{errors} error(s), {warnings} warning(s), {signer}{transport}")
            }
        }
    }
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
        Report::Ran {
            result,
            signature,
            plan,
        } => {
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
            lines.extend(transport_lines(plan));
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
        Report::Ran {
            result,
            signature,
            plan,
        } => {
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
                )
                // §37.1's `transport` stage. Present-and-null when there was nothing to plan,
                // rather than absent: "this package declares no action space" is a fact worth
                // stating, and a consumer reading an absent key cannot tell it from a producer that
                // forgot to run the stage — which is precisely what every producer did until now.
                .set(
                    "transport",
                    match plan {
                        Some(plan) => skynet_contracts::json::plan_to_json(plan),
                        None => Json::Null,
                    },
                );
        }
    }
    document
}

/// The transport plan, for a terminal (§64).
///
/// Silent when there is nothing to plan — a driver has no action space and a line saying so on
/// every driver install is a line that trains an operator to skim. A plan that came out anything
/// but `direct` gets its channels named, because "the policy's action space cannot be transported
/// onto this body" is a true sentence that tells nobody which joint to look at.
fn transport_lines(plan: &Option<Box<TransportPlan>>) -> Vec<String> {
    let Some(plan) = plan else {
        return Vec::new();
    };
    let mut lines = Vec::new();
    lines.push(format!(
        "transport: {} — {} channel(s) onto `{}`{}",
        plan.verdict.as_str(),
        plan.bindings.len(),
        plan.robot_slug,
        match plan.control_hz {
            Some(hz) => format!(" at {hz} Hz"),
            None => String::new(),
        }
    ));

    for binding in &plan.bindings {
        // The bindings that came out `direct` are the ones nobody needs to read about. Naming all
        // forty on a humanoid would bury the one that is blocked, which is §8.2's rule about
        // findings applied to the thing findings are about.
        if binding.method == TransportMethod::Direct {
            continue;
        }
        lines.push(format!(
            "  channel {}{} — {}{}: {}",
            binding.source_index,
            match &binding.source_role {
                Some(role) => format!(" `{role}`"),
                None => String::new(),
            },
            binding.method.as_str(),
            match &binding.target {
                Some(joint) => format!(" → `{joint}`"),
                None => String::new(),
            },
            binding.reason
        ));
    }
    for warning in &plan.warnings {
        lines.push(format!("  {} — {}", warning.code.as_str(), warning.message));
    }
    lines
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
            source_embodiment: None,
            required_capabilities: Vec::new(),
            required_extensions: Vec::new(),
            serving: None,
            policy_source: None,
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

    /// [`run`], on a robot with no descriptor and no retargeting.
    ///
    /// §37.1's `transport` stage needs the body, and every test in this module is about something
    /// else — the signature, the budget, a capability. `Body::Absent` is what a robot nobody has
    /// provisioned has, it produces no transport verdict, and admission reads that as *no verdict
    /// was reached* rather than as a pass. The tests that are about the stage build a body.
    fn ran(
        manifest: &skynet_contracts::admission::PackageManifest,
        capacity: &Capacity,
        observed_digest: &str,
        signature: SignatureNote,
    ) -> Report {
        run(
            manifest,
            capacity,
            observed_digest,
            signature,
            &crate::embodiment::Body::Absent,
            false,
        )
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
        let report = ran(
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
            let report = ran(
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
        let verified = ran(
            &manifest(64.0, &[]),
            &capacity(1024.0, &[]),
            &"a".repeat(64),
            signed_by("acme-robotics"),
        );
        assert!(verified.driver_may_activate());

        let stranger = ran(
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
        let report = ran(
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
        let report = ran(
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
        let report = ran(
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
        let report = ran(
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
        let report = ran(
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
            ran(
                &manifest(64.0, &[]),
                &capacity(1024.0, &[]),
                &"a".repeat(64),
                signed_by("acme-robotics"),
            ),
            ran(
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

    // ── §37.1's `transport` stage ────────────────────────────────────────────

    /// A body with one joint, one role, and a rating.
    ///
    /// Written to a file and read back through [`crate::embodiment::read`] rather than constructed,
    /// so the test exercises the same decoder an install does — a descriptor built in memory would
    /// skip the reader that is half of what the stage depends on.
    fn arm(label: &str, role: &str, effort_nm: f64) -> crate::embodiment::Body {
        let document = format!(
            r#"{{"robot_slug":"bench-arm","base":"fixed","dof":1,
                 "control":{{"modes":["torque"],"rate_hz_max":100}},
                 "envelope":{{"joint_torque_max_nm":{effort_nm},"watchdog_ms":100,
                              "source":"https://example.invalid/arm.pdf"}},
                 "joints":[{{"name":"J1","role":"{role}","type":"revolute",
                             "control_modes":["torque"],
                             "limits":{{"effort_max_nm":{effort_nm}}}}}]}}"#
        );
        let root = crate::tests::TempRoot::new(label);
        let path = root.path().join("embodiment.json");
        std::fs::write(&path, document).unwrap();
        let body = crate::embodiment::read(&path);
        assert!(body.known().is_some(), "the fixture descriptor loads");
        body
    }

    /// A policy that commands one torque channel on one role, at a declared peak.
    fn policy_for(
        role: &str,
        peak_nm: f64,
        hz: Option<f64>,
    ) -> skynet_contracts::admission::PackageManifest {
        let source = Json::parse(&format!(
            r#"{{"control_mode":"torque","control_hz":50,
                 "action_channels":[{{"index":0,"kind":"joint_torque","role":"{role}",
                                      "unit":"Nm","peak":{peak_nm}}}]}}"#
        ))
        .expect("the source parses");
        let mut manifest = manifest(64.0, &[]);
        manifest.budget.control_hz = hz;
        manifest.source_embodiment =
            Some(skynet_contracts::json::source_from_json(&source).expect("decodes"));
        manifest
    }

    fn ran_on(
        manifest: &skynet_contracts::admission::PackageManifest,
        body: &crate::embodiment::Body,
        retarget: bool,
    ) -> Report {
        run(
            manifest,
            &capacity(1024.0, &[]),
            &"a".repeat(64),
            signed_by("acme-robotics"),
            body,
            retarget,
        )
    }

    /// The stage, running for the first time. A channel whose role the body has goes `direct`, and
    /// the verdict reaches admission — which had taken `None` on every robot since it was written.
    #[test]
    fn a_policy_that_fits_the_body_transports_direct_and_admission_is_told() {
        let report = ran_on(
            &policy_for("right_arm.elbow.flex", 40.0, None),
            &arm("transport-fit", "right_arm.elbow.flex", 80.0),
            false,
        );
        let Report::Ran { plan, result, .. } = &report else {
            panic!("{report:?}");
        };
        let plan = plan.as_ref().expect("a plan");
        assert_eq!(plan.verdict.as_str(), "direct");
        assert!(result.admitted, "{:?}", result.findings);
        assert_eq!(
            to_json(&report)
                .get("transport")
                .map(|t| t.get("verdict").cloned()),
            Some(Some(Json::from("direct"))),
            "and it is on the wire, where nothing was before"
        );
    }

    /// §6: *"One blocked required channel refuses the whole plan, because a policy missing an
    /// actuator it depends on does not do most of its job — it does something nobody has
    /// characterised."* This is that refusal reaching an install, which it never has.
    #[test]
    fn a_required_channel_this_body_has_no_equivalent_for_refuses_the_install() {
        let report = ran_on(
            &policy_for("left_leg.knee.flex", 40.0, None),
            &arm("transport-fit", "right_arm.elbow.flex", 80.0),
            false,
        );
        let Report::Ran { plan, result, .. } = &report else {
            panic!("{report:?}");
        };
        assert_eq!(plan.as_ref().expect("a plan").verdict.as_str(), "refused");
        assert!(!result.admitted);
        assert!(
            result
                .findings
                .iter()
                .any(|f| f.code == PackageRejectionCode::TransportRefused),
            "{:?}",
            result.findings
        );
        // §64: the refusal names the channel, not only the verdict. "The policy's action space
        // cannot be transported onto this body" is true and tells nobody which joint to look at.
        let lines = to_lines(&report).join("\n");
        assert!(lines.contains("left_leg.knee.flex"), "{lines}");
        assert!(lines.contains("blocked"), "{lines}");
    }

    /// The two ways there is nothing to plan, and neither is a pass.
    #[test]
    fn no_action_space_and_no_descriptor_both_produce_no_verdict() {
        // A driver, an environment, a dataset: nothing that commands a joint.
        let no_space = ran_on(
            &manifest(64.0, &[]),
            &arm("transport-fit", "right_arm.elbow.flex", 80.0),
            false,
        );
        assert!(matches!(&no_space, Report::Ran { plan: None, .. }));
        assert!(no_space.clean(), "{no_space:?}");

        // And a robot nobody has provisioned. The descriptor's absence is reported by the routes
        // that report descriptors; inventing a `refused` here would fail every install on it with
        // a sentence about the policy.
        let no_body = ran_on(
            &policy_for("right_arm.elbow.flex", 40.0, None),
            &crate::embodiment::Body::Absent,
            false,
        );
        assert!(matches!(&no_body, Report::Ran { plan: None, .. }));
        assert!(
            !no_body
                .errors()
                .iter()
                .any(|f| f.code == PackageRejectionCode::TransportRefused),
            "a missing body is not a refused transport"
        );
    }

    /// §6.1: retargeting is off unless somebody decided otherwise, and the decision is the
    /// operator's — a flag on the command they are standing at, not a field the publisher fills in.
    #[test]
    fn retargeting_is_off_until_the_operator_asks_for_it() {
        // A role this body does not have, on a chain it does: with no solver and no permission the
        // channel is blocked either way, and what changes is *which* refusal an operator reads.
        let policy = policy_for("right_arm.wrist.roll", 10.0, None);
        let body = arm("transport-body", "right_arm.elbow.flex", 80.0);

        let refused = ran_on(&policy, &body, false);
        let Report::Ran { plan, .. } = &refused else {
            panic!()
        };
        assert_eq!(plan.as_ref().unwrap().verdict.as_str(), "refused");

        // Asking for it does not manufacture a solver: this body declares none, so §6.1's "the
        // request is refused rather than promised and discovered at the first command" holds.
        let asked = ran_on(&policy, &body, true);
        let Report::Ran { plan, .. } = &asked else {
            panic!()
        };
        assert_eq!(
            plan.as_ref().unwrap().verdict.as_str(),
            "refused",
            "a body with no solver cannot retarget, however the flag is set"
        );
    }

    /// The rate the plan is made against is the one the loop will run at, not the one the policy
    /// was trained at. A publisher may declare both.
    #[test]
    fn the_plan_uses_the_rate_the_package_will_actually_run_at() {
        let body = arm("transport-body", "right_arm.elbow.flex", 80.0);
        // Trained at 50, and this package declares it will run its loop at 25.
        let report = ran_on(
            &policy_for("right_arm.elbow.flex", 40.0, Some(25.0)),
            &body,
            false,
        );
        let Report::Ran { plan, .. } = &report else {
            panic!()
        };
        assert_eq!(plan.as_ref().unwrap().control_hz, Some(25.0));
    }

    /// An untrusted report carries the detail, because the code alone does not say where to look.
    #[test]
    fn an_untrusted_signature_reports_where_to_look() {
        let report = ran(
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

//! The `.sky` package, and whether it may be installed.
//!
//! Android's second invention, after the vendor interface, was the APK: one artifact that declares
//! what it needs, is signed by whoever built it, and is admitted onto any conformant device by a
//! system that understands the declaration. Before that, software was ported to devices; after it,
//! software was *installed* on them. Robotics is still in the porting era — a policy arrives as a
//! git repository, a conda environment and a README, and getting it onto a second robot is a
//! project rather than a command.
//!
//! # Why the budget is mandatory
//!
//! A phone that runs out of memory kills an app. A robot that runs out of memory mid-stride falls
//! over. [`admit_package`] refuses a package the robot cannot feed rather than admitting it and
//! discovering the shortfall at 60 kg of angular momentum. This is the one place the format is
//! stricter than an APK, and the reason is physics.
//!
//! # Every finding, not the first
//!
//! [`admit_package`] returns *all* findings rather than stopping at the first error. Fixing four
//! problems one deployment at a time is precisely how people end up granting everything.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// What a package is.
pub const PACKAGE_KINDS: [&str; 5] = ["policy", "skill", "driver", "service", "embodiment"];

/// How a package is isolated.
///
/// `driver` is the only kind that touches hardware, and the only one that may be `native`. A
/// policy is always sandboxed, without exception and without an override flag, because the scanner
/// tells us most policies execute code on load and the whole safety argument rests on them being
/// unable to reach an actuator. An escape hatch here would quietly undo the invariant the Safety
/// Kernel is built to hold.
pub const ISOLATION_MODES: [&str; 2] = ["sandboxed", "native"];

/// What a package declares it needs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResourceBudget {
    /// Resident memory, megabytes. Becomes the cgroup `memory.max` on the robot (§40.1).
    pub ram_mb: f64,
    /// Accelerator memory, megabytes.
    pub vram_mb: Option<f64>,
    /// Sustained fraction of one core; 1.0 is a full core.
    pub cpu_cores: Option<f64>,
    /// Storage, megabytes.
    pub disk_mb: Option<f64>,
    /// Rate the package intends to run its loop at.
    pub control_hz: Option<f64>,
    /// Worst-case time from observation in to action out, in milliseconds.
    ///
    /// Declared, then *measured* by the conformance suite and compared. A package that overruns its
    /// own declaration on real hardware is one the OS can refuse before it is trusted with a loop.
    pub inference_budget_ms: Option<f64>,
}

/// What a package needs from the body.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbodimentRequirement {
    /// Capability tokens from the Standard Robot Interface the package needs.
    pub capabilities: Vec<String>,
    /// Vendor-specific extension tokens, declared rather than reached for silently.
    pub extensions: Vec<String>,
    /// The mode the package commands in.
    pub control_mode: String,
    /// Minimum degrees of freedom, when the policy genuinely needs them.
    pub min_dof: Option<f64>,
    /// Base kinds this package can drive. Empty means any.
    pub base_kinds: Vec<String>,
    /// Robot slugs the publisher has actually verified, not the ones they hope work.
    pub verified_on: Vec<String>,
}

/// A dependency, by id and semver range.
#[derive(Debug, Clone, PartialEq)]
pub struct Dependency {
    /// `namespace/name`.
    pub id: String,
    /// Semver range.
    pub range: String,
}

/// The loader contract from the Hub's load plan, carried so an offline robot need not re-derive it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuntimeSpec {
    /// Loader kind the Hub derived from the file headers.
    pub loader: Option<String>,
    /// Entry point.
    pub entrypoint: Option<String>,
    /// Configuration file.
    pub config: Option<String>,
    /// Container image, for the sandboxed path.
    pub container_image: Option<String>,
}

/// What installs onto a robot.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageManifest {
    /// Schema version. 1 today.
    pub schema_version: u32,
    /// `namespace/name`, matching the Hub repository it came from.
    pub id: String,
    /// Version string. A label; `digest` is the identity that matters.
    pub version: String,
    /// One of [`PACKAGE_KINDS`].
    pub kind: String,
    /// Version digest from the Hub.
    pub digest: String,
    /// One of [`ISOLATION_MODES`].
    pub isolation: String,
    /// What it needs to run.
    pub budget: ResourceBudget,
    /// What it needs from the body.
    pub embodiment: EmbodimentRequirement,
    /// Capability tokens the package asks the operator to grant.
    ///
    /// Always a subset of `embodiment.capabilities` — a package cannot request a permission for
    /// something it did not declare it needs, which stops "grant everything, use some" from being
    /// the path of least resistance.
    pub requests: Vec<String>,
    /// Loader contract.
    pub runtime: Option<RuntimeSpec>,
    /// Packages that must be installed first.
    pub depends_on: Vec<Dependency>,
    /// Minimum OS version.
    pub min_os_version: Option<String>,
}

/// Why a package was refused, or warned about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageRejectionCode {
    /// No publisher signature accompanied the package.
    Unsigned,
    /// A signature was present and did not verify.
    ///
    /// The one signature state that says something *went wrong* rather than something is missing.
    /// An operator who reads `unsigned` shrugs; one who reads this stops.
    SignatureFailed,
    /// A signature verified, and the key that made it is not one this robot trusts.
    ///
    /// Nothing is wrong with the bytes and nothing is wrong with the signature. What is missing is
    /// a *decision*, and it is the operator's: this robot has not been told that this signer may
    /// publish to it. Reporting it as `signature_failed` would send somebody hunting for tampering
    /// that did not happen, and reporting it as `verified` would make the trust store decorative.
    SignatureUntrusted,
    /// This platform cannot verify a signature at all.
    ///
    /// A gap in the operating system rather than in the package, and it is named separately so the
    /// remedy lands on the right desk.
    SignatureUnverifiable,
    /// The bytes do not hash to the digest in the manifest.
    DigestMismatch,
    /// A non-driver asked to run unsandboxed.
    IsolationViolation,
    /// The robot cannot supply the declared budget.
    BudgetExceeded,
    /// A capability is unprovided by the body, or awaiting an operator grant.
    CapabilityUngranted,
    /// The operator profile withholds a capability the package requires.
    CapabilityDenied,
    /// The package requests a permission it never declared a need for.
    OverRequest,
    /// A declared dependency is not installed.
    MissingDependency,
    /// The robot's OS predates the package's minimum.
    OsTooOld,
    /// The policy's action space cannot be transported onto this body.
    TransportRefused,
    /// Files execute code when read, and the operator has not accepted that.
    ExecutesOnLoadUntrusted,
}

impl PackageRejectionCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            PackageRejectionCode::Unsigned => "unsigned",
            PackageRejectionCode::SignatureFailed => "signature_failed",
            PackageRejectionCode::SignatureUntrusted => "signature_untrusted",
            PackageRejectionCode::SignatureUnverifiable => "signature_unverifiable",
            PackageRejectionCode::DigestMismatch => "digest_mismatch",
            PackageRejectionCode::IsolationViolation => "isolation_violation",
            PackageRejectionCode::BudgetExceeded => "budget_exceeded",
            PackageRejectionCode::CapabilityUngranted => "capability_ungranted",
            PackageRejectionCode::CapabilityDenied => "capability_denied",
            PackageRejectionCode::OverRequest => "over_request",
            PackageRejectionCode::MissingDependency => "missing_dependency",
            PackageRejectionCode::OsTooOld => "os_too_old",
            PackageRejectionCode::TransportRefused => "transport_refused",
            PackageRejectionCode::ExecutesOnLoadUntrusted => "executes_on_load_untrusted",
        }
    }
}

/// Whether a finding stops the install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Refuses the install.
    Error,
    /// Proceeds, and is shown to the operator.
    Warning,
}

impl Severity {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        }
    }
}

/// One thing wrong, or worth saying.
#[derive(Debug, Clone, PartialEq)]
pub struct PackageFinding {
    /// Why.
    pub code: PackageRejectionCode,
    /// Whether it stops the install.
    pub severity: Severity,
    /// English, for logs.
    pub message: String,
    /// The specific things named.
    pub subjects: Option<Vec<String>>,
}

/// An installed package, for dependency checking.
#[derive(Debug, Clone, PartialEq)]
pub struct InstalledPackage {
    /// `namespace/name`.
    pub id: String,
    /// Version string.
    pub version: String,
}

/// What the robot can supply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceCapacity {
    /// Resident memory, megabytes.
    pub ram_mb: f64,
    /// Accelerator memory, megabytes.
    pub vram_mb: Option<f64>,
    /// Cores.
    pub cpu_cores: Option<f64>,
    /// Storage, megabytes.
    pub disk_mb: Option<f64>,
    /// The OS version running.
    pub os_version: String,
    /// Capability tokens the robot plus its adapter actually provide.
    pub capabilities: Vec<String>,
    /// Vendor extension tokens available.
    pub extensions: Vec<String>,
    /// Grants the operator has made on this robot.
    pub granted: Vec<String>,
    /// Tokens the operator profile withholds outright, whatever a package requests.
    pub denied: Vec<String>,
    /// What is already installed.
    pub installed: Vec<InstalledPackage>,
}

/// What is known about a package's release signature.
///
/// Five states, because a boolean held two and the other three mattered. They have different owners
/// and different remedies, and collapsing them left an operator unable to tell whether to chase a
/// publisher, add a key, upgrade the OS, or stop and investigate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureState {
    /// Checked against a key the operator trusts.
    Verified,
    /// Present, and it did not verify. The bytes are attested to somebody other than the manifest
    /// claims, which is what tampering looks like.
    Failed,
    /// It verified, against a key this robot does not trust. The operator's job.
    ///
    /// The state a verifier discovers the moment there is a trust store to check against: a
    /// signature can be perfectly good and made by a stranger. It is not `Failed` — nothing went
    /// wrong — and it is not `Verified`, because §37.1 says *"against a key the operator trusts"*
    /// and this one was not.
    Untrusted,
    /// No signature accompanied the package. The publisher's job.
    Absent,
    /// This edition cannot check a signature at all. The platform's job.
    Unverifiable,
}

impl SignatureState {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SignatureState::Verified => "verified",
            SignatureState::Failed => "failed",
            SignatureState::Untrusted => "untrusted",
            SignatureState::Absent => "absent",
            SignatureState::Unverifiable => "unverifiable",
        }
    }

    /// Parse the wire spelling.
    ///
    /// No default. A document whose signature state does not parse is one nobody should guess
    /// about, and the caller decides what to do with `None` — which is the whole reason this type
    /// replaced a boolean that read a missing field as `false`.
    pub fn parse(text: &str) -> Option<SignatureState> {
        match text {
            "verified" => Some(SignatureState::Verified),
            "failed" => Some(SignatureState::Failed),
            "untrusted" => Some(SignatureState::Untrusted),
            "absent" => Some(SignatureState::Absent),
            "unverifiable" => Some(SignatureState::Unverifiable),
            _ => None,
        }
    }
}

/// Everything [`admit_package`] needs.
#[derive(Debug, Clone)]
pub struct AdmissionParams {
    /// The package.
    pub manifest: PackageManifest,
    /// The robot.
    pub capacity: DeviceCapacity,
    /// What is known about the release signature.
    pub signature: SignatureState,
    /// The digest the bytes on disk actually hash to.
    pub observed_digest: String,
    /// Verdict from the transport plan for this body, when one was resolved.
    pub transport_verdict: Option<String>,
    /// Whether any file in the package executes code when read.
    pub executes_on_load: bool,
    /// Whether the operator explicitly accepted that. Never defaulted true.
    pub trust_remote_code: bool,
}

/// The decision.
#[derive(Debug, Clone, PartialEq)]
pub struct AdmissionResult {
    /// Whether the install proceeds.
    pub admitted: bool,
    /// Everything found, in the order the checks run.
    pub findings: Vec<PackageFinding>,
    /// Capabilities that will be active: requested, provided by the body, and granted.
    pub effective_grants: Vec<String>,
}

/// Decide whether a package may be installed.
///
/// Pure, and shared: the Hub runs it to tell an operator what will happen before they deploy, and
/// the OS runs it at install time on the robot, offline. Neither is authoritative over the other
/// because both compute the same function over the same declared facts.
pub fn admit_package(params: &AdmissionParams) -> AdmissionResult {
    let manifest = &params.manifest;
    let capacity = &params.capacity;
    let mut findings: Vec<PackageFinding> = Vec::new();

    // Four of the five states are a finding, and they are four *different* findings, with four
    // different people at the end of them. `failed` is the one that says something went wrong
    // rather than something being missing; `untrusted` is the one where nothing is missing at all
    // except a decision nobody has made.
    match params.signature {
        SignatureState::Verified => {}
        SignatureState::Failed => findings.push(PackageFinding {
            code: PackageRejectionCode::SignatureFailed,
            severity: Severity::Error,
            message: "A release signature was present and did not verify. The bytes are not the ones the signer attested to — treat this as tampering until somebody proves otherwise.".to_string(),
            subjects: None,
        }),
        SignatureState::Untrusted => findings.push(PackageFinding {
            code: PackageRejectionCode::SignatureUntrusted,
            severity: Severity::Error,
            message: "A release signature verified, and the key that made it is not one this robot trusts. Nothing is wrong with the bytes; what is missing is the operator's decision that this signer may publish here.".to_string(),
            subjects: None,
        }),
        SignatureState::Absent => findings.push(PackageFinding {
            code: PackageRejectionCode::Unsigned,
            severity: Severity::Error,
            message: "No publisher signature accompanied this package. The bytes may be intact and nobody has attested to who produced them.".to_string(),
            subjects: None,
        }),
        SignatureState::Unverifiable => findings.push(PackageFinding {
            code: PackageRejectionCode::SignatureUnverifiable,
            severity: Severity::Error,
            message: "This platform cannot verify a release signature, so nothing has established who produced these bytes. That is a gap in the operating system rather than in the package.".to_string(),
            subjects: None,
        }),
    }

    if params.observed_digest != manifest.digest {
        findings.push(PackageFinding {
            code: PackageRejectionCode::DigestMismatch,
            severity: Severity::Error,
            message: "The package contents do not hash to the digest in its manifest.".to_string(),
            subjects: Some(alloc::vec![
                manifest.digest.clone(),
                params.observed_digest.clone()
            ]),
        });
    }

    if manifest.kind != "driver" && manifest.isolation == "native" {
        findings.push(PackageFinding {
            code: PackageRejectionCode::IsolationViolation,
            severity: Severity::Error,
            message: format!(
                "A {} package may not run unsandboxed. Only drivers touch hardware directly.",
                manifest.kind
            ),
            subjects: None,
        });
    }

    findings.extend(check_budget(&manifest.budget, capacity));

    // A package must not request more than it declared it needs.
    let over_requested: Vec<String> = manifest
        .requests
        .iter()
        .filter(|token| !manifest.embodiment.capabilities.contains(token))
        .cloned()
        .collect();
    if !over_requested.is_empty() {
        findings.push(PackageFinding {
            code: PackageRejectionCode::OverRequest,
            severity: Severity::Error,
            message: "The package requests permissions it never declared a need for.".to_string(),
            subjects: Some(over_requested),
        });
    }

    let explicitly_denied: Vec<String> = manifest
        .requests
        .iter()
        .filter(|token| capacity.denied.contains(token))
        .cloned()
        .collect();
    if !explicitly_denied.is_empty() {
        findings.push(PackageFinding {
            code: PackageRejectionCode::CapabilityDenied,
            severity: Severity::Error,
            message: "The operator profile withholds capabilities this package requires."
                .to_string(),
            subjects: Some(explicitly_denied),
        });
    }

    let provided = |token: &String| {
        capacity.capabilities.contains(token) || capacity.extensions.contains(token)
    };

    let unmet: Vec<String> = manifest
        .embodiment
        .capabilities
        .iter()
        .chain(manifest.embodiment.extensions.iter())
        .filter(|token| !provided(token))
        .cloned()
        .collect();
    if !unmet.is_empty() {
        findings.push(PackageFinding {
            code: PackageRejectionCode::CapabilityUngranted,
            severity: Severity::Error,
            message: "This robot does not provide capabilities the package requires.".to_string(),
            subjects: Some(unmet),
        });
    }

    let ungranted: Vec<String> = manifest
        .requests
        .iter()
        .filter(|token| {
            provided(token) && !capacity.granted.contains(token) && !capacity.denied.contains(token)
        })
        .cloned()
        .collect();
    if !ungranted.is_empty() {
        // A warning, not an error: this is the Android runtime-permission moment. The install
        // proceeds and the operator is prompted; the package simply cannot use the capability
        // until they say yes, and the mode gate enforces that at every cycle. Refusing the install
        // would train operators to pre-approve everything to make the error go away.
        findings.push(PackageFinding {
            code: PackageRejectionCode::CapabilityUngranted,
            severity: Severity::Warning,
            message: "These capabilities are awaiting an operator grant. The package installs and cannot use them until it is given.".to_string(),
            subjects: Some(ungranted),
        });
    }

    for dependency in &manifest.depends_on {
        if !capacity
            .installed
            .iter()
            .any(|installed| installed.id == dependency.id)
        {
            findings.push(PackageFinding {
                code: PackageRejectionCode::MissingDependency,
                severity: Severity::Error,
                message: format!(
                    "Requires {} {}, which is not installed.",
                    dependency.id, dependency.range
                ),
                subjects: Some(alloc::vec![dependency.id.clone()]),
            });
        }
    }

    if let Some(minimum) = &manifest.min_os_version {
        if compare_versions(&capacity.os_version, minimum) < 0 {
            findings.push(PackageFinding {
                code: PackageRejectionCode::OsTooOld,
                severity: Severity::Error,
                message: format!(
                    "Requires Skynet OS {} or later; this robot runs {}.",
                    minimum, capacity.os_version
                ),
                subjects: None,
            });
        }
    }

    if params.transport_verdict.as_deref() == Some("refused") {
        findings.push(PackageFinding {
            code: PackageRejectionCode::TransportRefused,
            severity: Severity::Error,
            message: "The policy's action space cannot be transported onto this body.".to_string(),
            subjects: None,
        });
    }

    if params.executes_on_load && !params.trust_remote_code {
        findings.push(PackageFinding {
            code: PackageRejectionCode::ExecutesOnLoadUntrusted,
            severity: Severity::Error,
            message: "This package contains files that execute code when read, and the operator has not accepted that. Install with trust_remote_code once you have decided the publisher is trustworthy.".to_string(),
            subjects: None,
        });
    }

    let admitted = !findings
        .iter()
        .any(|finding| finding.severity == Severity::Error);
    let effective_grants = if admitted {
        manifest
            .requests
            .iter()
            .filter(|token| provided(token) && capacity.granted.contains(token))
            .cloned()
            .collect()
    } else {
        Vec::new()
    };

    AdmissionResult {
        admitted,
        findings,
        effective_grants,
    }
}

fn check_budget(budget: &ResourceBudget, capacity: &DeviceCapacity) -> Vec<PackageFinding> {
    let mut shortfalls: Vec<String> = Vec::new();
    let n = crate::json::number_text;

    if budget.ram_mb > capacity.ram_mb {
        shortfalls.push(format!(
            "RAM: needs {} MB, has {} MB",
            n(budget.ram_mb),
            n(capacity.ram_mb)
        ));
    }
    if let Some(vram) = budget.vram_mb {
        let have = capacity.vram_mb.unwrap_or(0.0);
        if vram > have {
            shortfalls.push(format!("VRAM: needs {} MB, has {} MB", n(vram), n(have)));
        }
    }
    if let Some(disk) = budget.disk_mb {
        let have = capacity.disk_mb.unwrap_or(0.0);
        if disk > have {
            shortfalls.push(format!("Disk: needs {} MB, has {} MB", n(disk), n(have)));
        }
    }
    if let Some(cores) = budget.cpu_cores {
        let have = capacity.cpu_cores.unwrap_or(0.0);
        if cores > have {
            shortfalls.push(format!("CPU: needs {} cores, has {}", n(cores), n(have)));
        }
    }

    if shortfalls.is_empty() {
        return Vec::new();
    }
    alloc::vec![PackageFinding {
        code: PackageRejectionCode::BudgetExceeded,
        severity: Severity::Error,
        message: "This robot cannot supply the resources the package declares it needs."
            .to_string(),
        subjects: Some(shortfalls),
    }]
}

/// Numeric-segment comparison, sufficient for `major.minor.patch` OS versions.
///
/// A non-numeric segment reads as 0, matching the reference implementation's `Number()` coercion of
/// `NaN` — deliberately kept rather than "fixed", because a pre-release suffix comparing as 0 is a
/// behaviour the corpus pins and the two implementations must share.
fn compare_versions(a: &str, b: &str) -> i32 {
    let left: Vec<f64> = a
        .split('.')
        .map(|s| s.parse::<f64>().unwrap_or(0.0))
        .collect();
    let right: Vec<f64> = b
        .split('.')
        .map(|s| s.parse::<f64>().unwrap_or(0.0))
        .collect();
    let len = left.len().max(right.len());
    for i in 0..len {
        let diff = left.get(i).copied().unwrap_or(0.0) - right.get(i).copied().unwrap_or(0.0);
        if diff != 0.0 {
            return if diff < 0.0 { -1 } else { 1 };
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_numeric_segment() {
        assert_eq!(compare_versions("1.2.3", "1.2.3"), 0);
        assert_eq!(compare_versions("1.2.3", "1.10.0"), -1);
        assert_eq!(compare_versions("2.0", "1.99.99"), 1);
        assert_eq!(
            compare_versions("1.2", "1.2.0"),
            0,
            "absent segments are zero"
        );
    }
}

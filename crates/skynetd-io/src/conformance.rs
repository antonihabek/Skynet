//! §15's suite, run by the robot on itself.
//!
//! §73's conformance row has said the same thing for as long as it has existed:
//!
//! > No runner executes the suite on a robot, and nothing signs a result.
//!
//! This is the first half. `skynet driver check` runs the *driver-facing* sim tests against a
//! shared object and lists, by name, the eight it cannot cover — the ones that are properties of
//! the **runtime** rather than of a driver. Those eight are this module's subject, because the
//! runtime is what this daemon is.
//!
//! # The eight sentences, most of which had gone stale
//!
//! `NOT_COVERED` in `skynet-cli` explains each omission with a "needs X". Five of those sentences
//! were true when they were written and are not now:
//!
//! | Test | Said | Since |
//! |---|---|---|
//! | `package.signature.required` | needs the package installer | §37.1's installer, and five signature states |
//! | `package.grants.gate` | needs the permission model | grants, admission, `effective_grants` |
//! | `ota.safety_gate` | needs the updater | §12, and §75's bundle route |
//! | `telemetry.replay.deterministic` | needs the black box | ring, MCAP, sealing, replay |
//! | `security.credential.scope` | needs device tokens | `skd_` credentials and their scopes |
//! | `safety.policy.isolation` | needs a sandbox | §59's confinement, and §62's read-back |
//!
//! This is finding 49 at scale — *"which we do not" is a sentence to check, not to believe* — and
//! the value of a runner is as much in re-asking those questions as in answering them. Two of the
//! five turned out to be answerable **right now**, one is answerable and is not this machine's to
//! answer, and two needed something that did not exist.
//!
//! `safety.policy.isolation` is the sixth, and it is the one that took two rounds. §59 built the
//! confinement, which made the sentence *"needs a sandbox"* false — and left a different one true,
//! because what the suite could assert was the launcher's own argv. Asking `systemd-run` for seven
//! directives proves what was **requested**; §15 asks what a package is held to, and a directive
//! systemd rejected, dropped or does not support on this kernel produces exactly the same argv. So
//! it stayed skipped, with a reason naming the probe that would execute it, until the daemon could
//! read a running unit back.
//!
//! # A check that runs, or a reason. Never a pass by omission
//!
//! §15's first award rule is *"absence is not a pass"*, and it applies to the runner harder than to
//! anything it runs: a conformance runner that reported `pass` for a test it did not perform would
//! be the single most damaging component in this repository. So every entry here is one of:
//!
//! * a check this daemon **executed**, against its own installed code, with a real verdict;
//! * [`SctsOutcome::Skipped`], with a sentence naming what is missing and whose it is.
//!
//! # The uncomfortable result, which is the point
//!
//! `package.signature.required` runs, and this edition **fails it**. An unsigned package stages and
//! installs today — `crate::admit` says so in its own header — because nothing on a robot can
//! produce a signature and refusing every locally-built package would refuse the golden path.
//!
//! A runner that reported that as `skipped` would be hiding a known gap behind an unmeasured one.
//! It is a `fail`, it caps this robot's evidence below guest, and that is the honest reading: the
//! platform measuring itself and reporting that its own installer does not yet meet its own bar.

use skynet_contracts::admission::{
    admit_package, AdmissionParams, EmbodimentRequirement, PackageManifest, ResourceBudget,
    SignatureState,
};
use skynet_contracts::certification::{
    award_certification, CertificationInput, SctsOutcome, SctsResult,
};
use skynet_contracts::ota::{admit_bundle, BundleManifest, UpdateSubject};
use skynet_contracts::safety::{evaluate_cycle, CommandCycle, Enforcement, Guard};
use skynet_json::Json;

use skynet_blackbox::record::{CycleView, Entry, Shape, NO_CAPABILITY};
use skynet_blackbox::replay::replay_ring;
use skynet_blackbox::ring::{Geometry, Meta, Ring};

use crate::capacity::Capacity;
use crate::embodiment::Body;

/// The eight §15 leaves to the runtime, in the order they are reported.
///
/// Taken from `skynet-cli`'s `NOT_COVERED` rather than restated, in spirit: these are exactly the
/// tests that command lists as beyond it. Keeping the two lists the same length is the check that
/// nothing falls between them — a test neither command covers is a test nobody runs, and it would
/// be invisible from either side.
pub const RUNTIME_TESTS: [&str; 8] = [
    "safety.kernel.parity",
    "safety.policy.isolation",
    "package.signature.required",
    "package.grants.gate",
    "ota.safety_gate",
    "telemetry.replay.deterministic",
    "security.identity.attested",
    "security.credential.scope",
];

/// What this robot concluded about itself.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// One entry per [`RUNTIME_TESTS`], in that order.
    pub results: Vec<SctsResult>,
    /// How many were actually executed rather than reported as skipped.
    pub executed: usize,
}

impl Run {
    /// The tier this robot's **own** evidence supports, claiming the most it could.
    ///
    /// Claiming `certified` rather than the tier the robot thinks it is, on purpose: this asks
    /// "what does the evidence support", and the answer is only interesting if nothing capped it
    /// beforehand. §15's rule is that the evidence caps the award rather than the claim, so a low
    /// claim would produce a low award for a reason that has nothing to do with the machine.
    ///
    /// **Never a certificate.** This is what a robot can say about itself from what it ran, and it
    /// omits every bench and field result — which is most of the suite — so the answer is always at
    /// or below what a real award would be. §19 keeps it that way from the other end: a robot
    /// verifies and never signs, so nothing here is evidence anybody else has to accept.
    pub fn attainable(
        &self,
        simulator: bool,
    ) -> skynet_contracts::certification::CertificationVerdict {
        award_certification(&CertificationInput {
            claimed: "certified".to_string(),
            results: self.results.clone(),
            // Not `Some(1.0)`. No fleet data exists on the robot that produced it, and §15's floor
            // is checked against this — a runner substituting a flattering 1.0 would be awarding on
            // evidence nobody collected, which is the sentence `conformance_of` is written around.
            field_conformance: None,
            bench_age_days: None,
            simulator,
        })
    }
}

/// A result that was measured.
fn ran(id: &str, outcome: SctsOutcome, at: &str, detail: impl Into<String>) -> SctsResult {
    SctsResult {
        test_id: id.to_string(),
        outcome,
        value: None,
        ran_at: at.to_string(),
        detail: Some(detail.into()),
    }
}

/// A result that was not, and why.
///
/// `ran_at` is **empty**, and that is not a formality: §15 decays an award from the age of the
/// oldest bench result, and a skipped test carrying today's date would be a gap that looked fresh.
fn skipped(id: &str, detail: impl Into<String>) -> SctsResult {
    SctsResult {
        test_id: id.to_string(),
        outcome: SctsOutcome::Skipped,
        value: None,
        ran_at: String::new(),
        detail: Some(detail.into()),
    }
}

/// Run the eight.
///
/// `at` is an ISO-8601 timestamp from §35.1's correlation clock — the one thing wall time is for
/// here, because a result is a record a Hub places against other robots' records. Nothing in this
/// module decides anything with it.
pub fn run(
    capacity: &Capacity,
    body: &Body,
    limits: &crate::limits::Owned,
    grants: &[String],
    isolation: Option<&Confinement>,
    at: &str,
) -> Run {
    let mut results = Vec::with_capacity(RUNTIME_TESTS.len());

    results.push(skipped(
        "safety.kernel.parity",
        "an edition property rather than a machine one: it asks whether this implementation \
         reproduces §31.4's conformance corpus, which `cargo test -p skynet-corpus` measures at \
         build time. A robot cannot run it, and a robot reporting it would be reporting on its \
         compiler.",
    ));

    results.push(policy_isolation(isolation, at));

    results.push(signature_required(capacity, at));
    results.push(grants_gate(body, limits, grants, at));

    results.push(ota_safety_gate(at));

    results.push(replay_deterministic(body, limits, at));

    results.push(skipped(
        "security.identity.attested",
        "the descriptor's digest and the running image are reported at every check-in (§10), and \
         what `attested` asks for is a key bound to a TPM or a secure element. No key is generated \
         on a robot on this edition, so there is nothing to bind.",
    ));

    results.push(skipped(
        "security.credential.scope",
        "answerable, and not by this machine. A credential lifted from this robot cannot read \
         repositories or write to another namespace *if the Hub enforces the scopes on it*, and \
         only the Hub can measure that. This robot can say which scopes its credential carries — \
         `GET /identity` does — and a robot reporting a pass here would be reporting on somebody \
         else's enforcement.",
    ));

    let executed = results
        .iter()
        .filter(|result| result.outcome != SctsOutcome::Skipped)
        .count();
    Run { results, executed }
}

/// `package.signature.required` — *"An unsigned package, and one whose bytes do not match its
/// digest, are both refused."*
///
/// Run against this robot's **real** capacity rather than a synthetic one, so the answer is about
/// this machine. Two synthetic manifests, because the test names two refusals and a runner that
/// checked one would be reporting a pass it had half measured.
///
/// A robot whose capacity could not be assembled reports `skipped`: admission over an invented
/// capacity would be a conformance result about a robot that does not exist.
/// What this robot could observe about one running unit's confinement (§8.3, §59).
///
/// Assembled by the caller rather than here, because reading it is I/O and `run` is a function
/// over facts. `Err` carries the sentence the skip will show — the reason nothing was read is as
/// much a conformance result as the reading would have been.
pub struct Confinement {
    /// The package whose unit was read.
    pub package: String,
    /// What kind it is, which decides which directives were asked for (§8.3: a driver is native).
    pub kind: skynetd::desired::PackageKind,
    /// `Ok` with what systemd reported, `Err` with why nothing could be.
    pub observed: Result<Vec<(String, String)>, String>,
    /// `Ok` with what the **kernel** applied to the unit's process, `Err` with why nothing could be.
    ///
    /// A separate result from `observed` rather than merged into it, because the two can fail
    /// independently and for reasons an operator has to tell apart: systemd not describing the
    /// unit is a manager problem, and `/proc` not carrying the process is a unit that stopped.
    pub applied: Result<Vec<(String, String)>, String>,
}

/// §15's `safety.policy.isolation`, executed against a unit systemd is actually running.
///
/// # Why this could not be answered from the launcher's own argv
///
/// §59 built the confinement and the suite asserted it at the argv layer: `command_line` puts
/// seven `--property=` flags on `systemd-run` for every non-driver package, and a test read them
/// back. That proves what was **requested**. §15 asks what a sandboxed package is actually held
/// to, and those are different questions in every way that matters — a directive systemd rejected,
/// dropped or does not support on this kernel produces exactly the same argv.
///
/// It is also why answering from argv would have been the wrong kind of result. A check whose
/// subject is this repository's own code rather than this machine's state is an **edition**
/// property, which is precisely what `safety.kernel.parity` above is skipped for being. A robot
/// reporting one would be reporting on its compiler.
///
/// # What a pass means, and the three ways there is nothing to say
///
/// Pass: every directive `confinement_properties` asks for this kind is present on the running
/// unit, compared as a whole `Key=Value` line. `CapabilityBoundingSet=` is the one that makes the
/// line-exactness load-bearing rather than tidy — matching the key alone would pass a unit holding
/// `CapabilityBoundingSet=cap_sys_admin`, which is the failure the directive exists to prevent.
///
/// Fail: the unit is running and a directive did not land, named. A property systemd did not
/// report counts as not landed, which is the fail-closed direction: a sandbox nobody can read is
/// not a sandbox anybody should certify.
///
/// Skipped, in three cases, each naming what is missing:
///
/// * **no non-driver unit is running** — there is no sandbox on this machine to read. A robot
///   running only drivers is correctly outside this test rather than passing it, because §8.3
///   makes a driver native by design;
/// * **the launcher cannot read units** — `--dry-run`, or a machine with no systemd;
/// * **systemd would not describe the unit** — it stopped between the two calls, most likely.
///
/// # The third rung: what the kernel applied
///
/// `systemctl show` answers what the **manager recorded**, which is one rung above what the process
/// is actually held to. A directive the manager accepted, stored and reports happily can fail to be
/// applied by the kernel underneath it, and every layer above would go on saying it landed.
///
/// So the check now reads `/proc/<MainPID>/status` on the unit's own process as well, and compares
/// the two facts the kernel exposes per-process — `NoNewPrivs` and the capability bounding set —
/// against what the unit claims. **A disagreement between the layers is its own failure**, and it
/// is the most interesting one this test can produce: a robot whose service manager and whose
/// kernel do not agree about a sandbox is a robot whose sandbox nobody can describe.
///
/// Two rather than seven, and [`crate::launch::KERNEL_CONFINEMENT`] says why: the five `Protect*`
/// directives are mount namespaces, and what a namespace contains is not a line in `status`.
/// Checking two and saying two is better than checking two and implying seven.
///
/// # What it still does not establish
///
/// That a sandboxed package which *attempted* the forbidden hold was refused. The kernel reading
/// above is the strongest **observation** available — it is the kernel's own answer rather than a
/// record of a request — but an observation is not an attempt. A probe performing the hold and
/// being stopped is the demonstration, and it needs a package built to try, plus a manager running
/// as root: on a rootless user manager most of these directives forbid something the user could not
/// do anyway, so a refusal would prove nothing about the sandbox.
fn policy_isolation(isolation: Option<&Confinement>, at: &str) -> SctsResult {
    let id = "safety.policy.isolation";

    let Some(confinement) = isolation else {
        return skipped(
            id,
            "no sandboxed unit is running on this robot, so there is no confinement to read. A \
             driver is native by design (§8.3) and is not the subject of this test; start a \
             policy, service or skill package and this runs against the unit systemd gives it.",
        );
    };

    let observed = match &confinement.observed {
        Ok(observed) => observed,
        Err(detail) => {
            return skipped(
                id,
                format!(
                    "{} is running and its confinement could not be read: {detail}",
                    confinement.package
                ),
            )
        }
    };

    let shortfall = crate::launch::confinement_shortfall(confinement.kind, observed);
    if shortfall.is_empty() {
        // The manager agrees. Now ask the kernel, which is the rung below and the only one that is
        // a fact about the running process rather than about somebody's record of it.
        let applied = match &confinement.applied {
            Ok(applied) => applied,
            Err(detail) => {
                return skipped(
                    id,
                    format!(
                        "every directive §8.3 asks of a {} is on the unit systemd is running for \
                         {}, and what the kernel applied to its process could not be read: \
                         {detail}. A sandbox confirmed by its manager and unread by anybody else \
                         is not one this robot will certify.",
                        confinement.kind.as_str(),
                        confinement.package
                    ),
                )
            }
        };
        let contradicted = crate::launch::kernel_shortfall(confinement.kind, applied);
        if !contradicted.is_empty() {
            return ran(
                id,
                SctsOutcome::Fail,
                at,
                format!(
                    "systemd reports every directive §8.3 asks of a {} on {}'s unit, and the \
                     kernel does not agree: {}. The two layers describe the same process, so one \
                     of them is wrong about it — and a sandbox whose manager and whose kernel \
                     disagree is a sandbox nobody can describe.",
                    confinement.kind.as_str(),
                    confinement.package,
                    contradicted.join("; "),
                ),
            );
        }
        return ran(
            id,
            SctsOutcome::Pass,
            at,
            format!(
                "every directive §8.3 asks of a {} landed on the unit systemd is running for {} — \
                 read back with `systemctl show`, compared line-exactly, not taken from the argv \
                 that asked for them — and the kernel agrees about the two it exposes per process: \
                 the running process has NoNewPrivs set and an empty capability bounding set, read \
                 from /proc rather than from the manager's record of what it was asked for.",
                confinement.kind.as_str(),
                confinement.package
            ),
        );
    }

    ran(
        id,
        SctsOutcome::Fail,
        at,
        format!(
            "{} is running without {}. The launcher asked for {} on every {}; systemd is not \
             reporting {} of them, so this robot cannot show that its sandbox is one.",
            confinement.package,
            shortfall.join(", "),
            crate::launch::confinement_properties(confinement.kind).len(),
            confinement.kind.as_str(),
            shortfall.len(),
        ),
    )
}

fn signature_required(capacity: &Capacity, at: &str) -> SctsResult {
    let id = "package.signature.required";
    let Some(capacity) = capacity.known() else {
        return skipped(
            id,
            "this robot's capacity could not be assembled, so admission cannot be run against it. \
             Running it against an invented capacity would produce a conformance result about a \
             machine that does not exist (`GET /capacity` says what is missing).",
        );
    };

    let digest = "a".repeat(64);
    let manifest = PackageManifest {
        schema_version: 1,
        id: "skynet/conformance-probe".to_string(),
        version: "0.0.0".to_string(),
        kind: "policy".to_string(),
        digest: format!("sha256:{digest}"),
        isolation: "sandboxed".to_string(),
        budget: ResourceBudget::default(),
        embodiment: EmbodimentRequirement::default(),
        requests: Vec::new(),
        runtime: None,
        depends_on: Vec::new(),
        min_os_version: None,
        source_embodiment: None,
        required_capabilities: Vec::new(),
        required_extensions: Vec::new(),
    };

    let unsigned = admit_package(&AdmissionParams {
        manifest: manifest.clone(),
        capacity: capacity.clone(),
        signature: SignatureState::Absent,
        observed_digest: format!("sha256:{digest}"),
        transport_verdict: None,
        executes_on_load: false,
        trust_remote_code: false,
    });
    let mismatched = admit_package(&AdmissionParams {
        manifest,
        capacity: capacity.clone(),
        signature: SignatureState::Verified,
        // The bytes hash to something else, which is the second half of the test.
        observed_digest: format!("sha256:{}", "b".repeat(64)),
        transport_verdict: None,
        executes_on_load: false,
        trust_remote_code: false,
    });

    // Both halves have to refuse, and `admit_package` refuses both. The gap is downstream of it:
    // §37.1 puts admission between `verify` and `stage`, and on this edition it **reports** there
    // rather than gating, for every kind but a driver.
    if !unsigned.admitted && !mismatched.admitted {
        return ran(
            id,
            SctsOutcome::Fail,
            at,
            "admission refuses both — an unsigned package and one whose bytes do not match its \
             digest — and the install proceeds anyway. §37.1's `admit` stage reports on this \
             edition rather than gating, for every package kind but a driver (§19), because \
             nothing on a robot can produce a signature and refusing every locally-built package \
             would refuse the golden path. The decision is correct and the enforcement is absent, \
             and this test measures the enforcement.",
        );
    }

    ran(
        id,
        SctsOutcome::Fail,
        at,
        format!(
            "admission did not refuse both halves: unsigned admitted={}, digest-mismatched \
             admitted={}. Both must be refused (§15).",
            unsigned.admitted, mismatched.admitted
        ),
    )
}

/// `ota.safety_gate` — *"An update changing the safety kernel or the envelope is refused without a
/// fresh bench pass. OTA must not be a route around certification."*
///
/// The one test in this list this edition passes outright, and it is worth saying why it is a real
/// check rather than a restatement: `admit_bundle` is the function §12 gates on, this runs the
/// exact function the updater runs, over a bundle built to be the case the test names, against a
/// subject holding no bench pass. A pass here is *this robot's installed updater refusing it*.
fn ota_safety_gate(at: &str) -> SctsResult {
    let id = "ota.safety_gate";
    let bundle = BundleManifest {
        version: "9.9.9".to_string(),
        channel: "stable".to_string(),
        digest: format!("sha256:{}", "c".repeat(64)),
        size: None,
        compatible: Vec::new(),
        // The two components §12 gates on, together, because the test names both.
        changes: alloc_changes(),
    };
    let verdict = admit_bundle(
        &bundle,
        &UpdateSubject {
            running_version: "0.0.0".to_string(),
            board: None,
            channel: "stable".to_string(),
            // Verified, so the refusal that follows is about the gate and not about the signature.
            // A probe that failed for two reasons would measure neither.
            signature: SignatureState::Verified,
            bench: None,
        },
    );

    if verdict.admitted {
        return ran(
            id,
            SctsOutcome::Fail,
            at,
            "this robot's updater admitted an image changing the safety kernel and the envelope \
             with no bench pass behind it. OTA is then a route around certification (§12).",
        );
    }
    ran(
        id,
        SctsOutcome::Pass,
        at,
        format!(
            "an image changing the safety kernel and the envelope, with no bench pass, is refused \
             by the same `admit_bundle` the updater runs: {}",
            verdict
                .faults
                .iter()
                .map(|fault| fault.code.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

/// `telemetry.replay.deterministic` — *"given the same policy digest, observation stream and
/// embodiment digest, replay reproduces the same actions."*
///
/// # What this can honestly check, and what it cannot
///
/// Not that a neural network reproduces its output on a different accelerator — `skynet-blackbox`
/// says in its own header that it cannot make that claim, and neither can this. What replay
/// verifies is the deterministic half of the cycle: **given the recorded desired values, the
/// recorded timing and the same resolved limits, does this edition's Safety Kernel reach the
/// recorded verdict?**
///
/// # The window is synthesised, and the limits are not
///
/// This robot's ring holds audit events and notes and **no control cycles**, because nothing on
/// this edition drives a control loop. So the runner records a window itself, exactly as
/// `signature_required` builds two manifests and `ota_safety_gate` builds a bundle: synthetic
/// documents, real decision functions, this robot's real state.
///
/// What is real here is everything that decides the answer — this body's channel names out of its
/// descriptor, `crate::limits`' resolved set out of its envelope, its signed profile and its cell,
/// the same `evaluate_cycle` a control loop would call, the same ring the recorder writes, and the
/// same `replay()` an incident review runs. What is synthetic is only the four commands, and they
/// are chosen to exercise the fields a round trip can lose: a trip, a clamp, an advisory
/// forwarding, and a jump large enough for the rate limit to see.
///
/// So it is a real check with a stated reach: it catches the regression its own module names —
/// *"an OS upgrade that quietly altered a clamp"* — and a record format that stops carrying
/// something a verdict depends on. It does not and cannot say that a recording made a week ago by
/// a policy replays, because no policy has ever run here.
///
/// # A body with no joints is skipped rather than invented
///
/// The same rule `signature_required` applies to an unknown capacity. Replay indexes commands by
/// the header's channel names, and a window recorded over invented ones would be a conformance
/// result about a machine that does not exist.
fn replay_deterministic(body: &Body, limits: &crate::limits::Owned, at: &str) -> SctsResult {
    let id = "telemetry.replay.deterministic";
    let Some(known) = body.known() else {
        return skipped(
            id,
            "this robot has no usable embodiment descriptor, so it has no channel names to record \
             a command on. Replay indexes a recorded command by the header's channels, and a \
             window recorded over invented ones would be a conformance result about a machine that \
             does not exist (`GET /embodiment`).",
        );
    };

    // Sorted, because `CommandCycle::desired` is documented as sorted by name and a descriptor
    // lists joints in whatever order its author wrote them.
    let mut channels: Vec<String> = known
        .descriptor
        .joints
        .iter()
        .map(|joint| joint.name.clone())
        .collect();
    channels.sort();
    channels.dedup();
    if channels.is_empty() {
        return skipped(
            id,
            "this robot's descriptor declares no joints, so there is no action space to record a \
             command over. A body with nothing to command is not a body a replay can say anything \
             about.",
        );
    }
    if channels.len() > skynet_contracts::safety::MAX_CHANNELS {
        return skipped(
            id,
            "this robot declares more joints than the kernel evaluates channels for \
             (`MAX_CHANNELS`), so a window over all of them could not be evaluated. Recording a \
             subset would be a replay of part of a robot reported as a replay of the robot.",
        );
    }

    limits.with_resolved(|resolved, envelope| {
        let count = channels.len();
        let shape = Shape {
            joints: count as u16,
            channels: count as u16,
        };
        let geometry = Geometry {
            shape,
            record_size: shape.record_size(),
            capacity: WINDOW.len(),
            header_len: 4096,
            digests: 1,
            capabilities: 1,
        };
        let (mut ring, mut store) = Ring::create(
            geometry,
            Meta {
                // The real identity and the real body digest. A recording is *of* a robot, and the
                // two fields that say which are the two this daemon can actually fill (§26).
                robot_id: known.descriptor.robot_slug.clone(),
                embodiment: body.digest_or_zero(),
                os_image: [0u8; 32],
                tier: resolved.tier as u8,
                enforcement: resolved.enforcement as u8,
                simulator: known.descriptor.simulator,
                wall_epoch_ns: 0,
                mono_ns: 0,
                channels: channels.clone(),
            },
        );

        let names: Vec<&str> = channels.iter().map(String::as_str).collect();
        let mut previous: Option<Vec<f64>> = None;
        let mut last_sequence: Option<u64> = None;
        let mut exercised: Vec<&'static str> = Vec::new();

        for (index, step) in WINDOW.iter().enumerate() {
            let tick = index as u64 + 1;
            // A second a cycle, so a command can be stamped half a second ago without the
            // arithmetic going negative on the first one.
            let now_ms = tick as f64 * 1000.0;
            let command_ms = now_ms - step.age_ms;
            // Quantised to `f32` before the kernel ever sees it, and this line is load-bearing.
            //
            // §25's command ring carries `f32` values and §26 records `desired` as `f32`, so on a
            // robot every desired value the kernel is handed is already exactly representable in
            // `f32` — which is what makes an unclamped command survive the recording bit for bit.
            // A window built from `f64` values that are not (0.2 is not) records a command computed
            // from one number and replays it against another, and `replay()` compares commands by
            // bit equality. The first run of this check diverged on exactly that, and the answer
            // was not to loosen the comparison: a desired value §25's wire cannot carry is a value
            // no policy on this robot could ever have produced, so a runner using one would be
            // reporting a failure about arithmetic rather than about the machine.
            let asked = f64::from(step.value as f32);
            let desired: Vec<(&str, f64)> = names.iter().map(|name| (*name, asked)).collect();
            let carried: Option<Vec<(&str, f64)>> = previous.as_ref().map(|held| {
                names
                    .iter()
                    .zip(held.iter())
                    .map(|(name, value)| (*name, *value))
                    .collect()
            });

            let verdict = evaluate_cycle(
                &CommandCycle {
                    now_ms,
                    command_ms,
                    sequence: tick,
                    desired: &desired,
                    previous: carried.as_deref(),
                    capability: None,
                    nearest_person_m: None,
                },
                resolved,
                envelope,
                last_sequence,
            );

            for guard in verdict.tripped.iter() {
                if !exercised.contains(&guard.as_str()) {
                    exercised.push(guard.as_str());
                }
            }

            let command: Vec<f64> = names
                .iter()
                .map(|name| {
                    verdict
                        .command
                        .iter()
                        .find(|(channel, _)| channel == name)
                        .map(|(_, value)| *value)
                        .unwrap_or(0.0)
                })
                .collect();
            let clamped: Vec<bool> = names
                .iter()
                .map(|name| verdict.clamped.iter().any(|entry| entry.channel == *name))
                .collect();
            let tripped: Vec<Guard> = verdict.tripped.iter().copied().collect();
            let desired_f32: Vec<f32> = vec![asked as f32; count];
            let zeros_f32: Vec<f32> = vec![0.0; count];
            let mask: Vec<u8> = vec![1; count];

            let view = CycleView {
                tick,
                observation: [0u8; 32],
                policy: 0,
                state_flags: 0,
                fault_code: 0,
                read_result: 0,
                rhal_result: 0,
                state_seq: tick as u32,
                state_stamp_ns: (now_ms * 1.0e6) as u64,
                command_stamp_ns: (command_ms * 1.0e6) as u64,
                command_seq: tick as u32,
                mode: 0,
                advisory: verdict.advisory,
                response: verdict.response,
                tripped: &tripped,
                // Nothing supplied one, which is not the same as "no capability" — and the mode
                // gate treats the two differently, so the round trip has to carry the difference.
                capability: NO_CAPABILITY,
                nearest_person_m: f32::NAN,
                base_pose: [f32::NAN; 7],
                base_twist: [0.0; 6],
                desired: &desired_f32,
                command: &command,
                mask: &mask,
                clamped: &clamped,
                position: &zeros_f32,
                velocity: &zeros_f32,
                effort: &zeros_f32,
            };
            if let Err(error) =
                ring.append(&mut store, (now_ms * 1.0e6) as u64, Entry::Cycle(&view))
            {
                return ran(
                    id,
                    SctsOutcome::Fail,
                    at,
                    format!(
                        "this robot's black box refused to record a cycle it had just evaluated: \
                         {error:?}. A recording that cannot be written is a recording no incident \
                         review can read (§13)."
                    ),
                );
            }

            previous = Some(command);
            last_sequence = Some(tick);
        }

        let result = replay_ring(&ring, &store, resolved, envelope);

        // Said plainly rather than hidden, and said *precisely*. A pass over a window in which no
        // guard had anything to say is a weaker claim than one in which several did, and an
        // operator reading the one word both produce would take the first for more evidence than
        // it is. The reason is the interesting half: the third cycle asks for a thousand times any
        // plausible ceiling and is forwarded unchanged, because a joint's ceiling comes from
        // §6.3's transport plan and no plan is in force on a robot running no policy (§73).
        let exercised_note = if !exercised.is_empty() {
            format!("guards exercised: {}", exercised.join(", "))
        } else if resolved.per_channel.is_empty() {
            "no guard tripped anywhere in the window, and the reason is §73's transport row: a \
             joint's ceiling comes from the transport plan (§6.3) and no plan is in force on a \
             robot running no policy, so the `envelope` guard is active with nothing to clamp \
             against. The round trip was exercised and the guard verdicts were not"
                .to_string()
        } else {
            "no guard tripped anywhere in the window: this robot's resolved limits are wide enough \
             that nothing in it was refused. The round trip was exercised and the guard verdicts \
             were not"
                .to_string()
        };

        // §26.4's other half. A window can reproduce perfectly and still be built on cycles where
        // nothing read the body — a failed `read_state` leaves the previous cycle's positions in
        // the record, so the replay shows a robot holding still. Determinism is not the question
        // that answers, and a pass that reported the first without the second would hand an
        // operator a green tick over a window nobody measured.
        let observation_note = result
            .observation_note()
            .map(|note| format!(" {note}"))
            .unwrap_or_default();

        if result.deterministic() {
            return ran(
                id,
                SctsOutcome::Pass,
                at,
                format!(
                    "{} cycles recorded through this robot's own ring against its resolved limits \
                     and replayed by the same `replay()` an incident review runs; all {} \
                     reproduced, {} of them with a predecessor in the window so the rate limit was \
                     evaluated as it was. {exercised_note}. The window is synthetic and the limits \
                     are this machine's: nothing on this edition drives a control loop, so there \
                     is no recorded policy to replay.{observation_note}",
                    result.cycles, result.agreed, result.warmed,
                ),
            );
        }

        let mut detail = format!(
            "{} of {} cycles did not reproduce against the limits they were recorded under. This \
             edition's Safety Kernel and its own recording disagree, which is either a kernel that \
             changed behaviour or a record format that has stopped carrying something a verdict \
             depends on (§31.4).",
            result.diverged.len(),
            result.cycles,
        );
        detail.push_str(&observation_note);
        for outcome in result.diverged.iter().take(3) {
            for divergence in &outcome.divergences {
                detail.push_str(&format!(
                    " tick {}: {} recorded {}, recomputed {}.",
                    outcome.tick, divergence.field, divergence.recorded, divergence.recomputed
                ));
            }
        }
        for skip in result.skipped.iter().take(3) {
            detail.push_str(&format!(
                " cycle {} could not be evaluated: {}.",
                skip.sequence, skip.reason
            ));
        }
        ran(id, SctsOutcome::Fail, at, detail)
    })
}

/// One cycle of the replay window.
struct Asked {
    /// The desired value, on every channel, in the body's own units.
    value: f64,
    /// How long before this cycle the command was stamped, in milliseconds.
    age_ms: f64,
}

/// The window the runner records, and why it is this shape.
///
/// A window of constant fresh values would round-trip perfectly while exercising one code path, so
/// each row is here for a field the round trip could lose:
///
/// | Cycle | For |
/// |---|---|
/// | an ordinary command | the baseline, with no predecessor — the unwarmed cycle |
/// | a second ordinary command | the first cycle whose rate limit is evaluated as it was |
/// | far above any plausible ceiling | whatever clamp this body declares: a reduction at the host tier, an advisory forwarding at guest |
/// | a command stamped long ago | the watchdog, which short-circuits and emits **no** command — a different record from a command of zero |
/// | back to nothing | recovery after a trip, and the largest single-cycle change a jerk ceiling could object to |
///
/// Every value is exactly representable in `f32`; see the quantisation in [`replay_deterministic`]
/// for why that is not a coincidence.
const WINDOW: [Asked; 5] = [
    Asked {
        value: 0.25,
        age_ms: 0.0,
    },
    Asked {
        value: 0.5,
        age_ms: 0.0,
    },
    Asked {
        value: 1024.0,
        age_ms: 0.0,
    },
    // Older than any watchdog window a body would declare — §46 measures them in milliseconds.
    Asked {
        value: 0.5,
        age_ms: 500.0,
    },
    Asked {
        value: 0.0,
        age_ms: 0.0,
    },
];

/// The two components §12 gates on.
fn alloc_changes() -> Vec<String> {
    vec!["safety_kernel".to_string(), "envelope".to_string()]
}

/// The run, for the API and for a terminal.
pub fn to_json(run: &Run, simulator: bool) -> Json {
    let verdict = run.attainable(simulator);

    let mut results = Json::arr();
    for result in &run.results {
        let mut row = Json::obj();
        row.set("test_id", Json::from(result.test_id.as_str()))
            .set("outcome", Json::from(result.outcome.as_str()))
            .set(
                "detail",
                match &result.detail {
                    Some(detail) => Json::from(detail.as_str()),
                    None => Json::Null,
                },
            )
            .set(
                "ran_at",
                if result.ran_at.is_empty() {
                    Json::Null
                } else {
                    Json::from(result.ran_at.as_str())
                },
            );
        results.push(row);
    }

    let mut out = Json::obj();
    out.set("results", results)
        .set("executed", Json::Num(run.executed as f64))
        .set("of", Json::Num(RUNTIME_TESTS.len() as f64))
        .set("attainable", Json::from(verdict.awarded.as_str()))
        .set(
            "failures",
            Json::Arr(
                verdict
                    .failures
                    .iter()
                    .map(|id| Json::from(id.as_str()))
                    .collect(),
            ),
        )
        .set(
            "missing",
            Json::Arr(
                verdict
                    .missing
                    .iter()
                    .map(|id| Json::from(id.as_str()))
                    .collect(),
            ),
        )
        .set(
            "reasons",
            Json::Arr(
                verdict
                    .reasons
                    .iter()
                    .map(|reason| Json::from(reason.as_str()))
                    .collect(),
            ),
        )
        // Said on every run. §19 is the reason and it is not a limitation of this edition: a robot
        // verifies and never signs, so a result produced here is a robot's report about itself and
        // not a certificate. The Hub is what awards a tier, from bench evidence a human witnessed
        // (§15) — and this run carries none of that, which is why `attainable` is at or below what
        // a real award would be, always.
        .set(
            "note",
            Json::from(
                "These are the eight §15 tests that are properties of the runtime rather than of a \
                 driver — the ones `skynet driver check` lists as not covered. A result here is \
                 this robot reporting on itself: it is not signed, it is not a certificate, and it \
                 carries no bench or field evidence. §19: a robot verifies and never signs.",
            ),
        );
    out
}

/// The run, for a terminal.
pub fn to_lines(run: &Run, simulator: bool) -> Vec<String> {
    let verdict = run.attainable(simulator);
    let mut lines = Vec::new();
    lines.push(format!(
        "conformance: {} of {} runtime tests executed; the evidence here supports `{}`",
        run.executed,
        RUNTIME_TESTS.len(),
        verdict.awarded
    ));
    for result in &run.results {
        lines.push(format!(
            "  {:<32} {}",
            result.test_id,
            result.outcome.as_str()
        ));
        if let Some(detail) = &result.detail {
            lines.push(format!("      {detail}"));
        }
    }
    lines
}

/// `package.grants.gate` — the mode gate, against this robot's own grants.
///
/// # Why this stopped being skipped
///
/// It was skipped for eleven sessions with a reason that was true when it was written:
///
/// > half of this is built and it is not the half the test measures. Admission refuses a package
/// > requesting a capability the operator did not grant, at install (§8). The test is about a
/// > package that *commands* one at run time and the `mode_gate` guard refusing it — which is the
/// > Safety Kernel's, through a path §33.1 denies this daemon.
///
/// The last clause named a path that did not exist. `skynet-kernel-host` is that path: a process
/// that reads §25's ring, calls `evaluate_cycle`, and writes only what comes back. The guard is no
/// longer the Safety Kernel's *in principle* — it is on a loop that runs.
///
/// # What is measured, and what is not
///
/// The same reach `replay_deterministic` has and for the same reason. Real: this robot's descriptor,
/// its channel names, `crate::limits`' resolved set out of its envelope and signed profile, the
/// grants an operator actually made on this machine, and the same `evaluate_cycle` the loop calls.
/// Synthetic: the two commands, because no policy has ever run here.
///
/// So it establishes that **the gate on this robot's own grants refuses and admits the right
/// commands**. It does not establish that a sandboxed package was stopped: units launch confined
/// (`launch::confinement_properties`) but nothing performs the forbidden hold — that is
/// `safety.policy.isolation`, which stays skipped, and its reason says what would execute it.
///
/// # Both halves, because a gate that refuses everything is not a gate
///
/// A robot with no grants can only be shown refusing, and a check that only ever refuses would pass
/// on a kernel whose mode gate returned "no" unconditionally. So the positive half needs a granted
/// token to admit, and a robot with no grants in force is **skipped** rather than half-measured.
///
/// The third case is the one §7.2 is about: a command that names **no** capability. `None` is *the
/// runtime did not say*, which is not the same as "no capability" — and the kernel treats it as
/// neither a pass nor a trip, because a guard with no input has not shown the command safe. It is
/// asserted here so that an edition that quietly made `None` mean "allowed" fails this test rather
/// than shipping.
fn grants_gate(
    body: &Body,
    limits: &crate::limits::Owned,
    grants: &[String],
    at: &str,
) -> SctsResult {
    let id = "package.grants.gate";

    let Some(known) = body.known() else {
        return skipped(
            id,
            "this robot has no usable embodiment descriptor, so it has no channel to command. A \
             mode-gate result over invented channels would be a conformance result about a machine \
             that does not exist (`GET /embodiment`).",
        );
    };

    let mut channels: Vec<String> = known
        .descriptor
        .joints
        .iter()
        .map(|joint| joint.name.clone())
        .collect();
    channels.sort();
    channels.dedup();
    let Some(channel) = channels.first().cloned() else {
        return skipped(
            id,
            "this robot's descriptor declares no joints, so there is no command for a capability to \
             be exercised by.",
        );
    };

    let Some(granted) = grants.first() else {
        return skipped(
            id,
            "no capability is granted on this robot, so only the refusing half of the gate could be \
             measured. A check that only ever refuses would pass on a kernel whose mode gate \
             returned no unconditionally, which is the failure it exists to catch. Grant a \
             capability to a package (`skynet grant`) and this runs.",
        );
    };

    // A token no operator grants, and not a plausible one: it has to be outside the vocabulary so
    // that a robot which happens to hold every real capability still exercises the refusal.
    const UNGRANTED: &str = "scts.capability.never_granted";
    if grants.iter().any(|held| held == UNGRANTED) {
        return skipped(
            id,
            "this robot holds a grant for the token this check uses as its counter-example, so the \
             refusing half cannot be measured against it.",
        );
    }

    limits.with_resolved(|resolved, envelope| {
        // §7.4 and §42: an advisory tier observes and enforces nothing, and `evaluate_cycle`
        // encodes that by forwarding the *desired* command whatever the guards said. So on a
        // guest-tier robot the gate trips and the command still goes out — by design — and a check
        // asserting that an ungranted command was withheld would be asserting enforcement this
        // robot has never claimed.
        //
        // Skipped rather than reported as a weaker pass. §15's test is about a gate that refuses;
        // a tier that cannot refuse has not passed it, and guest observation never becomes a safety
        // claim.
        if resolved.enforcement != Enforcement::Enforcing {
            return skipped(
                id,
                format!(
                    "this robot resolves limits at the {} tier, where enforcement is {} — the mode                      gate trips and the command is forwarded anyway (§7.4). The test is about a                      gate that refuses, and a tier that cannot refuse has not passed it.",
                    resolved.tier.as_str(),
                    resolved.enforcement.as_str(),
                ),
            );
        }

        if !resolved.is_active(Guard::ModeGate) {
            return skipped(
                id,
                "the mode gate is not among the guards this robot resolves as active, so there is \
                 nothing here to measure. §7.2's rule is that an inactive guard reports itself \
                 rather than being assumed to have passed.",
            );
        }

        let with_grants: Vec<&str> = grants.iter().map(String::as_str).collect();
        let resolved = resolved.clone().with_grants(&with_grants);

        // One channel, well inside every ceiling, so nothing but the mode gate has anything to say.
        // A command that also tripped the envelope would make the verdict ambiguous about which
        // guard produced the hold.
        let desired = [(channel.as_str(), 0.0f64)];
        let granted_token: &str = granted.as_str();

        // Written out rather than built by a closure. A closure would have to name one lifetime
        // for `capability`, and the three commands do not share one: two borrow strings with
        // different extents and the third names none at all.
        let admitted = evaluate_cycle(
            &CommandCycle {
                now_ms: 1000.0,
                command_ms: 1000.0,
                sequence: 1,
                desired: &desired,
                previous: None,
                capability: Some(granted_token),
                nearest_person_m: None,
            },
            &resolved,
            envelope,
            None,
        );
        let refused = evaluate_cycle(
            &CommandCycle {
                now_ms: 1000.0,
                command_ms: 1000.0,
                sequence: 2,
                desired: &desired,
                previous: None,
                capability: Some(UNGRANTED),
                nearest_person_m: None,
            },
            &resolved,
            envelope,
            Some(1),
        );
        let unstated = evaluate_cycle(
            &CommandCycle {
                now_ms: 1000.0,
                command_ms: 1000.0,
                sequence: 3,
                desired: &desired,
                previous: None,
                capability: None,
                nearest_person_m: None,
            },
            &resolved,
            envelope,
            Some(2),
        );

        let admitted_tripped = admitted.tripped.iter().any(|guard| *guard == Guard::ModeGate);
        let refused_tripped = refused.tripped.iter().any(|guard| *guard == Guard::ModeGate);
        let unstated_tripped = unstated.tripped.iter().any(|guard| *guard == Guard::ModeGate);

        let mut faults: Vec<String> = Vec::new();
        if admitted_tripped {
            faults.push(format!(
                "a command exercising `{granted_token}`, which this operator granted, was refused \
                 by the mode gate"
            ));
        }
        if !refused_tripped {
            faults.push(format!(
                "a command exercising `{UNGRANTED}`, which nobody granted, was not refused"
            ));
        }
        if !refused.command.is_empty() {
            faults.push(
                "the refused command still reached the wire: a mode-gate trip demands a hold, and a \
                 hold sends no channel"
                    .to_string(),
            );
        }
        if unstated_tripped {
            faults.push(
                "a command naming no capability tripped the mode gate. §7.2 keeps `absent` and \
                 `denied` apart: the runtime did not say, which is not the same as asking for \
                 something it was refused"
                    .to_string(),
            );
        }

        if faults.is_empty() {
            ran(
                id,
                SctsOutcome::Pass,
                at,
                format!(
                    "measured against this robot's own grants ({} in force). A command exercising \
                     `{granted_token}` was admitted; one exercising `{UNGRANTED}` was refused and \
                     sent no channel; one naming no capability was neither. Synthetic in one \
                     respect and stated rather than hidden: no policy has run on this edition, so \
                     the two commands are this runner's, not a package's.",
                    grants.len()
                ),
            )
        } else {
            ran(id, SctsOutcome::Fail, at, faults.join("; "))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use skynet_contracts::admission::DeviceCapacity;

    fn capacity() -> Capacity {
        Capacity::Known(Box::new(DeviceCapacity {
            ram_mb: 8192.0,
            os_version: "0.1.0".to_string(),
            ..Default::default()
        }))
    }

    /// The limits of a robot nobody has provisioned: no descriptor, no profile, no cell.
    ///
    /// Rooted at a path that does not exist, on purpose. Every read `crate::limits` performs is a
    /// file read that returns nothing, which is exactly the state these tests are about — and a
    /// fixture that wrote files would be testing the writer.
    fn bare_limits() -> crate::limits::Owned {
        crate::limits::read(
            &crate::layout::Layout::new("does-not-exist"),
            &Body::Absent,
            crate::limits::ROBOT_TIER,
            &crate::blackbox::ProximitySighting::Unobservable("this fixture has no ring"),
        )
    }

    fn outcome(run: &Run, id: &str) -> SctsOutcome {
        run.results
            .iter()
            .find(|result| result.test_id == id)
            .unwrap_or_else(|| panic!("{id} is not in the run"))
            .outcome
    }

    #[test]
    fn every_runtime_test_gets_exactly_one_entry_in_order() {
        let run = run(
            &capacity(),
            &Body::Absent,
            &bare_limits(),
            &[],
            None,
            "2026-09-02T00:00:00Z",
        );
        let ids: Vec<&str> = run
            .results
            .iter()
            .map(|result| result.test_id.as_str())
            .collect();
        assert_eq!(ids, RUNTIME_TESTS);
    }

    /// The same eight `skynet driver check` says it cannot cover. A test in one list and not the
    /// other is a test nobody runs, and it would be invisible from either side.
    fn landed(kind: skynetd::desired::PackageKind) -> Vec<(String, String)> {
        crate::launch::confinement_properties(kind)
            .iter()
            .map(|line| {
                let (key, value) = line.split_once('=').expect("Key=Value");
                (key.to_string(), value.to_string())
            })
            .collect()
    }

    /// What `/proc/<pid>/status` says on a process the kernel really is holding.
    fn held() -> Vec<(String, String)> {
        vec![
            ("NoNewPrivs".to_string(), "1".to_string()),
            ("CapBnd".to_string(), "0000000000000000".to_string()),
        ]
    }

    /// The pass, and the sentence it carries.
    ///
    /// What makes this a machine property rather than an edition one is in the detail: it says the
    /// directives were read back off a running unit and compared line-exactly, never that the
    /// launcher asked for them. A robot reporting the second would be reporting on its compiler.
    #[test]
    fn a_confined_unit_passes_isolation() {
        let result = policy_isolation(
            Some(&Confinement {
                package: "openrobot/holder".to_string(),
                kind: skynetd::desired::PackageKind::Policy,
                observed: Ok(landed(skynetd::desired::PackageKind::Policy)),
                applied: Ok(held()),
            }),
            "2026-09-07T00:00:00Z",
        );
        assert_eq!(result.outcome, SctsOutcome::Pass);
        assert!(
            !result.ran_at.is_empty(),
            "a measured result carries its time"
        );
        let detail = result.detail.unwrap_or_default();
        assert!(detail.contains("systemctl show"));
        assert!(detail.contains("not taken from the argv"));
    }

    /// The fail names the directive, because §64 makes a refusal that does not say what to fix a
    /// defect rather than a message.
    #[test]
    fn a_unit_that_kept_capabilities_fails_isolation() {
        let mut observed = landed(skynetd::desired::PackageKind::Policy);
        for entry in &mut observed {
            if entry.0 == "CapabilityBoundingSet" {
                entry.1 = "cap_sys_admin".to_string();
            }
        }
        let result = policy_isolation(
            Some(&Confinement {
                package: "openrobot/holder".to_string(),
                kind: skynetd::desired::PackageKind::Policy,
                observed: Ok(observed),
                applied: Ok(held()),
            }),
            "2026-09-07T00:00:00Z",
        );
        assert_eq!(result.outcome, SctsOutcome::Fail);
        assert!(result
            .detail
            .unwrap_or_default()
            .contains("CapabilityBoundingSet="));
    }

    /// Nothing running is a skip, never a pass. §15's first award rule is that absence is not one.
    #[test]
    fn no_sandboxed_unit_is_a_skip() {
        let result = policy_isolation(None, "2026-09-07T00:00:00Z");
        assert_eq!(result.outcome, SctsOutcome::Skipped);
        // A skipped test carries no time: §15 decays an award from the age of the oldest bench
        // result, and a gap dated today would look fresh.
        assert!(result.ran_at.is_empty());
        assert!(result
            .detail
            .unwrap_or_default()
            .contains("A driver is native by design"));
    }

    /// A unit that is running and could not be described is a skip carrying the reason, not a
    /// fail. Nothing was measured, so nothing failed.
    #[test]
    fn a_unit_that_could_not_be_read_is_a_skip_that_says_why() {
        let result = policy_isolation(
            Some(&Confinement {
                package: "openrobot/holder".to_string(),
                kind: skynetd::desired::PackageKind::Policy,
                observed: Err("systemctl could not describe the unit: it stopped".to_string()),
                applied: Err("no process".to_string()),
            }),
            "2026-09-07T00:00:00Z",
        );
        assert_eq!(result.outcome, SctsOutcome::Skipped);
        let detail = result.detail.unwrap_or_default();
        assert!(detail.contains("openrobot/holder"));
        assert!(detail.contains("it stopped"));
    }

    /// The disagreement, which is the case this rung exists for.
    ///
    /// systemd reports every directive and the kernel is holding none of them. Before the process
    /// was read, that robot passed: the manager's record was the whole of the evidence, and a
    /// record is what a manager holds rather than what a kernel applied.
    #[test]
    fn a_kernel_that_did_not_apply_the_directives_fails_even_when_systemd_agrees() {
        let result = policy_isolation(
            Some(&Confinement {
                package: "openrobot/holder".to_string(),
                kind: skynetd::desired::PackageKind::Policy,
                observed: Ok(landed(skynetd::desired::PackageKind::Policy)),
                applied: Ok(vec![
                    ("NoNewPrivs".to_string(), "0".to_string()),
                    ("CapBnd".to_string(), "000001ffffffffff".to_string()),
                ]),
            }),
            "2026-09-07T00:00:00Z",
        );
        assert_eq!(result.outcome, SctsOutcome::Fail);
        let detail = result.detail.unwrap_or_default();
        assert!(detail.contains("the kernel does not agree"), "{detail}");
        assert!(detail.contains("NoNewPrivs is 0"), "{detail}");
        assert!(detail.contains("000001ffffffffff"), "{detail}");
    }

    /// A process `/proc` does not describe is a skip, on `confinement_shortfall`'s own rule.
    ///
    /// Fail-closed the other way would be worse: failing a robot because its own `/proc` could not
    /// be read reports a sandbox defect where the true finding is that nothing could be measured.
    /// Passing would be worse still — it is the manager's record standing in for the kernel's,
    /// which is the substitution this rung was added to stop.
    #[test]
    fn a_process_the_kernel_will_not_describe_is_a_skip() {
        let result = policy_isolation(
            Some(&Confinement {
                package: "openrobot/holder".to_string(),
                kind: skynetd::desired::PackageKind::Policy,
                observed: Ok(landed(skynetd::desired::PackageKind::Policy)),
                applied: Err("/proc/4141/status could not be read".to_string()),
            }),
            "2026-09-07T00:00:00Z",
        );
        assert_eq!(result.outcome, SctsOutcome::Skipped);
        let detail = result.detail.unwrap_or_default();
        assert!(detail.contains("/proc/4141/status"), "{detail}");
        assert!(
            detail.contains("unread by anybody else"),
            "the skip says why a manager's word alone is not enough: {detail}"
        );
    }

    /// A missing line is a shortfall, not a silence.
    ///
    /// `CapBnd` absent from `/proc/<pid>/status` means the kernel did not say, and §15's rule
    /// throughout is that a property nobody can read is not one anybody certifies.
    #[test]
    fn a_kernel_reading_with_a_line_missing_is_a_shortfall() {
        let shortfall = crate::launch::kernel_shortfall(
            skynetd::desired::PackageKind::Policy,
            &[("NoNewPrivs".to_string(), "1".to_string())],
        );
        assert_eq!(shortfall.len(), 1);
        assert!(
            shortfall[0].contains("does not report CapBnd"),
            "{shortfall:?}"
        );
    }

    /// A driver is native by design (§8.3), so the kernel has nothing to contradict.
    #[test]
    fn a_driver_asks_nothing_of_the_kernel_either() {
        assert!(
            crate::launch::kernel_shortfall(skynetd::desired::PackageKind::Driver, &[]).is_empty()
        );
    }

    /// The parser reads systemd's format and `/proc`'s, and they are not the same format.
    #[test]
    fn proc_status_is_parsed_by_its_own_rules() {
        let text = "Name:\tsh\nNoNewPrivs:\t1\nCapBnd:\t0000000000000000\nThreads:\t1\n";
        let read = crate::launch::parse_proc_status(text);
        assert_eq!(
            read,
            vec![
                ("NoNewPrivs".to_string(), "1".to_string()),
                ("CapBnd".to_string(), "0000000000000000".to_string()),
            ],
            "only the two lines this check is about, with the tabs stripped"
        );
    }

    #[test]
    fn the_two_lists_are_the_same_eight() {
        let mut mine: Vec<&str> = RUNTIME_TESTS.to_vec();
        mine.sort();
        // Restated here rather than imported, because `skynet-cli` is a binary crate this one does
        // not depend on — so the check is that a human keeping two lists in step has not slipped,
        // which is exactly the failure mode a shared constant would remove and a comment would not.
        let mut theirs = vec![
            "safety.kernel.parity",
            "safety.policy.isolation",
            "package.signature.required",
            "package.grants.gate",
            "ota.safety_gate",
            "telemetry.replay.deterministic",
            "security.identity.attested",
            "security.credential.scope",
        ];
        theirs.sort();
        assert_eq!(mine, theirs);
    }

    /// The uncomfortable one. This edition stages an unsigned package, so it fails a guest-tier
    /// test, and the runner says so rather than reporting the gap as unmeasured.
    #[test]
    fn an_unsigned_package_installs_so_this_edition_fails_its_own_guest_bar() {
        let run = run(
            &capacity(),
            &Body::Absent,
            &bare_limits(),
            &[],
            None,
            "2026-09-02T00:00:00Z",
        );
        assert_eq!(
            outcome(&run, "package.signature.required"),
            SctsOutcome::Fail
        );
        let detail = run.results[2].detail.clone().unwrap_or_default();
        assert!(
            detail.contains("reports on this edition rather than gating"),
            "{detail}"
        );
        assert!(detail.contains("§19"), "{detail}");
    }

    /// The one it passes, and it is a real refusal by the function the updater runs.
    #[test]
    fn the_safety_gate_refuses_a_kernel_change_with_no_bench_pass() {
        let run = run(
            &capacity(),
            &Body::Absent,
            &bare_limits(),
            &[],
            None,
            "2026-09-02T00:00:00Z",
        );
        assert_eq!(outcome(&run, "ota.safety_gate"), SctsOutcome::Pass);
        assert!(run.results[4]
            .detail
            .as_ref()
            .is_some_and(|detail| detail.contains("bench")));
    }

    #[test]
    fn two_of_the_eight_are_executed_and_six_are_named() {
        let run = run(
            &capacity(),
            &Body::Absent,
            &bare_limits(),
            &[],
            None,
            "2026-09-02T00:00:00Z",
        );
        assert_eq!(run.executed, 2);
        for result in &run.results {
            assert!(
                result.detail.as_ref().is_some_and(|d| !d.is_empty()),
                "{} carries no sentence, and a skipped test with no reason is the shape §15's \
                 first rule exists against",
                result.test_id
            );
        }
    }

    /// §15's first rule, from the runner's side: a skipped test is dated with nothing, because an
    /// award decays from the age of the oldest result and a gap that looked fresh would postpone
    /// the decay it should be causing.
    #[test]
    fn a_skipped_test_carries_no_date() {
        let run = run(
            &capacity(),
            &Body::Absent,
            &bare_limits(),
            &[],
            None,
            "2026-09-02T00:00:00Z",
        );
        for result in &run.results {
            if result.outcome == SctsOutcome::Skipped {
                assert!(result.ran_at.is_empty(), "{}", result.test_id);
            } else {
                assert_eq!(result.ran_at, "2026-09-02T00:00:00Z");
            }
        }
    }

    /// A robot whose capacity is unknown cannot run admission, and says so rather than guessing at
    /// one — the same rule `crate::capacity` applies to the install path.
    #[test]
    fn a_robot_with_no_capacity_skips_the_admission_check_rather_than_inventing_one() {
        let run = run(
            &Capacity::Unavailable {
                detail: "/proc/meminfo could not be read".to_string(),
            },
            &Body::Absent,
            &bare_limits(),
            &[],
            None,
            "2026-09-02T00:00:00Z",
        );
        assert_eq!(
            outcome(&run, "package.signature.required"),
            SctsOutcome::Skipped
        );
        assert_eq!(run.executed, 1, "only the safety gate ran");
    }

    /// The award over this robot's own evidence is never a certificate, and with six of eight
    /// skipped and one failed it cannot be anything but the floor.
    #[test]
    fn the_attainable_tier_is_capped_by_what_was_not_run() {
        let run = run(
            &capacity(),
            &Body::Absent,
            &bare_limits(),
            &[],
            None,
            "2026-09-02T00:00:00Z",
        );
        let verdict = run.attainable(false);
        assert_ne!(verdict.awarded, "certified");
        assert!(
            verdict
                .missing
                .contains(&"safety.estop.latency".to_string())
                || !verdict.missing.is_empty(),
            "every bench test is missing from a run that took none: {:?}",
            verdict.missing
        );
        assert!(verdict
            .failures
            .contains(&"package.signature.required".to_string()));
    }

    #[test]
    fn the_wire_says_it_is_not_a_certificate() {
        let run = run(
            &capacity(),
            &Body::Absent,
            &bare_limits(),
            &[],
            None,
            "2026-09-02T00:00:00Z",
        );
        let document = to_json(&run, false);
        assert_eq!(document.arr_or_empty("results").len(), 8);
        assert_eq!(document.get("executed"), Some(&Json::Num(2.0)));
        assert!(document
            .get("note")
            .and_then(Json::as_str)
            .is_some_and(|note| note.contains("never signs")));
        // A skipped test's date is null on the wire rather than an empty string, so a consumer
        // sorting by it does not sort eight empty strings to the top.
        assert_eq!(
            document.arr_or_empty("results")[0].get("ran_at"),
            Some(&Json::Null)
        );
    }
    // ── package.grants.gate ──────────────────────────────────────────────────
    //
    // The one runtime test in this module that changed from skipped to measured, and the reason it
    // could is that the path its skip reason named — the kernel's mode gate, on a loop that runs —
    // now exists in `skynet-kernel-host`.

    /// A descriptor with two joints, written to a real file, because `Body::Present` is only
    /// reachable through the reader and a hand-built `Known` would be a fixture of this module's
    /// own imagination rather than of the parser every robot uses.
    fn body_with_joints(dir: &std::path::Path) -> Body {
        let path = dir.join("embodiment.json");
        std::fs::write(
            &path,
            r#"{
              "schema": "skynet.embodiment/1",
              "robot_slug": "scts-probe",
              "base": "fixed",
              "dof": 2,
              "joints": [
                {"name": "elbow", "type": "revolute", "control_modes": ["position"]},
                {"name": "shoulder", "type": "revolute", "control_modes": ["position"]}
              ]
            }"#,
        )
        .expect("the fixture is writable");
        crate::embodiment::read(&path)
    }

    /// Limits resolved at the tier where the mode gate actually refuses.
    ///
    /// `ROBOT_TIER` is guest and `bare_limits` uses it, so on every robot today the gate is
    /// skipped as advisory. That is the honest live outcome and it is not the thing under test
    /// here: what these cases check is the gate's arithmetic at the tier it applies to.
    fn enforcing_limits() -> crate::limits::Owned {
        crate::limits::read(
            &crate::layout::Layout::new("does-not-exist"),
            &Body::Absent,
            skynet_contracts::safety::Tier::Host,
            &crate::blackbox::ProximitySighting::Unobservable("this fixture has no ring"),
        )
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("skynet-conformance-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A robot with no grants is skipped, not passed.
    ///
    /// The refusing half would measure fine on its own, and a check that only ever refuses would
    /// pass on a kernel whose mode gate said no unconditionally — which is the failure it exists to
    /// catch. So the absence of a granted token is a reason not to report a result.
    #[test]
    fn the_grants_gate_is_skipped_when_nothing_is_granted() {
        let dir = scratch("no-grants");
        let body = body_with_joints(&dir);
        let result = grants_gate(&body, &enforcing_limits(), &[], "2026-09-04T00:00:00Z");
        assert_eq!(result.outcome, SctsOutcome::Skipped);
        assert_eq!(result.ran_at, "", "a skipped test carries no date (§15)");
        assert!(
            result
                .detail
                .unwrap_or_default()
                .contains("only the refusing half"),
            "the reason has to say which half could not be measured",
        );
    }

    /// On a real robot the gate is skipped, because `ROBOT_TIER` is guest.
    ///
    /// This is the live outcome and it is worth a test of its own: §7.4 says an advisory tier
    /// observes and enforces nothing, so the mode gate trips and the command goes out anyway. A
    /// runner that reported a pass there would be turning guest observation into a safety claim,
    /// which is the one thing §42 forbids.
    #[test]
    fn the_grants_gate_is_skipped_at_the_tier_every_robot_actually_runs() {
        let dir = scratch("advisory");
        let body = body_with_joints(&dir);
        let result = grants_gate(
            &body,
            // `bare_limits` resolves at ROBOT_TIER, which is guest.
            &bare_limits(),
            &["hri.speech".to_string()],
            "2026-09-04T00:00:00Z",
        );

        assert_eq!(result.outcome, SctsOutcome::Skipped);
        let detail = result.detail.unwrap_or_default();
        assert!(detail.contains("guest"), "{detail}");
        assert!(
            detail.contains("forwarded anyway"),
            "the reason has to say that the command still goes out, not just that the tier is low:              {detail}",
        );
    }

    /// A robot with no descriptor has no channel to command, so there is nothing to measure.
    #[test]
    fn the_grants_gate_is_skipped_without_a_body() {
        let result = grants_gate(
            &Body::Absent,
            &bare_limits(),
            &["hri.speech".to_string()],
            "2026-09-04T00:00:00Z",
        );
        assert_eq!(result.outcome, SctsOutcome::Skipped);
        assert!(result
            .detail
            .unwrap_or_default()
            .contains("no usable embodiment descriptor"));
    }

    /// With a body and a grant, both halves are measured and the gate passes.
    ///
    /// What this asserts is narrow and deliberate: a command exercising the granted token is
    /// admitted, one exercising a token nobody granted is refused and sends no channel, and one
    /// naming no capability is neither — which is §7.2's distinction between *absent* and *denied*.
    #[test]
    fn the_grants_gate_measures_both_halves_against_a_real_grant() {
        let dir = scratch("granted");
        let body = body_with_joints(&dir);
        let result = grants_gate(
            &body,
            &enforcing_limits(),
            &["hri.speech".to_string()],
            "2026-09-04T00:00:00Z",
        );

        assert_eq!(
            result.outcome,
            SctsOutcome::Pass,
            "{}",
            result.detail.clone().unwrap_or_default()
        );
        assert_eq!(
            result.ran_at, "2026-09-04T00:00:00Z",
            "a measured test carries the date it ran",
        );
        let detail = result.detail.unwrap_or_default();
        assert!(detail.contains("hri.speech"), "{detail}");
        assert!(
            detail.contains("no policy has run on this edition"),
            "the reach has to be stated in the result, not only in the module: {detail}",
        );
    }

    /// The counter-example token being granted is itself a reason to skip.
    #[test]
    fn a_robot_holding_the_counter_example_token_is_skipped() {
        let dir = scratch("counter-example");
        let body = body_with_joints(&dir);
        let result = grants_gate(
            &body,
            &enforcing_limits(),
            &["scts.capability.never_granted".to_string()],
            "2026-09-04T00:00:00Z",
        );
        assert_eq!(result.outcome, SctsOutcome::Skipped);
        assert!(result
            .detail
            .unwrap_or_default()
            .contains("counter-example"));
    }

    /// The run carries the gate's result in its place, and a grant changes it from skipped.
    #[test]
    fn a_grant_moves_the_gate_from_skipped_to_measured_in_a_whole_run() {
        let dir = scratch("whole-run");
        let body = body_with_joints(&dir);

        let without = run(
            &capacity(),
            &body,
            &enforcing_limits(),
            &[],
            None,
            "2026-09-04T00:00:00Z",
        );
        assert_eq!(
            outcome(&without, "package.grants.gate"),
            SctsOutcome::Skipped
        );

        let with = run(
            &capacity(),
            &body,
            &enforcing_limits(),
            &["hri.speech".to_string()],
            None,
            "2026-09-04T00:00:00Z",
        );
        assert_eq!(outcome(&with, "package.grants.gate"), SctsOutcome::Pass);

        // And the count of tests that actually ran goes up by exactly one.
        assert_eq!(
            with.executed,
            without.executed + 1,
            "one more test executed, and no others changed",
        );
    }
}

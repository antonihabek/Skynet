//! §10 on a real machine: the identity on disk, the credential beside it, and pairing.
//!
//! [`skynet_contracts::identity`] decides §10's three questions and takes no clock, no network and
//! no filesystem. This is the other half — the two files, the exchange that fills them, and the
//! measurement of what the robot is actually running.
//!
//! # Two files, and the split is the point
//!
//! [`Layout::device`] holds the identity and [`Layout::credential`] holds the credential. Every
//! surface this daemon has reads the first: the console renders it, `GET /identity` returns it, and
//! a check-in will quote it. None of them may show the second. A single document would have to be
//! either unreadable by the console or a bearer credential served over a LAN listener, and neither
//! is a choice worth making when the alternative is two files.
//!
//! It has a second consequence worth stating, because it decides what a half-finished pairing looks
//! like. **Whether a robot is paired is read off the credential, not off the document.** So an
//! interrupted pairing that wrote the document and not the credential leaves a robot that reports
//! itself unpaired and can pair again — rather than one that claims a fleet it cannot talk to.
//!
//! # The code and the credential never cross a command line
//!
//! The request itself is [`crate::connectivity::post_json`], which is the one place in this daemon
//! that makes an outbound HTTPS request and the one place that knows why: `/proc/<pid>/cmdline` is
//! world-readable on Linux, so a secret passed as an argument is published to every local user for
//! the length of the request. The pairing code travels inside the request body and the credential
//! comes back on stdout; neither is ever an argument.
//!
//! It used to be written here, inline, and moving it was the right change for a reason worth
//! recording: two callers with the same rule is two places for the rule to be lost, and the second
//! one — the check-in — was written by somebody who would have copied whatever this file did.
//!
//! # What this edition cannot measure, and does not pretend to
//!
//! **The key binding.** §10 binds a device key to a TPM or secure element where one exists, and
//! `security.identity.attested` checks it. Nothing here measures that, so it is reported as
//! unmeasured on every robot and [`IdentityFinding::KeyBindingUnknown`] says so. Two probes suggest
//! themselves and neither answers the question: `/dev/tpm0` says a TPM exists, not that this key is
//! in it, and the absence of a private key file says only that this daemon cannot find one. Either
//! would turn an unmeasured robot into an attested one on a bench, which is finding 17 of the
//! twenty-first session — the most dangerous defaults are the ones that make a demo work.
//!
//! **Generating a key.** `skynet-ed25519` verifies; signing is a test feature. So the device key a
//! robot presents at pairing is the one its vendor put in the birth certificate, and a robot with no
//! certificate pairs with no key. That is guest tier's ordinary case and it is reported as two
//! warnings rather than hidden behind a key this daemon invented.

use std::path::{Path, PathBuf};

use skynet_contracts::identity::{
    check_identity, code_result_to_json, identity_verdict_to_json, normalize_pairing_code,
    Attestation, Identity, IdentityFinding, IdentityVerdict, Measure, Observed,
    DEVICE_TOKEN_PREFIX,
};
use skynet_json::Json;

use crate::layout::Layout;
use crate::trust::TrustStore;

/// What is on this robot's disk, and what it observes about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    /// The identity, with `credential` set from whether the credential file exists.
    pub identity: Identity,
    /// What §50's certificate says, when there is a trusted one.
    pub observed: Observed,
    /// Which Hub this robot was paired with, when it was paired with one.
    pub endpoint: Option<String>,
    /// Why the identity document could not be read, when it could not be.
    ///
    /// `None` and *"there is no file"* are different facts, and this field is why they can be told
    /// apart. §19's trust store makes the same distinction for the same reason: one is a robot
    /// nobody provisioned and the other is a file somebody broke, and the remedies are a pairing
    /// and a person.
    pub fault: Option<String>,
}

/// Read the identity document and the credential beside it.
///
/// Never fails. A document that will not parse produces a [`Held`] that reports itself unpaired
/// **and says why**, because the alternative — refusing to answer — would take `GET /identity` and
/// the console's identity panel away from the one robot whose identity somebody needs to look at.
pub fn read(layout: &Layout) -> Held {
    let credential = std::fs::read_to_string(layout.credential())
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());

    let mut identity = Identity {
        credential: credential.is_some(),
        credential_prefix: credential.as_deref().map(prefix_of).unwrap_or_default(),
        ..Identity::default()
    };

    let mut fault = None;
    let mut endpoint = None;
    match std::fs::read_to_string(layout.device()) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            fault = Some(format!(
                "{} could not be read: {error}",
                layout.device().display()
            ))
        }
        Ok(text) => match Json::parse(&text) {
            Err(error) => {
                fault = Some(format!(
                    "{} does not parse: {error}. Until it is replaced this robot reports itself \
                     unpaired, which is what it is: nothing here can say which fleet it belongs to.",
                    layout.device().display()
                ))
            }
            Ok(document) => {
                let text_of = |key: &str| {
                    document
                        .get(key)
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_string()
                };
                identity.device_id = text_of("device_id");
                identity.namespace = text_of("namespace");
                identity.device_key = text_of("device_key");
                identity.binding = document
                    .get_present("binding")
                    .and_then(Json::as_str)
                    .and_then(skynet_contracts::identity::KeyBinding::parse);
                identity.scopes = document.get_present("scopes").map(|scopes| {
                    scopes
                        .as_arr()
                        .map(|rows| {
                            rows.iter()
                                .filter_map(Json::as_str)
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default()
                });
                let named = text_of("endpoint");
                if !named.is_empty() {
                    endpoint = Some(named);
                }
            }
        },
    }

    Held {
        identity,
        // Deliberately empty. What §50's certificate says depends on whether its signature verifies
        // against the trust store, and this function does not have one — so it reports **no**
        // certificate, which is the conservative answer and is what makes
        // [`read`] safe to call from a surface that has no store to hand. [`evaluate`] fills it in.
        observed: Observed::default(),
        endpoint,
        fault,
    }
}

/// The credential's prefix, up to and including its first underscore.
///
/// Everything after it is the secret and is never carried anywhere. A credential with no underscore
/// has no prefix, which [`check_identity`] reports as not being a device credential — correctly,
/// because §10's class is the one that begins `skd_`.
fn prefix_of(credential: &str) -> String {
    match credential.find('_') {
        Some(index) => credential[..=index].to_string(),
        None => String::new(),
    }
}

/// What §50's certificate says, checked against the keys this robot trusts.
///
/// A certificate whose signature does not verify against the trust store attests **nothing**, and
/// is reported here exactly as an absent one is — the same rule [`crate::ota`] applies to a bench
/// pass. An unsigned certificate on a robot's disk is a file anybody with the disk could have
/// written, and treating it as evidence about the machine would make §50.3 a comment.
pub fn read_observed(layout: &Layout, trust: &TrustStore) -> Observed {
    match crate::birth::read(layout, trust) {
        Ok(Some(held))
            if held.certificate.signature
                == skynet_contracts::admission::SignatureState::Verified =>
        {
            Observed {
                birth_key: held.certificate.device_key.clone(),
                birth_certificate: true,
            }
        }
        _ => Observed::default(),
    }
}

/// Read everything, with the certificate checked against the trust store.
pub fn evaluate(layout: &Layout, trust: &TrustStore) -> (Held, IdentityVerdict) {
    let mut held = read(layout);
    held.observed = read_observed(layout, trust);
    let verdict = check_identity(&held.identity, &held.observed);
    (held, verdict)
}

/// The identity as an API answer, for `GET /identity` and the console.
///
/// Carries the identity, the verdict and the endpoint, and **no credential** — not truncated, not
/// masked, absent. A masked credential is a credential whose length is on a LAN listener, and the
/// prefix is the only part of it any surface has a use for.
pub fn to_json(held: &Held, verdict: &IdentityVerdict) -> Json {
    let mut out = Json::obj();
    out.set("paired", Json::Bool(verdict.paired))
        .set("attested", Json::Bool(verdict.attested))
        .set("device_id", string_or_null(&held.identity.device_id))
        .set("namespace", string_or_null(&held.identity.namespace))
        .set("device_key", string_or_null(&held.identity.device_key))
        .set(
            "binding",
            match held.identity.binding {
                Some(binding) => Json::Str(binding.as_str().to_string()),
                None => Json::Null,
            },
        )
        .set(
            "credential_prefix",
            string_or_null(&held.identity.credential_prefix),
        )
        .set(
            "endpoint",
            match &held.endpoint {
                Some(endpoint) => Json::Str(endpoint.clone()),
                None => Json::Null,
            },
        )
        .set(
            "fault",
            match &held.fault {
                Some(fault) => Json::Str(fault.clone()),
                None => Json::Null,
            },
        );

    let verdict_json = identity_verdict_to_json(verdict);
    out.set(
        "findings",
        verdict_json
            .get("findings")
            .cloned()
            .unwrap_or_else(Json::arr),
    );
    out
}

fn string_or_null(value: &str) -> Json {
    if value.trim().is_empty() {
        Json::Null
    } else {
        Json::Str(value.to_string())
    }
}

// ── The exchange ─────────────────────────────────────────────────────────────

/// What the robot presents when it offers a pairing code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Enrol {
    /// The device public key, from §50's certificate. Empty when there is no certificate.
    pub device_key: String,
    /// What the vendor called this machine, when a certificate says.
    pub model: String,
    /// Its serial, when a certificate says.
    pub serial: String,
    /// The agent making the request, so a Hub can tell which edition paired.
    pub agent: String,
}

impl Enrol {
    /// The request body, canonical.
    ///
    /// The code goes in here rather than into a URL or an argument. A query string is in the Hub's
    /// access log and an argument is in `/proc`; a body is in neither.
    pub fn body(&self, code: &str) -> Json {
        let mut out = Json::obj();
        out.set("agent", Json::Str(self.agent.clone()))
            .set("code", Json::Str(code.to_string()))
            .set("device_key", Json::Str(self.device_key.clone()))
            .set("model", Json::Str(self.model.clone()))
            .set("serial", Json::Str(self.serial.clone()));
        out
    }
}

/// What a Hub returns when it accepts a code.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Enrolled {
    /// The Hub's identifier for this device.
    pub device_id: String,
    /// The namespace this robot now belongs to.
    pub namespace: String,
    /// The credential. Held only long enough to be written, and never rendered.
    pub credential: String,
    /// The scopes it declares, when it declares any.
    pub scopes: Option<Vec<String>>,
}

/// Why an exchange did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnrolError {
    /// A code.
    pub code: &'static str,
    /// The sentence an operator gets.
    pub detail: String,
}

/// Whatever can turn a pairing code into a device credential (§10).
///
/// A trait for the reason [`crate::ota::Updater`] and [`crate::launch::Launcher`] are traits: the
/// suite has to drive every branch of this — a refusal, a malformed answer, a credential of the
/// wrong class — and an exchange that only runs against a real Hub is an exchange nobody tests
/// until a customer is standing in front of a robot.
pub trait Enrolment {
    /// Present the code. `Ok` means the Hub minted a credential.
    fn present(&self, code: &str, request: &Enrol) -> Result<Enrolled, EnrolError>;

    /// What this is, for an operator. Says what is actually happening rather than naming a type.
    fn describe(&self) -> String;

    /// Where it goes, recorded in the identity document so the robot knows who to report to.
    fn endpoint(&self) -> String;
}

/// Read a Hub's answer.
///
/// Separated from the transport, because the shape of somebody else's JSON is exactly the thing
/// that changes between releases and exactly the thing nobody notices has changed until a fleet
/// stops pairing — the same reason [`crate::observe::parse_rauc_status`] is its own function.
pub fn parse_enrolled(text: &str) -> Result<Enrolled, EnrolError> {
    let document = Json::parse(text).map_err(|error| EnrolError {
        code: "unreadable",
        detail: format!(
            "the Hub answered with something that is not JSON ({error}). Nothing has been stored."
        ),
    })?;
    let text_of = |key: &str| {
        document
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };

    let credential = text_of("credential");
    if credential.is_empty() {
        return Err(EnrolError {
            code: "no_credential",
            detail: "the Hub accepted the code and returned no credential, so there is nothing to \
                     pair with. Nothing has been stored."
                .to_string(),
        });
    }

    Ok(Enrolled {
        device_id: text_of("device_id"),
        namespace: text_of("namespace"),
        credential,
        // Absent and `null` both mean an opaque token, which `check_identity` reports as a warning
        // saying the scope check runs on the Hub. An empty array is a different fact — a credential
        // declaring it may do nothing — and it survives as one.
        scopes: document.get_present("scopes").map(|scopes| {
            scopes
                .as_arr()
                .map(|rows| {
                    rows.iter()
                        .filter_map(Json::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        }),
    })
}

/// The one HTTPS request §10 needs, over [`crate::connectivity::post_json`].
#[derive(Debug, Clone)]
pub struct CurlEnrolment {
    /// The Hub's base URL, as the operator gave it.
    pub endpoint: String,
    /// Where a request body may be staged. Under `state/`, so it is on the robot's own disk rather
    /// than in a world-writable temporary directory.
    pub scratch: PathBuf,
}

impl CurlEnrolment {
    /// Where a pairing code is presented.
    pub fn url(&self) -> String {
        format!("{}/devices/pair", self.endpoint.trim_end_matches('/'))
    }
}

impl Enrolment for CurlEnrolment {
    fn present(&self, code: &str, request: &Enrol) -> Result<Enrolled, EnrolError> {
        let body = request
            .body(code)
            .to_canonical()
            .map_err(|error| EnrolError {
                code: "internal",
                detail: format!("the request body could not be serialised: {error:?}"),
            })?;

        // No credential: pairing is the exchange that *obtains* one. The code is in the body, which
        // is the half of `post_json` that never reaches an argument list either.
        let posted = crate::connectivity::post_json(&self.url(), None, &body, &self.scratch)
            .map_err(|detail| EnrolError {
                code: "unreachable",
                detail,
            })?;

        if !(200..300).contains(&posted.status) {
            // The Hub's own message, when it sent one. A refusal that says only "the Hub said no"
            // sends an operator to a web console to find out what this request already knows.
            let said = Json::parse(&posted.body)
                .ok()
                .and_then(|document| {
                    document
                        .get("error")
                        .and_then(Json::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_default();
            return Err(EnrolError {
                code: "refused",
                detail: if said.is_empty() {
                    format!("the Hub refused the code with HTTP {}.", posted.status)
                } else {
                    format!(
                        "the Hub refused the code with HTTP {}: {said}",
                        posted.status
                    )
                },
            });
        }

        parse_enrolled(&posted.body)
    }

    fn describe(&self) -> String {
        format!("{}, over HTTPS via curl", self.url())
    }

    fn endpoint(&self) -> String {
        self.endpoint.trim_end_matches('/').to_string()
    }
}

/// An exchange that happens entirely in memory, for the suite.
///
/// Not a development convenience: every refusal in [`pair`] below is reachable only through a Hub
/// that answers a particular way, and a suite that could not produce those answers would be a suite
/// that tests the happy path and the local code check.
#[derive(Debug, Clone)]
pub struct RecordingEnrolment {
    /// What to answer with.
    pub answer: Result<Enrolled, EnrolError>,
    /// Where this pretends to be.
    pub endpoint: String,
    /// What it was asked, in order. One entry per call.
    pub presented: std::cell::RefCell<Vec<(String, Enrol)>>,
}

impl RecordingEnrolment {
    /// One that accepts, returning `enrolled`.
    pub fn accepting(enrolled: Enrolled) -> RecordingEnrolment {
        RecordingEnrolment {
            answer: Ok(enrolled),
            endpoint: "https://hub.example".to_string(),
            presented: std::cell::RefCell::new(Vec::new()),
        }
    }

    /// One that refuses.
    pub fn refusing(code: &'static str, detail: &str) -> RecordingEnrolment {
        RecordingEnrolment {
            answer: Err(EnrolError {
                code,
                detail: detail.to_string(),
            }),
            endpoint: "https://hub.example".to_string(),
            presented: std::cell::RefCell::new(Vec::new()),
        }
    }
}

impl Enrolment for RecordingEnrolment {
    fn present(&self, code: &str, request: &Enrol) -> Result<Enrolled, EnrolError> {
        self.presented
            .borrow_mut()
            .push((code.to_string(), request.clone()));
        self.answer.clone()
    }

    fn describe(&self) -> String {
        "a recording exchange — nothing leaves this machine".to_string()
    }

    fn endpoint(&self) -> String {
        self.endpoint.clone()
    }
}

/// Why a pairing did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairError {
    /// A code.
    pub code: &'static str,
    /// The sentence an operator gets. **Never contains the code they typed.**
    pub detail: String,
}

impl std::fmt::Display for PairError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.detail)
    }
}

impl std::error::Error for PairError {}

/// What a completed pairing produced, for the operator to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    /// The Hub's identifier for this device.
    pub device_id: String,
    /// The namespace it now belongs to.
    pub namespace: String,
    /// Where it will report.
    pub endpoint: String,
    /// What §10 finds about the identity that was just written.
    pub verdict: IdentityVerdict,
}

/// §10's one-time operator action.
///
/// The order is the argument. The code is checked **here**, before any network exists, because a
/// mistyped code should cost a sentence rather than a round trip and a line in somebody's failed-
/// pairing log. Then the robot is checked — a paired robot does not re-pair, because doing it
/// silently moves a machine between fleets with no record of which one it left. Only then is
/// anything sent.
///
/// # What is refused after the Hub has answered, and what is not
///
/// The answer is untrusted input like any other, so [`check_identity`] runs over it before it is
/// stored. Two classes of finding come out of that and they are treated differently:
///
/// * **Findings about the credential** — no namespace, the wrong class, a scope nobody recognises —
///   are refused. The Hub has just handed this robot something §10 and §19 say it should not have,
///   and storing it and complaining afterwards would leave the credential on the disk.
/// * **Findings about the machine** — [`IdentityFinding::KeyNotAttested`], which §50.4 says is the
///   expected result of a mainboard swap — are reported and do not refuse. Whether to adopt a robot
///   whose board was replaced is an operator's decision, and a pairing that refused it would leave
///   a repaired fleet unpairable until somebody found the vendor.
pub fn pair(
    layout: &Layout,
    trust: &TrustStore,
    typed: &str,
    enrolment: &dyn Enrolment,
    audit: &crate::audit::Audit,
    at: &str,
    actor: &str,
) -> Result<Paired, PairError> {
    let result = normalize_pairing_code(typed);
    let code = match (result.code, result.fault) {
        (Some(code), _) => code,
        (None, Some(fault)) => {
            return Err(PairError {
                code: fault.code.as_str(),
                detail: fault.detail,
            })
        }
        (None, None) => {
            return Err(PairError {
                code: "internal",
                detail: "the code was neither accepted nor refused".to_string(),
            })
        }
    };

    let existing = read(layout);
    if existing.identity.credential {
        return Err(PairError {
            code: "already_paired",
            detail: format!(
                "this robot already holds a device credential{}. Pairing again would move it \
                 between fleets with nothing recording which one it left — run `skynet unpair` \
                 first, which is audited.",
                match existing.identity.namespace.trim() {
                    "" => String::new(),
                    namespace => format!(" for `{namespace}`"),
                }
            ),
        });
    }

    let observed = read_observed(layout, trust);
    let certificate = crate::birth::read(layout, trust).ok().flatten();
    let request = Enrol {
        device_key: observed.birth_key.clone(),
        model: certificate
            .as_ref()
            .map(|held| held.certificate.model.clone())
            .unwrap_or_default(),
        serial: certificate
            .as_ref()
            .map(|held| held.certificate.serial.clone())
            .unwrap_or_default(),
        agent: format!("skynetd/{}", crate::api::VERSION),
    };

    let enrolled = enrolment
        .present(&code, &request)
        .map_err(|error| PairError {
            code: error.code,
            detail: error.detail,
        })?;

    let candidate = Identity {
        device_id: enrolled.device_id.clone(),
        namespace: enrolled.namespace.clone(),
        device_key: observed.birth_key.clone(),
        // Never claimed. See the module header: nothing here measures where the private half lives.
        binding: None,
        credential: true,
        credential_prefix: prefix_of(&enrolled.credential),
        scopes: enrolled.scopes.clone(),
    };
    let verdict = check_identity(&candidate, &observed);

    let refusing: Vec<&skynet_contracts::identity::Finding> = verdict
        .findings
        .iter()
        .filter(|finding| {
            finding.severity == skynet_contracts::admission::Severity::Error
                && finding.code != IdentityFinding::KeyNotAttested
        })
        .collect();
    if !refusing.is_empty() {
        return Err(PairError {
            code: "credential_refused",
            detail: format!(
                "the Hub returned a credential this robot will not store: {}. Nothing has been \
                 written, and the credential should be treated as issued — revoke it.",
                refusing
                    .iter()
                    .map(|finding| finding.message.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        });
    }

    // The document first and the credential second, deliberately. Whether a robot is paired is read
    // off the credential (see the module header), so an interrupted write leaves a robot that is
    // not paired and can pair again — rather than one claiming a fleet it has no credential for.
    let mut document = Json::obj();
    document
        .set("device_id", Json::Str(enrolled.device_id.clone()))
        .set("endpoint", Json::Str(enrolment.endpoint()))
        .set("namespace", Json::Str(enrolled.namespace.clone()))
        .set("device_key", Json::Str(observed.birth_key.clone()));
    if let Some(scopes) = &enrolled.scopes {
        document.set(
            "scopes",
            Json::Arr(scopes.iter().map(|s| Json::Str(s.clone())).collect()),
        );
    }
    let serialised = document.to_canonical().map_err(|error| PairError {
        code: "internal",
        detail: format!("the identity document could not be serialised: {error:?}"),
    })?;
    crate::observe::write_atomic(&layout.device(), serialised.as_bytes()).map_err(|error| {
        PairError {
            code: "io",
            detail: format!(
                "{} could not be written: {error}",
                layout.device().display()
            ),
        }
    })?;

    write_credential(&layout.credential(), &enrolled.credential).map_err(|error| PairError {
        code: "io",
        detail: format!(
            "{} could not be written: {error}. This robot is not paired — the credential the Hub \
             minted should be revoked.",
            layout.credential().display()
        ),
    })?;

    // Audited, for §19's reason: pairing is the change that decides who may tell this robot what to
    // do. The subject is the namespace and the detail names the endpoint; **the credential is in
    // neither**, and the audit log is a file an observer credential can read.
    let _ = audit.append(
        at,
        actor,
        "identity.pair",
        &enrolled.namespace,
        &format!(
            "paired as `{}` with {}",
            enrolled.device_id,
            enrolment.endpoint()
        ),
        None,
    );

    Ok(Paired {
        device_id: enrolled.device_id,
        namespace: enrolled.namespace,
        endpoint: enrolment.endpoint(),
        verdict,
    })
}

/// Write the credential with an owner-only mode where the platform has one.
///
/// The mode is set **before** the bytes are written rather than after, because a file created
/// world-readable and chmod-ed afterwards is world-readable for the length of a write — which is
/// short, and is exactly as long as a local process reading in a loop needs.
fn write_credential(path: &Path, credential: &str) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(credential.as_bytes())?;
    file.write_all(b"\n")
}

/// Undo a pairing (§39.1 gives it to Admin, beside pair).
///
/// Removes the credential and the document, in that order: the credential is what makes a robot
/// paired, so removing it first means an interruption leaves a robot that is unpaired with a stale
/// document rather than one that claims a fleet it can no longer reach.
///
/// **The Hub is not told.** A robot that has been unpaired may have been unpaired precisely because
/// it cannot reach its Hub, and an unpair that required a round trip would be an unpair that fails
/// exactly when it is needed. Revoking the credential is the operator's job and the audit entry
/// says so.
pub fn unpair(
    layout: &Layout,
    audit: &crate::audit::Audit,
    at: &str,
    actor: &str,
) -> Result<Option<String>, PairError> {
    let held = read(layout);
    if !held.identity.credential && held.fault.is_none() {
        return Ok(None);
    }

    let namespace = held.identity.namespace.clone();
    for path in [layout.credential(), layout.device()] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(PairError {
                    code: "io",
                    detail: format!("{} could not be removed: {error}", path.display()),
                })
            }
        }
    }

    let _ = audit.append(
        at,
        actor,
        "identity.unpair",
        if namespace.is_empty() {
            "<unnamed>"
        } else {
            &namespace
        },
        "the credential and the identity document were removed. Revoke the credential on the Hub — \
         nothing here can, and this robot may have been unpaired because it cannot reach one.",
        None,
    );

    Ok(Some(namespace))
}

// ── §10's image attestation ──────────────────────────────────────────────────

/// What the caller found out about the running policy.
///
/// Three variants because [`Measure`] has three states and the caller is the only party that can
/// tell them apart. A signature taking `Option<&str>` would collapse *"nothing is running"* into
/// *"I could not tell"*, which is the collapse §37.1 spends a paragraph refusing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunningPolicy {
    /// Looked, and nothing is running.
    None,
    /// Running, and this is its digest.
    Running {
        /// Package id, for the sentence.
        id: String,
        /// Its digest.
        digest: String,
    },
    /// Could not tell, and this is why.
    Unknown(String),
}

/// Which policy is running, observed rather than remembered (§33.3).
///
/// Read from the units the supervisor reports and the manifests in the store, not from the desired
/// state. A robot running a policy the Hub never asked for is exactly the robot §10's attestation
/// exists for, and an implementation that answered from the desired state would report the software
/// somebody intended instead of the software that is executing.
///
/// # Two policies is not one policy
///
/// §10's sentence names *"policy `sha256:…`"*, singular, and nothing in the design says a robot runs
/// one — §43 leaves multi-robot on one compute open, and nothing forbids two policies today. So two
/// running policies produce [`RunningPolicy::Unknown`] naming both, rather than one of them chosen
/// by iteration order. A check-in that silently picked the first would attest software while
/// omitting software, which is worse than attesting nothing.
pub fn running_policy(
    store: &crate::store::Store,
    actual: &skynetd::actual::Actual,
) -> RunningPolicy {
    let mut running: Vec<(String, String)> = Vec::new();
    let mut unreadable: Vec<String> = Vec::new();

    for unit in &actual.units {
        if unit.state != skynetd::actual::UnitState::Running {
            continue;
        }
        let Some(installed) = actual.installed(&unit.id) else {
            unreadable.push(format!(
                "`{}` is running and the store has no entry for it",
                unit.id
            ));
            continue;
        };
        match store.manifest(&installed.digest) {
            Ok(manifest) => {
                if manifest.get("kind").and_then(Json::as_str) == Some("policy") {
                    running.push((unit.id.clone(), installed.digest.clone()));
                }
            }
            Err(error) => unreadable.push(format!(
                "`{}`'s manifest could not be read ({error}), so nothing here can say whether it \
                 is a policy",
                unit.id
            )),
        }
    }

    if !unreadable.is_empty() {
        return RunningPolicy::Unknown(unreadable.join("; "));
    }
    match running.len() {
        0 => RunningPolicy::None,
        1 => RunningPolicy::Running {
            id: running[0].0.clone(),
            digest: running[0].1.clone(),
        },
        _ => RunningPolicy::Unknown(format!(
            "{} policies are running — {} — and §10's attestation names one. Reporting either of \
             them would attest software while omitting software",
            running.len(),
            running
                .iter()
                .map(|(id, _)| format!("`{id}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Assemble §10's attestation from what this machine can actually measure.
///
/// Three digests and three different sources, and each has a real way of being unmeasurable:
///
/// * **The OS image.** Its digest comes from the signed release manifest for the version RAUC says
///   is running (§12). A robot running a version whose manifest is not on the disk cannot name its
///   own image, and says so rather than reporting the version as though a version were a digest.
///   A robot running no Skynet image at all — guest tier, §1 — reports `nothing`.
/// * **The policy.** From the caller, which is the only party that knows which package is one.
/// * **The body.** The digest of the embodiment descriptor this robot is being commanded against.
///   A robot with no descriptor has no body, which §27 already has a state for.
pub fn attestation(
    layout: &Layout,
    trust: &TrustStore,
    identity: &Held,
    os_version: &str,
    policy: &RunningPolicy,
) -> Attestation {
    let os_image = if os_version.trim().is_empty() {
        // Short on purpose: this goes inside §10's sentence, and a clause somebody has to scroll
        // is a sentence nobody reads. The two cases it cannot tell apart — a guest-tier robot with
        // no Skynet image, and a host-tier one whose slots would not read — are in the module
        // header, where somebody diagnosing has room for them.
        Measure::unmeasured("no Skynet OS image is reported, and an unreadable slot looks the same")
    } else {
        match crate::ota::read_release(layout, trust, os_version) {
            Ok(Some(release)) if !release.manifest.digest.trim().is_empty() => {
                Measure::known(release.manifest.digest.clone())
            }
            _ => Measure::unmeasured(format!(
                "running `{os_version}`, with no release manifest naming its digest"
            )),
        }
    };

    let policy = match policy {
        RunningPolicy::None => Measure::nothing(),
        RunningPolicy::Running { digest, .. } => Measure::known(digest.clone()),
        RunningPolicy::Unknown(why) => Measure::unmeasured(why.clone()),
    };

    let embodiment = match crate::observe::read_json(&layout.embodiment()) {
        None => Measure::nothing(),
        Some(document) => match crate::embodiment::digest_of(&document) {
            Ok(digest) => Measure::known(format!("sha256:{}", hex(&digest))),
            Err(error) => Measure::unmeasured(format!(
                "a descriptor is loaded and its digest could not be taken: {error}"
            )),
        },
    };

    Attestation {
        device_id: identity.identity.device_id.clone(),
        os_image,
        policy,
        embodiment,
    }
}

/// The attestation as an API answer, sentence and all.
///
/// **One line, because this shape now belongs to the contract.** It was written here first, and for
/// two sessions the LAN console could see the three digests and the Hub could not — the check-in
/// carried [`skynet_contracts::identity::attestation_report_to_json`]'s sentence and nothing else.
/// That is finding 23 in its other form: two callers had the same rule, one of them was written
/// second, and the one that mattered got the smaller half. There is one shape now and both read it.
pub fn attestation_to_json(attestation: &Attestation) -> Json {
    skynet_contracts::identity::attestation_to_json(attestation)
}

/// One pairing code, checked and never echoed — for the CLI's `--check` and for tests.
pub fn check_code(typed: &str) -> Json {
    code_result_to_json(&normalize_pairing_code(typed))
}

/// Everything §34.2's sender needs, or `None` when this robot cannot report to anybody.
///
/// **The one function in this daemon that returns the credential**, and it is deliberately shaped
/// so that a caller cannot get it by accident: it returns all three or nothing, so there is no way
/// to ask for the secret alone and no reason for any other code path to hold one. Every other
/// reader of this module gets [`Held`], which carries the prefix and not the credential.
///
/// `None` for a robot with no credential, no device id, or no endpoint. All three are required to
/// address a report — §17's routes are `/devices/{id}/...` on a host somebody named — and a robot
/// missing any of them has nowhere to send rather than a broken send.
pub fn credential_for_sending(layout: &Layout) -> Option<(String, String, String)> {
    let held = read(layout);
    if !held.identity.credential {
        return None;
    }
    let endpoint = held.endpoint?;
    let device_id = held.identity.device_id.trim().to_string();
    if device_id.is_empty() || endpoint.trim().is_empty() {
        return None;
    }
    let credential = std::fs::read_to_string(layout.credential())
        .ok()?
        .trim()
        .to_string();
    if credential.is_empty() {
        return None;
    }
    Some((endpoint, device_id, credential))
}

/// The prefix a credential must have, re-exported so a surface does not re-spell it.
pub const REQUIRED_PREFIX: &str = DEVICE_TOKEN_PREFIX;

fn hex(digest: &[u8; 32]) -> String {
    String::from_utf8_lossy(&skynet_sha256::hex_of(digest)).into_owned()
}

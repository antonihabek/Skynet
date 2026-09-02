//! §10 — identity, pairing and attestation.
//!
//! Three decisions, and they are three because they are answered by three different parties:
//!
//! 1. **Is this what somebody typed a pairing code?** Answered on the robot, before any network
//!    exists, by [`normalize_pairing_code`].
//! 2. **Is the credential that came back the class §10 asked for?** Answered on the robot, against
//!    the credential itself, by [`check_identity`].
//! 3. **What is this robot running?** Answered by [`attest`], which produces the sentence §10 says
//!    an incident investigation needs and nobody can currently produce.
//!
//! # The two artifacts, and why the first check is that they were not confused
//!
//! Pairing is an exchange of one short-lived secret for one long-lived one: a **pairing code** goes
//! to the Hub and a **device credential** comes back. Both are bearer secrets, both are strings, and
//! an operator holding both will eventually paste the wrong one into `skynet pair`. A credential
//! presented as a code is sent to a Hub in a field nobody expected a credential in, and lands in
//! whatever log that Hub keeps of failed pairings — so it is refused here, by name, before anything
//! is sent.
//!
//! # No refusal ever echoes the code
//!
//! A pairing code is a bearer secret for the length of a pairing. A refusal that quotes it puts it
//! in a terminal scrollback, a shell history and the audit log of a robot the operator is standing
//! in front of. So the faults below carry a **position** and never a character, and never the input.
//! That costs the operator a little — *"character five is not one a code uses"* is worse advice than
//! naming the character — and it is the right trade for a secret whose whole lifetime is the minute
//! somebody is typing it.
//!
//! # There is no check character, and that is a decision rather than an omission
//!
//! §18 writes a code out in full: `skynet pair ABCD-1234`. Eight characters in two groups of four,
//! and no room in them for a checksum. A check character could be added and would turn a mistyped
//! code into a local refusal instead of a round trip — and it would make the specification's own
//! example invalid, which is this module overruling the document for the convenience of a better
//! error message. So the local check is the **shape**, and whether a well-shaped code exists is the
//! Hub's question, because the Hub is the only party that can answer it.
//!
//! # Pure
//!
//! No clock, no network, no secure element. Whether a key lives in one arrives as an observation,
//! the way a signature state arrives decided in §37.1 — the robot's own answer to that question and
//! the Hub's answer from a check-in are then the same function over the same facts.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

use crate::admission::Severity;

/// The alphabet a pairing code is drawn from — Crockford's base32.
///
/// Excludes `I`, `L`, `O` and `U`. The first three because a code is read off a screen and typed on
/// a keyboard, and 1/I/l and 0/O are the two confusions that account for most of it; `U` because
/// Crockford drops it so that a random code cannot spell something an operator has to read aloud to
/// a colleague.
///
/// The excluded letters are not rejected on input — they are **folded** by [`fold`], because an
/// operator who types the letter they saw should get the code they saw.
pub const PAIRING_ALPHABET: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// How many characters a pairing code has, hyphens and spacing removed.
///
/// Eight, from §18's `skynet pair ABCD-1234`. Forty bits, which is a large number of guesses
/// against a code that lives for minutes behind a Hub that rate-limits, and a small number of
/// characters for somebody reading them off a screen — which is the trade the length is actually
/// making.
pub const PAIRING_CODE_LENGTH: usize = 8;

/// How a pairing code is written for a person to read: two groups of four.
pub const PAIRING_CODE_GROUP: usize = 4;

/// The prefix on a Hub device credential (§10).
///
/// Here so that a credential pasted into `skynet pair` is recognised as one and refused by name.
pub const DEVICE_TOKEN_PREFIX: &str = "skd_";

/// Why a string is not a pairing code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeFaultCode {
    /// Nothing was given.
    Empty,
    /// This is the credential pairing hands back, not the code it takes.
    LooksLikeACredential,
    /// Fewer than [`PAIRING_CODE_LENGTH`] characters.
    TooShort,
    /// More than [`PAIRING_CODE_LENGTH`] characters.
    TooLong,
    /// A character no code uses, even after folding.
    NotInAlphabet,
}

impl CodeFaultCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            CodeFaultCode::Empty => "empty",
            CodeFaultCode::LooksLikeACredential => "looks_like_a_credential",
            CodeFaultCode::TooShort => "too_short",
            CodeFaultCode::TooLong => "too_long",
            CodeFaultCode::NotInAlphabet => "not_in_alphabet",
        }
    }
}

/// Every code fault, in the order the checks run.
pub const CODE_FAULTS: [CodeFaultCode; 5] = [
    CodeFaultCode::Empty,
    CodeFaultCode::LooksLikeACredential,
    CodeFaultCode::TooShort,
    CodeFaultCode::TooLong,
    CodeFaultCode::NotInAlphabet,
];

/// Why this is not a pairing code.
///
/// A struct of its own rather than three fields on [`CodeResult`], and the reason is the shape the
/// rest of the corpus already uses: a `fault` that is either null or `{ code, detail, ... }` is
/// what frame validation and the cell descriptor emit, and it is the shape the §64 refusal check
/// reads. Flat fields would have made every refusal in this module invisible to that check while
/// looking exactly as correct — which is the failure mode a cross-cutting check exists to catch,
/// and it caught this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeFault {
    /// What.
    pub code: CodeFaultCode,
    /// The sentence an operator gets. Never contains the input.
    pub detail: String,
    /// 1-based, into the normalized code. `None` unless the code is
    /// [`CodeFaultCode::NotInAlphabet`].
    pub position: Option<usize>,
}

/// What a robot made of the characters somebody typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeResult {
    /// The normalized code, or `None` when there is a fault. Upper case, no hyphens.
    pub code: Option<String>,
    /// What is wrong, or `None` when nothing is.
    pub fault: Option<CodeFault>,
}

/// Crockford's folding: the letters excluded from the alphabet map to the digits they resemble.
fn fold(character: char) -> char {
    match character {
        'O' => '0',
        'I' | 'L' => '1',
        other => other,
    }
}

/// Turn what somebody typed into a pairing code, or say why it is not one.
///
/// The order of the checks is the order of usefulness, and it is not the order they would fall out
/// of a parser. A credential pasted here is caught **before** anything is stripped, because `skd_`
/// contains an underscore and would otherwise be reported as an unusable character at position four
/// — which is true, useless, and would have the operator hunting for a typo in a string that has
/// none. Length is checked before the alphabet for the same reason: somebody who typed seven
/// characters needs to be told they typed seven.
pub fn normalize_pairing_code(input: &str) -> CodeResult {
    // ASCII-lowercased rather than `to_lowercase`, which allocates and would fold characters no
    // credential prefix contains. The prefix is four ASCII bytes and the comparison is about them.
    let trimmed = input.trim();
    let head: String = trimmed
        .chars()
        .take(DEVICE_TOKEN_PREFIX.len())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if head == DEVICE_TOKEN_PREFIX {
        return CodeResult {
            code: None,
            fault: Some(CodeFault {
                code: CodeFaultCode::LooksLikeACredential,
                detail: format!(
                    "This is a device credential — it begins `{DEVICE_TOKEN_PREFIX}` — and \
                     pairing takes the short code the Hub showed you, not the credential it hands \
                     back afterwards. Nothing has been sent. Treat the credential as exposed if it \
                     has been on a screen somebody else could see."
                ),
                position: None,
            }),
        };
    }

    let mut normalized = String::new();
    for character in input.chars() {
        if matches!(character, '-' | ' ' | '\t' | '\n' | '\r') {
            continue;
        }
        for upper in character.to_uppercase() {
            normalized.push(fold(upper));
        }
    }

    if normalized.is_empty() {
        return CodeResult {
            code: None,
            fault: Some(CodeFault {
                code: CodeFaultCode::Empty,
                detail: format!(
                    "No code was given. `skynet pair` takes the eight-character code the Hub \
                     showed you, written as {}-{}.",
                    "X".repeat(PAIRING_CODE_GROUP),
                    "X".repeat(PAIRING_CODE_GROUP)
                ),
                position: None,
            }),
        };
    }

    // Counted in characters, not bytes. Somebody who pasted eight kanji has typed eight things and
    // is told so, and then told which of them is not in the alphabet — rather than being told they
    // typed twenty-four, which would be a sentence about an encoding they did not choose.
    let length = normalized.chars().count();
    if length != PAIRING_CODE_LENGTH {
        return CodeResult {
            code: None,
            fault: Some(CodeFault {
                code: if length < PAIRING_CODE_LENGTH {
                    CodeFaultCode::TooShort
                } else {
                    CodeFaultCode::TooLong
                },
                detail: format!(
                    "A pairing code is {PAIRING_CODE_LENGTH} characters and this is {length}. \
                     Hyphens and spaces do not count towards it."
                ),
                position: None,
            }),
        };
    }

    for (index, character) in normalized.chars().enumerate() {
        if !PAIRING_ALPHABET.contains(character) {
            return CodeResult {
                code: None,
                fault: Some(CodeFault {
                    code: CodeFaultCode::NotInAlphabet,
                    detail: format!(
                        "Character {} of the {PAIRING_CODE_LENGTH} is not one a pairing code \
                         uses. Codes are digits and upper-case letters, without I, L, O or U — if \
                         you read one of those off the screen it was a 1 or a 0, and typing it \
                         that way works too.",
                        index + 1
                    ),
                    position: Some(index + 1),
                }),
            };
        }
    }

    CodeResult {
        code: Some(normalized),
        fault: None,
    }
}

/// Render a normalized code the way it is printed for a person.
pub fn format_pairing_code(code: &str) -> String {
    let characters: Vec<char> = code.chars().collect();
    let mut out = String::new();
    for (index, character) in characters.iter().enumerate() {
        if index > 0 && index % PAIRING_CODE_GROUP == 0 {
            out.push('-');
        }
        out.push(*character);
    }
    out
}

// ── §19's device credential ──────────────────────────────────────────────────

/// What a device credential may carry, as this edition reads §17.
///
/// **The direction is §19's and the vocabulary is not.** §19 says a device credential *"cannot read
/// repositories, claim jobs, or write to another namespace"*, and gives no list of scope names — the
/// Hub has not declared one. So this list is §17's and §57's device-facing routes, one scope each,
/// and it is a **default this edition freezes because a robot has to check something**, in the same
/// sense [`crate::ota::DEFAULT_BOOT_BUDGET_MS`] is a default rather than a derived constant.
///
/// What is *not* a default is the direction. This is an allow-list and anything outside it is
/// refused, for exactly the reason §12 gates an unrecognised OS component: a deny-list of §19's
/// three phrases would be got past by a credential that spelled repository read any other way, and
/// the one thing §19 says about this credential is that it is **assumed lost**. A capability nobody
/// has established is narrow, on a credential everybody assumes is stolen, is the combination the
/// whole clause exists to prevent.
pub const DEVICE_SCOPES: [&str; 8] = [
    "device:calibration",
    "device:checkin",
    "device:conformance",
    "device:desired",
    "device:episodes",
    "device:evaluations",
    "device:incidents",
    "device:releases",
];

/// Where a device key lives (§10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyBinding {
    /// Inside a TPM or secure element. The private half never left it.
    Hardware,
    /// A file on the robot's disk.
    Software,
}

impl KeyBinding {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            KeyBinding::Hardware => "hardware",
            KeyBinding::Software => "software",
        }
    }

    /// Parse one, or `None` for a word this edition does not know.
    pub fn parse(text: &str) -> Option<KeyBinding> {
        match text {
            "hardware" => Some(KeyBinding::Hardware),
            "software" => Some(KeyBinding::Software),
            _ => None,
        }
    }
}

/// Every binding, in the order they are reported.
pub const KEY_BINDINGS: [KeyBinding; 2] = [KeyBinding::Hardware, KeyBinding::Software];

/// The credential this robot holds, and the key it was bound to.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    /// The Hub's identifier for this device.
    pub device_id: String,
    /// The namespace pairing bound this robot to (§10).
    pub namespace: String,
    /// Lower-case hex of the device public key.
    pub device_key: String,
    /// Where the private half lives.
    ///
    /// `None` is *"nobody looked"* and is reported as such. It is not
    /// [`KeyBinding::Software`]: a robot that has not been asked whether it has a secure element
    /// has not established that it lacks one, and §10's `security.identity.attested` is a
    /// measurement rather than an assumption.
    pub binding: Option<KeyBinding>,
    /// Whether a credential is held at all.
    pub credential: bool,
    /// The credential's prefix, when it has one. **Never the credential.**
    pub credential_prefix: String,
    /// The scopes the credential declares, when it declares any.
    ///
    /// `None` is an opaque bearer token whose scopes live only on the Hub, which is the ordinary
    /// case and is not a fault — see [`IdentityFinding::ScopeUndeclared`].
    pub scopes: Option<Vec<String>>,
}

/// What the robot can observe about itself, from §50's certificate and from the hardware.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Observed {
    /// The device public key the vendor attested at manufacture (§50.2), when there is a
    /// certificate and it verified.
    pub birth_key: String,
    /// Whether the certificate was present and trusted at all.
    pub birth_certificate: bool,
}

/// One thing found about a robot's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityFinding {
    /// No credential. The state a robot ships in (§50.2).
    NotPaired,
    /// A credential that names no namespace.
    NamespaceMissing,
    /// A credential of some other class than §10's.
    NotADeviceCredential,
    /// A scope outside the allow-list.
    ScopeUnrecognised,
    /// An opaque token: the scope check cannot run here.
    ScopeUndeclared,
    /// Nobody measured whether the key is in a secure element.
    KeyBindingUnknown,
    /// The key is a file.
    KeyNotHardwareBound,
    /// The paired key is not the key the vendor signed.
    KeyNotAttested,
    /// There is no certificate to check the key against.
    NoBirthCertificate,
}

impl IdentityFinding {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            IdentityFinding::NotPaired => "not_paired",
            IdentityFinding::NamespaceMissing => "namespace_missing",
            IdentityFinding::NotADeviceCredential => "not_a_device_credential",
            IdentityFinding::ScopeUnrecognised => "scope_unrecognised",
            IdentityFinding::ScopeUndeclared => "scope_undeclared",
            IdentityFinding::KeyBindingUnknown => "key_binding_unknown",
            IdentityFinding::KeyNotHardwareBound => "key_not_hardware_bound",
            IdentityFinding::KeyNotAttested => "key_not_attested",
            IdentityFinding::NoBirthCertificate => "no_birth_certificate",
        }
    }
}

/// Every identity finding, in the order the checks run.
pub const IDENTITY_FINDINGS: [IdentityFinding; 9] = [
    IdentityFinding::NotPaired,
    IdentityFinding::NamespaceMissing,
    IdentityFinding::NotADeviceCredential,
    IdentityFinding::ScopeUnrecognised,
    IdentityFinding::ScopeUndeclared,
    IdentityFinding::KeyBindingUnknown,
    IdentityFinding::KeyNotHardwareBound,
    IdentityFinding::KeyNotAttested,
    IdentityFinding::NoBirthCertificate,
];

/// One finding, with the sentence an operator reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// What.
    pub code: IdentityFinding,
    /// Whether it stops the robot being a paired, attested member of a fleet.
    pub severity: Severity,
    /// A sentence naming the fix (§64).
    pub message: String,
    /// The scopes or keys it is about.
    pub subjects: Vec<String>,
}

/// §10's answer about one robot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityVerdict {
    /// Whether this robot holds a device credential and knows whose it is.
    pub paired: bool,
    /// §10's `security.identity.attested`, and it takes **both** halves.
    ///
    /// The key must live in hardware *and* be the key the vendor signed. Either alone is not the
    /// property: a hardware key nobody attested is a key the robot generated for itself and can say
    /// nothing about what the machine is, and an attested key sitting in a file is an attestation
    /// that copies to any machine somebody puts the file on.
    pub attested: bool,
    /// Everything found, in the order the checks run.
    pub findings: Vec<Finding>,
}

fn finding(
    code: IdentityFinding,
    severity: Severity,
    message: String,
    subjects: Vec<String>,
) -> Finding {
    Finding {
        code,
        severity,
        message,
        subjects,
    }
}

/// What state this robot's identity is in, and everything wrong with it.
///
/// Every check runs and the result carries all of them, for [`crate::admit_package`]'s reason: an
/// operator standing in front of a robot fixing one problem per attempt is the failure a finding
/// list exists to prevent, and pairing is a job somebody drove to a warehouse to do.
///
/// **Not paired is not a fault.** §50.2 ships a robot with a birth certificate and no namespace —
/// *"it belongs to nobody yet"* — so an unpaired robot is a robot at the correct point in its
/// lifecycle, and it is reported as a warning naming the command rather than as something broken.
pub fn check_identity(identity: &Identity, observed: &Observed) -> IdentityVerdict {
    let mut findings = Vec::new();
    let namespace = identity.namespace.trim();

    if !identity.credential {
        findings.push(finding(
            IdentityFinding::NotPaired,
            Severity::Warning,
            "This robot holds no device credential, so it belongs to nobody and reports to \
             nothing. That is the state a robot ships in (§50.2). Pair it with `skynet pair` and \
             the code the Hub showed you."
                .to_string(),
            Vec::new(),
        ));
    } else {
        if namespace.is_empty() {
            findings.push(finding(
                IdentityFinding::NamespaceMissing,
                Severity::Error,
                "This robot holds a credential and names no namespace. §10 pairs a robot to a \
                 namespace, so a credential without one has bound it to nothing — and every report \
                 it sends is addressed to a fleet nobody can name."
                    .to_string(),
                Vec::new(),
            ));
        }

        if identity.credential_prefix != DEVICE_TOKEN_PREFIX {
            let seen = if identity.credential_prefix.is_empty() {
                "with nothing recognisable".to_string()
            } else {
                format!("`{}`", identity.credential_prefix)
            };
            findings.push(finding(
                IdentityFinding::NotADeviceCredential,
                Severity::Error,
                format!(
                    "A device credential begins `{DEVICE_TOKEN_PREFIX}` and this one begins \
                     {seen}. §10 puts the narrowest credential class in the system on the robot on \
                     purpose, because a credential physically present in the field is the easiest \
                     one to lose. A wider one here is a wider one lost."
                ),
                Vec::new(),
            ));
        }

        match &identity.scopes {
            None => findings.push(finding(
                IdentityFinding::ScopeUndeclared,
                Severity::Warning,
                "This credential declares no scopes, so nothing on the robot can check that it is \
                 as narrow as §19 requires. That is the ordinary shape of an opaque bearer token \
                 and is not a fault — it means `security.credential.scope` is answered on the Hub \
                 and not here."
                    .to_string(),
                Vec::new(),
            )),
            Some(scopes) => {
                let mut unrecognised: Vec<String> = Vec::new();
                for scope in scopes {
                    if !DEVICE_SCOPES.contains(&scope.as_str())
                        && !unrecognised.iter().any(|held| held == scope)
                    {
                        unrecognised.push(scope.clone());
                    }
                }
                unrecognised.sort();
                if !unrecognised.is_empty() {
                    findings.push(finding(
                        IdentityFinding::ScopeUnrecognised,
                        Severity::Error,
                        format!(
                            "This credential carries {}, which is not a capability a device \
                             credential is recognised to need. §19 assumes this credential is \
                             lost, and a capability nobody has established is narrow, on a \
                             credential everybody assumes is stolen, is refused rather than \
                             allowed.",
                            unrecognised.join(", ")
                        ),
                        unrecognised,
                    ));
                }
            }
        }
    }

    // ── §10's second addition: what the key is bound to ────────────────────────
    let key = identity.device_key.trim();
    let birth_key = observed.birth_key.trim();

    match identity.binding {
        None => findings.push(finding(
            IdentityFinding::KeyBindingUnknown,
            Severity::Warning,
            "Nothing has established whether this key lives in a secure element. \
             `security.identity.attested` is a measurement, so an unmeasured robot is reported \
             unmeasured rather than counted as either answer."
                .to_string(),
            Vec::new(),
        )),
        Some(KeyBinding::Hardware) => {}
        Some(KeyBinding::Software) => findings.push(finding(
            IdentityFinding::KeyNotHardwareBound,
            Severity::Warning,
            "This device key is a file rather than a secret inside a secure element, so it leaves \
             with a copy of the filesystem and identifies whatever machine that copy is put on. \
             That is the ordinary state at guest tier and it is the ceiling §10 puts on \
             `security.identity.attested`."
                .to_string(),
            Vec::new(),
        )),
    }

    if !observed.birth_certificate {
        findings.push(finding(
            IdentityFinding::NoBirthCertificate,
            Severity::Warning,
            "This robot holds no trusted birth certificate, so there is nothing to check the \
             device key against. §50.3: without one, what this machine is remains a claim it makes \
             about itself."
                .to_string(),
            Vec::new(),
        ));
    } else if !birth_key.is_empty() && !key.is_empty() && birth_key != key {
        findings.push(finding(
            IdentityFinding::KeyNotAttested,
            Severity::Error,
            "The key this robot was paired on is not the key its vendor attested at manufacture. \
             §50.2 separates what it is from whose it is, and this is the seam between them coming \
             apart: either the mainboard was replaced — §50.4, expected, and the remedy is a \
             vendor-signed re-issue — or this robot was paired on a key nobody signed."
                .to_string(),
            alloc::vec![birth_key.to_string()],
        ));
    }

    let attested = identity.binding == Some(KeyBinding::Hardware)
        && observed.birth_certificate
        && !birth_key.is_empty()
        && !key.is_empty()
        && birth_key == key;

    IdentityVerdict {
        paired: identity.credential && !namespace.is_empty(),
        attested,
        findings,
    }
}

// ── §10's third addition: image attestation ──────────────────────────────────

/// The three states one of §10's digests can be in.
///
/// [`MeasureState::Nothing`] and [`MeasureState::Unmeasured`] are kept apart for §37.1's reason,
/// applied to a measurement rather than to a signature: *"there is no policy running"* and *"a
/// policy is running and its digest could not be read"* are two facts with two different people at
/// the end of them, and collapsing them into one missing field would report a robot running unknown
/// software as a robot running none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasureState {
    /// It was measured, and the digest is here.
    Known,
    /// There is nothing of this kind to measure.
    Nothing,
    /// Something is there and the digest could not be read.
    Unmeasured,
}

impl MeasureState {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            MeasureState::Known => "known",
            MeasureState::Nothing => "nothing",
            MeasureState::Unmeasured => "unmeasured",
        }
    }

    /// Parse one, or `None` for a word this edition does not know.
    pub fn parse(text: &str) -> Option<MeasureState> {
        match text {
            "known" => Some(MeasureState::Known),
            "nothing" => Some(MeasureState::Nothing),
            "unmeasured" => Some(MeasureState::Unmeasured),
            _ => None,
        }
    }
}

/// Every measure state.
pub const MEASURE_STATES: [MeasureState; 3] = [
    MeasureState::Known,
    MeasureState::Nothing,
    MeasureState::Unmeasured,
];

/// One of the three digests §10 requires at check-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Measure {
    /// Which of the three states this is in.
    pub state: MeasureState,
    /// `sha256:…`, when the state is [`MeasureState::Known`].
    pub digest: String,
    /// Why it could not be read, when the state is [`MeasureState::Unmeasured`].
    pub why: String,
}

impl Measure {
    /// A measured digest.
    pub fn known(digest: impl Into<String>) -> Measure {
        Measure {
            state: MeasureState::Known,
            digest: digest.into(),
            why: String::new(),
        }
    }

    /// There is nothing of this kind running.
    pub fn nothing() -> Measure {
        Measure {
            state: MeasureState::Nothing,
            digest: String::new(),
            why: String::new(),
        }
    }

    /// Something is there and its digest could not be read.
    pub fn unmeasured(why: impl Into<String>) -> Measure {
        Measure {
            state: MeasureState::Unmeasured,
            digest: String::new(),
            why: why.into(),
        }
    }
}

/// What is running, as §10 asks it to be reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attestation {
    /// The Hub's identifier for this robot, when it has one.
    pub device_id: String,
    /// The running OS image.
    pub os_image: Measure,
    /// The running policy.
    pub policy: Measure,
    /// The embodiment descriptor the robot is being commanded against.
    pub embodiment: Measure,
}

/// §10's sentence, and what is missing from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttestationReport {
    /// Whether every one of the three was measured.
    pub complete: bool,
    /// §10's sentence, with every hole in it rendered as the hole it is.
    pub sentence: String,
    /// The names of the digests that could not be read, sorted.
    pub unmeasured: Vec<String>,
}

/// One clause of §10's sentence, **including its own label**.
///
/// The label belongs to the clause rather than to the template, and that is not a stylistic choice:
/// a template reading `running image {…}` produces ``running image `sha256:aa` `` for a measured
/// image and *"running image an unmeasured OS image"* for one that is not. The first version of this
/// did exactly that, and it was found by opening the console rather than by any assertion here — the
/// bytes were correct, the shape was correct, and the sentence was unreadable.
///
/// So `Known` carries the label and the other two do not, which is how English works: *an unmeasured
/// OS image* already says what it is.
fn render(measure: &Measure, label: &str, noun: &str) -> String {
    match measure.state {
        MeasureState::Known => {
            let digest = measure.digest.trim();
            // A `Known` measure with nothing in it is a caller bug, and it is rendered as the
            // absence it is rather than as an empty pair of backticks somebody reads as a digest of
            // zero length.
            if digest.is_empty() {
                format!("an unnamed {noun}")
            } else {
                format!("{label}`{digest}`")
            }
        }
        MeasureState::Nothing => format!("no {noun}"),
        MeasureState::Unmeasured => {
            let why = measure.why.trim();
            if why.is_empty() {
                format!("an unmeasured {noun}")
            } else {
                format!("an unmeasured {noun} ({why})")
            }
        }
    }
}

/// §10's sentence: *"robot X is running image `sha256:…`, policy `sha256:…`, against body
/// `sha256:…`"*.
///
/// # This function cannot fail, and that is the design
///
/// There is no `Option` and no `Result`. §13's rule for a recording with a hole in it — seal it, and
/// say it is incomplete — is the same rule, because the reason is the same: the report most likely
/// to have a hole in it is the one covering whatever made the hole. A check-in withheld because a
/// digest could not be read is a robot the fleet believes is **offline**, at the exact moment
/// somebody needs to know what it is running, and it converts a named gap into an indistinguishable
/// silence.
///
/// So an attestation with three holes in it is still a sentence, and the sentence says so three
/// times.
///
/// # `Nothing` is a real answer for all three
///
/// Each has a case, and none of them is a stretch. A guest-tier robot runs *"a container beside the
/// vendor stack"* (§1) and has no Skynet OS image. A robot between policies is running none. A robot
/// whose driver has not come up has no body — §27's `kernel: down` — and reporting a body digest for
/// it would attest joint ratings nothing is applying.
pub fn attest(attestation: &Attestation) -> AttestationReport {
    let mut unmeasured: Vec<String> = Vec::new();
    if attestation.embodiment.state == MeasureState::Unmeasured {
        unmeasured.push("embodiment".to_string());
    }
    if attestation.os_image.state == MeasureState::Unmeasured {
        unmeasured.push("os_image".to_string());
    }
    if attestation.policy.state == MeasureState::Unmeasured {
        unmeasured.push("policy".to_string());
    }

    let device = attestation.device_id.trim();
    let subject = if device.is_empty() {
        "This robot"
    } else {
        device
    };

    let sentence = format!(
        "{subject} is running {}, {}, against {}.",
        render(&attestation.os_image, "image ", "OS image"),
        render(&attestation.policy, "policy ", "policy"),
        render(&attestation.embodiment, "body ", "body"),
    );

    AttestationReport {
        complete: unmeasured.is_empty(),
        sentence,
        unmeasured,
    }
}

/// One code result, as the corpus holds it.
pub fn code_result_to_json(result: &CodeResult) -> Json {
    let fault = match &result.fault {
        None => Json::Null,
        Some(fault) => {
            let mut node = Json::obj();
            node.set("code", Json::Str(fault.code.as_str().to_string()))
                .set("detail", Json::Str(fault.detail.clone()))
                .set(
                    "position",
                    match fault.position {
                        Some(position) => Json::Num(position as f64),
                        None => Json::Null,
                    },
                );
            node
        }
    };
    let mut out = Json::obj();
    out.set(
        "code",
        match &result.code {
            Some(code) => Json::Str(code.clone()),
            None => Json::Null,
        },
    )
    .set("fault", fault);
    out
}

/// One identity verdict, as the corpus holds it.
pub fn identity_verdict_to_json(verdict: &IdentityVerdict) -> Json {
    let mut findings = Json::arr();
    for entry in &verdict.findings {
        let mut row = Json::obj();
        row.set("code", Json::Str(entry.code.as_str().to_string()))
            .set("message", Json::Str(entry.message.clone()))
            .set("severity", Json::Str(entry.severity.as_str().to_string()))
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
        findings.push(row);
    }
    let mut out = Json::obj();
    out.set("attested", Json::Bool(verdict.attested))
        .set("findings", findings)
        .set("paired", Json::Bool(verdict.paired));
    out
}

/// One attestation report, as the corpus holds it.
pub fn attestation_report_to_json(report: &AttestationReport) -> Json {
    let mut out = Json::obj();
    out.set("complete", Json::Bool(report.complete))
        .set("sentence", Json::Str(report.sentence.clone()))
        .set(
            "unmeasured",
            Json::Arr(
                report
                    .unmeasured
                    .iter()
                    .map(|s| Json::Str(s.clone()))
                    .collect(),
            ),
        );
    out
}

/// One measure, with the digest **kept** rather than rendered into the sentence.
///
/// `digest` and `why` are present-and-null rather than omitted, for the reason
/// [`crate::checkin::health_report_to_json`] keeps all five of §33.6's fields: a consumer that has
/// to discover which keys this robot happened to fill in is a consumer that will get it wrong on
/// the robot that filled in none.
fn measure_to_json(measure: &Measure) -> Json {
    let mut out = Json::obj();
    out.set("digest", text_or_null(&measure.digest))
        .set("state", Json::Str(measure.state.as_str().to_string()))
        .set("why", text_or_null(&measure.why));
    out
}

fn text_or_null(value: &str) -> Json {
    if value.trim().is_empty() {
        Json::Null
    } else {
        Json::Str(value.to_string())
    }
}

/// The attestation as §10 asks for it: the sentence **and** the three digests it is made of.
///
/// # Why the sentence alone was not enough
///
/// §10 asks the Hub to record *"robot X is running image `sha256:…`, policy `sha256:…`, against
/// body `sha256:…`"*, and calls it "the sentence an incident investigation needs and nobody can
/// currently produce". [`attest`] produces that sentence, and for two sessions it was the whole of
/// what a check-in carried.
///
/// A sentence is not a record. Ask the fleet *"which robots are running image `sha256:aa`"* — the
/// question §32.1 puts a column on — and the only way to answer it from a sentence is to parse
/// English back out of prose that deliberately is not machine-readable: `no policy`, `an unmeasured
/// OS image (the slot could not be read)`, `an unnamed body`. Those renderings are right for a
/// person and useless for an index, and a Hub that regex'd them would be re-deriving a fact the
/// robot had and discarded.
///
/// So both go on the wire. The sentence, because it is what somebody reads at 3 a.m.; the measures,
/// because they are what somebody queries at 3 a.m. — and because a digest that survives only
/// inside a rendering is a digest that a change to the rendering silently deletes.
pub fn attestation_to_json(attestation: &Attestation) -> Json {
    let mut out = attestation_report_to_json(&attest(attestation));
    out.set("embodiment", measure_to_json(&attestation.embodiment))
        .set("os_image", measure_to_json(&attestation.os_image))
        .set("policy", measure_to_json(&attestation.policy));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn paired() -> Identity {
        Identity {
            device_id: "dev_7".to_string(),
            namespace: "acme".to_string(),
            device_key: "aa".to_string(),
            binding: Some(KeyBinding::Hardware),
            credential: true,
            credential_prefix: DEVICE_TOKEN_PREFIX.to_string(),
            scopes: Some(vec!["device:checkin".to_string()]),
        }
    }

    fn attested() -> Observed {
        Observed {
            birth_key: "aa".to_string(),
            birth_certificate: true,
        }
    }

    #[test]
    fn the_documents_own_example_is_a_pairing_code() {
        // §18 writes `skynet pair ABCD-1234`. If this ever stops parsing, either the alphabet or
        // the length has drifted away from the specification's own example.
        let result = normalize_pairing_code("ABCD-1234");
        assert_eq!(result.code.as_deref(), Some("ABCD1234"));
        assert!(result.fault.is_none());
    }

    #[test]
    fn the_confusable_letters_fold_to_the_digits_they_look_like() {
        // Somebody reading `01234567` off a screen types the letters they think they see.
        let result = normalize_pairing_code("OI234567");
        assert_eq!(result.code.as_deref(), Some("01234567"));
    }

    #[test]
    fn lower_case_and_spacing_are_cosmetic() {
        let result = normalize_pairing_code("  abcd 1234\n");
        assert_eq!(result.code.as_deref(), Some("ABCD1234"));
    }

    #[test]
    fn a_credential_pasted_into_pair_is_refused_before_anything_is_sent() {
        let fault = normalize_pairing_code("skd_livetokenvalue")
            .fault
            .expect("a credential is not a code");
        assert_eq!(fault.code, CodeFaultCode::LooksLikeACredential);
        // The whole point: the refusal names the class, never the secret.
        assert!(!fault.detail.contains("livetokenvalue"));
    }

    #[test]
    fn a_credential_is_caught_before_the_length_check_is() {
        // `skd_x` is five characters, so a parser that stripped first would call it too short and
        // send the operator hunting for three missing characters in a credential.
        let result = normalize_pairing_code("skd_x");
        assert_eq!(
            result.fault.map(|fault| fault.code),
            Some(CodeFaultCode::LooksLikeACredential)
        );
    }

    #[test]
    fn no_refusal_ever_contains_the_input() {
        for input in ["", "ABC", "ABCDEFGHIJ", "ABCDEF!U", "  "] {
            let fault = normalize_pairing_code(input)
                .fault
                .expect("none of these is a code");
            let trimmed = input.trim();
            if !trimmed.is_empty() {
                assert!(
                    !fault.detail.contains(trimmed),
                    "the refusal for {input:?} echoed it"
                );
            }
        }
    }

    #[test]
    fn a_character_outside_the_alphabet_is_named_by_position_and_not_by_value() {
        let fault = normalize_pairing_code("ABCDEFGU")
            .fault
            .expect("U is not in the alphabet");
        assert_eq!(fault.code, CodeFaultCode::NotInAlphabet);
        assert_eq!(fault.position, Some(8));
        assert!(!fault.detail.contains("ABCDEFGU"));
    }

    #[test]
    fn hyphens_do_not_count_towards_the_length() {
        assert_eq!(
            normalize_pairing_code("A-B-C-D-1-2-3-4").code.as_deref(),
            Some("ABCD1234")
        );
    }

    #[test]
    fn formatting_round_trips_the_documents_shape() {
        assert_eq!(format_pairing_code("ABCD1234"), "ABCD-1234");
    }

    #[test]
    fn an_unpaired_robot_is_a_warning_and_not_a_failure() {
        let verdict = check_identity(&Identity::default(), &Observed::default());
        assert!(!verdict.paired);
        assert!(verdict
            .findings
            .iter()
            .any(|f| f.code == IdentityFinding::NotPaired && f.severity == Severity::Warning));
        assert!(!verdict
            .findings
            .iter()
            .any(|f| f.severity == Severity::Error));
    }

    #[test]
    fn attested_takes_both_halves() {
        // Hardware, but no certificate to say the vendor ever saw this key.
        let mut identity = paired();
        identity.device_key = "bb".to_string();
        assert!(!check_identity(&identity, &Observed::default()).attested);

        // Attested, but the key is a file — so the attestation copies with the filesystem.
        let mut soft = paired();
        soft.binding = Some(KeyBinding::Software);
        assert!(!check_identity(&soft, &attested()).attested);

        assert!(check_identity(&paired(), &attested()).attested);
    }

    #[test]
    fn a_key_the_vendor_never_signed_is_an_error_naming_the_board_swap() {
        let mut identity = paired();
        identity.device_key = "cc".to_string();
        let verdict = check_identity(&identity, &attested());
        let found = verdict
            .findings
            .iter()
            .find(|f| f.code == IdentityFinding::KeyNotAttested)
            .expect("the seam between §10 and §50");
        assert_eq!(found.severity, Severity::Error);
        assert!(found.message.contains("§50.4"));
        assert_eq!(found.subjects, vec!["aa".to_string()]);
    }

    #[test]
    fn an_unrecognised_scope_is_refused_rather_than_ignored() {
        let mut identity = paired();
        identity.scopes = Some(vec![
            "device:checkin".to_string(),
            "repositories:read".to_string(),
        ]);
        let verdict = check_identity(&identity, &attested());
        let found = verdict
            .findings
            .iter()
            .find(|f| f.code == IdentityFinding::ScopeUnrecognised)
            .expect("§19's security.credential.scope");
        assert_eq!(found.severity, Severity::Error);
        assert_eq!(found.subjects, vec!["repositories:read".to_string()]);
    }

    #[test]
    fn every_declared_device_scope_is_accepted() {
        // The other direction, and it matters: an allow-list that refused its own members would
        // make every correctly issued credential an error.
        let mut identity = paired();
        identity.scopes = Some(DEVICE_SCOPES.iter().map(|s| s.to_string()).collect());
        let verdict = check_identity(&identity, &attested());
        assert!(!verdict
            .findings
            .iter()
            .any(|f| f.code == IdentityFinding::ScopeUnrecognised));
    }

    #[test]
    fn an_opaque_token_is_a_warning_that_says_the_check_did_not_run() {
        let mut identity = paired();
        identity.scopes = None;
        let verdict = check_identity(&identity, &attested());
        let found = verdict
            .findings
            .iter()
            .find(|f| f.code == IdentityFinding::ScopeUndeclared)
            .expect("an opaque bearer token is the ordinary case");
        assert_eq!(found.severity, Severity::Warning);
        assert!(found.message.contains("on the Hub and not here"));
    }

    #[test]
    fn an_attestation_with_three_holes_in_it_is_still_a_sentence() {
        let report = attest(&Attestation {
            device_id: String::new(),
            os_image: Measure::unmeasured("the slot could not be read"),
            policy: Measure::unmeasured(""),
            embodiment: Measure::unmeasured(""),
        });
        assert!(!report.complete);
        assert_eq!(report.unmeasured, vec!["embodiment", "os_image", "policy"]);
        assert!(report.sentence.starts_with("This robot is running"));
        assert!(report.sentence.contains("the slot could not be read"));
    }

    #[test]
    fn nothing_and_unmeasured_read_differently() {
        let nothing = attest(&Attestation {
            device_id: "dev_7".to_string(),
            os_image: Measure::known("sha256:aa"),
            policy: Measure::nothing(),
            embodiment: Measure::known("sha256:bb"),
        });
        assert!(nothing.complete);
        assert_eq!(
            nothing.sentence,
            "dev_7 is running image `sha256:aa`, no policy, against body `sha256:bb`."
        );

        let unmeasured = attest(&Attestation {
            device_id: "dev_7".to_string(),
            os_image: Measure::known("sha256:aa"),
            policy: Measure::unmeasured("the runtime is not answering"),
            embodiment: Measure::known("sha256:bb"),
        });
        assert!(!unmeasured.complete);
        assert_ne!(nothing.sentence, unmeasured.sentence);
    }

    #[test]
    fn a_known_measure_with_no_digest_does_not_render_as_empty_backticks() {
        let report = attest(&Attestation {
            device_id: String::new(),
            os_image: Measure::known(""),
            policy: Measure::nothing(),
            embodiment: Measure::nothing(),
        });
        assert!(report.sentence.contains("an unnamed OS image"));
        assert!(!report.sentence.contains("``"));
    }
}

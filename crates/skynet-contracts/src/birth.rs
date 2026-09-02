//! The birth certificate (§50).
//!
//! §50.3 says what its absence costs, and it is the sentence this module exists to make false:
//!
//! > Without it, *"this is a Unitree G1 with these joint ratings"* is a claim the robot makes about
//! > itself, and the entire safety envelope rests on believing it.
//!
//! A birth certificate is signed **at manufacture**, by the vendor, over a device public key that
//! was generated inside a secure element and never left it. It binds model, serial number, the
//! embodiment descriptor digest, hardware revision, manufacture date, and the factory calibration
//! records.
//!
//! # The distinction the whole section turns on
//!
//! §50.2: *"the birth certificate proves **what it is**; pairing (§10) binds it to **whose it
//! is**. Two separate facts, established by two separate parties, which is correct — the vendor is
//! the only party who can attest to the first and has no business asserting the second."*
//!
//! So a certificate carrying a namespace, an owner or an operator is not a richer certificate. It
//! is a vendor asserting something they are not a party to, and [`check`] reports it — as a
//! **finding about the certificate**, not as a failure of the robot, because the machine is fine
//! and the document is overreaching.
//!
//! # A mismatch is expected behaviour, not a fault
//!
//! §50.4: *"replacing a mainboard produces a robot whose birth certificate no longer matches its
//! hardware. That is the correct behaviour and not a bug — it is the same property that makes a
//! phone's secure enclave meaningful."*
//!
//! This is why the findings below are not a boolean. A robot that had a board swapped is not a
//! robot that failed; it is a robot whose identity needs a vendor-signed re-issue, and the finding
//! says that in the sentence rather than leaving somebody to work it out from `valid: false`. The
//! remedies for *"the serial does not match"*, *"the descriptor digest does not match"* and *"the
//! vendor's key is not one this robot trusts"* are three different phone calls.
//!
//! # Pure, like everything else here
//!
//! No clock, no key, no secure element. The signature state arrives decided, as it does for a
//! package (§37.1) and for an entitlement (§51), and what the robot actually observes about itself
//! arrives as an [`Observed`]. The Hub checking a fleet's certificates from a check-in and a robot
//! checking its own at boot are then the same function over the same facts.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::admission::SignatureState;

/// What a vendor signed at manufacture (§50.2).
///
/// No `Default`, and the compiler is the reason it has none: [`SignatureState`] does not implement
/// it either, because there is no defensible default signature state — `Verified` would be a lie
/// and `Absent` would be a document that looks constructed and attests nothing. Deriving it here
/// would have meant adding it there, which is a change to a type five other modules read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BirthCertificate {
    /// The device public key, generated inside the secure element. Lower-case hex.
    ///
    /// The subject of the whole document: everything else binds *to this key*, and the private half
    /// never left the element, so a certificate is only evidence about the machine holding it.
    pub device_key: String,
    /// The model, as the vendor names it.
    pub model: String,
    /// The serial number.
    pub serial: String,
    /// The digest of the embodiment descriptor this body was measured as.
    ///
    /// The field that closes §50.3. A robot presenting a descriptor whose digest is not this one is
    /// presenting joint ratings nobody attested, and §5's `verified` flag cannot tell you that
    /// because a descriptor can be perfectly well signed and be for a different machine.
    pub descriptor_digest: String,
    /// The hardware revision.
    pub hardware_revision: String,
    /// Manufacture date, as the vendor wrote it. Never parsed here.
    ///
    /// A string rather than an instant on purpose: it is provenance, not a deadline. Nothing in
    /// this module compares it to a clock, and a type that invited it to be compared would invite
    /// the certificate to expire — which §50 never says it does, and which would brick a robot in
    /// a basement for a reason §51.1 already rejects in the commercial case.
    pub manufactured: String,
    /// Digests of the factory calibration records this certificate covers (§45.4).
    ///
    /// §50.3: *"it also gives the factory calibration a provenance chain: the extrinsics measured on
    /// the line are signed by the party that measured them."*
    pub calibration_digests: Vec<String>,
    /// Anything the certificate says about **whose** the robot is.
    ///
    /// Should always be empty. Present as a field, rather than silently discarded at parse, so that
    /// [`check`] can report a vendor who asserted it — see [`Finding::OwnershipAsserted`].
    pub claims_ownership: Vec<String>,
    /// What is known about the vendor's signature over this certificate.
    pub signature: SignatureState,
}

/// What the robot can see about itself, right now.
///
/// Every field is optional because a robot early in boot has seen some of these and not others, and
/// §7.2's rule applies here as everywhere: an absent input makes a check **unavailable**, never
/// passed. A `None` never satisfies a comparison.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Observed {
    /// The public key the secure element actually holds, lower-case hex.
    pub device_key: Option<String>,
    /// The serial the machine reports.
    pub serial: Option<String>,
    /// The digest of the descriptor this robot is running with.
    pub descriptor_digest: Option<String>,
    /// The hardware revision the machine reports.
    pub hardware_revision: Option<String>,
}

/// One thing wrong, or one thing that could not be checked.
///
/// Ordered by how much they cost, and each names its own remedy in [`explain`] because the remedies
/// are genuinely different: a re-issue, a resync, a trust-store change, or a call to the vendor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Finding {
    /// The certificate is not signed by a key this robot trusts, so it attests nothing.
    ///
    /// First, because everything below it is a comparison against a document that may as well have
    /// been written by the robot itself — which is precisely the situation §50.3 describes as the
    /// one a birth certificate exists to end.
    Unattested(SignatureState),
    /// The secure element holds a different key from the one the certificate was issued over.
    ///
    /// The mainboard-swap case (§50.4), and the most serious of the mismatches: every other field
    /// binds *to* this key, so if the key is wrong the document is about another machine.
    DeviceKeyMismatch {
        /// The key the certificate was issued over.
        certificate: String,
        /// The key the secure element holds.
        observed: String,
    },
    /// The serial does not match.
    SerialMismatch {
        /// The serial the certificate names.
        certificate: String,
        /// The serial the machine reports.
        observed: String,
    },
    /// The descriptor this robot is running is not the one the vendor attested.
    ///
    /// §50.3's field. Not necessarily tampering — a vendor publishing an updated descriptor and a
    /// robot fetching it produces exactly this — but it does mean the joint ratings in force are
    /// ones nobody has signed *for this machine*.
    DescriptorMismatch {
        /// The descriptor digest the vendor attested.
        certificate: String,
        /// The descriptor digest this robot is running.
        observed: String,
    },
    /// The hardware revision does not match.
    HardwareRevisionMismatch {
        /// The revision the certificate names.
        certificate: String,
        /// The revision the machine reports.
        observed: String,
    },
    /// The certificate asserts something about ownership, which is not the vendor's to assert.
    OwnershipAsserted(Vec<String>),
    /// A field the certificate binds that the robot could not observe.
    ///
    /// Not a failure of either. §7.2: an absent input makes a check unavailable, and a system that
    /// reported "matches" for a comparison it could not make would be the one thing worse than
    /// reporting nothing.
    Unobservable(&'static str),
    /// The certificate is missing a field it must bind (§50.2).
    Incomplete(&'static str),
}

impl Finding {
    /// Whether this finding means the certificate establishes nothing at all.
    ///
    /// True only for [`Finding::Unattested`] and a mismatched device key. The others narrow what is
    /// attested; these two remove it.
    pub fn voids_the_certificate(&self) -> bool {
        matches!(
            self,
            Finding::Unattested(_) | Finding::DeviceKeyMismatch { .. }
        )
    }

    /// Whatever this finding is about, on the wire.
    ///
    /// Uniform across the eight, and the order within it is load-bearing: for a mismatch it is
    /// **what the certificate says, then what the machine reports**, in that order and never the
    /// reverse. A reader who has to work out which of two hex strings came from the document is a
    /// reader who will get it wrong once.
    ///
    /// This lives on the finding rather than in the encoder because the TypeScript edition carries
    /// no enum — a finding *is* a code and a subject list there — so a projection that disagreed
    /// with this one would be the two editions disagreeing about what a finding is while both
    /// reproducing every code.
    pub fn subjects(&self) -> Vec<String> {
        match self {
            Finding::Unattested(state) => alloc::vec![state.as_str().to_string()],
            Finding::DeviceKeyMismatch {
                certificate,
                observed,
            }
            | Finding::SerialMismatch {
                certificate,
                observed,
            }
            | Finding::DescriptorMismatch {
                certificate,
                observed,
            }
            | Finding::HardwareRevisionMismatch {
                certificate,
                observed,
            } => alloc::vec![certificate.clone(), observed.clone()],
            Finding::OwnershipAsserted(fields) => fields.clone(),
            Finding::Unobservable(field) | Finding::Incomplete(field) => {
                alloc::vec![field.to_string()]
            }
        }
    }

    /// The wire spelling.
    pub fn code(&self) -> &'static str {
        match self {
            Finding::Unattested(_) => "unattested",
            Finding::DeviceKeyMismatch { .. } => "device_key_mismatch",
            Finding::SerialMismatch { .. } => "serial_mismatch",
            Finding::DescriptorMismatch { .. } => "descriptor_mismatch",
            Finding::HardwareRevisionMismatch { .. } => "hardware_revision_mismatch",
            Finding::OwnershipAsserted(_) => "ownership_asserted",
            Finding::Unobservable(_) => "unobservable",
            Finding::Incomplete(_) => "incomplete",
        }
    }
}

/// The sentence an operator gets, naming the remedy.
pub fn explain(finding: &Finding) -> String {
    match finding {
        Finding::Unattested(state) => format!(
            "The birth certificate's signature is {}, so it attests nothing: everything it says \
             about this machine is currently a claim the machine makes about itself, which is the \
             situation a birth certificate exists to end. Register the vendor's key with this \
             robot's trust store.",
            state.as_str()
        ),
        Finding::DeviceKeyMismatch {
            certificate,
            observed,
        } => format!(
            "This certificate was issued over device key {certificate} and the secure element \
             holds {observed}. The document is about a different machine. If the mainboard was \
             replaced this is the expected result and not a fault — §50.4 — and the remedy is a \
             vendor-signed re-issue, which also invalidates the calibrations a board swap could \
             have disturbed."
        ),
        Finding::SerialMismatch {
            certificate,
            observed,
        } => format!(
            "The certificate names serial \"{certificate}\" and this machine reports \
             \"{observed}\". Ask the vendor to re-issue."
        ),
        Finding::DescriptorMismatch {
            certificate,
            observed,
        } => format!(
            "The certificate attests descriptor {certificate} and this robot is running \
             {observed}. The joint ratings in force are ones nobody has signed for this machine. \
             A vendor who has published an updated descriptor should re-issue the certificate over \
             it; until then the envelope rests on the descriptor's own signature rather than on \
             the vendor's attestation that it belongs to this body."
        ),
        Finding::HardwareRevisionMismatch {
            certificate,
            observed,
        } => format!(
            "The certificate names hardware revision \"{certificate}\" and this machine reports \
             \"{observed}\". Either the machine was modified or the certificate is stale; both are \
             a vendor re-issue."
        ),
        Finding::OwnershipAsserted(fields) => format!(
            "This certificate asserts ownership ({}), and ownership is not the vendor's to assert. \
             §50.2 keeps two facts apart: the certificate proves what this robot is, and pairing \
             proves whose it is. The assertion is ignored.",
            fields.join(", ")
        ),
        Finding::Unobservable(field) => format!(
            "The certificate binds {field} and this robot could not read its own. The binding is \
             neither confirmed nor contradicted — it is unchecked, which is not the same as \
             matching."
        ),
        Finding::Incomplete(field) => format!(
            "The certificate does not bind {field}, which §50.2 requires it to. It is not a \
             complete birth certificate and whatever it does bind is all it attests."
        ),
    }
}

/// What checking one certificate against one machine concluded.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Attested {
    /// Everything wrong or unchecked, worst first.
    pub findings: Vec<Finding>,
}

impl Attested {
    /// Whether this certificate attests anything at all about this machine.
    ///
    /// False when the signature cannot be believed or the device key is another machine's. Note
    /// what this is **not**: it is not "may the robot run". §50 establishes identity; it does not
    /// gate motion, and a robot whose certificate is void is a robot with an unattested identity,
    /// which is exactly the state every robot was in before §50 existed.
    pub fn attests(&self) -> bool {
        !self.findings.iter().any(Finding::voids_the_certificate)
    }

    /// Whether every binding the certificate makes was checked and matched.
    pub fn fully_confirmed(&self) -> bool {
        self.findings.is_empty()
    }

    /// The bindings that could not be checked, as their field names.
    pub fn unchecked(&self) -> Vec<&'static str> {
        self.findings
            .iter()
            .filter_map(|finding| match finding {
                Finding::Unobservable(field) => Some(*field),
                _ => None,
            })
            .collect()
    }
}

fn compare(
    certificate: &str,
    observed: &Option<String>,
    field: &'static str,
    mismatch: impl FnOnce(String, String) -> Finding,
    findings: &mut Vec<Finding>,
) {
    if certificate.is_empty() {
        findings.push(Finding::Incomplete(field));
        return;
    }
    match observed {
        None => findings.push(Finding::Unobservable(field)),
        Some(seen) if seen == certificate => {}
        Some(seen) => findings.push(mismatch(certificate.to_string(), seen.clone())),
    }
}

/// Check a birth certificate against what the robot observes about itself (§50).
///
/// The order of the findings is the order of their cost, and the first one is deliberately about
/// the *document* rather than the machine: comparing fields against a certificate nobody signed is
/// comparing the robot to a story it told itself.
pub fn check(certificate: &BirthCertificate, observed: &Observed) -> Attested {
    let mut findings = Vec::new();

    if certificate.signature != SignatureState::Verified {
        findings.push(Finding::Unattested(certificate.signature));
    }

    compare(
        &certificate.device_key,
        &observed.device_key,
        "the device key",
        |certificate, observed| Finding::DeviceKeyMismatch {
            certificate,
            observed,
        },
        &mut findings,
    );
    compare(
        &certificate.serial,
        &observed.serial,
        "a serial number",
        |certificate, observed| Finding::SerialMismatch {
            certificate,
            observed,
        },
        &mut findings,
    );
    compare(
        &certificate.descriptor_digest,
        &observed.descriptor_digest,
        "an embodiment descriptor digest",
        |certificate, observed| Finding::DescriptorMismatch {
            certificate,
            observed,
        },
        &mut findings,
    );
    compare(
        &certificate.hardware_revision,
        &observed.hardware_revision,
        "a hardware revision",
        |certificate, observed| Finding::HardwareRevisionMismatch {
            certificate,
            observed,
        },
        &mut findings,
    );

    if certificate.model.is_empty() {
        // Bound but not observable: a model is what the vendor calls this design, and there is
        // nothing on the machine to compare it against. Its absence is still incomplete.
        findings.push(Finding::Incomplete("a model"));
    }

    if !certificate.claims_ownership.is_empty() {
        findings.push(Finding::OwnershipAsserted(
            certificate.claims_ownership.clone(),
        ));
    }

    Attested { findings }
}

impl BirthCertificate {
    /// An empty certificate whose signature state the caller must state.
    ///
    /// The one argument with no default is the one whose omission would matter, which is the whole
    /// reason this is a constructor rather than a `Default`.
    pub fn unsigned(signature: SignatureState) -> BirthCertificate {
        BirthCertificate {
            device_key: String::new(),
            model: String::new(),
            serial: String::new(),
            descriptor_digest: String::new(),
            hardware_revision: String::new(),
            manufactured: String::new(),
            calibration_digests: Vec::new(),
            claims_ownership: Vec::new(),
            signature,
        }
    }
}

/// Every code [`check`] can produce, in the order of their cost.
///
/// A closed vocabulary rather than whatever the match arms happen to spell, because a consumer
/// deciding what to do about a finding needs the list to be enumerable — and because the order is
/// content: the first entry is about the document rather than the machine.
pub const FINDINGS: [&str; 8] = [
    "unattested",
    "device_key_mismatch",
    "serial_mismatch",
    "descriptor_mismatch",
    "hardware_revision_mismatch",
    "ownership_asserted",
    "unobservable",
    "incomplete",
];

/// The fields §50.2 says a certificate binds, in the order it lists them.
///
/// Exported because two things need the same list and disagreeing about it is how a field quietly
/// stops being required: the reader that decodes a certificate, and whatever eventually issues one.
pub const REQUIRED_FIELDS: [&str; 6] = [
    "device_key",
    "model",
    "serial",
    "descriptor_digest",
    "hardware_revision",
    "manufactured",
];

/// The fields a certificate must never carry (§50.2).
///
/// Ownership is established by pairing, by a different party. A vendor who put a namespace in a
/// birth certificate would be asserting a fact they cannot know, and the robot ignores it — but it
/// says so, because silently ignoring a field means the next vendor adds it too.
pub const OWNERSHIP_FIELDS: [&str; 4] = ["namespace", "owner", "operator", "customer"];

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    const KEY: &str = "79b5562e8fe654f94078b112e8a98ba7901f853ae695bed7e0e3910bad049664";
    const OTHER_KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
    const DESCRIPTOR: &str = "29fe4b8d49ec7c32b6936b2e25c3cbd7b7b96c8bb21ad8de5dce3e44f8d2e543";

    fn certificate() -> BirthCertificate {
        BirthCertificate {
            device_key: KEY.to_string(),
            model: "G1".to_string(),
            serial: "SN-0001".to_string(),
            descriptor_digest: DESCRIPTOR.to_string(),
            hardware_revision: "rev-C".to_string(),
            manufactured: "2026-03-14".to_string(),
            calibration_digests: vec!["cal-1".to_string()],
            claims_ownership: Vec::new(),
            signature: SignatureState::Verified,
        }
    }

    fn machine() -> Observed {
        Observed {
            device_key: Some(KEY.to_string()),
            serial: Some("SN-0001".to_string()),
            descriptor_digest: Some(DESCRIPTOR.to_string()),
            hardware_revision: Some("rev-C".to_string()),
        }
    }

    // ── The happy case, which is the only one with no findings ───────────────

    #[test]
    fn a_signed_certificate_over_this_machine_confirms_everything() {
        let attested = check(&certificate(), &machine());
        assert!(attested.findings.is_empty());
        assert!(attested.attests());
        assert!(attested.fully_confirmed());
        assert!(attested.unchecked().is_empty());
    }

    // ── §50.3: the sentence this module exists to make false ─────────────────

    #[test]
    fn an_unsigned_certificate_attests_nothing_however_well_it_matches() {
        // The findings below it would all "pass", and that is exactly the trap: comparing a machine
        // against a document nobody signed is comparing it to a story it told itself.
        for state in [
            SignatureState::Absent,
            SignatureState::Failed,
            SignatureState::Untrusted,
            SignatureState::Unverifiable,
        ] {
            let attested = check(
                &BirthCertificate {
                    signature: state,
                    ..certificate()
                },
                &machine(),
            );
            assert_eq!(attested.findings, vec![Finding::Unattested(state)]);
            assert!(!attested.attests(), "{state:?}");
            assert!(explain(&attested.findings[0]).contains("attests nothing"));
        }
    }

    #[test]
    fn an_unattested_certificate_is_the_first_finding() {
        // Order matters for a reader scanning a list. The document being unbelievable outranks any
        // field of it disagreeing with the machine.
        let attested = check(
            &BirthCertificate {
                signature: SignatureState::Untrusted,
                serial: "SN-9999".to_string(),
                ..certificate()
            },
            &machine(),
        );
        assert_eq!(attested.findings[0].code(), "unattested");
    }

    // ── §50.4: a mismatch is correct behaviour, not a fault ──────────────────

    #[test]
    fn a_replaced_mainboard_voids_the_certificate_and_the_sentence_says_it_is_expected() {
        let swapped = Observed {
            device_key: Some(OTHER_KEY.to_string()),
            ..machine()
        };
        let attested = check(&certificate(), &swapped);
        assert!(!attested.attests(), "the document is about another machine");

        let finding = attested
            .findings
            .iter()
            .find(|f| f.code() == "device_key_mismatch")
            .expect("the mismatch");
        let sentence = explain(finding);
        assert!(
            sentence.contains("expected result and not a fault"),
            "{sentence}"
        );
        assert!(sentence.contains("re-issue"), "{sentence}");
        assert!(
            sentence.contains("calibrations"),
            "§50.4 invalidates them too: {sentence}"
        );
    }

    #[test]
    fn a_serial_mismatch_narrows_what_is_attested_without_voiding_it() {
        // Unlike the device key. The certificate is still about this machine — the key proves it —
        // and one field of it is stale.
        let attested = check(
            &certificate(),
            &Observed {
                serial: Some("SN-0002".to_string()),
                ..machine()
            },
        );
        assert!(attested.attests());
        assert!(!attested.fully_confirmed());
        assert_eq!(attested.findings.len(), 1);
        assert_eq!(attested.findings[0].code(), "serial_mismatch");
    }

    #[test]
    fn the_three_remedies_are_three_different_sentences() {
        // The reason findings are an enumeration rather than a boolean: "the serial does not
        // match", "the descriptor digest does not match" and "the vendor's key is not trusted" are
        // three different phone calls.
        let unattested = explain(&Finding::Unattested(SignatureState::Untrusted));
        let serial = explain(&Finding::SerialMismatch {
            certificate: "a".to_string(),
            observed: "b".to_string(),
        });
        let descriptor = explain(&Finding::DescriptorMismatch {
            certificate: "a".to_string(),
            observed: "b".to_string(),
        });
        assert!(unattested.contains("trust store"));
        assert!(serial.contains("re-issue"));
        assert!(descriptor.contains("joint ratings"));
        assert_ne!(serial, descriptor);
    }

    // ── §50.3's field: the descriptor digest ─────────────────────────────────

    #[test]
    fn a_descriptor_this_vendor_never_attested_is_reported_even_when_it_is_signed() {
        // The gap §5's `verified` flag cannot see: a descriptor can be perfectly well signed and be
        // for a different machine. This is the only check that notices.
        let attested = check(
            &certificate(),
            &Observed {
                descriptor_digest: Some("f".repeat(64)),
                ..machine()
            },
        );
        assert_eq!(attested.findings.len(), 1);
        assert_eq!(attested.findings[0].code(), "descriptor_mismatch");
        assert!(explain(&attested.findings[0]).contains("nobody has signed for this machine"));
    }

    // ── §50.2: what it is, versus whose it is ────────────────────────────────

    #[test]
    fn a_vendor_asserting_ownership_is_reported_and_ignored() {
        // §50.2: the vendor is the only party who can attest to what this robot is, and has no
        // business asserting whose it is. Reported rather than silently dropped, because silently
        // ignoring a field means the next vendor adds it too.
        let attested = check(
            &BirthCertificate {
                claims_ownership: vec!["namespace".to_string(), "operator".to_string()],
                ..certificate()
            },
            &machine(),
        );
        assert!(
            attested.attests(),
            "the machine is fine; the document overreached"
        );
        let finding = attested
            .findings
            .iter()
            .find(|f| f.code() == "ownership_asserted")
            .expect("the finding");
        let sentence = explain(finding);
        assert!(
            sentence.contains("not the vendor's to assert"),
            "{sentence}"
        );
        assert!(sentence.contains("ignored"), "{sentence}");
    }

    #[test]
    fn the_ownership_field_names_are_written_down_once() {
        // So a decoder and whatever eventually issues a certificate cannot disagree about which
        // fields are forbidden.
        assert!(OWNERSHIP_FIELDS.contains(&"namespace"));
        assert!(OWNERSHIP_FIELDS.contains(&"owner"));
        assert_eq!(REQUIRED_FIELDS.len(), 6);
        assert!(REQUIRED_FIELDS.contains(&"descriptor_digest"));
    }

    // ── §7.2: an absent input is unavailable, never passed ───────────────────

    #[test]
    fn a_field_the_robot_cannot_observe_is_unchecked_and_not_matched() {
        // The one thing worse than reporting nothing is reporting "matches" for a comparison
        // nobody made.
        let attested = check(&certificate(), &Observed::default());
        assert!(attested.attests(), "nothing contradicted the certificate");
        assert!(
            !attested.fully_confirmed(),
            "and nothing confirmed it either"
        );
        assert_eq!(attested.unchecked().len(), 4);
        assert!(explain(&attested.findings[0]).contains("not the same as matching"));
    }

    #[test]
    fn an_unobservable_device_key_does_not_void_the_certificate() {
        // Distinct from a mismatched one. "I could not read the element" and "the element holds
        // another key" are different facts and only the second is about a different machine.
        let attested = check(
            &certificate(),
            &Observed {
                device_key: None,
                ..machine()
            },
        );
        assert!(attested.attests());
        assert_eq!(attested.unchecked(), vec!["the device key"]);
    }

    // ── An incomplete certificate ────────────────────────────────────────────

    #[test]
    fn a_certificate_missing_a_bound_field_says_so_rather_than_matching_an_empty_string() {
        // The bug this shape prevents: an empty `serial` compared against a machine reporting an
        // empty serial would "match", and a certificate binding nothing would confirm everything.
        let attested = check(
            &BirthCertificate {
                serial: String::new(),
                ..certificate()
            },
            &Observed {
                serial: Some(String::new()),
                ..machine()
            },
        );
        assert_eq!(
            attested.findings,
            vec![Finding::Incomplete("a serial number")]
        );
        assert!(explain(&attested.findings[0]).contains("§50.2 requires"));
    }

    #[test]
    fn the_unsigned_constructor_binds_nothing_and_attests_nothing() {
        let empty = BirthCertificate::unsigned(SignatureState::Absent);
        let attested = check(&empty, &machine());
        assert!(!attested.attests());
        // Every required field reported, plus the signature.
        assert_eq!(
            attested
                .findings
                .iter()
                .filter(|f| f.code() == "incomplete")
                .count(),
            5,
            "device key, serial, descriptor, hardware revision, model"
        );
    }

    // ── What `attests` is not ────────────────────────────────────────────────

    #[test]
    fn a_void_certificate_does_not_claim_the_robot_may_not_run() {
        // §50 establishes identity; it does not gate motion. A robot whose certificate is void is a
        // robot with an unattested identity, which is exactly the state every robot was in before
        // §50 existed — and nothing in this module says otherwise.
        let attested = check(
            &BirthCertificate {
                signature: SignatureState::Absent,
                ..certificate()
            },
            &machine(),
        );
        assert!(!attested.attests());
        for finding in &attested.findings {
            let sentence = explain(finding);
            assert!(
                !sentence.contains("must not run") && !sentence.contains("stop"),
                "§50 does not gate motion: {sentence}"
            );
        }
    }

    #[test]
    fn only_two_findings_void_a_certificate() {
        assert!(Finding::Unattested(SignatureState::Absent).voids_the_certificate());
        assert!(Finding::DeviceKeyMismatch {
            certificate: "a".to_string(),
            observed: "b".to_string(),
        }
        .voids_the_certificate());

        for finding in [
            Finding::SerialMismatch {
                certificate: "a".to_string(),
                observed: "b".to_string(),
            },
            Finding::DescriptorMismatch {
                certificate: "a".to_string(),
                observed: "b".to_string(),
            },
            Finding::HardwareRevisionMismatch {
                certificate: "a".to_string(),
                observed: "b".to_string(),
            },
            Finding::OwnershipAsserted(vec!["namespace".to_string()]),
            Finding::Unobservable("a serial number"),
            Finding::Incomplete("a serial number"),
        ] {
            assert!(
                !finding.voids_the_certificate(),
                "{} narrows what is attested; it does not remove it",
                finding.code()
            );
        }
    }

    #[test]
    fn the_codes_are_distinct() {
        let mut codes = vec![
            Finding::Unattested(SignatureState::Absent).code(),
            Finding::DeviceKeyMismatch {
                certificate: String::new(),
                observed: String::new(),
            }
            .code(),
            Finding::SerialMismatch {
                certificate: String::new(),
                observed: String::new(),
            }
            .code(),
            Finding::DescriptorMismatch {
                certificate: String::new(),
                observed: String::new(),
            }
            .code(),
            Finding::HardwareRevisionMismatch {
                certificate: String::new(),
                observed: String::new(),
            }
            .code(),
            Finding::OwnershipAsserted(Vec::new()).code(),
            Finding::Unobservable("x").code(),
            Finding::Incomplete("x").code(),
        ];
        codes.sort_unstable();
        let before = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), before, "two findings share a code");
    }

    // -- The wire, and the other edition ------------------------------------

    #[test]
    fn the_vocabulary_lists_every_code_in_the_order_the_checks_run() {
        // FINDINGS is what a consumer enumerates, and a code that exists and is not in it is a
        // finding nobody can write a handler for.
        assert_eq!(FINDINGS.len(), 8);
        assert_eq!(FINDINGS[0], "unattested", "the document outranks the machine");
        for code in FINDINGS {
            assert!(!code.is_empty());
        }
    }

    #[test]
    fn a_mismatch_puts_the_certificate_first_and_the_machine_second() {
        // The one property the TypeScript edition cannot get from the type system: it carries no
        // enum, so a finding there *is* a code and this list. A projection that put the observed
        // value first would leave every reader unable to tell which hex string came from the
        // document -- while both editions still reproduced every code.
        let finding = Finding::SerialMismatch {
            certificate: "SN-0001".to_string(),
            observed: "SN-0002".to_string(),
        };
        assert_eq!(finding.subjects(), vec!["SN-0001", "SN-0002"]);
        let sentence = explain(&finding);
        assert!(
            sentence.find("SN-0001").unwrap() < sentence.find("SN-0002").unwrap(),
            "the sentence reads in the same order as the subjects: {sentence}"
        );
    }

    #[test]
    fn every_finding_carries_what_it_is_about() {
        // An empty subject list would render a sentence with a hole in it, and the holes are the
        // values somebody has to act on.
        for finding in [
            Finding::Unattested(SignatureState::Untrusted),
            Finding::DeviceKeyMismatch {
                certificate: "a".to_string(),
                observed: "b".to_string(),
            },
            Finding::OwnershipAsserted(vec!["namespace".to_string()]),
            Finding::Unobservable("a serial number"),
            Finding::Incomplete("a model"),
        ] {
            assert!(
                !finding.subjects().is_empty(),
                "{} carries nothing",
                finding.code()
            );
        }
    }
}

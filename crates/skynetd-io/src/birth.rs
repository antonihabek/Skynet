//! The birth certificate, on disk (§50).
//!
//! [`skynet_contracts::birth`] decides what a certificate attests about a machine and takes no key,
//! no clock and no filesystem. This is the other half: reading the document the vendor shipped,
//! verifying their signature against the trust store, and assembling what the robot can observe
//! about itself.
//!
//! The split is the same one [`crate::embodiment`] and [`crate::entitlement`] use, and by now it is
//! this crate's shape rather than a choice made three times.
//!
//! # What this robot can observe, and what it cannot
//!
//! §50.2 binds six things. `skynetd` can check **two** of them today, and the honest answer to the
//! other four is [`Finding::Unobservable`](skynet_contracts::birth::Finding::Unobservable) rather
//! than silence:
//!
//! | Bound | Observable here |
//! |---|---|
//! | Device key | **No.** The private half never leaves the secure element, and §33.1 gives this process no path to one. |
//! | Serial | **No.** It is a DMI or vendor-bus read, which is the kernel's side of the RHAL. |
//! | Hardware revision | **No.** Same. |
//! | Descriptor digest | **Yes.** The descriptor is a file this daemon already owns and digests. |
//! | Model | Bound, not comparable — there is nothing on a machine to compare a marketing name to. |
//! | Manufacture date | Provenance. Never compared to anything. |
//!
//! So a robot running this today gets *"the descriptor this machine is running is the one the vendor
//! attested"* — which is §50.3's field, the one that closes *"the entire safety envelope rests on
//! believing it"* — plus four bindings reported as unchecked with the reason.
//!
//! **That is the useful half and it is also the limited half, and both are said out loud.** A
//! version of this that quietly omitted the four would produce a report reading *"certificate
//! verified"* on a robot that had compared one field, which is the shape of answer this project
//! refuses everywhere else.

use skynet_contracts::birth::{check, Attested, BirthCertificate, Observed};
use skynet_json::Json;

use crate::layout::Layout;
use crate::trust::{birth_message, read_detached, TrustStore};

/// The domain separator for a birth certificate's digest (§50.2).
pub const BIRTH_DIGEST_DOMAIN: &str = "skynet.birth.v1";

/// A certificate as it was read, with everything decided about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    /// The document, with its signature state filled in from the trust store.
    pub certificate: BirthCertificate,
    /// Its digest, as lower-case hex. What the vendor signed.
    pub digest: String,
    /// The signer's label, when the key was trusted and the signature verified.
    pub signer: Option<String>,
    /// Why the signature is in the state it is. Empty when it verified.
    pub signature_detail: String,
}

/// The digest of a certificate document.
pub fn digest_of(document: &Json) -> Result<[u8; 32], String> {
    let canonical = document.to_canonical().map_err(|error| {
        format!(
            "the birth certificate cannot be written canonically ({error:?}), so it has no digest \
             and no signature over it could be checked."
        )
    })?;

    let mut hasher = skynet_sha256::Sha256::new();
    hasher.update(BIRTH_DIGEST_DOMAIN.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical.as_bytes());
    Ok(hasher.finish())
}

fn hex(digest: &[u8; 32]) -> String {
    String::from_utf8_lossy(&skynet_sha256::hex_of(digest)).into_owned()
}

fn text(document: &Json, key: &str) -> String {
    document
        .get(key)
        .and_then(Json::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Read a certificate document, without touching a key.
///
/// The signature is left [`SignatureState::Absent`](skynet_contracts::admission::SignatureState),
/// which is the honest state for a document nobody has checked — and, per §50, the state under which
/// it attests nothing. [`read`] fills it in.
///
/// **Missing fields are read as empty rather than refused**, which is the opposite of what
/// [`crate::entitlement::from_json`] does with an absent expiry, and the difference is worth the
/// paragraph. An entitlement with no expiry is not a time-limited grant — it fails to be the thing
/// it claims. A certificate missing a field is still a certificate; it binds less than §50.2 asks,
/// and `check` reports each absence as `Incomplete` with the field named. Refusing it outright would
/// throw away the bindings it *does* make, and one of those is the descriptor digest.
pub fn from_json(document: &Json) -> BirthCertificate {
    BirthCertificate {
        device_key: text(document, "device_key"),
        model: text(document, "model"),
        serial: text(document, "serial"),
        descriptor_digest: text(document, "descriptor_digest"),
        hardware_revision: text(document, "hardware_revision"),
        manufactured: text(document, "manufactured"),
        calibration_digests: document.str_arr("calibration_digests"),
        // Collected rather than ignored, so `check` can report a vendor who asserted ownership.
        // §50.2 keeps two facts apart and this is the field that notices when somebody did not.
        claims_ownership: skynet_contracts::birth::OWNERSHIP_FIELDS
            .iter()
            .filter(|field| document.get_present(field).is_some())
            .map(|field| (*field).to_string())
            .collect(),
        signature: skynet_contracts::admission::SignatureState::Absent,
    }
}

/// Read the certificate this robot shipped with, and decide its signature.
///
/// `Ok(None)` is *"this robot has no birth certificate"*, which is not an error: §50 is new, every
/// robot manufactured before it has none, and a daemon that refused to start over it would be a
/// daemon nobody could run. What it costs is stated wherever the absence is displayed.
pub fn read(layout: &Layout, trust: &TrustStore) -> Result<Option<Held>, String> {
    let path = layout.birth_certificate();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };

    let document = Json::parse(&text)
        .map_err(|error| format!("{} does not parse: {error}", path.display()))?;
    let mut certificate = from_json(&document);

    let digest = hex(&digest_of(&document)?);
    let signed = read_detached(&layout.birth_signature())?;
    let verdict = trust.check(&birth_message(&digest), signed.as_ref());
    certificate.signature = verdict.state;

    Ok(Some(Held {
        certificate,
        digest,
        signer: verdict.signer,
        signature_detail: verdict.detail,
    }))
}

/// What this daemon can see about the machine it is running on (§50.2).
///
/// Two of the six, and four `None`s that are the point. §33.1 denies this process the RHAL, so the
/// device key, the serial and the hardware revision are the kernel's to read; the descriptor is a
/// file this daemon already owns.
///
/// Returning `None` for the three it cannot read is what makes `check` report them as *unchecked*
/// rather than as matching, which §7.2 requires and which is the difference between a report that
/// means something and one that reads well.
pub fn observe(layout: &Layout) -> Observed {
    Observed {
        device_key: None,
        serial: None,
        descriptor_digest: crate::observe::read_json(&layout.embodiment())
            .and_then(|document| crate::embodiment::digest_of(&document).ok())
            .map(|digest| hex(&digest)),
        hardware_revision: None,
    }
}

/// Read the certificate and check it against this machine, in one call.
///
/// `Ok(None)` when there is no certificate. The two steps are public separately because the Hub
/// will eventually check a fleet's certificates from a check-in, where the observation comes off the
/// wire rather than off this disk.
pub fn evaluate(layout: &Layout, trust: &TrustStore) -> Result<Option<(Held, Attested)>, String> {
    let Some(held) = read(layout, trust)? else {
        return Ok(None);
    };
    let attested = check(&held.certificate, &observe(layout));
    Ok(Some((held, attested)))
}

/// The certificate as an API answer, for `/identity` and the console.
pub fn to_json(held: &Held, attested: &Attested) -> Json {
    let mut out = Json::obj();
    out.set("digest", Json::Str(held.digest.clone()));
    out.set(
        "signature",
        Json::Str(held.certificate.signature.as_str().to_string()),
    );
    out.set(
        "signer",
        match &held.signer {
            Some(label) => Json::Str(label.clone()),
            None => Json::Null,
        },
    );
    // The two questions a reader has, answered separately. "Attests" is whether the document is
    // about this machine at all; "confirmed" is whether every binding it makes was checked and
    // matched. A single boolean would answer neither.
    out.set("attests", Json::Bool(attested.attests()));
    out.set("fully_confirmed", Json::Bool(attested.fully_confirmed()));
    out.set("model", Json::Str(held.certificate.model.clone()));
    out.set("serial", Json::Str(held.certificate.serial.clone()));
    out.set(
        "manufactured",
        Json::Str(held.certificate.manufactured.clone()),
    );
    out.set(
        "descriptor_digest",
        Json::Str(held.certificate.descriptor_digest.clone()),
    );

    let mut findings = Json::arr();
    for finding in &attested.findings {
        let mut row = Json::obj();
        row.set("code", Json::Str(finding.code().to_string()));
        row.set(
            "detail",
            Json::Str(skynet_contracts::birth::explain(finding)),
        );
        row.set("voids", Json::Bool(finding.voids_the_certificate()));
        findings.push(row);
    }
    out.set("findings", findings);

    let mut unchecked = Json::arr();
    for field in attested.unchecked() {
        unchecked.push(Json::Str(field.to_string()));
    }
    out.set("unchecked", unchecked);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TempRoot;
    use skynet_contracts::admission::SignatureState;
    use skynet_ed25519::signing::SigningKey;

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_seed(&[seed; 32])
    }

    fn store_with(label: &str, key: &SigningKey) -> TrustStore {
        let hex = String::from_utf8_lossy(&key.public_hex()).into_owned();
        TrustStore::from_text(&format!(
            "{{\"keys\":[{{\"label\":\"{label}\",\"key\":\"{hex}\",\
             \"added_at\":\"2026-08-30T12:00:00Z\",\"added_by\":\"alice\"}}]}}"
        ))
    }

    /// A descriptor this daemon can digest, and the digest it produces.
    fn install_descriptor(layout: &Layout) -> String {
        let document = Json::parse(crate::embodiment_vector::VECTOR_DESCRIPTOR).expect("parses");
        std::fs::create_dir_all(layout.state()).unwrap();
        std::fs::write(
            layout.embodiment(),
            document.to_canonical().unwrap().as_bytes(),
        )
        .unwrap();
        crate::embodiment_vector::VECTOR_DIGEST.to_string()
    }

    fn certificate_document(descriptor_digest: &str) -> Json {
        let mut document = Json::obj();
        document.set("device_key", Json::Str("a".repeat(64)));
        document.set("model", Json::Str("G1".to_string()));
        document.set("serial", Json::Str("SN-0001".to_string()));
        document.set(
            "descriptor_digest",
            Json::Str(descriptor_digest.to_string()),
        );
        document.set("hardware_revision", Json::Str("rev-C".to_string()));
        document.set("manufactured", Json::Str("2026-03-14".to_string()));
        document
    }

    fn install(layout: &Layout, document: &Json, signer: Option<&SigningKey>) {
        std::fs::create_dir_all(layout.identity()).unwrap();
        std::fs::write(
            layout.birth_certificate(),
            document.to_canonical().unwrap().as_bytes(),
        )
        .unwrap();

        let Some(signer) = signer else { return };
        let digest = hex(&digest_of(document).unwrap());
        let signature = signer.sign(&birth_message(&digest));
        let mut hexed = String::with_capacity(128);
        for byte in signature {
            hexed.push_str(&format!("{byte:02x}"));
        }
        let key_hex = String::from_utf8_lossy(&signer.public_hex()).into_owned();
        std::fs::write(
            layout.birth_signature(),
            format!("{{\"key\":\"{key_hex}\",\"value\":\"{hexed}\"}}").as_bytes(),
        )
        .unwrap();
    }

    // ── No certificate is not an error ───────────────────────────────────────

    #[test]
    fn a_robot_with_no_certificate_reads_as_none() {
        // §50 is new. Every robot manufactured before it has none, and a daemon that refused to
        // start over that would be a daemon nobody could run.
        let root = TempRoot::new("birth-absent");
        assert_eq!(read(&root.layout(), &TrustStore::missing()), Ok(None));
        assert_eq!(evaluate(&root.layout(), &TrustStore::missing()), Ok(None));
    }

    #[test]
    fn a_certificate_that_does_not_parse_is_an_error() {
        let root = TempRoot::new("birth-garbage");
        let layout = root.layout();
        std::fs::create_dir_all(layout.identity()).unwrap();
        std::fs::write(layout.birth_certificate(), b"{not json").unwrap();
        let error = read(&layout, &TrustStore::missing()).unwrap_err();
        assert!(error.contains("does not parse"), "{error}");
    }

    // ── §50.3's field, which is the one this daemon can actually check ───────

    #[test]
    fn the_descriptor_this_robot_runs_is_compared_to_the_one_the_vendor_attested() {
        let root = TempRoot::new("birth-descriptor");
        let layout = root.layout();
        let digest = install_descriptor(&layout);
        let issuer = key(9);
        install(&layout, &certificate_document(&digest), Some(&issuer));

        let (held, attested) = evaluate(&layout, &store_with("acme", &issuer))
            .unwrap()
            .expect("a certificate");
        assert_eq!(held.certificate.signature, SignatureState::Verified);
        assert!(attested.attests());
        assert!(
            !attested
                .findings
                .iter()
                .any(|f| f.code() == "descriptor_mismatch"),
            "the digests match"
        );
    }

    #[test]
    fn a_robot_running_a_descriptor_the_vendor_never_attested_is_told() {
        // The gap §5's `verified` flag cannot see: a descriptor can be perfectly well signed and be
        // for another machine.
        let root = TempRoot::new("birth-other-descriptor");
        let layout = root.layout();
        install_descriptor(&layout);
        let issuer = key(9);
        install(
            &layout,
            &certificate_document(&"f".repeat(64)),
            Some(&issuer),
        );

        let (_, attested) = evaluate(&layout, &store_with("acme", &issuer))
            .unwrap()
            .expect("a certificate");
        assert!(
            attested
                .findings
                .iter()
                .any(|f| f.code() == "descriptor_mismatch"),
            "{:?}",
            attested
                .findings
                .iter()
                .map(|f| f.code())
                .collect::<Vec<_>>()
        );
    }

    // ── What this daemon cannot see, said rather than assumed ────────────────

    #[test]
    fn the_three_bindings_this_process_may_not_read_are_reported_unchecked() {
        // §33.1 denies `skynetd` the RHAL, so the device key, the serial and the hardware revision
        // are the kernel's to read. Reported as unchecked rather than omitted, because a report
        // that silently dropped them would read as "certificate verified" on a robot that compared
        // one field.
        let root = TempRoot::new("birth-unchecked");
        let layout = root.layout();
        let digest = install_descriptor(&layout);
        let issuer = key(9);
        install(&layout, &certificate_document(&digest), Some(&issuer));

        let (_, attested) = evaluate(&layout, &store_with("acme", &issuer))
            .unwrap()
            .expect("a certificate");
        let unchecked = attested.unchecked();
        assert_eq!(unchecked.len(), 3, "{unchecked:?}");
        assert!(!attested.fully_confirmed());
        assert!(attested.attests(), "unchecked is not contradicted");
    }

    #[test]
    fn a_robot_with_no_descriptor_can_check_nothing_at_all() {
        let root = TempRoot::new("birth-no-descriptor");
        let layout = root.layout();
        let issuer = key(9);
        install(
            &layout,
            &certificate_document(&"f".repeat(64)),
            Some(&issuer),
        );

        let observed = observe(&layout);
        assert_eq!(observed.descriptor_digest, None);
        let (_, attested) = evaluate(&layout, &store_with("acme", &issuer))
            .unwrap()
            .expect("a certificate");
        assert_eq!(attested.unchecked().len(), 4);
        assert!(
            attested.attests(),
            "nothing contradicted it, and nothing confirmed it"
        );
    }

    // ── The signature, and the fifth domain tag ──────────────────────────────

    #[test]
    fn an_unsigned_certificate_attests_nothing() {
        let root = TempRoot::new("birth-unsigned");
        let layout = root.layout();
        let digest = install_descriptor(&layout);
        install(&layout, &certificate_document(&digest), None);

        let (held, attested) = evaluate(&layout, &TrustStore::missing())
            .unwrap()
            .expect("a certificate");
        assert_eq!(held.certificate.signature, SignatureState::Absent);
        assert!(!attested.attests());
    }

    #[test]
    fn editing_a_certificate_after_signing_breaks_it() {
        let root = TempRoot::new("birth-tampered");
        let layout = root.layout();
        let digest = install_descriptor(&layout);
        let issuer = key(9);
        install(
            &layout,
            &certificate_document(&"f".repeat(64)),
            Some(&issuer),
        );

        // Rewrite the descriptor digest to the one this robot runs, keeping the signature.
        std::fs::write(
            layout.birth_certificate(),
            certificate_document(&digest)
                .to_canonical()
                .unwrap()
                .as_bytes(),
        )
        .unwrap();

        let (held, attested) = evaluate(&layout, &store_with("acme", &issuer))
            .unwrap()
            .expect("a certificate");
        assert_eq!(held.certificate.signature, SignatureState::Failed);
        assert!(!attested.attests(), "a forged binding attests nothing");
    }

    #[test]
    fn a_signature_over_another_kind_of_digest_does_not_verify_over_a_certificate() {
        // Why there is a fifth domain tag. A certificate *binds* a descriptor digest, so a vendor
        // signs a document containing a hex string that is also a descriptor digest — and without
        // its own tag one signature could be presented as the other.
        let digest = "a".repeat(64);
        assert_ne!(
            birth_message(&digest),
            crate::trust::embodiment_message(&digest)
        );
        assert_ne!(
            birth_message(&digest),
            crate::trust::package_message(&digest)
        );
    }

    // ── §50.2: what it is, not whose it is ───────────────────────────────────

    #[test]
    fn a_vendor_who_named_an_owner_is_reported() {
        let root = TempRoot::new("birth-ownership");
        let layout = root.layout();
        let digest = install_descriptor(&layout);
        let mut document = certificate_document(&digest);
        document.set("namespace", Json::Str("acme-robotics".to_string()));
        let issuer = key(9);
        install(&layout, &document, Some(&issuer));

        let (held, attested) = evaluate(&layout, &store_with("acme", &issuer))
            .unwrap()
            .expect("a certificate");
        assert_eq!(
            held.certificate.claims_ownership,
            vec!["namespace".to_string()]
        );
        assert!(attested
            .findings
            .iter()
            .any(|f| f.code() == "ownership_asserted"));
        assert!(
            attested.attests(),
            "the machine is fine; the document overreached"
        );
    }

    // ── An incomplete certificate keeps the bindings it does make ────────────

    #[test]
    fn a_missing_field_is_incomplete_rather_than_a_refusal_to_read_the_document() {
        // The opposite of `entitlement::from_json`, which refuses a token with no expiry. An
        // entitlement with no expiry fails to be the thing it claims; a certificate missing a field
        // still binds the rest, and one of those is the descriptor digest.
        let root = TempRoot::new("birth-incomplete");
        let layout = root.layout();
        let digest = install_descriptor(&layout);
        let mut document = Json::obj();
        document.set("descriptor_digest", Json::Str(digest));
        let issuer = key(9);
        install(&layout, &document, Some(&issuer));

        let (_, attested) = evaluate(&layout, &store_with("acme", &issuer))
            .unwrap()
            .expect("a certificate");
        assert!(
            attested
                .findings
                .iter()
                .filter(|f| f.code() == "incomplete")
                .count()
                >= 4,
            "{:?}",
            attested
                .findings
                .iter()
                .map(|f| f.code())
                .collect::<Vec<_>>()
        );
        assert!(
            !attested
                .findings
                .iter()
                .any(|f| f.code() == "descriptor_mismatch"),
            "the one binding it makes still checked out"
        );
    }

    // ── The payload ──────────────────────────────────────────────────────────

    #[test]
    fn the_payload_answers_attests_and_confirmed_separately() {
        // One boolean would answer neither. "Attests" is whether the document is about this machine
        // at all; "confirmed" is whether every binding it makes was checked and matched.
        let root = TempRoot::new("birth-payload");
        let layout = root.layout();
        let digest = install_descriptor(&layout);
        let issuer = key(9);
        install(&layout, &certificate_document(&digest), Some(&issuer));

        let (held, attested) = evaluate(&layout, &store_with("acme", &issuer))
            .unwrap()
            .expect("a certificate");
        let payload = to_json(&held, &attested);
        assert_eq!(payload.get("attests").and_then(Json::as_bool), Some(true));
        assert_eq!(
            payload.get("fully_confirmed").and_then(Json::as_bool),
            Some(false),
            "three bindings were unchecked"
        );
        assert_eq!(payload.get("signer").and_then(Json::as_str), Some("acme"));
        assert!(payload.to_canonical().is_ok());
    }

    #[test]
    fn every_finding_in_the_payload_carries_its_sentence_and_whether_it_voids() {
        let root = TempRoot::new("birth-findings");
        let layout = root.layout();
        install_descriptor(&layout);
        install(&layout, &certificate_document(&"f".repeat(64)), None);

        let (held, attested) = evaluate(&layout, &TrustStore::missing())
            .unwrap()
            .expect("a certificate");
        let payload = to_json(&held, &attested);
        let findings = payload
            .get("findings")
            .and_then(Json::as_arr)
            .expect("findings");
        assert!(!findings.is_empty());
        for finding in findings {
            assert!(finding.get("code").and_then(Json::as_str).is_some());
            assert!(!finding
                .get("detail")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .is_empty());
            assert!(finding.get("voids").and_then(Json::as_bool).is_some());
        }
        assert_eq!(payload.get("attests").and_then(Json::as_bool), Some(false));
    }
}

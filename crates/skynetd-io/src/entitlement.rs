//! The cached entitlement token, on disk (§51).
//!
//! [`skynet_contracts::entitlement`] decides §51.2's table and takes no clock and no key. This is
//! the other half: reading the token a check-in cached, verifying the issuer's signature against
//! the trust store, and handing the contract a decided [`SignatureState`].
//!
//! The split is the same one [`crate::embodiment`] uses for descriptors, and for the same reason —
//! the decision has to run identically on the Hub and on a robot that has been offline for a week,
//! and a decision function that opened files could not.
//!
//! # Three refusals, and why each is a refusal rather than a shrug
//!
//! **A token whose `package` is not the one being asked about is refused.** `Layout::entitlement`
//! flattens `namespace/name` into a single path component, which is lossy: two packages can name
//! one file. Without this check that lossiness would be a way to make one package's grant answer
//! for another's, and a desired-state document is not a trusted input. The check is what makes the
//! flattening safe rather than clever.
//!
//! **A token that does not parse is refused, not treated as absent.** A file that exists and cannot
//! be read is a different fact from no file at all — the first means somebody delivered something
//! broken and the second means nobody delivered anything — and only the first is worth an operator's
//! attention.
//!
//! **A missing token is `Ok(None)`, and that is not a refusal at all.** Most packages are not paid
//! ones. Whether a given package *requires* an entitlement is §39's question about the marketplace
//! it came from, not this module's, and a reader that answered "refused" for every open-source
//! policy on a robot that has never bought anything would be answering a question nobody asked.

use skynet_contracts::admission::SignatureState;
use skynet_contracts::entitlement::{decide, Entitlement, Verdict, DEFAULT_GRACE_MS};
use skynet_json::Json;

use crate::layout::Layout;
use crate::trust::{entitlement_message, read_detached, TrustStore};

/// The domain separator for an entitlement token's digest (§51.2).
///
/// Distinct from the signature's domain tag in [`crate::trust`] for the same reason
/// [`crate::embodiment`]'s pair are distinct: one separates *digests* of different kinds of
/// document, the other separates *signatures* over different kinds of digest, and collapsing them
/// would make a scheme whose safety depended on nobody ever reusing a hash.
pub const ENTITLEMENT_DIGEST_DOMAIN: &str = "skynet.entitlement.v1";

/// A token as it was read, with everything decided about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached {
    /// The grant, with its signature state filled in from the trust store.
    pub entitlement: Entitlement,
    /// The token's digest, as lower-case hex. What the issuer signed.
    pub digest: String,
    /// The signer's label, when the key was trusted and the signature verified.
    pub signer: Option<String>,
    /// Why the signature is in the state it is. Empty when it verified.
    pub signature_detail: String,
}

impl Cached {
    /// §51.2's verdict for this token at `now_ms`.
    ///
    /// A convenience over [`decide`], and it exists so that no caller in this crate has to remember
    /// to pass `entitlement.signature` — the one argument whose omission would turn an unattested
    /// token into a licence.
    pub fn verdict(&self, now_ms: u64) -> Verdict {
        decide(&self.entitlement, now_ms)
    }
}

/// The digest of a token document.
///
/// Over the canonical form, under a domain separator, exactly as a descriptor's is.
pub fn digest_of(document: &Json) -> Result<[u8; 32], String> {
    let canonical = document.to_canonical().map_err(|error| {
        format!(
            "the entitlement cannot be written canonically ({error:?}), so it has no digest and \
             no signature over it could be checked."
        )
    })?;

    let mut hasher = skynet_sha256::Sha256::new();
    hasher.update(ENTITLEMENT_DIGEST_DOMAIN.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical.as_bytes());
    Ok(hasher.finish())
}

fn hex(digest: &[u8; 32]) -> String {
    String::from_utf8_lossy(&skynet_sha256::hex_of(digest)).into_owned()
}

/// Read a token document into a grant, without touching a key.
///
/// The signature is left [`SignatureState::Absent`], which is the honest state for a document
/// nobody has checked and the state that refuses an install. [`read`] fills it in.
pub fn from_json(document: &Json, package: &str) -> Result<Entitlement, String> {
    let named = document
        .get("package")
        .and_then(Json::as_str)
        .ok_or_else(|| "the entitlement does not say which package it is for".to_string())?;

    if named != package {
        return Err(format!(
            "this entitlement is for \"{named}\" and was read as \"{package}\"'s. A token names \
             the package it grants, and one that names another is not this package's licence \
             however it came to be in this file."
        ));
    }

    let expires_at_ms = document
        .get("expires_at_ms")
        .and_then(Json::as_f64)
        .ok_or_else(|| {
            "the entitlement has no expires_at_ms, so there is nothing time-limited about it. \
             §51.2's token is signed *and* time-limited, and a grant with no end is neither."
                .to_string()
        })?;
    if !expires_at_ms.is_finite() || expires_at_ms < 0.0 {
        return Err(format!(
            "expires_at_ms is {expires_at_ms}, which is not an instant."
        ));
    }

    // Absent means §51.2's ninety days, and the fallback is named rather than silent: a robot and a
    // Hub that both fall back have to agree about what they fell back to.
    let grace_ms = match document.get("grace_ms").and_then(Json::as_f64) {
        None => DEFAULT_GRACE_MS,
        Some(value) if value.is_finite() && value >= 0.0 => value as u64,
        Some(value) => {
            return Err(format!(
                "grace_ms is {value}, which is not a duration. Omit it for §51.2's ninety days."
            ))
        }
    };

    Ok(Entitlement {
        package: package.to_string(),
        expires_at_ms: expires_at_ms as u64,
        grace_ms,
        // Absent is `false`. A revocation is something the issuer says, and a token that does not
        // say it has not said it — the alternative, treating a missing field as revoked, would make
        // every older token a dispute.
        revoked: document
            .get("revoked")
            .and_then(Json::as_bool)
            .unwrap_or(false),
        signature: SignatureState::Absent,
    })
}

/// Read the cached token for one package, and decide its signature.
///
/// `Ok(None)` is *"no token was delivered for this package"*, which is not an error — see the module
/// header. `Err` is a token that exists and cannot be used.
pub fn read(layout: &Layout, trust: &TrustStore, package: &str) -> Result<Option<Cached>, String> {
    let path = layout.entitlement(package);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };

    let document = Json::parse(&text)
        .map_err(|error| format!("{} does not parse: {error}", path.display()))?;
    let mut entitlement = from_json(&document, package)?;

    let digest = hex(&digest_of(&document)?);
    let signed = read_detached(&layout.entitlement_signature(package))?;
    let verdict = trust.check(&entitlement_message(&digest), signed.as_ref());
    entitlement.signature = verdict.state;

    Ok(Some(Cached {
        entitlement,
        digest,
        signer: verdict.signer,
        signature_detail: verdict.detail,
    }))
}

/// The token as an API answer, for `/packages` and the console.
///
/// The verdict travels beside the token rather than being derived by every surface separately, and
/// `permits_install` travels beside the verdict for the same reason. What a console must never have
/// to do is compute §51.1's rule for itself.
pub fn to_json(cached: &Cached, now_ms: u64) -> Json {
    let verdict = cached.verdict(now_ms);
    let mut out = Json::obj();
    out.set("package", Json::Str(cached.entitlement.package.clone()));
    out.set("digest", Json::Str(cached.digest.clone()));
    out.set("verdict", Json::Str(verdict.as_str().to_string()));
    out.set(
        "expires_at_ms",
        Json::Num(cached.entitlement.expires_at_ms as f64),
    );
    out.set("grace_ms", Json::Num(cached.entitlement.grace_ms as f64));
    out.set("permits_install", Json::Bool(verdict.permits_install()));
    // Stated rather than implied, on every token, because §51.1's principle is invisible to
    // somebody reading a red badge unless it is written next to it.
    //
    // Through `permits_running` rather than as a literal `true`. The function returns a constant,
    // and writing the constant here instead would leave it called only from its own tests — which
    // is a single source of truth that nothing sources from, and one edit away from a payload that
    // says something §51.1 forbids.
    out.set(
        "permits_running",
        Json::Bool(skynet_contracts::entitlement::permits_running(verdict)),
    );
    out.set("raises_incident", Json::Bool(verdict.raises_incident()));
    out.set(
        "signature",
        Json::Str(cached.entitlement.signature.as_str().to_string()),
    );
    out.set(
        "signer",
        match &cached.signer {
            Some(label) => Json::Str(label.clone()),
            None => Json::Null,
        },
    );
    out.set(
        "explain",
        Json::Str(skynet_contracts::entitlement::explain(
            verdict,
            &cached.entitlement.package,
        )),
    );
    out.set(
        "remaining",
        match skynet_contracts::entitlement::remaining(&cached.entitlement, now_ms, verdict) {
            Some(text) => Json::Str(text),
            None => Json::Null,
        },
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TempRoot;
    use skynet_ed25519::signing::SigningKey;
    use skynet_json::Json;

    const DAY: u64 = 24 * 60 * 60 * 1000;
    const NOW: u64 = 1_000 * DAY;
    const PACKAGE: &str = "acme/grasp";

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

    /// A token document. `expires` is milliseconds since the epoch.
    fn token(package: &str, expires: u64) -> Json {
        let mut document = Json::obj();
        document.set("package", Json::Str(package.to_string()));
        document.set("expires_at_ms", Json::Num(expires as f64));
        document
    }

    /// Write a token, and optionally sign it with `signer`.
    fn install(layout: &Layout, package: &str, document: &Json, signer: Option<&SigningKey>) {
        std::fs::create_dir_all(layout.entitlements()).unwrap();
        let canonical = document.to_canonical().unwrap();
        std::fs::write(layout.entitlement(package), canonical.as_bytes()).unwrap();

        let Some(signer) = signer else { return };
        let digest = hex(&digest_of(document).unwrap());
        let signature = signer.sign(&entitlement_message(&digest));
        let mut hexed = String::with_capacity(128);
        for byte in signature {
            hexed.push_str(&format!("{byte:02x}"));
        }
        let key_hex = String::from_utf8_lossy(&signer.public_hex()).into_owned();
        std::fs::write(
            layout.entitlement_signature(package),
            format!("{{\"key\":\"{key_hex}\",\"value\":\"{hexed}\"}}").as_bytes(),
        )
        .unwrap();
    }

    // ── No token is not a refusal ────────────────────────────────────────────

    #[test]
    fn a_package_with_no_token_reads_as_none_rather_than_as_refused() {
        // Most packages are not paid ones. Answering "refused" here would refuse every
        // open-source policy on a robot that has never bought anything.
        let root = TempRoot::new("ent-absent");
        let layout = root.layout();
        assert_eq!(read(&layout, &TrustStore::missing(), PACKAGE), Ok(None));
    }

    #[test]
    fn a_token_that_does_not_parse_is_an_error_and_not_an_absence() {
        // A file that exists and cannot be read means somebody delivered something broken; no file
        // means nobody delivered anything. Only the first is worth an operator's attention.
        let root = TempRoot::new("ent-garbage");
        let layout = root.layout();
        std::fs::create_dir_all(layout.entitlements()).unwrap();
        std::fs::write(layout.entitlement(PACKAGE), b"{not json").unwrap();

        let error = read(&layout, &TrustStore::missing(), PACKAGE).unwrap_err();
        assert!(error.contains("does not parse"), "{error}");
    }

    // ── The check that makes the flattened path safe ─────────────────────────

    #[test]
    fn a_token_naming_another_package_is_refused() {
        // `Layout::entitlement` flattens `namespace/name` to one path component, and the flattening
        // is lossy. This check is what stops that lossiness becoming a way to make one package's
        // grant answer for another's.
        let root = TempRoot::new("ent-wrong-package");
        let layout = root.layout();
        install(&layout, PACKAGE, &token("acme/other", NOW + DAY), None);

        let error = read(&layout, &TrustStore::missing(), PACKAGE).unwrap_err();
        assert!(error.contains("acme/other"), "{error}");
        assert!(error.contains("is not this package's licence"), "{error}");
    }

    #[test]
    fn a_package_identifier_cannot_escape_the_entitlements_directory() {
        // The other half of the same property, at the path rather than at the document. A desired
        // state document is not a trusted input, and `../../etc/x` must not be a file name.
        let root = TempRoot::new("ent-traversal");
        let layout = root.layout();
        let path = layout.entitlement("../../etc/passwd");
        assert!(
            path.starts_with(layout.entitlements()),
            "{} escaped {}",
            path.display(),
            layout.entitlements().display()
        );
        // And a package named exactly `..` is a name, not a directory. The first draft of
        // `flatten` kept dots and this was the case that found it.
        let dots = layout.entitlement("..");
        assert!(dots.starts_with(layout.entitlements()));
        assert_eq!(dots.file_name().and_then(|n| n.to_str()), Some("__.json"));
        assert_eq!(
            layout.entitlement("../..").parent(),
            Some(layout.entitlements().as_path()),
            "a flattened identifier is always exactly one component deep"
        );
    }

    #[test]
    fn two_packages_that_flatten_alike_are_caught_by_the_document_and_not_by_the_path() {
        // `acme/grasp` and `acme_grasp` flatten to one file name. That collision is deliberate and
        // safe *because* the reader checks the token's own `package` field — which is asserted
        // here, so a future change that dropped the check fails with a test that says why.
        let root = TempRoot::new("ent-collision");
        let layout = root.layout();
        assert_eq!(
            layout.entitlement("acme/grasp"),
            layout.entitlement("acme_grasp")
        );

        install(&layout, "acme/grasp", &token("acme/grasp", NOW + DAY), None);
        assert!(read(&layout, &TrustStore::missing(), "acme_grasp").is_err());
    }

    // ── The signature, and what an unsigned token establishes ────────────────

    #[test]
    fn an_unsigned_token_is_unattested_and_refuses_an_install() {
        // It keeps the robot running, like everything else here. What it does not do is grant.
        let root = TempRoot::new("ent-unsigned");
        let layout = root.layout();
        install(&layout, PACKAGE, &token(PACKAGE, NOW + 365 * DAY), None);

        let cached = read(&layout, &TrustStore::missing(), PACKAGE)
            .unwrap()
            .expect("a token");
        assert_eq!(cached.entitlement.signature, SignatureState::Absent);
        assert_eq!(cached.verdict(NOW), Verdict::Unattested);
        assert!(!cached.verdict(NOW).permits_install());
    }

    #[test]
    fn a_token_signed_by_a_trusted_key_is_current() {
        let root = TempRoot::new("ent-signed");
        let layout = root.layout();
        let issuer = key(9);
        install(
            &layout,
            PACKAGE,
            &token(PACKAGE, NOW + 365 * DAY),
            Some(&issuer),
        );

        let cached = read(&layout, &store_with("acme", &issuer), PACKAGE)
            .unwrap()
            .expect("a token");
        assert_eq!(cached.entitlement.signature, SignatureState::Verified);
        assert_eq!(cached.signer.as_deref(), Some("acme"));
        assert_eq!(cached.verdict(NOW), Verdict::Current);
    }

    #[test]
    fn a_token_signed_by_a_stranger_is_untrusted_and_grants_nothing() {
        let root = TempRoot::new("ent-stranger");
        let layout = root.layout();
        let issuer = key(9);
        install(
            &layout,
            PACKAGE,
            &token(PACKAGE, NOW + 365 * DAY),
            Some(&issuer),
        );

        let cached = read(&layout, &store_with("somebody-else", &key(11)), PACKAGE)
            .unwrap()
            .expect("a token");
        assert_eq!(cached.entitlement.signature, SignatureState::Untrusted);
        assert_eq!(cached.verdict(NOW), Verdict::Unattested);
    }

    #[test]
    fn editing_a_token_after_it_was_signed_breaks_the_signature() {
        // The property the digest is for. Extending an expiry by hand must not work.
        let root = TempRoot::new("ent-tampered");
        let layout = root.layout();
        let issuer = key(9);
        install(&layout, PACKAGE, &token(PACKAGE, NOW - DAY), Some(&issuer));

        // Same package, later expiry, same signature file.
        let extended = token(PACKAGE, NOW + 365 * DAY);
        std::fs::write(
            layout.entitlement(PACKAGE),
            extended.to_canonical().unwrap().as_bytes(),
        )
        .unwrap();

        let cached = read(&layout, &store_with("acme", &issuer), PACKAGE)
            .unwrap()
            .expect("a token");
        assert_eq!(cached.entitlement.signature, SignatureState::Failed);
        assert_eq!(cached.verdict(NOW), Verdict::Unattested);
    }

    #[test]
    fn a_signature_over_a_package_digest_does_not_verify_over_a_token() {
        // Why there is a fourth domain tag. Without one, a release signature would grant a licence
        // to anything whose token digest happened to match a version digest.
        let document = token(PACKAGE, NOW + DAY);
        let digest = hex(&digest_of(&document).unwrap());
        assert_ne!(
            entitlement_message(&digest),
            crate::trust::package_message(&digest)
        );
    }

    // ── Reading the fields ───────────────────────────────────────────────────

    #[test]
    fn an_absent_grace_is_the_documented_ninety_days() {
        let entitlement = from_json(&token(PACKAGE, NOW), PACKAGE).unwrap();
        assert_eq!(entitlement.grace_ms, DEFAULT_GRACE_MS);
    }

    #[test]
    fn a_token_with_no_expiry_is_refused_because_it_is_not_time_limited() {
        let mut document = Json::obj();
        document.set("package", Json::Str(PACKAGE.to_string()));
        let error = from_json(&document, PACKAGE).unwrap_err();
        assert!(error.contains("expires_at_ms"), "{error}");
    }

    #[test]
    fn an_absent_revoked_field_is_not_a_revocation() {
        // A revocation is something the issuer says. Treating a missing field as revoked would make
        // every older token a dispute.
        assert!(!from_json(&token(PACKAGE, NOW), PACKAGE).unwrap().revoked);

        let mut revoked = token(PACKAGE, NOW);
        revoked.set("revoked", Json::Bool(true));
        assert!(from_json(&revoked, PACKAGE).unwrap().revoked);
    }

    #[test]
    fn a_token_read_from_disk_carries_its_own_grace() {
        let root = TempRoot::new("ent-grace");
        let layout = root.layout();
        let mut document = token(PACKAGE, NOW - 200 * DAY);
        document.set("grace_ms", Json::Num((365 * DAY) as f64));
        let issuer = key(9);
        install(&layout, PACKAGE, &document, Some(&issuer));

        let cached = read(&layout, &store_with("acme", &issuer), PACKAGE)
            .unwrap()
            .expect("a token");
        assert_eq!(cached.entitlement.grace_ms, 365 * DAY);
        assert_eq!(cached.verdict(NOW), Verdict::Lapsed);
    }

    // ── The payload a console reads ──────────────────────────────────────────

    #[test]
    fn the_payload_says_the_robot_keeps_running_on_every_verdict() {
        // §51.1's principle, on the wire rather than only in a doc comment. A console must never
        // have to compute it.
        let root = TempRoot::new("ent-payload");
        let layout = root.layout();
        let issuer = key(9);

        for (expires, expected) in [
            (NOW + DAY, "current"),
            (NOW - DAY, "lapsed"),
            (NOW - 200 * DAY, "expired"),
        ] {
            install(&layout, PACKAGE, &token(PACKAGE, expires), Some(&issuer));
            let cached = read(&layout, &store_with("acme", &issuer), PACKAGE)
                .unwrap()
                .expect("a token");
            let payload = to_json(&cached, NOW);
            assert_eq!(
                payload.get("verdict").and_then(Json::as_str),
                Some(expected)
            );
            assert_eq!(
                payload.get("permits_running").and_then(Json::as_bool),
                Some(true),
                "{expected}"
            );
            assert!(payload.get("explain").and_then(Json::as_str).is_some());
        }
    }

    #[test]
    fn the_payload_reports_an_incident_only_for_a_revocation() {
        let root = TempRoot::new("ent-incident");
        let layout = root.layout();
        let issuer = key(9);

        let mut document = token(PACKAGE, NOW + 365 * DAY);
        document.set("revoked", Json::Bool(true));
        install(&layout, PACKAGE, &document, Some(&issuer));

        let cached = read(&layout, &store_with("acme", &issuer), PACKAGE)
            .unwrap()
            .expect("a token");
        let payload = to_json(&cached, NOW);
        assert_eq!(
            payload.get("verdict").and_then(Json::as_str),
            Some("revoked")
        );
        assert_eq!(
            payload.get("raises_incident").and_then(Json::as_bool),
            Some(true)
        );
        assert_eq!(
            payload.get("permits_install").and_then(Json::as_bool),
            Some(false)
        );
        assert_eq!(
            payload.get("permits_running").and_then(Json::as_bool),
            Some(true)
        );
        // No countdown beside a state that is not about time.
        assert_eq!(payload.get("remaining"), Some(&Json::Null));
    }

    #[test]
    fn the_payload_is_canonical_so_two_editions_could_be_compared() {
        let root = TempRoot::new("ent-canonical");
        let layout = root.layout();
        let issuer = key(9);
        install(&layout, PACKAGE, &token(PACKAGE, NOW + DAY), Some(&issuer));
        let cached = read(&layout, &store_with("acme", &issuer), PACKAGE)
            .unwrap()
            .expect("a token");
        assert!(to_json(&cached, NOW).to_canonical().is_ok());
    }
}

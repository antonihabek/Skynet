//! The trust store: the keys this robot has been told to believe (§19, §37.1).
//!
//! §37.1's `verify` step is *"digest, then Ed25519 signature against a key the operator trusts"*,
//! and until this module existed the last five words had no referent. [`skynet_ed25519`] answers
//! *did this signature verify*; only an operator can answer *and should this robot care*.
//!
//! # A signature names its signer
//!
//! An Ed25519 signature does not say which key made it, which means a verifier holding a set of
//! trusted keys and a signature that verifies under none of them cannot tell **two very different
//! things apart**: bytes that were tampered with, and bytes signed correctly by a stranger. The
//! first is an attack and the second is an administrative gap, and sending an operator to look for
//! one when it is the other wastes the only attention a robot ever gets.
//!
//! So a signature arrives with the public key that made it:
//!
//! ```json
//! "signature": { "key": "<64 hex>", "value": "<128 hex>" }
//! ```
//!
//! and the two questions are asked in order, because they have different answers:
//!
//! | | |
//! |---|---|
//! | Does it verify under the key it names? | No → [`SignatureState::Failed`]. Something went wrong |
//! | Is that key one this robot trusts? | No → [`SignatureState::Untrusted`]. Nothing went wrong |
//! | Both | [`SignatureState::Verified`] |
//! | No signature at all | [`SignatureState::Absent`] |
//!
//! A stranger can of course produce a key and a valid signature over anything. That is exactly what
//! `Untrusted` reports, and it is why the state is an error rather than a warning: *somebody*
//! attested to these bytes and it was nobody here.
//!
//! # The file
//!
//! `/data/state/trust.json`, beside the credentials and the profiles:
//!
//! ```json
//! {
//!   "keys": [
//!     { "label": "acme-robotics", "key": "d75a98…", "note": "release signing, added at pairing" }
//!   ]
//! }
//! ```
//!
//! The **label** is why this is not a bare list of hex strings. A report that says *"signed by
//! `d75a980182b10ab7…`"* has told an operator nothing they can act on; one that says *"signed by
//! acme-robotics"* has. The label is local — this robot's name for a key — and nothing outside this
//! file depends on it, which is the property that lets an operator rename one.
//!
//! # A missing file is a robot that trusts nobody
//!
//! Not an error, and not a robot that trusts everybody. §39.2 draws exactly this line for
//! credentials — *"the failure mode of never provisioning it is a useless console rather than an
//! open one"* — and it is the same line here: every package on an unprovisioned robot reports
//! `signature_untrusted`, which is true, actionable, and names the file to create.
//!
//! # A malformed entry is skipped rather than fatal
//!
//! One mistyped key does not empty the store. The argument is short and it is about direction: a
//! key that fails to parse can only ever **remove** trust, never add it, so skipping it is safe in
//! the only direction that matters. Refusing the whole file would remove trust from every *other*
//! key as well — turning one typo into a robot that installs nothing — and it would buy no safety,
//! because the attacker who can write to this file can also write a well-formed entry.
//!
//! Every skipped entry is reported. A store that silently held four of five keys would be a store
//! whose fifth signer is mysteriously always untrusted.

use skynet_ed25519::{KeyError, Signature, SignatureError, VerifyError, VerifyingKey};
use skynet_json::Json;

use skynet_contracts::admission::SignatureState;

/// The domain tag for a package's release signature.
///
/// Prepended to the digest before signing, and it is not decoration. §38.1 signs operator profiles
/// *"with the same Ed25519 machinery as releases"*, so without a tag a signature made over a
/// profile's digest would verify over a package with the same digest — and the two are separate
/// decisions by separate people. The tag makes a signature say what it is a signature *of*.
pub const PACKAGE_DOMAIN: &str = "skynet.package.v1\n";

/// The domain tag for an operator profile's signature (§38.1).
pub const PROFILE_DOMAIN: &str = "skynet.profile.v1\n";

/// The domain tag for an embodiment descriptor's signature (§5).
pub const EMBODIMENT_DOMAIN: &str = "skynet.embodiment.v1\n";

/// The message a package signature is made over: the domain tag, then the version digest.
pub fn package_message(digest: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(PACKAGE_DOMAIN.len() + digest.len());
    out.extend_from_slice(PACKAGE_DOMAIN.as_bytes());
    out.extend_from_slice(digest.as_bytes());
    out
}

/// The message an operator profile's signature is made over (§38.1).
pub fn profile_message(digest: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(PROFILE_DOMAIN.len() + digest.len());
    out.extend_from_slice(PROFILE_DOMAIN.as_bytes());
    out.extend_from_slice(digest.as_bytes());
    out
}

/// The message a vendor's signature over an embodiment descriptor is made over (§5).
///
/// The digest is the one [`crate::embodiment`] computes over the canonical form of the document, so
/// two robots handed the same descriptor with different whitespace check the same signature.
pub fn embodiment_message(digest: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(EMBODIMENT_DOMAIN.len() + digest.len());
    out.extend_from_slice(EMBODIMENT_DOMAIN.as_bytes());
    out.extend_from_slice(digest.as_bytes());
    out
}

/// Read a detached signature from a `{ "key": …, "value": … }` file.
///
/// `Ok(None)` is "there is no such file", which is the [`SignatureState::Absent`] case and not an
/// error. `Err` is a file that exists and cannot be used, which is a different fact and gets a
/// different sentence.
pub fn read_detached(path: &std::path::Path) -> Result<Option<SignedBy>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{} could not be read: {error}", path.display())),
    };
    let document =
        Json::parse(&text).map_err(|e| format!("{} does not parse: {e}", path.display()))?;
    SignedBy::from_json(&document)
        .map(Some)
        .map_err(|e| format!("{}: {}", path.display(), e.reason()))
}

/// A signature as it travels: the key that made it, and the signature itself.
///
/// Held as the parsed forms, so a `SignedBy` that exists is one whose two halves are both
/// well-formed. Whether it *verifies*, and whether the key is trusted, are separate questions
/// answered by [`TrustStore::check`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignedBy {
    /// The public key the signer claims.
    pub key: VerifyingKey,
    /// The signature.
    pub signature: Signature,
}

/// Why a `{ key, value }` pair is not a usable signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignedByError {
    /// The `key` field is absent or not a string.
    KeyMissing,
    /// The `value` field is absent or not a string.
    ValueMissing,
    /// The key does not parse, or must not be trusted.
    Key(KeyError),
    /// The signature does not parse.
    Signature(SignatureError),
}

impl SignedByError {
    /// A sentence for an operator.
    pub fn reason(&self) -> String {
        match self {
            SignedByError::KeyMissing => {
                "the signature names no key, so there is nothing to verify it against — a \
                 signature must arrive as `{ \"key\": …, \"value\": … }`"
                    .to_string()
            }
            SignedByError::ValueMissing => {
                "the signature names a key and carries no signature value".to_string()
            }
            SignedByError::Key(e) => format!("the signing key is unusable: {}", e.reason()),
            SignedByError::Signature(e) => format!("the signature is unusable: {}", e.reason()),
        }
    }
}

impl SignedBy {
    /// Parse the two hex strings.
    pub fn from_hex(key: &str, signature: &str) -> Result<SignedBy, SignedByError> {
        Ok(SignedBy {
            key: VerifyingKey::from_hex(key).map_err(SignedByError::Key)?,
            signature: Signature::from_hex(signature).map_err(SignedByError::Signature)?,
        })
    }

    /// Read `{ "key": "<64 hex>", "value": "<128 hex>" }`.
    pub fn from_json(value: &Json) -> Result<SignedBy, SignedByError> {
        let key = value
            .get("key")
            .and_then(Json::as_str)
            .ok_or(SignedByError::KeyMissing)?;
        let signature = value
            .get("value")
            .and_then(Json::as_str)
            .ok_or(SignedByError::ValueMissing)?;
        SignedBy::from_hex(key, signature)
    }
}

/// A key this robot has been told to trust, and the operator's name for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKey {
    /// This robot's name for the signer. Local, and only ever used in reports.
    pub label: String,
    /// The key.
    pub key: VerifyingKey,
}

/// An entry that could not be loaded, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedKey {
    /// Where it was, so an operator can find it in the file.
    pub at: String,
    /// What is wrong.
    pub detail: String,
}

/// Where the store's contents came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The file is not there. This robot has not been given any keys.
    Missing,
    /// It was read.
    Loaded,
    /// It is there and it does not parse.
    ///
    /// Distinct from [`Source::Missing`] because the remedies are different: one is a robot nobody
    /// has provisioned, and the other is a file somebody broke — possibly the person who was
    /// halfway through adding a key, and possibly not.
    Unreadable {
        /// What went wrong.
        detail: String,
    },
}

/// The keys this robot trusts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustStore {
    keys: Vec<TrustedKey>,
    skipped: Vec<SkippedKey>,
    source: Source,
}

/// What checking a signature concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// The state to give admission.
    pub state: SignatureState,
    /// The signer's label, when the key was trusted and the signature verified.
    pub signer: Option<String>,
    /// A sentence naming what happened, for a report. Empty when the signature verified.
    pub detail: String,
}

impl TrustStore {
    /// A store holding nothing, because there is no file.
    pub fn missing() -> TrustStore {
        TrustStore {
            keys: Vec::new(),
            skipped: Vec::new(),
            source: Source::Missing,
        }
    }

    /// Read the store from the layout, if it is there.
    pub fn load(layout: &crate::layout::Layout) -> TrustStore {
        let path = layout.trust();
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return TrustStore::missing()
            }
            Err(error) => {
                return TrustStore {
                    keys: Vec::new(),
                    skipped: Vec::new(),
                    source: Source::Unreadable {
                        detail: format!("could not be read: {error}"),
                    },
                }
            }
        };
        TrustStore::from_text(&text)
    }

    /// Parse the store's document.
    pub fn from_text(text: &str) -> TrustStore {
        let document = match Json::parse(text) {
            Ok(document) => document,
            Err(error) => {
                return TrustStore {
                    keys: Vec::new(),
                    skipped: Vec::new(),
                    source: Source::Unreadable {
                        detail: format!("does not parse: {error}"),
                    },
                }
            }
        };

        let mut keys = Vec::new();
        let mut skipped = Vec::new();
        for (index, entry) in document.arr_or_empty("keys").iter().enumerate() {
            let at = format!("keys[{index}]");
            let label = entry
                .get("label")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            let Some(hex) = entry.get("key").and_then(Json::as_str) else {
                skipped.push(SkippedKey {
                    at,
                    detail: "has no `key` field".to_string(),
                });
                continue;
            };
            match VerifyingKey::from_hex(hex) {
                Ok(key) => {
                    // A label is not optional in practice: a report that names a key by its hex has
                    // told an operator nothing. An entry without one falls back to the hex rather
                    // than being refused, because refusing would be losing a key over a caption.
                    let label = if label.is_empty() {
                        let rendered = key.to_hex();
                        let text = core::str::from_utf8(&rendered)
                            .expect("hex is ASCII")
                            .to_string();
                        text[..16].to_string()
                    } else {
                        label
                    };
                    if keys.iter().any(|k: &TrustedKey| k.key == key) {
                        skipped.push(SkippedKey {
                            at,
                            detail: "is a key that already appears earlier in the file".to_string(),
                        });
                        continue;
                    }
                    keys.push(TrustedKey { label, key });
                }
                Err(error) => skipped.push(SkippedKey {
                    at,
                    detail: error.reason().to_string(),
                }),
            }
        }

        TrustStore {
            keys,
            skipped,
            source: Source::Loaded,
        }
    }

    /// The keys, in file order.
    pub fn keys(&self) -> &[TrustedKey] {
        &self.keys
    }

    /// Entries that could not be loaded.
    pub fn skipped(&self) -> &[SkippedKey] {
        &self.skipped
    }

    /// Where the contents came from.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Whether this robot trusts anybody at all.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// The label this robot has for a key, if it trusts it.
    pub fn label_for(&self, key: &VerifyingKey) -> Option<&str> {
        self.keys
            .iter()
            .find(|k| &k.key == key)
            .map(|k| k.label.as_str())
    }

    /// Decide what is known about a signature over `message`.
    ///
    /// `signature` is `None` when nothing accompanied the artifact, which is
    /// [`SignatureState::Absent`] — a fact about the publisher, and never confused with the other
    /// four.
    pub fn check(&self, message: &[u8], signature: Option<&SignedBy>) -> Verdict {
        let Some(signed) = signature else {
            return Verdict {
                state: SignatureState::Absent,
                signer: None,
                detail: "nothing accompanied these bytes to say who produced them".to_string(),
            };
        };

        // The signature is checked against the key it names *before* the key is checked against the
        // store, and the order is the whole point. A signature that does not verify under its own
        // claimed key is broken bytes; one that verifies under a key nobody trusts is a stranger.
        // Asking "is this key trusted" first would collapse the two back together.
        if let Err(error) = signed.key.verify(message, &signed.signature) {
            return Verdict {
                state: SignatureState::Failed,
                signer: None,
                detail: match error {
                    VerifyError::Mismatch => {
                        "the signature does not verify against the key it names, so the bytes are \
                         not the ones that were signed"
                            .to_string()
                    }
                    other => other.reason().to_string(),
                },
            };
        }

        match self.label_for(&signed.key) {
            Some(label) => Verdict {
                state: SignatureState::Verified,
                signer: Some(label.to_string()),
                detail: String::new(),
            },
            None => {
                let rendered = signed.key.to_hex();
                let hex = core::str::from_utf8(&rendered).expect("hex is ASCII");
                Verdict {
                    state: SignatureState::Untrusted,
                    signer: None,
                    detail: match &self.source {
                        Source::Missing => format!(
                            "signed by `{hex}`, and this robot has no trust store — nobody has \
                             told it whose signatures to believe"
                        ),
                        Source::Unreadable { detail } => format!(
                            "signed by `{hex}`, and the trust store {detail}, so this robot \
                             currently trusts nobody"
                        ),
                        Source::Loaded => format!(
                            "signed by `{hex}`, which is not among the {} key(s) this robot trusts",
                            self.keys.len()
                        ),
                    },
                }
            }
        }
    }

    /// Lines describing the store itself, for a report. Empty when it loaded and held keys.
    ///
    ///
    /// Kept separate from [`TrustStore::check`] because these are facts about the **robot**, and a
    /// per-package report is the wrong place to discover that a robot has no trust store — the
    /// package is fine and the machine is not configured.
    pub fn notices(&self) -> Vec<String> {
        let mut lines = Vec::new();
        match &self.source {
            Source::Missing => lines.push(
                "trust: no trust store — every signature will report `untrusted`. Write \
                 `state/trust.json` with the keys this robot should believe"
                    .to_string(),
            ),
            Source::Unreadable { detail } => lines.push(format!(
                "trust: the trust store {detail}. This robot trusts nobody until it is repaired, \
                 and a file that broke on its own is worth a second look"
            )),
            Source::Loaded => {
                if self.keys.is_empty() {
                    lines.push(
                        "trust: the trust store is present and names no keys, so every signature \
                         will report `untrusted`"
                            .to_string(),
                    );
                }
            }
        }
        for skipped in &self.skipped {
            lines.push(format!(
                "trust: {} was skipped — it {}",
                skipped.at, skipped.detail
            ));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skynet_ed25519::signing::SigningKey;

    const DIGEST: &str = "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn key(seed: u8) -> SigningKey {
        SigningKey::from_seed(&[seed; 32])
    }

    fn public_hex(key: &SigningKey) -> String {
        String::from_utf8(key.public_hex().to_vec()).expect("hex is ASCII")
    }

    fn sign(key: &SigningKey, message: &[u8]) -> SignedBy {
        SignedBy {
            key: key.verifying_key(),
            signature: skynet_ed25519::Signature::from_bytes(&key.sign(message))
                .expect("a produced signature is canonical"),
        }
    }

    fn store_with(entries: &[(&str, &SigningKey)]) -> TrustStore {
        let mut text = String::from("{\"keys\":[");
        for (index, (label, key)) in entries.iter().enumerate() {
            if index > 0 {
                text.push(',');
            }
            text.push_str(&format!(
                "{{\"label\":\"{label}\",\"key\":\"{}\"}}",
                public_hex(key)
            ));
        }
        text.push_str("]}");
        TrustStore::from_text(&text)
    }

    #[test]
    fn a_trusted_key_verifies_and_the_report_names_it() {
        let signer = key(1);
        let store = store_with(&[("acme-robotics", &signer)]);
        let message = package_message(DIGEST);
        let verdict = store.check(&message, Some(&sign(&signer, &message)));

        assert_eq!(verdict.state, SignatureState::Verified);
        assert_eq!(verdict.signer.as_deref(), Some("acme-robotics"));
        assert!(store.notices().is_empty(), "{:?}", store.notices());
    }

    /// The distinction the fifth state exists for.
    ///
    /// A stranger's signature is *valid*. Nothing went wrong with it, nothing was tampered with,
    /// and reporting `failed` would send somebody looking for an attack that did not happen.
    #[test]
    fn a_stranger_is_untrusted_and_not_failed() {
        let store = store_with(&[("acme-robotics", &key(1))]);
        let stranger = key(2);
        let message = package_message(DIGEST);
        let verdict = store.check(&message, Some(&sign(&stranger, &message)));

        assert_eq!(verdict.state, SignatureState::Untrusted);
        assert_eq!(verdict.signer, None);
        assert!(
            verdict.detail.contains("not among the 1 key(s)"),
            "{verdict:?}"
        );
        assert!(
            verdict.detail.contains(&public_hex(&stranger)),
            "{verdict:?}"
        );
    }

    /// And the other half of the same distinction: a signature over other bytes *is* `failed`.
    #[test]
    fn a_signature_over_other_bytes_is_failed_even_from_a_trusted_key() {
        let signer = key(1);
        let store = store_with(&[("acme-robotics", &signer)]);
        let elsewhere = package_message("sha256:something-else");
        let verdict = store.check(&package_message(DIGEST), Some(&sign(&signer, &elsewhere)));

        assert_eq!(verdict.state, SignatureState::Failed);
        assert_eq!(verdict.signer, None);
        assert!(
            verdict.detail.contains("not the ones that were signed"),
            "{verdict:?}"
        );
    }

    #[test]
    fn nothing_at_all_is_absent() {
        let verdict = store_with(&[("acme", &key(1))]).check(&package_message(DIGEST), None);
        assert_eq!(verdict.state, SignatureState::Absent);
    }

    /// §38.1 signs profiles with the same machinery, so the tag is what keeps the two apart.
    ///
    /// Without it, an operator who could get a profile signed could install a package with the same
    /// digest, and the two are separate decisions by separate people.
    #[test]
    fn a_profile_signature_does_not_verify_as_a_package_signature() {
        let signer = key(1);
        let store = store_with(&[("acme-robotics", &signer)]);
        let as_profile = sign(&signer, &profile_message(DIGEST));

        assert_ne!(package_message(DIGEST), profile_message(DIGEST));
        assert_eq!(
            store
                .check(&package_message(DIGEST), Some(&as_profile))
                .state,
            SignatureState::Failed
        );
        assert_eq!(
            store
                .check(&profile_message(DIGEST), Some(&as_profile))
                .state,
            SignatureState::Verified
        );
    }

    /// §39.2's line, applied to keys: no trust store is a robot that trusts nobody.
    #[test]
    fn a_missing_store_trusts_nobody_and_says_which_file_to_write() {
        let store = TrustStore::missing();
        let signer = key(1);
        let message = package_message(DIGEST);
        let verdict = store.check(&message, Some(&sign(&signer, &message)));

        assert_eq!(verdict.state, SignatureState::Untrusted);
        assert!(verdict.detail.contains("no trust store"), "{verdict:?}");
        let notices = store.notices().join("\n");
        assert!(notices.contains("state/trust.json"), "{notices}");
    }

    /// A broken file is not a missing one, and the two get different sentences.
    #[test]
    fn an_unreadable_store_says_so_rather_than_reading_as_empty() {
        let store = TrustStore::from_text("{ not json");
        assert!(matches!(store.source(), Source::Unreadable { .. }));
        assert!(store.is_empty());

        let notices = store.notices().join("\n");
        assert!(notices.contains("does not parse"), "{notices}");
        assert!(notices.contains("worth a second look"), "{notices}");
    }

    /// One mistyped key does not empty the store, and the skip is reported.
    #[test]
    fn a_malformed_entry_is_skipped_and_the_rest_still_work() {
        let good = key(1);
        let text = format!(
            "{{\"keys\":[{{\"label\":\"typo\",\"key\":\"nothex\"}},\
             {{\"label\":\"acme\",\"key\":\"{}\"}}]}}",
            public_hex(&good)
        );
        let store = TrustStore::from_text(&text);

        assert_eq!(store.keys().len(), 1);
        assert_eq!(store.skipped().len(), 1);
        assert_eq!(store.skipped()[0].at, "keys[0]");

        let message = package_message(DIGEST);
        assert_eq!(
            store.check(&message, Some(&sign(&good, &message))).state,
            SignatureState::Verified
        );
        let notices = store.notices().join("\n");
        assert!(notices.contains("keys[0] was skipped"), "{notices}");
    }

    /// A small-order key verifies signatures nobody produced, so an operator must not be able to
    /// trust one by pasting it into a file.
    #[test]
    fn a_small_order_key_cannot_be_trusted() {
        let store = TrustStore::from_text(
            "{\"keys\":[{\"label\":\"anyone\",\"key\":\
             \"0100000000000000000000000000000000000000000000000000000000000000\"}]}",
        );
        assert!(store.is_empty());
        assert_eq!(store.skipped().len(), 1);
        assert!(
            store.skipped()[0].detail.contains("small-order"),
            "{:?}",
            store.skipped()[0]
        );
    }

    /// The same key twice is one key, and the duplicate is reported rather than silently doubling
    /// the count in `not among the N key(s)`.
    #[test]
    fn the_same_key_twice_is_one_key() {
        let signer = key(1);
        let store = store_with(&[("first", &signer), ("second", &signer)]);
        assert_eq!(store.keys().len(), 1);
        assert_eq!(store.keys()[0].label, "first");
        assert_eq!(store.skipped().len(), 1);
    }

    /// A key with no label is still a key. Losing one over a missing caption would be the wrong
    /// trade, and the fallback is short enough to read out.
    #[test]
    fn an_unlabelled_key_falls_back_to_its_hex() {
        let signer = key(3);
        let store = TrustStore::from_text(&format!(
            "{{\"keys\":[{{\"key\":\"{}\"}}]}}",
            public_hex(&signer)
        ));
        assert_eq!(store.keys().len(), 1);
        assert_eq!(store.keys()[0].label, public_hex(&signer)[..16]);
    }

    /// A store that loaded and holds nothing is not the same as no store, and neither reads as a
    /// robot that trusts anybody.
    #[test]
    fn an_empty_list_is_reported_as_trusting_nobody() {
        let store = TrustStore::from_text("{\"keys\":[]}");
        assert!(matches!(store.source(), Source::Loaded));
        let notices = store.notices().join("\n");
        assert!(notices.contains("names no keys"), "{notices}");
    }

    #[test]
    fn a_signature_field_missing_half_of_itself_is_refused() {
        let mut only_key = Json::obj();
        only_key.set("key", Json::from("aa"));
        assert_eq!(
            SignedBy::from_json(&only_key),
            Err(SignedByError::ValueMissing)
        );

        let mut only_value = Json::obj();
        only_value.set("value", Json::from("bb"));
        assert_eq!(
            SignedBy::from_json(&only_value),
            Err(SignedByError::KeyMissing)
        );

        for error in [
            SignedByError::KeyMissing,
            SignedByError::ValueMissing,
            SignedByError::Key(KeyError::NotHex),
            SignedByError::Signature(SignatureError::NotHex),
        ] {
            assert!(error.reason().len() > 20, "{error:?}");
        }
    }
}

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

/// The domain tag for an offline entitlement token (§51.2).
///
/// The fourth tag, and the reason for a fourth is the reason there was a first: §51.2's token is
/// *"signed and time-limited"*, and it is signed over the same shape — a digest — as a release, a
/// profile and a descriptor. Without its own tag, a signature made over an entitlement's digest
/// would verify over a *package* whose digest happened to match, and the difference between those
/// two is the difference between "you may install this" and "these are the bytes".
pub const ENTITLEMENT_DOMAIN: &str = "skynet.entitlement.v1
";

/// The domain tag for a vendor's birth certificate (§50.2).
///
/// The fifth, and the reason is the reason there was a first. A birth certificate binds the
/// **embodiment descriptor digest**, so a vendor signs a document containing a hex string that is
/// also a descriptor digest — and without its own tag a signature over one could be presented as a
/// signature over the other. The tag makes a signature say what it is a signature *of*.
pub const BIRTH_DOMAIN: &str = "skynet.birth.v1
";

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

/// The message an issuer's signature over an entitlement token is made over (§51.2).
///
/// The digest is over the token's canonical form, computed the way every other digest in this
/// system is, so an issuer and a robot that disagree about whitespace still agree about the grant.
pub fn entitlement_message(digest: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(ENTITLEMENT_DOMAIN.len() + digest.len());
    out.extend_from_slice(ENTITLEMENT_DOMAIN.as_bytes());
    out.extend_from_slice(digest.as_bytes());
    out
}

/// The message a vendor's signature over a birth certificate is made over (§50.2).
pub fn birth_message(digest: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(BIRTH_DOMAIN.len() + digest.len());
    out.extend_from_slice(BIRTH_DOMAIN.as_bytes());
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

/// When a key was trusted, by whom, and why — or the recorded fact that nobody wrote it down.
///
/// §38.3 asks this of profiles: *"every change is a black box event and an audit-log entry,
/// carrying who, when, which layer, and the previous value."* A trust store entry is the same class
/// of thing — a decision somebody made that changes what may run on the machine — and until this
/// existed the store recorded only the outcome.
///
/// # Every field is optional, and that is the point
///
/// A store written by hand before this existed has none of them, and the honest report of such an
/// entry is **`unrecorded`** rather than a fabricated date. Defaulting `added_at` to the time the
/// file was first read would produce a provenance that is confidently wrong, which is worse than
/// one that is admittedly absent — the same rule §7.2 applies to a guard with no input.
///
/// [`Provenance::is_recorded`] is what a report asks, and [`skynetd trust list`] prints
/// *"provenance unrecorded"* for the rest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provenance {
    /// Wall time, RFC 3339, UTC, as written by whoever added the key.
    ///
    /// **Wall time and not monotonic**, which is not a slip: §35.1 gives `CLOCK_REALTIME` exactly
    /// three jobs — *"logging, scheduling, correlation with the Hub"* — and an audit record is the
    /// first of them. A monotonic reading here would be a number that means nothing once the
    /// machine reboots, and this record has to be comparable against a person's calendar. Nothing
    /// reads it to make a decision; a stepped clock spoils a record and cannot spoil a verdict.
    pub added_at: Option<String>,
    /// Who added it — a credential label, a person, a provisioning run.
    pub added_by: Option<String>,
    /// Why. The one field an operator writes for their successor rather than for themselves.
    pub note: Option<String>,
}

impl Provenance {
    /// Whether anybody wrote down anything about where this key came from.
    pub fn is_recorded(&self) -> bool {
        self.added_at.is_some() || self.added_by.is_some()
    }

    /// Read the three optional strings from an entry.
    ///
    /// An empty string is read as absence. A file that says `"added_by": ""` has not recorded an
    /// actor, and treating the empty string as one would put a blank name in an audit report.
    pub fn from_json(entry: &Json) -> Provenance {
        let field = |name: &str| {
            entry
                .get(name)
                .and_then(Json::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
        };
        Provenance {
            added_at: field("added_at"),
            added_by: field("added_by"),
            note: field("note"),
        }
    }

    /// A sentence for a report, or the fact that there is none.
    pub fn describe(&self) -> String {
        match (&self.added_at, &self.added_by) {
            (Some(at), Some(by)) => format!("added {at} by {by}"),
            (Some(at), None) => format!("added {at}, by nobody recorded"),
            (None, Some(by)) => format!("added by {by}, at no recorded time"),
            (None, None) => "provenance unrecorded — this key predates the audited store, or was \
                             added by editing the file"
                .to_string(),
        }
    }
}

/// A key this robot has been told to trust, the operator's name for it, and where it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedKey {
    /// This robot's name for the signer. Local, and only ever used in reports.
    pub label: String,
    /// The key.
    pub key: VerifyingKey,
    /// Who trusted it and when (§38.3), or the absence of that record.
    pub provenance: Provenance,
}

impl TrustedKey {
    /// The key as the file spells it.
    pub fn key_hex(&self) -> String {
        let rendered = self.key.to_hex();
        core::str::from_utf8(&rendered)
            .expect("hex is ASCII")
            .to_string()
    }

    /// The entry as it is written back.
    pub fn to_json(&self) -> Json {
        let mut entry = Json::obj();
        entry.set("label", Json::Str(self.label.clone()));
        entry.set("key", Json::Str(self.key_hex()));
        for (name, value) in [
            ("note", &self.provenance.note),
            ("added_at", &self.provenance.added_at),
            ("added_by", &self.provenance.added_by),
        ] {
            if let Some(text) = value {
                entry.set(name, Json::Str(text.clone()));
            }
        }
        entry
    }
}

/// An entry that could not be loaded, and why.
///
/// The raw document is kept because this store gets **written back** (§38.3's audited surface), and
/// a rewrite that dropped what it could not parse would delete the one entry an operator is trying
/// to find. `skynetd trust add` on a file with a mistyped key must not quietly take the typo away:
/// the typo is the bug report.
#[derive(Debug, Clone, PartialEq)]
pub struct SkippedKey {
    /// Where it was, so an operator can find it in the file.
    pub at: String,
    /// What is wrong.
    pub detail: String,
    /// The entry exactly as it was read, so writing the file back preserves it.
    pub raw: Json,
}

/// One line of the `keys` array, in the order the file gives it.
///
/// Order is preserved rather than reconstructed, so a rewrite is a diff an operator recognises.
///
/// Clippy is right that the variants differ in size: a [`TrustedKey`] carries a [`VerifyingKey`],
/// which caches the *decompressed* curve point so that verifying does not re-decompress on every
/// signature. Boxing it would put an allocation on the entry a robot actually uses, to save a few
/// hundred bytes on a list that a provisioned robot holds three of.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    /// A key this robot trusts.
    Trusted(TrustedKey),
    /// A line that could not be used, kept verbatim.
    Skipped(SkippedKey),
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
#[derive(Debug, Clone, PartialEq)]
pub struct TrustStore {
    entries: Vec<Entry>,
    source: Source,
}

/// How an operator names the entry they mean.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    /// This robot's name for a key.
    Label(String),
    /// The key itself, for a store holding two of the same label — or one whose labels are hex.
    Key(VerifyingKey),
    /// A skipped line's position, which is the only handle a malformed entry has.
    At(String),
}

impl Selector {
    /// How to say it back in an error.
    pub fn describe(&self) -> String {
        match self {
            Selector::Label(label) => format!("a key labelled `{label}`"),
            Selector::Key(key) => {
                let rendered = key.to_hex();
                format!(
                    "the key `{}`",
                    core::str::from_utf8(&rendered).expect("hex is ASCII")
                )
            }
            Selector::At(at) => format!("an entry at `{at}`"),
        }
    }
}

/// What a removal took out.
///
/// Same size asymmetry as [`Entry`], for the same reason, and returned once per command.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Removed {
    /// A key this robot was trusting.
    Trusted(TrustedKey),
    /// A line it never could.
    Skipped(SkippedKey),
}

/// Why a change to the store was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeError {
    /// The file is there and does not parse, so a rewrite would destroy it.
    StoreUnreadable {
        /// What is wrong with it.
        detail: String,
    },
    /// A key with no label is a key no report can name.
    LabelEmpty,
    /// The key is already trusted, under this label.
    KeyHeld {
        /// The label it is held under.
        label: String,
    },
    /// The label is taken, by this key.
    LabelHeld {
        /// The key holding it.
        key: String,
    },
    /// Nothing in the store matches.
    NotHeld {
        /// What was asked for.
        selector: String,
    },
}

impl ChangeError {
    /// A sentence for an operator, naming the remedy rather than the rule.
    pub fn reason(&self) -> String {
        match self {
            ChangeError::StoreUnreadable { detail } => format!(
                "the trust store {detail}, and writing over it would destroy the keys it still \
                 holds. Repair the file first — `skynetd trust list` says what it can read"
            ),
            ChangeError::LabelEmpty => {
                "a key needs a label: a report that names a signer by 64 hex characters has told \
                 nobody anything"
                    .to_string()
            }
            ChangeError::KeyHeld { label } => format!(
                "this robot already trusts that key, as `{label}`. Remove it first if the label is \
                 what needs to change"
            ),
            ChangeError::LabelHeld { key } => format!(
                "`{key}` already holds that label. Two keys under one name make every report \
                 ambiguous about which one signed"
            ),
            ChangeError::NotHeld { selector } => {
                format!("this robot does not hold {selector}")
            }
        }
    }
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
            entries: Vec::new(),
            source: Source::Missing,
        }
    }

    /// A store holding nothing, because the file is unusable.
    fn broken(detail: String) -> TrustStore {
        TrustStore {
            entries: Vec::new(),
            source: Source::Unreadable { detail },
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
            Err(error) => return TrustStore::broken(format!("could not be read: {error}")),
        };
        TrustStore::from_text(&text)
    }

    /// Parse the store's document.
    pub fn from_text(text: &str) -> TrustStore {
        let document = match Json::parse(text) {
            Ok(document) => document,
            Err(error) => return TrustStore::broken(format!("does not parse: {error}")),
        };

        let mut entries: Vec<Entry> = Vec::new();
        for (index, entry) in document.arr_or_empty("keys").iter().enumerate() {
            let at = format!("keys[{index}]");
            let skip = |detail: &str| {
                Entry::Skipped(SkippedKey {
                    at: at.clone(),
                    detail: detail.to_string(),
                    raw: entry.clone(),
                })
            };
            let label = entry
                .get("label")
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string();
            let Some(hex) = entry.get("key").and_then(Json::as_str) else {
                entries.push(skip("this entry names no `key`"));
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
                    if entries.iter().any(|held| match held {
                        Entry::Trusted(trusted) => trusted.key == key,
                        Entry::Skipped(_) => false,
                    }) {
                        entries.push(skip("this key already appears earlier in the file"));
                        continue;
                    }
                    entries.push(Entry::Trusted(TrustedKey {
                        label,
                        key,
                        provenance: Provenance::from_json(entry),
                    }));
                }
                Err(error) => entries.push(skip(error.reason())),
            }
        }

        TrustStore {
            entries,
            source: Source::Loaded,
        }
    }

    /// Every line of the file, usable or not, in file order.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// The keys, in file order.
    pub fn keys(&self) -> Vec<&TrustedKey> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Trusted(key) => Some(key),
                Entry::Skipped(_) => None,
            })
            .collect()
    }

    /// Entries that could not be loaded.
    pub fn skipped(&self) -> Vec<&SkippedKey> {
        self.entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Skipped(skipped) => Some(skipped),
                Entry::Trusted(_) => None,
            })
            .collect()
    }

    /// Where the contents came from.
    pub fn source(&self) -> &Source {
        &self.source
    }

    /// Whether this robot trusts anybody at all.
    pub fn is_empty(&self) -> bool {
        self.keys().is_empty()
    }

    /// The label this robot has for a key, if it trusts it.
    pub fn label_for(&self, key: &VerifyingKey) -> Option<&str> {
        self.keys()
            .into_iter()
            .find(|k| &k.key == key)
            .map(|k| k.label.as_str())
    }

    /// The entry this robot has for a key, if it trusts it.
    pub fn entry_for(&self, key: &VerifyingKey) -> Option<&TrustedKey> {
        self.keys().into_iter().find(|k| &k.key == key)
    }

    /// The document, ready to be written back.
    ///
    /// Round-trips: skipped lines are re-emitted exactly as they were read, in place, so a rewrite
    /// changes the one entry it means to change and nothing else. A store that dropped what it
    /// could not parse would make `trust add` a way to lose a key.
    pub fn to_json(&self) -> Json {
        let mut keys = Json::arr();
        for entry in &self.entries {
            keys.push(match entry {
                Entry::Trusted(trusted) => trusted.to_json(),
                Entry::Skipped(skipped) => skipped.raw.clone(),
            });
        }
        let mut document = Json::obj();
        document.set("keys", keys);
        document
    }

    /// Trust a key (§38.3).
    ///
    /// Refused rather than merged when the key is already held, and the error names the label it is
    /// held under. Silently relabelling would rewrite a decision somebody else recorded, and the
    /// case it would hide — the same key arriving under two names — is the case worth seeing.
    pub fn add(&mut self, key: TrustedKey) -> Result<(), ChangeError> {
        if let Source::Unreadable { detail } = &self.source {
            // Writing over a file that does not parse would destroy whatever is in it, including
            // the keys this robot is currently trusting. The remedy is a person, not a rewrite.
            return Err(ChangeError::StoreUnreadable {
                detail: detail.clone(),
            });
        }
        if key.label.trim().is_empty() {
            return Err(ChangeError::LabelEmpty);
        }
        if let Some(existing) = self.entry_for(&key.key) {
            return Err(ChangeError::KeyHeld {
                label: existing.label.clone(),
            });
        }
        if let Some(existing) = self.keys().into_iter().find(|held| held.label == key.label) {
            return Err(ChangeError::LabelHeld {
                key: existing.key_hex(),
            });
        }
        self.entries.push(Entry::Trusted(key));
        self.source = Source::Loaded;
        Ok(())
    }

    /// Stop trusting whatever `selector` names, and return what was removed.
    ///
    /// Removal is permitted on an unreadable store's *skipped* lines and nothing else, because
    /// removing trust is the safe direction and a broken entry is the thing somebody is here to
    /// remove. Nothing else about a file that does not parse is knowable.
    pub fn remove(&mut self, selector: &Selector) -> Result<Removed, ChangeError> {
        let position = self
            .entries
            .iter()
            .position(|entry| match (entry, selector) {
                (Entry::Trusted(trusted), Selector::Label(label)) => &trusted.label == label,
                (Entry::Trusted(trusted), Selector::Key(key)) => &trusted.key == key,
                (Entry::Skipped(skipped), Selector::At(at)) => &skipped.at == at,
                _ => false,
            });
        let Some(position) = position else {
            return Err(ChangeError::NotHeld {
                selector: selector.describe(),
            });
        };
        Ok(match self.entries.remove(position) {
            Entry::Trusted(trusted) => Removed::Trusted(trusted),
            Entry::Skipped(skipped) => Removed::Skipped(skipped),
        })
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
                            self.keys().len()
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
                if self.is_empty() {
                    lines.push(
                        "trust: the trust store is present and names no keys, so every signature \
                         will report `untrusted`"
                            .to_string(),
                    );
                }
            }
        }
        for skipped in self.skipped() {
            lines.push(format!(
                "trust: {} was skipped: {}",
                skipped.at, skipped.detail
            ));
        }

        // §38.3's question, asked of the store rather than of one entry: *who* decided this robot
        // should believe these signers. A key with no provenance still verifies — it is a real
        // decision somebody made — and the thing that is missing is the record of who made it,
        // which is a gap in the audit rather than in the security. So this is a notice and not a
        // refusal, and it names the count because "one of nine" and "nine of nine" are different
        // machines: the first is an entry somebody hand-edited, the second a robot provisioned
        // before anything recorded provenance at all.
        let unrecorded: Vec<&str> = self
            .keys()
            .into_iter()
            .filter(|key| !key.provenance.is_recorded())
            .map(|key| key.label.as_str())
            .collect();
        if !unrecorded.is_empty() {
            lines.push(format!(
                "trust: {} of {} trusted key(s) record nobody who added them and no date — {}. \
                 `skynetd trust list` shows the rest",
                unrecorded.len(),
                self.keys().len(),
                unrecorded.join(", ")
            ));
        }
        lines
    }

    /// The store as a report: every line of the file, with provenance (§38.3).
    ///
    /// Separate from [`TrustStore::to_json`], which is the file. This is what a console and
    /// `skynetd trust list` read, and it carries the two things the file does not spell out — the
    /// source of the store, and whether each entry's provenance was ever written down.
    pub fn to_report(&self) -> Json {
        let mut keys = Json::arr();
        for entry in &self.entries {
            let mut row = Json::obj();
            match entry {
                Entry::Trusted(trusted) => {
                    row.set("usable", Json::Bool(true));
                    row.set("label", Json::Str(trusted.label.clone()));
                    row.set("key", Json::Str(trusted.key_hex()));
                    row.set(
                        "provenance_recorded",
                        Json::Bool(trusted.provenance.is_recorded()),
                    );
                    row.set("provenance", Json::Str(trusted.provenance.describe()));
                    for (name, value) in [
                        ("note", &trusted.provenance.note),
                        ("added_at", &trusted.provenance.added_at),
                        ("added_by", &trusted.provenance.added_by),
                    ] {
                        if let Some(text) = value {
                            row.set(name, Json::Str(text.clone()));
                        }
                    }
                }
                Entry::Skipped(skipped) => {
                    row.set("usable", Json::Bool(false));
                    row.set("at", Json::Str(skipped.at.clone()));
                    row.set("detail", Json::Str(skipped.detail.clone()));
                }
            }
            keys.push(row);
        }

        let mut document = Json::obj();
        document.set(
            "source",
            Json::Str(
                match &self.source {
                    Source::Missing => "missing",
                    Source::Loaded => "loaded",
                    Source::Unreadable { .. } => "unreadable",
                }
                .to_string(),
            ),
        );
        if let Source::Unreadable { detail } = &self.source {
            document.set("source_detail", Json::Str(detail.clone()));
        }
        document.set("keys", keys);
        let mut notices = Json::arr();
        for line in self.notices() {
            notices.push(Json::Str(line));
        }
        document.set("notices", notices);
        document
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

    /// A store as `skynetd trust add` would have written it — provenance and all.
    ///
    /// The provenance is here rather than omitted because a store without it now raises a notice,
    /// and a helper that produced one would make every test in this module assert against a robot
    /// somebody hand-edited. That case has its own test.
    fn store_with(entries: &[(&str, &SigningKey)]) -> TrustStore {
        let mut text = String::from("{\"keys\":[");
        for (index, (label, key)) in entries.iter().enumerate() {
            if index > 0 {
                text.push(',');
            }
            text.push_str(&format!(
                "{{\"label\":\"{label}\",\"key\":\"{}\",\"added_at\":\"2026-08-30T12:00:00Z\",\
                 \"added_by\":\"alice\"}}",
                public_hex(key)
            ));
        }
        text.push_str("]}");
        TrustStore::from_text(&text)
    }

    /// The same store as a hand-edited file: keys, and nothing about where they came from.
    fn store_without_provenance(entries: &[(&str, &SigningKey)]) -> TrustStore {
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

    // ── The audited surface (§38.3) ────────────────────────────────────────

    fn added(label: &str, key: &SigningKey, by: Option<&str>) -> TrustedKey {
        TrustedKey {
            label: label.to_string(),
            key: key.verifying_key(),
            provenance: Provenance {
                added_at: Some("2026-08-30T12:00:00Z".to_string()),
                added_by: by.map(str::to_string),
                note: None,
            },
        }
    }

    #[test]
    fn provenance_is_read_back_off_the_entry() {
        let store = store_with(&[("acme-robotics", &key(1))]);
        let held = store.keys();
        let provenance = &held[0].provenance;

        assert_eq!(provenance.added_at.as_deref(), Some("2026-08-30T12:00:00Z"));
        assert_eq!(provenance.added_by.as_deref(), Some("alice"));
        assert!(provenance.is_recorded());
        assert_eq!(provenance.describe(), "added 2026-08-30T12:00:00Z by alice");
    }

    /// An empty string is not an actor. A file saying `"added_by": ""` has recorded nobody, and
    /// carrying it through would put a blank name in an audit report.
    #[test]
    fn an_empty_provenance_field_reads_as_absence() {
        let store = TrustStore::from_text(&format!(
            "{{\"keys\":[{{\"label\":\"a\",\"key\":\"{}\",\"added_by\":\"\"}}]}}",
            public_hex(&key(1))
        ));
        assert!(!store.keys()[0].provenance.is_recorded());
    }

    /// A key nobody recorded still verifies — the gap is in the audit, not in the security — so
    /// this is a notice rather than a refusal, and it names which entries.
    #[test]
    fn a_store_nobody_recorded_says_so_without_refusing_anything() {
        let signer = key(1);
        let store = store_without_provenance(&[("acme-robotics", &signer)]);
        let message = package_message(DIGEST);

        assert_eq!(
            store.check(&message, Some(&sign(&signer, &message))).state,
            SignatureState::Verified,
        );
        let notices = store.notices().join("\n");
        assert!(notices.contains("1 of 1 trusted key(s)"), "{notices}");
        assert!(notices.contains("acme-robotics"), "{notices}");
    }

    /// The property that makes `trust add` safe to run on a file somebody broke: the broken line
    /// comes back out exactly as it went in, in the position it was in.
    #[test]
    fn a_rewrite_preserves_an_entry_it_could_not_parse() {
        let before = format!(
            "{{\"keys\":[{{\"label\":\"good\",\"key\":\"{}\"}},             {{\"label\":\"typo\",\"key\":\"not-hex\",\"note\":\"keep me\"}}]}}",
            public_hex(&key(1))
        );
        let mut store = TrustStore::from_text(&before);
        assert_eq!(store.skipped().len(), 1);

        store
            .add(added("second", &key(2), Some("carol")))
            .expect("add");
        let written = store.to_json().to_canonical().expect("canonical");

        assert!(written.contains("not-hex"), "{written}");
        assert!(written.contains("keep me"), "{written}");

        // And it is still in the middle: order is preserved, so a rewrite is a diff somebody reads.
        let reread = TrustStore::from_text(&written);
        assert!(
            matches!(reread.entries()[1], Entry::Skipped(_)),
            "{written}"
        );
        assert_eq!(reread.keys().len(), 2);
        assert_eq!(reread.keys()[1].label, "second");
    }

    #[test]
    fn a_store_written_back_reads_back_identically() {
        let mut store = TrustStore::from_text("{\"keys\":[]}");
        store
            .add(added("acme-robotics", &key(1), Some("alice")))
            .expect("add");
        store.add(added("bench", &key(2), None)).expect("add");

        let once = store.to_json().to_canonical().expect("canonical");
        let twice = TrustStore::from_text(&once)
            .to_json()
            .to_canonical()
            .expect("canonical");
        assert_eq!(once, twice);
    }

    #[test]
    fn the_same_key_twice_is_refused_and_names_the_label_holding_it() {
        let mut store = TrustStore::from_text("{\"keys\":[]}");
        store
            .add(added("acme-robotics", &key(1), None))
            .expect("add");

        assert_eq!(
            store.add(added("acme-again", &key(1), None)),
            Err(ChangeError::KeyHeld {
                label: "acme-robotics".to_string()
            }),
        );
    }

    /// Two keys under one name make every report ambiguous about which one signed, which is the
    /// one thing a label exists to prevent.
    #[test]
    fn the_same_label_twice_is_refused() {
        let mut store = TrustStore::from_text("{\"keys\":[]}");
        store
            .add(added("acme-robotics", &key(1), None))
            .expect("add");

        assert!(matches!(
            store.add(added("acme-robotics", &key(2), None)),
            Err(ChangeError::LabelHeld { .. })
        ));
    }

    #[test]
    fn a_key_with_no_label_is_refused() {
        let mut store = TrustStore::from_text("{\"keys\":[]}");
        assert_eq!(
            store.add(added("   ", &key(1), None)),
            Err(ChangeError::LabelEmpty)
        );
    }

    /// Writing over a file that does not parse would destroy the keys it still holds, and this
    /// robot cannot tell which those are. The remedy is a person.
    #[test]
    fn nothing_is_added_to_a_store_that_does_not_parse() {
        let mut store = TrustStore::from_text("{ not json");
        assert!(matches!(
            store.add(added("acme", &key(1), None)),
            Err(ChangeError::StoreUnreadable { .. })
        ));
    }

    #[test]
    fn removing_a_key_stops_it_verifying() {
        let signer = key(1);
        let mut store = store_with(&[("acme-robotics", &signer)]);
        let message = package_message(DIGEST);
        assert_eq!(
            store.check(&message, Some(&sign(&signer, &message))).state,
            SignatureState::Verified
        );

        let removed = store
            .remove(&Selector::Label("acme-robotics".to_string()))
            .expect("remove");
        assert!(matches!(removed, Removed::Trusted(_)));
        assert_eq!(
            store.check(&message, Some(&sign(&signer, &message))).state,
            SignatureState::Untrusted,
        );
    }

    /// A malformed line has no label and no key, so its position is the only handle on it — and
    /// removing it is the reason somebody came to this command.
    #[test]
    fn a_skipped_line_can_be_removed_by_position() {
        let mut store =
            TrustStore::from_text("{\"keys\":[{\"label\":\"typo\",\"key\":\"not-hex\"}]}");
        assert_eq!(store.skipped().len(), 1);

        let removed = store
            .remove(&Selector::At("keys[0]".to_string()))
            .expect("remove");
        assert!(matches!(removed, Removed::Skipped(_)));
        assert!(store.entries().is_empty());
    }

    #[test]
    fn removing_something_absent_says_what_was_asked_for() {
        let mut store = store_with(&[("acme-robotics", &key(1))]);
        let error = store
            .remove(&Selector::Label("nobody".to_string()))
            .expect_err("absent");
        assert!(error.reason().contains("`nobody`"), "{error:?}");
    }

    #[test]
    fn the_report_carries_provenance_and_the_source() {
        let store = store_with(&[("acme-robotics", &key(1))]);
        let report = store.to_report().to_canonical().expect("canonical");

        assert!(report.contains("\"source\":\"loaded\""), "{report}");
        assert!(report.contains("\"provenance_recorded\":true"), "{report}");
        assert!(
            report.contains("added 2026-08-30T12:00:00Z by alice"),
            "{report}"
        );

        let missing = TrustStore::missing()
            .to_report()
            .to_canonical()
            .expect("canonical");
        assert!(missing.contains("\"source\":\"missing\""), "{missing}");
    }
}

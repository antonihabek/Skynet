//! Ed25519 signature verification (RFC 8032), `no_std` and dependency-free.
//!
//! §19 requires that a driver run only if it is *"signed by a key the operator has trusted"*, §37.1
//! puts a signature check immediately after the digest on the install path, and §38.1 makes the
//! operator profile carry one. Until this crate existed, all three were waiting on the same missing
//! primitive, and `skynetd` reported `signature_unverifiable` on every package it saw — honestly,
//! and uselessly.
//!
//! ```
//! # use skynet_ed25519::{Signature, VerifyingKey};
//! // RFC 8032 §7.1, the first test vector: an empty message.
//! let key = VerifyingKey::from_hex(
//!     "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
//! ).unwrap();
//! let sig = Signature::from_hex(
//!     "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155\
//!      5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
//! ).unwrap();
//! assert!(key.verify(b"", &sig).is_ok());
//! ```
//!
//! # Why this is written here rather than depended on
//!
//! The workspace has no third-party dependencies, for reasons its `Cargo.toml` gives: `no_std`
//! without `alloc` rules most of the ecosystem out by construction, byte-identical conformance
//! (§31.4) means the number formatting has to be ours, and a vendor building a driver in 2036 has
//! to be able to build this with no network. A signature verifier is the place where that policy
//! costs the most and where abandoning it would cost more: the alternative is a supply chain
//! reaching into the one component whose whole job is to decide what may run on a robot.
//!
//! Ed25519 has not changed since 2011 and its parameters are fixed by RFC 8032, which makes it — as
//! with SHA-256 — an unusually safe thing to vendor. What it is not is easy, so every constant in
//! here is checked against its *definition* in the tests rather than against itself, and the whole
//! is held to RFC 8032's published vectors and to the encoding cases that are known to divide
//! implementations.
//!
//! # Verification only
//!
//! There is no signing outside the `test-signing` feature, and that is a decision about where
//! secrets live. The Hub signs releases (§19); a robot holds public keys and verifies. Signing
//! multiplies a *secret* scalar, which demands constant-time arithmetic that this implementation
//! deliberately does not provide — see [`field`] — and shipping a signer that leaks its key through
//! timing would be worse than shipping no signer at all.
//!
//! `test-signing` exists so tests can prove the *verified* path works over bytes they chose, rather
//! than pinning a few signatures from some other tool and testing four fixed messages forever.
//! Under `resolver = "2"` a feature enabled by a dev-dependency is not enabled for a normal build.
//!
//! # What is checked, and what is not
//!
//! Checked, all of it strictly:
//!
//! | | |
//! |---|---|
//! | `y >= p` in a key or commitment | refused — one point would otherwise have two encodings |
//! | `y` names no point | refused |
//! | `x == 0` with the sign bit set | refused — a second spelling of a point that has one |
//! | a small-order key | refused — see [`KeyError::SmallOrder`] |
//! | `S >= L` | refused — see [`Signature::from_bytes`] |
//! | the equation | `[S]B == R + [k]A`, cofactorless |
//!
//! Not checked: whether the key lies in the prime-order subgroup. Excluding the eight small-order
//! points removes the keys that verify *anything*; a torsion-carrying key of full order is a
//! stranger thing, it costs a whole scalar multiplication to detect, and the cofactorless equation
//! above already refuses the signatures that a cofactored one would let such a key equivocate over.
//! It is recorded here rather than left to be discovered.
//!
//! Also not checked, because it is not this crate's question: whether the key is one the **operator
//! trusts**. A verified signature by an unknown key is not an accepted package. That decision has
//! an owner and it is the trust store on the robot.

#![no_std]
#![forbid(unsafe_code)]

// The test harness needs `std`; nothing outside `cfg(test)` may touch it.
#[cfg(test)]
extern crate std;

pub mod edwards;
pub mod field;
pub mod scalar;
mod sha512;

#[cfg(test)]
mod tests;

use edwards::{DecompressError, Point};
use scalar::Scalar;
use sha512::Sha512;

/// A public key, parsed and checked.
///
/// Parsing is separate from verifying because the two failures have different owners and happen at
/// different times. A malformed key is a fact about the **trust store**, discoverable when the
/// store is read — at boot, or when an operator adds a key — and an operator who typed one badly
/// should hear about it then. A signature that does not verify is a fact about a **package**, and
/// arrives later. Collapsing them into one call would report a broken trust store as a rejected
/// package, months after the mistake, on a robot in a warehouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyingKey {
    encoded: [u8; 32],
    point: Point,
}

/// Why 32 bytes are not a usable public key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyError {
    /// Not 64 hex characters.
    NotHex,
    /// `y` is at or above `p` — a field element spelled the long way, giving one key two names.
    NotCanonical,
    /// No point on the curve has this `y`.
    NotOnCurve,
    /// One of the eight points of order dividing 8.
    ///
    /// Refused because such a key is not an identity. The order-1 point verifies a signature under
    /// any message with `S == 0`, and the rest have the same property over a small set — so a
    /// package "signed" by one is attested to by nobody, while an operator reading a verified
    /// verdict would believe otherwise. There is no legitimate reason to trust one, and the eight
    /// of them are the only public keys a stranger can produce without a private key at all.
    SmallOrder,
}

impl KeyError {
    /// A sentence for an operator, ending in whose problem it is.
    pub fn reason(self) -> &'static str {
        match self {
            KeyError::NotHex => {
                "not a 64-character hex public key — check the trust store entry for stray \
                 whitespace or a truncated paste"
            }
            KeyError::NotCanonical => {
                "the key is written in a non-canonical form that would give it a second spelling; \
                 whoever published it should re-export it"
            }
            KeyError::NotOnCurve => {
                "these 32 bytes are not an Ed25519 public key — most likely the wrong key type or a \
                 corrupted entry"
            }
            KeyError::SmallOrder => {
                "this is a small-order point, which verifies signatures nobody produced. It must \
                 not be trusted, and the fact that somebody offered it is worth investigating"
            }
        }
    }
}

impl VerifyingKey {
    /// Parse and check 32 bytes.
    pub fn from_bytes(bytes: &[u8; 32]) -> Result<VerifyingKey, KeyError> {
        let point = Point::decompress_detail(bytes).map_err(|e| match e {
            DecompressError::NotCanonical => KeyError::NotCanonical,
            DecompressError::NotOnCurve => KeyError::NotOnCurve,
        })?;
        if point.is_small_order() {
            return Err(KeyError::SmallOrder);
        }
        Ok(VerifyingKey {
            encoded: *bytes,
            point,
        })
    }

    /// Parse 64 hex characters.
    ///
    /// Hex because that is how a key reaches a robot: in a file an operator edited, in a manifest,
    /// or read off a screen. Lower case on the way out, either case on the way in.
    pub fn from_hex(text: &str) -> Result<VerifyingKey, KeyError> {
        let bytes = unhex::<32>(text).ok_or(KeyError::NotHex)?;
        VerifyingKey::from_bytes(&bytes)
    }

    /// The 32 bytes, as given.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.encoded
    }

    /// The 64-character lower-case hex form, which is how a key is named in a report.
    pub fn to_hex(&self) -> [u8; 64] {
        hex(&self.encoded)
    }

    /// Verify `signature` over `message`.
    ///
    /// The cofactorless equation, `[S]B == R + [k]A`. RFC 8032 permits either this or the
    /// cofactored `[8][S]B == [8]R + [8][k]A`, and the two disagree on signatures involving torsion:
    /// the cofactored form accepts some the cofactorless form rejects. Accepting more here would
    /// mean two robots reading the same signature and reaching different verdicts — and the verdict
    /// gates what runs on a machine that can move. The stricter one is the one where two
    /// implementations agree.
    pub fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), VerifyError> {
        let commitment = Point::decompress_detail(&signature.r).map_err(|e| match e {
            DecompressError::NotCanonical => VerifyError::CommitmentNotCanonical,
            DecompressError::NotOnCurve => VerifyError::CommitmentNotOnCurve,
        })?;

        let mut hasher = Sha512::new();
        hasher.update(&signature.r);
        hasher.update(&self.encoded);
        hasher.update(message);
        let challenge = Scalar::reduce_wide(&hasher.finish());

        let basepoint = edwards::basepoint().ok_or(VerifyError::Mismatch)?;
        let lhs = basepoint.scalar_mul(&signature.s);
        let rhs = commitment + self.point.scalar_mul(&challenge);

        if lhs.eq(&rhs) {
            Ok(())
        } else {
            Err(VerifyError::Mismatch)
        }
    }
}

/// A signature: the commitment `R` and the scalar `S`.
///
/// `S` is held reduced, which is where the canonicality rule is enforced — see
/// [`Signature::from_bytes`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signature {
    r: [u8; 32],
    s: Scalar,
}

/// Why 64 bytes are not a well-formed signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureError {
    /// Not 128 hex characters.
    NotHex,
    /// `S` is at or above `L`.
    ScalarNotCanonical,
}

impl SignatureError {
    /// A sentence for an operator.
    pub fn reason(self) -> &'static str {
        match self {
            SignatureError::NotHex => "not a 128-character hex signature",
            SignatureError::ScalarNotCanonical => {
                "the signature's scalar is not reduced, which is a second encoding of a signature \
                 that already has one. Refused rather than reduced, so that a signature is \
                 identified by its bytes"
            }
        }
    }
}

impl Signature {
    /// Parse 64 bytes: `R` then `S`.
    ///
    /// `S` must be strictly below `L`. It is tempting to reduce instead — the equation would still
    /// hold — and that is exactly the mistake: `S` and `S + L` are then two different 64-byte
    /// strings that both verify the same message under the same key. Everything downstream that
    /// identifies a signature *by its bytes* — a replay window, an audit entry, the black box record
    /// of what was admitted (§13) — would see two events where the signer produced one.
    pub fn from_bytes(bytes: &[u8; 64]) -> Result<Signature, SignatureError> {
        let mut r = [0u8; 32];
        r.copy_from_slice(&bytes[..32]);
        let mut s = [0u8; 32];
        s.copy_from_slice(&bytes[32..]);
        let s = Scalar::from_canonical_bytes(&s).ok_or(SignatureError::ScalarNotCanonical)?;
        Ok(Signature { r, s })
    }

    /// Parse 128 hex characters. Whitespace is ignored, so a signature may be wrapped in a file.
    pub fn from_hex(text: &str) -> Result<Signature, SignatureError> {
        let bytes = unhex::<64>(text).ok_or(SignatureError::NotHex)?;
        Signature::from_bytes(&bytes)
    }
}

/// Why a signature did not verify.
///
/// Separate from [`KeyError`] because the remedies are different people's: a key problem is the
/// trust store's, and everything here is the package's or its publisher's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    /// `R` is at or above `p`.
    CommitmentNotCanonical,
    /// `R` is not a point on the curve.
    CommitmentNotOnCurve,
    /// Well-formed, and the equation did not hold.
    ///
    /// This is the only outcome that means what an operator will read it as: **these bytes were not
    /// signed by this key**. Either they were changed after signing, or they were signed by
    /// somebody else. The two cannot be told apart from here, and both are the same instruction —
    /// stop.
    Mismatch,
}

impl VerifyError {
    /// A sentence for an operator.
    pub fn reason(self) -> &'static str {
        match self {
            VerifyError::CommitmentNotCanonical | VerifyError::CommitmentNotOnCurve => {
                "the signature is malformed — its first half is not a curve point. This is a broken \
                 or truncated signature rather than a wrong one"
            }
            VerifyError::Mismatch => {
                "the signature does not verify against this key: the bytes were changed after \
                 signing, or somebody else signed them. Treat this as tampering until proven \
                 otherwise"
            }
        }
    }
}

/// Lower-case hex of 32 bytes.
fn hex(bytes: &[u8; 32]) -> [u8; 64] {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = [b'0'; 64];
    for (i, byte) in bytes.iter().enumerate() {
        out[i * 2] = DIGITS[usize::from(byte >> 4)];
        out[i * 2 + 1] = DIGITS[usize::from(byte & 0x0f)];
    }
    out
}

/// Parse exactly `N` bytes of hex, ignoring ASCII whitespace.
///
/// Whitespace is ignored because these strings are long enough that a file will wrap them, and a
/// key rejected for a line break is a key an operator will paste onto one line and stop reading
/// errors from.
fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    let mut out = [0u8; N];
    let mut nibbles = 0usize;
    for byte in text.as_bytes() {
        if byte.is_ascii_whitespace() {
            continue;
        }
        let value = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            b'A'..=b'F' => byte - b'A' + 10,
            _ => return None,
        };
        if nibbles >= N * 2 {
            return None;
        }
        if nibbles % 2 == 0 {
            out[nibbles / 2] = value << 4;
        } else {
            out[nibbles / 2] |= value;
        }
        nibbles += 1;
    }
    if nibbles == N * 2 {
        Some(out)
    } else {
        None
    }
}

/// Signing, for tests and for nothing that ships. See the crate documentation.
#[cfg(feature = "test-signing")]
pub mod signing {
    use super::{edwards, hex, scalar::Scalar, sha512, VerifyingKey};

    /// A signing key, expanded from a 32-byte seed.
    ///
    /// **Not constant-time.** The scalar multiplication underneath branches on bits of the secret
    /// scalar. Under a test that is irrelevant; anywhere else it is a key-recovery oracle, which is
    /// why this module is behind a feature nothing that ships may name.
    #[derive(Debug, Clone, Copy)]
    pub struct SigningKey {
        scalar: Scalar,
        prefix: [u8; 32],
        public: [u8; 32],
    }

    impl SigningKey {
        /// Expand a 32-byte seed, per RFC 8032 §5.1.5.
        pub fn from_seed(seed: &[u8; 32]) -> SigningKey {
            let h = sha512::digest(seed);

            let mut clamped = [0u8; 32];
            clamped.copy_from_slice(&h[..32]);
            clamped[0] &= 248;
            clamped[31] &= 127;
            clamped[31] |= 64;

            // Clamping sets bit 254, so the result is above `L` and cannot be decoded as a reduced
            // scalar. Reducing it changes neither `[a]B` nor `r + k a mod L`, because the basepoint
            // has order `L`.
            let mut wide = [0u8; 64];
            wide[..32].copy_from_slice(&clamped);
            let scalar = Scalar::reduce_wide(&wide);

            let mut prefix = [0u8; 32];
            prefix.copy_from_slice(&h[32..]);

            let public = edwards::basepoint()
                .expect("the basepoint constant decompresses")
                .scalar_mul(&scalar)
                .compress();

            SigningKey {
                scalar,
                prefix,
                public,
            }
        }

        /// The matching public key.
        pub fn verifying_key(&self) -> VerifyingKey {
            VerifyingKey::from_bytes(&self.public).expect("a derived public key is a valid point")
        }

        /// The public key as lower-case hex, which is the form a trust store holds.
        pub fn public_hex(&self) -> [u8; 64] {
            hex(&self.public)
        }

        /// Sign, per RFC 8032 §5.1.6.
        pub fn sign(&self, message: &[u8]) -> [u8; 64] {
            let mut hasher = sha512::Sha512::new();
            hasher.update(&self.prefix);
            hasher.update(message);
            let r = Scalar::reduce_wide(&hasher.finish());

            let commitment = edwards::basepoint()
                .expect("the basepoint constant decompresses")
                .scalar_mul(&r)
                .compress();

            let mut hasher = sha512::Sha512::new();
            hasher.update(&commitment);
            hasher.update(&self.public);
            hasher.update(message);
            let challenge = Scalar::reduce_wide(&hasher.finish());

            let s = challenge.mul_add(&self.scalar, &r);

            let mut out = [0u8; 64];
            out[..32].copy_from_slice(&commitment);
            out[32..].copy_from_slice(&s.to_bytes());
            out
        }

        /// Sign, and return the signature as lower-case hex.
        pub fn sign_hex(&self, message: &[u8]) -> [u8; 128] {
            let signature = self.sign(message);
            let mut out = [b'0'; 128];
            out[..64].copy_from_slice(&hex(&{
                let mut half = [0u8; 32];
                half.copy_from_slice(&signature[..32]);
                half
            }));
            out[64..].copy_from_slice(&hex(&{
                let mut half = [0u8; 32];
                half.copy_from_slice(&signature[32..]);
                half
            }));
            out
        }
    }
}

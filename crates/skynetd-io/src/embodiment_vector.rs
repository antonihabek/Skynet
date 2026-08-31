//! One frozen cross-edition vector for an embodiment descriptor (§5, §17).
//!
//! The Hub verifies a vendor's signature over a descriptor, and so does the robot. They are two
//! implementations of one scheme, in two languages, and the only thing that makes publishing a
//! descriptor worth doing is that they reach the same verdict. Three details decide it — the digest
//! domain, the signed message, and the encodings — and every one of them is the sort of detail two
//! teams agree about in prose and disagree about in bytes.
//!
//! So there is a vector, and it lives on both sides. This module produces and pins it here;
//! `lib/__tests__/skynet-embodiment.test.ts` in the Hub repository holds the same numbers and
//! verifies the same signature with Node's crypto. Either side changing the scheme fails on its own
//! side, which is the property a comment saying "these must agree" does not have.
//!
//! It is a *vector* rather than a shared implementation on purpose. §31.2 keeps formats
//! single-source and implementations per-edition, and a signature scheme is a format: what has to
//! be identical is the bytes, and the way to hold two implementations to bytes is to freeze some.
//!
//! # Regenerating
//!
//! ```text
//! cargo test -p skynetd-io --lib -- --ignored embodiment_vector --nocapture
//! ```
//!
//! It prints the vector as JSON. A change to the printed values is a change to the scheme, and it
//! has to land on both sides in the same commit.

/// The seed the vector's signing key is derived from.
///
/// Thirty-two bytes counting from one. A key nobody could mistake for a real one, in a vector that
/// exists to be published in two repositories.
pub const VECTOR_SEED: [u8; 32] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32,
];

/// The descriptor the vector is over: the smallest one that is still a descriptor.
///
/// Written as canonical JSON already, so what is hashed is visible rather than being the output of
/// a canonicaliser the reader has to trust. `"digest":null` is not an oversight — a published
/// descriptor carries the field as null, because the digest is over the document *including* it and
/// so cannot contain itself.
pub const VECTOR_DESCRIPTOR: &str = concat!(
    r#"{"base":"fixed","capabilities":[],"#,
    r#""control":{"command_latency_ms":null,"modes":["position"],"rate_hz_max":null,"rate_hz_min":null},"#,
    r#""digest":null,"dof":1,"end_effectors":[],"envelope":null,"extensions":[],"#,
    r#""joints":[{"control_modes":["position"],"limits":{"effort_max_nm":40,"position_max_rad":3,"#,
    r#""position_min_rad":-3,"velocity_max_rad_s":2},"name":"j0","role":null,"type":"revolute"}],"#,
    r#""robot_slug":"vector-arm","schema_version":1,"sensors":[],"simulator":false,"solvers":[],"#,
    r#""verified":false}"#,
);

/// The descriptor's digest: sha256 over `"skynet.embodiment.v1\n"` and the canonical form.
///
/// Frozen. The Hub recomputes this from [`VECTOR_DESCRIPTOR`] with Node's `crypto` and asserts the
/// same string, so a change to the digest domain, to the canonicaliser, or to the descriptor fails
/// on both sides rather than silently giving two robots two identities for one body.
pub const VECTOR_DIGEST: &str = "29fe4b8d49ec7c32b6936b2e25c3cbd7b7b96c8bb21ad8de5dce3e44f8d2e543";

/// The public key [`VECTOR_SEED`] expands to, as the lower-case hex a trust store holds.
pub const VECTOR_PUBLIC_KEY: &str =
    "79b5562e8fe654f94078b112e8a98ba7901f853ae695bed7e0e3910bad049664";

/// The signature over `"skynet.embodiment.v1\n" + VECTOR_DIGEST`, as lower-case hex.
///
/// Ed25519 is deterministic (RFC 8032 §5.1.6): one key over one message has exactly one signature,
/// which is what lets a signature be *frozen* rather than merely re-verified. Both editions check
/// these bytes, so an edition that signed a different message — the raw digest, say, or the domain
/// without its newline — fails here rather than at a robot that downloaded a descriptor the Hub
/// had already called verified.
pub const VECTOR_SIGNATURE: &str = concat!(
    "cde12b51b7a075e62d3774e300dcbb7299b427d80ec29c4e09cc60f63c2b99d5",
    "e98e036a383c63f72862d81edb074e5b3cbd1f2128bcd8907b7419f4feeae708",
);

#[cfg(test)]
mod tests {
    use super::*;
    use skynet_ed25519::signing::SigningKey;

    /// The digest, the key and the signature, printed for the Hub to freeze.
    ///
    /// `#[ignore]` because it produces an artifact rather than checking one; the assertions below
    /// are what runs every time.
    #[test]
    #[ignore]
    fn embodiment_vector() {
        let document = skynet_json::Json::parse(VECTOR_DESCRIPTOR).expect("the vector parses");
        let digest = crate::embodiment::digest_of(&document).expect("a digest");
        let digest_hex = hex(&digest);
        let key = SigningKey::from_seed(&VECTOR_SEED);
        let signature = key.sign(&crate::trust::embodiment_message(&digest_hex));

        println!("{{");
        println!("  \"descriptor\": {VECTOR_DESCRIPTOR},");
        println!("  \"digest\": \"{digest_hex}\",");
        println!("  \"public_key\": \"{}\",", ascii(&key.public_hex()));
        println!("  \"signature\": \"{}\"", hex64(&signature));
        println!("}}");
    }

    /// The canonical form of the vector is the vector.
    ///
    /// If this fails, the descriptor above was edited without being re-canonicalised, and every
    /// number the Hub has frozen is over different bytes than the ones a reader is looking at.
    #[test]
    fn the_vector_is_already_canonical() {
        let document = skynet_json::Json::parse(VECTOR_DESCRIPTOR).expect("the vector parses");
        assert_eq!(
            document.to_canonical().expect("canonical"),
            VECTOR_DESCRIPTOR
        );
    }

    /// The digest, frozen. The Hub computes this from the descriptor above with Node's crypto.
    #[test]
    fn the_vector_digest_is_what_the_hub_holds() {
        let document = skynet_json::Json::parse(VECTOR_DESCRIPTOR).expect("the vector parses");
        let digest = hex(&crate::embodiment::digest_of(&document).expect("a digest"));
        assert_eq!(
            digest, VECTOR_DIGEST,
            "the embodiment digest scheme changed. Every published descriptor's identity changed \
             with it, and lib/__tests__/skynet-embodiment.test.ts in the Hub holds the old value."
        );
    }

    /// The seed expands to the frozen public key.
    #[test]
    fn the_vector_key_is_what_the_hub_holds() {
        let key = SigningKey::from_seed(&VECTOR_SEED);
        assert_eq!(
            ascii(&key.public_hex()),
            VECTOR_PUBLIC_KEY,
            "the seed no longer expands to the frozen key, so the Hub is verifying against a \
             public key this edition would never produce."
        );
    }

    /// Signing the vector produces the frozen signature, byte for byte.
    ///
    /// Not "a valid signature" — *the* signature. Ed25519 is deterministic, so a re-derived
    /// signature that merely verifies would still pass if this edition signed the bare digest and
    /// the Hub signed the domain-tagged one; comparing to a constant will not.
    #[test]
    fn the_vector_signature_is_what_the_hub_holds() {
        let key = SigningKey::from_seed(&VECTOR_SEED);
        let signature = key.sign(&crate::trust::embodiment_message(VECTOR_DIGEST));
        assert_eq!(
            hex64(&signature),
            VECTOR_SIGNATURE,
            "the signed message changed. lib/__tests__/skynet-embodiment.test.ts in the Hub holds \
             the old signature and will keep accepting descriptors this edition now rejects."
        );
    }

    /// This edition verifies the frozen signature it did not just produce.
    ///
    /// The test above proves the signer agrees with the constant; this one proves the *verifier*
    /// does, which is the side a robot actually runs. They fail separately on purpose.
    #[test]
    fn the_frozen_signature_verifies_here() {
        let key = skynet_ed25519::VerifyingKey::from_hex(VECTOR_PUBLIC_KEY).expect("a key");
        let signature = skynet_ed25519::Signature::from_hex(VECTOR_SIGNATURE).expect("a signature");
        let message = crate::trust::embodiment_message(VECTOR_DIGEST);
        assert!(key.verify(&message, &signature).is_ok());
    }

    /// The signature is over the digest under its domain tag, and not over the digest.
    ///
    /// The one substitution a reader of the scheme is most likely to make, asserted to fail. A
    /// verifier that accepted both would accept a signature made for some other document whose
    /// digest happened to be this hex — which is the whole reason §5 tags the domain.
    #[test]
    fn the_bare_digest_is_not_the_signed_message() {
        let key = skynet_ed25519::VerifyingKey::from_hex(VECTOR_PUBLIC_KEY).expect("a key");
        let signature = skynet_ed25519::Signature::from_hex(VECTOR_SIGNATURE).expect("a signature");
        assert!(key.verify(VECTOR_DIGEST.as_bytes(), &signature).is_err());
    }

    /// The descriptor the digest is over is the one the Hub publishes.
    ///
    /// A descriptor whose `digest` field had been filled in before hashing would produce a
    /// different digest, and the Hub serves the document exactly as published for that reason.
    #[test]
    fn filling_in_the_digest_field_changes_the_digest() {
        let filled = VECTOR_DESCRIPTOR.replace(
            r#""digest":null"#,
            &format!(r#""digest":"{VECTOR_DIGEST}""#),
        );
        let document = skynet_json::Json::parse(&filled).expect("the edited vector parses");
        let digest = hex(&crate::embodiment::digest_of(&document).expect("a digest"));
        assert_ne!(digest, VECTOR_DIGEST);
    }

    fn hex(bytes: &[u8; 32]) -> String {
        String::from_utf8_lossy(&skynet_sha256::hex_of(bytes)).into_owned()
    }

    fn ascii(bytes: &[u8; 64]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    fn hex64(bytes: &[u8; 64]) -> String {
        let mut out = String::with_capacity(128);
        for byte in bytes {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }
}

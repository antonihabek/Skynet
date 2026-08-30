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

#[cfg(test)]
mod tests {
    use super::*;
    use skynet_ed25519::SigningKey;

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
        println!("  \"public_key\": \"{}\",", hex32(&key.verifying_key().to_bytes()));
        println!("  \"signature\": \"{}\"", hex64(&signature.to_bytes()));
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
            digest,
            "70a09e0e4e2fbc4dfa25cabb2fb1e1d1c9e5d6b7a8f9c0d1e2f3a4b5c6d7e8f9",
            "the embodiment digest scheme changed. Every published descriptor's identity changed \\
             with it, and lib/__tests__/skynet-embodiment.test.ts in the Hub holds the old value."
        );
    }

    /// The signature, frozen, and verified by this edition.
    #[test]
    fn the_vector_signature_verifies_here() {
        let document = skynet_json::Json::parse(VECTOR_DESCRIPTOR).expect("the vector parses");
        let digest = hex(&crate::embodiment::digest_of(&document).expect("a digest"));
        let key = SigningKey::from_seed(&VECTOR_SEED);
        let message = crate::trust::embodiment_message(&digest);
        let signature = key.sign(&message);
        assert!(key.verifying_key().verify(&message, &signature).is_ok());
    }

    fn hex(bytes: &[u8; 32]) -> String {
        String::from_utf8_lossy(&skynet_sha256::hex_of(bytes)).into_owned()
    }

    fn hex32(bytes: &[u8; 32]) -> String {
        hex(bytes)
    }

    fn hex64(bytes: &[u8; 64]) -> String {
        let mut out = String::with_capacity(128);
        for byte in bytes {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }
}

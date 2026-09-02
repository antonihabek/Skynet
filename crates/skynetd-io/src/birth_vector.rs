//! One frozen cross-edition vector for a birth certificate (§50, §57).
//!
//! The second of these, and it exists for the same reason as the first plus one. `POST
//! /robots/{slug}/attestations` made the Hub a verifier of vendor signatures over birth
//! certificates, and [`crate::birth::read`] already was one — so there are again two
//! implementations of one scheme in two languages, and again the only thing that makes a
//! certificate worth signing is that they reach the same verdict over the same bytes.
//!
//! The one extra reason is the domain tag. A birth certificate *contains* an embodiment descriptor
//! digest, so a vendor signing a certificate signs a document holding a hex string that is also a
//! descriptor digest. Without separate tags a signature over one could be presented as a signature
//! over the other, and the whole of §50.3 would rest on a substitution nobody would notice. The two
//! vectors together are what prove the tags actually reach the hash and the message: this one and
//! [`crate::embodiment_vector`] use the **same seed and the same key**, and produce different
//! digests and different signatures.
//!
//! # Regenerating
//!
//! ```text
//! cargo test -p skynetd-io --lib -- --ignored birth_vector --nocapture
//! ```
//!
//! It prints the vector as JSON. A change to the printed values is a change to the scheme, and it
//! has to land in `lib/__tests__/skynet-attestations.test.ts` in the Hub repository in the same
//! change.

/// The certificate the vector is over, as canonical JSON already.
///
/// Written as text rather than built from a struct, for [`crate::embodiment_vector`]'s reason: what
/// is hashed should be visible to a reader rather than being the output of a canonicaliser they
/// have to trust.
///
/// The `device_key` is deliberately **not** the vendor key below. They are two different keys doing
/// two different jobs — the vendor's signs the document, the device's is what the document is about
/// — and a vector that used one value for both would pass while an implementation that confused
/// them also passed.
pub const VECTOR_CERTIFICATE: &str = concat!(
    r#"{"calibration_digests":["cal-1"],"#,
    r#""descriptor_digest":"29fe4b8d49ec7c32b6936b2e25c3cbd7b7b96c8bb21ad8de5dce3e44f8d2e543","#,
    r#""device_key":"3f2a1c9e5b8d4706f1e2a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8","#,
    r#""hardware_revision":"rev-C","manufactured":"2026-03-14","model":"G1","serial":"SN-0001"}"#,
);

/// The certificate's digest: sha256 over `"skynet.birth.v1\n"` and the canonical form.
pub const VECTOR_DIGEST: &str = "fded47c48b582408282394fe2021a0d7905f35c9f87dd282c71252b77b1188ac";

/// The signature over `"skynet.birth.v1\n" + VECTOR_DIGEST`, as lower-case hex.
///
/// Ed25519 is deterministic (RFC 8032 §5.1.6), which is what lets a signature be frozen rather than
/// merely re-verified.
pub const VECTOR_SIGNATURE: &str = concat!(
    "e0fac9e06977dbcfce87913f455a069ccab41ea25b547ddad13fd1d50dead4f6",
    "ccb6315acdeaa92d0c568c9c78742a0e6c0959f102612ad2100da827977b2c0e",
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embodiment_vector::{VECTOR_PUBLIC_KEY, VECTOR_SEED};
    use skynet_ed25519::signing::SigningKey;

    fn hex(bytes: &[u8; 32]) -> String {
        String::from_utf8_lossy(&skynet_sha256::hex_of(bytes)).into_owned()
    }

    fn hex64(bytes: &[u8; 64]) -> String {
        let mut out = String::with_capacity(128);
        for byte in bytes {
            out.push_str(&format!("{byte:02x}"));
        }
        out
    }

    /// The digest and the signature, printed for the Hub to freeze.
    #[test]
    #[ignore]
    fn birth_vector() {
        let document = skynet_json::Json::parse(VECTOR_CERTIFICATE).expect("the vector parses");
        let digest = hex(&crate::birth::digest_of(&document).expect("a digest"));
        let key = SigningKey::from_seed(&VECTOR_SEED);
        let signature = key.sign(&crate::trust::birth_message(&digest));

        println!("{{");
        println!("  \"certificate\": {VECTOR_CERTIFICATE},");
        println!("  \"digest\": \"{digest}\",");
        println!("  \"public_key\": \"{VECTOR_PUBLIC_KEY}\",");
        println!("  \"signature\": \"{}\"", hex64(&signature));
        println!("}}");
    }

    /// The canonical form of the vector is the vector.
    #[test]
    fn the_vector_is_already_canonical() {
        let document = skynet_json::Json::parse(VECTOR_CERTIFICATE).expect("the vector parses");
        assert_eq!(
            document.to_canonical().expect("canonical"),
            VECTOR_CERTIFICATE,
            "the certificate was edited without being re-canonicalised, so every frozen number \
             below is over different bytes than the ones a reader is looking at"
        );
    }

    /// The digest, frozen. The Hub computes this with Node's crypto and asserts the same string.
    #[test]
    fn the_vector_digest_is_what_the_hub_holds() {
        let document = skynet_json::Json::parse(VECTOR_CERTIFICATE).expect("the vector parses");
        assert_eq!(
            hex(&crate::birth::digest_of(&document).expect("a digest")),
            VECTOR_DIGEST
        );
    }

    /// The signature, frozen, and it verifies here.
    #[test]
    fn the_vector_signature_verifies_against_the_frozen_key() {
        let key = SigningKey::from_seed(&VECTOR_SEED);
        assert_eq!(
            hex64(&key.sign(&crate::trust::birth_message(VECTOR_DIGEST))),
            VECTOR_SIGNATURE
        );
    }

    /// The point of the whole module: one key, one seed, two domains, two different signatures.
    ///
    /// A birth certificate binds a descriptor digest, so the two documents in these vectors carry
    /// the same hex string. If either tag stopped reaching the hash or the message, a signature
    /// over a descriptor would become presentable as a signature over a certificate — and this is
    /// the assertion that fails first.
    #[test]
    fn the_two_domains_do_not_collide() {
        assert_ne!(
            VECTOR_DIGEST,
            crate::embodiment_vector::VECTOR_DIGEST,
            "two documents, two digests"
        );
        assert_ne!(
            VECTOR_SIGNATURE,
            crate::embodiment_vector::VECTOR_SIGNATURE,
            "one key, two messages, two signatures"
        );

        // And the sharper form of the same property: signing the *embodiment* digest under the
        // birth domain must not produce the embodiment vector's signature.
        let key = SigningKey::from_seed(&VECTOR_SEED);
        let crossed = hex64(&key.sign(&crate::trust::birth_message(
            crate::embodiment_vector::VECTOR_DIGEST,
        )));
        assert_ne!(crossed, crate::embodiment_vector::VECTOR_SIGNATURE);
    }
}

//! The signer, and what it is for.
//!
//! These are integration tests rather than unit tests because `test-signing` reaches this crate
//! through the self dev-dependency in `Cargo.toml`, and an integration test links the crate the way
//! any other consumer does. `cargo build` never resolves dev-dependencies, so nothing here is in a
//! shipped binary — which is the point of the arrangement, and is asserted by the CI check that
//! greps for the feature name outside test configuration.
//!
//! The signer exists so the *verified* path can be tested over bytes a test chose. Without it the
//! only signatures available are RFC 8032's four, and `skynetd`'s trust store would be exercised
//! against a package whose digest happens to be one of them — which is not a test of the store.
//!
//! It also closes the loop on the verifier: RFC 8032's vectors prove this implementation agrees
//! with the document on four inputs, and re-deriving those exact signatures from their seeds proves
//! it agrees on the *whole computation* rather than on the answer.

use skynet_ed25519::signing::SigningKey;
use skynet_ed25519::{Signature, VerifyError, VerifyingKey};

/// RFC 8032 §7.1: `(seed, public key, message, signature)`.
const RFC_8032: [(&str, &str, &str, &str); 4] = [
    (
        "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155\
         5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    ),
    (
        "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da\
         085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
    (
        "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        "af82",
        "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac\
         18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    ),
    (
        "833fe62409237b9d62ec77587520911e9a759cec1d19755b7da901b96dca3d42",
        "ec172b93ad5e563bf4932c70e1245034c35467ef2efd4d64ebf819683467e2bf",
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
         2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        "dc2a4459e7369633a52b1bf277839a00201009a3efbf3ecb69bea2186c26b589\
         09351fc9ac90b3ecfdfbc7c66431e0303dca179c138ac17ad9bef1177331a704",
    ),
];

fn unhex(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII"), 16).expect("hex"))
        .collect()
}

fn seed_of(text: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&unhex(text));
    out
}

#[test]
fn the_rfc_8032_seeds_derive_their_published_public_keys() {
    for (i, (seed, public, _, _)) in RFC_8032.iter().enumerate() {
        let key = SigningKey::from_seed(&seed_of(seed));
        assert_eq!(
            std::str::from_utf8(&key.public_hex()).unwrap(),
            *public,
            "vector {i}"
        );
    }
}

#[test]
fn the_rfc_8032_seeds_reproduce_their_published_signatures_byte_for_byte() {
    // Ed25519 is deterministic — the nonce is a hash of the key's prefix and the message, not a
    // random number — so "the same signature" is a meaningful thing to demand, and demanding it
    // tests every step rather than the final comparison.
    for (i, (seed, _, message, signature)) in RFC_8032.iter().enumerate() {
        let key = SigningKey::from_seed(&seed_of(seed));
        let produced = key.sign_hex(&unhex(message));
        assert_eq!(
            std::str::from_utf8(&produced).unwrap(),
            signature.split_whitespace().collect::<String>(),
            "vector {i}"
        );
    }
}

#[test]
fn what_this_signs_this_verifies() {
    let key = SigningKey::from_seed(&[7u8; 32]);
    let public = key.verifying_key();

    for message in [
        b"".to_vec(),
        b"a".to_vec(),
        b"sha256:0000000000000000000000000000000000000000000000000000000000000000".to_vec(),
        vec![0u8; 1000],
        (0..=255u8).collect::<Vec<u8>>(),
    ] {
        let signature = Signature::from_bytes(&key.sign(&message)).expect("a produced signature");
        assert_eq!(public.verify(&message, &signature), Ok(()), "{message:?}");
    }
}

#[test]
fn a_signature_from_one_key_does_not_verify_under_another() {
    let mine = SigningKey::from_seed(&[1u8; 32]);
    let theirs = SigningKey::from_seed(&[2u8; 32]);
    assert_ne!(mine.public_hex(), theirs.public_hex());

    let message = b"the same bytes, signed by somebody else";
    let signature = Signature::from_bytes(&mine.sign(message)).unwrap();
    assert_eq!(
        theirs.verifying_key().verify(message, &signature),
        Err(VerifyError::Mismatch)
    );
}

#[test]
fn a_derived_public_key_parses_as_a_trust_store_entry_would() {
    // The round trip an operator's key actually takes: derived, rendered as hex, put in a file,
    // read back. If `public_hex` and `from_hex` disagreed, every trusted key would be rejected on
    // the second boot rather than the first.
    for seed in [0u8, 1, 99, 255] {
        let key = SigningKey::from_seed(&[seed; 32]);
        let text = String::from_utf8(key.public_hex().to_vec()).unwrap();
        let parsed = VerifyingKey::from_hex(&text).expect("a derived key is a valid trusted key");
        assert_eq!(parsed, key.verifying_key(), "seed {seed}");
    }
}

#[test]
fn every_signature_it_produces_is_canonically_encoded() {
    // `S < L` by construction, since `S` comes out of arithmetic modulo `L`. Asserted rather than
    // assumed, because the verifier refuses anything else and a signer that could emit one would
    // produce signatures nothing accepts.
    let key = SigningKey::from_seed(&[42u8; 32]);
    for n in 0..64u16 {
        let message = n.to_le_bytes();
        assert!(
            Signature::from_bytes(&key.sign(&message)).is_ok(),
            "message {n}"
        );
    }
}

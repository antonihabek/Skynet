//! What this crate is held to.
//!
//! Three kinds of test, in the order they matter:
//!
//! 1. **The constants are checked against their definitions**, not against themselves. `d` is
//!    verified by `d * 121666 + 121665 == 0`, `sqrt(-1)` by squaring it, and the basepoint by
//!    `5y == 4`. A mistyped digit in any of the three would otherwise produce a coherent
//!    implementation of a curve nobody else is on — one that verifies its own signatures and no
//!    published one, which is the failure that survives a test suite written against itself.
//! 2. **The published vectors**: RFC 8032 §7.1, and FIPS 180-4's SHA-512 examples.
//! 3. **The encodings that divide implementations.** Non-canonical `y`, small-order keys, `S >= L`.
//!    These are the cases where two verifiers disagree about the *same bytes*, and a disagreement
//!    about whether a package is signed is worse than either answer on its own.

use std::vec::Vec;

use crate::edwards::{basepoint, d, sqrt_m1, DecompressError, Point};
use crate::field::Fe;
use crate::scalar::Scalar;
use crate::sha512;
use crate::{KeyError, Signature, SignatureError, VerifyError, VerifyingKey};

/// Decode hex in a test, panicking on anything that is not exactly `N` bytes of it.
fn h<const N: usize>(text: &str) -> [u8; N] {
    crate::unhex::<N>(text).expect("a test vector is well-formed hex of the stated length")
}

// ── SHA-512 ─────────────────────────────────────────────────────────────────────────────────────

#[test]
fn sha512_matches_the_published_examples() {
    assert_eq!(
        sha512::digest(b"abc"),
        h::<64>(
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
        )
    );
    assert_eq!(
        sha512::digest(b""),
        h::<64>(
            "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce\
             47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"
        )
    );
}

#[test]
fn sha512_streams_the_same_answer_it_hashes_in_one_go() {
    // Padding is where a streaming hash goes wrong, and it goes wrong at block boundaries. 111,
    // 112 and 113 bytes bracket the point where the length field no longer fits in the final block
    // and a whole extra one has to be appended.
    for len in [0usize, 1, 55, 111, 112, 113, 127, 128, 129, 255, 256, 1000] {
        let message: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
        let one_shot = sha512::digest(&message);

        for chunk in [1usize, 7, 64, 128, 999] {
            let mut hasher = sha512::Sha512::new();
            for piece in message.chunks(chunk) {
                hasher.update(piece);
            }
            assert_eq!(
                hasher.finish(),
                one_shot,
                "length {len} fed in chunks of {chunk}"
            );
        }
    }
}

// ── The constants ───────────────────────────────────────────────────────────────────────────────

#[test]
fn d_is_minus_121665_over_121666() {
    assert!((d() * Fe::from_u64(121_666) + Fe::from_u64(121_665)).is_zero());
}

#[test]
fn sqrt_minus_one_squares_to_minus_one() {
    assert_eq!(sqrt_m1().square(), -Fe::ONE);
}

#[test]
fn the_basepoint_has_y_of_four_fifths_and_encodes_back_to_itself() {
    let b = basepoint().expect("the basepoint decompresses");
    assert_eq!(b.compress(), crate::edwards::BASEPOINT_BYTES);

    let y = Fe::from_bytes(&crate::edwards::BASEPOINT_BYTES);
    assert_eq!(y * Fe::from_u64(5), Fe::from_u64(4));
}

#[test]
fn the_basepoint_is_not_small_order() {
    assert!(!basepoint().unwrap().is_small_order());
}

// ── The field ───────────────────────────────────────────────────────────────────────────────────

#[test]
fn inversion_undoes_multiplication() {
    for n in [1u64, 2, 3, 19, 121_666, u64::from(u32::MAX)] {
        let x = Fe::from_u64(n);
        assert_eq!(x * x.invert(), Fe::ONE, "n = {n}");
    }
}

#[test]
fn subtraction_undoes_addition_at_the_wrap() {
    // p - 1 is where a representation that forgot to reduce shows itself.
    let p_minus_1 = Fe::ZERO - Fe::ONE;
    assert!((p_minus_1 + Fe::ONE).is_zero());
    assert_eq!(
        p_minus_1.to_bytes(),
        h::<32>("ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f")
    );
}

#[test]
fn the_nineteen_long_spellings_are_refused() {
    // p, p+1 … p+18 all decode to 0 … 18. Every one is a second name for an element that has one.
    let p = h::<32>("edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f");
    for extra in 0u8..19 {
        let mut bytes = p;
        bytes[0] += extra;
        assert!(
            !Fe::is_canonical_encoding(&bytes),
            "p + {extra} should not be canonical"
        );
        assert_eq!(
            Fe::from_bytes(&bytes),
            Fe::from_u64(u64::from(extra)),
            "p + {extra} decodes to {extra}"
        );
    }

    // p + 19 is 2^255, whose low 255 bits are zero — a canonical encoding of zero with the sign bit
    // set. That is a point-encoding question rather than a field one, and it is refused elsewhere:
    // see `x_zero_with_the_sign_bit_set_is_refused`.
    let two_255 = h::<32>("0000000000000000000000000000000000000000000000000000000000000080");
    assert!(Fe::is_canonical_encoding(&two_255));
}

#[test]
fn encoding_round_trips() {
    for n in [0u64, 1, 2, 19, 1 << 40, (1 << 51) - 1] {
        let x = Fe::from_u64(n);
        assert_eq!(Fe::from_bytes(&x.to_bytes()), x, "n = {n}");
    }
}

// ── Scalars ─────────────────────────────────────────────────────────────────────────────────────

const L_HEX: &str = "edd3f55c1a631258d69cf7a2def9de1400000000000000000000000000000010";

fn widen(narrow: &[u8; 32]) -> [u8; 64] {
    let mut wide = [0u8; 64];
    wide[..32].copy_from_slice(narrow);
    wide
}

#[test]
fn l_reduces_to_zero_and_l_minus_one_reduces_to_itself() {
    assert_eq!(Scalar::reduce_wide(&widen(&h::<32>(L_HEX))), Scalar::ZERO);

    let mut one_less = h::<32>(L_HEX);
    one_less[0] -= 1;
    assert_eq!(
        Scalar::reduce_wide(&widen(&one_less)),
        Scalar::from_canonical_bytes(&one_less).unwrap()
    );
}

#[test]
fn small_values_reduce_to_themselves() {
    for n in [0u8, 1, 2, 255] {
        let mut narrow = [0u8; 32];
        narrow[0] = n;
        assert_eq!(
            Scalar::reduce_wide(&widen(&narrow)),
            Scalar::from_canonical_bytes(&narrow).unwrap(),
            "n = {n}"
        );
    }
}

#[test]
fn a_scalar_at_or_above_l_is_refused_rather_than_reduced() {
    assert!(Scalar::from_canonical_bytes(&h::<32>(L_HEX)).is_none());
    assert!(Scalar::from_canonical_bytes(&[0xffu8; 32]).is_none());

    let mut one_less = h::<32>(L_HEX);
    one_less[0] -= 1;
    assert!(Scalar::from_canonical_bytes(&one_less).is_some());
}

#[test]
fn the_bits_above_a_reduced_scalar_are_clear() {
    let mut one_less = h::<32>(L_HEX);
    one_less[0] -= 1;
    let s = Scalar::from_canonical_bytes(&one_less).unwrap();
    assert!(s.bit(252), "L - 1 has bit 252 set");
    for i in Scalar::BITS..300 {
        assert!(!s.bit(i), "bit {i} of L-1 should be clear");
    }
}

// ── Curve arithmetic ────────────────────────────────────────────────────────────────────────────

fn scalar_of(n: u64) -> Scalar {
    let mut bytes = [0u8; 32];
    bytes[..8].copy_from_slice(&n.to_le_bytes());
    Scalar::from_canonical_bytes(&bytes).unwrap()
}

#[test]
fn doubling_agrees_with_adding_a_point_to_itself() {
    let mut p = basepoint().unwrap();
    for _ in 0..8 {
        assert_eq!(p.double(), p + p);
        p = p.double();
    }
}

#[test]
fn the_identity_is_neutral() {
    let b = basepoint().unwrap();
    let id = Point::identity();
    assert_eq!(b + id, b);
    assert_eq!(id + b, b);
    assert_eq!(id.double(), id);
    assert!(id.is_identity());
    assert!(id.is_small_order());
}

#[test]
fn scalar_multiplication_is_additive() {
    let b = basepoint().unwrap();
    for (a, c) in [(1u64, 1u64), (2, 3), (7, 11), (1 << 20, 12_345)] {
        let left = b.scalar_mul(&scalar_of(a)) + b.scalar_mul(&scalar_of(c));
        assert_eq!(left, b.scalar_mul(&scalar_of(a + c)), "{a} + {c}");
    }
}

#[test]
fn multiplying_by_zero_gives_the_identity() {
    assert!(basepoint().unwrap().scalar_mul(&Scalar::ZERO).is_identity());
}

#[test]
fn compression_round_trips() {
    let b = basepoint().unwrap();
    let mut p = b;
    for i in 1..16u64 {
        let back = Point::decompress(&p.compress()).expect("a curve point decompresses");
        assert_eq!(back, p, "step {i}");
        p = p + b;
    }
}

#[test]
fn x_zero_with_the_sign_bit_set_is_refused() {
    // y = 1 is the identity, whose x is zero. With the sign bit clear it is a point; with the bit
    // set it claims a negative zero, which is the same value written twice.
    let mut identity = [0u8; 32];
    identity[0] = 1;
    assert!(Point::decompress(&identity).is_some());

    identity[31] |= 0x80;
    assert_eq!(
        Point::decompress_detail(&identity),
        Err(DecompressError::NotCanonical)
    );
}

#[test]
fn a_y_with_no_square_root_is_refused() {
    // y = 2 is not the y-coordinate of any point on this curve.
    let mut bytes = [0u8; 32];
    bytes[0] = 2;
    assert_eq!(
        Point::decompress_detail(&bytes),
        Err(DecompressError::NotOnCurve)
    );
}

// ── The published signature vectors ─────────────────────────────────────────────────────────────

/// RFC 8032 §7.1, in the order the document gives them: `(public key, message, signature)`.
const RFC_8032: [(&str, &str, &str); 4] = [
    (
        "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
        "",
        "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e06522490155\
         5fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b",
    ),
    (
        "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
        "72",
        "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da\
         085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00",
    ),
    (
        "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
        "af82",
        "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac\
         18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a",
    ),
    // The "SHA(abc)" vector: the message is the SHA-512 of `abc`, which is also this crate's
    // SHA-512 test vector. A wrong SHA-512 fails both, in two different-looking ways.
    (
        "ec172b93ad5e563bf4932c70e1245034c35467ef2efd4d64ebf819683467e2bf",
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
         2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f",
        "dc2a4459e7369633a52b1bf277839a00201009a3efbf3ecb69bea2186c26b589\
         09351fc9ac90b3ecfdfbc7c66431e0303dca179c138ac17ad9bef1177331a704",
    ),
];

fn message_of(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    digits
        .chunks(2)
        .map(|pair| {
            let text = core::str::from_utf8(pair).expect("hex digits are ASCII");
            u8::from_str_radix(text, 16).expect("a test message is hex")
        })
        .collect()
}

#[test]
fn the_rfc_8032_vectors_verify() {
    for (i, (key, message, signature)) in RFC_8032.iter().enumerate() {
        let key = VerifyingKey::from_hex(key).unwrap_or_else(|e| panic!("vector {i}: {e:?}"));
        let sig = Signature::from_hex(signature).unwrap_or_else(|e| panic!("vector {i}: {e:?}"));
        assert_eq!(
            key.verify(&message_of(message), &sig),
            Ok(()),
            "RFC 8032 vector {i}"
        );
    }
}

#[test]
fn one_flipped_bit_anywhere_is_rejected() {
    let (key, message, signature) = RFC_8032[2];
    let key = VerifyingKey::from_hex(key).unwrap();
    let message = message_of(message);
    let good: [u8; 64] = h::<64>(signature);
    let sig = Signature::from_bytes(&good).unwrap();

    for bit in 0..message.len() * 8 {
        let mut broken = message.clone();
        broken[bit / 8] ^= 1 << (bit % 8);
        assert_eq!(
            key.verify(&broken, &sig),
            Err(VerifyError::Mismatch),
            "message bit {bit}"
        );
    }

    // In the signature. Some flips make it malformed rather than wrong, and both are refusals —
    // what must never happen is an `Ok`.
    for bit in 0..512 {
        let mut broken = good;
        broken[bit / 8] ^= 1 << (bit % 8);
        let verdict = match Signature::from_bytes(&broken) {
            Err(_) => continue,
            Ok(sig) => key.verify(&message, &sig),
        };
        assert!(verdict.is_err(), "signature bit {bit} verified");
    }
}

#[test]
fn a_signature_does_not_verify_under_another_key() {
    let (_, message, signature) = RFC_8032[2];
    let other = VerifyingKey::from_hex(RFC_8032[1].0).unwrap();
    let sig = Signature::from_hex(signature).unwrap();
    assert_eq!(
        other.verify(&message_of(message), &sig),
        Err(VerifyError::Mismatch)
    );
}

#[test]
fn the_same_signature_does_not_verify_over_a_longer_message() {
    // The message goes into SHA-512 whole, so a suffix changes the challenge. This is what makes a
    // length field unnecessary, and it is worth an assertion rather than an argument.
    let (key, _, signature) = RFC_8032[0];
    let key = VerifyingKey::from_hex(key).unwrap();
    let sig = Signature::from_hex(signature).unwrap();
    assert_eq!(key.verify(b"", &sig), Ok(()));
    assert_eq!(key.verify(b"\x00", &sig), Err(VerifyError::Mismatch));
}

// ── The encodings that divide implementations ───────────────────────────────────────────────────

#[test]
fn s_plus_l_is_refused_rather_than_reduced() {
    let (key, message, signature) = RFC_8032[2];
    let key = VerifyingKey::from_hex(key).unwrap();
    let good = h::<64>(signature);

    // S + L, added by hand into the second half. It satisfies the same equation, and it is a
    // different 64-byte string — which is the whole reason it is refused.
    let l = h::<32>(L_HEX);
    let mut malleable = good;
    let mut carry = 0u16;
    for i in 0..32 {
        let sum = u16::from(good[32 + i]) + u16::from(l[i]) + carry;
        malleable[32 + i] = sum as u8;
        carry = sum >> 8;
    }
    assert_eq!(carry, 0, "S + L fits in 32 bytes for this vector");
    assert_ne!(malleable, good);

    assert_eq!(
        Signature::from_bytes(&malleable),
        Err(SignatureError::ScalarNotCanonical)
    );
    // The original still verifies, so the refusal is about the encoding rather than the maths.
    assert_eq!(
        key.verify(&message_of(message), &Signature::from_bytes(&good).unwrap()),
        Ok(())
    );
}

#[test]
fn the_eight_small_order_keys_are_refused() {
    // The eight points of order dividing 8, canonically encoded. Every one is a public key a
    // stranger can produce with no private key at all.
    let small = [
        "0100000000000000000000000000000000000000000000000000000000000000",
        "ecffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
        "0000000000000000000000000000000000000000000000000000000000000080",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc05",
        "26e8958fc2b227b045c3f489f2ef98f0d5dfac05d3c63339b13802886d53fc85",
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac037a",
        "c7176a703d4dd84fba3c0b760d10670f2a2053fa2c39ccc64ec7fd7792ac03fa",
    ];
    for encoded in small {
        let bytes = h::<32>(encoded);
        let point = Point::decompress(&bytes)
            .unwrap_or_else(|| panic!("{encoded} should decompress to a point"));
        assert!(point.is_small_order(), "{encoded} should be small order");
        assert_eq!(
            VerifyingKey::from_bytes(&bytes),
            Err(KeyError::SmallOrder),
            "{encoded}"
        );
    }
}

#[test]
fn a_non_canonical_key_is_refused_before_anything_else() {
    // p + 1 decodes to y = 1 — the identity, spelled the long way. Two refusals apply, and the
    // encoding one comes first because it is the one an operator can act on.
    let bytes = h::<32>("eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f");
    assert_eq!(
        VerifyingKey::from_bytes(&bytes),
        Err(KeyError::NotCanonical)
    );
}

#[test]
fn a_key_that_is_not_a_point_is_refused() {
    let mut bytes = [0u8; 32];
    bytes[0] = 2;
    assert_eq!(VerifyingKey::from_bytes(&bytes), Err(KeyError::NotOnCurve));
}

#[test]
fn a_malformed_commitment_is_a_different_finding_from_a_wrong_one() {
    let (key, message, signature) = RFC_8032[2];
    let key = VerifyingKey::from_hex(key).unwrap();
    let mut bytes = h::<64>(signature);
    // y = 2 in R: well-formed as 64 bytes, and not a point.
    bytes[..32].copy_from_slice(&[0u8; 32]);
    bytes[0] = 2;
    let sig = Signature::from_bytes(&bytes).unwrap();
    assert_eq!(
        key.verify(&message_of(message), &sig),
        Err(VerifyError::CommitmentNotOnCurve)
    );

    // And a non-canonical R is the other malformed finding.
    let mut bytes = h::<64>(signature);
    bytes[..32].copy_from_slice(&h::<32>(
        "eeffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f",
    ));
    let sig = Signature::from_bytes(&bytes).unwrap();
    assert_eq!(
        key.verify(&message_of(message), &sig),
        Err(VerifyError::CommitmentNotCanonical)
    );
}

// ── Parsing ─────────────────────────────────────────────────────────────────────────────────────

#[test]
fn hex_parsing_ignores_whitespace_and_refuses_everything_else() {
    let wrapped = "d75a980182b10ab7d54bfed3c964073a\n0ee172f3daa62325af021a68f707511a\n";
    assert!(VerifyingKey::from_hex(wrapped).is_ok());

    assert_eq!(VerifyingKey::from_hex(""), Err(KeyError::NotHex));
    assert_eq!(VerifyingKey::from_hex("zz"), Err(KeyError::NotHex));
    assert_eq!(
        VerifyingKey::from_hex(&"a".repeat(63)),
        Err(KeyError::NotHex)
    );
    assert_eq!(
        VerifyingKey::from_hex(&"a".repeat(65)),
        Err(KeyError::NotHex)
    );
    assert_eq!(
        Signature::from_hex(&"a".repeat(127)),
        Err(SignatureError::NotHex)
    );
}

#[test]
fn a_key_renders_as_the_hex_it_was_given() {
    let text = RFC_8032[0].0;
    let key = VerifyingKey::from_hex(text).unwrap();
    assert_eq!(core::str::from_utf8(&key.to_hex()).unwrap(), text);
    assert_eq!(key.as_bytes(), &h::<32>(text));
}

#[test]
fn upper_case_hex_parses_to_the_same_key() {
    let lower = VerifyingKey::from_hex(RFC_8032[0].0).unwrap();
    let upper = VerifyingKey::from_hex(&RFC_8032[0].0.to_uppercase()).unwrap();
    assert_eq!(lower, upper);
}

#[test]
fn every_refusal_explains_itself() {
    // A finding nobody can act on is a finding that gets ignored, so each reason names a remedy or
    // an owner. Asserted rather than assumed, because these strings are what an operator reads.
    for reason in [
        KeyError::NotHex.reason(),
        KeyError::NotCanonical.reason(),
        KeyError::NotOnCurve.reason(),
        KeyError::SmallOrder.reason(),
        SignatureError::NotHex.reason(),
        SignatureError::ScalarNotCanonical.reason(),
        VerifyError::CommitmentNotCanonical.reason(),
        VerifyError::CommitmentNotOnCurve.reason(),
        VerifyError::Mismatch.reason(),
    ] {
        assert!(reason.len() > 20, "{reason:?} is too short to be useful");
        assert!(
            !reason.ends_with('.'),
            "{reason:?} is a clause, not a sentence — callers punctuate"
        );
    }
}

//! Arithmetic in GF(2^255 - 19).
//!
//! Five limbs of 51 bits in a `[u64; 5]`, with products accumulated in `u128`. That is the standard
//! 64-bit representation and it is chosen here for one reason above the others: **the overflow
//! argument is a sentence**. Every product of two weakly-reduced limbs is at most 2^108, five of
//! them sum below 2^115, and a `u128` holds that with thirteen bits to spare — so the bound can be
//! stated rather than proved, and `overflow-checks = true` in the release profile (see the
//! workspace `Cargo.toml`) would catch it if the sentence were wrong.
//!
//! The operators are the `core::ops` ones. `(y - x) * (y2 - x2)` is the curve formula as it is
//! written down; `y.sub(&x).mul(&y2.sub(&x2))` is the same thing with the shape removed, and the
//! shape is what a reader checks against the reference.
//!
//! # This is not constant-time, and that is a decision rather than an omission
//!
//! Everything this crate computes during verification is public. A verifier is handed a public key,
//! a message, and a signature; all three are known to whoever produced them, and two of the three
//! are chosen by an attacker. There is no secret to leak through a branch or a cache line, so
//! constant-time arithmetic would cost speed and complexity for a property with nothing behind it.
//!
//! The moment that stops being true is signing, which multiplies a secret scalar. That is why
//! signing is behind the `test-signing` feature and is in nothing that ships (§19).

use core::ops::{Add, Mul, Neg, Sub};

/// A field element: five limbs, radix 2^51, little-endian.
///
/// "Weakly reduced" throughout — limbs stay below 2^52 rather than the element being canonical. A
/// canonical form exists and is what [`Fe::to_bytes`] produces; equality goes through it, because
/// two weakly-reduced representations of the same element differ limb by limb.
#[derive(Debug, Clone, Copy)]
pub struct Fe(pub [u64; 5]);

const LOW_51: u64 = (1u64 << 51) - 1;

/// 16 * (2^51 - 19) — limb 0 of `16p`.
const P16_0: u64 = 36_028_797_018_963_664;
/// 16 * (2^51 - 1) — limbs 1 to 4 of `16p`.
const P16_N: u64 = 36_028_797_018_963_952;

impl Fe {
    /// Zero.
    pub const ZERO: Fe = Fe([0, 0, 0, 0, 0]);
    /// One.
    pub const ONE: Fe = Fe([1, 0, 0, 0, 0]);

    /// A small integer, for the handful of places one appears.
    pub const fn from_u64(n: u64) -> Fe {
        // Valid because every caller passes something well below 2^51; a larger value would be a
        // limb out of range rather than a wrong answer, and `debug_assert` says so in tests.
        debug_assert!(n < (1 << 51));
        Fe([n, 0, 0, 0, 0])
    }

    /// Propagate carries so every limb is below 2^51, plus at most a few bits in limb 0.
    ///
    /// Accepts limbs up to 2^64: the carry out of a 51-bit shift is then at most 2^13, and
    /// `carry * 19` cannot overflow limb 0. That bound is what lets subtraction add `16p` first and
    /// reduce afterwards.
    fn reduce(mut limbs: [u64; 5]) -> Fe {
        let c0 = limbs[0] >> 51;
        let c1 = limbs[1] >> 51;
        let c2 = limbs[2] >> 51;
        let c3 = limbs[3] >> 51;
        let c4 = limbs[4] >> 51;

        limbs[0] &= LOW_51;
        limbs[1] &= LOW_51;
        limbs[2] &= LOW_51;
        limbs[3] &= LOW_51;
        limbs[4] &= LOW_51;

        // 2^255 == 19 (mod p), so what falls off the top comes back in at the bottom, times 19.
        limbs[0] += c4 * 19;
        limbs[1] += c0;
        limbs[2] += c1;
        limbs[3] += c2;
        limbs[4] += c3;

        Fe(limbs)
    }

    /// Squaring.
    ///
    /// The same multiplication with both arguments equal. A dedicated squaring routine saves about
    /// a third of the partial products and is what a performance-shaped implementation would write;
    /// this is a second code path over the same modular reduction, and a verifier that finishes in
    /// under a millisecond either way does not need one. Speed here buys nothing an operator can
    /// perceive, and the second path would have to be argued correct separately.
    pub fn square(self) -> Fe {
        self * self
    }

    /// `self` squared `k` times.
    pub fn pow2k(self, k: u32) -> Fe {
        let mut out = self;
        for _ in 0..k {
            out = out.square();
        }
        out
    }

    /// Decode 32 little-endian bytes.
    ///
    /// **Bit 255 is discarded.** In Ed25519 that bit is the sign of `x`, not part of `y`, and every
    /// caller strips it deliberately. A value at or above `p` is *not* rejected here — canonicality
    /// is a separate question with a separate answer, because the encoding that must be refused is
    /// a valid field element reached by a spelling nobody may use. See [`Fe::is_canonical_encoding`].
    pub fn from_bytes(bytes: &[u8; 32]) -> Fe {
        let load8 = |at: usize| -> u64 {
            let mut word = [0u8; 8];
            word.copy_from_slice(&bytes[at..at + 8]);
            u64::from_le_bytes(word)
        };
        Fe([
            load8(0) & LOW_51,
            (load8(6) >> 3) & LOW_51,
            (load8(12) >> 6) & LOW_51,
            (load8(19) >> 1) & LOW_51,
            (load8(24) >> 12) & LOW_51,
        ])
    }

    /// The canonical 32-byte little-endian encoding, fully reduced and below `p`.
    pub fn to_bytes(self) -> [u8; 32] {
        let mut limbs = Fe::reduce(self.0).0;

        // After that reduction the value is below `2p`, so one conditional subtraction finishes it.
        // `h >= p` exactly when `h + 19` carries out of bit 255, and the chain below is that carry.
        let mut q = (limbs[0] + 19) >> 51;
        q = (limbs[1] + q) >> 51;
        q = (limbs[2] + q) >> 51;
        q = (limbs[3] + q) >> 51;
        q = (limbs[4] + q) >> 51;

        limbs[0] += 19 * q;

        limbs[1] += limbs[0] >> 51;
        limbs[0] &= LOW_51;
        limbs[2] += limbs[1] >> 51;
        limbs[1] &= LOW_51;
        limbs[3] += limbs[2] >> 51;
        limbs[2] &= LOW_51;
        limbs[4] += limbs[3] >> 51;
        limbs[3] &= LOW_51;
        limbs[4] &= LOW_51;

        let mut out = [0u8; 32];
        out[0..8].copy_from_slice(&(limbs[0] | (limbs[1] << 51)).to_le_bytes());
        out[8..16].copy_from_slice(&((limbs[1] >> 13) | (limbs[2] << 38)).to_le_bytes());
        out[16..24].copy_from_slice(&((limbs[2] >> 26) | (limbs[3] << 25)).to_le_bytes());
        out[24..32].copy_from_slice(&((limbs[3] >> 39) | (limbs[4] << 12)).to_le_bytes());
        out
    }

    /// Whether these 32 bytes are the canonical encoding of the element they decode to.
    ///
    /// `y` in a compressed point occupies 255 bits, and 2^255 - 19 of the 2^255 patterns name a
    /// field element. The other nineteen also decode — to 0 through 18, spelled the long way — and
    /// accepting them would give nineteen public keys two encodings apiece. Two encodings for one
    /// key is two identities for one signer, which is a fingerprint that fails to identify.
    ///
    /// Bit 255 is ignored, because it is the sign bit and not part of `y`.
    pub fn is_canonical_encoding(bytes: &[u8; 32]) -> bool {
        let mut without_sign = *bytes;
        without_sign[31] &= 0x7f;
        Fe::from_bytes(bytes).to_bytes() == without_sign
    }

    /// Whether this is zero.
    pub fn is_zero(self) -> bool {
        self.to_bytes() == [0u8; 32]
    }

    /// The sign bit RFC 8032 uses: the low bit of the canonical encoding.
    pub fn is_negative(self) -> bool {
        self.to_bytes()[0] & 1 == 1
    }

    /// `self` if `flag` is false, `-self` if it is true.
    pub fn negate_if(self, flag: bool) -> Fe {
        if flag {
            -self
        } else {
            self
        }
    }

    /// `self` raised to 2^250 - 1, and the intermediate `self^11` that both callers want.
    ///
    /// The addition chain is ref10's, unchanged: it reaches 2^250 - 1 in 265 squarings and eleven
    /// multiplications, and inversion and the square root are a handful of steps past it. The
    /// comment after each line is the exponent reached, which is how the chain is checked by eye.
    fn pow22501(self) -> (Fe, Fe) {
        let t0 = self.square(); // 2
        let t1 = t0.square().square(); // 8
        let t2 = self * t1; // 9
        let t3 = t0 * t2; // 11
        let t4 = t3.square(); // 22
        let t5 = t2 * t4; // 31 = 2^5 - 1
        let t6 = t5.pow2k(5); // 2^10 - 2^5
        let t7 = t6 * t5; // 2^10 - 1
        let t8 = t7.pow2k(10); // 2^20 - 2^10
        let t9 = t8 * t7; // 2^20 - 1
        let t10 = t9.pow2k(20); // 2^40 - 2^20
        let t11 = t10 * t9; // 2^40 - 1
        let t12 = t11.pow2k(10); // 2^50 - 2^10
        let t13 = t12 * t7; // 2^50 - 1
        let t14 = t13.pow2k(50); // 2^100 - 2^50
        let t15 = t14 * t13; // 2^100 - 1
        let t16 = t15.pow2k(100); // 2^200 - 2^100
        let t17 = t16 * t15; // 2^200 - 1
        let t18 = t17.pow2k(50); // 2^250 - 2^50
        let t19 = t18 * t13; // 2^250 - 1
        (t19, t3)
    }

    /// The multiplicative inverse, by Fermat: `self^(p-2)`, and `p - 2` is `2^255 - 21`.
    ///
    /// Zero inverts to zero, which is wrong arithmetically and is the useful answer here. The two
    /// places that invert are the final division by `Z` — never zero on a curve point — and the
    /// square-root ratio, whose result is checked against the equation it is supposed to satisfy,
    /// so a wrong answer is caught by the check rather than by the inverse.
    pub fn invert(self) -> Fe {
        let (t19, t3) = self.pow22501(); // 2^250 - 1, and 11
        let t20 = t19.pow2k(5); // 2^255 - 2^5
        t20 * t3 // 2^255 - 21
    }

    /// `self^((p-5)/8)`, which is `self^(2^252 - 3)` — the exponent in RFC 8032's square root.
    pub fn pow_p58(self) -> Fe {
        let (t19, _) = self.pow22501(); // 2^250 - 1
        let t20 = t19.pow2k(2); // 2^252 - 4
        t20 * self // 2^252 - 3
    }
}

/// Equality is equality of the canonical encoding.
///
/// It has to be: the representation is redundant, and two `Fe` values holding the same element can
/// have five different limbs each. A derived `PartialEq` would compare limbs and answer `false` for
/// equal elements — which in a verifier means rejecting valid signatures, sometimes.
impl PartialEq for Fe {
    fn eq(&self, other: &Fe) -> bool {
        self.to_bytes() == other.to_bytes()
    }
}

impl Eq for Fe {}

impl Add for Fe {
    type Output = Fe;

    /// Reduced on the way out, which the representation does not require: limbwise addition of two
    /// weakly-reduced elements stays below 2^53 and multiplication tolerates that. It is done
    /// anyway because the alternative is a bound tracked by hand through every curve formula, and a
    /// single unreduced value reaching a multiplication that assumed otherwise is a wrong answer
    /// with no symptom. One shift and one mask per limb deletes that class of bug.
    fn add(self, rhs: Fe) -> Fe {
        Fe::reduce([
            self.0[0] + rhs.0[0],
            self.0[1] + rhs.0[1],
            self.0[2] + rhs.0[2],
            self.0[3] + rhs.0[3],
            self.0[4] + rhs.0[4],
        ])
    }
}

impl Sub for Fe {
    type Output = Fe;

    /// `self + 16p - rhs`, so no limb can underflow: every limb of `16p` is above 2^55 and every
    /// limb of `rhs` is below 2^52. Adding a multiple of `p` changes nothing modulo `p`.
    fn sub(self, rhs: Fe) -> Fe {
        Fe::reduce([
            (self.0[0] + P16_0) - rhs.0[0],
            (self.0[1] + P16_N) - rhs.0[1],
            (self.0[2] + P16_N) - rhs.0[2],
            (self.0[3] + P16_N) - rhs.0[3],
            (self.0[4] + P16_N) - rhs.0[4],
        ])
    }
}

impl Neg for Fe {
    type Output = Fe;

    fn neg(self) -> Fe {
        Fe::ZERO - self
    }
}

impl Mul for Fe {
    type Output = Fe;

    /// Schoolbook, with the terms that would land above limb 4 folded back in multiplied by 19 —
    /// which is the whole of what "modulo 2^255 - 19" costs in this representation.
    fn mul(self, rhs: Fe) -> Fe {
        let a = self.0;
        let b = rhs.0;

        let b1_19 = u128::from(b[1]) * 19;
        let b2_19 = u128::from(b[2]) * 19;
        let b3_19 = u128::from(b[3]) * 19;
        let b4_19 = u128::from(b[4]) * 19;

        let m = |x: u64, y: u128| u128::from(x) * y;

        let c0 = m(a[0], u128::from(b[0]))
            + m(a[4], b1_19)
            + m(a[3], b2_19)
            + m(a[2], b3_19)
            + m(a[1], b4_19);
        let mut c1 = m(a[1], u128::from(b[0]))
            + m(a[0], u128::from(b[1]))
            + m(a[4], b2_19)
            + m(a[3], b3_19)
            + m(a[2], b4_19);
        let mut c2 = m(a[2], u128::from(b[0]))
            + m(a[1], u128::from(b[1]))
            + m(a[0], u128::from(b[2]))
            + m(a[4], b3_19)
            + m(a[3], b4_19);
        let mut c3 = m(a[3], u128::from(b[0]))
            + m(a[2], u128::from(b[1]))
            + m(a[1], u128::from(b[2]))
            + m(a[0], u128::from(b[3]))
            + m(a[4], b4_19);
        let mut c4 = m(a[4], u128::from(b[0]))
            + m(a[3], u128::from(b[1]))
            + m(a[2], u128::from(b[2]))
            + m(a[1], u128::from(b[3]))
            + m(a[0], u128::from(b[4]));

        let mut out = [0u64; 5];
        c1 += c0 >> 51;
        out[0] = (c0 as u64) & LOW_51;
        c2 += c1 >> 51;
        out[1] = (c1 as u64) & LOW_51;
        c3 += c2 >> 51;
        out[2] = (c2 as u64) & LOW_51;
        c4 += c3 >> 51;
        out[3] = (c3 as u64) & LOW_51;
        let carry = (c4 >> 51) as u64;
        out[4] = (c4 as u64) & LOW_51;

        out[0] += carry * 19;
        out[1] += out[0] >> 51;
        out[0] &= LOW_51;

        Fe(out)
    }
}

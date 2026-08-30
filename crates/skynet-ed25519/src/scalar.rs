//! Arithmetic modulo `L`, the order of the prime-order subgroup.
//!
//! `L = 2^252 + 27742317777372353535851937790883648493`, and it is not the field modulus. Scalars
//! live modulo `L`; coordinates live modulo `p = 2^255 - 19`. The two are close enough in size that
//! confusing them produces something that almost works, which is why they are separate types in
//! separate modules with no conversion between them.
//!
//! # Long division, one bit at a time
//!
//! Reducing a 512-bit hash modulo `L` is usually done with a hand-derived chain of shifts and
//! multiply-subtracts — ref10's `sc_reduce` is about a hundred lines of it, and it is fast and
//! nearly unreviewable. This does it the way it is defined instead: shift a bit in, subtract `L` if
//! the accumulator reached it, repeat 512 times. Every line is the schoolbook step, the invariant
//! (`acc < L`) holds after each one, and the whole routine fits on a screen.
//!
//! It costs roughly 512 four-limb compare-and-subtracts, which is a few microseconds, against a
//! verification that already spends milliseconds in the curve. Buying back that time with code
//! nobody in this project could check would be a bad trade twice over.

/// `L`, little-endian in 64-bit limbs.
///
/// `2^252` is limb 3's `0x1000000000000000`; the rest is the additive term, which is 128 bits and
/// therefore occupies limbs 0 and 1 exactly, leaving limb 2 zero.
const L: [u64; 4] = [
    0x5812631a5cf5d3ed,
    0x14def9dea2f79cd6,
    0x0000000000000000,
    0x1000000000000000,
];

/// A scalar, reduced: strictly below `L`.
///
/// The invariant is in the constructors, and there is no way to build one that breaks it — which is
/// what makes [`Scalar::bit`] safe to stop at bit 252.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scalar([u64; 4]);

impl Scalar {
    /// Zero.
    pub const ZERO: Scalar = Scalar([0, 0, 0, 0]);

    /// Reduce a 512-bit little-endian integer modulo `L`.
    ///
    /// This is what turns `SHA-512(R || A || M)` into the challenge scalar `k`.
    pub fn reduce_wide(bytes: &[u8; 64]) -> Scalar {
        let mut acc = [0u64; 4];
        for i in (0..512).rev() {
            let bit = ((bytes[i / 8] >> (i % 8)) & 1) as u64;
            shl1_add(&mut acc, bit);
            if at_least_l(&acc) {
                sub_l(&mut acc);
            }
        }
        Scalar(acc)
    }

    /// Decode 32 little-endian bytes, refusing anything at or above `L`.
    ///
    /// Refusing is the whole point. `S` in a signature is a scalar, and `S` and `S + L` encode the
    /// same scalar and both satisfy the verification equation — so an implementation that reduces
    /// instead of refusing accepts a second, different 64-byte signature for a message that was
    /// signed once. Anything that identifies a signature by its bytes (a cache, a replay window, a
    /// ledger, an audit log) then sees two, and §13's record of *which* signature was accepted stops
    /// being a fact about the signer.
    pub fn from_canonical_bytes(bytes: &[u8; 32]) -> Option<Scalar> {
        let mut limbs = [0u64; 4];
        for (i, limb) in limbs.iter_mut().enumerate() {
            let mut word = [0u8; 8];
            word.copy_from_slice(&bytes[i * 8..i * 8 + 8]);
            *limb = u64::from_le_bytes(word);
        }
        if at_least_l(&limbs) {
            return None;
        }
        Some(Scalar(limbs))
    }

    /// Bit `i`, counting from zero. Bits at or above 253 are always zero, because `L < 2^253`.
    pub fn bit(&self, i: usize) -> bool {
        if i >= 256 {
            return false;
        }
        (self.0[i / 64] >> (i % 64)) & 1 == 1
    }

    /// The highest bit position that could be set, plus one.
    ///
    /// Fixed rather than measured: a scalar multiplication that started at the top *set* bit would
    /// run for a number of iterations that depends on the scalar. Nothing here is secret, so that
    /// would leak nothing — but the fixed bound is also simpler, and the eight wasted doublings at
    /// the top of a small scalar are not worth a branch.
    pub const BITS: usize = 253;

    /// The little-endian encoding.
    #[cfg(feature = "test-signing")]
    pub fn to_bytes(self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, limb) in self.0.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&limb.to_le_bytes());
        }
        out
    }

    /// `self * other + addend`, modulo `L` — the one composite operation signing needs.
    ///
    /// Schoolbook into eight limbs, then back through [`Scalar::reduce_wide`]. Under
    /// `test-signing` only: nothing that ships computes with a secret scalar (§19).
    #[cfg(feature = "test-signing")]
    pub fn mul_add(&self, other: &Scalar, addend: &Scalar) -> Scalar {
        // Schoolbook, one row at a time with the carry kept in a `u128`. The bound is tight and
        // worth writing down: `a*b + acc + carry` is at most `(2^64-1)^2 + 2(2^64-1) = 2^128 - 1`,
        // so the accumulator never wraps and the carry out is always a single limb.
        let mut wide = [0u64; 8];
        for (i, a) in self.0.iter().enumerate() {
            let mut carry: u128 = 0;
            for (j, b) in other.0.iter().enumerate() {
                let t = (*a as u128) * (*b as u128) + (wide[i + j] as u128) + carry;
                wide[i + j] = t as u64;
                carry = t >> 64;
            }
            wide[i + 4] = carry as u64;
        }

        let mut bytes = [0u8; 64];
        for (i, limb) in wide.iter().enumerate() {
            bytes[i * 8..i * 8 + 8].copy_from_slice(&limb.to_le_bytes());
        }
        let product = Scalar::reduce_wide(&bytes);

        let mut sum = product.0;
        let mut carry = 0u64;
        for (slot, add) in sum.iter_mut().zip(addend.0.iter()) {
            let (a, c1) = slot.overflowing_add(*add);
            let (a, c2) = a.overflowing_add(carry);
            *slot = a;
            carry = u64::from(c1) | u64::from(c2);
        }
        // Both operands are below L < 2^253, so the sum is below 2^254 and no limb carried out.
        debug_assert_eq!(carry, 0, "two reduced scalars cannot overflow four limbs");
        if at_least_l(&sum) {
            sub_l(&mut sum);
        }
        Scalar(sum)
    }
}

/// `acc = acc * 2 + bit`. The caller guarantees `acc < 2^255`, so nothing carries out of limb 3.
fn shl1_add(acc: &mut [u64; 4], bit: u64) {
    let mut carry = bit;
    for limb in acc.iter_mut() {
        let next = *limb >> 63;
        *limb = (*limb << 1) | carry;
        carry = next;
    }
    debug_assert_eq!(carry, 0, "acc stays below 2^255 by the loop's invariant");
}

/// Whether `acc >= L`.
fn at_least_l(acc: &[u64; 4]) -> bool {
    for i in (0..4).rev() {
        if acc[i] != L[i] {
            return acc[i] > L[i];
        }
    }
    true
}

/// `acc -= L`. The caller has established `acc >= L`.
fn sub_l(acc: &mut [u64; 4]) {
    let mut borrow = 0u64;
    for (slot, l) in acc.iter_mut().zip(L.iter()) {
        let (d, b1) = slot.overflowing_sub(*l);
        let (d, b2) = d.overflowing_sub(borrow);
        *slot = d;
        borrow = u64::from(b1) | u64::from(b2);
    }
    debug_assert_eq!(borrow, 0, "acc >= L was checked before subtracting");
}

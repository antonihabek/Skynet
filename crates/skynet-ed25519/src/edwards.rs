//! Points on `-x^2 + y^2 = 1 + d x^2 y^2`, the twisted Edwards curve RFC 8032 signs over.
//!
//! Extended coordinates `(X : Y : Z : T)`, with `x = X/Z`, `y = Y/Z` and `T = XY/Z`. The fourth
//! coordinate is redundant, and it is what makes addition *complete*: one formula adds any two
//! points, a point to itself included, with no exceptional case to get wrong. Curve arithmetic with
//! exceptional cases is where implementations fail quietly, and an attacker who can steer a
//! computation into one is halfway to a forged signature.
//!
//! Only what verification needs is here: decompression, addition, doubling, scalar multiplication
//! by a variable point, and equality. There is no subtraction, no precomputed comb for the
//! basepoint, and no batch verification.

use core::ops::Add;

use crate::field::Fe;
use crate::scalar::Scalar;

/// `d = -121665/121666`, little-endian.
///
/// Pinned as bytes rather than as limbs so it can be read against the published constant, and
/// checked in this crate's tests against its *definition* — `d * 121666 + 121665 == 0` — so a
/// mistyped digit fails the build rather than producing a curve nobody else is on.
const D_BYTES: [u8; 32] = [
    0xa3, 0x78, 0x59, 0x13, 0xca, 0x4d, 0xeb, 0x75, 0xab, 0xd8, 0x41, 0x41, 0x4d, 0x0a, 0x70, 0x00,
    0x98, 0xe8, 0x79, 0x77, 0x79, 0x40, 0xc7, 0x8c, 0x73, 0xfe, 0x6f, 0x2b, 0xee, 0x6c, 0x03, 0x52,
];

/// `sqrt(-1)`, little-endian. Checked in tests by squaring it.
const SQRT_M1_BYTES: [u8; 32] = [
    0xb0, 0xa0, 0x0e, 0x4a, 0x27, 0x1b, 0xee, 0xc4, 0x78, 0xe4, 0x2f, 0xad, 0x06, 0x18, 0x43, 0x2f,
    0xa7, 0xd7, 0xfb, 0x3d, 0x99, 0x00, 0x4d, 0x2b, 0x0b, 0xdf, 0xc1, 0x4f, 0x80, 0x24, 0x83, 0x2b,
];

/// The basepoint, in its compressed form.
///
/// Stored compressed and decompressed on use rather than stored as coordinates. It costs one square
/// root per verification, which is nothing beside two scalar multiplications, and it buys two
/// things: one basepoint constant instead of four coordinate constants that have to agree with each
/// other, and [`Point::decompress`] — the routine that parses attacker-supplied keys — exercised on
/// the one input whose correct answer is published.
pub const BASEPOINT_BYTES: [u8; 32] = [
    0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
];

/// `d`.
pub fn d() -> Fe {
    Fe::from_bytes(&D_BYTES)
}

/// `sqrt(-1)`.
pub(crate) fn sqrt_m1() -> Fe {
    Fe::from_bytes(&SQRT_M1_BYTES)
}

/// The basepoint.
///
/// Its encoding is a published constant and is on the curve, so this cannot fail; if it ever did,
/// every signature would be *rejected* rather than wrongly accepted, which is the direction a
/// failure here should point.
pub fn basepoint() -> Option<Point> {
    Point::decompress(&BASEPOINT_BYTES)
}

/// A curve point in extended coordinates.
#[derive(Debug, Clone, Copy)]
pub struct Point {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

/// Why a 32-byte string is not a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecompressError {
    /// `y` is at or above `p`: a field element spelled the long way.
    NotCanonical,
    /// `y` is canonical, and no `x` satisfies the curve equation for it.
    NotOnCurve,
}

impl Point {
    /// The identity, `(0, 1)`.
    pub fn identity() -> Point {
        Point {
            x: Fe::ZERO,
            y: Fe::ONE,
            z: Fe::ONE,
            t: Fe::ZERO,
        }
    }

    /// Parse a compressed point: 255 bits of `y`, then the sign of `x` in the top bit.
    pub fn decompress(bytes: &[u8; 32]) -> Option<Point> {
        Self::decompress_detail(bytes).ok()
    }

    /// [`Point::decompress`], keeping the reason.
    ///
    /// Three ways this refuses, and every one of them is an encoding somebody has actually sent:
    ///
    /// * `y >= p` — a valid element written the long way, which would give one point two names;
    /// * no square root — `y` names nothing on this curve;
    /// * `x == 0` with the sign bit set — zero is the one value whose two signs are the same value,
    ///   so the sign bit carries a claim the encoding cannot support. There is exactly one such
    ///   string, and it is a second spelling of a point that already has one.
    pub fn decompress_detail(bytes: &[u8; 32]) -> Result<Point, DecompressError> {
        if !Fe::is_canonical_encoding(bytes) {
            return Err(DecompressError::NotCanonical);
        }
        let sign = bytes[31] >> 7 == 1;
        let y = Fe::from_bytes(bytes);
        let yy = y.square();

        // x^2 = (y^2 - 1) / (d y^2 + 1)
        let u = yy - Fe::ONE;
        let v = d() * yy + Fe::ONE;

        let x = sqrt_ratio(u, v).ok_or(DecompressError::NotOnCurve)?;

        if x.is_zero() && sign {
            return Err(DecompressError::NotCanonical);
        }
        let x = x.negate_if(x.is_negative() != sign);

        Ok(Point {
            t: x * y,
            x,
            y,
            z: Fe::ONE,
        })
    }

    /// The compressed encoding.
    pub fn compress(&self) -> [u8; 32] {
        let z_inv = self.z.invert();
        let x = self.x * z_inv;
        let y = self.y * z_inv;
        let mut out = y.to_bytes();
        out[31] |= u8::from(x.is_negative()) << 7;
        out
    }

    /// Doubling, `dbl-2008-hwcd` for `a = -1`.
    ///
    /// A separate formula from addition only because it is cheaper — adding a point to itself gives
    /// the same answer, which is what "complete" means and is what the tests assert.
    pub fn double(&self) -> Point {
        let a = self.x.square();
        let b = self.y.square();
        let c = self.z.square();
        let c = c + c;
        let neg_a = -a;
        let e = (self.x + self.y).square() - a - b;
        let g = neg_a + b;
        let f = g - c;
        let h = neg_a - b;

        Point {
            x: e * f,
            y: g * h,
            t: e * h,
            z: f * g,
        }
    }

    /// `[k] self`, by double-and-add from the top bit down.
    ///
    /// Branching on bits of `k`, which leaks `k` through timing. Every scalar multiplied during
    /// verification is public — `S` arrives in the signature, and `k` is a hash of three public
    /// values — so there is nothing to leak. Signing is where that stops being true, and it is
    /// behind a feature for exactly this reason.
    pub fn scalar_mul(&self, k: &Scalar) -> Point {
        let mut acc = Point::identity();
        for i in (0..Scalar::BITS).rev() {
            acc = acc.double();
            if k.bit(i) {
                acc = acc + *self;
            }
        }
        acc
    }

    /// Whether this is the identity.
    ///
    /// `x == 0` and `y == 1`, tested projectively so the answer costs no inversion: `X/Z == 0` is
    /// `X == 0`, and `Y/Z == 1` is `Y == Z`.
    pub fn is_identity(&self) -> bool {
        self.x.is_zero() && self.y == self.z
    }

    /// Whether this point has order 1, 2, 4 or 8 — the cofactor subgroup.
    ///
    /// Tested by three doublings rather than against a table of the eight encodings. Three
    /// doublings need no constants to keep correct; a table is a thing that can be wrong in a way
    /// nothing else notices.
    pub fn is_small_order(&self) -> bool {
        self.double().double().double().is_identity()
    }
}

/// Projective equality: `X1 Z2 == X2 Z1` and `Y1 Z2 == Y2 Z1`.
///
/// Four multiplications, against the two inversions that comparing compressed encodings would cost.
/// The saving is not the reason — the reason is that this cannot be got wrong by forgetting to
/// normalise, and a verifier that compares unnormalised coordinates directly rejects valid
/// signatures for reasons nobody can reproduce.
impl PartialEq for Point {
    fn eq(&self, other: &Point) -> bool {
        self.x * other.z == other.x * self.z && self.y * other.z == other.y * self.z
    }
}

impl Eq for Point {}

impl Add for Point {
    type Output = Point;

    /// `add-2008-hwcd-3` for `a = -1`. Complete: no input pair is exceptional.
    fn add(self, rhs: Point) -> Point {
        let d2 = d() + d();

        let a = (self.y - self.x) * (rhs.y - rhs.x);
        let b = (self.y + self.x) * (rhs.y + rhs.x);
        let c = self.t * d2 * rhs.t;
        let dd = self.z * rhs.z;
        let dd = dd + dd;

        let e = b - a;
        let f = dd - c;
        let g = dd + c;
        let h = b + a;

        Point {
            x: e * f,
            y: g * h,
            t: e * h,
            z: f * g,
        }
    }
}

/// The square root of `u/v`, if one exists.
///
/// RFC 8032's candidate is `x = u v^3 (u v^7)^((p-5)/8)`. Then `v x^2` is `u` if the root was
/// found, `-u` if the root of `-u/v` was found instead — in which case multiplying by `sqrt(-1)`
/// fixes it — and neither if `u/v` is not a square at all.
fn sqrt_ratio(u: Fe, v: Fe) -> Option<Fe> {
    let v2 = v.square();
    let v3 = v2 * v;
    let v6 = v3.square();
    let v7 = v6 * v;

    let candidate = u * v3 * (u * v7).pow_p58();

    let check = v * candidate.square();
    if check == u {
        return Some(candidate);
    }
    if check == -u {
        return Some(candidate * sqrt_m1());
    }
    None
}

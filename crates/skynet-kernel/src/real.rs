//! The real arithmetic the specification defines.
//!
//! Forward kinematics needs a square root and a sine. `core` has neither — they live in `std`,
//! which this crate cannot link (§23.1) — so something had to supply them, and the choice of
//! *what* is not a packaging detail. It decides whether two editions of Skynet OS compute the same
//! clamp.
//!
//! # Why the platform's library is the wrong answer
//!
//! Three requirements point the same way, and each on its own would be enough:
//!
//! * **§31.4 requires byte-identical output across editions.** The corpus compares canonical JSON
//!   strings, so a pose that differs in the last bit is a failed edition. ECMAScript permits
//!   `Math.sin` to be off by an implementation-defined amount and every C library rounds its own
//!   way; two editions calling their own libraries would diverge on the first revolute joint, and
//!   the divergence would look like a specification disagreement rather than what it is.
//! * **§7.4's certified tier mirrors this kernel on a separate safety MCU.** The MCU's toolchain is
//!   not the application processor's. A redundant channel that disagrees with the primary about a
//!   clamp is the exact failure the redundancy exists to detect, and a libm difference would make
//!   it fire on correct behaviour — which is the fastest way to have redundancy switched off.
//! * **The workspace has no third-party dependencies** and the kernel builds for bare metal, where
//!   there is no library to call.
//!
//! So the arithmetic is written out, once, as an explicit sequence of IEEE-754 binary64 operations.
//! Addition, subtraction, multiplication and division are correctly rounded on every conforming
//! platform, so **the same sequence of them produces the same bits everywhere** — which is the
//! property that matters here, and it is a stronger one than "accurate", because accuracy varies by
//! platform and this does not. `reference/typescript/kinematics.ts` holds the same sequence, and
//! the corpus pins both.
//!
//! # Accuracy, stated rather than implied
//!
//! [`sqrt`] is a fixed five-step Newton iteration from a bit-pattern seed, which reaches the
//! neighbourhood of the correctly rounded root and stops there rather than at whatever step
//! happened to converge. [`sin`] and [`cos`] are Cody–Waite range reduction onto `[-π/4, π/4]`
//! followed by the fdlibm minimax polynomials. Both are within roughly one unit in the last place —
//! and, much more to the point, within *zero* of each other across editions.
//!
//! Nothing here uses a fused multiply-add. `a * b + c` with an FMA rounds once and without one
//! rounds twice, and a platform that contracted the expression would produce different bits from
//! one that did not.

// ── Square root ──────────────────────────────────────────────────────────────

/// Square root.
///
/// Newton's iteration `y ← (y + x/y) / 2` from a seed built by halving the exponent, run a fixed
/// **five** times. The fixed count is the specification: an iteration that stopped on a convergence
/// test would stop after a different number of steps on a different platform, and two editions
/// taking different numbers of correctly rounded steps can land on different bits.
///
/// Five is chosen with room to spare. The seed is good to about 3% relative, and each step squares
/// the error: 3e-2, 5e-4, 1e-7, 6e-15, then a step that can only move the result by an ulp or two
/// and after which the iteration is at its fixed point.
pub fn sqrt(x: f64) -> f64 {
    if x.is_nan() || x < 0.0 {
        return f64::NAN;
    }
    if x == 0.0 {
        // Preserves the sign, so `sqrt(-0.0)` is `-0.0` as IEEE-754 requires.
        return x;
    }
    if x == f64::INFINITY {
        return x;
    }

    // Seed: halve the exponent by arithmetic on the bit pattern. For a subnormal this produces a
    // poor but positive estimate, and Newton repairs it — the alternative, a branch that scales
    // subnormals into the normal range, is more code for inputs a length in metres never takes.
    let bits = x.to_bits();
    let mut y = f64::from_bits((bits >> 1) + (1023u64 << 51));
    if y == 0.0 || !y.is_finite() {
        y = x;
    }

    let mut step = 0;
    while step < 5 {
        y = 0.5 * (y + x / y);
        step += 1;
    }
    y
}

// ── Range reduction ──────────────────────────────────────────────────────────

/// 2/π, to the nearest double: `0x1.45f306dc9c883p-1`.
///
/// Taken from `core` rather than written out, because a literal here is the one place a typo would
/// be invisible — every value would still be *a* number, just a quadrant off in the tail. The
/// TypeScript edition writes the same bit pattern as a literal and the corpus holds the two to each
/// other.
const TWO_OVER_PI: f64 = core::f64::consts::FRAC_2_PI;

/// π/2 with its low 33 bits cleared, so `k * PIO2_1` is exact for every `k` this reduction admits.
#[allow(clippy::excessive_precision)]
const PIO2_1: f64 = 1.57079632673412561417e+00;
/// π/2 − `PIO2_1`, itself split, so the second step recovers the bits the first one dropped.
#[allow(clippy::excessive_precision)]
const PIO2_2: f64 = 6.07710050630396597660e-11;
/// The remainder of that split.
#[allow(clippy::excessive_precision)]
const PIO2_2T: f64 = 2.02226624879595063154e-21;

/// The largest quadrant count the reduction is exact for.
///
/// `PIO2_1` carries 33 significant bits, so `k * PIO2_1` is representable exactly while `k` fits in
/// 20 — which is `|x|` up to about 1.65 × 10⁶ radians, a quarter of a million revolutions. Past
/// that the product rounds and the reduced angle is wrong by an amount that grows with `k`, so the
/// caller is told rather than handed a pose computed from it. See [`Reduced::exact`].
const MAX_QUADRANTS: f64 = 1_048_576.0;

/// A reduced angle and its quadrant.
#[derive(Debug, Clone, Copy)]
struct Reduced {
    /// The angle in `[-π/4, π/4]`.
    r: f64,
    /// Quadrant, `k mod 4`, non-negative.
    quadrant: u32,
    /// Whether the reduction was exact. False past [`MAX_QUADRANTS`], where the result is not to
    /// be used — a wrong pose is worse than a missing one (§7.2).
    exact: bool,
}

/// Round half away from zero, to an integer valued `f64`.
///
/// Written out rather than taken from a library because the libraries disagree: ECMAScript's
/// `Math.round` rounds −0.5 to −0, and Rust's `f64::round` rounds it to −1. Two editions differing
/// here would differ by a whole quadrant.
fn round_half_away(v: f64) -> f64 {
    let shifted = if v >= 0.0 { v + 0.5 } else { v - 0.5 };
    // `as i64` truncates towards zero, which is what the shift above expects. The caller has
    // already bounded the magnitude, so the cast cannot saturate.
    (shifted as i64) as f64
}

fn reduce(x: f64) -> Reduced {
    let k = round_half_away(x * TWO_OVER_PI);
    if k.is_nan() || abs(k) >= MAX_QUADRANTS {
        return Reduced {
            r: 0.0,
            quadrant: 0,
            exact: false,
        };
    }

    // Cody–Waite, two steps, taken unconditionally. fdlibm takes the second only when the first
    // cancelled badly; testing for that would make the number of operations depend on the input,
    // and a specification that says "sometimes three multiplications and sometimes six" is one an
    // edition can implement differently without noticing.
    let t = x - k * PIO2_1;
    let w = k * PIO2_2;
    let r = t - w;
    let w = k * PIO2_2T - ((t - r) - w);
    let r = r - w;

    // `k` is integral and within 2^20, so this is exact.
    let quadrant = (((k as i64) % 4 + 4) % 4) as u32;
    Reduced {
        r,
        quadrant,
        exact: true,
    }
}

// ── The kernel polynomials, on [-π/4, π/4] ───────────────────────────────────
//
// fdlibm's minimax coefficients, written with fdlibm's own digits. Clippy would rather they were
// truncated to the shortest decimal that round-trips, and the truncated form denotes the identical
// double — but the point of these twenty digits is that a reviewer can diff them against the
// published table without converting anything, and the TypeScript edition writes the same
// characters. A constant nobody can check against its source is a constant nobody will correct.
#[allow(clippy::excessive_precision)]
const S1: f64 = -1.66666666666666324348e-01;
#[allow(clippy::excessive_precision)]
const S2: f64 = 8.33333333332248946124e-03;
#[allow(clippy::excessive_precision)]
const S3: f64 = -1.98412698298579493134e-04;
#[allow(clippy::excessive_precision)]
const S4: f64 = 2.75573137070700676789e-06;
#[allow(clippy::excessive_precision)]
const S5: f64 = -2.50507602534068634195e-08;
#[allow(clippy::excessive_precision)]
const S6: f64 = 1.58969099521155010221e-10;

#[allow(clippy::excessive_precision)]
const C1: f64 = 4.16666666666666019037e-02;
#[allow(clippy::excessive_precision)]
const C2: f64 = -1.38888888888741095749e-03;
#[allow(clippy::excessive_precision)]
const C3: f64 = 2.48015872894767294178e-05;
#[allow(clippy::excessive_precision)]
const C4: f64 = -2.75573143513906633035e-07;
#[allow(clippy::excessive_precision)]
const C5: f64 = 2.08757232129817482790e-09;
#[allow(clippy::excessive_precision)]
const C6: f64 = -1.13596475577881948265e-11;

fn kernel_sin(x: f64) -> f64 {
    let z = x * x;
    let poly = S1 + z * (S2 + z * (S3 + z * (S4 + z * (S5 + z * S6))));
    x + x * z * poly
}

fn kernel_cos(x: f64) -> f64 {
    let z = x * x;
    let poly = C1 + z * (C2 + z * (C3 + z * (C4 + z * (C5 + z * C6))));
    1.0 - 0.5 * z + z * z * poly
}

/// Sine.
///
/// Returns NaN for an angle outside the range the reduction is exact over, and for a non-finite
/// input. A NaN propagates into the pose, and [`crate::kinematics`] treats a non-finite pose as an
/// unresolved frame and names it — which is the §7.2 answer, applied to arithmetic: a guard that
/// cannot compute its input reports unavailable rather than acting on a number it does not trust.
pub fn sin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let reduced = reduce(x);
    if !reduced.exact {
        return f64::NAN;
    }
    match reduced.quadrant {
        0 => kernel_sin(reduced.r),
        1 => kernel_cos(reduced.r),
        2 => -kernel_sin(reduced.r),
        _ => -kernel_cos(reduced.r),
    }
}

/// Cosine. Same domain rule as [`sin`].
pub fn cos(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    let reduced = reduce(x);
    if !reduced.exact {
        return f64::NAN;
    }
    match reduced.quadrant {
        0 => kernel_cos(reduced.r),
        1 => -kernel_sin(reduced.r),
        2 => -kernel_cos(reduced.r),
        _ => kernel_sin(reduced.r),
    }
}

/// Absolute value.
///
/// `f64::abs` is `std`. Written as a comparison rather than by clearing the sign bit so that the
/// two editions agree on `-0.0`, which this maps to `0.0` exactly as clearing the bit would.
pub fn abs(x: f64) -> f64 {
    if x < 0.0 {
        -x
    } else if x == 0.0 {
        0.0
    } else {
        x
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tolerance every check below uses: within one part in 10^15, which is about four ulp at
    /// these magnitudes. Tighter than the guards need and loose enough that a test failure means a
    /// mistake rather than a rounding mode.
    fn close(a: f64, b: f64) -> bool {
        let scale = if abs(b) > 1.0 { abs(b) } else { 1.0 };
        abs(a - b) <= 1e-15 * scale
    }

    #[test]
    fn square_roots_of_the_obvious_cases() {
        assert_eq!(sqrt(0.0), 0.0);
        assert_eq!(sqrt(1.0), 1.0);
        assert_eq!(sqrt(4.0), 2.0);
        assert_eq!(sqrt(9.0), 3.0);
        assert_eq!(sqrt(f64::INFINITY), f64::INFINITY);
        assert!(sqrt(-1.0).is_nan());
        assert!(sqrt(f64::NAN).is_nan());
    }

    #[test]
    fn square_roots_over_a_wide_range() {
        // A metre-scale robot's numbers, and then well past both ends of them.
        let cases = [
            1e-12, 1e-6, 0.001, 0.25, 0.5, 2.0, 1234.5678, 1e9, 1e18, 1e-300, 1e300,
        ];
        for x in cases {
            let y = sqrt(x);
            assert!(close(y * y, x), "sqrt({x}) = {y}, squared to {}", y * y);
        }
    }

    #[test]
    fn negative_zero_keeps_its_sign() {
        assert!(sqrt(-0.0).is_sign_negative());
    }

    #[test]
    fn sine_and_cosine_at_the_quadrant_boundaries() {
        let pi = core::f64::consts::PI;

        assert!(close(sin(0.0), 0.0));
        assert!(close(cos(0.0), 1.0));
        assert!(close(sin(pi / 2.0), 1.0));
        assert!(close(cos(pi / 2.0), 0.0));
        assert!(close(sin(pi), 0.0));
        assert!(close(cos(pi), -1.0));
        assert!(close(sin(-pi / 2.0), -1.0));
        assert!(close(cos(-pi / 2.0), 0.0));
    }

    #[test]
    fn the_pythagorean_identity_holds_across_the_working_range() {
        // Every tenth of a radian out to a hundred, which is sixteen revolutions — past anything a
        // limb reaches and into the territory a continuous joint occupies.
        let mut i = -1000;
        while i <= 1000 {
            let x = i as f64 * 0.1;
            let s = sin(x);
            let c = cos(x);
            assert!(close(s * s + c * c, 1.0), "at {x}");
            i += 1;
        }
    }

    #[test]
    fn the_angle_sum_identity_holds() {
        // A property the polynomials do not have by construction, so it exercises the reduction
        // rather than the fit.
        let cases = [
            (0.3, 0.4),
            (1.2, 2.7),
            (-0.9, 3.3),
            (10.0, 0.1),
            (100.0, 100.0),
        ];
        for (a, b) in cases {
            let expected = sin(a) * cos(b) + cos(a) * sin(b);
            assert!(close(sin(a + b), expected), "sin({a} + {b})");
        }
    }

    #[test]
    fn an_angle_past_the_exact_reduction_is_refused_rather_than_guessed() {
        // A quarter of a million revolutions. The specification's answer is "not a number", which
        // the kinematics reads as an unresolved frame — never a pose computed from a reduction
        // that has silently lost its low bits.
        assert!(sin(2.0e6).is_nan());
        assert!(cos(2.0e6).is_nan());
        assert!(sin(f64::INFINITY).is_nan());
        assert!(cos(f64::NAN).is_nan());
        // And the boundary itself is inside.
        assert!(!sin(1.6e6).is_nan());
    }

    #[test]
    fn rounding_half_away_from_zero_is_symmetric() {
        // The behaviour ECMAScript's Math.round does not have. Both editions implement this one.
        assert_eq!(round_half_away(0.5), 1.0);
        assert_eq!(round_half_away(-0.5), -1.0);
        assert_eq!(round_half_away(1.4), 1.0);
        assert_eq!(round_half_away(-1.6), -2.0);
    }

    #[test]
    fn absolute_value_normalises_negative_zero() {
        assert!(!abs(-0.0).is_sign_negative());
        assert_eq!(abs(-3.5), 3.5);
        assert_eq!(abs(3.5), 3.5);
    }
}

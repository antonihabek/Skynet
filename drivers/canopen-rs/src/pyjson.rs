//! Numbers and strings exactly as Python's `json.dumps` writes them.
//!
//! The Python bridge renders its descriptor with `json.dumps(descriptor, separators=(",", ":"))`,
//! and the port's descriptor is held to it byte for byte (`tests/live_vcan.rs`). A template with
//! holes cannot do that here as it does in `franka-rs`, because this body's figures are read off
//! the drives: travel and speed are computed floats, and their text is Python's `repr` of them.
//!
//! This is **not** `skynet-json`'s canonical form, deliberately. §31.4's canonical form is the
//! corpus's, it writes an integral float as an integer, and it has no exponent; `repr` writes
//! `3.0` and switches to `1e-05` below 1e-4 and to `1e+16` at 1e16. The descriptor is the Python
//! driver's output, so its rules are Python's. `tests/vectors.rs` holds both functions to
//! `json.dumps` over the cases where the two formats part.

/// A finite `f64` as `repr(float)` writes it. `None` for NaN and the infinities, which Python
/// would write as the non-JSON `NaN` and `Infinity` — a descriptor carrying one is a defect, and
/// the caller refuses rather than emits it.
pub fn number(value: f64) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    // Rust's `{:e}` is the shortest representation that round-trips, as `repr` is: `d.ddde±x`.
    let shortest = format!("{:e}", value.abs());
    let (mantissa, exponent) = shortest.split_once('e')?;
    let exponent: i32 = exponent.parse().ok()?;
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    // dtoa's `decpt`: the value is `0.<digits> × 10^decpt`.
    let decpt = exponent + 1;
    let sign = if value.is_sign_negative() { "-" } else { "" };

    // `float_repr_style = 'short'`: an exponent below 1e-4 and from 1e16 up (CPython,
    // `Python/pystrtod.c`, `format_float_short` with mode `'r'`).
    if decpt <= -4 || decpt > 16 {
        let (first, rest) = digits.split_at(1);
        let fraction = if rest.is_empty() {
            String::new()
        } else {
            format!(".{rest}")
        };
        let power = decpt - 1;
        let power_sign = if power < 0 { '-' } else { '+' };
        return Some(format!(
            "{sign}{first}{fraction}e{power_sign}{:02}",
            power.unsigned_abs()
        ));
    }
    let count = digits.len() as i32;
    let text = if decpt <= 0 {
        format!("0.{}{digits}", "0".repeat((-decpt) as usize))
    } else if decpt < count {
        let (whole, fraction) = digits.split_at(decpt as usize);
        format!("{whole}.{fraction}")
    } else {
        // `Py_DTSF_ADD_DOT_0`: an integral float keeps its `.0`, which is how Python tells a float
        // from an int on the way out.
        format!("{digits}{}.0", "0".repeat((decpt - count) as usize))
    };
    Some(format!("{sign}{text}"))
}

/// A string as `json.dumps` writes it under its default `ensure_ascii=True`: quoted, with the
/// seven short escapes, and every other character outside printable ASCII as `\uXXXX` —
/// lowercase, and as a surrogate pair above the Basic Multilingual Plane.
pub fn string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ' '..='~' => out.push(ch),
            other => {
                let mut units = [0u16; 2];
                for unit in other.encode_utf16(&mut units) {
                    out.push_str(&format!("\\u{unit:04x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_python_cannot_write_as_json_is_refused() {
        assert_eq!(number(f64::NAN), None);
        assert_eq!(number(f64::INFINITY), None);
        assert_eq!(number(f64::NEG_INFINITY), None);
    }
}

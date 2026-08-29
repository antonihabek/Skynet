//! Canonical JSON.
//!
//! §31.4 of the specification asks for something stricter than "the editions agree": it asks for
//! **byte-identical output**, on the grounds that equivalence is where drift hides — a warning in
//! a different order today is a different clamp tomorrow. That requirement is only meaningful if
//! there is exactly one way to write a given value down, so this crate defines one and every
//! edition implements it.
//!
//! # The canonical form
//!
//! 1. **No insignificant whitespace.** No spaces after `:` or `,`, no newlines, no trailing newline.
//! 2. **Object keys are sorted** by the byte sequence of their UTF-8 encoding, ascending.
//!    Duplicate keys cannot occur because the representation is a map.
//! 3. **Numbers** are finite. `NaN` and the infinities have no JSON representation and are a
//!    serialisation error rather than a silently emitted `null`, because a limit that serialised
//!    to `null` would read as "no limit".
//!    * An integral value with magnitude below 2^53 is written as a decimal integer with no
//!      fraction and no exponent. `-0.0` is written `0`.
//!    * Anything else is written as the **shortest decimal string that parses back to the same
//!      bits**, without an exponent. The algorithm is in [`format_number`] and is specified
//!      precisely enough to reimplement, because every edition must.
//! 4. **Strings** escape `"`, `\` and the C0 controls, using the short forms `\b \f \n \r \t`
//!    where they exist and `\u00xx` otherwise. Nothing else is escaped: `/` is written literally
//!    and non-ASCII is written as UTF-8.
//! 5. **Absent is not null.** A field with no value is omitted from the object entirely. This
//!    matters more here than in most schemas: the whole system distinguishes "nobody declared a
//!    safety envelope" from "an envelope was declared and is empty", and collapsing the two into
//!    `null` would erase exactly the distinction the platform exists to preserve.
//!
//! The parser is deliberately strict — no comments, no trailing commas, no leading `+`, no
//! hexadecimal. A corpus file is data that several implementations must read identically, so
//! every lenient extension is a place two parsers can disagree.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::fmt;

/// A JSON value.
///
/// Objects are a [`BTreeMap`], so key order is canonical by construction rather than by a sort
/// applied at write time — there is no way to hold a value whose serialisation would be
/// non-canonical.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// A finite number. Serialising a non-finite value is an error.
    Num(f64),
    /// A string.
    Str(String),
    /// An array. Order is significant and preserved.
    Arr(Vec<Json>),
    /// An object. Keys are held sorted.
    Obj(BTreeMap<String, Json>),
}

impl Json {
    /// An empty object, ready for [`Json::set`].
    pub fn obj() -> Json {
        Json::Obj(BTreeMap::new())
    }

    /// An empty array.
    pub fn arr() -> Json {
        Json::Arr(Vec::new())
    }

    /// Insert a field. Panics if `self` is not an object, which is a programming error rather
    /// than a data error and should not be recoverable.
    pub fn set(&mut self, key: &str, value: Json) -> &mut Self {
        match self {
            Json::Obj(map) => {
                map.insert(key.to_string(), value);
            }
            _ => panic!("set() on a non-object"),
        }
        self
    }

    /// Insert a field only when the value is present.
    ///
    /// This is the mechanism behind rule 5: an optional field that nobody declared is *absent*
    /// from the output rather than present and null.
    pub fn set_opt(&mut self, key: &str, value: Option<Json>) -> &mut Self {
        if let Some(value) = value {
            self.set(key, value);
        }
        self
    }

    /// Push onto an array. Panics if `self` is not an array.
    pub fn push(&mut self, value: Json) -> &mut Self {
        match self {
            Json::Arr(items) => items.push(value),
            _ => panic!("push() on a non-array"),
        }
        self
    }

    /// Field lookup, `None` for a missing key or a non-object.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(map) => map.get(key),
            _ => None,
        }
    }

    /// Field lookup that treats an explicit `null` as absent.
    ///
    /// Corpus files are hand-writable, and a field written as `null` means the same thing to a
    /// reader as a field left out. The distinction rule 5 preserves is in the *output*, where a
    /// producer chooses; on input both spellings mean "no value".
    pub fn get_present(&self, key: &str) -> Option<&Json> {
        match self.get(key) {
            None | Some(Json::Null) => None,
            other => other,
        }
    }

    /// The value as a string, `None` for any other type.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }

    /// The value as a number, `None` for any other type.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Json::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// The value as a `usize`, for counts and indices. Non-integral or negative yields `None`.
    pub fn as_usize(&self) -> Option<usize> {
        let n = self.as_f64()?;
        if n >= 0.0 && is_integral(n) {
            Some(n as usize)
        } else {
            None
        }
    }

    /// The value as a bool, `None` for any other type.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Json::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// The value as an array slice, `None` for any other type.
    pub fn as_arr(&self) -> Option<&[Json]> {
        match self {
            Json::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// The value as an object, `None` for any other type.
    pub fn as_obj(&self) -> Option<&BTreeMap<String, Json>> {
        match self {
            Json::Obj(map) => Some(map),
            _ => None,
        }
    }

    /// Array elements, or an empty slice for anything else.
    ///
    /// The lenient shape is right at the call sites that use it: a corpus case that declares no
    /// action channels and one that declares an empty list are the same case.
    pub fn arr_or_empty(&self, key: &str) -> &[Json] {
        self.get(key).and_then(Json::as_arr).unwrap_or(&[])
    }

    /// Every string in the named array, skipping non-strings.
    pub fn str_arr(&self, key: &str) -> Vec<String> {
        self.arr_or_empty(key)
            .iter()
            .filter_map(|v| v.as_str().map(ToString::to_string))
            .collect()
    }

    /// Serialise in canonical form.
    ///
    /// Returns [`JsonError::NonFinite`] rather than emitting `null` for `NaN` or an infinity.
    pub fn to_canonical(&self) -> Result<String, JsonError> {
        let mut out = String::new();
        write_value(self, &mut out)?;
        Ok(out)
    }

    /// Parse. Strict: no comments, no trailing commas, no unquoted keys.
    pub fn parse(input: &str) -> Result<Json, JsonError> {
        let bytes = input.as_bytes();
        let mut parser = Parser { bytes, pos: 0 };
        parser.skip_ws();
        let value = parser.value()?;
        parser.skip_ws();
        if parser.pos != bytes.len() {
            return Err(JsonError::Trailing(parser.pos));
        }
        Ok(value)
    }
}

impl From<bool> for Json {
    fn from(v: bool) -> Json {
        Json::Bool(v)
    }
}
impl From<f64> for Json {
    fn from(v: f64) -> Json {
        Json::Num(v)
    }
}
impl From<usize> for Json {
    fn from(v: usize) -> Json {
        Json::Num(v as f64)
    }
}
impl From<u32> for Json {
    fn from(v: u32) -> Json {
        Json::Num(v as f64)
    }
}
impl From<i64> for Json {
    fn from(v: i64) -> Json {
        Json::Num(v as f64)
    }
}
impl From<&str> for Json {
    fn from(v: &str) -> Json {
        Json::Str(v.to_string())
    }
}
impl From<String> for Json {
    fn from(v: String) -> Json {
        Json::Str(v)
    }
}
impl<T: Into<Json>> From<Vec<T>> for Json {
    fn from(v: Vec<T>) -> Json {
        Json::Arr(v.into_iter().map(Into::into).collect())
    }
}
impl<T: Into<Json>> From<Option<T>> for Json {
    fn from(v: Option<T>) -> Json {
        match v {
            Some(inner) => inner.into(),
            None => Json::Null,
        }
    }
}

/// What can go wrong reading or writing canonical JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonError {
    /// A `NaN` or infinity was serialised. There is no JSON spelling for either, and emitting
    /// `null` would read as "no value declared", which in this system means something specific.
    NonFinite,
    /// The input ended mid-value.
    Eof,
    /// Something unparseable at this byte offset.
    Unexpected(usize),
    /// A complete value was followed by more input.
    Trailing(usize),
    /// A `\u` escape that is not four hex digits, or an unpaired surrogate.
    BadEscape(usize),
    /// Nesting deeper than [`MAX_DEPTH`]. A bound exists so a hostile corpus file cannot exhaust
    /// the stack of the process reading it.
    TooDeep(usize),
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JsonError::NonFinite => write!(f, "cannot serialise a non-finite number"),
            JsonError::Eof => write!(f, "unexpected end of input"),
            JsonError::Unexpected(at) => write!(f, "unexpected byte at offset {at}"),
            JsonError::Trailing(at) => write!(f, "trailing input at offset {at}"),
            JsonError::BadEscape(at) => write!(f, "bad escape at offset {at}"),
            JsonError::TooDeep(at) => write!(f, "nesting too deep at offset {at}"),
        }
    }
}

/// Maximum nesting depth accepted by the parser.
pub const MAX_DEPTH: usize = 64;

// ── Writing ──────────────────────────────────────────────────────────────────

fn write_value(value: &Json, out: &mut String) -> Result<(), JsonError> {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Num(n) => out.push_str(&format_number(*n)?),
        Json::Str(s) => write_string(s, out),
        Json::Arr(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_value(item, out)?;
            }
            out.push(']');
        }
        Json::Obj(map) => {
            out.push('{');
            // BTreeMap iterates in key order, which *is* the canonical order — there is no sort
            // step here because there is no way to construct an unsorted object.
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(item, out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_string(s: &str, out: &mut String) {
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u");
                let code = c as u32;
                for shift in [12, 8, 4, 0] {
                    out.push(hex_digit(((code >> shift) & 0xf) as u8));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

fn hex_digit(nibble: u8) -> char {
    match nibble {
        0..=9 => (b'0' + nibble) as char,
        _ => (b'a' + nibble - 10) as char,
    }
}

/// The largest magnitude at which every integer is exactly representable as an `f64`.
const INT_SAFE: f64 = 9_007_199_254_740_992.0; // 2^53

/// Whether a value is a whole number that survives a round trip through `i64`.
///
/// `f64::fract` and `f64::trunc` live in `std` rather than `core`, and this crate is `no_std` so
/// that the same canonicalisation is available to a build that has no operating system under it.
/// Comparing against the `i64` round trip needs no libm and is exact inside 2^53, which is the
/// only range where the integer spelling is used anyway.
fn is_integral(value: f64) -> bool {
    value.is_finite() && value.abs() < INT_SAFE && (value as i64) as f64 == value
}

/// Format a number in canonical form.
///
/// The rule, stated so another edition can reimplement it and land on the same bytes:
///
/// * A value that is integral and has magnitude below 2^53 is written as a decimal integer.
///   `-0.0` is written `0`, because negative zero and zero are the same quantity and two
///   spellings of one quantity is precisely what canonicalisation exists to remove.
/// * Otherwise: find the smallest `p` in `1..=17` such that rendering the value to `p`
///   significant digits and parsing the result yields the identical `f64`, then write those
///   digits positionally — never in exponent form. `p` is guaranteed to exist because 17
///   significant digits round-trip every `f64`.
///
/// Positional output rather than exponent output is the one place this diverges from what most
/// languages print by default, and it is deliberate: `1e-7` and `0.0000001` are the same number,
/// and picking the form that never needs an exponent removes a whole class of disagreement
/// between implementations about when to switch notation.
pub fn format_number(value: f64) -> Result<String, JsonError> {
    if !value.is_finite() {
        return Err(JsonError::NonFinite);
    }
    if value == 0.0 {
        return Ok("0".to_string());
    }
    if is_integral(value) {
        let mut s = String::new();
        let _ = fmt::Write::write_fmt(&mut s, format_args!("{}", value as i64));
        return Ok(s);
    }

    for precision in 1..=17usize {
        let rendered = render_exponential(value, precision - 1);
        if rendered.parse::<f64>().map(|back| back.to_bits()) == Ok(value.to_bits()) {
            return Ok(exponential_to_positional(&rendered));
        }
    }
    // Unreachable: 17 significant digits round-trip every finite f64. Kept as a value rather than
    // an `unwrap` so a future change to the loop bound cannot turn this into a panic in a robot.
    Ok(exponential_to_positional(&render_exponential(value, 16)))
}

fn render_exponential(value: f64, fraction_digits: usize) -> String {
    let mut s = String::new();
    let _ = fmt::Write::write_fmt(&mut s, format_args!("{:.*e}", fraction_digits, value));
    s
}

/// Turn `-4.4e-1` into `-0.44`.
///
/// Rust renders `{:e}` as `<digits>e<exp>` with the decimal point after the first digit and no
/// `+` on a positive exponent, which is the only shape this has to handle.
fn exponential_to_positional(rendered: &str) -> String {
    let (mantissa, exponent) = match rendered.split_once('e') {
        Some(parts) => parts,
        None => return rendered.to_string(),
    };
    let exponent: i32 = exponent.parse().unwrap_or(0);

    let (negative, mantissa) = match mantissa.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, mantissa),
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let digit_count = digits.len() as i32;

    // `point` is where the decimal point sits within `digits`, counted from the left. The first
    // digit carries weight 10^exponent, so the point falls after `exponent + 1` digits.
    let point = exponent + 1;

    let mut out = String::new();
    if negative {
        out.push('-');
    }
    if point <= 0 {
        out.push_str("0.");
        for _ in 0..(-point) {
            out.push('0');
        }
        out.push_str(&digits);
    } else if point >= digit_count {
        out.push_str(&digits);
        for _ in 0..(point - digit_count) {
            out.push('0');
        }
    } else {
        out.push_str(&digits[..point as usize]);
        out.push('.');
        out.push_str(&digits[point as usize..]);
    }
    out
}

// ── Parsing ──────────────────────────────────────────────────────────────────

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Parser<'a> {
    fn skip_ws(&mut self) {
        while let Some(b) = self.bytes.get(self.pos) {
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Result<u8, JsonError> {
        self.bytes.get(self.pos).copied().ok_or(JsonError::Eof)
    }

    fn expect(&mut self, byte: u8) -> Result<(), JsonError> {
        if self.peek()? == byte {
            self.pos += 1;
            Ok(())
        } else {
            Err(JsonError::Unexpected(self.pos))
        }
    }

    fn literal(&mut self, word: &str, value: Json) -> Result<Json, JsonError> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(JsonError::Unexpected(self.pos))
        }
    }

    fn value(&mut self) -> Result<Json, JsonError> {
        self.value_at(0)
    }

    fn value_at(&mut self, depth: usize) -> Result<Json, JsonError> {
        if depth > MAX_DEPTH {
            return Err(JsonError::TooDeep(self.pos));
        }
        match self.peek()? {
            b'n' => self.literal("null", Json::Null),
            b't' => self.literal("true", Json::Bool(true)),
            b'f' => self.literal("false", Json::Bool(false)),
            b'"' => Ok(Json::Str(self.string()?)),
            b'[' => self.array(depth),
            b'{' => self.object(depth),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(JsonError::Unexpected(self.pos)),
        }
    }

    fn array(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.expect(b'[')?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek()? == b']' {
            self.pos += 1;
            return Ok(Json::Arr(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value_at(depth + 1)?);
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b']' => {
                    self.pos += 1;
                    return Ok(Json::Arr(items));
                }
                _ => return Err(JsonError::Unexpected(self.pos)),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Json, JsonError> {
        self.expect(b'{')?;
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.peek()? == b'}' {
            self.pos += 1;
            return Ok(Json::Obj(map));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            self.expect(b':')?;
            self.skip_ws();
            let value = self.value_at(depth + 1)?;
            map.insert(key, value);
            self.skip_ws();
            match self.peek()? {
                b',' => self.pos += 1,
                b'}' => {
                    self.pos += 1;
                    return Ok(Json::Obj(map));
                }
                _ => return Err(JsonError::Unexpected(self.pos)),
            }
        }
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.expect(b'"')?;
        let mut out = String::new();
        loop {
            let byte = self.peek()?;
            match byte {
                b'"' => {
                    self.pos += 1;
                    return Ok(out);
                }
                b'\\' => {
                    self.pos += 1;
                    let escape = self.peek()?;
                    self.pos += 1;
                    match escape {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => return Err(JsonError::BadEscape(self.pos - 1)),
                    }
                }
                _ => {
                    // Copy the whole UTF-8 sequence. `self.bytes` came from a `&str`, so it is
                    // valid UTF-8 and the boundary walk cannot desynchronise.
                    let start = self.pos;
                    let width = utf8_width(byte);
                    if start + width > self.bytes.len() {
                        return Err(JsonError::Eof);
                    }
                    self.pos += width;
                    match core::str::from_utf8(&self.bytes[start..self.pos]) {
                        Ok(chunk) => out.push_str(chunk),
                        Err(_) => return Err(JsonError::Unexpected(start)),
                    }
                }
            }
        }
    }

    fn unicode_escape(&mut self) -> Result<char, JsonError> {
        let at = self.pos;
        let first = self.hex4(at)?;
        // A surrogate pair is two escapes and must be recombined; a lone surrogate is not a
        // character and is refused rather than replaced, because silently substituting U+FFFD
        // would change a digest.
        if (0xd800..0xdc00).contains(&first) {
            if !self.bytes[self.pos..].starts_with(b"\\u") {
                return Err(JsonError::BadEscape(at));
            }
            self.pos += 2;
            let second = self.hex4(at)?;
            if !(0xdc00..0xe000).contains(&second) {
                return Err(JsonError::BadEscape(at));
            }
            let combined = 0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
            return char::from_u32(combined).ok_or(JsonError::BadEscape(at));
        }
        char::from_u32(first).ok_or(JsonError::BadEscape(at))
    }

    fn hex4(&mut self, report_at: usize) -> Result<u32, JsonError> {
        if self.pos + 4 > self.bytes.len() {
            return Err(JsonError::Eof);
        }
        let mut value = 0u32;
        for _ in 0..4 {
            let byte = self.bytes[self.pos];
            let nibble = match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => return Err(JsonError::BadEscape(report_at)),
            };
            value = (value << 4) | nibble as u32;
            self.pos += 1;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Json, JsonError> {
        let start = self.pos;
        if self.peek()? == b'-' {
            self.pos += 1;
        }
        self.int_digits()?;
        if self.bytes.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            self.digits()?;
        }
        if matches!(self.bytes.get(self.pos), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.bytes.get(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            self.digits()?;
        }
        let text = core::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| JsonError::Unexpected(start))?;
        let value: f64 = text.parse().map_err(|_| JsonError::Unexpected(start))?;
        if !value.is_finite() {
            // A literal so large it overflows to infinity is a corrupt corpus file, not a value.
            return Err(JsonError::Unexpected(start));
        }
        Ok(Json::Num(value))
    }

    /// The integer part of a number: either a lone `0`, or a non-zero digit and its followers.
    ///
    /// `01` is not JSON, and accepting it would be a place two parsers can disagree about what a
    /// corpus file says.
    fn int_digits(&mut self) -> Result<(), JsonError> {
        let start = self.pos;
        self.digits()?;
        if self.bytes[start] == b'0' && self.pos - start > 1 {
            return Err(JsonError::Unexpected(start));
        }
        Ok(())
    }

    fn digits(&mut self) -> Result<(), JsonError> {
        let start = self.pos;
        while matches!(self.bytes.get(self.pos), Some(b'0'..=b'9')) {
            self.pos += 1;
        }
        if self.pos == start {
            Err(JsonError::Unexpected(self.pos))
        } else {
            Ok(())
        }
    }
}

fn utf8_width(first: u8) -> usize {
    match first {
        0x00..=0x7f => 1,
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn integers_have_no_fraction() {
        assert_eq!(format_number(1.0).unwrap(), "1");
        assert_eq!(format_number(-80.0).unwrap(), "-80");
        assert_eq!(format_number(0.0).unwrap(), "0");
        // Negative zero is the same quantity as zero and gets one spelling.
        assert_eq!(format_number(-0.0).unwrap(), "0");
    }

    #[test]
    fn fractions_are_shortest_round_trip() {
        assert_eq!(format_number(0.5).unwrap(), "0.5");
        assert_eq!(format_number(0.1).unwrap(), "0.1");
        // 80/180, the scale a policy peaking at 180 Nm gets on an 80 Nm joint.
        assert_eq!(
            format_number(80.0f64 / 180.0).unwrap(),
            "0.4444444444444444"
        );
        assert_eq!(format_number(1000.0 / 3.0).unwrap(), "333.3333333333333");
    }

    #[test]
    fn small_magnitudes_stay_positional() {
        assert_eq!(format_number(1e-7).unwrap(), "0.0000001");
        assert_eq!(format_number(1.5e-9).unwrap(), "0.0000000015");
    }

    #[test]
    fn large_magnitudes_stay_positional() {
        assert_eq!(format_number(1e21).unwrap(), "1000000000000000000000");
    }

    #[test]
    fn every_formatted_number_round_trips() {
        let cases = [
            0.1,
            0.2,
            0.3,
            1.0 / 3.0,
            2.0 / 3.0,
            1e-7,
            1e21,
            6.283185307179999, // near TAU but deliberately not it: this is a round-trip case
            -0.4444444444444444,
            1.7976931348623157e308,
            5e-324,
            123.456,
            0.999,
        ];
        for case in cases {
            let text = format_number(case).unwrap();
            let back: f64 = text.parse().unwrap();
            assert_eq!(
                back.to_bits(),
                case.to_bits(),
                "round trip failed for {text}"
            );
        }
    }

    #[test]
    fn non_finite_is_an_error_not_a_null() {
        assert_eq!(format_number(f64::NAN), Err(JsonError::NonFinite));
        assert_eq!(format_number(f64::INFINITY), Err(JsonError::NonFinite));
    }

    #[test]
    fn object_keys_are_sorted() {
        let mut value = Json::obj();
        value.set("zebra", 1.0.into());
        value.set("alpha", 2.0.into());
        value.set("Beta", 3.0.into());
        // Byte order, so uppercase sorts before lowercase.
        assert_eq!(
            value.to_canonical().unwrap(),
            r#"{"Beta":3,"alpha":2,"zebra":1}"#
        );
    }

    #[test]
    fn absent_is_not_null() {
        let mut value = Json::obj();
        value.set_opt("present", Some(1.0.into()));
        value.set_opt("absent", None);
        assert_eq!(value.to_canonical().unwrap(), r#"{"present":1}"#);
    }

    #[test]
    fn strings_escape_the_minimum() {
        let value = Json::Str("a\"b\\c\nd\te/f\u{1}g—h".into());
        assert_eq!(
            value.to_canonical().unwrap(),
            "\"a\\\"b\\\\c\\nd\\te/f\\u0001g—h\""
        );
    }

    #[test]
    fn parses_and_reserialises_canonically() {
        let text = r#" { "b" : [ 1 , 2.50 , null ] , "a" : { "x" : true } } "#;
        let value = Json::parse(text).unwrap();
        assert_eq!(
            value.to_canonical().unwrap(),
            r#"{"a":{"x":true},"b":[1,2.5,null]}"#
        );
    }

    #[test]
    fn canonical_output_is_a_fixed_point() {
        let text = r#"{"scale":0.4444444444444444,"warnings":["scaled_down"],"limit":80}"#;
        let once = Json::parse(text).unwrap().to_canonical().unwrap();
        let twice = Json::parse(&once).unwrap().to_canonical().unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn parser_is_strict() {
        assert!(Json::parse("{\"a\":1,}").is_err(), "trailing comma");
        assert!(Json::parse("[1 2]").is_err(), "missing comma");
        assert!(Json::parse("+1").is_err(), "leading plus");
        assert!(Json::parse("01").is_err(), "leading zero is trailing input");
        assert!(Json::parse("{a:1}").is_err(), "unquoted key");
        assert!(Json::parse("// c\n1").is_err(), "comment");
        assert!(Json::parse("1e999").is_err(), "overflows to infinity");
    }

    #[test]
    fn surrogate_pairs_recombine() {
        let value = Json::parse(r#""😀""#).unwrap();
        assert_eq!(value.as_str(), Some("\u{1f600}"));
        assert!(Json::parse(r#""\ud83d""#).is_err(), "lone surrogate");
    }

    #[test]
    fn nesting_is_bounded() {
        let deep = "[".repeat(MAX_DEPTH + 5) + &"]".repeat(MAX_DEPTH + 5);
        assert!(matches!(Json::parse(&deep), Err(JsonError::TooDeep(_))));
    }

    #[test]
    fn get_present_treats_explicit_null_as_absent() {
        let value = Json::parse(r#"{"envelope":null,"dof":6}"#).unwrap();
        assert!(value.get_present("envelope").is_none());
        assert!(value.get_present("dof").is_some());
        // `get` still distinguishes them, for a caller that needs to.
        assert_eq!(value.get("envelope"), Some(&Json::Null));
    }

    #[test]
    fn arrays_keep_their_order() {
        let value = Json::Arr(vec![Json::from("z"), Json::from("a")]);
        assert_eq!(value.to_canonical().unwrap(), r#"["z","a"]"#);
    }
}

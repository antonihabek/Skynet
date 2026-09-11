//! Version parsing, comparison and range satisfaction.
//!
//! The Rust edition of `reference/typescript/semver.ts`, and it arrived for one reason: §54.2 makes
//! `semver_satisfies` an operator the **robot** evaluates, before a skill is allowed to start. A
//! robot that reads `^1.2.0` differently from the Hub that sold it the skill is the §31.2 failure —
//! *a format with two implementations of its own definition is not a format* — so range
//! satisfaction is a contract, and this is the second implementation of it, held to the first by
//! `corpus/semver/`.
//!
//! # Mirroring JavaScript on purpose
//!
//! The reference is JavaScript, so a few of its behaviours are not what a Rust author would write
//! and are reproduced deliberately. Each is marked at the point it happens:
//!
//! * numeric components are `f64`, because the reference reads them with `Number()`, and a version
//!   with twenty digits of major must round the same way in both editions rather than failing to
//!   parse in one of them;
//! * [`compare_versions`] answers `0` — *equal* — when either side does not parse, because the
//!   reference does, and a comparator that returned an error here would make the two editions order
//!   a malformed list differently;
//! * trimming and `\s` use **JavaScript's** whitespace set, not `char::is_whitespace`, which
//!   differs on `U+FEFF` and would otherwise be a divergence nobody would find for years. This
//!   module is where that rule was first written down; it now lives in [`crate::js`], because every
//!   contract trims and one module applying it is how ten others came not to;
//! * the range tokenizer is a scan, not an anchored parse, so `v1.2.3` yields the token `1.2.3` and
//!   `1.2.3.4` yields `1.2.3`. That is what the reference's global regex does.
//!
//! # What is not here
//!
//! `sortVersionsDesc` and `nextVersion` are in the reference and not in this crate. They are the
//! Hub's presentation helpers — ordering a version list on a page, offering the next number in a
//! publish form — and nothing on a robot sorts versions or invents one. Porting them would add two
//! functions no edition-level consumer calls and two more things to keep in step. If a robot-side
//! caller ever appears, they come across with a corpus.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

use crate::js::{is_js_space, js_trim};

/// A version, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedVersion {
    /// Major component. `f64` to mirror the reference's `Number()`; see the module note.
    pub major: f64,
    /// Minor component.
    pub minor: f64,
    /// Patch component.
    pub patch: f64,
    /// The prerelease tag, without its leading `-`.
    pub prerelease: Option<String>,
    /// The input, trimmed.
    pub raw: String,
}

/// Parse `major.minor.patch[-prerelease]`, or `None`.
///
/// The reference's regex is `^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$` over the trimmed input,
/// and `\d` in a JavaScript regex without the `u` flag is ASCII `[0-9]` — so a version written in
/// Devanagari digits does not parse here either.
pub fn parse_version(input: &str) -> Option<ParsedVersion> {
    let trimmed = js_trim(input);
    let bytes = trimmed.as_bytes();
    let mut at = 0usize;

    let component = |at: &mut usize| -> Option<f64> {
        let start = *at;
        while *at < bytes.len() && bytes[*at].is_ascii_digit() {
            *at += 1;
        }
        if *at == start {
            return None;
        }
        // Every byte in the run is ASCII, so this slice is a valid `str` and parses as JavaScript's
        // `Number()` would: both are correctly-rounded decimal-to-double conversions.
        core::str::from_utf8(&bytes[start..*at]).ok()?.parse().ok()
    };

    let major = component(&mut at)?;
    if at >= bytes.len() || bytes[at] != b'.' {
        return None;
    }
    at += 1;
    let minor = component(&mut at)?;
    if at >= bytes.len() || bytes[at] != b'.' {
        return None;
    }
    at += 1;
    let patch = component(&mut at)?;

    let prerelease = if at == bytes.len() {
        None
    } else if bytes[at] == b'-' {
        let tag = &bytes[at + 1..];
        if tag.is_empty() || !tag.iter().all(is_prerelease_byte) {
            return None;
        }
        Some(core::str::from_utf8(tag).ok()?.to_string())
    } else {
        return None;
    };

    Some(ParsedVersion {
        major,
        minor,
        patch,
        prerelease,
        raw: trimmed.to_string(),
    })
}

fn is_prerelease_byte(byte: &u8) -> bool {
    byte.is_ascii_alphanumeric() || *byte == b'.' || *byte == b'-'
}

/// Is this a version this edition can read?
pub fn is_valid_version(input: &str) -> bool {
    parse_version(input).is_some()
}

/// Compare prerelease identifiers per semver §11: numeric before alphanumeric, longer wins ties.
fn compare_prerelease(a: Option<&str>, b: Option<&str>) -> i32 {
    if a == b {
        return 0;
    }
    // A version without a prerelease outranks one with it.
    let (a, b) = match (a, b) {
        (None, _) => return 1,
        (_, None) => return -1,
        (Some(a), Some(b)) => (a, b),
    };

    let left: Vec<&str> = a.split('.').collect();
    let right: Vec<&str> = b.split('.').collect();
    let length = left.len().max(right.len());

    for i in 0..length {
        let (ai, bi) = match (left.get(i), right.get(i)) {
            (None, _) => return -1,
            (_, None) => return 1,
            (Some(ai), Some(bi)) => (*ai, *bi),
        };
        let a_num = !ai.is_empty() && ai.bytes().all(|b| b.is_ascii_digit());
        let b_num = !bi.is_empty() && bi.bytes().all(|b| b.is_ascii_digit());
        if a_num && b_num {
            let an: f64 = ai.parse().unwrap_or(0.0);
            let bn: f64 = bi.parse().unwrap_or(0.0);
            if an != bn {
                return if an < bn { -1 } else { 1 };
            }
            continue;
        }
        if a_num {
            return -1;
        }
        if b_num {
            return 1;
        }
        if ai != bi {
            // Both sides are ASCII by the prerelease charset, so byte order is code-unit order.
            return if ai < bi { -1 } else { 1 };
        }
    }
    0
}

/// `-1` if `a < b`, `0` if equal, `1` if `a > b`.
pub fn compare_versions(a: &ParsedVersion, b: &ParsedVersion) -> i32 {
    if a.major != b.major {
        return if a.major < b.major { -1 } else { 1 };
    }
    if a.minor != b.minor {
        return if a.minor < b.minor { -1 } else { 1 };
    }
    if a.patch != b.patch {
        return if a.patch < b.patch { -1 } else { 1 };
    }
    compare_prerelease(a.prerelease.as_deref(), b.prerelease.as_deref())
}

/// [`compare_versions`] over two strings.
///
/// **Unparseable means equal**, which is the reference's `if (!pa || !pb) return 0`. It is not what
/// this function would say if it were being designed today, and it is reproduced because a sort
/// that ordered a malformed list differently in two editions is a worse thing than a comparator
/// with a documented wart.
pub fn compare_version_strings(a: &str, b: &str) -> i32 {
    match (parse_version(a), parse_version(b)) {
        (Some(a), Some(b)) => compare_versions(&a, &b),
        _ => 0,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Gte,
    Gt,
    Lte,
    Lt,
    Eq,
    Caret,
    Tilde,
}

/// The operators, in the order the reference's alternation tries them. Longest first, so `>=` is
/// never read as `>` followed by a stray `=`.
const OPS: [(&str, Op); 7] = [
    (">=", Op::Gte),
    ("<=", Op::Lte),
    (">", Op::Gt),
    ("<", Op::Lt),
    ("^", Op::Caret),
    ("~", Op::Tilde),
    ("=", Op::Eq),
];

struct Comparator {
    op: Op,
    version: ParsedVersion,
}

fn satisfies_comparator(target: &ParsedVersion, comparator: &Comparator) -> bool {
    let cmp = compare_versions(target, &comparator.version);
    match comparator.op {
        Op::Gte => cmp >= 0,
        Op::Gt => cmp > 0,
        Op::Lte => cmp <= 0,
        Op::Lt => cmp < 0,
        Op::Eq => cmp == 0,
        // Caret: compatible within the leftmost non-zero component.
        Op::Caret => {
            if cmp < 0 {
                return false;
            }
            let base = &comparator.version;
            if base.major > 0.0 {
                target.major == base.major
            } else if base.minor > 0.0 {
                target.major == 0.0 && target.minor == base.minor
            } else {
                target.major == 0.0 && target.minor == 0.0 && target.patch == base.patch
            }
        }
        // Tilde: patch-level changes within the same minor.
        Op::Tilde => {
            if cmp < 0 {
                return false;
            }
            target.major == comparator.version.major && target.minor == comparator.version.minor
        }
    }
}

/// One `||` alternative, tokenized.
///
/// `None` means the alternative held no comparator at all — the reference's `tokens === null`. It
/// is not an error for the whole range: one unreadable alternative does not invalidate the readable
/// ones, which is what `satisfies` did before the two answers were separated.
fn scan_group(group: &[char]) -> Option<Vec<Comparator>> {
    let mut found = Vec::new();
    let mut at = 0usize;
    while at < group.len() {
        match match_comparator(group, at) {
            Some((comparator, end)) => {
                found.push(comparator);
                at = end;
            }
            // The global regex advances one position and tries again.
            None => at += 1,
        }
    }
    if found.is_empty() {
        None
    } else {
        Some(found)
    }
}

/// `(?:>=|<=|>|<|\^|~|=)?\s*\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?`, anchored at `start`.
///
/// The optional operator is an ordered alternation, so an operator that matches but leaves the rest
/// unmatchable must be retried as a shorter one and finally as nothing — which is why this is a
/// loop over [`OPS`] with the empty alternative last, rather than a single greedy take.
fn match_comparator(chars: &[char], start: usize) -> Option<(Comparator, usize)> {
    let mut alternatives: Vec<(usize, Op)> = Vec::with_capacity(OPS.len() + 1);
    for (text, op) in OPS {
        let symbol: Vec<char> = text.chars().collect();
        if chars.len() >= start + symbol.len() && chars[start..start + symbol.len()] == symbol[..] {
            alternatives.push((symbol.len(), op));
        }
    }
    // The empty alternative: no operator at all, which the reference spells `=`.
    alternatives.push((0, Op::Eq));

    for (width, op) in alternatives {
        let mut at = start + width;
        while at < chars.len() && is_js_space(chars[at]) {
            at += 1;
        }
        if let Some((version, end)) = match_version(chars, at) {
            return Some((Comparator { op, version }, end));
        }
    }
    None
}

/// `\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?`, anchored, returning the parsed version and where it ended.
fn match_version(chars: &[char], start: usize) -> Option<(ParsedVersion, usize)> {
    let mut at = start;
    let digits = |at: &mut usize| -> Option<f64> {
        let from = *at;
        while *at < chars.len() && chars[*at].is_ascii_digit() {
            *at += 1;
        }
        if *at == from {
            return None;
        }
        let text: String = chars[from..*at].iter().collect();
        text.parse().ok()
    };

    let major = digits(&mut at)?;
    if at >= chars.len() || chars[at] != '.' {
        return None;
    }
    at += 1;
    let minor = digits(&mut at)?;
    if at >= chars.len() || chars[at] != '.' {
        return None;
    }
    at += 1;
    let patch = digits(&mut at)?;

    let mut prerelease = None;
    if at < chars.len() && chars[at] == '-' {
        let from = at + 1;
        let mut to = from;
        while to < chars.len() && is_prerelease_char(chars[to]) {
            to += 1;
        }
        // `+` in the regex: an empty tag means the optional group matches nothing and the version
        // ends before the hyphen.
        if to > from {
            prerelease = Some(chars[from..to].iter().collect::<String>());
            at = to;
        }
    }

    let raw: String = chars[start..at].iter().collect();
    Some((
        ParsedVersion {
            major,
            minor,
            patch,
            prerelease,
            raw,
        },
        at,
    ))
}

fn is_prerelease_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '-'
}

/// A range, split on `||` and tokenized. `None` for the wildcards, which constrain nothing.
fn parse_range(range: &str) -> Option<Vec<Option<Vec<Comparator>>>> {
    let trimmed = js_trim(range);
    if trimmed.is_empty() || trimmed == "*" || trimmed.eq_ignore_ascii_case("any") {
        return None;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    let mut groups = Vec::new();
    let mut group = Vec::new();
    let mut at = 0usize;
    while at < chars.len() {
        if chars[at] == '|' && at + 1 < chars.len() && chars[at + 1] == '|' {
            groups.push(scan_group(&group));
            group.clear();
            at += 2;
            continue;
        }
        group.push(chars[at]);
        at += 1;
    }
    groups.push(scan_group(&group));
    Some(groups)
}

/// Does `version` satisfy `range`?
///
/// Supports `*`, space-separated AND comparators (`>=1.2.0 <2.0.0`) and `||` OR groups.
pub fn satisfies(version: &str, range: &str) -> bool {
    let Some(target) = parse_version(version) else {
        return false;
    };
    let Some(groups) = parse_range(range) else {
        return true;
    };
    groups.iter().any(|group| {
        group
            .as_ref()
            .is_some_and(|comparators| comparators.iter().all(|c| satisfies_comparator(&target, c)))
    })
}

/// Is `range` a range this edition can read at all?
///
/// [`satisfies`] answers false both for "this version is outside that window" and for "that is not
/// a window", and §54.2 needs those apart: a skill whose precondition names an unreadable range is
/// a skill nobody can ever run here, and its publisher is the one who has to hear about it.
pub fn is_valid_range(range: &str) -> bool {
    match parse_range(range) {
        None => true,
        Some(groups) => groups.iter().all(Option::is_some),
    }
}

/// The wire shape of a parsed version, `null` when it did not parse.
pub fn version_to_json(parsed: &Option<ParsedVersion>) -> Json {
    match parsed {
        None => Json::Null,
        Some(version) => {
            let mut out = Json::obj();
            out.set("major", Json::Num(version.major))
                .set("minor", Json::Num(version.minor))
                .set("patch", Json::Num(version.patch))
                .set(
                    "prerelease",
                    match &version.prerelease {
                        Some(tag) => Json::Str(tag.clone()),
                        None => Json::Null,
                    },
                )
                .set("raw", Json::Str(version.raw.clone()));
            out
        }
    }
}

/// Range satisfaction with **validity beside it**.
///
/// Pinned together because the two answers are different facts: `satisfies` is false both for "this
/// version is outside that window" and for "that is not a window", and §54.2 needs them apart.
pub fn satisfies_to_json(version: &str, range: &str) -> Json {
    let mut out = Json::obj();
    out.set("version", Json::Str(version.to_string()))
        .set("range", Json::Str(range.to_string()))
        .set("satisfies", Json::Bool(satisfies(version, range)))
        .set("valid_version", Json::Bool(is_valid_version(version)))
        .set("valid_range", Json::Bool(is_valid_range(range)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn parses_the_three_components_and_a_prerelease() {
        let parsed = parse_version("1.2.3-rc.1").unwrap();
        assert_eq!(parsed.major, 1.0);
        assert_eq!(parsed.minor, 2.0);
        assert_eq!(parsed.patch, 3.0);
        assert_eq!(parsed.prerelease.as_deref(), Some("rc.1"));
        assert_eq!(parsed.raw, "1.2.3-rc.1".to_string());
    }

    #[test]
    fn refuses_what_the_reference_regex_refuses() {
        for input in [
            "1.2",
            "1.2.3.4",
            "v1.2.3",
            "1.2.3-",
            "1.2.3+build",
            "",
            "abc",
            "1.2.x",
        ] {
            assert!(parse_version(input).is_none(), "{input} should not parse");
        }
    }

    /// `\d` in a JavaScript regex without the `u` flag is ASCII. An edition that used
    /// `char::is_numeric` here would accept versions the Hub rejects.
    #[test]
    fn digits_are_ascii_only() {
        assert!(parse_version("١.٢.٣").is_none());
    }

    /// JavaScript's whitespace set, not Rust's. `U+FEFF` is the one that differs in the direction
    /// that matters: `trim()` removes it and `char::is_whitespace` does not.
    #[test]
    fn trims_javascripts_whitespace() {
        assert!(parse_version("\u{FEFF}1.2.3\u{FEFF}").is_some());
        assert!(parse_version("\u{00A0}1.2.3").is_some());
        assert!(parse_version("\t 1.2.3 \n").is_some());
    }

    #[test]
    fn orders_by_component_then_prerelease() {
        assert_eq!(compare_version_strings("1.2.3", "1.2.3"), 0);
        assert_eq!(compare_version_strings("1.2.0", "1.10.0"), -1);
        assert_eq!(compare_version_strings("2.0.0", "1.9.9"), 1);
        // A release outranks its own prerelease.
        assert_eq!(compare_version_strings("1.0.0", "1.0.0-rc.1"), 1);
        // Numeric identifiers compare as numbers, not as strings.
        assert_eq!(compare_version_strings("1.0.0-2", "1.0.0-10"), -1);
        // A numeric identifier is below an alphanumeric one.
        assert_eq!(compare_version_strings("1.0.0-1", "1.0.0-alpha"), -1);
        // A longer identifier list wins a tie on the shared prefix.
        assert_eq!(compare_version_strings("1.0.0-alpha.1", "1.0.0-alpha"), 1);
    }

    /// Reproduced from the reference, and wrong if it were being designed now. See the note on
    /// [`compare_version_strings`].
    #[test]
    fn unparseable_compares_equal() {
        assert_eq!(compare_version_strings("nonsense", "1.0.0"), 0);
        assert_eq!(compare_version_strings("", ""), 0);
    }

    #[test]
    fn the_wildcards_constrain_nothing() {
        for range in ["*", "any", "ANY", "", "   "] {
            assert!(satisfies("9.9.9", range), "{range} should admit everything");
            assert!(is_valid_range(range));
        }
    }

    #[test]
    fn caret_holds_the_leftmost_non_zero_component() {
        assert!(satisfies("1.9.0", "^1.2.0"));
        assert!(!satisfies("2.0.0", "^1.2.0"));
        assert!(!satisfies("1.1.0", "^1.2.0"));
        assert!(satisfies("0.2.9", "^0.2.0"));
        assert!(!satisfies("0.3.0", "^0.2.0"));
        assert!(satisfies("0.0.5", "^0.0.5"));
        assert!(!satisfies("0.0.6", "^0.0.5"));
    }

    #[test]
    fn conjunction_and_disjunction() {
        assert!(satisfies("1.5.0", ">=1.0.0 <2.0.0"));
        assert!(!satisfies("2.1.0", ">=1.0.0 <2.0.0"));
        assert!(satisfies("3.1.0", "<2.0.0 || >=3.0.0"));
        assert!(!satisfies("2.5.0", "<2.0.0 || >=3.0.0"));
    }

    /// The tokenizer is the reference's global regex, and these are the four shapes where a
    /// hand-written parser would have drifted from it.
    #[test]
    fn the_tokenizer_scans_rather_than_anchors() {
        // Whitespace between the operator and the version.
        assert!(satisfies("1.3.0", ">= 1.2.0"));
        // A leading `v` is skipped over, and the version behind it is the token.
        assert!(satisfies("1.2.3", "v1.2.3"));
        // A fourth segment is not part of the version.
        assert!(satisfies("1.2.3", "1.2.3.4"));
        // An operator that cannot start a match is retried as a shorter one, then as none.
        assert!(satisfies("1.5.0", "=>1.0.0"));
    }

    #[test]
    fn validity_is_a_separate_answer_from_satisfaction() {
        // Readable and not satisfied.
        assert!(!satisfies("1.2.3", ">=2.0.0"));
        assert!(is_valid_range(">=2.0.0"));
        // Unreadable, which `satisfies` reports the same way and `is_valid_range` does not.
        assert!(!satisfies("1.2.3", "garbage"));
        assert!(!is_valid_range("garbage"));
        // One alternative readable: satisfied, and still not wholly readable.
        assert!(satisfies("1.2.3", ">=1.0.0 || garbage"));
        assert!(!is_valid_range(">=1.0.0 || garbage"));
    }
}

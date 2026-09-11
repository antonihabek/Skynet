//! The places where the obvious Rust spelling of a reference expression computes something else.
//!
//! Every contract in this crate is transcribed from `reference/typescript/*`, and most of that
//! transcription is uneventful. This module is for the operations where it is not: a line that reads
//! the same in both languages, reviews clean, and answers differently on an input rare enough that
//! nothing notices until a robot and its Hub disagree. The Python and C++ editions each have a module
//! with this job, for the same reason (`skynet_os/js.py`, `include/skynet_os/js.hpp`).
//!
//! # `trim()` is not `str::trim`
//!
//! `String.prototype.trim` removes ECMAScript's `WhiteSpace` and `LineTerminator`. `str::trim`
//! removes Unicode `White_Space`. The two sets differ in exactly two characters, in opposite
//! directions:
//!
//! | | `trim()` | `str::trim` |
//! |---|---|---|
//! | U+FEFF byte-order mark | removed | kept |
//! | U+0085 next line | kept | removed |
//!
//! The rule was first written down in `semver.rs`, and applied there and in `predicate.rs` and
//! nowhere else. Ten other files trimmed with `str::trim` — a namespace, a device key, a digest, a
//! role, a cell's name — and the corpus had no input that could tell, so this edition disagreed with
//! the reference about whether a byte-order mark made a namespace blank for as long as the rule had
//! existed. The corpus now puts both characters at every trim site, and `clippy.toml` beside this
//! crate refuses `str::trim` outright, so the next file cannot make the same choice by reaching for
//! the method that is right there.
//!
//! # Case
//!
//! The reference lower-cases with `toLowerCase()`, which is a full Unicode mapping, and this crate
//! uses [`str::to_lowercase`], which is the same mapping. The ASCII methods are refused by the same
//! `clippy.toml` for the same reason as `str::trim`: `toLowerCase()` turns U+212A KELVIN SIGN into
//! the ASCII `k`, so an ASCII lower-case disagrees with the reference at any site that lower-cases
//! and then compares against a word with a `k` in it. The credential prefix `skd_` is one.

/// JavaScript's `WhiteSpace` and `LineTerminator`: what `trim()` removes and what `\s` matches.
///
/// Written out rather than delegated to [`char::is_whitespace`] because the two sets differ, and
/// the difference is the whole point of this module.
pub fn is_js_space(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// `String.prototype.trim`. Every trim in this crate is this one.
pub fn js_trim(input: &str) -> &str {
    input.trim_matches(is_js_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The byte-order mark is the character where JavaScript and Rust disagree in the direction a
    /// pasted document reaches: `trim()` removes it and `str::trim` does not.
    #[test]
    fn removes_what_javascript_removes() {
        assert_eq!(js_trim("\u{FEFF}1.2.3\u{FEFF}"), "1.2.3");
        assert_eq!(js_trim("\u{00A0}1.2.3"), "1.2.3");
        assert_eq!(js_trim("\t 1.2.3 \n"), "1.2.3");
        assert_eq!(js_trim("\u{3000}\u{2028}x\u{2029}\u{205F}"), "x");
    }

    /// And the other direction, which a trim that merely added U+FEFF to Rust's set would get
    /// wrong: U+0085 is whitespace to Rust and not to JavaScript.
    #[test]
    fn keeps_what_javascript_keeps() {
        assert_eq!(js_trim("\u{0085}x\u{0085}"), "\u{0085}x\u{0085}");
        assert_eq!(js_trim("\u{001C}x"), "\u{001C}x");
        assert_eq!(js_trim("\u{200B}x"), "\u{200B}x");
    }
}

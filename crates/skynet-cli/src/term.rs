//! Terminal output.
//!
//! Colour when stderr is a terminal and `NO_COLOR` is unset, and never in a pipe — a report
//! captured into a file with escape codes in it is a report somebody has to clean up. The check is
//! deliberately conservative: when in doubt, no colour.

use std::io::Write;
use std::sync::OnceLock;

fn coloured() -> bool {
    static COLOURED: OnceLock<bool> = OnceLock::new();
    *COLOURED.get_or_init(|| {
        if std::env::var_os("NO_COLOR").is_some() {
            return false;
        }
        // No `isatty` without libc, so this reads the environment instead. `TERM=dumb` and a
        // missing `TERM` both mean plain text, and CI systems that set neither get colour they can
        // handle. It is a heuristic, and being wrong costs a few escape codes rather than a bug.
        match std::env::var("TERM") {
            Ok(term) => !term.is_empty() && term != "dumb",
            // Windows terminals do not set TERM and do render ANSI. `WT_SESSION` is Windows
            // Terminal; anything else gets plain text rather than a guess.
            Err(_) => std::env::var_os("WT_SESSION").is_some(),
        }
    })
}

fn paint(code: &str, text: &str) -> String {
    if coloured() {
        format!("\x1b[{code}m{text}\x1b[0m")
    } else {
        text.to_string()
    }
}

/// Emphasis.
pub fn bold(text: &str) -> String {
    paint("1", text)
}

/// Secondary text.
pub fn dim(text: &str) -> String {
    paint("2", text)
}

/// Something that passed.
pub fn ok(text: &str) -> String {
    paint("32", text)
}

/// Something that wants a person.
pub fn warn(text: &str) -> String {
    paint("33", text)
}

/// Something that failed.
pub fn bad(text: &str) -> String {
    paint("31", text)
}

/// Wrap at 92 columns with a hanging indent, so a paragraph-length refusal is readable.
///
/// 92 rather than 80 because these messages carry identifiers — joint names, role strings, file
/// paths — that read worse broken than long.
pub fn wrap(text: &str, indent: usize) -> String {
    const WIDTH: usize = 92;
    let pad = " ".repeat(indent);
    let mut out = String::with_capacity(text.len() + 16);
    let mut column = indent;
    for (index, word) in text.split_whitespace().enumerate() {
        let width = word.chars().count();
        if index > 0 {
            if column + 1 + width > WIDTH {
                out.push('\n');
                out.push_str(&pad);
                column = indent;
            } else {
                out.push(' ');
                column += 1;
            }
        }
        out.push_str(word);
        column += width;
    }
    out
}

/// Print a refusal to stderr and give back the exit code to return.
///
/// Exit code 1 throughout: this CLI distinguishes "it worked" from "it did not", and a taxonomy of
/// exit codes nobody documents is a taxonomy nobody can rely on. `driver check` is the exception
/// and says so where it returns.
pub fn fail(message: &str) -> i32 {
    let mut err = std::io::stderr();
    // Only the first paragraph is reflowed. Everything after a blank line is pre-formatted — a
    // usage block, a command to copy — and wrapping it would run the columns together and turn a
    // pasteable line into two.
    let (complaint, verbatim) = match message.split_once("\n\n") {
        Some((complaint, rest)) => (complaint, Some(rest)),
        None => (message, None),
    };
    let _ = writeln!(err, "\n  {} {}", bad("error"), wrap(complaint, 4));
    if let Some(verbatim) = verbatim {
        let _ = writeln!(err, "\n{verbatim}");
    }
    let _ = writeln!(err);
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_keeps_a_hanging_indent_and_never_splits_a_word() {
        let text = "left_arm.shoulder.pitch is suggested for the joint named \
                    left_shoulder_pitch_joint because the name says so, and the axis was read from \
                    the name rather than the geometry.";
        let wrapped = wrap(text, 4);
        for (index, line) in wrapped.lines().enumerate() {
            assert!(
                line.chars().count() <= 92,
                "line {index} is too long: {line}"
            );
            if index > 0 {
                assert!(
                    line.starts_with("    "),
                    "line {index} lost its indent: {line}"
                );
            }
        }
        // Nothing is dropped.
        let joined: Vec<&str> = wrapped.split_whitespace().collect();
        let original: Vec<&str> = text.split_whitespace().collect();
        assert_eq!(joined, original);
    }

    #[test]
    fn a_single_long_token_is_left_alone() {
        let token = "a".repeat(120);
        assert_eq!(wrap(&token, 4), token);
    }
}

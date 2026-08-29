//! A very small argument reader.
//!
//! Hand-written for the workspace's stated reason — no third-party dependencies — and kept
//! deliberately dumb: long flags, `--flag value` or `--flag=value`, `--` to stop parsing, and
//! positionals in order. No abbreviation matching, no clustering, no inferred subcommands.
//!
//! The one property worth having: **an unrecognised flag is an error, not silence.** A typo'd
//! `--assume-sides right` that is quietly ignored produces a descriptor with no roles and no
//! explanation, which is the sort of failure that costs an afternoon.

use std::collections::BTreeMap;

/// A parsed command line.
#[derive(Debug, Default, Clone)]
pub struct Args {
    /// Positional arguments in order.
    pub positional: Vec<String>,
    values: BTreeMap<String, String>,
    flags: Vec<String>,
}

/// What a command accepts.
pub struct Spec {
    /// Flags that take a value.
    pub values: &'static [&'static str],
    /// Flags that do not.
    pub switches: &'static [&'static str],
}

impl Args {
    /// Parse, refusing anything the spec does not list.
    pub fn parse(input: &[String], spec: &Spec) -> Result<Args, String> {
        let mut out = Args::default();
        let mut index = 0;
        let mut only_positional = false;

        while index < input.len() {
            let arg = &input[index];
            index += 1;

            if arg == "--" {
                only_positional = true;
                continue;
            }
            // A single-dash argument is a flag only when what follows is a letter, so `-42` stays a
            // positional. Short flags are looked up in the same lists as long ones — a spec listing
            // `"o"` gets `-o` and `--o` alike, which is one rule rather than a second table to keep
            // in step with the first.
            let short = !arg.starts_with("--")
                && arg.len() > 1
                && arg.starts_with('-')
                && arg[1..2].chars().all(char::is_alphabetic);
            if only_positional || !(arg.starts_with("--") || short) {
                out.positional.push(arg.clone());
                continue;
            }

            let (name, inline) = match arg.split_once('=') {
                Some((name, value)) => (name.to_string(), Some(value.to_string())),
                None => (arg.clone(), None),
            };
            let bare = name.trim_start_matches('-').to_string();

            if spec.switches.contains(&bare.as_str()) {
                if inline.is_some() {
                    return Err(format!("`--{bare}` takes no value."));
                }
                out.flags.push(bare);
                continue;
            }
            if spec.values.contains(&bare.as_str()) {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        let next = input.get(index).ok_or_else(|| {
                            format!("`--{bare}` needs a value, and the command line ends here.")
                        })?;
                        index += 1;
                        next.clone()
                    }
                };
                out.values.insert(bare, value);
                continue;
            }

            return Err(format!(
                "`--{bare}` is not an option here. This command accepts: {}.",
                known(spec)
            ));
        }

        Ok(out)
    }

    /// The value of a flag, when it was given.
    pub fn value(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    /// Whether a switch was given.
    pub fn flag(&self, name: &str) -> bool {
        self.flags.iter().any(|flag| flag == name)
    }

    /// A comma-separated value, split and trimmed, empty entries dropped.
    pub fn list(&self, name: &str) -> Vec<String> {
        self.value(name)
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|part| !part.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn known(spec: &Spec) -> String {
    let mut all: Vec<String> = spec
        .values
        .iter()
        .map(|name| format!("--{name} <value>"))
        .chain(spec.switches.iter().map(|name| format!("--{name}")))
        .collect();
    all.sort();
    all.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEC: Spec = Spec {
        values: &["slug", "base"],
        switches: &["verbose"],
    };

    fn args(parts: &[&str]) -> Result<Args, String> {
        let owned: Vec<String> = parts.iter().map(|part| part.to_string()).collect();
        Args::parse(&owned, &SPEC)
    }

    #[test]
    fn both_spellings_of_a_value_flag_work() {
        let parsed = args(&["--slug", "g1", "--base=legged", "robot.urdf"]).unwrap();
        assert_eq!(parsed.value("slug"), Some("g1"));
        assert_eq!(parsed.value("base"), Some("legged"));
        assert_eq!(parsed.positional, ["robot.urdf"]);
    }

    /// The property this parser exists for. A silently ignored typo produces a descriptor with no
    /// roles and no explanation of why.
    #[test]
    fn an_unknown_flag_is_refused_and_the_message_lists_the_real_ones() {
        let err = args(&["--slugg", "x"]).unwrap_err();
        assert!(err.contains("`--slugg` is not an option"), "{err}");
        assert!(err.contains("--slug <value>"), "{err}");
        assert!(err.contains("--verbose"), "{err}");
    }

    #[test]
    fn a_value_flag_with_nothing_after_it_says_so() {
        let err = args(&["--slug"]).unwrap_err();
        assert!(err.contains("needs a value"), "{err}");
    }

    #[test]
    fn a_switch_given_a_value_is_refused() {
        assert!(args(&["--verbose=1"])
            .unwrap_err()
            .contains("takes no value"));
    }

    #[test]
    fn double_dash_stops_flag_parsing() {
        let parsed = args(&["--verbose", "--", "--not-a-flag"]).unwrap();
        assert!(parsed.flag("verbose"));
        assert_eq!(parsed.positional, ["--not-a-flag"]);
    }

    #[test]
    fn lists_split_and_trim() {
        let parsed = args(&["--slug", " position , velocity ,"]).unwrap();
        assert_eq!(parsed.list("slug"), ["position", "velocity"]);
        assert!(parsed.list("base").is_empty());
    }
}

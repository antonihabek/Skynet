//! §38.1's other half: the configuration that is deliberately **not** signed.
//!
//! > *"Non-safety configuration — log level, telemetry sampling, console theme — is unsigned and
//! > local. The line between the two is drawn by whether the value can reach the kernel."*
//!
//! Every other configuration path on this robot is signed (§38.1), delivered through desired state,
//! and is a decision somebody stood behind. These three are none of those things, and that is the
//! point: an engineer standing at a bench wanting the daemon to say more should not have to hold a
//! signing key, and a signing ceremony to change a page's colours would mean nobody ever changed it.
//!
//! # The line, and how it is enforced
//!
//! §38.1's sentence is a rule about *which* values may live here, and a rule stated in prose is a
//! rule that drifts. So this store holds a **closed set** — [`Setting`] is an enum with three
//! variants and there is no way to write a fourth key into the file. That is the whole enforcement:
//! not a list of forbidden names, which would have to be kept in step with a kernel that grows, but
//! an allowlist of three that cannot silently acquire a fourth.
//!
//! [`Setting::of`] does also recognise the kernel's own ceiling names, and that is **a courtesy and
//! not a safety property**. Somebody who types `joint_velocity_max_rad_s` here deserves the sentence
//! that says where such a value does live (a signed profile, §38.1; a session override, §38.2)
//! rather than a bare *unknown setting*. If that little list ever falls behind the kernel, the key
//! is still refused — it is simply refused with the duller sentence.
//!
//! # Everything here is read by something
//!
//! A settings store whose values nothing consults is a page of controls that lie, which §32.2
//! forbids in the plainest terms it uses anywhere. So each of the three is wired:
//!
//! * [`Setting::LogLevel`] decides which of this daemon's own messages are printed
//!   ([`crate::log`]), and it is consulted through a process-global so a log call costs no
//!   filesystem access.
//! * [`Setting::TelemetrySampling`] decides how many reconcile passes one §74 window covers. It
//!   makes windows **longer**, never sparser: a window carries its own `from_ms`, `to_ms` and
//!   `elapsed_ms`, so a four-pass window is self-describing, whereas dropping three windows in four
//!   would be §47.4's *"gap that looks like a quiet period"* manufactured on purpose.
//! * [`Setting::ConsoleTheme`] decides what the on-robot console (§32.2) renders as, for a bench
//!   whose lighting the workstation's `prefers-color-scheme` knows nothing about.
//!
//! # There is always a value
//!
//! [`read`] is total and returns [`Settings`] whatever is on the disk, because every one of these
//! has a default and a daemon that could not decide its own log level would have nothing to print
//! the complaint with. A file that cannot be parsed yields the defaults **and a note**, never
//! silence: the same asymmetry §38.2's store keeps, and for the same reason — somebody wrote that
//! file expecting it to do something.
//!
//! # Provenance, per §38.3
//!
//! Each entry carries `set_at_ms` and `set_by`, like §19's trust store, and an entry written by
//! hand without them reports **`unrecorded`** rather than a fabricated date or the invoking user's
//! name. §38.3 is explicit that provenance is recorded and never inferred.

use std::path::Path;

use skynet_json::Json;

use crate::layout::Layout;

/// The most reconcile passes one telemetry window may cover.
///
/// Sixty, which at the default ten-second period is a ten-minute window. The cap exists because
/// §47.4 reads a fleet's health off the *spacing* of windows: past some length a robot that has
/// stopped reporting and a robot that is merely sampling coarsely stop being distinguishable from
/// the Hub, and that is the one confusion §47.4 is written to prevent.
pub const MAX_SAMPLING: u32 = 60;

/// How much of its own working this daemon prints.
///
/// Four levels rather than a boolean, because the two questions a bench actually asks are *"say
/// less, this robot is in production"* and *"say everything, I am trying to see why it did that"*,
/// and one flag answers neither well.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Only what has gone wrong.
    Error,
    /// And what is about to.
    Warn,
    /// And what it is doing. The default.
    Info,
    /// And how it decided.
    Debug,
}

impl Level {
    /// The word for this level, as it is written in the file and on the wire.
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warn => "warn",
            Level::Info => "info",
            Level::Debug => "debug",
        }
    }

    /// The level for a word, or `None` for anything else.
    pub fn of(word: &str) -> Option<Level> {
        match word {
            "error" => Some(Level::Error),
            "warn" => Some(Level::Warn),
            "info" => Some(Level::Info),
            "debug" => Some(Level::Debug),
            _ => None,
        }
    }

    /// Every level, quietest first, for a refusal that lists what it would have accepted (§64).
    pub const ALL: [Level; 4] = [Level::Error, Level::Warn, Level::Info, Level::Debug];
}

/// What the on-robot console renders as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Theme {
    /// Whatever the browser asks for. The default, and the right one for a workstation.
    System,
    /// Pinned light, for a bench under shop lighting.
    Light,
    /// Pinned dark.
    Dark,
}

impl Theme {
    /// The word for this theme.
    pub fn as_str(self) -> &'static str {
        match self {
            Theme::System => "system",
            Theme::Light => "light",
            Theme::Dark => "dark",
        }
    }

    /// The theme for a word, or `None` for anything else.
    pub fn of(word: &str) -> Option<Theme> {
        match word {
            "system" => Some(Theme::System),
            "light" => Some(Theme::Light),
            "dark" => Some(Theme::Dark),
            _ => None,
        }
    }

    /// Every theme, for a refusal that lists what it would have accepted.
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Light, Theme::Dark];
}

/// The three settings that exist.
///
/// An enum and not a string, which is the enforcement §38.1's line needs: there is no code path
/// that writes a key this type does not name, so the store cannot grow a value that reaches the
/// kernel by anybody merely adding one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// How much of its own working this daemon prints.
    LogLevel,
    /// How many reconcile passes one §74 telemetry window covers.
    TelemetrySampling,
    /// What the on-robot console (§32.2) renders as.
    ConsoleTheme,
}

/// The names the kernel's own ceilings go by.
///
/// **This is for the sentence, not for the safety.** [`Setting::of`] refuses everything that is not
/// one of the three regardless; this list only lets it explain *where the value the operator typed
/// actually lives*, which is the difference between a refusal somebody can act on and one they
/// re-read. It may fall behind the kernel without consequence.
const KERNEL_CEILINGS: [&str; 6] = [
    "joint_velocity_max_rad_s",
    "joint_torque_max_nm",
    "base_velocity_max_m_s",
    "max_delta_per_cycle",
    "watchdog_ms",
    "rate_limit",
];

impl Setting {
    /// The key this setting is written under.
    pub fn as_str(self) -> &'static str {
        match self {
            Setting::LogLevel => "log_level",
            Setting::TelemetrySampling => "telemetry_sampling",
            Setting::ConsoleTheme => "console_theme",
        }
    }

    /// The setting for a key.
    ///
    /// `Err` carries the sentence for a key that is not one of the three, which names the fix (§64)
    /// and, for a kernel ceiling, names the two places such a value does belong.
    pub fn of(key: &str) -> Result<Setting, String> {
        match key {
            "log_level" => Ok(Setting::LogLevel),
            "telemetry_sampling" => Ok(Setting::TelemetrySampling),
            "console_theme" => Ok(Setting::ConsoleTheme),
            _ if KERNEL_CEILINGS.contains(&key) => Err(format!(
                "`{key}` is a limit the Safety Kernel enforces, and this store is unsigned. §38.1 \
                 draws the line at whether a value can reach the kernel: set it in a signed \
                 profile, or narrow it for an afternoon with a session override (§38.2)."
            )),
            _ => Err(format!(
                "`{key}` is not a setting this robot has. There are three, and they are unsigned \
                 because none of them reaches the Safety Kernel (§38.1): log_level, \
                 telemetry_sampling, console_theme."
            )),
        }
    }

    /// All three, for listing.
    pub const ALL: [Setting; 3] = [
        Setting::LogLevel,
        Setting::TelemetrySampling,
        Setting::ConsoleTheme,
    ];
}

/// What this robot is configured to do.
///
/// Always complete. There is no "unset" variant, because every one of these has a default and a
/// caller that had to handle absence would end up inventing one at each call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// How much of its own working this daemon prints.
    pub log_level: Level,
    /// How many reconcile passes one telemetry window covers. At least one.
    pub telemetry_sampling: u32,
    /// What the on-robot console renders as.
    pub console_theme: Theme,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            log_level: Level::Info,
            telemetry_sampling: 1,
            console_theme: Theme::System,
        }
    }
}

impl Settings {
    /// One setting's value, as the word it is written as.
    pub fn value_of(&self, setting: Setting) -> String {
        match setting {
            Setting::LogLevel => self.log_level.as_str().to_string(),
            Setting::TelemetrySampling => self.telemetry_sampling.to_string(),
            Setting::ConsoleTheme => self.console_theme.as_str().to_string(),
        }
    }
}

/// The default value of one setting, as a word.
pub fn default_of(setting: Setting) -> String {
    Settings::default().value_of(setting)
}

/// Who set one setting, and when.
///
/// §38.3's provenance. Both optional, and both report **`unrecorded`** when a file written by hand
/// carries neither — §38.3 is explicit that provenance is recorded rather than inferred, and the
/// invoking user's name is exactly the confidently-wrong record it forbids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// The role that set it, or `None` when the entry does not say.
    pub set_by: Option<String>,
    /// When, in milliseconds since the epoch, or `None` when the entry does not say.
    pub set_at_ms: Option<i64>,
}

impl Provenance {
    /// The role, or the word §38.3 uses for one nobody recorded.
    pub fn set_by_or_unrecorded(&self) -> &str {
        self.set_by.as_deref().unwrap_or("unrecorded")
    }
}

/// What reading the store found.
///
/// The settings are always usable. What varies is how many of them somebody chose, and whether
/// anything on the disk could not be read.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// What this robot is configured to do, defaults filled in.
    pub settings: Settings,
    /// The settings that were set explicitly, with their provenance. Anything absent is a default.
    pub set: Vec<(Setting, Provenance)>,
    /// What could not be read, if anything. Reported rather than swallowed.
    pub notes: Vec<String>,
}

impl Reading {
    /// Whether one setting was chosen rather than defaulted.
    pub fn was_set(&self, setting: Setting) -> bool {
        self.set.iter().any(|(entry, _)| *entry == setting)
    }

    /// The provenance of one setting, if it was set.
    pub fn provenance(&self, setting: Setting) -> Option<&Provenance> {
        self.set
            .iter()
            .find(|(entry, _)| *entry == setting)
            .map(|(_, provenance)| provenance)
    }
}

/// Read what this robot is configured to do.
///
/// Total: a missing file, a torn file and a file full of keys this build does not know all yield
/// working [`Settings`]. Anything it could not use comes back in [`Reading::notes`], because a
/// setting somebody wrote that this robot ignored is a fact they need and never a fact to drop.
pub fn read(layout: &Layout) -> Reading {
    let mut reading = Reading {
        settings: Settings::default(),
        set: Vec::new(),
        notes: Vec::new(),
    };

    let path = layout.settings();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return reading,
        Err(error) => {
            reading
                .notes
                .push(format!("{path:?} could not be read: {error}"));
            return reading;
        }
    };
    let document = match Json::parse(&text) {
        Ok(document) => document,
        Err(error) => {
            reading.notes.push(format!(
                "{path:?} is not readable JSON, so this robot is running every setting's default: \
                 {error}"
            ));
            return reading;
        }
    };
    let Some(entries) = document.as_obj() else {
        reading.notes.push(format!(
            "{path:?} is JSON but not an object, so this robot is running every setting's default."
        ));
        return reading;
    };

    for (key, entry) in entries {
        let setting = match Setting::of(key) {
            Ok(setting) => setting,
            Err(said) => {
                // Kept rather than dropped. A key this build does not know is either a setting a
                // newer robot has — in which case the operator should be told this one ignored it
                // — or a typo, in which case they are wondering why nothing changed.
                reading.notes.push(said);
                continue;
            }
        };
        let Some(word) = value_word(entry) else {
            reading.notes.push(format!(
                "`{}` in {path:?} has no readable value, so its default is in force.",
                setting.as_str()
            ));
            continue;
        };
        match apply(&mut reading.settings, setting, &word) {
            Ok(()) => reading.set.push((setting, provenance_of(entry))),
            Err(said) => reading.notes.push(said),
        }
    }
    reading
}

/// The value out of one entry, which may be a bare value or an object with provenance beside it.
///
/// Both shapes are read, because a store somebody edited by hand to say `{"log_level":"debug"}` is
/// the obvious thing to write and refusing it would be this file being clever at an operator's
/// expense. What [`set`] *writes* is always the object form, so provenance is never lost by us.
fn value_word(entry: &Json) -> Option<String> {
    if let Some(word) = entry.as_str() {
        return Some(word.to_string());
    }
    if let Some(number) = entry.as_f64() {
        return Some(format_count(number));
    }
    let value = entry.get_present("value")?;
    if let Some(word) = value.as_str() {
        return Some(word.to_string());
    }
    value.as_f64().map(format_count)
}

/// A count as the word it is written as, with no trailing `.0`.
fn format_count(number: f64) -> String {
    if number.is_finite() && number.fract() == 0.0 {
        format!("{}", number as i64)
    } else {
        format!("{number}")
    }
}

fn provenance_of(entry: &Json) -> Provenance {
    Provenance {
        set_by: entry
            .get_present("set_by")
            .and_then(Json::as_str)
            .map(str::to_string),
        set_at_ms: entry
            .get_present("set_at_ms")
            .and_then(Json::as_f64)
            .map(|number| number as i64),
    }
}

/// Put one value into a [`Settings`], or say why it is not one.
///
/// `Err` is the sentence, and every one of them lists what would have been accepted. A refusal that
/// says only *invalid value* leaves somebody guessing at a vocabulary they cannot see (§64).
fn apply(settings: &mut Settings, setting: Setting, word: &str) -> Result<(), String> {
    match setting {
        Setting::LogLevel => match Level::of(word) {
            Some(level) => {
                settings.log_level = level;
                Ok(())
            }
            None => Err(format!(
                "`{word}` is not a log level. It is one of: {}.",
                Level::ALL
                    .iter()
                    .map(|level| level.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        },
        Setting::ConsoleTheme => match Theme::of(word) {
            Some(theme) => {
                settings.console_theme = theme;
                Ok(())
            }
            None => Err(format!(
                "`{word}` is not a console theme. It is one of: {}.",
                Theme::ALL
                    .iter()
                    .map(|theme| theme.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        },
        Setting::TelemetrySampling => match word.parse::<u32>() {
            Ok(passes) if (1..=MAX_SAMPLING).contains(&passes) => {
                settings.telemetry_sampling = passes;
                Ok(())
            }
            // Zero is refused rather than read as "off": a robot that silently stopped producing
            // §74 windows is indistinguishable on the Hub from one that stopped running, and that
            // is the confusion §47.4 exists to prevent. Turning telemetry off is not a sampling
            // rate and would need its own decision.
            _ => Err(format!(
                "`{word}` is not a telemetry sampling rate. It is a whole number of reconcile \
                 passes per window, from 1 to {MAX_SAMPLING}. There is no zero: a robot that \
                 quietly stopped sending windows reads on the Hub as a robot that stopped."
            )),
        },
    }
}

/// Why a change was refused.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// The key is not one of the three (§38.1's line).
    UnknownSetting(String),
    /// The key is one of the three and the value is not one it takes.
    Value(String),
    /// It could not be written.
    Write(String),
}

impl Refusal {
    /// The sentence for this refusal.
    pub fn detail(&self) -> String {
        match self {
            Refusal::UnknownSetting(said) | Refusal::Value(said) | Refusal::Write(said) => {
                said.clone()
            }
        }
    }
}

/// One change, for §38.3's two records.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// Which setting.
    pub setting: Setting,
    /// What it was — the value actually in force before, default or not.
    pub from: String,
    /// What it is now.
    pub to: String,
    /// Whether the value it was, was a default rather than something somebody set.
    pub from_was_default: bool,
}

impl Change {
    /// Whether this change changed anything.
    ///
    /// A no-op is still recorded — somebody asked, and §38.3 wants the record of the asking — but a
    /// route can use this to answer honestly rather than implying it moved something.
    pub fn moved(&self) -> bool {
        self.from != self.to
    }
}

/// Set one setting.
///
/// Read-modify-write, through a temporary file and a rename, so an interrupted write cannot leave
/// half a document — and so a key this build does not know **survives** the rewrite. That last part
/// is §38.3's rule for the trust store applied here: an entry somebody wrote is the thing they came
/// to the command about, and a rewrite that dropped it would make changing a log level a way to
/// delete somebody else's setting.
pub fn set(
    layout: &Layout,
    setting: Setting,
    word: &str,
    by: &str,
    now_ms: i64,
) -> Result<Change, Refusal> {
    let mut probe = Settings::default();
    apply(&mut probe, setting, word).map_err(Refusal::Value)?;
    let to = probe.value_of(setting);

    let before = read(layout);
    let change = Change {
        setting,
        from: before.settings.value_of(setting),
        to: to.clone(),
        from_was_default: !before.was_set(setting),
    };

    let mut document = preserved(layout);
    let mut entry = Json::obj();
    entry.set("value", Json::from(to.as_str()));
    entry.set("set_at_ms", Json::Num(now_ms as f64));
    entry.set("set_by", Json::from(by));
    document.set(setting.as_str(), entry);
    write(layout, &document)?;
    Ok(change)
}

/// Put one setting back to its default, and say what it was.
///
/// `Ok(None)` when it was already the default, which is not an error: the intent — *this setting
/// should be what it would be on a robot nobody configured* — is satisfied either way.
pub fn clear(layout: &Layout, setting: Setting) -> Result<Option<Change>, Refusal> {
    let before = read(layout);
    if !before.was_set(setting) {
        return Ok(None);
    }
    let change = Change {
        setting,
        from: before.settings.value_of(setting),
        to: default_of(setting),
        from_was_default: false,
    };

    let mut document = preserved(layout);
    let mut kept = Json::obj();
    if let Some(entries) = document.as_obj() {
        for (key, entry) in entries {
            if key != setting.as_str() {
                kept.set(key, entry.clone());
            }
        }
    }
    document = kept;
    write(layout, &document)?;
    Ok(Some(change))
}

/// The document on disk, or an empty one when there is nothing readable there.
///
/// Whatever is in it is carried forward — including keys this build does not know — so that a
/// rewrite is additive. A file that could not be parsed at all yields an empty object, which does
/// lose it; that is the one case where there is nothing to preserve, and [`read`] has already put a
/// note about it in front of whoever is looking.
fn preserved(layout: &Layout) -> Json {
    std::fs::read_to_string(layout.settings())
        .ok()
        .and_then(|text| Json::parse(&text).ok())
        .filter(|document| document.as_obj().is_some())
        .unwrap_or_else(Json::obj)
}

fn write(layout: &Layout, document: &Json) -> Result<(), Refusal> {
    let path = layout.settings();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| Refusal::Write(format!("{parent:?} could not be created: {error}")))?;
    }
    let text = document
        .to_canonical()
        .map_err(|error| Refusal::Write(format!("the settings could not be encoded: {error}")))?;
    write_atomically(&path, &text)
        .map_err(|error| Refusal::Write(format!("{path:?} could not be written: {error}")))
}

fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;

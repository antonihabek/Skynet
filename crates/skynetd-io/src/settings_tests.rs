//! §38.1's unsigned local configuration, on disk.
//!
//! The property worth more than every other assertion here is the first one: **nothing that can
//! reach the Safety Kernel can be written into this file.** §38.1 draws that line in a sentence —
//! *"the line between the two is drawn by whether the value can reach the kernel"* — and a line
//! drawn in a sentence is one that a later hand widens without noticing. So it is tested.

use super::*;

/// A temporary root that removes itself.
struct Root(std::path::PathBuf);

impl Root {
    fn new(tag: &str) -> Root {
        let path = std::env::temp_dir().join(format!(
            "skynet-settings-{tag}-{}",
            std::process::id() as u64 * 31 + tag.len() as u64
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("data/state")).expect("root");
        Root(path)
    }

    fn layout(&self) -> Layout {
        Layout::new(&self.0)
    }

    fn write(&self, text: &str) {
        std::fs::write(self.layout().settings(), text).expect("write");
    }

    fn text(&self) -> String {
        std::fs::read_to_string(self.layout().settings()).expect("read")
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ── §38.1's line ────────────────────────────────────────────────────────────────────────────────

#[test]
fn no_value_the_kernel_enforces_can_be_written_here() {
    // The one test this file exists for. Each of these is a ceiling the Safety Kernel composes, and
    // every one of them arrives signed (§38.1) or as a narrowing that can only narrow (§38.2).
    // An unsigned store that accepted one would be a path to the kernel that nobody signed.
    for name in [
        "joint_velocity_max_rad_s",
        "joint_torque_max_nm",
        "base_velocity_max_m_s",
        "max_delta_per_cycle",
        "watchdog_ms",
        "rate_limit",
    ] {
        let refusal = Setting::of(name).expect_err(&format!("{name} must not be a setting"));
        assert!(
            refusal.contains("Safety Kernel"),
            "{name} should be refused as a kernel limit: {refusal}"
        );
        assert!(
            refusal.contains("§38.2") || refusal.contains("profile"),
            "{name}'s refusal should say where such a value does live: {refusal}"
        );
    }
}

#[test]
fn the_set_is_closed_and_anything_outside_it_is_refused() {
    // Not a blocklist. The enforcement is that three keys are known and every other string on
    // earth is not, which is what stops the store growing a fourth by anybody adding one.
    for name in [
        "",
        "log",
        "loglevel",
        "LOG_LEVEL",
        "console",
        "anything_at_all",
    ] {
        assert!(
            Setting::of(name).is_err(),
            "`{name}` should not be a setting"
        );
    }
    for setting in Setting::ALL {
        assert_eq!(Setting::of(setting.as_str()), Ok(setting));
    }
}

#[test]
fn an_unknown_key_in_the_file_is_reported_rather_than_ignored() {
    // A key this build does not know is either a setting a newer robot has or a typo. Both are
    // facts the person who wrote it needs: in the first case this robot ignored it, in the second
    // they are waiting for something to change.
    let root = Root::new("unknown-key");
    root.write(r#"{"joint_velocity_max_rad_s":0.5,"log_level":"debug"}"#);

    let reading = read(&root.layout());
    assert_eq!(
        reading.settings.log_level,
        Level::Debug,
        "the known one applies"
    );
    assert_eq!(reading.notes.len(), 1);
    assert!(
        reading.notes[0].contains("Safety Kernel"),
        "{:?}",
        reading.notes
    );
}

// ── Defaults, and there is always a value ───────────────────────────────────────────────────────

#[test]
fn a_robot_nobody_configured_reads_as_the_defaults_with_nothing_to_say() {
    let root = Root::new("empty");
    let reading = read(&root.layout());

    assert_eq!(reading.settings, Settings::default());
    assert_eq!(reading.settings.log_level, Level::Info);
    assert_eq!(reading.settings.telemetry_sampling, 1);
    assert_eq!(reading.settings.console_theme, Theme::System);
    assert!(reading.set.is_empty());
    assert!(
        reading.notes.is_empty(),
        "nothing is wrong with an unconfigured robot"
    );
}

#[test]
fn a_torn_file_yields_working_settings_and_says_so() {
    // The safe direction is the defaults — a daemon that could not decide its own log level would
    // have nothing to print the complaint with. Silence is not, because somebody wrote that file.
    let root = Root::new("torn");
    root.write(r#"{"log_level":"deb"#);

    let reading = read(&root.layout());
    assert_eq!(reading.settings, Settings::default());
    assert_eq!(reading.notes.len(), 1);
    assert!(
        reading.notes[0].contains("not readable JSON"),
        "{:?}",
        reading.notes
    );
}

#[test]
fn a_value_this_build_does_not_take_leaves_that_setting_at_its_default() {
    let root = Root::new("bad-value");
    root.write(r#"{"log_level":"chatty","console_theme":"dark"}"#);

    let reading = read(&root.layout());
    assert_eq!(
        reading.settings.log_level,
        Level::Info,
        "the default stands"
    );
    assert_eq!(
        reading.settings.console_theme,
        Theme::Dark,
        "the other one still applies"
    );
    assert!(
        reading.notes[0].contains("error, warn, info, debug"),
        "{:?}",
        reading.notes
    );
}

// ── Setting one ─────────────────────────────────────────────────────────────────────────────────

#[test]
fn what_was_set_is_what_comes_back() {
    let root = Root::new("roundtrip");
    set(
        &root.layout(),
        Setting::LogLevel,
        "debug",
        "operator",
        1_000,
    )
    .expect("set");
    set(
        &root.layout(),
        Setting::ConsoleTheme,
        "light",
        "operator",
        1_000,
    )
    .expect("set");
    set(
        &root.layout(),
        Setting::TelemetrySampling,
        "6",
        "operator",
        1_000,
    )
    .expect("set");

    let reading = read(&root.layout());
    assert_eq!(reading.settings.log_level, Level::Debug);
    assert_eq!(reading.settings.console_theme, Theme::Light);
    assert_eq!(reading.settings.telemetry_sampling, 6);
    assert!(reading.notes.is_empty());
    for setting in Setting::ALL {
        assert!(reading.was_set(setting), "{}", setting.as_str());
    }
}

#[test]
fn a_change_carries_the_previous_value_that_was_actually_in_force() {
    // §38.3 wants the previous value. On a robot nobody configured that value is the default, and
    // reporting it as "nothing" would leave an audit entry that cannot say what changed.
    let root = Root::new("previous");
    let change = set(
        &root.layout(),
        Setting::LogLevel,
        "debug",
        "operator",
        1_000,
    )
    .expect("set");

    assert_eq!(change.from, "info");
    assert_eq!(change.to, "debug");
    assert!(change.from_was_default, "nobody had set it");
    assert!(change.moved());

    let again = set(&root.layout(), Setting::LogLevel, "warn", "operator", 2_000).expect("set");
    assert_eq!(again.from, "debug");
    assert!(!again.from_was_default, "somebody had set it");
}

#[test]
fn setting_a_value_to_what_it_already_is_is_recorded_and_says_it_moved_nothing() {
    let root = Root::new("noop");
    let change = set(&root.layout(), Setting::LogLevel, "info", "operator", 1_000).expect("set");

    assert!(!change.moved());
    assert!(
        read(&root.layout()).was_set(Setting::LogLevel),
        "the asking is still recorded"
    );
}

#[test]
fn setting_one_leaves_every_other_key_in_the_file_alone() {
    // §38.3's rule for the trust store, applied here: an entry somebody wrote must survive a
    // rewrite, or changing a log level becomes a way to delete somebody else's setting.
    let root = Root::new("preserve");
    root.write(r#"{"console_theme":"dark","a_setting_from_a_newer_robot":"yes"}"#);

    set(
        &root.layout(),
        Setting::LogLevel,
        "debug",
        "operator",
        1_000,
    )
    .expect("set");

    let text = root.text();
    assert!(
        text.contains("a_setting_from_a_newer_robot"),
        "text: {text}"
    );
    assert_eq!(read(&root.layout()).settings.console_theme, Theme::Dark);
}

#[test]
fn provenance_is_written_and_an_entry_without_it_says_unrecorded() {
    // §38.3: recorded, never inferred.
    let root = Root::new("provenance");
    set(
        &root.layout(),
        Setting::LogLevel,
        "debug",
        "operator (local API)",
        4_242,
    )
    .expect("set");

    let reading = read(&root.layout());
    let provenance = reading.provenance(Setting::LogLevel).expect("provenance");
    assert_eq!(provenance.set_by.as_deref(), Some("operator (local API)"));
    assert_eq!(provenance.set_at_ms, Some(4_242));

    root.write(r#"{"log_level":"warn"}"#);
    let bare = read(&root.layout());
    assert_eq!(
        bare.settings.log_level,
        Level::Warn,
        "a bare value still applies"
    );
    let provenance = bare.provenance(Setting::LogLevel).expect("provenance");
    assert_eq!(provenance.set_by, None);
    assert_eq!(provenance.set_by_or_unrecorded(), "unrecorded");
}

#[test]
fn a_hand_written_bare_value_is_read_as_readily_as_one_this_daemon_wrote() {
    // `{"log_level":"debug"}` is the obvious thing to type, and refusing it would be this store
    // being clever at the expense of somebody with a text editor and a broken robot.
    let root = Root::new("bare");
    root.write(r#"{"log_level":"debug","telemetry_sampling":4,"console_theme":"light"}"#);

    let reading = read(&root.layout());
    assert_eq!(reading.settings.log_level, Level::Debug);
    assert_eq!(reading.settings.telemetry_sampling, 4);
    assert_eq!(reading.settings.console_theme, Theme::Light);
    assert!(reading.notes.is_empty(), "{:?}", reading.notes);
}

// ── Sampling ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_sampling_rate_of_zero_is_refused_rather_than_read_as_off() {
    // A robot that quietly stopped sending §74 windows is, from the Hub, a robot that stopped —
    // which is §47.4's whole concern. Turning telemetry off is a different decision and would need
    // to be made somewhere it can be seen.
    let root = Root::new("sampling-zero");
    match set(
        &root.layout(),
        Setting::TelemetrySampling,
        "0",
        "operator",
        0,
    ) {
        Err(Refusal::Value(said)) => {
            assert!(said.contains("There is no zero"), "said: {said}");
        }
        other => panic!("expected a value refusal, got {other:?}"),
    }
}

#[test]
fn sampling_is_capped_and_the_cap_itself_is_allowed() {
    let root = Root::new("sampling-cap");
    set(
        &root.layout(),
        Setting::TelemetrySampling,
        &MAX_SAMPLING.to_string(),
        "operator",
        0,
    )
    .expect("the cap itself");

    match set(
        &root.layout(),
        Setting::TelemetrySampling,
        &(MAX_SAMPLING + 1).to_string(),
        "operator",
        0,
    ) {
        Err(Refusal::Value(said)) => {
            assert!(said.contains(&MAX_SAMPLING.to_string()), "said: {said}")
        }
        other => panic!("expected a value refusal, got {other:?}"),
    }
}

#[test]
fn a_sampling_rate_that_is_not_a_whole_number_is_refused() {
    let root = Root::new("sampling-fraction");
    for word in ["1.5", "-2", "many", ""] {
        assert!(
            matches!(
                set(
                    &root.layout(),
                    Setting::TelemetrySampling,
                    word,
                    "operator",
                    0
                ),
                Err(Refusal::Value(_))
            ),
            "`{word}` should be refused"
        );
    }
}

// ── Clearing ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn clearing_puts_one_back_to_its_default_and_leaves_the_others() {
    let root = Root::new("clear");
    set(&root.layout(), Setting::LogLevel, "debug", "operator", 0).expect("set");
    set(&root.layout(), Setting::ConsoleTheme, "dark", "operator", 0).expect("set");

    let change = clear(&root.layout(), Setting::LogLevel)
        .expect("clear")
        .expect("there was one");
    assert_eq!(change.from, "debug");
    assert_eq!(change.to, "info");

    let reading = read(&root.layout());
    assert_eq!(reading.settings.log_level, Level::Info);
    assert!(!reading.was_set(Setting::LogLevel));
    assert_eq!(
        reading.settings.console_theme,
        Theme::Dark,
        "the other one is untouched"
    );
}

#[test]
fn clearing_one_nobody_set_is_not_an_error() {
    // The intent — this setting should be what it would be on a robot nobody configured — is
    // satisfied either way, and an error would send somebody looking for something whose absence
    // is what they wanted.
    let root = Root::new("clear-unset");
    assert_eq!(clear(&root.layout(), Setting::LogLevel), Ok(None));
}

#[test]
fn clearing_leaves_a_key_this_build_does_not_know_in_place() {
    let root = Root::new("clear-preserve");
    root.write(r#"{"log_level":"debug","a_setting_from_a_newer_robot":"yes"}"#);

    clear(&root.layout(), Setting::LogLevel).expect("clear");

    let text = root.text();
    assert!(
        text.contains("a_setting_from_a_newer_robot"),
        "text: {text}"
    );
    assert!(!text.contains("log_level"), "text: {text}");
}

// ── The file ────────────────────────────────────────────────────────────────────────────────────

#[test]
fn a_setting_nobody_chose_is_absent_from_the_file_rather_than_written_as_its_default() {
    // §31.4's absent-not-null rule, and it earns its keep here: a default written down becomes a
    // choice the next reader believes somebody made, and §38.3's provenance would then be a
    // fabricated record of a decision nobody took.
    let root = Root::new("absent");
    set(&root.layout(), Setting::LogLevel, "debug", "operator", 0).expect("set");

    let text = root.text();
    assert!(text.contains("log_level"));
    assert!(!text.contains("console_theme"), "text: {text}");
    assert!(!text.contains("telemetry_sampling"), "text: {text}");
}

#[test]
fn the_file_is_canonical_json_like_every_other_document_this_robot_writes() {
    let root = Root::new("canonical");
    set(
        &root.layout(),
        Setting::ConsoleTheme,
        "light",
        "operator",
        7,
    )
    .expect("set");

    let text = root.text();
    let reparsed = Json::parse(&text).expect("canonical JSON");
    assert_eq!(reparsed.to_canonical().expect("re-encode"), text);
}

//! §45.4's event journal.
//!
//! The property this file exists for is the one in the module's own documentation: **an event can
//! only invalidate a calibration, never validate one.** That is what makes an unsigned journal safe
//! among signed records, and it is asserted rather than asserted-about.

use super::*;

/// A temporary root that removes itself.
struct Root(std::path::PathBuf);

impl Root {
    fn new(tag: &str) -> Root {
        let path = std::env::temp_dir().join(format!(
            "skynet-calevents-{tag}-{}",
            std::process::id() as u64 * 31 + tag.len() as u64
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("data/state/calibration")).expect("root");
        Root(path)
    }

    fn layout(&self) -> Layout {
        Layout::new(&self.0)
    }

    fn write(&self, text: &str) {
        std::fs::write(self.layout().calibration_events(), text).expect("write");
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn at(event: &str, at_s: f64) -> Entry {
    Entry {
        event: event.to_string(),
        at_s,
        recorded_by: "operator (local API)".to_string(),
        note: None,
    }
}

#[test]
fn a_robot_nothing_has_happened_to_has_an_empty_journal() {
    let root = Root::new("empty");
    assert_eq!(read(&root.layout()), Journal::default());
}

#[test]
fn what_was_recorded_is_what_comes_back_oldest_first() {
    let root = Root::new("roundtrip");
    record(&root.layout(), at("collision", 100.0)).expect("first");
    record(&root.layout(), at("tool_change", 200.0)).expect("second");

    let journal = read(&root.layout());
    assert_eq!(journal.entries.len(), 2);
    assert_eq!(journal.entries[0].event, "collision");
    assert_eq!(journal.entries[1].event, "tool_change");
    assert!(journal.unreadable.is_empty());
}

#[test]
fn an_event_only_counts_against_a_record_it_is_newer_than() {
    // The rule that makes re-measuring possible at all. Without it a set would expire again the
    // instant it was written, and there would be no way to clear an impact off a robot.
    let root = Root::new("since");
    record(&root.layout(), at("collision", 100.0)).expect("record");

    let journal = read(&root.layout());
    assert_eq!(journal.since(Some(50.0)), vec!["collision".to_string()]);
    assert!(
        journal.since(Some(150.0)).is_empty(),
        "a calibration taken after the impact is not invalidated by it"
    );
    // The boundary: an event at exactly the moment of measurement does not count. Either
    // convention is defensible; the one that is not is leaving it to whichever the code does.
    assert!(journal.since(Some(100.0)).is_empty());
}

#[test]
fn a_record_that_does_not_say_when_it_was_taken_counts_every_event() {
    // The conservative direction, and §45.4 is a section about not trusting what may have been
    // disturbed: a record whose date nobody wrote down cannot be shown to predate an impact.
    let root = Root::new("undated");
    record(&root.layout(), at("collision", 100.0)).expect("record");

    assert_eq!(
        read(&root.layout()).since(None),
        vec!["collision".to_string()]
    );
}

#[test]
fn an_event_this_vocabulary_does_not_have_is_refused_and_names_the_five() {
    // The hazard is specific: `collison` matches no policy, invalidates nothing, and leaves an
    // operator believing they have flagged an impact.
    let root = Root::new("vocabulary");
    match record(&root.layout(), at("collison", 1.0)) {
        Err(Refusal::UnknownEvent(said)) => {
            assert!(said.contains("collision"), "said: {said}");
            assert!(said.contains("would invalidate nothing"), "said: {said}");
        }
        other => panic!("expected an unknown-event refusal, got {other:?}"),
    }
    assert!(
        read(&root.layout()).entries.is_empty(),
        "nothing was written"
    );
}

#[test]
fn every_event_the_contract_publishes_is_accepted() {
    // The two lists must not drift: an event §45.4 names and this refuses would be a robot that
    // cannot be told about an impact it defines.
    let root = Root::new("vocabulary-all");
    for (index, event) in INVALIDATING_EVENTS.iter().enumerate() {
        record(&root.layout(), at(event, index as f64)).expect(event);
    }
    assert_eq!(
        read(&root.layout()).entries.len(),
        INVALIDATING_EVENTS.len()
    );
}

#[test]
fn a_line_that_cannot_be_read_is_reported_rather_than_skipped() {
    // §38.3's rule for the audit log, and it is worse here than there: a dropped line is an impact
    // that stops invalidating anything.
    let root = Root::new("torn");
    root.write("{\"event\":\"collision\",\"at_s\":1}\nnot json at all\n{\"at_s\":2}\n");

    let journal = read(&root.layout());
    assert_eq!(journal.entries.len(), 1, "the readable one is still read");
    assert_eq!(journal.unreadable.len(), 2);
    assert!(
        journal.unreadable[0].contains("line 2"),
        "{:?}",
        journal.unreadable
    );
    assert!(
        journal.unreadable[1].contains("line 3"),
        "{:?}",
        journal.unreadable
    );
}

#[test]
fn a_line_written_by_hand_without_provenance_says_unrecorded() {
    // §38.3: recorded, never inferred.
    let root = Root::new("provenance");
    root.write("{\"event\":\"tool_change\",\"at_s\":5}\n");

    let journal = read(&root.layout());
    assert_eq!(journal.entries[0].recorded_by, "unrecorded");
}

#[test]
fn recording_appends_rather_than_rewriting() {
    // An append must not require rewriting the file — §38.3's reason for JSON Lines, and rewriting
    // a record of impacts is what such a record exists to make difficult.
    let root = Root::new("append");
    record(&root.layout(), at("collision", 1.0)).expect("first");
    let after_one = std::fs::read_to_string(root.layout().calibration_events()).expect("read");
    record(&root.layout(), at("tool_change", 2.0)).expect("second");
    let after_two = std::fs::read_to_string(root.layout().calibration_events()).expect("read");

    assert!(
        after_two.starts_with(&after_one),
        "the first line is untouched"
    );
}

#[test]
fn there_is_no_way_to_clear_one() {
    // Not a test of behaviour but of surface, and it is the safety property in this module: a
    // journal that could clear an event would be an unsigned path to making a stale calibration
    // believed again — §45.1's opening failure, a guard measuring to the wrong place while
    // reporting healthy. If a `clear` or `remove` is ever added here, this is where the argument
    // against it is written down.
    let root = Root::new("no-clear");
    record(&root.layout(), at("collision", 1.0)).expect("record");
    assert_eq!(read(&root.layout()).entries.len(), 1);
}

#[test]
fn a_journal_longer_than_the_bound_keeps_the_newest_and_says_it_did() {
    // Reached only by something writing events in a loop, which is exactly when a calibration
    // report still has to answer. The oldest go, because an event older than every stored record
    // already counts against nothing.
    let root = Root::new("bound");
    let mut text = String::new();
    for index in 0..(MAX_ENTRIES + 5) {
        text.push_str(&format!("{{\"event\":\"collision\",\"at_s\":{index}}}\n"));
    }
    root.write(&text);

    let journal = read(&root.layout());
    assert_eq!(journal.entries.len(), MAX_ENTRIES);
    assert_eq!(journal.entries[0].at_s, 5.0, "the newest are kept");
    assert!(
        journal.unreadable[0].contains("5 older event"),
        "{:?}",
        journal.unreadable
    );
}

#[test]
fn a_note_survives_and_an_entry_without_one_writes_no_null() {
    // §31.4's absent-not-null, which means the same here: absent is "nobody said", not a null
    // somebody wrote down.
    let root = Root::new("note");
    let mut entry = at("collision", 1.0);
    entry.note = Some("clipped the fixture".to_string());
    record(&root.layout(), entry).expect("with note");
    record(&root.layout(), at("tool_change", 2.0)).expect("without");

    let journal = read(&root.layout());
    assert_eq!(
        journal.entries[0].note.as_deref(),
        Some("clipped the fixture")
    );
    assert_eq!(journal.entries[1].note, None);

    let text = std::fs::read_to_string(root.layout().calibration_events()).expect("read");
    assert!(!text.contains("null"), "text: {text}");
}

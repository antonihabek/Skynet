//! §38.2's session override, on disk.
//!
//! The two properties worth more than the rest: it **only ever narrows**, which is enforced in the
//! kernel and guarded here by refusing anything that could not be a narrowing; and it **expires**,
//! which is enforced here because the kernel has no clock. §38.2's own sentence for the second is
//! the one these are written against — *"a 'maintenance mode, half speed' that outlives the
//! maintenance is a hazard that looks like a setting."*

use super::*;

/// A temporary root that removes itself.
struct Root(std::path::PathBuf);

impl Root {
    fn new(tag: &str) -> Root {
        let path = std::env::temp_dir().join(format!(
            "skynet-session-{tag}-{}",
            std::process::id() as u64 * 31 + tag.len() as u64
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(path.join("data/state")).expect("root");
        Root(path)
    }

    fn layout(&self) -> Layout {
        Layout::new(&self.0)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A half-speed override set at `set_at_ms` and running for `duration`.
fn half_speed(set_at_ms: i64, duration: i64) -> Stored {
    Stored {
        name: "maintenance".to_string(),
        set_by: "engineer@bench".to_string(),
        reason: Some("replacing the wrist harness".to_string()),
        set_at_ms,
        expires_at_ms: set_at_ms + duration,
        joint_velocity_max_rad_s: Some(0.5),
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        max_delta_per_cycle: None,
    }
}

#[test]
fn nothing_set_reads_as_none() {
    let root = Root::new("none");
    assert_eq!(read(&root.layout(), 1_000), Reading::None);
}

#[test]
fn what_was_set_is_what_comes_back() {
    let root = Root::new("roundtrip");
    let stored = half_speed(1_000, 60_000);
    set(&root.layout(), stored.clone()).expect("set");

    match read(&root.layout(), 2_000) {
        Reading::Live(back) => assert_eq!(back, stored),
        other => panic!("expected a live override, got {other:?}"),
    }
}

#[test]
fn it_stops_applying_the_millisecond_it_expires() {
    // The boundary rather than a comfortable distance from it: `live_at` is `<`, so an override
    // whose deadline is exactly now has expired. Either convention is defensible and the one that
    // is not is having the test agree with whichever the code happens to do.
    let root = Root::new("expiry");
    set(&root.layout(), half_speed(1_000, 1_000)).expect("set");

    assert!(matches!(read(&root.layout(), 1_999), Reading::Live(_)));
    assert!(matches!(read(&root.layout(), 2_000), Reading::Expired(_)));
    assert!(matches!(read(&root.layout(), 9_999), Reading::Expired(_)));
}

#[test]
fn an_expired_override_is_not_collapsed_into_nothing_set() {
    // The distinction the `Reading` enum exists for. Somebody set this file expecting the robot to
    // be slowed; when it stops being slowed, that is an event, not a return to a state nobody
    // changed.
    let root = Root::new("expired-detail");
    set(&root.layout(), half_speed(1_000, 1_000)).expect("set");

    match read(&root.layout(), 5_000) {
        Reading::Expired(stored) => {
            assert_eq!(stored.name, "maintenance");
            assert_eq!(stored.set_by, "engineer@bench");
            assert_eq!(stored.remaining_ms(5_000), 0);
        }
        other => panic!("expected an expired override, got {other:?}"),
    }
}

#[test]
fn a_survivable_reboot_is_the_point_of_wall_time() {
    // Nothing here simulates a reboot — the file is the simulation. What it pins is that liveness
    // is decided entirely by two numbers in the document and the clock passed in, with no state in
    // this process, which is what makes an override survive a restart and expire during one.
    let root = Root::new("reboot");
    set(&root.layout(), half_speed(1_000, 600_000)).expect("set");

    assert!(matches!(read(&root.layout(), 300_000), Reading::Live(_)));
    assert!(matches!(read(&root.layout(), 700_000), Reading::Expired(_)));
}

#[test]
fn an_override_longer_than_the_cap_is_refused_and_says_what_the_cap_is() {
    let root = Root::new("cap");
    let result = set(&root.layout(), half_speed(0, MAX_DURATION_MS + 1));

    match result {
        Err(Refusal::Duration(said)) => {
            assert!(said.contains("8 hours"), "said: {said}");
            assert!(said.contains("signed profile"), "said: {said}");
        }
        other => panic!("expected a duration refusal, got {other:?}"),
    }
    assert_eq!(
        read(&root.layout(), 0),
        Reading::None,
        "nothing was written"
    );
}

#[test]
fn exactly_the_cap_is_allowed() {
    let root = Root::new("cap-edge");
    set(&root.layout(), half_speed(0, MAX_DURATION_MS)).expect("the cap itself is allowed");
}

#[test]
fn an_override_that_expires_in_the_past_is_refused() {
    let root = Root::new("backwards");
    assert!(matches!(
        set(&root.layout(), half_speed(5_000, -1_000)),
        Err(Refusal::Duration(_))
    ));
}

#[test]
fn an_override_that_narrows_nothing_is_refused() {
    // It would compose to no change and still raise the notice saying the robot is held back,
    // which is a false sentence about a robot that is not.
    let root = Root::new("empty");
    let mut stored = half_speed(0, 60_000);
    stored.joint_velocity_max_rad_s = None;

    match set(&root.layout(), stored) {
        Err(Refusal::NarrowsNothing) => {}
        other => panic!("expected a narrows-nothing refusal, got {other:?}"),
    }
}

#[test]
fn a_ceiling_of_zero_or_less_is_refused_rather_than_composed() {
    // `narrowest()` would take it as the tightest input and the robot would be unable to move —
    // safe, and undiagnosable from a limits page.
    let root = Root::new("zero");
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut stored = half_speed(0, 60_000);
        stored.joint_velocity_max_rad_s = Some(bad);
        match set(&root.layout(), stored) {
            Err(Refusal::Ceiling(said)) => {
                assert!(said.contains("greater than zero"), "said: {said}");
            }
            other => panic!("expected a ceiling refusal for {bad}, got {other:?}"),
        }
    }
}

#[test]
fn setting_one_replaces_the_last() {
    // Not merged. Two overrides live at once would be a fifth and sixth layer nobody specified,
    // and "which of the two is holding this robot" is not a question a bench should raise.
    let root = Root::new("replace");
    set(&root.layout(), half_speed(0, 60_000)).expect("first");

    let mut second = half_speed(0, 60_000);
    second.name = "second".to_string();
    second.joint_velocity_max_rad_s = Some(0.25);
    set(&root.layout(), second).expect("second");

    match read(&root.layout(), 1_000) {
        Reading::Live(stored) => {
            assert_eq!(stored.name, "second");
            assert_eq!(stored.joint_velocity_max_rad_s, Some(0.25));
        }
        other => panic!("expected the second override, got {other:?}"),
    }
}

#[test]
fn clearing_says_whether_there_was_one_and_clearing_twice_is_not_an_error() {
    let root = Root::new("clear");
    set(&root.layout(), half_speed(0, 60_000)).expect("set");

    assert!(clear(&root.layout()).expect("clear"), "there was one");
    assert!(
        !clear(&root.layout()).expect("clear again"),
        "there was not"
    );
    assert_eq!(read(&root.layout(), 0), Reading::None);
}

#[test]
fn a_torn_file_is_reported_rather_than_applied_or_ignored() {
    // Dropping it is the safe direction — the robot runs at its profile's ceilings, where it would
    // be with no override at all. Saying nothing is not, because somebody set that file expecting
    // the robot to be slowed.
    let root = Root::new("torn");
    std::fs::write(root.layout().session_override(), "{\"name\":\"half").expect("write");

    match read(&root.layout(), 0) {
        Reading::Unreadable(said) => assert!(said.contains("not readable JSON"), "said: {said}"),
        other => panic!("expected unreadable, got {other:?}"),
    }
}

#[test]
fn a_document_missing_its_deadline_is_unreadable_rather_than_permanent() {
    // The worst possible reading of a missing `expires_at_ms` would be "no deadline", which is an
    // override that never expires — precisely §38.2's hazard, arrived at by a parsing default.
    let root = Root::new("no-deadline");
    std::fs::write(
        root.layout().session_override(),
        "{\"name\":\"half\",\"set_at_ms\":0}",
    )
    .expect("write");

    match read(&root.layout(), 0) {
        Reading::Unreadable(said) => assert!(said.contains("expires_at_ms"), "said: {said}"),
        other => panic!("expected unreadable, got {other:?}"),
    }
}

#[test]
fn an_unset_ceiling_is_absent_from_the_file_rather_than_null() {
    // §31.4's rule, and it means the same thing here: absent is "nobody narrowed this", which is
    // not the same as a null somebody wrote down.
    let root = Root::new("absent");
    set(&root.layout(), half_speed(0, 60_000)).expect("set");

    let text = std::fs::read_to_string(root.layout().session_override()).expect("read");
    assert!(text.contains("joint_velocity_max_rad_s"));
    assert!(!text.contains("joint_torque_max_nm"), "text: {text}");
    assert!(!text.contains("null"), "text: {text}");
}

#[test]
fn provenance_survives_the_round_trip_and_a_file_without_it_says_unattributed() {
    // §38.3 wants who and when on every configuration change. A store written by hand has neither,
    // and the honest report of that is a word rather than a fabricated name — the same answer §38.3
    // gives for the trust store.
    let root = Root::new("provenance");
    std::fs::write(
        root.layout().session_override(),
        "{\"name\":\"half\",\"set_at_ms\":0,\"expires_at_ms\":9000,\"joint_velocity_max_rad_s\":0.5}",
    )
    .expect("write");

    match read(&root.layout(), 0) {
        Reading::Live(stored) => {
            assert_eq!(stored.set_by, "unattributed");
            assert_eq!(stored.reason, None);
        }
        other => panic!("expected live, got {other:?}"),
    }
}

#[test]
fn the_kernel_view_carries_the_ceilings_and_nothing_else() {
    // The kernel gets no clock, no provenance and no reason. It cannot ask whether this is still
    // valid, which is the design: it is handed only overrides that are.
    let stored = half_speed(0, 60_000);
    let view = stored.as_kernel();

    assert_eq!(view.name, "maintenance");
    assert_eq!(view.joint_velocity_max_rad_s, Some(0.5));
    assert_eq!(view.joint_torque_max_nm, None);
}

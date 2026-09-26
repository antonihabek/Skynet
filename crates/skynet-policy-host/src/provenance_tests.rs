//! §90.2 and §74: what a remote policy's host tells the supervisor, held here as facts and files.

use std::path::PathBuf;

use crate::provenance::{run_name, Fact, Mark, Provenance, Tracker, QUEUE_MAX};

fn mark(seq: u64, stamp_ns: u64) -> Mark {
    Mark { seq, stamp_ns }
}

fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("skynetp-provenance-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn records(dir: &PathBuf) -> Vec<(String, skynet_json::Json)> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.ends_with(".json") && !name.starts_with('.'))
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| {
            let text = std::fs::read_to_string(dir.join(&name)).unwrap();
            (name, skynet_json::Json::parse(&text).unwrap())
        })
        .collect()
}

fn text(record: &skynet_json::Json, key: &str) -> String {
    record
        .get(key)
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_string()
}

#[test]
fn a_session_is_opened_proposes_once_and_closes_on_its_last_proposal() {
    let mut tracker = Tracker::default();
    // Nothing open, nothing proposed: nothing to say.
    assert!(tracker.observe(None, None).is_empty());
    // Opened and proposing on one turn: said in that order, so a reader sees the model first.
    assert_eq!(
        tracker.observe(Some(("s1", "sha256:aa")), Some(mark(1, 100))),
        vec![
            Fact::Opened {
                session: "s1".into(),
                digest: "sha256:aa".into()
            },
            Fact::Proposing {
                session: "s1".into(),
                mark: mark(1, 100)
            },
        ]
    );
    // Later proposals move the end silently: `proposing` is said once.
    assert!(tracker
        .observe(Some(("s1", "sha256:aa")), Some(mark(2, 200)))
        .is_empty());
    assert!(tracker.observe(Some(("s1", "sha256:aa")), None).is_empty());
    // The session ends: closed on its last proposal.
    assert_eq!(
        tracker.observe(None, None),
        vec![Fact::Closed {
            session: "s1".into(),
            mark: mark(2, 200)
        }]
    );
}

#[test]
fn a_new_session_closes_the_old_one_first_and_one_that_never_proposed_is_not_closed() {
    let mut tracker = Tracker::default();
    tracker.observe(Some(("s1", "sha256:aa")), None);
    // s1 never proposed: its end has no command to name, so no `closed` is said for it.
    assert_eq!(
        tracker.observe(Some(("s2", "sha256:bb")), Some(mark(1, 10))),
        vec![
            Fact::Opened {
                session: "s2".into(),
                digest: "sha256:bb".into()
            },
            Fact::Proposing {
                session: "s2".into(),
                mark: mark(1, 10)
            },
        ]
    );
    // A repin arrives as a new session (D8): the old one is closed before the new one opens.
    assert_eq!(
        tracker.observe(Some(("s3", "sha256:cc")), None),
        vec![
            Fact::Closed {
                session: "s2".into(),
                mark: mark(1, 10)
            },
            Fact::Opened {
                session: "s3".into(),
                digest: "sha256:cc".into()
            },
        ]
    );
    // Stopping closes whatever proposed.
    tracker.observe(Some(("s3", "sha256:cc")), Some(mark(2, 20)));
    assert_eq!(
        tracker.finish(),
        vec![Fact::Closed {
            session: "s3".into(),
            mark: mark(2, 20)
        }]
    );
}

#[test]
fn facts_and_counts_reach_the_queue_whole_and_in_order() {
    let dir = scratch("order");
    let run = run_name(42, 7);
    assert_eq!(run, "00000000000000000042-7");
    let mut queue = Provenance::open(&dir, run.clone());
    queue.turn(
        Some(("s1", "sha256:aa")),
        Some(mark(1, 18_446_744_073_709_551_615)),
        1,
        0,
    );

    let written = records(&dir);
    let kinds: Vec<String> = written
        .iter()
        .map(|(_, record)| text(record, "kind"))
        .collect();
    assert_eq!(kinds, vec!["opened", "proposing", "counts"]);
    assert!(written
        .iter()
        .all(|(name, record)| name.starts_with(&run) && text(record, "run") == run));
    // A stamp past 2^53 survives exactly, because it travels as a string.
    assert_eq!(text(&written[1].1, "stamp_ns"), "18446744073709551615");
    assert_eq!(text(&written[2].1, "proposals"), "1");
    assert_eq!(
        written[2].1.get("final").and_then(|value| value.as_bool()),
        Some(false)
    );
    // No partial file is ever left where the supervisor reads.
    assert!(std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .all(|entry| !entry.file_name().to_string_lossy().ends_with(".partial")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn one_counts_record_is_in_flight_and_the_rest_accumulate_until_it_is_taken() {
    let dir = scratch("flight");
    let mut queue = Provenance::open(&dir, run_name(1, 1));
    queue.turn(None, None, 1, 0);
    // Not taken yet: these turns accumulate rather than piling files up.
    queue.turn(None, None, 5, 2);
    queue.turn(None, None, 9, 3);
    let pending = records(&dir);
    assert_eq!(pending.len(), 1);
    assert_eq!(text(&pending[0].1, "proposals"), "1");
    // The supervisor takes it; the next turn writes everything since, as one delta.
    std::fs::remove_file(dir.join(&pending[0].0)).unwrap();
    queue.turn(None, None, 10, 4);
    let next = records(&dir);
    assert_eq!(next.len(), 1);
    assert_eq!(
        (text(&next[0].1, "proposals"), text(&next[0].1, "stale")),
        ("9".to_string(), "4".to_string())
    );
    // Ending writes a final record even with one still in flight, so nothing counted is lost.
    queue.finish(12, 4);
    let last = records(&dir);
    assert_eq!(last.len(), 2);
    assert_eq!(text(&last[1].1, "proposals"), "2");
    assert_eq!(
        last[1].1.get("final").and_then(|value| value.as_bool()),
        Some(true)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_full_queue_drops_a_fact_and_says_so_on_the_next_one_it_writes() {
    let dir = scratch("full");
    std::fs::create_dir_all(&dir).unwrap();
    for index in 0..QUEUE_MAX {
        std::fs::write(dir.join(format!("filler-{index:05}.json")), "{}").unwrap();
    }
    let mut queue = Provenance::open(&dir, run_name(2, 2));
    queue.turn(Some(("s1", "sha256:aa")), None, 0, 0);
    assert_eq!(queue.unqueued(), 1);
    // Room again: the next fact carries the count of the one that was lost.
    for index in 0..QUEUE_MAX {
        std::fs::remove_file(dir.join(format!("filler-{index:05}.json"))).unwrap();
    }
    queue.turn(Some(("s1", "sha256:aa")), Some(mark(1, 5)), 1, 0);
    let written = records(&dir);
    let proposing = written
        .iter()
        .find(|(_, record)| text(record, "kind") == "proposing")
        .expect("the proposal was queued");
    assert_eq!(text(&proposing.1, "unqueued_before"), "1");
    assert_eq!(queue.unqueued(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

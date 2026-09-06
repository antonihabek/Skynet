//! What the counter has to be true of, and one thing it deliberately is not.

use super::*;
#[cfg(target_os = "linux")]
use std::sync::atomic::Ordering;

/// A directory nobody else is using, removed on drop. No `tempfile`: the workspace has no
/// third-party dependencies, and this is the whole of what one would be for.
///
/// Linux-only along with everything that uses it, and that is worth one line rather than an
/// `allow`: on any other target this crate's five interesting tests do not exist, and a helper
/// compiled for nobody is exactly what a dead-code warning is for.
#[cfg(target_os = "linux")]
struct TempDir(PathBuf);

#[cfg(target_os = "linux")]
impl TempDir {
    fn new(label: &str) -> TempDir {
        static COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("skynet-claim-{}-{label}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        TempDir(path)
    }
}

#[cfg(target_os = "linux")]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn a_seeded_counter_hands_out_the_scan_s_next_sequence_first() {
    // The seed is `Ring::next_sequence()`, so the first claim must *be* that number rather than
    // one past it. `fetch_add` returning the value before the addition is what makes that true,
    // and getting it backwards would overwrite the newest record on the disk.
    let dir = TempDir::new("seed");
    let claim = Claim::seed(&dir.0, 41).expect("seed");
    assert_eq!(claim.peek(), 41);
    assert_eq!(claim.next(), 41);
    assert_eq!(claim.next(), 42);
    assert_eq!(claim.peek(), 43);
}

#[cfg(target_os = "linux")]
#[test]
fn two_mappings_of_one_file_never_hand_out_the_same_number() {
    // The whole of §26.2 in one assertion. Two `Claim`s are two independent mappings — the same
    // relationship two *processes* have — and a counter held per-mapping rather than in the shared
    // page would give each of them 1, 2, 3 and encode two records into one slot.
    let dir = TempDir::new("two-mappings");
    let a = Claim::seed(&dir.0, 1).expect("seed");
    let b = Claim::attach_existing(&dir.0).expect("attach");

    let mut seen = std::collections::BTreeSet::new();
    for _ in 0..500 {
        assert!(seen.insert(a.next()), "a handed out a duplicate");
        assert!(seen.insert(b.next()), "b handed out a duplicate");
    }
    assert_eq!(seen.len(), 1000);
    // Dense as well as unique: a black box's numbering has no holes it did not record as gaps.
    assert_eq!(
        seen.iter().copied().collect::<Vec<_>>(),
        (1..=1000).collect::<Vec<_>>()
    );
}

#[cfg(target_os = "linux")]
#[test]
fn threads_racing_the_counter_take_every_number_exactly_once() {
    // `fetch_add` under contention, which is the property the mapping exists to provide. Sixteen
    // threads over four mappings: if the counter were per-mapping, or the ordering were `Relaxed`
    // in a way that lost updates, this would come back short.
    let dir = TempDir::new("threads");
    let seeded = Claim::seed(&dir.0, 1).expect("seed");
    drop(seeded);

    let mut handles = Vec::new();
    for _ in 0..4 {
        let path = dir.0.clone();
        handles.push(std::thread::spawn(move || {
            let claim = Claim::attach_existing(&path).expect("attach");
            let mut mine = Vec::new();
            for _ in 0..250 {
                mine.push(claim.next());
            }
            mine
        }));
    }
    let mut all: Vec<u64> = handles
        .into_iter()
        .flat_map(|h| h.join().unwrap())
        .collect();
    all.sort_unstable();
    assert_eq!(all, (1..=1000).collect::<Vec<_>>());
}

#[cfg(target_os = "linux")]
#[test]
fn seeding_again_resets_it_which_is_why_the_caller_holds_the_lock() {
    // Written down as a test rather than only as a doc comment, because it is the sharp edge:
    // `seed` is unconditional, so a second one while a writer is claiming resets the counter under
    // it. §26.2 narrows the directory lock's meaning to "one creator" for exactly this reason, and
    // a reader of this crate should meet the hazard rather than infer it.
    let dir = TempDir::new("reseed");
    let first = Claim::seed(&dir.0, 100).expect("seed");
    assert_eq!(first.next(), 100);

    let second = Claim::seed(&dir.0, 1).expect("re-seed");
    assert_eq!(second.peek(), 1);
    assert_eq!(
        first.peek(),
        1,
        "the two mappings are one page, so the reset is visible through both"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn a_file_that_is_not_there_yet_is_created_rather_than_refused() {
    // `attach_existing` names the ordinary case and not a precondition. A writer that arrived
    // first would otherwise have to distinguish "no recording yet" from "a broken one", and both
    // are the same syscall away from working.
    let dir = TempDir::new("absent");
    let claim = Claim::attach_existing(&dir.0).expect("attach to an absent file");
    assert_eq!(claim.peek(), 0, "an unseeded counter reads zero");
    assert!(path_in(&dir.0).exists());
}

#[test]
fn the_path_is_beside_the_ring() {
    // Pinned because two writers find each other by it: a crate that put it somewhere else would
    // give each of them their own counter and every property above would still hold.
    assert_eq!(
        path_in(Path::new("/data/blackbox")),
        Path::new("/data/blackbox/claim")
    );
}

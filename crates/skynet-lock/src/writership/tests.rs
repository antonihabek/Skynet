//! What §26.2's two roles have to be true of.
//!
//! The interesting ones need two live mappings of one file, so they are Linux-only for
//! `skynet-claim`'s reason — and, until this round, no test in this workspace that needed Linux had
//! ever run, because every logged suite ran on Windows. These are written to be run there.

use super::*;
use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};

/// A directory nobody else is using. No `tempfile`: the workspace has no third-party dependencies.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> TempDir {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = std::env::temp_dir().join(format!(
            "skynet-writership-{}-{label}-{n}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a scratch directory");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The first process into a directory creates; the second attaches. This is the whole of the
/// narrowing §26.2 asked for, and it holds on every platform because the roles are decided by the
/// lock rather than by the mapping.
#[test]
fn the_first_writer_creates_and_the_second_is_told_to_attach() {
    let dir = TempDir::new("roles");

    let first = match Writership::join(dir.path(), "skynetd").expect("a fresh directory") {
        Joining::Creator(creating) => creating,
        Joining::Attacher(_) => panic!("a directory with no lock file has no holder to attach to"),
    };
    let first = first.seed(1);
    assert_eq!(first.role(), Role::Creator);
    assert!(first.may_seal(), "the creator owns the window");

    match Writership::join(dir.path(), "skynetk").expect("joining is not an error") {
        Joining::Attacher(attaching) => {
            assert!(
                attaching.holder().contains("skynetd"),
                "the second writer is told who created the recording, not just that somebody did: \
                 {}",
                attaching.holder(),
            );
        }
        Joining::Creator(_) => panic!("two creators over one directory"),
    }
}

/// Releasing the creator's writership lets the next process create — so this is a lock and not a
/// permanent refusal.
#[test]
fn dropping_the_creator_lets_the_next_process_create() {
    let dir = TempDir::new("release");

    let first = match Writership::join(dir.path(), "skynetd").unwrap() {
        Joining::Creator(creating) => creating.seed(1),
        Joining::Attacher(_) => panic!("fresh"),
    };
    drop(first);

    match Writership::join(dir.path(), "skynetk").unwrap() {
        Joining::Creator(_) => {}
        Joining::Attacher(_) => panic!("the lock file outlived the writership that held it"),
    }
}

/// Two writerships over one directory hand out every number exactly once, and the numbers are
/// dense — a black box's numbering has no holes it did not record as gaps.
#[cfg(target_os = "linux")]
#[test]
fn a_creator_and_an_attacher_never_take_the_same_sequence() {
    let dir = TempDir::new("sequences");

    let creator = match Writership::join(dir.path(), "skynetd").unwrap() {
        Joining::Creator(creating) => creating.seed(1),
        Joining::Attacher(_) => panic!("fresh"),
    };
    let attacher = match Writership::join(dir.path(), "skynetk").unwrap() {
        Joining::Attacher(attaching) => attaching.attach().expect("the counter is seeded"),
        Joining::Creator(_) => panic!("held"),
    };
    assert_eq!(attacher.role(), Role::Attached);
    assert!(
        !attacher.may_seal(),
        "an attacher sealing would be a second reader of a file a writer is in the middle of",
    );

    // Interleaved rather than one after the other: the failure this guards against is two counters
    // in two processes, and a sequential test would pass against one counter per process.
    let mut taken = Vec::new();
    for _ in 0..500 {
        taken.push(creator.next().expect("a creator on Linux has a counter"));
        taken.push(attacher.next().expect("an attacher always has one"));
    }
    taken.sort_unstable();

    assert_eq!(
        taken,
        (1..=1000).collect::<Vec<u64>>(),
        "the two writers between them took 1..=1000, each exactly once",
    );
}

/// The seed is what the ring's scan says, so a recording that is resumed continues its numbering
/// instead of writing a second record 1 over the first.
#[cfg(target_os = "linux")]
#[test]
fn the_counter_starts_where_the_ring_left_off() {
    let dir = TempDir::new("resume");

    // A previous run wrote 4,000 records; `Ring::open`'s scan says the next one is 4,001.
    let creator = match Writership::join(dir.path(), "skynetd").unwrap() {
        Joining::Creator(creating) => creating.seed(4_001),
        Joining::Attacher(_) => panic!("fresh"),
    };
    assert_eq!(creator.peek(), Some(4_001));
    assert_eq!(creator.next(), Some(4_001));

    let attacher = match Writership::join(dir.path(), "skynetk").unwrap() {
        Joining::Attacher(attaching) => attaching.attach().unwrap(),
        Joining::Creator(_) => panic!("held"),
    };
    assert_eq!(
        attacher.next(),
        Some(4_002),
        "the attacher continues the recording rather than starting one",
    );
}

/// A holder that took the lock and never seeded is refused, with the wait named. This is the
/// pathological half of the race the bounded wait exists for; the ordinary half is the test above,
/// where the counter is already seeded and `attach` returns immediately.
#[cfg(target_os = "linux")]
#[test]
fn attaching_to_a_holder_that_never_seeded_refuses_rather_than_writing_at_zero() {
    let dir = TempDir::new("unseeded");

    // What a creator that died between `WriterLock::take` and `Creating::seed` leaves behind.
    fs::write(dir.path().join(crate::LOCK_FILE), "skynetd pid 4242\n").expect("writable");

    let attaching = match Writership::join(dir.path(), "skynetk").unwrap() {
        Joining::Attacher(attaching) => attaching,
        Joining::Creator(_) => panic!("the lock file is there"),
    };

    let error = attaching
        .attach_within(Duration::from_millis(30))
        .expect_err("an unseeded counter is not a recording to join");
    let text = error.to_string();
    assert!(text.contains("skynetd pid 4242"), "{text}");
    assert!(
        text.contains("safe to delete"),
        "a refusal that does not say how to clear it is a robot nobody can restart: {text}",
    );
}

/// Off Linux there is no mapping, so the second writer is refused exactly as it was before §26.2
/// was built — and the refusal says the mapping is why, rather than implying the other process is
/// at fault.
#[cfg(not(target_os = "linux"))]
#[test]
fn without_the_mapping_the_second_writer_is_refused_and_the_first_is_unaffected() {
    let dir = TempDir::new("unsupported");

    let creator = match Writership::join(dir.path(), "skynetd").unwrap() {
        Joining::Creator(creating) => creating.seed(1),
        Joining::Attacher(_) => panic!("fresh"),
    };
    assert_eq!(
        creator.next(),
        None,
        "a lone creator has no shared counter and appends through the ring's own, which is what \
         every version of this code before §26.2 did",
    );

    let attaching = match Writership::join(dir.path(), "skynetk").unwrap() {
        Joining::Attacher(attaching) => attaching,
        Joining::Creator(_) => panic!("held"),
    };
    let error = attaching
        .attach()
        .expect_err("no mapping, no second writer");
    let text = error.to_string();
    assert!(
        text.contains("skynetd"),
        "the refusal names the holder: {text}"
    );
    assert!(
        text.contains("Linux"),
        "and says the mapping is what is missing: {text}",
    );
}

/// Two **processes** over one directory, which is the thing the mapping exists for.
///
/// Every other test here is two `Writership`s inside one process, and that is a weaker claim than
/// it looks: two mappings in one address space could in principle be satisfied by anything shared
/// between them. §26.2 is about `skynetd` and `skynet-kernel-host`, which share nothing but a file.
///
/// The child is this same test binary, re-executed with an environment variable that makes it act
/// as the attacher and print the numbers it took. There is no second binary to build and no
/// fixture: the child does exactly what the daemon does, in a process the parent did not share an
/// allocator with.
#[cfg(target_os = "linux")]
#[test]
fn two_processes_over_one_directory_take_every_number_once() {
    const ROLE: &str = "SKYNET_WRITERSHIP_CHILD";
    const TAKE: u64 = 400;

    // The child half. Runs when the parent re-executes this binary with the variable set, and
    // returns before any test harness assertion so the process exits with what it printed.
    if let Ok(dir) = std::env::var(ROLE) {
        let attacher = match Writership::join(Path::new(&dir), "child").expect("joining") {
            Joining::Attacher(attaching) => attaching.attach().expect("the counter is seeded"),
            Joining::Creator(_) => {
                eprintln!("the parent did not hold the lock");
                std::process::exit(2);
            }
        };
        let mut mine = String::new();
        for _ in 0..TAKE {
            mine.push_str(&format!("{} ", attacher.next().expect("a number")));
        }
        // Marked, because the child is a test binary and prints a harness summary of its own —
        // and an unmarked parse read numbers out of "8 filtered out" the first time this ran.
        println!("CLAIMED {mine}");
        std::process::exit(0);
    }

    let dir = TempDir::new("processes");
    let creator = match Writership::join(dir.path(), "parent").unwrap() {
        Joining::Creator(creating) => creating.seed(1),
        Joining::Attacher(_) => panic!("fresh"),
    };

    let child = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .arg("writership::tests::two_processes_over_one_directory_take_every_number_once")
        .arg("--exact")
        .arg("--nocapture")
        .env(ROLE, dir.path())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("the child starts");

    // Claimed while the child is running, not before or after it: the numbers have to interleave or
    // this proves only that two processes can take turns.
    let mut ours: Vec<u64> = (0..TAKE)
        .map(|_| creator.next().expect("a number"))
        .collect();

    let output = child.wait_with_output().expect("the child finishes");
    assert!(
        output.status.success(),
        "the child failed: {}",
        String::from_utf8_lossy(&output.stdout),
    );
    let printed = String::from_utf8_lossy(&output.stdout);
    let theirs: Vec<u64> = printed
        .lines()
        .find(|line| line.starts_with("CLAIMED "))
        .unwrap_or_default()
        .split_whitespace()
        .filter_map(|word| word.parse().ok())
        .collect();
    assert_eq!(
        theirs.len(),
        TAKE as usize,
        "the child printed {} numbers, not {TAKE}: {printed}",
        theirs.len(),
    );

    ours.extend(theirs);
    ours.sort_unstable();
    assert_eq!(
        ours,
        (1..=TAKE * 2).collect::<Vec<u64>>(),
        "two processes took 1..={} between them, each number exactly once",
        TAKE * 2,
    );
}

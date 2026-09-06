//! What a free-space reading has to be true of, and the one thing it must never do.

use super::*;

/// The fraction is what §33.6 asks for: 0–1, and arithmetic rather than a claim.
#[test]
fn the_fraction_is_available_over_total() {
    let space = Space {
        available_bytes: 250,
        total_bytes: 1_000,
    };
    assert!((space.free_fraction() - 0.25).abs() < f64::EPSILON);

    let full = Space {
        available_bytes: 0,
        total_bytes: 1_000,
    };
    assert_eq!(full.free_fraction(), 0.0);
}

/// A real filesystem, measured. Everything below it is about failure; this is the one that says the
/// syscall was read correctly at all.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
#[test]
fn a_real_directory_measures_and_the_numbers_are_a_filesystem() {
    let space = space_of(&std::env::temp_dir()).expect("the temporary directory is on a volume");

    assert!(space.total_bytes > 0, "a filesystem has a size");
    assert!(
        space.available_bytes <= space.total_bytes,
        "{} available of {} total",
        space.available_bytes,
        space.total_bytes,
    );
    let fraction = space.free_fraction();
    assert!(
        (0.0..=1.0).contains(&fraction),
        "the headroom is a fraction: {fraction}",
    );

    // The reading has to be a *volume* rather than plausible-looking noise, and the cheapest
    // assertion that catches a mis-indexed structure is scale: no filesystem this OS runs on is
    // smaller than a mebibyte, and reading the wrong field gives a number nowhere near one.
    assert!(
        space.total_bytes >= 1024 * 1024,
        "a volume of {} bytes is a structure being read wrong, not a small disk",
        space.total_bytes,
    );

    // Printed so a person can hold it against `df -B1`, which is how this was first checked. The
    // **total** agreed to the byte, which is the number that says the structure is being read at
    // the right offsets; the available count differed by about two megabytes, because the two
    // readings were seconds apart on a live filesystem. An assertion cannot make that comparison —
    // `df` is not available to a test that must run anywhere — but a number nobody can see is a
    // number nobody checks.
    println!(
        "statfs: {} of {} bytes available ({:.1}% free)",
        space.available_bytes,
        space.total_bytes,
        space.free_fraction() * 100.0,
    );
}

/// Two directories on one filesystem measure the same volume — which is what makes this a
/// measurement of the *disk* rather than of a path.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
#[test]
fn two_paths_on_one_volume_report_one_size() {
    let root = std::env::temp_dir();
    let nested = root.join("skynet-statfs-nested");
    let _ = std::fs::create_dir_all(&nested);

    let outer = space_of(&root).expect("measured");
    let inner = space_of(&nested).expect("measured");
    assert_eq!(
        outer.total_bytes, inner.total_bytes,
        "a subdirectory is on the same filesystem as its parent",
    );

    let _ = std::fs::remove_dir_all(&nested);
}

/// A path that is not there is an error carrying its `errno`, not a zero.
///
/// The distinction is the whole point of the type: a robot that reported *zero bytes free* for a
/// directory that does not exist would be a robot whose retention rule deletes everything it holds.
#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
#[test]
fn a_missing_path_is_an_error_rather_than_no_space() {
    let error = space_of(Path::new("/skynet-no-such-directory-anywhere"))
        .expect_err("nothing is mounted there");
    match error {
        SpaceError::Call(errno) => assert_eq!(errno, 2, "ENOENT"),
        other => panic!("the wrong failure: {other}"),
    }
}

/// Off the supported platform there is no reading, and the refusal says which platform it wants.
#[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
#[test]
fn an_unsupported_platform_says_so_rather_than_answering() {
    let error = space_of(&std::env::temp_dir()).expect_err("not this platform");
    assert_eq!(error, SpaceError::Unsupported);
    assert!(
        error.to_string().contains("64-bit Linux"),
        "the refusal names what it needs: {error}",
    );
}

/// The plausibility rule, exercised on the values rather than through the syscall.
///
/// A `statfs` this code read wrongly produces numbers of exactly this kind, and the assertion worth
/// having is that each one alone is enough to refuse — a check that only caught two of them at once
/// would pass against an implementation that required all three.
#[test]
fn every_implausible_reading_is_refused_on_its_own() {
    assert!(
        plausible(4096, 1_000_000, 400_000),
        "an ordinary filesystem"
    );

    assert!(!plausible(0, 1_000_000, 400_000), "a block size of zero");
    assert!(!plausible(3000, 1_000_000, 400_000), "not a power of two");
    assert!(
        !plausible(1024 * 1024 * 8, 1_000_000, 400_000),
        "a block size larger than any filesystem uses is a field being read from the wrong offset",
    );
    assert!(!plausible(4096, 0, 0), "a filesystem of no blocks");
    assert!(
        !plausible(4096, 1_000, 2_000),
        "more available than exists, which is the signature of a mis-indexed structure",
    );
}

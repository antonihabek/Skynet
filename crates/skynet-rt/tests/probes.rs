//! The policy follows the limit, and a later mapping survives it.

use skynet_rt::{
    lock_if_unlimited, locked_kb, memlock_limit, parse_realtime, parse_swap_total, parse_vm_lck,
    preempt_rt, refuse_host, swap_kb,
};
use skynet_rt::{HostReadings, LockOutcome, MemlockLimit};

#[test]
fn vm_lck_is_kilobytes_or_nothing() {
    let status = "Name:\tskynetk\nVmLck:\t   12345 kB\nVmRSS:\t   67890 kB\n";
    assert_eq!(parse_vm_lck(status), Some(12345));
    // A unit misread is a budget misread in the reassuring direction, so the unit is checked.
    assert_eq!(parse_vm_lck("VmLck:\t   12345 mB\n"), None);
    assert_eq!(parse_vm_lck("VmLck:\t   12345\n"), None);
    assert_eq!(parse_vm_lck("Name:\tskynetk\n"), None);
}

#[test]
fn the_realtime_bit_is_one_bit() {
    assert_eq!(parse_realtime("1\n"), Some(true));
    assert_eq!(parse_realtime("0\n"), Some(false));
    assert_eq!(parse_realtime(""), None);
    assert_eq!(parse_realtime("yes\n"), None);
}

/// The outcome agrees with the limit, on any machine.
///
/// Whether the lock is taken depends on `RLIMIT_MEMLOCK`, which is the machine's — so the test
/// asserts the *agreement* instead: unlimited locks (and the probe shows locked bytes), finite
/// declines naming the same limit, unreadable declines. Every branch runs the real calls.
#[test]
fn the_policy_follows_the_limit() {
    match (lock_if_unlimited(), memlock_limit()) {
        (LockOutcome::Locked, MemlockLimit::Unlimited) => {
            let kb = locked_kb();
            assert!(
                kb.is_some_and(|kb| kb > 0),
                "the lock was taken and VmLck reads {kb:?}"
            );
        }
        (LockOutcome::Limited { bytes }, MemlockLimit::Limited(limit)) => {
            assert_eq!(bytes, limit, "the outcome names the limit it read");
        }
        (LockOutcome::Unknown, MemlockLimit::Unknown) => {}
        (LockOutcome::Failed(errno), MemlockLimit::Unlimited) => {
            panic!("the limit was unlimited and the lock still failed (errno {errno})")
        }
        (outcome, limit) => {
            panic!("the outcome disagrees with the limit: {outcome:?} vs {limit:?}")
        }
    }
}

/// The mapping that died, mapped after the policy.
///
/// The first version of this crate locked unconditionally, succeeded under a 64 MB shell limit,
/// and the loop then died mapping its black box ring — 67,108,632 bytes, on a machine with
/// fifteen gigabytes free (`DRY-COVERAGE-PLAN.md` F61). `MCL_FUTURE` under a finite limit fails
/// the mappings made afterwards, so the policy declines the lock instead, and this test maps
/// the same size after the policy runs and touches every page — on every limit, unlimited
/// included, where the lock really is taken and the mapping must still work.
///
/// What this test cannot do is reproduce the sabotage: the test binary maps far more than any
/// shell limit allows, so an unconditional call fails in here (safely) where it succeeded in
/// the small `skynetk` process (fatally). The sabotage's guard is the loop itself —
/// `check-loop` crashed against the unconditional call and closes against the policy — and
/// this test pins the other half: after the policy, a big mapping always works.
#[test]
fn a_later_mapping_survives_the_policy() {
    let _ = lock_if_unlimited();
    const BLACKBOX_BYTES: usize = 67_108_632;
    let mut ring = vec![0u8; BLACKBOX_BYTES];
    for page in ring.chunks_mut(4096) {
        page[0] = 1;
    }
    assert_eq!(ring.len(), BLACKBOX_BYTES);
    assert!(ring.iter().step_by(4096).all(|byte| *byte == 1));
}

#[test]
fn the_kernel_bit_is_read_whatever_it_is() {
    // No assertion on the value: this test runs on stock kernels and RT ones, and the one thing
    // both must do is answer without panicking.
    let _ = preempt_rt();
}

#[test]
fn swap_total_is_kilobytes_or_nothing() {
    let meminfo =
        "MemTotal:       16384000 kB\nSwapTotal:       2097148 kB\nSwapFree:        2097148 kB\n";
    assert_eq!(parse_swap_total(meminfo), Some(2097148));
    assert_eq!(parse_swap_total("SwapTotal:             0 kB\n"), Some(0));
    // Same unit discipline as VmLck: a misread unit is a budget misread.
    assert_eq!(parse_swap_total("SwapTotal:       2097148 mB\n"), None);
    assert_eq!(parse_swap_total("SwapTotal:       2097148\n"), None);
    assert_eq!(parse_swap_total("MemTotal:       16384000 kB\n"), None);
}

#[test]
fn the_swap_probe_answers_whatever_it_is() {
    let _ = swap_kb();
}

/// The refusal in both directions, on fixtures — including the allow direction, which no
/// machine in this repository's reach can produce live.
#[test]
fn the_host_claim_is_refused_for_what_was_read() {
    let robot = HostReadings {
        lock: LockOutcome::Locked,
        realtime: Some(true),
        swap_kb: Some(0),
    };
    assert!(refuse_host(&robot).is_empty(), "a robot backs the claim");

    let stock = HostReadings {
        realtime: Some(false),
        ..robot.clone()
    };
    let reasons = refuse_host(&stock);
    assert_eq!(reasons.len(), 1);
    assert!(reasons[0].contains("not PREEMPT_RT"), "{}", reasons[0]);
    assert!(reasons[0].contains("reads 0"), "{}", reasons[0]);

    let swapped = HostReadings {
        swap_kb: Some(2097148),
        ..robot.clone()
    };
    let reasons = refuse_host(&swapped);
    assert_eq!(reasons.len(), 1);
    assert!(reasons[0].contains("2097148"), "{}", reasons[0]);

    let shell = HostReadings {
        lock: LockOutcome::Limited { bytes: 67_108_864 },
        ..robot.clone()
    };
    let reasons = refuse_host(&shell);
    assert_eq!(reasons.len(), 1);
    assert!(reasons[0].contains("67108864"), "{}", reasons[0]);

    // Unknown is a reason, not a pass — F22's rule, at the gate.
    for (name, readings) in [
        (
            "lock",
            HostReadings {
                lock: LockOutcome::Unknown,
                ..robot.clone()
            },
        ),
        (
            "kernel",
            HostReadings {
                realtime: None,
                ..robot.clone()
            },
        ),
        (
            "swap",
            HostReadings {
                swap_kb: None,
                ..robot.clone()
            },
        ),
    ] {
        let reasons = refuse_host(&readings);
        assert_eq!(reasons.len(), 1, "an unknown {name} must refuse");
        assert!(
            reasons[0].contains("could not be read"),
            "an unknown {name} must say so: {}",
            reasons[0]
        );
    }

    // And a developer shell fails all three at once, naming each.
    let reasons = refuse_host(&HostReadings {
        lock: LockOutcome::Limited { bytes: 67_108_864 },
        realtime: Some(false),
        swap_kb: Some(1024),
    });
    assert_eq!(reasons.len(), 3);
}

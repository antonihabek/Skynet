//! The watchdog chain (§46).
//!
//! Four layers, each catching a failure the one above it cannot see:
//!
//! | # | Watchdog | Catches | Response | Tier |
//! |---|---|---|---|---|
//! | 1 | Kernel `watchdog` guard (§7.1) | The policy stopped producing commands | `stop_cat2` | All |
//! | 2 | systemd `WatchdogSec` on `skynetd` | The supervisor's reconcile loop hung | Restart `skynetd` | All |
//! | 3 | SoC hardware WDT | The application processor wedged entirely | **Reset the SoC** | Host, Certified |
//! | 4 | Safety MCU independent timer | The AP wedged *and* the reset did not recover it | **Cut drive enable** | Certified |
//!
//! Layer 3 is petted by the **Safety Kernel**, not by `skynetd`, and that is what makes it mean
//! anything: the WDT is satisfied only while the thing that actually enforces the envelope is
//! running its cycle. A watchdog petted by a supervisor tells you the supervisor is alive, which is
//! not the question anybody is asking.
//!
//! # A reset is not a stop
//!
//! §46.2's uncomfortable part, and the reason this module exists rather than a comment in the
//! kernel. When the SoC resets, the drives lose their command source, and what they then do is a
//! property of the **hardware**. Some hold. Some engage brakes. Some fault and disable. **Some
//! coast** — and on such a body, layer 3's own recovery path is a fall.
//!
//! So [`host_blocked_by_command_loss`] refuses the host tier for a coasting body, and the shape of
//! that refusal is unusual and correct: **the middle rung is the unsafe one.** Guest is fine —
//! enforcement is advisory and nothing relies on a hardware watchdog. Certified is fine — layer 4
//! cuts drive enable independently of the processor being reset. Only host, where the OS owns the
//! control loop and layer 3 is the last line, is the configuration where a reset means a fall.
//!
//! A tier ladder that only ever caps downward could not express that, which is why this is a
//! predicate rather than a ceiling.
//!
//! # Window nesting (§46.3)
//!
//! ```text
//! kernel cycle worst case  <  envelope.watchdog_ms  <  SoC WDT window  <  drive command timeout
//! ```
//!
//! Getting the order wrong makes an upper layer unreachable, and the two ways it goes wrong are
//! different bugs with different remedies:
//!
//! * **SoC WDT shorter than the kernel's window** — the processor resets before the kernel ever
//!   executes its controlled stop. The harsher recovery pre-empts the gentler one, so the robot's
//!   *best* behaviour is the one it never gets to perform.
//! * **SoC WDT longer than the drive's timeout** — the drive faults first and the WDT never
//!   matters. Layer 3 is present, configured, and dead.
//!
//! Computed at boot from the descriptor and the resolved limits, so *"a body whose declared figures
//! cannot be nested is reported at startup rather than discovered in the field"*.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::json::number_text;

/// §46.2's vocabulary, in the order the specification lists it.
pub const COMMAND_LOSS_BEHAVIOURS: [&str; 4] = ["hold", "brake", "fault", "coast"];

/// One layer of §46.1's chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchdogLayer {
    /// 1 to 4.
    pub layer: u8,
    /// Wire name.
    pub name: &'static str,
    /// What only this layer can see.
    pub catches: &'static str,
    /// What it does about it.
    pub response: &'static str,
    /// The lowest tier at which this layer is required.
    pub required_from: &'static str,
    /// Who satisfies it. Layer 3's answer is the interesting one.
    pub petted_by: &'static str,
}

/// The chain, as data (§46.1).
pub const CHAIN: [WatchdogLayer; 4] = [
    WatchdogLayer {
        layer: 1,
        name: "kernel_guard",
        catches: "the policy stopped producing commands",
        response: "stop_cat2",
        required_from: "guest",
        petted_by: "the policy runtime, every cycle",
    },
    WatchdogLayer {
        layer: 2,
        name: "supervisor",
        catches: "the supervisor's reconcile loop hung",
        response: "restart skynetd",
        required_from: "guest",
        petted_by: "skynetd, from inside the reconcile loop",
    },
    WatchdogLayer {
        layer: 3,
        name: "soc_wdt",
        catches: "the application processor wedged entirely",
        response: "reset the SoC",
        required_from: "host",
        // §46.1's sentence, and the reason layer 3 means anything.
        petted_by: "the Safety Kernel, not skynetd",
    },
    WatchdogLayer {
        layer: 4,
        name: "safety_mcu",
        catches: "the AP wedged and the reset did not recover it",
        response: "cut drive enable",
        required_from: "certified",
        petted_by: "an independent timer on the safety MCU",
    },
];

/// Whether a body may attempt the host tier, given what it says its drives do (§46.2).
///
/// Returns the reason it may not, or `None` when it may.
///
/// Two refusals, and they are the same refusal: **the recovery path has not been established to be
/// survivable.** A body that coasts has one that demonstrably is not; a body that has declared
/// nothing has one nobody has characterised, and treating silence as `hold` would be inventing the
/// most convenient answer to the only question layer 3 turns on.
pub fn host_blocked_by_command_loss(on_command_loss: Option<&str>) -> Option<String> {
    match on_command_loss {
        Some("coast") => Some(
            "this body's drives coast when commands stop, and layer 3 of the watchdog chain \
             recovers by resetting the processor — which on this machine is a fall. The sound \
             configuration is the certified tier, where layer 4 cuts drive enable independently of \
             the processor (§46.2)"
                .to_string(),
        ),
        Some(behaviour) if !COMMAND_LOSS_BEHAVIOURS.contains(&behaviour) => Some(format!(
            "this body declares `on_command_loss: {behaviour}`, which is not one of hold, brake, \
             fault or coast — so what it does while its processor resets is not something anybody \
             can read off the descriptor (§46.2)"
        )),
        None => Some(
            "this body does not declare what its drives do when commands stop, and layer 3 of the \
             watchdog chain recovers by resetting the processor. Silence is not `hold`: it is a \
             recovery path nobody has characterised (§46.2)"
                .to_string(),
        ),
        Some(_) => None,
    }
}

/// The four windows §46.3 nests, in order, innermost first.
pub const WINDOWS: [&str; 4] = [
    "kernel_cycle_worst_case_ms",
    "envelope_watchdog_ms",
    "soc_wdt_ms",
    "drive_command_timeout_ms",
];

/// What a body and its limits declare about the four windows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Windows {
    /// The kernel's own worst-case cycle, measured or declared.
    pub kernel_cycle_worst_case_ms: Option<f64>,
    /// `envelope.watchdog_ms` — the command deadline the kernel enforces.
    pub envelope_watchdog_ms: Option<f64>,
    /// The SoC hardware watchdog's window.
    pub soc_wdt_ms: Option<f64>,
    /// The drive's own command timeout, past which it faults on its own.
    pub drive_command_timeout_ms: Option<f64>,
}

impl Windows {
    /// The four, in nesting order, paired with their names.
    pub fn ordered(&self) -> [(&'static str, Option<f64>); 4] {
        [
            (WINDOWS[0], self.kernel_cycle_worst_case_ms),
            (WINDOWS[1], self.envelope_watchdog_ms),
            (WINDOWS[2], self.soc_wdt_ms),
            (WINDOWS[3], self.drive_command_timeout_ms),
        ]
    }
}

/// One way the nesting is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NestingFault {
    /// A code a console can translate.
    pub code: &'static str,
    /// The inner window, which must be the smaller.
    pub inner: &'static str,
    /// The outer window.
    pub outer: &'static str,
    /// The sentence, naming both numbers and what the inversion costs.
    pub detail: String,
}

/// What the nesting check concluded.
#[derive(Debug, Clone, PartialEq)]
pub struct NestingReport {
    /// Every inversion found, innermost pair first.
    pub faults: Vec<NestingFault>,
    /// Windows nobody declared, in nesting order.
    pub missing: Vec<&'static str>,
    /// The pairs actually compared, so a caller can tell "nested" from "nothing to compare".
    pub checked: Vec<(&'static str, &'static str)>,
}

impl NestingReport {
    /// Whether every pair that could be compared nests.
    ///
    /// **Not the same as "the chain is sound"** — an undeclared window is not a passing one, which
    /// is why [`NestingReport::missing`] is separate and why nothing here returns a bare boolean.
    pub fn ordered(&self) -> bool {
        self.faults.is_empty()
    }

    /// Whether every window is declared *and* every pair nests.
    pub fn complete(&self) -> bool {
        self.faults.is_empty() && self.missing.is_empty()
    }
}

/// Check §46.3's nesting.
///
/// Adjacent pairs only. Comparing every pair would report the same inversion up to three times and
/// bury the one an operator has to fix — and the transitive consequences follow from the adjacent
/// ones anyway.
///
/// **Strictly less than**, not less-than-or-equal. Two watchdogs with the same window is a race
/// whose winner is decided by interrupt latency, and a chain whose order depends on which timer
/// fires first is not a chain.
///
/// A gap in the middle does not stop the walk: with no `envelope_watchdog_ms` declared, the kernel
/// cycle and the SoC window are still worth comparing, because that inversion is real whatever sits
/// between them. So a missing window is skipped over rather than ending the check.
pub fn check_nesting(windows: &Windows) -> NestingReport {
    let ordered = windows.ordered();
    let mut faults = Vec::new();
    let mut missing = Vec::new();
    let mut checked = Vec::new();

    for (name, value) in ordered.iter() {
        if value.is_none() {
            missing.push(*name);
        }
    }

    // Walk the declared windows in order, comparing each with the next declared one.
    let declared: Vec<(&'static str, f64)> = ordered
        .iter()
        .filter_map(|(name, value)| value.map(|v| (*name, v)))
        .collect();

    for pair in declared.windows(2) {
        let (inner_name, inner) = pair[0];
        let (outer_name, outer) = pair[1];
        checked.push((inner_name, outer_name));
        if inner < outer {
            continue;
        }
        faults.push(NestingFault {
            code: nesting_code(inner_name, outer_name),
            inner: inner_name,
            outer: outer_name,
            detail: format!(
                "{inner_name} is {} ms and {outer_name} is {} ms, so the outer window is not \
                 outside the inner one. {}",
                number_text(inner),
                number_text(outer),
                nesting_consequence(inner_name, outer_name)
            ),
        });
    }

    NestingReport {
        faults,
        missing,
        checked,
    }
}

/// A code per inversion, so a console can say something specific rather than "windows wrong".
fn nesting_code(inner: &str, outer: &str) -> &'static str {
    match (inner, outer) {
        ("kernel_cycle_worst_case_ms", "envelope_watchdog_ms") => "cycle_exceeds_watchdog",
        ("envelope_watchdog_ms", "soc_wdt_ms") => "wdt_pre_empts_kernel",
        ("soc_wdt_ms", "drive_command_timeout_ms") => "drive_faults_first",
        _ => "windows_not_nested",
    }
}

/// What the inversion actually costs, which is the half worth reading.
fn nesting_consequence(inner: &str, outer: &str) -> &'static str {
    match (inner, outer) {
        ("kernel_cycle_worst_case_ms", "envelope_watchdog_ms") => {
            "The kernel cannot complete a cycle inside its own command deadline, so the watchdog \
             guard trips on a robot that is working correctly."
        }
        ("envelope_watchdog_ms", "soc_wdt_ms") => {
            "The processor resets before the kernel executes its controlled stop, so the harsher \
             recovery pre-empts the gentler one and the robot never performs its best available \
             behaviour (§46.3)."
        }
        ("soc_wdt_ms", "drive_command_timeout_ms") => {
            "The drive faults on its own before the hardware watchdog fires, so layer 3 is present, \
             configured, and never reached (§46.3)."
        }
        _ => "The windows must nest innermost-first for every layer above to be reachable.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn windows(kernel: f64, envelope: f64, wdt: f64, drive: f64) -> Windows {
        Windows {
            kernel_cycle_worst_case_ms: Some(kernel),
            envelope_watchdog_ms: Some(envelope),
            soc_wdt_ms: Some(wdt),
            drive_command_timeout_ms: Some(drive),
        }
    }

    #[test]
    fn a_correctly_nested_chain_reports_nothing() {
        let report = check_nesting(&windows(2.0, 100.0, 250.0, 500.0));
        assert!(report.ordered());
        assert!(report.complete());
        assert!(report.missing.is_empty());
        assert_eq!(report.checked.len(), 3);
    }

    /// §46.3's first inversion: the harsher recovery pre-empts the gentler one, so the robot's best
    /// available behaviour is the one it never gets to perform.
    #[test]
    fn a_wdt_shorter_than_the_kernel_window_is_named_with_its_consequence() {
        let report = check_nesting(&windows(2.0, 250.0, 100.0, 500.0));
        assert!(!report.ordered());
        let fault = &report.faults[0];
        assert_eq!(fault.code, "wdt_pre_empts_kernel");
        assert_eq!(fault.inner, "envelope_watchdog_ms");
        assert!(fault.detail.contains("250"), "{fault:?}");
        assert!(fault.detail.contains("100"), "{fault:?}");
        assert!(
            fault
                .detail
                .contains("never performs its best available behaviour")
                || fault.detail.contains("pre-empts the gentler"),
            "{fault:?}"
        );
    }

    /// And the second: layer 3 present, configured, and never reached.
    #[test]
    fn a_wdt_longer_than_the_drive_timeout_is_a_dead_layer() {
        let report = check_nesting(&windows(2.0, 100.0, 900.0, 500.0));
        let fault = &report.faults[0];
        assert_eq!(fault.code, "drive_faults_first");
        assert!(fault.detail.contains("never reached"), "{fault:?}");
    }

    #[test]
    fn a_cycle_that_cannot_meet_its_own_deadline_is_reported() {
        let report = check_nesting(&windows(120.0, 100.0, 250.0, 500.0));
        assert_eq!(report.faults[0].code, "cycle_exceeds_watchdog");
        assert!(
            report.faults[0].detail.contains("working correctly"),
            "{:?}",
            report.faults[0]
        );
    }

    /// Equal windows are a race whose winner is interrupt latency, and a chain whose order depends
    /// on which timer fires first is not a chain.
    #[test]
    fn equal_windows_do_not_nest() {
        let report = check_nesting(&windows(2.0, 100.0, 100.0, 500.0));
        assert!(!report.ordered());
        assert_eq!(report.faults[0].inner, "envelope_watchdog_ms");
    }

    /// An undeclared window is not a passing one, and it does not stop the walk either: the pair on
    /// either side of the gap is still worth comparing, because that inversion is real whatever
    /// sits between them.
    #[test]
    fn a_gap_is_reported_and_skipped_over_rather_than_ending_the_check() {
        let report = check_nesting(&Windows {
            kernel_cycle_worst_case_ms: Some(2.0),
            envelope_watchdog_ms: None,
            soc_wdt_ms: Some(1.0),
            drive_command_timeout_ms: Some(500.0),
        });
        assert_eq!(report.missing, vec!["envelope_watchdog_ms"]);
        assert!(!report.complete(), "a missing window is not a pass");
        // The kernel cycle and the SoC window were still compared, and they invert.
        assert_eq!(report.faults.len(), 1);
        assert_eq!(report.faults[0].inner, "kernel_cycle_worst_case_ms");
        assert_eq!(report.faults[0].outer, "soc_wdt_ms");
    }

    #[test]
    fn a_body_that_declares_nothing_reports_every_window_missing() {
        let report = check_nesting(&Windows::default());
        assert_eq!(report.missing.len(), 4);
        assert!(report.checked.is_empty());
        // Ordered and not complete: nothing contradicts, and nothing was established.
        assert!(report.ordered());
        assert!(!report.complete());
    }

    /// §46.2, and the shape of it is the point: **the middle rung is the unsafe one.**
    #[test]
    fn a_coasting_body_is_refused_host_and_nothing_else() {
        let blocked = host_blocked_by_command_loss(Some("coast")).expect("refused");
        assert!(blocked.contains("is a fall"), "{blocked}");
        assert!(blocked.contains("certified"), "{blocked}");

        for safe in ["hold", "brake", "fault"] {
            assert_eq!(host_blocked_by_command_loss(Some(safe)), None, "{safe}");
        }
    }

    /// Silence is not `hold`. Treating it as `hold` would be inventing the most convenient answer to
    /// the only question layer 3 turns on.
    #[test]
    fn an_undeclared_behaviour_blocks_host_too_and_says_why() {
        let blocked = host_blocked_by_command_loss(None).expect("refused");
        assert!(blocked.contains("Silence is not `hold`"), "{blocked}");
        assert!(blocked.contains("nobody has characterised"), "{blocked}");
    }

    /// A value outside the vocabulary is its own sentence: "this body says `freewheel`" and "this
    /// body says nothing" are different facts with different remedies.
    #[test]
    fn an_unrecognised_behaviour_is_not_reported_as_silence() {
        let blocked = host_blocked_by_command_loss(Some("freewheel")).expect("refused");
        assert!(blocked.contains("freewheel"), "{blocked}");
        assert!(!blocked.contains("Silence"), "{blocked}");
    }

    /// The chain is data, and every row of it has to say something an operator can act on. Layer 3's
    /// `petted_by` is the one that carries §46.1's whole argument.
    #[test]
    fn the_chain_is_four_layers_and_the_soc_wdt_is_petted_by_the_kernel() {
        assert_eq!(CHAIN.len(), 4);
        for (index, layer) in CHAIN.iter().enumerate() {
            assert_eq!(layer.layer as usize, index + 1);
            assert!(!layer.catches.is_empty());
            assert!(!layer.response.is_empty());
        }
        let soc = CHAIN[2];
        assert_eq!(soc.name, "soc_wdt");
        assert_eq!(soc.required_from, "host");
        assert!(soc.petted_by.contains("Safety Kernel"), "{soc:?}");
        assert!(soc.petted_by.contains("not skynetd"), "{soc:?}");
        assert_eq!(CHAIN[3].required_from, "certified");
    }
}

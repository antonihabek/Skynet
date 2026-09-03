//! §36 on a real machine: reading the gauge and the thermal zones.
//!
//! [`skynet_contracts::power`] decides what a reading *means* — the bands, the reserve, the ladder,
//! the brownout override — and takes no clock, no filesystem and no sysfs. This is the other half:
//! where the numbers come from on a robot, and what is honestly absent when they do not.
//!
//! # Where each field comes from
//!
//! | Field | Source |
//! |---|---|
//! | `energy_remaining_wh` | `/sys/class/power_supply/<pack>/energy_now`, µWh |
//! | `state_of_charge` | `/sys/class/power_supply/<pack>/capacity`, per cent — only when there is no `energy_now` |
//! | `temperature_c` | the hottest `/sys/class/thermal/thermal_zone*/temp`, millidegrees |
//! | `throttled_for_ms` | this module, from the instant the temperature first crossed the threshold |
//! | `rhal_degraded`, `fault_code` | **nothing** — see below |
//!
//! # Which supply is the pack
//!
//! `/sys/class/power_supply` holds mains adapters, USB ports and batteries, and each entry declares
//! its own `type`. So the batteries can be found without guessing, and the ambiguity that remains is
//! the real one: **a machine with two batteries has not said which is the traction pack.**
//!
//! * No battery — no reading. Whether that is a fault is the descriptor's business (§36.1): a body
//!   that declares no pack is a mains machine behaving correctly.
//! * Exactly one — that is the pack.
//! * More than one — no reading, and the reason names them. Summing them would be a guess about
//!   whether the second one is a backup cell, a tool battery or half of one pack, and §36.2's
//!   reserve is compared against the answer.
//!
//! `state/power.json` resolves the last case by naming the supply, which is the same shape §34.3's
//! bearers use and for the same reason: the parts a machine can measure are measured, the parts it
//! cannot are declared, and every surface says which is which.
//!
//! # Why the thermal zones need no declaration and the pack does
//!
//! §36.3 asks for "the hottest thermal zone", and the maximum over every zone the kernel exposes
//! **is** that, whatever each zone is a temperature of. There is no question here to guess the
//! answer to. Which of two batteries drives the legs is a different kind of question, and this
//! module does not answer those.
//!
//! # What it cannot read, and will not pretend to
//!
//! `rhal_degraded` and `fault_code` are §36.4's whole input, and they come from the RHAL. §33.1
//! keeps this daemon off that path — `skynetd-io` does not depend on `skynet-rhal` and CI asserts
//! the edge is absent — so on this edition the brownout override has no source and never fires.
//!
//! That is the same gap §48.2's self-test has, recorded the same way: named in §73's last column
//! rather than papered over with a `false` that looks like an observation. A `false` here is *no
//! RHAL flag was observed*, and the difference matters because the contract's answer to a degraded
//! RHAL with no declared codes is a fault an operator should be seeing.

use std::path::{Path, PathBuf};

use skynet_json::Json;

use skynet_contracts::power::{PowerObservation, PowerProfile};

/// Where the kernel puts power supplies.
pub const POWER_SUPPLY_ROOT: &str = "/sys/class/power_supply";

/// Where the kernel puts thermal zones.
pub const THERMAL_ROOT: &str = "/sys/class/thermal";

/// What could be read, and what could not.
///
/// A reading and its reasons travel together rather than the reasons being logged and dropped: an
/// operator looking at an absent battery on a fleet page needs the sentence, and the sentence is
/// produced here, once, where the directory listing is in hand.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    /// The observation, for [`skynet_contracts::power::assess`].
    pub observation: PowerObservation,
    /// Why a field is absent, in the order the fields are listed above. Empty when everything the
    /// host exposes was read.
    pub notes: Vec<String>,
    /// The supply that was read, for a console that has to say which one.
    pub supply: Option<String>,
    /// How many thermal zones contributed to the maximum.
    pub zones: usize,
    /// The state of charge as a fraction, for §33.6's `battery` column.
    ///
    /// §33.6 wants a fraction — it is what a fleet page renders as a bar — and a gauge may have
    /// reported watt-hours instead. So a watt-hour reading is converted here against the declared
    /// `pack_capacity_wh`, and a body with no declared capacity reports **nothing** rather than a
    /// watt-hour figure in a column labelled with a percentage. A column whose unit depends on
    /// which robot filled it in is a column nobody can sort.
    ///
    /// Computed at the read rather than by the caller, because the profile is in hand here and is
    /// not in hand in [`crate::checkin::health`] — and a rollup that had to fetch a descriptor to
    /// name a percentage would be the descriptor read from a third place.
    pub charge_fraction: Option<f64>,
}

/// The one piece of state §36 needs across cycles.
///
/// `throttled_for_ms` is a duration, and a duration is not observable in a single reading. So the
/// instant the temperature first crossed the throttle threshold is held here and cleared the moment
/// it drops back under — which makes "sustained" mean *continuously* rather than *cumulatively*, and
/// that is the reading §36.3 wants: a body that crosses the line for a second every minute is not a
/// body that has been throttling for an hour.
#[derive(Debug, Clone, Default)]
pub struct Sensors {
    /// Where the kernel's power supplies are. Overridable so the reader is testable.
    root: Option<PathBuf>,
    /// Where the kernel's thermal zones are.
    thermal_root: Option<PathBuf>,
    /// When the temperature first went above the throttle threshold and stayed there.
    throttling_since_ms: Option<u64>,
}

impl Sensors {
    /// Read from the real kernel interfaces.
    pub fn new() -> Self {
        Self::default()
    }

    /// Read from a tree somewhere else, which is how this is tested without a battery.
    pub fn rooted(power_supply: impl Into<PathBuf>, thermal: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(power_supply.into()),
            thermal_root: Some(thermal.into()),
            throttling_since_ms: None,
        }
    }

    fn supply_root(&self) -> &Path {
        self.root
            .as_deref()
            .unwrap_or_else(|| Path::new(POWER_SUPPLY_ROOT))
    }

    fn thermal(&self) -> &Path {
        self.thermal_root
            .as_deref()
            .unwrap_or_else(|| Path::new(THERMAL_ROOT))
    }

    /// Take one reading.
    ///
    /// `profile` is consulted for one thing only — the throttle threshold, which decides whether
    /// the clock in [`Self::throttling_since_ms`] is running. Everything else the profile says is
    /// the contract's business, and mixing the two here would put §36.1's bands in two places.
    pub fn read(&mut self, profile: &PowerProfile, declared: &Declared, now_ms: u64) -> Reading {
        let mut notes = Vec::new();
        let (charge, energy, supply) = self.gauge(declared, &mut notes);
        let (temperature, zones) = self.temperature(&mut notes);

        // The clock, and the only line in this module that remembers anything.
        let above = match (temperature, profile.thermal_throttle_c) {
            (Some(reading), Some(threshold)) => reading >= threshold,
            _ => false,
        };
        if above {
            self.throttling_since_ms.get_or_insert(now_ms);
        } else {
            self.throttling_since_ms = None;
        }

        let charge_fraction = match (charge, energy) {
            (Some(fraction), _) => Some(fraction),
            (None, Some(watt_hours)) => profile
                .pack_capacity_wh
                .filter(|value| value.is_finite() && *value > 0.0)
                .map(|capacity| watt_hours / capacity),
            (None, None) => None,
        };

        Reading {
            charge_fraction,
            observation: PowerObservation {
                state_of_charge: charge,
                energy_remaining_wh: energy,
                temperature_c: temperature,
                // `saturating_sub` rather than a subtraction, because §35.1 keeps two clocks and a
                // caller passing a `now_ms` behind the one that started this window is a caller
                // whose clock was corrected mid-throttle. Zero is the honest answer to that; a
                // wrapped `u64` is eighteen quintillion milliseconds of sustained throttling.
                throttled_for_ms: self
                    .throttling_since_ms
                    .map_or(0, |since| now_ms.saturating_sub(since)),
                // §33.1. See the module header: no RHAL from here, so no brownout on this edition.
                rhal_degraded: false,
                fault_code: None,
            },
            notes,
            supply,
            zones,
        }
    }

    /// The fuel gauge, or the reason there is none.
    fn gauge(
        &self,
        declared: &Declared,
        notes: &mut Vec<String>,
    ) -> (Option<f64>, Option<f64>, Option<String>) {
        let batteries = match batteries(self.supply_root()) {
            Ok(found) => found,
            Err(reason) => {
                notes.push(reason);
                return (None, None, None);
            }
        };

        let chosen = match (declared.supply.as_deref(), batteries.len()) {
            (Some(named), _) => {
                if batteries.iter().any(|name| name == named) {
                    named.to_string()
                } else {
                    notes.push(format!(
                        "`state/power.json` names `{named}` as the traction pack and \
                         {root} has no battery by that name. Found: {found}. Nothing is read \
                         rather than a different supply being substituted — the declaration is the \
                         only thing that says which pack §36.2's reserve is about.",
                        root = self.supply_root().display(),
                        found = list(&batteries),
                    ));
                    return (None, None, None);
                }
            }
            (None, 0) => {
                notes.push(format!(
                    "{} exposes no battery. Whether that is a fault is the descriptor's to say \
                     (§36.1): a body that declares no pack is a mains machine behaving correctly.",
                    self.supply_root().display()
                ));
                return (None, None, None);
            }
            (None, 1) => batteries[0].clone(),
            (None, _) => {
                notes.push(format!(
                    "this machine exposes {n} batteries — {found} — and nothing says which is the \
                     traction pack. Summing them would be a guess about whether the second is a \
                     backup cell, a tool battery or half of one pack, and §36.2's reserve is \
                     compared against the answer. Name it in `state/power.json`.",
                    n = batteries.len(),
                    found = list(&batteries),
                ));
                return (None, None, None);
            }
        };

        let directory = self.supply_root().join(&chosen);

        // Energy first, and it is the one that needs nothing from the descriptor. A gauge reporting
        // watt-hours has been told the pack's capacity by something closer to the pack than a
        // descriptor field is.
        if let Some(micro_wh) = number(&directory.join("energy_now")) {
            return (None, Some(micro_wh / 1_000_000.0), Some(chosen));
        }
        // Then the fraction, which the contract can only use with a declared `pack_capacity_wh` —
        // and says so, in the fault `capacity_undeclared`. Reported rather than converted here:
        // `charge_now` × `voltage_now` is watt-hours computed from a voltage that sags under load,
        // which under-reads exactly when a robot is working hardest. Better a named gap than a
        // number whose error moves with the thing it is measuring.
        if let Some(percent) = number(&directory.join("capacity")) {
            return (Some(percent / 100.0), None, Some(chosen));
        }

        notes.push(format!(
            "battery `{chosen}` reports neither `energy_now` nor `capacity`, so nothing on this \
             machine says how much energy is left. A gauge that exposes only `charge_now` and \
             `voltage_now` is not converted here: that product is watt-hours computed from a \
             voltage that sags under load, which under-reads exactly when the robot is working \
             hardest."
        ));
        (None, None, Some(chosen))
    }

    /// The hottest zone, and how many were read.
    fn temperature(&self, notes: &mut Vec<String>) -> (Option<f64>, usize) {
        let Ok(entries) = std::fs::read_dir(self.thermal()) else {
            notes.push(format!(
                "{} could not be listed, so no thermal zone was read. §36.3's ladder gives up \
                 nothing on a body whose temperature nobody knows, which is the correct direction \
                 and not a statement that it is cool.",
                self.thermal().display()
            ));
            return (None, 0);
        };

        let mut hottest: Option<f64> = None;
        let mut zones = 0usize;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with("thermal_zone") {
                continue;
            }
            // Millidegrees, which is the unit this interface has always used. A zone that cannot be
            // read is skipped rather than failing the lot: one unreadable zone on a machine with
            // twelve should not cost the other eleven, and the count says how many contributed.
            let Some(milli) = number(&entry.path().join("temp")) else {
                continue;
            };
            zones += 1;
            let celsius = milli / 1000.0;
            hottest = Some(match hottest {
                Some(previous) if previous >= celsius => previous,
                _ => celsius,
            });
        }

        if zones == 0 {
            notes.push(format!(
                "{} exposes no readable thermal zone, so §36.3 has no temperature to work from.",
                self.thermal().display()
            ));
        }
        (hottest, zones)
    }
}

/// The profile this body declares, or an empty one.
///
/// A body that is absent or malformed declares nothing about power, which is the same input as a
/// body that declared nothing — and both produce [`skynet_contracts::power::PowerState::Unmetered`]
/// with no fault, correctly: the descriptor is what would have said there was a pack to be missing,
/// and a robot with no descriptor is not a robot with a flat battery. The *descriptor's* absence is
/// reported by the routes that report descriptors; repeating it as a power fault would be the same
/// problem in two columns.
pub fn profile_of(body: &crate::embodiment::Body) -> PowerProfile {
    match body {
        crate::embodiment::Body::Present(known) => {
            known.descriptor.power.clone().unwrap_or_default()
        }
        _ => PowerProfile::default(),
    }
}

/// Take one reading against the body's declared profile, in one call.
///
/// The two are always wanted together and fetching them apart is how a caller ends up assessing
/// this cycle's temperature against a profile it read three passes ago.
pub fn observe(
    sensors: &mut Sensors,
    layout: &crate::layout::Layout,
    body: &crate::embodiment::Body,
    now_ms: u64,
) -> (PowerProfile, Reading) {
    let profile = profile_of(body);
    let declared = Declared::read(&layout.power());
    let reading = sensors.read(&profile, &declared, now_ms);
    (profile, reading)
}

/// What an operator has declared about this machine's power hardware.
///
/// One field today, and the file exists because of it: a machine with two batteries cannot be
/// resolved by measurement. Everything else here is read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Declared {
    /// The `/sys/class/power_supply` entry that is the traction pack.
    pub supply: Option<String>,
}

impl Declared {
    /// Read `state/power.json`, or nothing.
    ///
    /// A missing file is not an error and never has been on this daemon: a robot with one battery
    /// needs no declaration, which is most robots and every development host.
    pub fn read(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(document) = Json::parse(&text) else {
            return Self::default();
        };
        Self {
            supply: document
                .get("supply")
                .and_then(Json::as_str)
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_string),
        }
    }
}

/// Every `/sys/class/power_supply` entry whose `type` is `Battery`, sorted.
///
/// Sorted so that a machine with two batteries names them the same way on every boot — the same
/// reason §34.3's bearers sort. A listing order that changed after a reboot would change which
/// sentence an operator read for no reason anybody could see.
fn batteries(root: &Path) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(root).map_err(|error| {
        format!(
            "{} could not be listed ({error}), so nothing on this machine says whether it has a \
             battery. §23.5 says a robot runs Linux; on a host without this interface `skynetd` \
             runs for development and §36 has no input.",
            root.display()
        )
    })?;

    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(kind) = std::fs::read_to_string(entry.path().join("type")) else {
            continue;
        };
        if kind.trim() == "Battery" {
            found.push(name.to_string());
        }
    }
    found.sort();
    Ok(found)
}

/// One number out of a sysfs file, or nothing.
///
/// Every file this module reads holds a single integer and a newline. A file that holds anything
/// else is treated as absent rather than as zero, which is the rule the whole platform applies to a
/// reading it could not take.
fn number(path: &Path) -> Option<f64> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: f64 = text.trim().parse().ok()?;
    value.is_finite().then_some(value)
}

/// A list of names, for a sentence.
fn list(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<String>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TempRoot;

    struct Machine {
        root: TempRoot,
    }

    impl Machine {
        fn new(label: &str) -> Self {
            let root = TempRoot::new(label);
            std::fs::create_dir_all(root.path().join("power_supply")).unwrap();
            std::fs::create_dir_all(root.path().join("thermal")).unwrap();
            Self { root }
        }

        fn supply(&self, name: &str, kind: &str, files: &[(&str, &str)]) -> &Self {
            let directory = self.root.path().join("power_supply").join(name);
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("type"), kind).unwrap();
            for (file, value) in files {
                std::fs::write(directory.join(file), value).unwrap();
            }
            self
        }

        fn zone(&self, index: usize, milli: &str) -> &Self {
            let directory = self
                .root
                .path()
                .join("thermal")
                .join(format!("thermal_zone{index}"));
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(directory.join("temp"), milli).unwrap();
            self
        }

        fn sensors(&self) -> Sensors {
            Sensors::rooted(
                self.root.path().join("power_supply"),
                self.root.path().join("thermal"),
            )
        }
    }

    fn profile() -> PowerProfile {
        PowerProfile {
            safe_stop_energy_wh: Some(20.0),
            pack_capacity_wh: Some(500.0),
            thermal_throttle_c: Some(70.0),
            thermal_critical_c: Some(90.0),
            ..Default::default()
        }
    }

    // ── The gauge ────────────────────────────────────────────────────────────

    #[test]
    fn one_battery_reporting_energy_needs_nothing_from_the_descriptor() {
        let machine = Machine::new("power-energy");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000\n")]);
        machine.supply("AC", "Mains", &[]);
        let reading = machine
            .sensors()
            .read(&profile(), &Declared::default(), 1_000);

        assert_eq!(reading.observation.energy_remaining_wh, Some(45.0));
        assert_eq!(
            reading.observation.state_of_charge, None,
            "the fraction is not also reported: two spellings of one reading is two readings that \
             can disagree"
        );
        assert_eq!(reading.supply.as_deref(), Some("BAT0"));
    }

    #[test]
    fn a_gauge_with_only_a_percentage_reports_a_fraction() {
        let machine = Machine::new("power-percent");
        machine.supply("BAT0", "Battery", &[("capacity", "62\n")]);
        let reading = machine
            .sensors()
            .read(&profile(), &Declared::default(), 1_000);

        assert_eq!(reading.observation.state_of_charge, Some(0.62));
        assert_eq!(reading.observation.energy_remaining_wh, None);
    }

    #[test]
    fn a_mains_only_machine_reads_no_gauge_and_says_it_is_the_descriptors_business() {
        let machine = Machine::new("power-mains");
        machine.supply("AC", "Mains", &[]);
        let reading = machine
            .sensors()
            .read(&profile(), &Declared::default(), 1_000);

        assert_eq!(reading.observation.state_of_charge, None);
        assert_eq!(reading.supply, None);
        assert!(
            reading.notes.iter().any(|note| note.contains("§36.1")),
            "{:?}",
            reading.notes
        );
    }

    /// The ambiguity this module refuses to resolve, and the one the declaration exists for.
    #[test]
    fn two_batteries_are_not_summed_and_the_refusal_names_them() {
        let machine = Machine::new("power-two");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        machine.supply("BAT1", "Battery", &[("energy_now", "10000000")]);
        let reading = machine
            .sensors()
            .read(&profile(), &Declared::default(), 1_000);

        assert_eq!(reading.observation.energy_remaining_wh, None);
        let note = reading.notes.first().expect("a note");
        assert!(note.contains("`BAT0`") && note.contains("`BAT1`"), "{note}");
        assert!(note.contains("state/power.json"), "{note}");
    }

    #[test]
    fn a_declared_pack_resolves_two_batteries() {
        let machine = Machine::new("power-declared");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        machine.supply("BAT1", "Battery", &[("energy_now", "10000000")]);
        machine.zone(0, "42000");
        let declared = Declared {
            supply: Some("BAT1".to_string()),
        };
        let reading = machine.sensors().read(&profile(), &declared, 1_000);

        assert_eq!(reading.observation.energy_remaining_wh, Some(10.0));
        assert_eq!(reading.supply.as_deref(), Some("BAT1"));
        assert!(
            reading.notes.is_empty(),
            "everything this host exposes was read: {:?}",
            reading.notes
        );
    }

    /// A declaration that names nothing on this machine reads *nothing*, rather than falling back
    /// to whatever is there. The operator named a pack; substituting another is the OS deciding
    /// which battery §36.2's reserve is about.
    #[test]
    fn a_declaration_naming_a_supply_that_is_not_there_reads_nothing() {
        let machine = Machine::new("power-wrong-name");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        let declared = Declared {
            supply: Some("PACK".to_string()),
        };
        let reading = machine.sensors().read(&profile(), &declared, 1_000);

        assert_eq!(reading.observation.energy_remaining_wh, None);
        assert!(reading.notes[0].contains("`PACK`"), "{:?}", reading.notes);
        assert!(reading.notes[0].contains("`BAT0`"), "{:?}", reading.notes);
    }

    #[test]
    fn a_battery_with_neither_reading_is_named_rather_than_read_as_empty() {
        let machine = Machine::new("power-quiet");
        machine.supply("BAT0", "Battery", &[("charge_now", "3000000")]);
        let reading = machine
            .sensors()
            .read(&profile(), &Declared::default(), 1_000);

        assert_eq!(reading.observation.state_of_charge, None);
        assert_eq!(reading.observation.energy_remaining_wh, None);
        assert_eq!(
            reading.supply.as_deref(),
            Some("BAT0"),
            "the pack was found; it is the reading that is missing, and those are different facts"
        );
        assert!(
            reading.notes[0].contains("charge_now"),
            "{:?}",
            reading.notes
        );
    }

    #[test]
    fn a_file_that_is_not_a_number_reads_as_absent_rather_than_zero() {
        let machine = Machine::new("power-garbage");
        machine.supply("BAT0", "Battery", &[("energy_now", "unavailable\n")]);
        let reading = machine
            .sensors()
            .read(&profile(), &Declared::default(), 1_000);
        assert_eq!(reading.observation.energy_remaining_wh, None);
    }

    // ── The thermal zones ────────────────────────────────────────────────────

    #[test]
    fn the_hottest_zone_wins_and_the_count_says_how_many_were_read() {
        let machine = Machine::new("power-zones");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        machine.zone(0, "42000").zone(1, "78500").zone(2, "31000");
        let reading = machine
            .sensors()
            .read(&profile(), &Declared::default(), 1_000);

        assert_eq!(reading.observation.temperature_c, Some(78.5));
        assert_eq!(reading.zones, 3);
    }

    #[test]
    fn one_unreadable_zone_does_not_cost_the_others() {
        let machine = Machine::new("power-zone-broken");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        machine.zone(0, "42000").zone(1, "n/a");
        let reading = machine
            .sensors()
            .read(&profile(), &Declared::default(), 1_000);

        assert_eq!(reading.observation.temperature_c, Some(42.0));
        assert_eq!(reading.zones, 1);
        assert!(reading.notes.is_empty(), "{:?}", reading.notes);
    }

    #[test]
    fn a_machine_with_no_zones_reads_no_temperature_and_says_so() {
        let machine = Machine::new("power-no-zones");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        let reading = machine
            .sensors()
            .read(&profile(), &Declared::default(), 1_000);

        assert_eq!(reading.observation.temperature_c, None);
        assert_eq!(reading.zones, 0);
        assert!(
            reading.notes.iter().any(|note| note.contains("§36.3")),
            "{:?}",
            reading.notes
        );
    }

    // ── The one thing this module remembers ──────────────────────────────────

    #[test]
    fn the_throttle_clock_runs_from_the_first_reading_above_the_threshold() {
        let machine = Machine::new("power-clock");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        machine.zone(0, "75000");
        let mut sensors = machine.sensors();

        let first = sensors.read(&profile(), &Declared::default(), 10_000);
        assert_eq!(first.observation.throttled_for_ms, 0, "it starts here");

        let later = sensors.read(&profile(), &Declared::default(), 40_000);
        assert_eq!(later.observation.throttled_for_ms, 30_000);
    }

    /// Continuously, not cumulatively. A body that crosses the line for a second every minute is
    /// not a body that has been throttling for an hour, and §36.3's DEGRADED is about the second
    /// reading.
    #[test]
    fn dropping_back_under_the_threshold_clears_the_clock() {
        let machine = Machine::new("power-clock-reset");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        machine.zone(0, "75000");
        let mut sensors = machine.sensors();
        sensors.read(&profile(), &Declared::default(), 10_000);

        machine.zone(0, "50000");
        let cooled = sensors.read(&profile(), &Declared::default(), 40_000);
        assert_eq!(cooled.observation.throttled_for_ms, 0);

        machine.zone(0, "75000");
        let hot_again = sensors.read(&profile(), &Declared::default(), 50_000);
        assert_eq!(
            hot_again.observation.throttled_for_ms, 0,
            "the second excursion is its own window"
        );
    }

    #[test]
    fn a_body_that_declares_no_threshold_never_starts_the_clock() {
        let machine = Machine::new("power-no-threshold");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        machine.zone(0, "95000");
        let bare = PowerProfile {
            thermal_throttle_c: None,
            ..profile()
        };
        let mut sensors = machine.sensors();
        sensors.read(&bare, &Declared::default(), 10_000);
        let later = sensors.read(&bare, &Declared::default(), 90_000);

        assert_eq!(
            later.observation.throttled_for_ms, 0,
            "nothing on this body says what hot is, so nothing here can say it has been hot"
        );
    }

    /// §35.1 keeps two clocks and they are corrected independently. A `now_ms` behind the instant
    /// the window opened is a corrected clock, and zero is the honest answer — the alternative is a
    /// wrapped `u64` reporting eighteen quintillion milliseconds of sustained throttling.
    #[test]
    fn a_clock_that_went_backwards_reports_no_throttling_rather_than_wrapping() {
        let machine = Machine::new("power-clock-back");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        machine.zone(0, "75000");
        let mut sensors = machine.sensors();
        sensors.read(&profile(), &Declared::default(), 100_000);
        let corrected = sensors.read(&profile(), &Declared::default(), 40_000);
        assert_eq!(corrected.observation.throttled_for_ms, 0);
    }

    // ── §33.1 ────────────────────────────────────────────────────────────────

    #[test]
    fn no_rhal_flag_is_ever_observed_from_here() {
        let machine = Machine::new("power-rhal");
        machine.supply("BAT0", "Battery", &[("energy_now", "45000000")]);
        let reading = machine
            .sensors()
            .read(&profile(), &Declared::default(), 1_000);
        assert!(!reading.observation.rhal_degraded);
        assert_eq!(reading.observation.fault_code, None);
    }

    // ── The declaration ──────────────────────────────────────────────────────

    #[test]
    fn a_missing_declaration_is_not_an_error() {
        let root = TempRoot::new("power-declared-missing");
        assert_eq!(
            Declared::read(&root.path().join("power.json")),
            Declared::default()
        );
    }

    #[test]
    fn a_declaration_naming_nothing_usable_is_the_same_as_none() {
        let root = TempRoot::new("power-declared-empty");
        let path = root.path().join("power.json");
        for text in [
            r#"{"supply":""}"#,
            r#"{"supply":"   "}"#,
            r#"{"supply":7}"#,
            "{",
        ] {
            std::fs::write(&path, text).unwrap();
            assert_eq!(Declared::read(&path).supply, None, "{text}");
        }
    }

    #[test]
    fn a_declared_name_is_trimmed() {
        let root = TempRoot::new("power-declared-trim");
        let path = root.path().join("power.json");
        std::fs::write(&path, r#"{"supply":"  BAT1  "}"#).unwrap();
        assert_eq!(Declared::read(&path).supply.as_deref(), Some("BAT1"));
    }
}

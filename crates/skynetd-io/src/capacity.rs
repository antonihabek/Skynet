//! What this robot can supply, assembled for `admit_package` (§8, §37.1).
//!
//! §37.1 puts admission between `verify` and `stage`, and it has never run there. The reason was
//! not a missing call site: [`skynet_contracts::admission::DeviceCapacity`] wants the robot's
//! memory, its capability tokens, its grants and what is already installed, and until the
//! descriptor arrived on the robot (§28) two of those had no source at all.
//!
//! # Nothing here is estimated
//!
//! Every field is read from something, or the capacity does not assemble.
//!
//! That is the whole design of this module, and it is why [`read`] returns a [`Capacity`] rather
//! than a `DeviceCapacity` with plausible defaults in it. A `ram_mb` of 8192 because that is a
//! common figure would admit a package this robot cannot feed, and §8.1 is explicit about why that
//! is the one place the format is stricter than an APK: *a phone that runs out of memory kills an
//! app; a robot that runs out of memory mid-stride falls over.* Guessing here is guessing at
//! exactly the number admission exists to check.
//!
//! So on a host that does not expose total memory the way §28 expects, admission does not run and
//! the install is refused with the reason. That is a real limitation on a developer's laptop and it
//! is the correct behaviour on a robot: §23.5 says a robot runs Linux, and a robot whose memory
//! cannot be read is a robot nobody should be installing a policy onto.
//!
//! # Where each field comes from
//!
//! | Field | Source |
//! |---|---|
//! | `ram_mb` | `/proc/meminfo`, `MemTotal` |
//! | `cpu_cores` | `available_parallelism` |
//! | `os_version` | the running daemon's version |
//! | `capabilities`, `extensions` | the embodiment descriptor (§5.0) |
//! | `granted` | `/data/state/grants.json` |
//! | `denied` | *nothing yet* — see below |
//! | `installed` | the package store |
//!
//! `vram_mb` and `disk_mb` stay `None`, which admission already treats as "not declared" rather
//! than as zero. Accelerator memory needs a vendor runtime this daemon does not link, and free disk
//! needs `statvfs`; both are honest absences rather than placeholders.
//!
//! `denied` is empty and that is the one field here whose emptiness is *not* yet a claim: §38.1's
//! operator profile is delivered through desired state, signed, and this daemon does not verify a
//! signature — so a `denied` list read from an unverified profile would be a safety-relevant value
//! taken from a file anybody on the robot could edit. Empty means no capability is withheld, which
//! is the permissive direction, and it is recorded as a finding rather than papered over.

use skynet_contracts::admission::{DeviceCapacity, InstalledPackage};

use crate::embodiment::Body;
use crate::Store;

/// What could be established about this robot.
#[derive(Debug, Clone)]
pub enum Capacity {
    /// Everything admission needs.
    Known(Box<DeviceCapacity>),
    /// It could not be assembled, and why.
    ///
    /// Not a `DeviceCapacity` with holes in it. A partial capacity is one admission would reason
    /// over as though it were complete, and every conclusion it drew would be about a robot that
    /// does not exist.
    Unavailable {
        /// The field that has no source, and what would supply it.
        detail: String,
    },
}

impl Capacity {
    /// The capacity, when there is one.
    pub fn known(&self) -> Option<&DeviceCapacity> {
        match self {
            Capacity::Known(capacity) => Some(capacity),
            Capacity::Unavailable { .. } => None,
        }
    }
}

/// Total memory, in megabytes, or the reason it is not known.
///
/// `/proc/meminfo` because it is the one source that is right on every Linux a robot runs, needs no
/// crate, and reports the figure the kernel itself budgets against. `MemTotal` is the installed
/// memory less what the firmware reserved, which is the number a package budget should be compared
/// with — `MemAvailable` would make admission depend on what happened to be running at install
/// time, so the same package would install in the morning and be refused after lunch.
#[cfg(target_os = "linux")]
pub fn total_memory_mb() -> Result<f64, String> {
    let text = std::fs::read_to_string("/proc/meminfo")
        .map_err(|error| format!("/proc/meminfo could not be read: {error}"))?;
    parse_mem_total(&text)
}

/// The same, on a host that is not a robot.
#[cfg(not(target_os = "linux"))]
pub fn total_memory_mb() -> Result<f64, String> {
    Err(format!(
        "this host is {} and total memory is read from /proc/meminfo. §23.5 says a robot runs \
         Linux; on anything else `skynetd` runs for development and cannot make an admission \
         decision, because the one number §8.1 exists to check is the one it cannot read.",
        std::env::consts::OS
    ))
}

/// Pull `MemTotal` out of `/proc/meminfo`.
///
/// Split out so the parsing is testable on any host. The file is `MemTotal:  16316776 kB`, and the
/// unit is always kB — the kernel has printed it that way since forever — but it is checked rather
/// than assumed, because a unit misread by a factor of 1024 is a memory budget misread by a factor
/// of 1024, in the permissive direction.
pub fn parse_mem_total(meminfo: &str) -> Result<f64, String> {
    for line in meminfo.lines() {
        let Some(rest) = line.strip_prefix("MemTotal:") else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let (Some(value), unit) = (parts.next(), parts.next()) else {
            return Err(format!("MemTotal has no value: `{line}`"));
        };
        let value: f64 = value
            .parse()
            .map_err(|_| format!("MemTotal is not a number: `{line}`"))?;
        return match unit {
            Some("kB") | Some("KB") => Ok(value / 1024.0),
            Some(other) => Err(format!(
                "MemTotal is in {other}, and this reader knows only kB. Reading it as kB would be \
                 wrong by whatever {other} is, in the direction that admits a package this robot \
                 cannot feed."
            )),
            None => Err(format!("MemTotal has no unit: `{line}`")),
        };
    }
    Err("/proc/meminfo has no MemTotal line".to_string())
}

/// Assemble everything `admit_package` needs, or say what is missing.
pub fn read(store: &Store, body: &Body, os_version: &str) -> Capacity {
    // The body first, and the order is deliberate. Both of the next two can stop the assembly, and
    // they are not equally useful to report: a descriptor that does not parse is a file on *this
    // robot* that somebody can fix, and an unreadable `/proc/meminfo` is a property of the host the
    // daemon was started on. Reporting the robot's own fault ahead of the environment's is what an
    // operator needs, and it also keeps the descriptor branch reachable on a machine that is not a
    // robot — where the memory read fails first and would otherwise mask everything behind it.
    //
    // §5.0. An absent list reads as the empty set — "this body provides nothing" — which refuses a
    // package needing a capability rather than admitting it. A *malformed* descriptor is different
    // and stops the assembly, because a body nobody has established the shape of is not a body to
    // make an install decision about.
    //
    // §55.1's flag comes from the same place and by the same rule: the descriptor declares whether
    // this body is simulated, absence means `false`, and a body that is absent entirely is not a
    // simulator either — which is the conservative direction, since it is what makes an environment
    // package report that nothing will load it rather than silently accepting one.
    let (capabilities, extensions, simulator) = match body {
        Body::Present(known) => (
            known.descriptor.capabilities.clone(),
            known.descriptor.extensions.clone(),
            known.descriptor.simulator,
        ),
        Body::Absent => (Vec::new(), Vec::new(), false),
        Body::Malformed { detail } => {
            return Capacity::Unavailable {
                detail: format!(
                    "the embodiment descriptor could not be used, so what this body provides is \
                     unknown: {detail}"
                ),
            }
        }
    };

    let ram_mb = match total_memory_mb() {
        Ok(ram_mb) => ram_mb,
        Err(detail) => return Capacity::Unavailable { detail },
    };

    let granted: Vec<String> = crate::observe::read_grants(store.layout())
        .into_iter()
        .map(|(_, token)| token)
        .collect();

    let installed: Vec<InstalledPackage> = store
        .installed()
        .into_iter()
        .map(|(id, digest)| InstalledPackage {
            id: id.as_str(),
            // The store is content-addressed and holds no version string: §37.1's on-disk shape is
            // `blobs/<digest>`, and the version lives in desired state. A dependency range is
            // therefore checkable by *presence* and not by version, which is a real limit and is
            // recorded as one rather than filled with the digest pretending to be a version.
            version: digest,
        })
        .collect();

    Capacity::Known(Box::new(DeviceCapacity {
        ram_mb,
        vram_mb: None,
        cpu_cores: std::thread::available_parallelism()
            .ok()
            .map(|n| n.get() as f64),
        disk_mb: None,
        os_version: os_version.to_string(),
        capabilities,
        extensions,
        granted,
        // §38.1's profile is signed and nothing here verifies a signature. See the module docs.
        denied: Vec::new(),
        installed,
        simulator,
    }))
}

/// What the API and the console are told.
pub fn to_json(capacity: &Capacity) -> skynet_json::Json {
    use skynet_json::Json;

    let mut document = Json::obj();
    match capacity {
        Capacity::Unavailable { detail } => {
            document
                .set("state", Json::from("unavailable"))
                .set("detail", Json::from(detail.as_str()))
                .set(
                    "consequence",
                    Json::from(
                        "Admission cannot run, so no package can be installed. §37.1 puts \
                         admit_package between verify and stage, and a robot whose capacity is \
                         unknown is a robot no budget can be checked against.",
                    ),
                );
        }
        Capacity::Known(capacity) => {
            let strings =
                |v: &[String]| Json::Arr(v.iter().map(|s| Json::from(s.as_str())).collect());
            document
                .set("state", Json::from("known"))
                .set("ram_mb", Json::Num(capacity.ram_mb))
                .set(
                    "cpu_cores",
                    capacity.cpu_cores.map_or(Json::Null, Json::Num),
                )
                // Null rather than zero, and the note says which absences these are. A console
                // rendering `vram_mb: 0` would be reporting a robot with no accelerator, which is
                // a different claim from one nothing has measured.
                .set("vram_mb", Json::Null)
                .set("disk_mb", Json::Null)
                .set(
                    "unmeasured",
                    Json::from(
                        "vram_mb needs a vendor runtime this daemon does not link, and disk_mb \
                         needs statvfs. Both are absent rather than zero.",
                    ),
                )
                .set("os_version", Json::from(capacity.os_version.as_str()))
                .set("capabilities", strings(&capacity.capabilities))
                .set("extensions", strings(&capacity.extensions))
                .set("granted", strings(&capacity.granted))
                .set("denied", strings(&capacity.denied))
                .set(
                    "denied_note",
                    Json::from(
                        "Empty because §38.1's operator profile is signed and this edition \
                         verifies no signature. A withheld-capability list read from an unverified \
                         file would be a safety-relevant value anybody on the robot could edit.",
                    ),
                )
                .set(
                    "installed",
                    Json::Arr(
                        capacity
                            .installed
                            .iter()
                            .map(|p| {
                                let mut item = Json::obj();
                                item.set("id", Json::from(p.id.as_str()))
                                    .set("digest", Json::from(p.version.as_str()));
                                item
                            })
                            .collect(),
                    ),
                );
        }
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mem_total_is_read_in_kilobytes_and_converted() {
        let meminfo = "MemTotal:       16316776 kB\nMemFree:         1000000 kB\n";
        let mb = parse_mem_total(meminfo).expect("MemTotal parses");
        assert!((mb - 15934.35).abs() < 0.1, "{mb}");
    }

    /// The unit is checked rather than assumed.
    ///
    /// A kernel that started printing MB would make every budget check pass by a factor of 1024,
    /// in the direction that admits a package the robot cannot feed. Refusing an unfamiliar unit
    /// costs an error message; assuming one costs a fall.
    #[test]
    fn an_unfamiliar_unit_is_refused_rather_than_assumed() {
        let error = parse_mem_total("MemTotal:       16000 MB\n").expect_err("MB is not kB");
        assert!(error.contains("MB"), "{error}");
        assert!(
            parse_mem_total("MemTotal:       16000\n").is_err(),
            "no unit"
        );
        assert!(parse_mem_total("MemFree: 1 kB\n").is_err(), "no MemTotal");
        assert!(parse_mem_total("MemTotal:  banana kB\n").is_err());
    }

    /// On a host that is not a robot, admission does not run and says why.
    ///
    /// The important half is that it does not *succeed with a guess*. §8.1's whole argument is that
    /// the budget check is the one place this format is stricter than an APK, and a default memory
    /// figure would be that check silently answering from a constant.
    #[test]
    #[cfg(not(target_os = "linux"))]
    fn a_host_that_is_not_a_robot_refuses_to_estimate() {
        let error = total_memory_mb().expect_err("only Linux exposes /proc/meminfo");
        assert!(error.contains("/proc/meminfo"), "{error}");
        assert!(error.contains("§23.5"), "{error}");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn a_robot_reports_its_own_memory() {
        let mb = total_memory_mb().expect("a Linux host has /proc/meminfo");
        assert!(mb > 64.0, "{mb} MB is not a machine that runs this daemon");
    }

    #[test]
    fn an_unavailable_capacity_says_what_it_costs() {
        let capacity = Capacity::Unavailable {
            detail: "no /proc/meminfo".to_string(),
        };
        assert!(capacity.known().is_none());
        let document = to_json(&capacity);
        assert_eq!(
            document.get("state").and_then(skynet_json::Json::as_str),
            Some("unavailable")
        );
        // Not just the cause. An operator reading "unavailable" needs to know what it stops.
        assert!(document
            .get("consequence")
            .and_then(skynet_json::Json::as_str)
            .is_some_and(|s| s.contains("Admission cannot run")));
    }
}

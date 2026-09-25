//! Starting and stopping packages, and the limits they run under (§33.5, §40).
//!
//! # The restart policy is not systemd's
//!
//! Every package runs as a transient systemd unit that `skynetd` owns, and the unit is written with
//! `Restart=no`. That looks like throwing away the feature, and it is the point: §33.5's policy is
//! per package *kind* — a driver is never restarted automatically, because a driver that crashed
//! left hardware in an unknown state — and it counts failures in a window, and both of those live
//! in [`skynetd::Supervision`], which is a pure state machine with a test suite.
//!
//! Two supervisors with two opinions about when to restart is one supervisor too many. So systemd
//! runs the process and reports what happened to it; the decision is upstairs.
//!
//! # The declared budget becomes the limit, exactly
//!
//! > A package that under-declares to get admitted is killed at the number it declared, on the
//! > robot, in front of the person who installed it — rather than being admitted on a fiction and
//! > discovered later by exhausting a machine that is holding sixty kilograms.
//!
//! [`Limits::from_manifest`] reads `budget.ram_mb` and `budget.cpu_cores` and nothing else invents
//! a value. `MemoryMax=`, never `MemoryHigh=`: §40's table gives the reason, which is that a policy
//! thrashing at a high watermark misses its deadline every cycle while looking alive, and a hard
//! kill is a cleaner failure because the kernel holds throughout.
//!
//! # The RT core is not shareable, and this is where that is enforced
//!
//! > A policy that could be scheduled onto the core running the safety kernel could delay the
//! > safety kernel, and the entire real-time argument in §23.5 would be void.
//!
//! [`Cpus::allowed`] subtracts the isolated set and **refuses to produce an empty allowance** rather
//! than quietly omitting `AllowedCPUs=`. An omitted cpuset is every core, including the one the
//! kernel is on, so the failure mode of a misconfigured isolation list would be the exact thing
//! §40.2 says is not a tunable.
//!
//! # A policy runs sandboxed, and this is where
//!
//! §8.3 has one sentence with no override: only a `driver` may run `native`, and a `policy` is
//! always sandboxed. Admission records that property (`isolation: "sandboxed"`) and the kernel's
//! `mode_gate` guard enforces the grants — but until this module, nothing confined the process
//! itself: every unit launched with the ambient privilege of whatever systemd context the daemon
//! runs in. [`confinement_properties`] closes that half, per kind, on every launch.
//!
//! # What is deliberately not here
//!
//! Every directive below is one that cannot break a legitimate package whatever directory the
//! robot keeps its store in. The path-dependent ones — `PrivateTmp`, `ProtectHome`,
//! `ProtectSystem=strict` — stay out because a store under `/tmp` (which `e2e.sh` uses) or under
//! a home directory (which a development machine uses) would vanish or go read-only inside the
//! unit, and a sandbox that breaks the layout it confines is a sandbox operators remove. Device
//! confinement stays out because there is no declared device set to allow-list from: §40.3's
//! accelerator rule (one policy holds the GPU, the second is refused at admission) is the design
//! that closes it, and a `DevicePolicy=closed` written before that admission exists would refuse
//! every GPU policy rather than the second one. Syscall and address-family filters stay out
//! because nobody has measured the allow-list, and §64 makes a wrong filter a defect rather than
//! caution: a refusal an operator cannot act on. The designed end state is §23.4's OCI sandbox;
//! this is the systemd confinement that holds until it exists, and the omissions are listed so
//! nobody mistakes it for the finished one.

use std::collections::BTreeSet;
use std::fmt;
use std::process::Command;

use skynet_json::Json;
use skynetd::actual::{UnitState, UnitStatus};
use skynetd::desired::PackageKind;

/// The resource ceilings one package runs under (§40.1).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Limits {
    /// `memory.max`, from the declared `ram_mb`.
    pub ram_mb: Option<f64>,
    /// `cpu.max`, from the declared `cpu_cores`.
    pub cpu_cores: Option<f64>,
    /// `pids.max`. Small and fixed: a fork bomb in a policy is a denial of service against the
    /// robot, and no policy needs hundreds of tasks.
    pub tasks_max: u32,
}

/// `pids.max`, per §40.1's "small, fixed".
pub const DEFAULT_TASKS_MAX: u32 = 128;

impl Limits {
    /// Read the ceilings a `.sky` manifest declares.
    ///
    /// A budget field that is absent stays absent. §8.1 makes the budget mandatory at *admission*,
    /// which is where a package with no declaration is refused; by the time anything is launched the
    /// declaration exists, and a launcher that substituted a default for a missing number would be
    /// the thing that let an undeclared package through.
    pub fn from_manifest(manifest: &Json) -> Self {
        let budget = manifest.get("budget").cloned().unwrap_or_else(Json::obj);
        Self {
            ram_mb: positive(budget.get_present("ram_mb").and_then(Json::as_f64)),
            cpu_cores: positive(budget.get_present("cpu_cores").and_then(Json::as_f64)),
            tasks_max: DEFAULT_TASKS_MAX,
        }
    }

    /// The `systemd-run` properties these ceilings become.
    pub fn properties(&self) -> Vec<String> {
        let mut properties = Vec::new();
        if let Some(ram) = self.ram_mb {
            // `MemoryMax`, not `MemoryHigh` — §40.1.
            properties.push(format!("MemoryMax={}M", ram.round() as u64));
        }
        if let Some(cores) = self.cpu_cores {
            // Quota, not shares: shares are relative and give no guarantee under load.
            properties.push(format!("CPUQuota={}%", (cores * 100.0).round() as u64));
        }
        properties.push(format!("TasksMax={}", self.tasks_max));
        properties
    }
}

fn positive(value: Option<f64>) -> Option<f64> {
    value.filter(|n| n.is_finite() && *n > 0.0)
}

/// The systemd confinement a package kind launches under (§8.3, §40).
///
/// A `driver` runs `native` — that is §8.3's own exemption, and its compensating control is §19's
/// signature gate (a driver activates only against a trusted key) plus the resource limits and
/// cpuset every kind carries. Everything else launches confined, with no flag to ask out: an
/// escape hatch here would quietly undo the invariant §7 is built on, so there isn't one.
///
/// Each directive earns its place against a sentence in the spec:
///
/// * `NoNewPrivileges=yes` — §8.3's isolation made a property of the process: no setuid/setgid
///   escalation out of the sandbox, ever.
/// * `ProtectSystem=yes` — the OS tree (`/usr`, the boot loader directories) read-only. `yes`,
///   not `strict`: strict would also fence the directories a store legitimately lives in.
/// * `ProtectKernelTunables=yes`, `ProtectKernelModules=yes` — a package rewrites no sysctl and
///   loads no module.
/// * `ProtectControlGroups=yes` — the §40 line: a package that could rewrite its own cgroup
///   could raise the `MemoryMax` §40.1 killed it at, and the declared budget would mean nothing.
/// * `RestrictSUIDSGID=yes` — a content-addressed blob has no honest setuid binary to create.
/// * `CapabilityBoundingSet=` (empty) — §23.4's "no `CAP_*`". A policy needing real-time
///   scheduling or raw I/O is a policy contradicting §23.5, and refusing it at launch is the
///   audible version of that contradiction.
///
/// Pure, so the confinement every launch carries can be asserted on any machine: a launcher
/// whose sandbox is only ever seen by systemd is a sandbox nobody has read.
pub fn confinement_properties(kind: PackageKind) -> Vec<String> {
    match kind {
        PackageKind::Driver => Vec::new(),
        PackageKind::Policy | PackageKind::Service | PackageKind::Skill => vec![
            "NoNewPrivileges=yes".to_string(),
            "ProtectSystem=yes".to_string(),
            "ProtectKernelTunables=yes".to_string(),
            "ProtectKernelModules=yes".to_string(),
            "ProtectControlGroups=yes".to_string(),
            "RestrictSUIDSGID=yes".to_string(),
            "CapabilityBoundingSet=".to_string(),
        ],
    }
}

/// Every directive name any kind's confinement asks for, sorted and deduplicated.
///
/// This exists so the query and the comparison cannot drift apart, and the drift it prevents has a
/// nasty shape: `confinement` asks systemd for a **named** list of properties, and a directive
/// added to [`confinement_properties`] but not to that list would come back unreported — which
/// [`confinement_shortfall`] counts, correctly, as not landed. The result would be a robot failing
/// its own `safety.policy.isolation` because of a forgotten string, and the failure would name a
/// directive that was in fact applied. Deriving one from the other removes the possibility rather
/// than testing for it.
pub fn confinement_property_names() -> Vec<String> {
    let mut names: Vec<String> = [
        PackageKind::Policy,
        PackageKind::Service,
        PackageKind::Skill,
        PackageKind::Driver,
    ]
    .into_iter()
    .flat_map(confinement_properties)
    .map(|line| line.split('=').next().unwrap_or_default().to_string())
    .filter(|name| !name.is_empty())
    .collect();
    names.sort();
    names.dedup();
    names
}

/// The property that says whether there is a unit to read at all.
///
/// Deliberately not part of [`confinement_property_names`], which is *the directives §8.3 asks
/// for* and is pinned to that length by a test. This is a different question asked in the same
/// call, and merging the two would put a non-directive into a list [`confinement_shortfall`]
/// compares against.
const LOAD_STATE: &str = "LoadState";

/// The directives systemd reports for a unit, or why there is no unit to report them for.
///
/// # The failure this exists to prevent
///
/// `systemctl show` **exits 0 for a unit it has never heard of**, and prints the manager's
/// *defaults* for every property named — `NoNewPrivileges=no`, `ProtectSystem=no`,
/// `CapabilityBoundingSet=` the full set, `MemoryMax=infinity`. Every one of those is a directive
/// §8.3 asks for, reported as not landed, by a manager that is not describing anything.
///
/// So a reader that trusts the exit status cannot tell *the sandbox did not land* from *there is
/// no unit*, and the two are opposite facts about a robot. A transient unit is collected the
/// moment it goes inactive (`--collect`, and §33.5 wants it that way), so the second is an
/// ordinary thing to observe: a policy that stopped between being listed and being read. Read as
/// the first, it makes §15's `safety.policy.isolation` **fail**, naming seven directives that were
/// in fact applied — the exact shape [`confinement_property_names`] documents one cause of and
/// this is another.
///
/// `Err` rather than an empty list, because `policy_isolation` already distinguishes them: a
/// confinement that could not be read is a **skip** with the reason, and a sandbox nobody could
/// read is not one this robot certifies either way.
///
/// Pure, so the distinction can be tested on a machine with no systemd — which is most of them,
/// and which is why the bug survived: every test of this path fed it a unit that existed.
pub fn confinement_from_show(text: &str) -> Result<Vec<(String, String)>, String> {
    let properties = parse_properties(text);
    let load_state = properties
        .iter()
        .find(|(key, _)| key == LOAD_STATE)
        .map(|(_, value)| value.as_str());
    match load_state {
        Some("loaded") => Ok(properties
            .into_iter()
            .filter(|(key, _)| key != LOAD_STATE)
            .collect()),
        // `not-found` is the ordinary one and deserves its own sentence: it is what a transient
        // unit leaves behind, and an operator reading "systemd has no unit by that name" should
        // not be sent looking for a broken sandbox.
        Some("not-found") => Err(
            "systemd has no unit by that name any more. A transient unit is collected when it goes \
             inactive, so the package stopped between being listed as running and being read — \
             which says nothing either way about the confinement it was launched under"
                .to_string(),
        ),
        Some(other) => Err(format!(
            "systemd reports the unit as `{other}` rather than loaded, so what it prints for the \
             confinement directives is its own defaults and not this unit's"
        )),
        // No `LoadState` line at all: a systemctl that answered something else entirely, or a
        // version that stopped reporting it. Fail closed rather than reading the rest as if it
        // were about a unit.
        None => Err(format!(
            "systemctl printed no {LOAD_STATE}, so whether it was describing a unit at all cannot \
             be established from its output"
        )),
    }
}

/// The two facts the **kernel** exposes about a confined process, from `/proc/<pid>/status`.
///
/// # Why a second layer, when `systemctl show` already answers
///
/// It answers a different question. `systemctl show` reports what the service manager **recorded**
/// for a unit — its own copy of the directives it was given. What a process is actually held to is
/// the kernel's, and the two can disagree: a directive the manager accepted, stored and reports
/// happily can fail to be applied by the kernel it is running on, and every layer above would go on
/// saying it landed.
///
/// §15's `safety.policy.isolation` has been walking down this ladder one rung at a time, and the
/// reasoning is the same each time. §59 asserted the **argv** — what was requested. §62 read the
/// unit back with `systemctl show` — what the manager holds. This is what the **kernel applied**,
/// which is the last rung and the only one that is a fact about the running process rather than
/// about something's record of it.
///
/// # Only two, and why not more
///
/// `NoNewPrivs` and `CapBnd` are the two directives in [`confinement_properties`] that the kernel
/// exposes per-process and unambiguously. The five `Protect*` directives are implemented as mount
/// namespaces, and what a namespace contains is not a line in `status` — verifying those from
/// inside would mean attempting a write and reporting what happened, which is a probe rather than
/// a reading and belongs to the demonstration §15 still lacks. Claiming to check five and checking
/// two would be worse than checking two.
pub const KERNEL_CONFINEMENT: [&str; 2] = ["NoNewPrivs", "CapBnd"];

/// Parse `/proc/<pid>/status`, keeping only [`KERNEL_CONFINEMENT`]'s lines.
///
/// Pure, for [`parse_properties`]'s reason: the comparison has to be testable on a machine with no
/// `/proc`, which is every developer machine this was written on.
///
/// The format is `Key:\t<value>` rather than `Key=Value`, and the values are kept verbatim —
/// `CapBnd` is a 16-digit hex mask and turning it into a number here would decide, in a parser,
/// what counts as empty. [`kernel_shortfall`] decides that, where the decision is visible.
pub fn parse_proc_status(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let at = line.find(':')?;
            let key = line[..at].trim();
            if !KERNEL_CONFINEMENT.contains(&key) {
                return None;
            }
            Some((key.to_string(), line[at + 1..].trim().to_string()))
        })
        .collect()
}

/// Which of the kernel's two facts contradict the confinement this kind asked for.
///
/// Empty means the kernel agrees with the request. A **missing** line is a shortfall, on
/// [`confinement_shortfall`]'s own rule: a property nobody can read is not one anybody should
/// certify, and `/proc` not carrying it is the same fail-closed direction as systemd not
/// reporting it.
///
/// `CapBnd` is compared against zero **as a mask**, parsed from hex, because the empty bounding
/// set is written `0000000000000000` and a string comparison against that literal would be a
/// check that a future kernel could break by printing a different width.
pub fn kernel_shortfall(kind: PackageKind, applied: &[(String, String)]) -> Vec<String> {
    if confinement_properties(kind).is_empty() {
        return Vec::new();
    }
    let read = |key: &str| {
        applied
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    };
    let mut shortfall = Vec::new();

    match read("NoNewPrivs") {
        Some("1") => {}
        Some(other) => shortfall.push(format!(
            "NoNewPrivs is {other} on the running process, while the unit says \
             NoNewPrivileges=yes"
        )),
        None => shortfall.push(
            "the running process does not report NoNewPrivs, so whether the kernel applied it \
             cannot be established"
                .to_string(),
        ),
    }

    match read("CapBnd").map(|mask| u64::from_str_radix(mask, 16)) {
        Some(Ok(0)) => {}
        Some(Ok(mask)) => shortfall.push(format!(
            "the running process holds capability bounding set {mask:016x}, while the unit says \
             CapabilityBoundingSet= (empty)"
        )),
        Some(Err(_)) => shortfall.push(
            "the running process reports a capability bounding set this robot cannot read as a \
             mask"
                .to_string(),
        ),
        None => shortfall.push(
            "the running process does not report CapBnd, so whether the kernel emptied the \
             bounding set cannot be established"
                .to_string(),
        ),
    }

    shortfall
}

/// What systemd reports about a unit, as `Key=Value` pairs in the order it printed them.
///
/// Pure, so the comparison below can be tested on a machine with no systemd — which is most of
/// them, and which is exactly why this is separated from the call that produces the text.
pub fn parse_properties(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            let at = line.find('=')?;
            Some((line[..at].to_string(), line[at + 1..].to_string()))
        })
        .collect()
}

/// Which of the confinement directives this kind asks for did **not** land on the unit.
///
/// The comparison is on the whole `Key=Value` string and never on the key alone, and that is the
/// load-bearing decision rather than a stylistic one. `CapabilityBoundingSet=` is asked for as the
/// **empty** set, so a check that found the key and stopped would pass a unit carrying
/// `CapabilityBoundingSet=cap_sys_admin` — the exact failure the directive exists to prevent, and
/// the one `e2e.sh` learned to assert line-exactly for the same reason.
///
/// A property systemd did not report at all counts as not landed. That is the fail-closed
/// direction: an unreported directive is one this code cannot show was applied, and a sandbox
/// nobody can read is not a sandbox anybody should certify.
pub fn confinement_shortfall(kind: PackageKind, observed: &[(String, String)]) -> Vec<String> {
    let seen: Vec<String> = observed
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    confinement_properties(kind)
        .into_iter()
        .filter(|wanted| !seen.iter().any(|line| line == wanted))
        .collect()
}

/// Which cores a package may be scheduled on (§40.2).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cpus {
    /// Every core on the machine.
    pub present: BTreeSet<u32>,
    /// The cores reserved for the kernel and the RHAL, isolated at boot.
    pub isolated: BTreeSet<u32>,
}

/// Why a cpuset could not be computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpuError {
    /// The machine reported no cores at all.
    NoCores,
    /// Every core is isolated, so there is nowhere a package may run.
    ///
    /// Refused rather than resolved by relaxing the isolation. A robot configured this way cannot
    /// run packages, and saying so is the honest outcome; the alternative — omitting the cpuset —
    /// would place a policy on the core running the safety kernel, which §40.2 says is not a
    /// tunable.
    EverythingIsolated,
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCores => write!(f, "the machine reported no CPUs"),
            Self::EverythingIsolated => write!(
                f,
                "every CPU is isolated for the real-time path, so there is no core a package may \
                 run on. §40.2 makes the RT core unshareable, so this is a configuration to fix \
                 rather than a limit to relax"
            ),
        }
    }
}

impl Cpus {
    /// The cores a package may use.
    pub fn allowed(&self) -> Result<Vec<u32>, CpuError> {
        if self.present.is_empty() {
            return Err(CpuError::NoCores);
        }
        let allowed: Vec<u32> = self.present.difference(&self.isolated).copied().collect();
        if allowed.is_empty() {
            return Err(CpuError::EverythingIsolated);
        }
        Ok(allowed)
    }

    /// `AllowedCPUs=` as systemd spells it.
    pub fn property(&self) -> Result<String, CpuError> {
        let allowed = self.allowed()?;
        let list: Vec<String> = allowed.iter().map(u32::to_string).collect();
        Ok(format!("AllowedCPUs={}", list.join(" ")))
    }

    /// Read the machine's cores and the isolation list from sysfs.
    ///
    /// `root` is `/` on a robot. It is a parameter only so a test can point at a fixture — it is
    /// **not** the layout's root, because the CPU topology is a property of the machine rather than
    /// of where this robot keeps `/data`.
    ///
    /// Both files are a cpu list — `0-3,7` — and both may be absent: a machine with no isolation
    /// reports no `isolated` file at all, which is a real and ordinary state and means the empty
    /// set. A machine with no `possible` file is not one anybody should be launching packages on,
    /// and produces [`CpuError::NoCores`] at the first attempt rather than a guess.
    pub fn observe(root: &std::path::Path) -> Self {
        let present = read_cpu_list(&root.join("sys/devices/system/cpu/possible"));
        let isolated = read_cpu_list(&root.join("sys/devices/system/cpu/isolated"));
        Self { present, isolated }
    }
}

fn read_cpu_list(path: &std::path::Path) -> BTreeSet<u32> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeSet::new();
    };
    parse_cpu_list(text.trim())
}

/// Parse a Linux cpu list: `0-3,7,9-10`.
pub fn parse_cpu_list(text: &str) -> BTreeSet<u32> {
    let mut cpus = BTreeSet::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((from, to)) => {
                if let (Ok(from), Ok(to)) = (from.trim().parse::<u32>(), to.trim().parse::<u32>()) {
                    // An inverted range is a malformed file, and expanding it either way would be
                    // inventing a fact about the machine. It contributes nothing.
                    if from <= to && to - from < 4096 {
                        for cpu in from..=to {
                            cpus.insert(cpu);
                        }
                    }
                }
            }
            None => {
                if let Ok(cpu) = part.parse::<u32>() {
                    cpus.insert(cpu);
                }
            }
        }
    }
    cpus
}

/// Everything needed to start one package.
#[derive(Debug, Clone, PartialEq)]
pub struct Unit {
    /// Package id, as the supervisor knows it.
    pub id: String,
    /// Which restart rules apply, and therefore how a crash is treated.
    pub kind: PackageKind,
    /// The program, resolved inside the package's blob.
    pub program: std::path::PathBuf,
    /// Its arguments.
    pub args: Vec<String>,
    /// Environment variables to set, beyond what systemd gives every unit.
    ///
    /// Empty for almost every package. §25's rendezvous is the reason it exists: a policy the
    /// daemon launches has to be told where the runtime is offering its rings, and it cannot be an
    /// argument because a package declares its own `runtime.args` and the daemon appending to them
    /// would be the daemon editing somebody's command line.
    pub env: Vec<(String, String)>,
    /// The directory it runs in — the blob, so a package cannot reach the store's other versions.
    pub working_directory: std::path::PathBuf,
    /// Resource ceilings.
    pub limits: Limits,
    /// Cores it may run on.
    pub cpus: Cpus,
}

/// The systemd unit name for a package.
///
/// `/` is not legal in a unit name, and systemd's own escaping (`-` for `/`) is ambiguous with a
/// `-` already in a name. So the separator is `:`, which systemd accepts and a package id cannot
/// contain — [`crate::layout::PackageId`] restricts every segment to `a-z0-9._-`.
pub fn unit_name(id: &str) -> String {
    format!("skynet:{}", id.replace('/', ":"))
}

/// Whatever starts and stops packages.
pub trait Launcher {
    /// Start a unit. Returns once the request has been accepted, not once the package is ready —
    /// readiness is §33.6's question and a different signal.
    fn start(&mut self, unit: &Unit) -> Result<(), LaunchError>;

    /// Stop a unit. Idempotent: stopping something that is not running is what a level-triggered
    /// reconciler does on the pass after it already stopped it.
    fn stop(&mut self, id: &str) -> Result<(), LaunchError>;

    /// What the system says about every unit this launcher owns.
    fn observe(&self) -> Result<Vec<UnitStatus>, LaunchError>;

    /// What the system says about one unit's **confinement** (§8.3, §59).
    ///
    /// Separate from [`observe`](Launcher::observe) because it asks a different question of a
    /// different vocabulary: `observe` maps systemd's lifecycle words onto §33.3's, and this reads
    /// the sandbox directives back off a unit that is already running. It exists because a
    /// launcher's own argv is not evidence — asserting what `command_line` built proves what was
    /// requested, and §15's `safety.policy.isolation` is a question about what landed.
    ///
    /// `Err` when there is nothing to read, with the sentence a conformance result will carry.
    fn confinement(&self, id: &str) -> Result<Vec<(String, String)>, LaunchError>;

    /// What the **kernel** applied to the unit's own process (§15, and see [`KERNEL_CONFINEMENT`]).
    ///
    /// The rung below [`confinement`](Launcher::confinement). That reads the manager's record of
    /// the directives; this reads `/proc/<MainPID>/status` on the process they were supposed to
    /// confine. A launcher with no running process says so.
    fn applied(&self, id: &str) -> Result<Vec<(String, String)>, LaunchError>;
}

/// Why a launch failed.
#[derive(Debug)]
pub struct LaunchError {
    /// Which package.
    pub id: String,
    /// What went wrong, in a sentence an operator can act on.
    pub detail: String,
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.id, self.detail)
    }
}

impl std::error::Error for LaunchError {}

/// The real one: transient units through `systemd-run`.
#[derive(Debug, Clone, Default)]
pub struct SystemdLauncher {
    /// Prefix the commands with `--user`, for a development machine with no system bus.
    pub user_scope: bool,
}

impl SystemdLauncher {
    /// A launcher against the system manager.
    pub fn system() -> Self {
        Self { user_scope: false }
    }

    /// A launcher against the calling user's manager.
    pub fn user() -> Self {
        Self { user_scope: true }
    }

    fn scope(&self) -> &'static [&'static str] {
        if self.user_scope {
            &["--user"]
        } else {
            &[]
        }
    }

    /// The `systemd-run` invocation for a unit, as data.
    ///
    /// Separated from running it so the arguments can be asserted in a test on any machine. A
    /// launcher whose command line is only ever seen by systemd is a launcher whose `MemoryMax`
    /// nobody has read.
    pub fn command_line(&self, unit: &Unit) -> Result<Vec<String>, LaunchError> {
        let mut argv: Vec<String> = self.scope().iter().map(|s| s.to_string()).collect();
        argv.push(format!("--unit={}", unit_name(&unit.id)));
        argv.push("--collect".to_string());
        // Not a scope: a service is what systemd supervises and reports an exit status for, and the
        // exit status is what §33.5's state machine consumes.
        argv.push("--service-type=exec".to_string());
        argv.push(format!(
            "--working-directory={}",
            unit.working_directory.display()
        ));

        // Restart is skynetd's decision, per kind, in a window (§33.5). Saying so explicitly rather
        // than relying on the default means a systemd whose default changed cannot quietly acquire
        // a second opinion.
        argv.push("-p".to_string());
        argv.push("Restart=no".to_string());

        for property in unit.limits.properties() {
            argv.push("-p".to_string());
            argv.push(property);
        }

        let cpus = unit.cpus.property().map_err(|error| LaunchError {
            id: unit.id.clone(),
            detail: error.to_string(),
        })?;
        argv.push("-p".to_string());
        argv.push(cpus);

        // §8.3: a policy is always sandboxed, and the launch is where that stops being a record
        // in the admission and starts being true of the process. A driver carries none of these
        // by the same section's exemption — and still carries its limits and its cpuset above.
        for property in confinement_properties(unit.kind) {
            argv.push("-p".to_string());
            argv.push(property);
        }

        // Before the program, which is where `systemd-run` takes its own options. A `--setenv` after
        // it is an argument to the package.
        for (name, value) in &unit.env {
            argv.push(format!("--setenv={name}={value}"));
        }

        argv.push(unit.program.display().to_string());
        argv.extend(unit.args.iter().cloned());
        Ok(argv)
    }
}

impl Launcher for SystemdLauncher {
    fn start(&mut self, unit: &Unit) -> Result<(), LaunchError> {
        let argv = self.command_line(unit)?;
        run("systemd-run", &argv, &unit.id)
    }

    fn stop(&mut self, id: &str) -> Result<(), LaunchError> {
        let mut argv: Vec<String> = self.scope().iter().map(|s| s.to_string()).collect();
        argv.push("stop".to_string());
        argv.push(format!("{}.service", unit_name(id)));
        match run("systemctl", &argv, id) {
            Ok(()) => Ok(()),
            // Stopping a unit systemd has never heard of is the state a stop was asking for.
            Err(error) if error.detail.contains("not loaded") => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn confinement(&self, id: &str) -> Result<Vec<(String, String)>, LaunchError> {
        let mut argv: Vec<String> = self.scope().iter().map(|s| s.to_string()).collect();
        argv.push("show".to_string());
        argv.push(format!("{}.service", unit_name(id)));
        // Named rather than dumping every property: `systemctl show` with no `-p` prints a few
        // hundred lines, and a check that scanned them would be one whose cost grows with
        // systemd's feature list. The names come from `confinement_properties` itself, so a
        // directive added there is asked for here without anybody remembering to.
        //
        // `LoadState` rides along because the exit status does not carry it: see
        // `confinement_from_show`. Asking in the same call rather than a second one is deliberate
        // — two calls could straddle the moment the unit is collected, and then the answer would
        // be about two different states of the machine.
        argv.push(format!(
            "--property={},{LOAD_STATE}",
            confinement_property_names().join(",")
        ));
        let output = Command::new("systemctl")
            .args(&argv)
            .output()
            .map_err(|error| LaunchError {
                id: id.to_string(),
                detail: format!(
                    "could not run systemctl to read the unit's confinement: {error}. This robot \
                     cannot say what landed on its own sandbox."
                ),
            })?;
        if !output.status.success() {
            return Err(LaunchError {
                id: id.to_string(),
                detail: format!(
                    "systemctl could not describe {}.service: {}",
                    unit_name(id),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            });
        }
        confinement_from_show(&String::from_utf8_lossy(&output.stdout)).map_err(|detail| {
            LaunchError {
                id: id.to_string(),
                detail: format!("{}.service: {detail}", unit_name(id)),
            }
        })
    }

    fn applied(&self, id: &str) -> Result<Vec<(String, String)>, LaunchError> {
        // `MainPID` rather than the whole property list: this is a second call and the first one is
        // already asking for the directives. Asking for the pid there would have coupled two
        // questions whose failure modes differ — a unit systemd cannot describe and a process that
        // has exited between the two calls are different sentences.
        let mut argv: Vec<String> = self.scope().iter().map(|s| s.to_string()).collect();
        argv.extend([
            "show".to_string(),
            format!("{}.service", unit_name(id)),
            "--property=MainPID".to_string(),
        ]);
        let output = Command::new("systemctl")
            .args(&argv)
            .output()
            .map_err(|error| LaunchError {
                id: id.to_string(),
                detail: format!(
                    "could not run systemctl to find the unit's process: {error}. This robot \
                     cannot say what the kernel applied to its own sandbox."
                ),
            })?;
        let pid = parse_properties(&String::from_utf8_lossy(&output.stdout))
            .into_iter()
            .find(|(key, _)| key == "MainPID")
            .map(|(_, value)| value)
            .unwrap_or_default();
        // Zero is systemd's "no process", not a pid. Reading `/proc/0` would produce a confusing
        // file-not-found where the true answer is that the unit is not running any more.
        if pid.is_empty() || pid == "0" {
            return Err(LaunchError {
                id: id.to_string(),
                detail: format!(
                    "{}.service has no running process, so there is nothing for the kernel to be \
                     holding. It stopped between being listed and being read.",
                    unit_name(id)
                ),
            });
        }
        let path = format!("/proc/{pid}/status");
        let text = std::fs::read_to_string(&path).map_err(|error| LaunchError {
            id: id.to_string(),
            detail: format!(
                "{path} could not be read: {error}. What the kernel applied to this unit cannot be \
                 established from this machine."
            ),
        })?;
        Ok(parse_proc_status(&text))
    }

    fn observe(&self) -> Result<Vec<UnitStatus>, LaunchError> {
        let mut argv: Vec<String> = self.scope().iter().map(|s| s.to_string()).collect();
        argv.extend([
            "show".to_string(),
            "--type=service".to_string(),
            "--state=loaded".to_string(),
            "--property=Id,ActiveState,SubState,NRestarts,ExecMainStatus".to_string(),
            "skynet:*.service".to_string(),
        ]);
        let output = Command::new("systemctl")
            .args(&argv)
            .output()
            .map_err(|error| LaunchError {
                id: "*".to_string(),
                detail: format!("could not run systemctl: {error}"),
            })?;
        Ok(parse_systemctl_show(&String::from_utf8_lossy(
            &output.stdout,
        )))
    }
}

fn run(program: &str, argv: &[String], id: &str) -> Result<(), LaunchError> {
    let output = Command::new(program)
        .args(argv)
        .output()
        .map_err(|error| LaunchError {
            id: id.to_string(),
            detail: format!("could not run {program}: {error}"),
        })?;
    if output.status.success() {
        return Ok(());
    }
    Err(LaunchError {
        id: id.to_string(),
        detail: format!(
            "{program} exited {}: {}",
            output
                .status
                .code()
                .map_or_else(|| "by signal".to_string(), |code| code.to_string()),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    })
}

/// Parse `systemctl show`'s property blocks into unit statuses.
///
/// Blocks are separated by a blank line and each is `Key=value`. Pure, so the mapping from
/// systemd's vocabulary to §33.3's can be tested without systemd — which matters, because the
/// mapping is where a supervisor decides a crashing unit is merely stopped.
pub fn parse_systemctl_show(text: &str) -> Vec<UnitStatus> {
    let mut units = Vec::new();
    for block in text.split("\n\n") {
        let mut fields = std::collections::BTreeMap::new();
        for line in block.lines() {
            if let Some((key, value)) = line.split_once('=') {
                fields.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        let Some(unit) = fields.get("Id") else {
            continue;
        };
        let Some(id) = package_id_of_unit(unit) else {
            continue;
        };
        let active = fields.get("ActiveState").map(String::as_str).unwrap_or("");
        let sub = fields.get("SubState").map(String::as_str).unwrap_or("");
        units.push(UnitStatus {
            id,
            state: unit_state(active, sub),
            restarts: fields
                .get("NRestarts")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0),
            last_exit: fields.get("ExecMainStatus").and_then(|n| n.parse().ok()),
        });
    }
    units.sort_by(|a, b| a.id.cmp(&b.id));
    units
}

/// Turn `skynet:ns:name.service` back into `ns/name`.
pub fn package_id_of_unit(unit: &str) -> Option<String> {
    let stem = unit.strip_suffix(".service").unwrap_or(unit);
    let rest = stem.strip_prefix("skynet:")?;
    let (namespace, name) = rest.split_once(':')?;
    if namespace.is_empty() || name.is_empty() || name.contains(':') {
        return None;
    }
    Some(format!("{namespace}/{name}"))
}

/// systemd's `ActiveState`/`SubState` in §33.3's vocabulary.
///
/// The distinction that matters is `failed` from `stopped`: a package an operator stopped and one
/// that exited five times in ten minutes look identical on a process table and mean opposite things
/// to a reconciler. systemd's `failed` is the *unit* failing, which with `Restart=no` means the
/// process exited non-zero — so it is mapped to [`UnitState::Failed`] and §33.5's window decides
/// whether that becomes an abandoned package.
pub fn unit_state(active: &str, sub: &str) -> UnitState {
    match active {
        "active" => UnitState::Running,
        "activating" | "deactivating" | "reloading" => UnitState::Transitioning,
        "failed" => UnitState::Failed,
        "inactive" => UnitState::Stopped,
        // An unrecognised state is transitioning rather than stopped, and the asymmetry is
        // deliberate: a reconciler does nothing to a transitioning unit, so an unknown word costs a
        // pass, while calling it stopped would start a second copy of something that may be running.
        _ => {
            if sub == "dead" {
                UnitState::Stopped
            } else {
                UnitState::Transitioning
            }
        }
    }
}

/// A launcher that records instead of launching.
///
/// Not only for tests. `skynetd --dry-run` uses it to print what a reconcile pass would do to a
/// machine, which is the difference between an operator reading a plan and an operator running one.
#[derive(Debug, Clone, Default)]
pub struct RecordingLauncher {
    /// Every call, in order.
    pub calls: Vec<String>,
    /// What [`Launcher::observe`] will report.
    pub units: Vec<UnitStatus>,
    /// Package ids whose start should fail, and why.
    pub refuse: std::collections::BTreeMap<String, String>,
    /// Every unit it was asked to start, whole: the program, arguments and environment a real
    /// launcher would have run, which is what a plan is about.
    pub started: Vec<Unit>,
}

impl RecordingLauncher {
    /// An empty one.
    pub fn new() -> Self {
        Self::default()
    }

    /// Arrange the state the machine will report.
    pub fn with_units(units: Vec<UnitStatus>) -> Self {
        Self {
            units,
            ..Self::default()
        }
    }
}

impl Launcher for RecordingLauncher {
    fn start(&mut self, unit: &Unit) -> Result<(), LaunchError> {
        self.calls.push(format!("start {}", unit.id));
        self.started.push(unit.clone());
        if let Some(detail) = self.refuse.get(&unit.id) {
            return Err(LaunchError {
                id: unit.id.clone(),
                detail: detail.clone(),
            });
        }
        match self.units.iter_mut().find(|u| u.id == unit.id) {
            Some(existing) => existing.state = UnitState::Running,
            None => self.units.push(UnitStatus {
                id: unit.id.clone(),
                state: UnitState::Running,
                restarts: 0,
                last_exit: None,
            }),
        }
        Ok(())
    }

    fn stop(&mut self, id: &str) -> Result<(), LaunchError> {
        self.calls.push(format!("stop {id}"));
        if let Some(existing) = self.units.iter_mut().find(|u| u.id == id) {
            existing.state = UnitState::Stopped;
        }
        Ok(())
    }

    fn confinement(&self, id: &str) -> Result<Vec<(String, String)>, LaunchError> {
        Err(LaunchError {
            id: id.to_string(),
            detail: "this launcher records rather than launching, so no unit exists to read \
                     confinement from. A recorded start is a plan, and a plan is not evidence \
                     about a sandbox."
                .to_string(),
        })
    }

    fn applied(&self, id: &str) -> Result<Vec<(String, String)>, LaunchError> {
        Err(LaunchError {
            id: id.to_string(),
            detail: "this launcher records rather than launching, so there is no process for a \
                     kernel to be holding."
                .to_string(),
        })
    }

    fn observe(&self) -> Result<Vec<UnitStatus>, LaunchError> {
        let mut units = self.units.clone();
        units.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(units)
    }
}

//! The shape a supervisor opens its recording ring at (§26.3, §80).
//!
//! # The question, and why it was filed as blocked
//!
//! §26.3 ends on the one thing the two-writer ring did not close:
//!
//! > On a robot, `skynetd` starts first (§27) and opens a ring shaped for zero joints and zero
//! > channels, because that is the honest description of a supervisor that commands nothing. A
//! > control loop starting afterwards is then refused — its cycles do not fit. So the ordering that
//! > works today is the loop first, and the ordering a robot actually boots in is the daemon first.
//!
//! A record's size is a function of the joint and channel counts, so a ring holds one shape. The
//! daemon creates the recording because it starts first; the loop attaches and — unlike the daemon,
//! which writes only events and gaps — **cannot take any shape it finds**, because a cycle carries
//! a value per joint and per channel. `skynet-kernel-host`'s refusal already says what would fix
//! it, in the last clause: *"Point this loop at another directory, or **give the other writer this
//! body's shape**."* This module is that clause.
//!
//! §80 records the blocker as a channel count that nothing carries to the daemon:
//!
//! > This was blocked on a **channel count** the specification said was *"the policy runtime's, and
//! > that runtime is unwritten"* — and `skynet-policy-host` is that runtime, and
//! > `Commanding::channels` is that number, declared when a lease is granted, which is before
//! > either ring is opened. So the reason this is still open has changed, and shrunk: the number
//! > exists and **nothing carries it to the daemon**. That is a lease record `skynetd` can read,
//! > not a design question.
//!
//! # The finding: nothing has to carry it, because the daemon already computes it
//!
//! A lease record would have worked and would have been wrong in a way worth stating, because the
//! shape of the mistake is more useful than the fix. A lease is granted when a policy **starts**,
//! and the daemon needs the number when *it* starts, which is earlier. A record written at the
//! first lease and read at every later boot is a robot whose first boot after installing a policy
//! still refuses the loop, and whose ring is sized by whatever ran last — configuration inferred
//! from history, which is the thing [`crate::layout`]'s header argues against in as many words:
//! *"a record kept beside the thing it describes is a record that can disagree with it."*
//!
//! The number does not need carrying. §37.1's `transport` stage already resolves a plan for every
//! package this robot installs, and [`skynet_contracts::transport::TransportPlan::bindings`] is
//! *"every channel, in the policy's output order"* — which is the same count
//! `Commanding::channels` holds, derived from the same declaration, one process earlier. The daemon
//! computes it at install and throws it away. So the inputs are:
//!
//! | | Where |
//! |---|---|
//! | joints | the embodiment descriptor, which [`crate::embodiment::load`] already reads |
//! | channels | the transport plan, which [`crate::admit::transport_plan`] already resolves |
//!
//! Both are on disk before the first pass reconciles anything, both are read from the robot's own
//! provisioning rather than from a peer's announcement, and neither needs the policy runtime to be
//! running — or to exist. §80's entry can be closed without the lease record it asks for.
//!
//! # Refusing to choose is a case, not a failure
//!
//! §25 has one command ring, one loop and one lease. Two installed packages that both declare an
//! action space are a robot where **the daemon cannot know which policy is going to command**, and
//! sizing for either would be a guess that costs the other one its recording. So that is
//! [`Supervising::Ambiguous`], the ring is opened events-only as it is today, and the packages are
//! named — an operator reading the line can see exactly what to do about it.
//!
//! The same reasoning covers a plan whose verdict is `refused`: a policy that cannot be transported
//! onto this body is a loop that will never run, and a ring sized for its channels would spend the
//! window on arrays nothing fills. §13 asks for roughly an hour and a record's size divides
//! straight into it, so an unfillable channel is not a harmless over-allocation.
//!
//! # What this does not close
//!
//! **A loop nothing declared — closed 2026-09-07, by an operator declaring it.** `skynetk` run by
//! hand against a driver names no package, so this resolver sized the recording events-only and the
//! loop was refused at `Recorder::open`. The refusal was right and stopping there was not. Two
//! sentences were being run together: *the daemon must not invent a loop*, which is true and is why
//! nothing is guessed from the driver; and *therefore a bench loop cannot be recorded*, which does
//! not follow. An operator saying **I am about to run a loop this wide** is not the daemon
//! inventing anything.
//!
//! `state/bench.json` is that declaration, and it is §34.3's rule applied a fourth time — the same
//! shape as `power.json` naming which of two batteries is the traction pack, and as
//! `brownout_fault_codes` naming which codes mean sag. The parts a machine can measure are
//! measured, the parts it cannot are declared, and every surface says which is which.
//!
//! It is deliberately **weaker than a plan** in three ways, because a declaration is a person's
//! word and a plan is two signed documents:
//!
//! * a commanding package always wins, so a bench file left behind on a robot that later installs
//!   a policy changes nothing;
//! * it cannot resolve `Refused`, `Ambiguous` or `Unreadable` into a shape — those are robots with
//!   a problem an operator needs to see, and letting a bench file paper over `Ambiguous` would hide
//!   exactly the case §80 leaves both packages unringed for;
//! * every surface says the width was **declared** rather than resolved, so nothing reads a bench
//!   window as evidence about a policy.
//!
//! **A plan that drops an optional channel.** `bindings` keeps every channel the policy declared,
//! dropped ones included, so the count is the policy's output width and matches what the loop
//! writes. A loop that instead sized itself to the *bound* channels would disagree, and the
//! comparison on the other side is `!=` rather than `<=`. Nothing here relies on that today; it is
//! written down because a future change to either side would break it silently.

use skynet_blackbox::record::Shape;
use skynet_contracts::json::manifest_from_json;
use skynet_contracts::transport::is_installable;

use crate::admit::transport_plan;
use crate::embodiment::Body;
use crate::store::Store;

/// What the daemon concluded about the body it is recording.
#[derive(Debug, Clone, PartialEq)]
pub enum Supervising {
    /// No descriptor, or one that does not parse. Nothing can be said about the body.
    NoBody,
    /// A body, and nothing installed that declares an action space.
    NoPolicy {
        /// How many joints the descriptor declares.
        joints: usize,
    },
    /// One commanding package, and a plan that resolves onto this body.
    Policy {
        /// The package whose action space sized the ring.
        package: String,
        /// Joints, from the descriptor.
        joints: usize,
        /// Channels, from the plan's bindings.
        channels: usize,
        /// The plan's verdict, for the operator line.
        verdict: &'static str,
    },
    /// No commanding package, and an operator has declared a bench loop's width (§80).
    ///
    /// Ranked below [`Supervising::Policy`] and reachable only from [`Supervising::NoPolicy`]: a
    /// declaration is a person's word about what they are about to do, and a transport plan is two
    /// signed documents about what a package will do. Where both exist the plan decides.
    Bench {
        /// Joints, from the descriptor — measured, not declared. A bench loop reads the same body.
        joints: usize,
        /// Channels, as the operator declared them.
        channels: usize,
    },
    /// One commanding package whose plan this body refuses.
    Refused {
        /// The package.
        package: String,
        /// Joints, from the descriptor.
        joints: usize,
    },
    /// More than one commanding package. The daemon does not choose between them.
    Ambiguous {
        /// Every package that declares an action space, in installed order.
        packages: Vec<String>,
        /// Joints, from the descriptor.
        joints: usize,
    },
    /// An installed package whose manifest this daemon cannot read.
    ///
    /// Separate from [`Supervising::NoPolicy`] on the authority of the contract that produces it.
    /// `manifest_from_json` documents the same distinction where it decodes the block: *"a block
    /// that does not decode is an **error** rather than an absence: an action space nobody could
    /// parse and an action space nobody declared are opposite facts."*
    ///
    /// It is fatal to the whole answer rather than to one package, and that is the conservative
    /// reading: the unreadable manifest may be the one that declares the action space, so a count
    /// taken without it is a count that cannot be trusted. §37.1 already refuses to admit such a
    /// package — `tests.rs` asserts *"`manifest_from_json` refuses, so admission does not run at
    /// all"* — so this is a robot with a problem the operator needs to see, not a shape to guess
    /// around.
    Unreadable {
        /// The package whose manifest could not be read.
        package: String,
        /// Joints, from the descriptor.
        joints: usize,
        /// What went wrong, for the operator line.
        detail: String,
    },
}

impl Supervising {
    /// The shape to open the ring at.
    ///
    /// Everything but [`Supervising::Policy`] is events-only, and that is deliberate rather than a
    /// fallback: a shape with joints and no channels still sizes every record for a per-joint array
    /// that no writer fills, which spends the window §13 asks for on zeroes.
    pub fn shape(&self) -> Shape {
        match self {
            Supervising::Policy {
                joints, channels, ..
            }
            | Supervising::Bench { joints, channels } => Shape {
                joints: *joints as u16,
                channels: *channels as u16,
            },
            _ => Shape {
                joints: 0,
                channels: 0,
            },
        }
    }

    /// One line for the operator, saying what sized the recording and where the number came from.
    ///
    /// Printed at every start. A ring sized for a body is a different artifact from one sized for a
    /// supervisor — it holds a much shorter window — and an operator who cannot see which they have
    /// discovers it from an incident report that stops earlier than they expected.
    pub fn describe(&self) -> String {
        match self {
            Supervising::NoBody => "events only: this robot has no embodiment descriptor, so \
                 nothing here can describe the body a cycle would be about"
                .to_string(),
            Supervising::NoPolicy { joints } => format!(
                "events only: {joints} joints declared and no installed package declares an action \
                 space, so nothing commands this body"
            ),
            Supervising::Policy {
                package,
                joints,
                channels,
                verdict,
            } => format!(
                "sized for the body: {joints} joints and {channels} channels, from {package}'s \
                 transport plan ({verdict})"
            ),
            Supervising::Bench { joints, channels } => format!(
                "sized for a declared bench loop: {joints} joints and {channels} channels, from \
                 state/bench.json. No package declares an action space — this width is an \
                 operator's word rather than a transport plan, and a window recorded under it is \
                 evidence about a bench session and not about a policy"
            ),
            Supervising::Refused { package, joints } => format!(
                "events only: {package} declares an action space and this body refuses its \
                 transport plan, so no loop will run it — {joints} joints left unsized"
            ),
            Supervising::Ambiguous { packages, joints } => format!(
                "events only: {} packages declare an action space ({}) and §25 has one control \
                 loop, so which of them will command these {joints} joints is not this daemon's to \
                 guess — leave one installed to size the ring for it",
                packages.len(),
                packages.join(", "),
            ),
            Supervising::Unreadable {
                package,
                joints,
                detail,
            } => format!(
                "events only: {package}'s manifest cannot be read ({detail}), so whether anything \
                 commands these {joints} joints is not a question this robot can answer — an \
                 action space nobody could parse is not an action space nobody declared"
            ),
        }
    }
}

/// What this daemon can see of §27's **`policy`** condition — §25.2's lease, from outside it.
///
/// The last of §27's eight with a source in this process, and the only one where the honest answer
/// is *partly* unobservable — which is why it is a type rather than three lines.
///
/// # Two of the three values are exactly knowable and the third is not
///
/// `PolicyState` is `None` (*nothing to run*), `Held` (*something to run that is not ready to*) and
/// `Ready` (*lease granted and plan resolved*). This daemon resolves the plan itself — that is what
/// [`Supervising`] is — and it launches the unit, so it can establish:
///
/// * **`None`** exactly. Nothing installed declares an action space, or no body is described.
/// * **`Held`** exactly, in four distinct ways: the plan is refused, the manifest cannot be read,
///   two packages both command (§80 leaves both without a ring for the same reason), or the plan
///   resolves and the unit is not running — a policy whose process does not exist holds no lease.
///
/// What it cannot establish is **`Ready`**, and no amount of care here changes that: §25.2's lease
/// table is the kernel's, and a resolved plan beside a running unit is *necessary and not
/// sufficient*. So that one case is reported as unobserved, with the two facts that are known
/// named, rather than promoted to a `Ready` this process invented.
///
/// # And it fixes a sentence that was wrong on every robot with a package installed
///
/// Until this existed the condition was left at `Conditions::default()`, which is
/// `PolicyState::None` — and §27's rule 8 splits that from `Held` deliberately, because *"there is
/// nothing to run"* and *"there is something and it has not got a lease"* are different sentences
/// and the second is the one with a remedy. A robot with a commanding package installed and
/// activated was reporting `no_policy`, which is not a cautious reading of an unobserved condition:
/// it is a false statement about the store, made by the process that holds the store.
///
/// So the unobservable case feeds `Held` rather than the default. That is still the least capable
/// value the condition could take — `Held` is weaker than `Ready` — and it is the only one of the
/// three that is true.
#[derive(Debug, Clone, PartialEq)]
pub enum PolicySighting {
    /// Nothing installed declares an action space, or nothing describes the body.
    Nothing {
        /// Which of the two, for the sentence.
        no_body: bool,
    },
    /// Something to run, and a reason it is not running it.
    NotReady {
        /// The package, when one can be named. `Ambiguous` names none: the point is that the
        /// daemon does not choose between them.
        package: Option<String>,
        /// Why, in §64's terms.
        why: &'static str,
    },
    /// A plan resolved and a unit is running, and the lease is on the far side of §33.1.
    ///
    /// Not a reading. Carries the package so the sentence can name what it is *not* able to
    /// confirm about, which is more use than saying nothing.
    LeaseUnknown {
        /// The commanding package whose plan resolved.
        package: String,
    },
}

impl PolicySighting {
    /// Read §27's condition off the shape and the unit the daemon launched.
    ///
    /// `running` answers *is this package's unit running* — the daemon's own action's result rather
    /// than a question to the kernel, which is why taking it is not a §33.1 crossing. Passed in as
    /// a closure so the mapping stays a function over facts and can be driven without a launcher.
    pub fn of(supervising: &Supervising, running: impl Fn(&str) -> bool) -> PolicySighting {
        match supervising {
            Supervising::NoBody => PolicySighting::Nothing { no_body: true },
            // A declared bench loop is **not** a policy, and this is the arm where saying so
            // matters. §25.2's lease is granted to a policy the Safety Kernel knows about; a loop
            // an operator started by hand holds none and never will, and nothing installed
            // declares an action space. So §27's condition is `no_policy` — *there is nothing to
            // run* — exactly as it is without the file. What the declaration changes is the shape
            // of the recording, and deliberately nothing else.
            Supervising::NoPolicy { .. } | Supervising::Bench { .. } => {
                PolicySighting::Nothing { no_body: false }
            }
            Supervising::Refused { package, .. } => PolicySighting::NotReady {
                package: Some(package.clone()),
                why: "this body refuses its transport plan, so there is nothing for a lease to \
                      command (§6, §37.1)",
            },
            Supervising::Unreadable { package, .. } => PolicySighting::NotReady {
                package: Some(package.clone()),
                why: "its manifest cannot be read, so no plan could be resolved from it (§8)",
            },
            Supervising::Ambiguous { .. } => PolicySighting::NotReady {
                package: None,
                why:
                    "two packages declare an action space and this daemon does not choose between \
                      them — §25.2's arbitration is where that is settled, and until it is neither \
                      gets a ring (§80)",
            },
            Supervising::Policy { package, .. } if !running(package) => PolicySighting::NotReady {
                package: Some(package.clone()),
                why: "its plan resolves and its unit is not running, so no process exists to hold \
                      a lease (§33.5)",
            },
            Supervising::Policy { package, .. } => PolicySighting::LeaseUnknown {
                package: package.clone(),
            },
        }
    }

    /// §27's condition.
    ///
    /// Never `Ready`, on any input, and that is the design rather than a gap in the mapping: this
    /// process cannot see §25.2's lease table, and the one value that asserts a lease is the one it
    /// must not produce.
    pub fn state(&self) -> skynet_contracts::opstate::PolicyState {
        match self {
            PolicySighting::Nothing { .. } => skynet_contracts::opstate::PolicyState::None,
            // `Held` for both, including the unobservable case — see the type's header. `None`
            // there would be this process saying nothing is installed while holding the store that
            // says otherwise.
            _ => skynet_contracts::opstate::PolicyState::Held,
        }
    }

    /// Whether this was an observation at all.
    pub fn observed(&self) -> bool {
        !matches!(self, PolicySighting::LeaseUnknown { .. })
    }

    /// What was seen, in a sentence.
    pub fn detail(&self) -> String {
        match self {
            PolicySighting::Nothing { no_body: true } => {
                "no body is described, so nothing could declare an action space to command it (§5)"
                    .to_string()
            }
            PolicySighting::Nothing { no_body: false } => {
                "nothing installed declares an action space, so there is no policy to run (§8)"
                    .to_string()
            }
            PolicySighting::NotReady {
                package: Some(package),
                why,
            } => format!("`{package}` is installed and not running: {why}"),
            PolicySighting::NotReady { package: None, why } => why.to_string(),
            PolicySighting::LeaseUnknown { package } => format!(
                "`{package}`'s plan resolves and its unit is running, which is necessary and not \
                 sufficient: §25.2's lease is granted by the Safety Kernel and this daemon cannot \
                 read the table. Reported as unobserved and taken at `held`, which is the weaker \
                 of the two values it could have"
            ),
        }
    }
}

/// Work out the shape this supervisor should open its ring at.
///
/// `allow_retarget` is the caller's for the same reason [`crate::admit::transport_plan`] takes it:
/// §6.1 makes retargeting an operator's decision rather than a default, and a plan resolved with a
/// different answer to it can bind a different number of channels.
pub fn supervising(store: &Store, body: &Body, allow_retarget: bool) -> Supervising {
    let Some(known) = body.known() else {
        return Supervising::NoBody;
    };
    let joints = known.descriptor.joints.len();

    // Every installed package that declares an action space. `installed()` is the active set — a
    // staged package is not one this robot is running, and sizing a recording for something that
    // has not been activated would be sizing it for a rollout that may never happen.
    //
    // The manifest is kept rather than re-read, so the plan below is resolved against the same
    // bytes this loop counted. Re-reading would open a window in which the two disagree, which on
    // this path means a ring sized from one manifest for a loop described by another.
    let mut commanding: Vec<(String, skynet_contracts::admission::PackageManifest)> = Vec::new();
    for (id, digest) in store.installed() {
        let document = match store.manifest(&digest) {
            Ok(document) => document,
            Err(error) => {
                return Supervising::Unreadable {
                    package: id.as_str(),
                    joints,
                    detail: error.to_string(),
                }
            }
        };
        let manifest = match manifest_from_json(&document) {
            Ok(manifest) => manifest,
            Err(error) => {
                return Supervising::Unreadable {
                    package: id.as_str(),
                    joints,
                    detail: format!("{error:?}"),
                }
            }
        };
        if manifest.source_embodiment.is_some() {
            commanding.push((id.as_str(), manifest));
        }
    }

    match commanding.len() {
        // The one arm a declaration may fill. §80's rule is that the daemon must not invent a
        // loop; an operator naming one is not that, and every other arm here is a robot with a
        // problem an operator needs to see rather than one to size around.
        0 => match bench_channels(store.layout()) {
            Some(channels) => Supervising::Bench { joints, channels },
            None => Supervising::NoPolicy { joints },
        },
        1 => {
            let (package, manifest) = &commanding[0];
            match transport_plan(manifest, body, allow_retarget) {
                // `transport_plan` returns `None` when the manifest declares no source embodiment,
                // which cannot happen here — it is the condition that put the package in this list.
                // Treated as no policy rather than unwrapped, because a panic in the black box's
                // sizing path would be a robot that cannot record because of a recording.
                None => Supervising::NoPolicy { joints },
                Some(plan) if !is_installable(&plan) => Supervising::Refused {
                    package: package.clone(),
                    joints,
                },
                Some(plan) => Supervising::Policy {
                    package: package.clone(),
                    joints,
                    channels: plan.bindings.len(),
                    verdict: plan.verdict.as_str(),
                },
            }
        }
        _ => Supervising::Ambiguous {
            packages: commanding.iter().map(|(id, _)| id.clone()).collect(),
            joints,
        },
    }
}

/// The bench width an operator declared, if any (§80).
///
/// # What it refuses to read, and why each refusal is not fussiness
///
/// The file's only job is to size a recording, and every malformed value has a plausible-looking
/// wrong answer available:
///
/// * **A missing or unreadable file is nothing**, never zero. Zero channels is `events only`, which
///   is what the robot already does; there is no reason to spell it in a file and a reader that
///   accepted it would let a typo silently disarm a bench session.
/// * **Zero is refused for the same reason**, so `{"channels": 0}` is a mistake rather than a
///   request for the default.
/// * **A width above the record format's limit is refused**, because `Shape.channels` is a `u16`
///   and a value that wrapped would size a ring for a loop nobody could run.
/// * **A non-integer is refused.** JSON has one number type and `2.5` channels is not a typo this
///   should round.
///
/// Returning `None` for all of them is safe by construction: the caller falls back to the shape it
/// had, which is the events-only recording that was correct before this existed.
fn bench_channels(layout: &crate::layout::Layout) -> Option<usize> {
    let text = std::fs::read_to_string(layout.bench()).ok()?;
    let document = skynet_json::Json::parse(&text).ok()?;
    let channels = document
        .get("channels")
        .and_then(skynet_json::Json::as_f64)?;
    if !channels.is_finite() || channels.fract() != 0.0 {
        return None;
    }
    let channels = channels as i64;
    if channels <= 0 || channels > u16::MAX as i64 {
        return None;
    }
    Some(channels as usize)
}

/// Where §25's rings are, for the one package that may write one.
///
/// **Only the commanding package**, and the authority for who that is is
/// [`crate::shape::supervising`] rather than this function: the daemon already asks it which
/// package sized the black box, it refuses to choose when two packages declare an action space,
/// and a second answer here could disagree with the first. A package that is not commanding is
/// told nothing — the address of a command ring is not a thing every unit on the robot needs to
/// know, and §8 is the rule that a package gets what it declared and no more.
///
/// The channel count travels with the path because the policy cannot infer it: §6.2's plan is
/// resolved here, against this body, and a producer that guessed would be a producer whose mask
/// is the wrong width.
pub fn rendezvous(store: &Store, id: &str) -> Vec<(String, String)> {
    let layout = store.layout();
    let body = crate::embodiment::load(layout);
    // `false` for `allow_retarget`, the same reason the boot path gives: §6.1 makes retargeting
    // a decision somebody made rather than a fallback that happens quietly, and a reconcile pass
    // has no somebody in it.
    match supervising(store, &body, false) {
        Supervising::Policy {
            package, channels, ..
        } if package == id => vec![
            (
                "SKYNET_RING_SOCKET".to_string(),
                layout.policy_socket().display().to_string(),
            ),
            ("SKYNET_RING_CHANNELS".to_string(), channels.to_string()),
        ],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use skynet_json::Json;
    #[cfg(unix)]
    use std::path::Path;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::layout::Layout;
    // Only `install` names it, and that is `cfg(unix)` for the symlink swap.
    #[cfg(unix)]
    use crate::layout::PackageId;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TempRoot(PathBuf);

    impl TempRoot {
        fn new(label: &str) -> Self {
            let n = COUNTER.fetch_add(1, Ordering::SeqCst);
            let path = std::env::temp_dir().join(format!(
                "skynetd-shape-{}-{}-{}",
                std::process::id(),
                label,
                n
            ));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp root");
            Self(path)
        }

        fn layout(&self) -> Layout {
            Layout::new(&self.0)
        }

        #[cfg(unix)]
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A body with `joints` revolute joints named `J0..`, each carrying a role a policy can bind to.
    ///
    /// Roles as well as names, because [`skynet_contracts::transport::resolve_transport`] matches
    /// on the exact wire name first and the canonical role second, and a fixture that exercised
    /// only one of those paths would not notice the other breaking.
    fn descriptor(joints: usize) -> String {
        let entries: Vec<String> = (0..joints)
            .map(|index| {
                format!(
                    "{{\"name\":\"J{index}\",\"role\":\"right_arm.joint{index}\",\
                     \"type\":\"revolute\",\
                     \"limits\":{{\"position_min_rad\":-3,\"position_max_rad\":3,\
                     \"velocity_max_rad_s\":2,\"effort_max_nm\":80}},\
                     \"control_modes\":[\"position\"]}}"
                )
            })
            .collect();
        format!(
            "{{\"schema_version\":1,\"robot_slug\":\"bench-arm\",\"digest\":\"sha256:arm\",\
             \"verified\":true,\"base\":\"fixed\",\"dof\":{joints},\"joints\":[{}]}}",
            entries.join(",")
        )
    }

    fn write_descriptor(root: &TempRoot, joints: usize) {
        let layout = root.layout();
        std::fs::create_dir_all(layout.state()).unwrap();
        std::fs::write(layout.embodiment(), descriptor(joints)).unwrap();
    }

    /// A manifest, optionally declaring an action space of `channels` joint-position channels.
    ///
    /// `None` is a package that commands nothing — a skill, a driver, anything without a policy's
    /// output vector — which is the case that has to stay events-only.
    #[cfg(unix)]
    fn manifest(id: &str, channels: Option<usize>) -> Json {
        let mut budget = Json::obj();
        budget
            .set("ram_mb", Json::Num(512.0))
            .set("cpu_cores", Json::Num(1.5));
        let mut runtime = Json::obj();
        runtime.set("entrypoint", Json::from("bin/run"));
        let mut m = Json::obj();
        m.set("schema_version", Json::Num(1.0))
            .set("id", Json::from(id))
            .set("version", Json::from("1.0.0"))
            .set("kind", Json::from("policy"))
            .set("isolation", Json::from("wasm"))
            // Required by `manifest_from_json`, and carried by every real published manifest —
            // the admission corpus's own cases have one. A fixture without it does not decode,
            // which is how the first version of these tests read every package as commanding
            // nothing.
            .set(
                "digest",
                Json::from(
                    "sha256:1111111111111111111111111111111111111111111111111111111111111111",
                ),
            )
            .set("budget", budget)
            .set("runtime", runtime);

        if let Some(channels) = channels {
            let mut actions = Vec::new();
            for index in 0..channels {
                let role = format!("right_arm.joint{index}");
                let joint = format!("J{index}");
                let mut channel = Json::obj();
                channel
                    .set("index", Json::Num(index as f64))
                    .set("kind", Json::from("joint_position"))
                    .set("role", Json::from(role.as_str()))
                    .set("joint", Json::from(joint.as_str()));
                actions.push(channel);
            }
            let mut source = Json::obj();
            source
                .set("robot_slug", Json::from("trainer"))
                .set("digest", Json::from("sha256:policy"))
                .set("dof", Json::Num(channels as f64))
                .set("control_mode", Json::from("position"))
                .set("action_channels", Json::Arr(actions))
                .set("observations", Json::Arr(Vec::new()));
            m.set("source_embodiment", source);
        }
        m
    }

    /// Stage and activate a package, through the store's own path rather than by writing links.
    ///
    /// The activation matters to what is being tested: [`supervising`] reads `installed()`, which
    /// is the *active* set, and a fixture that staged without activating would be asserting about
    /// a rollout that has not happened.
    ///
    /// `cfg(unix)` because activation is the atomic symlink swap §37.1 requires, and this module's
    /// sibling in `tests.rs` states the same rule: *"Most of it runs anywhere. The parts that need
    /// POSIX — symlinks, Unix sockets — are `cfg(unix)`."* The two cases that reach no package run
    /// everywhere.
    #[cfg(unix)]
    fn install(store: &Store, root: &TempRoot, id: &str, channels: Option<usize>) {
        let source = root.path().join(format!("source-{}", id.replace('/', "-")));
        std::fs::create_dir_all(source.join("bin")).unwrap();
        let m = manifest(id, channels);
        std::fs::write(
            source.join("manifest.json"),
            m.to_canonical().unwrap().as_bytes(),
        )
        .unwrap();
        std::fs::write(source.join("bin/run"), b"x").unwrap();

        let files = vec![skynet_contracts::digest::DigestFile {
            path: "bin/run".to_string(),
            sha256: String::from_utf8_lossy(&skynet_sha256::hex_bytes(b"x")).into_owned(),
        }];
        let digest =
            skynet_contracts::digest::version_digest("policy", "1.0.0", &m, &files).unwrap();
        let parsed = PackageId::parse(id).unwrap();
        store
            .stage(&parsed, "policy", "1.0.0", &digest, &source)
            .unwrap();
        store.activate(&parsed, &digest).unwrap();
    }

    fn fixture(label: &str) -> (TempRoot, Store) {
        let root = TempRoot::new(label);
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        (root, store)
    }

    /// The commanding package is told where the rings are, and told how wide its mask must be.
    #[cfg(unix)]
    #[test]
    fn the_commanding_package_is_told_where_the_rings_are() {
        let (root, store) = fixture("rendezvous-policy");
        write_descriptor(&root, 4);
        install(&store, &root, "openrobot/grasp", Some(3));

        let told = rendezvous(&store, "openrobot/grasp");
        assert_eq!(told.len(), 2, "the path and the channel count: {told:?}");
        assert_eq!(
            told[0].1,
            root.layout().policy_socket().display().to_string()
        );
        assert_eq!(
            told[1],
            ("SKYNET_RING_CHANNELS".to_string(), "3".to_string()),
            "§6.2's plan resolved it; a producer that guessed would have the wrong mask width",
        );
    }

    /// Everything else is told nothing, which is §8's rule rather than an omission.
    #[cfg(unix)]
    #[test]
    fn a_package_that_commands_nothing_is_not_told_where_the_ring_is() {
        let (root, store) = fixture("rendezvous-quiet");
        write_descriptor(&root, 4);
        install(&store, &root, "openrobot/grasp", Some(3));
        install(&store, &root, "openrobot/logger", None);

        assert!(
            rendezvous(&store, "openrobot/logger").is_empty(),
            "the address of a command ring is not a thing every unit on the robot needs",
        );
    }

    /// Two commanding packages, and nobody is told anything.
    ///
    /// The daemon does not choose between them for the black box's shape, and it must not choose
    /// here either: handing the ring to whichever the loop happened to iterate first is exactly the
    /// silent arbitration §25.2 describes as two policies driving one elbow.
    #[cfg(unix)]
    #[test]
    fn two_commanding_packages_leave_both_without_a_ring() {
        let (root, store) = fixture("rendezvous-ambiguous");
        write_descriptor(&root, 4);
        install(&store, &root, "openrobot/grasp", Some(3));
        install(&store, &root, "openrobot/walk", Some(2));

        assert!(rendezvous(&store, "openrobot/grasp").is_empty());
        assert!(rendezvous(&store, "openrobot/walk").is_empty());
    }

    /// Write `state/bench.json`, as an operator about to run `skynetk` by hand would.
    fn declare_bench(root: &TempRoot, body: &str) {
        let layout = root.layout();
        std::fs::create_dir_all(layout.state()).unwrap();
        std::fs::write(layout.bench(), body).unwrap();
    }

    /// §80's *"a loop nothing declared"*, closed by the operator declaring it.
    ///
    /// The distinction this rests on: the daemon must not **invent** a loop, and an operator saying
    /// *I am about to run one this wide* is not the daemon inventing anything. It is the same rule
    /// `state/power.json` follows for a machine with two batteries.
    #[test]
    fn a_declared_bench_loop_sizes_the_ring_and_says_it_was_declared() {
        let (root, store) = fixture("bench-declared");
        write_descriptor(&root, 4);
        declare_bench(&root, r#"{"channels": 4}"#);
        let body = crate::embodiment::load(&root.layout());

        let resolved = supervising(&store, &body, false);

        assert_eq!(
            resolved,
            Supervising::Bench {
                joints: 4,
                channels: 4
            }
        );
        assert_eq!(
            resolved.shape(),
            Shape {
                joints: 4,
                channels: 4
            },
            "a cycle fits now, which is the whole of what this buys"
        );
        // The sentence is the other half. A window recorded under a declaration must never read as
        // evidence about a policy, so the operator line says which it is.
        let line = resolved.describe();
        assert!(line.contains("declared bench loop"), "{line}");
        assert!(line.contains("operator's word"), "{line}");
        assert!(!line.contains("transport plan ("), "{line}");
    }

    /// A declaration is weaker than a plan, in the two ways that need a package installed.
    ///
    /// Each is a case where letting the file win would hide something an operator needs to see.
    ///
    /// `cfg(unix)` for `install`'s reason: activation is §37.1's atomic symlink swap. The third
    /// way — a width with no body — needs no package and is asserted below.
    #[cfg(unix)]
    #[test]
    fn a_bench_declaration_never_outranks_a_transport_plan() {
        // A commanding package wins. A bench file left behind on a robot that later installs a
        // policy changes nothing — otherwise the ring would be sized by whoever wrote a file last
        // rather than by two signed documents.
        let (root, store) = fixture("bench-loses-to-policy");
        write_descriptor(&root, 4);
        declare_bench(&root, r#"{"channels": 9}"#);
        install(&store, &root, "openrobot/walk", Some(2));
        let body = crate::embodiment::load(&root.layout());
        let resolved = supervising(&store, &body, false);
        assert_eq!(
            resolved.shape().channels,
            2,
            "the plan's width, not the file's"
        );
        assert!(matches!(resolved, Supervising::Policy { .. }));

        // And it cannot resolve `Ambiguous`. Two commanding packages leave both without a ring
        // (§80), and a bench file papering over that would hide the one case the rule exists for —
        // a robot where nobody knows which policy would command the arm.
        let (root, store) = fixture("bench-does-not-resolve-ambiguity");
        write_descriptor(&root, 4);
        declare_bench(&root, r#"{"channels": 4}"#);
        install(&store, &root, "openrobot/grasp", Some(3));
        install(&store, &root, "openrobot/walk", Some(2));
        let body = crate::embodiment::load(&root.layout());
        let resolved = supervising(&store, &body, false);
        assert!(matches!(resolved, Supervising::Ambiguous { .. }));
        assert_eq!(resolved.shape().channels, 0);
    }

    /// A width with no body is not a shape.
    ///
    /// The third way a declaration is weaker than a plan, and the one that needs no package: a
    /// robot nobody has described has nothing for a cycle to be *about*, however many channels an
    /// operator names.
    #[test]
    fn a_bench_declaration_without_a_body_sizes_nothing() {
        let (root, store) = fixture("bench-needs-a-body");
        declare_bench(&root, r#"{"channels": 4}"#);
        let body = crate::embodiment::load(&root.layout());
        assert_eq!(supervising(&store, &body, false), Supervising::NoBody);
    }

    /// Every malformed declaration falls back to the recording the robot already had.
    ///
    /// Safe by construction — the fallback is events-only, which was correct before this existed —
    /// and worth asserting per case because each has a plausible-looking wrong answer available.
    /// A zero that read as "the default" would let a typo silently disarm a bench session.
    #[test]
    fn a_declaration_that_cannot_be_read_sizes_nothing() {
        for (name, body) in [
            ("empty", ""),
            ("not-json", "channels = 4"),
            ("no-field", r#"{"joints": 4}"#),
            ("zero", r#"{"channels": 0}"#),
            ("negative", r#"{"channels": -4}"#),
            ("fractional", r#"{"channels": 2.5}"#),
            ("above-u16", r#"{"channels": 70000}"#),
            ("a-string", r#"{"channels": "4"}"#),
        ] {
            let (root, store) = fixture(&format!("bench-bad-{name}"));
            write_descriptor(&root, 4);
            declare_bench(&root, body);
            let loaded = crate::embodiment::load(&root.layout());
            assert_eq!(
                supervising(&store, &loaded, false),
                Supervising::NoPolicy { joints: 4 },
                "{name} must not size a ring"
            );
        }
    }

    /// A declared bench loop is not a policy, and §27's condition still says `no_policy`.
    ///
    /// The assertion that keeps the declaration in its lane. §25.2's lease is granted to a policy
    /// the Safety Kernel knows about; a loop an operator started by hand holds none and never will.
    /// What the file changes is the shape of the recording, and deliberately nothing else.
    #[test]
    fn a_declared_bench_loop_is_not_a_policy() {
        let (root, store) = fixture("bench-not-a-policy");
        write_descriptor(&root, 4);
        declare_bench(&root, r#"{"channels": 4}"#);
        let body = crate::embodiment::load(&root.layout());

        let resolved = supervising(&store, &body, false);
        assert!(matches!(resolved, Supervising::Bench { .. }));
        assert_eq!(
            PolicySighting::of(&resolved, |_| true),
            PolicySighting::Nothing { no_body: false },
            "even with something running, nothing installed declares an action space"
        );
    }

    /// No descriptor at all. Nothing can be said about the body, so nothing is claimed about it.
    #[test]
    fn a_robot_with_no_descriptor_records_events_only() {
        let (root, store) = fixture("no-body");
        let body = crate::embodiment::load(&root.layout());

        let resolved = supervising(&store, &body, false);

        assert_eq!(resolved, Supervising::NoBody);
        assert_eq!(
            resolved.shape(),
            Shape {
                joints: 0,
                channels: 0
            }
        );
    }

    /// A body, and nothing installed that commands it. This is the shape the daemon opened
    /// unconditionally before §26.3 was closed, and it is still the right one for this robot.
    #[test]
    fn a_body_with_no_policy_records_events_only() {
        let (root, store) = fixture("no-policy");
        write_descriptor(&root, 4);
        let body = crate::embodiment::load(&root.layout());

        let resolved = supervising(&store, &body, false);

        assert_eq!(resolved, Supervising::NoPolicy { joints: 4 });
        assert_eq!(
            resolved.shape(),
            Shape {
                joints: 0,
                channels: 0
            },
            "joints without channels would size every record for an array no writer fills",
        );
    }

    /// A package that declares no action space does not make a robot a commanded one.
    #[test]
    #[cfg(unix)]
    fn a_package_that_commands_nothing_does_not_size_the_ring() {
        let (root, store) = fixture("non-commanding");
        write_descriptor(&root, 4);
        install(&store, &root, "openrobot/driver", None);
        let body = crate::embodiment::load(&root.layout());

        assert_eq!(
            supervising(&store, &body, false),
            Supervising::NoPolicy { joints: 4 }
        );
    }

    /// The case §26.3 was open on: the daemon sizes its ring for the body, from the plan.
    #[test]
    #[cfg(unix)]
    fn one_policy_sizes_the_ring_for_the_body() {
        let (root, store) = fixture("one-policy");
        write_descriptor(&root, 4);
        install(&store, &root, "openrobot/grasp", Some(3));
        let body = crate::embodiment::load(&root.layout());

        let resolved = supervising(&store, &body, false);

        let Supervising::Policy {
            package,
            joints,
            channels,
            verdict,
        } = &resolved
        else {
            panic!("expected a plan, got {resolved:?}");
        };
        assert_eq!(package, "openrobot/grasp");
        assert_eq!(*joints, 4, "joints are the body's, not the policy's");
        assert_eq!(
            *channels, 3,
            "channels are the policy's output width, which is what a cycle carries",
        );
        assert_eq!(*verdict, "direct");
        assert_eq!(
            resolved.shape(),
            Shape {
                joints: 4,
                channels: 3
            }
        );
    }

    /// Two policies and one control loop. The daemon says so rather than picking one.
    #[test]
    #[cfg(unix)]
    fn two_policies_are_not_a_shape_this_daemon_guesses() {
        let (root, store) = fixture("two-policies");
        write_descriptor(&root, 4);
        install(&store, &root, "openrobot/grasp", Some(3));
        install(&store, &root, "openrobot/walk", Some(2));
        let body = crate::embodiment::load(&root.layout());

        let resolved = supervising(&store, &body, false);

        let Supervising::Ambiguous { packages, joints } = &resolved else {
            panic!("expected ambiguity, got {resolved:?}");
        };
        assert_eq!(*joints, 4);
        assert_eq!(packages.len(), 2);
        assert!(packages.contains(&"openrobot/grasp".to_string()));
        assert!(packages.contains(&"openrobot/walk".to_string()));
        assert_eq!(
            resolved.shape(),
            Shape {
                joints: 0,
                channels: 0
            },
        );
        // The line an operator reads has to name both, or it is a refusal they cannot act on.
        let described = resolved.describe();
        assert!(described.contains("openrobot/grasp"), "{described}");
        assert!(described.contains("openrobot/walk"), "{described}");
    }

    /// A policy this body refuses is a loop that will never run, so its channels are not sized for.
    #[test]
    #[cfg(unix)]
    fn a_refused_plan_does_not_size_the_ring() {
        let (root, store) = fixture("refused");
        // One joint on the body, three required channels in the policy: two have no equivalent.
        write_descriptor(&root, 1);
        install(&store, &root, "openrobot/grasp", Some(3));
        let body = crate::embodiment::load(&root.layout());

        let resolved = supervising(&store, &body, false);

        assert_eq!(
            resolved,
            Supervising::Refused {
                package: "openrobot/grasp".to_string(),
                joints: 1,
            }
        );
        assert_eq!(
            resolved.shape(),
            Shape {
                joints: 0,
                channels: 0
            },
            "a ring sized for a loop that cannot run spends the window on arrays nothing fills",
        );
    }

    /// The property the whole change exists for, asserted as the value the other side compares.
    ///
    /// `skynet-kernel-host`'s recorder refuses an attach when `ring.geometry().shape != shape`,
    /// where its `shape` is the body's joints and the plan's channels. This crate cannot link that
    /// one — §33.1 keeps the daemon and the loop apart, and the two-writer test in `tests.rs`
    /// models the loop the same way — so what is asserted here is the equality that check
    /// performs, against a ring created the way the daemon now creates it.
    #[test]
    #[cfg(unix)]
    fn the_ring_the_daemon_creates_is_one_the_loop_can_attach_to() {
        let (root, store) = fixture("attachable");
        write_descriptor(&root, 4);
        install(&store, &root, "openrobot/grasp", Some(4));
        let body = crate::embodiment::load(&root.layout());
        let layout = root.layout();

        // The daemon's boot path, in order: resolve, then create.
        let resolved = supervising(&store, &body, false);
        let recorder = crate::blackbox::Recorder::open(
            &layout,
            crate::blackbox::supervisor_meta(&layout),
            resolved.shape(),
            64 * 1024,
        )
        .expect("a recorder");

        // What `skynet-kernel-host` would build for this body and this plan.
        let loop_shape = Shape {
            joints: 4,
            channels: 4,
        };
        assert_eq!(
            recorder.geometry().shape,
            loop_shape,
            "the loop compares with `!=` and refuses a mismatch by name",
        );
    }

    /// A manifest that does not decode is not a package that commands nothing.
    ///
    /// Found by running these tests on Linux for the first time: the fixture omitted the `digest`
    /// field `manifest_from_json` requires, every package therefore failed to decode, and the first
    /// version of `supervising` swallowed that with `let Ok(..) else { continue }` — reading four
    /// commanding robots as four robots with no policy. The fixture was wrong and so was the code,
    /// and only one of those would have been caught by fixing the fixture.
    ///
    /// `json.rs` states the rule this restores, where it decodes the same block: *"an action space
    /// nobody could parse and an action space nobody declared are opposite facts."*
    #[test]
    #[cfg(unix)]
    fn a_manifest_that_does_not_decode_is_not_a_package_that_commands_nothing() {
        let (root, store) = fixture("unreadable");
        write_descriptor(&root, 4);
        install(&store, &root, "openrobot/grasp", Some(3));

        // Overwrite the stored manifest with one missing a required field. This is what a truncated
        // write, a partial upgrade or a publisher on an older schema leaves behind — the state
        // `tests.rs` already asserts admission refuses to run against.
        let (_, digest) = store.installed().into_iter().next().expect("one package");
        let bare = "{\"id\":\"openrobot/grasp\",\"version\":\"1.0.0\",\"kind\":\"policy\"}";
        std::fs::write(
            store.layout().blob(&digest).join("manifest.json"),
            bare.as_bytes(),
        )
        .unwrap();

        let body = crate::embodiment::load(&root.layout());
        let resolved = supervising(&store, &body, false);

        let Supervising::Unreadable {
            package,
            joints,
            detail,
        } = &resolved
        else {
            panic!("expected an unreadable manifest, got {resolved:?}");
        };
        assert_eq!(package, "openrobot/grasp");
        assert_eq!(*joints, 4);
        assert!(
            !detail.is_empty(),
            "the operator needs to know what is wrong"
        );

        assert_eq!(
            resolved.shape(),
            Shape {
                joints: 0,
                channels: 0
            },
            "the unreadable manifest may be the one declaring the action space, so the count \
             taken without it cannot be trusted",
        );
        // And it must not read as the robot that simply has no policy, which is the bug.
        assert_ne!(resolved, Supervising::NoPolicy { joints: 4 });
    }

    /// §27's `policy`, and the two of its three values this daemon can establish exactly.
    ///
    /// The mapping is a function over a shape and one boolean, so it is driven here rather than
    /// through a launcher — which matters, because the interesting rows are the ones a fixture
    /// cannot easily produce: two commanding packages, and an unreadable manifest.
    #[test]
    fn the_policy_condition_is_never_ready_and_never_falsely_none() {
        use skynet_contracts::opstate::PolicyState;

        let never = |_: &str| false;
        let always = |_: &str| true;

        // Nothing to run. The one value with no remedy, and it is exactly knowable.
        for shape in [Supervising::NoBody, Supervising::NoPolicy { joints: 4 }] {
            let sighting = PolicySighting::of(&shape, never);
            assert_eq!(sighting.state(), PolicyState::None);
            assert!(sighting.observed());
        }

        // Something to run, four ways, and every one of them observed and `held` rather than
        // `none`. This is the defect: `none` says the store is empty and this process holds it.
        let refused = Supervising::Refused {
            package: "com.example.walk".to_string(),
            joints: 4,
        };
        let unreadable = Supervising::Unreadable {
            package: "com.example.walk".to_string(),
            joints: 4,
            detail: "the manifest is not JSON".to_string(),
        };
        let ambiguous = Supervising::Ambiguous {
            packages: vec!["a".to_string(), "b".to_string()],
            joints: 4,
        };
        let resolved = Supervising::Policy {
            package: "com.example.walk".to_string(),
            joints: 4,
            channels: 3,
            verdict: "direct",
        };

        for shape in [&refused, &unreadable, &ambiguous] {
            let sighting = PolicySighting::of(shape, always);
            assert_eq!(sighting.state(), PolicyState::Held);
            assert!(
                sighting.observed(),
                "a refusal this daemon computed is a reading, not a gap"
            );
            assert_ne!(sighting.state(), PolicyState::None);
        }

        // A plan that resolves and a unit that is not running: still exactly knowable, because a
        // policy whose process does not exist holds no lease.
        let stopped = PolicySighting::of(&resolved, never);
        assert_eq!(stopped.state(), PolicyState::Held);
        assert!(stopped.observed());
        assert!(stopped.detail().contains("unit is not running"));

        // And the one case that is not this daemon's: unobserved, `held`, and never `ready`.
        let running = PolicySighting::of(&resolved, always);
        assert_eq!(running.state(), PolicyState::Held);
        assert!(
            !running.observed(),
            "§25.2's lease is the kernel's; a resolved plan beside a running unit is necessary \
             and not sufficient"
        );
        assert_ne!(
            running.state(),
            PolicyState::Ready,
            "the one value that asserts a lease is the one this process must not produce"
        );
        assert!(running.detail().contains("necessary and not"));
    }

    /// The sentence this closes, stated as the assertion it is.
    ///
    /// §27 rule 8 splits `no_policy` from `policy_not_ready` because *"there is nothing to run"*
    /// and *"there is something and it has not got a lease"* are different sentences and the second
    /// is the one with a remedy. Until this mapping existed the condition sat at its default —
    /// `PolicyState::None` — so a robot with a commanding package installed and activated reported
    /// the first. That is not a cautious reading of an unobserved condition; it is a false
    /// statement about a store this process holds.
    #[test]
    fn an_installed_commanding_package_is_never_reported_as_nothing_to_run() {
        use skynet_contracts::opstate::{
            evaluate, Conditions, HomingState, KernelState, OperatingState, PolicyState,
            PostVerdict, RhalState, StateReason,
        };

        let installed = Supervising::Policy {
            package: "com.example.walk".to_string(),
            joints: 4,
            channels: 3,
            verdict: "direct",
        };
        let sighting = PolicySighting::of(&installed, |_| true);

        // What the default said, and what this says instead.
        assert_eq!(Conditions::default().policy, PolicyState::None);
        assert_eq!(sighting.state(), PolicyState::Held);

        // And the consequence, through §27's own machine rather than by inspection. The conditions
        // above rule 8 have to be satisfied for the difference to be reachable, which is itself the
        // reason this was latent for as long as it was: on this edition `homing` holds every robot
        // three rules earlier.
        let reached = |policy: PolicyState| {
            let conditions = Conditions {
                post: PostVerdict::Passed,
                homing: HomingState::NotRequired,
                kernel: KernelState::Live,
                // `rhal_lost` is rule 1 and its default is `Lost`, so without this every row below
                // is unreachable — which is the machine working: nothing else can be known about a
                // robot whose body cannot be observed.
                rhal: RhalState::Present,
                policy,
                ..Conditions::default()
            };
            let transition = evaluate(OperatingState::Hold, &conditions);
            (transition.to, transition.reason)
        };

        assert_eq!(
            reached(PolicyState::None),
            (OperatingState::Hold, StateReason::NoPolicy),
            "the default's sentence: there is nothing to run"
        );
        assert_eq!(
            reached(sighting.state()),
            (OperatingState::Hold, StateReason::PolicyNotReady),
            "the true one: there is something and it has not got a lease — and it is the sentence \
             with a remedy"
        );
    }
}

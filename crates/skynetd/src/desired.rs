//! Desired state — what the Hub says this robot should be running (§11).
//!
//! > **The Hub stores intent. The robot reconciles.** Never push.
//!
//! Pull-and-reconcile rather than push, for one reason: a robot offline for a week must converge on
//! reconnect without anyone intervening. Push assumes a link robots do not have — in a basement, in
//! a warehouse aisle, on a construction site.
//!
//! Two properties of this document are load-bearing rather than incidental.
//!
//! **The package list is flat and fully pinned** (§37.4). Every entry names an exact digest, and the
//! robot does no version solving. A SAT solver on a robot is a source of unbounded work and
//! non-reproducible outcomes: two robots with the same desired state could resolve differently and
//! neither operator could explain why. The robot's job is narrow — verify, admit, install exactly
//! what it was given, or refuse and say which check failed.
//!
//! **The `generation` counter makes reconciliation idempotent.** A robot reports the generation it
//! has applied; the Hub compares. Nothing else in the exchange needs to be exactly-once.
//!
//! # Decoding, and what a missing field means
//!
//! A desired state that fails to decode is **not** an empty desired state. §33.8 is explicit:
//! corrupt desired state falls back to the last-known-good copy, then to "run nothing", which is
//! the safe default. It never invents one. So [`DesiredState::from_json`] returns an error naming
//! the field rather than a document with holes in it — the caller decides which fallback to take,
//! and the distinction between "the Hub says run nothing" and "we could not read what the Hub said"
//! is one an operator must be able to see.

use skynet_json::Json;

/// What kind of thing a package is, which decides its restart policy (§33.5) and its isolation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    /// A policy: the thing that produces commands.
    Policy,
    /// A long-running service that is not a policy.
    Service,
    /// An RHAL driver. Changing one changes the body (§33.4), and a crashed one is never
    /// automatically restarted (§33.5).
    Driver,
    /// A skill (§54). Installed like a package, run on demand.
    Skill,
}

impl PackageKind {
    /// Parse a kind, returning `None` for one this edition does not implement.
    ///
    /// `None` rather than a default: a package whose kind nobody recognises would otherwise be
    /// supervised under a policy chosen by accident, and the driver rules are the ones that matter.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "policy" => Some(Self::Policy),
            "service" => Some(Self::Service),
            "driver" => Some(Self::Driver),
            "skill" => Some(Self::Skill),
            _ => None,
        }
    }

    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Policy => "policy",
            Self::Service => "service",
            Self::Driver => "driver",
            Self::Skill => "skill",
        }
    }
}

/// Whether a package should be running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// Installed and running.
    Running,
    /// Installed and not running. Distinct from absent: the blob stays, so starting it is a
    /// symlink swap rather than a download (§37.2).
    Stopped,
}

impl RunState {
    /// Parse a run state. Anything unrecognised is `Stopped`, which is the direction that reduces
    /// what the robot does — the asymmetry §33.4 applies throughout.
    pub fn parse(text: &str) -> Self {
        if text == "running" {
            Self::Running
        } else {
            Self::Stopped
        }
    }

    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Stopped => "stopped",
        }
    }
}

/// A detached signature as desired state carries it (§37.1).
///
/// The **key travels with the signature**, and that is not redundancy. An Ed25519 signature does
/// not name the key that made it, so a robot holding a set of trusted keys and a signature that
/// verifies under none of them cannot distinguish bytes that were tampered with from bytes signed
/// correctly by a stranger — an attack and an administrative gap, with the same symptom. Naming the
/// key lets the robot answer the two questions separately, and it answers them in that order.
///
/// Held as strings here, unparsed. This crate is the reconciler and it is pure (§33.3); deciding
/// whether a signature verifies needs the trust store, which is a file, which is `skynetd-io`'s.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredSignature {
    /// The signer's public key, 64 hex characters.
    pub key: String,
    /// The signature, 128 hex characters.
    pub value: String,
}

impl DesiredSignature {
    /// Read `{ "key": …, "value": … }`, or `None` when the field is absent or is not that shape.
    ///
    /// A malformed signature decodes to `None` — "nothing usable arrived" — rather than failing the
    /// whole document. The distinction that matters is downstream: `None` here means the verifier
    /// is told there is no signature, and §37.1's `absent` finding is what an operator sees, with
    /// the shape of the field named in the same breath by the caller that read it.
    pub fn from_json(value: &Json) -> Option<DesiredSignature> {
        let key = value.get("key").and_then(Json::as_str)?;
        let signature = value.get("value").and_then(Json::as_str)?;
        Some(DesiredSignature {
            key: key.to_string(),
            value: signature.to_string(),
        })
    }
}

/// One package the Hub wants installed.
#[derive(Debug, Clone, PartialEq)]
pub struct DesiredPackage {
    /// `namespace/name`, the id everything else refers to it by.
    pub id: String,
    /// Exact version. Informational — the digest is what is fetched.
    pub version: String,
    /// `sha256:…`. Content address, and the only thing identity actually rests on.
    pub digest: String,
    /// What kind of thing this is.
    pub kind: PackageKind,
    /// Whether it should be running.
    pub state: RunState,
    /// Capability tokens this package is granted, sorted.
    ///
    /// A grant is a *permission*, not a lease: holding a grant is what makes a package eligible to
    /// take an exclusive lease on the capability (§25.2), and two packages may hold the same grant
    /// while only one holds the lease.
    pub grants: Vec<String>,
    /// The release signature over the version digest, when one arrived.
    ///
    /// `None` is §37.1's `absent`: nobody attested to these bytes. It is deliberately not a default
    /// that reads as "fine" — a package with no signature reports as unsigned everywhere it is
    /// mentioned, and a **driver** with no signature does not activate at all (§19).
    pub signature: Option<DesiredSignature>,
}

/// The operator profile's ceilings (§38.1), which are the numbers the kernel enforces.
///
/// Owned rather than borrowed, because this document is decoded from bytes and outlives them. The
/// field set mirrors `skynet_kernel::OperatorProfile` deliberately: two spellings of the same
/// ceiling is how a limit gets applied in one place and not the other.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileLimits {
    /// Narrowed joint velocity ceiling.
    pub joint_velocity_max_rad_s: Option<f64>,
    /// Narrowed joint torque ceiling.
    pub joint_torque_max_nm: Option<f64>,
    /// Narrowed base velocity ceiling.
    pub base_velocity_max_m_s: Option<f64>,
    /// Jerk ceiling, as a fraction of the ceiling per cycle.
    pub max_delta_per_cycle: Option<f64>,
    /// How close two collision bodies may come before §7.1's fifth guard holds, in metres.
    ///
    /// A **floor**, not a ceiling, and every rule below has to know that. Absent means zero, which
    /// `skynet_kernel` calls a legal setting and a late one — the surfaces have to actually meet
    /// before anything happens — so a larger margin is the *stricter* profile.
    pub self_collision_margin_m: Option<f64>,
    /// §7.1's speed-and-separation thresholds.
    pub proximity: Option<ProfileProximity>,
    /// Narrower travel for named joints, `(joint, min, max)` in radians (§7.3,
    /// DRY-COVERAGE-PLAN.md F159). The kernel intersects it with the plan's travel.
    pub joint_travel: Vec<(String, f64, f64)>,
}

/// Speed-and-separation thresholds, as a profile carries them.
///
/// The reader's shape rather than the kernel's: `skynet_kernel::Proximity` requires all three, and a
/// profile that declares two of them is a document an operator wrote, not a fault. So they arrive
/// optional and are promoted to the kernel's triple only when all three are present — an incomplete
/// set leaves the guard reporting the input it has not got, which is §7.2 rather than a default.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ProfileProximity {
    /// Below this separation the robot stops. A **larger** distance is stricter.
    pub stop_distance_m: Option<f64>,
    /// Below this separation the robot is slowed. A **larger** distance is stricter.
    pub slow_distance_m: Option<f64>,
    /// The multiplier applied between the two. A **smaller** factor is stricter.
    pub slow_factor: Option<f64>,
}

impl ProfileLimits {
    /// Whether `self` allows no more than `other` on every ceiling both declare.
    ///
    /// This is the classifier §33.4's asymmetry rests on: **anything that reduces what the robot
    /// may do applies immediately, and anything that increases it waits.** So a narrowing is
    /// applied under a running policy and a widening is not.
    ///
    /// Three cases decide it, and the third is the one worth stating:
    ///
    /// * both declare a ceiling — narrower is smaller-or-equal;
    /// * `self` declares one and `other` does not — declaring a ceiling where there was none is a
    ///   narrowing, because absent means the layer above decides and a number is never looser than
    ///   deferring;
    /// * **`self` declares none and `other` did** — that is a *widening*. Dropping a ceiling
    ///   removes a limit, and a profile edit that quietly deletes a field must not be treated as
    ///   the harmless direction.
    pub fn narrows(&self, other: &Self) -> bool {
        narrower(
            self.joint_velocity_max_rad_s,
            other.joint_velocity_max_rad_s,
        ) && narrower(self.joint_torque_max_nm, other.joint_torque_max_nm)
            && narrower(self.base_velocity_max_m_s, other.base_velocity_max_m_s)
            && narrower(self.max_delta_per_cycle, other.max_delta_per_cycle)
            // Not `narrower`. A collision margin and a separation distance are **floors**: a bigger
            // number keeps the robot further away, so smaller-is-stricter would classify a profile
            // that halved the stop distance as a narrowing and apply it under a running policy —
            // which is the precise failure §33.4's asymmetry exists to prevent, arriving through
            // the function written to prevent it.
            && wider(self.self_collision_margin_m, other.self_collision_margin_m)
            && self.proximity_narrows(other.proximity)
            && self.travel_narrows(&other.joint_travel)
    }

    /// Whether every travel `other` bounded is bounded here at least as tightly (F159).
    ///
    /// A travel is two numbers and neither classifier above fits it: the lower end is a floor and
    /// the upper a ceiling. A joint `other` bounded and this profile does not is a widening, for the
    /// reason dropping a ceiling is; a joint bounded here and not in `other` is a narrowing.
    fn travel_narrows(&self, other: &[(String, f64, f64)]) -> bool {
        other.iter().all(|(joint, old_min, old_max)| {
            self.joint_travel
                .iter()
                .find(|(name, _, _)| name == joint)
                .is_some_and(|(_, min, max)| min >= old_min && max <= old_max)
        })
    }

    /// Whether this profile's separation thresholds allow no more than `other`'s.
    ///
    /// Two of the three are floors and the third is a ceiling, so they are classified separately
    /// rather than as a block. Dropping the whole set is a widening for the same reason dropping a
    /// ceiling is: it removes a limit.
    fn proximity_narrows(&self, other: Option<ProfileProximity>) -> bool {
        match (self.proximity, other) {
            (Some(new), Some(old)) => {
                wider(new.stop_distance_m, old.stop_distance_m)
                    && wider(new.slow_distance_m, old.slow_distance_m)
                    && narrower(new.slow_factor, old.slow_factor)
            }
            (Some(_), None) => true,
            (None, None) => true,
            (None, Some(_)) => false,
        }
    }
}

/// Whether `new` is no looser than `old` for a value where **smaller is stricter** — a ceiling.
fn narrower(new: Option<f64>, old: Option<f64>) -> bool {
    match (new, old) {
        (Some(new), Some(old)) => new <= old,
        (Some(_), None) => true,
        (None, None) => true,
        (None, Some(_)) => false,
    }
}

/// Whether `new` is no looser than `old` for a value where **larger is stricter** — a floor.
///
/// The mirror of [`narrower`], and it exists because this file had one classifier and acquired a
/// second kind of number. Every case but the comparison is identical: declaring a floor where there
/// was none is still a narrowing, and dropping one is still a widening.
fn wider(new: Option<f64>, old: Option<f64>) -> bool {
    match (new, old) {
        (Some(new), Some(old)) => new >= old,
        (Some(_), None) => true,
        (None, None) => true,
        (None, Some(_)) => false,
    }
}

/// The operator profile as desired state carries it.
#[derive(Debug, Clone, PartialEq)]
pub struct DesiredProfile {
    /// Which profile, for the notice text and the audit log.
    pub name: String,
    /// sha256 over the canonicalised profile.
    pub digest: String,
    /// The signature over `digest`, as the document carries it.
    pub signature: Option<DesiredSignature>,
    /// Whether that signature verified against a key the operator trusts.
    ///
    /// **Never read from the document, and this is the whole point.** It used to be, and that was a
    /// hole with the kernel's ceilings on the other side of it: §38.1's threat model is *"somebody
    /// editing limits on a robot's disk"*, and the desired state **is** a file on the robot's disk
    /// (`state/desired.json`). Anybody who could edit the limits could edit `"verified": true` in
    /// the same pass, and the robot applied them because the document said they had been checked.
    ///
    /// [`DesiredState::from_json`] therefore sets this to `false` unconditionally, whatever the
    /// document says. `skynetd-io` verifies [`DesiredProfile::signature`] against the trust store
    /// after decoding and sets it, so the only thing that can make it true is a signature that
    /// verified on this robot. CI asserts the decoder never reads a `verified` key.
    pub verified: bool,
    /// The ceilings.
    pub limits: ProfileLimits,
    /// §55.2's cell descriptor, as the profile carries it.
    ///
    /// Carried **raw** rather than decoded here, and that is deliberate: `skynetd` is the pure half
    /// and the cell reader lives in `skynet-contracts`, which this crate deliberately does not
    /// depend on for the reconciler's sake. What matters at this layer is that the document travels
    /// with the profile and is therefore covered by the profile's signature — §55.2 says a cell is
    /// *"signed like every other safety-relevant profile"*, and the cheapest way to make that true
    /// is to not give it a second delivery path.
    ///
    /// `None` is a profile with no cell, which is the ordinary case for a site nobody has surveyed
    /// — and it is not the same as an empty cell, which would draw a workspace layer permitting
    /// nothing.
    pub cell: Option<Json>,
    /// §90.2's two settings for a policy that infers elsewhere. `None` when the profile makes
    /// neither, which is the default the section chooses: the package's own deadline, and motion
    /// that does not resume after a lost session until an operator acknowledges.
    pub remote: Option<ProfileRemote>,
}

/// What the operator profile says about remote policies (§90.2), and it is the only place either
/// may be said.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProfileRemote {
    /// A bound on every remote policy's deadline. A package's own may be tighter and never looser:
    /// the policy host refuses one that is (`deadline_exceeds_profile`).
    pub max_chunk_age_ms: Option<f64>,
    /// Whether motion resumes on its own once a new session delivers a fresh chunk. `false` unless
    /// the profile says `automatic` — §90.2: *"With no setting, it does not, and an operator
    /// acknowledges."*
    pub resume_automatic: bool,
}

/// What the Hub says this robot should be.
#[derive(Debug, Clone, PartialEq)]
pub struct DesiredState {
    /// Monotonic counter. What the robot reports back at check-in (§11).
    pub generation: u64,
    /// OS image channel, e.g. `stable`.
    pub os_channel: String,
    /// OS image version the robot should be running.
    pub os_version: String,
    /// Every package, flat and fully pinned (§37.4).
    pub packages: Vec<DesiredPackage>,
    /// The operator profile, when one is assigned.
    pub profile: Option<DesiredProfile>,
}

/// Why a desired-state document could not be read.
///
/// Carries the field, because §33.8's fallback ladder — last-known-good, then run nothing — is a
/// decision an operator has to be able to understand, and "the desired state is corrupt" is not a
/// sentence anybody can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeError {
    /// The field, in dotted form.
    pub field: String,
    /// What was wrong with it.
    pub detail: String,
}

impl core::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {}", self.field, self.detail)
    }
}

impl DesiredState {
    /// Decode a desired-state document.
    pub fn from_json(value: &Json) -> Result<Self, DecodeError> {
        let generation = value
            .get("generation")
            .and_then(Json::as_f64)
            .ok_or_else(|| {
                err(
                    "generation",
                    "absent, and it is what makes a check-in idempotent",
                )
            })?;
        if !(generation.is_finite() && generation >= 0.0) {
            return Err(err("generation", "is not a counter"));
        }

        let os = value.get("os").cloned().unwrap_or_else(Json::obj);
        let os_channel = string(&os, "channel").unwrap_or_default();
        let os_version = string(&os, "version").unwrap_or_default();

        let mut packages = Vec::new();
        for (index, item) in value.arr_or_empty("packages").iter().enumerate() {
            let at = |field: &str| format!("packages[{index}].{field}");
            let id = string(item, "id").ok_or_else(|| {
                err(
                    &at("id"),
                    "absent; a package with no id cannot be referred to",
                )
            })?;
            let digest = string(item, "digest").ok_or_else(|| {
                err(
                    &at("digest"),
                    "absent. §37.4 requires a fully-pinned list: the robot fetches by content \
                     address and does no version solving",
                )
            })?;
            let kind_text = string(item, "kind").unwrap_or_default();
            let kind = PackageKind::parse(&kind_text).ok_or_else(|| {
                err(
                    &at("kind"),
                    &format!(
                        "`{kind_text}` is not a kind this edition implements. The kind decides the \
                         restart policy and the isolation, so guessing one is how a driver ends up \
                         supervised like a policy"
                    ),
                )
            })?;

            let mut grants: Vec<String> = item
                .arr_or_empty("grants")
                .iter()
                .filter_map(|g| g.as_str().map(String::from))
                .collect();
            grants.sort();
            grants.dedup();

            packages.push(DesiredPackage {
                id,
                version: string(item, "version").unwrap_or_default(),
                digest,
                kind,
                state: RunState::parse(&string(item, "state").unwrap_or_default()),
                grants,
                signature: item.get("signature").and_then(DesiredSignature::from_json),
            });
        }

        // Two entries for one id is a document that says two different things, and picking either
        // would be picking one at random. The Hub resolves; this is a decode error.
        for (index, package) in packages.iter().enumerate() {
            if packages[..index].iter().any(|other| other.id == package.id) {
                return Err(err(
                    &format!("packages[{index}].id"),
                    &format!(
                        "`{}` appears twice, and the two entries may disagree",
                        package.id
                    ),
                ));
            }
        }

        let profile = match value.get_present("profile") {
            None => None,
            Some(raw) => {
                let limits = raw.get("limits").cloned().unwrap_or_else(Json::obj);
                // §90.2. An unknown resume rule is a decode error rather than either rule: reading
                // it as `automatic` would move a robot nobody acknowledged, and reading it as
                // `acknowledge` would hide a profile nobody can apply.
                let remote = match raw.get_present("remote") {
                    None => None,
                    Some(block) => Some(ProfileRemote {
                        max_chunk_age_ms: number(block, "max_chunk_age_ms"),
                        resume_automatic: match string(block, "resume").as_deref() {
                            None | Some("acknowledge") => false,
                            Some("automatic") => true,
                            Some(other) => {
                                return Err(err(
                                    "profile.remote.resume",
                                    &format!(
                                        "`{other}` is not a resume rule. It is `acknowledge` or \
                                         `automatic` (§90.2)"
                                    ),
                                ))
                            }
                        },
                    }),
                };
                Some(DesiredProfile {
                    name: string(raw, "name").unwrap_or_default(),
                    digest: string(raw, "digest").unwrap_or_default(),
                    signature: raw.get("signature").and_then(DesiredSignature::from_json),
                    // Not from the document. See `DesiredProfile::verified`: a document that could
                    // assert its own verification is a document that can raise the kernel's
                    // ceilings by editing one boolean on the robot's own disk.
                    verified: false,
                    limits: ProfileLimits {
                        joint_velocity_max_rad_s: number(&limits, "joint_velocity_max_rad_s"),
                        joint_torque_max_nm: number(&limits, "joint_torque_max_nm"),
                        base_velocity_max_m_s: number(&limits, "base_velocity_max_m_s"),
                        max_delta_per_cycle: number(&limits, "max_delta_per_cycle"),
                        self_collision_margin_m: number(&limits, "self_collision_margin_m"),
                        proximity: limits
                            .get_present("proximity")
                            .map(|prox| ProfileProximity {
                                stop_distance_m: number(prox, "stop_distance_m"),
                                slow_distance_m: number(prox, "slow_distance_m"),
                                slow_factor: number(prox, "slow_factor"),
                            }),
                        joint_travel: travel_table(limits.get_present("joint_travel")),
                    },
                    cell: raw.get_present("cell").cloned(),
                    remote,
                })
            }
        };

        Ok(Self {
            generation: generation as u64,
            os_channel,
            os_version,
            packages,
            profile,
        })
    }

    /// The package with this id, when the document names one.
    pub fn package(&self, id: &str) -> Option<&DesiredPackage> {
        self.packages.iter().find(|package| package.id == id)
    }
}

fn err(field: &str, detail: &str) -> DecodeError {
    DecodeError {
        field: field.to_string(),
        detail: detail.to_string(),
    }
}

fn string(value: &Json, key: &str) -> Option<String> {
    value
        .get_present(key)
        .and_then(Json::as_str)
        .map(String::from)
}

/// A `{ joint: [min, max] }` table, as a profile's `joint_travel` carries it (§7.3, F159).
///
/// A range with an end that is not a finite number is left out rather than guessed at, the rule
/// [`number`] applies to one figure: a malformed range narrows nothing, and the plan's travel stands.
pub fn travel_table(value: Option<&Json>) -> Vec<(String, f64, f64)> {
    let Some(map) = value.and_then(Json::as_obj) else {
        return Vec::new();
    };
    map.iter()
        .filter_map(|(joint, range)| match range.as_arr()? {
            [min, max] => {
                let (min, max) = (min.as_f64()?, max.as_f64()?);
                (min.is_finite() && max.is_finite()).then(|| (joint.clone(), min, max))
            }
            _ => None,
        })
        .collect()
}

fn number(value: &Json, key: &str) -> Option<f64> {
    value
        .get_present(key)
        .and_then(Json::as_f64)
        .filter(|n| n.is_finite())
}

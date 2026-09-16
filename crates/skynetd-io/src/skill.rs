//! §54.4's lifecycle, driven — the loop the contract has been waiting for.
//!
//! Four sessions of §54 produced pure decisions and no caller. `evaluate_preconditions` answers
//! *may this skill start*, `judge_success` answers *did it work*, `skill_deadline` answers *when is
//! it over*, and `advance` answers *where is this invocation*. Every one of them takes its world as
//! an argument, and until now nothing assembled the world or acted on the answer.
//!
//! This module is that, and only that. The decisions stay where they are:
//!
//! | Question | Answered by |
//! |---|---|
//! | May it start? | [`skill::evaluate_preconditions`](skynet_contracts::skill::evaluate_preconditions) |
//! | What may it hold? | [`LeaseTable::acquire_all`](skynetd::lease::LeaseTable::acquire_all) |
//! | Where is it now? | [`skill::advance`](skynet_contracts::skill::advance) |
//! | Did it work? | [`skill::judge_success`](skynet_contracts::skill::judge_success) |
//! | What may a precondition read? | [`live::live_context`](skynet_contracts::live::live_context) |
//!
//! What is here is the sequencing, the state, and the effects.
//!
//! # Decisions in, effects out
//!
//! [`Runtime::advance_all`] does not start a process, take a lease it did not decide to take, write
//! a black box record or queue an episode. It returns [`Effect`]s and the daemon performs them.
//!
//! That is not tidiness. §54.4's terminal row is *"leases released, outcome recorded, episode
//! emitted"* — three things that must happen exactly once, together, on the transition **into**
//! `terminal` and never on a later pass over an invocation that is already there. A level-triggered
//! loop that performed them inline would emit a second episode the first time anything re-entered
//! the function, and the symptom would be duplicate training rows in a dataset nobody is watching.
//! As a returned list they are assertable: the tests below check that a terminal invocation
//! produces exactly one episode and one release, and that a second pass over it produces none.
//!
//! # One live invocation per package, and why that is not a limitation
//!
//! §54.5 says leases are held by the **root invocation**. §25.2's table holds them by *holder
//! string*, and the holder this runtime uses is the package id rather than an invocation id.
//!
//! The two agree because a second concurrent invocation of the same skill package is refused
//! outright. A skill runs as a unit (§33.5) and the supervisor is keyed by package id, so two live
//! invocations of one package would be two lifecycles arguing about one process — and the second
//! one's `Start` would be a start of something already running. Refusing is the honest shape, and it
//! keeps the lease holder equal to the thing an operator sees on the console.
//!
//! # Where the intervention count comes from
//!
//! §33.1 keeps the daemon away from the actuator path, so there is no kernel channel to ask *"is a
//! guard tripped right now"*. This module used to conclude from that that nothing could count an
//! episode's interventions, which does not follow and is the same mistake §74.6 records for
//! telemetry: the kernel does not have to be **asked**, because what it did is already written
//! down. §13's ring holds every cycle with the guards that tripped in it, and this daemon holds
//! the ring.
//!
//! So [`Cycle::interventions_since`] is a **count over the interval since the last pass**, read off
//! the recording by the caller. It replaced a `guard_tripped` boolean and a `guards_observable`
//! boolean beside it, and the pair is worth a sentence because §7.2 dislikes exactly that shape: a
//! value and a second value saying whether the first is real. An `Option` says both, and it cannot
//! be read without the question being asked.
//!
//! `None` is *nothing counted*, and it keeps `intervention_count` **out** of the episode rather
//! than in it as a zero — §54.4 asks for the number and a zero on a robot where nothing counts is
//! evidence a marketplace would quote.
//!
//! ## One invocation at a time, and the reason the limit is here
//!
//! A count read off the ring covers an interval, not an invocation. With one skill running the two
//! are the same thing; with two, the interval's trips belong to whichever policy was commanding and
//! the recording says which by digest — machinery this does not have. So a pass with more than one
//! invocation in `running` leaves the count **unmeasured for all of them**, because a number
//! attributed to the wrong skill is worse than no number, and §7.2's rule is that a measurement
//! nobody can attribute is not one.

use std::collections::{BTreeMap, BTreeSet};

use skynet_contracts::live::{live_context, LiveInputs};
use skynet_contracts::opstate::OperatingState;
use skynet_contracts::skill::{
    advance, bounded_deadline, evaluate_preconditions, judge_success, read_clauses,
    resolve_clauses, skill_deadline, validate_composition, Clause, ClauseOrigin, Deadline,
    LeaseState, LifecycleReason, Phase, Progress, SkillCall, SkillOutcome, SkillPhase,
    SkillTransition, DEFAULT_OVERRUN_FACTOR, MAX_SKILL_DEPTH,
};
use skynet_json::Json;
use skynetd::lease::LeaseTable;

/// What the runtime asks the daemon to do, in the order it must be done.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// Start the skill's unit. §54.4's `running` row: the skill's policy runs "exactly like any
    /// other policy", so this is the ordinary supervisor path and not a second launcher.
    Start {
        /// The package to start.
        package: String,
    },
    /// Stop it. Emitted on every terminal transition that had started something — an ordinary end
    /// and a timeout alike, because §54.4's controlled stop and a skill that returned reach the
    /// same place and the difference is the recorded outcome.
    Stop {
        /// The package to stop.
        package: String,
    },
    /// Release every lease this invocation's package holds (§54.4's terminal row).
    ReleaseLeases {
        /// The holder, which is the package id.
        holder: String,
    },
    /// One episode, ready for §34.4's queue.
    Episode {
        /// The body, canonical-serialisable.
        body: Json,
    },
}

/// What one pass concluded about one invocation.
#[derive(Debug, Clone)]
pub struct Report {
    /// Which invocation.
    pub invocation: String,
    /// Which package.
    pub package: String,
    /// §54.4's transition, whether or not it changed anything.
    pub transition: SkillTransition,
    /// What the daemon must now do, in order.
    pub effects: Vec<Effect>,
}

impl Report {
    /// The black box detail for this transition (§54.4: every transition is an event).
    ///
    /// One line carrying the phase pair, the reason and the outcome, because a sealed recording is
    /// read without the runtime that produced it and a reader has to be able to reconstruct the
    /// ladder from the records alone.
    pub fn event_detail(&self) -> String {
        let outcome = match self.transition.outcome {
            Some(outcome) => format!(" outcome={}", outcome.as_str()),
            None => String::new(),
        };
        format!(
            "{} {} -> {} reason={}{} {}",
            self.package,
            self.transition.from.as_str(),
            self.transition.to.as_str(),
            self.transition.reason.as_str(),
            outcome,
            self.transition.detail,
        )
    }
}

/// Why an invocation was never created.
///
/// §64: a refusal names the fix. These four are the ones that happen before a lifecycle exists to
/// carry a reason, so they carry their own.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    /// A stable code, for a console and for a test.
    pub code: &'static str,
    /// The sentence.
    pub detail: String,
}

/// What a caller asks for (§54.1's `execute({ skill, arguments, timeout_s })`).
#[derive(Debug, Clone)]
pub struct Request {
    /// The skill package.
    pub package: String,
    /// The arguments, carried into the episode unchanged.
    pub arguments: Json,
    /// The root invocation this one is nested under (§54.5), when it is nested.
    pub parent: Option<String>,
    /// §54.4's overrun factor, when the operator set one.
    ///
    /// An operator setting rather than a manifest field, and `skill_deadline` says why: a publisher
    /// choosing how long past their own estimate they may run is a publisher with no timeout.
    pub overrun_factor: Option<f64>,
}

/// Everything one invocation has, beyond what [`Progress`] carries.
#[derive(Debug, Clone)]
pub struct Invocation {
    /// `inv-<n>`, monotonic within this daemon's lifetime.
    pub id: String,
    /// The skill package.
    pub package: String,
    /// What the skill says it does, when its manifest says (§54.1).
    ///
    /// `SkillManifest.task` — *"pick the cup off the table"* — and it is the field §17's episode
    /// column means. The package id is what a robot addresses the thing by; the task is what a
    /// person, or a training set, needs in order to know what it was doing. A dataset whose task
    /// column holds `acme/pick` on every row is a dataset nobody can filter, and that is what this
    /// carried until somebody read the Hub's manifest schema against what the robot consumes.
    pub task: Option<String>,
    /// The digest that was active when it started, so the episode names the bytes that ran.
    pub digest: String,
    /// The arguments.
    pub arguments: Json,
    /// The root invocation, when this one is nested.
    pub parent: Option<String>,
    /// Where §54.4 says it is.
    pub phase: SkillPhase,
    /// Why.
    pub reason: LifecycleReason,
    /// What a console shows beside the reason.
    pub detail: String,
    /// The episode outcome, once there is one.
    pub outcome: Option<SkillOutcome>,

    clauses: Vec<Clause>,
    criteria: Vec<Clause>,
    capabilities: Vec<String>,
    deadline: Deadline,
    root: bool,
    /// The monotonic reading when `execute` accepted it, on the same clock `GET /status` reports
    /// its uptime on. A console subtracts; an invocation that has been in `acquiring` for four
    /// minutes is the one an operator needs to see, and `phase` alone does not say how long.
    pub invoked_ms: u64,
    running_since_ms: Option<u64>,
    ended_ms: Option<u64>,
    /// Wall-clock milliseconds at the terminal transition, when this robot has a clock.
    ///
    /// §35.1 puts measurement on the monotonic clock and correlation on the wall clock, and an
    /// episode needs both: `duration_ms` is measured and must not move when NTP steps, and
    /// `recorded_at` is a point on a timeline the Hub shares with every other robot.
    ///
    /// It matters because of §34.4's queue. A robot offline for a week uploads that week's episodes
    /// on reconnect, and a Hub that stamped arrival time would record them as a spike at the moment
    /// the link returned — every one of them at the wrong hour, on a row whose whole purpose is to
    /// say when something happened. `None` on a robot with no clock, which omits the field rather
    /// than inventing a time.
    ended_wall_ms: Option<u64>,
    started_unit: bool,
    leases: LeaseState,
    lease_token: String,
    lease_holder: String,
    refused_clause: String,
    entry_permitted: Option<bool>,
    hold_lost: bool,
    interventions: Option<u32>,
}

impl Invocation {
    /// Whether §54.4 is finished with it.
    pub fn terminal(&self) -> bool {
        self.phase == SkillPhase::Terminal
    }

    /// How long it ran, in milliseconds, or `None` if it never did.
    ///
    /// From entering `running` to the terminal transition, not from the invocation arriving:
    /// §54.4's episode carries a *duration* and a skill that waited four seconds for a lease did
    /// not take four seconds to do its work.
    pub fn duration_ms(&self) -> Option<u64> {
        let started = self.running_since_ms?;
        let ended = self.ended_ms?;
        Some(ended.saturating_sub(started))
    }

    /// The whole invocation, for `GET /skills`.
    pub fn to_json(&self) -> Json {
        let mut out = Json::obj();
        out.set("id", Json::Str(self.id.clone()))
            .set("package", Json::Str(self.package.clone()))
            .set("digest", Json::Str(self.digest.clone()))
            .set("phase", Json::Str(self.phase.as_str().to_string()))
            .set("reason", Json::Str(self.reason.as_str().to_string()))
            .set("detail", Json::Str(self.detail.clone()))
            .set("root", Json::Bool(self.root))
            .set("leases", Json::Str(self.leases.as_str().to_string()))
            .set("capabilities", str_list(&self.capabilities))
            .set(
                "parent",
                match &self.parent {
                    Some(parent) => Json::Str(parent.clone()),
                    None => Json::Null,
                },
            )
            .set(
                "outcome",
                match self.outcome {
                    Some(outcome) => Json::Str(outcome.as_str().to_string()),
                    None => Json::Null,
                },
            )
            .set(
                "deadline_ms",
                match self.deadline.deadline_ms {
                    Some(ms) => Json::Num(ms),
                    None => Json::Null,
                },
            )
            .set("enforceable", Json::Bool(self.deadline.enforceable))
            .set("invoked_ms", Json::Num(self.invoked_ms as f64))
            .set(
                "duration_ms",
                match self.duration_ms() {
                    Some(ms) => Json::Num(ms as f64),
                    None => Json::Null,
                },
            );
        out
    }

    /// §54.4's episode: *"the skill reference, arguments, outcome, duration and intervention
    /// count"*.
    ///
    /// # It is written in the shape the route reads, not in a shape of its own
    ///
    /// This document is `POST /episodes`'s body, and for one session it was not. The robot sent
    /// `skill`, `duration_ms` and `intervention_count`; the route reads `task`, `duration_seconds`
    /// and `interventions`, ignores everything it does not know, and answers `201`. Every episode
    /// this daemon produced would have been stored with no task, no duration, and every
    /// skill-specific fact discarded — successfully, quietly, with nothing on either side wrong in
    /// isolation. Neither suite could see it: the robot's asserted what the robot sends and the
    /// Hub's asserted what the Hub accepts.
    ///
    /// So the field names here are the route's, and the facts the route has no column for go in
    /// `metadata`, which is what a `jsonb` column is for.
    ///
    /// # Three things that are deliberately not the obvious value
    ///
    /// **`reason` is carried.** §54.4 says one episode per invocation and a refused invocation is
    /// an invocation, so a skill stopped at `preconditions` — nothing ran, no lease was taken, no
    /// duration exists — still produces one. That row is not training data, and a consumer has to
    /// be able to tell it apart from a skill that tried. The reason is what tells them.
    ///
    /// **`interventions` is absent rather than zero** when nothing counted them. §54.4 asks for the
    /// number and the module header says where it comes from — §13's ring, read by the caller —
    /// and why a pass may still leave it unmeasured: no recording to read, or more than one
    /// invocation sharing the interval. A zero in either case would be consumed by a marketplace as
    /// evidence of a clean run. §74.3 states the general rule: zero and unmeasured are opposite
    /// facts.
    ///
    /// **`recorded_at` is when it happened, not when it was sent.** §34.4's queue holds episodes
    /// through an outage, so a Hub stamping arrival time would file a week of work at the minute
    /// the link came back. Omitted on a robot with no clock, because a Hub's guess is better than
    /// this robot's invention.
    fn episode(&self) -> Json {
        let mut out = Json::obj();
        // The route's fields. `task` is what the skill says it does, and falls back to the package
        // id — a poor task label and a correct one, where the alternative is a null column on every
        // row a fleet produces.
        out.set(
            "task",
            Json::Str(self.task.clone().unwrap_or_else(|| self.package.clone())),
        )
        .set(
            "outcome",
            match self.outcome {
                Some(outcome) => Json::Str(outcome.as_str().to_string()),
                None => Json::Str(SkillOutcome::Aborted.as_str().to_string()),
            },
        )
        // `robot` rather than `sim`, and it is a statement about *who acted* rather than about
        // what was underneath: a skill invoked on this daemon ran through §54.4's ladder,
        // whatever the driver beneath it was talking to.
        .set("source", Json::Str("robot".to_string()));
        if let Some(duration) = self.duration_ms() {
            out.set("duration_seconds", Json::Num(duration as f64 / 1_000.0));
        }
        if let Some(count) = self.interventions {
            out.set("interventions", Json::Num(f64::from(count)));
        }
        if let Some(wall_ms) = self.ended_wall_ms {
            out.set("recorded_at", Json::Str(crate::audit::rfc3339(wall_ms)));
        }

        // Everything §54.4 asks for that the route has no column for. Not dropped, and not
        // smuggled into `task` either — a jsonb column is the honest place for facts a schema does
        // not model, and an incident investigation reading these rows needs the invocation id and
        // the digest of the bytes that ran.
        let mut metadata = Json::obj();
        metadata
            .set("invocation", Json::Str(self.id.clone()))
            // The package id, which `task` no longer carries when the manifest declares one. A
            // consumer grouping episodes by what produced them needs an identifier, not a sentence.
            .set("skill", Json::Str(self.package.clone()))
            .set("digest", Json::Str(self.digest.clone()))
            .set("arguments", self.arguments.clone())
            .set("reason", Json::Str(self.reason.as_str().to_string()))
            .set("detail", Json::Str(self.detail.clone()))
            .set("root", Json::Bool(self.root));
        if let Some(parent) = &self.parent {
            metadata.set("parent", Json::Str(parent.clone()));
        }
        if let Some(duration) = self.duration_ms() {
            // Kept beside the seconds the column holds, because the column is `numeric(10,3)` and
            // a duration measured in milliseconds should survive its own rounding.
            metadata.set("duration_ms", Json::Num(duration as f64));
        }
        // The absence, said out loud. A consumer reading `interventions` as null cannot tell a Hub
        // default from a robot that cannot count, and the difference decides whether the row is
        // training data.
        if self.interventions.is_none() {
            metadata.set("interventions_measured", Json::Bool(false));
        }
        out.set("metadata", metadata);
        out
    }
}

/// The world, this pass.
pub struct Cycle<'a> {
    /// Monotonic milliseconds.
    pub now_ms: u64,
    /// §54.2's context, minus the per-package `skill.grants` branch — which is filled in per
    /// invocation, because a grant is granted to a package and two skills on one robot do not hold
    /// the same tokens.
    pub inputs: &'a LiveInputs,
    /// Where §27's machine says the robot is.
    pub robot: OperatingState,
    /// Kernel interventions recorded since the last pass, or `None` when nothing counted.
    ///
    /// Read off §13's ring by the caller — see the module header. `None` is not zero: it keeps
    /// `intervention_count` out of the episode rather than putting it in as evidence of a clean run.
    pub interventions_since: Option<u64>,
    /// Wall-clock milliseconds, when this robot has a clock (§35.1).
    ///
    /// The one thing §35.1 permits wall time for in this module: the timestamp on a record that a
    /// Hub has to place against other robots' records. Nothing here decides anything with it.
    pub wall_ms: Option<u64>,
    /// Which packages the launcher reports running.
    pub running: &'a BTreeSet<String>,
    /// §25.2's table.
    pub leases: &'a mut LeaseTable,
    /// §36.1's `low` row, when it is in force: the sentence, or `None`.
    ///
    /// Assembled by the caller from [`skynet_contracts::power::assess`], because the sentence
    /// depends on *why* — a body nearly flat and a body whose gauge nothing can read both withhold
    /// leases, and they send an operator to different places.
    ///
    /// It gates the **acquisition** and nothing else, which is §36.1's row read exactly: no new
    /// task leases are granted, and a running task may finish. A skill already past `acquiring`
    /// holds its leases and is not disturbed, and the release path is untouched.
    pub power_withheld: Option<String>,
    /// `(package, token)` grants, as `observe::read_grants` returns them.
    pub grants: &'a [(String, String)],
}

impl Cycle<'_> {
    fn granted_to(&self, package: &str) -> Vec<String> {
        let mut tokens: Vec<String> = self
            .grants
            .iter()
            .filter(|(id, _)| id == package)
            .map(|(_, token)| token.clone())
            .collect();
        tokens.sort();
        tokens.dedup();
        tokens
    }
}

fn str_list(values: &[String]) -> Json {
    Json::Arr(values.iter().map(|v| Json::Str(v.clone())).collect())
}

/// Every invocation this daemon knows about.
#[derive(Debug, Default)]
pub struct Runtime {
    invocations: Vec<Invocation>,
    next: u64,
    /// §54.2's context, in the parts of it this robot could not build.
    ///
    /// Refreshed every pass rather than remembered, because both entries are facts about the robot
    /// *now*: a sensor that resumes reporting a finite number stops being unreadable, and a
    /// descriptor somebody replaced changes which names carry a dot. A list that persisted would
    /// tell an operator to chase a sensor that has since recovered.
    ///
    /// Not per invocation, deliberately. `live_context` is built per package because
    /// `skill.grants` differs between two skills on one robot; `unaddressable` and `unreadable`
    /// come from the robot's own facts and are identical for all of them, so hanging a copy on
    /// each invocation would be the same list rendered *n* times.
    omissions: Vec<String>,
}

/// How many terminal invocations are kept for `GET /skills` before the oldest is dropped.
///
/// A bound rather than unbounded history, because this list lives in memory and a robot running a
/// skill every few seconds would grow it forever. The **episode** is the durable record and it has
/// already left through §34.4's queue by the time an entry is dropped here, so what is lost is a
/// console row and never evidence.
pub const KEPT_TERMINAL: usize = 32;

impl Runtime {
    /// An empty runtime.
    pub fn new() -> Self {
        Self::default()
    }

    /// Every invocation, oldest first.
    pub fn invocations(&self) -> &[Invocation] {
        &self.invocations
    }

    /// One invocation by id.
    pub fn get(&self, id: &str) -> Option<&Invocation> {
        self.invocations.iter().find(|inv| inv.id == id)
    }

    /// Whether anything is live.
    pub fn busy(&self) -> bool {
        self.invocations.iter().any(|inv| !inv.terminal())
    }

    /// §54.1's `execute`, refused or accepted.
    ///
    /// `manifest` is the skill package's manifest as the store holds it; `operator` is the site's
    /// added preconditions (§54.2's third rule), which may only lengthen the list.
    ///
    /// Nothing runs here. The invocation is created in `idle` with [`Progress::invoked`] set, and
    /// the next [`Runtime::advance_all`] moves it — so a caller cannot get a skill past §54.2 by
    /// calling `execute` and reading the answer, and every invocation walks the same ladder whether
    /// it was started by an operator, by a cron, or by another skill.
    pub fn execute(
        &mut self,
        request: &Request,
        manifest: &Json,
        digest: &str,
        operator: &[Clause],
        now_ms: u64,
    ) -> Result<String, Refusal> {
        let kind = manifest.get("kind").and_then(Json::as_str).unwrap_or("");
        if kind != "skill" {
            return Err(Refusal {
                code: "not_a_skill",
                detail: format!(
                    "`{}` is a `{}` package. §54.1 separates a skill from a policy by *shape* — a \
                     skill is invoked with arguments and terminates — so there is nothing here to \
                     invoke. Start it with POST /packages/{}/start instead.",
                    request.package,
                    if kind.is_empty() { "kindless" } else { kind },
                    // `namespace:name`, because that route addresses the package in a **path
                    // segment** and an id carries a `/`. §64 asks a refusal to name the fix, and a
                    // fix spelled so it 404s is a refusal that names a detour.
                    request.package.replace('/', ":"),
                ),
            });
        }

        // §54.5's two graph limits, applied to the chain that is actually about to run rather than
        // to the one a manifest declared. `validate_composition` is reused rather than re-derived:
        // a second implementation of "acyclic and depth-limited" is a second answer to the same
        // question, and this one has a corpus behind it.
        let (root_package, calls) = match &request.parent {
            None => (request.package.clone(), Vec::new()),
            Some(parent) => {
                let Some(parent_invocation) = self.get(parent) else {
                    return Err(Refusal {
                        code: "no_such_parent",
                        detail: format!(
                            "This call names {parent} as its root and no such invocation exists. \
                             §54.5 holds leases at the root, so a nested call with no root is a \
                             call that would acquire its own — which is the deadlock the rule \
                             exists to prevent."
                        ),
                    });
                };
                if parent_invocation.terminal() {
                    return Err(Refusal {
                        code: "parent_over",
                        detail: format!(
                            "{parent} is already terminal, so its leases are released and its \
                             deadline has passed. A child of a finished root would run unbounded \
                             and unleased."
                        ),
                    });
                }
                let mut chain = self.ancestry(parent);
                // The parent's *package*, not its invocation id. `validate_composition` walks a
                // graph of skills, and an id in it would make every chain acyclic by accident —
                // ids are unique by construction, so a cycle could never close.
                chain.push((parent_invocation.package.clone(), request.package.clone()));
                let root = chain
                    .first()
                    .map(|(from, _)| from.clone())
                    .unwrap_or_else(|| request.package.clone());
                let calls: Vec<SkillCall> = chain
                    .into_iter()
                    .map(|(from, to)| SkillCall { from, to })
                    .collect();
                (root, calls)
            }
        };
        if let Some(fault) = validate_composition(&root_package, &calls, MAX_SKILL_DEPTH) {
            return Err(Refusal {
                code: fault.code.as_str(),
                detail: format!("{} Chain: {}.", fault.detail, fault.path.join(" → ")),
            });
        }

        // **After the graph check, deliberately.** A skill already live is the reason a *cycle*
        // presents at all — the ancestor is live by definition, so checking this first would report
        // every cycle as "already running" and hide the chain that produced it. A root invocation of
        // something already running is the ordinary case and lands here.
        if let Some(live) = self
            .invocations
            .iter()
            .find(|inv| inv.package == request.package && !inv.terminal())
        {
            return Err(Refusal {
                code: "already_running",
                detail: format!(
                    "`{}` is already invoked as {} and is in `{}`. A skill runs as one unit, so a \
                     second invocation would be a second lifecycle arguing about one process. Wait \
                     for it, or stop it.",
                    request.package,
                    live.id,
                    live.phase.as_str(),
                ),
            });
        }

        let published = read_clauses(
            crate::admit::card_field(manifest, "preconditions"),
            ClauseOrigin::Skill,
        );
        let criteria = read_clauses(
            crate::admit::card_field(manifest, "success_criteria"),
            ClauseOrigin::Skill,
        );
        // Both spellings, like every other field this reads: a manifest may carry the skill card
        // flat or under `skill`. Trimmed, and empty is absent — a task nobody wrote is not a task.
        let task = crate::admit::card_field(manifest, "task")
            .and_then(Json::as_str)
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);

        let capabilities = {
            let mut tokens = manifest.str_arr("required_capabilities");
            tokens.sort();
            tokens.dedup();
            tokens
        };

        let expected =
            crate::admit::card_field(manifest, "expected_duration_seconds").and_then(Json::as_f64);
        let factor = request.overrun_factor.unwrap_or(DEFAULT_OVERRUN_FACTOR);
        let own = skill_deadline(expected, factor);
        // §54.5's third limit. The root's remaining budget bounds the child, including bounding an
        // unbounded child — which is the composition failure the rule names.
        let deadline = match &request.parent {
            None => own,
            Some(parent) => bounded_deadline(&own, self.remaining_ms(parent, now_ms)),
        };

        self.next += 1;
        let id = format!("inv-{}", self.next);
        self.invocations.push(Invocation {
            id: id.clone(),
            package: request.package.clone(),
            task,
            digest: digest.to_string(),
            arguments: request.arguments.clone(),
            parent: request.parent.clone(),
            phase: SkillPhase::Idle,
            reason: LifecycleReason::NotInvoked,
            detail: String::new(),
            outcome: None,
            clauses: resolve_clauses(published, operator.to_vec()),
            criteria,
            capabilities,
            deadline,
            root: request.parent.is_none(),
            invoked_ms: now_ms,
            running_since_ms: None,
            ended_ms: None,
            ended_wall_ms: None,
            started_unit: false,
            leases: LeaseState::Pending,
            lease_token: String::new(),
            lease_holder: String::new(),
            refused_clause: String::new(),
            entry_permitted: None,
            hold_lost: false,
            interventions: None,
        });
        self.forget_old();
        Ok(id)
    }

    /// The `(from, to)` edges from the root down to `id`, root first.
    fn ancestry(&self, id: &str) -> Vec<(String, String)> {
        let mut edges = Vec::new();
        let mut cursor = self.get(id);
        // Bounded by the number of invocations, so a corrupted parent chain cannot spin.
        for _ in 0..self.invocations.len() {
            let Some(current) = cursor else { break };
            let Some(parent) = &current.parent else { break };
            let Some(above) = self.get(parent) else { break };
            edges.push((above.package.clone(), current.package.clone()));
            cursor = Some(above);
        }
        edges.reverse();
        edges
    }

    /// How much of the root's budget is left, for [`bounded_deadline`].
    ///
    /// `None` when the root is unbounded, which leaves the child alone — nothing here invents a
    /// ceiling for a tree nobody bounded.
    fn remaining_ms(&self, parent: &str, now_ms: u64) -> Option<f64> {
        let root = self.get(parent)?;
        let deadline = root.deadline.deadline_ms?;
        if !root.deadline.enforceable {
            return None;
        }
        let elapsed = root
            .running_since_ms
            .map(|started| now_ms.saturating_sub(started) as f64)
            .unwrap_or(0.0);
        Some(deadline - elapsed)
    }

    /// Drop the oldest terminal invocations past [`KEPT_TERMINAL`].
    fn forget_old(&mut self) {
        let terminal = self.invocations.iter().filter(|inv| inv.terminal()).count();
        if terminal <= KEPT_TERMINAL {
            return;
        }
        let mut to_drop = terminal - KEPT_TERMINAL;
        self.invocations.retain(|inv| {
            if to_drop > 0 && inv.terminal() {
                to_drop -= 1;
                false
            } else {
                true
            }
        });
    }

    /// Advance every live invocation by one evaluation.
    ///
    /// Level-triggered and safe to call faster than the work: `advance` returns `running → running`
    /// while a skill's own code runs, so nothing has to remember it already asked.
    pub fn advance_all(&mut self, cycle: &mut Cycle<'_>) -> Vec<Report> {
        // Whether the interval's interventions can be attributed to anybody. See the module header:
        // the ring counts an interval and an episode wants an invocation, and with two skills
        // running the recording knows which policy was commanding by digest and this does not.
        // Counted before anything advances, so a skill that reaches `running` this pass does not
        // change the attribution of the pass it arrived in.
        let solo_running = self
            .invocations
            .iter()
            .filter(|invocation| invocation.phase == SkillPhase::Running)
            .count()
            <= 1;

        // §54.2's omissions, refreshed for the pass. `explain_omissions` documents its own
        // consumer — *"what a console shows about a context it could not fully build"* — and until
        // this line there was no console showing it: `advance_one` built a context, read
        // `built.context` for the preconditions and dropped `unaddressable` and `unreadable` on the
        // floor.
        //
        // It matters because §54.2's refusal names a **path**, which is the question and not the
        // answer. An operator reading *"refused by: robot.tool_temperature > 40"* has no way to
        // learn that the sensor reported NaN and the fact is therefore absent rather than false —
        // and "absent" and "false" send them to two different places.
        //
        // Built from `cycle.inputs` without a package's grants, which is right for what it is used
        // for: `skill.grants` is always addressable and always readable, so it can contribute
        // neither entry.
        self.omissions = skynet_contracts::live::explain_omissions(&live_context(cycle.inputs));

        let mut reports = Vec::new();
        for index in 0..self.invocations.len() {
            if self.invocations[index].terminal() {
                continue;
            }
            let report = self.advance_one(index, cycle, solo_running);
            reports.push(report);
        }
        reports
    }

    /// What §54.2 could not put in a context this pass, one sentence each.
    pub fn omissions(&self) -> &[String] {
        &self.omissions
    }

    fn advance_one(&mut self, index: usize, cycle: &mut Cycle<'_>, solo_running: bool) -> Report {
        // The per-package context. `skill.grants` is filled here rather than by the caller because
        // it is the one branch of §54.2 that differs between two skills on one robot.
        let mut inputs = cycle.inputs.clone();
        inputs.grants = cycle.granted_to(&self.invocations[index].package);
        let built = live_context(&inputs);

        let phase = self.invocations[index].phase;
        let package = self.invocations[index].package.clone();

        // ── Gather the evidence this phase needs ─────────────────────────────
        match phase {
            SkillPhase::Preconditions => {
                let report = evaluate_preconditions(
                    &self.invocations[index].clauses,
                    &built.context,
                    Phase::Entry,
                );
                let invocation = &mut self.invocations[index];
                invocation.refused_clause = report
                    .refused
                    .as_ref()
                    .map(|clause| clause.statement.clone())
                    .unwrap_or_default();
                invocation.entry_permitted = Some(report.permitted);
            }
            // §36.1. Asked before §25.2's table rather than after, and the table is not consulted
            // at all: a refusal that had already taken two of three tokens would be holding an arm
            // for a skill that is about to abort.
            SkillPhase::Acquiring if cycle.power_withheld.is_some() => {}
            SkillPhase::Acquiring => {
                if self.invocations[index].root
                    && self.invocations[index].leases == LeaseState::Pending
                {
                    let tokens = self.invocations[index].capabilities.clone();
                    let granted = cycle.granted_to(&package);
                    // §25.2's safety-behaviour pre-emption is not a skill's to claim. A skill is an
                    // ordinary requester; the one allowed pre-emption belongs to the thing that
                    // stops the robot, and a skill that could pre-empt would be a skill that could
                    // take an arm away from whatever was already using it.
                    let acquisition = cycle.leases.acquire_all(
                        &package,
                        &tokens,
                        |token| granted.iter().any(|held| held == token),
                        false,
                    );
                    let mut progress = Progress::default();
                    acquisition.into_progress(&mut progress);
                    let invocation = &mut self.invocations[index];
                    invocation.leases = progress.leases;
                    invocation.lease_token = progress.lease_token;
                    invocation.lease_holder = progress.lease_holder;
                }
            }
            SkillPhase::Running => {
                let report = evaluate_preconditions(
                    &self.invocations[index].clauses,
                    &built.context,
                    Phase::Hold,
                );
                let invocation = &mut self.invocations[index];
                if !report.permitted {
                    invocation.hold_lost = true;
                    if let Some(clause) = &report.refused {
                        invocation.refused_clause = clause.statement.clone();
                    }
                }
            }
            _ => {}
        }

        // §54.4's intervention count, added rather than edge-detected: the ring reports how many
        // cycles the kernel intervened in over the interval, and two passes that each saw trips are
        // two lots of trips. The rising edge this replaced was a consequence of the input being a
        // boolean, and it collapsed a whole pass of them into one.
        {
            let attributable = solo_running;
            let invocation = &mut self.invocations[index];
            match (cycle.interventions_since, attributable) {
                (Some(count), true) => {
                    *invocation.interventions.get_or_insert(0) += count as u32;
                }
                // Nothing counted, or the interval's trips cannot be told apart between two live
                // invocations. Both leave the episode without the field, which is the honest report
                // and the one §74.3 asks for.
                _ => invocation.interventions = None,
            }
        }

        let progress = self.progress_of(index, cycle);
        let transition = advance(phase, &progress);
        self.apply(
            index,
            &transition,
            cycle.now_ms,
            cycle.wall_ms,
            &built.context,
        )
    }

    /// Assemble [`Progress`] from what has been gathered.
    fn progress_of(&self, index: usize, cycle: &Cycle<'_>) -> Progress {
        let invocation = &self.invocations[index];
        Progress {
            invoked: true,
            root: invocation.root,
            entry_permitted: invocation.entry_permitted,
            refused_clause: invocation.refused_clause.clone(),
            leases: invocation.leases,
            lease_token: invocation.lease_token.clone(),
            lease_holder: invocation.lease_holder.clone(),
            // Read from the cycle rather than remembered on the invocation, because it is a fact
            // about the robot in *this* pass and not about this skill. A robot that came off a
            // charger between passes stops withholding, and the next invocation to reach
            // `acquiring` is entitled to the arm.
            power_withheld: cycle.power_withheld.clone(),
            elapsed_ms: invocation
                .running_since_ms
                .map(|started| cycle.now_ms.saturating_sub(started) as f64),
            deadline: invocation.deadline,
            hold_lost: invocation.hold_lost,
            // §54.4's `running` row ends on a guard trip, which is a yes/no question and the
            // count answers it. `None` reads as `false` here — the same value the daemon used
            // to supply — so a robot that counts nothing aborts no skill it would not have
            // aborted before, which is the safe direction for a transition nobody measured.
            guard_tripped: cycle.interventions_since.is_some_and(|count| count > 0),
            robot: cycle.robot,
            // §54.4's `running` row ends when the skill's own code returns, and the evidence for
            // that is its unit no longer running.
            //
            // **Not on the pass that started it.** `Start` is issued on the transition *into*
            // `running`, and the launcher has not been observed since; reading the process table
            // from before the start would make every skill finish the instant it began. From the
            // next pass onward a missing unit is a skill that returned — including a unit that
            // started and exited between two passes, and including one the launcher refused, which
            // is reported on stderr and then judged by §54.3 over a post-state nothing changed.
            //
            // The earlier draft required having *seen* the unit running instead, which closed that
            // window and opened a worse one: a skill whose process was too short-lived to observe
            // sat in `running` until its timeout, and a skill with no declared duration sat there
            // for ever.
            finished: invocation
                .running_since_ms
                .is_some_and(|started| cycle.now_ms > started)
                && !cycle.running.contains(&invocation.package),
            outcome: invocation.outcome,
        }
    }

    /// Move the invocation and collect what the daemon must do.
    fn apply(
        &mut self,
        index: usize,
        transition: &SkillTransition,
        now_ms: u64,
        wall_ms: Option<u64>,
        post_state: &Json,
    ) -> Report {
        let mut effects = Vec::new();
        {
            let invocation = &mut self.invocations[index];
            invocation.phase = transition.to;
            invocation.reason = transition.reason;
            invocation.detail = transition.detail.clone();

            if transition.changed && transition.to == SkillPhase::Running {
                invocation.running_since_ms = Some(now_ms);
                invocation.started_unit = true;
                effects.push(Effect::Start {
                    package: invocation.package.clone(),
                });
            }
        }

        // §54.3, at the one moment it is asked: the criteria are judged over the context observed
        // *after* the skill returned. The same document a precondition reads, deliberately — a
        // publisher writing `robot.state.gripper` in a criterion and in a precondition is naming
        // one thing, and a second vocabulary for post-state would be the divergence §31.2 warns
        // about wearing different clothes.
        if transition.changed && transition.to == SkillPhase::Verifying {
            let judgement = judge_success(&self.invocations[index].criteria, post_state);
            let invocation = &mut self.invocations[index];
            invocation.outcome = Some(judgement.outcome);
            if let Some(deciding) = &judgement.deciding {
                invocation.detail = format!(
                    "{} — the criterion that decided it: {}",
                    transition.detail, deciding.statement
                );
            }
        }

        if transition.changed && transition.to == SkillPhase::Terminal {
            let invocation = &mut self.invocations[index];
            invocation.ended_ms = Some(now_ms);
            invocation.ended_wall_ms = wall_ms;
            invocation.outcome = transition.outcome.or(invocation.outcome);
            if invocation.started_unit {
                effects.push(Effect::Stop {
                    package: invocation.package.clone(),
                });
            }
            if transition.release_leases {
                effects.push(Effect::ReleaseLeases {
                    holder: invocation.package.clone(),
                });
            }
            effects.push(Effect::Episode {
                body: invocation.episode(),
            });
        }

        Report {
            invocation: self.invocations[index].id.clone(),
            package: self.invocations[index].package.clone(),
            transition: transition.clone(),
            effects,
        }
    }

    /// Stop everything, as `POST /stop` does for packages.
    ///
    /// Every live invocation is driven to `terminal` with `aborted`, because a robot that has been
    /// stopped is a robot whose skills are over and §54.4 has a word for a skill that did not
    /// finish. The episodes are emitted here, so a stop does not lose the record of what was
    /// running when it happened.
    pub fn stop_all(&mut self, now_ms: u64, wall_ms: Option<u64>) -> Vec<Report> {
        let mut reports = Vec::new();
        for index in 0..self.invocations.len() {
            if self.invocations[index].terminal() {
                continue;
            }
            let from = self.invocations[index].phase;
            let transition = SkillTransition {
                from,
                to: SkillPhase::Terminal,
                reason: LifecycleReason::RobotStopped,
                outcome: Some(SkillOutcome::Aborted),
                detail: "the robot was stopped, and §54.4 has a word for a skill that did not \
                         finish"
                    .to_string(),
                release_leases: true,
                changed: true,
            };
            reports.push(self.apply(index, &transition, now_ms, wall_ms, &Json::obj()));
        }
        reports
    }

    /// What a console shows: every invocation, newest first.
    pub fn to_json(&self) -> Json {
        let mut rows = Json::arr();
        for invocation in self.invocations.iter().rev() {
            rows.push(invocation.to_json());
        }
        let mut out = Json::obj();
        out.set("invocations", rows);
        out.set("busy", Json::Bool(self.busy()));
        // Always present, empty where there is nothing to say. A key that appeared only when
        // something was wrong would make its absence mean two things — an edition that does not
        // report omissions, and a robot with none — which is §7.2's rule about the shape rather
        // than about the value.
        out.set("context_omissions", str_list(&self.omissions));
        out
    }
}

/// §54.2's third rule, read from the operator profile.
///
/// > The operator may add preconditions; the skill may not remove them.
///
/// Site clauses live under `skills.<package>.preconditions` in `/data/state/profiles/active.json`,
/// which is the signed document (§38.1) — so a precondition an operator added is covered by the
/// same signature as the ceilings the kernel enforces, and adding one is not a way around the
/// verification that guards the rest of the profile.
///
/// A package id contains a `/`, which is legal in a JSON key and is **not** read as a dot path
/// here: this map is indexed directly rather than by [`predicate::read_path`], so the id needs no
/// escaping. That is worth stating because the neighbouring `env.*` branch has exactly the opposite
/// constraint.
pub fn operator_clauses(profile: Option<&Json>, package: &str) -> Vec<Clause> {
    let Some(profile) = profile else {
        return Vec::new();
    };
    let Some(entry) = profile.get("skills").and_then(|skills| skills.get(package)) else {
        return Vec::new();
    };
    read_clauses(entry.get_present("preconditions"), ClauseOrigin::Operator)
}

/// Which packages the launcher reports running, as [`Cycle::running`] wants them.
pub fn running_set(actual: &skynetd::actual::Actual) -> BTreeSet<String> {
    actual
        .units
        .iter()
        .filter(|unit| unit.state == skynetd::actual::UnitState::Running)
        .map(|unit| unit.id.clone())
        .collect()
}

/// A map from package id to the tokens it was granted, for a report.
pub fn grants_by_package(grants: &[(String, String)]) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (package, token) in grants {
        out.entry(package.clone()).or_default().push(token.clone());
    }
    out
}

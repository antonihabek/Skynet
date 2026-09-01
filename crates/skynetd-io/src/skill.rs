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
//! # What this edition cannot observe, and does not pretend to
//!
//! §33.1 keeps the daemon away from the actuator path, so there is no kernel channel and no guard
//! state. Two consequences, both deliberate:
//!
//! * **`guard_tripped` is an input**, supplied by the daemon, and the daemon supplies `false` with
//!   a reason. When the kernel channel exists it will supply the real thing, and nothing here
//!   changes.
//! * **`intervention_count` is absent from the episode**, not zero. §54.4 asks the episode to carry
//!   it; a count of zero on a robot where nothing counts is a number a marketplace would consume as
//!   evidence. [`Cycle::guards_observable`] is the switch, and it is the same shape
//!   `calibration::conditions_now` uses for a temperature nothing reads.

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
    started_unit: bool,
    leases: LeaseState,
    lease_token: String,
    lease_holder: String,
    refused_clause: String,
    entry_permitted: Option<bool>,
    hold_lost: bool,
    interventions: Option<u32>,
    guard_was_tripped: bool,
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
    /// Two departures from that list, both recorded rather than quiet.
    ///
    /// **`reason` is carried.** §54.4 says one episode per invocation and a refused invocation is
    /// an invocation, so a skill stopped at `preconditions` — nothing ran, no lease was taken, no
    /// duration exists — still produces one. That row is not training data, and a consumer has to
    /// be able to tell it apart from a skill that tried. The reason is what tells them.
    ///
    /// **`intervention_count` is absent rather than zero** when nothing on this robot counts
    /// interventions. §54.4 asks for the number; this edition has no kernel channel and therefore
    /// no source for it, and a zero would be consumed by a marketplace as evidence of a clean run.
    fn episode(&self) -> Json {
        let mut out = Json::obj();
        out.set("invocation", Json::Str(self.id.clone()))
            .set("skill", Json::Str(self.package.clone()))
            .set("digest", Json::Str(self.digest.clone()))
            .set("arguments", self.arguments.clone())
            .set(
                "outcome",
                match self.outcome {
                    Some(outcome) => Json::Str(outcome.as_str().to_string()),
                    None => Json::Str(SkillOutcome::Aborted.as_str().to_string()),
                },
            )
            .set("reason", Json::Str(self.reason.as_str().to_string()))
            .set("detail", Json::Str(self.detail.clone()))
            .set("root", Json::Bool(self.root));
        if let Some(parent) = &self.parent {
            out.set("parent", Json::Str(parent.clone()));
        }
        if let Some(duration) = self.duration_ms() {
            out.set("duration_ms", Json::Num(duration as f64));
        }
        if let Some(count) = self.interventions {
            out.set("intervention_count", Json::Num(f64::from(count)));
        }
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
    /// Whether a guard tripped since the last pass.
    pub guard_tripped: bool,
    /// Whether anything on this robot can answer the question at all.
    ///
    /// `false` means `guard_tripped` is a default and not an observation, and it is what keeps
    /// `intervention_count` out of the episode rather than in it as a zero.
    pub guards_observable: bool,
    /// Which packages the launcher reports running.
    pub running: &'a BTreeSet<String>,
    /// §25.2's table.
    pub leases: &'a mut LeaseTable,
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
            manifest.get_present("preconditions").or_else(|| {
                manifest
                    .get("skill")
                    .and_then(|s| s.get_present("preconditions"))
            }),
            ClauseOrigin::Skill,
        );
        let criteria = read_clauses(
            manifest.get_present("success_criteria").or_else(|| {
                manifest
                    .get("skill")
                    .and_then(|s| s.get_present("success_criteria"))
            }),
            ClauseOrigin::Skill,
        );
        let capabilities = {
            let mut tokens = manifest.str_arr("required_capabilities");
            tokens.sort();
            tokens.dedup();
            tokens
        };

        let expected = manifest
            .get_present("expected_duration_seconds")
            .or_else(|| {
                manifest
                    .get("skill")
                    .and_then(|s| s.get_present("expected_duration_seconds"))
            })
            .and_then(Json::as_f64);
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
            started_unit: false,
            leases: LeaseState::Pending,
            lease_token: String::new(),
            lease_holder: String::new(),
            refused_clause: String::new(),
            entry_permitted: None,
            hold_lost: false,
            interventions: None,
            guard_was_tripped: false,
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
        let mut reports = Vec::new();
        for index in 0..self.invocations.len() {
            if self.invocations[index].terminal() {
                continue;
            }
            let report = self.advance_one(index, cycle);
            reports.push(report);
        }
        reports
    }

    fn advance_one(&mut self, index: usize, cycle: &mut Cycle<'_>) -> Report {
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

        // Interventions are counted only where they can be counted at all (§33.1: there is no
        // kernel channel), and a rising edge rather than every pass a trip persists for.
        {
            let invocation = &mut self.invocations[index];
            if cycle.guards_observable {
                let count = invocation.interventions.get_or_insert(0);
                if cycle.guard_tripped && !invocation.guard_was_tripped {
                    *count += 1;
                }
            }
            invocation.guard_was_tripped = cycle.guard_tripped;
        }

        let progress = self.progress_of(index, cycle);
        let transition = advance(phase, &progress);
        self.apply(index, &transition, cycle.now_ms, &built.context)
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
            elapsed_ms: invocation
                .running_since_ms
                .map(|started| cycle.now_ms.saturating_sub(started) as f64),
            deadline: invocation.deadline,
            hold_lost: invocation.hold_lost,
            guard_tripped: cycle.guard_tripped,
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
    pub fn stop_all(&mut self, now_ms: u64) -> Vec<Report> {
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
            reports.push(self.apply(index, &transition, now_ms, &Json::obj()));
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

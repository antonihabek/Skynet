//! The local API (§33.7).
//!
//! > A Unix socket at `/run/skynetd.sock` and an HTTP listener on the LAN, both carrying **the same
//! > REST shapes as the Hub**. That is not tidiness for its own sake: it means `skynet ps`, `skynet
//! > logs`, `skynet install` and `skynet grants` are the same verbs whether they are pointed at a
//! > robot or at the cloud, and it means the on-robot console and the Hub console can share a
//! > client.
//!
//! | Route | Role (§39.1) | |
//! |---|---|---|
//! | `GET /` | *(none)* | The on-robot console (§32.2). `/console.css` and `/console.js` with it |
//! | `GET /status` | *(none)* | Tier, state, uptime, generation, and whether stop is reachable |
//! | `POST /stop` | *(none)* | Stop every package. The software stop §32.2 puts on the console |
//! | `GET /embodiment` | observer | What body this is (§5): slug, joints, frame graph, and the digest every recording carries |
//! | `GET /capacity` | observer | What `admit_package` would be told about this robot (§8), or why it cannot be assembled |
//! | `GET /state` | observer | §27's operating state, the reason, and which of its conditions anybody observed |
//! | `GET /health` | observer | Per-package liveness and readiness, every field with its age |
//! | `GET /packages` | observer | Installed, active digest, wanted state, grants |
//! | `GET /packages/{ns}:{name}` | observer | One package, with its rollback target |
//! | `GET /plan` | observer | The last reconcile pass, including everything deferred and why |
//! | `GET /cell` | observer | §55.2's cell descriptor, its `env.*` context and its faults |
//! | `GET /grants` | observer | Grants and leases |
//! | `GET /logs/{ns}:{name}` | observer | The package's journal — not the black box (§41.1) |
//! | `GET /blackbox` | observer | The recording's window, what is missing from it, and the sealed files |
//! | `POST /blackbox/seal` | operator | Seal the window now (§13) |
//! | `GET /blackbox/{name}` | operator | Download one sealed recording, as MCAP |
//! | `POST /packages/{id}/start` | operator | |
//! | `POST /packages/{id}/stop` | operator | |
//! | `POST /packages/{id}/rollback` | operator | §37.2's swap in reverse |
//! | `POST /grants` | operator | |
//! | `DELETE /grants/{ns}:{name}/{token}` | operator | |
//!
//! # Why `{ns}:{name}` and not `{ns}/{name}`
//!
//! A package id contains a `/`, and a route with a slash inside a path segment is a route two
//! implementations will parse differently. The `:` is the same substitution [`crate::launch::unit_name`]
//! makes for systemd, and a package id cannot contain one.
//!
//! # Nothing here reports a value it did not receive
//!
//! §32.2's second constraint, and it is why so many fields on `GET /health` are `null`. A robot with
//! no policy runtime cannot say whether a package is *producing*, and a console that showed a green
//! tick for "running, unmeasured" is worse than one that shows the gap. Every field carries its age.

use skynet_json::Json;

use crate::auth::{group, requirement, Credentials};
use crate::http::{Request, Response};
use crate::launch::Launcher;
use crate::layout::PackageId;
use crate::observe::Slots;
use crate::runtime::{Daemon, Outcome};

/// The version this daemon reports.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Everything a request is answered against.
pub struct Context<'a, L: Launcher, S: Slots> {
    /// The daemon.
    pub daemon: &'a mut Daemon<L, S>,
    /// Local credentials (§39.2).
    pub credentials: &'a Credentials,
    /// Monotonic milliseconds, supplied.
    pub now_ms: u64,
    /// When this daemon started, for uptime.
    pub started_ms: u64,
    /// The recorder, when this robot has one.
    ///
    /// An `Option` because a daemon whose black box could not be opened must still serve `POST
    /// /stop`. §28 preallocates the ring so that a full disk cannot silence the recorder; a robot
    /// whose disk was full *before* the ring existed is a different and rarer failure, and refusing
    /// to start would answer it by removing the stop button.
    pub blackbox: Option<&'a mut crate::blackbox::Recorder>,
    /// Monotonic nanoseconds, for the records this request writes.
    pub mono_ns: u64,
    /// Why sending cannot work at all, from `connectivity::Sink::obstacle`.
    pub outbound_obstacle: Option<&'a str>,
    /// Where outbound items would go, from `connectivity::Sink::describe`.
    ///
    /// A string rather than the sink itself, because this is the only thing an API answer needs
    /// from it and passing the sink would give every request handler the ability to send. It is
    /// supplied rather than constructed here for the reason `/outbound` needs it at all: the honest
    /// answer differs between a paired robot, an unpaired one and a developer's machine running
    /// `--outbox`, and only the process that chose the sink knows which of those this is.
    pub outbound_sink: &'a str,
}

/// Answer one request.
pub fn handle<L: Launcher, S: Slots>(
    context: &mut Context<'_, L, S>,
    request: &Request,
) -> Response {
    let segments = request.segments();

    // The console's own three files, answered before anything else looks at a credential. They
    // carry no robot state — every number on the page arrives later, from a route that enforces
    // its own role — and a console you must authenticate to *load* is a console whose stop button
    // is behind a login form (§39).
    if let Some(response) = crate::console::asset(&request.method, &segments) {
        return response;
    }

    let role = context
        .credentials
        .role_of_header(request.header("authorization"));
    let needed = requirement(&request.method, &segments);

    if !needed.satisfied_by(role) {
        // 401 when nothing was presented and 403 when something was: a client that sent no
        // credential should be told to send one, and a client whose credential is real but too
        // narrow should be told that instead of being sent round the pairing flow again.
        let presented = request.header("authorization").is_some();
        let status = if presented { 403 } else { 401 };
        return error(
            status,
            &format!(
                "this needs the {} role{}",
                needed.0.as_str(),
                if context.credentials.is_provisioned() {
                    ""
                } else {
                    ", and this robot has no credentials at all. §39.2: there is no default \
                     password, so an unprovisioned console offers status and stop and nothing else"
                }
            ),
        );
    }

    match (request.method.as_str(), segments.as_slice()) {
        ("GET", ["status"]) => ok(status(context)),
        ("POST", ["stop"]) => ok(stop_all(context)),
        ("GET", ["embodiment"]) => {
            let layout = context.daemon.store.layout();
            let body = crate::embodiment::load(layout);
            let signature = crate::embodiment::signature_verdict(layout, &body);
            ok(crate::embodiment::to_json(&body, &signature))
        }
        ("GET", ["capacity"]) => {
            let body = crate::embodiment::load(context.daemon.store.layout());
            ok(crate::capacity::to_json(&crate::capacity::read(
                &context.daemon.store,
                &body,
                VERSION,
            )))
        }
        ("GET", ["health"]) => ok(health(context)),
        ("GET", ["trust"]) => {
            ok(crate::trust::TrustStore::load(context.daemon.store.layout()).to_report())
        }
        ("GET", ["audit"]) => ok(audit(context)),
        ("GET", ["watchdog"]) => ok(watchdog(context)),
        ("GET", ["post"]) => {
            // Wall time, because the calibration row is an age comparison (§45.4). Not a control
            // decision, so §35.1 permits it.
            let wall_s = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|elapsed| elapsed.as_secs() as f64);
            ok(crate::post::run(context.daemon.store.layout(), wall_s).to_json())
        }
        ("GET", ["state"]) => ok(operating_state(context)),
        ("GET", ["outbound"]) => {
            let layout = context.daemon.store.layout();
            ok(crate::outbound::Queue::of(layout).to_json_with(
                &crate::connectivity::Table::of(layout),
                context.outbound_sink,
                context.outbound_obstacle,
            ))
        }
        ("GET", ["calibration"]) => {
            // Wall time, because §45.4's backstop is elapsed time and a calibration outlives every
            // reboot between it and its expiry — a monotonic reading means nothing across one.
            // §35.1 permits exactly this: it is not a control decision, it is a record's age.
            let wall_s = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|elapsed| elapsed.as_secs() as f64);
            ok(crate::calibration::status(context.daemon.store.layout(), wall_s).to_json())
        }
        // §55.2's cell, and the faults nothing else on this robot reports. An observer's read: it
        // is what the operator wrote down about the space, which is exactly what somebody
        // debugging a refused precondition needs to see.
        ("GET", ["cell"]) => ok(crate::cell::to_json(context.daemon.store.layout())),
        ("GET", ["packages"]) => ok(packages(context)),
        ("GET", ["packages", id]) => package(context, id),
        ("GET", ["plan"]) => ok(plan(context)),
        ("GET", ["grants"]) => ok(grants(context)),
        ("GET", ["logs", id]) => logs(id, request),
        ("GET", ["blackbox"]) => blackbox_index(context),
        ("POST", ["blackbox", "seal"]) => blackbox_seal(context),
        ("GET", ["blackbox", name]) => blackbox_download(context, name),
        ("POST", ["packages", id, "start"]) => act(context, id, Verb::Start),
        ("POST", ["packages", id, "stop"]) => act(context, id, Verb::Stop),
        ("POST", ["packages", id, "rollback"]) => act(context, id, Verb::Rollback),
        ("POST", ["grants"]) => edit_grant(context, request, true),
        ("DELETE", ["grants", id, token]) => remove_grant(context, id, token),
        ("GET", ["ota"]) => ok(ota(context)),
        ("GET", ["identity"]) => ok(identity(context)),
        ("GET", ["checkin"]) => ok(checkin(context)),
        ("GET", ["intent"]) => ok(intent(context)),
        ("GET", ["releases"]) => ok(releases(context)),
        ("GET", ["telemetry"]) => ok(telemetry(context)),
        ("POST", ["identity", "pair"]) => pair(context, request),
        ("POST", ["identity", "unpair"]) => unpair(context, request),
        ("GET", ["skills"]) => ok(context.daemon.skills.to_json()),
        ("GET", ["skills", id]) => match context.daemon.skills.get(id) {
            Some(invocation) => ok(invocation.to_json()),
            None => error(404, "no such invocation"),
        },
        ("POST", ["skills"]) => execute_skill(context, request),
        ("POST", ["skills", id, "stop"]) => stop_skill(context, id),

        // A path nobody serves, and a method nobody serves on a path somebody does, are different
        // answers. A client that gets 404 for a `POST` to a real route goes looking for a typo.
        (_, segments) if known_path(segments) => error(
            405,
            &format!("`{}` is not a method this route answers", request.method),
        ),
        _ => error(404, "no such route"),
    }
}

fn known_path(segments: &[&str]) -> bool {
    matches!(
        segments,
        ["status"]
            | ["stop"]
            | ["embodiment"]
            | ["capacity"]
            | ["health"]
            | ["trust"]
            | ["audit"]
            | ["calibration"]
            | ["outbound"]
            | ["watchdog"]
            | ["post"]
            | ["cell"]
            | ["packages"]
            | ["packages", _]
            | ["packages", _, _]
            | ["logs", _]
            | ["plan"]
            | ["grants"]
            | ["grants", _, _]
            | ["ota"]
            | ["identity"]
            | ["identity", _]
            | ["checkin"]
            | ["intent"]
            | ["releases"]
            | ["telemetry"]
            | ["skills"]
            | ["skills", _]
            | ["skills", _, _]
    )
}

enum Verb {
    Start,
    Stop,
    Rollback,
}

fn act<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>, raw: &str, verb: Verb) -> Response {
    let Some(id) = parse_route_id(raw) else {
        return error(400, "that is not a package id");
    };

    let outcome = match verb {
        // Stopping is always allowed and never waits (§33.4), so it is applied directly.
        Verb::Stop => match context.daemon.launcher.stop(&id.as_str()) {
            Ok(()) => Outcome::Applied,
            Err(error) => Outcome::Failed {
                reason: error.to_string(),
            },
        },
        // Starting is a change in what the robot is doing, so it goes through the reconciler rather
        // than round it: the API records the intent and the next pass applies it when §33.4's gate
        // is open. An endpoint that started a package directly would be a way to bypass the barrier
        // with an HTTP request.
        Verb::Start => {
            let pass = context.daemon.reconcile(context.now_ms);
            if pass
                .applied
                .iter()
                .any(|(action, _)| action.subject() == id.as_str())
            {
                Outcome::Applied
            } else if pass
                .deferred
                .iter()
                .any(|(action, _)| action.subject() == id.as_str())
            {
                Outcome::Failed {
                    reason: "deferred: starting is gated on a safe state (§33.4). It will start \
                             when the gate opens, and `GET /plan` says so"
                        .to_string(),
                }
            } else {
                Outcome::Failed {
                    reason: "the desired state does not ask for this package to be running, so \
                             nothing started it. A robot reconciles to intent (§11) rather than \
                             taking orders"
                        .to_string(),
                }
            }
        }
        Verb::Rollback => match context.daemon.store.rollback(&id) {
            Ok(digest) => {
                let mut body = Json::obj();
                body.set("id", Json::from(id.as_str().as_str()))
                    .set("digest", Json::from(digest.as_str()))
                    .set("outcome", Json::from("applied"));
                return ok(body);
            }
            Err(error) => Outcome::Failed {
                reason: error.to_string(),
            },
        },
    };

    let mut body = Json::obj();
    body.set("id", Json::from(id.as_str().as_str()))
        .set("outcome", Json::from(outcome.as_str()));
    if let Outcome::Failed { reason }
    | Outcome::Unimplemented { reason }
    | Outcome::Waiting { reason } = &outcome
    {
        body.set("reason", Json::from(reason.as_str()));
    }
    let status = if outcome.converged() { 200 } else { 409 };
    Response::json(status, encode(&body))
}

fn status<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let actual = context.daemon.observe(context.now_ms);
    let running = actual
        .units
        .iter()
        .filter(|unit| unit.state == skynetd::actual::UnitState::Running)
        .count();

    let mut body = Json::obj();
    body.set("edition", Json::from("rust"))
        .set("version", Json::from(VERSION))
        .set("contracts", Json::from(skynet_contracts::CONTRACTS_VERSION))
        .set("generation", Json::Num(actual.generation as f64))
        .set(
            "desired_generation",
            Json::Num(context.daemon.desired.generation as f64),
        )
        .set(
            "desired_source",
            Json::from(context.daemon.desired_source.as_str()),
        )
        .set_opt(
            "desired_source_reason",
            context.daemon.desired_source.because().map(Json::from),
        )
        .set("os_version", Json::from(actual.os.active.as_str()))
        .set(
            "uptime_ms",
            Json::Num(context.now_ms.saturating_sub(context.started_ms) as f64),
        )
        .set("packages_running", Json::Num(running as f64))
        // §27's state, which `GET /status` has claimed to carry since this module's own route
        // table was written and did not, because the machine was declared nowhere. `GET /state`
        // is the same fact with the observation gaps spelled out; this is the one line a console
        // header wants.
        .set("state", Json::from(context.daemon.operating_state.as_str()))
        .set(
            "state_reason",
            Json::from(context.daemon.state_reason.as_str()),
        )
        .set(
            "safe_state",
            Json::from(if context.daemon.safe_state.is_safe() {
                "yes"
            } else {
                "no"
            }),
        )
        // Named, because a console reading "safe state: yes" would otherwise read it as something
        // the robot measured. Half of it still is not: §33.1 keeps this daemon away from the body,
        // so the operator's declaration is one of the two sources and §27's machine is the other —
        // and on this edition the machine is answering from one observed condition out of eight.
        .set("safe_state_source", Json::from("declared_and_derived"))
        .set(
            "safe_state_note",
            Json::from(
                "Declared by an operator in /data/state/safe-state, then narrowed by §27's \
                 operating state. Either source may close the barrier and neither opens it alone. \
                 Nothing here has measured the body: skynetd cannot reach it (§33.1), so the \
                 machine is deciding from the self-test and eight unobserved conditions taken at \
                 their least capable values.",
            ),
        )
        // Unconditionally true, and stated rather than assumed. §39.1 makes stop unauthenticated,
        // and a console that could not tell whether stop would work is a console whose stop button
        // is decoration.
        .set("stop_available", Json::Bool(true))
        .set(
            "provisioned",
            Json::Bool(context.credentials.is_provisioned()),
        )
        // Not a tier claim. §15 awards a tier from evidence and nothing here has any, so the field
        // says what this robot is *enforcing* rather than what it has been certified for.
        .set("enforcement", Json::from("guest"));
    body
}

fn stop_all<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    // §54.4's lifecycles first, and the order matters. Every live invocation is driven to
    // `terminal` with `aborted`, which releases its leases and emits its episode — so a stop does
    // not lose the record of what the robot was doing when somebody pressed the button, which is
    // the one moment that record is worth most. Doing it after would mean stopping the units first
    // and letting the next pass discover the skills had "returned", which would judge them by
    // §54.3 and record a `failure` for a robot an operator stopped.
    // The wall clock, for the episode's `recorded_at` and nothing else (§35.1). A stop is exactly
    // the moment it matters: an operator pressing it produces the episodes an investigation reads.
    let ended = context
        .daemon
        .stop_skills(context.now_ms, Some(wall_ms()).filter(|ms| *ms > 0));
    let outcomes = context.daemon.stop_all();
    let mut stopped = Json::arr();
    let mut failed = Json::arr();
    for (id, outcome) in &outcomes {
        match outcome {
            Outcome::Applied | Outcome::AlreadyTrue => {
                stopped.push(Json::from(id.as_str()));
            }
            Outcome::Failed { reason }
            | Outcome::Unimplemented { reason }
            | Outcome::Waiting { reason } => {
                let mut item = Json::obj();
                item.set("id", Json::from(id.as_str()))
                    .set("reason", Json::from(reason.as_str()));
                failed.push(item);
            }
        }
    }
    let mut invocations = Json::arr();
    for report in &ended {
        let mut item = Json::obj();
        item.set("invocation", Json::from(report.invocation.as_str()))
            .set("skill", Json::from(report.package.as_str()))
            .set("from", Json::from(report.transition.from.as_str()))
            .set("outcome", Json::from("aborted"));
        invocations.push(item);
    }

    let mut body = Json::obj();
    body.set("stopped", stopped)
        .set("failed", failed)
        .set("invocations", invocations)
        .set(
            "note",
            Json::from(
                "This stops the processes producing commands. skynetd never touches an actuator \
                 (§33.1); the kernel sees no fresh commands, the watchdog observes it, and the \
                 robot comes to a controlled stop. Any skill that was running is recorded aborted \
                 and its episode emitted.",
            ),
        );
    body
}

/// How many audit entries `GET /audit` answers with.
///
/// The file is the record; this route is a window onto it. A console that asked for eleven months
/// of configuration history over a LAN would be asking a robot to serialise a file that only grows,
/// and the panel showing it has room for a screen. Whoever needs the whole thing reads
/// `state/audit.jsonl`, which is the point of a format one line long.
const AUDIT_WINDOW: usize = 50;

/// §46's chain, as this body declares it.
///
/// Three of the four nesting windows come from the descriptor and one does not: the kernel's
/// worst-case cycle is the kernel's own measurement, and §27's kernel is unwritten, so it is `null`
/// here and reported as missing rather than skipped. §46.3 wants the nesting *"reported at startup
/// rather than discovered in the field"*, and a report that quietly ignored the window it could not
/// get would be reporting on a chain it had not established.
/// `GET /state` — §27's operating state machine.
///
/// The state, the reason, and **which of §27's conditions anybody actually observed**. The third
/// field is what keeps this endpoint from being a lie: this edition can observe one of them, the
/// self-test, and every other condition is left at the least capable reading because §33.1 puts the
/// kernel and the RHAL on the far side of a line this process may not cross.
///
/// A page rendering `safe` with nothing beside it would read as a robot somebody stopped. It is a
/// robot nothing has looked at, which is a different sentence and a different errand.
fn operating_state<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let state = context.daemon.operating_state;

    let mut out = Json::obj();
    out.set("state", Json::Str(state.as_str().to_string()));
    out.set(
        "reason",
        Json::Str(context.daemon.state_reason.as_str().to_string()),
    );
    // The three properties other sections read off the state, rendered rather than left for a
    // console to re-derive. A page that decided for itself whether `stopping` was a safe state
    // would be the second definition of §33.4's barrier.
    out.set("safe_state", Json::Bool(state.is_safe_state()));
    out.set("commands", Json::Bool(state.commands_autonomously()));
    out.set("sticky", Json::Bool(state.is_sticky()));

    // Which of the eight conditions anybody looked at. §48.5 made the same admission about POST's
    // own table and it is the same line being drawn: a supervisor claiming a state it derived from
    // defaults would be claiming to have observed a body it is forbidden to touch.
    let mut observed = Json::obj();
    observed.set("post", Json::Bool(true));
    for condition in [
        "kernel",
        "rhal",
        "homing",
        "stop",
        "guard_hold",
        "guards",
        "policy",
    ] {
        observed.set(condition, Json::Bool(false));
    }
    out.set("observed", observed);
    out.set(
        "partial",
        Json::Str(
            "one of §27's eight conditions was observed here: the self-test. §33.1 keeps skynetd \
             from the actuator path, so the kernel, the RHAL, the guard set, the stop and the \
             homing state are not this process's to read — and an unobserved condition is taken \
             at the least capable value it could have, never at a hopeful one. So this answer is \
             at least as pessimistic as the robot is, and the reason beside it names the \
             condition that decided rather than a fault somebody found"
                .to_string(),
        ),
    );
    out
}

fn watchdog<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let body = crate::embodiment::load(context.daemon.store.layout());
    let known = body.known();
    let declared = known.and_then(|known| known.descriptor.watchdog.as_ref());

    let windows = skynet_contracts::watchdog::Windows {
        // §27 is unwritten, so nothing on this robot has measured a cycle.
        kernel_cycle_worst_case_ms: None,
        envelope_watchdog_ms: known
            .and_then(|known| known.descriptor.envelope.as_ref())
            .and_then(|envelope| envelope.watchdog_ms),
        soc_wdt_ms: declared.and_then(|w| w.soc_wdt_ms),
        drive_command_timeout_ms: declared.and_then(|w| w.drive_command_timeout_ms),
    };

    let mut out = skynet_contracts::json::watchdog_to_json(
        known.and_then(|known| known.descriptor.on_command_loss.as_deref()),
        &windows,
    );
    // Which layers this robot actually has, as against which the chain describes. Layers 1 and 2
    // are software this repository is meant to contain and one of them is written; 3 and 4 are
    // hardware nothing here can see. Reporting the chain without this would let a page of four
    // rows read as four running watchdogs.
    let mut running = Json::arr();
    for (name, state, detail) in [
        (
            "kernel_guard",
            "unavailable",
            "§27's kernel and the policy runtime are unwritten, so nothing evaluates a cycle and \
             nothing pets this",
        ),
        (
            "supervisor",
            "armed",
            "skynetd pings WatchdogSec from inside the reconcile loop, so a hung loop trips it \
             rather than a hung process only",
        ),
        (
            "soc_wdt",
            "unobserved",
            "a hardware window this daemon cannot read; it is declared on the descriptor and \
             petted by the kernel, which is unwritten",
        ),
        (
            "safety_mcu",
            "unobserved",
            "an independent timer on a safety MCU. Nothing in this edition talks to one",
        ),
    ] {
        let mut row = Json::obj();
        row.set("name", Json::Str(name.to_string()));
        row.set("state", Json::Str(state.to_string()));
        row.set("detail", Json::Str(detail.to_string()));
        running.push(row);
    }
    out.set("here", running);
    out
}

/// The recent configuration changes (§38.3), newest first.
fn audit<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let log = crate::audit::Audit::of(context.daemon.store.layout()).read();

    let mut entries = Json::arr();
    for entry in log.entries.iter().rev().take(AUDIT_WINDOW) {
        entries.push(entry.to_json());
    }
    let mut broken = Json::arr();
    for line in &log.broken {
        let mut row = Json::obj();
        row.set("line", Json::Num(line.line as f64));
        row.set("detail", Json::Str(line.detail.clone()));
        broken.push(row);
    }

    let mut document = Json::obj();
    document.set("entries", entries);
    // Reported rather than dropped: a log with invisible gaps is worse than none, because somebody
    // is relying on it. The count is what tells a reader the window they are looking at is partial.
    document.set("broken", broken);
    document.set("total", Json::Num(log.entries.len() as f64));
    document.set("window", Json::Num(AUDIT_WINDOW as f64));
    document
}

fn health<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let actual = context.daemon.observe(context.now_ms);
    let health = context.daemon.health(&actual, context.now_ms);

    let mut packages = Json::arr();
    for package in &health.packages {
        let mut item = Json::obj();
        item.set("id", Json::from(package.id.as_str()))
            .set("live", Json::Bool(package.live))
            .set(
                "readiness",
                Json::from(match package.readiness {
                    skynetd::health::Readiness::Ready => "ready",
                    skynetd::health::Readiness::Stalled => "stalled",
                    skynetd::health::Readiness::NotRunning => "not_running",
                    skynetd::health::Readiness::Unknown => "unknown",
                }),
            )
            .set(
                "last_sequence",
                package
                    .last_sequence
                    .map_or(Json::Null, |n| Json::Num(n as f64)),
            )
            .set("age_ms", Json::Num(package.age_ms as f64));
        packages.push(item);
    }

    let mut body = Json::obj();
    body.set("packages", packages)
        .set("degraded", Json::Bool(health.is_degraded()))
        // Every one of these is `null` because nothing has measured it. §33.6: a health field
        // nobody has measured is absent, never zero — zero guard trips and no measurement of guard
        // trips are opposite facts.
        .set(
            "generation_lag",
            nullable(health.generation_lag.map(|n| n as f64)),
        )
        .set("storage_headroom", nullable(health.storage_headroom))
        .set("guard_trip_rate", nullable(health.guard_trip_rate))
        .set("battery", nullable(health.battery))
        .set("temperature_c", nullable(health.temperature_c))
        .set(
            "unmeasured",
            Json::from(
                "readiness needs a command sequence from the policy runtime, and guard trip rate \
                 needs the kernel. Neither is written, so both report unknown rather than a value.",
            ),
        );
    body
}

fn packages<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let actual = context.daemon.observe(context.now_ms);
    let grants = group(&actual.grants);
    // Read once for the whole list rather than once per row. The store is a file, and a table of
    // twenty packages is not a reason to open it twenty times.
    let trust = crate::trust::TrustStore::load(context.daemon.store.layout());

    let mut list = Json::arr();
    for entry in &actual.installed {
        let signed = context
            .daemon
            .desired
            .package(&entry.id)
            .and_then(|package| package.signature.as_ref())
            .and_then(|raw| crate::trust::SignedBy::from_hex(&raw.key, &raw.value).ok());
        let verdict = trust.check(
            &crate::trust::package_message(&entry.digest),
            signed.as_ref(),
        );

        let mut item = Json::obj();
        item.set("id", Json::from(entry.id.as_str()))
            .set("digest", Json::from(entry.digest.as_str()))
            .set("activated", Json::Bool(entry.activated))
            .set("signature", Json::from(verdict.state.as_str()))
            .set(
                "signer",
                match &verdict.signer {
                    Some(label) => Json::from(label.as_str()),
                    None => Json::Null,
                },
            )
            .set(
                "state",
                Json::from(
                    actual
                        .unit(&entry.id)
                        .map(|unit| match unit.state {
                            skynetd::actual::UnitState::Running => "running",
                            skynetd::actual::UnitState::Stopped => "stopped",
                            skynetd::actual::UnitState::Failed => "failed",
                            skynetd::actual::UnitState::Transitioning => "transitioning",
                        })
                        .unwrap_or("stopped"),
                ),
            )
            .set(
                "wanted",
                Json::from(
                    crate::runtime::wanted_state(&context.daemon.desired, &entry.id)
                        .map_or("absent", |state| state.as_str()),
                ),
            )
            .set(
                "grants",
                Json::Arr(
                    grants
                        .get(&entry.id)
                        .map(|tokens| tokens.iter().map(|t| Json::from(t.as_str())).collect())
                        .unwrap_or_default(),
                ),
            );
        list.push(item);
    }

    let mut body = Json::obj();
    body.set("packages", list);
    body
}

fn package<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>, raw: &str) -> Response {
    let Some(id) = parse_route_id(raw) else {
        return error(400, "that is not a package id");
    };
    let Some(digest) = context.daemon.store.active_digest(&id) else {
        return error(404, "no such package on this robot");
    };

    // The signature the desired state carries for this package, checked here against the trust
    // store. Both halves are read on the request rather than held, so a key an operator has just
    // added is reflected by the next refresh of the console rather than by the next restart.
    let signed = context
        .daemon
        .desired
        .package(&id.as_str())
        .and_then(|package| package.signature.as_ref())
        .and_then(|raw| crate::trust::SignedBy::from_hex(&raw.key, &raw.value).ok());
    let verdict = crate::trust::TrustStore::load(context.daemon.store.layout())
        .check(&crate::trust::package_message(&digest), signed.as_ref());

    let mut body = Json::obj();
    body.set("id", Json::from(id.as_str().as_str()))
        .set("digest", Json::from(digest.as_str()))
        .set(
            "rollback_to",
            context
                .daemon
                .store
                .rollback_digest(&id)
                .map_or(Json::Null, |d| Json::from(d.as_str())),
        )
        .set(
            "manifest",
            context
                .daemon
                .store
                .manifest(&digest)
                .unwrap_or_else(|_| Json::obj()),
        )
        // §37.1's second verification step, answered rather than deferred. The verdict is computed
        // here rather than stored beside the blob: a content-addressed package is the same package
        // whoever signed it, and a recorded "verified" would outlive the trust store it came from.
        //
        // `signer` is the trust store's label, because a report naming a key by its hex has told an
        // operator nothing they can act on.
        .set("signature", Json::from(verdict.state.as_str()))
        .set(
            "signer",
            match &verdict.signer {
                Some(label) => Json::from(label.as_str()),
                None => Json::Null,
            },
        )
        .set("signature_detail", Json::from(verdict.detail.as_str()));
    ok(body)
}

fn plan<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let Some(pass) = context.daemon.last_pass.clone() else {
        let mut body = Json::obj();
        body.set("pass", Json::Null).set(
            "note",
            Json::from("no reconcile pass has run since this daemon started"),
        );
        return body;
    };

    let describe = |action: &skynetd::reconcile::Action| {
        let mut item = Json::obj();
        item.set("stage", Json::Num(f64::from(action.stage)))
            .set("action", Json::from(action_name(&action.kind)))
            .set("subject", Json::from(action.subject()));
        item
    };

    let mut applied = Json::arr();
    for (action, outcome) in &pass.applied {
        let mut item = describe(action);
        item.set("outcome", Json::from(outcome.as_str()));
        if let Outcome::Failed { reason }
        | Outcome::Unimplemented { reason }
        | Outcome::Waiting { reason } = outcome
        {
            item.set("reason", Json::from(reason.as_str()));
        }
        applied.push(item);
    }

    let mut deferred = Json::arr();
    for (action, reason) in &pass.deferred {
        let mut item = describe(action);
        item.set("reason", Json::from(*reason));
        deferred.push(item);
    }

    // Nested under `pass`, and it was not always. The route answered `{"pass": null, "note": …}`
    // when no pass had run and a *flat* object when one had, so a client could not write one parse
    // for both: the console read `value.pass.converged` and threw on every poll from the moment a
    // robot did anything, and the CLI read the flat fields and never saw the bug. Two consumers,
    // two shapes, one route — found by opening the page in a browser, which is the only thing that
    // was ever going to find it.
    let mut inner = Json::obj();
    inner
        .set("at_ms", Json::Num(pass.at_ms as f64))
        .set("applied", applied)
        .set("deferred", deferred)
        .set("converged", Json::Bool(pass.converged()))
        .set(
            "safe_state",
            Json::from(if context.daemon.safe_state.is_safe() {
                "yes"
            } else {
                "no"
            }),
        );

    let mut body = Json::obj();
    body.set("pass", inner);
    body
}

/// `GET /logs/{ns}:{name}` — the journal for one package's unit (§41).
///
/// > **The black box is not a log file.**
///
/// This is the log file, and the distinction is the point of §41.1: the black box is a
/// fixed-schema, every-cycle recording that a fault seals, and journald is where a package's own
/// stderr goes. A console that offered one under the other's name would send somebody looking for a
/// command trace in a stack trace.
///
/// Bounded by construction — `journalctl -n` with a cap — because a log route with no bound is a
/// way to make a robot spend its memory printing.
fn logs(raw: &str, request: &Request) -> Response {
    let Some(id) = parse_route_id(raw) else {
        return error(400, "that is not a package id");
    };
    let lines = request
        .param("lines")
        .and_then(|text| text.parse::<u32>().ok())
        .unwrap_or(200)
        .clamp(1, 5_000);

    let unit = format!("{}.service", crate::launch::unit_name(&id.as_str()));
    let output = std::process::Command::new("journalctl")
        .args([
            "--unit",
            &unit,
            "--no-pager",
            "--output=short-iso",
            "-n",
            &lines.to_string(),
        ])
        .output();

    let mut body = Json::obj();
    body.set("id", Json::from(id.as_str().as_str()))
        .set("unit", Json::from(unit.as_str()));
    match output {
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stdout);
            body.set("lines", Json::Arr(text.lines().map(Json::from).collect()));
            ok(body)
        }
        // No journalctl is a fact about the machine, not an empty log. An empty array here would
        // read as "this package has said nothing", which is the opposite of what happened.
        Err(error) => {
            body.set("lines", Json::Null).set(
                "unavailable",
                Json::from(format!("journalctl could not be run: {error}").as_str()),
            );
            Response::json(503, encode(&body))
        }
    }
}

fn grants<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let actual = context.daemon.observe(context.now_ms);
    let mut held = Json::obj();
    for (id, tokens) in group(&actual.grants) {
        held.set(
            &id,
            Json::Arr(tokens.iter().map(|t| Json::from(t.as_str())).collect()),
        );
    }
    let mut leases = Json::obj();
    for (id, tokens) in group(&actual.leases) {
        leases.set(
            &id,
            Json::Arr(tokens.iter().map(|t| Json::from(t.as_str())).collect()),
        );
    }
    let mut body = Json::obj();
    body.set("grants", held).set("leases", leases).set(
        "note",
        Json::from(
            "A grant is a permission; a lease is an arbitration. Two packages may hold the same \
             grant and only one holds the lease (§25.2).",
        ),
    );
    body
}

fn edit_grant<L: Launcher, S: Slots>(
    context: &mut Context<'_, L, S>,
    request: &Request,
    _add: bool,
) -> Response {
    let Ok(text) = std::str::from_utf8(&request.body) else {
        return error(400, "the body is not UTF-8");
    };
    let Ok(document) = Json::parse(text) else {
        return error(400, "the body is not JSON");
    };
    let (Some(raw), Some(token)) = (
        document.get_present("id").and_then(Json::as_str),
        document.get_present("token").and_then(Json::as_str),
    ) else {
        return error(400, "expected `id` and `token`");
    };
    let Ok(id) = PackageId::parse(raw) else {
        return error(400, "that is not a package id");
    };

    // The grant is written to the desired-state-shaped record on disk; the reconciler is what puts
    // it into force, and §33.4 gates a widening on the package being stopped. An endpoint that
    // granted a capability to a *running* process would hand it a permission its admission decision
    // did not cover — the exact case `Gate::PackageStopped` exists for.
    let layout = context.daemon.store.layout().clone();

    // §54.2's safety notes, at the moment the section names: *"displayed at grant time and
    // acknowledged once per skill per robot."* The refusal carries the notes themselves, so the
    // display and the gate are one exchange rather than a rule an operator is expected to have
    // followed somewhere else.
    if let Some(notes) = notes_for(context, &id) {
        let acknowledging = document
            .get_present("acknowledge")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();

        if !crate::notes::acknowledged(&layout, &id.as_str(), &notes) {
            if acknowledging != notes.digest {
                // 409 rather than 400: the request is well formed and the robot is in a state that
                // refuses it, which is a different thing for a client to handle. A digest that does
                // not match lands here too — acknowledging *other* notes is not acknowledging
                // these, and it is what an operator replaying an old command would send.
                return Response::json(409, encode(&crate::notes::refusal(&id, &notes)));
            }
            let actor = context
                .credentials
                .label_of_header(request.header("authorization"))
                .unwrap_or("unknown")
                .to_string();
            if let Err(failure) = crate::notes::acknowledge(
                &layout,
                &id,
                &notes,
                &crate::audit::Audit::of(&layout),
                &crate::audit::rfc3339(wall_ms()),
                &actor,
            ) {
                // Refused rather than granted-and-unrecorded. An acknowledgement that did not
                // persist is one the next grant would ask for again, and one no incident review
                // could find — which is the whole of what §54.2 asks for.
                return error(
                    500,
                    &format!("the acknowledgement could not be recorded: {failure}"),
                );
            }
        }
    }
    let mut held = crate::observe::read_grants(&layout);
    let entry = (id.as_str(), token.to_string());
    if held.contains(&entry) {
        return ok(grant_body(&id, token, "already_held"));
    }
    held.push(entry);
    held.sort();
    match crate::observe::write_pairs(&layout.grants(), &held) {
        Ok(()) => Response::json(201, encode(&grant_body(&id, token, "granted"))),
        Err(err) => error(500, &err.to_string()),
    }
}

/// The safety notes of the package a grant names, when this robot holds a manifest for it.
///
/// **Two places, in this order**, and both are the same question asked of a different tense. The
/// *active* digest is what the package is right now; the *desired state's* is what it is about to
/// be, and a grant written during a rollout is a grant about the version arriving. Taking the
/// active one first means an operator who is granting to what is running reads what is running.
///
/// `None` for a package this robot does not hold at all. A grant may legitimately be written
/// **before** an install — grants and the desired state are both level-triggered and either order
/// is valid — and refusing one for want of a manifest nobody has fetched would break the ordinary
/// order of a deployment. The note is read at the next grant, which is this rule applied when there
/// is something to read.
fn notes_for<L: Launcher, S: Slots>(
    context: &mut Context<'_, L, S>,
    id: &PackageId,
) -> Option<crate::notes::Notes> {
    let wanted = id.as_str();
    let digest = context.daemon.store.active_digest(id).or_else(|| {
        context
            .daemon
            .desired
            .packages
            .iter()
            .find(|package| package.id == wanted)
            .map(|package| package.digest.clone())
    })?;
    let manifest = context.daemon.store.manifest(&digest).ok()?;
    crate::notes::of(&manifest)
}

fn remove_grant<L: Launcher, S: Slots>(
    context: &mut Context<'_, L, S>,
    raw: &str,
    token: &str,
) -> Response {
    let Some(id) = parse_route_id(raw) else {
        return error(400, "that is not a package id");
    };
    let layout = context.daemon.store.layout().clone();
    let mut held = crate::observe::read_grants(&layout);
    let before = held.len();
    held.retain(|(package, existing)| !(package == &id.as_str() && existing == token));
    if held.len() == before {
        return ok(grant_body(&id, token, "not_held"));
    }
    // Revocation is the safe direction and never waits, including on a running package (§33.4). The
    // lease goes with it, so a revocation cannot take the permission away and leave the exclusivity.
    context.daemon.leases.release(&id.as_str(), token);
    match crate::observe::write_pairs(&layout.grants(), &held) {
        Ok(()) => ok(grant_body(&id, token, "revoked")),
        Err(err) => error(500, &err.to_string()),
    }
}

fn grant_body(id: &PackageId, token: &str, outcome: &str) -> Json {
    let mut body = Json::obj();
    body.set("id", Json::from(id.as_str().as_str()))
        .set("token", Json::from(token))
        .set("outcome", Json::from(outcome));
    body
}

/// `ns:name` back into a package id.
/// `GET /blackbox` — the index (§39.1: observer).
///
/// The window, the geometry, what is missing, and the sealed files. Not the recording: an index a
/// console polls must not be tens of megabytes.
/// `GET /ota` — §12's state, and what §12 would say about the version the Hub is asking for.
///
/// Three facts on one route because they are one question. Which images are in the two slots is
/// where the robot **is**; the verdict is whether it may move; the pending-boot state is whether
/// the last move is still on probation. A console showing the slots without the verdict would
/// render a robot that appears able to update and is refused every pass, which is the §32.2 failure
/// this file exists not to commit.
/// Where this robot's desired state came from (§11, §33.8).
///
/// Two answers on one route because there are exactly two: a pull, or §33.8's fallback ladder. A
/// route showing one of them would leave the other invisible — and the fallback is the one that
/// matters, because a robot reconciling against a last-known-good copy looks identical from every
/// other surface to one reconciling against a current desired state.
///
/// **It does not pull.** A `GET` that reached out to a Hub would be a network round trip behind a
/// verb that promises to read, and it would let anyone with an observer credential make this robot
/// talk to the internet. The reconcile loop pulls; this reports what the last one did.
fn intent<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let layout = context.daemon.store.layout().clone();
    let (_, source) = crate::runtime::load_desired(&layout);
    crate::intent::to_json(&context.daemon.last_pull, &source)
}

/// §74's open telemetry window, and what the contract makes of it.
///
/// The window **being counted** rather than the last one sent, for [`checkin`]'s reason: on a robot
/// this edition runs the second is very often nothing, and a route answering `null` would leave an
/// operator unable to see whether their robot is counting at all.
///
/// Reading it does not close it. A `GET` that closed a window would make every reader of this route
/// a producer of telemetry — and two people with the console open would tile the fleet's windows
/// differently from each other.
fn telemetry<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let layout = context.daemon.store.layout().clone();
    let queue = crate::outbound::Queue::of(&layout);
    let dropped: u64 = queue.status().dropped.iter().map(|(_, count)| *count).sum();
    crate::telemetry::to_json(
        &context.daemon.counting,
        wall_ms() as f64,
        context.now_ms as f64,
        dropped,
    )
}

/// What happened the last time this robot asked for the release catalogue (§12, §57).
///
/// **It does not pull**, for [`intent`]'s reason and one more: this request goes out with the
/// device credential, so a `GET` on the LAN that triggered one would let anybody holding an
/// observer credential spend a fleet's metered allowance a request at a time.
///
/// The channel and version are echoed from the desired state, because a pull that found nothing and
/// a robot that asked for nothing are different states and only one of them is a problem.
fn releases<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let mut out = crate::releases::to_json(&context.daemon.last_release_pull);
    out.set(
        "channel",
        Json::Str(context.daemon.desired.os_channel.clone()),
    )
    .set(
        "version",
        Json::Str(context.daemon.desired.os_version.clone()),
    )
    .set("arch", Json::Str(crate::releases::arch().to_string()));
    out
}

/// What this robot would tell the Hub right now (§11, §17).
///
/// The **document that would be sent**, not the last one that was — because on a robot this edition
/// runs, the second is very often nothing, and a route that answered `null` would leave an operator
/// unable to see what their robot is about to claim about itself. Whether one is queued is a
/// separate field beside it, so the two questions an operator actually has — *what does it say* and
/// *is anybody hearing it* — are answered separately.
fn checkin<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let layout = context.daemon.store.layout().clone();
    let trust = crate::trust::TrustStore::load(&layout);
    let (identity, _) = crate::identity::evaluate(&layout, &trust);

    let actual = context.daemon.observe(context.now_ms);
    let health = context.daemon.health(&actual, context.now_ms);
    let policy = crate::identity::running_policy(&context.daemon.store, &actual);
    let attestation =
        crate::identity::attestation(&layout, &trust, &identity, &actual.os.active, &policy);
    let progress =
        crate::checkin::progress(&context.daemon.desired, &actual, context.daemon.safe_state);
    let lag = context
        .daemon
        .desired
        .generation
        .checked_sub(actual.generation)
        .filter(|_| context.daemon.desired.generation >= actual.generation);
    let rolled = crate::checkin::health(&health, 0, lag);

    let built = crate::checkin::build(
        &identity,
        attestation,
        progress,
        rolled,
        context.daemon.operating_state.as_str(),
        &crate::intent::held_digest(&layout),
    );

    // Read, never written. This route reports what a check-in *would* say; the reconcile loop is
    // what queues one, and a `GET` that enqueued would be a state change behind a verb that
    // promises not to be one.
    let queued = if identity.identity.credential {
        Ok(None)
    } else {
        Err(crate::checkin::NotQueued::Unpaired)
    };
    crate::checkin::to_json(&built, &queued)
}

/// Wall time, for the two things §35.1 allows it (§35.1).
///
/// Never a deadline, a backoff or a watchdog. §35.1 gives `CLOCK_REALTIME` two jobs — logging and
/// *"correlation with the Hub"* — and this daemon has exactly one caller of each: an audit entry's
/// timestamp, and §74's telemetry window bounds. Every rate those windows produce is divided by the
/// **monotonic** duration beside them, for the reason §74.2 spells out.
///
/// One function rather than two, because the second caller is where a rule dies (finding 23) and
/// the rule here is which clock this is allowed to be.
pub fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// §10's one-time operator action, over the local API.
///
/// The code arrives in a **body** rather than in a path or a query string, for the reason it never
/// reaches curl's argument list either: a URL is in an access log and a request body is not. The
/// endpoint arrives with it because §39.2 gives this robot no default of any kind — there is no
/// default password and there is no default Hub, and a robot that would talk to a host somebody
/// else chose is the same mistake in a different field.
fn pair<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>, request: &Request) -> Response {
    let Ok(text) = std::str::from_utf8(&request.body) else {
        return error(400, "the body is not UTF-8");
    };
    let Ok(document) = Json::parse(text) else {
        return error(400, "the body is not JSON");
    };
    let (Some(code), Some(endpoint)) = (
        document.get_present("code").and_then(Json::as_str),
        document.get_present("endpoint").and_then(Json::as_str),
    ) else {
        return error(
            400,
            "expected `code` and `endpoint`. There is no default Hub, for §39.2's reason: a robot \
             that would report to a host nobody named is a robot somebody else can adopt",
        );
    };

    let layout = context.daemon.store.layout().clone();
    let trust = crate::trust::TrustStore::load(&layout);
    let enrolment = crate::identity::CurlEnrolment {
        endpoint: endpoint.to_string(),
        scratch: layout.state(),
    };
    let actor = context
        .credentials
        .label_of_header(request.header("authorization"))
        .unwrap_or("unknown")
        .to_string();

    match crate::identity::pair(
        &layout,
        &trust,
        code,
        &enrolment,
        &crate::audit::Audit::of(&layout),
        &crate::audit::rfc3339(wall_ms()),
        &actor,
    ) {
        Ok(paired) => {
            let mut out = Json::obj();
            out.set("device_id", Json::from(paired.device_id.as_str()))
                .set("namespace", Json::from(paired.namespace.as_str()))
                .set("endpoint", Json::from(paired.endpoint.as_str()))
                .set("attested", Json::Bool(paired.verdict.attested));
            let verdict = skynet_contracts::identity::identity_verdict_to_json(&paired.verdict);
            out.set(
                "findings",
                verdict.get("findings").cloned().unwrap_or_else(Json::arr),
            );
            Response::json(201, encode(&out))
        }
        // 409 rather than 400 for an already-paired robot: the request was well formed and the
        // robot is in a state that refuses it, which is a different thing for a client to handle.
        Err(failure) if failure.code == "already_paired" => error(409, &failure.detail),
        Err(failure) => error(400, &failure.detail),
    }
}

/// Undo a pairing (§39.1 gives it to Admin, beside pair).
fn unpair<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>, request: &Request) -> Response {
    let layout = context.daemon.store.layout().clone();
    let actor = context
        .credentials
        .label_of_header(request.header("authorization"))
        .unwrap_or("unknown")
        .to_string();
    match crate::identity::unpair(
        &layout,
        &crate::audit::Audit::of(&layout),
        &crate::audit::rfc3339(wall_ms()),
        &actor,
    ) {
        Ok(Some(namespace)) => {
            let mut out = Json::obj();
            out.set("unpaired", Json::Bool(true))
                .set(
                    "namespace",
                    if namespace.is_empty() {
                        Json::Null
                    } else {
                        Json::from(namespace.as_str())
                    },
                )
                .set(
                    "note",
                    Json::from(
                        "the credential and the identity document were removed. Revoke the \
                         credential on the Hub — nothing here can.",
                    ),
                );
            ok(out)
        }
        Ok(None) => {
            let mut out = Json::obj();
            out.set("unpaired", Json::Bool(false))
                .set("namespace", Json::Null)
                .set(
                    "note",
                    Json::from("this robot was not paired, so nothing was removed."),
                );
            ok(out)
        }
        Err(failure) => error(500, &failure.detail),
    }
}

/// Who this robot is, what it is, and what it is running (§10).
///
/// Three facts on one route because §50.2 makes them one question asked of two parties: the birth
/// certificate is what the vendor attested at manufacture, the identity is what an operator bound
/// it to at pairing, and the attestation is what it is executing right now. A console that showed
/// the first two without the third would render a robot's identity without the sentence §10 says an
/// incident investigation needs.
///
/// **No credential is on this route**, in any form. See `identity::to_json`.
fn identity<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let layout = context.daemon.store.layout().clone();
    let trust = crate::trust::TrustStore::load(&layout);
    let (held, verdict) = crate::identity::evaluate(&layout, &trust);

    let mut out = crate::identity::to_json(&held, &verdict);

    let actual = context.daemon.observe(context.now_ms);
    let policy = crate::identity::running_policy(&context.daemon.store, &actual);
    let attestation =
        crate::identity::attestation(&layout, &trust, &held, &actual.os.active, &policy);
    out.set(
        "attestation",
        crate::identity::attestation_to_json(&attestation),
    );

    // The certificate, beside the identity rather than on a route of its own, for §50.2's reason:
    // "what it is" and "whose it is" are two answers to one question and an operator reads them
    // together. `null` when there is none, which is a robot whose model is its own claim (§50.3).
    out.set(
        "birth",
        match crate::birth::evaluate(&layout, &trust) {
            Ok(Some((held, attested))) => crate::birth::to_json(&held, &attested),
            _ => Json::Null,
        },
    );
    out
}

fn ota<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Json {
    let slots = context.daemon.slots.read();
    let wanted = context.daemon.desired.os_version.clone();

    let mut out = Json::obj();
    out.set("active", Json::from(slots.active.as_str()));
    out.set(
        "staged",
        match &slots.staged {
            Some(version) => Json::from(version.as_str()),
            None => Json::Null,
        },
    );
    out.set(
        "channel",
        Json::from(context.daemon.desired.os_channel.as_str()),
    );
    out.set(
        "board",
        match context.daemon.board() {
            Some(board) => Json::Str(board),
            None => Json::Null,
        },
    );
    out.set(
        "wanted",
        if wanted.is_empty() {
            Json::Null
        } else {
            Json::from(wanted.as_str())
        },
    );
    out.set(
        "assessment",
        if wanted.is_empty() {
            Json::Null
        } else {
            context.daemon.assess_image(&wanted).to_json()
        },
    );

    // The pending-boot marker, read rather than decided: this route must not be able to confirm or
    // roll back a slot. §39.1 gives an observer read access, and a GET that told a bootloader
    // anything would be a state change behind a verb that promises not to be one.
    out.set(
        "pending",
        match crate::ota::read_pending(context.daemon.store.layout()) {
            Some(pending) => {
                let mut entry = Json::obj();
                entry.set("version", Json::Str(pending.version));
                entry.set("budget_ms", Json::Num(pending.budget_ms));
                entry
            }
            None => Json::Null,
        },
    );
    // §34.3: *"the wait is visible rather than silent"*. Every field is computed from the disk and
    // the bearer table on this read — the partial file's own length is the progress — so this can
    // never report a transfer whose figures the bytes disagree with.
    out.set(
        "transfer",
        match context
            .daemon
            .assess_image_digest(&wanted)
            .filter(|digest| !digest.trim().is_empty())
        {
            Some(digest) => crate::bundle::to_json(
                context.daemon.store.layout(),
                &digest,
                context.daemon.declared_size(&wanted),
                &crate::connectivity::Table::of(context.daemon.store.layout()),
            ),
            None => Json::Null,
        },
    );
    out.set(
        "note",
        Json::from(
            "§12's third rule is what `assessment` reports: an update that changes the Safety \
             Kernel or the envelope is refused without a bench pass for these exact bytes — and \
             it is applied before a byte moves, so a robot never pays for a transfer to reach a \
             refusal it could have reached for nothing. `transfer` is §34.3's half: an OS image is \
             the largest bulk transfer this system has, so it waits for an unmetered link and \
             moves in bounded slices that resume.",
        ),
    );
    out
}

/// §54.1's `execute({ skill, arguments, timeout_s })`, arriving over §32.2's local API.
///
/// Nothing runs in this handler. The invocation is created in `idle`, and the reconcile loop moves
/// it — so an operator cannot get a skill past §54.2 by calling this and reading the answer, and a
/// skill invoked from a console walks the same ladder as one invoked by anything else.
///
/// The response is therefore `202`, not `201`: the robot has accepted the invocation and has not
/// yet decided whether it may start. A `201` would say a skill was created and running, which is
/// the one thing §54.4 spends its length on not being true yet.
fn execute_skill<L: Launcher, S: Slots>(
    context: &mut Context<'_, L, S>,
    request: &Request,
) -> Response {
    let Ok(text) = std::str::from_utf8(&request.body) else {
        return error(400, "the body is not UTF-8");
    };
    let Ok(body) = Json::parse(text) else {
        return error(400, "the body is not JSON");
    };
    let Some(package) = body.get("skill").and_then(Json::as_str) else {
        return error(400, "no `skill`: name the package to invoke");
    };
    // `namespace/name`, as the desired state and every manifest spell it. Not the `namespace:name`
    // a *route segment* has to use — this arrives in a JSON body, where a slash is an ordinary
    // character, and asking a caller to escape one here would be a second spelling of an id for no
    // reason.
    let Ok(id) = PackageId::parse(package) else {
        return error(400, "that is not a package id");
    };
    let Some(digest) = context.daemon.store.active_digest(&id) else {
        return error(
            404,
            &format!("`{package}` has no activated version, so there is nothing to invoke"),
        );
    };
    let manifest = match context.daemon.store.manifest(&digest) {
        Ok(manifest) => manifest,
        Err(failure) => return error(500, &failure.to_string()),
    };

    // §54.2's third rule: the operator may add preconditions and the skill may not remove them.
    // Read from the profile in force, which is the signed document (§38.1).
    let profile = crate::observe::read_json(&context.daemon.store.layout().active_profile());
    let operator = crate::skill::operator_clauses(profile.as_ref(), package);

    let invocation = crate::skill::Request {
        package: package.to_string(),
        arguments: body.get("arguments").cloned().unwrap_or_else(Json::obj),
        parent: body
            .get("parent")
            .and_then(Json::as_str)
            .map(str::to_string),
        overrun_factor: body.get_present("overrun_factor").and_then(Json::as_f64),
    };

    match context
        .daemon
        .skills
        .execute(&invocation, &manifest, &digest, &operator, context.now_ms)
    {
        Err(refusal) => {
            let mut payload = Json::obj();
            payload.set("code", Json::Str(refusal.code.to_string()));
            payload.set("detail", Json::Str(refusal.detail));
            Response::json(409, encode(&payload))
        }
        Ok(accepted) => {
            let mut payload = Json::obj();
            payload.set("invocation", Json::Str(accepted.clone()));
            payload.set("skill", Json::Str(package.to_string()));
            payload.set("phase", Json::Str("idle".to_string()));
            payload.set(
                "note",
                Json::Str(
                    "Accepted, not started. §54.2 is evaluated on the next reconcile pass and may \
                     refuse it; watch GET /skills/<id>."
                        .to_string(),
                ),
            );
            Response::json(202, encode(&payload))
        }
    }
}

/// Stop a running skill's unit.
///
/// **This makes the skill return; it does not abort it**, and the difference is deliberate. §54.4
/// names four things that end a running skill — a timeout, a guard trip, a lost `hold` precondition
/// and a robot that stopped commanding — and an operator stopping one skill is none of them. Adding
/// a fifth reason would put a word in a closed vocabulary that no corpus case came out of the
/// document, so instead the process is stopped and the lifecycle sees what it would see for any
/// skill whose code returned: `running → verifying`, and §54.3 judges it over post-state.
///
/// That is also the more honest answer. A skill stopped halfway has not satisfied its success
/// criteria, so it is recorded `failure` or `unlabeled` on the evidence rather than on the fact that
/// somebody pressed a button.
fn stop_skill<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>, id: &str) -> Response {
    let Some(invocation) = context.daemon.skills.get(id) else {
        return error(404, "no such invocation");
    };
    if invocation.terminal() {
        return error(
            409,
            &format!("{id} is already over; its leases are released and its episode is emitted"),
        );
    }
    let package = invocation.package.clone();
    match context.daemon.launcher.stop(&package) {
        Ok(()) => {
            let mut payload = Json::obj();
            payload.set("invocation", Json::Str(id.to_string()));
            payload.set("skill", Json::Str(package));
            payload.set(
                "note",
                Json::Str(
                    "Stopped. The next pass reads this as a skill that returned, so §54.3 judges \
                     it over post-state rather than recording it aborted — a skill stopped halfway \
                     has not met its criteria, and that is the evidence rather than the button."
                        .to_string(),
                ),
            );
            ok(payload)
        }
        Err(failure) => error(500, &failure.to_string()),
    }
}

fn blackbox_index<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Response {
    let Some(recorder) = context.blackbox.as_ref() else {
        return error(503, UNAVAILABLE);
    };
    // The rate the window is reported against. There is no control loop on this robot, so the
    // honest denominator is the reconcile period rather than a joint rate nothing is producing.
    ok(recorder.index(1.0))
}

/// `POST /blackbox/seal` — seal the window now (§39.1: operator).
///
/// §13 seals on a fault, an estop, or an operator request, and this is the third. The seal is
/// itself recorded as an event first, so the file it produces contains the reason it was produced.
fn blackbox_seal<L: Launcher, S: Slots>(context: &mut Context<'_, L, S>) -> Response {
    let mono_ns = context.mono_ns;
    let Some(recorder) = context.blackbox.as_mut() else {
        return error(503, UNAVAILABLE);
    };
    if let Err(failure) = recorder.event(
        mono_ns,
        skynet_blackbox::record::EventCode::Sealed,
        "operator",
        "sealed through the local API",
    ) {
        return error(500, &format!("recording the seal: {failure}"));
    }
    match recorder.seal(skynet_blackbox::seal::Reason::Operator, mono_ns) {
        Err(failure) => error(500, &format!("sealing: {failure}")),
        Ok((sealed, path)) => {
            let mut body = Json::obj();
            body.set(
                "name",
                Json::Str(
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                ),
            );
            body.set("bytes", Json::Num(sealed.bytes.len() as f64));
            body.set(
                "digest",
                Json::Str(skynet_blackbox::mcap::hex(&sealed.digest)),
            );
            body.set("reason", Json::Str(sealed.reason.as_str().into()));
            body.set("records", Json::Num(sealed.continuity.records as f64));
            body.set("cycles", Json::Num(sealed.continuity.cycles as f64));
            body.set("complete", Json::Bool(sealed.continuity.complete()));
            Response::json(201, encode(&body))
        }
    }
}

/// `GET /blackbox/{name}` — one sealed recording (§39.1: operator).
///
/// The bytes, as MCAP. The name is validated inside the recorder rather than here, because a route
/// that joined a caller's path segment onto a directory is how a download becomes a way to read
/// anything on the disk.
fn blackbox_download<L: Launcher, S: Slots>(
    context: &mut Context<'_, L, S>,
    name: &str,
) -> Response {
    let Some(recorder) = context.blackbox.as_ref() else {
        return error(503, UNAVAILABLE);
    };
    match recorder.read_sealed(name) {
        None => error(404, "no such sealed recording"),
        Some(bytes) => Response {
            status: 200,
            content_type: "application/octet-stream".to_string(),
            body: bytes,
            extra: Vec::new(),
        },
    }
}

const UNAVAILABLE: &str = "this robot has no black box: the ring could not be opened. \
                           §28 preallocates it so a full disk cannot silence the recorder, which \
                           does not help if it was full before the ring existed.";

/// Parse a `{ns}:{name}` route segment into a package id.
///
/// The `:` rather than a `/`, because a package id contains a slash and a route with one inside a
/// path segment is a route two implementations will parse differently.
pub fn parse_route_id(raw: &str) -> Option<PackageId> {
    let (namespace, name) = raw.split_once(':')?;
    PackageId::parse(&format!("{namespace}/{name}")).ok()
}

/// The name an action carries in a report.
pub fn action_name(kind: &skynetd::reconcile::ActionKind) -> &'static str {
    use skynetd::reconcile::ActionKind as K;
    match kind {
        K::StageOsImage { .. } => "stage_os_image",
        K::ApplyOsImage { .. } => "apply_os_image",
        K::FetchPackage { .. } => "fetch_package",
        K::ActivatePackage { .. } => "activate_package",
        K::RemovePackage { .. } => "remove_package",
        K::ApplyProfile { .. } => "apply_profile",
        K::GrantCapability { .. } => "grant_capability",
        K::RevokeCapability { .. } => "revoke_capability",
        K::StartPackage { .. } => "start_package",
        K::StopPackage { .. } => "stop_package",
        K::ReleaseLease { .. } => "release_lease",
    }
}

fn nullable(value: Option<f64>) -> Json {
    value.map_or(Json::Null, Json::Num)
}

fn ok(body: Json) -> Response {
    Response::json(200, encode(&body))
}

/// An error, in the one shape every route uses.
pub fn error(status: u16, detail: &str) -> Response {
    let mut body = Json::obj();
    body.set("error", Json::from(crate::http::reason(status)))
        .set("detail", Json::from(detail));
    Response::json(status, encode(&body))
}

fn encode(body: &Json) -> Vec<u8> {
    body.to_canonical()
        .unwrap_or_else(|error| {
            // A value that cannot be canonicalised is a non-finite number somewhere in a report, and
            // emitting `null` for it would read as "no limit". Saying so is the only honest answer.
            format!(
                "{{\"error\":\"Internal Server Error\",\"detail\":\"a value in this report \
                     cannot be serialised: {error:?}\"}}"
            )
        })
        .into_bytes()
}

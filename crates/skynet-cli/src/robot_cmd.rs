//! The commands that talk to a running robot (§18, §33.7).
//!
//! ```text
//! skynet status                      what this robot is, and whether stop is reachable
//! skynet ps                          installed packages, their state and their grants
//! skynet logs <ns/name>              the package's journal — not the black box (§41.1)
//! skynet plan                        the last reconcile pass, including anything deferred
//! skynet cell                        the cell descriptor in force, and its faults (§55.2)
//! skynet grants                      what is granted, and what is leased
//! skynet grants add <ns/name> <token> [--acknowledge <digest>]
//! skynet grants remove <ns/name> <token>
//! skynet estop                       stop every package, now
//! skynet install <dir> --id … --kind … --version … --digest …
//! skynet rollback <ns/name>          the previous version, in milliseconds
//! skynet skills                      every invocation, and where §54.4 says it is
//! skynet skill run <ns/name>         invoke one (§54.1)
//! skynet skill stop <inv-id>         make a running one return
//! skynet ota                         the slots, and whether the next image may be applied
//! ```
//!
//! These were absent until `skynetd` existed, and the reason they were absent rather than stubbed is
//! the reason this file is careful about one thing in particular: **it prints what the robot said**.
//! Where the daemon reports a field as unmeasured, this prints "unmeasured". Where a package is
//! running and nobody has measured whether it is producing commands, this prints `unknown` and not
//! a tick. A CLI that tidied those into something reassuring would undo the work §33.6 does.
//!
//! # Two transports, one client
//!
//! `--socket` (the default, `/run/skynetd.sock`) and `--host addr:port`. §33.7 gives both the same
//! REST shapes, so the difference is four lines at the bottom of this file and nothing above it.
//!
//! # `estop`, and what it actually does
//!
//! It stops every package. `skynetd` never touches an actuator (§33.1), so what brings the robot to
//! rest is the kernel: no fresh commands arrive, the watchdog observes it, and the robot holds. That
//! is a real stop and it is not an emergency stop of the drive-enable line, which is the safety
//! MCU's job (§46) and is not built. The command says so rather than letting the name imply it.

use std::io::{Read, Write};

use skynet_contracts::admission::SignatureState;
use skynet_json::Json;

use crate::term;

/// `skynet <command>` for the commands that need a robot.
pub const USAGE: &str = "\
skynet — talking to a running robot (§33.7)

  status                      What this robot is; needs no credential (§39.1)
  state                       §27's machine: the state, the reason, and which of its
                              eight conditions anybody actually observed
  ps                          Installed packages, their state, their grants
  logs <ns/name> [--lines N]  The package's journal
  plan                        The last reconcile pass, and anything deferred
  cell                        The cell descriptor, its env.* facts and its faults
  power                       §36: the state, the reserve, and what is being given up
  limits [--package <ns/name>]
                              §7.3: the ceilings the kernel would enforce, and which of
                              the eight guards can run on this body
  conformance [--results <file>]
                              §15's eight runtime tests, run by this robot on itself.
                              --results also writes them as a results document, for
                              `skynet bench award` beside `skynet driver check --results`
  readiness [files...] [--fleet]
                              What this robot could attempt, from its declarations
                              alone — or the same for descriptor files here. --fleet
                              prints the summary: by_tier, common_blockers
  grants                      What is granted, and what is leased
  grants add <ns/name> <token> [--acknowledge <digest>]
                              A skill's safety notes are shown once and acknowledged (§54.2)
  grants remove <ns/name> <token>
  estop                       Stop every package; needs no credential (§39.1)
  state clear                 Request named SAFE clearance; needs operator (§27).
                              Read `state` for the next interval's result; no hardware reset.
  rollback <ns/name>          Back to the previous version (§37.2)
  install <dir> --id <ns/name> --kind <k> --version <v> --digest <hex>
  blackbox                    The recording's window, what is missing from it, and which
                              sealed windows the Hub has (§79)
  blackbox seal               Seal the window now (§13); needs operator
  blackbox get <name>         Download one sealed recording, as MCAP
  skills                      Every invocation, and where §54.4 says it is
  skill run <ns/name> [--args <json>] [--parent <inv-id>] [--overrun <factor>]
                              Invoke a skill; needs operator
  skill stop <inv-id>         Make a running skill return; needs operator
  skill check <dir>           Hold a skill package against §54. Talks to no robot; run it
                              before publishing, and `skynet skill check --help` for it
  calibration                 §45: which parameter sets are believed, and which guards lost
                              their input because one is not
  calibration event <event> [--note WHY]
                              Record an impact, a tool change or a swap (§45.4). It expires
                              every calibration whose policy names it; re-measuring clears it
  session                     §38.2's session override: what is narrowing this robot now
  session set --for <90s|20m|2h> [--velocity R] [--torque N] [--base M] [--jerk F]
              [--name NAME] [--reason WHY]
                              Slow this robot down while somebody works on it. It can only
                              narrow, and `--for` is required — §38.2 time-boxes this layer
  session clear               Put it back to the ceilings the profile sets
  settings                    §38.1's unsigned local configuration, and what set each value
  settings set <setting> <value>
                              log_level error|warn|info|debug · telemetry_sampling 1..60 ·
                              console_theme system|light|dark. Nothing that reaches the
                              Safety Kernel lives here (§38.1)
  settings reset <setting>    Back to its default
  ota                         A/B slots, and §12's verdict on the wanted image
  identity                    Who this robot is, what it is, and what it is running (§10)
  checkin                     What this robot would tell the Hub, and whether it is heard
  intent                      Where the desired state came from, and what the last pull did
  releases                    Whether the wanted OS image has a manifest this robot can judge
  pair <code|-> --hub <url>   Bind this robot to a namespace; needs admin (§10)
  unpair                      Remove the credential and the identity; needs admin
  shutdown                    Stop the supervisor: withdraw its mDNS name (§82.4), seal the
                              black box window (§13), exit. Needs admin. This does **not**
                              stop the robot — `estop` is the route that reaches the body.

  --socket <path>   Default /run/skynetd.sock
  --host <addr:port>  The LAN console instead of the socket
  --token <t>       Or $SKYNET_TOKEN. Not needed for `status` or `estop`.
  --root <dir>      For `install`: where /data lives. Default `/`.
  --limit <n>       For `blackbox events`: how many. Default 50, ceiling 500.

  `blackbox inspect <file>` talks to no robot: it opens a sealed recording on this machine
  and prints its provenance, whether it is complete, and the events in it.";

/// Where to send a request.
pub(crate) enum Target {
    /// The local socket.
    Socket(std::path::PathBuf),
    /// The LAN console.
    Host(String),
}

pub(crate) fn target(argv: &[String]) -> Target {
    if let Some(host) = flag(argv, "--host") {
        return Target::Host(host);
    }
    Target::Socket(std::path::PathBuf::from(
        flag(argv, "--socket").unwrap_or_else(|| "/run/skynetd.sock".to_string()),
    ))
}

/// Every flag in this module that takes a value.
///
/// `positional` has to know this to do its job, and until `DRY-COVERAGE-PLAN.md` F62 it did not:
/// it assumed **every** `--flag` took the argument after it, so `skynet readiness --fleet a.json`
/// read `a.json` as the flag's value and summarized a fleet of one. That command stripped its own
/// boolean before the scan, which fixed the command and left the trap set for the next boolean
/// anybody adds. The table is the general fix: a flag listed here consumes the argument after it,
/// and a flag that is not listed consumes nothing.
///
/// It is the one place the two halves can disagree, so [`flag`] asserts against it in debug builds
/// and `every_value_flag_this_module_reads_is_in_the_table` scans this file's own source. A value
/// flag added without a line here would otherwise leave its value among the positional arguments,
/// which is where a package id is expected.
const VALUE_FLAGS: &[&str] = &[
    "--acknowledge",
    "--args",
    "--base",
    "--digest",
    "--for",
    "--host",
    "--hub",
    "--id",
    "--jerk",
    "--kind",
    "--limit",
    "--limits",
    "--lines",
    "--name",
    "--note",
    "--overrun",
    "--package",
    "--parent",
    "--reason",
    "--results",
    "--root",
    "--signature",
    "--signed-by",
    "--socket",
    "--token",
    "--torque",
    "--velocity",
    "--version",
];

/// Whether `arg` is a flag that takes the argument after it (F62).
fn takes_a_value(arg: &str) -> bool {
    VALUE_FLAGS.contains(&arg)
}

/// `session set`'s narrowings: the flag an operator writes, and the ceiling it sets.
///
/// Module-level rather than inline in the command for F62's reason — these four names reach
/// [`flag`] through a variable, so the source scan that holds every literal flag name to
/// [`VALUE_FLAGS`] cannot see them, and `the_session_narrowings_are_value_flags` checks them
/// against the same table by value.
const SESSION_NARROWINGS: &[(&str, &str)] = &[
    ("--velocity", "joint_velocity_max_rad_s"),
    ("--torque", "joint_torque_max_nm"),
    ("--base", "base_velocity_max_m_s"),
    ("--jerk", "max_delta_per_cycle"),
];

fn flag(argv: &[String], name: &str) -> Option<String> {
    debug_assert!(
        takes_a_value(name),
        "`{name}` is read as a value flag and is not in VALUE_FLAGS, so `positional` would leave its value among the positional arguments (F62)"
    );
    let mut index = 0;
    while index < argv.len() {
        if argv[index] == name {
            return argv.get(index + 1).cloned();
        }
        if let Some(rest) = argv[index].strip_prefix(&format!("{name}=")) {
            return Some(rest.to_string());
        }
        index += 1;
    }
    None
}

fn positional(argv: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut index = 0;
    while index < argv.len() {
        let arg = &argv[index];
        if arg.starts_with("--") {
            // `--flag=value` consumes one argument; `--flag value` consumes two, and only for a
            // flag that takes a value. A boolean consumes itself and nothing else (F62).
            if !arg.contains('=') && takes_a_value(arg) {
                index += 1;
            }
        } else {
            out.push(arg);
        }
        index += 1;
    }
    out
}

fn token(argv: &[String]) -> Option<String> {
    flag(argv, "--token").or_else(|| std::env::var("SKYNET_TOKEN").ok())
}

/// Dispatch one robot command.
pub fn run(command: &str, argv: &[String]) -> i32 {
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return 0;
    }

    match command {
        "status" => show(argv, "GET", "/status", None, print_status),
        "ps" => show(argv, "GET", "/packages", None, print_packages),
        "plan" => show(argv, "GET", "/plan", None, print_plan),
        "estop" => show(argv, "POST", "/stop", Some(String::new()), print_stop),
        "logs" => logs(argv),
        "grants" => grants(argv),
        "session" => session(argv),
        "settings" => settings(argv),
        "calibration" => calibration(argv),
        "rollback" => rollback(argv),
        "install" => install(argv),
        "blackbox" => blackbox(argv),
        "ota" => show(argv, "GET", "/ota", None, print_ota),
        "identity" => show(argv, "GET", "/identity", None, print_identity),
        "checkin" => show(argv, "GET", "/checkin", None, print_checkin),
        "intent" => show(argv, "GET", "/intent", None, print_intent),
        "releases" => show(argv, "GET", "/releases", None, print_releases),
        "cell" => show(argv, "GET", "/cell", None, print_cell),
        "power" => show(argv, "GET", "/power", None, print_power),
        "state" if argv.first().map(String::as_str) == Some("clear") => show(
            &argv[1..],
            "POST",
            "/state/clear",
            Some(String::new()),
            print_note,
        ),
        "state" => show(argv, "GET", "/state", None, print_state),
        "shutdown" => show(argv, "POST", "/shutdown", Some(String::new()), print_note),
        "limits" => limits(argv),
        "conformance" => conformance(argv),
        "readiness" => readiness(argv),
        "pair" => pair(argv),
        "unpair" => show(
            argv,
            "POST",
            "/identity/unpair",
            Some(String::new()),
            print_note,
        ),
        "skills" => show(argv, "GET", "/skills", None, print_skills),
        "skill" => skill(argv),
        other => term::fail(&format!("`{other}` is not a robot command.\n\n{USAGE}")),
    }
}

/// `skynet skill run|stop` (§54.1).
///
/// Two subcommands rather than one because they are two different acts on two different nouns:
/// `run` names a *package*, `stop` names an *invocation*. Collapsing them would mean guessing which
/// from the shape of the argument, and a robot is the wrong place to guess.
fn skill(argv: &[String]) -> i32 {
    match argv.first().map(String::as_str) {
        Some("run") => skill_run(&argv[1..]),
        Some("stop") => match argv.get(1) {
            Some(id) if !id.starts_with('-') => show(
                &argv[2..],
                "POST",
                &format!("/skills/{id}/stop"),
                Some(String::new()),
                print_note,
            ),
            _ => term::fail("skynet skill stop <inv-id> — name the invocation, not the package"),
        },
        _ => term::fail(&format!(
            "skynet skill check <dir>, skynet skill run <ns/name>, or skynet skill stop \
             <inv-id>.\n\n{USAGE}"
        )),
    }
}

fn skill_run(argv: &[String]) -> i32 {
    let Some(package) = argv.first().filter(|arg| !arg.starts_with('-')) else {
        return term::fail("skynet skill run <ns/name> — name the skill package to invoke");
    };

    let mut body = Json::obj();
    body.set("skill", Json::from(package.as_str()));

    // `--args` is JSON on the command line, and a malformed one is refused here rather than sent.
    // The robot would refuse it too, but a parse error from a local argument is a parse error the
    // person typing it can fix, and sending it first would make the robot answer for the typo.
    if let Some(text) = flag(argv, "--args") {
        match Json::parse(&text) {
            Ok(parsed) => {
                body.set("arguments", parsed);
            }
            Err(error) => {
                return term::fail(&format!("--args is not JSON: {error:?}"));
            }
        }
    }
    if let Some(parent) = flag(argv, "--parent") {
        body.set("parent", Json::from(parent.as_str()));
    }
    if let Some(text) = flag(argv, "--overrun") {
        match text.parse::<f64>() {
            Ok(factor) => {
                body.set("overrun_factor", Json::Num(factor));
            }
            Err(_) => return term::fail("--overrun takes a number, e.g. --overrun 3"),
        }
    }

    let Ok(text) = body.to_canonical() else {
        return term::fail("the invocation could not be serialised");
    };
    show(argv, "POST", "/skills", Some(text), print_note)
}

/// The `note` a robot sent back with an acceptance, or the whole document when there is none.
/// `skynet pair <code|-> --hub <url>` (§10, §18).
///
/// # The code may be read from stdin, and there is a reason to
///
/// §18 writes the command as `skynet pair ABCD-1234`, so the code is an argument, so it is in the
/// shell history and in `/proc/<pid>/cmdline` for the length of the call. That is the
/// specification's own interface and this is not the place to overrule it — but `-` reads the code
/// from stdin instead, which is one pipe away from a code that never touches either. The daemon
/// side is careful about this whichever way the code arrives (see `identity.rs`); this is the half
/// an operator controls.
fn pair(argv: &[String]) -> i32 {
    let Some(raw) = argv.first().filter(|value| !value.starts_with('-')) else {
        return term::fail(
            "skynet pair <code> --hub <url>   — the code the Hub showed you, or `-` to read it \
             from stdin",
        );
    };
    let rest = &argv[1..];

    let Some(hub) = flag(rest, "--hub") else {
        return term::fail(
            "skynet pair needs --hub <url>. There is no default: §39.2 gives this robot no default \
             password and no default Hub either, and a robot that would report to a host nobody \
             named is a robot somebody else can adopt.",
        );
    };

    let code = if raw == "-" {
        let mut typed = String::new();
        if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut typed) {
            return term::fail(&format!("the code could not be read from stdin: {error}"));
        }
        typed
    } else {
        raw.clone()
    };

    // Refused here as well as on the robot, and the duplication is deliberate: this is the one
    // place that can refuse a mistyped code without it having crossed a socket, and §10's whole
    // argument for a local check is that a code is a bearer secret with a short life.
    let checked = skynet_contracts::identity::normalize_pairing_code(&code);
    let Some(normalized) = checked.code else {
        let detail = checked
            .fault
            .map(|fault| fault.detail)
            .unwrap_or_else(|| "that is not a pairing code".to_string());
        return term::fail(&detail);
    };

    let mut body = Json::obj();
    body.set("code", Json::from(normalized.as_str()));
    body.set("endpoint", Json::from(hub.as_str()));
    let Ok(encoded) = body.to_canonical() else {
        return term::fail("the request could not be serialised");
    };

    show(rest, "POST", "/identity/pair", Some(encoded), print_paired)
}

fn print_paired(document: &Json) {
    let text = |key: &str| {
        document
            .get_present(key)
            .and_then(Json::as_str)
            .unwrap_or("—")
            .to_string()
    };
    println!(
        "paired as {} in `{}`, reporting to {}",
        text("device_id"),
        text("namespace"),
        text("endpoint"),
    );
    print_identity_findings(document);
}

/// §11 and §33.8: where the desired state came from, and what the last pull made of it.
///
/// The rung comes first, because it is the half that is true whether or not a Hub was ever reached
/// — and a robot reconciling against a last-known-good copy looks identical from every other
/// surface to one reconciling against a current desired state.
/// `skynet releases` — what happened the last time this robot asked for the release catalogue.
///
/// Three states, and the middle one is why this command exists: **asked and got nothing back for
/// this version**. A robot whose desired state names an OS image nobody has published for its
/// architecture is indistinguishable, from every other surface, from one that is simply up to date
/// — it reconciles, it converges, and the OS stage never closes because there is nothing to close
/// it with.
fn print_releases(document: &Json) {
    let version = document
        .get("version")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let channel = document
        .get("channel")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let arch = document
        .get("arch")
        .and_then(Json::as_str)
        .unwrap_or_default();

    if version.is_empty() {
        println!(
            "{}",
            term::dim(
                "this robot's desired state names no OS version, so there is no release to \
                 ask about."
            )
        );
        return;
    }
    println!("{}", term::dim(&format!("{version} on {channel}, {arch}")));

    if !document
        .get("asked")
        .and_then(Json::as_bool)
        .unwrap_or(false)
    {
        println!();
        println!(
            "{}",
            term::dim(
                document
                    .get_present("note")
                    .and_then(Json::as_str)
                    .unwrap_or("no pull has been attempted.")
            )
        );
        return;
    }

    println!();
    if !document
        .get("reached")
        .and_then(Json::as_bool)
        .unwrap_or(false)
    {
        println!(
            "{}",
            term::warn(&format!(
                "the Hub was not reached: {}",
                document
                    .get_present("detail")
                    .and_then(Json::as_str)
                    .unwrap_or("no detail")
            ))
        );
        return;
    }

    let fault = document
        .get_present("fault")
        .and_then(Json::as_str)
        .unwrap_or_default();
    if !fault.is_empty() {
        println!("{}", term::bad(fault));
        return;
    }

    if document
        .get("written")
        .and_then(Json::as_bool)
        .unwrap_or(false)
    {
        println!("{}", term::ok("the manifest for this release is now held."));
    } else {
        println!(
            "{}",
            term::ok("this robot already holds the manifest for this release.")
        );
    }
}

fn print_intent(document: &Json) {
    let source = document
        .get("source")
        .and_then(Json::as_str)
        .unwrap_or("unreported");
    let line = format!("desired state: {source}");
    match source {
        "current" => println!("{}", term::ok(&line)),
        "last_known_good" => println!("{}", term::warn(&line)),
        _ => println!("{}", term::bad(&line)),
    }
    if let Some(why) = document
        .get_present("source_because")
        .and_then(Json::as_str)
    {
        println!("{}", term::dim(why));
    }

    if let Some(note) = document.get_present("note").and_then(Json::as_str) {
        println!();
        println!("{}", term::dim(note));
        return;
    }

    let Some(pull) = document
        .get_present("pull")
        .filter(|node| **node != Json::Null)
    else {
        println!();
        println!("{}", term::dim("no pull has been attempted."));
        return;
    };

    println!();
    if !pull.get("reached").and_then(Json::as_bool).unwrap_or(false) {
        println!(
            "{}",
            term::warn(&format!(
                "the Hub was not reached: {}",
                pull.get_present("detail")
                    .and_then(Json::as_str)
                    .unwrap_or("no detail")
            ))
        );
        // §11's point, said rather than left to be inferred: this costs the robot nothing it is not
        // already paying for most of its life.
        println!(
            "{}",
            term::dim(
                "Nothing changed. No control decision requires a network round trip (§11), so this \
                 robot reconciles against what it has."
            )
        );
        return;
    }

    let Some(verdict) = pull
        .get_present("verdict")
        .filter(|node| **node != Json::Null)
    else {
        return;
    };
    let adopt = verdict
        .get("adopt")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let changed = verdict
        .get("changed")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let generation = verdict
        .get("generation")
        .and_then(Json::as_f64)
        .unwrap_or(0.0);
    let headline = if !adopt {
        term::bad(&format!("refused; still on generation {generation}"))
    } else if changed {
        term::ok(&format!("adopted generation {generation}"))
    } else {
        term::dim(&format!("generation {generation}, unchanged"))
    };
    println!("{headline}");

    for fault in verdict.arr_or_empty("faults") {
        let message = fault.get("message").and_then(Json::as_str).unwrap_or("");
        match fault.get("severity").and_then(Json::as_str) {
            Some("error") => println!("{}", term::bad(message)),
            _ => println!("{}", term::warn(message)),
        }
    }
}

/// §11 and §17's check-in: what this robot would say, and whether anybody is hearing it.
///
/// Two questions rather than one, printed apart. A check-in that says exactly the right thing to a
/// Hub that never receives it is the state a fleet console cannot distinguish from a healthy robot,
/// and it is the state most robots running this edition are in.
fn print_checkin(document: &Json) {
    let text = |key: &str| {
        document
            .get_present(key)
            .and_then(Json::as_str)
            .unwrap_or("—")
            .to_string()
    };
    let applied = document
        .get("applied")
        .and_then(Json::as_bool)
        .unwrap_or(false);

    println!(
        "generation {}   {}   state {}",
        document
            .get("generation")
            .and_then(Json::as_f64)
            .unwrap_or(0.0),
        if applied {
            term::ok(&text("claim"))
        } else {
            term::warn(&text("claim"))
        },
        text("state"),
    );
    println!("{}", term::dim(&text("detail")));

    if let Some(sentence) = document
        .get("attestation")
        .and_then(|node| node.get("sentence"))
        .and_then(Json::as_str)
    {
        println!();
        println!("{sentence}");
    }

    // Health, and the two ways a field can be missing. §33.6 keeps them apart because "nobody
    // measured this" and "somebody measured this and lost track of when" send different people to
    // different places.
    if let Some(health) = document.get("health") {
        println!();
        let degraded = health
            .get("degraded")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let stalled = health.arr_or_empty("stalled");
        if degraded {
            println!(
                "{}",
                term::bad(&format!(
                    "degraded — alive and producing nothing: {}",
                    stalled
                        .iter()
                        .filter_map(Json::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                ))
            );
        }
        // What was measured, before what was not. The first version of this printed only the
        // absences, which is a health report that can never say a robot is well.
        if let Some(fields) = health.get("fields").and_then(Json::as_obj) {
            for (name, held) in fields {
                let (Some(value), Some(age)) = (
                    held.get("value").and_then(Json::as_f64),
                    held.get("age_ms").and_then(Json::as_f64),
                ) else {
                    continue;
                };
                println!("{name} {value} ({age} ms old)");
            }
        }

        let names = |key: &str| {
            health
                .arr_or_empty(key)
                .iter()
                .filter_map(Json::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        };
        let unmeasured = names("unmeasured");
        if !unmeasured.is_empty() {
            println!("{}", term::dim(&format!("nobody measured: {unmeasured}")));
        }
        let undated = names("undated");
        if !undated.is_empty() {
            println!(
                "{}",
                term::warn(&format!(
                    "measured and undateable, so not reported: {undated} (§33.6)"
                ))
            );
        }
    }

    let blocking = document.arr_or_empty("blocking");
    if !blocking.is_empty() {
        println!();
        for entry in document.arr_or_empty("pending") {
            let stage = entry.get("stage").and_then(Json::as_str).unwrap_or("?");
            let reason = entry.get("reason").and_then(Json::as_str).unwrap_or("");
            let deferred = entry
                .get("deferred")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            let line = format!("{stage}: {reason}");
            if deferred {
                println!("{}", term::warn(&line));
            } else {
                println!("{}", term::dim(&line));
            }
        }
    }

    println!();
    let queued = document
        .get("queued")
        .and_then(Json::as_bool)
        .unwrap_or(false);
    let note = text("queued_note");
    if queued {
        println!("{}", term::dim(&note));
    } else {
        println!("{}", term::warn(&note));
    }
}

/// §10, on one screen: whose this robot is, what it is, and what it is running.
fn print_identity(document: &Json) {
    let text = |key: &str| {
        document
            .get_present(key)
            .and_then(Json::as_str)
            .unwrap_or("—")
            .to_string()
    };
    let flag_of = |key: &str| document.get(key).and_then(Json::as_bool).unwrap_or(false);

    println!(
        "paired {}   attested {}   namespace {}   device {}",
        if flag_of("paired") { "yes" } else { "no" },
        if flag_of("attested") { "yes" } else { "no" },
        text("namespace"),
        text("device_id"),
    );
    println!("{}", term::dim(&format!("reports to {}", text("endpoint"))));

    if let Some(fault) = document.get_present("fault").and_then(Json::as_str) {
        println!("{}", term::warn(fault));
    }

    // §10's sentence, printed whole. It is the one an incident investigation reads, and printing
    // the three digests as a table instead would be this command deciding it knows better than the
    // section that asked for a sentence.
    if let Some(attestation) = document.get_present("attestation") {
        if let Some(sentence) = attestation.get("sentence").and_then(Json::as_str) {
            println!();
            println!("{sentence}");
        }
    }

    print_identity_findings(document);
}

fn print_identity_findings(document: &Json) {
    let findings = document.arr_or_empty("findings");
    if findings.is_empty() {
        return;
    }
    println!();
    for finding in findings {
        let message = finding
            .get("message")
            .and_then(Json::as_str)
            .unwrap_or_default();
        let code = finding.get("code").and_then(Json::as_str).unwrap_or("?");
        let line = format!("{code}: {message}");
        match finding.get("severity").and_then(Json::as_str) {
            Some("error") => println!("{}", term::bad(&line)),
            _ => println!("{}", term::warn(&line)),
        }
    }
}

fn print_note(document: &Json) {
    if let Some(invocation) = document.get_present("invocation").and_then(Json::as_str) {
        println!("{invocation}");
    }
    if let Some(note) = document.get_present("note").and_then(Json::as_str) {
        println!("{}", term::dim(note));
    }
}

/// §12's state: where the robot is, and whether it may move.
///
/// The verdict is printed with **every** error finding rather than the first, because an OS update
/// is the case where each attempt round the loop costs a reboot. A gated component is called out
/// separately from the refusal that names it: "this update touches the safety kernel" is what an
/// operator needs before they go and book bench time, and it is true whether or not they hold a
/// pass today.
fn print_ota(document: &Json) {
    let text = |key: &str| {
        document
            .get_present(key)
            .and_then(Json::as_str)
            .unwrap_or("—")
            .to_string()
    };
    println!(
        "active {}   staged {}   channel {}",
        text("active"),
        text("staged"),
        text("channel"),
    );

    if let Some(pending) = document
        .get_present("pending")
        .filter(|p| **p != Json::Null)
    {
        println!(
            "pending {} — this boot has to prove itself within {} ms or the bootloader returns",
            pending
                .get("version")
                .and_then(Json::as_str)
                .unwrap_or("(unknown)"),
            pending
                .get("budget_ms")
                .and_then(Json::as_f64)
                .unwrap_or(0.0),
        );
    }

    let Some(assessment) = document
        .get_present("assessment")
        .filter(|a| **a != Json::Null)
    else {
        println!(
            "{}",
            term::dim("The desired state names no OS version, so there is nothing to assess.")
        );
        return;
    };

    let admitted = assessment.get("admitted") == Some(&Json::Bool(true));
    println!(
        "wanted {}   {}",
        text("wanted"),
        if admitted { "admitted" } else { "REFUSED" }
    );
    if let Some(reason) = assessment.get_present("because").and_then(Json::as_str) {
        println!("  {}", term::dim(reason));
    }
    let Some(verdict) = assessment.get_present("verdict") else {
        return;
    };
    let gated = verdict.arr_or_empty("gated_components");
    if !gated.is_empty() {
        let names: Vec<&str> = gated.iter().filter_map(Json::as_str).collect();
        println!(
            "  {}",
            term::dim(&format!(
                "changes {} — §12 requires a bench pass for these exact bytes",
                names.join(", ")
            ))
        );
    }
    for fault in verdict.arr_or_empty("faults") {
        let severity = fault
            .get("severity")
            .and_then(Json::as_str)
            .unwrap_or("error");
        println!(
            "  {} {}",
            fault.get("code").and_then(Json::as_str).unwrap_or("?"),
            term::dim(fault.get("message").and_then(Json::as_str).unwrap_or("")),
        );
        let _ = severity;
    }
    if let Some(note) = assessment.get_present("bench_note").and_then(Json::as_str) {
        println!("  {}", term::dim(&format!("bench pass: {note}")));
    }
    print_transfer(document);
}

/// §34.3's half of `skynet ota`: where the bytes are, and what they are waiting for.
///
/// Drawn even when the assessment refuses the release, because the two answer different questions
/// and an operator watching a download needs the second one. It is deliberately explicit about
/// *whose* number the total is: a manifest that declares a size is a publisher's signed statement,
/// and a size from the server is the party serving the bytes describing its own file.
fn print_transfer(document: &Json) {
    let Some(transfer) = document
        .get_present("transfer")
        .filter(|value| **value != Json::Null)
    else {
        return;
    };
    let decision = transfer
        .get("decision")
        .and_then(Json::as_str)
        .unwrap_or("—");
    let received = transfer
        .get("received")
        .and_then(Json::as_f64)
        .unwrap_or(0.0);
    let total = transfer.get_present("total").and_then(Json::as_f64);
    let source = transfer
        .get_present("total_declared_by")
        .and_then(Json::as_str)
        .unwrap_or("");

    let progress = match total {
        Some(total) if total > 0.0 => format!(
            "{} of {} ({:.0}%, as the {source} declares it)",
            bytes(received),
            bytes(total),
            (received / total) * 100.0
        ),
        _ if received > 0.0 => format!("{} received, of an unstated total", bytes(received)),
        _ => "nothing received yet".to_string(),
    };

    let line = format!("bundle {decision} — {progress}");
    println!(
        "{}",
        match decision {
            "have" => term::ok(&line),
            "wait_for_unmetered" | "no_bearer" => term::warn(&line),
            "refuse" => term::bad(&line),
            _ => line,
        }
    );
    if let Some(detail) = transfer.get_present("detail").and_then(Json::as_str) {
        println!("{}", term::dim(&term::wrap(detail, 2)));
    }
    if let Some(error) = transfer.get_present("error").and_then(Json::as_str) {
        println!("  {}", term::bad(&term::wrap(error, 2)));
    }
}

/// A byte count a person reads.
///
/// Powers of two with the unit spelled the way the units are defined, because an operating system
/// image is measured against a partition and a partition is measured in mebibytes.
fn bytes(count: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut value = count.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{value:.0} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// §54.4's ladder, one line per invocation.
///
/// The phase and the reason on the same line, because neither is enough alone: `terminal` says it
/// is over and `lease_denied` says whether to grant a token, stop a package, or call the publisher.
fn print_skills(document: &Json) {
    // §54.2's omissions, before the invocations rather than after. A refusal names a *path* and
    // this is why that path is absent — an operator reading "refused by: robot.tool_temperature >
    // 40" and then a line saying the sensor reported NaN has the whole answer; the same two lines
    // in the other order is a puzzle.
    let omissions = document.str_arr("context_omissions");
    if !omissions.is_empty() {
        println!(
            "{}",
            term::warn(&format!(
                "{} fact(s) are missing from every precondition context on this robot (§54.2):",
                omissions.len()
            ))
        );
        for line in &omissions {
            println!("  {}", term::dim(line));
        }
        println!();
    }

    let rows = document.arr_or_empty("invocations");
    if rows.is_empty() {
        println!("Nothing has been invoked.");
        println!(
            "{}",
            term::dim(
                "A skill is installed like any other package and run on demand (§54.1): skynet \
                 skill run <ns/name>."
            )
        );
        return;
    }
    for row in rows {
        let text = |key: &str| {
            row.get_present(key)
                .and_then(Json::as_str)
                .unwrap_or("—")
                .to_string()
        };
        println!(
            "{}  {}  {}  {}",
            text("id"),
            text("package"),
            text("phase"),
            term::dim(&text("reason")),
        );
        let outcome = row.get_present("outcome").and_then(Json::as_str);
        if let Some(outcome) = outcome {
            println!("    outcome {outcome}");
        }
        let detail = text("detail");
        if detail != "—" && !detail.is_empty() {
            println!("    {}", term::dim(&detail));
        }
    }
    // The one thing a reader would otherwise get wrong, printed once rather than per row.
    println!(
        "{}",
        term::dim(
            "The outcome is the OS's, not the skill's (§54.3). A criterion nothing on this robot \
             can evaluate is `unlabeled` rather than a pass."
        )
    );
}

fn show(argv: &[String], method: &str, path: &str, body: Option<String>, print: fn(&Json)) -> i32 {
    match request(argv, method, path, body) {
        Ok((status, document)) => {
            if status >= 400 {
                return report_error(status, &document);
            }
            print(&document);
            0
        }
        Err(reason) => term::fail(&reason),
    }
}

fn report_error(status: u16, document: &Json) -> i32 {
    let detail = document
        .get_present("detail")
        .and_then(Json::as_str)
        .unwrap_or("no detail");
    let code = document.get_present("code").and_then(Json::as_str);
    // The code in brackets, and the lookup after a blank line so it is not reflowed: a hint
    // that wrapped mid-code would not be pasteable. Absent on answers from daemons older than
    // coded errors, which report the detail alone exactly as before.
    match code {
        Some(code) => term::fail(&format!(
            "the robot answered {status} [{code}]: {detail}\n\nskynet explain {code}"
        )),
        None => term::fail(&format!("the robot answered {status}: {detail}")),
    }
}

fn print_status(document: &Json) {
    let text = |key: &str| {
        document
            .get_present(key)
            .and_then(Json::as_str)
            .unwrap_or("—")
            .to_string()
    };
    let number = |key: &str| {
        document
            .get_present(key)
            .and_then(Json::as_f64)
            .unwrap_or(0.0)
    };

    println!(
        "skynetd {} · edition {} · contracts {}",
        text("version"),
        text("edition"),
        text("contracts")
    );
    // The body, on its own line and above the software's: the line above says what is running and
    // this one says what it is running *on*, which is the question somebody with a list of robots
    // is asking. Printed even when it is missing — `text()`'s em dash would read as a robot that
    // answered, and a robot with no descriptor (§28) is a provisioning step nobody ran rather than
    // a detail worth omitting.
    match document.get_present("robot_slug").and_then(Json::as_str) {
        Some(slug) => println!("body {}", term::bold(slug)),
        None => println!(
            "{}",
            term::dim("body: no usable descriptor (§28) — `GET /embodiment` has which of the two")
        ),
    }
    println!(
        "generation {} of {}   {}",
        number("generation"),
        number("desired_generation"),
        term::dim(&format!("desired state: {}", text("desired_source")))
    );
    if let Some(reason) = document
        .get_present("desired_source_reason")
        .and_then(Json::as_str)
    {
        println!("  {}", term::dim(reason));
    }
    println!(
        "enforcement {}   safe state {} ({})   {} package(s) running",
        text("enforcement"),
        text("safe_state"),
        text("safe_state_source"),
        number("packages_running")
    );
    if let Some(note) = document
        .get_present("safe_state_note")
        .and_then(Json::as_str)
    {
        println!("  {}", term::dim(note));
    }
    if document.get("provisioned") == Some(&Json::Bool(false)) {
        println!(
            "{}",
            term::dim(
                "not provisioned: this robot has no credentials, so only status and stop are \
                 answered (§39.2)"
            )
        );
    }
    let stop = document.get("stop_available") == Some(&Json::Bool(true));
    println!(
        "stop: {}",
        if stop {
            term::ok("reachable")
        } else {
            term::bad("NOT REACHABLE")
        }
    );
}

fn print_packages(document: &Json) {
    let packages = document.arr_or_empty("packages");
    if packages.is_empty() {
        println!("{}", term::dim("no packages installed"));
        return;
    }
    for package in packages {
        let text = |key: &str| {
            package
                .get_present(key)
                .and_then(Json::as_str)
                .unwrap_or("—")
        };
        let state = text("state");
        let painted = match state {
            "running" => term::ok(state),
            "failed" => term::bad(state),
            other => term::dim(other),
        };
        println!(
            "{:<32} {:<14} {}",
            text("id"),
            painted,
            term::dim(&format!(
                "{} · wanted {} · {}",
                short(text("digest")),
                text("wanted"),
                grants_of(package)
            ))
        );
    }
}

fn grants_of(package: &Json) -> String {
    let grants: Vec<&str> = package
        .arr_or_empty("grants")
        .iter()
        .filter_map(Json::as_str)
        .collect();
    if grants.is_empty() {
        "no grants".to_string()
    } else {
        grants.join(" ")
    }
}

fn short(digest: &str) -> String {
    digest.chars().take(12).collect()
}

fn print_plan(document: &Json) {
    // One shape for both cases: `pass` is the object, or it is null and `note` says why. The route
    // used to answer a flat object when a pass existed and this printer read it that way, which
    // worked here and broke the console — a route with two shapes has one consumer that is wrong.
    let Some(pass) = document.get_present("pass") else {
        println!(
            "{}",
            term::dim(
                document
                    .get_present("note")
                    .and_then(Json::as_str)
                    .unwrap_or("no pass yet")
            )
        );
        return;
    };
    let document = pass;

    let applied = document.arr_or_empty("applied");
    let deferred = document.arr_or_empty("deferred");
    if applied.is_empty() && deferred.is_empty() {
        println!(
            "{}",
            term::ok("nothing to do — the robot is at its desired state")
        );
    }

    for action in applied {
        let name = action
            .get_present("action")
            .and_then(Json::as_str)
            .unwrap_or("?");
        let subject = action
            .get_present("subject")
            .and_then(Json::as_str)
            .unwrap_or("");
        let outcome = action
            .get_present("outcome")
            .and_then(Json::as_str)
            .unwrap_or("?");
        let mark = match outcome {
            "applied" | "already_true" => term::ok("✓"),
            _ => term::bad("✗"),
        };
        println!("{mark} {name} {subject}  {}", term::dim(outcome));
        if let Some(reason) = action.get_present("reason").and_then(Json::as_str) {
            println!("    {}", term::dim(reason));
        }
    }

    for action in deferred {
        println!(
            "· {} {}  {}",
            action
                .get_present("action")
                .and_then(Json::as_str)
                .unwrap_or("?"),
            action
                .get_present("subject")
                .and_then(Json::as_str)
                .unwrap_or(""),
            term::dim("deferred")
        );
        if let Some(reason) = action.get_present("reason").and_then(Json::as_str) {
            println!("    {}", term::dim(reason));
        }
    }

    if document.get("converged") == Some(&Json::Bool(false)) {
        println!(
            "{}",
            term::dim("not converged: something in the plan has not been applied")
        );
    }
}

fn print_stop(document: &Json) {
    let stopped = document.arr_or_empty("stopped");
    let failed = document.arr_or_empty("failed");
    if stopped.is_empty() && failed.is_empty() {
        println!("{}", term::dim("nothing was running"));
    }
    for id in stopped {
        println!("{} {}", term::ok("stopped"), id.as_str().unwrap_or(""));
    }
    for item in failed {
        println!(
            "{} {}  {}",
            term::bad("could not stop"),
            item.get_present("id").and_then(Json::as_str).unwrap_or(""),
            term::dim(
                item.get_present("reason")
                    .and_then(Json::as_str)
                    .unwrap_or("")
            )
        );
    }
    if let Some(note) = document.get_present("note").and_then(Json::as_str) {
        println!("\n{}", term::dim(note));
    }
}

fn logs(argv: &[String]) -> i32 {
    let args = positional(argv);
    let Some(id) = args.first() else {
        return term::fail("skynet logs <ns/name>");
    };
    let Some(route) = route_id(id) else {
        return term::fail(&format!("`{id}` is not a package id"));
    };
    let lines = flag(argv, "--lines").unwrap_or_else(|| "200".to_string());

    match request(argv, "GET", &format!("/logs/{route}?lines={lines}"), None) {
        Ok((status, document)) => {
            if status >= 400 && document.get("lines") != Some(&Json::Null) {
                return report_error(status, &document);
            }
            if let Some(unavailable) = document.get_present("unavailable").and_then(Json::as_str) {
                // Not an empty log. "This package has said nothing" and "nothing could read what it
                // said" are opposite facts.
                return term::fail(unavailable);
            }
            for line in document.arr_or_empty("lines") {
                println!("{}", line.as_str().unwrap_or(""));
            }
            0
        }
        Err(reason) => term::fail(&reason),
    }
}

/// `skynet session` — §38.2's time-boxed narrowing, from the machine.
///
/// The verb a robotics engineer standing at a bench actually reaches for: *slow this thing down
/// while I have my hands near it, and do not let me forget I did.*
///
///     skynet session                                          what is in force
///     skynet session set --for 20m --velocity 0.25            narrow, for twenty minutes
///     skynet session clear                                    put it back
///
/// **`--for` is required and there is no way to omit it.** §38.2 time-boxes this layer, and a
/// default of "until somebody notices" is exactly the hazard it names — *"a 'maintenance mode, half
/// speed' that outlives the maintenance is a hazard that looks like a setting."*
fn session(argv: &[String]) -> i32 {
    let args = positional(argv);
    match args.first().map(|s| s.as_str()) {
        None => show(argv, "GET", "/session", None, print_session),
        Some("clear") => show(argv, "DELETE", "/session", Some(String::new()), print_note),
        Some("set") => {
            let Some(duration) = flag(argv, "--for") else {
                return term::fail(concat!(
                    "skynet session set --for <duration> [--velocity R] [--torque N] ",
                    "[--base M] [--jerk F] [--name NAME] [--reason WHY]\n\n",
                    "--for is required. A session override is time-boxed (§38.2): there is no way ",
                    "to ask for one that does not end.\n",
                    "Durations: 90s, 20m, 2h."
                ));
            };
            let Some(duration_ms) = parse_duration_ms(&duration) else {
                return term::fail(&format!(
                    "`{duration}` is not a duration. Use 90s, 20m or 2h."
                ));
            };

            let mut document = Json::obj();
            document.set(
                "name",
                Json::from(flag(argv, "--name").unwrap_or_else(|| "bench".to_string()).as_str()),
            );
            document.set("duration_ms", Json::Num(duration_ms as f64));
            if let Some(reason) = flag(argv, "--reason") {
                document.set("reason", Json::from(reason.as_str()));
            }
            let mut narrowed = false;
            for (name, key) in SESSION_NARROWINGS {
                if let Some(raw) = flag(argv, name) {
                    let Ok(number) = raw.parse::<f64>() else {
                        return term::fail(&format!("`{raw}` is not a number for {name}"));
                    };
                    document.set(key, Json::Num(number));
                    narrowed = true;
                }
            }
            if !narrowed {
                return term::fail(concat!(
                    "A session override has to narrow something. Give at least one of ",
                    "--velocity, --torque, --base or --jerk."
                ));
            }

            show(
                argv,
                "POST",
                "/session",
                Some(document.to_canonical().unwrap_or_default()),
                print_session,
            )
        }
        Some(other) => term::fail(&format!(
            "`{other}` is not a session command. Try `skynet session`, `skynet session set` or `skynet session clear`."
        )),
    }
}

/// `skynet calibration` — §45, from the machine.
///
///     skynet calibration                     what is believed, and which guards lost their input
///     skynet calibration event collision     record an impact (§45.4)
///
/// The report existed on the robot and nothing on a bench could ask for it, which is half of §45's
/// value missing: *"an expired extrinsic disables the proximity guard"* is a statement an operator
/// can act on only if somebody tells them.
///
/// **Recording an event is the one write here, and it only ever takes trust away.** §45.4: *"a
/// robot that has just hit something has, by definition, possibly moved its cameras, and continuing
/// to trust yesterday's extrinsics is exactly the wrong response to an impact."* There is
/// deliberately no verb that clears one — re-measuring the set is what clears it, which is the
/// thing that was actually supposed to happen.
fn calibration(argv: &[String]) -> i32 {
    let args = positional(argv);
    match args.first().map(|s| s.as_str()) {
        None => show(argv, "GET", "/calibration", None, print_calibration),
        Some("event") => {
            let Some(event) = args.get(1) else {
                return term::fail(concat!(
                    "skynet calibration event <event> [--note WHY]\n\n",
                    "§45.4's five: collision, estop_under_load, tool_change, driver_replaced, ",
                    "mainboard_swap.\n",
                    "Recording one expires every calibration measured before now whose policy ",
                    "names it. Re-measuring the set is what clears it."
                ));
            };
            let mut document = Json::obj();
            document.set("event", Json::from(event.as_str()));
            if let Some(note) = flag(argv, "--note") {
                document.set("note", Json::from(note.as_str()));
            }
            show(
                argv,
                "POST",
                "/calibration/events",
                Some(document.to_canonical().unwrap_or_default()),
                print_calibration,
            )
        }
        Some(other) => term::fail(&format!(
            "`{other}` is not a calibration command. Try `skynet calibration` or `skynet calibration event`."
        )),
    }
}

/// What `GET /calibration` and `POST /calibration/events` answer.
///
/// The guards come last and are the part worth reading. A table of parameter sets is a table of
/// nouns; *"proximity is unavailable because camera_extrinsics expired"* is the sentence §45.2's
/// table exists to make sayable.
fn print_calibration(document: &Json) {
    if let Some(detail) = document.get_present("detail").and_then(Json::as_str) {
        println!("{} {detail}\n", term::ok("✓"));
    }

    for row in document.arr_or_empty("sets") {
        let set = row.get("set").and_then(Json::as_str).unwrap_or("");
        let state = row.get("state").and_then(Json::as_str).unwrap_or("");
        let mark = if state == "valid" {
            term::ok("✓")
        } else if state == "absent" {
            term::dim("·")
        } else {
            term::warn("!")
        };
        // The reason beside the state, because §45.4's conditions are the whole policy: `expired`
        // on its own does not say whether this robot drifted, was hit, or simply got old, and the
        // three have different fixes.
        let reason = row
            .get_present("reason")
            .and_then(Json::as_str)
            .map(|reason| format!(" ({reason})"))
            .unwrap_or_default();
        println!("  {mark} {set:<20} {state}{reason}");
        if let Some(detail) = row.get_present("detail").and_then(Json::as_str) {
            if !detail.is_empty() {
                println!("      {}", term::dim(&term::wrap(detail, 6)));
            }
        }
    }

    let unavailable = document.arr_or_empty("unavailable");
    if unavailable.is_empty() {
        println!("\n{}", term::dim("no guard has lost its input"));
    } else {
        println!("\n{}", term::bold("guards without their input (§45.5)"));
        for entry in unavailable {
            println!(
                "  {} {:<16} {}",
                term::warn("!"),
                entry.get("guard").and_then(Json::as_str).unwrap_or(""),
                term::dim(entry.get("detail").and_then(Json::as_str).unwrap_or(""))
            );
        }
    }

    // What this robot could not measure, which is the difference between a robot that drifted and
    // one that cannot tell. §45.4's rule is that a missing current reading expires a record rather
    // than extending it, so this list explains expiries that are about the robot and not the work.
    let unmeasured = document.arr_or_empty("unmeasured");
    if !unmeasured.is_empty() {
        let names: Vec<&str> = unmeasured.iter().filter_map(Json::as_str).collect();
        println!(
            "\n{}",
            term::dim(&format!(
                "this robot cannot read: {} — a missing reading expires a record rather than \
                 extending it (§45.4)",
                names.join(", ")
            ))
        );
    }
}

/// `skynet settings` — §38.1's unsigned local configuration, from the machine.
///
///     skynet settings                          what this robot is set to
///     skynet settings set log_level debug      change one
///     skynet settings reset log_level          put one back to its default
///
/// Three settings and no more. §38.1 draws the line at whether a value can reach the Safety Kernel,
/// and everything on the other side of it — every ceiling the kernel enforces — arrives signed or
/// as a session override (§38.2). Typing one of those names here gets the sentence that says so
/// rather than a bare refusal.
fn settings(argv: &[String]) -> i32 {
    let args = positional(argv);
    match args.first().map(|s| s.as_str()) {
        None => show(argv, "GET", "/settings", None, print_settings),
        Some("set") => {
            let (Some(name), Some(value)) = (args.get(1), args.get(2)) else {
                return term::fail(concat!(
                    "skynet settings set <setting> <value>\n\n",
                    "  log_level           error | warn | info | debug\n",
                    "  telemetry_sampling  reconcile passes per §74 window, 1 to 60\n",
                    "  console_theme       system | light | dark"
                ));
            };
            let mut document = Json::obj();
            document.set("setting", Json::from(name.as_str()));
            document.set("value", Json::from(value.as_str()));
            show(
                argv,
                "POST",
                "/settings",
                Some(document.to_canonical().unwrap_or_default()),
                print_settings,
            )
        }
        Some("reset") => {
            let Some(name) = args.get(1) else {
                return term::fail("skynet settings reset <setting>");
            };
            let mut document = Json::obj();
            document.set("setting", Json::from(name.as_str()));
            document.set("reset", Json::Bool(true));
            show(
                argv,
                "POST",
                "/settings",
                Some(document.to_canonical().unwrap_or_default()),
                print_settings,
            )
        }
        Some(other) => term::fail(&format!(
            "`{other}` is not a settings command. Try `skynet settings`, `skynet settings set` or `skynet settings reset`."
        )),
    }
}

/// What `GET /settings` and `POST /settings` answer.
///
/// Every setting, with the ones nobody chose marked as defaults rather than hidden. A list of only
/// what somebody changed answers the wrong question: an engineer at a bench is asking what this
/// robot is doing, not what they did to it.
fn print_settings(document: &Json) {
    if let Some(detail) = document.get_present("detail").and_then(Json::as_str) {
        println!("{} {detail}\n", term::ok("✓"));
    }
    for entry in document.arr_or_empty("settings") {
        let name = entry.get("setting").and_then(Json::as_str).unwrap_or("");
        let value = entry.get("value").and_then(Json::as_str).unwrap_or("");
        let is_default = entry
            .get("is_default")
            .and_then(Json::as_bool)
            .unwrap_or(true);
        // The provenance §38.3 asks for, beside the value rather than in a second listing: *who
        // set this, and when* is the question that follows *what is it* by about a second.
        let provenance = if is_default {
            term::dim("default")
        } else {
            match entry.get_present("set_by").and_then(Json::as_str) {
                Some(who) => term::dim(&format!("set by {who}")),
                None => term::dim("set by unrecorded"),
            }
        };
        println!("  {name:<20} {value:<10} {provenance}");
    }

    let notes = document.arr_or_empty("notes");
    if !notes.is_empty() {
        println!();
        for note in notes {
            // Never swallowed. A setting somebody wrote that this robot ignored is the fact they
            // came here to find.
            println!("{} {}", term::warn("!"), note.as_str().unwrap_or(""));
        }
    }
}

/// `90s`, `20m`, `2h` — in milliseconds.
///
/// Suffixed rather than bare, and the reason is the unit a mistake costs. A bare `20` is twenty of
/// something, and the two readings a person is likeliest to have in mind here — minutes and seconds
/// — are sixty times apart on a control that slows a robot down.
fn parse_duration_ms(text: &str) -> Option<i64> {
    let (digits, scale) = match text.chars().last()? {
        's' => (&text[..text.len() - 1], 1_000),
        'm' => (&text[..text.len() - 1], 60_000),
        'h' => (&text[..text.len() - 1], 3_600_000),
        _ => return None,
    };
    let value: f64 = digits.parse().ok()?;
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    Some((value * scale as f64) as i64)
}

/// What `GET /session` and `POST /session` answer.
fn print_session(document: &Json) {
    let state = document
        .get("state")
        .and_then(Json::as_str)
        .unwrap_or("unknown");
    let Some(over) = document.get_present("override") else {
        // `if` rather than `match` on the state name, and the reason is a tool rather than taste:
        // `tools/check-golden-path.mjs` counts CLI commands by taking every string literal on a
        // match arm across this crate, and a `match` over state names here would put `none` and
        // `unreadable` into the count §73 publishes. The counting rule is broad by design — it
        // exists so that adding a command never fails the build falsely — and the cost of that
        // breadth is paid here, where a state name is not a command.
        if state == "unreadable" {
            println!(
                "{} {}",
                term::warn("!"),
                document
                    .get("detail")
                    .and_then(Json::as_str)
                    .unwrap_or("the override could not be read")
            );
        } else {
            println!(
                "{}",
                term::dim("no session override — this robot is at the ceilings its profile sets")
            );
        }
        return;
    };

    let name = over.get("name").and_then(Json::as_str).unwrap_or("");
    // Expired is drawn in the caution colour and live in the ordinary one. The two are different
    // facts and the difference is the whole reason this route reports three states rather than two.
    if state == "expired" {
        println!(
            "{} session override {} has expired — this robot is back at its profile's ceilings",
            term::warn("!"),
            term::bold(name)
        );
    } else {
        let remaining = over
            .get("remaining_ms")
            .and_then(Json::as_f64)
            .unwrap_or(0.0) as i64;
        println!(
            "{} session override {} — {} left",
            term::ok("●"),
            term::bold(name),
            human_duration(remaining)
        );
    }

    for (key, label, unit) in [
        ("joint_velocity_max_rad_s", "joint velocity", "rad/s"),
        ("joint_torque_max_nm", "joint torque", "Nm"),
        ("base_velocity_max_m_s", "base velocity", "m/s"),
        ("max_delta_per_cycle", "jerk", "of the ceiling per cycle"),
    ] {
        if let Some(number) = over.get_present(key).and_then(Json::as_f64) {
            println!("  {:<16} {} {}", label, number, term::dim(unit));
        }
    }

    if let Some(reason) = over.get_present("reason").and_then(Json::as_str) {
        println!("  {:<16} {}", "reason", term::dim(reason));
    }
    if let Some(who) = over.get_present("set_by").and_then(Json::as_str) {
        println!("  {:<16} {}", "set by", term::dim(who));
    }
}

/// A millisecond count as something a person reads off a bench.
fn human_duration(ms: i64) -> String {
    if ms <= 0 {
        return "no time".to_string();
    }
    let seconds = ms / 1_000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    format!("{}h{:02}m", minutes / 60, minutes % 60)
}

fn grants(argv: &[String]) -> i32 {
    let args = positional(argv);
    match args.first().map(|s| s.as_str()) {
        None => show(argv, "GET", "/grants", None, print_grants),
        Some("add") | Some("remove") => {
            let (Some(id), Some(token)) = (args.get(1), args.get(2)) else {
                return term::fail("skynet grants add|remove <ns/name> <token>");
            };
            let Some(route) = route_id(id) else {
                return term::fail(&format!("`{id}` is not a package id"));
            };
            let adding = args[0] == "add";
            // §54.2's acknowledgement, carried when the operator supplies one. The digest is not
            // typed from memory: the refusal below prints the notes and the exact command.
            let acknowledge = flag(argv, "--acknowledge");
            let (method, path, body) = if adding {
                let mut document = Json::obj();
                document
                    .set("id", Json::from(id.as_str()))
                    .set("token", Json::from(token.as_str()));
                if let Some(digest) = &acknowledge {
                    document.set("acknowledge", Json::from(digest.as_str()));
                }
                (
                    "POST",
                    "/grants".to_string(),
                    Some(document.to_canonical().unwrap_or_default()),
                )
            } else {
                ("DELETE", format!("/grants/{route}/{token}"), None)
            };
            match request(argv, method, &path, body) {
                Ok((status, document)) => {
                    // §54.2: the notes are *displayed*, here, at the moment the grant is refused
                    // for want of them. A refusal that only named the rule would leave an operator
                    // hunting for the text on a Hub they may not be able to reach.
                    if status == 409
                        && document.get("outcome").and_then(Json::as_str)
                            == Some("safety_notes_unread")
                    {
                        return print_safety_notes(id, token, &document);
                    }
                    if status >= 400 {
                        return report_error(status, &document);
                    }
                    println!(
                        "{} {} {}",
                        term::ok("✓"),
                        document
                            .get_present("outcome")
                            .and_then(Json::as_str)
                            .unwrap_or("done"),
                        term::dim(&format!("{id} {token}"))
                    );
                    0
                }
                Err(reason) => term::fail(&reason),
            }
        }
        Some(other) => term::fail(&format!(
            "`grants {other}` is not a subcommand. They are `add` and `remove`."
        )),
    }
}

/// `skynet limits` (§7.3, §6.3).
///
/// `--package` asks what one installed package would be held to, which folds §37.1's transport
/// clamp into the resolution. Without it the answer is the body's own ceilings — the right default,
/// because a clamp belongs to a policy and this edition runs none.
fn limits(argv: &[String]) -> i32 {
    let path = match flag(argv, "--package") {
        // Percent-encoding the id rather than interpolating it: a package id carries a `/`, and a
        // `/` in a query value is legal but a `#` or a `&` in one is a different request.
        Some(id) => format!("/limits?package={}", encode_query(&id)),
        None => "/limits".to_string(),
    };
    show(argv, "GET", &path, None, print_limits)
}

/// Percent-encode a query value, keeping the unreserved set and `/`.
fn encode_query(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The resolved set, for a terminal (§64).
///
/// The guards before the numbers, because the question an operator brings to this command is
/// usually *"why did nothing stop it"* rather than *"what is the ceiling"* — and a guard that
/// cannot run is the answer to the first. Every unavailable guard prints what it is missing, so the
/// output is a list of things to go and fix rather than a list of things that are wrong.
fn print_limits(document: &Json) {
    let text = |key: &str| document.get(key).and_then(Json::as_str).unwrap_or("—");
    println!(
        "limits resolved at {} ({}) — enforced by nothing on this edition",
        term::bold(text("tier")),
        text("enforcement"),
    );

    let number = |key: &str| match document.get(key).and_then(Json::as_f64) {
        Some(value) => format!("{value}"),
        None => "—".to_string(),
    };
    println!(
        "  joint velocity {} rad/s · torque {} Nm · base {} m/s · jerk {}",
        number("joint_velocity_max_rad_s"),
        number("joint_torque_max_nm"),
        number("base_velocity_max_m_s"),
        number("max_delta_per_cycle"),
    );
    println!(
        "  source: {}",
        match document.get("source").and_then(Json::as_str) {
            Some(source) => source.to_string(),
            None => term::dim("none cited — a safety limit without provenance is a rumour"),
        }
    );

    let active: Vec<&str> = document
        .arr_or_empty("active_guards")
        .iter()
        .filter_map(Json::as_str)
        .collect();
    let unavailable = document.arr_or_empty("unavailable_guards");
    println!(
        "\n{} of {} guards would run",
        active.len(),
        active.len() + unavailable.len()
    );
    for guard in &active {
        println!("  {} {guard}", term::ok("✓"));
    }
    for entry in unavailable {
        let missing: Vec<&str> = entry
            .arr_or_empty("missing")
            .iter()
            .filter_map(Json::as_str)
            .collect();
        println!(
            "  {} {:<16} needs {}",
            term::dim("·"),
            entry.get("guard").and_then(Json::as_str).unwrap_or("?"),
            term::dim(&missing.join(", "))
        );
    }

    for notice in document.arr_or_empty("notices") {
        println!(
            "\n{} {}",
            term::bold(
                notice
                    .get("code")
                    .and_then(Json::as_str)
                    .unwrap_or("notice")
            ),
            notice.get("message").and_then(Json::as_str).unwrap_or(""),
        );
    }

    print_transport_plan(document);

    if let Some(refusal) = document.get("package_clamp").and_then(Json::as_str) {
        println!("\n{} {}", term::bold("no package clamp:"), refusal);
    }
    if let Some(note) = document.get("enforced_note").and_then(Json::as_str) {
        println!("\n{}", term::dim(note));
    }
}

/// §37.1's transport plan for one package, when `--package` named one (§6, §32.2).
///
/// `print_transport_plan` rather than `print_plan`, and the collision is worth a name rather than a
/// disambiguating comment: `skynet plan` is the last **reconcile** pass and this is the resolved
/// **transport** plan. §32.2's console already carries a note saying two different plans share a
/// word, and two functions sharing one is how that note stops being enough.
///
/// The bindings that came out `direct` are not listed and the warnings always are, which is §64's
/// rule about what a terminal is for: "twelve channels bound direct" is a number, and "this policy
/// peaked at 180 Nm and your elbow is rated for 80, so it is scaled to 0.44" is the sentence
/// somebody came to the command for.
fn print_transport_plan(document: &Json) {
    let Some(plan) = document.get_present("plan") else {
        return;
    };
    let text = |value: &Json, key: &str| {
        value
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or("—")
            .to_string()
    };

    println!(
        "\n{} {} — {} channel(s) onto `{}`{}",
        term::bold("transport plan:"),
        text(plan, "verdict"),
        plan.arr_or_empty("bindings").len(),
        text(plan, "robot_slug"),
        match plan.get("control_hz").and_then(Json::as_f64) {
            Some(hz) => format!(" at {hz} Hz"),
            None => String::new(),
        }
    );
    // Which package, and whether it is the one running. "What would this be held to" and "what is
    // this held to" are different answers and only one of them is about the robot right now.
    if let Some(state) = document.get("package_state").and_then(Json::as_str) {
        println!("  {}", term::dim(&format!("this package is {state}")));
    }

    for binding in plan.arr_or_empty("bindings") {
        let method = text(binding, "method");
        if method == "direct" {
            continue;
        }
        println!(
            "  {:<10} {} → {}",
            method,
            text(binding, "source_role"),
            binding
                .get("target")
                .and_then(Json::as_str)
                .unwrap_or("nothing on this body"),
        );
    }
    // §84.3's motions, by the same rule: a direct one is a number, anything else is a sentence.
    for binding in plan.arr_or_empty("motions") {
        let method = text(binding, "method");
        if method == "direct" {
            continue;
        }
        println!(
            "  {:<10} {} → {}",
            method,
            text(binding, "source_motion"),
            binding
                .get("target")
                .and_then(Json::as_str)
                .unwrap_or("nothing on this body"),
        );
    }
    for warning in plan.arr_or_empty("warnings") {
        println!(
            "  {} {} {}",
            term::bold("!"),
            text(warning, "code"),
            term::dim(&text(warning, "message")),
        );
    }
}

/// `skynet conformance [--results <file>]` (§15).
///
/// # Why `--results`
///
/// `DRY-COVERAGE-PLAN.md` F21: guest requires nine sim tests beyond the bench one, four of them are
/// the runtime's, and `GET /conformance` has run those on a robot since `skynetd-io::conformance`
/// existed. The answer was then printed for a person and kept — so an award computed anywhere but on
/// the robot read all four as `missing`, whatever the robot had measured. This is
/// `skynet driver check --results` for the other half, in the same document, so `skynet bench award`
/// folds the two.
///
/// # Nothing is re-dated
///
/// `ran_at` is the robot's. §15 decays evidence from when it was measured, and a command that stamped
/// the moment it *fetched* a result would make a week-old run read as today's.
fn conformance(argv: &[String]) -> i32 {
    // Before the request: a mistyped flag should cost nothing, and a missing path should not be
    // discovered after the robot has run eight tests.
    let target = match results_target(argv) {
        Ok(target) => target,
        Err(message) => return term::fail(&message),
    };
    let (status, document) = match request(argv, "GET", "/conformance", None) {
        Ok(answer) => answer,
        Err(reason) => return term::fail(&reason),
    };
    if status >= 400 {
        return report_error(status, &document);
    }
    // Written before anything is printed, for `driver check`'s reason: a write that fails is a
    // refusal, not a line under a clean report that somebody scrolls past.
    if let Some(target) = target {
        let written =
            conformance_results(&document, &format!("skynet conformance {}", origin(argv)))
                .and_then(|text| {
                    std::fs::write(&target, text)
                        .map_err(|error| format!("cannot write {}: {error}", target.display()))
                });
        if let Err(message) = written {
            return term::fail(&message);
        }
    }
    print_conformance(&document);
    0
}

/// Where `--results` asked for the document to go, or `None` when it was not asked for.
///
/// `flag` answers `None` both for a flag that is absent and for one given with nothing after it, and
/// here those are opposite requests: the second is somebody who asked for a file and would get a
/// clean exit and no file.
fn results_target(argv: &[String]) -> Result<Option<std::path::PathBuf>, String> {
    let asked = argv
        .iter()
        .any(|arg| arg == "--results" || arg.starts_with("--results="));
    if !asked {
        return Ok(None);
    }
    match flag(argv, "--results") {
        Some(path) if !path.is_empty() && !path.starts_with("--") => {
            Ok(Some(std::path::PathBuf::from(path)))
        }
        _ => Err(
            "--results was given without a file. Name one — `--results runtime.json` — or omit the \
             flag to print the results without writing them."
                .to_string(),
        ),
    }
}

/// The robot a request went to, in the words that would reach it again.
fn origin(argv: &[String]) -> String {
    match target(argv) {
        Target::Host(address) => format!("--host {address}"),
        Target::Socket(path) => format!("--socket {}", path.display()),
    }
}

/// The robot's `GET /conformance` answer as a results document (§15), serialised canonically.
///
/// The rows go through [`results_from_json`] — the reader `skynet bench award` will judge them with —
/// before anything is written, so a row it would refuse is refused here, where the robot that sent
/// it is still on the other end of the command. A writer that could emit a file its own reader
/// refuses would be two implementations of one format.
fn conformance_results(document: &Json, source: &str) -> Result<String, String> {
    use skynet_contracts::bench::{results_from_json, results_to_json, RESULTS_SCHEMA};

    let rows = document.arr_or_empty("results");
    if rows.is_empty() {
        return Err(
            "the robot's conformance document carries no results, so there is nothing to write. \
             `skynet status` says whether this is a skynetd that serves §15's runtime tests."
                .to_string(),
        );
    }
    let mut candidate = Json::obj();
    candidate
        .set("schema", Json::from(RESULTS_SCHEMA))
        .set("source", Json::from(source))
        .set("results", Json::Arr(rows.to_vec()));
    let read = results_from_json(&candidate).map_err(|error| {
        format!(
            "the robot's results do not read as a results document: {error}. Nothing was written."
        )
    })?;
    let text = results_to_json(&read)
        .to_canonical()
        .map_err(|error| format!("the results could not be serialised: {error:?}"))?;
    Ok(format!("{text}\n"))
}

/// The conformance document, for a terminal.
///
/// The outcomes first and the sentences under them, because the sentences are long and an operator
/// scanning eight rows wants the shape before the argument. Every row has one: a skipped test with
/// no reason is exactly the shape §15's first rule exists against, and printing the outcome alone
/// would reproduce it on the terminal after the runner had avoided it in the data.
fn print_conformance(document: &Json) {
    println!(
        "conformance {} of {} runtime tests executed — this robot's own evidence supports {}",
        document
            .get("executed")
            .and_then(Json::as_f64)
            .unwrap_or(0.0),
        document.get("of").and_then(Json::as_f64).unwrap_or(0.0),
        term::bold(
            document
                .get("attainable")
                .and_then(Json::as_str)
                .unwrap_or("—")
        )
    );

    for result in document.arr_or_empty("results") {
        let outcome = result.get("outcome").and_then(Json::as_str).unwrap_or("?");
        let id = result.get("test_id").and_then(Json::as_str).unwrap_or("?");
        // A failure is the row an operator has to see. A skip is a gap and reads as one.
        let label = format!("  {id:<32} {outcome}");
        println!(
            "{}",
            if outcome == "fail" {
                term::bold(&label)
            } else {
                label
            }
        );
        if let Some(detail) = result.get("detail").and_then(Json::as_str) {
            println!("      {}", term::dim(detail));
        }
    }

    if let Some(note) = document.get("note").and_then(Json::as_str) {
        println!("\n{}", term::dim(note));
    }
}

/// `skynet readiness [files...] [--fleet]`.
///
/// Two modes, distinguished by whether files were named — the `blackbox`/`blackbox inspect`
/// precedent. With none, this reads the robot: `GET /readiness`, the installed descriptor
/// evaluated where it is installed. With files, each is a descriptor document —
/// `skynet driver check <lib> --descriptor` writes one from the driver itself — evaluated here,
/// through the same mapping the robot uses, so the two agree.
///
/// `--fleet` prints the summary instead of the reports: how many bodies could attempt each tier,
/// and which blocker stops the most of them. That is the denominator Phase 6 is measured by.
fn readiness(argv: &[String]) -> i32 {
    // `--fleet` takes no value, and `positional` knows it does not — it is absent from
    // VALUE_FLAGS, so it consumes itself and leaves the file after it a file. This command used to
    // strip the flag before the scan, which fixed this command only; F62 is the general fix.
    let files = positional(argv);
    if files.is_empty() {
        if argv.iter().any(|arg| arg == "--fleet") {
            return term::fail(
                "--fleet summarizes descriptor files, and none were named. Name at least one — \
                 `skynet readiness --fleet a.json b.json` — or drop the flag to read this robot.",
            );
        }
        return show(argv, "GET", "/readiness", None, print_readiness);
    }
    if flag(argv, "--host").is_some() || flag(argv, "--socket").is_some() {
        return term::fail(
            "name descriptor files or a robot, not both. `skynet readiness a.json` evaluates \
             files here; `skynet readiness --host addr:port` asks the robot at that address.",
        );
    }
    // Every file is read and decoded before anything is printed, for `conformance --results`'
    // reason: a refusal half-way through a fleet would leave the reports above it looking
    // complete.
    let mut evaluated: Vec<(String, skynet_contracts::readiness::ReadinessReport, bool)> =
        Vec::new();
    for file in &files {
        match read_descriptor_file(file) {
            Ok((report, simulated)) => evaluated.push(((*file).clone(), report, simulated)),
            Err(message) => return term::fail(&message),
        }
    }
    if argv.iter().any(|arg| arg == "--fleet") {
        let simulated = evaluated.iter().filter(|(_, _, sim)| *sim).count();
        let reports: Vec<skynet_contracts::readiness::ReadinessReport> =
            evaluated.into_iter().map(|(_, report, _)| report).collect();
        let fleet = skynet_contracts::readiness::summarize_readiness(&reports);
        let mut document = skynet_contracts::json::fleet_to_json(&fleet);
        document.set("simulated", Json::from(simulated as f64));
        print!("{}", format_fleet(&document));
        return 0;
    }
    for (index, (file, report, simulated)) in evaluated.iter().enumerate() {
        if evaluated.len() > 1 {
            if index > 0 {
                println!();
            }
            println!("==> {file} <==");
        }
        let mut document = skynet_contracts::json::readiness_to_json(report);
        document.set("simulator", Json::Bool(*simulated));
        print!("{}", format_readiness(&document));
    }
    0
}

/// One descriptor file, evaluated.
///
/// The filesystem is the only impure half: reading fails here, and everything after it is
/// [`evaluate_descriptor_text`], which is pure and is what the tests exercise.
fn read_descriptor_file(
    path: &str,
) -> Result<(skynet_contracts::readiness::ReadinessReport, bool), String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?;
    evaluate_descriptor_text(&text, path)
}

/// One descriptor document, evaluated.
///
/// A file carries no compute measurement, so `compute_keys` is empty: the token
/// `platform.onboard_compute` is the declaration that satisfies the check without one.
///
/// The `bool` is whether the body is simulated. It rides beside the report rather than in it —
/// the report's shape is mirrored into nine editions, and this flag is this surface's — so that
/// [`format_readiness`] can print the §35.3 cap next to the tier it would otherwise overstate.
fn evaluate_descriptor_text(
    text: &str,
    name: &str,
) -> Result<(skynet_contracts::readiness::ReadinessReport, bool), String> {
    let document = Json::parse(text).map_err(|error| format!("{name} is not JSON: {error:?}"))?;
    let descriptor = skynet_contracts::json::descriptor_from_json(&document)
        .map_err(|error| format!("{name} is not a descriptor: {error}"))?;
    let subject =
        skynet_contracts::readiness::subject_from_descriptor(&descriptor, &document, Vec::new());
    Ok((
        skynet_contracts::readiness::evaluate_readiness(&subject),
        descriptor.simulator,
    ))
}

/// One readiness report, for a terminal — and plain text, deliberately.
///
/// Fleet reports are grepped, and `term.rs` explains why colour breaks a pipe: without libc
/// there is no `isatty`, so a colour heuristic reads `TERM` and a report captured into a file
/// keeps its escape codes. Deterministic output is also what makes this testable — every
/// rendering assertion below is on this function's return.
///
/// What is printed is the headline and the blockers, because the blockers are the value: the
/// vendor's to-do list, in tier order. `completeness` is not printed — the struct's own docs
/// call it *"for sorting a fleet-wide view. Not a score anyone should quote."*
fn format_readiness(document: &Json) -> String {
    let mut out = String::new();
    let slug = document.get("slug").and_then(Json::as_str).unwrap_or("?");
    // A simulated body attaining host on paper and guest in fact. The cap is printed next to
    // the tier it qualifies — and only there: a body attaining nothing has blockers instead.
    let capped = document.get("simulator").and_then(Json::as_bool) == Some(true)
        && document.get("attainable").and_then(Json::as_str).is_some();
    let cap = if capped {
        " (simulated — awards cap at guest §35.3)"
    } else {
        ""
    };
    match document.get("attainable").and_then(Json::as_str) {
        Some(tier) => out.push_str(&format!("readiness {slug} — could attempt {tier}{cap}\n")),
        None => out.push_str(&format!("readiness {slug} — could attempt nothing yet\n")),
    }
    let blockers = document.arr_or_empty("blockers");
    if blockers.is_empty() {
        out.push_str("  no blockers — every declaration is in order\n");
    } else {
        for blocker in blockers {
            let check = blocker.get("check").and_then(Json::as_str).unwrap_or("?");
            let tier = blocker
                .get("required_for")
                .and_then(Json::as_str)
                .unwrap_or("?");
            out.push_str(&format!("  {check} (blocks {tier})\n"));
            if let Some(detail) = blocker.get("detail").and_then(Json::as_str) {
                out.push_str(&format!("    {detail}\n"));
            }
        }
    }
    out.push_str(
        "attainable means the paperwork is in order; it is not evidence about the machine.\n",
    );
    out
}

fn print_readiness(document: &Json) {
    print!("{}", format_readiness(document));
}

/// One fleet summary, for a terminal. Plain for [`format_readiness`]'s reason.
fn format_fleet(document: &Json) -> String {
    let mut out = String::new();
    let total = document.get("total").and_then(Json::as_f64).unwrap_or(0.0);
    out.push_str(&format!(
        "fleet of {} {}:\n",
        total,
        if total == 1.0 { "robot" } else { "robots" }
    ));
    let count = |tier: &str| {
        document
            .get("by_tier")
            .and_then(|by_tier| by_tier.get(tier))
            .and_then(Json::as_f64)
            .unwrap_or(0.0)
    };
    // Tier order, not key order: the document is an object and objects do not promise one.
    out.push_str(&format!(
        "  could attempt: none {}, guest {}, host {}, certified {}\n",
        count("none"),
        count("guest"),
        count("host"),
        count("certified")
    ));
    let simulated = document
        .get("simulated")
        .and_then(Json::as_f64)
        .unwrap_or(0.0);
    if simulated > 0.0 {
        out.push_str(&format!(
            "  including {} simulated (awards cap at guest §35.3)\n",
            simulated
        ));
    }
    let blockers = document.arr_or_empty("common_blockers");
    if blockers.is_empty() {
        out.push_str("  common blockers: none — every declaration is in order\n");
    } else {
        out.push_str("  common blockers:\n");
        for blocker in blockers {
            let check = blocker.get("check").and_then(Json::as_str).unwrap_or("?");
            let robots = blocker.get("robots").and_then(Json::as_f64).unwrap_or(0.0);
            out.push_str(&format!(
                "    {check} — {} {}\n",
                robots,
                if robots == 1.0 { "robot" } else { "robots" }
            ));
        }
    }
    out
}

/// `skynet power` (§36).
///
/// The state first, because it is the one word an operator came for, and the reserve underneath it
/// with the terms nobody supplied — §36.2's figure is a floor and a line that printed it bare would
/// be the one place on this platform that presented an under-estimate as a measurement.
///
/// The notes and the faults are printed apart. A note is *this daemon could not read a number* and
/// a fault is *§36 has a problem with what it was given*; they usually have different remedies and
/// often different people, and one merged list is how an operator learns to skim both.
/// `skynet state` — §27's machine, and how much of it anybody actually looked at.
///
/// # Why this is a command and not a section of `skynet status`
///
/// `GET /status` and `GET /state` are two routes on purpose, and `api.rs` says why where it serves
/// them: status is *"the one line a console header wants"* and state is *"the same fact with the
/// observation gaps spelled out"*. The gaps are the whole point of §27 — a state derived from
/// conditions nobody read is a state nobody should act on — and folding them into the header would
/// mean either a header nobody can scan or gaps nobody sees.
///
/// It existed on the wire and on the on-robot console and **nowhere in this CLI**, so the one
/// question §27 is written to answer had no terminal that could ask it. An operator on a bench
/// reads a terminal.
///
/// # What it refuses to flatten
///
/// Every condition is printed, including the ones this robot could not look at, because §48.5's
/// rule is that the difference between *false* and *not looked at* is the difference between a
/// robot that is safe and a robot nobody has checked. A list of only the observed conditions would
/// be shorter and would be the exact reading this whole section exists to prevent.
fn print_state(document: &Json) {
    let text = |key: &str| document.get(key).and_then(Json::as_str).unwrap_or("—");
    let flag = |key: &str| document.get(key).and_then(Json::as_bool).unwrap_or(false);

    // The state and the reason together, always. `safe` alone reads as a robot somebody stopped
    // rather than as a robot nothing has observed, and those send an operator to opposite ends of
    // the building — which is `api.rs`'s own sentence about the same pair.
    println!(
        "{}   {}",
        term::bold(text("state")),
        term::dim(text("reason"))
    );

    let mut properties = Vec::new();
    if flag("safe_state") {
        properties.push("a safe state");
    }
    if flag("commands") {
        properties.push("commanding autonomously");
    }
    if flag("sticky") {
        // The one an operator has to know about before they wonder why a condition improving
        // changed nothing: §27 requires a named human clearance to leave `SAFE`.
        properties.push("sticky — a clearance is needed to leave it, not a better reading");
    }
    if let Some(clearance) = document.get("clearance") {
        if let Some(actor) = clearance.get("pending_by").and_then(Json::as_str) {
            println!(
                "  SAFE clearance pending for {actor}; waiting for the next observed interval"
            );
        }
        if let Some(note) = clearance.get("note").and_then(Json::as_str) {
            println!("  {note}");
        }
    }
    if !properties.is_empty() {
        println!("  {}", term::dim(&properties.join(" · ")));
    }

    println!();
    if let Some(observed) = document.get("observed") {
        // The order is §27's own, which is the order `derive` consults them in — not alphabetical,
        // because the first row that matches decides the state and a reader following a refusal
        // upwards is reading this list.
        for condition in [
            "post",
            "kernel",
            "rhal",
            "homing",
            "stop",
            "guard_hold",
            "guards",
            "policy",
        ] {
            let seen = observed
                .get(condition)
                .and_then(Json::as_bool)
                .unwrap_or(false);
            // The detail and the state live beside the condition they are about, for the two that
            // have them.
            let block = document.get(condition);
            let detail = block
                .and_then(|block| block.get("detail"))
                .and_then(Json::as_str);
            let state = block
                .and_then(|block| block.get("state"))
                .and_then(Json::as_str);

            // The mark says whether anybody **looked**, and it must not be read as whether the
            // answer was good — those are different questions and §48.5 exists because conflating
            // them is how a robot nobody checked gets reported as a robot that is fine. The first
            // draft of this printer put a green tick beside `kernel` on a robot whose own detail
            // read *"no control cycle was recorded"*, which is exactly that mistake drawn in one
            // character. So an observation that found something wrong is marked as a finding, and
            // the state is printed rather than left implied.
            //
            // A slice and `contains`, never a `matches!` with `|` between the arms, and the reason
            // is two functions below in `print_power`: `tools/check-golden-path.mjs` harvests
            // quoted literals that a `|` or a `=>` follows, so spelling this as a pattern put
            // `live`, `present` and `passed` into the set of things `skynet <x>` may be — and the
            // check that counts CLI commands then reported 52 of them.
            // `none` joins the three because it is `policy`'s "nothing is wrong here" answer: a
            // robot with nothing installed to command it is not a robot with a problem, and
            // marking it would put a finding beside every bench machine. `held` is deliberately
            // absent — something is installed and is not running it, which is exactly what an
            // operator scanning this list is looking for.
            //
            // `not_required` and `complete` are §27's two good homing answers and joined the list
            // when `homing` gained a source (§24's two-bit field). `pending` is deliberately
            // absent, and it is the row an operator most needs marked: it covers both *this
            // machine has not been homed* and *this driver does not report homing*, which are the
            // two states that hold a robot at rule 5. Which of the two it is, is in the detail
            // beside it and in the block's `reported` flag — the mark says only that there is
            // something here to read.
            let good = state.is_none_or(|state| {
                [
                    "live",
                    "present",
                    "passed",
                    "none",
                    "not_required",
                    "complete",
                ]
                .contains(&state)
            }) && !flagged(block);
            let mark = match (seen, good) {
                (false, _) => term::dim("·"),
                (true, true) => term::ok("✓"),
                (true, false) => term::warn("!"),
            };
            // §27's `stop` has no state word — an open chain is a boolean — so its finding is
            // read off the block rather than off a vocabulary. Without this the one condition that
            // means *the robot has been stopped by hardware* would draw a green tick.
            let said = match (seen, detail) {
                (false, _) => "not observed — taken at its least capable value".to_string(),
                (true, Some(detail)) => detail.to_string(),
                (true, None) => String::new(),
            };
            match state {
                Some(state) => println!("  {mark} {condition:<11} {state:<8} {}", term::dim(&said)),
                None => println!("  {mark} {condition:<11} {:<8} {}", "", term::dim(&said)),
            }
        }
    }

    // The body the loop recorded, which is what a skill's preconditions read (§54.2). Not one of
    // §27's conditions, so it follows them rather than sitting in their list, and it carries no
    // mark: a joint's position is neither good nor bad.
    if let Some(body) = document.get("body") {
        let observed = body
            .get("observed")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        let detail = body.get("detail").and_then(Json::as_str).unwrap_or("");
        let joints: Vec<String> = match body.get("joints") {
            Some(Json::Obj(entries)) => entries
                .iter()
                .filter_map(|(name, value)| value.as_f64().map(|v| format!("{name} {v:.3}")))
                .collect(),
            _ => Vec::new(),
        };
        let unreadable = body.str_arr("unreadable");
        println!();
        if observed && !joints.is_empty() {
            println!("  body        {}", joints.join("  "));
        } else {
            println!("  body        {}", term::dim(detail));
        }
        if !unreadable.is_empty() {
            println!(
                "  {}",
                term::warn(&format!("not a number: {}", unreadable.join(", ")))
            );
        }
    }

    // The sentence the daemon derives rather than one this CLI composes. §32.2's rule is that a
    // console must never show a value it did not receive, and a count re-derived here would be a
    // second opinion about the robot's own reading — which is the failure `skynetd_io::observed`
    // was written to end.
    if let Some(partial) = document.get("partial").and_then(Json::as_str) {
        println!();
        println!("  {}", term::dim(partial));
    }
}

/// Whether a condition block reports something an operator should look at.
///
/// A separate reading from the state word because not every condition has one. §27's `stop` is a
/// boolean — the chain is open or it is not — so a mark derived only from `state` would draw a
/// green tick beside a robot that has been stopped by its hardware, which is the same class of
/// error as the tick this printer already had to fix for `kernel`.
///
/// Four keys rather than one, and `advisory` is the one worth defending. A guard that **demanded**
/// a hold and was overruled by §7.4's tier is not a robot that is held — `held` is false and §27's
/// state is unchanged — but it is emphatically something an operator should look at: a guard is
/// firing and nothing is acting on it. Only a cycle where a guard actually fired sets it, so this
/// does not mark every robot on the edition; it marks the ones where the guards have something to
/// say and no authority to say it.
///
/// `unavailable` is §27's `guards` — a guard that cannot run at all, for want of a declaration or a
/// calibration. It marks rather than ticks for the same reason: an operator scanning this list for
/// what to look at should find the guard that is off, and the detail beside it already names which
/// of the two it is.
fn flagged(block: Option<&Json>) -> bool {
    ["asserted", "held", "advisory", "unavailable"]
        .iter()
        .any(|key| {
            block
                .and_then(|block| block.get(key))
                .and_then(Json::as_bool)
                .unwrap_or(false)
        })
}

fn print_power(document: &Json) {
    let text = |key: &str| document.get(key).and_then(Json::as_str).unwrap_or("—");
    let flag = |key: &str| document.get(key).and_then(Json::as_bool).unwrap_or(false);

    let state = text("state");
    let headline = format!("power {}   {}", state, text("reason"));
    // The two states in which the robot is stopping. A slice rather than a `match` or a pair of
    // `||` comparisons, and the reason is in another file: `tools/check-golden-path.mjs` builds the
    // command table by harvesting quoted literals that a `|` or a `=>` follows, so both of those
    // spellings would put `critical` in the set of things `skynet <x>` may be — and a document that
    // wrote `skynet critical` would then pass a check whose whole job is to catch that.
    let stopping = ["critical", "reserve"].contains(&state);
    println!(
        "{}",
        if stopping {
            term::bold(&headline)
        } else {
            headline
        }
    );

    match document.get("remaining_wh").and_then(Json::as_f64) {
        Some(wh) => println!("  {wh} Wh remaining, from {}", text("supply")),
        None => println!("  {}", term::dim("no energy figure")),
    }

    if let Some(reserve) = document.get("reserve") {
        let unaccounted = reserve.str_arr("unaccounted");
        match reserve.get("wh").and_then(Json::as_f64) {
            Some(wh) if unaccounted.is_empty() => println!("  reserve {wh} Wh"),
            Some(wh) => println!(
                "  reserve {wh} Wh {}",
                term::dim(&format!(
                    "— a floor: §36.2 also counts {}, and nothing on this robot supplies {}",
                    unaccounted.join(" and "),
                    if unaccounted.len() > 1 {
                        "any of them"
                    } else {
                        "it"
                    }
                ))
            ),
            None => println!(
                "  {}",
                term::dim("no reserve — this body declares no `safe_stop_energy_wh` (§36.2)")
            ),
        }
        // §87.2. Beside `unaccounted` rather than instead of it: the term leaving that list says
        // the computation happened, and the basis says *how*. A reader who can see only the first
        // is reading a worst case over a warehouse as a measurement of where the robot is standing,
        // and on a mobile base those are different numbers.
        // A chain of comparisons rather than a `match`, and the reason is `stopping` above:
        // `tools/check-golden-path.mjs` harvests quoted literals that a match arrow follows to
        // build the table of what `skynet <x>` may be, so an arm for either basis below would put
        // that basis in the set of things this CLI dispatches. The arrow is deliberately not
        // written out in this comment either — the harvester reads comments too, which is how this
        // sentence came to exist on its second draft.
        let basis = reserve
            .get_present("safe_state_basis")
            .and_then(Json::as_str);
        if basis == Some("fixed") {
            println!(
                "  {}",
                term::dim(
                    "safe state: reached where it stands — a fixed base, which is the reading \
                     silence already had (§87.2)"
                )
            );
        } else if basis == Some("mapped_worst_case") {
            println!(
                "  {}",
                term::dim(
                    "safe state: a worst case measured once over a named area (§87.2). Bounded, \
                     not measured here — a figure timed from one spot is a measurement of that spot"
                )
            );
        } else if let Some(other) = basis {
            println!("  {}", term::dim(&format!("safe state: {other}")));
        } else {
            println!(
                "  {}",
                term::dim(
                    "safe state: no basis, so §36.2's distance term is nobody's. On a mobile base \
                     that also refuses the host tier (§87.2)"
                )
            );
        }
    }

    let shed = document.str_arr("shed");
    let thermal = text("thermal");
    if shed.is_empty() {
        println!("  thermal {thermal}");
    } else {
        println!("  thermal {thermal} — giving up {}", shed.join(", "));
    }

    // Four booleans an operator acts on, printed only when they say something. A line reading
    // "new leases: yes" on every healthy robot is a line nobody reads on the one robot where it
    // says no.
    //
    // §36.1's `notifies_operator` is not printed on its own, because it would be: it is true on
    // exactly the three states that already withhold leases, and the state is on the line above.
    // What it does carry that nothing else here does is **which of the two reasons** withheld
    // them. `grants_new_leases` is the state *and* the faults; this is the state alone, so the two
    // disagreeing means a fault took the leases while the charge level was fine. Those send an
    // operator to different places, which is the same distinction the daemon's own refusal
    // sentence draws, and this line drew none of it until 2026-09-16.
    if !flag("grants_new_leases") {
        println!(
            "  {}",
            term::bold(if flag("notifies_operator") {
                "no new task leases — the charge level itself (§36.1)"
            } else {
                "no new task leases — a fault, not the charge level (§36.1)"
            })
        );
    }
    if flag("requires_safe_state") {
        println!(
            "  {}",
            term::bold(if flag("stops_immediately") {
                "stop now, then a safe state (§36.1)"
            } else {
                "controlled stop, then a safe state (§36.1)"
            })
        );
    }
    if flag("degraded") {
        println!("  {}", term::bold("DEGRADED — sustained thermal (§36.3)"));
    }

    for fault in document.arr_or_empty("faults") {
        println!(
            "  {} {}",
            term::bold(fault.get("code").and_then(Json::as_str).unwrap_or("fault")),
            fault.get("detail").and_then(Json::as_str).unwrap_or("")
        );
    }
    for note in document.arr_or_empty("notes") {
        println!("  {}", term::dim(note.as_str().unwrap_or("")));
    }
}

/// §55.2's cell, and the faults that explain a refused precondition.
///
/// The environment is printed as the **context a precondition sees**, not as the facts as they were
/// written, because those differ by exactly the faults below — and an operator whose skill was
/// refused for `env.door closed` needs to see that the key never made it into the context.
fn print_cell(document: &Json) {
    if document.get("held") != Some(&Json::Bool(true)) {
        println!(
            "{}",
            term::dim(
                document
                    .get_present("note")
                    .and_then(Json::as_str)
                    .unwrap_or("no cell")
            )
        );
        return;
    }

    println!(
        "cell {}   {} fixture(s)",
        term::bold(document.get("name").and_then(Json::as_str).unwrap_or("—")),
        document
            .get("fixtures")
            .and_then(Json::as_f64)
            .unwrap_or(0.0),
    );

    if let Some(environment) = document.get("environment").and_then(Json::as_obj) {
        for (key, value) in environment {
            println!(
                "  env.{key} = {}",
                value.to_canonical().unwrap_or_else(|_| String::from("?"))
            );
        }
    }

    // §87.1. The robots this cell names, this one marked, and the layer each contributes. A cell
    // with none prints nothing, which is every cell written before §87 and is the state where there
    // is no pair and the cell *is* its frame.
    let robots = document.arr_or_empty("robots");
    if !robots.is_empty() {
        println!();
        println!(
            "  {}",
            term::bold(&format!("{} robot(s) in this cell", robots.len()))
        );
        for robot in robots {
            let slug = robot.get("slug").and_then(Json::as_str).unwrap_or("—");
            let mine = robot.get("mine") == Some(&Json::Bool(true));
            let placed = match robot.get_present("at").and_then(Json::as_arr) {
                Some(at) if at.len() == 3 => format!(
                    "at {}",
                    at.iter()
                        .map(|value| format!("{:.2}", value.as_f64().unwrap_or(f64::NAN)))
                        .collect::<Vec<String>>()
                        .join(", ")
                ),
                // Not "at the origin". §87.1 refuses an unplaced robot from the second one up, and
                // a line reading `at 0.00, 0.00, 0.00` would be this console supplying the
                // placement the fault exists to demand.
                _ => String::from("unplaced"),
            };
            let drawn = robot
                .get_present("layer")
                .map(|layer| layer.arr_or_empty("volumes").len())
                .unwrap_or(0);
            println!(
                "    {}{slug}   {}",
                if mine { "* " } else { "  " },
                term::dim(&format!("{placed} · {drawn} volume(s)"))
            );
        }
        println!(
            "  {}",
            term::dim(
                "Two kernels cannot see each other's envelope (§87.1), so a constraint over the \
                 pair is enforced by neither. What this cell can do is narrow each robot \
                 separately, and `shared_volume` below is the claim it refuses to let anybody make."
            )
        );
        println!();
    }

    let workspace = document.get_present("workspace");
    let volumes = workspace
        .map(|layer| layer.arr_or_empty("volumes").len())
        .unwrap_or(0);
    println!(
        "  {}",
        term::dim(&format!(
            "{volumes} allowed volume(s) — computed, and not applied: the workspace guard is the \
             kernel's (§33.1)"
        ))
    );

    let faults = document.arr_or_empty("faults");
    if faults.is_empty() {
        return;
    }
    println!();
    for fault in faults {
        println!(
            "  {} {}",
            term::bad(fault.get("code").and_then(Json::as_str).unwrap_or("?")),
            term::dim(fault.get("message").and_then(Json::as_str).unwrap_or("")),
        );
    }
    if let Some(note) = document.get_present("note").and_then(Json::as_str) {
        println!();
        println!("{}", term::dim(&term::wrap(note, 2)));
    }
}

/// §54.2's notes, on the terminal, with the command that acknowledges them.
///
/// The digest is printed inside the command rather than beside it, because the one thing an
/// operator must not have to do is retype a sha256 by hand — and a rule that is annoying to comply
/// with is a rule somebody scripts their way around.
fn print_safety_notes(id: &str, token: &str, document: &Json) -> i32 {
    let notes = document
        .get("safety_notes")
        .and_then(Json::as_str)
        .unwrap_or_default();
    let digest = document.get("digest").and_then(Json::as_str).unwrap_or("");

    println!("{}", term::warn(&format!("safety notes — {id}")));
    println!();
    for line in notes.lines() {
        println!("  {}", term::bold(line));
    }
    println!();
    println!(
        "{}",
        term::dim(&term::wrap(
            document
                .get("detail")
                .and_then(Json::as_str)
                .unwrap_or_default(),
            2,
        ))
    );
    println!();
    println!("  skynet grants add {id} {token} --acknowledge {digest}");
    // Non-zero: nothing was granted. A zero here would let a script treat an unread safety note as
    // a completed grant, which is the failure the whole section is about.
    1
}

fn print_grants(document: &Json) {
    let held = document.get("grants").cloned().unwrap_or_else(Json::obj);
    let leases = document.get("leases").cloned().unwrap_or_else(Json::obj);
    let empty = held.as_obj().is_none_or(|map| map.is_empty());
    if empty {
        println!("{}", term::dim("nothing is granted"));
    }
    if let Some(map) = held.as_obj() {
        for (id, tokens) in map {
            let list: Vec<&str> = tokens
                .as_arr()
                .unwrap_or(&[])
                .iter()
                .filter_map(Json::as_str)
                .collect();
            println!("{id:<32} {}", list.join(" "));
        }
    }
    if let Some(map) = leases.as_obj() {
        if !map.is_empty() {
            println!("\nleases");
            for (id, tokens) in map {
                let list: Vec<&str> = tokens
                    .as_arr()
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Json::as_str)
                    .collect();
                println!("{id:<32} {}", list.join(" "));
            }
        }
    }
    if let Some(note) = document.get_present("note").and_then(Json::as_str) {
        println!("\n{}", term::dim(note));
    }
}

fn rollback(argv: &[String]) -> i32 {
    let args = positional(argv);
    let Some(id) = args.first() else {
        return term::fail("skynet rollback <ns/name>");
    };
    let Some(route) = route_id(id) else {
        return term::fail(&format!("`{id}` is not a package id"));
    };
    match request(
        argv,
        "POST",
        &format!("/packages/{route}/rollback"),
        Some(String::new()),
    ) {
        Ok((status, document)) => {
            if status >= 400 {
                return report_error(status, &document);
            }
            println!(
                "{} rolled {id} back to {}",
                term::ok("✓"),
                document
                    .get_present("digest")
                    .and_then(Json::as_str)
                    .unwrap_or("?")
            );
            0
        }
        Err(reason) => term::fail(&reason),
    }
}

/// `skynet install` — stage a package directory into the store on this machine.
///
/// Local rather than over the API, and that is the honest shape while §34's fetcher does not exist:
/// there is no way to hand a robot bytes over the network, so the bytes are already here. The
/// reconciler then finds the blob present on its next pass and activates it, which is exactly the
/// "skipping any blob already present" path §37.1 describes.
fn install(argv: &[String]) -> i32 {
    let args = positional(argv);
    let (Some(source), Some(id), Some(kind), Some(version), Some(digest)) = (
        args.first(),
        flag(argv, "--id"),
        flag(argv, "--kind"),
        flag(argv, "--version"),
        flag(argv, "--digest"),
    ) else {
        return term::fail(
            "skynet install <dir> --id <ns/name> --kind <kind> --version <v> --digest <hex> \
             [--signed-by <64 hex>] [--signature <128 hex>]",
        );
    };

    // §37.1's second verification step. Both flags or neither: an Ed25519 signature does not say
    // who made it, so a robot handed one without a key cannot tell a stranger's valid signature
    // from a forgery — and a key that signed nothing attests to nothing.
    let signed = match (flag(argv, "--signed-by"), flag(argv, "--signature")) {
        (None, None) => None,
        (Some(key), Some(value)) => {
            let mut document = Json::obj();
            document
                .set("key", Json::from(key.as_str()))
                .set("value", Json::from(value.as_str()));
            match skynetd_io::trust::SignedBy::from_json(&document) {
                Ok(signed) => Some(signed),
                Err(error) => return term::fail(&error.reason()),
            }
        }
        (Some(_), None) => {
            return term::fail(
                "--signed-by names a key and no --signature accompanies it: a key that signed \
                 nothing attests to nothing",
            )
        }
        (None, Some(_)) => {
            return term::fail(
                "--signature carries a signature and no --signed-by names the key that made it",
            )
        }
    };

    let layout = skynetd_io::Layout::new(flag(argv, "--root").unwrap_or_else(|| "/".to_string()));
    let store = skynetd_io::Store::new(layout);
    if let Err(error) = store.prepare() {
        return term::fail(&error.to_string());
    }
    let parsed = match skynetd_io::PackageId::parse(&id) {
        Ok(parsed) => parsed,
        Err(error) => return term::fail(&error.to_string()),
    };

    match store.stage(
        &parsed,
        &kind,
        &version,
        &digest,
        std::path::Path::new(source.as_str()),
    ) {
        Ok(staged) => {
            println!("{} staged {} {}", term::ok("✓"), staged.id, staged.digest);

            let trust = skynetd_io::trust::TrustStore::load(store.layout());
            for line in trust.notices() {
                println!("  {}", term::dim(&line));
            }
            let verdict = trust.check(
                &skynetd_io::trust::package_message(&staged.digest),
                signed.as_ref(),
            );
            match (&verdict.signer, verdict.state) {
                (Some(signer), SignatureState::Verified) => println!(
                    "  {}",
                    term::ok(&format!("signature: verified — signed by `{signer}`"))
                ),
                _ => println!(
                    "  {}",
                    term::warn(&format!(
                        "signature: {} — {}",
                        verdict.state.as_str(),
                        verdict.detail
                    ))
                ),
            }
            if kind == "driver" && verdict.state != SignatureState::Verified {
                println!(
                    "  {}",
                    term::warn(
                        "§19: a driver runs native, so skynetd refuses to activate this one until \
                         its signature verifies against a key this robot trusts",
                    )
                );
            }
            // §54.5's declared graph, walked where the publisher is standing. The gate that holds
            // is the activation (`runtime::composition_refusal`); this is the one they can act on.
            if let Some(fault) =
                skynetd_io::admit::composition_fault(&staged.id.as_str(), &staged.manifest)
            {
                println!(
                    "  {}",
                    term::bad(&skynetd_io::admit::composition_refusal(
                        &staged.id.as_str(),
                        &fault
                    ))
                );
            }
            println!(
                "  {}",
                term::dim("skynetd activates it on its next pass, once the desired state names it")
            );
            0
        }
        Err(error) => term::fail(&error.to_string()),
    }
}

/// `ns/name` into the `ns:name` a route uses.
/// `skynet blackbox` — the index, a seal, or a download (§13, §39.1).
///
/// Three subcommands, and the split follows §39.1's roles rather than convenience: the index is an
/// observer's view (a window and some counts), and sealing and downloading are an operator's,
/// because a sealed recording is every command the robot was given.
///
/// `get` writes the file to the working directory under the name the robot gave it. Not to stdout:
/// an MCAP file is binary, and a terminal that has just had ten megabytes of it written into it is
/// a terminal somebody has to reset.
fn blackbox(argv: &[String]) -> i32 {
    let rest: Vec<String> = argv.to_vec();
    match positional(argv).first().map(|value| value.as_str()) {
        None | Some("index") => show(&rest, "GET", "/blackbox", None, print_blackbox),
        Some("seal") => show(
            &drop_first(&rest),
            "POST",
            "/blackbox/seal",
            Some(String::new()),
            print_sealed,
        ),
        Some("get") => {
            let names = positional(argv);
            let Some(name) = names.get(1) else {
                return term::fail(
                    "skynet blackbox get <name> — the name comes from `skynet blackbox`.",
                );
            };
            download(&drop_first(&rest), name)
        }
        Some("events") => {
            let limit = flag(argv, "--limit").unwrap_or_else(|| "50".to_string());
            let path = format!("/blackbox/events?limit={limit}");
            show(&drop_first(&rest), "GET", &path, None, print_events)
        }
        Some("replay") => {
            let names = positional(argv);
            let Some(path) = names.get(1) else {
                return term::fail(
                    "skynet blackbox replay <file> --limits <file> — a sealed recording on \
                     this machine, and the limits that were in force. Needs no robot.",
                );
            };
            replay_file(path, argv)
        }
        Some("inspect") => {
            let names = positional(argv);
            let Some(path) = names.get(1) else {
                return term::fail(
                    "skynet blackbox inspect <file> — a sealed recording on this machine. \
                     Needs no robot.",
                );
            };
            inspect(path)
        }
        Some(other) => term::fail(&format!(
            "`blackbox {other}` is not a subcommand. They are `index`, `events`, `inspect`, \
             `replay`, `seal` and `get`."
        )),
    }
}

/// `skynet blackbox inspect <file>` — what a sealed recording says, with no robot involved.
///
/// # The one command here that talks to nothing
///
/// Every other robot command is a request over §33.7. This one opens a file, and that is the point:
/// the recording of the day a robot failed is most often looked at *after* the robot is off a bench,
/// on a workstation, by somebody it was emailed to. Until [`skynet_blackbox::read`] existed there
/// was no way to answer *"what is in this file"* except to ask the robot that produced it — which
/// works until the robot is the thing that is broken.
///
/// It stops short of replay deliberately. Replaying a window against resolved limits is the more
/// valuable thing and needs the cycles reassembled out of the four messages each is written as;
/// doing it as a side effect of "open the file" would produce a bad version of both. What this
/// prints is what a person opens a recording to find out: where it came from, whether it is whole,
/// and what happened.
fn inspect(path: &str) -> i32 {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => {
            return term::fail(&format!("`{path}` could not be read: {error}"));
        }
    };

    let recording = match skynet_blackbox::read::read(&bytes) {
        Ok(recording) => recording,
        Err(error) => return term::fail(&error.detail()),
    };

    let meta = |key: &str| recording.meta(key).unwrap_or("—");

    println!("{}", term::bold("provenance"));
    println!("  robot        {}", meta("robot_id"));
    println!("  embodiment   {}", meta("embodiment_digest"));
    println!("  os image     {}", meta("os_image_digest"));
    println!(
        "  tier         {} · enforcement {} · simulator {}",
        meta("tier"),
        meta("enforcement"),
        meta("simulator")
    );
    println!("  sealed       {}", meta("sealed_reason"));

    // §26.3's continuity, printed as the two facts that decide whether this file is evidence.
    println!();
    println!("{}", term::bold("continuity"));
    let complete = recording.meta("complete") == Some("true");
    println!(
        "  {} records, sequences {}–{}",
        meta("records"),
        meta("first_sequence"),
        meta("last_sequence")
    );
    if complete {
        println!(
            "  {}",
            term::ok("complete — no record between those is missing")
        );
    } else {
        // Named individually. "Incomplete" covers three different things and they are not equally
        // bad: a torn slot is a damaged disk, a dropped record is a writer that fell behind, and a
        // hole is a range the ring wrapped past.
        println!(
            "  {}",
            term::bad(&format!(
                "incomplete — {} damaged, {} dropped, holes {}",
                meta("damaged_slots"),
                meta("dropped_records"),
                if recording.meta("holes").is_some_and(|h| h.is_empty()) {
                    "none"
                } else {
                    meta("holes")
                }
            ))
        );
    }

    println!();
    println!("{}", term::bold("what it holds"));
    let mut topics: Vec<(u16, &str, usize)> = recording
        .channels
        .iter()
        .map(|(id, topic)| (*id, topic.as_str(), recording.on(*id).count()))
        .collect();
    topics.sort_by_key(|(id, _, _)| *id);
    for (_, topic, count) in &topics {
        println!("  {count:>8}  {topic}");
    }
    if recording.skipped_records > 0 {
        // A file mostly made of records this edition does not understand is worth saying out loud:
        // it is what a reader from one edition looking at a newer file sees.
        println!(
            "  {}",
            term::dim(&format!(
                "{} record(s) this edition does not interpret, skipped by length",
                recording.skipped_records
            ))
        );
    }

    // The events, because they are what a person is looking for and they are already JSON objects
    // on one channel — no reassembly, unlike a cycle.
    let events: Vec<_> = recording.on(skynet_blackbox::schema::id::EVENT).collect();
    if !events.is_empty() {
        println!();
        println!("{}", term::bold("events"));
        for message in events {
            let Ok(document) = Json::parse(&message.payload) else {
                println!(
                    "  {}",
                    term::dim("(a payload this edition could not parse)")
                );
                continue;
            };
            let text = |key: &str| {
                document
                    .get_present(key)
                    .and_then(Json::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            println!(
                "  {:>8}  {:<20} {:<24} {}",
                message.sequence,
                term::bold(&text("event")),
                text("subject"),
                term::dim(&text("detail"))
            );
        }
    }

    println!();
    println!(
        "{}",
        term::dim(
            "What is above is the file's own account of itself. `skynet blackbox replay` is the \
             other question — whether this window still produces these verdicts — and it needs \
             the limits that were in force, which a recording names and does not carry."
        )
    );
    0
}

/// §41.3's events, printed so the gaps are as visible as the events either side of them.
///
/// The one thing this must not do is print a tidy list. A recording with four hundred dropped
/// records in the middle of it is a recording whose two neighbouring lines are **not** consecutive,
/// and a reader scanning a column of timestamps will believe they are unless the hole is drawn as
/// loudly as the entries.
fn print_events(document: &Json) {
    let rows = document.arr_or_empty("events");
    let held = document.get("held").and_then(Json::as_f64).unwrap_or(0.0);
    let returned = rows.len();

    if rows.is_empty() {
        println!(
            "{}",
            term::dim(
                "no events. §13 records lease grants, stops, seals, installs and gaps; a ring \
                 with none of them is a robot to which none of those has happened since it \
                 started recording."
            )
        );
        return;
    }

    for row in rows {
        let text = |key: &str| {
            row.get_present(key)
                .and_then(Json::as_str)
                .unwrap_or("")
                .to_string()
        };
        let number = |key: &str| row.get(key).and_then(Json::as_f64).unwrap_or(0.0);
        let sequence = number("sequence") as u64;
        if text("kind") == "gap" {
            println!(
                "  {:>8}  {}",
                sequence,
                term::bad(&format!(
                    "gap — {} record(s) lost from tick {} ({})",
                    number("dropped") as u64,
                    number("first_tick") as u64,
                    text("code"),
                )),
            );
            continue;
        }
        println!(
            "  {:>8}  {:<22} {:<28} {}",
            sequence,
            term::bold(&text("code")),
            text("subject"),
            term::dim(&text("detail")),
        );
    }

    // The denominator, always. "50 of 12,000" and "50 of 50" are different robots, and a list that
    // printed only its own length would let the second be read as the first.
    println!();
    println!(
        "{}",
        term::dim(&format!("{returned} of {} in the ring", held as u64))
    );
}

/// Drop the subcommand word, so the flag parser sees only flags and the remaining positionals.
fn drop_first(argv: &[String]) -> Vec<String> {
    let mut seen = false;
    argv.iter()
        .filter(|value| {
            if !seen && !value.starts_with('-') {
                seen = true;
                return false;
            }
            true
        })
        .cloned()
        .collect()
}

fn download(argv: &[String], name: &str) -> i32 {
    let Some(route) = sealed_name(name) else {
        return term::fail(&format!(
            "`{name}` is not a sealed recording's name. They end in `.mcap` and hold no path."
        ));
    };
    let (status, body) = match request_raw(argv, "GET", &format!("/blackbox/{route}"), None) {
        Ok(answer) => answer,
        Err(reason) => return term::fail(&reason),
    };
    if status >= 400 {
        let text = String::from_utf8_lossy(&body);
        let document = Json::parse(&text).unwrap_or_else(|_| Json::obj());
        return report_error(status, &document);
    }
    // Refused rather than overwritten. A recording is evidence, and a second download that silently
    // replaced the first would be the one thing this subsystem must never do.
    if std::path::Path::new(route).exists() {
        return term::fail(&format!(
            "`{route}` is already here. Move it, or download elsewhere."
        ));
    }
    match std::fs::write(route, &body) {
        Ok(()) => {
            println!("{route}  {} bytes", body.len());
            0
        }
        Err(error) => term::fail(&format!("could not write `{route}`: {error}")),
    }
}

/// A sealed recording's name, or nothing.
///
/// The same rule the daemon applies on its side. Checked here as well so that a mistyped name fails
/// at the keyboard rather than as a 404 from a robot, and because a client that will happily ask for
/// `../../etc/shadow` is a client somebody will point at a server that answers.
fn sealed_name(name: &str) -> Option<&str> {
    if name.ends_with(".mcap")
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains("..")
    {
        Some(name)
    } else {
        None
    }
}

/// §90.2's rule, read back: which cycles in the window ran on a remote policy's proposals, from
/// which session and model. Nothing when the recording names no remote session — a robot that has
/// never run one is not shown an empty section that reads like a measurement.
fn remote_lines(document: &Json) -> Vec<String> {
    let Some(remote) = document.get_present("remote") else {
        return Vec::new();
    };
    let count = |value: &Json, key: &str| {
        value
            .get_present(key)
            .and_then(Json::as_f64)
            .unwrap_or_default() as u64
    };
    let mut lines = vec!["remote sessions (§90.2):".to_string()];
    for entry in remote.arr_or_empty("sessions") {
        let text = |key: &str| {
            entry
                .get_present(key)
                .and_then(Json::as_str)
                .unwrap_or("—")
                .to_string()
        };
        // The ring is a window: an `opened` or `closed` that scrolled out is said to be missing,
        // not filled in.
        let digest = entry
            .get_present("digest")
            .and_then(Json::as_str)
            .unwrap_or("digest not in the window");
        let command = |key: &str| {
            entry
                .get_present(key)
                .and_then(|mark| mark.get_present("seq"))
                .and_then(Json::as_str)
                .map_or("—".to_string(), |seq| format!("#{seq}"))
        };
        lines.push(format!(
            "  {:36} {} cycles, commands {}–{}  {digest}",
            text("session"),
            count(entry, "cycles"),
            command("first"),
            command("last"),
        ));
    }
    lines.push(format!(
        "  {} cycle(s) ran on no remote proposal",
        count(remote, "other_cycles")
    ));
    let unreadable = count(remote, "unreadable");
    if unreadable > 0 {
        lines.push(format!(
            "  {unreadable} remote_session event(s) could not be read, and are not guessed at"
        ));
    }
    lines
}

fn print_blackbox(document: &Json) {
    let number = |value: &Json, key: &str| {
        value
            .get_present(key)
            .and_then(Json::as_f64)
            .unwrap_or_default()
    };
    println!(
        "ring       {} records of {} bytes ({} MB)",
        number(document, "capacity") as u64,
        number(document, "record_size") as u64,
        (number(document, "bytes") / (1024.0 * 1024.0)).round() as u64
    );
    println!(
        "window     {}s at {} Hz",
        number(document, "seconds_covered_at_rate").round() as u64,
        number(document, "rate_hz")
    );

    if let Some(window) = document.get_present("window") {
        println!(
            "held       {} records, sequences {}–{}",
            number(window, "records") as u64,
            number(window, "first_sequence") as u64,
            number(window, "last_sequence") as u64
        );
        println!(
            "cycles     {} ({} intervened)",
            number(window, "cycles") as u64,
            number(window, "interventions") as u64
        );
        let complete = window
            .get_present("complete")
            .and_then(Json::as_bool)
            .unwrap_or(false);
        println!(
            "complete   {}{}",
            if complete { "yes" } else { "no" },
            if complete {
                String::new()
            } else {
                format!(
                    " — {} damaged, {} dropped, {} hole(s)",
                    number(window, "damaged") as u64,
                    number(window, "dropped") as u64,
                    window.arr_or_empty("holes").len()
                )
            }
        );
    }

    // Printed rather than folded away. §41.1's guarantee is completeness, and a recording missing
    // four of §26's six channels is not something a console should let a reader assume away.
    if let Some(note) = document
        .get_present("cycles_unavailable")
        .and_then(Json::as_str)
    {
        println!();
        println!("{note}");
    }

    let remote = remote_lines(document);
    if !remote.is_empty() {
        println!();
        for line in remote {
            println!("{line}");
        }
    }

    // §79's collection. Printed from `collection.windows` rather than from `sealed`, because the
    // two lists are the same set and only one of them says whether the Hub has a copy — and a
    // sealed list that did not would leave an operator to assume either answer.
    let collection = document.get_present("collection");
    let listed = collection
        .map(|report| report.arr_or_empty("windows"))
        .unwrap_or_default();

    if !listed.is_empty() {
        println!();
        println!("sealed:");
        for entry in listed {
            let name = entry
                .get_present("name")
                .and_then(Json::as_str)
                .unwrap_or("—");
            let bytes = entry
                .get_present("size_bytes")
                .and_then(Json::as_f64)
                .unwrap_or_default() as u64;
            let collected = entry
                .get_present("collected")
                .and_then(Json::as_bool)
                .unwrap_or(false);
            println!(
                "  {name:40} {bytes:>12} bytes  {}",
                if collected { "on the Hub" } else { "here only" }
            );
        }

        if let Some(report) = collection {
            let count = |key: &str| {
                report
                    .get_present(key)
                    .and_then(Json::as_f64)
                    .unwrap_or_default() as u64
            };
            let pending = count("pending");
            println!();
            if pending == 0 {
                println!("Every sealed window has been collected (§79).");
            } else {
                // Named as a state rather than as a warning. A window that has not crossed is the
                // ordinary case on a metered link, and it is the *bytes* that are worth a number:
                // §34.3's wait is about the size of them.
                println!(
                    "{pending} window(s), {} bytes, are on this robot and nowhere else. \
                     `skynet blackbox get <name>` reads one here; the daemon sends them when an \
                     unmetered link is up (§34.3, §79.3).",
                    count("pending_bytes")
                );
            }
            if let Some(attempt) = report
                .get_present("last_attempt")
                .and_then(|last| last.get_present("error"))
                .and_then(Json::as_str)
            {
                println!("last attempt: {attempt}");
            }
        }
    } else if !document.arr_or_empty("sealed").is_empty() {
        // An older daemon, whose index has no `collection`. Better than showing nothing, and it
        // does not invent a collection state it has no way to know.
        println!();
        println!("sealed:");
        for entry in document.arr_or_empty("sealed") {
            println!(
                "  {:40} {} bytes",
                entry
                    .get_present("name")
                    .and_then(Json::as_str)
                    .unwrap_or("—"),
                entry
                    .get_present("bytes")
                    .and_then(Json::as_f64)
                    .unwrap_or_default() as u64
            );
        }
    }
}

fn print_sealed(document: &Json) {
    let text = |key: &str| {
        document
            .get_present(key)
            .and_then(Json::as_str)
            .unwrap_or("—")
            .to_string()
    };
    println!("sealed     {}", text("name"));
    println!("reason     {}", text("reason"));
    println!("digest     {}", text("digest"));
    println!(
        "records    {}",
        document
            .get_present("records")
            .and_then(Json::as_f64)
            .unwrap_or_default() as u64
    );
    if document.get_present("complete").and_then(Json::as_bool) == Some(false) {
        println!("complete   no — the file says why, and so does `skynet blackbox`.");
    }
    println!();
    println!("skynet blackbox get {} downloads it.", text("name"));
}

fn route_id(id: &str) -> Option<String> {
    let parsed = skynetd_io::PackageId::parse(id).ok()?;
    Some(format!("{}:{}", parsed.namespace(), parsed.name()))
}

/// Why the local socket could not be opened, as the thing to do next.
///
/// This was one sentence for every cause — *"Is skynetd running? Is this the robot?"* — and the
/// commonest cause on a freshly installed robot is neither: the socket belongs to root and the
/// command was run as somebody else. A developer told the daemon might be down goes and looks at a
/// daemon that is fine. The three causes that account for nearly every case get their own answer.
#[cfg(unix)]
fn unreachable_socket(path: &std::path::Path, error: &std::io::Error) -> String {
    let path = path.display();
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => format!(
            "could not open {path}: permission denied.\n  The robot is probably fine — the socket \
             belongs to root, and this command ran as a user who may not open it. Run it with sudo."
        ),
        std::io::ErrorKind::NotFound => format!(
            "there is no socket at {path}.\n  Is skynetd running (`systemctl status skynetd`)? Is \
             this the robot? A supervisor started with another --root puts it under that root."
        ),
        std::io::ErrorKind::ConnectionRefused => format!(
            "{path} is there and nothing is listening on it.\n  That is a socket left behind by a \
             supervisor that has stopped; `systemctl status skynetd` says why it stopped."
        ),
        _ => format!("could not reach {path}: {error}\n  Is skynetd running? Is this the robot?"),
    }
}

/// Send one request and read one response.
/// One request, with the answer's body handed back as bytes.
///
/// Separate from [`request`] because one route does not answer JSON: a sealed black box is MCAP, and
/// parsing it as JSON — or decoding it as UTF-8 on the way — would destroy it.
fn request_raw(
    argv: &[String],
    method: &str,
    path: &str,
    body: Option<String>,
) -> Result<(u16, Vec<u8>), String> {
    let body = body.unwrap_or_default();
    let mut head = format!("{method} {path} HTTP/1.1\r\nHost: skynetd\r\n");
    if let Some(token) = token(argv) {
        head.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if !body.is_empty() {
        head.push_str("Content-Type: application/json\r\n");
    }
    head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    head.push_str("Connection: close\r\n\r\n");
    let wire = format!("{head}{body}");

    let raw = match target(argv) {
        Target::Host(address) => {
            let mut stream = std::net::TcpStream::connect(&address)
                .map_err(|error| format!("could not reach {address}: {error}"))?;
            exchange(&mut stream, wire.as_bytes())?
        }
        #[cfg(unix)]
        Target::Socket(path) => {
            let mut stream = std::os::unix::net::UnixStream::connect(&path)
                .map_err(|error| unreachable_socket(&path, &error))?;
            exchange(&mut stream, wire.as_bytes())?
        }
        #[cfg(not(unix))]
        Target::Socket(path) => {
            let _ = path;
            return Err(
                "the local socket is a POSIX socket and this is not POSIX. Use --host to reach a \
                 robot's console over the LAN."
                    .to_string(),
            );
        }
    };

    // Split on the bytes rather than on a lossy string. Every other route answers JSON and either
    // would do; `GET /blackbox/{name}` answers MCAP, and a lossy decode of a binary body replaces
    // every invalid sequence with U+FFFD — which is to say, it hands back a corrupted recording that
    // looks like a recording.
    let separator = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "the robot's answer had no header".to_string())?;
    let head = String::from_utf8_lossy(&raw[..separator]).into_owned();
    let body = raw[separator + 4..].to_vec();
    let status: u16 = head
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| "the robot's answer had no status".to_string())?;
    Ok((status, body))
}

pub(crate) fn request(
    argv: &[String],
    method: &str,
    path: &str,
    body: Option<String>,
) -> Result<(u16, Json), String> {
    let (status, raw) = request_raw(argv, method, path, body)?;
    let text = String::from_utf8_lossy(&raw);
    let document =
        Json::parse(&text).map_err(|error| format!("the robot's answer is not JSON: {error}"))?;
    Ok((status, document))
}

fn exchange<S: Read + Write>(stream: &mut S, request: &[u8]) -> Result<Vec<u8>, String> {
    stream
        .write_all(request)
        .map_err(|error| format!("could not send the request: {error}"))?;
    stream
        .flush()
        .map_err(|error| format!("could not send the request: {error}"))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .map_err(|error| format!("could not read the answer: {error}"))?;
    Ok(response)
}

/// `skynet blackbox replay <file> --limits <file>` — would this window still produce these verdicts?
///
/// # The second of the two questions a person asks about a recording
///
/// `inspect` answers *what is in this file*. This answers the harder one, and §13's whole argument
/// rests on it: a recording turns *"the robot did something strange on Tuesday"* into a regression
/// test, and it only does that if somebody can put the window back through the Safety Kernel and
/// see whether it still reaches the verdicts it reached then. It fails when the kernel changed
/// under an upgrade, when two editions disagree, or when the limits handed to it are not the ones
/// that were in force — and the third is worth as much as the other two, because it means an
/// incident is being read against the wrong configuration.
///
/// # Why the limits are a separate file rather than something in the recording
///
/// They are not in it, deliberately (§26): a window carries what happened, and the ceilings it
/// happened under are resolved from the embodiment descriptor, the operator profile and the
/// transport clamp. The recording *names* the embodiment through its digest without carrying it.
///
/// `skynetk` therefore writes the set it ran under **beside** every window it seals, and that
/// sibling is what this reads when no `--limits` is given. It is the ordinary case: somebody was
/// handed a recording off a bench and has one file to point at. `--limits` stays for the other
/// thing replay is for — asking what this window would have done under a profile that was not in
/// force.
///
/// It takes either a `GET /limits` document or a whole **`skynet doctor` bundle**, which carries
/// it as a section. The second is the one an operator
/// is likely to have: §41.3's bundle is what a support conversation opens with, and a tool that
/// made somebody cut a subsection out of it by hand would be reintroducing the interview that
/// section exists to end. A bundle whose `limits` section is a **refusal** says so with the
/// robot’s own reason, which is more useful than a parse error about a missing field.
///
/// Handing them in is therefore also the check: a replay run against the wrong profile diverges
/// loudly rather than agreeing quietly.
///
/// # What it refuses, and it is not a shortage of effort
///
/// A limits document renders the guard *partition* for a console and leaves out two things the
/// kernel evaluates — §7.1's separation thresholds and §8.2's capability grants. That is harmless
/// for a window which never engaged either guard, and it is fatal for one that did: the replay
/// would run with the guard active and no input, recompute every cycle the guard decided, and
/// present the difference as the kernel disagreeing with itself.
///
/// So the window is asked. A guard the document could not reconstruct is fine when nothing in the
/// recording engaged it, and a refusal otherwise — including when the recording is too old to say,
/// because *cannot tell* is not *did not happen*.
fn replay_file(path: &str, argv: &[String]) -> i32 {
    // Beside the window unless told otherwise. `skynetk` writes the limits it ran under next to
    // every window it seals, so the ordinary case — somebody handed a recording off a bench —
    // needs no second argument, and `--limits` stays for the other thing replay is for: asking
    // what this window would have done under a profile that was not in force.
    let beside = std::path::Path::new(path).with_extension("limits.json");
    let limits_path = match flag(argv, "--limits") {
        Some(given) => given,
        None if beside.is_file() => beside.to_string_lossy().into_owned(),
        None => {
            return term::fail(&format!(
                "no limits to replay against. `{}` is not beside the recording, so pass \
                 --limits <file> — the body of `GET /limits` from the robot, or a \
                 `skynet doctor` bundle carrying that section. They are not in the recording: \
                 a window carries what happened, not the ceilings it happened under.",
                beside.display()
            ));
        }
    };

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) => return term::fail(&format!("`{path}` could not be read: {error}")),
    };
    let recording = match skynet_blackbox::read::read(&bytes) {
        Ok(recording) => recording,
        Err(error) => return term::fail(&error.detail()),
    };
    let put_back = match skynet_blackbox::assemble::assemble(&recording) {
        Ok(put_back) => put_back,
        Err(error) => return term::fail(&error.detail()),
    };
    if put_back.records.is_empty() {
        // Vacuous truth is the failure mode of every conformance predicate. A replay of no cycles
        // agrees about everything.
        return term::fail(
            "this recording holds no cycles, so there is nothing to replay. A window of events \
             and gaps is a robot that recorded no control loop — which `skynet blackbox inspect` \
             will show.",
        );
    }

    let limits_text = match std::fs::read_to_string(limits_path.as_str()) {
        Ok(text) => text,
        Err(error) => {
            return term::fail(&format!("`{limits_path}` could not be read: {error}"));
        }
    };
    let document = match Json::parse(&limits_text) {
        Ok(document) => document,
        Err(_) => {
            return term::fail(&format!(
                "`{limits_path}` is not JSON. It should be the body of `GET /limits`."
            ));
        }
    };
    let document = match limits_section(&document) {
        Ok(document) => document,
        Err(reason) => return term::fail(&reason),
    };
    let owned = match skynet_contracts::safety::limits_from_json(document) {
        Ok(owned) => owned,
        Err(error) => return term::fail(&error.detail()),
    };

    // The refusal, per guard the document could not supply an input for.
    for entry in owned.unreconstructed() {
        match engagement(&put_back, entry.guard) {
            Engagement::Never => {}
            Engagement::Somewhere => {
                return term::fail(&format!(
                    "the `{}` guard was active on this robot, {} is not in a limits document, and \
                     this window contains {}. Replaying it would recompute every cycle that guard \
                     decided against an input nothing here can reconstruct.",
                    entry.guard.as_str(),
                    entry.missing,
                    entry.engaged_by
                ));
            }
            Engagement::NoRule => {
                return term::fail(&format!(
                    "the `{}` guard was active on this robot and {} is not in a limits \
                     document, and this tool has no rule for deciding whether this window \
                     engaged it. That is a gap here rather than in the recording, and \
                     guessing either way would be worse than saying so.",
                    entry.guard.as_str(),
                    entry.missing
                ));
            }
            Engagement::Unknowable => {
                return term::fail(&format!(
                    "the `{}` guard was active on this robot, {} is not in a limits document, and \
                     this recording is too old to say whether it contains {}. Cannot tell is not \
                     did not happen, so this window cannot be replayed against this document.",
                    entry.guard.as_str(),
                    entry.missing,
                    entry.engaged_by
                ));
            }
        }
    }

    let result = owned.with_resolved(|limits, envelope| {
        skynet_blackbox::replay::replay(
            &put_back.records,
            &put_back.channels,
            &put_back.capabilities,
            limits,
            envelope,
        )
    });

    println!("{}", term::bold("replayed"));
    println!(
        "  {} cycles from `{}`, against the limits in `{}`",
        result.cycles, path, limits_path
    );
    println!(
        "  tier {} · enforcement {}",
        owned.tier().as_str(),
        owned.enforcement().as_str()
    );
    println!(
        "  {} of the {} cycles had a predecessor, so the rate limit was evaluated as it was on \
         the robot",
        result.warmed,
        result.evaluated()
    );

    // §26.4: whether the window recorded every field the replay evaluates.
    //
    // Said out loud in both directions, because a replay is evidence and evidence that will not
    // report its own completeness is the shape §15 names as the most damaging thing this platform
    // could ship. Until now a window missing `capability` or `nearest_person_m` on some of its
    // cycles replayed and printed exactly like a whole one: the per-guard `Unknowable` below fires
    // only when a field is absent from *every* cycle, which is the recording-is-too-old case and
    // the benign one.
    //
    // The partial case is the one worth the alarm. One writer does not stop writing a field
    // halfway through a window, so a count between zero and all of them is a file that has been
    // edited or a writer with a bug — and either way the verdicts below were reproduced from a
    // robot that is not quite the one that produced them.
    let cycle_count = put_back
        .records
        .iter()
        .filter(|record| matches!(&record.body, skynet_blackbox::Body::Cycle(_)))
        .count();
    if put_back.absent.verdict_inputs_all_recorded() {
        println!("  every field this replay evaluates was recorded on every cycle");
    } else {
        for (field, missing) in [
            ("capability", put_back.absent.capability),
            ("nearest_person_m", put_back.absent.nearest_person_m),
        ] {
            if missing == 0 {
                continue;
            }
            let line = if missing == cycle_count {
                format!(
                    "no cycle carries `{field}`, so this window was sealed before the field \
                     existed — the guards reading it are reported below as unknowable rather \
                     than as never engaged"
                )
            } else {
                format!(
                    "{missing} of the {cycle_count} cycles carry no `{field}`, and one writer \
                     does not stop midway — this file has been edited or the writer had a bug, \
                     and the verdicts below came from a robot not quite the one that recorded them"
                )
            };
            println!("  {}", term::warn(&line));
        }
    }

    // §25: the loop returns before the kernel when the policy posted nothing, so those cycles
    // carry no verdict to reproduce. Said before the result rather than after it, because a
    // window that is mostly idle makes the line below mean much less than it looks like.
    if result.unposted > 0 {
        println!(
            "  {}",
            term::warn(&format!(
                "{} of the {} cycles had nothing posted, so the kernel was never asked and there \
                 is no verdict in them to reproduce",
                result.unposted, result.cycles
            ))
        );
    }
    if result.evaluated() == 0 {
        println!();
        return term::fail(
            "no policy commanded anything anywhere in this window, so there is nothing to \
             replay. This is a recording of a robot whose loop turned and whose policy never \
             spoke — `skynet blackbox inspect` will show what it does hold.",
        );
    }

    println!();
    if result.deterministic() {
        println!(
            "{}",
            term::ok(&format!(
                "all {} evaluated cycles reproduced — this edition's Safety Kernel reaches every \
                 verdict the recording holds",
                result.agreed
            ))
        );
    } else {
        println!(
            "{}",
            term::bad(&format!(
                "{} of {} evaluated cycles diverged",
                result.diverged.len(),
                result.evaluated()
            ))
        );
        for outcome in result.diverged.iter().take(20) {
            for divergence in &outcome.divergences {
                println!(
                    "  tick {:<10} {:<10} recorded {:<24} recomputed {}",
                    outcome.tick, divergence.field, divergence.recorded, divergence.recomputed
                );
            }
        }
        if result.diverged.len() > 20 {
            println!(
                "  {}",
                term::dim(&format!("and {} more", result.diverged.len() - 20))
            );
        }
    }

    // §26.4. A cycle whose read failed replays perfectly and shows a robot holding still, and an
    // investigator drawing "the arm was stationary" out of it is reading the previous cycle's
    // positions repeated. So it is said here rather than left for somebody to notice.
    if let Some(note) = result.observation_note() {
        println!();
        println!("{}", term::warn(&note));
    }

    if !put_back.incomplete.is_empty() {
        println!();
        println!(
            "{}",
            term::warn(&format!(
                "{} cycle(s) were not replayed because the file does not hold all four of their \
                 messages",
                put_back.incomplete.len()
            ))
        );
        for gap in put_back.incomplete.iter().take(5) {
            println!("  {}  {}", gap.cycle, term::dim(&gap.reason));
        }
    }

    // The guards that were active and had nothing behind them, said out loud even when the window
    // never engaged them — because "it did not matter here" is a different sentence from "it was
    // checked", and only the first is true.
    if !owned.unreconstructed().is_empty() {
        println!();
        for entry in owned.unreconstructed() {
            println!(
                "{}",
                term::dim(&format!(
                    "`{}` was active and {} is not in a limits document. Nothing in this window \
                     engaged it, so no verdict here depended on it.",
                    entry.guard.as_str(),
                    entry.missing
                ))
            );
        }
    }

    // The documented exception to `term::fail`’s *exit 1 throughout*, and it is the same
    // exception `driver check` takes: this command **answers a question**, and *the window did
    // not reproduce* is an answer rather than a failure to run. Collapsing the two would be
    // worst for the caller that needs them apart — Skynet Console shells out to this, and a pane
    // that rendered a divergence as "the tool failed" would hide the finding under a tooling
    // complaint.
    //
    //     0  every evaluated cycle reproduced
    //     1  it could not be run: no file, no limits, or a guard whose input is missing
    //     2  it ran, and the window did not reproduce
    if !result.deterministic() {
        return 2;
    }
    0
}

/// Whether a window ever engaged a guard whose input the limits document could not supply.
enum Engagement {
    /// No cycle in the window carried the input, so the guard decided nothing here.
    Never,
    /// At least one did.
    Somewhere,
    /// The recording predates the field, so the question cannot be answered from it.
    Unknowable,
    /// This tool has no rule for that guard, which is a gap here rather than in the recording.
    ///
    /// Separate from [`Engagement::Unknowable`] because the two send somebody to different
    /// places, and conflating them was a real defect: the watchdog arrived with no rule and
    /// was reported as *"this recording is too old to say"* about a recording sealed minutes
    /// earlier. A refusal that misnames its own cause is worse than one that admits ignorance.
    NoRule,
}

fn engagement(
    put_back: &skynet_blackbox::assemble::Assembled,
    guard: skynet_blackbox::Guard,
) -> Engagement {
    let cycles = |body: &skynet_blackbox::Body| matches!(body, skynet_blackbox::Body::Cycle(_));
    let total = put_back
        .records
        .iter()
        .filter(|record| cycles(&record.body))
        .count();
    match guard {
        skynet_blackbox::Guard::Proximity => {
            if put_back.absent.nearest_person_m == total {
                return Engagement::Unknowable;
            }
            for record in &put_back.records {
                if let skynet_blackbox::Body::Cycle(cycle) = &record.body {
                    if cycle.nearest_person_m.is_finite() {
                        return Engagement::Somewhere;
                    }
                }
            }
            Engagement::Never
        }
        skynet_blackbox::Guard::ModeGate => {
            if put_back.absent.capability == total {
                return Engagement::Unknowable;
            }
            for record in &put_back.records {
                if let skynet_blackbox::Body::Cycle(cycle) = &record.body {
                    if cycle.capability != skynet_blackbox::record::NO_CAPABILITY {
                        return Engagement::Somewhere;
                    }
                }
            }
            Engagement::Never
        }
        // Every cycle the kernel was asked about is one the watchdog decided: it is the first
        // guard in the ladder and it runs on the command itself rather than on a reading. So
        // the question is whether this window has any command in it at all, which §24 answers
        // with the sequence — zero is *no command has ever arrived*, and the record has
        // carried it since it was written, so there is no older recording that cannot say.
        skynet_blackbox::Guard::Watchdog => {
            for record in &put_back.records {
                if let skynet_blackbox::Body::Cycle(cycle) = &record.body {
                    if cycle.command_seq != 0 || cycle.command_stamp_ns != 0 {
                        return Engagement::Somewhere;
                    }
                }
            }
            Engagement::Never
        }
        // A guard that becomes unreconstructable later must arrive here with its own answer
        // rather than borrowing one that happens to look safe.
        _ => Engagement::NoRule,
    }
}

/// The limits document itself, or the `limits` section of a `skynet doctor` bundle.
///
/// A bundle is recognised by its own `schema` rather than by guessing from the shape, because
/// the two documents have no field in common and a guess would misreport one as the other. The
/// interesting branch is the third: §41.3’s bundle records a route it could not read **as
/// unread, with the reason**, so a bundle collected without a credential carries a `limits`
/// section that is a refusal. Passing the robot’s own sentence back is the whole value of that
/// design, and reporting it as a malformed document would throw it away.
fn limits_section(document: &Json) -> Result<&Json, String> {
    if document.get("schema").and_then(Json::as_str) != Some("skynet.doctor.v1") {
        return Ok(document);
    }
    for section in document.arr_or_empty("sections") {
        if section.get("section").and_then(Json::as_str) != Some("limits") {
            continue;
        }
        if let Some(held) = section.get_present("document") {
            return Ok(held);
        }
        let reason = section
            .get_present("unread")
            .and_then(Json::as_str)
            .unwrap_or("no reason was recorded");
        return Err(format!(
            "this doctor bundle could not read the robot’s limits, so there is nothing to replay \
             against. The robot said: {reason}"
        ));
    }
    Err(String::from(
        "this doctor bundle has no `limits` section, so it was written by a tool that does not \
         collect one. Use the body of `GET /limits` instead.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn a_recording_with_remote_sessions_names_them_and_one_without_says_nothing() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        let document = Json::parse(&format!(
            r#"{{"remote":{{"sessions":[
                {{"session":"9f1c2d3e-0000-4000-8000-000000000001","digest":"{digest}",
                  "first":{{"seq":"1","stamp_ns":"1000"}},"last":{{"seq":"40","stamp_ns":"9000"}},
                  "cycles":40}},
                {{"session":"9f1c2d3e-0000-4000-8000-000000000002","digest":null,
                  "first":{{"seq":"1","stamp_ns":"20000"}},"last":null,"cycles":7}}
              ],"other_cycles":12,"unreadable":1}}}}"#
        ))
        .expect("a document");
        let lines = remote_lines(&document);
        assert_eq!(lines[0], "remote sessions (§90.2):");
        assert!(
            lines[1].contains("40 cycles, commands #1–#40"),
            "{}",
            lines[1]
        );
        assert!(lines[1].ends_with(&digest));
        // Still open, and its `opened` scrolled out of the ring: both said, neither invented.
        assert!(lines[2].contains("7 cycles, commands #1–—"), "{}", lines[2]);
        assert!(lines[2].ends_with("digest not in the window"));
        assert_eq!(lines[3], "  12 cycle(s) ran on no remote proposal");
        assert!(lines[4].contains("1 remote_session event(s) could not be read"));

        assert!(remote_lines(&Json::parse("{}").expect("empty")).is_empty());
    }

    #[test]
    fn flags_are_read_in_both_spellings() {
        let args = argv(&["--host", "10.0.0.4:8080", "--token=abc"]);
        assert_eq!(flag(&args, "--host").as_deref(), Some("10.0.0.4:8080"));
        assert_eq!(flag(&args, "--token").as_deref(), Some("abc"));
        assert_eq!(flag(&args, "--socket"), None);
    }

    /// A flag's value must not be mistaken for a package id — `skynet logs --lines 50` would
    /// otherwise ask the robot for the journal of a package called `50`.
    #[test]
    fn a_flag_value_is_not_a_positional_argument() {
        let args = argv(&["openrobot/grasp", "--lines", "50", "--token=x"]);
        let found = positional(&args);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0], "openrobot/grasp");
    }

    /// F62's control, and the reason the table exists. `--fleet` takes no value, so the file named
    /// after it is a file. Before the table `positional` consumed it as the flag's value and
    /// `skynet readiness --fleet a.json b.json` summarized a fleet of one — silently, because a
    /// fleet of one is a perfectly good report.
    #[test]
    fn a_boolean_flag_does_not_eat_the_file_after_it() {
        let args = argv(&["--fleet", "a.json", "b.json"]);
        let found = positional(&args);
        assert_eq!(found.len(), 2, "got {found:?}");
        assert_eq!(found[0], "a.json");
        assert_eq!(found[1], "b.json");

        // The same flag after the files, which is where it used to be harmless.
        let trailing = argv(&["a.json", "--fleet"]);
        assert_eq!(positional(&trailing).len(), 1);
    }

    /// The table and the code cannot drift. Every `flag(…, "--name")` in this module names a flag
    /// that takes a value, so every one of them has to be in [`VALUE_FLAGS`] — otherwise
    /// `positional` leaves that flag's value among the positional arguments, where a package id is
    /// expected. Read off this file's own source, because a value flag nobody wrote a test for is
    /// exactly the one this would miss.
    #[test]
    fn every_value_flag_this_module_reads_is_in_the_table() {
        let source = include_str!("robot_cmd.rs");
        let mut found = Vec::new();
        for (index, _) in source.match_indices("flag(") {
            let rest = &source[index + "flag(".len()..];
            let Some(comma) = rest.find(", \"--") else {
                continue;
            };
            // Only a call whose name is written out here: `flag(argv, "--lines")`. A name reached
            // through a variable is SESSION_NARROWINGS, checked by the test below.
            let after = &rest[comma + ", \"".len()..];
            let Some(close) = after.find('"') else {
                continue;
            };
            let name = &after[..close];
            if name.contains(char::is_whitespace) {
                continue;
            }
            found.push(name);
        }
        found.sort_unstable();
        found.dedup();
        assert!(
            found.len() > 20,
            "the scan found {} call sites, which is too few to be reading this file: {found:?}",
            found.len()
        );
        for name in found {
            assert!(
                VALUE_FLAGS.contains(&name),
                "`{name}` is read as a value flag and is not in VALUE_FLAGS (F62)"
            );
        }
    }

    /// The four names `session set` reaches [`flag`] with through a variable, which the scan above
    /// cannot see.
    #[test]
    fn the_session_narrowings_are_value_flags() {
        for (name, _) in SESSION_NARROWINGS {
            assert!(
                VALUE_FLAGS.contains(name),
                "`{name}` narrows a session and is not in VALUE_FLAGS (F62)"
            );
        }
    }

    /// The table is sorted and has no duplicate, so a new flag has one obvious place to go and
    /// reading it tells you whether a flag is already there.
    #[test]
    fn the_value_flag_table_is_sorted_and_unique() {
        let mut sorted = VALUE_FLAGS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted, VALUE_FLAGS,
            "VALUE_FLAGS is unsorted or has a duplicate"
        );
    }

    #[test]
    fn a_package_id_becomes_the_colon_form_a_route_uses() {
        assert_eq!(
            route_id("openrobot/grasp").as_deref(),
            Some("openrobot:grasp")
        );
        assert_eq!(route_id("../../etc"), None);
        assert_eq!(route_id("openrobot"), None);
    }

    /// What `GET /conformance` sends, built by the daemon's own serializer rather than written out
    /// here — a fixture typed from memory would test this module against its author's idea of the
    /// wire, which is the one thing F21's gap was about.
    fn conformance_document() -> Json {
        use skynet_contracts::certification::{SctsOutcome, SctsResult};
        use skynetd_io::conformance::{to_json, Run, RUNTIME_TESTS};

        let results = RUNTIME_TESTS
            .iter()
            .map(|id| {
                let (outcome, ran_at) = match *id {
                    "safety.kernel.parity" => (SctsOutcome::Pass, "2026-09-10T08:00:00Z"),
                    "package.signature.required" => (SctsOutcome::Fail, "2026-09-10T08:00:01Z"),
                    _ => (SctsOutcome::Skipped, ""),
                };
                SctsResult {
                    test_id: id.to_string(),
                    outcome,
                    value: None,
                    ran_at: ran_at.to_string(),
                    detail: Some(format!("{id}, as the robot described it")),
                }
            })
            .collect();
        to_json(
            &Run {
                results,
                executed: 2,
            },
            false,
        )
    }

    /// F21: the runtime's half of the sim suite reaches `skynet bench award` in the same document
    /// `driver check` writes, read back by the reader that will judge it — with the robot's dates
    /// and not this machine's, and with a skipped test still undated.
    #[test]
    fn a_robots_conformance_run_becomes_a_results_document_bench_award_reads() {
        use skynet_contracts::bench::results_from_json;
        use skynet_contracts::certification::SctsOutcome;

        let text = conformance_results(
            &conformance_document(),
            "skynet conformance --socket /run/skynetd.sock",
        )
        .expect("a well-formed run converts");
        assert!(text.ends_with('\n'));

        let read = results_from_json(&Json::parse(text.trim_end()).expect("json")).expect("reads");
        assert_eq!(read.source, "skynet conformance --socket /run/skynetd.sock");
        assert_eq!(read.results.len(), 8);

        let parity = &read.results[0];
        assert_eq!(parity.test_id, "safety.kernel.parity");
        assert_eq!(parity.outcome, SctsOutcome::Pass);
        assert_eq!(
            parity.ran_at, "2026-09-10T08:00:00Z",
            "the robot's date, not now"
        );

        let signature = read
            .results
            .iter()
            .find(|result| result.test_id == "package.signature.required")
            .expect("present");
        assert_eq!(
            signature.outcome,
            SctsOutcome::Fail,
            "a failure travels as one"
        );

        for result in &read.results {
            if result.outcome == SctsOutcome::Skipped {
                assert!(result.ran_at.is_empty(), "{} is undated", result.test_id);
            }
            assert!(
                result.detail.is_some(),
                "{} keeps its sentence",
                result.test_id
            );
        }
    }

    /// The control: a row the award's reader would refuse — a `bench` test, which no run on a robot
    /// can produce and a results document may not carry — is refused here, before a file exists.
    #[test]
    fn a_row_the_awards_reader_would_refuse_is_refused_before_anything_is_written() {
        let mut document = conformance_document();
        let mut rows = document.arr_or_empty("results").to_vec();
        let mut bench = Json::obj();
        bench
            .set("test_id", Json::from("rhal.state.rate"))
            .set("outcome", Json::from("pass"))
            .set("ran_at", Json::from("2026-09-10T08:00:00Z"));
        rows.push(bench);
        document.set("results", Json::Arr(rows));

        let error = conformance_results(&document, "skynet conformance").unwrap_err();
        assert!(error.contains("Nothing was written"), "{error}");
    }

    #[test]
    fn a_document_with_no_results_has_nothing_to_write() {
        let error = conformance_results(&Json::obj(), "skynet conformance").unwrap_err();
        assert!(error.contains("nothing to write"), "{error}");
    }

    /// `--results` with nothing after it is a request for a file, and must not read as no request.
    #[test]
    fn results_without_a_file_is_refused_rather_than_ignored() {
        assert_eq!(
            results_target(&argv(&["--host", "10.0.0.4:8080"])),
            Ok(None)
        );
        assert_eq!(
            results_target(&argv(&["--results", "runtime.json"])),
            Ok(Some(std::path::PathBuf::from("runtime.json")))
        );
        assert_eq!(
            results_target(&argv(&["--results=runtime.json"])),
            Ok(Some(std::path::PathBuf::from("runtime.json")))
        );
        assert!(results_target(&argv(&["--results"])).is_err());
        assert!(results_target(&argv(&["--results", "--host", "10.0.0.4:8080"])).is_err());
        assert!(results_target(&argv(&["--results="])).is_err());
    }

    const LIMITED_BODY: &str = r#"{
      "robot_slug": "limited-arm",
      "base": "fixed",
      "dof": 1,
      "envelope": { "joint_velocity_max_rad_s": 1.0, "joint_torque_max_nm": 80,
                    "watchdog_ms": 100, "source": "vendor datasheet rev C" },
      "joints": [{"name": "shoulder", "role": "right_arm.shoulder.pitch", "type": "revolute",
                  "control_modes": ["position", "torque"],
                  "limits": {"effort_max_nm": 80, "velocity_max_rad_s": 1.0}}]
    }"#;

    const READY_BODY: &str = r#"{
      "robot_slug": "ready-arm",
      "base": "fixed",
      "dof": 2,
      "capabilities": ["safety.estop", "platform.onboard_compute"],
      "control": {"modes": ["position", "torque"], "rate_hz_max": 500},
      "envelope": { "joint_velocity_max_rad_s": 3.0, "joint_torque_max_nm": 100.0,
                    "watchdog_ms": 50, "estop": true, "source": "vendor datasheet rev C" },
      "joints": [{"name": "shoulder", "role": "right_arm.shoulder.pitch", "type": "revolute",
                  "control_modes": ["position", "torque"],
                  "limits": {"effort_max_nm": 100, "velocity_max_rad_s": 3.0}}]
    }"#;

    fn readiness_json(text: &str) -> Json {
        let (report, simulated) =
            evaluate_descriptor_text(text, "body.json").expect("the fixture evaluates");
        let mut document = skynet_contracts::json::readiness_to_json(&report);
        document.set("simulator", Json::Bool(simulated));
        document
    }

    #[test]
    fn readiness_formats_a_report_with_its_blockers_and_the_load_bearing_sentence() {
        let text = format_readiness(&readiness_json(LIMITED_BODY));
        assert!(
            text.contains("readiness limited-arm — could attempt nothing yet"),
            "{text}"
        );
        assert!(text.contains("control_modes (blocks guest)"), "{text}");
        assert!(text.contains("estop_software (blocks guest)"), "{text}");
        assert!(
            text.contains("No control modes declared, so no policy can be matched to it."),
            "{text}"
        );
        // The sentence that keeps "could attempt host" from being read as a tier.
        assert!(
            text.contains("attainable means the paperwork is in order"),
            "{text}"
        );
        // `completeness` is for sorting a fleet view, and the struct says not to quote it.
        assert!(!text.contains("completeness"), "{text}");
    }

    #[test]
    fn a_clear_report_says_so() {
        let text = format_readiness(&readiness_json(READY_BODY));
        assert!(text.contains("could attempt certified"), "{text}");
        assert!(
            text.contains("no blockers — every declaration is in order"),
            "{text}"
        );
    }

    #[test]
    fn a_fleet_counts_by_tier_and_ranks_blockers() {
        use skynet_contracts::readiness::summarize_readiness;

        let (ready, _) = evaluate_descriptor_text(READY_BODY, "ready.json").unwrap();
        let (limited, _) = evaluate_descriptor_text(LIMITED_BODY, "limited.json").unwrap();
        let fleet = summarize_readiness(&[ready, limited]);
        let text = format_fleet(&skynet_contracts::json::fleet_to_json(&fleet));
        assert!(text.contains("fleet of 2 robots"), "{text}");
        assert!(
            text.contains("none 1, guest 0, host 0, certified 1"),
            "{text}"
        );
        // First-encounter order at equal counts: the limited body's to-do list, top to bottom.
        let modes = text
            .find("control_modes — 1 robot")
            .expect("the top blocker");
        let software = text.find("estop_software — 1 robot").expect("the next one");
        assert!(modes < software, "{text}");
        assert!(text.contains("onboard_compute — 1 robot"), "{text}");
    }

    #[test]
    fn a_fleet_names_how_many_of_it_is_simulated() {
        use skynet_contracts::readiness::summarize_readiness;

        let (ready, _) = evaluate_descriptor_text(READY_BODY, "ready.json").unwrap();
        let fleet = summarize_readiness(&[ready]);
        let mut document = skynet_contracts::json::fleet_to_json(&fleet);
        document.set("simulated", Json::from(1.0));
        let text = format_fleet(&document);
        assert!(
            text.contains("including 1 simulated (awards cap at guest §35.3)"),
            "{text}"
        );

        let mut document = skynet_contracts::json::fleet_to_json(&fleet);
        document.set("simulated", Json::from(0.0));
        assert!(!format_fleet(&document).contains("simulated"));
    }

    #[test]
    fn a_text_that_is_not_a_descriptor_is_refused_by_name() {
        let error = evaluate_descriptor_text("not json [", "a.json").unwrap_err();
        assert!(error.contains("a.json"), "{error}");
        assert!(error.contains("not JSON"), "{error}");
        let error = evaluate_descriptor_text(r#"{"joints": [{}]}"#, "b.json").unwrap_err();
        assert!(error.contains("b.json"), "{error}");
        assert!(error.contains("not a descriptor"), "{error}");
    }

    #[test]
    fn a_simulated_body_prints_the_cap_next_to_the_tier_it_qualifies() {
        let mut document = Json::parse(READY_BODY).unwrap();
        document.set("simulator", Json::Bool(true));
        let text = document.to_canonical().unwrap();
        let rendered = format_readiness(&readiness_json(&text));
        assert!(
            rendered.contains("could attempt certified (simulated — awards cap at guest §35.3)"),
            "{rendered}"
        );

        // And a body attaining nothing has blockers instead of a cap it cannot reach.
        let mut limited = Json::parse(LIMITED_BODY).unwrap();
        limited.set("simulator", Json::Bool(true));
        let text = limited.to_canonical().unwrap();
        let rendered = format_readiness(&readiness_json(&text));
        assert!(rendered.contains("could attempt nothing yet"), "{rendered}");
        assert!(!rendered.contains("awards cap"), "{rendered}");
    }

    #[test]
    fn a_drafted_file_counts_no_adapter() {
        let mut document = Json::parse(READY_BODY).unwrap();
        let mut provenance = Json::obj();
        provenance.set(
            "tool",
            Json::from(skynet_contracts::readiness::URDF_DRAFT_TOOL),
        );
        document.set("provenance", provenance);
        let text = document.to_canonical().unwrap();
        let (report, _) = evaluate_descriptor_text(&text, "draft.json").unwrap();
        assert_eq!(report.attainable, None);
        assert!(report
            .blockers
            .iter()
            .any(|blocker| blocker.check == "adapter"));
        let rendered = format_readiness(&skynet_contracts::json::readiness_to_json(&report));
        assert!(rendered.contains("adapter (blocks guest)"), "{rendered}");
    }

    #[test]
    fn the_usage_names_every_command_this_module_dispatches() {
        for command in [
            "status",
            "ps",
            "logs",
            "plan",
            "grants",
            "estop",
            "rollback",
            "install",
            "blackbox",
            "readiness",
        ] {
            assert!(USAGE.contains(command), "usage should name `{command}`");
        }
    }

    /// The permission case is the one a new user meets first, and it used to be reported as a daemon
    /// that might not be running. Each cause names its own next step, and none of them sends
    /// somebody to look for a problem that is not there.
    #[cfg(unix)]
    #[test]
    fn an_unreachable_socket_says_which_of_the_common_causes_it_is() {
        use std::io::{Error, ErrorKind};
        let path = std::path::Path::new("/run/skynetd.sock");

        let denied = unreachable_socket(path, &Error::from(ErrorKind::PermissionDenied));
        assert!(denied.contains("sudo"), "{denied}");
        assert!(!denied.contains("Is skynetd running"), "{denied}");

        let missing = unreachable_socket(path, &Error::from(ErrorKind::NotFound));
        assert!(missing.contains("systemctl status skynetd"), "{missing}");

        let stale = unreachable_socket(path, &Error::from(ErrorKind::ConnectionRefused));
        assert!(stale.contains("nothing is listening"), "{stale}");
    }
}

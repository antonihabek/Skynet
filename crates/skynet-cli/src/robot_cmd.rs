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
  conformance                 §15's eight runtime tests, run by this robot on itself
  grants                      What is granted, and what is leased
  grants add <ns/name> <token> [--acknowledge <digest>]
                              A skill's safety notes are shown once and acknowledged (§54.2)
  grants remove <ns/name> <token>
  estop                       Stop every package; needs no credential (§39.1)
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

fn flag(argv: &[String], name: &str) -> Option<String> {
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
            // A `--flag value` pair consumes two; `--flag=value` consumes one.
            if !arg.contains('=') {
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
        "state" => show(argv, "GET", "/state", None, print_state),
        "shutdown" => show(argv, "POST", "/shutdown", Some(String::new()), print_note),
        "limits" => limits(argv),
        "conformance" => show(argv, "GET", "/conformance", None, print_conformance),
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
            "skynet skill run <ns/name>, or skynet skill stop <inv-id>.\n\n{USAGE}"
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
            term::dim("this robot's desired state names no OS version, so there is no release to                        ask about.")
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
    term::fail(&format!("the robot answered {status}: {detail}"))
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
            for (name, key) in [
                ("--velocity", "joint_velocity_max_rad_s"),
                ("--torque", "joint_torque_max_nm"),
                ("--base", "base_velocity_max_m_s"),
                ("--jerk", "max_delta_per_cycle"),
            ] {
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
    for warning in plan.arr_or_empty("warnings") {
        println!(
            "  {} {} {}",
            term::bold("!"),
            text(warning, "code"),
            term::dim(&text(warning, "message")),
        );
    }
}

/// `skynet conformance` (§15).
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
                    "— a floor: §36.2 also counts {}, and nothing on this robot supplies either",
                    unaccounted.join(" and ")
                ))
            ),
            None => println!(
                "  {}",
                term::dim("no reserve — this body declares no `safe_stop_energy_wh` (§36.2)")
            ),
        }
    }

    let shed = document.str_arr("shed");
    let thermal = text("thermal");
    if shed.is_empty() {
        println!("  thermal {thermal}");
    } else {
        println!("  thermal {thermal} — giving up {}", shed.join(", "));
    }

    // Three booleans an operator acts on, printed only when they say something. A line reading
    // "new leases: yes" on every healthy robot is a line nobody reads on the one robot where it
    // says no.
    if !flag("grants_new_leases") {
        println!("  {}", term::bold("no new task leases (§36.1)"));
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

    let workspace = document.get_present("workspace");
    let volumes = workspace
        .map(|layer| layer.arr_or_empty("volumes").len())
        .unwrap_or(0);
    println!(
        "  {}",
        term::dim(&format!(
            "{volumes} allowed volume(s) — computed, and not applied: the workspace guard is the              kernel's (§33.1)"
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
                    "{pending} window(s), {} bytes, are on this robot and nowhere else.                      `skynet blackbox get <name>` reads one here; the daemon sends them when an                      unmetered link is up (§34.3, §79.3).",
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
            let mut stream = std::os::unix::net::UnixStream::connect(&path).map_err(|error| {
                format!(
                    "could not reach {}: {error}\n  Is skynetd running? Is this the robot?",
                    path.display()
                )
            })?;
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

    #[test]
    fn a_package_id_becomes_the_colon_form_a_route_uses() {
        assert_eq!(
            route_id("openrobot/grasp").as_deref(),
            Some("openrobot:grasp")
        );
        assert_eq!(route_id("../../etc"), None);
        assert_eq!(route_id("openrobot"), None);
    }

    #[test]
    fn the_usage_names_every_command_this_module_dispatches() {
        for command in [
            "status", "ps", "logs", "plan", "grants", "estop", "rollback", "install", "blackbox",
        ] {
            assert!(USAGE.contains(command), "usage should name `{command}`");
        }
    }
}

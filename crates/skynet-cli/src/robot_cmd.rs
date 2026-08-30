//! The commands that talk to a running robot (§18, §33.7).
//!
//! ```text
//! skynet status                      what this robot is, and whether stop is reachable
//! skynet ps                          installed packages, their state and their grants
//! skynet logs <ns/name>              the package's journal — not the black box (§41.1)
//! skynet plan                        the last reconcile pass, including anything deferred
//! skynet grants                      what is granted, and what is leased
//! skynet grants add <ns/name> <token>
//! skynet grants remove <ns/name> <token>
//! skynet estop                       stop every package, now
//! skynet install <dir> --id … --kind … --version … --digest …
//! skynet rollback <ns/name>          the previous version, in milliseconds
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
  ps                          Installed packages, their state, their grants
  logs <ns/name> [--lines N]  The package's journal
  plan                        The last reconcile pass, and anything deferred
  grants                      What is granted, and what is leased
  grants add <ns/name> <token>
  grants remove <ns/name> <token>
  estop                       Stop every package; needs no credential (§39.1)
  rollback <ns/name>          Back to the previous version (§37.2)
  install <dir> --id <ns/name> --kind <k> --version <v> --digest <hex>
  blackbox                    The recording's window, and what is missing from it
  blackbox seal               Seal the window now (§13); needs operator
  blackbox get <name>         Download one sealed recording, as MCAP

  --socket <path>   Default /run/skynetd.sock
  --host <addr:port>  The LAN console instead of the socket
  --token <t>       Or $SKYNET_TOKEN. Not needed for `status` or `estop`.
  --root <dir>      For `install`: where /data lives. Default `/`.";

/// Where to send a request.
enum Target {
    /// The local socket.
    Socket(std::path::PathBuf),
    /// The LAN console.
    Host(String),
}

fn target(argv: &[String]) -> Target {
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
        "rollback" => rollback(argv),
        "install" => install(argv),
        "blackbox" => blackbox(argv),
        other => term::fail(&format!("`{other}` is not a robot command.\n\n{USAGE}")),
    }
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
            let (method, path, body) = if adding {
                let mut document = Json::obj();
                document
                    .set("id", Json::from(id.as_str()))
                    .set("token", Json::from(token.as_str()));
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
        Some(other) => term::fail(&format!(
            "`blackbox {other}` is not a subcommand. They are `index`, `seal` and `get`."
        )),
    }
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

    let sealed = document.arr_or_empty("sealed");
    if !sealed.is_empty() {
        println!();
        println!("sealed:");
        for entry in sealed {
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

fn request(
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

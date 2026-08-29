//! `skynetd` — the supervisor.
//!
//! ```text
//! skynetd --root /            run the reconcile loop and serve the local API
//! skynetd once --root /       one pass, printed, then exit
//! skynetd stage <dir> --id ns/name --kind policy --version 1.0.0 --digest <hex>
//! ```
//!
//! `once` is the one worth explaining. §33.3's loop is idempotent by construction, so running
//! exactly one pass and printing what it did is a legitimate way to operate a robot as well as to
//! debug one — an operator can see the plan, the outcomes, and everything deferred, without leaving
//! a daemon running.
//!
//! # `WatchdogSec`, and pinging it from inside the loop
//!
//! > **Hang** — `WatchdogSec` in the unit file. `skynetd` pings it from the reconcile loop, so a
//! > hung loop — not merely a hung process — trips it.
//!
//! [`notify_watchdog`] sends `WATCHDOG=1` to `$NOTIFY_SOCKET`, and it is called from inside the
//! loop body rather than from a timer beside it. A ping on its own thread would keep answering
//! while the thing it is vouching for is wedged, which is the failure it exists to catch.

use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// The serving half only. `once` is portable; the loop that binds a socket and paces itself is not.
#[cfg(unix)]
use std::net::SocketAddr;
#[cfg(unix)]
use std::time::Duration;

use skynet_json::Json;
use skynetd_io::api::Context;
use skynetd_io::auth::Credentials;
use skynetd_io::launch::SystemdLauncher;
use skynetd_io::layout::{Layout, PackageId};
use skynetd_io::observe::RaucSlots;
use skynetd_io::runtime::Daemon;
use skynetd_io::store::Store;

const USAGE: &str = "\
skynetd — the Skynet OS supervisor

  skynetd [--root <dir>] [--lan <addr:port>] [--interval <ms>]
  skynetd once [--root <dir>]
  skynetd stage <dir> --id <ns/name> --kind <kind> --version <v> --digest <hex>
  skynetd credential add --label <name> --role observer|operator|admin

  --root       Where /data and /run live. Default `/`.
  --lan        Also serve the console on a link-local address. Refused if it is routable (§34.2).
  --interval   Reconcile period in milliseconds. Default 10000.
  --user       Use the calling user's systemd manager rather than the system one.

  The local socket is always served, at <root>/run/skynetd.sock.";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&argv));
}

fn run(argv: &[String]) -> i32 {
    if argv.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return 0;
    }

    match argv.first().map(String::as_str) {
        Some("credential") => credential(&argv[1..]),
        Some("stage") => stage(&argv[1..]),
        Some("once") => loop_or_once(&argv[1..], true),
        _ => loop_or_once(argv, false),
    }
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

fn root_of(argv: &[String]) -> Layout {
    Layout::new(flag(argv, "--root").unwrap_or_else(|| "/".to_string()))
}

/// Monotonic milliseconds since this process started.
///
/// Monotonic and not wall time, per §35.1: no control decision reads a clock that an NTP step can
/// move. Reconcile deadlines and backoff are control decisions in the sense that matters here — a
/// clock that stepped backwards would freeze a package in backoff for however long the step was.
fn monotonic(origin: Instant) -> u64 {
    origin.elapsed().as_millis() as u64
}

fn loop_or_once(argv: &[String], once: bool) -> i32 {
    let layout = root_of(argv);
    let store = Store::new(layout.clone());
    if let Err(error) = store.prepare() {
        eprintln!("skynetd: {error}");
        return 1;
    }

    let launcher = if argv.iter().any(|a| a == "--user") {
        SystemdLauncher::user()
    } else {
        SystemdLauncher::system()
    };

    let mut daemon = Daemon::new(store, launcher, RaucSlots);
    daemon.reload();
    daemon.refresh_safe_state();
    eprintln!(
        "skynetd: safe state is declared, not measured — `{}` says {}. There is no kernel channel \
         to ask (§33.1), so §33.4's barrier is standing on an operator's word.",
        layout.safe_state().display(),
        if daemon.safe_state.is_safe() {
            "yes"
        } else {
            "no"
        }
    );
    if let Some(reason) = daemon.desired_source.because() {
        // Reported at every start, because §33.8's ladder is exactly the thing an operator needs to
        // know they are standing on. A robot silently running the last-known-good desired state is
        // a robot whose configuration changes are not taking effect and nobody knows why.
        eprintln!(
            "skynetd: desired state came from the {} copy: {reason}",
            daemon.desired_source.as_str()
        );
    }

    // The black box (§13, §33.2). Opened before anything is reconciled, so the first pass's actions
    // are inside the recording rather than before it.
    //
    // A shape of zero joints and zero channels, because this robot has no control loop: the policy
    // runtime and observation assembly (§25) are unwritten, so there are no cycles to record and
    // claiming a shape would size every record for arrays nothing fills. The ring still records
    // §26's event channel, which is what a supervisor produces.
    let mut recorder = match skynetd_io::blackbox::Recorder::open(
        &layout,
        skynetd_io::blackbox::supervisor_meta(&layout),
        skynet_blackbox::record::Shape {
            joints: 0,
            channels: 0,
        },
        skynetd_io::blackbox::DEFAULT_BUDGET,
    ) {
        Ok(recorder) => {
            eprintln!(
                "skynetd: black box at {}, {} records of {} bytes. Events only — §26's four cycle \
                 channels need a control loop, and this robot has none.",
                layout.blackbox().display(),
                recorder.geometry().capacity,
                recorder.geometry().record_size,
            );
            Some(recorder)
        }
        Err(failure) => {
            // Reported, not fatal. §28 preallocates the ring so a full disk cannot silence the
            // recorder; a robot that cannot open one at all still needs to serve `POST /stop`.
            eprintln!("skynetd: no black box: {failure}");
            None
        }
    };
    if let Some(recorder) = recorder.as_mut() {
        let _ = recorder.event(
            0,
            skynet_blackbox::record::EventCode::RecordingStarted,
            "skynetd",
            "the supervisor started",
        );
    }

    let credentials = read_credentials(&layout);
    if !credentials.is_provisioned() {
        eprintln!(
            "skynetd: no credentials. §39.2: there is no default password, so the console offers \
             status and stop and nothing else until this robot is provisioned."
        );
    }

    let origin = Instant::now();
    let started_ms = monotonic(origin);

    if once {
        let context = Context {
            daemon: &mut daemon,
            credentials: &credentials,
            now_ms: monotonic(origin),
            started_ms,
            blackbox: recorder.as_mut(),
            mono_ns: monotonic(origin).saturating_mul(1_000_000),
        };
        context.daemon.refresh_safe_state();
        let passes = context.daemon.converge(context.now_ms);
        for pass in &passes {
            print_pass(pass);
        }
        return i32::from(!passes.last().is_some_and(|pass| pass.converged()));
    }

    // Everything from here needs POSIX: a Unix socket to serve the local API on, and a systemd
    // manager to launch anything. `once` above does not — it is `plan` and `apply` over a directory
    // — which is why the split is here rather than around the whole binary. A developer can run a
    // pass against a fixture root on any machine; nobody can run the daemon on one.
    #[cfg(not(unix))]
    {
        eprintln!("skynetd: the daemon serves its API on a Unix socket and launches packages");
        eprintln!("through systemd, neither of which exists here. A robot runs Linux (§23.5).");
        eprintln!("`skynetd once --root <dir>` works anywhere and is the useful thing here.");
        // Needless only on this platform, where everything below is compiled out and this becomes
        // the tail expression. On the platform the daemon actually runs on it is load-bearing.
        #[allow(clippy::needless_return)]
        return 1;
    }

    #[cfg(unix)]
    {
        let socket = layout.socket();
        let local = match skynetd_io::serve::Local::bind(&socket) {
            Ok(local) => local,
            Err(error) => {
                eprintln!("skynetd: could not bind {}: {error}", socket.display());
                return 1;
            }
        };
        println!("skynetd: local API on {}", socket.display());

        let lan = match flag(argv, "--lan") {
            None => None,
            Some(text) => match text.parse::<SocketAddr>() {
                Err(error) => {
                    eprintln!("skynetd: --lan {text} is not an address: {error}");
                    return 1;
                }
                Ok(address) => match skynetd_io::serve::Lan::bind(address) {
                    Ok(lan) => {
                        println!(
                            "skynetd: console on {}",
                            lan.local_addr()
                                .map(|a| a.to_string())
                                .unwrap_or_else(|_| text.clone())
                        );
                        Some(lan)
                    }
                    Err(error) => {
                        eprintln!("skynetd: {error}");
                        return 1;
                    }
                },
            },
        };

        let interval = flag(argv, "--interval")
            .and_then(|text| text.parse::<u64>().ok())
            .unwrap_or(10_000);

        // One thread, one loop, and the API is polled between passes. That is the whole concurrency
        // model: §33.2 lists several long-lived tasks and this edition has two of them, so a scheduler
        // would be machinery in front of a reconcile and an accept. The listeners are non-blocking, so
        // a quiet API costs a syscall per pass rather than a thread.
        let _ = local.listener().set_nonblocking(true);
        if let Some(lan) = &lan {
            let _ = lan.listener().set_nonblocking(true);
        }

        let mut next_reconcile = 0u64;
        loop {
            let now = monotonic(origin);
            if now >= next_reconcile {
                let context = Context {
                    daemon: &mut daemon,
                    credentials: &credentials,
                    now_ms: now,
                    started_ms,
                    blackbox: None,
                    mono_ns: now.saturating_mul(1_000_000),
                };
                context.daemon.refresh_safe_state();
                let passes = context.daemon.converge(now);
                if let Some(recorder) = recorder.as_mut() {
                    for pass in &passes {
                        record_pass(recorder, now.saturating_mul(1_000_000), pass);
                    }
                }
                for (action, reason) in passes
                    .last()
                    .map(|pass| pass.deferred.as_slice())
                    .unwrap_or(&[])
                {
                    eprintln!(
                        "skynetd: deferred {} {}: {reason}",
                        skynetd_io::api::action_name(&action.kind),
                        action.subject()
                    );
                }
                next_reconcile = now.saturating_add(interval);
                // Inside the loop body, on purpose. See the module documentation.
                notify_watchdog();
            }

            let mut served = false;
            for stream in local.listener().incoming() {
                match stream {
                    Ok(stream) => {
                        let now = monotonic(origin);
                        let mut context = Context {
                            daemon: &mut daemon,
                            credentials: &credentials,
                            now_ms: now,
                            started_ms,
                            blackbox: recorder.as_mut(),
                            mono_ns: now.saturating_mul(1_000_000),
                        };
                        let _ = stream.set_nonblocking(false);
                        skynetd_io::serve::serve_one(stream, &mut context);
                        served = true;
                    }
                    Err(_) => break,
                }
            }
            if let Some(lan) = &lan {
                for stream in lan.listener().incoming() {
                    match stream {
                        Ok(stream) => {
                            let now = monotonic(origin);
                            let mut context = Context {
                                daemon: &mut daemon,
                                credentials: &credentials,
                                now_ms: now,
                                started_ms,
                                blackbox: recorder.as_mut(),
                                mono_ns: now.saturating_mul(1_000_000),
                            };
                            let _ = stream.set_nonblocking(false);
                            skynetd_io::serve::serve_one(stream, &mut context);
                            served = true;
                        }
                        Err(_) => break,
                    }
                }
            }

            if !served {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// `skynetd credential add` — issue a local credential (§39.2).
///
/// > Bootstrapping uses physical presence — a chassis button held during pairing, or a QR code
/// > printed on the robot — because possession of the machine is the only credential that means
/// > anything before any other credential exists.
///
/// This is that bootstrap, in the form a machine without a chassis button has: you are on the robot,
/// you can write `/data/state`, and that is the presence. It is **not** pairing, which binds the
/// robot to a Hub namespace (§10) and needs an attestation path nobody has built.
///
/// The token is printed once and never stored — only a salted hash is. There is no command to read
/// it back, because a credential store you can read is a credential store an operator will read.
fn credential(argv: &[String]) -> i32 {
    let layout = root_of(argv);
    if argv.first().map(String::as_str) != Some("add") {
        eprintln!("skynetd credential add --label <name> --role observer|operator|admin");
        return 1;
    }
    let label = flag(argv, "--label").unwrap_or_else(|| "unnamed".to_string());
    let Some(role) = flag(argv, "--role").and_then(|text| skynetd_io::auth::Role::parse(&text))
    else {
        eprintln!("skynetd: --role must be observer, operator or admin");
        return 1;
    };
    if role == skynetd_io::auth::Role::Anonymous {
        // Issuing a credential that grants what everybody already has is a credential whose only
        // effect is to make somebody believe they have one.
        eprintln!("skynetd: `anonymous` is what an unauthenticated caller already has (§39.1)");
        return 1;
    }

    let (Ok(salt), Ok(token)) = (
        skynetd_io::auth::random_hex(16),
        skynetd_io::auth::random_hex(32),
    ) else {
        eprintln!(
            "skynetd: could not read /dev/urandom. No credential was issued — a weak token is \
             worse than none (§39.2)."
        );
        return 1;
    };

    let mut credentials = read_credentials(&layout);
    credentials.add(skynetd_io::auth::Credential::issue(
        &label, role, &salt, &token,
    ));
    let document = credentials.to_json();
    let Ok(text) = document.to_canonical() else {
        eprintln!("skynetd: the credentials file could not be serialised");
        return 1;
    };
    if let Err(error) = skynetd_io::observe::write_atomic(&layout.credentials(), text.as_bytes()) {
        eprintln!("skynetd: could not write the credentials file: {error}");
        return 1;
    }

    println!("{token}");
    eprintln!(
        "skynetd: issued a {} credential for `{label}`. This is the only time the token is \
         printed — nothing stores it, only a salted hash.",
        role.as_str()
    );
    0
}

fn stage(argv: &[String]) -> i32 {
    let layout = root_of(argv);
    let store = Store::new(layout);
    if let Err(error) = store.prepare() {
        eprintln!("skynetd: {error}");
        return 1;
    }

    let (Some(source), Some(id), Some(kind), Some(version), Some(digest)) = (
        argv.first().filter(|a| !a.starts_with("--")).cloned(),
        flag(argv, "--id"),
        flag(argv, "--kind"),
        flag(argv, "--version"),
        flag(argv, "--digest"),
    ) else {
        eprintln!("{USAGE}");
        return 1;
    };

    let id = match PackageId::parse(&id) {
        Ok(id) => id,
        Err(error) => {
            eprintln!("skynetd: {error}");
            return 1;
        }
    };

    match store.stage(&id, &kind, &version, &digest, &PathBuf::from(source)) {
        Ok(staged) => {
            println!("staged {} {}", staged.id, staged.digest);
            println!("signature: not checked — this edition has no Ed25519 (§37.1)");
            0
        }
        Err(error) => {
            eprintln!("skynetd: {error}");
            1
        }
    }
}

fn read_credentials(layout: &Layout) -> Credentials {
    let Ok(text) = std::fs::read_to_string(layout.credentials()) else {
        return Credentials::empty();
    };
    let Ok(document) = Json::parse(&text) else {
        // A credentials file that does not parse is not an empty one, and it is also not a reason
        // to open the robot up. It produces no credentials, which is the same behaviour as never
        // having been provisioned, and it says so.
        eprintln!(
            "skynetd: the credentials file does not parse; treating this robot as unprovisioned"
        );
        return Credentials::empty();
    };
    Credentials::from_json(&document)
}

fn print_pass(pass: &skynetd_io::runtime::Pass) {
    println!("plan: {} action(s)", pass.plan.actions.len());
    for (action, outcome) in &pass.applied {
        println!(
            "  {} {} → {}",
            skynetd_io::api::action_name(&action.kind),
            action.subject(),
            outcome.as_str()
        );
        match outcome {
            skynetd_io::runtime::Outcome::Failed { reason }
            | skynetd_io::runtime::Outcome::Unimplemented { reason } => {
                println!("      {reason}");
            }
            _ => {}
        }
    }
    for (action, reason) in &pass.deferred {
        println!(
            "  {} {} → deferred",
            skynetd_io::api::action_name(&action.kind),
            action.subject()
        );
        println!("      {reason}");
    }
    println!("converged: {}", if pass.converged() { "yes" } else { "no" });
}

/// Record what a reconcile pass did, on §26's event channel.
///
/// Applied actions only. A *planned* action is an intention and the black box is a record of what
/// happened — §41.1 keeps the two apart, and a recording full of things the robot meant to do is the
/// log file the black box is explicitly not.
///
/// A failed action is recorded too, with its reason. "The supervisor tried to start this and could
/// not" is exactly the fact an incident review is looking for, and a recorder that kept only the
/// successes would produce a file in which nothing ever went wrong.
#[cfg(unix)]
fn record_pass(
    recorder: &mut skynetd_io::blackbox::Recorder,
    mono_ns: u64,
    pass: &skynetd_io::runtime::Pass,
) {
    use skynet_blackbox::record::EventCode;
    use skynetd::reconcile::ActionKind as K;

    for (action, outcome) in &pass.applied {
        let code = match action.kind {
            K::StartPackage { .. } => EventCode::PackageStarted,
            K::StopPackage { .. } => EventCode::PackageStopped,
            K::GrantCapability { .. } | K::RevokeCapability { .. } => EventCode::GrantChanged,
            K::StageOsImage { .. } | K::ApplyOsImage { .. } => EventCode::Ota,
            // Everything else — activating, fetching, removing, applying a profile — is package
            // lifecycle rather than one of §26's named events. Recording it as a start or a stop
            // would misreport it, and inventing a code for each would make the vocabulary open,
            // which §41.1 is the reason it is closed.
            _ => continue,
        };
        let detail = match outcome {
            skynetd_io::runtime::Outcome::Failed { reason }
            | skynetd_io::runtime::Outcome::Unimplemented { reason } => format!(
                "{} {}: {reason}",
                skynetd_io::api::action_name(&action.kind),
                outcome.as_str()
            ),
            _ => format!(
                "{} {}",
                skynetd_io::api::action_name(&action.kind),
                outcome.as_str()
            ),
        };
        let _ = recorder.event(mono_ns, code, action.subject(), &detail);
    }
}

/// Tell systemd the loop is alive (§33.8).
///
/// A no-op when `$NOTIFY_SOCKET` is unset, which is every case except running under a unit with
/// `WatchdogSec=`. Failing quietly is right here and almost nowhere else in this codebase: a
/// watchdog ping that could not be delivered is a watchdog that will fire, which is the behaviour
/// being asked for.
///
/// Called only from the reconcile loop, which is POSIX-only, so the function is too rather than
/// carrying a body that compiles to nothing on a platform that never calls it.
#[cfg(unix)]
fn notify_watchdog() {
    use std::os::unix::net::UnixDatagram;

    let Ok(path) = std::env::var("NOTIFY_SOCKET") else {
        return;
    };
    let Ok(socket) = UnixDatagram::unbound() else {
        return;
    };
    // A leading `@` is systemd's spelling of an abstract socket, whose name begins with a NUL.
    let target = if let Some(rest) = path.strip_prefix('@') {
        let mut bytes = vec![0u8];
        bytes.extend_from_slice(rest.as_bytes());
        match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => return,
        }
    } else {
        path
    };
    let _ = socket.send_to(b"WATCHDOG=1", target);
}

/// Wall time, for a log line and nothing else (§35.1).
///
/// Never used for a deadline, a backoff or a watchdog. Present so a line in a journal can be
/// correlated with the Hub, which is the one job §35.1 gives `CLOCK_REALTIME`.
#[allow(dead_code)]
fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

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
// Only the socket loop builds one now: `once` reconciles and records through
// `converge_and_record`, which needs the daemon rather than a request context.
#[cfg(unix)]
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
              [--signed-by <64 hex key>] [--signature <128 hex>] [--allow-retarget]
  skynetd credential add --label <name> --role observer|operator|admin
  skynetd calibration
  skynetd trust list
  skynetd trust add --key <64 hex> --label <name> [--note <text>] [--by <who>]
  skynetd trust remove (--label <name> | --key <64 hex> | --at keys[<n>]) [--by <who>]

  --root       Where /data and /run live. Default `/`.
  --lan        Also serve the console on a link-local address. Refused if it is routable (§34.2).
  --interval   Reconcile period in milliseconds. Default 10000.
  --user       Use the calling user's systemd manager rather than the system one.
  --signed-by  The public key that signed this version's digest, and
  --signature  the signature it made. Both or neither; a signature with no key names nobody.
               Checked against <root>/data/state/trust.json (§19, §37.1).
  --by         Who is making this change, for the audit log (§38.3). Omitted is recorded as
               unattributed rather than guessed from the environment — a name nobody typed is
               a provenance nobody wrote.

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
        Some("trust") => trust(&argv[1..]),
        Some("calibration") => calibration(&argv[1..]),
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
    observe_state(&mut daemon);
    eprintln!(
        "skynetd: §27 state is `{}`, because {}. §33.4's barrier is `{}` says {}, narrowed by that \
         state — and the state rests on one of §27's eight conditions, because the other seven are \
         the kernel's and there is no channel to ask (§33.1).",
        daemon.operating_state.as_str(),
        // The reason as well as the state. `safe` on its own reads as a robot somebody stopped
        // rather than as a robot nothing has observed, and those send an operator to opposite ends
        // of the building.
        daemon.state_reason.as_str(),
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

    // Where outbound items go (§34), and which of the two honest refusals this robot is entitled
    // to. Read from the identity on disk rather than assumed: an unpaired robot is fixed by an
    // operator in a minute and a paired one is waiting on §34.2's sending half, and a surface that
    // showed one sentence for both would send somebody to pair a robot that already is.
    //
    // `--outbox <dir>` swaps in a directory on this machine. It is not a transport and is never
    // presented as one; it exists so the drain path is exercised by something, and so that somebody
    // bringing a robot up can watch the queue actually empty.
    let sink: Box<dyn skynetd_io::connectivity::Sink> = match flag(argv, "--outbox") {
        Some(path) => Box::new(skynetd_io::connectivity::Directory {
            path: PathBuf::from(path),
        }),
        None => match skynetd_io::identity::credential_for_sending(&layout) {
            Some((endpoint, device_id, credential)) => Box::new(skynetd_io::connectivity::Hub {
                endpoint,
                device_id,
                credential,
                scratch: layout.state(),
            }),
            // No credential, no device id, or no endpoint. Any of the three means there is nothing
            // to address a report to, and §34.4's queue holds rather than growing quietly.
            None => Box::new(skynetd_io::connectivity::Unpaired),
        },
    };
    // §79's uploader. `curl` for the same reason every other outbound path uses it, and a
    // refusing one where there is nothing to send to — which is not a degraded mode: an unpaired
    // robot has no endpoint, and a window offered to nobody is a transfer with no address.
    let mut uploader: Box<dyn skynetd_io::upload::Uploader> =
        Box::new(skynetd_io::upload::CurlUploader);

    let sink_description = sink.describe();
    let sink_obstacle = sink.obstacle();
    eprintln!("skynetd: outbound goes to {sink_description}");
    if let Some(obstacle) = &sink_obstacle {
        // Said at every start, beside the sink rather than after the first failed drain. A robot
        // that will never send anything should say so before it has a queue to fail to send.
        eprintln!("skynetd: {obstacle}");
    }

    let origin = Instant::now();
    // Only the daemon loop reports an uptime, and only on the platform that has one — `once`
    // returns before it would mean anything.
    #[cfg(unix)]
    let started_ms = monotonic(origin);

    if once {
        let now = monotonic(origin);
        observe_state(&mut daemon);
        // Through the same function the loop uses, and that is the whole of the fix. `once` used to
        // converge directly and record nothing, so a robot operated entirely through it — which
        // §33's own documentation calls "a legitimate way to operate a robot" — had a black box
        // holding `recording_started` and not one thing that happened afterwards. It was not a
        // decision; it was two call sites, one of which grew a recorder.
        // `now` as the start, because a one-shot pass *is* the whole of this process's life. On a
        // machine with /proc that reading is ignored anyway — §12's budget is a boot budget — and
        // where there is no /proc it means the fallback reports zero elapsed, which holds the boot
        // gate at `wait`. That is the safe direction: a `skynet reconcile` run should not be able
        // to roll a robot back on the strength of a clock that measured one command.
        pull_desired(&mut daemon, &layout, now);
        let passes = converge_and_record(&mut daemon, recorder.as_mut(), &layout, now, now);
        for pass in &passes {
            print_pass(pass);
        }
        // One drain, for the same reason `once` records: §33 calls it a legitimate way to operate a
        // robot, and a robot operated that way should not be one whose queue never moves. Jitter is
        // at the top of the window because a one-shot pass has nobody to be decorrelated from — the
        // thundering herd §34.4 guards against is a *fleet retrying in lockstep*, and a person
        // running one command is not one.
        report_checkin(&mut daemon, &layout, now);
        // Opens §74's window and closes nothing, which is the honest output of a single pass: a
        // window needs two passes to have an interval, and a zero-length one would be a row that
        // could produce no rate and no coverage.
        report_telemetry(&mut daemon, recorder.as_ref(), &layout, now);
        let mut sender =
            skynetd_io::connectivity::Sender::new(skynetd_io::outbound::Queue::of(&layout));
        report_drain(&mut sender, &layout, sink.as_ref(), now, 1.0);
        // §79's window, for the reason `once` drains: a robot operated entirely through single
        // passes should not be one whose sealed recordings are never collected. One slice, and
        // a large window therefore needs several runs — which is what resume is for.
        report_upload(&mut uploader, &layout, now, 1.0);
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
        let mut sender =
            skynetd_io::connectivity::Sender::new(skynetd_io::outbound::Queue::of(&layout));
        loop {
            let now = monotonic(origin);
            if now >= next_reconcile {
                // §34.4's jitter, supplied to the pass rather than drawn inside it (§35.2). The
                // fetcher backs off on the same curve as the outbound queue and for a sharper
                // reason: a staged rollout tells a whole canary ring to fetch one image at one
                // moment, so a fleet arrives at the endpoint in lockstep by design.
                daemon.jitter = jitter_for(now);
                // §11's loop: pull, then reconcile, then report. Each is the input to the next.
                pull_desired(&mut daemon, &layout, now);
                observe_state(&mut daemon);
                let passes =
                    converge_and_record(&mut daemon, recorder.as_mut(), &layout, now, started_ms);
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
                // The report, then the queue. In that order, so the item the drain picks up
                // describes the pass that just ran rather than the one before it.
                report_checkin(&mut daemon, &layout, now);
                // §74's window, closed by the same pass that opens the next one. There is no timer:
                // a robot that stops reconciling stops producing windows, and the gap is then
                // visible on the Hub as a gap — which is what §47.4 asks for and what a heartbeat
                // on a separate timer would hide.
                report_telemetry(&mut daemon, recorder.as_ref(), &layout, now);
                // §34.4's queue, drained. Bounded per pass rather than to exhaustion: a reconcile
                // loop that spent ten minutes uploading episodes would be a supervisor that stopped
                // supervising, and §33.2's watchdog would notice — correctly — that the loop had
                // hung.
                report_drain(&mut sender, &layout, sink.as_ref(), now, jitter_for(now));
                // §79's bytes, last in the pass and bounded like everything before it. Last
                // because it is the only stage that can spend twenty seconds on purpose, and
                // because the incident that names the window went out in the drain above — an
                // operator learns a fault happened before the robot starts paying to describe it
                // (§79.3).
                report_upload(&mut uploader, &layout, now, jitter_for(now));

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
                            outbound_sink: &sink_description,
                            outbound_obstacle: sink_obstacle.as_deref(),
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
                                outbound_sink: &sink_description,
                                outbound_obstacle: sink_obstacle.as_deref(),
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

/// `skynetd calibration` — what this robot has measured of itself, and what that costs it (§45).
///
/// Leads with the guards rather than the records, because that is the question somebody standing
/// next to a robot has: *what is this machine not checking right now, and why*. §45.5's whole
/// requirement is that an expired parameter set be named — "proximity unavailable" sends nobody
/// anywhere, and "proximity unavailable: the camera_extrinsics calibration expired 40 days ago"
/// sends them to a target board.
fn calibration(argv: &[String]) -> i32 {
    let layout = root_of(argv);
    let wall_s = Some(wall_ms() as f64 / 1000.0);
    let status = skynetd_io::calibration::status(&layout, wall_s);

    let starved = status.unavailable();
    if starved.is_empty() {
        println!("no guard is short of a calibration.");
    } else {
        println!("guards without a calibration:");
        for entry in &starved {
            println!("  {:<16} {}", entry.guard, entry.detail);
        }
    }

    println!();
    for held in &status.held {
        let validity = &held.validity;
        println!("{:<20} {}", held.set, validity.as_str());
        if let skynet_contracts::calibration::Validity::Valid { checked } = validity {
            // What was actually checked, because "still valid" and "nothing was checked" are the
            // same outcome and different facts.
            println!(
                "  {}",
                if checked.is_empty() {
                    "no expiry policy — nothing was checked".to_string()
                } else {
                    checked.join(" · ")
                }
            );
        } else if !validity.detail().is_empty() {
            println!("  {}", validity.detail());
        }
        if let Some(record) = &held.record {
            if let Some(method) = &record.method {
                println!("  method     {method}");
            }
            match &held.signature.signer {
                Some(signer) => println!("  signed by  {signer}"),
                None => println!("  signature  {}", held.signature.state.as_str()),
            }
        }
    }

    // §32.2's rule in a terminal: three of §45.4's four conditions have nothing producing them
    // here, and a list of expired records with no explanation reads as a robot that has drifted
    // rather than one that cannot measure.
    println!();
    println!(
        "Every guard above is already unavailable for a prior reason: §27's kernel and the policy\n\
         runtime are unwritten, so nothing on this robot evaluates a cycle. These are the\n\
         calibrations that would still be missing once they do."
    );
    0
}

const TRUST_USAGE: &str = "\
  skynetd trust list
  skynetd trust add --key <64 hex> --label <name> [--note <text>] [--by <who>]
  skynetd trust remove (--label <name> | --key <64 hex> | --at keys[<n>]) [--by <who>]";

/// `skynetd trust` — the audited surface §38.3 asks for, applied to keys.
///
/// Before this, keys were added by editing JSON, which works and records nothing: a store written
/// that way can say *which* signers a robot believes and never *who decided that, or when*. Those
/// are the two questions asked after an incident, and a file cannot answer them retrospectively.
///
/// Three properties this has and hand-editing does not:
///
/// * **The change is dated and attributed** — into the entry, and into the audit log beside it.
/// * **A malformed entry survives the rewrite.** The store round-trips what it cannot parse, so
///   adding a key to a file with a typo in it does not quietly delete the typo (see
///   [`skynetd_io::trust::SkippedKey`]).
/// * **A duplicate is refused rather than merged**, naming the label already holding the key.
///
/// What it deliberately does not do is guess who is running it. `--by` omitted records
/// `unattributed`, not `$USER`: an environment variable is not an identity, and a provenance
/// nobody typed is exactly the confidently-wrong record this whole feature exists to remove.
fn trust(argv: &[String]) -> i32 {
    let layout = root_of(argv);
    let audit = skynetd_io::audit::Audit::of(&layout);
    let actor = flag(argv, "--by").unwrap_or_else(|| "unattributed".to_string());
    let now = skynetd_io::audit::rfc3339(wall_ms());
    let mut store = skynetd_io::trust::TrustStore::load(&layout);

    match argv.first().map(String::as_str) {
        Some("list") => {
            trust_list(&store, &layout);
            0
        }
        Some("add") => {
            let (Some(hex), Some(label)) = (flag(argv, "--key"), flag(argv, "--label")) else {
                eprintln!("skynetd trust add --key <64 hex> --label <name>");
                return 1;
            };
            let key = match skynet_ed25519::VerifyingKey::from_hex(&hex) {
                Ok(key) => key,
                Err(error) => {
                    eprintln!("skynetd: {}", error.reason());
                    return 1;
                }
            };
            let entry = skynetd_io::trust::TrustedKey {
                label: label.clone(),
                key,
                provenance: skynetd_io::trust::Provenance {
                    added_at: Some(now.clone()),
                    added_by: flag(argv, "--by"),
                    note: flag(argv, "--note"),
                },
            };
            if let Err(error) = store.add(entry) {
                eprintln!("skynetd: {}", error.reason());
                return 1;
            }
            if !write_trust(&layout, &store) {
                return 1;
            }
            record(&audit, &now, &actor, "trust.add", &label, &hex, None);
            eprintln!(
                "skynetd: this robot now trusts `{label}`. Signatures made by that key report \
                 `verified` rather than `untrusted`."
            );
            0
        }
        Some("remove") => {
            let selector = match (
                flag(argv, "--label"),
                flag(argv, "--key"),
                flag(argv, "--at"),
            ) {
                (Some(label), _, _) => skynetd_io::trust::Selector::Label(label),
                (_, Some(hex), _) => match skynet_ed25519::VerifyingKey::from_hex(&hex) {
                    Ok(key) => skynetd_io::trust::Selector::Key(key),
                    Err(error) => {
                        eprintln!("skynetd: {}", error.reason());
                        return 1;
                    }
                },
                (_, _, Some(at)) => skynetd_io::trust::Selector::At(at),
                _ => {
                    eprintln!(
                        "skynetd trust remove --label <name> | --key <64 hex> | --at keys[<n>]"
                    );
                    return 1;
                }
            };
            let removed = match store.remove(&selector) {
                Ok(removed) => removed,
                Err(error) => {
                    eprintln!("skynetd: {}", error.reason());
                    return 1;
                }
            };
            if !write_trust(&layout, &store) {
                return 1;
            }
            match removed {
                skynetd_io::trust::Removed::Trusted(key) => {
                    record(
                        &audit,
                        &now,
                        &actor,
                        "trust.remove",
                        &key.label,
                        &key.key_hex(),
                        Some(&key.provenance.describe()),
                    );
                    eprintln!(
                        "skynetd: `{}` is no longer trusted. Anything it signed now reports \
                         `untrusted`, including packages already installed.",
                        key.label
                    );
                }
                skynetd_io::trust::Removed::Skipped(skipped) => {
                    record(
                        &audit,
                        &now,
                        &actor,
                        "trust.remove",
                        &skipped.at,
                        &skipped.detail,
                        None,
                    );
                    eprintln!(
                        "skynetd: removed {} — it was never usable: {}",
                        skipped.at, skipped.detail
                    );
                }
            }
            0
        }
        _ => {
            eprintln!("{TRUST_USAGE}");
            1
        }
    }
}

/// Write the store back, atomically.
fn write_trust(layout: &Layout, store: &skynetd_io::trust::TrustStore) -> bool {
    let Ok(text) = store.to_json().to_canonical() else {
        eprintln!("skynetd: the trust store could not be serialised");
        return false;
    };
    if let Err(error) = skynetd_io::observe::write_atomic(&layout.trust(), text.as_bytes()) {
        eprintln!("skynetd: could not write the trust store: {error}");
        return false;
    }
    true
}

/// Append to the audit log, and say so if it could not be written.
///
/// A failed append does **not** roll the change back. The key is trusted either way — the file has
/// already been written — and pretending otherwise by unwinding would leave the robot in whichever
/// state the second write happened to reach. So it reports loudly instead: the change stands, and
/// the record of it does not, which is a thing an operator must be told rather than a thing to
/// paper over.
fn record(
    audit: &skynetd_io::audit::Audit,
    at: &str,
    actor: &str,
    action: &str,
    subject: &str,
    detail: &str,
    previous: Option<&str>,
) {
    if let Err(error) = audit.append(at, actor, action, subject, detail, previous) {
        eprintln!(
            "skynetd: the change was made and could NOT be written to the audit log ({}): \
             {error}. §38.3 wants both; this robot now has one.",
            audit.path().display()
        );
    }
}

fn trust_list(store: &skynetd_io::trust::TrustStore, layout: &Layout) {
    use skynetd_io::trust::{Entry, Source};

    match store.source() {
        Source::Missing => {
            println!("no trust store at {}", layout.trust().display());
            println!("This robot trusts nobody, so every signature reports `untrusted` (§19).");
            return;
        }
        Source::Unreadable { detail } => {
            println!("the trust store at {} {detail}", layout.trust().display());
            println!("This robot trusts nobody until it is repaired.");
            return;
        }
        Source::Loaded => {}
    }

    if store.entries().is_empty() {
        println!("the trust store is present and names no keys.");
        return;
    }

    for entry in store.entries() {
        match entry {
            Entry::Trusted(key) => {
                println!("{}", key.label);
                println!("  key        {}", key.key_hex());
                println!("  {}", key.provenance.describe());
                if let Some(note) = &key.provenance.note {
                    println!("  note       {note}");
                }
            }
            Entry::Skipped(skipped) => {
                println!("{} — UNUSABLE", skipped.at);
                println!("  {}", skipped.detail);
                println!(
                    "  remove it with `skynetd trust remove --at {}`",
                    skipped.at
                );
            }
        }
    }
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

    // §6.1: *"retargeting changes what the policy is doing. That should be a decision somebody
    // made, not a fallback that happens quietly."* A flag on the command is that decision, and the
    // person running it is the somebody. The reconcile loop has no such person and passes `false`.
    let allow_retarget = argv.iter().any(|arg| arg == "--allow-retarget");

    // §37.1's `verify` step is "digest, then Ed25519 signature against a key the operator trusts",
    // and this is where the second half arrives. The two flags are read before staging so a
    // half-given signature is a usage error rather than something discovered after the bytes are
    // on disk — a signature with no key names nobody, and a key with no signature signs nothing.
    let signed = match (flag(argv, "--signed-by"), flag(argv, "--signature")) {
        (None, None) => None,
        (Some(key), Some(value)) => {
            let mut document = Json::obj();
            document
                .set("key", Json::from(key.as_str()))
                .set("value", Json::from(value.as_str()));
            match skynetd_io::trust::SignedBy::from_json(&document) {
                Ok(signed) => Some(signed),
                Err(error) => {
                    eprintln!("skynetd: {}", error.reason());
                    return 1;
                }
            }
        }
        (Some(_), None) => {
            eprintln!(
                "skynetd: --signed-by names a key and no --signature accompanies it. A key that \
                 signed nothing attests to nothing"
            );
            return 1;
        }
        (None, Some(_)) => {
            eprintln!(
                "skynetd: --signature carries a signature and no --signed-by names the key that \
                 made it. An Ed25519 signature does not say who produced it, so a robot given one \
                 without a key cannot tell a stranger from a forgery"
            );
            return 1;
        }
    };

    match store.stage(&id, &kind, &version, &digest, &PathBuf::from(source)) {
        Ok(staged) => {
            println!("staged {} {}", staged.id, staged.digest);

            // The trust store is read here rather than held, because it is a file an operator edits
            // and the next stage should see the key they just added.
            let trust = skynetd_io::trust::TrustStore::load(store.layout());
            for line in trust.notices() {
                println!("{line}");
            }
            let verdict = trust.check(
                &skynetd_io::trust::package_message(&staged.digest),
                signed.as_ref(),
            );

            // §37.1 puts admission between `verify` and `stage`. It reports rather than gates here
            // — the blob is inert until something activates it — and `skynetd_io::admit` carries
            // the argument for where the gate does live.
            let body = skynetd_io::embodiment::load(store.layout());
            let capacity = skynetd_io::capacity::read(&store, &body, skynetd_io::api::VERSION);
            let outcome = match skynet_contracts::json::manifest_from_json(&staged.manifest) {
                Ok(manifest) => {
                    let report = skynetd_io::admit::run(
                        &manifest,
                        &capacity,
                        &staged.digest,
                        verdict.into(),
                        &body,
                        allow_retarget,
                    );
                    for line in skynetd_io::admit::to_lines(&report) {
                        println!("{line}");
                    }
                    // §54.5, walked here rather than only at activation. The gate that *holds* is
                    // the symlink swap (see `runtime::composition_refusal`); this is where the
                    // person who can fix it is standing, and a publisher finding out at install
                    // is the whole of what the rule asks for.
                    let package_id = staged.id.as_str();
                    let composition =
                        skynetd_io::admit::composition_fault(&package_id, &staged.manifest);
                    if let Some(fault) = &composition {
                        println!(
                            "composition: {}",
                            skynetd_io::admit::composition_refusal(&package_id, fault)
                        );
                        println!(
                            "  The blob is staged and will not activate until the graph terminates."
                        );
                    }
                    let summary = skynetd_io::admit::summary(&report);
                    match &composition {
                        None => summary,
                        Some(fault) => format!("{summary}; composition {}", fault.code.as_str()),
                    }
                }
                Err(error) => {
                    // The store verified the digest over this manifest, so it is the manifest the
                    // publisher signed — and it still may not decode into the shape admission
                    // needs. Saying so beats an empty report.
                    let detail = format!("the manifest does not decode: {error:?}");
                    println!("admission: not run — {detail}");
                    format!("admission not run: {detail}")
                }
            };

            // Open since the twelfth session: the verdict was printed to whoever ran the command
            // and recorded nowhere. It is a decision about whether code may run on this machine,
            // which makes it exactly the thing an incident review asks about, and a terminal that
            // has been closed answers nothing.
            //
            // Recorded on **every** path out of a successful stage, including the two where
            // admission produced no verdict. "Nothing checked this package, and here is why" is a
            // fact about what is now on the disk; leaving it out would make an unexamined blob
            // indistinguishable from one nobody staged.
            //
            // Into the audit log rather than the ring, because `stage` is not the daemon and the
            // ring has one writer (see `blackbox::mirror_audit`). `skynetd` copies it across on its
            // next pass, which is where the delay this design accepts comes from.
            record(
                &skynetd_io::audit::Audit::of(store.layout()),
                &skynetd_io::audit::rfc3339(wall_ms()),
                &flag(argv, "--by").unwrap_or_else(|| "unattributed".to_string()),
                "package.admit",
                &format!("{} {version}", staged.id),
                &outcome,
                None,
            );
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
///
/// No longer `#[cfg(unix)]`. It was, because only the daemon loop called it and the loop needs a
/// Unix socket; `skynetd once` is `plan` and `apply` over a directory and runs anywhere, which is
/// exactly why it was the path with no recording.
/// Jitter for one backoff, drawn from the monotonic clock.
///
/// §34.4 wants the delay uniform over `[0, exponential]` and `backoff_ms` takes the fraction as an
/// argument, for the reason §35.2 gives about clocks: a function that reaches for a random number
/// cannot be tested. So the reaching happens here, at the edge, where the untestable part is one
/// line long.
///
/// Only the daemon loop uses it — `once` passes the top of the window, because the thundering herd
/// §34.4 guards against is a fleet retrying in lockstep and a person running one command is not one.
///
/// The low bits of a monotonic millisecond counter are not a random number and do not need to be.
/// What §34.4 asks for is **decorrelation**: a hundred robots that lost the same access point must
/// not retry in lockstep. Their uptimes differ by seconds at least, which is thousands of
/// milliseconds, so the low bits differ — and that is the whole requirement. A cryptographic source
/// here would be a dependency and a stronger claim than the problem needs.
#[cfg(unix)]
fn jitter_for(now_ms: u64) -> f64 {
    (now_ms % 1_000) as f64 / 1_000.0
}

/// Drain the outbound queue once, and say what happened when it is worth saying.
///
/// Quiet when the queue is empty or everything simply went, because a supervisor that logged a line
/// per idle pass is a supervisor whose log nobody reads. Loud about a **block**, because §34.3 is
/// explicit that the wait is visible rather than silent, and about failures, because a queue that is
/// filling while every send is refused is the shape of an outage.
/// Pull this robot's desired state from the Hub (§11).
///
/// **First in the pass**, before the observation and the plan, so a desired state that lands is
/// reconciled in the same pass rather than one later — which on a long reconcile interval is the
/// difference between an operator seeing their change take and seeing nothing happen for a minute.
///
/// Failing changes nothing. §11: *"no control decision may require a network round trip"*, and a
/// robot that cannot reach its Hub reconciles against what it has — which is what it does for most
/// of its life (§34.1).
fn pull_desired<L, S>(daemon: &mut skynetd_io::runtime::Daemon<L, S>, layout: &Layout, now_ms: u64)
where
    L: skynetd_io::launch::Launcher,
    S: skynetd_io::observe::Slots,
{
    // How many packages are running, which is what §11's "this empties the robot" finding is about.
    // Observed rather than counted from the desired state, for §33.3's reason: the finding is about
    // what would stop, and what would stop is what is running.
    let running = daemon
        .observe(now_ms)
        .units
        .iter()
        .filter(|unit| unit.state == skynetd::actual::UnitState::Running)
        .count();

    let Some(pulled) = skynetd_io::intent::pull(layout, running) else {
        return;
    };
    // Kept on the daemon so `GET /intent` reports the last pull rather than making one of its own —
    // a `GET` that reached out to a Hub would let anyone with an observer credential make this robot
    // talk to the internet.
    daemon.last_pull = Some(pulled.clone());
    if !pulled.reached {
        eprintln!(
            "skynetd: the desired state could not be pulled: {}",
            pulled.detail
        );
        return;
    }
    let Some(verdict) = &pulled.verdict else {
        return;
    };
    for fault in &verdict.faults {
        eprintln!("skynetd: desired state: {}", fault.message);
    }
    if pulled.written {
        // Reloaded here rather than left for the next pass, which is the point of pulling first.
        daemon.reload();
        eprintln!(
            "skynetd: adopted generation {} from the Hub",
            verdict.generation
        );
    }

    // The release manifest for whatever OS version the desired state now names (§12, §57).
    //
    // After the reload rather than before it, because the version to ask about is the one the
    // robot has just adopted — asking first would fetch a manifest for the previous generation's
    // image every time an operator changed one, which is the one moment it matters.
    //
    // A few hundred bytes, and deliberately not the bundle: §12's third rule is decided entirely
    // from the manifest, so the refusal that must happen before a hundred megabytes cross a
    // metered link happens here.
    pull_release(daemon, layout);
}

/// Close §74's telemetry window and open the next one.
///
/// Quiet on every path that is not a fault, which here means quiet almost always: an unpaired robot
/// counts nothing, and a window that cannot be counted is a window the queue is better off without.
///
/// The window is opened on the **first** call rather than at start-up, so its first bound is a real
/// pass and not a robot that had not begun.
///
/// `recorder` is §13's ring, which is where five of §74's eight counters are read from. A robot
/// whose black box would not open still reports a window — the three counters this loop keeps are
/// still true — and says nothing about the other five rather than zero for them.
fn report_telemetry<L, S>(
    daemon: &mut skynetd_io::runtime::Daemon<L, S>,
    recorder: Option<&skynetd_io::blackbox::Recorder>,
    layout: &Layout,
    now_ms: u64,
) where
    L: skynetd_io::launch::Launcher,
    S: skynetd_io::observe::Slots,
{
    let queue = skynetd_io::outbound::Queue::of(layout);
    let dropped: u64 = queue.status().dropped.iter().map(|(_, count)| *count).sum();
    let wall = skynetd_io::api::wall_ms() as f64;
    let monotonic = now_ms as f64;
    // Where the ring is now, which is where the *next* window begins. Read once and used for both
    // the close and the open below, so the two windows tile in the recording exactly as they tile
    // on the clock — a second reading taken between them would leave any record written in between
    // in neither window (§74.1).
    let sequence = recorder
        .map(|recorder| recorder.next_sequence())
        .unwrap_or(0);

    // Nothing open yet: this pass is the first bound, not a window from the epoch.
    if daemon.counting.opened_monotonic_ms == 0.0 && daemon.counting.opened_wall_ms == 0.0 {
        daemon.counting =
            skynetd_io::telemetry::Counting::opened(wall, monotonic, dropped, sequence);
        return;
    }

    daemon.counting.passes = daemon.counting.passes.saturating_add(1);
    let tally = recorder.and_then(|recorder| recorder.tally(daemon.counting.opened_sequence));
    let window = daemon.counting.close(wall, monotonic, dropped, tally);

    match skynetd_io::telemetry::enqueue(&queue, layout, &window) {
        Ok(_) => {}
        // An unpaired robot has nowhere to send one, which is §50.2's shipping state rather than a
        // failure. An unusable window is a clock that moved; both are quiet.
        Err(skynetd_io::telemetry::NotQueued::Unpaired) => {}
        Err(skynetd_io::telemetry::NotQueued::Unusable(_)) => {}
        Err(skynetd_io::telemetry::NotQueued::Refused(detail)) => {
            eprintln!("skynetd: a telemetry window could not be queued: {detail}");
        }
    }

    daemon.counting = skynetd_io::telemetry::Counting::opened(wall, monotonic, dropped, sequence);
}

/// Cache the release manifest for the desired OS version (§12, §57).
///
/// Quiet on every path that is not a fault, and that list is longer than it looks: an unpaired
/// robot, a desired state naming no OS version, and a Hub answering with a manifest this robot
/// already holds are all ordinary. §34.1 assumes the robot is offline most of the time.
fn pull_release<L, S>(daemon: &mut skynetd_io::runtime::Daemon<L, S>, layout: &Layout)
where
    L: skynetd_io::launch::Launcher,
    S: skynetd_io::observe::Slots,
{
    let channel = daemon.desired.os_channel.clone();
    let version = daemon.desired.os_version.clone();
    let Some(pulled) = skynetd_io::releases::pull(layout, &channel, &version) else {
        return;
    };
    if !pulled.reached {
        eprintln!(
            "skynetd: the release catalogue could not be pulled: {}",
            pulled.detail
        );
    } else if !pulled.fault.is_empty() {
        eprintln!("skynetd: release `{version}`: {}", pulled.fault);
    } else if pulled.written {
        eprintln!("skynetd: cached the manifest for release `{version}`");
    }
    daemon.last_release_pull = Some(pulled);
}

/// Build a check-in and put it in §34.4's queue (§11, §17, §33.3).
///
/// Called on every pass, immediately before the drain, so the item the drain picks up is the one
/// describing the pass that just ran. §34.4 collapses check-ins, so this holds the queue at exactly
/// one and always the current one — which is what makes calling it every pass correct rather than
/// wasteful, and is the level-triggered rule from §33.3 applied to the report.
///
/// Says nothing on an unpaired robot. That is not a failure to report: §50.2 ships a robot
/// belonging to nobody, and a loop that logged one every few seconds would train an operator to
/// ignore the line that matters.
fn report_checkin<L, S>(
    daemon: &mut skynetd_io::runtime::Daemon<L, S>,
    layout: &Layout,
    now_ms: u64,
) where
    L: skynetd_io::launch::Launcher,
    S: skynetd_io::observe::Slots,
{
    let trust = skynetd_io::trust::TrustStore::load(layout);
    let (identity, _) = skynetd_io::identity::evaluate(layout, &trust);
    if !identity.identity.credential {
        return;
    }

    let actual = daemon.observe(now_ms);
    // §36's gauge, read in this pass and dated with it. The descriptor is loaded here rather than
    // held on the daemon for the reason every other read in this function is fresh: the loop is
    // level-triggered (§33.3), and a power profile cached at start-up would survive a descriptor
    // somebody replaced on a running robot.
    let body = skynetd_io::embodiment::load(layout);
    let (_, reading) = skynetd_io::power::observe(&mut daemon.power, layout, &body, now_ms);
    let health = daemon.health(&actual, now_ms, Some(&reading));
    let policy = skynetd_io::identity::running_policy(&daemon.store, &actual);
    let attestation =
        skynetd_io::identity::attestation(layout, &trust, &identity, &actual.os.active, &policy);

    let progress = skynetd_io::checkin::progress(&daemon.desired, &actual, daemon.safe_state);
    // The lag is the one aggregate field this daemon can both compute and date, and it is dated
    // with the observation it came from rather than with the wall clock — §33.6 wants an age and
    // §35.1 does not want a control-path reading of `CLOCK_REALTIME`.
    let lag = daemon
        .desired
        .generation
        .checked_sub(actual.generation)
        .filter(|_| daemon.desired.generation >= actual.generation);
    let rolled = skynetd_io::checkin::health(&health, 0, lag);

    let built = skynetd_io::checkin::build(
        &identity,
        attestation,
        progress,
        rolled,
        daemon.operating_state.as_str(),
        &skynetd_io::intent::held_digest(layout),
        // §7.6. Over the `body` this pass already loaded, so the check-in and `GET /limits` cannot
        // report two different limit sets for one robot.
        Some(skynetd_io::limits::report(&skynetd_io::limits::read(
            layout,
            &body,
            skynetd_io::limits::ROBOT_TIER,
        ))),
    );
    let queue = skynetd_io::outbound::Queue::of(layout);
    match skynetd_io::checkin::enqueue(&queue, &identity, &built) {
        Ok(_) => {}
        Err(skynetd_io::checkin::NotQueued::Unpaired) => {}
        Err(skynetd_io::checkin::NotQueued::Refused(detail)) => {
            eprintln!("skynetd: the check-in could not be queued: {detail}");
        }
    }
}

/// Move what can be moved of the oldest uncollected sealed window (§79).
///
/// Says nothing on an unpaired robot, for `report_checkin`'s reason: §50.2 ships a robot belonging
/// to nobody, and a loop that logged one every few seconds would train an operator to ignore the
/// line that matters. It is also not a *failure* — there is no endpoint, so there is no address for
/// the bytes, and the window is on the disk where `skynet blackbox` can still hand it to whoever is
/// standing next to the robot.
///
/// Quiet on `Idle` and on `Waiting` too. A robot with nothing sealed, or one waiting for an
/// unmetered link, is in the ordinary state for most of its life (§34.1), and both are visible on
/// `GET /blackbox` for anybody who asks.
fn report_upload(
    uploader: &mut Box<dyn skynetd_io::upload::Uploader>,
    layout: &Layout,
    now_ms: u64,
    jitter: f64,
) {
    let Some((endpoint, device, credential)) = skynetd_io::identity::credential_for_sending(layout)
    else {
        return;
    };
    let table = skynetd_io::connectivity::Table::of(layout);
    let step = skynetd_io::upload::advance(
        layout,
        uploader.as_mut(),
        &table,
        &endpoint,
        &device,
        Some(&credential),
        now_ms,
        jitter,
    );
    match step {
        skynetd_io::upload::Step::Collected { name, .. } => {
            eprintln!("skynetd: black box — {name} is on the Hub, and the receipt is on the disk");
        }
        skynetd_io::upload::Step::Moved { name, held, .. } => {
            eprintln!(
                "skynetd: black box — {name}, the receiver holds {}",
                match held {
                    Some(bytes) => format!("{bytes} bytes"),
                    None => "an amount it did not report; the next pass asks".to_string(),
                }
            );
        }
        skynetd_io::upload::Step::Failed { detail } => {
            eprintln!("skynetd: black box — the window could not be sent: {detail}");
        }
        skynetd_io::upload::Step::Idle { .. } | skynetd_io::upload::Step::Waiting { .. } => {}
    }

    // §80's reclaim, after the collection pass rather than before it: a window that has just
    // gained a receipt is a window that may now be reclaimable, and running the two in the other
    // order would leave every reclaim a pass behind.
    //
    // It does nothing today, and the call is here rather than deferred so that the day
    // `free_of` returns a number there is no wiring left to remember. `run` refuses to scan
    // without a measurement, so this costs one `matches!`.
    let free = skynetd_io::retention::free_of(layout);
    let (plan, failures) =
        skynetd_io::retention::run(layout, free, skynetd_io::retention::DEFAULT_FLOOR_BYTES);
    for window in &plan.reclaim {
        eprintln!(
            "skynetd: black box — {} was reclaimed under §80's floor; its tombstone holds \
             {} bytes at {}",
            window.name, window.size_bytes, window.digest
        );
    }
    for failure in &failures {
        eprintln!("skynetd: black box — a window could not be reclaimed: {failure}");
    }
    if !plan.reclaim.is_empty() && !plan.reaches_floor {
        // The sentence that sends somebody to look somewhere else. Everything reclaimable is gone
        // and the disk is still under the floor, so what is left is either incidents — which are
        // never reclaimed — or something that is not the black box at all.
        eprintln!(
            "skynetd: black box — everything reclaimable is gone and the disk is still under \
             §80's floor. What remains is cited by an incident or uncollected; see GET /blackbox"
        );
    }
}

fn report_drain(
    sender: &mut skynetd_io::connectivity::Sender,
    layout: &Layout,
    sink: &dyn skynetd_io::connectivity::Sink,
    now_ms: u64,
    jitter: f64,
) {
    let bearers = skynetd_io::connectivity::Table::of(layout);
    // Sixteen per pass. Enough that a backlog clears over a few minutes of a ten-second loop, few
    // enough that one pass cannot become a long upload.
    let drained = sender.drain(&bearers, sink, now_ms, jitter, 16);
    if drained.sent > 0 || drained.failed > 0 {
        eprintln!(
            "skynetd: outbound — {} sent, {} failed, {} backing off",
            drained.sent, drained.failed, drained.backing_off
        );
    }
    // Its own line, and it does not say "sent". A `--outbox` pass empties the queue and delivers
    // nothing, and a log that spelled both the same way would read as a working uplink on a robot
    // that has never had one.
    if drained.handed_off > 0 {
        eprintln!(
            "skynetd: outbound — {} handed to a local sink, not sent: {}",
            drained.handed_off,
            sink.describe()
        );
    }
    if let Some(blocked) = drained.blocked {
        eprintln!("skynetd: outbound held — {}", blocked.detail());
    }
}

/// One reconcile pass, and its record.
///
/// The two exist together or the recording is a lie by omission, so they are one function and there
/// is one place either can be changed. `skynetd once` and `skynetd` (the loop) both come through
/// here; before this they were two call sites, and the one that did not grow a recorder was the one
/// §33 describes as a legitimate way to operate a robot.
///
/// The audit mirror (§38.3) runs here for the same reason it ran in the loop: it is bounded work
/// that belongs with the pass, it covers configuration changed by a process that is not this one,
/// and a robot whose daemon is not reconciling has bigger problems than a delayed record. A `once`
/// pass now picks up whatever the audit log gained since the last one, which is the behaviour an
/// operator running `once` from a script would assume they already had.
fn converge_and_record<L: skynetd_io::launch::Launcher, S: skynetd_io::observe::Slots>(
    daemon: &mut Daemon<L, S>,
    recorder: Option<&mut skynetd_io::blackbox::Recorder>,
    layout: &Layout,
    now_ms: u64,
    started_ms: u64,
) -> Vec<skynetd_io::runtime::Pass> {
    let passes = daemon.converge(now_ms);

    // §54.4's lifecycle, after the reconcile rather than before it. The runtime reads which units
    // are running and the reconciler is what changes that, so running it first would mean every
    // skill saw the previous pass's process table and a skill that had just exited took an extra
    // pass to be noticed.
    //
    // Wall time for the same reason `GET /calibration` takes it: §54.2 asks for a calibration's
    // *age*, which is a comparison against a clock, and §35.1 permits exactly this — it is a
    // record's age and not a control decision.
    let wall_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs() as f64);
    let actual = daemon.observe(now_ms);

    // §54.4's intervention count, off §13's ring. The kernel does not have to be asked what it did:
    // it wrote it down, and this daemon holds the recording. `None` is a robot with no recorder or
    // a ring that cannot hold a cycle — the same discriminator §74.6 uses — and it keeps the field
    // out of the episode rather than putting a zero in it.
    //
    // The cursor is advanced whether or not anything is counted, so an interval that produced no
    // reading is not re-counted into the next one.
    let interventions_since = recorder.as_deref().and_then(|recorder| {
        let tally = recorder.tally(daemon.skills_sequence)?;
        tally.measured().then_some(tally.interventions)
    });
    if let Some(recorder) = recorder.as_deref() {
        daemon.skills_sequence = recorder.next_sequence();
    }

    let skills = daemon.advance_skills(now_ms, wall_s, &actual, interventions_since);

    // §12's second rule, after the reconcile for the same reason the skills are: two of the three
    // health inputs — whether the wanted packages are running, and whether the plan converged — are
    // what the pass just established. Running it first would judge a boot on the previous pass's
    // evidence, and on the *first* pass after an update that is evidence from the old image.
    let confirmation = daemon.confirm_boot(
        started_ms,
        now_ms,
        wall_s,
        &actual,
        passes.last().is_some_and(|pass| pass.converged()),
    );
    if let Some(detail) = &confirmation.failed {
        // Loud, because a health gate that could not reach the bootloader is a robot whose
        // unattended recovery has quietly stopped being unattended.
        eprintln!("skynetd: §12's boot gate could not reach the bootloader: {detail}");
    }

    let Some(recorder) = recorder else {
        return passes;
    };
    let mono_ns = now_ms.saturating_mul(1_000_000);
    for pass in &passes {
        record_pass(recorder, mono_ns, pass);
    }
    record_skills(recorder, mono_ns, &skills);
    // §26 lists OTA on the event channel, and this is the half of it that happens without an
    // operator. A `wait` is not recorded: it is the ordinary state for most of a boot budget, and a
    // record every pass would fill the ring with a robot deciding nothing.
    if matches!(
        confirmation.verdict.decision,
        skynet_contracts::ota::BootDecision::Confirm
            | skynet_contracts::ota::BootDecision::RollBack
    ) {
        let _ = recorder.event(
            mono_ns,
            skynet_blackbox::record::EventCode::Ota,
            confirmation.version.as_deref().unwrap_or("(unknown)"),
            &format!(
                "boot {} — {}",
                confirmation.verdict.decision.as_str(),
                confirmation.verdict.detail
            ),
        );
    }
    let mirrored = skynetd_io::blackbox::mirror_audit(layout, recorder, mono_ns);
    if mirrored.remaining > 0 {
        eprintln!(
            "skynetd: {} configuration change(s) recorded, {} still to copy from the audit log",
            mirrored.recorded, mirrored.remaining
        );
    }
    passes
}

/// §54.4: *"Every transition is a black box event."*
///
/// Every transition, not every changed one. A skill sitting in `running` for four hundred cycles
/// would fill the ring with four hundred identical records and push out the cycles the incident
/// needs, which is the completeness guarantee (§13) destroyed by the thing meant to strengthen it.
/// So the unchanged ones are dropped here rather than in the lifecycle — `advance` is a pure
/// function and reports what it found, and the decision about what is worth a record belongs where
/// the ring is.
///
/// The consequence is stated rather than left to be discovered: a sealed recording shows a skill's
/// *ladder* and not its dwell time. How long it sat in each phase is in the episode's duration and
/// in the invocation's own timestamps, both of which outlive the ring.
fn record_skills(
    recorder: &mut skynetd_io::blackbox::Recorder,
    mono_ns: u64,
    reports: &[skynetd_io::skill::Report],
) {
    use skynet_blackbox::record::EventCode;
    for report in reports {
        if !report.transition.changed {
            continue;
        }
        let _ = recorder.event(
            mono_ns,
            EventCode::SkillTransition,
            &report.invocation,
            &report.event_detail(),
        );
    }
}

fn record_pass(
    recorder: &mut skynetd_io::blackbox::Recorder,
    mono_ns: u64,
    pass: &skynetd_io::runtime::Pass,
) {
    use skynet_blackbox::record::EventCode;
    use skynetd::reconcile::ActionKind as K;

    for (action, outcome) in &pass.applied {
        // A wait is not recorded, for the same reason the boot gate's is not (see the caller): it
        // is the ordinary state of a robot fetching an image over a warehouse link, and one event
        // per pass would fill a bounded ring with a robot that decided nothing — displacing the
        // records §26 keeps the ring for.
        if matches!(outcome, skynetd_io::runtime::Outcome::Waiting { .. }) {
            continue;
        }
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
/// Never used for a deadline, a backoff or a watchdog. Present so a line in a journal — and now an
/// audit entry — can be correlated with the Hub, which is the one job §35.1 gives `CLOCK_REALTIME`.
fn wall_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Wall time in seconds, for §45.4's age comparison. Same clock, same rule, same non-use.
fn wall_s() -> Option<f64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|elapsed| elapsed.as_secs() as f64)
}

/// One observation of §27's machine, then §33.4's barrier read off it.
///
/// The order is the whole point and it is easy to get backwards: the state is observed first, and
/// the barrier is derived from it. A `refresh_safe_state` that ran first would narrow this pass
/// against the previous pass's state, which on the pass that matters — the one where a policy
/// started — is the pass that would apply a driver swap under it.
fn observe_state<L: skynetd_io::launch::Launcher, S: skynetd_io::observe::Slots>(
    daemon: &mut Daemon<L, S>,
) {
    daemon.refresh_operating_state(wall_s());
    daemon.refresh_safe_state();
}

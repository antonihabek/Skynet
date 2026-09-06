//! `skynetk` — the process that owns the control loop.
//!
//! ```text
//! skynetk run --driver <library> [--config <json>] [--hz 100] [--cycles N]
//!             [--channels a,b,c] [--tier guest|host] [--velocity-max <rad/s>]
//!             [--jerk <fraction>]
//! ```
//!
//! # What running it proves, and what it does not
//!
//! It opens a real RHAL driver, turns the loop at a declared rate, and records every cycle. On a
//! robot with no policy runtime — which is every robot today — the ring stays empty, the kernel
//! holds on every cycle, and the driver receives a mask of zeros. That is not a degenerate case
//! being tolerated: it is the state §33.1 describes as the one that makes the rest of the OS
//! possible, and it is worth being able to reach and watch.
//!
//! **The ring is private to this process unless `--policy-socket` says otherwise.** §25 has the
//! runtime create the ring and hand the descriptor to the sandboxed policy, and this binary is the
//! side that creates and offers: with a socket it maps a `memfd`, gives the descriptor to whatever
//! connects, and reads what that process writes; without one it allocates the ring in its own
//! address space and says so on every run, rather than printing a path nothing can open. The other
//! half is `skynetp --ring-socket`, and the two have been run against each other — 199 commanded
//! cycles from a separate process, through the eight guards, to a driver.
//!
//! **Nothing here decides a tier.** `--tier` says what to resolve limits at, and
//! [`skynet_kernel::Tier::enforcement`] derives what that means. Passing `--tier host` on a
//! general-purpose scheduler does not make the clamp real; `overruns` in the summary is the one
//! number this binary can honestly contribute to that question.

use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::time::{Duration, Instant};

use skynet_blackbox::record::Shape;
use skynet_blackbox::ring::Meta;
use skynet_blackbox::seal::Reason;
use skynet_kernel::{
    can_start, resolve_limits, Available, Envelope, OperatorProfile, ResolveLimitsParams, Tier,
    TransportClamp, WarningChannel,
};
use skynet_kernel_host::driver::{Actuator, Rhal};
use skynet_kernel_host::handoff;
use skynet_kernel_host::recorder::{Recorder, DEFAULT_BUDGET};
use skynet_kernel_host::run::Host;
use skynet_ring::{words_for, Ring};

const USAGE: &str = "\
skynetk run --driver <library> [options]

  Turn the control loop against a real RHAL driver.

  --driver <path>       the shared object to load. Required.
  --config <json>       passed verbatim to the driver's open(). Default empty.
  --hz <rate>           control rate. Default 100.
  --cycles <n>          stop after n cycles. Default: run until interrupted.
  --channels <a,b,c>    channel names, in transport-plan order. Default: joint0..jointN
                        from the driver's own joint count.
  --tier guest|host     what to resolve limits at. Default guest, because a tier
                        is a claim and the default should not make one.
  --velocity-max <v>    declared per-joint velocity ceiling, rad/s. Default: none,
                        which leaves the envelope guard with nothing to enforce.
  --jerk <fraction>     jerk ceiling as a fraction of a channel's ceiling per cycle.
                        Also the rate a controlled stop ramps at (see cycle.rs).
                        Without it the rate-limit guard reports itself unavailable.
  --watchdog-ms <ms>    command timeout before the controller faults. Without it the
                        watchdog guard reports itself unavailable.
  --blackbox <dir>      record every cycle into §13's ring under this directory, and
                        seal the window on exit. Takes an exclusive writer lock: one
                        ring has one writer, and a second is refused rather than
                        allowed to corrupt it.
  --policy-socket <p>   create the command ring in shared memory and offer its
                        descriptor to whatever connects here (§25). Without it the
                        ring is private to this process and every cycle reads an
                        empty one, which is a robot with no policy.
  --quiet               only print the summary.

skynetk blackbox <dir>

  Read a ring back: its shape, what it holds, and §74's five counters. Needs no
  daemon and no socket — it opens the file  wrote.

  Without --policy-socket the ring is private to this process, so every cycle reads
  an empty one and the kernel holds — which is the honest state of a robot that is
  running no policy (§33.1). With it, `skynetp --ring-socket <p>` is the other half:
  the runtime creates the ring, the policy receives it and writes, and the guards
  are between them.";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&args));
}

/// One parsed option list.
struct Options {
    driver: PathBuf,
    config: String,
    hz: f64,
    cycles: Option<u64>,
    channels: Option<Vec<String>>,
    tier: Tier,
    velocity_max: Option<f64>,
    jerk: Option<f64>,
    watchdog_ms: Option<f64>,
    blackbox: Option<PathBuf>,
    policy_socket: Option<PathBuf>,
    quiet: bool,
}

/// How many slots the command ring holds.
///
/// Sixteen at 100 Hz is a sixth of a second of history, which is far more than a consumer
/// reading one slot per cycle needs — the ring is not a buffer, it is the newest command and
/// enough room behind it that a reader is never torn by a writer (§25.1). It is the runtime's
/// number and not the policy's: a policy is told what it received and cannot choose it.
const RING_SLOTS: usize = 16;

fn fail(message: &str) -> i32 {
    eprintln!("skynetk: {message}");
    1
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        driver: PathBuf::new(),
        config: String::new(),
        hz: 100.0,
        cycles: None,
        channels: None,
        tier: Tier::Guest,
        velocity_max: None,
        jerk: None,
        watchdog_ms: None,
        blackbox: None,
        policy_socket: None,
        quiet: false,
    };

    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let value = || -> Result<String, String> {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| format!("`{arg}` needs a value"))
        };
        match arg {
            "--driver" => {
                options.driver = PathBuf::from(value()?);
                index += 2;
            }
            "--config" => {
                options.config = value()?;
                index += 2;
            }
            "--hz" => {
                options.hz = value()?
                    .parse()
                    .map_err(|_| "--hz wants a number".to_string())?;
                index += 2;
            }
            "--cycles" => {
                options.cycles = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--cycles wants a number".to_string())?,
                );
                index += 2;
            }
            "--channels" => {
                options.channels = Some(
                    value()?
                        .split(',')
                        .map(|name| name.trim().to_string())
                        .collect(),
                );
                index += 2;
            }
            "--tier" => {
                options.tier = match value()?.as_str() {
                    "guest" => Tier::Guest,
                    "host" => Tier::Host,
                    // Certified is deliberately not selectable. It requires an independent safety
                    // processor with its own stop path, and a flag is not one.
                    other => return Err(format!("`{other}` is not a selectable tier")),
                };
                index += 2;
            }
            "--velocity-max" => {
                options.velocity_max = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--velocity-max wants a number".to_string())?,
                );
                index += 2;
            }
            "--jerk" => {
                options.jerk = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--jerk wants a number".to_string())?,
                );
                index += 2;
            }
            "--watchdog-ms" => {
                options.watchdog_ms = Some(
                    value()?
                        .parse()
                        .map_err(|_| "--watchdog-ms wants a number".to_string())?,
                );
                index += 2;
            }
            "--blackbox" => {
                options.blackbox = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--policy-socket" => {
                options.policy_socket = Some(PathBuf::from(value()?));
                index += 2;
            }
            "--quiet" => {
                options.quiet = true;
                index += 1;
            }
            other => {
                return Err(format!(
                    "`{other}` is not an option. Run `skynetk` for usage."
                ))
            }
        }
    }

    if options.driver.as_os_str().is_empty() {
        return Err("--driver is required".to_string());
    }
    if !(options.hz > 0.0 && options.hz.is_finite()) {
        return Err("--hz must be a positive rate".to_string());
    }
    Ok(options)
}

fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("run") => {}
        Some("blackbox") => return blackbox(&args[1..]),
        Some("--help") | Some("-h") | None => {
            println!("{USAGE}");
            return 0;
        }
        Some(other) => {
            return fail(&format!(
                "`{other}` is not a subcommand. They are `run` and `blackbox`."
            ))
        }
    }

    let options = match parse(&args[1..]) {
        Ok(options) => options,
        Err(message) => return fail(&message),
    };

    let mut body = match Rhal::open(&options.driver, &options.config) {
        Ok(body) => body,
        Err(error) => return fail(&error.to_string()),
    };

    let joints = body.joints();
    if joints == 0 {
        return fail(
            "the driver reported no joints. A body with nothing to command cannot be driven, and \
             guessing a width would be guessing how far to write into the driver's arrays.",
        );
    }

    // Channel names come from the transport plan on a real robot. Without one, they are positional
    // and named as such — `joint0` is honestly a position, where a made-up anatomical name would
    // read as knowledge this process does not have.
    let names: Vec<String> = match options.channels {
        Some(given) => {
            if given.len() != joints {
                return fail(&format!(
                    "--channels names {} channels and the driver reports {joints} joints. A plan \
                     that disagrees with the body would command a joint that does not exist.",
                    given.len()
                ));
            }
            given
        }
        None => (0..joints).map(|index| format!("joint{index}")).collect(),
    };
    let channels: Vec<&str> = names.iter().map(String::as_str).collect();

    // Ceilings, per channel, from whatever was declared. Nothing is invented: with no
    // `--velocity-max` there is no per-channel ceiling and the envelope guard reports itself
    // unavailable, which is §7.2's rule rather than a defect of this binary.
    let per_channel: Vec<(&str, f64)> = match options.velocity_max {
        Some(max) => channels.iter().map(|name| (*name, max)).collect(),
        None => Vec::new(),
    };

    let envelope = Envelope {
        joint_velocity_max_rad_s: options.velocity_max,
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        watchdog_ms: options.watchdog_ms,
        estop: Some(body.has_stop()),
        source: Some("skynetk command line"),
    };

    // The jerk ceiling is the **operator's**, not the transport plan's — `resolve_limits` reads it
    // off the profile, and putting it anywhere else means `--jerk` is accepted and does nothing.
    // Which is what happened here on the first run, and what the `unavailable RateLimit — needs
    // profile.max_delta_per_cycle` line in the output was saying.
    let profile = options.jerk.map(|jerk| OperatorProfile {
        name: "skynetk command line",
        joint_velocity_max_rad_s: None,
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        max_delta_per_cycle: Some(jerk),
        proximity: None,
        workspace: &[],
        self_collision_margin_m: None,
    });

    let transport = (!per_channel.is_empty()).then_some(TransportClamp {
        joint_velocity_max_rad_s: options.velocity_max,
        joint_torque_max_nm: None,
        base_velocity_max_m_s: None,
        min_control_hz: Some(options.hz),
        per_channel: &per_channel,
        source: Some("skynetk command line"),
    });

    let limits = resolve_limits(&ResolveLimitsParams {
        tier: options.tier,
        envelope: Some(envelope),
        transport,
        profile,
        available: Available {
            imu: false,
            people_detection: false,
            legged: false,
        },
        frames: None,
    });

    if !options.quiet {
        println!("driver     {}", options.driver.display());
        println!("joints     {joints}");
        println!("channels   {}", channels.join(", "));
        println!(
            "tier       {:?}, enforcement {:?}",
            limits.tier, limits.enforcement
        );
        println!("rate       {} Hz", options.hz);
        println!(
            "estop      {}",
            if body.has_stop() {
                "present"
            } else {
                "absent — a Cat 0 response cannot be performed on this body"
            }
        );
        for notice in limits.notices.iter() {
            println!("notice     {:?}", notice.code);
        }
        // §7.2: a guard that cannot run says so, and says what it is missing. Printing the list is
        // most of this binary's value on a robot with nothing declared.
        for guard in limits.unavailable_guards.iter() {
            let missing: Vec<&str> = guard.missing.iter().copied().collect();
            println!(
                "unavailable {:?} — needs {}",
                guard.guard,
                missing.join(", ")
            );
        }
        // Only when there is no socket. The shared case prints its own line below, once the port is
        // open — and printing both was this binary saying two contradictory things about one ring
        // in the same breath.
        if options.policy_socket.is_none() {
            println!(
                "ring       private to this process — no policy was offered one on this run (§25)"
            );
            println!();
        }
    }

    // The ring. Two shapes, and which one this run has is the difference between a loop that can be
    // commanded and one that cannot.
    //
    // **Private**, in this process's own memory, when nothing was asked for: the state every run of
    // this binary has been in since it was written, and the honest one on a robot with no policy.
    //
    // **Shared**, over a `memfd` this process creates and hands to whoever connects to
    // `--policy-socket`, which is §25's arrangement — the runtime creates the ring, the sandboxed
    // policy receives it, and the page tables rather than an agreement are what make the policy the
    // only writer.
    // The mapping and the port are two variables rather than a pair, and the compiler is why: the
    // ring below borrows the mapping for the rest of the run, and a tuple would make offering the
    // descriptor a mutable borrow of the same thing the ring is reading.
    let mut shared = None;
    let mut body_ring = None;
    let mut port = None;
    match options.policy_socket.as_deref() {
        None => {}
        Some(path) => match handoff::Port::open(path, RING_SLOTS, RING_SLOTS) {
            Ok((mapping, observations, opened)) => {
                if !options.quiet {
                    println!("ring       shared, offered at {}", path.display());
                    println!("body       published into the observation ring beside it");
                    println!();
                }
                shared = Some(mapping);
                body_ring = Some(observations);
                port = Some(opened);
            }
            // Refused rather than fallen back to a private ring. A loop that quietly ran with a
            // ring nobody can reach, after being asked for one that could be, is a robot that looks
            // commandable and is not — and §79.7 is this repository's record of what a route that
            // answers and moves nothing costs.
            Err(error) => return fail(&error.to_string()),
        },
    }

    let words: Vec<AtomicU32> = (0..words_for(RING_SLOTS))
        .map(|_| AtomicU32::new(0))
        .collect();
    let ring = match shared.as_ref() {
        Some(mapping) => match mapping.ring() {
            Ok(ring) => ring,
            Err(error) => return fail(&format!("the shared ring could not be opened: {error}")),
        },
        None => match Ring::create(&words, RING_SLOTS) {
            Ok(ring) => ring,
            Err(error) => {
                return fail(&format!(
                    "the ring could not be created: {}",
                    error.as_str()
                ))
            }
        },
    };
    let mut consumer = ring.consumer();

    // The other direction (§25, §73's IPC row). A policy in its own process can command and, until
    // this, could not see: `Host::step_observing` has taken a producer since `skynet-obs` was
    // written and the binary called `Host::step`, which passes none — so the observation ring had a
    // producer API and no production caller. It has one when a ring was handed over, and none when
    // there is nobody to read it, which is the same rule the command ring follows.
    let observing = match body_ring.as_ref() {
        None => None,
        Some(mapping) => match skynet_obs::Ring::open(mapping.words()) {
            Ok(ring) => Some(ring),
            Err(error) => {
                return fail(&format!(
                    "the observation ring could not be opened: {}",
                    error.as_str()
                ))
            }
        },
    };
    let mut observations = observing.map(|ring| ring.producer());

    // §13's ring, when one was asked for. The shape is the body's, because a record's size is a
    // function of the joint and channel counts — a ring sized for a different robot could not hold
    // this one's cycles.
    let mut recorder = match options.blackbox.as_ref() {
        None => None,
        Some(dir) => {
            let shape = Shape {
                joints: joints as u16,
                channels: channels.len() as u16,
            };
            let meta = Meta {
                // What this process can actually fill. The descriptor digest and the OS image are
                // the daemon's to know, and a recorder inventing them would put two unverifiable
                // claims into evidence.
                robot_id: String::new(),
                embodiment: [0u8; 32],
                os_image: [0u8; 32],
                tier: limits.tier as u8,
                enforcement: limits.enforcement as u8,
                simulator: false,
                wall_epoch_ns: 0,
                mono_ns: 0,
                channels: names.clone(),
            };
            match Recorder::open(dir, meta, shape, DEFAULT_BUDGET) {
                Ok(recorder) => {
                    if !options.quiet {
                        let geometry = recorder.geometry();
                        println!(
                            "blackbox   {} — {} slots of {} bytes, about {:.0} s at {} Hz",
                            dir.display(),
                            geometry.capacity,
                            geometry.record_size,
                            geometry.capacity as f64 / options.hz,
                            options.hz,
                        );
                        println!();
                    }
                    Some(recorder)
                }
                Err(error) => return fail(&error.to_string()),
            }
        }
    };

    // §81.3's interlock, and the first thing in this repository to consult it.
    //
    // `can_start` has existed since the kernel was written and had **no production caller** — four
    // tests asked it and nothing else did, so a loop could open a driver and turn a cycle without
    // the start gate ever being consulted. That is the same shape as §26.2 being enforced on one
    // writer: a rule that is correct, tested, and protects nothing.
    //
    // `transport_refused` is `false` here because this binary builds its own clamp from the command
    // line and has no plan that could fail to resolve; on a robot the plan arrives from §6.3's
    // hand-off and can.
    //
    // The warning channel is `Undeclared` because that is what is true: `skynetk` is a bench
    // harness with no audio device, and §81.1's declaration lives in `state/audio.json`, which
    // `skynetd` owns. It changes nothing at the tiers this binary can select — the refusal is
    // certified-only and `--tier certified` is deliberately not selectable — and passing `Ready`
    // to keep it quiet would be asserting a control this process has never had.
    if let Err(reason) = can_start(false, &limits, WarningChannel::Undeclared) {
        return fail(reason);
    }

    let mut host = Host::new(&channels);
    let period = Duration::from_secs_f64(1.0 / options.hz);
    let origin = Instant::now();
    let mut first_error: Option<String> = None;
    let mut stop_unavailable_seen = false;
    let mut recorder_failed = false;

    let mut cycle = 0u64;
    loop {
        if let Some(limit) = options.cycles {
            if cycle >= limit {
                break;
            }
        }

        // §25's hand-off, offered rather than waited for. A policy that starts a minute after this
        // loop gets the descriptor a minute later, and a loop that stopped to wait for a producer
        // would miss its deadline because nothing was commanding it.
        if let (Some(mapping), Some(body), Some(port)) =
            (shared.as_ref(), body_ring.as_ref(), port.as_mut())
        {
            if port.offer(mapping, body) > 0 && !options.quiet {
                println!("policy     took the rings ({} so far)", port.handed());
            }
        }

        let started = Instant::now();
        let now_ns = origin.elapsed().as_nanos() as u64;
        let turn = host.step_observing(
            now_ns,
            &mut body,
            &mut consumer,
            &limits,
            Some(&envelope),
            observations.as_mut(),
        );

        if let Some(error) = turn.read_error {
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
        stop_unavailable_seen |= turn.stop_unavailable;

        // Every cycle, including the idle ones. §15's field conformance is a ratio over cycles, and
        // a recorder that skipped the uneventful ones would be writing the numerator without the
        // denominator.
        if let Some(recorder) = recorder.as_mut() {
            if let Err(error) = recorder.cycle(now_ns, &turn.cycle) {
                // A recording that stops is a fault, and a loop that stopped commanding because of
                // one would be a black box that took the robot down. It is reported once and the
                // loop keeps turning — which is §13's own priority, in the order it states it.
                if first_error.is_none() {
                    first_error = Some(format!("the black box stopped recording: {error}"));
                }
                recorder_failed = true;
            }
        }

        let elapsed = started.elapsed();
        if elapsed > period {
            host.note_overrun();
        } else {
            std::thread::sleep(period - elapsed);
        }
        cycle += 1;
    }

    let summary = host.summary();
    println!("cycles          {}", summary.cycles);
    println!("interventions   {}", summary.interventions);
    println!("idle            {} (the ring was empty)", summary.idle);
    println!("read failures   {}", summary.read_failures);
    println!("write failures  {}", summary.write_failures);
    println!("stops           {}", summary.stops);
    println!(
        "overruns        {} — cycles that missed the {:.2} ms period",
        summary.overruns,
        period.as_secs_f64() * 1000.0
    );

    // Sealed on the way out, because a window nobody sealed is a ring the next run overwrites.
    // §13's reason for sealing at shutdown is its reason for sealing at an incident: the records
    // are only evidence once they are a file.
    if let Some(recorder) = recorder.as_ref() {
        match recorder.seal(Reason::Shutdown, origin.elapsed().as_nanos() as u64) {
            Ok((sealed, path)) => println!(
                "\nsealed          {} — {} bytes",
                path.display(),
                sealed.bytes.len(),
            ),
            Err(error) => println!("\nthe window could not be sealed: {error}"),
        }
    }

    if let Some(error) = first_error {
        println!("\nfirst failure: {error}");
    }
    if stop_unavailable_seen {
        println!(
            "\nA stop was demanded and this body has no emergency stop. The kernel wrote a full \
             hold mask, which is the strongest thing available here, and that is weaker than what \
             §7.1 asked for."
        );
    }

    // A run that could not read the body is not a successful run, however many cycles it turned.
    if summary.read_failures > 0 || recorder_failed {
        return 1;
    }
    0
}

/// `skynetk blackbox <dir>` — what the ring under `dir` holds.
///
/// A reader rather than a client. `skynet blackbox` in the workstation CLI asks `skynetd` over its
/// local socket and gets the **daemon's** ring, which holds events and gaps and no cycles — §26.2
/// is why there are two rings and why merging them is open. This opens a ring file directly, so the
/// window a control loop just recorded can be read on the machine that recorded it with nothing
/// else running.
///
/// # §74's five counters, reporting
///
/// `Tally::commands` has carried this sentence since it was written:
///
/// > the day a control loop opens a ring with channels in it, these counters begin reporting with
/// > no further change here.
///
/// That is what this prints. On a ring with no action channels — every ring this repository could
/// produce until the control loop existed — `cycles`, `interventions`, `guard_trips`, `clamps` and
/// `estops` are all zero **because nothing writes them**, which is a different fact from a robot
/// that commanded a body and tripped nothing. The two are rendered differently here, and the
/// difference is read off the ring rather than assumed from the edition.
fn blackbox(args: &[String]) -> i32 {
    let Some(dir) = args.first() else {
        eprintln!("skynetk: blackbox <dir> — the directory `--blackbox` was pointed at.");
        return 1;
    };

    let path = std::path::Path::new(dir).join("ring");
    let store = match std::fs::read(&path) {
        Ok(store) => store,
        Err(error) => {
            eprintln!(
                "skynetk: {} could not be read: {error}\n\
                 A ring is created by `skynetk run --blackbox <dir>`; this is the file it writes.",
                path.display()
            );
            return 1;
        }
    };

    let ring = match skynet_blackbox::ring::Ring::open(&store) {
        Ok(ring) => ring,
        Err(error) => {
            eprintln!(
                "skynetk: {} is not a ring this edition can read: {error:?}",
                path.display()
            );
            return 1;
        }
    };

    let geometry = ring.geometry();
    println!("ring       {}", path.display());
    println!(
        "shape      {} joints, {} channels — {} slots of {} bytes",
        geometry.shape.joints, geometry.shape.channels, geometry.capacity, geometry.record_size,
    );
    println!(
        "written    {} record(s)",
        ring.next_sequence().saturating_sub(1)
    );
    println!();

    // From the first sequence the ring still holds, which is what a window is. Counting from 1 on a
    // wrapped ring would report a window that includes records the ring overwrote.
    let tally = skynet_blackbox::tally::tally(&ring, &store, 1);

    if !tally.commands {
        println!("This ring has no action channels, so it cannot hold a control cycle.");
        println!(
            "Every counter below would be zero because nothing writes them — which is not the"
        );
        println!("same fact as a robot that commanded a body and tripped nothing (§74.3).");
        return 0;
    }

    println!("§74's counters, off this ring:");
    println!("  cycles          {}", tally.cycles);
    println!("  interventions   {}", tally.interventions);
    println!("  guard trips     {}", tally.guard_trips);
    println!("  clamps          {}", tally.clamps);
    println!("  estops          {}", tally.estops);

    // How many of those cycles a policy actually commanded. §26 records `command_seq`, and a cycle
    // that carries zero is one where the ring was empty and the kernel held.
    //
    // This is the difference between a conformance figure and a number that looks like one. A robot
    // whose policy never spoke has an intervention count of zero for the same reason it has a
    // command count of zero, and the ratio comes out at a perfect 1.0000 — which reads, on any page
    // that renders it, as a robot that did everything right. It did nothing at all.
    let commanded = ring
        .ordered(&store)
        .into_iter()
        .filter(|record| match &record.body {
            skynet_blackbox::record::Body::Cycle(cycle) => cycle.command_seq != 0,
            _ => false,
        })
        .count() as u64;

    if tally.cycles > 0 {
        println!();
        if commanded == 0 {
            println!(
                "  No policy commanded anything in this window: all {} cycles held, because",
                tally.cycles
            );
            println!(
                "  §25's ring had nothing in it. §15's field conformance is the ratio of cycles"
            );
            println!(
                "  the kernel did not have to intervene in, and over a window where nothing was"
            );
            println!(
                "  asked for it would be 1.0000 — which is arithmetic rather than a measurement,"
            );
            println!("  so it is not printed.");
        } else {
            // §15's figure, and this is the first place its numerator and denominator have both
            // been real numbers on a machine.
            let conformance = 1.0 - (tally.interventions as f64 / tally.cycles as f64);
            println!(
                "  field conformance  {conformance:.4}  ({} of {} cycles uninterrupted, {} commanded)",
                tally.cycles - tally.interventions,
                tally.cycles,
                commanded,
            );
            println!(
                "  — over this window only. A certification figure is a fleet's and is the Hub's"
            );
            println!("    to compute; nothing here signs anything (§19).");
        }
    }

    if tally.truncated {
        println!();
        println!("The window wrapped: records older than the first one here were overwritten.");
        println!("That is what a ring is, and it is reported rather than left to be inferred.");
    }
    if tally.holes > 0 {
        println!();
        println!(
            "{} record(s) are missing from inside the window. A hole is a record that was written",
            tally.holes
        );
        println!("and is not here, which is a defect rather than the window moving on.");
    }

    0
}

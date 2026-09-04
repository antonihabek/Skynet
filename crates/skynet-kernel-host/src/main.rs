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
//! **The ring is private to this process.** §25 has the runtime create the ring and hand the fd to
//! the sandboxed policy, and there is no policy to hand it to — `skynet-ring-shm` is the `memfd`
//! that would carry it and it is Linux-only. So this binary allocates the ring in its own address
//! space and says so on every run, rather than printing a path nothing can open.
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
    resolve_limits, Available, Envelope, OperatorProfile, ResolveLimitsParams, Tier, TransportClamp,
};
use skynet_kernel_host::driver::{Actuator, Rhal};
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
  --quiet               only print the summary.

  The ring is private to this process: there is no policy runtime to hand it to, so
  every cycle reads an empty ring and the kernel holds. That is the state §33.1
  describes, and it is what this binary can honestly demonstrate today.";

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
    quiet: bool,
}

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
        Some("--help") | Some("-h") | None => {
            println!("{USAGE}");
            return 0;
        }
        Some(other) => return fail(&format!("`{other}` is not a subcommand. It is `run`.")),
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
        println!(
            "ring       private to this process — no policy runtime exists to receive it (§25)"
        );
        println!();
    }

    // The ring, allocated here. See the module header: on a robot this is a `memfd` handed to the
    // sandbox, and there is no sandbox.
    let words: Vec<AtomicU32> = (0..words_for(16)).map(|_| AtomicU32::new(0)).collect();
    let ring = match Ring::create(&words, 16) {
        Ok(ring) => ring,
        Err(error) => {
            return fail(&format!(
                "the ring could not be created: {}",
                error.as_str()
            ))
        }
    };
    let mut consumer = ring.consumer();

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

        let started = Instant::now();
        let now_ns = origin.elapsed().as_nanos() as u64;
        let turn = host.step(now_ns, &mut body, &mut consumer, &limits, Some(&envelope));

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

//! `skynetp` — the process that fills §25's command ring.
//!
//! ```text
//! skynetp run [--hz 100] [--cycles N] [--channels N] [--mode position|velocity|torque]
//!             [--backend hold|trajectory|approach]
//!             [--setpoints "0:0.1,0.2;40:0.3,0.4"] [--target 0.4,-0.3] [--gain 0.05]
//!             [--ring-socket <path>]
//! ```
//!
//! # What running it proves, and what it does not
//!
//! It turns a producer at a declared rate and publishes real commands into a real ring, with a real
//! sequence advancing and a real mask. That is the half of §25 that has never existed.
//!
//! **`--ring-socket` is the deployment §25 describes, and it exists now.** The runtime creates the
//! ring, this process connects to its socket, receives the descriptor over `SCM_RIGHTS` and maps it
//! **writable**; the runtime's own mapping carries no `PROT_WRITE`, so *the policy is the only
//! writer* is a property of the page tables rather than an agreement between two programs.
//!
//! A draft of this binary once had `--ring-fd`, and it was removed rather than shipped: it adopted a
//! descriptor **nobody published**, which is half a handshake, and §79.7 records exactly what half
//! of one costs — the route answered, the sender backed off politely, no bytes moved, and the
//! failure was indistinguishable from patience. What made the option right this time is that the
//! other half was built with it: `skynet_kernel_host::handoff::Port` creates the mapping and offers
//! it, and the two binaries have been run against each other.
//!
//! **Both rings arrive, or neither.** The command ring is one of two descriptors in that message;
//! the other is the observation ring, mapped **read-only** here, and it is what makes `--backend
//! approach` possible outside a test. A closed-loop policy that received the commands and not the
//! body would be commanding blind, which is why they cross together rather than in two messages.
//!
//! **A frame pool, when one comes with the rings, is shown to the backend every turn** (§47.6): the
//! newest frame is taken on a loan, copied, and the loan given back before the backend decides. The
//! summary says how many turns were shown one, and says nothing about frames when no pool came.
//!
//! **Without a socket it publishes into a ring nobody reads**, in its own address space, and says
//! so on the last line rather than leaving a reader to assume a robot moved. `approach` refuses to
//! run at all there: a backend that closes a loop on a body it cannot see holds for ever, and a
//! policy that holds because it was never given anything looks exactly like one that decided to.
//!
//! **What runs in one process is the protocol**, and it still does:
//! `the_whole_control_path_in_one_process` puts this producer and `skynet-kernel-host`'s consumer on
//! one ring and drives a command from a backend, through the eight guards, to a driver. What the
//! split adds is not the wire — it is the mapping being made by one process and written by another,
//! which is the sentence §80 carried as open until the hand-off existed.

use std::sync::atomic::AtomicU32;
use std::time::{Duration, Instant};

use skynet_policy_host::backend::{Approach, Commanding, Hold, Trajectory};
use skynet_policy_host::rendezvous;
use skynet_policy_host::run::{PolicyHost, Summary, SystemClock, Tick};
use skynet_ring::{mode, words_for, Ring, MAX_CHANNELS};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(run(&args));
}

fn fail(message: &str) -> i32 {
    eprintln!("skynetp: {message}");
    1
}

/// One parsed option list.
struct Options {
    hz: f64,
    cycles: Option<u64>,
    channels: usize,
    mode: u32,
    backend: String,
    setpoints: Vec<(u64, Vec<f32>)>,
    target: Vec<f32>,
    gain: f32,
    ring_socket: Option<std::path::PathBuf>,
    quiet: bool,
}

/// `"0:0.1,0.2;40:0.3,0.4"` — a tick, a colon, comma-separated values, semicolons between rows.
///
/// Parsed strictly. A row this cannot read is an error rather than a skipped row: a sweep that
/// silently dropped its third setpoint is a robot that moved somewhere nobody asked it to.
fn parse_setpoints(text: &str) -> Result<Vec<(u64, Vec<f32>)>, String> {
    let mut rows = Vec::new();
    for row in text.split(';').filter(|part| !part.trim().is_empty()) {
        let (at, values) = row
            .split_once(':')
            .ok_or_else(|| format!("a setpoint row wants `tick:v1,v2`, and this is `{row}`"))?;
        let at: u64 = at
            .trim()
            .parse()
            .map_err(|_| format!("`{}` is not a tick number", at.trim()))?;
        let mut parsed = Vec::new();
        for value in values.split(',').filter(|part| !part.trim().is_empty()) {
            parsed.push(
                value
                    .trim()
                    .parse::<f32>()
                    .map_err(|_| format!("`{}` is not a number", value.trim()))?,
            );
        }
        if parsed.is_empty() {
            return Err(format!("the row at tick {at} commands nothing"));
        }
        rows.push((at, parsed));
    }
    if rows.is_empty() {
        return Err("--setpoints was given and holds no rows".to_string());
    }
    Ok(rows)
}

/// Everything that has to be true whether it came from a flag or from the environment.
///
/// Split out of `parse` for exactly that reason: §25's rendezvous supplies the channel count on a
/// robot, and a check that only ran over the command line would let a policy the daemon started
/// past a rule an operator running the same binary by hand is held to.
fn check(options: &Options) -> Result<(), String> {
    if options.hz <= 0.0 {
        return Err("--hz must be a positive rate".to_string());
    }
    if options.channels > MAX_CHANNELS {
        return Err(format!(
            "--channels is {} and this ring carries {MAX_CHANNELS}",
            options.channels
        ));
    }
    match options.backend.as_str() {
        "hold" => {}
        "trajectory" => {
            if options.setpoints.is_empty() {
                return Err("--backend trajectory wants --setpoints".to_string());
            }
            let widest = options
                .setpoints
                .iter()
                .map(|(_, values)| values.len())
                .max()
                .unwrap_or(0);
            if widest > options.channels {
                return Err(format!(
                    "a setpoint row commands {widest} channels and the lease grants {}",
                    options.channels
                ));
            }
        }
        "approach" => {
            if options.target.is_empty() {
                return Err("--backend approach wants --target, which is where it is going".into());
            }
            if options.target.len() > options.channels {
                return Err(format!(
                    "--target names {} channels and the lease grants {}",
                    options.target.len(),
                    options.channels
                ));
            }
            if !(options.gain > 0.0 && options.gain <= 1.0) {
                // Refused here rather than clamped in the backend, which clamps for its own safety:
                // a gain somebody typed and this build silently changed is a robot moving at a rate
                // nobody chose.
                return Err(
                    "--gain closes a fraction of the distance each cycle, so it is above 0 \
                            and at most 1"
                        .to_string(),
                );
            }
        }
        other => return Err(format!("`{other}` is not a backend this build has")),
    }

    Ok(())
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        hz: 100.0,
        cycles: None,
        channels: 0,
        mode: mode::POSITION,
        backend: "hold".to_string(),
        setpoints: Vec::new(),
        target: Vec::new(),
        gain: 0.1,
        ring_socket: None,
        quiet: false,
    };

    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let value = |name: &str| -> Result<String, String> {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| format!("{name} wants a value"))
        };
        match arg {
            "--hz" => {
                options.hz = value("--hz")?
                    .parse()
                    .map_err(|_| "--hz wants a number".to_string())?;
                index += 2;
            }
            "--cycles" => {
                options.cycles = Some(
                    value("--cycles")?
                        .parse()
                        .map_err(|_| "--cycles wants a number".to_string())?,
                );
                index += 2;
            }
            "--channels" => {
                options.channels = value("--channels")?
                    .parse()
                    .map_err(|_| "--channels wants a number".to_string())?;
                index += 2;
            }
            "--mode" => {
                options.mode = match value("--mode")?.as_str() {
                    "position" => mode::POSITION,
                    "velocity" => mode::VELOCITY,
                    "torque" => mode::TORQUE,
                    other => return Err(format!("`{other}` is not a control mode")),
                };
                index += 2;
            }
            "--backend" => {
                options.backend = value("--backend")?;
                index += 2;
            }
            "--setpoints" => {
                options.setpoints = parse_setpoints(&value("--setpoints")?)?;
                index += 2;
            }
            "--target" => {
                options.target = value("--target")?
                    .split(',')
                    .filter(|part| !part.trim().is_empty())
                    .map(|part| {
                        part.trim()
                            .parse::<f32>()
                            .map_err(|_| format!("`{}` is not a number", part.trim()))
                    })
                    .collect::<Result<Vec<f32>, String>>()?;
                index += 2;
            }
            "--gain" => {
                options.gain = value("--gain")?
                    .parse()
                    .map_err(|_| "--gain wants a number".to_string())?;
                index += 2;
            }
            "--ring-socket" => {
                options.ring_socket = Some(std::path::PathBuf::from(value("--ring-socket")?));
                index += 2;
            }
            "--quiet" => {
                options.quiet = true;
                index += 1;
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }

    Ok(options)
}

fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("run") => {}
        Some(other) => return fail(&format!("unknown command `{other}`")),
        None => {
            eprintln!(
                "skynetp — the producer for the Safety Kernel's command ring (§25).\n\n\
                 usage: skynetp run [--hz 100] [--cycles N] [--channels N]\n\
                 \x20                  [--mode position|velocity|torque]\n\
                 \x20                  [--backend hold|trajectory|approach]\n\
                 \x20                  [--setpoints \"0:0.1;40:0.3\"] [--target 0.1,0.2] [--gain 0.1]\n\
                 \x20                  [--ring-socket <path>] [--quiet]\n\n\
                 --ring-socket takes both rings from a runtime that is offering them\n\
                 (`skynetk run --policy-socket <path>`): the command ring this process writes\n\
                 and the observation ring it reads. Without it the ring is this process's own,\n\
                 nothing reads what goes in, and there is no body to close a loop on.\n\n\
                 SKYNET_RING_SOCKET and SKYNET_RING_CHANNELS say the same as --ring-socket and\n\
                 --channels, and are how a policy `skynetd` started finds the runtime: a package\n\
                 declares its own args, so the rendezvous arrives in the unit's environment\n\
                 rather than on a command line the supervisor edited. A flag always wins."
            );
            return 2;
        }
    }

    let mut options = match parse(&args[1..]) {
        Ok(options) => options,
        Err(error) => return fail(&error),
    };

    // §25's rendezvous, from the environment, when nothing was passed on the command line.
    //
    // This is how a policy the **daemon** starts finds the runtime. A package declares its own
    // `runtime.args` (§8) and `skynetd` appending to them would be the supervisor editing somebody
    // else's command line, so the path and the channel count arrive as `SKYNET_RING_SOCKET` and
    // `SKYNET_RING_CHANNELS` — set on the unit, and only for the one package §37.1 resolved a
    // commanding plan for.
    //
    // A flag always wins: an operator running this by hand is the one case where the environment
    // may be describing a different robot than the one they mean.
    if options.ring_socket.is_none() {
        if let Some(path) = std::env::var_os("SKYNET_RING_SOCKET") {
            options.ring_socket = Some(std::path::PathBuf::from(path));
        }
    }
    if options.channels == 0 {
        if let Some(declared) = std::env::var("SKYNET_RING_CHANNELS")
            .ok()
            .and_then(|text| text.parse::<usize>().ok())
        {
            options.channels = declared;
        }
    }
    if let Err(problem) = check(&options) {
        return fail(&problem);
    }

    let lease = Commanding {
        channels: options.channels,
        // No ring is attached, so nothing would be checked against a budget.
        observation_budget_ms: None,
        frame_budget_ms: None,
        mode: options.mode,
    };
    if !lease.valid() {
        return fail("the lease is not one this ring can carry");
    }

    if options.backend == "approach" {
        if options.target.is_empty() {
            return fail("--backend approach needs --target, which is where it is going");
        }
        if options.ring_socket.is_none() {
            // Refused rather than run into a hold on every cycle. A closed-loop backend with no
            // body reads `Body::Absent` and holds for ever, which looks like a policy that has
            // decided to hold rather than one that was never given anything to decide on.
            return fail(
                "--backend approach closes a loop on the body, and without --ring-socket there is \
                 no observation ring to read it from",
            );
        }
    }

    let slots = 16;

    // §25's ring, and which of the two this run has is the whole difference between a producer that
    // is driving a robot and one that is talking to itself.
    //
    // **Received**, when `--ring-socket` names a runtime's socket: the descriptor arrives over
    // `SCM_RIGHTS`, this process maps it **writable**, and the runtime that created it maps it
    // read-only. That direction is not a convention — the page tables are what make "the policy is
    // the only writer" true across a trust boundary.
    //
    // **Private**, otherwise: a ring in this process's own memory, which is what every run of this
    // binary was until the hand-off existed, and it says so on the line below.
    let (shared, body, frames) = match options.ring_socket.as_deref() {
        None => (None, None, None),
        Some(path) => match rendezvous::adopt_from(path, slots) {
            Ok(adopted) => (Some(adopted.commands), Some(adopted.body), adopted.frames),
            // Refused rather than fallen back to a private ring. A policy asked to join a robot's
            // ring and quietly publishing into its own address space is a producer that looks like
            // it is commanding and is not.
            Err(error) => return fail(&error),
        },
    };

    // The body, when one was handed over. A closed-loop backend with no observation ring sees
    // `Body::Absent` and holds — which is the honest answer rather than a degenerate one: a policy
    // closing a loop on a body it cannot see would be inventing the half of the loop that matters.
    let watching = match body.as_ref() {
        None => None,
        Some(mapping) => match skynet_obs::Ring::open(mapping.words()) {
            Ok(ring) => Some(ring),
            Err(error) => {
                return fail(&format!(
                    "what arrived on the observation descriptor is not a ring: {}",
                    error.as_str()
                ))
            }
        },
    };
    let mut observations = watching.map(|ring| ring.consumer());

    let words: Vec<AtomicU32> = (0..words_for(slots)).map(|_| AtomicU32::new(0)).collect();
    let ring = match shared.as_ref() {
        Some(mapping) => match mapping.ring() {
            Ok(ring) => ring,
            Err(error) => return fail(&format!("the ring that arrived is not one: {error}")),
        },
        None => match Ring::create(&words, slots) {
            Ok(ring) => ring,
            Err(error) => {
                return fail(&format!(
                    "the ring could not be created: {}",
                    error.as_str()
                ))
            }
        },
    };
    let mut producer = ring.producer();

    let period_ns = (1_000_000_000.0 / options.hz) as u64;
    let period = Duration::from_nanos(period_ns);

    if !options.quiet {
        let ring_is = match options.ring_socket.as_deref() {
            Some(path) => format!("ring received from {}", path.display()),
            None => "ring private to this process".to_string(),
        };
        println!(
            "skynetp: {} at {:.0} Hz, {} channel(s), {ring_is}",
            options.backend, options.hz, options.channels
        );
        // Said once, on the line after the ring, because it is a fact about what this process can
        // see rather than a per-cycle event. A pool that has published nothing says so: §47.6's
        // sentinel is what distinguishes *no frame yet* from a frame of zeros, and reporting the
        // second as the first is the one lie the sentinel exists to prevent.
        if let Some(pool) = frames.as_ref() {
            let newest = match pool.latest() {
                Some(handle) => format!("newest frame in slot {}", handle.slot),
                None => "no frame published yet".to_string(),
            };
            println!(
                "skynetp: frame pool \"{}\" received with the rings, {newest}",
                pool.stream()
            );
        }
    }

    // The observation and the frame pool reach the backend through here, which is why the closure
    // takes them rather than capturing them: `approach` is a function of the body and the other two
    // are not, and a consumer captured by one of them would be a borrow held for the life of the run.
    type Turn<'a> = Box<
        dyn FnMut(
                &mut skynet_ring::Producer<'_>,
                Option<&mut skynet_obs::Consumer<'_>>,
                Option<&skynet_frames::Reader>,
            ) -> (Tick, Summary)
            + 'a,
    >;
    let mut host: Turn<'_> = match options.backend.as_str() {
        "trajectory" => {
            let rows: Vec<(u64, &[f32])> = options
                .setpoints
                .iter()
                .map(|(at, values)| (*at, values.as_slice()))
                .collect();
            let mut inner = PolicyHost::new(
                Trajectory::from_setpoints(&rows),
                SystemClock::default(),
                lease,
                period_ns,
            );
            Box::new(move |producer, body, frames| {
                (inner.turn_seeing(producer, body, frames), inner.summary())
            })
        }
        // The closed-loop one, and the only backend whose command on cycle n is a function of what
        // the body did on cycle n - 1. It was written when `skynet-obs` was and had no way to be
        // selected from a command line, because until the observation ring crossed a process
        // boundary there was nothing for it to close on outside a test.
        "approach" => {
            let mut inner = PolicyHost::new(
                Approach::toward(&options.target, options.gain),
                SystemClock::default(),
                lease,
                period_ns,
            );
            Box::new(move |producer, body, frames| {
                (inner.turn_seeing(producer, body, frames), inner.summary())
            })
        }
        _ => {
            let mut inner = PolicyHost::new(Hold, SystemClock::default(), lease, period_ns);
            Box::new(move |producer, body, frames| {
                (inner.turn_seeing(producer, body, frames), inner.summary())
            })
        }
    };

    let mut published = 0u64;
    let mut held = 0u64;
    let mut refused = 0u64;
    let mut turns = 0u64;
    let mut summary = Summary::default();

    loop {
        if let Some(limit) = options.cycles {
            if turns >= limit {
                break;
            }
        }
        let started = Instant::now();
        let (tick, so_far) = host(&mut producer, observations.as_mut(), frames.as_ref());
        summary = so_far;
        match tick {
            Tick::Published(_) => published += 1,
            Tick::Held(_) => held += 1,
            Tick::Silent => {}
            Tick::Refused(refusal) => {
                refused += 1;
                if !options.quiet {
                    eprintln!("skynetp: refused — {refusal:?}");
                }
            }
            Tick::RingError(error) => {
                return fail(&format!("the ring refused a command: {}", error.as_str()))
            }
        }
        turns += 1;
        if let Some(remaining) = period.checked_sub(started.elapsed()) {
            std::thread::sleep(remaining);
        }
        if options.cycles.is_none() && turns == u64::MAX {
            break;
        }
    }

    if !options.quiet {
        println!(
            "skynetp: {turns} turn(s) — {published} commanded, {held} held, {refused} refused"
        );
        // Only when a pool came with the rings: a policy that printed a frame line on every robot
        // would be claiming a camera that is not there.
        if frames.is_some() {
            println!(
                "skynetp: {} of {turns} turn(s) were shown a frame ({} missed, {} too old)",
                summary.frames, summary.frames_missed, summary.stale_frames
            );
        }
        // What became of the commands, and the two answers are not the same sentence. A private
        // ring has no consumer by construction. A received one has the runtime that handed it over
        // — and this process cannot say what that runtime did with a command, because §7's guards
        // are on the other side of the mapping and a producer that claimed to know would be
        // reporting a robot it cannot see.
        match options.ring_socket.as_deref() {
            None => println!(
                "skynetp: nothing read them — this ring is private to this process. \
                 §25 hands the descriptor to the policy, and this run was offered none."
            ),
            Some(path) => println!(
                "skynetp: published into the ring received from {}. What reached the body is the \
                 runtime's to report: every command crosses §7's guards before a driver sees it.",
                path.display()
            ),
        }
    }
    0
}

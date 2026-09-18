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
use skynet_contracts::media::Placement;
use skynet_kernel::{
    can_start, resolve_limits, Available, Envelope, OperatorProfile, ResolveLimitsParams, Tier,
    TransportClamp, WarningChannel,
};
use skynet_kernel_host::capture::{Camera, CaptureThread, Declared, RhalSensor};
use skynet_kernel_host::driver::{Actuator, Rhal};
use skynet_kernel_host::handoff;
use skynet_kernel_host::motion::Authority;
use skynet_kernel_host::recorder::{Recorder, DEFAULT_BUDGET};
use skynet_kernel_host::run::{Attached, Host};
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
  --frame-pool <spec>   create §47.6's frame pool as `<stream>:<slots>x<bytes>@<hz>`,
                        hand it over with the rings, and fill it from the driver's
                        sensor named <stream>, read at <hz> (§47.2's RHAL backend).
                        Every number is declared, not measured (§47.5), and the
                        sensor must be one the driver's descriptor declares.
                        Frames are not recorded: this build has no encoder, and
                        the summary names the gap rather than leaving it implied.
                        Needs --policy-socket: a pool nobody receives is a mapping.
  --lease <holder>      who holds this body (§25.2), as §88.2's first refusal compares it.
                        Without it every motion is refused `no_lease` — this process does
                        not own the lease table and will not guess who does.
  --operating-state <s> §27's state, as a supervisor would report it. Without it every
                        motion is refused `not_active`. The estop bit is read off the
                        body every cycle rather than declared here.
  --motion-token <tok>  §4's capability token this body's controller takes motions on.
                        Default: the one entry under `extensions` in the driver's own
                        descriptor, when it declares exactly one. A body declaring none
                        or several is named rather than guessed at.
  --quiet               only print the summary.

skynetk blackbox <dir>

  Read a ring back: its shape, what it holds, and §74's five counters. Needs no
  daemon and no socket — it opens the file `run --blackbox` wrote.

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
    frame_pool: Option<FramePool>,
    /// §88.2's first refusal, declared. See `--lease`.
    lease: Option<String>,
    /// §27's state, declared. See `--operating-state`.
    operating_state: Option<String>,
    /// §4's token, declared, or read off the descriptor when it names exactly one.
    motion_token: Option<String>,
    quiet: bool,
}

/// What `--frame-pool` declared: the stream, the pool's shape, and the rate to read it at.
struct FramePool {
    stream: String,
    geometry: skynet_frames::Geometry,
    hz: f64,
}

/// `"head_rgb:4x2073600@30"` — a stream name, a colon, slots, an `x`, bytes per slot, an `@`, a rate.
///
/// Parsed strictly, and every failure names the fix (§64). The numbers are a **declaration**
/// (§47.5): none is derived from anything this process can see. The rate in particular is required
/// rather than defaulted — a camera that renders on demand has no frame rate that is a property of
/// the body, and a default here would be this binary choosing one for it.
fn parse_frame_pool(text: &str) -> Result<FramePool, String> {
    let (stream, shape) = text.split_once(':').ok_or_else(|| {
        format!("--frame-pool wants `<stream>:<slots>x<bytes>@<hz>`, and this is `{text}`")
    })?;
    let (shape, hz) = shape.split_once('@').ok_or_else(|| {
        format!(
            "--frame-pool wants a capture rate after the shape — `{shape}@<hz>` — because the \
             pool is now filled from the driver and a stream is read at a rate somebody declared"
        )
    })?;
    let hz: f64 = hz
        .trim()
        .parse()
        .map_err(|_| format!("`{}` is not a capture rate", hz.trim()))?;
    if !(hz.is_finite() && hz > 0.0) {
        return Err(format!(
            "a capture rate of {hz} Hz reads no frames; declare a positive rate"
        ));
    }
    if stream.is_empty() || stream.len() > skynet_frames::MAX_NAME {
        return Err(format!(
            "`{stream}` is not a stream name: it must be between 1 and {} bytes, because the name \
             is written into the pool's header where a receiver reads it back",
            skynet_frames::MAX_NAME
        ));
    }
    let (slots, bytes) = shape.split_once('x').ok_or_else(|| {
        format!("--frame-pool wants `<slots>x<bytes>` after the stream name, and this is `{shape}`")
    })?;
    let slots: usize = slots
        .trim()
        .parse()
        .map_err(|_| format!("`{}` is not a slot count", slots.trim()))?;
    let slot_bytes: usize = bytes
        .trim()
        .parse()
        .map_err(|_| format!("`{}` is not a byte count", bytes.trim()))?;
    if slots == 0 || slot_bytes == 0 {
        return Err(
            "a pool wants at least one slot of at least one byte; a pool of neither holds no \
             frames and offering it would tell a policy it has a camera"
                .to_string(),
        );
    }
    Ok(FramePool {
        stream: stream.to_string(),
        geometry: skynet_frames::Geometry { slots, slot_bytes },
        hz,
    })
}

/// Check that the body has the stream a pool was declared for, before anything is offered.
///
/// A pool offered for a sensor the driver cannot read tells a policy it has a camera. So the
/// descriptor is asked — the driver's own statement of what the body is — and the refusal names
/// what it does declare. Where the descriptor gives an RGB sensor's size, a slot too small for one
/// frame is refused here rather than on every frame for the length of the run.
/// §88's authority, assembled from the driver's own descriptor and what the command line declared.
///
/// Three of its four fields are facts this process does not own — the lease table is `skynetd`'s,
/// §27's state is the supervisor's, and §4's token is the vendor's — so each is **declared** here
/// in the way `--tier` and `--velocity-max` already are, and each defaults to the reading that
/// refuses. See `crate::motion::Authority` for why that is the only honest default.
///
/// The motions are the one half this process can read for itself: they are in the descriptor the
/// driver publishes, which is where §84.1 puts them. A descriptor that does not decode is reported
/// rather than treated as a body with no motions, because those are different facts and the second
/// one is a lie an operator would act on.
fn authority_of(body: &mut Rhal, options: &Options) -> Result<Authority, String> {
    let text = body
        .describe()
        .map_err(|error| format!("the driver's descriptor could not be read: {error}"))?;
    let descriptor = skynet_json::Json::parse(&text)
        .map_err(|_| "the driver's descriptor is not JSON".to_string())
        .and_then(|json| {
            skynet_contracts::json::descriptor_from_json(&json)
                .map_err(|error| format!("the driver's descriptor does not decode: {error}"))
        })?;

    // Declared first, then the descriptor's single entry. **Never a choice between several**: a
    // body offering two tokens has not said which one its motions go to, and picking the first
    // would be this binary inventing a rule §88 does not have.
    let token = match (&options.motion_token, descriptor.extensions.as_slice()) {
        (Some(declared), _) => Some(declared.clone()),
        (None, [only]) => Some(only.clone()),
        (None, _) => None,
    };

    Ok(Authority {
        motions: descriptor.motions.clone(),
        state: options.operating_state.clone().unwrap_or_default(),
        lease_held_by: options.lease.clone(),
        token,
    })
}

/// One line about §88's route, for the report this binary prints before it starts turning.
///
/// It names what is missing rather than saying the route is absent, because every way a motion can
/// fail to reach a controller here has a different remedy: a body with no motions is not a
/// program-level body at all, a body with no token needs `--motion-token`, and a run with no lease
/// or no state has the two facts this process will not guess. A robot that silently refused every
/// motion and printed nothing would send an integrator to the policy.
fn describe_motion_route(authority: &Authority) -> String {
    if authority.motions.is_empty() {
        return String::from("none declared — this body is not commanded by named motions (§84.1)");
    }
    let names: Vec<&str> = authority
        .motions
        .iter()
        .map(|motion| motion.name.as_str())
        .collect();
    let mut missing: Vec<&str> = Vec::new();
    if authority.token.is_none() {
        missing.push("--motion-token (the descriptor names none, or more than one)");
    }
    if authority.lease_held_by.is_none() {
        missing.push("--lease");
    }
    if authority.state.is_empty() {
        missing.push("--operating-state");
    }
    if missing.is_empty() {
        return format!(
            "{} — {}, on `{}` (§88)",
            names.len(),
            names.join(", "),
            authority.token.as_deref().unwrap_or_default()
        );
    }
    format!(
        "{} — {}, and every one of them will be refused: this run declares no {} (§88.2)",
        names.len(),
        names.join(", "),
        missing.join(", no ")
    )
}

fn sensor_declared(body: &mut Rhal, pool: &FramePool) -> Result<(), String> {
    let stream = &pool.stream;
    if !body.has_sensors() {
        return Err(format!(
            "--frame-pool names `{stream}`, and this driver has no read_sensor entry point, so no \
             sensor can be read through the RHAL (§47.2). A camera read directly from V4L2 or \
             GenICam is a backend this build does not have."
        ));
    }
    let text = body.describe().map_err(|error| {
        format!(
            "the driver's descriptor could not be read, so `{stream}` cannot be checked: {error}"
        )
    })?;
    let descriptor = skynet_json::Json::parse(&text)
        .map_err(|_| "the driver's descriptor is not JSON".to_string())
        .and_then(|json| {
            skynet_contracts::json::descriptor_from_json(&json)
                .map_err(|error| format!("the driver's descriptor does not decode: {error}"))
        })?;
    let Some(sensor) = descriptor
        .sensors
        .iter()
        .find(|sensor| &sensor.name == stream)
    else {
        let names: Vec<&str> = descriptor
            .sensors
            .iter()
            .map(|sensor| sensor.name.as_str())
            .collect();
        return Err(format!(
            "--frame-pool names `{stream}` and the driver's descriptor declares {}. A pool for a \
             stream the body does not have would tell a policy it has a camera.",
            if names.is_empty() {
                "no sensors".to_string()
            } else {
                format!("only {}", names.join(", "))
            }
        ));
    };
    if let (Some(width), Some(height), "rgb") =
        (sensor.width, sensor.height, sensor.modality.as_str())
    {
        let needed = (width * height * 3.0) as usize;
        if needed > pool.geometry.slot_bytes {
            return Err(format!(
                "`{stream}` is declared {width}x{height} RGB — {needed} bytes a frame — and the \
                 pool's slots hold {}. Declare slots of at least {needed} bytes.",
                pool.geometry.slot_bytes
            ));
        }
    }
    Ok(())
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
        frame_pool: None,
        lease: None,
        operating_state: None,
        motion_token: None,
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
            "--lease" => {
                options.lease = Some(value()?);
                index += 2;
            }
            "--operating-state" => {
                options.operating_state = Some(value()?);
                index += 2;
            }
            "--motion-token" => {
                options.motion_token = Some(value()?);
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
            "--frame-pool" => {
                options.frame_pool = Some(parse_frame_pool(&value()?)?);
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
    // A pool with nobody to hand it to is a mapping this process talks to itself through. Refused
    // here rather than created and left unoffered, because the second is a run that looks like it
    // is serving frames and is not — §79.7's shape, at a smaller scale.
    if options.frame_pool.is_some() && options.policy_socket.is_none() {
        return Err(
            "--frame-pool creates a region for a policy to map, and without --policy-socket there \
             is nobody to hand it to. Add --policy-socket <path>, or drop --frame-pool"
                .to_string(),
        );
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

    // Before anything is printed or offered: a pool declared for a stream the body does not have
    // is refused here, while nothing has been created that would have to be taken back.
    if let Some(pool) = options.frame_pool.as_ref() {
        if let Err(message) = sensor_declared(&mut body, pool) {
            return fail(&message);
        }
    }

    // §88's half of what this loop has been told: the body's declared motions, read off its own
    // descriptor, and the three facts this process does not own. Read here, before anything is
    // offered or created, for the reason the frame pool's check is here — a descriptor that does
    // not decode is a reason to stop rather than to start and find out.
    let authority = match authority_of(&mut body, &options) {
        Ok(authority) => authority,
        Err(message) => return fail(&message),
    };
    let motion_route = describe_motion_route(&authority);

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
        // §86.3. `skynetk --velocity-max` names a ceiling and no joint's mode, so there is no map
        // to hand over: the per-channel figure is the whole of what this path knows.
        joint_modes: &[],
        source: Some("skynetk command line"),
    });

    let limits = resolve_limits(&ResolveLimitsParams {
        tier: options.tier,
        envelope: Some(envelope),
        transport,
        profile,
        // §38.2's session override is a supervisor's, not a control loop's. `skynetd` holds the
        // store and the clock that expires it; `skynetk` is handed a body, a plan and a rate on a
        // command line and runs them. A loop that read the override itself would be a second place
        // deciding what this robot's ceilings are, which is the one thing this module's own header
        // says must not happen.
        session: None,
        available: Available {
            imu: false,
            people_detection: false,
            legged: false,
            // §80. This host has no proximity producer — nothing here fills
            // `cycle.nearest_person_m` — and it says so rather than leaving the resolver to a
            // default that happens to be right. The day a producer exists, this is the line that
            // changes and the guard stops being inert.
            proximity_reading: false,
        },
        frames: None,
    });

    if !options.quiet {
        println!("driver     {}", options.driver.display());
        println!("joints     {joints}");
        println!("motions    {motion_route}");
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
    // §47.6's pool, created here, handed over below and filled by the camera the loop turns. A reader
    // that maps it before the first frame sees a pool with no frame yet, which is exactly what a
    // camera that has not delivered one looks like, and is the reason §47.6 has a sentinel for it
    // rather than slot 0.
    let mut camera = None;
    // §89.3's handle, when the driver declared the stream `concurrent_read`, and the thread it is
    // read on once the loop's clock exists. `capture` is joined before `body` drops, which is
    // `CaptureThread::join`'s condition.
    let mut concurrent = None;
    let mut capture: Option<CaptureThread> = None;
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

                if let Some(declared) = options.frame_pool.as_ref() {
                    // Refused rather than run without it, for the reason the ring above is.
                    let pool =
                        match skynet_frames::Writer::create(&declared.stream, declared.geometry) {
                            Ok(pool) => pool,
                            Err(error) => {
                                return fail(&format!("--frame-pool could not be created: {error}"))
                            }
                        };
                    // Two buffers on this side: the frame in flight to the pool, and one more so
                    // that a turn which reads never finds the sensor dry by its own doing. The pool
                    // is where frames are held for the policy, and its slots are the declaration
                    // §47.5 asks about.
                    let sensor = match RhalSensor::new(
                        0,
                        &Declared {
                            name: declared.stream.clone(),
                            hz: declared.hz,
                            buffers: 2,
                            slot_bytes: declared.geometry.slot_bytes,
                        },
                    ) {
                        Ok(sensor) => sensor,
                        Err(message) => return fail(&message),
                    };
                    if !options.quiet {
                        println!(
                            "frames     pool \"{}\" offered with the rings, {} × {} B, filled from \
                             the driver's sensor at {} Hz (§47.2)",
                            declared.stream,
                            declared.geometry.slots,
                            declared.geometry.slot_bytes,
                            declared.hz
                        );
                        println!(
                            "           not recorded: this build has no encoder, so every frame \
                             is a gap in the black box, and the summary names it (§47.4)"
                        );
                    }
                    // §89.1: where the stream is read is the driver's to say, and `skynet-dl` reads
                    // the declaration out of the driver's own descriptor. Anything but a declared
                    // `concurrent_read` — including a descriptor that would not answer — keeps the
                    // stream on the control thread, which is the conservative placement.
                    match body.concurrent_sensor(&declared.stream) {
                        Ok(reader) => {
                            if !options.quiet {
                                println!(
                                    "           read on a thread of its own: the driver declares \
                                     `concurrent_read` on it (§89.1)"
                                );
                            }
                            concurrent = Some(reader);
                        }
                        Err(refused) => {
                            if !options.quiet {
                                println!(
                                    "           read between cycles on the control thread, and \
                                     suspended if its reads keep costing the period (§89.2)"
                                );
                                println!("           {refused}");
                            }
                        }
                    }
                    if !options.quiet {
                        println!();
                    }
                    camera = Some(Camera::new(sensor, Some(pool)));
                }
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
    // Before the first cycle, because a motion arriving on cycle one is admitted against whatever
    // the loop knows at that moment — and a loop that learned its authority on cycle two would
    // refuse a request that was sound (§88.2).
    host.authorise(authority);
    let period = Duration::from_secs_f64(1.0 / options.hz);
    let origin = Instant::now();
    // §89.3: a declared stream leaves the loop here, before the first cycle, and comes back at the
    // end for the report. The camera moves with it — buffers, split and pool — and the loop keeps
    // only the pool's descriptors and a way to ask for its loans back.
    if let Some(reader) = concurrent.take() {
        if let Some(moving) = camera.take() {
            match CaptureThread::spawn(moving, reader, origin) {
                Ok(thread) => capture = Some(thread),
                Err(message) => return fail(&message),
            }
        }
    }
    let mut first_error: Option<String> = None;
    let mut stop_unavailable_seen = false;
    let mut recorder_failed = false;
    let mut camera_failed = false;

    let mut loans_revoked = 0u64;
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
            let frames = camera
                .as_ref()
                .and_then(Camera::pool_descriptors)
                .or_else(|| capture.as_ref().and_then(CaptureThread::pool_descriptors));
            if port.offer(mapping, body, frames) > 0 && !options.quiet {
                let took = match frames {
                    Some(_) => "took the rings and the frame pool",
                    None => "took the rings",
                };
                println!("policy     {took} ({} so far)", port.handed());
            }
        }

        // §47.6's loans, given back on behalf of a reader that cannot give them back itself. The
        // port answers `true` only when every policy that took the pool has exited — a pidfd from
        // the kernel, not the socket closing — and a slot reclaimed while a reader is alive is the
        // one thing the pool exists to prevent, so the condition is all-or-nothing and its argument
        // is in `Port::reap_departed`.
        if let Some(port) = port
            .as_mut()
            .filter(|_| camera.is_some() || capture.is_some())
        {
            // On a capture thread the revocation is a request the thread acts on at its next turn,
            // which is sound for the same reason revoking at all is: nobody is left to hold a loan.
            let revoked = port.reap_departed()
                && match (camera.as_mut(), capture.as_ref()) {
                    (Some(camera), _) => camera.revoke_pool_loans(),
                    (None, Some(thread)) => thread.request_revoke(),
                    (None, None) => false,
                };
            if revoked {
                loans_revoked += 1;
                if !options.quiet {
                    println!(
                        "policy     every policy that took the frame pool has exited — its loans \
                         are revoked (§47.6)"
                    );
                }
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
            Attached {
                observations: observations.as_mut(),
                camera: camera.as_mut(),
            },
        );

        if let Some(failure) = turn.read_error {
            if first_error.is_none() {
                // The code as well as the sentence: a reader of this line and a reader of the
                // recording should see the same fact, and the record keeps the number.
                first_error = Some(format!("{} ({})", failure.detail, failure.code));
            }
        }
        stop_unavailable_seen |= turn.stop_unavailable;
        if let Some(capture) = turn.capture.as_ref() {
            camera_failed |= capture.refused.is_some();
        }

        // §88.3. Every request and every verdict, accepted and refused alike, and printed as well
        // as recorded: a motion is a thing an operator asked for and the answer is a sentence,
        // which is not true of any of the numbers below.
        if let Some(motion) = turn.motion.as_ref() {
            if !options.quiet {
                println!("motion     {}", motion.detail);
            }
            if let Some(recorder) = recorder.as_mut() {
                if let Err(error) = recorder.motion(now_ns, motion) {
                    if first_error.is_none() {
                        first_error = Some(format!("the black box stopped recording: {error}"));
                    }
                    recorder_failed = true;
                }
            }
        }

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
        // §89.2, now that the cycle's length is known: a control-thread read that made it miss is
        // counted, and a stream whose reads keep doing so is suspended — once, printed and recorded.
        if let Some(camera) = camera.as_mut() {
            if let Some(verdict) =
                camera.settle(elapsed.as_nanos() as u64, period.as_nanos() as u64)
            {
                let stream = options
                    .frame_pool
                    .as_ref()
                    .map_or("the stream", |declared| declared.stream.as_str());
                let detail = format!(
                    "suspended at read {}: {} of the last {} reads on the control thread cost \
                     the {:.2} ms period, and the driver does not declare `concurrent_read` \
                     (§89.2)",
                    verdict.suspended_at.unwrap_or(0),
                    verdict.misses,
                    verdict.window,
                    period.as_secs_f64() * 1000.0
                );
                println!("frames     {stream} {detail}");
                if let Some(recorder) = recorder.as_mut() {
                    if let Err(error) = recorder.capture_suspended(
                        origin.elapsed().as_nanos() as u64,
                        stream,
                        &detail,
                    ) {
                        if first_error.is_none() {
                            first_error = Some(format!("the black box stopped recording: {error}"));
                        }
                        recorder_failed = true;
                    }
                }
            }
        }
        if elapsed > period {
            host.note_overrun();
        } else {
            std::thread::sleep(period - elapsed);
        }
        cycle += 1;
    }

    let motions = host.motions();
    let (refused, delivered, overdue) = (motions.refused, motions.delivered, motions.overdue);
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
    // §88.3's completeness argument, as three numbers rather than one: a body whose report said
    // only how many motions ran could not distinguish a skill that asked for nothing from a skill
    // that was refused ten times, and the refusal is the more interesting record of the two.
    if delivered + refused + overdue > 0 {
        println!("motions         {delivered} delivered, {refused} refused, {overdue} overdue");
    }

    // Joined here, before the report and long before `body` drops: the capture thread reads through
    // a handle to the driver's context, and §89.3's promise is that nothing but `read_sensor` is ever
    // called from it — the context's close included.
    if let Some(thread) = capture.take() {
        match thread.join() {
            Ok(joined) => camera = Some(joined),
            Err(message) => {
                if first_error.is_none() {
                    first_error = Some(message);
                }
                camera_failed = true;
            }
        }
    }
    if let Some(camera) = camera.as_mut() {
        camera_failed |= report_camera(camera);
    }

    // §47.6's loans, and whether this run could ever have given one back. Printed even at zero,
    // for `report_camera`'s reason: a run in which no policy ever exited and a run in which the
    // kernel could not tell would otherwise print the same thing, and they are opposite facts.
    if let Some(port) = port.as_ref() {
        if port.handed() > 0 {
            println!("loans revoked   {loans_revoked} (§47.6, on a policy's last process exiting)");
            if port.unidentified() > 0 {
                println!(
                    "                {} of {} policies could not be named by the kernel, so no \
                     loan was ever revocable on this run. A kernel older than 6.5 has no \
                     SO_PEERPIDFD, and there is no race-free substitute — a crashed policy keeps \
                     the slots it held, and `published` above counts what that cost",
                    port.unidentified(),
                    port.handed()
                );
            }
        }
    }

    // Sealed on the way out, because a window nobody sealed is a ring the next run overwrites.
    // §13's reason for sealing at shutdown is its reason for sealing at an incident: the records
    // are only evidence once they are a file.
    if let Some(recorder) = recorder.as_ref() {
        match recorder.seal(Reason::Shutdown, origin.elapsed().as_nanos() as u64) {
            Ok((sealed, path)) => {
                println!(
                    "\nsealed          {} — {} bytes",
                    path.display(),
                    sealed.bytes.len(),
                );
                match write_limits_beside(&path, &limits, Some(&envelope)) {
                    Ok(beside) => println!("limits          {}", beside.display()),
                    Err(error) => println!(
                        "the window was sealed and its limits could not be written beside it: \
                         {error}"
                    ),
                }
            }
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

    // A run that could not read the body is not a successful run, however many cycles it turned —
    // and nor is one whose declared stream could not be read or published. A camera a policy was
    // offered and did not get is a fault, not a statistic.
    if summary.read_failures > 0 || recorder_failed || camera_failed {
        return 1;
    }
    0
}

/// The summary's capture lines, and whether the stream failed.
///
/// Every count is printed, including the zeros, because a stream that read nothing and a stream
/// nobody declared would otherwise print the same thing: nothing.
fn report_camera(camera: &mut Camera) -> bool {
    let sensor = camera.sensor().counts();
    let counts = camera.counts();
    println!();
    // Where it was read first, because every number below means something different on each (§89).
    match (camera.placement(), camera.suspension()) {
        (Placement::OwnThread, _) => println!(
            "read on         a thread of its own — the driver declares `concurrent_read` (§89.3)"
        ),
        (_, Some(verdict)) => println!(
            "read on         nothing after read {} — suspended, {} of the last {} reads cost \
             the period (§89.2)",
            verdict.suspended_at.unwrap_or(0),
            verdict.misses,
            verdict.window
        ),
        _ => println!("read on         the control thread, between cycles (§89.2)"),
    }
    println!(
        "frames read     {} of {} reads ({} repeated a frame already seen)",
        sensor.frames, sensor.reads, sensor.repeats
    );
    println!(
        "frames starved  {} (due with every buffer held, §47.5)",
        sensor.starved
    );
    println!("read failures   {} (sensor)", sensor.failures);
    println!(
        "published       {} to the pool, {} dropped by it (every slot on loan), {} refused",
        counts.published, counts.pool_dropped, counts.pool_failed
    );
    match camera.close() {
        Some(gap) => println!(
            "not recorded    {} frame(s), sequence {} to {} — this build has no encoder (§47.4)",
            gap.frames, gap.from_seq, gap.to_seq
        ),
        None => println!("not recorded    nothing was captured, so nothing was left out"),
    }
    if let Some(failure) = camera.sensor().first_failure() {
        println!(
            "first sensor failure: {} ({})",
            failure.detail, failure.code
        );
    }
    if let Some(refusal) = camera.first_refusal() {
        println!("first pool refusal: {refusal}");
    }
    // A stream that was due and read nothing is a stream that failed, even with no error: a
    // declaration nobody could satisfy is the finding.
    sensor.failures > 0 || (sensor.reads > 0 && sensor.frames == 0)
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

/// Write the limits this loop ran under beside the window it sealed.
///
/// # Why it is here and not in the recording
///
/// §26 keeps the two apart on purpose: a window carries what happened, and the ceilings it
/// happened under are *resolved* — from the embodiment descriptor, the operator profile and the
/// transport clamp — so a recording names them through its embodiment digest without carrying
/// them. Replay then takes them as a separate input, which is also the check: a window replayed
/// against a profile that was not in force diverges loudly rather than agreeing quietly.
///
/// # Why it has to be written at all
///
/// Because otherwise **nothing can state them**. This process resolves the limits from its own
/// flags; `skynetd` resolves its own from the descriptor and whatever transport plan is in
/// force, which on a bench is neither the same set nor a wrong one — it is a different robot’s
/// answer to a different question. So a window sealed here and taken away could be inspected and
/// never replayed, which is half of what §13 says a recording is for.
///
/// It is the **same document** `GET /limits` returns, deliberately: `skynet blackbox replay`
/// then has one shape to read whether the limits came off a robot, out of a `skynet doctor`
/// bundle, or off the disk beside the window.
///
/// What it is not is *bound* to the window. Nothing ties this file to that one but the name —
/// the recording carries an embodiment digest and a limits document carries none — so a
/// replay is evidence about the limits it was handed, and a reader who wants more than that
/// must check the provenance `skynet blackbox inspect` prints.
fn write_limits_beside(
    sealed: &std::path::Path,
    limits: &skynet_kernel::ResolvedLimits<'_>,
    envelope: Option<&skynet_kernel::Envelope<'_>>,
) -> Result<std::path::PathBuf, String> {
    let mut document = skynet_contracts::safety::limits_to_json(limits, None);
    // Beside the resolved set rather than folded into it, because it is not a resolved
    // ceiling: `evaluate_cycle` reads `watchdog_ms` off the **declaration**, and a replay
    // handed the resolved set alone has the watchdog guard active with no window to measure
    // staleness against. `GET /limits` does not carry this and does not need to — a console
    // renders ceilings — so it is added here rather than by widening that route’s document.
    if let Some(envelope) = envelope {
        let mut held = skynet_json::Json::obj();
        let number = |value: Option<f64>| match value {
            Some(value) => skynet_json::Json::Num(value),
            None => skynet_json::Json::Null,
        };
        held.set(
            "joint_velocity_max_rad_s",
            number(envelope.joint_velocity_max_rad_s),
        );
        held.set("joint_torque_max_nm", number(envelope.joint_torque_max_nm));
        held.set(
            "base_velocity_max_m_s",
            number(envelope.base_velocity_max_m_s),
        );
        held.set("watchdog_ms", number(envelope.watchdog_ms));
        held.set(
            "estop",
            match envelope.estop {
                Some(fitted) => skynet_json::Json::Bool(fitted),
                None => skynet_json::Json::Null,
            },
        );
        held.set(
            "source",
            match envelope.source {
                Some(source) => skynet_json::Json::from(source),
                None => skynet_json::Json::Null,
            },
        );
        document.set("envelope", held);
    }
    let text = document
        .to_canonical()
        .map_err(|_| String::from("the limit set would not serialise"))?;
    let beside = sealed.with_extension("limits.json");
    std::fs::write(&beside, text).map_err(|error| error.to_string())?;
    Ok(beside)
}

//! `skynetp` — the process that fills §25's command ring.
//!
//! ```text
//! skynetp run [--hz 100] [--cycles N] [--channels N] [--mode position|velocity|torque]
//!             [--backend hold|trajectory|approach|motion|remote]
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

use skynet_policy_host::backend::{
    Approach, Commanding, Hold, Inference, Motion as MotionBackend, Trajectory,
};

/// The model runners `--policy` can name, chosen by the artefact's own `kind`.
///
/// An enum rather than `Box<dyn Session>` for the reason [`skynet_infer::session::Session`] takes an
/// output slice: this is a per-control-period path, and a virtual call plus an allocation per cycle
/// is exactly the kind of cost §23.3 warns about. It is also the honest shape — the set of runners
/// this build has is closed and known at compile time, and `skynet_infer::KINDS` is the one list of
/// them.
enum Runner {
    /// `kind: "mlp"` — dense feed-forward, in `skynet-infer`, no library needed.
    Mlp(skynet_infer::Mlp),
    /// `kind: "onnx"` — §23.3's production runner, an ONNX Runtime session loaded at run time.
    Onnx(skynet_ort::Onnx),
}

impl skynet_infer::Session for Runner {
    fn input_width(&self) -> usize {
        match self {
            Runner::Mlp(inner) => inner.input_width(),
            Runner::Onnx(inner) => inner.input_width(),
        }
    }

    fn output_width(&self) -> usize {
        match self {
            Runner::Mlp(inner) => inner.output_width(),
            Runner::Onnx(inner) => inner.output_width(),
        }
    }

    fn run(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), skynet_infer::SessionError> {
        match self {
            Runner::Mlp(inner) => inner.run(input, output),
            Runner::Onnx(inner) => inner.run(input, output),
        }
    }

    fn describe(&self) -> &str {
        match self {
            Runner::Mlp(inner) => inner.describe(),
            Runner::Onnx(inner) => inner.describe(),
        }
    }
}

/// Read an artefact and open whichever runner its `kind` names.
///
/// The **file** chooses, not a flag. A `--runner onnx` beside a file saying `mlp` would be two
/// records of one fact, and the failure when they disagreed would be a model run by something that
/// was not asked to run it — which is precisely what each runner's `kind` check exists to prevent.
///
/// `path` is the artefact's own path, and its parent is what a relative `model` resolves against, so
/// a policy directory can be moved or mounted without editing what is inside it.
fn load_runner(
    label: &str,
    text: &str,
    path: &std::path::Path,
) -> Result<(Runner, skynet_infer::Inputs), String> {
    // Peeked rather than consumed: each runner re-reads the whole document and checks `kind` itself,
    // so a file that reached the wrong one is still refused by the runner rather than only here.
    match skynet_infer::kind_of(label, text)?.as_str() {
        "mlp" => {
            let (session, inputs) = skynet_infer::Mlp::load(label, text)?;
            Ok((Runner::Mlp(session), inputs))
        }
        "onnx" => {
            let beside = path.parent().unwrap_or(std::path::Path::new("."));
            let (session, inputs) =
                skynet_ort::Onnx::load(label, text, beside).map_err(|error| error.to_string())?;
            Ok((Runner::Onnx(session), inputs))
        }
        // `kind_of` only returns a name in `skynet_infer::KINDS`, so this arm is reached only by
        // adding an entry there and not here. Named rather than `unreachable!()`: the next runner is
        // somebody's afternoon, and a panic in `skynetp` is a robot's policy process exiting.
        other => Err(format!(
            "{label} declares kind `{other}`, which `skynet_infer::KINDS` lists and this binary has \
             no arm for. That is a defect in skynetp rather than in the file"
        )),
    }
}
use skynet_policy_host::bridge::{BridgeEvent, BridgeProcess, SocketLink, REDIAL_EVERY};
use skynet_policy_host::provenance::{run_name, Mark, Provenance};
use skynet_policy_host::remote::{
    Acknowledgement, Remote, RemoteCounts, RemoteDeclaration, ResumeRule,
};
use skynet_policy_host::rendezvous;
use skynet_policy_host::run::{Clock, KernelClock, PolicyHost, Summary, Tick};
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
    policy: Option<std::path::PathBuf>,
    /// §88.1's request: the motion's name, who is asking, its deadline and its parameters.
    motion: Option<String>,
    holder: Option<String>,
    deadline_ms: f32,
    parameters: Vec<(String, f64)>,
    /// §90.2's session bridge, its declared deadline, the operator profile's bound and resume rule.
    bridge_socket: Option<std::path::PathBuf>,
    max_chunk_age_ms: Option<f64>,
    profile_max_chunk_age_ms: Option<f64>,
    resume: ResumeRule,
    resume_file: Option<std::path::PathBuf>,
    /// The session bridge this process starts and tends, and the endpoint it opens (§90.2). Absent
    /// when a bridge is run some other way — by hand, beside this process.
    bridge_command: Option<std::path::PathBuf>,
    endpoint_id: Option<String>,
    /// Where this process tells the supervisor what its sessions did (§90.2, §74): the queue
    /// `skynetd` carries into the recording and the telemetry window. Absent on a bench.
    provenance: Option<std::path::PathBuf>,
    quiet: bool,
}

/// `"pose=0.4,speed=0.1"` — a parameter name, an equals sign, a number, commas between them.
///
/// Parsed strictly, for `parse_setpoints`' reason one section along: a parameter this cannot read is
/// an error rather than a dropped one, and §88.2's eighth refusal exists precisely because a motion
/// missing an argument is a command a controller will either reject on a moving robot or accept.
fn parse_parameters(text: &str) -> Result<Vec<(String, f64)>, String> {
    let mut out = Vec::new();
    for pair in text.split(',').filter(|part| !part.trim().is_empty()) {
        let (name, value) = pair
            .split_once('=')
            .ok_or_else(|| format!("a parameter wants `name=value`, and this is `{pair}`"))?;
        let number: f64 = value
            .trim()
            .parse()
            .map_err(|_| format!("`{}` is not a number for `{}`", value.trim(), name.trim()))?;
        out.push((name.trim().to_string(), number));
    }
    Ok(out)
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
        // §23.3's in-process backend. What it eats — how many channels, and the camera's shape when
        // it takes one — is declared by the model file rather than by a flag, so there is exactly one
        // artefact and no way for two flags to disagree about what order the floats arrive in.
        "inference" => {
            if options.policy.is_none() {
                return Err(
                    "--backend inference wants --policy, which is the model to run. It declares \
                     its own channel count and frame shape, so there is nothing else to pass"
                        .to_string(),
                );
            }
        }
        // §88.1's, and the only backend here that does not command a joint. Everything it needs is
        // on the command line rather than in a file, because a motion **is** the request: a name,
        // who is asking, a bound and some arguments.
        "motion" => {
            if options.motion.is_none() {
                return Err(
                    "--backend motion wants --motion, which is the body's own name for it (§84.1)"
                        .to_string(),
                );
            }
            if options.holder.is_none() {
                return Err(
                    "--backend motion wants --holder, which §88.2 compares against the lease. \
                     There is no default: a request from nobody is refused `no_lease`, and \
                     supplying a name here would be this binary claiming a lease it does not hold"
                        .to_string(),
                );
            }
            if !(options.deadline_ms.is_finite() && options.deadline_ms > 0.0) {
                return Err(
                    "--backend motion wants --deadline-ms, a positive number. §7.1's watchdog has \
                     nothing to watch while a motion runs (§88.3), so this is the bound that fires"
                        .to_string(),
                );
            }
            if options.parameters.len() > skynet_ring::MAX_MOTION_PARAMETERS {
                return Err(format!(
                    "--parameters names {} and this wire carries {}",
                    options.parameters.len(),
                    skynet_ring::MAX_MOTION_PARAMETERS
                ));
            }
        }
        // §90.2's. The deadline is checked where the backend is built, so the refusal carries
        // §87.4's code; what is checked here is that there is somewhere to reach the session.
        "remote" => {
            if options.bridge_socket.is_none() {
                return Err(
                    "--backend remote wants --bridge-socket, the local socket the session bridge \
                     listens on (or SKYNET_REMOTE_BRIDGE). The bridge dials out to the Hub and the \
                     replica; this process never opens the network itself (§90.2)"
                        .to_string(),
                );
            }
            if options.channels == 0 {
                return Err(
                    "--backend remote commands the channels the lease grants, and this lease grants \
                     none"
                        .to_string(),
                );
            }
            if options.bridge_command.is_some() && options.endpoint_id.is_none() {
                return Err(
                    "--bridge-command starts a session bridge, and a bridge opens one endpoint: \
                     --endpoint-id (or SKYNET_REMOTE_ENDPOINT) names it"
                        .to_string(),
                );
            }
        }
        other => return Err(format!("`{other}` is not a backend this build has")),
    }

    Ok(())
}

/// §90.2's resume rule, as the operator profile names it.
fn parse_resume(text: &str) -> Result<ResumeRule, String> {
    match text {
        "acknowledge" => Ok(ResumeRule::Acknowledge),
        "automatic" => Ok(ResumeRule::Automatic),
        other => Err(format!(
            "`{other}` is not a resume rule: `acknowledge` (the default) waits for an operator after \
             a lost session, `automatic` resumes on the new session's first fresh chunk"
        )),
    }
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        hz: 100.0,
        cycles: None,
        channels: 0,
        mode: mode::POSITION,
        backend: "hold".to_string(),
        motion: None,
        holder: None,
        // Zero, which §88.2's sixth rule refuses. A deadline is required and this binary will not
        // supply one: a motion's bound is what replaces §7.1's watchdog on this path (§88.3), and a
        // default would be this process choosing when a robot gives up on a movement.
        deadline_ms: 0.0,
        parameters: Vec::new(),
        setpoints: Vec::new(),
        target: Vec::new(),
        gain: 0.1,
        ring_socket: None,
        policy: None,
        bridge_socket: None,
        max_chunk_age_ms: None,
        profile_max_chunk_age_ms: None,
        // §90.2: with no setting, motion does not resume on its own after a lost session.
        resume: ResumeRule::Acknowledge,
        resume_file: None,
        provenance: None,
        bridge_command: None,
        endpoint_id: None,
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
            "--motion" => {
                options.motion = Some(value("--motion")?);
                index += 2;
            }
            "--holder" => {
                options.holder = Some(value("--holder")?);
                index += 2;
            }
            "--deadline-ms" => {
                options.deadline_ms = value("--deadline-ms")?
                    .parse()
                    .map_err(|_| "--deadline-ms wants a number".to_string())?;
                index += 2;
            }
            "--parameters" => {
                options.parameters = parse_parameters(&value("--parameters")?)?;
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
            "--policy" => {
                options.policy = Some(std::path::PathBuf::from(value("--policy")?));
                index += 2;
            }
            "--bridge-socket" => {
                options.bridge_socket = Some(std::path::PathBuf::from(value("--bridge-socket")?));
                index += 2;
            }
            "--max-chunk-age-ms" => {
                options.max_chunk_age_ms =
                    Some(value("--max-chunk-age-ms")?.parse().map_err(|_| {
                        "--max-chunk-age-ms wants a number of milliseconds".to_string()
                    })?);
                index += 2;
            }
            "--profile-max-chunk-age-ms" => {
                options.profile_max_chunk_age_ms =
                    Some(value("--profile-max-chunk-age-ms")?.parse().map_err(|_| {
                        "--profile-max-chunk-age-ms wants a number of milliseconds".to_string()
                    })?);
                index += 2;
            }
            "--resume" => {
                options.resume = parse_resume(&value("--resume")?)?;
                index += 2;
            }
            "--resume-file" => {
                options.resume_file = Some(std::path::PathBuf::from(value("--resume-file")?));
                index += 2;
            }
            "--provenance" => {
                options.provenance = Some(std::path::PathBuf::from(value("--provenance")?));
                index += 2;
            }
            "--bridge-command" => {
                options.bridge_command = Some(std::path::PathBuf::from(value("--bridge-command")?));
                index += 2;
            }
            "--endpoint-id" => {
                options.endpoint_id = Some(value("--endpoint-id")?);
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
                 \x20                  [--backend hold|trajectory|approach|inference|motion|remote]\n\
                 \x20                  [--setpoints \"0:0.1;40:0.3\"] [--target 0.1,0.2] [--gain 0.1]\n\
                 \x20                  [--policy <model.json>]\n\
                 \x20                  [--motion move_l --holder skill:pick --deadline-ms 4000\n\
                 \x20                   --parameters pose=0.4,speed=0.1]\n\
                 \x20                  [--bridge-socket <path> --max-chunk-age-ms N\n\
                 \x20                   --profile-max-chunk-age-ms N --resume acknowledge|automatic\n\
                 \x20                   --resume-file <path>\n\
                 \x20                   --bridge-command <path> --endpoint-id <id>]\n\
                 \x20                  [--ring-socket <path>] [--quiet]\n\n\
                 --backend remote is §90.2's: a policy that infers on another machine. The session\n\
                 bridge holds the network half and listens on --bridge-socket; this process sends\n\
                 it the body and the camera and proposes only actions younger than the declared\n\
                 --max-chunk-age-ms. With none declared it is refused, deadline_undeclared. After\n\
                 a lost session nothing is proposed until an operator creates --resume-file,\n\
                 unless the operator profile says --resume automatic. With --bridge-command this\n\
                 process starts the bridge for --endpoint-id itself, and starts it again if it\n\
                 exits.\n\n\
                 --backend motion is §88.1's: it asks the body's controller for one named motion\n\
                 and then holds. The four flags above are the request, and none of them has a\n\
                 default — a deadline this binary chose would be it deciding when a robot gives\n\
                 up on a movement, and a holder it supplied would be it claiming a lease. What\n\
                 admits the request is `skynetk`, against the body's own declared table.\n\n\
                 --backend inference runs §23.3's in-process model. --policy is the only thing it\n\
                 takes: the file declares how many channels it commands and, when it reads a\n\
                 camera, the frame shape and the grid it is pooled into. The file's own `kind`\n\
                 picks the runner — `mlp` is a dense network needing nothing, `onnx` is §23.3's\n\
                 production runner and loads ONNX Runtime at run time. SKYNET_ORT_LIBRARY names\n\
                 that library when it is not where the system loader looks.\n\n\
                 --ring-socket takes both rings from a runtime that is offering them\n\
                 (`skynetk run --policy-socket <path>`): the command ring this process writes\n\
                 and the observation ring it reads. Without it the ring is this process's own,\n\
                 nothing reads what goes in, and there is no body to close a loop on.\n\n\
                 SKYNET_RING_SOCKET and SKYNET_RING_CHANNELS say the same as --ring-socket and\n\
                 --channels, and are how a policy `skynetd` started finds the runtime: a package\n\
                 declares its own args, so the rendezvous arrives in the unit's environment\n\
                 rather than on a command line the supervisor edited. So do §90.2's settings:\n\
                 SKYNET_REMOTE_BRIDGE, _MAX_CHUNK_AGE_MS, _RESUME, _RESUME_FILE, _BRIDGE_COMMAND,\n\
                 _ENDPOINT, _PROVENANCE and SKYNET_PROFILE_MAX_CHUNK_AGE_MS. A flag always wins."
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
    // §90.2's settings, from the unit's environment when a package's supervisor set them. The
    // deadline is the package's and the bound and resume rule the operator profile's, so neither is
    // something `skynetd` should write onto a command line the package declared.
    if options.bridge_socket.is_none() {
        if let Some(path) = std::env::var_os("SKYNET_REMOTE_BRIDGE") {
            options.bridge_socket = Some(std::path::PathBuf::from(path));
        }
    }
    if options.max_chunk_age_ms.is_none() {
        options.max_chunk_age_ms = std::env::var("SKYNET_REMOTE_MAX_CHUNK_AGE_MS")
            .ok()
            .and_then(|text| text.parse::<f64>().ok());
    }
    if options.profile_max_chunk_age_ms.is_none() {
        options.profile_max_chunk_age_ms = std::env::var("SKYNET_PROFILE_MAX_CHUNK_AGE_MS")
            .ok()
            .and_then(|text| text.parse::<f64>().ok());
    }
    if options.resume_file.is_none() {
        if let Some(path) = std::env::var_os("SKYNET_REMOTE_RESUME_FILE") {
            options.resume_file = Some(std::path::PathBuf::from(path));
        }
    }
    if options.provenance.is_none() {
        if let Some(path) = std::env::var_os("SKYNET_REMOTE_PROVENANCE") {
            options.provenance = Some(std::path::PathBuf::from(path));
        }
    }
    if options.bridge_command.is_none() {
        if let Some(path) = std::env::var_os("SKYNET_REMOTE_BRIDGE_COMMAND") {
            options.bridge_command = Some(std::path::PathBuf::from(path));
        }
    }
    if options.endpoint_id.is_none() {
        options.endpoint_id = std::env::var("SKYNET_REMOTE_ENDPOINT")
            .ok()
            .filter(|id| !id.is_empty());
    }
    if let Ok(text) = std::env::var("SKYNET_REMOTE_RESUME") {
        if !args.iter().any(|arg| arg == "--resume") {
            match parse_resume(&text) {
                Ok(rule) => options.resume = rule,
                Err(problem) => return fail(&problem),
            }
        }
    }
    if let Err(problem) = check(&options) {
        return fail(&problem);
    }
    // §90.2's deadline, refused before any socket is touched: a policy that joined a robot's ring
    // and *then* discovered it may not run has already announced itself as the commanding process.
    if options.backend == "remote" {
        let declaration = RemoteDeclaration {
            max_chunk_age_ms: options.max_chunk_age_ms,
            profile_max_chunk_age_ms: options.profile_max_chunk_age_ms,
            resume: options.resume,
        };
        if let Err(refusal) = declaration.deadline_ns() {
            return fail(&format!("{} — {}", refusal.code(), refusal.message()));
        }
    }

    // §23.3's model, read before anything is mapped.
    //
    // Loaded here rather than where the backend is built so that a file which disagrees with the ring
    // is refused before a socket is touched: a policy that mapped a robot's command ring and *then*
    // discovered its model commands a different number of joints has already announced itself to the
    // runtime as the commanding process.
    let inference = match (options.backend.as_str(), options.policy.as_deref()) {
        ("inference", Some(path)) => {
            let label = path.display().to_string();
            let text = match std::fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => return fail(&format!("{label} could not be read: {error}")),
            };
            let (session, inputs) = match load_runner(&label, &text, path) {
                Ok(loaded) => loaded,
                Err(why) => return fail(&why),
            };
            if inputs.channels != options.channels {
                // Refused rather than reconciled in either direction. Narrowing the model would run
                // it on a shorter input than it was trained on; widening the lease would grant a
                // policy authority over a joint §6's transport plan did not resolve for it.
                return fail(&format!(
                    "{label} commands {} channel(s) and this lease grants {}. --channels comes from \
                     the runtime that offered the ring (or from SKYNET_RING_CHANNELS), and a model \
                     is run against the body it was resolved for or not at all",
                    inputs.channels, options.channels
                ));
            }
            match skynet_infer::Policy::new(session, inputs) {
                Ok(policy) => Some(policy),
                Err(why) => return fail(&why),
            }
        }
        _ => None,
    };

    let lease = Commanding {
        // No flag declares either budget, so §25.1's staleness check has nothing to check against and
        // every closed-loop cycle is counted `unbudgeted`. That is the honest state rather than a
        // permissive default: the budgets belong to a policy's *declared observation requirements*,
        // which arrive with a package (§8) rather than on the command line of a binary run by hand.
        observation_budget_ms: None,
        frame_budget_ms: None,
        channels: options.channels,
        mode: options.mode,
    };
    if !lease.valid() {
        return fail("the lease is not one this ring can carry");
    }

    if options.backend == "inference" && options.ring_socket.is_none() {
        // Same refusal as `approach`, for the same reason and one step further: a model reads the
        // body on every cycle, and one that takes a camera reads §47.6's pool as well. Neither
        // arrives without the hand-off, so a run with no `--ring-socket` would hold on every cycle
        // and look like a policy that had decided to.
        return fail(
            "--backend inference reads the body, and a model that declares a camera reads the \
             frame pool too. Both arrive over --ring-socket from a runtime that is offering them \
             (`skynetk run --policy-socket <path>`); without it there is nothing to read",
        );
    }

    if options.backend == "remote" && options.ring_socket.is_none() {
        // The replica answers observations of this body; without the body there is nothing true to
        // send it, and a remote policy that proposed on nothing would be commanding blind.
        return fail(
            "--backend remote sends the body and the camera to its session, and both arrive over \
             --ring-socket from a runtime that is offering them; without it there is nothing to send",
        );
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
    // §35.3: which clock the kernel's time is on, as the process holding the driver wrote it into
    // the ring's header after reading the body's own descriptor. Every command below is stamped on
    // it and every reading aged on it, so the watchdog on the far side and the budgets on this one
    // subtract like from like (F70). Not a flag: this process cannot see the body, and a flag
    // typed for one robot and run against another is the disagreement this is here to prevent.
    let mut clock = KernelClock::for_body(watching);
    // When this run began on the kernel's clock: the first half of its name to the supervisor, so
    // one run's records sort before the next one's (`provenance::run_name`).
    let clock_started_ns = clock.now_ns();

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
        // Only when a body came with the rings: a private run has no kernel to share a clock with.
        if watching.is_some() {
            println!(
                "skynetp: time — {}, as the observation ring names it",
                clock.time_base().as_str()
            );
        }
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
    //
    // The third element is §23.3's per-reason count, and it is `None` for every backend that is not
    // a model. Not zeroes: a `Trajectory` that held has held for its own reason, and reporting it as
    // `no_body: 0` would be a line about a question it was never asked.
    type Turn<'a> = Box<
        dyn FnMut(
                &mut skynet_ring::Producer<'_>,
                Option<&mut skynet_obs::Consumer<'_>>,
                Option<&skynet_frames::Reader>,
            ) -> (Tick, Summary, Option<skynet_infer::Counts>)
            + 'a,
    >;
    // §90.2's per-cause counts, read after the loop; `None` for every backend that is not remote.
    let remote_counts: std::rc::Rc<std::cell::Cell<Option<RemoteCounts>>> = Default::default();
    // §90.2's and §74's queue to the supervisor, finished after the loop so a run that ends says so.
    let provenance: std::rc::Rc<std::cell::RefCell<Option<Provenance>>> = Default::default();
    let mut host: Turn<'_> = match options.backend.as_str() {
        "trajectory" => {
            let rows: Vec<(u64, &[f32])> = options
                .setpoints
                .iter()
                .map(|(at, values)| (*at, values.as_slice()))
                .collect();
            let mut inner =
                PolicyHost::new(Trajectory::from_setpoints(&rows), clock, lease, period_ns);
            Box::new(move |producer, body, frames| {
                (
                    inner.turn_seeing(producer, body, frames),
                    inner.summary(),
                    None,
                )
            })
        }
        // The closed-loop one, and the only backend whose command on cycle n is a function of what
        // the body did on cycle n - 1. It was written when `skynet-obs` was and had no way to be
        // selected from a command line, because until the observation ring crossed a process
        // boundary there was nothing for it to close on outside a test.
        "approach" => {
            let mut inner = PolicyHost::new(
                Approach::toward(&options.target, options.gain),
                clock,
                lease,
                period_ns,
            );
            Box::new(move |producer, body, frames| {
                (
                    inner.turn_seeing(producer, body, frames),
                    inner.summary(),
                    None,
                )
            })
        }
        // §88.1's. One request, then a hold for ever — see `backend::Motion` for why re-asking
        // every cycle would be a hundred requests for one movement and going silent would present
        // a correctly behaving policy as a crashed producer.
        "motion" => {
            let parameters: Vec<(&str, f64)> = options
                .parameters
                .iter()
                .map(|(name, value)| (name.as_str(), *value))
                .collect();
            let Some(backend) = MotionBackend::asking(
                options.motion.as_deref().unwrap_or_default(),
                options.holder.as_deref().unwrap_or_default(),
                options.deadline_ms,
                &parameters,
            ) else {
                return fail(
                    "a name in this request is longer than the ring carries. It is refused rather \
                     than cut down: half a motion name is a different motion, and on a body whose \
                     controller has one by that shorter name it is a different command (§88.1)",
                );
            };
            let mut inner = PolicyHost::new(backend, clock, lease, period_ns);
            Box::new(move |producer, body, frames| {
                (
                    inner.turn_seeing(producer, body, frames),
                    inner.summary(),
                    None,
                )
            })
        }
        // §90.2's: a policy that infers on another machine, reached through the session bridge.
        // Everything that is a safety decision — the deadline, going quiet, not resuming on its own
        // — is decided in `remote.rs`, in this process, on the kernel's clock.
        "remote" => {
            let declaration = RemoteDeclaration {
                max_chunk_age_ms: options.max_chunk_age_ms,
                profile_max_chunk_age_ms: options.profile_max_chunk_age_ms,
                resume: options.resume,
            };
            let acknowledgement = Acknowledgement::new();
            let link = SocketLink::to(options.bridge_socket.clone().unwrap_or_default());
            let backend =
                match Remote::open(link, options.channels, declaration, acknowledgement.clone()) {
                    Ok(backend) => backend,
                    Err(refusal) => {
                        return fail(&format!("{} — {}", refusal.code(), refusal.message()))
                    }
                };
            let backend_deadline_ms = backend.deadline_ms();
            if !options.quiet {
                println!(
                    "skynetp: remote — deadline {:.1} ms, resume {}",
                    backend.deadline_ms(),
                    match options.resume {
                        ResumeRule::Acknowledge => "after an operator acknowledges",
                        ResumeRule::Automatic => "on the new session's first fresh chunk",
                    }
                );
            }
            let mut inner = PolicyHost::new(backend, clock, lease, period_ns);
            let resume_file = options.resume_file.clone();
            let quiet = options.quiet;
            let mut last = RemoteCounts::default();
            let remote_seen = remote_counts.clone();
            // The bridge, when this process was asked to run it. Started before the first turn and
            // tended every `REDIAL_EVERY` turns after — the same cadence the link redials at, so a
            // bridge that is restarting and a link that is reconnecting catch each other up.
            let mut bridge = match (&options.bridge_command, &options.endpoint_id) {
                (Some(program), Some(endpoint)) => Some(BridgeProcess::new(
                    program.clone(),
                    options
                        .bridge_socket
                        .as_deref()
                        .unwrap_or(std::path::Path::new("")),
                    endpoint,
                    backend_deadline_ms,
                )),
                _ => None,
            };
            if let (Some(bridge), false) = (bridge.as_ref(), options.quiet) {
                println!(
                    "skynetp: session bridge — {} {}",
                    options
                        .bridge_command
                        .as_deref()
                        .unwrap_or(std::path::Path::new(""))
                        .display(),
                    bridge.args().join(" ")
                );
            }
            let mut tended: u64 = 0;
            if let Some(dir) = options.provenance.as_deref() {
                *provenance.borrow_mut() = Some(Provenance::open(
                    dir,
                    run_name(clock_started_ns, std::process::id()),
                ));
            }
            let told = provenance.clone();
            Box::new(move |producer, body, frames| {
                if let Some(bridge) = bridge.as_mut() {
                    if tended % REDIAL_EVERY == 0 {
                        match bridge.tend(std::time::Instant::now()) {
                            Some(BridgeEvent::Started(pid)) if !quiet => eprintln!(
                                "skynetp: session bridge started (pid {pid}, start {})",
                                bridge.starts()
                            ),
                            Some(BridgeEvent::Exited(how)) if !quiet => eprintln!(
                                "skynetp: session bridge exited ({how}); the policy is quiet until \
                                 it is back and a new session delivers a fresh chunk"
                            ),
                            // Always said, quiet or not: a bridge that cannot start is a robot that
                            // will never move under this policy, and the reason is here or nowhere.
                            Some(BridgeEvent::Failed(why)) => eprintln!("skynetp: {why}"),
                            _ => {}
                        }
                    }
                    tended = tended.wrapping_add(1);
                }
                // An operator's acknowledgement is a file they create on this robot, and it is spent
                // whether or not the policy was waiting: one left lying about is not an
                // authorisation for the next loss.
                if let Some(path) = resume_file.as_deref() {
                    if std::fs::remove_file(path).is_ok() {
                        acknowledgement.give();
                    }
                }
                let tick = inner.turn_seeing(producer, body, frames);
                let counts = inner.backend().counts();
                // What this turn changed about the session, told to the supervisor: a proposal is
                // named by the sequence and the stamp the kernel will record beside it.
                if let Some(queue) = told.borrow_mut().as_mut() {
                    let proposed = match tick {
                        Tick::Published(seq) => Some(Mark {
                            seq,
                            stamp_ns: inner.stamp_ns(),
                        }),
                        _ => None,
                    };
                    queue.turn(
                        inner.backend().provenance(),
                        proposed,
                        counts.commanded,
                        counts.quiet_stale,
                    );
                }
                if !quiet {
                    if counts.sessions_ended > last.sessions_ended {
                        // Said as what this process knows: it has stopped proposing. The hold is the
                        // kernel's, on the other side of the ring (§7.1), and not this process's to report.
                        eprintln!(
                            "skynetp: the remote session ended; the policy is quiet until a new session \
                             delivers a fresh chunk"
                        );
                    }
                    if counts.sessions > last.sessions {
                        if let Some((session, digest)) = inner.backend().provenance() {
                            eprintln!("skynetp: remote session {session} serving {digest}");
                        }
                    }
                }
                last = counts;
                remote_seen.set(Some(counts));
                (tick, inner.summary(), None)
            })
        }
        // §23.3's, and the first backend here whose command depends on a pixel. Everything it decides
        // is in `skynet-infer`; `Inference` is the adapter, and this arm is the one place a session
        // is chosen — which is why a real inference library, when there is one, is selected here and
        // changes nothing else.
        "inference" => {
            let policy = match inference {
                Some(policy) => policy,
                // Unreachable: `check` refuses `--backend inference` with no `--policy`, and the load
                // above returns rather than falling through. Said out loud because the alternative is
                // an `unwrap` in the one arm that has a file in it.
                None => return fail("--backend inference has no model loaded, which is a defect"),
            };
            if !options.quiet {
                println!("skynetp: model — {}", policy.describe());
            }
            let mut inner = PolicyHost::new(Inference::over(policy), clock, lease, period_ns);
            let quiet = options.quiet;
            // Said once, when it first happens, rather than kept for the end: a session that refuses
            // tends to refuse identically at the control rate, and an operator watching a robot that
            // will not move wants the reason now.
            let mut said = false;
            Box::new(move |producer, body, frames| {
                let tick = inner.turn_seeing(producer, body, frames);
                if !said && !quiet {
                    if let Some(why) = inner.backend().first_refusal() {
                        eprintln!("skynetp: the model refused — {why}");
                        said = true;
                    }
                }
                (tick, inner.summary(), Some(inner.backend().counts()))
            })
        }
        _ => {
            let mut inner = PolicyHost::new(Hold, clock, lease, period_ns);
            Box::new(move |producer, body, frames| {
                (
                    inner.turn_seeing(producer, body, frames),
                    inner.summary(),
                    None,
                )
            })
        }
    };

    let mut published = 0u64;
    let mut held = 0u64;
    let mut refused = 0u64;
    let mut turns = 0u64;
    let mut summary = Summary::default();
    let mut model = None;

    loop {
        if let Some(limit) = options.cycles {
            if turns >= limit {
                break;
            }
        }
        let started = Instant::now();
        let (tick, so_far, counts) = host(&mut producer, observations.as_mut(), frames.as_ref());
        summary = so_far;
        model = counts;
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

    // A run that ends says so: the open session is closed where it stands and the last counts are
    // marked final, which is how the supervisor tells this run's end from a crash (§74.6's floor).
    if let (Some(queue), Some(counts)) = (provenance.borrow_mut().as_mut(), remote_counts.get()) {
        queue.finish(counts.commanded, counts.quiet_stale);
    }

    if !options.quiet {
        println!(
            "skynetp: {turns} turn(s) — {published} commanded, {held} held, {refused} refused"
        );
        // The three counts §25 keeps that nothing was reporting.
        //
        // `overruns` is the one that matters most and is the one that had no surface at all: §23.3
        // says inference is never in the real-time path, and this is the number that says whether a
        // backend is holding to the period it was given. It is deliberately not acted on — the fourth
        // rule in `run.rs` — which makes printing it the *only* thing that makes it visible.
        //
        // `stale_holds` and `unbudgeted` are §25.1's two halves: cycles held because the body reading
        // was older than the lease allowed, and cycles where nothing could say whether it was. The
        // second is zero on a run with no observation ring, and on a run with one it is every cycle
        // until a package declares a budget.
        println!(
            "skynetp: {} overrun(s) past the {:.1} ms period, {} hold(s) on a stale body, \
             {} cycle(s) with no budget to check one against",
            summary.overruns,
            period_ns as f64 / 1_000_000.0,
            summary.stale_holds,
            summary.unbudgeted
        );
        // What a budget would have been compared against, whether or not one was declared — the
        // only line that says how old the body a policy acted on actually was. Printed only when a
        // reading was aged: a run that never read the body has no age, not an age of zero.
        if let Some(oldest) = summary.oldest_body_ns {
            println!(
                "skynetp: the body was at most {:.3} ms old when a command was stamped",
                oldest as f64 / 1_000_000.0
            );
        }
        // Only when a pool came with the rings: a policy that printed a frame line on every robot
        // would be claiming a camera that is not there.
        if frames.is_some() {
            // The unbudgeted count is on the line because the one before it is only a
            // measurement when there is a budget: with none, "0 too old" is a check that never
            // ran, and F50 is the run that read it as one.
            println!(
                "skynetp: {} of {turns} turn(s) were shown a frame ({} missed, {} too old, {} with \
                 no frame budget to check one against)",
                summary.frames,
                summary.frames_missed,
                summary.stale_frames,
                summary.frames_unbudgeted
            );
            if let Some(oldest) = summary.oldest_frame_ns {
                println!(
                    "skynetp: the frame shown was at most {:.3} ms old when a command was stamped",
                    oldest as f64 / 1_000_000.0
                );
            }
        }
        // Why the model held, per reason, and only when there was a model. Every reason is printed
        // whether it fired or not, because a zero beside its six siblings is a fact and a line that
        // omitted the zeros would leave a reader unable to tell "never happened" from "not reported".
        if let Some(counts) = model {
            println!(
                "skynetp: the model commanded on {} of {turns} turn(s) and held on {} — \
                 no_body {}, body_too_narrow {}, no_frame {}, frame_shape {}, \
                 input_not_finite {}, session_refused {}, output_not_finite {}",
                counts.commanded,
                counts.held(),
                counts.no_body,
                counts.body_too_narrow,
                counts.no_frame,
                counts.frame_shape,
                counts.input_not_finite,
                counts.session_refused,
                counts.output_not_finite
            );
        }
        // §90.2's, and only for a remote policy: why each quiet turn was quiet, and what the session
        // delivered. Every cause is printed, zeros included, for the model's reason above.
        if let Some(counts) = remote_counts.get() {
            println!(
                "skynetp: the remote policy proposed on {} of {turns} turn(s) and was quiet on the \
                 rest — no_session {}, no_plan {}, stale {}, exhausted {}, awaiting_resume {}",
                counts.commanded,
                counts.quiet_no_session,
                counts.quiet_no_plan,
                counts.quiet_stale,
                counts.quiet_exhausted,
                counts.quiet_awaiting_resume
            );
            println!(
                "skynetp: {} observation(s) sent, {} chunk(s) used, {} late, {} unknown, {} \
                 malformed; {} session(s) opened, {} ended, {} resumed by an operator",
                counts.sent,
                counts.chunks,
                counts.chunks_late,
                counts.chunks_unknown,
                counts.chunks_malformed,
                counts.sessions,
                counts.sessions_ended,
                counts.resumed
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

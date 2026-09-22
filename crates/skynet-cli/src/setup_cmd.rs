//! `skynet setup` — install Skynet OS on a robot from this PC (§28).
//!
//! The installer lives on the robot (`packaging/install.sh`, run where root and systemd are).
//! This is the guided drive from the bench: it checks the plan here, copies a stage over SSH,
//! runs the installer there, streams its numbered steps back, verifies, and keeps a local log.
//! Nothing about the installation is decided here that the installer does not re-decide there —
//! the flags pass through by name, and a flag this command does not name is one the installer
//! answers with its own default.
//!
//! ```text
//! skynet setup robot.local --driver ./libmy_arm.so
//! skynet setup 192.168.7.2 --tier host --start
//! skynet setup robot.local --dry-run        # the plan, and every ssh invocation, printed
//! ```
//!
//! Host and driver are the whole of the required input: the body is derived from the driver
//! when `--descriptor` is omitted, and the bundle is fetched for the robot's architecture
//! when none is here. Everything derived or fetched is named in the plan, because a silent
//! default for a body or a binary is how a robot gets somebody else's.
//!
//! Five phases, each announced: preflight (reachability, architecture, writability), copy (a
//! tar pipe plus a byte count that proves the stage arrived whole), install (the installer's
//! own output, streamed), verify (`install.sh --check`, streamed), and done (cleanup, the
//! token, what to run next). A failure stops at the phase it belongs to, with the remote's own
//! words carried into the sentence.
//!
//! # What is trusted, and what is not
//!
//! The SSH connection is whatever `ssh` negotiates — keys, agent, or a password on the
//! terminal — because this command invents no transport. The architecture comparison is the one
//! thing checked here that the installer cannot: a bundle built for x86-64 installs cleanly on
//! an aarch64 robot and then every unit dies, so the ELF headers are read before anything is
//! copied and a mismatch refuses. Everything else about the robot (Linux, systemd, the unit
//! directories) is the installer's own `require_host` and `need_write`, re-run there; the
//! preflight duplicates only the writability probe, so that connecting as a user who cannot
//! write /usr fails in seconds rather than after the copy.

use crate::args::{Args, Spec};
use crate::term;
use skynet_json::Json;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::time::Instant;

pub const USAGE: &str = "\
skynet setup — install Skynet OS on a robot over SSH, guided (§28)

  skynet setup <host> --driver <lib> --descriptor <body.json> [options]

  The plan is checked here, the installer runs there, and every line it prints is
  streamed back and kept in a local log. What it asks for when a flag is missing:

  <host>                  The robot, as ssh reaches it. No default.
  --user <name>           The SSH user, default root. The install writes /usr, /etc
                          and the systemd directories, and a user who cannot is
                          refused in preflight rather than after the copy.
  --driver <lib>          The RHAL shared object, as for install.sh. Default: the
                          mock in the bundle, when the bundle has one — the summary
                          says so loudly, because a robot installed against the mock
                          is a bench robot whatever its hardware is.
  --robot <model>         A supported robot instead of a driver path: the model is
                          looked up in drivers/models.json and its library used
                          built (or refused with the reason: a missing SDK, a
                          simulation-only model, a mapping that does not exist
                          yet). One of --robot or --driver, not both.
  --descriptor <file>     The body, as for install.sh. Default: derived from the
                          driver (`driver check` writes it; a driver that will not
                          describe itself is an error naming this flag).
  --bundle <path>         A release tarball, an unpacked bundle, or a repo checkout.
                          Default: the bundle beside this binary, else ./dist, else
                          the checkout beside this binary, else this directory —
                          else fetched for the robot's architecture, checksummed
                          like packaging/get.sh. SKYNET_BASE_URL overrides where
                          releases come from, as there.

  --tier, --hz, --label, --lan, --interval, --rt-cpu, --general-cpus, --prefix,
  --root, --driver-config
                          Passed to install.sh by name when given; omitted when not,
                          so the installer's own defaults always apply.
  --enable, --start       As install.sh: arm the units, start them now. Neither is
                          the default, for the reason install.sh gives.
  --port <n>              SSH port. Default 22.
  --identity <file>       SSH identity file (-i).
  --ssh-opt <k=v,...>     Extra -o options for every ssh invocation.
  --log-file <path>       The local log. Default skynet-setup-<host>.log here,
                          truncated per run like the installer's own log.
  --yes                   Answer yes to the confirmation and never prompt: anything
                          missing is an error naming the flag.
  --dry-run               Print the plan and every ssh invocation, and run nothing.
                          Never prompts and writes no log.

  The SSH user needs no password when keys or an agent answer; otherwise ssh asks on
  the terminal, once per run on Unix (one multiplexed connection).";

const SPEC: Spec = Spec {
    values: &[
        "user",
        "port",
        "bundle",
        "robot",
        "driver",
        "driver-config",
        "descriptor",
        "tier",
        "hz",
        "label",
        "lan",
        "interval",
        "rt-cpu",
        "general-cpus",
        "prefix",
        "root",
        "identity",
        "ssh-opt",
        "log-file",
    ],
    switches: &["enable", "start", "yes", "dry-run"],
};

/// The four binaries, in the order the installer and the bundle both use.
const BINARIES: &[&str] = &["skynetd", "skynetk", "skynet", "skynetp"];

/// The remote staging directory. Fixed rather than unique: if a run dies mid-copy, the next
/// one removes exactly this, and an engineer poking at a failed install knows where to look.
const STAGE: &str = "/tmp/skynet-setup";

/// How to reach the robot. Resolved first, because everything else — including which
/// release to fetch — may need to ask it something.
struct Connection {
    host: String,
    user: String,
    port: Option<String>,
    identity: Option<String>,
    ssh_opts: Vec<String>,
}

/// The flags and the two prompted answers, before anything is looked for, derived, or
/// fetched. The bundle, driver and descriptor resolve in [`complete`] (or [`complete_dry`])
/// because each may need the others or the network: the driver's default lives in the
/// bundle, the descriptor is derived from the driver, and a missing bundle is fetched for
/// the robot's architecture.
struct Inputs {
    connection: Connection,
    bundle_flag: Option<String>,
    robot_flag: Option<String>,
    driver_flag: Option<String>,
    descriptor_flag: Option<String>,
    install_args: Vec<String>,
    enable: bool,
    start: bool,
    log_file: std::path::PathBuf,
}

/// The local answers, before anything touches the robot beyond asking.
struct Resolved {
    connection: Connection,
    bundle: std::path::PathBuf,
    bundle_fetched: bool,
    binaries: Vec<std::path::PathBuf>,
    driver: std::path::PathBuf,
    driver_is_bundle_mock: bool,
    descriptor: std::path::PathBuf,
    descriptor_derived: bool,
    install_args: Vec<String>,
    enable: bool,
    start: bool,
    log_file: std::path::PathBuf,
}

/// Terminal plus the local log. Every narration goes to both; the log gets it without color.
struct Out {
    log: std::fs::File,
}

impl Out {
    fn say(&mut self, text: &str) {
        let _ = writeln!(std::io::stderr(), "{text}");
        let _ = writeln!(self.log, "{}", strip_ansi(text));
    }
}

pub fn run(rest: &[String]) -> i32 {
    let args = match Args::parse(rest, &SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut output = std::io::stderr();
    match run_inner(&args, &mut input, &mut output) {
        Ok(()) => 0,
        Err(message) => term::fail(&message),
    }
}

/// The whole run, returning sentences. `Out` is created inside (the log path needs the host),
/// and the failure is written to it before the sentence comes back — a log that ends mid-phase
/// with no verdict is a record that stops where the trouble starts.
fn run_inner(args: &Args, input: &mut dyn BufRead, output: &mut dyn Write) -> Result<(), String> {
    let interactive = !args.flag("yes") && !args.flag("dry-run");
    let inputs = resolve(args, input, output, interactive)?;

    if args.flag("dry-run") {
        return print_plan(&complete_dry(&inputs)?, output);
    }

    let log = std::fs::File::create(&inputs.log_file).map_err(|err| {
        format!(
            "cannot write the log to {}: {err}",
            inputs.log_file.display()
        )
    })?;
    let mut out = Out { log };
    let resolved = match complete(&inputs, input, output, &mut out, interactive) {
        Ok(resolved) => resolved,
        Err(message) => {
            let _ = writeln!(out.log);
            let _ = writeln!(out.log, "error: {message}");
            return Err(message);
        }
    };
    let outcome = drive(&resolved, input, output, &mut out, interactive);
    if let Err(message) = &outcome {
        let _ = writeln!(out.log);
        let _ = writeln!(out.log, "error: {message}");
    }
    outcome
}

/// Ask on the terminal, with an optional default. There is no `isatty` without libc (see
/// `term`), so a closed stdin reads as EOF and EOF reads as "this run is not interactive":
/// the error names the flag instead of hanging on a question nobody can answer.
fn prompt(
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    interactive: bool,
    label: &str,
    flag: &str,
    default: Option<&str>,
) -> Result<String, String> {
    if !interactive {
        // A default is an answer whether or not anybody is there to confirm it; only a
        // question with no default is an error.
        if let Some(value) = default {
            return Ok(value.to_string());
        }
        return Err(format!(
            "`--{flag}` was not given and --yes/--dry-run never prompts for it."
        ));
    }
    loop {
        match default {
            Some(value) => {
                let _ = write!(output, "  {label} [{value}]: ");
            }
            None => {
                let _ = write!(output, "  {label}: ");
            }
        }
        let _ = output.flush();
        let mut line = String::new();
        let bytes = input
            .read_line(&mut line)
            .map_err(|err| format!("cannot read stdin: {err}"))?;
        if bytes == 0 {
            return Err(format!(
                "`--{flag}` was not given and stdin is closed, so there is nobody to ask."
            ));
        }
        let answer = line.trim().to_string();
        if answer.is_empty() {
            if let Some(value) = default {
                return Ok(value.to_string());
            }
            continue;
        }
        return Ok(answer);
    }
}

/// An offer, not a gate: start the units, run the first check. Anything but yes is no —
/// and a closed stdin or `--yes` declines quietly, because a piped run must not fail on
/// a question it never asked, and starting a robot is not something a flag implies.
fn offer(
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    interactive: bool,
    question: &str,
) -> bool {
    if !interactive {
        return false;
    }
    let _ = write!(output, "  {question} [y/N] ");
    let _ = output.flush();
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) => false,
        // The same answers as `confirm`, in the same list form for the same reason.
        Ok(_) => ["y", "Y", "yes", "Yes", "YES"].contains(&line.trim()),
        Err(_) => false,
    }
}

/// The one question with no flag: whether to go ahead. Anything but yes is no.
fn confirm(
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    interactive: bool,
    question: &str,
) -> Result<bool, String> {
    if !interactive {
        return Ok(true);
    }
    let _ = write!(output, "  {question} [y/N] ");
    let _ = output.flush();
    let mut line = String::new();
    let bytes = input
        .read_line(&mut line)
        .map_err(|err| format!("cannot read stdin: {err}"))?;
    if bytes == 0 {
        return Err("stdin is closed, so the confirmation cannot be answered. Pass --yes to run without prompting.".to_string());
    }
    // A list, not `matches!`: string literals on a match arm read as subcommands to
    // the golden-path checker (see the tier check in `resolve`).
    Ok(["y", "Y", "yes", "Yes", "YES"].contains(&line.trim()))
}

/// The connection and the flags — and nothing looked for, derived, or fetched. The
/// bundle, driver and descriptor complete in [`complete`] (or [`complete_dry`]) because
/// each may need the others or the network.
fn resolve(
    args: &Args,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    interactive: bool,
) -> Result<Inputs, String> {
    let host = match args.positional.first() {
        Some(host) => host.clone(),
        None => prompt(input, output, interactive, "robot host", "host", None).map_err(|_| {
            "no host: `skynet setup <host>`, where <host> is the robot as ssh reaches it."
                .to_string()
        })?,
    };
    if host.trim().is_empty() {
        return Err("the host is blank.".to_string());
    }
    // No prompt: root is the default and the confirmation names the user, so a second
    // stop just to accept it is ceremony. An operator who needs another user passes --user.
    let user = args.value("user").unwrap_or("root").to_string();

    // Pass-through, and only when given: the installer's defaults are its own, and this
    // command pinning the values it remembers would be a second record of that fact.
    let mut install_args = Vec::new();
    for flag in [
        "tier",
        "hz",
        "label",
        "lan",
        "interval",
        "rt-cpu",
        "general-cpus",
        "prefix",
        "root",
        "driver-config",
    ] {
        if let Some(value) = args.value(flag) {
            install_args.push(format!("--{flag}"));
            install_args.push(value.to_string());
        }
    }
    if let Some(tier) = args.value("tier") {
        // A list, not `matches!`: the golden-path checker reads string literals on
        // match arms as subcommands, and tiers are not commands.
        if !["guest", "host", "certified"].contains(&tier) {
            return Err(format!("--tier is guest, host or certified, not `{tier}`."));
        }
    }

    let log_file = match args.value("log-file") {
        Some(path) => std::path::PathBuf::from(path),
        None => std::path::PathBuf::from(format!("skynet-setup-{}.log", sanitize_host(&host))),
    };
    Ok(Inputs {
        connection: Connection {
            host,
            user,
            port: args.value("port").map(str::to_string),
            identity: args.value("identity").map(str::to_string),
            ssh_opts: args.list("ssh-opt"),
        },
        bundle_flag: args.value("bundle").map(str::to_string),
        robot_flag: args.value("robot").map(str::to_string),
        driver_flag: args.value("driver").map(str::to_string),
        descriptor_flag: args.value("descriptor").map(str::to_string),
        install_args,
        enable: args.flag("enable"),
        start: args.flag("start"),
        log_file,
    })
}

/// Complete the inputs into a plan: bundle (flag, detection, fetch, or prompt), driver
/// (flag, bundle mock, or prompt), descriptor (flag, or derived from the driver). The one
/// path that may touch the network, and it says what it is doing while it does.
fn complete(
    inputs: &Inputs,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    out: &mut Out,
    interactive: bool,
) -> Result<Resolved, String> {
    let (bundle, bundle_fetched) = ensure_bundle(inputs, input, output, out, interactive)?;
    let binaries = bundle_binaries(&bundle)?;
    let mock = bundle.join("lib").join("libskynet_mock.so");
    let mock_here = mock.is_file();
    if inputs.robot_flag.is_some() && inputs.driver_flag.is_some() {
        return Err("pass one of --robot or --driver, not both.".to_string());
    }
    let (driver, driver_is_bundle_mock) =
        match (inputs.robot_flag.as_deref(), inputs.driver_flag.as_deref()) {
            (Some(model), None) => (resolve_robot(model, &bundle)?, false),
            (_, Some(path)) => (std::path::PathBuf::from(path), false),
            (None, None) => {
                let default = mock_here.then(|| mock.to_string_lossy().to_string());
                let answer = prompt(
                    input,
                    output,
                    interactive,
                    "driver (.so)",
                    "driver",
                    default.as_deref(),
                )?;
                let defaulted = default.as_deref() == Some(answer.as_str());
                (std::path::PathBuf::from(answer), defaulted)
            }
        };
    if !driver.is_file() {
        return Err(format!(
            "no driver at {}. Name the robot's RHAL shared object with --driver.",
            driver.display()
        ));
    }
    let (descriptor, descriptor_derived) = match inputs.descriptor_flag.as_deref() {
        Some(path) => (std::path::PathBuf::from(path), false),
        None => {
            out.say("  deriving the body from the driver...");
            let skynet = std::env::current_exe()
                .map_err(|err| format!("cannot find this binary to derive with: {err}"))?;
            let dest = scratch_dir("derived")?.join("descriptor.json");
            let warned = derive_descriptor(
                &driver,
                passthrough(&inputs.install_args, "driver-config"),
                &skynet,
                &dest,
            )?;
            if warned {
                out.say(&format!(
                    "  {} the driver failed its own checks; its body is used anyway, because the installer and the daemon are the judges of it, not this command.",
                    term::warn("!")
                ));
            }
            (dest, true)
        }
    };
    looks_like_json_object(&descriptor).map_err(|why| {
        format!(
            "the descriptor at {} is not a JSON object ({why}). Pass --descriptor with a body written another way.",
            descriptor.display()
        )
    })?;
    Ok(Resolved {
        connection: Connection {
            host: inputs.connection.host.clone(),
            user: inputs.connection.user.clone(),
            port: inputs.connection.port.clone(),
            identity: inputs.connection.identity.clone(),
            ssh_opts: inputs.connection.ssh_opts.clone(),
        },
        bundle,
        bundle_fetched,
        binaries,
        driver,
        driver_is_bundle_mock,
        descriptor,
        descriptor_derived,
        install_args: inputs.install_args.clone(),
        enable: inputs.enable,
        start: inputs.start,
        log_file: inputs.log_file.clone(),
    })
}

/// Complete the inputs for `--dry-run`: detection only — no fetch, no derive, no prompts.
/// A dry run that downloaded a release or loaded a driver would be a plan with side effects.
fn complete_dry(inputs: &Inputs) -> Result<Resolved, String> {
    let bundle = match inputs.bundle_flag.as_deref() {
        Some(path) => bundle_from(path)?,
        None => match detect_bundle()? {
            Detected::One(bundle) => bundle,
            Detected::Several(tarballs) => {
                return Err(format!(
                    "several release tarballs are here ({}). Name one with --bundle.",
                    tarballs
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            Detected::None => {
                return Err("no bundle here, and --dry-run never fetches one. Pass --bundle, or run without --dry-run to fetch the robot's release.".to_string());
            }
        },
    };
    let binaries = bundle_binaries(&bundle)?;
    let mock = bundle.join("lib").join("libskynet_mock.so");
    if inputs.robot_flag.is_some() && inputs.driver_flag.is_some() {
        return Err("pass one of --robot or --driver, not both.".to_string());
    }
    let (driver, driver_is_bundle_mock) = match (
        inputs.robot_flag.as_deref(),
        inputs.driver_flag.as_deref(),
    ) {
        (Some(model), None) => (resolve_robot(model, &bundle)?, false),
        (_, Some(path)) => (std::path::PathBuf::from(path), false),
        (None, None) if mock.is_file() => (mock, true),
        (None, None) => {
            return Err(
                "no --driver was given and the bundle holds no mock. Name the robot's RHAL shared object.".to_string(),
            );
        }
    };
    let descriptor = match inputs.descriptor_flag.as_deref() {
        Some(path) => std::path::PathBuf::from(path),
        None => {
            return Err("no --descriptor was given and --dry-run never derives one. Pass --descriptor, or run without --dry-run to derive it from the driver.".to_string());
        }
    };
    Ok(Resolved {
        connection: Connection {
            host: inputs.connection.host.clone(),
            user: inputs.connection.user.clone(),
            port: inputs.connection.port.clone(),
            identity: inputs.connection.identity.clone(),
            ssh_opts: inputs.connection.ssh_opts.clone(),
        },
        bundle,
        bundle_fetched: false,
        binaries,
        driver,
        driver_is_bundle_mock,
        descriptor,
        descriptor_derived: false,
        install_args: inputs.install_args.clone(),
        enable: inputs.enable,
        start: inputs.start,
        log_file: inputs.log_file.clone(),
    })
}

/// What the binary finds beside itself: one bundle, several tarballs to choose from,
/// or nothing (which is when the release is fetched).
enum Detected {
    One(std::path::PathBuf),
    Several(Vec<std::path::PathBuf>),
    None,
}

/// Where the installable bytes come from without asking and without the network: a release
/// tarball, an unpacked bundle, or a repo checkout with a release build in it. The binary
/// looks beside itself first (a bundle's own `skynet` just works), then in `./dist`, then
/// here. No prompt and no fetch: the caller decides what a miss means.
fn detect_bundle() -> Result<Detected, String> {
    if let Ok(exe) = std::env::current_exe() {
        // `<bundle>/bin/skynet` and `<repo>/target/debug/skynet` alike: walk up until
        // something recognisable, and stop before the filesystem root.
        let mut anchor = exe.as_path();
        for _ in 0..4 {
            anchor = match anchor.parent() {
                Some(parent) => parent,
                None => break,
            };
            if bundle_shape(anchor).is_some() {
                return Ok(Detected::One(anchor.to_path_buf()));
            }
        }
    }
    let mut tarballs: Vec<std::path::PathBuf> = Vec::new();
    // `dist/` first, and a missing directory is no tarballs rather than an error: most
    // checkouts have never built a release.
    for dir in ["dist", "."] {
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        let mut names: Vec<std::path::PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("skynet-os-") && name.ends_with(".tar.gz"))
            })
            .collect();
        names.sort();
        tarballs.extend(names);
        if dir == "dist" && !tarballs.is_empty() {
            break;
        }
    }
    match tarballs.len() {
        0 => {}
        1 => return Ok(Detected::One(unpack_bundle(&tarballs[0])?)),
        _ => {
            // This machine's architecture first: a dist/ holding both releases should not
            // install aarch64 from an x86-64 bench without being asked.
            let arch = std::env::consts::ARCH;
            let mine: Vec<&std::path::PathBuf> = tarballs
                .iter()
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.contains(arch))
                })
                .collect();
            match mine.as_slice() {
                [only] => return Ok(Detected::One(unpack_bundle(only)?)),
                _ => return Ok(Detected::Several(tarballs)),
            }
        }
    }
    if bundle_shape(std::path::Path::new(".")).is_some() {
        return Ok(Detected::One(std::path::PathBuf::from(".")));
    }
    Ok(Detected::None)
}

/// The bundle, by any means: explicit flag, detection, a fetch for the robot's
/// architecture, or a prompt. Returns whether it was fetched, for the plan to say.
fn ensure_bundle(
    inputs: &Inputs,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    out: &mut Out,
    interactive: bool,
) -> Result<(std::path::PathBuf, bool), String> {
    if let Some(path) = inputs.bundle_flag.as_deref() {
        return Ok((bundle_from(path)?, false));
    }
    match detect_bundle()? {
        Detected::One(bundle) => return Ok((bundle, false)),
        Detected::Several(tarballs) => {
            if !interactive {
                return Err(format!(
                    "several release tarballs are here ({}). Name one with --bundle.",
                    tarballs
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
            let _ = writeln!(output, "  several release tarballs are here:");
            for (index, tarball) in tarballs.iter().enumerate() {
                let _ = writeln!(output, "    [{}] {}", index + 1, tarball.display());
            }
            let answer = prompt(input, output, interactive, "which one", "bundle", Some("1"))?;
            match answer.parse::<usize>() {
                Ok(choice) if (1..=tarballs.len()).contains(&choice) => {
                    return Ok((unpack_bundle(&tarballs[choice - 1])?, false));
                }
                _ => {
                    return Err(format!(
                        "`{answer}` is not one of the {} tarballs above.",
                        tarballs.len()
                    ));
                }
            }
        }
        Detected::None => {}
    }
    // Nothing here: ask the robot its architecture and fetch its release. A robot that
    // cannot be asked is an unreachable robot, and that error is returned rather than
    // papered over with a bundle prompt — a local path would not fix it.
    out.say("  no bundle here; asking the robot its architecture...");
    let argv = ssh_for(&inputs.connection, &arch_probe_command());
    let (code, stdout, stderr) = run_capture(&argv)?;
    if code != 0 {
        return Err(unreachable(&argv, code, &stderr));
    }
    let uname = parse_arch_probe(&stdout).ok_or_else(|| {
        format!(
            "{} answered the architecture probe with {:?}. Its shell may not be sh-compatible.",
            inputs.connection.host,
            stdout.lines().take(3).collect::<Vec<_>>().join(" / ")
        )
    })?;
    let Some(arch) = release_arch(&uname) else {
        return Err(format!(
            "the robot reports {uname}, and there is no prebuilt release for it. Build from source and pass --bundle."
        ));
    };
    match fetch_bundle(arch, &release_base()) {
        Ok(bundle) => {
            out.say(&format!(
                "  fetched and verified skynet-os-linux-{arch}.tar.gz."
            ));
            Ok((bundle, true))
        }
        Err(message) => {
            if !interactive {
                return Err(format!(
                    "{message} Pass --bundle with a release tarball, bundle, or checkout instead."
                ));
            }
            let _ = writeln!(output, "  {message}");
            let answer = prompt(
                input,
                output,
                interactive,
                "bundle (tarball, bundle dir, or repo checkout)",
                "bundle",
                None,
            )?;
            Ok((bundle_from(&answer)?, false))
        }
    }
}

/// A path that names a bundle, in any of the three shapes.
fn bundle_from(path: &str) -> Result<std::path::PathBuf, String> {
    let candidate = std::path::PathBuf::from(path);
    if candidate.is_file() {
        return unpack_bundle(&candidate);
    }
    if candidate.is_dir() {
        if bundle_shape(&candidate).is_some() {
            return Ok(candidate);
        }
        return Err(format!(
            "{} is neither a bundle (no packaging/install.sh with bin/skynetd beside it) nor a repo checkout with a release build (no target/release/skynetd). Build one with `cargo build --release` or point --bundle at a release tarball.",
            candidate.display()
        ));
    }
    Err(format!(
        "no bundle at {path}: not a file and not a directory."
    ))
}

/// Unpack a release tarball into a temporary directory. The top directory is whatever the
/// archive holds — verified to be a bundle afterwards, not assumed from the file name.
fn unpack_bundle(archive: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let scratch = scratch_dir("bundle")?;
    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(&scratch)
        .status()
        .map_err(|err| format!("cannot run tar to unpack {}: {err}", archive.display()))?;
    if !status.success() {
        return Err(format!(
            "{} does not unpack (tar exited {}).",
            archive.display(),
            status.code().unwrap_or(-1)
        ));
    }
    let mut tops: Vec<std::path::PathBuf> = std::fs::read_dir(&scratch)
        .map_err(|err| format!("cannot read {}: {err}", scratch.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect();
    tops.sort();
    for top in &tops {
        if bundle_shape(top).is_some() {
            return Ok(top.clone());
        }
    }
    Err(format!(
        "{} unpacks but holds no bundle (nothing with packaging/install.sh and bin/skynetd).",
        archive.display()
    ))
}

/// The one question asked before the bundle is known: what the robot is. Marker-wrapped
/// like the preflight probe, for the same motd reason.
fn arch_probe_command() -> String {
    "echo SKYNET-ARCH; uname -m".to_string()
}

/// The architecture line out of the probe's reply: the first non-empty line after the
/// marker, or nothing when the reply is not a probe answer at all.
fn parse_arch_probe(stdout: &str) -> Option<String> {
    let mut after = false;
    for line in stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if after {
            return Some(line.to_string());
        }
        if line.contains("SKYNET-ARCH") {
            after = true;
        }
    }
    None
}

/// `uname -m` to release architecture. The robot's, not this PC's: a bench is usually
/// x86-64 and a robot is usually aarch64, and fetching for the wrong one is exactly the
/// wrong-architecture install the preflight refuses.
fn release_arch(uname: &str) -> Option<&'static str> {
    // Ifs, not a match: string literals on a match arm read as subcommands to the
    // golden-path checker (see the tier check in `resolve`).
    let uname = uname.trim();
    if uname == "x86_64" {
        Some("x86_64")
    } else if uname == "aarch64" || uname == "arm64" {
        Some("aarch64")
    } else {
        None
    }
}

/// Where releases come from. `SKYNET_BASE_URL` overrides — for mirrors, and for the tests,
/// which point it at a local directory — exactly as in `packaging/get.sh`.
fn release_base() -> String {
    std::env::var("SKYNET_BASE_URL").unwrap_or_else(|_| {
        "https://github.com/antonihabek/Skynet/releases/latest/download".to_string()
    })
}

/// The tarball URL for an architecture, off a base. The same layout `get.sh` downloads:
/// `skynet-os-linux-<arch>.tar.gz` beside a `SHA256SUMS` that names it.
fn release_url(base: &str, arch: &str) -> String {
    format!(
        "{}/skynet-os-linux-{arch}.tar.gz",
        base.trim_end_matches('/')
    )
}

/// Whether a helper binary answers. `curl`, `wget`, `sha256sum` and `shasum` are probed
/// this way rather than looked up on PATH, because there is no `which` without libc.
fn have_tool(name: &str) -> bool {
    Command::new(name)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Download a URL to a path: `curl -fsSL` first, `wget -q` second, a sentence naming both
/// when neither is installed.
fn download(url: &str, dest: &std::path::Path) -> Result<(), String> {
    let attempts: &[(&str, &[&str])] = &[("curl", &["-fsSL", "-o"]), ("wget", &["-q", "-O"])];
    for (tool, flags) in attempts {
        if !have_tool(tool) {
            continue;
        }
        let output = Command::new(tool)
            .args(flags.iter().copied())
            .arg(dest)
            .arg(url)
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("cannot run {tool}: {err}"))?;
        if output.status.success() {
            return Ok(());
        }
        return Err(format!(
            "could not download {url}: {}",
            first_words(
                &String::from_utf8_lossy(&output.stderr),
                &format!("{tool} exited {}", output.status.code().unwrap_or(-1))
            )
        ));
    }
    Err(format!(
        "could not download {url}: neither curl nor wget is installed."
    ))
}

/// The expected hash out of a `SHA256SUMS` file: the first field of the line naming the
/// archive. The file is written by `sha256sum *.tar.gz` (see `bundle.sh`), so the name is
/// the line's second field and anything else on the line is not parsed.
fn parse_sums(text: &str, archive: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let hash = fields.next()?;
        fields
            .any(|field| field == archive)
            .then(|| hash.to_string())
    })
}

/// The hash out of `certutil -hashfile <path> SHA256`: the first line that is nothing
/// but hex (older Windows separates the bytes with spaces; newer does not). Pure, so the
/// Windows-only branch below is still tested on every platform.
fn parse_certutil(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let compact: String = line.chars().filter(|char| *char != ' ').collect();
        (compact.len() == 64 && compact.chars().all(|char| char.is_ascii_hexdigit()))
            .then(|| compact.to_lowercase())
    })
}

/// The sha256 of a file: `sha256sum`, `shasum -a 256`, or Windows' `certutil -hashfile`.
/// Absent all three, the download is refused rather than installed unverified — `get.sh`'s
/// rule, kept here.
fn hash_file(path: &std::path::Path) -> Result<String, String> {
    for (tool, extra) in [("sha256sum", &[][..]), ("shasum", &["-a", "256"][..])] {
        if !have_tool(tool) {
            continue;
        }
        let output = Command::new(tool)
            .args(extra.iter().copied())
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("cannot run {tool}: {err}"))?;
        if !output.status.success() {
            continue;
        }
        if let Some(hash) = String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
        {
            return Ok(hash.to_string());
        }
    }
    if have_tool("certutil") {
        let output = Command::new("certutil")
            .arg("-hashfile")
            .arg(path)
            .arg("SHA256")
            .stdin(Stdio::null())
            .output()
            .map_err(|err| format!("cannot run certutil: {err}"))?;
        if output.status.success() {
            if let Some(hash) = parse_certutil(&String::from_utf8_lossy(&output.stdout)) {
                return Ok(hash);
            }
        }
    }
    Err(
        "no sha256sum, shasum or certutil to verify the download with, and an unverified download is not something this command will install."
            .to_string(),
    )
}

/// Fetch the robot's release: tarball plus checksums, verified, unpacked. Every step of
/// `get.sh`, minus the parts that install things on this PC.
fn fetch_bundle(arch: &str, base: &str) -> Result<std::path::PathBuf, String> {
    let dir = scratch_dir("fetch")?;
    let archive = format!("skynet-os-linux-{arch}.tar.gz");
    let url = release_url(base, arch);
    download(
        &format!(
            "{base_url}/SHA256SUMS",
            base_url = base.trim_end_matches('/')
        ),
        &dir.join("SHA256SUMS"),
    )?;
    download(&url, &dir.join(&archive))?;
    let sums = std::fs::read_to_string(dir.join("SHA256SUMS"))
        .map_err(|err| format!("cannot read the downloaded SHA256SUMS: {err}"))?;
    let expected = parse_sums(&sums, &archive).ok_or_else(|| {
        format!("SHA256SUMS has no entry for {archive}, so it cannot be verified.")
    })?;
    let actual = hash_file(&dir.join(&archive))?;
    if expected != actual {
        return Err(format!(
            "checksum mismatch for {archive} — the download is damaged or is not the file that was released. Nothing was unpacked.\n  expected {expected}\n  got      {actual}"
        ));
    }
    unpack_bundle(&dir.join(&archive))
}

/// Derive the body from the driver by running this binary's own `driver check` against it
/// in a child process — a child, because the thing being loaded is arbitrary native code
/// and a driver that segfaults in `describe()` must kill a derivation, not the wizard.
/// Returns whether the run that produced the body also failed its checks, which the caller
/// reports as a warning: the installer and the daemon judge a body, not this command.
fn derive_descriptor(
    driver: &std::path::Path,
    config: Option<&str>,
    skynet: &std::path::Path,
    dest: &std::path::Path,
) -> Result<bool, String> {
    let mut command = Command::new(skynet);
    command
        .arg("driver")
        .arg("check")
        .arg(driver)
        .arg("--descriptor")
        .arg(dest);
    if let Some(config) = config {
        command.arg("--config").arg(config);
    }
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run {} to derive the body: {err}", skynet.display()))?;
    let written = dest.is_file() && std::fs::metadata(dest).is_ok_and(|meta| meta.len() > 0);
    if !written {
        let words = first_words(&String::from_utf8_lossy(&output.stderr), "it said nothing");
        return Err(format!(
            "the driver would not describe itself ({words}). Pass --descriptor with a body written another way."
        ));
    }
    Ok(!output.status.success())
}

/// Which shape a directory is, if it is a bundle at all: `bin/` for a release bundle,
/// `target/release/` for a repo checkout.
fn bundle_shape(dir: &std::path::Path) -> Option<&'static str> {
    if !dir.join("packaging").join("install.sh").is_file() {
        return None;
    }
    if dir.join("bin").join("skynetd").is_file() {
        return Some("bin");
    }
    if dir.join("target").join("release").join("skynetd").is_file() {
        return Some("target/release");
    }
    None
}

/// The four binaries, all present, or the sentence that says what to build.
fn bundle_binaries(bundle: &std::path::Path) -> Result<Vec<std::path::PathBuf>, String> {
    let shape = bundle_shape(bundle)
        .ok_or_else(|| format!("{} stopped being a bundle.", bundle.display()))?;
    let bin = bundle.join(shape);
    let mut binaries = Vec::new();
    for name in BINARIES {
        let path = bin.join(name);
        if !path.is_file() {
            return Err(format!(
                "{} has no {name}. A bundle carries all four binaries; rebuild it with packaging/bundle.sh or, in a checkout, `cargo build --release`.",
                bin.display()
            ));
        }
        binaries.push(path);
    }
    Ok(binaries)
}

/// Locate the robot manifest: a checkout's `drivers/models.json` beside this binary
/// first (it has the source beside it), then the checkout this runs in, else the copy
/// release bundles ship (typed refusals without a checkout). The middle step is the one
/// a `cargo install`ed binary needs: it lives in `~/.cargo/bin`, five climbs from
/// nothing, while the driver the engineer just built sits in the working tree. Returns
/// the file and, when it sits in a checkout, the `drivers/` directory its entries are
/// relative to.
fn find_manifest(
    bundle: &std::path::Path,
) -> Option<(std::path::PathBuf, Option<std::path::PathBuf>)> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(hit) = manifest_above(exe.as_path()) {
            return Some(hit);
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(hit) = manifest_above(&cwd) {
            return Some(hit);
        }
    }
    let shipped = bundle.join("models.json");
    if shipped.is_file() {
        return Some((shipped, None));
    }
    None
}

/// Climb from `start` looking for `drivers/models.json`. A directory checks itself
/// first (the working tree); a file starts at its parent (the binary's own directory).
/// Either way the climb is five parents deep, and the tests pin it with scratch trees
/// instead of the process's own executable or directory.
fn manifest_above(
    start: &std::path::Path,
) -> Option<(std::path::PathBuf, Option<std::path::PathBuf>)> {
    let mut anchor = start;
    for depth in 0..6 {
        if depth > 0 || anchor.is_dir() {
            let candidate = anchor.join("drivers").join("models.json");
            if candidate.is_file() {
                return Some((candidate, Some(anchor.join("drivers"))));
            }
        }
        anchor = match anchor.parent() {
            Some(parent) => parent,
            None => break,
        };
    }
    None
}

/// Parse the manifest, with the file named in every complaint: a hand-edited manifest
/// that does not parse must read as what it is, not as an unknown model.
fn read_manifest(path: &std::path::Path) -> Result<Json, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    Json::parse(&text).map_err(|err| format!("{} is not JSON: {err:?}", path.display()))
}

/// Turn `--robot <model>` into a driver library, via the manifest.
fn resolve_robot(model: &str, bundle: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let Some((file, drivers)) = find_manifest(bundle) else {
        return Err("--robot needs drivers/models.json, and there is none beside this binary, none above this directory, and none in the bundle. Run from a checkout, or pass --driver with the library.".to_string());
    };
    let manifest = read_manifest(&file)?;
    resolve_model(model, &manifest, drivers.as_deref(), &bundle.join("lib"))
        .map_err(|err| format!("{err} (from {})", file.display()))
}

/// The manifest lookup itself: pure over the parsed document, so the tests assert every
/// kind. Ifs, not a match: string literals on a match arm read as subcommands to the
/// golden-path checker (see the tier check in `resolve`).
fn resolve_model(
    model: &str,
    manifest: &Json,
    drivers: Option<&std::path::Path>,
    bundle_lib: &std::path::Path,
) -> Result<std::path::PathBuf, String> {
    let models = manifest
        .get("models")
        .and_then(Json::as_obj)
        .ok_or_else(|| "the manifest holds no models table".to_string())?;
    let entry = models.get(model).ok_or_else(|| {
        let known: Vec<&str> = models.keys().map(String::as_str).collect();
        format!("no such robot model: {model}. Known: {}.", known.join(", "))
    })?;
    let dir = entry
        .get("dir")
        .and_then(Json::as_str)
        .ok_or_else(|| format!("{model} names no directory"))?;
    let kind = entry
        .get("kind")
        .and_then(Json::as_str)
        .ok_or_else(|| format!("{model} names no kind"))?;
    let note = entry.get("note").and_then(Json::as_str).unwrap_or("");
    if kind == "sim" {
        return Err(format!(
            "{model} is simulation-only ({note}); it has no installable library. For the bench, --robot mock."
        ));
    }
    if kind == "python" {
        return Err(format!(
            "{model}'s driver is Python ({note}). Install the host with --driver and configure the payload per drivers/{dir}/README.md."
        ));
    }
    if kind == "host" {
        return Err(format!(
            "python-host is the generic Python-driver host, not a robot ({note}). Install it with --driver and configure the payload per drivers/{dir}/README.md."
        ));
    }
    if kind == "unmapped" {
        return Err(format!(
            "no install mapping for {model} yet (see drivers/{dir}/README.md). Pass --driver with the built library."
        ));
    }
    if kind != "prebuilt" && kind != "sdk" {
        return Err(format!("{model} has an unknown kind: {kind}."));
    }
    let lib = entry
        .get("lib")
        .and_then(Json::as_str)
        .ok_or_else(|| format!("{model} names no library"))?;
    // Shipped first (a bundle's own mock needs no checkout), then built beside the
    // manifest. Either may hold it; neither is an error until both miss.
    let shipped = bundle_lib.join(lib);
    if shipped.is_file() {
        return Ok(shipped);
    }
    if let Some(drivers) = drivers {
        let built = drivers.join(dir).join(lib);
        if built.is_file() {
            return Ok(built);
        }
        if kind == "sdk" {
            let sdk = entry
                .get("sdk")
                .and_then(Json::as_str)
                .unwrap_or("its vendor SDK");
            return Err(format!(
                "{model} links {sdk}: set it and build (make -C {}/{} lib), then re-run — or pass --driver with the built library.",
                drivers.display(),
                dir
            ));
        }
        return Err(format!(
            "drivers/{dir}/{lib} is not built. From a checkout, on Linux: make -C {}/{} lib — then re-run. (A library built on Windows or macOS will not run on the robot.)",
            drivers.display(),
            dir
        ));
    }
    if kind == "sdk" {
        let sdk = entry
            .get("sdk")
            .and_then(Json::as_str)
            .unwrap_or("its vendor SDK");
        return Err(format!(
            "{model} links {sdk}, and this bundle ships no copy. Build from a checkout (make -C drivers/{dir} lib) or pass --driver."
        ));
    }
    Err(format!(
        "this bundle ships no {lib}, and there is no checkout to build drivers/{dir} from. Install from a checkout, or pass --driver."
    ))
}

/// A fresh temporary directory, unique to this call. Two wizards installing two robots
/// from one PC must not share a stage, and neither must two tests — a fixed name raced,
/// and the loser reported its *source* missing when the winner had removed the *dest*.
fn scratch_dir(purpose: &str) -> Result<std::path::PathBuf, String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "skynet-setup-{}-{}-{}",
        std::process::id(),
        purpose,
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    std::fs::create_dir_all(&dir)
        .map_err(|err| format!("cannot create {}: {err}", dir.display()))?;
    Ok(dir)
}

/// The ELF machine field: two bytes at offset 18, after the sixteen-byte identity. Read with
/// no dependency, because the alternative is shelling out to `file`, which a PC may not have.
fn elf_machine(path: &std::path::Path) -> Result<u16, String> {
    let mut head = [0u8; 20];
    let mut file = std::fs::File::open(path)
        .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
    file.read_exact(&mut head)
        .map_err(|_| format!("{} is too small to be an executable.", path.display()))?;
    if head[0..4] != [0x7f, b'E', b'L', b'F'] {
        return Err(format!("{} is not an ELF binary.", path.display()));
    }
    Ok(u16::from_le_bytes([head[18], head[19]]))
}

/// The machine field as `uname -m` spells it, so the comparison below is string to string.
fn machine_name(machine: u16) -> String {
    match machine {
        62 => "x86_64".to_string(),
        183 => "aarch64".to_string(),
        other => format!("machine {other}"),
    }
}

/// Refuse to install one architecture's binaries on another architecture's robot. Doing so
/// installs cleanly and then every unit dies — the one failure the installer cannot catch,
/// because by the time it runs, the wrong bytes are already the plan.
fn check_arch(local_machine: u16, remote_uname: &str, what: &str) -> Result<(), String> {
    let local = machine_name(local_machine);
    let remote = remote_uname.trim();
    let same = local == remote
        || (local == "aarch64" && remote == "arm64")
        || (local == "x86_64" && remote == "x86-64");
    if same {
        return Ok(());
    }
    Err(format!(
        "{what} is built for {local} but the robot reports {remote}. A wrong-architecture install copies cleanly and then nothing runs; fetch the {remote} release instead."
    ))
}

/// The installer's own rule (`looks_like_json`), applied here so a bad descriptor fails
/// before anything is copied rather than on the robot. Deliberately no stricter: a full
/// decode is the daemon's, and this command must not refuse what the daemon would take.
fn looks_like_json_object(path: &std::path::Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|err| format!("it cannot be read: {err}"))?;
    let first = bytes
        .iter()
        .find(|byte| !byte.is_ascii_whitespace())
        .copied();
    match first {
        Some(b'{') => Ok(()),
        Some(_) => Err("it does not begin with `{`".to_string()),
        None => Err("it is empty".to_string()),
    }
}

/// Quote one word for the remote shell. The transport passes the remote command as a single
/// ssh argument, so this is the only quoting in the whole run.
fn sh_quote(word: &str) -> String {
    if !word.is_empty()
        && word
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || "-_@%+=:,./".contains(char))
    {
        return word.to_string();
    }
    format!("'{}'", word.replace('\'', "'\\''"))
}

/// The ssh invocation for one remote command, as argv. Pure, so the tests assert the exact
/// bytes of every call the wizard makes.
fn ssh_argv(
    user: &str,
    host: &str,
    port: Option<&str>,
    identity: Option<&str>,
    opts: &[String],
    remote: &str,
) -> Vec<String> {
    let mut argv = vec!["ssh".to_string()];
    if let Some(port) = port {
        argv.push("-p".to_string());
        argv.push(port.to_string());
    }
    if let Some(identity) = identity {
        argv.push("-i".to_string());
        argv.push(identity.to_string());
    }
    // One connection for the whole run: without multiplexing, a password setup asks once per
    // phase, which is five interruptions. Unix only — Windows' ssh does not do ControlMaster,
    // and an option it rejects would fail every call there.
    if cfg!(unix) {
        let path = std::env::temp_dir()
            .join("skynet-setup-%C")
            .to_string_lossy()
            .to_string();
        argv.push("-o".to_string());
        argv.push("ControlMaster=auto".to_string());
        argv.push("-o".to_string());
        argv.push("ControlPersist=60".to_string());
        argv.push("-o".to_string());
        argv.push(format!("ControlPath={path}"));
    }
    for opt in opts {
        argv.push("-o".to_string());
        argv.push(opt.clone());
    }
    argv.push(format!("{user}@{host}"));
    argv.push(remote.to_string());
    argv
}

/// Filenames cannot hold everything a host can (`::1` on Windows). The log name keeps what
/// identifies the robot and flattens the rest.
fn sanitize_host(host: &str) -> String {
    host.chars()
        .map(|char| {
            if char.is_ascii_alphanumeric() || char == '.' || char == '-' || char == '_' {
                char
            } else {
                '_'
            }
        })
        .collect()
}

/// Strip SGR color sequences for the log. The narration is painted for the terminal and
/// plain in the file, and a log full of escapes is a log that needs cleaning before grep.
fn strip_ansi(text: &str) -> String {
    // Named `stripped`: `clean` would read, to check-production-callers.mjs, as a production
    // caller of the allow-listed `Report::clean` — a use is any mention of the name.
    let mut stripped = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(char) = chars.next() {
        if char == '\x1b' {
            // An escape introduces a sequence ending in a letter; anything else is passed
            // through, because dropping bytes is worse than keeping noise.
            let mut end = false;
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    end = true;
                    break;
                }
            }
            if !end {
                break;
            }
            continue;
        }
        stripped.push(char);
    }
    stripped
}

/// What is copied to the robot: install.sh, the units, the binaries, the driver, the body.
struct Stage {
    dir: std::path::PathBuf,
    files: usize,
    bytes: u64,
    driver_base: String,
}

/// Assemble the stage in a temporary directory. Copying (rather than tarring the bundle in
/// place) is what lets the driver's and descriptor's local paths be anything: they land at
/// fixed remote names the install command below can quote once.
fn assemble_stage(resolved: &Resolved) -> Result<Stage, String> {
    let dir = scratch_dir("stage")?;
    let mut files = 0;
    let mut bytes = 0u64;
    let mut copy = |from: &std::path::Path, to: &std::path::Path| -> Result<(), String> {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("cannot create {}: {err}", parent.display()))?;
        }
        let size = std::fs::copy(from, to)
            .map_err(|err| format!("cannot stage {} -> {}: {err}", from.display(), to.display()))?;
        files += 1;
        bytes += size;
        Ok(())
    };
    copy(
        &resolved.bundle.join("packaging").join("install.sh"),
        &dir.join("install.sh"),
    )?;
    let units = resolved.bundle.join("packaging").join("systemd");
    let mut services: Vec<std::path::PathBuf> = std::fs::read_dir(&units)
        .map_err(|err| format!("cannot read {}: {err}", units.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("service"))
        .collect();
    services.sort();
    if services.is_empty() {
        return Err(format!(
            "{} holds no systemd units. A bundle carries skynetd.service and skynetk.service here.",
            units.display()
        ));
    }
    for service in &services {
        let name = service
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unit.service");
        copy(service, &dir.join("systemd").join(name))?;
    }
    for binary in &resolved.binaries {
        let name = binary
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("binary");
        copy(binary, &dir.join("bin").join(name))?;
    }
    let driver_base = resolved
        .driver
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("driver.so")
        .to_string();
    copy(&resolved.driver, &dir.join("driver").join(&driver_base))?;
    copy(&resolved.descriptor, &dir.join("descriptor.json"))?;
    Ok(Stage {
        dir,
        files,
        bytes,
        driver_base,
    })
}

/// One remote command's argv, from the connection.
fn ssh_for(connection: &Connection, remote: &str) -> Vec<String> {
    ssh_argv(
        &connection.user,
        &connection.host,
        connection.port.as_deref(),
        connection.identity.as_deref(),
        &connection.ssh_opts,
        remote,
    )
}

/// Run argv to completion, capturing both streams. Short calls only (probes, mkdir): anything
/// whose output a person should watch goes through [`run_streaming`].
fn run_capture(argv: &[String]) -> Result<(i32, String, String), String> {
    let output = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run {}: {err}. Is OpenSSH installed?", argv[0]))?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ))
}

/// What one streamed line is: an installer's step (contributing its milliseconds), a
/// failure (contributing its sentence), or anything else.
enum StreamLine {
    Step { ms: u64 },
    Fail { text: String },
    Other,
}

/// Parse one line of installer output. The shape `✓ [N] (+Nms)` is the contract install.sh
/// keeps for this parser; anything else relays uninterpreted, because a line this parser
/// does not know is still a line the operator should see.
fn parse_line(line: &str) -> StreamLine {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("✓ [") {
        if let Some(end) = rest.find(']') {
            // Parsed and validated, not kept: the number proves the shape, and the relay
            // counts steps itself rather than trusting a counter it did not increment.
            let _number: u64 = match rest[..end].parse() {
                Ok(number) => number,
                Err(_) => return StreamLine::Other,
            };
            let after = rest[end + 1..].trim_start();
            if let Some(ms_end) = after.strip_prefix("(+").and_then(|tail| tail.find("ms)")) {
                let tail = &after[2..];
                if let Ok(ms) = tail[..ms_end].parse::<u64>() {
                    return StreamLine::Step { ms };
                }
            }
        }
    }
    if let Some(text) = trimmed.strip_prefix("FAIL ") {
        return StreamLine::Fail {
            text: text.to_string(),
        };
    }
    StreamLine::Other
}

/// What a streamed run reported, counted as it relayed. `step_ms` sums the per-step
/// durations, which the installer measures against its own clock — so the summary can say
/// how long the robot worked separately from how long the run took.
#[derive(Default)]
struct RelayStats {
    steps: u64,
    step_ms: u64,
    failures: Vec<String>,
    transcript: Vec<String>,
}

/// Relay a child's lines to the terminal and the log, counting steps and failures as they
/// pass. The lines print verbatim: the numbering is the progress, and rewording it would be
/// a second narration over the installer's own.
fn relay<I>(lines: I, out: &mut Out) -> RelayStats
where
    I: Iterator<Item = String>,
{
    let mut stats = RelayStats::default();
    for line in lines {
        match parse_line(&line) {
            StreamLine::Step { ms } => {
                stats.steps += 1;
                stats.step_ms += ms;
            }
            StreamLine::Fail { text } => stats.failures.push(text),
            StreamLine::Other => {}
        }
        out.say(&line);
        stats.transcript.push(line);
    }
    stats
}

/// Run argv with its merged output streamed through [`relay`]. Stdin stays the terminal's, so
/// ssh can ask for a password; the remote command merges its own stderr into stdout, so the
/// relay sees one ordered stream rather than two racing ones.
fn run_streaming(argv: &[String], out: &mut Out) -> Result<(i32, RelayStats), String> {
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("cannot run {}: {err}. Is OpenSSH installed?", argv[0]))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("cannot read {}'s output.", argv[0]))?;
    let reader = BufReader::new(stdout);
    let lines = reader
        .lines()
        .map_while(Result::ok)
        .map(|line| line.replace('\r', ""));
    let stats = relay(lines, out);
    let status = child
        .wait()
        .map_err(|err| format!("cannot wait for {}: {err}", argv[0]))?;
    Ok((status.code().unwrap_or(-1), stats))
}

/// The admin token, printed once by the installer and buried in scrollback by everything
/// after it. Re-printed in the final summary: the operator has to transcribe it, and a token
/// nobody can find is a robot nobody can talk to. `None` when the installer kept an existing
/// token (a reinstall) or when its wording drifted — both read as sentences, not failures.
fn extract_token(transcript: &[String]) -> Option<String> {
    let start = transcript
        .iter()
        .position(|line| line.contains("printed once"))?;
    let end = transcript[start..]
        .iter()
        .position(|line| line.contains("Write it down"))
        .map(|offset| start + offset)?;
    transcript[start + 1..end]
        .iter()
        .map(|line| line.trim().to_string())
        .find(|line| !line.is_empty())
}

fn phase(out: &mut Out, number: u8, title: &str) -> Instant {
    out.say("");
    out.say(&format!("  {} [{number}/5] {title}", term::bold("phase")));
    Instant::now()
}

fn took(out: &mut Out, since: Instant, what: &str) {
    let seconds = since.elapsed().as_secs();
    let human = if seconds < 90 {
        format!("{seconds}s")
    } else {
        format!("{}m{}s", seconds / 60, seconds % 60)
    };
    out.say(&format!("  {} {what} in {human}.", term::dim("done:")));
}

/// One pass-through value, out of the flat flag/value pairs [`resolve`] kept.
fn passthrough<'a>(install_args: &'a [String], flag: &str) -> Option<&'a str> {
    let want = format!("--{flag}");
    install_args
        .windows(2)
        .find(|pair| pair[0] == want)
        .map(|pair| pair[1].as_str())
}

/// The five phases. Local checks first (while the plan is still free to refuse), then one
/// ssh call per remote step, each with the sentence its failure deserves.
fn drive(
    resolved: &Resolved,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    out: &mut Out,
    interactive: bool,
) -> Result<(), String> {
    let started = Instant::now();
    out.say(&format!(
        "  {} Skynet OS {} {}@{}",
        term::bold("setup:"),
        term::dim("installing"),
        resolved.connection.user,
        resolved.connection.host
    ));
    out.say(&format!(
        "  {} {}",
        term::dim("log:"),
        resolved.log_file.display()
    ));

    // The bundle's architecture, from its own bytes — and all four binaries agreeing, because
    // a bundle that mixes architectures is corrupt, not cross-platform.
    let mut machines = Vec::new();
    for binary in &resolved.binaries {
        machines.push(elf_machine(binary)?);
    }
    let bundle_machine = machines[0];
    for (binary, machine) in resolved.binaries.iter().zip(machines.iter()) {
        if machine != &bundle_machine {
            return Err(format!(
                "the bundle mixes architectures: {} is {} and {} is {}. Rebuild it with packaging/bundle.sh.",
                resolved.binaries[0].display(),
                machine_name(bundle_machine),
                binary.display(),
                machine_name(*machine)
            ));
        }
    }
    let driver_machine = elf_machine(&resolved.driver)?;
    let stage = assemble_stage(resolved)?;

    out.say("");
    out.say(&format!(
        "  bundle:    {} ({} files, {} MB{})",
        resolved.bundle.display(),
        stage.files,
        stage.bytes / 1_000_000,
        if resolved.bundle_fetched {
            ", fetched for this robot"
        } else {
            ""
        }
    ));
    out.say(&format!(
        "  body:      {} -> {STAGE}/descriptor.json{}",
        resolved.descriptor.display(),
        if resolved.descriptor_derived {
            " (derived from the driver)"
        } else {
            ""
        }
    ));
    out.say(&format!(
        "  driver:    {} -> {STAGE}/driver/{}",
        resolved.driver.display(),
        stage.driver_base
    ));
    if resolved.driver_is_bundle_mock {
        out.say(&format!(
            "  {} the driver defaulted to the bundle's mock: no hardware will move. For a real robot, pass --driver.",
            term::warn("!")
        ));
    }
    out.say(&format!(
        "  arch:      {} (the robot is asked in preflight)",
        machine_name(bundle_machine)
    ));
    if !resolved.install_args.is_empty() {
        out.say(&format!(
            "  installer: {}",
            resolved
                .install_args
                .iter()
                .map(|word| sh_quote(word))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    if resolved.enable || resolved.start {
        out.say(&format!(
            "  units:     {}{}",
            if resolved.enable {
                "enabled"
            } else {
                "not enabled"
            },
            if resolved.start { ", started" } else { "" }
        ));
    }
    if passthrough(&resolved.install_args, "lan").is_none() {
        out.say("  console:   none (--lan not passed; add --lan <robot-ip>:<port> to reach");
        out.say("             status and doctor from here instead of over SSH)");
    }
    if !confirm(
        input,
        output,
        interactive,
        &format!(
            "install on {} as {} ({} files, {} MB)?",
            resolved.connection.host,
            resolved.connection.user,
            stage.files,
            stage.bytes / 1_000_000
        ),
    )? {
        let _ = std::fs::remove_dir_all(&stage.dir);
        return Err("aborted at the confirmation.".to_string());
    }

    // ── 1/5: preflight ──
    let since = phase(
        out,
        1,
        &format!(
            "preflight: reaching {} as {}",
            resolved.connection.host, resolved.connection.user
        ),
    );
    let argv = ssh_for(&resolved.connection, &probe_command());
    let (code, stdout, stderr) = run_capture(&argv)?;
    if code != 0 {
        return Err(unreachable(&argv, code, &stderr));
    }
    let between: Vec<&str> = stdout
        .lines()
        .skip_while(|line| !line.contains("SKYNET-BEGIN"))
        .skip(1)
        .take_while(|line| !line.contains("SKYNET-END"))
        .collect();
    if between.len() < 2 {
        return Err(format!(
            "{} answered, but not with the probe. Its shell may not be sh-compatible; the reply was: {}",
            resolved.connection.host,
            stdout.lines().take(3).collect::<Vec<_>>().join(" / ")
        ));
    }
    if between[0].trim() != "Linux" {
        return Err(format!(
            "{} runs {}, not Linux. Skynet OS installs on Linux.",
            resolved.connection.host,
            between[0].trim()
        ));
    }
    check_arch(bundle_machine, between[1], "the bundle")?;
    check_arch(driver_machine, between[1], "the driver")?;
    let denied: Vec<&str> = between[2..]
        .iter()
        .filter_map(|line| line.strip_prefix("RO "))
        .collect();
    if !denied.is_empty() {
        return Err(format!(
            "{} as {} cannot write {}. The install writes /usr, /etc and the systemd directories; connect as a user that can (on most robots: root).",
            resolved.connection.host,
            resolved.connection.user,
            denied.join(", ")
        ));
    }
    out.say(&format!(
        "  reachable, Linux, {}, writable.",
        between[1].trim()
    ));
    took(out, since, "preflight");

    // ── 2/5: copy ──
    let since = phase(
        out,
        2,
        &format!(
            "copy: {} files, {} MB -> {STAGE}",
            stage.files,
            stage.bytes / 1_000_000
        ),
    );
    let argv = ssh_for(&resolved.connection, &mkdir_command());
    let (code, stdout, stderr) = run_capture(&argv)?;
    if code != 0 || !stdout.contains("SKYNET-READY") {
        return Err(format!(
            "cannot prepare {STAGE} on {} (exit {code}): {}",
            resolved.connection.host,
            first_words(&stderr, "ssh said nothing")
        ));
    }
    copy_stage(resolved, &stage)?;
    out.say(&format!("  received {} bytes, verified.", stage.bytes));
    took(out, since, "the copy");

    // ── 3/5: install ──
    let since = phase(out, 3, "install: streaming the installer's output");
    let install = install_command(resolved, &stage);
    let argv = ssh_for(&resolved.connection, &format!("{install} 2>&1"));
    let (code, install_stats) = run_streaming(&argv, out)?;
    if code != 0 {
        return Err(format!(
            "install.sh exited {code} on {}. Its words are above; the stage is left at {STAGE} (with install.log beside them) and this run is in {}.",
            resolved.connection.host,
            resolved.log_file.display()
        ));
    }
    took(
        out,
        since,
        &format!(
            "the install ({} steps, {}s installer time)",
            install_stats.steps,
            install_stats.step_ms / 1000
        ),
    );

    // ── 4/5: verify ──
    let since = phase(out, 4, "verify: install.sh --check on the robot");
    let check = format!(
        "{} --log-file {STAGE}/check.log 2>&1",
        check_command(resolved)
    );
    let argv = ssh_for(&resolved.connection, &check);
    let (code, check_stats) = run_streaming(&argv, out)?;
    if code != 0 {
        return Err(format!(
            "installed but does not verify on {} (exit {code}): do not start the robot. The stage is left at {STAGE} with install.log and check.log, and this run is in {}.",
            resolved.connection.host,
            resolved.log_file.display()
        ));
    }
    took(
        out,
        since,
        &format!(
            "verification ({} checks held, {}s installer time)",
            check_stats.steps,
            check_stats.step_ms / 1000
        ),
    );

    // ── 5/5: done ──
    let since = phase(out, 5, "done: tidying up");
    let argv = ssh_for(&resolved.connection, &format!("rm -rf {STAGE}"));
    match run_capture(&argv) {
        Ok((0, _, _)) => {}
        Ok((code, _, stderr)) => out.say(&format!(
            "  {} could not remove {STAGE} (exit {code}): {} — remove it by hand; the install does not need it.",
            term::warn("!"),
            first_words(&stderr, "ssh said nothing")
        )),
        Err(message) => out.say(&format!(
            "  {} could not remove {STAGE}: {message} — remove it by hand; the install does not need it.",
            term::warn("!")
        )),
    }
    let _ = std::fs::remove_dir_all(&stage.dir);
    took(out, since, "cleanup");

    out.say("");
    let total = started.elapsed().as_secs();
    out.say(&format!(
        "  {} Skynet OS is on {} ({}m{}s). Log: {}",
        term::ok("installed:"),
        resolved.connection.host,
        total / 60,
        total % 60,
        resolved.log_file.display()
    ));
    let token = extract_token(&install_stats.transcript);
    match &token {
        Some(token) => {
            out.say("");
            out.say(&format!(
                "  {}",
                term::bold("The admin token, as the installer printed it:")
            ));
            out.say("");
            out.say(&format!("    {token}"));
            out.say("");
            out.say("  Nothing keeps it but a salted hash (§39.2). Write it down now —");
            out.say("  or paste this into the shell you will run skynet from:");
            out.say("");
            out.say(&format!("    export SKYNET_TOKEN={token}"));
        }
        None => {
            out.say("  No new token was issued — this robot already had one, and reinstalling");
            out.say("  never invalidates the token an operator wrote down.");
        }
    }
    out.say("");
    tail(resolved, token.as_deref(), input, output, out, interactive);
    out.say("");
    Ok(())
}

/// After the install: start the units and run the first health check, offered rather than
/// taken. Both are best-effort — a tail step that fails warns loudly and the install still
/// stands, because installed and running are different promises and only the first was made.
fn tail(
    resolved: &Resolved,
    token: Option<&str>,
    input: &mut dyn BufRead,
    output: &mut dyn Write,
    out: &mut Out,
    interactive: bool,
) {
    let ssh_start = format!(
        "ssh {}@{} systemctl start skynetk skynetd",
        resolved.connection.user, resolved.connection.host
    );
    if !resolved.start {
        if interactive
            && offer(
                input,
                output,
                interactive,
                "start the units now? Only if the workspace is clear",
            )
        {
            let argv = ssh_for(&resolved.connection, "systemctl start skynetk skynetd");
            match run_capture(&argv) {
                Ok((0, _, _)) => out.say("  started: skynetk and skynetd are running."),
                Ok((code, _, stderr)) => out.say(&format!(
                    "  {} could not start the units (exit {code}): {} — the install stands; start by hand: {ssh_start}",
                    term::warn("!"),
                    first_words(&stderr, "ssh said nothing")
                )),
                Err(message) => out.say(&format!(
                    "  {} could not start the units: {message} — the install stands; start by hand: {ssh_start}",
                    term::warn("!")
                )),
            }
        } else {
            out.say("  Nothing is running. When the workspace is clear:");
            out.say("");
            out.say(&format!("    {ssh_start}"));
        }
        out.say("");
    }
    let Some(token) = token else {
        out.say("  No new token was issued, so the first check is yours to run with the");
        out.say("  existing one: skynet doctor on the robot, or --host with --token.");
        return;
    };
    match passthrough(&resolved.install_args, "lan") {
        Some(lan) => {
            if interactive
                && offer(
                    input,
                    output,
                    interactive,
                    "run the first health check now, from here",
                )
            {
                run_first_doctor_local(lan, token, out);
            } else {
                out.say("  Then, from here (unauthenticated routes answer right away;");
                out.say("  the rest want --token <token> or $SKYNET_TOKEN):");
                out.say("");
                out.say(&format!("    skynet status --host {lan}"));
                out.say(&format!("    skynet doctor --host {lan}"));
            }
        }
        None => {
            if interactive
                && offer(
                    input,
                    output,
                    interactive,
                    "run the first health check now, on the robot",
                )
            {
                let argv = ssh_for(&resolved.connection, "skynet doctor 2>&1");
                match run_streaming(&argv, out) {
                    Ok((0, _)) => out.say("  the robot reports healthy."),
                    Ok((code, _)) => out.say(&format!(
                        "  {} doctor finished unhappy (exit {code}) — the install stands; its complaints are above.",
                        term::warn("!")
                    )),
                    Err(message) => out.say(&format!(
                        "  {} could not run doctor on the robot: {message}",
                        term::warn("!")
                    )),
                }
            } else {
                out.say("  Then, on the robot (no --lan means no console on the LAN):");
                out.say("");
                out.say(&format!(
                    "    ssh {}@{}",
                    resolved.connection.user, resolved.connection.host
                ));
                out.say("    skynet status");
            }
        }
    }
}

/// `skynet doctor --host <lan>` as this binary, with the token passed by environment
/// rather than argv — a token on a command line is visible in `ps` to every user.
/// Captured, not streamed: reading one pipe to EOF while the child fills the other
/// deadlocks, and the check is seconds long.
fn run_first_doctor_local(lan: &str, token: &str, out: &mut Out) {
    let skynet = match std::env::current_exe() {
        Ok(exe) => exe.to_string_lossy().to_string(),
        Err(err) => {
            out.say(&format!(
                "  {} cannot find this binary to run doctor with: {err} — run it by hand: skynet doctor --host {lan}",
                term::warn("!")
            ));
            return;
        }
    };
    let output = match Command::new(&skynet)
        .arg("doctor")
        .arg("--host")
        .arg(lan)
        .env("SKYNET_TOKEN", token)
        .stdin(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(err) => {
            out.say(&format!(
                "  {} cannot run doctor: {err} — run it by hand: skynet doctor --host {lan}",
                term::warn("!")
            ));
            return;
        }
    };
    // Both streams relayed into the log: doctor narrates to stderr and reports to stdout.
    // Stdout first, then stderr — the order within each is exact, and the two never
    // shared an order to preserve.
    for text in [
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    ] {
        relay(text.lines().map(str::to_string), out);
    }
    if output.status.success() {
        out.say("  the robot reports healthy.");
    } else {
        out.say(&format!(
            "  {} doctor finished unhappy (exit {}) — the install stands; its complaints are above.",
            term::warn("!"),
            output.status.code().unwrap_or(-1)
        ));
    }
}

/// The first line of a command's complaint, or the fallback when it said nothing. Remote
/// failures arrive with pages of context; the sentence carries the first line and the log
/// carries the rest.
fn first_words(stderr: &str, fallback: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(fallback)
        .to_string()
}

/// The preflight ssh call failed: unreachable, refused, or unauthenticated. ssh's own first
/// line is the diagnosis ("Connection refused", "Permission denied (publickey)"), so it leads
/// and the remedy follows it.
fn unreachable(argv: &[String], code: i32, stderr: &str) -> String {
    let words = first_words(stderr, "ssh said nothing");
    let mut hint = String::new();
    let lower = stderr.to_lowercase();
    if lower.contains("permission denied") {
        hint = " Check the user (--user), the key (--identity), and that the robot trusts this PC."
            .to_string();
    } else if lower.contains("connection refused") {
        hint = " Nothing is listening there: is sshd running on the robot?".to_string();
    } else if lower.contains("could not resolve") || lower.contains("name or service not known") {
        hint = " That host does not resolve from here.".to_string();
    } else if lower.contains("connection timed out") || lower.contains("timed out") {
        hint = " The robot is not answering: wrong address, or a firewall between here and it."
            .to_string();
    } else if lower.contains("host key verification failed") {
        hint = " Its host key changed, or another machine answers at that address now.".to_string();
    }
    format!(
        "cannot reach the robot (ssh exited {code}): {words}.{hint} The call was: {}",
        argv_display(argv)
    )
}

/// One argv, printable: every word quoted for a local shell except the last, which is the
/// remote command and prints single-quoted so it reads as one thing.
fn argv_display(argv: &[String]) -> String {
    let mut words: Vec<String> = argv[..argv.len().saturating_sub(1)]
        .iter()
        .map(|word| sh_quote(word))
        .collect();
    if let Some(remote) = argv.last() {
        words.push(format!("'{remote}'"));
    }
    words.join(" ")
}

/// The preflight probe: kernel, architecture, and one verdict per path the install
/// writes. Marker-wrapped, because an sshd banner or motd would otherwise read as the
/// kernel's answer.
fn probe_command() -> String {
    "echo SKYNET-BEGIN; uname -s; uname -m; for d in /usr/bin /etc /data /run /etc/systemd/system /usr/lib/systemd/system /lib/systemd/system; do if [ -e \"$d\" ]; then if [ -w \"$d\" ]; then echo \"OK $d\"; else echo \"RO $d\"; fi; else echo \"ABSENT $d\"; fi; done; echo SKYNET-END".to_string()
}

/// Fresh staging directory on the robot. `rm -rf` first: an aborted run leaves a stage
/// behind, and a partial stage topped up by a second copy is a stage nobody verified.
fn mkdir_command() -> String {
    format!("rm -rf {STAGE} && mkdir -p {STAGE} && echo SKYNET-READY")
}

/// Unpack the piped stage and count the bytes back. The count is matched against the bytes
/// this PC sent; see [`copy_stage`].
fn receive_command() -> String {
    format!("tar -xz -C {STAGE} && echo SKYNET-OK && find {STAGE} -type f -exec cat {{}} + | wc -c")
}

/// The verification invocation. `--prefix` and `--root` travel with it when the install
/// carried them; every other flag shaped the install, not the check.
fn check_command(resolved: &Resolved) -> String {
    let mut check = format!("bash {STAGE}/install.sh --check");
    for flag in ["prefix", "root"] {
        if let Some(value) = passthrough(&resolved.install_args, flag) {
            check.push(' ');
            check.push_str(&format!("--{flag} {}", sh_quote(value)));
        }
    }
    check
}

/// The remote install invocation, as one shell line. Every value is quoted where it stands;
/// the caller appends `2>&1` for streaming.
fn install_command(resolved: &Resolved, stage: &Stage) -> String {
    let mut parts = vec![
        "bash".to_string(),
        format!("{STAGE}/install.sh"),
        "install".to_string(),
        "--from".to_string(),
        format!("{STAGE}/bin"),
        "--driver".to_string(),
        format!("{STAGE}/driver/{}", stage.driver_base),
        "--descriptor".to_string(),
        format!("{STAGE}/descriptor.json"),
    ];
    parts.extend(resolved.install_args.iter().cloned());
    if resolved.enable {
        parts.push("--enable".to_string());
    }
    if resolved.start {
        parts.push("--start".to_string());
    }
    parts.push("--log-file".to_string());
    parts.push(format!("{STAGE}/install.log"));
    parts
        .iter()
        .map(|word| sh_quote(word))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pipe the stage across: local `tar -cz` into the remote `tar -xz`, then a byte count back.
/// The count is the proof — a truncated transfer that still unpacked would install missing
/// pieces, and tar's exit status alone does not catch every short write.
fn copy_stage(resolved: &Resolved, stage: &Stage) -> Result<(), String> {
    let mut tar = Command::new("tar")
        .arg("-cz")
        .arg("-C")
        .arg(&stage.dir)
        .arg(".")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("cannot run tar: {err}"))?;
    let tar_out = tar
        .stdout
        .take()
        .ok_or_else(|| "cannot read tar's output.".to_string())?;
    let tar_err = tar
        .stderr
        .take()
        .ok_or_else(|| "cannot read tar's errors.".to_string())?;
    let argv = ssh_for(&resolved.connection, &receive_command());
    let ssh_output = Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(tar_out)
        .output()
        .map_err(|err| format!("cannot run {}: {err}. Is OpenSSH installed?", argv[0]))?;
    let tar_status = tar
        .wait()
        .map_err(|err| format!("cannot wait for tar: {err}"))?;
    if !tar_status.success() {
        let mut err = String::new();
        BufReader::new(tar_err)
            .read_to_string(&mut err)
            .unwrap_or(0);
        return Err(format!(
            "cannot pack the stage (tar exited {}): {}",
            tar_status.code().unwrap_or(-1),
            first_words(&err, "tar said nothing")
        ));
    }
    let code = ssh_output.status.code().unwrap_or(-1);
    let stdout = String::from_utf8_lossy(&ssh_output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&ssh_output.stderr).to_string();
    if code != 0 {
        return Err(format!(
            "the copy failed (ssh exited {code}): {}. The call was: {}",
            first_words(&stderr, "ssh said nothing"),
            argv_display(&argv)
        ));
    }
    let mut lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    match (lines.next(), lines.next()) {
        (Some("SKYNET-OK"), Some(count)) => match count.parse::<u64>() {
            Ok(got) if got == stage.bytes => Ok(()),
            Ok(got) => Err(format!(
                "a short copy: this PC sent {} bytes and the robot holds {got}. The network dropped half the stage; run again.",
                stage.bytes
            )),
            Err(_) => Err(format!(
                "the robot answered the copy with {count:?}, which is not a byte count."
            )),
        },
        _ => Err(format!(
            "the robot answered the copy with {:?}. Its shell may not be sh-compatible.",
            stdout.lines().take(3).collect::<Vec<_>>().join(" / ")
        )),
    }
}

/// `--dry-run`: the plan and every ssh invocation, printed instead of run.
fn print_plan(resolved: &Resolved, output: &mut dyn Write) -> Result<(), String> {
    let mut machines = Vec::new();
    for binary in &resolved.binaries {
        machines.push(elf_machine(binary)?);
    }
    let stage = assemble_stage(resolved)?;
    let install = install_command(resolved, &stage);
    let check = check_command(resolved);
    let _ = writeln!(output);
    let _ = writeln!(
        output,
        "  plan: install Skynet OS on {}@{} ({} files, {} MB, {})",
        resolved.connection.user,
        resolved.connection.host,
        stage.files,
        stage.bytes / 1_000_000,
        machine_name(machines[0])
    );
    let _ = writeln!(output, "  bundle: {}", resolved.bundle.display());
    let _ = writeln!(output, "  driver: {}", resolved.driver.display());
    let _ = writeln!(output, "  body:   {}", resolved.descriptor.display());
    let _ = writeln!(output, "  log:    {}", resolved.log_file.display());
    let _ = writeln!(output);
    let _ = writeln!(output, "  ssh calls, in order (exact):");
    for (index, remote) in [
        probe_command(),
        mkdir_command(),
        receive_command(),
        format!("{install} 2>&1"),
        format!("{check} --log-file {STAGE}/check.log 2>&1"),
        format!("rm -rf {STAGE}"),
    ]
    .iter()
    .enumerate()
    {
        let argv = ssh_for(&resolved.connection, remote);
        let _ = writeln!(output, "  [{}] {}", index + 1, argv_display(&argv));
    }
    let _ = writeln!(output);
    let _ = writeln!(output, "  dry run: nothing was copied and nothing ran.");
    let _ = std::fs::remove_dir_all(&stage.dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// A fresh directory, unique within and across test processes. Removed by the test that
    /// made it: a suite that litters /tmp is a suite that hides its own evidence.
    fn scratch(suite: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "skynet-setup-test-{}-{suite}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    fn connection() -> Connection {
        Connection {
            host: "robot.local".to_string(),
            user: "root".to_string(),
            port: None,
            identity: None,
            ssh_opts: Vec::new(),
        }
    }

    /// Twenty bytes of ELF header with the given machine field: everything
    /// [`elf_machine`] reads, and nothing it does not.
    fn fake_elf(dir: &std::path::Path, name: &str, machine: u16) -> std::path::PathBuf {
        let mut head = [0u8; 20];
        head[0..4].copy_from_slice(&[0x7f, b'E', b'L', b'F']);
        head[18..20].copy_from_slice(&machine.to_le_bytes());
        let path = dir.join(name);
        std::fs::write(&path, head).unwrap();
        path
    }

    /// A bundle directory in the release shape, with fake-ELF binaries for this machine's
    /// architecture claim (x86-64 unless the test says otherwise).
    fn fake_bundle(machine: u16) -> std::path::PathBuf {
        let dir = scratch("bundle");
        std::fs::create_dir_all(dir.join("packaging").join("systemd")).unwrap();
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::create_dir_all(dir.join("lib")).unwrap();
        std::fs::write(
            dir.join("packaging").join("install.sh"),
            "#!/usr/bin/env bash\n",
        )
        .unwrap();
        for unit in ["skynetd.service", "skynetk.service"] {
            std::fs::write(
                dir.join("packaging").join("systemd").join(unit),
                "[Service]\n",
            )
            .unwrap();
        }
        for binary in BINARIES {
            fake_elf(&dir.join("bin"), binary, machine);
        }
        fake_elf(&dir.join("lib"), "libskynet_mock.so", machine);
        dir
    }

    #[test]
    fn quoting_leaves_plain_words_bare_and_singles_the_rest() {
        assert_eq!(sh_quote("--tier"), "--tier");
        assert_eq!(sh_quote("/tmp/skynet-setup/bin"), "/tmp/skynet-setup/bin");
        assert_eq!(sh_quote("my robot"), "'my robot'");
        assert_eq!(sh_quote("o'clock"), "'o'\\''clock'");
        assert_eq!(sh_quote(""), "''");
    }

    #[test]
    fn the_ssh_call_is_one_remote_command_on_user_at_host() {
        let argv = ssh_argv("root", "robot.local", None, None, &[], "uname -m");
        assert_eq!(argv[0], "ssh");
        assert_eq!(argv[argv.len() - 2], "root@robot.local");
        assert_eq!(argv[argv.len() - 1], "uname -m");
    }

    #[test]
    fn port_identity_and_extra_options_ride_along() {
        let argv = ssh_argv(
            "ada",
            "10.0.0.4",
            Some("2222"),
            Some("/home/ada/.ssh/robot"),
            &["StrictHostKeyChecking=no".to_string()],
            "true",
        );
        let text = argv.join(" ");
        assert!(text.contains("-p 2222"), "{text}");
        assert!(text.contains("-i /home/ada/.ssh/robot"), "{text}");
        assert!(text.contains("-o StrictHostKeyChecking=no"), "{text}");
        assert!(text.contains("ada@10.0.0.4"), "{text}");
    }

    #[test]
    fn unix_multiplexes_one_connection_for_the_whole_run() {
        let argv = ssh_argv("root", "robot.local", None, None, &[], "true");
        let text = argv.join(" ");
        if cfg!(unix) {
            assert!(text.contains("ControlMaster=auto"), "{text}");
            assert!(text.contains("ControlPersist=60"), "{text}");
        } else {
            assert!(!text.contains("ControlMaster"), "{text}");
        }
    }

    #[test]
    fn installer_steps_parse_and_anything_else_relays() {
        match parse_line("  ✓ [3] (+45ms) binaries: skynetd skynetk skynet skynetp") {
            StreamLine::Step { ms } => assert_eq!(ms, 45),
            _ => panic!("a step did not parse as one"),
        }
        match parse_line("  FAIL /usr/bin/skynetd is installed and executable") {
            StreamLine::Fail { text } => {
                assert!(text.contains("skynetd"), "{text}");
            }
            _ => panic!("a failure did not parse as one"),
        }
        for other in [
            "  logging this run to /tmp/x.log",
            "",
            "  ✓ [bogus] (+12ms) not a step",
            "  ✓ [4] soon, not yet",
            "FAILURE at large",
        ] {
            assert!(
                matches!(parse_line(other), StreamLine::Other),
                "{other:?} parsed as something"
            );
        }
    }

    #[test]
    fn the_relay_counts_steps_and_keeps_failures() {
        let dir = scratch("relay");
        let log = std::fs::File::create(dir.join("relay.log")).unwrap();
        let mut out = Out { log };
        let stats = relay(
            [
                "  logging this run to /tmp/install.log".to_string(),
                "  ✓ [1] (+14ms) first".to_string(),
                "  ✓ [2] (+1006ms) second".to_string(),
                "  FAIL third is missing".to_string(),
            ]
            .into_iter(),
            &mut out,
        );
        assert_eq!(stats.steps, 2);
        assert_eq!(stats.step_ms, 1020);
        assert_eq!(stats.failures, vec!["third is missing".to_string()]);
        assert_eq!(stats.transcript.len(), 4);
        let logged = std::fs::read_to_string(dir.join("relay.log")).unwrap();
        assert!(logged.contains("✓ [2]"), "{logged}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_token_is_the_line_between_the_two_markers() {
        let transcript = [
            "  The admin token, printed once and stored nowhere but the env file:",
            "",
            "    s3cret-token-words",
            "",
            "  Nothing keeps it but a salted hash (§39.2). Write it down now.",
        ]
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
        assert_eq!(
            extract_token(&transcript).as_deref(),
            Some("s3cret-token-words")
        );
    }

    #[test]
    fn no_markers_means_no_token_not_an_error() {
        let transcript = ["  ✓ [5] (+3ms) admin credential issued".to_string()];
        assert_eq!(extract_token(&transcript), None);
        // The wording drifted: a start with no end is also just none.
        let transcript = ["  printed once, probably".to_string()];
        assert_eq!(extract_token(&transcript), None);
    }

    #[test]
    fn architectures_compare_by_uname_spelling() {
        assert!(check_arch(62, "x86_64", "the bundle").is_ok());
        assert!(check_arch(183, "aarch64", "the bundle").is_ok());
        assert!(check_arch(183, "arm64", "the bundle").is_ok());
        let err = check_arch(62, "aarch64", "the bundle").unwrap_err();
        assert!(err.contains("x86_64") && err.contains("aarch64"), "{err}");
        assert_eq!(machine_name(62), "x86_64");
        assert_eq!(machine_name(999), "machine 999");
    }

    #[test]
    fn the_machine_field_is_two_bytes_at_offset_18() {
        let dir = scratch("elf");
        assert_eq!(elf_machine(&fake_elf(&dir, "x64", 62)).unwrap(), 62);
        assert_eq!(elf_machine(&fake_elf(&dir, "arm", 183)).unwrap(), 183);
        std::fs::write(
            dir.join("text"),
            b"#!/bin/sh\necho padding to twenty bytes\n",
        )
        .unwrap();
        assert!(elf_machine(&dir.join("text"))
            .unwrap_err()
            .contains("not an ELF"));
        assert!(elf_machine(&dir.join("missing")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_descriptor_rule_matches_the_installers() {
        let dir = scratch("descriptor");
        std::fs::write(dir.join("good.json"), "  \n{\"joints\": []}\n").unwrap();
        assert!(looks_like_json_object(&dir.join("good.json")).is_ok());
        std::fs::write(dir.join("empty.json"), "   ").unwrap();
        assert!(looks_like_json_object(&dir.join("empty.json"))
            .unwrap_err()
            .contains("empty"));
        std::fs::write(dir.join("oops.json"), "<html>").unwrap();
        assert!(looks_like_json_object(&dir.join("oops.json")).is_err());
        assert!(looks_like_json_object(&dir.join("missing.json")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn prompts_take_answers_defaults_and_closed_stdin() {
        let mut output = Vec::new();
        let answer = prompt(
            &mut Cursor::new(b"robot.local\n"),
            &mut output,
            true,
            "robot host",
            "host",
            None,
        )
        .unwrap();
        assert_eq!(answer, "robot.local");
        let answer = prompt(
            &mut Cursor::new(b"\n"),
            &mut output,
            true,
            "SSH user",
            "user",
            Some("root"),
        )
        .unwrap();
        assert_eq!(answer, "root");
        let err = prompt(&mut Cursor::new(b""), &mut output, true, "x", "xflag", None).unwrap_err();
        assert!(err.contains("--xflag"), "{err}");
        let err = prompt(
            &mut Cursor::new(b"ignored\n"),
            &mut output,
            false,
            "x",
            "xflag",
            None,
        )
        .unwrap_err();
        assert!(err.contains("--xflag"), "{err}");
        let answer = prompt(
            &mut Cursor::new(b""),
            &mut output,
            false,
            "x",
            "xflag",
            Some("dflt"),
        )
        .unwrap();
        assert_eq!(answer, "dflt");
    }

    #[test]
    fn confirmation_is_yes_or_it_is_no() {
        let mut output = Vec::new();
        assert!(confirm(&mut Cursor::new(b"y\n"), &mut output, true, "go?").unwrap());
        assert!(confirm(&mut Cursor::new(b"yes\n"), &mut output, true, "go?").unwrap());
        assert!(!confirm(&mut Cursor::new(b"\n"), &mut output, true, "go?").unwrap());
        assert!(!confirm(&mut Cursor::new(b"no\n"), &mut output, true, "go?").unwrap());
        assert!(confirm(&mut Cursor::new(b""), &mut output, true, "go?").is_err());
        assert!(confirm(&mut Cursor::new(b""), &mut output, false, "go?").unwrap());
    }

    #[test]
    fn offers_decline_on_anything_but_yes_and_on_closed_stdin() {
        let mut output = Vec::new();
        assert!(offer(&mut Cursor::new(b"y\n"), &mut output, true, "go?"));
        assert!(offer(&mut Cursor::new(b"Yes\n"), &mut output, true, "go?"));
        assert!(!offer(&mut Cursor::new(b"\n"), &mut output, true, "go?"));
        assert!(!offer(&mut Cursor::new(b"no\n"), &mut output, true, "go?"));
        assert!(!offer(&mut Cursor::new(b""), &mut output, true, "go?"));
        assert!(!offer(&mut Cursor::new(b"y\n"), &mut output, false, "go?"));
    }

    #[test]
    fn host_names_sanitize_for_filenames_and_ansi_strips_for_logs() {
        assert_eq!(sanitize_host("robot.local"), "robot.local");
        assert_eq!(sanitize_host("::1"), "__1");
        assert_eq!(sanitize_host("10.0.0.4"), "10.0.0.4");
        assert_eq!(strip_ansi("  \x1b[32m✓\x1b[0m [1] done"), "  ✓ [1] done");
        assert_eq!(strip_ansi("plain"), "plain");
    }

    #[test]
    fn the_install_command_names_the_stage_and_the_pass_through() {
        let dir = scratch("install-cmd");
        let resolved = Resolved {
            connection: connection(),
            bundle: dir.clone(),
            bundle_fetched: false,
            binaries: Vec::new(),
            driver: dir.join("libmy_arm.so"),
            driver_is_bundle_mock: false,
            descriptor: dir.join("body.json"),
            descriptor_derived: false,
            install_args: vec!["--tier".to_string(), "host".to_string()],
            enable: false,
            start: true,
            log_file: dir.join("x.log"),
        };
        let stage = Stage {
            dir: dir.clone(),
            files: 9,
            bytes: 10,
            driver_base: "libmy_arm.so".to_string(),
        };
        let command = install_command(&resolved, &stage);
        assert!(
            command.contains("bash /tmp/skynet-setup/install.sh install"),
            "{command}"
        );
        assert!(
            command.contains("--from /tmp/skynet-setup/bin"),
            "{command}"
        );
        assert!(
            command.contains("--driver /tmp/skynet-setup/driver/libmy_arm.so"),
            "{command}"
        );
        assert!(
            command.contains("--descriptor /tmp/skynet-setup/descriptor.json"),
            "{command}"
        );
        assert!(command.contains("--tier host"), "{command}");
        assert!(command.contains("--start"), "{command}");
        assert!(!command.contains("--enable"), "{command}");
        assert!(
            command.contains("--log-file /tmp/skynet-setup/install.log"),
            "{command}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ssh_failures_get_the_remedy_their_words_imply() {
        let argv = [
            "ssh".to_string(),
            "root@nowhere".to_string(),
            "true".to_string(),
        ];
        let denied = unreachable(&argv, 255, "root@h: Permission denied (publickey).");
        assert!(denied.contains("--identity"), "{denied}");
        let refused = unreachable(
            &argv,
            255,
            "ssh: connect to host h port 22: Connection refused",
        );
        assert!(refused.contains("sshd"), "{refused}");
        let unknown = unreachable(
            &argv,
            255,
            "ssh: Could not resolve hostname nah: Name or service not known",
        );
        assert!(unknown.contains("does not resolve"), "{unknown}");
        let quiet = unreachable(&argv, 255, "");
        assert!(quiet.contains("said nothing"), "{quiet}");
    }

    #[test]
    fn the_stage_holds_installer_units_binaries_driver_and_body() {
        let bundle = fake_bundle(62);
        let driver = fake_elf(&bundle, "libmy_arm.so", 62);
        let descriptor = bundle.join("body.json");
        std::fs::write(&descriptor, "{\"joints\": []}").unwrap();
        let binaries: Vec<std::path::PathBuf> = BINARIES
            .iter()
            .map(|name| bundle.join("bin").join(name))
            .collect();
        let resolved = Resolved {
            connection: connection(),
            bundle: bundle.clone(),
            bundle_fetched: false,
            binaries,
            driver,
            driver_is_bundle_mock: false,
            descriptor,
            descriptor_derived: false,
            install_args: Vec::new(),
            enable: false,
            start: false,
            log_file: bundle.join("x.log"),
        };
        let stage = assemble_stage(&resolved).unwrap();
        // install.sh, two units, four binaries, the driver, the body.
        assert_eq!(stage.files, 9);
        assert!(stage.bytes > 0);
        assert_eq!(stage.driver_base, "libmy_arm.so");
        assert!(stage.dir.join("install.sh").is_file());
        assert!(stage.dir.join("systemd").join("skynetd.service").is_file());
        assert!(stage.dir.join("bin").join("skynetd").is_file());
        assert!(stage.dir.join("driver").join("libmy_arm.so").is_file());
        assert!(stage.dir.join("descriptor.json").is_file());
        std::fs::remove_dir_all(&bundle).unwrap();
        std::fs::remove_dir_all(&stage.dir).unwrap();
    }

    #[test]
    fn dry_run_prints_the_plan_and_runs_nothing() {
        let bundle = fake_bundle(62);
        let driver = bundle.join("lib").join("libskynet_mock.so");
        let descriptor = bundle.join("body.json");
        std::fs::write(&descriptor, "{\"joints\": []}").unwrap();
        let args = Args::parse(
            &argv(&[
                "robot.local",
                "--bundle",
                bundle.to_str().unwrap(),
                "--driver",
                driver.to_str().unwrap(),
                "--descriptor",
                descriptor.to_str().unwrap(),
                "--tier",
                "host",
                "--dry-run",
            ]),
            &SPEC,
        )
        .unwrap();
        let mut output = Vec::new();
        let mut input = Cursor::new(b"");
        run_inner(&args, &mut input, &mut output).unwrap();
        let plan = String::from_utf8(output).unwrap();
        assert!(plan.contains("robot.local"), "{plan}");
        assert!(plan.contains("9 files"), "{plan}");
        assert!(plan.contains("--tier host"), "{plan}");
        assert!(plan.contains("SKYNET-BEGIN"), "{plan}");
        assert!(plan.contains("install.sh install"), "{plan}");
        assert!(plan.contains("--check"), "{plan}");
        assert!(plan.contains("nothing was copied"), "{plan}");
        std::fs::remove_dir_all(&bundle).unwrap();
    }

    #[test]
    fn dry_run_without_a_descriptor_names_the_flag() {
        let bundle = fake_bundle(62);
        let driver = bundle.join("lib").join("libskynet_mock.so");
        let args = Args::parse(
            &argv(&[
                "robot.local",
                "--bundle",
                bundle.to_str().unwrap(),
                "--driver",
                driver.to_str().unwrap(),
                "--dry-run",
            ]),
            &SPEC,
        )
        .unwrap();
        let mut output = Vec::new();
        let mut input = Cursor::new(b"");
        let err = run_inner(&args, &mut input, &mut output).unwrap_err();
        assert!(err.contains("--descriptor"), "{err}");
        std::fs::remove_dir_all(&bundle).unwrap();
    }

    #[test]
    fn release_names_come_from_the_robots_uname() {
        assert_eq!(release_arch("x86_64"), Some("x86_64"));
        assert_eq!(release_arch("aarch64"), Some("aarch64"));
        assert_eq!(release_arch("arm64"), Some("aarch64"));
        assert_eq!(release_arch("armv7l"), None);
        assert_eq!(release_arch(""), None);
        assert_eq!(
            release_url("https://example.invalid/r", "aarch64"),
            "https://example.invalid/r/skynet-os-linux-aarch64.tar.gz"
        );
        assert_eq!(
            release_url("https://example.invalid/r/", "x86_64"),
            "https://example.invalid/r/skynet-os-linux-x86_64.tar.gz"
        );
    }

    #[test]
    fn the_arch_probe_reads_past_motd_noise() {
        assert_eq!(
            parse_arch_probe("welcome\nSKYNET-ARCH\naarch64\n").as_deref(),
            Some("aarch64")
        );
        assert_eq!(parse_arch_probe("garbage without a marker\n"), None);
        assert_eq!(parse_arch_probe("SKYNET-ARCH\n"), None);
    }

    #[test]
    fn checksums_parse_out_of_a_sums_file() {
        let sums = "aaa  skynet-os-linux-x86_64.tar.gz\nbbb  skynet-os-linux-aarch64.tar.gz\n";
        assert_eq!(
            parse_sums(sums, "skynet-os-linux-aarch64.tar.gz").as_deref(),
            Some("bbb")
        );
        assert_eq!(parse_sums(sums, "skynet-os-linux-riscv.tar.gz"), None);
        assert_eq!(parse_sums("", "skynet-os-linux-x86_64.tar.gz"), None);
    }

    #[test]
    fn a_tool_that_is_not_there_is_not_there() {
        assert!(!have_tool("skynet-definitely-not-a-tool"));
    }

    #[test]
    fn certutil_hashes_parse_with_and_without_spaces() {
        let hash = "abc123".to_string() + &"0".repeat(58);
        let modern = format!(
            "SHA256 hash of file x:\n{hash}\nCertUtil: -hashfile command completed successfully.\n"
        );
        assert_eq!(parse_certutil(&modern).as_deref(), Some(hash.as_str()));
        let spaced: String = hash
            .chars()
            .collect::<Vec<_>>()
            .chunks(2)
            .map(|pair| pair.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join(" ");
        let legacy = format!("SHA256 hash of file x:\n{spaced}\nCertUtil: -hashfile command completed successfully.\n");
        assert_eq!(parse_certutil(&legacy).as_deref(), Some(hash.as_str()));
        assert_eq!(parse_certutil("CertUtil: too much homework\n"), None);
    }

    /// Derivation through a stub `skynet`: the wizard's plumbing (exit code to warning,
    /// missing file to error) without loading a real driver. Unix-only: the stub is a
    /// shell script, and Windows has no `/bin/sh` to run it with.
    #[cfg(unix)]
    #[test]
    fn derivation_warns_on_a_failing_check_and_refuses_a_silent_driver() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch("derive");
        let stub = dir.join("skynet-stub");
        std::fs::write(
            &stub,
            "#!/bin/sh\nif [ \"$SKYNET_STUB_MODE\" = write ]; then echo '{\"joints\": []}' > \"$5\"; fi\nexit \"${SKYNET_STUB_EXIT:-0}\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        let driver = dir.join("lib.so");
        std::fs::write(&driver, "not a library").unwrap();

        // Clean run: file written, exit zero, no warning.
        std::env::set_var("SKYNET_STUB_MODE", "write");
        std::env::set_var("SKYNET_STUB_EXIT", "0");
        let dest = dir.join("clean.json");
        assert!(!derive_descriptor(&driver, None, &stub, &dest).unwrap());
        assert!(dest.is_file());
        // Failing checks with a body anyway: warned, not refused.
        std::env::set_var("SKYNET_STUB_EXIT", "1");
        let dest = dir.join("warned.json");
        assert!(derive_descriptor(&driver, Some("--cfg"), &stub, &dest).unwrap());
        // Nothing written: the error names the escape hatch.
        std::env::set_var("SKYNET_STUB_MODE", "silent");
        let dest = dir.join("missing.json");
        let err = derive_descriptor(&driver, None, &stub, &dest).unwrap_err();
        assert!(err.contains("--descriptor"), "{err}");
        std::env::remove_var("SKYNET_STUB_MODE");
        std::env::remove_var("SKYNET_STUB_EXIT");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A manifest covering every kind the lookup dispatches on.
    fn fake_manifest() -> Json {
        Json::parse(
            r#"{"models": {
            "built": {"dir": "built", "kind": "prebuilt", "lib": "libbuilt.so"},
            "sdked": {"dir": "sdked", "kind": "sdk", "lib": "libsdked.so", "sdk": "SDKED_SDK"},
            "simmed": {"dir": "simmed", "kind": "sim", "note": "a screen, not a machine"},
            "snaked": {"dir": "snaked", "kind": "python", "note": "needs a host"},
            "hosted": {"dir": "hosted", "kind": "host", "note": "the host itself"},
            "mapped-not": {"dir": "mapped-not", "kind": "unmapped"},
            "weird": {"dir": "weird", "kind": "quantum"}
        }}"#,
        )
        .unwrap()
    }

    #[test]
    fn robot_models_resolve_shipped_then_built() {
        let dir = scratch("robot");
        let drivers = dir.join("drivers");
        std::fs::create_dir_all(drivers.join("built")).unwrap();
        std::fs::write(drivers.join("built").join("libbuilt.so"), "elf").unwrap();
        let bundle_lib = dir.join("lib");
        std::fs::create_dir_all(&bundle_lib).unwrap();
        let manifest = fake_manifest();
        // Built beside the manifest: found.
        assert_eq!(
            resolve_model("built", &manifest, Some(&drivers), &bundle_lib).unwrap(),
            drivers.join("built").join("libbuilt.so")
        );
        // Shipped in the bundle wins over built beside the manifest.
        std::fs::write(bundle_lib.join("libbuilt.so"), "elf").unwrap();
        assert_eq!(
            resolve_model("built", &manifest, Some(&drivers), &bundle_lib).unwrap(),
            bundle_lib.join("libbuilt.so")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn robot_models_refuse_typed() {
        let dir = scratch("robot-refuse");
        let drivers = dir.join("drivers");
        std::fs::create_dir_all(&drivers).unwrap();
        let bundle_lib = dir.join("lib");
        std::fs::create_dir_all(&bundle_lib).unwrap();
        let manifest = fake_manifest();

        let err = resolve_model("nousch", &manifest, Some(&drivers), &bundle_lib).unwrap_err();
        assert!(
            err.contains("no such robot model") && err.contains("built"),
            "{err}"
        );

        let err = resolve_model("built", &manifest, Some(&drivers), &bundle_lib).unwrap_err();
        assert!(
            err.contains("is not built") && err.contains("make -C"),
            "{err}"
        );

        let err = resolve_model("sdked", &manifest, Some(&drivers), &bundle_lib).unwrap_err();
        assert!(err.contains("SDKED_SDK"), "{err}");

        let err = resolve_model("simmed", &manifest, Some(&drivers), &bundle_lib).unwrap_err();
        assert!(
            err.contains("simulation-only") && err.contains("--robot mock"),
            "{err}"
        );

        let err = resolve_model("snaked", &manifest, Some(&drivers), &bundle_lib).unwrap_err();
        assert!(err.contains("Python") && err.contains("README"), "{err}");

        let err = resolve_model("hosted", &manifest, Some(&drivers), &bundle_lib).unwrap_err();
        assert!(err.contains("not a robot"), "{err}");

        let err = resolve_model("mapped-not", &manifest, Some(&drivers), &bundle_lib).unwrap_err();
        assert!(
            err.contains("no install mapping") && err.contains("--driver"),
            "{err}"
        );

        let err = resolve_model("weird", &manifest, Some(&drivers), &bundle_lib).unwrap_err();
        assert!(err.contains("unknown kind"), "{err}");

        // Without a checkout, only the shipped library can resolve.
        let err = resolve_model("built", &manifest, None, &bundle_lib).unwrap_err();
        assert!(
            err.contains("ships no") && err.contains("--driver"),
            "{err}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A scratch checkout: `drivers/models.json` at the top, per the shape
    /// [`manifest_above`] climbs for. The manifest's content is irrelevant here —
    /// only that the file exists where a checkout keeps it.
    fn fake_checkout() -> std::path::PathBuf {
        let root = scratch("checkout");
        std::fs::create_dir_all(root.join("drivers")).unwrap();
        std::fs::write(root.join("drivers").join("models.json"), "{}").unwrap();
        root
    }

    #[test]
    fn manifest_climb_finds_checkout_from_directory() {
        let root = fake_checkout();
        let deep = root.join("crates").join("skynet-cli");
        std::fs::create_dir_all(&deep).unwrap();
        // The working tree itself, and a directory three climbs down from it.
        for start in [&root, &deep] {
            let (file, drivers) = manifest_above(start).unwrap();
            assert_eq!(file, root.join("drivers").join("models.json"));
            assert_eq!(drivers, Some(root.join("drivers")));
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn manifest_climb_starts_at_parent_for_files() {
        let root = fake_checkout();
        // A binary sitting in the checkout's bin directory: the climb starts beside
        // it, not in a `drivers` directory of its own that it does not have.
        let bin = root.join("target").join("release");
        std::fs::create_dir_all(&bin).unwrap();
        let exe = bin.join("skynet");
        std::fs::write(&exe, "elf").unwrap();
        let (file, drivers) = manifest_above(&exe).unwrap();
        assert_eq!(file, root.join("drivers").join("models.json"));
        assert_eq!(drivers, Some(root.join("drivers")));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn manifest_climb_misses_bare_tree() {
        let dir = scratch("bare");
        let deep = dir.join("a").join("b");
        std::fs::create_dir_all(&deep).unwrap();
        assert!(manifest_above(&deep).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_bad_tier_is_refused_here_not_on_the_robot() {
        let bundle = fake_bundle(62);
        let driver = bundle.join("lib").join("libskynet_mock.so");
        let descriptor = bundle.join("body.json");
        std::fs::write(&descriptor, "{\"joints\": []}").unwrap();
        let args = Args::parse(
            &argv(&[
                "robot.local",
                "--bundle",
                bundle.to_str().unwrap(),
                "--driver",
                driver.to_str().unwrap(),
                "--descriptor",
                descriptor.to_str().unwrap(),
                "--tier",
                "ultra",
                "--dry-run",
            ]),
            &SPEC,
        )
        .unwrap();
        let mut output = Vec::new();
        let mut input = Cursor::new(b"");
        let err = run_inner(&args, &mut input, &mut output).unwrap_err();
        assert!(err.contains("--tier"), "{err}");
        std::fs::remove_dir_all(&bundle).unwrap();
    }
}

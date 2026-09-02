//! `skynet` — the workstation CLI.
//!
//! §18 sketches the command set and §65 gives the reading order a developer actually follows. This
//! binary is the part of both that can exist before a robot does:
//!
//! ```text
//! skynet descriptor from-urdf robot.urdf > embodiment.json   §63.4
//! skynet driver new my-arm --lang c                          §63.1
//! skynet driver check ./my-arm/libmy_arm.so                  §63.3
//! skynet transport ./policy.json --robot embodiment.json     §18
//! skynet role left_arm.shoulder.pitch                        §5.1
//! ```
//!
//! and, since `skynetd` exists, the commands that need a robot on the other end (§33.7):
//!
//! ```text
//! skynet status · ps · plan · logs · grants · estop · rollback     §18, over the local socket
//! skynet install ./package --id ns/name --kind policy …            §37.1, into the store
//! ```
//!
//! One is still absent, and is absent rather than stubbed: `pair` needs §10's attestation path,
//! which is not written. A command that printed a plausible answer from nothing is the failure this
//! project is arranged to avoid.
//!
//! # `std`, and one `unsafe` module
//!
//! The workspace's other crates are `no_std` and `forbid(unsafe_code)`. This one is a workstation
//! tool that reads files and, in [`dl`], loads a shared object — which cannot be done safely by
//! construction, because the thing being loaded is arbitrary native code. The unsafe is confined to
//! that module and every block in it carries the reason it is sound.

#![warn(missing_docs)]

mod args;
mod check;
mod descriptor_cmd;
mod dl;
mod driver_cmd;
mod frames_cmd;
mod robot_cmd;
mod scaffold;
mod term;
mod transport_cmd;

use skynet_contracts::parse_role;

/// The version this binary reports, and stamps into a descriptor's provenance.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
skynet — Skynet OS workstation tools

  descriptor from-urdf <file>     Draft an embodiment descriptor from a URDF (§63.4)
  driver new <name>               Scaffold a driver that builds and fails honestly (§63.1)
  driver check <library>          Run the sim half of SCTS against a driver (§63.3)
  transport <policy> --robot <d>  What this body would do with this policy (§18)
  frames <descriptor>             The frame graph, its poses, and what the pose
                                  guards can do with it (§44)
  role <role>...                  Parse canonical roles (§5.1)
  version                         Version, ABI version and contract version

  Talking to a robot (§33.7), over /run/skynetd.sock or --host:

  status · ps · plan · logs · grants · estop · rollback
  install <dir> --id <ns/name> --kind <k> --version <v> --digest <hex>
  blackbox [index] · blackbox seal · blackbox get <name>

  --help after any command prints its options.

  pair is not here: it needs the attestation path (§10), which is not written,
  so it is absent rather than stubbed.";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(dispatch(&argv));
}

fn dispatch(argv: &[String]) -> i32 {
    let Some(command) = argv.first().map(String::as_str) else {
        println!("{USAGE}");
        return 0;
    };
    let rest = &argv[1..];

    match command {
        "--help" | "-h" | "help" => {
            println!("{USAGE}");
            0
        }
        "--version" | "version" => {
            println!("skynet {VERSION}");
            println!("rhal abi {}", skynet_rhal::ABI_VERSION);
            println!("contracts {}", skynet_contracts::CONTRACTS_VERSION);
            0
        }
        "descriptor" => match rest.first().map(String::as_str) {
            Some("from-urdf") => {
                run_or_help(&rest[1..], descriptor_cmd::USAGE, descriptor_cmd::run)
            }
            Some("--help") | Some("-h") | None => {
                println!("{}", descriptor_cmd::USAGE);
                0
            }
            Some(other) => term::fail(&format!(
                "`descriptor {other}` is not a subcommand. The only one is `from-urdf`."
            )),
        },
        "driver" => driver_cmd::run(rest),
        "transport" => run_or_help(rest, transport_cmd::USAGE, transport_cmd::run),
        "frames" => run_or_help(rest, frames_cmd::USAGE, frames_cmd::run),
        "role" => role(rest),
        "status" | "ps" | "plan" | "logs" | "grants" | "estop" | "rollback" | "install"
        | "blackbox" | "skills" | "skill" | "ota" | "identity" | "pair" | "unpair" | "checkin"
        | "intent" | "releases" => robot_cmd::run(command, rest),
        other => term::fail(&format!("`{other}` is not a skynet command.\n\n{USAGE}")),
    }
}

fn run_or_help(rest: &[String], usage: &str, run: fn(&[String]) -> i32) -> i32 {
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{usage}");
        return 0;
    }
    run(rest)
}

/// `skynet role` — parse role strings against the canonical grammar (§5.1).
///
/// Small, and worth having: the grammar's whole value is that an unrecognised role returns nothing
/// rather than something plausible, and a developer writing a mapping table wants to find that out
/// at the keyboard rather than from a transport plan that silently dropped a channel.
fn role(rest: &[String]) -> i32 {
    if rest.is_empty() || rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "skynet role <role>...\n\n  \
             role := chain \".\" link [ \".\" axis ] [ \"#\" index ]\n  \
             chain ∈ {}\n  \
             axis  ∈ {}\n\n  \
             Anything that does not parse returns nothing rather than something plausible: a joint\n  \
             with an unparseable role can only ever be matched by exact name, and never silently\n  \
             maps to something close.\n\n  \
             Exit code is 1 if any argument fails to parse.",
            skynet_contracts::roles::CANONICAL_CHAINS.join(" "),
            skynet_contracts::roles::CANONICAL_AXES.join(" "),
        );
        return 0;
    }

    let mut failed = false;
    for raw in rest {
        match parse_role(Some(raw)) {
            Some(parsed) => {
                let mut parts = format!("chain={} link={}", parsed.chain, parsed.link);
                if let Some(axis) = &parsed.axis {
                    parts.push_str(&format!(" axis={axis}"));
                }
                if let Some(index) = parsed.index {
                    parts.push_str(&format!(" index={index}"));
                }
                println!("{} {raw}  {}", term::ok("✓"), term::dim(&parts));
                if parsed.key() != *raw {
                    println!(
                        "  {}",
                        term::dim(&format!("canonical form: {}", parsed.key()))
                    );
                }
            }
            None => {
                failed = true;
                println!(
                    "{} {raw}  {}",
                    term::bad("✗"),
                    term::dim("does not parse; this role can only be matched by exact joint name")
                );
            }
        }
    }
    i32::from(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn no_arguments_prints_usage_and_succeeds() {
        assert_eq!(dispatch(&[]), 0);
        assert_eq!(dispatch(&argv(&["--help"])), 0);
        assert_eq!(dispatch(&argv(&["version"])), 0);
    }

    #[test]
    fn an_unknown_command_fails_and_the_usage_names_the_real_ones() {
        assert_eq!(dispatch(&argv(&["deploy"])), 1);
        assert!(USAGE.contains("descriptor from-urdf"));
        assert!(USAGE.contains("driver check"));
    }

    /// The two §18 commands that still have nothing behind them are absent, and the usage says
    /// why. A stub that printed a plausible fleet status would be the exact failure the project is
    /// arranged to avoid.
    #[test]
    fn pair_is_absent_and_named_as_absent() {
        // One command, where there were two: `blackbox` became real this session. The loop stays,
        // because the property is about the *set* of commands named as absent rather than about
        // whichever of them is currently in it.
        let absent = ["pair"];
        for command in absent {
            assert_eq!(dispatch(&argv(&[command])), 1, "`{command}` should not run");
            assert!(
                USAGE.contains(command),
                "usage should name `{command}` as absent"
            );
        }
    }

    /// And the ones that now have a daemon behind them are dispatched rather than refused. They
    /// fail here because there is no robot on the other end of the socket, which is a different
    /// answer from "no such command" — and the message says which.
    #[test]
    fn the_robot_commands_are_dispatched_and_fail_by_not_reaching_a_robot() {
        for command in ["status", "ps", "plan", "grants", "estop"] {
            let code = dispatch(&argv(&[command, "--socket", "/nonexistent/skynetd.sock"]));
            assert_eq!(code, 1, "`{command}` has nothing to talk to here");
            assert!(USAGE.contains(command), "usage should name `{command}`");
        }
    }

    #[test]
    fn roles_parse_or_report_that_they_did_not() {
        assert_eq!(dispatch(&argv(&["role", "left_arm.shoulder.pitch"])), 0);
        assert_eq!(dispatch(&argv(&["role", "base.wheel#2"])), 0);
        assert_eq!(dispatch(&argv(&["role", "elbow"])), 1);
        assert_eq!(dispatch(&argv(&["role", "third_arm.shoulder.pitch"])), 1);
        // One failure among successes still fails.
        assert_eq!(dispatch(&argv(&["role", "torso.waist.yaw", "nonsense"])), 1);
    }

    #[test]
    fn descriptor_has_exactly_one_subcommand() {
        assert_eq!(dispatch(&argv(&["descriptor"])), 0);
        assert_eq!(dispatch(&argv(&["descriptor", "to-urdf"])), 1);
    }
}

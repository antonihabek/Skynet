//! `skynet discover` — which robots are on this link (§82).
//!
//! Every other command in this CLI needs to know where a robot is: `--host`, or the Unix socket of
//! the robot you are already logged in to. §82.1 is the record of why that is a hole rather than a
//! convenience — §34.2 binds the console link-local *on purpose*, and a link-local IPv6 address
//! with a zone index is not something anybody reads off a chassis. This is the one command that
//! answers the question the others assume.
//!
//! # What it will not do
//!
//! **There is no watch mode**, and that is a property of the transport rather than a missing flag.
//! [`skynet_mdns::browse`] cannot hold port 5353 (§82.3), so it asks with RFC 6762 §5.4's QU bit and
//! sees answers to its own question and nothing else. A robot that boots a second after the browse
//! is not in the result. A watch built on that would silently miss arrivals, and a watch that misses
//! arrivals is worse to offer than no watch.
//!
//! **It does not connect.** Discovery says a robot answered and where; whether it will let you in
//! is `skynet status`'s question, one credential later.

use skynet_mdns::browse::{self, Outcome};
use skynet_mdns::service::Discovered;

use crate::args::{Args, Spec};
use crate::term;

/// What this command accepts. An unrecognised flag is an error here as everywhere else.
const SPEC: Spec = Spec {
    values: &["timeout"],
    switches: &[],
};

/// `--help` for this command.
pub const USAGE: &str = "\
skynet discover [--timeout <ms>]

  Ask the local link which robots are on it (§82). Prints what answered, and — when nothing
  did — how well it was able to look.

  --timeout <ms>   How long to wait for answers. Default 2000.

  A robot appears here only if it is advertising: `skynetd --lan <addr:port>` starts the
  responder, and a daemon serving no console advertises nothing on purpose.

  There is no watch mode. See this command's notes: the browse cannot see announcements it
  did not ask for, so a robot that boots after the question is not in the answer.";

/// Run the browse.
pub fn run(argv: &[String]) -> i32 {
    let args = match Args::parse(argv, &SPEC) {
        Ok(args) => args,
        Err(problem) => return term::fail(&problem),
    };
    let timeout = match args.value("timeout") {
        None => browse::DEFAULT_TIMEOUT,
        Some(text) => match text.parse::<u64>() {
            Ok(ms) if ms > 0 => std::time::Duration::from_millis(ms),
            _ => return term::fail("--timeout takes a whole number of milliseconds"),
        },
    };

    match browse::browse_for(timeout) {
        // The distinction this whole module exists to preserve: an empty list is a statement about
        // robots, and this is a statement about the machine the browse ran on.
        Err(error) => term::fail(&format!("{error}")),
        Ok(outcome) => {
            report(&outcome);
            0
        }
    }
}

/// Print what a browse found, and what it could not tell.
fn report(outcome: &Outcome) {
    if outcome.robots.is_empty() {
        println!("No robot answered.");
    }
    for robot in &outcome.robots {
        print_robot(robot);
    }
    println!("{}", term::dim(&summary(outcome)));
}

fn print_robot(robot: &Discovered) {
    let name = if robot.goodbye {
        // A goodbye is the most useful thing a discovery tool can be told, and rendering it as an
        // ordinary entry would list a robot that has just said it is leaving.
        format!("{} {}", robot.label, term::warn("(leaving)"))
    } else {
        term::bold(&robot.label)
    };
    println!("{name}");

    match robot.url() {
        Some(url) => println!("  {}", url),
        // §82.5: a link-local IPv6 address with no zone is not an address, and printing one that
        // will fail to connect with a confusing error is the hole this command exists to close,
        // one layer further in.
        None => println!(
            "  {}",
            term::dim("answered, but with no address this machine can connect to")
        ),
    }
    for address in &robot.addresses {
        println!("  {}", term::dim(&address.to_string()));
    }
}

/// The counters, in one line, because an empty list has several causes.
fn summary(outcome: &Outcome) -> String {
    let mut line = format!(
        "{} robot(s), asked on {} socket(s)",
        outcome.robots.len(),
        outcome.interfaces_asked
    );
    if outcome.malformed > 0 {
        line.push_str(&format!(", {} malformed datagram(s)", outcome.malformed));
    }
    if outcome.other_services > 0 {
        line.push_str(&format!(
            ", {} answer(s) about another service",
            outcome.other_services
        ));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn discovered(label: &str) -> Discovered {
        let advertisement = skynet_mdns::service::advertisement(
            label,
            label,
            8099,
            &[IpAddr::V4(Ipv4Addr::new(169, 254, 3, 4))],
            false,
        )
        .unwrap();
        skynet_mdns::service::read(&advertisement, None).remove(0)
    }

    #[test]
    fn the_summary_names_the_sockets_it_asked_on() {
        let outcome = Outcome {
            robots: vec![discovered("arm-01")],
            interfaces_asked: 2,
            ..Outcome::default()
        };
        assert_eq!(summary(&outcome), "1 robot(s), asked on 2 socket(s)");
    }

    #[test]
    fn a_noisy_link_says_so_beside_an_empty_list() {
        let outcome = Outcome {
            interfaces_asked: 1,
            malformed: 3,
            other_services: 5,
            ..Outcome::default()
        };
        let line = summary(&outcome);
        assert!(line.contains("0 robot(s)"), "{line}");
        assert!(line.contains("3 malformed"), "{line}");
        assert!(
            line.contains("5 answer(s) about another service"),
            "a link with printers on it is not a link with nothing on it: {line}"
        );
    }

    #[test]
    fn a_robot_that_answered_prints_the_url_a_browser_can_open() {
        let robot = discovered("arm-01");
        assert_eq!(robot.url().as_deref(), Some("http://169.254.3.4:8099/"));
    }
}

//! `skynet doctor` — §41.3's bundle.
//!
//! > One command producing one bundle: versions and edition, image slot and generation lag, active
//! > and unavailable guards with their missing inputs, resolved limits and provenance, package list
//! > with budgets versus observed usage, the last N black box events, connectivity and queue depth,
//! > thermal and battery history, and the last reconcile plan including anything deferred and why.
//! >
//! > It is the first thing to ask for in a support conversation, and it exists so that the answer
//! > to "what is wrong with this robot" is a file rather than an interview.
//!
//! Specified since Part IV and unwritten until now, which made the sentence above false in the one
//! way that matters: the answer *was* an interview, conducted one `skynet` command at a time by
//! somebody who already knew which ones to run.
//!
//! # It runs on the workstation, and that is the whole reason it can exist at all
//!
//! Every section is a route on §33.7's local API, so this command is an **aggregator** and holds no
//! second opinion about anything. It works over the socket on the robot and over `--host` from a
//! bench, and the second is what makes it usable: the person who needs the bundle is usually not
//! sitting at the machine, and §34.2 binds the console link-local precisely so they can reach it
//! when the Hub cannot.
//!
//! Nothing is computed here that a route already answers. A `doctor` that derived its own view of
//! the guards would be a second implementation of §7.2's partition, and this repository has §78 as
//! the record of what that costs.
//!
//! # A route that could not be read is recorded as unread, with its reason
//!
//! The single rule this file is built on, and the one that decides whether the bundle is worth
//! having. A robot that answers `403` for `/audit` and `503` for `/blackbox` is **telling you
//! something** — no operator credential was presented, and the recorder is not running — and a
//! bundle that omitted those sections, or wrote them as empty objects, would erase the two most
//! interesting facts in it.
//!
//! So every section is present in the output whatever happened, carrying either a `document` or an
//! `unread` sentence, never both and never neither. The summary counts them separately for the same
//! reason: *nine of eleven sections read* is a different sentence from *the bundle is complete*.
//!
//! # Why `--out` writes a file rather than the command printing JSON
//!
//! §41.3 asks for a *file*, because the thing that has to survive the conversation is not what the
//! operator saw at the time. The terminal gets a summary — the half a person acts on — and the file
//! gets everything, including the sections the summary has no line for.

use skynet_json::Json;

use crate::robot_cmd::{request, Target};
use crate::term;

/// `skynet doctor`'s own usage.
pub const USAGE: &str =
    "skynet doctor — one bundle, for the question 'what is wrong with this robot' (§41.3)

  skynet doctor [--out <file>] [--socket <path> | --host <addr:port>] [--token <t>]

  Reads every route §41.3 names, writes one JSON bundle, and prints the half a person acts on.
  A route it could not read is recorded as unread with the reason, never as empty — a robot
  refusing `/audit` for want of a credential, or answering 503 because its recorder is not running,
  is telling you something the bundle must not swallow.

  --out <file>      Where the bundle goes. Default `skynet-doctor.json` in the working directory.
  --socket <path>   Default /run/skynetd.sock
  --host <addr:port>  The LAN console instead of the socket
  --token <t>       Or $SKYNET_TOKEN. Without one, the observer routes still answer and the rest
                    are recorded as refused — which is itself a readable bundle.
  --events <n>      How many black box events to carry. Default 200.";

/// One section of the bundle: a route, and what came back.
struct Section {
    /// The name §41.3's list gives it.
    name: &'static str,
    /// The route, printed so a reader can reproduce the request by hand.
    route: String,
    /// The status, or `None` when the request never got one.
    status: Option<u16>,
    /// What came back, or the reason nothing did. Exactly one is `Some`.
    document: Option<Json>,
    /// Why this section is empty. See the module header.
    unread: Option<String>,
}

impl Section {
    fn to_json(&self) -> Json {
        let mut row = Json::obj();
        row.set("section", Json::from(self.name));
        row.set("route", Json::Str(self.route.clone()));
        // Not zero for a request that never reached a status: a reader summing statuses should not
        // be handed one it can mistake for an answer.
        row.set(
            "status",
            match self.status {
                Some(status) => Json::Num(status as f64),
                None => Json::Null,
            },
        );
        match (&self.document, &self.unread) {
            (Some(document), _) => row.set("document", document.clone()),
            (None, Some(reason)) => row.set("unread", Json::Str(reason.clone())),
            // Unreachable by construction: `fetch` produces one or the other. Handled rather than
            // asserted, because a panic in the command somebody runs when a robot is already
            // broken is the worst possible time for one.
            (None, None) => row.set("unread", Json::from("no answer and no reason was recorded")),
        };
        row
    }
}

/// What §41.3 asks for, in the order it asks for it.
///
/// Five are additions and the rest are the specification's list. §41.3 opens with *"versions and
/// edition"* and the four routes above that line are the questions a support conversation actually
/// opens with — **what state is this robot in**, **is its supervisor healthy**, and the counters
/// behind both — so a bundle that made somebody open a second tool for them would not be the file
/// §41.3 describes. All are observer-readable. The fifth addition is the audit trail at the end:
/// forensics asks *who changed what* after *what crashed*, and the bundle is the file that
/// answers without a second trip to the robot.
const SECTIONS: &[(&str, &str)] = &[
    ("state", "/state"),
    ("health", "/health"),
    // The daemon's own counters, beside the health they join to: a bundle that says the
    // supervisor is unhealthy without saying how many passes overran or which unit is
    // crash-looping is half the file §41.3 describes.
    ("metrics", "/metrics"),
    ("status", "/status"),
    // "image slot and generation lag" — two routes, because the slot is the robot's and the lag is
    // between the robot and the Hub.
    ("ota", "/ota"),
    ("intent", "/intent"),
    // "active and unavailable guards with their missing inputs, resolved limits and provenance" —
    // one route answers both halves, which is why §7.6 put them in one document.
    ("limits", "/limits"),
    // "package list with budgets versus observed usage"
    ("packages", "/packages"),
    // "connectivity and queue depth"
    ("outbound", "/outbound"),
    // "thermal and battery history"
    ("power", "/power"),
    // "the last reconcile plan including anything deferred and why"
    ("plan", "/plan"),
    // The window's shape, beside the events below it: how much recording there is, and what is in
    // the recent end of it.
    ("blackbox", "/blackbox"),
    // Who changed what, when: the trust store, the profiles, the session override. Forensics
    // asks for it after "what crashed", and a bundle without it sends somebody back to the
    // robot for the second tool §41.3 exists to replace.
    ("audit", "/audit"),
];

/// Run it.
pub fn run(argv: &[String]) -> i32 {
    if argv.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return 0;
    }

    let out = flag(argv, "--out").unwrap_or_else(|| "skynet-doctor.json".to_string());
    let events = flag(argv, "--events").unwrap_or_else(|| "200".to_string());

    let mut sections: Vec<Section> = SECTIONS
        .iter()
        .map(|(name, path)| fetch(argv, name, path))
        .collect();
    // §41.3's "last N black box events", which is a route with a parameter rather than a bare one.
    sections.push(fetch_at(
        argv,
        "blackbox_events",
        "/blackbox/events",
        &format!("/blackbox/events?limit={events}"),
    ));

    let read = sections.iter().filter(|s| s.document.is_some()).count();
    let bundle = assemble(argv, &sections);

    // The summary is printed whatever happens to the file. Everything in it was read from the
    // robot before the disk was touched, and a write failure is not a reason to withhold what was
    // already learned from somebody standing next to a robot that is not working.
    let written = match encode(&bundle) {
        Ok(text) => std::fs::write(&out, text).map_err(|error| {
            format!(
                "the bundle could not be written to {out}: {error}. Everything above was read \
                 from the robot; only the file failed."
            )
        }),
        Err(reason) => Err(reason),
    };
    summarise(&sections, read);

    match written {
        Ok(()) => {
            println!();
            println!("{} {}", term::ok("bundle"), out);
            // The count, always, and stated as a fraction. A bundle whose header said only
            // "written" would let eleven refused sections read as a healthy robot.
            // "the rest" only when there is a rest. The first version said it unconditionally, so
            // a complete bundle announced twelve of twelve and then referred to sections that do
            // not exist — a small thing, and the kind a person stops reading the tool over.
            println!(
                "{}",
                term::dim(&if read == sections.len() {
                    format!("all {read} sections read")
                } else {
                    format!(
                        "{read} of {} sections read; the rest are in the file with the reason \
                         they are not",
                        sections.len()
                    )
                })
            );
            0
        }
        Err(reason) => term::fail(&reason),
    }
}

fn fetch(argv: &[String], name: &'static str, path: &str) -> Section {
    fetch_at(argv, name, path, path)
}

/// `route` is what gets recorded; `path` is what gets requested.
///
/// They differ for exactly one section — the events route carries a `limit` — and separating them
/// keeps the bundle's `route` field readable as *the route* rather than as one particular call of
/// it. The query is not lost: it is in the document's own `limit` and `ceiling`.
fn fetch_at(argv: &[String], name: &'static str, route: &str, path: &str) -> Section {
    match request(argv, "GET", path, None) {
        Err(reason) => Section {
            name,
            route: format!("GET {route}"),
            status: None,
            document: None,
            unread: Some(reason),
        },
        Ok((status, document)) if status >= 400 => {
            let detail = document
                .get_present("detail")
                .and_then(Json::as_str)
                .unwrap_or("no detail")
                .to_string();
            // The code rides along when the daemon sends one: an unread section that says *why*
            // in a matchable word is data a support conversation can act on without re-asking.
            let unread = match document.get_present("code").and_then(Json::as_str) {
                Some(code) => format!("the robot answered {status} [{code}]: {detail}"),
                None => format!("the robot answered {status}: {detail}"),
            };
            Section {
                name,
                route: format!("GET {route}"),
                status: Some(status),
                document: None,
                unread: Some(unread),
            }
        }
        Ok((status, document)) => Section {
            name,
            route: format!("GET {route}"),
            status: Some(status),
            document: Some(document),
            unread: None,
        },
    }
}

fn assemble(argv: &[String], sections: &[Section]) -> Json {
    let mut bundle = Json::obj();
    bundle.set("schema", Json::from("skynet.doctor.v1"));
    // The wall clock, because this file is read hours later beside a Hub's own timestamps, and
    // §35.1's monotonic clock says nothing to somebody comparing two machines. Zero would be a
    // time; a platform that cannot answer gets null.
    bundle.set(
        "taken_at_wall_ms",
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(since) => Json::Num(since.as_millis() as f64),
            Err(_) => Json::Null,
        },
    );
    bundle.set(
        "target",
        Json::Str(match crate::robot_cmd::target(argv) {
            Target::Socket(path) => format!("socket:{}", path.display()),
            Target::Host(host) => format!("host:{host}"),
        }),
    );
    // Which tool produced it. A bundle read against the wrong edition's expectations is a bundle
    // read wrongly, and the version that matters is the *reader's* as much as the robot's.
    let mut tool = Json::obj();
    tool.set("skynet", Json::from(crate::VERSION));
    tool.set("contracts", Json::from(skynet_contracts::CONTRACTS_VERSION));
    bundle.set("tool", tool);
    bundle.set(
        "sections",
        Json::Arr(sections.iter().map(Section::to_json).collect()),
    );
    bundle
}

/// The half a person acts on.
///
/// Every line here is lifted from a document that was actually read; nothing is derived and nothing
/// is defaulted. A section that could not be read prints as unread rather than as a dash, because
/// a dash reads as *the robot has none of that* and unread means *nobody asked it successfully*.
fn summarise(sections: &[Section], read: usize) {
    println!();

    // A robot that never answered is **one** sentence, not eighteen copies of one sentence.
    //
    // The first version of this printed the same connection error once per section and once more
    // per unread row, which is the failure §64 is about wearing a different hat: the output was
    // accurate and unreadable, and the one fact in it — *nothing answered* — was the hardest thing
    // in it to find. A `doctor` run against an unplugged robot is a common way to run it, and it
    // should look like a short answer rather than like a fault in the tool.
    //
    // Distinguished by the status, not by the text: `None` means the request never reached a
    // status, which is exactly *the robot did not answer*. A robot that answered `403` to
    // everything has told you something quite different, and still gets the full list.
    if read == 0 && sections.iter().all(|section| section.status.is_none()) {
        println!("{}", term::bold("this robot did not answer"));
        println!();
        println!(
            "  {}",
            sections
                .first()
                .and_then(|section| section.unread.as_deref())
                .unwrap_or("no reason was recorded")
        );
        println!();
        println!(
            "{}",
            term::dim(
                "Every section is in the bundle with that reason. Nothing here is a statement \
                 about the robot: it is a statement about the link."
            )
        );
        return;
    }

    println!("{}", term::bold("what this robot says about itself"));
    println!();

    let find = |name: &str| sections.iter().find(|section| section.name == name);
    let doc = |name: &str| find(name).and_then(|section| section.document.as_ref());

    // §27's state, first, because it is the first question.
    match doc("state") {
        Some(state) => {
            let word = state
                .get_present("state")
                .and_then(Json::as_str)
                .unwrap_or("—");
            let because = state
                .get_present("because")
                .and_then(Json::as_str)
                .unwrap_or("");
            println!(
                "  state       {}{}",
                term::bold(word),
                if because.is_empty() {
                    String::new()
                } else {
                    format!("  {}", term::dim(because))
                }
            );
        }
        None => println!("  state       {}", unread_of(find("state"))),
    }

    // §64's crash, when the health document carries one — directly under the state, because it is
    // the fact the rest of the summary is read under. Nothing when it does not: null means no
    // crash was recorded, which is the uninteresting case, and an unread health section is already
    // listed below with its reason rather than needing a second line here.
    if let Some(health) = doc("health") {
        if let Some(line) = previous_exit_line(health) {
            println!("  previous    {}", term::bad(&line));
        }
        // The kernel host's crash, beside the supervisor's (§64). Same silence rule: null
        // means no crash noticed yet, which is the uninteresting case.
        if let Some(line) = kernel_exit_line(health) {
            println!("  kernel      {}", term::bad(&line));
        }
    }

    // The loop's timing, when it has missed. Silence at zero, like the line above: the summary
    // is the half a person acts on, and a loop that has never overrun needs no line saying so.
    if let Some(metrics) = doc("metrics") {
        if let Some(line) = overruns_line(metrics) {
            println!("  passes      {}", term::warn(&line));
        }
    }

    match doc("status") {
        Some(status) => {
            let text = |key: &str| {
                status
                    .get_present(key)
                    .and_then(Json::as_str)
                    .unwrap_or("—")
                    .to_string()
            };
            println!(
                "  version     skynetd {} · edition {} · contracts {}",
                text("version"),
                text("edition"),
                text("contracts")
            );
        }
        None => println!("  version     {}", unread_of(find("status"))),
    }

    // §7.2's partition, in the three words it actually has.
    //
    // The first version of this counted a `guards` array with a `state` field on each row, which
    // `GET /limits` does not have — so it printed **"0 of 0 active"** for every robot, which reads
    // as a measurement and is an artefact of looking for a key that is not there. Caught by running
    // it against a robot rather than by reading the route; §7.6's document names the three lists
    // separately (`active_guards`, `inert_guards`, `unavailable_guards`) and §80 spends a row on
    // why the third and the second are not the same absence.
    match doc("limits") {
        Some(limits) => {
            let active = limits.arr_or_empty("active_guards").len();
            let inert = limits.arr_or_empty("inert_guards").len();
            let unavailable = limits.arr_or_empty("unavailable_guards").len();
            let mut line = format!("{active} active");
            // Named rather than summed into "not active". An inert guard has every input and wants
            // a producer in the operating system; an unavailable one wants a sensor or a profile.
            // Those are different errands for different people (§80).
            if inert > 0 {
                line.push_str(&format!(", {}", term::warn(&format!("{inert} inert"))));
            }
            if unavailable > 0 {
                line.push_str(&format!(
                    ", {}",
                    term::dim(&format!("{unavailable} unavailable"))
                ));
            }
            println!("  guards      {line}");
        }
        None => println!("  guards      {}", unread_of(find("limits"))),
    }

    match doc("packages") {
        Some(packages) => {
            let rows = packages.arr_or_empty("packages");
            let running = rows
                .iter()
                .filter(|row| row.get_present("state").and_then(Json::as_str) == Some("running"))
                .count();
            println!("  packages    {running} of {} running", rows.len());
        }
        None => println!("  packages    {}", unread_of(find("packages"))),
    }

    // §34.4's queue, summed over the classes it is actually kept in — and the dropped count beside
    // it, because a queue that is empty because everything in it was thrown away is not an empty
    // queue in any sense an operator cares about.
    match doc("outbound") {
        Some(outbound) => {
            let classes = outbound.arr_or_empty("classes");
            let sum = |key: &str| -> u64 {
                classes
                    .iter()
                    .filter_map(|row| row.get(key).and_then(Json::as_f64))
                    .sum::<f64>() as u64
            };
            let held = sum("held");
            let dropped = sum("dropped");
            println!(
                "  outbound    {held} held{}",
                if dropped == 0 {
                    String::new()
                } else {
                    format!("  {}", term::warn(&format!("{dropped} dropped")))
                }
            );
        }
        None => println!("  outbound    {}", unread_of(find("outbound"))),
    }

    match doc("blackbox_events") {
        Some(events) => {
            let rows = events.arr_or_empty("events");
            let gaps = rows
                .iter()
                .filter(|row| row.get_present("kind").and_then(Json::as_str) == Some("gap"))
                .count();
            println!(
                "  events      {} carried, {} in the ring{}",
                rows.len(),
                events.get("held").and_then(Json::as_f64).unwrap_or(0.0) as u64,
                if gaps == 0 {
                    String::new()
                } else {
                    format!("  {}", term::bad(&format!("{gaps} gap(s)")))
                }
            );
        }
        None => println!("  events      {}", unread_of(find("blackbox_events"))),
    }

    if read < sections.len() {
        println!();
        for section in sections.iter().filter(|s| s.document.is_none()) {
            println!(
                "  {} {:<18} {}",
                term::warn("unread"),
                section.name,
                term::dim(section.unread.as_deref().unwrap_or("")),
            );
        }
    }
}

/// §64's previous exit, as one summary line when there is something to say.
///
/// Two of the four states say something: a crash carries its message, and an unclean exit
/// names its suspects. Clean and unknown are silence — the summary is the half a person
/// acts on, and neither asks for anything. `None` also covers the null and the missing
/// key, which are what daemons from before the states answer; a `doctor` that demanded
/// the object would report every old robot as unreadable in a new and confusing way.
fn previous_exit_line(health: &Json) -> Option<String> {
    let exit = health.get("previous_exit")?;
    match exit.get("state").and_then(Json::as_str) {
        Some("crashed") => {
            let message = exit
                .get("message")
                .and_then(Json::as_str)
                .unwrap_or("no message recorded");
            let location = exit.get("location").and_then(Json::as_str).unwrap_or("");
            Some(if location.is_empty() {
                format!("the previous supervisor crashed: {message}")
            } else {
                format!("the previous supervisor crashed: {message} ({location})")
            })
        }
        Some("unclean") => Some(
            "the previous supervisor left no record of its exit — killed, out of memory, \
             aborted, or power loss; the journal around the death is where that question is \
             answered"
                .to_string(),
        ),
        _ => None,
    }
}

/// The daemon's overruns, as one summary line when there are any.
///
/// `None` at zero and when the document is older than the counters — a daemon from before
/// `/metrics` answers 404, which is already listed below with its reason rather than needing
/// a second line here.
fn overruns_line(metrics: &Json) -> Option<String> {
    let daemon = metrics.get("daemon")?;
    let overruns = daemon.get("pass_overruns").and_then(Json::as_f64)? as u64;
    if overruns == 0 {
        return None;
    }
    let interval = daemon
        .get("interval_ms")
        .and_then(Json::as_f64)
        .unwrap_or(0.0) as u64;
    let passes = if overruns == 1 { "pass" } else { "passes" };
    Some(format!(
        "{overruns} {passes} overran the {interval}ms interval — the loop is falling behind"
    ))
}

/// §64's kernel exit, as one summary line when there is something to say.
///
/// The same shape as [`previous_exit_line`] for the other crash file: `None` renders nothing,
/// covering the null and the missing key — a daemon from before the field answers neither.
/// The binary names itself in the line, because two crash lines that both said "the previous
/// supervisor" would be read as one crash twice.
fn kernel_exit_line(health: &Json) -> Option<String> {
    let exit = health.get("kernel_exit")?;
    if exit.get("state").and_then(Json::as_str) != Some("crashed") {
        return None;
    }
    let binary = exit
        .get("binary")
        .and_then(Json::as_str)
        .unwrap_or("skynetk");
    let message = exit
        .get("message")
        .and_then(Json::as_str)
        .unwrap_or("no message recorded");
    let location = exit.get("location").and_then(Json::as_str).unwrap_or("");
    Some(if location.is_empty() {
        format!("{binary} crashed: {message}")
    } else {
        format!("{binary} crashed: {message} ({location})")
    })
}

fn unread_of(section: Option<&Section>) -> String {
    term::dim(
        section
            .and_then(|section| section.unread.as_deref())
            .unwrap_or("unread"),
    )
}

/// Serialise the bundle.
///
/// Canonical form, which is what every other document this platform writes uses — so a bundle can
/// be digested, diffed against an earlier run, or checked against the copy somebody emailed, and
/// two runs that found the same robot produce the same bytes.
///
/// A document that will not serialise is reported rather than silently truncated: `to_canonical`
/// refuses `NaN` and the infinities, and a robot reporting one of those in a number is a robot
/// whose bundle should say so loudly rather than arrive with a hole in it.
fn encode(document: &Json) -> Result<String, String> {
    let mut text = document.to_canonical().map_err(|error| {
        format!(
            "the bundle could not be serialised ({error:?}). One of the robot's own numbers is \
             not finite, which is itself the finding."
        )
    })?;
    text.push('\n');
    Ok(text)
}

/// This command's own reader, because it takes no positional arguments.
///
/// `robot_cmd`'s pair of helpers carries F62's table, which exists so that `positional` knows
/// which flags consume the word after them. `doctor` has no positionals for a flag to eat, so it
/// needs the table only if it ever grows one — and then this copy goes and the table gains
/// `--out` and `--events`.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn health_with(previous_exit: Json) -> Json {
        let mut health = Json::obj();
        health.set("previous_exit", previous_exit);
        health
    }

    #[test]
    fn a_crash_renders_as_one_line() {
        let mut exit = Json::obj();
        exit.set("state", Json::from("crashed"))
            .set("message", Json::from("index out of bounds"))
            .set("location", Json::from("src/api.rs:1234"));
        let line = previous_exit_line(&health_with(exit)).expect("a crash says something");
        assert!(line.contains("index out of bounds"), "{line}");
        assert!(line.contains("src/api.rs:1234"), "{line}");
    }

    #[test]
    fn no_crash_recorded_says_nothing() {
        assert!(previous_exit_line(&health_with(Json::Null)).is_none());
        // A daemon from before the field: the key is missing rather than null, and that is
        // silence too rather than a new way to call an old robot broken.
        assert!(previous_exit_line(&Json::obj()).is_none());
    }

    #[test]
    fn an_unclean_exit_names_its_suspects() {
        let mut exit = Json::obj();
        exit.set("state", Json::from("unclean"));
        let line = previous_exit_line(&health_with(exit)).expect("unclean says something");
        assert!(line.contains("no record"), "{line}");
        assert!(line.contains("journal"), "{line}");
    }

    #[test]
    fn a_clean_exit_says_nothing() {
        let mut exit = Json::obj();
        exit.set("state", Json::from("clean"));
        assert!(previous_exit_line(&health_with(exit)).is_none());
        let mut unknown = Json::obj();
        unknown.set("state", Json::from("unknown"));
        assert!(previous_exit_line(&health_with(unknown)).is_none());
    }

    fn metrics_with(overruns: f64) -> Json {
        let mut daemon = Json::obj();
        daemon
            .set("pass_overruns", Json::Num(overruns))
            .set("interval_ms", Json::Num(10_000.0));
        let mut metrics = Json::obj();
        metrics.set("daemon", daemon);
        metrics
    }

    #[test]
    fn overruns_render_as_one_line() {
        let line = overruns_line(&metrics_with(3.0)).expect("overruns say something");
        assert!(line.contains('3'), "{line}");
        assert!(line.contains("10000ms"), "{line}");
        let one = overruns_line(&metrics_with(1.0)).expect("one overrun says something");
        assert!(one.contains("1 pass overran"), "{one}");
    }

    #[test]
    fn no_overruns_says_nothing() {
        assert!(overruns_line(&metrics_with(0.0)).is_none());
        assert!(overruns_line(&Json::obj()).is_none());
    }

    #[test]
    fn a_kernel_crash_names_its_binary() {
        let mut exit = Json::obj();
        exit.set("state", Json::from("crashed"))
            .set("binary", Json::from("skynetk"))
            .set("message", Json::from("watchdog guard failed"))
            .set("location", Json::from("src/cycle.rs:9"));
        let mut health = Json::obj();
        health.set("kernel_exit", exit);
        let line = kernel_exit_line(&health).expect("a crash says something");
        assert!(line.contains("skynetk"), "{line}");
        assert!(line.contains("watchdog guard failed"), "{line}");
    }

    #[test]
    fn no_kernel_crash_says_nothing() {
        let mut health = Json::obj();
        health.set("kernel_exit", Json::Null);
        assert!(kernel_exit_line(&health).is_none());
        assert!(kernel_exit_line(&Json::obj()).is_none());
    }
}

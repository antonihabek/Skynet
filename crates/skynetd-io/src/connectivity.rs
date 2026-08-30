//! Bearers, and what may be sent over them (§34.3).
//!
//! §34.4's queue has been complete for two sessions and nothing has ever taken an item out of it.
//! That is the half this module adds — the *decision*, and a place to plug a transport into — and it
//! stops short of the transport itself for a reason stated plainly below.
//!
//! # The table, and the rule underneath it
//!
//! | Bearer | Preference | Metered | Used for |
//! |---|---|---|---|
//! | Ethernet | 1 | No | Everything, including bulk upload and OS images |
//! | Wi-Fi | 2 | Configurable | Everything unless marked metered |
//! | Cellular | 3 | Assume yes | Check-ins and incidents only, by default |
//!
//! Three rows, and **one rule**: an unmetered bearer carries everything, and a metered one carries
//! incidents and check-ins. The rows differ only in what each kind *assumes* about metering when
//! nobody has said. Writing it as one rule rather than three is what makes "Wi-Fi in a hotel, marked
//! metered" behave exactly like cellular, which is what §34.3's "unless marked metered" asks for and
//! what a table read literally would not give.
//!
//! # The uncomfortable part of that rule, which is correct
//!
//! An incident is a sealed black box, *"tens of megabytes"*, and it is on the list of things allowed
//! over the most expensive link a robot has. That is not an oversight in the specification. §34.4:
//! **"There is nothing in this queue worth less than an incident."** A fleet that discovers the
//! recording of its worst day was waiting for Wi-Fi has saved a few pounds and lost the thing the
//! black box exists for. Telemetry and episodes wait; the recording does not.
//!
//! # Declared, then observed
//!
//! Which interface is the cellular modem is a fact about the machine that nothing here can infer
//! reliably — `/sys/class/net` will not tell you whether `wwan0` is a modem or a USB dongle somebody
//! left plugged in, and a robot guessing that its ethernet is metered would silently hold every
//! episode for ever. So the bearers are **declared** in `state/bearers.json`, and only their
//! liveness is observed.
//!
//! This is the same shape as §33.4's safe state, and for the same reason: the parts a machine can
//! measure are measured, the parts it cannot are declared, and every surface says which is which. A
//! robot with no `bearers.json` reports that it has no bearers rather than inventing one, and the
//! queue holds — which is exactly what it did before this module existed, with the difference that
//! it now says why.
//!
//! # What this module deliberately does not do
//!
//! It does not speak to the Hub. §34.2's outbound path is HTTPS to an endpoint this robot was paired
//! with in §10, and §10 is unwritten — there is no identity, no attestation, and no endpoint. A
//! module that shipped an HTTP client here would be a client with nowhere to connect and a TLS stack
//! this workspace has no dependency for.
//!
//! What it does instead is make the *absence* explicit. [`Unpaired`] is the default sink and it
//! refuses every item with a sentence naming §10, so `/outbound` shows a queue that is held for a
//! stated reason rather than a queue that only grows. [`Directory`] is the other sink, behind a
//! flag, and it exists so the drain path — pick, decide, send, remove, back off — is exercised by
//! something rather than by nothing.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use skynet_json::Json;

use crate::outbound::{backoff_ms, Class, Item, Queue};

/// The three bearers §34.3 names, and everything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// On a dock. Unmetered, carries everything.
    Ethernet,
    /// In a building. Metering is a property of the network, so it is configurable.
    WiFi,
    /// Outdoors. Assume metered.
    Cellular,
    /// Something else a vendor declared. Assume metered, for the reason below.
    Other,
}

/// Every kind, in preference order.
pub const KINDS: [Kind; 4] = [Kind::Ethernet, Kind::WiFi, Kind::Cellular, Kind::Other];

impl Kind {
    /// 1 to 4. Lower is preferred, exactly as §34.3's table numbers them.
    pub fn preference(self) -> u8 {
        match self {
            Kind::Ethernet => 1,
            Kind::WiFi => 2,
            Kind::Cellular => 3,
            Kind::Other => 4,
        }
    }

    /// What to assume about metering when nobody has declared it.
    ///
    /// Ethernet and Wi-Fi assume unmetered; cellular assumes metered, per the table. **`Other`
    /// assumes metered too**, and that is the safe direction: an unrecognised bearer is one nobody
    /// has characterised, and the cost of holding an episode too long is an episode that arrives
    /// late, while the cost of the opposite is an invoice.
    pub fn assume_metered(self) -> bool {
        matches!(self, Kind::Cellular | Kind::Other)
    }

    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Ethernet => "ethernet",
            Kind::WiFi => "wifi",
            Kind::Cellular => "cellular",
            Kind::Other => "other",
        }
    }

    /// Read one back. Unknown spellings become [`Kind::Other`] rather than `None`.
    ///
    /// A vendor naming a bearer this OS has not heard of should get the conservative assumptions,
    /// not a parse error that removes the interface from the table entirely — the second is how a
    /// robot with one working link comes to report that it has none.
    pub fn parse(text: &str) -> Kind {
        KINDS
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .unwrap_or(Kind::Other)
    }
}

/// One declared bearer, and whether it is up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bearer {
    /// The interface name, which is also what liveness is observed through.
    pub name: String,
    /// Which of §34.3's rows this is.
    pub kind: Kind,
    /// Metering, when somebody has said. `None` takes [`Kind::assume_metered`].
    pub declared_metered: Option<bool>,
    /// Whether the interface is up, from the kernel.
    ///
    /// `None` means **this operating system could not answer** — there is no `/sys/class/net` on a
    /// developer's laptop, and a robot's own interface can disappear from it entirely. It is
    /// deliberately not `false`: "the link is down" is a measurement and "nothing measured it" is
    /// not, and the first draft of this module collapsed them, which made every bearer on a
    /// non-Linux machine report as down. That is an invented measurement, which is the one thing
    /// §7.2 exists to prevent.
    pub observed_up: Option<bool>,
    /// Liveness somebody wrote down, for a link this OS cannot see.
    ///
    /// It exists because the alternative is worse in both directions: without it a robot whose
    /// modem is managed by a vendor daemon outside `/sys/class/net` can never send anything, and a
    /// development machine can never render the panel in any state but "everything is down". The
    /// observation **wins** where there is one — a declaration cannot talk a robot into believing a
    /// link that the kernel says is down — and every surface says which of the two it is showing.
    pub declared_up: Option<bool>,
}

impl Bearer {
    /// Whether this bearer is up.
    ///
    /// The kernel's answer where there is one, the declaration otherwise, and `false` when neither
    /// exists — which is the safe direction, because a bearer nobody can vouch for is not one to
    /// send over.
    pub fn up(&self) -> bool {
        self.observed_up.or(self.declared_up).unwrap_or(false)
    }

    /// Where [`Bearer::up`]'s answer came from: `observed`, `declared`, or `unknown`.
    pub fn liveness(&self) -> &'static str {
        if self.observed_up.is_some() {
            "observed"
        } else if self.declared_up.is_some() {
            "declared"
        } else {
            "unknown"
        }
    }

    /// Whether this bearer is metered.
    pub fn metered(&self) -> bool {
        self.declared_metered
            .unwrap_or_else(|| self.kind.assume_metered())
    }

    /// Whether the metering figure was declared or assumed, for a surface that must say which.
    pub fn metering_is_declared(&self) -> bool {
        self.declared_metered.is_some()
    }

    /// This bearer, as `/outbound` reports it.
    pub fn to_json(&self) -> Json {
        let mut row = Json::obj();
        row.set("name", Json::Str(self.name.clone()));
        row.set("kind", Json::Str(self.kind.as_str().to_string()));
        row.set("preference", Json::Num(self.kind.preference() as f64));
        row.set("up", Json::Bool(self.up()));
        // Where that came from. A panel showing "down" over a laptop that has never heard of
        // `/sys/class/net` would be reporting a measurement nobody took.
        row.set("liveness", Json::Str(self.liveness().to_string()));
        row.set("metered", Json::Bool(self.metered()));
        // Two facts, not one. "This link is metered" and "somebody measured that" are different
        // things to be told, and a console that showed only the first would present an assumption
        // as a measurement — which is the one thing §7.2 spends its whole existence on.
        row.set("metering_declared", Json::Bool(self.metering_is_declared()));
        row
    }
}

/// Every declared bearer, with liveness filled in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    /// The bearers, in preference order.
    pub bearers: Vec<Bearer>,
    /// Whether anybody declared any.
    ///
    /// Distinct from an empty list, which cannot happen through [`Table::read`] but can be
    /// constructed: "nobody has written `bearers.json`" and "the file declares no bearers" are
    /// different administrative situations and the second is somebody's decision.
    pub declared: bool,
}

impl Table {
    /// Read `state/bearers.json` and observe liveness for what it names.
    pub fn of(layout: &crate::layout::Layout) -> Table {
        Table::read(&layout.state().join("bearers.json"), &observed_up)
    }

    /// The same, with the observation supplied — which is how the suite exercises every row of
    /// §34.3 without a network interface, and how a platform with no `/sys/class/net` says so.
    pub fn read(path: &Path, up: &dyn Fn(&str) -> Option<bool>) -> Table {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Table::default();
        };
        let Ok(document) = Json::parse(&text) else {
            return Table::default();
        };
        let mut bearers = Vec::new();
        for raw in document.arr_or_empty("bearers") {
            let Some(name) = raw.get("name").and_then(Json::as_str) else {
                continue;
            };
            let kind = Kind::parse(raw.get("kind").and_then(Json::as_str).unwrap_or("other"));
            bearers.push(Bearer {
                observed_up: up(name),
                declared_up: raw.get("up").and_then(Json::as_bool),
                name: name.to_string(),
                kind,
                declared_metered: raw.get("metered").and_then(Json::as_bool),
            });
        }
        // Preference first, then name, so two bearers of one kind order the same way on every boot.
        // A send decision that depended on directory order would be a decision that changed after a
        // reboot for no reason anybody could see.
        bearers.sort_by(|a, b| {
            a.kind
                .preference()
                .cmp(&b.kind.preference())
                .then_with(|| a.name.cmp(&b.name))
        });
        Table {
            bearers,
            declared: true,
        }
    }

    /// The bearer a send would go over: the most preferred one that is up.
    pub fn active(&self) -> Option<&Bearer> {
        self.bearers.iter().find(|bearer| bearer.up())
    }

    /// Whether every bearer's liveness came from the kernel.
    ///
    /// The one thing a "nothing is up" message must not get wrong. "All three links are down" is
    /// the machine's own answer only when the machine answered.
    pub fn liveness_observed(&self) -> bool {
        self.bearers
            .iter()
            .all(|bearer| bearer.observed_up.is_some())
    }
}

/// Whether an interface is up, from the kernel.
///
/// `/sys/class/net/<name>/operstate`, which is the interface's own view rather than an inference
/// from an address or a route. A robot with an address on a link that is down is a robot that would
/// otherwise spend fifteen minutes of backoff discovering it.
/// `None` where this operating system has no answer, which is every platform that is not Linux and
/// every interface that is not in `/sys/class/net`. Not `false`: see [`Bearer::observed_up`].
fn observed_up(name: &str) -> Option<bool> {
    if name.contains('/') || name.contains("..") {
        return None;
    }
    std::fs::read_to_string(format!("/sys/class/net/{name}/operstate"))
        .ok()
        .map(|text| text.trim() == "up")
}

/// What §34.3 says about sending this class over this bearer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    /// Send it.
    ///
    /// Carries what it is about, like the other two. A verdict that said only *"yes"* forced every
    /// caller to reconstruct the question — and the console did, badly: `Link: the active bearer
    /// carries this class` is a sentence whose subject is not on the page.
    Send {
        /// Which class this is a yes for.
        class: Class,
        /// The bearer it would go over.
        bearer: String,
    },
    /// A bulk class on a metered link. §34.3: the wait is visible rather than silent.
    WaitForUnmetered {
        /// Which class is waiting.
        class: Class,
        /// The bearer it is waiting past.
        bearer: String,
        /// The sentence.
        detail: String,
    },
    /// Nothing is up.
    NoBearer {
        /// Why — and "no bearer has been declared" and "every declared bearer is down" are
        /// different sentences leading to different repairs.
        detail: String,
    },
}

impl Dispatch {
    /// Whether this verdict permits a send.
    pub fn permits(&self) -> bool {
        matches!(self, Dispatch::Send { .. })
    }

    /// A code a console can translate.
    pub fn as_str(&self) -> &'static str {
        match self {
            Dispatch::Send { .. } => "send",
            Dispatch::WaitForUnmetered { .. } => "wait_for_unmetered",
            Dispatch::NoBearer { .. } => "no_bearer",
        }
    }

    /// The sentence, whatever the verdict.
    pub fn detail(&self) -> String {
        match self {
            Dispatch::Send { class, bearer } => format!(
                "the next item is {} and `{bearer}` carries it",
                class.as_str()
            ),
            Dispatch::WaitForUnmetered { detail, .. } | Dispatch::NoBearer { detail } => {
                detail.clone()
            }
        }
    }
}

/// Whether a class may go over a bearer now (§34.3).
///
/// One rule for three rows: an unmetered bearer carries everything, a metered one carries incidents
/// and check-ins. See the module documentation for why the biggest payload is on the second list.
pub fn may_send(class: Class, table: &Table) -> Dispatch {
    let Some(bearer) = table.active() else {
        return Dispatch::NoBearer {
            detail: if !table.declared {
                "no bearer has been declared. §34.3's table is a property of the machine — which \
                 interface is the modem is not something this OS can infer — so it is read from \
                 `state/bearers.json`, and there is no such file. Nothing is being sent and nothing \
                 is being dropped for it."
                    .to_string()
            } else if table.bearers.is_empty() {
                "`state/bearers.json` declares no bearers, so there is nothing to send over. That \
                 is a decision somebody made rather than a link that is down."
                    .to_string()
            } else {
                format!(
                    "none of the {} declared bearer(s) is up. {}",
                    table.bearers.len(),
                    if table.liveness_observed() {
                        "Liveness came from the kernel for every one of them, so this is the \
                         machine's own answer."
                    } else {
                        "Liveness for at least one of them was not measured — this operating \
                         system has no `/sys/class/net` entry for it — so this is what was written \
                         down rather than what was observed."
                    }
                )
            },
        };
    };

    // Incidents and check-ins go over anything that is up. Everything else waits for a link that
    // does not charge by the megabyte.
    if !bearer.metered() || matches!(class, Class::Incident | Class::CheckIn) {
        return Dispatch::Send {
            class,
            bearer: bearer.name.clone(),
        };
    }

    Dispatch::WaitForUnmetered {
        class,
        bearer: bearer.name.clone(),
        detail: format!(
            "{} is a bulk class and the active bearer `{}` is {} metered, so it waits for an \
             unmetered link (§34.3). Nothing is dropped for waiting; the queue's budget still \
             applies.",
            class.as_str(),
            bearer.name,
            if bearer.metering_is_declared() {
                "declared"
            } else {
                // The assumption, named as one. A vendor who thinks their Wi-Fi is free needs to
                // know the robot does not, and needs to know it is one line of configuration away.
                "assumed"
            }
        ),
    }
}

/// Why one item could not be delivered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError {
    /// A code.
    pub code: &'static str,
    /// The sentence.
    pub detail: String,
}

/// Somewhere an item can go.
///
/// A trait rather than a function so a robot, a test and a development machine can each hold a
/// different one, and so that the drain path below is written once for all three.
pub trait Sink {
    /// Deliver one item. `Ok(())` means it is gone and the queue may forget it.
    fn send(&self, item: &Item) -> Result<(), SendError>;

    /// What this sink is, for `/outbound` and the console. Shown to an operator, so it says what is
    /// actually happening rather than naming a type.
    fn describe(&self) -> String;

    /// Why sending cannot work at all, when that is knowable without trying.
    ///
    /// Added because the console asked a question this trait could not answer, and answered it
    /// wrongly as a result: `/outbound` reported §34.3's verdict — *"the active bearer carries this
    /// class"* — beside a sink with nowhere to send, and the two rendered as one sentence saying
    /// the queue was fine. Both facts were true and the pair of them was a lie.
    ///
    /// `None` means nothing here stands in the way, which is not a promise that a send will
    /// succeed — a network can still refuse. It is only the difference between *unreachable* and
    /// *not configured*, and an operator needs to know which of those they are looking at.
    fn obstacle(&self) -> Option<String> {
        None
    }
}

/// The default: there is nowhere to send.
///
/// §34.2's outbound path is HTTPS to an endpoint this robot was paired with in §10, and §10 is
/// unwritten. This sink refuses everything with that sentence, which is the difference between a
/// queue that is held for a stated reason and a queue that only grows.
#[derive(Debug, Clone, Copy, Default)]
pub struct Unpaired;

impl Sink for Unpaired {
    fn send(&self, _item: &Item) -> Result<(), SendError> {
        Err(SendError {
            code: "unpaired",
            detail: "this robot has no Hub to send to. §10's pairing and attestation are unwritten, \
                     so there is no endpoint and no identity to present at one. The queue holds and \
                     §34.4's drop policy applies to it."
                .to_string(),
        })
    }

    fn describe(&self) -> String {
        "none — this robot is not paired with a Hub (§10)".to_string()
    }

    fn obstacle(&self) -> Option<String> {
        Some(
            "nothing will be sent whatever the link is doing: this robot has no Hub to send to. \
             §10's pairing and attestation are unwritten. Items are held under §34.4's drop policy \
             and the counters on this panel are what that costs."
                .to_string(),
        )
    }
}

/// A directory on this machine.
///
/// Not a transport, and it is not presented as one anywhere. It exists so the drain path — pick,
/// decide, send, remove, back off — is exercised by something rather than by nothing, and so that
/// somebody bringing up a robot can watch the queue actually empty. Behind `--outbox <dir>`.
#[derive(Debug, Clone)]
pub struct Directory {
    /// Where items are written.
    pub path: PathBuf,
}

impl Sink for Directory {
    fn send(&self, item: &Item) -> Result<(), SendError> {
        std::fs::create_dir_all(&self.path).map_err(|error| SendError {
            code: "io",
            detail: format!("{error}"),
        })?;
        crate::observe::write_atomic(&self.path.join(&item.id), item.body.as_bytes()).map_err(
            |error| SendError {
                code: "io",
                detail: format!("{error}"),
            },
        )
    }

    fn describe(&self) -> String {
        format!(
            "a directory on this machine, {} — a development sink, not the Hub",
            self.path.display()
        )
    }
}

/// What one drain did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Drained {
    /// Items delivered and removed.
    pub sent: usize,
    /// Items attempted and refused, now in backoff.
    pub failed: usize,
    /// Items skipped because they are still in backoff.
    pub backing_off: usize,
    /// Why the drain stopped, when it stopped for a reason other than an empty queue.
    pub blocked: Option<Dispatch>,
}

/// Drains the queue, one pass at a time.
///
/// Holds the retry schedule **in memory** rather than on disk, and that is deliberate. A backoff
/// deadline is a monotonic instant (§35.1), and a monotonic instant written to disk means nothing
/// after a reboot — it would have to be stored as wall time, which is the clock §35.1 forbids
/// control decisions from reading. So a restart clears the schedule and every item is eligible
/// again, which is also the right behaviour: a reboot is a change in the world, and a robot that
/// came back up should try before waiting out a fifteen-minute cap it inherited from before.
///
/// The attempt *count* does survive, because it is in the item on disk. So the backoff a restarted
/// robot computes is the one its history earns — it just does not wait out the remainder of the
/// interval it was in the middle of.
pub struct Sender {
    queue: Queue,
    /// Item id to the earliest monotonic instant it may be tried again.
    schedule: BTreeMap<String, u64>,
}

impl Sender {
    /// A sender over a queue.
    pub fn new(queue: Queue) -> Sender {
        Sender {
            queue,
            schedule: BTreeMap::new(),
        }
    }

    /// The queue underneath.
    pub fn queue(&self) -> &Queue {
        &self.queue
    }

    /// Attempt up to `limit` items.
    ///
    /// `jitter` is supplied rather than drawn, for §35.2's reason and §34.4's: a function that
    /// reaches for a random number cannot be tested, and an untested backoff is how a fleet
    /// discovers the cap was off by a factor of sixty.
    ///
    /// The drain stops at the first item §34.3 will not carry, and does **not** skip past it to a
    /// lower-priority one it could. The queue is in priority order and sending a telemetry item
    /// past a waiting episode would quietly invert §34.4's table — the wait is meant to be visible,
    /// not routed around.
    pub fn drain(
        &mut self,
        table: &Table,
        sink: &dyn Sink,
        now_ms: u64,
        jitter: f64,
        limit: usize,
    ) -> Drained {
        let mut drained = Drained::default();
        for item in self.queue.items().into_iter().take(limit) {
            if let Some(not_before) = self.schedule.get(&item.id) {
                if now_ms < *not_before {
                    drained.backing_off += 1;
                    continue;
                }
            }

            let verdict = may_send(item.class, table);
            if !verdict.permits() {
                drained.blocked = Some(verdict);
                break;
            }

            match sink.send(&item) {
                Ok(()) => {
                    if self.queue.remove(&item.id).is_ok() {
                        self.schedule.remove(&item.id);
                        drained.sent += 1;
                    }
                }
                Err(_) => {
                    let attempts = self.queue.attempted(&item.id).unwrap_or(item.attempts + 1);
                    self.schedule.insert(
                        item.id.clone(),
                        now_ms.saturating_add(backoff_ms(attempts, jitter)),
                    );
                    drained.failed += 1;
                }
            }
        }
        drained
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TempRoot;

    fn total_dropped(status: &crate::outbound::Status) -> u64 {
        status.dropped.iter().map(|(_, count)| *count).sum()
    }

    fn table(rows: &[(&str, Kind, Option<bool>, bool)]) -> Table {
        let mut bearers: Vec<Bearer> = rows
            .iter()
            .map(|(name, kind, metered, up)| Bearer {
                name: name.to_string(),
                kind: *kind,
                declared_metered: *metered,
                observed_up: Some(*up),
                declared_up: None,
            })
            .collect();
        bearers.sort_by(|a, b| {
            a.kind
                .preference()
                .cmp(&b.kind.preference())
                .then_with(|| a.name.cmp(&b.name))
        });
        Table {
            bearers,
            declared: true,
        }
    }

    #[test]
    fn the_most_preferred_bearer_that_is_up_wins() {
        let t = table(&[
            ("eth0", Kind::Ethernet, None, false),
            ("wlan0", Kind::WiFi, None, true),
            ("wwan0", Kind::Cellular, None, true),
        ]);
        assert_eq!(t.active().map(|b| b.name.as_str()), Some("wlan0"));
    }

    #[test]
    fn an_unmetered_bearer_carries_every_class() {
        let t = table(&[("eth0", Kind::Ethernet, None, true)]);
        for class in crate::outbound::CLASSES {
            assert!(may_send(class, &t).permits(), "{class:?}");
        }
    }

    #[test]
    fn a_yes_says_what_it_is_a_yes_about() {
        // A verdict that said only "yes" made every caller reconstruct the question, and the
        // console reconstructed it wrongly: `the active bearer carries this class` is a sentence
        // whose subject is not on the page.
        let verdict = may_send(
            Class::Incident,
            &table(&[("wwan0", Kind::Cellular, None, true)]),
        );
        assert_eq!(verdict.as_str(), "send");
        assert!(verdict.detail().contains("incident"));
        assert!(verdict.detail().contains("wwan0"));
    }

    #[test]
    fn a_metered_bearer_carries_incidents_and_check_ins_and_nothing_else() {
        // The uncomfortable half of §34.3, and it is correct: the sealed black box is tens of
        // megabytes and it is on the list, because there is nothing in this queue worth less than
        // an incident.
        let t = table(&[("wwan0", Kind::Cellular, None, true)]);
        assert!(may_send(Class::Incident, &t).permits());
        assert!(may_send(Class::CheckIn, &t).permits());
        assert!(!may_send(Class::Episode, &t).permits());
        assert!(!may_send(Class::Telemetry, &t).permits());
    }

    #[test]
    fn wifi_marked_metered_behaves_exactly_like_cellular() {
        // §34.3's "everything unless marked metered", which a table read row by row would not give.
        let t = table(&[("wlan0", Kind::WiFi, Some(true), true)]);
        assert!(!may_send(Class::Telemetry, &t).permits());
        assert!(may_send(Class::Incident, &t).permits());
    }

    #[test]
    fn cellular_declared_unmetered_carries_everything() {
        // The other direction, which matters for a robot on a private APN somebody already paid
        // for: the assumption is conservative and it is only an assumption.
        let t = table(&[("wwan0", Kind::Cellular, Some(false), true)]);
        assert!(may_send(Class::Telemetry, &t).permits());
    }

    #[test]
    fn the_wait_says_whether_the_metering_was_declared_or_assumed() {
        let assumed = may_send(
            Class::Telemetry,
            &table(&[("wwan0", Kind::Cellular, None, true)]),
        );
        let declared = may_send(
            Class::Telemetry,
            &table(&[("wlan0", Kind::WiFi, Some(true), true)]),
        );
        assert!(assumed.detail().contains("assumed metered"));
        assert!(declared.detail().contains("declared metered"));
    }

    #[test]
    fn three_reasons_for_no_bearer_and_they_are_three_sentences() {
        let undeclared = may_send(Class::CheckIn, &Table::default());
        let none_declared = may_send(
            Class::CheckIn,
            &Table {
                bearers: Vec::new(),
                declared: true,
            },
        );
        let all_down = may_send(
            Class::CheckIn,
            &table(&[("eth0", Kind::Ethernet, None, false)]),
        );
        assert!(undeclared.detail().contains("no such file"));
        assert!(none_declared.detail().contains("declares no bearers"));
        assert!(all_down.detail().contains("is up"));
        for verdict in [undeclared, none_declared, all_down] {
            assert_eq!(verdict.as_str(), "no_bearer");
        }
    }

    #[test]
    fn an_unrecognised_kind_is_other_and_assumed_metered() {
        // Not a parse failure. A bearer this OS has not heard of must not vanish from the table —
        // that is how a robot with one working link comes to report that it has none.
        assert_eq!(Kind::parse("satellite"), Kind::Other);
        let t = table(&[("sat0", Kind::Other, None, true)]);
        assert!(may_send(Class::Incident, &t).permits());
        assert!(!may_send(Class::Episode, &t).permits());
    }

    #[test]
    fn a_declared_table_is_read_in_preference_order() {
        let root = TempRoot::new("bearers-read");
        let path = root.path().join("bearers.json");
        std::fs::write(
            &path,
            r#"{"bearers":[{"name":"wwan0","kind":"cellular"},
                           {"name":"eth0","kind":"ethernet"},
                           {"name":"wlan0","kind":"wifi","metered":true}]}"#,
        )
        .unwrap();
        let table = Table::read(&path, &|name| Some(name != "eth0"));
        assert!(table.declared);
        let names: Vec<&str> = table.bearers.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["eth0", "wlan0", "wwan0"]);
        // Ethernet is declared and down, so the metered Wi-Fi is what a send would go over.
        assert_eq!(table.active().map(|b| b.name.as_str()), Some("wlan0"));
        assert!(table.active().unwrap().metered());
    }

    #[test]
    fn an_unmeasurable_link_is_unknown_rather_than_down() {
        // The bug the console fixture found on the first look. A platform with no `/sys/class/net`
        // cannot say whether an interface is up, and the first draft returned `false` for it — so
        // every bearer on a developer's machine reported as *down*, which is a measurement nobody
        // took presented as one somebody did.
        let root = TempRoot::new("bearers-unknown");
        let path = root.path().join("bearers.json");
        std::fs::write(&path, r#"{"bearers":[{"name":"eth0","kind":"ethernet"}]}"#).unwrap();
        let table = Table::read(&path, &|_| None);
        assert_eq!(table.bearers[0].liveness(), "unknown");
        assert!(!table.bearers[0].up());
        assert!(!table.liveness_observed());
        // And the sentence says so, rather than claiming the machine answered.
        assert!(may_send(Class::CheckIn, &table)
            .detail()
            .contains("not measured"));
    }

    #[test]
    fn an_observation_beats_a_declaration() {
        // A declaration cannot talk a robot into believing a link the kernel says is down. That
        // direction is the whole reason the override is safe to have at all.
        let root = TempRoot::new("bearers-override");
        let path = root.path().join("bearers.json");
        std::fs::write(
            &path,
            r#"{"bearers":[{"name":"eth0","kind":"ethernet","up":true}]}"#,
        )
        .unwrap();
        assert!(!Table::read(&path, &|_| Some(false)).bearers[0].up());
        assert_eq!(
            Table::read(&path, &|_| Some(false)).bearers[0].liveness(),
            "observed"
        );
        // And where the kernel has nothing to say, the declaration is used and labelled.
        let table = Table::read(&path, &|_| None);
        assert!(table.bearers[0].up());
        assert_eq!(table.bearers[0].liveness(), "declared");
    }

    #[test]
    fn a_missing_file_is_undeclared_rather_than_empty() {
        let table = Table::read(Path::new("does/not/exist.json"), &|_| Some(true));
        assert!(!table.declared);
        assert!(table.bearers.is_empty());
    }

    #[test]
    fn the_drain_sends_over_an_unmetered_link_and_empties_the_queue() {
        let root = TempRoot::new("drain-sends");
        let dir = root.path();
        let queue = Queue::at(dir.join("q"), 1024 * 1024, 256 * 1024);
        queue.enqueue(Class::Telemetry, "one").unwrap();
        queue.enqueue(Class::Telemetry, "two").unwrap();

        let outbox = Directory {
            path: dir.join("out"),
        };
        let mut sender = Sender::new(queue);
        let drained = sender.drain(
            &table(&[("eth0", Kind::Ethernet, None, true)]),
            &outbox,
            0,
            0.0,
            16,
        );
        assert_eq!(drained.sent, 2);
        assert_eq!(drained.failed, 0);
        assert!(sender.queue().items().is_empty());
        assert_eq!(std::fs::read_dir(dir.join("out")).unwrap().count(), 2);
    }

    #[test]
    fn a_bulk_item_on_a_metered_link_stops_the_drain_rather_than_being_routed_around() {
        // The queue is in priority order and the wait is meant to be visible. A drain that skipped
        // the episode and sent the telemetry behind it would quietly invert §34.4's table.
        let root = TempRoot::new("drain-blocked");
        let dir = root.path();
        let queue = Queue::at(dir.join("q"), 1024 * 1024, 256 * 1024);
        queue.enqueue(Class::Episode, "an episode").unwrap();
        queue.enqueue(Class::Telemetry, "some metrics").unwrap();

        let mut sender = Sender::new(queue);
        let drained = sender.drain(
            &table(&[("wwan0", Kind::Cellular, None, true)]),
            &Directory {
                path: dir.join("out"),
            },
            0,
            0.0,
            16,
        );
        assert_eq!(drained.sent, 0);
        assert_eq!(sender.queue().items().len(), 2);
        assert_eq!(
            drained.blocked.as_ref().map(Dispatch::as_str),
            Some("wait_for_unmetered")
        );

        // The incident in front of them both goes, because §34.3 puts it on the list.
        sender.queue().enqueue(Class::Incident, "a fault").unwrap();
        let drained = sender.drain(
            &table(&[("wwan0", Kind::Cellular, None, true)]),
            &Directory {
                path: dir.join("out"),
            },
            0,
            0.0,
            16,
        );
        assert_eq!(drained.sent, 1);
        assert_eq!(sender.queue().items().len(), 2);
    }

    #[test]
    fn the_unpaired_sink_refuses_and_the_item_backs_off() {
        let root = TempRoot::new("drain-unpaired");
        let dir = root.path();
        let queue = Queue::at(dir.join("q"), 1024 * 1024, 256 * 1024);
        queue.enqueue(Class::CheckIn, "hello").unwrap();

        let mut sender = Sender::new(queue);
        let bearers = table(&[("eth0", Kind::Ethernet, None, true)]);
        // Jitter at the top of the window, which is the only value that makes a delay observable:
        // §34.4's backoff is **full** jitter, uniform over `[0, exponential]`, so a jitter of zero
        // is a legitimate instruction to retry immediately rather than a way of saying "no jitter".
        let first = sender.drain(&bearers, &Unpaired, 0, 1.0, 16);
        assert_eq!(first.failed, 1);
        assert_eq!(sender.queue().items()[0].attempts, 1);

        // Immediately again: still in backoff, so it is not attempted a second time.
        let second = sender.drain(&bearers, &Unpaired, 1, 1.0, 16);
        assert_eq!(second.failed, 0);
        assert_eq!(second.backing_off, 1);
        assert_eq!(sender.queue().items()[0].attempts, 1);

        // And past the backoff it is attempted again, so a refusal is a delay and not a discard.
        let later = sender.drain(&bearers, &Unpaired, 10_000_000, 1.0, 16);
        assert_eq!(later.failed, 1);
        assert_eq!(sender.queue().items()[0].attempts, 2);
    }

    #[test]
    fn nothing_is_ever_dropped_by_a_failed_send() {
        // §34.4 gives exactly one reason to discard an item — making room — and a delivery failure
        // is not it. A sender that tidied up after itself would be a drop policy nobody wrote.
        let root = TempRoot::new("drain-keeps");
        let dir = root.path();
        let queue = Queue::at(dir.join("q"), 1024 * 1024, 256 * 1024);
        queue.enqueue(Class::Incident, "the recording").unwrap();
        let mut sender = Sender::new(queue);
        for tick in 0..5 {
            sender.drain(
                &table(&[("eth0", Kind::Ethernet, None, true)]),
                &Unpaired,
                tick * 10_000_000,
                1.0,
                16,
            );
        }
        assert_eq!(sender.queue().items().len(), 1);
        assert_eq!(total_dropped(&sender.queue().status()), 0);
    }
}

//! The outbound queue (§34.4).
//!
//! §34.2 makes every connection the robot's own — nothing dials in — so everything the robot has to
//! say to the Hub goes through one bounded, on-disk queue that survives a reboot. Bounded is the
//! interesting word: a robot offline for three weeks fills any queue, so **something must be
//! discarded, and deciding what after the disk is full means deciding it badly.**
//!
//! # The drop policy is the design
//!
//! | Class | Priority | Dropped |
//! |---|---|---|
//! | [`Class::Incident`] | 1 | **Never.** Disk is reserved for them at install |
//! | [`Class::CheckIn`] | 2 | Superseded — only the newest matters, so older ones collapse |
//! | [`Class::Episode`] | 3 | Oldest first, above the quota |
//! | [`Class::Telemetry`] | 4 | Oldest first, aggressively |
//!
//! Those four rows are four different policies, not one policy with four thresholds, and the
//! difference is what makes the queue worth having. Telemetry is a stream where the newest is worth
//! most and any single item is worth nothing. A check-in is *level-triggered state*: a week offline
//! should produce **one** current check-in on reconnect, not ten thousand historical ones, so
//! enqueueing one removes the last. An episode is data somebody collected and paid for, so it goes
//! oldest-first and only above a quota. An incident is the whole reason the black box exists.
//!
//! # A drop is recorded, always
//!
//! §47.4's rule, applied to a queue rather than to a ring: *a gap annotated as a gap is honest; a
//! gap that looks like a quiet period is not.* Every discard increments a counter that survives with
//! the queue and is reported by [`Queue::status`], so a fleet console can say "this robot dropped
//! 40,000 telemetry items last month" instead of showing a suspiciously tidy series.
//!
//! Collapsing a check-in is **not** a drop and is counted separately. Nothing was lost: the newer
//! item carries the same level-triggered state, which is the whole reason the older one can go.
//!
//! # The reserve, and the one failure it cannot cover
//!
//! Incidents are never dropped, which is only true if there is somewhere to put them. So the budget
//! is split at construction: everything else shares `budget - reserve`, and the reserve is
//! untouchable. A robot that has filled its reserve as well is a robot with a genuinely full disk,
//! and that is reported as an error rather than resolved by dropping the incident — the one thing
//! this queue exists to keep.
//!
//! # Backoff is supplied jitter, not a random number generator
//!
//! §34.4 asks for exponential backoff with jitter, capped at fifteen minutes, and the jitter is the
//! part that matters at fleet scale: *a hundred robots that lost the same access point must not
//! retry in lockstep and take down the endpoint the moment it returns.* [`backoff_ms`] takes the
//! jitter as a fraction, for the reason the kernel takes its clock as a parameter (§35.2) — a
//! function that reaches for a random number cannot be tested, and an untested backoff is how a
//! fleet discovers the cap was off by a factor of sixty.

use std::path::{Path, PathBuf};

use skynet_json::Json;

/// What an outbound item is, in §34.4's priority order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// A sealed black box on a fault. Never dropped.
    Incident,
    /// The robot's current state. Level-triggered: only the newest matters.
    CheckIn,
    /// Collected data. Oldest first, above the quota.
    Episode,
    /// Metrics. Oldest first, aggressively.
    Telemetry,
}

/// Every class, in priority order.
pub const CLASSES: [Class; 4] = [
    Class::Incident,
    Class::CheckIn,
    Class::Episode,
    Class::Telemetry,
];

impl Class {
    /// 1 to 4, as §34.4's table numbers them. Also the sort key, and the filename prefix.
    pub fn priority(self) -> u8 {
        match self {
            Class::Incident => 1,
            Class::CheckIn => 2,
            Class::Episode => 3,
            Class::Telemetry => 4,
        }
    }

    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Incident => "incident",
            Class::CheckIn => "check_in",
            Class::Episode => "episode",
            Class::Telemetry => "telemetry",
        }
    }

    /// Read one back.
    pub fn parse(text: &str) -> Option<Class> {
        CLASSES.into_iter().find(|class| class.as_str() == text)
    }

    /// Whether this class may ever be discarded to make room.
    pub fn droppable(self) -> bool {
        self != Class::Incident
    }
}

/// One queued item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Its file name, which is also its handle for [`Queue::remove`].
    pub id: String,
    /// Which class.
    pub class: Class,
    /// Position in the queue, monotonic across a reboot because it is in the name.
    pub sequence: u64,
    /// The body, as it will be sent.
    pub body: String,
    /// How many delivery attempts have been made.
    pub attempts: u32,
}

/// What a queue holds and what it has thrown away.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// Items held, per class.
    pub held: Vec<(Class, usize)>,
    /// Bytes held, per class.
    pub bytes: Vec<(Class, usize)>,
    /// Items discarded to make room, per class, for the life of the queue.
    pub dropped: Vec<(Class, u64)>,
    /// Check-ins superseded by a newer one. Not a drop: nothing was lost.
    pub collapsed: u64,
    /// The total byte budget.
    pub budget: usize,
    /// Of which this much is reserved for incidents and unavailable to anything else.
    pub reserve: usize,
}

/// Why an item could not be queued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueError {
    /// The item is larger than everything this queue could ever hold.
    TooLarge {
        /// Its size.
        size: usize,
        /// What was available to it.
        available: usize,
    },
    /// An incident did not fit even in its reserve. A genuinely full disk.
    ReserveFull {
        /// Its size.
        size: usize,
        /// What the reserve had left.
        free: usize,
    },
    /// The filesystem refused.
    Io {
        /// What happened.
        detail: String,
    },
}

impl QueueError {
    /// A sentence for an operator.
    pub fn reason(&self) -> String {
        match self {
            QueueError::TooLarge { size, available } => format!(
                "this item is {size} bytes and the queue's whole budget for its class is \
                 {available}. Dropping every other item would still not make room, so nothing was \
                 dropped"
            ),
            QueueError::ReserveFull { size, free } => format!(
                "an incident of {size} bytes did not fit in the {free} bytes left of the reserve. \
                 §34.4 reserves disk for incidents at install precisely so this cannot happen, so \
                 this is a full disk rather than a full queue — and nothing was discarded to make \
                 room, because there is nothing here worth less than an incident"
            ),
            QueueError::Io { detail } => detail.clone(),
        }
    }
}

/// The queue at `<root>/data/state/outbound/`.
///
/// One file per item, named `<priority>-<sequence:020>-<class>`, so the directory listing sorted
/// lexicographically **is** the send order: priority first, then age. No index file, and that is
/// deliberate — an index is a second record of what the directory already says, and the two
/// disagree exactly once, after the crash that matters.
#[derive(Debug, Clone)]
pub struct Queue {
    directory: PathBuf,
    budget: usize,
    reserve: usize,
}

/// The default budget: 64 MiB, of which 16 MiB is reserved for incidents.
///
/// A sealed black box is *"tens of megabytes"* (§34.3), so the reserve holds one comfortably and a
/// second at a squeeze. Larger would be guessing at a disk this code has not seen; smaller would
/// mean the one thing that is never dropped is the one thing that does not fit.
pub const DEFAULT_BUDGET: usize = 64 * 1024 * 1024;
/// See [`DEFAULT_BUDGET`].
pub const DEFAULT_RESERVE: usize = 16 * 1024 * 1024;

impl Queue {
    /// Open the queue for a layout.
    ///
    /// The budget comes **off the disk** when the queue has one, and the defaults only apply to a
    /// queue nobody has sized yet. §34.4 says disk is reserved for incidents *at install*, and this
    /// is that sentence taken literally: the reserve is a property of the queue on the disk, not of
    /// whichever process happened to open it. Two callers that disagreed about the budget would
    /// enforce two different drop policies over one directory, and the console would report a limit
    /// nothing was applying.
    pub fn of(layout: &crate::layout::Layout) -> Queue {
        Queue::at(layout.outbound(), DEFAULT_BUDGET, DEFAULT_RESERVE).sized_from_disk()
    }

    /// Open a queue with an explicit budget, sizing it if it has never been sized.
    ///
    /// The reserve is clamped to the budget rather than rejected: a misconfiguration that made the
    /// reserve larger than the disk should not be a robot that cannot queue an incident.
    pub fn at(directory: impl Into<PathBuf>, budget: usize, reserve: usize) -> Queue {
        let queue = Queue {
            directory: directory.into(),
            budget,
            reserve: reserve.min(budget),
        };
        queue.record_size();
        queue
    }

    /// Adopt the budget the queue was created with, if it has one.
    fn sized_from_disk(mut self) -> Queue {
        let counters = self.counters();
        if let Some(budget) = counters.get("budget").and_then(Json::as_usize) {
            self.budget = budget;
            self.reserve = counters
                .get("reserve")
                .and_then(Json::as_usize)
                .unwrap_or(self.reserve)
                .min(budget);
        }
        self
    }

    /// Write the budget down the first time, and leave it alone afterwards.
    ///
    /// Re-sizing an existing queue is not offered here on purpose: it changes which items would
    /// have been dropped, retrospectively, and the honest way to do it is a deliberate operation
    /// that says what it is discarding rather than a side effect of opening the directory.
    fn record_size(&self) {
        let counters = self.counters();
        if counters.get("budget").is_some() {
            return;
        }
        let mut updated = counters;
        updated.set("budget", Json::Num(self.budget as f64));
        updated.set("reserve", Json::Num(self.reserve as f64));
        self.write_counters(&updated);
    }

    /// Where it lives.
    pub fn path(&self) -> &Path {
        &self.directory
    }

    /// Every item, in send order.
    pub fn items(&self) -> Vec<Item> {
        let Ok(entries) = std::fs::read_dir(&self.directory) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort();

        names.iter().filter_map(|name| self.read(name)).collect()
    }

    /// The next item to send, or `None` for an empty queue.
    pub fn next(&self) -> Option<Item> {
        self.items().into_iter().next()
    }

    fn read(&self, name: &str) -> Option<Item> {
        let (class, sequence) = parse_name(name)?;
        let text = std::fs::read_to_string(self.directory.join(name)).ok()?;
        let document = Json::parse(&text).ok()?;
        Some(Item {
            id: name.to_string(),
            class,
            sequence,
            body: document.get("body").and_then(Json::as_str)?.to_string(),
            attempts: document
                .get("attempts")
                .and_then(Json::as_usize)
                .unwrap_or(0) as u32,
        })
    }

    /// Add an item, applying §34.4's drop policy to make room.
    ///
    /// Returns what had to be discarded to fit it, which is never nothing quietly: a caller that
    /// wants to record the loss (and one should) is handed the count rather than having to diff the
    /// status before and after.
    pub fn enqueue(&self, class: Class, body: &str) -> Result<Enqueued, QueueError> {
        std::fs::create_dir_all(&self.directory).map_err(|error| QueueError::Io {
            detail: format!("{error}"),
        })?;

        let size = body.len();
        let mut collapsed = 0;
        let mut dropped = 0;

        // A check-in supersedes the last one. Done before the budget check, because the room the
        // old one frees is often exactly the room the new one needs.
        if class == Class::CheckIn {
            for item in self
                .items()
                .into_iter()
                .filter(|i| i.class == Class::CheckIn)
            {
                if self.remove(&item.id).is_ok() {
                    collapsed += 1;
                }
            }
        }

        // Incidents draw on the whole budget; everything else stops at the reserve line.
        let available = if class == Class::Incident {
            self.budget
        } else {
            self.budget.saturating_sub(self.reserve)
        };
        if size > available {
            return Err(QueueError::TooLarge { size, available });
        }

        // Make room, lowest priority first and oldest first within a class. An item is only ever
        // discarded for something at least as important as itself: filling a disk with telemetry
        // must not be able to evict an episode, and nothing evicts an incident.
        while self.used(class) + size > available {
            let Some(victim) = self.victim(class) else {
                return Err(if class == Class::Incident {
                    QueueError::ReserveFull {
                        size,
                        free: available.saturating_sub(self.used(class)),
                    }
                } else {
                    QueueError::TooLarge { size, available }
                });
            };
            if self.remove(&victim.id).is_err() {
                break;
            }
            dropped += 1;
        }

        let sequence = self.next_sequence();
        let name = format!("{}-{sequence:020}-{}", class.priority(), class.as_str());
        let mut document = Json::obj();
        document.set("class", Json::Str(class.as_str().to_string()));
        document.set("body", Json::Str(body.to_string()));
        document.set("attempts", Json::Num(0.0));
        let text = document.to_canonical().map_err(|error| QueueError::Io {
            detail: format!("{error:?}"),
        })?;
        crate::observe::write_atomic(&self.directory.join(&name), text.as_bytes()).map_err(
            |error| QueueError::Io {
                detail: format!("{error}"),
            },
        )?;

        self.bump_counters(class, dropped, collapsed);
        Ok(Enqueued {
            id: name,
            sequence,
            dropped,
            collapsed,
        })
    }

    /// Bytes an enqueue of `class` counts against.
    ///
    /// An incident counts everything, because it draws on the whole budget. Everything else counts
    /// only what shares its pool, so a large incident cannot make a small telemetry item unqueueable
    /// — the reserve is a floor for incidents, not a ceiling for everybody else.
    fn used(&self, class: Class) -> usize {
        self.items()
            .iter()
            .filter(|item| class == Class::Incident || item.class != Class::Incident)
            .map(|item| item.body.len())
            .sum()
    }

    /// The next item to discard, given something of `class` wants room.
    ///
    /// Lowest priority first, oldest first within a class, and **never something more important than
    /// the item asking**. Returning `None` means everything left outranks the newcomer, which is a
    /// refusal rather than a licence to evict upwards.
    fn victim(&self, class: Class) -> Option<Item> {
        self.items()
            .into_iter()
            .filter(|item| item.class.droppable() && item.class >= class)
            .max_by_key(|item| (item.class.priority(), std::cmp::Reverse(item.sequence)))
    }

    /// Take an item out, after it has been delivered or dropped.
    pub fn remove(&self, id: &str) -> std::io::Result<()> {
        if id.contains('/') || id.contains("..") {
            return Err(std::io::Error::other("an item id is a file name"));
        }
        match std::fs::remove_file(self.directory.join(id)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// Record a failed delivery, so backoff can be computed from it.
    pub fn attempted(&self, id: &str) -> std::io::Result<u32> {
        let Some(item) = self.read(id) else {
            return Err(std::io::Error::other("no such item"));
        };
        let attempts = item.attempts.saturating_add(1);
        let mut document = Json::obj();
        document.set("class", Json::Str(item.class.as_str().to_string()));
        document.set("body", Json::Str(item.body));
        document.set("attempts", Json::Num(attempts as f64));
        let text = document
            .to_canonical()
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        crate::observe::write_atomic(&self.directory.join(id), text.as_bytes())?;
        Ok(attempts)
    }

    fn next_sequence(&self) -> u64 {
        // From the *counters* file rather than from the highest name in the directory: a queue that
        // drains completely would otherwise restart its numbering, and two items a month apart
        // sharing a sequence makes the send order ambiguous the next time both are present.
        let counters = self.counters();
        let next = counters
            .get("sequence")
            .and_then(Json::as_usize)
            .unwrap_or(0) as u64
            + 1;
        let mut updated = counters;
        updated.set("sequence", Json::Num(next as f64));
        self.write_counters(&updated);
        next
    }

    fn counters_path(&self) -> PathBuf {
        self.directory.join(".counters.json")
    }

    fn counters(&self) -> Json {
        std::fs::read_to_string(self.counters_path())
            .ok()
            .and_then(|text| Json::parse(&text).ok())
            .filter(|value| value.as_obj().is_some())
            .unwrap_or_else(Json::obj)
    }

    fn write_counters(&self, document: &Json) {
        if let Ok(text) = document.to_canonical() {
            let _ = std::fs::create_dir_all(&self.directory);
            let _ = crate::observe::write_atomic(&self.counters_path(), text.as_bytes());
        }
    }

    fn bump_counters(&self, class: Class, dropped: u64, collapsed: u64) {
        if dropped == 0 && collapsed == 0 {
            return;
        }
        let mut counters = self.counters();
        if dropped > 0 {
            let key = format!("dropped_{}", class.as_str());
            let was = counters.get(&key).and_then(Json::as_usize).unwrap_or(0) as u64;
            counters.set(&key, Json::Num((was + dropped) as f64));
        }
        if collapsed > 0 {
            let was = counters
                .get("collapsed")
                .and_then(Json::as_usize)
                .unwrap_or(0) as u64;
            counters.set("collapsed", Json::Num((was + collapsed) as f64));
        }
        self.write_counters(&counters);
    }

    /// What this queue holds and what it has thrown away.
    pub fn status(&self) -> Status {
        let items = self.items();
        let counters = self.counters();
        let mut status = Status {
            budget: self.budget,
            reserve: self.reserve,
            collapsed: counters
                .get("collapsed")
                .and_then(Json::as_usize)
                .unwrap_or(0) as u64,
            ..Status::default()
        };
        for class in CLASSES {
            let held: Vec<&Item> = items.iter().filter(|item| item.class == class).collect();
            status.held.push((class, held.len()));
            status
                .bytes
                .push((class, held.iter().map(|item| item.body.len()).sum()));
            let dropped = counters
                .get(&format!("dropped_{}", class.as_str()))
                .and_then(Json::as_usize)
                .unwrap_or(0) as u64;
            status.dropped.push((class, dropped));
        }
        status
    }

    /// The status, for `GET /outbound` and the console.
    pub fn to_json(&self) -> Json {
        let status = self.status();
        let mut classes = Json::arr();
        for (index, class) in CLASSES.into_iter().enumerate() {
            let mut row = Json::obj();
            row.set("class", Json::Str(class.as_str().to_string()));
            row.set("priority", Json::Num(class.priority() as f64));
            row.set("held", Json::Num(status.held[index].1 as f64));
            row.set("bytes", Json::Num(status.bytes[index].1 as f64));
            row.set("dropped", Json::Num(status.dropped[index].1 as f64));
            row.set("droppable", Json::Bool(class.droppable()));
            classes.push(row);
        }

        let mut out = Json::obj();
        out.set("classes", classes);
        out.set("collapsed", Json::Num(status.collapsed as f64));
        out.set("budget", Json::Num(status.budget as f64));
        out.set("reserve", Json::Num(status.reserve as f64));
        // §34.2: nothing here has ever been sent, because there is no sender. Said in the payload
        // rather than left to a reader to notice a queue that only grows.
        out.set(
            "delivery",
            Json::Str(
                "nothing sends from this queue yet — §34.2's outbound path and §10's pairing are \
                 unwritten, so items accumulate under the drop policy and are delivered by nobody"
                    .to_string(),
            ),
        );
        out
    }
}

/// What one enqueue cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enqueued {
    /// The new item's handle.
    pub id: String,
    /// Its sequence.
    pub sequence: u64,
    /// Items discarded to make room. §47.4: a loss nobody counted is a loss nobody sees.
    pub dropped: u64,
    /// Check-ins superseded. Not a loss — the newer one carries the same state.
    pub collapsed: u64,
}

fn parse_name(name: &str) -> Option<(Class, u64)> {
    let mut parts = name.splitn(3, '-');
    let _priority = parts.next()?;
    let sequence = parts.next()?.parse().ok()?;
    let class = Class::parse(parts.next()?)?;
    Some((class, sequence))
}

/// The cap §34.4 puts on backoff: fifteen minutes.
pub const BACKOFF_CAP_MS: u64 = 15 * 60 * 1000;
/// The first retry delay, doubled per attempt up to the cap.
pub const BACKOFF_BASE_MS: u64 = 1_000;

/// Exponential backoff with jitter, capped (§34.4).
///
/// `jitter` is a fraction in `[0, 1)` — supplied, not drawn, for §35.2's reason: a function that
/// reaches for a random number cannot be tested, and an untested backoff is how a fleet discovers
/// the cap was off by a factor of sixty.
///
/// **Full jitter**, not a small perturbation: the delay is uniform over `[0, exponential]` rather
/// than `exponential ± 10%`. The narrow form leaves a hundred robots that lost the same access point
/// retrying inside the same tenth of a window, which is the thundering herd §34.4 names — smeared
/// slightly rather than broken up. Full jitter is the form that actually decorrelates them, and its
/// cost is that a robot occasionally retries sooner than the curve suggests, which is harmless.
pub fn backoff_ms(attempts: u32, jitter: f64) -> u64 {
    if attempts == 0 {
        return 0;
    }
    let exponential = BACKOFF_BASE_MS
        .saturating_mul(1u64 << (attempts - 1).min(63))
        .min(BACKOFF_CAP_MS);
    let fraction = if jitter.is_finite() {
        jitter.clamp(0.0, 1.0)
    } else {
        0.0
    };
    (exponential as f64 * fraction) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let path = std::env::temp_dir().join(format!("skynet-outbound-{name}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("temp dir");
            Dir(path)
        }
        /// A queue small enough to fill in a test, with a reserve a quarter its size.
        fn queue(&self) -> Queue {
            Queue::at(self.0.join("outbound"), 400, 100)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn body(size: usize) -> String {
        "x".repeat(size)
    }

    fn held(queue: &Queue, class: Class) -> usize {
        queue
            .status()
            .held
            .iter()
            .find(|(c, _)| *c == class)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    }

    #[test]
    fn an_empty_queue_answers_rather_than_failing() {
        let dir = Dir::new("empty");
        let queue = dir.queue();
        assert!(queue.next().is_none());
        assert!(queue.items().is_empty());
        assert_eq!(queue.status().collapsed, 0);
    }

    /// Priority first, age second — which is what makes the directory listing the send order.
    #[test]
    fn the_send_order_is_priority_then_age() {
        let dir = Dir::new("order");
        let queue = dir.queue();
        queue.enqueue(Class::Telemetry, "t1").unwrap();
        queue.enqueue(Class::Episode, "e1").unwrap();
        queue.enqueue(Class::Incident, "i1").unwrap();
        queue.enqueue(Class::Telemetry, "t2").unwrap();

        let items = queue.items();
        let order: Vec<&str> = items.iter().map(|item| item.body.as_str()).collect();
        assert_eq!(order, vec!["i1", "e1", "t1", "t2"]);
        assert_eq!(queue.next().unwrap().body, "i1");
    }

    /// §34.4's level-triggered rule: a week offline produces one current check-in on reconnect, not
    /// ten thousand historical ones.
    #[test]
    fn a_check_in_supersedes_the_last_one() {
        let dir = Dir::new("collapse");
        let queue = dir.queue();
        for generation in 0..5 {
            let outcome = queue
                .enqueue(Class::CheckIn, &format!("generation {generation}"))
                .unwrap();
            assert_eq!(outcome.collapsed, u64::from(generation > 0));
        }

        assert_eq!(held(&queue, Class::CheckIn), 1);
        assert_eq!(queue.next().unwrap().body, "generation 4");
        // Counted, and counted as *collapsed* rather than dropped: nothing was lost.
        assert_eq!(queue.status().collapsed, 4);
        let dropped: u64 = queue.status().dropped.iter().map(|(_, n)| n).sum();
        assert_eq!(dropped, 0);
    }

    #[test]
    fn telemetry_is_discarded_oldest_first_and_the_loss_is_counted() {
        let dir = Dir::new("telemetry");
        let queue = dir.queue();
        // Budget 400 with a 100 reserve leaves 300 for everything else.
        for index in 0..3 {
            queue
                .enqueue(Class::Telemetry, &format!("{index}{}", body(99)))
                .unwrap();
        }
        assert_eq!(held(&queue, Class::Telemetry), 3);

        let outcome = queue
            .enqueue(Class::Telemetry, &format!("3{}", body(99)))
            .unwrap();
        assert_eq!(outcome.dropped, 1);
        assert_eq!(held(&queue, Class::Telemetry), 3);
        // The oldest went, and the newest is the one that arrived.
        assert!(
            queue.items()[0].body.starts_with('1'),
            "{:?}",
            queue.items()[0].body
        );

        let dropped = queue
            .status()
            .dropped
            .into_iter()
            .find(|(class, _)| *class == Class::Telemetry)
            .unwrap()
            .1;
        assert_eq!(dropped, 1);
    }

    /// The rule that makes the four classes worth having: filling a disk with telemetry must not be
    /// able to evict an episode somebody collected and paid for.
    #[test]
    fn telemetry_never_evicts_an_episode() {
        let dir = Dir::new("no-upward-eviction");
        let queue = dir.queue();
        queue.enqueue(Class::Episode, &body(290)).unwrap();

        // Ten bytes are left in the shared pool; this needs fifty.
        let error = queue.enqueue(Class::Telemetry, &body(50)).unwrap_err();
        assert!(matches!(error, QueueError::TooLarge { .. }), "{error:?}");
        assert_eq!(held(&queue, Class::Episode), 1);
        assert_eq!(held(&queue, Class::Telemetry), 0);
    }

    /// And the reverse: an episode may evict telemetry, because telemetry is what §34.4 drops
    /// aggressively.
    #[test]
    fn an_episode_evicts_telemetry_to_make_room() {
        let dir = Dir::new("downward-eviction");
        let queue = dir.queue();
        for index in 0..3 {
            queue
                .enqueue(Class::Telemetry, &format!("{index}{}", body(99)))
                .unwrap();
        }
        let outcome = queue.enqueue(Class::Episode, &body(200)).unwrap();
        assert_eq!(outcome.dropped, 2);
        assert_eq!(held(&queue, Class::Episode), 1);
        assert_eq!(held(&queue, Class::Telemetry), 1);
    }

    /// The whole reason the reserve exists. A queue packed to its shared limit with lower classes
    /// still takes an incident, and does not have to discard anything to do it.
    #[test]
    fn an_incident_fits_into_a_queue_that_is_otherwise_full() {
        let dir = Dir::new("reserve");
        let queue = dir.queue();
        for index in 0..3 {
            queue
                .enqueue(Class::Telemetry, &format!("{index}{}", body(99)))
                .unwrap();
        }
        assert_eq!(held(&queue, Class::Telemetry), 3);

        let outcome = queue.enqueue(Class::Incident, &body(90)).unwrap();
        assert_eq!(
            outcome.dropped, 0,
            "the reserve is not made of other people's items"
        );
        assert_eq!(held(&queue, Class::Incident), 1);
        assert_eq!(held(&queue, Class::Telemetry), 3);
        assert_eq!(queue.next().unwrap().class, Class::Incident);
    }

    /// An incident is never dropped, so a second one that will not fit is an error naming a full
    /// disk — not a licence to discard the first.
    #[test]
    fn an_incident_that_does_not_fit_is_an_error_and_not_a_discard() {
        let dir = Dir::new("reserve-full");
        let queue = dir.queue();
        queue.enqueue(Class::Incident, &body(390)).unwrap();

        let error = queue.enqueue(Class::Incident, &body(90)).unwrap_err();
        assert!(matches!(error, QueueError::ReserveFull { .. }), "{error:?}");
        assert!(error.reason().contains("full disk"), "{}", error.reason());
        assert_eq!(
            held(&queue, Class::Incident),
            1,
            "the first incident survived"
        );
    }

    /// It is on disk, and that is the requirement: a reboot is a new `Queue` over the same
    /// directory, and it must see everything — including the counters.
    #[test]
    fn the_queue_survives_being_reopened() {
        let dir = Dir::new("reboot");
        {
            let queue = dir.queue();
            queue.enqueue(Class::Episode, "e").unwrap();
            for index in 0..4 {
                queue
                    .enqueue(Class::Telemetry, &format!("{index}{}", body(99)))
                    .unwrap();
            }
        }
        let reopened = dir.queue();
        assert!(reopened.status().dropped.iter().any(|(_, n)| *n > 0));
        assert_eq!(held(&reopened, Class::Episode), 1);
        assert_eq!(reopened.next().unwrap().class, Class::Episode);
    }

    /// Sequences come from a counter rather than from the highest name present, so a queue that
    /// drains completely does not restart its numbering and make two items ambiguous later.
    #[test]
    fn a_drained_queue_does_not_reuse_a_sequence() {
        let dir = Dir::new("sequence");
        let queue = dir.queue();
        let first = queue.enqueue(Class::Telemetry, "a").unwrap();
        queue.remove(&first.id).unwrap();
        assert!(queue.items().is_empty());

        let second = queue.enqueue(Class::Telemetry, "b").unwrap();
        assert_eq!(second.sequence, first.sequence + 1);
    }

    #[test]
    fn a_failed_delivery_counts_towards_backoff() {
        let dir = Dir::new("attempts");
        let queue = dir.queue();
        let item = queue.enqueue(Class::CheckIn, "state").unwrap();

        assert_eq!(queue.attempted(&item.id).unwrap(), 1);
        assert_eq!(queue.attempted(&item.id).unwrap(), 2);
        assert_eq!(queue.next().unwrap().attempts, 2);
        // And the body survives the rewrite, which is the thing a partial update would lose.
        assert_eq!(queue.next().unwrap().body, "state");
    }

    #[test]
    fn an_item_id_cannot_leave_the_directory() {
        let dir = Dir::new("traversal");
        let queue = dir.queue();
        assert!(queue.remove("../../etc/passwd").is_err());
        assert!(queue.remove("a/b").is_err());
    }

    /// §34.4 reserves disk for incidents *at install*, which means the budget is a property of the
    /// queue on the disk rather than of whoever opened it. Two openers disagreeing would enforce
    /// two drop policies over one directory and report a limit nothing was applying.
    #[test]
    fn the_budget_is_recorded_with_the_queue_and_survives_a_reopen() {
        let dir = Dir::new("sized");
        let queue = dir.queue();
        queue.enqueue(Class::Telemetry, "t").unwrap();
        assert_eq!(queue.status().budget, 400);

        // A second opener asking for the default gets the queue's own numbers instead.
        let reopened =
            Queue::at(dir.0.join("outbound"), DEFAULT_BUDGET, DEFAULT_RESERVE).sized_from_disk();
        assert_eq!(reopened.status().budget, 400);
        assert_eq!(reopened.status().reserve, 100);
    }

    /// A queue nobody has sized takes the default and writes it down, so the *next* opener agrees
    /// with this one even if the defaults change under it.
    #[test]
    fn a_new_queue_records_the_default_it_was_given() {
        let dir = Dir::new("default-size");
        let queue = Queue::at(dir.0.join("outbound"), DEFAULT_BUDGET, DEFAULT_RESERVE);
        assert_eq!(queue.status().budget, DEFAULT_BUDGET);

        let smaller = Queue::at(dir.0.join("outbound"), 4_096, 1_024).sized_from_disk();
        assert_eq!(
            smaller.status().budget,
            DEFAULT_BUDGET,
            "a later opener must not silently re-size a queue that already has a policy"
        );
    }

    /// §34.4's cap, and full jitter rather than a perturbation.
    #[test]
    fn backoff_doubles_jitters_and_caps() {
        assert_eq!(backoff_ms(0, 1.0), 0);
        assert_eq!(backoff_ms(1, 1.0), 1_000);
        assert_eq!(backoff_ms(2, 1.0), 2_000);
        assert_eq!(backoff_ms(10, 1.0), 512_000);
        // Doubling would reach 1,024,000 ms on the eleventh attempt, which is past fifteen minutes,
        // so that is where the cap starts biting — about eight and a half minutes into the retry
        // sequence rather than at some round number.
        assert_eq!(backoff_ms(11, 1.0), BACKOFF_CAP_MS);
        assert_eq!(backoff_ms(60, 1.0), BACKOFF_CAP_MS);

        // Full jitter: uniform over [0, exponential], not exponential ± a little. A hundred robots
        // drawing here spread across the whole window rather than across a tenth of it.
        assert_eq!(backoff_ms(12, 0.0), 0);
        assert_eq!(backoff_ms(12, 0.5), BACKOFF_CAP_MS / 2);

        // A jitter nobody clamped is a jitter that could exceed the cap.
        assert_eq!(backoff_ms(12, 4.0), BACKOFF_CAP_MS);
        assert_eq!(backoff_ms(12, f64::NAN), 0);
    }

    /// An overflow guard rather than a behaviour: `1 << 64` is undefined, and an attempt counter is
    /// a `u32` that a robot offline for a decade could plausibly reach.
    #[test]
    fn a_very_large_attempt_count_does_not_overflow() {
        assert_eq!(backoff_ms(u32::MAX, 1.0), BACKOFF_CAP_MS);
    }
}

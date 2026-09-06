//! The robot's half of §82: answering a browse.
//!
//! [`crate::browse`] asks; nothing answered. This is the other end of that sentence, and until it
//! existed a browse on a real network found nothing — a true report, and an empty one.
//!
//! # Why the socket is not here
//!
//! What to say is a property of the protocol and belongs beside [`crate::service::advertisement`],
//! which produces the records; §78 is this repository's record of what two spellings of one
//! document cost, and a responder that rebuilt the record set would be exactly that. What to say it
//! *on* is a property of the machine: binding port 5353, joining a multicast group and choosing an
//! interface are the daemon's, and `skynetd_io::discovery` owns them.
//!
//! So this type takes bytes and a clock reading and returns bytes and a destination. It opens
//! nothing, and every rule in it is therefore reachable from a test without a network — which is
//! most of what there is to get wrong.
//!
//! # A name two robots answer to is a stop button pointed at the wrong machine
//!
//! RFC 6762 §8.1 requires a responder to probe before claiming a name and, on a conflict, to pick a
//! different one — typically by appending a number. **This does not rename.** The reason is what
//! the name is for here: an operator reads a label off a list and opens the console it points at,
//! and the console's largest control is a stop (§32.2). Two robots answering to `arm-01` is an
//! operator stopping the machine that is not the one in front of them; `arm-01 (2)` is a label
//! nobody assigned, on a bench where the physical robots carry the labels somebody did.
//!
//! So a conflict is **refused and named**, which is the same answer `skynet-lock` gives to a second
//! writer of one recording: the holder is named, the fix is stated, and nothing is stolen. The
//! console keeps serving — only discovery stops, and the daemon reports why. A robot nobody can
//! find is a bootstrap problem; a robot found under somebody else's name is a safety one.
//!
//! **Which robot refuses is the whole of it**, and the first version of this got it backwards. A
//! responder that treats every claim on its name as a conflict yields to whoever booted last: the
//! newcomer probes, the incumbent falls silent, the newcomer hears nothing and takes the name. Two
//! daemons on one link demonstrated exactly that, and no single-responder test could have. An
//! established responder **defends** — see [`Rival`].
//!
//! # What it will not answer, and when
//!
//! * **Nothing at all before the claim completes.** Answering during probing is claiming a name
//!   this robot has not yet established, which is the failure probing exists to prevent.
//! * **Nothing after a conflict**, until the label changes and the daemon restarts.
//! * **Nothing about another service.** A query for `_http._tcp` is somebody else's.
//! * **Nothing to itself.** Multicast loopback returns this responder's own datagrams, and a
//!   responder that read its own announcement as another robot's would refuse to advertise the
//!   moment it started. Every datagram this type can produce is recognised and dropped.

use std::net::{IpAddr, SocketAddr};

use crate::service::{self, endpoint};
use crate::wire::{
    Message, Name, Question, Record, RecordData, CLASS_IN, TYPE_A, TYPE_AAAA, TYPE_ANY, TYPE_PTR,
    TYPE_SRV, TYPE_TXT,
};

/// How long between probes (RFC 6762 §8.1: 250 ms).
pub const PROBE_INTERVAL_MS: u64 = 250;

/// How many probes before a name is claimed (RFC 6762 §8.1: three).
pub const PROBES: u32 = 3;

/// How long between announcements (RFC 6762 §8.3: one second).
pub const ANNOUNCE_INTERVAL_MS: u64 = 1_000;

/// How many announcements (RFC 6762 §8.3 permits up to eight; two is the common floor).
pub const ANNOUNCEMENTS: u32 = 2;

/// The shortest delay before a multicast answer to a shared record (RFC 6762 §6).
pub const SHARED_DELAY_MIN_MS: u64 = 20;

/// The longest such delay.
pub const SHARED_DELAY_MAX_MS: u64 = 120;

/// Where a datagram goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    /// The mDNS group, on whichever family the socket is.
    Multicast,
    /// Straight back to the asker (RFC 6762 §5.4's QU bit).
    Unicast(SocketAddr),
}

/// One datagram, ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outgoing {
    /// The encoded message.
    pub datagram: Vec<u8>,
    /// Where it goes.
    pub destination: Destination,
}

/// Where this responder stands on the name it wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Claim {
    /// Probing, with the number of probes still to send.
    Probing {
        /// Probes not yet sent.
        remaining: u32,
    },
    /// Established. The only state in which anything is answered.
    Claimed,
    /// Somebody else on this link answers to this name, so this robot does not.
    Conflicted {
        /// The address the conflicting record came from, for the operator to go and look at.
        seen_from: IpAddr,
    },
}

impl Claim {
    /// Whether a browse would find this robot.
    pub fn advertising(&self) -> bool {
        matches!(self, Claim::Claimed)
    }
}

impl std::fmt::Display for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Claim::Probing { remaining } => write!(f, "probing ({remaining} to send)"),
            Claim::Claimed => write!(f, "advertising"),
            Claim::Conflicted { seen_from } => write!(
                f,
                "not advertising: another responder at {seen_from} already answers to this name. \
                 Give one of the two robots a different label and restart; this one will not \
                 rename itself, because a label an operator did not assign is a console they \
                 cannot identify"
            ),
        }
    }
}

/// What this responder has seen, for the same reason [`crate::browse::Outcome`] carries counters:
/// an empty result and a broken link look identical from the outside.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    /// Queries for this robot that were answered.
    pub answered: usize,
    /// Datagrams that arrived and would not parse.
    pub malformed: usize,
    /// Datagrams that parsed and were about some other service.
    pub other_services: usize,
    /// This responder's own datagrams, returned by multicast loopback.
    pub own: usize,
    /// Probes for this robot's name that it answered because it already held it.
    ///
    /// Worth a counter of its own rather than folding into `answered`: a robot that has defended
    /// its name is a robot somebody else on the link was told to call something different, and it
    /// is the first thing to look at when two benches disagree about which arm is which.
    pub defended: usize,
}

/// What an arriving message says about this robot's name.
///
/// The distinction is the one a live run found this type getting wrong. A responder that treats
/// both shapes as a conflict **yields its name to whichever robot booted last**: the newcomer
/// probes, the incumbent reads the probe as a rival and stops advertising, the newcomer hears
/// nothing and claims — and a browse then points an operator at the robot that arrived second,
/// which is the exact hazard this module's header says it exists to prevent.
///
/// So the two are separated and answered differently:
///
/// * [`Rival::Established`] — a **response** carrying a record named for this instance. Somebody
///   already answers to this name, and this responder does not, whatever state it is in.
/// * [`Rival::Probe`] — a **query with an authority section** for this name: somebody claiming it
///   right now. An established responder **defends** it, immediately and by multicast, which is
///   RFC 6762 §8.1's own rule and what makes the prober back off. A responder still probing has
///   nothing to defend and refuses — §8.2 would break that tie by comparing the proposed records,
///   and the loser of a tie-break is a robot that has to rename itself, which this one will not do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rival {
    None,
    Established,
    Probe,
}

/// What a queued entry is.
///
/// Only answers and defences are queued. Probes and announcements are paced by
/// [`Responder::next_step_ms`] instead, because a queue of them can fall behind and then discharge
/// all at once, which is the defect `due`'s header is about.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Kind {
    Answer(Destination),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pending {
    due_ms: u64,
    kind: Kind,
}

/// A robot's mDNS responder: what to say, and when.
#[derive(Debug, Clone)]
pub struct Responder {
    instance: Name,
    host_name: Name,
    service_name: Name,
    /// The advertisement, encoded once. Every answer and every announcement is these bytes.
    advertisement: Vec<u8>,
    /// The same records at TTL 0 (RFC 6762 §10.1), sent on the way out.
    goodbye: Vec<u8>,
    /// The probe, encoded once.
    probe: Vec<u8>,
    addresses: Vec<IpAddr>,
    claim: Claim,
    /// Answers and defences waiting on RFC 6762 §6's delay. Never probes or announcements.
    pending: Vec<Pending>,
    /// Probes still to send.
    probes_left: u32,
    /// Announcements still to send, once the name is held.
    announcements_left: u32,
    /// The earliest the next probe or announcement may go.
    ///
    /// A single field because the two phases are sequential, and — more importantly — because it is
    /// measured **from the last one that actually went out** rather than from a schedule fixed at
    /// start-up. See [`Responder::due`] for what that difference cost when it was the other way.
    next_step_ms: u64,
    counters: Counters,
}

impl Responder {
    /// The mDNS port, restated so a socket layer has one place to read it from.
    pub const PORT: u16 = endpoint::PORT;

    /// Build a responder for one robot, and start probing at `now_ms`.
    ///
    /// `label` is the instance name an operator reads; `host` is the machine's own name, which the
    /// SRV record targets and the address records are filed under. `addresses` are the addresses
    /// the console is actually bound to — never every address the machine has, because §34.2 binds
    /// the console to one and a record naming an interface it does not answer on sends an operator
    /// to a port that is closed.
    ///
    /// `None` when the advertisement cannot be encoded, which for this key set means a pathological
    /// label: a responder with no record to send sends nothing rather than an advertisement missing
    /// the field that says how to reach it.
    pub fn new(
        label: &str,
        host: &str,
        port: u16,
        addresses: &[IpAddr],
        now_ms: u64,
    ) -> Option<Self> {
        let advertisement = service::advertisement(label, host, port, addresses, false)?
            .encode()
            .ok()?;
        let goodbye = service::advertisement(label, host, port, addresses, true)?
            .encode()
            .ok()?;
        let instance = Name::parse(&format!("{label}.{}", service::SERVICE));
        let host_name = Name::parse(&format!("{host}.local"));
        // The unique records are the proposal; the PTR is not one of them, because it enumerates a
        // service several instances legitimately share (RFC 6762 §10.2, and the comment in
        // `service::advertisement` that sets `cache_flush` accordingly).
        let proposed: Vec<Record> = service::advertisement(label, host, port, addresses, false)?
            .answers
            .into_iter()
            .filter(|record| record.cache_flush)
            .collect();
        let probe = probe_message(&instance, proposed).encode().ok()?;

        Some(Responder {
            instance,
            host_name,
            service_name: Name::parse(service::SERVICE),
            advertisement,
            goodbye,
            probe,
            addresses: addresses.to_vec(),
            claim: Claim::Probing { remaining: PROBES },
            pending: Vec::new(),
            probes_left: PROBES,
            announcements_left: 0,
            next_step_ms: now_ms,
            counters: Counters::default(),
        })
    }

    /// Where the claim stands.
    pub fn claim(&self) -> &Claim {
        &self.claim
    }

    /// What has been seen.
    pub fn counters(&self) -> &Counters {
        &self.counters
    }

    /// The instance name, as text.
    pub fn instance(&self) -> String {
        self.instance.to_text()
    }

    /// The addresses this responder advertises, for a caller reporting what it is saying.
    pub fn addresses(&self) -> &[IpAddr] {
        &self.addresses
    }

    /// Fold one arriving datagram in.
    ///
    /// `jitter` is a fraction in `0.0..1.0`, supplied by the caller rather than drawn here: §35.2's
    /// rule that a component is handed its clock and its randomness is what makes RFC 6762 §6's
    /// delay reproducible in a test.
    pub fn heard(&mut self, datagram: &[u8], from: SocketAddr, now_ms: u64, jitter: f64) {
        if self.is_own(datagram) {
            self.counters.own += 1;
            return;
        }
        let Ok(message) = Message::decode(datagram) else {
            self.counters.malformed += 1;
            return;
        };

        match self.rival(&message) {
            // Somebody is already answering to this name. Terminal, and it clears anything queued:
            // a probe still in the queue would go on claiming a name this responder has just
            // learned it does not own.
            Rival::Established => {
                self.claim = Claim::Conflicted {
                    seen_from: from.ip(),
                };
                self.pending.clear();
                return;
            }
            // Somebody is claiming this name *now*. Which of the two is right depends entirely on
            // whether this robot already holds it — and getting that backwards is how a responder
            // hands its name to whoever booted last. See [`Rival`].
            Rival::Probe => {
                if self.claim.advertising() {
                    self.counters.defended += 1;
                    self.pending.push(Pending {
                        due_ms: now_ms,
                        kind: Kind::Answer(Destination::Multicast),
                    });
                } else {
                    self.claim = Claim::Conflicted {
                        seen_from: from.ip(),
                    };
                    self.pending.clear();
                }
                return;
            }
            Rival::None => {}
        }

        if message.is_response() {
            self.counters.other_services += 1;
            return;
        }

        // Nothing is answered before the name is established, and nothing after it is lost.
        if !self.claim.advertising() {
            return;
        }

        let mut unicast = false;
        let mut shared = false;
        let mut unique = false;
        for question in &message.questions {
            if self.asks_about_service(question) {
                shared = true;
            } else if self.asks_about_us(question) {
                unique = true;
            } else {
                continue;
            }
            unicast |= question.unicast_response;
        }

        if !shared && !unique {
            if !message.questions.is_empty() {
                self.counters.other_services += 1;
            }
            return;
        }

        self.counters.answered += 1;
        let destination = if unicast {
            Destination::Unicast(from)
        } else {
            Destination::Multicast
        };
        // RFC 6762 §6: an answer to a *shared* record waits 20–120 ms so that responders on a busy
        // link do not all transmit at once; an answer only about this robot's own unique records,
        // and any unicast answer, goes at once. The PTR enumerating `_skynet._tcp` is the shared
        // one, and it is exactly the record a browse asks for — so the delay is the normal path
        // here rather than the exception.
        let due_ms = if shared && !unicast {
            now_ms.saturating_add(shared_delay(jitter))
        } else {
            now_ms
        };
        self.pending.push(Pending {
            due_ms,
            kind: Kind::Answer(destination),
        });
    }

    /// Everything due at `now_ms`, in the order it should go out.
    ///
    /// # One probe per pass, timed from the last one that left
    ///
    /// The obvious implementation queues three probes at start-up, 250 ms apart, and sends whatever
    /// is overdue. It is wrong, and a live run is what showed it: `skynetd` preallocates a black box
    /// on its first pass, so by the time it polls this the whole probe schedule is in the past —
    /// all three go out in one datagram burst and the name is claimed in the same instant. **A
    /// responder that sends its probes together has not probed.** The 250 ms in RFC 6762 §8.1 is
    /// not pacing; it is the window in which somebody else gets to say the name is taken, and a
    /// robot that closes it wins every conflict against a robot that is already there.
    ///
    /// So a probe is sent only when the interval has elapsed **since the previous one actually
    /// went**, one per call, and the claim completes one interval after the last of them rather
    /// than in the same breath. Answers and defences are unaffected — those are replies, and
    /// nothing is waiting on the link for them.
    pub fn due(&mut self, now_ms: u64) -> Vec<Outgoing> {
        let mut out = Vec::new();
        let mut still_pending = Vec::new();

        for entry in std::mem::take(&mut self.pending) {
            if entry.due_ms > now_ms {
                still_pending.push(entry);
                continue;
            }
            match entry.kind {
                Kind::Answer(destination) => out.push(Outgoing {
                    datagram: self.advertisement.clone(),
                    destination,
                }),
            }
        }
        self.pending = still_pending;

        if now_ms < self.next_step_ms {
            return out;
        }

        match self.claim {
            Claim::Probing { .. } => {
                if self.probes_left > 0 {
                    self.probes_left -= 1;
                    self.claim = Claim::Probing {
                        remaining: self.probes_left,
                    };
                    self.next_step_ms = now_ms.saturating_add(PROBE_INTERVAL_MS);
                    out.push(Outgoing {
                        datagram: self.probe.clone(),
                        destination: Destination::Multicast,
                    });
                } else {
                    // A full interval has passed since the last probe with nobody objecting, so the
                    // name is this robot's. RFC 6762 §8.3's announcements start immediately: an
                    // operator who has just powered a robot on should not wait another second for
                    // it to appear in a list that is already open.
                    self.claim = Claim::Claimed;
                    self.announcements_left = ANNOUNCEMENTS;
                    out.extend(self.announce(now_ms));
                }
            }
            Claim::Claimed => out.extend(self.announce(now_ms)),
            Claim::Conflicted { .. } => {}
        }

        out
    }

    /// The next announcement, if one is owed and the interval has passed.
    fn announce(&mut self, now_ms: u64) -> Option<Outgoing> {
        if self.announcements_left == 0 {
            return None;
        }
        self.announcements_left -= 1;
        self.next_step_ms = now_ms.saturating_add(ANNOUNCE_INTERVAL_MS);
        Some(Outgoing {
            datagram: self.advertisement.clone(),
            destination: Destination::Multicast,
        })
    }

    /// When the next thing is due, for a caller that would otherwise poll blindly.
    pub fn next_due_ms(&self) -> Option<u64> {
        let step = match self.claim {
            Claim::Probing { .. } => Some(self.next_step_ms),
            Claim::Claimed if self.announcements_left > 0 => Some(self.next_step_ms),
            _ => None,
        };
        self.pending
            .iter()
            .map(|entry| entry.due_ms)
            .chain(step)
            .min()
    }

    /// The goodbye (RFC 6762 §10.1), or nothing if this robot never claimed the name.
    ///
    /// A responder that withdrew a name it never held would be telling every cache on the link to
    /// drop records belonging to whichever robot actually owns it.
    pub fn farewell(&self) -> Option<Outgoing> {
        self.claim.advertising().then(|| Outgoing {
            datagram: self.goodbye.clone(),
            destination: Destination::Multicast,
        })
    }

    /// Whether these bytes are ones this responder itself can send.
    ///
    /// Compared as bytes rather than by source address, and that is the point: multicast loopback
    /// returns a datagram from whichever interface address the stack sent it on, which need not be
    /// the one address §34.2 bound the console to. A source-address test would therefore let a
    /// responder read its own announcement as a rival's and refuse to advertise a name nobody else
    /// wanted.
    fn is_own(&self, datagram: &[u8]) -> bool {
        datagram == self.advertisement || datagram == self.probe || datagram == self.goodbye
    }

    /// Whether a message is somebody else answering to, or asking for, this robot's name.
    fn rival(&self, message: &Message) -> Rival {
        if message.is_response() {
            return if message
                .records()
                .any(|record| record.name.eq_ignore_case(&self.instance))
            {
                Rival::Established
            } else {
                Rival::None
            };
        }
        // A probe is a query that carries its proposed records in the authority section. A plain
        // browse carries none, which is what keeps an ordinary query from reading as a claim.
        if !message.authorities.is_empty()
            && message
                .questions
                .iter()
                .any(|question| question.name.eq_ignore_case(&self.instance))
        {
            return Rival::Probe;
        }
        Rival::None
    }

    /// A question about `_skynet._tcp` itself — the shared PTR a browse asks for.
    fn asks_about_service(&self, question: &Question) -> bool {
        question.name.eq_ignore_case(&self.service_name)
            && (question.qtype == TYPE_PTR || question.qtype == TYPE_ANY)
    }

    /// A question about this robot's own records: the instance, or the host the SRV points at.
    fn asks_about_us(&self, question: &Question) -> bool {
        if question.name.eq_ignore_case(&self.instance) {
            return matches!(question.qtype, TYPE_SRV | TYPE_TXT | TYPE_ANY);
        }
        if question.name.eq_ignore_case(&self.host_name) {
            return matches!(question.qtype, TYPE_A | TYPE_AAAA | TYPE_ANY);
        }
        false
    }
}

/// RFC 6762 §6's delay for a shared record, from a fraction the caller supplies.
fn shared_delay(jitter: f64) -> u64 {
    let span = SHARED_DELAY_MAX_MS - SHARED_DELAY_MIN_MS;
    let fraction = if jitter.is_finite() {
        jitter.clamp(0.0, 1.0)
    } else {
        0.0
    };
    SHARED_DELAY_MIN_MS + (fraction * span as f64) as u64
}

/// The probe: a query for the instance name, asking for any record anybody holds under it.
///
/// `proposed` is what this robot intends to publish under that name — its SRV, its TXT and its
/// addresses. RFC 6762 §8.1 puts them in the authority section, and two separate things here depend
/// on their being the real ones rather than a placeholder:
///
/// * a **non-empty** authority section is what distinguishes a probe from a browse, so a responder
///   reading one knows a claim is being made rather than a question asked;
/// * their **content** is what makes one robot's probe different from another's. A probe carrying
///   only a PTR to the instance name is byte-identical between two robots that chose the same
///   label — which is exactly the pair this matters for — and a responder comparing datagrams to
///   recognise its own would then read a rival's probe as its own loopback and never defend. That
///   was a real defect, found by a test that put two responders in a room together.
fn probe_message(instance: &Name, proposed: Vec<Record>) -> Message {
    Message {
        id: 0,
        flags: 0,
        questions: vec![Question {
            name: instance.clone(),
            qtype: TYPE_ANY,
            // RFC 6762 §8.1 asks for a unicast reply to a probe: the answer is for the prober, and
            // multicasting it would tell the whole link about a name that is being contested.
            unicast_response: true,
        }],
        answers: Vec::new(),
        authorities: if proposed.is_empty() {
            // Only reachable if an advertisement had no unique records at all, which
            // `service::advertisement` cannot produce. Kept as a claim rather than degrading into
            // something that reads as a browse.
            vec![Record {
                name: instance.clone(),
                class: CLASS_IN,
                cache_flush: false,
                ttl: service::TTL,
                data: RecordData::Ptr(instance.clone()),
            }]
        } else {
            proposed
        },
        additionals: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::advertisement;
    use std::net::Ipv4Addr;

    const V4: IpAddr = IpAddr::V4(Ipv4Addr::new(169, 254, 3, 4));

    fn asker() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(169, 254, 9, 9), 5353))
    }

    /// A responder that has finished probing, with everything it owed the link drained.
    fn claimed() -> Responder {
        let mut responder = Responder::new("arm-01", "arm-01", 8099, &[V4], 0).unwrap();
        for step in 0..PROBES {
            let out = responder.due(u64::from(step) * PROBE_INTERVAL_MS);
            assert_eq!(out.len(), 1, "one probe per pass, never a burst");
            assert!(!responder.claim().advertising());
        }
        // One interval after the last probe, and not before: the window is what the probe is for.
        let out = responder.due(u64::from(PROBES) * PROBE_INTERVAL_MS);
        assert_eq!(responder.claim(), &Claim::Claimed);
        assert_eq!(out.len(), 1, "the first announcement");
        // And the second, so that a test asserting silence is asserting silence rather than
        // reading RFC 6762 §8.3's second transmission as an answer to its own question.
        let _ = responder.due(u64::from(PROBES) * PROBE_INTERVAL_MS + ANNOUNCE_INTERVAL_MS);
        assert_eq!(
            responder.next_due_ms(),
            None,
            "claimed, announced, and quiet"
        );
        responder
    }

    fn browse_query(unicast: bool) -> Vec<u8> {
        service::query(unicast).encode().unwrap()
    }

    #[test]
    fn nothing_is_answered_until_the_name_is_claimed() {
        let mut responder = Responder::new("arm-01", "arm-01", 8099, &[V4], 0).unwrap();
        responder.heard(&browse_query(true), asker(), 0, 0.0);
        assert_eq!(responder.counters().answered, 0);
        assert!(matches!(responder.claim(), Claim::Probing { .. }));
    }

    #[test]
    fn three_probes_go_one_per_pass_and_the_claim_waits_an_interval_after_the_last() {
        let mut responder = Responder::new("arm-01", "arm-01", 8099, &[V4], 0).unwrap();
        assert_eq!(responder.due(0).len(), 1, "one probe is due at zero");
        assert_eq!(
            responder.due(0).len(),
            0,
            "and only one — the interval has not passed"
        );
        assert_eq!(responder.due(PROBE_INTERVAL_MS).len(), 1);
        assert!(matches!(responder.claim(), Claim::Probing { remaining: 1 }));
        assert_eq!(responder.due(2 * PROBE_INTERVAL_MS).len(), 1, "the third");
        assert!(
            !responder.claim().advertising(),
            "sending the last probe is not holding the name — somebody may still object"
        );
        assert!(responder.due(2 * PROBE_INTERVAL_MS + 1).is_empty());
        let claim = responder.due(3 * PROBE_INTERVAL_MS);
        assert_eq!(responder.claim(), &Claim::Claimed);
        assert_eq!(
            claim.len(),
            1,
            "the first announcement, and no fourth probe"
        );
    }

    /// The defect a live run found: a caller that is late does not get to skip the probe window.
    #[test]
    fn a_pass_that_arrives_late_still_probes_one_at_a_time() {
        let mut responder = Responder::new("arm-01", "arm-01", 8099, &[V4], 0).unwrap();
        // `skynetd` preallocates a black box on its first pass, so the first poll can be seconds
        // after the responder was built. Every probe is overdue; they must not go together.
        assert_eq!(responder.due(9_000).len(), 1, "one probe, not three");
        assert!(matches!(responder.claim(), Claim::Probing { remaining: 2 }));
        assert_eq!(responder.due(9_000).len(), 0);
        assert_eq!(responder.due(9_000 + PROBE_INTERVAL_MS).len(), 1);
        assert_eq!(responder.due(9_000 + 2 * PROBE_INTERVAL_MS).len(), 1);
        assert!(!responder.claim().advertising());
        let _ = responder.due(9_000 + 3 * PROBE_INTERVAL_MS);
        assert_eq!(
            responder.claim(),
            &Claim::Claimed,
            "and the name is taken a full interval after the third probe, not before"
        );
    }

    #[test]
    fn a_browse_is_answered_with_the_advertisement() {
        let mut responder = claimed();
        responder.heard(&browse_query(true), asker(), 2_000, 0.5);
        let out = responder.due(2_000);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].destination, Destination::Unicast(asker()));
        assert_eq!(
            out[0].datagram,
            advertisement("arm-01", "arm-01", 8099, &[V4], false)
                .unwrap()
                .encode()
                .unwrap(),
            "the answer is the advertisement, not a second spelling of it"
        );
    }

    #[test]
    fn a_multicast_question_waits_and_a_unicast_one_does_not() {
        let mut responder = claimed();
        responder.heard(&browse_query(false), asker(), 2_000, 0.5);
        assert!(responder.due(2_000).is_empty(), "§6's delay is not zero");
        let out = responder.due(2_000 + SHARED_DELAY_MAX_MS);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].destination, Destination::Multicast);

        let mut responder = claimed();
        responder.heard(&browse_query(true), asker(), 2_000, 0.5);
        assert_eq!(responder.due(2_000).len(), 1, "a QU answer goes at once");
    }

    #[test]
    fn the_delay_is_the_callers_jitter_and_stays_inside_the_rfcs_window() {
        assert_eq!(shared_delay(0.0), SHARED_DELAY_MIN_MS);
        assert_eq!(shared_delay(1.0), SHARED_DELAY_MAX_MS);
        assert!((SHARED_DELAY_MIN_MS..=SHARED_DELAY_MAX_MS).contains(&shared_delay(0.5)));
        assert_eq!(
            shared_delay(f64::NAN),
            SHARED_DELAY_MIN_MS,
            "never a wait of NaN"
        );
        assert_eq!(
            shared_delay(9.0),
            SHARED_DELAY_MAX_MS,
            "clamped, not scaled"
        );
    }

    #[test]
    fn a_question_about_another_service_is_not_answered() {
        let mut responder = claimed();
        let other = Message {
            id: 0,
            flags: 0,
            questions: vec![Question {
                name: Name::parse("_http._tcp.local"),
                qtype: TYPE_PTR,
                unicast_response: true,
            }],
            ..Default::default()
        };
        responder.heard(&other.encode().unwrap(), asker(), 2_000, 0.0);
        assert_eq!(responder.counters().answered, 0);
        assert_eq!(responder.counters().other_services, 1);
        assert!(responder.due(3_000).is_empty());
    }

    #[test]
    fn an_srv_question_about_this_instance_is_answered_immediately() {
        let mut responder = claimed();
        let direct = Message {
            id: 0,
            flags: 0,
            questions: vec![Question {
                name: Name::parse("arm-01._skynet._tcp.local"),
                qtype: TYPE_SRV,
                unicast_response: false,
            }],
            ..Default::default()
        };
        responder.heard(&direct.encode().unwrap(), asker(), 2_000, 1.0);
        let out = responder.due(2_000);
        assert_eq!(out.len(), 1, "a unique record needs no §6 delay");
        assert_eq!(out[0].destination, Destination::Multicast);
    }

    #[test]
    fn its_own_announcement_coming_back_is_not_a_rival() {
        let mut responder = claimed();
        let own = advertisement("arm-01", "arm-01", 8099, &[V4], false)
            .unwrap()
            .encode()
            .unwrap();
        // From an address it does not advertise, which is what multicast loopback does on a
        // machine with more than one interface.
        responder.heard(
            &own,
            SocketAddr::from((Ipv4Addr::new(10, 1, 1, 1), 5353)),
            2_000,
            0.0,
        );
        assert_eq!(responder.claim(), &Claim::Claimed);
        assert_eq!(responder.counters().own, 1);
    }

    #[test]
    fn another_robot_answering_to_this_name_stops_the_advertisement() {
        let mut responder = claimed();
        let rival = advertisement("arm-01", "other-host", 9000, &[V4], false)
            .unwrap()
            .encode()
            .unwrap();
        responder.heard(&rival, asker(), 2_000, 0.0);
        assert_eq!(
            responder.claim(),
            &Claim::Conflicted {
                seen_from: asker().ip()
            }
        );
        assert!(!responder.claim().advertising());
        assert!(
            responder.farewell().is_none(),
            "a name it does not hold is not withdrawn"
        );

        // And it stays refused: a browse arriving afterwards gets nothing.
        responder.heard(&browse_query(true), asker(), 3_000, 0.0);
        assert!(responder.due(4_000).is_empty());
    }

    #[test]
    fn a_rival_probe_during_probing_is_a_conflict_and_the_queue_is_dropped() {
        let mut responder = Responder::new("arm-01", "arm-01", 8099, &[V4], 0).unwrap();
        responder.heard(&rival_probe(10), asker(), 10, 0.0);
        assert!(matches!(responder.claim(), Claim::Conflicted { .. }));
        assert!(
            responder.due(10_000).is_empty(),
            "the queued probes went with the claim"
        );
    }

    /// The first probe of a second robot that wants the same label.
    fn rival_probe(now_ms: u64) -> Vec<u8> {
        let mut rival = Responder::new(
            "arm-01",
            "other-host",
            8100,
            &[IpAddr::V4(Ipv4Addr::new(169, 254, 5, 6))],
            now_ms,
        )
        .unwrap();
        rival.due(now_ms).remove(0).datagram
    }

    #[test]
    fn two_robots_that_want_one_label_still_probe_differently() {
        let mine = Responder::new("arm-01", "arm-01", 8099, &[V4], 0)
            .unwrap()
            .probe;
        assert_ne!(
            mine,
            rival_probe(0),
            "the proposed records are in the probe, so one robot's is not the other's — the \
             defect that let a responder read a rival's probe as its own loopback"
        );
    }

    #[test]
    fn an_established_responder_defends_its_name_instead_of_yielding_it() {
        let mut responder = claimed();
        let rival = rival_probe(2_000);
        responder.heard(&rival, asker(), 2_000, 1.0);

        assert_eq!(
            responder.claim(),
            &Claim::Claimed,
            "the robot that already holds the name keeps it"
        );
        assert_eq!(responder.counters().defended, 1);
        let out = responder.due(2_000);
        assert_eq!(
            out.len(),
            1,
            "a defence is immediate — §6's delay is for shared records"
        );
        assert_eq!(out[0].destination, Destination::Multicast);
        assert_eq!(
            out[0].datagram,
            advertisement("arm-01", "arm-01", 8099, &[V4], false)
                .unwrap()
                .encode()
                .unwrap()
        );
    }

    /// The case two daemons on one link found, and no single responder could.
    #[test]
    fn the_robot_that_arrives_second_is_the_one_that_refuses() {
        let mut incumbent = claimed();
        let mut newcomer = Responder::new(
            "arm-01",
            "other-host",
            8100,
            &[IpAddr::V4(Ipv4Addr::new(169, 254, 5, 6))],
            3_000,
        )
        .unwrap();

        // The newcomer probes; the incumbent hears it.
        let probes = newcomer.due(3_000);
        assert_eq!(probes.len(), 1);
        incumbent.heard(&probes[0].datagram, asker(), 3_000, 0.0);
        assert_eq!(incumbent.claim(), &Claim::Claimed);

        // The incumbent defends; the newcomer hears that.
        let defence = incumbent.due(3_000);
        assert_eq!(defence.len(), 1);
        newcomer.heard(
            &defence[0].datagram,
            SocketAddr::from((Ipv4Addr::new(169, 254, 3, 4), 5353)),
            3_010,
            0.0,
        );

        assert!(matches!(newcomer.claim(), Claim::Conflicted { .. }));
        assert!(
            newcomer.due(10_000).is_empty(),
            "the newcomer's remaining probes went with its claim"
        );
        assert!(
            incumbent.claim().advertising(),
            "and the robot an operator has already been finding all afternoon still answers"
        );
    }

    #[test]
    fn a_browse_is_not_a_claim() {
        let mut responder = Responder::new("arm-01", "arm-01", 8099, &[V4], 0).unwrap();
        responder.heard(&browse_query(true), asker(), 10, 0.0);
        assert!(
            matches!(responder.claim(), Claim::Probing { .. }),
            "a query with no authority section is somebody looking, not somebody claiming"
        );
    }

    #[test]
    fn a_malformed_datagram_is_counted_and_discarded_whole() {
        let mut responder = claimed();
        responder.heard(&[0xff, 0x00, 0x13], asker(), 2_000, 0.0);
        assert_eq!(responder.counters().malformed, 1);
        assert_eq!(responder.claim(), &Claim::Claimed);
        assert!(responder.due(3_000).is_empty());
    }

    #[test]
    fn the_goodbye_is_the_records_at_ttl_zero() {
        let responder = claimed();
        let farewell = responder.farewell().unwrap();
        let decoded = Message::decode(&farewell.datagram).unwrap();
        assert!(decoded.is_response());
        assert!(
            decoded.answers.iter().all(|record| record.ttl == 0),
            "a goodbye is the same records with nothing left to live"
        );
        assert_eq!(farewell.destination, Destination::Multicast);
    }

    #[test]
    fn what_it_advertises_is_what_a_browse_reads_back() {
        let responder = claimed();
        let announcement = advertisement("arm-01", "arm-01", 8099, &[V4], false)
            .unwrap()
            .encode()
            .unwrap();
        let message = Message::decode(&announcement).unwrap();
        let found = service::read(&message, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "arm-01");
        assert_eq!(found[0].port, 8099);
        assert_eq!(responder.instance(), "arm-01._skynet._tcp.local");
        assert_eq!(responder.addresses(), &[V4]);
    }

    #[test]
    fn next_due_is_what_a_poll_loop_reads_to_know_when_to_come_back() {
        let mut responder = Responder::new("arm-01", "arm-01", 8099, &[V4], 5_000).unwrap();
        assert_eq!(responder.next_due_ms(), Some(5_000));
        let _ = responder.due(5_000);
        assert_eq!(responder.next_due_ms(), Some(5_000 + PROBE_INTERVAL_MS));
        assert_eq!(
            claimed().next_due_ms(),
            None,
            "nothing owed once it has announced"
        );
    }
}

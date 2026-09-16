//! What a Skynet robot advertises, and the rule for what may go in it.
//!
//! # The rule
//!
//! > **A discovery record may carry only what `GET /status` already answers to
//! > [`Role::Anonymous`], and of that, only what is needed to tell two robots apart in a list and
//! > to open a connection to one.**
//!
//! It is derived rather than invented, and the derivation is the point. `skynetd`'s authorisation
//! table (§39.1) gives exactly two routes to nobody in particular — `POST /stop` and
//! `GET /status` — and puts everything else behind a credential. A multicast advertisement is
//! readable by **every host on the link, unauthenticated, unsolicited**. So a key placed in a TXT
//! record is that key published at `Anonymous`, whatever the route that also serves it requires.
//!
//! The obvious first draft of this file put the **device identifier** in the record, because
//! correlating a robot on the bench with a device in a Hub fleet is exactly what an operator wants.
//! `GET /identity` is [`Role::Observer`]. Advertising the identifier would have moved it to
//! anonymous — this crate quietly overruling the daemon's own table from the outside, which is the
//! silent widening that table exists to prevent. It is not there. An operator gets it by
//! connecting, with a credential, as §39.1 intends.
//!
//! # A discovery record carries identity, not state
//!
//! The second cut is about staleness rather than authorisation, and it removes more than the first.
//!
//! mDNS records are cached — by other hosts, by the operating system's resolver, for the TTL the
//! responder set. A record announced when a robot booted is still being served to somebody after
//! the robot has been paired, upgraded, put into `HOLD` or unplugged. So **anything that changes is
//! wrong to advertise**, and the whole of §32.2's discipline applies with more force here than on
//! the console: *"a dashboard that renders a stale torque reading as a live one is worse than a
//! blank panel."* A discovery list rendering a tier, an operating state or a package count from a
//! cached datagram is that failure with a longer cache.
//!
//! What survives both cuts is small, and every field is either a constant or an address:
//!
//! | | |
//! |---|---|
//! | The instance name | The label an operator gave the robot. Already on the wire in the SRV target |
//! | [`KEY_TXTVERS`] | RFC 6763 §6.7's version of the key set. A constant |
//! | [`KEY_API`] | The path the daemon's API is under. A contract, not state |
//! | SRV | Host and port — without which there is nothing to connect to |
//! | A / AAAA | The address |
//!
//! **The OS version is deliberately absent**, and it is the one worth naming because it is the most
//! tempting: it is anonymous on `GET /status`, so the first rule permits it, and it is genuinely
//! useful in a fleet list. It fails the second rule — it changes on every OTA, and a discovery list
//! that shows a robot on the image it had before last night's update is a list that will be trusted
//! and be wrong. It is one request away over a connection this record just made possible.
//!
//! # This is one half of a route
//!
//! [`advertisement`] is what a robot answers a browse *with*. Nothing calls it yet: binding the
//! responder socket belongs to `skynetd`, which is Unix-only. It is defined here, beside the parser
//! that reads it, so that the two halves are one definition rather than two spellings that agree
//! until they do not (§78) — and so that the browse side can be tested against exactly the bytes a
//! robot will send.
//!
//! [`Role::Anonymous`]: https://docs.rs/skynetd-io
//! [`Role::Observer`]: https://docs.rs/skynetd-io

use std::net::{IpAddr, Ipv6Addr};

use crate::txt::Txt;
use crate::wire::{
    Message, Name, Record, RecordData, CLASS_IN, FLAG_AUTHORITATIVE, FLAG_RESPONSE, TYPE_PTR,
};

/// The DNS-SD service type.
///
/// `_skynet`, not `_skynet-os` or `_http`. RFC 6763 §7 caps a service name at fifteen characters
/// and asks for one that names the *protocol*, and the protocol here is the daemon's API — a robot
/// that also served plain HTTP would be a different service on the same host.
pub const SERVICE: &str = "_skynet._tcp.local";

/// RFC 6763 §6.7's key-set version. Present so that a later key set can be recognised rather than
/// guessed at, and first in the record because that section asks for it.
pub const KEY_TXTVERS: &str = "txtvers";

/// The path prefix the daemon's API is served under.
pub const KEY_API: &str = "api";

/// The value of [`KEY_TXTVERS`] this crate writes and understands.
pub const TXTVERS: &str = "1";

/// The API prefix a robot advertises today.
pub const API_PREFIX: &str = "/";

/// The default TTL on an advertised record, in seconds.
///
/// RFC 6763 §10 suggests 75 minutes for records that rarely change. This is two minutes, and the
/// difference is deliberate: a robot is not a printer. It is moved between benches, power-cycled
/// and re-addressed, and the cost of a short TTL is a query every two minutes on a link that is
/// carrying a robot's telemetry anyway.
pub const TTL: u32 = 120;

/// The TTL on a goodbye record (RFC 6762 §10.1).
pub const TTL_GOODBYE: u32 = 0;

/// A robot found on the network.
///
/// Assembled from the records of one response. Every field is something the responder said; there
/// are no defaults, and [`Discovered::endpoint`] returns `None` rather than guessing when what
/// arrived cannot be connected to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    /// The full instance name, `label._skynet._tcp.local`.
    pub instance: Name,
    /// The first label of the instance name — what an operator called this robot.
    pub label: String,
    /// The SRV target: the host name to resolve.
    pub host: Name,
    /// The port the daemon's LAN listener is on.
    pub port: u16,
    /// Addresses from the A and AAAA records in the same response.
    pub addresses: Vec<IpAddr>,
    /// The API path prefix the robot advertised, if it advertised one.
    pub api: Option<String>,
    /// The TXT key-set version, if present.
    pub txtvers: Option<String>,
    /// The interface index the response arrived on.
    ///
    /// Not from the packet — mDNS does not carry it — but from the socket that received it. It is
    /// here because **a link-local IPv6 address without a scope is not an address**: `fe80::1` is
    /// ambiguous across interfaces and connecting to it fails. Since the whole reason this crate
    /// exists is that `serve` binds the console link-local, dropping the scope would reproduce the
    /// bootstrap hole one layer further in.
    pub scope_id: Option<u32>,
    /// Whether this response withdrew the instance (RFC 6762's goodbye, a TTL of zero).
    pub goodbye: bool,
}

impl Discovered {
    /// An address that can actually be connected to, preferring IPv4 for its lack of scoping
    /// problems, then a scoped link-local, then anything else.
    ///
    /// `None` when the only addresses on offer are link-local IPv6 and no scope is known — which is
    /// an honest refusal rather than a connection attempt that will fail with a confusing error.
    pub fn endpoint(&self) -> Option<(IpAddr, u16)> {
        let usable = |address: &IpAddr| match address {
            IpAddr::V4(_) => true,
            IpAddr::V6(v6) => !is_unicast_link_local(v6) || self.scope_id.is_some(),
        };
        let best = self
            .addresses
            .iter()
            .find(|address| matches!(address, IpAddr::V4(_)))
            .or_else(|| self.addresses.iter().find(|address| usable(address)))?;
        Some((*best, self.port))
    }

    /// The base URL of this robot's API, if it has a usable address.
    pub fn url(&self) -> Option<String> {
        let (address, port) = self.endpoint()?;
        let authority = match address {
            IpAddr::V4(v4) => format!("{v4}:{port}"),
            IpAddr::V6(v6) if is_unicast_link_local(&v6) => {
                // A scope is required here, and `endpoint` has already refused the case where
                // there is none.
                format!("[{v6}%{}]:{port}", self.scope_id.unwrap_or(0))
            }
            IpAddr::V6(v6) => format!("[{v6}]:{port}"),
        };
        let prefix = self.api.as_deref().unwrap_or(API_PREFIX);
        Some(format!("http://{authority}{prefix}"))
    }
}

/// `fe80::/10`, without waiting for `Ipv6Addr::is_unicast_link_local` to stabilise.
fn is_unicast_link_local(address: &Ipv6Addr) -> bool {
    let [a, b, ..] = address.octets();
    a == 0xFE && (b & 0xC0) == 0x80
}

/// The question a browse asks.
///
/// `unicast_response` is RFC 6762 §5.4's QU bit, and which value is right depends entirely on
/// whether the asker holds port 5353:
///
/// * **`true`** for [`crate::browse`], which cannot hold it — `SO_REUSEPORT` is not reachable from
///   `std::net` and the port is permanently held by `mDNSResponder` on macOS and by Avahi on most
///   Linux. That module's header has the whole argument and what it costs.
/// * **`false`** for anything that does hold the port, because a multicast reply is seen by every
///   browser on the link and a second operator looking for the same robot learns from the first
///   one's question.
///
/// It is a parameter rather than a constant so that the choice is made by the caller that knows
/// which of those it is, instead of being a default one of them silently gets wrong.
pub fn query(unicast_response: bool) -> Message {
    Message {
        id: 0,
        flags: 0,
        questions: vec![crate::wire::Question {
            name: Name::parse(SERVICE),
            qtype: TYPE_PTR,
            unicast_response,
        }],
        ..Default::default()
    }
}

/// The records a robot answers a browse with.
///
/// `label` is the instance name and `host` the robot's own name; both are appended to `.local`.
/// Returns `None` if the TXT record cannot be encoded, which for this key set cannot happen unless
/// a caller passes a pathological API prefix — and a responder with no record to send should send
/// nothing rather than an advertisement missing the field that says how to talk to it.
pub fn advertisement(
    label: &str,
    host: &str,
    port: u16,
    addresses: &[IpAddr],
    goodbye: bool,
) -> Option<Message> {
    let instance = Name::parse(&format!("{label}.{SERVICE}"));
    let host_name = Name::parse(&format!("{host}.local"));
    let ttl = if goodbye { TTL_GOODBYE } else { TTL };

    let mut txt = Txt::new();
    txt.set(KEY_TXTVERS, TXTVERS).set(KEY_API, API_PREFIX);

    // The shared records are cache-flush; the PTR is not. RFC 6762 §10.2: the PTR enumerates a
    // service and several instances legitimately share the name, so flushing it would have one
    // robot's announcement evict another's.
    let mut answers = vec![Record {
        name: Name::parse(SERVICE),
        class: CLASS_IN,
        cache_flush: false,
        ttl,
        data: RecordData::Ptr(instance.clone()),
    }];

    answers.push(Record {
        name: instance.clone(),
        class: CLASS_IN,
        cache_flush: true,
        ttl,
        data: RecordData::Srv {
            priority: 0,
            weight: 0,
            port,
            target: host_name.clone(),
        },
    });

    answers.push(Record {
        name: instance,
        class: CLASS_IN,
        cache_flush: true,
        ttl,
        data: RecordData::Txt(txt.encode()?),
    });

    for address in addresses {
        answers.push(Record {
            name: host_name.clone(),
            class: CLASS_IN,
            cache_flush: true,
            ttl,
            data: match address {
                IpAddr::V4(v4) => RecordData::A(*v4),
                IpAddr::V6(v6) => RecordData::Aaaa(*v6),
            },
        });
    }

    Some(Message {
        id: 0,
        flags: FLAG_RESPONSE | FLAG_AUTHORITATIVE,
        questions: Vec::new(),
        answers,
        authorities: Vec::new(),
        additionals: Vec::new(),
    })
}

/// Pull every Skynet instance out of one response.
///
/// The rule that shapes this: **an instance with no SRV is not reported.** A PTR and a TXT describe
/// a robot that exists and give no way to reach it, and a discovery tool listing a robot an
/// operator then cannot open is worse than one that lists nothing — it sends them looking for a
/// network fault that is really a partial response. Responders scatter records between the answer
/// and additional sections at their discretion, so both are read (see [`Message::records`]).
pub fn read(message: &Message, scope_id: Option<u32>) -> Vec<Discovered> {
    if !message.is_response() {
        return Vec::new();
    }
    let service = Name::parse(SERVICE);
    let mut found = Vec::new();

    for record in message.records() {
        let RecordData::Srv { port, target, .. } = &record.data else {
            continue;
        };
        if !record.name.ends_with(&service) || record.name.0.len() <= service.0.len() {
            continue;
        }

        let label = record
            .name
            .first_label()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
            .unwrap_or_default();

        let txt = message
            .records()
            .filter(|other| other.name.eq_ignore_case(&record.name))
            .find_map(Txt::of);

        let addresses = message
            .records()
            .filter(|other| other.name.eq_ignore_case(target))
            .filter_map(|other| match &other.data {
                RecordData::A(v4) => Some(IpAddr::V4(*v4)),
                RecordData::Aaaa(v6) => Some(IpAddr::V6(*v6)),
                _ => None,
            })
            .collect();

        found.push(Discovered {
            instance: record.name.clone(),
            label,
            host: target.clone(),
            port: *port,
            addresses,
            api: txt
                .as_ref()
                .and_then(|txt| txt.get_str(KEY_API))
                .map(str::to_owned),
            txtvers: txt
                .as_ref()
                .and_then(|txt| txt.get_str(KEY_TXTVERS))
                .map(str::to_owned),
            scope_id,
            // A goodbye is a zero TTL on the record that withdraws the instance.
            goodbye: record.ttl == 0,
        });
    }

    found
}

/// Whether a message is a query this robot should answer: a `PTR` question for the service name, or
/// an `ANY` question — both of which RFC 6763 browsers send.
///
/// # The responder does not call this, and cannot
///
/// It said *"for the responder half"* until 2026-09-16, which was true when it was written and had
/// stopped being so. [`crate::responder::Responder`] has to know **which** question matched — a
/// shared record, a unique one, or one asking for a unicast answer, because §82.4 answers the three
/// differently — so it asks its own per-question predicate and never the whole-message form. One
/// `bool` over a message with several questions in it is a decision the responder has no way to act
/// on, and no amount of wiring changes that: what would be needed is a different return type, at
/// which point this is a different function.
///
/// So it is kept as the vocabulary a caller *outside* the responder wants — a monitor deciding
/// whether a captured packet was aimed at this robot at all — and not as a step the responder skips.
pub fn asks_for_us(message: &Message) -> bool {
    if message.is_response() {
        return false;
    }
    let service = Name::parse(SERVICE);
    message.questions.iter().any(|question| {
        question.name.eq_ignore_case(&service)
            && (question.qtype == TYPE_PTR || question.qtype == crate::wire::TYPE_ANY)
    })
}

/// The addresses and port mDNS itself runs on.
pub mod endpoint {
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// The mDNS port.
    pub const PORT: u16 = 5353;
    /// The IPv4 multicast group.
    pub const GROUP_V4: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
    /// The IPv6 multicast group.
    pub const GROUP_V6: Ipv6Addr = Ipv6Addr::new(0xFF02, 0, 0, 0, 0, 0, 0, 0x00FB);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wire::CACHE_FLUSH;
    use std::net::Ipv4Addr;

    const V4: IpAddr = IpAddr::V4(Ipv4Addr::new(169, 254, 3, 4));

    fn link_local_v6() -> IpAddr {
        "fe80::a1b2:c3d4:e5f6:708".parse().unwrap()
    }

    fn advert() -> Message {
        advertisement("arm-01", "arm-01", 8099, &[V4], false).unwrap()
    }

    // ── The rule ──

    #[test]
    fn the_record_carries_no_key_beyond_the_two_this_module_defines() {
        // The guard on the rule in this module's header. A key added without an argument for why
        // it is safe at `Anonymous` and stable over a TTL fails here first.
        let message = advert();
        let txt = message.records().find_map(Txt::of).unwrap();
        let keys: Vec<String> = txt
            .keys()
            .map(|key| String::from_utf8_lossy(key).into_owned())
            .collect();
        assert_eq!(keys, vec![KEY_TXTVERS, KEY_API]);
    }

    #[test]
    fn no_advertised_field_names_a_device_identifier_or_a_state() {
        // `GET /identity` is Observer and `GET /status`'s state fields change; neither belongs in
        // a datagram every host on the link can read and cache.
        let bytes = advert().encode().unwrap();
        let text = String::from_utf8_lossy(&bytes).to_lowercase();
        for forbidden in ["id=", "tier", "state", "paired", "os_version", "guard"] {
            assert!(
                !text.contains(forbidden),
                "the advertisement contains {forbidden:?}"
            );
        }
    }

    // ── Round trip, which is what makes the browse side testable against real bytes ──

    #[test]
    fn a_robots_advertisement_reads_back_as_that_robot() {
        let message = Message::decode(&advert().encode().unwrap()).unwrap();
        let found = read(&message, None);
        assert_eq!(found.len(), 1);
        let robot = &found[0];
        assert_eq!(robot.label, "arm-01");
        assert_eq!(robot.port, 8099);
        assert_eq!(robot.addresses, vec![V4]);
        assert_eq!(robot.api.as_deref(), Some(API_PREFIX));
        assert_eq!(robot.txtvers.as_deref(), Some(TXTVERS));
        assert!(!robot.goodbye);
        assert_eq!(robot.url().as_deref(), Some("http://169.254.3.4:8099/"));
    }

    #[test]
    fn two_robots_in_one_response_are_two_robots() {
        let mut message = advert();
        let second = advertisement("arm-02", "arm-02", 8099, &[V4], false).unwrap();
        message.answers.extend(second.answers);
        let found = read(&Message::decode(&message.encode().unwrap()).unwrap(), None);
        let labels: Vec<&str> = found.iter().map(|robot| robot.label.as_str()).collect();
        assert_eq!(labels, vec!["arm-01", "arm-02"]);
    }

    #[test]
    fn a_goodbye_is_reported_rather_than_filtered_out() {
        let message = advertisement("arm-01", "arm-01", 8099, &[V4], true).unwrap();
        let found = read(&Message::decode(&message.encode().unwrap()).unwrap(), None);
        assert!(
            found[0].goodbye,
            "a robot leaving the network is worth saying"
        );
    }

    // ── What is not reported, and why ──

    #[test]
    fn an_instance_with_no_srv_is_not_reported_because_it_cannot_be_opened() {
        let mut message = advert();
        message
            .answers
            .retain(|record| !matches!(record.data, RecordData::Srv { .. }));
        assert!(read(&message, None).is_empty());
    }

    #[test]
    fn a_query_is_not_read_as_an_answer() {
        assert!(read(&query(true), None).is_empty());
    }

    #[test]
    fn another_services_instance_on_the_same_link_is_ignored() {
        let mut message = advert();
        message.answers.push(Record {
            name: Name::parse("printer._ipp._tcp.local"),
            class: CLASS_IN,
            cache_flush: true,
            ttl: TTL,
            data: RecordData::Srv {
                priority: 0,
                weight: 0,
                port: 631,
                target: Name::parse("printer.local"),
            },
        });
        let found = read(&message, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "arm-01");
    }

    #[test]
    fn the_service_name_itself_is_not_an_instance_of_the_service() {
        let mut message = advert();
        message.answers.push(Record {
            name: Name::parse(SERVICE),
            class: CLASS_IN,
            cache_flush: true,
            ttl: TTL,
            data: RecordData::Srv {
                priority: 0,
                weight: 0,
                port: 1,
                target: Name::parse("nowhere.local"),
            },
        });
        assert_eq!(read(&message, None).len(), 1);
    }

    // ── The scope, which is the whole reason this crate exists ──

    #[test]
    fn a_link_local_v6_address_with_no_scope_is_refused_rather_than_offered() {
        let message = advertisement("arm-01", "arm-01", 8099, &[link_local_v6()], false).unwrap();
        let found = read(&Message::decode(&message.encode().unwrap()).unwrap(), None);
        assert_eq!(found[0].endpoint(), None);
        assert_eq!(found[0].url(), None);
    }

    #[test]
    fn the_same_address_with_a_scope_is_a_url_that_can_be_opened() {
        let message = advertisement("arm-01", "arm-01", 8099, &[link_local_v6()], false).unwrap();
        let found = read(
            &Message::decode(&message.encode().unwrap()).unwrap(),
            Some(7),
        );
        assert_eq!(
            found[0].url().as_deref(),
            Some("http://[fe80::a1b2:c3d4:e5f6:708%7]:8099/")
        );
    }

    #[test]
    fn ipv4_is_preferred_because_it_has_no_scoping_problem() {
        let message =
            advertisement("arm-01", "arm-01", 8099, &[link_local_v6(), V4], false).unwrap();
        let found = read(
            &Message::decode(&message.encode().unwrap()).unwrap(),
            Some(7),
        );
        assert_eq!(found[0].endpoint(), Some((V4, 8099)));
    }

    #[test]
    fn a_routable_v6_address_needs_no_scope() {
        let global: IpAddr = "2001:db8::1".parse().unwrap();
        let message = advertisement("arm-01", "arm-01", 8099, &[global], false).unwrap();
        let found = read(&Message::decode(&message.encode().unwrap()).unwrap(), None);
        assert_eq!(
            found[0].url().as_deref(),
            Some("http://[2001:db8::1]:8099/")
        );
    }

    // ── The responder's side of the question ──

    #[test]
    fn a_browse_for_this_service_is_recognised_and_another_services_is_not() {
        assert!(asks_for_us(&query(true)));

        let mut other = query(false);
        other.questions[0].name = Name::parse("_ipp._tcp.local");
        assert!(!asks_for_us(&other));
    }

    #[test]
    fn an_any_question_is_answered_because_browsers_send_them() {
        let mut any = query(true);
        any.questions[0].qtype = crate::wire::TYPE_ANY;
        assert!(asks_for_us(&any));
    }

    #[test]
    fn a_response_is_never_mistaken_for_a_question_to_answer() {
        assert!(!asks_for_us(&advert()));
    }

    #[test]
    fn the_ptr_is_not_cache_flush_because_instances_share_it() {
        let message = advert();
        let ptr = message
            .records()
            .find(|record| matches!(record.data, RecordData::Ptr(_)))
            .unwrap();
        assert!(
            !ptr.cache_flush,
            "flushing the PTR would have one robot evict another"
        );
        // Everything else is, since each is one instance's own.
        for record in message
            .records()
            .filter(|record| !matches!(record.data, RecordData::Ptr(_)))
        {
            assert!(record.cache_flush);
        }
    }

    #[test]
    fn the_cache_flush_constant_is_the_class_bit_and_not_part_of_the_class() {
        assert_eq!(CACHE_FLUSH, 0x8000);
        assert_eq!(CLASS_IN, 1);
    }
}

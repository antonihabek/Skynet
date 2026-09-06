//! A one-shot browse: ask the link which robots are on it, and wait.
//!
//! # Why this asks for a unicast reply, which is not the obvious choice
//!
//! The textbook mDNS client binds port **5353**, joins the multicast group, and receives every
//! announcement on the link including unsolicited ones. It is the better design and this is not it,
//! for a reason that is worth stating plainly rather than discovering later:
//!
//! **binding 5353 requires `SO_REUSEPORT`, and `std::net` does not expose it.** On macOS the port is
//! permanently held by `mDNSResponder`, and on any Linux running Avahi it is held too, so a plain
//! `UdpSocket::bind` on 5353 fails on the two platforms this tool is for. Setting the option needs
//! either `unsafe` and a raw `setsockopt` or a dependency, and this crate has ruled out both — the
//! first by `forbid(unsafe_code)`, the second by the workspace.
//!
//! So the query sets RFC 6762 §5.4's **QU bit**, asking responders to reply directly, and this
//! listens on an ephemeral port. That is a legitimate mode of the protocol rather than a
//! workaround: §5.4 exists precisely for a client that cannot hold the well-known port, and a
//! responder is required to honour it for a first query.
//!
//! **What it costs, exactly:**
//!
//! * **unsolicited announcements are not seen.** A robot that boots thirty seconds after a browse
//!   is not in that browse's results. This is why [`crate::browse`] is a question with a deadline
//!   and there is no watch mode — a watch built on this transport would silently miss arrivals,
//!   which is a worse thing to offer than no watch at all.
//! * **a responder that ignores QU is missed.** Conformant ones do not; this is named because it is
//!   the failure that would look like a robot being absent.
//!
//! Both are removed by the same change — a responder in `skynetd` and a socket that holds 5353 —
//! and that change needs a platform layer this crate has deliberately not grown.
//!
//! # One interface, and it says so
//!
//! Sending to a multicast group sends it out the interface the routing table picks. Reaching a
//! robot on a *second* adapter means sending once per interface, which needs interface enumeration
//! that `std::net` also does not provide. [`Outcome::interfaces_asked`] is therefore 1 or 2 — the
//! IPv4 socket and the IPv6 one — never "all of them", and a caller rendering a robot count should
//! not describe it as the network's.
//!
//! # Not answering is not the same as not being asked
//!
//! [`browse`] returns [`Outcome`] on success and [`BrowseError`] when it could not ask at all. On a
//! laptop with a VPN filtering multicast, or a firewall prompt nobody clicked, those two are
//! indistinguishable from the results alone and only one of them is a statement about robots. The
//! error is not folded into an empty list.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use crate::service::{self, endpoint, Discovered};
use crate::wire::{Message, MAX_MESSAGE};

/// How long a browse waits by default.
///
/// RFC 6762 §5.2 asks a browser to wait at least a second before concluding a service is absent, and
/// responders delay a reply by up to 120 ms to avoid colliding on the link. Two seconds is
/// comfortably past both and short enough to sit behind a button.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// mDNS requires a TTL of 255 so that a packet is confined to the link (RFC 6762 §11).
const MULTICAST_TTL: u32 = 255;

/// Why a browse could not be performed.
#[derive(Debug)]
pub enum BrowseError {
    /// Neither socket could be opened.
    Bind(io::Error),
    /// The query could not be encoded — a defect here, not a network condition.
    Encode(crate::wire::WireError),
    /// Sockets opened and nothing could be sent on any of them.
    ///
    /// The ordinary cause is a firewall or a VPN filtering multicast, which is exactly the case
    /// that must not be reported as "no robots found".
    Send(io::Error),
}

impl std::fmt::Display for BrowseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind(error) => write!(f, "no multicast socket could be opened: {error}"),
            Self::Encode(error) => write!(f, "the query could not be encoded: {error}"),
            Self::Send(error) => write!(
                f,
                "the query could not be sent — a firewall or VPN filtering multicast is the usual \
                 cause, and this is not a report that no robot is present: {error}"
            ),
        }
    }
}

impl std::error::Error for BrowseError {}

/// What a browse found, and how well it was able to look.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// The robots that answered, one entry per instance name.
    pub robots: Vec<Discovered>,
    /// How many sockets the query actually went out on: the IPv4 one, the IPv6 one, or both.
    ///
    /// Never the number of network interfaces. See this module's header.
    pub interfaces_asked: usize,
    /// Datagrams that arrived and could not be parsed.
    ///
    /// Reported rather than discarded silently. A link with a broken responder on it produces a
    /// steady count here, and an operator staring at an empty robot list deserves to know the
    /// difference between a quiet network and a noisy one.
    pub malformed: usize,
    /// Datagrams that parsed and were about some other service.
    pub other_services: usize,
}

/// Browse with [`DEFAULT_TIMEOUT`].
pub fn browse() -> Result<Outcome, BrowseError> {
    browse_for(DEFAULT_TIMEOUT)
}

/// Browse, waiting `timeout` for replies.
pub fn browse_for(timeout: Duration) -> Result<Outcome, BrowseError> {
    // QU: see this module's header. Not a preference — the only mode available without a socket
    // option `std` does not expose.
    let query = service::query(true).encode().map_err(BrowseError::Encode)?;

    let mut sockets = Vec::new();
    let mut last_bind_error = None;

    match open_v4() {
        Ok(socket) => sockets.push((
            socket,
            SocketAddr::from((endpoint::GROUP_V4, endpoint::PORT)),
        )),
        Err(error) => last_bind_error = Some(error),
    }
    match open_v6() {
        Ok(socket) => sockets.push((
            socket,
            SocketAddr::from((endpoint::GROUP_V6, endpoint::PORT)),
        )),
        Err(error) => last_bind_error = Some(error),
    }

    if sockets.is_empty() {
        return Err(BrowseError::Bind(
            last_bind_error.unwrap_or_else(|| io::Error::other("no socket could be opened")),
        ));
    }

    let mut outcome = Outcome::default();
    let mut last_send_error = None;
    let mut live = Vec::new();

    for (socket, group) in sockets {
        match socket.send_to(&query, group) {
            Ok(_) => {
                outcome.interfaces_asked += 1;
                live.push(socket);
            }
            Err(error) => last_send_error = Some(error),
        }
    }

    if live.is_empty() {
        return Err(BrowseError::Send(
            last_send_error.unwrap_or_else(|| io::Error::other("the query was not sent")),
        ));
    }

    let deadline = Instant::now() + timeout;
    let mut buffer = vec![0u8; MAX_MESSAGE];

    // Round-robin across the sockets so a silent IPv6 socket cannot spend the whole budget. Each
    // read gets a slice of what is left rather than the whole of it.
    while let Some(remaining) = deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
    {
        let slice = (remaining / live.len() as u32).max(Duration::from_millis(20));
        let mut heard = false;

        for socket in &live {
            // A zero timeout means "block for ever" to the platform, which is the one value that
            // must never be set here.
            let _ = socket.set_read_timeout(Some(slice.max(Duration::from_millis(1))));
            match socket.recv_from(&mut buffer) {
                Ok((len, from)) => {
                    heard = true;
                    absorb(&mut outcome, &buffer[..len], from);
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                // A datagram larger than the buffer, or an ICMP error surfacing on the socket.
                // Neither is a reason to abandon a browse that other responders may still answer.
                Err(_) => {}
            }
        }

        if !heard && Instant::now() >= deadline {
            break;
        }
    }

    Ok(outcome)
}

/// Fold one datagram into the outcome.
///
/// Separated from the socket loop so that it is testable without a network, which is most of what
/// there is to get wrong here.
pub(crate) fn absorb(outcome: &mut Outcome, datagram: &[u8], from: SocketAddr) {
    let Ok(message) = Message::decode(datagram) else {
        outcome.malformed += 1;
        return;
    };

    // The scope index comes from the sender's address, not from the packet: mDNS carries no zone
    // and a link-local IPv6 address without one cannot be connected to. See `Discovered::scope_id`.
    let scope = match from {
        SocketAddr::V6(v6) if v6.scope_id() != 0 => Some(v6.scope_id()),
        _ => None,
    };

    let found = service::read(&message, scope);
    if found.is_empty() {
        if message.is_response() {
            outcome.other_services += 1;
        }
        return;
    }

    for mut robot in found {
        // A responder that answered from an address it did not advertise is still reachable at the
        // address it answered from, and on a robot with a half-configured hostname that may be the
        // only one that works.
        let sender = from.ip();
        if !robot.addresses.contains(&sender) && is_plausible(&sender) {
            robot.addresses.push(sender);
        }

        match outcome
            .robots
            .iter_mut()
            .find(|seen| seen.instance.eq_ignore_case(&robot.instance))
        {
            // The same robot answering on both sockets is one robot with two addresses.
            Some(seen) => {
                for address in robot.addresses {
                    if !seen.addresses.contains(&address) {
                        seen.addresses.push(address);
                    }
                }
                seen.scope_id = seen.scope_id.or(robot.scope_id);
                // A goodbye is never un-said by a later record in the same browse.
                seen.goodbye |= robot.goodbye;
            }
            None => outcome.robots.push(robot),
        }
    }
}

/// Whether an address is worth adding to a robot's list.
///
/// The unspecified and loopback addresses are what a misconfigured stack reports and neither is a
/// robot; a multicast source address is not a source address at all.
fn is_plausible(address: &IpAddr) -> bool {
    match address {
        IpAddr::V4(v4) => !v4.is_unspecified() && !v4.is_multicast() && !v4.is_broadcast(),
        IpAddr::V6(v6) => !v6.is_unspecified() && !v6.is_multicast(),
    }
}

fn open_v4() -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)))?;
    // Not fatal if the platform refuses either: the query still goes out, and a TTL the platform
    // chose is a link-local packet either way on every stack that matters.
    let _ = socket.set_multicast_ttl_v4(MULTICAST_TTL);
    // Loopback on, so a responder on this same machine is visible. That is how the responder half
    // will be tested, and turning it off to save one datagram would make the test impossible.
    let _ = socket.set_multicast_loop_v4(true);
    Ok(socket)
}

fn open_v6() -> io::Result<UdpSocket> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)))?;
    let _ = socket.set_multicast_loop_v6(true);
    Ok(socket)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::advertisement;

    const V4: IpAddr = IpAddr::V4(Ipv4Addr::new(169, 254, 3, 4));

    fn from_v4() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(169, 254, 3, 4), endpoint::PORT))
    }

    fn advert(label: &str, addresses: &[IpAddr]) -> Vec<u8> {
        advertisement(label, label, 8099, addresses, false)
            .unwrap()
            .encode()
            .unwrap()
    }

    #[test]
    fn a_robots_answer_becomes_a_robot() {
        let mut outcome = Outcome::default();
        absorb(&mut outcome, &advert("arm-01", &[V4]), from_v4());
        assert_eq!(outcome.robots.len(), 1);
        assert_eq!(outcome.robots[0].label, "arm-01");
        assert_eq!(outcome.malformed, 0);
    }

    #[test]
    fn the_same_robot_answering_twice_is_one_robot_with_both_addresses() {
        let other: IpAddr = "fe80::1".parse().unwrap();
        let mut outcome = Outcome::default();
        absorb(&mut outcome, &advert("arm-01", &[V4]), from_v4());
        absorb(
            &mut outcome,
            &advert("arm-01", &[other]),
            SocketAddr::from((Ipv4Addr::new(169, 254, 3, 4), endpoint::PORT)),
        );
        assert_eq!(outcome.robots.len(), 1, "one instance name is one robot");
        assert!(outcome.robots[0].addresses.contains(&V4));
        assert!(outcome.robots[0].addresses.contains(&other));
    }

    #[test]
    fn two_robots_are_two_robots() {
        let mut outcome = Outcome::default();
        absorb(&mut outcome, &advert("arm-01", &[V4]), from_v4());
        absorb(&mut outcome, &advert("arm-02", &[V4]), from_v4());
        assert_eq!(outcome.robots.len(), 2);
    }

    #[test]
    fn a_goodbye_is_not_undone_by_another_record_in_the_same_browse() {
        let leaving = advertisement("arm-01", "arm-01", 8099, &[V4], true)
            .unwrap()
            .encode()
            .unwrap();
        let mut outcome = Outcome::default();
        absorb(&mut outcome, &leaving, from_v4());
        absorb(&mut outcome, &advert("arm-01", &[V4]), from_v4());
        assert!(outcome.robots[0].goodbye);
    }

    #[test]
    fn the_address_a_robot_answered_from_is_kept_even_if_it_advertised_another() {
        let mut outcome = Outcome::default();
        let elsewhere: IpAddr = "169.254.9.9".parse().unwrap();
        absorb(&mut outcome, &advert("arm-01", &[elsewhere]), from_v4());
        assert!(
            outcome.robots[0].addresses.contains(&V4),
            "a robot is reachable where it answered from"
        );
    }

    #[test]
    fn an_implausible_source_address_is_not_added() {
        let mut outcome = Outcome::default();
        absorb(
            &mut outcome,
            &advert("arm-01", &[V4]),
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, endpoint::PORT)),
        );
        assert_eq!(outcome.robots[0].addresses, vec![V4]);
    }

    #[test]
    fn a_scope_is_taken_from_the_sender_because_the_packet_does_not_carry_one() {
        let link_local: IpAddr = "fe80::a1b2".parse().unwrap();
        let mut outcome = Outcome::default();
        let from = SocketAddr::V6(std::net::SocketAddrV6::new(
            "fe80::a1b2".parse().unwrap(),
            endpoint::PORT,
            0,
            7,
        ));
        absorb(&mut outcome, &advert("arm-01", &[link_local]), from);
        assert_eq!(outcome.robots[0].scope_id, Some(7));
        assert!(
            outcome.robots[0].url().is_some(),
            "scoped, so it can be opened"
        );
    }

    // ── The counters, which are what an operator reads when the list is empty ──

    #[test]
    fn a_malformed_datagram_is_counted_rather_than_discarded_silently() {
        let mut outcome = Outcome::default();
        absorb(&mut outcome, &[0xFF; 8], from_v4());
        assert_eq!(outcome.malformed, 1);
        assert!(outcome.robots.is_empty());
    }

    #[test]
    fn another_services_answer_is_counted_separately_from_a_broken_one() {
        // A well-formed response about a printer. An empty robot list on a busy link and an empty
        // robot list on a silent one are different diagnoses.
        let mut printer = Message {
            flags: crate::wire::FLAG_RESPONSE,
            ..Default::default()
        };
        printer.answers.push(crate::wire::Record {
            name: crate::wire::Name::parse("printer._ipp._tcp.local"),
            class: crate::wire::CLASS_IN,
            cache_flush: true,
            ttl: 120,
            data: crate::wire::RecordData::Srv {
                priority: 0,
                weight: 0,
                port: 631,
                target: crate::wire::Name::parse("printer.local"),
            },
        });
        let mut outcome = Outcome::default();
        absorb(&mut outcome, &printer.encode().unwrap(), from_v4());
        assert_eq!(outcome.other_services, 1);
        assert_eq!(outcome.malformed, 0);
    }

    #[test]
    fn our_own_query_coming_back_on_the_loopback_is_neither() {
        // Multicast loopback is on, so a browse hears itself. It is a question, not a response, and
        // counting it as another service would make every browse report traffic that was its own.
        let mut outcome = Outcome::default();
        absorb(
            &mut outcome,
            &service::query(true).encode().unwrap(),
            from_v4(),
        );
        assert_eq!(outcome.other_services, 0);
        assert_eq!(outcome.malformed, 0);
        assert!(outcome.robots.is_empty());
    }

    #[test]
    fn absorbing_arbitrary_bytes_never_panics() {
        let mut outcome = Outcome::default();
        for len in 0..64usize {
            let datagram: Vec<u8> = (0..len).map(|i| (i * 37) as u8).collect();
            absorb(&mut outcome, &datagram, from_v4());
        }
    }

    // ── The socket layer, as far as it can be exercised without a robot ──

    #[test]
    fn a_browse_can_open_a_socket_and_send_on_this_machine() {
        // Deliberately not asserting that anything answers: nothing on a CI runner advertises this
        // service, and a test that required one would be a test that fails for being right.
        match browse_for(Duration::from_millis(150)) {
            Ok(outcome) => assert!(
                outcome.interfaces_asked >= 1,
                "a successful browse asked on at least one socket"
            ),
            // A sandboxed runner with no multicast route is a real environment and not a defect
            // here; what must not happen is a panic or a hang.
            Err(error) => {
                let _ = error.to_string();
            }
        }
    }

    #[test]
    fn a_browse_respects_its_deadline() {
        let started = Instant::now();
        let _ = browse_for(Duration::from_millis(200));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a browse must not outlast its timeout by an order of magnitude"
        );
    }
}

//! §82's responder, on the machine it describes.
//!
//! `skynet-mdns` decides *what* a robot answers a browse with and *when*; this binds the socket it
//! goes out on. The split is §82.4's own: the wire format belongs beside the parser that reads it,
//! and port 5353, a multicast group and an interface belong to the daemon.
//!
//! Until this existed, [`skynet_mdns::browse`] asked and nothing answered — and §79 is this
//! repository's record of what half a route costs: *"the address answered, the sender backed off on
//! §34.4's curve, no bytes moved, and the failure was indistinguishable from patience."*
//!
//! # Reported, not fatal
//!
//! A robot that cannot advertise is a robot somebody has to find another way. A robot that will not
//! start because it could not advertise is one nobody can reach at all — so every failure here is
//! printed and stepped over, which is the same call `skynetd` makes about a recorder it cannot open
//! (§33.1: a supervisor that cannot record is not a robot that cannot stop).
//!
//! The one that will actually happen is **port 5353 already held**. On a developer's Linux box
//! Avahi holds it and on macOS `mDNSResponder` does, permanently; taking it alongside them needs
//! `SO_REUSEPORT`, which `std::net` does not expose and which this workspace will not reach for
//! through `unsafe` or a dependency (§82.3 makes the same argument on the browse side). On a robot
//! image nothing else is listening, so the responder binds — and on a laptop it says clearly that
//! it did not, rather than advertising into a socket it never got.
//!
//! # It advertises the console, or it does not run
//!
//! The SRV record's port is the console's, and the address records are the address §34.2 bound it
//! to. So a daemon started without `--lan` starts no responder: an advertisement is an invitation
//! to open a page, and a robot that answered a browse while serving nothing on the LAN would send
//! an operator to a closed port and look like a network fault.
//!
//! It never enumerates interfaces. `getifaddrs` is not reachable from `std`, and the address to
//! advertise is not a matter of discovery anyway: it is the one address the console is bound to,
//! and a record naming a second interface would be a claim this robot does not serve there.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

use skynet_mdns::responder::{Claim, Destination, Responder};
use skynet_mdns::service::endpoint;
use skynet_mdns::wire::MAX_MESSAGE;

/// mDNS requires a TTL of 255 so that a packet is confined to the link (RFC 6762 §11).
const MULTICAST_TTL: u32 = 255;

/// One socket the responder can speak on.
///
/// A trait for the reason [`crate::launch::Launcher`] is one: the states worth testing — a link
/// that refuses every send, a datagram from a robot that is not there, a query arriving mid-probe —
/// are not states a real socket reaches on purpose.
pub trait Link {
    /// Take one datagram, or [`io::ErrorKind::WouldBlock`] when there is none.
    fn recv_from(&self, buffer: &mut [u8]) -> io::Result<(usize, SocketAddr)>;

    /// Send one datagram.
    fn send_to(&self, datagram: &[u8], to: SocketAddr) -> io::Result<usize>;

    /// Where a multicast datagram goes on this link.
    fn group(&self) -> SocketAddr;
}

/// A real multicast socket.
#[derive(Debug)]
pub struct Multicast {
    socket: UdpSocket,
    group: SocketAddr,
}

impl Link for Multicast {
    fn recv_from(&self, buffer: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
        self.socket.recv_from(buffer)
    }

    fn send_to(&self, datagram: &[u8], to: SocketAddr) -> io::Result<usize> {
        self.socket.send_to(datagram, to)
    }

    fn group(&self) -> SocketAddr {
        self.group
    }
}

/// Why the responder could not be started.
#[derive(Debug)]
pub enum OpenError {
    /// No socket could be bound on either family.
    ///
    /// Nearly always port 5353 held by a system responder. See the module header.
    Bind(io::Error),
    /// The advertisement could not be built, which is a defect rather than a network condition.
    Advertisement,
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bind(error) => write!(
                f,
                "no mDNS socket could be bound on port {}: {error}. A system responder (Avahi, \
                 mDNSResponder) already holding the port is the usual cause, and taking it \
                 alongside one needs SO_REUSEPORT, which std does not expose (§82.3). The console \
                 is unaffected; it is discovery that is off",
                endpoint::PORT
            ),
            Self::Advertisement => write!(
                f,
                "the advertisement could not be encoded, which for this key set means a label no \
                 DNS name can carry — rename the robot"
            ),
        }
    }
}

impl std::error::Error for OpenError {}

/// What the responder has done, for a caller that has to say whether discovery is working.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    /// Datagrams sent — answers, probes, announcements.
    pub sent: usize,
    /// Sends that failed. A link with multicast filtered produces a steady count here.
    pub send_failures: usize,
    /// Datagrams received, before any of them were understood.
    pub received: usize,
}

/// §82's responder with sockets under it.
#[derive(Debug)]
pub struct Discovery<L: Link> {
    links: Vec<L>,
    responder: Responder,
    activity: Activity,
    buffer: Vec<u8>,
}

impl Discovery<Multicast> {
    /// Bind the mDNS sockets and start probing for `label`.
    ///
    /// `console` is the address §34.2 bound the console to; its IP is what gets advertised and its
    /// port is what the SRV record carries.
    pub fn open(
        label: &str,
        host: &str,
        console: SocketAddr,
        now_ms: u64,
    ) -> Result<Self, OpenError> {
        let responder = Responder::new(label, host, console.port(), &[console.ip()], now_ms)
            .ok_or(OpenError::Advertisement)?;

        let mut links = Vec::new();
        let mut last = None;

        // IPv4 first, deliberately. On a Linux with `bindv6only` off — the default — an IPv6
        // socket on 5353 takes the IPv4 port with it, and the IPv4 bind then fails; the other
        // order would leave a robot reachable only over IPv6, which is the family §82.5 says
        // carries an address a browser cannot connect to without a zone index.
        match open_v4() {
            Ok(link) => links.push(link),
            Err(error) => last = Some(error),
        }
        match open_v6() {
            Ok(link) => links.push(link),
            Err(error) => last = Some(error),
        }

        if links.is_empty() {
            return Err(OpenError::Bind(
                last.unwrap_or_else(|| io::Error::other("no socket could be opened")),
            ));
        }

        Ok(Self {
            links,
            responder,
            activity: Activity::default(),
            buffer: vec![0u8; MAX_MESSAGE],
        })
    }
}

impl<L: Link> Discovery<L> {
    /// Build over links a caller supplies. The seam the tests use.
    pub fn with_links(links: Vec<L>, responder: Responder) -> Self {
        Self {
            links,
            responder,
            activity: Activity::default(),
            buffer: vec![0u8; MAX_MESSAGE],
        }
    }

    /// Where the name claim stands.
    pub fn claim(&self) -> &Claim {
        self.responder.claim()
    }

    /// What has been sent and received.
    pub fn activity(&self) -> &Activity {
        &self.activity
    }

    /// What the responder itself has seen.
    pub fn counters(&self) -> &skynet_mdns::responder::Counters {
        self.responder.counters()
    }

    /// The instance name being claimed.
    pub fn instance(&self) -> String {
        self.responder.instance()
    }

    /// One pass: take what has arrived, then send what is due.
    ///
    /// Non-blocking, and called from the daemon's single loop beside the two listeners — §33.2's
    /// concurrency model, which is one thread and a poll rather than a task per subsystem.
    ///
    /// Reading before sending is deliberate: a conflicting announcement that arrived since the last
    /// pass must be able to stop a probe that is due in this one, and the other order would put the
    /// claim on the wire first.
    pub fn poll(&mut self, now_ms: u64, jitter: f64) {
        for index in 0..self.links.len() {
            loop {
                let mut buffer = std::mem::take(&mut self.buffer);
                let taken = self.links[index].recv_from(&mut buffer);
                let done = match taken {
                    Ok((len, from)) => {
                        self.activity.received += 1;
                        self.responder.heard(&buffer[..len], from, now_ms, jitter);
                        false
                    }
                    // Nothing waiting, or an ICMP error surfacing on the socket, or a datagram
                    // larger than the buffer. None of them is a reason to stop answering.
                    Err(_) => true,
                };
                self.buffer = buffer;
                if done {
                    break;
                }
            }
        }

        for outgoing in self.responder.due(now_ms) {
            match outgoing.destination {
                Destination::Multicast => {
                    for link in &self.links {
                        self.activity
                            .count(link.send_to(&outgoing.datagram, link.group()));
                    }
                }
                // Back on whichever link it came in on. Matching the family is what picks the
                // socket: an IPv4 asker cannot be answered on the IPv6 one.
                Destination::Unicast(to) => {
                    let mut answered = false;
                    for link in &self.links {
                        if link.group().is_ipv4() == to.is_ipv4() {
                            self.activity.count(link.send_to(&outgoing.datagram, to));
                            answered = true;
                        }
                    }
                    if !answered {
                        self.activity.send_failures += 1;
                    }
                }
            }
        }
    }

    /// Withdraw the name (RFC 6762 §10.1), on the way out.
    ///
    /// Best effort by construction: a robot losing power sends nothing, which is why the records
    /// carry a TTL as well. What this removes is the case where a robot was shut down cleanly and a
    /// browser goes on listing it for two minutes.
    pub fn farewell(&mut self) {
        let Some(outgoing) = self.responder.farewell() else {
            return;
        };
        for link in &self.links {
            self.activity
                .count(link.send_to(&outgoing.datagram, link.group()));
        }
    }

    /// One line for the log, saying what a browse would find.
    ///
    /// The received count is in it because of what it distinguishes: a responder that has answered
    /// nothing on a link where nothing was asked, and one on a link whose multicast never reaches
    /// it, are the same line without it — and the second is a robot nobody will find.
    pub fn report(&self) -> String {
        let counters = self.responder.counters();
        format!(
            "{} — {}, {} sent, {} received, {} answered, {} defended, {} malformed",
            self.responder.instance(),
            self.responder.claim(),
            self.activity.sent,
            self.activity.received,
            counters.answered,
            counters.defended,
            counters.malformed
        )
    }
}

impl Activity {
    fn count(&mut self, result: io::Result<usize>) {
        match result {
            Ok(_) => self.sent += 1,
            Err(_) => self.send_failures += 1,
        }
    }
}

fn open_v4() -> io::Result<Multicast> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv4Addr::UNSPECIFIED, endpoint::PORT)))?;
    socket.join_multicast_v4(&endpoint::GROUP_V4, &Ipv4Addr::UNSPECIFIED)?;
    // Neither is fatal if the platform refuses it: the answer still goes out, and every stack that
    // matters keeps a multicast datagram on the link regardless.
    let _ = socket.set_multicast_ttl_v4(MULTICAST_TTL);
    // Loopback on, so a browse running on this same machine sees this robot. That is how §82.4
    // said this half would first be exercised, and turning it off to save one datagram would make
    // that impossible.
    let _ = socket.set_multicast_loop_v4(true);
    socket.set_nonblocking(true)?;
    Ok(Multicast {
        socket,
        group: SocketAddr::from((endpoint::GROUP_V4, endpoint::PORT)),
    })
}

fn open_v6() -> io::Result<Multicast> {
    let socket = UdpSocket::bind(SocketAddr::from((Ipv6Addr::UNSPECIFIED, endpoint::PORT)))?;
    // Interface 0: the one the routing table picks. Sending on every interface needs enumeration
    // `std` does not provide, and §82.3 already records that a browse asks on one interface too —
    // this is the same limit on the answering side, not a new one.
    socket.join_multicast_v6(&endpoint::GROUP_V6, 0)?;
    let _ = socket.set_multicast_loop_v6(true);
    socket.set_nonblocking(true)?;
    Ok(Multicast {
        socket,
        group: SocketAddr::from((endpoint::GROUP_V6, endpoint::PORT)),
    })
}

/// This machine's name, for the SRV target and as the default label.
///
/// Read from the kernel rather than from a configuration file, because it is the name the machine
/// already answers to on this network and a second place to write a robot's name is a second place
/// for it to be wrong. `/proc/sys/kernel/hostname` is the Linux robot's; the environment variables
/// are what a developer's machine has.
///
/// **Only the first label.** A hostname of `arm-01.factory.example` describes a robot called
/// `arm-01`, and putting the whole of it in a DNS-SD instance name would produce
/// `arm-01.factory.example._skynet._tcp.local` — a name that parses, resolves to nothing, and reads
/// as a robot in a domain nobody can reach.
pub fn machine_name() -> Option<String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .or_else(|| std::env::var("HOSTNAME").ok())
        .or_else(|| std::env::var("COMPUTERNAME").ok())?;
    let first = raw.trim().split('.').next()?.trim();
    // A DNS label is 63 bytes (RFC 1035), and `Name::encode` refuses a longer one — which would
    // turn into "the advertisement could not be encoded" three layers away from the cause.
    if first.is_empty() || first.len() > 63 {
        return None;
    }
    Some(first.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use skynet_mdns::service;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::net::IpAddr;

    /// A link with a queue in and a list out.
    #[derive(Debug, Default)]
    struct Fake {
        inbound: RefCell<VecDeque<(Vec<u8>, SocketAddr)>>,
        sent: RefCell<Vec<(Vec<u8>, SocketAddr)>>,
        refuse_sends: bool,
    }

    impl Link for Fake {
        fn recv_from(&self, buffer: &mut [u8]) -> io::Result<(usize, SocketAddr)> {
            match self.inbound.borrow_mut().pop_front() {
                Some((datagram, from)) => {
                    buffer[..datagram.len()].copy_from_slice(&datagram);
                    Ok((datagram.len(), from))
                }
                None => Err(io::Error::from(io::ErrorKind::WouldBlock)),
            }
        }

        fn send_to(&self, datagram: &[u8], to: SocketAddr) -> io::Result<usize> {
            if self.refuse_sends {
                return Err(io::Error::other("multicast filtered"));
            }
            self.sent.borrow_mut().push((datagram.to_vec(), to));
            Ok(datagram.len())
        }

        fn group(&self) -> SocketAddr {
            SocketAddr::from((endpoint::GROUP_V4, endpoint::PORT))
        }
    }

    const CONSOLE: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(169, 254, 3, 4)), 8099);

    fn asker() -> SocketAddr {
        SocketAddr::from((Ipv4Addr::new(169, 254, 9, 9), endpoint::PORT))
    }

    fn responder(now_ms: u64) -> Responder {
        Responder::new("arm-01", "arm-01", CONSOLE.port(), &[CONSOLE.ip()], now_ms).unwrap()
    }

    /// Probe to completion and clear what that sent.
    ///
    /// Four passes: three probes an interval apart, then the interval after the last one, which is
    /// when the name is taken.
    fn claimed(discovery: &mut Discovery<Fake>) {
        for step in 0..=3 {
            discovery.poll(step * 250, 0.0);
        }
        assert!(discovery.claim().advertising());
        discovery.poll(750 + 1_000, 0.0);
        discovery.links[0].sent.borrow_mut().clear();
    }

    #[test]
    fn a_browse_is_answered_on_the_link_it_arrived_on() {
        let mut discovery = Discovery::with_links(vec![Fake::default()], responder(0));
        claimed(&mut discovery);

        discovery.links[0]
            .inbound
            .borrow_mut()
            .push_back((service::query(true).encode().unwrap(), asker()));
        discovery.poll(5_000, 0.0);

        let sent = discovery.links[0].sent.borrow();
        assert_eq!(sent.len(), 1, "one answer");
        assert_eq!(sent[0].1, asker(), "unicast, because the query set QU");
        let message = skynet_mdns::wire::Message::decode(&sent[0].0).unwrap();
        let found = service::read(&message, None);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "arm-01");
        assert_eq!(
            found[0].port,
            CONSOLE.port(),
            "the console's port, not 5353"
        );
        assert_eq!(found[0].addresses, vec![CONSOLE.ip()]);
    }

    #[test]
    fn probing_puts_three_datagrams_on_the_link_before_anything_is_claimed() {
        let mut discovery = Discovery::with_links(vec![Fake::default()], responder(0));
        discovery.poll(0, 0.0);
        assert!(!discovery.claim().advertising(), "one probe is not a claim");
        discovery.poll(250, 0.0);
        discovery.poll(500, 0.0);
        assert!(
            !discovery.claim().advertising(),
            "nor is the third — the window after it is the point"
        );
        discovery.poll(750, 0.0);
        assert!(discovery.claim().advertising());
        let sent = discovery.links[0].sent.borrow();
        assert_eq!(sent.len(), 4, "three probes and the first announcement");
        assert!(
            sent.iter()
                .all(|(_, to)| to.ip() == IpAddr::V4(endpoint::GROUP_V4)),
            "probes and announcements are multicast"
        );
    }

    #[test]
    fn a_rival_stops_the_advertisement_and_the_daemon_can_say_so() {
        let mut discovery = Discovery::with_links(vec![Fake::default()], responder(0));
        claimed(&mut discovery);

        let rival = service::advertisement("arm-01", "someone-else", 9000, &[CONSOLE.ip()], false)
            .unwrap()
            .encode()
            .unwrap();
        discovery.links[0]
            .inbound
            .borrow_mut()
            .push_back((rival, asker()));
        discovery.poll(5_000, 0.0);

        assert!(!discovery.claim().advertising());
        assert!(
            discovery.report().contains("169.254.9.9"),
            "{}",
            discovery.report()
        );
        discovery.links[0]
            .inbound
            .borrow_mut()
            .push_back((service::query(true).encode().unwrap(), asker()));
        discovery.poll(6_000, 0.0);
        assert!(
            discovery.links[0].sent.borrow().is_empty(),
            "a name it does not hold is a name it does not answer to"
        );
    }

    #[test]
    fn its_own_announcement_returning_on_loopback_is_not_a_rival() {
        let mut discovery = Discovery::with_links(vec![Fake::default()], responder(0));
        claimed(&mut discovery);

        let own =
            service::advertisement("arm-01", "arm-01", CONSOLE.port(), &[CONSOLE.ip()], false)
                .unwrap()
                .encode()
                .unwrap();
        discovery.links[0].inbound.borrow_mut().push_back((
            own,
            SocketAddr::from((Ipv4Addr::new(10, 0, 0, 9), endpoint::PORT)),
        ));
        discovery.poll(5_000, 0.0);

        assert!(
            discovery.claim().advertising(),
            "loopback is not a conflict"
        );
        assert_eq!(discovery.counters().own, 1);
    }

    #[test]
    fn a_link_that_refuses_every_send_is_counted_rather_than_hidden() {
        let link = Fake {
            refuse_sends: true,
            ..Fake::default()
        };
        let mut discovery = Discovery::with_links(vec![link], responder(0));
        discovery.poll(0, 0.0);
        assert_eq!(discovery.activity().sent, 0);
        assert_eq!(
            discovery.activity().send_failures,
            1,
            "a firewall eating multicast is not the same as a quiet network"
        );
    }

    #[test]
    fn the_farewell_goes_out_only_after_a_claim() {
        let mut discovery = Discovery::with_links(vec![Fake::default()], responder(0));
        discovery.farewell();
        assert!(
            discovery.links[0].sent.borrow().is_empty(),
            "nothing withdraws a name it never held"
        );

        claimed(&mut discovery);
        discovery.farewell();
        let sent = discovery.links[0].sent.borrow();
        assert_eq!(sent.len(), 1);
        let message = skynet_mdns::wire::Message::decode(&sent[0].0).unwrap();
        assert!(message.answers.iter().all(|record| record.ttl == 0));
    }

    #[test]
    fn a_malformed_datagram_does_not_stop_the_pass() {
        let mut discovery = Discovery::with_links(vec![Fake::default()], responder(0));
        claimed(&mut discovery);
        {
            let mut inbound = discovery.links[0].inbound.borrow_mut();
            inbound.push_back((vec![0x01, 0x02], asker()));
            inbound.push_back((service::query(true).encode().unwrap(), asker()));
        }
        discovery.poll(5_000, 0.0);
        assert_eq!(discovery.counters().malformed, 1);
        assert_eq!(
            discovery.links[0].sent.borrow().len(),
            1,
            "the query behind the bad datagram was still answered"
        );
    }

    #[test]
    fn the_machine_name_is_one_label_or_nothing() {
        // The function reads the machine it runs on, so what is asserted here is the rule rather
        // than a value: whatever it returns is a single DNS label a name can be built from.
        if let Some(name) = machine_name() {
            assert!(!name.is_empty());
            assert!(!name.contains('.'), "one label, never a domain: {name}");
            assert!(name.len() <= 63);
        }
    }
}

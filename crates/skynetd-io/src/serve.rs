//! The listeners (§33.7, §34.2, §39).
//!
//! Two of them, carrying the same routes:
//!
//! * a **Unix socket** at `/run/skynetd.sock`, root-and-`skynet`-group only. This is what the CLI
//!   talks to, and it is the one that works when everything else is broken;
//! * a **LAN listener** for the console, bound to a link-local address.
//!
//! > **The robot never accepts an inbound connection from the Hub.** Every exchange is initiated by
//! > the robot. […] The LAN console (§39) is the sole listener, and it is bound to link-local
//! > interfaces only.
//!
//! [`Lan::bind`] enforces that in the only place it can be enforced: it **refuses a routable
//! address**. Not warns, not logs — refuses, and names the address. A robot whose console is
//! reachable from outside its network is the failure §34.2 spends its whole section preventing, and
//! a configuration file is exactly where that gets set once by accident and never noticed.
//!
//! # One request per connection, and the reason
//!
//! Both listeners are single-threaded and handle one request per connection. That is a real
//! limit — a console making six requests makes six connections — and it buys two things worth more
//! than the throughput: there is no request-pipelining state machine that can desynchronise two
//! requests on one socket, and there is no shared mutable state between concurrent handlers, so the
//! daemon's own bookkeeping cannot be raced by an HTTP client.
//!
//! A read deadline bounds every connection, so a client that opens a socket and says nothing holds
//! the API for [`READ_TIMEOUT`] and not for ever.

use std::io::{ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use crate::api::{self, Context};
use crate::http::{self, Response};
use crate::launch::Launcher;
use crate::observe::Slots;

/// How long a connection may take to send a complete request.
pub const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The socket's mode: owner and group only (§33.7).
pub const SOCKET_MODE: u32 = 0o660;

/// The local socket.
#[derive(Debug)]
pub struct Local {
    listener: UnixListener,
}

impl Local {
    /// Bind `/run/skynetd.sock`, replacing a socket left behind by a previous run.
    ///
    /// Unlinking first is safe and necessary: a Unix socket file outlives the process that made it,
    /// so a `skynetd` that crashed leaves one behind and a bind onto it fails with `EADDRINUSE`.
    /// The path is inside `/run`, which is root-owned, so the file being removed cannot be
    /// something an unprivileged process planted there.
    pub fn bind(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        // Mode before anybody can connect. Setting it after the bind leaves a window in which the
        // socket is world-writable, which on a robot is a window in which anything on the machine
        // can stop a package.
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(SOCKET_MODE))?;
        Ok(Self { listener })
    }

    /// Serve until `should_continue` says otherwise.
    pub fn serve<L: Launcher, S: Slots>(
        &self,
        context: &mut Context<'_, L, S>,
        mut should_continue: impl FnMut() -> bool,
    ) -> std::io::Result<()> {
        for stream in self.listener.incoming() {
            if !should_continue() {
                break;
            }
            match stream {
                Ok(stream) => serve_one(stream, context),
                Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// The listener, for a caller running its own accept loop.
    pub fn listener(&self) -> &UnixListener {
        &self.listener
    }
}

/// Why a LAN address was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotLinkLocal {
    /// The address as configured.
    pub address: IpAddr,
}

impl std::fmt::Display for NotLinkLocal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} is routable. §34.2 binds the console to link-local interfaces only and says \
             reaching a robot's console from outside its network is not a feature and not a \
             configuration option — so this is refused rather than warned about",
            self.address
        )
    }
}

impl std::error::Error for NotLinkLocal {}

/// Whether an address is one the console may bind.
///
/// Loopback, IPv4 link-local (`169.254/16`), IPv6 link-local (`fe80::/10`), and the private ranges
/// a robot's own network actually uses — `10/8`, `172.16/12`, `192.168/16` — plus IPv6 unique-local
/// (`fc00::/7`). Everything else is refused.
///
/// The private ranges are included because a robot on a warehouse network has a `192.168` address
/// and no other, and a console that refused to bind there would be a console nobody could reach at
/// exactly the moment §32.1 describes: the network is down, the Hub is unreachable, and somebody
/// needs to see the robot. They are also not routable from the internet, which is the property
/// §34.2 is actually asking for.
pub fn is_link_local(address: IpAddr) -> bool {
    match address {
        // `0.0.0.0` is deliberately absent: it is not an address a robot has, it is a request to
        // bind every address the robot has — including the routable one §34.2 forbids. The clause
        // that used to say so was `v4 == Ipv4Addr::UNSPECIFIED && false`, which reads as a rule and
        // is a comment the compiler deletes; a newer clippy calls it a logic bug and is right.
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_link_local() || v4.is_private(),
        IpAddr::V6(v6) => v6.is_loopback() || is_v6_link_local(v6) || is_v6_unique_local(v6),
    }
}

fn is_v6_link_local(address: Ipv6Addr) -> bool {
    address.segments()[0] & 0xffc0 == 0xfe80
}

fn is_v6_unique_local(address: Ipv6Addr) -> bool {
    address.octets()[0] & 0xfe == 0xfc
}

/// The console listener.
#[derive(Debug)]
pub struct Lan {
    listener: TcpListener,
}

impl Lan {
    /// Bind a link-local address, refusing a routable one.
    ///
    /// `0.0.0.0` is refused too, and it is the case this check exists for: binding every interface
    /// is how a console ends up on the interface facing the world, and it is the value somebody
    /// reaches for when the specific one does not work.
    pub fn bind(address: SocketAddr) -> std::io::Result<Self> {
        if address.ip().is_unspecified() || !is_link_local(address.ip()) {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                NotLinkLocal {
                    address: address.ip(),
                }
                .to_string(),
            ));
        }
        Ok(Self {
            listener: TcpListener::bind(address)?,
        })
    }

    /// The address it actually bound, which is what to print.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Serve until `should_continue` says otherwise.
    pub fn serve<L: Launcher, S: Slots>(
        &self,
        context: &mut Context<'_, L, S>,
        mut should_continue: impl FnMut() -> bool,
    ) -> std::io::Result<()> {
        for stream in self.listener.incoming() {
            if !should_continue() {
                break;
            }
            match stream {
                Ok(stream) => serve_one(stream, context),
                Err(error) if error.kind() == ErrorKind::WouldBlock => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// The listener, for a caller running its own accept loop.
    pub fn listener(&self) -> &TcpListener {
        &self.listener
    }
}

/// Anything a request can arrive over.
pub trait Connection: Read + Write {
    /// Bound how long this connection may take.
    fn set_deadline(&self, timeout: Duration) -> std::io::Result<()>;
}

impl Connection for UnixStream {
    fn set_deadline(&self, timeout: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(timeout))?;
        self.set_write_timeout(Some(timeout))
    }
}

impl Connection for TcpStream {
    fn set_deadline(&self, timeout: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(timeout))?;
        self.set_write_timeout(Some(timeout))
    }
}

/// Read one request, answer it, and close.
pub fn serve_one<C: Connection, L: Launcher, S: Slots>(
    mut stream: C,
    context: &mut Context<'_, L, S>,
) {
    let _ = stream.set_deadline(READ_TIMEOUT);
    let response = match read_request(&mut stream) {
        Ok(request) => {
            // §38.1's `log_level` at `debug`: one line per request, which is the first thing
            // anybody wants on a bench — *did my request arrive at all, and what did the robot
            // decide about it*. Off by default, because a robot whose console is open is asked
            // several times a second and this is a log, not a trace.
            let line = format!("{} /{}", request.method, request.segments().join("/"));
            let response = api::handle(context, &request);
            crate::log_debug!("{line} → {}", response.status);
            response
        }
        Err(status) => {
            crate::log_debug!("a request could not be read → {status}");
            api::error(status, "the request could not be read")
        }
    };
    let _ = stream.write_all(&response.encode());
    let _ = stream.flush();
}

/// Read a complete request off a connection.
///
/// Returns the status to answer with rather than an error type, because every failure here is
/// something the client is told about and nothing is retried: a request that could not be read
/// cannot be read better on a second attempt over the same bytes.
pub fn read_request<C: Read>(stream: &mut C) -> Result<http::Request, u16> {
    let mut buffer = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    let head_end = loop {
        if let Some(at) = http::head_end(&buffer) {
            break at;
        }
        if buffer.len() > http::MAX_HEAD {
            return Err(413);
        }
        match stream.read(&mut chunk) {
            Ok(0) => return Err(400),
            Ok(read) => buffer.extend_from_slice(&chunk[..read]),
            Err(_) => return Err(400),
        }
    };

    let head = std::str::from_utf8(&buffer[..head_end]).map_err(|_| 400u16)?;
    let length = http::declared_length(head).map_err(http::ParseError::status)?;

    let mut body = buffer[head_end..].to_vec();
    while body.len() < length {
        match stream.read(&mut chunk) {
            Ok(0) => return Err(400),
            Ok(read) => body.extend_from_slice(&chunk[..read]),
            Err(_) => return Err(400),
        }
        if body.len() > http::MAX_BODY {
            return Err(413);
        }
    }
    body.truncate(length);

    http::parse(head, body).map_err(http::ParseError::status)
}

/// Encode a response — exposed so a caller running its own loop writes the same bytes.
pub fn write_response<C: Write>(stream: &mut C, response: &Response) -> std::io::Result<()> {
    stream.write_all(&response.encode())?;
    stream.flush()
}

//! The segment: where a frame goes after the master sends it — a Rust port of
//! `drivers/ethercat/skynet_ethercat/segment.py`'s `RawSocketSegment`.
//!
//! The Python module also carries `LoopbackSegment`, the slave stack in-process. This port has no
//! slave stack of its own and wants none: it is measured against the Python one in its own process,
//! served on one end of a veth pair by `python3 -m skynet_ethercat.wire` — the arrangement SOEM,
//! an independent master, is cross-checked in (`DRY-COVERAGE-PLAN.md` F48). So [`Segment`] has one
//! production implementation, [`RawSocket`], and the trait exists for the bridge's unit tests.
//!
//! # AF_PACKET without libc
//!
//! `socket`, `if_nametoindex`, `bind`, `poll` and `recv` are declared here, as `canopen-rs`
//! declares its SocketCAN calls, and writes go through [`std::fs::File`] over the owned
//! descriptor. Linux only: elsewhere [`RawSocket::open`] refuses rather than failing to link.

use std::fmt;
use std::time::{Duration, Instant};

use crate::frames::Frame;

/// A transport that could not be opened or did not answer, named rather than fallen back from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentError(pub String);

impl fmt::Display for SegmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a frame goes round the ring and comes back.
pub trait Segment {
    /// Send `frame` and return the ring's answer to it.
    fn cycle(&mut self, frame: &Frame) -> Result<Frame, SegmentError>;
}

/// How long a cycle waits for the ring's answer: `segment.py`'s 50 ms.
pub const ANSWER_TIMEOUT: Duration = Duration::from_millis(50);

/// The largest frame read off the interface: an Ethernet frame without its FCS.
#[cfg(target_os = "linux")]
const MAX_FRAME: usize = 1518;

/// EtherCAT frames on a real interface through `AF_PACKET`. Needs `CAP_NET_RAW` over the
/// interface's network namespace, and something on the other end.
pub struct RawSocket {
    #[cfg(target_os = "linux")]
    socket: std::fs::File,
}

#[cfg(target_os = "linux")]
mod sys {
    use std::ffi::{c_char, c_int, c_short, c_uint, c_ulong, c_void};

    pub const AF_PACKET: c_int = 17;
    pub const SOCK_RAW: c_int = 3;
    pub const SOCK_CLOEXEC: c_int = 0o2000000;
    pub const POLLIN: c_short = 1;
    pub const MSG_DONTWAIT: c_int = 0x40;

    /// `struct sockaddr_ll`, twenty bytes.
    #[repr(C)]
    pub struct SockaddrLl {
        pub family: u16,
        pub protocol: u16,
        pub ifindex: c_int,
        pub hatype: u16,
        pub pkttype: u8,
        pub halen: u8,
        pub addr: [u8; 8],
    }

    #[repr(C)]
    pub struct PollFd {
        pub fd: c_int,
        pub events: c_short,
        pub revents: c_short,
    }

    extern "C" {
        pub fn socket(domain: c_int, kind: c_int, protocol: c_int) -> c_int;
        pub fn if_nametoindex(name: *const c_char) -> c_uint;
        pub fn bind(fd: c_int, addr: *const SockaddrLl, len: u32) -> c_int;
        pub fn poll(fds: *mut PollFd, count: c_ulong, timeout_ms: c_int) -> c_int;
        pub fn recv(fd: c_int, buf: *mut c_void, len: usize, flags: c_int) -> isize;
    }
}

impl RawSocket {
    /// A raw EtherCAT socket bound to `interface`. There is no fallback, on purpose: a master that
    /// quietly ran against something else would report a ring that is not there.
    #[cfg(target_os = "linux")]
    pub fn open(interface: &str) -> Result<RawSocket, SegmentError> {
        use std::ffi::CString;
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        use crate::frames::ETHERTYPE;

        let refuse = |what: String| {
            SegmentError(format!(
                "a raw EtherCAT socket on `{interface}` could not be opened: {what}. This needs \
                 Linux and CAP_NET_RAW. There is no fallback here on purpose: a master that \
                 silently ran against anything else would report a ring that is not there."
            ))
        };
        let name = CString::new(interface).map_err(|_| refuse("a NUL in the name".into()))?;
        let protocol = ETHERTYPE.to_be();
        // SAFETY: `socket` takes three integers and returns a descriptor or -1.
        let fd = unsafe {
            sys::socket(
                sys::AF_PACKET,
                sys::SOCK_RAW | sys::SOCK_CLOEXEC,
                protocol as i32,
            )
        };
        if fd < 0 {
            return Err(refuse(std::io::Error::last_os_error().to_string()));
        }
        // SAFETY: `fd` was just opened and nothing else owns it.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: `name` is a live NUL-terminated string.
        let index = unsafe { sys::if_nametoindex(name.as_ptr()) };
        if index == 0 {
            return Err(SegmentError(format!(
                "`{interface}` could not be bound: there is no such interface"
            )));
        }
        let address = sys::SockaddrLl {
            family: sys::AF_PACKET as u16,
            protocol,
            ifindex: index as i32,
            hatype: 0,
            pkttype: 0,
            halen: 0,
            addr: [0; 8],
        };
        // SAFETY: `address` is a live `sockaddr_ll` and its size is passed.
        let rc = unsafe {
            sys::bind(
                owned.as_raw_fd(),
                &address,
                std::mem::size_of::<sys::SockaddrLl>() as u32,
            )
        };
        if rc != 0 {
            return Err(SegmentError(format!(
                "`{interface}` could not be bound: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(RawSocket {
            socket: std::fs::File::from(owned),
        })
    }

    /// `AF_PACKET` is Linux's.
    #[cfg(not(target_os = "linux"))]
    pub fn open(interface: &str) -> Result<RawSocket, SegmentError> {
        Err(SegmentError(format!(
            "a raw EtherCAT socket on `{interface}` needs Linux's AF_PACKET"
        )))
    }

    /// A second descriptor on the same socket, for the stop path.
    #[cfg(target_os = "linux")]
    pub fn try_clone(&self) -> std::io::Result<RawSocket> {
        Ok(RawSocket {
            socket: self.socket.try_clone()?,
        })
    }

    /// Off Linux there is never a socket to clone.
    #[cfg(not(target_os = "linux"))]
    pub fn try_clone(&self) -> std::io::Result<RawSocket> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    /// Put one frame on the wire. One `write` of bytes already built: the stop path's call.
    #[cfg(target_os = "linux")]
    pub fn send_raw(&self, raw: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        let written = (&self.socket).write(raw)?;
        if written != raw.len() {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        Ok(())
    }

    /// Off Linux nothing is ever sent.
    #[cfg(not(target_os = "linux"))]
    pub fn send_raw(&self, _raw: &[u8]) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    /// The next frame off the interface, or `None` when `timeout` passed with nothing. What the
    /// other end of a cable reads: the scripted ring in `tests/common` serves a veth pair with it.
    #[cfg(target_os = "linux")]
    pub fn recv_raw(&self, timeout: Duration) -> std::io::Result<Option<Vec<u8>>> {
        use std::os::fd::AsRawFd;
        let fd = self.socket.as_raw_fd();
        let mut ready = sys::PollFd {
            fd,
            events: sys::POLLIN,
            revents: 0,
        };
        let timeout_ms = timeout.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32;
        // SAFETY: one live `pollfd`, and the count says one.
        let rc = unsafe { sys::poll(&mut ready, 1, timeout_ms) };
        if rc < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                return Ok(None);
            }
            return Err(error);
        }
        if rc == 0 {
            return Ok(None);
        }
        let mut raw = vec![0u8; MAX_FRAME];
        // SAFETY: `raw` is writable for its whole length, and that length is passed.
        let n = unsafe {
            sys::recv(
                fd,
                raw.as_mut_ptr() as *mut std::ffi::c_void,
                raw.len(),
                sys::MSG_DONTWAIT,
            )
        };
        if n < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(error);
        }
        raw.truncate(n as usize);
        Ok(Some(raw))
    }

    /// Off Linux nothing is ever received.
    #[cfg(not(target_os = "linux"))]
    pub fn recv_raw(&self, _timeout: Duration) -> std::io::Result<Option<Vec<u8>>> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

impl Segment for RawSocket {
    /// Send, then read until the ring's answer. A raw socket sees this master's own outgoing frame
    /// as well as the answer, and the two are told apart the way `segment.py` tells them apart: an
    /// unparseable frame, one whose first index is another exchange's, and one byte-identical with
    /// what was sent are all skipped. Unlike `segment.py`, the 50 ms is a deadline for the whole
    /// wait, not for each read — so a stream of frames that are not the answer cannot hold a cycle
    /// open for ever.
    fn cycle(&mut self, frame: &Frame) -> Result<Frame, SegmentError> {
        let sent = frame
            .pack()
            .map_err(|error| SegmentError(error.to_string()))?;
        self.send_raw(&sent)
            .map_err(|error| SegmentError(format!("the frame did not leave: {error}")))?;
        let index = frame.datagrams.first().map(|datagram| datagram.index);
        let deadline = Instant::now() + ANSWER_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(SegmentError(format!(
                    "no answer from the ring within {} ms. A frame that does not come back is a \
                     cable, a slave or a switch, and this master does not guess which.",
                    ANSWER_TIMEOUT.as_millis()
                )));
            }
            let Some(raw) = self
                .recv_raw(remaining)
                .map_err(|error| SegmentError(format!("reading the answer failed: {error}")))?
            else {
                continue;
            };
            let Ok(answer) = Frame::unpack(&raw) else {
                continue;
            };
            if answer.datagrams.first().map(|datagram| datagram.index) != index {
                continue;
            }
            if raw == sent {
                continue; // this master's own echo, unmodified
            }
            return Ok(answer);
        }
    }
}

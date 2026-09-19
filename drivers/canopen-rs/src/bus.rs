//! Where a CAN frame goes — a Rust port of the half of `drivers/canopen/skynet_canopen/bus.py`
//! that is a real bus.
//!
//! The Python module also carries `LoopbackBus`, an in-process bus its simulated drives attach
//! to. This port has no simulated drive of its own and wants none: it is measured against the
//! Python drives, in their own process, over a kernel CAN interface (`make check`, through
//! `drivers/canopen/vcan.sh`) — which is also the one substrate on which a drive can answer
//! *after* the master looks, where `DRY-COVERAGE-PLAN.md` F71 lived.
//!
//! So [`Bus`] has one production implementation, [`SocketCan`], and the trait exists for the
//! bridge's own unit tests, which script a bus frame by frame to reach the orderings a kernel bus
//! produces only sometimes: a stale SDO reply, an axis that misses a SYNC, traffic from nobody.
//!
//! # SocketCAN without libc
//!
//! The workspace carries no third-party dependencies, so the five calls a raw CAN socket needs —
//! `socket`, `if_nametoindex`, `bind`, `poll` and `recv` — are declared here, as `skynet-rt`
//! declares `mlockall`. Everything after the socket is open goes through [`std::fs::File`] over
//! the owned descriptor, because a raw CAN socket is read and written with `read(2)` and
//! `write(2)`, and `File` is exactly those two calls and a `close` on drop. Linux only: SocketCAN
//! is Linux's, and elsewhere [`SocketCan::open`] says so rather than failing to link.

use std::io;
use std::time::Duration;

use crate::frames::Frame;

/// Two methods and one property. Everything above this layer is written against them.
pub trait Bus {
    /// Whether a frame that is not here now will never arrive. False on a real bus, where a
    /// drive answers some microseconds after the master looks; true only for a scripted bus that
    /// has nothing more to say.
    fn synchronous(&self) -> bool {
        false
    }

    /// Put one frame on the bus.
    fn send(&mut self, frame: &Frame) -> io::Result<()>;

    /// The next frame, waiting at most `timeout`. `Ok(None)` when nothing arrived.
    fn recv(&mut self, timeout: Duration) -> io::Result<Option<Frame>>;
}

/// `struct can_frame` is sixteen bytes on the wire whatever `len` says: a 32-bit identifier with
/// the EFF/RTR/ERR flags in its top bits, a length, three bytes of padding, eight of data.
pub const CAN_FRAME_SIZE: usize = 16;

/// The flag bits of `can_id`. A frame with any set is extended, remote or an error frame — this
/// driver speaks 11-bit data frames, and anything else is somebody else's traffic.
const CAN_FLAGS: u32 = 0xE000_0000;

/// A frame as the kernel wants it written.
pub fn encode(frame: &Frame) -> [u8; CAN_FRAME_SIZE] {
    let mut raw = [0u8; CAN_FRAME_SIZE];
    raw[..4].copy_from_slice(&(frame.id as u32).to_le_bytes());
    raw[4] = frame.len;
    raw[8..16].copy_from_slice(&frame.data);
    raw
}

/// A frame as the kernel delivers it, or `None` for one this driver does not speak: flagged,
/// longer than classic CAN, or short.
pub fn decode(raw: &[u8]) -> Option<Frame> {
    if raw.len() != CAN_FRAME_SIZE {
        return None;
    }
    let can_id = u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]);
    if can_id & CAN_FLAGS != 0 {
        return None;
    }
    let len = raw[4] as usize;
    if len > 8 {
        return None;
    }
    Frame::new((can_id & 0x7FF) as u16, &raw[8..8 + len])
}

/// Linux SocketCAN, on a named interface — `can0`, or `vcan0` for a virtual one.
pub struct SocketCan {
    #[cfg(target_os = "linux")]
    socket: std::fs::File,
}

#[cfg(target_os = "linux")]
mod sys {
    use std::ffi::{c_char, c_int, c_short, c_uint, c_ulong, c_void};

    pub const AF_CAN: c_int = 29;
    pub const SOCK_RAW: c_int = 3;
    pub const SOCK_CLOEXEC: c_int = 0o2000000;
    pub const CAN_RAW: c_int = 1;
    pub const POLLIN: c_short = 1;
    pub const MSG_DONTWAIT: c_int = 0x40;

    /// `struct sockaddr_can`: the family, the interface index, and a 16-byte union of transport
    /// addresses this driver leaves zero. 24 bytes with the kernel's alignment.
    #[repr(C)]
    pub struct SockaddrCan {
        pub family: u16,
        pub ifindex: c_int,
        pub addr: [u8; 16],
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
        pub fn bind(fd: c_int, addr: *const SockaddrCan, len: u32) -> c_int;
        pub fn poll(fds: *mut PollFd, count: c_ulong, timeout_ms: c_int) -> c_int;
        pub fn recv(fd: c_int, buf: *mut c_void, len: usize, flags: c_int) -> isize;
    }
}

impl SocketCan {
    /// A raw CAN socket bound to `interface`. The error names the interface and what to do when
    /// it is absent, which on a fresh machine is the likeliest failure there is.
    #[cfg(target_os = "linux")]
    pub fn open(interface: &str) -> io::Result<SocketCan> {
        use std::ffi::CString;
        use std::os::fd::{FromRawFd, OwnedFd};

        let name = CString::new(interface).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "an interface name has no NUL in it",
            )
        })?;
        // SAFETY: `socket` takes three integers and returns a descriptor or -1.
        let fd =
            unsafe { sys::socket(sys::AF_CAN, sys::SOCK_RAW | sys::SOCK_CLOEXEC, sys::CAN_RAW) };
        if fd < 0 {
            let error = io::Error::last_os_error();
            return Err(io::Error::new(
                error.kind(),
                format!(
                    "this kernel will not give out a raw CAN socket ({error}). On Linux that is \
                     `modprobe can_raw vcan`; a container also needs the privilege to load them"
                ),
            ));
        }
        // SAFETY: `fd` is a descriptor this call just opened and nothing else owns.
        let owned = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: `name` is a live NUL-terminated string for the duration of the call.
        let index = unsafe { sys::if_nametoindex(name.as_ptr()) };
        if index == 0 {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!(
                    "there is no interface called `{interface}`. For a virtual bus: `ip link add \
                     dev {interface} type vcan && ip link set up {interface}`, or run under \
                     drivers/canopen/vcan.sh, which makes one without root"
                ),
            ));
        }
        let address = sys::SockaddrCan {
            family: sys::AF_CAN as u16,
            ifindex: index as i32,
            addr: [0; 16],
        };
        // SAFETY: `address` is a live `sockaddr_can` of the kernel's shape and its size is passed.
        let rc = unsafe {
            sys::bind(
                std::os::fd::AsRawFd::as_raw_fd(&owned),
                &address,
                std::mem::size_of::<sys::SockaddrCan>() as u32,
            )
        };
        if rc != 0 {
            let error = io::Error::last_os_error();
            return Err(io::Error::new(
                error.kind(),
                format!("could not bind to `{interface}`: {error}"),
            ));
        }
        Ok(SocketCan {
            socket: std::fs::File::from(owned),
        })
    }

    /// SocketCAN is Linux's. Elsewhere this refuses rather than failing to link, so the workspace
    /// still builds where the driver cannot run.
    #[cfg(not(target_os = "linux"))]
    pub fn open(interface: &str) -> io::Result<SocketCan> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("`{interface}` is a SocketCAN interface, and SocketCAN is Linux's"),
        ))
    }

    /// A second handle on the same socket, for the stop path: frames written on it go out on the
    /// same bus, and it needs no lock the connection holds.
    #[cfg(target_os = "linux")]
    pub fn try_clone(&self) -> io::Result<SocketCan> {
        Ok(SocketCan {
            socket: self.socket.try_clone()?,
        })
    }

    /// Off Linux there is never a socket to clone.
    #[cfg(not(target_os = "linux"))]
    pub fn try_clone(&self) -> io::Result<SocketCan> {
        Err(io::ErrorKind::Unsupported.into())
    }

    /// Write one frame, built on the stack. `write` on a raw CAN socket is one frame or an error,
    /// never part of one. This is the stop path's call, so it allocates nothing.
    #[cfg(target_os = "linux")]
    pub fn write_frame(&self, frame: &Frame) -> io::Result<()> {
        use std::io::Write;
        let raw = encode(frame);
        let written = (&self.socket).write(&raw)?;
        if written != CAN_FRAME_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "the kernel took part of a CAN frame",
            ));
        }
        Ok(())
    }

    /// Off Linux there is no socket, so nothing is ever written.
    #[cfg(not(target_os = "linux"))]
    pub fn write_frame(&self, _frame: &Frame) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(target_os = "linux")]
impl Bus for SocketCan {
    fn send(&mut self, frame: &Frame) -> io::Result<()> {
        self.write_frame(frame)
    }

    fn recv(&mut self, timeout: Duration) -> io::Result<Option<Frame>> {
        use std::os::fd::AsRawFd;
        let fd = self.socket.as_raw_fd();
        loop {
            let mut ready = sys::PollFd {
                fd,
                events: sys::POLLIN,
                revents: 0,
            };
            // Rounded up to whole milliseconds, so a sub-millisecond wait is still a wait: a
            // window rounded down to zero would be `F71`'s empty look, one layer down.
            let timeout_ms = timeout.as_nanos().div_ceil(1_000_000).min(i32::MAX as u128) as i32;
            // SAFETY: one live `pollfd`, and the count says one.
            let rc = unsafe { sys::poll(&mut ready, 1, timeout_ms) };
            if rc < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if rc == 0 {
                return Ok(None);
            }
            let mut raw = [0u8; CAN_FRAME_SIZE];
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
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::WouldBlock {
                    return Ok(None);
                }
                return Err(error);
            }
            // A flagged or malformed frame is somebody else's traffic, and it is consumed rather
            // than handed up: the caller asked for the next frame this driver speaks.
            match decode(&raw[..n as usize]) {
                Some(frame) => return Ok(Some(frame)),
                None => continue,
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Bus for SocketCan {
    fn send(&mut self, _frame: &Frame) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }

    fn recv(&mut self, _timeout: Duration) -> io::Result<Option<Frame>> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_round_trips_through_the_kernel_layout() {
        let frame = Frame::new(0x281, &[0x37, 0x06, 0x40, 0xE2, 0x01, 0x00]).unwrap();
        let raw = encode(&frame);
        assert_eq!(&raw[..8], &[0x81, 0x02, 0, 0, 6, 0, 0, 0]);
        assert_eq!(decode(&raw), Some(frame));
    }

    #[test]
    fn flagged_long_and_short_frames_are_not_ours() {
        let mut raw = encode(&Frame::new(0x281, &[1]).unwrap());
        raw[3] |= 0x80; // CAN_EFF_FLAG
        assert_eq!(decode(&raw), None, "an extended frame");
        let mut raw = encode(&Frame::new(0x281, &[1]).unwrap());
        raw[4] = 9;
        assert_eq!(decode(&raw), None, "a length past classic CAN");
        assert_eq!(decode(&raw[..15]), None, "a short read");
    }
}

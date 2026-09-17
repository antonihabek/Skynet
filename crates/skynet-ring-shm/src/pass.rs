//! Handing the ring's descriptor to the process that will write it (§25).
//!
//! §25 says the runtime creates the ring and *"the fd is passed over the policy's control socket"*,
//! and until this module existed nothing passed one. The consequence is recorded in §80 — **no two
//! processes have ever shared a ring** — and in `skynetp`'s own header, which removed a `--ring-fd`
//! rather than ship half a handshake: *"an option to adopt a descriptor nobody publishes"*.
//!
//! # Why a socket and not inheritance
//!
//! A child inherits an open descriptor across `exec`, which is simpler, needs no control messages,
//! and is what this crate's cross-process test uses. It is not available here. §33.5 launches a
//! policy as a **systemd unit**, so the process that creates the ring is not the process that starts
//! the policy — the daemon starts it, and §33.1 keeps the daemon off the control path entirely. Two
//! processes that are started independently can only meet at a name in the filesystem, and
//! `SCM_RIGHTS` over a Unix socket is the mechanism that carries a descriptor across one.
//!
//! # What crosses, and what does not
//!
//! A descriptor, and one byte of payload. The byte is not decoration: an implementation may discard
//! ancillary data attached to an empty message, so a zero-length send is a portable way to lose the
//! fd and learn nothing.
//!
//! What does **not** cross is any authority to decide the geometry. The receiver is told how many
//! slots to expect by the caller, `Mapping::adopt` reads and validates the header before it returns,
//! and the file is sealed against resizing before it ever leaves the creating process. A hostile
//! peer on this socket can hand over something that is not a ring, and the answer to that is
//! [`ShmError::Ring`] rather than a mapping.
//!
//! # The socket itself belongs to the caller
//!
//! Binding, connecting, permissions and the path are `std::os::unix::net`'s work and carry no
//! `unsafe`, so they stay in the hosts where a reviewer expects them. What is here is two syscalls
//! and the control-message layout they need — which is the same argument the crate header makes for
//! keeping `mmap` in one short file.

use crate::ShmError;

#[cfg(target_os = "linux")]
mod sys {
    use core::ffi::{c_int, c_void};

    /// Ancillary data at the socket API level.
    pub const SOL_SOCKET: c_int = 1;
    /// The control-message type that carries descriptors.
    pub const SCM_RIGHTS: c_int = 1;
    /// Set `FD_CLOEXEC` on the descriptor as it arrives.
    ///
    /// The same argument `MFD_CLOEXEC` makes on the creating side: a ring that leaks into every
    /// child the receiving process happens to spawn is an accident nobody would see.
    pub const MSG_CMSG_CLOEXEC: c_int = 0x4000_0000;

    #[repr(C)]
    pub struct IoVec {
        pub base: *mut c_void,
        pub len: usize,
    }

    /// `struct msghdr`, as glibc lays it out.
    ///
    /// The padding after `namelen` is the compiler's to insert, which is exactly why this is a
    /// `repr(C)` struct with the real field types rather than a hand-counted byte array: `socklen_t`
    /// is 32 bits and the pointer after it is 64 on an LP64 target, and writing the hole in by hand
    /// is how a struct like this ends up right on one architecture and wrong on the next.
    #[repr(C)]
    pub struct MsgHdr {
        pub name: *mut c_void,
        pub namelen: u32,
        pub iov: *mut IoVec,
        pub iovlen: usize,
        pub control: *mut c_void,
        pub controllen: usize,
        pub flags: c_int,
    }

    /// `struct cmsghdr`.
    #[repr(C)]
    pub struct CmsgHdr {
        pub len: usize,
        pub level: c_int,
        pub ty: c_int,
    }

    /// How many descriptors one message may carry.
    ///
    /// Four: §25's command ring, the observation ring that answers it, and §47.6's two frame-pool
    /// regions — the pixels and the loans.
    ///
    /// A fixed number rather than a slice, because the buffer's size and alignment are what the
    /// kernel reads and a compile-time type is the only version of that a reviewer can check by
    /// looking.
    ///
    /// It was two until §47.6's mapping half existed, and raising it is the whole of what that
    /// cost here: the regions travel in the **same message** as the rings for the reason the
    /// observation ring travels with the commands — a policy handed some of them and not others is
    /// a program running on a world it half has, and both halves of that failure are silent.
    pub const MAX_FDS: usize = 4;

    /// One control message carrying up to [`MAX_FDS`] descriptors.
    ///
    /// `CMSG_SPACE(sizeof(int) * MAX_FDS)` expressed as a type. Four ints are exactly two words on
    /// an LP64 target, so there is no trailing padding to write out; the test below asserts that
    /// rather than assuming it, because it is the target where that stops being true that would
    /// otherwise pass a buffer the kernel refuses.
    #[repr(C)]
    pub struct Control {
        pub header: CmsgHdr,
        pub fds: [c_int; MAX_FDS],
    }

    /// The bytes a message claims when it carries `count` descriptors: `CMSG_LEN`.
    pub const fn cmsg_len(count: usize) -> usize {
        core::mem::size_of::<CmsgHdr>() + core::mem::size_of::<c_int>() * count
    }

    /// `SO_PEERPIDFD` — a pidfd for the process on the other end of a connected Unix socket.
    ///
    /// 77 in `asm-generic/socket.h`, which is x86-64, aarch64, riscv and every architecture a robot
    /// is plausibly built on (§23.5). Four of Linux's architectures number socket options
    /// differently — alpha, mips, parisc and sparc — and on one of those this constant asks for
    /// something else; the caller's failure mode there is a `getsockopt` that refuses, because the
    /// option it would land on does not return a descriptor either.
    pub const SO_PEERPIDFD: c_int = 77;

    /// There is data to read. On a pidfd it means one thing only: the process has exited.
    pub const POLLIN: i16 = 1;

    /// `struct pollfd`.
    #[repr(C)]
    pub struct PollFd {
        pub fd: c_int,
        pub events: i16,
        pub revents: i16,
    }

    extern "C" {
        pub fn sendmsg(socket: c_int, message: *const MsgHdr, flags: c_int) -> isize;
        pub fn recvmsg(socket: c_int, message: *mut MsgHdr, flags: c_int) -> isize;
        pub fn getsockopt(
            socket: c_int,
            level: c_int,
            name: c_int,
            value: *mut c_void,
            len: *mut u32,
        ) -> c_int;
        pub fn poll(fds: *mut PollFd, nfds: u64, timeout: c_int) -> c_int;
    }
}

/// Duplicate a descriptor, as `SCM_RIGHTS` does when one crosses a socket.
///
/// Here because this is where the workspace's fd plumbing lives and because the crates that need
/// it forbid `unsafe`. A test that maps a pool it also created needs two independent descriptors
/// for the same reason a receiver does: both ends close what they hold on drop, and one fd closed
/// twice is a bug that shows up somewhere else entirely.
#[cfg(target_os = "linux")]
pub fn dup(fd: core::ffi::c_int) -> Result<core::ffi::c_int, ShmError> {
    extern "C" {
        fn dup(fd: core::ffi::c_int) -> core::ffi::c_int;
    }
    // SAFETY: one syscall over a descriptor the caller owns. It creates a new one or returns -1.
    let copy = unsafe { dup(fd) };
    if copy < 0 {
        return Err(ShmError::Create(super::sys::errno()));
    }
    Ok(copy)
}

/// Give a descriptor back to the kernel.
///
/// Here for [`dup`]'s reason and no other: the crates that receive descriptors over this socket
/// forbid `unsafe`, so without this the only way for one of them to release an fd it decided not to
/// map is to map it and drop it — which is a syscall, an address-space entry and a page fault to
/// achieve a `close`.
///
/// Errors are not reported, because there is nothing a caller could do with one. `close` fails on a
/// descriptor that was already closed or was never a descriptor, and both of those are defects in
/// the caller rather than conditions in the world; on Linux the descriptor is released even when it
/// returns an error, so retrying would close somebody else's.
#[cfg(target_os = "linux")]
pub fn close(fd: core::ffi::c_int) {
    extern "C" {
        fn close(fd: core::ffi::c_int) -> core::ffi::c_int;
    }
    // SAFETY: one syscall over a descriptor the caller is giving up. It returns a status this
    // deliberately ignores, and touches no memory.
    unsafe {
        close(fd);
    }
}

/// Not supported off Linux, like everything else here.
#[cfg(not(target_os = "linux"))]
pub fn close(_fd: core::ffi::c_int) {}

/// Not supported off Linux, like everything else here.
#[cfg(not(target_os = "linux"))]
pub fn dup(_fd: core::ffi::c_int) -> Result<core::ffi::c_int, ShmError> {
    Err(ShmError::Unsupported)
}

/// A descriptor for the process on the other end of a connected Unix socket.
///
/// # Why the kernel is asked rather than the peer
///
/// §47.6's frame pool lends a slot to a reader and the writer will not reuse it until the loan is
/// released. A reader that dies holding one pins that slot for the life of the robot, and
/// `skynet_frames::Writer::revoke_loans` is the remedy — but it may only be called when the reader
/// is **gone**, because calling it while one is alive takes a slot out from under a live reader,
/// which is the one property the pool exists to provide.
///
/// Every cheaper signal is unsound, and the cheapest is the one that looks best. The socket's
/// end-of-file is not it: `SCM_RIGHTS` **duplicated** the descriptors, so a policy may close the
/// socket and keep the mapping, and revoking there would be exactly the failure above. A timeout is
/// not it either — §47.6 puts no bound on how long a policy may hold a frame, so any deadline short
/// enough to reclaim a crashed reader's slot is short enough to take a working one's.
///
/// What is sound is the process being gone. `SO_PEERCRED` gives its pid, and a pid is a name that
/// can be reused between reading it and opening it — which is precisely why Linux 6.5 added
/// `SO_PEERPIDFD`: the kernel returns a **descriptor** for the peer, resolved inside the kernel,
/// with no window in which the answer can come to mean another process.
///
/// # When it refuses
///
/// [`ShmError::NoPeerIdentity`] on an older kernel (`ENOPROTOOPT`), and the caller's answer to that
/// is to do without the signal rather than to fall back to something racy. A pool that never
/// revokes loses a slot to a crashed policy and says so in `stats()`; a pool that revokes on a guess
/// hands a live reader's buffer to the writer, which is a policy acting on pixels something else is
/// overwriting, three hours in.
///
/// The returned descriptor is the caller's to [`close`].
#[cfg(target_os = "linux")]
pub fn peer_pidfd(socket: core::ffi::c_int) -> Result<core::ffi::c_int, ShmError> {
    let mut pidfd: core::ffi::c_int = -1;
    let mut len = core::mem::size_of::<core::ffi::c_int>() as u32;
    // SAFETY: `getsockopt` writes at most `len` bytes into `pidfd` and updates `len`. Both point at
    // locals of exactly that size, and the socket is one the caller owns.
    let status = unsafe {
        sys::getsockopt(
            socket,
            sys::SOL_SOCKET,
            sys::SO_PEERPIDFD,
            (&mut pidfd) as *mut core::ffi::c_int as *mut core::ffi::c_void,
            &mut len,
        )
    };
    if status != 0 {
        return Err(ShmError::NoPeerIdentity(super::sys::errno()));
    }
    // A success that wrote something other than one descriptor is a kernel this transcription does
    // not understand, and a garbage fd polled every cycle is worse than no signal at all.
    if pidfd < 0 || len as usize != core::mem::size_of::<core::ffi::c_int>() {
        if pidfd >= 0 {
            close(pidfd);
        }
        return Err(ShmError::NoPeerIdentity(0));
    }
    Ok(pidfd)
}

/// Whether the process a pidfd refers to has exited.
///
/// A pidfd becomes readable exactly once, when its process terminates, so `poll` with a zero
/// timeout is the whole question and it never blocks. **`false` on any error**, which is the
/// fail-closed direction for this caller: the fact this answers is used to revoke a loan, and an
/// unreadable answer must not become *"the reader is gone"*.
#[cfg(target_os = "linux")]
pub fn process_exited(pidfd: core::ffi::c_int) -> bool {
    if pidfd < 0 {
        return false;
    }
    let mut entry = sys::PollFd {
        fd: pidfd,
        events: sys::POLLIN,
        revents: 0,
    };
    // SAFETY: one `pollfd`, and the count says one. Zero timeout, so it returns immediately.
    let status = unsafe { sys::poll(&mut entry as *mut sys::PollFd, 1, 0) };
    status > 0 && entry.revents & sys::POLLIN != 0
}

/// Not supported off Linux, like everything else here.
#[cfg(not(target_os = "linux"))]
pub fn peer_pidfd(_socket: core::ffi::c_int) -> Result<core::ffi::c_int, ShmError> {
    Err(ShmError::Unsupported)
}

/// Not supported off Linux, like everything else here.
#[cfg(not(target_os = "linux"))]
pub fn process_exited(_pidfd: core::ffi::c_int) -> bool {
    false
}

/// Send `fds` over `socket`, which must be a connected Unix socket.
///
/// Every descriptor rides in **one** control message, so a receiver gets all of them or none. A
/// policy handed a command ring and not the observation ring would be a closed-loop program
/// commanding blind, and two messages is how that becomes reachable.
///
/// The descriptors are **duplicated** into the receiving process, so the sender still owns its own
/// and should not close them: [`crate::Mapping`] closes on drop, which is what makes the runtime's
/// mappings outlive the hand-off.
#[cfg(target_os = "linux")]
pub fn send_fds(socket: core::ffi::c_int, fds: &[core::ffi::c_int]) -> Result<(), ShmError> {
    if fds.is_empty() || fds.len() > sys::MAX_FDS {
        return Err(ShmError::NoDescriptor);
    }
    let mut payload = *b"R";
    let mut iov = sys::IoVec {
        base: payload.as_mut_ptr() as *mut core::ffi::c_void,
        len: payload.len(),
    };
    let mut carried = [-1; sys::MAX_FDS];
    carried[..fds.len()].copy_from_slice(fds);
    let mut control = sys::Control {
        header: sys::CmsgHdr {
            // `CMSG_LEN`: the header plus the descriptors, without the padding `CMSG_SPACE` adds.
            // The two differ, and sending the larger of them is a message the kernel reads as
            // claiming more data than it carries.
            len: sys::cmsg_len(fds.len()),
            level: sys::SOL_SOCKET,
            ty: sys::SCM_RIGHTS,
        },
        fds: carried,
    };
    let message = sys::MsgHdr {
        name: core::ptr::null_mut(),
        namelen: 0,
        iov: &mut iov,
        iovlen: 1,
        control: &mut control as *mut sys::Control as *mut core::ffi::c_void,
        controllen: sys::cmsg_len(fds.len()),
        flags: 0,
    };

    // SAFETY: `message` points at a `msghdr` this function owns, whose `iov` and `control` point at
    // live locals that outlive the call. `socket` is a descriptor the caller holds open.
    let sent = unsafe { sys::sendmsg(socket, &message, 0) };
    if sent < 0 {
        return Err(ShmError::Send(crate::sys::errno()));
    }
    Ok(())
}

/// Send one descriptor. [`send_fds`] with a single-element slice.
#[cfg(target_os = "linux")]
pub fn send_fd(socket: core::ffi::c_int, fd: core::ffi::c_int) -> Result<(), ShmError> {
    send_fds(socket, &[fd])
}

/// Receive descriptors over `socket`, into `out`, and return how many arrived.
///
/// Blocks until a message arrives or the peer closes: a caller that must not block should set a
/// read timeout or non-blocking mode on its own socket, which is `std`s work and not this
/// function's.
///
/// **Takes ownership of what it returns.** The descriptors are this process's from here, and
/// [`crate::Mapping::adopt`] is what should be given them — that type closes on drop, so a caller
/// that adopts must not also close.
#[cfg(target_os = "linux")]
pub fn recv_fds(socket: core::ffi::c_int, out: &mut [core::ffi::c_int]) -> Result<usize, ShmError> {
    let mut payload = [0u8; 1];
    let mut iov = sys::IoVec {
        base: payload.as_mut_ptr() as *mut core::ffi::c_void,
        len: payload.len(),
    };
    // Zeroed, so a message that carries no control data leaves a header this function can tell
    // apart from one that does: `SCM_RIGHTS` is 1 and `SOL_SOCKET` is 1, and neither is 0.
    let mut control = sys::Control {
        header: sys::CmsgHdr {
            len: 0,
            level: 0,
            ty: 0,
        },
        fds: [-1; sys::MAX_FDS],
    };
    let mut message = sys::MsgHdr {
        name: core::ptr::null_mut(),
        namelen: 0,
        iov: &mut iov,
        iovlen: 1,
        control: &mut control as *mut sys::Control as *mut core::ffi::c_void,
        controllen: core::mem::size_of::<sys::Control>(),
        flags: 0,
    };

    // SAFETY: as above, and every buffer the header points at is a live local of this function.
    let received = unsafe { sys::recvmsg(socket, &mut message, sys::MSG_CMSG_CLOEXEC) };
    if received < 0 {
        return Err(ShmError::Receive(crate::sys::errno()));
    }
    // A peer that closed without sending, and a peer that sent bytes with no ancillary data, land
    // here together. Both are "there is no ring on this socket", which is what the caller has to
    // act on; the distinction between them is not one a policy can do anything about.
    if received == 0
        || message.controllen < sys::cmsg_len(1)
        || control.header.level != sys::SOL_SOCKET
        || control.header.ty != sys::SCM_RIGHTS
    {
        return Err(ShmError::NoDescriptor);
    }
    // How many the sender put in, from the length it declared rather than from the size of the
    // buffer this side happened to offer.
    let carried = (control.header.len.saturating_sub(sys::cmsg_len(0)))
        / core::mem::size_of::<core::ffi::c_int>();
    let carried = carried.min(sys::MAX_FDS);
    if carried == 0 || carried > out.len() {
        return Err(ShmError::NoDescriptor);
    }
    for (slot, fd) in out.iter_mut().zip(control.fds.iter().take(carried)) {
        if *fd < 0 {
            return Err(ShmError::NoDescriptor);
        }
        *slot = *fd;
    }
    Ok(carried)
}

/// Receive exactly one descriptor. [`recv_fds`] with a single-element buffer.
#[cfg(target_os = "linux")]
pub fn recv_fd(socket: core::ffi::c_int) -> Result<core::ffi::c_int, ShmError> {
    let mut one = [-1; 1];
    recv_fds(socket, &mut one)?;
    Ok(one[0])
}

/// Not Linux: there is no `memfd` to pass, so there is nothing to pass it over.
#[cfg(not(target_os = "linux"))]
pub fn send_fd(_socket: core::ffi::c_int, _fd: core::ffi::c_int) -> Result<(), ShmError> {
    Err(ShmError::Unsupported)
}

/// Not Linux.
#[cfg(not(target_os = "linux"))]
pub fn recv_fd(_socket: core::ffi::c_int) -> Result<core::ffi::c_int, ShmError> {
    Err(ShmError::Unsupported)
}

/// Not Linux.
#[cfg(not(target_os = "linux"))]
pub fn send_fds(_socket: core::ffi::c_int, _fds: &[core::ffi::c_int]) -> Result<(), ShmError> {
    Err(ShmError::Unsupported)
}

/// Not Linux.
#[cfg(not(target_os = "linux"))]
pub fn recv_fds(
    _socket: core::ffi::c_int,
    _out: &mut [core::ffi::c_int],
) -> Result<usize, ShmError> {
    Err(ShmError::Unsupported)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::{Access, Mapping};
    use skynet_ring::Command;
    use std::os::fd::AsRawFd;
    use std::os::unix::net::UnixStream;

    /// Both rings, in the one message §25's hand-off sends.
    ///
    /// A socket pair rather than a bound path, in this test and the ones below it: what is under
    /// test is the control message rather than the filesystem, and a path would put file
    /// permissions into a test about `SCM_RIGHTS`. The hosts do the binding.
    #[test]
    fn two_descriptors_cross_in_one_message() {
        let (runtime, policy) = UnixStream::pair().expect("a socket pair");
        let commands = Mapping::create("skynet-commands", 2).expect("create");
        let body = Mapping::create_raw("skynet-body", 2).expect("create raw");

        send_fds(runtime.as_raw_fd(), &[commands.fd(), body.fd()]).expect("send both");

        let mut received = [-1; 2];
        assert_eq!(
            recv_fds(policy.as_raw_fd(), &mut received).expect("receive"),
            2,
            "all of them or none: a policy given commands and no body commands blind",
        );
        assert!(received.iter().all(|fd| *fd >= 0));
        assert_ne!(received[0], received[1]);

        // Both are usable, and the order is the order they were sent in.
        Mapping::adopt(received[0], 2, Access::Write).expect("the command ring");
        Mapping::adopt_raw(received[1], 2, Access::Read).expect("the observation ring");
    }

    #[test]
    fn a_buffer_too_small_for_what_arrived_is_refused_rather_than_truncated() {
        let (runtime, policy) = UnixStream::pair().expect("a socket pair");
        let commands = Mapping::create("skynet-commands", 2).expect("create");
        let body = Mapping::create_raw("skynet-body", 2).expect("create raw");
        send_fds(runtime.as_raw_fd(), &[commands.fd(), body.fd()]).expect("send both");

        let mut one = [-1; 1];
        assert_eq!(
            recv_fds(policy.as_raw_fd(), &mut one),
            Err(ShmError::NoDescriptor),
            "taking the first of two would leave a descriptor open in this process and a caller \
             believing it had everything",
        );
    }

    /// The whole hand-off for one ring, in one process and over a real socket.
    #[test]
    fn a_ring_crosses_a_socket_and_is_writable_on_the_other_side() {
        let (runtime, policy) = UnixStream::pair().expect("a socket pair");

        let created = Mapping::create("skynet-handoff", 2).expect("create");
        send_fd(runtime.as_raw_fd(), created.fd()).expect("send");

        let received = recv_fd(policy.as_raw_fd()).expect("receive");
        assert_ne!(received, created.fd(), "a duplicate, not the same number");

        // The receiving side maps it **writable**: §25's producer is the policy, and the whole
        // point of the hand-off is that the process which did not create the ring is the one that
        // fills it.
        let adopted = Mapping::adopt(received, 2, Access::Write).expect("adopt");
        let mut command = Command {
            seq: 99,
            channels: 2,
            ..Command::default()
        };
        command.values[..2].copy_from_slice(&[0.5, -0.5]);
        command.mask[..2].copy_from_slice(&[1, 1]);
        adopted
            .ring()
            .expect("ring")
            .producer()
            .publish(&command)
            .expect("publish");

        // And the creating side sees it, which is the sentence §80 has been waiting for.
        let mut got = Command::default();
        assert_eq!(
            created.ring().expect("ring").consumer().read(&mut got),
            skynet_ring::Reading::Fresh,
        );
        assert_eq!(got.seq, 99);
        assert_eq!(&got.values[..2], &[0.5, -0.5]);
    }

    /// A peer that says something that is not a descriptor.
    #[test]
    fn a_message_with_no_descriptor_is_refused_by_name() {
        use std::io::Write;
        let (mut sender, receiver) = UnixStream::pair().expect("a socket pair");
        sender.write_all(b"hello").expect("write");

        assert_eq!(
            recv_fd(receiver.as_raw_fd()),
            Err(ShmError::NoDescriptor),
            "bytes with no ancillary data are not a ring, and not a transport failure either",
        );
    }

    /// A peer that closed without sending anything.
    #[test]
    fn a_closed_socket_is_refused_rather_than_hanging() {
        let (sender, receiver) = UnixStream::pair().expect("a socket pair");
        drop(sender);
        assert_eq!(recv_fd(receiver.as_raw_fd()), Err(ShmError::NoDescriptor));
    }

    /// A socket pair's peer is **this** process, whatever a descriptor was later handed to.
    ///
    /// The negative half of the pair of facts loan revocation rests on, and the one that must never
    /// be wrong: a pidfd for something that is running may not read as exited, because that reading
    /// is what takes a slot out from under a live reader.
    ///
    /// The second assertion is the whole argument for using a pidfd rather than the socket. Closing
    /// one end is not the peer dying — `SCM_RIGHTS` duplicated the descriptors, so a policy may do
    /// exactly this and go on reading the pool — and an end-of-file signal would have revoked here.
    #[test]
    fn a_running_peer_does_not_read_as_gone() {
        let (ours, theirs) = UnixStream::pair().expect("a socket pair");
        match peer_pidfd(ours.as_raw_fd()) {
            Ok(pidfd) => {
                assert!(!process_exited(pidfd), "this process is running");
                drop(theirs);
                assert!(
                    !process_exited(pidfd),
                    "a closed socket is not a dead process, and revoking on it would be the bug",
                );
                close(pidfd);
            }
            // Older than Linux 6.5. The caller does without the signal, and so does this test:
            // asserting a refusal here would make it pass for the wrong reason on a new kernel.
            Err(ShmError::NoPeerIdentity(_)) => {}
            Err(other) => panic!("unexpected: {other}"),
        }
    }

    /// And the positive half, which needs a peer that is another process and really exits.
    ///
    /// A socket **pair** cannot show this: both ends are created by one process, so its peer
    /// credentials name this one and a child that inherits a descriptor does not become the peer.
    /// It takes a real `connect`, which is what `handoff::Port` accepts on a robot — so this test
    /// re-executes the test binary as the client, named exactly, and talks to it over a bound path.
    ///
    /// The child blocks on a read until this side drops the connection, which removes the race:
    /// `SO_PEERPIDFD` fails with `ESRCH` once the peer is gone, so the descriptor has to be taken
    /// while it is alive — which is also exactly when `Port` takes it.
    #[test]
    fn a_peer_that_exits_reads_as_gone() {
        let path = std::env::temp_dir().join(format!("skynet-peer-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = match std::os::unix::net::UnixListener::bind(&path) {
            Ok(listener) => listener,
            // A temp directory that will not hold a socket is not a failure of this crate.
            Err(_) => return,
        };

        let mut child = std::process::Command::new(std::env::current_exe().expect("this binary"))
            .args([
                "--exact",
                "pass::tests::the_peer_this_test_needs",
                "--quiet",
            ])
            .env(PEER_SOCKET, &path)
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("the test binary is executable");

        let (stream, _) = listener.accept().expect("the child connects");
        let pidfd = match peer_pidfd(stream.as_raw_fd()) {
            Ok(pidfd) => pidfd,
            Err(ShmError::NoPeerIdentity(_)) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = std::fs::remove_file(&path);
                return;
            }
            Err(other) => panic!("unexpected: {other}"),
        };
        assert!(
            !process_exited(pidfd),
            "the child is blocked on a read, so it is alive",
        );

        drop(stream);
        child.wait().expect("the child is ours to reap");
        assert!(
            process_exited(pidfd),
            "the peer exited and was reaped; a pidfd for it must read as gone",
        );
        close(pidfd);
        let _ = std::fs::remove_file(&path);
    }

    /// The other side of the test above, run as its own process.
    ///
    /// Not a test of anything on its own, and it does nothing at all unless the environment names a
    /// socket — which only [`a_peer_that_exits_reads_as_gone`] does. A test that spawned a separate
    /// helper binary would need a second build target for eight lines.
    #[test]
    fn the_peer_this_test_needs() {
        let Some(path) = std::env::var_os(PEER_SOCKET) else {
            return;
        };
        let mut stream = UnixStream::connect(std::path::Path::new(&path)).expect("the listener");
        // Blocks until the other side drops it, which is what makes the timing above deterministic.
        let mut byte = [0u8; 1];
        let _ = std::io::Read::read(&mut stream, &mut byte);
    }

    /// Names the socket the helper above connects to, and is set by nothing else.
    const PEER_SOCKET: &str = "SKYNET_PASS_PEER_SOCKET";

    /// The control buffer is `CMSG_SPACE(sizeof(int))`, which is what the kernel expects.
    #[test]
    fn the_control_message_is_the_size_the_macro_would_have_computed() {
        let header = core::mem::size_of::<sys::CmsgHdr>();
        let word = core::mem::size_of::<usize>();
        let fds = core::mem::size_of::<core::ffi::c_int>() * sys::MAX_FDS;
        // CMSG_SPACE rounds the header and the payload up to the alignment of `size_t`.
        let expected = header + fds.div_ceil(word) * word;
        assert_eq!(
            core::mem::size_of::<sys::Control>(),
            expected,
            "the buffer is what the kernel reads: a byte short is EINVAL and a byte over is a lie",
        );
        assert_eq!(core::mem::align_of::<sys::Control>(), word);
        assert_eq!(sys::cmsg_len(1), header + 4);
        assert_eq!(sys::cmsg_len(2), header + 8);
        // The four §47.6 made necessary: two rings, and a frame pool's pixels and loans.
        assert_eq!(sys::cmsg_len(4), header + 16);
        assert_eq!(
            sys::cmsg_len(sys::MAX_FDS),
            core::mem::size_of::<sys::Control>()
        );
    }
}

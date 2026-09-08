//! §25's hand-off, from the side that creates the ring.
//!
//! > The runtime creates the ring and hands the descriptor to the sandboxed policy.
//!
//! This is the runtime's half of that sentence. It creates the mappings, listens on a Unix socket,
//! and gives the descriptors to whatever connects. `skynet_ring_shm::pass` carries them; everything
//! here is `std` and there is no `unsafe` in this file, which is the split the shm crate's header
//! argues for.
//!
//! # Two rings, one message — and a frame pool beside them when there is one
//!
//! A policy needs both: §25's command ring, which it writes, and the observation ring, which it
//! reads. They cross in a **single** `SCM_RIGHTS` message, so a policy receives both or neither —
//! two messages would make a closed-loop program that got the commands and not the body reachable,
//! and that is a policy commanding blind. The order is the order they are sent in: commands first,
//! then the body.
//!
//! §47.6's frame pool crosses in that same message, when the runtime has one: pixels third, loans
//! fourth. Same argument, one section along — a policy handed pixels and no loans region can read a
//! frame but cannot stop the writer taking the slot out from under it, which is the one property
//! the pool exists to provide. The count is what tells the receiver which arrangement it got: two
//! descriptors is a robot with no camera, four is a robot with one, and neither is a degraded
//! version of the other.
//!
//! The pool is **not** created here. `Port` creates the rings because §25 says the runtime does;
//! the pool is created by whatever captures into it and is only carried through, because a port
//! that created an empty pool would be answering a question about hardware that it cannot see.
//!
//! The directions are opposite and deliberate. The policy maps the command ring **writable** and
//! the runtime maps it read-only; the runtime maps the observation ring writable and the policy
//! reads it. Neither side has to agree to anything: the page tables enforce both.
//!
//! # Level-triggered, like everything else that waits for something
//!
//! The port does not block for a policy. A control loop that stopped to wait for a producer would be
//! a loop that misses its deadline because nothing is commanding it — which is precisely the state
//! §7.6 says is normal on a robot running nothing. So the listener is non-blocking, it is offered
//! once per cycle, and a policy that starts a minute later gets the descriptor a minute later.
//!
//! # It hands the same descriptor to everyone who asks, and that is not a hole
//!
//! `SCM_RIGHTS` duplicates the descriptor, so two policies connecting would both be able to write
//! one ring — two producers on a structure whose correctness argument is that there is one. That is
//! §25.2's *"a trajectory neither was trained on and neither can be held responsible for"*, and the
//! answer to it is **not** here: a robot runs one commanding package because §37.1's transport stage
//! resolves one plan, and `skynetd_io`'s `shape::supervising` refuses two commanding packages by
//! name rather than picking one. This port is the mechanism; the arbitration is the daemon's, and
//! putting a second copy of it here would be a second answer that could disagree with the first.
//!
//! What this port does report is **how many policies have taken it**, because a robot that handed
//! its ring to two processes is a fact an operator should be able to read rather than infer.

use std::io;
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use skynet_ring_shm::pass;
use skynet_ring_shm::{Mapping, ShmError};

/// The runtime's side of the hand-off: a mapping, and a socket policies connect to.
#[cfg(unix)]
#[derive(Debug)]
pub struct Port {
    listener: UnixListener,
    path: PathBuf,
    handed: usize,
    refused: usize,
}

/// Why a port could not be opened.
#[derive(Debug)]
pub enum PortError {
    /// The ring itself could not be created.
    Ring(ShmError),
    /// The socket could not be bound.
    Bind(io::Error),
    /// The observation ring could not be written into its mapping.
    ///
    /// A defect rather than a condition: the mapping was created at the size this crate asked for,
    /// so a header that will not fit it is arithmetic that disagrees with itself.
    Observation(skynet_obs::ObsError),
    /// Not a Unix: there is no socket to hand a descriptor over.
    ///
    /// A robot runs Linux (§23.5) and this binary also runs on a developer's machine, where the
    /// honest answer to `--policy-socket` is a refusal rather than a ring nobody can reach.
    Unsupported,
}

impl std::fmt::Display for PortError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PortError::Ring(error) => write!(f, "the shared ring could not be created: {error}"),
            PortError::Bind(error) => write!(
                f,
                "the policy socket could not be bound: {error}. A path left behind by a process \
                 that was killed is the usual cause, and it is safe to delete once nothing is \
                 listening on it"
            ),
            PortError::Observation(error) => write!(
                f,
                "the observation ring could not be written: {}",
                error.as_str()
            ),
            PortError::Unsupported => write!(
                f,
                "a command ring is handed over a Unix socket and this is not a Unix. A robot runs \
                 Linux (§23.5); on this machine the ring can only be private to this process"
            ),
        }
    }
}

impl std::error::Error for PortError {}

#[cfg(unix)]
impl Port {
    /// Create both rings and bind the socket policies will connect to.
    ///
    /// Returns the mappings, because they are the caller's to hold: a ring borrows from its
    /// mapping, and a `Port` that owned them would hand out rings whose lifetime nobody could name.
    ///
    /// The observation ring is sized in slots like the command ring, and both are the runtime's
    /// numbers rather than the policy's — a policy is told what it received.
    pub fn open(
        path: &Path,
        slot_count: usize,
        observation_slots: usize,
    ) -> Result<(Mapping, Mapping, Port), PortError> {
        let mapping = Mapping::create("skynet-command", slot_count).map_err(PortError::Ring)?;
        // Raw, because the observation ring is not a command ring: `skynet-obs` writes its own
        // header, with its own magic, so that a consumer built for commands which opened one is
        // refused at the first word rather than reading joint positions as setpoints.
        let observations = Mapping::create_raw("skynet-observation", observation_slots)
            .map_err(PortError::Ring)?;
        skynet_obs::Ring::create(observations.words(), observation_slots)
            .map_err(PortError::Observation)?;
        // A path left by a process that was killed is a bind failure with a confusing message, and
        // it is the ordinary state after a crash. Removing it is safe here in a way that removing a
        // *lock* is not (§26.2): a socket file grants nothing and holds no evidence — if another
        // process really is listening, the bind below fails and says so.
        let _ = std::fs::remove_file(path);
        let listener = UnixListener::bind(path).map_err(PortError::Bind)?;
        listener.set_nonblocking(true).map_err(PortError::Bind)?;
        Ok((
            mapping,
            observations,
            Port {
                listener,
                path: path.to_path_buf(),
                handed: 0,
                refused: 0,
            },
        ))
    }

    /// Hand the descriptors to anything waiting, and return how many took them this call.
    ///
    /// `frames` is §47.6's pool when the runtime has one and `None` when it does not, and that is
    /// the *only* difference the receiver sees: with a pool the message carries four descriptors,
    /// without one it carries two. There is deliberately no third arrangement — no pool of zero
    /// slots, no descriptor for a region nobody wrote — because a receiver's first question is
    /// "have I got pixels", and an empty pool answers it wrongly.
    ///
    /// Never blocks and never fails the caller: a policy that connected and went away before the
    /// send is a policy that did not start, which is a thing the loop keeps running through.
    pub fn offer(
        &mut self,
        commands: &Mapping,
        observations: &Mapping,
        frames: Option<&skynet_frames::Writer>,
    ) -> usize {
        // Built once rather than per connection: the descriptors are the same for every policy
        // that connects, and `SCM_RIGHTS` duplicates them at the kernel rather than here.
        let mut carried = [commands.fd(), observations.fd(), -1, -1];
        let count = match frames {
            None => 2,
            Some(pool) => {
                let [pixels, loans] = pool.descriptors();
                carried[2] = pixels;
                carried[3] = loans;
                4
            }
        };
        let mut handed = 0;
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => match pass::send_fds(stream.as_raw_fd(), &carried[..count]) {
                    Ok(()) => {
                        self.handed += 1;
                        handed += 1;
                    }
                    Err(_) => self.refused += 1,
                },
                // Nothing waiting. The ordinary result on almost every cycle.
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.refused += 1;
                    break;
                }
            }
        }
        handed
    }

    /// How many policies have taken the descriptor.
    pub fn handed(&self) -> usize {
        self.handed
    }

    /// How many connections could not be given one.
    pub fn refused(&self) -> usize {
        self.refused
    }

    /// Where it is listening.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(unix)]
impl Drop for Port {
    fn drop(&mut self) {
        // A socket file outlives the process that bound it, and the next run's `bind` would fail on
        // it. Removed here, and removed again before the next bind, because a process that is
        // killed never reaches this.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// The same surface where there is no Unix socket, so a caller needs no `cfg` of its own.
///
/// Every method that would open something answers [`PortError::Unsupported`]; the rest answer what
/// is true of a port that was never opened. The workspace's own idiom, and `skynet-ring-shm`'s
/// header states it: *"on any other target every function here returns `Unsupported` rather than
/// failing to compile, so the workspace still builds and tests on a developer's machine"*.
#[cfg(not(unix))]
#[derive(Debug)]
pub struct Port {
    path: PathBuf,
}

#[cfg(not(unix))]
impl Port {
    /// Refused, and it says why.
    pub fn open(
        _path: &Path,
        _slot_count: usize,
        _observation_slots: usize,
    ) -> Result<(Mapping, Mapping, Port), PortError> {
        Err(PortError::Unsupported)
    }

    /// Nothing can connect, so nothing takes anything.
    pub fn offer(
        &mut self,
        _commands: &Mapping,
        _observations: &Mapping,
        _frames: Option<&skynet_frames::Writer>,
    ) -> usize {
        0
    }

    /// None, always.
    pub fn handed(&self) -> usize {
        0
    }

    /// None, always.
    pub fn refused(&self) -> usize {
        0
    }

    /// Where it would have listened.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use skynet_ring::{Command, Reading};
    use skynet_ring_shm::Access;
    use std::os::unix::net::UnixStream;

    fn scratch(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("skynet-handoff-{name}.sock"));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn a_policy_that_connects_gets_a_ring_it_can_write() {
        let path = scratch("one");
        let (mapping, body, mut port) = Port::open(&path, 4, 4).expect("open");
        assert_eq!(
            port.offer(&mapping, &body, None),
            0,
            "nobody has connected yet"
        );

        let client = UnixStream::connect(&path).expect("connect");
        assert_eq!(port.offer(&mapping, &body, None), 1);
        assert_eq!(port.handed(), 1);

        let mut fds = [-1; 2];
        assert_eq!(
            pass::recv_fds(client.as_raw_fd(), &mut fds).expect("receive"),
            2,
            "both rings, in one message",
        );
        let adopted = Mapping::adopt(fds[0], 4, Access::Write).expect("adopt");
        let mut command = Command {
            seq: 7,
            channels: 1,
            ..Command::default()
        };
        command.values[0] = 1.25;
        command.mask[0] = 1;
        adopted
            .ring()
            .expect("ring")
            .producer()
            .publish(&command)
            .expect("publish");

        // The runtime reads what the policy wrote, across two mappings of one file.
        let mut got = Command::default();
        assert_eq!(
            mapping.ring().expect("ring").consumer().read(&mut got),
            Reading::Fresh
        );
        assert_eq!(got.seq, 7);
        assert_eq!(got.values[0], 1.25);
    }

    /// The body ring is the runtime's to write and the policy's to read.
    #[test]
    fn the_observation_ring_arrives_openable_and_read_only_on_the_policy_side() {
        let path = scratch("body");
        let (commands, body, mut port) = Port::open(&path, 4, 4).expect("open");

        // What the runtime publishes: the state a driver reported.
        let ring = skynet_obs::Ring::open(body.words()).expect("the runtime's own view");
        let mut producer = ring.producer();
        let mut sample = skynet_obs::Observation {
            stamp_ns: 1,
            seq: 1,
            joints: 2,
            ..skynet_obs::Observation::default()
        };
        sample.position[0] = 0.5;
        sample.position[1] = -0.25;
        producer.publish(&sample).expect("publish");

        let client = UnixStream::connect(&path).expect("connect");
        assert_eq!(port.offer(&commands, &body, None), 1);
        let mut fds = [-1; 2];
        assert_eq!(
            pass::recv_fds(client.as_raw_fd(), &mut fds).expect("receive"),
            2
        );

        let adopted = Mapping::adopt_raw(fds[1], 4, Access::Read).expect("adopt the body");
        let seen = skynet_obs::Ring::open(adopted.words()).expect("a ring on the other side");
        let mut into = skynet_obs::Observation::default();
        assert_eq!(seen.consumer().read(&mut into), skynet_obs::Reading::Fresh);
        assert_eq!(into.joints, 2);
        assert_eq!(into.position[0], 0.5);
        assert_eq!(into.position[1], -0.25);
    }

    #[test]
    fn the_socket_is_gone_when_the_port_is() {
        let path = scratch("drop");
        let (_mapping, _body, port) = Port::open(&path, 2, 2).expect("open");
        assert!(path.exists());
        drop(port);
        assert!(
            !path.exists(),
            "a socket left behind is the next bind's error"
        );
    }

    #[test]
    fn a_path_left_by_a_killed_process_is_not_a_permanent_refusal() {
        let path = scratch("stale");
        std::fs::write(&path, b"not a socket").expect("writable");
        let opened = Port::open(&path, 2, 2);
        assert!(
            opened.is_ok(),
            "a socket file grants nothing and holds no evidence, so it is not a lock (§26.2)"
        );
    }
}

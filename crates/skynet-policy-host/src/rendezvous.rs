//! §25's hand-over: taking the rings a runtime offers over its socket.
//!
//! # Why this is in the library and not in `skynetp`
//!
//! It was in the binary, which made it unreachable by the one kind of program that most needs it: a
//! **skill or policy package**, whose entry point the supervisor starts with `SKYNET_RING_SOCKET`
//! in its environment and which then has to do exactly this. A third party writing one had two
//! options, and both were bad — copy ninety lines of descriptor passing, or link a binary. Copies
//! is what would have happened, and the copies would have diverged on the branch nobody exercises:
//! the four-descriptor case, where §47.6's frame pool arrives with the rings.
//!
//! So the hand-over is published, and `skynetp` is now one caller of it rather than its owner. What
//! a package's entry point has to write is the backend and the loop — the parts that are its own.
//!
//! # The environment the supervisor sets, which is the whole interface
//!
//! ```text
//! SKYNET_RING_SOCKET    the path to connect to
//! SKYNET_RING_CHANNELS  how many channels the runtime opened the ring for
//! ```
//!
//! Both are set by `skynetd` for the package it is supervising as the policy, and for nothing else.
//! A package that finds them absent is not the commanding policy on this robot, and
//! [`from_environment`] says which of the two was missing rather than defaulting one — a channel
//! count guessed wrong is a ring addressed at the wrong stride, which reads as plausible numbers in
//! the wrong slots rather than as an error.

use skynet_ring_shm::Mapping;

/// Connect to a runtime's socket and take the ring it offers (§25).
///
/// Three failures, and each sends a person somewhere different:
///
/// * **the socket is not there** — the runtime is not running, or is running without
///   `--policy-socket`;
/// * **no descriptor arrived** — something is listening on that path and it is not a runtime;
/// * **what arrived is not a ring** — a descriptor crossed and its header is not one this build can
///   read, which is a version disagreement rather than a network problem.
///
/// The slot count is the runtime's. It is passed in here as what to expect, and
/// [`Mapping::adopt`] validates the header before it returns — so a policy cannot choose the
/// geometry, and one that is told the wrong number is refused rather than mapping a ring at the
/// wrong stride.
#[cfg(unix)]
pub fn adopt_from(path: &std::path::Path, slots: usize) -> Result<Adopted, String> {
    use skynet_ring_shm::Access;
    use std::os::fd::AsRawFd;

    let socket = std::os::unix::net::UnixStream::connect(path)
        .map_err(|error| format!("no runtime is listening on {}: {error}", path.display()))?;
    let mut fds = [-1; 4];
    let taken = skynet_ring_shm::pass::recv_fds(socket.as_raw_fd(), &mut fds)
        .map_err(|error| format!("{} offered no ring: {error}", path.display()))?;
    // Two or four, and nothing else. Two is a runtime with no frame pool, four is one with §47.6's
    // — the count is the whole of how the arrangement is announced, which is why any other number
    // is an error rather than a pool this side quietly does without. A runtime that sent three sent
    // pixels with no loans region, and a reader with no loans cannot stop a slot moving under it.
    if taken != 2 && taken != 4 {
        return Err(format!(
            "{} sent {taken} descriptor(s). A policy needs two — the command ring it writes and \
             the observation ring it reads — or four, when the frame pool comes with them",
            path.display()
        ));
    }
    let commands = Mapping::adopt(fds[0], slots, Access::Write).map_err(|error| {
        format!(
            "the command descriptor from {} is not a ring: {error}",
            path.display()
        )
    })?;
    // Read-only, and raw: the observation ring has its own magic and its own header, and mapping it
    // as a command ring would be the confusion §25 gives it a different format to prevent.
    let body = Mapping::adopt_raw(fds[1], slots, Access::Read).map_err(|error| {
        format!(
            "the observation descriptor from {} could not be mapped: {error}",
            path.display()
        )
    })?;
    // The pool, when one came with them. Its shape is read out of its own header rather than from
    // an environment variable beside `SKYNET_RING_CHANNELS`, because a second copy of the slot
    // count is a number that can disagree with the region it describes — and the failure when it
    // does is not an error, it is this process addressing the wrong offset for every slot past the
    // first and finding plausible pixels there.
    let frames = match taken {
        4 => Some(
            skynet_frames::Reader::adopt_declared(fds[2], fds[3]).map_err(|error| {
                format!(
                    "the frame descriptors from {} are not a pool: {error}",
                    path.display()
                )
            })?,
        ),
        _ => None,
    };
    Ok(Adopted {
        commands,
        body,
        frames,
    })
}

/// What arrived over §25's socket.
///
/// A struct rather than a tuple because there are now three of them and the third is optional, and
/// `(Mapping, Mapping, Option<Reader>)` at a call site is three positions a reader has to count
/// rather than three names they can read.
pub struct Adopted {
    /// The command ring, mapped **writable**: this process is its only writer.
    pub commands: Mapping,
    /// The observation ring, mapped read-only. Without it a policy commands blind.
    pub body: Mapping,
    /// §47.6's pool, when the runtime offered one. `None` is a robot with no camera, which is most
    /// of them, and is not a degraded version of having one.
    pub frames: Option<skynet_frames::Reader>,
}

/// Not Unix: there is no socket to receive a descriptor over.
#[cfg(not(unix))]
pub fn adopt_from(_path: &std::path::Path, _slots: usize) -> Result<Adopted, String> {
    Err("a ring is handed over a Unix socket, and this is not a Unix (§23.5)".to_string())
}

/// A ring hand-over that succeeded, and the channel count that came with it.
///
/// The two arrive from different places and both are needed to command: the descriptors come over
/// the socket and the channel count comes from the environment, because it is a property of the
/// *body* rather than of the mapping and the ring header does not carry it.
pub struct Ready {
    /// The rings, and §47.6's pool when there was one.
    pub adopted: Adopted,
    /// How many channels this body has, from `SKYNET_RING_CHANNELS`.
    pub channels: usize,
}

/// The two variables a supervisor sets for the package it is running as the policy.
pub const SOCKET_VAR: &str = "SKYNET_RING_SOCKET";

/// See [`SOCKET_VAR`].
pub const CHANNELS_VAR: &str = "SKYNET_RING_CHANNELS";

/// Take the rings a supervisor has offered this process, reading where from out of the environment.
///
/// This is what a skill or policy package's entry point calls first. `skynetd` sets both variables
/// for the package it is supervising as the policy and for no other, so their absence is a fact
/// about this process's role rather than a misconfiguration, and the error says so.
///
/// # `expect_slots` is required, and a wrong one is refused rather than mapped
///
/// The ring's geometry is the runtime's. It is passed here as what to expect and
/// [`Mapping::adopt`] validates it against the header before returning, so a caller that expects
/// the wrong number is refused. That is the reason this is a required argument with no default: a
/// default would be this crate quietly asserting a geometry it does not own, and the failure it
/// would hide — a ring addressed at the wrong stride — does not look like an error. It looks like
/// plausible numbers in the wrong slots.
pub fn from_environment(expect_slots: usize) -> Result<Ready, String> {
    let socket = std::env::var(SOCKET_VAR).map_err(|_| {
        format!(
            "{SOCKET_VAR} is not set, so nothing has offered this process a ring. A supervisor \
             sets it for the package it is running as the policy and for no other, so this is \
             most likely a package that is installed and is not the one commanding."
        )
    })?;
    let raw = std::env::var(CHANNELS_VAR).map_err(|_| {
        format!(
            "{SOCKET_VAR} is set and {CHANNELS_VAR} is not. The count is not guessed here: a \
             channel count guessed wrong is a ring read at the wrong stride, which yields \
             plausible numbers in the wrong slots rather than an error."
        )
    })?;
    let channels: usize = raw
        .parse()
        .map_err(|_| format!("{CHANNELS_VAR} is `{raw}`, which is not a count"))?;

    let adopted = adopt_from(std::path::Path::new(&socket), expect_slots)?;
    Ok(Ready { adopted, channels })
}

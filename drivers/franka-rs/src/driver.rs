//! One arm, over one FCI connection — a Rust port of
//! `drivers/franka/skynet_franka/bridge.py`, which is the executable specification.
//!
//! §24's five entry points onto the two transports in [`crate::protocol`]. What this driver is
//! careful about is the three places where the FCI's shape and §24's shape disagree.
//!
//! **A Franka is commanded only while a motion is running.** The UDP stream carries a
//! `RobotCommand` every tick, and the Control box reads the field belonging to the motion
//! generator the `Move` command started. Outside a motion the stream still runs and nothing in it
//! is read. So `write_command` cannot simply write: the first command in a mode starts a motion
//! in that mode, a command in a different mode stops the running one and starts another, and a
//! body with no motion running reports its state and refuses commands.
//!
//! **`emergency_stop` is `StopMove` and nothing else.** §24's second rule is that the stop must
//! work from a signal handler and from a dying process — so it is one `send` of twelve bytes on
//! an already open socket, with no reply waited for, no lock taken and no allocation. It is not
//! `AutomaticErrorRecovery`, it is not a torque of zero, and it does not wait for the robot to
//! confirm: a stop path that waits is not a stop path. What it is **not** is the hardware stop.
//! Franka's own stop is a button wired to the Control box, and this driver cannot press it — the
//! Python driver's README says so under what a bench session has to measure.
//!
//! **The 41 error bits are one fault, surfaced verbatim.** §24 says a vendor fault code is passed
//! through and not interpreted, so a latched error sets `SKR_FLAG_FAULT` and `fault_code` carries
//! the index of the first bit.
//!
//! ## Where the port knowingly differs from the Python driver
//!
//! * The stop path sends on a `TcpStream` cloned at open and held outside the connection lock, so
//!   the stop takes no lock at all. Python sends on the shared socket under the GIL, which is a
//!   lock it does not name.
//! * `write_command` re-checks the estop latch after acquiring the connection lock, closing a race
//!   the Python driver has: a stop landing between the latch check and the send.
//! * A malformed `port` (config or environment) is `SKR_EINVAL`. Python's `int()` raises there,
//!   which is a traceback out of `open` rather than a refusal.
//! * None in the clock, since F65 in `DRY-COVERAGE-PLAN.md` was decided: both stamp
//!   `CLOCK_MONOTONIC_RAW`, the clock §24 and §35.1 document. The port first matched the Python
//!   driver's `CLOCK_MONOTONIC` and named the disagreement; the owner moved the tree to the text.
//!   The reading is `skynet_rt::monotonic_raw_ns`, the workspace's one, rather than a copy of it
//!   here — a local copy is what stopped the workspace linking on Windows (F72).
//!
//! ## What the port does not claim
//!
//! This is 6.6's whole point stated as a negative: the driver is in a language that permits the
//! Host ceiling, and it does not approach it. `read_state` blocks on the socket with a timeout,
//! `write_command` transacts on the calling thread, and the connection sits behind a mutex — the
//! Python driver's structure, translated. A background thread with a lock-free ring, and a
//! wait-free `read_state` over it, is the follow-up this port enables; claiming it here would be
//! claiming timing behaviour no machine in this phase can measure.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use skynet_json::Json;
use skynet_rhal::{flags, status, Homing, Mode};

use crate::protocol as fci;
use crate::protocol::RobotState;

/// Joint travel, radians, from Franka Robotics' published joint-limit table for the FR3/Panda arm.
/// A travel limit is a *kinematic* fact about the arm and is what §5.2 asks a descriptor to carry.
/// These stay narrower than the vendor table (which `JOINT_RATINGS` cites beside its own
/// figures): travel is where the arm may go, and the margin is the driver's, not the vendor's.
pub const JOINT_LIMITS: [(f64, f64); fci::JOINTS] = [
    (-2.7437, 2.7437),
    (-1.7837, 1.7837),
    (-2.9007, 2.9007),
    (-3.0421, -0.1518),
    (-2.8065, 2.8065),
    (0.5445, 4.5169),
    (-3.0159, 3.0159),
];

/// Joint velocity (rad/s) and effort (Nm) ratings, transcribed exactly — no margin, so the
/// citation matches the numbers — from Franka Robotics' own `franka_description` repository,
/// `robots/fr3/joint_limits.yaml`: the vendor's joint-limit table, which their own stack treats
/// as the limits.
pub const JOINT_RATINGS: [(f64, f64); fci::JOINTS] = [
    (2.62, 87.0),
    (2.62, 87.0),
    (2.62, 87.0),
    (2.62, 87.0),
    (5.26, 12.0),
    (4.18, 12.0),
    (5.26, 12.0),
];

/// The watchdog window, in milliseconds: ten FCI periods at the 1 kHz the Control box reads.
/// Ten missed commands is a dead policy rather than jitter, and §46.3's nesting wants the window
/// comfortably above the worst-case cycle.
pub const WATCHDOG_MS: f64 = 10.0;

/// §5.1 roles, one per joint, naming the **axis each one actually turns about** rather than where
/// it sits in the list.
pub const ROLES: [&str; fci::JOINTS] = [
    "right_arm.shoulder.yaw",
    "right_arm.shoulder.pitch",
    "right_arm.upper_arm.roll",
    "right_arm.elbow.flex",
    "right_arm.forearm.roll",
    "right_arm.wrist.pitch",
    "right_arm.wrist.roll",
];

/// How `open` resolves its settings: the config first, then the environment, then the default —
/// the order every other setting in this repository resolves in.
#[derive(Debug, Clone)]
pub struct Config {
    /// The Control box's host: config, then `SKYNET_FRANKA_HOST`, then `127.0.0.1`.
    pub host: String,
    /// The command port: config, then `SKYNET_FRANKA_PORT`, then the FCI's own 1337.
    pub port: u16,
    /// The descriptor's `robot_slug`.
    pub slug: String,
    /// Whether the body is simulated. Defaults true: declaring simulated about real caps the
    /// tier, while the reverse presents sim as evidence (§35.3).
    pub simulator: bool,
    /// The interface the UDP socket binds to.
    pub bind: String,
    /// The TCP connect and command timeout.
    pub timeout: Duration,
}

impl Config {
    /// Resolve the config JSON against the environment and the defaults. Malformed JSON, and a
    /// port that is present but not a port, are `SKR_EINVAL`.
    pub fn parse(config: &str) -> Result<Config, i32> {
        let settings = if config.trim().is_empty() {
            Json::obj()
        } else {
            Json::parse(config).map_err(|_| status::EINVAL)?
        };
        let host = settings
            .get("host")
            .and_then(Json::as_str)
            .map(str::to_string)
            .or_else(|| std::env::var("SKYNET_FRANKA_HOST").ok())
            .unwrap_or_else(|| "127.0.0.1".to_string());
        // A `port` that is present and neither a number in range nor parseable is refused
        // rather than defaulted: silently connecting to 1337 when the operator named a box is how
        // a driver moves the wrong arm.
        let port = match settings.get("port") {
            None => match std::env::var("SKYNET_FRANKA_PORT") {
                Err(_) => fci::COMMAND_PORT,
                Ok(raw) => raw.parse::<u16>().map_err(|_| status::EINVAL)?,
            },
            Some(raw) => {
                let value = raw.as_f64().ok_or(status::EINVAL)?;
                if !(0.0..=65535.0).contains(&value) {
                    return Err(status::EINVAL);
                }
                value as u16
            }
        };
        let slug = settings
            .get("robot_slug")
            .and_then(Json::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "franka-fr3".to_string());
        let simulator = settings
            .get("simulator")
            .and_then(Json::as_bool)
            .or_else(|| {
                std::env::var("SKYNET_FRANKA_SIM")
                    .ok()
                    .map(|raw| raw == "1")
            })
            .unwrap_or(true);
        let bind = settings
            .get("bind")
            .and_then(Json::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "127.0.0.1".to_string());
        let timeout = settings
            .get("timeout_s")
            .and_then(Json::as_f64)
            .map(Duration::from_secs_f64)
            .unwrap_or_else(|| Duration::from_secs_f64(3.0));
        Ok(Config {
            host,
            port,
            slug,
            simulator,
            bind,
            timeout,
        })
    }
}

/// One arm, over one FCI connection.
///
/// Held behind a mutex by the crate root: the Python driver's structure, translated — see the
/// module header for what that costs and what follows it.
pub struct Connection {
    tcp: Option<TcpStream>,
    udp: Option<UdpSocket>,
    command_id: u32,
    opened: bool,
    seq: u32,
    state: Option<RobotState>,
    motion_mode: Option<u32>,
    slug: String,
    simulator: bool,
    buffer: Vec<u8>,
    peer: Option<SocketAddr>,
    /// §24's position hold, per joint: the last position setpoint written, and the target a
    /// masked joint is held at (`None` while it is commanded) — see [`resolve_hold`].
    setpoint: [Option<f64>; fci::JOINTS],
    hold: [Option<f64>; fci::JOINTS],
}

impl Connection {
    /// A closed connection. Sockets, state and motion arrive with [`Connection::open`].
    pub fn new() -> Connection {
        Connection {
            tcp: None,
            udp: None,
            command_id: 0,
            opened: false,
            seq: 0,
            state: None,
            motion_mode: None,
            slug: "franka-fr3".to_string(),
            simulator: true,
            buffer: Vec::new(),
            peer: None,
            setpoint: [None; fci::JOINTS],
            hold: [None; fci::JOINTS],
        }
    }

    /// Whether the handshake completed and the first state arrived.
    pub fn is_open(&self) -> bool {
        self.opened
    }

    /// Open the connection. On success, hands back a `TcpStream` cloned for the stop path — the
    /// socket `emergency_stop` sends on without taking the connection lock.
    pub fn open(&mut self, config: &Config) -> Result<TcpStream, i32> {
        let udp = UdpSocket::bind((config.bind.as_str(), 0)).map_err(|error| {
            eprintln!(
                "skynet-franka: could not bind UDP on {}: {error:?}",
                config.bind
            );
            status::EIO
        })?;
        udp.set_read_timeout(Some(Duration::from_secs(1))).ok();
        let udp_port = udp.local_addr().map(|addr| addr.port()).unwrap_or(0);

        let addr = (config.host.as_str(), config.port)
            .to_socket_addrs()
            .ok()
            .and_then(|mut addrs| addrs.next());
        let Some(addr) = addr else {
            eprintln!(
                "skynet-franka: could not resolve {}:{}",
                config.host, config.port
            );
            return Err(status::EIO);
        };
        let tcp = TcpStream::connect_timeout(&addr, config.timeout).map_err(|error| {
            eprintln!(
                "skynet-franka: could not reach {}:{}: {error:?}",
                config.host, config.port
            );
            status::EIO
        })?;
        tcp.set_read_timeout(Some(config.timeout)).ok();
        // Nagle off before anything is sent. A command that waits for a full segment is a command
        // that arrives late, and the FCI's command port is request/response.
        tcp.set_nodelay(true).ok();
        let stop_tcp = tcp.try_clone().map_err(|_| status::EIO)?;

        self.udp = Some(udp);
        self.tcp = Some(tcp);
        self.slug = config.slug.clone();
        self.simulator = config.simulator;

        let request = fci::pack_connect_request(fci::VERSION, udp_port);
        let reply = self.transact(fci::COMMAND_CONNECT, &request);
        let Some((_header, payload)) = reply else {
            self.shutdown();
            return Err(status::EIO);
        };
        let (handshake, theirs) = fci::unpack_connect_response(&payload);
        if handshake != fci::CONNECT_SUCCESS {
            eprintln!(
                "skynet-franka: this Control box speaks FCI version {theirs} and this driver \
                 speaks {}. Every message after the handshake is a fixed-width struct whose \
                 meaning is the version's, so the connection is refused rather than read.",
                fci::VERSION
            );
            self.shutdown();
            return Err(status::EPROTO);
        }

        // The first state, waited for rather than assumed. A driver that returned success here and
        // let the first `read_state` discover the stream was dead would have reported a body it
        // never heard from.
        if self.receive(Duration::from_secs(2)).is_none() {
            eprintln!("skynet-franka: connected, and no RobotState arrived within two seconds.");
            self.shutdown();
            return Err(status::EIO);
        }

        // No setpoint and no hold carries into a new session.
        self.setpoint = [None; fci::JOINTS];
        self.hold = [None; fci::JOINTS];
        self.opened = true;
        Ok(stop_tcp)
    }

    /// Stop the running motion, if any, and release both sockets. Silent by design: close
    /// is where a dying process lands, and there is nothing left to report to.
    pub fn close(&mut self) {
        if self.opened && self.motion_mode.is_some() {
            self.transact(fci::COMMAND_STOP_MOVE, &[]);
        }
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.opened = false;
        self.motion_mode = None;
        self.tcp = None;
        self.udp = None;
    }

    // ── The command port ─────────────────────────────────────────────────────

    /// One request and its reply, matched by command id.
    ///
    /// The id is what makes this safe: the Control box answers a `Move` twice — once when the
    /// motion starts and once when it finishes — so a client that took "the next message" as its
    /// reply would read a motion's completion as the answer to whatever it asked next. Replies
    /// that are not this exchange's are dropped here.
    fn transact(&mut self, command: u32, payload: &[u8]) -> Option<(fci::Header, Vec<u8>)> {
        let tcp = self.tcp.as_mut()?;
        self.command_id = self.command_id.wrapping_add(1);
        let mine = self.command_id;
        tcp.write_all(&fci::pack_command(command, mine, payload))
            .ok()?;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            let (header, body) = self.read_message()?;
            if header.command_id == mine {
                return Some((header, body));
            }
        }
        None
    }

    fn read_message(&mut self) -> Option<(fci::Header, Vec<u8>)> {
        loop {
            if self.buffer.len() >= fci::HEADER_SIZE {
                let header = fci::Header::unpack(&self.buffer).ok()?;
                if self.buffer.len() >= header.size as usize {
                    let body = self.buffer[fci::HEADER_SIZE..header.size as usize].to_vec();
                    self.buffer.drain(..header.size as usize);
                    return Some((header, body));
                }
            }
            let mut chunk = [0u8; 4096];
            let tcp = self.tcp.as_mut()?;
            let n = tcp.read(&mut chunk).ok()?;
            if n == 0 {
                return None;
            }
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }

    // ── The state stream ─────────────────────────────────────────────────────

    /// The **newest** state in the socket, never the oldest.
    ///
    /// F27's lesson, arriving from a second vendor: a stream at 1 kHz fills a socket buffer with
    /// seconds of history, and a client that reads one datagram per cycle reads the past. Every
    /// pending datagram is drained and the last complete one wins, which is §47.6's latest-wins
    /// rule one layer down.
    fn receive(&mut self, timeout: Duration) -> Option<&RobotState> {
        let udp = self.udp.as_ref()?;
        udp.set_read_timeout(Some(timeout)).ok();
        let mut newest: Option<Vec<u8>> = None;
        // The first datagram is waited for; everything after it is drained without waiting. The
        // socket goes non-blocking after the first read and blocking again before this returns,
        // which is Python's `settimeout(0)` in a language where the two are different calls.
        let mut first = true;
        loop {
            if !first {
                udp.set_nonblocking(true).ok()?;
            }
            first = false;
            let mut raw = [0u8; 4096];
            match udp.recv_from(&mut raw) {
                Ok((n, peer)) => {
                    if n == fci::ROBOT_STATE_SIZE {
                        newest = Some(raw[..n].to_vec());
                        // Commands go back to wherever the state came from, refreshed on every
                        // read rather than cached at open: a Control box that restarts its stream
                        // is followed rather than shouted at, and a driver that has heard nothing
                        // has nowhere to send.
                        self.peer = Some(peer);
                    }
                }
                Err(_) => break,
            }
        }
        udp.set_nonblocking(false).ok();
        let newest = newest?;
        match RobotState::unpack(&newest) {
            Ok(state) => {
                self.state = Some(state);
                self.state.as_ref()
            }
            Err(error) => {
                eprintln!("skynet-franka: {error}");
                None
            }
        }
    }

    // ── §24 ──────────────────────────────────────────────────────────────────

    /// Drain the stream to its newest state and translate it to §24: flags, fault code and
    /// the three joint arrays. `SKR_EPERM` before open, `SKR_EIO` when the stream is silent.
    pub fn read_state(&mut self, estopped: bool) -> Result<StateView, i32> {
        if !self.opened {
            return Err(status::EPERM);
        }
        // Clone the state's live fields out of the borrow: the caller fills caller-owned arrays.
        let state = self.receive(Duration::from_millis(50)).ok_or(status::EIO)?;
        let view = StateView {
            q: state.q,
            dq: state.dq,
            tau_j: state.tau_j,
            robot_mode: state.robot_mode,
            faulted: state.faulted(),
            fault_code: state.fault_code(),
            flags: 0,
            seq: 0,
        };
        self.seq = self.seq.wrapping_add(1);
        let mut flags = 0u16;
        if estopped || view.robot_mode == fci::ROBOT_MODE_USER_STOPPED {
            flags |= flags::ESTOP;
        }
        if view.faulted || view.robot_mode == fci::ROBOT_MODE_REFLEX {
            flags |= flags::FAULT;
        }
        // §24's homing field. A Franka's joints are absolute and there is nothing to home, which
        // is a property of the arm rather than a guess about this one.
        flags |= Homing::NotRequired.to_flags();
        Ok(StateView {
            flags,
            seq: self.seq,
            ..view
        })
    }

    /// Start the motion the mode needs, if it is not running, and send the command the
    /// stream's next tick will read. `estopped` is the crate root's latch, re-checked under the
    /// connection lock.
    #[allow(clippy::too_many_arguments)]
    pub fn write_command(
        &mut self,
        mode: Mode,
        values: &[f32; fci::JOINTS],
        mask: &[u8; fci::JOINTS],
        estopped: bool,
    ) -> i32 {
        if !self.opened {
            return status::EPERM;
        }
        if estopped {
            return status::EPERM;
        }
        let Some(state) = self.state.clone() else {
            return status::EIO;
        };
        if state.faulted() {
            // Refused rather than commanded into the fault. The recovery is
            // `AutomaticErrorRecovery`, which an operator asks for — a driver that cleared a
            // latched error on its own would erase why the robot stopped, which is
            // `drivers/canopen`'s F26 arriving at a second vendor.
            return status::EPERM;
        }

        let wanted = motion_for(mode);
        if self.motion_mode != Some(wanted) {
            if self.motion_mode.is_some() {
                self.transact(fci::COMMAND_STOP_MOVE, &[]);
                self.motion_mode = None;
            }
            let payload = fci::pack_move_payload(
                move_controller_for(mode),
                wanted,
                [10.0, 10.0, 10.0],
                [10.0, 10.0, 10.0],
            );
            let Some((_header, reply)) = self.transact(fci::COMMAND_MOVE, &payload) else {
                return status::EIO;
            };
            let move_status = reply.first().copied().unwrap_or(fci::MOVE_ABORTED);
            if move_status != fci::MOVE_STARTED && move_status != fci::MOVE_SUCCESS {
                eprintln!(
                    "skynet-franka: Move refused — {}",
                    fci::move_status_name(move_status)
                );
                return status::EPERM;
            }
            self.motion_mode = Some(wanted);
        }

        let (commanded, hold) =
            resolve_hold(mode, values, mask, &state.q, &self.setpoint, &self.hold);
        let message = match mode {
            Mode::Position => {
                fci::pack_robot_command(state.message_id, Some(&commanded), None, None, false)
            }
            Mode::Velocity => {
                fci::pack_robot_command(state.message_id, None, Some(&commanded), None, false)
            }
            Mode::Torque => {
                fci::pack_robot_command(state.message_id, None, None, Some(&commanded), false)
            }
        };
        let Some(peer) = self.peer else {
            return status::EIO;
        };
        let sent = match self.udp.as_ref() {
            Some(udp) => udp.send_to(&message, peer).is_ok(),
            None => false,
        };
        if !sent {
            return status::EIO;
        }
        // Committed only once the command is on the wire.
        self.hold = hold;
        if mode == Mode::Position {
            for i in 0..fci::JOINTS {
                if mask[i] != 0 {
                    self.setpoint[i] = Some(commanded[i]);
                }
            }
        } else {
            self.setpoint = [None; fci::JOINTS];
        }
        status::OK
    }

    /// The latch half of the stop, for the crate root to call after the message half: a stopped
    /// motion is no motion, so the next command starts one rather than joining this one.
    pub fn note_stopped(&mut self) {
        self.motion_mode = None;
    }

    // ── The body ─────────────────────────────────────────────────────────────

    /// The embodiment descriptor, byte-identical with the Python driver's for the same slug and
    /// simulator flag: the wire is not the only place the two must agree, and a template with two
    /// holes is cheaper to hold than a serializer to match.
    pub fn describe(&self) -> String {
        describe_for(&self.slug, self.simulator)
    }
}

/// What `read_state` hands the crate root to copy into the caller's arrays.
#[derive(Debug, Clone)]
pub struct StateView {
    /// Joint positions, radians.
    pub q: [f32; fci::JOINTS],
    /// Joint velocities, rad/s.
    pub dq: [f32; fci::JOINTS],
    /// Measured link-side torques, Nm.
    pub tau_j: [f32; fci::JOINTS],
    /// The robot's own mode (`ROBOT_MODE_*`), for the flag translation.
    pub robot_mode: u8,
    /// Whether any of the 41 error bits is set.
    pub faulted: bool,
    /// The lowest set error bit, one-based, or 0.
    pub fault_code: u32,
    /// The assembled §24 flags word.
    pub flags: u16,
    /// The sample sequence number.
    pub seq: u32,
}

fn motion_for(mode: Mode) -> u32 {
    match mode {
        Mode::Position => fci::MOVE_MOTION_JOINT_POSITION,
        Mode::Velocity => fci::MOVE_MOTION_JOINT_VELOCITY,
        Mode::Torque => fci::MOVE_MOTION_NONE,
    }
}

fn move_controller_for(mode: Mode) -> u32 {
    match mode {
        Mode::Torque => fci::MOVE_CONTROLLER_EXTERNAL,
        _ => fci::MOVE_CONTROLLER_JOINT_IMPEDANCE,
    }
}

/// Minimal JSON string escaping: the slug is operator-supplied and the template is not.
fn escape_json_string(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len() + 2);
    for ch in raw.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other if (other as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => out.push(other),
        }
    }
    out
}

/// The descriptor template: `franka-fr3` over `SIMULATOR`, exactly as the Python driver's
/// `json.dumps` emits it. Regenerate the comparison — not this text — with the live-fake test,
/// which asserts byte equality against the Python bridge's own `describe()`.
fn describe_for(slug: &str, simulator: bool) -> String {
    const HEAD: &str = r#"{"schema_version": 1, "robot_slug": ""#;
    const MID: &str = r#"", "digest": null, "verified": false, "simulator": "#;
    const TAIL: &str = r#", "base": "fixed", "dof": 7, "joints": [{"name": "panda_joint1", "role": "right_arm.shoulder.yaw", "type": "revolute", "limits": {"position_min_rad": -2.7437, "position_max_rad": 2.7437, "velocity_max_rad_s": 2.62, "effort_max_nm": 87.0}, "control_modes": ["position", "velocity", "torque"]}, {"name": "panda_joint2", "role": "right_arm.shoulder.pitch", "type": "revolute", "limits": {"position_min_rad": -1.7837, "position_max_rad": 1.7837, "velocity_max_rad_s": 2.62, "effort_max_nm": 87.0}, "control_modes": ["position", "velocity", "torque"]}, {"name": "panda_joint3", "role": "right_arm.upper_arm.roll", "type": "revolute", "limits": {"position_min_rad": -2.9007, "position_max_rad": 2.9007, "velocity_max_rad_s": 2.62, "effort_max_nm": 87.0}, "control_modes": ["position", "velocity", "torque"]}, {"name": "panda_joint4", "role": "right_arm.elbow.flex", "type": "revolute", "limits": {"position_min_rad": -3.0421, "position_max_rad": -0.1518, "velocity_max_rad_s": 2.62, "effort_max_nm": 87.0}, "control_modes": ["position", "velocity", "torque"]}, {"name": "panda_joint5", "role": "right_arm.forearm.roll", "type": "revolute", "limits": {"position_min_rad": -2.8065, "position_max_rad": 2.8065, "velocity_max_rad_s": 5.26, "effort_max_nm": 12.0}, "control_modes": ["position", "velocity", "torque"]}, {"name": "panda_joint6", "role": "right_arm.wrist.pitch", "type": "revolute", "limits": {"position_min_rad": 0.5445, "position_max_rad": 4.5169, "velocity_max_rad_s": 4.18, "effort_max_nm": 12.0}, "control_modes": ["position", "velocity", "torque"]}, {"name": "panda_joint7", "role": "right_arm.wrist.roll", "type": "revolute", "limits": {"position_min_rad": -3.0159, "position_max_rad": 3.0159, "velocity_max_rad_s": 5.26, "effort_max_nm": 12.0}, "control_modes": ["position", "velocity", "torque"]}], "end_effectors": [], "sensors": [], "control": {"modes": ["position", "velocity", "torque"], "rate_hz_max": 1000, "rate_hz_min": 1000, "command_latency_ms": null}, "envelope": {"joint_velocity_max_rad_s": 2.62, "joint_torque_max_nm": 12.0, "watchdog_ms": 10.0, "source": "Franka Robotics franka_description, robots/fr3/joint_limits.yaml (the vendor's joint-limit table); watchdog ten FCI periods per driver design", "basis": "declared"}, "capabilities": ["safety.estop"], "solvers": []}"#;
    format!(
        "{HEAD}{}{MID}{}{TAIL}",
        escape_json_string(slug),
        if simulator { "true" } else { "false" }
    )
}

impl Default for Connection {
    fn default() -> Connection {
        Connection::new()
    }
}

/// §24's mask, read per mode: what each joint is commanded, and the hold targets to keep.
///
/// A zero in the mask holds the joint at its current setpoint. In position that is the target
/// latched when the hold began — the last position setpoint, or the joint's reading `q` when it
/// has none — and it is not re-read: re-reading `q` every cycle zeroes the position error, and
/// only the Control box's own gravity compensation kept `franka-sim` from showing the drift that
/// leaves on a loaded joint (DRY-COVERAGE-PLAN.md F121, carried as 8.13; the Python driver is the
/// executable specification and carries the same rule). In velocity a hold is zero, which holds
/// the pose; in torque it is zero too, which under the FCI's gravity compensation floats the joint
/// rather than dropping it. Any command outside position voids the hold targets, because the arm
/// may have moved under it and a later hold must not jump back.
fn resolve_hold(
    mode: Mode,
    values: &[f32; fci::JOINTS],
    mask: &[u8; fci::JOINTS],
    q: &[f32; fci::JOINTS],
    setpoint: &[Option<f64>; fci::JOINTS],
    hold: &[Option<f64>; fci::JOINTS],
) -> ([f64; fci::JOINTS], [Option<f64>; fci::JOINTS]) {
    let mut commanded = [0.0f64; fci::JOINTS];
    let mut next = [None; fci::JOINTS];
    for i in 0..fci::JOINTS {
        if mask[i] != 0 {
            commanded[i] = values[i] as f64;
        } else if mode == Mode::Position {
            let target = hold[i].unwrap_or_else(|| setpoint[i].unwrap_or(q[i] as f64));
            next[i] = Some(target);
            commanded[i] = target;
        }
    }
    (commanded, next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_masked_joint_is_held_at_its_setpoint_and_latched() {
        let values = [0.5f32; fci::JOINTS];
        let mut mask = [1u8; fci::JOINTS];
        mask[1] = 0;
        let q = [0.25f32; fci::JOINTS];
        let none = [None; fci::JOINTS];
        // No setpoint yet: the hold latches where the joint is.
        let (commanded, hold) = resolve_hold(Mode::Position, &values, &mask, &q, &none, &none);
        assert_eq!(commanded[0], 0.5);
        assert_eq!(commanded[1], 0.25);
        // The joint moves under the hold; the target does not follow it.
        let pushed = [0.75f32; fci::JOINTS];
        let (commanded, _) = resolve_hold(Mode::Position, &values, &mask, &pushed, &none, &hold);
        assert_eq!(commanded[1], 0.25, "latched, not re-read (F121)");
        // With a setpoint behind it, the hold is the setpoint.
        let mut setpoint = [None; fci::JOINTS];
        setpoint[1] = Some(0.7);
        let (commanded, _) =
            resolve_hold(Mode::Position, &values, &mask, &pushed, &setpoint, &none);
        assert_eq!(commanded[1], 0.7);
        // Velocity and torque hold at zero and keep no target.
        let (commanded, hold) = resolve_hold(Mode::Velocity, &values, &mask, &q, &setpoint, &none);
        assert_eq!(commanded[1], 0.0);
        assert_eq!(hold, none);
    }

    #[test]
    fn config_defaults_match_the_python_driver() {
        // Assumes the ambient environment carries no `SKYNET_FRANKA_*`: unsetting from a test
        // would race every other test in the process, so the suite runs on a clean one.
        let config = Config::parse("").unwrap();
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, fci::COMMAND_PORT);
        assert_eq!(config.slug, "franka-fr3");
        assert!(config.simulator);
    }

    #[test]
    fn config_is_not_json_is_einval() {
        assert_eq!(Config::parse("{nope").unwrap_err(), status::EINVAL);
    }

    #[test]
    fn config_explicit_values_win() {
        // The environment arm of the resolution order is not tested here: tests run in parallel
        // in one process, and mutating the environment from one of them is a race with every
        // other. The order is one `match` above and reads in full.
        let config = Config::parse(
            r#"{"port": 9999, "host": "10.0.0.2", "simulator": false, "timeout_s": 1.5}"#,
        )
        .unwrap();
        assert_eq!(config.port, 9999);
        assert_eq!(config.host, "10.0.0.2");
        assert!(!config.simulator);
        assert_eq!(config.timeout, Duration::from_secs_f64(1.5));
    }

    #[test]
    fn config_garbage_port_is_refused() {
        assert_eq!(
            Config::parse(r#"{"port": "lots"}"#).unwrap_err(),
            status::EINVAL
        );
    }

    #[test]
    fn envelope_is_the_narrowest_rating() {
        let described = describe_for("franka-fr3", true);
        assert!(described.contains(r#""joint_velocity_max_rad_s": 2.62"#));
        assert!(described.contains(r#""joint_torque_max_nm": 12.0"#));
        assert!(described.contains(r#""watchdog_ms": 10.0"#));
        assert!(described.contains(r#""basis": "declared""#));
    }

    #[test]
    fn descriptor_template_agrees_with_the_tables() {
        // The template is pasted text, and pasted text drifts. The tables are the source, and the
        // template is held to them *joint by joint*: a figure that merely appears somewhere in the
        // template (a sign flipped on a limit still contains its own digits; a torque rating
        // copied from a neighbour appears under the neighbour) is not the figure for that joint.
        let described = Json::parse(&describe_for("franka-fr3", true)).unwrap();
        let joints = described.get("joints").and_then(Json::as_arr).unwrap();
        assert_eq!(joints.len(), fci::JOINTS);
        let number = |joint: &Json, key: &str| {
            joint
                .get("limits")
                .and_then(|limits| limits.get(key))
                .and_then(Json::as_f64)
                .unwrap()
        };
        for (index, joint) in joints.iter().enumerate() {
            let ((low, high), (velocity, effort)) = (JOINT_LIMITS[index], JOINT_RATINGS[index]);
            let at = format!("joint {}", index + 1);
            assert_eq!(number(joint, "position_min_rad"), low, "{at}: lower travel");
            assert_eq!(
                number(joint, "position_max_rad"),
                high,
                "{at}: upper travel"
            );
            assert_eq!(number(joint, "velocity_max_rad_s"), velocity, "{at}: speed");
            assert_eq!(number(joint, "effort_max_nm"), effort, "{at}: torque");
            assert_eq!(
                joint.get("role").and_then(Json::as_str),
                Some(ROLES[index]),
                "{at}: role"
            );
        }
        let envelope = described.get("envelope").unwrap();
        assert_eq!(
            envelope.get("watchdog_ms").and_then(Json::as_f64),
            Some(WATCHDOG_MS),
            "the watchdog window"
        );
        let slowest = JOINT_RATINGS.iter().map(|r| r.0).fold(f64::MAX, f64::min);
        let weakest = JOINT_RATINGS.iter().map(|r| r.1).fold(f64::MAX, f64::min);
        assert_eq!(
            envelope
                .get("joint_velocity_max_rad_s")
                .and_then(Json::as_f64),
            Some(slowest),
            "the envelope's speed is the narrowest rating"
        );
        assert_eq!(
            envelope.get("joint_torque_max_nm").and_then(Json::as_f64),
            Some(weakest),
            "and its torque"
        );
    }
}

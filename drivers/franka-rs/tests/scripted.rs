//! The port's own decisions, against a scripted Control box (DRY-COVERAGE-PLAN.md 12.C.3).
//!
//! `live_fake` runs the driver against the Python driver's fake box, and `layout` and `vectors` hold the bytes. What none of
//! them held were the decisions the driver makes above the wire: which motion a mode starts and with which controller, what a
//! hold holds, what the stop sends and to whom, what a stopped motion is, what `open` will not accept, what a flag says. The
//! seeded-defect campaign (`tools/seed-defects/`) found that 49 of this crate's 86 realistic defects — a torque command that
//! starts a position motion, a stop that sends `AutomaticErrorRecovery`, a user-stop that never shows as the estop flag, a
//! hold that falls back to the reading and sags — left `cargo test` green.
//!
//! The box here is a TCP command port and a UDP state stream on ephemeral ports, scripted by the test: the pose it reports, the
//! modes and error bits, what `Connect` and `Move` answer, and — what makes the assertions possible — every message it was
//! sent. **It agrees with whoever wrote it**, which is F15's point and the reason `live_fake` and franka-sim exist: nothing
//! here says the bytes are a real Control box's. The ports are ephemeral, so the campaign's workers do not meet; the driver's
//! state is one static body, so these tests take a lock and run one at a time.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use skynet_franka_rs::driver::{Config, Connection};
use skynet_franka_rs::protocol as fci;
use skynet_rhal::{flags, status, Command as RhalCommand, Ctx, Homing, Mode, RawDriver, State};

// ── the scripted box ─────────────────────────────────────────────────────────

/// A command the box received over UDP, and which of its sockets it arrived at.
#[derive(Debug, Clone)]
struct Received {
    socket: usize,
    fields: fci::RobotCommandFields,
}

/// What the box says, and what it has been sent.
struct Script {
    q: [f32; 7],
    dq: [f32; 7],
    tau_j: [f32; 7],
    robot_mode: u8,
    /// Indexes of the error bits that are set.
    errors: Vec<usize>,
    /// Whether the state stream runs, and how fast.
    stream: bool,
    hz: f64,
    /// The message id the next state carries, and the last one sent.
    next_id: u64,
    last_id: u64,
    /// What `Connect` answers: a status, and whether the reply carries its payload at all.
    connect_status: u8,
    connect_empty: bool,
    /// What `Move` answers, and whether a reply to some other exchange comes first.
    move_status: u8,
    stray_before_move: bool,
    /// Close the command connection, as a box that went away does.
    drop_tcp: bool,
    // What it saw.
    connects: Vec<(u16, u16)>,
    /// Every command on the TCP port, in order: its number.
    tcp_log: Vec<u32>,
    moves: Vec<(u32, u32)>,
    commands: Vec<Received>,
}

impl Script {
    fn new() -> Script {
        Script {
            q: START_Q,
            dq: [0.0; 7],
            tau_j: [0.0; 7],
            robot_mode: fci::ROBOT_MODE_IDLE,
            errors: Vec::new(),
            stream: true,
            hz: 200.0,
            next_id: 1000,
            last_id: 0,
            connect_status: fci::CONNECT_SUCCESS,
            connect_empty: false,
            move_status: fci::MOVE_STARTED,
            stray_before_move: false,
            drop_tcp: false,
            connects: Vec::new(),
            tcp_log: Vec::new(),
            moves: Vec::new(),
            commands: Vec::new(),
        }
    }

    fn count(&self, command: u32) -> usize {
        self.tcp_log.iter().filter(|seen| **seen == command).count()
    }

    /// One `RobotState` datagram, with the fields this driver reads at the offsets Franka's headers give.
    fn datagram(&self, message_id: u64) -> Vec<u8> {
        let mut raw = vec![0u8; fci::ROBOT_STATE_SIZE];
        raw[fci::STATE_MESSAGE_ID..fci::STATE_MESSAGE_ID + 8]
            .copy_from_slice(&message_id.to_le_bytes());
        for joint in 0..7 {
            raw[fci::STATE_Q + joint * 4..fci::STATE_Q + joint * 4 + 4]
                .copy_from_slice(&self.q[joint].to_le_bytes());
            raw[fci::STATE_DQ + joint * 4..fci::STATE_DQ + joint * 4 + 4]
                .copy_from_slice(&self.dq[joint].to_le_bytes());
            raw[fci::STATE_TAU_J + joint * 4..fci::STATE_TAU_J + joint * 4 + 4]
                .copy_from_slice(&self.tau_j[joint].to_le_bytes());
        }
        for bit in &self.errors {
            raw[fci::STATE_ERRORS + bit] = 1;
        }
        raw[fci::STATE_ROBOT_MODE] = self.robot_mode;
        raw
    }
}

/// The fake's start pose, in values an `f32` holds exactly, each joint different.
const START_Q: [f32; 7] = [0.0, -0.75, 0.125, -2.25, 0.25, 1.5, 0.75];

type Shared = Arc<Mutex<Script>>;

fn locked(shared: &Shared) -> MutexGuard<'_, Script> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn timed_out(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

struct ScriptedBox {
    shared: Shared,
    stop: Arc<AtomicBool>,
    /// The command port the driver is told to connect to.
    port: u16,
    /// The UDP port the driver named in `Connect`, once it has.
    driver_udp: Arc<Mutex<Option<u16>>>,
    threads: Vec<JoinHandle<()>>,
}

impl ScriptedBox {
    fn start(script: Script) -> ScriptedBox {
        let shared: Shared = Arc::new(Mutex::new(script));
        let stop = Arc::new(AtomicBool::new(false));
        let driver_udp: Arc<Mutex<Option<u16>>> = Arc::new(Mutex::new(None));
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("a command port");
        listener.set_nonblocking(true).expect("non-blocking");
        let port = listener.local_addr().expect("an address").port();
        let udp = Arc::new(UdpSocket::bind(("127.0.0.1", 0)).expect("a state socket"));
        udp.set_read_timeout(Some(Duration::from_millis(20)))
            .expect("a timeout");
        let mut threads = Vec::new();
        {
            let (shared, stop, driver_udp) = (shared.clone(), stop.clone(), driver_udp.clone());
            threads.push(thread::spawn(move || {
                serve_tcp(listener, shared, stop, driver_udp)
            }));
        }
        {
            let (shared, stop, driver_udp, udp) = (
                shared.clone(),
                stop.clone(),
                driver_udp.clone(),
                udp.clone(),
            );
            threads.push(thread::spawn(move || stream(shared, stop, driver_udp, udp)));
        }
        {
            let (shared, stop, udp) = (shared.clone(), stop.clone(), udp);
            threads.push(thread::spawn(move || listen(shared, stop, udp, 0)));
        }
        ScriptedBox {
            shared,
            stop,
            port,
            driver_udp,
            threads,
        }
    }

    fn with<R>(&self, act: impl FnOnce(&mut Script) -> R) -> R {
        act(&mut locked(&self.shared))
    }

    fn wait_for(&self, what: &str, condition: impl Fn(&Script) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition(&locked(&self.shared)) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("the box never saw {what} within five seconds");
    }

    fn settled<R>(&self, read: impl Fn(&Script) -> R) -> R {
        thread::sleep(Duration::from_millis(150));
        read(&locked(&self.shared))
    }

    /// The UDP port the driver is listening on for states, once `Connect` has named it.
    fn driver_port(&self) -> u16 {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Some(port) = *self.driver_udp.lock().unwrap() {
                return port;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("the driver never named a UDP port in Connect");
    }

    fn config(&self) -> String {
        format!(
            r#"{{"host": "127.0.0.1", "port": {}, "simulator": true}}"#,
            self.port
        )
    }
}

impl Drop for ScriptedBox {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

fn serve_tcp(
    listener: TcpListener,
    shared: Shared,
    stop: Arc<AtomicBool>,
    driver_udp: Arc<Mutex<Option<u16>>>,
) {
    let mut handlers: Vec<JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).expect("blocking");
                stream
                    .set_read_timeout(Some(Duration::from_millis(20)))
                    .expect("a timeout");
                let (shared, stop, driver_udp) = (shared.clone(), stop.clone(), driver_udp.clone());
                handlers.push(thread::spawn(move || {
                    command_port(stream, shared, stop, driver_udp)
                }));
            }
            Err(error) if timed_out(&error) => thread::sleep(Duration::from_millis(1)),
            Err(_) => break,
        }
    }
    for handler in handlers {
        let _ = handler.join();
    }
}

fn command_port(
    mut stream: TcpStream,
    shared: Shared,
    stop: Arc<AtomicBool>,
    driver_udp: Arc<Mutex<Option<u16>>>,
) {
    let mut buffer: Vec<u8> = Vec::new();
    loop {
        if stop.load(Ordering::Acquire) || locked(&shared).drop_tcp {
            return;
        }
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(error) if timed_out(&error) => {}
            Err(_) => return,
        }
        while buffer.len() >= fci::HEADER_SIZE {
            let Ok(header) = fci::Header::unpack(&buffer) else {
                return;
            };
            if buffer.len() < header.size as usize {
                break;
            }
            let payload = buffer[fci::HEADER_SIZE..header.size as usize].to_vec();
            buffer.drain(..header.size as usize);
            let replies: Vec<Vec<u8>> = {
                let mut script = locked(&shared);
                script.tcp_log.push(header.command);
                match header.command {
                    fci::COMMAND_CONNECT => {
                        let version = u16::from_le_bytes([payload[0], payload[1]]);
                        let port = u16::from_le_bytes([payload[2], payload[3]]);
                        script.connects.push((version, port));
                        *driver_udp.lock().unwrap() = Some(port);
                        if script.connect_empty {
                            vec![fci::pack_command(header.command, header.command_id, &[])]
                        } else {
                            vec![fci::pack_response(
                                header.command,
                                header.command_id,
                                script.connect_status,
                                &fci::VERSION.to_le_bytes(),
                            )]
                        }
                    }
                    fci::COMMAND_MOVE => {
                        let controller = u32::from_le_bytes(payload[0..4].try_into().unwrap());
                        let generator = u32::from_le_bytes(payload[4..8].try_into().unwrap());
                        script.moves.push((controller, generator));
                        let mut replies = Vec::new();
                        if script.stray_before_move {
                            // The Control box answers a Move twice; an earlier exchange's answer arriving first is the same shape.
                            replies.push(fci::pack_response(
                                header.command,
                                header.command_id.wrapping_add(777),
                                fci::MOVE_ABORTED,
                                &[],
                            ));
                        }
                        replies.push(fci::pack_response(
                            header.command,
                            header.command_id,
                            script.move_status,
                            &[],
                        ));
                        replies
                    }
                    other => vec![fci::pack_response(other, header.command_id, 0, &[])],
                }
            };
            for reply in replies {
                if stream.write_all(&reply).is_err() {
                    return;
                }
            }
        }
    }
}

/// The state stream: a datagram per tick to the port the driver named, while the script says to.
fn stream(
    shared: Shared,
    stop: Arc<AtomicBool>,
    driver_udp: Arc<Mutex<Option<u16>>>,
    udp: Arc<UdpSocket>,
) {
    while !stop.load(Ordering::Acquire) {
        let period = Duration::from_secs_f64(1.0 / locked(&shared).hz);
        let port = *driver_udp.lock().unwrap();
        if let Some(port) = port {
            let datagram = {
                let mut script = locked(&shared);
                if script.stream {
                    let id = script.next_id;
                    script.next_id += 1;
                    script.last_id = id;
                    Some(script.datagram(id))
                } else {
                    None
                }
            };
            if let Some(datagram) = datagram {
                let _ = udp.send_to(&datagram, ("127.0.0.1", port));
            }
        }
        thread::sleep(period);
    }
}

/// Commands arriving at a socket, recorded with the socket's number.
fn listen(shared: Shared, stop: Arc<AtomicBool>, udp: Arc<UdpSocket>, socket: usize) {
    let mut raw = [0u8; 4096];
    while !stop.load(Ordering::Acquire) {
        match udp.recv_from(&mut raw) {
            Ok((n, _)) => {
                if let Ok(fields) = fci::unpack_robot_command(&raw[..n]) {
                    locked(&shared).commands.push(Received { socket, fields });
                }
            }
            Err(error) if timed_out(&error) => {}
            Err(_) => return,
        }
    }
}

// ── the tests' own grip on the driver ────────────────────────────────────────

fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn driver() -> &'static RawDriver {
    // SAFETY: the entry point returns a pointer to a static vtable.
    unsafe { &*skynet_franka_rs::skynet_rhal_entry() }
}

fn open_driver(config: &str) -> (i32, *mut Ctx) {
    let config = std::ffi::CString::new(config).unwrap();
    let mut ctx: *mut Ctx = std::ptr::null_mut();
    // SAFETY: a NUL-terminated config and a live out-pointer.
    let rc = unsafe { (driver().open.unwrap())(config.as_ptr(), &mut ctx) };
    (rc, ctx)
}

/// `open` against the box, expected to succeed.
fn opened(script: Script) -> (ScriptedBox, *mut Ctx) {
    let scripted = ScriptedBox::start(script);
    let (rc, ctx) = open_driver(&scripted.config());
    assert_eq!(rc, status::OK, "the scripted box is brought up");
    (scripted, ctx)
}

struct Reading {
    rc: i32,
    flags: u16,
    fault_code: u32,
    seq: u32,
    stamp_ns: u64,
    q: [f32; 7],
    dq: [f32; 7],
    effort: [f32; 7],
    base_pose: [f32; 7],
}

fn read(ctx: *mut Ctx) -> Reading {
    let mut q = [0.0f32; 7];
    let mut dq = [0.0f32; 7];
    let mut effort = [0.0f32; 7];
    let mut state = State {
        stamp_ns: 0,
        seq: 0,
        joint_count: 7,
        flags: 0,
        position_rad: q.as_mut_ptr(),
        velocity_rad_s: dq.as_mut_ptr(),
        effort_nm: effort.as_mut_ptr(),
        base_pose: [0.0; 7],
        base_twist: [0.0; 6],
        fault_code: 0,
    };
    // SAFETY: a live context and a state whose arrays are seven long.
    let rc = unsafe { (driver().read_state.unwrap())(ctx, &mut state, 0) };
    Reading {
        rc,
        flags: state.flags,
        fault_code: state.fault_code,
        seq: state.seq,
        stamp_ns: state.stamp_ns,
        q,
        dq,
        effort,
        base_pose: state.base_pose,
    }
}

fn write(ctx: *mut Ctx, mode: Mode, values: &[f32; 7], mask: &[u8; 7]) -> i32 {
    let command = RhalCommand {
        stamp_ns: 1,
        seq: 1,
        mode: mode as u8,
        flags: 0,
        _pad: [0; 2],
        joint_count: 7,
        values: values.as_ptr(),
        mask: mask.as_ptr(),
        modes: std::ptr::null(),
    };
    // SAFETY: a live context and a command whose arrays are seven long.
    unsafe { (driver().write_command.unwrap())(ctx, &command, 0) }
}

fn stop(ctx: *mut Ctx) -> i32 {
    // SAFETY: a live context.
    unsafe { (driver().emergency_stop.unwrap())(ctx) }
}

fn release(ctx: *mut Ctx) -> i32 {
    // SAFETY: a live context.
    unsafe { (driver().release_stop.unwrap())(ctx) }
}

fn close(ctx: *mut Ctx) -> i32 {
    // SAFETY: a live context.
    unsafe { (driver().close.unwrap())(ctx) }
}

const ALL: [u8; 7] = [1; 7];
const NONE: [u8; 7] = [0; 7];
const TARGET: [f32; 7] = [0.125, -0.5, 0.375, -2.0, 0.5, 1.25, 0.875];
const OTHER: [f32; 7] = [-0.25, -0.625, 0.0625, -1.75, 0.1875, 1.0, 0.5];

fn as_f64(values: &[f32; 7]) -> [f64; 7] {
    let mut out = [0.0; 7];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = *value as f64;
    }
    out
}

/// A read until the driver has heard the state the box now publishes: `condition` of what it read.
fn read_until(ctx: *mut Ctx, what: &str, condition: impl Fn(&Reading) -> bool) -> Reading {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let reading = read(ctx);
        if reading.rc == status::OK && condition(&reading) {
            return reading;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("the driver never read {what} within five seconds");
}

/// A command the box received that satisfies `wanted`, waited for: a command is found by what it says, because a UDP datagram
/// written a moment ago may not have been recorded yet, and "the last one" would then be an earlier command.
fn command_where(
    scripted: &ScriptedBox,
    what: &str,
    wanted: impl Fn(&fci::RobotCommandFields) -> bool,
) -> Received {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Some(found) =
            scripted.with(|s| s.commands.iter().rev().find(|c| wanted(&c.fields)).cloned())
        {
            return found;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("the box never received {what} within five seconds");
}

/// The last command the box received at `socket`, once one has arrived after `seen`.
fn command_after(scripted: &ScriptedBox, seen: usize) -> Received {
    scripted.wait_for("a command on the state socket", |s| s.commands.len() > seen);
    scripted.with(|s| s.commands.last().cloned().unwrap())
}

// ── open ─────────────────────────────────────────────────────────────────────

#[test]
fn open_names_a_udp_port_and_the_version_it_speaks_and_waits_for_the_first_state() {
    let _one = one_at_a_time();
    let (scripted, ctx) = opened(Script::new());
    let (version, port) = scripted.with(|s| s.connects[0]);
    assert_eq!(
        version,
        fci::VERSION,
        "the protocol version this driver was transcribed against"
    );
    assert_eq!(port, scripted.driver_port());
    assert_ne!(port, 0, "a UDP port the box can send states to");
    assert_eq!(read(ctx).rc, status::OK);
    assert_eq!(close(ctx), status::OK);
    assert_eq!(read(ctx).rc, status::EPERM, "a closed body refuses");
}

#[test]
fn a_box_that_does_not_shake_hands_properly_is_refused() {
    let _one = one_at_a_time();
    let mut script = Script::new();
    script.connect_status = fci::CONNECT_INCOMPATIBLE_LIBRARY_VERSION;
    let scripted = ScriptedBox::start(script);
    let (rc, _) = open_driver(&scripted.config());
    assert_eq!(rc, status::EPROTO, "an incompatible FCI version is refused, not read: every later message is a fixed-width struct of that version");
    drop(scripted);

    let mut script = Script::new();
    script.connect_empty = true;
    let scripted = ScriptedBox::start(script);
    let (rc, _) = open_driver(&scripted.config());
    assert_ne!(
        rc,
        status::OK,
        "a reply with no payload at all is not a successful handshake"
    );
    assert_eq!(rc, status::EPROTO);
}

#[test]
fn a_body_never_heard_from_is_not_reported_open() {
    let _one = one_at_a_time();
    let mut script = Script::new();
    script.stream = false;
    let scripted = ScriptedBox::start(script);
    let started = Instant::now();
    let (rc, _) = open_driver(&scripted.config());
    assert_eq!(
        rc,
        status::EIO,
        "connected, and no state in two seconds, is not open"
    );
    assert!(
        started.elapsed() >= Duration::from_millis(1900),
        "it waited the two seconds it says it waits: {:?}",
        started.elapsed()
    );
}

#[test]
fn a_state_is_exactly_the_size_of_a_state() {
    let datagram = Script::new().datagram(1);
    assert_eq!(
        datagram.len(),
        1377,
        "Franka's RobotState, from layout.json"
    );
    assert!(fci::RobotState::unpack(&datagram).is_ok());
    let mut long = datagram.clone();
    long.push(0);
    assert!(
        fci::RobotState::unpack(&long).is_err(),
        "longer is a different firmware, not a state with something extra on the end"
    );
    assert!(
        fci::RobotState::unpack(&datagram[..1376]).is_err(),
        "and shorter is a partial read"
    );
}

#[test]
fn a_port_is_a_port_and_a_malformed_one_is_refused_rather_than_wrapped() {
    for bad in [
        r#"{"port": 70000}"#,
        r#"{"port": -1}"#,
        r#"{"port": "lots"}"#,
        r#"{"port": 65536}"#,
    ] {
        assert_eq!(
            Config::parse(bad).unwrap_err(),
            status::EINVAL,
            "{bad}: a driver that wrapped this would move another arm"
        );
    }
    assert_eq!(Config::parse(r#"{"port": 65535}"#).unwrap().port, 65535);
    assert_eq!(Config::parse(r#"{"port": 0}"#).unwrap().port, 0);
}

#[test]
fn a_closed_connection_reaches_no_socket() {
    let mut connection = Connection::new();
    assert!(!connection.is_open());
    assert_eq!(
        connection.write_command(Mode::Position, &TARGET, &ALL, false),
        status::EPERM,
        "a command to a body that is not open"
    );
    assert_eq!(
        connection.read_state(false).unwrap_err(),
        status::EPERM,
        "a read on one"
    );
}

// ── what arrives from the box ────────────────────────────────────────────────

#[test]
fn the_state_is_the_boxs_joint_for_joint_and_carries_the_torque_it_measured() {
    let _one = one_at_a_time();
    let mut script = Script::new();
    script.dq = [0.0625, -0.125, 0.1875, -0.25, 0.3125, -0.375, 0.4375];
    script.tau_j = [1.5, -2.5, 3.5, -4.5, 5.5, -6.5, 7.5];
    let (scripted, ctx) = opened(script);
    let before = skynet_rt::monotonic_raw_ns();
    let state = read(ctx);
    let after = skynet_rt::monotonic_raw_ns();
    assert_eq!(state.rc, status::OK);
    assert_eq!(state.q, START_Q, "positions in order");
    assert_eq!(
        state.dq,
        scripted.with(|s| s.dq),
        "velocities in order, not the positions again"
    );
    assert_eq!(state.effort, scripted.with(|s| s.tau_j), "the torque is the measured link-side torque, which this arm reports: not the velocities, and not NaN");
    assert!(
        state.base_pose.iter().all(|v| v.is_nan()),
        "a fixed base has no pose: NaN and not the origin"
    );
    assert!(
        state.stamp_ns >= before && state.stamp_ns <= after,
        "the stamp is the clock at the read, never later: {} not in {before}..={after}",
        state.stamp_ns
    );
    let next = read(ctx);
    assert_eq!(
        next.seq,
        state.seq.wrapping_add(1),
        "the sequence advances by one a read"
    );
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn a_mode_or_an_error_raises_the_flag_it_means_and_no_other() {
    let _one = one_at_a_time();
    for mode in 0..=6u8 {
        for errors in [vec![], vec![2usize], vec![0, 5]] {
            let mut script = Script::new();
            script.robot_mode = mode;
            script.errors = errors.clone();
            let (_scripted, ctx) = opened(script);
            let state = read(ctx);
            let at = format!("robot_mode {mode}, errors {errors:?}");
            assert_eq!(state.rc, status::OK, "{at}");
            // The numbers are Franka's (rbk_types.h `RobotMode`: kReflex 4, kUserStopped 5), typed here and not read from the
            // crate's own constants, which is where a constant that had been changed would also be read from.
            assert_eq!(
                state.flags & flags::ESTOP != 0,
                mode == 5,
                "{at}: the user-stop is the estop flag, and only it"
            );
            // The maker's own words for what the arm is doing, typed here (rbk_types.h: kOther 0, kGuiding 3, kReflex 4,
            // kAutomaticErrorRecovery 6): in each the Control box does not move the arm for the FCI, which is a fault.
            let not_moving = matches!(mode, 0 | 3 | 4 | 6);
            assert_eq!(
                state.flags & flags::FAULT != 0,
                !errors.is_empty() || not_moving,
                "{at}: a latched error, or a mode in which the arm takes no motion, is a fault"
            );
            assert_eq!(
                state.flags & flags::DEGRADED != 0,
                !errors.is_empty() || !matches!(mode, 1 | 2),
                "{at}: and DEGRADED beside every fault and every stop, the bit §27's rhal condition reads, and in no healthy mode"
            );
            assert_eq!(
                state.fault_code,
                errors.iter().min().map(|bit| *bit as u32 + 1).unwrap_or(0),
                "{at}: the lowest set error bit, one-based, is the vendor code"
            );
            assert_eq!(
                state.flags & flags::HOMING_MASK,
                Homing::NotRequired.to_flags(),
                "{at}: absolute encoders"
            );
            assert_eq!(close(ctx), status::OK);
        }
    }
}

/// The fault-vocabulary contract (`tools/fault-vocab/contract.py`, DRY-COVERAGE-PLAN.md 12.C.6), held to the table `bridge.py` is held to:
/// `drivers/fault-vocabulary.json`, read here with the workspace's own JSON so the two drivers cannot be given different tables.
///
/// For every error flag and robot mode the maker documents, and every mode outside the table: the flags the class requires and never one it
/// forbids, the fault word, a command refused or taken as the class says, and the arm reading healthy again when the box clears it. **It
/// agrees with whoever scripted the box**, and so does the table: what it holds is what the driver does with a flag or a mode.
#[test]
fn every_error_and_mode_the_vocabulary_names_is_reported_as_its_class_and_an_unlisted_mode_as_a_fault(
) {
    let _one = one_at_a_time();
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fault-vocabulary.json"
    ))
    .expect("drivers/fault-vocabulary.json is where the contract's table lives");
    let spec = skynet_json::Json::parse(&text).expect("the vocabulary file parses");
    let franka = spec
        .get("drivers")
        .and_then(|drivers| drivers.get("franka"))
        .and_then(|entry| entry.get("vocabularies"))
        .expect("franka has vocabularies");

    // (vocabulary, code, class, name, wants the fault word verbatim)
    let mut cases: Vec<(String, u32, String, String, bool)> = Vec::new();
    for vocabulary in ["errors", "robot_mode"] {
        let entry = franka
            .get(vocabulary)
            .unwrap_or_else(|| panic!("franka has a `{vocabulary}` vocabulary"));
        let verbatim = entry.get("fault_code").and_then(|k| k.as_str()) == Some("verbatim");
        for (key, code) in entry.get("codes").and_then(|c| c.as_obj()).expect("codes") {
            let class = code
                .get("class")
                .and_then(|c| c.as_str())
                .expect("a class")
                .to_string();
            let name = code
                .get("name")
                .and_then(|c| c.as_str())
                .expect("a name")
                .to_string();
            cases.push((
                vocabulary.to_string(),
                key.parse().expect("a numeric code"),
                class,
                name,
                verbatim,
            ));
        }
        let escalation = entry
            .get("unmapped")
            .and_then(|c| c.as_str())
            .expect("an escalation")
            .to_string();
        assert!(
            escalation == "fault" || escalation == "estop",
            "an unlisted code escalates to a fault or a stop, never to healthy"
        );
        for probe in entry
            .get("probes")
            .and_then(|p| p.as_arr())
            .expect("probes")
        {
            let code = probe.as_f64().expect("a numeric probe") as u32;
            cases.push((
                vocabulary.to_string(),
                code,
                escalation.clone(),
                format!("unlisted ({code})"),
                verbatim,
            ));
        }
    }
    assert!(
        cases.len() >= 52,
        "the tables and their probes were read: {} cases",
        cases.len()
    );

    for (vocabulary, code, class, name, verbatim) in cases {
        let mut script = Script::new();
        if vocabulary == "errors" {
            // The flag alone, with the robot mode as it was: the Reflex mode that a real arm is in is the `robot_mode` vocabulary's row.
            script.errors = vec![code as usize - 1];
        } else {
            script.robot_mode = code as u8;
        }
        let (scripted, ctx) = opened(script);
        let at = format!("{vocabulary} {code} {name} -> {class}");
        let reading = read(ctx);
        assert_eq!(reading.rc, status::OK, "{at}");
        let (required, forbidden, refused) = match class.as_str() {
            "ok" => (0, flags::ESTOP | flags::FAULT | flags::DEGRADED, false),
            "degraded" => (flags::DEGRADED, flags::ESTOP | flags::FAULT, false),
            "fault" => (flags::FAULT | flags::DEGRADED, flags::ESTOP, true),
            "estop" => (flags::ESTOP | flags::DEGRADED, 0, true),
            other => panic!("{at}: `{other}` is not one of the four classes"),
        };
        assert_eq!(
            reading.flags & required,
            required,
            "{at}: the class requires these flags, read {:#06x}",
            reading.flags
        );
        assert_eq!(
            reading.flags & forbidden,
            0,
            "{at}: and none of these, read {:#06x}",
            reading.flags
        );
        if verbatim {
            assert_eq!(
                reading.fault_code, code,
                "{at}: the fault word is the maker's code, verbatim"
            );
        } else if class == "ok" {
            assert_eq!(reading.fault_code, 0, "{at}: and zero when healthy");
        }
        let command = write(ctx, Mode::Position, &TARGET, &ALL);
        if refused {
            assert_eq!(command, status::EPERM, "{at}: a command is refused, because `SKR_OK` would report success the robot did not have");
        } else {
            assert_eq!(command, status::OK, "{at}: a command is taken");
        }
        // The box clears it, as a person recovering the arm does: a fault the driver holds after the box has let go is F142's shape.
        scripted.with(|s| {
            s.errors.clear();
            s.robot_mode = fci::ROBOT_MODE_IDLE;
        });
        let healed = read_until(ctx, "healthy again", |r| {
            r.rc == status::OK && r.flags & (flags::ESTOP | flags::FAULT | flags::DEGRADED) == 0
        });
        assert_eq!(healed.fault_code, 0, "{at}: and the fault word with it");
        assert_eq!(
            write(ctx, Mode::Position, &TARGET, &ALL),
            status::OK,
            "{at}: and a command is taken again"
        );
        assert_eq!(close(ctx), status::OK);
    }
}

#[test]
fn the_newest_state_in_the_buffer_is_the_one_read_and_a_silent_stream_is_not_read_as_fresh() {
    let _one = one_at_a_time();
    let (scripted, ctx) = opened(Script::new());
    // The box goes quiet and a burst is already queued: the driver reads the newest, not the oldest of a second of history (F27).
    scripted.with(|s| s.stream = false);
    thread::sleep(Duration::from_millis(60));
    let source = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    let port = scripted.driver_port();
    for step in 0..30 {
        let mut state = START_Q;
        state[0] = 0.0625 * step as f32;
        let mut script = Script::new();
        script.q = state;
        source
            .send_to(&script.datagram(5000 + step), ("127.0.0.1", port))
            .unwrap();
    }
    thread::sleep(Duration::from_millis(50));
    let newest = read(ctx);
    assert_eq!(newest.rc, status::OK);
    assert_eq!(
        newest.q[0],
        0.0625 * 29.0,
        "the drain keeps the last datagram of the burst"
    );
    // Nothing more arrives: two reads later the stream is silent and the driver says so rather than repeating the last.
    let silent = read(ctx);
    assert_eq!(
        silent.rc,
        status::EIO,
        "a stream that has stopped is an I/O error, not the last state served again as fresh"
    );
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn commands_follow_the_stream_to_where_it_now_comes_from_and_not_to_a_datagram_that_is_not_a_state()
{
    let _one = one_at_a_time();
    let (scripted, ctx) = opened(Script::new());
    let port = scripted.driver_port();

    // Datagrams that are not states — too long, too short — must not move where commands go. They arrive last, after the valid
    // ones, which is the order in which a peer cached from any datagram would be wrong.
    thread::sleep(Duration::from_millis(80)); // the stream runs, unread: states queue in the driver's socket
    scripted.with(|s| s.stream = false);
    thread::sleep(Duration::from_millis(40));
    let stranger = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    stranger
        .set_read_timeout(Some(Duration::from_millis(300)))
        .unwrap();
    let valid = Script::new().datagram(7000);
    let mut long = valid.clone();
    long.extend_from_slice(&[0u8; 8]);
    stranger.send_to(&long, ("127.0.0.1", port)).unwrap();
    stranger
        .send_to(&valid[..valid.len() - 100], ("127.0.0.1", port))
        .unwrap();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(
        read(ctx).rc,
        status::OK,
        "the valid states queued before them are read"
    );
    assert_eq!(write(ctx, Mode::Position, &TARGET, &ALL), status::OK);
    scripted.wait_for("the command", |s| !s.commands.is_empty());
    let mut buffer = [0u8; 4096];
    assert!(
        stranger.recv_from(&mut buffer).is_err(),
        "a datagram that is not a state is not where commands are sent"
    );
    assert!(
        scripted.with(|s| s.commands.iter().all(|c| c.socket == 0)),
        "they went to the box's own socket, where the states came from"
    );

    // A box that restarts its stream from another socket is followed.
    scripted.with(|s| s.stream = false);
    thread::sleep(Duration::from_millis(60));
    let restarted = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    restarted
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    for step in 0..5 {
        restarted
            .send_to(&Script::new().datagram(8000 + step), ("127.0.0.1", port))
            .unwrap();
    }
    thread::sleep(Duration::from_millis(50));
    let _ = read_until(ctx, "the restarted stream", |_| true);
    assert_eq!(write(ctx, Mode::Position, &TARGET, &ALL), status::OK);
    let (n, _) = restarted
        .recv_from(&mut buffer)
        .expect("the command goes to where the newest state came from");
    assert_eq!(n, fci::ROBOT_COMMAND_SIZE, "a whole RobotCommand");
    assert_eq!(close(ctx), status::OK);
}

// ── what goes back ───────────────────────────────────────────────────────────

#[test]
fn each_mode_starts_the_motion_and_controller_it_needs_and_writes_the_slot_that_motion_reads() {
    let _one = one_at_a_time();
    let (scripted, ctx) = opened(Script::new());
    let id_before = scripted.with(|s| s.last_id);
    scripted.wait_for("a state newer than the one noted", |s| {
        s.last_id > id_before
    });
    let _ = read(ctx);

    // Position: joint impedance control, a joint-position motion, q_c.
    assert_eq!(write(ctx, Mode::Position, &TARGET, &ALL), status::OK);
    assert_eq!(
        scripted.with(|s| s.moves.clone()),
        // Move::ControllerMode { kJointImpedance 0, kCartesianImpedance 1, kExternalController 2 } and
        // Move::MotionGeneratorMode { kJointPosition 0, kJointVelocity 1, kCartesianPosition 2, kCartesianVelocity 3, kNone 4 }:
        // service_types.h's numbers, typed here.
        [(0, 0)],
        "a position command starts a joint-position motion under the joint impedance controller"
    );
    let first = command_after(&scripted, 0);
    assert_eq!(first.fields.q_c, as_f64(&TARGET));
    assert!(
        first.fields.dq_c.iter().all(|v| *v == 0.0)
            && first.fields.tau_j_d.iter().all(|v| *v == 0.0),
        "and only the slot that motion reads"
    );
    assert!(first.fields.message_id > id_before, "the command echoes the message id of the state it answers, not a stale or zero one: {} after {id_before}", first.fields.message_id);
    assert!(first.fields.message_id <= scripted.with(|s| s.last_id));

    // The same mode again does not start a second motion.
    assert_eq!(write(ctx, Mode::Position, &OTHER, &ALL), status::OK);
    assert_eq!(
        scripted.with(|s| s.moves.len()),
        1,
        "a motion already running in this mode is joined, not restarted"
    );

    // Velocity: the running motion is stopped, a joint-velocity one started, dq_c.
    let spin = [0.0625f32, -0.125, 0.1875, -0.25, 0.3125, -0.375, 0.4375];
    assert_eq!(write(ctx, Mode::Velocity, &spin, &ALL), status::OK);
    assert_eq!(
        scripted.with(|s| s.moves.clone())[1],
        (0, 1),
        "a velocity command starts a joint-velocity motion, not a position one"
    );
    assert_eq!(
        scripted
            .with(|s| s.tcp_log.clone())
            .iter()
            .filter(|c| **c == fci::COMMAND_STOP_MOVE)
            .count(),
        1,
        "after stopping the one that was running"
    );
    let velocity = command_where(&scripted, "the velocity command", |c| {
        c.dq_c == as_f64(&spin)
    });
    assert!(
        velocity.fields.q_c.iter().all(|v| *v == 0.0)
            && velocity.fields.tau_j_d.iter().all(|v| *v == 0.0)
    );

    // Torque: the external controller, no motion generator, tau_J_d.
    let effort = [1.5f32, -2.5, 3.5, -4.5, 5.5, -6.5, 7.5];
    assert_eq!(write(ctx, Mode::Torque, &effort, &ALL), status::OK);
    assert_eq!(scripted.with(|s| s.moves.clone())[2], (2, 4), "a torque command runs under the external controller with no motion generator: the box's own controller must not fight it");
    let torque = command_where(&scripted, "the torque command", |c| {
        c.tau_j_d == as_f64(&effort)
    });
    assert!(
        torque.fields.q_c.iter().all(|v| *v == 0.0) && torque.fields.dq_c.iter().all(|v| *v == 0.0),
        "and never in the position or velocity slot"
    );
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn a_move_the_box_refuses_is_a_refused_command_and_a_stray_reply_is_not_its_answer() {
    let _one = one_at_a_time();
    for refusal in [
        fci::MOVE_REJECTED_DUE_TO_SAFETY,
        fci::MOVE_COMMAND_NOT_POSSIBLE_REJECTED,
        fci::MOVE_ABORTED,
    ] {
        let mut script = Script::new();
        script.move_status = refusal;
        let (scripted, ctx) = opened(script);
        assert_eq!(
            write(ctx, Mode::Position, &TARGET, &ALL),
            status::EPERM,
            "a Move answered `{}` starts nothing, and commands sent into it would go nowhere",
            fci::move_status_name(refusal)
        );
        assert!(
            scripted.settled(|s| s.commands.is_empty()),
            "and no command was written into a stream nothing reads"
        );
        assert_eq!(close(ctx), status::OK);
    }
    let mut script = Script::new();
    script.move_status = fci::MOVE_SUCCESS;
    script.stray_before_move = true;
    let (scripted, ctx) = opened(script);
    assert_eq!(
        write(ctx, Mode::Position, &TARGET, &ALL),
        status::OK,
        "the Move's own reply is found by its id, past an answer to some other exchange"
    );
    scripted.wait_for("a command", |s| !s.commands.is_empty());
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn a_command_into_a_latched_error_is_refused_and_so_is_one_under_the_stop_latch() {
    let _one = one_at_a_time();
    let mut script = Script::new();
    script.errors = vec![2];
    let (scripted, ctx) = opened(script);
    assert_eq!(
        write(ctx, Mode::Position, &TARGET, &ALL),
        status::EPERM,
        "a latched error is refused, not commanded into"
    );
    assert!(
        scripted.settled(|s| s.moves.is_empty()),
        "and no motion was started"
    );
    assert_eq!(close(ctx), status::OK);

    // The latch the crate root holds is honoured inside the connection as well, which is where the re-check under the lock lives.
    let scripted = ScriptedBox::start(Script::new());
    let mut connection = Connection::new();
    let config = Config::parse(&scripted.config()).unwrap();
    let _stop_handle = connection.open(&config).expect("a connection");
    assert_eq!(
        connection.write_command(Mode::Position, &TARGET, &ALL, true),
        status::EPERM,
        "the latch argument is read"
    );
    assert!(
        scripted.settled(|s| s.moves.is_empty()),
        "and nothing was sent"
    );
    assert_eq!(
        connection.write_command(Mode::Position, &TARGET, &ALL, false),
        status::OK
    );
    connection.close();
}

#[test]
fn a_hold_holds_at_the_last_setpoint_and_a_setpoint_does_not_outlive_what_voided_it() {
    let _one = one_at_a_time();
    let (scripted, ctx) = opened(Script::new());
    let _ = read(ctx);
    assert_eq!(write(ctx, Mode::Position, &TARGET, &ALL), status::OK);

    // The arm sags under the hold: the reading moves away from the setpoint, and a hold goes on asking for the setpoint.
    let sagged = [0.0f32, -0.4375, 0.3125, -1.9375, 0.4375, 1.1875, 0.8125];
    scripted.with(|s| s.q = sagged);
    let _ = read_until(ctx, "the sagged pose", |r| r.q == sagged);
    let seen = scripted.with(|s| s.commands.len());
    assert_eq!(write(ctx, Mode::Position, &OTHER, &NONE), status::OK);
    assert_eq!(
        command_after(&scripted, seen).fields.q_c,
        as_f64(&TARGET),
        "a hold holds at the setpoint it was last given, not at where the arm has sagged to"
    );

    // Anything outside position voids it: the arm may have moved under a velocity command, and a later hold must not jump back.
    assert_eq!(write(ctx, Mode::Velocity, &[0.0; 7], &ALL), status::OK);
    let moved = [0.5f32, -0.3125, 0.25, -1.5, 0.375, 1.125, 0.625];
    scripted.with(|s| s.q = moved);
    let _ = read_until(ctx, "the moved pose", |r| r.q == moved);
    let seen = scripted.with(|s| s.commands.len());
    assert_eq!(write(ctx, Mode::Position, &OTHER, &NONE), status::OK);
    assert_eq!(command_after(&scripted, seen).fields.q_c, as_f64(&moved), "after a velocity command a hold latches where the arm is, and does not jump back to the old setpoint");
    assert_eq!(close(ctx), status::OK);

    // No hold and no setpoint carries into a new session.
    scripted.with(|s| s.q = START_Q);
    let (rc, ctx) = open_driver(&scripted.config());
    assert_eq!(rc, status::OK);
    assert_eq!(write(ctx, Mode::Position, &TARGET, &ALL), status::OK);
    assert_eq!(close(ctx), status::OK);
    let fresh = [0.25f32, -0.5, 0.5, -2.0, 0.125, 1.375, 0.5];
    scripted.with(|s| s.q = fresh);
    let (rc, ctx) = open_driver(&scripted.config());
    assert_eq!(rc, status::OK);
    let seen = scripted.with(|s| s.commands.len());
    assert_eq!(write(ctx, Mode::Position, &OTHER, &NONE), status::OK);
    assert_eq!(command_after(&scripted, seen).fields.q_c, as_f64(&fresh), "the first hold of a new session holds where the arm is, not at the last session's setpoint");
    assert_eq!(close(ctx), status::OK);
}

// ── the stop ─────────────────────────────────────────────────────────────────

#[test]
fn the_stop_is_stop_move_and_nothing_else_it_latches_and_release_lets_a_new_motion_start() {
    let _one = one_at_a_time();
    let (scripted, ctx) = opened(Script::new());
    assert_eq!(write(ctx, Mode::Position, &TARGET, &ALL), status::OK);
    assert_eq!(scripted.with(|s| s.moves.len()), 1);

    assert_eq!(stop(ctx), status::OK, "emergency_stop returns");
    scripted.wait_for("StopMove", |s| s.count(fci::COMMAND_STOP_MOVE) == 1);
    assert_eq!(
        scripted.settled(|s| s.count(fci::COMMAND_AUTOMATIC_ERROR_RECOVERY)),
        0,
        "the stop does not clear the error that says why the robot stopped"
    );
    let state = read(ctx);
    assert_ne!(
        state.flags & flags::ESTOP,
        0,
        "the next state says the stop is asserted"
    );
    let sent = scripted.with(|s| (s.commands.len(), s.moves.len()));
    assert_eq!(
        write(ctx, Mode::Position, &TARGET, &ALL),
        status::EPERM,
        "and a command after it is refused"
    );
    assert_eq!(
        scripted.settled(|s| (s.commands.len(), s.moves.len())),
        sent,
        "before anything is written"
    );

    assert_eq!(release(ctx), status::OK, "release_stop returns");
    assert_eq!(read(ctx).flags & flags::ESTOP, 0, "and the flag is gone");
    assert_eq!(
        write(ctx, Mode::Position, &TARGET, &ALL),
        status::OK,
        "commands are accepted again"
    );
    assert_eq!(
        scripted.with(|s| s.moves.len()),
        2,
        "and the stopped motion is over, so the command starts a new one rather than joining it"
    );
    assert_eq!(close(ctx), status::OK);
}

#[test]
fn a_stop_that_reached_nobody_does_not_say_it_did_and_a_close_stops_a_running_motion() {
    let _one = one_at_a_time();
    let (scripted, ctx) = opened(Script::new());
    scripted.with(|s| s.drop_tcp = true);
    thread::sleep(Duration::from_millis(150));
    let mut last = status::OK;
    for _ in 0..20 {
        last = stop(ctx);
        if last != status::OK {
            break;
        }
        thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(last, status::EIO, "a stop whose message cannot be written is EIO: the latch is set, and claiming a stop that was not sent is the lie");
    assert_eq!(close(ctx), status::OK);
    drop(scripted);

    let (scripted, ctx) = opened(Script::new());
    assert_eq!(write(ctx, Mode::Position, &TARGET, &ALL), status::OK);
    assert_eq!(close(ctx), status::OK);
    assert_eq!(
        scripted.with(|s| s.count(fci::COMMAND_STOP_MOVE)),
        1,
        "close ends the motion it started"
    );
    let (scripted, ctx) = opened(Script::new());
    assert_eq!(close(ctx), status::OK);
    assert_eq!(
        scripted.with(|s| s.count(fci::COMMAND_STOP_MOVE)),
        0,
        "and stops nothing when there was no motion"
    );
}

#[test]
fn a_stop_does_not_outlive_its_session() {
    let _one = one_at_a_time();
    let (scripted, ctx) = opened(Script::new());
    assert_eq!(write(ctx, Mode::Position, &TARGET, &ALL), status::OK);
    assert_eq!(stop(ctx), status::OK);
    assert_eq!(write(ctx, Mode::Position, &TARGET, &ALL), status::EPERM);
    assert_eq!(close(ctx), status::OK);
    let (rc, ctx) = open_driver(&scripted.config());
    assert_eq!(rc, status::OK);
    assert_eq!(
        write(ctx, Mode::Position, &TARGET, &ALL),
        status::OK,
        "the same driver, opened again after a stop and a close, commands again (F76)"
    );
    assert_eq!(stop(ctx), status::OK);
    assert_eq!(close(ctx), status::OK);
    assert_eq!(
        stop(ctx),
        status::OK,
        "a stop after close has nothing to stop and says so"
    );
}

// ── the body ─────────────────────────────────────────────────────────────────

#[test]
fn the_descriptor_carries_the_vendors_limits_roles_and_the_slug_it_was_given_and_needs_room_for_its_terminator(
) {
    let _one = one_at_a_time();
    let scripted = ScriptedBox::start(Script::new());
    let (rc, ctx) = open_driver(&format!(
        r#"{{"host": "127.0.0.1", "port": {}, "robot_slug": "my \"arm\" \\ 1"}}"#,
        scripted.port
    ));
    assert_eq!(rc, status::OK);
    let describe = |len: usize| -> (i32, String) {
        let mut buffer = vec![0u8; len.max(1)];
        // SAFETY: a live context and a buffer of the length passed.
        let n = unsafe {
            (driver().describe.unwrap())(ctx, buffer.as_mut_ptr() as *mut std::ffi::c_char, len)
        };
        (
            n,
            if n > 0 {
                String::from_utf8_lossy(&buffer[..n as usize]).to_string()
            } else {
                String::new()
            },
        )
    };
    let (length, text) = describe(1 << 15);
    let document = skynet_json::Json::parse(&text)
        .expect("a quote and a backslash in the slug leave the descriptor parseable");
    assert_eq!(
        document
            .get("robot_slug")
            .and_then(skynet_json::Json::as_str),
        Some("my \"arm\" \\ 1"),
        "the slug the operator gave is the slug described"
    );
    assert_eq!(
        describe(length as usize).0,
        status::EINVAL,
        "a buffer exactly as long as the text has no room for the terminator"
    );
    assert_eq!(describe(length as usize + 1).0, length);

    // The vendor's joint table (franka_description, robots/fr3/joint_limits.yaml) as it is read from that file, typed here
    // again: a change to a figure is a change to a datasheet claim, made by reading the file and not by editing one row.
    let vendor: [(&str, &str, f64, f64, f64, f64); 7] = [
        (
            "panda_joint1",
            "right_arm.shoulder.yaw",
            -2.7437,
            2.7437,
            2.62,
            87.0,
        ),
        (
            "panda_joint2",
            "right_arm.shoulder.pitch",
            -1.7837,
            1.7837,
            2.62,
            87.0,
        ),
        (
            "panda_joint3",
            "right_arm.upper_arm.roll",
            -2.9007,
            2.9007,
            2.62,
            87.0,
        ),
        (
            "panda_joint4",
            "right_arm.elbow.flex",
            -3.0421,
            -0.1518,
            2.62,
            87.0,
        ),
        (
            "panda_joint5",
            "right_arm.forearm.roll",
            -2.8065,
            2.8065,
            5.26,
            12.0,
        ),
        (
            "panda_joint6",
            "right_arm.wrist.pitch",
            0.5445,
            4.5169,
            4.18,
            12.0,
        ),
        (
            "panda_joint7",
            "right_arm.wrist.roll",
            -3.0159,
            3.0159,
            5.26,
            12.0,
        ),
    ];
    let joints = document
        .get("joints")
        .and_then(skynet_json::Json::as_arr)
        .expect("joints");
    assert_eq!(joints.len(), 7);
    for (joint, (name, role, low, high, speed, torque)) in joints.iter().zip(vendor) {
        let number = |key: &str| {
            joint
                .get("limits")
                .and_then(|l| l.get(key))
                .and_then(skynet_json::Json::as_f64)
                .unwrap()
        };
        assert_eq!(
            joint.get("name").and_then(skynet_json::Json::as_str),
            Some(name)
        );
        assert_eq!(
            joint.get("role").and_then(skynet_json::Json::as_str),
            Some(role),
            "{name}: the role names the axis it turns about"
        );
        assert_eq!(
            (number("position_min_rad"), number("position_max_rad")),
            (low, high),
            "{name}: travel"
        );
        assert_eq!(
            (number("velocity_max_rad_s"), number("effort_max_nm")),
            (speed, torque),
            "{name}: ratings"
        );
    }
    let envelope = document.get("envelope").expect("an envelope");
    assert_eq!(
        envelope
            .get("watchdog_ms")
            .and_then(skynet_json::Json::as_f64),
        Some(10.0),
        "ten FCI periods at 1 kHz"
    );
    assert_eq!(
        envelope
            .get("joint_velocity_max_rad_s")
            .and_then(skynet_json::Json::as_f64),
        Some(2.62)
    );
    assert_eq!(
        envelope
            .get("joint_torque_max_nm")
            .and_then(skynet_json::Json::as_f64),
        Some(12.0),
        "the narrowest across the arm"
    );

    // An arm with no images refuses the read, with the status that says so.
    // SAFETY: a live context; this entry point reads neither argument.
    let rc =
        unsafe { (driver().read_sensor.unwrap())(ctx, std::ptr::null(), std::ptr::null_mut()) };
    assert_eq!(
        rc,
        status::ENODEV,
        "a sensor read on an arm with no sensors is refused, never a success"
    );
    assert_eq!(close(ctx), status::OK);
}

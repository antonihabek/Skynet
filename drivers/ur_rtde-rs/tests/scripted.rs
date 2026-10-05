//! The port's own decisions, against a scripted controller on loopback (DRY-COVERAGE-PLAN.md 12.C.3).
//!
//! `live_ursim` is the half that matters and it needs a container; `vectors` holds the bytes to the Python driver's. Between
//! them nothing ran the decisions the driver makes *above* the wire without a controller: what a sample becomes, what the
//! stop writes and to whom, what `release_stop` restarts, what `close` leaves behind, what `open` refuses. The seeded-defect
//! campaign (`tools/seed-defects/`) found that 48 of this crate's 77 realistic defects — a stop that keeps the gate open, a
//! hold package carrying zeros, a latch that outlives its session, a protective stop the driver would power over — left
//! `cargo test` green, because only the URSim half could see them.
//!
//! The controller here is three small servers on the ports a UR uses (29999 dashboard, 30002 program, 30004 RTDE), scripted
//! by the test: which modes the dashboard reports and in what order, what the RTDE stream publishes, how many times the
//! registers answer `IN_USE`, and — what makes the assertions possible — everything it was sent. **It agrees with whoever
//! wrote it**, which is F15's point and the reason `live_ursim` exists: nothing here says the bytes are the controller's.
//!
//! The ports are fixed, so these tests take a lock and run one at a time, and refuse loudly — naming the cause — when
//! something else is listening (a URSim container started for `live_ursim`). The seeding campaign runs each suite in a network
//! namespace of its own (`tools/seed-defects/in-netns.sh`), so its workers do not meet.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use skynet_rhal::{flags, status, Command as RhalCommand, Ctx, Homing, Mode, RawDriver, State};
use skynet_ur_rtde_rs::bridge::{Config, Connection, OpenError, Sample, INPUTS, OUTPUTS};
use skynet_ur_rtde_rs::control;
use skynet_ur_rtde_rs::dashboard::{Dashboard, DashboardError};
use skynet_ur_rtde_rs::rtde::{self, Session, Value};

// ── the scripted controller ──────────────────────────────────────────────────

/// Joints that differ from each other and from zero, in values an `f32` holds exactly, so a sign, an order or a home pose shows.
const Q0: [f64; 6] = [0.125, -0.25, 0.375, -0.5, 0.625, -0.75];
const QD0: [f64; 6] = [0.0625, -0.125, 0.1875, -0.25, 0.3125, -0.375];

/// One input package the controller received.
#[derive(Debug, Clone, PartialEq)]
struct Written {
    recipe: u8,
    setpoint: [f64; 6],
    gate: i32,
}

/// What the controller says, and what it has been sent.
struct Script {
    /// The dashboard's `safetystatus` word.
    safety: String,
    /// The `robotmode` answers, in order; the last repeats.
    modes: Vec<String>,
    /// The sample the RTDE stream publishes.
    q: [f64; 6],
    qd: [f64; 6],
    robot_mode: i32,
    safety_status: i32,
    /// How many `setup_inputs` the controller answers `IN_USE` before it answers.
    in_use_attempts: usize,
    /// How it types the input registers when it does answer.
    input_types: String,
    /// Whether the protocol version is accepted.
    accept_version: bool,
    /// Whether the stream publishes at the negotiated rate, and how many extra samples to send at once.
    publish: bool,
    burst: usize,
    /// Close the dashboard / RTDE connections, as a controller that went away does.
    drop_dashboard: bool,
    drop_rtde: bool,
    /// Whether the controller answers the pause of the exchange. A controller that has gone quiet does not.
    answer_pause: bool,
    // What it saw.
    dashboard: Vec<String>,
    writes: Vec<Written>,
    program: Vec<u8>,
    pauses: usize,
    rtde_sessions: usize,
    setup_outputs: Vec<(f64, String)>,
    setup_inputs: Vec<String>,
}

impl Script {
    fn new() -> Script {
        Script {
            safety: "NORMAL".into(),
            modes: vec!["RUNNING".into()],
            q: Q0,
            qd: QD0,
            robot_mode: 7,
            safety_status: 1,
            in_use_attempts: 0,
            input_types: rtde::INPUT_TYPES.join(","),
            accept_version: true,
            publish: true,
            burst: 0,
            drop_dashboard: false,
            drop_rtde: false,
            answer_pause: true,
            dashboard: Vec::new(),
            writes: Vec::new(),
            program: Vec::new(),
            pauses: 0,
            rtde_sessions: 0,
            setup_outputs: Vec::new(),
            setup_inputs: Vec::new(),
        }
    }

    /// One output package under the recipe the driver asked for: three vectors, two modes and a runtime state.
    fn package(&self, recipe: u8) -> Vec<u8> {
        let mut body = vec![recipe];
        for vector in [&self.q, &self.qd, &[0.0; 6]] {
            for value in vector {
                body.extend_from_slice(&value.to_be_bytes());
            }
        }
        body.extend_from_slice(&self.robot_mode.to_be_bytes());
        body.extend_from_slice(&self.safety_status.to_be_bytes());
        body.extend_from_slice(&2u32.to_be_bytes());
        frame(rtde::DATA_PACKAGE, &body)
    }
}

fn frame(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut packet = ((3 + body.len()) as u16).to_be_bytes().to_vec();
    packet.push(kind);
    packet.extend_from_slice(body);
    packet
}

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

#[derive(Clone, Copy)]
enum Kind {
    Dashboard,
    Program,
    Rtde,
}

struct Controller {
    shared: Shared,
    stop: Arc<AtomicBool>,
    servers: Vec<JoinHandle<()>>,
}

impl Controller {
    fn start(script: Script) -> Controller {
        Controller::start_on(script, (29999, 30002, 30004))
    }

    /// A controller whose dashboard, secondary interface and RTDE listen on these ports, for the configuration that moves them.
    fn start_on(script: Script, ports: (u16, u16, u16)) -> Controller {
        let shared: Shared = Arc::new(Mutex::new(script));
        let stop = Arc::new(AtomicBool::new(false));
        let mut servers = Vec::new();
        for (port, kind) in [
            (ports.0, Kind::Dashboard),
            (ports.1, Kind::Program),
            (ports.2, Kind::Rtde),
        ] {
            let listener = TcpListener::bind(("127.0.0.1", port)).unwrap_or_else(|error| {
                panic!(
                    "the scripted controller needs 127.0.0.1:{port} and it is taken ({error}). A URSim container, or another \
                     controller, is listening there: stop it, or run the URSim half alone with `cargo test --test live_ursim`."
                )
            });
            listener
                .set_nonblocking(true)
                .expect("a non-blocking listener");
            let shared = shared.clone();
            let stop = stop.clone();
            servers.push(thread::spawn(move || serve(listener, kind, shared, stop)));
        }
        Controller {
            shared,
            stop,
            servers,
        }
    }

    fn with<R>(&self, act: impl FnOnce(&mut Script) -> R) -> R {
        act(&mut locked(&self.shared))
    }

    /// Wait until `condition` holds of what the controller has seen, or fail saying what never happened.
    fn wait_for(&self, what: &str, condition: impl Fn(&Script) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if condition(&locked(&self.shared)) {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("the controller never saw {what} within five seconds");
    }

    /// What the controller has seen after it has had time to: for asserting that something did *not* reach it.
    fn settled<R>(&self, read: impl Fn(&Script) -> R) -> R {
        thread::sleep(Duration::from_millis(150));
        read(&locked(&self.shared))
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for server in self.servers.drain(..) {
            let _ = server.join();
        }
    }
}

fn serve(listener: TcpListener, kind: Kind, shared: Shared, stop: Arc<AtomicBool>) {
    let mut handlers: Vec<JoinHandle<()>> = Vec::new();
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                stream.set_nonblocking(false).expect("a blocking stream");
                stream
                    .set_read_timeout(Some(Duration::from_millis(20)))
                    .expect("a read timeout");
                let shared = shared.clone();
                let stop = stop.clone();
                handlers.push(thread::spawn(move || match kind {
                    Kind::Dashboard => dashboard(stream, shared, stop),
                    Kind::Program => program(stream, shared, stop),
                    Kind::Rtde => rtde_session(stream, shared, stop),
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

fn dashboard(stream: TcpStream, shared: Shared, stop: Arc<AtomicBool>) {
    let mut writer = stream.try_clone().expect("a second handle");
    if writer
        .write_all(b"Connected: Universal Robots Dashboard Server\n")
        .is_err()
    {
        return;
    }
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    loop {
        if stop.load(Ordering::Acquire) || locked(&shared).drop_dashboard {
            return;
        }
        match reader.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) if line.ends_with('\n') => {
                let command = line.trim().to_string();
                line.clear();
                let reply = {
                    let mut script = locked(&shared);
                    script.dashboard.push(command.clone());
                    match command.as_str() {
                        "robotmode" => {
                            let mode = if script.modes.len() > 1 {
                                script.modes.remove(0)
                            } else {
                                script.modes[0].clone()
                            };
                            format!("Robotmode: {mode}")
                        }
                        "safetystatus" => format!("Safetystatus: {}", script.safety),
                        "power on" => "Powering on".into(),
                        "brake release" => "Brake releasing".into(),
                        "stop" => "Stopped".into(),
                        "play" => "Starting program".into(),
                        _ => "ok".into(),
                    }
                };
                if writer.write_all(format!("{reply}\n").as_bytes()).is_err() {
                    return;
                }
            }
            Ok(_) => {}
            Err(error) if timed_out(&error) => {}
            Err(_) => return,
        }
    }
}

fn program(mut stream: TcpStream, shared: Shared, stop: Arc<AtomicBool>) {
    let mut chunk = [0u8; 4096];
    loop {
        if stop.load(Ordering::Acquire) {
            return;
        }
        match stream.read(&mut chunk) {
            Ok(0) => return,
            Ok(n) => locked(&shared).program.extend_from_slice(&chunk[..n]),
            Err(error) if timed_out(&error) => {}
            Err(_) => return,
        }
    }
}

/// RTDE's type for each output the driver can ask for.
fn type_of(name: &str) -> &'static str {
    match name {
        "actual_q" | "actual_qd" | "actual_current" => "VECTOR6D",
        "robot_mode" | "safety_status" => "INT32",
        "runtime_state" => "UINT32",
        _ => "NOT_FOUND",
    }
}

fn rtde_session(mut stream: TcpStream, shared: Shared, stop: Arc<AtomicBool>) {
    locked(&shared).rtde_sessions += 1;
    let writer = Arc::new(Mutex::new(stream.try_clone().expect("a second handle")));
    let send = |kind: u8, body: &[u8]| -> bool {
        writer
            .lock()
            .map(|mut w| w.write_all(&frame(kind, body)).is_ok())
            .unwrap_or(false)
    };
    let streaming = Arc::new(AtomicBool::new(false));
    let mut streamer: Option<JoinHandle<()>> = None;
    let mut output_recipe = 0u8;
    let mut frequency = 125.0f64;
    let mut buffer: Vec<u8> = Vec::new();
    'session: loop {
        if stop.load(Ordering::Acquire) || locked(&shared).drop_rtde {
            break;
        }
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(error) if timed_out(&error) => {}
            Err(_) => break,
        }
        while buffer.len() >= 3 {
            let size = u16::from_be_bytes([buffer[0], buffer[1]]) as usize;
            if buffer.len() < size {
                break;
            }
            let kind = buffer[2];
            let body = buffer[3..size].to_vec();
            buffer.drain(..size);
            match kind {
                rtde::REQUEST_PROTOCOL_VERSION => {
                    let accepted = locked(&shared).accept_version;
                    // A controller's first words come unsolicited and in the middle of a handshake (rtde.rs's header).
                    send(
                        rtde::TEXT_MESSAGE,
                        b"Safety Setup has not been confirmed yet",
                    );
                    send(kind, &[u8::from(accepted)]);
                }
                rtde::GET_URCONTROL_VERSION => {
                    let mut version = Vec::new();
                    for part in [5u32, 26, 0, 0] {
                        version.extend_from_slice(&part.to_be_bytes());
                    }
                    send(kind, &version);
                }
                rtde::SETUP_OUTPUTS => {
                    frequency = f64::from_be_bytes(body[..8].try_into().expect("a frequency"));
                    let names = String::from_utf8_lossy(&body[8..]).to_string();
                    let types: Vec<&str> = names.split(',').map(type_of).collect();
                    locked(&shared).setup_outputs.push((frequency, names));
                    output_recipe = 1;
                    let mut reply = vec![output_recipe];
                    reply.extend_from_slice(types.join(",").as_bytes());
                    send(kind, &reply);
                }
                rtde::SETUP_INPUTS => {
                    let names = String::from_utf8_lossy(&body).to_string();
                    let reply = {
                        let mut script = locked(&shared);
                        script.setup_inputs.push(names.clone());
                        if script.in_use_attempts > 0 {
                            script.in_use_attempts -= 1;
                            let mut reply = vec![0u8];
                            reply.extend_from_slice(
                                vec!["IN_USE"; names.split(',').count()]
                                    .join(",")
                                    .as_bytes(),
                            );
                            reply
                        } else {
                            let mut reply = vec![2u8];
                            reply.extend_from_slice(script.input_types.as_bytes());
                            reply
                        }
                    };
                    send(kind, &reply);
                }
                rtde::START => {
                    send(kind, &[1]);
                    streaming.store(true, Ordering::Release);
                    let (streaming, shared, stop, writer) = (
                        streaming.clone(),
                        shared.clone(),
                        stop.clone(),
                        writer.clone(),
                    );
                    let period = Duration::from_secs_f64(1.0 / frequency);
                    streamer = Some(thread::spawn(move || {
                        let mut last = Instant::now() - period;
                        while streaming.load(Ordering::Acquire) && !stop.load(Ordering::Acquire) {
                            let packet = {
                                let mut script = locked(&shared);
                                if script.burst > 0 {
                                    script.burst -= 1;
                                    Some(script.package(output_recipe))
                                } else if script.publish && last.elapsed() >= period {
                                    Some(script.package(output_recipe))
                                } else {
                                    None
                                }
                            };
                            if let Some(packet) = packet {
                                last = Instant::now();
                                if writer
                                    .lock()
                                    .map(|mut w| w.write_all(&packet).is_err())
                                    .unwrap_or(true)
                                {
                                    return;
                                }
                            }
                            thread::sleep(Duration::from_millis(1));
                        }
                    }));
                }
                rtde::PAUSE => {
                    streaming.store(false, Ordering::Release);
                    locked(&shared).pauses += 1;
                    if locked(&shared).answer_pause {
                        send(kind, &[1]);
                    }
                }
                rtde::DATA_PACKAGE if body.len() == 1 + 48 + 4 => {
                    let mut setpoint = [0.0f64; 6];
                    for (index, value) in setpoint.iter_mut().enumerate() {
                        *value = f64::from_be_bytes(
                            body[1 + index * 8..9 + index * 8]
                                .try_into()
                                .expect("a double"),
                        );
                    }
                    let gate = i32::from_be_bytes(body[49..53].try_into().expect("a gate"));
                    locked(&shared).writes.push(Written {
                        recipe: body[0],
                        setpoint,
                        gate,
                    });
                }
                _ => {}
            }
            if stop.load(Ordering::Acquire) {
                break 'session;
            }
        }
    }
    streaming.store(false, Ordering::Release);
    if let Some(streamer) = streamer {
        let _ = streamer.join();
    }
}

// ── the tests' own grip on the driver ────────────────────────────────────────

/// The ports are fixed and the driver's state is one static body: one test at a time.
fn one_at_a_time() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn config(hz: f64) -> Config {
    Config::parse(&format!(
        r#"{{"host": "127.0.0.1", "frequency_hz": {hz}, "model": "UR5e"}}"#
    ))
    .expect("a configuration")
}

fn quietly(_: &str) {}

fn opened(script: Script, hz: f64) -> (Controller, Connection) {
    let controller = Controller::start(script);
    let connection = Connection::open(&config(hz), &quietly).unwrap_or_else(|error| {
        panic!(
            "the scripted controller refused to bring up: {}",
            error.message()
        )
    });
    (controller, connection)
}

fn clock() -> u64 {
    0
}

fn driver() -> &'static RawDriver {
    // SAFETY: the entry point returns a pointer to a static vtable.
    unsafe { &*skynet_ur_rtde_rs::skynet_rhal_entry() }
}

fn open_driver(text: &str) -> (i32, *mut Ctx) {
    let config = std::ffi::CString::new(text).unwrap();
    let mut ctx: *mut Ctx = std::ptr::null_mut();
    // SAFETY: a NUL-terminated config and a live out-pointer.
    let rc = unsafe { (driver().open.unwrap())(config.as_ptr(), &mut ctx) };
    (rc, ctx)
}

struct Reading {
    rc: i32,
    flags: u16,
    fault_code: u32,
    q: [f32; 6],
    qd: [f32; 6],
    effort: [f32; 6],
    base_pose: [f32; 7],
}

fn read(ctx: *mut Ctx) -> Reading {
    let mut q = [0.0f32; 6];
    let mut qd = [0.0f32; 6];
    let mut effort = [0.0f32; 6];
    let mut state = State {
        stamp_ns: 0,
        seq: 0,
        joint_count: 6,
        flags: 0,
        position_rad: q.as_mut_ptr(),
        velocity_rad_s: qd.as_mut_ptr(),
        effort_nm: effort.as_mut_ptr(),
        base_pose: [0.0; 7],
        base_twist: [0.0; 6],
        fault_code: 0,
    };
    // SAFETY: a live context and a state whose arrays are six long.
    let rc = unsafe { (driver().read_state.unwrap())(ctx, &mut state, 0) };
    Reading {
        rc,
        flags: state.flags,
        fault_code: state.fault_code,
        q,
        qd,
        effort,
        base_pose: state.base_pose,
    }
}

fn write(ctx: *mut Ctx, mode: u8, values: &[f32], mask: &[u8]) -> i32 {
    let command = RhalCommand {
        stamp_ns: 1,
        seq: 1,
        mode,
        flags: 0,
        _pad: [0; 2],
        joint_count: values.len() as u16,
        values: values.as_ptr(),
        mask: mask.as_ptr(),
        modes: std::ptr::null(),
    };
    // SAFETY: a live context and a command whose arrays are `joint_count` long.
    unsafe { (driver().write_command.unwrap())(ctx, &command, 0) }
}

fn position(ctx: *mut Ctx, values: &[f32; 6], mask: &[u8; 6]) -> i32 {
    write(ctx, Mode::Position as u8, values, mask)
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

fn as_f64(values: &[f32; 6]) -> [f64; 6] {
    let mut out = [0.0; 6];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = *value as f64;
    }
    out
}

const ALL: [u8; 6] = [1; 6];
const NONE: [u8; 6] = [0; 6];
const TARGET: [f32; 6] = [0.5, 0.25, -0.125, 0.75, -0.625, 0.0625];

// ── bring-up ─────────────────────────────────────────────────────────────────

#[test]
fn bring_up_waits_for_each_mode_then_negotiates_the_recipes_and_uploads_the_program() {
    let _one = one_at_a_time();
    let mut script = Script::new();
    script.modes = [
        "POWER_OFF",
        "POWER_OFF",
        "POWER_OFF",
        "IDLE",
        "IDLE",
        "RUNNING",
    ]
    .map(String::from)
    .to_vec();
    let (controller, connection) = opened(script, 125.0);
    let log = controller.with(|s| s.dashboard.clone());
    let steps: Vec<&str> = log
        .iter()
        .map(String::as_str)
        .filter(|c| *c != "robotmode")
        .collect();
    assert_eq!(
        steps,
        ["safetystatus", "power on", "brake release"],
        "the safety status first, then power, then the brakes: {log:?}"
    );
    let before_brakes = log.iter().position(|c| c == "brake release").unwrap();
    assert!(
        log[..before_brakes].iter().filter(|c| *c == "robotmode").count() >= 4,
        "the brakes are released after the mode says powered, not after the reply `Powering on` (the driver polled the mode): {log:?}"
    );
    assert_eq!(
        log.iter().filter(|c| *c == "robotmode").count(),
        6,
        "and it asked for the mode until it read RUNNING, and no more: {log:?}"
    );
    assert_eq!(
        controller.with(|s| s.setup_outputs.clone()),
        [(125.0, OUTPUTS.join(","))],
        "the output recipe is the body, at the rate asked"
    );
    assert_eq!(
        controller.with(|s| s.setup_inputs.clone()),
        [INPUTS.join(",")],
        "the input recipe is six setpoints and the gate"
    );
    controller.wait_for("the control program", |s| {
        s.program == control::SERVO_PROGRAM.as_bytes()
    });
    drop(connection);
}

#[test]
fn a_protective_stop_is_refused_before_anything_is_powered_negotiated_or_uploaded() {
    let _one = one_at_a_time();
    let mut script = Script::new();
    script.safety = "PROTECTIVE_STOP".into();
    script.modes = vec!["POWER_OFF".into()];
    let controller = Controller::start(script);
    let refused = Connection::open(&config(125.0), &quietly)
        .err()
        .expect("a robot in a protective stop is not brought up");
    assert!(
        matches!(refused, OpenError::Refused(_)),
        "it is the controller's refusal and not a socket's: {refused:?}"
    );
    assert!(
        refused.message().contains("PROTECTIVE_STOP")
            && refused.message().contains("Clear it at the machine"),
        "{}",
        refused.message()
    );
    let (log, sessions, program) =
        controller.settled(|s| (s.dashboard.clone(), s.rtde_sessions, s.program.clone()));
    assert_eq!(
        log,
        ["safetystatus"],
        "it read the safety status and sent nothing else: no power, no brakes, no unlock"
    );
    assert_eq!(sessions, 0, "no RTDE session was opened");
    assert!(program.is_empty(), "and no program was uploaded");
}

#[test]
fn a_robot_that_does_not_reach_running_is_refused_with_the_mode_it_did_reach() {
    let _one = one_at_a_time();
    let mut script = Script::new();
    script.modes = ["POWER_OFF", "IDLE"].map(String::from).to_vec();
    let controller = Controller::start(script);
    let stream = TcpStream::connect(("127.0.0.1", 29999)).expect("the scripted dashboard");
    let mut dashboard = Dashboard::connect(stream).expect("a banner");
    let refused = dashboard
        .bring_up(Duration::from_millis(700))
        .expect_err("IDLE for ever is not RUNNING");
    let DashboardError::Refused(text) = refused else {
        panic!("a robot stuck at IDLE is the controller's refusal, not a socket's")
    };
    assert!(
        text.contains("`IDLE` rather than `RUNNING`"),
        "it says the mode it reached: {text}"
    );
    let log = controller.with(|s| s.dashboard.clone());
    assert!(
        log.contains(&"power on".to_string()) && log.contains(&"brake release".to_string()),
        "it tried power and the brakes first: {log:?}"
    );
}

#[test]
fn registers_a_closed_session_held_are_waited_out_for_a_bounded_time_and_no_longer() {
    let _one = one_at_a_time();
    let mut script = Script::new();
    script.in_use_attempts = 3;
    let (controller, connection) = opened(script, 125.0);
    assert_eq!(
        controller.with(|s| s.rtde_sessions),
        4,
        "three answers of IN_USE, each on a session of its own, then the one that opened (F81)"
    );
    drop(connection);
    drop(controller);

    let mut script = Script::new();
    script.in_use_attempts = 100_000;
    let controller = Controller::start(script);
    let started = Instant::now();
    let refused = Connection::open(&config(125.0), &quietly)
        .err()
        .expect("registers another client keeps are a refusal");
    let waited = started.elapsed();
    assert!(
        refused.message().contains("IN_USE"),
        "it says why: {}",
        refused.message()
    );
    assert!(
        waited >= Duration::from_millis(1900),
        "it waited out the two seconds a released session needs before refusing: {waited:?}"
    );
    assert!(
        waited < Duration::from_secs(8),
        "and only that long: {waited:?}"
    );
    drop(controller);
}

#[test]
fn a_controller_that_types_the_setpoint_registers_otherwise_is_refused() {
    let _one = one_at_a_time();
    let mut script = Script::new();
    script.input_types = ["DOUBLE"; 7].join(",");
    let controller = Controller::start(script);
    let refused = Connection::open(&config(125.0), &quietly)
        .err()
        .expect("a setpoint typed as seven doubles is not written");
    assert!(
        refused.message().contains("typed the setpoint registers"),
        "{}",
        refused.message()
    );
    assert!(
        controller.settled(|s| s.writes.is_empty()),
        "nothing was written under the wrong layout"
    );
}

#[test]
fn a_controller_that_will_not_speak_protocol_two_is_refused() {
    let _one = one_at_a_time();
    let mut script = Script::new();
    script.accept_version = false;
    let controller = Controller::start(script);
    let refused = Connection::open(&config(125.0), &quietly)
        .err()
        .expect("version 1 has no frequency field");
    assert!(
        refused
            .message()
            .contains("refused RTDE protocol version 2"),
        "{}",
        refused.message()
    );
    assert!(
        controller.settled(|s| s.setup_outputs.is_empty()),
        "and nothing was set up after the refusal"
    );
}

// ── RTDE, over a socket ──────────────────────────────────────────────────────

#[test]
fn a_pause_is_read_past_the_data_still_streaming_to_its_own_reply() {
    let _one = one_at_a_time();
    let controller = Controller::start(Script::new());
    let mut session =
        Session::new(TcpStream::connect(("127.0.0.1", 30004)).expect("the scripted RTDE"));
    session.negotiate(2).expect("protocol 2");
    session.controller_version().expect("a version");
    session.setup_outputs(&OUTPUTS, 125.0).expect("outputs");
    session.setup_inputs(&INPUTS).expect("inputs");
    session.start().expect("start");
    thread::sleep(Duration::from_millis(80));
    session
        .pause()
        .expect("the pause's own reply comes after data packages, and is read past them");
    assert_eq!(
        controller.with(|s| s.pauses),
        1,
        "and the request reached the controller"
    );
    assert!(
        session.messages.iter().any(|m| m.contains("Safety Setup")),
        "the controller's unsolicited text was kept: {:?}",
        session.messages
    );
}

#[test]
fn a_package_longer_than_its_recipe_is_refused_and_so_is_a_sample_of_other_fields() {
    let types: Vec<String> = ["INT32", "DOUBLE"].map(String::from).to_vec();
    assert!(
        rtde::decode(&types, &[0; 13]).is_err(),
        "a package longer than its recipe is not read"
    );
    assert!(rtde::decode(&types, &[0; 11]).is_err(), "nor one shorter");
    assert!(rtde::decode(&types, &[0; 12]).is_ok());
    let six = || Value::Vec6(vec![0.0; 6]);
    let good = [
        six(),
        six(),
        six(),
        Value::I32(7),
        Value::I32(1),
        Value::U32(2),
    ];
    assert!(Sample::from_values(&good).is_ok());
    assert!(
        Sample::from_values(&good[..5]).is_err(),
        "a sample with a field missing is refused, not indexed past"
    );
    let mut mistyped = good.to_vec();
    mistyped[3] = Value::F64(7.0);
    assert!(
        Sample::from_values(&mistyped).is_err(),
        "and one whose mode is a double is another field"
    );
    let mut short_vector = good.to_vec();
    short_vector[0] = Value::Vec6(vec![0.0; 5]);
    assert!(
        Sample::from_values(&short_vector).is_err(),
        "and a joint vector of five"
    );
}

// ── what a sample becomes ────────────────────────────────────────────────────

#[test]
fn a_sample_is_reported_joint_for_joint_with_the_flags_its_statuses_raise() {
    let _one = one_at_a_time();
    for safety in 1..=9i32 {
        for mode in [7, 5] {
            let mut script = Script::new();
            script.safety_status = safety;
            script.robot_mode = mode;
            let (_controller, mut connection) = opened(script, 125.0);
            for estopped in [false, true] {
                let reading = connection.read_state(estopped, clock).expect("a reading");
                let at = format!("safety {safety}, robot_mode {mode}, latched {estopped}");
                assert_eq!(
                    reading.sample.q, Q0,
                    "{at}: positions are the controller's, joint for joint and sign for sign"
                );
                assert_eq!(
                    reading.sample.qd, QD0,
                    "{at}: and velocities each in their own slot"
                );
                assert_eq!(reading.flags & flags::ESTOP != 0, estopped || safety == 6 || safety == 7, "{at}: the emergency-stop flag is the latch or one of the two emergency statuses");
                assert_eq!(
                    reading.flags & flags::DEGRADED != 0,
                    safety != 1 || mode != 7,
                    "{at}: degraded exactly when the arm is not running normally"
                );
                assert_eq!(
                    reading.fault_code,
                    if safety == 1 { 0 } else { safety as u32 },
                    "{at}: the controller's own safety code is surfaced"
                );
                assert_eq!(
                    reading.flags & Homing::NotRequired.to_flags(),
                    Homing::NotRequired.to_flags(),
                    "{at}: absolute encoders, nothing to home"
                );
            }
        }
    }
}

/// The fault-vocabulary contract (`tools/fault-vocab/contract.py`, DRY-COVERAGE-PLAN.md 12.C.6), held to the same table `bridge.py` is held
/// to: `drivers/fault-vocabulary.json`, read here with the workspace's own JSON so the two drivers cannot be given different tables.
///
/// For every state the maker documents and every probe outside the table: the flags the class requires and never one it forbids, the fault
/// word, a command refused or taken as the class says, and the arm reading healthy again when the controller clears it. **It agrees with
/// whoever scripted the controller**, and so does the table — what it holds is what the driver does with a number, not what a UR does with a
/// collision.
#[test]
fn every_state_the_vocabulary_names_is_reported_as_its_class_and_an_unlisted_one_as_a_stop() {
    let _one = one_at_a_time();
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fault-vocabulary.json"
    ))
    .expect("drivers/fault-vocabulary.json is where the contract's table lives");
    let spec = skynet_json::Json::parse(&text).expect("the vocabulary file parses");
    let vocabulary = spec
        .get("drivers")
        .and_then(|drivers| drivers.get("ur_rtde"))
        .and_then(|entry| entry.get("vocabularies"))
        .and_then(|vocabularies| vocabularies.get("safety_status"))
        .expect("ur_rtde has a `safety_status` vocabulary");

    // (code, class, name): the listed ones, then the probes at the class an unlisted code escalates to.
    let mut cases: Vec<(i32, String, String)> = Vec::new();
    for (key, code) in vocabulary
        .get("codes")
        .and_then(|c| c.as_obj())
        .expect("codes")
    {
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
        cases.push((key.parse().expect("a numeric code"), class, name));
    }
    let escalation = vocabulary
        .get("unmapped")
        .and_then(|c| c.as_str())
        .expect("an escalation")
        .to_string();
    assert!(
        escalation == "fault" || escalation == "estop",
        "an unlisted code escalates to a fault or a stop, never to healthy"
    );
    for probe in vocabulary
        .get("probes")
        .and_then(|p| p.as_arr())
        .expect("probes")
    {
        let code = probe.as_f64().expect("a numeric probe") as i32;
        cases.push((code, escalation.clone(), format!("unlisted ({code})")));
    }
    assert!(
        cases.len() >= 12,
        "the table and its probes were read: {} cases",
        cases.len()
    );

    for (code, class, name) in cases {
        let mut script = Script::new();
        script.safety_status = code;
        let (controller, mut connection) = opened(script, 125.0);
        let at = format!("safety_status {code} {name} -> {class}");
        let reading = connection.read_state(false, clock).expect("a reading");
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
        assert_eq!(
            reading.fault_code,
            if class == "ok" { 0 } else { code as u32 },
            "{at}: the fault word is the maker's code, verbatim, and zero only when healthy"
        );
        let command = connection.write_command(&[0.1; 6], &[1; 6]);
        if refused {
            assert_eq!(command, status::EPERM, "{at}: a command is refused, because `SKR_OK` would report success the robot did not have");
        } else {
            assert_eq!(command, status::OK, "{at}: a command is taken");
        }
        // The controller clears it: a fault the driver holds after the controller has let go is F142's shape.
        controller.with(|s| s.safety_status = 1);
        let healed = connection.read_state(false, clock).expect("a reading");
        assert_eq!(
            healed.flags & (flags::ESTOP | flags::FAULT | flags::DEGRADED),
            0,
            "{at}: and healthy again, read {:#06x}",
            healed.flags
        );
        assert_eq!(
            connection.write_command(&[0.1; 6], &[1; 6]),
            status::OK,
            "{at}: and a command is taken again"
        );
    }
}

#[test]
fn the_sequence_advances_on_a_new_sample_and_is_held_when_the_controller_falls_silent() {
    let _one = one_at_a_time();
    let (controller, mut connection) = opened(Script::new(), 125.0);
    let first = connection.read_state(false, clock).expect("a reading").seq;
    let second = connection.read_state(false, clock).expect("a reading").seq;
    assert_eq!(
        second,
        first.wrapping_add(1),
        "each new sample advances the sequence by one (§24)"
    );
    drop(connection);
    drop(controller);

    // One hertz: a period is a second, so a stall is two whole seconds and a late sample has half a second either side.
    let (controller, mut connection) = opened(Script::new(), 1.0);
    let held = connection
        .read_state(false, clock)
        .expect("a fresh sample, a second on")
        .seq;
    controller.with(|s| s.publish = false);
    let started = Instant::now();
    let stalled = connection
        .read_state(false, clock)
        .expect("a stalled producer is reported, not refused");
    let waited = started.elapsed();
    assert_eq!(stalled.seq, held, "two whole periods of silence report the last sample with its sequence held: that is how a stall is told");
    assert_eq!(
        stalled.sample.q, Q0,
        "the held sample is the last one read, not zeros"
    );
    assert!(connection.stale_cycles >= 1, "and the stall is counted");
    assert!(
        waited >= Duration::from_millis(1900),
        "the wait was two periods, not less: {waited:?}"
    );

    let stale_before = connection.stale_cycles;
    let seq_before = stalled.seq;
    let late = {
        let shared = controller.shared.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1500));
            locked(&shared).burst = 1;
        })
    };
    let fresh = connection.read_state(false, clock).expect("a reading");
    late.join().unwrap();
    assert_eq!(
        fresh.seq,
        seq_before.wrapping_add(1),
        "a sample a period and a half late is still fresh: jitter is not a stalled producer (F171)"
    );
    assert_eq!(
        connection.stale_cycles, stale_before,
        "and is not counted as a stall"
    );
}

// ── through the vtable ───────────────────────────────────────────────────────

#[test]
fn the_state_the_driver_hands_out_is_the_samples_and_claims_no_torque_and_no_base() {
    let _one = one_at_a_time();
    let controller = Controller::start(Script::new());
    let (rc, ctx) = open_driver(r#"{"host": "127.0.0.1"}"#);
    assert_eq!(rc, status::OK);
    let state = read(ctx);
    assert_eq!(state.rc, status::OK);
    assert_eq!(as_f64(&state.q), Q0, "positions in order");
    assert_eq!(
        as_f64(&state.qd),
        QD0,
        "velocities in order, not the positions again"
    );
    assert!(
        state.effort.iter().all(|v| v.is_nan()),
        "torque is NaN: current is not torque and a zero would be a measurement (§24)"
    );
    assert!(
        state.base_pose.iter().all(|v| v.is_nan()),
        "a fixed base has no pose, which is NaN and not the origin"
    );
    assert_eq!(state.flags, Homing::NotRequired.to_flags());
    assert_eq!(state.fault_code, 0);
    assert_eq!(close(ctx), status::OK);
    assert_eq!(read(ctx).rc, status::EPERM, "a closed arm refuses");
    drop(controller);
}

#[test]
fn a_command_goes_out_as_six_setpoints_and_the_gate_open_and_a_refused_one_goes_nowhere() {
    let _one = one_at_a_time();
    let controller = Controller::start(Script::new());
    let (rc, ctx) = open_driver(r#"{"host": "127.0.0.1"}"#);
    assert_eq!(rc, status::OK);

    // A hold as the very first command holds where the arm *is*, and not at zero.
    assert_eq!(position(ctx, &[9.0; 6], &NONE), status::OK);
    controller.wait_for("the first hold", |s| !s.writes.is_empty());
    let first = controller.with(|s| s.writes[0].clone());
    assert_eq!(
        first.setpoint, Q0,
        "the first command, a hold, carries the pose the arm was read at, not a home pose"
    );
    assert_eq!(
        first.gate,
        control::CMD_SERVO,
        "with the gate open: a hold is a setpoint held, the program keeps servoing it"
    );
    assert_eq!(
        first.recipe, 2,
        "under the input recipe the controller gave"
    );

    assert_eq!(position(ctx, &TARGET, &ALL), status::OK);
    controller.wait_for("the command", |s| s.writes.len() >= 2);
    assert_eq!(
        controller.with(|s| s.writes[1].clone()),
        Written {
            recipe: 2,
            setpoint: as_f64(&TARGET),
            gate: control::CMD_SERVO
        },
        "six setpoints in order, the gate open"
    );

    // The mask: a masked joint keeps the setpoint it had.
    let other = [0.1875f32, -0.3125, 0.4375, -0.5625, 0.6875, -0.8125];
    assert_eq!(position(ctx, &other, &[1, 0, 1, 0, 1, 0]), status::OK);
    controller.wait_for("the masked command", |s| s.writes.len() >= 3);
    let masked = controller.with(|s| s.writes[2].setpoint);
    let want: Vec<f64> = (0..6)
        .map(|i| {
            if i % 2 == 0 {
                other[i] as f64
            } else {
                TARGET[i] as f64
            }
        })
        .collect();
    assert_eq!(
        masked.to_vec(),
        want,
        "the joints the mask names move and the others hold where they were told to be"
    );

    // Refusals: each goes nowhere, and stores nothing (F74).
    let sent = controller.with(|s| s.writes.len());
    let mut refused = other;
    refused[0] += 0.25;
    refused[1] = f32::NAN;
    assert_eq!(
        position(ctx, &refused, &ALL),
        status::EINVAL,
        "a NaN setpoint"
    );
    let mut infinite = other;
    infinite[5] = f32::INFINITY;
    assert_eq!(
        position(ctx, &infinite, &ALL),
        status::EINVAL,
        "an infinite one"
    );
    assert_eq!(
        write(ctx, Mode::Position as u8, &[0.0; 3], &[1; 3]),
        status::EINVAL,
        "the wrong joint count"
    );
    assert_eq!(
        write(ctx, Mode::Torque as u8, &TARGET, &ALL),
        status::EINVAL,
        "a torque command"
    );
    assert_eq!(
        write(ctx, Mode::Velocity as u8, &TARGET, &ALL),
        status::EINVAL,
        "a velocity command"
    );
    assert_eq!(
        controller.settled(|s| s.writes.len()),
        sent,
        "none of the refused commands reached the controller"
    );
    assert_eq!(position(ctx, &[9.0; 6], &NONE), status::OK);
    controller.wait_for("the hold after the refusals", |s| s.writes.len() > sent);
    assert_eq!(
        controller.with(|s| s.writes.last().unwrap().setpoint.to_vec()),
        want,
        "a hold after a command refused on its second joint holds where the arm was told to be, not at the refused command's first joint (F74)"
    );
    assert_eq!(close(ctx), status::OK);
    drop(controller);
}

#[test]
fn the_stop_holds_the_program_stops_it_latches_and_release_restarts_it() {
    let _one = one_at_a_time();
    let controller = Controller::start(Script::new());
    let (rc, ctx) = open_driver(r#"{"host": "127.0.0.1"}"#);
    assert_eq!(rc, status::OK);
    assert_eq!(position(ctx, &TARGET, &ALL), status::OK);
    controller.wait_for("the command", |s| s.writes.len() == 1);

    assert_eq!(stop(ctx), status::OK, "emergency_stop returns");
    controller.wait_for("the hold package", |s| s.writes.len() == 2);
    let hold = controller.with(|s| s.writes[1].clone());
    assert_eq!(
        hold.gate,
        control::CMD_HOLD,
        "the control program is told to hold: the gate is shut, so servoj stops being fed"
    );
    assert_eq!(hold.setpoint, as_f64(&TARGET), "and the package carries the setpoint the registers held, not zeros and not the pose at open");
    assert_eq!(hold.recipe, 2);
    controller.wait_for("the dashboard's stop", |s| {
        s.dashboard.iter().any(|c| c == "stop")
    });

    let stopped = read(ctx);
    assert_ne!(
        stopped.flags & flags::ESTOP,
        0,
        "the next state says the stop is asserted, though the controller's own status is normal"
    );
    let sent = controller.with(|s| s.writes.len());
    assert_eq!(
        position(ctx, &TARGET, &ALL),
        status::EPERM,
        "and a command after it is refused"
    );
    assert_eq!(
        controller.settled(|s| s.writes.len()),
        sent,
        "before it is written"
    );
    assert_eq!(stop(ctx), status::OK, "a second stop is absorbed");

    assert_eq!(release(ctx), status::OK, "release_stop returns");
    controller.wait_for("the program's restart", |s| {
        s.dashboard.iter().any(|c| c == "play")
    });
    assert_eq!(
        position(ctx, &TARGET, &ALL),
        status::OK,
        "the latch is cleared: commands are accepted again"
    );
    assert_eq!(
        read(ctx).flags & flags::ESTOP,
        0,
        "and the state no longer says a stop is asserted"
    );
    assert_eq!(close(ctx), status::OK);
    drop(controller);
}

#[test]
fn a_stop_says_ok_when_either_route_reached_the_controller_and_not_when_neither_did() {
    let _one = one_at_a_time();
    let controller = Controller::start(Script::new());
    let (rc, ctx) = open_driver(r#"{"host": "127.0.0.1"}"#);
    assert_eq!(rc, status::OK);

    // The dashboard goes away: the hold package still stops `servoj` being fed, and that is a stop.
    controller.with(|s| s.drop_dashboard = true);
    thread::sleep(Duration::from_millis(150));
    for attempt in 0..4 {
        assert_eq!(stop(ctx), status::OK, "attempt {attempt}: the hold reached the controller by the RTDE route, so the stop is reported as delivered");
        thread::sleep(Duration::from_millis(30));
    }

    // Then RTDE goes too: a stop that reached the controller by neither route must not say it did.
    controller.with(|s| s.drop_rtde = true);
    thread::sleep(Duration::from_millis(150));
    let mut last = status::OK;
    for _ in 0..20 {
        last = stop(ctx);
        if last != status::OK {
            break;
        }
        thread::sleep(Duration::from_millis(30));
    }
    assert_eq!(
        last,
        status::EIO,
        "a stop whose two routes are both gone is EIO, never a claimed stop that was not sent"
    );
    assert_eq!(close(ctx), status::OK);
    drop(controller);
}

#[test]
fn a_stop_does_not_outlive_its_session_and_a_session_that_is_replaced_is_let_go_of() {
    let _one = one_at_a_time();
    let controller = Controller::start(Script::new());
    let (rc, ctx) = open_driver(r#"{"host": "127.0.0.1"}"#);
    assert_eq!(rc, status::OK);
    assert_eq!(position(ctx, &TARGET, &ALL), status::OK);
    assert_eq!(stop(ctx), status::OK);
    assert_eq!(position(ctx, &TARGET, &ALL), status::EPERM);
    controller.wait_for("the stop's hold package and the dashboard's stop", |s| {
        s.writes.last().map(|w| w.gate) == Some(control::CMD_HOLD)
            && s.dashboard.iter().any(|c| c == "stop")
    });

    // A close leaves the program holding, the exchange paused, and the stop path with nothing to write to.
    let writes_before = controller.with(|s| s.writes.len());
    let stops_before = controller.with(|s| s.dashboard.iter().filter(|c| *c == "stop").count());
    let pauses_before = controller.with(|s| s.pauses);
    assert_eq!(close(ctx), status::OK);
    controller.wait_for("the close's hold package", |s| {
        s.writes.len() > writes_before
    });
    let closing = controller.with(|s| s.writes.last().unwrap().clone());
    assert_eq!(
        (closing.gate, closing.setpoint),
        (control::CMD_HOLD, as_f64(&TARGET)),
        "close leaves the program holding the setpoint it had"
    );
    controller.wait_for("the close's pause", |s| s.pauses > pauses_before);
    let writes_after = controller.with(|s| s.writes.len());
    assert_eq!(
        stop(ctx),
        status::OK,
        "a stop after close has nothing to stop and says so"
    );
    assert_eq!(
        controller.settled(|s| (
            s.writes.len(),
            s.dashboard.iter().filter(|c| *c == "stop").count()
        )),
        (writes_after, stops_before),
        "and writes nothing to the sockets of a session that is gone"
    );

    // F76: the latch belonged to the session it was asserted in.
    let (rc, ctx) = open_driver(r#"{"host": "127.0.0.1"}"#);
    assert_eq!(
        rc,
        status::OK,
        "the same driver opens again at once after a stop and a close"
    );
    assert_eq!(
        position(ctx, &TARGET, &ALL),
        status::OK,
        "and commands: the latch was not carried over (F76)"
    );

    // A second open is a new session: the first is paused and closed, not stacked.
    let paused = controller.with(|s| s.pauses);
    let (rc, ctx) = open_driver(r#"{"host": "127.0.0.1"}"#);
    assert_eq!(rc, status::OK);
    controller.wait_for("the replaced session's pause", |s| s.pauses > paused);
    assert_eq!(close(ctx), status::OK);
    drop(controller);
}

#[test]
fn a_description_needs_room_for_its_terminator_and_after_close_is_empty() {
    let _one = one_at_a_time();
    let controller = Controller::start(Script::new());
    let describe = |ctx: *mut Ctx, len: usize| -> (i32, String) {
        let mut buffer = vec![0u8; len.max(1)];
        // SAFETY: a live context and a buffer of the length passed.
        let n = unsafe {
            (driver().describe.unwrap())(ctx, buffer.as_mut_ptr() as *mut std::ffi::c_char, len)
        };
        let text = if n > 0 {
            String::from_utf8_lossy(&buffer[..n as usize]).to_string()
        } else {
            String::new()
        };
        (n, text)
    };
    let (rc, ctx) = open_driver(r#"{"host": "127.0.0.1"}"#);
    assert_eq!(rc, status::OK);
    let (length, text) = describe(ctx, 1 << 14);
    assert!(
        length > 100 && text.starts_with('{'),
        "a descriptor: {length} bytes"
    );
    assert_eq!(describe(ctx, length as usize).0, status::EINVAL, "a buffer exactly as long as the text has no room for the terminator, and is refused rather than truncated");
    assert_eq!(
        describe(ctx, length as usize + 1),
        (length, text),
        "one byte more holds it"
    );
    assert_eq!(close(ctx), status::OK);
    assert_eq!(
        describe(ctx, 64),
        (2, "{}".to_string()),
        "a driver with no arm describes nothing, not the arm it had"
    );
    drop(controller);
}

// ── a link that has gone (DRY-COVERAGE-PLAN.md 12.C.4) ───────────────────────

#[test]
fn ports_move_where_the_driver_connects_for_a_controller_behind_a_forward_or_a_relay() {
    let _one = one_at_a_time();
    let controller = Controller::start_on(Script::new(), (41999, 41002, 41004));
    let config = Config::parse(
        r#"{"host": "127.0.0.1", "model": "UR5e", "ports": {"dashboard": 41999, "script": 41002, "rtde": 41004}}"#,
    )
    .expect("a configuration");
    let connection = Connection::open(&config, &quietly).unwrap_or_else(|error| {
        panic!(
            "a controller on the moved ports was not reached: {}",
            error.message()
        )
    });
    controller.wait_for(
        "the control program on the secondary interface's own port",
        |s| s.program == control::SERVO_PROGRAM.as_bytes(),
    );
    let seen = controller.with(|s| (s.dashboard.len(), s.rtde_sessions));
    assert!(
        seen.0 >= 2 && seen.1 == 1,
        "the dashboard and RTDE were each reached on the port `ports` named: {seen:?}"
    );
    drop(connection);
}

#[test]
fn the_sockets_the_stop_writes_on_are_bounded_so_a_stop_cannot_wait_for_a_full_buffer() {
    let _one = one_at_a_time();
    let (_controller, connection) = opened(Script::new(), 125.0);
    let (rtde, _dashboard, _recipe) = connection.stop_handles().expect("the stop path's sockets");
    let bound = rtde.write_timeout().expect("a readable option");
    assert!(
        matches!(bound, Some(timeout) if timeout <= Duration::from_millis(100)),
        "the RTDE socket, which the stop path writes its hold package on a duplicate of, gives a write no more than a tenth of a second (it is {bound:?}, and the session's own was five seconds)"
    );
    drop(connection);
}

#[test]
fn a_close_does_not_wait_out_the_sessions_five_seconds_for_a_controller_that_has_gone_quiet() {
    let _one = one_at_a_time();
    let (controller, connection) = opened(Script::new(), 125.0);
    controller.with(|s| {
        s.publish = false;
        s.answer_pause = false;
    });
    thread::sleep(Duration::from_millis(100));
    let began = Instant::now();
    connection.close();
    let took = began.elapsed();
    assert!(
        took < Duration::from_millis(2000),
        "close learned that the controller had gone in {took:?}, and not in the session's five seconds"
    );
}

#[test]
fn a_stream_the_controller_closes_is_an_io_error_and_not_a_protocol_one() {
    let _one = one_at_a_time();
    let (controller, mut connection) = opened(Script::new(), 125.0);
    controller.with(|s| s.drop_rtde = true);
    thread::sleep(Duration::from_millis(150));
    for attempt in 0..5 {
        match connection.read_state(false, clock) {
            Err(code) => assert_eq!(
                code,
                status::EIO,
                "attempt {attempt}: a stream the controller closed is a link that has gone (EIO), and was reported as {code}"
            ),
            Ok(_) => panic!("attempt {attempt}: a read of a closed stream succeeded"),
        }
    }
    drop(connection);
}

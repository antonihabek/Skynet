//! A Universal Robots arm over RTDE, as one body — a Rust port of
//! `drivers/ur_rtde/skynet_ur/bridge.py`, which is the executable specification and was measured
//! against URSim, UR's own controller software.
//!
//! Three sockets, because one is not enough: the dashboard (29999) powers the arm and releases the
//! brakes and reads the safety status; the secondary interface (30002) takes the control program
//! once; RTDE (30004) carries the body at the negotiated rate and the setpoint registers.
//!
//! It refuses what the Python driver refuses: a configuration with no `host` (a guessed default
//! would one day connect to somebody else's robot); a robot whose safety status is not `NORMAL`
//! (`unlock protective stop` exists and is never sent — a protective stop is the machine saying a
//! person or a collision happened); a recipe the controller answered `IN_USE` for; any mode but
//! position. It does not claim joint torque — `actual_current` is amps, the constant that makes
//! it newton-metres is not published, and §5.2 forbids inventing one — so `effort_nm` is NaN.
//!
//! # Where the port knowingly differs from the Python driver
//!
//! * A configuration value of the wrong JSON type is refused rather than read for its truthiness
//!   or its `str()`.
//! * The setpoint registers must come back typed as six doubles and an integer, the only layout
//!   the stop path's stack-built package writes; Python encodes whatever types came back.
//! * `pause` at close reads past the data packages still streaming to its own reply. Python's went
//!   through `transact`, which refuses the first data package, and `close` swallowed the error.
//! * Only the newest data package of a drain is decoded; Python decodes every one it passes.
//! * The stop writes the hold package and `stop` on sockets duplicated at open and waits for no
//!   reply (the RHAL: *"Never blocks"*). Python's dashboard `stop` waited for the reply line.

use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use skynet_json::Json;
use skynet_rhal::{flags, status, Homing};

use crate::control::{CMD_HOLD, CMD_SERVO, SERVO_PERIOD_S, SERVO_PROGRAM};
use crate::dashboard::{Dashboard, DashboardError};
use crate::rtde::{RtdeError, Session, Value};

/// The body, read every cycle.
pub const OUTPUTS: [&str; 6] = [
    "actual_q",
    "actual_qd",
    "actual_current",
    "robot_mode",
    "safety_status",
    "runtime_state",
];

/// The setpoint, written every cycle: six doubles and the gate the control program reads.
pub const INPUTS: [&str; 7] = [
    "input_double_register_0",
    "input_double_register_1",
    "input_double_register_2",
    "input_double_register_3",
    "input_double_register_4",
    "input_double_register_5",
    "input_int_register_0",
];

/// The vendor's joint names, in the order `actual_q` reports them.
pub const JOINT_NAMES: [&str; 6] = [
    "shoulder_pan",
    "shoulder_lift",
    "elbow",
    "wrist_1",
    "wrist_2",
    "wrist_3",
];

/// §5.1 roles: a six-axis arm with a spherical wrist, which is what every UR is.
pub const JOINT_ROLES: [&str; 6] = [
    "right_arm.shoulder.yaw",
    "right_arm.shoulder.pitch",
    "right_arm.elbow.flex",
    "right_arm.wrist.pitch",
    "right_arm.wrist.yaw",
    "right_arm.wrist.roll",
];

/// Joint ratings per e-Series model, base to wrist 3: maximum speed in degrees per second, maximum
/// torque in newton-metres — `bridge.py`'s `MODEL_RATINGS`, whose comment cites each model's tech
/// sheet (axis-movement table) and UR's support article "Max. joint torques CB3 and e-Series".
/// The controller never says which model it is, so the operator declares it.
pub const MODEL_RATINGS: [(&str, [u32; 6], [u32; 6]); 6] = [
    (
        "UR3E",
        [180, 180, 180, 360, 360, 360],
        [54, 54, 28, 9, 9, 9],
    ),
    (
        "UR5E",
        [180, 180, 180, 180, 180, 180],
        [150, 150, 150, 28, 28, 28],
    ),
    (
        "UR7E",
        [180, 180, 180, 180, 180, 180],
        [150, 150, 150, 28, 28, 28],
    ),
    (
        "UR10E",
        [120, 120, 180, 180, 180, 180],
        [330, 330, 150, 54, 54, 54],
    ),
    (
        "UR12E",
        [120, 120, 180, 180, 180, 180],
        [330, 330, 150, 54, 54, 54],
    ),
    (
        "UR16E",
        [120, 120, 180, 180, 180, 180],
        [330, 330, 150, 54, 54, 54],
    ),
];

/// The watchdog window, milliseconds: ten `servoj` periods, in `bridge.py`'s arithmetic.
pub const WATCHDOG_MS: f64 = 10.0 * SERVO_PERIOD_S * 1000.0;

/// `robot_mode`'s running value.
const MODE_RUNNING: i64 = 7;
/// `safety_status`'s normal value; everything above it is a person's problem.
const SAFETY_NORMAL: i64 = 1;
/// The two emergency-stop statuses, which set §24's `ESTOP` flag.
const SAFETY_EMERGENCY: [i64; 2] = [6, 7];

const DASHBOARD_PORT: u16 = 29999;
const SECONDARY_PORT: u16 = 30002;
const RTDE_PORT: u16 = 30004;
const SOCKET_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the bring-up waits for each mode, `dashboard.py`'s thirty seconds.
const BRING_UP_TIMEOUT: Duration = Duration::from_secs(30);
/// How often `read_state` looks again while it waits for a new sample: `bridge.py`'s 0.5 ms.
const SAMPLE_POLL: Duration = Duration::from_micros(500);

/// How long bring-up waits for the RTDE registers a closed session held — `bridge.py`'s
/// `REGISTER_RELEASE_S`, for its reason: URSim 5.26 frees a cleanly closed session's about 60 ms
/// after its socket goes, and this port, which has no interpreter to start first, was answered
/// `IN_USE` by the second session `skynet driver check` opens at once (F81).
const REGISTER_RELEASE: Duration = Duration::from_secs(2);

/// How often bring-up asks again while it waits for them.
const REGISTER_POLL: Duration = Duration::from_millis(50);

/// `open`'s configuration, resolved and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The controller. No default, deliberately.
    pub host: String,
    /// Whether the body is simulated. Defaults true (§35.3's safe direction).
    pub simulator: bool,
    /// The declared e-Series model, canonicalised, if it is one with transcribed ratings.
    pub model: Option<&'static str>,
    /// What the operator wrote for the model, for the message when it is not transcribed.
    pub model_given: Option<String>,
    /// The output frequency RTDE is asked for.
    pub frequency_hz: f64,
    /// How long `open` waits for the first data package.
    pub startup_timeout: Duration,
}

/// Why a body did not come up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// A malformed configuration: `SKR_EINVAL`.
    Malformed(String),
    /// The configuration or the controller refused the body: `SKR_ENODEV`.
    Refused(String),
    /// A socket failed: `SKR_EIO`.
    Io(String),
}

impl OpenError {
    /// The RHAL status this returns.
    pub fn status(&self) -> i32 {
        match self {
            OpenError::Malformed(_) => status::EINVAL,
            OpenError::Refused(_) => status::ENODEV,
            OpenError::Io(_) => status::EIO,
        }
    }

    /// The sentence for stderr.
    pub fn message(&self) -> &str {
        match self {
            OpenError::Malformed(text) | OpenError::Refused(text) | OpenError::Io(text) => text,
        }
    }
}

fn malformed(text: impl Into<String>) -> OpenError {
    OpenError::Malformed(text.into())
}

impl From<RtdeError> for OpenError {
    fn from(error: RtdeError) -> OpenError {
        match error {
            RtdeError::Refused(text) | RtdeError::InUse(text) => OpenError::Refused(text),
            RtdeError::Io(error) => OpenError::Io(error.to_string()),
        }
    }
}

impl From<DashboardError> for OpenError {
    fn from(error: DashboardError) -> OpenError {
        match error {
            DashboardError::Refused(text) => OpenError::Refused(text),
            DashboardError::Io(error) => OpenError::Io(error.to_string()),
        }
    }
}

impl Config {
    /// Parse and check the configuration `open` is handed.
    pub fn parse(text: &str) -> Result<Config, OpenError> {
        let settings = if text.trim().is_empty() {
            Json::obj()
        } else {
            Json::parse(text)
                .map_err(|error| malformed(format!("the configuration is not JSON: {error:?}")))?
        };
        if settings.as_obj().is_none() {
            return Err(malformed(
                "the configuration is JSON and not an object; give {\"host\": ...}",
            ));
        }
        let host = match settings.get_present("host") {
            None => String::new(),
            Some(value) => value
                .as_str()
                .ok_or_else(|| malformed("`host` is an address, as a string."))?
                .to_string(),
        };
        if host.is_empty() {
            return Err(OpenError::Refused(
                "no `host` in the configuration. There is no default: a driver that guessed \
                 192.168.1.100 would eventually connect to somebody else's robot. Give \
                 `{\"host\": \"192.168.56.101\"}` — or `127.0.0.1` for a URSim container."
                    .into(),
            ));
        }
        let simulator = match settings.get("simulator") {
            None => true,
            Some(value) => value.as_bool().ok_or_else(|| {
                malformed(
                    "`simulator` is true or false. Anything else is refused rather than read for \
                     its truthiness, because the answer caps the body's tier (§35.3).",
                )
            })?,
        };
        let model_given = match settings.get_present("model") {
            None => None,
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| malformed("`model` names the arm, as a string: \"UR5e\"."))?
                    .to_string(),
            ),
        };
        // Case and whitespace only, never a guess: "ur5e" is the UR5e, and "UR5" is a CB3 arm
        // whose speeds differ.
        let model = model_given.as_deref().and_then(|given| {
            let canonical = given.trim().to_uppercase();
            MODEL_RATINGS
                .iter()
                .find(|(name, _, _)| *name == canonical)
                .map(|(name, _, _)| *name)
        });
        let frequency_hz = match settings.get("frequency_hz") {
            None => 125.0,
            Some(value) => value
                .as_f64()
                .ok_or_else(|| malformed("`frequency_hz` is a number of hertz."))?,
        };
        if !(1.0..=500.0).contains(&frequency_hz) {
            return Err(malformed(format!(
                "`frequency_hz` of {frequency_hz} is outside what RTDE will negotiate (1 to 500)."
            )));
        }
        let startup_timeout_s = match settings.get("startup_timeout_s") {
            None => 10.0,
            Some(value) => value
                .as_f64()
                .filter(|seconds| *seconds > 0.0 && seconds.is_finite())
                .ok_or_else(|| malformed("`startup_timeout_s` is a positive number of seconds."))?,
        };
        Ok(Config {
            host,
            simulator,
            model,
            model_given,
            frequency_hz,
            startup_timeout: Duration::from_secs_f64(startup_timeout_s),
        })
    }

    /// What `open` says about the model, as `bridge.py` says it: nothing when a transcribed model
    /// was declared, otherwise why the envelope is null.
    pub fn model_note(&self) -> Option<String> {
        match (&self.model_given, self.model) {
            (_, Some(_)) => None,
            (Some(given), None) => {
                let mut known: Vec<&str> = MODEL_RATINGS.iter().map(|(name, _, _)| *name).collect();
                known.sort_unstable();
                Some(format!(
                    "model \"{given}\" has no transcribed ratings (this driver knows {}); the \
                     envelope is null. The arm still runs — position control is the same servoj \
                     on every model — but nothing here is clamped to its ratings.",
                    known.join(", ")
                ))
            }
            (None, None) => Some(
                "no `model` in the configuration, so the envelope is null. Declare the arm — \
                 `{\"host\": ..., \"model\": \"UR5e\"}` — and the descriptor carries its \
                 published ratings with the citation beside them."
                    .to_string(),
            ),
        }
    }
}

/// One output package, read by name.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// `actual_q`, radians.
    pub q: [f64; 6],
    /// `actual_qd`, rad/s.
    pub qd: [f64; 6],
    /// `robot_mode`.
    pub robot_mode: i64,
    /// `safety_status`.
    pub safety_status: i64,
}

impl Sample {
    /// The fields this driver reads, out of a package decoded under [`OUTPUTS`]. A controller
    /// that typed one of them otherwise is refused rather than read.
    pub fn from_values(values: &[Value]) -> Result<Sample, RtdeError> {
        let wrong = || {
            RtdeError::Refused(
                "the output recipe came back typed other than VECTOR6D for the joints and INT32 \
                 for the modes, and a field read under the wrong type is another field."
                    .into(),
            )
        };
        let six = |value: &Value| match value {
            Value::Vec6(items) if items.len() == 6 => {
                let mut out = [0.0; 6];
                out.copy_from_slice(items);
                Ok(out)
            }
            _ => Err(wrong()),
        };
        let int = |value: &Value| match value {
            Value::I32(v) => Ok(*v as i64),
            _ => Err(wrong()),
        };
        if values.len() != OUTPUTS.len() {
            return Err(wrong());
        }
        Ok(Sample {
            q: six(&values[0])?,
            qd: six(&values[1])?,
            robot_mode: int(&values[3])?,
            safety_status: int(&values[4])?,
        })
    }
}

/// What one `read_state` found, for the crate root to copy out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading {
    /// When the sample was taken off the stream, on §35.1's clock.
    pub stamp_ns: u64,
    /// The sequence number — held when no new sample arrived.
    pub seq: u32,
    /// §24's flags.
    pub flags: u16,
    /// The safety status when it is not normal, else 0.
    pub fault_code: u32,
    /// The sample reported.
    pub sample: Sample,
}

/// One arm over its three sockets.
pub struct Connection {
    rtde: Session<TcpStream>,
    dashboard: Dashboard,
    _script: TcpStream,
    setpoint: [f64; 6],
    last: Option<Sample>,
    seq: u32,
    period: Duration,
    descriptor: String,
    /// Cycles on which nothing new had arrived — the number that says whether this machine keeps
    /// up with the controller's stream.
    pub stale_cycles: u64,
}

fn resolve(host: &str, port: u16) -> Result<SocketAddr, OpenError> {
    (host, port)
        .to_socket_addrs()
        .map_err(|error| OpenError::Io(format!("{host}: {error}")))?
        .next()
        .ok_or_else(|| OpenError::Io(format!("{host} resolves to no address")))
}

fn connect(host: &str, port: u16) -> Result<TcpStream, OpenError> {
    let stream = TcpStream::connect_timeout(&resolve(host, port)?, SOCKET_TIMEOUT)
        .map_err(|error| OpenError::Io(format!("{host}:{port}: {error}")))?;
    stream
        .set_read_timeout(Some(SOCKET_TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(SOCKET_TIMEOUT)))
        .map_err(|error| OpenError::Io(format!("{host}:{port}: {error}")))?;
    // Nagle off: a setpoint that waits for a full segment arrives late.
    stream
        .set_nodelay(true)
        .map_err(|error| OpenError::Io(format!("{host}:{port}: {error}")))?;
    Ok(stream)
}

impl Connection {
    /// Bring the arm up, in `bridge.py`'s order: the safety status first, then power and brakes,
    /// then RTDE, then the control program, then the first sample. `say` carries what the Python
    /// driver prints on the way.
    pub fn open(config: &Config, say: &dyn Fn(&str)) -> Result<Connection, OpenError> {
        let mut dashboard = Dashboard::connect(connect(&config.host, DASHBOARD_PORT)?)
            .map_err(|error| OpenError::Io(format!("{}: {error}", config.host)))?;
        let safety = dashboard
            .safety_status()
            .map_err(|error| OpenError::Io(error.to_string()))?;
        if safety != "NORMAL" {
            return Err(OpenError::Refused(format!(
                "this robot's safety status is `{safety}`. This driver does not clear it: a \
                 protective stop is the machine saying a person or a collision happened, and a \
                 driver that unlocked it on the way up would erase the one signal that says so. \
                 Clear it at the machine."
            )));
        }
        dashboard.bring_up(BRING_UP_TIMEOUT)?;

        let mut rtde = Self::exchange(config, say)?;
        rtde.start()?;

        // The control program, once, on the secondary interface — where a program goes.
        let mut script = connect(&config.host, SECONDARY_PORT)?;
        std::io::Write::write_all(&mut script, SERVO_PROGRAM.as_bytes())
            .map_err(|error| OpenError::Io(error.to_string()))?;

        let first = Self::await_sample(&mut rtde, config.startup_timeout)?;
        Ok(Connection {
            rtde,
            dashboard,
            _script: script,
            setpoint: first.q,
            last: Some(first),
            seq: 0,
            period: Duration::from_secs_f64(1.0 / config.frequency_hz),
            descriptor: describe(config.model, config.simulator)
                .map_err(|error| OpenError::Refused(format!("{error:?}")))?,
            stale_cycles: 0,
        })
    }

    /// An RTDE session with both recipes set, waiting out registers a closed session held —
    /// `bridge.py`'s `_exchange`. `IN_USE` is the one refusal retried, and only until
    /// [`REGISTER_RELEASE`]: a session this process closed a moment ago frees its registers in tens
    /// of milliseconds, while a variable this controller does not have, or another client that
    /// keeps them, answers `IN_USE` for the whole window and is refused with the sentence it always
    /// was. Each try is a new connection; the refused one is dropped, which closes it.
    fn exchange(config: &Config, say: &dyn Fn(&str)) -> Result<Session<TcpStream>, OpenError> {
        let deadline = Instant::now() + REGISTER_RELEASE;
        loop {
            let mut rtde = Session::new(connect(&config.host, RTDE_PORT)?);
            rtde.negotiate(2)?;
            let version = rtde.controller_version()?;
            rtde.setup_outputs(&OUTPUTS, config.frequency_hz)?;
            match rtde.setup_inputs(&INPUTS) {
                Ok(_) => {
                    say(&format!(
                        "URControl {}.{}.{}.{} on {}",
                        version[0], version[1], version[2], version[3], config.host
                    ));
                    return Ok(rtde);
                }
                Err(RtdeError::InUse(_)) if Instant::now() < deadline => {
                    std::thread::sleep(REGISTER_POLL);
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn await_sample(rtde: &mut Session<TcpStream>, timeout: Duration) -> Result<Sample, OpenError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Some(values) = rtde.newest()? {
                return Ok(Sample::from_values(&values)?);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(OpenError::Refused(format!(
            "no data package arrived within {:.0}s of starting the exchange. The controller \
             says: {}",
            timeout.as_secs_f64(),
            rtde.messages
                .last()
                .map(String::as_str)
                .unwrap_or("nothing")
        )))
    }

    /// The RTDE socket, the dashboard socket and the input recipe id — what the stop path writes
    /// to without the connection's lock.
    pub fn stop_handles(&self) -> std::io::Result<(TcpStream, TcpStream, u8)> {
        let recipe = self
            .rtde
            .input
            .as_ref()
            .map(|recipe| recipe.identifier)
            .ok_or_else(|| std::io::Error::other("no input recipe"))?;
        Ok((
            self.rtde.stream().try_clone()?,
            self.dashboard.stop_socket()?,
            recipe,
        ))
    }

    /// The setpoint the registers hold now.
    pub fn setpoint(&self) -> [f64; 6] {
        self.setpoint
    }

    /// The embodiment descriptor.
    pub fn describe(&self) -> &str {
        &self.descriptor
    }

    /// Hold, pause and let the sockets go. Silent: closing is where a dying process lands.
    pub fn close(mut self) {
        // Each step is attempted whatever the one before did, and a failure is uninteresting here
        // for the reason `bridge.py`'s `except (RtdeError, OSError): pass` gives: the arm is being
        // let go of, and a controller that has already gone needs nothing more.
        let _held = self.rtde.write_setpoint(&self.setpoint, CMD_HOLD);
        let _paused = self.rtde.pause();
    }

    /// Wait for a **new** sample, bounded by the controller's own publication period: RTDE
    /// publishes on its own clock, so two reads microseconds apart genuinely have one sample
    /// between them, and returning the old one at once reads to §15 as a stalled producer. Two
    /// whole periods with nothing new is a stalled producer for real: the previous sample, with its
    /// sequence held. Two and not one, as `bridge.py` explains (DRY-COVERAGE-PLAN.md F152): one
    /// period turned a sample published a moment late into an invented stall.
    pub fn read_state(&mut self, estopped: bool, clock: fn() -> u64) -> Result<Reading, i32> {
        let deadline = Instant::now() + 2 * self.period;
        let mut sample = self.newest()?;
        while sample.is_none() && Instant::now() < deadline {
            std::thread::sleep(SAMPLE_POLL);
            sample = self.newest()?;
        }
        let sample = match sample {
            Some(sample) => {
                self.last = Some(sample);
                self.seq = self.seq.wrapping_add(1);
                sample
            }
            None => {
                self.stale_cycles += 1;
                match self.last {
                    Some(last) => last,
                    None => return Err(status::ETIMEDOUT),
                }
            }
        };
        let stamp_ns = clock();
        // A UR has absolute encoders and is mastered at the factory: nothing to home.
        let mut flags = Homing::NotRequired.to_flags();
        let mut fault_code = 0;
        if estopped || SAFETY_EMERGENCY.contains(&sample.safety_status) {
            flags |= flags::ESTOP;
        }
        if sample.safety_status != SAFETY_NORMAL {
            flags |= flags::DEGRADED;
            fault_code = sample.safety_status.clamp(0, u32::MAX as i64) as u32;
        }
        if sample.robot_mode != MODE_RUNNING {
            flags |= flags::DEGRADED;
        }
        Ok(Reading {
            stamp_ns,
            seq: self.seq,
            flags,
            fault_code,
            sample,
        })
    }

    fn newest(&mut self) -> Result<Option<Sample>, i32> {
        match self.rtde.newest() {
            Ok(None) => Ok(None),
            Ok(Some(values)) => Sample::from_values(&values).map(Some).map_err(|error| {
                eprintln!("skynet-ur: {error}");
                status::EPROTO
            }),
            Err(RtdeError::Refused(text) | RtdeError::InUse(text)) => {
                eprintln!("skynet-ur: {text}");
                Err(status::EPROTO)
            }
            Err(RtdeError::Io(error)) => {
                eprintln!("skynet-ur: the RTDE stream failed: {error}");
                Err(status::EIO)
            }
        }
    }

    /// Check the whole command, store it, and write it with the gate open. Nothing is stored
    /// unless all of it passes (F74): a masked joint holds the setpoint it had, not a value from a
    /// command that was refused.
    pub fn write_command(&mut self, values: &[f32; 6], mask: &[u8; 6]) -> i32 {
        let mut setpoint = self.setpoint;
        for joint in 0..6 {
            if mask[joint] == 0 {
                continue; // hold: the previous setpoint stands, which is what the mask means
            }
            let value = values[joint] as f64;
            if !value.is_finite() {
                return status::EINVAL;
            }
            setpoint[joint] = value;
        }
        self.setpoint = setpoint;
        match self.rtde.write_setpoint(&self.setpoint, CMD_SERVO) {
            Ok(()) => status::OK,
            Err(error) => {
                eprintln!("skynet-ur: the setpoint did not reach the controller: {error}");
                status::EIO
            }
        }
    }
}

fn strings(values: &[&str]) -> Json {
    Json::Arr(values.iter().map(|value| Json::from(*value)).collect())
}

/// `bridge.py`'s `_build_descriptor`, as a value in the workspace's canonical form — the form the
/// embodiment digest is taken over. `tests/vectors.rs` holds it to the Python descriptor for every
/// model, simulated and not, by parsed value.
pub fn describe(model: Option<&str>, simulator: bool) -> Result<String, skynet_json::JsonError> {
    let ratings = model.and_then(|model| {
        MODEL_RATINGS
            .iter()
            .find(|(name, _, _)| *name == model)
            .map(|(_, speeds, torques)| (speeds, torques))
    });
    let mut joints = Json::arr();
    for (index, (name, role)) in JOINT_NAMES.iter().zip(JOINT_ROLES).enumerate() {
        let mut limits = Json::obj();
        // Every UR joint is continuous over ±2π; the software limits live in the installation and
        // are not published over RTDE, so the mechanical range is declared and nothing narrower.
        limits
            .set("position_min_rad", Json::from(-2.0 * std::f64::consts::PI))
            .set("position_max_rad", Json::from(2.0 * std::f64::consts::PI));
        if let Some((speeds, torques)) = ratings {
            limits
                .set(
                    "velocity_max_rad_s",
                    Json::from((speeds[index] as f64).to_radians()),
                )
                .set("effort_max_nm", Json::from(torques[index] as f64));
        }
        let mut joint = Json::obj();
        joint
            .set("name", Json::from(*name))
            .set("role", Json::from(role))
            .set("type", Json::from("continuous"))
            .set("limits", limits)
            .set("control_modes", strings(&["position"]));
        joints.push(joint);
    }
    let envelope = match (ratings, model) {
        (Some((speeds, torques)), Some(model)) => {
            let slowest = speeds.iter().copied().min().unwrap_or(0);
            let weakest = torques.iter().copied().min().unwrap_or(0);
            let mut envelope = Json::obj();
            envelope
                .set(
                    "joint_velocity_max_rad_s",
                    Json::from((slowest as f64).to_radians()),
                )
                .set("joint_torque_max_nm", Json::from(weakest as f64))
                .set("watchdog_ms", Json::from(WATCHDOG_MS))
                .set(
                    "source",
                    Json::from(format!(
                        "Universal Robots {model} tech sheet, axis-movement table \
                         ({model}_techsheet_en.pdf); joint torques from UR support article 'Max. \
                         joint torques CB3 and e-Series'; watchdog ten servoj periods per driver \
                         design"
                    )),
                )
                // Transcribed from documentation, stated rather than defaulted (§5.3).
                .set("basis", Json::from("declared"));
            envelope
        }
        _ => Json::Null,
    };
    let mut control = Json::obj();
    control
        .set("modes", strings(&["position"]))
        // The controller's own rate: `servoj` covers 8 ms per statement.
        .set(
            "rate_hz_max",
            Json::from((1.0 / SERVO_PERIOD_S).round() as u32),
        )
        .set("rate_hz_min", Json::from(1u32));
    let mut descriptor = Json::obj();
    descriptor
        .set("schema_version", Json::from(1u32))
        .set("robot_slug", Json::from("universal-robots-arm"))
        .set("digest", Json::Null)
        .set("verified", Json::from(false))
        .set("simulator", Json::from(simulator))
        .set("base", Json::from("fixed"))
        .set("dof", Json::from(6u32))
        .set("joints", joints)
        .set("end_effectors", Json::arr())
        .set("sensors", Json::arr())
        .set("control", control)
        .set("envelope", envelope)
        // The software stop the driver has: hold, stop the program, latch. The rated stop is the
        // emergency circuit, which no frame on any of these sockets reaches.
        .set("capabilities", strings(&["safety.estop"]))
        .set("extensions", Json::arr())
        .set("solvers", Json::arr());
    descriptor.to_canonical()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_needs_a_host_and_refuses_what_it_cannot_read() {
        assert!(matches!(Config::parse("{}"), Err(OpenError::Refused(_))));
        assert!(matches!(
            Config::parse("not json"),
            Err(OpenError::Malformed(_))
        ));
        for bad in [
            r#"{"host": 5}"#,
            r#"{"host": "h", "simulator": "false"}"#,
            r#"{"host": "h", "model": 5}"#,
            r#"{"host": "h", "frequency_hz": 0.5}"#,
            r#"{"host": "h", "frequency_hz": 501}"#,
            r#"{"host": "h", "startup_timeout_s": 0}"#,
        ] {
            assert!(
                matches!(Config::parse(bad), Err(OpenError::Malformed(_))),
                "{bad} is refused"
            );
        }
        let config = Config::parse(r#"{"host": "10.0.0.2", "model": " ur5e "}"#).unwrap();
        assert_eq!(config.model, Some("UR5E"), "case and whitespace only");
        assert!(config.simulator);
        assert_eq!(config.model_note(), None);
        let config = Config::parse(r#"{"host": "10.0.0.2", "model": "UR5"}"#).unwrap();
        assert_eq!(config.model, None, "a CB3 arm is not an e-Series one");
        assert!(config
            .model_note()
            .unwrap()
            .contains("UR10E, UR12E, UR16E, UR3E"));
    }

    #[test]
    fn the_watchdog_is_ten_servo_periods() {
        assert_eq!(WATCHDOG_MS, 80.0);
    }
}

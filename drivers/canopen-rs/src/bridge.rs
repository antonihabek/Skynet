//! CiA 402 axes on a CANopen bus, as one body — a Rust port of
//! `drivers/canopen/skynet_canopen/bridge.py`, which is the executable specification.
//!
//! What the Python driver reads off the drives rather than out of a file, it reads here too, and
//! refuses the body over in the same places:
//!
//! | From | Object | Used for |
//! |---|---|---|
//! | encoder resolution | `0x608F:1`, `0x608F:2` | increments to radians |
//! | gear ratio | `0x6091:1`, `0x6091:2` | motor revolutions to shaft revolutions |
//! | software position limits | `0x607D:1`, `0x607D:2` | the descriptor's travel |
//! | max motor speed | `0x6080` | the descriptor's velocity ceiling |
//!
//! Before it commands anything it verifies the PDO mapping (`0x1A01`, `0x1601`), refuses a
//! latched fault **before** anything could clear it (there is no NMT reset in the open path, and
//! the Python module's `_start` says why), and walks each drive to `operation enabled` reading the
//! statusword back at every step. It declares no torque rating: `0x6076` is the motor's design
//! value, not a joint's safety rating (§63.4), so the envelope is null.
//!
//! # Where the port knowingly differs from the Python driver
//!
//! * The configuration is checked whole before the bus is opened, so a configuration that is both
//!   malformed and pointed at an absent interface is refused for the first rather than the second.
//! * A configuration value of the wrong JSON type is refused: a non-string `name` or `role`, a
//!   non-boolean `simulator`, an integral-looking `node` that is not a number. Python's `bool()`
//!   and truthiness accept those. (JSON's `1.0` and `1` are one number to `skynet-json`, so a node
//!   of `1.0` is node 1 here and refused by Python's `isinstance(node, int)`.)
//! * A TPDO2 that is not six bytes is counted as a frame this driver does not speak. Python's
//!   `parse_tpdo2` raises there, out of `read_state` or `open`.
//! * A bus error is `SKR_EIO`. Python raises `OSError` out of whichever entry point met it.
//!
//! Everything else is the Python structure, translated — including `DRY-COVERAGE-PLAN.md` F73's
//! four corrections, which were made to the Python driver first because it is the specification.

use std::io;
use std::time::{Duration, Instant};

use skynet_json::Json;
use skynet_rhal::{flags, status, Homing};

use crate::bus::Bus;
use crate::cia402::{self, od, Command, Object, State, Units};
use crate::frames::{self, Frame, SdoError};

/// The default mapping this driver speaks, as CiA 301 packs a mapping entry.
const TPDO2_EXPECTED: [u32; 2] = [(0x6041 << 16) | 16, (0x6064 << 16) | 32];
const RPDO2_EXPECTED: [u32; 2] = [(0x6040 << 16) | 16, (0x607A << 16) | 32];

/// How long to wait for a drive to answer an SDO before giving up on the body. `bridge.py`'s.
const SDO_TIMEOUT: Duration = Duration::from_millis(500);

/// How long one poll inside an SDO wait lasts. `bridge.py`'s ten milliseconds.
const SDO_POLL: Duration = Duration::from_millis(10);

/// `read_state`'s default bound on waiting for every axis's TPDO2 after its SYNC — five
/// milliseconds, some forty six-byte frames at 1 Mbit/s. A bound, not a sleep.
const SYNC_WINDOW_MS: f64 = 5.0;

/// One joint as the operator names it. Nothing on a CANopen bus says which axis of which arm a
/// node is, so the name and the role are configuration and never discovered.
#[derive(Debug, Clone, PartialEq)]
pub struct JointConfig {
    /// The CANopen node id, 1 to 127.
    pub node: u8,
    /// The joint's name, which a policy channel binds to.
    pub name: String,
    /// The §5.1 role, if the operator gave one.
    pub role: Option<String>,
}

/// `open`'s configuration, resolved and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// One entry per axis, in the order the body reports them.
    pub joints: Vec<JointConfig>,
    /// The SocketCAN interface: `vcan0` unless the configuration names one.
    pub interface: String,
    /// Whether the body is simulated. Defaults true, for §35.3's reason: declaring simulated about
    /// a real body caps its tier, while the reverse presents a simulator as evidence — and a CiA
    /// 402 drive and a simulator of one answer identically, so the driver cannot tell.
    pub simulator: bool,
    /// How long a read waits for this SYNC's answers.
    pub sync_window: Duration,
}

/// Why a body did not come up, in the RHAL's terms and in an integrator's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The configuration is not a JSON object: `SKR_EINVAL`.
    Malformed(String),
    /// The configuration or the drives refused the body: `SKR_ENODEV`, `BridgeRefusal`'s code.
    Refused(String),
    /// The bus could not be opened or used: `SKR_EIO`.
    Bus(String),
}

impl OpenError {
    /// The RHAL status this refusal returns.
    pub fn status(&self) -> i32 {
        match self {
            OpenError::Malformed(_) => status::EINVAL,
            OpenError::Refused(_) => status::ENODEV,
            OpenError::Bus(_) => status::EIO,
        }
    }

    /// The sentence for stderr.
    pub fn message(&self) -> &str {
        match self {
            OpenError::Malformed(text) | OpenError::Refused(text) | OpenError::Bus(text) => text,
        }
    }
}

fn refused(text: impl Into<String>) -> OpenError {
    OpenError::Refused(text.into())
}

fn bus_error(error: io::Error) -> OpenError {
    OpenError::Bus(format!("the bus failed: {error}"))
}

impl Config {
    /// Parse and check the configuration `open` is handed.
    pub fn parse(text: &str) -> Result<Config, OpenError> {
        let settings = if text.trim().is_empty() {
            Json::obj()
        } else {
            Json::parse(text).map_err(|error| {
                OpenError::Malformed(format!("the configuration is not JSON: {error:?}"))
            })?
        };
        if settings.as_obj().is_none() {
            return Err(OpenError::Malformed(
                "the configuration is JSON and not an object; give {\"joints\": [...]}".into(),
            ));
        }

        let entries = match settings.get_present("joints") {
            None => &[][..],
            Some(joints) => joints
                .as_arr()
                .ok_or_else(|| refused("`joints` is a list, one entry per axis."))?,
        };
        if entries.is_empty() {
            return Err(refused(
                "no joints in the configuration. This driver cannot discover them: CANopen's \
                 scan tells you which node ids answer, and nothing on the bus says which axis of \
                 which arm a node is, nor what it should be called. Give \
                 `{\"joints\": [{\"node\": 1, \"name\": \"J1\", \"role\": \
                 \"right_arm.shoulder.pitch\"}]}`.",
            ));
        }

        let interface = match settings.get("interface") {
            None => "vcan0".to_string(),
            Some(value) => value
                .as_str()
                .ok_or_else(|| refused("`interface` names a SocketCAN interface, as a string."))?
                .to_string(),
        };
        let simulator = match settings.get("simulator") {
            None => true,
            Some(value) => value.as_bool().ok_or_else(|| {
                refused(
                    "`simulator` is true or false. Anything else is refused rather than read \
                     for its truthiness, because the answer caps the body's tier (§35.3).",
                )
            })?,
        };
        let window_ms = match settings.get("sync_window_ms") {
            None => SYNC_WINDOW_MS,
            Some(value) => value.as_f64().unwrap_or(f64::NAN),
        };
        if !(window_ms > 0.0 && window_ms <= 1000.0) {
            return Err(refused(format!(
                "`sync_window_ms` of {} is not a window. It bounds how long a read waits for \
                 every axis to answer its SYNC, in milliseconds, above zero and at most a second \
                 — a longer one would be a control loop waiting on a bus that has gone.",
                settings
                    .get("sync_window_ms")
                    .and_then(|value| value.to_canonical().ok())
                    .unwrap_or_else(|| "that".into())
            )));
        }

        let mut joints: Vec<JointConfig> = Vec::with_capacity(entries.len());
        for entry in entries {
            let node = entry.get("node").and_then(Json::as_f64);
            let node = match node {
                Some(value) if value.fract() == 0.0 && (1.0..=127.0).contains(&value) => {
                    value as u8
                }
                _ => {
                    let shown = entry
                        .get("node")
                        .and_then(|value| value.to_canonical().ok())
                        .unwrap_or_else(|| "None".into());
                    return Err(refused(format!(
                        "`{shown}` is not a CANopen node id (1 to 127)."
                    )));
                }
            };
            let name = match entry.get("name").and_then(Json::as_str) {
                Some(name) if !name.is_empty() => name.to_string(),
                _ => {
                    return Err(refused(format!(
                        "node {node} has no `name`; a joint the OS cannot name is one no policy \
                         channel can bind to."
                    )))
                }
            };
            let role = match entry.get("role") {
                None | Some(Json::Null) => None,
                Some(value) => Some(
                    value
                        .as_str()
                        .ok_or_else(|| {
                            refused(format!("node {node}'s `role` is a §5.1 role, as a string."))
                        })?
                        .to_string(),
                ),
            };
            if joints.iter().any(|joint| joint.node == node) {
                return Err(refused(duplicate_node(node)));
            }
            joints.push(JointConfig { node, name, role });
        }

        Ok(Config {
            joints,
            interface,
            simulator,
            sync_window: Duration::from_secs_f64(window_ms / 1000.0),
        })
    }
}

/// `bridge.py`'s refusal for two joints on one node, word for word.
fn duplicate_node(node: u8) -> String {
    format!(
        "node {node} is named by two joints. One drive is one axis: two joints bound to it would \
         send it two setpoints a cycle and report one position twice."
    )
}

/// One drive, and the numbers read off it.
#[derive(Debug, Clone)]
struct Axis {
    joint: JointConfig,
    units: Units,
    limit_min: i64,
    limit_max: i64,
    max_motor_rpm: i64,
    position: i32,
    statusword: u16,
    target: i32,
    /// Set once the drive has reported `operation enabled` itself.
    enabled: bool,
}

impl Axis {
    fn node(&self) -> u8 {
        self.joint.node
    }

    fn state(&self) -> State {
        cia402::state_of(self.statusword)
    }

    /// Increments to radians. The units were checked at open, so a `None` here is a defect in
    /// this driver rather than a drive's answer, and it reads as NaN — §24's "not measured" —
    /// rather than as an angle.
    fn to_rad(&self, counts: i64) -> f64 {
        self.units.to_rad(counts).unwrap_or(f64::NAN)
    }
}

/// What one `read_state` found, for the crate root to copy into the caller's `skr_state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// `SKR_OK`, or `SKR_ETIMEDOUT` when no axis answered — the fields are filled either way.
    pub status: i32,
    /// When the SYNC left, on §35.1's clock.
    pub stamp_ns: u64,
    /// The sample sequence number.
    pub seq: u32,
    /// §24's flags word.
    pub flags: u16,
    /// The node of a faulted axis, or 0.
    pub fault_code: u32,
}

/// A body made of CiA 402 axes, over one bus.
pub struct Bridge<B: Bus> {
    bus: B,
    axes: Vec<Axis>,
    simulated: bool,
    descriptor: String,
    seq: u32,
    sync_window: Duration,
    /// Which axes answered this SYNC. Allocated once at open, so a read allocates nothing.
    answered: Vec<bool>,
    /// Frames on an identifier this driver does not speak — counted, never dropped in silence.
    unexpected_frames: u64,
}

impl<B: Bus> Bridge<B> {
    /// Bring the body up over `bus`, in `bridge.py`'s order: start every node, refuse a latched
    /// fault before anything could clear it, read units, mapping and limits, then set the mode and
    /// enable. The bus is consumed; a refusal drops it.
    pub fn open(bus: B, config: &Config) -> Result<Bridge<B>, OpenError> {
        let simulated = config.simulator || bus.synchronous();
        let axes = config
            .joints
            .iter()
            .map(|joint| Axis {
                joint: joint.clone(),
                units: Units {
                    increments: 0,
                    revolutions: 0,
                    gear_motor: 0,
                    gear_shaft: 0,
                },
                limit_min: 0,
                limit_max: 0,
                max_motor_rpm: 0,
                position: 0,
                statusword: 0,
                target: 0,
                enabled: false,
            })
            .collect::<Vec<_>>();
        let count = axes.len();
        let mut bridge = Bridge {
            bus,
            axes,
            simulated,
            descriptor: String::new(),
            seq: 0,
            sync_window: config.sync_window,
            answered: vec![false; count],
            unexpected_frames: 0,
        };

        // Order matters, and the Python suite found out why: a latched fault is read *before*
        // anything that could clear it.
        for index in 0..count {
            let node = bridge.axes[index].node();
            bridge
                .bus
                .send(&frames::nmt(frames::NMT_START, node))
                .map_err(bus_error)?;
        }
        for index in 0..count {
            bridge.refuse_if_faulted(index)?;
        }
        for index in 0..count {
            bridge.read_units(index)?;
            bridge.check_mapping(index)?;
            bridge.read_limits(index)?;
        }
        for index in 0..count {
            bridge.set_mode(index)?;
            bridge.enable(index)?;
        }
        bridge.descriptor = bridge.build_descriptor()?;
        Ok(bridge)
    }

    /// How many joints the body has.
    pub fn joint_count(&self) -> usize {
        self.axes.len()
    }

    /// The node ids, in joint order — what the stop path needs to know without the lock.
    pub fn nodes(&self) -> impl Iterator<Item = u8> + '_ {
        self.axes.iter().map(Axis::node)
    }

    /// Each axis's current target, in joint order, for the stop path's quick-stop frames.
    pub fn targets(&self) -> impl Iterator<Item = i32> + '_ {
        self.axes.iter().map(|axis| axis.target)
    }

    /// Frames seen on identifiers this driver does not speak.
    pub fn unexpected_frames(&self) -> u64 {
        self.unexpected_frames
    }

    /// The embodiment descriptor, equal as a value to `bridge.py`'s for the same drives.
    pub fn describe(&self) -> &str {
        &self.descriptor
    }

    /// Disable every drive and drop the bus to pre-operational. Silent about drives that have
    /// gone away: closing a body whose drives have gone is still closing it.
    pub fn close(mut self) {
        for index in 0..self.axes.len() {
            // A drive that does not answer its disable is a drive that has gone, and the rest are
            // still disabled — `bridge.py`'s `except (BridgeRefusal, SdoAbort): pass`.
            let _gone = self.sdo_write(
                index,
                &od::CONTROLWORD,
                Command::DisableVoltage.controlword() as i64,
            );
        }
        // The body is closing and there is nobody left to report to: a failed send here is the
        // bus already gone, which is what closing it was for.
        let _ = self.bus.send(&frames::nmt(frames::NMT_PRE_OPERATIONAL, 0));
    }

    // ── §24 ──────────────────────────────────────────────────────────────────

    /// Drain what earlier SYNCs left, stamp, send a SYNC and wait up to the window for every
    /// axis's TPDO2 — `bridge.py`'s read after F71. Positions go into `positions`, one per axis;
    /// the caller has checked its length.
    pub fn read_state(
        &mut self,
        estopped: bool,
        positions: &mut [f32],
        clock: fn() -> u64,
    ) -> Result<Reading, i32> {
        // Anything still on the bus answers an *earlier* SYNC. It is applied — a late position is
        // newer than none — and the SYNC after it is the one this read waits for.
        self.drain().map_err(|_| status::EIO)?;
        // §24: stamped at acquisition. A CiA 402 drive samples its synchronous PDOs on SYNC, so
        // the instant the SYNC leaves is the instant the positions were taken.
        let acquired = clock();
        self.bus.send(&frames::sync()).map_err(|_| status::EIO)?;
        let seen = self.collect_tpdo().map_err(|_| status::EIO)?;
        self.seq = self.seq.wrapping_add(1);

        let mut flags = Homing::NotRequired.to_flags();
        if estopped {
            flags |= flags::ESTOP;
        }
        let mut fault_code = 0u32;
        let mut faulted = false;
        for (index, axis) in self.axes.iter().enumerate() {
            positions[index] = axis.to_rad(axis.position as i64) as f32;
            if axis.state().is_fault() {
                faulted = true;
                fault_code = axis.node() as u32;
            }
        }
        if faulted {
            flags |= flags::DEGRADED | flags::FAULT;
        }
        let count = self.axes.len();
        if seen > 0 && seen < count {
            // Some positions under this stamp are a period or more old. Reported, not smoothed.
            flags |= flags::DEGRADED;
        }
        Ok(Reading {
            // Every drive silent for a whole cycle: the sequence still advances, because the read
            // happened, and the caller is told the bus answered nothing.
            status: if seen == 0 {
                status::ETIMEDOUT
            } else {
                status::OK
            },
            stamp_ns: acquired,
            seq: self.seq,
            flags,
            fault_code,
        })
    }

    /// Convert and check every setpoint, then send one RPDO2 per enabled axis — nothing is sent
    /// and nothing stored unless all of them pass (F73). A zero in `mask` holds the axis at its
    /// previous target; its RPDO2 still goes out, because a drive in cyclic synchronous position
    /// mode reads a missing setpoint as a communication fault rather than a hold.
    pub fn write_command(&mut self, values: &[f32], mask: &[u8]) -> i32 {
        if values.len() != self.axes.len() || mask.len() != self.axes.len() {
            return status::EINVAL;
        }
        for (index, axis) in self.axes.iter().enumerate() {
            if !axis.enabled || mask[index] == 0 {
                continue;
            }
            let value = values[index] as f64;
            if !value.is_finite() {
                return status::EINVAL;
            }
            match axis.units.to_increments(value) {
                Some(target) if i32::try_from(target).is_ok() => {}
                _ => return status::EINVAL,
            }
        }
        let enable = Command::EnableOperation.controlword();
        for index in 0..self.axes.len() {
            let axis = &mut self.axes[index];
            if !axis.enabled {
                continue;
            }
            if mask[index] != 0 {
                // Checked above; the conversion is deterministic, so it succeeds again.
                if let Some(target) = axis.units.to_increments(values[index] as f64) {
                    axis.target = target as i32;
                }
            }
            let frame = frames::rpdo2(axis.joint.node, enable, axis.target);
            if self.bus.send(&frame).is_err() {
                return status::EIO;
            }
        }
        status::OK
    }

    // ── CANopen ──────────────────────────────────────────────────────────────

    fn refuse_if_faulted(&mut self, index: usize) -> Result<(), OpenError> {
        let statusword = self.sdo_read(index, &od::STATUSWORD)? as u16;
        let axis = &mut self.axes[index];
        axis.statusword = statusword;
        let state = axis.state();
        if state.is_fault() {
            return Err(refused(format!(
                "node {} is in `{}` (statusword {:#06x}). This body does not come up over a \
                 latched fault. Read 0x603F for the error code and clear it at the drive \
                 deliberately — a driver that sent a fault reset here would restart a machine \
                 whose reason for stopping nobody had looked at, and would do it again on the \
                 next power cycle.",
                axis.node(),
                state.as_str(),
                statusword
            )));
        }
        Ok(())
    }

    fn read_units(&mut self, index: usize) -> Result<(), OpenError> {
        let read = |bridge: &mut Self| -> Result<Units, OpenError> {
            Ok(Units {
                increments: bridge.sdo_read(index, &od::ENCODER_INCREMENTS)?,
                revolutions: bridge.sdo_read(index, &od::ENCODER_REVOLUTIONS)?,
                gear_motor: bridge.sdo_read(index, &od::GEAR_MOTOR_REVOLUTIONS)?,
                gear_shaft: bridge.sdo_read(index, &od::GEAR_SHAFT_REVOLUTIONS)?,
            })
        };
        let units = match read(self) {
            Ok(units) => units,
            Err(OpenError::Refused(why)) => {
                return Err(refused(format!(
                    "{why}\n\nThis body does not come up without it. An increment is not an \
                     angle until the drive says how many of them make a turn, and a driver that \
                     assumed a common value would report every joint's position in radians, \
                     wrong by whatever the real drive uses, to a kernel that bounds travel with \
                     it (§5.2)."
                )))
            }
            Err(other) => return Err(other),
        };
        let axis = &mut self.axes[index];
        axis.units = units;
        if units.increments <= 0
            || units.revolutions <= 0
            || units.gear_motor <= 0
            || units.gear_shaft <= 0
        {
            return Err(refused(format!(
                "node {} reports an encoder resolution of {}/{} and a gear ratio of {}/{}. A zero \
                 in either is not a ratio, and dividing by it would produce an angle nobody \
                 measured.",
                axis.node(),
                units.increments,
                units.revolutions,
                units.gear_motor,
                units.gear_shaft
            )));
        }
        Ok(())
    }

    fn check_mapping(&mut self, index: usize) -> Result<(), OpenError> {
        let pdos: [(Object, [Object; 2], [u32; 2], &str); 2] = [
            (
                od::TPDO2_MAPPING_COUNT,
                [od::TPDO2_MAPPING_1, od::TPDO2_MAPPING_2],
                TPDO2_EXPECTED,
                "TPDO2",
            ),
            (
                od::RPDO2_MAPPING_COUNT,
                [od::RPDO2_MAPPING_1, od::RPDO2_MAPPING_2],
                RPDO2_EXPECTED,
                "RPDO2",
            ),
        ];
        for (count_object, entries, expected, which) in pdos {
            let count = self.sdo_read(index, &count_object)?;
            let found = [
                self.sdo_read(index, &entries[0])? as u32,
                self.sdo_read(index, &entries[1])? as u32,
            ];
            if count != 2 || found != expected {
                let shown = found
                    .iter()
                    .take(count.clamp(0, 2) as usize)
                    .map(|value| format!("{value:#010x}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                let wanted = expected
                    .iter()
                    .map(|value| format!("{value:#010x}"))
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(refused(format!(
                    "node {}'s {which} maps {shown} and this driver speaks the CiA 402 default, \
                     {wanted}. A frame under another mapping is well-formed and means something \
                     else, so it is refused rather than sent: an axis that moves to a number \
                     nobody wrote is the failure this check exists for. Remap the drive, or use \
                     a driver that reads its mapping.",
                    self.axes[index].node()
                )));
            }
        }
        Ok(())
    }

    fn read_limits(&mut self, index: usize) -> Result<(), OpenError> {
        let limit_min = self.sdo_read(index, &od::POSITION_LIMIT_MIN)?;
        let limit_max = self.sdo_read(index, &od::POSITION_LIMIT_MAX)?;
        let max_motor_rpm = self.sdo_read(index, &od::MAX_MOTOR_SPEED)?;
        let axis = &mut self.axes[index];
        axis.limit_min = limit_min;
        axis.limit_max = limit_max;
        axis.max_motor_rpm = max_motor_rpm;
        Ok(())
    }

    fn set_mode(&mut self, index: usize) -> Result<(), OpenError> {
        self.sdo_write(index, &od::MODES_OF_OPERATION, cia402::MODE_CSP as i64)?;
        let display = self.sdo_read(index, &od::MODES_DISPLAY)?;
        if display != cia402::MODE_CSP as i64 {
            return Err(refused(format!(
                "node {} was asked for mode {} (CSP) and reports mode {display}. A drive running \
                 a profile mode generates its own trajectory between setpoints, which is a second \
                 interpolator that does not know the Safety Kernel exists.",
                self.axes[index].node(),
                cia402::MODE_CSP
            )));
        }
        Ok(())
    }

    fn enable(&mut self, index: usize) -> Result<(), OpenError> {
        for (command, expected) in [
            (Command::Shutdown, State::ReadyToSwitchOn),
            (Command::SwitchOn, State::SwitchedOn),
            (Command::EnableOperation, State::OperationEnabled),
        ] {
            self.sdo_write(index, &od::CONTROLWORD, command.controlword() as i64)?;
            let statusword = self.sdo_read(index, &od::STATUSWORD)? as u16;
            let axis = &mut self.axes[index];
            axis.statusword = statusword;
            let reached = axis.state();
            if reached != expected {
                let advice = if reached.is_fault() {
                    "A drive in `fault` has latched something; read 0x603F for the code and \
                     clear it at the drive before the body will come up."
                } else {
                    "The state is read back rather than assumed, because a master that trusted \
                     its own transition would write setpoints into a disabled drive and watch the \
                     axis stand still."
                };
                return Err(refused(format!(
                    "node {} was asked to {} and reports `{}` rather than `{}` (statusword \
                     {statusword:#06x}). {advice}",
                    axis.node(),
                    command.as_str(),
                    reached.as_str(),
                    expected.as_str()
                )));
            }
        }
        self.axes[index].enabled = true;
        Ok(())
    }

    fn sdo_read(&mut self, index: usize, object: &Object) -> Result<i64, OpenError> {
        let node = self.axes[index].node();
        self.bus
            .send(&frames::sdo_upload_request(
                node,
                object.index,
                object.subindex,
            ))
            .map_err(bus_error)?;
        match self.await_sdo(index, object)? {
            Some(raw) => Ok(object.interpret(raw)),
            None => Err(refused(format!(
                "node {node} acknowledged a read of {} as a write, which is a drive whose SDO \
                 server does not follow CiA 301.",
                object.name
            ))),
        }
    }

    fn sdo_write(&mut self, index: usize, object: &Object, value: i64) -> Result<(), OpenError> {
        let node = self.axes[index].node();
        let Some(request) =
            frames::sdo_download_request(node, object.index, object.subindex, value, object.size)
        else {
            return Err(refused(format!(
                "{} is {} bytes wide, which expedited SDO cannot carry.",
                object.name, object.size
            )));
        };
        self.bus.send(&request).map_err(bus_error)?;
        self.await_sdo(index, object)?;
        Ok(())
    }

    fn await_sdo(&mut self, index: usize, object: &Object) -> Result<Option<u32>, OpenError> {
        let node = self.axes[index].node();
        let deadline = Instant::now() + SDO_TIMEOUT;
        while Instant::now() <= deadline {
            let Some(frame) = self.bus.recv(SDO_POLL).map_err(bus_error)? else {
                // A synchronous bus delivers inside `recv`, so nothing now means nothing ever; a
                // real one answers some microseconds later, and the deadline is what waits.
                if self.bus.synchronous() {
                    break;
                }
                continue;
            };
            if frame.id != frames::sdo_response_id(node) {
                self.stash(&frame);
                continue;
            }
            return frames::parse_sdo_response(frame.payload(), object.index, object.subindex)
                .map_err(|error| match error {
                    SdoError::Abort { .. } => {
                        refused(format!("node {node}, {}: {}", object.name, error.explain()))
                    }
                    other => refused(format!("node {node}, {}: {other}", object.name)),
                });
        }
        Err(refused(format!(
            "node {node} did not answer a read of {} ({:#06x}:{:02x}) within 0.5s. A node that \
             is not on the bus and a node that is powered off look the same from here; `candump` \
             is what tells them apart.",
            object.name, object.index, object.subindex
        )))
    }

    /// A frame that arrived while waiting for something else.
    fn stash(&mut self, frame: &Frame) {
        for axis in &mut self.axes {
            let node = axis.joint.node;
            if frame.id == frames::tpdo2_id(node) {
                if let Some((statusword, position)) = frames::parse_tpdo2(frame.payload()) {
                    axis.statusword = statusword;
                    axis.position = position;
                    return;
                }
                break; // not six bytes: a drive speaking some other mapping, counted below
            }
            if frame.id == frames::sdo_response_id(node) {
                return; // another axis's transfer; its own wait will not see it, and says so
            }
            if frame.id == frames::heartbeat_id(node) || frame.id == frames::emcy_id(node) {
                return; // heartbeat and emergency objects are expected traffic
            }
        }
        self.unexpected_frames = self.unexpected_frames.saturating_add(1);
    }

    /// Apply everything already waiting, without waiting for more.
    fn drain(&mut self) -> io::Result<()> {
        while let Some(frame) = self.bus.recv(Duration::ZERO)? {
            self.stash(&frame);
        }
        Ok(())
    }

    /// How many axes answered this SYNC, waiting up to the window for the rest.
    fn collect_tpdo(&mut self) -> io::Result<usize> {
        self.answered.iter_mut().for_each(|seen| *seen = false);
        let mut waiting = self.axes.len();
        let deadline = Instant::now() + self.sync_window;
        while waiting > 0 {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let Some(frame) = self.bus.recv(remaining)? else {
                if self.bus.synchronous() || remaining.is_zero() {
                    break;
                }
                continue;
            };
            let position = self
                .axes
                .iter()
                .position(|axis| frame.id == frames::tpdo2_id(axis.joint.node));
            match position {
                Some(index) => match frames::parse_tpdo2(frame.payload()) {
                    Some((statusword, position)) => {
                        let axis = &mut self.axes[index];
                        axis.statusword = statusword;
                        axis.position = position;
                        if !self.answered[index] {
                            self.answered[index] = true;
                            waiting -= 1;
                        }
                    }
                    None => self.unexpected_frames = self.unexpected_frames.saturating_add(1),
                },
                None => self.stash(&frame),
            }
        }
        Ok(self.axes.len() - waiting)
    }

    // ── the descriptor ───────────────────────────────────────────────────────

    /// `bridge.py`'s `_build_descriptor`, as a value. Rendered in the workspace's one canonical
    /// form rather than in Python's `json.dumps` spelling: the embodiment digest is taken over the
    /// canonical form and never over a file's bytes (`skynetd-io`'s `embodiment`), so the two
    /// drivers agree where it matters exactly when their documents are equal as values — which is
    /// what `tests/live_vcan.rs` asserts against the Python bridge's own `describe()`.
    fn build_descriptor(&self) -> Result<String, OpenError> {
        let mut joints = Json::arr();
        for axis in &self.axes {
            let mut limits = Json::obj();
            limits
                .set("position_min_rad", Json::from(axis.to_rad(axis.limit_min)))
                .set("position_max_rad", Json::from(axis.to_rad(axis.limit_max)));
            if axis.max_motor_rpm > 0 && axis.units.gear_motor > 0 {
                // rpm at the motor to rad/s at the shaft, in `bridge.py`'s order of operations.
                let velocity = axis.max_motor_rpm as f64 / 60.0
                    * 2.0
                    * std::f64::consts::PI
                    * (axis.units.gear_shaft as f64 / axis.units.gear_motor as f64);
                limits.set("velocity_max_rad_s", Json::from(velocity));
            }
            // **No `effort_max_nm`.** `0x6076` is the motor's rated torque: a design value from a
            // data sheet, not a joint safety rating (§63.4).
            let mut joint = Json::obj();
            joint
                .set("name", Json::from(axis.joint.name.as_str()))
                .set(
                    "role",
                    axis.joint
                        .role
                        .as_deref()
                        .map(Json::from)
                        .unwrap_or(Json::Null),
                )
                .set("type", Json::from("revolute"))
                .set("limits", limits)
                .set("control_modes", strings(&["position"]));
            joints.push(joint);
        }
        let mut control = Json::obj();
        control
            .set("modes", strings(&["position"]))
            .set("rate_hz_max", Json::from(1000u32))
            .set("rate_hz_min", Json::from(1u32));
        let mut descriptor = Json::obj();
        descriptor
            .set("schema_version", Json::from(1u32))
            .set("robot_slug", Json::from("canopen-cia402-axes"))
            .set("digest", Json::Null)
            .set("verified", Json::from(false))
            .set("simulator", Json::from(self.simulated))
            .set("base", Json::from("fixed"))
            .set("dof", Json::from(self.axes.len()))
            .set("joints", joints)
            .set("end_effectors", Json::arr())
            .set("sensors", Json::arr())
            .set("control", control)
            // Null, not an empty object: nobody has declared a safety envelope for this body.
            .set("envelope", Json::Null)
            // The software stop the driver has: quick-stop, NMT pre-operational, latched.
            .set("capabilities", strings(&["safety.estop"]))
            .set("extensions", Json::arr())
            .set("solvers", Json::arr());
        descriptor.to_canonical().map_err(|error| {
            refused(format!(
                "the descriptor could not be written ({error:?}): a figure read off a drive is not                  a finite number, and a descriptor carrying one would carry it to the kernel."
            ))
        })
    }
}

fn strings(values: &[&str]) -> Json {
    Json::Arr(values.iter().map(|value| Json::from(*value)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A bus scripted frame by frame: every `send` is recorded, and every `recv` pops the next
    /// scripted answer. It reaches the orderings a kernel bus produces only sometimes — and it is
    /// not a drive: nothing here decides what a drive would say, the test does.
    struct Scripted {
        sent: Vec<Frame>,
        replies: VecDeque<Option<Frame>>,
    }

    impl Bus for Scripted {
        fn synchronous(&self) -> bool {
            true
        }
        fn send(&mut self, frame: &Frame) -> io::Result<()> {
            self.sent.push(*frame);
            Ok(())
        }
        fn recv(&mut self, _timeout: Duration) -> io::Result<Option<Frame>> {
            Ok(self.replies.pop_front().flatten())
        }
    }

    fn opened(nodes: &[u8]) -> Bridge<Scripted> {
        let axes = nodes
            .iter()
            .map(|node| Axis {
                joint: JointConfig {
                    node: *node,
                    name: format!("J{node}"),
                    role: None,
                },
                units: Units {
                    increments: 131072,
                    revolutions: 1,
                    gear_motor: 50,
                    gear_shaft: 1,
                },
                limit_min: -(1 << 22),
                limit_max: 1 << 22,
                max_motor_rpm: 3000,
                position: 0,
                statusword: 0x0237,
                target: 0,
                enabled: true,
            })
            .collect::<Vec<_>>();
        Bridge {
            bus: Scripted {
                sent: Vec::new(),
                replies: VecDeque::new(),
            },
            answered: vec![false; axes.len()],
            axes,
            simulated: true,
            descriptor: String::new(),
            seq: 0,
            sync_window: Duration::from_millis(5),
            unexpected_frames: 0,
        }
    }

    fn clock() -> u64 {
        42
    }

    #[test]
    fn config_needs_joints_and_refuses_what_it_cannot_read() {
        assert!(matches!(Config::parse("{}"), Err(OpenError::Refused(_))));
        assert!(matches!(
            Config::parse("not json"),
            Err(OpenError::Malformed(_))
        ));
        assert!(matches!(Config::parse("[1]"), Err(OpenError::Malformed(_))));
        for bad in [
            r#"{"joints":[{"node":200,"name":"J1"}]}"#,
            r#"{"joints":[{"node":1}]}"#,
            r#"{"joints":[{"node":1,"name":""}]}"#,
            r#"{"joints":[{"node":1.5,"name":"J1"}]}"#,
            r#"{"joints":[{"node":1,"name":"J1","role":7}]}"#,
            r#"{"joints":[{"node":1,"name":"J1"}],"sync_window_ms":0}"#,
            r#"{"joints":[{"node":1,"name":"J1"}],"sync_window_ms":true}"#,
            r#"{"joints":[{"node":1,"name":"J1"}],"sync_window_ms":1001}"#,
            r#"{"joints":[{"node":1,"name":"J1"}],"simulator":"false"}"#,
            r#"{"joints":[{"node":1,"name":"J1"},{"node":1,"name":"J2"}]}"#,
        ] {
            assert!(
                matches!(Config::parse(bad), Err(OpenError::Refused(_))),
                "{bad} is refused"
            );
        }
        let config = Config::parse(
            r#"{"joints":[{"node":3,"name":"J3","role":"right_arm.wrist.roll"}],"sync_window_ms":2}"#,
        )
        .expect("a well-formed configuration");
        assert_eq!(config.interface, "vcan0");
        assert!(
            config.simulator,
            "simulated unless the operator says otherwise (§35.3)"
        );
        assert_eq!(config.sync_window, Duration::from_millis(2));
        assert_eq!(
            config.joints[0].role.as_deref(),
            Some("right_arm.wrist.roll")
        );
    }

    #[test]
    fn a_stale_sdo_answer_is_not_this_question_s_answer() {
        let mut bridge = opened(&[1]);
        // The answer to an earlier question about 0x6041 arrives while 0x608F:1 is asked.
        let stale = Frame::new(0x581, &[0x4B, 0x41, 0x60, 0x00, 0x37, 0x02, 0, 0]).unwrap();
        bridge.bus.replies.push_back(Some(stale));
        let error = bridge
            .sdo_read(0, &od::ENCODER_INCREMENTS)
            .expect_err("refused");
        assert!(
            error.message().contains("answered about 0x6041:00"),
            "{}",
            error.message()
        );
    }

    #[test]
    fn a_read_waits_for_this_syncs_answers_and_says_when_one_is_missing() {
        let mut bridge = opened(&[1, 2]);
        let mut positions = [f32::NAN; 2];
        // A TPDO2 an earlier SYNC left, drained first; then this SYNC's answer from node 1 only.
        bridge
            .bus
            .replies
            .push_back(Some(frames_tpdo2(1, 0x0237, 100)));
        bridge.bus.replies.push_back(None); // the drain ends
        bridge
            .bus
            .replies
            .push_back(Some(frames_tpdo2(1, 0x0237, 131072 * 50)));
        let reading = bridge.read_state(false, &mut positions, clock).unwrap();
        assert_eq!(reading.status, status::OK);
        assert_eq!(reading.stamp_ns, 42, "stamped when the SYNC left");
        assert_ne!(
            reading.flags & flags::DEGRADED,
            0,
            "node 2 missed this SYNC"
        );
        assert!((positions[0] as f64 - 2.0 * std::f64::consts::PI).abs() < 1e-5);
        assert_eq!(
            bridge.bus.sent.last().map(|frame| frame.id),
            Some(frames::SYNC_ID)
        );

        // Nobody answers: the fields are filled and the read says the bus was silent.
        let reading = bridge.read_state(true, &mut positions, clock).unwrap();
        assert_eq!(reading.status, status::ETIMEDOUT);
        assert_ne!(reading.flags & flags::ESTOP, 0);
    }

    #[test]
    fn a_fault_is_reported_with_the_node_that_latched_it() {
        let mut bridge = opened(&[4, 5]);
        let mut positions = [f32::NAN; 2];
        bridge.bus.replies.push_back(None);
        bridge
            .bus
            .replies
            .push_back(Some(frames_tpdo2(4, 0x0237, 0)));
        bridge
            .bus
            .replies
            .push_back(Some(frames_tpdo2(5, 0x0008, 0)));
        let reading = bridge.read_state(false, &mut positions, clock).unwrap();
        assert_eq!(reading.fault_code, 5);
        assert_ne!(reading.flags & flags::FAULT, 0);
    }

    #[test]
    fn a_command_is_refused_whole_before_any_frame_goes_out() {
        let mut bridge = opened(&[1, 2, 3]);
        assert_eq!(
            bridge.write_command(&[0.25, f32::NAN, 0.0], &[1, 1, 1]),
            status::EINVAL
        );
        assert_eq!(
            bridge.write_command(&[1.0e6, 0.0, 0.0], &[1, 1, 1]),
            status::EINVAL,
            "a setpoint beyond RPDO2's i32 target"
        );
        assert!(bridge.bus.sent.is_empty(), "no frame for a refused command");
        assert!(
            bridge.targets().all(|target| target == 0),
            "and nothing stored"
        );

        // A NaN on a held joint is not a setpoint, and is not refused.
        assert_eq!(
            bridge.write_command(&[0.5, f32::NAN, 0.0], &[1, 0, 1]),
            status::OK
        );
        assert_eq!(
            bridge.bus.sent.len(),
            3,
            "one RPDO2 per axis, held ones included"
        );
        assert_eq!(
            bridge.targets().nth(1),
            Some(0),
            "the held axis keeps its target"
        );
    }

    #[test]
    fn traffic_from_nobody_is_counted() {
        let mut bridge = opened(&[1]);
        bridge.stash(&Frame::new(0x123, &[1]).unwrap());
        bridge.stash(&Frame::new(frames::heartbeat_id(1), &[5]).unwrap());
        bridge.stash(&Frame::new(frames::tpdo2_id(1), &[0; 4]).unwrap());
        assert_eq!(
            bridge.unexpected_frames(),
            2,
            "a stranger and a short TPDO2, not the heartbeat"
        );
    }

    #[test]
    fn a_duplicate_node_refusal_is_the_python_sentence() {
        assert!(duplicate_node(3).starts_with("node 3 is named by two joints."));
    }

    fn frames_tpdo2(node: u8, statusword: u16, position: i32) -> Frame {
        let [s0, s1] = statusword.to_le_bytes();
        let [p0, p1, p2, p3] = position.to_le_bytes();
        Frame::new(frames::tpdo2_id(node), &[s0, s1, p0, p1, p2, p3]).unwrap()
    }
}

//! A segment of CiA 402 drives over EtherCAT, as one body — a Rust port of
//! `drivers/ethercat/skynet_ethercat/bridge.py`, which is the executable specification.
//!
//! It reads off each drive what `drivers/canopen` reads — the encoder resolution and gear ratio
//! (`0x608F`, `0x6091`), refusing the body over either, and the software travel (`0x607D`) — and
//! it declares no torque rating: `0x6076` is the motor's design value, not a joint's safety rating
//! (§63.4), so the envelope is null. The profile is `skynet-cia402-rs`, shared with `canopen-rs` as
//! the Python driver imports the CANopen package's.
//!
//! Every cycle is **one** `LRW` carrying every drive's process data, and the master asserts three
//! per slave on its working counter: a drive that dropped off the segment mid-run changes nothing
//! but that number, and `read_state` answers `SKR_EIO` because of it.
//!
//! # Where the port knowingly differs from the Python driver
//!
//! * **No in-process segment.** The Python driver brings up simulated drives in its own process
//!   when told `sim`, and derives `simulator` from the segment's type. The port speaks only to an
//!   interface — the Python drives are served on one by `python3 -m skynet_ethercat.wire` — so it
//!   cannot tell simulated from real by type, and does what every other driver here does: defaults
//!   to `simulator: true` (§35.3's safe direction) and a real integration says `false`.
//! * A configuration value of the wrong JSON type is refused, and so is a body larger than one
//!   `LRW` can carry (123 drives of twelve bytes); Python's `FrameError` there was uncaught.
//! * A frame the ring does not return within 50 ms is `SKR_EIO`; Python's socket timeout raised out
//!   of `read_state`.
//! * `emergency_stop` sends its quick-stop frame without waiting for the answer (the RHAL: *"Never
//!   blocks"*), and the next `read_state` keeps quick-stop in every controlword and asserts the
//!   working counter; Python cycled once, waiting, inside the stop.

use skynet_cia402_rs::{state_of, Command, State, Units};
use skynet_json::Json;
use skynet_rhal::{flags, status};

use crate::master::{Master, MasterError};
use crate::segment::Segment;

/// One `LRW` carries at most this many drives' process data: twelve bytes each, in a datagram of
/// at most 1,486.
pub const MAX_DRIVES: usize =
    crate::frames::MAX_DATAGRAM_DATA / (crate::master::PDO_OUT_BYTES + crate::master::PDO_IN_BYTES);

/// The range of the target position in the cyclic image, an `i32` (`0x607A`).
const TARGET_RANGE: std::ops::RangeInclusive<i64> = (i32::MIN as i64)..=(i32::MAX as i64);

/// One joint as the operator names it: segment order is position, and §5.1's roles are never
/// inferred from a position.
#[derive(Debug, Clone, PartialEq)]
pub struct JointConfig {
    /// The joint's name. `J<n>` when the operator gave none, as in Python.
    pub name: String,
    /// The §5.1 role, if given.
    pub role: Option<String>,
}

/// `open`'s configuration, resolved and checked.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// One entry per drive, in segment order.
    pub joints: Vec<JointConfig>,
    /// The interface the segment is on: config, then `SKYNET_ETHERCAT_INTERFACE`.
    pub interface: Option<String>,
    /// The descriptor's `robot_slug`.
    pub slug: String,
    /// Whether the body is simulated. Defaults true (§35.3).
    pub simulator: bool,
}

/// Why a body did not come up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// A malformed configuration: `SKR_EINVAL`.
    Malformed(String),
    /// The segment could not be opened or did not come up: `SKR_EIO`.
    Io(String),
    /// A drive answered in a way the body cannot be built on: `SKR_EPROTO`, as in Python.
    Protocol(String),
}

impl OpenError {
    /// The RHAL status this returns.
    pub fn status(&self) -> i32 {
        match self {
            OpenError::Malformed(_) => status::EINVAL,
            OpenError::Io(_) => status::EIO,
            OpenError::Protocol(_) => status::EPROTO,
        }
    }

    /// The sentence for stderr.
    pub fn message(&self) -> &str {
        match self {
            OpenError::Malformed(text) | OpenError::Io(text) | OpenError::Protocol(text) => text,
        }
    }
}

fn malformed(text: impl Into<String>) -> OpenError {
    OpenError::Malformed(text.into())
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
            return Err(malformed("the configuration is JSON and not an object."));
        }
        let entries = settings.get("joints").and_then(Json::as_arr).unwrap_or(&[]);
        if entries.is_empty() {
            return Err(malformed(
                "this driver needs a `joints` list in its config, naming each drive in segment \
                 order with the role it carries. Auto-increment addressing is positional and \
                 §5.1's roles are never inferred from a position — `shoulder` is a pitch on one \
                 robot and a yaw on the next.",
            ));
        }
        if entries.len() > MAX_DRIVES {
            return Err(malformed(format!(
                "{} joints is more than one LRW carries: twelve bytes of process data each, in a \
                 datagram of at most 1,486, is {MAX_DRIVES} drives.",
                entries.len()
            )));
        }
        let mut joints = Vec::with_capacity(entries.len());
        for (position, entry) in entries.iter().enumerate() {
            if entry.as_obj().is_none() {
                return Err(malformed(format!(
                    "joint {position} is not an object: {{\"name\": ..., \"role\": ...}}."
                )));
            }
            let name = match entry.get("name") {
                None => format!("J{}", position + 1),
                Some(value) => value
                    .as_str()
                    .ok_or_else(|| malformed(format!("joint {position}'s `name` is a string.")))?
                    .to_string(),
            };
            let role = match entry.get("role") {
                None | Some(Json::Null) => None,
                Some(value) => Some(
                    value
                        .as_str()
                        .ok_or_else(|| {
                            malformed(format!(
                                "joint {position}'s `role` is a §5.1 role, a string."
                            ))
                        })?
                        .to_string(),
                ),
            };
            joints.push(JointConfig { name, role });
        }
        if settings.get("sim").is_some() {
            return Err(OpenError::Io(
                "`sim` asks for drives in this process, and this port has none: serve the Python \
                 driver's simulated drives on one end of a veth pair with `python3 -m \
                 skynet_ethercat.wire --interface ecat1 --drives N` and name the other end as \
                 `interface` — which is how `make check` runs it."
                    .into(),
            ));
        }
        let interface = match settings.get_present("interface") {
            Some(value) => Some(
                value
                    .as_str()
                    .ok_or_else(|| malformed("`interface` names a network interface, a string."))?
                    .to_string(),
            ),
            None => std::env::var("SKYNET_ETHERCAT_INTERFACE")
                .ok()
                .filter(|name| !name.is_empty()),
        };
        let slug = match settings.get("robot_slug") {
            None => "ethercat-segment".to_string(),
            Some(value) => value
                .as_str()
                .ok_or_else(|| malformed("`robot_slug` is a string."))?
                .to_string(),
        };
        let simulator = match settings.get("simulator") {
            None => true,
            Some(value) => value.as_bool().ok_or_else(|| {
                malformed(
                    "`simulator` is true or false — the answer caps the body's tier (§35.3), so \
                     it is not read for its truthiness.",
                )
            })?,
        };
        Ok(Config {
            joints,
            interface,
            slug,
            simulator,
        })
    }
}

/// One drive, and the figures read off it.
#[derive(Debug, Clone)]
struct Drive {
    position: usize,
    joint: JointConfig,
    units: Units,
    travel: Option<(i64, i64)>,
}

impl Drive {
    fn to_rad(&self, counts: i64) -> f64 {
        // The units were checked at open, so `None` would be a defect here rather than a drive's
        // answer, and it reads as NaN — §24's "not measured" — rather than as an angle.
        self.units.to_rad(counts).unwrap_or(f64::NAN)
    }
}

/// What one `read_state` found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    /// The sample sequence number.
    pub seq: u32,
    /// §24's flags.
    pub flags: u16,
    /// The first faulted drive's position, one-based, or 0.
    pub fault_code: u32,
}

/// A segment of CiA 402 drives.
pub struct Bridge<S: Segment> {
    master: Master<S>,
    drives: Vec<Drive>,
    outputs: Vec<u8>,
    last_image: Vec<u8>,
    seq: u32,
    descriptor: String,
}

impl<S: Segment> Bridge<S> {
    /// Bring the segment up and read every drive, in `bridge.py`'s order. The segment is consumed.
    pub fn open(segment: S, config: &Config) -> Result<Bridge<S>, OpenError> {
        let mut master = Master::new(segment);
        let count = config.joints.len();
        master
            .bring_up(count)
            .map_err(|error| OpenError::Io(format!("the segment did not come up: {error}")))?;
        let mut drives = Vec::with_capacity(count);
        for (position, joint) in config.joints.iter().enumerate() {
            drives.push(read_drive(&mut master, position, joint)?);
        }
        let descriptor = describe(&drives, config)?;
        let outputs = vec![0u8; master.logical_length];
        Ok(Bridge {
            master,
            drives,
            outputs,
            last_image: Vec::new(),
            seq: 0,
            descriptor,
        })
    }

    /// How many joints the body has.
    pub fn joint_count(&self) -> usize {
        self.drives.len()
    }

    /// The embodiment descriptor.
    pub fn describe(&self) -> &str {
        &self.descriptor
    }

    /// The master, for the crate root to duplicate the segment and learn the layout.
    pub fn master(&self) -> &Master<S> {
        &self.master
    }

    /// Each drive's target in the image, in joint order, for the stop path's quick-stop frame.
    pub fn targets(&self) -> impl Iterator<Item = i32> + '_ {
        self.drives.iter().map(|drive| {
            let at = self.master.output_at[drive.position];
            i32::from_le_bytes(self.outputs[at + 2..at + 6].try_into().expect("four bytes"))
        })
    }

    /// Quick-stop every drive and let the segment go. Silent: a segment that has gone needs
    /// nothing more, which is `bridge.py`'s `except MasterError: pass`.
    pub fn close(mut self) {
        let _gone = self.quick_stop();
    }

    fn quick_stop(&mut self) -> Result<(), MasterError> {
        self.hold_quick_stop();
        self.last_image = self.master.cycle(&self.outputs)?;
        Ok(())
    }

    /// Every controlword at quick stop, the targets left where they are.
    fn hold_quick_stop(&mut self) {
        let word = Command::QuickStop.controlword().to_le_bytes();
        for drive in &self.drives {
            let at = self.master.output_at[drive.position];
            self.outputs[at..at + 2].copy_from_slice(&word);
        }
    }

    /// One cycle: the outputs out, every drive's statusword and position back. After a stop, every
    /// controlword stays at quick stop — the image the stop path sent, held (see the module header).
    pub fn read_state(
        &mut self,
        estopped: bool,
        positions: &mut [f32],
    ) -> Result<Reading, MasterError> {
        if estopped {
            self.hold_quick_stop();
        }
        let image = self.master.cycle(&self.outputs)?;
        let mut flags = 0u16;
        let mut fault_code = 0u32;
        for (index, drive) in self.drives.iter().enumerate() {
            let at = self.master.input_at[drive.position];
            let statusword = u16::from_le_bytes([image[at], image[at + 1]]);
            let counts = i32::from_le_bytes(image[at + 2..at + 6].try_into().expect("four bytes"));
            positions[index] = drive.to_rad(counts as i64) as f32;
            if state_of(statusword).is_fault() {
                flags |= flags::FAULT;
                if fault_code == 0 {
                    fault_code = drive.position as u32 + 1;
                }
            }
        }
        self.last_image = image;
        self.seq = self.seq.wrapping_add(1);
        if estopped {
            flags |= flags::ESTOP;
        }
        // §27's homing is left unreported (no bits): an absolute encoder needs none and an
        // incremental one does, and nothing this driver reads says which.
        Ok(Reading {
            seq: self.seq,
            flags,
            fault_code,
        })
    }

    /// Convert and check every target, then write each drive's output with one step of the CiA 402
    /// walk. Nothing is written unless all of it passes (F77). `values` and `mask` are one per
    /// drive; the crate root has checked the mode and the counts.
    pub fn write_command(&mut self, values: &[f32], mask: &[u8]) -> i32 {
        if values.len() != self.drives.len() || mask.len() != self.drives.len() {
            return status::EINVAL;
        }
        let mut targets = Vec::with_capacity(self.drives.len());
        for (index, drive) in self.drives.iter().enumerate() {
            let at = self.master.output_at[drive.position];
            let mut target =
                i32::from_le_bytes(self.outputs[at + 2..at + 6].try_into().expect("four")) as i64;
            if mask[index] != 0 {
                match drive.units.to_increments(values[index] as f64) {
                    Some(counts) if TARGET_RANGE.contains(&counts) => target = counts,
                    _ => return status::EINVAL,
                }
            }
            targets.push(target as i32);
        }
        for (index, target) in targets.into_iter().enumerate() {
            let word = self.advance(index);
            let at = self.master.output_at[self.drives[index].position];
            self.outputs[at..at + 2].copy_from_slice(&word.to_le_bytes());
            self.outputs[at + 2..at + 6].copy_from_slice(&target.to_le_bytes());
        }
        status::OK
    }

    /// One step of the walk towards `operation enabled`, from what the drive last *reported* — the
    /// statusword is the fact and a transition is only a request. A latched fault is not reset
    /// here: `drivers/canopen`'s F26, a reset being an operator's to ask for.
    fn advance(&self, index: usize) -> u16 {
        let state = self.state_of(index);
        match state {
            State::OperationEnabled | State::SwitchedOn => Command::EnableOperation.controlword(),
            State::ReadyToSwitchOn => Command::SwitchOn.controlword(),
            State::SwitchOnDisabled => Command::Shutdown.controlword(),
            _ => 0,
        }
    }

    fn state_of(&self, index: usize) -> State {
        let at = self.master.input_at[self.drives[index].position];
        match self.last_image.get(at..at + 2) {
            Some(bytes) => state_of(u16::from_le_bytes([bytes[0], bytes[1]])),
            // Nothing read yet: `switch on disabled` is the state a drive comes up in, and it
            // starts the walk from the beginning rather than commanding a drive never heard from.
            None => State::SwitchOnDisabled,
        }
    }
}

fn read_drive<S: Segment>(
    master: &mut Master<S>,
    position: usize,
    joint: &JointConfig,
) -> Result<Drive, OpenError> {
    let mut read =
        |index: u16, subindex: u8| master.sdo_upload(position, index, subindex, 4, false);
    let units = (|| -> Result<Units, MasterError> {
        Ok(Units {
            increments: read(0x608F, 1)?,
            revolutions: read(0x608F, 2)?,
            gear_motor: read(0x6091, 1)?,
            gear_shaft: read(0x6091, 2)?,
        })
    })()
    .map_err(|error| {
        OpenError::Protocol(format!(
            "drive {position} would not report its encoder resolution or its gear ratio ({error}). \
             A position in increments means nothing without both: a driver that assumed a direct \
             drive would report the motor's angle as the joint's, wrong by the gearbox, in \
             radians, to a Safety Kernel that bounds travel with it."
        ))
    })?;
    // All four figures (F77).
    if units.increments <= 0
        || units.revolutions <= 0
        || units.gear_motor <= 0
        || units.gear_shaft <= 0
    {
        return Err(OpenError::Protocol(format!(
            "drive {position} reports a zero or negative figure in a ratio ({}/{}, {}/{}). \
             Refusing rather than dividing: the result would be a joint angle nobody measured.",
            units.increments, units.revolutions, units.gear_motor, units.gear_shaft
        )));
    }
    let travel = match (
        master.sdo_upload(position, 0x607D, 1, 4, true),
        master.sdo_upload(position, 0x607D, 2, 4, true),
    ) {
        (Ok(low), Ok(high)) => Some((low, high)),
        // Travel is optional in the profile: a drive without it declares none.
        _ => None,
    };
    Ok(Drive {
        position,
        joint: joint.clone(),
        units,
        travel,
    })
}

/// `round(x, 6)` as Python computes it: the correctly rounded six-decimal string, read back.
fn round6(value: f64) -> f64 {
    format!("{value:.6}").parse().unwrap_or(value)
}

fn strings(values: &[&str]) -> Json {
    Json::Arr(values.iter().map(|value| Json::from(*value)).collect())
}

/// `bridge.py`'s `_build_descriptor`, as a value in the canonical form the embodiment digest is
/// taken over.
fn describe(drives: &[Drive], config: &Config) -> Result<String, OpenError> {
    let mut joints = Json::arr();
    for drive in drives {
        let mut limits = Json::obj();
        if let Some((low, high)) = drive.travel {
            limits
                .set("position_min_rad", Json::from(round6(drive.to_rad(low))))
                .set("position_max_rad", Json::from(round6(drive.to_rad(high))));
        }
        // Travel only: `0x6076` is the motor's rated torque, a design value (§63.4).
        let mut joint = Json::obj();
        joint
            .set("name", Json::from(drive.joint.name.as_str()))
            .set(
                "role",
                drive
                    .joint
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
        .set("rate_hz_min", Json::from(50u32))
        .set("command_latency_ms", Json::Null);
    let mut descriptor = Json::obj();
    descriptor
        .set("schema_version", Json::from(1u32))
        .set("robot_slug", Json::from(config.slug.as_str()))
        .set("digest", Json::Null)
        .set("verified", Json::from(false))
        .set("simulator", Json::from(config.simulator))
        .set("base", Json::from("fixed"))
        .set("dof", Json::from(drives.len()))
        .set("joints", joints)
        .set("end_effectors", Json::arr())
        .set("sensors", Json::arr())
        .set("control", control)
        .set("envelope", Json::Null)
        // Quick-stop on every drive, latched: the software stop the driver has. FSoE is the stop
        // it does not have, and the envelope carries no rated stop to match.
        .set("capabilities", strings(&["safety.estop"]))
        .set("solvers", Json::arr());
    descriptor.to_canonical().map_err(|error| {
        OpenError::Protocol(format!(
            "the descriptor could not be written ({error:?}): a figure read off a drive is not a \
             finite number."
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_names_joints_and_refuses_what_it_cannot_read() {
        assert!(matches!(Config::parse("{}"), Err(OpenError::Malformed(_))));
        assert!(matches!(
            Config::parse("nope"),
            Err(OpenError::Malformed(_))
        ));
        assert!(matches!(
            Config::parse(r#"{"joints":[{}],"sim":3}"#),
            Err(OpenError::Io(_))
        ));
        for bad in [
            r#"{"joints":[5]}"#,
            r#"{"joints":[{"name":5}]}"#,
            r#"{"joints":[{"role":5}]}"#,
            r#"{"joints":[{}],"simulator":"no"}"#,
            r#"{"joints":[{}],"interface":7}"#,
        ] {
            assert!(
                matches!(Config::parse(bad), Err(OpenError::Malformed(_))),
                "{bad}"
            );
        }
        let config = Config::parse(
            r#"{"joints":[{},{"name":"elbow","role":"right_arm.elbow.flex"}],"interface":"ecat0"}"#,
        )
        .unwrap();
        assert_eq!(config.joints[0].name, "J1", "the Python default");
        assert_eq!(
            config.joints[1].role.as_deref(),
            Some("right_arm.elbow.flex")
        );
        assert!(config.simulator);
        assert_eq!(config.slug, "ethercat-segment");
    }

    #[test]
    fn a_body_one_lrw_cannot_carry_is_refused() {
        let joints = vec!["{}"; MAX_DRIVES + 1].join(",");
        assert!(matches!(
            Config::parse(&format!(r#"{{"joints":[{joints}]}}"#)),
            Err(OpenError::Malformed(_))
        ));
        assert_eq!(MAX_DRIVES, 123);
    }

    #[test]
    fn round6_is_python_s_round() {
        assert_eq!(round6(0.6283185307179586), 0.628319);
        assert_eq!(round6(-0.6283185307179586), -0.628319);
        assert_eq!(round6(2.0), 2.0);
    }
}

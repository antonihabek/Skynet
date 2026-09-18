//! The Franka Control Interface on the wire — a Rust port of
//! `drivers/franka/skynet_franka/protocol.py`, which is the executable specification.
//!
//! Two transports, and they are not alternatives:
//!
//! * **TCP 1337** carries commands — connect, move, stop, error recovery. Request and reply, one
//!   `CommandHeader` each, and every reply carries the id of the request it answers.
//! * **UDP**, on a port the client names in `Connect`, carries the robot at **1 kHz**: a
//!   `RobotState` out on every tick and a `RobotCommand` back. Nothing is acknowledged and nothing
//!   is retried.
//!
//! ## What holds this port, and in which order
//!
//! 1. `drivers/franka/layout.json` — the vendor's own headers compiled with `offsetof`/`sizeof`.
//!    `tests/layout.rs` asserts these constants against it, so this module is held to Franka's
//!    compiler directly rather than through the Python transcription.
//! 2. `tests/vectors.json` — golden wire bytes generated from `protocol.py`. `tests/vectors.rs`
//!    asserts these pack/unpack functions against them, so the framing is byte-identical with the
//!    Python driver.
//!
//! What neither establishes is behaviour against a real Control box — that needs the box, and the
//! Python driver's README says so. See 6.6 in `DRY-COVERAGE-PLAN.md`.

//! The constants below are Franka's own enum and offset vocabulary, transcribed — the names are
//! the documentation, and a doc comment repeating each one would be noise. So the module allows
//! missing docs the way `skynet_rhal`'s own vocabulary modules do, and the items with behaviour
//! (`Header`, `RobotState`, the pack/unpack functions) are documented individually.
#![allow(missing_docs)]

use std::fmt;

/// The protocol version this transcription targets. `Connect` is refused by a Control box running
/// a different one, which is the vendor's own version gate and the only one there is.
pub const VERSION: u16 = 10;

/// Where a Control box listens for commands.
pub const COMMAND_PORT: u16 = 1337;

// ── Commands (service_types.h, `enum class Command : uint32_t`) ───────────────

pub const COMMAND_CONNECT: u32 = 0;
pub const COMMAND_MOVE: u32 = 1;
pub const COMMAND_STOP_MOVE: u32 = 2;
pub const COMMAND_SET_COLLISION_BEHAVIOR: u32 = 3;
pub const COMMAND_SET_JOINT_IMPEDANCE: u32 = 4;
pub const COMMAND_SET_CARTESIAN_IMPEDANCE: u32 = 5;
pub const COMMAND_SET_GUIDING_MODE: u32 = 6;
pub const COMMAND_SET_EE_TO_K: u32 = 7;
pub const COMMAND_SET_NE_TO_EE: u32 = 8;
pub const COMMAND_SET_LOAD: u32 = 9;
pub const COMMAND_AUTOMATIC_ERROR_RECOVERY: u32 = 10;
pub const COMMAND_GET_ROBOT_MODEL: u32 = 11;

/// The name `Move`'s refusal carries on stderr, mirroring `MOVE_STATUS_NAMES`.
pub fn move_status_name(status: u8) -> &'static str {
    match status {
        MOVE_SUCCESS => "success",
        MOVE_STARTED => "motion started",
        MOVE_PREEMPTED => "preempted",
        MOVE_PREEMPTED_DUE_TO_SAFETY => "preempted by an activated safety function",
        MOVE_REJECTED_DUE_TO_SAFETY => "rejected by an activated safety function",
        MOVE_COMMAND_NOT_POSSIBLE_REJECTED => "not possible in this state",
        MOVE_START_AT_SINGULAR_POSE_REJECTED => "rejected: the start pose is singular",
        MOVE_INVALID_ARGUMENT_REJECTED => "rejected: invalid argument",
        MOVE_REFLEX_ABORTED => "aborted by a reflex",
        MOVE_EMERGENCY_ABORTED => "aborted by the emergency stop",
        MOVE_INPUT_ERROR_ABORTED => "aborted: input error",
        MOVE_ABORTED => "aborted",
        _ => "unknown move status",
    }
}

// ── Statuses ─────────────────────────────────────────────────────────────────

pub const CONNECT_SUCCESS: u8 = 0;
pub const CONNECT_INCOMPATIBLE_LIBRARY_VERSION: u8 = 1;

pub const MOVE_SUCCESS: u8 = 0;
pub const MOVE_STARTED: u8 = 1;
pub const MOVE_PREEMPTED: u8 = 2;
pub const MOVE_PREEMPTED_DUE_TO_SAFETY: u8 = 3;
pub const MOVE_REJECTED_DUE_TO_SAFETY: u8 = 4;
pub const MOVE_COMMAND_NOT_POSSIBLE_REJECTED: u8 = 5;
pub const MOVE_START_AT_SINGULAR_POSE_REJECTED: u8 = 6;
pub const MOVE_INVALID_ARGUMENT_REJECTED: u8 = 7;
pub const MOVE_REFLEX_ABORTED: u8 = 8;
pub const MOVE_EMERGENCY_ABORTED: u8 = 9;
pub const MOVE_INPUT_ERROR_ABORTED: u8 = 10;
pub const MOVE_ABORTED: u8 = 11;

// ── The robot's own modes (rbk_types.h) ──────────────────────────────────────

pub const MOTION_IDLE: u8 = 0;
pub const MOTION_JOINT_POSITION: u8 = 1;
pub const MOTION_JOINT_VELOCITY: u8 = 2;
#[allow(dead_code)]
pub const MOTION_CARTESIAN_POSITION: u8 = 3;
#[allow(dead_code)]
pub const MOTION_CARTESIAN_VELOCITY: u8 = 4;
pub const MOTION_NONE: u8 = 5;

pub const CONTROLLER_JOINT_IMPEDANCE: u8 = 0;
#[allow(dead_code)]
pub const CONTROLLER_CARTESIAN_IMPEDANCE: u8 = 1;
pub const CONTROLLER_EXTERNAL: u8 = 2;
#[allow(dead_code)]
pub const CONTROLLER_OTHER: u8 = 3;

/// `Move::Request`'s own enums, which are **uint32_t** and not the uint8_t ones above. Two
/// vocabularies for one idea, in one header, and the wire tells them apart only by width.
pub const MOVE_CONTROLLER_JOINT_IMPEDANCE: u32 = 0;
#[allow(dead_code)]
pub const MOVE_CONTROLLER_CARTESIAN_IMPEDANCE: u32 = 1;
pub const MOVE_CONTROLLER_EXTERNAL: u32 = 2;

pub const MOVE_MOTION_JOINT_POSITION: u32 = 0;
pub const MOVE_MOTION_JOINT_VELOCITY: u32 = 1;
#[allow(dead_code)]
pub const MOVE_MOTION_CARTESIAN_POSITION: u32 = 2;
#[allow(dead_code)]
pub const MOVE_MOTION_CARTESIAN_VELOCITY: u32 = 3;
pub const MOVE_MOTION_NONE: u32 = 4;

#[allow(dead_code)]
pub const ROBOT_MODE_OTHER: u8 = 0;
pub const ROBOT_MODE_IDLE: u8 = 1;
pub const ROBOT_MODE_MOVE: u8 = 2;
#[allow(dead_code)]
pub const ROBOT_MODE_GUIDING: u8 = 3;
pub const ROBOT_MODE_REFLEX: u8 = 4;
pub const ROBOT_MODE_USER_STOPPED: u8 = 5;
#[allow(dead_code)]
pub const ROBOT_MODE_AUTOMATIC_ERROR_RECOVERY: u8 = 6;

/// Seven joints, and it is not a configuration: the FCI's structs are fixed at seven and a Franka
/// arm has seven. A body with a different number is a different protocol.
pub const JOINTS: usize = 7;

// ── Sizes and offsets, held to `layout.json` ─────────────────────────────────

/// `CommandHeader`: command, command_id, size — three uint32, twelve bytes.
pub const HEADER_SIZE: usize = 12;

/// `Connect::Request`: version and udp_port, two uint16.
pub const CONNECT_REQUEST_SIZE: usize = 4;

/// `Connect::Response`: a uint8 status with a uint16 after it and no padding.
pub const CONNECT_RESPONSE_SIZE: usize = 3;

/// `Move::Request`: two uint32 enums, two `Deviation`s of three doubles, a bool, and seven doubles
/// of maximum velocity that only the async generator reads. 8 + 48 + 1 + 56 = 113.
pub const MOVE_REQUEST_SIZE: usize = 113;

pub const ROBOT_STATE_SIZE: usize = 1377;
pub const ROBOT_COMMAND_SIZE: usize = 371;

/// Offsets into `RobotState` for the fields a §24 driver reads. The rest of the 1,377 bytes is
/// real and is skipped rather than parsed: a Cartesian pose this driver does not publish is a
/// field it would have to keep correct for no reader.
pub const STATE_MESSAGE_ID: usize = 0;
pub const STATE_TAU_J: usize = 512;
pub const STATE_Q: usize = 596;
pub const STATE_DQ: usize = 652;
pub const STATE_MOTION_GENERATOR_MODE: usize = 1288;
pub const STATE_CONTROLLER_MODE: usize = 1289;
pub const STATE_ERRORS: usize = 1290;
pub const STATE_REFLEX_REASON: usize = 1331;
pub const STATE_ROBOT_MODE: usize = 1372;
pub const STATE_SUCCESS_RATE: usize = 1373;

/// 41 `bool`s, one byte each, packed.
pub const ERROR_COUNT: usize = 41;

pub const COMMAND_MESSAGE_ID: usize = 0;
pub const COMMAND_MOTION: usize = 8;
pub const COMMAND_CONTROL: usize = 314;
pub const MOTION_Q_C: usize = 0;
pub const MOTION_DQ_C: usize = 56;
#[allow(dead_code)]
pub const MOTION_VALID_ELBOW: usize = 304;
pub const MOTION_FINISHED: usize = 305;
pub const CONTROL_TAU_J_D: usize = 0;
pub const CONTROL_FINISHED: usize = 56;

/// A message that is not the message the protocol says comes next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolError(pub String);

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProtocolError {}

fn read_u16_le(raw: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([raw[at], raw[at + 1]])
}

fn read_u32_le(raw: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]])
}

fn read_u64_le(raw: &[u8], at: usize) -> u64 {
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&raw[at..at + 8]);
    u64::from_le_bytes(buf)
}

fn read_f32_le(raw: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]])
}

fn read_seven_f32(raw: &[u8], at: usize) -> [f32; JOINTS] {
    let mut out = [0.0f32; JOINTS];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = read_f32_le(raw, at + i * 4);
    }
    out
}

fn write_seven_f64(out: &mut [u8], at: usize, values: &[f64; JOINTS]) {
    for (i, value) in values.iter().enumerate() {
        out[at + i * 8..at + (i + 1) * 8].copy_from_slice(&value.to_le_bytes());
    }
}

/// One `CommandHeader`: what, which exchange, and how many bytes in total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub command: u32,
    pub command_id: u32,
    pub size: u32,
}

impl Header {
    pub fn pack(self) -> [u8; HEADER_SIZE] {
        let mut out = [0u8; HEADER_SIZE];
        out[0..4].copy_from_slice(&self.command.to_le_bytes());
        out[4..8].copy_from_slice(&self.command_id.to_le_bytes());
        out[8..12].copy_from_slice(&self.size.to_le_bytes());
        out
    }

    pub fn unpack(raw: &[u8]) -> Result<Header, ProtocolError> {
        if raw.len() < HEADER_SIZE {
            return Err(ProtocolError(format!(
                "a command header is {HEADER_SIZE} bytes and {} arrived",
                raw.len()
            )));
        }
        let size = read_u32_le(raw, 8);
        if size < HEADER_SIZE as u32 {
            return Err(ProtocolError(format!(
                "a command header declares {size} bytes in total, which is less than the header \
                 itself ({HEADER_SIZE}). Nothing can be read from it."
            )));
        }
        Ok(Header {
            command: read_u32_le(raw, 0),
            command_id: read_u32_le(raw, 4),
            size,
        })
    }
}

/// The seven-joint half of `RobotState`, and the flags a driver has to act on.
///
/// Everything here is read out of the 1,377 bytes at the offsets above. The rest of the message
/// is not parsed and not discarded — [`RobotState::raw`] keeps it — so a caller that needs the
/// Cartesian pose later reads it from the bytes the robot sent rather than from a field this
/// struct kept warm.
#[derive(Debug, Clone)]
pub struct RobotState {
    pub message_id: u64,
    pub q: [f32; JOINTS],
    pub dq: [f32; JOINTS],
    pub tau_j: [f32; JOINTS],
    pub robot_mode: u8,
    pub motion_generator_mode: u8,
    pub controller_mode: u8,
    pub errors: [bool; ERROR_COUNT],
    pub reflex_reason: [bool; ERROR_COUNT],
    pub success_rate: f32,
    pub raw: Vec<u8>,
}

impl RobotState {
    pub fn unpack(raw: &[u8]) -> Result<RobotState, ProtocolError> {
        if raw.len() != ROBOT_STATE_SIZE {
            return Err(ProtocolError(format!(
                "a RobotState is {ROBOT_STATE_SIZE} bytes and {} arrived. A short read here is a \
                 different firmware, not a partial message: this is one UDP datagram.",
                raw.len()
            )));
        }
        let mut errors = [false; ERROR_COUNT];
        for (i, slot) in errors.iter_mut().enumerate() {
            *slot = raw[STATE_ERRORS + i] != 0;
        }
        let mut reflex_reason = [false; ERROR_COUNT];
        for (i, slot) in reflex_reason.iter_mut().enumerate() {
            *slot = raw[STATE_REFLEX_REASON + i] != 0;
        }
        Ok(RobotState {
            message_id: read_u64_le(raw, STATE_MESSAGE_ID),
            q: read_seven_f32(raw, STATE_Q),
            dq: read_seven_f32(raw, STATE_DQ),
            tau_j: read_seven_f32(raw, STATE_TAU_J),
            robot_mode: raw[STATE_ROBOT_MODE],
            motion_generator_mode: raw[STATE_MOTION_GENERATOR_MODE],
            controller_mode: raw[STATE_CONTROLLER_MODE],
            errors,
            reflex_reason,
            success_rate: read_f32_le(raw, STATE_SUCCESS_RATE),
            raw: raw.to_vec(),
        })
    }

    /// Whether any of the 41 error bits is set.
    ///
    /// Read as a whole rather than by name. §24 surfaces a vendor fault code verbatim and the OS
    /// does not interpret it, so which bit is set belongs in `fault_code` and in the black box —
    /// not in a branch here that would go stale the first time Franka adds a forty-second.
    pub fn faulted(&self) -> bool {
        self.errors.iter().any(|bit| *bit)
    }

    /// The lowest set error bit, one-based, or 0.
    ///
    /// §24's `fault_code` is *vendor-specific and surfaced verbatim*, so this is the index of the
    /// first error the robot reported rather than anything this driver invented. Zero is a healthy
    /// drive, which is a measurement, and it is why an unread field is absent rather than zero.
    pub fn fault_code(&self) -> u32 {
        for (index, bit) in self.errors.iter().enumerate() {
            if *bit {
                return index as u32 + 1;
            }
        }
        0
    }
}

/// The fields [`pack_robot_command`] sets, read back. For the fake, and for the suite.
#[derive(Debug, Clone, PartialEq)]
pub struct RobotCommandFields {
    pub message_id: u64,
    pub q_c: [f64; JOINTS],
    pub dq_c: [f64; JOINTS],
    pub tau_j_d: [f64; JOINTS],
    pub motion_finished: bool,
}

fn read_seven_f64(raw: &[u8], at: usize) -> [f64; JOINTS] {
    let mut out = [0.0f64; JOINTS];
    for (i, slot) in out.iter_mut().enumerate() {
        let mut buf = [0u8; 8];
        buf.copy_from_slice(&raw[at + i * 8..at + (i + 1) * 8]);
        *slot = f64::from_le_bytes(buf);
    }
    out
}

/// Pack a `Connect::Request`: the version this driver speaks and the UDP port it listens on.
pub fn pack_connect_request(version: u16, udp_port: u16) -> [u8; CONNECT_REQUEST_SIZE] {
    let mut out = [0u8; CONNECT_REQUEST_SIZE];
    out[0..2].copy_from_slice(&version.to_le_bytes());
    out[2..4].copy_from_slice(&udp_port.to_le_bytes());
    out
}

/// Unpack a `Connect::Response` payload: the status byte and the version after it.
pub fn unpack_connect_response(payload: &[u8]) -> (u8, u16) {
    let status = payload
        .first()
        .copied()
        .unwrap_or(CONNECT_INCOMPATIBLE_LIBRARY_VERSION);
    let theirs = if payload.len() >= CONNECT_RESPONSE_SIZE {
        read_u16_le(payload, 1)
    } else {
        0
    };
    (status, theirs)
}

/// One `RobotCommand`, 371 bytes, with the fields this driver sets and zeros elsewhere.
///
/// Every field is a **double** here and a float in the state, which is the asymmetry this module's
/// header calls out. Unset arrays are zeros rather than the last value: the robot reads the mode it
/// was started in and ignores the rest, so a stale array left in a field the robot does not read
/// costs nothing and a stale array in one it *does* read is a command nobody issued.
pub fn pack_robot_command(
    message_id: u64,
    q_c: Option<&[f64; JOINTS]>,
    dq_c: Option<&[f64; JOINTS]>,
    tau_j_d: Option<&[f64; JOINTS]>,
    motion_finished: bool,
) -> [u8; ROBOT_COMMAND_SIZE] {
    let mut out = [0u8; ROBOT_COMMAND_SIZE];
    out[COMMAND_MESSAGE_ID..COMMAND_MESSAGE_ID + 8].copy_from_slice(&message_id.to_le_bytes());
    if let Some(values) = q_c {
        write_seven_f64(&mut out, COMMAND_MOTION + MOTION_Q_C, values);
    }
    if let Some(values) = dq_c {
        write_seven_f64(&mut out, COMMAND_MOTION + MOTION_DQ_C, values);
    }
    if let Some(values) = tau_j_d {
        write_seven_f64(&mut out, COMMAND_CONTROL + CONTROL_TAU_J_D, values);
    }
    out[COMMAND_MOTION + MOTION_FINISHED] = u8::from(motion_finished);
    out[COMMAND_CONTROL + CONTROL_FINISHED] = u8::from(motion_finished);
    out
}

pub fn unpack_robot_command(raw: &[u8]) -> Result<RobotCommandFields, ProtocolError> {
    if raw.len() != ROBOT_COMMAND_SIZE {
        return Err(ProtocolError(format!(
            "a RobotCommand is {ROBOT_COMMAND_SIZE} bytes and {} arrived.",
            raw.len()
        )));
    }
    Ok(RobotCommandFields {
        message_id: read_u64_le(raw, COMMAND_MESSAGE_ID),
        q_c: read_seven_f64(raw, COMMAND_MOTION + MOTION_Q_C),
        dq_c: read_seven_f64(raw, COMMAND_MOTION + MOTION_DQ_C),
        tau_j_d: read_seven_f64(raw, COMMAND_CONTROL + CONTROL_TAU_J_D),
        motion_finished: raw[COMMAND_MOTION + MOTION_FINISHED] != 0,
    })
}

/// A `Move::Request`, 113 bytes, without its header.
///
/// The deviations default wide deliberately. They are the Control box's own tolerance on how far a
/// motion may stray before it aborts, and a driver that narrowed them would be putting a second
/// envelope in front of §7's — one the Safety Kernel cannot see, cannot report and did not choose.
pub fn pack_move_payload(
    controller_mode: u32,
    motion_generator_mode: u32,
    path_deviation: [f64; 3],
    goal_deviation: [f64; 3],
) -> [u8; MOVE_REQUEST_SIZE] {
    let mut out = [0u8; MOVE_REQUEST_SIZE];
    out[0..4].copy_from_slice(&controller_mode.to_le_bytes());
    out[4..8].copy_from_slice(&motion_generator_mode.to_le_bytes());
    for (i, value) in path_deviation.iter().enumerate() {
        out[8 + i * 8..16 + i * 8].copy_from_slice(&value.to_le_bytes());
    }
    for (i, value) in goal_deviation.iter().enumerate() {
        out[32 + i * 8..40 + i * 8].copy_from_slice(&value.to_le_bytes());
    }
    out[56] = 0; // the bool the async generator reads; this driver never sets it
                 // out[57..113] is seven doubles of maximum velocity, unread here, and stays zero.
    out
}

/// The same, as a whole command message.
pub fn pack_move_request(
    command_id: u32,
    controller_mode: u32,
    motion_generator_mode: u32,
) -> Vec<u8> {
    let payload = pack_move_payload(
        controller_mode,
        motion_generator_mode,
        [10.0, 10.0, 10.0],
        [10.0, 10.0, 10.0],
    );
    pack_command(COMMAND_MOVE, command_id, &payload)
}

/// Any command message: a header sized to its payload, then the payload.
pub fn pack_command(command: u32, command_id: u32, payload: &[u8]) -> Vec<u8> {
    let header = Header {
        command,
        command_id,
        size: HEADER_SIZE as u32 + payload.len() as u32,
    };
    let mut out = Vec::with_capacity(HEADER_SIZE + payload.len());
    out.extend_from_slice(&header.pack());
    out.extend_from_slice(payload);
    out
}

/// A reply: the same header, then a one-byte status and whatever the reply carries after it.
pub fn pack_response(command: u32, command_id: u32, status: u8, extra: &[u8]) -> Vec<u8> {
    let mut payload = Vec::with_capacity(1 + extra.len());
    payload.push(status);
    payload.extend_from_slice(extra);
    pack_command(command, command_id, &payload)
}

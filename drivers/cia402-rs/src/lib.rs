//! skynet-cia402-rs — CiA 402's drive profile, for the Rust drivers that speak it.
//!
//! A port of `drivers/canopen/skynet_canopen/cia402.py`, which is the executable specification,
//! and shared the way the Python drivers share it: `drivers/ethercat` imports the CANopen
//! package's profile rather than writing it twice, because CoE is CANopen's application layer on
//! EtherCAT's transport — and two implementations of one state machine disagree exactly once,
//! after the failure that matters (§80). `canopen-rs` and `ethercat-rs` both use this crate.
//!
//! A crate of its own rather than a module of `canopen-rs`, because a driver is a cdylib that
//! exports `skynet_rhal_entry`, and a second driver linking the first would link its entry point
//! too. Pure and safe: no I/O, no clock, no `unsafe`.
//!
//! Held to the Python module by `drivers/canopen-rs/tests/vectors.rs`: every state's statusword
//! decodes as Python decodes it, every command's controlword is Python's, and the unit
//! conversions agree with it bit for bit across rounding boundaries.
//!
//! Only the master's half is ported. The Python module also *produces* statuswords and runs the
//! transition table, because its simulated drive needs them; a master reads a statusword and never
//! predicts one — it reads the drive's own report back after every transition it requests.
//!
//! # Decoding a statusword
//!
//! Eight states in six bits, and not a plain enumeration: two states are matched with bit 5
//! masked out, because quick-stop is inactive-high and a faulted drive has no meaningful value for
//! it. The `0x6F` rows are tested first because `0x4F & 0x27` is `0x07`, so `operation enabled`
//! would otherwise read as `fault reaction active`.
//!
//! # Units, and the number a driver must refuse to guess
//!
//! CiA 402 works in increments, and `0x608F` and `0x6091` — the encoder resolution and the gear
//! ratio — are optional in the profile. So the conversions take all four figures as arguments,
//! with no default, and a driver refuses a body whose drives will not report them (§5.2).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::f64::consts::PI;

/// CiA 402's eight states, in the profile's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// `not ready to switch on`.
    NotReadyToSwitchOn,
    /// `switch on disabled`.
    SwitchOnDisabled,
    /// `ready to switch on`.
    ReadyToSwitchOn,
    /// `switched on`.
    SwitchedOn,
    /// `operation enabled` — the only state in which a setpoint moves the axis.
    OperationEnabled,
    /// `quick stop active`.
    QuickStopActive,
    /// `fault reaction active`.
    FaultReactionActive,
    /// `fault` — latched until somebody clears it at the drive.
    Fault,
}

impl State {
    /// The spelling `cia402.State.value` uses, which the refusals and the vectors carry.
    pub fn as_str(self) -> &'static str {
        match self {
            State::NotReadyToSwitchOn => "not_ready_to_switch_on",
            State::SwitchOnDisabled => "switch_on_disabled",
            State::ReadyToSwitchOn => "ready_to_switch_on",
            State::SwitchedOn => "switched_on",
            State::OperationEnabled => "operation_enabled",
            State::QuickStopActive => "quick_stop_active",
            State::FaultReactionActive => "fault_reaction_active",
            State::Fault => "fault",
        }
    }

    /// The two states a person has to clear before anything moves.
    pub fn is_fault(self) -> bool {
        matches!(self, State::Fault | State::FaultReactionActive)
    }
}

/// `(mask, value, state)`, in the order they must be tested. See the module header.
const DECODE: [(u16, u16, State); 8] = [
    (0x6F, 0x27, State::OperationEnabled),
    (0x6F, 0x23, State::SwitchedOn),
    (0x6F, 0x21, State::ReadyToSwitchOn),
    (0x6F, 0x07, State::QuickStopActive),
    (0x4F, 0x0F, State::FaultReactionActive),
    (0x4F, 0x08, State::Fault),
    (0x4F, 0x40, State::SwitchOnDisabled),
    (0x4F, 0x00, State::NotReadyToSwitchOn),
];

/// Which state a statusword reports. Total: the last row masks to zero and matches everything
/// the rows above it did not, so there is no statusword without a state.
pub fn state_of(statusword: u16) -> State {
    for (mask, value, state) in DECODE {
        if statusword & mask == value {
            return state;
        }
    }
    State::NotReadyToSwitchOn
}

/// The controlword commands, and the reset that is an edge rather than a level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// `shutdown`: to `ready to switch on`.
    Shutdown,
    /// `switch on`: to `switched on`.
    SwitchOn,
    /// `disable voltage`: to `switch on disabled`.
    DisableVoltage,
    /// `quick stop`: the profile's controlled stop.
    QuickStop,
    /// `disable operation` — the same word as `switch on`, told apart by where it is issued from.
    DisableOperation,
    /// `enable operation`: setpoints move the axis.
    EnableOperation,
    /// `fault reset` — bit 7, and only its rising edge means anything.
    FaultReset,
}

impl Command {
    /// The spelling `cia402.Command.value` uses.
    pub fn as_str(self) -> &'static str {
        match self {
            Command::Shutdown => "shutdown",
            Command::SwitchOn => "switch_on",
            Command::DisableVoltage => "disable_voltage",
            Command::QuickStop => "quick_stop",
            Command::DisableOperation => "disable_operation",
            Command::EnableOperation => "enable_operation",
            Command::FaultReset => "fault_reset",
        }
    }

    /// The controlword that carries this command.
    pub fn controlword(self) -> u16 {
        match self {
            Command::Shutdown => 0x0006,
            Command::SwitchOn => 0x0007,
            Command::DisableOperation => 0x0007,
            Command::EnableOperation => 0x000F,
            Command::DisableVoltage => 0x0000,
            Command::QuickStop => 0x0002,
            Command::FaultReset => 0x0080,
        }
    }
}

/// Modes of operation (`0x6060`): cyclic synchronous position, the one this driver wires.
///
/// Cyclic synchronous rather than profile, for `cia402.py`'s reason — a profile mode has the drive
/// interpolate between setpoints, a second trajectory generator that does not know the Safety
/// Kernel exists. CSV and CST exist in the profile and are not wired here or in Python.
pub const MODE_CSP: i8 = 8;

/// One object-dictionary entry, with the width an expedited SDO needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Object {
    /// The index.
    pub index: u16,
    /// The subindex.
    pub subindex: u8,
    /// Width in bytes, one to four.
    pub size: u8,
    /// Whether the value is two's-complement.
    pub signed: bool,
    /// The name the refusals use, `cia402.OD`'s own.
    pub name: &'static str,
}

impl Object {
    const fn new(index: u16, subindex: u8, size: u8, signed: bool, name: &'static str) -> Object {
        Object {
            index,
            subindex,
            size,
            signed,
            name,
        }
    }

    /// A raw wire value read as this object's type: sign-extended where the object is signed.
    pub fn interpret(&self, raw: u32) -> i64 {
        if !self.signed {
            return raw as i64;
        }
        let bits = 8 * self.size as u32;
        let raw = raw as i64;
        if raw >= 1i64 << (bits - 1) {
            raw - (1i64 << bits)
        } else {
            raw
        }
    }
}

/// The objects this driver touches, and nothing else — `cia402.OD`, less the ones only the
/// simulated drive reads.
pub mod od {
    use super::Object;

    /// `0x6040`.
    pub const CONTROLWORD: Object = Object::new(0x6040, 0, 2, false, "controlword");
    /// `0x6041`.
    pub const STATUSWORD: Object = Object::new(0x6041, 0, 2, false, "statusword");
    /// `0x6060`.
    pub const MODES_OF_OPERATION: Object = Object::new(0x6060, 0, 1, true, "modes of operation");
    /// `0x6061`.
    pub const MODES_DISPLAY: Object = Object::new(0x6061, 0, 1, true, "modes of operation display");
    /// `0x607D:1`.
    pub const POSITION_LIMIT_MIN: Object =
        Object::new(0x607D, 1, 4, true, "software position limit, minimum");
    /// `0x607D:2`.
    pub const POSITION_LIMIT_MAX: Object =
        Object::new(0x607D, 2, 4, true, "software position limit, maximum");
    /// `0x6080`.
    pub const MAX_MOTOR_SPEED: Object = Object::new(0x6080, 0, 4, false, "max motor speed");
    /// `0x608F:1`.
    pub const ENCODER_INCREMENTS: Object = Object::new(
        0x608F,
        1,
        4,
        false,
        "position encoder resolution, increments",
    );
    /// `0x608F:2`.
    pub const ENCODER_REVOLUTIONS: Object = Object::new(
        0x608F,
        2,
        4,
        false,
        "position encoder resolution, revolutions",
    );
    /// `0x6091:1`.
    pub const GEAR_MOTOR_REVOLUTIONS: Object =
        Object::new(0x6091, 1, 4, false, "gear ratio, motor revolutions");
    /// `0x6091:2`.
    pub const GEAR_SHAFT_REVOLUTIONS: Object =
        Object::new(0x6091, 2, 4, false, "gear ratio, shaft revolutions");
    /// `0x1A01:0`.
    pub const TPDO2_MAPPING_COUNT: Object =
        Object::new(0x1A01, 0, 1, false, "TPDO2 mapping, entry count");
    /// `0x1A01:1`.
    pub const TPDO2_MAPPING_1: Object =
        Object::new(0x1A01, 1, 4, false, "TPDO2 mapping, first entry");
    /// `0x1A01:2`.
    pub const TPDO2_MAPPING_2: Object =
        Object::new(0x1A01, 2, 4, false, "TPDO2 mapping, second entry");
    /// `0x1601:0`.
    pub const RPDO2_MAPPING_COUNT: Object =
        Object::new(0x1601, 0, 1, false, "RPDO2 mapping, entry count");
    /// `0x1601:1`.
    pub const RPDO2_MAPPING_1: Object =
        Object::new(0x1601, 1, 4, false, "RPDO2 mapping, first entry");
    /// `0x1601:2`.
    pub const RPDO2_MAPPING_2: Object =
        Object::new(0x1601, 2, 4, false, "RPDO2 mapping, second entry");
}

/// Increments per revolution of the **output shaft**, or `None` when any of the four figures is
/// zero or negative — which is not a ratio, and dividing by it would produce an angle nobody
/// measured. All four, not the two denominators: `cia402.py` checked only those until
/// `DRY-COVERAGE-PLAN.md` F73, and a zero motor-side gear figure became a division by zero.
fn counts_per_shaft_turn(
    increments: i64,
    revolutions: i64,
    gear_motor: i64,
    gear_shaft: i64,
) -> Option<f64> {
    if increments <= 0 || revolutions <= 0 || gear_motor <= 0 || gear_shaft <= 0 {
        return None;
    }
    // Written in `cia402.py`'s order of operations, because floating point is not associative and
    // the vectors compare bits.
    Some((increments as f64 / revolutions as f64) * (gear_motor as f64 / gear_shaft as f64))
}

/// The four figures that make an increment an angle, all read off the drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Units {
    /// `0x608F:1`.
    pub increments: i64,
    /// `0x608F:2`.
    pub revolutions: i64,
    /// `0x6091:1`.
    pub gear_motor: i64,
    /// `0x6091:2`.
    pub gear_shaft: i64,
}

impl Units {
    /// Encoder increments to radians of the output shaft. `None` where the ratio is not one.
    pub fn to_rad(&self, counts: i64) -> Option<f64> {
        let per_turn = counts_per_shaft_turn(
            self.increments,
            self.revolutions,
            self.gear_motor,
            self.gear_shaft,
        )?;
        Some((counts as f64 / per_turn) * 2.0 * PI)
    }

    /// Radians to increments, rounded half away from zero — never truncated, because a truncation
    /// biases every setpoint toward zero and a control loop integrates a bias. `None` where the
    /// ratio is not one or the result is not a finite count.
    pub fn to_increments(&self, radians: f64) -> Option<i64> {
        let per_turn = counts_per_shaft_turn(
            self.increments,
            self.revolutions,
            self.gear_motor,
            self.gear_shaft,
        )?;
        let exact = radians / (2.0 * PI) * per_turn;
        if !exact.is_finite() || exact.abs() >= i64::MAX as f64 {
            return None;
        }
        Some(if exact >= 0.0 {
            (exact + 0.5).floor() as i64
        } else {
            (exact - 0.5).ceil() as i64
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_denominator_is_not_a_ratio() {
        let units = Units {
            increments: 131072,
            revolutions: 0,
            gear_motor: 50,
            gear_shaft: 1,
        };
        assert_eq!(units.to_rad(1), None);
        assert_eq!(units.to_increments(1.0), None);
        let units = Units {
            revolutions: 1,
            gear_shaft: 0,
            ..units
        };
        assert_eq!(units.to_rad(1), None);
        let units = Units {
            gear_shaft: 1,
            gear_motor: 0,
            ..units
        };
        assert_eq!(units.to_rad(1), None, "a zero numerator too (F73)");
    }

    #[test]
    fn a_setpoint_that_is_not_a_count_is_refused() {
        let units = Units {
            increments: 131072,
            revolutions: 1,
            gear_motor: 50,
            gear_shaft: 1,
        };
        assert_eq!(units.to_increments(f64::NAN), None);
        assert_eq!(units.to_increments(f64::INFINITY), None);
        assert_eq!(units.to_increments(1e300), None);
    }

    #[test]
    fn signed_objects_sign_extend_at_their_own_width() {
        assert_eq!(od::MODES_DISPLAY.interpret(0xFD), -3);
        assert_eq!(od::POSITION_LIMIT_MIN.interpret(0xFFC0_0000), -(1 << 22));
        assert_eq!(od::STATUSWORD.interpret(0xFFFF), 0xFFFF);
        assert_eq!(od::MAX_MOTOR_SPEED.interpret(0xFFFF_FFFF), 0xFFFF_FFFF);
    }
}

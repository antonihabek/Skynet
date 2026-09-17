//! The robot, behind a trait.
//!
//! [`Actuator`] is what the loop drives. There are two implementations and the second one is the
//! reason the trait exists: [`Rhal`] is a real driver loaded from a shared object, and [`Fake`] is a
//! body made of arrays, so every awkward path in [`crate::run`] — a driver that refuses a command,
//! one with no emergency stop, one whose joint count disagrees with the plan — is reachable from a
//! test on a machine with no `.so` in front of it.
//!
//! The trait is deliberately narrow — read, command, stop, and read a sensor — and none of it is
//! *describe the robot*: the descriptor decides the transport plan, the plan decides the channels,
//! and by the time the loop is turning, all of that is settled. A control loop that could still ask
//! the driver what shape the robot is would be a control loop that could change its mind about it
//! mid-run. [`Rhal::describe`] exists for the moment before the loop, and is not on the trait.

use skynet_dl::driver::{Buffers, Driver, DriverError, Sample, SensorFrame};
use std::ffi::CStr;
use std::path::Path;

/// Why a state read failed, in the two forms a caller needs.
///
/// The code is what the record keeps and the sentence is what an operator reads. Splitting them is
/// not tidiness: a recording is replayed months later by somebody who does not have this build's
/// error strings, and a `-110` in a record is a timeout in any language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadFailure {
    /// Negative `errno` (§24). Never zero — zero is what a successful read writes.
    pub code: i32,
    /// What went wrong, for a person.
    pub detail: String,
}

impl ReadFailure {
    /// Build one from a driver error, mapping each variant onto the `errno` that is true of it.
    ///
    /// Every arm is a fact rather than a placeholder, which is the whole reason this is a `match`
    /// and not a constant:
    ///
    /// * a call that returned a code carries **that** code, which is the case §24 is written for;
    /// * a vtable with no `read_state` is `-ENOSYS`, the same answer `Rhal::command` already gives
    ///   for a missing `write_command` — the entry point does not exist;
    /// * a driver whose `open()` refused carries the code it refused with;
    /// * a shared object that would not load, or a vtable this OS rejected, is `-ENODEV`: there is
    ///   no device behind this handle at all. Neither is reachable once a loop is turning, and
    ///   giving them a code rather than a panic is the same contract `command` states — a cycle
    ///   always produces a record, and a record of `-ENODEV` is a true one.
    pub fn from_driver(error: &skynet_dl::driver::DriverError) -> ReadFailure {
        use skynet_dl::driver::DriverError;
        let code = match error {
            DriverError::Call { code, .. } => *code,
            DriverError::Absent(_) => -38,
            DriverError::Open(code) => *code,
            DriverError::Load(_) | DriverError::Rejected(_) => -19,
            // `-EPROTO`, §24's "the device answered, and the answer was not valid": the call
            // succeeded and what came back — an unstamped frame, a header its bytes do not fit, a
            // frame too big for its declared slot — cannot be used as given.
            DriverError::Malformed { .. } | DriverError::Oversized { .. } => -71,
        };
        ReadFailure {
            // A driver that returned zero and called it an error is a driver disagreeing with §24.
            // Recorded as `-EIO` rather than as a success, because the alternative is a record
            // saying the read worked on the cycle the driver said it did not.
            code: if code == 0 { -5 } else { code },
            detail: error.to_string(),
        }
    }
}

/// One command a [`Fake`] was given, kept so a test can assert what reached the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    /// The mode of every joint that inherits.
    pub mode: u8,
    /// The setpoints.
    pub values: Vec<f32>,
    /// 0 = hold, non-zero = apply.
    pub mask: Vec<u8>,
    /// `SKR_JMODE_*` per joint (§86.1), empty on a body-wide command.
    pub modes: Vec<u8>,
}

/// One command the loop is about to write.
///
/// A struct rather than six arguments, and that is not only a lint: `values`, `mask` and `modes`
/// are three arrays indexed by one joint count, two of them `&[u8]`, and passing them positionally
/// is an invitation to swap the two whose meanings are unrelated.
pub struct Write<'a> {
    /// When the command was produced, not when it was sent.
    pub stamp_ns: u64,
    /// Must advance; a repeat is a stalled producer to the watchdog.
    pub seq: u32,
    /// The mode of every joint that inherits (§86.1).
    pub mode: u8,
    /// Setpoints, in each joint's own units.
    pub values: &'a [f32],
    /// 0 = hold, non-zero = apply.
    pub mask: &'a [u8],
    /// `SKR_JMODE_*` per joint (§86.1), or **empty** for a body commanded in one mode — which is
    /// every body that predates the section, and every body whose joints agree.
    pub modes: &'a [u8],
}

/// A body the loop can read and command.
pub trait Actuator {
    /// How many joints, which is the width of every array crossing this trait.
    fn joints(&self) -> usize;

    /// Read one state sample.
    ///
    /// The failure carries a **code** as well as a sentence, because §26's cycle record now holds
    /// what the read returned beside what the write did, and a string is not something a black box
    /// can be replayed against. §24's rule applies to both directions: negative is an `errno`.
    fn sample(&mut self, now_ns: u64) -> Result<Sample, ReadFailure>;

    /// Positions, velocities and efforts from the last successful [`Actuator::sample`].
    fn state(&self) -> (&[f32], &[f32], &[f32]);

    /// Write one command. The return is the driver's own code; negative is an `errno` (§24).
    fn command(&mut self, write: Write<'_>, now_ns: u64) -> i32;

    /// Whether this body has an emergency stop at all.
    ///
    /// Asked before it is needed, because §7.1's Cat 0 response has to know that a driver cannot
    /// perform it rather than find out at the moment it is required.
    fn has_stop(&self) -> bool;

    /// Trigger the emergency stop.
    fn stop(&mut self) -> Result<(), String>;

    /// Read the named sensor's newest frame into `into`, which is its stream's declared slot.
    ///
    /// §47.2's fourth backend — *"anything only the driver can reach"* — and the reason it is on
    /// this trait rather than beside it: the driver is one context, the control loop already holds
    /// it mutably every cycle, and a camera that took its own handle to the same context would be a
    /// second caller on a C object whose thread-safety the ABI never promised.
    ///
    /// No default. A body with no sensors answers `-ENOSYS` because it says so, not because a trait
    /// said it for it.
    fn read_sensor(&mut self, name: &CStr, into: &mut [u8]) -> Result<SensorFrame, ReadFailure>;
}

/// A driver loaded from a shared object.
pub struct Rhal {
    driver: Driver,
    buffers: Buffers,
    joints: usize,
}

impl Rhal {
    /// Load, open, and size the buffers to the joint count the driver reports.
    ///
    /// The width comes from the driver's first state read rather than from the descriptor, and the
    /// two disagreeing is a condition the caller checks: a plan with more channels than the body has
    /// joints is a plan that would command a joint that does not exist.
    pub fn open(path: &Path, config: &str) -> Result<Rhal, DriverError> {
        let mut driver = Driver::open(path, config)?;
        let joints = driver.probe_joints();
        Ok(Rhal {
            driver,
            buffers: Buffers::new(joints),
            joints,
        })
    }

    /// The driver's embodiment descriptor, when it publishes one.
    pub fn describe(&mut self) -> Result<String, DriverError> {
        self.driver.describe()
    }

    /// Whether the driver has a `read_sensor` entry point at all.
    pub fn has_sensors(&self) -> bool {
        self.driver.has_sensors()
    }
}

impl Actuator for Rhal {
    fn joints(&self) -> usize {
        self.joints
    }

    fn sample(&mut self, now_ns: u64) -> Result<Sample, ReadFailure> {
        self.driver
            .read_state(&mut self.buffers, now_ns)
            .map_err(|error| ReadFailure::from_driver(&error))
    }

    fn state(&self) -> (&[f32], &[f32], &[f32]) {
        (
            self.buffers.position(),
            self.buffers.velocity(),
            self.buffers.effort(),
        )
    }

    fn command(&mut self, write: Write<'_>, now_ns: u64) -> i32 {
        // `-ENOSYS` on the one error this can return: a vtable with no `write_command`. `accepts`
        // rejects that before a context is opened, so it is unreachable through `Rhal::open` — and
        // it is given a code rather than a panic because the loop's contract is that a cycle always
        // produces a record, and a record of `-ENOSYS` is a true one.
        //
        // §86.1: an empty `modes` is a body-wide command and takes the entry point every driver
        // written before §86 understands. The array is sent only when there is one, so a v1 driver
        // is never handed a flag it does not read.
        let result = if write.modes.is_empty() {
            self.driver.write_command(
                write.stamp_ns,
                write.seq,
                write.mode,
                write.values,
                write.mask,
                now_ns,
            )
        } else {
            self.driver.write_modal(
                skynet_dl::driver::ModalFrame {
                    stamp_ns: write.stamp_ns,
                    seq: write.seq,
                    mode: write.mode,
                    values: write.values,
                    mask: write.mask,
                    modes: write.modes,
                },
                now_ns,
            )
        };
        result.unwrap_or(-38)
    }

    fn has_stop(&self) -> bool {
        self.driver.has_emergency_stop()
    }

    fn stop(&mut self) -> Result<(), String> {
        self.driver
            .emergency_stop()
            .map_err(|error| error.to_string())
    }

    fn read_sensor(&mut self, name: &CStr, into: &mut [u8]) -> Result<SensorFrame, ReadFailure> {
        self.driver
            .read_sensor(name, into)
            .map_err(|error| ReadFailure::from_driver(&error))
    }
}

/// A body made of arrays.
///
/// Everything the loop can encounter is settable here: the code `write_command` returns, whether an
/// emergency stop exists, and whether the next read fails.
#[derive(Debug, Clone)]
pub struct Fake {
    /// Measured positions, which the fake integrates from what it is commanded.
    pub position: Vec<f32>,
    /// Measured velocities.
    pub velocity: Vec<f32>,
    /// Measured efforts.
    pub effort: Vec<f32>,
    /// What `write_command` returns.
    pub write_result: i32,
    /// Whether this body has an emergency stop.
    pub stop_available: bool,
    /// How many times the stop was triggered.
    pub stops: usize,
    /// Set to make the next [`Actuator::sample`] fail.
    pub read_fails: bool,
    /// Every command the loop wrote, as `(mode, values, mask)`.
    /// Every command this fake was given, in order.
    pub written: Vec<Written>,
    /// The sample sequence, which advances on every read.
    pub seq: u32,
    /// Reported verbatim on every sample.
    pub fault_code: u32,
    /// Reported verbatim on every sample.
    pub flags: u16,
    /// A camera, when the test wants one: its name and its frame size in bytes.
    ///
    /// Each read renders `bytes` bytes of a pattern that is a function of the frame number, so two
    /// frames are told apart by content and a test can check the policy was handed the right one.
    pub camera: Option<(String, usize)>,
    /// Frames the camera has rendered.
    pub frames: u64,
    /// Set to make the next [`Actuator::read_sensor`] fail.
    pub camera_fails: bool,
    /// How many commands had been written when each frame was read.
    ///
    /// The evidence for the order a cycle does things in, which is a claim [`crate::run`] makes
    /// and a test can otherwise only take on trust.
    pub frames_after: Vec<usize>,
    /// The instant of the last [`Actuator::sample`], which is what the camera stamps with.
    ///
    /// The ABI gives `read_sensor` no clock, so a driver stamps with its own. The fake's own clock
    /// is the one the loop last showed it, which is the nearest thing an array has to a sensor.
    pub last_ns: u64,
}

impl Fake {
    /// A body of `joints` joints, at rest, with an emergency stop.
    pub fn new(joints: usize) -> Fake {
        Fake {
            position: vec![0.0; joints],
            velocity: vec![0.0; joints],
            effort: vec![0.0; joints],
            write_result: 0,
            stop_available: true,
            stops: 0,
            read_fails: false,
            written: Vec::new(),
            seq: 0,
            fault_code: 0,
            flags: 0,
            camera: None,
            frames: 0,
            camera_fails: false,
            frames_after: Vec::new(),
            last_ns: 0,
        }
    }

    /// The bytes the camera renders for its `frame`-th frame, counting from one.
    ///
    /// Public so a test can say what it expected rather than re-deriving the pattern.
    pub fn pattern(frame: u64, bytes: usize) -> Vec<u8> {
        (0..bytes)
            .map(|index| (frame as usize).wrapping_add(index) as u8)
            .collect()
    }

    /// The values of the last command written, or `None` if nothing has been written.
    pub fn last(&self) -> Option<&Written> {
        self.written.last()
    }
}

impl Actuator for Fake {
    fn joints(&self) -> usize {
        self.position.len()
    }

    fn sample(&mut self, now_ns: u64) -> Result<Sample, ReadFailure> {
        if self.read_fails {
            // `-ETIMEDOUT`, because that is the failure a bench driver actually produces and a
            // fake that returned a code no real driver returns would exercise a path nothing takes.
            return Err(ReadFailure {
                code: -110,
                detail: "the fake was told to fail this read".to_string(),
            });
        }
        self.seq = self.seq.wrapping_add(1);
        self.last_ns = now_ns;
        Ok(Sample {
            stamp_ns: now_ns,
            seq: self.seq,
            joints: self.position.len(),
            flags: self.flags,
            fault_code: self.fault_code,
            base_pose: [f32::NAN; 7],
            base_twist: [0.0; 6],
        })
    }

    fn state(&self) -> (&[f32], &[f32], &[f32]) {
        (&self.position, &self.velocity, &self.effort)
    }

    fn command(&mut self, write: Write<'_>, _now_ns: u64) -> i32 {
        let Write {
            mode, values, mask, ..
        } = write;
        // A masked-out channel is one the OS said nothing about, so the fake holds it — which is
        // what §24 says a zero mask means, and modelling it as "commanded to `values[i]`" would let
        // a test pass over the exact bug the mask exists to prevent.
        for (index, (value, keep)) in values.iter().zip(mask.iter()).enumerate() {
            if *keep != 0 {
                if let Some(slot) = self.position.get_mut(index) {
                    *slot = *value;
                }
            }
        }
        self.written.push(Written {
            mode,
            values: values.to_vec(),
            mask: mask.to_vec(),
            modes: write.modes.to_vec(),
        });
        self.write_result
    }

    fn has_stop(&self) -> bool {
        self.stop_available
    }

    fn stop(&mut self) -> Result<(), String> {
        if !self.stop_available {
            return Err("this body has no emergency stop".to_string());
        }
        self.stops += 1;
        Ok(())
    }

    fn read_sensor(&mut self, name: &CStr, into: &mut [u8]) -> Result<SensorFrame, ReadFailure> {
        let Some((camera, bytes)) = self.camera.as_ref() else {
            // What a body with no sensors on the RHAL answers: the entry point is not there.
            return Err(ReadFailure {
                code: -38,
                detail: "this fake has no camera".to_string(),
            });
        };
        if name.to_bytes() != camera.as_bytes() {
            return Err(ReadFailure {
                code: -19,
                detail: format!("this fake's camera is `{camera}`"),
            });
        }
        if self.camera_fails {
            return Err(ReadFailure {
                code: -5,
                detail: "the fake was told to fail this frame".to_string(),
            });
        }
        let bytes = *bytes;
        if bytes > into.len() {
            return Err(ReadFailure {
                code: -71,
                detail: format!("a {bytes}-byte frame for a {}-byte slot", into.len()),
            });
        }
        self.frames += 1;
        self.frames_after.push(self.written.len());
        into[..bytes].copy_from_slice(&Fake::pattern(self.frames, bytes));
        Ok(SensorFrame {
            // Never zero, which is the one stamp a driver may not report (§47.2).
            stamp_ns: self.last_ns.max(1),
            width: bytes as u32,
            height: 1,
            stride: bytes as u32,
            format: 0,
            len: bytes,
        })
    }
}

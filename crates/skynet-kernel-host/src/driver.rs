//! The robot, behind a trait.
//!
//! [`Actuator`] is what the loop drives. There are two implementations and the second one is the
//! reason the trait exists: [`Rhal`] is a real driver loaded from a shared object, and [`Fake`] is a
//! body made of arrays, so every awkward path in [`crate::run`] — a driver that refuses a command,
//! one with no emergency stop, one whose joint count disagrees with the plan — is reachable from a
//! test on a machine with no `.so` in front of it.
//!
//! The trait is deliberately narrow. Four operations, and none of them is *describe the robot*:
//! the descriptor decides the transport plan, the plan decides the channels, and by the time the
//! loop is turning, all of that is settled. A control loop that could still ask the driver what
//! shape the robot is would be a control loop that could change its mind about it mid-run.

use skynet_dl::driver::{Buffers, Driver, DriverError, Sample};
use std::path::Path;

/// A body the loop can read and command.
pub trait Actuator {
    /// How many joints, which is the width of every array crossing this trait.
    fn joints(&self) -> usize;

    /// Read one state sample.
    fn sample(&mut self, now_ns: u64) -> Result<Sample, String>;

    /// Positions, velocities and efforts from the last successful [`Actuator::sample`].
    fn state(&self) -> (&[f32], &[f32], &[f32]);

    /// Write one command. The return is the driver's own code; negative is an `errno` (§24).
    fn command(
        &mut self,
        stamp_ns: u64,
        seq: u32,
        mode: u8,
        values: &[f32],
        mask: &[u8],
        now_ns: u64,
    ) -> i32;

    /// Whether this body has an emergency stop at all.
    ///
    /// Asked before it is needed, because §7.1's Cat 0 response has to know that a driver cannot
    /// perform it rather than find out at the moment it is required.
    fn has_stop(&self) -> bool;

    /// Trigger the emergency stop.
    fn stop(&mut self) -> Result<(), String>;
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
}

impl Actuator for Rhal {
    fn joints(&self) -> usize {
        self.joints
    }

    fn sample(&mut self, now_ns: u64) -> Result<Sample, String> {
        self.driver
            .read_state(&mut self.buffers, now_ns)
            .map_err(|error| error.to_string())
    }

    fn state(&self) -> (&[f32], &[f32], &[f32]) {
        (
            self.buffers.position(),
            self.buffers.velocity(),
            self.buffers.effort(),
        )
    }

    fn command(
        &mut self,
        stamp_ns: u64,
        seq: u32,
        mode: u8,
        values: &[f32],
        mask: &[u8],
        now_ns: u64,
    ) -> i32 {
        // `-ENOSYS` on the one error this can return: a vtable with no `write_command`. `accepts`
        // rejects that before a context is opened, so it is unreachable through `Rhal::open` — and
        // it is given a code rather than a panic because the loop's contract is that a cycle always
        // produces a record, and a record of `-ENOSYS` is a true one.
        self.driver
            .write_command(stamp_ns, seq, mode, values, mask, now_ns)
            .unwrap_or(-38)
    }

    fn has_stop(&self) -> bool {
        self.driver.has_emergency_stop()
    }

    fn stop(&mut self) -> Result<(), String> {
        self.driver
            .emergency_stop()
            .map_err(|error| error.to_string())
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
    pub written: Vec<(u8, Vec<f32>, Vec<u8>)>,
    /// The sample sequence, which advances on every read.
    pub seq: u32,
    /// Reported verbatim on every sample.
    pub fault_code: u32,
    /// Reported verbatim on every sample.
    pub flags: u16,
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
        }
    }

    /// The values of the last command written, or `None` if nothing has been written.
    pub fn last(&self) -> Option<&(u8, Vec<f32>, Vec<u8>)> {
        self.written.last()
    }
}

impl Actuator for Fake {
    fn joints(&self) -> usize {
        self.position.len()
    }

    fn sample(&mut self, now_ns: u64) -> Result<Sample, String> {
        if self.read_fails {
            return Err("the fake was told to fail this read".to_string());
        }
        self.seq = self.seq.wrapping_add(1);
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

    fn command(
        &mut self,
        _stamp_ns: u64,
        _seq: u32,
        mode: u8,
        values: &[f32],
        mask: &[u8],
        _now_ns: u64,
    ) -> i32 {
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
        self.written.push((mode, values.to_vec(), mask.to_vec()));
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
}

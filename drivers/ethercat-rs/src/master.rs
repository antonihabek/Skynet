//! The master — a Rust port of `drivers/ethercat/skynet_ethercat/master.py`, which is the
//! executable specification and has been cross-checked against SOEM, an independent master.
//!
//! Scan, configure, walk the AL states, run the cycle — and **assert the working counter on every
//! exchange**, because nothing in EtherCAT refuses: a datagram no slave serviced comes back the
//! right length full of what was sent, and the counter is the only thing that says so.
//!
//! The bring-up order is the protocol's: Init → Pre-Op → Safe-Op → Op. Station addresses are
//! written in Init; the mailbox sync managers before Pre-Op, because CoE is what Pre-Op is for; the
//! process-data managers and the FMMUs before Safe-Op. A mailbox is written whole and read whole,
//! with the input mailbox's *full* bit polled in between — what every real ESC requires, and what
//! SOEM does.
//!
//! Where the port differs: index 0 is never used by an exchange here. It is the stop path's, so a
//! quick-stop frame sent without the master's lock can never be read as the answer to a cycle.

use std::fmt;

use crate::frames::{self, Datagram, Frame, FrameError};
use crate::segment::{Segment, SegmentError};

/// ESC register: type — what a broadcast read counts slaves by.
pub const REG_TYPE: u16 = 0x0000;
/// ESC register: configured station address.
pub const REG_STATION_ADDRESS: u16 = 0x0010;
/// ESC register: AL control, the state a master requests.
pub const REG_AL_CONTROL: u16 = 0x0120;
/// ESC register: AL status, the state a slave holds.
pub const REG_AL_STATUS: u16 = 0x0130;
/// ESC register: AL status code, why a transition was refused.
pub const REG_AL_STATUS_CODE: u16 = 0x0134;
/// Sync manager registers start here, eight bytes each.
pub const REG_SM_BASE: u16 = 0x0800;
/// One sync manager's register block.
pub const REG_SM_STRIDE: u16 = 8;
/// FMMU registers start here, sixteen bytes each.
pub const REG_FMMU_BASE: u16 = 0x0600;
/// One FMMU's register block.
pub const REG_FMMU_STRIDE: u16 = 16;

/// AL state Init.
pub const AL_INIT: u16 = 0x01;
/// AL state Pre-Operational.
pub const AL_PREOP: u16 = 0x02;
/// AL state Safe-Operational.
pub const AL_SAFEOP: u16 = 0x04;
/// AL state Operational.
pub const AL_OP: u16 = 0x08;
/// AL status's error bit: the slave refused the last request and holds a code saying why.
pub const AL_ERROR_BIT: u8 = 0x10;

/// An AL state's name, `slave.AL_NAMES`'s spelling.
pub fn al_name(state: u16) -> String {
    match state {
        0x01 => "Init".into(),
        0x02 => "Pre-Op".into(),
        0x03 => "Boot".into(),
        0x04 => "Safe-Op".into(),
        0x08 => "Op".into(),
        other => other.to_string(),
    }
}

/// An AL status code's meaning, ETG.1000.6's table as `slave.AL_STATUS_NAMES` pins it (F52).
pub fn al_status_name(code: u16) -> &'static str {
    match code {
        0x0000 => "no error",
        0x0011 => "invalid requested state change",
        0x0012 => "unknown requested state",
        0x0013 => "bootstrap not supported",
        0x0016 => "invalid mailbox configuration",
        0x001D => "invalid output configuration",
        0x001E => "invalid input configuration",
        _ => "a code ETG.1000.6 names and this master does not",
    }
}

/// Where each slave's mailbox and process data sit in ESC memory, `master.py`'s choices.
pub const MBX_OUT_START: u16 = 0x1000;
/// The input mailbox.
pub const MBX_IN_START: u16 = 0x1400;
/// A mailbox's length, written and read whole.
pub const MBX_LENGTH: usize = 128;
/// The output process data.
pub const PDO_OUT_START: u16 = 0x1800;
/// The input process data.
pub const PDO_IN_START: u16 = 0x1880;
/// Controlword and target position out.
pub const PDO_OUT_BYTES: usize = 6;
/// Statusword and actual position in.
pub const PDO_IN_BYTES: usize = 6;
/// How many polls of the input mailbox's status before a silent drive is given up on.
pub const MAILBOX_POLLS: usize = 100;
/// The first station address; 0 means "unconfigured" to an ESC.
pub const FIRST_STATION: u16 = 0x1001;

/// CoE's mailbox type.
pub const MBX_TYPE_COE: u8 = 0x03;
/// Six bytes of mailbox header, two of CoE header, then the SDO.
pub const SDO_SPEC_AT: usize = 6 + 2;
/// The SDO's index.
pub const SDO_INDEX_AT: usize = SDO_SPEC_AT + 1;
/// The SDO's data.
pub const SDO_DATA_AT: usize = SDO_SPEC_AT + 4;
/// An expedited SDO message, whole.
pub const SDO_MESSAGE_BYTES: usize = SDO_DATA_AT + 4;

/// The segment did not answer the way the protocol says it must.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterError(pub String);

impl fmt::Display for MasterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<SegmentError> for MasterError {
    fn from(error: SegmentError) -> MasterError {
        MasterError(error.to_string())
    }
}

impl From<FrameError> for MasterError {
    fn from(error: FrameError) -> MasterError {
        MasterError(error.0)
    }
}

/// One segment, brought up and then cycled.
pub struct Master<S: Segment> {
    segment: S,
    index: u8,
    /// How many slaves the last scan counted.
    pub slave_count: usize,
    /// Each slave's output window in the logical image.
    pub output_at: Vec<usize>,
    /// Each slave's input window.
    pub input_at: Vec<usize>,
    /// The logical image's length.
    pub logical_length: usize,
}

impl<S: Segment> Master<S> {
    /// A master over a segment, before any bring-up.
    pub fn new(segment: S) -> Master<S> {
        Master {
            segment,
            index: 0,
            slave_count: 0,
            output_at: Vec::new(),
            input_at: Vec::new(),
            logical_length: 0,
        }
    }

    /// The segment, for the crate root to duplicate for the stop path.
    pub fn segment(&self) -> &S {
        &self.segment
    }

    /// The next index, skipping 0 — which is the stop path's (see the module header).
    fn next_index(&mut self) -> u8 {
        self.index = self.index.wrapping_add(1);
        if self.index == 0 {
            self.index = 1;
        }
        self.index
    }

    /// One datagram, refused unless its working counter is exactly `expect`.
    pub fn exchange(&mut self, datagram: Datagram, expect: u16) -> Result<Datagram, MasterError> {
        let command = datagram.command;
        let answer = self.segment.cycle(&Frame::new(vec![datagram]))?;
        let Some(out) = answer.datagrams.into_iter().next() else {
            return Err(MasterError(
                "the segment returned a frame with no datagram in it.".into(),
            ));
        };
        if out.working_counter != expect {
            return Err(MasterError(format!(
                "{} came back with a working counter of {} and {expect} was required. Nothing in \
                 EtherCAT refuses: a datagram no slave serviced returns the right length full of \
                 what was sent, and this number is the only thing that says so.",
                frames::command_name(command),
                out.working_counter
            )));
        }
        Ok(out)
    }

    /// How many slaves are on the segment, by a broadcast read's working counter.
    pub fn scan(&mut self) -> Result<usize, MasterError> {
        let index = self.next_index();
        let answer = self.segment.cycle(&Frame::new(vec![Datagram::new(
            frames::BRD,
            index,
            (0, REG_TYPE),
            vec![0],
        )]))?;
        self.slave_count = answer
            .datagrams
            .first()
            .map(|datagram| datagram.working_counter as usize)
            .unwrap_or(0);
        Ok(self.slave_count)
    }

    /// Station addresses, sync managers and FMMUs — everything Init is for. A segment with a
    /// different number of slaves than `expected` is refused: auto-increment addressing is
    /// positional, and configuring anyway would address somebody else's machine.
    pub fn configure(&mut self, expected: usize) -> Result<(), MasterError> {
        let found = self.scan()?;
        if found != expected {
            return Err(MasterError(format!(
                "the segment has {found} slave(s) and this configuration describes {expected}. \
                 Auto-increment addressing is positional, so configuring anyway would write each \
                 station address onto whichever machine happens to be at that position."
            )));
        }
        let count = expected as u16;
        let index = self.next_index();
        self.exchange(
            Datagram::new(
                frames::BWR,
                index,
                (0, REG_AL_CONTROL),
                AL_INIT.to_le_bytes().to_vec(),
            ),
            count,
        )?;
        for position in 0..count {
            let station = FIRST_STATION + position;
            let auto = if position == 0 {
                0
            } else {
                (0x10000u32 - position as u32) as u16
            };
            let index = self.next_index();
            self.exchange(
                Datagram::new(
                    frames::APWR,
                    index,
                    (auto, REG_STATION_ADDRESS),
                    station.to_le_bytes().to_vec(),
                ),
                1,
            )?;
        }
        for position in 0..count {
            let station = FIRST_STATION + position;
            self.write_sync_manager(station, 0, MBX_OUT_START, MBX_LENGTH as u16, 0x26)?;
            self.write_sync_manager(station, 1, MBX_IN_START, MBX_LENGTH as u16, 0x22)?;
            self.write_sync_manager(station, 2, PDO_OUT_START, PDO_OUT_BYTES as u16, 0x64)?;
            self.write_sync_manager(station, 3, PDO_IN_START, PDO_IN_BYTES as u16, 0x20)?;
        }
        self.output_at.clear();
        self.input_at.clear();
        let mut cursor = 0;
        for _ in 0..expected {
            self.output_at.push(cursor);
            cursor += PDO_OUT_BYTES;
        }
        for _ in 0..expected {
            self.input_at.push(cursor);
            cursor += PDO_IN_BYTES;
        }
        self.logical_length = cursor;
        for position in 0..expected {
            let station = FIRST_STATION + position as u16;
            let (output, input) = (self.output_at[position], self.input_at[position]);
            self.write_fmmu(
                station,
                0,
                output as u32,
                PDO_OUT_BYTES as u16,
                PDO_OUT_START,
                2,
            )?;
            self.write_fmmu(
                station,
                1,
                input as u32,
                PDO_IN_BYTES as u16,
                PDO_IN_START,
                1,
            )?;
        }
        Ok(())
    }

    fn write_sync_manager(
        &mut self,
        station: u16,
        manager: u16,
        start: u16,
        length: u16,
        control: u8,
    ) -> Result<(), MasterError> {
        let mut body = Vec::with_capacity(8);
        body.extend_from_slice(&start.to_le_bytes());
        body.extend_from_slice(&length.to_le_bytes());
        body.extend_from_slice(&[control, 0, 0x01, 0]);
        let index = self.next_index();
        self.exchange(
            Datagram::new(
                frames::FPWR,
                index,
                (station, REG_SM_BASE + REG_SM_STRIDE * manager),
                body,
            ),
            1,
        )?;
        Ok(())
    }

    /// One FMMU entry, all sixteen bytes in ETG.1000.4's order — `master.py` once had `type` and
    /// `activate` a byte early, which maps nothing and reads as a working counter of zero.
    fn write_fmmu(
        &mut self,
        station: u16,
        fmmu: u16,
        logical: u32,
        length: u16,
        physical: u16,
        kind: u8,
    ) -> Result<(), MasterError> {
        let mut body = Vec::with_capacity(16);
        body.extend_from_slice(&logical.to_le_bytes());
        body.extend_from_slice(&length.to_le_bytes());
        body.extend_from_slice(&[0, 7]); // logical start bit, logical end bit
        body.extend_from_slice(&physical.to_le_bytes());
        body.extend_from_slice(&[0, kind, 0x01, 0, 0, 0]); // physical bit, type, activate, reserved
        let index = self.next_index();
        self.exchange(
            Datagram::new(
                frames::FPWR,
                index,
                (station, REG_FMMU_BASE + REG_FMMU_STRIDE * fmmu),
                body,
            ),
            1,
        )?;
        Ok(())
    }

    /// Ask every slave for `state`, then read back what each holds: AL control is a request and AL
    /// status the answer, and a refusal sets the error bit rather than failing the write.
    pub fn request_state(&mut self, state: u16, expected: usize) -> Result<(), MasterError> {
        let index = self.next_index();
        self.exchange(
            Datagram::new(
                frames::BWR,
                index,
                (0, REG_AL_CONTROL),
                state.to_le_bytes().to_vec(),
            ),
            expected as u16,
        )?;
        for position in 0..expected {
            let station = FIRST_STATION + position as u16;
            let index = self.next_index();
            let answer = self.exchange(
                Datagram::new(frames::FPRD, index, (station, REG_AL_STATUS), vec![0, 0]),
                1,
            )?;
            let held = answer.data[0];
            if held & AL_ERROR_BIT != 0 {
                let index = self.next_index();
                let code = self.exchange(
                    Datagram::new(
                        frames::FPRD,
                        index,
                        (station, REG_AL_STATUS_CODE),
                        vec![0, 0],
                    ),
                    1,
                )?;
                let number = u16::from_le_bytes([code.data[0], code.data[1]]);
                return Err(MasterError(format!(
                    "slave {position} refused {} and is in {}, AL status code 0x{number:04X} ({}).",
                    al_name(state),
                    al_name((held & 0x0F) as u16),
                    al_status_name(number)
                )));
            }
            if (held & 0x0F) as u16 != state {
                return Err(MasterError(format!(
                    "slave {position} was asked for {} and reports {}.",
                    al_name(state),
                    al_name((held & 0x0F) as u16)
                )));
            }
        }
        Ok(())
    }

    /// Init → Pre-Op → configure → Safe-Op → Op, in the order the protocol requires.
    pub fn bring_up(&mut self, expected: usize) -> Result<(), MasterError> {
        self.configure(expected)?;
        self.request_state(AL_PREOP, expected)?;
        self.request_state(AL_SAFEOP, expected)?;
        self.request_state(AL_OP, expected)
    }

    /// Read one object over the mailbox, expedited, as a signed or unsigned integer.
    pub fn sdo_upload(
        &mut self,
        position: usize,
        index: u16,
        subindex: u8,
        size: usize,
        signed: bool,
    ) -> Result<i64, MasterError> {
        let request = coe_request(0x40, index, subindex, [0; 4]);
        let answer = self.mailbox_exchange(position, &request)?;
        coe_value(&answer, index, subindex, size, signed)
    }

    /// The whole output mailbox written, the input mailbox's *full* bit polled, the whole input
    /// mailbox read — each asserting its working counter.
    fn mailbox_exchange(
        &mut self,
        position: usize,
        request: &[u8],
    ) -> Result<Vec<u8>, MasterError> {
        let station = FIRST_STATION + position as u16;
        let mut whole = request.to_vec();
        whole.resize(MBX_LENGTH, 0);
        let index = self.next_index();
        self.exchange(
            Datagram::new(frames::FPWR, index, (station, MBX_OUT_START), whole),
            1,
        )?;
        let status_register = REG_SM_BASE + REG_SM_STRIDE + 5;
        let mut full = false;
        for _ in 0..MAILBOX_POLLS {
            let index = self.next_index();
            let status = self.exchange(
                Datagram::new(frames::FPRD, index, (station, status_register), vec![0]),
                1,
            )?;
            if status.data[0] & 0x08 != 0 {
                full = true;
                break;
            }
        }
        if !full {
            return Err(MasterError(format!(
                "slave {position} did not answer its mailbox within {MAILBOX_POLLS} polls of the \
                 input mailbox's status. Reading the buffer anyway would be denied, and a master \
                 that treated the denial as an empty answer would report a value nobody sent."
            )));
        }
        let index = self.next_index();
        let answer = self.exchange(
            Datagram::new(
                frames::FPRD,
                index,
                (station, MBX_IN_START),
                vec![0; MBX_LENGTH],
            ),
            1,
        )?;
        Ok(answer.data)
    }

    /// One `LRW`: every slave's outputs written and inputs read in one pass, at three per slave.
    pub fn cycle(&mut self, outputs: &[u8]) -> Result<Vec<u8>, MasterError> {
        let mut image = vec![0u8; self.logical_length];
        let n = outputs.len().min(image.len());
        image[..n].copy_from_slice(&outputs[..n]);
        let index = self.next_index();
        let expect = (3 * self.slave_count) as u16;
        let answer = self.exchange(Datagram::new(frames::LRW, index, (0, 0), image), expect)?;
        Ok(answer.data)
    }
}

/// A CoE SDO request in a mailbox message: mailbox header, CoE header, the SDO.
pub fn coe_request(spec: u8, index: u16, subindex: u8, payload: [u8; 4]) -> Vec<u8> {
    let mut body = Vec::with_capacity(10);
    body.extend_from_slice(&(0x02u16 << 12).to_le_bytes());
    body.push(spec);
    body.extend_from_slice(&index.to_le_bytes());
    body.push(subindex);
    body.extend_from_slice(&payload);
    let mut message = Vec::with_capacity(6 + body.len());
    message.extend_from_slice(&(body.len() as u16).to_le_bytes());
    message.extend_from_slice(&0u16.to_le_bytes());
    message.push(0);
    message.push(MBX_TYPE_COE);
    message.extend_from_slice(&body);
    message
}

/// The value in an SDO upload reply, refused where `master.py` refuses: short, not CoE (an
/// all-zero mailbox is the reply that never came), an abort, or an answer about another object.
pub fn coe_value(
    data: &[u8],
    index: u16,
    subindex: u8,
    size: usize,
    signed: bool,
) -> Result<i64, MasterError> {
    if data.len() < SDO_MESSAGE_BYTES {
        return Err(MasterError(format!(
            "an SDO reply is {SDO_MESSAGE_BYTES} bytes and {} came back.",
            data.len()
        )));
    }
    if data[5] & 0x0F != MBX_TYPE_COE {
        return Err(MasterError(
            "the mailbox reply is not a CoE message. All-zero bytes here are a mailbox nobody \
             filled — the reply that never came — and reading them as a value would be a \
             measurement nobody took."
                .into(),
        ));
    }
    let spec = data[SDO_SPEC_AT];
    let reply_index = u16::from_le_bytes([data[SDO_INDEX_AT], data[SDO_INDEX_AT + 1]]);
    let reply_sub = data[SDO_INDEX_AT + 2];
    if spec == 0x80 {
        let code = u32::from_le_bytes(data[SDO_DATA_AT..SDO_DATA_AT + 4].try_into().unwrap());
        return Err(MasterError(format!(
            "the drive aborted the read of 0x{index:04X}:{subindex} with code 0x{code:08X}. An \
             abort is an answer: reading a zero instead would be a measurement nobody took."
        )));
    }
    if (reply_index, reply_sub) != (index, subindex) {
        return Err(MasterError(format!(
            "asked for 0x{index:04X}:{subindex} and the reply names \
             0x{reply_index:04X}:{reply_sub}. A mailbox holds one message and this is somebody \
             else's."
        )));
    }
    let size = size.clamp(1, 4);
    let mut raw = [0u8; 8];
    raw[..size].copy_from_slice(&data[SDO_DATA_AT..SDO_DATA_AT + size]);
    let unsigned = u64::from_le_bytes(raw);
    Ok(if signed {
        let shift = 64 - 8 * size as u32;
        ((unsigned << shift) as i64) >> shift
    } else {
        unsigned as i64
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signed_value_sign_extends_at_its_width() {
        let mut reply = coe_request(0x43, 0x607D, 1, (-(1i32 << 22)).to_le_bytes());
        reply[SDO_SPEC_AT] = 0x43;
        assert_eq!(coe_value(&reply, 0x607D, 1, 4, true).unwrap(), -(1 << 22));
        assert_eq!(
            coe_value(&reply, 0x607D, 1, 4, false).unwrap(),
            0xFFC0_0000,
            "and unsigned, it is not"
        );
    }

    #[test]
    fn an_empty_mailbox_is_not_a_value() {
        assert!(coe_value(&[0; 16], 0x608F, 1, 4, false).is_err());
        assert!(coe_value(&[0; 15], 0x608F, 1, 4, false).is_err());
    }
}

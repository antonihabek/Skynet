//! EtherCAT on the wire — a Rust port of `drivers/ethercat/skynet_ethercat/frames.py`, which is the
//! executable specification.
//!
//! One Ethernet frame, one EtherCAT header, one or more datagrams. Every datagram ends with a
//! 16-bit **working counter** that the master sends as zero and each slave that serviced the
//! datagram increments — so a master does not ask whether something worked, it asserts what the
//! total must be. A slave that is absent, mis-addressed or in the wrong state produces a low
//! counter rather than an error: nothing refuses, and the number at the end is one short.
//!
//! `tests/vectors.rs` holds [`Datagram::pack`], [`Frame::pack`] and the two `unpack`s to frames
//! generated from `frames.py` itself.

use std::fmt;

/// EtherCAT's own EtherType. A frame with any other is somebody else's traffic.
pub const ETHERTYPE: u16 = 0x88A4;
/// EtherCAT frame type 1: datagrams, the only type this driver speaks.
pub const FRAME_TYPE_DATAGRAMS: u16 = 1;
/// An Ethernet payload's floor. A shorter frame is padded, and a decoder that trusted the Ethernet
/// length rather than EtherCAT's own would read the padding as datagrams.
pub const MIN_ETHERNET_PAYLOAD: usize = 46;
/// The largest datagram payload the eleven-bit length field carries.
pub const MAX_DATAGRAM_DATA: usize = 1486;

/// Configured-address read.
pub const FPRD: u8 = 0x04;
/// Configured-address write.
pub const FPWR: u8 = 0x05;
/// Auto-increment write — how a master names a slave it has not configured yet.
pub const APWR: u8 = 0x02;
/// Broadcast read.
pub const BRD: u8 = 0x07;
/// Broadcast write.
pub const BWR: u8 = 0x08;
/// Logical read-write: every drive's process data in one pass.
pub const LRW: u8 = 0x0C;

/// A command's name for a refusal, as `frames.COMMAND_NAMES` spells it.
pub fn command_name(command: u8) -> &'static str {
    match command {
        0x00 => "NOP",
        0x01 => "APRD",
        APWR => "APWR",
        0x03 => "APRW",
        FPRD => "FPRD",
        FPWR => "FPWR",
        0x06 => "FPRW",
        BRD => "BRD",
        BWR => "BWR",
        0x09 => "BRW",
        0x0A => "LRD",
        0x0B => "LWR",
        LRW => "LRW",
        0x0D => "ARMW",
        0x0E => "FRMW",
        _ => "an unknown command",
    }
}

/// Bytes that are not an EtherCAT frame, or one that could not have been sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameError(pub String);

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One datagram: a command, an address, some data and a working counter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Datagram {
    /// The command.
    pub command: u8,
    /// The master's index, echoed back — how an answer is told from another frame.
    pub index: u8,
    /// (position or station, register), or a logical address as (low16, high16).
    pub address: (u16, u16),
    /// The payload.
    pub data: Vec<u8>,
    /// Set on every datagram but a frame's last, by [`Frame::pack`].
    pub more: bool,
    /// The interrupt field.
    pub irq: u16,
    /// Sent as zero; each slave that serviced the datagram adds to it.
    pub working_counter: u16,
}

impl Datagram {
    /// A datagram with nothing serviced yet.
    pub fn new(command: u8, index: u8, address: (u16, u16), data: Vec<u8>) -> Datagram {
        Datagram {
            command,
            index,
            address,
            data,
            more: false,
            irq: 0,
            working_counter: 0,
        }
    }

    fn pack_into(&self, out: &mut Vec<u8>) -> Result<(), FrameError> {
        if self.data.len() > MAX_DATAGRAM_DATA {
            return Err(FrameError(format!(
                "a datagram carries at most {MAX_DATAGRAM_DATA} bytes and this one has {}; the \
                 length field is eleven bits.",
                self.data.len()
            )));
        }
        let mut length = (self.data.len() & 0x7FF) as u16;
        if self.more {
            length |= 1 << 15;
        }
        out.push(self.command);
        out.push(self.index);
        out.extend_from_slice(&self.address.0.to_le_bytes());
        out.extend_from_slice(&self.address.1.to_le_bytes());
        out.extend_from_slice(&length.to_le_bytes());
        out.extend_from_slice(&self.irq.to_le_bytes());
        out.extend_from_slice(&self.data);
        out.extend_from_slice(&self.working_counter.to_le_bytes());
        Ok(())
    }

    /// One datagram from `raw` at `offset`, and where the next one starts.
    pub fn unpack(raw: &[u8], offset: usize) -> Result<(Datagram, usize), FrameError> {
        if raw.len() < offset + 10 {
            return Err(FrameError(format!(
                "a datagram header is ten bytes and only {} remain.",
                raw.len().saturating_sub(offset)
            )));
        }
        let u16_at = |at: usize| u16::from_le_bytes([raw[at], raw[at + 1]]);
        let length = u16_at(offset + 6);
        let data_length = (length & 0x7FF) as usize;
        let start = offset + 10;
        let end = start + data_length;
        if raw.len() < end + 2 {
            return Err(FrameError(format!(
                "a datagram declares {data_length} bytes of data and a working counter, and only \
                 {} remain. A frame that claims more than it carries is a truncation, not a short \
                 read: this is one Ethernet frame.",
                raw.len().saturating_sub(start)
            )));
        }
        Ok((
            Datagram {
                command: raw[offset],
                index: raw[offset + 1],
                address: (u16_at(offset + 2), u16_at(offset + 4)),
                data: raw[start..end].to_vec(),
                more: length & (1 << 15) != 0,
                irq: u16_at(offset + 8),
                working_counter: u16_at(end),
            },
            end + 2,
        ))
    }
}

/// The master's MAC address, `frames.Frame`'s default: locally administered, never a vendor's.
pub const MASTER_MAC: [u8; 6] = [0x02, 0x00, 0x00, 0x00, 0x00, 0x01];
/// Broadcast: an EtherCAT ring does not route by MAC.
pub const BROADCAST_MAC: [u8; 6] = [0xFF; 6];

/// One EtherCAT frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The datagrams, in order.
    pub datagrams: Vec<Datagram>,
    /// The destination MAC.
    pub destination: [u8; 6],
    /// The source MAC.
    pub source: [u8; 6],
}

impl Frame {
    /// A frame from the master, broadcast.
    pub fn new(datagrams: Vec<Datagram>) -> Frame {
        Frame {
            datagrams,
            destination: BROADCAST_MAC,
            source: MASTER_MAC,
        }
    }

    /// The Ethernet frame. `more` is set here, from each datagram's position, because a caller
    /// that set it by hand would eventually set it on the last one.
    pub fn pack(&self) -> Result<Vec<u8>, FrameError> {
        if self.datagrams.is_empty() {
            return Err(FrameError(
                "an EtherCAT frame with no datagram in it carries nothing.".into(),
            ));
        }
        let mut body = Vec::new();
        let count = self.datagrams.len();
        for (index, datagram) in self.datagrams.iter().enumerate() {
            let mut datagram = datagram.clone();
            datagram.more = index + 1 < count;
            datagram.pack_into(&mut body)?;
        }
        if body.len() > 0x7FF {
            return Err(FrameError(format!(
                "the EtherCAT header's length field is eleven bits and these datagrams are {} \
                 bytes.",
                body.len()
            )));
        }
        let header = (body.len() as u16 & 0x7FF) | (FRAME_TYPE_DATAGRAMS << 12);
        let mut out = Vec::with_capacity(16 + body.len().max(MIN_ETHERNET_PAYLOAD));
        out.extend_from_slice(&self.destination);
        out.extend_from_slice(&self.source);
        out.extend_from_slice(&ETHERTYPE.to_be_bytes());
        out.extend_from_slice(&header.to_le_bytes());
        out.extend_from_slice(&body);
        let payload = out.len() - 14;
        if payload < MIN_ETHERNET_PAYLOAD {
            out.resize(14 + MIN_ETHERNET_PAYLOAD, 0);
        }
        Ok(out)
    }

    /// A frame off the wire, read by EtherCAT's own length rather than the Ethernet frame's.
    pub fn unpack(raw: &[u8]) -> Result<Frame, FrameError> {
        if raw.len() < 16 {
            return Err(FrameError(format!(
                "an Ethernet frame is at least sixteen bytes here and this is {}.",
                raw.len()
            )));
        }
        let ethertype = u16::from_be_bytes([raw[12], raw[13]]);
        if ethertype != ETHERTYPE {
            return Err(FrameError(format!(
                "EtherType 0x{ethertype:04X} is not EtherCAT's 0x{ETHERTYPE:04X}. A master that \
                 decoded this would be reading somebody else's traffic as process data."
            )));
        }
        let header = u16::from_le_bytes([raw[14], raw[15]]);
        let length = (header & 0x7FF) as usize;
        let frame_type = (header >> 12) & 0xF;
        if frame_type != FRAME_TYPE_DATAGRAMS {
            return Err(FrameError(format!(
                "EtherCAT frame type {frame_type} is not 1 (datagrams); this driver speaks no \
                 other."
            )));
        }
        let end = 16 + length;
        if raw.len() < end {
            return Err(FrameError(format!(
                "the EtherCAT header declares {length} bytes of datagrams and only {} follow.",
                raw.len() - 16
            )));
        }
        let mut datagrams = Vec::new();
        let mut offset = 16;
        while offset < end {
            let (datagram, next) = Datagram::unpack(&raw[..end], offset)?;
            offset = next;
            let more = datagram.more;
            datagrams.push(datagram);
            if !more {
                break;
            }
        }
        let mut destination = [0u8; 6];
        let mut source = [0u8; 6];
        destination.copy_from_slice(&raw[0..6]);
        source.copy_from_slice(&raw[6..12]);
        Ok(Frame {
            datagrams,
            destination,
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_refuses_what_could_not_have_been_sent() {
        assert!(Frame::new(Vec::new()).pack().is_err(), "no datagram");
        let big = Datagram::new(LRW, 1, (0, 0), vec![0; MAX_DATAGRAM_DATA + 1]);
        assert!(
            Frame::new(vec![big]).pack().is_err(),
            "an eleven-bit length overflowed"
        );
    }

    #[test]
    fn padding_is_not_read_as_datagrams() {
        let frame = Frame::new(vec![Datagram::new(BRD, 7, (0, 0x130), vec![0, 0])]);
        let raw = frame.pack().unwrap();
        assert_eq!(raw.len(), 14 + MIN_ETHERNET_PAYLOAD);
        assert_eq!(Frame::unpack(&raw).unwrap(), frame);
    }

    #[test]
    fn somebody_else_s_traffic_is_refused() {
        let mut raw = Frame::new(vec![Datagram::new(BRD, 1, (0, 0), vec![0])])
            .pack()
            .unwrap();
        raw[12] = 0x08;
        raw[13] = 0x00;
        assert!(Frame::unpack(&raw).is_err());
    }
}

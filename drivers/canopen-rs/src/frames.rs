//! CANopen on the wire — a Rust port of `drivers/canopen/skynet_canopen/frames.py`, which is the
//! executable specification.
//!
//! Pure: every function is bytes in, bytes out, and nothing here opens a socket or reads a clock.
//! `tests/vectors.rs` holds each one to golden frames generated from `frames.py` itself, so the
//! framing is the Python driver's byte for byte rather than a second reading of CiA 301.
//!
//! Only the **master's** half is here. The Python module also packs the drive's replies, because
//! its simulated drive lives beside it; this port is a master, so it packs requests and parses
//! replies, and the vectors carry the drive-side frames as bytes the Python module produced.
//!
//! # The identifiers
//!
//! CiA 301's default allocation gives every service a base identifier and adds the node id:
//!
//! | Service | COB-ID | Direction |
//! |---|---|---|
//! | NMT | `0x000` | master → all |
//! | SYNC | `0x080` | master → all |
//! | EMCY | `0x080 + node` | drive → master |
//! | TPDO2 | `0x280 + node` | drive → master |
//! | RPDO2 | `0x300 + node` | master → drive |
//! | SDO response | `0x580 + node` | drive → master |
//! | SDO request | `0x600 + node` | master → drive |
//! | Heartbeat | `0x700 + node` | drive → master |

use std::fmt;

/// NMT, master to every node.
pub const NMT_ID: u16 = 0x000;
/// SYNC, master to every node. EMCY shares the base and never collides: node 0 is not a node.
pub const SYNC_ID: u16 = 0x080;

/// NMT *start remote node*: the node's PDOs flow.
pub const NMT_START: u8 = 0x01;
/// NMT *enter pre-operational*: PDOs stop, SDO still answers.
pub const NMT_PRE_OPERATIONAL: u8 = 0x80;

/// EMCY from `node`.
pub fn emcy_id(node: u8) -> u16 {
    0x080 + node as u16
}

/// TPDO2 from `node`: statusword and position under CiA 402's default mapping.
pub fn tpdo2_id(node: u8) -> u16 {
    0x280 + node as u16
}

/// RPDO2 to `node`: controlword and target position under CiA 402's default mapping.
pub fn rpdo2_id(node: u8) -> u16 {
    0x300 + node as u16
}

/// An SDO reply from `node`.
pub fn sdo_response_id(node: u8) -> u16 {
    0x580 + node as u16
}

/// An SDO request to `node`.
pub fn sdo_request_id(node: u8) -> u16 {
    0x600 + node as u16
}

/// A heartbeat from `node`.
pub fn heartbeat_id(node: u8) -> u16 {
    0x700 + node as u16
}

/// One classic CAN frame: an 11-bit identifier and up to eight bytes.
///
/// Fixed-size and `Copy`, so the stop path can build one on the stack with no allocation. `len`
/// is carried rather than padded away, for `frames.py`'s reason: a drive that answers a two-byte
/// object with two bytes is answering correctly, and padding would make a short reply
/// indistinguishable from a long one ending in zeros.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frame {
    /// The 11-bit identifier.
    pub id: u16,
    /// How many of `data`'s bytes are the frame's.
    pub len: u8,
    /// The payload, zero past `len`.
    pub data: [u8; 8],
}

impl Frame {
    /// A frame from an identifier and a payload. `None` for an identifier wider than 11 bits or a
    /// payload longer than eight bytes — the two things `frames.py`'s constructor raises on.
    pub fn new(id: u16, payload: &[u8]) -> Option<Frame> {
        if id > 0x7FF || payload.len() > 8 {
            return None;
        }
        let mut data = [0u8; 8];
        data[..payload.len()].copy_from_slice(payload);
        Some(Frame {
            id,
            len: payload.len() as u8,
            data,
        })
    }

    /// The frame's own bytes, without the padding.
    pub fn payload(&self) -> &[u8] {
        &self.data[..self.len as usize]
    }
}

/// An NMT command to `node`, or to every node for node 0.
pub fn nmt(command: u8, node: u8) -> Frame {
    Frame {
        id: NMT_ID,
        len: 2,
        data: [command, node, 0, 0, 0, 0, 0, 0],
    }
}

/// A SYNC: no payload, and every drive samples its synchronous PDOs on it.
pub fn sync() -> Frame {
    Frame {
        id: SYNC_ID,
        len: 0,
        data: [0; 8],
    }
}

// ── Expedited SDO ─────────────────────────────────────────────────────────────
//
// Byte 0 is the command specifier, bytes 1-2 the index little-endian, byte 3 the subindex, bytes
// 4-7 the value little-endian. Segmented transfer is deliberately absent, as in `frames.py`: every
// object this driver touches fits in four bytes.

const UPLOAD_REQUEST: u8 = 0x40;
const DOWNLOAD_RESPONSE: u8 = 0x60;
const ABORT: u8 = 0x80;

/// CiA 301's "object does not exist in the object dictionary".
pub const ABORT_NO_SUCH_OBJECT: u32 = 0x0602_0000;
/// CiA 301's "sub-index does not exist".
pub const ABORT_NO_SUCH_SUBINDEX: u32 = 0x0609_0011;
/// CiA 301's "attempt to write a read-only object".
pub const ABORT_READ_ONLY: u32 = 0x0601_0002;
/// CiA 301's "data type does not match, length of service parameter does not match".
pub const ABORT_LENGTH: u32 = 0x0607_0010;

fn download_specifier(size: u8) -> Option<u8> {
    match size {
        1 => Some(0x2F),
        2 => Some(0x2B),
        3 => Some(0x27),
        4 => Some(0x23),
        _ => None,
    }
}

fn upload_size(specifier: u8) -> Option<usize> {
    match specifier {
        0x4F => Some(1),
        0x4B => Some(2),
        0x47 => Some(3),
        0x43 => Some(4),
        _ => None,
    }
}

/// Read an object.
pub fn sdo_upload_request(node: u8, index: u16, subindex: u8) -> Frame {
    let [low, high] = index.to_le_bytes();
    Frame {
        id: sdo_request_id(node),
        len: 8,
        data: [UPLOAD_REQUEST, low, high, subindex, 0, 0, 0, 0],
    }
}

/// Write an object, `size` bytes wide, with `value` two's-complement if negative. `None` for a
/// size expedited SDO cannot carry — `frames.py` raises there.
pub fn sdo_download_request(
    node: u8,
    index: u16,
    subindex: u8,
    value: i64,
    size: u8,
) -> Option<Frame> {
    let specifier = download_specifier(size)?;
    let [low, high] = index.to_le_bytes();
    let raw = (value as u64).to_le_bytes();
    let mut data = [specifier, low, high, subindex, 0, 0, 0, 0];
    data[4..4 + size as usize].copy_from_slice(&raw[..size as usize]);
    Some(Frame {
        id: sdo_request_id(node),
        len: 8,
        data,
    })
}

/// Why an SDO reply was not the answer to the question asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdoError {
    /// The drive refused, with CiA 301's own code.
    Abort {
        /// The object the drive named.
        index: u16,
        /// Its subindex.
        subindex: u8,
        /// The abort code.
        code: u32,
    },
    /// The reply names a different object. On a bus where an earlier transfer timed out, this is
    /// the stale answer to the previous question — and here that number becomes an encoder
    /// resolution, so it is an error rather than a value.
    WrongObject {
        /// What was asked about.
        asked: (u16, u8),
        /// What the drive answered about.
        answered: (u16, u8),
    },
    /// Not eight bytes, or a command specifier that is not an expedited response.
    Malformed(&'static str),
}

impl SdoError {
    /// The abort in an integrator's terms, byte-identical with `SdoAbort.explain()`.
    pub fn explain(&self) -> String {
        match self {
            SdoError::Abort {
                index,
                subindex,
                code,
            } => {
                let reason = match *code {
                    ABORT_NO_SUCH_OBJECT => "the drive has no such object",
                    ABORT_NO_SUCH_SUBINDEX => "the object exists and has no such subindex",
                    ABORT_READ_ONLY => "the object is read-only",
                    ABORT_LENGTH => "the value's length does not match the object's",
                    _ => "the drive did not say which of CiA 301's reasons applies",
                };
                format!("{index:#06x}:{subindex:02x} was refused with {code:#010x} — {reason}.")
            }
            other => other.to_string(),
        }
    }
}

impl fmt::Display for SdoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SdoError::Abort { .. } => f.write_str(&self.explain()),
            SdoError::WrongObject { asked, answered } => write!(
                f,
                "asked about {:#06x}:{:02x} and the drive answered about {:#06x}:{:02x}",
                asked.0, asked.1, answered.0, answered.1
            ),
            SdoError::Malformed(why) => f.write_str(why),
        }
    }
}

/// The value a drive returned — unsigned, as the wire carries it — or `None` for a download
/// acknowledgement. Mirrors `parse_sdo_response`, refusals included.
pub fn parse_sdo_response(data: &[u8], index: u16, subindex: u8) -> Result<Option<u32>, SdoError> {
    if data.len() != 8 {
        return Err(SdoError::Malformed("an SDO frame is eight bytes"));
    }
    let command = data[0];
    let got_index = u16::from_le_bytes([data[1], data[2]]);
    let got_sub = data[3];
    if command == ABORT {
        return Err(SdoError::Abort {
            index: got_index,
            subindex: got_sub,
            code: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
        });
    }
    if (got_index, got_sub) != (index, subindex) {
        return Err(SdoError::WrongObject {
            asked: (index, subindex),
            answered: (got_index, got_sub),
        });
    }
    if command == DOWNLOAD_RESPONSE {
        return Ok(None);
    }
    let Some(size) = upload_size(command) else {
        return Err(SdoError::Malformed(
            "the command specifier is not an expedited SDO response",
        ));
    };
    let mut raw = [0u8; 4];
    raw[..size].copy_from_slice(&data[4..4 + size]);
    Ok(Some(u32::from_le_bytes(raw)))
}

// ── PDO ──────────────────────────────────────────────────────────────────────
//
// CiA 402's default mapping, which the bridge verifies by SDO before it commands anything:
//
//   RPDO2 (0x300 + node)   controlword u16, target i32
//   TPDO2 (0x280 + node)   statusword u16, position i32

/// A controlword and a target position, to `node`.
pub fn rpdo2(node: u8, controlword: u16, target: i32) -> Frame {
    let [c0, c1] = controlword.to_le_bytes();
    let [t0, t1, t2, t3] = target.to_le_bytes();
    Frame {
        id: rpdo2_id(node),
        len: 6,
        data: [c0, c1, t0, t1, t2, t3, 0, 0],
    }
}

/// A drive's statusword and position, or `None` for a frame that is not six bytes — which under
/// the default mapping is a drive speaking some other mapping.
pub fn parse_tpdo2(data: &[u8]) -> Option<(u16, i32)> {
    if data.len() != 6 {
        return None;
    }
    Some((
        u16::from_le_bytes([data[0], data[1]]),
        i32::from_le_bytes([data[2], data[3], data[4], data[5]]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_refuses_what_classic_can_cannot_carry() {
        assert!(Frame::new(0x800, &[]).is_none(), "a 12-bit identifier");
        assert!(Frame::new(0x100, &[0; 9]).is_none(), "nine bytes");
        let frame = Frame::new(0x7FF, &[1, 2]).expect("the widest id and a short payload");
        assert_eq!(frame.payload(), &[1, 2]);
    }

    #[test]
    fn expedited_sdo_carries_one_to_four_bytes() {
        assert!(sdo_download_request(1, 0x6040, 0, 0, 0).is_none());
        assert!(sdo_download_request(1, 0x6040, 0, 0, 5).is_none());
        assert!(sdo_download_request(1, 0x6040, 0, 0, 3).is_some());
    }

    #[test]
    fn a_short_or_wrong_reply_is_not_a_value() {
        assert!(matches!(
            parse_sdo_response(&[0x43, 0x41, 0x60, 0, 0, 0], 0x6041, 0),
            Err(SdoError::Malformed(_))
        ));
        assert!(matches!(
            parse_sdo_response(&[0x99, 0x41, 0x60, 0, 0, 0, 0, 0], 0x6041, 0),
            Err(SdoError::Malformed(_))
        ));
        assert!(parse_tpdo2(&[0; 5]).is_none());
        assert!(parse_tpdo2(&[0; 8]).is_none());
    }
}

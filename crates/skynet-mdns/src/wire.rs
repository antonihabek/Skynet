//! The DNS message format (RFC 1035), and only the parts DNS-SD over mDNS uses.
//!
//! Written rather than pulled in, for the root manifest's reason and for one specific to a parser:
//! every packet this reads arrived unsolicited from an unauthenticated host on the same network as
//! a robot. The bounds are the whole security argument, so they are in one file a reviewer can read
//! end to end.
//!
//! # Name compression, and the loop it invites
//!
//! A DNS name is a run of length-prefixed labels ending in a zero byte, except that a label whose
//! top two bits are set is a **pointer**: fourteen bits of offset into the same message, from which
//! reading continues. Responders use it heavily — every record in a DNS-SD answer repeats the
//! service name — so a decoder that does not implement it cannot read real traffic.
//!
//! It is also the classic denial of service. A pointer at offset 30 whose target is 30 is a decoder
//! that never returns, from twelve bytes of hostile packet, and the tool it hangs is the one an
//! operator opened *because* the network was behaving strangely.
//!
//! Two bounds, and they are belt and braces on purpose:
//!
//! * **a pointer must target an offset strictly before its own** — RFC 1035's own rule, that a
//!   pointer refers to a *prior* occurrence;
//! * **and the number of jumps is capped** at [`MAX_JUMPS`].
//!
//! The first alone very nearly suffices, and "very nearly" is not the standard for the code between
//! a hostile packet and a robot. Reading forward after a jump can leave the cursor past the
//! pointer that produced it, so a proof that offsets strictly decrease needs a case analysis, and a
//! counter needs none. The stricter reading — *each jump must go before the previous target* —
//! would terminate trivially and was rejected: it is stricter than the RFC permits, and a
//! discovery tool that fails against a conformant responder somebody else wrote is worse than one
//! carrying a counter.
//!
//! # Counts in the header are claims, not sizes
//!
//! The twelve-byte header declares how many records follow, and a hostile twelve bytes can declare
//! sixty-five thousand. Nothing here allocates from a declared count: the vectors grow as records
//! are actually read, so an inflated count costs one truncation error rather than a megabyte of
//! empty capacity per packet. It is a small thing and it is the difference between a parser that is
//! merely correct and one that can be pointed at a network.

use std::net::{Ipv4Addr, Ipv6Addr};

/// The largest single label, per RFC 1035.
pub const MAX_LABEL: usize = 63;

/// The largest encoded name, per RFC 1035: labels plus their length bytes plus the terminator.
pub const MAX_NAME: usize = 255;

/// How many compression pointers one name may follow before it is refused.
///
/// A pointer costs two bytes, so a legitimate name cannot usefully chain more than a handful. This
/// is set well above any real responder and well below anything that takes measurable time.
pub const MAX_JUMPS: usize = 64;

/// The largest message this will parse.
///
/// RFC 6762 permits mDNS messages up to 9000 bytes so that a response can carry a full record set
/// without truncation. Anything larger is not a long answer, it is a datagram nobody should be
/// assembling.
pub const MAX_MESSAGE: usize = 9000;

/// `A` — an IPv4 address.
pub const TYPE_A: u16 = 1;
/// `PTR` — the record DNS-SD uses to enumerate instances of a service.
pub const TYPE_PTR: u16 = 12;
/// `TXT` — the record carrying an instance's key/value pairs.
pub const TYPE_TXT: u16 = 16;
/// `AAAA` — an IPv6 address.
pub const TYPE_AAAA: u16 = 28;
/// `SRV` — the record carrying an instance's host and port.
pub const TYPE_SRV: u16 = 33;
/// `ANY` — matches every type, in a question.
pub const TYPE_ANY: u16 = 255;

/// The Internet class.
pub const CLASS_IN: u16 = 1;

/// In a **response**, the top bit of a record's class is RFC 6762's cache-flush bit.
pub const CACHE_FLUSH: u16 = 0x8000;

/// In a **question**, the top bit of the class asks for a unicast reply (RFC 6762 §5.4).
pub const UNICAST_RESPONSE: u16 = 0x8000;

/// The header bit distinguishing a response from a query.
pub const FLAG_RESPONSE: u16 = 0x8000;

/// The header bit an mDNS responder sets: the answer is authoritative.
pub const FLAG_AUTHORITATIVE: u16 = 0x0400;

/// Why a message could not be read.
///
/// Every one of these discards the whole message. There is no partial parse, because a record set
/// missing the records that did not fit is a discovery answer that is wrong rather than short.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The message ended in the middle of something.
    Truncated,
    /// A message larger than [`MAX_MESSAGE`].
    TooLarge {
        /// How many bytes arrived.
        len: usize,
    },
    /// A compression pointer targeting its own offset or a later one.
    PointerNotBackwards {
        /// Where the pointer was.
        at: usize,
        /// Where it pointed.
        to: usize,
    },
    /// More than [`MAX_JUMPS`] compression pointers in one name.
    TooManyJumps,
    /// A name whose encoded length exceeds [`MAX_NAME`].
    NameTooLong,
    /// A label using one of the two reserved length prefixes (`0b01`, `0b10`).
    ReservedLabel {
        /// The byte found.
        byte: u8,
    },
    /// A label longer than [`MAX_LABEL`], on encode.
    LabelTooLong {
        /// How long it was.
        len: usize,
    },
    /// A record whose `RDLENGTH` disagrees with the data that follows it.
    BadRecordLength,
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Truncated => write!(f, "the message ended mid-record"),
            Self::TooLarge { len } => {
                write!(f, "{len} bytes is larger than the {MAX_MESSAGE}-byte limit")
            }
            Self::PointerNotBackwards { at, to } => {
                write!(
                    f,
                    "a compression pointer at {at} targets {to}, which is not earlier"
                )
            }
            Self::TooManyJumps => {
                write!(f, "more than {MAX_JUMPS} compression pointers in one name")
            }
            Self::NameTooLong => write!(f, "a name longer than {MAX_NAME} bytes"),
            Self::ReservedLabel { byte } => write!(f, "a label prefix of {byte:#04x} is reserved"),
            Self::LabelTooLong { len } => write!(f, "a {len}-byte label exceeds {MAX_LABEL}"),
            Self::BadRecordLength => {
                write!(f, "a record's declared length does not match its data")
            }
        }
    }
}

impl std::error::Error for WireError {}

/// A domain name, as its labels.
///
/// Labels are bytes rather than `String`s. DNS-SD instance names are UTF-8 (RFC 6763 §4.1.1) and
/// the ones this crate constructs are checked, but a name arriving from the network is not this
/// crate's to validate: refusing a whole packet because some unrelated host on the LAN has a
/// hostname with a stray byte in it would discard the robots in the same response.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Name(pub Vec<Vec<u8>>);

impl Name {
    /// Split a dotted name into labels. Does not escape `\.`, which no name here contains.
    pub fn parse(text: &str) -> Self {
        Self(
            text.split('.')
                .filter(|label| !label.is_empty())
                .map(|label| label.as_bytes().to_vec())
                .collect(),
        )
    }

    /// The name as text, with any non-UTF-8 label lossily converted.
    ///
    /// For display and for logs. Comparison uses [`Name::eq_ignore_case`], which works on the
    /// bytes, so a lossy replacement can never make two different names look equal.
    pub fn to_text(&self) -> String {
        self.0
            .iter()
            .map(|label| String::from_utf8_lossy(label).into_owned())
            .collect::<Vec<_>>()
            .join(".")
    }

    /// DNS comparison: label by label, ASCII case-insensitive.
    ///
    /// Case-insensitivity is ASCII-only and deliberately not Unicode-aware. RFC 4343 defines DNS
    /// case-insensitivity over the ASCII range and nothing else, and a decoder that folded UTF-8
    /// here would make two distinct instance names compare equal — which for a service that
    /// enumerates *robots* is two machines rendered as one.
    pub fn eq_ignore_case(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self
                .0
                .iter()
                .zip(&other.0)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
    }

    /// Whether this name ends in `suffix` — how a PTR answer is matched to the service asked about.
    pub fn ends_with(&self, suffix: &Self) -> bool {
        if suffix.0.len() > self.0.len() {
            return false;
        }
        let start = self.0.len() - suffix.0.len();
        self.0[start..]
            .iter()
            .zip(&suffix.0)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    }

    /// The first label — for a DNS-SD instance name, the human-readable part.
    pub fn first_label(&self) -> Option<&[u8]> {
        self.0.first().map(Vec::as_slice)
    }

    /// Append the encoded name to `out`, without compression.
    ///
    /// Compression is legal to omit and it is omitted: it saves bytes in a datagram that is already
    /// far below the MTU, and it is the single most error-prone part of a DNS encoder. Nothing this
    /// crate sends is large enough for the trade to be worth making.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<(), WireError> {
        let mut total = 1;
        for label in &self.0 {
            if label.len() > MAX_LABEL {
                return Err(WireError::LabelTooLong { len: label.len() });
            }
            total += label.len() + 1;
            if total > MAX_NAME {
                return Err(WireError::NameTooLong);
            }
            out.push(label.len() as u8);
            out.extend_from_slice(label);
        }
        out.push(0);
        Ok(())
    }
}

impl std::fmt::Display for Name {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_text())
    }
}

/// A question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    /// What is being asked about.
    pub name: Name,
    /// Which record type.
    pub qtype: u16,
    /// Whether a unicast reply was requested (RFC 6762 §5.4).
    pub unicast_response: bool,
}

/// A resource record's payload, decoded for the types DNS-SD uses and kept raw otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordData {
    /// An IPv4 address.
    A(Ipv4Addr),
    /// An IPv6 address.
    Aaaa(Ipv6Addr),
    /// A pointer to a service instance name.
    Ptr(Name),
    /// Host and port for an instance.
    Srv {
        /// Priority; unused by DNS-SD in practice.
        priority: u16,
        /// Weight; unused by DNS-SD in practice.
        weight: u16,
        /// The port the service listens on.
        port: u16,
        /// The host name to resolve.
        target: Name,
    },
    /// The raw TXT payload, still in RFC 6763's length-prefixed form. See [`crate::txt`].
    Txt(Vec<u8>),
    /// Any other type, kept whole.
    ///
    /// Not an error: a response carrying an `NSEC` this crate has no use for is a perfectly good
    /// response, and discarding the message over it would discard the robot it was about.
    Other {
        /// The record type.
        rtype: u16,
        /// Its bytes.
        data: Vec<u8>,
    },
}

/// A resource record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// What the record is about.
    pub name: Name,
    /// Its class, with [`CACHE_FLUSH`] masked off.
    pub class: u16,
    /// Whether the cache-flush bit was set.
    pub cache_flush: bool,
    /// Seconds this record may be considered valid.
    ///
    /// A TTL of zero is RFC 6762's *goodbye*: the responder is withdrawing the record. It is
    /// carried through rather than filtered, because "this robot has just left the network" is one
    /// of the more useful things a discovery tool can be told.
    pub ttl: u32,
    /// The payload.
    pub data: RecordData,
}

/// A decoded message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Message {
    /// The transaction id. Zero in mDNS.
    pub id: u16,
    /// The raw header flags.
    pub flags: u16,
    /// Questions asked.
    pub questions: Vec<Question>,
    /// The answer section.
    pub answers: Vec<Record>,
    /// The authority section — RFC 6762 puts probe tie-breaking records here.
    pub authorities: Vec<Record>,
    /// The additional section, where a responder puts the SRV, TXT and address records that save
    /// the asker three more round trips.
    pub additionals: Vec<Record>,
}

impl Message {
    /// Whether this is a response.
    pub fn is_response(&self) -> bool {
        self.flags & FLAG_RESPONSE != 0
    }

    /// Every record in the message, in section order.
    ///
    /// DNS-SD answers scatter an instance's records across the answer and additional sections at
    /// the responder's discretion, so a consumer that reads only `answers` works against some
    /// implementations and not others. Nothing in this crate distinguishes them.
    pub fn records(&self) -> impl Iterator<Item = &Record> {
        self.answers
            .iter()
            .chain(&self.authorities)
            .chain(&self.additionals)
    }

    /// Encode, without name compression.
    pub fn encode(&self) -> Result<Vec<u8>, WireError> {
        let mut out = Vec::with_capacity(64);
        out.extend_from_slice(&self.id.to_be_bytes());
        out.extend_from_slice(&self.flags.to_be_bytes());
        out.extend_from_slice(&(self.questions.len() as u16).to_be_bytes());
        out.extend_from_slice(&(self.answers.len() as u16).to_be_bytes());
        out.extend_from_slice(&(self.authorities.len() as u16).to_be_bytes());
        out.extend_from_slice(&(self.additionals.len() as u16).to_be_bytes());

        for question in &self.questions {
            question.name.encode(&mut out)?;
            out.extend_from_slice(&question.qtype.to_be_bytes());
            let class = if question.unicast_response {
                CLASS_IN | UNICAST_RESPONSE
            } else {
                CLASS_IN
            };
            out.extend_from_slice(&class.to_be_bytes());
        }

        for record in self
            .answers
            .iter()
            .chain(&self.authorities)
            .chain(&self.additionals)
        {
            encode_record(record, &mut out)?;
        }

        if out.len() > MAX_MESSAGE {
            return Err(WireError::TooLarge { len: out.len() });
        }
        Ok(out)
    }

    /// Decode a datagram.
    pub fn decode(buf: &[u8]) -> Result<Self, WireError> {
        if buf.len() > MAX_MESSAGE {
            return Err(WireError::TooLarge { len: buf.len() });
        }
        if buf.len() < 12 {
            return Err(WireError::Truncated);
        }

        let id = u16::from_be_bytes([buf[0], buf[1]]);
        let flags = u16::from_be_bytes([buf[2], buf[3]]);
        let counts = [
            u16::from_be_bytes([buf[4], buf[5]]) as usize,
            u16::from_be_bytes([buf[6], buf[7]]) as usize,
            u16::from_be_bytes([buf[8], buf[9]]) as usize,
            u16::from_be_bytes([buf[10], buf[11]]) as usize,
        ];

        // Nothing below reserves capacity from these. See the header.
        let mut at = 12;
        let mut questions = Vec::new();
        for _ in 0..counts[0] {
            let (name, next) = read_name(buf, at)?;
            at = next;
            let qtype = read_u16(buf, at)?;
            let class = read_u16(buf, at + 2)?;
            at += 4;
            questions.push(Question {
                name,
                qtype,
                unicast_response: class & UNICAST_RESPONSE != 0,
            });
        }

        let mut sections: [Vec<Record>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        for (section, count) in sections.iter_mut().zip(&counts[1..]) {
            for _ in 0..*count {
                let (record, next) = read_record(buf, at)?;
                at = next;
                section.push(record);
            }
        }
        let [answers, authorities, additionals] = sections;

        Ok(Self {
            id,
            flags,
            questions,
            answers,
            authorities,
            additionals,
        })
    }
}

fn encode_record(record: &Record, out: &mut Vec<u8>) -> Result<(), WireError> {
    record.name.encode(out)?;
    let (rtype, payload) = match &record.data {
        RecordData::A(address) => (TYPE_A, address.octets().to_vec()),
        RecordData::Aaaa(address) => (TYPE_AAAA, address.octets().to_vec()),
        RecordData::Ptr(name) => {
            let mut bytes = Vec::new();
            name.encode(&mut bytes)?;
            (TYPE_PTR, bytes)
        }
        RecordData::Srv {
            priority,
            weight,
            port,
            target,
        } => {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&priority.to_be_bytes());
            bytes.extend_from_slice(&weight.to_be_bytes());
            bytes.extend_from_slice(&port.to_be_bytes());
            target.encode(&mut bytes)?;
            (TYPE_SRV, bytes)
        }
        RecordData::Txt(bytes) => (TYPE_TXT, bytes.clone()),
        RecordData::Other { rtype, data } => (*rtype, data.clone()),
    };

    out.extend_from_slice(&rtype.to_be_bytes());
    let class = if record.cache_flush {
        record.class | CACHE_FLUSH
    } else {
        record.class
    };
    out.extend_from_slice(&class.to_be_bytes());
    out.extend_from_slice(&record.ttl.to_be_bytes());
    if payload.len() > u16::MAX as usize {
        return Err(WireError::BadRecordLength);
    }
    out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(())
}

fn read_u16(buf: &[u8], at: usize) -> Result<u16, WireError> {
    let bytes = buf.get(at..at + 2).ok_or(WireError::Truncated)?;
    Ok(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(buf: &[u8], at: usize) -> Result<u32, WireError> {
    let bytes = buf.get(at..at + 4).ok_or(WireError::Truncated)?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// Read a name, following compression pointers.
///
/// Returns the name and the offset **after** it in the message — which for a name containing a
/// pointer is the offset after the *pointer*, not after whatever it targeted. Getting that wrong
/// walks the parser into the middle of an earlier record and produces garbage that decodes.
fn read_name(buf: &[u8], start: usize) -> Result<(Name, usize), WireError> {
    let mut labels = Vec::new();
    let mut at = start;
    let mut total = 1;
    let mut jumps = 0;
    let mut after: Option<usize> = None;

    loop {
        let byte = *buf.get(at).ok_or(WireError::Truncated)?;
        match byte & 0xC0 {
            0x00 => {
                if byte == 0 {
                    return Ok((Name(labels), after.unwrap_or(at + 1)));
                }
                let len = byte as usize;
                let bytes = buf.get(at + 1..at + 1 + len).ok_or(WireError::Truncated)?;
                total += len + 1;
                if total > MAX_NAME {
                    return Err(WireError::NameTooLong);
                }
                labels.push(bytes.to_vec());
                at += 1 + len;
            }
            0xC0 => {
                let low = *buf.get(at + 1).ok_or(WireError::Truncated)? as usize;
                let target = (((byte & 0x3F) as usize) << 8) | low;
                if target >= at {
                    return Err(WireError::PointerNotBackwards { at, to: target });
                }
                jumps += 1;
                if jumps > MAX_JUMPS {
                    return Err(WireError::TooManyJumps);
                }
                // Only the first pointer decides where the caller continues from.
                if after.is_none() {
                    after = Some(at + 2);
                }
                at = target;
            }
            other => return Err(WireError::ReservedLabel { byte: other }),
        }
    }
}

fn read_record(buf: &[u8], at: usize) -> Result<(Record, usize), WireError> {
    let (name, at) = read_name(buf, at)?;
    let rtype = read_u16(buf, at)?;
    let class = read_u16(buf, at + 2)?;
    let ttl = read_u32(buf, at + 4)?;
    let len = read_u16(buf, at + 8)? as usize;
    let start = at + 10;
    let end = start + len;
    let payload = buf.get(start..end).ok_or(WireError::Truncated)?;

    let data = match rtype {
        TYPE_A if len == 4 => RecordData::A(Ipv4Addr::new(
            payload[0], payload[1], payload[2], payload[3],
        )),
        TYPE_AAAA if len == 16 => {
            let mut octets = [0u8; 16];
            octets.copy_from_slice(payload);
            RecordData::Aaaa(Ipv6Addr::from(octets))
        }
        // A pointer inside a PTR or SRV may target anywhere earlier in the *message*, so these are
        // read against the whole buffer and then checked to have consumed exactly their record.
        TYPE_PTR => {
            let (target, consumed) = read_name(buf, start)?;
            if consumed != end {
                return Err(WireError::BadRecordLength);
            }
            RecordData::Ptr(target)
        }
        TYPE_SRV if len >= 7 => {
            let priority = read_u16(buf, start)?;
            let weight = read_u16(buf, start + 2)?;
            let port = read_u16(buf, start + 4)?;
            let (target, consumed) = read_name(buf, start + 6)?;
            if consumed != end {
                return Err(WireError::BadRecordLength);
            }
            RecordData::Srv {
                priority,
                weight,
                port,
                target,
            }
        }
        TYPE_TXT => RecordData::Txt(payload.to_vec()),
        rtype => RecordData::Other {
            rtype,
            data: payload.to_vec(),
        },
    };

    Ok((
        Record {
            name,
            class: class & !CACHE_FLUSH,
            cache_flush: class & CACHE_FLUSH != 0,
            ttl,
            data,
        },
        end,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ptr_message() -> Message {
        Message {
            id: 0,
            flags: FLAG_RESPONSE | FLAG_AUTHORITATIVE,
            questions: Vec::new(),
            answers: vec![Record {
                name: Name::parse("_skynet._tcp.local"),
                class: CLASS_IN,
                cache_flush: false,
                ttl: 120,
                data: RecordData::Ptr(Name::parse("arm-01._skynet._tcp.local")),
            }],
            authorities: Vec::new(),
            additionals: Vec::new(),
        }
    }

    #[test]
    fn a_message_survives_a_round_trip() {
        let message = ptr_message();
        let decoded = Message::decode(&message.encode().unwrap()).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn every_record_type_this_crate_uses_survives_a_round_trip() {
        let mut message = ptr_message();
        message.additionals = vec![
            Record {
                name: Name::parse("arm-01._skynet._tcp.local"),
                class: CLASS_IN,
                cache_flush: true,
                ttl: 120,
                data: RecordData::Srv {
                    priority: 0,
                    weight: 0,
                    port: 8099,
                    target: Name::parse("arm-01.local"),
                },
            },
            Record {
                name: Name::parse("arm-01._skynet._tcp.local"),
                class: CLASS_IN,
                cache_flush: true,
                ttl: 120,
                data: RecordData::Txt(b"\x05id=a1".to_vec()),
            },
            Record {
                name: Name::parse("arm-01.local"),
                class: CLASS_IN,
                cache_flush: true,
                ttl: 120,
                data: RecordData::A(Ipv4Addr::new(169, 254, 3, 4)),
            },
            Record {
                name: Name::parse("arm-01.local"),
                class: CLASS_IN,
                cache_flush: true,
                ttl: 120,
                data: RecordData::Aaaa("fe80::1".parse().unwrap()),
            },
        ];
        let decoded = Message::decode(&message.encode().unwrap()).unwrap();
        assert_eq!(decoded, message);
    }

    #[test]
    fn the_cache_flush_bit_does_not_leak_into_the_class() {
        let mut message = ptr_message();
        message.answers[0].cache_flush = true;
        let decoded = Message::decode(&message.encode().unwrap()).unwrap();
        assert_eq!(decoded.answers[0].class, CLASS_IN);
        assert!(decoded.answers[0].cache_flush);
    }

    #[test]
    fn a_question_carries_the_unicast_bit_without_it_becoming_the_class() {
        let message = Message {
            questions: vec![Question {
                name: Name::parse("_skynet._tcp.local"),
                qtype: TYPE_PTR,
                unicast_response: true,
            }],
            ..Default::default()
        };
        let decoded = Message::decode(&message.encode().unwrap()).unwrap();
        assert_eq!(decoded.questions, message.questions);
    }

    // ── The bounds. Each of these is a packet any host on the LAN can send. ──

    #[test]
    fn a_pointer_to_itself_is_refused_rather_than_followed_for_ever() {
        // Header, then a name at offset 12 that is a pointer to offset 12.
        let mut buf = vec![0u8; 12];
        buf[5] = 1; // one question
        buf.extend_from_slice(&[0xC0, 12]);
        buf.extend_from_slice(&[0, TYPE_PTR as u8, 0, 1]);
        assert_eq!(
            Message::decode(&buf),
            Err(WireError::PointerNotBackwards { at: 12, to: 12 })
        );
    }

    #[test]
    fn a_pointer_to_a_later_offset_is_refused() {
        let mut buf = vec![0u8; 12];
        buf[5] = 1;
        buf.extend_from_slice(&[0xC0, 20]);
        buf.extend_from_slice(&[0, TYPE_PTR as u8, 0, 1]);
        assert_eq!(
            Message::decode(&buf),
            Err(WireError::PointerNotBackwards { at: 12, to: 20 })
        );
    }

    #[test]
    fn a_backwards_pointer_is_followed_because_real_responders_send_them() {
        // The shape every real responder emits: a question carrying "local" in full, then an answer
        // whose name is the labels "arm-01" followed by a pointer back to it. A decoder without
        // compression cannot read this, which is why it is implemented at all.
        let mut buf = vec![0u8; 12];
        buf[5] = 1; // one question
        buf[7] = 1; // one answer

        let local_at = buf.len(); // 12
        buf.extend_from_slice(&[5, b'l', b'o', b'c', b'a', b'l', 0]);
        buf.extend_from_slice(&[0, TYPE_PTR as u8, 0, 1]); // qtype, qclass

        buf.extend_from_slice(&[6, b'a', b'r', b'm', b'-', b'0', b'1']);
        buf.extend_from_slice(&[0xC0, local_at as u8]);
        buf.extend_from_slice(&[0, TYPE_A as u8, 0, 1]);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&4u16.to_be_bytes());
        buf.extend_from_slice(&[169, 254, 3, 4]);

        let decoded = Message::decode(&buf).unwrap();
        assert_eq!(decoded.questions[0].name.to_text(), "local");
        assert_eq!(decoded.answers[0].name.to_text(), "arm-01.local");
        assert_eq!(
            decoded.answers[0].data,
            RecordData::A(Ipv4Addr::new(169, 254, 3, 4))
        );
    }

    #[test]
    fn a_name_that_only_grows_by_jumping_is_capped() {
        // **This is the test that makes MAX_JUMPS load-bearing**, and building it is what shows the
        // strictly-backwards rule is not sufficient on its own.
        //
        // A pointer that targets another pointer directly does decrease monotonically — the cursor
        // becomes the target, so the chain is bounded by the message. Interleaving a *label* with
        // each jump breaks that: reading the label moves the cursor forward again, so the next
        // pointer sits above the one before it and every jump is still strictly backwards.
        //
        // The ladder is built inside a TXT record's RDATA, which the parser walks past by its
        // declared length, and a second record's name enters it at the top.
        const BLOCK: usize = 4;
        let base = 23; // 12 header + 1 root name + 2 type + 2 class + 4 ttl + 2 rdlength
        let blocks = MAX_JUMPS + 1;

        let mut buf = vec![0u8; 12];
        buf[7] = 2; // two answers

        // Record A: a root name and a TXT record whose payload is the ladder.
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&[0, TYPE_TXT as u8, 0, 1]);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&((blocks * BLOCK) as u16).to_be_bytes());
        assert_eq!(
            buf.len(),
            base,
            "the ladder starts where the arithmetic says"
        );

        for k in 0..blocks {
            // Every block is one label and one pointer to the block below it. Block zero ends the
            // name instead, so a decoder that respected only the RFC's rule would terminate here
            // having followed sixty-five pointers.
            buf.push(1);
            buf.push(b'a');
            if k == 0 {
                buf.extend_from_slice(&[0, 0]);
            } else {
                let target = base + (k - 1) * BLOCK;
                buf.extend_from_slice(&[0xC0 | (target >> 8) as u8, target as u8]);
            }
        }

        // Record B: its name enters the ladder at the top block.
        let top = base + (blocks - 1) * BLOCK;
        buf.extend_from_slice(&[0xC0 | (top >> 8) as u8, top as u8]);
        buf.extend_from_slice(&[0, TYPE_A as u8, 0, 1]);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&4u16.to_be_bytes());
        buf.extend_from_slice(&[169, 254, 3, 4]);

        assert_eq!(Message::decode(&buf), Err(WireError::TooManyJumps));
    }

    #[test]
    fn a_jump_chain_just_under_the_cap_is_still_read() {
        // The other half: the cap must not be so tight that it refuses a legal name. Same ladder,
        // one block shorter, and it decodes to the labels it actually spells.
        const BLOCK: usize = 4;
        let base = 23;
        let blocks = MAX_JUMPS - 1;

        let mut buf = vec![0u8; 12];
        buf[7] = 2;
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&[0, TYPE_TXT as u8, 0, 1]);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&((blocks * BLOCK) as u16).to_be_bytes());

        for k in 0..blocks {
            buf.push(1);
            buf.push(b'a');
            if k == 0 {
                buf.extend_from_slice(&[0, 0]);
            } else {
                let target = base + (k - 1) * BLOCK;
                buf.extend_from_slice(&[0xC0 | (target >> 8) as u8, target as u8]);
            }
        }

        let top = base + (blocks - 1) * BLOCK;
        buf.extend_from_slice(&[0xC0 | (top >> 8) as u8, top as u8]);
        buf.extend_from_slice(&[0, TYPE_A as u8, 0, 1]);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&4u16.to_be_bytes());
        buf.extend_from_slice(&[169, 254, 3, 4]);

        let decoded = Message::decode(&buf).unwrap();
        assert_eq!(decoded.answers[1].name.0.len(), blocks);
    }

    #[test]
    fn a_header_claiming_records_that_are_not_there_is_truncated_not_trusted() {
        let mut buf = vec![0u8; 12];
        buf[6] = 0xFF; // 65,535 answers
        buf[7] = 0xFF;
        assert_eq!(Message::decode(&buf), Err(WireError::Truncated));
    }

    #[test]
    fn a_message_shorter_than_a_header_is_refused() {
        assert_eq!(Message::decode(&[0u8; 11]), Err(WireError::Truncated));
    }

    #[test]
    fn a_datagram_over_the_limit_is_refused_before_it_is_walked() {
        let buf = vec![0u8; MAX_MESSAGE + 1];
        assert_eq!(
            Message::decode(&buf),
            Err(WireError::TooLarge {
                len: MAX_MESSAGE + 1
            })
        );
    }

    #[test]
    fn a_reserved_label_prefix_is_refused() {
        let mut buf = vec![0u8; 12];
        buf[5] = 1;
        buf.extend_from_slice(&[0x80]); // 0b10 — reserved
        assert_eq!(
            Message::decode(&buf),
            Err(WireError::ReservedLabel { byte: 0x80 })
        );
    }

    #[test]
    fn a_ptr_whose_name_runs_past_its_declared_length_is_refused() {
        // RDLENGTH says 2, but the name inside is longer. Without the check this walks the parser
        // into the next record with a name it half-read.
        let mut buf = vec![0u8; 12];
        buf[7] = 1;
        buf.extend_from_slice(&[0]); // root name
        buf.extend_from_slice(&[0, TYPE_PTR as u8, 0, 1]);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&2u16.to_be_bytes());
        buf.extend_from_slice(&[6, b'a', b'r', b'm', b'-', b'0', b'1', 0]);
        assert_eq!(Message::decode(&buf), Err(WireError::BadRecordLength));
    }

    #[test]
    fn an_encoded_label_over_63_bytes_is_refused() {
        let name = Name(vec![vec![b'a'; 64]]);
        assert_eq!(
            name.encode(&mut Vec::new()),
            Err(WireError::LabelTooLong { len: 64 })
        );
    }

    #[test]
    fn an_encoded_name_over_255_bytes_is_refused() {
        let name = Name(vec![vec![b'a'; 63]; 5]);
        assert_eq!(name.encode(&mut Vec::new()), Err(WireError::NameTooLong));
    }

    // ── Comparison ──

    #[test]
    fn names_compare_without_ascii_case_and_with_everything_else() {
        assert!(Name::parse("Arm-01.LOCAL").eq_ignore_case(&Name::parse("arm-01.local")));
        assert!(!Name::parse("arm-01.local").eq_ignore_case(&Name::parse("arm-02.local")));
        // Two distinct instance names that a Unicode fold would collapse into one robot.
        assert!(!Name::parse("straße.local").eq_ignore_case(&Name::parse("STRASSE.local")));
    }

    #[test]
    fn a_suffix_match_is_how_a_ptr_answer_is_recognised() {
        let service = Name::parse("_skynet._tcp.local");
        assert!(Name::parse("arm-01._skynet._tcp.local").ends_with(&service));
        assert!(!Name::parse("arm-01._printer._tcp.local").ends_with(&service));
        // Not a suffix of something shorter than itself.
        assert!(!service.ends_with(&Name::parse("arm-01._skynet._tcp.local")));
    }

    #[test]
    fn an_unknown_record_type_is_kept_rather_than_failing_the_message() {
        // A responder that includes an NSEC must not cost us the robot in the same packet.
        let mut buf = vec![0u8; 12];
        buf[7] = 1;
        buf.extend_from_slice(&[0]);
        buf.extend_from_slice(&[0, 47, 0, 1]); // NSEC
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&1u16.to_be_bytes());
        buf.extend_from_slice(&[0]);
        let decoded = Message::decode(&buf).unwrap();
        assert!(matches!(
            decoded.answers[0].data,
            RecordData::Other { rtype: 47, .. }
        ));
    }

    #[test]
    fn a_non_utf8_label_is_kept_because_it_may_not_be_the_robots_label() {
        let mut buf = vec![0u8; 12];
        buf[7] = 1;
        buf.extend_from_slice(&[2, 0xFF, 0xFE, 0]);
        buf.extend_from_slice(&[0, TYPE_A as u8, 0, 1]);
        buf.extend_from_slice(&0u32.to_be_bytes());
        buf.extend_from_slice(&4u16.to_be_bytes());
        buf.extend_from_slice(&[169, 254, 3, 4]);
        let decoded = Message::decode(&buf).unwrap();
        assert_eq!(decoded.answers[0].name.0[0], vec![0xFF, 0xFE]);
    }
}

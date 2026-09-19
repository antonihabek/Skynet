//! RTDE on the wire — a Rust port of `drivers/ur_rtde/skynet_ur/rtde.py`, which is the executable
//! specification and was itself measured against URSim, Universal Robots' own controller software.
//!
//! Three bytes of header, **big-endian** throughout (CANopen next door is little-endian):
//!
//! ```text
//!  size : u16   the whole packet, header included
//!  type : u8    one of the constants below
//!  payload
//! ```
//!
//! The two rules the Python module's measurement taught are kept here, in the same places:
//!
//! * **`TEXT_MESSAGE` arrives unsolicited, interleaved with replies**, so [`Session::transact`]
//!   reads until the type it asked for and collects every text message rather than taking the next
//!   packet as its reply — which is off by one for the rest of the session, and every reply parses.
//! * **The output stream is a firehose**, so [`Session::newest`] drains to the last complete
//!   package: for a control loop an old sample is not data (§47.6's latest-wins rule).
//!
//! And the one writing the port found (`DRY-COVERAGE-PLAN.md` F75): nothing is consumed from the
//! buffer until a whole packet is there, so a packet that straddles the moment a non-blocking drain
//! runs dry is completed by the next drain rather than read from the wrong offset for ever.
//!
//! `tests/vectors.rs` drives this module with the bytes the Python session wrote and read against a
//! scripted controller, so every request is the Python driver's byte for byte.

use std::fmt;
use std::io::{self, Read, Write};

/// `REQUEST_PROTOCOL_VERSION`, `V`.
pub const REQUEST_PROTOCOL_VERSION: u8 = 86;
/// `GET_URCONTROL_VERSION`, `v`.
pub const GET_URCONTROL_VERSION: u8 = 118;
/// `TEXT_MESSAGE`, `M` — the controller talking, unsolicited.
pub const TEXT_MESSAGE: u8 = 77;
/// `CONTROL_PACKAGE_SETUP_OUTPUTS`, `O`.
pub const SETUP_OUTPUTS: u8 = 79;
/// `CONTROL_PACKAGE_SETUP_INPUTS`, `I`.
pub const SETUP_INPUTS: u8 = 73;
/// `CONTROL_PACKAGE_START`, `S`.
pub const START: u8 = 83;
/// `CONTROL_PACKAGE_PAUSE`, `P`.
pub const PAUSE: u8 = 80;
/// `DATA_PACKAGE`, `U`.
pub const DATA_PACKAGE: u8 = 85;

/// How many packets `transact` reads past before giving up on its reply: `rtde.py`'s 64.
const TRANSACT_PACKETS: usize = 64;

/// A refusal from the controller, or a frame that is not one.
#[derive(Debug)]
pub enum RtdeError {
    /// The controller refused, or answered in a way this client will not read.
    Refused(String),
    /// The transport failed.
    Io(io::Error),
}

impl fmt::Display for RtdeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RtdeError::Refused(text) => f.write_str(text),
            RtdeError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl From<io::Error> for RtdeError {
    fn from(error: io::Error) -> RtdeError {
        RtdeError::Io(error)
    }
}

fn refused(text: impl Into<String>) -> RtdeError {
    RtdeError::Refused(text.into())
}

/// One RTDE value, of one of the types this client decodes.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// `BOOL`.
    Bool(bool),
    /// `UINT8`.
    U8(u8),
    /// `INT32`.
    I32(i32),
    /// `UINT32`.
    U32(u32),
    /// `UINT64`.
    U64(u64),
    /// `DOUBLE`.
    F64(f64),
    /// `VECTOR3D`.
    Vec3(Vec<f64>),
    /// `VECTOR6D`.
    Vec6(Vec<f64>),
    /// `VECTOR6INT32`.
    Vec6I32(Vec<i32>),
    /// `VECTOR6UINT32`.
    Vec6U32(Vec<u32>),
}

/// Bytes one value of `type_name` occupies. `IN_USE` is what the controller answers for a
/// variable it will not give — a refusal wearing a type's clothes, and a client that read it as a
/// width would slide every field after it.
pub fn width_of(type_name: &str) -> Result<usize, RtdeError> {
    Ok(match type_name {
        "BOOL" | "UINT8" => 1,
        "INT32" | "UINT32" => 4,
        "UINT64" | "DOUBLE" => 8,
        "VECTOR3D" | "VECTOR6INT32" | "VECTOR6UINT32" => 24,
        "VECTOR6D" => 48,
        other => {
            return Err(refused(format!(
                "`{other}` is not an RTDE type. `IN_USE` here means the controller does not \
                 have the variable that was asked for — check the name against this \
                 controller's version, because the set grew between PolyScope releases."
            )))
        }
    })
}

fn f64_at(bytes: &[u8]) -> f64 {
    f64::from_be_bytes(bytes[..8].try_into().expect("eight bytes"))
}

/// One value per type, in order. Refuses a payload whose length is not the recipe's.
pub fn decode(types: &[String], payload: &[u8]) -> Result<Vec<Value>, RtdeError> {
    let mut expected = 0;
    for name in types {
        expected += width_of(name)?;
    }
    if payload.len() != expected {
        return Err(refused(format!(
            "a data package carried {} bytes and this recipe is {expected}. A recipe and a \
             package that disagree is a client reading one variable's bytes as another's.",
            payload.len()
        )));
    }
    let mut out = Vec::with_capacity(types.len());
    let mut at = 0;
    for name in types {
        let size = width_of(name)?;
        let bytes = &payload[at..at + size];
        out.push(match name.as_str() {
            "BOOL" => Value::Bool(bytes[0] != 0),
            "UINT8" => Value::U8(bytes[0]),
            "INT32" => Value::I32(i32::from_be_bytes(bytes.try_into().expect("four"))),
            "UINT32" => Value::U32(u32::from_be_bytes(bytes.try_into().expect("four"))),
            "UINT64" => Value::U64(u64::from_be_bytes(bytes.try_into().expect("eight"))),
            "DOUBLE" => Value::F64(f64_at(bytes)),
            "VECTOR3D" => Value::Vec3(bytes.chunks(8).map(f64_at).collect()),
            "VECTOR6D" => Value::Vec6(bytes.chunks(8).map(f64_at).collect()),
            "VECTOR6INT32" => Value::Vec6I32(
                bytes
                    .chunks(4)
                    .map(|b| i32::from_be_bytes(b.try_into().expect("four")))
                    .collect(),
            ),
            _ => Value::Vec6U32(
                bytes
                    .chunks(4)
                    .map(|b| u32::from_be_bytes(b.try_into().expect("four")))
                    .collect(),
            ),
        });
        at += size;
    }
    Ok(out)
}

/// A named set of variables the controller has agreed to exchange.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipe {
    /// The controller's id for it.
    pub identifier: u8,
    /// The variables, in order.
    pub names: Vec<String>,
    /// Their types, as the controller answered them.
    pub types: Vec<String>,
}

/// The input recipe this driver speaks, and the only one [`setpoint_package`] encodes: six
/// doubles and the gate. [`Session::setup_inputs`] refuses a controller that types it otherwise.
pub const INPUT_TYPES: [&str; 7] = [
    "DOUBLE", "DOUBLE", "DOUBLE", "DOUBLE", "DOUBLE", "DOUBLE", "INT32",
];

/// A setpoint package's size on the wire: header, recipe id, six doubles, the gate.
pub const SETPOINT_PACKAGE_SIZE: usize = 3 + 1 + 6 * 8 + 4;

/// One input data package — six joint positions and the gate the control program reads — built
/// on the stack. The stop path writes one of these, so it allocates nothing.
pub fn setpoint_package(recipe: u8, setpoint: &[f64; 6], gate: i32) -> [u8; SETPOINT_PACKAGE_SIZE] {
    let mut raw = [0u8; SETPOINT_PACKAGE_SIZE];
    raw[..2].copy_from_slice(&(SETPOINT_PACKAGE_SIZE as u16).to_be_bytes());
    raw[2] = DATA_PACKAGE;
    raw[3] = recipe;
    for (index, value) in setpoint.iter().enumerate() {
        raw[4 + index * 8..12 + index * 8].copy_from_slice(&value.to_be_bytes());
    }
    raw[52..56].copy_from_slice(&gate.to_be_bytes());
    raw
}

/// A byte stream an RTDE session runs over, with the one switch the drain needs.
pub trait Transport: Read + Write {
    /// Non-blocking for the drain, blocking for everything else.
    fn set_nonblocking(&mut self, nonblocking: bool) -> io::Result<()>;
}

impl Transport for std::net::TcpStream {
    fn set_nonblocking(&mut self, nonblocking: bool) -> io::Result<()> {
        std::net::TcpStream::set_nonblocking(self, nonblocking)
    }
}

/// One RTDE session.
pub struct Session<T: Transport> {
    stream: T,
    buffer: Vec<u8>,
    /// The output recipe, once set up.
    pub output: Option<Recipe>,
    /// The input recipe, once set up.
    pub input: Option<Recipe>,
    /// Everything the controller said, in order: a text message during a handshake is usually the
    /// reason a later step fails, and it is read afterwards.
    pub messages: Vec<String>,
}

impl<T: Transport> Session<T> {
    /// A session over a connected stream.
    pub fn new(stream: T) -> Session<T> {
        Session {
            stream,
            buffer: Vec::with_capacity(8192),
            output: None,
            input: None,
            messages: Vec::new(),
        }
    }

    /// The stream, for the stop path to clone.
    pub fn stream(&self) -> &T {
        &self.stream
    }

    fn send(&mut self, packet_type: u8, payload: &[u8]) -> Result<(), RtdeError> {
        let size = u16::try_from(3 + payload.len())
            .map_err(|_| refused("an RTDE packet is at most 65,535 bytes"))?;
        let mut packet = Vec::with_capacity(3 + payload.len());
        packet.extend_from_slice(&size.to_be_bytes());
        packet.push(packet_type);
        packet.extend_from_slice(payload);
        self.stream.write_all(&packet)?;
        Ok(())
    }

    /// The next whole packet. Nothing is consumed until all of it is buffered (F75).
    fn packet(&mut self) -> Result<(u8, Vec<u8>), RtdeError> {
        loop {
            if self.buffer.len() >= 3 {
                let size = u16::from_be_bytes([self.buffer[0], self.buffer[1]]) as usize;
                if size < 3 {
                    return Err(refused(format!(
                        "a packet claiming {size} bytes cannot hold its own header"
                    )));
                }
                if self.buffer.len() >= size {
                    let packet_type = self.buffer[2];
                    let body = self.buffer[3..size].to_vec();
                    self.buffer.drain(..size);
                    return Ok((packet_type, body));
                }
            }
            let mut chunk = [0u8; 4096];
            let n = self.stream.read(&mut chunk)?;
            if n == 0 {
                return Err(refused("the controller closed the connection"));
            }
            self.buffer.extend_from_slice(&chunk[..n]);
        }
    }

    /// `<level><message><source>` on protocol 2. The message is the useful part, and the rest is
    /// not worth a parser that would break on a version change — `rtde.py`'s `_note`.
    fn note(&mut self, payload: &[u8]) {
        let text = String::from_utf8_lossy(payload);
        self.messages
            .push(text.trim_matches('\0').trim().to_string());
    }

    /// Send, then read until the reply of that type. Text messages are collected, not dropped.
    pub fn transact(&mut self, packet_type: u8, payload: &[u8]) -> Result<Vec<u8>, RtdeError> {
        self.send(packet_type, payload)?;
        for _ in 0..TRANSACT_PACKETS {
            let (got, body) = self.packet()?;
            if got == TEXT_MESSAGE {
                self.note(&body);
                continue;
            }
            if got != packet_type {
                return Err(refused(format!(
                    "asked for {packet_type} and the controller answered {got}. This client \
                     reads past text messages on purpose; anything else out of order is a \
                     session two clients are sharing."
                )));
            }
            return Ok(body);
        }
        Err(refused("the controller sent nothing but text messages"))
    }

    /// Ask for protocol `version`, and refuse a controller that will not speak it.
    pub fn negotiate(&mut self, version: u16) -> Result<(), RtdeError> {
        let accepted = self.transact(REQUEST_PROTOCOL_VERSION, &version.to_be_bytes())?;
        if accepted != [1] {
            return Err(refused(format!(
                "this controller refused RTDE protocol version {version}. Version 1 has no \
                 frequency field on the output setup and this client does not speak it."
            )));
        }
        Ok(())
    }

    /// URControl's version: major, minor, bugfix, build.
    pub fn controller_version(&mut self) -> Result<[u32; 4], RtdeError> {
        let body = self.transact(GET_URCONTROL_VERSION, &[])?;
        if body.len() != 16 {
            return Err(refused(format!(
                "a version reply is four u32 and this one is {} bytes",
                body.len()
            )));
        }
        let mut version = [0u32; 4];
        for (index, part) in version.iter_mut().enumerate() {
            *part = u32::from_be_bytes(body[index * 4..index * 4 + 4].try_into().expect("four"));
        }
        Ok(version)
    }

    fn recipe(body: &[u8], names: &[&str], which: &str) -> Result<Recipe, RtdeError> {
        let Some((&identifier, types)) = body.split_first() else {
            return Err(refused(format!("the {which} setup reply is empty")));
        };
        let types: Vec<String> = String::from_utf8_lossy(types)
            .split(',')
            .map(str::to_string)
            .collect();
        let missing: Vec<&str> = names
            .iter()
            .zip(&types)
            .filter(|(_, kind)| kind.as_str() == "IN_USE")
            .map(|(name, _)| *name)
            .collect();
        if !missing.is_empty() {
            return Err(refused(format!(
                "the controller answered `IN_USE` for {which} variable(s) {}, which means one of \
                 two things and the first is far more common: **another RTDE client already \
                 holds them** — the general-purpose registers are allocated to one session at a \
                 time, and a driver that was not closed cleanly keeps them for a few seconds \
                 after its socket goes — or this controller does not have the variable at all, \
                 which is a name from a newer PolyScope. Either way the recipe is refused here \
                 rather than used, because RTDE answers `IN_USE` *in place of a type* and a \
                 client that did not look would decode every field after it at the wrong offset.",
                missing.join(", ")
            )));
        }
        if types.len() != names.len() {
            return Err(refused(format!(
                "{} {which} name(s) were asked for and {} type(s) came back",
                names.len(),
                types.len()
            )));
        }
        Ok(Recipe {
            identifier,
            names: names.iter().map(|name| name.to_string()).collect(),
            types,
        })
    }

    /// Ask for `names` at `frequency_hz`.
    pub fn setup_outputs(&mut self, names: &[&str], frequency_hz: f64) -> Result<(), RtdeError> {
        let mut payload = frequency_hz.to_be_bytes().to_vec();
        payload.extend_from_slice(names.join(",").as_bytes());
        let body = self.transact(SETUP_OUTPUTS, &payload)?;
        self.output = Some(Self::recipe(&body, names, "output")?);
        Ok(())
    }

    /// Offer `names` as inputs. Refuses a controller that types them other than
    /// [`INPUT_TYPES`], which is the only layout [`setpoint_package`] writes.
    pub fn setup_inputs(&mut self, names: &[&str]) -> Result<(), RtdeError> {
        let body = self.transact(SETUP_INPUTS, names.join(",").as_bytes())?;
        let recipe = Self::recipe(&body, names, "input")?;
        if recipe.types != INPUT_TYPES {
            return Err(refused(format!(
                "the controller typed the setpoint registers {} and this driver writes {}. A \
                 package written under the wrong types is six joint positions read as something \
                 else, so the recipe is refused rather than used.",
                recipe.types.join(","),
                INPUT_TYPES.join(",")
            )));
        }
        self.input = Some(recipe);
        Ok(())
    }

    /// Start the exchange.
    pub fn start(&mut self) -> Result<(), RtdeError> {
        if self.transact(START, &[])? != [1] {
            return Err(refused(
                "the controller refused to start the data exchange. A recipe that was never set \
                 up, or another client already holding the session, are the two causes.",
            ));
        }
        Ok(())
    }

    /// Pause the exchange, reading past the data packages still streaming and every text message
    /// until the pause's own reply. `rtde.py`'s `pause` went through `transact`, which refuses the
    /// first data package it meets — so it always raised while the stream ran, and `close`
    /// swallowed it; the request still reached the controller, as it does here.
    pub fn pause(&mut self) -> Result<(), RtdeError> {
        self.send(PAUSE, &[])?;
        for _ in 0..TRANSACT_PACKETS * 16 {
            let (got, body) = self.packet()?;
            match got {
                PAUSE => return Ok(()),
                TEXT_MESSAGE => self.note(&body),
                _ => {}
            }
        }
        Err(refused("the controller never answered the pause"))
    }

    /// One setpoint package: six joint positions and the gate.
    pub fn write_setpoint(&mut self, setpoint: &[f64; 6], gate: i32) -> Result<(), RtdeError> {
        let Some(input) = &self.input else {
            return Err(refused("no input recipe has been set up"));
        };
        let package = setpoint_package(input.identifier, setpoint, gate);
        self.stream.write_all(&package)?;
        Ok(())
    }

    /// The most recent complete output package, discarding everything behind it, or `None` when
    /// nothing whole has arrived. Draining is the point — see the module header.
    pub fn newest(&mut self) -> Result<Option<Vec<Value>>, RtdeError> {
        let Some(output) = self.output.clone() else {
            return Err(refused("no output recipe has been set up"));
        };
        self.stream.set_nonblocking(true)?;
        let result = self.drain(&output);
        self.stream.set_nonblocking(false)?;
        result
    }

    fn drain(&mut self, output: &Recipe) -> Result<Option<Vec<Value>>, RtdeError> {
        let mut latest: Option<Vec<u8>> = None;
        loop {
            let (packet_type, body) = match self.packet() {
                Ok(packet) => packet,
                Err(RtdeError::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    break;
                }
                Err(other) => return Err(other),
            };
            if packet_type == TEXT_MESSAGE {
                self.note(&body);
                continue;
            }
            if packet_type != DATA_PACKAGE || body.first() != Some(&output.identifier) {
                continue;
            }
            latest = Some(body);
        }
        // Only the newest is decoded: the ones behind it were never going to be read.
        match latest {
            None => Ok(None),
            Some(body) => decode(&output.types, &body[1..]).map(Some),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_use_is_a_refusal_not_a_width() {
        assert!(matches!(width_of("IN_USE"), Err(RtdeError::Refused(_))));
        assert_eq!(width_of("VECTOR6D").unwrap(), 48);
    }

    #[test]
    fn a_package_that_disagrees_with_its_recipe_is_refused() {
        let types = vec!["INT32".to_string(), "DOUBLE".to_string()];
        assert!(decode(&types, &[0; 11]).is_err());
        let values = decode(
            &types,
            &[0xFF, 0xFF, 0xFF, 0xF9, 0x3F, 0xF8, 0, 0, 0, 0, 0, 0],
        );
        assert_eq!(values.unwrap(), vec![Value::I32(-7), Value::F64(1.5)]);
    }

    #[test]
    fn the_setpoint_package_is_big_endian_and_sized() {
        let raw = setpoint_package(2, &[1.0, 0.0, 0.0, 0.0, 0.0, -1.0], 1);
        assert_eq!(&raw[..4], &[0, 56, DATA_PACKAGE, 2]);
        assert_eq!(&raw[4..12], &1.0f64.to_be_bytes());
        assert_eq!(&raw[44..52], &(-1.0f64).to_be_bytes());
        assert_eq!(&raw[52..], &[0, 0, 0, 1]);
    }
}

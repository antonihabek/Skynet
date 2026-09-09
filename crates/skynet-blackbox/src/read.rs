//! Reading a sealed recording back (§13, §41.3).
//!
//! # The half that was missing
//!
//! [`crate::seal`] has written MCAP since the sixth session and **nothing has ever read one**. That
//! is a stranger gap than it sounds: §13's whole argument is that a recording turns *"the robot did
//! something strange on Tuesday"* from an anecdote into evidence, and evidence nobody can open is
//! an anecdote with a checksum. Every tool that wanted to know what was in a window asked the robot
//! — which works until the robot is the thing that is broken, or until the window is a file
//! somebody emailed.
//!
//! So this is the reader, and it deliberately stops short of replay. [`crate::replay`] evaluates
//! recorded cycles against resolved limits and is the harder, more valuable thing; it needs the
//! cycle records reassembled out of the four messages a cycle is written as, and building that as a
//! side effect of *"open the file"* would produce a bad version of both. What this answers is the
//! question a person actually opens a recording with: **what is this, where did it come from, is it
//! complete, and what happened.**
//!
//! # What it reads, and what it skips
//!
//! An MCAP file is `magic · records · magic`, and a record is `opcode · u64 length · body`. Three
//! opcodes matter here — [`op::METADATA`] carries §56.2's provenance, [`op::CHANNEL`] names the
//! topics, and [`op::MESSAGE`] carries the records themselves — and everything else is skipped by
//! length without being understood. That is not laziness: it is what makes this reader survive a
//! writer that later adds a chunk index or an attachment, and it is the property the MCAP framing
//! exists to provide.
//!
//! # Nothing here trusts the file
//!
//! Every length is checked against what is left before it is used. A recording is exactly the kind
//! of file that arrives from somewhere else — off a robot that was failing, over a link, through
//! whatever a support conversation involves — and a reader that indexed past the end of a buffer
//! because a length said so would be a reader with a hole in it. Every failure is a named
//! [`ReadError`] and none of them is a panic.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::mcap::{op, MAGIC};

/// Why a sealed recording could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The file does not begin with MCAP's magic. It is not a recording.
    NotMcap,
    /// It begins correctly and ends before it should.
    Truncated {
        /// Where the reader was when it ran out.
        at: usize,
    },
    /// A record's declared length runs past the end of the file.
    Overrun {
        /// Which opcode declared it.
        opcode: u8,
        /// Where that record began.
        at: usize,
        /// How many bytes it asked for.
        wanted: u64,
    },
    /// A string or map inside a record is longer than the record holding it.
    Malformed {
        /// Which record.
        opcode: u8,
        /// What was being read out of it.
        field: &'static str,
    },
}

impl ReadError {
    /// A sentence for an operator, naming what to do about it where there is anything.
    pub fn detail(&self) -> String {
        match self {
            ReadError::NotMcap => "this file does not begin with MCAP's magic, so it is not a \
                                   sealed recording. A window's name ends in `.mcap` and its first \
                                   eight bytes say so too."
                .to_string(),
            ReadError::Truncated { at } => {
                let mut text = String::from("the file ends in the middle of a record, at byte ");
                text.push_str(&at.to_string());
                text.push_str(
                    ". A recording cut short is usually a transfer that stopped: what is here is \
                     still readable up to that point, and §79's upload resumes rather than \
                     restarting.",
                );
                text
            }
            ReadError::Overrun { opcode, at, wanted } => {
                let mut text = String::from("a record at byte ");
                text.push_str(&at.to_string());
                text.push_str(" (opcode ");
                text.push_str(&opcode.to_string());
                text.push_str(") declares ");
                text.push_str(&wanted.to_string());
                text.push_str(
                    " bytes and the file does not hold them. The file is damaged rather than \
                     merely incomplete.",
                );
                text
            }
            ReadError::Malformed { opcode, field } => {
                let mut text = String::from("a record (opcode ");
                text.push_str(&opcode.to_string());
                text.push_str(") has a `");
                text.push_str(field);
                text.push_str("` that runs past the end of the record holding it.");
                text
            }
        }
    }
}

/// One message, as the file holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Which channel it was published on. See [`crate::schema::id`].
    pub channel_id: u16,
    /// The channel's own sequence for this message.
    pub sequence: u32,
    /// The wall clock at the record, in nanoseconds (§35.1's correlation clock).
    pub log_time_ns: u64,
    /// The payload, which for every channel this platform writes is one JSON object.
    pub payload: String,
}

/// What a sealed recording holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Recording {
    /// §56.2's provenance, as `skynet.blackbox` recorded it: robot, digests, tier, continuity.
    ///
    /// Key/value pairs in the order the seal wrote them. Not decoded into a struct here, because
    /// the set grows — `policy_digest_0`, `policy_digest_1` and so on are written per digest — and
    /// a reader that only surfaced the fields it knew about would silently drop the ones a later
    /// edition added, which is the opposite of what a black box is for.
    pub metadata: Vec<(String, String)>,
    /// The topics, by channel id, so a message can be named rather than numbered.
    pub channels: Vec<(u16, String)>,
    /// Every message, in file order.
    pub messages: Vec<Message>,
    /// Records whose opcode this reader does not interpret, counted rather than dropped silently.
    ///
    /// Reported because a file that is mostly records nobody read is worth knowing about: it is how
    /// a reader from one edition looks at a file from a newer one, and *"I understood four of nine
    /// thousand records"* is a different sentence from *"the recording is nearly empty"*.
    pub skipped_records: usize,
}

impl Recording {
    /// One metadata value by key, when the file recorded a value for it.
    ///
    /// An **empty** value reads as absent, and that is the interesting half. The seal writes every
    /// key whether or not the robot had something to put in it — `robot_id` is empty on a machine
    /// that has never been paired — so a reader that returned `Some("")` would hand a caller a
    /// blank where the caller expects a fact, and every renderer downstream would print an empty
    /// column that reads as a value nobody noticed was missing.
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
            .filter(|value| !value.is_empty())
    }

    /// The topic a channel publishes, when the file declared one.
    pub fn topic(&self, channel_id: u16) -> Option<&str> {
        self.channels
            .iter()
            .find(|(id, _)| *id == channel_id)
            .map(|(_, topic)| topic.as_str())
    }

    /// Every message on one channel.
    pub fn on(&self, channel_id: u16) -> impl Iterator<Item = &Message> {
        self.messages
            .iter()
            .filter(move |message| message.channel_id == channel_id)
    }
}

/// Read a sealed recording.
///
/// Both magics are checked. The trailing one matters more than it looks: a file that begins
/// correctly and does not end correctly is a **truncated transfer**, which is a different thing
/// from a damaged one and sends somebody somewhere different — so the two are separate errors
/// rather than one "bad file".
pub fn read(bytes: &[u8]) -> Result<Recording, ReadError> {
    if bytes.len() < MAGIC.len() * 2 || bytes[..MAGIC.len()] != MAGIC {
        return Err(ReadError::NotMcap);
    }
    if bytes[bytes.len() - MAGIC.len()..] != MAGIC {
        return Err(ReadError::Truncated { at: bytes.len() });
    }

    let end = bytes.len() - MAGIC.len();
    let mut at = MAGIC.len();
    let mut found = Recording::default();

    while at < end {
        // `opcode · u64 length`. Nine bytes, and a file that has fewer left has ended mid-record.
        if at + 9 > end {
            return Err(ReadError::Truncated { at });
        }
        let opcode = bytes[at];
        let length = u64_at(bytes, at + 1);
        let body_at = at + 9;
        let Some(body_end) = length
            .try_into()
            .ok()
            .and_then(|length: usize| body_at.checked_add(length))
            .filter(|body_end| *body_end <= end)
        else {
            return Err(ReadError::Overrun {
                opcode,
                at,
                wanted: length,
            });
        };
        let body = &bytes[body_at..body_end];

        match opcode {
            op::METADATA => found.metadata.extend(metadata(body)?),
            op::CHANNEL => {
                let (id, topic) = channel(body)?;
                // **The first declaration wins, and the repeats are skipped.**
                //
                // MCAP puts the schemas and channels in the data section *and* again in the
                // summary, so a reader that pushed every `Channel` record it saw would report each
                // topic twice — which the first version of this did, and `skynet blackbox inspect`
                // duly printed ten lines for five channels. The duplicate is not a defect in the
                // file; repeating them is what lets a reader that seeks to the summary find the
                // channels without walking the data section.
                if !found.channels.iter().any(|(seen, _)| *seen == id) {
                    found.channels.push((id, topic));
                }
            }
            op::MESSAGE => found.messages.push(message(body)?),
            // Skipped by length, without being understood. See the module header: this is what
            // lets a reader from one edition open a file from a later one.
            _ => found.skipped_records += 1,
        }

        at = body_end;
    }

    Ok(found)
}

/// `name · u32 byte-length · (string key, string value)*`
fn metadata(body: &[u8]) -> Result<Vec<(String, String)>, ReadError> {
    let mut at = 0usize;
    // The record's own name — `skynet.blackbox` — read and dropped: this reader returns the pairs,
    // and a caller that wanted to know which metadata record they came from would be a caller for
    // whom the seal writes more than one, which it does not.
    let _name = string_at(body, &mut at, op::METADATA, "name")?;
    let map_len = u32_at_checked(body, &mut at, op::METADATA, "map length")? as usize;
    let Some(map_end) = at.checked_add(map_len).filter(|end| *end <= body.len()) else {
        return Err(ReadError::Malformed {
            opcode: op::METADATA,
            field: "map",
        });
    };

    let mut pairs = Vec::new();
    while at < map_end {
        let key = string_at(body, &mut at, op::METADATA, "key")?;
        let value = string_at(body, &mut at, op::METADATA, "value")?;
        pairs.push((key, value));
    }
    Ok(pairs)
}

/// `u16 id · u16 schema_id · string topic · string encoding · u32 metadata length · …`
fn channel(body: &[u8]) -> Result<(u16, String), ReadError> {
    let mut at = 0usize;
    let id = u16_at_checked(body, &mut at, op::CHANNEL, "id")?;
    let _schema_id = u16_at_checked(body, &mut at, op::CHANNEL, "schema id")?;
    let topic = string_at(body, &mut at, op::CHANNEL, "topic")?;
    Ok((id, topic))
}

/// `u16 channel · u32 sequence · u64 log_time · u64 publish_time · payload`
fn message(body: &[u8]) -> Result<Message, ReadError> {
    let mut at = 0usize;
    let channel_id = u16_at_checked(body, &mut at, op::MESSAGE, "channel")?;
    let sequence = u32_at_checked(body, &mut at, op::MESSAGE, "sequence")?;
    let log_time_ns = u64_at_checked(body, &mut at, op::MESSAGE, "log time")?;
    let _publish_time = u64_at_checked(body, &mut at, op::MESSAGE, "publish time")?;
    // The rest is the payload. Lossy rather than refusing: a message with one bad byte in it is
    // still a message somebody should see, and a whole recording refused over one is the failure
    // mode a black box cannot have.
    let payload = String::from_utf8_lossy(&body[at..]).into_owned();
    Ok(Message {
        channel_id,
        sequence,
        log_time_ns,
        payload,
    })
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut value = 0u64;
    for index in 0..8 {
        value |= (bytes[at + index] as u64) << (index * 8);
    }
    value
}

fn u16_at_checked(
    body: &[u8],
    at: &mut usize,
    opcode: u8,
    field: &'static str,
) -> Result<u16, ReadError> {
    if *at + 2 > body.len() {
        return Err(ReadError::Malformed { opcode, field });
    }
    let value = (body[*at] as u16) | ((body[*at + 1] as u16) << 8);
    *at += 2;
    Ok(value)
}

fn u32_at_checked(
    body: &[u8],
    at: &mut usize,
    opcode: u8,
    field: &'static str,
) -> Result<u32, ReadError> {
    if *at + 4 > body.len() {
        return Err(ReadError::Malformed { opcode, field });
    }
    let mut value = 0u32;
    for index in 0..4 {
        value |= (body[*at + index] as u32) << (index * 8);
    }
    *at += 4;
    Ok(value)
}

fn u64_at_checked(
    body: &[u8],
    at: &mut usize,
    opcode: u8,
    field: &'static str,
) -> Result<u64, ReadError> {
    if *at + 8 > body.len() {
        return Err(ReadError::Malformed { opcode, field });
    }
    let value = u64_at(body, *at);
    *at += 8;
    Ok(value)
}

/// `u32 length · bytes`, MCAP's string encoding.
fn string_at(
    body: &[u8],
    at: &mut usize,
    opcode: u8,
    field: &'static str,
) -> Result<String, ReadError> {
    let length = u32_at_checked(body, at, opcode, field)? as usize;
    let Some(end) = at.checked_add(length).filter(|end| *end <= body.len()) else {
        return Err(ReadError::Malformed { opcode, field });
    };
    let text = String::from_utf8_lossy(&body[*at..end]).into_owned();
    *at = end;
    Ok(text)
}

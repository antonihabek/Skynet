//! MCAP, written by hand (§26).
//!
//! > **MCAP**, which is already the ROS 2 default bag format since Iron and is what the robotics
//! > tooling ecosystem reads. […] Inventing a format here would have bought nothing and cost the
//! > entire existing toolchain.
//!
//! So this module writes MCAP rather than something of ours, and writes it without a library
//! because the workspace has no third-party dependencies and CI asserts it every commit. That is a
//! smaller undertaking than it sounds: MCAP's data model is a magic, a sequence of
//! `opcode · u64 length · body` records, and a summary section that repeats the schemas and
//! channels so a reader can index the file without scanning it. What follows is that, and nothing
//! else — no chunking, no compression, no attachments.
//!
//! # What this writer deliberately does not do
//!
//! **No chunks.** A `Chunk` record wraps messages in a compressed blob, which is how MCAP keeps a
//! multi-gigabyte bag small. A sealed black box is bounded by the ring, and every reader
//! understands unchunked files, so chunking would trade a property (any tool can open this) for a
//! saving (a smaller file) that the ring has already made unnecessary. It also keeps the writer
//! within a few hundred lines, which matters for something that runs after a fault.
//!
//! **No message index.** Its purpose is seeking within a chunk. There are no chunks.
//!
//! # JSON messages, and why not CDR
//!
//! Message bodies are canonical JSON with a JSON Schema on each channel, rather than ROS 2's CDR.
//! Three reasons, in order of weight:
//!
//! 1. §26 asks for channels "each with an embedded schema so the file stays readable without the
//!    code that wrote it". A JSON Schema and a JSON body meet that literally; a CDR body needs a
//!    generator and an IDL to be anything but bytes.
//! 2. Canonical JSON is already a shared, dual-implemented, corpus-tested artifact of this project
//!    (`skynet-json`). Every edition already agrees on the bytes, so §31.2's "a black box sealed by
//!    one edition must be readable by every other" comes for free rather than needing a second
//!    serialisation format to be specified and held identical.
//! 3. The file is produced once, after an incident, from a bounded ring. The compactness argument
//!    for CDR applies to a bag being written at line rate, which this is not.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::crc32::Crc32;

/// The eight bytes that begin and end every MCAP file.
pub const MAGIC: [u8; 8] = [0x89, b'M', b'C', b'A', b'P', 0x30, b'\r', b'\n'];

/// Record opcodes, from the MCAP specification.
pub mod op {
    /// The file header.
    pub const HEADER: u8 = 0x01;
    /// The file footer.
    pub const FOOTER: u8 = 0x02;
    /// A message schema.
    pub const SCHEMA: u8 = 0x03;
    /// A channel, referring to a schema.
    pub const CHANNEL: u8 = 0x04;
    /// One message on a channel.
    pub const MESSAGE: u8 = 0x05;
    /// Free-form key/value metadata.
    pub const METADATA: u8 = 0x0C;
    /// Counts and time bounds, in the summary.
    pub const STATISTICS: u8 = 0x0B;
    /// Where a group of summary records begins.
    pub const SUMMARY_OFFSET: u8 = 0x0E;
    /// The end of the data section, carrying its CRC.
    pub const DATA_END: u8 = 0x0F;
}

/// One channel's schema, as it will be embedded.
///
/// `definition` is owned rather than borrowed because the documents are *built* and serialised
/// through canonical JSON (see [`crate::schema`]) rather than written as literals — which is what
/// makes two editions emit the same bytes without two people keeping two literals in step.
#[derive(Debug, Clone)]
pub struct Schema {
    /// One-based, and stable: the corpus pins these.
    pub id: u16,
    /// The schema's name, as tooling will show it.
    pub name: &'static str,
    /// The schema document itself.
    pub definition: String,
}

/// One channel.
#[derive(Debug, Clone, Copy)]
pub struct Channel {
    /// One-based, and stable.
    pub id: u16,
    /// The schema it publishes.
    pub schema_id: u16,
    /// The topic, as §26's table names it.
    pub topic: &'static str,
}

/// An MCAP file, accumulated in memory.
///
/// In memory because sealing produces a file that is then written once, and because the alternative
/// — streaming to disk — would need the writer to seek back and patch the summary offsets. A sealed
/// recording is bounded by the ring, so the peak is known before the first byte.
#[derive(Debug)]
pub struct Writer {
    bytes: Vec<u8>,
    data_crc: Crc32,
    schemas: Vec<Schema>,
    channels: Vec<Channel>,
    counts: Vec<(u16, u64)>,
    metadata_count: u32,
    first_ns: Option<u64>,
    last_ns: u64,
    sequences: Vec<(u16, u32)>,
}

impl Writer {
    /// Begin a file, writing the magic and the `Header` record.
    ///
    /// The profile is empty rather than `ros2`: this file's channels are not ROS topics and
    /// claiming the profile would tell a reader to expect ROS conventions that are not there.
    pub fn new(library: &str) -> Writer {
        let mut writer = Writer {
            bytes: Vec::new(),
            data_crc: Crc32::new(),
            schemas: Vec::new(),
            channels: Vec::new(),
            counts: Vec::new(),
            metadata_count: 0,
            first_ns: None,
            last_ns: 0,
            sequences: Vec::new(),
        };
        writer.raw(&MAGIC);
        let mut body = Vec::new();
        put_string(&mut body, "");
        put_string(&mut body, library);
        writer.record(op::HEADER, &body);
        writer
    }

    /// Declare a schema. Must precede any channel referring to it.
    pub fn schema(&mut self, schema: Schema) {
        let mut body = Vec::new();
        put_u16(&mut body, schema.id);
        put_string(&mut body, schema.name);
        put_string(&mut body, "jsonschema");
        put_u32(&mut body, schema.definition.len() as u32);
        body.extend_from_slice(schema.definition.as_bytes());
        self.record(op::SCHEMA, &body);
        self.schemas.push(schema);
    }

    /// Declare a channel.
    pub fn channel(&mut self, channel: Channel) {
        let mut body = Vec::new();
        put_u16(&mut body, channel.id);
        put_u16(&mut body, channel.schema_id);
        put_string(&mut body, channel.topic);
        put_string(&mut body, "json");
        // An empty metadata map: four bytes of zero length. Written explicitly because a reader
        // parses the field whether or not there is anything in it.
        put_u32(&mut body, 0);
        self.record(op::CHANNEL, &body);
        self.channels.push(channel);
        self.sequences.push((channel.id, 0));
        self.counts.push((channel.id, 0));
    }

    /// Write one message.
    ///
    /// `log_time` is nanoseconds since the epoch, because that is what MCAP means by the field and
    /// what every reader will render. The recording's own `CLOCK_MONOTONIC_RAW` value is carried
    /// *inside* the message as well, since that is the time base every control decision was made
    /// against (§35.1) and it must survive a reader that only understands wall clocks.
    pub fn message(&mut self, channel_id: u16, log_time: u64, payload: &str) {
        let sequence = {
            let entry = self
                .sequences
                .iter_mut()
                .find(|(id, _)| *id == channel_id)
                .expect("a message on a channel that was never declared");
            entry.1 += 1;
            entry.1
        };
        let mut body = Vec::new();
        put_u16(&mut body, channel_id);
        put_u32(&mut body, sequence);
        put_u64(&mut body, log_time);
        put_u64(&mut body, log_time);
        body.extend_from_slice(payload.as_bytes());
        self.record(op::MESSAGE, &body);

        if let Some(entry) = self.counts.iter_mut().find(|(id, _)| *id == channel_id) {
            entry.1 += 1;
        }
        self.first_ns = Some(self.first_ns.map_or(log_time, |first| first.min(log_time)));
        self.last_ns = self.last_ns.max(log_time);
    }

    /// Write a metadata record: the recording's provenance, as key/value pairs.
    pub fn metadata(&mut self, name: &str, pairs: &[(String, String)]) {
        let mut map = Vec::new();
        for (key, value) in pairs {
            put_string(&mut map, key);
            put_string(&mut map, value);
        }
        let mut body = Vec::new();
        put_string(&mut body, name);
        put_u32(&mut body, map.len() as u32);
        body.extend_from_slice(&map);
        self.record(op::METADATA, &body);
        self.metadata_count += 1;
    }

    /// Close the file: `DataEnd`, the summary, the offsets, the footer, the trailing magic.
    pub fn finish(mut self) -> Vec<u8> {
        let mut end = Vec::new();
        put_u32(&mut end, self.data_crc.value());
        self.record(op::DATA_END, &end);

        let summary_start = self.bytes.len() as u64;

        let schema_start = self.bytes.len() as u64;
        let schemas = core::mem::take(&mut self.schemas);
        for schema in &schemas {
            let mut body = Vec::new();
            put_u16(&mut body, schema.id);
            put_string(&mut body, schema.name);
            put_string(&mut body, "jsonschema");
            put_u32(&mut body, schema.definition.len() as u32);
            body.extend_from_slice(schema.definition.as_bytes());
            self.record(op::SCHEMA, &body);
        }
        let schema_len = self.bytes.len() as u64 - schema_start;

        let channel_start = self.bytes.len() as u64;
        let channels = core::mem::take(&mut self.channels);
        for channel in &channels {
            let mut body = Vec::new();
            put_u16(&mut body, channel.id);
            put_u16(&mut body, channel.schema_id);
            put_string(&mut body, channel.topic);
            put_string(&mut body, "json");
            put_u32(&mut body, 0);
            self.record(op::CHANNEL, &body);
        }
        let channel_len = self.bytes.len() as u64 - channel_start;

        let statistics_start = self.bytes.len() as u64;
        let mut body = Vec::new();
        let total: u64 = self.counts.iter().map(|(_, count)| *count).sum();
        put_u64(&mut body, total);
        put_u16(&mut body, schemas.len() as u16);
        put_u32(&mut body, channels.len() as u32);
        put_u32(&mut body, 0); // attachments
        put_u32(&mut body, self.metadata_count);
        put_u32(&mut body, 0); // chunks
        put_u64(&mut body, self.first_ns.unwrap_or(0));
        put_u64(&mut body, self.last_ns);
        let mut per_channel = Vec::new();
        for (id, count) in &self.counts {
            put_u16(&mut per_channel, *id);
            put_u64(&mut per_channel, *count);
        }
        put_u32(&mut body, per_channel.len() as u32);
        body.extend_from_slice(&per_channel);
        self.record(op::STATISTICS, &body);
        let statistics_len = self.bytes.len() as u64 - statistics_start;

        let summary_offset_start = self.bytes.len() as u64;
        for (opcode, start, length) in [
            (op::SCHEMA, schema_start, schema_len),
            (op::CHANNEL, channel_start, channel_len),
            (op::STATISTICS, statistics_start, statistics_len),
        ] {
            let mut body = Vec::new();
            body.push(opcode);
            put_u64(&mut body, start);
            put_u64(&mut body, length);
            self.record(op::SUMMARY_OFFSET, &body);
        }

        // The footer's CRC covers the summary section, the summary offset section, and the footer's
        // own opcode, length and two offsets — everything but the four CRC bytes themselves.
        let mut footer = Vec::new();
        footer.push(op::FOOTER);
        put_u64(&mut footer, 20);
        put_u64(&mut footer, summary_start);
        put_u64(&mut footer, summary_offset_start);
        let mut crc = Crc32::new();
        crc.update(&self.bytes[summary_start as usize..]);
        crc.update(&footer);
        let summary_crc = crc.value();
        put_u32(&mut footer, summary_crc);
        self.bytes.extend_from_slice(&footer);

        self.bytes.extend_from_slice(&MAGIC);
        self.bytes
    }

    fn record(&mut self, opcode: u8, body: &[u8]) {
        let mut framed = Vec::with_capacity(body.len() + 9);
        framed.push(opcode);
        put_u64(&mut framed, body.len() as u64);
        framed.extend_from_slice(body);
        self.raw(&framed);
    }

    fn raw(&mut self, bytes: &[u8]) {
        self.data_crc.update(bytes);
        self.bytes.extend_from_slice(bytes);
    }
}

fn put_u16(into: &mut Vec<u8>, value: u16) {
    into.extend_from_slice(&value.to_le_bytes());
}
fn put_u32(into: &mut Vec<u8>, value: u32) {
    into.extend_from_slice(&value.to_le_bytes());
}
fn put_u64(into: &mut Vec<u8>, value: u64) {
    into.extend_from_slice(&value.to_le_bytes());
}
fn put_string(into: &mut Vec<u8>, value: &str) {
    put_u32(into, value.len() as u32);
    into.extend_from_slice(value.as_bytes());
}

/// Hex, lower case, for the digests a sealed file carries in its metadata.
pub fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

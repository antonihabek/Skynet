//! The black box corpus, from the Rust edition's side.
//!
//! Four corpora, and each holds a different kind of thing: `schemas` pins the six MCAP channel
//! schemas (§31.2 keeps them single-source across editions), `records` pins the ring's record
//! layout, `header` pins the ring header and its two tables, and `seal` pins the framing of a whole
//! file by its digest.
//!
//! # Sixty-four-bit fields arrive as decimal strings
//!
//! A wall-clock epoch in nanoseconds is about 1.7e18, past 2^53, so a corpus input that carried one
//! as a JSON number would be comparing two editions on a value neither of them received intact.
//! The oracle writes them as strings and this module parses them as `u64`, which is the only shape
//! in which the two can be talking about the same instant.

use skynet_blackbox::mcap::{self, Writer};
use skynet_blackbox::record::{
    encode, CycleView, Entry, EventCode, EventView, FrameView, Gap, GapReason, Shape,
};
use skynet_blackbox::ring::{Geometry, Meta, Ring};
use skynet_blackbox::schema;
use skynet_json::Json;
use skynet_kernel::{FaultResponse, Guard, GUARDS};

/// Hex to bytes. A malformed digit is an error rather than a zero, because a digest that silently
/// became zeroes would compare equal to an empty table slot.
fn unhex(text: &str) -> Result<Vec<u8>, String> {
    if text.len() % 2 != 0 {
        return Err(format!("`{text}` is not an even number of hex digits"));
    }
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(text.len() / 2);
    for pair in bytes.chunks(2) {
        let value = u8::from_str_radix(
            core::str::from_utf8(pair).map_err(|_| "not hex".to_string())?,
            16,
        )
        .map_err(|_| format!("`{text}` is not hex"))?;
        out.push(value);
    }
    Ok(out)
}

fn digest32(text: &str) -> Result<[u8; 32], String> {
    let bytes = unhex(text)?;
    if bytes.len() != 32 {
        return Err(format!("`{text}` is {} bytes, not 32", bytes.len()));
    }
    let mut digest = [0u8; 32];
    digest.copy_from_slice(&bytes);
    Ok(digest)
}

fn u64_of(input: &Json, key: &str) -> Result<u64, String> {
    let text = input
        .get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("no `{key}`"))?;
    text.parse::<u64>()
        .map_err(|_| format!("`{key}` is `{text}`, which is not a decimal u64"))
}

fn u32_of(input: &Json, key: &str) -> Result<u32, String> {
    input
        .get(key)
        .and_then(Json::as_f64)
        .map(|value| value as u32)
        .ok_or_else(|| format!("no `{key}`"))
}

fn f64s(input: &Json, key: &str) -> Vec<f64> {
    input
        .arr_or_empty(key)
        .iter()
        // `null` is how the oracle spells NaN, which is how §24 spells "this is not measured".
        .map(|value| value.as_f64().unwrap_or(f64::NAN))
        .collect()
}

fn f32s(input: &Json, key: &str) -> Vec<f32> {
    f64s(input, key)
        .into_iter()
        .map(|value| value as f32)
        .collect()
}

fn shape_of(input: &Json) -> Result<Shape, String> {
    let shape = input.get("shape").ok_or("no `shape`")?;
    Ok(Shape {
        joints: shape.get("joints").and_then(Json::as_usize).unwrap_or(0) as u16,
        channels: shape.get("channels").and_then(Json::as_usize).unwrap_or(0) as u16,
    })
}

fn guard_of(name: &str) -> Result<Guard, String> {
    GUARDS
        .iter()
        .copied()
        .find(|guard| guard.as_str() == name)
        .ok_or_else(|| format!("`{name}` is not a guard"))
}

fn response_of(name: &str) -> Result<FaultResponse, String> {
    for response in [
        FaultResponse::Clamp,
        FaultResponse::Hold,
        FaultResponse::StopCat2,
        FaultResponse::StopCat1,
        FaultResponse::StopCat0,
    ] {
        if response.as_str() == name {
            return Ok(response);
        }
    }
    Err(format!("`{name}` is not a fault response"))
}

fn event_code_of(name: &str) -> Result<EventCode, String> {
    (1..=12)
        .filter_map(EventCode::from_u16)
        .find(|code| code.as_str() == name)
        .ok_or_else(|| format!("`{name}` is not an event code"))
}

fn gap_reason_of(name: &str) -> Result<GapReason, String> {
    (1..=4)
        .filter_map(GapReason::from_u16)
        .find(|reason| reason.as_str() == name)
        .ok_or_else(|| format!("`{name}` is not a gap reason"))
}

fn sized(size: usize, hex: String) -> Result<String, String> {
    let mut document = Json::obj();
    document.set("size", Json::Num(size as f64));
    document.set("hex", Json::Str(hex));
    document.to_canonical().map_err(|error| error.to_string())
}

/// `blackbox/schemas`: one channel's embedded schema.
pub fn schemas(input: &Json) -> Result<String, String> {
    let topic = input
        .get("channel")
        .and_then(Json::as_str)
        .ok_or("no `channel`")?;
    let wire = schema::wire();
    let found = wire
        .iter()
        .find(|w| w.channel.topic == topic)
        .ok_or_else(|| format!("`{topic}` is not a channel this edition writes"))?;
    let mut document = Json::obj();
    document.set("id", Json::Num(found.channel.id as f64));
    document.set("topic", Json::Str(found.channel.topic.into()));
    document.set("name", Json::Str(found.schema.name.into()));
    document.set("definition", Json::Str(found.schema.definition.clone()));
    document.to_canonical().map_err(|error| error.to_string())
}

/// `blackbox/records`: one encoded record.
pub fn records(input: &Json) -> Result<String, String> {
    let shape = shape_of(input)?;
    let sequence = u64_of(input, "sequence")?;
    let mono_ns = u64_of(input, "mono_ns")?;
    let entry = input.get("entry").ok_or("no `entry`")?;
    let kind = entry.get("kind").and_then(Json::as_str).unwrap_or("");

    let mut slot = vec![0u8; shape.record_size()];

    match kind {
        "cycle" => {
            let observation = digest32(
                entry
                    .get("observation")
                    .and_then(Json::as_str)
                    .ok_or("no `observation`")?,
            )?;
            let tripped: Vec<Guard> = entry
                .arr_or_empty("tripped")
                .iter()
                .filter_map(Json::as_str)
                .map(guard_of)
                .collect::<Result<_, _>>()?;
            let response = match entry.get_present("response").and_then(Json::as_str) {
                Some(name) => Some(response_of(name)?),
                None => None,
            };
            let mut base_pose = [f32::NAN; 7];
            for (index, value) in f32s(entry, "base_pose").into_iter().enumerate().take(7) {
                base_pose[index] = value;
            }
            let mut base_twist = [f32::NAN; 6];
            for (index, value) in f32s(entry, "base_twist").into_iter().enumerate().take(6) {
                base_twist[index] = value;
            }
            let desired = f32s(entry, "desired");
            let command = f64s(entry, "command");
            let mask: Vec<u8> = entry
                .arr_or_empty("mask")
                .iter()
                .map(|value| value.as_f64().unwrap_or(0.0) as u8)
                .collect();
            let clamped: Vec<bool> = entry
                .arr_or_empty("clamped")
                .iter()
                .map(|value| value.as_bool().unwrap_or(false))
                .collect();
            let position = f32s(entry, "position");
            let velocity = f32s(entry, "velocity");
            let effort = f32s(entry, "effort");

            let view = CycleView {
                tick: u64_of(entry, "tick")?,
                observation,
                policy: u32_of(entry, "policy")? as u16,
                state_flags: u32_of(entry, "state_flags")? as u16,
                fault_code: u32_of(entry, "fault_code")?,
                read_result: entry
                    .get("read_result")
                    .and_then(Json::as_f64)
                    .unwrap_or(0.0) as i32,
                rhal_result: entry
                    .get("rhal_result")
                    .and_then(Json::as_f64)
                    .unwrap_or(0.0) as i32,
                state_seq: u32_of(entry, "state_seq")?,
                state_stamp_ns: u64_of(entry, "state_stamp_ns")?,
                command_stamp_ns: u64_of(entry, "command_stamp_ns")?,
                command_seq: u32_of(entry, "command_seq")?,
                mode: u32_of(entry, "mode")? as u8,
                advisory: entry
                    .get("advisory")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
                response,
                tripped: &tripped,
                capability: u32_of(entry, "capability")? as u16,
                nearest_person_m: entry
                    .get_present("nearest_person_m")
                    .and_then(Json::as_f64)
                    .unwrap_or(f64::NAN) as f32,
                base_pose,
                base_twist,
                desired: &desired,
                command: &command,
                mask: &mask,
                clamped: &clamped,
                position: &position,
                velocity: &velocity,
                effort: &effort,
            };
            encode(shape, &mut slot, sequence, mono_ns, Entry::Cycle(&view))
                .map_err(|error| format!("{error:?}"))?;
        }
        "frame" => {
            let observation = digest32(
                entry
                    .get("observation")
                    .and_then(Json::as_str)
                    .ok_or("no `observation`")?,
            )?;
            let streams: Vec<(String, u32, bool)> = entry
                .arr_or_empty("streams")
                .iter()
                .map(|stream| {
                    (
                        stream
                            .get("name")
                            .and_then(Json::as_str)
                            .unwrap_or("")
                            .to_string(),
                        stream.get("age_us").and_then(Json::as_f64).unwrap_or(0.0) as u32,
                        stream.get("stale").and_then(Json::as_bool).unwrap_or(false),
                    )
                })
                .collect();
            let borrowed: Vec<(&str, u32, bool)> = streams
                .iter()
                .map(|(name, age, stale)| (name.as_str(), *age, *stale))
                .collect();
            let view = FrameView {
                observation,
                target_ns: u64_of(entry, "target_ns")?,
                // Present-and-null in the case file means "not measurable", which is a value the
                // record carries rather than a field it omits — so an absent key is still a broken
                // case rather than a quiet `None`.
                worst_skew_us: match entry.get("worst_skew_us") {
                    Some(skynet_json::Json::Null) => None,
                    _ => Some(u32_of(entry, "worst_skew_us")?),
                },
                streams_omitted: u32_of(entry, "streams_omitted")? as u16,
                streams_absent: u32_of(entry, "streams_absent")? as u16,
                streams: &borrowed,
            };
            encode(shape, &mut slot, sequence, mono_ns, Entry::Frame(&view))
                .map_err(|error| format!("{error:?}"))?;
        }
        "event" => {
            let view = EventView {
                code: event_code_of(
                    entry
                        .get("code")
                        .and_then(Json::as_str)
                        .ok_or("no `code`")?,
                )?,
                subject: entry.get("subject").and_then(Json::as_str).unwrap_or(""),
                detail: entry.get("detail").and_then(Json::as_str).unwrap_or(""),
            };
            encode(shape, &mut slot, sequence, mono_ns, Entry::Event(&view))
                .map_err(|error| format!("{error:?}"))?;
        }
        "gap" => {
            let gap = Gap {
                dropped: u64_of(entry, "dropped")?,
                first_tick: u64_of(entry, "first_tick")?,
                reason: gap_reason_of(
                    entry
                        .get("reason")
                        .and_then(Json::as_str)
                        .ok_or("no `reason`")?,
                )?,
            };
            encode(shape, &mut slot, sequence, mono_ns, Entry::Gap(gap))
                .map_err(|error| format!("{error:?}"))?;
        }
        other => return Err(format!("`{other}` is not a record kind")),
    }

    sized(slot.len(), mcap::hex(&slot))
}

/// `blackbox/header`: a ring header, with both of its tables.
pub fn header(input: &Json) -> Result<String, String> {
    let geometry_input = input.get("geometry").ok_or("no `geometry`")?;
    let shape = shape_of(geometry_input)?;
    let meta_input = input.get("meta").ok_or("no `meta`")?;

    let geometry = Geometry {
        shape,
        record_size: shape.record_size(),
        capacity: geometry_input
            .get("capacity")
            .and_then(Json::as_usize)
            .unwrap_or(0),
        header_len: 0, // Replaced below: the oracle and this edition must compute it, not read it.
        digests: geometry_input
            .get("digests")
            .and_then(Json::as_usize)
            .unwrap_or(0) as u16,
        capabilities: geometry_input
            .get("capabilities")
            .and_then(Json::as_usize)
            .unwrap_or(0) as u16,
    };
    // `for_budget` is the only public way to get a header length, and it is the arithmetic under
    // test: asking it for exactly the bytes this geometry needs makes it produce them.
    let sized_geometry = Geometry::for_budget(
        shape,
        geometry.digests,
        geometry.capabilities,
        Geometry::budget_for(
            shape,
            geometry.digests,
            geometry.capabilities,
            1.0,
            geometry.capacity as f64,
        ),
    )
    .map_err(|error| format!("{error:?}"))?;

    let meta = Meta {
        robot_id: meta_input
            .get("robot_id")
            .and_then(Json::as_str)
            .unwrap_or("")
            .to_string(),
        embodiment: digest32(
            meta_input
                .get("embodiment")
                .and_then(Json::as_str)
                .ok_or("no `embodiment`")?,
        )?,
        os_image: digest32(
            meta_input
                .get("os_image")
                .and_then(Json::as_str)
                .ok_or("no `os_image`")?,
        )?,
        tier: u32_of(meta_input, "tier")? as u8,
        enforcement: u32_of(meta_input, "enforcement")? as u8,
        simulator: meta_input
            .get("simulator")
            .and_then(Json::as_bool)
            .unwrap_or(false),
        wall_epoch_ns: u64_of(meta_input, "wall_epoch_ns")?,
        mono_ns: u64_of(meta_input, "mono_ns")?,
        channels: meta_input.str_arr("channels"),
    };

    let (mut ring, mut store) = Ring::create(sized_geometry, meta);
    for digest in input
        .arr_or_empty("digests")
        .iter()
        .filter_map(Json::as_str)
    {
        ring.intern_digest(&mut store, digest32(digest)?)
            .map_err(|error| format!("{error:?}"))?;
    }
    for token in input
        .arr_or_empty("capabilities")
        .iter()
        .filter_map(Json::as_str)
    {
        ring.intern_capability(&mut store, token)
            .map_err(|error| format!("{error:?}"))?;
    }

    let header_len = ring.geometry().header_len;
    let mut document = Json::obj();
    document.set("size", Json::Num(header_len as f64));
    document.set("record_size", Json::Num(shape.record_size() as f64));
    document.set("hex", Json::Str(mcap::hex(&store[..header_len])));
    document.to_canonical().map_err(|error| error.to_string())
}

/// `blackbox/seal`: a whole sealed file, held by its length and its digest.
///
/// The digest rather than the hex, because every *component* of the file is already pinned exactly
/// by the other three corpora, and what is left for this case to hold is the framing and the order.
/// Eighty kilobytes of hex would hold the same property and be unreviewable.
pub fn seal(input: &Json) -> Result<String, String> {
    let library = input
        .get("library")
        .and_then(Json::as_str)
        .ok_or("no `library`")?;
    let mut writer = Writer::new(library);
    let wire = schema::wire();
    for entry in &wire {
        writer.schema(entry.schema.clone());
    }
    for entry in &wire {
        writer.channel(entry.channel);
    }

    if let Some(metadata) = input.get("metadata") {
        let name = metadata.get("name").and_then(Json::as_str).unwrap_or("");
        let pairs: Vec<(String, String)> = metadata
            .arr_or_empty("pairs")
            .iter()
            .filter_map(|pair| {
                let entries = pair.as_arr()?;
                Some((
                    entries.first()?.as_str()?.to_string(),
                    entries.get(1)?.as_str()?.to_string(),
                ))
            })
            .collect();
        writer.metadata(name, &pairs);
    }

    for message in input.arr_or_empty("messages") {
        let channel_id = message
            .get("channel_id")
            .and_then(Json::as_usize)
            .ok_or("no `channel_id`")? as u16;
        let log_time = u64_of(message, "log_time")?;
        let payload = message
            .get("payload")
            .and_then(Json::as_str)
            .ok_or("no `payload`")?;
        writer.message(channel_id, log_time, payload);
    }

    let bytes = writer.finish();
    let mut document = Json::obj();
    document.set("size", Json::Num(bytes.len() as f64));
    document.set(
        "sha256",
        Json::Str(mcap::hex(&skynet_sha256::digest(&bytes))),
    );
    document.set("opcodes", Json::Str(opcodes(&bytes)?));
    document.to_canonical().map_err(|error| error.to_string())
}

/// The file's record opcodes, in order, as two-digit hex.
///
/// Pinned beside the digest because a digest says *that* two editions disagree and this says
/// *where*. The first divergence this corpus found was an ordering one — schemas interleaved with
/// channels in one edition and grouped in the other — and it is legible in this string and in
/// nothing else short of a hex dump.
///
/// Walking the framing is also the check that the framing walks: a file whose record lengths do not
/// land exactly on the trailing magic is a file no reader will open.
fn opcodes(bytes: &[u8]) -> Result<String, String> {
    let mut at = 8;
    let end = bytes.len() - 8;
    let mut found = Vec::new();
    while at < end {
        if at + 9 > end {
            return Err(format!(
                "a record header runs past the end of the file at {at}"
            ));
        }
        let mut length = [0u8; 8];
        length.copy_from_slice(&bytes[at + 1..at + 9]);
        found.push(format!("{:02x}", bytes[at]));
        at += 9 + u64::from_le_bytes(length) as usize;
    }
    if at != end {
        return Err(format!(
            "the framing ended at {at} and the trailing magic is at {end}"
        ));
    }
    Ok(found.join(","))
}

//! The six channels of §26, and the schemas embedded beside them.
//!
//! | Channel | Rate | Carries |
//! |---|---|---|
//! | `/sk/state` | joint rate | `rhal_state` verbatim |
//! | `/sk/desired` | policy rate | what the policy asked for |
//! | `/sk/verdict` | joint rate | guards tripped, clamps applied, response |
//! | `/sk/command` | joint rate | what actually went to `write_command`, and its return |
//! | `/sk/frame` | policy rate | observation hashes, per-stream ages, worst skew |
//! | `/sk/event` | on change | lease grants, pre-emptions, estop, OTA, grant changes |
//!
//! # These are a single-source artifact, and that is why they are built rather than written
//!
//! §31.2 puts "MCAP channel schemas" in the short list of things that stay single-source however
//! many editions exist, because *a black box sealed by one edition must be readable by every
//! other*. The obvious way to hold that is a string literal copied into each edition, and the
//! obvious way for it to fail is somebody reformatting one copy.
//!
//! So the schemas are **constructed** and then serialised through canonical JSON — the one
//! serialisation this project already dual-implements, already corpus-tests, and already requires
//! every edition to reproduce byte for byte (§31.4). Two editions building the same document emit
//! the same bytes because the canonical form leaves them no choice, rather than because two people
//! kept two literals in step. The corpus pins the result anyway, so a divergence is caught at the
//! byte.
//!
//! # `null` where a sensor said `NaN`
//!
//! §24 uses `NaN` for "this joint does not measure it" — an unmeasured velocity is not a velocity
//! of zero, and the ABI is careful about the difference. JSON has no `NaN`, and canonical JSON
//! refuses to emit one rather than quietly writing `null` (`skynet-json`'s rule 3). The mapping is
//! therefore explicit and one-way: an unmeasured value is `null` in the sealed file, which is the
//! same fact in a format that can hold it. A reader must not read `null` as zero, and the schema
//! says so in each field's description.

use alloc::string::String;
use alloc::vec::Vec;

use skynet_json::Json;

use crate::mcap::{Channel, Schema};

/// One channel and the schema it publishes, ready for [`crate::mcap::Writer`].
#[derive(Debug, Clone)]
pub struct Wire {
    /// The schema, with its document already serialised.
    pub schema: Schema,
    /// The channel that refers to it.
    pub channel: Channel,
}

/// Channel ids, fixed for the life of the format.
///
/// A reader that has learned "channel 3 is the verdict" from one file must not be wrong about the
/// next one, so these are constants rather than an enumeration order.
pub mod id {
    /// `/sk/state`.
    pub const STATE: u16 = 1;
    /// `/sk/desired`.
    pub const DESIRED: u16 = 2;
    /// `/sk/verdict`.
    pub const VERDICT: u16 = 3;
    /// `/sk/command`.
    pub const COMMAND: u16 = 4;
    /// `/sk/frame`.
    pub const FRAME: u16 = 5;
    /// `/sk/event`.
    pub const EVENT: u16 = 6;
}

fn field(kind: &str, description: &str) -> Json {
    let mut property = Json::obj();
    property.set("type", Json::Str(kind.into()));
    property.set("description", Json::Str(description.into()));
    property
}

/// A field that is a string or `null` — a fact with no value this cycle.
fn maybe_string(description: &str) -> Json {
    let mut property = Json::obj();
    let mut kinds = Json::arr();
    kinds.push(Json::Str("string".into()));
    kinds.push(Json::Str("null".into()));
    property.set("type", kinds);
    property.set("description", Json::Str(description.into()));
    property
}

/// A field that is a number or `null` — an unmeasured value (§24).
fn maybe_number(description: &str) -> Json {
    let mut property = Json::obj();
    let mut kinds = Json::arr();
    kinds.push(Json::Str("number".into()));
    kinds.push(Json::Str("null".into()));
    property.set("type", kinds);
    property.set("description", Json::Str(description.into()));
    property
}

/// A field that is an integer or `null` — a quantity this frame could not measure.
fn maybe_integer(description: &str) -> Json {
    let mut property = Json::obj();
    let mut kinds = Json::arr();
    kinds.push(Json::Str("integer".into()));
    kinds.push(Json::Str("null".into()));
    property.set("type", kinds);
    property.set("description", Json::Str(description.into()));
    property
}

fn array_of(items: Json, description: &str) -> Json {
    let mut property = Json::obj();
    property.set("type", Json::Str("array".into()));
    property.set("items", items);
    property.set("description", Json::Str(description.into()));
    property
}

fn number_array(description: &str) -> Json {
    let mut items = Json::obj();
    let mut kinds = Json::arr();
    kinds.push(Json::Str("number".into()));
    kinds.push(Json::Str("null".into()));
    items.set("type", kinds);
    array_of(items, description)
}

fn object(properties: &[(&str, Json)], required: &[&str], description: &str) -> Json {
    let mut schema = Json::obj();
    schema.set("type", Json::Str("object".into()));
    schema.set("description", Json::Str(description.into()));
    let mut props = Json::obj();
    for (name, value) in properties {
        props.set(name, value.clone());
    }
    schema.set("properties", props);
    let mut list = Json::arr();
    for name in required {
        list.push(Json::Str((*name).into()));
    }
    schema.set("required", list);
    schema
}

/// The `mono_ns` field every message carries.
///
/// Present on all six channels, and not merely a duplicate of MCAP's `log_time`. `log_time` is
/// nanoseconds since the epoch, because that is the only thing a general reader can render; every
/// control decision on the robot was made against `CLOCK_MONOTONIC_RAW` (§35.1). Carrying both
/// means a wall-clock step is *visible* in the file — the two disagree across it — instead of
/// silently displacing an hour of the timeline.
fn mono() -> Json {
    field(
        "integer",
        "CLOCK_MONOTONIC_RAW at the instant this record was produced. The time base every control \
         decision was made against (§35.1); MCAP's log_time is the wall-clock rendering of it.",
    )
}

fn state() -> Json {
    object(
        &[
            ("mono_ns", mono()),
            (
                "stamp_ns",
                field(
                    "integer",
                    "rhal_state.stamp_ns: CLOCK_MONOTONIC_RAW at acquisition, never at return (§24).",
                ),
            ),
            ("seq", field("integer", "rhal_state.seq.")),
            (
                "flags",
                field("integer", "Bitwise-or of the RHAL flags: estop, fault, degraded, braked."),
            ),
            (
                "fault_code",
                field("integer", "Vendor-specific, surfaced verbatim (§24)."),
            ),
            (
                "read_result",
                maybe_integer(
                    "What read_state returned this cycle: 0, or a negative errno (§24). Null in a \
                     recording sealed before the field existed, which is not the same as a read \
                     that succeeded (§26.4).",
                ),
            ),
            (
                "nearest_person_m",
                maybe_number(
                    "Distance to the nearest detected person, in metres; null when nothing was measuring \
                     it. An input to the proximity guard rather than a reading the RHAL \
                     supplies (§7.1).",
                ),
            ),
            ("position_rad", number_array("Measured joint positions.")),
            (
                "velocity_rad_s",
                number_array("Measured velocities; null where the joint does not measure it."),
            ),
            (
                "effort_nm",
                number_array("Measured efforts; null where unmeasured."),
            ),
            (
                "base_pose",
                number_array("x y z qx qy qz qw; all null when the base is not localised."),
            ),
            ("base_twist", number_array("vx vy vz wx wy wz.")),
        ],
        &[
            "mono_ns",
            "stamp_ns",
            "seq",
            "flags",
            "fault_code",
            "read_result",
            "nearest_person_m",
            "position_rad",
            "velocity_rad_s",
            "effort_nm",
            "base_pose",
            "base_twist",
        ],
        "One sample of the robot's state: what the RHAL reported (§24), whether the read \
         succeeded, and the proximity reading the guards saw.",
    )
}

fn channel_values(description: &str) -> Json {
    let item = object(
        &[
            ("channel", field("string", "The action channel's name.")),
            ("value", maybe_number("The value on that channel.")),
            (
                "hold",
                field(
                    "boolean",
                    "True where the mask says hold this joint at its current setpoint — the OS \
                     saying nothing rather than saying zero (§24).",
                ),
            ),
        ],
        &["channel", "value", "hold"],
        "One channel's value.",
    );
    array_of(item, description)
}

fn desired() -> Json {
    object(
        &[
            ("mono_ns", mono()),
            ("tick", field("integer", "The control tick.")),
            (
                "stamp_ns",
                field("integer", "When the policy produced this command, not when it was sent."),
            ),
            ("seq", field("integer", "rhal_command.seq; must advance (§24).")),
            (
                "mode",
                field("string", "position, velocity, torque, or unknown."),
            ),
            (
                "policy_digest",
                field(
                    "string",
                    "Hex SHA-256 of the package version that produced this, or empty when the \
                     recording's digest table did not hold it.",
                ),
            ),
            (
                "capability",
                maybe_string(
                    "The capability token this command exercised, or null when the runtime did not say \
                     — which is not the same as no capability, and the mode gate treats the two \
                     differently (§7.2).",
                ),
            ),
            (
                "observation",
                field(
                    "string",
                    "Hex SHA-256 of the observation frame the policy was given. A hash, never the \
                     bytes: an hour of three-camera video is not something the robot can keep (§26).",
                ),
            ),
            ("values", channel_values("What the policy asked for.")),
        ],
        &[
            "mono_ns",
            "tick",
            "stamp_ns",
            "seq",
            "mode",
            "policy_digest",
            "capability",
            "observation",
            "values",
        ],
        "What the policy asked for, before the Safety Kernel saw it.",
    )
}

fn verdict() -> Json {
    let clamp = object(
        &[
            ("channel", field("string", "Which channel was reduced.")),
            ("from", maybe_number("What the policy asked for.")),
            ("to", maybe_number("What the kernel sent instead.")),
        ],
        &["channel", "from", "to"],
        "One clamped channel, and by how much.",
    );
    let mut guards = Json::obj();
    guards.set("type", Json::Str("string".into()));
    guards.set(
        "description",
        Json::Str(
            "A guard name, in evaluation order: watchdog, envelope, rate_limit, workspace, \
             self_collision, balance, proximity, mode_gate (§7.1)."
                .into(),
        ),
    );
    object(
        &[
            ("mono_ns", mono()),
            ("tick", field("integer", "The control tick.")),
            (
                "response",
                maybe_string(
                    "clamp, hold, stop_cat2, stop_cat1 or stop_cat0 — the strongest response \
                     any tripped guard demanded. Null when nothing tripped, which is a fact \
                     rather than an absence and so is present-and-null.",
                ),
            ),
            ("tripped", array_of(guards, "Guards that tripped.")),
            (
                "clamped",
                array_of(clamp, "Channels whose values were reduced."),
            ),
            (
                "advisory",
                field(
                    "boolean",
                    "True when the tier could only advise: the command was forwarded unchanged \
                     despite a trip. Always false at host and certified (§42).",
                ),
            ),
        ],
        &[
            "mono_ns", "tick", "response", "tripped", "clamped", "advisory",
        ],
        "What the Safety Kernel decided this cycle.",
    )
}

fn command() -> Json {
    object(
        &[
            ("mono_ns", mono()),
            ("tick", field("integer", "The control tick.")),
            (
                "values",
                channel_values("What actually went to write_command."),
            ),
            (
                "rhal_result",
                field(
                    "integer",
                    "What write_command returned: 0, or a negative errno. -1 EPERM because the \
                     drive is faulted is a different fact from -110 ETIMEDOUT (§24).",
                ),
            ),
        ],
        &["mono_ns", "tick", "values", "rhal_result"],
        "What was sent to the hardware, and what the hardware said about it.",
    )
}

fn frame() -> Json {
    let stream = object(
        &[
            ("name", field("string", "The stream's name.")),
            (
                "age_us",
                field(
                    "integer",
                    "How far back this stream's sample was, relative to the instant the assembler \
                     targeted.",
                ),
            ),
            (
                "stale",
                field(
                    "boolean",
                    "Whether that exceeded the policy's declared staleness budget, which is a hold \
                     rather than a silently older frame (§25.1).",
                ),
            ),
        ],
        &["name", "age_us", "stale"],
        "One stream's contribution to the frame.",
    );
    object(
        &[
            ("mono_ns", mono()),
            (
                "observation",
                field("string", "Hex SHA-256 of the assembled frame."),
            ),
            (
                "target_ns",
                field("integer", "The instant the assembler targeted."),
            ),
            (
                "worst_skew_us",
                maybe_integer(
                    "The worst skew between any stream and that instant. Recorded so that 'the \
                     policy behaved oddly' can be checked against 'the frame it was given was 80 ms \
                     skewed' months later (§25.1). Null when fewer than two streams contributed: \
                     the spread of one instant is undefined rather than zero, and zero would make a \
                     frame with three dead capture paths read as a perfectly synchronised one.",
                ),
            ),
            (
                "streams_omitted",
                field(
                    "integer",
                    "Streams the record could not carry, counted rather than dropped. A limit of \
                     the format.",
                ),
            ),
            (
                "streams_absent",
                field(
                    "integer",
                    "Streams the policy declared that produced no sample at all. A limit of the \
                     robot, and deliberately not the field above: an investigation reading one as \
                     the other goes looking for a bigger record instead of a dead camera.",
                ),
            ),
            (
                "streams",
                array_of(stream, "Per-stream ages, for the streams that had one."),
            ),
        ],
        &[
            "mono_ns",
            "observation",
            "target_ns",
            "worst_skew_us",
            "streams_omitted",
            "streams_absent",
            "streams",
        ],
        "One assembled observation frame, and how well aligned it was.",
    )
}

fn event() -> Json {
    object(
        &[
            ("mono_ns", mono()),
            (
                "event",
                field(
                    "string",
                    "lease_granted, lease_released, lease_preempted, estop, estop_cleared, \
                     package_started, package_stopped, grant_changed, ota, clock_step, \
                     recording_started, sealed, config_changed, admission, skill_transition, or \
                     gap.",
                ),
            ),
            (
                "subject",
                field("string", "What it happened to: a package id, a capability token, a slot."),
            ),
            ("detail", field("string", "A sentence about it.")),
            (
                "dropped",
                field(
                    "integer",
                    "Gap events only: how many records were not written. A gap annotated as a gap \
                     is honest; a gap that looks like a quiet period is not (§47.4).",
                ),
            ),
            (
                "first_tick",
                field("integer", "Gap events only: the first tick that went unrecorded."),
            ),
            (
                "reason",
                field(
                    "string",
                    "Gap events only: writer_behind, encoder_behind, not_recording or write_failed.",
                ),
            ),
        ],
        &["mono_ns", "event", "subject", "detail"],
        "Something that changed, and every gap in the recording.",
    )
}

/// The six channels, in id order, with their schemas serialised.
///
/// Panics only if canonical serialisation fails, which it cannot here: every value in these
/// documents is a string, an array or an object, and the failure modes of the canonical form are
/// non-finite numbers.
pub fn wire() -> Vec<Wire> {
    let mut wires = Vec::new();
    for (id, topic, name, document) in [
        (id::STATE, "/sk/state", "sk_state", state()),
        (id::DESIRED, "/sk/desired", "sk_desired", desired()),
        (id::VERDICT, "/sk/verdict", "sk_verdict", verdict()),
        (id::COMMAND, "/sk/command", "sk_command", command()),
        (id::FRAME, "/sk/frame", "sk_frame", frame()),
        (id::EVENT, "/sk/event", "sk_event", event()),
    ] {
        let definition: String = document
            .to_canonical()
            .expect("a schema document holds no numbers, so it cannot fail to serialise");
        wires.push(Wire {
            schema: Schema {
                id,
                name,
                definition,
            },
            channel: Channel {
                id,
                schema_id: id,
                topic,
            },
        });
    }
    wires
}

//! Owned buffers for the kernel's borrowed inputs.
//!
//! [`skynet_kernel`] takes `&str` and `&[(&str, f64)]` throughout, because it is `no_std` with no
//! allocator and the frames it reads are preallocated by the runtime that owns them. A corpus
//! runner has the opposite situation: it has just parsed a JSON document and needs somewhere for
//! the strings to live while the kernel borrows them.
//!
//! So these two types are the join. They own; the kernel borrows. It is a small amount of
//! ceremony, and it is the same ceremony `skynetd` will perform for real — the runtime owns the
//! command frame, the kernel reads it, and nothing in the hot path allocates.

use skynet_json::Json;
use skynet_kernel::{
    Available, CommandCycle, Envelope, OperatorProfile, Proximity, ResolveLimitsParams, Tier,
    TransportClamp,
};

/// Everything [`skynet_kernel::resolve_limits`] needs, owned.
pub struct LimitsOwned {
    tier: Tier,
    envelope: Option<EnvelopeOwned>,
    transport: Option<ClampOwned>,
    profile: Option<ProfileOwned>,
    available: Available,
}

struct EnvelopeOwned {
    joint_velocity_max_rad_s: Option<f64>,
    joint_torque_max_nm: Option<f64>,
    base_velocity_max_m_s: Option<f64>,
    watchdog_ms: Option<f64>,
    estop: Option<bool>,
    source: Option<String>,
}

struct ClampOwned {
    joint_velocity_max_rad_s: Option<f64>,
    joint_torque_max_nm: Option<f64>,
    base_velocity_max_m_s: Option<f64>,
    min_control_hz: Option<f64>,
    per_channel: Vec<(String, f64)>,
    source: Option<String>,
}

struct ProfileOwned {
    name: String,
    joint_velocity_max_rad_s: Option<f64>,
    joint_torque_max_nm: Option<f64>,
    base_velocity_max_m_s: Option<f64>,
    max_delta_per_cycle: Option<f64>,
    proximity: Option<Proximity>,
}

fn num(value: &Json, key: &str) -> Option<f64> {
    value.get_present(key)?.as_f64()
}

fn text(value: &Json, key: &str) -> Option<String> {
    Some(value.get_present(key)?.as_str()?.to_string())
}

impl LimitsOwned {
    /// Decode a `ResolveLimitsParams` document.
    pub fn from_json(value: &Json) -> Result<LimitsOwned, String> {
        let tier_text = value
            .get("tier")
            .and_then(Json::as_str)
            .ok_or("no `tier`")?;
        let tier = Tier::parse(tier_text).ok_or_else(|| format!("unknown tier `{tier_text}`"))?;

        let envelope = value.get_present("envelope").map(|node| EnvelopeOwned {
            joint_velocity_max_rad_s: num(node, "joint_velocity_max_rad_s"),
            joint_torque_max_nm: num(node, "joint_torque_max_nm"),
            base_velocity_max_m_s: num(node, "base_velocity_max_m_s"),
            watchdog_ms: num(node, "watchdog_ms"),
            estop: node.get_present("estop").and_then(Json::as_bool),
            source: text(node, "source"),
        });

        let transport = value.get_present("transport").map(|node| {
            let per_channel = node
                .get("per_channel")
                .and_then(Json::as_obj)
                .map(|map| {
                    map.iter()
                        .filter_map(|(name, value)| {
                            value.as_f64().map(|limit| (name.clone(), limit))
                        })
                        .collect()
                })
                .unwrap_or_default();
            ClampOwned {
                joint_velocity_max_rad_s: num(node, "joint_velocity_max_rad_s"),
                joint_torque_max_nm: num(node, "joint_torque_max_nm"),
                base_velocity_max_m_s: num(node, "base_velocity_max_m_s"),
                min_control_hz: num(node, "min_control_hz"),
                per_channel,
                source: text(node, "source"),
            }
        });

        let profile = value.get_present("profile").map(|node| ProfileOwned {
            name: text(node, "name").unwrap_or_default(),
            joint_velocity_max_rad_s: num(node, "joint_velocity_max_rad_s"),
            joint_torque_max_nm: num(node, "joint_torque_max_nm"),
            base_velocity_max_m_s: num(node, "base_velocity_max_m_s"),
            max_delta_per_cycle: num(node, "max_delta_per_cycle"),
            proximity: node.get_present("proximity").and_then(|prox| {
                Some(Proximity {
                    stop_distance_m: num(prox, "stop_distance_m")?,
                    slow_distance_m: num(prox, "slow_distance_m")?,
                    slow_factor: num(prox, "slow_factor")?,
                })
            }),
        });

        let available = value
            .get_present("available")
            .map(|node| Available {
                imu: node.get("imu").and_then(Json::as_bool).unwrap_or(false),
                people_detection: node
                    .get("people_detection")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
                collision_model: node
                    .get("collision_model")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
                solvers: node.get("solvers").and_then(Json::as_bool).unwrap_or(false),
                legged: node.get("legged").and_then(Json::as_bool).unwrap_or(false),
            })
            .unwrap_or_default();

        Ok(LimitsOwned {
            tier,
            envelope,
            transport,
            profile,
            available,
        })
    }

    /// A borrowed view of the envelope.
    pub fn envelope(&self) -> Option<Envelope<'_>> {
        self.envelope.as_ref().map(|owned| Envelope {
            joint_velocity_max_rad_s: owned.joint_velocity_max_rad_s,
            joint_torque_max_nm: owned.joint_torque_max_nm,
            base_velocity_max_m_s: owned.base_velocity_max_m_s,
            watchdog_ms: owned.watchdog_ms,
            estop: owned.estop,
            source: owned.source.as_deref(),
        })
    }

    /// Per-channel ceilings as the kernel wants them.
    pub fn per_channel_refs(&self) -> Vec<(&str, f64)> {
        self.transport
            .as_ref()
            .map(|clamp| {
                clamp
                    .per_channel
                    .iter()
                    .map(|(name, limit)| (name.as_str(), *limit))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The profile's name, for rendering the widened-limits notice.
    pub fn profile_name(&self) -> Option<&str> {
        self.profile.as_ref().map(|profile| profile.name.as_str())
    }

    /// Assemble the borrowed parameter struct.
    pub fn params<'a>(&'a self, per_channel: &'a [(&'a str, f64)]) -> ResolveLimitsParams<'a> {
        ResolveLimitsParams {
            tier: self.tier,
            envelope: self.envelope(),
            transport: self.transport.as_ref().map(|clamp| TransportClamp {
                joint_velocity_max_rad_s: clamp.joint_velocity_max_rad_s,
                joint_torque_max_nm: clamp.joint_torque_max_nm,
                base_velocity_max_m_s: clamp.base_velocity_max_m_s,
                min_control_hz: clamp.min_control_hz,
                per_channel,
                source: clamp.source.as_deref(),
            }),
            profile: self.profile.as_ref().map(|profile| OperatorProfile {
                name: profile.name.as_str(),
                joint_velocity_max_rad_s: profile.joint_velocity_max_rad_s,
                joint_torque_max_nm: profile.joint_torque_max_nm,
                base_velocity_max_m_s: profile.base_velocity_max_m_s,
                max_delta_per_cycle: profile.max_delta_per_cycle,
                proximity: profile.proximity,
            }),
            available: self.available,
        }
    }
}

/// One cycle vector, owned.
pub struct CycleOwned {
    now_ms: f64,
    command_ms: f64,
    sequence: u64,
    desired: Vec<(String, f64)>,
    previous: Option<Vec<(String, f64)>>,
}

/// The desired channel map and, when the case supplied one, last cycle's.
///
/// Both are `Vec` rather than slices because the borrow has to be materialised somewhere and the
/// kernel wants `&[(&str, f64)]`. The caller holds them for the length of the cycle, which is
/// exactly the lifetime the runtime will hold a real command frame for.
pub type BorrowedChannels<'a> = (Vec<(&'a str, f64)>, Option<Vec<(&'a str, f64)>>);

fn borrow_channels(list: &[(String, f64)]) -> Vec<(&str, f64)> {
    list.iter().map(|(k, v)| (k.as_str(), *v)).collect()
}

fn channel_map(value: Option<&Json>) -> Vec<(String, f64)> {
    value
        .and_then(Json::as_obj)
        .map(|map| {
            map.iter()
                .filter_map(|(name, value)| value.as_f64().map(|v| (name.clone(), v)))
                .collect()
        })
        .unwrap_or_default()
}

impl CycleOwned {
    /// Decode a `CommandCycle` document.
    pub fn from_json(value: &Json) -> Result<CycleOwned, String> {
        Ok(CycleOwned {
            now_ms: value
                .get("now_ms")
                .and_then(Json::as_f64)
                .ok_or("no `now_ms`")?,
            command_ms: value
                .get("command_ms")
                .and_then(Json::as_f64)
                .ok_or("no `command_ms`")?,
            sequence: value
                .get("sequence")
                .and_then(Json::as_f64)
                .ok_or("no `sequence`")? as u64,
            desired: channel_map(value.get("desired")),
            previous: value.get_present("previous").map(|p| channel_map(Some(p))),
        })
    }

    /// Borrowed views of the two channel maps.
    pub fn refs(&self) -> BorrowedChannels<'_> {
        (
            borrow_channels(&self.desired),
            self.previous.as_deref().map(borrow_channels),
        )
    }

    /// Assemble the borrowed cycle.
    ///
    /// `capability` and `nearest_person_m` are always `None`: the reference implementation's cycle
    /// vector carries neither, so a corpus case cannot exercise the mode gate or the proximity
    /// guard. Those two are covered by this edition's own suite instead — and until the reference
    /// grows the fields, a corpus case that set them would be comparing against an expectation
    /// nobody computed.
    pub fn cycle<'a>(
        &self,
        desired: &'a [(&'a str, f64)],
        previous: Option<&'a [(&'a str, f64)]>,
    ) -> CommandCycle<'a> {
        CommandCycle {
            now_ms: self.now_ms,
            command_ms: self.command_ms,
            sequence: self.sequence,
            desired,
            previous,
            capability: None,
            nearest_person_m: None,
        }
    }
}

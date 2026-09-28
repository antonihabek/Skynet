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

use skynet_contracts::embodiment::FrameGraph;
use skynet_contracts::json::frames_from_json;
use skynet_contracts::kinematics::{flatten, workspace_from_json, FlatFrames, WorkspaceOwned};
use skynet_json::Json;
use skynet_kernel::kinematics::{Frame, FrameSet, JointBinding, Volume, WorkspaceLayer};
use skynet_kernel::{
    Available, CommandCycle, Envelope, OperatorProfile, Proximity, ResolveLimitsParams,
    SessionOverride, Tier, TransportClamp,
};

/// Everything [`skynet_kernel::resolve_limits`] needs, owned.
pub struct LimitsOwned {
    tier: Tier,
    envelope: Option<EnvelopeOwned>,
    transport: Option<ClampOwned>,
    profile: Option<ProfileOwned>,
    session: Option<SessionOwned>,
    available: Available,
    grants: Vec<String>,
    /// The body's frames, flattened. `None` where the case declared none, and the two pose guards
    /// then report unavailable exactly as they would on a robot whose descriptor has no tree.
    frames: Option<FlatFrames>,
}

struct EnvelopeOwned {
    joint_velocity_max_rad_s: Option<f64>,
    joint_torque_max_nm: Option<f64>,
    base_velocity_max_m_s: Option<f64>,
    watchdog_ms: Option<f64>,
    estop: Option<bool>,
    source: Option<String>,
    basis: Option<String>,
}

struct ClampOwned {
    joint_velocity_max_rad_s: Option<f64>,
    joint_torque_max_nm: Option<f64>,
    base_velocity_max_m_s: Option<f64>,
    min_control_hz: Option<f64>,
    per_channel: Vec<(String, f64)>,
    joint_modes: Vec<(String, String)>,
    travel: Vec<(String, f64, f64)>,
    source: Option<String>,
}

struct ProfileOwned {
    workspace: WorkspaceOwned,
    self_collision_margin_m: Option<f64>,
    name: String,
    joint_velocity_max_rad_s: Option<f64>,
    joint_torque_max_nm: Option<f64>,
    base_velocity_max_m_s: Option<f64>,
    max_delta_per_cycle: Option<f64>,
    proximity: Option<Proximity>,
    joint_travel: Vec<(String, f64, f64)>,
}

/// §38.2's session layer, as a case declares it.
struct SessionOwned {
    name: String,
    joint_velocity_max_rad_s: Option<f64>,
    joint_torque_max_nm: Option<f64>,
    base_velocity_max_m_s: Option<f64>,
    max_delta_per_cycle: Option<f64>,
}

fn num(value: &Json, key: &str) -> Option<f64> {
    value.get_present(key)?.as_f64()
}

fn text(value: &Json, key: &str) -> Option<String> {
    Some(value.get_present(key)?.as_str()?.to_string())
}

/// A `{ joint: [min, max] }` table (§7.1, F159), in the order the document wrote it.
///
/// Each end is a bare `number` in the reference's type, so a `null` is how the generator spells a
/// non-finite one, and it reads as NaN — which the kernel then declines to narrow anything with
/// (`corpus/README.md`, *A `null` in an input*). An entry that is not a two-element array is not a
/// range at all, and is left out.
fn travel_table(value: &Json, key: &str) -> Vec<(String, f64, f64)> {
    let end = |item: &Json| {
        if matches!(item, Json::Null) {
            Some(f64::NAN)
        } else {
            item.as_f64()
        }
    };
    value
        .get_present(key)
        .and_then(Json::as_obj)
        .map(|map| {
            map.iter()
                .filter_map(|(name, range)| match range.as_arr()? {
                    [min, max] => Some((name.clone(), end(min)?, end(max)?)),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn travel_refs(table: &[(String, f64, f64)]) -> Vec<(&str, f64, f64)> {
    table
        .iter()
        .map(|(name, min, max)| (name.as_str(), *min, *max))
        .collect()
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
            basis: text(node, "basis"),
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
            // §86.3. Absent on every case written before this section, and on every body no joint
            // of which is bound.
            let joint_modes = node
                .get("joint_modes")
                .and_then(Json::as_obj)
                .map(|map| {
                    map.iter()
                        .filter_map(|(name, value)| {
                            value.as_str().map(|mode| (name.clone(), mode.to_string()))
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
                joint_modes,
                travel: travel_table(node, "travel"),
                source: text(node, "source"),
            }
        });

        let session = value.get_present("session").map(|node| SessionOwned {
            name: text(node, "name").unwrap_or_default(),
            joint_velocity_max_rad_s: num(node, "joint_velocity_max_rad_s"),
            joint_torque_max_nm: num(node, "joint_torque_max_nm"),
            base_velocity_max_m_s: num(node, "base_velocity_max_m_s"),
            max_delta_per_cycle: num(node, "max_delta_per_cycle"),
        });

        let profile = value.get_present("profile").map(|node| ProfileOwned {
            workspace: workspace_from_json(node.get_present("workspace")),
            self_collision_margin_m: num(node, "self_collision_margin_m"),
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
            joint_travel: travel_table(node, "joint_travel"),
        });

        let available = value
            .get_present("available")
            .map(|node| Available {
                imu: node.get("imu").and_then(Json::as_bool).unwrap_or(false),
                people_detection: node
                    .get("people_detection")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
                legged: node.get("legged").and_then(Json::as_bool).unwrap_or(false),
                proximity_reading: node
                    .get("proximity_reading")
                    .and_then(Json::as_bool)
                    .unwrap_or(false),
            })
            .unwrap_or_default();

        // A case that declares a frame graph the flattener refuses is a broken case, not a silent
        // pass: the corpus is where a malformed body is supposed to be noticed.
        let frames = match frames_from_json(value.get_present("frames")) {
            None => None,
            Some(graph) => Some(flatten_or_report(&graph)?),
        };

        let grants = value
            .get_present("grants")
            .and_then(Json::as_arr)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        Ok(LimitsOwned {
            session,
            tier,
            envelope,
            transport,
            profile,
            available,
            grants,
            frames,
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
            basis: owned.basis.as_deref(),
        })
    }

    /// Capability tokens the case granted the running package.
    pub fn grant_refs(&self) -> Vec<&str> {
        self.grants.iter().map(String::as_str).collect()
    }

    /// The frames, borrowed, for [`FrameSet::new`].
    pub fn frame_refs(&self) -> Option<Vec<Frame<'_>>> {
        self.frames.as_ref().map(FlatFrames::frames)
    }

    /// The workspace volumes, borrowed one level.
    pub fn volume_refs(&self) -> Vec<Vec<Volume<'_>>> {
        self.profile
            .as_ref()
            .map(|profile| profile.workspace.volumes())
            .unwrap_or_default()
    }

    /// The workspace layers, over a view produced by [`Self::volume_refs`].
    pub fn layer_refs<'a>(&'a self, volumes: &'a [Vec<Volume<'a>>]) -> Vec<WorkspaceLayer<'a>> {
        match &self.profile {
            Some(profile) => profile.workspace.layers(volumes),
            None => Vec::new(),
        }
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

    /// The mode each bound joint is commanded in, as the kernel wants them (§86.3).
    pub fn joint_mode_refs(&self) -> Vec<(&str, &str)> {
        self.transport
            .as_ref()
            .map(|clamp| {
                clamp
                    .joint_modes
                    .iter()
                    .map(|(name, mode)| (name.as_str(), mode.as_str()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The plan's travel, as the kernel wants it (§7.1, F159).
    pub fn travel_refs(&self) -> Vec<(&str, f64, f64)> {
        self.transport
            .as_ref()
            .map(|clamp| travel_refs(&clamp.travel))
            .unwrap_or_default()
    }

    /// The profile's narrowing of it (§7.3, F159).
    pub fn joint_travel_refs(&self) -> Vec<(&str, f64, f64)> {
        self.profile
            .as_ref()
            .map(|profile| travel_refs(&profile.joint_travel))
            .unwrap_or_default()
    }

    /// The profile's name, for rendering the widened-limits notice.
    pub fn profile_name(&self) -> Option<&str> {
        self.profile.as_ref().map(|profile| profile.name.as_str())
    }

    /// Assemble the borrowed parameter struct.
    ///
    /// Four borrowed views go in and one struct comes out. It is more ceremony than a corpus runner
    /// would like, and it is the same ceremony `skynetd` performs for real: the runtime owns the
    /// frame set, the volume lists and the channel map for the length of a lease, and the kernel
    /// borrows all three without allocating.
    pub fn params<'a>(
        &'a self,
        per_channel: &'a [(&'a str, f64)],
        joint_modes: &'a [(&'a str, &'a str)],
        travel: &'a [(&'a str, f64, f64)],
        joint_travel: &'a [(&'a str, f64, f64)],
        frames: Option<&'a [Frame<'a>]>,
        layers: &'a [WorkspaceLayer<'a>],
    ) -> ResolveLimitsParams<'a> {
        ResolveLimitsParams {
            frames: frames.and_then(|frames| FrameSet::new(frames).ok()),
            tier: self.tier,
            envelope: self.envelope(),
            transport: self.transport.as_ref().map(|clamp| TransportClamp {
                joint_velocity_max_rad_s: clamp.joint_velocity_max_rad_s,
                joint_torque_max_nm: clamp.joint_torque_max_nm,
                base_velocity_max_m_s: clamp.base_velocity_max_m_s,
                min_control_hz: clamp.min_control_hz,
                per_channel,
                joint_modes,
                travel,
                source: clamp.source.as_deref(),
            }),
            profile: self.profile.as_ref().map(|profile| OperatorProfile {
                name: profile.name.as_str(),
                joint_velocity_max_rad_s: profile.joint_velocity_max_rad_s,
                joint_torque_max_nm: profile.joint_torque_max_nm,
                base_velocity_max_m_s: profile.base_velocity_max_m_s,
                max_delta_per_cycle: profile.max_delta_per_cycle,
                proximity: profile.proximity,
                workspace: layers,
                self_collision_margin_m: profile.self_collision_margin_m,
                joint_travel,
            }),
            session: self.session.as_ref().map(|session| SessionOverride {
                name: session.name.as_str(),
                joint_velocity_max_rad_s: session.joint_velocity_max_rad_s,
                joint_torque_max_nm: session.joint_torque_max_nm,
                base_velocity_max_m_s: session.base_velocity_max_m_s,
                max_delta_per_cycle: session.max_delta_per_cycle,
            }),
            available: self.available,
        }
    }
}

/// Flatten, or turn the fault into the runner's error string.
fn flatten_or_report(graph: &FrameGraph) -> Result<FlatFrames, String> {
    flatten(graph).map_err(|fault| format!("frames: {} — {}", fault.code, fault.detail))
}

/// The joint bindings a cycle case declares.
///
/// Owned for the same reason everything else here is: the kernel borrows.
pub struct BindingsOwned {
    entries: Vec<(String, Option<String>, f64)>,
}

impl BindingsOwned {
    /// Decode a `joints` list: which channel drives each joint, and where it is now.
    pub fn from_json(value: Option<&Json>) -> BindingsOwned {
        let mut entries = Vec::new();
        for item in value.and_then(Json::as_arr).unwrap_or(&[]) {
            let Some(joint) = item.get("joint").and_then(Json::as_str) else {
                continue;
            };
            entries.push((
                joint.to_string(),
                item.get_present("channel")
                    .and_then(Json::as_str)
                    .map(str::to_string),
                item.get("measured").and_then(Json::as_f64).unwrap_or(0.0),
            ));
        }
        BindingsOwned { entries }
    }

    /// The borrowed view.
    pub fn refs(&self) -> Vec<JointBinding<'_>> {
        self.entries
            .iter()
            .map(|(joint, channel, measured)| JointBinding {
                joint: joint.as_str(),
                channel: channel.as_deref(),
                measured: *measured,
            })
            .collect()
    }
}

/// One cycle vector, owned.
pub struct CycleOwned {
    now_ms: f64,
    command_ms: f64,
    sequence: u64,
    desired: Vec<(String, f64)>,
    previous: Option<Vec<(String, f64)>>,
    capability: Option<String>,
    nearest_person_m: Option<f64>,
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

/// A channel map as an ordered list of pairs.
///
/// Sorted by name, because the underlying JSON object is a map and the kernel is handed a slice —
/// two editions iterating a map in different orders would produce verdicts whose `clamped` lists
/// differ only in order, which the corpus would report as a divergence and a reader would spend an
/// afternoon on.
pub fn channel_pairs(value: Option<&Json>) -> Vec<(String, f64)> {
    channel_map(value)
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
            capability: text(value, "capability"),
            nearest_person_m: num(value, "nearest_person_m"),
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
    /// `capability` and `nearest_person_m` are read from the case. They were hard-wired to `None`
    /// for as long as the reference edition's cycle vector had no such fields, which meant two of
    /// the eight guards were pinned by this edition's own suite and by nothing else — the
    /// arrangement §31.4 exists to prevent. The reference now carries both.
    pub fn cycle<'a>(
        &'a self,
        desired: &'a [(&'a str, f64)],
        previous: Option<&'a [(&'a str, f64)]>,
    ) -> CommandCycle<'a> {
        CommandCycle {
            now_ms: self.now_ms,
            command_ms: self.command_ms,
            sequence: self.sequence,
            desired,
            previous,
            capability: self.capability.as_deref(),
            nearest_person_m: self.nearest_person_m,
        }
    }
}

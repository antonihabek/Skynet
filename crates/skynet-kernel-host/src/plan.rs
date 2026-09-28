//! §6's plan for this body, resolved where the guards run (§6.3, §93).
//!
//! `skynet_contracts::resolve_transport` is the one planner. `skynet transport` runs it for a person,
//! `skynetd` runs it at install and for `GET /limits`, and this module runs it for the loop, over the
//! same two declared documents: the policy's source embodiment and the body's own descriptor. One
//! pure function with three callers cannot hold three answers.
//!
//! # Where the source comes from
//!
//! * **`--source <file>`**, on a bench: a file somebody named, read once at start.
//! * **`--source-from <file>`**, on a robot (§93): the rendezvous `skynetd` writes the commanding
//!   package's source to. It is read at start and again each time a policy connects for the rings.
//!   With nothing there, the plan is the body's own ratings ([`own`]): every joint by exact name, in
//!   the first mode it declares. That is what a policy trained on this body would resolve to, and
//!   it is not a guess about any package.
//!
//! # Narrower now, wider later
//!
//! §33.4: *anything that reduces what the robot may do applies immediately, and anything that
//! increases it waits.* [`Plan::narrows`] is that classifier for a plan, and it is deliberately
//! blunt. A plan that changes a joint's mode is not narrower, whatever its numbers, because a figure
//! in radians and one in newton-metres cannot be compared, and a mode changed under a running policy
//! changes what every one of its commands means.

use skynet_contracts::embodiment::EmbodimentDescriptor;
use skynet_json::Json;

/// What a source resolved to: the channels in plan order, and each one's ceiling, mode and travel.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Channel names, which are joint names: this loop writes channel *i* to joint *i*.
    pub channels: Vec<String>,
    /// Each channel's ceiling, in its own mode's units.
    pub ceilings: Vec<(String, f64)>,
    /// Each joint's mode, as the plan bound it (§86).
    pub modes: Vec<(String, &'static str)>,
    /// Both ends of each position joint's travel (§92).
    pub travel: Vec<(String, f64, f64)>,
    /// One line for the startup report.
    pub summary: String,
    /// Where the plan came from, in words — what `--limits-report` names as its source (§93).
    pub origin: String,
}

/// §93's rendezvous: the commanding package's source when `skynetd` left one, the body's own
/// ratings when it left nothing.
///
/// A file that cannot be read for any reason but its absence is an error rather than the body's own
/// plan. Absent means *no commanding package*; unreadable means something is wrong with the place
/// the supervisor writes to, and running on the body's ratings would hide it.
pub fn at_rendezvous(
    path: &std::path::Path,
    descriptor: &EmbodimentDescriptor,
    hz: f64,
) -> Result<Plan, String> {
    let shown = path.display().to_string();
    match std::fs::read_to_string(path) {
        Ok(text) => from_text(&text, &shown, &shown, descriptor, hz),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => own(
            descriptor,
            hz,
            &format!("the body's own ratings — no package's source at {shown}"),
        ),
        Err(error) => Err(format!("{shown} could not be read: {error}")),
    }
}

/// `--source <file>`: a source somebody named, on a bench.
pub fn from_file(
    path: &std::path::Path,
    descriptor: &EmbodimentDescriptor,
    hz: f64,
) -> Result<Plan, String> {
    let shown = path.display().to_string();
    let label = format!("--source {shown}");
    let text = std::fs::read_to_string(path).map_err(|e| format!("{label}: {e}"))?;
    from_text(&text, &label, &shown, descriptor, hz)
}

/// A source embodiment's text, bare or under `source_embodiment`.
///
/// `package`, beside it, names whose source it is. `skynetd` writes it so a report can say which
/// package's plan the loop holds rather than which file it read.
pub fn from_text(
    text: &str,
    label: &str,
    shown: &str,
    descriptor: &EmbodimentDescriptor,
    hz: f64,
) -> Result<Plan, String> {
    let json = Json::parse(text).map_err(|_| format!("{label} is not JSON"))?;
    let node = json.get("source_embodiment").unwrap_or(&json);
    let source = skynet_contracts::json::source_from_json(node)
        .map_err(|error| format!("{label} is not a source embodiment: {error:?}"))?;
    let origin = match json.get("package").and_then(Json::as_str) {
        Some(package) => format!("`{package}`'s source, from {shown}"),
        None => shown.to_string(),
    };
    resolve(&source, label, origin, descriptor, hz)
}

/// The body's own ratings, as the plan a policy trained on this body would resolve to.
///
/// One channel per joint, by exact name, in joint order, each commanding its joint in the first mode
/// the joint declares — so on a body whose joints sit in two modes, each is bound in its own (§86).
/// A joint that declares no mode leaves no such plan, and that is said rather than filled in.
pub fn own(descriptor: &EmbodimentDescriptor, hz: f64, origin: &str) -> Result<Plan, String> {
    let mut channels = Json::arr();
    let mut first = None;
    for (index, joint) in descriptor.joints.iter().enumerate() {
        let Some(mode) = joint.control_modes.first() else {
            return Err(format!(
                "joint `{}` declares no control mode, so the body has no plan of its own to run a \
                 policy under",
                joint.name
            ));
        };
        first.get_or_insert_with(|| mode.clone());
        let mut channel = Json::obj();
        channel
            .set("index", Json::Num(index as f64))
            .set("kind", Json::Str(format!("joint_{mode}")))
            .set("joint", Json::Str(joint.name.clone()));
        channels.push(channel);
    }
    let Some(mode) = first else {
        return Err("the body declares no joints, so there is nothing to plan".to_string());
    };
    let mut source = Json::obj();
    source
        .set("robot_slug", Json::Str(descriptor.robot_slug.clone()))
        .set("dof", Json::Num(descriptor.joints.len() as f64))
        .set("control_mode", Json::Str(mode))
        .set("control_hz", Json::Num(hz))
        .set("action_channels", channels)
        .set("observations", Json::arr());
    let source = skynet_contracts::json::source_from_json(&source)
        .map_err(|error| format!("the body's own source did not decode: {error:?}"))?;
    resolve(&source, origin, origin.to_string(), descriptor, hz)
}

/// Resolve one source against this body, and hold the result to what this loop can carry out.
///
/// This loop writes channel *i* to joint *i*, so a plan that binds a channel anywhere else is
/// refused rather than obeyed at the wrong joint: a channel map is a separate piece of this host.
fn resolve(
    source: &skynet_contracts::embodiment::SourceEmbodiment,
    label: &str,
    origin: String,
    descriptor: &EmbodimentDescriptor,
    hz: f64,
) -> Result<Plan, String> {
    use skynet_contracts::transport::{TransportMethod, TransportVerdict};
    let options = skynet_contracts::ResolveTransportOptions {
        allow_retarget: false,
        requested_hz: Some(hz),
    };
    let plan = skynet_contracts::resolve_transport(source, descriptor, options);
    if plan.verdict == TransportVerdict::Refused {
        let blocked: Vec<String> = (plan.bindings.iter())
            .filter(|binding| binding.method == TransportMethod::Blocked)
            .map(|binding| format!("channel {}: {}", binding.source_index, binding.reason))
            .collect();
        return Err(format!(
            "the plan {label} resolves to for this body is refused ({}). `skynet transport \
             <source> --robot <descriptor>` prints the whole of it, warnings included.",
            if blocked.is_empty() {
                "no channel binds".to_string()
            } else {
                blocked.join("; ")
            }
        ));
    }
    let mut channels = Vec::with_capacity(plan.bindings.len());
    for (index, binding) in plan.bindings.iter().enumerate() {
        let joint = descriptor
            .joints
            .get(index)
            .map(|joint| joint.name.as_str());
        if binding.target.as_deref() != joint || joint.is_none() {
            return Err(format!(
                "the plan binds channel {index} to `{}` and this loop writes channel {index} to \
                 joint {index}, which is `{}`. A plan that reorders joints needs a channel map \
                 this host does not have yet; refused rather than commanding the wrong joint.",
                binding.target.as_deref().unwrap_or("nothing"),
                joint.unwrap_or("absent"),
            ));
        }
        channels.push(binding.target.clone().unwrap_or_default());
    }
    if channels.len() != descriptor.joints.len() {
        return Err(format!(
            "the plan binds {} channel(s) and the body declares {} joint(s). This loop commands \
             every joint on every cycle, so a plan that leaves one out is refused.",
            channels.len(),
            descriptor.joints.len()
        ));
    }
    let (ceilings, modes, travel) = match &plan.clamp {
        Some(clamp) => (
            clamp.per_channel.clone(),
            clamp.joint_modes.clone(),
            clamp.travel.clone(),
        ),
        None => (Vec::new(), Vec::new(), Vec::new()),
    };
    let unbounded: Vec<&str> = (channels.iter())
        .filter(|name| !ceilings.iter().any(|(bound, _)| bound == *name))
        .map(String::as_str)
        .collect();
    let summary = format!(
        "{} channel(s) from {origin}, verdict {}; {} with a ceiling in its own mode's units, {} \
         held at both ends of its travel{}",
        channels.len(),
        plan.verdict.as_str(),
        ceilings.len(),
        travel.len(),
        if unbounded.is_empty() {
            String::new()
        } else {
            format!(", and no rating declared for {}", unbounded.join(", "))
        }
    );
    Ok(Plan {
        channels,
        ceilings,
        modes,
        travel,
        summary,
        origin,
    })
}

impl Plan {
    /// Whether the two plans hold the loop to the same figures, whatever their origins say.
    ///
    /// Two packages whose sources resolve to one plan are one plan to the kernel, and a change
    /// between them changes nothing a guard reads.
    pub fn holds_as(&self, other: &Plan) -> bool {
        let sorted = |mut pairs: Vec<(String, f64)>| {
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            pairs
        };
        let modes = |plan: &Plan| {
            let mut modes = plan.modes.clone();
            modes.sort();
            modes
        };
        let travel = |plan: &Plan| {
            let mut travel = plan.travel.clone();
            travel.sort_by(|a, b| a.0.cmp(&b.0));
            travel
        };
        self.channels == other.channels
            && sorted(self.ceilings.clone()) == sorted(other.ceilings.clone())
            && modes(self) == modes(other)
            && travel(self) == travel(other)
    }

    /// Whether this plan allows no more than `than` on anything a guard reads (§33.4).
    ///
    /// * **Every ceiling `than` held is held here at least as tightly.** A ceiling dropped is a
    ///   widening; a ceiling added where `than` had none is a narrowing.
    /// * **Every travel `than` held lies inside the one held here.** Dropped is a widening; added is
    ///   a narrowing.
    /// * **Every joint is in the mode `than` bound it in.** A changed mode is never a narrowing: see
    ///   the module's header.
    ///
    /// `None` is a loop holding no plan at all, and any plan narrows that.
    pub fn narrows(&self, than: Option<&Plan>) -> bool {
        let Some(than) = than else {
            return true;
        };
        if self.channels != than.channels {
            return false;
        }
        let mode_of = |plan: &Plan, name: &str| {
            plan.modes
                .iter()
                .find(|(joint, _)| joint == name)
                .map(|(_, mode)| *mode)
        };
        let names = self.modes.iter().chain(than.modes.iter());
        if names
            .map(|(name, _)| name)
            .any(|name| mode_of(self, name) != mode_of(than, name))
        {
            return false;
        }
        let ceilings_hold = than.ceilings.iter().all(|(name, old)| {
            self.ceilings
                .iter()
                .find(|(bound, _)| bound == name)
                .is_some_and(|(_, new)| new <= old)
        });
        let travel_holds = than.travel.iter().all(|(name, low, high)| {
            self.travel
                .iter()
                .find(|(bound, _, _)| bound == name)
                .is_some_and(|(_, new_low, new_high)| new_low >= low && new_high <= high)
        });
        ceilings_hold && travel_holds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(
        ceilings: &[(&str, f64)],
        modes: &[(&str, &'static str)],
        travel: &[(&str, f64, f64)],
    ) -> Plan {
        Plan {
            channels: vec!["a".to_string(), "b".to_string()],
            ceilings: ceilings.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
            modes: modes.iter().map(|(n, m)| (n.to_string(), *m)).collect(),
            travel: travel
                .iter()
                .map(|(n, lo, hi)| (n.to_string(), *lo, *hi))
                .collect(),
            summary: String::new(),
            origin: String::new(),
        }
    }

    const POSITION: &[(&str, &str)] = &[("a", "position"), ("b", "position")];

    #[test]
    fn a_lower_ceiling_and_a_shorter_travel_narrow_and_the_reverse_widens() {
        let wide = plan(
            &[("a", 2.0), ("b", 2.0)],
            POSITION,
            &[("a", -2.0, 2.0), ("b", 0.0, 2.0)],
        );
        let narrow = plan(
            &[("a", 1.0), ("b", 2.0)],
            POSITION,
            &[("a", -1.0, 1.0), ("b", 0.0, 2.0)],
        );
        assert!(narrow.narrows(Some(&wide)));
        assert!(!wide.narrows(Some(&narrow)));
        assert!(wide.narrows(Some(&wide)), "the same plan is not a widening");
        assert!(wide.narrows(None), "any plan narrows a loop holding none");
    }

    #[test]
    fn a_dropped_ceiling_or_travel_widens_and_an_added_one_narrows() {
        let both = plan(&[("a", 1.0), ("b", 1.0)], POSITION, &[("a", -1.0, 1.0)]);
        let fewer = plan(&[("a", 1.0)], POSITION, &[("a", -1.0, 1.0)]);
        let no_travel = plan(&[("a", 1.0), ("b", 1.0)], POSITION, &[]);
        assert!(!fewer.narrows(Some(&both)));
        assert!(both.narrows(Some(&fewer)));
        assert!(!no_travel.narrows(Some(&both)));
        assert!(both.narrows(Some(&no_travel)));
    }

    #[test]
    fn a_changed_mode_is_never_a_narrowing() {
        let position = plan(&[("a", 1.0), ("b", 1.0)], POSITION, &[]);
        let torque = plan(
            &[("a", 0.5), ("b", 0.5)],
            &[("a", "torque"), ("b", "position")],
            &[],
        );
        assert!(!torque.narrows(Some(&position)));
        assert!(!position.narrows(Some(&torque)));
    }

    #[test]
    fn two_origins_with_one_set_of_figures_hold_alike() {
        let mut first = plan(&[("a", 1.0), ("b", 2.0)], POSITION, &[("a", -1.0, 1.0)]);
        let mut second = plan(&[("b", 2.0), ("a", 1.0)], POSITION, &[("a", -1.0, 1.0)]);
        first.origin = "one".to_string();
        second.origin = "two".to_string();
        assert!(first.holds_as(&second));
        second.ceilings[0].1 = 1.5;
        assert!(!first.holds_as(&second));
    }
}

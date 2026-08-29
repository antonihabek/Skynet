//! `skynet frames` — the frame graph, made visible before it matters.
//!
//! §44.3 pins a `frames` block in the descriptor and §44.2 has the Safety Kernel walk it in cycle.
//! Between those two there is a body description a human wrote or a tool generated, and the first
//! time anybody finds out whether it is *usable* should not be the first time a robot holds itself
//! still for a reason nobody can see.
//!
//! So this command answers the three questions the pose guards will ask, on a workstation, with no
//! hardware:
//!
//! * **Is the tree well formed?** [`flatten`] runs the same validation the robot runs, and reports
//!   the same fault with the same frames named.
//! * **Where does a configuration put things?** Forward kinematics, from the same kernel the robot
//!   runs — `--joints name=value,...`.
//! * **What will the guards be able to do?** Which frames the `workspace` guard could constrain,
//!   how many frames carry geometry the `self_collision` guard can evaluate, and how many pairs
//!   that costs per cycle.
//!
//! The last one is the reason this is a command rather than a debugging print. §7.2's rule is that
//! a guard which cannot run says so — and a vendor reading "1 collision body, 0 pairs" here has
//! learned something they would otherwise learn from a guard silently doing nothing.

use skynet_contracts::embodiment::FrameGraph;
use skynet_contracts::json::frames_from_json;
use skynet_contracts::kinematics::flatten;
use skynet_json::Json;
// Through the contracts rather than as a direct dependency: `skynet-contracts` already re-exports
// the kernel, and a second edge from a workstation tool to the safety kernel is an edge somebody
// would eventually use to call `evaluate_cycle` from a CLI.
use skynet_contracts::safety::kinematics::{FrameSet, JointBinding, JointKind, Kinematics};

use crate::args::{Args, Spec};
use crate::term;

/// What `--help` prints for this command.
pub const USAGE: &str = "\
skynet frames <descriptor.json> [--joints <name>=<v>,...]

  The body's frame graph (§44.3): the tree, where a configuration puts it, and
  what the two pose guards will be able to do with it.

    --joints <n>=<v>,... Joint values, in radians or metres, comma separated.
                         Joints with no value are reported unresolved rather
                         than assumed to be at zero — a limb assumed straight is
                         a limb in a place it is not.
    --json               Print the solved poses as canonical JSON.

  No hardware. The validation, the flattening and the kinematics are the same
  code the robot runs (§44.2), so a graph that works here works there.

  Exit code is 1 when the graph is absent or cannot be used, and 0 otherwise.";

const SPEC: Spec = Spec {
    values: &["joints"],
    switches: &["json"],
};

/// Run it. Returns the process exit code.
pub fn run(rest: &[String]) -> i32 {
    let args = match Args::parse(rest, &SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };

    let [path] = args.positional.as_slice() else {
        return term::fail(&format!(
            "expected exactly one descriptor file, got {}.\n\n{USAGE}",
            args.positional.len()
        ));
    };

    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => return term::fail(&format!("cannot read {path}: {err}")),
    };
    let document = match Json::parse(&text) {
        Ok(document) => document,
        Err(err) => return term::fail(&format!("{path} is not valid JSON: {err}")),
    };

    // Absent and null are two different bodies and only one of them said anything, so the message
    // distinguishes them (§44.3). Both leave the pose guards unavailable.
    let Some(graph) = frames_from_json(document.get_present("frames")) else {
        let said_so = matches!(document.get("frames"), Some(Json::Null));
        return term::fail(&if said_so {
            format!(
                "{path} declares `frames: null` — the generator could not express this body's \
                 tree. The workspace and self_collision guards will report unavailable, which is \
                 §44.5's answer and not a defect in this file."
            )
        } else {
            format!(
                "{path} has no `frames` block. Either the schema predates §44.3 or nothing \
                 generated one; `skynet descriptor from-urdf` writes it, and so does the MuJoCo \
                 driver's `describe`."
            )
        });
    };

    let flat = match flatten(&graph) {
        Ok(flat) => flat,
        Err(fault) => {
            let subjects = if fault.subjects.is_empty() {
                String::new()
            } else {
                format!(" ({})", fault.subjects.join(", "))
            };
            return term::fail(&format!("{}{subjects}: {}", fault.code, fault.detail));
        }
    };

    let frames = flat.frames();
    let set: FrameSet<'_> = match FrameSet::new(&frames) {
        Ok(set) => set,
        Err(fault) => {
            return term::fail(&format!(
                "the flattened frame set was refused: {}. The descriptor validated, so this is a \
                 defect in the flattening and worth reporting.",
                fault.as_str()
            ))
        }
    };

    let commanded = match joint_values(&args) {
        Ok(values) => values,
        Err(message) => return term::fail(&message),
    };

    // Every joint in the graph gets a binding, and a joint the caller did not name has no value at
    // all — not zero. `measured` is only read when a channel is bound and the command omits it,
    // which cannot happen here, so it never stands in for a number nobody supplied.
    let bindings: Vec<JointBinding<'_>> = set
        .frames()
        .iter()
        .filter_map(|frame| frame.joint.as_ref())
        .filter(|joint| commanded.iter().any(|(name, _)| name == joint.name))
        .map(|joint| JointBinding {
            joint: joint.name,
            channel: Some(joint.name),
            measured: 0.0,
        })
        .collect();

    let mut solved = Kinematics::new();
    solved.solve(&set, &bindings, |name| {
        commanded
            .iter()
            .find(|(joint, _)| joint == name)
            .map(|(_, value)| *value)
    });

    if args.flag("json") {
        match skynet_contracts::kinematics::solve_to_json(&set, &solved).to_canonical() {
            Ok(json) => println!("{json}"),
            Err(err) => return term::fail(&format!("the poses could not be serialised: {err}")),
        }
        return 0;
    }

    report(&graph, &set, &solved, &commanded);
    0
}

fn joint_values(args: &Args) -> Result<Vec<(String, f64)>, String> {
    let mut out: Vec<(String, f64)> = Vec::new();
    for raw in args.list("joints") {
        let Some((name, value)) = raw.split_once('=') else {
            return Err(format!(
                "`{raw}` is not a `name=value` pair. A joint needs both."
            ));
        };
        let Ok(parsed) = value.parse::<f64>() else {
            return Err(format!("`{raw}`: {value} is not a number."));
        };
        if !parsed.is_finite() {
            return Err(format!("`{raw}`: a joint value must be finite."));
        }
        out.push((name.to_string(), parsed));
    }
    Ok(out)
}

fn report(
    graph: &FrameGraph,
    set: &FrameSet<'_>,
    solved: &Kinematics,
    commanded: &[(String, f64)],
) {
    use std::io::Write;
    let out = &mut std::io::stderr();

    let generator = graph.generator.as_deref().unwrap_or("unstated");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {} frames rooted at {} — {}, generated by {generator}",
        term::bold(&set.len().to_string()),
        term::bold(&graph.root),
        graph.convention
    );
    if graph.convention != "rep103" {
        let _ = writeln!(
            out,
            "  {}",
            term::warn(&format!(
                "this body declares the {} convention, not REP-103. Every sign below is in that \
                 convention and not in the one §44.4 assumes.",
                graph.convention
            ))
        );
    }
    let _ = writeln!(out);

    for (index, frame) in set.frames().iter().enumerate() {
        let depth = depth_of(set, index);
        let indent = "  ".repeat(depth + 1);
        let mut line = format!("{indent}{}", frame.name);
        if let Some(joint) = &frame.joint {
            let kind = joint.kind.as_str();
            line.push_str(&term::dim(&format!("  ← {} ({kind})", joint.name)));
            if let Some(mimic) = joint.mimic {
                line.push_str(&term::dim(&format!(
                    "  mimics {} ×{} +{}",
                    mimic.joint, mimic.multiplier, mimic.offset
                )));
            }
        }
        if let Some(capsule) = frame.collision {
            line.push_str(&term::dim(&format!("  ⌀{:.3}m", capsule.radius_m)));
        }
        let _ = writeln!(out, "{line}");

        match solved.pose(index) {
            Some(pose) => {
                let _ = writeln!(
                    out,
                    "{indent}  {}",
                    term::dim(&format!(
                        "at ({:.4}, {:.4}, {:.4})",
                        pose.translation[0], pose.translation[1], pose.translation[2]
                    ))
                );
            }
            None => {
                let reason = unresolved_reason(set, solved, index, commanded);
                let _ = writeln!(
                    out,
                    "{indent}  {}",
                    term::warn(&format!("no pose — {reason}"))
                );
            }
        }
    }

    // What the guards will be able to do, which is the question this command exists to answer.
    let _ = writeln!(out);
    let bodies = set.collision_bodies();
    let pairs = set.collision_pairs();
    let constrainable = set
        .frames()
        .iter()
        .enumerate()
        .filter(|(index, _)| set.statically_resolvable(*index))
        .count();

    let _ = writeln!(
        out,
        "  workspace       {} of {} frames can be constrained",
        constrainable,
        set.len()
    );
    if constrainable < set.len() {
        let _ = writeln!(
            out,
            "  {}",
            term::dim(
                "  the rest sit below a ball or floating joint, which one scalar cannot describe"
            )
        );
    }

    let collision = if bodies < 2 {
        term::warn(&format!(
            "{bodies} collision {} — the guard reports unavailable",
            if bodies == 1 { "body" } else { "bodies" }
        ))
    } else {
        format!("{bodies} collision bodies, {pairs} pairs each cycle")
    };
    let _ = writeln!(out, "  self_collision  {collision}");
    if pairs > 300 {
        // Not a warning about correctness. §7.4's certified tier mirrors this kernel onto a safety
        // MCU, and that is where a quadratic cost stops being free.
        let _ = writeln!(
            out,
            "  {}",
            term::dim(&format!(
                "  {pairs} segment-to-segment distances per cycle is comfortable on an \
                 application processor and is the number to check before mirroring this guard \
                 onto a safety MCU"
            ))
        );
    }
    let _ = writeln!(out);
}

/// How deep a frame is, for indentation. Bounded by the ordering invariant.
fn depth_of(set: &FrameSet<'_>, index: usize) -> usize {
    let mut depth = 0;
    let mut here = index;
    while let Some(parent) = set.frames()[here].parent {
        depth += 1;
        here = parent as usize;
    }
    depth
}

/// Why one frame has no pose, in the words the operator needs.
///
/// The order of these tests is the point. A frame below an unresolved parent has no pose *because
/// of the parent*, and saying anything else sends a reader to fix the wrong thing — so the parent
/// is checked before this frame's own joint, however true the second sentence would also be.
fn unresolved_reason(
    set: &FrameSet<'_>,
    solved: &Kinematics,
    index: usize,
    commanded: &[(String, f64)],
) -> String {
    let frame = &set.frames()[index];
    if let Some(parent) = frame.parent {
        if solved.pose(parent as usize).is_none() {
            return format!("{} above it has none", set.frames()[parent as usize].name);
        }
        if !set.statically_resolvable(index) {
            // Distinguished from "no value" because they need different fixes: one is a body this
            // representation cannot describe, the other is an argument the caller did not pass.
            return format!(
                "the walk to the root passes a joint one scalar cannot describe; {} is where it \
                 stops",
                first_non_scalar(set, index)
            );
        }
    }
    match &frame.joint {
        Some(joint) if !commanded.iter().any(|(name, _)| name == joint.name) => {
            let driver = joint.mimic.map(|m| m.joint).unwrap_or(joint.name);
            format!("no value for {driver}")
        }
        Some(joint) if matches!(joint.kind, JointKind::Ball | JointKind::Floating) => {
            format!("{} is a {} joint", joint.name, joint.kind.as_str())
        }
        Some(joint) => format!(
            "the value for {} produced no transform — check the axis and the magnitude",
            joint.name
        ),
        None => "its parent has none".to_string(),
    }
}

fn first_non_scalar(set: &FrameSet<'_>, index: usize) -> String {
    let mut here = index;
    loop {
        let frame = &set.frames()[here];
        if let Some(joint) = &frame.joint {
            if !joint.kind.is_scalar() {
                return frame.name.to_string();
            }
        }
        match frame.parent {
            None => return set.frames()[index].name.to_string(),
            Some(parent) => here = parent as usize,
        }
    }
}

//! `skynet transport` — §18.
//!
//! > **`skynet transport` needs no hardware.** Because the planner is pure and descriptors are
//! > published, a developer can ask "would this run on a robot I do not own?" and get a per-channel
//! > answer. This is the loop that makes cross-body publishing normal instead of heroic.
//!
//! The same `resolve_transport` the Hub runs and the robot runs at install time (§6.2). Neither
//! side is authoritative because all three compute the same function over the same declared facts,
//! and so they cannot disagree. This command is the third caller.

use skynet_contracts::json::{descriptor_from_json, plan_to_json, source_from_json};
use skynet_contracts::transport::{TransportMethod, TransportVerdict};
use skynet_contracts::{resolve_transport, ResolveTransportOptions};
use skynet_json::Json;

use crate::args::{Args, Spec};
use crate::term;

/// What `--help` prints for this command.
pub const USAGE: &str = "\
skynet transport <policy.json> --robot <descriptor.json> [options]

  What this body would do with this policy's action space, channel by channel.
  No hardware, no download: both sides are declared facts and the planner is pure.

    --robot <file>       The target embodiment descriptor. Required.
    --allow-retarget     Permit task-space retargeting when direct and role
                         matching both fail. Off by default: retargeting changes
                         what the policy is doing.
    --hz <n>             The rate to request, clamped to what the body accepts.
    --json               Print the canonical plan instead of the report.

  <policy.json> is a source embodiment — the body the policy was trained on and
  what it emits — either bare or under a `source_embodiment` key.

  Exit code is 1 when the verdict is `refused` — at least one required channel
  has no equivalent on this body — and 0 otherwise.";

const SPEC: Spec = Spec {
    values: &["robot", "hz"],
    switches: &["allow-retarget", "json"],
};

/// Run it. Returns the process exit code.
pub fn run(rest: &[String]) -> i32 {
    let args = match Args::parse(rest, &SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };

    let [policy_path] = args.positional.as_slice() else {
        return term::fail(&format!(
            "expected exactly one policy file, got {}.\n\n{USAGE}",
            args.positional.len()
        ));
    };
    let Some(robot_path) = args.value("robot") else {
        return term::fail(&format!(
            "--robot is required: this command answers \"what would this body do with this \
             policy?\", and needs the body.\n\n{USAGE}"
        ));
    };

    let policy_json = match read_json(policy_path) {
        Ok(json) => json,
        Err(message) => return term::fail(&message),
    };
    let robot_json = match read_json(robot_path) {
        Ok(json) => json,
        Err(message) => return term::fail(&message),
    };

    // Accept a bare source embodiment or one nested under the key a package manifest uses, because
    // both spellings are in circulation and neither is wrong.
    let source_node = policy_json
        .get("source_embodiment")
        .unwrap_or(&policy_json)
        .clone();

    let source = match source_from_json(&source_node) {
        Ok(source) => source,
        Err(err) => {
            return term::fail(&format!(
                "{policy_path} is not a source embodiment: {err:?}. It needs at least \
                 `control_mode` and an `action_channels` array."
            ))
        }
    };
    let target = match descriptor_from_json(&robot_json) {
        Ok(target) => target,
        Err(err) => {
            return term::fail(&format!(
                "{robot_path} is not an embodiment descriptor: {err:?}. It needs at least \
                 `robot_slug`, `base` and `dof`."
            ))
        }
    };

    let requested_hz = match args.value("hz") {
        None => None,
        Some(raw) => match raw.parse::<f64>() {
            Ok(hz) if hz.is_finite() && hz > 0.0 => Some(hz),
            _ => return term::fail(&format!("`--hz {raw}` is not a positive rate.")),
        },
    };

    let plan = resolve_transport(
        &source,
        &target,
        ResolveTransportOptions {
            allow_retarget: args.flag("allow-retarget"),
            requested_hz,
        },
    );

    if args.flag("json") {
        match plan_to_json(&plan).to_canonical() {
            Ok(json) => println!("{json}"),
            Err(err) => return term::fail(&format!("the plan could not be serialised: {err}")),
        }
    } else {
        print_plan(&plan);
    }

    // A blocked plan is a failure of the question, not of the command, but a script asking "can I
    // deploy this?" wants an exit code it can branch on.
    match plan.verdict {
        TransportVerdict::Refused => 1,
        _ => 0,
    }
}

fn read_json(path: &str) -> Result<Json, String> {
    let text = std::fs::read_to_string(path).map_err(|err| format!("cannot read {path}: {err}"))?;
    Json::parse(&text).map_err(|err| format!("{path} is not valid JSON: {err}"))
}

fn print_plan(plan: &skynet_contracts::TransportPlan) {
    use std::io::Write;
    let out = &mut std::io::stderr();

    let verdict = match plan.verdict {
        TransportVerdict::Direct => term::ok("direct"),
        TransportVerdict::Retargeted => term::warn("retargeted"),
        TransportVerdict::Degraded => term::warn("degraded"),
        TransportVerdict::Refused => term::bad("refused"),
    };
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {} on {} — {verdict}",
        term::bold(&plan.control_mode),
        term::bold(&plan.robot_slug)
    );
    if let Some(hz) = plan.control_hz {
        let _ = writeln!(out, "  {}", term::dim(&format!("running at {hz} Hz")));
    }

    if plan.bindings.is_empty() {
        let _ = writeln!(
            out,
            "\n  {}",
            term::dim("the policy declares no action space")
        );
    } else {
        let _ = writeln!(out);
        for binding in &plan.bindings {
            let mark = match binding.method {
                TransportMethod::Direct => term::ok("✓"),
                TransportMethod::Remap | TransportMethod::Synthesize => term::ok("~"),
                TransportMethod::Retarget => term::warn("~"),
                TransportMethod::Drop => term::warn("–"),
                TransportMethod::Blocked => term::bad("✗"),
            };
            let target = binding
                .target
                .clone()
                .unwrap_or_else(|| "(none)".to_string());
            let mut line = format!(
                "    {mark} [{}] {} → {target}",
                binding.source_index as i64,
                binding
                    .source_role
                    .clone()
                    .unwrap_or_else(|| "?".to_string()),
            );
            if binding.scale != 1.0 {
                line.push_str(&format!("  ×{:.3}", binding.scale));
            }
            if let Some(limit) = binding.limit {
                line.push_str(&format!("  ≤{limit}"));
            }
            let _ = writeln!(out, "{line}");
            let _ = writeln!(
                out,
                "      {}",
                term::dim(&format!("{} — {}", binding.method.as_str(), binding.reason))
            );
        }
    }

    for observation in &plan.observations {
        let _ = writeln!(
            out,
            "    {} {} — {}",
            term::dim("obs"),
            observation.required,
            term::dim(&observation.reason)
        );
    }

    if let Some(clamp) = &plan.clamp {
        let mut parts: Vec<String> = Vec::new();
        if let Some(value) = clamp.joint_velocity_max_rad_s {
            parts.push(format!("joint velocity ≤ {value} rad/s"));
        }
        if let Some(value) = clamp.joint_torque_max_nm {
            parts.push(format!("joint torque ≤ {value} Nm"));
        }
        if let Some(value) = clamp.base_velocity_max_m_s {
            parts.push(format!("base velocity ≤ {value} m/s"));
        }
        if !parts.is_empty() {
            let _ = writeln!(out, "\n  {} {}", term::bold("Clamp"), parts.join(", "));
        }
    }

    if !plan.warnings.is_empty() {
        let _ = writeln!(out);
        for warning in &plan.warnings {
            let _ = writeln!(
                out,
                "  {} {}",
                term::warn(warning.code.as_str()),
                term::wrap(&warning.message, 4)
            );
        }
    }
    let _ = writeln!(out);
}

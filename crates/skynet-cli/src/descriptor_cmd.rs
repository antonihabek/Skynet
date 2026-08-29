//! `skynet descriptor from-urdf` — §63.4.
//!
//! The output split is the design: **the descriptor goes to stdout, the report goes to stderr.**
//! §63.4 writes the invocation as `skynet descriptor from-urdf robot.urdf > embodiment.json`, and
//! that only works if the reasoning does not end up in the file. It also means the reasoning is
//! seen: a report written to a file beside the descriptor is a report nobody reads, and the whole
//! value of this tool is in the four things it refuses to fill in.

use std::path::Path;

use skynet_urdf::descriptor::{self, ImportOptions, NoteLevel};
use skynet_urdf::model;
use skynet_urdf::suggest::{Confidence, Side};

use crate::args::{Args, Spec};
use crate::term;

/// What `--help` prints for this command.
pub const USAGE: &str = "\
skynet descriptor from-urdf <file.urdf> [options]

  Draft an embodiment descriptor from a URDF. The descriptor goes to stdout; the
  report — including everything the URDF could not tell us — goes to stderr.

    --slug <name>              Robot slug. Defaults to a slug of <robot name>.
    --base <kind>              fixed|wheeled|tracked|legged|aerial|rail.
    --control-modes <a,b>      position|velocity|torque|trajectory|cartesian.
                               Sets the body profile and every joint together.
    --assume-side left|right   Assert the side for a single-limb robot whose joint
                               names do not say. Suggestions become `inferred`.
    --accept-suggested-roles   Write suggested roles into the descriptor. This is
                               the human-confirms step; every suggestion and how it
                               was reached is still printed.
    -o, --out <file>           Write the descriptor here instead of stdout.
    --quiet                    Suppress the report.

  Two fields are never populated from a URDF, whatever flags are passed:
  `envelope` (a CAD effort limit is not a datasheet safety rating) and `digest`
  (the vendor signs the descriptor, not this tool).";

const SPEC: Spec = Spec {
    values: &["slug", "base", "control-modes", "assume-side", "out", "o"],
    switches: &["accept-suggested-roles", "quiet"],
};

/// Run it. Returns the process exit code.
pub fn run(rest: &[String]) -> i32 {
    let args = match Args::parse(rest, &SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };

    let [path] = args.positional.as_slice() else {
        return term::fail(&format!(
            "expected exactly one URDF path, got {}.\n\n{USAGE}",
            args.positional.len()
        ));
    };

    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => return term::fail(&format!("cannot read {path}: {err}")),
    };
    let text = match String::from_utf8(bytes.clone()) {
        Ok(text) => text,
        Err(_) => {
            return term::fail(&format!(
                "{path} is not UTF-8. URDF is XML and XML is text; a binary file here is usually a \
                 mesh or an archive that was named `.urdf` by mistake."
            ))
        }
    };

    let assume_side = match args.value("assume-side") {
        None => None,
        Some(raw) => match Side::parse(raw) {
            Some(side) => Some(side),
            None => {
                return term::fail(&format!(
                    "`--assume-side {raw}` is not a side. Use `left` or `right`."
                ))
            }
        },
    };

    let options = ImportOptions {
        robot_slug: args.value("slug").map(str::to_string),
        assume_side,
        accept_suggested_roles: args.flag("accept-suggested-roles"),
        base: args.value("base").map(str::to_string),
        control_modes: args.list("control-modes"),
        source_file: Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
        source_sha256: Some(
            String::from_utf8(skynet_sha256::hex_bytes(&bytes).to_vec()).unwrap_or_default(),
        ),
        tool_version: crate::VERSION.to_string(),
    };

    let model = match model::parse(&text) {
        Ok(model) => model,
        Err(err) => return term::fail(&format!("{path}: {err}")),
    };

    let import = match descriptor::build(&model, &options) {
        Ok(import) => import,
        Err(err) => return term::fail(&format!("{path}: {}", err.message)),
    };

    let json = match import.descriptor.to_canonical() {
        Ok(json) => json,
        Err(err) => return term::fail(&format!("the descriptor could not be serialised: {err}")),
    };

    let destination = args.value("out").or_else(|| args.value("o"));
    match destination {
        Some(out) => {
            if let Err(err) = std::fs::write(out, format!("{json}\n")) {
                return term::fail(&format!("cannot write {out}: {err}"));
            }
        }
        None => println!("{json}"),
    }

    if !args.flag("quiet") {
        let base = import
            .descriptor
            .get("base")
            .and_then(skynet_json::Json::as_str)
            .unwrap_or("fixed")
            .to_string();
        print_report(&import.report, &model, &base, destination);
    }

    0
}

fn print_report(
    report: &skynet_urdf::ImportReport,
    model: &skynet_urdf::UrdfModel,
    base: &str,
    destination: Option<&str>,
) {
    let out = &mut std::io::stderr();
    use std::io::Write;

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {} — {} joints, {} degrees of freedom, {} frames, base `{base}`",
        term::bold(&report.robot_slug),
        model.joints.len(),
        report.dof,
        report.frame_count,
    );
    if let Some(path) = destination {
        let _ = writeln!(out, "  written to {path}");
    }

    if !report.suggestions.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "  {} — {}",
            term::bold("Roles"),
            if report.roles_accepted {
                "accepted into the descriptor at your instruction"
            } else {
                "suggested only; every `role` in the descriptor is null"
            }
        );
        let width = report
            .suggestions
            .iter()
            .map(|s| s.joint.len())
            .max()
            .unwrap_or(0);
        for suggestion in &report.suggestions {
            let (mark, role) = match (&suggestion.role, suggestion.confidence) {
                (Some(role), Some(Confidence::Named)) => (term::ok("✓"), role.clone()),
                (Some(role), _) => (term::warn("?"), role.clone()),
                (None, _) => (term::warn("–"), "(none)".to_string()),
            };
            let _ = writeln!(
                out,
                "    {mark} {:width$}  {role}",
                suggestion.joint,
                width = width
            );
            let _ = writeln!(
                out,
                "      {}",
                term::dim(&term::wrap(&suggestion.reason, 6))
            );
        }
        let _ = writeln!(
            out,
            "    {} named, {} inferred, {} not suggested",
            report
                .suggestions
                .iter()
                .filter(|s| s.confidence == Some(Confidence::Named))
                .count(),
            report
                .suggestions
                .iter()
                .filter(|s| s.confidence == Some(Confidence::Inferred))
                .count(),
            report.suggestions.len() - report.suggested_count(),
        );
    }

    if !report.excluded.is_empty() {
        let _ = writeln!(out);
        let _ = writeln!(out, "  {}", term::bold("Not in joints[]"));
        for (name, why) in &report.excluded {
            let _ = writeln!(out, "    {name} — {}", term::dim(why));
        }
    }

    let mut notes = report.notes.clone();
    if let Some(advice) = descriptor::base_advice(model, report, base) {
        notes.push(advice);
    }

    let _ = writeln!(out);
    for note in notes.iter().filter(|note| note.level == NoteLevel::Action) {
        let _ = writeln!(
            out,
            "  {} {}",
            term::warn("!"),
            term::wrap(&note.message, 4)
        );
    }
    for note in notes.iter().filter(|note| note.level == NoteLevel::Info) {
        let _ = writeln!(
            out,
            "  {} {}",
            term::dim("·"),
            term::dim(&term::wrap(&note.message, 4))
        );
    }
    let _ = writeln!(out);
}

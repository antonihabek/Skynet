//! `skynet driver new` and `skynet driver check` — §63.1 and §63.3.

use std::path::{Path, PathBuf};

use crate::args::{Args, Spec};
use crate::check;
use crate::scaffold;
use crate::term;
use skynet_dl::Library;

/// What `--help` prints for this command.
pub const USAGE: &str = "\
skynet driver new <name> [--lang c|rust] [--dir <path>]
skynet driver check <library> [--config <json>]

  new    Scaffold a driver that builds, loads, and fails conformance in exactly
         the places where only you know the answer. The failures are the to-do
         list, and each one carries the SCTS id it stands for.

  check  Run the driver-facing sim tests against a built shared object — the ABI,
         state completeness, command rejection, descriptor validity, the estop
         path. Turning up to a bench session with these already green is what
         makes bench time cheap.

         --config <json>   passed verbatim to the driver's open(). Defaults to
                           an empty string.

         The sim tests that are properties of the runtime rather than of a driver
         are listed as not covered, by name, on every run.

  Exit code for `check` is 0 only when nothing failed and nothing went unmeasured:
  absence is not a pass (§15).";

const NEW_SPEC: Spec = Spec {
    values: &["lang", "dir"],
    switches: &["force"],
};

const CHECK_SPEC: Spec = Spec {
    values: &["config"],
    switches: &[],
};

/// Dispatch a `driver` subcommand.
pub fn run(rest: &[String]) -> i32 {
    match rest.first().map(String::as_str) {
        Some("new") => new(&rest[1..]),
        Some("check") => run_check(&rest[1..]),
        Some("--help") | Some("-h") | None => {
            println!("{USAGE}");
            0
        }
        Some(other) => term::fail(&format!(
            "`driver {other}` is not a subcommand. They are `new` and `check`."
        )),
    }
}

fn new(rest: &[String]) -> i32 {
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let args = match Args::parse(rest, &NEW_SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };
    let [name] = args.positional.as_slice() else {
        return term::fail(&format!(
            "expected exactly one driver name, got {}.\n\n{USAGE}",
            args.positional.len()
        ));
    };

    let language = args.value("lang").unwrap_or("c");
    let target: PathBuf = match args.value("dir") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(name),
    };

    if target.exists() {
        let occupied = std::fs::read_dir(&target)
            .map(|entries| entries.count() > 0)
            .unwrap_or(false);
        if occupied && !args.flag("force") {
            return term::fail(&format!(
                "{} already exists and is not empty. Pick another name, or pass --force to write \
                 into it anyway — which overwrites files with the same names and leaves everything \
                 else alone.",
                target.display()
            ));
        }
    }

    let header = scaffold::header_path(&target);
    let files = match scaffold::build(name, language, &header) {
        Ok(files) => files,
        Err(message) => return term::fail(&message),
    };

    for file in &files {
        let path = target.join(&file.path);
        if let Some(parent) = path.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                return term::fail(&format!("cannot create {}: {err}", parent.display()));
            }
        }
        if let Err(err) = std::fs::write(&path, &file.body) {
            return term::fail(&format!("cannot write {}: {err}", path.display()));
        }
    }

    use std::io::Write;
    let out = &mut std::io::stderr();
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {} — {} files in {}",
        term::bold(name),
        files.len(),
        target.display()
    );
    for file in &files {
        let _ = writeln!(out, "    {}", term::dim(&file.path.display().to_string()));
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "  {}", term::bold("Next"));
    let build_line = if language == "rust" {
        "cargo build --release"
    } else {
        "make"
    };
    let _ = writeln!(out, "    cd {} && {build_line}", target.display());
    let _ = writeln!(out, "    skynet driver check <the built library>");
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {}",
        term::wrap(
            "It will fail `embodiment.roles.coverage` at 0% and report \
             `embodiment.descriptor.signed` as not measured. Both are correct on day one: roles are \
             yours to assign, and a driver cannot sign its own descriptor. Everything else marked \
             TODO in the source is the rest of the list.",
            2
        )
    );
    let _ = writeln!(out);
    0
}

fn run_check(rest: &[String]) -> i32 {
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return 0;
    }
    let args = match Args::parse(rest, &CHECK_SPEC) {
        Ok(args) => args,
        Err(message) => return term::fail(&message),
    };
    let [path] = args.positional.as_slice() else {
        return term::fail(&format!(
            "expected exactly one library path, got {}.\n\n{USAGE}",
            args.positional.len()
        ));
    };

    let path = Path::new(path);
    if !path.exists() {
        return term::fail(&format!(
            "{} does not exist. Build the driver first; `skynet driver new` writes a build file \
             that produces the right kind of artifact for your platform.",
            path.display()
        ));
    }

    let library = match Library::open(path) {
        Ok(library) => library,
        Err(err) => return term::fail(&err.to_string()),
    };

    let report = check::run(&library, args.value("config").unwrap_or(""));
    check::print(&report)
}

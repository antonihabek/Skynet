//! §55.2's cell descriptor, on the robot that is standing in one.
//!
//! > The real-world analogue of an environment — where the fixtures are, which volumes are
//! > forbidden, where the safe parking pose is, what the operator has declared about the space — is
//! > a **cell descriptor**, and it belongs in the operator profile (§38), not in the repository
//! > catalogue. […] The cell descriptor supplies `env.*` to the precondition context (§54.2) and
//! > the allowed volumes to the `workspace` guard, and is signed like every other safety-relevant
//! > profile.
//!
//! The first half has worked since §54.2's context was built: `cell_environment` runs on every
//! precondition evaluation and `env.door_closed` reaches a clause. What was missing is everything a
//! person needs when it does not.
//!
//! # The failure this closes, which is a diagnosis rather than a defect
//!
//! A cell fact key with a dot in it, or a space around it, is a key no dot path can address. §54.2's
//! rule is that *a precondition that cannot be evaluated refuses the skill*, naming the path — so an
//! operator who typed `env.door closed` into their profile sees a skill refused with
//! `env.door closed unresolvable`, correctly, and has no way to learn that the key they wrote is the
//! reason. `cell_environment` silently drops it, which is the right behaviour for a context and a
//! terrible one for a diagnosis.
//!
//! `validate_cell` has enumerated exactly those faults since the cell was designed, and nothing
//! called it. This is the caller, and `GET /cell` is where the answer goes.
//!
//! # It reports; it does not refuse
//!
//! A malformed cell must **not** refuse the profile, and the reason is §38.2's direction. A profile
//! layers ceilings that only ever narrow; refusing one because a fixture is unnamed would drop the
//! limits along with the cell, leaving the robot on **wider** ceilings than the operator asked for.
//! A safety document that fails open when part of it is malformed is worse than one that reports.
//!
//! # The workspace layer is computed and has nowhere to go
//!
//! §55.2 sends the allowed volumes to the `workspace` guard, which lives in the kernel — and §33.1
//! keeps this daemon off that path deliberately. So the layer is computed and **reported**, with the
//! reason it goes no further, rather than being left out of the answer. A console that showed the
//! facts and silently omitted the volumes would suggest the volumes are not there.

use skynet_contracts::cell::{
    cell_environment, cell_faults_to_json, cell_from_json, cell_layer_to_json, cell_robot_layer,
    cell_workspace_layer, validate_cell, CellDescriptor,
};
use skynet_json::Json;

use crate::layout::Layout;

/// The cell in force, decoded, or nothing.
///
/// Three of [`to_json`]'s four states collapse to `None` here — no profile, a profile with no
/// cell, and a cell that carries faults is still a cell — because a caller that wants the
/// descriptor wants the descriptor, and the four-way distinction is a *report*. `crate::limits`
/// is that caller: it folds §55.2's volumes into the kernel's operator profile, and a cell with
/// faults layers the volumes that survived them exactly as `GET /cell` says it does.
pub fn active(layout: &Layout) -> Option<CellDescriptor> {
    let profile = crate::observe::read_json(&layout.active_profile())?;
    Some(cell_from_json(profile.get_present("cell")?))
}

/// The cell in force, as `GET /cell` answers.
///
/// Four states, and they are four different things to do about it:
///
/// * **no profile** — nothing has been applied. Not a fault.
/// * **a profile with no cell** — the operator declared limits and said nothing about the space,
///   which is ordinary and means every `env.*` precondition is unevaluable.
/// * **a cell with faults** — it applies, `env.*` carries what is usable, and the faults say what
///   was dropped and why.
/// * **a clean cell** — the facts, the volumes, the fixtures and the parking pose.
pub fn to_json(layout: &Layout) -> Json {
    let mut out = Json::obj();
    // §87.1's subject: this robot's own `robot_slug`, the identifier `Runtime::board` already
    // uses. `None` on a machine that has not been told what it is, which answers `env.robot` with
    // the empty string — the same answer a cell naming no robots gives, and the correct one, since
    // a robot that cannot say what it is cannot be one of the robots a cell names.
    let body = crate::embodiment::load(layout);
    let subject = body
        .known()
        .map(|known| known.descriptor.robot_slug.as_str());

    let Some(profile) = crate::observe::read_json(&layout.active_profile()) else {
        out.set("held", Json::Bool(false));
        out.set(
            "note",
            Json::from(
                "No operator profile is in force, so this robot has no cell descriptor. §54.2's \
                 `env.*` facts are therefore unresolvable, and a precondition naming one refuses \
                 the skill rather than assuming it true.",
            ),
        );
        return out;
    };

    out.set(
        "profile",
        profile
            .get("name")
            .cloned()
            .unwrap_or(Json::Str(String::new())),
    );

    let Some(document) = profile.get_present("cell") else {
        out.set("held", Json::Bool(false));
        out.set(
            "note",
            Json::from(
                "The profile in force declares no cell. Its limits apply; §54.2's `env.*` facts \
                 are unresolvable, which refuses any precondition that names one.",
            ),
        );
        return out;
    };

    let cell = cell_from_json(document);
    let faults = validate_cell(&cell);

    out.set("held", Json::Bool(true));
    out.set("name", Json::Str(cell.name.clone()));
    // The context as a precondition sees it, rather than the facts as they were written. They
    // differ by exactly the faults below, which is the whole point of answering both.
    out.set("environment", cell_environment(&cell, subject));
    // Unwrapped from the contract's document shape. `cell_faults_to_json` answers
    // `{"faults":[…]}` because the corpus freezes whole documents; a route that nested it would
    // make every consumer write `body.faults.faults`, which is the sort of thing a console gets
    // wrong once and then carries.
    out.set(
        "faults",
        cell_faults_to_json(&faults)
            .get("faults")
            .cloned()
            .unwrap_or_else(Json::arr),
    );
    out.set("fixtures", Json::Num(cell.fixtures.len() as f64));
    out.set("parking", parking(&cell));
    // `cell_layer_to_json` takes the option, so an absent layer renders as the same shape rather
    // than as a null a consumer has to special-case.
    // The same unwrapping, and `null` for a cell with no usable volume — which is a fact rather
    // than an empty layer, because an empty allowed-volume list and no list at all mean opposite
    // things to a guard.
    out.set(
        "workspace",
        cell_layer_to_json(&cell_workspace_layer(&cell))
            .get("layer")
            .cloned()
            .unwrap_or(Json::Null),
    );
    // §87.1's robots, each with the layer it contributes. A cell naming none answers an empty
    // array rather than being absent, because *"this cell names no robots"* and *"this robot's
    // supervisor is too old to have asked"* are two facts a console has to be able to tell apart —
    // and the first is every cell written before §87.
    //
    // `subject` is what makes one of these rows **this** robot: `mine` rather than a console
    // re-deriving it from the slug, because the slug the robot answers to is the descriptor's and a
    // page that matched on the string would be matching on a spelling.
    let mut robots = Json::arr();
    for robot in &cell.robots {
        let mut row = Json::obj();
        row.set("slug", Json::Str(robot.slug.clone()));
        row.set("mine", Json::Bool(subject == Some(robot.slug.as_str())));
        row.set(
            "at",
            match robot.at {
                Some(at) => Json::Arr(at.iter().map(|value| Json::Num(*value)).collect()),
                // Absent rather than an origin. §87.1 refuses a placement nobody made
                // (`robot_unplaced`) from the second robot up, and writing `[0,0,0]` here would be
                // this route supplying the placement the fault exists to demand.
                None => Json::Null,
            },
        );
        row.set(
            "layer",
            cell_layer_to_json(&cell_robot_layer(&cell, &robot.slug))
                .get("layer")
                .cloned()
                .unwrap_or(Json::Null),
        );
        robots.push(row);
    }
    out.set("robots", robots);

    out.set(
        "workspace_note",
        Json::from(
            "§55.2 sends these volumes to the `workspace` guard, which is the kernel's (§7.1). §33.1 \
             keeps this daemon off that path, so the layer is computed here and reported rather \
             than applied — a console that showed the facts and omitted the volumes would suggest \
             there were none.",
        ),
    );
    if !faults.is_empty() {
        out.set(
            "note",
            Json::from(
                "The cell applies and these faults name what was dropped from it. A fact key with \
                 a dot or a space in it is a key no precondition can address, so §54.2 refuses any \
                 skill that names one — and this list is the only place that says why. The profile \
                 is not refused for them: it layers ceilings that only narrow (§38.2), and dropping \
                 it would leave this robot on wider limits than its operator asked for.",
            ),
        );
    }
    out
}

/// The parking pose, per channel, as §48 stores it.
fn parking(cell: &CellDescriptor) -> Json {
    let mut out = Json::obj();
    for channel in &cell.parking_pose {
        out.set(&channel.channel, Json::Num(channel.position));
    }
    out
}

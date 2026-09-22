//! §54.2's safety notes, and the acknowledgement that makes them exist.
//!
//! > `safety_notes` stops being decorative too: it is displayed at grant time and acknowledged once
//! > per skill per robot, recorded in the audit log. **A safety note nobody has read is a safety
//! > note that does not exist.**
//!
//! The manifest field has been carried by the Hub since skills were built and read by nothing. This
//! is the half that gives it teeth on a robot.
//!
//! # Why the grant, and not the invocation
//!
//! §54.2 names the moment and it is the right one. A grant is the operator saying *this package may
//! use this capability on this machine* — it is the one point in a skill's life where a person is
//! present, deciding, and able to stop. An invocation is a robot doing what it was told, often by a
//! reconcile loop at three in the morning, and a prompt there is a prompt nobody reads.
//!
//! It also fails in the safe direction. An unacknowledged note blocks a **widening** of what a
//! package may do; revocation, stopping and every other narrowing path is untouched, because
//! §39.1's rule is that the safe direction never waits.
//!
//! # Bound to the text, not to the name
//!
//! An acknowledgement records the **digest of the notes**, so a publisher who edits a warning has
//! written a new warning and it is acknowledged again. §54.2 says *"once per skill per robot"*, and
//! the reading that makes the sentence worth anything is the one where a changed note is a note
//! nobody has read yet — the alternative lets a package acquire its warning after the operator
//! agreed to a blank one.
//!
//! # It is a record, not a gate somebody can be talked past
//!
//! The acknowledgement is written to `<state>/acknowledgements.json` and to the audit log (§38.3),
//! which is the one surface §41.1 keeps for *"who changed what, when"*. A person who cannot find
//! evidence that a note was read is entitled to conclude it was not.

use std::collections::BTreeMap;

use skynet_json::Json;

use crate::layout::{Layout, PackageId};

/// A package's safety notes, and what they hash to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notes {
    /// The text, as the manifest declares it.
    pub text: String,
    /// `sha256:…` over the exact bytes of that text.
    pub digest: String,
}

/// The notes a manifest declares, when it declares any.
///
/// `None` for an absent, empty or whitespace-only field — all three are a publisher who wrote no
/// warning, and requiring an acknowledgement of nothing would train an operator to acknowledge
/// without reading, which is the failure §54.2's sentence is about.
pub fn of(manifest: &Json) -> Option<Notes> {
    let text = manifest
        .get_present("safety_notes")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .trim();
    if text.is_empty() {
        return None;
    }
    Some(Notes {
        digest: format!(
            "sha256:{}",
            String::from_utf8_lossy(&skynet_sha256::hex_bytes(text.as_bytes()))
        ),
        text: text.to_string(),
    })
}

/// Where the acknowledgements live.
fn path(layout: &Layout) -> std::path::PathBuf {
    layout.state().join("acknowledgements.json")
}

/// Every acknowledgement this robot holds, as `package -> notes digest`.
///
/// One digest per package rather than a list: §54.2 asks for the *current* notes to have been read,
/// and a history of acknowledged texts would be a record of what an operator agreed to in the past,
/// which is the audit log's job and not this file's.
pub fn held(layout: &Layout) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(document) = crate::observe::read_json(&path(layout)) else {
        return out;
    };
    if let Some(map) = document.as_obj() {
        for (package, digest) in map {
            if let Some(digest) = digest.as_str() {
                out.insert(package.clone(), digest.to_string());
            }
        }
    }
    out
}

/// Whether the operator has read *these* notes for this package.
pub fn acknowledged(layout: &Layout, package: &str, notes: &Notes) -> bool {
    held(layout)
        .get(package)
        .is_some_and(|digest| digest == &notes.digest)
}

/// Record that they have, in the file and in the audit log.
///
/// Both, because they answer different questions. The file answers *may this grant proceed*, which
/// has to be decidable offline and instantly; the log answers *who read it and when*, which is what
/// somebody asks after an incident and is the only one of the two that cannot be reconstructed.
pub fn acknowledge(
    layout: &Layout,
    package: &PackageId,
    notes: &Notes,
    audit: &crate::audit::Audit,
    at: &str,
    actor: &str,
) -> std::io::Result<()> {
    let mut all = held(layout);
    all.insert(package.as_str(), notes.digest.clone());

    let mut document = Json::obj();
    for (id, digest) in &all {
        document.set(id, Json::Str(digest.clone()));
    }
    let text = document
        .to_canonical()
        .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
    crate::observe::write_atomic(&path(layout), text.as_bytes())?;

    // The first line of the notes, so the log says what was agreed to rather than only that
    // something was. The digest is the identifier; the sentence is what a person reads back.
    let first = notes.text.lines().next().unwrap_or_default();
    let _ = audit.append(
        at,
        actor,
        "safety_notes_acknowledged",
        &package.as_str(),
        &format!("{} — {first}", notes.digest),
        None,
    );
    Ok(())
}

/// What a refused grant answers with (§54.2, §64: every refusal names the fix).
pub fn refusal(package: &PackageId, notes: &Notes) -> Json {
    let mut out = Json::obj();
    // The code beside the outcome: `outcome` says what the grant attempt came to, and `code`
    // classifies the error the way every other refusal on this API is classified, so the one
    // reader — `report_error` in the CLI, the fleet matching on codes — reads one member.
    out.set(
        "code",
        Json::Str("api.grants.safety_notes_unread".to_string()),
    )
    .set("outcome", Json::Str("safety_notes_unread".to_string()))
    .set("id", Json::Str(package.as_str()))
    .set("safety_notes", Json::Str(notes.text.clone()))
    .set("digest", Json::Str(notes.digest.clone()))
    .set(
        "detail",
        Json::Str(format!(
            "§54.2: this skill's safety notes have not been acknowledged on this robot. They \
                 are above; grant again with `acknowledge` set to `{}` once they have been read. A \
                 safety note nobody has read is a safety note that does not exist.",
            notes.digest
        )),
    );
    out
}

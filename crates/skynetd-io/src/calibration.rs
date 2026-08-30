//! Calibration on the robot (§45).
//!
//! [`skynet_contracts::calibration`] decides *whether a record is still believed*; this reads the
//! records off the disk, establishes the conditions the robot is in now, checks the signatures, and
//! turns the answer into something a console and a CLI can show.
//!
//! # The records
//!
//! `/data/state/calibration/<set>.json`, one per parameter set, with a detached
//! `<set>.sig.json` beside it — detached for the same reason the embodiment descriptor's is (§5):
//! the digest is taken over the record's canonical form, so a signature written *inside* would
//! change the digest it is a signature of.
//!
//! Signed because §45.3 says so, and it says so for the reason profiles are signed: the numbers
//! reach the kernel. An unsigned record is reported rather than refused — the same asymmetry §37.1
//! records for packages, and for the same reason: nothing on a robot can produce a signature, so
//! refusing unsigned records would refuse every calibration a robot ever takes of itself.
//!
//! # `now`, and the fields this robot cannot fill
//!
//! §45.4's conditions are elapsed time, joint travel, temperature and events. This robot can answer
//! the first — a wall clock is a wall clock — and **cannot answer the other three**: there is no
//! policy runtime posting joint travel, no thermal channel, and no incident path to record a
//! collision on. So they are `None`, and the contract's rule takes over: *a missing current reading
//! expires the record rather than extending it.*
//!
//! That is a deliberately loud result. A robot with a temperature-sensitive F/T bias will report
//! `envelope` unavailable on day one, naming a reading nothing supplies, which is the honest state
//! of a machine whose thermal channel is unwritten — and it is exactly the sentence that would go
//! missing if the rule were "check it when you can".

use skynet_contracts::calibration::{
    evaluate, guards_unavailable, sets_for, CalibrationRecord, Conditions, Validity,
};
use skynet_json::Json;

use crate::layout::Layout;

/// The domain tag for a calibration record's signature (§45.3).
///
/// Its own tag, because §45.3 signs a *measurement* and §38.1 signs a *limit*: without one, a
/// signature over a record's digest would verify over a profile with the same digest, and those are
/// separate decisions by separate people.
pub const CALIBRATION_DOMAIN: &str = "skynet.calibration.v1\n";

/// The message a calibration signature is made over.
pub fn calibration_message(digest: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(CALIBRATION_DOMAIN.len() + digest.len());
    out.extend_from_slice(CALIBRATION_DOMAIN.as_bytes());
    out.extend_from_slice(digest.as_bytes());
    out
}

/// One parameter set as this robot holds it.
#[derive(Debug, Clone)]
pub struct Held {
    /// Which set.
    pub set: String,
    /// The verdict.
    pub validity: Validity,
    /// The record, when one was read.
    pub record: Option<CalibrationRecord>,
    /// What is known about its signature.
    pub signature: crate::trust::Verdict,
    /// A file-level problem: unreadable, unparsable, or filed under the wrong name.
    pub fault: Option<String>,
}

/// Everything this robot knows about its calibration.
#[derive(Debug, Clone)]
pub struct Status {
    /// The conditions used to judge, so a reader can see what was and was not available.
    pub now: Conditions,
    /// One entry per applicable set, in the order [`sets_for`] gives them.
    pub held: Vec<Held>,
    /// Which sets this body can hold at all.
    pub applicable: Vec<String>,
}

/// Read a record, distinguishing "no file" from "a file that cannot be used".
///
/// Three outcomes rather than two, and the middle one is why: a missing record is a set nobody has
/// measured, and an unreadable one is a set somebody measured and whose record is now broken. They
/// send an operator to completely different places.
fn read_record(
    path: &std::path::Path,
    set: &str,
) -> Result<Option<(CalibrationRecord, Json)>, String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("could not be read: {error}")),
    };
    let document = Json::parse(&text).map_err(|error| format!("does not parse: {error}"))?;
    let record = skynet_contracts::json::calibration_from_json(&document)
        .map_err(|error| format!("does not decode: {error:?}"))?;

    // A file named `camera_extrinsics.json` whose contents say `joint_zeros` is not a small
    // inconsistency: whichever one is wrong, some set is being judged by another set's policy, and
    // the guards that lose their input are the wrong guards.
    if record.set != set {
        return Err(format!(
            "is filed as `{set}` and declares itself `{}` — one of the two is wrong, and until \
             somebody says which, this record would judge the wrong guards",
            record.set
        ));
    }
    Ok(Some((record, document)))
}

/// The conditions this robot is in now (§45.4).
///
/// `wall_s` is supplied rather than read, for the reason the kernel's clock is (§35.2): a function
/// that reads a clock cannot be tested against a fixed one.
///
/// The other three are `None` and will stay `None` until the things that would produce them exist.
/// Filling them with plausible values is the one failure this whole subsystem is built to prevent.
pub fn conditions_now(wall_s: Option<f64>) -> Conditions {
    Conditions {
        at_s: wall_s,
        // §47's sensor pipeline is unwritten, so nothing on this robot reads a temperature.
        temperature_c: None,
        // The policy runtime is unwritten, so no cumulative travel counter exists.
        joint_travel_rad: None,
        tool: None,
        payload_kg: None,
    }
}

/// Read and judge every applicable set.
pub fn status(layout: &Layout, wall_s: Option<f64>) -> Status {
    let body = crate::embodiment::load(layout);
    let (capabilities, base) = match body.known() {
        Some(known) => (
            known.descriptor.capabilities.clone(),
            Some(known.descriptor.base.clone()),
        ),
        // A robot that has not been told what it is has an unknown base, which `sets_for` treats
        // as mobile — the conservative direction, and the one that does not quietly retire a guard.
        None => (Vec::new(), None),
    };
    let applicable: Vec<String> = sets_for(&capabilities, base.as_deref())
        .iter()
        .map(|set| (*set).to_string())
        .collect();

    let now = conditions_now(wall_s);
    let trust = crate::trust::TrustStore::load(layout);
    let directory = layout.calibration();

    let mut held = Vec::new();
    for set in &applicable {
        let path = directory.join(format!("{set}.json"));
        match read_record(&path, set) {
            Ok(Some((record, document))) => {
                let signature = signature_of(&trust, &directory, set, &document);
                held.push(Held {
                    set: set.clone(),
                    validity: evaluate(&record, &now),
                    record: Some(record),
                    signature,
                    fault: None,
                });
            }
            Ok(None) => held.push(Held {
                set: set.clone(),
                validity: Validity::Absent,
                record: None,
                signature: crate::trust::Verdict {
                    state: skynet_contracts::admission::SignatureState::Absent,
                    signer: None,
                    detail: String::new(),
                },
                fault: None,
            }),
            Err(detail) => held.push(Held {
                set: set.clone(),
                // A record that cannot be read is not a record that was never taken, and the
                // difference is the whole of `Unusable`: somebody measured this and the evidence is
                // now unusable, which is a thing to go and look at rather than a thing to schedule.
                validity: Validity::Unusable {
                    detail: format!("the stored record {detail}"),
                },
                record: None,
                signature: crate::trust::Verdict {
                    state: skynet_contracts::admission::SignatureState::Absent,
                    signer: None,
                    detail: String::new(),
                },
                fault: Some(detail),
            }),
        }
    }

    Status {
        now,
        held,
        applicable,
    }
}

/// Check a record's detached signature against the trust store.
fn signature_of(
    trust: &crate::trust::TrustStore,
    directory: &std::path::Path,
    set: &str,
    document: &Json,
) -> crate::trust::Verdict {
    let digest = match crate::embodiment::digest_of(document) {
        Ok(digest) => digest,
        Err(detail) => {
            return crate::trust::Verdict {
                state: skynet_contracts::admission::SignatureState::Failed,
                signer: None,
                detail: format!(
                    "this record cannot be digested, so nothing can be checked against \
                                 a signature: {detail}"
                ),
            }
        }
    };
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();

    match crate::trust::read_detached(&directory.join(format!("{set}.sig.json"))) {
        Ok(signed) => trust.check(&calibration_message(&hex), signed.as_ref()),
        // A signature file that exists and cannot be read is `failed` rather than `absent`:
        // somebody put it there, and "the file you wrote is unusable" is a different job from
        // "nobody signed this".
        Err(detail) => crate::trust::Verdict {
            state: skynet_contracts::admission::SignatureState::Failed,
            signer: None,
            detail,
        },
    }
}

impl Status {
    /// The guards that lose their input, per §45.5.
    pub fn unavailable(&self) -> Vec<skynet_contracts::calibration::GuardUnavailable> {
        let judged: Vec<(String, Validity)> = self
            .held
            .iter()
            .map(|held| (held.set.clone(), held.validity.clone()))
            .collect();
        let applicable: Vec<&str> = self.applicable.iter().map(String::as_str).collect();
        guards_unavailable(&judged, &applicable)
    }

    /// The whole status, for `GET /calibration` and the console.
    pub fn to_json(&self) -> Json {
        let mut sets = Json::arr();
        for held in &self.held {
            let mut row = skynet_contracts::json::validity_to_json(&held.set, &held.validity);
            row.set(
                "signature",
                Json::Str(held.signature.state.as_str().to_string()),
            );
            row.set(
                "signer",
                match &held.signature.signer {
                    Some(label) => Json::Str(label.clone()),
                    None => Json::Null,
                },
            );
            if !held.signature.detail.is_empty() {
                row.set("signature_detail", Json::Str(held.signature.detail.clone()));
            }
            if let Some(record) = &held.record {
                row.set(
                    "method",
                    match &record.method {
                        Some(method) => Json::Str(method.clone()),
                        None => Json::Null,
                    },
                );
                row.set(
                    "operator",
                    match &record.operator {
                        Some(operator) => Json::Str(operator.clone()),
                        None => Json::Null,
                    },
                );
                row.set(
                    "residual",
                    match record.residual {
                        Some(residual) => Json::Num(residual),
                        None => Json::Null,
                    },
                );
            }
            sets.push(row);
        }

        let mut unavailable = Json::arr();
        for entry in self.unavailable() {
            unavailable.push(skynet_contracts::json::guard_unavailable_to_json(&entry));
        }

        // The conditions, including the ones that are missing — because a reader looking at three
        // expired records needs to know whether this robot drifted or simply cannot measure.
        let mut now = Json::obj();
        now.set(
            "at_s",
            match self.now.at_s {
                Some(at) => Json::Num(at),
                None => Json::Null,
            },
        );
        now.set("temperature_c", Json::Null);
        now.set("joint_travel_rad", Json::Null);
        let mut unmeasured = Json::arr();
        unmeasured.push(Json::Str(
            "temperature_c — §47's sensor pipeline is unwritten, so nothing on this robot reads a \
             temperature"
                .to_string(),
        ));
        unmeasured.push(Json::Str(
            "joint_travel_rad — the policy runtime is unwritten, so no cumulative travel counter \
             exists"
                .to_string(),
        ));
        unmeasured.push(Json::Str(
            "events — §34.4's outbound queue and the incident path are unwritten, so no collision \
             or estop is recorded against a set"
                .to_string(),
        ));

        let mut out = Json::obj();
        out.set("now", now);
        out.set("unmeasured", unmeasured);
        out.set("sets", sets);
        out.set("unavailable", unavailable);
        // Said here, because a page listing "proximity unavailable: the extrinsic expired" beside a
        // guards panel listing "proximity unavailable: no separation distance from a sensor" invites
        // exactly one wrong conclusion — that re-measuring the extrinsic would restore the guard.
        // It would not. §27's kernel and the policy runtime are unwritten, so nothing on this robot
        // evaluates a cycle, and every guard is unavailable for that reason first. What this panel
        // reports is what would *still* be missing once they exist.
        out.set(
            "precedes",
            Json::Str(
                "every guard here is already unavailable for a prior reason: §27's kernel and the \
                 policy runtime are unwritten, so nothing on this robot evaluates a cycle. These \
                 are the calibrations that would still be missing once they do"
                    .to_string(),
            ),
        );
        out
    }
}

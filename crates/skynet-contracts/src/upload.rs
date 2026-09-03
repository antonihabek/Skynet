//! §79's upload — moving a sealed black box window to the Hub, and deciding what the far side's
//! answer means.
//!
//! The mirror of [`crate::bundle`], and the whole of the difference is one sentence from §79.2:
//!
//! > §75's fetch has a rule that makes progress free: *"the partial's length is the progress"*. An
//! > upload cannot have that rule, because **the partial is on the other machine.**
//!
//! Three consequences, and they are the reason this is a separate module rather than a flag on
//! [`crate::bundle::plan_fetch`]:
//!
//! 1. **There is a probe.** A decision this side cannot reach without asking, so
//!    [`UploadDecision::Probe`] is a decision rather than a step hidden inside a caller. An edition
//!    that folded it into `start` would re-send whole windows.
//! 2. **The offset is a claim, not evidence.** It places the next byte and it never concludes a
//!    transfer. That conclusion is the digest the receiver computed against the digest recorded at
//!    the seal, before the transfer existed (§77.1).
//! 3. **A failure cannot discard anything.** [`crate::bundle::check_bundle`] deletes a file the
//!    robot owns; [`check_upload`] can only stop believing an offset and start again at zero. The
//!    robot has no authority over the receiver's disk, and a contract that pretended otherwise
//!    would be one whose remedy nothing could carry out.
//!
//! # The size is known here, and it is not known there
//!
//! [`crate::bundle::FetchSubject::size`] is an `Option` because a release manifest may not declare
//! one. [`UploadSubject::size`] is not: the window is a file this robot sealed and measured, so its
//! length is a fact rather than a claim, and the branch §12 leaves unexercised — *"a partial cannot
//! be larger than what it is part of"* — is reachable on every upload.
//!
//! # This decides, and does not transfer
//!
//! No network, no filesystem, no clock, exactly as [`crate::bundle`].

use alloc::format;
use alloc::string::{String, ToString};

use skynet_json::Json;

pub use crate::bundle::Bearer;

/// What to do about one sealed window (§79).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadDecision {
    /// The receiver has it, and has said so by digest.
    Done,
    /// Ask how much of it the receiver holds. The step a download does not have.
    Probe,
    /// Send it, from the beginning.
    Start,
    /// Send the rest of it, from the receiver's offset.
    Resume,
    /// Send it again from the beginning: what is over there is not part of this window.
    Restart,
    /// A metered link, and a window is bulk (§34.3).
    WaitForUnmetered,
    /// Nothing is up.
    NoBearer,
    /// There is nothing here worth sending.
    Refuse,
}

impl UploadDecision {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            UploadDecision::Done => "done",
            UploadDecision::Probe => "probe",
            UploadDecision::Start => "start",
            UploadDecision::Resume => "resume",
            UploadDecision::Restart => "restart",
            UploadDecision::WaitForUnmetered => "wait_for_unmetered",
            UploadDecision::NoBearer => "no_bearer",
            UploadDecision::Refuse => "refuse",
        }
    }
}

/// Every decision, in the order the tests run.
pub const UPLOAD_DECISIONS: [UploadDecision; 8] = [
    UploadDecision::Done,
    UploadDecision::Probe,
    UploadDecision::Start,
    UploadDecision::Resume,
    UploadDecision::Restart,
    UploadDecision::WaitForUnmetered,
    UploadDecision::NoBearer,
    UploadDecision::Refuse,
];

/// What the receiver says it holds under this digest.
///
/// `digest` is `None` because a receiver holding a partial has nothing meaningful to hash — and
/// because a receiver holding a *complete* object and declining to say what it hashes to has given
/// this robot nothing it can act on, which §79.2 resolves by restarting rather than by assuming.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Held {
    /// How many bytes it claims.
    pub bytes: u64,
    /// What it says they hash to, when it says.
    pub digest: Option<String>,
}

/// The window, and what is known about the far side of the link (§79.2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UploadSubject {
    /// `sha256:…` recorded at the seal. The address, and the only evidence in this structure.
    pub digest: String,
    /// The window's length. A file this robot owns, so this is measured rather than declared.
    pub size: u64,
    /// What the receiver reported, or `None` where it has not been asked this pass.
    pub held: Option<Held>,
    /// Whether a receipt for these exact bytes is already on the disk (§79.4).
    pub receipt: bool,
    /// The active bearer, when there is one.
    pub bearer: Option<Bearer>,
}

/// §79's answer for one window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadPlan {
    /// Which of the eight.
    pub decision: UploadDecision,
    /// The byte offset to send from. Zero unless the decision is [`UploadDecision::Resume`].
    pub from: u64,
    /// The sentence an operator reads.
    pub detail: String,
}

/// What to do about one sealed window (§79, §34.3).
///
/// The order of the tests is the order of cost, as in [`crate::bundle::plan_fetch`]. Having a
/// receipt is free; refusing costs nothing; waiting for a link costs nothing; asking costs one
/// small request; and only then are megabytes considered.
pub fn plan_upload(subject: &UploadSubject) -> UploadPlan {
    let digest = subject.digest.trim();
    let size = subject.size;

    if subject.receipt {
        return UploadPlan {
            decision: UploadDecision::Done,
            from: 0,
            detail: "The receiver has acknowledged this window by digest and the receipt is on \
                     the disk. Nothing to send."
                .to_string(),
        };
    }

    if digest.is_empty() {
        return UploadPlan {
            decision: UploadDecision::Refuse,
            from: 0,
            detail: "This window carries no digest, so there is no address to send it to and \
                     nothing to check the far side's answer against. §79.1 makes the digest the \
                     route, and bytes with no name have nowhere to go."
                .to_string(),
        };
    }

    if size == 0 {
        return UploadPlan {
            decision: UploadDecision::Refuse,
            from: 0,
            detail: "This window is empty. A seal that produced no bytes is a fact about the \
                     recorder worth reporting (§77.2) and is not something to upload — a \
                     zero-length object under a digest would be an artifact an investigator could \
                     open and mistake for the window."
                .to_string(),
        };
    }

    let Some(bearer) = &subject.bearer else {
        return UploadPlan {
            decision: UploadDecision::NoBearer,
            from: 0,
            detail:
                "No bearer is up, so there is nothing to send over. That is the ordinary state \
                     for most of a robot's life (§34.1) and nothing is lost by it — the window is \
                     sealed on the disk and the incident naming it is already queued."
                    .to_string(),
        };
    };

    if bearer.metered {
        return UploadPlan {
            decision: UploadDecision::WaitForUnmetered,
            from: 0,
            detail: format!(
                "A sealed window is tens of megabytes and `{}` is {} metered, so it waits for an \
                 unmetered link (§34.3). The alarm does not wait: §79.3 puts the incident on any \
                 bearer that is up, so an operator already knows this recording exists and what it \
                 hashes to.",
                bearer.name,
                if bearer.metering_declared {
                    "declared"
                } else {
                    "assumed"
                }
            ),
        };
    }

    let Some(held) = &subject.held else {
        return UploadPlan {
            decision: UploadDecision::Probe,
            from: 0,
            detail: "The receiver has not been asked how much of this window it holds. §79.2: the \
                     partial is on the other machine, so an upload has a step a download does not \
                     — and a robot that skipped it would re-send the whole window every pass."
                .to_string(),
        };
    };

    let bytes = held.bytes;
    let held_digest = held.digest.as_deref().unwrap_or("").trim();

    if bytes >= size {
        if bytes == size && !held_digest.is_empty() && held_digest == digest {
            return UploadPlan {
                decision: UploadDecision::Done,
                from: 0,
                detail: format!(
                    "The receiver holds all {size} bytes and says they hash to this window's \
                     digest. Nothing is sent and a receipt is written — which is the state a robot \
                     reaches after an upload whose acknowledgement was lost."
                ),
            };
        }
        return UploadPlan {
            decision: UploadDecision::Restart,
            from: 0,
            detail: format!(
                "The receiver claims {bytes} bytes for a window of {size}{}. It is sent again from \
                 the beginning.",
                if bytes == size {
                    ", and does not say what they hash to. A complete object nobody will name is \
                     not evidence this window arrived, and writing a receipt for it would record \
                     an upload that may never have happened"
                } else {
                    ". A partial cannot be larger than what it is part of, so what is over there \
                     is not part of this window"
                }
            ),
        };
    }

    if bytes > 0 {
        return UploadPlan {
            decision: UploadDecision::Resume,
            from: bytes,
            detail: format!(
                "Resuming at {bytes} of {size} bytes. The offset is the receiver's claim and is \
                 used to place the next byte, never to conclude the window arrived — that is the \
                 digest's job (§79.2)."
            ),
        };
    }

    UploadPlan {
        decision: UploadDecision::Start,
        from: 0,
        detail: format!("Sending {size} bytes over `{}`.", bearer.name),
    }
}

// ── What the receiver ended up with ──────────────────────────────────────────

/// What the far side turned out to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadCheckCode {
    /// The whole window, hashing to what was sealed.
    Complete,
    /// Fewer bytes than were sent. A transfer that stopped.
    Short,
    /// More bytes than the window has. Not this object.
    Long,
    /// The right number of different bytes.
    DigestMismatch,
}

impl UploadCheckCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            UploadCheckCode::Complete => "complete",
            UploadCheckCode::Short => "short",
            UploadCheckCode::Long => "long",
            UploadCheckCode::DigestMismatch => "digest_mismatch",
        }
    }
}

/// Every outcome, in the order the checks run.
pub const UPLOAD_CHECKS: [UploadCheckCode; 4] = [
    UploadCheckCode::Complete,
    UploadCheckCode::Short,
    UploadCheckCode::Long,
    UploadCheckCode::DigestMismatch,
];

/// What the receiver reported after the last byte was offered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reported {
    /// How many bytes it says it holds.
    pub bytes: u64,
    /// What it says they hash to, when it says.
    pub digest: Option<String>,
}

/// What this robot sealed, and measured.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Window {
    /// `sha256:…`, recorded at the seal.
    pub digest: String,
    /// The file's length.
    pub size: u64,
}

/// §79's verdict on one upload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadCheck {
    /// Which of the four.
    pub code: UploadCheckCode,
    /// Whether a receipt may be written for this window.
    pub ok: bool,
    /// Whether the next attempt must begin at zero rather than resume.
    pub restart: bool,
    /// The sentence an operator reads.
    pub detail: String,
}

/// Did the window arrive (§79.2, §79.4)?
///
/// The same four outcomes as [`crate::bundle::check_bundle`] and one different remedy, which is the
/// asymmetry worth naming: a bundle that fails its check is **deleted**, because the robot owns
/// those bytes. A window that fails this check cannot be deleted from anywhere — the robot can only
/// decline to trust the offset it was given and begin again. `restart` is therefore about what this
/// robot will do next, never about what the receiver should do with what it has.
///
/// Short keeps the offset, for `check_bundle`'s reason in the other direction: a transfer that
/// stopped early is what a link dropping looks like, and it is exactly what a resume is for.
pub fn check_upload(reported: &Reported, window: &Window) -> UploadCheck {
    let want = window.digest.trim();
    let got = reported.digest.as_deref().unwrap_or("").trim();
    let size = window.size;
    let bytes = reported.bytes;

    if bytes < size {
        return UploadCheck {
            code: UploadCheckCode::Short,
            ok: false,
            restart: false,
            detail: format!(
                "The receiver holds {bytes} of {size} bytes. The transfer stopped early, which is \
                 what a link dropping looks like, and the next pass resumes from what is there."
            ),
        };
    }

    if bytes > size {
        return UploadCheck {
            code: UploadCheckCode::Long,
            ok: false,
            restart: true,
            detail: format!(
                "The receiver holds {bytes} bytes for a window of {size}. More than was sent is \
                 not a transfer that stopped — it is another object under this digest, or a proxy \
                 answering for one. The next attempt begins at zero; this robot cannot delete what \
                 is over there and does not pretend to."
            ),
        };
    }

    if want.is_empty() || got.is_empty() || want != got {
        return UploadCheck {
            code: UploadCheckCode::DigestMismatch,
            ok: false,
            restart: true,
            detail: "The receiver holds the right number of bytes and they are not this window's. \
                     The digest was recorded at the seal, before any transfer existed (§77.1), so \
                     it is the half of this comparison that is evidence. No receipt is written and \
                     the next attempt begins at zero."
                .to_string(),
        };
    }

    UploadCheck {
        code: UploadCheckCode::Complete,
        ok: true,
        restart: false,
        detail: "The receiver holds the whole window and it hashes to what was sealed. A receipt \
                 is written beside the recording; the recording itself is kept, because §39.1's \
                 local route is for the operator standing next to the robot (§79.4)."
            .to_string(),
    }
}

/// One upload plan, as the corpus holds it.
pub fn upload_plan_to_json(plan: &UploadPlan) -> Json {
    let mut out = Json::obj();
    out.set("decision", Json::Str(plan.decision.as_str().to_string()))
        .set("detail", Json::Str(plan.detail.clone()))
        .set("from", Json::Num(plan.from as f64));
    out
}

/// One upload check, as the corpus holds it.
pub fn upload_check_to_json(check: &UploadCheck) -> Json {
    let mut out = Json::obj();
    out.set("code", Json::Str(check.code.as_str().to_string()))
        .set("detail", Json::Str(check.detail.clone()))
        .set("ok", Json::Bool(check.ok))
        .set("restart", Json::Bool(check.restart));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ethernet() -> Bearer {
        Bearer {
            name: "eth0".to_string(),
            metered: false,
            metering_declared: true,
        }
    }

    fn cellular() -> Bearer {
        Bearer {
            name: "wwan0".to_string(),
            metered: true,
            metering_declared: false,
        }
    }

    fn subject() -> UploadSubject {
        UploadSubject {
            digest: "sha256:aa".to_string(),
            size: 1_000,
            held: None,
            receipt: false,
            bearer: Some(ethernet()),
        }
    }

    #[test]
    fn a_window_with_a_receipt_is_not_sent_over_anything() {
        // Order-of-cost, and the case that catches getting it wrong: a robot that already has an
        // acknowledgement should not report itself as waiting for a link.
        let plan = plan_upload(&UploadSubject {
            receipt: true,
            bearer: None,
            ..subject()
        });
        assert_eq!(plan.decision, UploadDecision::Done);
    }

    #[test]
    fn the_probe_comes_before_any_bytes_and_after_the_bearer() {
        // The whole of §79.2 in two assertions: a robot on a metered link does not even ask, and a
        // robot on an unmetered one asks before it sends.
        assert_eq!(
            plan_upload(&UploadSubject {
                bearer: Some(cellular()),
                ..subject()
            })
            .decision,
            UploadDecision::WaitForUnmetered
        );
        assert_eq!(plan_upload(&subject()).decision, UploadDecision::Probe);
    }

    #[test]
    fn an_empty_window_is_refused_rather_than_uploaded() {
        let plan = plan_upload(&UploadSubject {
            size: 0,
            ..subject()
        });
        assert_eq!(plan.decision, UploadDecision::Refuse);
        assert!(plan.detail.contains("empty"));
    }

    #[test]
    fn a_receiver_holding_nothing_starts_and_one_holding_some_resumes() {
        let plan = plan_upload(&UploadSubject {
            held: Some(Held::default()),
            ..subject()
        });
        assert_eq!(plan.decision, UploadDecision::Start);
        assert_eq!(plan.from, 0);

        let plan = plan_upload(&UploadSubject {
            held: Some(Held {
                bytes: 400,
                digest: None,
            }),
            ..subject()
        });
        assert_eq!(plan.decision, UploadDecision::Resume);
        assert_eq!(plan.from, 400);
    }

    #[test]
    fn a_receiver_claiming_more_than_the_window_is_not_resumed_from() {
        // `plan_fetch`'s `discard_and_fetch`, in the other direction. Resuming here would append
        // this window's tail to whatever else is under the digest.
        let plan = plan_upload(&UploadSubject {
            held: Some(Held {
                bytes: 1_400,
                digest: None,
            }),
            ..subject()
        });
        assert_eq!(plan.decision, UploadDecision::Restart);
        assert_eq!(plan.from, 0);
        assert!(plan.detail.contains("cannot be larger"));
    }

    #[test]
    fn a_complete_object_is_done_only_when_the_receiver_names_it() {
        // The pair that matters. The same byte count, and the digest is the difference between a
        // receipt and a re-send.
        let named = plan_upload(&UploadSubject {
            held: Some(Held {
                bytes: 1_000,
                digest: Some("sha256:aa".to_string()),
            }),
            ..subject()
        });
        assert_eq!(named.decision, UploadDecision::Done);

        let anonymous = plan_upload(&UploadSubject {
            held: Some(Held {
                bytes: 1_000,
                digest: None,
            }),
            ..subject()
        });
        assert_eq!(anonymous.decision, UploadDecision::Restart);

        let wrong = plan_upload(&UploadSubject {
            held: Some(Held {
                bytes: 1_000,
                digest: Some("sha256:bb".to_string()),
            }),
            ..subject()
        });
        assert_eq!(wrong.decision, UploadDecision::Restart);
    }

    #[test]
    fn a_short_report_resumes_and_everything_else_starts_again() {
        let window = Window {
            digest: "sha256:aa".to_string(),
            size: 1_000,
        };
        let short = check_upload(
            &Reported {
                bytes: 600,
                digest: None,
            },
            &window,
        );
        assert_eq!(short.code, UploadCheckCode::Short);
        assert!(!short.restart);

        let long = check_upload(
            &Reported {
                bytes: 1_001,
                digest: Some("sha256:aa".to_string()),
            },
            &window,
        );
        assert_eq!(long.code, UploadCheckCode::Long);
        assert!(long.restart);

        let wrong = check_upload(
            &Reported {
                bytes: 1_000,
                digest: Some("sha256:bb".to_string()),
            },
            &window,
        );
        assert_eq!(wrong.code, UploadCheckCode::DigestMismatch);
        assert!(wrong.restart);

        let ok = check_upload(
            &Reported {
                bytes: 1_000,
                digest: Some("sha256:aa".to_string()),
            },
            &window,
        );
        assert_eq!(ok.code, UploadCheckCode::Complete);
        assert!(ok.ok);
    }

    #[test]
    fn a_receiver_that_says_nothing_about_a_complete_object_is_a_mismatch_rather_than_a_pass() {
        // The failure mode this ordering exists for: a receiver that answers `200` with an empty
        // body must never produce a receipt, because a receipt is the record that says the bytes
        // are somewhere else and this robot's copy is no longer the only one.
        let check = check_upload(
            &Reported {
                bytes: 1_000,
                digest: None,
            },
            &Window {
                digest: "sha256:aa".to_string(),
                size: 1_000,
            },
        );
        assert_eq!(check.code, UploadCheckCode::DigestMismatch);
        assert!(!check.ok);
    }
}

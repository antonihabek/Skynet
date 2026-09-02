//! §11's pull on a real machine: fetching a desired state, and deciding whether it may land.
//!
//! [`skynet_contracts::intent`] decides whether an offered document may become the desired state and
//! takes no network and no filesystem. This is the other half — the `GET`, the decode, and the one
//! atomic write.
//!
//! # The order is the whole of the safety here
//!
//! Fetch, **decode**, assess, *then* write. A version that wrote the response and let §33.8's ladder
//! sort it out afterwards would work almost always and would be wrong in the one case that matters:
//! a Hub — or a captive portal, or a proxy — returning something that is not a desired state would
//! overwrite a good one, and the robot would fall back to last-known-good on the next boot having
//! silently lost the intent it was reconciling against. §33.8 is a recovery ladder, not a licence
//! to write first.
//!
//! # Nothing here is in the control path
//!
//! §11: *"no control decision may require a network round trip."* A pull that fails changes nothing
//! — not the desired state, not the plan, not what is running. A robot that cannot reach its Hub
//! reconciles against what it has, which is what it does for most of its life (§34.1), and the only
//! observable difference is a line on `GET /intent` saying when it last succeeded.
//!
//! # What is still not fetched
//!
//! The **bundle**. §12's `stage_image` reports `Unimplemented` for want of the bytes, and this
//! module fetches a JSON document rather than a hundred-megabyte image — which wants resume,
//! progress and §34.3's metering, and is its own piece of work rather than a second call to
//! [`crate::connectivity::get_json`].

use skynet_contracts::intent::{assess_offer, offer_verdict_to_json, Held, Offer, OfferVerdict};
use skynet_json::Json;

use crate::layout::Layout;

/// The digest of a desired-state document, as the offer comparison uses it.
///
/// Over the **canonical** form rather than the bytes as they arrived, so that a Hub that
/// reformatted its JSON does not read as a changed document. Two documents that mean the same thing
/// have one digest, which is the property the reused-generation check needs to be about content
/// rather than about whitespace.
pub fn digest_of(document: &Json) -> Option<String> {
    let canonical = document.to_canonical().ok()?;
    let digest = skynet_sha256::hex_bytes(canonical.as_bytes());
    Some(format!(
        "sha256:{}",
        String::from_utf8_lossy(&digest).into_owned()
    ))
}

/// The digest of the desired state on this robot's disk, or empty when it holds none.
///
/// Its own function because two callers need it for two different reasons — [`held`] compares it
/// against an offer, and the check-in puts it on the wire so the Hub can make the same comparison —
/// and a second spelling of "what document is this robot running" is a second answer to it.
pub fn held_digest(layout: &Layout) -> String {
    crate::observe::read_json(&layout.desired())
        .as_ref()
        .and_then(digest_of)
        .unwrap_or_default()
}

/// What the robot is reconciling against now, read off its own disk.
pub fn held(layout: &Layout, running: usize) -> Held {
    let (desired, _) = crate::runtime::load_desired(layout);
    let digest = crate::observe::read_json(&layout.desired())
        .as_ref()
        .and_then(digest_of)
        .unwrap_or_default();
    Held {
        generation: desired.generation,
        digest,
        running,
    }
}

/// What one pull did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    /// Whether the Hub answered at all.
    ///
    /// Kept apart from the verdict, because *"the Hub said something this robot will not take"* and
    /// *"the Hub said nothing"* are the two states a fleet operator most needs to tell apart, and a
    /// single "did not adopt" would cover both.
    pub reached: bool,
    /// Why it did not answer, when it did not.
    pub detail: String,
    /// What §11 made of the document, when there was one.
    pub verdict: Option<OfferVerdict>,
    /// Whether the desired state on disk was replaced.
    pub written: bool,
}

impl Pulled {
    /// A pull that never got a document.
    pub fn unreachable(detail: impl Into<String>) -> Pulled {
        Pulled {
            reached: false,
            detail: detail.into(),
            verdict: None,
            written: false,
        }
    }
}

/// Fetch this robot's desired state and adopt it if §11 says it may (§11, §17, §33.8).
///
/// `Ok(None)` for an unpaired robot: there is no `/devices/{id}/desired` to ask, which is not a
/// failure — it is §50.2's shipping state, and a loop that logged an error every few seconds for it
/// would train an operator to ignore the line that matters.
pub fn pull(layout: &Layout, running: usize) -> Option<Pulled> {
    let (endpoint, device_id, credential) = crate::identity::credential_for_sending(layout)?;
    let url = format!(
        "{}/devices/{device_id}/desired",
        endpoint.trim_end_matches('/')
    );

    let answer = match crate::connectivity::get_json(&url, Some(&credential)) {
        Err(detail) => return Some(Pulled::unreachable(detail)),
        Ok(answer) => answer,
    };

    if !(200..300).contains(&answer.status) {
        let said = Json::parse(&answer.body)
            .ok()
            .and_then(|document| {
                document
                    .get("error")
                    .and_then(Json::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        return Some(Pulled::unreachable(format!(
            "the Hub answered HTTP {} to {url}{}",
            answer.status,
            if said.is_empty() {
                String::new()
            } else {
                format!(": {said}")
            }
        )));
    }

    Some(adopt(layout, &answer.body, running))
}

/// Decide and, where §11 allows, write. Separated from the fetch so the suite can drive every
/// branch without a network.
pub fn adopt(layout: &Layout, body: &str, running: usize) -> Pulled {
    let holding = held(layout, running);

    // Decoded here, once, through the decoder that already owns the question. A second
    // implementation of "is this a desired state" would be a second answer to it, and the two would
    // disagree on exactly the malformed document nobody anticipated.
    let (offer, document) = match Json::parse(body) {
        Err(error) => (
            Offer {
                decoded: false,
                decode_error: format!("does not parse: {error}"),
                ..Offer::default()
            },
            None,
        ),
        Ok(document) => match skynetd::desired::DesiredState::from_json(&document) {
            Err(error) => (
                Offer {
                    decoded: false,
                    decode_error: format!("does not decode: {error}"),
                    ..Offer::default()
                },
                None,
            ),
            Ok(desired) => (
                Offer {
                    decoded: true,
                    decode_error: String::new(),
                    generation: desired.generation,
                    digest: digest_of(&document).unwrap_or_default(),
                    packages: desired.packages.len(),
                },
                Some(document),
            ),
        },
    };

    let verdict = assess_offer(&offer, &holding);

    // Written only when it is both adopted and different. The ordinary poll is neither an error nor
    // a write: §11's loop asks the same question every few seconds, and a daemon that rewrote the
    // desired state on every tick would churn a file the black box and the audit log both care
    // about the mtime of.
    let mut written = false;
    if verdict.adopt && verdict.changed {
        if let Some(document) = &document {
            // Written **canonically**, not as it arrived. The digest this robot compares against on
            // the next poll is over the canonical form, so storing the Hub's own formatting would
            // make every poll report a change.
            if let Ok(canonical) = document.to_canonical() {
                written =
                    crate::observe::write_atomic(&layout.desired(), canonical.as_bytes()).is_ok();
            }
        }
    }

    Pulled {
        reached: true,
        detail: String::new(),
        verdict: Some(verdict),
        written,
    }
}

/// One pull as an API answer, for `GET /intent` and the console.
pub fn to_json(pulled: &Option<Pulled>, source: &crate::runtime::DesiredSource) -> Json {
    let mut out = Json::obj();

    // §33.8's ladder, which decides what the robot is reconciling against whether or not a Hub was
    // ever reached. It belongs on this route because "where did my desired state come from" has
    // exactly two answers — a pull, or the fallback — and a route showing one of them would leave
    // the other invisible.
    let (rung, because) = match source {
        crate::runtime::DesiredSource::Current => ("current", String::new()),
        crate::runtime::DesiredSource::LastKnownGood { because } => {
            ("last_known_good", because.clone())
        }
        crate::runtime::DesiredSource::RunNothing { because } => ("run_nothing", because.clone()),
    };
    out.set("source", Json::from(rung));
    out.set(
        "source_because",
        if because.is_empty() {
            Json::Null
        } else {
            Json::Str(because)
        },
    );

    match pulled {
        None => {
            out.set("pull", Json::Null).set(
                "note",
                Json::from(
                    "this robot is not paired (§10), so there is no `/devices/{id}/desired` to ask. \
                     It reconciles against the desired state on its own disk, which is what it does \
                     whenever it cannot reach a Hub (§34.1).",
                ),
            );
        }
        Some(pulled) => {
            let mut node = Json::obj();
            node.set("reached", Json::Bool(pulled.reached))
                .set("written", Json::Bool(pulled.written))
                .set(
                    "detail",
                    if pulled.detail.is_empty() {
                        Json::Null
                    } else {
                        Json::Str(pulled.detail.clone())
                    },
                )
                .set(
                    "verdict",
                    match &pulled.verdict {
                        Some(verdict) => offer_verdict_to_json(verdict),
                        None => Json::Null,
                    },
                );
            out.set("pull", node).set("note", Json::Null);
        }
    }
    out
}

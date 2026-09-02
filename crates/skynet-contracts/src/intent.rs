//! §11's pull — what a robot does with a desired state it has just been handed.
//!
//! > **The Hub stores intent. The robot reconciles.** Never push.
//! >
//! > `GET /devices/{id}/desired` → `{ os, packages, policy, profile, generation: 41 }`
//!
//! The document arrives over a network, which makes it the least trusted input in the system that is
//! also the most consequential: it decides what software runs on a machine that moves. §33.8 already
//! says what a robot does with a *corrupt file on its own disk* — last-known-good, then run nothing,
//! and it never invents one. This module answers the question one step earlier: **may this response
//! become that file at all?**
//!
//! # Refusing is not the same as ignoring
//!
//! Two of the four findings below refuse the offer, and in both cases the robot keeps reconciling
//! against what it already has. That is the correct outcome and not a degraded one — §34.1 assumes
//! the robot is offline most of the time, so *"carry on with the last good intent"* is the
//! **ordinary** state rather than a failure mode. A refused offer costs nothing that being offline
//! does not.
//!
//! # Why a reused generation is refused rather than adopted
//!
//! §11 rests one property on the counter: *"the `generation` counter makes it idempotent."* A Hub
//! that serves two different documents under one generation has broken that, and the robot is the
//! only party that can see it — the Hub has no way to know the robot already applied a different 41.
//!
//! The two ways to handle it are not symmetric:
//!
//! * **Adopt**, and the robot runs the new content while reporting generation 41 as applied. A fleet
//!   console then shows robots converged on 41 that are running different software, which is exactly
//!   the claim §17 forbids the platform from making — and it is invisible, because every robot looks
//!   right.
//! * **Refuse**, and the robot keeps the old content and reports a fault naming the Hub's mistake. A
//!   console shows a robot that is not adopting and says why; an operator bumps the generation.
//!
//! One produces a visible, actionable state and the other produces an invisible wrong one. So it
//! refuses.
//!
//! # And why a *lower* generation is adopted
//!
//! The opposite call, and for a reason worth writing down. A generation that goes backwards is a Hub
//! restored from a backup, and a robot that refused to follow one would be a fleet an operator could
//! not recover — permanently, with no remedy short of visiting every machine. The replay it would
//! otherwise guard against is already the thing the credential and TLS prevent. So it is adopted,
//! and reported, because *"intent went backwards"* is a sentence somebody wants to see once.
//!
//! # Pure
//!
//! No network, no filesystem. Whether the document decoded arrives decided, the way a signature
//! state does in §37.1, so the same function runs on the robot and on a Hub checking what it just
//! served.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use skynet_json::Json;

use crate::admission::Severity;

/// Why an offered desired state is not simply adopted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferFaultCode {
    /// It is not a desired-state document. Refuses.
    Undecodable,
    /// A different document under a generation this robot has already applied. Refuses.
    GenerationReused,
    /// A generation below the one this robot has applied. Adopted, and said out loud.
    GenerationWentBackwards,
    /// It names no packages and this robot is running some. Adopted, and said out loud.
    EmptiesTheRobot,
}

impl OfferFaultCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            OfferFaultCode::Undecodable => "undecodable",
            OfferFaultCode::GenerationReused => "generation_reused",
            OfferFaultCode::GenerationWentBackwards => "generation_went_backwards",
            OfferFaultCode::EmptiesTheRobot => "empties_the_robot",
        }
    }
}

/// Every fault, in the order the checks run.
pub const OFFER_FAULTS: [OfferFaultCode; 4] = [
    OfferFaultCode::Undecodable,
    OfferFaultCode::GenerationReused,
    OfferFaultCode::GenerationWentBackwards,
    OfferFaultCode::EmptiesTheRobot,
];

/// One thing found about an offered desired state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferFault {
    /// What.
    pub code: OfferFaultCode,
    /// Whether it refuses the offer.
    pub severity: Severity,
    /// A sentence naming the fix (§64).
    pub message: String,
}

/// A desired-state document the robot has just been handed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Offer {
    /// Whether it decoded.
    ///
    /// Arrives decided, because decoding is `DesiredState::from_json`'s job and a second
    /// implementation of it here would be a second answer to a question already answered.
    pub decoded: bool,
    /// Why it did not, when it did not.
    pub decode_error: String,
    /// The counter §11 rests idempotence on.
    pub generation: u64,
    /// `sha256:…` over the document's canonical form. What distinguishes two documents.
    pub digest: String,
    /// How many packages it names.
    pub packages: usize,
}

/// What the robot is reconciling against now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Held {
    /// The generation it has applied.
    pub generation: u64,
    /// The digest of the document it holds.
    pub digest: String,
    /// How many packages the robot is actually running.
    pub running: usize,
}

/// §11's answer for one offered document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferVerdict {
    /// Whether this document may become the desired state on disk.
    pub adopt: bool,
    /// Whether adopting it changes anything.
    ///
    /// False for the ordinary poll, which is most of them: §11's loop asks the same question every
    /// few seconds and the answer is usually the document it already has. Separated from `adopt` so
    /// a caller can skip the write without having to compare digests itself — and so that a log line
    /// exists for *"the Hub answered and nothing changed"*, which is different from *"the Hub did
    /// not answer"*.
    pub changed: bool,
    /// The generation the robot would be reconciling towards. Its own when the offer is refused.
    pub generation: u64,
    /// Everything found, in the order the checks run.
    pub faults: Vec<OfferFault>,
}

fn fault(code: OfferFaultCode, severity: Severity, message: String) -> OfferFault {
    OfferFault {
        code,
        severity,
        message,
    }
}

/// May this response become the desired state (§11, §17, §33.8)?
///
/// Every check runs and the result carries all of them, for [`crate::admit_package`]'s reason: a
/// fleet operator diagnosing why a robot will not take an update should get one list rather than one
/// problem per poll.
pub fn assess_offer(offer: &Offer, held: &Held) -> OfferVerdict {
    let mut faults = Vec::new();
    let held_digest = held.digest.trim();
    let digest = offer.digest.trim();

    if !offer.decoded {
        let why = offer.decode_error.trim();
        faults.push(fault(
            OfferFaultCode::Undecodable,
            Severity::Error,
            format!(
                "This response is not a desired state{}. It has not been written to disk, and this \
                 robot carries on reconciling against what it already has — which is what it does \
                 whenever it cannot reach the Hub at all (§34.1), so nothing about the robot is \
                 worse for it.",
                if why.is_empty() {
                    String::new()
                } else {
                    format!(": {why}")
                }
            ),
        ));
        return OfferVerdict {
            adopt: false,
            changed: false,
            generation: held.generation,
            faults,
        };
    }

    // The reused generation, and the reason it refuses. See the header.
    if offer.generation == held.generation
        && !digest.is_empty()
        && !held_digest.is_empty()
        && digest != held_digest
    {
        faults.push(fault(
            OfferFaultCode::GenerationReused,
            Severity::Error,
            format!(
                "The Hub is serving a different document under generation {}, which this robot has \
                 already applied. §11 rests idempotence on that counter, so adopting this would \
                 have the robot running new software while reporting the old generation as applied \
                 — and a fleet console would show it converged. Bump the generation on the Hub and \
                 this robot will take it on the next poll.",
                offer.generation
            ),
        ));
        return OfferVerdict {
            adopt: false,
            changed: false,
            generation: held.generation,
            faults,
        };
    }

    if offer.generation < held.generation {
        faults.push(fault(
            OfferFaultCode::GenerationWentBackwards,
            Severity::Warning,
            format!(
                "This robot has applied generation {} and the Hub is asking for {}. Intent has gone \
                 backwards, which is what a Hub restored from a backup looks like. It is adopted — \
                 a robot that refused to follow one would be a fleet nobody could recover — and it \
                 is reported, because it is worth somebody knowing once.",
                held.generation, offer.generation
            ),
        ));
    }

    if offer.packages == 0 && held.running > 0 {
        faults.push(fault(
            OfferFaultCode::EmptiesTheRobot,
            Severity::Warning,
            format!(
                "This desired state names no packages and this robot is running {}. That is what an \
                 operator emptying a robot looks like, and it is also what a Hub serving a stub \
                 looks like. It is adopted, because stopping is never the dangerous direction \
                 (§33.4) — and it is said out loud, because the two are indistinguishable from \
                 here.",
                held.running
            ),
        ));
    }

    OfferVerdict {
        adopt: true,
        changed: digest != held_digest,
        generation: offer.generation,
        faults,
    }
}

/// One offer verdict, as the corpus holds it.
pub fn offer_verdict_to_json(verdict: &OfferVerdict) -> Json {
    let mut faults = Json::arr();
    for entry in &verdict.faults {
        let mut row = Json::obj();
        row.set("code", Json::Str(entry.code.as_str().to_string()))
            .set("message", Json::Str(entry.message.clone()))
            .set("severity", Json::Str(entry.severity.as_str().to_string()));
        faults.push(row);
    }
    let mut out = Json::obj();
    out.set("adopt", Json::Bool(verdict.adopt))
        .set("changed", Json::Bool(verdict.changed))
        .set("faults", faults)
        .set("generation", Json::Num(verdict.generation as f64));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offered(generation: u64, digest: &str, packages: usize) -> Offer {
        Offer {
            decoded: true,
            decode_error: String::new(),
            generation,
            digest: digest.to_string(),
            packages,
        }
    }

    fn held(generation: u64, digest: &str, running: usize) -> Held {
        Held {
            generation,
            digest: digest.to_string(),
            running,
        }
    }

    #[test]
    fn the_ordinary_poll_adopts_and_changes_nothing() {
        // Most polls. The Hub answers with the document the robot already has, and a caller that
        // could not tell would rewrite the file every few seconds.
        let verdict = assess_offer(&offered(41, "sha256:aa", 2), &held(41, "sha256:aa", 2));
        assert!(verdict.adopt);
        assert!(!verdict.changed);
        assert!(verdict.faults.is_empty());
    }

    #[test]
    fn a_new_generation_is_adopted_and_changes_something() {
        let verdict = assess_offer(&offered(42, "sha256:bb", 2), &held(41, "sha256:aa", 2));
        assert!(verdict.adopt);
        assert!(verdict.changed);
        assert_eq!(verdict.generation, 42);
        assert!(verdict.faults.is_empty());
    }

    #[test]
    fn a_response_that_is_not_a_desired_state_never_reaches_the_disk() {
        let verdict = assess_offer(
            &Offer {
                decoded: false,
                decode_error: "`generation` is absent".to_string(),
                ..Offer::default()
            },
            &held(41, "sha256:aa", 2),
        );
        assert!(!verdict.adopt);
        // And the robot keeps reconciling towards what it has.
        assert_eq!(verdict.generation, 41);
        assert!(verdict.faults[0].message.contains("`generation` is absent"));
    }

    #[test]
    fn a_reused_generation_is_refused_because_adopting_it_would_be_invisible() {
        let verdict = assess_offer(&offered(41, "sha256:bb", 2), &held(41, "sha256:aa", 2));
        assert!(!verdict.adopt);
        assert_eq!(verdict.faults[0].code, OfferFaultCode::GenerationReused);
        assert_eq!(verdict.faults[0].severity, Severity::Error);
        assert!(verdict.faults[0].message.contains("Bump the generation"));
    }

    #[test]
    fn a_generation_that_went_backwards_is_adopted_and_reported() {
        let verdict = assess_offer(&offered(40, "sha256:bb", 2), &held(41, "sha256:aa", 2));
        assert!(verdict.adopt, "a fleet nobody could recover is worse");
        assert_eq!(
            verdict.faults[0].code,
            OfferFaultCode::GenerationWentBackwards
        );
        assert_eq!(verdict.faults[0].severity, Severity::Warning);
    }

    #[test]
    fn emptying_a_running_robot_is_adopted_and_said_out_loud() {
        let verdict = assess_offer(&offered(42, "sha256:bb", 0), &held(41, "sha256:aa", 3));
        assert!(verdict.adopt);
        assert_eq!(verdict.faults[0].code, OfferFaultCode::EmptiesTheRobot);
        assert!(verdict.faults[0].message.contains("stub"));
    }

    #[test]
    fn emptying_a_robot_that_is_running_nothing_is_not_news() {
        let verdict = assess_offer(&offered(42, "sha256:bb", 0), &held(41, "sha256:aa", 0));
        assert!(verdict.adopt);
        assert!(verdict.faults.is_empty());
    }

    #[test]
    fn a_first_ever_pull_has_nothing_to_compare_against() {
        // No held digest, so neither the reuse check nor `changed` has anything to work with. It
        // must not report a reuse against a robot that has never pulled — which is what a naive
        // string comparison against an empty digest would do.
        let verdict = assess_offer(&offered(0, "sha256:aa", 1), &Held::default());
        assert!(verdict.adopt);
        assert!(verdict.changed);
        assert!(verdict.faults.is_empty());
    }

    #[test]
    fn a_refusal_carries_the_generation_the_robot_still_holds() {
        for offer in [
            Offer {
                decoded: false,
                ..Offer::default()
            },
            offered(41, "sha256:bb", 2),
        ] {
            let verdict = assess_offer(&offer, &held(41, "sha256:aa", 2));
            if !verdict.adopt {
                assert_eq!(verdict.generation, 41);
                assert!(!verdict.changed);
            }
        }
    }
}

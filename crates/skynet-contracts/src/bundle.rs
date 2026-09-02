//! §12's fetch — deciding whether to download an OS image, from where, and whether what arrived is
//! the image.
//!
//! §12 gates *installing* an image and says nothing about getting one, because getting one looks
//! like an implementation detail. It is not. Three of the rules in this file exist because a
//! hundred-megabyte transfer over a link somebody pays for, to a machine in a warehouse, is a
//! different problem from a JSON `GET`:
//!
//! 1. **§34.3's metering.** *"Bulk transfer respects metering […] pushing those over a metered link
//!    because it happened to be the only one up is how a fleet generates a surprise invoice."* An
//!    OS image is the largest bulk transfer this system has, so it always waits for an unmetered
//!    bearer, and the wait is reported rather than silent.
//! 2. **A partial download is bound to the digest it is a partial of.** This is §12's own rule about
//!    a bench pass — *"evidence that is not bound to what it is evidence of is not evidence"* —
//!    applied to bytes. A partial keyed only by version would let a resume append one release's tail
//!    to another's head, and the failure would surface as a digest mismatch after the whole transfer
//!    rather than before it.
//! 3. **What arrived is checked against the manifest, and a failure deletes it.** A bundle that does
//!    not hash to what the signed manifest says is not the image; keeping it means the next resume
//!    appends to bad bytes for ever, and it leaves an artifact on disk that some future code path
//!    will find and treat as one.
//!
//! # This decides, and does not transfer
//!
//! No network, no filesystem, no clock. The bearer's facts arrive as values, the size of whatever is
//! on the disk arrives as a number, and the digest of what arrived arrives already computed — so the
//! same function runs on the robot and on a Hub asking whether a robot could have fetched an image.

use alloc::format;
use alloc::string::{String, ToString};

use skynet_json::Json;

/// What to do about one OS image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchDecision {
    /// It is here and it has been checked.
    Have,
    /// Start a transfer.
    Fetch,
    /// Continue one.
    Resume,
    /// What is here is not a partial of this bundle. Throw it away and start again.
    DiscardAndFetch,
    /// §34.3: the only link up charges by the megabyte.
    WaitForUnmetered,
    /// Nothing is up.
    NoBearer,
    /// There is nothing to check the bytes against.
    Refuse,
}

impl FetchDecision {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            FetchDecision::Have => "have",
            FetchDecision::Fetch => "fetch",
            FetchDecision::Resume => "resume",
            FetchDecision::DiscardAndFetch => "discard_and_fetch",
            FetchDecision::WaitForUnmetered => "wait_for_unmetered",
            FetchDecision::NoBearer => "no_bearer",
            FetchDecision::Refuse => "refuse",
        }
    }

    /// Whether this decision moves bytes.
    pub fn transfers(self) -> bool {
        matches!(
            self,
            FetchDecision::Fetch | FetchDecision::Resume | FetchDecision::DiscardAndFetch
        )
    }
}

/// Every decision, in the order the tests run.
pub const FETCH_DECISIONS: [FetchDecision; 7] = [
    FetchDecision::Have,
    FetchDecision::Fetch,
    FetchDecision::Resume,
    FetchDecision::DiscardAndFetch,
    FetchDecision::WaitForUnmetered,
    FetchDecision::NoBearer,
    FetchDecision::Refuse,
];

/// What the robot knows about the link it would download over (§34.3).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bearer {
    /// The interface name, for the sentence.
    pub name: String,
    /// Whether it charges by the megabyte.
    pub metered: bool,
    /// Whether the metering was declared rather than assumed.
    pub metering_declared: bool,
}

/// The image, and what is already on the disk for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FetchSubject {
    /// `sha256:…` from the signed release manifest. What the bytes must hash to.
    pub digest: String,
    /// The size the manifest declares, when it declares one.
    pub size: Option<u64>,
    /// Whether a complete, verified bundle for this digest is already here.
    pub have: bool,
    /// Bytes of a partial download **keyed to this digest**.
    pub partial: u64,
    /// The active bearer, when there is one.
    pub bearer: Option<Bearer>,
}

/// §12's answer for one image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchPlan {
    /// Which of the seven.
    pub decision: FetchDecision,
    /// The byte offset to start from. Zero unless the decision is [`FetchDecision::Resume`].
    pub from: u64,
    /// The sentence an operator reads.
    pub detail: String,
}

/// What to do about one OS image (§12, §34.3).
///
/// The order of the tests is the order of cost. Having it already is free; refusing costs nothing;
/// waiting for a link costs nothing; and only then is a transfer considered. A version that checked
/// metering first would report a robot as waiting for an unmetered bearer to fetch an image it
/// already has.
pub fn plan_fetch(subject: &FetchSubject) -> FetchPlan {
    let digest = subject.digest.trim();

    if subject.have {
        return FetchPlan {
            decision: FetchDecision::Have,
            from: 0,
            detail:
                "This bundle is already on the disk and has been checked against the manifest. \
                     Nothing to fetch."
                    .to_string(),
        };
    }

    if digest.is_empty() {
        return FetchPlan {
            decision: FetchDecision::Refuse,
            from: 0,
            detail: "This release names no bundle digest, so there is nothing to check the bytes \
                     against. §12 binds a bench pass to the exact bytes it was taken against, and \
                     bytes with no name cannot be bound to — so they are not fetched either."
                .to_string(),
        };
    }

    let Some(bearer) = &subject.bearer else {
        return FetchPlan {
            decision: FetchDecision::NoBearer,
            from: 0,
            detail:
                "No bearer is up, so there is nothing to fetch over. That is the ordinary state \
                     for most of a robot's life (§34.1) and nothing about the update is lost by it \
                     — the release manifest is already here and the transfer resumes when a link \
                     returns."
                    .to_string(),
        };
    };

    if bearer.metered {
        return FetchPlan {
            decision: FetchDecision::WaitForUnmetered,
            from: 0,
            detail: format!(
                "An OS image is the largest bulk transfer this system has, and `{}` is {} metered. \
                 It waits for an unmetered link (§34.3). Nothing is lost by waiting, and a fleet \
                 that downloaded hundreds of megabytes per robot over cellular would find out at \
                 the end of the month.",
                bearer.name,
                if bearer.metering_declared {
                    "declared"
                } else {
                    "assumed"
                }
            ),
        };
    }

    // A partial at least as large as the whole thing is not a partial of this bundle. Resuming from
    // it would append this image's tail to something else's, and the mismatch would only surface
    // after the entire transfer — which on a poor link is hours.
    if let Some(size) = subject.size.filter(|size| *size > 0) {
        if subject.partial >= size {
            return FetchPlan {
                decision: FetchDecision::DiscardAndFetch,
                from: 0,
                detail: format!(
                    "There are {} bytes here for a bundle the manifest says is {size}. A partial \
                     cannot be larger than what it is part of, so this is left over from something \
                     else. It is discarded and the transfer starts again.",
                    subject.partial
                ),
            };
        }
    }

    if subject.partial > 0 {
        return FetchPlan {
            decision: FetchDecision::Resume,
            from: subject.partial,
            detail: format!(
                "Resuming at {} bytes{}. The partial is keyed to this bundle's digest, so it cannot \
                 be a different image's tail.",
                subject.partial,
                match subject.size {
                    Some(size) => format!(" of {size}"),
                    None => String::new(),
                }
            ),
        };
    }

    FetchPlan {
        decision: FetchDecision::Fetch,
        from: 0,
        detail: format!(
            "Fetching {} over `{}`.",
            match subject.size {
                Some(size) => format!("{size} bytes"),
                None => "the bundle".to_string(),
            },
            bearer.name
        ),
    }
}

// ── What arrived ─────────────────────────────────────────────────────────────

/// What the bytes on the disk turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleCheckCode {
    /// They are the release the manifest describes.
    Match,
    /// Fewer bytes than declared. A transfer that stopped.
    Short,
    /// More bytes than declared. Not this artifact.
    Long,
    /// The right number of different bytes.
    DigestMismatch,
}

impl BundleCheckCode {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            BundleCheckCode::Match => "match",
            BundleCheckCode::Short => "short",
            BundleCheckCode::Long => "long",
            BundleCheckCode::DigestMismatch => "digest_mismatch",
        }
    }
}

/// Every outcome, in the order the checks run.
pub const BUNDLE_CHECKS: [BundleCheckCode; 4] = [
    BundleCheckCode::Match,
    BundleCheckCode::Short,
    BundleCheckCode::Long,
    BundleCheckCode::DigestMismatch,
];

/// The bytes that are on the disk.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Arrived {
    /// `sha256:…` over them.
    pub digest: String,
    /// How many there are.
    pub size: u64,
}

/// What the signed manifest says they should be.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expected {
    /// `sha256:…`.
    pub digest: String,
    /// The declared size, when there is one.
    pub size: Option<u64>,
}

/// §12's verdict on one downloaded bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleCheck {
    /// Which of the four.
    pub code: BundleCheckCode,
    /// Whether these bytes may be installed.
    pub ok: bool,
    /// Whether the file should be deleted rather than kept as a partial.
    pub discard: bool,
    /// The sentence an operator reads.
    pub detail: String,
}

/// Are these the bytes the signed manifest describes (§12)?
///
/// Size is checked before the digest, and the reason is the sentence rather than the verdict — both
/// refuse. A file short of its declared size is a transfer that stopped, which is ordinary and
/// resumable; a file of exactly the right size whose digest is wrong is a different set of bytes,
/// which is a build somebody re-cut or a transfer somebody interfered with. Reporting the first as a
/// digest mismatch would send an engineer looking for tampering every time a link dropped.
///
/// # A failure discards, except the one that does not
///
/// [`BundleCheckCode::Short`] keeps the file, because that is exactly what a resume is for.
/// Everything else deletes it: bytes that will never hash correctly are bytes a later resume would
/// append to for ever, and an unverified bundle left on the disk is an artifact some future code
/// path will find.
pub fn check_bundle(arrived: &Arrived, expected: &Expected) -> BundleCheck {
    let want = expected.digest.trim();
    let got = arrived.digest.trim();

    if let Some(size) = expected.size {
        if arrived.size < size {
            return BundleCheck {
                code: BundleCheckCode::Short,
                ok: false,
                discard: false,
                detail: format!(
                    "{} of {size} bytes arrived. The transfer stopped early, which is what a link \
                     dropping looks like. The partial is kept and the next attempt resumes from it.",
                    arrived.size
                ),
            };
        }
        if arrived.size > size {
            return BundleCheck {
                code: BundleCheckCode::Long,
                ok: false,
                discard: true,
                detail: format!(
                    "{} bytes arrived for a bundle the manifest says is {size}. More than was \
                     declared is not a transfer that stopped — it is a different artifact, or a \
                     proxy that served something else. Discarded.",
                    arrived.size
                ),
            };
        }
    }

    if want.is_empty() || got.is_empty() || want != got {
        return BundleCheck {
            code: BundleCheckCode::DigestMismatch,
            ok: false,
            discard: true,
            detail: "These bytes do not hash to what the signed release manifest says this bundle \
                     is. §12 binds every gate it applies to that digest, so bytes that are not it \
                     are not this release — do not install them, and find out where they came from. \
                     Discarded, because bytes that will never verify are bytes a resume would append \
                     to for ever."
                .to_string(),
        };
    }

    BundleCheck {
        code: BundleCheckCode::Match,
        ok: true,
        discard: false,
        detail: "These bytes are the release the manifest describes.".to_string(),
    }
}

/// One fetch plan, as the corpus holds it.
pub fn fetch_plan_to_json(plan: &FetchPlan) -> Json {
    let mut out = Json::obj();
    out.set("decision", Json::Str(plan.decision.as_str().to_string()))
        .set("detail", Json::Str(plan.detail.clone()))
        .set("from", Json::Num(plan.from as f64));
    out
}

/// One bundle check, as the corpus holds it.
pub fn bundle_check_to_json(check: &BundleCheck) -> Json {
    let mut out = Json::obj();
    out.set("code", Json::Str(check.code.as_str().to_string()))
        .set("detail", Json::Str(check.detail.clone()))
        .set("discard", Json::Bool(check.discard))
        .set("ok", Json::Bool(check.ok));
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

    fn subject() -> FetchSubject {
        FetchSubject {
            digest: "sha256:aa".to_string(),
            size: Some(1_000),
            have: false,
            partial: 0,
            bearer: Some(ethernet()),
        }
    }

    #[test]
    fn an_image_that_is_here_is_not_fetched_over_anything() {
        // Order-of-cost, and the case that catches it: a robot with the image already should not be
        // reported as waiting for an unmetered link.
        let plan = plan_fetch(&FetchSubject {
            have: true,
            bearer: Some(cellular()),
            ..subject()
        });
        assert_eq!(plan.decision, FetchDecision::Have);
    }

    #[test]
    fn a_bundle_with_no_digest_is_not_fetched() {
        let plan = plan_fetch(&FetchSubject {
            digest: String::new(),
            ..subject()
        });
        assert_eq!(plan.decision, FetchDecision::Refuse);
        assert!(plan.detail.contains("cannot be bound to"));
    }

    #[test]
    fn an_image_waits_for_a_link_nobody_pays_by_the_megabyte_for() {
        let plan = plan_fetch(&FetchSubject {
            bearer: Some(cellular()),
            ..subject()
        });
        assert_eq!(plan.decision, FetchDecision::WaitForUnmetered);
        assert!(plan.detail.contains("assumed metered"));
        assert!(!plan.decision.transfers());
    }

    #[test]
    fn a_partial_resumes_from_where_it_stopped() {
        let plan = plan_fetch(&FetchSubject {
            partial: 400,
            ..subject()
        });
        assert_eq!(plan.decision, FetchDecision::Resume);
        assert_eq!(plan.from, 400);
    }

    #[test]
    fn a_partial_bigger_than_the_bundle_is_not_a_partial_of_it() {
        for partial in [1_000, 1_001] {
            let plan = plan_fetch(&FetchSubject {
                partial,
                ..subject()
            });
            assert_eq!(plan.decision, FetchDecision::DiscardAndFetch, "{partial}");
            assert_eq!(plan.from, 0);
        }
    }

    #[test]
    fn a_manifest_with_no_size_still_resumes() {
        // The size check is what catches a partial from another image; without one there is nothing
        // to compare, and the digest check at the end is what catches it instead. Resuming is still
        // right — refusing to resume without a declared size would make every poor link start over.
        let plan = plan_fetch(&FetchSubject {
            size: None,
            partial: 400,
            ..subject()
        });
        assert_eq!(plan.decision, FetchDecision::Resume);
        assert_eq!(plan.from, 400);
    }

    #[test]
    fn nothing_up_is_not_a_failure() {
        let plan = plan_fetch(&FetchSubject {
            bearer: None,
            ..subject()
        });
        assert_eq!(plan.decision, FetchDecision::NoBearer);
        assert!(plan.detail.contains("ordinary state"));
    }

    #[test]
    fn the_right_bytes_pass() {
        let check = check_bundle(
            &Arrived {
                digest: "sha256:aa".to_string(),
                size: 1_000,
            },
            &Expected {
                digest: "sha256:aa".to_string(),
                size: Some(1_000),
            },
        );
        assert!(check.ok);
        assert!(!check.discard);
    }

    #[test]
    fn a_short_file_is_kept_and_everything_else_is_thrown_away() {
        let short = check_bundle(
            &Arrived {
                digest: "sha256:zz".to_string(),
                size: 400,
            },
            &Expected {
                digest: "sha256:aa".to_string(),
                size: Some(1_000),
            },
        );
        assert_eq!(short.code, BundleCheckCode::Short);
        assert!(!short.discard, "a resume is exactly what a partial is for");

        let long = check_bundle(
            &Arrived {
                digest: "sha256:zz".to_string(),
                size: 1_400,
            },
            &Expected {
                digest: "sha256:aa".to_string(),
                size: Some(1_000),
            },
        );
        assert_eq!(long.code, BundleCheckCode::Long);
        assert!(long.discard);
    }

    #[test]
    fn the_right_size_and_the_wrong_bytes_is_not_a_dropped_link() {
        // The distinction the ordering exists for: reporting a truncated transfer as a digest
        // mismatch would send an engineer looking for tampering every time a warehouse link dropped.
        let check = check_bundle(
            &Arrived {
                digest: "sha256:zz".to_string(),
                size: 1_000,
            },
            &Expected {
                digest: "sha256:aa".to_string(),
                size: Some(1_000),
            },
        );
        assert_eq!(check.code, BundleCheckCode::DigestMismatch);
        assert!(check.discard);
        assert!(check.detail.contains("where they came from"));
    }

    #[test]
    fn an_undeclared_size_still_checks_the_digest() {
        let check = check_bundle(
            &Arrived {
                digest: "sha256:zz".to_string(),
                size: 12,
            },
            &Expected {
                digest: "sha256:aa".to_string(),
                size: None,
            },
        );
        assert_eq!(check.code, BundleCheckCode::DigestMismatch);
    }

    #[test]
    fn a_missing_digest_on_either_side_never_matches() {
        for (want, got) in [("", "sha256:aa"), ("sha256:aa", ""), ("", "")] {
            let check = check_bundle(
                &Arrived {
                    digest: got.to_string(),
                    size: 1,
                },
                &Expected {
                    digest: want.to_string(),
                    size: None,
                },
            );
            assert!(!check.ok, "{want:?} {got:?}");
        }
    }
}

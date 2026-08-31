//! Offline entitlement (§51).
//!
//! §51.1 states one principle and then says the whole design follows from refusing its opposite:
//!
//! > **A licence check must never stop a running robot.**
//!
//! > A billing system that can halt a humanoid mid-task has invented a safety hazard to protect
//! > revenue.
//!
//! Everything below is that sentence made mechanical, and the most important thing about this
//! module is a **type that does not exist**. There is no outcome meaning *stop*, *halt*, *suspend*
//! or *deny at run*. [`Verdict`] answers one question — may this package be **installed or
//! upgraded** — and the running robot is not a party to it. A future change that wanted to stop a
//! robot for a licence would have to add a variant, in a file whose first paragraph says why not.
//!
//! # §51.2's table, and where each row comes from
//!
//! | Situation | Behaviour |
//! |---|---|
//! | Valid entitlement | Install, upgrade and run |
//! | Expired, inside grace | Runs. Upgrades allowed. Reported as lapsed. |
//! | Expired, past grace | **Keeps running.** New installs and upgrades of that package refused. |
//! | Revoked | **Keeps running.** Reinstall refused, incident raised. |
//!
//! Two of the four rows contain the words *keeps running* in bold, in the specification, which is
//! unusual enough to be a deliberate instruction to whoever implemented this.
//!
//! The middle row is the one with a design in it: **expired-inside-grace still permits upgrades.**
//! That looks backwards until you read §51.2's reason for the grace period being ninety days —
//! *"the failure it guards against (a robot in a basement losing a feature because a token expired)
//! is worse than the leakage it permits"* — and then it is obvious. A robot whose token lapsed
//! three weeks ago and which needs a security fix should get the security fix.
//!
//! # No clock, and no crypto
//!
//! The instant is an argument, for §35.2's reason and §45's: a function that reaches for a clock
//! cannot be tested at an expiry boundary, and the Hub answering *"is this fleet lapsed"* from a
//! check-in and the robot answering it offline have to be the same function over the same facts.
//!
//! The signature is somebody else's job too. §51.2 says the token is *signed and time-limited*, and
//! verifying the signature is `skynetd-io`'s trust store — this module takes
//! [`Entitlement::signature`] as a [`SignatureState`](crate::admission::SignatureState) already
//! decided, exactly as [`admission`](crate::admission) does for a package. Splitting it that way is
//! what lets the whole of §51's decision table be exercised without a key anywhere.
//!
//! # Absence is a decision, and it is not this module's
//!
//! A package with **no entitlement at all** is not automatically refused here, because most
//! packages are not paid ones. [`decide`] takes an [`Entitlement`] that the caller has already
//! decided applies; whether a given package requires one is a property of the package and of the
//! marketplace it came from (§39), not of this table. A module that assumed otherwise would refuse
//! every open-source policy on a robot that has never bought anything.

use alloc::format;
use alloc::string::{String, ToString};

use crate::admission::SignatureState;

/// A signed, time-limited grant to install one package (§51.2).
///
/// Delivered with the desired state and cached on disk — §57 is explicit that this is *"a field in
/// the desired-state document, not an endpoint"*, so a robot that has been offline for a week is
/// deciding from the copy it already has, which is the whole point of calling it *offline*
/// entitlement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entitlement {
    /// The package this grant is for. One token, one package.
    pub package: String,
    /// When the grant expires, in milliseconds since the Unix epoch.
    ///
    /// Wall time, unlike almost everything else in this system, and deliberately: §35.1 forbids the
    /// wall clock for *control* decisions because it can step, and this is not a control decision.
    /// An entitlement is a fact about a calendar — somebody's subscription runs to a date — and a
    /// monotonic instant cannot express one.
    pub expires_at_ms: u64,
    /// How long past expiry installs and upgrades still work, in milliseconds.
    ///
    /// §51.2's ninety days by default. Carried on the token rather than compiled in, so a vendor
    /// who wants a longer grace can give one without an OS release, and so the number a robot is
    /// using is visible on the token rather than inferred from a version.
    pub grace_ms: u64,
    /// Whether the issuer has revoked this grant.
    ///
    /// Separate from expiry because they are different facts with different remedies and §51.2
    /// gives them different rows: an expired token is renewed, a revoked one is a dispute. Only the
    /// revoked row raises an incident.
    pub revoked: bool,
    /// What is known about the issuer's signature over this token.
    ///
    /// Decided by the caller's trust store. `Verified` is the only state this module treats as
    /// evidence, for the reason §5 gives about descriptors: the platform will not *call* an
    /// unverified thing verified.
    pub signature: SignatureState,
}

/// What §51.2 permits, for one package, at one instant.
///
/// **Every variant permits the robot to keep running.** That is not a property of any one variant;
/// it is a property of the enumeration, because there is no variant that says otherwise. See the
/// module header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Row one. Install, upgrade and run.
    Current,
    /// Row two. Expired, inside grace: runs, upgrades allowed, reported as lapsed.
    ///
    /// The row that looks backwards and is not: a robot whose token lapsed three weeks ago and
    /// which needs a security fix should get the security fix.
    Lapsed,
    /// Row three. Expired past grace: keeps running, new installs and upgrades refused.
    Expired,
    /// Row four. Revoked: keeps running, reinstall refused, and an incident is raised.
    Revoked,
    /// The token cannot be believed, so it establishes nothing.
    ///
    /// Not in §51.2's table, because that table assumes a token. A token whose signature is absent,
    /// untrusted or invalid has told the robot nothing at all — it is neither evidence of a grant
    /// nor evidence of its absence — and installing on the strength of it would let anyone who can
    /// write a file into the cache grant themselves a licence.
    ///
    /// It refuses installs like the two rows above it, and like them it does not stop anything.
    Unattested,
}

impl Verdict {
    /// Whether a package may be installed or upgraded under this verdict (§51.2).
    pub fn permits_install(self) -> bool {
        matches!(self, Verdict::Current | Verdict::Lapsed)
    }

    /// Whether an incident is raised (§51.2's fourth row, and only that row).
    ///
    /// Expiry is not an incident. A subscription ending on the day it was always going to end is
    /// the system working, and a fleet that raised an incident for it would teach its operators to
    /// ignore incidents.
    pub fn raises_incident(self) -> bool {
        matches!(self, Verdict::Revoked)
    }

    /// The wire spelling, for `/packages` and the console.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Current => "current",
            Verdict::Lapsed => "lapsed",
            Verdict::Expired => "expired",
            Verdict::Revoked => "revoked",
            Verdict::Unattested => "unattested",
        }
    }
}

/// Whether this verdict permits the robot to keep running.
///
/// It is a free function returning a constant, and that is the point: §51.1's principle expressed
/// as something a reader can find, a test can assert, and a future change would have to edit on
/// purpose rather than erode by adding a variant.
///
/// §51.3: *"a revoked entitlement blocks distribution, not operation. The lever against genuine
/// abuse is the Hub: no new installs, no upgrades, no support, and a record. Not a robot that stops
/// moving in somebody's warehouse because a payment failed."*
pub const fn permits_running(_verdict: Verdict) -> bool {
    true
}

/// Decide §51.2's table for one entitlement at one instant.
///
/// The order of the checks is the order of the table, and it is not interchangeable. **Revocation
/// outranks expiry**: a token that was revoked *and* has since expired is a dispute, not a renewal,
/// and reporting it as expired would send somebody to a billing page instead of to whoever revoked
/// it. **Attestation outranks both**, because a token nobody can verify has not told the robot that
/// it was revoked either.
pub fn decide(entitlement: &Entitlement, now_ms: u64) -> Verdict {
    if entitlement.signature != SignatureState::Verified {
        return Verdict::Unattested;
    }
    if entitlement.revoked {
        return Verdict::Revoked;
    }
    if now_ms <= entitlement.expires_at_ms {
        return Verdict::Current;
    }
    // Saturating, because a token with a grace period near `u64::MAX` is a malformed token and not
    // a reason to panic in an install path. It reads as "grace has not run out", which is the
    // direction §51.2 already errs in everywhere else.
    if now_ms
        <= entitlement
            .expires_at_ms
            .saturating_add(entitlement.grace_ms)
    {
        return Verdict::Lapsed;
    }
    Verdict::Expired
}

/// The sentence an operator gets, naming the remedy rather than the state.
///
/// Each of these ends by saying what the robot is still doing, because §51.1's principle is
/// invisible to somebody reading a red badge unless it is written next to it. An operator who sees
/// "expired" on a robot in a basement and is not told it is still running will go and look.
pub fn explain(verdict: Verdict, package: &str) -> String {
    match verdict {
        Verdict::Current => {
            format!("\"{package}\" is entitled. Installs and upgrades are allowed.")
        }
        Verdict::Lapsed => format!(
            "\"{package}\" has a lapsed entitlement and is inside its grace period. It runs, and \
             upgrades are still allowed — a robot that needs a security fix should get one. Renew \
             the entitlement to clear this."
        ),
        Verdict::Expired => format!(
            "\"{package}\" has an entitlement that expired past its grace period. It keeps \
             running; new installs and upgrades of it are refused until the entitlement is renewed."
        ),
        Verdict::Revoked => format!(
            "\"{package}\" has a revoked entitlement. It keeps running; reinstalling or upgrading \
             it is refused, and an incident has been raised. This is a dispute with whoever issued \
             the entitlement, not an expiry."
        ),
        Verdict::Unattested => format!(
            "The entitlement for \"{package}\" is not signed by a key this robot trusts, so it \
             establishes nothing — neither a grant nor the absence of one. It keeps running; \
             installs and upgrades are refused until a verifiable token is supplied."
        ),
    }
}

/// How long is left, or how long is left of the grace, as a sentence.
///
/// Returns `None` where the question has no answer — a revoked or unattested token has no clock to
/// report. Days rather than milliseconds because §51.2's units are days and nobody has ever needed
/// this to the millisecond; the truncation is toward zero, so *"0 days"* means *"today"* rather
/// than rounding a robot into a day it does not have.
pub fn remaining(entitlement: &Entitlement, now_ms: u64, verdict: Verdict) -> Option<String> {
    const DAY_MS: u64 = 24 * 60 * 60 * 1000;
    match verdict {
        Verdict::Current => {
            let left = entitlement.expires_at_ms.saturating_sub(now_ms) / DAY_MS;
            Some(format!("{left} day(s) until it expires"))
        }
        Verdict::Lapsed => {
            let end = entitlement
                .expires_at_ms
                .saturating_add(entitlement.grace_ms);
            let left = end.saturating_sub(now_ms) / DAY_MS;
            Some(format!("{left} day(s) of grace remaining"))
        }
        Verdict::Expired => {
            let end = entitlement
                .expires_at_ms
                .saturating_add(entitlement.grace_ms);
            let over = now_ms.saturating_sub(end) / DAY_MS;
            Some(format!("grace ended {over} day(s) ago"))
        }
        Verdict::Revoked | Verdict::Unattested => None,
    }
}

/// The default grace period: ninety days (§51.2).
///
/// > Grace is deliberately generous — ninety days by default — because the failure it guards
/// > against (a robot in a basement losing a feature because a token expired) is worse than the
/// > leakage it permits.
///
/// A default rather than a constant the code enforces: a token carries its own `grace_ms`, and this
/// is what a token that did not say gets. Naming it here means a robot and a Hub that both fall
/// back agree about what they fell back to.
pub const DEFAULT_GRACE_MS: u64 = 90 * 24 * 60 * 60 * 1000;

impl Entitlement {
    /// A grant, with §51.2's default grace and no signature yet decided.
    ///
    /// [`SignatureState::Absent`] rather than `Verified`, so a caller that forgot to run the token
    /// through its trust store gets [`Verdict::Unattested`] and refuses an install, rather than
    /// getting [`Verdict::Current`] and permitting one. Defaults fail in the safe direction or they
    /// are not defaults, they are traps.
    pub fn new(package: &str, expires_at_ms: u64) -> Entitlement {
        Entitlement {
            package: package.to_string(),
            expires_at_ms,
            grace_ms: DEFAULT_GRACE_MS,
            revoked: false,
            signature: SignatureState::Absent,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use alloc::vec::Vec;

    const DAY: u64 = 24 * 60 * 60 * 1000;
    /// An arbitrary "now", far enough from zero that subtracting a grace period cannot underflow
    /// into a test that passes for the wrong reason.
    const NOW: u64 = 1_000 * DAY;

    fn token(expires_at_ms: u64) -> Entitlement {
        Entitlement {
            signature: SignatureState::Verified,
            ..Entitlement::new("vendor/policy", expires_at_ms)
        }
    }

    // ── §51.1, the principle the whole section follows from ──────────────────

    #[test]
    fn no_verdict_stops_a_running_robot() {
        // §51.1: "a licence check must never stop a running robot." Asserted over every variant
        // rather than trusted to the ones this suite happens to construct, so a variant added later
        // has to be added to this list — which is the moment somebody reads the sentence above.
        let every: Vec<Verdict> = vec![
            Verdict::Current,
            Verdict::Lapsed,
            Verdict::Expired,
            Verdict::Revoked,
            Verdict::Unattested,
        ];
        for verdict in every {
            assert!(
                permits_running(verdict),
                "{} would stop a running robot, which §51.1 forbids",
                verdict.as_str()
            );
        }
    }

    #[test]
    fn every_verdict_says_the_robot_keeps_running() {
        // The principle again, at the level an operator meets it. A red badge with no sentence
        // beside it sends somebody to a basement.
        for verdict in [
            Verdict::Lapsed,
            Verdict::Expired,
            Verdict::Revoked,
            Verdict::Unattested,
        ] {
            let sentence = explain(verdict, "vendor/policy");
            assert!(
                sentence.contains("runs") || sentence.contains("running"),
                "{} does not tell the reader the robot is still running: {sentence}",
                verdict.as_str()
            );
        }
    }

    // ── §51.2's table, row by row ────────────────────────────────────────────

    #[test]
    fn row_one_a_valid_entitlement_installs_upgrades_and_runs() {
        let verdict = decide(&token(NOW + DAY), NOW);
        assert_eq!(verdict, Verdict::Current);
        assert!(verdict.permits_install());
        assert!(!verdict.raises_incident());
    }

    #[test]
    fn row_two_expired_inside_grace_still_permits_upgrades() {
        // The row with a design in it, and the one most likely to be "corrected" by somebody who
        // reads it as a bug. A robot whose token lapsed three weeks ago and which needs a security
        // fix should get the security fix.
        let verdict = decide(&token(NOW - 21 * DAY), NOW);
        assert_eq!(verdict, Verdict::Lapsed);
        assert!(
            verdict.permits_install(),
            "§51.2: upgrades allowed inside grace"
        );
        assert!(!verdict.raises_incident());
    }

    #[test]
    fn row_three_expired_past_grace_refuses_installs_and_nothing_else() {
        let verdict = decide(&token(NOW - 200 * DAY), NOW);
        assert_eq!(verdict, Verdict::Expired);
        assert!(!verdict.permits_install());
        assert!(
            !verdict.raises_incident(),
            "a subscription ending on the day it was always going to end is the system working"
        );
        assert!(permits_running(verdict));
    }

    #[test]
    fn row_four_revoked_refuses_installs_and_raises_an_incident() {
        let revoked = Entitlement {
            revoked: true,
            ..token(NOW + 365 * DAY)
        };
        let verdict = decide(&revoked, NOW);
        assert_eq!(verdict, Verdict::Revoked);
        assert!(!verdict.permits_install());
        assert!(
            verdict.raises_incident(),
            "§51.2: reinstall refused, incident raised"
        );
        assert!(permits_running(verdict));
    }

    // ── The boundaries, which are where a grace period is wrong or right ─────

    #[test]
    fn the_instant_of_expiry_is_still_current() {
        // Inclusive on purpose: a token that says it expires at noon is good at noon. The
        // alternative loses a robot a licence to a millisecond of rounding.
        assert_eq!(decide(&token(NOW), NOW), Verdict::Current);
        assert_eq!(decide(&token(NOW - 1), NOW), Verdict::Lapsed);
    }

    #[test]
    fn the_last_instant_of_grace_is_still_lapsed() {
        let expires = NOW - DEFAULT_GRACE_MS;
        assert_eq!(decide(&token(expires), NOW), Verdict::Lapsed);
        assert_eq!(decide(&token(expires - 1), NOW), Verdict::Expired);
    }

    #[test]
    fn the_default_grace_is_ninety_days() {
        // §51.2's figure, pinned. Shortening it is a commercial decision somebody should have to
        // make deliberately, not a constant that drifts.
        assert_eq!(DEFAULT_GRACE_MS, 90 * DAY);
        assert_eq!(Entitlement::new("p", 0).grace_ms, DEFAULT_GRACE_MS);
    }

    #[test]
    fn a_token_carries_its_own_grace_and_it_is_used() {
        let generous = Entitlement {
            grace_ms: 365 * DAY,
            ..token(NOW - 200 * DAY)
        };
        assert_eq!(decide(&generous, NOW), Verdict::Lapsed);

        let none = Entitlement {
            grace_ms: 0,
            ..token(NOW - 1)
        };
        assert_eq!(decide(&none, NOW), Verdict::Expired);
    }

    #[test]
    fn an_absurd_grace_does_not_overflow_an_install_path() {
        // A malformed token is not a reason to panic while installing a package. It reads as
        // "grace has not run out", which is the direction §51.2 errs in everywhere else.
        let absurd = Entitlement {
            grace_ms: u64::MAX,
            ..token(NOW - DAY)
        };
        assert_eq!(decide(&absurd, NOW), Verdict::Lapsed);
    }

    // ── Precedence, which decides where a person is sent ─────────────────────

    #[test]
    fn revocation_outranks_expiry() {
        // A token revoked and since expired is a dispute, not a renewal. Reporting it as expired
        // would send somebody to a billing page instead of to whoever revoked it.
        let both = Entitlement {
            revoked: true,
            ..token(NOW - 500 * DAY)
        };
        assert_eq!(decide(&both, NOW), Verdict::Revoked);
        assert!(decide(&both, NOW).raises_incident());
    }

    #[test]
    fn an_unverifiable_token_outranks_everything_it_claims() {
        // Including its own revocation. A token nobody can verify has not told the robot that it
        // was revoked either, and an unsigned file in a cache directory must not be able to grant
        // a licence *or* to raise an incident against somebody.
        for state in [
            SignatureState::Absent,
            SignatureState::Failed,
            SignatureState::Untrusted,
            SignatureState::Unverifiable,
        ] {
            let unattested = Entitlement {
                signature: state,
                revoked: true,
                ..token(NOW + 365 * DAY)
            };
            let verdict = decide(&unattested, NOW);
            assert_eq!(verdict, Verdict::Unattested, "{state:?}");
            assert!(!verdict.permits_install());
            assert!(!verdict.raises_incident());
            assert!(permits_running(verdict));
        }
    }

    #[test]
    fn a_new_entitlement_is_unattested_until_a_trust_store_says_otherwise() {
        // Defaults fail in the safe direction or they are traps. A caller who forgot to run the
        // token through its trust store refuses an install; it does not permit one.
        let fresh = Entitlement::new("vendor/policy", NOW + 365 * DAY);
        assert_eq!(fresh.signature, SignatureState::Absent);
        assert_eq!(decide(&fresh, NOW), Verdict::Unattested);
    }

    // ── What an operator is told ─────────────────────────────────────────────

    #[test]
    fn the_sentence_names_the_package_and_the_remedy() {
        let lapsed = explain(Verdict::Lapsed, "acme/grasp");
        assert!(lapsed.contains("acme/grasp"));
        assert!(lapsed.contains("Renew"));

        let revoked = explain(Verdict::Revoked, "acme/grasp");
        assert!(revoked.contains("dispute"), "a revocation is not an expiry");
        assert!(
            !revoked.contains("Renew"),
            "renewing does not clear a revocation"
        );
    }

    #[test]
    fn remaining_is_days_and_has_no_answer_where_there_is_no_clock() {
        let current = token(NOW + 10 * DAY);
        assert_eq!(
            remaining(&current, NOW, Verdict::Current).as_deref(),
            Some("10 day(s) until it expires")
        );

        let lapsed = token(NOW - 10 * DAY);
        assert_eq!(
            remaining(&lapsed, NOW, Verdict::Lapsed).as_deref(),
            Some("80 day(s) of grace remaining")
        );

        let expired = token(NOW - 100 * DAY);
        assert_eq!(
            remaining(&expired, NOW, Verdict::Expired).as_deref(),
            Some("grace ended 10 day(s) ago")
        );

        // A revoked or unattested token has no clock to report, and inventing one would put a
        // countdown next to a state that is not about time.
        assert_eq!(remaining(&current, NOW, Verdict::Revoked), None);
        assert_eq!(remaining(&current, NOW, Verdict::Unattested), None);
    }

    #[test]
    fn the_final_day_reads_as_zero_rather_than_rounding_up() {
        // Truncation toward zero: "0 day(s)" means today. Rounding up would tell a robot it has a
        // day it does not have.
        let ending = token(NOW + DAY - 1);
        assert_eq!(
            remaining(&ending, NOW, Verdict::Current).as_deref(),
            Some("0 day(s) until it expires")
        );
    }

    #[test]
    fn the_wire_spellings_are_distinct() {
        let mut seen: Vec<&str> = vec![
            Verdict::Current.as_str(),
            Verdict::Lapsed.as_str(),
            Verdict::Expired.as_str(),
            Verdict::Revoked.as_str(),
            Verdict::Unattested.as_str(),
        ];
        seen.sort_unstable();
        let before = seen.len();
        seen.dedup();
        assert_eq!(seen.len(), before, "two verdicts share a wire spelling");
    }
}

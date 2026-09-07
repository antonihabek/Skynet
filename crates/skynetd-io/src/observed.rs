//! Which of §27's eight conditions this daemon has actually looked at.
//!
//! # Why this is a module and not three sentences
//!
//! §27's machine derives an operating state from eight conditions, and `skynetd` observes some of
//! them. Which some has changed three times in a fortnight — `post` alone, then `kernel`, then
//! `rhal` — and every time it changed, the number went stale somewhere. Three rounds running the
//! build log recorded the same correction, which is the point at which a repeated mistake stops
//! being carelessness and starts being a missing mechanism. `tools/scale-report.mjs` exists for
//! exactly that reason about a different set of figures, and its header makes the argument:
//! *"nobody updates a summary while they are building the thing it summarises."*
//!
//! The specific failure is worth naming, because it is not "a comment drifted". Before this module
//! there were three rendered strings, each with a hand-typed number, and they **disagreed with each
//! other on the same pass**:
//!
//! | Where | Said | True |
//! |---|---|---|
//! | `GET /state`'s `partial` | three of eight | three |
//! | `GET /status`'s `safe_state_note` | *"the self-test and **eight** unobserved conditions"* | five unobserved, and the eight was never right — it counted the total as the remainder |
//! | the start-up banner | three of eight | three |
//!
//! So an operator reading the console header was told the machine had observed nothing, while the
//! same daemon's own state route said it had observed three. §32.2's rule for the console is that
//! it *must never show a value it did not receive*; a value the daemon contradicts one route later
//! is worse than one it never sent.
//!
//! # What is derived and what is not
//!
//! The **names** are §27's and are fixed. The **observability** is per-pass and is not: `kernel`
//! and `rhal` are observed only when the recording could have held a control cycle, because
//! silence from a ring with no action channels says nothing about a kernel. So this is a reading
//! taken from the same two sightings the machine itself was derived from, never a constant — a
//! constant would be a fourth number to keep in step.
//!
//! `post` is the one condition this process owns outright (§48.5). `kernel`, `rhal`, `stop` and
//! `guard_hold` are read off §13's ring, which is a file this process already holds rather than a
//! channel to the kernel.
//!
//! `guard_hold` is the one that had to be argued for rather than mapped, because the reason
//! written here for leaving it out — *"the kernel's"* — was true about the **subject** and false
//! about the **evidence**. Every one of these four is the kernel's; what decides whether this
//! daemon can observe one is whether the kernel wrote it down, and §26's cycle record carries the
//! guard verdict beside the flags. See [`HoldSighting`] for what a failed read costs it, and for
//! why an advisory hold is not one.
//!
//! `guards` is the fifth and the only one **not** off the ring. Its reason here was *"a cycle
//! records what guards did, not which were available"* — true about the ring, and the wrong place
//! to have been looking: §7.6's partition is resolved on this daemon and already served at
//! `GET /limits`. See [`GuardsSighting`], and in particular for the second source the condition's
//! own documentation named and nothing read.
//!
//! `policy` is the sixth and the only one that is observable on some robots and not on others.
//! Two of `PolicyState`'s three values — *nothing to run* and *something that is not ready* — are
//! exactly knowable from the store and the unit this daemon launched; the third asserts §25.2's
//! lease, which is the kernel's table. So a robot whose plan resolves and whose unit is running
//! reports this one unobserved, and every other robot reports it observed. See [`PolicySighting`].
//!
//! `homing` is the last, and it closed differently from the other seven. Each of those was found
//! already written down somewhere this process holds. This one was not written down anywhere,
//! because §24 had no field for it — and the conclusion drawn from that for months, *"it needs a
//! vendor to recompile rather than a daemon to look somewhere else"*, did not follow. §24's `flags`
//! is a `u16` with four flags and twelve spare bits, so a two-bit homing field could be added to it
//! without moving an offset, breaking a driver or bumping the ABI version. Once the field existed
//! the reading was the same reading as the other four: off §13's ring, out of a record this process
//! already holds. See [`crate::blackbox::HomingSighting`], and in particular for why a driver that
//! declines to answer is **observed** rather than unobserved.

use skynet_json::Json;

use crate::blackbox::{HoldSighting, HomingSighting, KernelSighting, RhalSighting, StopSighting};
use crate::limits::GuardsSighting;
use crate::shape::PolicySighting;

/// §27's eight conditions, in the order [`skynet_contracts::opstate::evaluate`] consults them.
///
/// The wire spellings, not prose: these are the keys of `GET /state`'s `observed` object and a
/// console reads them. Renaming one is a wire change.
pub const CONDITIONS: [&str; 8] = [
    "post",
    "kernel",
    "rhal",
    "homing",
    "stop",
    "guard_hold",
    "guards",
    "policy",
];

/// What each observed condition is called in a sentence.
///
/// Deliberately not the wire spelling. `post` is *the self-test* to a person and `rhal` is *the
/// RHAL*, and a note that read "the post condition was observed" would be the wire leaking into
/// the one place §64 asks for English.
const PROSE: [&str; 8] = [
    "the self-test",
    "the kernel",
    "the RHAL",
    "the homing state",
    "the stop",
    "the guard hold",
    "the guard set",
    "the policy's lease",
];

/// English for the counts this can produce. Nine of them, and there will never be a tenth.
///
/// Spelled out rather than rendered as a digit because every sentence these appear in is prose an
/// operator reads. "3 of §27's 8 conditions" is a log line; this is a note on a console.
const WORDS: [&str; 9] = [
    "none", "one", "two", "three", "four", "five", "six", "seven", "eight",
];

/// The seven readings one reconcile pass took of §27's conditions.
///
/// A struct because there are six of them and they were positional, which clippy noticed before a
/// person did: `refresh_operating_state(wall_s, &kernel, &rhal, &stop, &hold, &guards, &policy)` is
/// six arguments of six different types that all read as "a sighting", and the day two of them
/// share a type is the day a transposition compiles. Named fields make that a compiler error.
///
/// They belong together for a second reason that is not ergonomics: **§27's conditions are only
/// comparable when they are readings of one instant** (`blackbox::Recorder::chain` says so where
/// the ring is scanned). A struct assembled once per pass and handed on is the shape that says so;
/// six parameters invite a caller to gather them at six different moments.
#[derive(Debug, Clone, Copy)]
pub struct Sightings<'a> {
    /// §27's `kernel`, off §13's ring.
    pub kernel: &'a KernelSighting,
    /// §27's `rhal`, off the newest cycle in it.
    pub rhal: &'a RhalSighting,
    /// §27's `stop`, off §24's flags on that cycle.
    pub stop: &'a StopSighting,
    /// §27's `guard_hold`, off the guard verdict on that cycle.
    pub hold: &'a HoldSighting,
    /// §27's `guards`, off §7.6's partition and §45.5's expiry — not the ring.
    pub guards: &'a GuardsSighting,
    /// §27's `policy`, off the store and the unit this daemon launched.
    pub policy: &'a PolicySighting,
    /// §27's `homing`, off §24's two-bit field on the same newest cycle as `stop`.
    pub homing: &'a HomingSighting,
}

/// Which of §27's conditions this daemon looked at on the pass being reported.
///
/// Constructed from the sightings rather than holding its own copy of them: two records of one
/// fact is the shape this module exists to remove, not to reproduce one layer up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    seen: [bool; 8],
}

impl Observed {
    /// Read the seven conditions that are evidence-dependent, and fix the one that is not.
    ///
    /// `post` is always observed: [`crate::post::run`] executes on this process on every pass, and
    /// a robot whose self-test could not run reports a *failed* self-test rather than an absent
    /// one (§48.4).
    ///
    /// Nothing is hard-coded `false` any more. That line existed for `homing` and carried the
    /// reason *"driver-side, and §24 gives the ABI no flag for it"* — the first half true, the
    /// second half a conclusion that stopped being true the moment the flags word was read
    /// properly. Every one of §27's eight conditions is now a reading, which is what this module's
    /// header hoped for when it said the day one gains a producer is the day this file changes.
    pub fn of(seen: &Sightings<'_>) -> Observed {
        let Sightings {
            kernel,
            rhal,
            stop,
            hold,
            guards,
            policy,
            homing,
        } = seen;
        Observed {
            seen: [
                true,              // post — §48.5, this process's own
                kernel.observed(), // §27, off §13's ring
                rhal.observed(),   // §27, off the newest cycle in it
                homing.observed(), // §27, off §24's two-bit field on that same cycle
                stop.observed(),   // §27, off the same cycle's §24 flags
                hold.observed(),   // §27, off the same cycle's guard verdict
                guards.observed(), // §27, off §7.6's partition and §45.5's expiry — not the ring
                policy.observed(), // §27, off the store and the unit — except where §25.2's lease
                                   // is what the answer turns on
            ],
        }
    }

    /// How many were observed. Between one and seven today, and never fewer than one.
    pub fn count(&self) -> usize {
        self.seen.iter().filter(|seen| **seen).count()
    }

    /// How many were not. The complement, so the two can never be stated inconsistently.
    pub fn unobserved(&self) -> usize {
        CONDITIONS.len() - self.count()
    }

    /// `GET /state`'s `observed` object: every condition, named, with what was looked at.
    ///
    /// Every one of the eight, including the false ones. A console that received only the observed
    /// conditions could not tell an unobserved condition from one this daemon had never heard of,
    /// and §48.5 is the section that insists the difference be visible.
    pub fn to_json(&self) -> Json {
        let mut out = Json::obj();
        for (name, seen) in CONDITIONS.iter().zip(self.seen.iter()) {
            out.set(name, Json::Bool(*seen));
        }
        out
    }

    /// The observed conditions in English, comma-joined with a final "and".
    ///
    /// Empty is unreachable — `post` is always observed — but it is handled rather than asserted,
    /// because a panic while rendering a status note would take out the one route that works on a
    /// robot where everything else has failed (§32.2).
    pub fn in_prose(&self) -> String {
        let named: Vec<&str> = PROSE
            .iter()
            .zip(self.seen.iter())
            .filter(|(_, seen)| **seen)
            .map(|(name, _)| *name)
            .collect();
        join_with_and(&named)
    }

    /// The unobserved conditions in English, the same way.
    pub fn missing_in_prose(&self) -> String {
        let named: Vec<&str> = PROSE
            .iter()
            .zip(self.seen.iter())
            .filter(|(_, seen)| !**seen)
            .map(|(name, _)| *name)
            .collect();
        join_with_and(&named)
    }

    /// "three of §27's eight conditions" — the clause every one of these sentences shares.
    pub fn tally(&self) -> String {
        format!(
            "{} of §27's {} conditions",
            WORDS[self.count()],
            WORDS[CONDITIONS.len()]
        )
    }

    /// The count as a word, for a sentence that has already said what it is counting.
    pub fn unobserved_word(&self) -> &'static str {
        WORDS[self.unobserved()]
    }

    /// `GET /state`'s phrasing of the same complement: what is left for another process to read.
    ///
    /// A third renderer rather than a third `format!`, for the reason the other two are here. Each
    /// of these sentences is a different shape of English about one array, and the failure this
    /// module exists to stop is two of them disagreeing — which cannot happen while none of them
    /// owns a number.
    pub fn missing_sentence(&self) -> String {
        if self.unobserved() == 0 {
            return "nothing is left for another process to read".to_string();
        }
        format!(
            "the other {} — {} — are still not this process's to read",
            self.unobserved_word(),
            self.missing_in_prose(),
        )
    }

    /// The trailing clause every note about this ends on: how many were not observed, and which.
    ///
    /// A method rather than a `format!` at each call site, because the two halves have to agree
    /// grammatically as well as numerically. Writing it twice is how the first version ended up
    /// with a count in one sentence and a list in another that did not match it — and the empty
    /// case is here for the same reason: a robot that observed all eight would otherwise be
    /// described as deciding with "the other none — — taken at the least capable value".
    pub fn unobserved_clause(&self) -> String {
        if self.unobserved() == 0 {
            return "with nothing left unobserved".to_string();
        }
        format!(
            "with the other {} — {} — taken at the least capable value they could have",
            self.unobserved_word(),
            self.missing_in_prose(),
        )
    }
}

/// `a`, `a and b`, `a, b and c` — an Oxford-free list, because these are read aloud off a console.
fn join_with_and(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_string(),
        [first, second] => format!("{first} and {second}"),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skynet_contracts::opstate::RhalState;

    #[allow(clippy::type_complexity)]
    fn sightings(
        observed: bool,
    ) -> (
        KernelSighting,
        RhalSighting,
        StopSighting,
        HoldSighting,
        GuardsSighting,
        PolicySighting,
        HomingSighting,
    ) {
        if observed {
            (
                KernelSighting::Turning { cycles: 12 },
                RhalSighting::Seen {
                    state: RhalState::Present,
                    errno: None,
                    fault_code: 0,
                },
                StopSighting::Clear,
                HoldSighting::Clear,
                GuardsSighting::AllAvailable,
                PolicySighting::Nothing { no_body: false },
                // A driver that answers. `Unreported` would also count as observed — that is the
                // judgement `HomingSighting::observed` makes — so the fixture picks the state a
                // robot with an updated driver actually reports, and the `Unreported` case is
                // asserted on its own below.
                HomingSighting::Reported(skynet_contracts::opstate::HomingState::Complete),
            )
        } else {
            (
                KernelSighting::Unobservable("no ring"),
                RhalSighting::Unobservable("no ring"),
                StopSighting::Unobservable("no ring"),
                HoldSighting::Unobservable("no ring"),
                GuardsSighting::Unobservable("no descriptor"),
                PolicySighting::LeaseUnknown {
                    package: "com.example.walk".to_string(),
                },
                HomingSighting::Unobservable("no ring"),
            )
        }
    }

    /// The three sentences, and the property that they cannot disagree.
    ///
    /// This is the whole point of the module, so it is asserted rather than left to reading: the
    /// number in the tally, the number of names in the prose, and the complement all come from one
    /// array, and a change to that array moves all three together.
    #[test]
    fn the_tally_and_the_names_are_the_same_reading() {
        for observed in [true, false] {
            let (kernel, rhal, stop, hold, guards, policy, homing) = sightings(observed);
            let seen = Observed::of(&Sightings {
                kernel: &kernel,
                rhal: &rhal,
                stop: &stop,
                hold: &hold,
                guards: &guards,
                policy: &policy,
                homing: &homing,
            });

            let named = seen.in_prose().split(", ").count();
            // "a and b" is one split and two names, so count the conjunction too.
            let named = named + seen.in_prose().matches(" and ").count();
            assert_eq!(named, seen.count(), "the prose names what the tally counts");

            assert_eq!(
                seen.count() + seen.unobserved(),
                CONDITIONS.len(),
                "every condition is either observed or not"
            );
            assert!(
                seen.tally().starts_with(WORDS[seen.count()]),
                "the tally opens with the count it is a tally of"
            );
        }
    }

    /// `post` is observed on a robot with no ring at all, and it is the only one.
    ///
    /// The regression this guards is the sentence that was live for a fortnight: *"the self-test
    /// and eight unobserved conditions"*, which counted the total as the remainder and therefore
    /// described a daemon that had looked at nine things.
    #[test]
    fn the_self_test_is_observed_even_when_nothing_else_is() {
        let (kernel, rhal, stop, hold, guards, policy, homing) = sightings(false);
        let seen = Observed::of(&Sightings {
            kernel: &kernel,
            rhal: &rhal,
            stop: &stop,
            hold: &hold,
            guards: &guards,
            policy: &policy,
            homing: &homing,
        });
        assert_eq!(seen.count(), 1);
        assert_eq!(seen.unobserved(), 7);
        assert_eq!(seen.in_prose(), "the self-test");
        assert_eq!(seen.tally(), "one of §27's eight conditions");
    }

    /// And with a turning loop, a described body, nothing installed to command it and a driver
    /// that answers §24's homing field, it is **all eight** — which is what every surface should
    /// then say.
    ///
    /// Eight is the end of this sequence rather than a step in it: there is no ninth condition, so
    /// `unobserved_word()` is `none` here and the complement is zero for the first time. The
    /// arithmetic below is unchanged, which is the property the module was built for.
    #[test]
    fn a_turning_loop_and_an_answering_driver_make_it_eight() {
        let (kernel, rhal, stop, hold, guards, policy, homing) = sightings(true);
        let seen = Observed::of(&Sightings {
            kernel: &kernel,
            rhal: &rhal,
            stop: &stop,
            hold: &hold,
            guards: &guards,
            policy: &policy,
            homing: &homing,
        });
        assert_eq!(seen.count(), 8);
        assert_eq!(seen.unobserved(), 0);
        assert_eq!(
            seen.in_prose(),
            "the self-test, the kernel, the RHAL, the homing state, the stop, the guard hold, the \
             guard set and the policy's lease"
        );
        assert_eq!(seen.tally(), "eight of §27's eight conditions");
        assert_eq!(seen.unobserved_word(), "none");
    }

    /// A hold that was demanded and not enforced is still an **observation**.
    ///
    /// The distinction §7.4 forces and the one this module is most likely to lose: `Advisory` says
    /// the daemon looked and a guard fired, so it counts here exactly as `Held` does. What it must
    /// not do is set §27's condition — that is [`HoldSighting::held`]'s job and it is asserted
    /// where the mapping is, not here.
    #[test]
    fn an_advisory_hold_counts_as_looked_at() {
        let (kernel, rhal, stop, _, guards, policy, homing) = sightings(true);
        let seen = Observed::of(&Sightings {
            kernel: &kernel,
            rhal: &rhal,
            stop: &stop,
            hold: &HoldSighting::Advisory,
            guards: &guards,
            policy: &policy,
            homing: &homing,
        });
        assert_eq!(seen.count(), 8);
        assert!(seen.to_json().get("guard_hold").is_some());
    }

    /// The object carries all eight, not only the true ones (§48.5).
    #[test]
    fn every_condition_is_reported_including_the_unobserved_ones() {
        let (kernel, rhal, stop, hold, guards, policy, homing) = sightings(true);
        let document = Observed::of(&Sightings {
            kernel: &kernel,
            rhal: &rhal,
            stop: &stop,
            hold: &hold,
            guards: &guards,
            policy: &policy,
            homing: &homing,
        })
        .to_json();
        for condition in CONDITIONS {
            assert!(
                document.get(condition).is_some(),
                "{condition} is missing from the observed object"
            );
        }
    }
}

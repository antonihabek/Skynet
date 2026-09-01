//! Exclusive capability leases — §25.2, "two packages, one arm".
//!
//! > Capability grants say a package *may* use the arm; they do not say what happens when two
//! > packages hold that grant simultaneously.
//!
//! **Capabilities are leased exclusively.** A running package holds a lease on each capability token
//! it commands, `skynetd` is the arbiter, and a second package requesting a held token is refused
//! rather than interleaved. Two policies alternating commands to the same joint produce a trajectory
//! neither was trained on and neither can be held responsible for.
//!
//! Three rules, and the third is the one that makes the design work at all:
//!
//! 1. **A grant is not a lease.** Two packages may hold the same grant; one holds the lease. The
//!    grant is a permission decided at admission; the lease is an arbitration decided at run time.
//! 2. **Sensing is shared freely.** Two policies reading the same camera is normal; two policies
//!    driving the same elbow is not. So a sensing token is never exclusive, and the distinction is
//!    made by the token's own prefix rather than by a table somebody has to maintain.
//! 3. **Exactly one pre-emption exists.** A package registered as a **safety behaviour** — a
//!    fall-arrest reflex, a retreat — may take a lease another package holds. It is a distinct
//!    package kind, it is granted separately, and every pre-emption is a black box event.
//!
//! There is deliberately no priority *number*. A numeric priority invites a second package to be
//! given a slightly higher one, and then a third, and the property that exactly one thing can
//! pre-empt is gone with nobody having decided to give it up.

use skynet_contracts::skill::{LeaseState, Progress};

/// Why a lease request was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseDenial {
    /// The requester does not hold a grant for this token.
    ///
    /// Checked before the holder, so a package with no permission is told it has no permission
    /// rather than that somebody else is using the arm — which would be a true statement that sends
    /// them looking in the wrong place.
    NotGranted,
    /// Another package holds it, and the requester is not a safety behaviour.
    Held {
        /// The package holding it.
        holder: String,
    },
}

/// What happened to a lease request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreemptionOutcome {
    /// Taken, and nothing was displaced.
    Acquired,
    /// Taken from a holder, which is a black box event (§25.2).
    Preempted {
        /// Who lost it.
        displaced: String,
    },
    /// Already held by the requester. Idempotent, so a reconciler re-running is harmless.
    AlreadyHeld,
    /// Refused.
    Denied(LeaseDenial),
}

/// One lease.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    /// The capability token.
    pub token: String,
    /// The package holding it.
    pub holder: String,
}

/// The arbiter's table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LeaseTable {
    leases: Vec<Lease>,
}

/// Whether a token is one that may be held by two packages at once.
///
/// Sensing is shared; actuation is not. The prefix decides, because the alternative is a list of
/// exclusive tokens that somebody has to remember to add to — and a token missing from that list is
/// two policies driving one elbow, which is the failure this whole module exists to prevent. Erring
/// this way means an unrecognised token is treated as exclusive, which costs a refusal rather than
/// a collision.
pub fn is_shared(token: &str) -> bool {
    token.starts_with("sensing.")
}

impl LeaseTable {
    /// An empty table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Who holds a token, if anyone.
    pub fn holder(&self, token: &str) -> Option<&str> {
        self.leases
            .iter()
            .find(|lease| lease.token == token)
            .map(|lease| lease.holder.as_str())
    }

    /// Every lease, for reporting.
    pub fn leases(&self) -> &[Lease] {
        &self.leases
    }

    /// Ask for a token.
    ///
    /// `granted` is whether the requester holds a grant for it — the permission decided at
    /// admission — and `safety_behaviour` is whether this package is the one kind that may
    /// pre-empt.
    pub fn acquire(
        &mut self,
        requester: &str,
        token: &str,
        granted: bool,
        safety_behaviour: bool,
    ) -> PreemptionOutcome {
        if !granted {
            return PreemptionOutcome::Denied(LeaseDenial::NotGranted);
        }

        if is_shared(token) {
            // Nothing to arbitrate. Recorded anyway so a report can say who is reading what, which
            // is the difference between a console that shows the machine and one that shows the
            // parts of it somebody thought to model.
            if !self
                .leases
                .iter()
                .any(|lease| lease.token == token && lease.holder == requester)
            {
                self.leases.push(Lease {
                    token: token.to_string(),
                    holder: requester.to_string(),
                });
                return PreemptionOutcome::Acquired;
            }
            return PreemptionOutcome::AlreadyHeld;
        }

        match self.holder(token) {
            None => {
                self.leases.push(Lease {
                    token: token.to_string(),
                    holder: requester.to_string(),
                });
                PreemptionOutcome::Acquired
            }
            Some(holder) if holder == requester => PreemptionOutcome::AlreadyHeld,
            Some(holder) => {
                if !safety_behaviour {
                    return PreemptionOutcome::Denied(LeaseDenial::Held {
                        holder: holder.to_string(),
                    });
                }
                let displaced = holder.to_string();
                for lease in &mut self.leases {
                    if lease.token == token {
                        lease.holder = requester.to_string();
                    }
                }
                PreemptionOutcome::Preempted { displaced }
            }
        }
    }

    /// Give a token back.
    ///
    /// Idempotent: releasing a lease nobody holds is not an error, because the reconciler emits a
    /// release for any lease whose holder is no longer running and will emit it again on the next
    /// pass if the observation has not caught up.
    pub fn release(&mut self, holder: &str, token: &str) {
        self.leases
            .retain(|lease| !(lease.token == token && lease.holder == holder));
    }

    /// Drop everything a package holds — what happens when it stops, crashes or is stopped.
    ///
    /// A package that died holding the arm must not keep holding it. This is the half of §25.2 that
    /// only matters when something goes wrong, which is the half worth writing carefully: a lease
    /// leaked by a crashed policy refuses the arm to its replacement, and the symptom is a robot
    /// that will not start work with no reason on any screen.
    pub fn release_all(&mut self, holder: &str) -> Vec<String> {
        let released: Vec<String> = self
            .leases
            .iter()
            .filter(|lease| lease.holder == holder)
            .map(|lease| lease.token.clone())
            .collect();
        self.leases.retain(|lease| lease.holder != holder);
        released
    }
}
/// What acquiring a whole skill's `required_capabilities` produced.
///
/// §54.4's `acquiring` stage takes leases for a **list** of tokens, and a list is where the
/// single-token rule above needs one more property that §25.2 does not have to state because it is
/// only visible from here: **acquisition is all or nothing.**
///
/// A skill that got two of its three tokens holds two joints it cannot use and refuses them to
/// everything else, and the symptom is a second package that will not start with no reason on any
/// screen — the same symptom as the leaked lease [`LeaseTable::release_all`] exists to prevent,
/// arriving through the front door instead. So a denial rolls back everything this call took.
///
/// It rolls back only what **this call** took. A requester that already held a token before asking
/// keeps it, because it is not this acquisition's to give away.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acquisition {
    /// The one fact §54.4's lifecycle acts on.
    pub state: LeaseState,
    /// The token that was refused, when one was.
    pub token: Option<String>,
    /// Who holds it, when somebody does. `None` for a token the requester was never granted, which
    /// is a different errand: nobody has the arm, this package may not ask for it.
    pub holder: Option<String>,
    /// Everything taken, in request order — empty after a rollback.
    pub taken: Vec<String>,
    /// Anything displaced, for §25.2's black box event.
    pub displaced: Vec<(String, String)>,
}

impl LeaseTable {
    /// Take every token a skill declared, or none of them.
    ///
    /// `granted` answers "does this package hold a grant for this token", which is admission's
    /// decision (§8) rather than this table's. It is a closure rather than a list so that the
    /// caller's own grant lookup stays the single source of that answer.
    pub fn acquire_all<F: Fn(&str) -> bool>(
        &mut self,
        requester: &str,
        tokens: &[String],
        granted: F,
        safety_behaviour: bool,
    ) -> Acquisition {
        let mut taken: Vec<String> = Vec::new();
        let mut displaced: Vec<(String, String)> = Vec::new();

        for token in tokens {
            match self.acquire(requester, token, granted(token), safety_behaviour) {
                PreemptionOutcome::Acquired => taken.push(token.clone()),
                PreemptionOutcome::Preempted { displaced: holder } => {
                    taken.push(token.clone());
                    displaced.push((token.clone(), holder));
                }
                // Already ours, from an earlier pass of a level-triggered loop. Not taken *here*,
                // so a rollback below must not give it away.
                PreemptionOutcome::AlreadyHeld => {}
                PreemptionOutcome::Denied(denial) => {
                    for rollback in &taken {
                        self.release(requester, rollback);
                    }
                    return Acquisition {
                        state: LeaseState::Denied,
                        token: Some(token.clone()),
                        holder: match denial {
                            LeaseDenial::Held { holder } => Some(holder),
                            LeaseDenial::NotGranted => None,
                        },
                        taken: Vec::new(),
                        displaced: Vec::new(),
                    };
                }
            }
        }

        Acquisition {
            state: LeaseState::Held,
            token: None,
            holder: None,
            taken,
            displaced,
        }
    }
}

impl Acquisition {
    /// Fill in §54.4's `acquiring` evidence.
    ///
    /// This is the projection the contract's own documentation promised lived at a call site rather
    /// than as a second enumeration: [`LeaseState`] answers the one part the lifecycle acts on, and
    /// [`PreemptionOutcome`] keeps the rest. Writing it here rather than in `skynet-contracts` is
    /// what keeps the richer answer out of a `no_std` crate that has no use for it.
    pub fn into_progress(self, progress: &mut Progress) {
        progress.leases = self.state;
        progress.lease_token = self.token.unwrap_or_default();
        progress.lease_holder = self.holder.unwrap_or_default();
    }
}

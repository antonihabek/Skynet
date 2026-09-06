//! The loop, and the only part of this crate that writes anything.
//!
//! Ask the backend, check what it asked for against the lease, publish or do not publish. Once per
//! control period, for as long as the lease is held.
//!
//! # The four rules that shape it
//!
//! **The sequence advances once per published command, and never otherwise.** §24 makes a repeated
//! sequence the definition of a stalled producer, so the number is not a counter of cycles or of
//! ticks — it is a counter of *commands that reached the ring*. Advancing it on a cycle that
//! published nothing would tell the watchdog a producer is alive when it is not, which is the one
//! lie this file could tell that would matter.
//!
//! **The stamp is taken before the backend runs.** §24: *"when the command was produced, not when
//! it was sent"*. Stamping after inference folds the backend's own latency into the number every
//! downstream budget is measured against, and it does it in the direction that flatters the robot.
//!
//! **A refused intent is never quietly narrowed.** A backend that commands more channels than its
//! lease allows is a fault: nothing is published, the sequence does not advance, and the turn says
//! which rule was broken. Clamping it to fit would be this process forming an opinion about what is
//! safe, and §7's guards are where that opinion lives.
//!
//! **A late command is published and counted, not dropped.** The kernel does not block on this
//! producer and it does not need this producer's help to notice staleness: every command carries
//! the instant it was produced, and the staleness guard is on the other side of the ring with the
//! authority to act on it. What this loop owes is an honest stamp and a count of the periods it
//! overran, which is the part the kernel cannot see.

use skynet_ring::{Command, Producer, RingError};

use crate::backend::{Backend, Body, Commanding, Intent};

/// A monotonic clock, so a test can drive the loop without waiting.
///
/// §35.1: control reads one clock and it is monotonic. The same trait, for the same reason, as
/// `skynet_kernel_host::run::Clock` — and deliberately not imported from there, because this crate
/// takes no dependency on the process on the other side of the ring. Two processes that meet at a
/// shared mapping should not link each other's code; that is the property that lets a vendor
/// replace either one.
pub trait Clock {
    /// Nanoseconds on a monotonic base. The origin does not matter; only differences do.
    fn now_ns(&mut self) -> u64;
}

/// `CLOCK_MONOTONIC_RAW` where the platform has it, and the closest available elsewhere.
#[derive(Debug)]
pub struct SystemClock {
    origin: std::time::Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        SystemClock {
            origin: std::time::Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now_ns(&mut self) -> u64 {
        self.origin.elapsed().as_nanos() as u64
    }
}

/// A clock a test advances by hand.
#[derive(Debug, Default)]
pub struct StepClock {
    /// The current reading.
    pub now_ns: u64,
    /// How far [`Clock::now_ns`] advances it on each call.
    pub step_ns: u64,
}

impl Clock for StepClock {
    fn now_ns(&mut self) -> u64 {
        let now = self.now_ns;
        self.now_ns += self.step_ns;
        now
    }
}

/// What one turn of the loop did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// A command reached the ring, carrying this sequence number.
    Published(u64),
    /// The backend asked for nothing and said so. A command with a zero mask was published.
    Held(u64),
    /// The backend published nothing. The sequence did not advance, and the kernel's watchdog will
    /// observe a producer that has stalled.
    Silent,
    /// The intent broke the lease and was refused. Nothing was published.
    Refused(Refusal),
    /// The ring itself refused the write.
    ///
    /// Distinct from [`Tick::Refused`], which is this runtime declining to publish. This one is the
    /// transport failing, and it is the only variant here that is not about a policy's behaviour.
    RingError(RingError),
}

/// Why an intent was not published.
///
/// Each one names a rule rather than a value, because a refusal that quoted the command would put a
/// policy's setpoints in a log line, and §19's argument about not repeating a pairing code back
/// applies to anything a sandbox produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The lease itself is not one this ring can carry — too many channels, or a mode outside §24's
    /// three. Caught when the host is built rather than on a cycle.
    LeaseInvalid,
    /// The backend commanded a channel beyond the number the lease granted.
    ChannelsExceeded,
}

// There is deliberately no `ModeChanged` here, and an earlier draft of this enum had one.
//
// The mode is a property of the **lease**, and [`Intent::Command`] carries values and a mask and
// nothing else — so a backend cannot ask for a different one, and a refusal for asking would be a
// variant no input could reach. Making the state unrepresentable is better than checking for it,
// and shipping a check that cannot fire is the thing this repository keeps finding in its own
// tools. The rule it enforces is unchanged: a policy commands in the mode §6's transport plan
// resolved for it, because that is the mode the ceilings the kernel clamps to were computed for.

/// What the observation ring said this turn, before it becomes a [`Body`].
///
/// An internal step rather than a public type. It exists because the stale case must be decided
/// **before** a `Body` is built — §25.1 makes it a hold the backend never sees — and because
/// building the `Body` borrows the host's observation buffer, which cannot happen until every
/// decision that needs `&mut self` has been taken.
#[derive(Debug, Clone, Copy)]
enum Feed {
    Detached,
    Empty,
    Torn,
    TooOld,
    Fresh { age_ms: f64, budgeted: bool },
}

/// What a run of the loop did in total.
///
/// Every field is a count of something that happened, and there is deliberately no ratio here: a
/// rate computed over a window this struct cannot see is §15's `field_conformance`, it belongs to
/// the party that knows the window, and computing one here would be the second definition of it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    /// Turns that published a command with at least one masked-in channel.
    pub published: u64,
    /// Turns that published a hold.
    pub held: u64,
    /// Turns that published nothing because the backend went silent.
    pub silent: u64,
    /// Turns refused for breaking the lease.
    pub refused: u64,
    /// Turns the ring refused.
    pub ring_errors: u64,
    /// Turns whose backend took longer than the control period.
    ///
    /// Counted, never acted on. See the fourth rule in this module's documentation.
    pub overruns: u64,
    /// Turns held because the body reading was older than the lease's budget (§25.1).
    ///
    /// Worth a number of its own rather than folding into `held`: a policy holding because it
    /// chose to and a policy holding because its observation feed fell behind are the same on the
    /// wire and completely different to whoever is asked why the robot stopped moving.
    pub stale_holds: u64,
    /// Turns where a reading was passed to a backend with no budget to check it against.
    ///
    /// §25.1's undeclared-budget case. Not an error and not zero-cost: it is the count of cycles on
    /// which nothing in this system could say whether the body the policy acted on was current.
    pub unbudgeted: u64,
}

/// The producer side of §25's command ring.
///
/// Holds the lease, the backend, the clock and the sequence — and no ring, because a ring is
/// borrowed for the length of a publish rather than owned for the length of a process. That is what
/// lets the suite drive this against an array and `main` drive it against a `memfd` mapping with no
/// difference in the code being tested.
#[derive(Debug)]
pub struct PolicyHost<B: Backend, C: Clock> {
    backend: B,
    clock: C,
    lease: Commanding,
    period_ns: u64,
    tick: u64,
    sequence: u64,
    summary: Summary,
    /// One buffer, reused every turn, because §24's rule about allocators applies here too. It is
    /// also what makes [`skynet_obs::Consumer::read`]'s contract useful: a `Stale` or an `Empty`
    /// leaves it untouched, so what it holds is the last reading that was genuinely delivered.
    observation: skynet_obs::Observation,
}

impl<B: Backend, C: Clock> PolicyHost<B, C> {
    /// Build a host for one lease.
    ///
    /// `period_ns` is the control period the loop intends to hold, and it is used for exactly one
    /// thing: deciding whether a turn overran. It is not a sleep and this struct does not pace
    /// itself — pacing belongs to whatever drives [`Self::turn`], which in `main` is a timer and in
    /// the suite is a `for` loop.
    pub fn new(backend: B, clock: C, lease: Commanding, period_ns: u64) -> PolicyHost<B, C> {
        PolicyHost {
            backend,
            clock,
            lease,
            period_ns,
            tick: 0,
            // Starts at zero and the first published command carries **one**. Zero is the sequence
            // an unwritten slot already holds, so a first command carrying it would be
            // indistinguishable from a ring nobody has published to — and `Reading::Empty` exists
            // precisely to be told apart from a real first command.
            sequence: 0,
            summary: Summary::default(),
            observation: skynet_obs::Observation::default(),
        }
    }

    /// The lease this host is holding.
    pub fn lease(&self) -> Commanding {
        self.lease
    }

    /// How many commands have reached the ring.
    pub fn sequence(&self) -> u64 {
        self.sequence
    }

    /// What every turn so far did.
    pub fn summary(&self) -> Summary {
        self.summary
    }

    /// The backend, for a caller that needs to ask it something.
    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// Run one turn: read the body, ask the backend, and publish what the lease allows.
    ///
    /// `observations` is the mirror ring. `None` is an **open-loop deployment** — the backend is
    /// handed [`Body::Detached`] and told so, which is not the same as a closed-loop policy whose
    /// feed has died, and keeping those apart is why [`Body`] has four variants rather than being
    /// an `Option`.
    pub fn turn(
        &mut self,
        producer: &mut Producer<'_>,
        observations: Option<&mut skynet_obs::Consumer<'_>>,
    ) -> Tick {
        if !self.lease.valid() {
            self.tick += 1;
            self.summary.refused += 1;
            return Tick::Refused(Refusal::LeaseInvalid);
        }

        // Before the backend runs, for the reason in this module's second rule.
        let produced_ns = self.clock.now_ns();
        let tick = self.tick;
        self.tick += 1;

        // ── the body ─────────────────────────────────────────────────────────
        //
        // Read before the backend is asked and after the stamp is taken, so the age below is the
        // age at the instant the command is stamped with — not at the instant it is published,
        // which would be a smaller number and a flattering one.
        let feed = match observations {
            None => Feed::Detached,
            Some(consumer) => match consumer.read(&mut self.observation) {
                skynet_obs::Reading::Empty => Feed::Empty,
                skynet_obs::Reading::Stale => Feed::Torn,
                skynet_obs::Reading::Fresh => {
                    // Saturating: a reading stamped in the future is a clock that has stepped
                    // (§35.4) or a producer on a different base, and either way the honest age is
                    // zero rather than an enormous number from an unsigned wrap.
                    let age_ms =
                        produced_ns.saturating_sub(self.observation.stamp_ns) as f64 / 1_000_000.0;
                    match self.lease.observation_budget_ms {
                        Some(budget) if age_ms > budget => Feed::TooOld,
                        Some(_) => Feed::Fresh {
                            age_ms,
                            budgeted: true,
                        },
                        None => Feed::Fresh {
                            age_ms,
                            budgeted: false,
                        },
                    }
                }
            },
        };

        // §25.1: *"Exceeding it is a `hold`, not a silently older frame."* Enforced here rather
        // than in each backend, because a backend that forgot it would be a policy acting on a
        // body that has moved since — and the backend is never asked, so it cannot decide
        // otherwise.
        if matches!(feed, Feed::TooOld) {
            self.summary.stale_holds += 1;
            return self.publish(producer, produced_ns, Intent::Hold);
        }
        if let Feed::Fresh {
            budgeted: false, ..
        } = feed
        {
            self.summary.unbudgeted += 1;
        }

        let body = match feed {
            Feed::Detached => Body::Detached,
            Feed::Empty => Body::Empty,
            Feed::Torn => Body::Torn,
            Feed::TooOld => unreachable!("returned above"),
            Feed::Fresh { age_ms, budgeted } => Body::Fresh {
                observation: &self.observation,
                age_ms,
                budgeted,
            },
        };

        let intent = self.backend.decide(tick, produced_ns, body);

        let finished_ns = self.clock.now_ns();
        if finished_ns.saturating_sub(produced_ns) > self.period_ns {
            self.summary.overruns += 1;
        }

        self.publish(producer, produced_ns, intent)
    }

    /// Turn one intent into a slot on the ring, or into nothing.
    ///
    /// Split out because §25.1's stale hold reaches it without a backend having been asked, and two
    /// copies of the sequence arithmetic is one copy too many for the number the watchdog reads.
    fn publish(&mut self, producer: &mut Producer<'_>, produced_ns: u64, intent: Intent) -> Tick {
        let (values, mask, held) = match intent {
            Intent::Silent => {
                self.summary.silent += 1;
                return Tick::Silent;
            }
            Intent::Hold => (
                [0.0; skynet_ring::MAX_CHANNELS],
                [0u8; skynet_ring::MAX_CHANNELS],
                true,
            ),
            Intent::Command { values, mask } => {
                // Beyond the lease is a fault rather than a clamp — the third rule. Checked over
                // the **mask**, not the values: a non-zero value on a masked-out channel commands
                // nothing, and refusing it would refuse a backend that keeps a full-width scratch
                // buffer, which is exactly what §24 asks a producer to do.
                if mask
                    .iter()
                    .skip(self.lease.channels)
                    .any(|applied| *applied != 0)
                {
                    self.summary.refused += 1;
                    return Tick::Refused(Refusal::ChannelsExceeded);
                }
                (values, mask, false)
            }
        };

        let command = Command {
            stamp_ns: produced_ns,
            seq: self.sequence + 1,
            mode: self.lease.mode,
            channels: self.lease.channels,
            values,
            mask,
        };

        match producer.publish(&command) {
            Ok(()) => {
                self.sequence += 1;
                if held {
                    self.summary.held += 1;
                    Tick::Held(self.sequence)
                } else {
                    self.summary.published += 1;
                    Tick::Published(self.sequence)
                }
            }
            Err(error) => {
                // The sequence does not advance on a failed publish. It counts commands that
                // reached the ring, and this one did not — so the next attempt reuses the number,
                // and the watchdog reads a producer that has not moved, which is true.
                self.summary.ring_errors += 1;
                Tick::RingError(error)
            }
        }
    }
}

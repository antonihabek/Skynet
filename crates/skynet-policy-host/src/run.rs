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

use crate::backend::{Backend, Body, Commanding, Intent, Sight};

/// A monotonic clock, so a test can drive the loop without waiting.
///
/// §35.1: control reads one clock and it is monotonic. The same trait, for the same reason, as
/// `skynet_kernel_host::run::Clock` — and deliberately not imported from there, because this crate
/// takes no dependency on the process on the other side of the ring. Two processes that meet at a
/// shared mapping should not link each other's code; that is the property that lets a vendor
/// replace either one.
///
/// # Two readings, because there are two questions
///
/// A command's stamp and a reading's age are on **the kernel's** time, because the kernel ages the
/// one and stamped the other — and on a simulated body that time is the simulation's (§35.3), which
/// this process can only read off the ring. Whether this process's own work fitted its period is a
/// question about **this process**, and it is answered on this process's clock: a clock that moves
/// only when the kernel publishes measures every turn as zero or a whole period, and counted the
/// stamps' jitter as overruns the first time the live gate ran it (`DRY-COVERAGE-PLAN.md` F70).
/// `skynet-kernel-host` makes the same split, for the same reason — its overruns are timed on the
/// process's clock and its guards on the body's.
pub trait Clock {
    /// The kernel's time, in nanoseconds — what a command is stamped with and a reading aged by.
    /// Differences are taken across processes, so the origin matters: [`SystemClock`] is
    /// `CLOCK_MONOTONIC_RAW` from boot, [`SimulatedClock`] the simulation's own time, and a test
    /// clock may start anywhere only because the test owns both ends.
    fn now_ns(&mut self) -> u64;

    /// This process's own time, in nanoseconds — what its own work is timed on, and nothing else.
    /// Only differences of two readings are meaningful.
    fn process_ns(&mut self) -> u64;
}

/// `CLOCK_MONOTONIC_RAW`, absolute — §35.1's clock, read through [`skynet_rt::monotonic_raw_ns`].
///
/// Absolute because every stamp this loop writes is aged in another process: the kernel host's
/// watchdog subtracts a command's stamp from its own reading, and this loop subtracts a driver's
/// acquisition stamp from this one. It held an `Instant` from this process's start until
/// `DRY-COVERAGE-PLAN.md` F65 — `CLOCK_MONOTONIC`, from an origin no other process shares, so
/// every age it computed across the ring was the difference between two start times.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ns(&mut self) -> u64 {
        skynet_rt::monotonic_raw_ns()
    }

    fn process_ns(&mut self) -> u64 {
        skynet_rt::monotonic_raw_ns()
    }
}

/// §35.3's clock, as a process on the far side of the ring can read it: the newest stamp the kernel
/// published.
///
/// On a simulated body *"sim time simply is the kernel's time"*, and the kernel's time on a cycle
/// is the stamp of the state it read — which it publishes into the observation ring every cycle,
/// including the ones whose read failed. So this process has no clock of its own to consult and
/// does not pretend to: it reads the one the kernel is on. A command stamped here is then aged by
/// §7's watchdog in the simulation's units, and a body reading aged here against a budget is aged
/// in them too — the two subtractions that were each a difference between two clocks until
/// `DRY-COVERAGE-PLAN.md` F70.
///
/// What it cannot see is time passing between two kernel cycles, and on a simulator none does: the
/// simulation advances when the loop steps it, and not otherwise.
#[derive(Debug, Clone, Copy)]
pub struct SimulatedClock<'a> {
    ring: skynet_obs::Ring<'a>,
    /// The last instant read, which is the answer while the newest slot is being written or before
    /// anything has been published — a simulation that has not started is at time zero.
    last_ns: u64,
}

impl<'a> SimulatedClock<'a> {
    /// The kernel's time on `ring`, which must be a [`skynet_obs::TimeBase::Simulated`] ring —
    /// [`KernelClock::for_body`] is the caller that decides that, from the ring's own header.
    pub fn over(ring: skynet_obs::Ring<'a>) -> SimulatedClock<'a> {
        SimulatedClock { ring, last_ns: 0 }
    }
}

impl Clock for SimulatedClock<'_> {
    fn now_ns(&mut self) -> u64 {
        if let Some(stamp) = self.ring.newest_stamp() {
            self.last_ns = stamp;
        }
        self.last_ns
    }

    /// §35.1's clock. The simulation's time says nothing about how long this process took.
    fn process_ns(&mut self) -> u64 {
        skynet_rt::monotonic_raw_ns()
    }
}

/// The kernel's time, on whichever clock the body it is driving keeps (§35.3).
///
/// An enum rather than a choice of type parameter, because the choice is made at run time from a
/// header this process received — and it is made **there**, not on a command line: the process
/// holding the driver read the body's descriptor and wrote the answer into the ring (F70).
#[derive(Debug, Clone, Copy)]
pub enum KernelClock<'a> {
    /// A real body: §35.1's clock, which this process reads for itself.
    Monotonic(SystemClock),
    /// A simulated one: the newest stamp the kernel published.
    Simulated(SimulatedClock<'a>),
}

impl<'a> KernelClock<'a> {
    /// The clock a policy fed by `body` must keep.
    ///
    /// `None` is a run with no observation ring, which is a run whose command ring is private to
    /// this process — both cross together or neither does — so nothing will ever age what it
    /// stamps, and §35.1's clock is the honest one to stamp with.
    pub fn for_body(body: Option<skynet_obs::Ring<'a>>) -> KernelClock<'a> {
        match body {
            Some(ring) if ring.time_base() == skynet_obs::TimeBase::Simulated => {
                KernelClock::Simulated(SimulatedClock::over(ring))
            }
            _ => KernelClock::Monotonic(SystemClock),
        }
    }

    /// Which clock this is, as the observation ring names it.
    pub fn time_base(&self) -> skynet_obs::TimeBase {
        match self {
            KernelClock::Monotonic(_) => skynet_obs::TimeBase::Monotonic,
            KernelClock::Simulated(_) => skynet_obs::TimeBase::Simulated,
        }
    }
}

impl Clock for KernelClock<'_> {
    fn now_ns(&mut self) -> u64 {
        match self {
            KernelClock::Monotonic(clock) => clock.now_ns(),
            KernelClock::Simulated(clock) => clock.now_ns(),
        }
    }

    fn process_ns(&mut self) -> u64 {
        match self {
            KernelClock::Monotonic(clock) => clock.process_ns(),
            KernelClock::Simulated(clock) => clock.process_ns(),
        }
    }
}

/// A clock a test advances by hand.
///
/// One counter for both readings, which is a test's simplification and not a claim that the two
/// are one clock: [`Clock::now_ns`] steps it and [`Clock::process_ns`] only reads it, so a turn
/// reads the process's time, then the kernel's — stepping — and then the process's again, and the
/// difference is exactly one step.
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

    fn process_ns(&mut self) -> u64 {
        self.now_ns
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

/// What the frame pool said this turn, before it becomes a [`Sight`].
///
/// For [`Feed`]'s reason: the stale case is decided before the backend is asked, and a `Sight`
/// borrows the pixel buffer, which cannot happen until every decision needing `&mut self` is made.
#[derive(Debug, Clone, Copy)]
enum Seen {
    Blind,
    NoFrameYet,
    Missed,
    Frame {
        sequence: u64,
        stamp_ns: u64,
        age_ms: f64,
    },
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
    /// Turns that published a **motion request** rather than a joint command (§88.1).
    ///
    /// Its own counter rather than a share of `published`, because the two are different things
    /// reaching different halves of the process that holds the driver: one is admitted by §7.1's
    /// eight guards and one by §88.2's ten refusals, and a body is commanded by one or the other.
    pub motions: u64,
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
    /// Turns a backend was shown a camera frame.
    pub frames: u64,
    /// Turns the newest frame moved before it could be taken (§47.6's stale handle).
    pub frames_missed: u64,
    /// Turns held because the frame was older than the lease's frame budget (§25.1).
    pub stale_frames: u64,
    /// Turns a frame was shown with no frame budget to check its age against — `unbudgeted`'s
    /// counterpart for the camera.
    ///
    /// Without it, `stale_frames` reads **0** on a lease that declares no budget, which is a count
    /// of a check that never ran printed as a measurement (`DRY-COVERAGE-PLAN.md` F50). The case
    /// that found it is §89.2's: a suspended stream leaves its last frame in the pool, and a policy
    /// with no budget is shown that frame on every turn after — seconds old, and counted as fine.
    pub frames_unbudgeted: u64,
    /// The oldest a body reading was, in nanoseconds on the kernel's clock, at the instant a command
    /// was stamped — over every turn that read one. `None` when no turn did.
    ///
    /// The number §25.1's budget is compared against, kept rather than thrown away after the
    /// comparison, because without a declared budget the comparison never runs and this is then the
    /// only evidence of what the budget would have seen. It is also how `DRY-COVERAGE-PLAN.md` F70
    /// shows up on a run: aged against the wrong clock, a simulator's readings are as old as the
    /// machine has been up.
    pub oldest_body_ns: Option<u64>,
    /// The same, for the frame each turn was shown. `None` when no frame was.
    pub oldest_frame_ns: Option<u64>,
}

/// The larger of a kept maximum and a new reading, where `None` is *nothing measured yet* rather
/// than zero — a zero would be a claim that a reading was exactly current.
fn widest(kept: Option<u64>, reading: u64) -> Option<u64> {
    Some(kept.map_or(reading, |kept| kept.max(reading)))
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
    /// The newest frame's pixels, copied out of the pool and reused every turn for the same reason.
    pixels: Vec<u8>,
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
            pixels: Vec::new(),
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
        self.turn_seeing(producer, observations, None)
    }

    /// Run one turn with a camera: as [`Self::turn`], and the newest frame in `frames` is shown to
    /// the backend as a [`Sight`].
    ///
    /// # The loan is released before the backend is asked
    ///
    /// The frame is taken the way §47.6 says a reader takes one — name the newest, publish a loan,
    /// validate it, copy — and the loan is given back as soon as the copy is done. The backend then
    /// works on this process's copy for as long as it likes, and the writer is free to reuse the
    /// slot the moment the copy ends rather than the moment inference does. A policy that wants to
    /// hold a frame across cycles can keep its own copy; what it cannot do is pin the pool.
    ///
    /// # What copying costs
    ///
    /// One slot of bytes per turn, into a buffer that is kept and reused. `skynet-frames` also
    /// builds a `String` for the stream name on every `latest` and `acquire` — §47.6's contract names
    /// a stream by string — so a turn with a camera allocates where a turn without one does not.
    /// It is stated here rather than assumed away; the overrun count is what says whether it
    /// matters.
    pub fn turn_seeing(
        &mut self,
        producer: &mut Producer<'_>,
        observations: Option<&mut skynet_obs::Consumer<'_>>,
        frames: Option<&skynet_frames::Reader>,
    ) -> Tick {
        if !self.lease.valid() {
            self.tick += 1;
            self.summary.refused += 1;
            return Tick::Refused(Refusal::LeaseInvalid);
        }

        // Before the backend runs, for the reason in this module's second rule — and the process's
        // own reading beside it, which is what the overrun below is timed on (see [`Clock`]).
        let started_ns = self.clock.process_ns();
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
                    let age_ns = produced_ns.saturating_sub(self.observation.stamp_ns);
                    self.summary.oldest_body_ns = widest(self.summary.oldest_body_ns, age_ns);
                    let age_ms = age_ns as f64 / 1_000_000.0;
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

        // ── the camera ───────────────────────────────────────────────────────
        //
        // After the body, for the same reason the body is read after the stamp: the age a backend
        // is told is the age at the instant its command will carry.
        let seen = match frames {
            None => Seen::Blind,
            Some(pool) => self.look(pool, produced_ns),
        };
        match seen {
            Seen::Frame { age_ms, .. } => match self.lease.frame_budget_ms {
                Some(budget) if age_ms > budget => {
                    self.summary.stale_frames += 1;
                    return self.publish(producer, produced_ns, Intent::Hold);
                }
                Some(_) => self.summary.frames += 1,
                None => {
                    self.summary.frames += 1;
                    self.summary.frames_unbudgeted += 1;
                }
            },
            Seen::Missed => self.summary.frames_missed += 1,
            Seen::NoFrameYet | Seen::Blind => {}
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

        let sight = match seen {
            Seen::Frame {
                sequence,
                stamp_ns,
                age_ms,
            } => Sight::Frame {
                pixels: &self.pixels,
                sequence,
                stamp_ns,
                age_ms,
            },
            Seen::NoFrameYet => Sight::NoFrameYet,
            Seen::Missed => Sight::Missed,
            Seen::Blind => Sight::Blind,
        };

        let intent = self.backend.decide(tick, produced_ns, body, sight);

        let finished_ns = self.clock.process_ns();
        if finished_ns.saturating_sub(started_ns) > self.period_ns {
            self.summary.overruns += 1;
        }

        self.publish(producer, produced_ns, intent)
    }

    /// Take the newest frame out of the pool, into [`Self::pixels`], and give the loan back.
    ///
    /// A refusal at either step is [`Seen::Missed`]: the handle named a slot the writer has since
    /// reused, and §47.6 refuses it by name rather than showing another frame.
    fn look(&mut self, pool: &skynet_frames::Reader, produced_ns: u64) -> Seen {
        let Some(handle) = pool.latest() else {
            return Seen::NoFrameYet;
        };
        let Ok(identity) = pool.acquire(&handle) else {
            return Seen::Missed;
        };
        let copied = pool.read(&handle, &mut self.pixels);
        pool.release(&handle);
        if copied.is_none() {
            self.pixels.clear();
            return Seen::Missed;
        }
        // Saturating, for the body's reason: a frame stamped in the future is a clock that has
        // stepped, and the honest age is zero rather than an unsigned wrap.
        let age_ns = produced_ns.saturating_sub(identity.stamp_ns);
        self.summary.oldest_frame_ns = widest(self.summary.oldest_frame_ns, age_ns);
        Seen::Frame {
            sequence: identity.sequence,
            stamp_ns: identity.stamp_ns,
            age_ms: age_ns as f64 / 1_000_000.0,
        }
    }

    /// Turn one intent into a slot on the ring, or into nothing.
    ///
    /// Split out because §25.1's stale hold reaches it without a backend having been asked, and two
    /// copies of the sequence arithmetic is one copy too many for the number the watchdog reads.
    /// Publish one motion request (§88.1).
    ///
    /// It advances the same sequence a joint command does, because the sequence counts what reached
    /// the ring and a motion reached it. What it deliberately does **not** do is count toward
    /// `published`: that field is *"turns that published a command with at least one masked-in
    /// channel"*, and a motion masks in none. `motions` is its own counter for the same reason
    /// §88.3 keeps its own record — a body driven by motions and a body driven by joints are not
    /// two settings of one number.
    fn publish_motion(
        &mut self,
        producer: &mut Producer<'_>,
        produced_ns: u64,
        request: &skynet_ring::Motion,
        values: &[f32],
    ) -> Tick {
        match producer.publish_motion(produced_ns, self.sequence + 1, request, values) {
            Ok(()) => {
                self.sequence += 1;
                self.summary.motions += 1;
                Tick::Published(self.sequence)
            }
            Err(error) => {
                self.summary.ring_errors += 1;
                Tick::RingError(error)
            }
        }
    }

    fn publish(&mut self, producer: &mut Producer<'_>, produced_ns: u64, intent: Intent) -> Tick {
        // §88.1's other kind of frame, and it leaves this function before the joint machinery
        // below it: a motion has no mask, no mode and no channels, so every rule the rest of this
        // function applies is about a fact it does not have.
        if let Intent::Motion {
            request,
            values,
            parameters,
        } = intent
        {
            return self.publish_motion(producer, produced_ns, &request, &values[..parameters]);
        }

        let (values, mask, held) = match intent {
            Intent::Motion { .. } => unreachable!("returned above"),
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

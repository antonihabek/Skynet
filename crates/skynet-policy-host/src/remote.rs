//! §90.2's `remote` backend: a policy whose forward pass runs on another machine.
//!
//! ```text
//!  skynetp (this process)                        session bridge (another process)       GPU
//!  ─────────────────────                         ────────────────────────────────       ───
//!  Body + Sight ──▶ Outbound{seq, age} ──link──▶ Hub session, ticket, TLS ──────────▶ replica
//!                                                                                      │
//!  Intent ◀── plan (chunk, capture_ns) ◀──link── Chunk{obs_seq, hz, actions} ◀────────┘
//! ```
//!
//! # What is decided here, and why here
//!
//! §90.1: *no safety decision may require a network round trip.* Everything on the left of the link
//! is a safety decision and stays in this process, on the kernel's clock:
//!
//! * **The deadline.** Every action's age is measured from the acquisition stamp of the observation
//!   it answers (§24), which this process recorded when it sent that observation. The age is never
//!   taken from anything the link reports: the link says which observation a chunk answers, by
//!   sequence number, and this process looks up when that observation was captured. An action older
//!   than `max_chunk_age_ms` is **never** proposed.
//! * **The quiet policy.** An empty plan, a stale one, an exhausted one, a session that ended, a link
//!   that broke: each is [`Intent::Silent`]. The sequence does not advance, §7.1's watchdog sees a
//!   stalled producer, and the robot holds in a controlled way — the same path any policy that goes
//!   quiet takes. Nothing here interpolates, extrapolates or replays an action to bridge a gap.
//! * **Resume.** After a session is lost, a new session delivering a fresh chunk makes the policy able
//!   to propose again, and whether motion resumes on its own is the operator profile's setting
//!   ([`ResumeRule`]). With no setting it does not: an operator acknowledges, **locally**
//!   ([`Remote::acknowledge`]). The link can deliver chunks; it can never resume a robot.
//! * **The declaration.** A remote policy with no declared deadline is refused at construction with
//!   `deadline_undeclared`, §87.4's refusal and §87.4's code. No maximum is invented here: the figure
//!   is the package's, bounded by the operator profile when it sets one (§5.2).
//!
//! # What is not decided here
//!
//! The network. This edition has no TLS stack of its own (§34.2 goes through `curl`), and a session
//! is a Hub request, a single-use ticket and a WebSocket over TLS. All of that is the **session
//! bridge**'s: a separate process that dials out (§87.4: nothing on the robot becomes routable) and
//! talks to this one over [`Link`]. The bridge is on the proposer's side of §90.1's line — it can
//! delay a chunk, lose one or invent one, and every one of those ends in the same place: an action
//! that is late is not proposed, a chunk that answers no observation this process sent is dropped,
//! and a chunk of the wrong shape is refused. [`crate::bridge`] is the one implementation of the
//! link this crate ships.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use skynet_ring::MAX_CHANNELS;

use crate::backend::{Backend, Body, Intent, Sight};

/// Observations remembered while their answers are awaited. A chunk for one older than this many
/// sends is dropped as unknown; at one observation in flight the bound is never approached.
const PENDING: usize = 16;

/// Whether motion resumes on its own after a lost session is replaced (§90.2, the operator profile).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeRule {
    /// The default: after a loss, nothing is proposed until an operator acknowledges, locally.
    Acknowledge,
    /// The operator profile allows a new session's first fresh chunk to resume motion.
    Automatic,
}

/// What a remote policy package declares, and what the operator profile bounds it by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RemoteDeclaration {
    /// §90.2's `max_chunk_age_ms`: the oldest an action may be, from the capture of the observation
    /// it answers to the cycle that would execute it. `None` is refused.
    pub max_chunk_age_ms: Option<f64>,
    /// The operator profile's ceiling on that figure, when it sets one.
    pub profile_max_chunk_age_ms: Option<f64>,
    /// How resumption after a lost session is decided.
    pub resume: ResumeRule,
}

/// Why a remote policy was refused before it ran.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RemoteRefusal {
    /// No deadline declared: §87.4's `deadline_undeclared`.
    DeadlineUndeclared,
    /// A deadline that is not a positive, finite number of milliseconds.
    DeadlineInvalid,
    /// A deadline the operator profile does not allow.
    DeadlineExceedsProfile {
        /// What the package declared.
        declared_ms: f64,
        /// What the profile allows.
        allowed_ms: f64,
    },
}

impl RemoteRefusal {
    /// The stable code, for logs and for the operator.
    pub fn code(&self) -> &'static str {
        match self {
            RemoteRefusal::DeadlineUndeclared => "deadline_undeclared",
            RemoteRefusal::DeadlineInvalid => "deadline_invalid",
            RemoteRefusal::DeadlineExceedsProfile { .. } => "deadline_exceeds_profile",
        }
    }

    /// What happened and what to do, in English.
    pub fn message(&self) -> String {
        match self {
            RemoteRefusal::DeadlineUndeclared => {
                "a remote policy must declare max_chunk_age_ms, the oldest an action may be \
                 when it is executed (§90.2). A session with no deadline is refused at open, as a \
                 relay session is (§87.4); declare one in the package"
                    .to_string()
            }
            RemoteRefusal::DeadlineInvalid => {
                "max_chunk_age_ms must be a positive, finite number of milliseconds (§90.2)"
                    .to_string()
            }
            RemoteRefusal::DeadlineExceedsProfile {
                declared_ms,
                allowed_ms,
            } => format!(
                "the package declares max_chunk_age_ms {declared_ms} and the operator profile \
                 allows at most {allowed_ms} (§90.2, §5.2). Lower the package's figure or raise \
                 the profile's, deliberately"
            ),
        }
    }
}

impl RemoteDeclaration {
    /// The deadline in nanoseconds, or why there is none this process will run under.
    pub fn deadline_ns(&self) -> Result<u64, RemoteRefusal> {
        let declared = self
            .max_chunk_age_ms
            .ok_or(RemoteRefusal::DeadlineUndeclared)?;
        if !(declared.is_finite() && declared > 0.0) {
            return Err(RemoteRefusal::DeadlineInvalid);
        }
        if let Some(allowed) = self.profile_max_chunk_age_ms {
            if allowed.is_finite() && declared > allowed {
                return Err(RemoteRefusal::DeadlineExceedsProfile {
                    declared_ms: declared,
                    allowed_ms: allowed,
                });
            }
        }
        Ok((declared * 1_000_000.0) as u64)
    }
}

/// One observation going out to the session.
#[derive(Debug, Clone, Copy)]
pub struct Outbound<'a> {
    /// This process's sequence number; the chunk answering it names it back.
    pub seq: u64,
    /// How old the oldest input in it is at the moment it is handed over, in milliseconds. The link
    /// turns this into the capture stamp the wire carries on its own clock; this process never reads
    /// a stamp back.
    pub age_ms: f64,
    /// Joint positions, as the body reported them. `NaN` above the declared joint count (§24).
    pub state: &'a [f32],
    /// The newest camera frame's bytes, when a pool is attached and delivered one.
    pub frame: Option<&'a [u8]>,
}

/// What the link reports.
#[derive(Debug, Clone, PartialEq)]
pub enum LinkEvent {
    /// A session is open on a replica.
    Session {
        /// The Hub's session id.
        id: String,
        /// The model digest the replica serves.
        model_digest: String,
    },
    /// A chunk of actions answering one observation.
    Chunk {
        /// The session it came on.
        session: String,
        /// The observation it answers, by this process's sequence number.
        obs_seq: u64,
        /// Actions per second.
        hz: f64,
        /// Rows.
        horizon: usize,
        /// Columns.
        dims: usize,
        /// Row-major, `horizon * dims` values.
        actions: Vec<f32>,
    },
    /// The session ended, for any reason: link loss, preemption, balance, an operator closing it.
    Ended {
        /// The reason, as the bridge knows it.
        reason: String,
    },
}

/// The transport to the session bridge. Non-blocking in both directions: a control cycle never waits
/// on the network.
pub trait Link {
    /// Hand one observation to the session. `false` when the link cannot take it now (not connected,
    /// or its buffer is full); the observation is then not in flight and will be sent again later.
    fn send(&mut self, observation: &Outbound<'_>) -> bool;
    /// Whatever has arrived since the last poll, appended to `events`.
    fn poll(&mut self, events: &mut Vec<LinkEvent>);
}

/// Why each cycle was quiet or commanded, and what arrived. Every cause is its own counter, because
/// a robot that held for ten seconds has one of them to go and look at.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RemoteCounts {
    /// Cycles that proposed an action.
    pub commanded: u64,
    /// Quiet: no session was open.
    pub quiet_no_session: u64,
    /// Quiet: a session was open and no chunk had arrived to act on.
    pub quiet_no_plan: u64,
    /// Quiet: the plan was older than the deadline.
    pub quiet_stale: u64,
    /// Quiet: the plan's last action was in the past.
    pub quiet_exhausted: u64,
    /// Quiet: after a lost session, awaiting the operator's acknowledgement.
    pub quiet_awaiting_resume: u64,
    /// Observations handed to the link.
    pub sent: u64,
    /// Chunks accepted into the plan.
    pub chunks: u64,
    /// Chunks answering no observation this process remembers, or from a session not open.
    pub chunks_unknown: u64,
    /// Chunks of the wrong shape, or with a value that is not finite.
    pub chunks_malformed: u64,
    /// Chunks older than the deadline on arrival, or older than the plan already held.
    pub chunks_late: u64,
    /// Sessions opened.
    pub sessions: u64,
    /// Sessions that ended.
    pub sessions_ended: u64,
    /// Acknowledgements that resumed motion.
    pub resumed: u64,
}

#[derive(Debug, Clone)]
struct Plan {
    capture_ns: u64,
    period_ns: u64,
    horizon: usize,
    dims: usize,
    actions: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
enum Session {
    None,
    Open { id: String, model_digest: String },
}

/// The shared flag an operator's local acknowledgement sets (§90.2). Cloned into whatever the
/// operator reaches — a file watch, a console button — and never into the link.
#[derive(Debug, Clone, Default)]
pub struct Acknowledgement(Arc<AtomicBool>);

impl Acknowledgement {
    /// A fresh, unset flag.
    pub fn new() -> Acknowledgement {
        Acknowledgement::default()
    }

    /// An operator acknowledges. It resumes only a policy that is waiting: an acknowledgement given
    /// before a loss is not an authorisation for the next one.
    pub fn give(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    fn take(&self) -> bool {
        self.0.swap(false, Ordering::SeqCst)
    }
}

/// §90.2's backend.
#[derive(Debug)]
pub struct Remote<L: Link> {
    link: L,
    deadline_ns: u64,
    channels: usize,
    resume: ResumeRule,
    acknowledgement: Acknowledgement,
    awaiting_resume: bool,
    session: Session,
    ever_opened: bool,
    plan: Option<Plan>,
    pending: [(u64, u64); PENDING],
    pending_len: usize,
    next_seq: u64,
    in_flight: Option<(u64, u64)>,
    events: Vec<LinkEvent>,
    state: [f32; MAX_CHANNELS],
    counts: RemoteCounts,
}

impl<L: Link> Remote<L> {
    /// A remote policy over `link`, commanding `channels` channels, under `declaration`.
    ///
    /// Refused, with §87.4's code, when the declaration names no usable deadline.
    pub fn open(
        link: L,
        channels: usize,
        declaration: RemoteDeclaration,
        acknowledgement: Acknowledgement,
    ) -> Result<Remote<L>, RemoteRefusal> {
        let deadline_ns = declaration.deadline_ns()?;
        Ok(Remote {
            link,
            deadline_ns,
            channels: channels.min(MAX_CHANNELS),
            resume: declaration.resume,
            acknowledgement,
            awaiting_resume: false,
            session: Session::None,
            ever_opened: false,
            plan: None,
            pending: [(0, 0); PENDING],
            pending_len: 0,
            next_seq: 1,
            in_flight: None,
            events: Vec::new(),
            state: [f32::NAN; MAX_CHANNELS],
            counts: RemoteCounts::default(),
        })
    }

    /// What happened, cause by cause.
    pub fn counts(&self) -> RemoteCounts {
        self.counts
    }

    /// The session and model digest the current plan came from, for the recording (§90.2: the
    /// recording notes which cycles ran on remote proposals, with the session id and the digest).
    pub fn provenance(&self) -> Option<(&str, &str)> {
        match &self.session {
            Session::Open { id, model_digest } => Some((id.as_str(), model_digest.as_str())),
            Session::None => None,
        }
    }

    /// Whether the policy is waiting for an operator before it may propose again.
    pub fn awaiting_resume(&self) -> bool {
        self.awaiting_resume
    }

    /// The deadline this policy runs under, in milliseconds.
    pub fn deadline_ms(&self) -> f64 {
        self.deadline_ns as f64 / 1_000_000.0
    }

    /// The link, for a caller that reports on it.
    pub fn link(&self) -> &L {
        &self.link
    }

    fn remember(&mut self, seq: u64, capture_ns: u64) {
        if self.pending_len == PENDING {
            self.pending.copy_within(1.., 0);
            self.pending_len -= 1;
        }
        self.pending[self.pending_len] = (seq, capture_ns);
        self.pending_len += 1;
    }

    /// The capture stamp of the observation `seq`, forgetting it and every older one.
    fn recall(&mut self, seq: u64) -> Option<u64> {
        let position = self.pending[..self.pending_len]
            .iter()
            .position(|(sent, _)| *sent == seq)?;
        let capture = self.pending[position].1;
        self.pending.copy_within(position + 1..self.pending_len, 0);
        self.pending_len -= position + 1;
        Some(capture)
    }

    fn lose_session(&mut self) {
        if self.session != Session::None {
            self.counts.sessions_ended += 1;
        }
        self.session = Session::None;
        self.plan = None;
        self.in_flight = None;
        self.pending_len = 0;
        if self.ever_opened && self.resume == ResumeRule::Acknowledge {
            self.awaiting_resume = true;
        }
    }

    fn take_events(&mut self, now_ns: u64) {
        self.events.clear();
        let mut events = core::mem::take(&mut self.events);
        self.link.poll(&mut events);
        for event in events.drain(..) {
            match event {
                LinkEvent::Session { id, model_digest } => {
                    // A second Session without an Ended between is a new session all the same: the
                    // old one's plan answers observations the new replica never saw.
                    if self.session != Session::None {
                        self.lose_session();
                    }
                    self.session = Session::Open { id, model_digest };
                    self.ever_opened = true;
                    self.counts.sessions += 1;
                }
                LinkEvent::Ended { .. } => self.lose_session(),
                LinkEvent::Chunk {
                    session,
                    obs_seq,
                    hz,
                    horizon,
                    dims,
                    actions,
                } => self.accept(now_ns, &session, obs_seq, hz, horizon, dims, actions),
            }
        }
        self.events = events;
    }

    #[allow(clippy::too_many_arguments)]
    fn accept(
        &mut self,
        now_ns: u64,
        session: &str,
        obs_seq: u64,
        hz: f64,
        horizon: usize,
        dims: usize,
        actions: Vec<f32>,
    ) {
        let open = matches!(&self.session, Session::Open { id, .. } if id == session);
        if !open {
            self.counts.chunks_unknown += 1;
            return;
        }
        if self.in_flight.map(|(seq, _)| seq) == Some(obs_seq) {
            self.in_flight = None;
        }
        let Some(capture_ns) = self.recall(obs_seq) else {
            self.counts.chunks_unknown += 1;
            return;
        };
        let shaped = hz.is_finite()
            && hz > 0.0
            && horizon > 0
            && dims == self.channels
            && actions.len() == horizon * dims
            && actions.iter().all(|value| value.is_finite());
        if !shaped {
            self.counts.chunks_malformed += 1;
            return;
        }
        if now_ns.saturating_sub(capture_ns) > self.deadline_ns {
            self.counts.chunks_late += 1;
            return;
        }
        if let Some(plan) = &self.plan {
            if plan.capture_ns >= capture_ns {
                self.counts.chunks_late += 1;
                return;
            }
        }
        self.plan = Some(Plan {
            capture_ns,
            period_ns: (1_000_000_000.0 / hz) as u64,
            horizon,
            dims,
            actions,
        });
        self.counts.chunks += 1;
    }

    fn send(&mut self, now_ns: u64, body: &Body<'_>, sight: &Sight<'_>) {
        if !matches!(self.session, Session::Open { .. }) {
            return;
        }
        // One observation in flight, and one that has waited past the deadline is abandoned: its
        // answer would be refused on arrival.
        if let Some((_, sent_at)) = self.in_flight {
            if now_ns.saturating_sub(sent_at) <= self.deadline_ns {
                return;
            }
            self.in_flight = None;
        }
        let (state_stamp, joints) = match body {
            Body::Fresh { observation, .. } => {
                let joints = observation
                    .joints
                    .min(observation.position.len())
                    .min(MAX_CHANNELS);
                for joint in 0..MAX_CHANNELS {
                    self.state[joint] = if joint < joints {
                        observation.position[joint]
                    } else {
                        f32::NAN
                    };
                }
                (Some(observation.stamp_ns), joints)
            }
            // Without the body there is nothing true to send: the replica would be answering an
            // observation of a robot nobody measured.
            Body::Empty | Body::Torn | Body::Detached => return,
        };
        let (frame, frame_stamp) = match sight {
            Sight::Frame {
                pixels, stamp_ns, ..
            } => (Some(*pixels), Some(*stamp_ns)),
            Sight::NoFrameYet | Sight::Missed | Sight::Blind => (None, None),
        };
        // The observation is as old as its oldest input.
        let capture_ns = match (state_stamp, frame_stamp) {
            (Some(state), Some(frame)) => state.min(frame),
            (Some(state), None) => state,
            (None, Some(frame)) => frame,
            (None, None) => return,
        };
        let seq = self.next_seq;
        let outbound = Outbound {
            seq,
            age_ms: now_ns.saturating_sub(capture_ns) as f64 / 1_000_000.0,
            state: &self.state[..joints],
            frame,
        };
        if self.link.send(&outbound) {
            self.next_seq += 1;
            self.remember(seq, capture_ns);
            self.in_flight = Some((seq, now_ns));
            self.counts.sent += 1;
        }
    }

    /// The action for `now_ns`, or the reason there is none.
    fn action(&mut self, now_ns: u64) -> Result<Intent, fn(&mut RemoteCounts)> {
        if !matches!(self.session, Session::Open { .. }) {
            return Err(|counts| counts.quiet_no_session += 1);
        }
        let Some(plan) = &self.plan else {
            return Err(|counts| counts.quiet_no_plan += 1);
        };
        let age = now_ns.saturating_sub(plan.capture_ns);
        if age > self.deadline_ns {
            self.plan = None;
            return Err(|counts| counts.quiet_stale += 1);
        }
        let index = (age / plan.period_ns.max(1)) as usize;
        if index >= plan.horizon {
            return Err(|counts| counts.quiet_exhausted += 1);
        }
        let row = &plan.actions[index * plan.dims..(index + 1) * plan.dims];
        Ok(Intent::apply(row))
    }
}

impl<L: Link> Backend for Remote<L> {
    fn name(&self) -> &'static str {
        "remote"
    }

    fn decide(&mut self, _tick: u64, now_ns: u64, body: Body<'_>, sight: Sight<'_>) -> Intent {
        self.take_events(now_ns);
        // An acknowledgement resumes only a policy that is waiting, and is spent either way.
        if self.acknowledgement.take() && self.awaiting_resume {
            self.awaiting_resume = false;
            self.counts.resumed += 1;
        }
        self.send(now_ns, &body, &sight);
        let decided = self.action(now_ns);
        match decided {
            Ok(_) if self.awaiting_resume => {
                self.counts.quiet_awaiting_resume += 1;
                Intent::Silent
            }
            Ok(intent) => {
                self.counts.commanded += 1;
                intent
            }
            Err(count) => {
                count(&mut self.counts);
                Intent::Silent
            }
        }
    }
}

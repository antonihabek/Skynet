//! Tests for §90.2's remote backend and its bridge.
//!
//! The first group is the declaration: a remote policy with no deadline never runs. The second is
//! the arithmetic every action is held to — age from the capture of the observation it answers, on
//! this process's clock, never from anything the link says. The third is §90.3's
//! `offline.control.remote_holds`, as a simulation: the network is removed mid-task, the policy goes
//! quiet within the deadline, no action older than it is ever proposed, and motion does not resume
//! until a new session delivers a chunk **and** the resume rule is satisfied. The last group is the
//! bridge: its wire, and a real Unix socket.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::rc::Rc;
use std::sync::atomic::AtomicU32;

use skynet_ring::{mode, words_for, Ring};

use crate::backend::{Backend, Body, Commanding, Intent, Sight};
use crate::bridge::{
    base64_into, encode_observation, parse_event, BridgeEvent, BridgeProcess, SocketLink,
};
use crate::remote::{
    Acknowledgement, Link, LinkEvent, Outbound, Remote, RemoteDeclaration, RemoteRefusal,
    ResumeRule,
};
use crate::run::{PolicyHost, StepClock, Tick};

const MS: u64 = 1_000_000;

#[derive(Debug, Default)]
struct Wire {
    sent: Vec<(u64, f64, Vec<f32>, bool)>,
    incoming: VecDeque<LinkEvent>,
    refuse: bool,
}

#[derive(Debug, Clone, Default)]
struct Scripted(Rc<RefCell<Wire>>);

impl Link for Scripted {
    fn send(&mut self, observation: &Outbound<'_>) -> bool {
        let mut wire = self.0.borrow_mut();
        if wire.refuse {
            return false;
        }
        wire.sent.push((
            observation.seq,
            observation.age_ms,
            observation.state.to_vec(),
            observation.frame.is_some(),
        ));
        true
    }

    fn poll(&mut self, events: &mut Vec<LinkEvent>) {
        events.extend(self.0.borrow_mut().incoming.drain(..));
    }
}

impl Scripted {
    fn push(&self, event: LinkEvent) {
        self.0.borrow_mut().incoming.push_back(event);
    }

    fn last_seq(&self) -> Option<u64> {
        self.0.borrow().sent.last().map(|entry| entry.0)
    }

    fn sent(&self) -> usize {
        self.0.borrow().sent.len()
    }
}

fn declaration(deadline_ms: Option<f64>, resume: ResumeRule) -> RemoteDeclaration {
    RemoteDeclaration {
        max_chunk_age_ms: deadline_ms,
        profile_max_chunk_age_ms: None,
        resume,
    }
}

fn observation(stamp_ns: u64) -> skynet_obs::Observation {
    let mut observation = skynet_obs::Observation {
        stamp_ns,
        seq: 1,
        joints: 2,
        ..skynet_obs::Observation::default()
    };
    observation.position[0] = 0.25;
    observation.position[1] = -0.5;
    observation
}

fn open(deadline_ms: f64, resume: ResumeRule) -> (Remote<Scripted>, Scripted, Acknowledgement) {
    let link = Scripted::default();
    let acknowledgement = Acknowledgement::new();
    let remote = Remote::open(
        link.clone(),
        2,
        declaration(Some(deadline_ms), resume),
        acknowledgement.clone(),
    )
    .expect("a declared deadline opens");
    (remote, link, acknowledgement)
}

fn session(id: &str) -> LinkEvent {
    LinkEvent::Session {
        id: id.to_string(),
        model_digest: format!("sha256:{}", "a".repeat(64)),
    }
}

fn chunk(session: &str, obs_seq: u64, hz: f64, rows: &[[f32; 2]]) -> LinkEvent {
    LinkEvent::Chunk {
        session: session.to_string(),
        obs_seq,
        hz,
        horizon: rows.len(),
        dims: 2,
        actions: rows.iter().flat_map(|row| row.iter().copied()).collect(),
    }
}

/// Decide at `now` with a body captured `body_age` before it.
fn step(remote: &mut Remote<Scripted>, tick: u64, now: u64, body_age: u64) -> Intent {
    let body = observation(now - body_age);
    remote.decide(
        tick,
        now,
        Body::Fresh {
            observation: &body,
            age_ms: body_age as f64 / MS as f64,
            budgeted: false,
        },
        Sight::Blind,
    )
}

fn first_value(intent: &Intent) -> Option<f32> {
    match intent {
        Intent::Command { values, mask } if mask[0] == 1 => Some(values[0]),
        _ => None,
    }
}

// ── the declaration ──────────────────────────────────────────────────────────

/// §90.2: *"A remote policy with no declared deadline is refused at open with
/// `deadline_undeclared`, the same refusal and the same code as §87.4."*
#[test]
fn a_remote_policy_with_no_deadline_never_runs() {
    let refusal = Remote::open(
        Scripted::default(),
        2,
        declaration(None, ResumeRule::Acknowledge),
        Acknowledgement::new(),
    )
    .expect_err("refused");
    assert_eq!(refusal, RemoteRefusal::DeadlineUndeclared);
    assert_eq!(refusal.code(), "deadline_undeclared");
    assert!(refusal.message().contains("§87.4"));
}

#[test]
fn a_deadline_must_be_a_positive_finite_number() {
    for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
        let refusal = Remote::open(
            Scripted::default(),
            2,
            declaration(Some(bad), ResumeRule::Acknowledge),
            Acknowledgement::new(),
        )
        .expect_err("refused");
        assert_eq!(refusal.code(), "deadline_invalid", "{bad}");
    }
}

/// *"No maximum is invented here. The figure is the package's, bounded by the operator profile."*
#[test]
fn the_operator_profile_bounds_the_packages_deadline() {
    let mut bounded = declaration(Some(400.0), ResumeRule::Acknowledge);
    bounded.profile_max_chunk_age_ms = Some(250.0);
    let refusal = bounded.deadline_ns().expect_err("above the profile");
    assert_eq!(
        refusal,
        RemoteRefusal::DeadlineExceedsProfile {
            declared_ms: 400.0,
            allowed_ms: 250.0
        }
    );
    bounded.max_chunk_age_ms = Some(250.0);
    assert_eq!(bounded.deadline_ns(), Ok(250 * MS));
    // No profile bound, no invented one.
    assert_eq!(
        declaration(Some(5_000.0), ResumeRule::Acknowledge).deadline_ns(),
        Ok(5_000 * MS)
    );
}

// ── the arithmetic ───────────────────────────────────────────────────────────

/// Quiet until there is a session and a chunk; the observation carries the body's own age.
#[test]
fn it_is_quiet_until_a_session_delivers_a_chunk() {
    let (mut remote, link, _) = open(250.0, ResumeRule::Acknowledge);
    let t0 = 1_000 * MS;

    assert_eq!(step(&mut remote, 0, t0, 3 * MS), Intent::Silent);
    assert_eq!(remote.counts().quiet_no_session, 1);
    assert_eq!(link.sent(), 0, "nothing is sent without a session");

    link.push(session("s1"));
    assert_eq!(step(&mut remote, 1, t0 + 10 * MS, 3 * MS), Intent::Silent);
    assert_eq!(remote.counts().quiet_no_plan, 1);
    let wire = link.0.borrow();
    let (seq, age_ms, state, frame) = &wire.sent[0];
    assert_eq!(*seq, 1);
    assert!(
        (age_ms - 3.0).abs() < 1e-9,
        "the age of the body, measured here: {age_ms}"
    );
    assert_eq!(state, &vec![0.25, -0.5]);
    assert!(!frame);
}

/// Each action is for `capture + k / hz`, and none is proposed once the capture is older than the
/// deadline — whatever the chunk's horizon would have allowed.
#[test]
fn actions_are_timed_from_the_capture_and_cut_off_at_the_deadline() {
    let (mut remote, link, _) = open(250.0, ResumeRule::Acknowledge);
    let t0 = 1_000 * MS;
    link.push(session("s1"));
    // Sent at t0 with a body captured at t0 - 5 ms.
    step(&mut remote, 0, t0, 5 * MS);
    let capture = t0 - 5 * MS;
    // 10 Hz, 4 rows: rows are for capture+0, +100, +200, +300 ms.
    link.push(chunk(
        "s1",
        1,
        10.0,
        &[[1.0, 0.0], [2.0, 0.0], [3.0, 0.0], [4.0, 0.0]],
    ));

    let at = |remote: &mut Remote<Scripted>, since_capture_ms: u64| {
        let intent = step(remote, 1, capture + since_capture_ms * MS, 0);
        first_value(&intent)
    };
    assert_eq!(at(&mut remote, 40), Some(1.0));
    assert_eq!(at(&mut remote, 140), Some(2.0));
    assert_eq!(at(&mut remote, 240), Some(3.0));
    // Row 3 is for +300 ms, and the deadline is 250: it is never proposed.
    assert_eq!(at(&mut remote, 260), None);
    assert_eq!(remote.counts().quiet_stale, 1);
    assert_eq!(
        at(&mut remote, 310),
        None,
        "a stale plan is dropped, not revisited"
    );
}

/// A chunk is used only when it answers an observation this process sent, on the session that is
/// open, in the declared shape, with every value finite, and younger than the deadline.
#[test]
fn chunks_that_answer_nothing_or_are_misshapen_are_dropped() {
    let (mut remote, link, _) = open(250.0, ResumeRule::Acknowledge);
    let t0 = 1_000 * MS;
    link.push(session("s1"));
    step(&mut remote, 0, t0, 0);

    link.push(chunk("s1", 99, 10.0, &[[1.0, 0.0]])); // answers nothing sent
    link.push(chunk("other", 1, 10.0, &[[1.0, 0.0]])); // another session
    link.push(LinkEvent::Chunk {
        session: "s1".to_string(),
        obs_seq: 1,
        hz: 10.0,
        horizon: 1,
        dims: 3,
        actions: vec![1.0, 2.0, 3.0],
    }); // three columns for a two-channel lease
    assert_eq!(step(&mut remote, 1, t0 + 10 * MS, 0), Intent::Silent);
    let counts = remote.counts();
    assert_eq!(counts.chunks_unknown, 2);
    assert_eq!(counts.chunks_malformed, 1);
    assert_eq!(counts.chunks, 0);

    // A value that is not finite refuses the whole chunk, never just the row.
    step(&mut remote, 2, t0 + 300 * MS, 0);
    let seq = link
        .last_seq()
        .expect("sent again after the first was abandoned");
    link.push(chunk("s1", seq, 10.0, &[[1.0, f32::NAN]]));
    assert_eq!(step(&mut remote, 3, t0 + 310 * MS, 0), Intent::Silent);
    assert_eq!(remote.counts().chunks_malformed, 2);
}

#[test]
fn a_chunk_older_than_the_deadline_on_arrival_is_never_used() {
    let (mut remote, link, _) = open(100.0, ResumeRule::Acknowledge);
    let t0 = 1_000 * MS;
    link.push(session("s1"));
    step(&mut remote, 0, t0, 0);
    // Arrives 150 ms after its observation was captured.
    link.push(chunk("s1", 1, 10.0, &[[1.0, 0.0], [2.0, 0.0]]));
    assert_eq!(step(&mut remote, 1, t0 + 150 * MS, 0), Intent::Silent);
    assert_eq!(remote.counts().chunks_late, 1);
}

// ── §90.3, offline.control.remote_holds ─────────────────────────────────────

/// A responsive replica: every observation is answered `latency` after it was sent with a four-row
/// chunk at 50 Hz, whose first value is the observation's own sequence number. While disconnected,
/// what it would have answered is lost, as it is on a real link.
struct Replica {
    link: Scripted,
    latency: u64,
    queue: VecDeque<(u64, u64)>,
    queued_through: u64,
    connected: bool,
    session: String,
}

impl Replica {
    fn serve(&mut self, now: u64) {
        let fresh: Vec<u64> = self
            .link
            .0
            .borrow()
            .sent
            .iter()
            .map(|entry| entry.0)
            .filter(|seq| *seq > self.queued_through)
            .collect();
        for seq in fresh {
            self.queue.push_back((seq, now));
            self.queued_through = seq;
        }
        while let Some((seq, at)) = self.queue.front().copied() {
            if now < at + self.latency {
                break;
            }
            self.queue.pop_front();
            if self.connected {
                self.link
                    .push(chunk(&self.session, seq, 50.0, &[[seq as f32, 0.0]; 4]));
            }
        }
    }
}

/// The bench test, simulated: 10 ms control cycles, a 30 ms replica, a 120 ms deadline. The network
/// is removed at 500 ms, the bridge notices at 800 ms, and a new session opens at 1.2 s.
/// Each tick and the first channel it proposed, if it proposed anything.
type Trace = Vec<(u64, Option<f32>)>;

fn remote_holds(resume: ResumeRule) -> (Trace, Remote<Scripted>, Acknowledgement) {
    let (mut remote, link, acknowledgement) = open(120.0, resume);
    let mut replica = Replica {
        link: link.clone(),
        latency: 30 * MS,
        queue: VecDeque::new(),
        queued_through: 0,
        connected: true,
        session: "s1".to_string(),
    };
    link.push(session("s1"));
    let t0 = 10_000 * MS;
    let mut trace = Vec::new();
    let mut captured = std::collections::HashMap::new();
    for tick in 0..200u64 {
        let now = t0 + tick * 10 * MS;
        if tick == 50 {
            // The network is removed mid-task: nothing more arrives, and the bridge notices a while
            // later — deliberately later than the deadline, which the robot must not wait for.
            replica.connected = false;
        }
        if tick == 80 {
            link.push(LinkEvent::Ended {
                reason: "link_lost".to_string(),
            });
        }
        if tick == 120 {
            replica.connected = true;
            replica.session = "s2".to_string();
            replica.queue.clear();
            link.push(session("s2"));
        }
        replica.serve(now);
        // Bodies captured 2 ms before each tick.
        let before = link.sent();
        let intent = step(&mut remote, tick, now, 2 * MS);
        if link.sent() > before {
            captured.insert(link.last_seq().unwrap(), now - 2 * MS);
        }
        if let Some(value) = first_value(&intent) {
            // §90.3: no command older than the deadline reaches the ring. The value names the
            // observation it answers; its capture is this test's own record, not the policy's.
            let capture = captured[&(value as u64)];
            assert!(
                now - capture <= 120 * MS,
                "tick {tick}: an action {} ms old",
                (now - capture) / MS
            );
        }
        trace.push((tick, first_value(&intent)));
    }
    (trace, remote, acknowledgement)
}

fn commanding(trace: &[(u64, Option<f32>)], from: u64, to: u64) -> usize {
    trace
        .iter()
        .filter(|(tick, value)| *tick >= from && *tick < to && value.is_some())
        .count()
}

#[test]
fn remote_holds_goes_quiet_within_the_deadline_and_waits_for_the_operator() {
    let (trace, remote, _) = remote_holds(ResumeRule::Acknowledge);
    // Before the loss it was driving.
    assert!(commanding(&trace, 10, 50) > 30, "{trace:?}");
    // The last chunk answered an observation captured before 500 ms; 120 ms later at most, the
    // policy is quiet — long before the bridge reports the loss at 800 ms.
    let quiet_from = trace
        .iter()
        .find(|(tick, value)| *tick >= 50 && value.is_none())
        .map(|(tick, _)| *tick)
        .expect("it went quiet");
    assert!(quiet_from <= 50 + 12, "quiet at tick {quiet_from}");
    assert_eq!(
        commanding(&trace, quiet_from, 200),
        0,
        "and stayed quiet: {trace:?}"
    );
    // A new session delivered chunks from tick 120, and with the default rule nothing moved.
    let counts = remote.counts();
    assert!(counts.quiet_awaiting_resume > 0);
    assert_eq!(counts.sessions, 2);
    assert!(remote.awaiting_resume());
}

#[test]
fn an_operator_acknowledgement_resumes_only_after_the_loss() {
    let (mut remote, link, acknowledgement) = open(120.0, ResumeRule::Acknowledge);
    let t0 = 10_000 * MS;
    // Given before anything was lost: it authorises nothing.
    acknowledgement.give();
    link.push(session("s1"));
    step(&mut remote, 0, t0, 0);
    link.push(chunk("s1", 1, 50.0, &[[1.0, 0.0]; 4]));
    assert_eq!(
        first_value(&step(&mut remote, 1, t0 + 10 * MS, 0)),
        Some(1.0)
    );
    assert_eq!(remote.counts().resumed, 0);

    link.push(LinkEvent::Ended {
        reason: "preempted".to_string(),
    });
    assert_eq!(step(&mut remote, 2, t0 + 20 * MS, 0), Intent::Silent);
    link.push(session("s2"));
    step(&mut remote, 3, t0 + 30 * MS, 0);
    let seq = link.last_seq().unwrap();
    link.push(chunk("s2", seq, 50.0, &[[7.0, 0.0]; 4]));
    assert_eq!(
        step(&mut remote, 4, t0 + 40 * MS, 0),
        Intent::Silent,
        "a fresh chunk alone does not resume"
    );
    assert!(remote.awaiting_resume());

    acknowledgement.give();
    assert_eq!(
        first_value(&step(&mut remote, 5, t0 + 50 * MS, 0)),
        Some(7.0)
    );
    assert_eq!(remote.counts().resumed, 1);
}

#[test]
fn an_automatic_profile_resumes_on_the_new_sessions_first_fresh_chunk() {
    let (trace, remote, _) = remote_holds(ResumeRule::Automatic);
    let quiet = commanding(&trace, 62, 120);
    assert_eq!(quiet, 0, "quiet while no session delivers: {trace:?}");
    assert!(
        commanding(&trace, 125, 200) > 50,
        "resumed on the new session: {trace:?}"
    );
    assert!(!remote.awaiting_resume());
}

/// Through the real producer: a quiet policy publishes nothing and the sequence does not advance,
/// which is what §7.1's watchdog reads as a stalled producer and answers with a controlled hold.
#[test]
fn a_quiet_remote_policy_does_not_advance_the_sequence() {
    let words: Vec<AtomicU32> = (0..words_for(8)).map(|_| AtomicU32::new(0)).collect();
    let ring = Ring::create(&words, 8).expect("a ring");
    let mut producer = ring.producer();

    let region: Vec<AtomicU32> = (0..skynet_obs::words_for(4))
        .map(|_| AtomicU32::new(0))
        .collect();
    let body_ring =
        skynet_obs::Ring::create(&region, 4, skynet_obs::TimeBase::Monotonic).expect("a body ring");
    let mut bodies = skynet_obs::Ring::open(&region)
        .expect("the same ring")
        .consumer();

    let (remote, link, _) = open(120.0, ResumeRule::Acknowledge);
    let start = 10_000 * MS;
    let lease = Commanding {
        channels: 2,
        observation_budget_ms: None,
        frame_budget_ms: None,
        mode: mode::POSITION,
    };
    let mut host = PolicyHost::new(
        remote,
        StepClock {
            now_ns: start,
            step_ns: 10 * MS,
        },
        lease,
        10 * MS,
    );
    link.push(session("s1"));

    let mut ticks = Vec::new();
    for turn in 0..30u64 {
        let now = start + turn * 10 * MS;
        body_ring
            .producer()
            .publish(&observation(now))
            .expect("published");
        if turn == 2 {
            let seq = link.last_seq().expect("an observation went out");
            link.push(chunk("s1", seq, 50.0, &[[0.5, 0.5]; 6]));
        }
        ticks.push(host.turn(&mut producer, Some(&mut bodies)));
    }
    let published: Vec<u64> = ticks
        .iter()
        .filter_map(|tick| match tick {
            Tick::Published(seq) => Some(*seq),
            _ => None,
        })
        .collect();
    assert!(!published.is_empty(), "the chunk was proposed: {ticks:?}");
    // After the chunk's rows ran out or went stale, every turn is silent and the sequence stops.
    assert!(matches!(ticks.last(), Some(Tick::Silent)), "{ticks:?}");
    let highest = *published.iter().max().unwrap();
    assert_eq!(
        published.len() as u64,
        highest,
        "one sequence number per published command"
    );
}

// ── the bridge ───────────────────────────────────────────────────────────────

#[test]
fn base64_matches_rfc_4648() {
    for (input, expected) in [
        ("", ""),
        ("f", "Zg=="),
        ("fo", "Zm8="),
        ("foo", "Zm9v"),
        ("foob", "Zm9vYg=="),
        ("fooba", "Zm9vYmE="),
        ("foobar", "Zm9vYmFy"),
    ] {
        let mut out = String::new();
        base64_into(input.as_bytes(), &mut out);
        assert_eq!(out, expected, "{input}");
    }
}

#[test]
fn an_observation_is_one_canonical_line_with_unmeasured_joints_as_null() {
    let mut out = Vec::new();
    encode_observation(
        &Outbound {
            seq: 7,
            age_ms: 12.5,
            state: &[0.25, f32::NAN],
            frame: Some(b"foo"),
        },
        &mut out,
    );
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "{\"age_ms\":12.5,\"frame\":\"Zm9v\",\"seq\":7,\"state\":[0.25,null],\"type\":\"observe\"}\n"
    );
}

#[test]
fn the_three_events_parse_and_anything_else_is_refused() {
    assert_eq!(
        parse_event("{\"type\":\"session\",\"id\":\"s1\",\"model_digest\":\"sha256:ab\"}"),
        Some(LinkEvent::Session {
            id: "s1".into(),
            model_digest: "sha256:ab".into()
        })
    );
    assert_eq!(
        parse_event("{\"type\":\"ended\",\"reason\":\"preempted\"}"),
        Some(LinkEvent::Ended {
            reason: "preempted".into()
        })
    );
    assert_eq!(
        parse_event("{\"type\":\"chunk\",\"session\":\"s1\",\"obs_seq\":3,\"hz\":30,\"horizon\":1,\"dims\":2,\"actions\":[0.5,-1]}"),
        Some(LinkEvent::Chunk {
            session: "s1".into(),
            obs_seq: 3,
            hz: 30.0,
            horizon: 1,
            dims: 2,
            actions: vec![0.5, -1.0],
        })
    );
    for bad in [
        "not json",
        "{\"type\":\"resume\"}",
        "{\"type\":\"chunk\",\"session\":\"s1\",\"obs_seq\":-1,\"hz\":30,\"horizon\":1,\"dims\":1,\"actions\":[0]}",
        "{\"type\":\"chunk\",\"session\":\"s1\",\"obs_seq\":1.5,\"hz\":30,\"horizon\":1,\"dims\":1,\"actions\":[0]}",
        "{\"type\":\"chunk\",\"session\":\"s1\",\"obs_seq\":1,\"hz\":30,\"horizon\":1,\"dims\":1,\"actions\":[\"x\"]}",
        "{\"type\":\"session\",\"id\":\"s1\"}",
    ] {
        assert_eq!(parse_event(bad), None, "{bad}");
    }
}

/// A real socket: the bridge listens, the link dials, an observation goes out as a line, events
/// come back, and a bridge that closes is an ended session.
#[test]
fn the_socket_link_talks_to_a_bridge_and_reports_it_closing() {
    let directory = std::env::temp_dir().join(format!("skynetp-bridge-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("bridge.sock");
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).expect("a listening bridge");

    let mut link = SocketLink::to(path.clone());
    let mut events = Vec::new();
    link.poll(&mut events);
    assert!(events.is_empty());
    let (mut accepted, _) = listener.accept().expect("the link dialled");

    assert!(link.send(&Outbound {
        seq: 1,
        age_ms: 2.0,
        state: &[0.5],
        frame: None
    }));
    let mut reader = BufReader::new(accepted.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(
        line.contains("\"seq\":1") && line.ends_with("\"type\":\"observe\"}\n"),
        "{line}"
    );

    accepted
        .write_all(
            b"{\"type\":\"session\",\"id\":\"s1\",\"model_digest\":\"sha256:ab\"}\nnot an event\n",
        )
        .unwrap();
    for _ in 0..200 {
        link.poll(&mut events);
        if !events.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        events,
        vec![LinkEvent::Session {
            id: "s1".into(),
            model_digest: "sha256:ab".into()
        }]
    );
    assert_eq!(link.counts().bad_lines, 1);

    drop(reader);
    drop(accepted);
    events.clear();
    for _ in 0..200 {
        link.poll(&mut events);
        if !events.is_empty() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        events,
        vec![LinkEvent::Ended {
            reason: "bridge_closed".into()
        }]
    );
    assert_eq!(link.counts().disconnects, 1);
    let _ = std::fs::remove_file(&path);
}

// ── the bridge as a child of the policy host ────────────────────────────────

/// A script in a fresh directory, executable, with `body` after the shebang.
fn script(label: &str, body: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!(
        "skynetp-bridge-{}-{label}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("bridge");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

#[test]
fn the_bridge_is_told_its_socket_its_endpoint_and_a_deadline_no_looser_than_ours() {
    let bridge = BridgeProcess::new(
        "/nonexistent".into(),
        std::path::Path::new("/run/skynet/remote/acme:pick.sock"),
        "ep-7f3a",
        250.9,
    );
    assert_eq!(
        bridge.args(),
        [
            "--socket",
            "/run/skynet/remote/acme:pick.sock",
            "--endpoint-id",
            "ep-7f3a",
            "--max-chunk-age-ms",
            "250",
        ],
        "rounded down: the bridge's figure may only tighten ours"
    );
}

#[test]
fn a_bridge_that_exits_is_started_again_but_not_in_a_loop() {
    let program = script("exits", "exit 3");
    let mut bridge = BridgeProcess::new(program, std::path::Path::new("/tmp/x.sock"), "ep", 100.0);
    let start = std::time::Instant::now();
    assert!(matches!(bridge.tend(start), Some(BridgeEvent::Started(_))));
    // Let it exit, then notice.
    let mut exited = None;
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        if let Some(event) = bridge.tend(start) {
            exited = Some(event);
            break;
        }
    }
    assert!(
        matches!(&exited, Some(BridgeEvent::Exited(how)) if how.contains('3')),
        "{exited:?}"
    );
    // Within the interval it is not started again, however often it is tended.
    for _ in 0..10 {
        assert_eq!(bridge.tend(start + std::time::Duration::from_secs(1)), None);
    }
    assert_eq!(bridge.starts(), 1);
    // After it, it is.
    assert!(matches!(
        bridge.tend(start + BridgeProcess::RESPAWN_AFTER),
        Some(BridgeEvent::Started(_))
    ));
    assert_eq!(bridge.starts(), 2);
}

#[test]
fn a_bridge_that_cannot_start_says_so_and_is_tried_again_later() {
    let mut bridge = BridgeProcess::new(
        "/nonexistent/skynet-remote-bridge".into(),
        std::path::Path::new("/tmp/x.sock"),
        "ep",
        100.0,
    );
    let start = std::time::Instant::now();
    assert!(
        matches!(bridge.tend(start), Some(BridgeEvent::Failed(why)) if why.contains("/nonexistent"))
    );
    assert_eq!(bridge.tend(start), None, "not retried inside the interval");
    assert!(matches!(
        bridge.tend(start + BridgeProcess::RESPAWN_AFTER),
        Some(BridgeEvent::Failed(_))
    ));
}

#[test]
fn a_policy_host_that_goes_takes_its_bridge_with_it() {
    let program = script("stays", "exec sleep 30");
    let mut bridge = BridgeProcess::new(program, std::path::Path::new("/tmp/x.sock"), "ep", 100.0);
    let Some(BridgeEvent::Started(pid)) = bridge.tend(std::time::Instant::now()) else {
        panic!("the bridge did not start");
    };
    let alive = std::path::PathBuf::from(format!("/proc/{pid}"));
    assert!(alive.exists());
    drop(bridge);
    assert!(
        !alive.exists(),
        "a bridge left running would hold a billed session open for nobody"
    );
}

//! The one [`Link`] this crate ships: a Unix socket to the session bridge, one JSON object per line.
//!
//! The bridge is a separate process that holds the network half of a hosted session — the Hub
//! request with the device's `skd_` credential, the single-use ticket, the TLS WebSocket to the
//! replica (§90.2) — and listens on a local socket. This process connects to it. Nothing here opens a
//! port or accepts a connection, so nothing on the robot becomes routable (§87.4).
//!
//! # The wire
//!
//! To the bridge, one line per observation:
//!
//! ```text
//! {"age_ms":12.5,"frame":"<base64>"|null,"seq":7,"state":[0.1,null,…],"type":"observe"}
//! ```
//!
//! `age_ms` is how old the observation's oldest input was when it was handed over; the bridge turns
//! it into the capture stamp its wire carries, on its own clock. `state` holds `null` where the body
//! reported `NaN` (§24: not measured). `frame` is the camera's bytes as the pool published them.
//!
//! From the bridge, one line per event:
//!
//! ```text
//! {"type":"session","id":"…","model_digest":"sha256:…"}
//! {"type":"chunk","session":"…","obs_seq":7,"hz":30,"horizon":24,"dims":6,"actions":[…]}
//! {"type":"ended","reason":"…"}
//! ```
//!
//! # Failure is a quiet policy, never a stall
//!
//! Every read and write is non-blocking, so a bridge that hangs costs this process nothing but the
//! chunks it does not deliver — and a policy with no chunks is quiet, which is §90.2's hold. A bridge
//! that closes the socket, or sends a line longer than [`MAX_LINE`], is an ended session. A line that
//! is not one of the three events is counted and dropped. The socket is dialled again every
//! [`REDIAL_EVERY`] polls while it is down.

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use skynet_json::Json;

use crate::remote::{Link, LinkEvent, Outbound};

/// The longest line accepted from the bridge. A chunk of 64 rows × 64 channels in decimal is well
/// under this; anything longer is a bridge that has lost its framing.
pub const MAX_LINE: usize = 4 * 1024 * 1024;

/// Polls between attempts to reach a bridge that is not connected.
pub const REDIAL_EVERY: u64 = 100;

/// What the link has seen, for the run's summary.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BridgeCounts {
    /// Times the socket was connected.
    pub connects: u64,
    /// Times it was lost.
    pub disconnects: u64,
    /// Lines that were not an event this link knows, or were malformed.
    pub bad_lines: u64,
    /// Observations the bridge could not take because the previous one was still being written.
    pub busy: u64,
}

/// A link over a Unix socket at `path`.
#[derive(Debug)]
pub struct SocketLink {
    path: PathBuf,
    stream: Option<UnixStream>,
    inbound: Vec<u8>,
    outbound: Vec<u8>,
    written: usize,
    polls: u64,
    /// A loss noticed while sending, reported by the next poll: `send` has no event list.
    lost: Option<&'static str>,
    counts: BridgeCounts,
}

impl SocketLink {
    /// A link to the bridge listening at `path`. Not dialled until the first poll.
    pub fn to(path: PathBuf) -> SocketLink {
        SocketLink {
            path,
            stream: None,
            inbound: Vec::new(),
            outbound: Vec::new(),
            written: 0,
            polls: 0,
            lost: None,
            counts: BridgeCounts::default(),
        }
    }

    /// What the link has seen.
    pub fn counts(&self) -> BridgeCounts {
        self.counts
    }

    fn dial(&mut self) {
        if let Ok(stream) = UnixStream::connect(&self.path) {
            if stream.set_nonblocking(true).is_ok() {
                self.stream = Some(stream);
                self.inbound.clear();
                self.outbound.clear();
                self.written = 0;
                self.counts.connects += 1;
            }
        }
    }

    fn drop_stream(&mut self, events: &mut Vec<LinkEvent>, reason: &str) {
        if self.stream.take().is_some() {
            self.counts.disconnects += 1;
            events.push(LinkEvent::Ended {
                reason: reason.to_string(),
            });
        }
        self.inbound.clear();
        self.outbound.clear();
        self.written = 0;
    }

    fn flush(&mut self) -> Result<(), ()> {
        let Some(stream) = self.stream.as_mut() else {
            return Ok(());
        };
        while self.written < self.outbound.len() {
            match stream.write(&self.outbound[self.written..]) {
                Ok(0) => return Err(()),
                Ok(count) => self.written += count,
                Err(error) if error.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => return Err(()),
            }
        }
        self.outbound.clear();
        self.written = 0;
        Ok(())
    }
}

impl Link for SocketLink {
    fn send(&mut self, observation: &Outbound<'_>) -> bool {
        if self.stream.is_none() {
            return false;
        }
        if !self.outbound.is_empty() {
            self.counts.busy += 1;
            return false;
        }
        encode_observation(observation, &mut self.outbound);
        // Written now as far as the socket takes it; the rest drains on the next polls.
        if self.flush().is_err() {
            // Reported by the next poll as an ended session; this observation was not handed over.
            if self.stream.take().is_some() {
                self.counts.disconnects += 1;
                self.lost = Some("bridge_lost");
            }
            self.outbound.clear();
            self.written = 0;
            return false;
        }
        true
    }

    fn poll(&mut self, events: &mut Vec<LinkEvent>) {
        self.polls += 1;
        if let Some(reason) = self.lost.take() {
            events.push(LinkEvent::Ended {
                reason: reason.to_string(),
            });
        }
        if self.stream.is_none() {
            if self.polls % REDIAL_EVERY == 1 {
                self.dial();
            }
            if self.stream.is_none() {
                return;
            }
        }
        if self.flush().is_err() {
            self.drop_stream(events, "bridge_lost");
            return;
        }
        let mut buffer = [0u8; 65_536];
        loop {
            let Some(stream) = self.stream.as_mut() else {
                return;
            };
            match stream.read(&mut buffer) {
                Ok(0) => {
                    self.drop_stream(events, "bridge_closed");
                    return;
                }
                Ok(count) => self.inbound.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => continue,
                Err(_) => {
                    self.drop_stream(events, "bridge_lost");
                    return;
                }
            }
            if self.inbound.len() > MAX_LINE && !self.inbound.contains(&b'\n') {
                self.drop_stream(events, "bridge_line_too_long");
                return;
            }
        }
        while let Some(end) = self.inbound.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.inbound.drain(..=end).collect();
            match std::str::from_utf8(&line[..line.len() - 1])
                .ok()
                .and_then(parse_event)
            {
                Some(event) => events.push(event),
                None => self.counts.bad_lines += 1,
            }
        }
    }
}

/// One observation as a line, appended to `out`.
pub fn encode_observation(observation: &Outbound<'_>, out: &mut Vec<u8>) {
    let mut line = String::with_capacity(64 + observation.state.len() * 12);
    line.push_str("{\"age_ms\":");
    push_number(&mut line, observation.age_ms);
    line.push_str(",\"frame\":");
    match observation.frame {
        Some(bytes) => {
            line.push('"');
            base64_into(bytes, &mut line);
            line.push('"');
        }
        None => line.push_str("null"),
    }
    line.push_str(",\"seq\":");
    line.push_str(&observation.seq.to_string());
    line.push_str(",\"state\":[");
    for (index, value) in observation.state.iter().enumerate() {
        if index > 0 {
            line.push(',');
        }
        push_number(&mut line, *value as f64);
    }
    line.push_str("],\"type\":\"observe\"}\n");
    out.extend_from_slice(line.as_bytes());
}

fn push_number(line: &mut String, value: f64) {
    if value.is_finite() {
        match skynet_json::format_number(value) {
            Ok(text) => line.push_str(&text),
            Err(_) => line.push_str("null"),
        }
    } else {
        line.push_str("null");
    }
}

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (RFC 4648 §4).
pub fn base64_into(bytes: &[u8], out: &mut String) {
    out.reserve(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(triple >> 18) as usize & 63] as char);
        out.push(ALPHABET[(triple >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 63] as char
        } else {
            '='
        });
    }
}

/// One line from the bridge as an event, or `None` when it is not one this link knows.
pub fn parse_event(line: &str) -> Option<LinkEvent> {
    let json = Json::parse(line).ok()?;
    match json.get("type")?.as_str()? {
        "session" => Some(LinkEvent::Session {
            id: json.get("id")?.as_str()?.to_string(),
            model_digest: json.get("model_digest")?.as_str()?.to_string(),
        }),
        "ended" => Some(LinkEvent::Ended {
            reason: json
                .get("reason")
                .and_then(Json::as_str)
                .unwrap_or("unknown")
                .to_string(),
        }),
        "chunk" => {
            let obs_seq = json.get("obs_seq")?.as_f64()?;
            if !(obs_seq >= 0.0 && obs_seq.fract() == 0.0 && obs_seq < 9.007_199_254_740_992e15) {
                return None;
            }
            let mut actions = Vec::new();
            for value in json.get("actions")?.as_arr()? {
                actions.push(value.as_f64()? as f32);
            }
            Some(LinkEvent::Chunk {
                session: json.get("session")?.as_str()?.to_string(),
                obs_seq: obs_seq as u64,
                hz: json.get("hz")?.as_f64()?,
                horizon: json.get("horizon")?.as_usize()?,
                dims: json.get("dims")?.as_usize()?,
                actions,
            })
        }
        _ => None,
    }
}

/// The session bridge as a child of this process, when the runtime that started it asked for one.
///
/// `skynetd` starts a remote-policy package as this binary rather than as anything the package
/// carries (§90.2: it runs no publisher code on the robot), and this process starts the bridge.
/// Owning it here keeps one supervised unit per package: the reconciler sees the package it was
/// asked to run and nothing else, and stopping the unit stops both, because systemd stops a unit's
/// whole cgroup.
///
/// A bridge that exits is started again, never sooner than [`BridgeProcess::RESPAWN_AFTER`] after
/// the last start, so a bridge that cannot start costs a line in the journal every few seconds and
/// not a fork loop. While it is down, nothing is proposed: the link has no session, which is
/// §90.2's hold.
///
/// Its credential is not an argument. The runtime puts the path of the device credential in this
/// process's environment and the child inherits it, so it appears on no command line.
#[derive(Debug)]
pub struct BridgeProcess {
    program: PathBuf,
    args: Vec<String>,
    child: Option<Child>,
    started_at: Option<Instant>,
    starts: u64,
}

/// What [`BridgeProcess::tend`] did, for a line in the journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeEvent {
    /// Started, with the process id.
    Started(u32),
    /// Found to have exited, with how.
    Exited(String),
    /// Could not be started.
    Failed(String),
}

impl BridgeProcess {
    /// The shortest interval between two starts.
    pub const RESPAWN_AFTER: Duration = Duration::from_secs(5);

    /// A bridge for the hosted session behind `endpoint_id`, listening at `socket`.
    ///
    /// The deadline is passed down as whole milliseconds, rounded **down**: the bridge's figure only
    /// tightens what its session serves, and the one this process enforces is its own.
    pub fn new(
        program: PathBuf,
        socket: &Path,
        endpoint_id: &str,
        max_chunk_age_ms: f64,
    ) -> BridgeProcess {
        let whole = if max_chunk_age_ms.is_finite() && max_chunk_age_ms >= 1.0 {
            max_chunk_age_ms.floor() as u64
        } else {
            1
        };
        BridgeProcess {
            program,
            args: bridge_args(socket, endpoint_id, whole),
            child: None,
            started_at: None,
            starts: 0,
        }
    }

    /// The arguments it is started with.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// How many times it has been started.
    pub fn starts(&self) -> u64 {
        self.starts
    }

    /// Start it if it is not running and the interval since the last start has passed.
    pub fn tend(&mut self, now: Instant) -> Option<BridgeEvent> {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(None) => return None,
                Ok(Some(status)) => {
                    self.child = None;
                    return Some(BridgeEvent::Exited(status.to_string()));
                }
                Err(error) => {
                    self.child = None;
                    return Some(BridgeEvent::Exited(error.to_string()));
                }
            }
        }
        if self
            .started_at
            .is_some_and(|at| now.saturating_duration_since(at) < Self::RESPAWN_AFTER)
        {
            return None;
        }
        self.started_at = Some(now);
        self.starts += 1;
        match Command::new(&self.program).args(&self.args).spawn() {
            Ok(child) => {
                let id = child.id();
                self.child = Some(child);
                Some(BridgeEvent::Started(id))
            }
            Err(error) => Some(BridgeEvent::Failed(format!(
                "{} could not be started: {error}",
                self.program.display()
            ))),
        }
    }
}

impl Drop for BridgeProcess {
    fn drop(&mut self) {
        // Best effort: systemd stops the unit's whole cgroup anyway, and a bridge left running by a
        // policy host that exited by hand would hold a billed session open for nobody.
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn bridge_args(socket: &Path, endpoint_id: &str, max_chunk_age_ms: u64) -> Vec<String> {
    vec![
        "--socket".to_string(),
        socket.display().to_string(),
        "--endpoint-id".to_string(),
        endpoint_id.to_string(),
        "--max-chunk-age-ms".to_string(),
        max_chunk_age_ms.to_string(),
    ]
}

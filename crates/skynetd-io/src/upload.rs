//! §79's bytes: sending a sealed black box window to the Hub, resumably, and writing down what the
//! far side said it got.
//!
//! [`skynet_contracts::upload`] decides *whether* to send, from where, and what an answer means —
//! and it takes no network, no clock and no filesystem. This is the other half: the probe, the
//! `PUT`, the receipt on the disk, and the one rule that keeps a supervisor supervising while tens
//! of megabytes move.
//!
//! For as long as §77 has existed, an incident has carried the window's *identity* — name, size,
//! digest, record count — with `uploaded: false`, and nothing has ever collected the bytes. §77.1
//! said so plainly and called the upload *"the mirror of §75's fetch, and unwritten"*. This is that
//! mirror.
//!
//! # Four rules it shares with [`crate::bundle`], and two it does not
//!
//! Shared, because they are properties of §33.2's single thread rather than of a direction:
//!
//! 1. **A slice, not a transfer.** Each pass moves what it can inside [`SLICE_SECONDS`] and the
//!    next one resumes. A pass that blocked for the twenty minutes a hundred-megabyte window takes
//!    over a warehouse link would be a supervisor that had stopped supervising.
//! 2. **One subject per pass.** The fetch has one image; this has a directory that can hold many
//!    windows, so the rule has to be stated rather than inherited: the oldest un-receipted window
//!    goes first, alone. Hashing every sealed file every pass to decide would be the expensive part
//!    of this module, and doing one is what keeps the pass bounded.
//! 3. **The bytes are checked, however they arrived.** Here that is the receiver's digest against
//!    the one recorded at the seal, and the comparison is the only thing that writes a receipt.
//! 4. **Nothing here decides whether a window should exist.** §13 does, at the seal.
//!
//! Not shared, and both are §79.2:
//!
//! * **There is a probe**, because the partial is on the other machine. A download reads its own
//!   disk for free; an upload has to ask, and the answer is a claim rather than evidence.
//! * **A failure discards nothing.** [`crate::bundle`] deletes a partial that will never verify.
//!   This robot has no authority over the receiver's disk, so the strongest thing a bad answer can
//!   produce is *start again at zero*.
//!
//! # Why the receipt is a file, when §75 forbade one
//!
//! [`crate::bundle`]'s second rule is that the partial's length is the progress and a second record
//! of it is one that disagrees exactly once, after the crash that matters. That rule holds for
//! anything the disk can answer. It cannot answer *"does another machine have these bytes"*, and
//! §79.4 is the exception written down: the receipt records **what the far side said**, is checked
//! against the window's own digest before it is believed, and is not a fact about the window.

use std::path::{Path, PathBuf};

use skynet_contracts::upload::{
    check_upload, plan_upload, Held, Reported, UploadDecision, UploadPlan, UploadSubject, Window,
};
use skynet_json::Json;

use crate::bundle::{backoff_ms, hash_file, Attempts};
use crate::layout::Layout;

/// How long one pass may spend moving bytes.
///
/// The same twenty seconds [`crate::bundle::SLICE_SECONDS`] gives a download, and for the same
/// reason: while this runs, nothing serves the local API, nothing pings the watchdog and nothing
/// observes the machine.
pub const SLICE_SECONDS: u32 = 20;

/// How long the probe may take.
///
/// Seconds rather than the slice's twenty, because it is one small request and a probe that hung
/// would spend the whole pass learning nothing. A probe that times out is a failed attempt, which
/// backs off — a robot that retried a dead endpoint every ten seconds for a week would be a robot
/// whose recorder had become its busiest network client.
pub const PROBE_SECONDS: u32 = 10;

/// How many bytes one request may carry when the receiver has not said (§79.2).
///
/// **A slice bounded only by time is a request body bounded only by link speed**, and that is the
/// defect this constant closes. [`SLICE_SECONDS`] bounds the pass, which is a promise to the
/// *supervisor*; it promises the receiver nothing at all. A robot on a warehouse link offering a
/// hundred-megabyte window inside twenty seconds is offering a hundred-megabyte request body, and
/// there is no receiver anywhere that takes one — the Hub this OS is built against runs as
/// serverless functions and refuses a body over four and a half megabytes before any of its own
/// code runs, with a status and no sentence.
///
/// Four mebibytes is therefore a floor rather than a preference: it is what an unknown receiver can
/// be assumed to take. A receiver that can take more says so, in `max_slice_bytes` on its probe
/// answer, and [`Answer::max_slice`] is where that arrives — so this number governs the first
/// transfer to a Hub that has never been asked, and nothing after it.
///
/// The window is not bounded by this. §79 is resumable, and a slice that ends is one the next pass
/// continues past; a *window* refused for being large is evidence lost, which is why the ceiling
/// lives on the request and not on the recording.
pub const SLICE_BYTES: u64 = 4 * 1024 * 1024;

/// The largest slice this robot will send however large a receiver says it can take.
///
/// A receiver's declared limit is a claim from the other end of a link, and this one is used to
/// decide how much of a robot's disk to copy and how much of its bearer to spend in one request.
/// A Hub answering `max_slice_bytes: 10000000000` would otherwise get a slice extraction the size
/// of the disk. Sixty-four mebibytes is well above anything a receiver plausibly wants and well
/// below anything that hurts.
pub const SLICE_BYTES_CEILING: u64 = 64 * 1024 * 1024;

// ── One request ──────────────────────────────────────────────────────────────

/// What one probe should do.
#[derive(Debug, Clone)]
pub struct Probe {
    /// Where the object would live.
    pub url: String,
    /// The device credential, when this robot holds one.
    pub credential: Option<String>,
    /// How long this attempt may take.
    pub budget_s: u32,
}

/// What one push should do.
#[derive(Debug, Clone)]
pub struct Push {
    /// Where the object would live.
    pub url: String,
    /// The device credential, when this robot holds one.
    pub credential: Option<String>,
    /// The offset to send from. Zero starts the object again.
    pub from: u64,
    /// How many bytes to send from there, and never more than the window has left.
    pub len: u64,
    /// The whole window's length, for the `Content-Range` this slice carries.
    pub total: u64,
    /// The sealed window on this robot's disk.
    pub file: PathBuf,
    /// Where a slice may be extracted to, for the length of this request.
    pub scratch: PathBuf,
    /// How long this attempt may take.
    pub budget_s: u32,
}

/// What the far side said, if anything.
///
/// **Every field is optional except the status**, and that is the shape §79.2 asks for: a receiver
/// that answered without saying how much it holds has told this robot nothing it can act on, and
/// the honest representation of that is an absent number rather than a zero. A zero here would mean
/// *"it holds none of it"*, which is a different fact and would restart a transfer that was fine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Answer {
    /// The status line's code, or zero where the request never got one.
    pub status: u16,
    /// How many bytes it says it holds.
    pub bytes: Option<u64>,
    /// What it says they hash to.
    pub digest: Option<String>,
    /// The largest slice it says it will take, when it says.
    ///
    /// §79.2 already makes the *offset* the receiver's to state, for the reason that the partial is
    /// on the receiver's disk. The size of a request is the same kind of fact for the same reason —
    /// it is a property of the machine being written to — and a sender that guesses it is a sender
    /// whose transfers work or do not depending on which Hub it was pointed at.
    ///
    /// Absent is not zero. A receiver that has not said gets [`SLICE_BYTES`], which is the
    /// conservative floor; a receiver that says something absurd is clamped by
    /// [`SLICE_BYTES_CEILING`], because this number decides how much of this robot's disk gets
    /// copied.
    pub max_slice: Option<u64>,
    /// Why the attempt did not finish, when it did not.
    pub error: Option<String>,
    /// The address the request actually ended at, where that is known.
    ///
    /// §79.1 gives the route as a path and a Hub is entitled to canonicalise one — the Hub this
    /// robot pairs to appends a trailing slash and answers `308` to the spelling the specification
    /// prints. Following that is [`curl_probe`]'s job and costs a round trip; doing it **twice** is
    /// what this field prevents. The probe happens on every pass before any bytes move, so it is
    /// already paying for the discovery, and a push that reused the redirected address sends tens
    /// of megabytes once rather than offering them at an address that will answer *"over there"*.
    pub effective_url: Option<String>,
}

/// Something that can ask a receiver what it holds and give it more.
///
/// A trait for [`crate::bundle::Fetcher`]'s reason: the suite has to be able to drive a probe that
/// fails, one that answers with an impossible offset, a push that stops halfway, one that comes
/// back with the wrong digest and one that cannot reach anything — and none of those is reachable
/// through a network a test controls.
pub trait Uploader {
    /// Ask how much of this object the receiver holds.
    fn probe(&mut self, ask: &Probe) -> Answer;
    /// Offer the rest of it.
    fn push(&mut self, ask: &Push) -> Answer;
}

/// The real one: `curl`, for the reason [`crate::connectivity::post_json`] gives.
#[derive(Debug, Default)]
pub struct CurlUploader;

impl Uploader for CurlUploader {
    fn probe(&mut self, ask: &Probe) -> Answer {
        curl_probe(ask)
    }

    fn push(&mut self, ask: &Push) -> Answer {
        curl_push(ask)
    }
}

/// An uploader that refuses, for a robot with no `curl` and for the tests that need one.
#[derive(Debug, Clone)]
pub struct RefusingUploader {
    /// What it says.
    pub reason: String,
}

impl Uploader for RefusingUploader {
    fn probe(&mut self, _ask: &Probe) -> Answer {
        Answer {
            error: Some(self.reason.clone()),
            ..Answer::default()
        }
    }

    fn push(&mut self, _ask: &Push) -> Answer {
        Answer {
            error: Some(self.reason.clone()),
            ..Answer::default()
        }
    }
}

/// A credential this robot will not put in a curl config.
///
/// The rule from [`crate::connectivity`]: refused rather than escaped, because the escape set
/// curl's parser accepts is small and version-dependent.
fn unusable(credential: Option<&str>) -> Option<Answer> {
    let credential = credential?;
    if !credential
        .chars()
        .any(|c| c == '"' || c == '\\' || c.is_control())
    {
        return None;
    }
    Some(Answer {
        error: Some(
            "this robot holds a credential containing a character it will not put in a curl \
             config — a quote, a backslash or a control character. Nothing has been sent."
                .to_string(),
        ),
        ..Answer::default()
    })
}

fn curl_probe(ask: &Probe) -> Answer {
    if let Some(refusal) = unusable(ask.credential.as_deref()) {
        return refusal;
    }
    let mut command = std::process::Command::new("curl");
    command.args(["--silent", "--show-error", "--request", "GET"]);
    // A redirect is an address, not a refusal. See [`Answer::effective_url`] and the same flag on
    // [`crate::bundle`]'s fetch: credentials are dropped on a cross-host redirect because
    // `--location-trusted` is deliberately not used.
    command.args(["--location", "--max-time"]);
    command.arg(ask.budget_s.to_string());
    // A 404 is an answer, not a failure: it is the receiver saying it holds nothing under this
    // digest, which is the ordinary state for a window nobody has sent yet. `--fail` would turn
    // that into an error and a backoff, so the first upload of every window would wait out a curve
    // before it started.
    command.args(["--write-out", "\n%{http_code} %{url_effective}"]);
    if ask.credential.is_some() {
        command.args(["--config", "-"]);
    }
    command.arg(&ask.url);
    interpret(command, ask.credential.as_deref(), &ask.url, "probing")
}

/// Copy `[from, from + len)` of the window into the scratch file, and say how many bytes landed.
///
/// The extraction is the price of a bounded request and it is worth naming what it costs: one
/// slice-sized read and one slice-sized write per pass, on the disk §28 reserves for recordings.
/// The alternative — `--upload-file -` with the bytes fed to `curl`'s stdin — spends no disk and
/// sends a *chunked* body, because a stream has no length; a receiver then has to buffer an unknown
/// quantity before it can place a byte, which is the shape §79.2 is written against.
///
/// The file is truncated rather than appended to and is overwritten every pass. Nothing resumes
/// from it: the resume offset is the receiver's (§79.2), so a slice is worthless the moment its
/// request ends.
pub fn extract(ask: &Push) -> Result<u64, String> {
    use std::io::{Read, Seek, SeekFrom, Write};

    if let Some(parent) = ask.scratch.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("{} could not be created: {error}", parent.display()))?;
    }

    let mut source = std::fs::File::open(&ask.file)
        .map_err(|error| format!("{} could not be opened: {error}", ask.file.display()))?;
    source.seek(SeekFrom::Start(ask.from)).map_err(|error| {
        format!(
            "{} could not be sought to {}: {error}",
            ask.file.display(),
            ask.from
        )
    })?;

    let mut sink = std::fs::File::create(&ask.scratch)
        .map_err(|error| format!("{} could not be written: {error}", ask.scratch.display()))?;

    let mut left = ask.len;
    let mut buffer = [0u8; 64 * 1024];
    let mut written = 0u64;
    while left > 0 {
        let want = std::cmp::min(left as usize, buffer.len());
        let read = source
            .read(&mut buffer[..want])
            .map_err(|error| format!("{} could not be read: {error}", ask.file.display()))?;
        // The window is shorter than the plan believed — a file truncated under this daemon. Sent
        // as what is actually there rather than padded, and the digest is what notices.
        if read == 0 {
            break;
        }
        sink.write_all(&buffer[..read])
            .map_err(|error| format!("{} could not be written: {error}", ask.scratch.display()))?;
        written += read as u64;
        left -= read as u64;
    }
    sink.flush()
        .map_err(|error| format!("{} could not be flushed: {error}", ask.scratch.display()))?;

    Ok(written)
}

fn curl_push(ask: &Push) -> Answer {
    if let Some(refusal) = unusable(ask.credential.as_deref()) {
        return refusal;
    }

    let written = match extract(ask) {
        Ok(0) => {
            return Answer {
                error: Some(format!(
                    "no bytes could be read from {} at offset {}. Nothing was sent.",
                    ask.file.display(),
                    ask.from
                )),
                ..Answer::default()
            }
        }
        Ok(written) => written,
        Err(reason) => {
            return Answer {
                error: Some(reason),
                ..Answer::default()
            }
        }
    };

    let mut command = std::process::Command::new("curl");
    command.args(["--silent", "--show-error", "--fail"]);
    // Present here as well as on the probe, and it should almost never fire: [`advance`] pushes to
    // the address the probe ended at. It is the fallback for the case the probe cannot cover — a
    // Hub that redirects a `PUT` and not a `GET` — and following one costs a re-send of the body,
    // which is why the probe is the mechanism and this is the safety net.
    command.args(["--location", "--max-time"]);
    command.arg(ask.budget_s.to_string());
    command.args([
        "--speed-limit",
        &crate::bundle::STALL_BYTES_PER_SECOND.to_string(),
    ]);
    command.args(["--speed-time", &crate::bundle::STALL_SECONDS.to_string()]);
    command.args(["--write-out", "\n%{http_code} %{url_effective}"]);
    // The header `--continue-at` used to generate, written out rather than delegated.
    //
    // `--continue-at` sets the offset *and* sends the rest of the file, and the second half is what
    // had to go: a slice is bounded now, so the range has an end that is not the window's end and
    // curl has no flag that expresses one on an upload. The spelling is the one curl produces, so
    // the wire is unchanged for a receiver that was already reading it — what changed is that the
    // end of the range is now the end of *this request* rather than the end of the recording.
    //
    // The total is still the window's, because that is what `Content-Range` means. A receiver that
    // ignores the header is caught by the digest rather than trusted, which is the rule that lets
    // this be a header rather than a negotiation.
    command.args([
        "--header",
        &format!(
            "Content-Range: bytes {}-{}/{}",
            ask.from,
            ask.from + written - 1,
            ask.total
        ),
    ]);
    command.arg("--upload-file");
    command.arg(&ask.scratch);
    if ask.credential.is_some() {
        command.args(["--config", "-"]);
    }
    command.arg(&ask.url);
    let answer = interpret(command, ask.credential.as_deref(), &ask.url, "sending");
    // The slice has done its job whether or not the request worked, and a copy of a recording left
    // on the disk §28 reserves for recordings is the one thing this file must not become.
    let _ = std::fs::remove_file(&ask.scratch);
    answer
}

/// Run one curl, and read what came back.
fn interpret(
    mut command: std::process::Command,
    credential: Option<&str>,
    url: &str,
    doing: &str,
) -> Answer {
    use std::io::Write;

    let child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => {
            return Answer {
                error: Some(format!(
                "`curl` could not be started ({error}). §34.2's outbound path is HTTPS and this \
                     edition has no TLS stack of its own, so the window needs curl."
            )),
                ..Answer::default()
            }
        }
    };

    if let Some(stdin) = child.stdin.as_mut() {
        if let Some(credential) = credential {
            // The one line that carries the secret, on the one channel that is not world-readable.
            let _ = stdin
                .write_all(format!("header = \"Authorization: Bearer {credential}\"\n").as_bytes());
        }
    }
    drop(child.stdin.take());

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => {
            return Answer {
                error: Some(format!("curl did not finish: {error}")),
                ..Answer::default()
            }
        }
    };

    let combined = String::from_utf8_lossy(&output.stdout).to_string();
    let (payload, trailer) = match combined.rsplit_once('\n') {
        Some((payload, trailer)) => (payload.to_string(), trailer.trim().to_string()),
        None => (String::new(), combined.trim().to_string()),
    };
    // `%{http_code} %{url_effective}`. Split on the first space rather than the last: a status has
    // no space in it and a URL is percent-encoded, so this is unambiguous in both directions.
    let (status, effective_url) = match trailer.split_once(' ') {
        Some((status, url)) => (status.trim().to_string(), {
            let url = url.trim();
            (!url.is_empty()).then(|| url.to_string())
        }),
        None => (trailer, None),
    };
    let status: u16 = status.parse().unwrap_or(0);

    let error = if output.status.success() {
        None
    } else {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Some(if said.is_empty() {
            format!(
                "curl exited {} {doing} {url}",
                output.status.code().unwrap_or(-1)
            )
        } else {
            format!("curl could not reach {url} while {doing}: {said}")
        })
    };

    // A body that does not parse is not an answer, and it is not a crash either: a captive portal
    // returns HTML with a `200`, and reading `bytes` out of it would be reading a number out of an
    // advertisement. Absent, which is what §79.2 says an unobtained offset is.
    let document = Json::parse(&payload).ok();
    let bytes = document
        .as_ref()
        .and_then(|d| d.get_present("bytes"))
        .and_then(Json::as_f64)
        .filter(|value| *value >= 0.0 && *value <= u64::MAX as f64)
        .filter(|value| (*value as u64) as f64 == *value)
        .map(|value| value as u64);
    let digest = document
        .as_ref()
        .and_then(|d| d.get_present("digest"))
        .and_then(Json::as_str)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    // Read with the same suspicion as `bytes`, and for a sharper reason: this number decides how
    // much of this robot's disk gets copied into a scratch file. A receiver that answers zero, a
    // fraction, or something that does not fit a `u64` has said nothing usable, and the absence is
    // what [`SLICE_BYTES`] is the answer to.
    let max_slice = document
        .as_ref()
        .and_then(|d| d.get_present("max_slice_bytes"))
        .and_then(Json::as_f64)
        .filter(|value| *value >= 1.0 && *value <= u64::MAX as f64)
        .filter(|value| (*value as u64) as f64 == *value)
        .map(|value| value as u64);

    Answer {
        status,
        bytes,
        digest,
        max_slice,
        error,
        effective_url,
    }
}

/// How many bytes to offer this pass (§79.2).
///
/// Three inputs and one rule: never more than the window has left, never more than the receiver
/// says it takes, and never more than [`SLICE_BYTES_CEILING`] whatever it says. A receiver that has
/// not been asked, or that answered with nothing usable, gets [`SLICE_BYTES`] — the floor an
/// unknown receiver can be assumed to accept.
pub fn slice_len(remaining: u64, receiver_says: Option<u64>) -> u64 {
    let allowed = receiver_says
        .unwrap_or(SLICE_BYTES)
        .clamp(1, SLICE_BYTES_CEILING);
    std::cmp::min(remaining, allowed)
}

// ── The receipt (§79.4) ──────────────────────────────────────────────────────

/// What the receiver said it got, written beside the window it is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// `sha256:…` — the window this is a receipt *for*.
    pub digest: String,
    /// How many bytes the receiver said it held.
    pub bytes: u64,
    /// When, on the loop's own clock.
    pub at_ms: u64,
}

impl Receipt {
    /// Read one, or nothing.
    ///
    /// **A receipt naming other bytes is not this window's**, and reads as absent. The sealed
    /// directory can hold a file whose name was reused across a re-provisioning, and a receipt
    /// believed on its filename alone would report a window as collected because a previous one
    /// with the same name was.
    pub fn read(layout: &Layout, recording: &str, digest: &str) -> Option<Receipt> {
        let document = crate::observe::read_json(&layout.receipt(recording))?;
        let held = document.get("digest").and_then(Json::as_str)?;
        if held.trim() != digest.trim() {
            return None;
        }
        Some(Receipt {
            digest: held.trim().to_string(),
            bytes: document
                .get("bytes")
                .and_then(Json::as_f64)
                .unwrap_or(0.0)
                .max(0.0) as u64,
            at_ms: document
                .get("at_ms")
                .and_then(Json::as_f64)
                .unwrap_or(0.0)
                .max(0.0) as u64,
        })
    }

    /// Write it.
    pub fn write(&self, layout: &Layout, recording: &str) -> std::io::Result<()> {
        let mut document = Json::obj();
        document.set("digest", Json::Str(self.digest.clone()));
        document.set("bytes", Json::Num(self.bytes as f64));
        document.set("at_ms", Json::Num(self.at_ms as f64));
        let text = document
            .to_canonical()
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        let path = layout.receipt(recording);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::observe::write_atomic(&path, text.as_bytes())
    }
}

// ── What is on the disk ──────────────────────────────────────────────────────

/// One sealed window, measured.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    /// The file name under `<blackbox>/sealed/`.
    pub name: String,
    /// `sha256:…` over its bytes.
    pub digest: String,
    /// How many there are.
    pub size: u64,
    /// Whether a receipt for these exact bytes is beside it.
    pub collected: bool,
}

/// Every sealed window's name and length, oldest first.
///
/// §13 names a window for the monotonic instant it was sealed at, zero-padded, so a plain sort is
/// oldest first. The oldest goes first because it is the one closest to being lost: a robot that
/// filled its disk would delete from that end, and it is the fault furthest from anybody's memory
/// of what happened.
fn windows(dir: &Path) -> Vec<(String, u64)> {
    let mut found: Vec<(String, u64)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.ends_with(".mcap") {
                return None;
            }
            Some((name, entry.metadata().ok()?.len()))
        })
        .collect();
    found.sort();
    found
}

/// One window's digest, off the manifest the seal wrote, or by hashing it (§79).
///
/// The manifest is the ordinary path and hashing is the fallback, and the split is the difference
/// between a reconcile pass that reads a few hundred bytes and one that reads every sealed byte on
/// the disk. A window sealed by an edition before the manifest existed has none, so it is hashed
/// once — and nothing writes one retrospectively, because a manifest is a record of what the
/// producer computed and a consumer that wrote one would be recording its own arithmetic as the
/// producer's.
fn digest_of(dir: &Path, name: &str, size: u64, trust_manifest: bool) -> Option<String> {
    if trust_manifest {
        if let Some(digest) = crate::blackbox::read_manifest(dir, name, size) {
            return Some(digest);
        }
    }
    hash_file(&dir.join(name)).ok().map(|(digest, _)| digest)
}

/// Every sealed window, oldest first, with its digest and whether it has been collected.
///
/// Reads manifests rather than hashing, so this is cheap enough for a status route. See
/// [`digest_of`] for what happens to a window that has no manifest.
pub fn sealed(layout: &Layout) -> Vec<Sealed> {
    let dir = layout.blackbox().join("sealed");
    windows(&dir)
        .into_iter()
        .filter_map(|(name, size)| {
            let digest = digest_of(&dir, &name, size, true)?;
            let collected = Receipt::read(layout, &name, &digest).is_some();
            Some(Sealed {
                name,
                digest,
                size,
                collected,
            })
        })
        .collect()
}

/// The oldest window nothing has collected, measured — and measured for real.
///
/// The manifest picks the *candidate* and the hash confirms it, which is the one place the
/// distinction matters. A manifest is a record left beside a file name, and §13's names repeat
/// across a reboot; believing one about the bytes this robot is *about to send* would mean
/// uploading a window under another window's digest, which the receiver would store, acknowledge
/// and file against the wrong incident.
///
/// So the scan is cheap and the decision is not: every collected window is skipped on its manifest,
/// and the one window that is about to move is hashed. That is one hash per pass at the very most,
/// and none at all once everything has a receipt.
pub fn next_pending(layout: &Layout) -> Option<Sealed> {
    let dir = layout.blackbox().join("sealed");
    for (name, size) in windows(&dir) {
        // Cheap: does the manifest's digest already have a receipt?
        if let Some(hint) = crate::blackbox::read_manifest(&dir, &name, size) {
            if Receipt::read(layout, &name, &hint).is_some() {
                continue;
            }
        }
        // This one is going to be sent, so measure it rather than trusting a sidecar.
        let Some(digest) = digest_of(&dir, &name, size, false) else {
            continue;
        };
        if Receipt::read(layout, &name, &digest).is_some() {
            continue;
        }
        return Some(Sealed {
            name,
            digest,
            size,
            collected: false,
        });
    }
    None
}

/// The active bearer as the contract wants it (§34.3).
pub fn bearer_of(table: &crate::connectivity::Table) -> Option<skynet_contracts::upload::Bearer> {
    crate::bundle::bearer_of(table)
}

/// Where one window's bytes live on the Hub (§79.1).
///
/// Content-addressed, and the digest goes in the path rather than in a query because it *is* the
/// name of the object. The device id is there because §19 scopes a credential to one robot, so the
/// route a robot may write to is the robot's own.
pub fn url_for(endpoint: &str, device: &str, digest: &str) -> String {
    format!(
        "{}/devices/{}/recordings/{}",
        endpoint.trim_end_matches('/'),
        encode(device.trim()),
        encode(digest.trim()),
    )
}

/// Percent-encode one path component.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'~' | b'-' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

// ── One pass's worth of work ─────────────────────────────────────────────────

/// What a pass did about the window.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Nothing to send: no window, or every window has a receipt.
    Idle {
        /// The sentence.
        detail: String,
    },
    /// The receiver has it, and now so does the disk.
    Collected {
        /// Which window.
        name: String,
        /// The sentence.
        detail: String,
    },
    /// Nothing was done, and nothing is wrong. A metered link, no link at all, or a backoff.
    Waiting {
        /// Which of them.
        detail: String,
    },
    /// Bytes moved and the object is not complete.
    Moved {
        /// Which window.
        name: String,
        /// How many the receiver held when last asked.
        held: Option<u64>,
        /// The sentence.
        detail: String,
    },
    /// Something is wrong and an operator should read it.
    Failed {
        /// The sentence.
        detail: String,
    },
}

impl Step {
    /// The sentence, whichever it is.
    pub fn detail(&self) -> &str {
        match self {
            Step::Idle { detail }
            | Step::Waiting { detail }
            | Step::Failed { detail }
            | Step::Collected { detail, .. }
            | Step::Moved { detail, .. } => detail,
        }
    }
}

/// Send what can be sent of the oldest uncollected window (§79, §34.3).
///
/// One pass, one window. The decision comes from the contract, the bytes go through the uploader,
/// and the verdict on what the receiver ended up with comes from the contract again — this function
/// is the plumbing between them and deliberately makes no judgement of its own.
#[allow(clippy::too_many_arguments)]
pub fn advance(
    layout: &Layout,
    uploader: &mut dyn Uploader,
    table: &crate::connectivity::Table,
    endpoint: &str,
    device: &str,
    credential: Option<&str>,
    now_ms: u64,
    jitter: f64,
) -> Step {
    let Some(window) = next_pending(layout) else {
        return Step::Idle {
            detail:
                "Every sealed window on this robot has a receipt, or there are none. §13 seals \
                     on a fault, an estop or an operator request, and none of those has happened \
                     since the last one was collected."
                    .to_string(),
        };
    };

    let subject = UploadSubject {
        digest: window.digest.clone(),
        size: window.size,
        held: None,
        receipt: false,
        bearer: bearer_of(table),
    };
    let plan = plan_upload(&subject);
    match plan.decision {
        UploadDecision::Done => {
            // Unreachable through `next_pending`, which filters on the receipt this branch is
            // about. Handled rather than asserted, because the alternative is a panic in a
            // supervisor for a state that would be *correct*.
            return Step::Collected {
                name: window.name,
                detail: plan.detail,
            };
        }
        UploadDecision::Refuse => {
            return Step::Failed {
                detail: plan.detail,
            }
        }
        UploadDecision::NoBearer | UploadDecision::WaitForUnmetered => {
            return Step::Waiting {
                detail: plan.detail,
            }
        }
        _ => {}
    }

    // The backoff, read after the decision rather than before it: a robot waiting for an unmetered
    // link is not backing off, and reporting it as one would hide §34.3's wait behind a timer
    // nobody asked about. Keyed to the digest, so a new window does not inherit an old one's curve.
    let record = layout.upload();
    let mut attempts = Attempts::read_at(&record, &window.digest);
    if now_ms < attempts.next_ms {
        return Step::Waiting {
            detail: format!(
                "The last attempt failed and the next is held for another {} s (§34.4's curve). {}",
                (attempts.next_ms - now_ms).div_ceil(1_000),
                attempts.error.clone().unwrap_or_default()
            ),
        };
    }
    attempts.digest = window.digest.clone();

    let url = url_for(endpoint, device, &window.digest);

    // §79.2's probe. Always, and before any bytes: the offset is the receiver's and this robot
    // cannot read it off its own disk.
    let probed = uploader.probe(&Probe {
        url: url.clone(),
        credential: credential.map(str::to_string),
        budget_s: PROBE_SECONDS,
    });
    // §79.1: a receiver holding no object under this digest answers `404`, and that is a
    // **measured zero** rather than an absence — it was asked and it answered. The rule lives here
    // rather than in [`CurlUploader`] because it is a property of the route, not of the transport:
    // an uploader written against a different client would otherwise have to rediscover it, and the
    // one that did not would back off for ever on the first upload of every window.
    let held_bytes = match (probed.bytes, probed.status) {
        (Some(bytes), _) => Some(bytes),
        (None, 404) => Some(0),
        (None, _) => None,
    };
    let Some(held_bytes) = held_bytes else {
        return fail(
            layout,
            attempts,
            now_ms,
            jitter,
            probed.error.unwrap_or_else(|| {
                format!(
                    "the receiver answered {} without saying how much of this window it holds. \
                     Nothing is sent: pushing from zero whenever a probe fails would re-send the \
                     whole window every pass (§79.2).",
                    probed.status
                )
            }),
        );
    };

    let placed = plan_upload(&UploadSubject {
        held: Some(Held {
            bytes: held_bytes,
            digest: probed.digest.clone(),
        }),
        ..subject
    });
    if placed.decision == UploadDecision::Done {
        return collect(layout, &window, held_bytes, now_ms, &record, placed);
    }

    let answer = uploader.push(&Push {
        // The address the probe ended at, which is the same one unless a Hub canonicalises the
        // path. Reusing it is what keeps a redirect from costing a re-send of the window: the probe
        // is a round trip this pass has already made, so the discovery is free and the push is not.
        url: probed.effective_url.clone().unwrap_or(url),
        credential: credential.map(str::to_string),
        from: placed.from,
        // Bounded by what the receiver says it takes, which is a fact about the receiver in exactly
        // the way §79.2's offset is. A slice bounded only by `SLICE_SECONDS` is a request body
        // bounded only by link speed, and no receiver accepts one of those.
        len: slice_len(window.size.saturating_sub(placed.from), probed.max_slice),
        total: window.size,
        file: layout.blackbox().join("sealed").join(&window.name),
        scratch: layout.upload_slice(),
        budget_s: SLICE_SECONDS,
    });

    // A receiver that did not report is not a failure by itself — the budget ends a slice on a
    // large window, and the next pass's probe is what finds out how far it got. That is the probe
    // earning its place a second time.
    let Some(reported) = answer.bytes else {
        if let Some(error) = answer.error {
            return fail(layout, attempts, now_ms, jitter, error);
        }
        attempts.failures = 0;
        attempts.next_ms = 0;
        attempts.error = None;
        let _ = attempts.write_at(&record);
        return Step::Moved {
            name: window.name,
            held: None,
            detail: format!(
                "A {SLICE_SECONDS}s slice ended without the receiver saying what it now holds. The \
                 next pass asks before it sends, so nothing is assumed and nothing is re-sent \
                 blindly."
            ),
        };
    };

    let check = check_upload(
        &Reported {
            bytes: reported,
            digest: answer.digest.clone(),
        },
        &Window {
            digest: window.digest.clone(),
            size: window.size,
        },
    );

    if check.ok {
        return collect(
            layout,
            &window,
            reported,
            now_ms,
            &record,
            UploadPlan {
                decision: UploadDecision::Done,
                from: 0,
                detail: check.detail,
            },
        );
    }

    if check.restart {
        return fail(layout, attempts, now_ms, jitter, check.detail);
    }

    // Short. Progress, and the ordinary way a slice of a large window ends.
    attempts.failures = 0;
    attempts.next_ms = 0;
    attempts.error = answer.error.clone();
    let _ = attempts.write_at(&record);
    Step::Moved {
        name: window.name,
        held: Some(reported),
        detail: check.detail,
    }
}

/// Write the receipt and clear the curve.
fn collect(
    layout: &Layout,
    window: &Sealed,
    bytes: u64,
    now_ms: u64,
    record: &Path,
    plan: UploadPlan,
) -> Step {
    let receipt = Receipt {
        digest: window.digest.clone(),
        bytes,
        at_ms: now_ms,
    };
    if let Err(error) = receipt.write(layout, &window.name) {
        // The bytes are over there and this robot cannot write down that they are. Reported as a
        // failure rather than a success, because the next pass will send the whole window again —
        // which is the honest consequence and not something to hide behind a cheerful line.
        return Step::Failed {
            detail: format!(
                "{} arrived, and the receipt could not be written ({error}). The next pass will \
                 offer it again; the receiver will say it already has it and no bytes will move \
                 (§79.2), but this disk cannot answer the question without asking.",
                window.name
            ),
        };
    }
    Attempts::clear_at(record);
    Step::Collected {
        name: window.name.clone(),
        detail: plan.detail,
    }
}

/// Record a failed attempt and back off (§34.4's curve).
fn fail(layout: &Layout, mut attempts: Attempts, now_ms: u64, jitter: f64, error: String) -> Step {
    attempts.failures = attempts.failures.saturating_add(1);
    attempts.error = Some(error.clone());
    attempts.next_ms = now_ms.saturating_add(backoff_ms(attempts.failures, jitter));
    let _ = attempts.write_at(&layout.upload());
    Step::Failed { detail: error }
}

/// What the console and `skynet blackbox` show about collection (§79).
///
/// One shape whether or not anything has been collected, for §77.2's reason: a consumer that had to
/// switch on the presence of a key would get it wrong on the robot where it matters.
pub fn report(layout: &Layout) -> Json {
    let windows = sealed(layout);
    let collected = windows.iter().filter(|window| window.collected).count();
    let bytes_pending: u64 = windows
        .iter()
        .filter(|window| !window.collected)
        .map(|window| window.size)
        .sum();

    let mut out = Json::obj();
    out.set("sealed", Json::Num(windows.len() as f64));
    out.set("collected", Json::Num(collected as f64));
    out.set("pending", Json::Num((windows.len() - collected) as f64));
    out.set("pending_bytes", Json::Num(bytes_pending as f64));

    let mut list = Json::arr();
    for window in &windows {
        let mut entry = Json::obj();
        entry.set("name", Json::Str(window.name.clone()));
        entry.set("digest", Json::Str(window.digest.clone()));
        entry.set("size_bytes", Json::Num(window.size as f64));
        entry.set("collected", Json::Bool(window.collected));
        list.push(entry);
    }
    out.set("windows", list);

    // The last attempt, when there was one. Absent rather than null: a robot that has never tried
    // has not failed, and a null `error` would read as an attempt that ended cleanly.
    if let Some(document) = crate::observe::read_json(&layout.upload()) {
        out.set("last_attempt", document);
    }
    out
}

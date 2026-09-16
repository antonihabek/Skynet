//! §12's bytes: fetching an OS image, checking it is the image, and keeping the loop turning while
//! it arrives.
//!
//! [`skynet_contracts::bundle`] decides *whether* to transfer, from where in the file, and whether
//! what arrived is the release — and it takes no network, no clock and no filesystem. This is the
//! other half: the request, the partial on the disk, the digest over what came back, and the one
//! rename that turns a download into a bundle.
//!
//! For twenty-five sessions `stage_image` reported `Unimplemented` and said why: *"§34's inbound
//! fetcher is not written; nothing here can go and get it."* The manifest arrives, the gate runs,
//! the refusals are correct — and an image had to be put on the robot by hand. This closes that.
//!
//! # Four rules, and none of them is about HTTP
//!
//! 1. **A slice, not a transfer.** §33.2's loop is one thread: it reconciles, serves the local API
//!    and pings the watchdog, in that order, for ever. A pass that blocked for the twenty minutes a
//!    hundred-megabyte image takes over a warehouse link would be a supervisor that stopped
//!    supervising — which is the reason `report_drain` is bounded per pass, and it is the same
//!    reason here. Each pass moves what it can inside [`SLICE_SECONDS`] and the next one resumes.
//!    Resume is what makes that free: a bounded attempt loses nothing.
//! 2. **The partial's length is the progress.** No counter, no percentage written anywhere. §34.4's
//!    queue keeps no index for the same reason — a second record of what the directory already says
//!    is a record that disagrees exactly once, after the crash that matters. [`Layout::transfer`]
//!    holds only what the disk cannot say: how the last attempt ended, and when the next may start.
//! 3. **The bytes are checked before they are installed, however they arrived.** This is the hole
//!    this module found rather than filled: `stage_image` passed whatever file the manifest named
//!    straight to RAUC, so a bundle placed on the disk by hand — the *only* way to update a robot
//!    before today — was installed without anything hashing it. Every gate §12 applies is bound to
//!    the digest, and none of them was checking that the bytes were the ones the digest names.
//! 4. **Nothing here decides whether the image may be applied.** [`crate::ota::assess`] does, before
//!    a byte moves. Fetching an image this robot will refuse would be a robot paying for a transfer
//!    to reach a refusal it could have reached for nothing — and §12 is explicit that the refusal is
//!    cheapest at staging.
//!
//! # Why the location is not in the signed manifest
//!
//! The manifest names a digest, not a URL, and this module derives the URL from the endpoint §10
//! paired this robot to. That is deliberate: **a location is not evidence.** The bytes are checked
//! against a digest inside a document a publisher signed, so where they came from does not need to
//! be trusted — a hostile mirror serving the wrong image produces a digest mismatch and a deleted
//! file. Putting a URL in the manifest would sign a fact that does not need signing, and would make
//! a release manifest go stale the day a hostname changed.
//!
//! It also keeps [`crate::releases::adopt`]'s decision intact: the Hub's row carries an artifact id
//! and this robot copies five fields out of it, precisely so no future edition starts depending on
//! the Hub's storage layout.

use std::io::Read;
use std::path::{Path, PathBuf};

use skynet_contracts::bundle::{
    check_bundle, plan_fetch, Arrived, Bearer, BundleCheck, Expected, FetchDecision, FetchPlan,
    FetchSubject,
};
use skynet_json::Json;

use crate::layout::Layout;

/// How long one pass may spend moving bytes.
///
/// Twenty seconds, and the number is a property of §33.2's single thread rather than of any link:
/// while this runs, nothing serves the local API, nothing pings the watchdog and nothing observes
/// the machine. `curl`'s other timeouts here are shaped the same way — a stalled link is abandoned
/// long before the budget rather than holding the loop for the whole of it.
pub const SLICE_SECONDS: u32 = 20;

/// A transfer moving slower than this many bytes per second for [`STALL_SECONDS`] is abandoned.
///
/// Not a quality bar. It is the difference between a slice that ends because the budget ran out —
/// having moved something — and one that spent twenty seconds holding a socket that a captive
/// portal accepted and never answered.
pub const STALL_BYTES_PER_SECOND: u32 = 1_024;

/// How long a transfer may sit below [`STALL_BYTES_PER_SECOND`] before it is abandoned.
pub const STALL_SECONDS: u32 = 8;

/// The first backoff after a failed attempt, doubled per attempt to [`MAX_BACKOFF_MS`].
pub const FIRST_BACKOFF_MS: u64 = 30_000;

/// The cap on backoff between attempts: fifteen minutes, as §34.4 gives for the outbound queue.
pub const MAX_BACKOFF_MS: u64 = 900_000;

// ── One attempt ──────────────────────────────────────────────────────────────

/// What one request for bytes should do.
#[derive(Debug, Clone)]
pub struct Ask {
    /// Where the bytes are.
    pub url: String,
    /// The device credential, when this robot holds one.
    pub credential: Option<String>,
    /// The offset to resume from. Zero starts the file again.
    pub from: u64,
    /// The partial to write into.
    pub into: PathBuf,
    /// How long this attempt may take.
    pub budget_s: u32,
}

/// What one attempt did.
///
/// **A short slice is not a failure.** A transfer that ran out of budget having moved forty
/// megabytes is exactly what this design asks for, so the distinction the caller needs is *did the
/// file grow* rather than *did the process exit zero*.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Slice {
    /// The status line's code, or zero where the request never got one.
    pub status: u16,
    /// The file's length before the attempt.
    pub before: u64,
    /// The file's length after it.
    pub after: u64,
    /// The whole artifact's size as the *server* described it, when it described one. Untrusted:
    /// used for a progress line and never for a decision.
    pub declared_total: Option<u64>,
    /// Whether the response body was received in full.
    pub complete: bool,
    /// Why the attempt did not finish, when it did not. Present alongside progress: a slice can
    /// both move bytes and end badly.
    pub error: Option<String>,
}

/// Something that can move bytes into a file.
///
/// A trait for [`crate::ota::Updater`]'s reason: the suite has to be able to drive a transfer that
/// stops halfway, one that resumes, one that returns the wrong bytes and one that cannot reach
/// anything — and none of those is reachable through a network a test controls.
pub trait Fetcher {
    /// Move what can be moved, and say what happened.
    fn slice(&mut self, ask: &Ask) -> Slice;
}

/// The real one: `curl`, for the reason [`crate::connectivity::post_json`] gives.
///
/// §34.2's transport is HTTPS, this workspace has no third-party dependencies, and a hand-rolled
/// TLS stack for a security boundary would be the worst option on the table.
#[derive(Debug, Default)]
pub struct CurlFetcher;

impl Fetcher for CurlFetcher {
    fn slice(&mut self, ask: &Ask) -> Slice {
        curl_slice(ask)
    }
}

/// A fetcher that refuses, for a robot with no `curl` and for the tests that need one.
#[derive(Debug, Clone)]
pub struct RefusingFetcher {
    /// What it says.
    pub reason: String,
}

impl Fetcher for RefusingFetcher {
    fn slice(&mut self, ask: &Ask) -> Slice {
        Slice {
            before: size_of(&ask.into),
            after: size_of(&ask.into),
            error: Some(self.reason.clone()),
            ..Slice::default()
        }
    }
}

/// One request, run.
fn curl_slice(ask: &Ask) -> Slice {
    let before = size_of(&ask.into);

    // Refused before the directory is made, so a credential this robot will not present leaves no
    // trace. The rule and its sentence are `connectivity`'s, and so is the process: nothing about
    // the credential is ever an argument, because `/proc/<pid>/cmdline` is world-readable.
    if let Err(refusal) = crate::connectivity::presentable(ask.credential.as_deref()) {
        return Slice {
            before,
            after: before,
            error: Some(refusal),
            ..Slice::default()
        };
    }

    if let Some(parent) = ask.into.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            return Slice {
                before,
                after: before,
                error: Some(format!(
                    "{} could not be created: {error}",
                    parent.display()
                )),
                ..Slice::default()
            };
        }
    }

    let mut arguments = crate::connectivity::curl_arguments([
        "--silent",
        "--show-error",
        // Without this, an HTTP error page is a body, and a body is written into the partial —
        // which is how a 404 becomes forty bytes of HTML appended to an operating system.
        "--fail",
        // The bytes may be behind a redirect to object storage, which is where an artifact of this
        // size belongs. Credentials are *not* carried across hosts: curl drops them on a cross-host
        // redirect unless told otherwise, and this deliberately does not tell it otherwise.
        "--location",
        "--max-time",
    ]);
    arguments.push(ask.budget_s.to_string().into());
    arguments.push("--speed-limit".into());
    arguments.push(STALL_BYTES_PER_SECOND.to_string().into());
    arguments.push("--speed-time".into());
    arguments.push(STALL_SECONDS.to_string().into());
    // The body goes to the file, so stdout carries only these. `content_length_download` is what
    // remains of *this* response, so the whole artifact is that plus the offset — which is right for
    // a `206` and for a `200` alike.
    arguments.push("--write-out".into());
    arguments.push("%{http_code} %{size_download} %{content_length_download}".into());
    if ask.from > 0 {
        // `--continue-at` sets the range *and* appends, which is the pair of behaviours a resume
        // needs. A server that ignores the range is caught below rather than trusted.
        arguments.push("--continue-at".into());
        arguments.push(ask.from.to_string().into());
    }
    arguments.push("--output".into());
    arguments.push(ask.into.clone().into_os_string());

    let output = match crate::connectivity::curl(&arguments, ask.credential.as_deref(), &ask.url) {
        Ok(output) => output,
        Err(error) => {
            return Slice {
                before,
                after: size_of(&ask.into),
                error: Some(error),
                ..Slice::default()
            }
        }
    };

    let after = size_of(&ask.into);
    let fields = String::from_utf8_lossy(&output.stdout).to_string();
    let mut parts = fields.split_whitespace();
    let status = parts.next().and_then(|f| f.parse().ok()).unwrap_or(0);
    let _transferred = parts.next().and_then(|f| f.parse::<f64>().ok());
    let remaining = parts
        .next()
        .and_then(|f| f.parse::<f64>().ok())
        .filter(|value| *value >= 0.0)
        .map(|value| value as u64);
    let declared_total = remaining.map(|remaining| ask.from.saturating_add(remaining));

    let code = output.status.code().unwrap_or(-1);
    let complete = output.status.success();
    // 28 is `CURLE_OPERATION_TIMEDOUT`; the budget above is the ordinary way a slice ends on a large
    // image, and calling that a failure would put an error on the console every twenty seconds of a
    // transfer that is working perfectly.
    let error = if complete || code == 28 {
        None
    } else {
        let said = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Some(if said.is_empty() {
            format!("curl exited {code} fetching {}", ask.url)
        } else {
            format!("curl could not fetch {}: {said}", ask.url)
        })
    };

    Slice {
        status,
        before,
        after,
        declared_total,
        complete,
        error,
    }
}

/// A file's length, or zero where there is no file.
///
/// Zero rather than an error, and it is the honest answer: [`plan_fetch`] asks how many bytes of a
/// partial are here, and "none, because there is no file" and "none, because the file is empty" are
/// the same fact to it.
pub fn size_of(path: &Path) -> u64 {
    std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

// ── What is on the disk ──────────────────────────────────────────────────────

/// `sha256:…` over a file, and its length, without reading it into memory.
///
/// Streaming rather than `fs::read`, because the subject is an operating system image: a robot with
/// 2 GiB of RAM (§28's floor) hashing a 400 MB file by loading it would be a robot that runs out of
/// memory doing the one thing §12 requires before it installs anything.
pub fn hash_file(path: &Path) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = skynet_sha256::Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total = total.saturating_add(read as u64);
    }
    let digest = skynet_sha256::hex_of(&hasher.finish());
    Ok((
        format!("sha256:{}", String::from_utf8_lossy(&digest)),
        total,
    ))
}

/// The facts [`plan_fetch`] needs, read from this robot.
///
/// `size` is the manifest's declared size and is `None` on a manifest that declares none — which is
/// every manifest a Hub serves today. The contract's *"a partial cannot be larger than what it is
/// part of"* branch is therefore unexercised here, and that is a gap in the manifest rather than in
/// this function: the Hub holds `size_bytes` on the release row and the five-field manifest does not
/// carry it. Recorded rather than papered over with the server's own claim, which would put an
/// untrusted number into a decision about deleting a file.
pub fn subject(
    layout: &Layout,
    digest: &str,
    size: Option<u64>,
    bearer: Option<Bearer>,
) -> FetchSubject {
    FetchSubject {
        digest: digest.to_string(),
        size,
        have: layout.bundle(digest).is_file(),
        partial: size_of(&layout.partial(digest)),
        bearer,
    }
}

/// The active bearer as the contract wants it (§34.3).
pub fn bearer_of(table: &crate::connectivity::Table) -> Option<Bearer> {
    table.active().map(|bearer| Bearer {
        name: bearer.name.clone(),
        metered: bearer.metered(),
        metering_declared: bearer.metering_is_declared(),
    })
}

/// What one image's transfer looks like right now (§12, §34.3).
pub fn plan(
    layout: &Layout,
    digest: &str,
    size: Option<u64>,
    table: &crate::connectivity::Table,
) -> FetchPlan {
    plan_fetch(&subject(layout, digest, size, bearer_of(table)))
}

// ── The record of the last attempt ───────────────────────────────────────────

/// What the last attempt did, and when the next may start.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Attempts {
    /// The digest this record is about. A record for other bytes is not this transfer's.
    pub digest: String,
    /// How many attempts have failed in a row. Reset by any attempt that moved a byte.
    pub failures: u32,
    /// When the next attempt may start, on the same clock the reconcile loop uses.
    pub next_ms: u64,
    /// How the last attempt ended, when it ended badly.
    pub error: Option<String>,
    /// The whole artifact's size as the server last described it. For a progress line only.
    pub declared_total: Option<u64>,
}

impl Attempts {
    /// Read the record, or an empty one.
    ///
    /// A record naming other bytes is discarded rather than adapted: it is the backoff of a
    /// different transfer, and inheriting it would make a fresh image wait out a failed one's curve.
    pub fn read(layout: &Layout, digest: &str) -> Attempts {
        Attempts::read_at(&layout.transfer(), digest)
    }

    /// Write it.
    pub fn write(&self, layout: &Layout) -> std::io::Result<()> {
        self.write_at(&layout.transfer())
    }

    /// Forget everything about this transfer.
    pub fn clear(layout: &Layout) {
        Attempts::clear_at(&layout.transfer());
    }

    /// The same three, against any record.
    ///
    /// §79's upload keeps its own, at [`Layout::upload`], and the curve, the reset rule and the
    /// "a record for other bytes is not this transfer's" rule are identical in both directions —
    /// so they are one implementation. What is *not* shared is the file: an image's backoff and a
    /// recording's backoff are two independent transfers, and one record would make a failed
    /// upload delay an OS update.
    pub fn read_at(path: &Path, digest: &str) -> Attempts {
        let Some(document) = crate::observe::read_json(path) else {
            return Attempts::default();
        };
        let held = document.get("digest").and_then(Json::as_str).unwrap_or("");
        if held != digest {
            return Attempts::default();
        }
        Attempts {
            digest: digest.to_string(),
            failures: document
                .get("failures")
                .and_then(Json::as_f64)
                .unwrap_or(0.0)
                .max(0.0) as u32,
            next_ms: document
                .get("next_ms")
                .and_then(Json::as_f64)
                .unwrap_or(0.0)
                .max(0.0) as u64,
            error: document
                .get_present("error")
                .and_then(Json::as_str)
                .filter(|text| !text.is_empty())
                .map(str::to_string),
            declared_total: document
                .get_present("declared_total")
                .and_then(Json::as_f64)
                .filter(|value| *value >= 0.0)
                .map(|value| value as u64),
        }
    }

    /// Write it, against any record.
    pub fn write_at(&self, path: &Path) -> std::io::Result<()> {
        let mut document = Json::obj();
        document.set("digest", Json::Str(self.digest.clone()));
        document.set("failures", Json::Num(self.failures as f64));
        document.set("next_ms", Json::Num(self.next_ms as f64));
        // Absent is not null (§31.4's rule, applied to a record): a transfer that has not failed has
        // no error, and writing `null` would say one was recorded and was empty.
        if let Some(error) = &self.error {
            document.set("error", Json::Str(error.clone()));
        }
        if let Some(total) = self.declared_total {
            document.set("declared_total", Json::Num(total as f64));
        }
        let text = document
            .to_canonical()
            .map_err(|error| std::io::Error::other(format!("{error:?}")))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::observe::write_atomic(path, text.as_bytes())
    }

    /// Forget everything about it, against any record.
    pub fn clear_at(path: &Path) {
        let _ = std::fs::remove_file(path);
    }
}

/// The delay after `failures` consecutive failures, with jitter, capped (§34.4's curve).
///
/// The same shape as [`crate::outbound::backoff_ms`] and a longer first step, because the two are
/// paced by different things: a check-in that fails costs one request, and a fetch that fails may
/// have cost a hundred megabytes of somebody's cellular allowance before it did.
///
/// **Full jitter**, for §34.4's reason and one that is sharper here: a staged rollout tells a canary
/// ring to fetch the same image at the same moment, so the fleet arrives at one endpoint in lockstep
/// *by design*. `jitter` is supplied rather than drawn (§35.2).
pub fn backoff_ms(failures: u32, jitter: f64) -> u64 {
    if failures == 0 {
        return 0;
    }
    let exponential = FIRST_BACKOFF_MS
        .saturating_mul(1u64 << (failures - 1).min(20))
        .min(MAX_BACKOFF_MS);
    (exponential as f64 * jitter.clamp(0.0, 1.0)) as u64
}

// ── One pass's worth of work ─────────────────────────────────────────────────

/// What a pass did about the bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// The bundle is here and has been checked against the manifest.
    Have {
        /// Where it is.
        path: PathBuf,
        /// The sentence.
        detail: String,
    },
    /// Nothing was done, and nothing is wrong. A metered link, no link at all, or a backoff.
    Waiting {
        /// Which of them.
        detail: String,
    },
    /// Bytes moved and the transfer is not finished.
    Moved {
        /// How many are here now.
        received: u64,
        /// How many there are in total, as the server described it.
        total: Option<u64>,
        /// The sentence.
        detail: String,
    },
    /// Something is wrong and an operator should read it.
    Failed {
        /// What.
        detail: String,
    },
}

/// Fetch what can be fetched of one image, and check it if it is all here (§12, §34.3).
///
/// One pass. The decision comes from the contract, the bytes come from the fetcher, and the verdict
/// on what arrived comes from the contract again — this function is the plumbing between them and
/// deliberately makes no judgement of its own.
#[allow(clippy::too_many_arguments)]
pub fn advance(
    layout: &Layout,
    fetcher: &mut dyn Fetcher,
    digest: &str,
    size: Option<u64>,
    table: &crate::connectivity::Table,
    url: &str,
    credential: Option<&str>,
    now_ms: u64,
    jitter: f64,
) -> Step {
    let subject = subject(layout, digest, size, bearer_of(table));
    let plan = plan_fetch(&subject);

    match plan.decision {
        FetchDecision::Have => {
            Attempts::clear(layout);
            return Step::Have {
                path: layout.bundle(digest),
                detail: plan.detail,
            };
        }
        FetchDecision::Refuse => {
            return Step::Failed {
                detail: plan.detail,
            }
        }
        FetchDecision::NoBearer | FetchDecision::WaitForUnmetered => {
            return Step::Waiting {
                detail: plan.detail,
            }
        }
        FetchDecision::Fetch | FetchDecision::Resume | FetchDecision::DiscardAndFetch => {}
    }

    // The backoff, read after the decision rather than before it: a robot that is waiting for an
    // unmetered link is not backing off, and reporting it as one would hide §34.3's wait behind a
    // timer nobody asked about.
    let mut attempts = Attempts::read(layout, digest);
    if now_ms < attempts.next_ms {
        return Step::Waiting {
            detail: format!(
                "The last attempt failed and the next is held for another {} s (§34.4's curve). \
                 {}",
                (attempts.next_ms - now_ms).div_ceil(1_000),
                attempts.error.clone().unwrap_or_default()
            ),
        };
    }

    let partial = layout.partial(digest);
    if plan.decision == FetchDecision::DiscardAndFetch {
        let _ = std::fs::remove_file(&partial);
    }

    let slice = fetcher.slice(&Ask {
        url: url.to_string(),
        credential: credential.map(str::to_string),
        from: plan.from,
        into: partial.clone(),
        budget_s: SLICE_SECONDS,
    });

    // A server that answered `200` to a resume sent the whole file and curl appended it to what was
    // already here. The bytes on the disk are now one image's head followed by the whole of
    // something; the digest would catch it after the transfer, and catching it here saves the rest
    // of a transfer that cannot succeed.
    if plan.from > 0 && slice.status == 200 {
        let _ = std::fs::remove_file(&partial);
        attempts.digest = digest.to_string();
        attempts.failures = attempts.failures.saturating_add(1);
        attempts.error = Some(
            "the server ignored the range on a resume and sent the whole file, which curl appended \
             to the partial. The partial is discarded and the next attempt starts from the \
             beginning."
                .to_string(),
        );
        attempts.next_ms = now_ms.saturating_add(backoff_ms(attempts.failures, jitter));
        let _ = attempts.write(layout);
        return Step::Failed {
            detail: attempts.error.clone().unwrap_or_default(),
        };
    }

    let moved = slice.after.saturating_sub(slice.before);
    attempts.digest = digest.to_string();
    attempts.declared_total = slice.declared_total.or(attempts.declared_total);

    if let Some(error) = &slice.error {
        // Progress and a failure are not exclusive, and the distinction decides the backoff: an
        // attempt that moved forty megabytes and then lost the link has not failed in the sense the
        // curve is about, and doubling a delay for it would make a poor link exponentially poorer.
        if moved > 0 {
            attempts.failures = 0;
            attempts.next_ms = 0;
            attempts.error = Some(error.clone());
            let _ = attempts.write(layout);
            return Step::Moved {
                received: slice.after,
                total: attempts.declared_total,
                detail: format!(
                    "{} bytes are here{}. The link went before the transfer finished ({error}), and \
                     the next pass resumes.",
                    slice.after,
                    match attempts.declared_total {
                        Some(total) => format!(" of the {total} the server declared"),
                        None => String::new(),
                    }
                ),
            };
        }
        attempts.failures = attempts.failures.saturating_add(1);
        attempts.error = Some(error.clone());
        attempts.next_ms = now_ms.saturating_add(backoff_ms(attempts.failures, jitter));
        let _ = attempts.write(layout);
        return Step::Failed {
            detail: error.clone(),
        };
    }

    if !slice.complete {
        // The budget ended the slice. The ordinary case on a large image, and the one this whole
        // design exists for.
        attempts.failures = 0;
        attempts.next_ms = 0;
        attempts.error = None;
        let _ = attempts.write(layout);
        return Step::Moved {
            received: slice.after,
            total: attempts.declared_total,
            detail: format!(
                "{} bytes are here{} after a {SLICE_SECONDS}s slice. The next pass resumes: a \
                 reconcile loop that spent twenty minutes on one download would be a supervisor \
                 that had stopped supervising.",
                slice.after,
                match attempts.declared_total {
                    Some(total) => format!(" of the {total} the server declared"),
                    None => String::new(),
                }
            ),
        };
    }

    // The body arrived in full. Whether it is the *image* is the contract's question, not this
    // function's.
    finish(layout, digest, size, now_ms, jitter, attempts)
}

/// Check a completed download and, if it is the release, make it the bundle.
///
/// Separated from [`advance`] so the suite can drive the four verdicts over files it wrote itself,
/// which is the only way to reach `long` and `digest_mismatch` without a hostile server.
pub fn finish(
    layout: &Layout,
    digest: &str,
    size: Option<u64>,
    now_ms: u64,
    jitter: f64,
    mut attempts: Attempts,
) -> Step {
    let partial = layout.partial(digest);
    let (arrived_digest, arrived_size) = match hash_file(&partial) {
        Ok(pair) => pair,
        Err(error) => {
            return Step::Failed {
                detail: format!("{} could not be read back: {error}", partial.display()),
            }
        }
    };

    let check = check_bundle(
        &Arrived {
            digest: arrived_digest,
            size: arrived_size,
        },
        &Expected {
            digest: digest.to_string(),
            size,
        },
    );

    if check.ok {
        let destination = layout.bundle(digest);
        if let Some(parent) = destination.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                return Step::Failed {
                    detail: format!("{} could not be created: {error}", parent.display()),
                };
            }
        }
        if let Err(error) = std::fs::rename(&partial, &destination) {
            return Step::Failed {
                detail: format!(
                    "{} is the release and could not be moved to {}: {error}",
                    partial.display(),
                    destination.display()
                ),
            };
        }
        Attempts::clear(layout);
        return Step::Have {
            path: destination,
            detail: check.detail,
        };
    }

    if check.discard {
        let _ = std::fs::remove_file(&partial);
        attempts.digest = digest.to_string();
        attempts.failures = attempts.failures.saturating_add(1);
        attempts.error = Some(check.detail.clone());
        attempts.next_ms = now_ms.saturating_add(backoff_ms(attempts.failures, jitter));
        let _ = attempts.write(layout);
        return Step::Failed {
            detail: check.detail,
        };
    }

    // Short, and kept: a resume is exactly what a partial is for.
    attempts.digest = digest.to_string();
    attempts.failures = 0;
    attempts.next_ms = 0;
    attempts.error = None;
    let _ = attempts.write(layout);
    Step::Moved {
        received: arrived_size,
        total: attempts.declared_total,
        detail: check.detail,
    }
}

/// Check the bytes that are about to be installed, whatever put them there (§12).
///
/// The gate `stage_image` did not have. Every rule §12 applies — the signature over the manifest,
/// the bench pass, the compatibility list — is bound to a digest, and until this function existed
/// nothing compared that digest against the bytes RAUC was handed. A bundle placed on the disk by
/// hand, which was the *only* way to update a robot before this module, went in unchecked.
///
/// The cost is one pass over the file at staging time, which is the moment §12 already calls the
/// cheapest place for a refusal.
pub fn verify_file(path: &Path, digest: &str, size: Option<u64>) -> Result<BundleCheck, String> {
    let (arrived_digest, arrived_size) = hash_file(path)
        .map_err(|error| format!("{} could not be read: {error}", path.display()))?;
    Ok(check_bundle(
        &Arrived {
            digest: arrived_digest,
            size: arrived_size,
        },
        &Expected {
            digest: digest.to_string(),
            size,
        },
    ))
}

/// Where this robot would ask for one image's bytes.
///
/// `{endpoint}/os/releases/{version}/bundle`, beside the catalogue §57 already serves. The version
/// is percent-encoded for [`crate::releases`]'s reason — it is a string from a document, not a
/// path component — and the architecture goes on the query so a Hub serving several can answer
/// without this robot having to say which artifact it means.
pub fn url_for(endpoint: &str, version: &str) -> String {
    format!(
        "{}/os/releases/{}/bundle?arch={}",
        endpoint.trim_end_matches('/'),
        encode(version.trim()),
        encode(crate::releases::arch()),
    )
}

/// Percent-encode one path or query component.
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

/// The transfer, as `GET /ota` reports it.
///
/// Computed from the disk on every read rather than remembered: the partial's length **is** the
/// progress, and a route that reported a number from somewhere else would eventually report one the
/// bytes disagree with.
pub fn to_json(
    layout: &Layout,
    digest: &str,
    size: Option<u64>,
    table: &crate::connectivity::Table,
) -> Json {
    let subject = subject(layout, digest, size, bearer_of(table));
    let plan = plan_fetch(&subject);
    let attempts = Attempts::read(layout, digest);
    let total = size.or(attempts.declared_total);

    let mut out = Json::obj();
    out.set("digest", Json::Str(digest.to_string()));
    out.set("decision", Json::Str(plan.decision.as_str().to_string()));
    out.set("detail", Json::Str(plan.detail.clone()));
    out.set("have", Json::Bool(subject.have));
    out.set("received", Json::Num(subject.partial as f64));
    // Absent, not null, and not zero: a robot that has not been told how big the image is has no
    // total, and a `0` there would render as a completed transfer of nothing.
    if let Some(total) = total {
        out.set("total", Json::Num(total as f64));
        // Whose number it is, because they are different kinds of statement: a size in the manifest
        // is a publisher's, inside a document somebody signed, and a size from the server is the
        // party serving the bytes describing its own file.
        out.set(
            "total_declared_by",
            Json::Str(if size.is_some() { "manifest" } else { "server" }.to_string()),
        );
    }
    out.set("failures", Json::Num(attempts.failures as f64));
    if let Some(error) = &attempts.error {
        out.set("error", Json::Str(error.clone()));
    }
    if attempts.next_ms > 0 {
        out.set("next_attempt_ms", Json::Num(attempts.next_ms as f64));
    }
    out
}

//! Starting a remote-policy package (§90.2).
//!
//! A remote-policy package is a `policy` whose manifest declares
//! `policy_source: { kind: "hosted", endpoint_id }` and a serving deadline. It carries no weights
//! and runs **none of the publisher's code on the robot**, so it is not started from its own entry
//! point. The daemon starts the policy host instead — `skynetp run --backend remote` — and the
//! policy host starts the session bridge beside it, so the reconciler supervises one unit per
//! package, as it does for every other package, and stopping the unit stops both.
//!
//! Every setting reaches the policy host through the unit's environment, for §25's reason: nothing
//! here edits a command line. The deadline is the package's, from its signed manifest; the bound
//! and the resume rule are the operator profile's, from `active.json`, which holds only a profile
//! whose signature verified; the device credential arrives as a **path**, so it is on no command
//! line and in no process listing.
//!
//! # A remote policy that cannot run is refused by name
//!
//! Admission has already refused a remote policy with no deadline (`deadline_undeclared`). The
//! check is repeated here, on the manifest that is about to run, because a daemon that started
//! the policy host and let it refuse would have announced a commanding process first. And a robot
//! with no session bridge installed is refused before anything starts: a remote policy with no way
//! to open a session is a robot that will never move under it, and the reason belongs in the
//! reconcile report rather than in a journal nobody is reading.

use std::path::{Path, PathBuf};

use skynet_json::Json;

use crate::layout::Layout;

/// The programs a remote policy runs, resolved once when the daemon starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePrograms {
    /// `skynetp`: `SKYNET_POLICY_HOST`, or the binary beside this daemon.
    pub policy_host: PathBuf,
    /// The session bridge: `SKYNET_REMOTE_BRIDGE_COMMAND`, or `skynet-remote-bridge` beside this
    /// daemon or on `PATH` — which is where `pip install skynet-sdk[hosted]` puts it. `None` when
    /// none is installed.
    pub bridge: Option<PathBuf>,
}

/// The name of the session bridge's program (the SDK's console script).
pub const BRIDGE_PROGRAM: &str = "skynet-remote-bridge";

impl RemotePrograms {
    /// Resolve both from the environment this daemon was started with.
    pub fn resolve() -> RemotePrograms {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let policy_host = std::env::var_os("SKYNET_POLICY_HOST")
            .map(PathBuf::from)
            .or_else(|| beside.as_ref().map(|dir| dir.join("skynetp")))
            .unwrap_or_else(|| PathBuf::from("skynetp"));
        let bridge = std::env::var_os("SKYNET_REMOTE_BRIDGE_COMMAND")
            .map(PathBuf::from)
            .or_else(|| {
                beside
                    .as_ref()
                    .map(|dir| dir.join(BRIDGE_PROGRAM))
                    .filter(|path| path.is_file())
            })
            .or_else(|| on_path(BRIDGE_PROGRAM));
        RemotePrograms {
            policy_host,
            bridge,
        }
    }
}

fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// What a remote-policy manifest declares.
#[derive(Debug, Clone, PartialEq)]
pub struct RemotePolicy {
    /// The Hub endpoint the session opens on.
    pub endpoint_id: String,
    /// The package's deadline, from `serving.max_chunk_age_ms`.
    pub max_chunk_age_ms: f64,
}

/// Whether a staged manifest is a remote-policy package, and if it is, what it declares.
///
/// `None` for every package whose policy infers on the robot, which is started from its own entry
/// point as it always was. `Some(Err)` for one that declares a hosted source this cannot run: the
/// sentence names the field.
pub fn remote_policy(manifest: &Json) -> Option<Result<RemotePolicy, String>> {
    let source = manifest.get_present("policy_source")?;
    match source.get("kind").and_then(Json::as_str) {
        Some("local") => return None,
        Some("hosted") => {}
        _ => {
            return Some(Err(
                "policy_source_invalid — policy_source must be { kind: \"local\" } or { kind: \
                 \"hosted\", endpoint_id } (§90.2)"
                    .to_string(),
            ))
        }
    }
    let endpoint_id = match source.get("endpoint_id").and_then(Json::as_str) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => {
            return Some(Err(
                "policy_source_invalid — a hosted policy source names no endpoint (§90.2)"
                    .to_string(),
            ))
        }
    };
    let deadline = manifest
        .get("serving")
        .and_then(|serving| serving.get("max_chunk_age_ms"))
        .and_then(Json::as_f64);
    match deadline {
        Some(ms) if ms.is_finite() && ms > 0.0 => Some(Ok(RemotePolicy {
            endpoint_id,
            max_chunk_age_ms: ms,
        })),
        _ => Some(Err(
            "deadline_undeclared — a remote policy declares serving.max_chunk_age_ms, the oldest \
             an action may be when it runs (§90.2)"
                .to_string(),
        )),
    }
}

/// The operator profile's two settings (§90.2), from the profile in force.
///
/// `(None, None)` when no profile is in force or it makes neither, which leaves the policy host at
/// the package's deadline and at `acknowledge`.
pub fn profile_settings(layout: &Layout) -> (Option<f64>, Option<&'static str>) {
    let Some(profile) = crate::observe::read_json(&layout.active_profile()) else {
        return (None, None);
    };
    let Some(remote) = profile.get_present("remote") else {
        return (None, None);
    };
    let bound = remote.get("max_chunk_age_ms").and_then(Json::as_f64);
    let resume = match remote.get("resume").and_then(Json::as_str) {
        Some("automatic") => Some("automatic"),
        Some("acknowledge") => Some("acknowledge"),
        _ => None,
    };
    (bound, resume)
}

/// The Hub's host for the session client: the paired endpoint without the API prefix the robot
/// keeps it with.
pub fn sdk_host(endpoint: &str) -> String {
    let trimmed = endpoint.trim_end_matches('/');
    trimmed
        .strip_suffix("/api/skynet/v1")
        .unwrap_or(trimmed)
        .to_string()
}

/// Everything the policy host needs for one remote-policy package, beyond §25's rendezvous.
pub fn environment(
    layout: &Layout,
    id: &str,
    policy: &RemotePolicy,
    bridge: &Path,
) -> Vec<(String, String)> {
    let mut env = vec![
        (
            "SKYNET_REMOTE_BRIDGE".to_string(),
            layout.remote_bridge_socket(id).display().to_string(),
        ),
        (
            "SKYNET_REMOTE_MAX_CHUNK_AGE_MS".to_string(),
            number(policy.max_chunk_age_ms),
        ),
        (
            "SKYNET_REMOTE_ENDPOINT".to_string(),
            policy.endpoint_id.clone(),
        ),
        (
            "SKYNET_REMOTE_BRIDGE_COMMAND".to_string(),
            bridge.display().to_string(),
        ),
        (
            "SKYNET_REMOTE_RESUME_FILE".to_string(),
            layout.remote_resume(id).display().to_string(),
        ),
        // §90.2's and §74's queue: which session proposed what, and how often the deadline fired,
        // carried by this daemon into the recording and the telemetry window.
        (
            "SKYNET_REMOTE_PROVENANCE".to_string(),
            layout.remote_provenance(id).display().to_string(),
        ),
        // A path, never the credential: the bridge reads the file, so nothing secret is on a
        // command line or in the unit's environment block.
        (
            "SKYNET_TOKEN_FILE".to_string(),
            layout.credential().display().to_string(),
        ),
    ];
    if let Some(endpoint) = crate::identity::read(layout).endpoint {
        env.push(("SKYNET_HOST".to_string(), sdk_host(&endpoint)));
    }
    let (bound, resume) = profile_settings(layout);
    if let Some(bound) = bound {
        env.push(("SKYNET_PROFILE_MAX_CHUNK_AGE_MS".to_string(), number(bound)));
    }
    if let Some(resume) = resume {
        env.push(("SKYNET_REMOTE_RESUME".to_string(), resume.to_string()));
    }
    env
}

fn number(value: f64) -> String {
    skynet_json::format_number(value).unwrap_or_else(|_| value.to_string())
}

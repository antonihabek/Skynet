//! The on-disk layout (§28), and the two identifiers that become path components.
//!
//! ```text
//! /data
//!   /packages
//!     /blobs/<digest>/…                unpacked package content, shared by digest
//!     /active/<ns>/<name>/current      symlink -> ../../../blobs/<digest>
//!     /active/<ns>/<name>/prev.1       the version a rollback returns to
//!     /active/<ns>/<name>/prev.2       and the one before that
//!   /blackbox                          preallocated ring buffer
//!   /state                             desired state, grants, operator profiles
//!   /identity                          device key
//! /run/skynetd.sock                    the local API
//! ```
//!
//! Everything is rooted rather than absolute. A [`Layout`] over a temporary directory is the same
//! program as one over `/`, which is what lets the store be tested against real files instead of
//! against a fake filesystem that agrees with whatever the store believes.
//!
//! # The rollback set is the set of links, and nothing else
//!
//! §37.2 pins "the running version and the previous N" and §37.3 says GC never touches them, so
//! something has to know what "previous" was. It could have been a history file. It is not, and the
//! reason is the same one §33.3 gives for observation: a record kept beside the thing it describes
//! is a record that can disagree with it, and the disagreement is discovered by an operator whose
//! rollback landed somewhere unexpected.
//!
//! So the rollback set is **literally the symlinks**. `prev.1` and `prev.2` are links like
//! `current`, GC's rule is "collect a blob nothing points at", and rollback is "read `prev.1`, make
//! it current". There is no second copy of the truth to drift.
//!
//! The links live in a directory named for the package rather than beside it as `name.prev.1`,
//! because a package legitimately named `grasp.prev.1` would otherwise be a package whose blobs are
//! deleted when `grasp` is upgraded.
//!
//! # Two identifiers that are also path components, which makes them a security boundary
//!
//! A desired state arrives over the network and names packages and digests. Both become path
//! components under `/data`, so [`PackageId::parse`] and [`parse_digest`] are the check that stops
//! `../../etc/systemd/system` from being a package id. They are strict on purpose — one spelling
//! per value, nothing normalised into legality — because a validator that repairs its input is a
//! validator whose callers stop believing the value was ever checked.

use std::path::{Path, PathBuf};

/// A package identifier: `namespace/name`.
///
/// Held parsed rather than as a string, so a path can only be built from one that was checked.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PackageId {
    namespace: String,
    name: String,
}

/// Why an identifier was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdError {
    /// The value as given.
    pub value: String,
    /// What is wrong with it.
    pub detail: &'static str,
}

impl std::fmt::Display for IdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "`{}` is not a package id: {}", self.value, self.detail)
    }
}

impl std::error::Error for IdError {}

impl PackageId {
    /// Parse `namespace/name`.
    ///
    /// The grammar is deliberately narrow: lowercase ASCII letters and digits, then any of
    /// `. _ -`, in exactly two segments. It is narrower than the Hub's namespace rules would
    /// strictly require, and that asymmetry is the right direction — a robot refusing an id the
    /// Hub would have accepted is a package that does not install, while the reverse is a path.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        let bad = |detail: &'static str| {
            Err(IdError {
                value: value.to_string(),
                detail,
            })
        };

        if value.len() > 128 {
            return bad("longer than 128 bytes");
        }
        let mut parts = value.split('/');
        let (Some(namespace), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
            return bad("not exactly two segments separated by one `/`");
        };
        for segment in [namespace, name] {
            if segment.is_empty() {
                return bad("has an empty segment");
            }
            if segment.len() > 64 {
                return bad("has a segment longer than 64 bytes");
            }
            let mut chars = segment.chars();
            let first = chars.next().unwrap_or('/');
            if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
                return bad("has a segment that does not begin with a lowercase letter or digit");
            }
            if !segment.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            }) {
                return bad(
                    "has a segment with a character outside `a-z0-9._-`; an id is a path component \
                     and anything else is a way out of the package store",
                );
            }
            // `.` and `..` are already excluded by the first-character rule, but a segment that is
            // only dots and dashes is still nothing anybody meant to type.
            if segment.chars().all(|c| matches!(c, '.' | '_' | '-')) {
                return bad("has a segment made only of punctuation");
            }
        }
        Ok(Self {
            namespace: namespace.to_string(),
            name: name.to_string(),
        })
    }

    /// The namespace half.
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    /// The name half.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `namespace/name`, the spelling everything else refers to it by.
    pub fn as_str(&self) -> String {
        format!("{}/{}", self.namespace, self.name)
    }
}

impl std::fmt::Display for PackageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.namespace, self.name)
    }
}

/// Accept a digest, in the one spelling the Hub writes.
///
/// Bare lowercase hex, 64 characters — `computeVersionDigest` returns exactly that and
/// `skynet_repo_versions.digest` stores exactly that. No `sha256:` prefix is accepted, and that is
/// not pedantry: a digest is compared as a string in a dozen places on both sides of the link, and
/// a second accepted spelling is a comparison that answers "different" about identical bytes.
pub fn parse_digest(value: &str) -> Result<&str, IdError> {
    if skynet_contracts::digest::is_hex_sha256(value) {
        Ok(value)
    } else {
        Err(IdError {
            value: value.to_string(),
            detail:
                "not 64 lowercase hex characters. The Hub writes a version digest bare, with no \
                     `sha256:` prefix, and one spelling is the whole point",
        })
    }
}

/// Where everything lives, rooted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Default for Layout {
    /// The layout on a robot.
    fn default() -> Self {
        Self::new("/")
    }
}

impl Layout {
    /// A layout rooted anywhere.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `/data`.
    pub fn data(&self) -> PathBuf {
        self.root.join("data")
    }

    /// `/data/packages`.
    pub fn packages(&self) -> PathBuf {
        self.data().join("packages")
    }

    /// `/data/packages/blobs`.
    pub fn blobs(&self) -> PathBuf {
        self.packages().join("blobs")
    }

    /// `/data/packages/blobs/<digest>`.
    pub fn blob(&self, digest: &str) -> PathBuf {
        self.blobs().join(digest)
    }

    /// `/data/packages/active`.
    pub fn active_root(&self) -> PathBuf {
        self.packages().join("active")
    }

    /// `/data/packages/active/<ns>/<name>`, the directory holding this package's links.
    pub fn links(&self, id: &PackageId) -> PathBuf {
        self.active_root().join(&id.namespace).join(&id.name)
    }

    /// The `current` link.
    pub fn current_link(&self, id: &PackageId) -> PathBuf {
        self.links(id).join("current")
    }

    /// A `prev.N` link. `depth` is 1 for the version a rollback returns to.
    pub fn prev_link(&self, id: &PackageId, depth: usize) -> PathBuf {
        self.links(id).join(format!("prev.{depth}"))
    }

    /// What a link at `active/<ns>/<name>/` must point at to reach a blob.
    ///
    /// Relative, so the store survives being mounted somewhere else — in a test root, in a
    /// recovery image, under a different `/data`. An absolute target would make every link in the
    /// store wrong the first time anybody moved it.
    pub fn link_target(&self, digest: &str) -> PathBuf {
        Path::new("..")
            .join("..")
            .join("..")
            .join("blobs")
            .join(digest)
    }

    /// `/data/state`.
    pub fn state(&self) -> PathBuf {
        self.data().join("state")
    }

    /// The desired state as last fetched (§11).
    pub fn desired(&self) -> PathBuf {
        self.state().join("desired.json")
    }

    /// The last desired state that decoded, which §33.8's ladder falls back to.
    pub fn desired_last_good(&self) -> PathBuf {
        self.state().join("desired.last-good.json")
    }

    /// Capability grants in force.
    pub fn grants(&self) -> PathBuf {
        self.state().join("grants.json")
    }

    /// `/data/state/profiles` (§38.3).
    pub fn profiles(&self) -> PathBuf {
        self.state().join("profiles")
    }

    /// The operator profile in force.
    pub fn active_profile(&self) -> PathBuf {
        self.profiles().join("active.json")
    }

    /// Where an operator declares the robot is at a safe state (§33.4).
    ///
    /// A file rather than a flag, because the barrier is re-evaluated every pass and a value fixed
    /// at start-up would mean a robot that reached a safe state had to be restarted to notice. The
    /// file holds `yes` or `no`; absent is `no`, because deferring is the direction that reduces
    /// what the robot does.
    pub fn safe_state(&self) -> PathBuf {
        self.state().join("safe-state")
    }

    /// Local credentials, hashed (§39.2).
    pub fn credentials(&self) -> PathBuf {
        self.state().join("credentials.json")
    }

    /// The keys this robot trusts (§19, §37.1).
    ///
    /// Beside the credentials, and the parallel is exact: `credentials.json` answers *who may tell
    /// this robot what to do*, and this answers *whose signatures it should believe*. Both are
    /// written by an operator with physical possession of the machine (§39.2), and neither has a
    /// default — a robot nobody has provisioned trusts nobody, which is the useless-rather-than-open
    /// failure §39.2 chooses on purpose.
    pub fn trust(&self) -> PathBuf {
        self.state().join("trust.json")
    }

    /// The outbound queue (§34.4).
    ///
    /// Under `state/` rather than under `/var/spool` or beside the black box: it is state the robot
    /// carries across a reboot, and §34.4's "survives reboot" is the whole point of it being a
    /// directory rather than a channel.
    pub fn outbound(&self) -> PathBuf {
        self.state().join("outbound")
    }

    /// Where calibration records live (§45.3).
    ///
    /// A directory rather than one file, because each parameter set is measured by a different
    /// procedure at a different time and expires under a different policy — one document would make
    /// re-measuring the camera extrinsics a rewrite of the joint zeros, and a torn write during a
    /// calibration would take every set with it.
    pub fn calibration(&self) -> PathBuf {
        self.state().join("calibration")
    }

    /// The append-only record of configuration changes (§38.3).
    ///
    /// Beside the things it is a record of, and not under `/data/blackbox`: the ring wraps and this
    /// does not. A key trusted eleven months ago is out of the black box's window by a factor of a
    /// hundred thousand and still in this file, which is the whole reason §38.3 asks for both.
    pub fn audit(&self) -> PathBuf {
        self.state().join("audit.jsonl")
    }

    /// How far the daemon has mirrored the audit log into the black box.
    ///
    /// Separate from the log because the log is written by whoever makes a change — often a CLI
    /// process — and this is written only by `skynetd`. One file, one writer.
    pub fn audit_cursor(&self) -> PathBuf {
        self.state().join("audit.cursor")
    }

    /// The embodiment descriptor for this body (§5).
    ///
    /// Beside the profiles rather than under them, because it is not one. A profile is what an
    /// operator may narrow (§38.2); the descriptor is what the vendor says the machine *is*, and
    /// the layering rule has nothing to say about a joint count.
    ///
    /// One file and one body. A robot is one machine, and a directory here would invite a second
    /// descriptor whose relationship to the first nothing defines — which of two bodies the kernel
    /// is guarding is not a question this system should ever have to answer at run time.
    pub fn embodiment(&self) -> PathBuf {
        self.state().join("embodiment.json")
    }

    /// The vendor's detached signature over the descriptor's digest (§5).
    ///
    /// A **sibling file**, and it has to be. The digest is taken over the descriptor's canonical
    /// form, so a signature written inside the document would change the digest it is a signature
    /// of — and a scheme that excluded one member from the hash would mean two robots disagreeing
    /// about which bytes were signed the first time somebody added a field beside it.
    pub fn embodiment_signature(&self) -> PathBuf {
        self.state().join("embodiment.sig.json")
    }

    /// `/data/blackbox` (§28) — preallocated, so a full disk cannot silence it.
    pub fn blackbox(&self) -> PathBuf {
        self.data().join("blackbox")
    }

    /// `/data/identity` (§10).
    pub fn identity(&self) -> PathBuf {
        self.data().join("identity")
    }

    /// `/run/skynetd.sock` (§33.7).
    pub fn socket(&self) -> PathBuf {
        self.root.join("run").join("skynetd.sock")
    }
}

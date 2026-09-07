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

    /// What an operator has declared about this machine's power hardware (§36).
    ///
    /// One field, and the file exists because of it: a machine with two batteries cannot be
    /// resolved by measurement, and `crates/skynetd-io/src/power.rs` will not guess which is the
    /// traction pack. Absent on every robot with one battery, which is most of them.
    pub fn power(&self) -> PathBuf {
        self.state().join("power.json")
    }

    /// An operator's declaration that a control loop is being run by hand (§80).
    ///
    /// The second file in `state/` that exists because a daemon refuses to guess, and it is the
    /// same shape as [`Layout::power`] one layer up: `power.json` names which of two batteries is
    /// the traction pack, and this names how wide a bench loop's action space is. Neither is
    /// measurable from here, and both have a wrong answer that looks plausible.
    ///
    /// §80 left this open as *"a loop nothing declared"*: `skynetk` run against a driver names no
    /// package, so `shape::supervising` sizes the recording events-only and the loop is refused at
    /// `Recorder::open`. That was the right refusal and the wrong conclusion to stop at — the
    /// daemon must not **invent** a loop, and an operator saying *I am about to run one this wide*
    /// is not the daemon inventing anything. It is §34.3's rule, which this repository applies to
    /// bearers, to the traction pack and to `brownout_fault_codes`: the parts a machine can
    /// measure are measured, the parts it cannot are declared, and every surface says which is
    /// which.
    ///
    /// Absent on every robot that is not a bench, which is every robot in the field.
    pub fn bench(&self) -> PathBuf {
        self.state().join("bench.json")
    }

    /// Where the control loop offers §25's rings, and a policy connects to take them.
    ///
    /// In `/run` rather than `/data` because it is a rendezvous rather than state: it exists while
    /// something is listening and means nothing across a reboot. `skynetk --policy-socket` binds it
    /// and `skynetd` passes it to the one package that may write a command ring, which is the whole
    /// of how the two halves of §25 find each other on a robot where neither starts the other
    /// (§33.1: the kernel is started by systemd **before** `skynetd`).
    pub fn policy_socket(&self) -> PathBuf {
        self.root.join("run").join("policy.sock")
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

    /// Cached entitlement tokens (§51.2).
    ///
    /// A directory, unlike the descriptor's single file, because §51.2 grants one token per package
    /// and a robot runs several. One file per package, named by the package's own identifier with
    /// its slash replaced, so an operator can see what the robot believes it is entitled to by
    /// listing a directory.
    ///
    /// Under `state/` rather than beside the packages: §57 delivers these *"with the desired
    /// state"*, they are refreshed on a check-in the packages are not, and a token whose package
    /// has been removed is a fact worth keeping until it expires rather than deleting with the
    /// package that prompted it.
    pub fn entitlements(&self) -> PathBuf {
        self.state().join("entitlements")
    }

    /// The cached token for one package, and its detached signature beside it.
    ///
    /// Sibling files, for exactly the reason [`embodiment_signature`](Layout::embodiment_signature)
    /// gives: the digest is over the token's canonical form, so a signature written inside it would
    /// change the digest it is a signature of.
    pub fn entitlement(&self, package: &str) -> PathBuf {
        self.entitlements()
            .join(format!("{}.json", flatten(package)))
    }

    /// The issuer's detached signature over one token's digest (§51.2).
    pub fn entitlement_signature(&self, package: &str) -> PathBuf {
        self.entitlements()
            .join(format!("{}.sig.json", flatten(package)))
    }

    /// `/data/state/os` (§12) — what this robot has been told about OS releases.
    ///
    /// Under `state/` rather than under `/boot`, and the distinction is §28's: `/boot` and the two
    /// rootfs slots belong to the bootloader and RAUC, and this directory holds the *documents* —
    /// the release manifests a check-in cached and the bench passes that gate them. A robot with no
    /// network still has to be able to answer "may this image be applied", so those documents live
    /// where `/data` lives, which survives an update.
    pub fn os(&self) -> PathBuf {
        self.state().join("os")
    }

    /// The cached release manifest for one version (§57's `GET /os/releases`).
    ///
    /// One file per version, named by the version, so an operator can list the directory and see
    /// what this robot believes it may install. A version is not a path component the way a package
    /// id is not — see [`release_name`] — so it is flattened by the same rule.
    pub fn release(&self, version: &str) -> PathBuf {
        self.os()
            .join("releases")
            .join(format!("{}.json", flatten(version)))
    }

    /// The publisher's detached signature over one release manifest's digest (§12).
    pub fn release_signature(&self, version: &str) -> PathBuf {
        self.os()
            .join("releases")
            .join(format!("{}.sig.json", flatten(version)))
    }

    /// A bench pass, keyed by the **image digest** it was taken against (§12, §15).
    ///
    /// By digest and not by version, because that is what §12's third rule binds evidence to: a
    /// pass for `1.4.0` is meaningless if `1.4.0` was rebuilt, and the digest is the only name that
    /// cannot be reused.
    pub fn bench_pass(&self, image_digest: &str) -> PathBuf {
        self.os()
            .join("bench")
            .join(format!("{}.json", flatten(image_digest)))
    }

    /// The detached signature over a bench pass (§12).
    ///
    /// Signed for the reason the operator profile is (§38.1): this document decides whether an
    /// update touching the Safety Kernel may be applied, so an unsigned one on a robot's disk is
    /// exactly the attacker §38.1 names — somebody editing safety-relevant configuration locally —
    /// arriving through the one gate §12 calls the most important in the suite.
    pub fn bench_pass_signature(&self, image_digest: &str) -> PathBuf {
        self.os()
            .join("bench")
            .join(format!("{}.sig.json", flatten(image_digest)))
    }

    /// Where a complete, checked bundle lives, keyed by the **digest** rather than the version.
    ///
    /// By digest for [`Self::bench_pass`]'s reason and one more. §12 binds every gate it applies to
    /// the digest, so a file named for a version would let a rebuilt `1.4.0` be installed against a
    /// bench pass taken on the first cut of it — the exact substitution the whole section exists to
    /// refuse. It also means two channels naming the same bytes share one file rather than two.
    pub fn bundle(&self, digest: &str) -> PathBuf {
        self.os()
            .join("bundles")
            .join(format!("{}.raucb", flatten(digest)))
    }

    /// A transfer in progress, keyed to the digest it is a transfer **of** (§12, §34.3).
    ///
    /// The key is the rule rather than the naming convention: a partial keyed by version would let a
    /// resume append one release's tail to another's head, and the mismatch would surface after the
    /// whole transfer rather than before it. Separate from [`Self::bundle`] so that nothing can find
    /// a half-downloaded image and treat it as one — a directory listing distinguishes them.
    pub fn partial(&self, digest: &str) -> PathBuf {
        self.os()
            .join("partial")
            .join(format!("{}.part", flatten(digest)))
    }

    /// What the last transfer attempt did (§12).
    ///
    /// Deliberately small, and it holds only what the disk cannot say: how the last attempt ended,
    /// how many have failed in a row, and when the next may start. Progress is **not** here — the
    /// partial file's own length is the progress, and a second record of it is one that disagrees
    /// with the bytes exactly once, after the crash that matters.
    pub fn transfer(&self) -> PathBuf {
        self.os().join("transfer.json")
    }

    /// The slot this robot applied and has not yet confirmed (§12's second rule).
    ///
    /// A file rather than memory, because the whole point is that it survives the reboot the apply
    /// causes: the daemon that writes it is not the daemon that reads it.
    pub fn pending_boot(&self) -> PathBuf {
        self.os().join("pending.json")
    }

    /// `/data/blackbox` (§28) — preallocated, so a full disk cannot silence it.
    pub fn blackbox(&self) -> PathBuf {
        self.data().join("blackbox")
    }

    /// What the last upload attempt did (§79).
    ///
    /// The same shape as [`Layout::transfer`] and beside the black box rather than beside the OS,
    /// because the subject is a recording. It holds what the disk cannot say about an *attempt*;
    /// what the far side holds is not written here at all, because that is not a fact about this
    /// machine — it is asked for every pass (§79.2).
    pub fn upload(&self) -> PathBuf {
        self.blackbox().join("upload.json")
    }

    /// One slice of a window, extracted for the length of one request (§79.2).
    ///
    /// A file rather than a pipe, because `curl` uploads a *file* and needs a length: an upload
    /// from stdin is chunked, and a receiver that has to buffer an unknown length is the receiver
    /// this whole path exists not to need.
    ///
    /// One path rather than one per window, and it is overwritten every pass. The alternative
    /// keyed the slice to the digest, which reads as more careful and is not: a directory of
    /// abandoned slices on a disk §28 reserves for recordings would cost the black box the space
    /// it was preallocated to have, to hold copies of bytes that are already on the same disk.
    /// Nothing resumes from it — the resume offset comes from the receiver (§79.2), so a slice is
    /// worthless the moment its request ends.
    pub fn upload_slice(&self) -> PathBuf {
        self.blackbox().join("upload.slice")
    }

    /// The receipt for one sealed window, beside the window (§79.4).
    ///
    /// Named from the recording rather than from the digest, so that `ls` in the sealed directory
    /// answers "which of these has been collected" without anything having to hash a file. The
    /// digest is *inside* it, and it is what makes the receipt believable: a receipt naming other
    /// bytes is not this window's.
    pub fn receipt(&self, recording: &str) -> PathBuf {
        self.blackbox()
            .join("sealed")
            .join(format!("{recording}.receipt.json"))
    }

    /// `/data/identity` (§10).
    pub fn identity(&self) -> PathBuf {
        self.data().join("identity")
    }

    /// What pairing bound this robot to (§10).
    ///
    /// Under `identity/` beside the birth certificate, and the pair is the whole of §50.2's
    /// distinction on one directory listing: `birth.json` is what the machine **is**, signed by a
    /// vendor before it had an owner, and `device.json` is whose it **is**, written by an operator
    /// who had one. A robot that lost the first cannot obtain another; a robot that lost the second
    /// pairs again in a minute.
    ///
    /// The credential is deliberately **not** in it — see [`Layout::credential`].
    pub fn device(&self) -> PathBuf {
        self.identity().join("device.json")
    }

    /// The device credential itself (§10), and nothing else.
    ///
    /// A separate file from [`Layout::device`] for one reason: `device.json` is read by every
    /// surface this daemon has — the console renders it, `GET /identity` returns it, the check-in
    /// quotes it — and a document with a bearer credential inside it is a document none of those
    /// may show. Splitting them is what lets the identity be public and the secret be read by the
    /// one caller that presents it.
    ///
    /// No extension, because it is not a document. One line, the credential, and nothing around it.
    pub fn credential(&self) -> PathBuf {
        self.identity().join("credential")
    }

    /// The vendor's birth certificate (§50.2).
    ///
    /// Under `identity/` rather than under `state/`, and the directory is the argument: everything
    /// in `state/` is something the robot fetched, was granted or measured, and can be re-fetched.
    /// A birth certificate is none of those. It was signed at manufacture, before the machine had an
    /// owner, and a robot that lost it cannot obtain another without going back to the vendor.
    pub fn birth_certificate(&self) -> PathBuf {
        self.identity().join("birth.json")
    }

    /// The vendor's detached signature over the certificate's digest (§50.2).
    ///
    /// A sibling file, for the third time in this file and for the same reason each time: the digest
    /// is over the document's canonical form, so a signature written inside it would change the
    /// digest it is a signature of.
    pub fn birth_signature(&self) -> PathBuf {
        self.identity().join("birth.sig.json")
    }

    /// `/run/skynetd.sock` (§33.7).
    pub fn socket(&self) -> PathBuf {
        self.root.join("run").join("skynetd.sock")
    }
}

/// A package identifier as one path component.
///
/// A package is `namespace/name`, and a slash in a file name is a directory somebody did not intend
/// to create — or, from an untrusted desired-state document, a path traversal. Every character
/// outside a small allowed set becomes `_`, which is deliberately **lossy**: two packages can
/// flatten to one name, and that is a collision the caller sees as a token belonging to the wrong
/// package rather than as a write outside `state/`.
///
/// Lossy is safe here only because the token names its own package and the reader checks it — see
/// [`entitlement::read`](crate::entitlement::read), which refuses a token whose `package` field is
/// not the one it was asked for. Without that check this function would be a vulnerability rather
/// than a convenience.
///
/// **`.` is not in the allowed set**, and the reason is worth the line it costs. The first draft
/// kept it, on the grounds that a dot is harmless in a file name — and a package named `..` then
/// flattened to `..`, which is the parent directory. Nothing escaped, because both callers append
/// `.json` or `.sig.json` and `"...json"` is an ordinary name. That is a safety property resting
/// on a suffix, one refactor away from not being one, and a package identifier has no use for a
/// dot anyway.
fn flatten(package: &str) -> String {
    package
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

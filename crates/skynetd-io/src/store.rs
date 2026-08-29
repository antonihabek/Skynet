//! The package store — §37, the whole lifecycle.
//!
//! ```text
//! resolve   the desired state names id, version and digest — already resolved, §37.4
//! fetch     by sha256, skipping any blob already present
//! verify    digest, then the release signature
//! admit     admitPackage() — budget, capabilities, grants, dependencies, scan verdict
//! transport resolveTransport() against this body; a `refused` verdict fails the install
//! stage     unpack into /data/packages/<digest>/, still inert
//! activate  atomic symlink swap, then register with the supervisor
//! ```
//!
//! [`Store::stage`] is the middle four. Fetching is §34's and is not written; activation is
//! [`Store::activate`]; admission is [`skynet_contracts::admit_package`] and is called by the
//! caller who also holds the robot's capacity.
//!
//! # Activation is a rename, and the guarantee is what a rename gives
//!
//! > **Activation is a symlink swap**, so a crash at any point before it leaves the previous version
//! > running and a crash after it leaves the new one running. There is no window in which the robot
//! > has half a policy.
//!
//! `symlink` then `rename` is how that is obtained: a link is created under a temporary name and
//! renamed over `current`, and POSIX `rename` is atomic. Writing a link in place — unlink, then
//! symlink — has a window between the two calls in which the package has no current version at all,
//! and a robot that lost power there would come back with a policy nobody removed and nothing to
//! run.
//!
//! # What is verified, and what is not
//!
//! The **digest** is: [`Store::stage`] hashes every file it writes, computes the version digest the
//! Hub computes, and refuses if it differs from the one the desired state named. That is the check
//! that makes a content address mean something.
//!
//! The **signature** is not. §37.1 puts an Ed25519 verification against an operator-trusted key
//! immediately after the digest, and this edition has no Ed25519 — so [`StagedPackage::signature`]
//! reports [`Signature::NotChecked`] and every caller has to decide what to do about that, which is
//! the point. A `bool` defaulting to `false` would have been read as "unsigned"; a `bool` defaulting
//! to `true` would be a lie with a robot on the end of it. There is no third value that means
//! "verified" until something can produce one.
//!
//! # The on-disk shape of a package, which the specification does not pin
//!
//! §8 defines the `.sky` manifest and §28 says blobs are content-addressed, but nothing says how a
//! staged package is laid out. This edition pins it, because a second edition that chose
//! differently would install packages the first could not read:
//!
//! * `manifest.json` at the package root is the `.sky` manifest — the Hub's `manifest` column;
//! * every **other** file is a version file — the Hub's artifacts — and those are what the digest's
//!   file list covers;
//! * **exactly one file is executable**: the one `runtime.entrypoint` names. Everything else is
//!   `0644`.
//!
//! The first follows from the Hub keeping the two apart: `computeVersionDigest` hashes the manifest
//! as a *value* and the files as a list, so a manifest that also appeared in the list would be
//! hashed twice and no version published by the Hub would verify.
//!
//! The third is not a convenience. A file's mode is **not** part of the digest — the Hub stores
//! artifacts as bytes and a `(path, sha256)` pair carries no permission — so a store that preserved
//! whatever mode it found would let two robots holding the same digest disagree about which files
//! are executable. Deriving it from `runtime.entrypoint` instead makes the mode a function of the
//! digest, because the manifest is digested. And it is the smaller privilege: a package gets one
//! executable, the one it declared, rather than every file its publisher happened to `chmod +x`.
//!
//! A package needing a second executable — a driver with a helper — cannot express that today. That
//! is a real limit and it is recorded as one rather than papered over with a mode-preserving unpack
//! that would take the reproducibility with it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use skynet_contracts::digest::{version_digest, DigestFile};
use skynet_json::Json;

use crate::layout::{parse_digest, IdError, Layout, PackageId};

/// How many superseded versions stay pinned (§37.2's `N`, default 2).
pub const ROLLBACK_DEPTH: usize = 2;

/// The manifest file at the root of a staged package.
pub const MANIFEST_FILE: &str = "manifest.json";

/// What is known about a package's release signature.
///
/// Three states rather than a `bool`, because "nobody checked" is a distinct fact from "checked and
/// it failed" and collapsing them is how an unverified package becomes an unsigned one in a report
/// somebody trusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signature {
    /// No verification was performed. This edition has no Ed25519 (§37.1), and says so rather than
    /// reporting a verdict it did not reach.
    NotChecked,
    /// Verified against a key the operator trusts.
    Verified,
    /// Checked and refused.
    Failed,
}

impl Signature {
    /// Whether this may be passed to admission as a verified signature.
    ///
    /// Only [`Signature::Verified`] may. `NotChecked` is deliberately *not* treated as verified,
    /// which means every package this edition stages carries admission's `unsigned` finding — and
    /// that is the correct report for a robot that cannot check a signature yet.
    pub fn is_verified(self) -> bool {
        self == Self::Verified
    }
}

/// A package as it sits in the store, staged and possibly activated.
#[derive(Debug, Clone, PartialEq)]
pub struct StagedPackage {
    /// Which package.
    pub id: PackageId,
    /// The digest its content actually hashes to.
    pub digest: String,
    /// The `.sky` manifest as read from the blob.
    pub manifest: Json,
    /// What is known about its signature.
    pub signature: Signature,
}

/// Why a store operation failed.
#[derive(Debug)]
pub enum StoreError {
    /// The filesystem said no.
    Io {
        /// What was being done.
        doing: String,
        /// The path involved.
        path: PathBuf,
        /// The error.
        source: io::Error,
    },
    /// An identifier that cannot be a path component.
    Id(IdError),
    /// A digest could not be computed over the staged content.
    Digest(skynet_contracts::digest::DigestError),
    /// The content does not hash to the digest that was asked for.
    ///
    /// The most important error in this module: it is the difference between "these are the bytes
    /// the Hub published" and "these are some bytes".
    DigestMismatch {
        /// What the desired state named.
        expected: String,
        /// What the content hashes to.
        observed: String,
    },
    /// The package has no readable manifest.
    Manifest {
        /// What is wrong with it.
        detail: String,
    },
    /// Something was asked for that is not in the store.
    NotFound {
        /// What.
        what: String,
    },
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io {
                doing,
                path,
                source,
            } => {
                write!(f, "{doing} ({}): {source}", path.display())
            }
            Self::Id(err) => write!(f, "{err}"),
            Self::Digest(err) => write!(f, "{err}"),
            Self::DigestMismatch { expected, observed } => write!(
                f,
                "the content hashes to {observed}, and {expected} was asked for. These are not the \
                 bytes that were published"
            ),
            Self::Manifest { detail } => write!(f, "manifest: {detail}"),
            Self::NotFound { what } => write!(f, "{what} is not in the store"),
        }
    }
}

impl std::error::Error for StoreError {}

fn io_err(doing: &str, path: &Path, source: io::Error) -> StoreError {
    StoreError::Io {
        doing: doing.to_string(),
        path: path.to_path_buf(),
        source,
    }
}

/// The store, over a [`Layout`].
#[derive(Debug, Clone)]
pub struct Store {
    layout: Layout,
}

impl Store {
    /// A store over this layout. Creates nothing until asked to.
    pub fn new(layout: Layout) -> Self {
        Self { layout }
    }

    /// The layout it is over.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Stage a package from a directory of files, verifying it hashes to `expect_digest`.
    ///
    /// `kind` and `version` are the repository kind and the version label, because both are inputs
    /// to the digest — the same bytes published as a dataset and as a policy are not the same
    /// artifact, and the digest says so.
    ///
    /// **Idempotent, and that matters more than it looks.** A blob already present is left exactly
    /// as it is and reported as staged. §37.1 calls that dedupe; §33.3 needs it, because a
    /// level-triggered reconciler emits the same fetch again on every pass until it observes the
    /// result, and a stage that failed on "already exists" would make convergence depend on an
    /// observation arriving between two passes.
    pub fn stage(
        &self,
        id: &PackageId,
        kind: &str,
        version: &str,
        expect_digest: &str,
        source: &Path,
    ) -> Result<StagedPackage, StoreError> {
        parse_digest(expect_digest).map_err(StoreError::Id)?;

        let manifest_path = source.join(MANIFEST_FILE);
        let manifest_bytes = fs::read(&manifest_path)
            .map_err(|e| io_err("reading the package manifest", &manifest_path, e))?;
        let manifest_text =
            String::from_utf8(manifest_bytes).map_err(|_| StoreError::Manifest {
                detail: "is not UTF-8".to_string(),
            })?;
        let manifest = Json::parse(&manifest_text).map_err(|e| StoreError::Manifest {
            detail: format!("does not parse: {e}"),
        })?;

        let files = walk(source, source)?;
        let digest_files: Vec<DigestFile> = files
            .iter()
            .filter(|(path, _)| path.as_str() != MANIFEST_FILE)
            .map(|(path, bytes)| DigestFile {
                path: path.clone(),
                sha256: hex(bytes),
            })
            .collect();

        let observed =
            version_digest(kind, version, &manifest, &digest_files).map_err(StoreError::Digest)?;
        if observed != expect_digest {
            return Err(StoreError::DigestMismatch {
                expected: expect_digest.to_string(),
                observed,
            });
        }

        let blob = self.layout.blob(&observed);
        if !blob.exists() {
            let staging = self.layout.blobs().join(format!(".staging.{observed}"));
            if staging.exists() {
                // A previous attempt died between writing and renaming. Its content is unverified
                // and no link points at it, so it is rubbish rather than history.
                fs::remove_dir_all(&staging)
                    .map_err(|e| io_err("clearing an abandoned staging directory", &staging, e))?;
            }
            fs::create_dir_all(&staging)
                .map_err(|e| io_err("creating the staging directory", &staging, e))?;
            for (path, bytes) in &files {
                let target = staging.join(path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| io_err("creating a package directory", parent, e))?;
                }
                fs::write(&target, bytes)
                    .map_err(|e| io_err("writing a package file", &target, e))?;
            }
            mark_entrypoint(&manifest, &staging)?;
            // The rename is what makes the blob appear, so a blob that exists is a blob that was
            // written completely. A partially written `blobs/<digest>` would be indistinguishable
            // from a verified one on the next boot.
            fs::rename(&staging, &blob).map_err(|e| io_err("staging the package", &blob, e))?;
        }

        Ok(StagedPackage {
            id: id.clone(),
            digest: observed,
            manifest,
            signature: Signature::NotChecked,
        })
    }

    /// Make a staged digest the current version of a package.
    ///
    /// Idempotent: activating what is already current does nothing at all, including to the
    /// rollback links — a reconciler that re-emitted an activation must not be able to push the
    /// version a rollback would return to off the end of the list.
    pub fn activate(&self, id: &PackageId, digest: &str) -> Result<(), StoreError> {
        parse_digest(digest).map_err(StoreError::Id)?;
        let blob = self.layout.blob(digest);
        if !blob.is_dir() {
            return Err(StoreError::NotFound {
                what: format!("blob {digest}"),
            });
        }

        if self.active_digest(id) == Some(digest.to_string()) {
            return Ok(());
        }

        let links = self.layout.links(id);
        fs::create_dir_all(&links).map_err(|e| io_err("creating the link directory", &links, e))?;

        // Shift the history down before the swap. Doing it after would leave a crash-shaped window
        // in which the new version is current and the version it replaced is not pinned — which is
        // exactly the moment a rollback is most likely to be wanted.
        for depth in (1..ROLLBACK_DEPTH).rev() {
            let from = self.layout.prev_link(id, depth);
            if from.symlink_metadata().is_ok() {
                let to = self.layout.prev_link(id, depth + 1);
                replace_link(&read_link_target(&from)?, &to)?;
            }
        }
        if let Some(current) = self.active_digest(id) {
            replace_link(
                &self.layout.link_target(&current),
                &self.layout.prev_link(id, 1),
            )?;
        }

        replace_link(
            &self.layout.link_target(digest),
            &self.layout.current_link(id),
        )
    }

    /// The digest currently activated for a package, if any.
    pub fn active_digest(&self, id: &PackageId) -> Option<String> {
        link_digest(&self.layout.current_link(id))
    }

    /// The digest a rollback would return to (§37.2).
    pub fn rollback_digest(&self, id: &PackageId) -> Option<String> {
        link_digest(&self.layout.prev_link(id, 1))
    }

    /// Roll back to the previous version.
    ///
    /// > Rollback is therefore the same swap in reverse and completes in milliseconds without a
    /// > network — which matters, because the moment you most need to roll back is the moment the
    /// > new version broke something.
    pub fn rollback(&self, id: &PackageId) -> Result<String, StoreError> {
        let Some(previous) = self.rollback_digest(id) else {
            return Err(StoreError::NotFound {
                what: format!("a previous version of {id}"),
            });
        };
        self.activate(id, &previous)?;
        Ok(previous)
    }

    /// Forget a package: drop its links, leaving the blobs for GC.
    ///
    /// The blobs stay because another package may share them — the store is content-addressed and
    /// two policies built from the same bytes are one blob (§2). Deciding what is unreferenced is
    /// [`Store::collect`]'s job and it does it by looking, not by counting as it goes.
    pub fn remove(&self, id: &PackageId) -> Result<(), StoreError> {
        let links = self.layout.links(id);
        if links.exists() {
            fs::remove_dir_all(&links).map_err(|e| io_err("removing package links", &links, e))?;
        }
        Ok(())
    }

    /// Every package with a current version, and the digest it points at.
    pub fn installed(&self) -> Vec<(PackageId, String)> {
        let mut found = Vec::new();
        let root = self.layout.active_root();
        let Ok(namespaces) = fs::read_dir(&root) else {
            return found;
        };
        for namespace in namespaces.flatten() {
            let Ok(names) = fs::read_dir(namespace.path()) else {
                continue;
            };
            for name in names.flatten() {
                let raw = format!(
                    "{}/{}",
                    namespace.file_name().to_string_lossy(),
                    name.file_name().to_string_lossy()
                );
                // A directory whose name is not a legal id was not put there by this code. It is
                // reported by being skipped rather than by being parsed leniently: an id that only
                // half-exists is how a package ends up installed under a name nothing can address.
                let Ok(id) = PackageId::parse(&raw) else {
                    continue;
                };
                if let Some(digest) = self.active_digest(&id) {
                    found.push((id, digest));
                }
            }
        }
        found.sort();
        found
    }

    /// Every blob in the store, with the package its manifest says it belongs to.
    ///
    /// Blobs are content-addressed, so a directory name says nothing about which package it is —
    /// the manifest inside it does. This is the read that makes **staged but not activated** an
    /// observable state rather than a state only `skynetd` remembers, and §37.1 needs that state to
    /// exist: fetching and verifying is free and happens under a running policy, and only the
    /// symlink swap waits for a gate.
    ///
    /// A blob with no manifest, or one naming an id that is not an id, is skipped. It is content
    /// nothing put there through [`Store::stage`], and guessing an owner for it would attach a
    /// package name to bytes on the strength of a directory listing.
    pub fn staged(&self) -> Vec<(PackageId, String)> {
        let mut found = Vec::new();
        let Ok(entries) = fs::read_dir(self.layout.blobs()) else {
            return found;
        };
        for entry in entries.flatten() {
            let digest = entry.file_name().to_string_lossy().to_string();
            if parse_digest(&digest).is_err() {
                continue;
            }
            let Ok(manifest) = self.manifest(&digest) else {
                continue;
            };
            let Some(raw) = manifest.get_present("id").and_then(Json::as_str) else {
                continue;
            };
            let Ok(id) = PackageId::parse(raw) else {
                continue;
            };
            found.push((id, digest));
        }
        found.sort();
        found
    }

    /// Every digest any link points at — the set GC may never touch (§37.3).
    pub fn pinned(&self) -> BTreeSet<String> {
        let mut pinned = BTreeSet::new();
        let root = self.layout.active_root();
        let Ok(namespaces) = fs::read_dir(&root) else {
            return pinned;
        };
        for namespace in namespaces.flatten() {
            let Ok(names) = fs::read_dir(namespace.path()) else {
                continue;
            };
            for name in names.flatten() {
                let Ok(links) = fs::read_dir(name.path()) else {
                    continue;
                };
                for link in links.flatten() {
                    if let Some(digest) = link_digest(&link.path()) {
                        pinned.insert(digest);
                    }
                }
            }
        }
        pinned
    }

    /// Collect blobs nothing points at (§37.3).
    ///
    /// > **GC never touches the rollback set**, even under disk pressure. A robot that frees space
    /// > by deleting the version it would roll back to has optimised for the wrong failure.
    ///
    /// Which is free here rather than careful: the rollback set *is* the set of links, so "collect
    /// what nothing points at" cannot reach it. There is no pressure flag and no override, because
    /// the only way this function could delete a pinned blob is if somebody added one.
    ///
    /// Returns the digests collected, so the caller can record them — a blob disappearing with no
    /// record is how a store becomes a mystery.
    pub fn collect(&self) -> Result<Vec<String>, StoreError> {
        let pinned = self.pinned();
        let blobs = self.layout.blobs();
        let mut collected = Vec::new();
        let Ok(entries) = fs::read_dir(&blobs) else {
            return Ok(collected);
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // An abandoned staging directory is rubbish: unverified content nothing can point at,
            // because a link is only ever written to a digest that verified.
            if let Some(rest) = name.strip_prefix(".staging.") {
                let _ = rest;
                fs::remove_dir_all(entry.path()).map_err(|e| {
                    io_err("clearing an abandoned staging directory", &entry.path(), e)
                })?;
                continue;
            }
            if parse_digest(&name).is_err() || pinned.contains(&name) {
                continue;
            }
            fs::remove_dir_all(entry.path())
                .map_err(|e| io_err("collecting a blob", &entry.path(), e))?;
            collected.push(name);
        }
        collected.sort();
        Ok(collected)
    }

    /// Read a staged package's manifest.
    pub fn manifest(&self, digest: &str) -> Result<Json, StoreError> {
        let path = self.layout.blob(digest).join(MANIFEST_FILE);
        let text = fs::read_to_string(&path).map_err(|e| io_err("reading a manifest", &path, e))?;
        Json::parse(&text).map_err(|e| StoreError::Manifest {
            detail: format!("does not parse: {e}"),
        })
    }

    /// Create the directories the store needs. Safe to call repeatedly.
    pub fn prepare(&self) -> Result<(), StoreError> {
        for dir in [
            self.layout.blobs(),
            self.layout.active_root(),
            self.layout.state(),
        ] {
            fs::create_dir_all(&dir).map_err(|e| io_err("creating a store directory", &dir, e))?;
        }
        Ok(())
    }
}

/// The mode the declared entry point is written with.
pub const ENTRYPOINT_MODE: u32 = 0o755;

/// Make the file `runtime.entrypoint` names executable, and nothing else.
///
/// Silent when the manifest declares no entry point: a `skill` or a dataset has nothing to run, and
/// a policy that declares none is refused later, by the launcher, with a sentence about the
/// manifest. Failing here would report a manifest problem as a staging problem.
#[cfg(unix)]
fn mark_entrypoint(manifest: &Json, staged: &Path) -> Result<(), StoreError> {
    use std::os::unix::fs::PermissionsExt;

    let Some(entrypoint) = manifest
        .get("runtime")
        .and_then(|runtime| runtime.get_present("entrypoint"))
        .and_then(Json::as_str)
    else {
        return Ok(());
    };
    // The same rule the digest applies. A manifest naming `../../bin/sh` must not get a `chmod`
    // outside the package, and checking here as well as at launch means the two cannot drift.
    if skynet_contracts::digest::check_path(entrypoint).is_err() {
        return Err(StoreError::Manifest {
            detail: format!(
                "`runtime.entrypoint` is not a path inside the package: `{entrypoint}`"
            ),
        });
    }
    let path = staged.join(entrypoint);
    if !path.is_file() {
        return Err(StoreError::Manifest {
            detail: format!(
                "`runtime.entrypoint` names `{entrypoint}`, which the package does not contain"
            ),
        });
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(ENTRYPOINT_MODE))
        .map_err(|e| io_err("marking the entry point executable", &path, e))
}

/// Modes are a POSIX idea; on anything else the store writes the files and the launcher is what
/// fails, which is the honest failure for a platform a robot does not run on.
#[cfg(not(unix))]
fn mark_entrypoint(_manifest: &Json, _staged: &Path) -> Result<(), StoreError> {
    Ok(())
}

/// Read every file under `dir`, keyed by its `/`-separated path relative to `base`.
///
/// A `BTreeMap` rather than a `Vec`, so the walk order of a filesystem cannot reach the digest.
/// Two robots hashing the same tree on two filesystems must agree, and directory iteration order
/// is not a property either of them controls.
fn walk(base: &Path, dir: &Path) -> Result<BTreeMap<String, Vec<u8>>, StoreError> {
    let mut found = BTreeMap::new();
    let entries = fs::read_dir(dir).map_err(|e| io_err("reading a package directory", dir, e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err("reading a package directory entry", dir, e))?;
        let path = entry.path();
        let kind = entry
            .file_type()
            .map_err(|e| io_err("inspecting a package entry", &path, e))?;
        if kind.is_dir() {
            found.extend(walk(base, &path)?);
        } else if kind.is_file() {
            let bytes = fs::read(&path).map_err(|e| io_err("reading a package file", &path, e))?;
            let relative = path
                .strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            found.insert(relative, bytes);
        }
        // Anything else — a symlink, a device node, a socket — is skipped and not hashed. A package
        // is content, and a symlink is a reference to content somewhere else on a machine the
        // publisher has never seen.
    }
    Ok(found)
}

fn hex(bytes: &[u8]) -> String {
    let digest = skynet_sha256::hex_bytes(bytes);
    String::from_utf8_lossy(&digest).into_owned()
}

/// The digest a link points at, or `None` if it is not a link into the blob store.
fn link_digest(link: &Path) -> Option<String> {
    let target = fs::read_link(link).ok()?;
    let name = target.file_name()?.to_string_lossy().to_string();
    parse_digest(&name).ok()?;
    // A link whose parent component is not `blobs` points somewhere else entirely, and treating it
    // as a pin would let a link outside the store keep a blob alive — or worse, name one.
    let parent = target.parent()?.file_name()?.to_string_lossy().to_string();
    if parent == "blobs" {
        Some(name)
    } else {
        None
    }
}

fn read_link_target(link: &Path) -> Result<PathBuf, StoreError> {
    fs::read_link(link).map_err(|e| io_err("reading a link", link, e))
}

/// Point `at` at `target`, atomically, whatever was there before.
///
/// Written under a temporary name and renamed, because `rename` is atomic and unlink-then-symlink
/// is not: between the two calls the package has no current version, and a robot that lost power
/// in that window would boot with a policy nobody removed and nothing to run.
#[cfg(unix)]
fn replace_link(target: &Path, at: &Path) -> Result<(), StoreError> {
    let parent = at.parent().unwrap_or(Path::new("."));
    let temporary = parent.join(format!(
        ".{}.new",
        at.file_name().unwrap_or_default().to_string_lossy()
    ));
    if temporary.symlink_metadata().is_ok() {
        fs::remove_file(&temporary).map_err(|e| io_err("clearing a stale link", &temporary, e))?;
    }
    std::os::unix::fs::symlink(target, &temporary)
        .map_err(|e| io_err("creating a link", &temporary, e))?;
    fs::rename(&temporary, at).map_err(|e| io_err("swapping a link", at, e))
}

/// Everything above is POSIX. A robot runs Linux (§23.5); this exists so the rest of the crate
/// compiles on a workstation, and it refuses rather than emulating — a store whose activation is
/// not atomic is a store that does not have the property §37.1 rests on, and pretending otherwise
/// on a developer's machine is how the difference stops being noticed.
#[cfg(not(unix))]
fn replace_link(_target: &Path, at: &Path) -> Result<(), StoreError> {
    Err(io_err(
        "activating a package: the atomic symlink swap §37.1 requires exists on POSIX and this is \
         not POSIX",
        at,
        io::Error::new(io::ErrorKind::Unsupported, "not a POSIX filesystem"),
    ))
}

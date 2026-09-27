//! §37.1's `fetch`, over §91's route: a package this robot was told to run and does not hold.
//!
//! For thirty-four sessions `FetchPackage` answered `Unimplemented` unless the blob was already on
//! the disk, and its sentence said *"§34's outbound path is not written"*. That stopped being true
//! when §75's image fetch and §11's pull were built on the path. What was missing was a route: the
//! Hub had no way to serve a package to a device credential (F148). §91 gives it one, and this is
//! the robot's half.
//!
//! # Three checks, and the location is none of them
//!
//! The document names a short-lived location for every file, and a location is not evidence
//! (§75.1). What makes the bytes the package is:
//!
//! 1. **The document is about what was asked for.** Its id, kind, version and digest are the
//!    desired state's, or nothing is fetched. A document about other bytes would otherwise fill
//!    this version's directory with them.
//! 2. **Each file is the file named.** Its length and its sha256 are checked before the partial is
//!    renamed into place, so a file that is in place is a file that was checked.
//! 3. **The whole is the version.** [`Store::stage`] recomputes the version digest over the kind,
//!    the version, the manifest and every file, exactly as for a package staged by hand. A document
//!    that lied consistently about its files passes the second check and fails this one.
//!
//! # A slice, not a transfer
//!
//! §75.2's rule, and the same budget: one pass moves what it can inside [`SLICE_SECONDS`] across
//! every file it touches, and the next pass resumes from the partials. The document is read again
//! on each pass, because its locations expire and a pass is where a fresh one is cheapest.
//!
//! # Nothing here carries the device credential off the Hub
//!
//! The document comes from the Hub, with the credential. The files come from wherever their
//! locations point, which is object storage and not the Hub. A signed location carries its own
//! authority, and presenting a device credential to a host that did not issue it would hand a
//! stranger the key §19 assumes is already lost. So the file requests carry none.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use skynet_json::Json;

use crate::bundle::{
    backoff_ms, bearer_of, hash_file, size_of, Ask, Attempts, Fetcher, SLICE_SECONDS,
};
use crate::layout::{parse_digest, Layout, PackageId};
use crate::store::{Store, StoreError, MANIFEST_FILE};

/// What the desired state asked for: the four facts the document must agree with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    /// `namespace/name`.
    pub id: String,
    /// The repository kind, which the version digest covers.
    pub kind: String,
    /// The version label, which the version digest covers.
    pub version: String,
    /// The version digest, bare.
    pub digest: String,
}

/// One file of the version, as §91.1's document names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexFile {
    /// Where it goes inside the package.
    pub path: String,
    /// Its sha256, bare hex.
    pub sha256: String,
    /// Its length in bytes.
    pub size: u64,
    /// Where to get it. Not evidence.
    pub url: String,
}

/// §91.1's document, checked against what was asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct Index {
    /// What the desired state asked for, which the document agreed with.
    pub want: Want,
    /// The `.sky` manifest, which becomes `manifest.json`.
    pub manifest: Json,
    /// The version's files.
    pub files: Vec<IndexFile>,
}

/// What one pass did about one package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Every file arrived, and the store staged the version.
    Staged {
        /// The sentence.
        detail: String,
    },
    /// Nothing was done, and nothing is wrong: no link, a metered one, or a backoff.
    Waiting {
        /// Which.
        detail: String,
    },
    /// Bytes moved and the version is not all here yet.
    Moved {
        /// The sentence.
        detail: String,
    },
    /// Something is wrong and an operator should read it.
    Failed {
        /// What.
        detail: String,
    },
}

/// `{endpoint}/devices/{id}/packages/{digest}` (§91).
pub fn url_for(endpoint: &str, device_id: &str, digest: &str) -> String {
    format!(
        "{}/devices/{device_id}/packages/{digest}",
        endpoint.trim_end_matches('/')
    )
}

/// Whether a path from the document can be a file inside this package (§91.3).
///
/// Refused: an empty path, an absolute one, a backslash or a NUL anywhere, an empty `.` or `..`
/// segment, and `manifest.json` at the root, which §37's layout reserves for the manifest. The
/// first five are ways out of the package's directory or ways to name the same file twice. The
/// last would let the files overwrite the manifest the digest was computed over.
pub fn safe_path(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("an empty path".to_string());
    }
    if path.starts_with('/') {
        return Err(format!("`{path}` is absolute"));
    }
    if path.contains('\\') || path.contains('\0') {
        return Err(format!("`{path}` contains a backslash or a NUL"));
    }
    if path
        .split('/')
        .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(format!("`{path}` has an empty, `.` or `..` segment"));
    }
    if path == MANIFEST_FILE {
        return Err(format!(
            "`{MANIFEST_FILE}` at the root is the manifest's place, and a file may not take it"
        ));
    }
    Ok(())
}

/// Read §91.1's document and hold it to what the desired state asked for.
///
/// Refuses before a byte is fetched when the document is about anything else, names a path that
/// could leave the package, names the same path twice, or leaves out a length or a checksum.
pub fn read_index(body: &str, want: &Want) -> Result<Index, String> {
    let document =
        Json::parse(body).map_err(|error| format!("§91's document does not parse: {error}"))?;

    for (field, expected) in [
        ("id", &want.id),
        ("kind", &want.kind),
        ("version", &want.version),
        ("digest", &want.digest),
    ] {
        let said = document.get(field).and_then(Json::as_str).unwrap_or("");
        if said != expected.as_str() {
            return Err(format!(
                "the Hub's document names {field} `{said}`, and the desired state asked for \
                 `{expected}`. A document about other bytes is not fetched from"
            ));
        }
    }

    let manifest = match document.get("manifest") {
        Some(manifest @ Json::Obj(_)) => manifest.clone(),
        _ => return Err("the Hub's document carries no manifest object".to_string()),
    };
    let Some(Json::Arr(entries)) = document.get("files") else {
        return Err("the Hub's document carries no file list".to_string());
    };

    let mut files = Vec::with_capacity(entries.len());
    let mut seen = BTreeSet::new();
    for entry in entries {
        let path = entry.get("path").and_then(Json::as_str).unwrap_or("");
        safe_path(path).map_err(|why| format!("the Hub's document names {why}"))?;
        if !seen.insert(path.to_string()) {
            return Err(format!("the Hub's document names `{path}` twice"));
        }
        let sha256 = entry.get("sha256").and_then(Json::as_str).unwrap_or("");
        if !skynet_contracts::digest::is_hex_sha256(sha256) {
            return Err(format!(
                "the Hub's document gives `{path}` no sha256, so nothing could check it"
            ));
        }
        let size = entry
            .get("size")
            .and_then(Json::as_f64)
            .filter(|size| size.is_finite() && *size >= 0.0 && size.fract() == 0.0)
            .ok_or_else(|| format!("the Hub's document gives `{path}` no length"))?;
        let url = entry.get("url").and_then(Json::as_str).unwrap_or("");
        if url.is_empty() {
            return Err(format!("the Hub's document gives `{path}` no location"));
        }
        files.push(IndexFile {
            path: path.to_string(),
            sha256: sha256.to_string(),
            size: size as u64,
            url: url.to_string(),
        });
    }

    Ok(Index {
        want: want.clone(),
        manifest,
        files,
    })
}

/// Record a failed attempt, so the next waits out §34.4's curve.
///
/// Used for a refusal from the Hub as well as for bad bytes: a level-triggered reconciler asks
/// again every pass, and a robot that re-asked a Hub that had said no every few seconds would be
/// the fleet-wide hammering the curve exists to prevent.
pub fn note_failure(layout: &Layout, digest: &str, error: &str, now_ms: u64, jitter: f64) {
    let record = layout.package_transfer(digest);
    let mut attempts = Attempts::read_at(&record, digest);
    attempts.digest = digest.to_string();
    attempts.failures = attempts.failures.saturating_add(1);
    attempts.error = Some(error.to_string());
    attempts.next_ms = now_ms.saturating_add(backoff_ms(attempts.failures, jitter));
    let _ = attempts.write_at(&record);
}

/// When the next attempt at this version may start, if it is not now.
pub fn held_until(layout: &Layout, digest: &str, now_ms: u64) -> Option<(u64, Option<String>)> {
    let attempts = Attempts::read_at(&layout.package_transfer(digest), digest);
    (now_ms < attempts.next_ms).then_some((attempts.next_ms, attempts.error))
}

/// Where a file's bytes wait until they have been checked.
fn partial_of(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
}

/// Fetch what can be fetched of one version this pass, and stage it once every file is here.
#[allow(clippy::too_many_arguments)]
pub fn advance(
    layout: &Layout,
    store: &Store,
    fetcher: &mut dyn Fetcher,
    index: &Index,
    table: &crate::connectivity::Table,
    now_ms: u64,
    jitter: f64,
) -> Step {
    let want = &index.want;
    if let Err(error) = parse_digest(&want.digest) {
        return Step::Failed {
            detail: error.to_string(),
        };
    }
    let id = match PackageId::parse(&want.id) {
        Ok(id) => id,
        Err(error) => {
            return Step::Failed {
                detail: error.to_string(),
            }
        }
    };

    // §34.3, and the rule §75 applies to an image applies here unchanged (§91.4): a package's
    // files are a bulk transfer, and a model's weights are hundreds of megabytes.
    match bearer_of(table) {
        None => {
            return Step::Waiting {
                detail: format!(
                    "No link is up, so {}'s files wait for one (§34.1). Nothing already here is \
                     lost, and the transfer resumes when a link returns.",
                    want.id
                ),
            }
        }
        Some(bearer) if bearer.metered => {
            return Step::Waiting {
                detail: format!(
                    "`{}` is {} metered, and {}'s files are a bulk transfer, so they wait for an \
                     unmetered link (§34.3).",
                    bearer.name,
                    if bearer.metering_declared {
                        "declared"
                    } else {
                        "assumed"
                    },
                    want.id
                ),
            }
        }
        Some(_) => {}
    }

    if let Some((next_ms, error)) = held_until(layout, &want.digest, now_ms) {
        return Step::Waiting {
            detail: format!(
                "The last attempt at {} failed and the next is held for another {} s (§34.4's \
                 curve). {}",
                want.id,
                (next_ms - now_ms).div_ceil(1_000),
                error.unwrap_or_default()
            ),
        };
    }

    let root = layout.incoming(&want.digest);
    let tree = root.join("files");
    let record = layout.package_transfer(&want.digest);
    let started = Instant::now();

    for file in &index.files {
        let target = tree.join(&file.path);
        // In place means checked: a partial is renamed only after its length and sha256 matched.
        if target.is_file() {
            continue;
        }
        let partial = partial_of(&target);
        if let Some(parent) = partial.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                return Step::Failed {
                    detail: format!("{} could not be created: {error}", parent.display()),
                };
            }
        }

        // A partial longer than the file is not part of it: resuming would append the file's tail
        // to something else.
        let mut from = size_of(&partial);
        if from > file.size {
            let _ = std::fs::remove_file(&partial);
            from = 0;
        }

        if file.size == 0 {
            // An empty file has no bytes to fetch and curl may create nothing for it. Its checksum
            // is still checked below: the empty string's sha256 is a claim like any other.
            if let Err(error) = std::fs::write(&partial, b"") {
                return Step::Failed {
                    detail: format!("{} could not be written: {error}", partial.display()),
                };
            }
        } else if from < file.size {
            let spent = started.elapsed().as_secs() as u32;
            let budget = SLICE_SECONDS.saturating_sub(spent);
            if budget == 0 {
                return Step::Moved {
                    detail: format!(
                        "{}'s files are partly here; this pass's {SLICE_SECONDS} s are spent and the \
                         next pass resumes.",
                        want.id
                    ),
                };
            }
            // No credential: see the module header. The location is signed by the Hub for this
            // one read.
            let slice = fetcher.slice(&Ask {
                url: file.url.clone(),
                credential: None,
                from,
                into: partial.clone(),
                budget_s: budget,
            });
            let moved = slice.after.saturating_sub(slice.before);

            // A server that answered a resume with the whole file had curl append it to the part
            // already here, and no checksum could come of those bytes.
            if from > 0 && slice.status == 200 {
                let _ = std::fs::remove_file(&partial);
                let why = format!(
                    "the server ignored the range on a resume of `{}` and sent the whole file; \
                     the partial is discarded and the next attempt starts it again",
                    file.path
                );
                note_failure(layout, &want.digest, &why, now_ms, jitter);
                return Step::Failed { detail: why };
            }
            if let Some(error) = slice.error {
                if moved > 0 {
                    // Progress and a lost link are not a failure the curve should punish.
                    Attempts::clear_at(&record);
                    return Step::Moved {
                        detail: format!(
                            "{} of `{}`'s {} bytes are here. The link went before it finished \
                             ({error}), and the next pass resumes.",
                            slice.after, file.path, file.size
                        ),
                    };
                }
                note_failure(layout, &want.digest, &error, now_ms, jitter);
                return Step::Failed { detail: error };
            }
            if !slice.complete {
                Attempts::clear_at(&record);
                return Step::Moved {
                    detail: format!(
                        "{} of `{}`'s {} bytes are here after a bounded slice, and the next pass \
                         resumes.",
                        slice.after, file.path, file.size
                    ),
                };
            }
        }

        // The body arrived, or was already all here. Whether it is the file is checked now.
        let (arrived, length) = match hash_file(&partial) {
            Ok(pair) => pair,
            Err(error) => {
                return Step::Failed {
                    detail: format!("{} could not be read back: {error}", partial.display()),
                }
            }
        };
        if length < file.size {
            // Short, and kept: resuming is what a partial is for.
            return Step::Moved {
                detail: format!(
                    "{length} of `{}`'s {} bytes are here; the transfer stopped early and the next \
                     pass resumes.",
                    file.path, file.size
                ),
            };
        }
        if length != file.size || arrived != format!("sha256:{}", file.sha256) {
            let _ = std::fs::remove_file(&partial);
            let why = format!(
                "`{}` arrived as {length} bytes hashing to {arrived}, and the document said {} \
                 bytes hashing to sha256:{}. These are not the bytes that were published; \
                 discarded.",
                file.path, file.size, file.sha256
            );
            note_failure(layout, &want.digest, &why, now_ms, jitter);
            return Step::Failed { detail: why };
        }
        if let Err(error) = std::fs::rename(&partial, &target) {
            return Step::Failed {
                detail: format!(
                    "`{}` was checked and could not be moved into place: {error}",
                    file.path
                ),
            };
        }
    }

    // Every file is here and each is the file named. Now the whole.
    let manifest = match index.manifest.to_canonical() {
        Ok(text) => text,
        Err(error) => {
            return Step::Failed {
                detail: format!("the manifest could not be written: {error:?}"),
            }
        }
    };
    if let Err(error) = std::fs::create_dir_all(&tree)
        .and_then(|()| std::fs::write(tree.join(MANIFEST_FILE), manifest.as_bytes()))
    {
        return Step::Failed {
            detail: format!("the manifest could not be written: {error}"),
        };
    }

    match store.stage(&id, &want.kind, &want.version, &want.digest, &tree) {
        Ok(staged) => {
            let _ = std::fs::remove_dir_all(&root);
            Attempts::clear_at(&record);
            Step::Staged {
                detail: format!(
                    "{} {} fetched: {} file(s), each checked against its sha256, and the whole \
                     against the version digest {}",
                    want.id,
                    want.version,
                    index.files.len(),
                    staged.digest
                ),
            }
        }
        Err(error @ StoreError::DigestMismatch { .. }) => {
            // Every file matched the document and the version does not: the document lied about
            // the set, not about a file. Nothing here can be trusted to resume from.
            let _ = std::fs::remove_dir_all(&root);
            let why = format!(
                "every file matched the Hub's document and the version does not: {error}. The \
                 files are discarded"
            );
            note_failure(layout, &want.digest, &why, now_ms, jitter);
            Step::Failed { detail: why }
        }
        Err(error) => Step::Failed {
            detail: error.to_string(),
        },
    }
}

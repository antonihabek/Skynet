//! Version digests — the identity a package is verified against (§37.1).
//!
//! > ```text
//! > fetch     by sha256, skipping any blob already present
//! > verify    digest, then Ed25519 signature against a key the operator trusts
//! > ```
//!
//! [`admit_package`](crate::admit_package) takes `observed_digest` as an argument: the digest the
//! bytes on disk actually hash to, which it compares against the one the manifest claims. Something
//! has to compute that argument, and until this module existed nothing did — which meant the
//! digest-mismatch check was a check whose input came from whoever was calling, and a verification
//! step nobody can perform is decoration.
//!
//! # The algorithm is the Hub's, and that is the point
//!
//! A digest is only useful if two parties compute the same one. The Hub has published versions
//! under `computeVersionDigest` since before the OS existed, so this is that function transcribed
//! rather than a new one invented for the robot:
//!
//! ```text
//! sha256(
//!   "skynet.version.v1"  \n
//!   kind                 \n
//!   version              \n
//!   canonical(manifest)  \n
//!   "<path>\0<sha256>" \n … for every file, sorted by path
//! )
//! ```
//!
//! Three of those lines are worth a sentence each.
//!
//! **The file list is `(path, sha256)` rather than the concatenated bytes.** A robot verifying an
//! eight-gigabyte package streams each file once and holds one hash at a time; a robot verifying a
//! concatenation has to decide a framing, and a framing without an explicit length is a framing in
//! which two different trees produce one stream.
//!
//! **The separator between a path and its hash is a NUL, not a space.** It is the one byte a path
//! may not contain — [`check_path`] enforces exactly that, which is what makes the separator
//! unambiguous rather than merely unlikely to collide. A space would have been readable and would
//! have let `a b` with one hash and `a` with a hash beginning `b` produce the same line.
//!
//! **The manifest is canonicalised, not stringified.** Key order in a JSON document is an accident
//! of whoever wrote it, and a digest that changed when a publisher reordered two fields would make
//! reproducibility a property of a text editor.
//!
//! # Where the two implementations could disagree, and why they cannot silently
//!
//! The Hub canonicalises with `JSON.stringify` for scalars, and JavaScript writes a number in
//! exponent form once its magnitude reaches 10^21 or falls below 10^-6. [`skynet_json`] never emits
//! an exponent — §31.4 requires one spelling per value and picked the one without. For every number
//! either side would actually meet in a manifest — a schema version, a megabyte count, a core count
//! — the two agree exactly; outside that range they would produce different bytes and therefore
//! different identities for the same package.
//!
//! The second is key order. The Hub sorts object keys by JavaScript's `<`, which compares UTF-16
//! code units; this edition sorts by UTF-8 byte, which is code-point order. For every key in the
//! basic multilingual plane the two are the same order — a UTF-16 code unit *is* the code point
//! there — and they diverge only once a key carries a character above U+FFFF, whose surrogate pair
//! sorts below U+E000 in one order and above it in the other. So a manifest with an astral
//! character in a **key** is refused too, and one with astral characters in a *value* is not,
//! because a value's spelling does not decide anybody's ordering.
//!
//! So [`version_digest`] **refuses** a manifest that carries either rather than returning a digest
//! only one side would reproduce. A refusal is a package that does not install and a person reading
//! a reason; agreement-by-luck is a package that verifies on the robot and fails on the Hub, or
//! worse, the reverse.
//!
//! The file list needs no such rule: it is sorted here by UTF-16 code unit explicitly, which is
//! exactly what the Hub's comparison does, so any path either side can spell produces one order.

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::cmp::Ordering;

use skynet_json::Json;

/// The domain separator. Present so a digest over a version can never collide with a digest over
/// anything else the platform hashes, however similar the inputs look.
pub const VERSION_DIGEST_DOMAIN: &str = "skynet.version.v1";

/// The domain separator for a bench subject — the artefacts one SCTS bench session measured.
///
/// Separate from [`VERSION_DIGEST_DOMAIN`] so that a digest over a package and a digest over a
/// bench subject can never be the same value for the same file list. See [`subject_digest`].
pub const BENCH_SUBJECT_DOMAIN: &str = "skynet.bench.subject.v1";

/// The smallest magnitude JavaScript writes without an exponent.
const JS_MIN_PLAIN: f64 = 1e-6;

/// The magnitude at which JavaScript switches to an exponent.
const JS_MAX_PLAIN: f64 = 1e21;

/// One file, as the digest sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestFile {
    /// Path relative to the package root, with `/` separators.
    pub path: String,
    /// Lowercase hex sha256 of the file's bytes.
    pub sha256: String,
}

/// Why a digest could not be computed.
///
/// Every variant names the input rather than saying the digest failed, because the caller's next
/// action differs completely between "this path is not a path" and "this manifest holds a number
/// the two implementations spell differently".
#[derive(Debug, Clone, PartialEq)]
pub enum DigestError {
    /// A path that is not usable as one, with the reason.
    ///
    /// The same list is what lays the package out on disk, so a path that escapes the package root
    /// is a package that writes outside its own directory. Checked here rather than only at unpack
    /// time because a digest computed over a traversal is a digest that *attests* to one.
    Path {
        /// The offending path, as given.
        path: String,
        /// What is wrong with it.
        detail: &'static str,
    },
    /// Two entries for one path. The digest would depend on which one a sorter happened to put
    /// first, and the two may name different bytes.
    DuplicatePath {
        /// The path that appears twice.
        path: String,
    },
    /// A hash that is not a lowercase hex sha256.
    Sha256 {
        /// The path it belongs to.
        path: String,
        /// The value that is not a digest.
        value: String,
    },
    /// The manifest could not be canonicalised.
    Manifest {
        /// What went wrong.
        detail: String,
    },
    /// A number the Hub and this crate would write differently. See the module documentation.
    NumberOutOfRange {
        /// The dotted path to the field.
        field: String,
        /// The value.
        value: f64,
    },
    /// An object key the Hub and this crate would sort differently. See the module documentation.
    KeyOutsideBmp {
        /// The dotted path to the object holding it.
        at: String,
        /// The key.
        key: String,
    },
}

impl core::fmt::Display for DigestError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Path { path, detail } => write!(f, "path `{path}`: {detail}"),
            Self::DuplicatePath { path } => write!(
                f,
                "path `{path}` appears twice, and the two entries may name different bytes"
            ),
            Self::Sha256 { path, value } => write!(
                f,
                "`{path}` carries `{value}`, which is not a lowercase hex sha256"
            ),
            Self::Manifest { detail } => write!(f, "manifest: {detail}"),
            Self::NumberOutOfRange { field, value } => write!(
                f,
                "`{field}` is {value}, which the Hub writes in exponent form and this edition does \
                 not. The two would compute different identities for the same package, so no digest \
                 is returned"
            ),
            Self::KeyOutsideBmp { at, key } => write!(
                f,
                "the object at `{at}` has a key `{key}` carrying a character above U+FFFF, which \
                 the Hub and this edition sort into different positions. The two would compute \
                 different identities for the same package, so no digest is returned"
            ),
        }
    }
}

/// Compute the digest of a version, as the Hub computes it.
///
/// `kind` and `version` are the repository kind (`policy`, `dataset`, …) and the version label.
/// Both are hashed as given: a version string is an identifier the publisher chose, and normalising
/// it here would fold two spellings of one release into one digest — the collision this whole
/// mechanism exists to prevent.
pub fn version_digest(
    kind: &str,
    version: &str,
    manifest: &Json,
    files: &[DigestFile],
) -> Result<String, DigestError> {
    let manifest_text = canonical_manifest(manifest)?;
    let file_lines = file_lines(files)?;

    let mut hasher = skynet_sha256::Sha256::new();
    hasher.update(VERSION_DIGEST_DOMAIN.as_bytes());
    hasher.update(b"\n");
    hasher.update(kind.as_bytes());
    hasher.update(b"\n");
    hasher.update(version.as_bytes());
    hasher.update(b"\n");
    hasher.update(manifest_text.as_bytes());
    hasher.update(b"\n");
    for (index, line) in file_lines.iter().enumerate() {
        if index > 0 {
            hasher.update(b"\n");
        }
        hasher.update(line.as_bytes());
    }

    let hex = skynet_sha256::hex_of(&hasher.finish());
    Ok(core::str::from_utf8(&hex)
        .expect("hex is ascii by construction")
        .to_string())
}

/// The digest of the artefacts a bench measurement was taken against (§15, and §12's shape).
///
/// The same `(path, sha256)` construction [`version_digest`] uses, over its own domain and with no
/// manifest — a bench subject has no publisher, no version label and nothing to canonicalise, and
/// inventing an empty manifest to reach one function would make the two digests differ by a field
/// that means nothing on either side.
///
/// **The domain is the point of the separate function.** A digest over a package version and a
/// digest over a set of bench artefacts are the same bytes hashed the same way, and a caller that
/// could compare one to the other would be comparing two facts that happen to have the same type.
/// [`BENCH_SUBJECT_DOMAIN`] makes that comparison impossible rather than merely unlikely.
pub fn subject_digest(files: &[DigestFile]) -> Result<String, DigestError> {
    let lines = file_lines(files)?;

    let mut hasher = skynet_sha256::Sha256::new();
    hasher.update(BENCH_SUBJECT_DOMAIN.as_bytes());
    hasher.update(b"\n");
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            hasher.update(b"\n");
        }
        hasher.update(line.as_bytes());
    }

    let hex = skynet_sha256::hex_of(&hasher.finish());
    Ok(core::str::from_utf8(&hex)
        .expect("hex is ascii by construction")
        .to_string())
}

/// The `<path> <sha256>` lines, ordered and validated.
///
/// Ordered by **UTF-16 code unit**, which is what the Hub's `a < b` on two strings compares. For
/// ASCII paths — which is every path anybody has — that is byte order; the two diverge only above
/// the basic multilingual plane, and matching exactly costs one line and removes a class of
/// divergence nobody would find by testing.
fn file_lines(files: &[DigestFile]) -> Result<Vec<String>, DigestError> {
    let mut sorted: Vec<&DigestFile> = files.iter().collect();
    for file in &sorted {
        check_path(&file.path)?;
        check_sha256(file)?;
    }
    sorted.sort_by(|a, b| utf16_cmp(&a.path, &b.path));
    for pair in sorted.windows(2) {
        if pair[0].path == pair[1].path {
            return Err(DigestError::DuplicatePath {
                path: pair[0].path.clone(),
            });
        }
    }
    Ok(sorted
        .iter()
        .map(|file| format!("{}\0{}", file.path, file.sha256))
        .collect())
}

/// Compare two strings the way JavaScript's relational operators do.
pub fn utf16_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// Whether a relative path may be used as one.
///
/// Deliberately strict. A package's file list is both the thing that is hashed and the thing that
/// is written to disk, and every rule here closes a way for the second to leave the package's own
/// directory or to mean two different things on two filesystems.
///
/// The control-character rule carries more weight than it looks: it excludes NUL, which is what
/// makes NUL a safe separator inside a digest line. Relax this and the framing above stops being a
/// framing.
pub fn check_path(path: &str) -> Result<(), DigestError> {
    let bad = |detail: &'static str| {
        Err(DigestError::Path {
            path: path.to_string(),
            detail,
        })
    };

    if path.is_empty() {
        return bad("empty");
    }
    if path.len() > 1024 {
        return bad("longer than 1024 bytes");
    }
    if path.starts_with('/') {
        return bad("absolute; paths are relative to the package root");
    }
    if path.contains('\\') {
        return bad(
            "contains a backslash, which is a separator on one filesystem and an ordinary \
             filename character on another",
        );
    }
    if path.chars().any(|c| (c as u32) < 0x20 || c == '\u{7f}') {
        return bad("contains a control character");
    }
    for component in path.split('/') {
        match component {
            "" => return bad("has an empty component, so two spellings name one file"),
            "." => return bad("has a `.` component"),
            ".." => return bad("has a `..` component, which leaves the package root"),
            _ => {}
        }
        if component.ends_with(' ') || component.ends_with('.') {
            return bad(
                "has a component ending in a space or a dot, which some filesystems silently strip",
            );
        }
    }
    Ok(())
}

fn check_sha256(file: &DigestFile) -> Result<(), DigestError> {
    if is_hex_sha256(&file.sha256) {
        Ok(())
    } else {
        Err(DigestError::Sha256 {
            path: file.path.clone(),
            value: file.sha256.clone(),
        })
    }
}

/// Whether a string is a lowercase hex sha256.
///
/// Lowercase only, and that is not fussiness: a digest is compared as a string in a dozen places on
/// both sides of the link, and two spellings of one hash is one more way for an equality test to
/// answer "different" about identical bytes.
pub fn is_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() && b <= b'f')
}

/// Canonicalise a manifest, refusing anything the two implementations would spell differently.
fn canonical_manifest(manifest: &Json) -> Result<String, DigestError> {
    check_numbers(manifest, "")?;
    manifest
        .to_canonical()
        .map_err(|err| DigestError::Manifest {
            detail: format!("{err:?}"),
        })
}

fn check_numbers(value: &Json, at: &str) -> Result<(), DigestError> {
    match value {
        Json::Num(n) => {
            let magnitude = n.abs();
            let diverges = !n.is_finite()
                || magnitude >= JS_MAX_PLAIN
                || (magnitude != 0.0 && magnitude < JS_MIN_PLAIN);
            if diverges {
                return Err(DigestError::NumberOutOfRange {
                    field: if at.is_empty() {
                        "(root)".to_string()
                    } else {
                        at.to_string()
                    },
                    value: *n,
                });
            }
            Ok(())
        }
        Json::Arr(items) => {
            for (index, item) in items.iter().enumerate() {
                check_numbers(item, &format!("{at}[{index}]"))?;
            }
            Ok(())
        }
        Json::Obj(map) => check_object(map, at),
        _ => Ok(()),
    }
}

fn check_object(map: &BTreeMap<String, Json>, at: &str) -> Result<(), DigestError> {
    for (key, item) in map {
        if key.chars().any(|c| c as u32 > 0xFFFF) {
            return Err(DigestError::KeyOutsideBmp {
                at: if at.is_empty() {
                    "(root)".to_string()
                } else {
                    at.to_string()
                },
                key: key.clone(),
            });
        }
        let child = if at.is_empty() {
            key.clone()
        } else {
            format!("{at}.{key}")
        };
        check_numbers(item, &child)?;
    }
    Ok(())
}

//! The embodiment descriptor, on the robot (§5).
//!
//! Until now the daemon has had no idea what body it is running on. That single absence is what
//! kept three separate things from being written: `admit_package` off the install path, because
//! `DeviceCapacity` needs the body's capabilities; the black box's `embodiment` digest at zero,
//! because a recording carrying an invented one could be replayed against the wrong machine; and
//! the console's guards and transport panels reporting the whole body absent rather than the one
//! piece actually missing.
//!
//! # It arrives as a file, and it has to
//!
//! §4 gives the driver a `describe()` that returns exactly this document, and calling it here is
//! impossible: §33.1 says `skynetd` never touches an actuator, the workspace forbids it a
//! dependency on `skynet-rhal`, and CI asserts the edge is absent. Loading a driver to ask it what
//! body this is would hand the supervisor a vtable of function pointers into vendor code, which is
//! the whole of what that rule exists to prevent.
//!
//! So the descriptor is written to `/data/state/embodiment.json` — at provisioning (§50), by
//! `skynet descriptor from-urdf`, or by a vendor's own tooling — and this module reads it. The
//! daemon learns the body the same way it learns everything else about the world: from a file it
//! did not produce.
//!
//! # Three states, not two
//!
//! [`Body`] distinguishes **absent** from **malformed**, and that is §7.2's rule rather than
//! fastidiousness. "Nobody has told this robot what it is" and "somebody told it, and the answer
//! does not parse" imply completely different work: the first is a provisioning step nobody ran,
//! the second is a generator that emitted something wrong, and a console that rendered both as an
//! empty panel would be hiding the difference at exactly the moment it matters.
//!
//! A malformed descriptor is never partially believed. There is no field-by-field salvage here,
//! because a body description half of which parsed is a body nobody has established the shape of,
//! and every consumer downstream would be reasoning about a machine that does not exist.
//!
//! # The digest is over canonical JSON, not over the file
//!
//! Two robots handed the same descriptor with different whitespace must agree about which body they
//! are, and hashing the bytes on disk would give them two answers. So the digest is taken over
//! [`skynet_json`]'s canonical form, under its own domain separator, which is the same rule
//! [`skynet_contracts::digest::version_digest`] applies to a package manifest and for the same
//! reason.

use std::fs;
use std::path::Path;

use skynet_contracts::admission::SignatureState;
use skynet_contracts::embodiment::{
    validate_frame_bindings, validate_frames, EmbodimentDescriptor,
};
use skynet_contracts::json::descriptor_from_json;
use skynet_json::Json;

use crate::Layout;

/// The domain separator for an embodiment digest.
///
/// Present so a digest over a body can never collide with a digest over a package version, however
/// similar two documents look. `skynet.version.v1` is the other one.
pub const EMBODIMENT_DIGEST_DOMAIN: &str = "skynet.embodiment.v1";

/// What this robot knows about its own body.
#[derive(Debug, Clone)]
pub enum Body {
    /// No descriptor has been written. A provisioning step nobody ran.
    Absent,
    /// A descriptor exists and could not be used, with the reason.
    ///
    /// Carries `detail` rather than only a code because the two audiences are an operator reading
    /// a console and a vendor reading a generator's output, and both need to know which field.
    Malformed {
        /// Why it could not be used, in a sentence.
        detail: String,
    },
    /// A descriptor this robot can act on.
    Present(Box<Known>),
}

/// A descriptor that loaded, with everything derived from it once.
#[derive(Debug, Clone)]
pub struct Known {
    /// The parsed descriptor.
    pub descriptor: EmbodimentDescriptor,
    /// The document as read, kept for the digest and for anything that needs a field this
    /// edition's struct does not carry.
    pub document: Json,
    /// sha256 over the canonical form, under [`EMBODIMENT_DIGEST_DOMAIN`].
    pub digest: [u8; 32],
    /// The same digest, lowercase hex.
    pub digest_hex: String,
}

impl Known {
    /// Whether the frame graph is present and usable, and what the pose guards would get from it.
    ///
    /// Three answers, not two. `None` is "no frame graph", which §44.5 already covers; `Some(0)` is
    /// a graph that is present and carries no collision geometry, which is a *different* thing an
    /// operator should see; and `Some(n)` is what `self_collision` would have to work with.
    pub fn collision_bodies(&self) -> Option<usize> {
        let graph = self.descriptor.frames.as_ref()?;
        Some(
            graph
                .nodes
                .iter()
                .filter(|node| node.collision && node.collision_shape.is_some())
                .count(),
        )
    }

    /// How many frames name a joint, which is what `workspace` would be able to move.
    pub fn dynamic_frames(&self) -> Option<usize> {
        let graph = self.descriptor.frames.as_ref()?;
        Some(
            graph
                .nodes
                .iter()
                .filter(|node| node.joint.is_some())
                .count(),
        )
    }
}

impl Body {
    /// The descriptor, when there is one.
    pub fn known(&self) -> Option<&Known> {
        match self {
            Body::Present(known) => Some(known),
            _ => None,
        }
    }

    /// The digest to record, and it is zero when there is no body.
    ///
    /// Zero rather than an error, because the black box has to open on a robot that has not been
    /// told what it is — §28 preallocates the ring precisely so nothing can silence the recorder.
    /// The zero is readable as "no body was known", which is true and is not a value anybody could
    /// mistake for a digest.
    pub fn digest_or_zero(&self) -> [u8; 32] {
        self.known().map(|k| k.digest).unwrap_or([0u8; 32])
    }

    /// Whether the body declares itself simulated (§35.3, §70).
    ///
    /// `false` when unknown, and that direction is deliberate. §52 makes `simulator` the one
    /// descriptor field where absent means `false`, because the obligation is asymmetric: a
    /// simulator presented as a robot is the failure, and a robot presented as a robot is not.
    pub fn simulator(&self) -> bool {
        self.known().is_some_and(|k| k.descriptor.simulator)
    }

    /// A one-line account for a report or a console.
    pub fn summary(&self) -> String {
        match self {
            Body::Absent => "no embodiment descriptor on this robot".to_string(),
            Body::Malformed { detail } => format!("the descriptor could not be used: {detail}"),
            Body::Present(known) => format!(
                "{} — {} joints, {}",
                known.descriptor.robot_slug,
                known.descriptor.joints.len(),
                match known.descriptor.frames.as_ref() {
                    None => "no frame graph".to_string(),
                    Some(graph) => format!("{} frames", graph.nodes.len()),
                }
            ),
        }
    }
}

/// Read the descriptor this robot was provisioned with.
///
/// Never fails: an unreadable file, a document that is not JSON and a body whose frame graph names
/// a joint it does not declare are all *answers* about this robot rather than errors in the daemon,
/// and every one of them is something the console has to be able to show.
pub fn load(layout: &Layout) -> Body {
    read(&layout.embodiment())
}

/// The same, against a path.
pub fn read(path: &Path) -> Body {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Body::Absent,
        Err(error) => {
            return Body::Malformed {
                detail: format!(
                    "{} could not be read: {error}. The file is there and this daemon cannot open \
                     it, which is a permissions or a storage question rather than a schema one.",
                    path.display()
                ),
            }
        }
    };

    // An empty file is absent rather than malformed. It is what a truncated write leaves behind,
    // and reporting "the descriptor does not parse" would send somebody looking for a schema error
    // in a file with nothing in it.
    if text.trim().is_empty() {
        return Body::Absent;
    }

    let document = match Json::parse(&text) {
        Ok(document) => document,
        Err(error) => {
            return Body::Malformed {
                detail: format!("{} is not valid JSON: {error:?}", path.display()),
            }
        }
    };

    let descriptor = match descriptor_from_json(&document) {
        Ok(descriptor) => descriptor,
        Err(error) => {
            return Body::Malformed {
                detail: format!(
                    "{} does not decode as an embodiment descriptor: {error:?}. It needs at least \
                     `robot_slug`, `base` and `dof`.",
                    path.display()
                ),
            }
        }
    };

    // The frame graph, held to §44.3 in both halves — the tree, and the tree against the joint
    // list. A graph whose `joint` names nothing is the defect that presents as a limb which never
    // resolves and two pose guards tripping on it every cycle, and finding that here rather than in
    // cycle is the whole reason `validate_frame_bindings` exists.
    if let Some(graph) = &descriptor.frames {
        if let Some(fault) = validate_frames(graph) {
            return Body::Malformed {
                detail: format!(
                    "the frame graph is not usable [{}]: {}",
                    fault.code, fault.detail
                ),
            };
        }
        let joints: Vec<&str> = descriptor.joints.iter().map(|j| j.name.as_str()).collect();
        if let Some(fault) = validate_frame_bindings(graph, &joints) {
            return Body::Malformed {
                detail: format!(
                    "the frame graph does not match the joint list [{}]: {}",
                    fault.code, fault.detail
                ),
            };
        }
    }

    let digest = match digest_of(&document) {
        Ok(digest) => digest,
        Err(detail) => return Body::Malformed { detail },
    };

    Body::Present(Box::new(Known {
        descriptor,
        document,
        digest,
        digest_hex: hex(&digest),
    }))
}

/// The digest of a descriptor document.
///
/// Over the canonical form so that two robots handed the same body with different whitespace agree
/// about which body it is, and under a domain separator so it cannot collide with a version digest.
pub fn digest_of(document: &Json) -> Result<[u8; 32], String> {
    let canonical = document.to_canonical().map_err(|error| {
        format!(
            "the descriptor cannot be written canonically ({error:?}), so it has no digest. A \
             non-finite number is the usual cause, and it is a number no body has."
        )
    })?;

    let mut hasher = skynet_sha256::Sha256::new();
    hasher.update(EMBODIMENT_DIGEST_DOMAIN.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical.as_bytes());
    Ok(hasher.finish())
}

fn hex(digest: &[u8; 32]) -> String {
    let bytes = skynet_sha256::hex_of(digest);
    // `hex_of` produces ASCII, so this cannot fail; `from_utf8_lossy` rather than an unwrap so a
    // future change to that function cannot panic a daemon at start-up.
    String::from_utf8_lossy(&bytes).into_owned()
}

/// What is known about the vendor's signature over this descriptor (§5).
///
/// The signature is detached, in a sibling file, for the reason
/// [`Layout::embodiment_signature`](crate::Layout::embodiment_signature) gives: the digest is over
/// the descriptor's canonical form, so a signature written inside it would change the digest it
/// signs.
///
/// A signature file that exists and cannot be read is reported as [`SignatureState::Failed`] rather
/// than as absent. Somebody put it there, and "the file you wrote is unusable" is a different job
/// from "nobody signed this".
pub fn signature_verdict(layout: &Layout, body: &Body) -> crate::trust::Verdict {
    let Body::Present(known) = body else {
        return crate::trust::Verdict {
            state: SignatureState::Absent,
            signer: None,
            detail: "there is no descriptor to have been signed".to_string(),
        };
    };

    let signed = match crate::trust::read_detached(&layout.embodiment_signature()) {
        Ok(signed) => signed,
        Err(detail) => {
            return crate::trust::Verdict {
                state: SignatureState::Failed,
                signer: None,
                detail: format!("a signature file is present and unusable — {detail}"),
            }
        }
    };

    crate::trust::TrustStore::load(layout).check(
        &crate::trust::embodiment_message(&known.digest_hex),
        signed.as_ref(),
    )
}

/// What the API and the console are told.
///
/// `signature` comes from [`signature_verdict`] rather than being computed here, so that this stays
/// a rendering function over a body that may have been read from anywhere — the tests read one from
/// a path, and the daemon reads one from its layout.
pub fn to_json(body: &Body, signature: &crate::trust::Verdict) -> Json {
    let mut document = Json::obj();
    match body {
        Body::Absent => {
            document
                .set("state", Json::from("absent"))
                .set("digest", Json::Null)
                .set(
                    "detail",
                    Json::from(
                        "No embodiment descriptor has been written to this robot. §4 gives the \
                         driver a describe() that returns exactly this document, and §33.1 forbids \
                         skynetd from loading a driver to ask — so it arrives as a file, at \
                         provisioning or from `skynet descriptor from-urdf`.",
                    ),
                );
        }
        Body::Malformed { detail } => {
            // A distinct state rather than `absent` with a note. Somebody wrote a descriptor here
            // and it did not work, which is a different job from nobody having written one.
            document
                .set("state", Json::from("malformed"))
                .set("digest", Json::Null)
                .set("detail", Json::from(detail.as_str()));
        }
        Body::Present(known) => {
            let d = &known.descriptor;
            document
                .set("state", Json::from("present"))
                .set("digest", Json::from(known.digest_hex.as_str()))
                .set("robot_slug", Json::from(d.robot_slug.as_str()))
                .set("base", Json::from(d.base.as_str()))
                .set("dof", Json::Num(d.dof))
                .set("joints", Json::Num(d.joints.len() as f64))
                .set("simulator", Json::Bool(d.simulator))
                // §5 has the vendor sign the descriptor and `embodiment.descriptor.signed` is an
                // SCTS test. The digest identifies the *file*; the signature is what makes it
                // identify a body somebody vouched for, and the two are reported separately so a
                // reader cannot mistake one for the other.
                .set("signature", Json::from(signature.state.as_str()))
                .set(
                    "signer",
                    match &signature.signer {
                        Some(label) => Json::from(label.as_str()),
                        None => Json::Null,
                    },
                )
                .set(
                    "signature_note",
                    Json::from(if signature.detail.is_empty() {
                        "The digest is over the canonical form of this document, and a key this \
                         robot trusts has signed it."
                            .to_string()
                    } else {
                        format!(
                            "The digest is over the canonical form of this document and identifies \
                             the file. Its provenance: {}.",
                            signature.detail
                        )
                    }),
                );

            let mut frames = Json::obj();
            match (known.dynamic_frames(), known.collision_bodies()) {
                (Some(dynamic), Some(bodies)) => {
                    let graph = d.frames.as_ref().expect("counted from a graph");
                    frames
                        .set("present", Json::Bool(true))
                        .set("nodes", Json::Num(graph.nodes.len() as f64))
                        .set("root", Json::from(graph.root.as_str()))
                        .set("convention", Json::from(graph.convention.as_str()))
                        .set("dynamic", Json::Num(dynamic as f64))
                        .set("collision_bodies", Json::Num(bodies as f64));
                }
                _ => {
                    frames.set("present", Json::Bool(false)).set(
                        "detail",
                        Json::from(
                            "This body declares no frame graph, so the workspace and \
                             self_collision guards have no tree to walk (§44.5).",
                        ),
                    );
                }
            }
            document.set("frames", frames);
        }
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique path per call.
    ///
    /// Named after a counter rather than after the descriptor, because two tests handed the same
    /// descriptor would otherwise share a directory — and the first version of this helper cleared
    /// that directory on entry, so the two raced. It passed for as long as the scheduling happened
    /// to suit it.
    fn body_from(text: &str) -> Body {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static NEXT: AtomicUsize = AtomicUsize::new(0);

        let dir = std::env::temp_dir().join("skynetd-embodiment-tests");
        fs::create_dir_all(&dir).expect("scratch");
        let path = dir.join(format!("{}.json", NEXT.fetch_add(1, Ordering::Relaxed)));
        fs::write(&path, text).expect("write");
        read(&path)
    }

    const ARM: &str = r#"{
        "robot_slug": "bench-arm",
        "base": "fixed",
        "dof": 1,
        "joints": [{"name": "shoulder", "type": "revolute", "control_modes": ["position"]}]
    }"#;

    #[test]
    fn a_robot_nobody_told_is_absent_rather_than_broken() {
        let dir = std::env::temp_dir().join("skynetd-embodiment-missing");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch");
        assert!(matches!(read(&dir.join("nothing.json")), Body::Absent));

        // A truncated write leaves an empty file, and reporting a schema error in a file with
        // nothing in it sends somebody looking for the wrong thing.
        assert!(matches!(body_from("   \n"), Body::Absent));
    }

    #[test]
    fn a_descriptor_that_does_not_parse_is_malformed_and_says_which_way() {
        let Body::Malformed { detail } = body_from("{not json") else {
            panic!("a document that is not JSON is malformed");
        };
        assert!(detail.contains("not valid JSON"), "{detail}");

        let Body::Malformed { detail } = body_from(r#"{"nothing": true}"#) else {
            panic!("a document that is not a descriptor is malformed");
        };
        assert!(detail.contains("robot_slug"), "{detail}");
    }

    #[test]
    fn a_body_that_loads_carries_its_digest_and_its_shape() {
        let Body::Present(known) = body_from(ARM) else {
            panic!("this descriptor loads");
        };
        assert_eq!(known.descriptor.robot_slug, "bench-arm");
        assert_eq!(known.descriptor.joints.len(), 1);
        assert_eq!(known.digest_hex.len(), 64);
        assert!(known.digest_hex.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(known.digest, [0u8; 32]);
        // No frame graph is not a fault. §44.5 has the two pose guards report unavailable.
        assert_eq!(known.collision_bodies(), None);
    }

    /// The property the digest exists for.
    ///
    /// Two robots handed the same body with different whitespace have to agree about which body it
    /// is. Hashing the file would give them two answers, and a fleet where one robot's recording
    /// cannot be matched to another's is a fleet where §56.4's replay argument stops working.
    #[test]
    fn whitespace_does_not_change_which_body_this_is() {
        let compact = body_from(
            r#"{"robot_slug":"bench-arm","base":"fixed","dof":1,"joints":[{"name":"shoulder","type":"revolute","control_modes":["position"]}]}"#,
        );
        let spaced = body_from(ARM);
        assert_eq!(
            compact.known().expect("compact").digest,
            spaced.known().expect("spaced").digest,
        );
    }

    /// A different body is a different digest, and the key ordering does not decide it.
    #[test]
    fn a_different_body_is_a_different_digest() {
        let other = body_from(&ARM.replace("bench-arm", "bench-arm-2"));
        assert_ne!(
            body_from(ARM).known().expect("arm").digest,
            other.known().expect("other").digest,
        );
    }

    /// The digest is domain-separated, so it cannot be confused with a package version's.
    #[test]
    fn the_digest_is_not_a_bare_hash_of_the_canonical_form() {
        let document = Json::parse(ARM).expect("parses");
        let canonical = document.to_canonical().expect("canonical");
        assert_ne!(
            digest_of(&document).expect("digest"),
            skynet_sha256::digest(canonical.as_bytes()),
            "an undomained digest could collide with a digest over something else"
        );
    }

    /// The check this session's `validate_frame_bindings` made possible, applied where it matters.
    ///
    /// A frame graph naming a joint the body does not declare is a limb whose pose never resolves,
    /// and both pose guards trip on it every cycle. Refusing the descriptor at load turns that into
    /// a provisioning error somebody can read.
    #[test]
    fn a_frame_graph_that_names_a_joint_the_body_does_not_declare_is_refused() {
        let with_frames = r#"{
            "robot_slug": "bench-arm",
            "base": "fixed",
            "dof": 1,
            "joints": [{"name": "shoulder", "type": "revolute", "control_modes": ["position"]}],
            "frames": {
                "root": "base_link",
                "convention": "rep103",
                "nodes": [
                    {"name": "base_link", "parent": null, "kind": "root", "collision": false},
                    {"name": "upper", "parent": "base_link", "kind": "dynamic",
                     "joint": "shoulder_pitch", "joint_type": "revolute", "axis": [0, 1, 0],
                     "translation": [0, 0, 0.1], "rotation": [0, 0, 0, 1],
                     "source": "descriptor", "collision": false}
                ]
            }
        }"#;
        let Body::Malformed { detail } = body_from(with_frames) else {
            panic!("a graph naming an undeclared joint is not a usable body");
        };
        assert!(detail.contains("dangling_joint"), "{detail}");
        assert!(detail.contains("shoulder_pitch"), "{detail}");
    }

    /// A verdict for the rendering tests, which are about the body rather than about the signer.
    fn unsigned() -> crate::trust::Verdict {
        crate::trust::Verdict {
            state: SignatureState::Absent,
            signer: None,
            detail: "nothing accompanied it".to_string(),
        }
    }

    #[test]
    fn the_three_states_are_distinguishable_on_the_wire() {
        let none = unsigned();
        assert_eq!(
            to_json(&Body::Absent, &none)
                .get("state")
                .and_then(Json::as_str),
            Some("absent")
        );
        assert_eq!(
            to_json(&Body::Malformed { detail: "x".into() }, &none)
                .get("state")
                .and_then(Json::as_str),
            Some("malformed")
        );
        assert_eq!(
            to_json(&body_from(ARM), &none)
                .get("state")
                .and_then(Json::as_str),
            Some("present")
        );

        // Neither absence carries a digest, and the one that loads does.
        assert_eq!(
            to_json(&Body::Absent, &none).get("digest"),
            Some(&Json::Null)
        );
        assert!(to_json(&body_from(ARM), &none)
            .get("digest")
            .and_then(Json::as_str)
            .is_some_and(|d| d.len() == 64));
    }

    /// §5. A digest identifies the file; a signature identifies who stands behind it, and the wire
    /// keeps the two apart rather than letting one imply the other.
    #[test]
    fn a_descriptor_reports_its_provenance_separately_from_its_digest() {
        let unsigned = to_json(&body_from(ARM), &unsigned());
        assert_eq!(
            unsigned.get("signature").and_then(Json::as_str),
            Some("absent")
        );
        assert_eq!(unsigned.get("signer"), Some(&Json::Null));
        assert!(unsigned
            .get("signature_note")
            .and_then(Json::as_str)
            .is_some_and(|note| note.contains("identifies the file")));

        let verified = to_json(
            &body_from(ARM),
            &crate::trust::Verdict {
                state: SignatureState::Verified,
                signer: Some("acme-robotics".to_string()),
                detail: String::new(),
            },
        );
        assert_eq!(
            verified.get("signature").and_then(Json::as_str),
            Some("verified")
        );
        assert_eq!(
            verified.get("signer").and_then(Json::as_str),
            Some("acme-robotics")
        );
    }

    /// A signature file that is there and unusable is not a descriptor nobody signed.
    #[test]
    fn an_unusable_signature_file_is_failed_rather_than_absent() {
        let dir = std::env::temp_dir().join(format!(
            "skynet-embodiment-sig-{}",
            std::process::id() as u64 + 7
        ));
        let layout = Layout::new(&dir);
        std::fs::create_dir_all(layout.state()).unwrap();
        std::fs::write(layout.embodiment(), ARM).unwrap();
        std::fs::write(layout.embodiment_signature(), "{ not json").unwrap();

        let body = load(&layout);
        let verdict = signature_verdict(&layout, &body);
        assert_eq!(verdict.state, SignatureState::Failed);
        assert!(
            verdict.detail.contains("present and unusable"),
            "{verdict:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// §52's one asymmetric field, and the direction it fails in.
    #[test]
    fn an_unknown_body_is_not_a_simulator() {
        assert!(!Body::Absent.simulator());
        assert!(!Body::Malformed { detail: "x".into() }.simulator());
        assert!(!body_from(ARM).simulator());
        assert!(
            body_from(&ARM.replace(r#""dof": 1"#, r#""dof": 1, "simulator": true"#)).simulator()
        );
    }
}

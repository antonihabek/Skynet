//! §57's `GET /os/releases`, on the robot — the manifest, and only the manifest.
//!
//! [`crate::ota`] has been able to answer §12's third rule since the twenty-first session and has
//! had nothing to answer it about, because *"nothing on this robot describes release `1.4.0`"* was
//! the only outcome a fleet could produce: `assess` reads a release manifest from
//! [`Layout::release`] and nothing ever put one there. This is what puts one there.
//!
//! # The bytes are still not here, and that is the point of splitting them
//!
//! §12's fetch is two transfers with nothing in common. A manifest is a few hundred bytes of JSON
//! that says *what an image would change*; a bundle is hundreds of megabytes that wants resume,
//! progress and §34.3's metering, which is [`skynet_contracts::bundle`]'s subject and an unwritten
//! piece of work.
//!
//! Fetching the small one first is not an ordering convenience. §12's third rule — *"an update that
//! changes the Safety Kernel or the envelope is refused without a fresh bench pass"* — is decided
//! **entirely from the manifest**, so a robot that has one can refuse an update it must not take
//! before spending a hundred megabytes of somebody's metered link discovering the same thing. The
//! expensive transfer is the one that should happen after the decision, and until now the decision
//! had no input.
//!
//! # A release manifest is immutable, and this refuses to pretend otherwise
//!
//! §12 binds a bench pass to a digest because *"evidence that is not bound to what it is evidence
//! of is not evidence."* A manifest is the other half of that binding: it says which components
//! these bytes change. A Hub serving **different content under one version** has broken it, exactly
//! as a Hub serving a different document under one generation breaks §11 — and the two get the same
//! treatment for the same reason, set out at length in [`skynet_contracts::intent`]:
//!
//! * **Adopt**, and the robot judges an update against a manifest that no longer describes the
//!   bundle a bench pass was taken against — invisibly, because everything looks right.
//! * **Refuse**, and the robot keeps what it has and says the Hub changed a release under it.
//!
//! One is a visible, actionable state and the other is an invisible wrong one, so it refuses.
//!
//! # It writes one manifest, not a catalogue
//!
//! The desired state names one version. Caching the rest would put statements about images this
//! robot will never install on its disk, where §32.2's rule — *never show a value you did not
//! receive* — has nothing to say about them and an operator listing the directory has to work out
//! which are real. One version asked for, one file written.

use skynet_json::Json;

use crate::layout::Layout;

/// What one pull did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pulled {
    /// Whether the Hub answered at all.
    pub reached: bool,
    /// Why not, when it did not. Empty on success.
    pub detail: String,
    /// How many manifests the Hub offered for this channel and architecture.
    pub offered: usize,
    /// Whether the one this robot wants was among them.
    pub matched: bool,
    /// Whether a manifest was written. False for an unchanged one, which is the ordinary poll.
    pub written: bool,
    /// What is wrong, when something is. Empty is nothing wrong.
    pub fault: String,
}

impl Pulled {
    /// A pull that never got an answer.
    fn unreachable(detail: impl Into<String>) -> Pulled {
        Pulled {
            reached: false,
            detail: detail.into(),
            offered: 0,
            matched: false,
            written: false,
            fault: String::new(),
        }
    }
}

/// What this binary was built for, which is what an image has to match.
///
/// `std::env::consts::ARCH` rather than `uname`: it is what this executable *is*, and a robot
/// running an aarch64 supervisor under emulation on an x86 host would have `uname` say the wrong
/// one. The question `GET /os/releases?arch=` asks is which image will boot the machine this code
/// is already running on.
pub fn arch() -> &'static str {
    std::env::consts::ARCH
}

/// Fetch the release catalogue and cache the manifest the desired state asked for (§12, §57).
///
/// `None` for an unpaired robot, and for a desired state that names no OS version — neither is a
/// failure. §50.2 ships a robot belonging to nobody, and a fleet that pins no OS version is a fleet
/// managing its images some other way; a loop that logged an error every few seconds for either
/// would train an operator to ignore the line that matters.
pub fn pull(layout: &Layout, channel: &str, version: &str) -> Option<Pulled> {
    let wanted = version.trim();
    if wanted.is_empty() {
        return None;
    }
    let (endpoint, _device_id, credential) = crate::identity::credential_for_sending(layout)?;

    let url = format!(
        "{}/os/releases?channel={}&arch={}",
        endpoint.trim_end_matches('/'),
        encode(channel.trim()),
        encode(arch()),
    );

    let answer = match crate::connectivity::get_json(&url, Some(&credential)) {
        Err(detail) => return Some(Pulled::unreachable(detail)),
        Ok(answer) => answer,
    };

    if !(200..300).contains(&answer.status) {
        let said = Json::parse(&answer.body)
            .ok()
            .and_then(|document| {
                document
                    .get("error")
                    .and_then(Json::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        return Some(Pulled::unreachable(format!(
            "the Hub answered HTTP {} to {url}{}",
            answer.status,
            if said.is_empty() {
                String::new()
            } else {
                format!(": {said}")
            }
        )));
    }

    Some(adopt(layout, &answer.body, wanted))
}

/// Decide and, where the rules above allow, write.
///
/// Separated from the fetch for [`crate::intent::adopt`]'s reason: every branch below is reachable
/// only through a Hub that answers a particular way, and a suite that could not produce those
/// answers would be a suite that tests the happy path.
pub fn adopt(layout: &Layout, body: &str, wanted: &str) -> Pulled {
    let document = match Json::parse(body) {
        Err(error) => {
            return Pulled {
                reached: true,
                detail: String::new(),
                offered: 0,
                matched: false,
                written: false,
                fault: format!(
                    "the release catalogue does not parse ({error}). Nothing has been written, and \
                     this robot goes on judging updates against the manifests it already holds."
                ),
            };
        }
        Ok(document) => document,
    };

    let releases = document.arr_or_empty("releases");
    let offered = releases.len();

    // The manifest, as the contract defines it, and nothing else from the row. A Hub row carries
    // fields this robot has no business reading — who published it, an artifact id — and copying
    // the whole row to disk would invite a future edition to start depending on one.
    let Some(row) = releases.iter().find(|row| {
        row.get("version")
            .and_then(Json::as_str)
            .is_some_and(|version| version.trim() == wanted)
    }) else {
        return Pulled {
            reached: true,
            detail: String::new(),
            offered,
            matched: false,
            written: false,
            fault: format!(
                "the Hub offers {offered} release(s) on this channel for {} and none of them is \
                 `{wanted}`, which is the version this robot's desired state asks for. Either the \
                 image has not been published for this architecture or the desired state names a \
                 version that does not exist.",
                arch()
            ),
        };
    };

    let manifest = manifest_of(row);
    let declared = manifest
        .get("version")
        .and_then(Json::as_str)
        .unwrap_or_default();
    if declared != wanted {
        // Cannot happen through the search above, and it is checked anyway: `read_release` refuses
        // a manifest filed under a name it disagrees with, and writing one would be manufacturing
        // exactly the state that refusal exists for.
        return Pulled {
            reached: true,
            detail: String::new(),
            offered,
            matched: true,
            written: false,
            fault: format!(
                "the Hub's entry for `{wanted}` declares itself `{declared}`. Nothing has been \
                 written: a manifest filed under a name it disagrees with is one this robot would \
                 refuse to read anyway."
            ),
        };
    }

    let Ok(canonical) = manifest.to_canonical() else {
        return Pulled {
            reached: true,
            detail: String::new(),
            offered,
            matched: true,
            written: false,
            fault: "the Hub's manifest could not be canonicalised, so there is nothing to write \
                    that this robot could digest the same way twice."
                .to_string(),
        };
    };

    // The immutability rule. See the header: a release that changed under a version is the same
    // mistake as §11's reused generation, and it gets the same refusal.
    let path = layout.release(wanted);
    if let Ok(existing) = std::fs::read_to_string(&path) {
        if existing.trim() == canonical.trim() {
            return Pulled {
                reached: true,
                detail: String::new(),
                offered,
                matched: true,
                written: false,
                fault: String::new(),
            };
        }
        return Pulled {
            reached: true,
            detail: String::new(),
            offered,
            matched: true,
            written: false,
            fault: format!(
                "the Hub is serving a different manifest for release `{wanted}`, which this robot \
                 already holds. §12 binds a bench pass to the bytes it was taken against, so a \
                 release that changes under its own version would have this robot judging an \
                 update against a description of some other image. The held manifest is kept. \
                 Publish the change as a new version and this robot will take it."
            ),
        };
    }

    match crate::observe::write_atomic(&path, canonical.as_bytes()) {
        Ok(()) => Pulled {
            reached: true,
            detail: String::new(),
            offered,
            matched: true,
            written: true,
            fault: String::new(),
        },
        Err(error) => Pulled {
            reached: true,
            detail: String::new(),
            offered,
            matched: true,
            written: false,
            fault: format!("{} could not be written: {error}", path.display()),
        },
    }
}

/// The five fields [`skynet_contracts::ota::BundleManifest`] is made of, and no others.
///
/// Taken from the row's own `manifest` when the Hub sent one — it does — and reassembled from the
/// row otherwise, so a Hub that stops sending the narrow shape does not stop a fleet updating.
fn manifest_of(row: &Json) -> Json {
    if let Some(manifest) = row.get_present("manifest") {
        if manifest.get("version").and_then(Json::as_str).is_some() {
            return manifest.clone();
        }
    }
    let mut out = Json::obj();
    out.set("channel", copy_str(row, "channel"))
        .set("compatible", copy_arr(row, "compatible"))
        .set("changes", copy_arr(row, "changes"))
        .set("digest", copy_str(row, "digest"))
        .set("version", copy_str(row, "version"));
    out
}

fn copy_str(row: &Json, key: &str) -> Json {
    Json::Str(
        row.get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_string(),
    )
}

fn copy_arr(row: &Json, key: &str) -> Json {
    Json::Arr(
        row.arr_or_empty(key)
            .iter()
            .filter_map(Json::as_str)
            .map(|entry| Json::Str(entry.to_string()))
            .collect(),
    )
}

/// Percent-encode a query parameter.
///
/// Written out rather than pulled in, because this workspace has no third-party dependencies and
/// that is a property worth keeping for four lines. The allow-list is deliberately narrow: a
/// channel is a word an operator typed, and anything outside `[A-Za-z0-9._~-]` is encoded rather
/// than passed through — a channel containing `&` would otherwise become a second parameter.
fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'~' | b'-' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// One pull as an API answer, for `GET /releases` and the console.
pub fn to_json(pulled: &Option<Pulled>) -> Json {
    let mut out = Json::obj();
    match pulled {
        None => {
            out.set("asked", Json::Bool(false)).set(
                "note",
                Json::Str(
                    "this robot is either unpaired (§10) or has no OS version in its desired \
                     state, so there is no release catalogue to ask for. Neither is a fault."
                        .to_string(),
                ),
            );
        }
        Some(pulled) => {
            out.set("asked", Json::Bool(true))
                .set("reached", Json::Bool(pulled.reached))
                .set("detail", Json::Str(pulled.detail.clone()))
                .set("offered", Json::Num(pulled.offered as f64))
                .set("matched", Json::Bool(pulled.matched))
                .set("written", Json::Bool(pulled.written))
                .set("fault", Json::Str(pulled.fault.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TempRoot;

    fn layout(label: &str) -> (TempRoot, Layout) {
        let root = TempRoot::new(label);
        let layout = root.layout();
        std::fs::create_dir_all(layout.os().join("releases")).unwrap();
        (root, layout)
    }

    fn catalogue(version: &str, changes: &str) -> String {
        format!(
            r#"{{"releases":[{{"version":"{version}","channel":"stable","arch":"x86_64",
               "digest":"sha256:aa","compatible":["g1"],"changes":[{changes}],
               "manifest":{{"version":"{version}","channel":"stable","digest":"sha256:aa",
               "compatible":["g1"],"changes":[{changes}]}}}}]}}"#
        )
    }

    #[test]
    fn a_manifest_the_desired_state_asked_for_is_written() {
        let (_root, layout) = layout("releases");
        let pulled = adopt(&layout, &catalogue("1.4.0", "\"rhal_driver\""), "1.4.0");
        assert!(
            pulled.reached && pulled.matched && pulled.written,
            "{pulled:?}"
        );
        assert_eq!(pulled.fault, "");
        assert!(layout.release("1.4.0").exists());
    }

    #[test]
    fn an_unchanged_manifest_is_not_rewritten() {
        // The ordinary poll. A daemon that rewrote this every pass would churn a file whose mtime
        // an operator reads as "somebody changed what this robot may install".
        let (_root, layout) = layout("releases");
        let body = catalogue("1.4.0", "\"rhal_driver\"");
        assert!(adopt(&layout, &body, "1.4.0").written);
        let second = adopt(&layout, &body, "1.4.0");
        assert!(!second.written);
        assert_eq!(second.fault, "");
    }

    #[test]
    fn a_release_that_changed_under_its_version_is_refused() {
        // §12 binds a bench pass to the bytes it was taken against. A manifest that changes under
        // one version would have this robot judging an update against a description of some other
        // image — and it would look right from every surface.
        let (_root, layout) = layout("releases");
        adopt(&layout, &catalogue("1.4.0", "\"rhal_driver\""), "1.4.0");
        let second = adopt(&layout, &catalogue("1.4.0", "\"safety_kernel\""), "1.4.0");
        assert!(!second.written);
        assert!(second.fault.contains("different manifest"), "{second:?}");
        // The held manifest is the one still on disk.
        let held = std::fs::read_to_string(layout.release("1.4.0")).unwrap();
        assert!(held.contains("rhal_driver"));
        assert!(!held.contains("safety_kernel"));
    }

    #[test]
    fn a_version_the_hub_does_not_have_is_reported_and_not_invented() {
        let (_root, layout) = layout("releases");
        let pulled = adopt(&layout, &catalogue("1.4.0", "\"rhal_driver\""), "1.5.0");
        assert!(pulled.reached);
        assert!(!pulled.matched && !pulled.written);
        assert!(pulled.fault.contains("1.5.0"));
        assert!(!layout.release("1.5.0").exists());
    }

    #[test]
    fn a_catalogue_that_does_not_parse_writes_nothing() {
        let (_root, layout) = layout("releases");
        let pulled = adopt(&layout, "not json", "1.4.0");
        assert!(pulled.reached);
        assert!(!pulled.written);
        assert!(pulled.fault.contains("does not parse"));
    }

    #[test]
    fn a_channel_with_an_ampersand_cannot_become_a_second_parameter() {
        assert_eq!(encode("stable"), "stable");
        assert_eq!(encode("beta&arch=x86_64"), "beta%26arch%3Dx86_64");
        assert_eq!(encode("a b"), "a%20b");
    }
}

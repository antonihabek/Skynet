//! The suite for the half that can be wrong about the world.
//!
//! [`skynetd`]'s own suite proves the reconciler converges and is idempotent, against a model. This
//! one proves the things a model cannot: that a digest mismatch is refused, that an activation is a
//! rename, that garbage collection cannot reach the version a rollback would return to, that a
//! console cannot be bound to a routable address, and that the two routes §39.1 leaves
//! unauthenticated are the only two.
//!
//! Most of it runs anywhere. The parts that need POSIX — symlinks, Unix sockets — are `cfg(unix)`,
//! and are what the container job in CI exists to run.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use skynet_contracts::opstate::OperatingState;
use skynet_json::Json;
use skynetd::actual::{UnitState, UnitStatus};
use skynetd::desired::PackageKind;
use skynetd::reconcile::SafeState;

use crate::api::{self, Context};
use crate::auth::{requirement, Credential, Credentials, Role};
use crate::http;
use crate::launch::{self, CpuError, Cpus, Limits, RecordingLauncher, SystemdLauncher, Unit};
use crate::layout::{parse_digest, Layout, PackageId};
use crate::observe::FixedSlots;
use crate::runtime::{Daemon, DesiredSource, Outcome};
use crate::store::Store;

// ── A temporary root ─────────────────────────────────────────────────────────
//
// No `tempfile` crate, because the workspace has no third-party dependencies. This is the whole of
// what one is needed for: a directory nobody else is using, removed on drop.

static COUNTER: AtomicU64 = AtomicU64::new(0);

pub(crate) struct TempRoot(pub PathBuf);

impl TempRoot {
    pub(crate) fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("skynetd-io-{}-{}-{}", std::process::id(), label, n));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp root");
        Self(path)
    }

    pub(crate) fn layout(&self) -> Layout {
        Layout::new(&self.0)
    }

    pub(crate) fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write a package tree and return the digest it hashes to.
fn write_package(dir: &std::path::Path, manifest: &Json, files: &[(&str, &str)]) -> String {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("manifest.json"),
        manifest.to_canonical().unwrap().as_bytes(),
    )
    .unwrap();
    let mut digest_files = Vec::new();
    for (path, body) in files {
        let target = dir.join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, body.as_bytes()).unwrap();
        digest_files.push(skynet_contracts::digest::DigestFile {
            path: (*path).to_string(),
            sha256: String::from_utf8_lossy(&skynet_sha256::hex_bytes(body.as_bytes()))
                .into_owned(),
        });
    }
    let kind = manifest
        .get_present("kind")
        .and_then(Json::as_str)
        .unwrap_or("policy");
    let version = manifest
        .get_present("version")
        .and_then(Json::as_str)
        .unwrap_or("1.0.0");
    skynet_contracts::digest::version_digest(kind, version, manifest, &digest_files).unwrap()
}

fn manifest(id: &str, version: &str, kind: &str) -> Json {
    let mut budget = Json::obj();
    budget
        .set("ram_mb", Json::Num(512.0))
        .set("cpu_cores", Json::Num(1.5));
    let mut runtime = Json::obj();
    runtime.set("entrypoint", Json::from("bin/run"));
    let mut m = Json::obj();
    m.set("schema_version", Json::Num(1.0))
        .set("id", Json::from(id))
        .set("version", Json::from(version))
        .set("kind", Json::from(kind))
        .set("isolation", Json::from("wasm"))
        .set("budget", budget)
        .set("runtime", runtime);
    m
}

// ── Identifiers, which are path components ───────────────────────────────────

#[test]
fn an_ordinary_package_id_parses() {
    let id = PackageId::parse("openrobot/grasp-v2").unwrap();
    assert_eq!(id.namespace(), "openrobot");
    assert_eq!(id.name(), "grasp-v2");
    assert_eq!(id.as_str(), "openrobot/grasp-v2");
}

/// The one that matters. A desired state arrives over the network and its ids become directories
/// under `/data`.
#[test]
fn an_id_that_would_leave_the_package_store_is_refused() {
    for hostile in [
        "../../etc/systemd/system",
        "..",
        "../etc",
        "a/../../b",
        "/etc/passwd",
        "openrobot/../../root",
        "openrobot/..",
        "openrobot/.",
        "openrobot/grasp/extra",
        "openrobot",
        "",
        "openrobot/",
        "/openrobot/grasp",
    ] {
        assert!(
            PackageId::parse(hostile).is_err(),
            "`{hostile}` should not be a package id"
        );
    }
}

#[test]
fn an_id_with_a_character_outside_the_grammar_is_refused() {
    for hostile in [
        "OpenRobot/grasp",
        "openrobot/grasp!",
        "openrobot/gr asp",
        "openrobot/grasp\u{0}",
        "openrobot/grasp\n",
        "openrobot/..grasp",
        "-openrobot/grasp",
        ".../...",
    ] {
        assert!(
            PackageId::parse(hostile).is_err(),
            "`{hostile}` should not be a package id"
        );
    }
}

#[test]
fn a_digest_has_exactly_one_spelling() {
    let hex = "a".repeat(64);
    assert!(parse_digest(&hex).is_ok());
    assert!(parse_digest(&format!("sha256:{hex}")).is_err());
    assert!(parse_digest(&hex.to_uppercase()).is_err());
    assert!(parse_digest(&hex[..63]).is_err());
    assert!(parse_digest("").is_err());
}

// ── The store ────────────────────────────────────────────────────────────────

#[test]
fn staging_refuses_content_that_does_not_hash_to_the_digest_asked_for() {
    let root = TempRoot::new("mismatch");
    let store = Store::new(root.layout());
    store.prepare().unwrap();

    let source = root.path().join("source");
    let manifest = manifest("openrobot/grasp", "1.0.0", "policy");
    let real = write_package(&source, &manifest, &[("bin/run", "#!/bin/sh\n")]);

    let id = PackageId::parse("openrobot/grasp").unwrap();
    let wrong = "b".repeat(64);
    let error = store
        .stage(&id, "policy", "1.0.0", &wrong, &source)
        .unwrap_err();
    let text = error.to_string();
    assert!(
        text.contains(&real),
        "the error names what it observed: {text}"
    );
    assert!(
        text.contains("not the bytes that were published"),
        "and says what that means: {text}"
    );
    assert!(!store.layout().blob(&real).exists(), "and stages nothing");
}

#[test]
fn staging_the_same_content_twice_is_the_same_blob() {
    let root = TempRoot::new("dedupe");
    let store = Store::new(root.layout());
    store.prepare().unwrap();

    let source = root.path().join("source");
    let manifest = manifest("openrobot/grasp", "1.0.0", "policy");
    let digest = write_package(&source, &manifest, &[("bin/run", "#!/bin/sh\n")]);
    let id = PackageId::parse("openrobot/grasp").unwrap();

    let first = store
        .stage(&id, "policy", "1.0.0", &digest, &source)
        .unwrap();
    let second = store
        .stage(&id, "policy", "1.0.0", &digest, &source)
        .unwrap();
    assert_eq!(first.digest, second.digest);
    assert_eq!(
        std::fs::read_dir(store.layout().blobs()).unwrap().count(),
        1,
        "one blob, not two"
    );
}

/// The store reports the bytes and nothing about who vouched for them.
///
/// It used to carry a `signature: NotChecked`, which was honest while nothing could check one. Now
/// that something can, the field is gone rather than filled in: a content-addressed blob is the
/// same blob whoever signed it, and a verdict recorded beside it would be a verdict that outlives
/// the trust store it came from. The answer lives in `crate::trust`, computed per install.
#[test]
fn a_staged_package_carries_no_verdict_about_its_signer() {
    let root = TempRoot::new("signature");
    let store = Store::new(root.layout());
    store.prepare().unwrap();
    let source = root.path().join("source");
    let manifest = manifest("openrobot/grasp", "1.0.0", "policy");
    let digest = write_package(&source, &manifest, &[("bin/run", "x")]);
    let id = PackageId::parse("openrobot/grasp").unwrap();
    let staged = store
        .stage(&id, "policy", "1.0.0", &digest, &source)
        .unwrap();
    assert_eq!(staged.digest, digest);

    // And the trust store, asked about it with nothing to go on, says `absent` rather than
    // anything that could be read as a pass.
    let trust = crate::trust::TrustStore::load(&root.layout());
    let verdict = trust.check(&crate::trust::package_message(&staged.digest), None);
    assert_eq!(
        verdict.state,
        skynet_contracts::admission::SignatureState::Absent
    );
}

#[cfg(unix)]
mod posix {
    use super::*;

    fn staged(store: &Store, root: &TempRoot, version: &str, body: &str) -> String {
        let source = root.path().join(format!("source-{version}"));
        let m = manifest("openrobot/grasp", version, "policy");
        let digest = write_package(&source, &m, &[("bin/run", body)]);
        let id = PackageId::parse("openrobot/grasp").unwrap();
        store
            .stage(&id, "policy", version, &digest, &source)
            .unwrap();
        digest
    }

    /// Found by running the daemon against real systemd: the unit failed `203/EXEC`, which is
    /// `EACCES`, because a file written with `fs::write` is `0644` and a policy's entry point has
    /// to be executable. A mode is not in the digest, so it is derived from the manifest — see the
    /// module documentation for why preserving the source's mode would have been worse.
    #[test]
    fn the_declared_entry_point_is_executable_and_nothing_else_is() {
        use std::os::unix::fs::PermissionsExt;

        let root = TempRoot::new("modes");
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let source = root.path().join("source");
        let m = manifest("openrobot/grasp", "1.0.0", "policy");
        let digest = write_package(
            &source,
            &m,
            &[
                (
                    "bin/run",
                    "#!/bin/sh
exit 0
",
                ),
                ("weights.bin", "data"),
            ],
        );
        let id = PackageId::parse("openrobot/grasp").unwrap();
        store
            .stage(&id, "policy", "1.0.0", &digest, &source)
            .unwrap();

        let blob = store.layout().blob(&digest);
        let mode = |path: &str| {
            std::fs::metadata(blob.join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode("bin/run"), crate::store::ENTRYPOINT_MODE);
        assert_eq!(mode("weights.bin") & 0o111, 0, "nothing else is executable");
        assert_eq!(mode("manifest.json") & 0o111, 0);
    }

    /// A manifest naming an entry point the package does not contain is refused at staging rather
    /// than becoming a unit that fails to start an hour later.
    #[test]
    fn an_entry_point_the_package_does_not_contain_is_refused_at_staging() {
        let root = TempRoot::new("missing-entry");
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let source = root.path().join("source");
        let mut m = manifest("openrobot/grasp", "1.0.0", "policy");
        let mut runtime = Json::obj();
        runtime.set("entrypoint", Json::from("bin/absent"));
        m.set("runtime", runtime);
        let digest = write_package(&source, &m, &[("bin/run", "x")]);
        let id = PackageId::parse("openrobot/grasp").unwrap();
        let error = store
            .stage(&id, "policy", "1.0.0", &digest, &source)
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not contain"), "{error}");
        assert!(
            !store.layout().blob(&digest).exists(),
            "and nothing is staged"
        );
    }

    #[test]
    fn activation_points_the_current_link_at_the_blob() {
        let root = TempRoot::new("activate");
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let id = PackageId::parse("openrobot/grasp").unwrap();
        let digest = staged(&store, &root, "1.0.0", "a");

        store.activate(&id, &digest).unwrap();
        assert_eq!(store.active_digest(&id).as_deref(), Some(digest.as_str()));
        assert_eq!(store.installed(), vec![(id.clone(), digest.clone())]);

        // The link resolves to a real directory holding the manifest, which is the property that
        // makes `current` usable as a working directory.
        let resolved = std::fs::canonicalize(store.layout().current_link(&id)).unwrap();
        assert!(resolved.join("manifest.json").is_file());
    }

    #[test]
    fn activating_what_is_already_current_does_not_disturb_the_rollback_set() {
        let root = TempRoot::new("idempotent");
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let id = PackageId::parse("openrobot/grasp").unwrap();

        let one = staged(&store, &root, "1.0.0", "a");
        let two = staged(&store, &root, "2.0.0", "b");
        store.activate(&id, &one).unwrap();
        store.activate(&id, &two).unwrap();
        assert_eq!(store.rollback_digest(&id).as_deref(), Some(one.as_str()));

        // Twice more. A reconciler re-emitting an activation must not push `1.0.0` off the end.
        store.activate(&id, &two).unwrap();
        store.activate(&id, &two).unwrap();
        assert_eq!(store.rollback_digest(&id).as_deref(), Some(one.as_str()));
    }

    #[test]
    fn rollback_is_the_same_swap_in_reverse() {
        let root = TempRoot::new("rollback");
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let id = PackageId::parse("openrobot/grasp").unwrap();

        let one = staged(&store, &root, "1.0.0", "a");
        let two = staged(&store, &root, "2.0.0", "b");
        store.activate(&id, &one).unwrap();
        store.activate(&id, &two).unwrap();

        let back = store.rollback(&id).unwrap();
        assert_eq!(back, one);
        assert_eq!(store.active_digest(&id).as_deref(), Some(one.as_str()));
        // And forward again: the version rolled away from is what a roll-forward returns to.
        assert_eq!(store.rollback_digest(&id).as_deref(), Some(two.as_str()));
    }

    #[test]
    fn a_package_with_no_previous_version_cannot_be_rolled_back() {
        let root = TempRoot::new("no-rollback");
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let id = PackageId::parse("openrobot/grasp").unwrap();
        let one = staged(&store, &root, "1.0.0", "a");
        store.activate(&id, &one).unwrap();
        assert!(store.rollback_digest(&id).is_none());
        assert!(store.rollback(&id).is_err());
    }

    /// §37.3's rule, and the reason the rollback set is the links: this cannot be got wrong by
    /// forgetting a flag, because there is no path through `collect` that looks at anything but
    /// what is pointed at.
    #[test]
    fn collection_never_reaches_the_rollback_set() {
        let root = TempRoot::new("gc");
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let id = PackageId::parse("openrobot/grasp").unwrap();

        let one = staged(&store, &root, "1.0.0", "a");
        let two = staged(&store, &root, "2.0.0", "b");
        let three = staged(&store, &root, "3.0.0", "c");
        let four = staged(&store, &root, "4.0.0", "d");
        for digest in [&one, &two, &three, &four] {
            store.activate(&id, digest).unwrap();
        }

        let collected = store.collect().unwrap();
        // Current plus two previous are pinned; the oldest falls off and is collected.
        assert_eq!(collected, vec![one.clone()]);
        assert!(store.layout().blob(&four).is_dir());
        assert!(store.layout().blob(&three).is_dir());
        assert!(store.layout().blob(&two).is_dir());
        assert!(!store.layout().blob(&one).exists());

        // And it is idempotent: a second run collects nothing.
        assert!(store.collect().unwrap().is_empty());
    }

    #[test]
    fn collection_clears_a_staging_directory_a_crash_left_behind() {
        let root = TempRoot::new("gc-staging");
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let abandoned = store
            .layout()
            .blobs()
            .join(format!(".staging.{}", "c".repeat(64)));
        std::fs::create_dir_all(abandoned.join("weights")).unwrap();
        assert!(store.collect().unwrap().is_empty(), "it is not a blob");
        assert!(!abandoned.exists(), "but it does not survive");
    }

    #[test]
    fn removing_a_package_drops_its_links_and_then_its_blobs() {
        let root = TempRoot::new("remove");
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let id = PackageId::parse("openrobot/grasp").unwrap();
        let one = staged(&store, &root, "1.0.0", "a");
        store.activate(&id, &one).unwrap();

        store.remove(&id).unwrap();
        assert!(store.active_digest(&id).is_none());
        assert!(store.installed().is_empty());
        assert!(
            store.layout().blob(&one).is_dir(),
            "the blob is still there"
        );
        assert_eq!(store.collect().unwrap(), vec![one.clone()]);
        assert!(!store.layout().blob(&one).exists());
    }

    /// Two packages built from the same bytes are one blob (§2), so removing one must not delete
    /// the other's content.
    #[test]
    fn a_blob_two_packages_share_survives_one_of_them_being_removed() {
        let root = TempRoot::new("shared");
        let store = Store::new(root.layout());
        store.prepare().unwrap();

        let source = root.path().join("source");
        let m = manifest("openrobot/grasp", "1.0.0", "policy");
        let digest = write_package(&source, &m, &[("bin/run", "a")]);
        let first = PackageId::parse("openrobot/grasp").unwrap();
        let second = PackageId::parse("otherlab/grasp").unwrap();
        store
            .stage(&first, "policy", "1.0.0", &digest, &source)
            .unwrap();
        store.activate(&first, &digest).unwrap();
        store.activate(&second, &digest).unwrap();

        store.remove(&first).unwrap();
        assert!(store.collect().unwrap().is_empty());
        assert!(store.layout().blob(&digest).is_dir());
        assert_eq!(
            store.active_digest(&second).as_deref(),
            Some(digest.as_str())
        );
    }

    /// A link into somewhere other than the blob store must not keep a blob alive or be read as
    /// one — otherwise a link planted outside `blobs/` would name a directory GC then refuses to
    /// touch.
    #[test]
    fn a_link_pointing_outside_the_blob_store_is_not_an_installation() {
        let root = TempRoot::new("outside");
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let id = PackageId::parse("openrobot/grasp").unwrap();
        let links = store.layout().links(&id);
        std::fs::create_dir_all(&links).unwrap();
        let elsewhere = root.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::os::unix::fs::symlink(&elsewhere, links.join("current")).unwrap();

        assert!(store.active_digest(&id).is_none());
        assert!(store.installed().is_empty());
        assert!(store.pinned().is_empty());
    }
}

// ── The launcher ─────────────────────────────────────────────────────────────

#[test]
fn the_declared_budget_becomes_the_cgroup_limit_and_nothing_invents_one() {
    let m = manifest("openrobot/grasp", "1.0.0", "policy");
    let limits = Limits::from_manifest(&m);
    assert_eq!(limits.ram_mb, Some(512.0));
    assert_eq!(limits.cpu_cores, Some(1.5));

    let properties = limits.properties();
    assert!(properties.contains(&"MemoryMax=512M".to_string()));
    assert!(
        !properties.iter().any(|p| p.starts_with("MemoryHigh")),
        "§40.1: max, not high — a policy thrashing at a high watermark looks alive"
    );
    assert!(properties.contains(&"CPUQuota=150%".to_string()));
    assert!(properties.contains(&format!("TasksMax={}", launch::DEFAULT_TASKS_MAX)));
}

#[test]
fn a_budget_field_that_is_absent_stays_absent() {
    let empty = Json::obj();
    let limits = Limits::from_manifest(&empty);
    assert_eq!(limits.ram_mb, None);
    assert_eq!(limits.cpu_cores, None);
    let properties = limits.properties();
    assert!(!properties.iter().any(|p| p.starts_with("MemoryMax")));
    assert!(!properties.iter().any(|p| p.starts_with("CPUQuota")));
}

#[test]
fn the_rt_core_is_never_in_a_package_cpuset() {
    let cpus = Cpus {
        present: [0, 1, 2, 3].into_iter().collect(),
        isolated: [3].into_iter().collect(),
    };
    assert_eq!(cpus.allowed().unwrap(), vec![0, 1, 2]);
    assert_eq!(cpus.property().unwrap(), "AllowedCPUs=0 1 2");
}

/// The failure mode this refusal exists for: an omitted `AllowedCPUs=` is *every* core, so a
/// misconfiguration that isolated everything would place a policy on the safety kernel's core.
#[test]
fn a_machine_with_every_core_isolated_is_refused_rather_than_given_all_of_them() {
    let cpus = Cpus {
        present: [0, 1].into_iter().collect(),
        isolated: [0, 1].into_iter().collect(),
    };
    assert_eq!(cpus.allowed(), Err(CpuError::EverythingIsolated));
    assert!(cpus.property().is_err());

    let none = Cpus::default();
    assert_eq!(none.allowed(), Err(CpuError::NoCores));
}

#[test]
fn a_unit_that_cannot_be_given_a_cpuset_is_not_started() {
    let unit = Unit {
        id: "openrobot/grasp".to_string(),
        kind: PackageKind::Policy,
        program: PathBuf::from("/data/packages/blobs/x/bin/run"),
        args: Vec::new(),
        working_directory: PathBuf::from("/data/packages/blobs/x"),
        limits: Limits::default(),
        cpus: Cpus {
            present: [0].into_iter().collect(),
            isolated: [0].into_iter().collect(),
        },
    };
    let error = SystemdLauncher::system().command_line(&unit).unwrap_err();
    assert!(
        error.detail.contains("every CPU is isolated"),
        "{}",
        error.detail
    );
}

#[test]
fn the_systemd_command_line_says_restart_is_not_systemds_decision() {
    let unit = Unit {
        id: "openrobot/grasp".to_string(),
        kind: PackageKind::Policy,
        program: PathBuf::from("/data/packages/blobs/x/bin/run"),
        args: vec!["--fast".to_string()],
        working_directory: PathBuf::from("/data/packages/blobs/x"),
        limits: Limits {
            ram_mb: Some(512.0),
            cpu_cores: Some(1.0),
            tasks_max: 128,
        },
        cpus: Cpus {
            present: [0, 1].into_iter().collect(),
            isolated: [1].into_iter().collect(),
        },
    };
    let argv = SystemdLauncher::system().command_line(&unit).unwrap();
    assert!(argv.contains(&"--unit=skynet:openrobot:grasp".to_string()));
    assert!(argv.contains(&"Restart=no".to_string()));
    assert!(argv.contains(&"MemoryMax=512M".to_string()));
    assert!(argv.contains(&"AllowedCPUs=0".to_string()));
    assert_eq!(argv.last().unwrap(), "--fast");
}

#[test]
fn a_unit_name_round_trips_back_to_a_package_id() {
    assert_eq!(
        launch::unit_name("openrobot/grasp"),
        "skynet:openrobot:grasp"
    );
    assert_eq!(
        launch::package_id_of_unit("skynet:openrobot:grasp.service").as_deref(),
        Some("openrobot/grasp")
    );
    // Anything that is not ours is not ours.
    assert!(launch::package_id_of_unit("sshd.service").is_none());
    assert!(launch::package_id_of_unit("skynet:openrobot.service").is_none());
    assert!(launch::package_id_of_unit("skynet:a:b:c.service").is_none());
}

#[test]
fn systemctl_output_becomes_unit_statuses() {
    let text = "Id=skynet:openrobot:grasp.service\n\
                ActiveState=active\n\
                SubState=running\n\
                NRestarts=2\n\
                ExecMainStatus=0\n\
                \n\
                Id=skynet:otherlab:nav.service\n\
                ActiveState=failed\n\
                SubState=failed\n\
                NRestarts=5\n\
                ExecMainStatus=1\n\
                \n\
                Id=sshd.service\n\
                ActiveState=active\n\
                SubState=running\n";
    let units = launch::parse_systemctl_show(text);
    assert_eq!(units.len(), 2, "somebody else's unit is not ours");
    assert_eq!(units[0].id, "openrobot/grasp");
    assert_eq!(units[0].state, UnitState::Running);
    assert_eq!(units[0].restarts, 2);
    assert_eq!(units[1].id, "otherlab/nav");
    assert_eq!(units[1].state, UnitState::Failed);
    assert_eq!(units[1].last_exit, Some(1));
}

/// The distinction §33.3 leans on: an operator's stop and a crash look the same on a process table.
#[test]
fn systemd_states_map_so_that_failed_is_never_read_as_stopped() {
    assert_eq!(launch::unit_state("active", "running"), UnitState::Running);
    assert_eq!(launch::unit_state("inactive", "dead"), UnitState::Stopped);
    assert_eq!(launch::unit_state("failed", "failed"), UnitState::Failed);
    assert_eq!(
        launch::unit_state("activating", "start"),
        UnitState::Transitioning
    );
    // An unrecognised word costs a pass rather than starting a second copy of something running.
    assert_eq!(
        launch::unit_state("something-new", "elsewhere"),
        UnitState::Transitioning
    );
}

#[test]
fn a_cpu_list_parses_the_way_linux_writes_one() {
    assert_eq!(
        launch::parse_cpu_list("0-3,7,9-10"),
        [0, 1, 2, 3, 7, 9, 10].into_iter().collect()
    );
    assert_eq!(launch::parse_cpu_list(""), Default::default());
    // A malformed range contributes nothing rather than a guess about the machine.
    assert_eq!(launch::parse_cpu_list("5-2"), Default::default());
}

// ── RAUC ─────────────────────────────────────────────────────────────────────

#[test]
fn rauc_status_becomes_slots() {
    let text = r#"{"compatible":"robot","booted":"A","slots":[
        {"rootfs.0":{"bootname":"A","slot_status":{"bundle_version":"1.4.0"}}},
        {"rootfs.1":{"bootname":"B","slot_status":{"bundle_version":"1.5.0"}}}]}"#;
    let slots = crate::observe::parse_rauc_status(text);
    assert_eq!(slots.active, "1.4.0");
    assert_eq!(slots.staged.as_deref(), Some("1.5.0"));
}

/// A shape this parser does not recognise produces empty slots, and the reconciler then stages
/// rather than applies — the harmless direction. A guess would apply an image the robot is running.
#[test]
fn an_unrecognised_rauc_shape_produces_no_claim_about_the_image() {
    for text in ["", "not json", "{}", r#"{"slots":[]}"#] {
        let slots = crate::observe::parse_rauc_status(text);
        assert_eq!(slots.active, "");
        assert_eq!(slots.staged, None);
    }
}

// ── HTTP ─────────────────────────────────────────────────────────────────────

#[test]
fn an_ordinary_request_parses() {
    let head =
        "GET /packages?verbose=1 HTTP/1.1\r\nHost: robot\r\nAuthorization: Bearer abc\r\n\r\n";
    let request = http::parse(head, Vec::new()).unwrap();
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/packages");
    assert_eq!(request.segments(), vec!["packages"]);
    assert_eq!(request.header("authorization"), Some("Bearer abc"));
    assert_eq!(request.param("verbose").as_deref(), Some("1"));
    assert_eq!(request.param("absent"), None);
}

#[test]
fn a_percent_encoded_path_decodes_and_a_broken_one_does_not() {
    let head = "GET /packages/openrobot%3Agrasp HTTP/1.1\r\n\r\n";
    let request = http::parse(head, Vec::new()).unwrap();
    assert_eq!(request.segments(), vec!["packages", "openrobot:grasp"]);

    assert!(http::percent_decode("%zz").is_none());
    assert!(http::percent_decode("%f").is_none());
    // Invalid UTF-8 is refused rather than replaced, so a path cannot decode to a replacement
    // character and produce a 404 naming something nobody sent.
    assert!(http::percent_decode("%ff%fe").is_none());
}

/// A duplicated header is a request nobody meant to send, and "last wins" is how a second
/// `Authorization` overrides the first.
#[test]
fn a_duplicated_header_is_refused() {
    let head = "GET /status HTTP/1.1\r\nAuthorization: Bearer a\r\nAuthorization: Bearer b\r\n\r\n";
    assert_eq!(
        http::parse(head, Vec::new()),
        Err(http::ParseError::Malformed)
    );
}

#[test]
fn a_traversal_never_reaches_routing() {
    for target in ["/../etc/passwd", "/packages/../../etc", "/a/%2e%2e/b"] {
        let head = format!("GET {target} HTTP/1.1\r\n\r\n");
        assert_eq!(
            http::parse(&head, Vec::new()),
            Err(http::ParseError::Malformed),
            "{target}"
        );
    }
}

#[test]
fn a_body_with_no_declared_length_is_refused_rather_than_read() {
    let head = "POST /grants HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n";
    assert_eq!(
        http::declared_length(head),
        Err(http::ParseError::LengthRequired)
    );
    assert_eq!(http::ParseError::LengthRequired.status(), 411);

    let huge = format!(
        "POST /grants HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        http::MAX_BODY + 1
    );
    assert_eq!(
        http::declared_length(&huge),
        Err(http::ParseError::TooLarge)
    );
}

#[test]
fn a_malformed_request_line_is_refused() {
    for head in [
        "GET\r\n\r\n",
        "GET /status\r\n\r\n",
        "GET /status HTTP/2\r\n\r\n",
        "get /status HTTP/1.1\r\n\r\n",
        "GET status HTTP/1.1\r\n\r\n",
        "GET /status HTTP/1.1 extra\r\n\r\n",
    ] {
        assert!(http::parse(head, Vec::new()).is_err(), "{head:?}");
    }
}

#[test]
fn a_response_carries_the_headers_a_console_on_a_robot_needs() {
    let encoded = http::Response::json(200, "{}").encode();
    let text = String::from_utf8(encoded).unwrap();
    assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
    assert!(text.contains("Content-Length: 2\r\n"));
    assert!(text.contains("X-Content-Type-Options: nosniff\r\n"));
    assert!(text.contains("X-Frame-Options: DENY\r\n"));
    assert!(text.contains("Cache-Control: no-store\r\n"));
    assert!(text.ends_with("\r\n\r\n{}"));
}

// ── Authorisation ────────────────────────────────────────────────────────────

/// §39.1's table has exactly two entries that need nothing, and this is the assertion that keeps it
/// at two. A route that quietly joined them would be an unauthenticated way to change the robot.
#[test]
fn stop_and_status_are_the_only_routes_that_need_no_credential() {
    let routes: &[(&str, &[&str])] = &[
        ("GET", &["status"]),
        ("POST", &["stop"]),
        ("GET", &["health"]),
        ("GET", &["packages"]),
        ("GET", &["packages", "a:b"]),
        ("GET", &["plan"]),
        ("GET", &["grants"]),
        ("POST", &["packages", "a:b", "start"]),
        ("POST", &["packages", "a:b", "stop"]),
        ("POST", &["packages", "a:b", "rollback"]),
        ("POST", &["grants"]),
        ("DELETE", &["grants", "a:b", "manipulation.grasp"]),
        ("POST", &["packages"]),
        ("POST", &["profile"]),
        ("GET", &["something-nobody-wrote"]),
    ];

    let open: Vec<(&str, &[&str])> = routes
        .iter()
        .copied()
        .filter(|(method, segments)| requirement(method, segments).0 == Role::Anonymous)
        .collect();
    assert_eq!(open.len(), 2);
    assert!(open.contains(&("GET", &["status"][..])));
    assert!(open.contains(&("POST", &["stop"][..])));
}

#[test]
fn an_unrecognised_route_needs_the_most_rather_than_the_least() {
    assert_eq!(requirement("GET", &["nothing", "here"]).0, Role::Admin);
    assert_eq!(requirement("PATCH", &["packages"]).0, Role::Admin);
}

#[test]
fn roles_are_ordered_so_that_more_includes_less() {
    assert!(requirement("GET", &["health"]).satisfied_by(Role::Admin));
    assert!(requirement("GET", &["health"]).satisfied_by(Role::Observer));
    assert!(!requirement("GET", &["health"]).satisfied_by(Role::Anonymous));
    assert!(!requirement("POST", &["grants"]).satisfied_by(Role::Observer));
    assert!(requirement("POST", &["grants"]).satisfied_by(Role::Operator));
}

#[test]
fn a_credential_matches_only_its_own_token() {
    let credential = Credential::issue("bench laptop", Role::Operator, "0011", "s3cret-token");
    assert!(credential.matches("s3cret-token"));
    assert!(!credential.matches("s3cret-toker"));
    assert!(!credential.matches(""));
    // The salt is what stops two identical tokens producing one stored hash.
    let other = Credential::issue("other", Role::Operator, "2233", "s3cret-token");
    assert_ne!(credential.hash, other.hash);
}

#[test]
fn an_unprovisioned_robot_grants_nothing_to_anybody() {
    let credentials = Credentials::empty();
    assert!(!credentials.is_provisioned());
    assert_eq!(credentials.role_of("anything"), Role::Anonymous);
    assert_eq!(
        credentials.role_of_header(Some("Bearer anything")),
        Role::Anonymous
    );
}

#[test]
fn a_credentials_file_round_trips_and_an_unknown_role_is_dropped() {
    let mut credentials = Credentials::empty();
    credentials.add(Credential::issue("laptop", Role::Admin, "aa", "token-a"));
    credentials.add(Credential::issue(
        "console",
        Role::Observer,
        "bb",
        "token-b",
    ));

    let document = credentials.to_json();
    let read = Credentials::from_json(&document);
    assert_eq!(read.role_of("token-a"), Role::Admin);
    assert_eq!(read.role_of("token-b"), Role::Observer);

    let mut hostile = Json::obj();
    let mut entry = Json::obj();
    entry
        .set("label", Json::from("x"))
        .set("role", Json::from("superuser"))
        .set("salt", Json::from("cc"))
        .set("hash", Json::from("dd"));
    let mut list = Json::arr();
    list.push(entry);
    hostile.set("credentials", list);
    assert!(
        !Credentials::from_json(&hostile).is_provisioned(),
        "a role this edition does not implement grants nothing"
    );
}

// ── The daemon, end to end ───────────────────────────────────────────────────

struct Fixture {
    root: TempRoot,
    daemon: Daemon<RecordingLauncher, FixedSlots>,
    credentials: Credentials,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let root = TempRoot::new(label);
        let store = Store::new(root.layout());
        store.prepare().unwrap();
        let mut daemon = Daemon::new(store, RecordingLauncher::new(), FixedSlots::default());
        // A machine with two cores and one isolated for the real-time path.
        daemon.cpus = Cpus {
            present: [0, 1].into_iter().collect(),
            isolated: [1].into_iter().collect(),
        };
        Self {
            root,
            daemon,
            credentials: Credentials::empty(),
        }
    }

    fn provision(&mut self) {
        self.credentials
            .add(Credential::issue("suite", Role::Admin, "aa", "admin-token"));
        self.credentials.add(Credential::issue(
            "watcher",
            Role::Observer,
            "bb",
            "observer-token",
        ));
    }

    fn write_desired(&self, document: &str) {
        let path = self.daemon.store.layout().desired();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, document).unwrap();
    }

    fn request(
        &mut self,
        method: &str,
        target: &str,
        token: Option<&str>,
        body: &str,
    ) -> (u16, Json) {
        let mut head = format!("{method} {target} HTTP/1.1\r\nHost: robot\r\n");
        if let Some(token) = token {
            head.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        if !body.is_empty() {
            head.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
        head.push_str("\r\n");
        let request = http::parse(&head, body.as_bytes().to_vec()).unwrap();

        let credentials = self.credentials.clone();
        let sink = crate::connectivity::Sink::describe(&crate::connectivity::Unpaired);
        let obstacle = crate::connectivity::Sink::obstacle(&crate::connectivity::Unpaired);
        let mut context = Context {
            daemon: &mut self.daemon,
            credentials: &credentials,
            now_ms: 1_000,
            started_ms: 0,
            // The suite exercises the API without a recorder: the routes that need one
            // answer 503, and the rest do not care.
            blackbox: None,
            mono_ns: 0,
            // The sink a robot actually has today, and the reason it has nothing else. A fixture
            // that said "a test" here would render the console panel in a state no robot is in,
            // which is the failure the console fixture exists to catch rather than to commit.
            outbound_sink: &sink,
            outbound_obstacle: obstacle.as_deref(),
        };
        let response = api::handle(&mut context, &request);
        let parsed = Json::parse(std::str::from_utf8(&response.body).unwrap()).unwrap();
        (response.status, parsed)
    }

    /// The same path, without insisting the answer is JSON.
    ///
    /// The console's three files are not, and a helper that parses every response as JSON is a
    /// helper that cannot test the routes a browser actually asks for first.
    fn raw(&mut self, method: &str, target: &str, token: Option<&str>) -> crate::http::Response {
        let mut head = format!("{method} {target} HTTP/1.1\r\nHost: robot\r\n");
        if let Some(token) = token {
            head.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        head.push_str("\r\n");
        let request = http::parse(&head, Vec::new()).unwrap();

        let credentials = self.credentials.clone();
        let sink = crate::connectivity::Sink::describe(&crate::connectivity::Unpaired);
        let obstacle = crate::connectivity::Sink::obstacle(&crate::connectivity::Unpaired);
        let mut context = Context {
            daemon: &mut self.daemon,
            credentials: &credentials,
            now_ms: 1_000,
            started_ms: 0,
            blackbox: None,
            mono_ns: 0,
            // The sink a robot actually has today, and the reason it has nothing else. A fixture
            // that said "a test" here would render the console panel in a state no robot is in,
            // which is the failure the console fixture exists to catch rather than to commit.
            outbound_sink: &sink,
            outbound_obstacle: obstacle.as_deref(),
        };
        api::handle(&mut context, &request)
    }
}

// ── Rendering the console without a robot ────────────────────────────────────

/// Write the console and one answer per route it fetches, so a browser can render it.
///
/// `cargo test -p skynetd-io -- --ignored console_fixture --nocapture`, then serve the directory it
/// names — any static server will do, and §32.2's whole point is that the page needs nothing else.
///
/// **Why this exists.** The console has been written, served, routed and tested for several sessions
/// and has never been *looked at*. Its script is syntax-checked in CI and every route it calls has a
/// test, and neither of those catches a panel that renders as a blank box, a column that wraps, or a
/// `null` printed where a sentence belongs — the failures a person sees in a second and no assertion
/// is watching for.
///
/// The answers come from `api::handle` rather than from hand-written JSON, which is the property
/// that makes this worth having: a fixture somebody typed would drift from the routes, and the page
/// would be rendered against a robot that does not exist.
///
/// `#[ignore]` because it writes files and is a thing somebody *does*, not a thing CI checks.
#[test]
#[ignore]
fn console_fixture() {
    let mut fixture = Fixture::new("console-fixture");
    fixture.provision();

    // A robot with a body, a trusted vendor, a signed policy and an unsigned one — so every state
    // the Signed by column can show is on the page at once. The body declares §46's windows and
    // gets one of them wrong, so the watchdog panel has the case it exists for rather than an
    // honest blank.
    write_embodiment(&fixture, BENCH_ARM_WATCHDOG);

    // §27's machine, observed once — because the page renders what the daemon has seen, and a
    // daemon that has never looked reports `post`. A fixture left at the initial value would draw
    // the panel for a robot mid-boot rather than for the one the rest of this fixture describes,
    // which is the drift the fixture exists to prevent: the answers come from `api::handle` so
    // that the page is rendered against a robot that exists.
    fixture.daemon.refresh_operating_state(None);
    let vendor = driver_key(31);
    write_trust(&fixture, "bench-robotics", &public_hex(&vendor));

    let layout = fixture.daemon.store.layout().clone();

    // Every row the Trust panel can draw, on one page: a key with full provenance, one added by
    // hand before anything recorded it, and a line the robot cannot read at all. The panel's job is
    // to keep those three apart, and a fixture holding only the happy one would not show whether it
    // does.
    std::fs::write(
        layout.trust(),
        format!(
            "{{\"keys\":[\
               {{\"label\":\"bench-robotics\",\"key\":\"{vendor}\",\
                 \"added_at\":\"2026-08-30T09:14:02Z\",\"added_by\":\"alice\",\
                 \"note\":\"release signing, added at pairing\"}},\
               {{\"label\":\"legacy-integrator\",\"key\":\"{other}\"}},\
               {{\"label\":\"typo\",\"key\":\"not-a-key\"}}\
             ]}}",
            vendor = public_hex(&vendor),
            other = public_hex(&driver_key(77)),
        ),
    )
    .unwrap();

    // Calibration in four states at once (§45): one valid and signed, one expired on age, one that
    // fails its own residual, and one nobody has ever measured. A fixture holding only the healthy
    // case would not show whether the panel keeps them apart.
    let calibration = layout.calibration();
    std::fs::create_dir_all(&calibration).unwrap();
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as f64)
        .unwrap_or(0.0);
    for (set, extra, taken) in [
        ("joint_zeros", "\"policy\":{}", now_s - 900.0),
        (
            "camera_extrinsics",
            "\"policy\":{\"max_age_s\":600,\"invalidating_events\":[\"collision\"]}",
            now_s - 3_600.0,
        ),
        (
            "tool_offset",
            "\"residual\":12,\"residual_threshold\":2,\"policy\":{}",
            now_s - 60.0,
        ),
    ] {
        std::fs::write(
            calibration.join(format!("{set}.json")),
            format!(
                "{{\"set\":\"{set}\",\"method\":\"charuco-12x9, 40 poses\",\"residual\":0.4,\
                  \"residual_threshold\":1,\"operator\":\"alice\",\
                  \"conditions\":{{\"at_s\":{taken},\"temperature_c\":22}},{extra}}}"
            ),
        )
        .unwrap();
    }
    // And one of them signed, so the Signed by column has something other than `absent` in it.
    let zeros = std::fs::read_to_string(calibration.join("joint_zeros.json")).unwrap();
    let zeros_digest = crate::embodiment::digest_of(&Json::parse(&zeros).unwrap()).unwrap();
    let zeros_hex: String = zeros_digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    std::fs::write(
        calibration.join("joint_zeros.sig.json"),
        format!(
            "{{\"key\":\"{}\",\"value\":\"{}\"}}",
            public_hex(&vendor),
            hex_of(&vendor.sign(&crate::calibration::calibration_message(&zeros_hex)))
        ),
    )
    .unwrap();

    // A queue with something in every class and a drop already counted, so the panel's whole
    // vocabulary is on the page: an incident that is never discarded, a check-in that superseded
    // four of its own, episodes, and telemetry that has been thrown away.
    let queue = crate::outbound::Queue::at(layout.outbound(), 4_096, 1_024);
    queue
        .enqueue(
            crate::outbound::Class::Incident,
            "{\"kind\":\"estop\",\"detail\":\"the button on the chassis\"}",
        )
        .unwrap();
    for generation in 0..5 {
        queue
            .enqueue(
                crate::outbound::Class::CheckIn,
                &format!("{{\"generation\":{generation}}}"),
            )
            .unwrap();
    }
    queue
        .enqueue(crate::outbound::Class::Episode, &"e".repeat(600))
        .unwrap();
    // Past the shared pool on purpose, so the panel shows a non-zero Dropped column: a queue that
    // has never thrown anything away does not demonstrate the one number worth looking at.
    for index in 0..12 {
        queue
            .enqueue(
                crate::outbound::Class::Telemetry,
                &format!("{index:02}{}", "t".repeat(300)),
            )
            .unwrap();
    }

    // §34.3's bearers, in the state that makes the panel worth looking at: ethernet declared and
    // down, Wi-Fi up and *marked* metered, cellular up and metered by assumption. So the active
    // bearer is a metered one, the head of the queue is an incident — which §34.3 sends over
    // anything — and the two figures behind it are waiting. A fixture with a docked robot on
    // ethernet would show a panel in which every sentence is "yes".
    std::fs::write(
        layout.state().join("bearers.json"),
        "{\"bearers\":[{\"name\":\"eth0\",\"kind\":\"ethernet\",\"up\":false},\
           {\"name\":\"wlan0\",\"kind\":\"wifi\",\"metered\":true,\"up\":true},           {\"name\":\"wwan0\",\"kind\":\"cellular\",\"up\":true}]}",
    )
    .unwrap();

    let audit = crate::audit::Audit::of(&layout);
    for (at, actor, action, subject, previous) in [
        (
            "2026-08-30T09:14:02Z",
            "alice",
            "trust.add",
            "bench-robotics",
            None,
        ),
        (
            "2026-08-30T09:20:41Z",
            "alice",
            "trust.add",
            "openrobot",
            None,
        ),
        (
            "2026-08-30T11:02:07Z",
            "carol",
            "trust.remove",
            "openrobot",
            Some("added 2026-08-30T09:20:41Z by alice"),
        ),
    ] {
        audit
            .append(at, actor, action, subject, "", previous)
            .unwrap();
    }
    let body = crate::embodiment::load(&layout);
    let descriptor_digest = body.known().expect("a body").digest_hex.clone();
    std::fs::write(
        layout.embodiment_signature(),
        format!(
            "{{\"key\":\"{}\",\"value\":\"{}\"}}",
            public_hex(&vendor),
            hex_of(&vendor.sign(&crate::trust::embodiment_message(&descriptor_digest)))
        ),
    )
    .unwrap();

    let source = fixture.root.path().join("source");
    let m = manifest("openrobot/grasp", "1.0.0", "policy");
    let digest = write_package(&source, &m, &[("bin/run", "#!/bin/sh\nsleep 1\n")]);
    let id = PackageId::parse("openrobot/grasp").unwrap();
    fixture
        .daemon
        .store
        .stage(&id, "policy", "1.0.0", &digest, &source)
        .unwrap();

    let signature = vendor.sign(&crate::trust::package_message(&digest));
    fixture.write_desired(&format!(
        r#"{{
  "generation": 12,
  "os": {{ "channel": "stable", "version": "" }},
  "packages": [
    {{ "id": "openrobot/grasp", "version": "1.0.0", "digest": "{digest}",
      "kind": "policy", "state": "running", "grants": ["manipulation.grasp"],
      "signature": {{ "key": "{key}", "value": "{value}" }} }}
  ]
}}"#,
        digest = digest,
        key = public_hex(&vendor),
        value = hex_of(&signature),
    ));
    fixture.daemon.reload();
    let _ = fixture.daemon.reconcile(1_000);

    let out = std::env::var("SKYNET_CONSOLE_FIXTURE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("skynet-console"));
    std::fs::create_dir_all(&out).unwrap();

    // The page itself, straight out of the binary, so what is served here is what a robot serves.
    for (name, response) in [
        ("index.html", fixture.raw("GET", "/", None)),
        ("console.css", fixture.raw("GET", "/console.css", None)),
        ("console.js", fixture.raw("GET", "/console.js", None)),
    ] {
        std::fs::write(out.join(name), &response.body).unwrap();
    }

    // One file per feed, named exactly as the page fetches it, **with its status code beside it**.
    // A 503 replayed as a 200 is a different page: the console's job on a failed panel is to say
    // what is missing, and serving the error body with a success status would render the error
    // object as data and hide exactly the behaviour worth looking at.
    for route in [
        "status",
        "embodiment",
        "plan",
        "packages",
        "health",
        "blackbox",
        "capacity",
        "grants",
        "trust",
        "audit",
        "calibration",
        "outbound",
        "watchdog",
        "post",
        "state",
        "skills",
        "ota",
        "identity",
        "checkin",
        "intent",
    ] {
        let response = fixture.raw("GET", &format!("/{route}"), Some("observer-token"));
        std::fs::write(out.join(route), &response.body).unwrap();
        std::fs::write(
            out.join(format!("{route}.status")),
            response.status.to_string(),
        )
        .unwrap();
    }

    // The server that replays those status codes. Shipped with the fixture rather than left to
    // "any static server will do", because a static server does not know that the file beside
    // `blackbox` is a status code — it answers 200 with the error body, and the console dutifully
    // renders `undefined records of undefined B` for a robot that said *service unavailable*.
    // That is the §32.2 violation this console exists not to commit, arriving through the way it
    // was being looked at rather than through its own code, and it is what a static server drew the
    // first time somebody actually opened the page.
    std::fs::write(
        out.join("fixture-serve.mjs"),
        include_str!("../console/fixture-serve.mjs"),
    )
    .unwrap();

    println!("console fixture written to {}", out.display());
    println!(
        "node {}/fixture-serve.mjs 8080   # then open http://127.0.0.1:8080/",
        out.display()
    );
    println!("serve it with that, not a plain static server: it replays each route's status code");
}

// ── The power-on self-test (§48.2) ───────────────────────────────────────────

fn post_check<'a>(body: &'a Json, name: &str) -> &'a Json {
    body.arr_or_empty("checks")
        .iter()
        .find(|row| row.get("check").and_then(Json::as_str) == Some(name))
        .expect("every §48.2 check is reported")
}

/// The structural fact §33.1 forces: four of §48.2's eight checks need the actuator path, and this
/// daemon is forbidden from touching it. They are reported as somebody else's rather than as
/// unavailable, because "nobody has run this" and "this is not mine to run" are different sentences.
#[test]
fn the_checks_skynetd_cannot_run_are_named_as_the_kernels() {
    let mut fixture = Fixture::new("post-elsewhere");
    fixture.provision();

    let (status, body) = fixture.request("GET", "/post", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(
        body.arr_or_empty("checks").len(),
        8,
        "§48.2's table is eight rows"
    );

    for name in [
        "rhal_abi",
        "describe_matches_descriptor",
        "estop_asserted_and_observed",
        "encoder_sanity",
        "kernel_corpus_selfcheck",
    ] {
        let check = post_check(&body, name);
        assert_eq!(
            check.get("outcome").and_then(Json::as_str),
            Some("elsewhere"),
            "{name}"
        );
        // The row still carries what a failure would mean, which is what makes it worth showing.
        assert!(!check
            .get("failure_means")
            .and_then(Json::as_str)
            .unwrap_or("")
            .is_empty());
    }

    // And nothing may run on the strength of a self-test that ran three of eight checks.
    assert_eq!(body.get("may_run").and_then(Json::as_bool), Some(false));
    assert_eq!(body.get("failed").and_then(Json::as_bool), Some(false));
    assert_eq!(
        body.get("verdict").and_then(Json::as_str),
        Some("post_incomplete")
    );
}

/// §48.2 sends a *failed* POST to `SAFE`, which is sticky and needs a human. A robot whose checks
/// could not run does not need a human — it needs the rest of the operating system — so the two
/// verdicts are separate.
#[test]
fn an_unreachable_watchdog_layer_fails_post_and_the_verdict_is_safe() {
    let mut fixture = Fixture::new("post-inverted");
    fixture.provision();
    write_embodiment(
        &fixture,
        &NESTED_ARM.replace("\"soc_wdt_ms\": 250", "\"soc_wdt_ms\": 40"),
    );

    let (_, body) = fixture.request("GET", "/post", Some("observer-token"), "");
    let nesting = post_check(&body, "watchdog_nesting");
    assert_eq!(nesting.get("outcome").and_then(Json::as_str), Some("fail"));
    assert_eq!(body.get("failed").and_then(Json::as_bool), Some(true));
    assert_eq!(body.get("verdict").and_then(Json::as_str), Some("safe"));
}

/// An undeclared window is not a fault. Nothing contradicts, and a body nobody has measured is not
/// a body that is wrong — so it is `unavailable`, and POST is incomplete rather than failed.
#[test]
fn an_undeclared_window_leaves_post_incomplete_rather_than_failed() {
    let mut fixture = Fixture::new("post-undeclared");
    fixture.provision();
    write_embodiment(&fixture, BENCH_ARM);

    let (_, body) = fixture.request("GET", "/post", Some("observer-token"), "");
    let nesting = post_check(&body, "watchdog_nesting");
    assert_eq!(
        nesting.get("outcome").and_then(Json::as_str),
        Some("unavailable")
    );
    assert_eq!(body.get("failed").and_then(Json::as_bool), Some(false));
    assert_eq!(
        body.get("verdict").and_then(Json::as_str),
        Some("post_incomplete")
    );
}

/// §45.5 degrades rather than refuses, so a stale calibration must not stop a robot booting — a
/// self-test that refused would be a robot nobody could get into a state where they could
/// recalibrate it. What POST does is make the list known at power-on.
#[test]
fn an_expired_calibration_is_listed_by_post_rather_than_failing_it() {
    let mut fixture = Fixture::new("post-calibration");
    fixture.provision();
    write_embodiment(&fixture, BENCH_ARM);

    let (_, body) = fixture.request("GET", "/post", Some("observer-token"), "");
    let calibration = post_check(&body, "calibration_expiry");
    assert_eq!(
        calibration.get("outcome").and_then(Json::as_str),
        Some("pass")
    );
    let detail = calibration
        .get("detail")
        .and_then(Json::as_str)
        .unwrap_or("");
    assert!(detail.contains("guard(s) will be unavailable"), "{detail}");
    assert!(detail.contains("workspace"), "{detail}");
    assert_eq!(body.get("failed").and_then(Json::as_bool), Some(false));
}

/// The recorder: a ring nobody has opened has established nothing, which is not the same as a ring
/// that cannot be written.
#[test]
fn a_robot_with_no_ring_reports_the_recorder_unavailable_rather_than_broken() {
    let mut fixture = Fixture::new("post-no-ring");
    fixture.provision();

    let (_, body) = fixture.request("GET", "/post", Some("observer-token"), "");
    let recorder = post_check(&body, "blackbox_writable");
    assert_eq!(
        recorder.get("outcome").and_then(Json::as_str),
        Some("unavailable")
    );
    assert_eq!(
        recorder.get("failure_means").and_then(Json::as_str),
        Some("the recorder cannot record")
    );
}

// ── The watchdog chain (§46) ─────────────────────────────────────────────────

/// A body whose declared windows nest, so the interesting cases are the ones that do not.
const NESTED_ARM: &str = r#"{
  "robot_slug": "bench-arm",
  "base": "fixed",
  "dof": 1,
  "on_command_loss": "hold",
  "envelope": { "watchdog_ms": 100 },
  "watchdog": { "soc_wdt_ms": 250, "drive_command_timeout_ms": 500 },
  "joints": [{"name": "shoulder", "type": "revolute", "control_modes": ["position"]}]
}"#;

#[test]
fn the_watchdog_route_reports_the_chain_and_the_nesting() {
    let mut fixture = Fixture::new("watchdog-nested");
    fixture.provision();
    write_embodiment(&fixture, NESTED_ARM);

    let (status, body) = fixture.request("GET", "/watchdog", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.arr_or_empty("chain").len(), 4);
    assert_eq!(
        body.get("on_command_loss").and_then(Json::as_str),
        Some("hold")
    );
    assert_eq!(body.get("host_blocked"), Some(&Json::Null));

    let nesting = body.get("nesting").cloned().expect("a nesting report");
    assert!(nesting.arr_or_empty("faults").is_empty());
    // The kernel's own worst case is the kernel's to measure and §27 is unwritten, so it is
    // reported missing rather than skipped: a report that ignored the window it could not get would
    // be reporting on a chain it had not established.
    assert_eq!(
        nesting.str_arr("missing"),
        vec!["kernel_cycle_worst_case_ms"]
    );
    assert_eq!(nesting.get("ordered").and_then(Json::as_bool), Some(true));
    assert_eq!(nesting.get("complete").and_then(Json::as_bool), Some(false));
}

/// §46.3's inversion, over the wire: the harsher recovery pre-empts the gentler one.
#[test]
fn a_wdt_inside_the_kernel_window_is_reported_with_what_it_costs() {
    let mut fixture = Fixture::new("watchdog-inverted");
    fixture.provision();
    write_embodiment(
        &fixture,
        &NESTED_ARM.replace("\"soc_wdt_ms\": 250", "\"soc_wdt_ms\": 40"),
    );

    let (_, body) = fixture.request("GET", "/watchdog", Some("observer-token"), "");
    let faults = body
        .get("nesting")
        .map(|n| n.arr_or_empty("faults").to_vec())
        .unwrap_or_default();
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert_eq!(
        faults[0].get("code").and_then(Json::as_str),
        Some("wdt_pre_empts_kernel")
    );
    let detail = faults[0].get("detail").and_then(Json::as_str).unwrap_or("");
    assert!(detail.contains("controlled stop"), "{detail}");
}

/// §46.2. The refusal is host-shaped: a coasting body may be guest and may be certified, and only
/// the middle rung — where layer 3 is the last line — turns a reset into a fall.
#[test]
fn a_coasting_body_is_told_why_host_is_refused() {
    let mut fixture = Fixture::new("watchdog-coast");
    fixture.provision();
    write_embodiment(
        &fixture,
        &NESTED_ARM.replace(
            "\"on_command_loss\": \"hold\"",
            "\"on_command_loss\": \"coast\"",
        ),
    );

    let (_, body) = fixture.request("GET", "/watchdog", Some("observer-token"), "");
    let blocked = body
        .get("host_blocked")
        .and_then(Json::as_str)
        .unwrap_or("");
    assert!(blocked.contains("is a fall"), "{blocked}");
    assert!(blocked.contains("certified"), "{blocked}");
}

/// A body that declares nothing is not a body that holds.
#[test]
fn an_undeclared_command_loss_behaviour_blocks_host_too() {
    let mut fixture = Fixture::new("watchdog-silent");
    fixture.provision();
    write_embodiment(&fixture, BENCH_ARM);

    let (_, body) = fixture.request("GET", "/watchdog", Some("observer-token"), "");
    assert_eq!(body.get("on_command_loss"), Some(&Json::Null));
    let blocked = body
        .get("host_blocked")
        .and_then(Json::as_str)
        .unwrap_or("");
    assert!(blocked.contains("Silence is not"), "{blocked}");

    // And with no descriptor windows at all, three of four are missing and nothing is established.
    let nesting = body.get("nesting").cloned().expect("a nesting report");
    assert_eq!(nesting.str_arr("missing").len(), 4);
    assert_eq!(nesting.get("complete").and_then(Json::as_bool), Some(false));
}

/// Four rows of vocabulary over a machine with one of them running would read as four running
/// watchdogs, so the report says which layers exist *here*.
#[test]
fn the_route_separates_the_chain_from_what_this_robot_has() {
    let mut fixture = Fixture::new("watchdog-here");
    fixture.provision();
    write_embodiment(&fixture, NESTED_ARM);

    let (_, body) = fixture.request("GET", "/watchdog", Some("observer-token"), "");
    let here = body.arr_or_empty("here");
    assert_eq!(here.len(), 4);
    let armed: Vec<&str> = here
        .iter()
        .filter(|row| row.get("state").and_then(Json::as_str) == Some("armed"))
        .filter_map(|row| row.get("name").and_then(Json::as_str))
        .collect();
    assert_eq!(
        armed,
        vec!["supervisor"],
        "only the supervisor's watchdog is written"
    );
}

// ── Calibration on the robot (§45) ───────────────────────────────────────────

/// Write a record for `set`, with `body` merged over the usual fields.
fn write_calibration(fixture: &Fixture, set: &str, body: &str) {
    let directory = fixture.daemon.store.layout().calibration();
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join(format!("{set}.json")), body).unwrap();
}

const HOUR: f64 = 3_600.0;
const TAKEN_AT: f64 = 1_700_000_000.0;

fn record_json(set: &str, policy: &str) -> String {
    format!(
        "{{\"set\":\"{set}\",\"method\":\"charuco-12x9, 40 poses\",\"residual\":0.4,\
          \"residual_threshold\":1,\"operator\":\"alice\",\
          \"conditions\":{{\"at_s\":{TAKEN_AT},\"temperature_c\":22,\"joint_travel_rad\":1000}},\
          \"policy\":{policy}}}"
    )
}

/// A robot that has never been calibrated reports every applicable set absent, and every guard
/// those sets feed as unavailable — because absence is not validity.
#[test]
fn a_robot_with_no_records_starves_the_guards_that_need_them() {
    let mut fixture = Fixture::new("calib-none");
    fixture.provision();
    write_embodiment(&fixture, BENCH_ARM);

    let (status, body) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    assert_eq!(status, 200);

    for set in body.arr_or_empty("sets") {
        assert_eq!(set.get("state").and_then(Json::as_str), Some("absent"));
    }
    let starved = body.arr_or_empty("unavailable");
    assert!(!starved.is_empty());
    for entry in starved {
        assert_eq!(entry.get("state").and_then(Json::as_str), Some("absent"));
        // §45.5's requirement: the guard names the parameter set it is waiting on.
        let detail = entry.get("detail").and_then(Json::as_str).unwrap_or("");
        let set = entry.get("set").and_then(Json::as_str).unwrap_or("");
        assert!(detail.contains(set), "{detail}");
    }
}

/// §45.5, end to end on a real file: an expired extrinsic disables `proximity` and `workspace`, by
/// name, and the sentence carries both numbers.
#[test]
fn an_expired_record_disables_its_guards_and_names_the_set() {
    let mut fixture = Fixture::new("calib-expired");
    fixture.provision();
    write_embodiment(&fixture, BENCH_ARM);
    write_calibration(
        &fixture,
        "camera_extrinsics",
        &record_json("camera_extrinsics", "{\"max_age_s\":600}"),
    );

    let (_, body) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    let extrinsics = body
        .arr_or_empty("sets")
        .iter()
        .find(|row| row.get("set").and_then(Json::as_str) == Some("camera_extrinsics"))
        .cloned()
        .expect("the set is applicable on a body with a camera");
    assert_eq!(
        extrinsics.get("state").and_then(Json::as_str),
        Some("expired")
    );
    assert_eq!(extrinsics.get("reason").and_then(Json::as_str), Some("age"));

    let starved: Vec<String> = body
        .arr_or_empty("unavailable")
        .iter()
        .filter(|row| row.get("set").and_then(Json::as_str) == Some("camera_extrinsics"))
        .filter_map(|row| row.get("guard").and_then(Json::as_str).map(str::to_string))
        .collect();
    assert_eq!(starved, vec!["proximity", "workspace"]);
}

/// A record filed under one name that declares another is not a small inconsistency: whichever is
/// wrong, some set is being judged by another set's policy and the wrong guards lose their input.
#[test]
fn a_record_filed_under_the_wrong_set_is_refused_rather_than_believed() {
    let mut fixture = Fixture::new("calib-misfiled");
    fixture.provision();
    write_embodiment(&fixture, BENCH_ARM);
    write_calibration(
        &fixture,
        "camera_extrinsics",
        &record_json("joint_zeros", "{}"),
    );

    let (_, body) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    let row = body
        .arr_or_empty("sets")
        .iter()
        .find(|row| row.get("set").and_then(Json::as_str) == Some("camera_extrinsics"))
        .cloned()
        .expect("the set is applicable");
    assert_eq!(row.get("state").and_then(Json::as_str), Some("unusable"));
    let detail = row.get("detail").and_then(Json::as_str).unwrap_or("");
    assert!(detail.contains("joint_zeros"), "{detail}");
}

/// §45.3's provenance rule against a file: a record with no method is a rumour with six decimal
/// places, and it does not keep a guard alive.
#[test]
fn a_record_with_no_method_does_not_keep_its_guards_running() {
    let mut fixture = Fixture::new("calib-no-method");
    fixture.provision();
    write_embodiment(&fixture, BENCH_ARM);
    write_calibration(
        &fixture,
        "joint_zeros",
        "{\"set\":\"joint_zeros\",\"residual\":0.1,\"residual_threshold\":1}",
    );

    let (_, body) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    let row = body
        .arr_or_empty("sets")
        .iter()
        .find(|row| row.get("set").and_then(Json::as_str) == Some("joint_zeros"))
        .cloned()
        .expect("joint_zeros is applicable on every body");
    assert_eq!(row.get("state").and_then(Json::as_str), Some("unusable"));

    let starved: Vec<String> = body
        .arr_or_empty("unavailable")
        .iter()
        .filter(|row| row.get("set").and_then(Json::as_str) == Some("joint_zeros"))
        .filter_map(|row| row.get("guard").and_then(Json::as_str).map(str::to_string))
        .collect();
    assert_eq!(starved, vec!["workspace", "self_collision"]);
}

/// A valid record's signature is checked against the trust store, and a valid one starves nothing.
#[test]
fn a_signed_record_names_its_signer_and_keeps_its_guards() {
    let mut fixture = Fixture::new("calib-signed");
    fixture.provision();
    write_embodiment(&fixture, BENCH_ARM);
    let vendor = driver_key(41);
    write_trust(&fixture, "bench-robotics", &public_hex(&vendor));

    let document = record_json("joint_zeros", "{}");
    write_calibration(&fixture, "joint_zeros", &document);

    let parsed = Json::parse(&document).unwrap();
    let digest = crate::embodiment::digest_of(&parsed).unwrap();
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let signature = vendor.sign(&crate::calibration::calibration_message(&hex));
    std::fs::write(
        fixture
            .daemon
            .store
            .layout()
            .calibration()
            .join("joint_zeros.sig.json"),
        format!(
            "{{\"key\":\"{}\",\"value\":\"{}\"}}",
            public_hex(&vendor),
            hex_of(&signature)
        ),
    )
    .unwrap();

    let (_, body) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    let row = body
        .arr_or_empty("sets")
        .iter()
        .find(|row| row.get("set").and_then(Json::as_str) == Some("joint_zeros"))
        .cloned()
        .expect("joint_zeros is applicable");
    assert_eq!(row.get("state").and_then(Json::as_str), Some("valid"));
    assert_eq!(
        row.get("signature").and_then(Json::as_str),
        Some("verified")
    );
    assert_eq!(
        row.get("signer").and_then(Json::as_str),
        Some("bench-robotics")
    );

    assert!(body
        .arr_or_empty("unavailable")
        .iter()
        .all(|entry| entry.get("set").and_then(Json::as_str) != Some("joint_zeros")));
}

/// §45.3 signs a *measurement* and §38.1 signs a *limit*. Without separate domain tags a signature
/// over one document's digest verifies over the other's, and those are separate decisions by
/// separate people.
#[test]
fn a_calibration_signature_does_not_verify_as_a_profile_signature() {
    let signer = skynet_ed25519::signing::SigningKey::from_seed(&[9u8; 32]);
    let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    let store = crate::trust::TrustStore::from_text(&format!(
        "{{\"keys\":[{{\"label\":\"a\",\"key\":\"{}\",\"added_at\":\"2026-08-30T00:00:00Z\"}}]}}",
        public_hex(&signer)
    ));

    let over_calibration = signer.sign(&crate::calibration::calibration_message(digest));
    let signed = crate::trust::SignedBy {
        key: signer.verifying_key(),
        signature: skynet_ed25519::Signature::from_bytes(&over_calibration).unwrap(),
    };

    assert_eq!(
        store
            .check(
                &crate::calibration::calibration_message(digest),
                Some(&signed)
            )
            .state,
        skynet_contracts::admission::SignatureState::Verified
    );
    assert_eq!(
        store
            .check(&crate::trust::profile_message(digest), Some(&signed))
            .state,
        skynet_contracts::admission::SignatureState::Failed
    );
}

/// §32.2's rule, applied to a subsystem that does not exist yet: three of §45.4's four conditions
/// have nothing producing them on this robot, and the report says so rather than letting a page of
/// expired records read as a machine that has drifted.
#[test]
fn the_conditions_this_robot_cannot_measure_are_named() {
    let mut fixture = Fixture::new("calib-unmeasured");
    fixture.provision();

    let (_, body) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    let unmeasured = body.str_arr("unmeasured").join(" ");
    assert!(unmeasured.contains("temperature_c"), "{unmeasured}");
    assert!(unmeasured.contains("joint_travel_rad"), "{unmeasured}");
    assert!(unmeasured.contains("events"), "{unmeasured}");

    let now = body.get("now").cloned().unwrap_or_else(Json::obj);
    assert_eq!(now.get("temperature_c"), Some(&Json::Null));
    assert_eq!(now.get("joint_travel_rad"), Some(&Json::Null));
}

/// The rule that keeps an expiry policy from switching itself off, on this robot specifically:
/// nothing reads a temperature here, so a bias with a temperature policy expires rather than
/// quietly staying valid for ever.
#[test]
fn a_temperature_policy_expires_on_a_robot_that_cannot_read_one() {
    let mut fixture = Fixture::new("calib-no-thermal");
    fixture.provision();
    write_embodiment(&fixture, BENCH_ARM);
    write_calibration(
        &fixture,
        "force_torque_bias",
        &record_json("force_torque_bias", "{\"max_temperature_delta_c\":5}"),
    );

    let (_, body) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    let row = body
        .arr_or_empty("sets")
        .iter()
        .find(|row| row.get("set").and_then(Json::as_str) == Some("force_torque_bias"))
        .cloned()
        .expect("a manipulator holds an F/T bias");
    assert_eq!(row.get("state").and_then(Json::as_str), Some("expired"));
    assert_eq!(
        row.get("reason").and_then(Json::as_str),
        Some("temperature")
    );
    let _ = HOUR;
}

// ── A contradiction between two shared documents, pinned ────────────────────

/// **`manifest.digest` cannot be this package's own version digest, and two shared documents
/// disagree about whether it is.**
///
/// `reference/typescript/package.ts` says of the field: *"Version digest from the Hub. The identity
/// that matters."* §37.1's store layout says the version digest is *"computed by `versionDigest()`
/// over the manifest as a value and the remaining files as a `(path, sha256)` list."*
///
/// Those cannot both hold. The digest is a hash **of** the manifest, so a manifest carrying it is a
/// fixed point of SHA-256: writing the digest into the field changes the digest. This test does
/// exactly that and shows the two values, which is the demonstration rather than the argument.
///
/// The consequence is not cosmetic. On the robot's own staging path, one of two things happens:
///
/// * the field is **absent** — the common case, since nobody can compute a satisfying value — and
///   `manifest_from_json` refuses, so **admission does not run at all**. §37.1's `verify → admit →
///   stage` loses its middle step silently, and everything the twelfth and thirteenth sessions
///   built on top of the report never executes;
/// * the field is **present** with any value, and `digest_mismatch` fires, on every package, for
///   ever.
///
/// Nothing caught it because `corpus/admission/*` uses `"sha256:pkg"` for both `manifest.digest`
/// and `observed_digest` — a placeholder that makes them trivially equal, and one that also
/// violates the same section's rule that a digest is *"64 lowercase hex characters and nothing
/// else — no `sha256:` prefix"*. A corpus case is only as sharp as the value it pins.
///
/// This test asserts **what happens today**, not what should. The fix is a contract decision with
/// two defensible answers — exclude `digest` from the hashed manifest, or remove it from the
/// manifest and let the desired state carry the identity, which is where §37.1 already puts it —
/// and it changes the identity of every package in three implementations and the corpus. Recorded
/// rather than guessed.
#[test]
fn a_manifest_cannot_carry_its_own_version_digest() {
    let root = TempRoot::new("digest-fixed-point");
    let store = Store::new(root.layout());
    store.prepare().unwrap();
    let source = root.path().join("source");

    let files = [("bin/run", "#!/bin/sh\nsleep 1\n")];
    let without = write_package(
        &source,
        &manifest("openrobot/grasp", "1.0.0", "policy"),
        &files,
    );

    // Now write that answer into the manifest, which is what the field is documented to hold.
    let mut claiming = manifest("openrobot/grasp", "1.0.0", "policy");
    claiming.set("digest", Json::from(without.as_str()));
    let with = write_package(&source, &claiming, &files);

    assert_ne!(
        without, with,
        "if these were ever equal the field would be satisfiable and this test should be deleted"
    );

    // And the decode of a manifest without the field, which is the other horn.
    let bare = manifest("openrobot/grasp", "1.0.0", "policy");
    assert!(
        skynet_contracts::json::manifest_from_json(&bare).is_err(),
        "a manifest with no `digest` still decodes, so admission runs after all"
    );
}

// ── §38.3's audited surface, over the API ────────────────────────────────────

/// A robot nobody has provisioned answers the trust route rather than failing on it, and the
/// answer names the file to write. A 404 here would send somebody looking for a bug in the daemon.
#[test]
fn the_trust_route_reports_a_robot_with_no_store() {
    let mut fixture = Fixture::new("api-trust-missing");
    fixture.provision();

    let (status, body) = fixture.request("GET", "/trust", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.get("source").and_then(Json::as_str), Some("missing"));
    assert!(body.arr_or_empty("keys").is_empty());
    let notices = body.str_arr("notices").join(" ");
    assert!(notices.contains("no trust store"), "{notices}");
}

/// The three rows the console has to keep apart, over the wire: a key with provenance, one
/// without, and a line the robot could not read.
#[test]
fn the_trust_route_separates_recorded_unrecorded_and_unusable() {
    let mut fixture = Fixture::new("api-trust-rows");
    fixture.provision();

    std::fs::write(
        fixture.daemon.store.layout().trust(),
        format!(
            "{{\"keys\":[               {{\"label\":\"acme\",\"key\":\"{a}\",\"added_at\":\"2026-08-30T09:00:00Z\",                 \"added_by\":\"alice\"}},               {{\"label\":\"legacy\",\"key\":\"{b}\"}},               {{\"label\":\"typo\",\"key\":\"not-hex\"}}             ]}}",
            a = public_hex(&driver_key(11)),
            b = public_hex(&driver_key(12)),
        ),
    )
    .unwrap();

    let (status, body) = fixture.request("GET", "/trust", Some("observer-token"), "");
    assert_eq!(status, 200);
    let keys = body.arr_or_empty("keys");
    assert_eq!(keys.len(), 3);

    assert_eq!(
        keys[0].get("provenance_recorded").and_then(Json::as_bool),
        Some(true)
    );
    assert_eq!(
        keys[0].get("provenance").and_then(Json::as_str),
        Some("added 2026-08-30T09:00:00Z by alice")
    );
    assert_eq!(
        keys[1].get("provenance_recorded").and_then(Json::as_bool),
        Some(false)
    );
    assert_eq!(keys[2].get("usable").and_then(Json::as_bool), Some(false));
    assert_eq!(keys[2].get("at").and_then(Json::as_str), Some("keys[2]"));
}

/// §39.1 gives observer *"read health, guards, plan, packages, black box index"*. Reading which
/// public keys a robot holds belongs in that register: the person staring at an `untrusted`
/// package needs it, and requiring an admin credential would put the one diagnosis this store
/// supports behind a role most people watching a robot do not have.
#[test]
fn trust_and_audit_are_readable_by_an_observer_and_not_by_nobody() {
    let mut fixture = Fixture::new("api-trust-role");
    fixture.provision();

    for route in ["/trust", "/audit"] {
        assert_eq!(fixture.request("GET", route, None, "").0, 401, "{route}");
        assert_eq!(
            fixture.request("GET", route, Some("observer-token"), "").0,
            200,
            "{route}"
        );
    }
}

#[test]
fn the_audit_route_answers_newest_first_and_counts_what_it_did_not_send() {
    let mut fixture = Fixture::new("api-audit");
    fixture.provision();

    let audit = crate::audit::Audit::of(fixture.daemon.store.layout());
    for index in 0..3 {
        audit
            .append(
                "2026-08-30T09:00:00Z",
                "alice",
                "trust.add",
                &format!("key-{index}"),
                "",
                None,
            )
            .unwrap();
    }

    let (status, body) = fixture.request("GET", "/audit", Some("observer-token"), "");
    assert_eq!(status, 200);
    let entries = body.arr_or_empty("entries");
    assert_eq!(entries.len(), 3);
    assert_eq!(
        entries[0].get("subject").and_then(Json::as_str),
        Some("key-2"),
        "newest first"
    );
    assert_eq!(body.get("total").and_then(Json::as_usize), Some(3));
}

/// A log with invisible gaps is worse than none, because somebody is relying on it.
#[test]
fn a_broken_audit_line_is_reported_rather_than_dropped() {
    let mut fixture = Fixture::new("api-audit-broken");
    fixture.provision();

    let audit = crate::audit::Audit::of(fixture.daemon.store.layout());
    audit
        .append(
            "2026-08-30T09:00:00Z",
            "alice",
            "trust.add",
            "acme",
            "",
            None,
        )
        .unwrap();
    let mut text = std::fs::read_to_string(audit.path()).unwrap();
    text.push_str("{ truncated\n");
    std::fs::write(audit.path(), text).unwrap();

    let (status, body) = fixture.request("GET", "/audit", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.arr_or_empty("entries").len(), 1);
    assert_eq!(body.arr_or_empty("broken").len(), 1);
}

// ── §19's gate on a driver's signature ───────────────────────────────────────

/// A desired state naming one driver, with `SIGNATURE` replaced by a `"signature"` member or by
/// nothing at all.
const DESIRED_ONE_DRIVER: &str = r#"{
  "generation": 3,
  "os": { "channel": "stable", "version": "" },
  "packages": [
    { "id": "acme/arm", "version": "1.0.0", "digest": "DIGEST",
      "kind": "driver", "state": "running", "grants": []SIGNATURE }
  ]
}"#;

/// Write a desired state naming `acme/arm` as a driver, optionally signed.
fn desired_driver(fixture: &Fixture, digest: &str, signature: Option<(&str, &str)>) {
    let signature = match signature {
        None => String::new(),
        Some((key, value)) => {
            format!(", \"signature\": {{ \"key\": \"{key}\", \"value\": \"{value}\" }}")
        }
    };
    fixture.write_desired(
        &DESIRED_ONE_DRIVER
            .replace("DIGEST", digest)
            .replace("SIGNATURE", &signature),
    );
}

fn write_trust(fixture: &Fixture, label: &str, key_hex: &str) {
    let path = fixture.daemon.store.layout().trust();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        path,
        format!("{{\"keys\":[{{\"label\":\"{label}\",\"key\":\"{key_hex}\"}}]}}"),
    )
    .unwrap();
}

fn driver_key(seed: u8) -> skynet_ed25519::signing::SigningKey {
    skynet_ed25519::signing::SigningKey::from_seed(&[seed; 32])
}

fn public_hex(key: &skynet_ed25519::signing::SigningKey) -> String {
    String::from_utf8(key.public_hex().to_vec()).expect("hex is ASCII")
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// §19. A driver runs native, so an unsigned one does not activate.
#[test]
fn an_unsigned_driver_is_refused_at_activation() {
    let mut fixture = Fixture::new("driver-unsigned");
    let digest = format!("sha256:{}", "a".repeat(64));
    desired_driver(&fixture, &digest, None);
    fixture.daemon.reload();

    let refusal = fixture
        .daemon
        .driver_signature_refusal("acme/arm", &digest)
        .expect("an unsigned driver is refused");
    let Outcome::Failed { reason } = refusal else {
        panic!("a refusal, not {refusal:?}");
    };
    assert!(reason.contains("§19"), "{reason}");
    assert!(reason.contains("absent"), "{reason}");
    assert!(reason.contains("staged and inert"), "{reason}");
}

/// The case the fifth state exists for, at the place it matters most: native code, validly signed,
/// by somebody this robot has never been told to believe.
#[test]
fn a_driver_signed_by_a_stranger_is_refused_at_activation() {
    let mut fixture = Fixture::new("driver-stranger");
    let digest = format!("sha256:{}", "b".repeat(64));

    let trusted = driver_key(1);
    let stranger = driver_key(2);
    write_trust(&fixture, "acme-robotics", &public_hex(&trusted));

    let signature = stranger.sign(&crate::trust::package_message(&digest));
    desired_driver(
        &fixture,
        &digest,
        Some((&public_hex(&stranger), &hex_of(&signature))),
    );
    fixture.daemon.reload();

    let refusal = fixture
        .daemon
        .driver_signature_refusal("acme/arm", &digest)
        .expect("a stranger's driver is refused");
    let Outcome::Failed { reason } = refusal else {
        panic!("a refusal, not {refusal:?}");
    };
    assert!(reason.contains("untrusted"), "{reason}");
    assert!(reason.contains("state/trust.json"), "{reason}");
}

/// And the same driver, once the operator trusts the key that signed it.
#[test]
fn a_driver_signed_by_a_trusted_key_is_not_refused() {
    let mut fixture = Fixture::new("driver-trusted");
    let digest = format!("sha256:{}", "c".repeat(64));

    let signer = driver_key(7);
    let public = public_hex(&signer);
    write_trust(&fixture, "acme-robotics", &public);

    let signature = signer.sign(&crate::trust::package_message(&digest));
    desired_driver(&fixture, &digest, Some((&public, &hex_of(&signature))));
    fixture.daemon.reload();

    assert!(fixture
        .daemon
        .driver_signature_refusal("acme/arm", &digest)
        .is_none());
}

/// A signature that arrived and could not be read is not the same fact as no signature, and it must
/// not be reported as one — that would send an operator to ask a publisher for what they sent.
#[test]
fn a_malformed_driver_signature_is_not_reported_as_absent() {
    let mut fixture = Fixture::new("driver-malformed");
    let digest = format!("sha256:{}", "d".repeat(64));
    desired_driver(&fixture, &digest, Some(("not-hex", "also-not-hex")));
    fixture.daemon.reload();

    let refusal = fixture
        .daemon
        .driver_signature_refusal("acme/arm", &digest)
        .expect("a driver with an unreadable signature is refused");
    let Outcome::Failed { reason } = refusal else {
        panic!("a refusal, not {refusal:?}");
    };
    assert!(reason.contains("could not be read"), "{reason}");
    assert!(!reason.contains("absent"), "{reason}");
}

/// The gate is §19's, and §19 is about native code. A policy is sandboxed, and refusing it would
/// refuse every local install — which is how a gate becomes a flag.
#[test]
fn a_policy_is_not_gated_on_its_signature() {
    let mut fixture = Fixture::new("policy-ungated");
    let digest = format!("sha256:{}", "e".repeat(64));
    fixture.write_desired(&DESIRED_ONE_POLICY.replace("DIGEST", &digest));
    fixture.daemon.reload();

    assert!(fixture
        .daemon
        .driver_signature_refusal("openrobot/grasp", &digest)
        .is_none());
}

// ── §38.1's profile signature ────────────────────────────────────────────────

const PROFILE_DIGEST: &str =
    "sha256:5555555555555555555555555555555555555555555555555555555555555555";

/// A desired state with one profile, `SIGNATURE` replaced by a member or by nothing.
const DESIRED_WITH_PROFILE: &str = r#"{
  "generation": 9,
  "os": { "channel": "stable", "version": "" },
  "packages": [],
  "profile": { "name": "site-a", "digest": "PROFILE_DIGEST", "verified": true SIGNATURE,
               "limits": { "joint_velocity_max_rad_s": 1.5 } }
}"#;

fn desired_profile(fixture: &Fixture, signature: Option<(&str, &str)>) {
    let signature = match signature {
        None => String::new(),
        Some((key, value)) => {
            format!(", \"signature\": {{ \"key\": \"{key}\", \"value\": \"{value}\" }}")
        }
    };
    fixture.write_desired(
        &DESIRED_WITH_PROFILE
            .replace("PROFILE_DIGEST", PROFILE_DIGEST)
            .replace("SIGNATURE", &signature),
    );
}

/// The hole this closes, stated as a test.
///
/// The document says `"verified": true`. The desired state is a file on the robot's own disk, which
/// is precisely where §38.1 puts the attacker — *"somebody editing limits on a robot's disk"* — so
/// anybody who could raise the ceilings could raise that boolean in the same pass. The robot now
/// checks instead of believing.
#[test]
fn a_profile_claiming_to_be_verified_is_not_believed() {
    let mut fixture = Fixture::new("profile-claimed");
    desired_profile(&fixture, None);
    fixture.daemon.reload();

    let profile = fixture.daemon.desired.profile.as_ref().expect("a profile");
    assert!(
        !profile.verified,
        "the document asserted its own verification"
    );
}

#[test]
fn a_profile_signed_by_a_trusted_key_is_verified() {
    let mut fixture = Fixture::new("profile-signed");
    let signer = driver_key(11);
    write_trust(&fixture, "fleet-admin", &public_hex(&signer));

    let signature = signer.sign(&crate::trust::profile_message(PROFILE_DIGEST));
    desired_profile(&fixture, Some((&public_hex(&signer), &hex_of(&signature))));
    fixture.daemon.reload();

    assert!(fixture.daemon.desired.profile.as_ref().unwrap().verified);
}

/// A stranger's valid signature on the kernel's ceilings is the case §38.1 is about.
#[test]
fn a_profile_signed_by_a_stranger_is_not_verified() {
    let mut fixture = Fixture::new("profile-stranger");
    write_trust(&fixture, "fleet-admin", &public_hex(&driver_key(11)));

    let stranger = driver_key(12);
    let signature = stranger.sign(&crate::trust::profile_message(PROFILE_DIGEST));
    desired_profile(
        &fixture,
        Some((&public_hex(&stranger), &hex_of(&signature))),
    );
    fixture.daemon.reload();

    assert!(!fixture.daemon.desired.profile.as_ref().unwrap().verified);
}

/// §38.1's alarm, raised at last (§34.4).
///
/// Open since the thirteenth session for want of anywhere to put it. A signature that verifies
/// against nothing was made over **other bytes**, which is what §38.1's attacker — "somebody
/// editing limits on a robot's disk" — leaves behind. The limits are refused either way; what was
/// missing was anybody being told.
#[test]
fn a_failed_profile_signature_raises_an_incident() {
    let mut fixture = Fixture::new("profile-incident");
    let signer = driver_key(14);
    write_trust(&fixture, "fleet-admin", &public_hex(&signer));

    // A well-formed signature over a different digest: it parses, it verifies against nothing.
    let over_other_bytes = signer.sign(&crate::trust::profile_message(
        "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    ));
    desired_profile(
        &fixture,
        Some((&public_hex(&signer), &hex_of(&over_other_bytes))),
    );
    fixture.daemon.reload();

    assert!(!fixture.daemon.desired.profile.as_ref().unwrap().verified);

    let queue = crate::outbound::Queue::of(fixture.daemon.store.layout());
    let queued = queue.items();
    assert_eq!(queued.len(), 1, "{queued:?}");
    assert_eq!(queued[0].class, crate::outbound::Class::Incident);
    assert!(
        queued[0].body.contains("profile_signature_failed"),
        "{:?}",
        queued[0].body
    );
    // The incident says the limits were not applied, because an operator reading it needs to know
    // whether the robot is currently running on ceilings somebody edited.
    assert!(
        queued[0].body.contains("NOT applied"),
        "{:?}",
        queued[0].body
    );
}

/// The two states that are **not** incidents, and the reason is the same one §37.1 gives for
/// keeping `untrusted` apart from `failed`: both are administrative gaps, both are already visible
/// on every surface, and raising an incident for either trains an operator to close incidents
/// without reading them.
#[test]
fn a_missing_or_untrusted_profile_signature_raises_nothing() {
    for (label, trusted) in [
        ("profile-incident-absent", true),
        ("profile-incident-stranger", false),
    ] {
        let mut fixture = Fixture::new(label);
        let signer = driver_key(15);
        write_trust(&fixture, "fleet-admin", &public_hex(&driver_key(16)));

        if trusted {
            desired_profile(&fixture, None);
        } else {
            let signature = signer.sign(&crate::trust::profile_message(PROFILE_DIGEST));
            desired_profile(&fixture, Some((&public_hex(&signer), &hex_of(&signature))));
        }
        fixture.daemon.reload();

        assert!(!fixture.daemon.desired.profile.as_ref().unwrap().verified);
        let queue = crate::outbound::Queue::of(fixture.daemon.store.layout());
        assert!(queue.items().is_empty(), "{label}: {:?}", queue.items());
    }
}

/// A profile signature made over the package domain does not apply limits, and the reverse.
#[test]
fn a_package_signature_cannot_stand_in_for_a_profile_signature() {
    let mut fixture = Fixture::new("profile-domain");
    let signer = driver_key(13);
    write_trust(&fixture, "fleet-admin", &public_hex(&signer));

    let wrong_domain = signer.sign(&crate::trust::package_message(PROFILE_DIGEST));
    desired_profile(
        &fixture,
        Some((&public_hex(&signer), &hex_of(&wrong_domain))),
    );
    fixture.daemon.reload();

    assert!(!fixture.daemon.desired.profile.as_ref().unwrap().verified);
}

const DESIRED_ONE_POLICY: &str = r#"{
  "generation": 7,
  "os": { "channel": "stable", "version": "" },
  "packages": [
    { "id": "openrobot/grasp", "version": "1.0.0", "digest": "DIGEST",
      "kind": "policy", "state": "running", "grants": ["manipulation.grasp"] }
  ]
}"#;

#[cfg(unix)]
mod daemon {
    use super::*;

    fn with_policy(fixture: &mut Fixture) -> String {
        let source = fixture.root.path().join("source");
        let m = manifest("openrobot/grasp", "1.0.0", "policy");
        let digest = write_package(&source, &m, &[("bin/run", "#!/bin/sh\nsleep 1\n")]);
        let id = PackageId::parse("openrobot/grasp").unwrap();
        fixture
            .daemon
            .store
            .stage(&id, "policy", "1.0.0", &digest, &source)
            .unwrap();
        fixture.write_desired(&DESIRED_ONE_POLICY.replace("DIGEST", &digest));
        fixture.daemon.reload();
        digest
    }

    #[test]
    fn a_locally_staged_package_converges_without_a_network() {
        let mut fixture = Fixture::new("converge");
        let digest = with_policy(&mut fixture);
        fixture.daemon.safe_state = SafeState::Yes;

        // `converge` runs passes until a plan taken from a fresh observation is empty. It takes
        // more than one: activating a package is what makes it startable, and §33.3 computes one
        // plan per observation rather than re-planning inside a pass.
        let passes = fixture.daemon.converge(1_000);
        assert!(passes.len() > 1, "activation and start are separate passes");
        assert!(passes.last().unwrap().converged());
        let id = PackageId::parse("openrobot/grasp").unwrap();
        assert_eq!(
            fixture.daemon.store.active_digest(&id).as_deref(),
            Some(digest.as_str())
        );
        assert!(fixture
            .daemon
            .launcher
            .calls
            .contains(&"start openrobot/grasp".to_string()));

        // Idempotent, which is the whole claim of §33.3: the second pass has nothing to do.
        let second = fixture.daemon.reconcile(2_000);
        assert!(second.plan.is_empty(), "{:?}", second.plan.actions);
        assert!(second.converged());
    }

    /// §33.4's barrier, observed rather than asserted: starting waits, and the deferral is reported
    /// with the reason rather than silently dropped.
    #[test]
    fn starting_waits_for_a_safe_state_and_says_so() {
        let mut fixture = Fixture::new("barrier");
        with_policy(&mut fixture);
        fixture.daemon.safe_state = SafeState::No;

        let passes = fixture.daemon.converge(1_000);
        let pass = passes.last().unwrap();
        let deferred: Vec<&str> = pass
            .deferred
            .iter()
            .map(|(action, _)| api::action_name(&action.kind))
            .collect();
        assert!(deferred.contains(&"start_package"), "{deferred:?}");
        assert!(pass.deferred[0].1.contains("waiting for a safe state"));
        assert!(!pass.converged());
        assert!(!fixture
            .daemon
            .launcher
            .calls
            .contains(&"start openrobot/grasp".to_string()));

        // And it starts once the gate opens, with no re-plan needed beyond the next pass.
        fixture.daemon.safe_state = SafeState::Yes;
        let passes = fixture.daemon.converge(2_000);
        assert!(passes.last().unwrap().converged());
        assert!(fixture
            .daemon
            .launcher
            .calls
            .contains(&"start openrobot/grasp".to_string()));
    }

    /// The generation is a claim about what the robot is running. §17: the platform never claims a
    /// robot is running something it has not heard from — and the robot must not claim it of itself.
    #[test]
    fn the_generation_is_not_stamped_while_anything_is_outstanding() {
        let mut fixture = Fixture::new("generation");
        with_policy(&mut fixture);
        fixture.daemon.safe_state = SafeState::No;

        fixture.daemon.converge(1_000);
        let observed = fixture.daemon.observe(1_000);
        assert_eq!(
            observed.generation, 0,
            "the start is deferred, so the robot is not running generation 7 and must not say so"
        );

        fixture.daemon.safe_state = SafeState::Yes;
        fixture.daemon.converge(2_000);
        let observed = fixture.daemon.observe(2_000);
        assert_eq!(observed.generation, 7);
    }

    /// OTA is not written, and the action stays in the plan carrying that. A supervisor that
    /// dropped it would converge on a lie.
    #[test]
    fn an_os_update_nobody_can_apply_is_reported_rather_than_dropped() {
        let mut fixture = Fixture::new("ota");
        fixture.write_desired(
            r#"{"generation":3,"os":{"channel":"stable","version":"2.0.0"},"packages":[]}"#,
        );
        fixture.daemon.reload();
        fixture.daemon.safe_state = SafeState::Yes;

        let passes = fixture.daemon.converge(1_000);
        let pass = passes.last().unwrap();
        assert_eq!(pass.applied.len(), 1);
        let (action, outcome) = &pass.applied[0];
        assert_eq!(api::action_name(&action.kind), "stage_os_image");
        match outcome {
            Outcome::Unimplemented { reason } => assert!(reason.contains("§12")),
            other => panic!("expected unimplemented, got {other:?}"),
        }
        assert!(!pass.converged());
        assert_eq!(
            fixture.daemon.observe(1_000).generation,
            0,
            "and the generation is not advanced past it"
        );
    }

    #[test]
    fn a_package_the_desired_state_stops_naming_is_removed_and_its_blob_collected() {
        let mut fixture = Fixture::new("drop");
        let digest = with_policy(&mut fixture);
        fixture.daemon.safe_state = SafeState::Yes;
        fixture.daemon.converge(1_000);

        fixture.write_desired(
            r#"{"generation":8,"os":{"channel":"stable","version":""},"packages":[]}"#,
        );
        fixture.daemon.reload();

        // Removing a running package takes two passes: the removal is emitted only once the stop
        // has been observed, which §33.3 requires and the previous session's log calls out as
        // correct rather than as a missing optimisation.
        let passes = fixture.daemon.converge(2_000);
        assert!(
            passes.len() >= 2,
            "the stop is observed before the removal is emitted"
        );

        let id = PackageId::parse("openrobot/grasp").unwrap();
        assert!(fixture.daemon.store.active_digest(&id).is_none());
        assert!(!fixture.daemon.store.layout().blob(&digest).exists());
        assert!(fixture
            .daemon
            .launcher
            .calls
            .contains(&"stop openrobot/grasp".to_string()));
    }

    #[test]
    fn a_grant_is_written_and_a_revocation_takes_the_lease_with_it() {
        let mut fixture = Fixture::new("grants");
        with_policy(&mut fixture);
        fixture.daemon.safe_state = SafeState::Yes;
        fixture.daemon.converge(1_000);

        let observed = fixture.daemon.observe(1_000);
        assert_eq!(
            observed.grants,
            vec![(
                "openrobot/grasp".to_string(),
                "manipulation.grasp".to_string()
            )]
        );

        // Take the lease, then revoke the grant underneath it.
        assert_eq!(
            fixture
                .daemon
                .leases
                .acquire("openrobot/grasp", "manipulation.grasp", true, false),
            skynetd::lease::PreemptionOutcome::Acquired
        );
        fixture.write_desired(
            &DESIRED_ONE_POLICY
                .replace("DIGEST", &fixture.daemon.store.installed()[0].1)
                .replace(r#"["manipulation.grasp"]"#, "[]"),
        );
        fixture.daemon.reload();
        fixture.daemon.converge(2_000);

        assert!(fixture.daemon.observe(2_000).grants.is_empty());
        assert!(
            fixture.daemon.leases.holder("manipulation.grasp").is_none(),
            "a revocation must not take the permission and leave the exclusivity"
        );
    }

    #[test]
    fn a_package_with_no_entry_point_fails_with_a_sentence_about_the_manifest() {
        let mut fixture = Fixture::new("no-entry");
        let source = fixture.root.path().join("source");
        let mut m = manifest("openrobot/grasp", "1.0.0", "policy");
        m.set("runtime", Json::obj());
        let digest = write_package(&source, &m, &[("bin/run", "x")]);
        let id = PackageId::parse("openrobot/grasp").unwrap();
        fixture
            .daemon
            .store
            .stage(&id, "policy", "1.0.0", &digest, &source)
            .unwrap();
        fixture.write_desired(&DESIRED_ONE_POLICY.replace("DIGEST", &digest));
        fixture.daemon.reload();
        fixture.daemon.safe_state = SafeState::Yes;

        let passes = fixture.daemon.converge(1_000);
        let failure = passes
            .iter()
            .flat_map(|pass| pass.applied.iter())
            .find(|(action, _)| api::action_name(&action.kind) == "start_package")
            .map(|(_, outcome)| outcome.clone())
            .expect("a start was attempted");
        match failure {
            Outcome::Failed { reason } => assert!(reason.contains("entrypoint"), "{reason}"),
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    /// A manifest naming `../../bin/sh` would be a package running something else entirely, under
    /// this package's grants. Staging refuses it, and so does the launcher — the second is what
    /// catches a blob that arrived some other way, which is why this test writes one directly
    /// rather than going through [`Store::stage`].
    #[test]
    fn an_entry_point_that_leaves_the_package_is_refused_at_launch_too() {
        let mut fixture = Fixture::new("escape");
        let mut m = manifest("openrobot/grasp", "1.0.0", "policy");
        let mut runtime = Json::obj();
        runtime.set("entrypoint", Json::from("../../../bin/sh"));
        m.set("runtime", runtime);

        // Straight into the store, past every check staging would have applied.
        let digest = "e".repeat(64);
        let blob = fixture.daemon.store.layout().blob(&digest);
        std::fs::create_dir_all(&blob).unwrap();
        std::fs::write(blob.join("manifest.json"), m.to_canonical().unwrap()).unwrap();
        let id = PackageId::parse("openrobot/grasp").unwrap();
        fixture.daemon.store.activate(&id, &digest).unwrap();

        fixture.write_desired(&DESIRED_ONE_POLICY.replace("DIGEST", &digest));
        fixture.daemon.reload();
        fixture.daemon.safe_state = SafeState::Yes;

        let passes = fixture.daemon.converge(1_000);
        assert!(passes
            .iter()
            .flat_map(|pass| pass.applied.iter())
            .any(|(_, outcome)| matches!(outcome, Outcome::Failed { reason } if reason.contains("inside the package"))));
        assert!(fixture
            .daemon
            .launcher
            .calls
            .iter()
            .all(|call| call != "start openrobot/grasp"));
    }
}

// ── §33.8's ladder ───────────────────────────────────────────────────────────

#[test]
fn a_desired_state_that_decodes_is_promoted_to_the_last_known_good_copy() {
    let fixture = Fixture::new("ladder-good");
    let layout = fixture.daemon.store.layout().clone();
    fixture.write_desired(r#"{"generation":3,"os":{"version":""},"packages":[]}"#);

    let (desired, source) = crate::runtime::load_desired(&layout);
    assert_eq!(desired.generation, 3);
    assert_eq!(source, DesiredSource::Current);
    assert!(layout.desired_last_good().is_file());
}

#[test]
fn a_corrupt_desired_state_falls_back_rather_than_becoming_an_empty_one() {
    let fixture = Fixture::new("ladder-corrupt");
    let layout = fixture.daemon.store.layout().clone();
    fixture.write_desired(r#"{"generation":3,"os":{"version":""},"packages":[]}"#);
    let _ = crate::runtime::load_desired(&layout);

    // Now break the current copy.
    fixture.write_desired("{ this is not json");
    let (desired, source) = crate::runtime::load_desired(&layout);
    assert_eq!(
        desired.generation, 3,
        "the last-known-good copy, not nothing"
    );
    match source {
        DesiredSource::LastKnownGood { because } => assert!(because.contains("does not parse")),
        other => panic!("expected the fallback, got {other:?}"),
    }
}

/// "The Hub says run nothing" and "we could not read what the Hub said" produce the same plan and
/// are completely different facts.
#[test]
fn with_nothing_readable_at_all_the_answer_is_run_nothing_and_it_says_why() {
    let fixture = Fixture::new("ladder-nothing");
    let layout = fixture.daemon.store.layout().clone();
    let (desired, source) = crate::runtime::load_desired(&layout);
    assert!(desired.packages.is_empty());
    assert_eq!(desired.generation, 0);
    match source {
        DesiredSource::RunNothing { because } => {
            assert!(because.contains("could not be read"));
            assert!(because.contains("last-known-good"));
        }
        other => panic!("expected run-nothing, got {other:?}"),
    }
}

/// A desired state naming a package id that is also a path is refused at decode rather than
/// producing a directory outside the store. Belt and braces: the id is checked again at apply.
#[test]
fn a_desired_state_naming_a_hostile_id_never_becomes_a_directory() {
    let mut fixture = Fixture::new("hostile-desired");
    fixture.write_desired(
        r#"{"generation":1,"os":{"version":""},"packages":[
            {"id":"../../etc/systemd/system","version":"1","digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
             "kind":"policy","state":"running","grants":[]}]}"#,
    );
    fixture.daemon.reload();
    fixture.daemon.safe_state = SafeState::Yes;
    let passes = fixture.daemon.converge(1_000);

    for (_, outcome) in passes.iter().flat_map(|pass| pass.applied.iter()) {
        assert!(!matches!(outcome, Outcome::Applied), "{outcome:?}");
    }
    assert!(!fixture
        .root
        .path()
        .join("data/packages/active/../../etc")
        .exists());
    assert!(fixture.daemon.store.installed().is_empty());
}

/// §33.4's barrier stands on this value, and nothing in this daemon measures it — so the rule is
/// that anything but an explicit `yes` keeps the barrier shut.
#[test]
fn a_safe_state_is_only_ever_declared_explicitly() {
    let fixture = Fixture::new("safe-state");
    let layout = fixture.daemon.store.layout().clone();
    let mut daemon = fixture.daemon;

    daemon.safe_state = SafeState::Yes;
    daemon.refresh_safe_state();
    assert_eq!(
        daemon.safe_state,
        SafeState::No,
        "an absent file is not a safe state"
    );

    for text in ["no", "", "true", "YES", "yes please", "1", "y"] {
        std::fs::write(layout.safe_state(), text).unwrap();
        daemon.safe_state = SafeState::Yes;
        daemon.refresh_safe_state();
        assert_eq!(daemon.safe_state, SafeState::No, "`{text}` is not `yes`");
    }

    std::fs::write(
        layout.safe_state(),
        "yes
",
    )
    .unwrap();
    daemon.refresh_safe_state();
    assert_eq!(daemon.safe_state, SafeState::Yes);
}

/// The note used to say the safe state was "declared, not measured", and that was the whole truth
/// until §27's machine existed. It is now declared **and** narrowed by that machine, and the note
/// has to say so — a console repeating a sentence that stopped being true is exactly the failure
/// §32.2 spends its length on.
///
/// What is still true, and still said: nothing here has *measured* anything about the body.
#[test]
fn status_says_where_the_safe_state_came_from_and_that_nothing_measured_it() {
    let mut fixture = Fixture::new("safe-state-status");
    let (_, body) = fixture.request("GET", "/status", None, "");
    assert_eq!(
        body.get("safe_state_source").and_then(Json::as_str),
        Some("declared_and_derived")
    );
    let note = body
        .get("safe_state_note")
        .and_then(Json::as_str)
        .unwrap_or_default();
    assert!(note.contains("narrowed"), "the note names both sources");
    assert!(
        note.contains("measured the body"),
        "and still says nothing measured the body"
    );
}

/// §27's state reaches `GET /status`, which its own route table has claimed since the module was
/// written. A robot that has just booted and observed nothing is in `post`, and one pass of the
/// loop moves it to `safe` with `rhal_lost` — because this edition has no kernel and no RHAL, and
/// the machine says the least capable thing rather than the hopeful one.
#[test]
fn status_carries_the_operating_state_and_the_reason_it_is_there() {
    let mut fixture = Fixture::new("state-status");
    let (_, before) = fixture.request("GET", "/status", None, "");
    assert_eq!(before.get("state").and_then(Json::as_str), Some("post"));
    assert_eq!(
        before.get("state_reason").and_then(Json::as_str),
        Some("post_incomplete"),
        "a daemon that has not observed has not observed a failure either"
    );

    fixture.daemon.refresh_operating_state(None);
    let (_, after) = fixture.request("GET", "/status", None, "");
    assert_eq!(after.get("state").and_then(Json::as_str), Some("safe"));
    assert_eq!(
        after.get("state_reason").and_then(Json::as_str),
        Some("rhal_lost")
    );
}

/// `GET /state` says which of §27's eight conditions anybody looked at, and this edition looks at
/// one. A page that showed the state without that would be reporting a robot as safe when the
/// truth is that nothing has looked at it.
#[test]
fn the_state_route_admits_how_little_it_observed() {
    let mut fixture = Fixture::new("state-route");
    fixture.provision();
    fixture.daemon.refresh_operating_state(None);
    let (status, body) = fixture.request("GET", "/state", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.get("state").and_then(Json::as_str), Some("safe"));
    assert_eq!(body.get("reason").and_then(Json::as_str), Some("rhal_lost"));
    // The three properties other sections read off the state, rendered rather than re-derived.
    assert_eq!(body.get("safe_state").and_then(Json::as_bool), Some(true));
    assert_eq!(body.get("commands").and_then(Json::as_bool), Some(false));
    assert_eq!(body.get("sticky").and_then(Json::as_bool), Some(true));

    let observed = body.get("observed").expect("the observation map");
    assert_eq!(observed.get("post").and_then(Json::as_bool), Some(true));
    for unobserved in [
        "kernel",
        "rhal",
        "homing",
        "stop",
        "guard_hold",
        "guards",
        "policy",
    ] {
        assert_eq!(
            observed.get(unobserved).and_then(Json::as_bool),
            Some(false),
            "`{unobserved}` is not this process's to observe (§33.1)"
        );
    }
}

/// The narrowing, from the direction that matters. §33.4's asymmetry says either source may close
/// the barrier and neither opens it alone — so an operator's `yes` cannot outvote a machine that
/// can see a policy commanding, whatever else is true.
///
/// The state has to be set directly, because no observation this edition can make produces
/// `running`: the kernel and the RHAL are on the far side of §33.1. That is the point of asserting
/// it now rather than when the channel exists — the rule is in place before anything can trip it.
#[test]
fn a_declared_safe_state_cannot_outvote_a_robot_that_is_commanding() {
    let fixture = Fixture::new("safe-state-narrowing");
    let layout = fixture.daemon.store.layout().clone();
    let mut daemon = fixture.daemon;
    std::fs::write(layout.safe_state(), "yes").unwrap();

    daemon.operating_state = OperatingState::Hold;
    daemon.refresh_safe_state();
    assert_eq!(daemon.safe_state, SafeState::Yes, "hold is a safe state");

    for commanding in [OperatingState::Running, OperatingState::Degraded] {
        daemon.operating_state = commanding;
        daemon.refresh_safe_state();
        assert_eq!(
            daemon.safe_state,
            SafeState::No,
            "`{}` is a policy commanding the body",
            commanding.as_str()
        );
    }

    // And the state most likely to be mistaken for a safe one by somebody watching a robot slow
    // down. §33.4's barrier does not open mid-ramp.
    daemon.operating_state = OperatingState::Stopping;
    daemon.refresh_safe_state();
    assert_eq!(daemon.safe_state, SafeState::No);
}

// ── The body (§5) ────────────────────────────────────────────────────────────

/// A descriptor that would load, for the tests below.
const BENCH_ARM: &str = r#"{
  "robot_slug": "bench-arm",
  "base": "fixed",
  "dof": 1,
  "joints": [{"name": "shoulder", "type": "revolute", "control_modes": ["position"]}]
}"#;

/// The same arm, declaring the two things §46 needs and one inversion to look at.
///
/// The console fixture uses this rather than [`BENCH_ARM`] so the watchdog panel has something to
/// draw: a body that declares nothing produces a correct and completely empty panel, which shows
/// that the panel handles absence and not that it handles the case it exists for.
const BENCH_ARM_WATCHDOG: &str = r#"{
  "robot_slug": "bench-arm",
  "base": "fixed",
  "dof": 1,
  "on_command_loss": "coast",
  "envelope": { "watchdog_ms": 100 },
  "watchdog": { "soc_wdt_ms": 40, "drive_command_timeout_ms": 500 },
  "joints": [{"name": "shoulder", "type": "revolute", "control_modes": ["position"]}]
}"#;

fn write_embodiment(fixture: &Fixture, document: &str) {
    let path = fixture.daemon.store.layout().embodiment();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, document).unwrap();
}

/// The three states, over the wire, through the real router.
///
/// The distinction between the first two is the one worth an integration test: an operator seeing
/// `absent` has a provisioning step to run and an operator seeing `malformed` has a generator to
/// fix, and collapsing them into "no body" would send half of them to the wrong job.
#[test]
fn the_body_is_reported_as_absent_malformed_or_present() {
    let mut fixture = Fixture::new("api-embodiment");
    fixture.provision();

    let (status, body) = fixture.request("GET", "/embodiment", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.get("state").and_then(Json::as_str), Some("absent"));
    assert_eq!(body.get("digest"), Some(&Json::Null));

    write_embodiment(&fixture, "{ not json");
    let (status, body) = fixture.request("GET", "/embodiment", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.get("state").and_then(Json::as_str), Some("malformed"));
    assert!(body
        .get("detail")
        .and_then(Json::as_str)
        .is_some_and(|d| d.contains("not valid JSON")));

    write_embodiment(&fixture, BENCH_ARM);
    let (status, body) = fixture.request("GET", "/embodiment", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.get("state").and_then(Json::as_str), Some("present"));
    assert_eq!(
        body.get("robot_slug").and_then(Json::as_str),
        Some("bench-arm")
    );
    assert_eq!(
        body.get("digest").and_then(Json::as_str).map(str::len),
        Some(64)
    );
    // §5 has the vendor sign the descriptor, and the check now runs. Nobody signed this one, so the
    // answer is `absent` — a fact about the file rather than a verdict about its contents — and the
    // digest is reported beside it without being allowed to imply a provenance it does not carry.
    assert_eq!(body.get("signature").and_then(Json::as_str), Some("absent"));
    assert_eq!(body.get("signer"), Some(&Json::Null));
    assert!(body
        .get("signature_note")
        .and_then(Json::as_str)
        .is_some_and(|note| note.contains("identifies the file")));
}

/// §5, end to end: a descriptor signed by a key this robot trusts reports its signer by name.
#[test]
fn a_signed_descriptor_names_its_vendor_on_the_wire() {
    let mut fixture = Fixture::new("api-embodiment-signed");
    fixture.provision();
    write_embodiment(&fixture, BENCH_ARM);

    // The digest is the daemon's own, so the signature is over what the robot computed rather than
    // over what a test asserted it would compute.
    let layout = fixture.daemon.store.layout().clone();
    let body = crate::embodiment::load(&layout);
    let digest = body.known().expect("a body").digest_hex.clone();

    let vendor = driver_key(21);
    write_trust(&fixture, "bench-robotics", &public_hex(&vendor));
    let signature = vendor.sign(&crate::trust::embodiment_message(&digest));
    std::fs::write(
        layout.embodiment_signature(),
        format!(
            "{{\"key\":\"{}\",\"value\":\"{}\"}}",
            public_hex(&vendor),
            hex_of(&signature)
        ),
    )
    .unwrap();

    let (status, answer) = fixture.request("GET", "/embodiment", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(
        answer.get("signature").and_then(Json::as_str),
        Some("verified")
    );
    assert_eq!(
        answer.get("signer").and_then(Json::as_str),
        Some("bench-robotics")
    );
}

/// It is a read, and §39.1 gives reads to the observer.
#[test]
fn the_body_needs_a_credential_and_a_wrong_method_is_not_a_404() {
    let mut fixture = Fixture::new("api-embodiment-auth");
    let (status, _) = fixture.request("GET", "/embodiment", None, "");
    assert_eq!(status, 401, "an unprovisioned robot has no observer");

    // §33.7 distinguishes "no such route" from "not that method here", and a path added without a
    // `known_path` entry silently loses that distinction.
    //
    // With a credential, because the authorisation check runs *before* the route match: an
    // unauthenticated caller gets 401 for every path, real or not. That is fail-closed rather than
    // an oversight — route enumeration is not something to hand out for free — and it means this
    // distinction is only observable by somebody the robot already trusts.
    fixture.provision();
    let (status, _) = fixture.request("POST", "/embodiment", Some("admin-token"), "");
    assert_eq!(status, 405);
    let (status, _) = fixture.request("GET", "/embodiment", Some("observer-token"), "");
    assert_eq!(status, 200, "and the observer may read it");
}

/// What admission would be told about this robot, or why it cannot be told anything.
///
/// The answer is platform-dependent and both branches are asserted, because the interesting
/// property is the same either way: **nothing is estimated**. On Linux the memory figure is read;
/// everywhere else the capacity does not assemble and says so. What must never happen is a
/// plausible default, because §8.1 makes the budget check the one place this format is stricter
/// than an APK — a robot that runs out of memory mid-stride falls over.
#[test]
fn the_capacity_is_read_or_refused_and_never_estimated() {
    let mut fixture = Fixture::new("api-capacity");
    fixture.provision();

    let (status, body) = fixture.request("GET", "/capacity", Some("observer-token"), "");
    assert_eq!(status, 200);

    match body.get("state").and_then(Json::as_str) {
        Some("known") => {
            let ram = body.get("ram_mb").and_then(Json::as_f64).expect("ram_mb");
            assert!(
                ram > 64.0,
                "{ram} MB is not a machine that runs this daemon"
            );
            // Absent rather than zero. A console rendering `vram_mb: 0` reports a robot with no
            // accelerator, which is a different claim from one nothing has measured.
            assert_eq!(body.get("vram_mb"), Some(&Json::Null));
            assert_eq!(body.get("disk_mb"), Some(&Json::Null));
            assert!(body.get_present("unmeasured").is_some());
            // §38.1: a withheld-capability list read from an unverified profile would be a
            // safety-relevant value anybody on the robot could edit.
            assert!(body.get_present("denied_note").is_some());
        }
        Some("unavailable") => {
            assert!(body
                .get("detail")
                .and_then(Json::as_str)
                .is_some_and(|d| d.contains("/proc/meminfo")));
            // Not only the cause. An operator reading this needs to know what it stops.
            assert!(body
                .get("consequence")
                .and_then(Json::as_str)
                .is_some_and(|c| c.contains("Admission cannot run")));
        }
        other => panic!("unexpected capacity state: {other:?}"),
    }
}

/// A body nobody can read is not a body to make an install decision about.
///
/// The malformed case is the one worth an integration test: an *absent* descriptor still assembles
/// — the capability set is empty, which §5.0 makes a claim rather than an absence — but a
/// descriptor that exists and does not parse leaves what this body provides genuinely unknown, and
/// admission must not proceed on a guess.
#[test]
fn a_malformed_descriptor_stops_the_capacity_from_assembling() {
    let mut fixture = Fixture::new("api-capacity-malformed");
    fixture.provision();
    write_embodiment(&fixture, "{ not json");

    let (status, body) = fixture.request("GET", "/capacity", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(
        body.get("state").and_then(Json::as_str),
        Some("unavailable")
    );
    assert!(body
        .get("detail")
        .and_then(Json::as_str)
        .is_some_and(|d| d.contains("descriptor")));
}

// ── The console (§32.2) ──────────────────────────────────────────────────────

/// The property the whole console rests on, checked through the router.
///
/// `Fixture::new` builds an **unprovisioned** robot — no credentials at all — and this asks for the
/// page with no token. §39.2: "an unprovisioned robot's console offers status and stop and nothing
/// else", and a page that answered 401 would offer neither. The catch-all in `requirement` is
/// `Role::Admin`, so this is one table entry away from being wrong at any time.
#[test]
fn the_console_loads_on_an_unprovisioned_robot_with_no_credential() {
    let mut fixture = Fixture::new("console-anon");
    assert!(!fixture.credentials.is_provisioned());

    for (target, kind) in [
        ("/", "text/html"),
        ("/console.css", "text/css"),
        ("/console.js", "text/javascript"),
    ] {
        let response = fixture.raw("GET", target, None);
        assert_eq!(
            response.status, 200,
            "{target} did not load without a token"
        );
        assert!(
            response.content_type.starts_with(kind),
            "{target} came back as {}",
            response.content_type
        );
        assert!(!response.body.is_empty(), "{target} is empty");
    }
}

/// The console is served, and the API underneath it is not changed by that.
///
/// The asset arm runs before the authorisation check, so the risk it introduces is swallowing a
/// route it should not. `/status` is anonymous and `/health` is not, and both must behave exactly
/// as they did.
#[test]
fn routing_the_console_first_does_not_swallow_the_api() {
    let mut fixture = Fixture::new("console-passthrough");

    let (status, _) = fixture.request("GET", "/status", None, "");
    assert_eq!(status, 200, "an anonymous API route still answers");

    let (status, _) = fixture.request("GET", "/health", None, "");
    assert_eq!(status, 401, "an authenticated one still refuses");

    let (status, _) = fixture.request("GET", "/nothing-here", None, "");
    assert_eq!(status, 401, "an unknown path still fails closed");

    // Only GET. A console asset is not something to POST to, and the asset arm must not turn
    // `POST /` into a 200 page.
    let response = fixture.raw("POST", "/", None);
    assert_ne!(response.status, 200, "POST / is not the console");
}

/// Every response carries the policy that makes the page unable to reach off the robot.
#[test]
fn the_console_is_served_under_a_policy_that_permits_no_other_origin() {
    let mut fixture = Fixture::new("console-csp");
    for target in ["/", "/console.css", "/console.js"] {
        let response = fixture.raw("GET", target, None);
        let policy = response
            .extra
            .iter()
            .find(|(name, _)| name == "Content-Security-Policy")
            .map(|(_, value)| value.clone())
            .unwrap_or_else(|| panic!("{target} carries no Content-Security-Policy"));
        assert!(policy.contains("default-src 'none'"), "{target}: {policy}");
        assert!(!policy.contains("unsafe-inline"), "{target}: {policy}");

        // The headers every response already carried, still there on the console's own.
        let encoded = String::from_utf8(response.encode()).unwrap();
        assert!(encoded.contains("X-Frame-Options: DENY"), "{target}");
        assert!(encoded.contains("Cache-Control: no-store"), "{target}");
    }
}

// ── The API ──────────────────────────────────────────────────────────────────

#[test]
fn status_is_answered_without_a_credential_and_says_stop_is_available() {
    let mut fixture = Fixture::new("api-status");
    let (status, body) = fixture.request("GET", "/status", None, "");
    assert_eq!(status, 200);
    assert_eq!(body.get("stop_available"), Some(&Json::Bool(true)));
    assert_eq!(body.get("provisioned"), Some(&Json::Bool(false)));
    assert_eq!(
        body.get("desired_source").and_then(Json::as_str),
        Some("run_nothing")
    );
    assert!(body.get_present("desired_source_reason").is_some());
}

#[test]
fn stop_is_answered_without_a_credential() {
    let mut fixture = Fixture::new("api-stop");
    fixture.daemon.launcher = RecordingLauncher::with_units(vec![UnitStatus {
        id: "openrobot/grasp".to_string(),
        state: UnitState::Running,
        restarts: 0,
        last_exit: None,
    }]);

    let (status, body) = fixture.request("POST", "/stop", None, "");
    assert_eq!(status, 200);
    assert_eq!(
        body.get("stopped")
            .and_then(Json::as_arr)
            .map(<[Json]>::len),
        Some(1)
    );
    assert!(fixture
        .daemon
        .launcher
        .calls
        .contains(&"stop openrobot/grasp".to_string()));
}

#[test]
fn everything_else_needs_a_credential_and_says_which_one() {
    let mut fixture = Fixture::new("api-401");
    let (status, body) = fixture.request("GET", "/health", None, "");
    assert_eq!(status, 401);
    let detail = body
        .get("detail")
        .and_then(Json::as_str)
        .unwrap_or_default();
    assert!(detail.contains("observer"), "{detail}");
    assert!(detail.contains("no default password"), "{detail}");
}

#[test]
fn a_credential_that_is_real_but_too_narrow_is_told_that_and_not_sent_to_pair_again() {
    let mut fixture = Fixture::new("api-403");
    fixture.provision();
    let (status, body) = fixture.request("POST", "/grants", Some("observer-token"), "{}");
    assert_eq!(status, 403);
    assert!(body
        .get("detail")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .contains("operator"));
}

#[test]
fn health_reports_what_nobody_has_measured_as_unmeasured() {
    let mut fixture = Fixture::new("api-health");
    fixture.provision();
    fixture.daemon.launcher = RecordingLauncher::with_units(vec![UnitStatus {
        id: "openrobot/grasp".to_string(),
        state: UnitState::Running,
        restarts: 0,
        last_exit: None,
    }]);

    let (status, body) = fixture.request("GET", "/health", Some("observer-token"), "");
    assert_eq!(status, 200);
    for field in [
        "guard_trip_rate",
        "battery",
        "temperature_c",
        "storage_headroom",
    ] {
        assert_eq!(
            body.get(field),
            Some(&Json::Null),
            "{field} must not be zero"
        );
    }
    let packages = body.get("packages").and_then(Json::as_arr).unwrap();
    assert_eq!(
        packages[0].get("readiness").and_then(Json::as_str),
        Some("unknown"),
        "running and unmeasured is not ready"
    );
    assert_eq!(packages[0].get("last_sequence"), Some(&Json::Null));
}

#[test]
fn a_route_that_exists_answers_405_for_a_method_it_does_not() {
    let mut fixture = Fixture::new("api-405");
    fixture.provision();
    let (status, _) = fixture.request("DELETE", "/status", Some("admin-token"), "");
    assert_eq!(status, 405);
    let (status, _) = fixture.request("GET", "/nothing-here", Some("admin-token"), "");
    assert_eq!(status, 404);
}

#[test]
fn a_grant_can_be_added_and_removed_over_the_api() {
    let mut fixture = Fixture::new("api-grants");
    fixture.provision();

    let (status, body) = fixture.request(
        "POST",
        "/grants",
        Some("admin-token"),
        r#"{"id":"openrobot/grasp","token":"manipulation.grasp"}"#,
    );
    assert_eq!(status, 201);
    assert_eq!(body.get("outcome").and_then(Json::as_str), Some("granted"));

    let (_, body) = fixture.request("GET", "/grants", Some("observer-token"), "");
    let held = body.get("grants").unwrap().get("openrobot/grasp").unwrap();
    assert_eq!(held.as_arr().map(<[Json]>::len), Some(1));

    let (status, body) = fixture.request(
        "DELETE",
        "/grants/openrobot:grasp/manipulation.grasp",
        Some("admin-token"),
        "",
    );
    assert_eq!(status, 200);
    assert_eq!(body.get("outcome").and_then(Json::as_str), Some("revoked"));
}

#[test]
fn a_hostile_package_id_in_a_route_is_refused() {
    let mut fixture = Fixture::new("api-hostile");
    fixture.provision();
    let (status, _) = fixture.request("POST", "/packages/..:../start", Some("admin-token"), "");
    assert!(status == 400 || status == 404, "got {status}");
}

#[test]
fn the_plan_route_reports_a_deferral_with_its_reason() {
    let mut fixture = Fixture::new("api-plan");
    fixture.provision();
    fixture.write_desired(
        r#"{"generation":2,"os":{"channel":"stable","version":"9.9.9"},"packages":[]}"#,
    );
    fixture.daemon.reload();
    fixture.daemon.safe_state = SafeState::No;
    fixture.daemon.converge(500);

    let (status, body) = fixture.request("GET", "/plan", Some("observer-token"), "");
    assert_eq!(status, 200);
    let pass = body.get("pass").expect("the pass is nested under `pass`");
    assert_eq!(pass.get("converged"), Some(&Json::Bool(false)));
    let applied = pass.get("applied").and_then(Json::as_arr).unwrap();
    // `failed` rather than `unimplemented`, and the change is the point rather than an
    // adjustment. This case used OTA as a stand-in for "an action nobody can apply", which it was
    // until §12 was built. It is now an action that *is* applied and refuses: this robot has been
    // told to run 9.9.9 and holds no release manifest describing it, and §12 will not install an
    // image whose manifest nobody has seen — because the manifest is what says whether the update
    // touches the Safety Kernel.
    assert_eq!(
        applied[0].get("outcome").and_then(Json::as_str),
        Some("failed")
    );
    let reason = applied[0]
        .get("reason")
        .and_then(Json::as_str)
        .unwrap_or_default();
    assert!(reason.contains("9.9.9"), "{reason}");
}

/// One route, one shape — the assertion that was missing while the console could not render.
///
/// `GET /plan` answered `{"pass": null, "note": …}` before a pass had run and a **flat** object
/// after one, so `value.pass` was `null` in the first case and `undefined` in the second. The
/// console discriminated on `=== null`, took the second branch, and threw on `pass.converged` every
/// two seconds from the moment a robot did anything. Nothing caught it: the CLI happened to read the
/// flat shape, and no test looked at both branches of the same route.
#[test]
fn the_plan_route_answers_the_same_shape_before_and_after_a_pass() {
    let mut fixture = Fixture::new("api-plan-shape");
    fixture.provision();

    // Before: `pass` is present and null, and something says why.
    let (status, body) = fixture.request("GET", "/plan", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.get("pass"), Some(&Json::Null));
    assert!(body.get("note").and_then(Json::as_str).is_some());

    // After: `pass` is present and is the pass. A client that reads `pass` reads both.
    fixture
        .write_desired(r#"{"generation":1,"os":{"channel":"stable","version":""},"packages":[]}"#);
    fixture.daemon.reload();
    let _ = fixture.daemon.reconcile(500);

    let (status, body) = fixture.request("GET", "/plan", Some("observer-token"), "");
    assert_eq!(status, 200);
    let pass = body
        .get_present("pass")
        .expect("a pass has run, so `pass` is an object");
    assert!(pass.get("converged").is_some());
    assert!(pass.get("applied").is_some());
    assert!(pass.get("at_ms").is_some());
    // And nothing is left at the top level for a second parse to find.
    assert!(body.get("converged").is_none(), "{body:?}");
    assert!(body.get("applied").is_none(), "{body:?}");
}

// ── The listeners ────────────────────────────────────────────────────────────

#[cfg(unix)]
mod listeners {
    use super::*;
    use std::io::{Read, Write};

    /// §34.2 says reaching a robot's console from outside its network is not a feature and not a
    /// configuration option, so the check is a refusal and `0.0.0.0` is the case it exists for.
    #[test]
    fn the_console_refuses_to_bind_a_routable_address() {
        for address in [
            "0.0.0.0:0",
            "8.8.8.8:0",
            "203.0.113.4:0",
            "[::]:0",
            "[2001:db8::1]:0",
        ] {
            let parsed: std::net::SocketAddr = address.parse().unwrap();
            let error = crate::serve::Lan::bind(parsed).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput, "{address}");
            assert!(
                error.to_string().contains("§34.2"),
                "the refusal cites the rule: {error}"
            );
        }
    }

    #[test]
    fn the_console_binds_loopback_and_the_private_ranges_a_robot_actually_lives_on() {
        let parsed: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        assert!(crate::serve::Lan::bind(parsed).is_ok());

        // IPv6 loopback, on a machine that has an IPv6 stack. Many containers do not, and a bind
        // that fails for want of a stack is not the same fact as a bind this policy refused — so
        // the two are told apart rather than the case being skipped. What is asserted either way is
        // that `[::1]` is never refused *by us*: `InvalidInput` citing §34.2 is the refusal, and it
        // must not appear here.
        let parsed: std::net::SocketAddr = "[::1]:0".parse().unwrap();
        match crate::serve::Lan::bind(parsed) {
            Ok(_) => {}
            Err(error) => assert_ne!(
                error.kind(),
                std::io::ErrorKind::InvalidInput,
                "the IPv6 loopback was refused by policy rather than by the machine: {error}"
            ),
        }
        for ip in ["169.254.1.2", "10.0.0.4", "192.168.1.9", "172.16.0.1"] {
            assert!(
                crate::serve::is_link_local(ip.parse().unwrap()),
                "{ip} should be bindable"
            );
        }
        for ip in ["8.8.8.8", "203.0.113.4", "172.32.0.1"] {
            assert!(
                !crate::serve::is_link_local(ip.parse().unwrap()),
                "{ip} should not be"
            );
        }
    }

    /// The whole path: a real Unix socket, a real HTTP request over it, a real response.
    #[test]
    fn a_request_over_the_local_socket_is_answered() {
        let mut fixture = Fixture::new("socket");
        let path = fixture.root.path().join("run/skynetd.sock");
        let local = crate::serve::Local::bind(&path).unwrap();

        let client = std::thread::spawn({
            let path = path.clone();
            move || {
                let mut stream = std::os::unix::net::UnixStream::connect(&path).unwrap();
                stream
                    .write_all(b"GET /status HTTP/1.1\r\nHost: robot\r\n\r\n")
                    .unwrap();
                let mut response = Vec::new();
                stream.read_to_end(&mut response).unwrap();
                response
            }
        });

        let credentials = fixture.credentials.clone();
        let sink = crate::connectivity::Sink::describe(&crate::connectivity::Unpaired);
        let obstacle = crate::connectivity::Sink::obstacle(&crate::connectivity::Unpaired);
        let mut context = Context {
            daemon: &mut fixture.daemon,
            credentials: &credentials,
            now_ms: 1_000,
            started_ms: 0,
            // The suite exercises the API without a recorder: the routes that need one
            // answer 503, and the rest do not care.
            blackbox: None,
            mono_ns: 0,
            // The sink a robot actually has today, and the reason it has nothing else.
            outbound_sink: &sink,
            outbound_obstacle: obstacle.as_deref(),
        };
        let (stream, _) = local.listener().accept().unwrap();
        crate::serve::serve_one(stream, &mut context);

        let response = String::from_utf8(client.join().unwrap()).unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(response.contains("\"stop_available\":true"), "{response}");
    }

    /// The socket is a control channel for a machine that can move.
    #[test]
    fn the_socket_is_not_world_writable() {
        use std::os::unix::fs::PermissionsExt;
        let root = TempRoot::new("socket-mode");
        let path = root.path().join("run/skynetd.sock");
        let _local = crate::serve::Local::bind(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, crate::serve::SOCKET_MODE);
    }

    /// A `skynetd` that crashed leaves the socket file behind, and the next start must not fail on
    /// `EADDRINUSE` — a supervisor that cannot restart itself is worse than no supervisor.
    #[test]
    fn a_socket_left_behind_by_a_crash_does_not_stop_the_next_start() {
        let root = TempRoot::new("socket-stale");
        let path = root.path().join("run/skynetd.sock");
        let first = crate::serve::Local::bind(&path).unwrap();
        drop(first);
        assert!(path.exists(), "the file outlives the listener");
        assert!(crate::serve::Local::bind(&path).is_ok());
    }
}

// ── The wiring nobody would notice was wrong ────────────────────────────────

#[test]
fn every_action_kind_has_a_name_in_a_report() {
    use skynetd::reconcile::ActionKind as K;
    let one = "a".to_string();
    let kinds = [
        K::StageOsImage {
            version: one.clone(),
        },
        K::ApplyOsImage {
            version: one.clone(),
        },
        K::FetchPackage {
            id: one.clone(),
            digest: one.clone(),
        },
        K::ActivatePackage {
            id: one.clone(),
            digest: one.clone(),
        },
        K::RemovePackage { id: one.clone() },
        K::ApplyProfile {
            name: one.clone(),
            digest: one.clone(),
        },
        K::GrantCapability {
            id: one.clone(),
            token: one.clone(),
        },
        K::RevokeCapability {
            id: one.clone(),
            token: one.clone(),
        },
        K::StartPackage { id: one.clone() },
        K::StopPackage { id: one.clone() },
        K::ReleaseLease {
            id: one.clone(),
            token: one.clone(),
        },
    ];
    let mut seen = BTreeMap::new();
    for kind in &kinds {
        let name = api::action_name(kind);
        assert!(!name.is_empty());
        assert!(seen.insert(name, ()).is_none(), "`{name}` is used twice");
    }
    assert_eq!(seen.len(), kinds.len());
}

#[test]
fn a_route_id_round_trips_through_the_colon_form() {
    let id = api::parse_route_id("openrobot:grasp").unwrap();
    assert_eq!(id.as_str(), "openrobot/grasp");
    assert!(api::parse_route_id("openrobot/grasp").is_none());
    assert!(api::parse_route_id("openrobot").is_none());
    assert!(api::parse_route_id("..:..").is_none());
}

// ── The black box, against a disk (§13, §28) ─────────────────────────────────

mod recorder {
    use super::*;

    use skynet_blackbox::record::{EventCode, Gap, GapReason, Shape};
    use skynet_blackbox::seal::Reason;

    use crate::blackbox::{supervisor_meta, Recorder, DEFAULT_BUDGET};

    /// Small enough that a test is not writing 64 MB to a temporary directory, and large enough to
    /// hold a window worth asserting about.
    const BUDGET: usize = 64 * 1024;

    fn events_only() -> Shape {
        Shape {
            joints: 0,
            channels: 0,
        }
    }

    fn open(root: &TempRoot, shape: Shape) -> Recorder {
        let layout = root.layout();
        Recorder::open(&layout, supervisor_meta(&layout), shape, BUDGET).expect("a recorder")
    }

    /// §56.4, end to end: the recording says which body it is of.
    ///
    /// Both directions matter. A robot nobody has told what it is records a **zero** embodiment
    /// digest, and that is the honest answer — the ring has to open on an unprovisioned machine,
    /// because §28 preallocates it precisely so that nothing can silence the recorder. A robot with
    /// a descriptor records that descriptor's digest, and it is the digest of the *canonical* form,
    /// so two robots handed the same body agree.
    ///
    /// A recording carrying an invented digest could be replayed against the wrong machine without
    /// anybody noticing, which is why this was zero for two sessions rather than filled in.
    #[test]
    fn a_sealed_recording_names_the_body_it_was_taken_from() {
        let root = TempRoot::new("bb-embodiment");
        let layout = root.layout();

        // Before: nobody has told this robot what it is.
        assert_eq!(supervisor_meta(&layout).embodiment, [0u8; 32]);
        assert!(!supervisor_meta(&layout).simulator);

        std::fs::create_dir_all(layout.state()).unwrap();
        std::fs::write(
            layout.embodiment(),
            r#"{"robot_slug":"bench-arm","base":"fixed","dof":1,
                "simulator":true,
                "joints":[{"name":"shoulder","type":"revolute","control_modes":["position"]}]}"#,
        )
        .unwrap();

        let meta = supervisor_meta(&layout);
        let expected = crate::embodiment::load(&layout)
            .known()
            .expect("the descriptor loads")
            .digest;
        assert_eq!(meta.embodiment, expected);
        assert_ne!(meta.embodiment, [0u8; 32]);

        // §56.2: a benchmark replayed from a simulated recording must never be mistaken for a field
        // one, and the header is where that is settled rather than somebody's memory.
        assert!(meta.simulator, "the body declares itself simulated");

        // The OS image digest is still zero, and that is not the same kind of gap: §10's
        // attestation is unwritten, so nothing has established which image is running.
        assert_eq!(meta.os_image, [0u8; 32]);

        // And it survives into a file somebody could replay.
        let mut recorder =
            Recorder::open(&layout, meta, events_only(), BUDGET).expect("a recorder");
        recorder
            .event(10, EventCode::RecordingStarted, "skynetd", "started")
            .unwrap();
        let (sealed, path) = recorder.seal(Reason::Operator, 20).expect("sealed");
        let hex: String = expected.iter().map(|b| format!("{b:02x}")).collect();
        assert!(
            String::from_utf8_lossy(&sealed.bytes).contains(&hex),
            "the sealed recording carries the embodiment digest"
        );
        // And the file on disk, not only the bytes in hand — the thing somebody would replay.
        let written = std::fs::read(&path).expect("the sealed file");
        assert!(String::from_utf8_lossy(&written).contains(&hex), "{path:?}");
    }

    #[test]
    fn the_ring_is_preallocated_to_its_whole_length() {
        let root = TempRoot::new("bb-prealloc");
        let recorder = open(&root, events_only());
        let bytes = std::fs::metadata(root.path().join("data").join("blackbox").join("ring"))
            .expect("the ring exists")
            .len();
        // §28: the bytes are claimed while there are bytes to claim. A sparse file would report the
        // same length and fail at the first write after the disk filled, which is the failure the
        // preallocation exists to prevent.
        assert_eq!(bytes, recorder.geometry().total_bytes() as u64);
        assert!(recorder.geometry().capacity > 1);
    }

    #[test]
    fn a_restart_continues_the_sequence() {
        let root = TempRoot::new("bb-restart");
        let mut recorder = open(&root, events_only());
        recorder
            .event(10, EventCode::RecordingStarted, "skynetd", "started")
            .unwrap();
        recorder
            .event(
                20,
                EventCode::PackageStarted,
                "openrobot/grasp",
                "start_package applied",
            )
            .unwrap();
        drop(recorder);

        // Nothing in memory survives a restart, and the next sequence has to come off the disk.
        let reopened = open(&root, events_only());
        assert_eq!(reopened.geometry().shape, events_only());
        let index = reopened.index(1.0);
        assert_eq!(index.get("next_sequence").unwrap().as_f64(), Some(3.0));
        let window = index.get("window").unwrap();
        assert_eq!(window.get("records").unwrap().as_f64(), Some(2.0));
        assert_eq!(window.get("first_sequence").unwrap().as_f64(), Some(1.0));
        assert_eq!(window.get("complete").unwrap().as_bool(), Some(true));
    }

    #[test]
    fn a_changed_shape_seals_the_old_recording_before_replacing_it() {
        let root = TempRoot::new("bb-reshape");
        let mut recorder = open(&root, events_only());
        recorder
            .event(10, EventCode::RecordingStarted, "skynetd", "started")
            .unwrap();
        drop(recorder);

        // A control loop arrives: the body now has joints, so a record is a different size and the
        // ring cannot hold both. The window that was there becomes a file rather than a casualty.
        let reshaped = open(
            &root,
            Shape {
                joints: 6,
                channels: 6,
            },
        );
        assert_eq!(reshaped.geometry().shape.joints, 6);
        assert_eq!(
            reshaped
                .index(1.0)
                .get("window")
                .unwrap()
                .get("records")
                .unwrap()
                .as_f64(),
            Some(0.0)
        );
        let sealed = reshaped.sealed();
        assert_eq!(sealed.len(), 1, "the previous window was not sealed");
        assert!(sealed[0].0.ends_with("-shutdown.mcap"), "{:?}", sealed[0].0);
    }

    #[test]
    fn the_index_says_when_it_holds_no_cycles() {
        // The honesty that matters most here. §26 gives the black box six channels; a supervisor
        // with no control loop can fill one, and a console that showed a healthy recording without
        // saying so would report a complete black box on a robot that has never recorded a cycle.
        let root = TempRoot::new("bb-honest");
        let recorder = open(&root, events_only());
        let index = recorder.index(1.0);
        let note = index
            .get("cycles_unavailable")
            .and_then(Json::as_str)
            .expect("the index does not say the cycle channels are empty");
        assert!(note.contains("no control loop"), "{note}");
        assert_eq!(
            index.get("window").unwrap().get("cycles").unwrap().as_f64(),
            Some(0.0)
        );
    }

    #[test]
    fn sealing_writes_a_file_and_names_its_digest() {
        let root = TempRoot::new("bb-seal");
        let mut recorder = open(&root, events_only());
        recorder
            .event(
                10,
                EventCode::Estop,
                "operator",
                "the button on the chassis",
            )
            .unwrap();
        let (sealed, path) = recorder.seal(Reason::Estop, 42).unwrap();

        assert!(path.exists());
        assert_eq!(std::fs::read(&path).unwrap(), sealed.bytes);
        assert_eq!(&sealed.bytes[..8], &skynet_blackbox::mcap::MAGIC);
        assert_eq!(sealed.digest, skynet_sha256::digest(&sealed.bytes));
        assert!(sealed.continuity.complete());
        // The name orders by the monotonic clock rather than the wall clock: §35.1 gives
        // CLOCK_REALTIME one job, and a robot whose clock was set between two incidents would
        // otherwise produce two files whose names imply the wrong order.
        assert_eq!(
            path.file_name().unwrap().to_string_lossy(),
            "00000000000000000042-estop.mcap"
        );
    }

    // ── §38.3, mirrored: the audit log onto the event channel ────────────────

    fn audit_of(root: &TempRoot) -> crate::audit::Audit {
        crate::audit::Audit::of(&root.layout())
    }

    fn append(root: &TempRoot, seq_label: &str, action: &str) {
        audit_of(root)
            .append("2026-08-30T09:00:00Z", "alice", action, seq_label, "", None)
            .unwrap();
    }

    fn sealed_text(recorder: &Recorder) -> String {
        let (sealed, _) = recorder.seal(Reason::Operator, 99).unwrap();
        String::from_utf8_lossy(&sealed.bytes).into_owned()
    }

    /// The decision this design turns on. A robot provisioned a year ago has an audit log with
    /// hundreds of entries and a ring that holds minutes; replaying the log would throw away the
    /// incident window to record a year of configuration history. So the first pass records where
    /// to read it and jumps the cursor to the end.
    #[test]
    fn the_first_pass_records_a_baseline_rather_than_replaying_history() {
        let root = TempRoot::new("bb-mirror-baseline");
        let mut recorder = open(&root, events_only());
        for index in 0..40 {
            append(&root, &format!("key-{index}"), "trust.add");
        }

        let mirrored = crate::blackbox::mirror_audit(&root.layout(), &mut recorder, 10);
        assert!(mirrored.baseline);
        assert_eq!(mirrored.recorded, 0);

        let text = sealed_text(&recorder);
        assert!(
            text.contains("40 earlier configuration change(s)"),
            "{text}"
        );
        assert!(
            !text.contains("key-7"),
            "history was replayed into the ring"
        );

        // And the cursor is at the end, so nothing older ever arrives.
        assert_eq!(
            std::fs::read_to_string(root.layout().audit_cursor())
                .unwrap()
                .trim(),
            "40"
        );
    }

    /// A robot with no history at all writes no baseline event: there is nothing to point at, and
    /// an event saying "0 earlier changes" is noise in a ring that holds minutes.
    #[test]
    fn an_empty_audit_log_produces_no_baseline_event() {
        let root = TempRoot::new("bb-mirror-empty");
        let mut recorder = open(&root, events_only());

        let mirrored = crate::blackbox::mirror_audit(&root.layout(), &mut recorder, 10);
        assert!(mirrored.baseline);
        assert!(!sealed_text(&recorder).contains("earlier configuration change"));
    }

    #[test]
    fn changes_after_the_baseline_reach_the_event_channel() {
        let root = TempRoot::new("bb-mirror-after");
        let mut recorder = open(&root, events_only());
        crate::blackbox::mirror_audit(&root.layout(), &mut recorder, 10);

        append(&root, "acme-robotics", "trust.add");
        append(&root, "openrobot/grasp 1.0.0", "package.admit");

        let mirrored = crate::blackbox::mirror_audit(&root.layout(), &mut recorder, 20);
        assert_eq!(mirrored.recorded, 2);
        assert_eq!(mirrored.remaining, 0);

        let text = sealed_text(&recorder);
        // The action decides the code: a configuration change and an admission verdict are
        // different questions, and §41.1's vocabulary is closed so neither invents a third.
        assert!(text.contains("\"event\":\"config_changed\""), "{text}");
        assert!(text.contains("\"event\":\"admission\""), "{text}");
        assert!(text.contains("acme-robotics"), "{text}");
    }

    /// Idempotent: the cursor means a second pass over the same log records nothing again. A ring
    /// re-filled with the same three changes on every reconcile would hold ten seconds of history.
    #[test]
    fn a_second_pass_over_the_same_log_records_nothing() {
        let root = TempRoot::new("bb-mirror-idempotent");
        let mut recorder = open(&root, events_only());
        crate::blackbox::mirror_audit(&root.layout(), &mut recorder, 10);
        append(&root, "acme-robotics", "trust.add");

        assert_eq!(
            crate::blackbox::mirror_audit(&root.layout(), &mut recorder, 20).recorded,
            1
        );
        assert_eq!(
            crate::blackbox::mirror_audit(&root.layout(), &mut recorder, 30).recorded,
            0
        );
    }

    /// A provisioning script that trusts six keys between two reconciles must not be able to wrap
    /// the recording. The cap drains over a few passes and says how many are still waiting.
    #[test]
    fn a_burst_is_capped_per_pass_and_the_rest_still_arrive() {
        let root = TempRoot::new("bb-mirror-burst");
        let mut recorder = open(&root, events_only());
        crate::blackbox::mirror_audit(&root.layout(), &mut recorder, 10);

        for index in 0..20 {
            append(&root, &format!("key-{index}"), "trust.add");
        }

        let first = crate::blackbox::mirror_audit(&root.layout(), &mut recorder, 20);
        assert_eq!(first.recorded, 16);
        assert_eq!(first.remaining, 4);

        let second = crate::blackbox::mirror_audit(&root.layout(), &mut recorder, 30);
        assert_eq!(second.recorded, 4);
        assert_eq!(second.remaining, 0);
    }

    #[test]
    fn a_gap_survives_into_the_sealed_file() {
        let root = TempRoot::new("bb-gap");
        let mut recorder = open(&root, events_only());
        recorder
            .gap(
                10,
                Gap {
                    dropped: 12,
                    first_tick: 900,
                    reason: GapReason::WriterBehind,
                },
            )
            .unwrap();
        let (sealed, _) = recorder.seal(Reason::Operator, 20).unwrap();
        assert_eq!(sealed.continuity.dropped, 12);
        assert!(!sealed.continuity.complete());
        let text = String::from_utf8_lossy(&sealed.bytes);
        assert!(text.contains("\"event\":\"gap\""));
    }

    #[test]
    fn a_download_name_cannot_leave_the_directory() {
        // The route hands this a caller-supplied segment. A join without a check is how a download
        // becomes a way to read anything on the disk.
        let root = TempRoot::new("bb-traversal");
        let recorder = open(&root, events_only());
        recorder.seal(Reason::Operator, 1).unwrap();
        for name in [
            "../ring",
            "../../etc/shadow",
            "..\\ring",
            "ring",
            "sealed.mcap/../../ring",
        ] {
            assert!(recorder.read_sealed(name).is_none(), "{name} was readable");
        }
        let real = recorder.sealed();
        assert_eq!(real.len(), 1);
        assert!(recorder.read_sealed(&real[0].0).is_some());
    }

    #[test]
    fn the_default_budget_buys_a_window_the_geometry_can_state() {
        // §13 says "roughly an hour". An hour is not a size, and this is the conversion: at the
        // default budget a supervisor recording only events holds a very long window, and the same
        // ring at a humanoid's shape and rate holds a short one. Both numbers come out of the same
        // function rather than out of a constant named for an aspiration.
        let root = TempRoot::new("bb-window");
        let layout = root.layout();
        let recorder = Recorder::open(
            &layout,
            supervisor_meta(&layout),
            events_only(),
            DEFAULT_BUDGET,
        )
        .unwrap();
        let geometry = recorder.geometry();
        assert!(geometry.seconds_covered(1.0) > 3600.0);
        assert!(geometry.seconds_covered(1000.0) < 3600.0);
    }
}

// ── The black box, through the API (§39.1) ───────────────────────────────────

#[test]
fn the_blackbox_routes_need_the_roles_39_1_gives_them() {
    // §39.1: observer reads "the black box index"; operator may "seal and download the black box".
    // The index is a count and a window. The file is every command the robot was given.
    assert_eq!(requirement("GET", &["blackbox"]).0, Role::Observer);
    assert_eq!(requirement("POST", &["blackbox", "seal"]).0, Role::Operator);
    assert_eq!(
        requirement("GET", &["blackbox", "0001-estop.mcap"]).0,
        Role::Operator
    );
}

#[test]
fn a_robot_with_no_recorder_says_so_rather_than_pretending() {
    let mut fixture = Fixture::new("bb-absent");
    fixture.provision();
    let (status, body) = fixture.request("GET", "/blackbox", Some("admin-token"), "");
    assert_eq!(status, 503);
    // `error` is the status reason; `detail` is the sentence. A console shows the second.
    let detail = body
        .get("detail")
        .and_then(Json::as_str)
        .unwrap_or_default();
    assert!(detail.contains("no black box"), "{detail}");
}

// ── §54.4's lifecycle, driven ────────────────────────────────────────────────
//
// The contract's own suite proves `advance` answers correctly for a given `Progress`. These prove
// the half nothing could test until there was a loop: that the runtime *assembles* the right
// progress, performs each effect exactly once, and stops.

mod skills {
    use super::*;
    use crate::skill::{Cycle, Effect, Request, Runtime};
    use skynet_contracts::live::LiveInputs;
    use skynet_contracts::skill::{Clause, ClauseOrigin, SkillOutcome, SkillPhase};
    use skynetd::lease::LeaseTable;
    use std::collections::BTreeSet;

    /// A skill manifest with the fields §54 reads.
    fn skill_manifest(id: &str, seconds: Option<f64>, tokens: &[&str]) -> Json {
        let mut m = manifest(id, "1.0.0", "skill");
        if let Some(seconds) = seconds {
            m.set("expected_duration_seconds", Json::Num(seconds));
        }
        if !tokens.is_empty() {
            m.set(
                "required_capabilities",
                Json::Arr(tokens.iter().map(|t| Json::from(*t)).collect()),
            );
        }
        m
    }

    fn clause(statement: &str, predicate: Option<&str>, hold: bool) -> Clause {
        Clause {
            statement: statement.to_string(),
            predicate: predicate.map(|text| Json::parse(text).expect("the predicate parses")),
            hold,
            origin: ClauseOrigin::Skill,
        }
    }

    fn with_clauses(manifest: &mut Json, key: &str, clauses: &[(&str, Option<&str>, bool)]) {
        let mut list = Json::arr();
        for (statement, predicate, hold) in clauses {
            let mut entry = Json::obj();
            entry.set("statement", Json::from(*statement));
            entry.set("hold", Json::Bool(*hold));
            if let Some(text) = predicate {
                entry.set(
                    "predicate",
                    Json::parse(text).expect("the predicate parses"),
                );
            }
            list.push(entry);
        }
        manifest.set(key, list);
    }

    /// The world, arranged.
    struct World {
        leases: LeaseTable,
        running: BTreeSet<String>,
        grants: Vec<(String, String)>,
        inputs: LiveInputs,
        robot: OperatingState,
        now_ms: u64,
    }

    impl World {
        fn new() -> Self {
            Self {
                leases: LeaseTable::new(),
                running: BTreeSet::new(),
                grants: Vec::new(),
                inputs: LiveInputs {
                    battery_percent: Some(80.0),
                    operating_state: Some(OperatingState::Running),
                    ..LiveInputs::default()
                },
                robot: OperatingState::Running,
                now_ms: 0,
            }
        }

        fn step(&mut self, runtime: &mut Runtime, by_ms: u64) -> Vec<crate::skill::Report> {
            self.now_ms += by_ms;
            let mut cycle = Cycle {
                now_ms: self.now_ms,
                inputs: &self.inputs,
                robot: self.robot,
                guard_tripped: false,
                guards_observable: false,
                running: &self.running,
                leases: &mut self.leases,
                grants: &self.grants,
            };
            let reports = runtime.advance_all(&mut cycle);
            // The launcher, modelled: a `Start` makes the unit appear and a `Stop` removes it.
            for report in &reports {
                for effect in &report.effects {
                    match effect {
                        Effect::Start { package } => {
                            self.running.insert(package.clone());
                        }
                        Effect::Stop { package } => {
                            self.running.remove(package);
                        }
                        Effect::ReleaseLeases { holder } => {
                            self.leases.release_all(holder);
                        }
                        Effect::Episode { .. } => {}
                    }
                }
            }
            reports
        }
    }

    fn invoke(runtime: &mut Runtime, manifest: &Json, operator: &[Clause]) -> String {
        runtime
            .execute(
                &Request {
                    package: manifest
                        .get("id")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    arguments: Json::obj(),
                    parent: None,
                    overrun_factor: None,
                },
                manifest,
                "sha256:test",
                operator,
                0,
            )
            .expect("the invocation is accepted")
    }

    fn phases(reports: &[crate::skill::Report]) -> Vec<&'static str> {
        reports
            .iter()
            .map(|report| report.transition.to.as_str())
            .collect()
    }

    /// The whole ladder, one pass at a time, on a skill that does everything right.
    #[test]
    fn an_invocation_walks_the_diagram_and_ends_with_one_episode() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let mut m = skill_manifest("acme/pick", Some(10.0), &["arm.left"]);
        with_clauses(
            &mut m,
            "preconditions",
            &[(
                "the battery is above 20%",
                Some(r#"{"op":"gte","path":"robot.state.battery_percent","value":20}"#),
                true,
            )],
        );
        with_clauses(
            &mut m,
            "success_criteria",
            &[(
                "the battery is still above 10%",
                Some(r#"{"op":"gte","path":"robot.state.battery_percent","value":10}"#),
                false,
            )],
        );
        world
            .grants
            .push(("acme/pick".to_string(), "arm.left".to_string()));

        let id = invoke(&mut runtime, &m, &[]);

        assert_eq!(phases(&world.step(&mut runtime, 1)), ["preconditions"]);
        assert_eq!(phases(&world.step(&mut runtime, 1)), ["acquiring"]);

        // Entering `running` starts the unit, and nothing else.
        let reports = world.step(&mut runtime, 1);
        assert_eq!(phases(&reports), ["running"]);
        assert_eq!(
            reports[0].effects,
            [Effect::Start {
                package: "acme/pick".to_string()
            }]
        );
        assert_eq!(
            world.leases.holder("arm.left"),
            Some("acme/pick"),
            "§25.2's lease is taken by the package, which is what an operator sees"
        );

        // Running is a fixed point while the skill's own code runs.
        assert_eq!(phases(&world.step(&mut runtime, 100)), ["running"]);
        assert!(world.step(&mut runtime, 100)[0].effects.is_empty());

        // The unit exits: the skill returned.
        world.running.remove("acme/pick");
        assert_eq!(phases(&world.step(&mut runtime, 1)), ["verifying"]);

        let reports = world.step(&mut runtime, 1);
        assert_eq!(phases(&reports), ["terminal"]);
        let episodes: Vec<&Effect> = reports[0]
            .effects
            .iter()
            .filter(|effect| matches!(effect, Effect::Episode { .. }))
            .collect();
        assert_eq!(episodes.len(), 1, "§54.4: one episode per invocation");
        assert!(reports[0].effects.contains(&Effect::ReleaseLeases {
            holder: "acme/pick".to_string()
        }));
        assert_eq!(world.leases.holder("arm.left"), None);

        let invocation = runtime.get(&id).expect("the invocation is kept");
        assert_eq!(invocation.outcome, Some(SkillOutcome::Success));
        assert!(invocation.terminal());

        // And a second pass over a terminal invocation does nothing at all — which is the failure
        // that would otherwise fill a dataset with duplicate rows nobody is watching for.
        assert!(world.step(&mut runtime, 1).is_empty());
    }

    /// §54.6's `skill.preconditions.enforced`, through the runtime rather than through the contract.
    #[test]
    fn a_false_precondition_refuses_before_any_lease_is_taken() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        world.inputs.battery_percent = Some(4.0);
        let mut m = skill_manifest("acme/pick", Some(10.0), &["arm.left"]);
        with_clauses(
            &mut m,
            "preconditions",
            &[(
                "the battery is above 20%",
                Some(r#"{"op":"gte","path":"robot.state.battery_percent","value":20}"#),
                false,
            )],
        );
        world
            .grants
            .push(("acme/pick".to_string(), "arm.left".to_string()));

        let id = invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        let reports = world.step(&mut runtime, 1);

        assert_eq!(phases(&reports), ["terminal"]);
        assert_eq!(
            runtime.get(&id).unwrap().outcome,
            Some(SkillOutcome::Aborted),
            "a skill never allowed to start did not fail at its task"
        );
        assert_eq!(
            world.leases.holder("arm.left"),
            None,
            "nothing was acquired, so nothing is held"
        );
        assert!(
            !reports[0]
                .effects
                .iter()
                .any(|e| matches!(e, Effect::Start { .. })),
            "and nothing was started"
        );
        // §54.4 says one episode per invocation and a refused invocation is an invocation. The
        // reason is what tells a consumer this row is not training data.
        let Effect::Episode { body } = reports[0]
            .effects
            .iter()
            .find(|e| matches!(e, Effect::Episode { .. }))
            .expect("an episode")
        else {
            unreachable!()
        };
        assert_eq!(
            body.get("reason").and_then(Json::as_str),
            Some("preconditions_refused")
        );
        assert!(
            body.get("duration_ms").is_none(),
            "nothing ran, so there is no duration to report"
        );
        assert!(
            body.get("intervention_count").is_none(),
            "and nothing counts interventions on this robot, so the count is absent, not zero"
        );
    }

    /// A precondition naming a fact this robot does not have. §54.2's first rule, end to end.
    #[test]
    fn a_precondition_the_robot_cannot_evaluate_refuses_it() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        // No joints: §33.1 keeps the daemon off the actuator path, so this is the ordinary case.
        let mut m = skill_manifest("acme/pick", Some(10.0), &[]);
        with_clauses(
            &mut m,
            "preconditions",
            &[(
                "the elbow is near zero",
                Some(r#"{"op":"lte","path":"robot.state.joints.elbow","value":0.1}"#),
                false,
            )],
        );

        let id = invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        let invocation = runtime.get(&id).unwrap();
        assert!(invocation.terminal());
        assert!(
            invocation.detail.contains("the elbow is near zero"),
            "the refusal carries the publisher's own sentence: {}",
            invocation.detail
        );
    }

    /// §54.6's `skill.preconditions.rechecked`. The battery starts fine and falls mid-run.
    #[test]
    fn a_hold_precondition_falsified_mid_run_stops_the_skill() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        world.inputs.battery_percent = Some(62.0);
        let mut m = skill_manifest("acme/pick", Some(10.0), &[]);
        with_clauses(
            &mut m,
            "preconditions",
            &[(
                "the battery is above 20%",
                Some(r#"{"op":"gte","path":"robot.state.battery_percent","value":20}"#),
                true,
            )],
        );

        let id = invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        assert_eq!(phases(&world.step(&mut runtime, 1)), ["running"]);

        world.inputs.battery_percent = Some(11.0);
        let reports = world.step(&mut runtime, 1);
        assert_eq!(phases(&reports), ["terminal"]);
        assert_eq!(reports[0].transition.reason.as_str(), "hold_lost");
        assert_eq!(
            runtime.get(&id).unwrap().outcome,
            Some(SkillOutcome::Aborted)
        );
        assert!(
            reports[0].effects.contains(&Effect::Stop {
                package: "acme/pick".to_string()
            }),
            "the skill it started is the skill it stops"
        );
    }

    /// §54.6's `skill.timeout.enforced`.
    #[test]
    fn an_overrunning_skill_is_stopped_and_recorded_aborted() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let m = skill_manifest("acme/slow", Some(1.0), &[]);
        let id = invoke(&mut runtime, &m, &[]);

        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        // One second declared, doubled by the default overrun factor: 2000 ms.
        assert_eq!(phases(&world.step(&mut runtime, 1_999)), ["running"]);
        let reports = world.step(&mut runtime, 2);
        assert_eq!(phases(&reports), ["terminal"]);
        assert_eq!(reports[0].transition.reason.as_str(), "timeout");
        assert_eq!(
            runtime.get(&id).unwrap().outcome,
            Some(SkillOutcome::Aborted)
        );
    }

    /// §54.3, and the reason it is the OS that judges: the skill returning is not the skill winning.
    #[test]
    fn a_skill_that_returns_against_a_false_criterion_is_recorded_failure() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let mut m = skill_manifest("acme/pick", Some(10.0), &[]);
        with_clauses(
            &mut m,
            "success_criteria",
            &[(
                "the battery is above 90%",
                Some(r#"{"op":"gte","path":"robot.state.battery_percent","value":90}"#),
                false,
            )],
        );
        let id = invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        world.running.remove("acme/pick");
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        assert_eq!(
            runtime.get(&id).unwrap().outcome,
            Some(SkillOutcome::Failure)
        );
    }

    /// And a criterion nothing can read is `unlabeled`, never a pass.
    #[test]
    fn an_unenforceable_criterion_is_unlabeled_rather_than_a_pass() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let mut m = skill_manifest("acme/pick", Some(10.0), &[]);
        with_clauses(
            &mut m,
            "success_criteria",
            &[("the cup is upright", None, false)],
        );
        let id = invoke(&mut runtime, &m, &[]);
        for _ in 0..3 {
            world.step(&mut runtime, 1);
        }
        world.running.remove("acme/pick");
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        assert_eq!(
            runtime.get(&id).unwrap().outcome,
            Some(SkillOutcome::Unlabeled)
        );
    }

    /// §25.2 through §54.4's `acquiring` stage: denied if held.
    #[test]
    fn a_capability_another_package_holds_denies_the_skill() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        world
            .leases
            .acquire("other/policy", "arm.left", true, false);
        let m = skill_manifest("acme/pick", Some(10.0), &["arm.left"]);
        world
            .grants
            .push(("acme/pick".to_string(), "arm.left".to_string()));

        let id = invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        let reports = world.step(&mut runtime, 1);
        assert_eq!(phases(&reports), ["terminal"]);
        assert_eq!(reports[0].transition.reason.as_str(), "lease_denied");
        assert!(
            reports[0].transition.detail.contains("other/policy"),
            "the refusal names who has the arm: {}",
            reports[0].transition.detail
        );
        assert_eq!(
            world.leases.holder("arm.left"),
            Some("other/policy"),
            "and the holder keeps it"
        );
        assert_eq!(
            runtime.get(&id).unwrap().outcome,
            Some(SkillOutcome::Aborted)
        );
    }

    /// A token the operator never granted is refused before the arm is touched, and the two
    /// refusals read differently — §25.2's grant-before-holder rule, surviving the runtime.
    #[test]
    fn a_capability_nobody_granted_is_a_different_refusal_from_one_somebody_holds() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let m = skill_manifest("acme/pick", Some(10.0), &["arm.left"]);
        let id = invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        let reports = world.step(&mut runtime, 1);
        assert_eq!(reports[0].transition.reason.as_str(), "lease_denied");
        assert!(
            reports[0].transition.detail.contains("never granted"),
            "nobody has the arm; this package may not ask for it, and the fix is a grant: {}",
            reports[0].transition.detail
        );
        assert!(
            !reports[0].transition.detail.contains("is held by"),
            "and it must not send an operator looking for a package that does not exist: {}",
            reports[0].transition.detail
        );
        assert!(runtime.get(&id).unwrap().terminal());
    }

    /// §54.5's acquisition rollback, arriving through the front door. A skill that got two of its
    /// three tokens would hold two joints it cannot use and refuse them to everything else.
    #[test]
    fn a_partial_acquisition_rolls_back_what_this_call_took() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        world
            .leases
            .acquire("other/policy", "arm.right", true, false);
        let m = skill_manifest("acme/pick", Some(10.0), &["arm.left", "arm.right"]);
        for token in ["arm.left", "arm.right"] {
            world
                .grants
                .push(("acme/pick".to_string(), token.to_string()));
        }
        invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        assert_eq!(
            world.leases.holder("arm.left"),
            None,
            "the token this call did take is given back, or the next package to want it gets no \
             reason on any screen"
        );
    }

    /// §54.2's third rule. The site adds a clause the publisher never wrote, and it refuses.
    #[test]
    fn an_operator_may_add_a_precondition_the_publisher_never_imagined() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        world.inputs.env = Some(Json::parse(r#"{"door_closed":false}"#).unwrap());
        let m = skill_manifest("acme/pick", Some(10.0), &[]);
        let site = vec![clause(
            "the cell door is closed",
            Some(r#"{"op":"eq","path":"env.door_closed","value":true}"#),
            false,
        )];

        let id = invoke(&mut runtime, &m, &site);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        let invocation = runtime.get(&id).unwrap();
        assert!(invocation.terminal());
        assert!(
            invocation.detail.contains("the cell door is closed"),
            "{}",
            invocation.detail
        );
    }

    /// §27's machine reaching the lifecycle: a robot that is not commanding has no running skills.
    #[test]
    fn a_robot_that_stops_commanding_ends_the_skill() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let m = skill_manifest("acme/pick", Some(10.0), &[]);
        let id = invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        assert_eq!(phases(&world.step(&mut runtime, 1)), ["running"]);

        world.robot = OperatingState::Safe;
        let reports = world.step(&mut runtime, 1);
        assert_eq!(reports[0].transition.reason.as_str(), "robot_stopped");
        assert_eq!(
            runtime.get(&id).unwrap().outcome,
            Some(SkillOutcome::Aborted)
        );
    }

    /// `hold` is not an aborting state, and it is the exclusion worth a test: it is where §27 sits
    /// between a lease being granted and a policy commanding, so aborting on it would kill every
    /// skill in the moment it started.
    #[test]
    fn a_robot_holding_does_not_end_the_skill() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        world.robot = OperatingState::Hold;
        let m = skill_manifest("acme/pick", Some(10.0), &[]);
        let id = invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        assert_eq!(runtime.get(&id).unwrap().phase, SkillPhase::Running);
    }

    /// §54.5's first limit at runtime: a nested call runs under the root's leases.
    #[test]
    fn a_nested_call_does_not_acquire_and_is_bounded_by_its_root() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let root_manifest = skill_manifest("acme/root", Some(10.0), &["arm.left"]);
        world
            .grants
            .push(("acme/root".to_string(), "arm.left".to_string()));
        let root = invoke(&mut runtime, &root_manifest, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);

        // A child declaring nothing is still bounded, which is the composition failure §54.5 names.
        let child_manifest = skill_manifest("acme/child", None, &["arm.left"]);
        let child = runtime
            .execute(
                &Request {
                    package: "acme/child".to_string(),
                    arguments: Json::obj(),
                    parent: Some(root.clone()),
                    overrun_factor: None,
                },
                &child_manifest,
                "sha256:child",
                &[],
                world.now_ms,
            )
            .expect("a nested call is accepted");

        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        let nested = runtime.get(&child).unwrap();
        assert_eq!(nested.phase, SkillPhase::Running);
        assert_eq!(
            nested.to_json().get("leases").and_then(Json::as_str),
            Some("pending"),
            "§54.5: leases are held by the root invocation, not re-acquired per call"
        );
        assert!(
            nested
                .to_json()
                .get("deadline_ms")
                .and_then(Json::as_f64)
                .is_some(),
            "and an unbounded child under a bounded root is still bounded"
        );
    }

    /// §54.5's acyclicity, applied to the chain that is actually about to run.
    ///
    /// `a → b → a`, because the interesting ordering question is here: every ancestor of a nested
    /// call is live by definition, so a runtime that checked "already running" first would report
    /// every cycle as a busy package and never show the chain.
    #[test]
    fn a_call_that_would_re_enter_its_own_root_is_refused_with_the_chain_named() {
        let mut runtime = Runtime::new();
        let a = skill_manifest("acme/a", Some(10.0), &[]);
        let b = skill_manifest("acme/b", Some(10.0), &[]);

        let root = invoke(&mut runtime, &a, &[]);
        let middle = runtime
            .execute(
                &Request {
                    package: "acme/b".to_string(),
                    arguments: Json::obj(),
                    parent: Some(root),
                    overrun_factor: None,
                },
                &b,
                "sha256:b",
                &[],
                0,
            )
            .expect("one level down is fine");

        let refusal = runtime
            .execute(
                &Request {
                    package: "acme/a".to_string(),
                    arguments: Json::obj(),
                    parent: Some(middle),
                    overrun_factor: None,
                },
                &a,
                "sha256:a",
                &[],
                0,
            )
            .expect_err("a cycle is refused");
        assert_eq!(refusal.code, "cycle");
        assert!(
            refusal.detail.contains("acme/a → acme/b → acme/a"),
            "the chain is what an operator reads: {}",
            refusal.detail
        );
    }

    #[test]
    fn a_second_invocation_of_a_live_skill_is_refused() {
        let mut runtime = Runtime::new();
        let m = skill_manifest("acme/pick", Some(10.0), &[]);
        invoke(&mut runtime, &m, &[]);
        let refusal = runtime
            .execute(
                &Request {
                    package: "acme/pick".to_string(),
                    arguments: Json::obj(),
                    parent: None,
                    overrun_factor: None,
                },
                &m,
                "sha256:test",
                &[],
                0,
            )
            .expect_err("one live invocation per package");
        assert_eq!(refusal.code, "already_running");
    }

    /// §54.1's distinction, enforced at the door. A policy is not something you invoke.
    #[test]
    fn a_policy_cannot_be_invoked_as_a_skill() {
        let mut runtime = Runtime::new();
        let policy = manifest("acme/drive", "1.0.0", "policy");
        let refusal = runtime
            .execute(
                &Request {
                    package: "acme/drive".to_string(),
                    arguments: Json::obj(),
                    parent: None,
                    overrun_factor: None,
                },
                &policy,
                "sha256:test",
                &[],
                0,
            )
            .expect_err("a policy is not invoked");
        assert_eq!(refusal.code, "not_a_skill");
        assert!(
            refusal.detail.contains("POST /packages/acme:drive/start"),
            "§64: the refusal names the fix, spelled so the route actually answers it — {}",
            refusal.detail
        );
    }

    /// A stop takes every live invocation with it, and emits their episodes rather than losing
    /// the record of what was running when somebody pressed the button.
    #[test]
    fn stopping_the_robot_ends_every_invocation_with_an_episode() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let m = skill_manifest("acme/pick", Some(10.0), &[]);
        invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);

        let reports = runtime.stop_all(world.now_ms);
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].transition.outcome, Some(SkillOutcome::Aborted));
        assert_eq!(
            reports[0]
                .effects
                .iter()
                .filter(|e| matches!(e, Effect::Episode { .. }))
                .count(),
            1
        );
        assert!(runtime.stop_all(world.now_ms).is_empty());
    }
}

// ── The skill routes ─────────────────────────────────────────────────────────

/// Install an activated skill package, so `POST /skills` has a manifest to read.
///
/// POSIX only, like every other test that activates: §37.1 requires the atomic symlink swap and
/// `Store::activate` refuses to fake one.
#[cfg(unix)]
fn install_skill(fixture: &Fixture, id: &str, extra: impl Fn(&mut Json)) -> String {
    let source = fixture
        .root
        .path()
        .join(format!("src-{}", id.replace('/', "-")));
    let mut m = manifest(id, "1.0.0", "skill");
    m.set("expected_duration_seconds", Json::Num(5.0));
    extra(&mut m);
    let digest = write_package(&source, &m, &[("bin/run", "#!/bin/sh\n")]);
    let parsed = PackageId::parse(id).unwrap();
    fixture
        .daemon
        .store
        .stage(&parsed, "skill", "1.0.0", &digest, &source)
        .unwrap();
    fixture.daemon.store.activate(&parsed, &digest).unwrap();
    digest
}

#[test]
#[cfg(unix)]
fn invoking_a_skill_is_accepted_and_not_started() {
    let mut fixture = Fixture::new("skill-execute");
    fixture.provision();
    install_skill(&fixture, "acme/pick", |_| {});

    let (status, body) = fixture.request(
        "POST",
        "/skills",
        Some("admin-token"),
        r#"{"skill":"acme/pick","arguments":{"target":"cup"}}"#,
    );
    assert_eq!(status, 202, "accepted, not created: {body:?}");
    assert_eq!(body.get("phase").and_then(Json::as_str), Some("idle"));

    let (status, list) = fixture.request("GET", "/skills", Some("observer-token"), "");
    assert_eq!(status, 200);
    let rows = list.get("invocations").and_then(Json::as_arr).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("package").and_then(Json::as_str),
        Some("acme/pick")
    );
    assert_eq!(rows[0].get("phase").and_then(Json::as_str), Some("idle"));
}

#[test]
#[cfg(unix)]
fn invoking_a_skill_needs_an_operator_and_reading_one_does_not() {
    assert_eq!(requirement("GET", &["skills"]).0, Role::Observer);
    assert_eq!(requirement("GET", &["skills", "inv-1"]).0, Role::Observer);
    assert_eq!(requirement("POST", &["skills"]).0, Role::Operator);
    assert_eq!(
        requirement("POST", &["skills", "inv-1", "stop"]).0,
        Role::Operator
    );

    let mut fixture = Fixture::new("skill-roles");
    fixture.provision();
    install_skill(&fixture, "acme/pick", |_| {});
    let (status, _) = fixture.request(
        "POST",
        "/skills",
        Some("observer-token"),
        r#"{"skill":"acme/pick"}"#,
    );
    assert_eq!(
        status, 403,
        "an observer may watch a skill and not start one"
    );
}

#[test]
fn invoking_a_package_that_is_not_installed_says_so() {
    let mut fixture = Fixture::new("skill-missing");
    fixture.provision();
    let (status, body) = fixture.request(
        "POST",
        "/skills",
        Some("admin-token"),
        r#"{"skill":"acme/absent"}"#,
    );
    assert_eq!(status, 404);
    let detail = body
        .get("detail")
        .and_then(Json::as_str)
        .unwrap_or_default();
    assert!(detail.contains("no activated version"), "{detail}");
}

/// The cell reaching a precondition, which is the second contract this session gave a caller.
#[test]
fn a_signed_profile_carries_the_cell_into_the_precondition_context() {
    let fixture = Fixture::new("skill-cell");
    let layout = fixture.daemon.store.layout().clone();
    std::fs::create_dir_all(layout.profiles()).unwrap();
    std::fs::write(
        layout.active_profile(),
        r#"{"name":"site-a","digest":"sha256:x","limits":{},"cell":{"name":"bench","facts":{"door_closed":true},"fixtures":[{"name":"jig"}]}}"#,
    )
    .unwrap();

    let inputs = fixture.daemon.live_inputs(None);
    let built = skynet_contracts::live::live_context(&inputs);
    let env = built.context.get("env").expect("the cell reaches env.*");
    assert_eq!(env.get("cell").and_then(Json::as_str), Some("bench"));
    assert_eq!(env.get("door_closed"), Some(&Json::Bool(true)));
    assert_eq!(
        env.get("fixtures")
            .and_then(Json::as_arr)
            .map(<[Json]>::len),
        Some(1)
    );
}

/// And what the daemon will *not* claim. §33.1 keeps it off the actuator path, so five of §54.2's
/// branches are absent — which refuses the preconditions that named them, with the path named.
#[test]
fn the_daemon_claims_nothing_it_cannot_observe() {
    let fixture = Fixture::new("skill-context-honesty");
    let inputs = fixture.daemon.live_inputs(None);
    assert!(inputs.joints.is_empty());
    assert!(inputs.battery_percent.is_none());
    assert!(inputs.gripper.is_none());
    assert!(inputs.base_pose.is_none());
    assert!(
        inputs.guards_active.is_none(),
        "there is no kernel channel, so nobody has asked which guards are running — and an empty \
         list would be this daemon answering for one"
    );
    assert!(
        inputs.tier.is_none(),
        "nothing on this robot decides its tier: §15 awards one from conformance evidence and §11 \
         delivers it in desired state, and neither path is written"
    );
    assert_eq!(inputs.operating_state, Some(OperatingState::Post));
}

// ── §12, on a machine ────────────────────────────────────────────────────────
//
// The contract's own suite proves `admit_bundle` and `confirm_boot` answer correctly for given
// inputs. These prove the half a pure function cannot: that the documents are read and *verified*,
// that an unsigned bench pass is not a bench pass, and that §12's unattended rollback can be driven
// end to end without a bootloader — which is what `ota.rollback.works` asks for and what a rollback
// nobody can exercise until a fleet needs it does not have.

mod ota {
    use super::*;
    use crate::ota::{
        assess, confirm, read_bench_pass, read_pending, BootHealth, RecordingUpdater,
    };
    use crate::trust::{bench_message, release_message, TrustStore};
    use skynet_contracts::ota::BootDecision;

    fn write_signed(fixture: &Fixture, path: &std::path::Path, document: &Json, domain: u8) {
        let canonical = document.to_canonical().unwrap();
        let digest =
            String::from_utf8_lossy(&skynet_sha256::hex_bytes(canonical.as_bytes())).into_owned();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, canonical.as_bytes()).unwrap();

        let key = driver_key(domain);
        let message = if domain == 7 {
            release_message(&digest)
        } else {
            bench_message(&digest)
        };
        let signature = hex_of(&key.sign(&message));
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        std::fs::write(
            path.parent().unwrap().join(format!("{stem}.sig.json")),
            format!(
                "{{\"key\":\"{}\",\"value\":\"{signature}\"}}",
                public_hex(&key)
            ),
        )
        .unwrap();
        let _ = fixture;
    }

    fn release_document(changes: &[&str]) -> Json {
        let mut document = Json::obj();
        document.set("version", Json::from("1.4.0"));
        document.set("channel", Json::from("stable"));
        document.set("digest", Json::from("sha256:aaaa"));
        document.set("compatible", Json::Arr(vec![Json::from("bench-arm")]));
        document.set(
            "changes",
            Json::Arr(changes.iter().map(|c| Json::from(*c)).collect()),
        );
        document
    }

    fn bench_document(passed: bool) -> Json {
        let mut document = Json::obj();
        document.set("image_digest", Json::from("sha256:aaaa"));
        document.set("tier", Json::from("host"));
        document.set("passed", Json::Bool(passed));
        document
    }

    /// A fixture with a release manifest signed by a key this robot trusts.
    fn with_release(label: &str, changes: &[&str]) -> Fixture {
        let mut fixture = Fixture::new(label);
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        write_trust(&fixture, "publisher", &public_hex(&driver_key(7)));
        write_signed(
            &fixture,
            &layout.release("1.4.0"),
            &release_document(changes),
            7,
        );
        fixture
    }

    fn assess_it(fixture: &Fixture) -> crate::ota::Assessment {
        let layout = fixture.daemon.store.layout();
        assess(
            layout,
            &TrustStore::load(layout),
            "1.4.0",
            "1.3.0",
            Some("bench-arm".to_string()),
            "stable",
        )
    }

    #[test]
    fn a_release_nobody_has_described_is_refused_rather_than_installed() {
        let mut fixture = Fixture::new("ota-no-manifest");
        fixture.provision();
        let assessment = assess_it(&fixture);
        assert!(!assessment.admitted());
        let because = assessment.because.unwrap_or_default();
        assert!(
            because.contains("nothing on this robot describes"),
            "{because}"
        );
        assert!(
            because.contains("Safety Kernel"),
            "the refusal says why a manifest is required rather than only that one is: {because}"
        );
    }

    #[test]
    fn an_ordinary_signed_release_is_admitted() {
        let fixture = with_release("ota-ordinary", &["userspace"]);
        let assessment = assess_it(&fixture);
        assert!(assessment.admitted(), "{:?}", assessment.verdict);
    }

    /// §12's third rule, through the documents rather than through the contract.
    #[test]
    fn a_kernel_change_is_refused_until_a_signed_bench_pass_covers_it() {
        let fixture = with_release("ota-gate", &["safety_kernel"]);
        let layout = fixture.daemon.store.layout().clone();

        let assessment = assess_it(&fixture);
        assert!(!assessment.admitted());
        assert_eq!(
            assessment
                .verdict
                .as_ref()
                .unwrap()
                .gated_components
                .as_slice(),
            ["safety_kernel"]
        );

        // The same bench pass, unsigned. Not a bench pass.
        let canonical = bench_document(true).to_canonical().unwrap();
        std::fs::create_dir_all(layout.bench_pass("sha256:aaaa").parent().unwrap()).unwrap();
        std::fs::write(layout.bench_pass("sha256:aaaa"), canonical.as_bytes()).unwrap();
        let assessment = assess_it(&fixture);
        assert!(
            !assessment.admitted(),
            "an unsigned bench pass is not a bench pass"
        );
        let note = assessment.bench_note.unwrap_or_default();
        assert!(note.contains("absent"), "{note}");
        assert!(
            note.contains("§38.1"),
            "and the refusal says which attacker this is: {note}"
        );

        // Signed by a key the robot trusts. Now it is one.
        write_signed(
            &fixture,
            &layout.bench_pass("sha256:aaaa"),
            &bench_document(true),
            8,
        );
        std::fs::write(
            layout.trust(),
            format!(
                "{{\"keys\":[{{\"label\":\"publisher\",\"key\":\"{}\"}},{{\"label\":\"bench\",\"key\":\"{}\"}}]}}",
                public_hex(&driver_key(7)),
                public_hex(&driver_key(8)),
            ),
        )
        .unwrap();
        let assessment = assess_it(&fixture);
        assert!(assessment.admitted(), "{:?}", assessment.verdict);
    }

    #[test]
    fn a_bench_pass_filed_under_other_bytes_is_refused_with_both_digests_named() {
        let fixture = with_release("ota-bench-mismatch", &["safety_kernel"]);
        let layout = fixture.daemon.store.layout().clone();
        let mut document = bench_document(true);
        document.set("image_digest", Json::from("sha256:bbbb"));
        std::fs::create_dir_all(layout.bench_pass("sha256:aaaa").parent().unwrap()).unwrap();
        std::fs::write(
            layout.bench_pass("sha256:aaaa"),
            document.to_canonical().unwrap().as_bytes(),
        )
        .unwrap();

        let (pass, note) = read_bench_pass(&layout, &TrustStore::load(&layout), "sha256:aaaa");
        assert!(pass.is_none());
        let note = note.unwrap_or_default();
        assert!(
            note.contains("sha256:aaaa") && note.contains("sha256:bbbb"),
            "{note}"
        );
    }

    /// The lossy flattening, from the direction it would be exploited.
    #[test]
    fn a_manifest_filed_under_another_version_is_refused() {
        let mut fixture = Fixture::new("ota-filed-wrong");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        write_trust(&fixture, "publisher", &public_hex(&driver_key(7)));
        // `1.4.0` and `1_4_0` flatten to one path, so a document declaring the other one is filed
        // where this robot looks for this one.
        let mut document = release_document(&["userspace"]);
        document.set("version", Json::from("1_4_0"));
        write_signed(&fixture, &layout.release("1.4.0"), &document, 7);

        let assessment = assess_it(&fixture);
        assert!(!assessment.admitted());
        let because = assessment.because.unwrap_or_default();
        assert!(because.contains("declares itself"), "{because}");
    }

    // ── §12's second rule, driven ────────────────────────────────────────────

    fn healthy() -> BootHealth {
        BootHealth {
            post_passed: Some(true),
            packages_healthy: Some(true),
            converged: Some(true),
        }
    }

    #[test]
    fn an_ordinary_boot_tells_the_bootloader_nothing() {
        let fixture = Fixture::new("ota-boot-ordinary");
        let layout = fixture.daemon.store.layout().clone();
        let mut updater = RecordingUpdater::default();
        let confirmation = confirm(&layout, &mut updater, "1.4.0", 1_000.0, "boot", &healthy());
        assert_eq!(confirmation.verdict.decision, BootDecision::NotPending);
        assert!(updater.calls.is_empty(), "{:?}", updater.calls);
    }

    #[test]
    fn a_healthy_first_boot_marks_the_slot_good_and_forgets_it() {
        let fixture = Fixture::new("ota-boot-good");
        let layout = fixture.daemon.store.layout().clone();
        crate::ota::mark_pending(&layout, "1.4.0", 600_000.0).unwrap();

        let mut updater = RecordingUpdater::default();
        let confirmation = confirm(&layout, &mut updater, "1.4.0", 42_000.0, "boot", &healthy());
        assert_eq!(confirmation.verdict.decision, BootDecision::Confirm);
        assert_eq!(updater.calls, ["mark-good"]);
        assert!(
            read_pending(&layout).is_none(),
            "a confirmed slot is not still on probation"
        );
    }

    /// §12's `ota.rollback.works`, as far as it can be driven without a bootloader: a deliberately
    /// unhealthy boot, a budget that expires, and an unattended `mark-bad`.
    #[test]
    fn an_unhealthy_boot_rolls_back_when_the_budget_expires() {
        let fixture = Fixture::new("ota-boot-bad");
        let layout = fixture.daemon.store.layout().clone();
        crate::ota::mark_pending(&layout, "1.4.0", 30_000.0).unwrap();

        let unhealthy = BootHealth {
            post_passed: Some(false),
            ..BootHealth::default()
        };
        let mut updater = RecordingUpdater::default();

        // Inside the budget: nothing is said.
        let confirmation = confirm(&layout, &mut updater, "1.4.0", 29_999.0, "boot", &unhealthy);
        assert_eq!(confirmation.verdict.decision, BootDecision::Wait);
        assert!(updater.calls.is_empty());

        // Past it: the slot is condemned.
        let confirmation = confirm(&layout, &mut updater, "1.4.0", 30_000.0, "boot", &unhealthy);
        assert_eq!(confirmation.verdict.decision, BootDecision::RollBack);
        assert_eq!(updater.calls, ["mark-bad"]);
        assert!(
            read_pending(&layout).is_some(),
            "the marker is kept: the rollback happens on the next boot, and a robot that forgot it \
             had condemned this slot would report a healthy pending-free state while still running \
             the image it just condemned"
        );
    }

    /// The end of the rollback story, which nothing else would clear up.
    #[test]
    fn a_marker_for_a_version_the_robot_is_not_running_is_the_rollback_that_happened() {
        let fixture = Fixture::new("ota-boot-returned");
        let layout = fixture.daemon.store.layout().clone();
        crate::ota::mark_pending(&layout, "1.4.0", 30_000.0).unwrap();

        let mut updater = RecordingUpdater::default();
        // The bootloader returned to 1.3.0 and the marker outlived it.
        let confirmation = confirm(&layout, &mut updater, "1.3.0", 5_000.0, "boot", &healthy());
        assert_eq!(confirmation.verdict.decision, BootDecision::NotPending);
        assert_eq!(confirmation.version.as_deref(), Some("1.4.0"));
        assert!(
            updater.calls.is_empty(),
            "nothing more to tell the bootloader"
        );
        assert!(
            read_pending(&layout).is_none(),
            "and the next ordinary boot must not look pending"
        );
    }

    #[test]
    fn a_bootloader_that_cannot_be_reached_is_reported_rather_than_swallowed() {
        let fixture = Fixture::new("ota-boot-refused");
        let layout = fixture.daemon.store.layout().clone();
        crate::ota::mark_pending(&layout, "1.4.0", 600_000.0).unwrap();

        let mut updater = RecordingUpdater {
            refuse: Some("rauc is not installed".to_string()),
            ..RecordingUpdater::default()
        };
        let confirmation = confirm(&layout, &mut updater, "1.4.0", 1_000.0, "boot", &healthy());
        assert_eq!(confirmation.verdict.decision, BootDecision::Confirm);
        assert!(confirmation.failed.is_some());
        assert!(
            read_pending(&layout).is_some(),
            "a slot the bootloader was never told about is still on probation"
        );
    }

    /// §12's first rule is the reconciler's, and this is the assertion that it still is.
    #[test]
    fn applying_an_image_waits_for_a_safe_state_and_staging_does_not() {
        use skynetd::reconcile::{ActionKind, Gate};
        let mut fixture = Fixture::new("ota-gate-order");
        fixture.provision();
        fixture.write_desired(
            r#"{"generation":1,"os":{"channel":"stable","version":"1.4.0"},"packages":[]}"#,
        );
        fixture.daemon.reload();

        let plan =
            skynetd::reconcile::plan(&fixture.daemon.desired, &fixture.daemon.observe(1_000));
        assert!(matches!(
            plan.actions[0].kind,
            ActionKind::StageOsImage { .. }
        ));
        assert_eq!(
            plan.actions[0].gate,
            Gate::Open,
            "staging touches nothing the robot is doing"
        );

        // Once staged, the apply is what waits. §12: never apply mid-task.
        let mut fixture = Fixture::new("ota-gate-order-2");
        fixture.provision();
        fixture.write_desired(
            r#"{"generation":1,"os":{"channel":"stable","version":"1.4.0"},"packages":[]}"#,
        );
        fixture.daemon.reload();
        fixture.daemon.slots = FixedSlots(skynetd::actual::OsSlots {
            active: "1.3.0".to_string(),
            staged: Some("1.4.0".to_string()),
        });
        let plan =
            skynetd::reconcile::plan(&fixture.daemon.desired, &fixture.daemon.observe(1_000));
        assert!(matches!(
            plan.actions[0].kind,
            ActionKind::ApplyOsImage { .. }
        ));
        assert_eq!(plan.actions[0].gate, Gate::SafeState);
    }

    #[test]
    fn the_ota_route_reports_the_slots_and_the_verdict_together() {
        let mut fixture = with_release("ota-route", &["safety_kernel"]);
        fixture.write_desired(
            r#"{"generation":1,"os":{"channel":"stable","version":"1.4.0"},"packages":[]}"#,
        );
        fixture.daemon.reload();

        let (status, body) = fixture.request("GET", "/ota", Some("observer-token"), "");
        assert_eq!(status, 200);
        assert_eq!(body.get("wanted").and_then(Json::as_str), Some("1.4.0"));
        let assessment = body.get("assessment").expect("an assessment");
        assert_eq!(assessment.get("admitted"), Some(&Json::Bool(false)));
        let gated = assessment
            .get("verdict")
            .and_then(|verdict| verdict.get("gated_components"))
            .and_then(Json::as_arr)
            .unwrap();
        assert_eq!(gated.len(), 1);
        assert_eq!(body.get("pending"), Some(&Json::Null));
    }

    #[test]
    fn reading_the_ota_state_needs_an_observer_and_changes_nothing() {
        assert_eq!(requirement("GET", &["ota"]).0, Role::Observer);
        let mut fixture = Fixture::new("ota-role");
        fixture.provision();
        let (status, _) = fixture.request("GET", "/ota", None, "");
        assert_eq!(status, 401);
    }
}

/// §10 on a real machine: the two files, the exchange, and the sentence.
mod identity {
    use super::*;
    use crate::identity::{
        attestation, pair, parse_enrolled, read, running_policy, unpair, CurlEnrolment, Enrol,
        Enrolled, RecordingEnrolment, RunningPolicy,
    };
    use crate::trust::{birth_message, TrustStore};

    fn vendor() -> skynet_ed25519::signing::SigningKey {
        driver_key(9)
    }

    /// A birth certificate for `key`, signed by a vendor this robot trusts (§50.2).
    fn write_certificate(fixture: &Fixture, device_key: &str) {
        let layout = fixture.daemon.store.layout().clone();
        write_trust(fixture, "vendor", &public_hex(&vendor()));

        let mut document = Json::obj();
        document.set("device_key", Json::from(device_key));
        document.set("model", Json::from("bench-arm"));
        document.set("serial", Json::from("SN-0001"));
        document.set("descriptor_digest", Json::from("sha256:dddd"));
        document.set("hardware_revision", Json::from("rev-a"));
        document.set("manufactured", Json::from("2026-01-01"));

        let canonical = document.to_canonical().unwrap();
        // Through the module's own digest, which is domain-separated (§50.2). A fixture that hashed
        // the canonical bytes directly would produce a certificate nothing verifies, and every test
        // below it would quietly be a test about a robot with no certificate.
        let digest = hex_of(&crate::birth::digest_of(&document).unwrap());
        std::fs::create_dir_all(layout.identity()).unwrap();
        std::fs::write(layout.birth_certificate(), canonical.as_bytes()).unwrap();

        let signature = hex_of(&vendor().sign(&birth_message(&digest)));
        std::fs::write(
            layout.birth_signature(),
            format!(
                "{{\"key\":\"{}\",\"value\":\"{signature}\"}}",
                public_hex(&vendor())
            ),
        )
        .unwrap();
    }

    fn enrolled() -> Enrolled {
        Enrolled {
            device_id: "dev_7".to_string(),
            namespace: "acme".to_string(),
            credential: "skd_secretvalue".to_string(),
            scopes: Some(vec!["device:checkin".to_string()]),
        }
    }

    fn audit_of(fixture: &Fixture) -> crate::audit::Audit {
        crate::audit::Audit::of(fixture.daemon.store.layout())
    }

    #[test]
    fn a_robot_nobody_has_paired_reports_itself_unpaired() {
        let fixture = Fixture::new("identity-unpaired");
        let held = read(fixture.daemon.store.layout());
        assert!(!held.identity.credential);
        assert!(held.fault.is_none());
        assert!(held.endpoint.is_none());
    }

    #[test]
    fn pairing_writes_two_files_and_only_one_of_them_holds_the_credential() {
        let mut fixture = Fixture::new("identity-pair");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        write_certificate(&fixture, "aa");

        let hub = RecordingEnrolment::accepting(enrolled());
        let paired = pair(
            &layout,
            &TrustStore::load(&layout),
            "abcd-1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .expect("the Hub accepted the code");

        assert_eq!(paired.namespace, "acme");
        assert_eq!(paired.device_id, "dev_7");

        // The document is rendered by every surface, so it must not carry the credential — not
        // truncated, not masked, absent.
        let document = std::fs::read_to_string(layout.device()).unwrap();
        assert!(document.contains("acme"));
        assert!(
            !document.contains("secretvalue"),
            "the identity document carries the credential: {document}"
        );

        assert_eq!(
            std::fs::read_to_string(layout.credential()).unwrap().trim(),
            "skd_secretvalue"
        );
    }

    #[test]
    fn the_code_is_normalized_before_it_is_presented() {
        let mut fixture = Fixture::new("identity-normalize");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(enrolled());
        pair(
            &layout,
            &TrustStore::load(&layout),
            " abcd-1234 ",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();
        assert_eq!(hub.presented.borrow()[0].0, "ABCD1234");
    }

    #[test]
    fn a_mistyped_code_never_reaches_the_hub() {
        let mut fixture = Fixture::new("identity-mistyped");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(enrolled());
        let failure = pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD-123",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .expect_err("seven characters are not a code");
        assert_eq!(failure.code, "too_short");
        // The point of checking locally: nothing was sent, so nothing is in anybody's log.
        assert!(hub.presented.borrow().is_empty());
        assert!(!layout.credential().exists());
    }

    #[test]
    fn a_credential_pasted_into_pair_never_reaches_the_hub_either() {
        let mut fixture = Fixture::new("identity-pasted");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(enrolled());
        let failure = pair(
            &layout,
            &TrustStore::load(&layout),
            "skd_livecredential",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .expect_err("a credential is not a code");
        assert_eq!(failure.code, "looks_like_a_credential");
        assert!(hub.presented.borrow().is_empty());
        assert!(
            !failure.detail.contains("livecredential"),
            "the refusal echoed the credential: {}",
            failure.detail
        );
    }

    #[test]
    fn a_paired_robot_does_not_quietly_re_pair() {
        let mut fixture = Fixture::new("identity-repair");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(enrolled());
        pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();

        let failure = pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:01:00Z",
            "suite",
        )
        .expect_err("this robot is already in a fleet");
        assert_eq!(failure.code, "already_paired");
        assert!(failure.detail.contains("acme"));
        assert!(failure.detail.contains("unpair"));
    }

    #[test]
    fn a_credential_of_the_wrong_class_is_not_stored() {
        let mut fixture = Fixture::new("identity-wrong-class");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(Enrolled {
            credential: "pat_userstoken".to_string(),
            ..enrolled()
        });
        let failure = pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .expect_err("§10 puts the narrowest credential in the system on the robot");
        assert_eq!(failure.code, "credential_refused");
        assert!(failure.detail.contains("revoke"));
        // Refused *before* it was written, which is the whole point — storing it and complaining
        // afterwards would leave the credential on the disk.
        assert!(!layout.credential().exists());
        assert!(!layout.device().exists());
    }

    #[test]
    fn a_credential_carrying_a_scope_nobody_recognises_is_not_stored() {
        let mut fixture = Fixture::new("identity-wide-scope");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(Enrolled {
            scopes: Some(vec![
                "device:checkin".to_string(),
                "repositories:read".to_string(),
            ]),
            ..enrolled()
        });
        let failure = pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .expect_err("§19's security.credential.scope");
        assert_eq!(failure.code, "credential_refused");
        assert!(!layout.credential().exists());
    }

    #[test]
    fn a_board_swap_is_reported_and_does_not_refuse_the_pairing() {
        // §50.4: replacing a mainboard produces a robot whose certificate no longer matches its
        // hardware, and that is the correct behaviour rather than a bug. A pairing that refused it
        // would leave a repaired fleet unpairable until somebody found the vendor.
        let mut fixture = Fixture::new("identity-board-swap");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        write_certificate(&fixture, "aa");

        let hub = RecordingEnrolment::accepting(enrolled());
        // The robot presents the key the certificate names, so the two agree here; the mismatch
        // this test is about is constructed by moving the certificate underneath afterwards.
        pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();

        write_certificate(&fixture, "bb");
        let (_, verdict) = crate::identity::evaluate(&layout, &TrustStore::load(&layout));
        let found = verdict
            .findings
            .iter()
            .find(|finding| {
                finding.code == skynet_contracts::identity::IdentityFinding::KeyNotAttested
            })
            .expect("the seam between §10 and §50");
        assert!(found.message.contains("§50.4"));
        assert!(!verdict.attested);
        // Still paired: the credential is untouched by a board that changed underneath it.
        assert!(verdict.paired);
    }

    #[test]
    fn a_robot_with_no_certificate_pairs_with_no_key() {
        let mut fixture = Fixture::new("identity-no-certificate");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(enrolled());
        pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();
        assert_eq!(hub.presented.borrow()[0].1.device_key, "");

        let (_, verdict) = crate::identity::evaluate(&layout, &TrustStore::load(&layout));
        assert!(verdict.paired);
        assert!(!verdict.attested);
        assert!(verdict
            .findings
            .iter()
            .all(|finding| finding.severity == skynet_contracts::admission::Severity::Warning));
    }

    #[test]
    fn an_unsigned_certificate_attests_nothing() {
        // A certificate on a robot's disk that nobody trusted is a file anybody with the disk could
        // have written. Reported exactly as an absent one is — §37.1's rule, applied to §50.
        let mut fixture = Fixture::new("identity-unsigned-certificate");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        write_certificate(&fixture, "aa");
        // Remove the key, leaving a well-formed certificate signed by a stranger.
        std::fs::write(layout.trust(), "{\"keys\":[]}").unwrap();

        let observed = crate::identity::read_observed(&layout, &TrustStore::load(&layout));
        assert!(!observed.birth_certificate);
        assert_eq!(observed.birth_key, "");
    }

    #[test]
    fn a_pairing_is_audited_and_the_entry_does_not_carry_the_credential() {
        let mut fixture = Fixture::new("identity-audit");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(enrolled());
        pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();

        let log = audit_of(&fixture).read();
        let entry = log
            .entries
            .iter()
            .find(|entry| entry.action == "identity.pair")
            .expect("§19 audit-logs the change that decides who may command this robot");
        assert_eq!(entry.subject, "acme");
        assert_eq!(entry.actor, "suite");
        let line = format!("{entry:?}");
        assert!(!line.contains("secretvalue"), "{line}");
        // Nor the code, which is a bearer secret for the length of the pairing.
        assert!(!line.contains("ABCD1234"), "{line}");
    }

    #[test]
    fn unpairing_removes_both_files_and_says_the_hub_was_not_told() {
        let mut fixture = Fixture::new("identity-unpair");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(enrolled());
        pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();

        let namespace = unpair(
            &layout,
            &audit_of(&fixture),
            "2026-09-02T00:05:00Z",
            "suite",
        )
        .unwrap();
        assert_eq!(namespace.as_deref(), Some("acme"));
        assert!(!layout.credential().exists());
        assert!(!layout.device().exists());

        let log = audit_of(&fixture).read();
        let entry = log
            .entries
            .iter()
            .find(|entry| entry.action == "identity.unpair")
            .expect("unpairing is a change too");
        assert!(
            entry.detail.contains("Revoke the credential on the Hub"),
            "{}",
            entry.detail
        );
    }

    #[test]
    fn unpairing_a_robot_that_was_never_paired_is_not_an_error() {
        let fixture = Fixture::new("identity-unpair-twice");
        let layout = fixture.daemon.store.layout().clone();
        let answer = unpair(
            &layout,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();
        assert_eq!(answer, None);
    }

    #[test]
    fn a_document_that_does_not_parse_is_not_the_same_as_no_document() {
        let fixture = Fixture::new("identity-broken-document");
        let layout = fixture.daemon.store.layout().clone();
        std::fs::create_dir_all(layout.identity()).unwrap();
        std::fs::write(layout.device(), "{ this is not json").unwrap();

        let held = read(&layout);
        let fault = held.fault.expect("a broken file is a fact worth reporting");
        assert!(fault.contains("does not parse"), "{fault}");
        // And it still answers, because the robot whose identity somebody needs to look at is
        // exactly this one.
        assert!(!held.identity.credential);
    }

    #[test]
    fn an_interrupted_pairing_leaves_a_robot_that_can_pair_again() {
        // Whether a robot is paired is read off the credential, not off the document. This is that
        // rule, from the direction it matters: a document with no credential beside it must not
        // claim a fleet.
        let mut fixture = Fixture::new("identity-half-written");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(enrolled());
        pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();
        std::fs::remove_file(layout.credential()).unwrap();

        let held = read(&layout);
        assert!(!held.identity.credential);
        assert!(pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:02:00Z",
            "suite",
        )
        .is_ok());
    }

    #[test]
    fn a_hub_that_returns_no_credential_is_refused_by_name() {
        let answer = parse_enrolled("{\"device_id\":\"dev_7\",\"namespace\":\"acme\"}")
            .expect_err("there is nothing to pair with");
        assert_eq!(answer.code, "no_credential");
        assert!(answer.detail.contains("Nothing has been stored"));
    }

    #[test]
    fn an_absent_scope_list_and_an_empty_one_are_different_facts() {
        let opaque = parse_enrolled("{\"credential\":\"skd_a\"}").unwrap();
        assert_eq!(opaque.scopes, None);
        let empty = parse_enrolled("{\"credential\":\"skd_a\",\"scopes\":[]}").unwrap();
        assert_eq!(empty.scopes, Some(Vec::new()));
    }

    #[test]
    fn the_request_body_carries_the_code_and_the_url_does_not() {
        let enrolment = CurlEnrolment {
            endpoint: "https://hub.example/".to_string(),
            scratch: std::path::PathBuf::from("/nonexistent"),
        };
        assert_eq!(enrolment.url(), "https://hub.example/devices/pair");
        assert!(!enrolment.url().contains("ABCD"));

        let request = Enrol {
            device_key: "aa".to_string(),
            model: "bench-arm".to_string(),
            serial: "SN-0001".to_string(),
            agent: "skynetd/test".to_string(),
        };
        let body = request.body("ABCD1234").to_canonical().unwrap();
        assert!(body.contains("ABCD1234"));
    }

    #[test]
    fn the_sentence_names_the_version_when_it_cannot_name_the_bytes() {
        let fixture = Fixture::new("identity-attest-no-manifest");
        let layout = fixture.daemon.store.layout().clone();
        let (held, _) = crate::identity::evaluate(&layout, &TrustStore::load(&layout));
        let attestation = attestation(
            &layout,
            &TrustStore::load(&layout),
            &held,
            "1.4.0",
            &RunningPolicy::None,
        );
        let report = skynet_contracts::identity::attest(&attestation);
        assert!(!report.complete);
        assert_eq!(report.unmeasured, vec!["os_image"]);
        assert!(
            report.sentence.contains("1.4.0"),
            "the sentence names the version it cannot turn into a digest: {}",
            report.sentence
        );
        // And it is still a sentence. §13's rule: a report with a hole in it is still produced.
        assert!(report.sentence.starts_with("This robot is running"));
        assert!(report.sentence.contains("no policy"));
    }

    #[test]
    fn a_robot_with_no_descriptor_has_no_body_rather_than_an_unmeasured_one() {
        let fixture = Fixture::new("identity-attest-no-body");
        let layout = fixture.daemon.store.layout().clone();
        let (held, _) = crate::identity::evaluate(&layout, &TrustStore::load(&layout));
        let attestation = attestation(
            &layout,
            &TrustStore::load(&layout),
            &held,
            "",
            &RunningPolicy::Unknown("the runtime is not answering".to_string()),
        );
        assert_eq!(
            attestation.embodiment.state,
            skynet_contracts::identity::MeasureState::Nothing
        );
        let report = skynet_contracts::identity::attest(&attestation);
        assert_eq!(report.unmeasured, vec!["os_image", "policy"]);
        assert!(report.sentence.contains("no body"));
        assert!(report.sentence.contains("the runtime is not answering"));
    }

    #[test]
    fn nothing_running_is_not_the_same_as_nothing_observed() {
        let fixture = Fixture::new("identity-policy-none");
        let actual = fixture.daemon.observe(1_000);
        assert_eq!(
            running_policy(&fixture.daemon.store, &actual),
            RunningPolicy::None
        );
    }

    #[test]
    fn the_identity_route_carries_no_credential_in_any_form() {
        let mut fixture = Fixture::new("identity-route");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(enrolled());
        pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();

        let (status, document) = fixture.request("GET", "/identity", Some("observer-token"), "");
        assert_eq!(status, 200);
        let rendered = document.to_canonical().unwrap();
        assert!(
            !rendered.contains("secretvalue"),
            "the identity route served the credential: {rendered}"
        );
        assert_eq!(
            document.get("credential_prefix").and_then(Json::as_str),
            Some("skd_")
        );
        assert_eq!(document.get("paired").and_then(Json::as_bool), Some(true));
        assert!(document
            .get("attestation")
            .and_then(|node| node.get("sentence"))
            .and_then(Json::as_str)
            .is_some());
    }

    #[test]
    fn reading_an_identity_needs_an_observer_and_pairing_needs_an_admin() {
        let mut fixture = Fixture::new("identity-roles");
        fixture.provision();
        assert_eq!(
            fixture.request("GET", "/identity", None, "").0,
            401,
            "§39.1 gives an unauthenticated caller stop and status"
        );
        assert_eq!(
            fixture
                .request("GET", "/identity", Some("observer-token"), "")
                .0,
            200
        );

        let body = "{\"code\":\"ABCD1234\",\"endpoint\":\"https://hub.example\"}";
        assert_eq!(
            fixture
                .request("POST", "/identity/pair", Some("observer-token"), body)
                .0,
            403,
            "§39.1: pair and unpair are Admin"
        );
    }

    #[test]
    fn pairing_over_the_api_needs_an_endpoint_because_there_is_no_default() {
        let mut fixture = Fixture::new("identity-no-default-hub");
        fixture.provision();
        let (status, document) = fixture.request(
            "POST",
            "/identity/pair",
            Some("admin-token"),
            "{\"code\":\"ABCD1234\"}",
        );
        assert_eq!(status, 400);
        let detail = document.get("detail").and_then(Json::as_str).unwrap_or("");
        assert!(detail.contains("no default Hub"), "{detail}");
    }

    #[test]
    fn a_robot_with_nowhere_to_send_names_the_command_that_fixes_it() {
        use crate::connectivity::{Sink, Unpaired};
        let obstacle = Unpaired.obstacle().unwrap();
        assert!(obstacle.contains("skynet pair"));
        assert!(obstacle.contains("§34.4"));
    }

    #[test]
    fn a_paired_robot_gets_a_sink_that_actually_sends() {
        // The state this replaced: for one session a paired robot had a sink whose whole content
        // was that §34.2's transport did not exist. It does now, and the sink is chosen by whether
        // all three of the things a report needs are on the disk.
        let mut fixture = Fixture::new("sink-paired");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        assert_eq!(crate::identity::credential_for_sending(&layout), None);

        let hub = RecordingEnrolment::accepting(enrolled());
        pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &audit_of(&fixture),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();

        let (endpoint, device_id, credential) =
            crate::identity::credential_for_sending(&layout).expect("paired");
        assert_eq!(endpoint, "https://hub.example");
        assert_eq!(device_id, "dev_7");
        assert_eq!(credential, "skd_secretvalue");

        let sink = crate::connectivity::Hub {
            endpoint,
            device_id,
            credential,
            scratch: layout.state(),
        };
        assert!(crate::connectivity::Sink::obstacle(&sink).is_none());
        assert!(crate::connectivity::Sink::delivers(&sink));
        assert!(!crate::connectivity::Sink::describe(&sink).contains("secretvalue"));
    }
}

/// §11 and §17's check-in on a real machine.
mod checkin {
    use super::*;
    use crate::checkin::{build, enqueue, health, progress, stage_name, to_json, NotQueued};
    use crate::identity::{pair, RecordingEnrolment};
    use crate::outbound::Queue;
    use crate::trust::TrustStore;
    use skynet_contracts::checkin::Claim;

    fn enrolled() -> crate::identity::Enrolled {
        crate::identity::Enrolled {
            device_id: "dev_7".to_string(),
            namespace: "acme".to_string(),
            credential: "skd_secretvalue".to_string(),
            scopes: Some(vec!["device:checkin".to_string()]),
        }
    }

    fn become_paired(fixture: &Fixture) {
        let layout = fixture.daemon.store.layout().clone();
        let hub = RecordingEnrolment::accepting(enrolled());
        pair(
            &layout,
            &TrustStore::load(&layout),
            "ABCD1234",
            &hub,
            &crate::audit::Audit::of(&layout),
            "2026-09-02T00:00:00Z",
            "suite",
        )
        .unwrap();
    }

    fn built_for(fixture: &mut Fixture) -> skynet_contracts::checkin::CheckIn {
        let layout = fixture.daemon.store.layout().clone();
        let trust = TrustStore::load(&layout);
        let (identity, _) = crate::identity::evaluate(&layout, &trust);
        let actual = fixture.daemon.observe(1_000);
        let observed = fixture.daemon.health(&actual, 1_000);
        let policy = crate::identity::running_policy(&fixture.daemon.store, &actual);
        let attestation =
            crate::identity::attestation(&layout, &trust, &identity, &actual.os.active, &policy);
        let lag = fixture
            .daemon
            .desired
            .generation
            .checked_sub(actual.generation)
            .filter(|_| fixture.daemon.desired.generation >= actual.generation);
        build(
            &identity,
            attestation,
            progress(&fixture.daemon.desired, &actual, fixture.daemon.safe_state),
            health(&observed, 0, lag),
            fixture.daemon.operating_state.as_str(),
            &crate::intent::held_digest(&layout),
        )
    }

    #[test]
    fn the_six_stage_numbers_map_to_the_six_names() {
        let names: Vec<String> = (1..=6).map(stage_name).collect();
        assert_eq!(
            names,
            vec![
                "os_image",
                "rhal_driver",
                "package",
                "profile",
                "policy",
                "lease"
            ]
        );
    }

    #[test]
    fn a_stage_number_nobody_recognises_gets_a_column_of_its_own() {
        // Rather than one of the six, and rather than a shared `unknown`. It cannot happen from
        // this workspace; if it ever does, a fleet console should show it as the thing nobody
        // recognises rather than silently as a lease.
        assert_eq!(stage_name(0), "stage_0");
        assert_eq!(stage_name(9), "stage_9");
    }

    #[test]
    fn an_unpaired_robot_queues_nothing_and_says_why() {
        let mut fixture = Fixture::new("checkin-unpaired");
        let layout = fixture.daemon.store.layout().clone();
        let built = built_for(&mut fixture);
        let trust = TrustStore::load(&layout);
        let (identity, _) = crate::identity::evaluate(&layout, &trust);

        let queued = enqueue(&Queue::of(&layout), &identity, &built);
        assert_eq!(queued, Err(NotQueued::Unpaired));

        // And the queue stays empty rather than filling with documents addressed to nobody, which
        // would eventually show up as this robot discarding its own state.
        assert!(Queue::of(&layout).items().is_empty());

        let rendered = to_json(&built, &queued);
        assert_eq!(rendered.get("queued").and_then(Json::as_bool), Some(false));
        assert!(rendered
            .get("queued_note")
            .and_then(Json::as_str)
            .is_some_and(|note| note.contains("not paired")));
    }

    #[test]
    fn a_paired_robot_queues_one_and_the_next_supersedes_it() {
        // §34.4: check-ins are the class where only the newest matters. Queueing every pass is
        // therefore correct rather than wasteful, and this is the property that makes it so.
        let mut fixture = Fixture::new("checkin-collapses");
        fixture.provision();
        become_paired(&fixture);
        let layout = fixture.daemon.store.layout().clone();
        let trust = TrustStore::load(&layout);
        let (identity, _) = crate::identity::evaluate(&layout, &trust);

        let queue = Queue::of(&layout);
        for _ in 0..3 {
            let built = built_for(&mut fixture);
            enqueue(&queue, &identity, &built)
                .expect("a paired robot has somewhere to address one");
        }

        let held = queue.items();
        assert_eq!(held.len(), 1, "three check-ins collapsed to one");
        assert_eq!(held[0].class, crate::outbound::Class::CheckIn);

        // Collapsing is not a drop, and is counted apart from one (§34.4).
        let status = queue.status();
        assert_eq!(status.dropped.iter().map(|(_, n)| *n).sum::<u64>(), 0);
    }

    #[test]
    fn the_queued_body_is_the_document_and_carries_no_credential() {
        let mut fixture = Fixture::new("checkin-body");
        fixture.provision();
        become_paired(&fixture);
        let layout = fixture.daemon.store.layout().clone();
        let trust = TrustStore::load(&layout);
        let (identity, _) = crate::identity::evaluate(&layout, &trust);

        let queue = Queue::of(&layout);
        let built = built_for(&mut fixture);
        enqueue(&queue, &identity, &built).unwrap();

        let body = queue.items().remove(0).body;
        assert!(!body.contains("secretvalue"), "{body}");
        let document = Json::parse(&body).expect("a check-in is canonical JSON");
        assert_eq!(
            document.get("device_id").and_then(Json::as_str),
            Some("dev_7")
        );
        // §10's sentence rides along, which is the whole point of the pair of them.
        assert!(document
            .get("attestation")
            .and_then(|node| node.get("sentence"))
            .and_then(Json::as_str)
            .is_some());
    }

    #[test]
    fn a_robot_told_nothing_reports_applied_and_a_robot_told_something_does_not() {
        let mut fixture = Fixture::new("checkin-claim");
        fixture.provision();
        become_paired(&fixture);

        // Nothing desired: generation zero against zero, empty plan.
        assert_eq!(built_for(&mut fixture).claim.claim, Claim::Applied);

        // A package the robot does not have. Now there is work, and the generation on the wire is
        // still the one this robot has actually applied.
        fixture.write_desired(
            "{\"generation\":41,\"packages\":[{\"id\":\"acme/grasp\",\"kind\":\"policy\",\
             \"version\":\"1.0.0\",\"digest\":\"sha256:aa\",\"state\":\"running\"}]}",
        );
        fixture.daemon.reload();

        let built = built_for(&mut fixture);
        assert_ne!(built.claim.claim, Claim::Applied);
        assert!(!built.claim.applied);
        assert_eq!(built.claim.generation, 0, "never the desired generation");
        assert!(built.claim.blocking.contains(&"package".to_string()));
    }

    #[test]
    fn the_pending_reasons_use_the_same_action_names_as_the_plan_route() {
        // One vocabulary for one plan. A Hub reading `fetch_package` and a console reading
        // something else would be two descriptions of the same action.
        let mut fixture = Fixture::new("checkin-vocabulary");
        fixture.provision();
        become_paired(&fixture);
        fixture.write_desired(
            "{\"generation\":41,\"packages\":[{\"id\":\"acme/grasp\",\"kind\":\"policy\",\
             \"version\":\"1.0.0\",\"digest\":\"sha256:aa\",\"state\":\"running\"}]}",
        );
        fixture.daemon.reload();

        let built = built_for(&mut fixture);
        assert!(
            built
                .pending
                .iter()
                .any(|entry| entry.reason.starts_with("fetch_package ")),
            "{:?}",
            built.pending
        );
    }

    #[test]
    fn four_of_the_five_aggregate_fields_are_absent_rather_than_zero() {
        // §33.6: no measurement of guard trips and zero guard trips are opposite facts. Nothing on
        // this robot measures four of them, and the check-in says so.
        let mut fixture = Fixture::new("checkin-unmeasured");
        fixture.provision();
        become_paired(&fixture);
        let built = built_for(&mut fixture);
        assert_eq!(
            built.health.unmeasured,
            vec![
                "battery",
                "guard_trip_rate",
                "storage_headroom",
                "temperature_c"
            ]
        );
        assert!(built.health.undated.is_empty());
        // The one this daemon can compute, and it is dated.
        assert_eq!(built.health.fields.len(), 1);
        assert_eq!(built.health.fields[0].0, "generation_lag");
    }

    #[test]
    fn the_route_answers_what_it_would_say_rather_than_nothing() {
        let mut fixture = Fixture::new("checkin-route");
        fixture.provision();
        become_paired(&fixture);

        let (status, document) = fixture.request("GET", "/checkin", Some("observer-token"), "");
        assert_eq!(status, 200);
        assert_eq!(
            document.get("device_id").and_then(Json::as_str),
            Some("dev_7")
        );
        assert!(document.get("summary").and_then(Json::as_str).is_some());
        let rendered = document.to_canonical().unwrap();
        assert!(!rendered.contains("secretvalue"), "{rendered}");
    }

    #[test]
    fn reading_a_check_in_needs_an_observer() {
        let mut fixture = Fixture::new("checkin-role");
        fixture.provision();
        assert_eq!(fixture.request("GET", "/checkin", None, "").0, 401);
        assert_eq!(
            fixture
                .request("GET", "/checkin", Some("observer-token"), "")
                .0,
            200
        );
    }

    #[test]
    fn a_get_never_queues_one() {
        // §33.7's routes are the Hub's shapes, and a `GET` that enqueued would be a state change
        // behind a verb that promises not to be one. The reconcile loop is what queues.
        let mut fixture = Fixture::new("checkin-get-is-read-only");
        fixture.provision();
        become_paired(&fixture);
        let layout = fixture.daemon.store.layout().clone();

        for _ in 0..3 {
            assert_eq!(
                fixture
                    .request("GET", "/checkin", Some("observer-token"), "")
                    .0,
                200
            );
        }
        assert!(Queue::of(&layout).items().is_empty());
    }

    #[test]
    fn every_class_has_somewhere_to_go() {
        // This test was `every_class_but_telemetry_has_somewhere_to_go` for three sessions, and its
        // last assertion was the finding: §34.4 gave telemetry a priority, a drop policy and a
        // counter, and neither §17's eight endpoints nor §57's revised nine gave it a route, so a
        // robot that guessed one would have been inventing a Hub surface.
        //
        // §74 closes it. The assertion is kept in the same shape rather than deleted, because the
        // thing worth pinning is unchanged — a class with no route must never be given a guessed
        // one — and the day a fifth class is added this is where that decision will be made.
        let hub = crate::connectivity::Hub {
            endpoint: "https://hub.example/".to_string(),
            device_id: "dev_7".to_string(),
            credential: "skd_x".to_string(),
            scratch: std::path::PathBuf::from("/nonexistent"),
        };
        assert_eq!(
            hub.route(crate::outbound::Class::CheckIn).as_deref(),
            Some("https://hub.example/devices/dev_7/checkin")
        );
        assert_eq!(
            hub.route(crate::outbound::Class::Incident).as_deref(),
            Some("https://hub.example/devices/dev_7/incidents")
        );
        assert_eq!(
            hub.route(crate::outbound::Class::Episode).as_deref(),
            Some("https://hub.example/episodes")
        );
        assert_eq!(
            hub.route(crate::outbound::Class::Telemetry).as_deref(),
            Some("https://hub.example/devices/dev_7/telemetry")
        );
        // And the general form: no class is silently unroutable.
        for class in crate::outbound::CLASSES {
            assert!(hub.route(class).is_some(), "{}", class.as_str());
        }
    }

    #[test]
    fn a_credential_that_could_break_the_config_is_refused_rather_than_escaped() {
        // The credential travels in curl's own config on stdin, which is a quoted string. A
        // credential containing a quote is a Hub doing something nobody designed, and escaping it
        // would be this daemon guessing at a parser's escape set.
        let scratch = TempRoot::new("post-quoting");
        for bad in ["skd_a\"b", "skd_a\\b", "skd_a\nb"] {
            let error = crate::connectivity::post_json(
                "https://hub.example/x",
                Some(bad),
                "{}",
                scratch.layout().state().as_path(),
            )
            .expect_err("a credential with a quote is not one this robot presents");
            assert!(error.contains("will not put in a curl config"), "{error}");
        }
    }
}

/// §11's pull on a real machine.
mod intent {
    use super::*;
    use crate::intent::{adopt, digest_of, held, pull};
    use skynet_contracts::intent::OfferFaultCode;

    const ONE: &str =
        "{\"generation\":41,\"packages\":[{\"id\":\"acme/grasp\",\"kind\":\"policy\",\
                       \"version\":\"1.0.0\",\"digest\":\"sha256:aa\",\"state\":\"running\"}]}";

    #[test]
    fn an_unpaired_robot_does_not_ask() {
        // There is no `/devices/{id}/desired` to ask, which is §50.2's shipping state rather than a
        // failure. `None` rather than a `Pulled` that failed, so the route can say which.
        let fixture = Fixture::new("intent-unpaired");
        assert_eq!(pull(fixture.daemon.store.layout(), 0), None);
    }

    #[test]
    fn a_response_that_is_not_a_desired_state_never_reaches_the_disk() {
        // The rule this module exists for. A version that wrote first and let §33.8's ladder sort it
        // out would lose the intent the robot was reconciling against, silently.
        let fixture = Fixture::new("intent-garbage");
        let layout = fixture.daemon.store.layout().clone();
        fixture.write_desired(ONE);
        let before = std::fs::read_to_string(layout.desired()).unwrap();

        for body in ["not json at all", "{}", "{\"packages\":[]}", ""] {
            let pulled = adopt(&layout, body, 1);
            assert!(pulled.reached);
            assert!(!pulled.written, "{body:?} was written");
            let verdict = pulled.verdict.expect("a decision was made");
            assert!(!verdict.adopt);
            assert_eq!(verdict.faults[0].code, OfferFaultCode::Undecodable);
            // And the robot keeps reconciling towards what it had.
            assert_eq!(verdict.generation, 41);
        }
        assert_eq!(std::fs::read_to_string(layout.desired()).unwrap(), before);
    }

    #[test]
    fn a_new_generation_lands_and_is_stored_canonically() {
        let fixture = Fixture::new("intent-adopt");
        let layout = fixture.daemon.store.layout().clone();
        fixture.write_desired(ONE);

        // The same document at a higher generation, pretty-printed the way a Hub might serve it.
        let offered = "{\n  \"generation\" : 42,\n  \"packages\" : []\n}";
        let pulled = adopt(&layout, offered, 1);
        let verdict = pulled.verdict.expect("a decision");
        assert!(verdict.adopt);
        assert!(verdict.changed);
        assert!(pulled.written);

        let stored = std::fs::read_to_string(layout.desired()).unwrap();
        // Canonical, not as it arrived. Storing the Hub's own formatting would make the next poll
        // report a change against a document that had not changed.
        assert_eq!(stored, "{\"generation\":42,\"packages\":[]}");
    }

    #[test]
    fn reformatting_alone_is_not_a_change() {
        // The digest is over the canonical form, so a Hub that reflowed its JSON does not make every
        // poll a write.
        let fixture = Fixture::new("intent-reformat");
        let layout = fixture.daemon.store.layout().clone();
        fixture.write_desired("{\"generation\":41,\"packages\":[]}");

        let pulled = adopt(&layout, "{\n \"packages\":[],\n \"generation\": 41\n}", 0);
        let verdict = pulled.verdict.expect("a decision");
        assert!(verdict.adopt);
        assert!(!verdict.changed);
        assert!(!pulled.written);
    }

    #[test]
    fn the_ordinary_poll_writes_nothing() {
        let fixture = Fixture::new("intent-ordinary");
        let layout = fixture.daemon.store.layout().clone();
        fixture.write_desired("{\"generation\":41,\"packages\":[]}");
        let pulled = adopt(&layout, "{\"generation\":41,\"packages\":[]}", 0);
        assert!(!pulled.written);
        assert!(!pulled.verdict.unwrap().changed);
    }

    #[test]
    fn a_reused_generation_is_refused_and_the_old_document_stays() {
        let fixture = Fixture::new("intent-reuse");
        let layout = fixture.daemon.store.layout().clone();
        fixture.write_desired(ONE);
        let before = std::fs::read_to_string(layout.desired()).unwrap();

        // Generation 41 again, different content.
        let pulled = adopt(&layout, "{\"generation\":41,\"packages\":[]}", 1);
        let verdict = pulled.verdict.expect("a decision");
        assert!(!verdict.adopt);
        assert_eq!(verdict.faults[0].code, OfferFaultCode::GenerationReused);
        assert!(!pulled.written);
        assert_eq!(std::fs::read_to_string(layout.desired()).unwrap(), before);
    }

    #[test]
    fn a_backup_restore_is_followed_and_reported() {
        let fixture = Fixture::new("intent-backwards");
        let layout = fixture.daemon.store.layout().clone();
        fixture.write_desired(ONE);

        let pulled = adopt(&layout, "{\"generation\":40,\"packages\":[]}", 1);
        let verdict = pulled.verdict.expect("a decision");
        assert!(verdict.adopt, "a fleet nobody could recover is worse");
        assert!(pulled.written);
        assert!(verdict
            .faults
            .iter()
            .any(|fault| fault.code == OfferFaultCode::GenerationWentBackwards));
    }

    #[test]
    fn emptying_a_running_robot_is_adopted_and_said_out_loud() {
        let fixture = Fixture::new("intent-empties");
        let layout = fixture.daemon.store.layout().clone();
        fixture.write_desired(ONE);

        let pulled = adopt(&layout, "{\"generation\":42,\"packages\":[]}", 3);
        let verdict = pulled.verdict.expect("a decision");
        assert!(verdict.adopt);
        assert!(verdict
            .faults
            .iter()
            .any(|fault| fault.code == OfferFaultCode::EmptiesTheRobot));
    }

    #[test]
    fn the_held_digest_is_over_the_canonical_form() {
        let fixture = Fixture::new("intent-digest");
        let layout = fixture.daemon.store.layout().clone();
        fixture.write_desired("{\"generation\":41,\"packages\":[]}");
        let one = held(&layout, 0);

        fixture.write_desired("{\n\"packages\" : [],\n\"generation\":41\n}");
        let two = held(&layout, 0);
        assert_eq!(one.digest, two.digest);
        assert!(one.digest.starts_with("sha256:"));
    }

    #[test]
    fn a_robot_with_no_desired_state_at_all_holds_nothing_and_takes_anything() {
        let fixture = Fixture::new("intent-first-pull");
        let layout = fixture.daemon.store.layout().clone();
        let holding = held(&layout, 0);
        assert_eq!(holding.generation, 0);
        assert_eq!(holding.digest, "");

        let pulled = adopt(&layout, "{\"generation\":1,\"packages\":[]}", 0);
        let verdict = pulled.verdict.expect("a decision");
        assert!(verdict.adopt);
        assert!(verdict.changed);
        // And no reuse reported against a robot that had nothing to reuse.
        assert!(verdict.faults.is_empty());
        assert!(pulled.written);
    }

    #[test]
    fn the_route_reports_the_last_pull_and_never_makes_one() {
        let mut fixture = Fixture::new("intent-route");
        fixture.provision();
        fixture.write_desired(ONE);

        let (status, document) = fixture.request("GET", "/intent", Some("observer-token"), "");
        assert_eq!(status, 200);
        // §33.8's ladder, which is the half that is true whether or not a Hub was ever reached.
        assert_eq!(
            document.get("source").and_then(Json::as_str),
            Some("current")
        );
        // Nothing has pulled, and an unpaired robot never will — reported apart from a pull that
        // failed.
        assert_eq!(document.get("pull"), Some(&Json::Null));
        assert!(document
            .get("note")
            .and_then(Json::as_str)
            .is_some_and(|note| note.contains("not paired")));
    }

    #[test]
    fn the_route_shows_the_fallback_rung_when_the_desired_state_is_broken() {
        // The state that is invisible from every other surface: a robot reconciling against a
        // last-known-good copy looks exactly like one reconciling against a current desired state.
        let mut fixture = Fixture::new("intent-fallback");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        fixture.write_desired(ONE);
        // Promote it to last-known-good by reading once, then break the current copy.
        let _ = crate::runtime::load_desired(&layout);
        fixture.write_desired("{ broken");

        let (status, document) = fixture.request("GET", "/intent", Some("observer-token"), "");
        assert_eq!(status, 200);
        assert_eq!(
            document.get("source").and_then(Json::as_str),
            Some("last_known_good")
        );
        assert!(document
            .get("source_because")
            .and_then(Json::as_str)
            .is_some_and(|why| why.contains("does not parse")));
    }

    #[test]
    fn reading_the_intent_needs_an_observer() {
        let mut fixture = Fixture::new("intent-role");
        fixture.provision();
        assert_eq!(fixture.request("GET", "/intent", None, "").0, 401);
        assert_eq!(
            fixture
                .request("GET", "/intent", Some("observer-token"), "")
                .0,
            200
        );
    }

    #[test]
    fn two_documents_that_mean_the_same_thing_have_one_digest() {
        let a = Json::parse("{\"generation\":1,\"packages\":[]}").unwrap();
        let b = Json::parse("{\n\"packages\":[],\n\"generation\":1}").unwrap();
        assert_eq!(digest_of(&a), digest_of(&b));
    }
}

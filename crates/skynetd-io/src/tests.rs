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

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("skynetd-io-{}-{}-{}", std::process::id(), label, n));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp root");
        Self(path)
    }

    fn layout(&self) -> Layout {
        Layout::new(&self.0)
    }

    fn path(&self) -> &std::path::Path {
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

#[test]
fn a_staged_package_is_never_reported_as_signed() {
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
    assert_eq!(staged.signature, crate::store::Signature::NotChecked);
    assert!(!staged.signature.is_verified());
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
        let mut context = Context {
            daemon: &mut self.daemon,
            credentials: &credentials,
            now_ms: 1_000,
            started_ms: 0,
            // The suite exercises the API without a recorder: the routes that need one
            // answer 503, and the rest do not care.
            blackbox: None,
            mono_ns: 0,
        };
        let response = api::handle(&mut context, &request);
        let parsed = Json::parse(std::str::from_utf8(&response.body).unwrap()).unwrap();
        (response.status, parsed)
    }
}

// Read only by the `daemon` module below, which needs POSIX. Gated with it rather than
// `#[allow(dead_code)]`, so that a constant which stops being used anywhere is still reported.
#[cfg(unix)]
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

#[test]
fn status_says_the_safe_state_was_declared_rather_than_measured() {
    let mut fixture = Fixture::new("safe-state-status");
    let (_, body) = fixture.request("GET", "/status", None, "");
    assert_eq!(
        body.get("safe_state_source").and_then(Json::as_str),
        Some("declared")
    );
    assert!(body
        .get("safe_state_note")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .contains("not measured"));
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
    assert_eq!(body.get("converged"), Some(&Json::Bool(false)));
    let applied = body.get("applied").and_then(Json::as_arr).unwrap();
    assert_eq!(
        applied[0].get("outcome").and_then(Json::as_str),
        Some("unimplemented")
    );
    assert!(applied[0].get("reason").is_some());
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
        let mut context = Context {
            daemon: &mut fixture.daemon,
            credentials: &credentials,
            now_ms: 1_000,
            started_ms: 0,
            // The suite exercises the API without a recorder: the routes that need one
            // answer 503, and the rest do not care.
            blackbox: None,
            mono_ns: 0,
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

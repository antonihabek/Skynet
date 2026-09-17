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
use crate::blackbox;
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
        env: Vec::new(),
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
        env: vec![(
            "SKYNET_RING_SOCKET".to_string(),
            "/run/policy.sock".to_string(),
        )],
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
    // The environment goes to `systemd-run`, before the program: after it, a `--setenv` is an
    // argument the package receives and does nothing.
    let setenv = argv
        .iter()
        .position(|arg| arg == "--setenv=SKYNET_RING_SOCKET=/run/policy.sock")
        .expect("the rendezvous is on the command line");
    let program = argv
        .iter()
        .position(|arg| arg.ends_with("bin/run"))
        .expect("the program");
    assert!(
        setenv < program,
        "a --setenv after the program is an argument to it"
    );
    assert!(argv.contains(&"MemoryMax=512M".to_string()));
    assert!(argv.contains(&"AllowedCPUs=0".to_string()));
    assert_eq!(argv.last().unwrap(), "--fast");
}

fn confined_unit(kind: PackageKind) -> Unit {
    Unit {
        id: "openrobot/grasp".to_string(),
        kind,
        program: PathBuf::from("/data/packages/blobs/x/bin/run"),
        args: Vec::new(),
        env: Vec::new(),
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
    }
}

/// §8.3 has no override: a policy launches sandboxed, and the exemption for a driver is the same
/// section's — so the test asserts both halves, on the argv systemd will actually see.
#[test]
fn a_policy_unit_launches_confined_and_a_driver_does_not() {
    let policy = SystemdLauncher::system()
        .command_line(&confined_unit(PackageKind::Policy))
        .unwrap();
    for property in [
        "NoNewPrivileges=yes",
        "ProtectSystem=yes",
        "ProtectKernelTunables=yes",
        "ProtectKernelModules=yes",
        "ProtectControlGroups=yes",
        "RestrictSUIDSGID=yes",
        "CapabilityBoundingSet=",
    ] {
        assert!(
            policy.contains(&property.to_string()),
            "a policy launches with {property}"
        );
    }
    // The confinement changes nothing about the ceilings: the sandbox is not a second budget.
    assert!(policy.contains(&"MemoryMax=512M".to_string()));
    assert!(policy.contains(&"AllowedCPUs=0".to_string()));

    let driver = SystemdLauncher::system()
        .command_line(&confined_unit(PackageKind::Driver))
        .unwrap();
    for property in [
        "NoNewPrivileges=yes",
        "ProtectSystem=yes",
        "ProtectKernelTunables=yes",
        "ProtectKernelModules=yes",
        "ProtectControlGroups=yes",
        "RestrictSUIDSGID=yes",
        "CapabilityBoundingSet=",
    ] {
        assert!(
            !driver.contains(&property.to_string()),
            "a driver runs native per §8.3, so {property} must not be there"
        );
    }
    // Native is not unlimited: the exemption is from confinement, not from §40.
    assert!(driver.contains(&"MemoryMax=512M".to_string()));
    assert!(driver.contains(&"AllowedCPUs=0".to_string()));
}

/// Services and skills are packages too, and §8.3 names only the driver as native — so every
/// other kind carries exactly the policy's confinement, now and after the next kind is added.
#[test]
fn every_non_driver_kind_carries_the_same_confinement() {
    let policy = launch::confinement_properties(PackageKind::Policy);
    assert!(!policy.is_empty());
    assert_eq!(launch::confinement_properties(PackageKind::Service), policy);
    assert_eq!(launch::confinement_properties(PackageKind::Skill), policy);
    assert!(launch::confinement_properties(PackageKind::Driver).is_empty());
}

/// What systemd printed, parsed. Blank lines and lines without `=` are not properties.
#[test]
fn a_property_block_parses_to_pairs() {
    let text = "NoNewPrivileges=yes\nCapabilityBoundingSet=\nAllowedCPUs=0-3\n\nrubbish\n";
    let parsed = launch::parse_properties(text);
    assert_eq!(
        parsed,
        vec![
            ("NoNewPrivileges".to_string(), "yes".to_string()),
            // The empty value survives as an empty value rather than being dropped, which is the
            // whole point of the directive it belongs to.
            ("CapabilityBoundingSet".to_string(), String::new()),
            ("AllowedCPUs".to_string(), "0-3".to_string()),
        ]
    );
}

/// A unit that landed everything §8.3 asks of a policy.
#[test]
fn a_fully_confined_unit_has_no_shortfall() {
    let observed: Vec<(String, String)> = launch::confinement_properties(PackageKind::Policy)
        .iter()
        .map(|line| {
            let (key, value) = line.split_once('=').expect("every directive is Key=Value");
            (key.to_string(), value.to_string())
        })
        .collect();
    assert!(launch::confinement_shortfall(PackageKind::Policy, &observed).is_empty());
    // A driver asks for none, so it cannot fall short — §8.3 makes it native by design, and a
    // driver reported as failing confinement would be this robot failing itself for obeying the
    // specification.
    assert!(launch::confinement_shortfall(PackageKind::Driver, &observed).is_empty());
}

/// The comparison that must be line-exact, and the reason it must be.
///
/// A unit holding `CapabilityBoundingSet=cap_sys_admin` has the key the launcher asked for and
/// none of the property. A shortfall computed on keys would report this unit as fully confined,
/// which is the single most misleading result this check could produce.
#[test]
fn a_capability_set_that_is_not_empty_is_a_shortfall() {
    let mut observed: Vec<(String, String)> = launch::confinement_properties(PackageKind::Policy)
        .iter()
        .map(|line| {
            let (key, value) = line.split_once('=').expect("Key=Value");
            (key.to_string(), value.to_string())
        })
        .collect();
    for entry in &mut observed {
        if entry.0 == "CapabilityBoundingSet" {
            entry.1 = "cap_sys_admin".to_string();
        }
    }
    assert_eq!(
        launch::confinement_shortfall(PackageKind::Policy, &observed),
        vec!["CapabilityBoundingSet=".to_string()]
    );
}

/// A directive systemd did not report counts as not landed.
///
/// Fail-closed, and the direction matters more here than almost anywhere: an unreported directive
/// is one this code cannot show was applied, and a sandbox nobody can read is not a sandbox
/// anybody should certify.
#[test]
fn an_unreported_directive_is_a_shortfall() {
    let shortfall = launch::confinement_shortfall(PackageKind::Policy, &[]);
    assert_eq!(
        shortfall.len(),
        launch::confinement_properties(PackageKind::Policy).len(),
        "a unit systemd said nothing about has landed nothing this code can show"
    );
}

/// The query and the comparison are one list, so a new directive cannot be half-added.
///
/// Adding a directive to `confinement_properties` and forgetting the `systemctl show -p` list
/// would make it come back unreported, which the shortfall counts as not landed — a robot failing
/// its own isolation test over a forgotten string, naming a directive that was in fact applied.
#[test]
fn every_directive_asked_for_is_a_directive_asked_about() {
    let names = launch::confinement_property_names();
    for kind in [
        PackageKind::Policy,
        PackageKind::Service,
        PackageKind::Skill,
        PackageKind::Driver,
    ] {
        for property in launch::confinement_properties(kind) {
            let key = property.split('=').next().expect("Key=Value").to_string();
            assert!(
                names.contains(&key),
                "{key} is asked of a unit and never asked about"
            );
        }
    }
    // And nothing else: a name in the query that no kind asks for would be a property read and
    // never compared, which is cost without an assertion.
    assert_eq!(
        names.len(),
        launch::confinement_properties(PackageKind::Policy).len()
    );
}

/// A launcher that records rather than launching has nothing to read, and says so.
///
/// The alternative — reporting an empty confinement — would make `--dry-run` fail a safety
/// conformance test on a machine it never touched.
#[test]
fn a_recording_launcher_refuses_to_describe_a_sandbox() {
    let launcher = launch::RecordingLauncher::new();
    let error = launch::Launcher::confinement(&launcher, "openrobot/holder")
        .expect_err("nothing was launched");
    assert!(error.detail.contains("records rather than launching"));
}

/// What `systemctl show` really prints for a unit it has never heard of.
///
/// Copied from a run rather than invented, because the whole hazard is that it looks like an
/// answer: every directive present, every one of them a refusal, and exit status 0.
const SHOW_UNIT_NOT_FOUND: &str = "AllowedCPUs=\n\
     MemoryMax=infinity\n\
     TasksMax=37039\n\
     CapabilityBoundingSet=cap_chown cap_dac_override cap_sys_admin\n\
     ProtectKernelTunables=no\n\
     ProtectKernelModules=no\n\
     ProtectControlGroups=no\n\
     ProtectSystem=no\n\
     NoNewPrivileges=no\n\
     RestrictSUIDSGID=no\n\
     LoadState=not-found\n";

/// A unit systemd has never heard of is refused, not read as a sandbox that landed nothing.
///
/// The two are opposite facts and `systemctl show` gives them the same exit status, so the load
/// state is the only thing that separates them. Read as a shortfall it fails
/// `safety.policy.isolation` and names seven directives that were in fact applied — to a unit that
/// was collected when it went inactive, which is the ordinary end of a transient unit.
#[test]
fn a_unit_systemd_has_never_heard_of_is_not_a_sandbox_that_failed() {
    let error = launch::confinement_from_show(SHOW_UNIT_NOT_FOUND)
        .expect_err("there is no unit to describe");
    assert!(
        error.contains("no unit by that name"),
        "the refusal must say there is no unit, not that a directive is missing: {error}"
    );

    // What the old reading did with the same bytes, kept as an assertion so the fix cannot be
    // undone quietly: every directive §8.3 asks for, reported as not landed.
    let misread = launch::parse_properties(SHOW_UNIT_NOT_FOUND);
    assert_eq!(
        launch::confinement_shortfall(PackageKind::Policy, &misread).len(),
        launch::confinement_properties(PackageKind::Policy).len(),
        "this is the failure the load state prevents, and it has to stay demonstrable"
    );
}

/// A loaded unit answers with its directives, and the load state is not one of them.
///
/// Stripped rather than passed through: `confinement_shortfall` compares against
/// `confinement_properties`, and a pair that is not a directive would be a property read and never
/// compared — which `every_directive_asked_for_is_a_directive_asked_about` forbids for the query
/// and this keeps true of the answer.
#[test]
fn a_loaded_unit_answers_with_its_directives_alone() {
    let text =
        "NoNewPrivileges=yes\nCapabilityBoundingSet=\nLoadState=loaded\nMemoryMax=67108864\n";
    let observed = launch::confinement_from_show(text).expect("the unit is loaded");
    assert_eq!(
        observed,
        vec![
            ("NoNewPrivileges".to_string(), "yes".to_string()),
            ("CapabilityBoundingSet".to_string(), String::new()),
            ("MemoryMax".to_string(), "67108864".to_string()),
        ]
    );
}

/// Any load state but `loaded` is a refusal that names it, and a missing one fails closed.
///
/// `masked` and `error` are not `not-found` and an operator sent to the wrong one of the three has
/// been told the wrong thing — the same reason §27 keeps `unreported` apart from `pending`.
#[test]
fn a_unit_that_is_not_loaded_says_which_it_is() {
    let masked = launch::confinement_from_show("NoNewPrivileges=no\nLoadState=masked\n")
        .expect_err("a masked unit describes nothing");
    assert!(masked.contains("masked"), "{masked}");

    let silent = launch::confinement_from_show("NoNewPrivileges=yes\n")
        .expect_err("no load state, so no way to know what was described");
    assert!(silent.contains("LoadState"), "{silent}");
}

/// The restraint, pinned: none of these may appear without the design that makes them safe —
/// a path-dependent directive against an unmeasured store layout, a device policy with no
/// declared device set (§40.3), or a filter with no measured allow-list (§64).
#[test]
fn confinement_touches_no_path_and_invents_no_allow_list() {
    for kind in [
        PackageKind::Policy,
        PackageKind::Service,
        PackageKind::Skill,
    ] {
        let properties = launch::confinement_properties(kind);
        for property in &properties {
            // `ProtectSystem=strict` in full rather than `strict`: the substring sits inside
            // `RestrictSUIDSGID`, which is confinement, not the fenced store this forbids.
            for forbidden in [
                "PrivateTmp",
                "ProtectHome",
                "DevicePolicy",
                "DeviceAllow",
                "ProtectSystem=strict",
                "ReadWritePaths",
                "AddressFamilies",
                "SystemCallFilter",
            ] {
                assert!(
                    !property.contains(forbidden),
                    "{property} reaches for {forbidden}, which has no measured basis yet"
                );
            }
        }
    }
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
    /// A recorder, when a test wants the routes that need one to answer with something.
    ///
    /// `None` by default and for almost every test: the suite exercises the API without a
    /// recorder, the routes that need one answer 503, and the rest do not care. The console
    /// fixture is the exception, and the reason is worth stating — a fixture whose black box
    /// and events panels can only ever render *unavailable* is a fixture in which nobody has ever
    /// looked at either panel, which is the one thing a fixture exists to make possible.
    recorder: Option<crate::blackbox::Recorder>,
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
        // §36's sensors, pointed at a tree that does not exist. A daemon left on the real `/sys`
        // would read *this build machine's* battery, so a laptop and a container would produce
        // different check-in documents from the same fixture — and the suite would pass on one and
        // fail on the other for a reason nothing in it mentions. The tests that exercise a gauge
        // build one, in `crate::power`'s own module.
        daemon.power = crate::power::Sensors::rooted(
            root.path().join("no-power-supply"),
            root.path().join("no-thermal"),
        );
        Self {
            root,
            daemon,
            credentials: Credentials::empty(),
            recorder: None,
        }
    }

    /// Open a recorder over this fixture's own layout, so §13's routes answer.
    fn record(&mut self) -> &mut crate::blackbox::Recorder {
        let layout = self.daemon.store.layout().clone();
        self.recorder.get_or_insert_with(|| {
            crate::blackbox::Recorder::open(
                &layout,
                crate::blackbox::supervisor_meta(&layout),
                skynet_blackbox::record::Shape {
                    joints: 0,
                    channels: 0,
                },
                64 * 1024,
            )
            .expect("a recorder over the fixture's layout")
        })
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
            // See `Fixture::recorder`: `None` for almost every test, and the console fixture
            // opens one so §13's panels have something to draw.
            blackbox: self.recorder.as_mut(),
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
            blackbox: self.recorder.as_mut(),
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

/// Every route the console fixture records an answer for.
///
/// A constant rather than a literal in the loop, because [`every_console_feed_has_a_fixture_route`]
/// checks it against the page. A feed the fixture does not record renders as a 404 from
/// `fixture-serve.mjs` — which the console correctly draws as an unreachable panel, so the omission
/// looks exactly like the failure the fixture exists to let somebody look at.
const CONSOLE_FIXTURE_ROUTES: &[&str] = &[
    "status",
    "embodiment",
    "limits",
    // §32.2's Plan panel asks for one package's plan, so the fixture records one. The console's
    // picker asks for whichever package the operator chose; this is the one the fixture installs.
    "limits?package=openrobot%2Fgrasp",
    "plan",
    "packages",
    "health",
    "blackbox",
    // The limit is part of the request, exactly as the package is for `limits` above: the page asks
    // for a hundred and the fixture has to answer the request the page makes, not a similar one.
    "blackbox/events?limit=100",
    "capacity",
    "grants",
    "trust",
    "audit",
    "calibration",
    "cell",
    "outbound",
    "watchdog",
    "post",
    "state",
    "skills",
    "ota",
    "releases",
    "identity",
    "checkin",
    "intent",
];

/// The file one request's answer is recorded in.
///
/// **The query is part of the request.** `/limits` and `/limits?package=openrobot/grasp` are two
/// different answers — the body's own ceilings, and those with a package's transport clamp folded
/// in — and a fixture that recorded one under both names would show the first where the console
/// asked for the second. `console/fixture-serve.mjs` computes the same name; the two are one rule
/// in two languages, and the rule is written where the server reads it.
fn fixture_name(route: &str) -> String {
    // **The path is flattened too, and it has to be.** `/blackbox` and `/blackbox/events` are both
    // routes, so a scheme that kept the separator would need `blackbox` to be a file and a
    // directory at once — which is not a subtle failure, it is `AlreadyExists` on the second write,
    // and it arrives the first time anybody adds a route under an existing one. Flat names have no
    // such shape.
    let path_of = |path: &str| path.replace('/', "-");
    let Some((path, query)) = route.split_once('?') else {
        return path_of(route);
    };
    let path = path_of(path);
    let flattened: String = query
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    // Runs of non-alphanumerics collapse, matching the server's `[^A-Za-z0-9]+`.
    let mut collapsed = String::with_capacity(flattened.len());
    for ch in flattened.chars() {
        if ch == '-' && collapsed.ends_with('-') {
            continue;
        }
        collapsed.push(ch);
    }
    format!("{path}__{collapsed}")
}

/// The fixture records an answer for every feed the page polls.
///
/// Two lists in two languages, kept in step by reading one of them. `console.js` is compiled into
/// this binary by `include_str!`, so the page under test is the page that ships — and the check is
/// a scan for `new Feed('name', '/path'`, which is the one place a feed is declared.
///
/// Not `--ignored` like the fixture itself: writing the fixture needs a temporary directory and a
/// human to look at the result, and *this* needs neither. The failure it catches is a panel added
/// to the console and silently missing from every fixture anybody renders afterwards, which is a
/// gap nobody notices because the page still draws.
#[test]
fn every_console_feed_has_a_fixture_route() {
    let script = include_str!("../console/console.js");
    let mut declared: Vec<String> = Vec::new();
    for tail in script.split("new Feed(").skip(1) {
        let Some(path) = tail.split('\'').nth(3) else {
            continue;
        };
        let Some(route) = path.strip_prefix('/') else {
            panic!("a feed path that is not absolute: `{path}`");
        };
        declared.push(route.to_string());
    }
    assert!(
        declared.len() > 15,
        "the scan found {} feeds, which means `new Feed(` stopped being how one is declared",
        declared.len()
    );
    for route in &declared {
        assert!(
            CONSOLE_FIXTURE_ROUTES.contains(&route.as_str()),
            "the console polls `/{route}` and the fixture records no answer for it, so anybody \
             rendering the fixture sees that panel unreachable rather than drawn"
        );
    }
}

/// Write the console and one answer per route it fetches, so a browser can render it.
///
/// `cargo test -p skynetd-io -- --ignored console_fixture --nocapture`, then serve the directory it
/// names **with the `fixture-serve.mjs` it writes beside the page**, which the run prints the command
/// for. Not a plain static server: the fixture records one `<route>.status` per route and a static
/// server has no idea that file is a status code, so a 503 replays as a 200 and the page renders an
/// error object as data — `undefined records of undefined B` where the robot said service unavailable,
/// which is the §32.2 violation the console is built not to commit. That is not hypothetical; it is
/// what a static server drew the first time somebody looked, and `console/fixture-serve.mjs` carries
/// the account. §32.2's point survives it: the server is thirty lines of Node standard library and the
/// page itself still needs nothing.
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
    fixture.daemon.refresh_operating_state(
        None,
        &crate::observed::Sightings {
            kernel: &blackbox::KernelSighting::Unobservable("no ring in this fixture"),
            rhal: &blackbox::RhalSighting::Unobservable("no ring in this fixture"),
            stop: &blackbox::StopSighting::Unobservable("no ring in this fixture"),
            hold: &blackbox::HoldSighting::Unobservable("no ring in this fixture"),
            guards: &crate::limits::GuardsSighting::Unobservable("no descriptor in this fixture"),
            policy: &crate::shape::PolicySighting::Nothing { no_body: true },
            homing: &blackbox::HomingSighting::Unobservable("no ring in this fixture"),
        },
    );
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
    let mut m = manifest("openrobot/grasp", "1.0.0", "policy");
    // §37.1's `transport` stage needs the body this policy was trained on. Without it the console's
    // Plan panel renders the honest-but-uninteresting "this package commands no joints", and the
    // shape §32.2 asks the panel for — bindings, a method per channel, a ceiling with provenance,
    // and a `scaled_down` warning naming the joint — goes unlooked-at.
    // The digest a publisher declares over its own version. Separate from the digest this store
    // observes over the blob — admission compares the two and the planner reads neither.
    m.set(
        "digest",
        Json::from(format!("sha256:{}", "1".repeat(64)).as_str()),
    );
    m.set(
        "source_embodiment",
        Json::parse(
            r#"{"control_mode":"torque","control_hz":50,
                 "action_channels":[{"index":0,"kind":"joint_torque",
                                     "role":"right_arm.shoulder.pitch","unit":"Nm","peak":180}]}"#,
        )
        .expect("the source parses"),
    );
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

    // §13's recording, so the two panels that need one have something to draw.
    //
    // Until this the fixture had no recorder at all, so `blackbox` and `events` both answered 503
    // and every look at this page saw the same two grey boxes. That is a legitimate state and it
    // is *one* of them; a fixture that can only produce it is a fixture in which the interesting
    // half of both panels has never been on screen.
    //
    // What goes in is chosen the way everything else here is — one of each case the panel has to
    // keep apart. A grant, an install, a package starting and stopping, a seal, and **a gap in the
    // middle of them**, because two events either side of four hundred lost records read as
    // consecutive and the panel's whole job is to stop that.
    {
        use skynet_blackbox::record::{EventCode, Gap, GapReason};
        let recorder = fixture.record();
        for (mono, code, subject, detail) in [
            (
                1_000_u64,
                EventCode::RecordingStarted,
                "skynetd",
                "ring opened at 64 KiB",
            ),
            (
                2_000,
                EventCode::Admission,
                "openrobot/grasp",
                "admitted: signature verified",
            ),
            (
                3_000,
                EventCode::GrantChanged,
                "camera.front",
                "granted to openrobot/grasp",
            ),
            (
                4_000,
                EventCode::PackageStarted,
                "openrobot/grasp",
                "unit started",
            ),
        ] {
            recorder.event(mono, code, subject, detail).unwrap();
        }
        recorder
            .gap(
                5_000,
                Gap {
                    dropped: 412,
                    first_tick: 9_100,
                    reason: GapReason::WriterBehind,
                },
            )
            .unwrap();
        for (mono, code, subject, detail) in [
            (
                6_000_u64,
                EventCode::PackageStopped,
                "openrobot/grasp",
                "unit exited 0",
            ),
            (
                7_000,
                EventCode::Sealed,
                "operator",
                "sealed through the local API",
            ),
        ] {
            recorder.event(mono, code, subject, detail).unwrap();
        }
    }

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
    for route in CONSOLE_FIXTURE_ROUTES {
        let response = fixture.raw("GET", &format!("/{route}"), Some("observer-token"));
        let name = fixture_name(route);
        std::fs::write(out.join(&name), &response.body).unwrap();
        std::fs::write(
            out.join(format!("{name}.status")),
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

const POWERED_ARM: &str = r#"{
  "robot_slug": "bench-arm",
  "base": "fixed",
  "dof": 1,
  "power": {
    "time_to_safe_state_s": 2,
    "safe_stop_energy_wh": 20,
    "pack_capacity_wh": 500,
    "thermal_throttle_c": 70,
    "thermal_critical_c": 90,
    "brownout_fault_codes": [4097]
  },
  "joints": [{"name": "shoulder", "type": "revolute", "control_modes": ["position"]}]
}"#;

/// The same body, declaring a pack and **no** `brownout_fault_codes`.
///
/// §36.4: an empty list is *nobody has said*, which is a different answer from a list that does not
/// contain the code that arrived. `POWERED_ARM` declares `[4097]` and therefore exercises the
/// second; this one exercises the first, which is the case that raises a fault.
const POWERED_ARM_NO_CODES: &str = r#"{
  "schema": 1,
  "robot_id": "arm-1",
  "power": {
    "pack_capacity_wh": 500,
    "reserve_fraction": 0.15,
    "thermal_throttle_c": 70
  },
  "joints": [{"name": "shoulder", "type": "revolute", "control_modes": ["position"]}]
}"#;

/// Put a §27 `rhal` sighting on the daemon, as a reconcile pass would have.
///
/// The routes read `daemon.rhal_sighting` rather than the ring, because §36.4's bus reading and
/// §27's condition have to be one reading of one instant — so a test that wrote cycles into a ring
/// would be exercising `RhalSighting::of`, which has its own suite, rather than the wiring.
fn set_rhal_sighting(fixture: &mut Fixture, sighting: blackbox::RhalSighting) {
    fixture.daemon.rhal_sighting = sighting;
}

/// A machine with a battery, under the fixture's own root.
fn with_gauge(fixture: &mut Fixture, files: &[(&str, &str)], zones: &[&str]) {
    let supplies = fixture.root.path().join("power_supply");
    let thermal = fixture.root.path().join("thermal");
    let bat = supplies.join("BAT0");
    std::fs::create_dir_all(&bat).unwrap();
    std::fs::write(bat.join("type"), "Battery").unwrap();
    for (name, value) in files {
        std::fs::write(bat.join(name), value).unwrap();
    }
    for (index, milli) in zones.iter().enumerate() {
        let zone = thermal.join(format!("thermal_zone{index}"));
        std::fs::create_dir_all(&zone).unwrap();
        std::fs::write(zone.join("temp"), milli).unwrap();
    }
    fixture.daemon.power = crate::power::Sensors::rooted(supplies, thermal);
}

/// §36 end to end: a descriptor, a gauge in `/sys`, and the state the two produce.
#[test]
fn the_power_route_assesses_a_real_gauge_against_the_declared_reserve() {
    let mut fixture = Fixture::new("power-route");
    fixture.provision();
    write_embodiment(&fixture, POWERED_ARM);
    // 40 Wh against a 20 Wh reserve: inside the low band, where a running task may finish and no
    // new one may start.
    with_gauge(&mut fixture, &[("energy_now", "40000000")], &["55000"]);

    let (status, body) = fixture.request("GET", "/power", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.get("state").and_then(Json::as_str), Some("low"));
    assert_eq!(
        body.get("reason").and_then(Json::as_str),
        Some("approaching_reserve")
    );
    assert_eq!(body.get("remaining_wh").and_then(Json::as_f64), Some(40.0));
    assert_eq!(
        body.get("grants_new_leases").and_then(Json::as_bool),
        Some(false)
    );
    assert_eq!(
        body.get("requires_safe_state").and_then(Json::as_bool),
        Some(false),
        "§36.1's low row: the running task may finish"
    );
    assert_eq!(body.get("supply").and_then(Json::as_str), Some("BAT0"));
    assert_eq!(body.get("zones").and_then(Json::as_f64), Some(1.0));
    assert_eq!(body.get("thermal").and_then(Json::as_str), Some("nominal"));
    assert!(body.arr_or_empty("shed").is_empty());
    assert!(body.arr_or_empty("faults").is_empty());
    assert!(body.arr_or_empty("notes").is_empty());
    assert_eq!(body.get("declared").and_then(Json::as_bool), Some(true));

    // The reserve is a floor, and the route says which terms are missing rather than presenting
    // 20 Wh as though §36.2's three had all been accounted for.
    let reserve = body.get("reserve").cloned().expect("a reserve");
    assert_eq!(reserve.get("wh").and_then(Json::as_f64), Some(20.0));
    assert_eq!(
        reserve.str_arr("unaccounted"),
        vec!["payload", "distance_to_safe_state"]
    );
    assert_eq!(reserve.get("complete").and_then(Json::as_bool), Some(false));
}

/// The reading is one thing and the assessment is another, and the route keeps them apart.
#[test]
fn a_body_that_declares_a_pack_and_has_no_gauge_reports_both_halves() {
    let mut fixture = Fixture::new("power-route-silent");
    fixture.provision();
    write_embodiment(&fixture, POWERED_ARM);

    let (_, body) = fixture.request("GET", "/power", Some("observer-token"), "");
    assert_eq!(body.get("state").and_then(Json::as_str), Some("unmetered"));
    assert_eq!(
        body.get("reason").and_then(Json::as_str),
        Some("no_reading")
    );
    assert_eq!(
        body.get("grants_new_leases").and_then(Json::as_bool),
        Some(false)
    );
    // The contract's half.
    let codes: Vec<&str> = body
        .arr_or_empty("faults")
        .iter()
        .filter_map(|fault| fault.get("code").and_then(Json::as_str))
        .collect();
    assert_eq!(codes, ["gauge_silent"]);
    // The reader's half, which is the one that says *where* to go and look.
    assert!(
        !body.arr_or_empty("notes").is_empty(),
        "a fault says §36 has a problem; a note says this daemon could not read a number"
    );
}

/// §36.3, end to end. The ladder is cut three rungs deep and the kernel is not on it.
#[test]
fn a_throttling_body_sheds_three_rungs_and_never_the_kernel() {
    let mut fixture = Fixture::new("power-route-hot");
    fixture.provision();
    write_embodiment(&fixture, POWERED_ARM);
    with_gauge(
        &mut fixture,
        &[("energy_now", "400000000")],
        &["42000", "78000"],
    );

    let (_, body) = fixture.request("GET", "/power", Some("observer-token"), "");
    assert_eq!(body.get("state").and_then(Json::as_str), Some("normal"));
    assert_eq!(
        body.get("thermal").and_then(Json::as_str),
        Some("throttling")
    );
    assert_eq!(
        body.str_arr("shed"),
        vec!["telemetry", "ota", "blackbox_compression"]
    );
    for never in ["rhal", "kernel", "control_loop"] {
        assert!(
            !body.str_arr("shed").contains(&never.to_string()),
            "{never}"
        );
    }
    assert_eq!(
        body.get("degraded").and_then(Json::as_bool),
        Some(false),
        "the first reading above the threshold is not sustained throttling"
    );
}

/// §36.4 on the route, in all three of the states the bus can be in.
///
/// This test used to assert the *reason* §36.4 had no input — that §33.1 kept the daemon off the
/// RHAL — and it passed for as long as that reason went unexamined. The premise was true and the
/// conclusion was not: §26's cycle record carries `state_flags` and `fault_code` verbatim, and
/// `RhalSighting` was already reading both for §27's `rhal` condition. So the note is a reading
/// now, and what this asserts is that the two kinds of `normal` are told apart.
///
/// | The bus | `state` | `bus_observed` | What `normal` would mean |
/// |---|---|---|---|
/// | nothing read it | `normal` | `false` | the gauge agrees; nobody asked the bus |
/// | read, healthy | `normal` | `true` | both |
/// | read, degraded, code declared | `critical` | `true` | — |
#[test]
fn the_power_route_reports_what_the_bus_said_in_each_of_its_three_states() {
    let mut fixture = Fixture::new("power-route-bus");
    fixture.provision();
    write_embodiment(&fixture, POWERED_ARM);
    with_gauge(&mut fixture, &[("energy_now", "400000000")], &[]);

    // 1. Nothing has read the bus, which is every robot whose ring holds no control cycles
    //    (§26.2). Still `normal`, and the route says the word is carrying less than it looks.
    let (_, unobserved) = fixture.request("GET", "/power", Some("observer-token"), "");
    assert_eq!(
        unobserved.get("state").and_then(Json::as_str),
        Some("normal")
    );
    assert_eq!(
        unobserved.get("bus_observed").and_then(Json::as_bool),
        Some(false)
    );
    assert!(unobserved
        .get("brownout_note")
        .and_then(Json::as_str)
        .is_some_and(|note| note.contains("nothing has read the bus")));

    // 2. A cycle read the RHAL and it was answering. Same state, and now the word means both.
    set_rhal_sighting(
        &mut fixture,
        blackbox::RhalSighting::Seen {
            state: skynet_contracts::opstate::RhalState::Present,
            errno: None,
            fault_code: 0,
        },
    );
    let (_, healthy) = fixture.request("GET", "/power", Some("observer-token"), "");
    assert_eq!(healthy.get("state").and_then(Json::as_str), Some("normal"));
    assert_eq!(
        healthy.get("bus_observed").and_then(Json::as_bool),
        Some(true)
    );
    assert!(healthy
        .get("brownout_note")
        .and_then(Json::as_str)
        .is_some_and(|note| note.contains("not degraded")));

    // 3. `RHAL_DEGRADED` carrying a code the descriptor declares as sag. §36.4's override reaches
    //    `critical` past a gauge reporting a healthy pack — which is the entire point of the rule,
    //    and the first time any robot on this edition could produce it.
    set_rhal_sighting(
        &mut fixture,
        blackbox::RhalSighting::Seen {
            state: skynet_contracts::opstate::RhalState::Degraded,
            errno: None,
            fault_code: 4097,
        },
    );
    let (_, sagging) = fixture.request("GET", "/power", Some("observer-token"), "");
    assert_eq!(
        sagging.get("state").and_then(Json::as_str),
        Some("critical"),
        "a gauge that disagrees with the bus is a gauge that is wrong (§36.4)"
    );
    assert_eq!(
        sagging.get("reason").and_then(Json::as_str),
        Some("brownout")
    );
    assert_eq!(
        sagging.get("bus_observed").and_then(Json::as_bool),
        Some(true)
    );
}

/// A degraded bus whose code nobody declared is a fault, not a brownout and not silence.
///
/// §36.4: *"an empty list is not 'this body cannot brown out'. It is nobody has said."* The robot
/// keeps working — a hole in a vendor's declaration is a reason to say so, not a reason to stop a
/// machine that is running — and the fault appears at the moment it matters, which is when the RHAL
/// is actually degraded and nothing on the robot can say why.
///
/// This is the answer the old module header said an operator *should* be seeing and could not,
/// because nothing supplied the flag. It is reachable now.
#[test]
fn a_degraded_bus_with_no_declared_codes_raises_brownout_undeclarable() {
    let mut fixture = Fixture::new("power-route-undeclarable");
    fixture.provision();
    write_embodiment(&fixture, POWERED_ARM_NO_CODES);
    with_gauge(&mut fixture, &[("energy_now", "400000000")], &[]);
    set_rhal_sighting(
        &mut fixture,
        blackbox::RhalSighting::Seen {
            state: skynet_contracts::opstate::RhalState::Degraded,
            errno: None,
            fault_code: 9001,
        },
    );

    let (_, body) = fixture.request("GET", "/power", Some("observer-token"), "");
    assert!(
        body.arr_or_empty("faults").iter().any(|fault| {
            fault.get("code").and_then(Json::as_str) == Some("brownout_undeclarable")
        }),
        "a degraded bus nobody can explain is a fault an operator should be seeing"
    );
    assert_ne!(
        body.get("reason").and_then(Json::as_str),
        Some("brownout"),
        "an undeclared code is not a sag — it is the dead encoder that flag also covers"
    );
    assert_eq!(
        body.get("bus_observed").and_then(Json::as_bool),
        Some(true),
        "the bus was read; what is missing is the vendor's list, not the reading"
    );
}

/// A faulted or lost RHAL is `Unobserved`, and deliberately not `Healthy`.
///
/// The one mapping decision in `BusReading`. §36.4 asks whether the bus is sagging, and a driver
/// that has stopped answering has not said that it is not — reporting the reassuring half of a
/// fact nobody has would be the failure this whole module is arranged against.
#[test]
fn a_bus_nobody_can_read_is_not_a_healthy_bus() {
    use crate::power::BusReading;

    assert_eq!(
        BusReading::of(&blackbox::RhalSighting::Silent),
        BusReading::Unobserved
    );
    assert_eq!(
        BusReading::of(&blackbox::RhalSighting::Unobservable("no ring")),
        BusReading::Unobserved
    );
    // Lost, and carrying a fault code. Neither the flag nor the code is taken.
    assert_eq!(
        BusReading::of(&blackbox::RhalSighting::Seen {
            state: skynet_contracts::opstate::RhalState::Lost,
            errno: Some(-110),
            fault_code: 4097,
        }),
        BusReading::Unobserved
    );

    // And the two that are readings.
    assert_eq!(
        BusReading::of(&blackbox::RhalSighting::Seen {
            state: skynet_contracts::opstate::RhalState::Present,
            errno: None,
            fault_code: 0,
        }),
        BusReading::Healthy
    );
    assert_eq!(
        BusReading::of(&blackbox::RhalSighting::Seen {
            state: skynet_contracts::opstate::RhalState::Degraded,
            errno: None,
            fault_code: 4097,
        }),
        BusReading::Degraded { fault_code: 4097 }
    );

    // The pair travels together: a degraded bus always carries whatever code it had, so
    // `brownout_undeclarable` is never produced by this module losing the number.
    assert_eq!(
        BusReading::Degraded { fault_code: 4097 }.fault_code(),
        Some(4097)
    );
    assert_eq!(BusReading::Healthy.fault_code(), None);
    assert!(!BusReading::Unobserved.observed());
}

#[test]
fn a_body_that_declares_no_power_block_is_unmetered_without_a_fault() {
    let mut fixture = Fixture::new("power-route-bare");
    fixture.provision();
    write_embodiment(&fixture, NESTED_ARM);

    let (_, body) = fixture.request("GET", "/power", Some("observer-token"), "");
    assert_eq!(body.get("state").and_then(Json::as_str), Some("unmetered"));
    assert_eq!(body.get("reason").and_then(Json::as_str), Some("no_pack"));
    assert_eq!(body.get("declared").and_then(Json::as_bool), Some(false));
    assert_eq!(
        body.get("grants_new_leases").and_then(Json::as_bool),
        Some(true),
        "a body that declares no pack is a mains machine behaving correctly"
    );
    assert!(body.arr_or_empty("faults").is_empty());
}

/// §7.3's limits, over the real router.
///
/// The route is the first thing on a robot that calls `resolve_limits`, and what it answers is a
/// composition of three files this fixture writes: the descriptor's envelope, the signed profile's
/// ceilings and §55.2's cell. Asserting the composition rather than the fields, because a route
/// that read one file and rendered it would pass a field-by-field test.
#[test]
fn the_limits_route_composes_the_body_the_profile_and_the_cell() {
    let mut fixture = Fixture::new("limits-route");
    fixture.provision();
    write_embodiment(&fixture, LIMITED_ARM);
    write_profile(&fixture, SITE_PROFILE);

    let (status, body) = fixture.request("GET", "/limits", Some("observer-token"), "");
    assert_eq!(status, 200);

    // The profile narrows the envelope's velocity and the envelope keeps its torque: the minimum
    // of every source, which is the one rule §7.3 has.
    assert_eq!(
        body.get("joint_velocity_max_rad_s").and_then(Json::as_f64),
        Some(0.5)
    );
    assert_eq!(
        body.get("joint_torque_max_nm").and_then(Json::as_f64),
        Some(80.0)
    );
    assert_eq!(
        body.get("source").and_then(Json::as_str),
        Some("vendor datasheet rev C"),
        "a safety limit without provenance is a rumour"
    );

    // The cell's volumes reached the guard that reads them.
    let active: Vec<&str> = body
        .arr_or_empty("active_guards")
        .iter()
        .filter_map(Json::as_str)
        .collect();
    assert!(active.contains(&"workspace"), "{active:?}");

    // And the route never claims any of it is enforced.
    assert_eq!(body.get("enforced"), Some(&Json::Bool(false)));
    assert!(body
        .get("tier_source")
        .and_then(Json::as_str)
        .is_some_and(|note| note.contains("Nothing on this robot decides its tier")));
}

/// A named package that is not installed is refused by name rather than answered with the body's
/// own ceilings, which a caller would read as the package's.
#[test]
fn the_limits_route_refuses_a_package_it_does_not_have() {
    let mut fixture = Fixture::new("limits-package");
    fixture.provision();
    write_embodiment(&fixture, LIMITED_ARM);

    let (status, body) = fixture.request(
        "GET",
        "/limits?package=acme%2Fwalk",
        Some("observer-token"),
        "",
    );
    assert_eq!(status, 200);
    assert!(body
        .get("package_clamp")
        .and_then(Json::as_str)
        .is_some_and(|note| note.contains("neither installed nor staged")));
    assert_eq!(
        body.get("sources")
            .and_then(|sources| sources.get("transport_of")),
        Some(&Json::Null)
    );
}

/// §6.3's hand-off, and §32.2's Plan panel, over the real router.
///
/// `?package=` folds §37.1's transport clamp into the resolution and answers with the plan it came
/// from. The two halves are different questions and the panel asks both: the clamp is the number a
/// guard would enforce, and the plan is why that number is what it is — which channel bound to
/// which joint, by what method, and whether anything was scaled down on the way.
#[test]
fn the_limits_route_folds_in_an_installed_packages_plan() {
    let mut fixture = Fixture::new("limits-plan");
    fixture.provision();
    write_embodiment(&fixture, LIMITED_ARM);

    // A policy that commands one torque channel on the role this arm declares, peaking well above
    // what the joint is rated for — so the plan has to scale it and say so.
    let source = Json::parse(
        r#"{"control_mode":"torque","control_hz":50,
             "action_channels":[{"index":0,"kind":"joint_torque","role":"right_arm.shoulder.pitch",
                                 "unit":"Nm","peak":180}]}"#,
    )
    .expect("the source parses");
    let mut document = manifest("openrobot/lift", "1.0.0", "policy");
    // The digest a publisher declares over its own version, which is a different number from the
    // one this store observes over the blob — admission compares them and the planner does not.
    document
        .set(
            "digest",
            Json::from(format!("sha256:{}", "1".repeat(64)).as_str()),
        )
        .set("source_embodiment", source);

    let dir = fixture.root.path().join("lift");
    let digest = write_package(&dir, &document, &[("bin/run", "#!/bin/sh\nsleep 1\n")]);
    let id = PackageId::parse("openrobot/lift").unwrap();
    fixture
        .daemon
        .store
        .stage(&id, "policy", "1.0.0", &digest, &dir)
        .unwrap();
    // Staged and not activated, which is the state §6.2 says this answer is most wanted in — and
    // the state a developer's machine can actually reach: §37.1's activation is an atomic symlink
    // swap, which is POSIX, and a test that needed one would be green in CI and unrunnable here.
    let (status, body) = fixture.request(
        "GET",
        "/limits?package=openrobot%2Flift",
        Some("observer-token"),
        "",
    );
    assert_eq!(status, 200);
    assert_eq!(body.get("package_clamp"), Some(&Json::Null), "no refusal");

    // The plan, which is the half no ceiling carries.
    let plan = body.get("plan").expect("a plan");
    assert_eq!(plan.get("verdict").and_then(Json::as_str), Some("direct"));
    // §6.1: scaling never guesses, and when it scales it names the joint. 80 of 180 is 0.44, and a
    // panel that showed only the resulting ceiling would not say the policy had been turned down.
    let warnings: Vec<&str> = plan
        .arr_or_empty("warnings")
        .iter()
        .filter_map(|entry| entry.get("code").and_then(Json::as_str))
        .collect();
    assert!(warnings.contains(&"scaled_down"), "{warnings:?}");

    // And the clamp reached `resolve_limits`, which is §6.3's hand-off: the plan's per-channel
    // ceiling is what gives a joint a limit the `envelope` guard can compare a command against.
    let per_channel = body.get("per_channel").expect("per-channel ceilings");
    assert_eq!(
        per_channel.get("shoulder").and_then(Json::as_f64),
        Some(80.0)
    );
    assert_eq!(
        body.get("sources")
            .and_then(|sources| sources.get("transport_of"))
            .and_then(Json::as_str),
        Some("openrobot/lift")
    );
    // The sentence that was true of every robot until a plan was in force stops being printed once
    // one is: this robot now has a ceiling to clamp against.
    assert_eq!(body.get("per_channel_note"), None);
    assert_eq!(
        body.get("package_state").and_then(Json::as_str),
        Some("staged"),
        "a plan for a package that is not running is a different sentence, and the route says which"
    );
}

/// A read that does work is still a read.
#[test]
fn reading_the_limits_changes_nothing() {
    let mut fixture = Fixture::new("limits-pure");
    fixture.provision();
    write_embodiment(&fixture, LIMITED_ARM);
    let before = fixture
        .request("GET", "/status", Some("observer-token"), "")
        .1;
    fixture.request("GET", "/limits", Some("observer-token"), "");
    let after = fixture
        .request("GET", "/status", Some("observer-token"), "")
        .1;
    assert_eq!(before, after);
}

/// An arm with an envelope worth resolving, an IMU, and a frame graph with two collision bodies.
const LIMITED_ARM: &str = r#"{
  "robot_slug": "bench-arm",
  "base": "fixed",
  "dof": 1,
  "envelope": { "joint_velocity_max_rad_s": 1.0, "joint_torque_max_nm": 80,
                "watchdog_ms": 100, "source": "vendor datasheet rev C" },
  "sensors": [{"name": "torso_imu", "modality": "imu"}],
  "joints": [{"name": "shoulder", "role": "right_arm.shoulder.pitch", "type": "revolute",
              "control_modes": ["position", "torque"],
              "limits": {"effort_max_nm": 80, "velocity_max_rad_s": 1.0}}],
  "frames": {
    "root": "base_link",
    "convention": "rep103",
    "nodes": [
      {"name": "base_link", "parent": null, "kind": "root", "collision": true,
       "collision_shape": {"kind": "capsule", "radius_m": 0.1, "length_m": 0.2},
       "translation": [0, 0, 0], "rotation": [0, 0, 0, 1], "source": "descriptor"},
      {"name": "tool0", "parent": "base_link", "kind": "dynamic",
       "joint": "shoulder", "joint_type": "revolute", "axis": [0, 1, 0],
       "translation": [0, 0, 0.1], "rotation": [0, 0, 0, 1],
       "source": "descriptor", "collision": true,
       "collision_shape": {"kind": "capsule", "radius_m": 0.05, "length_m": 0.3}}
    ]
  }
}"#;

/// The profile as `crate::runtime` writes it once a signature has verified (§38.1).
const SITE_PROFILE: &str = r#"{
  "name": "site-a",
  "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
  "limits": { "joint_velocity_max_rad_s": 0.5, "max_delta_per_cycle": 0.1 },
  "cell": { "name": "bay-3", "facts": { "door_closed": true },
            "allowed": [{ "frame": "tool0",
                          "shape": { "kind": "box", "min": [-1, -1, 0], "max": [1, 1, 2] } }] }
}"#;

const MARGIN_PROFILE: &str = r#"{
  "name": "site-a",
  "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
  "limits": { "joint_velocity_max_rad_s": 0.5, "self_collision_margin_m": 0.05,
              "proximity": { "stop_distance_m": 1.2, "slow_distance_m": 2.5, "slow_factor": 0.25 } }
}"#;

const PARTIAL_PROXIMITY_PROFILE: &str = r#"{
  "name": "site-a",
  "digest": "sha256:1111111111111111111111111111111111111111111111111111111111111111",
  "limits": { "proximity": { "stop_distance_m": 1.2 } }
}"#;

fn write_profile(fixture: &Fixture, document: &str) {
    let layout = fixture.daemon.store.layout();
    std::fs::create_dir_all(layout.profiles()).unwrap();
    std::fs::write(layout.active_profile(), document).unwrap();
}

/// §7.6's two undeliverable profile fields, delivered.
///
/// `resolveLimits` has accepted `self_collision_margin_m` and `proximity` since it was written and
/// §11's profile carried neither, so two of the seven fields the kernel reads had nowhere to arrive
/// from. Both are on the profile now, under the profile's own signature (§38.1) rather than through
/// a second path.
///
/// The two land differently and the test says so, because the difference is the honest report:
/// the margin changes what the machine does, and the separation thresholds change what it *says*.
#[test]
fn the_profile_delivers_a_collision_margin_and_separation_thresholds() {
    let mut fixture = Fixture::new("limits-margin");
    fixture.provision();
    write_embodiment(&fixture, LIMITED_ARM);
    write_profile(&fixture, MARGIN_PROFILE);

    let (status, body) = fixture.request("GET", "/limits", Some("observer-token"), "");
    assert_eq!(status, 200);

    // The margin reached the kernel, so §7.1's fifth guard has a distance to hold at instead of
    // waiting for two surfaces to meet.
    assert_eq!(
        body.get("self_collision_margin_m").and_then(Json::as_f64),
        Some(0.05)
    );

    // And `no_collision_margin` is no longer emitted, which it was on every robot alive.
    let notices: Vec<&str> = body
        .arr_or_empty("notices")
        .iter()
        .filter_map(|notice| notice.get("code").and_then(Json::as_str))
        .collect();
    assert!(
        !notices.contains(&"no_collision_margin"),
        "the margin is set: {notices:?}"
    );

    // `proximity` is delivered and the guard is still unavailable — and the input it names is now
    // the sensor rather than the profile. That is the whole change on this half: "this platform
    // cannot deliver it" became "this robot has not got one", which is a fact about the body and
    // the thing §7.2 asks a guard to report.
    let unavailable = body
        .arr_or_empty("unavailable_guards")
        .iter()
        .find(|entry| entry.get("guard") == Some(&Json::from("proximity")))
        .cloned()
        .expect("proximity is unavailable without a person detector");
    let missing: Vec<&str> = unavailable
        .arr_or_empty("missing")
        .iter()
        .filter_map(Json::as_str)
        .collect();
    assert!(
        !missing.iter().any(|why| why.contains("profile.proximity")),
        "the profile carries it now: {missing:?}"
    );
    assert!(!missing.is_empty(), "and something is still missing");
}

/// A separation block a profile did not finish stays unfinished.
///
/// `skynet_kernel::Proximity` needs all three numbers. Inventing the missing one would be this
/// layer choosing a safety parameter, so an incomplete block leaves the guard naming
/// `profile.proximity` — §7.2's rule, and the reason the promotion is a `?` chain rather than a
/// set of defaults.
#[test]
fn a_half_written_separation_block_does_not_acquire_a_default() {
    let mut fixture = Fixture::new("limits-partial-proximity");
    fixture.provision();
    write_embodiment(&fixture, LIMITED_ARM);
    write_profile(&fixture, PARTIAL_PROXIMITY_PROFILE);

    let (status, body) = fixture.request("GET", "/limits", Some("observer-token"), "");
    assert_eq!(status, 200);

    let unavailable = body
        .arr_or_empty("unavailable_guards")
        .iter()
        .find(|entry| entry.get("guard") == Some(&Json::from("proximity")))
        .cloned()
        .expect("still unavailable");
    let missing: Vec<&str> = unavailable
        .arr_or_empty("missing")
        .iter()
        .filter_map(Json::as_str)
        .collect();
    assert!(
        missing.iter().any(|why| why.contains("profile.proximity")),
        "two of three is not a proximity configuration: {missing:?}"
    );
}

/// `GET /state` reports what was seen of the kernel, and whether anybody looked.
///
/// Three of §27's eight conditions are observed now, and `crate::observed` is what counts them.
/// The one that matters on this route is the
/// distinction §27 itself does not draw: a robot whose ring cannot hold a cycle and a robot whose
/// kernel has stopped are both `down` to the machine and are not the same fact to a person. A
/// console that showed "kernel: down" about the first would send somebody to restart a kernel that
/// is not the problem.
#[test]
fn the_state_route_says_whether_the_kernel_was_observed_at_all() {
    let mut fixture = Fixture::new("state-kernel");
    fixture.provision();
    let (status, body) = fixture.request("GET", "/state", Some("observer-token"), "");
    assert_eq!(status, 200);

    let observed = body.get("observed").cloned().expect("the condition table");
    assert_eq!(observed.get("post"), Some(&Json::Bool(true)));
    // This fixture's daemon has never run a pass, so nothing has looked.
    assert_eq!(observed.get("kernel"), Some(&Json::Bool(false)));
    assert_eq!(observed.get("rhal"), Some(&Json::Bool(false)));
    for condition in ["homing", "stop", "guard_hold", "guards", "policy"] {
        assert_eq!(
            observed.get(condition),
            Some(&Json::Bool(false)),
            "{condition} is still §33.1's side of the line"
        );
    }

    let kernel = body.get("kernel").cloned().expect("the reading beside it");
    assert_eq!(kernel.get("state"), Some(&Json::from("down")));
    assert_eq!(kernel.get("observed"), Some(&Json::Bool(false)));
    // The source, because §32.2 says a console must never show a value it did not receive — and
    // the *reason* this comes off the ring is the load-bearing half of the design.
    let source = kernel
        .get("source")
        .and_then(Json::as_str)
        .unwrap_or_default();
    assert!(source.contains("§13's ring"));
    assert!(
        source.contains("hung kernel is an active unit"),
        "the reason systemd is not the source has to travel with the reading"
    );

    // And the RHAL beside it, off the same records. A robot whose kernel is turning while its
    // driver has stopped answering has to be able to report exactly that pair.
    let rhal = body.get("rhal").cloned().expect("the RHAL reading");
    assert_eq!(rhal.get("state"), Some(&Json::from("lost")));
    assert_eq!(rhal.get("observed"), Some(&Json::Bool(false)));
    assert!(rhal
        .get("source")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .contains("faulted drive still answers reads"));
}

/// §15's runtime tests, over the real router.
#[test]
fn the_conformance_route_runs_what_it_can_and_names_what_it_cannot() {
    let mut fixture = Fixture::new("conformance-route");
    fixture.provision();

    let (status, body) = fixture.request("GET", "/conformance", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.arr_or_empty("results").len(), 8);

    let by_id = |id: &str| -> String {
        body.arr_or_empty("results")
            .iter()
            .find(|r| r.get("test_id").and_then(Json::as_str) == Some(id))
            .and_then(|r| r.get("outcome").and_then(Json::as_str))
            .unwrap_or("missing")
            .to_string()
    };

    // The one this edition passes, by running the function its own updater runs. Host-independent:
    // `admit_bundle` reads no file and asks the machine nothing.
    assert_eq!(by_id("ota.safety_gate"), "pass");
    // Skipped here, and the reason is a property of the fixture rather than of the check: this
    // route runs against a `RecordingLauncher` with nothing started, so there is no sandboxed unit
    // to read. The three verdicts the check itself produces are asserted in `conformance`'s own
    // suite, and the pass is asserted against a real systemd unit in `e2e.sh`.
    assert_eq!(by_id("safety.policy.isolation"), "skipped");
    let isolation = body
        .arr_or_empty("results")
        .iter()
        .find(|r| r.get("test_id").and_then(Json::as_str) == Some("safety.policy.isolation"))
        .and_then(|r| r.get("detail").and_then(Json::as_str))
        .unwrap_or_default()
        .to_string();
    assert!(
        isolation.contains("no sandboxed unit is running"),
        "a skip has to name what is missing: {isolation}"
    );

    // And the one it fails, which is the point of running the suite on yourself — and the only row
    // whose verdict depends on the **host** rather than on the edition. §8.1's memory figure comes
    // from `/proc/meminfo`, and `crate::capacity` refuses to invent one anywhere else, so the
    // admission probe runs on a robot and is honestly skipped on a developer's laptop.
    //
    // Asserting `fail` unconditionally made this suite green in CI (ubuntu-latest) and red on
    // every other machine, in the direction that hides the single row the runner exists to
    // report. Both branches are asserted, so neither host is checking a shorter exam.
    if cfg!(target_os = "linux") {
        assert_eq!(by_id("package.signature.required"), "fail");
        assert_eq!(body.get("executed"), Some(&Json::Num(2.0)));
    } else {
        assert_eq!(by_id("package.signature.required"), "skipped");
        assert_eq!(body.get("executed"), Some(&Json::Num(1.0)));
        let detail = body
            .arr_or_empty("results")
            .iter()
            .find(|r| r.get("test_id").and_then(Json::as_str) == Some("package.signature.required"))
            .and_then(|r| r.get("detail").and_then(Json::as_str))
            .unwrap_or_default()
            .to_string();
        assert!(
            detail.contains("capacity could not be assembled"),
            "a skip has to name what it was waiting for: {detail}"
        );
    }

    // Every row carries a sentence, including — especially — the skipped ones.
    for result in body.arr_or_empty("results") {
        assert!(
            result
                .get("detail")
                .and_then(Json::as_str)
                .is_some_and(|d| d.len() > 20),
            "{:?}",
            result.get("test_id")
        );
    }

    assert!(body
        .get("note")
        .and_then(Json::as_str)
        .is_some_and(|note| note.contains("never signs")));
}

/// `telemetry.replay.deterministic`, which had never executed anywhere.
///
/// Two robots, and the pair is the test: one with no descriptor, which is skipped with the reason,
/// and one with a body, which runs a window through its own ring and its own kernel and replays it.
/// Asserting only the second would leave a runner that could quietly report a pass for a machine it
/// had described to itself.
#[test]
fn the_replay_test_runs_on_a_robot_with_a_body_and_is_skipped_on_one_without() {
    let outcome = |fixture: &mut Fixture| -> (String, String) {
        let (_, body) = fixture.request("GET", "/conformance", Some("observer-token"), "");
        let row = body
            .arr_or_empty("results")
            .iter()
            .find(|r| {
                r.get("test_id").and_then(Json::as_str) == Some("telemetry.replay.deterministic")
            })
            .cloned()
            .expect("the replay row is always present");
        (
            row.get("outcome")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
            row.get("detail")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string(),
        )
    };

    let mut bare = Fixture::new("replay-bare");
    bare.provision();
    let (state, detail) = outcome(&mut bare);
    assert_eq!(state, "skipped");
    assert!(
        detail.contains("no channel names"),
        "a skip names what it was waiting for: {detail}"
    );

    let mut described = Fixture::new("replay-body");
    described.provision();
    write_embodiment(&described, LIMITED_ARM);
    let (state, detail) = outcome(&mut described);
    assert_eq!(state, "pass", "{detail}");
    // The window is not trivially deterministic: a guard actually had something to say about it,
    // and the sentence says which. A pass over a window nothing tripped is a weaker claim and the
    // runner distinguishes the two rather than printing one word for both.
    assert!(detail.contains("guards exercised: watchdog"), "{detail}");
    assert!(
        detail.contains("4 of them with a predecessor"),
        "five cycles, four warmed: {detail}"
    );
    // Always said, because a reader must not take this for a recorded policy replaying.
    assert!(detail.contains("no recorded policy to replay"), "{detail}");

    // The other branch, and the more interesting one. A body that declares no envelope has no
    // watchdog window either, so nothing in the window trips — and the runner names the reason
    // rather than reporting a bare pass. §73's transport row, measured on a machine: the third
    // cycle asks for a thousand times any plausible ceiling and is forwarded unchanged, because a
    // joint's ceiling comes from a transport plan and no policy is running to have one.
    let mut bare_body = Fixture::new("replay-unclamped");
    bare_body.provision();
    write_embodiment(&bare_body, BENCH_ARM);
    let (state, detail) = outcome(&mut bare_body);
    assert_eq!(state, "pass", "{detail}");
    assert!(detail.contains("§73's transport row"), "{detail}");
    assert!(detail.contains("active with nothing to clamp"), "{detail}");
}

/// A `GET` that runs checks must still be a `GET`: nothing it does may change the robot.
#[test]
fn running_the_conformance_suite_changes_nothing() {
    let mut fixture = Fixture::new("conformance-pure");
    fixture.provision();
    let before = fixture
        .request("GET", "/status", Some("observer-token"), "")
        .1;
    fixture.request("GET", "/conformance", Some("observer-token"), "");
    fixture.request("GET", "/conformance", Some("observer-token"), "");
    let after = fixture
        .request("GET", "/status", Some("observer-token"), "")
        .1;
    assert_eq!(
        before.to_canonical().ok(),
        after.to_canonical().ok(),
        "two runs of the suite left the robot in a different state"
    );
}

#[test]
fn reading_conformance_needs_an_observer() {
    let mut fixture = Fixture::new("conformance-role");
    fixture.provision();
    assert_eq!(fixture.request("GET", "/conformance", None, "").0, 401);
    assert_eq!(
        fixture
            .request("GET", "/conformance", Some("observer-token"), "")
            .0,
        200
    );
}

#[test]
fn reading_power_needs_an_observer() {
    let mut fixture = Fixture::new("power-route-role");
    fixture.provision();
    assert_eq!(fixture.request("GET", "/power", None, "").0, 401);
    assert_eq!(
        fixture
            .request("GET", "/power", Some("observer-token"), "")
            .0,
        200
    );
}

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
    // §13's window, named. On this fixture there is no ring, so the incident says *that* rather
    // than omitting the field — a consumer that sees no `recording` cannot tell a robot with no
    // black box from an edition that never attached one, and those send an investigator to two
    // different places.
    let document = Json::parse(&queued[0].body).expect("the incident is JSON");
    let recording = document.get("recording").expect("a recording block");
    assert_eq!(recording.get("sealed"), Some(&Json::Bool(false)));
    assert!(recording
        .get("why")
        .and_then(Json::as_str)
        .is_some_and(|why| why.contains("no black box ring")));

    // The incident says the limits were not applied, because an operator reading it needs to know
    // whether the robot is currently running on ceilings somebody edited.
    assert!(
        queued[0].body.contains("NOT applied"),
        "{:?}",
        queued[0].body
    );

    // And it carries the context of the **fault**, not of whenever this reaches a Hub. §34.4
    // delivers an incident whenever the link comes back, which can be after a reboot and two
    // generations later — so a Hub filling these in at receipt would be attaching a statement about
    // now to an event in the past.
    assert!(
        queued[0].body.contains("\"generation\""),
        "an incident that does not say which generation it happened on can only be dated by \
         whenever it arrived: {:?}",
        queued[0].body
    );
    assert!(
        queued[0].body.contains("\"state\""),
        "§27's state at the fault says whether the robot had already stopped or was still moving: \
         {:?}",
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

    /// An OS update this robot cannot apply stays in the plan carrying why. A supervisor that
    /// dropped it would converge on a lie.
    ///
    /// The reason used to be *"OTA is not written"*. §75 wrote it, and the refusal this asserts is
    /// now the real one: §12's third rule needs the release manifest, because the manifest is what
    /// says whether the image touches the Safety Kernel, and this robot has never been sent one.
    /// The outcome moved from `Unimplemented` to `Failed` with it — an absent document is a
    /// condition of *this* robot that a later check-in can fix, not a missing feature.
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
            // §64: the refusal names the document that is missing and the rule that wants it,
            // because the person reading it has to know which of the two sides to go and fix.
            Outcome::Failed { reason } => {
                assert!(reason.contains("release `2.0.0`"), "{reason}");
                assert!(reason.contains("§57"), "{reason}");
                assert!(reason.contains("§12"), "{reason}");
            }
            other => panic!("expected a named refusal, got {other:?}"),
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

    fixture.daemon.refresh_operating_state(
        None,
        &crate::observed::Sightings {
            kernel: &blackbox::KernelSighting::Unobservable("no ring in this fixture"),
            rhal: &blackbox::RhalSighting::Unobservable("no ring in this fixture"),
            stop: &blackbox::StopSighting::Unobservable("no ring in this fixture"),
            hold: &blackbox::HoldSighting::Unobservable("no ring in this fixture"),
            guards: &crate::limits::GuardsSighting::Unobservable("no descriptor in this fixture"),
            policy: &crate::shape::PolicySighting::Nothing { no_body: true },
            homing: &blackbox::HomingSighting::Unobservable("no ring in this fixture"),
        },
    );
    let (_, after) = fixture.request("GET", "/status", None, "");
    assert_eq!(after.get("state").and_then(Json::as_str), Some("safe"));
    assert_eq!(
        after.get("state_reason").and_then(Json::as_str),
        Some("rhal_lost")
    );
}

/// `GET /state` says which of §27's eight conditions anybody looked at, and on a robot with no
/// ring, no descriptor and nothing installed it looks at two. A page that showed the state without
/// that would be reporting a robot as safe when the truth is that almost nothing has looked at it.
///
/// The two are `post` — this process's own self-test — and `policy`, and the second is worth
/// stating: *nothing is installed that declares an action space* is a fact this daemon holds the
/// store for, and answering it is not a §33.1 crossing. What would be one is claiming §25.2's
/// lease, which is why the fixture that has a package installed reports this condition unobserved
/// and this one does not.
#[test]
fn the_state_route_admits_how_little_it_observed() {
    let mut fixture = Fixture::new("state-route");
    fixture.provision();
    fixture.daemon.refresh_operating_state(
        None,
        &crate::observed::Sightings {
            kernel: &blackbox::KernelSighting::Unobservable("no ring in this fixture"),
            rhal: &blackbox::RhalSighting::Unobservable("no ring in this fixture"),
            stop: &blackbox::StopSighting::Unobservable("no ring in this fixture"),
            hold: &blackbox::HoldSighting::Unobservable("no ring in this fixture"),
            guards: &crate::limits::GuardsSighting::Unobservable("no descriptor in this fixture"),
            policy: &crate::shape::PolicySighting::Nothing { no_body: true },
            homing: &blackbox::HomingSighting::Unobservable("no ring in this fixture"),
        },
    );
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
    assert_eq!(
        observed.get("policy").and_then(Json::as_bool),
        Some(true),
        "`policy` is answerable here: nothing is installed, and this daemon holds the store"
    );
    assert_eq!(
        body.get("policy")
            .and_then(|block| block.get("state"))
            .and_then(Json::as_str),
        Some("none"),
        "and the answer is `none`, which is the sentence with no remedy rather than the one with"
    );
    for unobserved in ["kernel", "rhal", "homing", "stop", "guard_hold", "guards"] {
        assert_eq!(
            observed.get(unobserved).and_then(Json::as_bool),
            Some(false),
            "`{unobserved}` is not observable on this fixture"
        );
    }
}

/// Two routes describe one pass, and they must agree about how much of it was observed.
///
/// The regression this exists for is not hypothetical and was not small. `GET /state`'s `partial`
/// note, `GET /status`'s `safe_state_note` and the start-up banner each carried a hand-written
/// count, and on 2026-09-07 they disagreed: two said three conditions were observed and the third
/// told an operator the machine was deciding from *"the self-test and eight unobserved
/// conditions"* — a sentence that counted §27's total as the remainder, so it described a daemon
/// that had looked at nine things and observed one. It had been wrong on every pass for a
/// fortnight and nothing could have failed.
///
/// So the assertion is the **relationship**, never the number: whatever `observed` holds, both
/// sentences count it the same way. A condition gaining a producer moves all three together, and
/// this test does not need editing when one does — which is the property the old arrangement
/// lacked and the only reason `crate::observed` exists.
#[test]
fn every_sentence_about_the_conditions_counts_the_same_object() {
    for (kernel, rhal, stop, hold, guards, policy, homing, expected) in [
        (
            blackbox::KernelSighting::Unobservable("no ring in this fixture"),
            blackbox::RhalSighting::Unobservable("no ring in this fixture"),
            blackbox::StopSighting::Unobservable("no ring in this fixture"),
            blackbox::HoldSighting::Unobservable("no ring in this fixture"),
            crate::limits::GuardsSighting::Unobservable("no descriptor in this fixture"),
            crate::shape::PolicySighting::LeaseUnknown {
                package: "com.example.walk".to_string(),
            },
            blackbox::HomingSighting::Unobservable("no ring in this fixture"),
            1,
        ),
        (
            blackbox::KernelSighting::Turning { cycles: 40 },
            blackbox::RhalSighting::Seen {
                state: skynet_contracts::opstate::RhalState::Present,
                errno: None,
                fault_code: 0,
            },
            blackbox::StopSighting::Clear,
            blackbox::HoldSighting::Clear,
            crate::limits::GuardsSighting::AllAvailable,
            crate::shape::PolicySighting::Nothing { no_body: false },
            // A driver that answers §24's homing field, which is what makes this row eight rather
            // than seven — and the row this test existed to protect: a condition gaining a
            // producer moves the tally, the prose and the complement together and edits nothing.
            blackbox::HomingSighting::Reported(skynet_contracts::opstate::HomingState::NotRequired),
            8,
        ),
    ] {
        let mut fixture = Fixture::new("condition-count");
        fixture.provision();
        fixture.daemon.refresh_operating_state(
            None,
            &crate::observed::Sightings {
                kernel: &kernel,
                rhal: &rhal,
                stop: &stop,
                hold: &hold,
                guards: &guards,
                policy: &policy,
                homing: &homing,
            },
        );

        let (_, state) = fixture.request("GET", "/state", Some("observer-token"), "");
        let observed = state.get("observed").expect("the observation map");
        let counted = crate::observed::CONDITIONS
            .iter()
            .filter(|name| observed.get(name).and_then(Json::as_bool) == Some(true))
            .count();
        assert_eq!(
            counted, expected,
            "the object itself is the authority and it says {counted}"
        );

        // The word every one of these sentences opens its tally with. Read out of the rendered
        // string rather than recomputed, because a test that recomputed it would pass on two
        // sentences that were both wrong in the same way.
        let word = [
            "none", "one", "two", "three", "four", "five", "six", "seven", "eight",
        ][counted];
        let clause = format!("{word} of §27's eight conditions");

        let partial = state
            .get("partial")
            .and_then(Json::as_str)
            .unwrap_or_default();
        assert!(
            partial.contains(&clause),
            "GET /state's partial note must say `{clause}`, and says: {partial}"
        );

        let (_, status) = fixture.request("GET", "/status", None, "");
        let note = status
            .get("safe_state_note")
            .and_then(Json::as_str)
            .unwrap_or_default();
        assert!(
            note.contains(&clause),
            "GET /status's safe-state note must say `{clause}`, and says: {note}"
        );

        // And the complement, by name rather than by count — which is what the note renders and is
        // the stronger half: a reader who is told five conditions went unobserved still does not
        // know which, and "the homing state" is the one that decides whether a robot may move.
        //
        // Every condition appears exactly once, whichever side of the sentence it is on, so a
        // rendering that dropped one or listed it twice fails here.
        for (name, label) in crate::observed::CONDITIONS.iter().zip([
            "the self-test",
            "the kernel",
            "the RHAL",
            "the homing state",
            "the stop",
            "the guard hold",
            "the guard set",
            "the policy's lease",
        ]) {
            assert_eq!(
                note.matches(label).count(),
                1,
                "`{name}` must appear once in the note, on whichever side it belongs. It says: \
                 {note}"
            );
        }
    }
}

/// §82.4's shutdown: a request, an actor, and a route that stops nothing by itself.
///
/// The split is the design rather than a limitation. This function runs inside a connection the
/// caller is still waiting on; the mDNS responder is another thread and the recording is the
/// loop's to seal. So the route records **who asked** and the loop performs it at the top of its
/// next turn — §33.3's shape, where the API states an intent and the loop converges on it.
#[test]
fn shutdown_records_who_asked_and_stops_nothing_by_itself() {
    let mut fixture = Fixture::new("shutdown-request");
    fixture.provision();

    assert!(
        fixture.daemon.shutdown_requested.is_none(),
        "nothing has asked yet"
    );

    let (status, body) = fixture.request("POST", "/shutdown", Some("admin-token"), "");
    assert_eq!(status, 200);
    assert_eq!(body.get("stopping").and_then(Json::as_bool), Some(true));

    // The actor, by the credential's label rather than by its token. §38.3 wants the name of the
    // somebody, and a shutdown is exactly the act somebody will be asked about later.
    let asked = fixture
        .daemon
        .shutdown_requested
        .as_deref()
        .expect("the request is recorded");
    assert!(!asked.is_empty(), "a shutdown carries who asked for it");

    // And the note says what it does *not* do, because the confusion is one an operator can make
    // under pressure: this stops the supervisor and leaves the body alone.
    let note = body.get("note").and_then(Json::as_str).unwrap_or_default();
    assert!(note.contains("not the robot"), "{note}");
    assert!(note.contains("§82.4"), "it names the goodbye: {note}");
    assert!(note.contains("shutdown"), "and the seal reason: {note}");
}

/// It is admin, and an operator is refused (§39.1).
///
/// The fall-through in `auth` would have given the same answer, and the entry is explicit for the
/// reason the table gives about `unpair`: a robot whose supervisor is not running applies no
/// desired state and answers nobody, and only systemd can start it again.
#[test]
fn shutdown_is_admin_and_an_operator_cannot_stop_the_supervisor() {
    let mut fixture = Fixture::new("shutdown-role");
    fixture.provision();

    let (status, _) = fixture.request("POST", "/shutdown", Some("operator-token"), "");
    assert_eq!(
        status, 403,
        "an operator may stop the robot, not the daemon"
    );
    assert!(
        fixture.daemon.shutdown_requested.is_none(),
        "a refused request is not a request"
    );

    // And anonymously, which is the one that matters most: §39.1 makes `POST /stop` open on
    // purpose, and a caller reaching for the wrong one must not take the supervisor down with it.
    let (status, _) = fixture.request("POST", "/shutdown", None, "");
    assert_eq!(status, 401);
    assert!(fixture.daemon.shutdown_requested.is_none());
}

/// §27's `stop`, off §24's flags, and the two halves of the decision taken on 2026-09-07.
///
/// A robot whose emergency-stop chain is open reports `stopping` with `stopping_estop` — its own
/// cause, never `operator`, because §24 gives the chain one bit and a light curtain opens the same
/// chain as the button. And it never reports the stop as **finished**: `stop_settled` is not
/// observable from that bit, so the machine cannot reach `stop_complete` and a robot whose chain
/// has been open for an hour goes on saying `stopping`. That is the whole of the trade, and it is
/// the fail-closed direction — this can hold a robot and can never release one.
#[test]
fn an_open_stop_chain_is_its_own_cause_and_never_a_completed_stop() {
    let mut fixture = Fixture::new("estop-condition");
    fixture.provision();
    fixture.daemon.refresh_operating_state(
        None,
        &crate::observed::Sightings {
            kernel: &blackbox::KernelSighting::Turning { cycles: 40 },
            rhal: &blackbox::RhalSighting::Seen {
                state: skynet_contracts::opstate::RhalState::Present,
                errno: None,
                fault_code: 0,
            },
            stop: &blackbox::StopSighting::Asserted,
            hold: &blackbox::HoldSighting::Clear,
            guards: &crate::limits::GuardsSighting::AllAvailable,
            policy: &crate::shape::PolicySighting::Nothing { no_body: false },
            homing: &blackbox::HomingSighting::Unreported,
        },
    );

    let (status, state) = fixture.request("GET", "/state", Some("observer-token"), "");
    assert_eq!(status, 200);
    assert_eq!(
        state.get("state").and_then(Json::as_str),
        Some("stopping"),
        "an open chain is a stop in progress, not a completed one"
    );
    assert_eq!(
        state.get("reason").and_then(Json::as_str),
        Some("stopping_estop"),
        "its own cause: `operator` would claim a person asked, and the chain cannot say"
    );

    let observed = state.get("observed").expect("the observation map");
    assert_eq!(observed.get("stop").and_then(Json::as_bool), Some(true));

    let stop = state.get("stop").expect("the stop block");
    assert_eq!(stop.get("asserted").and_then(Json::as_bool), Some(true));
    assert!(stop
        .get("detail")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .contains("open on the newest cycle"));
}

/// A cycle whose read failed is not a reading of the chain (§26.4), in either direction.
///
/// The flags in such a record are the *previous* cycle's, so reporting `Clear` off one would be
/// §26.4's own failure a field further along — and reporting `Asserted` off one would raise a stop
/// nobody observed. It is unobservable, and §27 is already holding the robot through `rhal`, which
/// reads the same record.
#[test]
fn a_failed_read_says_nothing_about_the_stop_chain() {
    use skynet_blackbox::record::ReadOutcome;

    let mut cycle = skynet_blackbox::Cycle {
        tick: 1,
        observation: [0; 32],
        policy: 0,
        state_flags: 0,
        fault_code: 0,
        rhal_result: 0,
        read: ReadOutcome::Failed(-110),
        state_seq: 1,
        state_stamp_ns: 0,
        command_stamp_ns: 0,
        command_seq: 0,
        mode: 0,
        advisory: false,
        response: None,
        tripped: Vec::new(),
        capability: skynet_blackbox::record::NO_CAPABILITY,
        nearest_person_m: f32::NAN,
        base_pose: [f32::NAN; 7],
        base_twist: [0.0; 6],
        desired: Vec::new(),
        command: Vec::new(),
        mask: Vec::new(),
        clamped: Vec::new(),
        position: Vec::new(),
        velocity: Vec::new(),
        effort: Vec::new(),
    };
    assert!(
        !blackbox::StopSighting::of(Some(&cycle)).observed(),
        "a fill value is not a reading of the chain"
    );

    // And with the bit set in the fill, which is the dangerous direction: the previous cycle's
    // asserted chain must not be reported as this cycle's.
    cycle.state_flags = 1;
    assert!(!blackbox::StopSighting::of(Some(&cycle)).observed());

    // The same record with a successful read is a reading, and says so.
    cycle.read = ReadOutcome::Ok;
    assert_eq!(
        blackbox::StopSighting::of(Some(&cycle)),
        blackbox::StopSighting::Asserted
    );
}

/// §27's `guard_hold`, and the two things it must never say.
///
/// This condition was `false` on every robot for as long as §27's machine has existed, filed as
/// *"the kernel's"* — which was true about whose fact it is and false about where the evidence
/// lives. §26 records the guard verdict beside the flags, so the daemon can read it without
/// asking the kernel anything, exactly as it reads `kernel`, `rhal` and `stop`.
///
/// The two refusals are the whole design and neither is obvious from the field name:
///
/// 1. **An advisory hold is not a hold.** §7.4 lets a tier that cannot promise a clamp record the
///    verdict and forward the command anyway, and every robot on this edition is one. Reporting
///    `HOLD` there would tell an operator the robot is held at the moment it is moving.
/// 2. **A failed body read does not stop this one.** `rhal` and `stop` refuse such a cycle because
///    §26.4's fill replaces what the driver reported; the kernel's decision on that cycle is not a
///    fill, and "is a guard holding" is a question about the kernel.
#[test]
fn a_guard_hold_is_read_off_the_verdict_and_only_when_it_was_enforced() {
    use skynet_blackbox::record::ReadOutcome;

    let mut cycle = skynet_blackbox::Cycle {
        tick: 1,
        observation: [0; 32],
        policy: 0,
        state_flags: 0,
        fault_code: 0,
        rhal_result: 0,
        read: ReadOutcome::Ok,
        state_seq: 1,
        state_stamp_ns: 0,
        command_stamp_ns: 0,
        command_seq: 0,
        mode: 0,
        advisory: false,
        response: None,
        tripped: Vec::new(),
        capability: skynet_blackbox::record::NO_CAPABILITY,
        nearest_person_m: f32::NAN,
        base_pose: [f32::NAN; 7],
        base_twist: [0.0; 6],
        desired: Vec::new(),
        command: Vec::new(),
        mask: Vec::new(),
        clamped: Vec::new(),
        position: Vec::new(),
        velocity: Vec::new(),
        effort: Vec::new(),
    };

    // No verdict: the guards ran and passed, or nothing was posted. Observed, and not a hold.
    let clear = blackbox::HoldSighting::of(Some(&cycle));
    assert_eq!(clear, blackbox::HoldSighting::Clear);
    assert!(clear.observed());
    assert!(!clear.held());

    // An enforced hold.
    cycle.response = Some(skynet_blackbox::FaultResponse::Hold);
    let held = blackbox::HoldSighting::of(Some(&cycle));
    assert_eq!(held, blackbox::HoldSighting::Held);
    assert!(held.held());

    // The same verdict, advisory. Looked at, a guard fired, and **nothing was withheld** — so §27's
    // condition is false and the reading still counts as an observation. This is the case that
    // exists on every robot this edition runs on.
    cycle.advisory = true;
    let advisory = blackbox::HoldSighting::of(Some(&cycle));
    assert_eq!(advisory, blackbox::HoldSighting::Advisory);
    assert!(
        advisory.observed(),
        "a guard fired and somebody should see it"
    );
    assert!(
        !advisory.held(),
        "an advisory verdict forwards the command; reporting HOLD would describe a robot that \
         does not exist"
    );
    assert!(advisory.detail().contains("advisory"));

    // A clamp is not a hold: the command was reduced and sent, so the robot is moving.
    cycle.advisory = false;
    cycle.response = Some(skynet_blackbox::FaultResponse::Clamp);
    assert_eq!(
        blackbox::HoldSighting::of(Some(&cycle)),
        blackbox::HoldSighting::Clear
    );

    // And the split from `stop`: a cycle whose body read failed is unreadable to `StopSighting`
    // and readable here, because the verdict is the kernel's own act rather than a fill of the
    // body's flags (§26.4).
    cycle.response = Some(skynet_blackbox::FaultResponse::Hold);
    cycle.read = ReadOutcome::Failed(-110);
    assert!(!blackbox::StopSighting::of(Some(&cycle)).observed());
    assert_eq!(
        blackbox::HoldSighting::of(Some(&cycle)),
        blackbox::HoldSighting::Held,
        "the kernel withheld the command on that cycle whatever its read of the body returned"
    );

    // No cycle at all is not a reading, and says which absence it is.
    let none = blackbox::HoldSighting::of(None);
    assert!(!none.observed());
    assert!(!none.held());
}

/// An enforced hold reaches the route — and does **not** reach §27's derived state.
///
/// `guard_hold` is rule 7 of `derive` and homing is rule 5, so a robot whose homing state is
/// `Pending` reports `unhomed` while carrying a perfectly good guard-hold observation.
///
/// **The reason this fixture is `Unreported` is the point of the test now.** It used to be that
/// nothing could report homing at all; §24's two-bit field changed that, and what this fixture
/// describes is a robot on a **v1 driver** — one that answers everything else and leaves the new
/// field zero. That is the majority of the driver set on the day it was written and it fails
/// closed exactly as before, which is what makes it the right fixture for asserting the ordering.
/// The robot that gets past rule 5 is
/// [`a_driver_that_answers_the_homing_field_reaches_the_condition_and_the_machine`].
///
/// So this asserts the two things that did change — the condition is **observed**, and the block
/// §32.2 requires renders what was seen — and asserts the ordering explicitly rather than leaving
/// somebody to discover it. That `derive` turns `guard_hold` into `HOLD` is not this test's to
/// prove; `opstate`'s own suite does it.
#[test]
fn an_enforced_hold_reaches_the_route_and_homing_still_outranks_it() {
    let mut fixture = Fixture::new("guard-hold-condition");
    fixture.provision();
    fixture.daemon.refresh_operating_state(
        None,
        &crate::observed::Sightings {
            kernel: &blackbox::KernelSighting::Turning { cycles: 40 },
            rhal: &blackbox::RhalSighting::Seen {
                state: skynet_contracts::opstate::RhalState::Present,
                errno: None,
                fault_code: 0,
            },
            stop: &blackbox::StopSighting::Clear,
            hold: &blackbox::HoldSighting::Held,
            guards: &crate::limits::GuardsSighting::AllAvailable,
            policy: &crate::shape::PolicySighting::Nothing { no_body: false },
            homing: &blackbox::HomingSighting::Unreported,
        },
    );

    let (status, state) = fixture.request("GET", "/state", Some("observer-token"), "");
    assert_eq!(status, 200);

    let observed = state.get("observed").expect("the observation map");
    assert_eq!(
        observed.get("guard_hold").and_then(Json::as_bool),
        Some(true)
    );

    let block = state.get("guard_hold").expect("the guard-hold block");
    assert_eq!(block.get("held").and_then(Json::as_bool), Some(true));
    assert_eq!(block.get("advisory").and_then(Json::as_bool), Some(false));
    assert!(block
        .get("detail")
        .and_then(Json::as_str)
        .unwrap_or_default()
        .contains("withheld it on every channel"));

    // The ordering, stated as an assertion so that the day `homing` and the self-test gain
    // producers this test says which sentence to rewrite.
    assert_ne!(
        state.get("reason").and_then(Json::as_str),
        Some("guard_hold"),
        "a condition above this one is unobserved and takes the machine first — closing \
         `guard_hold` moves the observation, not yet the state"
    );
}

/// A driver that answers §24's homing field reaches §27's condition, and moves the machine.
///
/// The cascade this whole change exists for, asserted in the two halves it actually has.
///
/// Rule 5 — *"joint positions are unknown, so a policy must not run"* — held **every robot on this
/// edition** for as long as nothing could report a homing state, and it sits above the kernel (6),
/// the guard hold (7) and the policy (8). So the four conditions below it were observations no
/// robot's state ever reflected.
///
/// **The daemon's half** is that a driver's answer arrives as §27's condition and that all eight
/// conditions are then readings. That is what `refresh_operating_state` is asserted on here, and it
/// is asserted on the fixture's real state rather than a hoped-for one: this fixture's self-test
/// does not pass, so rule 4 stops it before rule 5 and the *state* is `post` either way. Asserting
/// a state this fixture cannot reach would be asserting the fixture, not the change.
///
/// **The machine's half** is the ordering, and it is driven directly — the only way to reach rule 5
/// from here, and the same technique the `policy` condition's suite needed for rule 8. What it
/// shows is that passing rule 5 does **not** buy a commanding robot: the machine stops at rule 8,
/// because `PolicySighting` never produces `Ready` on any input and §25.2's lease is the Safety
/// Kernel's table.
///
/// That is the honest size of the step, and it is a result rather than a disappointment: the
/// robot's reason changes from *"nobody has homed this machine"* — false, about a machine with no
/// homing routine — to *"there is nothing here holding a lease"*, which is true and has a remedy.
#[test]
fn a_driver_that_answers_the_homing_field_reaches_the_condition_and_the_machine() {
    use skynet_contracts::opstate::{
        evaluate, Conditions, HomingState, KernelState, OperatingState, PolicyState, PostVerdict,
        RhalState, StateReason,
    };

    let mut fixture = Fixture::new("homing-answered");
    fixture.provision();
    fixture.daemon.refresh_operating_state(
        None,
        &crate::observed::Sightings {
            kernel: &blackbox::KernelSighting::Turning { cycles: 40 },
            rhal: &blackbox::RhalSighting::Seen {
                state: RhalState::Present,
                errno: None,
                fault_code: 0,
            },
            stop: &blackbox::StopSighting::Clear,
            hold: &blackbox::HoldSighting::Clear,
            guards: &crate::limits::GuardsSighting::AllAvailable,
            policy: &crate::shape::PolicySighting::Nothing { no_body: false },
            homing: &blackbox::HomingSighting::Reported(HomingState::NotRequired),
        },
    );

    let (status, state) = fixture.request("GET", "/state", Some("observer-token"), "");
    assert_eq!(status, 200);

    // Every one of §27's eight conditions is a reading now. This is the assertion that goes stale
    // if a condition ever loses its producer, and it is deliberately over the whole list rather
    // than over the one that was added.
    let observed = state.get("observed").expect("the observation map");
    for condition in crate::observed::CONDITIONS {
        assert_eq!(
            observed.get(condition).and_then(Json::as_bool),
            Some(true),
            "{condition} should be observed once a driver answers the homing field"
        );
    }

    // The machine, driven directly past the self-test this fixture cannot pass. Everything below
    // rule 5 is exactly what the sightings above reported.
    let conditions = |homing| Conditions {
        post: PostVerdict::Passed,
        kernel: KernelState::Live,
        rhal: RhalState::Present,
        homing,
        stop: None,
        guard_hold: false,
        policy: PolicyState::None,
        ..Conditions::default()
    };

    // A v1 driver leaves the field zero, which reads as `Pending`, which is rule 5.
    let v1 = evaluate(OperatingState::Post, &conditions(HomingState::Pending));
    assert_eq!(v1.to, OperatingState::Unhomed);
    assert_eq!(v1.reason, StateReason::HomingPending);

    // A driver that answers passes it, and stops at the next rule that is actually true.
    for answered in [HomingState::NotRequired, HomingState::Complete] {
        let moved = evaluate(OperatingState::Post, &conditions(answered));
        assert_ne!(
            moved.to,
            OperatingState::Unhomed,
            "a body whose encoders are absolute is not a body waiting to be homed"
        );
        assert_eq!(moved.to, OperatingState::Hold);
        assert_eq!(
            moved.reason,
            StateReason::NoPolicy,
            "rule 8 is the next one that stops it, and it names the real blocker"
        );
    }
}

/// And the advisory case, on the route, which is the one every robot here produces.
///
/// A guard fired and the command went through. The state must **not** be `hold`, the condition
/// must be reported as observed, and `advisory` must be visible — a console given only `held`
/// could not tell this robot from one no guard touched, and those are opposite facts.
#[test]
fn an_advisory_hold_is_reported_without_holding_the_robot() {
    let mut fixture = Fixture::new("guard-hold-advisory");
    fixture.provision();
    fixture.daemon.refresh_operating_state(
        None,
        &crate::observed::Sightings {
            kernel: &blackbox::KernelSighting::Turning { cycles: 40 },
            rhal: &blackbox::RhalSighting::Seen {
                state: skynet_contracts::opstate::RhalState::Present,
                errno: None,
                fault_code: 0,
            },
            stop: &blackbox::StopSighting::Clear,
            hold: &blackbox::HoldSighting::Advisory,
            guards: &crate::limits::GuardsSighting::AllAvailable,
            policy: &crate::shape::PolicySighting::Nothing { no_body: false },
            homing: &blackbox::HomingSighting::Unreported,
        },
    );

    let (status, state) = fixture.request("GET", "/state", Some("observer-token"), "");
    assert_eq!(status, 200);
    // Weaker than it looks on this edition — a condition above `guard_hold` is holding the machine
    // anyway — so the assertion that carries the rule is on `reason`, below, and on `held` in the
    // block. Both would be wrong the moment an advisory verdict was mapped onto §27's condition.
    assert_ne!(
        state.get("reason").and_then(Json::as_str),
        Some("guard_hold"),
        "nothing was withheld, so nothing may report the robot as held for a guard"
    );

    let observed = state.get("observed").expect("the observation map");
    assert_eq!(
        observed.get("guard_hold").and_then(Json::as_bool),
        Some(true),
        "the daemon looked; the answer was no"
    );

    let block = state.get("guard_hold").expect("the guard-hold block");
    assert_eq!(block.get("held").and_then(Json::as_bool), Some(false));
    assert_eq!(block.get("advisory").and_then(Json::as_bool), Some(true));
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
  "joints": [{"name": "shoulder", "role": "right_arm.shoulder.pitch", "type": "revolute",
              "control_modes": ["position", "torque"],
              "limits": {"effort_max_nm": 80}}]
}"#;

fn write_embodiment(fixture: &Fixture, document: &str) {
    let path = fixture.daemon.store.layout().embodiment();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, document).unwrap();
}

/// `GET /status` names the robot, and says nothing when it cannot.
///
/// This is the assertion the first bring-up on a real host failed. `virtual-robot.sh` reads its ten
/// facts back *from the robot* rather than from what the installer wrote, and the one that says
/// which body was installed had nothing to read: the status document carried the edition, the
/// contracts version, the generation, the tier and the safe state, and no part of the machine's own
/// identity. Every other route agreed with it, because the field was nowhere.
///
/// Both directions are tested, and the absent one matters as much: a placeholder here would read as
/// a robot that answered the question, and §28 makes "no descriptor" a provisioning step somebody
/// has to run.
#[test]
fn status_names_the_body_when_there_is_one_and_omits_it_when_there_is_not() {
    let mut fixture = Fixture::new("status-body");
    fixture.provision();

    let (status, body) = fixture.request("GET", "/status", None, "");
    assert_eq!(status, 200);
    assert_eq!(
        body.get_present("robot_slug"),
        None,
        "a robot with no descriptor must not name a body"
    );

    write_embodiment(
        &fixture,
        r#"{"robot_slug":"mock-arm","base":"fixed","dof":1,
            "joints":[{"name":"shoulder","type":"revolute","control_modes":["position"]}]}"#,
    );
    let (_, body) = fixture.request("GET", "/status", None, "");
    assert_eq!(
        body.get("robot_slug").and_then(Json::as_str),
        Some("mock-arm")
    );

    // Malformed is the third state, and it is not a name. An operator whose generator emitted
    // something wrong must not read a slug off a descriptor this robot refused.
    write_embodiment(&fixture, "{ not json");
    let (_, body) = fixture.request("GET", "/status", None, "");
    assert_eq!(body.get_present("robot_slug"), None);
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

            // `disk_mb` was on that line until `skynet-statfs` was written, and the rule it is
            // held to now is the same one: **never estimated**. A machine whose volume can be read
            // reports the megabytes available where packages are installed — which admission
            // compares a package's declared need against — and one that cannot reports nothing.
            // Zero is the answer neither may give, because §8.1 would then refuse every package
            // that declares any disk at all.
            match body.get("disk_mb") {
                Some(Json::Null) => {}
                Some(Json::Num(mb)) => assert!(
                    *mb > 1.0,
                    "{mb} MB free is a reading being misinterpreted, not a machine that runs this \
                     daemon",
                ),
                other => panic!("disk_mb is megabytes or nothing: {other:?}"),
            }
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
    for field in ["guard_trip_rate", "battery", "temperature_c"] {
        assert_eq!(
            body.get(field),
            Some(&Json::Null),
            "{field} must not be zero"
        );
    }

    // Storage headroom left that list when `skynet-statfs` was written, and the assertion that
    // replaces it is the one that was underneath it all along: **never zero unless it is true**.
    // A machine that can measure reports a fraction; one that cannot reports null; neither reports
    // a full disk on a fixture with room on it.
    match body.get("storage_headroom") {
        Some(Json::Null) => {}
        Some(Json::Num(fraction)) => assert!(
            *fraction > 0.0 && *fraction <= 1.0,
            "a measured headroom is a fraction of a disk that is not full: {fraction}",
        ),
        other => panic!("storage_headroom is a fraction or nothing: {other:?}"),
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
fn a_grant_made_at_the_robot_is_recorded_in_both_places() {
    // §38.3 asks for two records of every configuration change, and §49.2 names the audit half
    // again for one token. Neither was written for a grant made over the local API: this robot
    // recorded that somebody read a safety note (§54.2) and not that it then handed over the
    // capability the note was about.
    let mut fixture = Fixture::new("api-grant-audit");
    fixture.provision();

    let (status, _) = fixture.request(
        "POST",
        "/grants",
        Some("admin-token"),
        r#"{"id":"openrobot/grasp","token":"sensing.audio"}"#,
    );
    assert_eq!(status, 201);

    let log = crate::audit::Audit::of(fixture.daemon.store.layout()).read();
    let entry = log
        .entries
        .iter()
        .find(|entry| entry.action == "capability.grant")
        .expect("the grant is in the audit log");
    assert_eq!(entry.subject, "openrobot/grasp:sensing.audio");
    assert_eq!(entry.previous, None);

    let (status, _) = fixture.request(
        "DELETE",
        "/grants/openrobot:grasp/sensing.audio",
        Some("admin-token"),
        "",
    );
    assert_eq!(status, 200);
    let log = crate::audit::Audit::of(fixture.daemon.store.layout()).read();
    let revoked = log
        .entries
        .iter()
        .find(|entry| entry.action == "capability.revoke")
        .expect("the revocation too");
    // §38.3 wants the previous value, and for a token there is only one thing it can be.
    assert_eq!(revoked.previous.as_deref(), Some("held"));
}

#[test]
fn a_request_that_changed_nothing_is_not_recorded_as_a_change() {
    // An audit log that recorded no-ops would train a reader to skim it, which is the failure
    // §38.3's log exists to avoid — and `already_held` and `not_held` are exactly the replays an
    // operator's shell history produces.
    let mut fixture = Fixture::new("api-grant-audit-noop");
    fixture.provision();
    let body = r#"{"id":"openrobot/grasp","token":"sensing.audio"}"#;
    fixture.request("POST", "/grants", Some("admin-token"), body);
    fixture.request("POST", "/grants", Some("admin-token"), body);
    fixture.request(
        "DELETE",
        "/grants/openrobot:grasp/manipulation.grasp",
        Some("admin-token"),
        "",
    );

    let log = crate::audit::Audit::of(fixture.daemon.store.layout()).read();
    assert_eq!(
        log.entries
            .iter()
            .filter(|entry| entry.action.starts_with("capability."))
            .count(),
        1,
        "one grant happened; the repeat and the revocation of a token nobody held did not"
    );
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
            // This one builds its `Context` by hand rather than through `Fixture::raw`, because
            // it is about the socket rather than about a route — so it takes the recorder the
            // fixture has, which for this test is none.
            blackbox: fixture.recorder.as_mut(),
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

    /// §80's proximity reading, and the two rules worth pinning are both about what is *not*
    /// counted.
    ///
    /// **An interval, not an instant.** A detector at 30 Hz under a 1 kHz control loop leaves `NaN`
    /// in almost every cycle, so a reading taken off the newest one — the way `rhal`, `stop` and
    /// `homing` are taken — would report a working detector as absent nearly always, and flicker.
    ///
    /// **§26.4, one field along.** A cycle whose body read failed carries the previous cycle's
    /// values, so it is counted in neither the numerator nor the denominator: a `NaN` in a fill is
    /// not evidence that nothing produced a reading, and a number in one is not evidence that
    /// something did.
    ///
    /// Driven through `ProximitySighting::of` rather than a disk, for the reason the kernel
    /// condition's suite is: only a control loop can put a cycle in a ring (§26.2), so a test that
    /// had to start one would be a rule nobody could check.
    #[test]
    fn the_proximity_reading_is_counted_over_an_interval_and_skips_a_failed_read() {
        use crate::blackbox::ProximitySighting;
        use skynet_blackbox::record::ReadOutcome;

        fn cycle(read: ReadOutcome, nearest: f32) -> skynet_blackbox::Cycle {
            skynet_blackbox::Cycle {
                tick: 1,
                observation: [0; 32],
                policy: 0,
                state_flags: 0,
                fault_code: 0,
                rhal_result: 0,
                read,
                state_seq: 1,
                state_stamp_ns: 0,
                command_stamp_ns: 0,
                command_seq: 0,
                mode: 0,
                advisory: false,
                response: None,
                tripped: Vec::new(),
                capability: skynet_blackbox::record::NO_CAPABILITY,
                nearest_person_m: nearest,
                base_pose: [f32::NAN; 7],
                base_twist: [0.0; 6],
                desired: Vec::new(),
                command: Vec::new(),
                mask: Vec::new(),
                clamped: Vec::new(),
                position: Vec::new(),
                velocity: Vec::new(),
                effort: Vec::new(),
            }
        }

        // An interval with no cycles in it is not a reading.
        assert!(!ProximitySighting::of(std::iter::empty()).observed());

        // A slow sensor under a fast loop: two of eight cycles carry a number.
        let slow: Vec<_> = (1..=8)
            .map(|tick| cycle(ReadOutcome::Ok, if tick % 4 == 0 { 1.4 } else { f32::NAN }))
            .collect();
        let counted = ProximitySighting::of(slow.iter().cloned());
        assert_eq!(
            counted,
            ProximitySighting::Counted {
                cycles: 8,
                filled: 2
            }
        );
        assert!(
            counted.arriving(),
            "a minority of filled cycles is the expected shape for a slow sensor, not an absence"
        );
        assert!(counted.detail().contains("a producer exists"));

        // Nothing produced one: counted, and not arriving. The count is carried because 0 of 4,000
        // and 0 of 0 are different problems.
        let none: Vec<_> = (1..=8).map(|_| cycle(ReadOutcome::Ok, f32::NAN)).collect();
        let absent = ProximitySighting::of(none.iter().cloned());
        assert_eq!(
            absent,
            ProximitySighting::Counted {
                cycles: 8,
                filled: 0
            }
        );
        assert!(!absent.arriving());
        assert!(absent.detail().contains("none carried"));

        // A failed read carrying a number is counted in neither direction (§26.4).
        let with_fill = vec![
            cycle(ReadOutcome::Ok, f32::NAN),
            cycle(ReadOutcome::Failed(-110), 1.4),
            cycle(ReadOutcome::Unrecorded, 1.4),
        ];
        assert_eq!(
            ProximitySighting::of(with_fill.into_iter()),
            ProximitySighting::Counted {
                cycles: 1,
                filled: 0
            },
            "a fill value is not a reading in either direction (§26.4)"
        );
    }

    /// A supervisor's own ring cannot answer the question at all.
    ///
    /// §26.2 gives `skynetd` the event channel and a control loop the cycle channels, so this is
    /// every shipping robot — and the answer is `Unobservable` rather than *no producer*, because
    /// those are different facts and only one of them is about a sensor.
    #[test]
    fn an_events_only_ring_says_nothing_about_a_proximity_reading() {
        let root = TempRoot::new("proximity-events-only");
        let recorder = open(&root, events_only());
        let sighting = recorder.proximity(0);
        assert!(!sighting.observed());
        assert!(!sighting.arriving());
        assert!(sighting.detail().contains("no action channels"));
    }

    /// §27's `kernel` condition, off the records rather than off systemd.
    ///
    /// The three answers, in the order a robot meets them: a ring that cannot hold a cycle says
    /// **nothing** about a kernel, a ring that can and holds none says the loop is not turning, and
    /// a ring with cycles in it is the only thing that says it is.
    ///
    /// Driven through `from_tally` rather than a disk, because only a control loop can put a cycle
    /// in a ring (§26.2) — which is also the property that makes this evidence worth having:
    /// `skynetd` is structurally incapable of writing the record it reads here.
    #[test]
    fn a_kernel_is_live_only_when_it_recorded_cycles() {
        use crate::blackbox::KernelSighting;
        use skynet_blackbox::tally::Tally;
        use skynet_contracts::opstate::KernelState;

        fn tally(commands: bool, cycles: u64) -> Tally {
            Tally {
                commands,
                cycles,
                ..Tally::default()
            }
        }

        // A ring that could not be read at all. Not a reading, and not a kernel fault.
        let unread = KernelSighting::from_tally(None);
        assert!(!unread.observed());
        assert_eq!(unread.state(), KernelState::Down);

        // Every robot on this edition: no commanding package, so the supervisor's ring has no
        // action channels. Silence from it is not evidence, and §27 still holds — what changes is
        // whether a console sends somebody to restart a kernel that is not the problem.
        let inert = KernelSighting::from_tally(Some(&tally(false, 0)));
        assert!(!inert.observed(), "no channels means no reading");
        assert_eq!(inert.state(), KernelState::Down);
        assert!(inert.detail().contains("no action channels"));

        // A ring a control loop shaped, with nothing in it. Now silence *is* a reading.
        let quiet = KernelSighting::from_tally(Some(&tally(true, 0)));
        assert!(quiet.observed(), "this ring could have held a cycle");
        assert_eq!(quiet.state(), KernelState::Down);
        assert!(quiet.detail().contains("no control cycle was recorded"));

        // And cycles. The only input in the system that separates a turning kernel from a hung
        // one: both are an active unit to systemd, which is why this is not read off systemd.
        let turning = KernelSighting::from_tally(Some(&tally(true, 200)));
        assert!(turning.observed());
        assert_eq!(turning.state(), KernelState::Live);
        assert_eq!(turning, KernelSighting::Turning { cycles: 200 });
        assert!(turning.detail().contains("200 control cycles"));
    }

    /// §27's `rhal` condition, mapped off the newest cycle (§24).
    ///
    /// Five inputs, three states, and the interesting one is `RHAL_FAULT`: §24 defines it as *"the
    /// drive has faulted; commands are being ignored"*, which is a fact about the **body**. A
    /// faulted drive still answers reads, so the kernel has not lost it — and §27's `rhal_lost` is
    /// the row that says nothing else can be known, which would be false at exactly the moment the
    /// robot is telling somebody something.
    #[test]
    fn the_rhal_state_is_the_newest_cycles_reading() {
        use crate::blackbox::RhalSighting;
        use skynet_blackbox::record::ReadOutcome;
        use skynet_contracts::opstate::RhalState;

        fn cycle(read: ReadOutcome, flags: u16, fault_code: u32) -> skynet_blackbox::Cycle {
            skynet_blackbox::Cycle {
                tick: 1,
                observation: [0; 32],
                policy: 0,
                state_flags: flags,
                fault_code,
                rhal_result: 0,
                read,
                state_seq: 1,
                state_stamp_ns: 0,
                command_stamp_ns: 0,
                command_seq: 0,
                mode: 0,
                advisory: false,
                response: None,
                tripped: Vec::new(),
                capability: skynet_blackbox::record::NO_CAPABILITY,
                nearest_person_m: f32::NAN,
                base_pose: [f32::NAN; 7],
                base_twist: [0.0; 6],
                desired: Vec::new(),
                command: Vec::new(),
                mask: Vec::new(),
                clamped: Vec::new(),
                position: Vec::new(),
                velocity: Vec::new(),
                effort: Vec::new(),
            }
        }

        // Nothing read the body at all.
        assert_eq!(RhalSighting::of(None), RhalSighting::Silent);
        assert_eq!(RhalSighting::of(None).state(), RhalState::Lost);

        // A good read, nothing wrong.
        let ok = RhalSighting::of(Some(&cycle(ReadOutcome::Ok, 0, 0)));
        assert_eq!(ok.state(), RhalState::Present);

        // §24's `RHAL_DEGRADED`: running, but something is wrong.
        let degraded = RhalSighting::of(Some(&cycle(ReadOutcome::Ok, 1 << 2, 0)));
        assert_eq!(degraded.state(), RhalState::Degraded);

        // §24's `RHAL_FAULT`. The drive has faulted and the RHAL is fine — the read worked, which
        // is how anybody knows about the fault at all.
        let faulted = RhalSighting::of(Some(&cycle(ReadOutcome::Ok, 1 << 1, 0x2A)));
        assert_eq!(
            faulted.state(),
            RhalState::Present,
            "a faulted drive still answers reads; §27's rhal_lost means nothing can be known"
        );
        assert!(
            faulted.detail().contains("fault code 42"),
            "and the code travels verbatim (§24): {}",
            faulted.detail()
        );

        // The read failed. This is the case §26.4 added a field for, and before it this record was
        // indistinguishable from the healthy one above.
        let lost = RhalSighting::of(Some(&cycle(ReadOutcome::Failed(-110), 0, 0)));
        assert_eq!(lost.state(), RhalState::Lost);
        assert!(lost.detail().contains("-110"));

        // A recording from before the field existed cannot say, and fails closed.
        let older = RhalSighting::of(Some(&cycle(ReadOutcome::Unrecorded, 0, 0)));
        assert_eq!(
            older.state(),
            RhalState::Lost,
            "silence from a schema-1 record is not a good read"
        );
    }

    /// §27's `homing` condition, mapped off §24's two-bit field on the newest cycle.
    ///
    /// The last of the eight to get a producer, and the four cases below are the whole argument
    /// for why the field is two bits rather than one. A single `HOMED` flag would collapse the
    /// first two rows into each other, and they are the pair an operator most needs kept apart:
    ///
    /// | The driver said | This reports | What an operator does |
    /// |---|---|---|
    /// | nothing (a v1 driver) | `Unreported` | update the driver, or ask the vendor |
    /// | `SKR_HOMING_PENDING` | `Reported(Pending)` | run the homing routine |
    /// | `SKR_HOMING_NOT_REQUIRED` | `Reported(NotRequired)` | nothing; the encoders are absolute |
    /// | `SKR_HOMING_COMPLETE` | `Reported(Complete)` | nothing |
    ///
    /// Both of the first two hold the robot, so the distinction costs a sentence and buys an
    /// operator not being sent to home a machine that has no homing routine.
    #[test]
    fn the_homing_state_is_the_newest_cycles_field() {
        use crate::blackbox::HomingSighting;
        use skynet_blackbox::record::ReadOutcome;
        use skynet_contracts::opstate::HomingState;

        fn cycle(read: ReadOutcome, flags: u16) -> skynet_blackbox::Cycle {
            skynet_blackbox::Cycle {
                read,
                tick: 1,
                observation: [0; 32],
                policy: 0,
                state_flags: flags,
                fault_code: 0,
                rhal_result: 0,
                state_seq: 1,
                state_stamp_ns: 0,
                command_stamp_ns: 0,
                command_seq: 0,
                mode: 0,
                advisory: false,
                response: None,
                tripped: Vec::new(),
                capability: skynet_blackbox::record::NO_CAPABILITY,
                nearest_person_m: f32::NAN,
                base_pose: [f32::NAN; 7],
                base_twist: [0.0; 6],
                desired: Vec::new(),
                command: Vec::new(),
                mask: Vec::new(),
                clamped: Vec::new(),
                position: Vec::new(),
                velocity: Vec::new(),
                effort: Vec::new(),
            }
        }

        // Nothing turned, so nothing read the homing state. Not an observation.
        assert!(!HomingSighting::of(None).observed());
        assert_eq!(HomingSighting::of(None).state(), HomingState::Pending);

        // A v1 driver: it set the flags it knows about and left bits 4-5 zero. This *is* an
        // observation — the ring answered and the driver declined — and it is the case the whole
        // encoding exists to keep distinct from the row below.
        let unreported = HomingSighting::of(Some(&cycle(ReadOutcome::Ok, 1 << 3)));
        assert_eq!(unreported, HomingSighting::Unreported);
        assert!(
            unreported.observed(),
            "the daemon looked and the driver answered everything else"
        );
        assert_eq!(
            unreported.state(),
            HomingState::Pending,
            "it still holds the robot; what changes is what an operator is told to do"
        );
        assert!(unreported.detail().contains("has not been updated"));

        // The three states a driver can report, each beside a flag a real driver also sets.
        for (bits, expected) in [
            (1u16 << 4, HomingState::NotRequired),
            (2 << 4, HomingState::Pending),
            (3 << 4, HomingState::Complete),
        ] {
            let seen = HomingSighting::of(Some(&cycle(ReadOutcome::Ok, bits | (1 << 3))));
            assert_eq!(seen, HomingSighting::Reported(expected));
            assert_eq!(seen.state(), expected);
            assert!(seen.observed());
        }

        // §26.4, and the rule this shares with `StopSighting` rather than with `HoldSighting`: a
        // failed read carries the *previous* cycle's flags, so the field in it is a copy. Reading
        // `Complete` off one would release a robot on evidence nobody gathered — which is the one
        // direction this must never fail in.
        let filled = HomingSighting::of(Some(&cycle(ReadOutcome::Failed(-110), 3 << 4)));
        assert!(!filled.observed());
        assert_eq!(
            filled.state(),
            HomingState::Pending,
            "a fill value that says `complete` must not read as homed (§26.4)"
        );
        assert!(filled.detail().contains("fill value"));

        // A recording from before §26.4's field existed cannot say either.
        let older = HomingSighting::of(Some(&cycle(ReadOutcome::Unrecorded, 3 << 4)));
        assert!(!older.observed());
        assert_eq!(older.state(), HomingState::Pending);
    }

    /// The homing field must not be readable out of the flags the other sightings act on.
    ///
    /// The failure this guards is a bit-mask collision, which would be silent and would cross two
    /// subsystems: a braked robot reporting itself homed, or a homed robot reporting an emergency
    /// stop. Both halves would look correct in isolation, which is why it is asserted from the
    /// consumer side as well as inside `skynet-rhal`.
    #[test]
    fn the_homing_field_does_not_disturb_the_stop_or_the_rhal_state() {
        use crate::blackbox::{HomingSighting, RhalSighting, StopSighting};
        use skynet_blackbox::record::ReadOutcome;
        use skynet_contracts::opstate::{HomingState, RhalState};

        fn cycle(flags: u16) -> skynet_blackbox::Cycle {
            skynet_blackbox::Cycle {
                read: ReadOutcome::Ok,
                tick: 1,
                observation: [0; 32],
                policy: 0,
                state_flags: flags,
                fault_code: 0,
                rhal_result: 0,
                state_seq: 1,
                state_stamp_ns: 0,
                command_stamp_ns: 0,
                command_seq: 0,
                mode: 0,
                advisory: false,
                response: None,
                tripped: Vec::new(),
                capability: skynet_blackbox::record::NO_CAPABILITY,
                nearest_person_m: f32::NAN,
                base_pose: [f32::NAN; 7],
                base_twist: [0.0; 6],
                desired: Vec::new(),
                command: Vec::new(),
                mask: Vec::new(),
                clamped: Vec::new(),
                position: Vec::new(),
                velocity: Vec::new(),
                effort: Vec::new(),
            }
        }

        // Homed, and the chain is closed. A mask collision would read the chain as open.
        let homed = cycle(3 << 4);
        assert_eq!(
            HomingSighting::of(Some(&homed)).state(),
            HomingState::Complete
        );
        assert_eq!(StopSighting::of(Some(&homed)), StopSighting::Clear);
        assert_eq!(RhalSighting::of(Some(&homed)).state(), RhalState::Present);

        // Both true at once, and each read independently of the other.
        let homed_and_stopped = cycle((3 << 4) | 1);
        assert_eq!(
            HomingSighting::of(Some(&homed_and_stopped)).state(),
            HomingState::Complete
        );
        assert_eq!(
            StopSighting::of(Some(&homed_and_stopped)),
            StopSighting::Asserted
        );
    }

    /// The events-only ring a shipping robot actually has, read through the real file.
    ///
    /// `from_tally` covers the rule; this covers the path to it, and the case it covers is the one
    /// every robot is in today.
    #[test]
    fn a_supervisors_own_ring_says_nothing_about_a_kernel() {
        let root = TempRoot::new("kernel-sighting");
        let recorder = open(&root, events_only());
        let sighting = recorder.sighting(0);
        assert!(!sighting.observed());
        assert_eq!(
            sighting.state(),
            skynet_contracts::opstate::KernelState::Down
        );
        assert!(sighting.detail().contains("no action channels"));
    }

    /// §26.2: the daemon attaches to a recording a control loop created, and takes its shape.
    ///
    /// This is the asymmetry, asserted where it is decided. A cycle carries a value per joint and
    /// per channel and does not fit a ring shaped for a smaller body, so the loop refuses a
    /// mismatch. An event carries neither, so this recorder writes into whatever the creator sized
    /// — which is what makes **one** recording possible on a robot where the loop knows the body
    /// and the supervisor does not.
    #[test]
    fn the_daemon_attaches_to_a_ring_a_control_loop_shaped() {
        let root = TempRoot::new("blackbox-attach");
        let layout = root.layout();

        // What `skynet-kernel-host` creates: a ring for a body with joints and channels in it.
        let commanding = Shape {
            joints: 6,
            channels: 3,
        };
        let creator = Recorder::open(&layout, supervisor_meta(&layout), commanding, BUDGET)
            .expect("a fresh directory");
        assert_eq!(creator.role(), skynet_lock::Role::Creator);
        assert_eq!(creator.geometry().shape, commanding);

        // What this daemon asks for, and does not get: its own events-only shape.
        let second = Recorder::open(&layout, supervisor_meta(&layout), events_only(), BUDGET);

        #[cfg(target_os = "linux")]
        {
            let mut second = second.expect("the daemon attaches rather than replacing the ring");
            assert_eq!(second.role(), skynet_lock::Role::Attached);
            assert_eq!(
                second.geometry().shape,
                commanding,
                "an attacher takes the ring it finds; it does not reshape it",
            );

            let mut creator = creator;
            let mut taken = Vec::new();
            for n in 0..8u64 {
                taken.push(
                    creator
                        .event(n, EventCode::RecordingStarted, "creator", "x")
                        .expect("the creator writes"),
                );
                taken.push(
                    second
                        .event(n, EventCode::RecordingStarted, "attacher", "x")
                        .expect("the attacher writes"),
                );
            }
            taken.sort_unstable();
            assert_eq!(
                taken,
                (1..=16).collect::<Vec<u64>>(),
                "sixteen records, sixteen sequence numbers, none of them twice",
            );

            assert!(
                second.seal(Reason::Operator, 100).is_err(),
                "the creator seals the window (§26.2)",
            );
            let (sealed, _) = creator
                .seal(Reason::Operator, 100)
                .expect("the creator seals");
            assert!(
                !sealed.bytes.is_empty(),
                "and what it seals is the one recording both wrote",
            );
        }

        #[cfg(not(target_os = "linux"))]
        {
            // No shared counter, so a second writer is refused exactly as it was before §26.2 was
            // built — and, importantly, the creator's ring is **not** replaced on the way out.
            let error = second
                .expect_err("no mapping, no second writer")
                .to_string();
            assert!(
                error.contains("skynetd"),
                "the refusal names the holder: {error}"
            );
            assert_eq!(
                creator.geometry().shape,
                commanding,
                "the refused writer left the recording alone",
            );
        }
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
    // §41.3's events go with the index, not with the file. The arm has to come *before* the
    // download arm in the table, and a table that had them the other way round would answer
    // `Operator` here — silently putting the one route a support conversation starts with behind
    // the role most people watching a robot do not hold.
    assert_eq!(
        requirement("GET", &["blackbox", "events"]).0,
        Role::Observer
    );
}

/// A recorder over an events-only ring, for §41.3's list.
fn events_recorder(label: &str) -> (TempRoot, crate::blackbox::Recorder) {
    let root = TempRoot::new(label);
    let layout = root.layout();
    let recorder = crate::blackbox::Recorder::open(
        &layout,
        crate::blackbox::supervisor_meta(&layout),
        skynet_blackbox::record::Shape {
            joints: 0,
            channels: 0,
        },
        64 * 1024,
    )
    .expect("recorder");
    (root, recorder)
}

/// §41.3's events: what happened, in order, with the gaps left in.
#[test]
fn the_event_list_carries_the_gaps_and_says_what_it_left_out() {
    use skynet_blackbox::record::{EventCode, Gap, GapReason};

    let (_root, mut recorder) = events_recorder("bb-events");

    // Six events and, between the third and the fourth, a gap. The gap is the point: two events
    // either side of four hundred dropped records read as consecutive unless the hole is in the
    // list with them.
    for index in 1..4u64 {
        recorder
            .event(index, EventCode::LeaseGranted, "acme/pick", "granted")
            .expect("event");
    }
    recorder
        .gap(
            4,
            Gap {
                dropped: 400,
                first_tick: 900,
                reason: GapReason::WriterBehind,
            },
        )
        .expect("gap");
    for index in 5..8u64 {
        recorder
            .event(index, EventCode::LeaseReleased, "acme/pick", "released")
            .expect("event");
    }

    let all = recorder.events(50);
    let rows = all.arr_or_empty("events");
    assert_eq!(rows.len(), 7, "six events and the gap between them");
    assert_eq!(all.get("held").and_then(Json::as_f64), Some(7.0));
    // Newest last: a list read top to bottom ends where the robot is now.
    assert_eq!(
        rows.last()
            .and_then(|row| row.get("detail"))
            .and_then(Json::as_str),
        Some("released")
    );
    // The gap is in its place, neither appended nor dropped.
    let gap = &rows[3];
    assert_eq!(gap.get("kind").and_then(Json::as_str), Some("gap"));
    assert_eq!(gap.get("dropped").and_then(Json::as_f64), Some(400.0));

    // A limit takes the *recent* end. Taking the oldest is the natural way to write this and would
    // answer the opposite of the question — "what happened before it stopped".
    let last_two = recorder.events(2);
    let rows = last_two.arr_or_empty("events");
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].get("detail").and_then(Json::as_str),
        Some("released")
    );
    // And `held` still reports the whole ring, so a two-row answer is never read as a robot that
    // has done two things.
    assert_eq!(last_two.get("held").and_then(Json::as_f64), Some(7.0));
}

// **Cycles are excluded, and that is asserted where cycles exist.** `Recorder` here writes events
// and gaps and nothing else — the kernel host is what records a cycle (§26.2's two roles) — so a
// unit test in this crate cannot put one in a ring to leave out of a list. `e2e.sh` can and does:
// it runs `skynetk` against the mock driver, which fills a ring with over a thousand cycles, and
// then asks `skynet blackbox events` what it holds. A test that cannot construct the thing it is
// excluding proves nothing about the exclusion.

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
        /// §36.1's `low` row, when the test puts the robot in it. `None` on a healthy robot, which
        /// is what `World::new` builds and what every test that is about something else gets.
        power_withheld: Option<String>,
        /// §13's ring, as the caller would have counted it. `None` is nothing counted.
        interventions_since: Option<u64>,
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
                power_withheld: None,
                // Nothing counted, which is what a robot with no recording reports and what every
                // test that does not set it should see.
                interventions_since: None,
            }
        }

        fn step(&mut self, runtime: &mut Runtime, by_ms: u64) -> Vec<crate::skill::Report> {
            self.now_ms += by_ms;
            let mut cycle = Cycle {
                now_ms: self.now_ms,
                inputs: &self.inputs,
                robot: self.robot,
                interventions_since: self.interventions_since,
                // A fixed wall clock, so `recorded_at` is a value the suite can assert on rather
                // than whatever the machine running the tests thinks the date is.
                wall_ms: Some(1_788_307_200_000 + self.now_ms),
                running: &self.running,
                leases: &mut self.leases,
                grants: &self.grants,
                power_withheld: self.power_withheld.clone(),
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

    /// Walk one invocation to its episode, reporting `per_pass[i]` on the i-th pass after `running`.
    ///
    /// Every pass's effects are collected rather than only the last one's, because §54.4's diagram
    /// ends `running` on a guard trip: an invocation that sees an intervention emits its episode
    /// *at that pass* and the passes after it do nothing at all.
    fn episode_with(before: Option<u64>, per_pass: &[Option<u64>]) -> Json {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let m = skill_manifest("acme/pick", Some(10.0), &[]);
        world.interventions_since = before;

        let _ = invoke(&mut runtime, &m, &[]);
        let mut episodes: Vec<Json> = Vec::new();
        let collect = |reports: Vec<crate::skill::Report>, into: &mut Vec<Json>| {
            for effect in reports.iter().flat_map(|report| report.effects.iter()) {
                if let Effect::Episode { body } = effect {
                    into.push(body.clone());
                }
            }
        };

        for _ in 0..3 {
            // preconditions, acquiring, running
            collect(world.step(&mut runtime, 1), &mut episodes);
        }

        for count in per_pass {
            world.interventions_since = *count;
            collect(world.step(&mut runtime, 1), &mut episodes);
        }

        // The ordinary ending, for an invocation no guard trip has already finished.
        world.running.remove("acme/pick");
        collect(world.step(&mut runtime, 1), &mut episodes);
        collect(world.step(&mut runtime, 1), &mut episodes);

        assert_eq!(episodes.len(), 1, "§54.4: one episode per invocation");
        episodes.remove(0)
    }

    /// §54.4's intervention count, off §13's ring rather than off a boolean.
    ///
    /// **Added, not edge-detected**, and one pass is enough to show the difference. A reconcile pass
    /// is not a control cycle: the ring can hold three interventions inside one interval, and the
    /// boolean this replaced could only say *something tripped* — so the episode recorded one.
    ///
    /// One pass rather than several because §54.4's diagram ends `running` on a guard trip: the
    /// first interval that reports an intervention is also the last one this invocation sees. That
    /// is the shape of the real thing, and it is why the count has to be right on a single pass
    /// rather than accumulated over many.
    #[test]
    fn an_episodes_interventions_are_the_rings_count_and_not_an_edge() {
        let episode = episode_with(Some(0), &[Some(3)]);
        assert_eq!(
            episode.get("interventions").and_then(Json::as_f64),
            Some(3.0),
            "three cycles the kernel intervened in, not one rising edge"
        );
        // And the trip is what ended it, which is §54.4's diagram rather than this counter.
        assert_eq!(
            episode.get("outcome").and_then(Json::as_str),
            Some("aborted")
        );
    }

    /// A skill that ran and tripped nothing reports **zero**, which is the fact.
    ///
    /// The half of §74.3 this change makes reachable: with a recording to count, "no guard tripped
    /// during this skill" is a measurement rather than an absence, and it is the number a
    /// marketplace is entitled to quote.
    #[test]
    fn a_skill_that_tripped_nothing_reports_zero_rather_than_nothing() {
        let episode = episode_with(Some(0), &[Some(0), Some(0)]);
        assert_eq!(
            episode.get("interventions").and_then(Json::as_f64),
            Some(0.0)
        );
    }

    /// Nothing counted keeps the field **out** of the episode rather than in it as a zero.
    ///
    /// §74.3 at the boundary it crosses. A zero here is read by a marketplace as evidence of a clean
    /// run — the number that gets misread precisely because it looks like good news — and §76.3 is
    /// three sessions of care about this one column.
    #[test]
    fn an_uncounted_invocation_omits_the_field_and_says_it_was_not_measured() {
        let episode = episode_with(None, &[None, None]);
        assert!(
            episode.get("interventions").is_none(),
            "a robot with no recording to count does not report a count"
        );
        assert_eq!(
            episode
                .get("metadata")
                .and_then(|metadata| metadata.get("interventions_measured")),
            Some(&Json::Bool(false)),
            "and it says so, so a Hub can tell it from a Hub that dropped the field"
        );
    }

    /// A recording that stops being countable mid-invocation unmeasures the whole episode.
    ///
    /// Not a partial sum. The count is over the invocation, and an invocation with a hole in it has
    /// a floor rather than a total — the same rule `Tally::complete` applies one layer down, and the
    /// same reason: a number that is quietly smaller than the truth is worse than no number.
    #[test]
    fn an_invocation_that_loses_its_recording_reports_nothing_rather_than_a_partial_sum() {
        let episode = episode_with(Some(0), &[Some(0), None]);
        assert!(
            episode.get("interventions").is_none(),
            "a counted pass and an uncounted one is not a total"
        );
    }

    /// …and a recording that **comes back** does not undo that.
    ///
    /// The case the test above does not reach, because its hole is on the last pass. Until
    /// 2026-09-16 an uncounted pass set the count to `None` and the next counted one revived it from
    /// zero, so an invocation with a hole in the middle reported the passes after the hole as its
    /// whole — here, a clean zero over an interval nobody watched.
    #[test]
    fn a_recording_that_returns_does_not_turn_a_floor_back_into_a_total() {
        let episode = episode_with(Some(0), &[None, Some(0)]);
        assert!(
            episode.get("interventions").is_none(),
            "a hole in the middle leaves the episode unmeasured: {}",
            episode.to_canonical().unwrap_or_default()
        );
        assert_eq!(
            episode
                .get("metadata")
                .and_then(|metadata| metadata.get("interventions_measured")),
            Some(&Json::Bool(false))
        );
    }

    /// A count wider than the episode's field saturates, and never wraps to a clean run.
    ///
    /// The ring counts in `u64` and the episode carries `u32`. The cast that joined them was `as`,
    /// which keeps the low bits — so 2³² interventions arrived as **zero**, the one value a
    /// marketplace reads as a perfect run. Unreachable from a working ring, and exactly what a
    /// corrupt one would produce.
    #[test]
    fn a_count_too_wide_for_the_episode_saturates_rather_than_wrapping_to_zero() {
        let episode = episode_with(Some(0), &[Some(1 << 32)]);
        assert_eq!(
            episode.get("interventions").and_then(Json::as_f64),
            Some(f64::from(u32::MAX))
        );
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
        let metadata = body
            .get("metadata")
            .expect("the facts the route has no column for");
        assert_eq!(
            metadata.get("reason").and_then(Json::as_str),
            Some("preconditions_refused")
        );
        assert!(
            body.get("duration_seconds").is_none(),
            "nothing ran, so there is no duration to report"
        );
        assert!(
            body.get("interventions").is_none(),
            "and nothing counts interventions on this robot, so the count is absent, not zero"
        );
        assert_eq!(
            metadata.get("interventions_measured"),
            Some(&Json::Bool(false)),
            "said out loud, because an absent count and a Hub default read the same downstream"
        );
    }

    /// §54.4's episode, frozen for the Hub's suite to read (`reference/wire/episode.json`).
    ///
    /// In this module rather than beside the other two fixtures, because the only honest way to
    /// produce this document is to walk §54.4's ladder — and the helpers that do that are here.
    #[test]
    fn the_episode_a_skill_invocation_produces() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let mut m = skill_manifest("acme/pick", Some(10.0), &[]);
        // A real skill card declares what it does, and the frozen document should carry the case a
        // Hub actually stores rather than the fallback.
        m.set("task", Json::from("pick the cup off the table"));
        invoke(&mut runtime, &m, &[]);

        let mut body = None;
        for pass in 0..8 {
            if pass == 3 {
                world.running.remove("acme/pick");
            }
            for report in world.step(&mut runtime, 1_000) {
                for effect in &report.effects {
                    if let Effect::Episode { body: document } = effect {
                        body = Some(document.clone());
                    }
                }
            }
            if body.is_some() {
                break;
            }
        }

        super::frozen("episode", &body.expect("one episode per invocation"));
    }

    /// The document is the route's, not one of the robot's own.
    ///
    /// Written after the two halves were compared for the first time. The robot sent `skill`,
    /// `duration_ms` and `intervention_count`; `POST /episodes` reads `task`, `duration_seconds`
    /// and `interventions`, ignores what it does not know, and answers `201`. Every episode would
    /// have stored a null task, a null duration and none of the skill — successfully, with nothing
    /// on either side wrong in isolation and no suite able to see it.
    ///
    /// The field names in this test are copied from `lib/skynet/devices.ts`'s `validateEpisodeInput`
    /// rather than from the producer, which is the only way a test like this can be worth anything.
    #[test]
    fn an_episode_is_written_in_the_shape_the_hub_route_reads() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let m = skill_manifest("acme/wave", Some(10.0), &[]);
        invoke(&mut runtime, &m, &[]);

        // Through the ladder: preconditions, acquiring, running — then the unit exits and §54.3
        // judges it.
        let mut episode = None;
        for pass in 0..8 {
            if pass == 4 {
                world.running.remove("acme/wave");
            }
            for report in world.step(&mut runtime, 1_000) {
                for effect in &report.effects {
                    if let Effect::Episode { body } = effect {
                        episode = Some(body.clone());
                    }
                }
            }
            if episode.is_some() {
                break;
            }
        }
        let body = episode.expect("one episode per invocation");

        assert_eq!(
            body.get("task").and_then(Json::as_str),
            Some("acme/wave"),
            "the column the Hub stores, not a `skill` key it has never read"
        );
        assert!(
            skynet_contracts::skill::SKILL_OUTCOMES
                .iter()
                .any(|outcome| {
                    Some(outcome.as_str()) == body.get("outcome").and_then(Json::as_str)
                }),
            "the outcome is one of the four the Hub's CHECK constraint permits"
        );
        assert_eq!(body.get("source").and_then(Json::as_str), Some("robot"));
        assert!(
            body.get("recorded_at")
                .and_then(Json::as_str)
                .is_some_and(|stamp| stamp.starts_with("2026-") && stamp.ends_with('Z')),
            "when it happened, not when the queue drained: {:?}",
            body.get("recorded_at")
        );
        let metadata = body.get("metadata").expect("metadata");
        assert_eq!(
            metadata.get("skill").and_then(Json::as_str),
            Some("acme/wave")
        );
        assert_eq!(
            metadata.get("digest").and_then(Json::as_str),
            Some("sha256:test"),
            "an investigation needs the bytes that ran"
        );
        assert!(metadata.get("invocation").is_some());
    }

    /// §17's `task` column means *what the robot was doing*, and a skill card declares it.
    ///
    /// Found by reading the Hub's manifest schema against what this robot consumes — `task` was
    /// validated, stored and shipped on every skill card, and no robot had ever read one. Every
    /// episode carried the package id instead, which is an identifier rather than a description: a
    /// dataset whose task column holds `acme/pick` on all forty thousand rows cannot be filtered by
    /// what was being done, which is the one question a training set is asked.
    #[test]
    fn an_episode_carries_the_task_the_skill_declares() {
        let declared = |task: Option<&str>| {
            let mut runtime = Runtime::new();
            let mut world = World::new();
            let mut m = skill_manifest("acme/pick", Some(10.0), &[]);
            if let Some(task) = task {
                m.set("task", Json::from(task));
            }
            invoke(&mut runtime, &m, &[]);
            let mut body = None;
            for pass in 0..8 {
                if pass == 3 {
                    world.running.remove("acme/pick");
                }
                for report in world.step(&mut runtime, 1_000) {
                    for effect in &report.effects {
                        if let Effect::Episode { body: document } = effect {
                            body = Some(document.clone());
                        }
                    }
                }
                if body.is_some() {
                    break;
                }
            }
            body.expect("an episode")
        };

        let with = declared(Some("  pick the cup off the table  "));
        assert_eq!(
            with.get("task").and_then(Json::as_str),
            Some("pick the cup off the table"),
            "trimmed, and it is the declaration rather than the id"
        );
        assert_eq!(
            with.get("metadata")
                .and_then(|m| m.get("skill"))
                .and_then(Json::as_str),
            Some("acme/pick"),
            "and the id is still there, because grouping needs an identifier"
        );

        // No declaration, and the fallback is the id: a poor task label and a correct one, where
        // the alternative is a null column on every row.
        let without = declared(None);
        assert_eq!(
            without.get("task").and_then(Json::as_str),
            Some("acme/pick")
        );

        // Whitespace is not a declaration.
        let blank = declared(Some("   "));
        assert_eq!(blank.get("task").and_then(Json::as_str), Some("acme/pick"));
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

    /// §36.1's `low` row, through the runtime that had never obeyed it.
    ///
    /// The whole row in one test: the skill is refused, the sentence names the power state, and the
    /// **lease table is not touched** — a refusal that had taken the token first would be holding an
    /// arm for an invocation that is about to abort.
    #[test]
    fn a_robot_withholding_power_refuses_a_new_skill_without_touching_the_arm() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        world.power_withheld = Some(
            "§36.1 withholds new task leases: this robot's power state is `reserve` (at_reserve)"
                .to_string(),
        );
        let m = skill_manifest("acme/pick", Some(10.0), &["arm.left"]);
        world
            .grants
            .push(("acme/pick".to_string(), "arm.left".to_string()));

        let id = invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        let reports = world.step(&mut runtime, 1);

        assert_eq!(phases(&reports), ["terminal"]);
        assert_eq!(reports[0].transition.reason.as_str(), "power_withheld");
        assert!(
            reports[0].transition.detail.contains("`reserve`"),
            "the refusal names the state, so an operator knows to fetch a charger rather than to \
             hunt for the package holding the arm: {}",
            reports[0].transition.detail
        );
        assert_eq!(
            world.leases.holder("arm.left"),
            None,
            "the token was never taken — a refusal that acquired first would hold an arm for an \
             invocation that is aborting"
        );
        assert_eq!(
            runtime.get(&id).unwrap().outcome,
            Some(SkillOutcome::Aborted)
        );
    }

    /// The other half of §36.1's row: *a running task may finish.*
    ///
    /// The one that would be missed by a check placed anywhere but the `acquiring` stage — a robot
    /// that fell to `reserve` mid-skill must not have the arm taken out from under a policy that is
    /// mid-motion, because a policy dropped mid-motion is the failure the controlled stop exists to
    /// avoid.
    #[test]
    fn a_skill_already_holding_its_leases_is_not_stopped_when_power_falls() {
        let mut runtime = Runtime::new();
        let mut world = World::new();
        let m = skill_manifest("acme/pick", Some(10.0), &["arm.left"]);
        world
            .grants
            .push(("acme/pick".to_string(), "arm.left".to_string()));

        let id = invoke(&mut runtime, &m, &[]);
        world.step(&mut runtime, 1);
        world.step(&mut runtime, 1);
        let reports = world.step(&mut runtime, 1);
        assert_eq!(
            phases(&reports),
            ["running"],
            "it started on a full battery"
        );
        assert_eq!(world.leases.holder("arm.left"), Some("acme/pick"));

        // And now the battery falls.
        world.power_withheld = Some("§36.1 withholds new task leases".to_string());
        let reports = world.step(&mut runtime, 1);
        assert_eq!(
            phases(&reports),
            ["running"],
            "§36.1: a running task may finish"
        );
        assert_eq!(
            world.leases.holder("arm.left"),
            Some("acme/pick"),
            "and it keeps the arm it is using"
        );
        assert!(!runtime.get(&id).unwrap().terminal());
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

        let reports = runtime.stop_all(world.now_ms, Some(1_788_307_200_000));
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
        assert!(runtime.stop_all(world.now_ms, None).is_empty());
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

/// §54.4's `running` row through the daemon rather than the contract: entering `running` starts
/// the skill's unit on the ordinary supervisor path — the same `start()` a desired state uses,
/// so with the same budget, cpuset, rendezvous and confinement — and leaving it stops the unit
/// the same way. The contract suite proves the lifecycle emits the effects; this proves the
/// daemon performs them, which is the half a skill that never starts a policy would be missing.
#[test]
#[cfg(unix)]
fn a_skill_entering_running_starts_its_unit_and_leaving_it_stops_it() {
    let mut fixture = Fixture::new("skill-starts-unit");
    fixture.provision();
    install_skill(&fixture, "acme/pick", |_| {});

    let (status, _) = fixture.request(
        "POST",
        "/skills",
        Some("admin-token"),
        r#"{"skill":"acme/pick"}"#,
    );
    assert_eq!(status, 202);

    // idle → preconditions → acquiring: two advances, and nothing has started, because a unit
    // started before `running` would be a skill running that never cleared acquiring.
    for now_ms in [1_001, 1_002] {
        let actual = fixture.daemon.observe(now_ms);
        fixture.daemon.advance_skills(now_ms, None, &actual, None);
    }
    assert!(
        !fixture
            .daemon
            .launcher
            .calls
            .iter()
            .any(|call| call == "start acme/pick"),
        "calls: {:?}",
        fixture.daemon.launcher.calls
    );

    // The third advance enters `running`, and the daemon performs the `Start` on the pass that
    // emitted it rather than leaving it for the reconciler to discover.
    let actual = fixture.daemon.observe(1_003);
    fixture.daemon.advance_skills(1_003, None, &actual, None);
    assert!(
        fixture
            .daemon
            .launcher
            .calls
            .iter()
            .any(|call| call == "start acme/pick"),
        "calls: {:?}",
        fixture.daemon.launcher.calls
    );

    // The unit exits: the skill returned. Observed, not assumed — the lifecycle reads the
    // process table the supervisor changes, which is why `advance_skills` runs after the
    // reconciler in the main loop rather than before it.
    let unit = fixture
        .daemon
        .launcher
        .units
        .iter_mut()
        .find(|unit| unit.id == "acme/pick")
        .expect("the start above registered the unit");
    unit.state = UnitState::Stopped;
    // Gone → `verifying` → `terminal`: two advances, and the terminal transition stops it.
    for now_ms in [1_004, 1_005] {
        let actual = fixture.daemon.observe(now_ms);
        fixture.daemon.advance_skills(now_ms, None, &actual, None);
    }
    assert!(
        fixture
            .daemon
            .launcher
            .calls
            .iter()
            .any(|call| call == "stop acme/pick"),
        "calls: {:?}",
        fixture.daemon.launcher.calls
    );
}

/// The reconciler's half of §54.4, through the daemon the way the main loop drives it:
/// reconcile, then advance. A unit a live invocation started is not the plan's to stop, so a
/// pass taken while the skill is running must hold no stop for it — otherwise every skill on
/// a looping daemon lives exactly one interval. When the invocation ends, the lifecycle's own
/// `Stop` performs the stop (proved above), so sparing here strands nothing.
#[test]
#[cfg(unix)]
fn reconcile_spares_a_unit_while_its_skill_is_live() {
    let mut fixture = Fixture::new("skill-spared");
    fixture.provision();
    let digest = install_skill(&fixture, "acme/pick", |_| {});
    // Named in desired state as installed but not running: the Hub wants the skill present
    // and the invocation — not the reconciler — decides when it runs. Without this entry
    // the pass below would collect the activation (§37.3) before the skill ever starts,
    // which is the same removal an unnamed package gets and the test would prove nothing.
    fixture.write_desired(&format!(
        r#"{{"generation":7,"os":{{"channel":"stable","version":""}},
             "packages":[{{"id":"acme/pick","version":"1.0.0","digest":"sha256:{digest}",
                           "kind":"skill","state":"installed","grants":[]}}]}}"#
    ));
    fixture.daemon.reload();

    let (status, _) = fixture.request(
        "POST",
        "/skills",
        Some("admin-token"),
        r#"{"skill":"acme/pick"}"#,
    );
    assert_eq!(status, 202);

    // The loop's order, three times: the reconcile sees what the last advance did, and the
    // advance reads the process table the reconcile changed.
    for now_ms in [1_001, 1_002, 1_003] {
        fixture.daemon.reconcile(now_ms);
        let actual = fixture.daemon.observe(now_ms);
        fixture.daemon.advance_skills(now_ms, None, &actual, None);
    }
    assert!(
        fixture
            .daemon
            .launcher
            .calls
            .iter()
            .any(|call| call == "start acme/pick"),
        "the skill is running: {:?}",
        fixture.daemon.launcher.calls
    );

    // The next pass observes a unit desired state never named — and must not stop it.
    let pass = fixture.daemon.reconcile(1_004);
    assert!(
        !pass.plan.actions.iter().any(|action| matches!(
            &action.kind,
            skynetd::reconcile::ActionKind::StopPackage { id } if id == "acme/pick"
        )),
        "a live skill owns its unit"
    );
    assert!(
        !pass.applied.iter().any(|(action, _)| matches!(
            &action.kind,
            skynetd::reconcile::ActionKind::StopPackage { id } if id == "acme/pick"
        )),
        "neither planned nor applied"
    );
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

    let inputs = fixture.daemon.live_inputs(None, None);
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
    let inputs = fixture.daemon.live_inputs(None, None);
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

    // ── §12's bytes, through the reconciler ──────────────────────────────────
    //
    // The tests above drive `crate::bundle` directly. These drive the action: a robot told to run a
    // version it does not have, reconciling until it does — and the case that had gone unchecked for
    // twenty-five sessions, where the bytes on the disk are not the bytes the gate ran against.

    const IMAGE: &[u8] = b"an operating system, for the purposes of this suite";

    fn image_digest() -> String {
        format!(
            "sha256:{}",
            String::from_utf8_lossy(&skynet_sha256::hex_bytes(IMAGE))
        )
    }

    /// A release manifest naming the digest of [`IMAGE`], signed by a key this robot trusts.
    fn with_real_release(label: &str) -> Fixture {
        let mut fixture = Fixture::new(label);
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        write_trust(&fixture, "publisher", &public_hex(&driver_key(7)));
        let mut document = release_document(&["userspace"]);
        document.set("digest", Json::from(image_digest().as_str()));
        write_signed(&fixture, &layout.release("1.4.0"), &document, 7);
        fixture.write_desired(
            r#"{"generation":1,"os":{"channel":"stable","version":"1.4.0"},"packages":[]}"#,
        );
        fixture.daemon.reload();
        fixture
    }

    /// Pair the robot, so §34.2 has somewhere to ask.
    fn pair_to(fixture: &Fixture, endpoint: &str) {
        let layout = fixture.daemon.store.layout();
        std::fs::create_dir_all(layout.identity()).unwrap();
        std::fs::write(
            layout.device(),
            format!(
                "{{\"device_id\":\"dev_7\",\"namespace\":\"acme\",\"endpoint\":\"{endpoint}\"}}"
            ),
        )
        .unwrap();
        std::fs::write(layout.credential(), "skd_secretvalue").unwrap();
    }

    fn on_ethernet(fixture: &Fixture) {
        let path = fixture.daemon.store.layout().state().join("bearers.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "{\"bearers\":[{\"name\":\"eth0\",\"kind\":\"ethernet\",\"up\":true,\"metered\":false}]}",
        )
        .unwrap();
    }

    /// An updater the test can read back, since the daemon owns the box it is put in.
    #[derive(Default)]
    struct SharedUpdater(std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>);

    impl crate::ota::Updater for SharedUpdater {
        fn install(&mut self, bundle: &std::path::Path) -> Result<(), crate::ota::UpdateError> {
            self.0.lock().unwrap().push(bundle.to_path_buf());
            Ok(())
        }

        fn mark_good(&mut self) -> Result<(), crate::ota::UpdateError> {
            Ok(())
        }

        fn mark_bad(&mut self) -> Result<(), crate::ota::UpdateError> {
            Ok(())
        }
    }

    /// A fetcher that hands over one fixed body.
    struct Serving(&'static [u8]);

    impl crate::bundle::Fetcher for Serving {
        fn slice(&mut self, ask: &crate::bundle::Ask) -> crate::bundle::Slice {
            std::fs::create_dir_all(ask.into.parent().unwrap()).unwrap();
            std::fs::write(&ask.into, self.0).unwrap();
            crate::bundle::Slice {
                status: 200,
                before: 0,
                after: self.0.len() as u64,
                declared_total: Some(self.0.len() as u64),
                complete: true,
                error: None,
            }
        }
    }

    /// The whole loop: told to run a version, fetches it, checks it, installs it.
    #[test]
    fn a_robot_told_to_run_a_version_it_does_not_have_goes_and_gets_it() {
        let mut fixture = with_real_release("ota-fetch-e2e");
        pair_to(&fixture, "https://hub.example");
        on_ethernet(&fixture);
        let layout = fixture.daemon.store.layout().clone();
        fixture.daemon.fetcher = Box::new(Serving(IMAGE));
        let installs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        fixture.daemon.updater = Box::new(SharedUpdater(installs.clone()));

        let pass = fixture.daemon.reconcile(1_000);
        let (_, outcome) = pass
            .applied
            .iter()
            .find(|(action, _)| {
                matches!(
                    action.kind,
                    skynetd::reconcile::ActionKind::StageOsImage { .. }
                )
            })
            .expect("the plan stages the image");
        assert_eq!(outcome, &Outcome::Applied, "{outcome:?}");
        assert_eq!(
            installs.lock().unwrap().as_slice(),
            [layout.bundle(&image_digest())],
            "RAUC is handed the file the digest names"
        );
        assert_eq!(
            std::fs::read(layout.bundle(&image_digest())).unwrap(),
            IMAGE
        );
    }

    /// §34.3 through the action: a wait is not a failure, and the reason reaches the pass.
    #[test]
    fn a_robot_with_no_link_reports_waiting_rather_than_failing() {
        let mut fixture = with_real_release("ota-fetch-offline");
        pair_to(&fixture, "https://hub.example");
        let path = fixture.daemon.store.layout().state().join("bearers.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{\"bearers\":[]}").unwrap();

        let pass = fixture.daemon.reconcile(1_000);
        let (_, outcome) = pass
            .applied
            .iter()
            .find(|(action, _)| {
                matches!(
                    action.kind,
                    skynetd::reconcile::ActionKind::StageOsImage { .. }
                )
            })
            .expect("the plan stages the image");
        match outcome {
            Outcome::Waiting { reason } => assert!(reason.contains("ordinary state"), "{reason}"),
            other => panic!("{other:?}"),
        }
        assert!(
            !pass.converged(),
            "and the robot does not report itself up to date"
        );
    }

    /// An unpaired robot has nowhere to ask, which is a wait naming the command that fixes it.
    #[test]
    fn an_unpaired_robot_says_where_the_bytes_would_have_to_come_from() {
        let mut fixture = with_real_release("ota-fetch-unpaired");
        on_ethernet(&fixture);

        let pass = fixture.daemon.reconcile(1_000);
        let (_, outcome) = pass
            .applied
            .iter()
            .find(|(action, _)| {
                matches!(
                    action.kind,
                    skynetd::reconcile::ActionKind::StageOsImage { .. }
                )
            })
            .expect("the plan stages the image");
        match outcome {
            Outcome::Waiting { reason } => {
                assert!(reason.contains("skynet pair"), "{reason}");
                assert!(reason.contains("by hand"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
    }

    /// The hole this session found rather than filled.
    ///
    /// Every gate §12 applies is bound to the manifest's digest, and for twenty-five sessions
    /// nothing compared that digest against the bytes RAUC was handed — so a bundle placed on the
    /// disk by hand, which was the *only* way to update a robot, went in unchecked. The manifest is
    /// signed, the release is admitted, and the file is somebody else's image.
    #[test]
    fn bytes_on_the_disk_are_held_to_the_digest_the_gate_ran_against() {
        let mut fixture = with_real_release("ota-hand-placed");
        let layout = fixture.daemon.store.layout().clone();
        let bundle = layout.bundle(&image_digest());
        std::fs::create_dir_all(bundle.parent().unwrap()).unwrap();
        std::fs::write(&bundle, b"a different operating system entirely").unwrap();
        let installs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        fixture.daemon.updater = Box::new(SharedUpdater(installs.clone()));

        let pass = fixture.daemon.reconcile(1_000);
        let (_, outcome) = pass
            .applied
            .iter()
            .find(|(action, _)| {
                matches!(
                    action.kind,
                    skynetd::reconcile::ActionKind::StageOsImage { .. }
                )
            })
            .expect("the plan stages the image");
        match outcome {
            Outcome::Failed { reason } => {
                assert!(reason.contains("is not release `1.4.0`"), "{reason}");
                assert!(reason.contains("do not hash"), "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            installs.lock().unwrap().is_empty(),
            "nothing reached the bootloader"
        );
        assert!(
            !bundle.exists(),
            "and the bytes are gone: an unverified bundle left on the disk is an artifact some \
             future code path finds"
        );
    }

    /// The refusal comes before the transfer, so nobody pays for bytes to reach a `no`.
    #[test]
    fn a_gated_release_is_refused_without_anything_being_fetched() {
        let mut fixture = Fixture::new("ota-gate-before-bytes");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        write_trust(&fixture, "publisher", &public_hex(&driver_key(7)));
        let mut document = release_document(&["safety_kernel"]);
        document.set("digest", Json::from(image_digest().as_str()));
        write_signed(&fixture, &layout.release("1.4.0"), &document, 7);
        fixture.write_desired(
            r#"{"generation":1,"os":{"channel":"stable","version":"1.4.0"},"packages":[]}"#,
        );
        fixture.daemon.reload();
        pair_to(&fixture, "https://hub.example");
        on_ethernet(&fixture);

        struct Never;
        impl crate::bundle::Fetcher for Never {
            fn slice(&mut self, _ask: &crate::bundle::Ask) -> crate::bundle::Slice {
                panic!("§12's gate must run before a byte moves");
            }
        }
        fixture.daemon.fetcher = Box::new(Never);

        let pass = fixture.daemon.reconcile(1_000);
        let (_, outcome) = pass
            .applied
            .iter()
            .find(|(action, _)| {
                matches!(
                    action.kind,
                    skynetd::reconcile::ActionKind::StageOsImage { .. }
                )
            })
            .expect("the plan stages the image");
        assert!(matches!(outcome, Outcome::Failed { .. }), "{outcome:?}");
    }

    /// The size the publisher signed, reaching the two places that want it (§75.5).
    ///
    /// This was the named gap when the fetcher was written: the contract has a branch for a
    /// declared size and §57's manifest carried none, so the branch was unexercised and progress
    /// was a count rather than a fraction. Both are closed by one field, and this is the assertion
    /// that it travels — from the signed document, through `declared_size`, onto the route.
    #[test]
    fn a_manifest_that_declares_a_size_makes_progress_a_fraction() {
        let mut fixture = Fixture::new("ota-size");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        write_trust(&fixture, "publisher", &public_hex(&driver_key(7)));
        let mut document = release_document(&["userspace"]);
        document.set("digest", Json::from(image_digest().as_str()));
        document.set("size", Json::Num(IMAGE.len() as f64));
        write_signed(&fixture, &layout.release("1.4.0"), &document, 7);
        fixture.write_desired(
            r#"{"generation":1,"os":{"channel":"stable","version":"1.4.0"},"packages":[]}"#,
        );
        fixture.daemon.reload();

        std::fs::create_dir_all(layout.partial(&image_digest()).parent().unwrap()).unwrap();
        std::fs::write(layout.partial(&image_digest()), &IMAGE[..10]).unwrap();

        let (status, body) = fixture.request("GET", "/ota", Some("observer-token"), "");
        assert_eq!(status, 200);
        let transfer = body.get("transfer").expect("a transfer block");
        assert_eq!(
            transfer.get("total").and_then(Json::as_f64),
            Some(IMAGE.len() as f64)
        );
        assert_eq!(
            transfer.get("total_declared_by").and_then(Json::as_str),
            Some("manifest"),
            "whose number it is, because a publisher's signed size and a server's claim about its \
             own file are different kinds of statement"
        );
        assert_eq!(transfer.get("received").and_then(Json::as_f64), Some(10.0));
    }

    /// The branch the declared size exists for.
    #[test]
    fn a_partial_larger_than_the_declared_bundle_is_thrown_away_before_a_transfer() {
        let mut fixture = Fixture::new("ota-size-oversized");
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        write_trust(&fixture, "publisher", &public_hex(&driver_key(7)));
        let mut document = release_document(&["userspace"]);
        document.set("digest", Json::from(image_digest().as_str()));
        document.set("size", Json::Num(IMAGE.len() as f64));
        write_signed(&fixture, &layout.release("1.4.0"), &document, 7);
        fixture.write_desired(
            r#"{"generation":1,"os":{"channel":"stable","version":"1.4.0"},"packages":[]}"#,
        );
        fixture.daemon.reload();
        pair_to(&fixture, "https://hub.example");
        on_ethernet(&fixture);

        // Left over from something else, and larger than this bundle can be.
        std::fs::create_dir_all(layout.partial(&image_digest()).parent().unwrap()).unwrap();
        std::fs::write(
            layout.partial(&image_digest()),
            [IMAGE, b" and then some more"].concat(),
        )
        .unwrap();

        // A fetcher that records where it was asked to start.
        struct FromWhere(std::sync::Arc<std::sync::Mutex<Vec<u64>>>);
        impl crate::bundle::Fetcher for FromWhere {
            fn slice(&mut self, ask: &crate::bundle::Ask) -> crate::bundle::Slice {
                self.0.lock().unwrap().push(ask.from);
                std::fs::write(&ask.into, IMAGE).unwrap();
                crate::bundle::Slice {
                    status: 200,
                    before: 0,
                    after: IMAGE.len() as u64,
                    declared_total: Some(IMAGE.len() as u64),
                    complete: true,
                    error: None,
                }
            }
        }
        let asks = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        fixture.daemon.fetcher = Box::new(FromWhere(asks.clone()));
        let installs = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        fixture.daemon.updater = Box::new(SharedUpdater(installs.clone()));

        let pass = fixture.daemon.reconcile(1_000);
        let (_, outcome) = pass
            .applied
            .iter()
            .find(|(action, _)| {
                matches!(
                    action.kind,
                    skynetd::reconcile::ActionKind::StageOsImage { .. }
                )
            })
            .expect("the plan stages the image");
        assert_eq!(outcome, &Outcome::Applied, "{outcome:?}");
        assert_eq!(installs.lock().unwrap().len(), 1);
        assert_eq!(
            asks.lock().unwrap().as_slice(),
            [0],
            "a partial cannot be larger than what it is part of, so the transfer starts again \
             rather than resuming into bytes from another image"
        );
    }

    /// §34.3's wait, on the route an operator reads.
    #[test]
    fn the_ota_route_shows_where_the_bytes_are() {
        let mut fixture = with_real_release("ota-route-transfer");
        let layout = fixture.daemon.store.layout().clone();
        let path = layout.state().join("bearers.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "{\"bearers\":[{\"name\":\"wwan0\",\"kind\":\"cellular\",\"up\":true}]}",
        )
        .unwrap();
        // Half an image, from an earlier pass on a link that has since gone.
        std::fs::create_dir_all(layout.partial(&image_digest()).parent().unwrap()).unwrap();
        std::fs::write(layout.partial(&image_digest()), &IMAGE[..20]).unwrap();

        let (status, body) = fixture.request("GET", "/ota", Some("observer-token"), "");
        assert_eq!(status, 200);
        let transfer = body.get("transfer").expect("a transfer block");
        assert_eq!(
            transfer.get("decision").and_then(Json::as_str),
            Some("wait_for_unmetered"),
            "a cellular bearer is assumed metered and an OS image waits for a link nobody pays by \
             the megabyte for"
        );
        assert_eq!(transfer.get("received").and_then(Json::as_f64), Some(20.0));
        assert_eq!(transfer.get("have"), Some(&Json::Bool(false)));
        assert_eq!(
            transfer.get("digest").and_then(Json::as_str),
            Some(image_digest().as_str())
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

    /// `pub(super)` so `mod wire` can freeze a check-in from a robot this module already knows how
    /// to make. The alternative is a second pairing written beside the fixture, which is how a
    /// fixture stops being a document the production path produces.
    pub(super) fn become_paired(fixture: &Fixture) {
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

    pub(super) fn built_for(fixture: &mut Fixture) -> skynet_contracts::checkin::CheckIn {
        built_with_power(fixture, None)
    }

    /// The same, with a §36 reading supplied. Two functions rather than one with an `Option` at
    /// every call site: nearly every check-in test is about something else, and threading a `None`
    /// through forty of them to serve two would be forty places to read past.
    fn built_with_power(
        fixture: &mut Fixture,
        power: Option<&crate::power::Reading>,
    ) -> skynet_contracts::checkin::CheckIn {
        let layout = fixture.daemon.store.layout().clone();
        let trust = TrustStore::load(&layout);
        let (identity, _) = crate::identity::evaluate(&layout, &trust);
        let actual = fixture.daemon.observe(1_000);
        let observed = fixture
            .daemon
            .health(&actual, 1_000, power, fixture.daemon.read_headroom());
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
            // §7.6, over whatever body this fixture has been given — so a test that writes a
            // descriptor gets that descriptor's guard partition on the wire rather than a fixed one.
            Some(crate::limits::report(&crate::limits::read(
                &layout,
                &crate::embodiment::load(&layout),
                crate::limits::ROBOT_TIER,
                &crate::blackbox::ProximitySighting::Unobservable("this fixture has no ring"),
            ))),
        )
    }

    /// The two fields §33.6 has named and nothing has ever filled.
    ///
    /// Until §36 was written, `battery` and `temperature_c` were `Measured::none()` in the rollup
    /// itself — a literal, not a reading that failed — so a check-in from a robot with a full
    /// battery said the same thing as one from a robot with no battery at all. This is the first
    /// test in the suite that puts a number in either.
    #[test]
    fn a_gauge_reading_reaches_the_check_in_dated() {
        let mut fixture = Fixture::new("checkin-power");
        become_paired(&fixture);

        let reading = crate::power::Reading {
            charge_fraction: Some(0.62),
            observation: skynet_contracts::power::PowerObservation {
                state_of_charge: Some(0.62),
                temperature_c: Some(48.5),
                ..Default::default()
            },
            ..Default::default()
        };
        let built = built_with_power(&mut fixture, Some(&reading));

        let fields: Vec<(String, f64, u64)> = built.health.fields.clone();
        let battery = fields
            .iter()
            .find(|(name, _, _)| name == "battery")
            .expect("a battery field");
        assert_eq!(battery.1, 0.62);
        assert_eq!(battery.2, 0, "measured in this pass, so its age is nothing");

        let temperature = fields
            .iter()
            .find(|(name, _, _)| name == "temperature_c")
            .expect("a temperature field");
        assert_eq!(temperature.1, 48.5);

        assert!(
            !built.health.unmeasured.contains(&"battery".to_string()),
            "{:?}",
            built.health.unmeasured
        );
        assert_eq!(
            built.health.unmeasured,
            vec![
                "guard_trip_rate".to_string(),
                "storage_headroom".to_string()
            ],
            "the two that are still nobody's"
        );
        assert!(
            !built.health.degraded,
            "a measured battery is not a fault, whatever it reads"
        );
    }

    /// And the other direction, which is the state every robot in this suite is in: a daemon whose
    /// sensors point at nothing reports both fields absent, exactly as it did before §36.
    #[test]
    fn a_robot_with_no_gauge_still_reports_both_fields_unmeasured() {
        let mut fixture = Fixture::new("checkin-no-power");
        become_paired(&fixture);
        let built = built_for(&mut fixture);
        assert_eq!(
            built.health.unmeasured,
            vec![
                "battery".to_string(),
                "guard_trip_rate".to_string(),
                "storage_headroom".to_string(),
                "temperature_c".to_string()
            ]
        );
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

    /// The rule above, reached from every module that makes a request — not only from the one it is
    /// written in.
    ///
    /// Until 2026-09-16 `bundle` and `upload` each spawned their own curl and each carried their own
    /// copy of this check, which CI's *one caller* rule exists to prevent and which nothing had
    /// noticed. They go through `connectivity::curl` now, and this is what says the refusal
    /// travelled with them. A refused push also has to refuse **before** copying its slice out:
    /// the copy is a piece of a recording on the disk §28 reserves for recordings.
    #[test]
    fn every_request_refuses_the_same_credential_before_it_does_anything() {
        use crate::bundle::Fetcher;
        use crate::upload::Uploader;

        let scratch = TempRoot::new("every-caller-quoting");
        let state = scratch.layout().state();
        let window = state.join("window");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(&window, b"sealed bytes").unwrap();

        for bad in ["skd_a\"b", "skd_a\\b", "skd_a\nb"] {
            let partial = state.join("partial/image");
            let slice = crate::bundle::CurlFetcher.slice(&crate::bundle::Ask {
                url: "https://hub.example/image".to_string(),
                credential: Some(bad.to_string()),
                from: 0,
                into: partial.clone(),
                budget_s: 1,
            });
            let said = slice
                .error
                .expect("a fetch with that credential is refused");
            assert!(said.contains("will not put in a curl config"), "{said}");
            assert!(
                !partial.parent().unwrap().exists(),
                "refused before the partial's directory was made"
            );

            let probed = crate::upload::CurlUploader.probe(&crate::upload::Probe {
                url: "https://hub.example/blackbox".to_string(),
                credential: Some(bad.to_string()),
                budget_s: 1,
            });
            let said = probed
                .error
                .expect("a probe with that credential is refused");
            assert!(said.contains("will not put in a curl config"), "{said}");

            // In a directory of its own, because the slice file is removed after every push whatever
            // happened — its absence afterwards proves nothing. `extract` makes the directory, so
            // its absence says the copy was never started.
            let slice_file = state.join("slices/slice");
            let pushed = crate::upload::CurlUploader.push(&crate::upload::Push {
                url: "https://hub.example/blackbox".to_string(),
                credential: Some(bad.to_string()),
                from: 0,
                len: 12,
                total: 12,
                file: window.clone(),
                scratch: slice_file.clone(),
                budget_s: 1,
            });
            let said = pushed
                .error
                .expect("a push with that credential is refused");
            assert!(said.contains("will not put in a curl config"), "{said}");
            assert!(
                !slice_file.parent().unwrap().exists(),
                "refused before any of the recording was copied"
            );
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

// ── §12's bytes, on a machine ────────────────────────────────────────────────
//
// `skynet_contracts::bundle` proves the seven decisions and the four verdicts for given inputs.
// These prove the half a pure function cannot: that a partial on the disk is what the decision is
// made from, that a slice resumes where the last one stopped, that bytes which are not the release
// are deleted rather than kept, that a metered link asks for nothing at all — and that a bundle
// somebody put on the robot by hand is held to the same digest as one that was fetched.

mod bundle {
    use super::*;
    use crate::bundle::{advance, hash_file, url_for, Ask, Attempts, Fetcher, Slice, Step};

    /// What a scripted server does about one request.
    enum Answer {
        /// Append these bytes and say whether that was the whole body.
        Send {
            chunk: &'static [u8],
            complete: bool,
            total: Option<u64>,
        },
        /// Answer `200` to a resume, which is a server that ignored the range.
        WholeAgain(&'static [u8]),
        /// Fail without moving anything.
        Fail(&'static str),
    }

    /// A server the suite writes the script for.
    ///
    /// A trait double rather than a local HTTP server, for [`crate::ota::RecordingUpdater`]'s
    /// reason: the four interesting cases here are a link that drops halfway, a resume, a mirror
    /// serving the wrong image and a proxy ignoring a range header, and none of them is reachable
    /// through a network a test controls.
    struct Scripted {
        answers: std::collections::VecDeque<Answer>,
        asks: Vec<(u64, String)>,
    }

    impl Scripted {
        fn new(answers: Vec<Answer>) -> Scripted {
            Scripted {
                answers: answers.into_iter().collect(),
                asks: Vec::new(),
            }
        }

        fn silent() -> Scripted {
            Scripted::new(Vec::new())
        }
    }

    impl Fetcher for Scripted {
        fn slice(&mut self, ask: &Ask) -> Slice {
            use std::io::Write;
            self.asks.push((ask.from, ask.url.clone()));
            let before = crate::bundle::size_of(&ask.into);
            let answer = self
                .answers
                .pop_front()
                .unwrap_or(Answer::Fail("the script ran out of answers"));

            let write = |bytes: &[u8], append: bool| {
                std::fs::create_dir_all(ask.into.parent().unwrap()).unwrap();
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(append)
                    .write(true)
                    .truncate(!append)
                    .open(&ask.into)
                    .unwrap();
                file.write_all(bytes).unwrap();
            };

            match answer {
                Answer::Send {
                    chunk,
                    complete,
                    total,
                } => {
                    write(chunk, ask.from > 0);
                    Slice {
                        status: if ask.from > 0 { 206 } else { 200 },
                        before,
                        after: crate::bundle::size_of(&ask.into),
                        declared_total: total,
                        complete,
                        error: None,
                    }
                }
                Answer::WholeAgain(bytes) => {
                    write(bytes, true);
                    Slice {
                        status: 200,
                        before,
                        after: crate::bundle::size_of(&ask.into),
                        declared_total: Some(bytes.len() as u64),
                        complete: true,
                        error: None,
                    }
                }
                Answer::Fail(detail) => Slice {
                    status: 0,
                    before,
                    after: before,
                    declared_total: None,
                    complete: false,
                    error: Some(detail.to_string()),
                },
            }
        }
    }

    const IMAGE: &[u8] = b"the whole operating system, in twenty-nine bytes";
    const HEAD: &[u8] = b"the whole operating";
    const TAIL: &[u8] = b" system, in twenty-nine bytes";

    fn digest_of(bytes: &[u8]) -> String {
        format!(
            "sha256:{}",
            String::from_utf8_lossy(&skynet_sha256::hex_bytes(bytes))
        )
    }

    /// A bearer table with one link, up.
    fn bearers(fixture: &Fixture, kind: &str, metered: bool) -> crate::connectivity::Table {
        let layout = fixture.daemon.store.layout();
        let path = layout.state().join("bearers.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "{{\"bearers\":[{{\"name\":\"link0\",\"kind\":\"{kind}\",\"up\":true,\
                 \"metered\":{metered}}}]}}"
            ),
        )
        .unwrap();
        crate::connectivity::Table::of(layout)
    }

    /// No bearers declared at all, which is a robot nobody has told about its own links.
    fn nothing_up(fixture: &Fixture) -> crate::connectivity::Table {
        let layout = fixture.daemon.store.layout();
        let path = layout.state().join("bearers.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{\"bearers\":[]}").unwrap();
        crate::connectivity::Table::of(layout)
    }

    #[test]
    fn a_whole_image_arrives_and_becomes_the_bundle() {
        let fixture = Fixture::new("bundle-whole");
        let layout = fixture.daemon.store.layout().clone();
        let table = bearers(&fixture, "ethernet", false);
        let digest = digest_of(IMAGE);
        let mut server = Scripted::new(vec![Answer::Send {
            chunk: IMAGE,
            complete: true,
            total: Some(IMAGE.len() as u64),
        }]);

        let step = advance(
            &layout,
            &mut server,
            &digest,
            None,
            &table,
            "https://hub.example/os/releases/1.4.0/bundle",
            Some("skd_x"),
            1_000,
            1.0,
        );

        match step {
            Step::Have { path, .. } => {
                assert_eq!(path, layout.bundle(&digest));
                assert_eq!(std::fs::read(&path).unwrap(), IMAGE);
            }
            other => panic!("{other:?}"),
        }
        assert!(
            !layout.partial(&digest).exists(),
            "the partial becomes the bundle rather than being copied out of"
        );
        assert!(
            !layout.transfer().exists(),
            "a finished transfer leaves no attempt record behind"
        );
    }

    /// The property the whole design rests on: a pass moves what it can and the next one continues.
    #[test]
    fn a_slice_that_runs_out_of_budget_resumes_where_it_stopped() {
        let fixture = Fixture::new("bundle-resume");
        let layout = fixture.daemon.store.layout().clone();
        let table = bearers(&fixture, "ethernet", false);
        let digest = digest_of(IMAGE);
        let mut server = Scripted::new(vec![
            Answer::Send {
                chunk: HEAD,
                complete: false,
                total: Some(IMAGE.len() as u64),
            },
            Answer::Send {
                chunk: TAIL,
                complete: true,
                total: Some(IMAGE.len() as u64),
            },
        ]);

        let fetch = |server: &mut Scripted, now: u64| {
            advance(
                &layout,
                server,
                &digest,
                None,
                &table,
                "https://hub.example/os/releases/1.4.0/bundle",
                Some("skd_x"),
                now,
                1.0,
            )
        };

        match fetch(&mut server, 1_000) {
            Step::Moved {
                received, total, ..
            } => {
                assert_eq!(received, HEAD.len() as u64);
                assert_eq!(total, Some(IMAGE.len() as u64));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            crate::bundle::size_of(&layout.partial(&digest)),
            HEAD.len() as u64
        );

        match fetch(&mut server, 11_000) {
            Step::Have { path, .. } => assert_eq!(std::fs::read(&path).unwrap(), IMAGE),
            other => panic!("{other:?}"),
        }
        assert_eq!(
            server.asks,
            vec![
                (
                    0,
                    "https://hub.example/os/releases/1.4.0/bundle".to_string()
                ),
                (
                    HEAD.len() as u64,
                    "https://hub.example/os/releases/1.4.0/bundle".to_string()
                ),
            ],
            "the second request resumes at the partial's length"
        );
    }

    /// §34.3, and the assertion that matters is that nothing was *asked for*.
    #[test]
    fn a_metered_link_is_not_asked_for_an_operating_system() {
        let fixture = Fixture::new("bundle-metered");
        let layout = fixture.daemon.store.layout().clone();
        let table = bearers(&fixture, "cellular", true);
        let digest = digest_of(IMAGE);
        let mut server = Scripted::silent();

        let step = advance(
            &layout,
            &mut server,
            &digest,
            None,
            &table,
            "https://hub.example/os/releases/1.4.0/bundle",
            Some("skd_x"),
            1_000,
            1.0,
        );
        match step {
            Step::Waiting { detail } => assert!(detail.contains("metered"), "{detail}"),
            other => panic!("{other:?}"),
        }
        assert!(
            server.asks.is_empty(),
            "a metered bearer is a link nothing is requested over, not one a request fails on"
        );
    }

    #[test]
    fn no_link_at_all_is_the_ordinary_state_rather_than_a_fault() {
        let fixture = Fixture::new("bundle-offline");
        let layout = fixture.daemon.store.layout().clone();
        let table = nothing_up(&fixture);
        let mut server = Scripted::silent();

        let step = advance(
            &layout,
            &mut server,
            &digest_of(IMAGE),
            None,
            &table,
            "https://hub.example/os/releases/1.4.0/bundle",
            Some("skd_x"),
            1_000,
            1.0,
        );
        match step {
            Step::Waiting { detail } => assert!(detail.contains("ordinary state"), "{detail}"),
            other => panic!("{other:?}"),
        }
        assert!(server.asks.is_empty());
    }

    /// The rule the module header calls the one that keeps a resume honest.
    #[test]
    fn bytes_that_are_not_the_release_are_deleted_rather_than_resumed_from() {
        let fixture = Fixture::new("bundle-wrong");
        let layout = fixture.daemon.store.layout().clone();
        let table = bearers(&fixture, "ethernet", false);
        let digest = digest_of(IMAGE);
        let mut server = Scripted::new(vec![Answer::Send {
            chunk: b"a perfectly good image of something else entirely",
            complete: true,
            total: None,
        }]);

        let step = advance(
            &layout,
            &mut server,
            &digest,
            None,
            &table,
            "https://hub.example/os/releases/1.4.0/bundle",
            Some("skd_x"),
            1_000,
            1.0,
        );
        match step {
            Step::Failed { detail } => {
                assert!(detail.contains("where they came from"), "{detail}");
            }
            other => panic!("{other:?}"),
        }
        assert!(
            !layout.partial(&digest).exists(),
            "bytes that will never verify are bytes a resume would append to for ever"
        );
        assert!(!layout.bundle(&digest).exists());

        let attempts = Attempts::read(&layout, &digest);
        assert_eq!(attempts.failures, 1);
        assert!(attempts.next_ms > 1_000, "and the next attempt is held off");
    }

    /// A proxy that ignores `Range` produces a file that is one image's head plus a whole other
    /// copy. Caught at the response rather than after the transfer.
    #[test]
    fn a_server_that_ignores_the_range_has_its_partial_thrown_away() {
        let fixture = Fixture::new("bundle-range");
        let layout = fixture.daemon.store.layout().clone();
        let table = bearers(&fixture, "ethernet", false);
        let digest = digest_of(IMAGE);
        let mut server = Scripted::new(vec![
            Answer::Send {
                chunk: HEAD,
                complete: false,
                total: Some(IMAGE.len() as u64),
            },
            Answer::WholeAgain(IMAGE),
        ]);

        let fetch = |server: &mut Scripted, now: u64| {
            advance(
                &layout,
                server,
                &digest,
                None,
                &table,
                "https://hub.example/os/releases/1.4.0/bundle",
                Some("skd_x"),
                now,
                1.0,
            )
        };
        assert!(matches!(fetch(&mut server, 1_000), Step::Moved { .. }));
        match fetch(&mut server, 2_000) {
            Step::Failed { detail } => assert!(detail.contains("ignored the range"), "{detail}"),
            other => panic!("{other:?}"),
        }
        assert!(!layout.partial(&digest).exists());
    }

    /// §34.4's curve, applied to a transfer: a failure holds the next attempt off, and the hold is
    /// reported rather than being a silent skipped pass.
    #[test]
    fn a_failed_attempt_backs_off_before_the_next_one() {
        let fixture = Fixture::new("bundle-backoff");
        let layout = fixture.daemon.store.layout().clone();
        let table = bearers(&fixture, "ethernet", false);
        let digest = digest_of(IMAGE);
        let mut server = Scripted::new(vec![
            Answer::Fail("the Hub answered HTTP 503"),
            Answer::Send {
                chunk: IMAGE,
                complete: true,
                total: Some(IMAGE.len() as u64),
            },
        ]);
        let fetch = |server: &mut Scripted, now: u64| {
            advance(
                &layout,
                server,
                &digest,
                None,
                &table,
                "https://hub.example/os/releases/1.4.0/bundle",
                Some("skd_x"),
                now,
                1.0,
            )
        };

        match fetch(&mut server, 1_000) {
            Step::Failed { detail } => assert!(detail.contains("503"), "{detail}"),
            other => panic!("{other:?}"),
        }
        // The very next pass, ten seconds later, must not ask again.
        match fetch(&mut server, 11_000) {
            Step::Waiting { detail } => assert!(detail.contains("held for another"), "{detail}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(server.asks.len(), 1, "the second pass asked nothing");

        // Past the delay, it asks — and the answer that works clears the record.
        match fetch(&mut server, 100_000) {
            Step::Have { .. } => {}
            other => panic!("{other:?}"),
        }
        assert!(!layout.transfer().exists());
    }

    /// The jitter is supplied, so the curve is a thing a test can assert on (§35.2).
    #[test]
    fn the_backoff_curve_doubles_and_is_capped() {
        use crate::bundle::{backoff_ms, FIRST_BACKOFF_MS, MAX_BACKOFF_MS};
        assert_eq!(backoff_ms(0, 1.0), 0);
        assert_eq!(backoff_ms(1, 1.0), FIRST_BACKOFF_MS);
        assert_eq!(backoff_ms(2, 1.0), FIRST_BACKOFF_MS * 2);
        assert_eq!(backoff_ms(40, 1.0), MAX_BACKOFF_MS);
        // Full jitter: uniform over `[0, exponential]`, not a perturbation of it.
        assert_eq!(backoff_ms(1, 0.0), 0);
        assert_eq!(backoff_ms(1, 0.5), FIRST_BACKOFF_MS / 2);
    }

    /// An attempt record belongs to the bytes it is about.
    #[test]
    fn the_backoff_of_one_image_does_not_hold_up_another() {
        let fixture = Fixture::new("bundle-other-record");
        let layout = fixture.daemon.store.layout().clone();
        Attempts {
            digest: "sha256:something-else".to_string(),
            failures: 9,
            next_ms: 10_000_000,
            error: Some("an older transfer".to_string()),
            declared_total: Some(12),
        }
        .write(&layout)
        .unwrap();

        let held = Attempts::read(&layout, &digest_of(IMAGE));
        assert_eq!(held.failures, 0);
        assert_eq!(held.next_ms, 0);
        assert_eq!(held.error, None);
    }

    /// A transfer that ends badly having moved bytes has not failed in the sense the curve is about.
    #[test]
    fn a_link_that_drops_mid_slice_does_not_count_as_a_failure() {
        let fixture = Fixture::new("bundle-dropped");
        let layout = fixture.daemon.store.layout().clone();
        let table = bearers(&fixture, "ethernet", false);
        let digest = digest_of(IMAGE);
        // Bytes arrived *and* curl reported an error, which is what a link going down looks like.
        struct Flaky;
        impl Fetcher for Flaky {
            fn slice(&mut self, ask: &Ask) -> Slice {
                use std::io::Write;
                std::fs::create_dir_all(ask.into.parent().unwrap()).unwrap();
                let mut file = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&ask.into)
                    .unwrap();
                file.write_all(HEAD).unwrap();
                Slice {
                    status: 200,
                    before: 0,
                    after: HEAD.len() as u64,
                    declared_total: None,
                    complete: false,
                    error: Some("curl could not fetch: transfer closed".to_string()),
                }
            }
        }

        let step = advance(
            &layout,
            &mut Flaky,
            &digest,
            None,
            &table,
            "https://hub.example/os/releases/1.4.0/bundle",
            Some("skd_x"),
            1_000,
            1.0,
        );
        match step {
            Step::Moved { received, .. } => assert_eq!(received, HEAD.len() as u64),
            other => panic!("{other:?}"),
        }
        let attempts = Attempts::read(&layout, &digest);
        assert_eq!(attempts.failures, 0, "progress is not a failure");
        assert_eq!(attempts.next_ms, 0, "so the next pass resumes immediately");
        assert!(attempts.error.is_some(), "and the reason is still recorded");
    }

    #[test]
    fn an_image_already_here_is_not_fetched_again() {
        let fixture = Fixture::new("bundle-have");
        let layout = fixture.daemon.store.layout().clone();
        let table = bearers(&fixture, "ethernet", false);
        let digest = digest_of(IMAGE);
        std::fs::create_dir_all(layout.bundle(&digest).parent().unwrap()).unwrap();
        std::fs::write(layout.bundle(&digest), IMAGE).unwrap();

        let mut server = Scripted::silent();
        match advance(
            &layout,
            &mut server,
            &digest,
            None,
            &table,
            "https://hub.example/os/releases/1.4.0/bundle",
            Some("skd_x"),
            1_000,
            1.0,
        ) {
            Step::Have { .. } => {}
            other => panic!("{other:?}"),
        }
        assert!(server.asks.is_empty());
    }

    #[test]
    fn a_digest_is_taken_over_the_file_without_reading_it_into_memory() {
        let fixture = Fixture::new("bundle-hash");
        let layout = fixture.daemon.store.layout().clone();
        let path = layout.state().join("big");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // Larger than the read buffer, so the streaming path is actually exercised.
        let bytes = vec![7u8; 200_000];
        std::fs::write(&path, &bytes).unwrap();

        let (digest, size) = hash_file(&path).unwrap();
        assert_eq!(size, 200_000);
        assert_eq!(digest, digest_of(&bytes));
    }

    #[test]
    fn the_url_is_beside_the_catalogue_and_encodes_the_version() {
        assert_eq!(
            url_for("https://hub.example/api/skynet/v1/", "1.4.0+rc 1"),
            format!(
                "https://hub.example/api/skynet/v1/os/releases/1.4.0%2Brc%201/bundle?arch={}",
                crate::releases::arch()
            )
        );
    }
}

// ── The wire fixtures ────────────────────────────────────────────────────────
//
// `reference/wire/` holds the exact canonical body this daemon puts on the wire for three of §17's
// routes, produced here by the real producers. The Hub keeps its own copy and pushes each one
// through the receiving route's validation.
//
// This exists because of what the twenty-seventh session found: both repositories had built the
// episode path and had never put one document through the other, and they did not agree about a
// single field name. Neither suite could see it — one asserted what the robot sends and the other
// asserted what the Hub accepts, and a boundary needs a test that holds one side's *real output*
// against the other side's *real contract*.
//
// A producer that changes fails here, with the command that re-freezes the file. A route that stops
// reading a field fails in the Hub's suite. The two copies are pinned by digest, so neither
// repository can move alone.

/// Where the frozen documents live.
fn wire_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("reference")
        .join("wire")
}

/// Compare one document against its frozen copy, or re-freeze it when asked.
///
/// The environment variable is the whole of the write path, deliberately: a fixture that rewrites
/// itself whenever it disagrees is not a fixture, it is a log of the last run.
fn frozen(name: &str, document: &Json) {
    let canonical = document.to_canonical().expect("the document canonicalises");
    let path = wire_dir().join(format!("{name}.json"));

    if std::env::var("SKYNET_WRITE_WIRE").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "{canonical}
"
            ),
        )
        .unwrap();
        return;
    }

    let held = std::fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "{} could not be read ({error}). Freeze it with:
                 SKYNET_WRITE_WIRE=1 cargo test -p skynetd-io",
            path.display()
        )
    });

    assert_eq!(
        held.trim(),
        canonical,
        "
{} no longer matches what this daemon produces.

         This document crosses §17 and the Hub keeps its own copy of it. If the change is intended,          re-freeze and resync — and expect the Hub's suite to have something to say, which is the          point:
             SKYNET_WRITE_WIRE=1 cargo test -p skynetd-io
             node tools/check-contract-mirrors.mjs --sync
",
        path.display()
    );
}

mod wire {
    use super::*;

    /// §17's check-in, from a robot with a body and a profile.
    ///
    /// The fourth fixture, and the one that closes a gap the other three made visible: the check-in
    /// is the most-sent document on §17 and it had never been frozen. Both repositories decode it,
    /// both suites passed, and nothing held one side's real bytes against the other side's real
    /// route — which is exactly the arrangement the twenty-seventh session found the episode path in.
    ///
    /// The robot is **described and configured** on purpose. A fixture taken on a bare development
    /// root freezes a check-in whose `limits` block is all nulls, and the shape a fleet page
    /// actually has to render — ceilings with provenance, three active guards, five unavailable
    /// ones each naming what it wants — would go untested on both sides.
    #[test]
    fn the_check_in_a_described_robot_sends() {
        let mut fixture = Fixture::new("wire-checkin");
        // Through the check-in module's own helpers, so this document is built by exactly the path
        // forty other tests assert about rather than by a second assembly written beside a fixture.
        super::checkin::become_paired(&fixture);
        write_embodiment(&fixture, LIMITED_ARM);
        write_profile(&fixture, SITE_PROFILE);
        fixture.daemon.reload();

        let built = super::checkin::built_for(&mut fixture);
        let document = skynet_contracts::checkin::check_in_to_json(&built);

        // Asserted before it is frozen, because a fixture is only worth freezing if somebody has
        // said what it is supposed to contain. §7.6's block is the reason this fixture exists.
        let limits = document.get("limits").expect("§7.6's block");
        assert_eq!(limits.get("resolved_at"), Some(&Json::from("guest")));
        assert_eq!(
            limits.get("enforced"),
            Some(&Json::Bool(false)),
            "a fleet page must never render a guard list as enforcement this edition does not do"
        );
        assert_eq!(
            limits.get("source"),
            Some(&Json::from("vendor datasheet rev C")),
            "a safety limit without provenance is a rumour, and the provenance has to cross"
        );
        let active: Vec<&str> = limits
            .arr_or_empty("active")
            .iter()
            .filter_map(Json::as_str)
            .collect();
        assert!(active.contains(&"workspace"), "{active:?}");
        // §7.2's sentence, verbatim across the boundary: a wheeled or fixed base is a shape and not
        // a defect, and a Hub that re-derived this row would re-derive it as a missing IMU.
        let balance = limits
            .arr_or_empty("unavailable")
            .iter()
            .find(|entry| entry.get("guard") == Some(&Json::from("balance")))
            .cloned()
            .expect("balance is unavailable on a fixed base");
        assert_eq!(
            balance.arr_or_empty("missing")[0],
            Json::from("a legged base — not applicable to this body")
        );

        frozen("checkin", &document);
    }

    /// §74's window, closed the way a reconcile pass closes one **on the edition that ships**.
    ///
    /// Through a real recorder, opened at the shape the daemon opens one at: zero joints and zero
    /// channels, because there is no control loop. So the five counters §13's ring supplies are
    /// absent, and that is not this test being lazy — it is the document every robot alive sends,
    /// and the one the Hub has to render today.
    ///
    /// [`the_telemetry_window_a_robot_with_a_control_loop_closes`] is the other half, and the two
    /// exist together on purpose: a fixture taken on an empty robot freezes the empty case, and a
    /// fixture taken only on a full one freezes a case no robot can currently produce.
    #[test]
    fn the_telemetry_window_a_reconcile_pass_closes() {
        let root = TempRoot::new("wire-telemetry");
        let layout = root.layout();
        let recorder = crate::blackbox::Recorder::open(
            &layout,
            crate::blackbox::supervisor_meta(&layout),
            skynet_blackbox::record::Shape {
                joints: 0,
                channels: 0,
            },
            64 * 1024,
        )
        .expect("a recorder");

        let mut counting = crate::telemetry::Counting::opened(
            1_788_307_200_000.0,
            60_000.0,
            7,
            recorder.next_sequence(),
        );
        counting.passes = 30;
        // Five minutes of wall clock, five of monotonic, and two drops inside the window.
        let window = counting.close(
            1_788_307_500_000.0,
            360_000.0,
            9,
            recorder.tally(counting.opened_sequence),
        );
        frozen("telemetry", &crate::telemetry::window_to_json(&window));
    }

    /// §74's window from a robot that **commands a body**, counted off §13's ring.
    ///
    /// Every step is the real producer: a real ring at a real shape, cycles encoded by the ring's
    /// own encoder, counted by the real [`skynet_blackbox::tally::tally`], closed by the real
    /// `Counting::close`. What is synthetic is the cycles themselves — no policy runtime exists to
    /// have produced them — which is the same and only synthetic ingredient
    /// `telemetry.replay.deterministic` has, for the same reason.
    ///
    /// It is frozen now rather than when a control loop appears, because the Hub has to render this
    /// today: a fleet page whose guard-trip column is only ever exercised against nulls is a column
    /// nobody has seen work.
    #[test]
    fn the_telemetry_window_a_robot_with_a_control_loop_closes() {
        use skynet_blackbox::record::{
            CycleView, Entry, EventCode, EventView, Shape, NO_CAPABILITY,
        };
        use skynet_blackbox::ring::{Geometry, Meta, Ring};

        let shape = Shape {
            joints: 2,
            channels: 2,
        };
        let (mut ring, mut store) = Ring::create(
            Geometry {
                shape,
                record_size: shape.record_size(),
                capacity: 64,
                header_len: 4096,
                digests: 4,
                capabilities: 4,
            },
            Meta {
                robot_id: "dev-0001".to_string(),
                embodiment: [0x11; 32],
                os_image: [0x22; 32],
                tier: 1,
                enforcement: 1,
                simulator: false,
                wall_epoch_ns: 1_788_307_200_000_000_000,
                mono_ns: 60_000_000_000,
                channels: vec![
                    "arm.shoulder.pitch".to_string(),
                    "arm.elbow.pitch".to_string(),
                ],
            },
        );

        let opened = ring.next_sequence();

        // Ten cycles. Two of them trip — one on a single guard, one on two at once — so the frozen
        // document carries the case the two counters exist to tell apart: three trips across two
        // interventions.
        for tick in 1..=10u64 {
            let tripped: Vec<skynet_blackbox::Guard> = match tick {
                4 => vec![skynet_blackbox::Guard::Envelope],
                7 => vec![
                    skynet_blackbox::Guard::Envelope,
                    skynet_blackbox::Guard::RateLimit,
                ],
                _ => Vec::new(),
            };
            let clamped = vec![!tripped.is_empty(), false];
            let view = CycleView {
                tick,
                observation: [0xAB; 32],
                policy: 0,
                state_flags: 0,
                fault_code: 0,
                read_result: 0,
                rhal_result: 0,
                state_seq: tick as u32,
                state_stamp_ns: tick * 1_000_000,
                command_stamp_ns: tick * 1_000_000,
                command_seq: tick as u32,
                mode: 0,
                advisory: false,
                response: None,
                tripped: &tripped,
                capability: NO_CAPABILITY,
                nearest_person_m: f32::NAN,
                base_pose: [f32::NAN; 7],
                base_twist: [0.0; 6],
                desired: &[0.10, 0.20],
                command: &[0.10, 0.20],
                mask: &[1, 1],
                clamped: &clamped,
                position: &[0.09, 0.19],
                velocity: &[0.0, 0.0],
                effort: &[1.0, 2.0],
            };
            ring.append(&mut store, tick * 1_000_000, Entry::Cycle(&view))
                .expect("a cycle");
        }
        ring.append(
            &mut store,
            11_000_000,
            Entry::Event(&EventView {
                code: EventCode::Estop,
                subject: "robot",
                detail: "button",
            }),
        )
        .expect("an estop");

        let mut counting =
            crate::telemetry::Counting::opened(1_788_307_200_000.0, 60_000.0, 7, opened);
        counting.passes = 30;
        let window = counting.close(
            1_788_307_500_000.0,
            360_000.0,
            9,
            Some(skynet_blackbox::tally::tally(&ring, &store, opened)),
        );

        // The assertions belong here as well as in the frozen bytes: a fixture that changed for the
        // wrong reason would still be a fixture, and these say what the right numbers are.
        assert_eq!(window.counter("cycles"), Some(10.0));
        assert_eq!(window.counter("interventions"), Some(2.0));
        assert_eq!(window.counter("guard_trips"), Some(3.0));
        assert_eq!(window.counter("clamps"), Some(2.0));
        assert_eq!(window.counter("estops"), Some(1.0));

        frozen(
            "telemetry-counted",
            &crate::telemetry::window_to_json(&window),
        );
    }

    /// §38.1's alarm, through the reconcile path that raises it.
    ///
    /// The robot has a **black box ring**, so the frozen document carries the case that matters:
    /// §13 seals on a fault, and the incident names the window it sealed. A fixture taken on a
    /// robot with no recorder would freeze the honest-but-uninteresting sentence and leave the
    /// shape a Hub actually has to render untested.
    #[test]
    fn the_incident_a_failed_profile_signature_raises() {
        let mut fixture = Fixture::new("wire-incident");
        // Opened and dropped: this creates the ring §28 preallocates, which is what the daemon's
        // sealer reads. Nothing here holds a writer while the incident is raised.
        {
            let layout = fixture.daemon.store.layout().clone();
            let _recorder = crate::blackbox::Recorder::open(
                &layout,
                crate::blackbox::supervisor_meta(&layout),
                skynet_blackbox::record::Shape {
                    joints: 0,
                    channels: 0,
                },
                64 * 1024,
            )
            .expect("a recorder");
        }
        let signer = driver_key(14);
        write_trust(&fixture, "fleet-admin", &public_hex(&signer));
        let over_other_bytes = signer.sign(&crate::trust::profile_message(
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        ));
        desired_profile(
            &fixture,
            Some((&public_hex(&signer), &hex_of(&over_other_bytes))),
        );
        fixture.daemon.reload();

        let queue = crate::outbound::Queue::of(fixture.daemon.store.layout());
        let queued = queue.items();
        assert_eq!(queued.len(), 1, "{queued:?}");
        let document = Json::parse(&queued[0].body).expect("the incident is JSON");
        // §13's window, sealed by the raise rather than by anything the test did.
        let recording = document.get("recording").expect("a recording block");
        assert_eq!(recording.get("sealed"), Some(&Json::Bool(true)));
        assert_eq!(
            recording.get("uploaded"),
            Some(&Json::Bool(false)),
            "the bytes do not cross §17 yet, and the document says so rather than implying they did"
        );
        frozen("incident", &document);
    }
}

// ── §54.2's safety notes ─────────────────────────────────────────────────────
//
// > `safety_notes` stops being decorative too: it is displayed at grant time and acknowledged once
// > per skill per robot, recorded in the audit log. A safety note nobody has read is a safety note
// > that does not exist.
//
// The field has been in the Hub's skill manifest since skills were built and read by nothing. These
// prove the half that gives it teeth: the grant is refused until the notes have been read, the
// refusal *carries* them, the acknowledgement is bound to the text rather than to the name, and the
// whole thing leaves a record somebody can find after an incident.

mod safety_notes {
    use super::*;

    const NOTES: &str = "This skill moves the arm through the operator's working envelope.\nClear the cell before granting.";

    /// A skill this robot holds and is meant to run, with notes.
    ///
    /// Staged and named by the desired state rather than activated: activation is a symlink swap
    /// that §37.1 requires to be atomic, which is POSIX-only — and the rule under test is about the
    /// manifest this robot holds, which is the same document either way. `mod posix` covers the
    /// activation path itself.
    fn install(fixture: &mut Fixture, version: &str, notes: Option<&str>) -> String {
        let source = fixture.root.path().join(format!("skill-source-{version}"));
        let mut m = manifest("acme/wave", version, "skill");
        if let Some(notes) = notes {
            m.set("safety_notes", Json::from(notes));
        }
        let digest = write_package(
            &source,
            &m,
            &[(
                "bin/run",
                "#!/bin/sh
exit 0
",
            )],
        );
        let id = PackageId::parse("acme/wave").unwrap();
        fixture
            .daemon
            .store
            .stage(&id, "skill", version, &digest, &source)
            .unwrap();
        fixture.write_desired(&format!(
            "{{\"generation\":1,\"packages\":[{{\"id\":\"acme/wave\",\"version\":\"{version}\",\"digest\":\"{digest}\",\"kind\":\"skill\"}}]}}"
        ));
        fixture.daemon.reload();
        digest
    }

    fn with_notes(label: &str, notes: Option<&str>) -> Fixture {
        let mut fixture = Fixture::new(label);
        fixture.provision();
        install(&mut fixture, "1.0.0", notes);
        fixture
    }

    fn grant(fixture: &mut Fixture, body: &str) -> (u16, Json) {
        fixture.request("POST", "/grants", Some("admin-token"), body)
    }

    #[test]
    fn a_grant_is_refused_until_the_notes_have_been_read_and_the_refusal_carries_them() {
        let mut fixture = with_notes("notes-refused", Some(NOTES));
        let (status, body) = grant(&mut fixture, r#"{"id":"acme/wave","token":"arm.left"}"#);

        assert_eq!(status, 409, "{body:?}");
        assert_eq!(
            body.get("outcome").and_then(Json::as_str),
            Some("safety_notes_unread")
        );
        // Displayed, which is the word §54.2 uses. A refusal that only named the rule would send an
        // operator to a Hub they may not be able to reach.
        assert_eq!(body.get("safety_notes").and_then(Json::as_str), Some(NOTES));
        assert!(body
            .get("detail")
            .and_then(Json::as_str)
            .is_some_and(|detail| detail.contains("does not exist")));

        // And nothing was granted.
        let held = crate::observe::read_grants(fixture.daemon.store.layout());
        assert!(held.is_empty(), "{held:?}");
    }

    #[test]
    fn acknowledging_them_grants_and_leaves_a_record() {
        let mut fixture = with_notes("notes-acknowledged", Some(NOTES));
        let (_, refusal) = grant(&mut fixture, r#"{"id":"acme/wave","token":"arm.left"}"#);
        let digest = refusal
            .get("digest")
            .and_then(Json::as_str)
            .expect("a digest to acknowledge")
            .to_string();

        let (status, body) = grant(
            &mut fixture,
            &format!(r#"{{"id":"acme/wave","token":"arm.left","acknowledge":"{digest}"}}"#),
        );
        assert_eq!(status, 201, "{body:?}");

        // §38.3's log, because the file answers "may this proceed" and only the log answers "who
        // read it, and when" — which is the question asked after an incident.
        let entries = crate::audit::Audit::of(fixture.daemon.store.layout()).read();
        let entry = entries
            .entries
            .iter()
            .find(|entry| entry.action == "safety_notes_acknowledged")
            .expect("an audit entry");
        assert_eq!(entry.subject, "acme/wave");
        assert!(entry.detail.contains(&digest));
        assert!(
            entry.detail.contains("moves the arm"),
            "the log says what was agreed to, not only that something was: {}",
            entry.detail
        );
        assert_eq!(entry.actor, "suite");
    }

    #[test]
    fn a_second_grant_does_not_ask_again() {
        // "Once per skill per robot." An operator who has read a note and is adding a second
        // capability is not asked to read it twice.
        let mut fixture = with_notes("notes-once", Some(NOTES));
        let (_, refusal) = grant(&mut fixture, r#"{"id":"acme/wave","token":"arm.left"}"#);
        let digest = refusal
            .get("digest")
            .and_then(Json::as_str)
            .unwrap()
            .to_string();
        assert_eq!(
            grant(
                &mut fixture,
                &format!(r#"{{"id":"acme/wave","token":"arm.left","acknowledge":"{digest}"}}"#)
            )
            .0,
            201
        );

        let (status, _) = grant(&mut fixture, r#"{"id":"acme/wave","token":"camera.front"}"#);
        assert_eq!(status, 201);
    }

    #[test]
    fn acknowledging_other_notes_is_not_acknowledging_these() {
        // The digest is the point of binding to the text. A replayed command carrying last
        // version's digest is an operator who has not read this version's warning.
        let mut fixture = with_notes("notes-wrong-digest", Some(NOTES));
        let (status, body) = grant(
            &mut fixture,
            r#"{"id":"acme/wave","token":"arm.left","acknowledge":"sha256:not-these-notes"}"#,
        );
        assert_eq!(status, 409, "{body:?}");
        assert!(crate::observe::read_grants(fixture.daemon.store.layout()).is_empty());
    }

    #[test]
    fn changed_notes_are_a_note_nobody_has_read() {
        // A publisher who edits a warning has written a new warning. The alternative lets a package
        // acquire its notes *after* the operator agreed to a blank one, which is the version of
        // this rule that would be worth nothing.
        let mut fixture = with_notes("notes-changed", Some(NOTES));
        let (_, refusal) = grant(&mut fixture, r#"{"id":"acme/wave","token":"arm.left"}"#);
        let digest = refusal
            .get("digest")
            .and_then(Json::as_str)
            .unwrap()
            .to_string();
        assert_eq!(
            grant(
                &mut fixture,
                &format!(r#"{{"id":"acme/wave","token":"arm.left","acknowledge":"{digest}"}}"#)
            )
            .0,
            201
        );

        // The same package, a new version, a different warning.
        install(
            &mut fixture,
            "1.1.0",
            Some("This version also releases the gripper on stop."),
        );

        let (status, body) = grant(&mut fixture, r#"{"id":"acme/wave","token":"gripper"}"#);
        assert_eq!(status, 409, "{body:?}");
        assert!(body
            .get("safety_notes")
            .and_then(Json::as_str)
            .is_some_and(|text| text.contains("releases the gripper")));
    }

    #[test]
    fn a_package_with_no_notes_is_granted_without_ceremony() {
        // Requiring an acknowledgement of nothing is how an operator learns to acknowledge without
        // reading, which is the failure §54.2's sentence is about.
        let mut fixture = with_notes("notes-none", None);
        assert_eq!(
            grant(&mut fixture, r#"{"id":"acme/wave","token":"arm.left"}"#).0,
            201
        );
    }

    #[test]
    fn a_package_this_robot_does_not_hold_is_granted_and_asked_later() {
        // Grants and installs are both level-triggered and either order is legal, so a grant
        // written before the package arrives must not fail for want of a manifest nobody fetched.
        // The note is read at the next grant, which is the same rule applied when there is
        // something to read.
        let mut fixture = Fixture::new("notes-not-installed");
        fixture.provision();
        assert_eq!(
            grant(&mut fixture, r#"{"id":"acme/wave","token":"arm.left"}"#).0,
            201
        );
    }

    #[test]
    fn revoking_never_waits_on_a_note() {
        // §39.1: the safe direction never waits. An unread note blocks a widening and nothing else.
        let mut fixture = with_notes("notes-revoke", Some(NOTES));
        let (_, refusal) = grant(&mut fixture, r#"{"id":"acme/wave","token":"arm.left"}"#);
        let digest = refusal
            .get("digest")
            .and_then(Json::as_str)
            .unwrap()
            .to_string();
        grant(
            &mut fixture,
            &format!(r#"{{"id":"acme/wave","token":"arm.left","acknowledge":"{digest}"}}"#),
        );

        let (status, body) = fixture.request(
            "DELETE",
            "/grants/acme:wave/arm.left",
            Some("admin-token"),
            "",
        );
        assert_eq!(status, 200, "{body:?}");
        assert_eq!(body.get("outcome").and_then(Json::as_str), Some("revoked"));
    }

    #[test]
    fn whitespace_is_not_a_warning() {
        let mut blank = Json::obj();
        blank.set("safety_notes", Json::from("   \n  "));
        assert_eq!(crate::notes::of(&blank), None);

        let mut real = Json::obj();
        real.set("safety_notes", Json::from(NOTES));
        let notes = crate::notes::of(&real).expect("notes");
        assert!(notes.digest.starts_with("sha256:"));
        assert_eq!(notes.text, NOTES);
    }
}

// ── §54.5's declared call graph ──────────────────────────────────────────────
//
// > Depth-limited (default 4) and **acyclic** — a cycle is refused at install by walking the
// > declared graph, **not discovered at runtime**.
//
// The runtime's own check walks the *live ancestry* and refuses the call that would close a cycle.
// It is correct, and it is the thing this sentence forbids being the only check: it fires on a
// robot, with leases held and a deadline running. The declared graph had never been read at all —
// `read_calls` existed, was corpus-verified, and had no caller outside the corpus runner.

mod composition {
    use super::*;

    fn composing(id: &str, calls: &[(&str, &str)]) -> Json {
        let mut m = manifest(id, "1.0.0", "skill");
        let mut composition = Json::obj();
        composition.set(
            "calls",
            Json::Arr(
                calls
                    .iter()
                    .map(|(from, to)| {
                        let mut edge = Json::obj();
                        edge.set("from", Json::from(*from))
                            .set("to", Json::from(*to));
                        edge
                    })
                    .collect(),
            ),
        );
        m.set("composition", composition);
        m
    }

    #[test]
    fn a_graph_that_terminates_is_not_a_fault() {
        let m = composing("acme/pick", &[("acme/pick", "acme/grasp")]);
        assert!(crate::admit::composition_fault("acme/pick", &m).is_none());
    }

    #[test]
    fn a_declared_cycle_is_a_fault_before_anything_invokes_it() {
        let m = composing(
            "acme/pick",
            &[("acme/pick", "acme/grasp"), ("acme/grasp", "acme/pick")],
        );
        let fault = crate::admit::composition_fault("acme/pick", &m).expect("a fault");
        assert_eq!(
            fault.code,
            skynet_contracts::skill::CompositionFaultCode::Cycle
        );
        // §64: the refusal names the path, because "this graph has a cycle" is not something a
        // publisher can act on and `a -> b -> a` is.
        let sentence = crate::admit::composition_refusal("acme/pick", &fault);
        assert!(sentence.contains("acme/grasp"), "{sentence}");
        assert!(sentence.contains("at runtime"), "{sentence}");
    }

    #[test]
    fn a_graph_deeper_than_the_limit_is_a_fault() {
        let m = composing(
            "s/1",
            &[
                ("s/1", "s/2"),
                ("s/2", "s/3"),
                ("s/3", "s/4"),
                ("s/4", "s/5"),
            ],
        );
        let fault = crate::admit::composition_fault("s/1", &m).expect("a fault");
        assert_eq!(
            fault.code,
            skynet_contracts::skill::CompositionFaultCode::TooDeep
        );
    }

    #[test]
    fn a_package_that_declares_no_graph_is_not_walked() {
        // Which is nearly every package. A composition check with an opinion about a policy that
        // has no `composition` key would be a check that refuses things for saying nothing.
        let m = manifest("acme/pick", "1.0.0", "policy");
        assert!(crate::admit::composition_fault("acme/pick", &m).is_none());
        assert!(crate::admit::declared_calls(&m).is_empty());
    }

    #[test]
    fn the_graph_is_read_under_either_spelling() {
        // A manifest may carry the skill card flat or under `skill`, and every other reader here
        // takes both. One that took only one would decline to walk half the manifests in the world.
        let inner = composing("acme/pick", &[("acme/pick", "acme/pick")]);
        let mut nested = manifest("acme/pick", "1.0.0", "skill");
        let mut skill = Json::obj();
        skill.set(
            "composition",
            inner.get("composition").cloned().unwrap_or_else(Json::obj),
        );
        nested.set("skill", skill);
        assert!(crate::admit::composition_fault("acme/pick", &nested).is_some());
    }
}

// ── §55.2's cell, and the diagnosis it makes possible ────────────────────────
//
// The cell has supplied `env.*` to every precondition since §54.2's context was built. What it
// could not do was explain itself: `cell_environment` drops a fact key no dot path can address —
// correct for a context, useless for a person whose skill was just refused for naming it — and
// `validate_cell` had enumerated exactly those faults since the cell was designed, with no caller.

mod cell {
    use super::*;

    /// A profile carrying a cell, applied the way the reconciler applies one.
    fn with_cell(label: &str, cell: &str) -> Fixture {
        let mut fixture = Fixture::new(label);
        fixture.provision();
        let layout = fixture.daemon.store.layout().clone();
        let document = format!(
            "{{\"name\":\"site-a\",\"digest\":\"sha256:aa\",\"limits\":{{}},\"cell\":{cell}}}"
        );
        std::fs::create_dir_all(layout.active_profile().parent().unwrap()).unwrap();
        std::fs::write(layout.active_profile(), document).unwrap();
        fixture
    }

    #[test]
    fn a_robot_with_no_profile_says_so_rather_than_answering_nothing() {
        let mut fixture = Fixture::new("cell-none");
        fixture.provision();
        let (status, body) = fixture.request("GET", "/cell", Some("observer-token"), "");
        assert_eq!(status, 200);
        assert_eq!(body.get("held"), Some(&Json::Bool(false)));
        assert!(body
            .get("note")
            .and_then(Json::as_str)
            .is_some_and(|note| note.contains("unresolvable")));
    }

    #[test]
    fn the_environment_is_the_context_a_precondition_sees() {
        let mut fixture = with_cell(
            "cell-context",
            r#"{"name":"bay-3","facts":{"door_closed":true,"door.closed":2},"fixtures":[{"name":"bench"}]}"#,
        );
        let (status, body) = fixture.request("GET", "/cell", Some("observer-token"), "");
        assert_eq!(status, 200);
        assert_eq!(body.get("held"), Some(&Json::Bool(true)));

        let environment = body.get("environment").expect("an environment");
        assert_eq!(environment.get("door_closed"), Some(&Json::Bool(true)));
        assert!(
            environment.get("door.closed").is_none(),
            "a key with a dot in it is a key no dot path can address, and the context drops it"
        );
        // §55.2's derived facts, which a cell's own may not shadow.
        assert_eq!(
            environment.get("cell").and_then(Json::as_str),
            Some("bay-3")
        );
    }

    /// The whole reason this route exists.
    #[test]
    fn the_faults_say_what_was_dropped_and_why() {
        let mut fixture = with_cell(
            "cell-faults",
            r#"{"name":"bay-3","facts":{"door.closed":2},"fixtures":[{"name":""}]}"#,
        );
        let (_, body) = fixture.request("GET", "/cell", Some("observer-token"), "");
        let codes: Vec<&str> = body
            .arr_or_empty("faults")
            .iter()
            .filter_map(|fault| fault.get("code").and_then(Json::as_str))
            .collect();
        assert!(codes.contains(&"fact_key_unusable"), "{codes:?}");
        assert!(codes.contains(&"fixture_unnamed"), "{codes:?}");
        assert!(body
            .get("note")
            .and_then(Json::as_str)
            .is_some_and(|note| note.contains("only place that says why")));
    }

    /// §38.2's direction, which decides that this reports rather than refuses.
    #[test]
    fn a_faulty_cell_does_not_take_the_limits_with_it() {
        let mut fixture = with_cell("cell-limits", r#"{"name":"","facts":{}}"#);
        let (_, body) = fixture.request("GET", "/cell", Some("observer-token"), "");
        eprintln!("BODY {}", body.to_canonical().unwrap_or_default());
        assert!(!body.arr_or_empty("faults").is_empty(), "an unnamed cell");
        // The profile is still the profile: refusing it for a malformed cell would drop the
        // ceilings with it, which is the *widening* direction and the one that is never safe.
        assert_eq!(
            body.get("profile").and_then(Json::as_str),
            Some("site-a"),
            "the profile in force is unaffected"
        );
    }

    #[test]
    fn the_workspace_layer_is_computed_and_says_it_is_not_applied() {
        let mut fixture = with_cell(
            "cell-workspace",
            r#"{"name":"bay-3","allowed":[{"frame":"base","shape":{"kind":"box","min":[0,0,0],"max":[2,2,2]}}]}"#,
        );
        let (_, body) = fixture.request("GET", "/cell", Some("observer-token"), "");
        let workspace = body.get("workspace").expect("a layer");
        assert_eq!(workspace.arr_or_empty("volumes").len(), 1);
        // §33.1: the guard is the kernel's and this daemon cannot reach it. Drawing the volumes
        // without saying so would suggest they are in force.
        assert!(body
            .get("workspace_note")
            .and_then(Json::as_str)
            .is_some_and(|note| note.contains("§33.1")));
    }

    /// The volume an operator drew and the reader could not parse.
    ///
    /// This one is here because the test above was written with a shape spelled
    /// `{"box":{…}}` — which is not the spelling either edition reads — and asserted a volume it
    /// never got. The route's answer to that document is the interesting one, and it is two
    /// halves that have to arrive together: **no layer**, because a layer built from nothing an
    /// operator can see is the widening direction, and a **fault that names the volume**, because
    /// a null layer with no explanation is indistinguishable from a cell that draws nothing on
    /// purpose (§55.2, and the `CellVolume::shape` option that exists for exactly this).
    #[test]
    fn a_volume_whose_shape_cannot_be_read_draws_no_layer_and_is_named() {
        let mut fixture = with_cell(
            "cell-unreadable",
            r#"{"name":"bay-3","allowed":[{"frame":"base","shape":{"box":{"min":[0,0,0],"max":[2,2,2]}}}]}"#,
        );
        let (_, body) = fixture.request("GET", "/cell", Some("observer-token"), "");
        assert_eq!(
            body.get("workspace"),
            Some(&Json::Null),
            "an unreadable volume is not an empty layer: an empty layer forbids every frame it \
             names, and this cell named one"
        );
        let codes: Vec<&str> = body
            .arr_or_empty("faults")
            .iter()
            .filter_map(|fault| fault.get("code").and_then(Json::as_str))
            .collect();
        assert!(codes.contains(&"volume_shape_invalid"), "{codes:?}");
    }

    #[test]
    fn reading_the_cell_needs_an_observer() {
        let mut fixture = Fixture::new("cell-role");
        fixture.provision();
        assert_eq!(fixture.request("GET", "/cell", None, "").0, 401);
        assert_eq!(
            fixture
                .request("GET", "/cell", Some("observer-token"), "")
                .0,
            200
        );
    }
}

// ── §79's upload: the sealed window's bytes ──────────────────────────────────
//
// `skynet_contracts::upload` proves the eight decisions and the four verdicts for given inputs.
// These prove the half a pure function cannot: that the probe happens before any bytes and its
// answer places them, that a slice resumes where the receiver says it stopped, that a receipt is
// written only against the digest recorded at the seal, that nothing is deleted when an answer is
// wrong, and that a window with a manifest is skipped without being hashed.

mod upload {
    use super::*;
    use crate::upload::{
        advance, extract, next_pending, report, sealed, slice_len, upload_target, url_for, Answer,
        Probe, Push, Receipt, Step, Uploader, SLICE_BYTES, SLICE_BYTES_CEILING,
    };

    /// What a scripted receiver does about one request.
    #[derive(Clone)]
    enum Reply {
        /// "I hold this many bytes", with or without saying what they hash to.
        Holds(u64, Option<&'static str>),
        /// Answer nothing useful: a status and no number.
        Silent(u16),
        /// Fail without answering.
        Fail(&'static str),
        /// "I hold this many bytes, and my address is over there" — a canonicalising Hub.
        Redirected(u64, &'static str),
        /// "I hold this many bytes, and I take slices of at most this many."
        Limits(u64, u64),
    }

    impl Reply {
        fn answer(&self) -> Answer {
            match self {
                Reply::Holds(bytes, digest) => Answer {
                    status: 200,
                    bytes: Some(*bytes),
                    digest: digest.map(str::to_string),
                    max_slice: None,
                    error: None,
                    effective_url: None,
                },
                Reply::Redirected(bytes, url) => Answer {
                    status: 200,
                    bytes: Some(*bytes),
                    digest: None,
                    max_slice: None,
                    error: None,
                    effective_url: Some((*url).to_string()),
                },
                Reply::Limits(bytes, max_slice) => Answer {
                    status: 200,
                    bytes: Some(*bytes),
                    digest: None,
                    max_slice: Some(*max_slice),
                    error: None,
                    effective_url: None,
                },
                Reply::Silent(status) => Answer {
                    status: *status,
                    ..Answer::default()
                },
                Reply::Fail(why) => Answer {
                    error: Some((*why).to_string()),
                    ..Answer::default()
                },
            }
        }
    }

    /// A receiver the suite writes the script for.
    ///
    /// A trait double rather than a local HTTPS server, for [`crate::ota::RecordingUpdater`]'s
    /// reason: the interesting cases here are a receiver that will not say what it holds, one that
    /// claims more than exists, one that stores the wrong bytes and one that cannot be reached, and
    /// none of them is reachable through a network a test controls.
    struct Scripted {
        probes: std::collections::VecDeque<Reply>,
        pushes: std::collections::VecDeque<Reply>,
        probed: Vec<String>,
        pushed: Vec<(u64, String)>,
        /// `(from, len, total)` per push — what a receiver's `Content-Range` would have said.
        ranges: Vec<(u64, u64, u64)>,
    }

    impl Scripted {
        fn new(probes: Vec<Reply>, pushes: Vec<Reply>) -> Scripted {
            Scripted {
                probes: probes.into_iter().collect(),
                pushes: pushes.into_iter().collect(),
                probed: Vec::new(),
                pushed: Vec::new(),
                ranges: Vec::new(),
            }
        }
    }

    impl Uploader for Scripted {
        fn probe(&mut self, ask: &Probe) -> Answer {
            self.probed.push(ask.url.clone());
            self.probes
                .pop_front()
                .unwrap_or(Reply::Fail("the script ran out of probes"))
                .answer()
        }

        fn push(&mut self, ask: &Push) -> Answer {
            self.pushed.push((ask.from, ask.url.clone()));
            self.ranges.push((ask.from, ask.len, ask.total));
            self.pushes
                .pop_front()
                .unwrap_or(Reply::Fail("the script ran out of pushes"))
                .answer()
        }
    }

    const WINDOW: &[u8] = b"an mcap file, as far as this test is concerned, is these bytes.";

    fn digest_of(bytes: &[u8]) -> String {
        format!(
            "sha256:{}",
            String::from_utf8_lossy(&skynet_sha256::hex_bytes(bytes))
        )
    }

    /// Put one sealed window on the disk, with no manifest — an older edition's recording.
    fn seal_bare(layout: &Layout, name: &str, bytes: &[u8]) {
        let dir = layout.blackbox().join("sealed");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    fn bearers(layout: &Layout, metered: bool) -> crate::connectivity::Table {
        let path = layout.state().join("bearers.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!(
                "{{\"bearers\":[{{\"name\":\"link0\",\"kind\":\"ethernet\",\"up\":true,\
                 \"metered\":{metered}}}]}}"
            ),
        )
        .unwrap();
        crate::connectivity::Table::of(layout)
    }

    fn send(
        layout: &Layout,
        server: &mut Scripted,
        table: &crate::connectivity::Table,
        now: u64,
    ) -> Step {
        advance(
            layout,
            server,
            table,
            "https://hub.example/api/skynet/v1",
            "dev_1",
            Some("skd_x"),
            now,
            1.0,
        )
    }

    #[test]
    fn the_route_is_the_digest_and_the_device() {
        // §79.1. The digest is the object's name, so it is a path segment rather than a query, and
        // the `:` in `sha256:…` is encoded rather than left to a router to interpret.
        let url = url_for("https://hub.example/api/skynet/v1/", "dev_1", "sha256:abc");
        assert_eq!(
            url,
            "https://hub.example/api/skynet/v1/devices/dev_1/recordings/sha256%3Aabc"
        );
    }

    /// A `PUT` never goes to a URL ending in a slash, whatever the probe came back with.
    ///
    /// The rule is curl's rather than §79's: `--upload-file` against a slash-terminated URL appends
    /// the *local* file's name, so a push aimed at the canonical address of a window would be sent
    /// to a path named after this daemon's scratch file. It is not a hypothetical shape — a Hub
    /// with `trailingSlash` canonicalisation answers 308 to the form `url_for` builds, and
    /// [`advance`] then reuses that effective URL precisely so a redirect does not cost a re-sent
    /// window.
    ///
    /// Asserted here and not in `curl_push`, which spawns a process. What this pins is the pure
    /// half — the one line that decides whether the bytes reach the route.
    #[test]
    fn a_canonicalised_address_is_trimmed_before_it_reaches_curl() {
        let canonical = "https://hub.example/api/skynet/v1/devices/dev_1/recordings/sha256%3Aabc/";
        assert_eq!(
            upload_target(canonical),
            "https://hub.example/api/skynet/v1/devices/dev_1/recordings/sha256%3Aabc",
        );
        // And the ordinary case is untouched, so nothing about a receiver that does not
        // canonicalise changes.
        let plain = url_for("https://hub.example/api/skynet/v1", "dev_1", "sha256:abc");
        assert_eq!(upload_target(&plain), plain);
        // The address this daemon constructs never ends in one to begin with. If that ever stops
        // being true, the trim above is doing work nobody expects it to do.
        assert!(!plain.ends_with('/'));
    }

    #[test]
    fn a_robot_with_nothing_sealed_is_idle_rather_than_failing() {
        let root = TempRoot::new("upload-idle");
        let layout = root.layout();
        let table = bearers(&layout, false);
        let mut server = Scripted::new(vec![], vec![]);
        match send(&layout, &mut server, &table, 1_000) {
            Step::Idle { .. } => {}
            other => panic!("{other:?}"),
        }
        assert!(server.probed.is_empty(), "nothing to ask about");
    }

    #[test]
    fn a_metered_link_is_not_even_asked() {
        // §79.3's half that costs money. The incident naming this window went over this same
        // bearer when it was raised; the megabytes wait.
        let root = TempRoot::new("upload-metered");
        let layout = root.layout();
        seal_bare(&layout, "00000000000000000001-fault.mcap", WINDOW);
        let table = bearers(&layout, true);
        let mut server = Scripted::new(vec![], vec![]);
        match send(&layout, &mut server, &table, 1_000) {
            Step::Waiting { detail } => assert!(detail.contains("unmetered"), "{detail}"),
            other => panic!("{other:?}"),
        }
        assert!(
            server.probed.is_empty(),
            "a probe is a request, and §34.3's wait is a wait"
        );
    }

    #[test]
    fn the_probe_comes_first_and_a_whole_window_goes_in_one_slice() {
        let root = TempRoot::new("upload-whole");
        let layout = root.layout();
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);
        let table = bearers(&layout, false);
        let digest = digest_of(WINDOW);

        let mut server = Scripted::new(
            vec![Reply::Silent(404)],
            vec![Reply::Holds(
                WINDOW.len() as u64,
                Some(Box::leak(digest.clone().into_boxed_str())),
            )],
        );

        match send(&layout, &mut server, &table, 1_000) {
            Step::Collected { name: got, .. } => assert_eq!(got, name),
            other => panic!("{other:?}"),
        }
        assert_eq!(server.probed.len(), 1);
        assert_eq!(server.pushed, vec![(0, server.probed[0].clone())]);

        // The receipt is on the disk and names these bytes.
        let receipt = Receipt::read(&layout, name, &digest).expect("receipt");
        assert_eq!(receipt.bytes, WINDOW.len() as u64);
        assert_eq!(receipt.at_ms, 1_000);

        // And the window itself is still here. §79.4: the operator standing next to the robot is
        // frequently the one who cannot reach the Hub.
        assert!(layout.blackbox().join("sealed").join(name).is_file());
    }

    #[test]
    fn the_push_goes_to_the_address_the_probe_ended_at() {
        // §79.1 gives the route as a *path*, and a Hub is entitled to canonicalise one. The Hub
        // this OS is built against does: `trailingSlash` is on, so `…/recordings/sha256%3A…`
        // answers `308` and the address is the same path with a slash. Reproduced against the
        // running Hub before this test was written — every robot-facing route on it behaves the
        // same way, which is why `--location` went on the outbound queue in the same commit.
        //
        // Following it is `curl`'s job. Following it **once** is this function's: the probe runs on
        // every pass before any bytes move, so the discovery is a round trip already paid for, and
        // a push that started again from the original address would pay for the redirect in
        // megabytes rather than in one small request.
        let root = TempRoot::new("upload-redirect");
        let layout = root.layout();
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);
        let table = bearers(&layout, false);
        let digest = digest_of(WINDOW);

        let canonical = "https://hub.example/api/skynet/v1/devices/dev_1/recordings/x/";
        let mut server = Scripted::new(
            vec![Reply::Redirected(0, canonical)],
            vec![Reply::Holds(
                WINDOW.len() as u64,
                Some(Box::leak(digest.clone().into_boxed_str())),
            )],
        );

        match send(&layout, &mut server, &table, 1_000) {
            Step::Collected { .. } => {}
            other => panic!("{other:?}"),
        }

        // The probe asked at the address §79.1 prints…
        assert_eq!(server.probed.len(), 1);
        assert!(server.probed[0].contains("/recordings/sha256%3A"));
        // …and the bytes went to the one it ended at.
        assert_eq!(server.pushed, vec![(0, canonical.to_string())]);
    }

    #[test]
    fn a_slice_is_bounded_by_bytes_and_not_only_by_seconds() {
        // The defect this closes, stated as an assertion: `SLICE_SECONDS` bounds the *pass*, which
        // is a promise to the supervisor, and it promises a receiver nothing. A robot on a fast
        // link offering a hundred-megabyte window inside twenty seconds is offering a
        // hundred-megabyte request body, and no receiver takes one — the Hub this OS is built
        // against refuses a body over four and a half megabytes before its own code runs.
        //
        // The window here is short, so `remaining` is the binding constraint and the receiver's
        // limit is not. That is the ordinary case and it is worth pinning too: the bound must never
        // make a robot send *more* than the window has left.
        assert_eq!(slice_len(10, None), 10);
        // Longer than the floor, and nobody has said otherwise.
        assert_eq!(slice_len(SLICE_BYTES * 3, None), SLICE_BYTES);
        // The receiver said, so the receiver decides.
        assert_eq!(slice_len(SLICE_BYTES * 3, Some(1_000)), 1_000);
        assert_eq!(
            slice_len(SLICE_BYTES * 3, Some(SLICE_BYTES * 2)),
            SLICE_BYTES * 2
        );
        // …up to the point where what it said would decide how much of this disk to copy.
        assert_eq!(
            slice_len(u64::MAX, Some(u64::MAX)),
            SLICE_BYTES_CEILING,
            "a receiver's claim is clamped: this number sizes a scratch file on the robot's disk",
        );
        // Nonsense from the far side does not produce a zero-length request, which would be a
        // transfer that never advances and never fails.
        assert!(slice_len(100, Some(0)) >= 1);
    }

    #[test]
    fn the_push_carries_the_window_s_length_and_this_request_s_share_of_it() {
        // `Content-Range: bytes <from>-<from+len-1>/<total>` — the header curl used to generate
        // from `--continue-at`, now written out, because a bounded slice has an end that is not the
        // window's end and there is no curl flag for one on an upload.
        let root = TempRoot::new("upload-bounded");
        let layout = root.layout();
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);
        let table = bearers(&layout, false);
        let digest = digest_of(WINDOW);

        // A receiver that takes four bytes at a time, which is smaller than the window.
        let mut server = Scripted::new(vec![Reply::Limits(0, 4)], vec![Reply::Holds(4, None)]);
        match send(&layout, &mut server, &table, 1_000) {
            Step::Moved { held, .. } => assert_eq!(held, Some(4)),
            other => panic!("{other:?}"),
        }
        assert_eq!(server.ranges, vec![(0, 4, WINDOW.len() as u64)]);

        // And the next pass resumes where it said it stopped, still four at a time.
        let mut server = Scripted::new(
            vec![Reply::Limits(4, 4)],
            vec![Reply::Holds(
                WINDOW.len() as u64,
                Some(Box::leak(digest.clone().into_boxed_str())),
            )],
        );
        match send(&layout, &mut server, &table, 2_000) {
            Step::Collected { .. } => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(server.ranges, vec![(4, 4, WINDOW.len() as u64)]);
    }

    #[test]
    fn a_slice_is_the_window_s_bytes_at_the_offset_and_no_others() {
        // `extract` is the one piece of this path that touches both files, and getting it wrong
        // produces bytes that hash to nothing — a transfer that completes, fails its digest, and
        // restarts for ever. Checked directly rather than through a receiver, because a receiver
        // that saw the wrong bytes would report exactly what a receiver seeing the right ones does.
        let root = TempRoot::new("upload-extract");
        let layout = root.layout();
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);

        let ask = Push {
            url: "https://hub.example/x".to_string(),
            credential: None,
            from: 3,
            len: 5,
            total: WINDOW.len() as u64,
            file: layout.blackbox().join("sealed").join(name),
            scratch: layout.upload_slice(),
            budget_s: 20,
        };
        assert_eq!(extract(&ask).expect("extracted"), 5);
        assert_eq!(std::fs::read(&ask.scratch).expect("slice"), &WINDOW[3..8]);

        // Asking for more than the window has left copies what is there rather than padding. The
        // digest is what notices a window that changed under this daemon; `extract` does not
        // pretend to know.
        let tail = Push {
            from: WINDOW.len() as u64 - 2,
            len: 1_000,
            ..ask
        };
        assert_eq!(extract(&tail).expect("extracted"), 2);
        assert_eq!(
            std::fs::read(&tail.scratch).expect("slice"),
            &WINDOW[WINDOW.len() - 2..]
        );
    }

    #[test]
    fn a_receiver_that_does_not_redirect_is_pushed_to_where_it_was_probed() {
        // The other half of the pair, and the one that would break silently if `effective_url` were
        // ever defaulted to something rather than left absent: a Hub that serves the path §79.1
        // prints must keep being addressed at it.
        let root = TempRoot::new("upload-no-redirect");
        let layout = root.layout();
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);
        let table = bearers(&layout, false);
        let digest = digest_of(WINDOW);

        let mut server = Scripted::new(
            vec![Reply::Holds(0, None)],
            vec![Reply::Holds(
                WINDOW.len() as u64,
                Some(Box::leak(digest.clone().into_boxed_str())),
            )],
        );

        match send(&layout, &mut server, &table, 1_000) {
            Step::Collected { .. } => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(server.pushed, vec![(0, server.probed[0].clone())]);
    }

    #[test]
    fn a_collected_window_is_not_offered_twice() {
        let root = TempRoot::new("upload-twice");
        let layout = root.layout();
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);
        let table = bearers(&layout, false);
        let digest = digest_of(WINDOW);
        Receipt {
            digest: digest.clone(),
            bytes: WINDOW.len() as u64,
            at_ms: 5,
        }
        .write(&layout, name)
        .unwrap();

        let mut server = Scripted::new(vec![], vec![]);
        match send(&layout, &mut server, &table, 1_000) {
            Step::Idle { .. } => {}
            other => panic!("{other:?}"),
        }
        assert!(server.probed.is_empty());
        assert!(next_pending(&layout).is_none());
    }

    #[test]
    fn a_receipt_for_other_bytes_is_not_this_window_s() {
        // §13 names a window for a monotonic instant and a reboot restarts that clock, so a name
        // is reused and a file at that name is overwritten. A receipt believed on its filename
        // alone would report the *new* window as collected.
        let root = TempRoot::new("upload-stale-receipt");
        let layout = root.layout();
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);
        Receipt {
            digest: digest_of(b"a different window entirely"),
            bytes: 27,
            at_ms: 5,
        }
        .write(&layout, name)
        .unwrap();

        assert!(Receipt::read(&layout, name, &digest_of(WINDOW)).is_none());
        let pending = next_pending(&layout).expect("this window has not been collected");
        assert_eq!(pending.digest, digest_of(WINDOW));
    }

    #[test]
    fn a_resume_starts_where_the_receiver_says_it_stopped() {
        let root = TempRoot::new("upload-resume");
        let layout = root.layout();
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);
        let table = bearers(&layout, false);
        let digest = digest_of(WINDOW);
        let half = (WINDOW.len() / 2) as u64;

        let mut server = Scripted::new(
            vec![Reply::Silent(404), Reply::Holds(half, None)],
            vec![
                Reply::Holds(half, None),
                Reply::Holds(
                    WINDOW.len() as u64,
                    Some(Box::leak(digest.clone().into_boxed_str())),
                ),
            ],
        );

        match send(&layout, &mut server, &table, 1_000) {
            Step::Moved { held, .. } => assert_eq!(held, Some(half)),
            other => panic!("{other:?}"),
        }
        match send(&layout, &mut server, &table, 2_000) {
            Step::Collected { .. } => {}
            other => panic!("{other:?}"),
        }
        // Zero, then the offset the receiver reported. Never a second zero: re-sending from the
        // beginning is what §79.2's probe exists to avoid.
        assert_eq!(
            server
                .pushed
                .iter()
                .map(|(from, _)| *from)
                .collect::<Vec<_>>(),
            vec![0, half]
        );
    }

    #[test]
    fn a_probe_that_says_nothing_moves_no_bytes_and_backs_off() {
        // §79.2's second refusal. Pushing from zero whenever a probe failed would re-send the whole
        // window every pass for as long as the failure lasted.
        let root = TempRoot::new("upload-blind");
        let layout = root.layout();
        seal_bare(&layout, "00000000000000000001-fault.mcap", WINDOW);
        let table = bearers(&layout, false);

        let mut server = Scripted::new(vec![Reply::Silent(500)], vec![]);
        match send(&layout, &mut server, &table, 1_000) {
            Step::Failed { detail } => {
                assert!(detail.contains("without saying how much"), "{detail}")
            }
            other => panic!("{other:?}"),
        }
        assert!(server.pushed.is_empty(), "no bytes on a blind offset");

        // And the next pass is held on §34.4's curve rather than retried immediately.
        let mut again = Scripted::new(vec![Reply::Silent(404)], vec![]);
        match send(&layout, &mut again, &table, 1_100) {
            Step::Waiting { detail } => assert!(detail.contains("curve"), "{detail}"),
            other => panic!("{other:?}"),
        }
        assert!(again.probed.is_empty());
    }

    #[test]
    fn a_receiver_claiming_more_than_the_window_holds_gets_it_from_the_beginning() {
        let root = TempRoot::new("upload-overlong");
        let layout = root.layout();
        seal_bare(&layout, "00000000000000000001-fault.mcap", WINDOW);
        let table = bearers(&layout, false);

        let mut server = Scripted::new(vec![Reply::Holds(WINDOW.len() as u64 + 500, None)], vec![]);
        match send(&layout, &mut server, &table, 1_000) {
            // `restart` from a *probe* is not a push from zero this pass: the plan says restart,
            // and the push that follows carries `from: 0`.
            Step::Moved { .. } | Step::Failed { .. } => {}
            other => panic!("{other:?}"),
        }
        assert!(
            server.pushed.iter().all(|(from, _)| *from == 0),
            "never resume from an impossible offset"
        );
    }

    #[test]
    fn the_wrong_bytes_over_there_write_no_receipt_and_delete_nothing() {
        let root = TempRoot::new("upload-wrong");
        let layout = root.layout();
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);
        let table = bearers(&layout, false);

        let mut server = Scripted::new(
            vec![Reply::Silent(404)],
            vec![Reply::Holds(WINDOW.len() as u64, Some("sha256:nope"))],
        );
        match send(&layout, &mut server, &table, 1_000) {
            Step::Failed { detail } => assert!(detail.contains("not this window"), "{detail}"),
            other => panic!("{other:?}"),
        }
        assert!(Receipt::read(&layout, name, &digest_of(WINDOW)).is_none());
        // Nothing here can delete what is over there, and it does not delete what is here either.
        assert_eq!(
            std::fs::read(layout.blackbox().join("sealed").join(name)).unwrap(),
            WINDOW
        );
    }

    #[test]
    fn a_receiver_that_already_has_it_writes_a_receipt_without_sending_anything() {
        // The acknowledgement that was lost. The re-send this avoids is the whole window.
        let root = TempRoot::new("upload-already");
        let layout = root.layout();
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);
        let table = bearers(&layout, false);
        let digest = digest_of(WINDOW);

        let mut server = Scripted::new(
            vec![Reply::Holds(
                WINDOW.len() as u64,
                Some(Box::leak(digest.clone().into_boxed_str())),
            )],
            vec![],
        );
        match send(&layout, &mut server, &table, 1_000) {
            Step::Collected { .. } => {}
            other => panic!("{other:?}"),
        }
        assert!(server.pushed.is_empty(), "not one byte");
        assert!(Receipt::read(&layout, name, &digest).is_some());
    }

    #[test]
    fn the_oldest_uncollected_window_goes_first_and_alone() {
        let root = TempRoot::new("upload-order");
        let layout = root.layout();
        seal_bare(&layout, "00000000000000000009-operator.mcap", b"newer");
        seal_bare(&layout, "00000000000000000002-fault.mcap", b"older");
        let table = bearers(&layout, false);

        let mut server = Scripted::new(
            vec![Reply::Silent(404)],
            vec![Reply::Holds(
                5,
                Some(Box::leak(digest_of(b"older").into_boxed_str())),
            )],
        );
        match send(&layout, &mut server, &table, 1_000) {
            Step::Collected { name, .. } => assert_eq!(name, "00000000000000000002-fault.mcap"),
            other => panic!("{other:?}"),
        }
        // One window per pass. The newer one is still pending.
        let pending = next_pending(&layout).expect("the newer window");
        assert_eq!(pending.name, "00000000000000000009-operator.mcap");
    }

    #[test]
    fn the_report_says_what_is_here_and_what_has_been_collected() {
        let root = TempRoot::new("upload-report");
        let layout = root.layout();
        seal_bare(&layout, "00000000000000000001-fault.mcap", WINDOW);
        seal_bare(&layout, "00000000000000000002-operator.mcap", b"short");
        Receipt {
            digest: digest_of(WINDOW),
            bytes: WINDOW.len() as u64,
            at_ms: 5,
        }
        .write(&layout, "00000000000000000001-fault.mcap")
        .unwrap();

        let document = report(&layout);
        assert_eq!(document.get("sealed").and_then(Json::as_f64), Some(2.0));
        assert_eq!(document.get("collected").and_then(Json::as_f64), Some(1.0));
        assert_eq!(document.get("pending").and_then(Json::as_f64), Some(1.0));
        assert_eq!(
            document.get("pending_bytes").and_then(Json::as_f64),
            Some(5.0)
        );

        // Every window is listed whether or not it has been collected — one shape, discriminated
        // by a field, for §77.2's reason.
        let listed = document.arr_or_empty("windows");
        assert_eq!(listed.len(), 2);
        assert_eq!(
            listed
                .iter()
                .filter(|entry| entry
                    .get("collected")
                    .and_then(Json::as_bool)
                    .unwrap_or(false))
                .count(),
            1
        );

        // And `sealed()` agrees with it, which is the pair the console reads.
        assert_eq!(sealed(&layout).iter().filter(|w| w.collected).count(), 1);
    }

    #[test]
    fn a_manifest_left_beside_a_reused_name_is_not_believed() {
        // The window manifest is a cache and the length is its key. A robot that rebooted, sealed
        // a different window at the same monotonic instant and overwrote the file must not inherit
        // the old digest — which would upload these bytes under the previous window's name.
        let root = TempRoot::new("upload-stale-manifest");
        let layout = root.layout();
        let dir = layout.blackbox().join("sealed");
        let name = "00000000000000000001-fault.mcap";
        seal_bare(&layout, name, WINDOW);
        std::fs::write(
            crate::blackbox::manifest_path(&dir, name),
            format!(
                "{{\"digest\":\"sha256:stale\",\"size_bytes\":{}}}",
                WINDOW.len() + 1
            ),
        )
        .unwrap();

        assert_eq!(
            crate::blackbox::read_manifest(&dir, name, WINDOW.len() as u64),
            None,
            "a manifest describing a file of another length is about other bytes"
        );
        let pending = next_pending(&layout).expect("pending");
        assert_eq!(pending.digest, digest_of(WINDOW));
    }

    #[test]
    fn a_seal_writes_the_manifest_the_collector_reads() {
        // The producer records the digest, which is what keeps the hash off the reconcile path —
        // and it has to be the *same* digest the collector would compute, or the cache is a bug
        // that only appears on a robot with recordings.
        let root = TempRoot::new("upload-manifest-roundtrip");
        let layout = root.layout();
        let mut recorder = crate::blackbox::Recorder::open(
            &layout,
            crate::blackbox::supervisor_meta(&layout),
            skynet_blackbox::record::Shape {
                joints: 0,
                channels: 0,
            },
            64 * 1024,
        )
        .expect("recorder");
        recorder
            .event(
                1,
                skynet_blackbox::record::EventCode::Sealed,
                "test",
                "a window worth keeping",
            )
            .unwrap();
        let (written, path) = recorder
            .seal(skynet_blackbox::seal::Reason::Operator, 7)
            .unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let dir = layout.blackbox().join("sealed");

        let declared = crate::blackbox::read_manifest(&dir, &name, written.bytes.len() as u64)
            .expect("the seal wrote a manifest");
        let (computed, _) = crate::bundle::hash_file(&dir.join(&name)).unwrap();
        assert_eq!(declared, computed);
    }
}

// ── §49.1's warning tone ─────────────────────────────────────────────────────
//
// Two thirds of §49.1 were enforceable before this: `hri.warning` is reserved so no grant can
// confer it, and `warning_required` decides which transitions demand a tone. The third — anything
// that makes a sound — did not exist, and `Transition::warning` reached `observe_state` and was
// dropped.
//
// The case worth reading first is `a_robot_that_cannot_warn_raises_an_incident`. This process
// cannot refuse to enter a commanding state, because §27's state is derived rather than granted, so
// the only honest answer to a missing safety control is to be loud about it.

mod audio {
    use super::*;
    use crate::audio::{warn_for, Channel, Sounded, Sounder};
    use skynet_contracts::opstate::{OperatingState, StateReason, Transition};

    /// A speaker the suite writes the script for.
    struct Scripted {
        answers: std::collections::VecDeque<Sounded>,
        heard: Vec<Vec<String>>,
    }

    impl Scripted {
        fn new(answers: Vec<Sounded>) -> Scripted {
            Scripted {
                answers: answers.into_iter().collect(),
                heard: Vec::new(),
            }
        }
    }

    impl Sounder for Scripted {
        fn warn(&mut self, channel: &Channel) -> Sounded {
            self.heard.push(channel.command.clone());
            self.answers.pop_front().unwrap_or(Sounded::Yes)
        }
    }

    fn declared() -> Channel {
        Channel {
            command: vec!["aplay".to_string(), "warning.wav".to_string()],
            label: "mast klaxon".to_string(),
        }
    }

    fn transition(from: OperatingState, to: OperatingState) -> Transition {
        Transition {
            from,
            to,
            reason: StateReason::Nominal,
            changed: from != to,
            warning: skynet_contracts::opstate::warning_required(from, to),
        }
    }

    #[test]
    fn the_tone_sounds_when_a_policy_begins_commanding_and_not_otherwise() {
        let mut speaker = Scripted::new(vec![Sounded::Yes, Sounded::Yes]);
        let channel = declared();

        // Into a commanding state: the tone sounds.
        assert_eq!(
            warn_for(
                &mut speaker,
                &channel,
                &transition(OperatingState::Hold, OperatingState::Running)
            ),
            Some(Sounded::Yes)
        );

        // §27's `DEGRADED` is the impaired *running* state, not a third resting one — so entering
        // it from `hold` is a policy beginning to command a body and demands the same tone. This is
        // the case the old string-typed predicate got wrong.
        assert_eq!(
            warn_for(
                &mut speaker,
                &channel,
                &transition(OperatingState::Hold, OperatingState::Degraded)
            ),
            Some(Sounded::Yes)
        );

        // And moving between two commanding states does not re-warn: the motion never stopped.
        assert_eq!(
            warn_for(
                &mut speaker,
                &channel,
                &transition(OperatingState::Running, OperatingState::Degraded)
            ),
            None
        );
        assert_eq!(speaker.heard.len(), 2);
    }

    #[test]
    fn resumption_after_a_stop_warns_again() {
        // §49.1: "on transition into RUNNING **and on resumption after a stop**". It falls out of
        // the predicate rather than being a second rule — `safe` and `stopping` command nothing.
        let mut speaker = Scripted::new(vec![Sounded::Yes]);
        assert_eq!(
            warn_for(
                &mut speaker,
                &declared(),
                &transition(OperatingState::Safe, OperatingState::Running)
            ),
            Some(Sounded::Yes)
        );
    }

    #[test]
    fn an_undeclared_channel_is_reported_rather_than_guessed_at() {
        // Which output is the warning speaker is a property of the machine. A default device would
        // be a robot that believes it is warning people while playing into an unplugged port.
        let mut speaker = Scripted::new(vec![Sounded::Yes]);
        let outcome = warn_for(
            &mut speaker,
            &Channel::default(),
            &transition(OperatingState::Hold, OperatingState::Running),
        );
        assert_eq!(outcome, Some(Sounded::Undeclared));
        assert!(
            speaker.heard.is_empty(),
            "nothing was asked to make a noise"
        );
        assert!(!outcome.unwrap().ok());
    }

    /// A robot with a declared warning channel, and a speaker the test controls.
    fn robot(label: &str, answers: Vec<Sounded>) -> Fixture {
        let mut fixture = Fixture::new(label);
        let layout = fixture.daemon.store.layout().clone();
        std::fs::create_dir_all(layout.state()).unwrap();
        std::fs::write(
            layout.state().join("audio.json"),
            r#"{"warning":{"command":["aplay","warning.wav"],"label":"mast klaxon"}}"#,
        )
        .unwrap();
        fixture.daemon.sounder = Box::new(Scripted::new(answers));
        fixture
    }

    /// Every incident in §34.4's queue, decoded.
    fn incidents(layout: &Layout) -> Vec<Json> {
        let queue = crate::outbound::Queue::of(layout);
        let mut found = Vec::new();
        while let Some(item) = queue.next() {
            if item.class == crate::outbound::Class::Incident {
                if let Ok(document) = Json::parse(&item.body) {
                    found.push(document);
                }
            }
            queue.remove(&item.id).unwrap();
        }
        found
    }

    #[test]
    fn a_robot_that_cannot_warn_raises_an_incident() {
        // §49.1 makes the tone a control an operator's risk assessment relies on, and this process
        // cannot refuse to enter a commanding state — §27's state is derived, not granted. So the
        // failure is raised, which §34.4 never drops, and it names the channel that did not work
        // rather than only the fact that something did not.
        let mut fixture = robot(
            "audio-incident",
            vec![Sounded::Failed("no such device".into())],
        );
        let layout = fixture.daemon.store.layout().clone();

        let outcome = fixture
            .daemon
            .warn_for_transition(&transition(OperatingState::Hold, OperatingState::Running))
            .expect("this transition demands a tone");
        assert!(!outcome.ok());

        let raised = incidents(&layout);
        assert_eq!(raised.len(), 1, "one incident, never dropped (§34.4)");
        let incident = &raised[0];
        assert_eq!(
            incident.get("kind").and_then(Json::as_str),
            Some("warning_did_not_sound")
        );
        assert_eq!(incident.get("from").and_then(Json::as_str), Some("hold"));
        assert_eq!(incident.get("to").and_then(Json::as_str), Some("running"));
        assert!(incident
            .get("detail")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .contains("no such device"));
        // The channel goes with it. An investigator reading "the warning did not sound" needs to
        // know what this robot believed its warning channel *was*, which is the field that
        // distinguishes a broken speaker from a provisioning script that named the wrong device.
        assert_eq!(
            incident
                .get("channel")
                .and_then(|channel| channel.get("declared"))
                .and_then(Json::as_bool),
            Some(true)
        );
    }

    #[test]
    fn a_warning_that_sounds_raises_nothing() {
        // The other half, and the one that keeps the rule above from being an alarm generator: a
        // robot that warns correctly on every transition must leave the queue empty, or an operator
        // learns to close these without reading them.
        let mut fixture = robot("audio-quiet", vec![Sounded::Yes]);
        let layout = fixture.daemon.store.layout().clone();
        let outcome = fixture
            .daemon
            .warn_for_transition(&transition(OperatingState::Safe, OperatingState::Running));
        assert_eq!(outcome, Some(Sounded::Yes));
        assert!(incidents(&layout).is_empty());
    }

    #[test]
    fn a_transition_that_needs_no_tone_neither_sounds_nor_records() {
        let mut fixture = robot("audio-none", vec![]);
        let layout = fixture.daemon.store.layout().clone();
        assert_eq!(
            fixture
                .daemon
                .warn_for_transition(&transition(OperatingState::Hold, OperatingState::Safe)),
            None
        );
        assert!(fixture.daemon.last_warning.is_none());
        assert!(incidents(&layout).is_empty());
    }

    #[test]
    fn the_channel_is_read_from_the_file_and_absent_where_there_is_none() {
        let root = TempRoot::new("audio-channel");
        let layout = root.layout();
        assert!(!Channel::of(&layout).declared());
        assert!(Channel::of(&layout).detail().contains("no warning channel"));

        std::fs::create_dir_all(layout.state()).unwrap();
        std::fs::write(
            layout.state().join("audio.json"),
            r#"{"warning":{"command":["speaker-test","-t","sine"],"label":"cabinet buzzer"}}"#,
        )
        .unwrap();
        let channel = Channel::of(&layout);
        assert!(channel.declared());
        assert_eq!(channel.label, "cabinet buzzer");
        assert_eq!(channel.command.len(), 3);

        // A declaration with an empty command is not a declaration. It is the shape a
        // half-finished provisioning script leaves behind, and reading it as "declared" would give
        // this robot a warning channel that runs nothing.
        std::fs::write(
            layout.state().join("audio.json"),
            r#"{"warning":{"command":[],"label":"nothing"}}"#,
        )
        .unwrap();
        assert!(!Channel::of(&layout).declared());
    }

    #[test]
    fn the_state_route_says_whether_this_robot_can_warn_and_whether_it_has() {
        let mut fixture = Fixture::new("audio-route");
        fixture.provision();
        let (status, body) = fixture.request("GET", "/state", Some("observer-token"), "");
        assert_eq!(status, 200);
        let warning = body
            .get("warning")
            .expect("§49.1's channel is on this route");
        assert_eq!(warning.get("declared").and_then(Json::as_bool), Some(false));

        // Three states, and this is the third: nothing has been attempted, which is not a success
        // and not a failure. A `last: false` here would report a robot as having failed to warn
        // when it has never been asked to.
        assert!(warning.get_present("last").is_none());
        assert!(warning
            .get("last_detail")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .contains("has yet required"));
    }

    #[test]
    fn no_grant_can_confer_the_warning_channel() {
        // The half that was already built, asserted here beside the half that was not — because
        // §49.1 is one sentence and a suite that tested the two in different files would let one of
        // them rot. `hri.warning` exists precisely to be refused.
        assert!(skynet_contracts::admission::is_reserved(
            skynet_contracts::admission::WARNING_CHANNEL
        ));
        assert!(!skynet_contracts::admission::is_reserved("hri.speech"));
        assert!(!skynet_contracts::admission::is_reserved("sensing.audio"));
    }
}

// ── §54.2's omissions, which nothing had ever shown ──────────────────────────
//
// `explain_omissions` documents its own consumer — "what a console shows about a context it could
// not fully build" — and there was no console showing it. `advance_one` built a context, read
// `built.context` for the preconditions, and dropped `unaddressable` and `unreadable`.
//
// The gap is not cosmetic. §54.2's refusal names a **path**, which is the question; the omission is
// the answer, and "absent because a sensor reported NaN" and "false" send an operator to two
// different places.
//
// No invocation is needed to exercise it, and that is itself the point: the omissions are facts
// about the *robot*, identical for every skill on it, which is why they are refreshed per pass and
// hang off the runtime rather than off an invocation.

mod context_omissions {
    use skynet_contracts::live::{JointReading, LiveInputs};
    use skynet_contracts::opstate::OperatingState;
    use skynetd::lease::LeaseTable;

    /// One pass with nothing invoked, over the joints a test names.
    fn omissions_for(joints: Vec<JointReading>) -> Vec<String> {
        let mut runtime = crate::skill::Runtime::new();
        pass(&mut runtime, joints);
        runtime.omissions().to_vec()
    }

    fn pass(runtime: &mut crate::skill::Runtime, joints: Vec<JointReading>) {
        let inputs = LiveInputs {
            battery_percent: Some(80.0),
            operating_state: Some(OperatingState::Running),
            joints,
            ..LiveInputs::default()
        };
        let running = std::collections::BTreeSet::new();
        let grants: Vec<(String, String)> = Vec::new();
        let mut leases = LeaseTable::new();
        let mut cycle = crate::skill::Cycle {
            now_ms: 0,
            inputs: &inputs,
            robot: OperatingState::Running,
            interventions_since: None,
            wall_ms: Some(1_788_307_200_000),
            running: &running,
            leases: &mut leases,
            grants: &grants,
            power_withheld: None,
        };
        runtime.advance_all(&mut cycle);
    }

    #[test]
    fn a_joint_reporting_nan_reaches_the_surface_as_a_sentence() {
        let mut runtime = crate::skill::Runtime::new();
        pass(
            &mut runtime,
            vec![JointReading::at("shoulder_pitch", f64::NAN)],
        );

        let omissions = runtime.omissions();
        assert_eq!(omissions.len(), 1, "{omissions:?}");
        assert!(omissions[0].contains("shoulder_pitch"), "{}", omissions[0]);
        assert!(omissions[0].contains("did not report a finite value"));

        // And on the document a console reads. Always present, empty where there is nothing to
        // say, so its absence means "this edition does not report them" and never "there are none".
        assert_eq!(runtime.to_json().str_arr("context_omissions").len(), 1);
    }

    #[test]
    fn a_name_a_path_cannot_address_is_named_too() {
        // §54.2 addresses members with dots, so a joint whose own name contains one cannot be
        // written in a precondition at all. Nothing here can be done about it — the body chose the
        // name — which is exactly why it has to be said out loud rather than silently dropped.
        let omissions = omissions_for(vec![JointReading::at("arm.wrist", 0.5)]);
        assert_eq!(omissions.len(), 1);
        assert!(omissions[0].contains("carries a dot"), "{}", omissions[0]);
    }

    #[test]
    fn a_robot_whose_context_is_whole_says_nothing() {
        // The half that keeps the rule above from becoming noise. A healthy robot must produce an
        // empty list, or an operator learns to skim the one place this information appears.
        let mut runtime = crate::skill::Runtime::new();
        pass(&mut runtime, vec![JointReading::at("shoulder_pitch", 0.25)]);
        assert!(runtime.omissions().is_empty());
        assert!(runtime.to_json().str_arr("context_omissions").is_empty());
    }

    #[test]
    fn the_list_is_refreshed_rather_than_accumulated() {
        // Both entries are facts about the robot *now*: a sensor that resumes reporting a finite
        // number stops being unreadable. A list that persisted would send an operator after a
        // sensor that has since recovered.
        let mut runtime = crate::skill::Runtime::new();
        pass(
            &mut runtime,
            vec![JointReading::at("shoulder_pitch", f64::NAN)],
        );
        assert_eq!(runtime.omissions().len(), 1);

        pass(&mut runtime, vec![JointReading::at("shoulder_pitch", 0.25)]);
        assert!(runtime.omissions().is_empty(), "the sensor recovered");
    }
}

// ── Two facts this robot knew and §54.2 could not read ───────────────────────
//
// The same shape as `explain_omissions`, one layer down: `LiveInputs` has a field, the robot has
// the number, and nothing connected them. `battery_percent` is the sharper of the two because it is
// the example §54.2's own module documentation uses —
//
//   > A robot with no battery sensor that reported `robot.state.battery_percent = 0` would refuse
//   > every skill; one that reported `100` would run every skill; one that omits the path refuses
//   > only the skills that asked.
//
// — and this robot reads its own state of charge on every pass, prints it on `GET /power`, renders
// it in §33.6's health column and carries it to the Hub. The one place it did not arrive was the
// context that decides whether a skill may start.

mod live_context_inputs {
    use super::*;

    #[test]
    fn the_battery_reaches_the_precondition_context_as_a_percentage() {
        let mut fixture = Fixture::new("live-battery");
        fixture.provision();
        let now = 1_000;

        // Without a reading, absent — never a zero and never a hundred. §54.2's first rule, and the
        // reason the field is an `Option` at all.
        let blind = fixture.daemon.live_inputs(None, None);
        assert_eq!(blind.battery_percent, None);

        // With one, the number the rest of the platform reports, converted once at the boundary:
        // §33.6 wants a fraction because a fleet page draws a bar, and a precondition is written by
        // a person who writes `> 20`.
        let reading = crate::power::Reading {
            charge_fraction: Some(0.42),
            ..fixture.daemon.read_power(now)
        };
        let seen = fixture.daemon.live_inputs(None, Some(&reading));
        assert_eq!(seen.battery_percent, Some(42.0));

        // And it is addressable, which is the half that matters: a value in a struct nothing
        // publishes is the gap this test exists to close.
        let built = skynet_contracts::live::live_context(&seen);
        let percent = built
            .context
            .get_present("robot")
            .and_then(|robot| robot.get_present("state"))
            .and_then(|state| state.get_present("battery_percent"))
            .and_then(Json::as_f64);
        assert_eq!(percent, Some(42.0));
    }

    #[test]
    fn a_guard_with_no_input_is_named_in_the_context() {
        // §45.5's unavailable set is derived from what the *body declares* — a chassis with no IMU
        // can never run `balance` — so it is answerable on this side of §33.1's line and is already
        // computed twice a pass for `GET /limits` and the check-in.
        let mut fixture = Fixture::new("live-guards");
        fixture.provision();
        write_embodiment(&fixture, BENCH_ARM_WATCHDOG);

        let inputs = fixture.daemon.live_inputs(None, None);
        assert!(
            !inputs.guards_unavailable.is_empty(),
            "a bench arm cannot feed every guard"
        );

        let built = skynet_contracts::live::live_context(&inputs);
        let unavailable = built
            .context
            .get_present("robot")
            .and_then(|robot| robot.get_present("guards"))
            .and_then(|guards| guards.get_present("unavailable"))
            .cloned()
            .expect("§45.5's set is in the context");
        let names: Vec<&str> = unavailable
            .as_arr()
            .map(|items| items.iter().filter_map(Json::as_str).collect())
            .unwrap_or_default();
        assert_eq!(names.len(), inputs.guards_unavailable.len());

        // `guards.active` stays absent, and the asymmetry is the point. Running is a fact about a
        // kernel this process has no channel to; unavailable is a fact about a body on this disk.
        assert!(built
            .context
            .get_present("robot")
            .and_then(|robot| robot.get_present("guards"))
            .and_then(|guards| guards.get_present("active"))
            .is_none());
    }

    #[test]
    fn the_tier_is_still_refused_and_that_is_deliberate() {
        // The one field on this struct that must stay absent. §15 awards a tier and no robot may
        // assert one about itself — `limits.rs` says so in as many words, and a sweep that filled
        // every empty field would have filled this one too.
        let mut fixture = Fixture::new("live-tier");
        fixture.provision();
        assert_eq!(fixture.daemon.live_inputs(None, None).tier, None);
    }
}

// ── §80's retention of sealed windows ────────────────────────────────────────
//
// The three conditions are tested against **each other** rather than one at a time, because the
// rule is a conjunction and every interesting failure is one term being sufficient on its own. A
// suite that checked "a cited window is kept" and "an uncollected window is kept" separately would
// pass against an implementation that reclaimed whenever *either* held.

mod retention {
    use super::TempRoot;
    use crate::retention::{self, Candidate, Free, Kept};

    fn window(name: &str, size: u64, receipted: bool, cited: bool) -> Candidate {
        Candidate {
            name: name.to_string(),
            size_bytes: size,
            sealed_at_ns: 1,
            reason: "operator".to_string(),
            digest: format!("sha256:{name}"),
            receipted,
            cited,
        }
    }

    /// The measurement §80's third condition waited on, taken from a real disk.
    ///
    /// Every other test in this module supplies `Free::Bytes` because the policy is decided in a
    /// test rather than on a machine whose disk happens to be full. This one is about the input:
    /// `free_of` reads a volume now, and the two answers it may give are a number or *unmeasured* —
    /// never a zero, which is the reading that would run §80's rule at full force against every
    /// window the robot holds.
    #[test]
    fn free_disk_is_measured_or_absent_and_never_zero() {
        let root = TempRoot::new("retention-free");
        let layout = root.layout();

        match retention::free_of(&layout) {
            Free::Bytes(bytes) => {
                assert!(
                    bytes > 0,
                    "a volume with a temporary directory on it has bytes free",
                );
                let headroom = retention::headroom_of(&layout).expect("measured either way");
                assert!(
                    headroom > 0.0 && headroom <= 1.0,
                    "§33.6's headroom is a fraction: {headroom}",
                );
            }
            // The platform this suite also runs on. `skynet-statfs` reads a structure that is the
            // kernel's on 64-bit Linux and nothing anywhere else, and refuses rather than guessing.
            Free::Unmeasured => {
                assert!(
                    retention::headroom_of(&layout).is_none(),
                    "a disk that cannot be measured has no headroom to report either",
                );
            }
        }
    }

    #[test]
    fn nothing_is_reclaimed_above_the_floor_however_collectable_it_is() {
        // The condition that makes a deletion *necessary* rather than tidy. Every window here is
        // receipted and uncited — the state that authorises a reclaim — and none is taken, because
        // there is no benefit to weigh against losing local evidence.
        let windows = [
            window("a.mcap", 1_000, true, false),
            window("b.mcap", 1_000, true, false),
        ];
        let plan = retention::plan(&windows, Free::Bytes(10_000), 5_000);
        assert!(plan.reclaim.is_empty());
        assert!(plan.reaches_floor);
        assert_eq!(plan.freed_bytes, 0);
        assert!(plan.kept.iter().all(|(_, why)| *why == Kept::NotNeeded));
    }

    #[test]
    fn an_unmeasured_disk_reclaims_nothing() {
        // The direction a rule whose failure mode is destroying evidence has to fail in. Nothing on
        // this robot measures free disk, so this is the state the daemon is actually in.
        let windows = [window("a.mcap", 1_000, true, false)];
        let plan = retention::plan(&windows, Free::Unmeasured, 5_000);
        assert!(plan.reclaim.is_empty());
        assert!(!plan.reaches_floor);
        assert_eq!(plan.kept, vec![("a.mcap".to_string(), Kept::Unmeasured)]);
    }

    #[test]
    fn a_receipt_alone_does_not_authorise_a_reclaim_and_neither_does_the_floor() {
        // Both halves of the conjunction, in one test, because either alone is the bug. Under the
        // floor with no receipt: kept. Receipted above the floor: kept. Only both reclaim.
        let uncollected = [window("a.mcap", 4_000, false, false)];
        let under = retention::plan(&uncollected, Free::Bytes(1_000), 5_000);
        assert!(under.reclaim.is_empty());
        assert_eq!(under.kept, vec![("a.mcap".to_string(), Kept::Uncollected)]);
        assert!(!under.reaches_floor, "it freed nothing and is still under");

        let collected = [window("a.mcap", 4_000, true, false)];
        let over = retention::plan(&collected, Free::Bytes(9_000), 5_000);
        assert!(over.reclaim.is_empty());

        let both = retention::plan(&collected, Free::Bytes(1_000), 5_000);
        assert_eq!(both.reclaim.len(), 1);
        assert!(both.reaches_floor);
    }

    #[test]
    fn a_cited_window_is_never_reclaimed_and_the_disk_pressure_does_not_matter() {
        // The absolute term. This window is receipted, it is the oldest, the disk is at zero, and
        // freeing it is the only way to reach the floor — and it stays, which is the whole of §80's
        // first sentence about incidents. The pass reports that it could not reach the floor rather
        // than reaching it.
        let windows = [window("a.mcap", 9_000, true, true)];
        let plan = retention::plan(&windows, Free::Bytes(0), 5_000);
        assert!(plan.reclaim.is_empty());
        assert_eq!(plan.kept, vec![("a.mcap".to_string(), Kept::Cited)]);
        assert!(
            !plan.reaches_floor,
            "a robot that freed what it could and is still under has to say so"
        );
    }

    #[test]
    fn oldest_first_and_only_as_far_as_the_floor() {
        // Two properties in one arrangement, because the second is what stops a pass from emptying
        // a disk it only needed to clear. `a` and `b` reach the floor; `c` is left, and it is left
        // as `NotNeeded` rather than as a failure — an operator who reclaimed to reach a floor
        // should not be told the survivors were collectable.
        let windows = [
            window("a.mcap", 2_000, true, false),
            window("b.mcap", 2_000, true, false),
            window("c.mcap", 2_000, true, false),
        ];
        let plan = retention::plan(&windows, Free::Bytes(1_000), 5_000);
        assert_eq!(
            plan.reclaim
                .iter()
                .map(|w| w.name.as_str())
                .collect::<Vec<_>>(),
            vec!["a.mcap", "b.mcap"]
        );
        assert_eq!(plan.freed_bytes, 4_000);
        assert!(plan.reaches_floor);
        assert_eq!(plan.kept, vec![("c.mcap".to_string(), Kept::NotNeeded)]);
    }

    #[test]
    fn a_cited_window_is_skipped_and_the_next_one_is_taken() {
        // The interaction that a per-condition suite misses entirely: the protected window is the
        // *oldest*, so an implementation that stopped at the first ineligible candidate rather than
        // skipping it would free nothing and report the disk as unrecoverable.
        let windows = [
            window("a.mcap", 4_000, true, true),
            window("b.mcap", 4_000, true, false),
        ];
        let plan = retention::plan(&windows, Free::Bytes(1_000), 5_000);
        assert_eq!(
            plan.reclaim
                .iter()
                .map(|w| w.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b.mcap"]
        );
        assert!(plan.kept.contains(&("a.mcap".to_string(), Kept::Cited)));
    }

    #[test]
    fn the_floor_boundary_is_reached_rather_than_crossed() {
        // Exactly at the floor is *at* it, and the pass is over: `free >= floor` is the test, so a
        // robot sitting on the number reclaims nothing. The corpus rounds this session found four
        // of these; this is the one in this module.
        let windows = [window("a.mcap", 1_000, true, false)];
        assert!(retention::plan(&windows, Free::Bytes(5_000), 5_000)
            .reclaim
            .is_empty());
        assert_eq!(
            retention::plan(&windows, Free::Bytes(4_999), 5_000)
                .reclaim
                .len(),
            1
        );
    }

    #[test]
    fn a_tombstone_is_written_before_the_bytes_are_freed_and_survives_the_window() {
        let root = TempRoot::new("retention-tombstone");
        let layout = root.layout();
        let dir = layout.blackbox().join("sealed");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("00000000000000001234-operator.mcap"),
            b"window bytes",
        )
        .unwrap();

        let scanned = retention::scan(&layout);
        assert_eq!(scanned.len(), 1);
        // The name is parsed, not guessed: §13's zero-padded instant and the reason are both in it.
        assert_eq!(scanned[0].sealed_at_ns, 1234);
        assert_eq!(scanned[0].reason, "operator");
        assert!(!scanned[0].receipted, "nothing wrote a receipt");

        // Receipt it, and reclaim under the floor.
        crate::upload::Receipt {
            digest: scanned[0].digest.clone(),
            bytes: scanned[0].size_bytes,
            at_ms: 5,
        }
        .write(&layout, &scanned[0].name)
        .unwrap();

        let (plan, failures) = retention::run(&layout, Free::Bytes(0), 1_000_000);
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(plan.reclaim.len(), 1);
        assert!(!dir.join("00000000000000001234-operator.mcap").exists());

        let stones = retention::tombstones(&layout);
        assert_eq!(stones.len(), 1);
        assert_eq!(stones[0].digest, scanned[0].digest);
        assert_eq!(stones[0].sealed_at_ns, 1234);
        assert_eq!(stones[0].bytes, "window bytes".len() as u64);
        assert_eq!(stones[0].reason, "operator");
        // The sidecars go with the bytes they describe.
        assert!(!layout.receipt(&scanned[0].name).exists());
    }

    #[test]
    fn a_citation_is_about_bytes_rather_than_about_a_name() {
        // §13 names a window for a monotonic instant and a reboot restarts that clock, so a
        // citation believed on its filename would protect whichever window took the name next —
        // and leave the one it was actually about looking routine.
        let root = TempRoot::new("retention-citation");
        let layout = root.layout();
        let dir = layout.blackbox().join("sealed");
        std::fs::create_dir_all(&dir).unwrap();

        retention::cite(&layout, "w.mcap", "sha256:aaaa", "guard_tripped").unwrap();
        assert!(retention::cited(&layout, "w.mcap", "sha256:aaaa"));
        assert!(
            !retention::cited(&layout, "w.mcap", "sha256:bbbb"),
            "a citation for other bytes is not this window's"
        );
        assert!(!retention::cited(&layout, "other.mcap", "sha256:aaaa"));
    }

    #[test]
    fn a_cited_window_on_the_disk_survives_a_pass_that_would_otherwise_take_it() {
        // The whole rule, end to end, against real files: the oldest window is receipted and cited,
        // the second is receipted and not, the disk is empty, and only the second goes.
        let root = TempRoot::new("retention-end-to-end");
        let layout = root.layout();
        let dir = layout.blackbox().join("sealed");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("00000000000000000001-fault.mcap"), b"the incident").unwrap();
        std::fs::write(dir.join("00000000000000000002-operator.mcap"), b"routine").unwrap();

        for window in retention::scan(&layout) {
            crate::upload::Receipt {
                digest: window.digest.clone(),
                bytes: window.size_bytes,
                at_ms: 1,
            }
            .write(&layout, &window.name)
            .unwrap();
            if window.reason == "fault" {
                retention::cite(&layout, &window.name, &window.digest, "guard_tripped").unwrap();
            }
        }

        let (plan, failures) = retention::run(&layout, Free::Bytes(0), 1_000_000);
        assert!(failures.is_empty(), "{failures:?}");
        assert_eq!(
            plan.reclaim
                .iter()
                .map(|w| w.name.as_str())
                .collect::<Vec<_>>(),
            vec!["00000000000000000002-operator.mcap"]
        );
        assert!(
            dir.join("00000000000000000001-fault.mcap").exists(),
            "the window an incident names is never reclaimed"
        );
        assert!(!dir.join("00000000000000000002-operator.mcap").exists());
        assert!(
            !plan.reaches_floor,
            "it could not reach a megabyte and says so rather than taking the incident's window"
        );
    }

    #[test]
    fn a_manifest_naming_another_windows_digest_does_not_get_a_window_deleted() {
        // The reason `scan` hashes rather than reading manifests. `blackbox::write_manifest`'s own
        // comment calls the sidecar "a cache in the sense that it can be rebuilt, and not a second
        // source of truth" — and §13's names repeat, so a manifest can outlive the bytes it
        // described. A pass that believed one would receipt-match the wrong digest and free a
        // window nothing has collected.
        let root = TempRoot::new("retention-stale-manifest");
        let layout = root.layout();
        let dir = layout.blackbox().join("sealed");
        std::fs::create_dir_all(&dir).unwrap();
        let name = "00000000000000000001-operator.mcap";
        std::fs::write(dir.join(name), b"these bytes").unwrap();

        // A manifest and a receipt that agree with each other and not with the file.
        std::fs::write(
            crate::blackbox::manifest_path(&dir, name),
            br#"{"digest":"sha256:deadbeef","size_bytes":11}"#,
        )
        .unwrap();
        crate::upload::Receipt {
            digest: "sha256:deadbeef".to_string(),
            bytes: 11,
            at_ms: 1,
        }
        .write(&layout, name)
        .unwrap();

        let scanned = retention::scan(&layout);
        assert_eq!(scanned.len(), 1);
        assert_ne!(scanned[0].digest, "sha256:deadbeef");
        assert!(
            !scanned[0].receipted,
            "the receipt is for other bytes, so this window has none"
        );

        let (plan, _) = retention::run(&layout, Free::Bytes(0), 1_000_000);
        assert!(plan.reclaim.is_empty());
        assert!(dir.join(name).exists());
    }

    #[test]
    fn the_scan_is_oldest_first_whatever_order_the_directory_reports() {
        // §80 reclaims oldest first, and the ordering comes from §13's zero-padded monotonic
        // instant rather than from `read_dir`, which has no order worth relying on. Written in
        // reverse so a scan that returned directory order would have to be lucky to agree.
        let root = TempRoot::new("retention-order");
        let layout = root.layout();
        let dir = layout.blackbox().join("sealed");
        std::fs::create_dir_all(&dir).unwrap();
        for n in [30u64, 10, 20] {
            std::fs::write(
                dir.join(format!("{n:020}-operator.mcap")),
                format!("window {n}").as_bytes(),
            )
            .unwrap();
        }
        let scanned = retention::scan(&layout);
        assert_eq!(
            scanned.iter().map(|w| w.sealed_at_ns).collect::<Vec<_>>(),
            vec![10, 20, 30]
        );
    }

    #[test]
    fn a_tombstone_that_cannot_be_written_stops_the_delete() {
        // The ordering guarantee, forced. A reclaim that freed bytes it could not account for is
        // exactly the state §80 exists to make impossible, so the write is `?` rather than best
        // effort — and the window survives to be reclaimed on a later pass, which is the direction
        // that loses nothing.
        let root = TempRoot::new("retention-unwritable");
        let layout = root.layout();
        let dir = layout.blackbox().join("sealed");
        std::fs::create_dir_all(&dir).unwrap();
        let name = "00000000000000000001-operator.mcap";
        std::fs::write(dir.join(name), b"bytes").unwrap();

        // A directory where the log wants a file: the append fails and nothing else does.
        std::fs::create_dir_all(retention::tombstones_path(&layout)).unwrap();

        let scanned = retention::scan(&layout);
        crate::upload::Receipt {
            digest: scanned[0].digest.clone(),
            bytes: scanned[0].size_bytes,
            at_ms: 1,
        }
        .write(&layout, name)
        .unwrap();

        let (plan, failures) = retention::run(&layout, Free::Bytes(0), 1_000_000);
        assert_eq!(plan.reclaim.len(), 1, "the plan named it");
        assert_eq!(failures.len(), 1, "and the reclaim refused");
        assert!(
            dir.join(name).exists(),
            "the bytes are still there, because nothing could account for freeing them"
        );
    }

    #[test]
    fn tombstones_accumulate_and_a_truncated_line_costs_that_line() {
        // §38.3's degradation argument, which this file inherits along with the JSON Lines shape.
        let root = TempRoot::new("retention-tombstone-log");
        let layout = root.layout();
        for n in 1..=3u64 {
            retention::record(
                &layout,
                &retention::Tombstone {
                    digest: format!("sha256:{n}"),
                    sealed_at_ns: n,
                    bytes: n * 10,
                    reason: "operator".to_string(),
                },
            )
            .unwrap();
        }
        assert_eq!(retention::tombstones(&layout).len(), 3);

        // A power cut mid-append.
        let path = retention::tombstones_path(&layout);
        let mut text = std::fs::read_to_string(&path).unwrap();
        text.push_str("{\"digest\":\"sha256:4\",\"byt");
        std::fs::write(&path, text).unwrap();
        let read = retention::tombstones(&layout);
        assert_eq!(read.len(), 3, "the three before it still parse");
        assert_eq!(read[2].digest, "sha256:3");
    }
}

// ── §38.2's session override, over the local API ─────────────────────────────

/// A fixture with an operator credential, which is the role these routes take.
fn bench(tag: &str) -> Fixture {
    let mut fixture = Fixture::new(tag);
    fixture.provision();
    fixture.credentials.add(Credential::issue(
        "bench",
        Role::Operator,
        "cc",
        "operator-token",
    ));
    fixture
}

#[test]
fn a_robot_with_no_override_says_none_and_publishes_the_cap() {
    let mut fixture = bench("session-none");
    let (status, body) = fixture.request("GET", "/session", Some("observer-token"), "");

    assert_eq!(status, 200);
    assert_eq!(body.get("state").and_then(Json::as_str), Some("none"));
    // The cap is published rather than left for a client to discover by being refused. §64's rule:
    // a refusal names its fix, and a form that knows the maximum can stop somebody typing a week.
    assert!(body.get("max_duration_ms").and_then(Json::as_f64).unwrap() > 0.0);
}

#[test]
fn setting_one_narrows_the_limits_the_robot_reports() {
    // The whole point, end to end: a request over the API changes the number `GET /limits` serves.
    // Neither route knows about the other — they meet in `resolve_limits`, which is where §7.6 says
    // a ceiling is chosen and the only place one is.
    let mut fixture = bench("session-narrows-limits");

    let (before, limits) = fixture.request("GET", "/limits", Some("observer-token"), "");
    assert_eq!(before, 200);
    let was = limits
        .get("joint_velocity_max_rad_s")
        .and_then(Json::as_f64);

    let (status, _) = fixture.request(
        "POST",
        "/session",
        Some("operator-token"),
        r#"{"name":"maintenance","duration_ms":600000,"joint_velocity_max_rad_s":0.25}"#,
    );
    assert_eq!(status, 200);

    let (_, after) = fixture.request("GET", "/limits", Some("observer-token"), "");
    assert_eq!(
        after.get("joint_velocity_max_rad_s").and_then(Json::as_f64),
        Some(0.25)
    );
    assert_ne!(was, Some(0.25), "the fixture would prove nothing otherwise");
}

#[test]
fn a_robot_under_an_override_says_so_on_its_limits() {
    // §38.2's hazard is a robot that is quietly slow. The notice is how it stops being quiet, and a
    // console renders notices without knowing this layer exists.
    let mut fixture = bench("session-notice");
    fixture.request(
        "POST",
        "/session",
        Some("operator-token"),
        r#"{"name":"wrist-harness","duration_ms":600000,"joint_velocity_max_rad_s":0.25}"#,
    );

    let (_, limits) = fixture.request("GET", "/limits", Some("observer-token"), "");
    let notices = limits
        .get("notices")
        .and_then(Json::as_arr)
        .expect("notices");
    let announced = notices
        .iter()
        .find(|notice| notice.get("code").and_then(Json::as_str) == Some("session_override"));
    let announced = announced.expect("an override announces itself on the limits");
    assert!(announced
        .get("message")
        .and_then(Json::as_str)
        .unwrap()
        .contains("wrist-harness"));
}

#[test]
fn an_override_without_a_duration_is_refused_and_told_why() {
    // There is deliberately no way to ask for one that does not end. §38.2 time-boxes this layer,
    // so a missing duration is a refusal rather than a default of "until somebody notices".
    let mut fixture = bench("session-no-duration");
    let (status, body) = fixture.request(
        "POST",
        "/session",
        Some("operator-token"),
        r#"{"name":"forever","joint_velocity_max_rad_s":0.25}"#,
    );

    assert_eq!(status, 400);
    assert!(body
        .get("detail")
        .and_then(Json::as_str)
        .unwrap()
        .contains("time-boxed"));
}

#[test]
fn an_override_past_the_cap_is_refused() {
    let mut fixture = bench("session-cap");
    let (status, body) = fixture.request(
        "POST",
        "/session",
        Some("operator-token"),
        r#"{"name":"a-week","duration_ms":604800000,"joint_velocity_max_rad_s":0.25}"#,
    );

    assert_eq!(status, 400);
    assert!(body
        .get("detail")
        .and_then(Json::as_str)
        .unwrap()
        .contains("signed profile"));
}

#[test]
fn an_observer_may_read_an_override_and_may_not_set_one() {
    let mut fixture = bench("session-roles");

    let (read, _) = fixture.request("GET", "/session", Some("observer-token"), "");
    assert_eq!(read, 200);

    let (write, _) = fixture.request(
        "POST",
        "/session",
        Some("observer-token"),
        r#"{"name":"nope","duration_ms":60000,"joint_velocity_max_rad_s":0.25}"#,
    );
    assert_eq!(write, 403, "an observer reads; it does not narrow");
}

#[test]
fn nothing_anonymous_may_set_one() {
    let mut fixture = bench("session-anonymous");
    let (status, _) = fixture.request(
        "POST",
        "/session",
        None,
        r#"{"name":"nope","duration_ms":60000,"joint_velocity_max_rad_s":0.25}"#,
    );
    assert_eq!(status, 401);
}

#[test]
fn clearing_restores_the_profiles_ceilings_and_is_idempotent() {
    let mut fixture = bench("session-clear");
    let (_, limits) = fixture.request("GET", "/limits", Some("observer-token"), "");
    let was = limits
        .get("joint_velocity_max_rad_s")
        .and_then(Json::as_f64);

    fixture.request(
        "POST",
        "/session",
        Some("operator-token"),
        r#"{"name":"maintenance","duration_ms":600000,"joint_velocity_max_rad_s":0.25}"#,
    );

    let (first, body) = fixture.request("DELETE", "/session", Some("operator-token"), "");
    assert_eq!(first, 200);
    assert_eq!(body.get("cleared").and_then(Json::as_bool), Some(true));

    let (_, after) = fixture.request("GET", "/limits", Some("observer-token"), "");
    assert_eq!(
        after.get("joint_velocity_max_rad_s").and_then(Json::as_f64),
        was,
        "the robot is back where its profile puts it"
    );

    // Clearing nothing is not an error — the intent is satisfied either way.
    let (second, body) = fixture.request("DELETE", "/session", Some("operator-token"), "");
    assert_eq!(second, 200);
    assert_eq!(body.get("cleared").and_then(Json::as_bool), Some(false));
}

#[test]
fn setting_and_clearing_both_leave_an_audit_entry_with_the_previous_value() {
    // §38.3: "Every change is a black box event and an audit-log entry, carrying who, when, which
    // layer, and the previous value." The previous value is the half that is easy to skip, and it
    // is the half that makes the log answer "what did somebody change" rather than "what is it now".
    let mut fixture = bench("session-audit");
    fixture.request(
        "POST",
        "/session",
        Some("operator-token"),
        r#"{"name":"first","duration_ms":600000,"joint_velocity_max_rad_s":0.5}"#,
    );
    fixture.request(
        "POST",
        "/session",
        Some("operator-token"),
        r#"{"name":"second","duration_ms":600000,"joint_velocity_max_rad_s":0.25}"#,
    );

    let log = crate::audit::Audit::of(fixture.daemon.store.layout()).read();
    let entries: Vec<_> = log
        .entries
        .iter()
        .filter(|entry| entry.action == "session_override")
        .collect();
    assert_eq!(entries.len(), 2);

    assert_eq!(
        entries[0].previous, None,
        "there was nothing before the first"
    );
    let second = entries[1];
    assert_eq!(second.subject, "second");
    assert!(second.actor.contains("operator"), "actor: {}", second.actor);
    assert!(
        second.previous.as_deref().unwrap().contains("0.5"),
        "the entry says what it replaced: {second:?}"
    );
}

#[test]
fn an_expired_override_reads_as_expired_and_stops_narrowing() {
    // Written straight to disk with a deadline in the past, because the route cannot be asked for
    // one — which is the property `an_override_that_expires_in_the_past_is_refused` pins, and the
    // reason this test has to reach past the route to test the reading.
    let mut fixture = bench("session-expired");
    let path = fixture.daemon.store.layout().session_override();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"name":"stale","set_by":"operator","set_at_ms":0,"expires_at_ms":1,"joint_velocity_max_rad_s":0.1}"#,
    )
    .unwrap();

    let (_, body) = fixture.request("GET", "/session", Some("observer-token"), "");
    assert_eq!(body.get("state").and_then(Json::as_str), Some("expired"));

    let (_, limits) = fixture.request("GET", "/limits", Some("observer-token"), "");
    assert_ne!(
        limits
            .get("joint_velocity_max_rad_s")
            .and_then(Json::as_f64),
        Some(0.1),
        "an expired override narrows nothing"
    );
    let notices = limits
        .get("notices")
        .and_then(Json::as_arr)
        .expect("notices");
    assert!(
        !notices
            .iter()
            .any(|notice| notice.get("code").and_then(Json::as_str) == Some("session_override")),
        "and it does not claim the robot is held back"
    );
}

// ── §38.1's unsigned local configuration, over the local API ─────────────────

/// A fixture with an operator credential, which is the role that changes a setting.
fn configured(tag: &str) -> Fixture {
    let mut fixture = Fixture::new(tag);
    fixture.provision();
    fixture.credentials.add(Credential::issue(
        "bench",
        Role::Operator,
        "cc",
        "operator-token",
    ));
    fixture
}

#[test]
fn a_robot_nobody_configured_reports_every_setting_with_its_default() {
    // Every one, not only the ones somebody set. A page listing only changes answers "what did I
    // change" and never "what is this robot doing", and the second is the bench's question.
    let mut fixture = configured("settings-defaults");
    let (status, body) = fixture.request("GET", "/settings", Some("observer-token"), "");

    assert_eq!(status, 200);
    let settings = body
        .get("settings")
        .and_then(Json::as_arr)
        .expect("settings");
    assert_eq!(settings.len(), 3);
    for entry in settings {
        assert_eq!(entry.get("is_default").and_then(Json::as_bool), Some(true));
        assert_eq!(
            entry.get("value").and_then(Json::as_str),
            entry.get("default").and_then(Json::as_str),
            "a robot nobody configured runs its defaults"
        );
        // §38.3's provenance: absent, because nobody set it. Not "unrecorded" — there is no
        // missing record here, there is no record because there was no decision.
        assert!(matches!(entry.get("set_by"), Some(Json::Null)));
        assert!(!entry
            .get("choices")
            .and_then(Json::as_arr)
            .expect("choices")
            .is_empty());
    }
    assert!(body
        .get("notes")
        .and_then(Json::as_arr)
        .expect("notes")
        .is_empty());
}

#[test]
fn setting_the_console_theme_changes_the_page_the_robot_serves() {
    // The test that would have caught a store nothing consults. `POST /settings` and the console
    // asset route know nothing about each other; they meet in the served bytes, which is the only
    // place the setting is worth anything.
    let mut fixture = configured("settings-theme-serves");

    let before = fixture.raw("GET", "/", None);
    assert!(
        !String::from_utf8_lossy(&before.body).contains("data-theme"),
        "a robot at the default pins nothing and lets the browser decide"
    );

    let (status, body) = fixture.request(
        "POST",
        "/settings",
        Some("operator-token"),
        r#"{"setting":"console_theme","value":"light"}"#,
    );
    assert_eq!(status, 200, "{body:?}");

    let after = fixture.raw("GET", "/", None);
    assert!(
        String::from_utf8_lossy(&after.body).contains("data-theme=\"light\""),
        "the page the robot serves has to carry the theme somebody set"
    );
}

#[test]
fn setting_the_log_level_changes_what_this_process_prints() {
    // The other half that has to meet: a value on disk and a process-global that decides whether a
    // line is printed. The route sets it directly rather than waiting for a reconcile pass,
    // because a level that took ten seconds to apply is one somebody sets twice.
    let _guard = crate::log::TEST_LOCK
        .lock()
        .unwrap_or_else(|held| held.into_inner());
    let mut fixture = configured("settings-log-level");
    let restore = crate::log::level();

    assert!(crate::log::enabled(crate::log::Level::Info));
    assert!(!crate::log::enabled(crate::log::Level::Debug));

    let (status, _) = fixture.request(
        "POST",
        "/settings",
        Some("operator-token"),
        r#"{"setting":"log_level","value":"debug"}"#,
    );
    assert_eq!(status, 200);
    assert!(
        crate::log::enabled(crate::log::Level::Debug),
        "the route has to take effect in this process, not on the next pass"
    );

    fixture.request(
        "POST",
        "/settings",
        Some("operator-token"),
        r#"{"setting":"log_level","value":"error"}"#,
    );
    assert!(!crate::log::enabled(crate::log::Level::Warn));

    crate::log::set_level(restore);
}

#[test]
fn a_value_the_kernel_enforces_is_refused_by_the_route_as_well_as_by_the_store() {
    // §38.1's line, reached over HTTP. The store is where it is enforced; this asserts that the
    // route does not have a path around it.
    let mut fixture = configured("settings-kernel-key");
    let (status, body) = fixture.request(
        "POST",
        "/settings",
        Some("operator-token"),
        r#"{"setting":"joint_velocity_max_rad_s","value":"99"}"#,
    );

    assert_eq!(status, 400);
    let detail = body
        .get("detail")
        .and_then(Json::as_str)
        .unwrap_or_default();
    assert!(detail.contains("Safety Kernel"), "detail: {detail}");
    assert!(
        detail.contains("§38.2"),
        "it should say where such a value lives: {detail}"
    );
}

#[test]
fn changing_a_setting_needs_an_operator_and_reading_one_does_not() {
    let mut fixture = configured("settings-roles");

    let (read, _) = fixture.request("GET", "/settings", Some("observer-token"), "");
    assert_eq!(read, 200, "an observer may see what this robot is set to");

    let (write, _) = fixture.request(
        "POST",
        "/settings",
        Some("observer-token"),
        r#"{"setting":"log_level","value":"debug"}"#,
    );
    assert_eq!(write, 403, "an observer may not change one");

    let (anonymous, _) = fixture.request("GET", "/settings", None, "");
    assert_eq!(
        anonymous, 401,
        "and §39.2 gives an unprovisioned caller nothing here"
    );
}

#[test]
fn resetting_puts_one_back_and_says_so_without_claiming_it_moved_something() {
    let mut fixture = configured("settings-reset");
    fixture.request(
        "POST",
        "/settings",
        Some("operator-token"),
        r#"{"setting":"console_theme","value":"dark"}"#,
    );

    let (status, body) = fixture.request(
        "POST",
        "/settings",
        Some("operator-token"),
        r#"{"setting":"console_theme","reset":true}"#,
    );
    assert_eq!(status, 200);
    assert!(
        body.get("detail")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .contains("was dark"),
        "{body:?}"
    );

    let (_, again) = fixture.request(
        "POST",
        "/settings",
        Some("operator-token"),
        r#"{"setting":"console_theme","reset":true}"#,
    );
    assert!(
        again
            .get("detail")
            .and_then(Json::as_str)
            .unwrap_or_default()
            .contains("already at its default"),
        "{again:?}"
    );
}

#[test]
fn every_change_leaves_the_audit_entry_and_the_previous_value_it_owes() {
    // §38.3: *"every change is a black box event and an audit-log entry, carrying who, when, which
    // layer, and the previous value."* On a robot nobody had configured, the previous value is the
    // default — said so, because "it was info" and "nobody had ever chosen" differ.
    let mut fixture = configured("settings-audit");
    fixture.request(
        "POST",
        "/settings",
        Some("operator-token"),
        r#"{"setting":"telemetry_sampling","value":6}"#,
    );
    fixture.request(
        "POST",
        "/settings",
        Some("operator-token"),
        r#"{"setting":"telemetry_sampling","value":12}"#,
    );

    let entries = crate::audit::Audit::of(fixture.daemon.store.layout())
        .read()
        .entries;
    let ours: Vec<_> = entries
        .iter()
        .filter(|entry| entry.action == "setting")
        .collect();
    assert_eq!(ours.len(), 2, "one entry per change");
    assert_eq!(ours[0].previous.as_deref(), Some("1 (the default)"));
    assert_eq!(ours[1].previous.as_deref(), Some("6"));
    assert!(ours[1].detail.contains("telemetry_sampling"));
    // §39.2's credentials name a role and never a person, and the record says a role.
    assert!(
        ours[0].actor.contains("operator"),
        "actor: {}",
        ours[0].actor
    );
}

#[test]
fn a_setting_written_by_hand_that_this_build_cannot_use_is_reported_on_the_route() {
    // Never swallowed. Somebody wrote that key expecting something to happen.
    let mut fixture = configured("settings-notes");
    std::fs::write(
        fixture.daemon.store.layout().settings(),
        r#"{"log_level":"chatty"}"#,
    )
    .expect("write");

    let (status, body) = fixture.request("GET", "/settings", Some("observer-token"), "");
    assert_eq!(status, 200, "a bad setting does not break the route");
    let notes = body.get("notes").and_then(Json::as_arr).expect("notes");
    assert_eq!(notes.len(), 1);
    assert!(notes[0]
        .as_str()
        .unwrap_or_default()
        .contains("error, warn, info, debug"));
}

// ── §45.4's event triggers, over the local API ───────────────────────────────

/// A robot with one camera-extrinsics record, measured `age_s` ago, that a collision invalidates.
///
/// The set is chosen from §45.2's own table: extrinsics drift with *"vibration, thermal cycling,
/// knocks"* and feed `proximity` and `workspace`, which is exactly §45.1's opening failure — a
/// camera four centimetres out means the proximity guard measures separation to the wrong place
/// while reporting perfectly healthy.
fn calibrated(tag: &str, age_s: f64) -> Fixture {
    let mut fixture = Fixture::new(tag);
    fixture.provision();
    fixture.credentials.add(Credential::issue(
        "bench",
        Role::Operator,
        "cc",
        "operator-token",
    ));

    let directory = fixture.daemon.store.layout().calibration();
    std::fs::create_dir_all(&directory).expect("calibration directory");
    let now_s = crate::api::wall_ms() as f64 / 1000.0;
    std::fs::write(
        directory.join("camera_extrinsics.json"),
        format!(
            "{{\"set\":\"camera_extrinsics\",\"method\":\"charuco-12x9, 40 poses\",\
              \"residual\":0.4,\"residual_threshold\":1,\"operator\":\"alice\",\
              \"conditions\":{{\"at_s\":{}}},\
              \"policy\":{{\"invalidating_events\":[\"collision\"]}}}}",
            now_s - age_s
        ),
    )
    .expect("record");
    fixture
}

fn validity_of(body: &Json, set: &str) -> Option<String> {
    body.get("sets")
        .and_then(Json::as_arr)?
        .iter()
        .find(|row| row.get("set").and_then(Json::as_str) == Some(set))
        .and_then(|row| row.get("state").and_then(Json::as_str))
        .map(str::to_string)
}

/// Why one set is in the state it is, from §45.4's closed vocabulary.
fn expiry_reason_of(body: &Json, set: &str) -> Option<String> {
    body.get("sets")
        .and_then(Json::as_arr)?
        .iter()
        .find(|row| row.get("set").and_then(Json::as_str) == Some(set))
        .and_then(|row| row.get_present("reason").and_then(Json::as_str))
        .map(str::to_string)
}

#[test]
fn recording_a_collision_expires_the_calibrations_it_could_have_disturbed() {
    // The whole point, end to end, and the thing that was unreachable: `events_since` came from the
    // stored record and from nowhere else, so a collision that happened this morning invalidated
    // nothing. Neither route knows about the other — they meet in `evaluate`, which is the only
    // place a calibration's validity is decided.
    let mut fixture = calibrated("calibration-event-expires", 3_600.0);

    let (before, body) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    assert_eq!(before, 200);
    assert_eq!(
        validity_of(&body, "camera_extrinsics").as_deref(),
        Some("valid"),
        "the fixture would prove nothing otherwise"
    );

    let (status, answered) = fixture.request(
        "POST",
        "/calibration/events",
        Some("operator-token"),
        r#"{"event":"collision","note":"clipped the fixture"}"#,
    );
    assert_eq!(status, 200, "{answered:?}");
    // The status comes straight back, so the answer is *which sets this retired* rather than an
    // acknowledgement the operator has to follow with a second request.
    assert_eq!(
        validity_of(&answered, "camera_extrinsics").as_deref(),
        Some("expired")
    );
    // The reason as well as the state, from §45.4's closed vocabulary. `expired` on its own would
    // pass for a record that had simply aged out, which is a different fact and a different fix.
    assert_eq!(
        expiry_reason_of(&answered, "camera_extrinsics").as_deref(),
        Some("event")
    );

    let (_, after) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    assert_eq!(
        validity_of(&after, "camera_extrinsics").as_deref(),
        Some("expired"),
        "and it stays expired on the next read, because the journal is on the disk"
    );
}

#[test]
fn a_calibration_measured_after_the_impact_is_not_invalidated_by_it() {
    // Without this rule re-measuring would be impossible: the set would expire again the instant it
    // was written, and there would be no way to clear an impact off a robot.
    let mut fixture = calibrated("calibration-event-after", 3_600.0);
    fixture.request(
        "POST",
        "/calibration/events",
        Some("operator-token"),
        r#"{"event":"collision"}"#,
    );

    // The same robot, re-measured now — which is what an engineer does after an impact.
    let directory = fixture.daemon.store.layout().calibration();
    let now_s = crate::api::wall_ms() as f64 / 1000.0;
    std::fs::write(
        directory.join("camera_extrinsics.json"),
        format!(
            "{{\"set\":\"camera_extrinsics\",\"method\":\"charuco-12x9, 40 poses\",\
              \"residual\":0.4,\"residual_threshold\":1,\"operator\":\"alice\",\
              \"conditions\":{{\"at_s\":{}}},\
              \"policy\":{{\"invalidating_events\":[\"collision\"]}}}}",
            now_s + 60.0
        ),
    )
    .expect("re-measured");

    let (_, after) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    assert_eq!(
        validity_of(&after, "camera_extrinsics").as_deref(),
        Some("valid"),
        "re-measuring has to be able to clear an impact"
    );
}

#[test]
fn an_event_a_records_policy_does_not_name_leaves_it_alone() {
    // §45.4 invalidates *"the calibrations the impact could plausibly have disturbed"*, and which
    // those are is the record's own policy to say — it was written by whoever knew the method. A
    // tool change does not move a camera.
    let mut fixture = calibrated("calibration-event-unrelated", 3_600.0);

    let (status, answered) = fixture.request(
        "POST",
        "/calibration/events",
        Some("operator-token"),
        r#"{"event":"tool_change"}"#,
    );
    assert_eq!(status, 200);
    assert_eq!(
        validity_of(&answered, "camera_extrinsics").as_deref(),
        Some("valid")
    );
}

#[test]
fn an_event_this_vocabulary_does_not_have_is_refused_by_the_route() {
    // A `collison` recorded with a typo matches no policy and invalidates nothing, leaving an
    // operator believing they have flagged an impact.
    let mut fixture = calibrated("calibration-event-typo", 60.0);
    let (status, body) = fixture.request(
        "POST",
        "/calibration/events",
        Some("operator-token"),
        r#"{"event":"collison"}"#,
    );

    assert_eq!(status, 400);
    let detail = body
        .get("detail")
        .and_then(Json::as_str)
        .unwrap_or_default();
    assert!(detail.contains("collision"), "detail: {detail}");
}

#[test]
fn recording_an_event_needs_an_operator_and_reading_the_report_does_not() {
    let mut fixture = calibrated("calibration-event-roles", 60.0);

    let (read, _) = fixture.request("GET", "/calibration", Some("observer-token"), "");
    assert_eq!(read, 200);

    let (write, _) = fixture.request(
        "POST",
        "/calibration/events",
        Some("observer-token"),
        r#"{"event":"collision"}"#,
    );
    assert_eq!(write, 403, "an observer may not retire a guard");
}

#[test]
fn a_recorded_event_leaves_the_audit_row_it_owes() {
    // Not a configuration change, but it changes which guards may run — which is the thing §38.3's
    // pair exists to leave a trail of.
    let mut fixture = calibrated("calibration-event-audit", 60.0);
    fixture.request(
        "POST",
        "/calibration/events",
        Some("operator-token"),
        r#"{"event":"collision","note":"clipped the fixture"}"#,
    );

    let entries = crate::audit::Audit::of(fixture.daemon.store.layout())
        .read()
        .entries;
    let ours = entries
        .iter()
        .find(|entry| entry.action == "calibration_event")
        .expect("an audit row");
    assert_eq!(ours.subject, "collision");
    assert!(ours.detail.contains("clipped the fixture"));
    assert!(ours.actor.contains("operator"), "actor: {}", ours.actor);
}

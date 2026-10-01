//! The bridge's own decisions, against a scripted drive and no kernel (DRY-COVERAGE-PLAN.md 12.C.3).
//!
//! `live_vcan` runs the port against the Python driver's simulated drives over a kernel bus, and the unit tests beside the
//! code script a bus frame by frame. What neither could do was let a test *choose what a drive says* and read what the master
//! wrote in reply: a drive with no encoder resolution, a TPDO2 that maps a third object, a fault latched when it is enabled, a
//! heartbeat and an emergency object in the middle of a SYNC. The seeded-defect campaign (`tools/seed-defects/`) found that
//! twelve of this crate's realistic defects on exactly those paths left `cargo test` green.
//!
//! The drive is `common::Model`, answering inside `Bus::send` (a synchronous bus), so nothing here waits on a clock and
//! nothing here is timing. **It agrees with whoever scripted it**: F15's point, and the reason `live_vcan` and the Python
//! drives exist.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use common::{with, Cia, Drive, DriveBus, Model};
use skynet_canopen_rs::bridge::{Bridge, Config, OpenError};
use skynet_canopen_rs::frames::{self, Frame};
use skynet_json::Json;
use skynet_rhal::flags;

fn config(nodes: &[u8]) -> Config {
    let joints: Vec<String> = nodes
        .iter()
        .map(|node| format!(r#"{{"node": {node}, "name": "J{node}"}}"#))
        .collect();
    Config::parse(&format!(
        r#"{{"joints": [{}], "simulator": false}}"#,
        joints.join(", ")
    ))
    .expect("a configuration")
}

fn opened(bus: DriveBus, nodes: &[u8]) -> Bridge<DriveBus> {
    Bridge::open(bus, &config(nodes))
        .unwrap_or_else(|error| panic!("the scripted drives are brought up: {}", error.message()))
}

/// The refusal a body that should not come up was given, and that it is a drive's refusal and not a bus failure.
fn refusal(result: Result<Bridge<DriveBus>, OpenError>) -> String {
    match result {
        Ok(_) => panic!("the body came up, and it should have been refused"),
        Err(error) => {
            assert!(
                matches!(error, OpenError::Refused(_)),
                "a drive's refusal is ENODEV, not a bus failure: {error:?}"
            );
            error.message().to_string()
        }
    }
}

static TICK: AtomicU64 = AtomicU64::new(1);

fn ticking_clock() -> u64 {
    TICK.fetch_add(1, Ordering::SeqCst)
}

fn peek_clock() -> u64 {
    TICK.load(Ordering::SeqCst)
}

#[test]
fn every_node_is_started_first_and_a_latched_fault_is_refused_before_anything_could_clear_it() {
    let bridge = opened(DriveBus::new(Model::with(&[1, 2])), &[1, 2]);
    assert_eq!(bridge.joint_count(), 2);
    drop(bridge);

    let mut model = Model::with(&[1, 2]);
    *model.drive(2) = Drive::new(2).faulted();
    let bus = DriveBus::new(model);
    let handle = bus.handle();
    let message = refusal(Bridge::open(bus, &config(&[1, 2])));
    assert!(
        message.contains("does not come up over a latched fault") && message.contains("0x603F"),
        "it is refused before the enable sequence, and says what to read: {message}"
    );
    with(&handle, |model| {
        assert!(
            model.drive(2).controlwords.is_empty() && model.drive(1).controlwords.is_empty(),
            "nothing was written to any controlword, so nothing was cleared and nothing was enabled: {:?}",
            model.drive(2).controlwords
        );
        assert_eq!(
            model.drive(2).state,
            Cia::Fault,
            "the fault is where the drive left it"
        );
        assert!(
            model
                .nmt()
                .starts_with(&[(frames::NMT_START, 1), (frames::NMT_START, 2)]),
            "each node is started, in joint order, before anything else: {:?}",
            model.nmt()
        );
    });
}

#[test]
fn a_drive_that_reports_no_resolution_or_no_gear_is_refused_for_it() {
    for (which, zeroed) in [
        ("the encoder's increments", 0),
        ("its revolutions", 1),
        ("the gear's motor side", 2),
        ("the gear's shaft side", 3),
    ] {
        let mut model = Model::with(&[1]);
        model.drive(1).units[zeroed] = 0;
        let message = refusal(Bridge::open(DriveBus::new(model), &config(&[1])));
        assert!(
            message.contains("encoder resolution of") && message.contains("not a ratio"),
            "{which} at zero is refused as a ratio: {message}"
        );
    }
}

#[test]
fn a_drive_under_another_pdo_mapping_is_refused_whatever_else_is_right_about_it() {
    // [TPDO2 count, entry 1, entry 2, RPDO2 count, entry 1, entry 2]
    for (what, slot, value) in [
        (
            "a TPDO2 whose second entry is some other object",
            2,
            (0x606C << 16) | 32,
        ),
        (
            "a TPDO2 whose first entry is some other object",
            1,
            (0x6077 << 16) | 16,
        ),
        (
            "an RPDO2 whose target is some other object",
            5,
            (0x60FF << 16) | 32,
        ),
        ("an RPDO2 of three objects", 3, 3),
        ("a TPDO2 of three objects", 0, 3),
        ("a TPDO2 of one object", 0, 1),
    ] {
        let mut model = Model::with(&[1]);
        model.drive(1).mapping[slot] = value;
        let message = refusal(Bridge::open(DriveBus::new(model), &config(&[1])));
        assert!(
            message.contains("maps") && message.contains("CiA 402 default"),
            "{what} is refused: {message}"
        );
    }
}

#[test]
fn a_drive_that_answers_another_mode_than_the_one_asked_for_is_refused() {
    let mut model = Model::with(&[1]);
    model.drive(1).mode_display = Some(1);
    let bus = DriveBus::new(model);
    let handle = bus.handle();
    let message = refusal(Bridge::open(bus, &config(&[1])));
    assert!(
        message.contains("reports mode 1") && message.contains("profile mode"),
        "{message}"
    );
    assert_eq!(
        with(&handle, |model| model.drive(1).mode),
        8,
        "cyclic synchronous position was asked for, by its number"
    );
}

#[test]
fn a_fault_latched_on_enable_ends_the_wait_at_once_and_a_drive_that_never_answers_is_named() {
    let mut model = Model::with(&[1]);
    model.drive(1).fault_on_enable = true;
    let started = Instant::now();
    let message = refusal(Bridge::open(DriveBus::new(model), &config(&[1])));
    let waited = started.elapsed();
    assert!(
        message.contains("`fault`") && message.contains("0x603F"),
        "{message}"
    );
    assert!(waited < Duration::from_millis(500), "a latched fault is not a state that arrives if waited for: it took {waited:?}, and the wait for a slow drive is a second");

    // No drive on the bus at all: the SDO is asked, and nobody answers.
    let error = Bridge::open(DriveBus::new(Model::default()), &config(&[1]))
        .err()
        .expect("a node that is not there");
    assert!(
        error.message().contains("did not answer a read of"),
        "{}",
        error.message()
    );
}

#[test]
fn a_body_is_simulated_when_the_bus_says_it_is_and_not_otherwise() {
    let described = |bridge: &Bridge<DriveBus>| {
        Json::parse(bridge.describe())
            .unwrap()
            .get("simulator")
            .and_then(Json::as_bool)
            .unwrap()
    };
    let declared = |simulator: &str| {
        Config::parse(&format!(
            r#"{{"joints": [{{"node": 1, "name": "J1"}}], "simulator": {simulator}}}"#
        ))
        .unwrap()
    };

    let bridge = Bridge::open(DriveBus::new(Model::with(&[1])), &declared("false"))
        .unwrap_or_else(|e| panic!("{}", e.message()));
    assert!(
        described(&bridge),
        "a bus that answers inside send is a scripted one, whatever the operator declared: a simulator and a drive answer alike, and the safe direction is the declaration that caps the tier (§35.3)"
    );
    let bridge = Bridge::open(DriveBus::real(Model::with(&[1])), &declared("false"))
        .unwrap_or_else(|e| panic!("{}", e.message()));
    assert!(
        !described(&bridge),
        "and one that does not, declared real, is described as real"
    );
    let bridge = Bridge::open(DriveBus::real(Model::with(&[1])), &declared("true"))
        .unwrap_or_else(|e| panic!("{}", e.message()));
    assert!(described(&bridge), "declared simulated, it is");
}

#[test]
fn a_fault_or_a_fault_reaction_is_a_fault_and_the_node_is_the_code() {
    for (state, is_fault) in [
        (Cia::Fault, true),
        (Cia::FaultReactionActive, true),
        (Cia::OperationEnabled, false),
        (Cia::QuickStopActive, false),
        (Cia::SwitchOnDisabled, false),
    ] {
        let bus = DriveBus::new(Model::with(&[4, 5]));
        let handle = bus.handle();
        let mut bridge = opened(bus, &[4, 5]);
        with(&handle, |model| model.drive(5).state = state);
        let mut positions = [f32::NAN; 2];
        let reading = bridge
            .read_state(false, &mut positions, peek_clock)
            .expect("a reading");
        let at = format!("a drive in {state:?}");
        assert_eq!(
            reading.flags & flags::FAULT != 0,
            is_fault,
            "{at}: the fault flag"
        );
        assert_eq!(
            reading.flags & flags::DEGRADED != 0,
            is_fault,
            "{at}: degraded with it, and only with it"
        );
        assert_eq!(
            reading.fault_code,
            if is_fault { 5 << 16 } else { 0 },
            "{at}: the node that latched it, in the upper half; the drive sent no emergency, so no code in the lower"
        );
    }
}

/// The fault-vocabulary contract (`tools/fault-vocab/contract.py`, DRY-COVERAGE-PLAN.md 12.C.6), held to the table `bridge.py` is held to:
/// `drivers/fault-vocabulary.json`, read here with the workspace's own JSON so the two drivers cannot be given different tables.
///
/// For every emergency code CiA 301 lists, every CiA 402 state the table covers and every emergency code in no table: the flags the class
/// requires and never one it forbids, the fault word (`node << 16 | code`), a command refused or taken as the class says, and the drive reading
/// well again when it is reset. **It agrees with whoever scripted the drive**, and so does the table: what it holds is what the driver does
/// with a code or a state a drive reports.
#[test]
fn every_emergency_code_and_state_the_vocabulary_names_is_reported_as_its_class() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fault-vocabulary.json"
    ))
    .expect("drivers/fault-vocabulary.json is where the contract's table lives");
    let spec = Json::parse(&text).expect("the vocabulary file parses");
    let canopen = spec
        .get("drivers")
        .and_then(|drivers| drivers.get("canopen"))
        .and_then(|entry| entry.get("vocabularies"))
        .expect("canopen has vocabularies");

    // (vocabulary, code, class, name)
    let mut cases: Vec<(String, u32, String, String)> = Vec::new();
    for vocabulary in ["emcy_error_code", "statusword_state"] {
        let entry = canopen
            .get(vocabulary)
            .unwrap_or_else(|| panic!("canopen has a `{vocabulary}` vocabulary"));
        for (key, code) in entry.get("codes").and_then(|c| c.as_obj()).expect("codes") {
            let class = code
                .get("class")
                .and_then(|c| c.as_str())
                .expect("a class")
                .to_string();
            let name = code
                .get("name")
                .and_then(|c| c.as_str())
                .expect("a name")
                .to_string();
            cases.push((
                vocabulary.to_string(),
                key.parse().expect("a numeric code"),
                class,
                name,
            ));
        }
        let escalation = entry
            .get("unmapped")
            .and_then(|c| c.as_str())
            .expect("an escalation")
            .to_string();
        for probe in entry
            .get("probes")
            .and_then(|p| p.as_arr())
            .expect("probes")
        {
            let code = probe.as_f64().expect("a numeric probe") as u32;
            cases.push((
                vocabulary.to_string(),
                code,
                escalation.clone(),
                format!("unlisted ({code})"),
            ));
        }
    }
    assert!(
        cases.len() >= 43,
        "the tables and their probes were read: {} cases",
        cases.len()
    );

    for (vocabulary, code, class, name) in cases {
        let bus = DriveBus::new(Model::with(&[1]));
        let handle = bus.handle();
        let mut bridge = opened(bus, &[1]);
        let mut positions = [f32::NAN; 1];
        let at = format!("{vocabulary} {code} {name} -> {class}");
        let emergency = |code: u32| {
            vec![Frame::new(
                frames::emcy_id(1),
                &[code as u8, (code >> 8) as u8, 0x01, 0, 0, 0, 0, 0],
            )
            .unwrap()]
        };
        if vocabulary == "emcy_error_code" {
            // The emergency arrives after a SYNC and is applied by the next read's drain, as it is on a bus.
            with(&handle, |model| model.after_sync = emergency(code));
            bridge
                .read_state(false, &mut positions, peek_clock)
                .expect("a reading");
            with(&handle, |model| model.after_sync.clear());
        } else {
            let state = match code {
                55 => Cia::OperationEnabled,
                15 => Cia::FaultReactionActive,
                8 => Cia::Fault,
                other => {
                    panic!("{at}: the table lists a statusword {other} this test has no state for")
                }
            };
            with(&handle, |model| model.drive(1).state = state);
        }
        let reading = bridge
            .read_state(false, &mut positions, peek_clock)
            .expect("a reading");
        let (required, forbidden, refused) = match class.as_str() {
            "ok" => (0, flags::ESTOP | flags::FAULT | flags::DEGRADED, false),
            "degraded" => (flags::DEGRADED, flags::ESTOP | flags::FAULT, false),
            "fault" => (flags::FAULT | flags::DEGRADED, flags::ESTOP, true),
            "estop" => (flags::ESTOP | flags::DEGRADED, 0, true),
            other => panic!("{at}: `{other}` is not one of the four classes"),
        };
        assert_eq!(
            reading.flags & required,
            required,
            "{at}: the class requires these flags, read {:#06x}",
            reading.flags
        );
        assert_eq!(
            reading.flags & forbidden,
            0,
            "{at}: and none of these, read {:#06x}",
            reading.flags
        );
        let wanted = if class == "ok" {
            0
        } else {
            1 << 16
                | if vocabulary == "emcy_error_code" {
                    code
                } else {
                    0
                }
        };
        assert_eq!(
            reading.fault_code, wanted,
            "{at}: the node in the upper half and the drive's own code, verbatim, in the lower"
        );
        let command = bridge.write_command(&[0.0], &[1]);
        if refused {
            assert_eq!(command, skynet_rhal::status::EPERM, "{at}: a command is refused, because `SKR_OK` would report success the robot did not have");
        } else {
            assert_eq!(command, skynet_rhal::status::OK, "{at}: a command is taken");
        }
        // The drive is reset, as a person at it does: a fault the driver holds after the drive has let go is F142's shape.
        if vocabulary == "emcy_error_code" {
            with(&handle, |model| model.after_sync = emergency(0));
            bridge
                .read_state(false, &mut positions, peek_clock)
                .expect("a reading");
            with(&handle, |model| model.after_sync.clear());
        } else {
            with(&handle, |model| {
                model.drive(1).state = Cia::OperationEnabled
            });
        }
        let healed = bridge
            .read_state(false, &mut positions, peek_clock)
            .expect("a reading");
        assert_eq!(
            healed.flags & (flags::ESTOP | flags::FAULT | flags::DEGRADED),
            0,
            "{at}: and well again, read {:#06x}",
            healed.flags
        );
        assert_eq!(healed.fault_code, 0, "{at}: and the fault word with it");
        assert_eq!(
            bridge.write_command(&[0.0], &[1]),
            skynet_rhal::status::OK,
            "{at}: and a command is taken again"
        );
    }
}

#[test]
fn a_sample_is_stamped_when_the_sync_leaves_and_heartbeats_and_emergencies_are_not_strangers() {
    let mut model = Model::with(&[1]);
    model.sync_clock = Some(peek_clock);
    let bus = DriveBus::new(model);
    let handle = bus.handle();
    let mut bridge = opened(bus, &[1]);
    let mut positions = [f32::NAN; 1];
    let reading = bridge
        .read_state(false, &mut positions, ticking_clock)
        .expect("a reading");
    let at_sync =
        with(&handle, |model| model.synced_at.last().copied()).expect("the drive saw a SYNC");
    assert_eq!(reading.stamp_ns + 1, at_sync, "the stamp was read before the SYNC went out, and the clock was not read again after the answers came in");

    // Expected traffic is not counted: a heartbeat, and an emergency object, from the node itself.
    with(&handle, |model| {
        model.after_sync = vec![
            Frame::new(frames::heartbeat_id(1), &[5]).unwrap(),
            Frame::new(frames::emcy_id(1), &[0x00, 0x73, 0x01, 0, 0, 0, 0, 0]).unwrap(),
        ]
    });
    for _ in 0..3 {
        bridge
            .read_state(false, &mut positions, peek_clock)
            .expect("a reading");
    }
    assert_eq!(bridge.unexpected_frames(), 0, "a heartbeat and an emergency object are what a drive says, and are not a frame this driver does not speak");
    with(&handle, |model| {
        model.after_sync = vec![
            Frame::new(0x123, &[1]).unwrap(),
            Frame::new(frames::tpdo2_id(1), &[0; 4]).unwrap(),
        ]
    });
    bridge
        .read_state(false, &mut positions, peek_clock)
        .expect("a reading");
    // The stranger and the short frame arrive after the SYNC's answer, and are read by the next read's drain.
    with(&handle, |model| model.after_sync.clear());
    bridge
        .read_state(false, &mut positions, peek_clock)
        .expect("a reading");
    assert_eq!(
        bridge.unexpected_frames(),
        2,
        "a stranger and a TPDO2 that is not six bytes are counted, each once"
    );
}

#[test]
fn an_sdo_frame_is_eight_bytes_and_no_more() {
    let good = [0x4B, 0x41, 0x60, 0x00, 0x37, 0x02, 0, 0];
    assert_eq!(
        frames::parse_sdo_response(&good, 0x6041, 0).unwrap(),
        Some(0x0237)
    );
    let mut long = good.to_vec();
    long.push(0);
    assert!(
        frames::parse_sdo_response(&long, 0x6041, 0).is_err(),
        "nine bytes are not an SDO frame, and the first eight are not to be read out of them"
    );
    assert!(
        frames::parse_sdo_response(&good[..7], 0x6041, 0).is_err(),
        "nor are seven"
    );
}

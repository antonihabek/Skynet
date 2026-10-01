//! The master and the bridge against a scripted ring, in process (DRY-COVERAGE-PLAN.md 12.C.3).
//!
//! `live_wire` shows the port brings the Python slave stack up, walks it to `operation enabled` and stops it. It cannot *choose*
//! what the ring says, so it cannot show what the master does when a slave refuses Safe-Op with a code, stays in Safe-Op without
//! saying so, fills its mailbox on the fourth poll or never, answers about another object, reports an encoder resolution of zero,
//! falls off the ring between two cycles, or answers with a working counter one too high. The seeded-defect campaign
//! (`tools/seed-defects/`) found that twenty-eight of this crate's realistic defects left `make check` green — a working counter
//! not asserted, an AL state trusted rather than read back, an all-zero mailbox read as a value, a stop that does not hold quick
//! stop, a latched fault the driver resets.
//!
//! The ring is `common::Ring`, which logs every frame it was sent, so a test reads what the master wrote and in what order. **It
//! agrees with whoever scripted it** — F15's point, and the reason `live_wire` and the Python slave stack exist — and every
//! register and controlword in it is a number from ETG.1000 or CiA 402 written out, not one of the crate's constants. It needs no
//! kernel and no namespace.

mod common;

use common::*;
use skynet_ethercat_rs::bridge::{Bridge, Config, OpenError};
use skynet_ethercat_rs::frames::Datagram;
use skynet_ethercat_rs::master::{al_status_name, coe_request, coe_value, Master};
use skynet_rhal::{flags, status};

const INCREMENTS_PER_TURN: f64 = 131072.0 * 50.0;

fn increments(radians: f64) -> i32 {
    (radians / std::f64::consts::TAU * INCREMENTS_PER_TURN).round() as i32
}

fn config(joints: usize) -> Config {
    Config::parse(&format!(
        r#"{{"joints": [{}]}}"#,
        vec!["{}"; joints].join(",")
    ))
    .expect("a configuration of empty joints")
}

fn opened(ring: Ring) -> (Bridge<RingSegment>, Handle) {
    let joints = ring.slaves.len();
    let segment = RingSegment::new(ring);
    let handle = segment.handle();
    match Bridge::open(segment, &config(joints)) {
        Ok(bridge) => (bridge, handle),
        Err(error) => panic!("the scripted ring did not come up: {}", error.message()),
    }
}

fn refused(ring: Ring) -> OpenError {
    let joints = ring.slaves.len();
    match Bridge::open(RingSegment::new(ring), &config(joints)) {
        Ok(_) => panic!("the bridge opened over a ring that should have been refused"),
        Err(error) => error,
    }
}

/// A master brought up over `ring`, with a handle on it.
fn up(ring: Ring) -> (Master<RingSegment>, Handle) {
    let count = ring.slaves.len();
    let segment = RingSegment::new(ring);
    let handle = segment.handle();
    let mut master = Master::new(segment);
    master
        .bring_up(count)
        .expect("the scripted ring comes up to Op");
    (master, handle)
}

/// One CiA 402 step for every drive: a command, then a cycle that carries it.
fn step(bridge: &mut Bridge<RingSegment>, targets: &[f32]) {
    let mask = vec![1u8; targets.len()];
    assert_eq!(bridge.write_command(targets, &mask), status::OK);
    let mut positions = vec![0f32; targets.len()];
    bridge.read_state(false, &mut positions).expect("a cycle");
}

/// The controlword and target each drive was sent in the ring's last LRW, by the layout the master configured.
fn last_outputs(handle: &Handle, drives: usize) -> Vec<(u16, i32)> {
    let (_, image) = with(handle, |ring| ring.lrws().last().cloned()).expect("an LRW was sent");
    (0..drives)
        .map(|k| {
            let at = 6 * k;
            (
                u16::from_le_bytes([image[at], image[at + 1]]),
                i32::from_le_bytes([image[at + 2], image[at + 3], image[at + 4], image[at + 5]]),
            )
        })
        .collect()
}

#[test]
fn a_datagram_is_answered_by_exactly_as_many_slaves_as_were_expected() {
    let mut master = Master::new(RingSegment::new(Ring::of(2)));
    let broadcast = || Datagram::new(BRD, 1, (0, 0x0000), vec![0]);
    assert!(
        master.exchange(broadcast(), 2).is_ok(),
        "two slaves answered two"
    );
    let few = master.exchange(broadcast(), 3).unwrap_err().0;
    assert!(
        few.contains("working counter of 2 and 3 was required"),
        "a counter below the expected is refused and says both numbers: {few}"
    );
    let many = master.exchange(broadcast(), 1).unwrap_err().0;
    assert!(
        many.contains("working counter of 2 and 1 was required"),
        "and so is one above it: a ring with an extra slave answering is not this body: {many}"
    );
}

#[test]
fn a_ring_of_another_size_is_not_configured() {
    for (present, expected) in [(4usize, 3usize), (2, 3)] {
        let segment = RingSegment::new(Ring::of(present));
        let handle = segment.handle();
        let mut master = Master::new(segment);
        let error = master.configure(expected).unwrap_err().0;
        assert!(
            error.contains(&format!("has {present} slave(s)"))
                && error.contains(&format!("describes {expected}")),
            "the refusal counts both: {error}"
        );
        with(&handle, |ring| {
            assert!(
                ring.datagrams().iter().all(|datagram| datagram.command != APWR),
                "no station address was written onto a machine that happened to be at a position ({present} on the ring, {expected} configured)"
            );
            assert!(ring.slaves.iter().all(|slave| slave.station == 0));
        });
    }
}

#[test]
fn bring_up_walks_init_preop_safeop_op_and_writes_what_the_protocol_says() {
    let (_master, handle) = up(Ring::of(3));
    with(&handle, |ring| {
        let states: Vec<u8> = ring
            .datagrams()
            .iter()
            .filter(|d| d.command == BWR && d.address == (0, 0x0120))
            .map(|d| d.data[0])
            .collect();
        assert_eq!(
            states,
            [INIT, PRE_OP, SAFE_OP, OP],
            "the protocol's order: Init, Pre-Op, Safe-Op, Op"
        );
        for (position, slave) in ring.slaves.iter().enumerate() {
            assert_eq!(slave.al, OP);
            assert_eq!(
                slave.station,
                0x1001 + position as u16,
                "a configured address, from 0x1001 because 0 means unconfigured to an ESC"
            );
            assert_eq!(
                slave.sm[0],
                [0x00, 0x10, 0x80, 0x00, 0x26, 0x00, 0x01, 0x00],
                "SM0: the output mailbox at 0x1000, 128 bytes, written by the master"
            );
            assert_eq!(
                slave.sm[1],
                [0x00, 0x14, 0x80, 0x00, 0x22, 0x00, 0x01, 0x00],
                "SM1: the input mailbox at 0x1400"
            );
            assert_eq!(
                slave.sm[2],
                [0x00, 0x18, 0x06, 0x00, 0x64, 0x00, 0x01, 0x00],
                "SM2: the outputs at 0x1800, six bytes"
            );
            assert_eq!(
                slave.sm[3],
                [0x80, 0x18, 0x06, 0x00, 0x20, 0x00, 0x01, 0x00],
                "SM3: the inputs at 0x1880, six bytes"
            );
            let (out, inp) = (6 * position as u8, 18 + 6 * position as u8);
            assert_eq!(slave.fmmu[0], [out, 0, 0, 0, 6, 0, 0, 7, 0x00, 0x18, 0, 2, 1, 0, 0, 0], "FMMU0 maps this slave's six output bytes from the logical image, type write, all sixteen bytes in ETG.1000.4's order");
            assert_eq!(
                slave.fmmu[1],
                [inp, 0, 0, 0, 6, 0, 0, 7, 0x80, 0x18, 0, 1, 1, 0, 0, 0],
                "FMMU1 maps the six input bytes, type read"
            );
        }
    });
}

#[test]
fn a_state_a_slave_did_not_reach_is_read_back_and_refused() {
    let mut ring = Ring::of(2);
    ring.slaves[1].stuck_at = Some(SAFE_OP);
    match refused(ring) {
        OpenError::Io(text) => assert!(
            text.contains("slave 1 was asked for Op and reports Safe-Op"),
            "a slave that stayed in Safe-Op is not driven as if it were operational: {text}"
        ),
        other => panic!("expected an I/O refusal, got {other:?}"),
    }
}

#[test]
fn a_refused_transition_names_the_code_the_slave_holds() {
    for (code, meaning) in [
        (0x001Eu16, "invalid input configuration"),
        (0x0016, "invalid mailbox configuration"),
        (0x4321, "a code ETG.1000.6 names and this master does not"),
    ] {
        let mut ring = Ring::of(2);
        ring.slaves[0].refuse = Some((SAFE_OP, code));
        match refused(ring) {
            OpenError::Io(text) => {
                assert!(
                    text.contains("slave 0 refused Safe-Op and is in Pre-Op"),
                    "the refusal says which slave, which state it was asked for and which it holds: {text}"
                );
                assert!(
                    text.contains(&format!("AL status code 0x{code:04X} ({meaning})")),
                    "and the code, in ETG.1000.6's words: {text}"
                );
            }
            other => panic!("expected an I/O refusal, got {other:?}"),
        }
    }
    for (code, meaning) in [
        (0x0000u16, "no error"),
        (0x0011, "invalid requested state change"),
        (0x0012, "unknown requested state"),
        (0x0013, "bootstrap not supported"),
        (0x0016, "invalid mailbox configuration"),
        (0x001D, "invalid output configuration"),
        (0x001E, "invalid input configuration"),
    ] {
        assert_eq!(al_status_name(code), meaning, "0x{code:04X}");
    }
}

#[test]
fn the_input_mailbox_is_polled_until_it_says_full_and_not_read_before() {
    let mut ring = Ring::of(1);
    ring.slaves[0].polls_before_full = 3;
    let (mut master, handle) = up(ring);
    assert_eq!(
        master.sdo_upload(0, 0x608F, 1, 4, false).expect("a reply"),
        131072,
        "the value the drive holds"
    );
    with(&handle, |ring| {
        assert_eq!(
            ring.slaves[0].status_polls, 4,
            "three polls found it empty and the fourth found it full; reading before that is a denied read taken for an answer"
        )
    });
}

#[test]
fn a_mailbox_that_never_fills_is_given_up_on_after_a_bounded_number_of_polls() {
    let mut ring = Ring::of(1);
    ring.slaves[0].reply = Reply::Never;
    let (mut master, handle) = up(ring);
    let error = master.sdo_upload(0, 0x608F, 1, 4, false).unwrap_err().0;
    assert!(
        error.contains("did not answer its mailbox within")
            && error.contains("polls of the input mailbox's status"),
        "{error}"
    );
    with(&handle, |ring| {
        // How long a silent drive is given is a tunable, and the suite does not pin it: a handful is too few to tell a slow
        // drive from a silent one, and a hundred thousand would hold a cycle open. Anywhere between is the driver's choice.
        let polls = ring.slaves[0].status_polls;
        assert!(
            (10..=10_000).contains(&polls),
            "a bounded, real wait: {polls} polls"
        );
    });
}

#[test]
fn a_reply_that_is_not_the_answer_is_refused_and_not_read_as_a_value() {
    let ask = |reply: Reply, index: u16, subindex: u8| {
        let mut ring = Ring::of(1);
        ring.slaves[0].reply = reply;
        let (mut master, _) = up(ring);
        master.sdo_upload(0, index, subindex, 4, false)
    };
    let zeros = ask(Reply::Zeros, 0x608F, 1).unwrap_err().0;
    assert!(
        zeros.contains("not a CoE message"),
        "an all-zero mailbox is the reply that never came, not a value of zero: {zeros}"
    );
    let other = ask(Reply::OtherObject, 0x608F, 1).unwrap_err().0;
    assert!(
        other.contains("asked for 0x608F:1 and the reply names 0x6090:1"),
        "an answer about another object is somebody else's message: {other}"
    );
    let aborted = ask(Reply::Abort(0x0609_0011), 0x608F, 1).unwrap_err().0;
    assert!(
        aborted.contains("aborted the read of 0x608F:1 with code 0x06090011"),
        "{aborted}"
    );
    let missing = ask(Reply::Normal, 0x1234, 0).unwrap_err().0;
    assert!(
        missing.contains("aborted the read of 0x1234:0 with code 0x06020000"),
        "an object the drive does not have is an abort, and not a zero: {missing}"
    );
    let (mut master, _) = up(Ring::of(1));
    assert_eq!(
        master.sdo_upload(0, 0x607D, 1, 4, true).unwrap(),
        -(1 << 22),
        "a signed object sign-extends at its width"
    );
    assert_eq!(master.sdo_upload(0, 0x607D, 2, 4, true).unwrap(), 1 << 22);
}

#[test]
fn a_reply_shorter_than_an_sdo_message_is_refused_before_it_is_indexed() {
    let mut reply = coe_request(0x43, 0x608F, 1, 131072u32.to_le_bytes());
    assert_eq!(reply.len(), 16, "an expedited SDO message");
    assert_eq!(coe_value(&reply, 0x608F, 1, 4, false).unwrap(), 131072);
    reply.truncate(15);
    let error = coe_value(&reply, 0x608F, 1, 4, false).unwrap_err().0;
    assert!(
        error.contains("an SDO reply is 16 bytes and 15 came back"),
        "a message that is CoE in every byte it has, and one short: {error}"
    );
}

#[test]
fn a_drive_that_reports_a_zero_in_a_ratio_or_will_not_report_one_is_refused() {
    for which in 0..4 {
        let mut ring = Ring::of(1);
        let mut units = [131072, 1, 50, 1];
        units[which] = 0;
        ring.slaves[0].units = Some(units);
        match refused(ring) {
            OpenError::Protocol(text) => assert!(
                text.contains("zero or negative figure"),
                "figure {which} of the ratios being zero: {text}"
            ),
            other => panic!("figure {which}: expected a protocol refusal, got {other:?}"),
        }
    }
    let mut ring = Ring::of(1);
    ring.slaves[0].units = None;
    match refused(ring) {
        OpenError::Protocol(text) => assert!(
            text.contains("would not report its encoder resolution or its gear ratio"),
            "{text}"
        ),
        other => panic!("expected a protocol refusal, got {other:?}"),
    }
}

#[test]
fn the_fault_code_names_the_first_faulted_drive_one_based() {
    let mut ring = Ring::of(3);
    ring.slaves[1] = Slave::faulted();
    ring.slaves[2] = Slave::faulted();
    let (mut bridge, _) = opened(ring);
    let mut positions = [0f32; 3];
    let reading = bridge.read_state(false, &mut positions).expect("a cycle");
    assert_ne!(
        reading.flags & flags::FAULT,
        0,
        "a faulted drive reaches the flag"
    );
    assert_eq!(
        reading.fault_code, 2,
        "the first faulted drive, at position 1, one-based — not the last"
    );
}

#[test]
fn the_drives_are_walked_to_operation_enabled_one_step_a_cycle_and_reach_the_target() {
    let (mut bridge, handle) = opened(Ring::of(2));
    let targets = [0.5f32, -0.25];
    for _ in 0..6 {
        step(&mut bridge, &targets);
    }
    with(&handle, |ring| {
        for slave in &ring.slaves {
            assert_eq!(
                slave.controlwords,
                [SHUTDOWN, SWITCH_ON, ENABLE_OPERATION, ENABLE_OPERATION, ENABLE_OPERATION, ENABLE_OPERATION],
                "shutdown, switch on, enable operation — from what the drive last reported, one step a cycle"
            );
            assert_eq!(slave.state, Cia::OperationEnabled);
        }
        for (slave, radians) in ring.slaves.iter().zip(targets) {
            assert!(
                (slave.target - increments(radians as f64)).abs() <= 1,
                "{} for {radians}",
                slave.target
            );
        }
    });
}

#[test]
fn after_a_stop_every_controlword_stays_at_quick_stop_and_keeps_its_target() {
    let (mut bridge, handle) = opened(Ring::of(2));
    let targets = [0.5f32, -0.25];
    for _ in 0..6 {
        step(&mut bridge, &targets);
    }
    let mut positions = [0f32; 2];
    let reading = bridge.read_state(true, &mut positions).expect("a cycle");
    assert_ne!(reading.flags & flags::ESTOP, 0, "the stop is visible");
    let sent = last_outputs(&handle, 2);
    for (k, (controlword, target)) in sent.iter().enumerate() {
        assert_eq!(
            *controlword, QUICK_STOP,
            "drive {k}: quick stop, not the enable the last command left in the image"
        );
        assert!(
            (target - increments(targets[k] as f64)).abs() <= 1,
            "drive {k} holds the target it had: {target}"
        );
    }
    with(&handle, |ring| {
        assert!(ring
            .slaves
            .iter()
            .all(|slave| slave.state == Cia::QuickStopActive));
    });
}

#[test]
fn close_quick_stops_every_drive_and_says_nothing_if_the_ring_has_gone() {
    let (mut bridge, handle) = opened(Ring::of(2));
    for _ in 0..6 {
        step(&mut bridge, &[0.5, -0.25]);
    }
    bridge.close();
    for (k, (controlword, _)) in last_outputs(&handle, 2).iter().enumerate() {
        assert_eq!(
            *controlword, QUICK_STOP,
            "drive {k} is left in quick stop, not enabled"
        );
    }
    with(&handle, |ring| {
        assert!(ring
            .slaves
            .iter()
            .all(|slave| slave.state == Cia::QuickStopActive))
    });

    let (bridge, handle) = opened(Ring::of(1));
    with(&handle, |ring| ring.deaf = true);
    bridge.close(); // a segment that has gone needs nothing more, and the close does not panic over it
}

#[test]
fn a_latched_fault_is_not_reset_by_the_driver() {
    let mut ring = Ring::of(2);
    ring.slaves[0] = Slave::faulted();
    let (mut bridge, handle) = opened(ring);
    // One cycle first, so the bridge has heard what state each drive is in: before that it assumes
    // `switch on disabled` and starts every drive's walk from the beginning.
    bridge.read_state(false, &mut [0f32; 2]).expect("a cycle");
    let neighbour = with(&handle, |ring| ring.slaves[1].state);
    for _ in 0..6 {
        // Refused whole while a drive holds a fault (12.C.6): a body that has lost one of its joints is not walked on towards a target with
        // the others, and `SKR_OK` for a command the faulted drive drops would be success the robot did not have. This test once asserted
        // the neighbour *was* walked on; what it holds is unchanged, nothing is sent that resets the fault.
        assert_eq!(bridge.write_command(&[0.5, -0.25], &[1, 1]), status::EPERM);
        bridge.read_state(false, &mut [0f32; 2]).expect("a cycle");
    }
    with(&handle, |ring| {
        assert!(
            ring.slaves[0].controlwords.iter().all(|word| *word == DISABLE_VOLTAGE),
            "a faulted drive is sent nothing that resets it — the operator reads the reason first: {:?}",
            ring.slaves[0].controlwords
        );
        assert!(ring.slaves[0]
            .controlwords
            .iter()
            .all(|word| word & FAULT_RESET == 0));
        assert_eq!(ring.slaves[0].state, Cia::Fault, "still latched");
        assert_eq!(
            ring.slaves[1].state, neighbour,
            "and its neighbour is left where it was, not walked on"
        );
    });
    let mut positions = [0f32; 2];
    let reading = bridge.read_state(false, &mut positions).unwrap();
    assert_ne!(reading.flags & flags::FAULT, 0);
    assert_eq!(reading.fault_code, 1);
}

/// The fault-vocabulary contract (`tools/fault-vocab/contract.py`, DRY-COVERAGE-PLAN.md 12.C.6), held to the table `bridge.py` is held to:
/// `drivers/fault-vocabulary.json`, read here with the workspace's own JSON so the two drivers cannot be given different tables.
///
/// For every CiA 402 state the table covers: the flags the class requires and never one it forbids, the fault word (the drive's position on the
/// ring plus one — the cyclic image carries the statusword and not the drive's own code), a command refused or taken as the class says, and
/// the drive reading well again when it is reset. **It agrees with whoever scripted the ring**, and so does the table.
#[test]
fn every_state_the_vocabulary_names_is_reported_as_its_class() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../fault-vocabulary.json"
    ))
    .expect("drivers/fault-vocabulary.json is where the contract's table lives");
    let spec = skynet_json::Json::parse(&text).expect("the vocabulary file parses");
    let entry = spec
        .get("drivers")
        .and_then(|drivers| drivers.get("ethercat"))
        .and_then(|entry| entry.get("vocabularies"))
        .and_then(|vocabularies| vocabularies.get("statusword_state"))
        .expect("ethercat has a `statusword_state` vocabulary");
    let codes = entry.get("codes").and_then(|c| c.as_obj()).expect("codes");
    assert!(
        codes.len() >= 3,
        "the table was read: {} codes",
        codes.len()
    );

    for (key, code) in codes {
        let class = code.get("class").and_then(|c| c.as_str()).expect("a class");
        let name = code.get("name").and_then(|c| c.as_str()).expect("a name");
        let state = match key.as_str() {
            "55" => Cia::OperationEnabled,
            "15" => Cia::FaultReactionActive,
            "8" => Cia::Fault,
            other => panic!("the table lists a statusword {other} this test has no state for"),
        };
        let (mut bridge, handle) = opened(Ring::of(1));
        with(&handle, |ring| ring.slaves[0].state = state);
        let mut positions = [0f32; 1];
        let at = format!("statusword_state {key} {name} -> {class}");
        // One cycle to hear the state, and the drive walked to `operation enabled` first when it is meant to be healthy.
        let mut reading = bridge.read_state(false, &mut positions).expect("a cycle");
        if class == "ok" {
            for _ in 0..8 {
                assert_eq!(bridge.write_command(&[0.0], &[1]), status::OK, "{at}");
                reading = bridge.read_state(false, &mut positions).expect("a cycle");
            }
        }
        let (required, forbidden, refused) = match class {
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
        assert_eq!(
            reading.fault_code,
            if class == "ok" { 0 } else { 1 },
            "{at}: the drive's position plus one, and zero when healthy"
        );
        let command = bridge.write_command(&[0.0], &[1]);
        if refused {
            assert_eq!(command, status::EPERM, "{at}: a command is refused, because `SKR_OK` would report success the robot did not have");
        } else {
            assert_eq!(command, status::OK, "{at}: a command is taken");
        }
        // The drive is reset, as a person at it does: a fault the driver holds after the drive has let go is F142's shape.
        with(&handle, |ring| ring.slaves[0].state = Cia::OperationEnabled);
        let healed = bridge.read_state(false, &mut positions).expect("a cycle");
        assert_eq!(
            healed.flags & (flags::ESTOP | flags::FAULT | flags::DEGRADED),
            0,
            "{at}: and well again, read {:#06x}",
            healed.flags
        );
        assert_eq!(healed.fault_code, 0, "{at}: and the fault word with it");
        assert_eq!(
            bridge.write_command(&[0.0], &[1]),
            status::OK,
            "{at}: and a command is taken again"
        );
    }
}

#[test]
fn a_ring_that_drops_a_slave_or_answers_out_of_count_fails_the_read() {
    let (mut bridge, handle) = opened(Ring::of(2));
    let mut positions = [0f32; 2];
    assert!(bridge.read_state(false, &mut positions).is_ok());

    with(&handle, |ring| ring.wkc_delta = vec![(LRW, 1)]);
    let high = bridge.read_state(false, &mut positions).unwrap_err().0;
    assert!(
        high.contains("working counter of 7 and 6 was required"),
        "one too high is not read as healthy: {high}"
    );

    with(&handle, |ring| {
        ring.wkc_delta.clear();
        ring.slaves[1].offline = true;
    });
    let low = bridge.read_state(false, &mut positions).unwrap_err().0;
    assert!(
        low.contains("working counter of 3 and 6 was required"),
        "a slave off the ring changes nothing but this number: {low}"
    );

    with(&handle, |ring| ring.deaf = true);
    let silent = bridge.read_state(false, &mut positions).unwrap_err().0;
    assert!(silent.contains("no answer from the ring"), "{silent}");
}

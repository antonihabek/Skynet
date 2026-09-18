//! The port's framing against the Python driver's own outputs.
//!
//! `tests/vectors.json` is eleven wire messages generated from `skynet_franka.protocol` — the
//! executable specification's actual bytes. Every pack function here must reproduce them exactly,
//! and every unpack function must read them back. A framing divergence fails here as bytes that
//! differ, not as a robot that misbehaves.

use skynet_franka_rs::protocol as fci;
use skynet_json::Json;

fn vectors() -> Json {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors.json");
    let text = std::fs::read_to_string(path).expect("vectors.json reads");
    Json::parse(&text).expect("vectors.json parses")
}

fn bytes(vectors: &Json, name: &str) -> Vec<u8> {
    let hex = vectors
        .get(name)
        .and_then(Json::as_str)
        .unwrap_or_else(|| panic!("vectors.json has {name}"));
    assert!(hex.len() % 2 == 0, "{name} is whole bytes");
    hex.as_bytes()
        .chunks(2)
        .map(|pair| {
            let word = std::str::from_utf8(pair).expect("hex is ASCII");
            u8::from_str_radix(word, 16).expect("hex parses")
        })
        .collect()
}

#[test]
fn connect_request_is_four_bytes() {
    let vectors = vectors();
    let expected = bytes(&vectors, "connect_request");
    assert_eq!(expected.len(), fci::CONNECT_REQUEST_SIZE);
    assert_eq!(
        fci::pack_connect_request(fci::VERSION, 51234).as_slice(),
        expected.as_slice(),
        "Connect::Request packs as the Python driver packs it"
    );
}

#[test]
fn connect_message_is_header_plus_request() {
    let vectors = vectors();
    let expected = bytes(&vectors, "connect_message");
    let request = fci::pack_connect_request(fci::VERSION, 51234);
    let packed = fci::pack_command(fci::COMMAND_CONNECT, 7, &request);
    assert_eq!(packed, expected);
    let header = fci::Header::unpack(&packed).expect("the header reads");
    assert_eq!(header.command, fci::COMMAND_CONNECT);
    assert_eq!(header.command_id, 7);
    assert_eq!(header.size as usize, packed.len());
}

#[test]
fn connect_response_reads_back() {
    let vectors = vectors();
    let raw = bytes(&vectors, "connect_response_ok");
    let header = fci::Header::unpack(&raw).expect("the header reads");
    assert_eq!(header.command, fci::COMMAND_CONNECT);
    assert_eq!(header.command_id, 7);
    let payload = &raw[fci::HEADER_SIZE..header.size as usize];
    let (status, theirs) = fci::unpack_connect_response(payload);
    assert_eq!(status, fci::CONNECT_SUCCESS);
    assert_eq!(theirs, fci::VERSION);
    // And the pack direction agrees with the same bytes.
    assert_eq!(
        fci::pack_response(
            fci::COMMAND_CONNECT,
            7,
            fci::CONNECT_SUCCESS,
            &fci::VERSION.to_le_bytes()
        ),
        raw
    );
}

#[test]
fn move_requests_pack() {
    let vectors = vectors();
    assert_eq!(
        fci::pack_move_request(
            9,
            fci::MOVE_CONTROLLER_JOINT_IMPEDANCE,
            fci::MOVE_MOTION_JOINT_POSITION
        ),
        bytes(&vectors, "move_request"),
        "a joint-position Move packs as the Python driver's"
    );
    assert_eq!(
        fci::pack_move_request(10, fci::MOVE_CONTROLLER_EXTERNAL, fci::MOVE_MOTION_NONE),
        bytes(&vectors, "move_request_torque"),
        "an external-torque Move packs as the Python driver's"
    );
}

#[test]
fn stop_move_is_twelve_bare_bytes() {
    let vectors = vectors();
    let packed = fci::pack_command(fci::COMMAND_STOP_MOVE, 11, &[]);
    assert_eq!(packed.len(), fci::HEADER_SIZE);
    assert_eq!(packed, bytes(&vectors, "stop_move"));
}

#[test]
fn robot_commands_pack() {
    let vectors = vectors();
    let position = fci::pack_robot_command(
        123456789,
        Some(&[0.1, -0.2, 0.3, -1.1, 0.0, 1.5, 0.7]),
        None,
        None,
        false,
    );
    assert_eq!(
        position.as_slice(),
        bytes(&vectors, "robot_command_position").as_slice()
    );
    let velocity = fci::pack_robot_command(
        42,
        None,
        Some(&[0.5, 0.0, -0.5, 0.25, 0.0, 0.0, 1.0]),
        None,
        true,
    );
    assert_eq!(
        velocity.as_slice(),
        bytes(&vectors, "robot_command_velocity_finished").as_slice()
    );
    let torque = fci::pack_robot_command(
        7,
        None,
        None,
        Some(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]),
        false,
    );
    assert_eq!(
        torque.as_slice(),
        bytes(&vectors, "robot_command_torque").as_slice()
    );

    // The fields read back, which is what the fake asserts against.
    let fields = fci::unpack_robot_command(&position).expect("the command reads");
    assert_eq!(fields.message_id, 123456789);
    assert_eq!(fields.q_c[0], 0.1);
    assert_eq!(fields.dq_c, [0.0; fci::JOINTS]);
    assert!(!fields.motion_finished);
    let finished = fci::unpack_robot_command(&velocity).expect("the command reads");
    assert!(finished.motion_finished);
    assert_eq!(finished.dq_c[2], -0.5);
}

#[test]
fn robot_state_reads() {
    let vectors = vectors();
    let raw = bytes(&vectors, "robot_state");
    let state = fci::RobotState::unpack(&raw).expect("the state reads");
    assert_eq!(state.message_id, 987654321);
    assert_eq!(
        state.q,
        [0.0, -0.785, 0.0, -2.356, 0.0, 1.571, 0.785].map(|v: f64| v as f32)
    );
    assert_eq!(
        state.dq,
        [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7].map(|v: f64| v as f32)
    );
    assert_eq!(
        state.tau_j,
        [2.0, -1.0, 0.5, 0.0, 0.25, -0.25, 1.5].map(|v: f64| v as f32)
    );
    assert_eq!(state.robot_mode, fci::ROBOT_MODE_MOVE);
    assert_eq!(state.motion_generator_mode, fci::MOTION_JOINT_POSITION);
    assert_eq!(state.controller_mode, fci::CONTROLLER_JOINT_IMPEDANCE);
    assert!(!state.faulted());
    assert_eq!(state.fault_code(), 0);
    assert_eq!(state.success_rate, 0.997f64 as f32);
    assert_eq!(
        state.raw, raw,
        "the unparsed remainder is kept, not discarded"
    );
}

#[test]
fn robot_state_fault_reads() {
    let vectors = vectors();
    let raw = bytes(&vectors, "robot_state_faulted");
    let state = fci::RobotState::unpack(&raw).expect("the state reads");
    assert!(state.faulted());
    assert_eq!(state.fault_code(), 3, "the lowest set bit, one-based");
    assert_eq!(state.robot_mode, fci::ROBOT_MODE_REFLEX);
}

#[test]
fn short_messages_are_refused_not_read() {
    let vectors = vectors();
    let state = bytes(&vectors, "robot_state");
    assert!(fci::Header::unpack(&state[..fci::HEADER_SIZE - 1]).is_err());
    assert!(fci::Header::unpack(&[0u8; fci::HEADER_SIZE]).is_err());
    assert!(fci::RobotState::unpack(&state[..fci::ROBOT_STATE_SIZE - 1]).is_err());
    assert!(fci::RobotState::unpack(&[]).is_err());
    let command = bytes(&vectors, "robot_command_position");
    assert!(fci::unpack_robot_command(&command[..fci::ROBOT_COMMAND_SIZE - 1]).is_err());
}

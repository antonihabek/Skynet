//! The port's constants against the vendor's own compiler.
//!
//! `drivers/franka/layout.json` holds what Franka's `research_interface` headers compile to
//! (`offsetof`/`sizeof`), and the Python driver's suite asserts `protocol.py` against it on every
//! run. This is the same assertion for the Rust port — so the port is held to Franka's compiler
//! directly rather than through the Python transcription, and a field that moves in a future
//! release fails a check here rather than arriving as a joint angle that is somebody else's field
//! read as a double.

use skynet_franka_rs::protocol as fci;
use skynet_json::Json;

fn layout() -> Json {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../franka/layout.json");
    let text = std::fs::read_to_string(path).expect("layout.json reads");
    Json::parse(&text).expect("layout.json parses")
}

fn number(value: &Json, what: &str) -> usize {
    value
        .as_usize()
        .unwrap_or_else(|| panic!("{what} is a number"))
}

#[test]
fn constants_are_the_vendors() {
    let pinned = layout();
    let constants = pinned.get("constants").expect("layout has constants");
    assert_eq!(
        number(constants.get("version").expect("version"), "version"),
        fci::VERSION as usize,
        "the FCI version is the vendor's"
    );
    assert_eq!(
        number(constants.get("command_port").expect("port"), "command_port"),
        fci::COMMAND_PORT as usize,
        "the command port is the vendor's"
    );
}

#[test]
fn struct_sizes_are_the_vendors() {
    let pinned = layout();
    let structs = pinned.get("structs").expect("layout has structs");
    for (name, ours) in [
        ("CommandHeader", fci::HEADER_SIZE),
        ("ConnectRequest", fci::CONNECT_REQUEST_SIZE),
        ("ConnectResponse", fci::CONNECT_RESPONSE_SIZE),
        ("MoveRequest", fci::MOVE_REQUEST_SIZE),
        ("RobotState", fci::ROBOT_STATE_SIZE),
        ("RobotCommand", fci::ROBOT_COMMAND_SIZE),
    ] {
        let entry = structs
            .get(name)
            .unwrap_or_else(|| panic!("layout has {name}"));
        let size = number(entry.get("size").expect("size"), name);
        assert_eq!(size, ours, "{name} is {size} bytes");
    }
}

#[test]
fn state_offsets_are_the_vendors() {
    let pinned = layout();
    let structs = pinned.get("structs").expect("layout has structs");
    let fields = structs
        .get("RobotState")
        .expect("RobotState")
        .get("fields")
        .expect("RobotState fields");
    for (name, ours) in [
        ("message_id", fci::STATE_MESSAGE_ID),
        ("tau_J", fci::STATE_TAU_J),
        ("q", fci::STATE_Q),
        ("dq", fci::STATE_DQ),
        ("motion_generator_mode", fci::STATE_MOTION_GENERATOR_MODE),
        ("controller_mode", fci::STATE_CONTROLLER_MODE),
        ("errors", fci::STATE_ERRORS),
        ("reflex_reason", fci::STATE_REFLEX_REASON),
        ("robot_mode", fci::STATE_ROBOT_MODE),
        ("control_command_success_rate", fci::STATE_SUCCESS_RATE),
    ] {
        let at = fields
            .get(name)
            .unwrap_or_else(|| panic!("RobotState has {name}"));
        assert_eq!(number(at, name), ours, "RobotState.{name} is at {ours}");
    }
}

#[test]
fn command_offsets_are_the_vendors() {
    let pinned = layout();
    let structs = pinned.get("structs").expect("layout has structs");
    let fields = structs
        .get("RobotCommand")
        .expect("RobotCommand")
        .get("fields")
        .expect("RobotCommand fields");
    for (name, ours) in [
        ("message_id", fci::COMMAND_MESSAGE_ID),
        ("motion", fci::COMMAND_MOTION),
        ("control", fci::COMMAND_CONTROL),
    ] {
        let at = fields
            .get(name)
            .unwrap_or_else(|| panic!("RobotCommand has {name}"));
        assert_eq!(number(at, name), ours, "RobotCommand.{name} is at {ours}");
    }
    let motion = structs
        .get("MotionGeneratorCommand")
        .expect("MotionGeneratorCommand")
        .get("fields")
        .expect("motion fields");
    for (name, ours) in [
        ("q_c", fci::MOTION_Q_C),
        ("dq_c", fci::MOTION_DQ_C),
        ("motion_generation_finished", fci::MOTION_FINISHED),
    ] {
        let at = motion
            .get(name)
            .unwrap_or_else(|| panic!("motion has {name}"));
        assert_eq!(
            number(at, name),
            ours,
            "MotionGeneratorCommand.{name} is at {ours}"
        );
    }
    let control = structs
        .get("ControllerCommand")
        .expect("ControllerCommand")
        .get("fields")
        .expect("control fields");
    for (name, ours) in [
        ("tau_J_d", fci::CONTROL_TAU_J_D),
        ("torque_command_finished", fci::CONTROL_FINISHED),
    ] {
        let at = control
            .get(name)
            .unwrap_or_else(|| panic!("control has {name}"));
        assert_eq!(
            number(at, name),
            ours,
            "ControllerCommand.{name} is at {ours}"
        );
    }
}

#[test]
fn the_float_double_asymmetry_is_asserted_not_commented() {
    let pinned = layout();
    let structs = pinned.get("structs").expect("layout has structs");
    let motion = structs
        .get("MotionGeneratorCommand")
        .expect("MotionGeneratorCommand")
        .get("fields")
        .expect("motion fields");
    let dq_c = number(
        motion.get("dq_c").expect("dq_c"),
        "MotionGeneratorCommand.dq_c",
    );
    let q_c = number(
        motion.get("q_c").expect("q_c"),
        "MotionGeneratorCommand.q_c",
    );
    assert_eq!(
        dq_c - q_c,
        7 * 8,
        "the command's joint arrays are doubles (56 bytes for seven)"
    );
    let state = structs
        .get("RobotState")
        .expect("RobotState")
        .get("fields")
        .expect("state fields");
    let dq = number(state.get("dq").expect("dq"), "RobotState.dq");
    let q_d = number(state.get("q_d").expect("q_d"), "RobotState.q_d");
    assert_eq!(
        dq - q_d,
        7 * 4,
        "the state's joint arrays are floats (28 bytes for seven) — the same seven joints"
    );
}

//! The port's wire, program and descriptor against the Python driver's own.
//!
//! `tests/vectors.json` comes from `tests/gen_vectors.py`, which drove the Python `Rtde` session
//! through a scripted socket: each exchange holds the bytes Python *wrote* and the replies it
//! *read*. Here the Rust session reads the same replies and must write the same bytes and reach the
//! same result — or refuse where Python refused. Nothing in the vectors was typed in.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::rc::Rc;

use skynet_json::Json;
use skynet_ur_rtde_rs::bridge::{self, INPUTS, OUTPUTS};
use skynet_ur_rtde_rs::control;
use skynet_ur_rtde_rs::rtde::{self, RtdeError, Session, Transport, Value};

/// A controller played back from bytes: reads come from the script and "would block" when it runs
/// out, as the Python scripted socket raised `BlockingIOError`; writes are recorded.
struct Scripted {
    /// Shared, so a test can deliver more of the controller's bytes after the session owns this.
    replies: Rc<RefCell<VecDeque<u8>>>,
    sent: Vec<u8>,
}

impl Scripted {
    fn new(replies: &[u8]) -> Scripted {
        Scripted {
            replies: Rc::new(RefCell::new(replies.iter().copied().collect())),
            sent: Vec::new(),
        }
    }
}

impl Read for Scripted {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut replies = self.replies.borrow_mut();
        if replies.is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let n = buf.len().min(replies.len());
        for slot in buf.iter_mut().take(n) {
            *slot = replies.pop_front().expect("counted");
        }
        Ok(n)
    }
}

impl Write for Scripted {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.sent.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Transport for Scripted {
    fn set_nonblocking(&mut self, _nonblocking: bool) -> io::Result<()> {
        Ok(())
    }
}

fn vectors() -> Json {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors.json");
    Json::parse(&std::fs::read_to_string(path).expect("vectors.json reads"))
        .expect("vectors.json parses")
}

fn field<'a>(value: &'a Json, key: &str) -> &'a Json {
    value
        .get(key)
        .unwrap_or_else(|| panic!("vectors.json has `{key}`"))
}

fn hex(text: &str) -> Vec<u8> {
    text.as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).expect("hex"))
        .collect()
}

fn strings(value: &Json) -> Vec<String> {
    value
        .as_arr()
        .expect("a list")
        .iter()
        .map(|item| item.as_str().expect("a string").to_string())
        .collect()
}

fn exchange<'a>(vectors: &'a Json, name: &str) -> &'a Json {
    field(vectors, "exchanges")
        .as_arr()
        .unwrap()
        .iter()
        .find(|exchange| field(exchange, "name").as_str() == Some(name))
        .unwrap_or_else(|| panic!("an exchange called {name}"))
}

/// Run `act` over the exchange's replies; return the session, and what it wrote.
fn replay<R>(
    exchange: &Json,
    act: impl FnOnce(&mut Session<Scripted>) -> Result<R, RtdeError>,
) -> (Result<R, RtdeError>, Session<Scripted>, Vec<u8>) {
    let replies = hex(field(exchange, "replies").as_str().unwrap());
    let mut session = Session::new(Scripted::new(&replies));
    let result = act(&mut session);
    let sent = sent_by(&session);
    (result, session, sent)
}

fn sent_by(session: &Session<Scripted>) -> Vec<u8> {
    session.stream().sent.clone()
}

#[test]
fn the_names_are_the_python_driver_s() {
    let vectors = vectors();
    assert_eq!(strings(field(&vectors, "outputs")), OUTPUTS);
    assert_eq!(strings(field(&vectors, "inputs")), INPUTS);
}

#[test]
fn negotiation_reads_past_a_text_message() {
    let vectors = vectors();
    let case = exchange(&vectors, "negotiate_past_a_text_message");
    let (result, session, sent) = replay(case, |s| s.negotiate(2));
    assert!(result.is_ok(), "{:?}", result.err());
    assert_eq!(sent, hex(field(case, "sent").as_str().unwrap()));
    assert_eq!(
        session.messages,
        strings(field(case, "messages")),
        "the text message is collected, not taken as the reply"
    );

    let case = exchange(&vectors, "negotiate_refused");
    let (result, _, sent) = replay(case, |s| s.negotiate(2));
    assert_eq!(sent, hex(field(case, "sent").as_str().unwrap()));
    let error = result.expect_err("refused");
    assert!(error
        .to_string()
        .starts_with("this controller refused RTDE protocol version 2"));
}

#[test]
fn the_controller_version_reads() {
    let vectors = vectors();
    let case = exchange(&vectors, "controller_version");
    let (result, _, sent) = replay(case, |s| s.controller_version());
    assert_eq!(sent, hex(field(case, "sent").as_str().unwrap()));
    let expected: Vec<u32> = field(case, "result")
        .as_arr()
        .unwrap()
        .iter()
        .map(|part| part.as_f64().unwrap() as u32)
        .collect();
    assert_eq!(result.unwrap().to_vec(), expected);
}

#[test]
fn the_recipes_set_up_as_python_sets_them_up() {
    let vectors = vectors();
    let case = exchange(&vectors, "setup_outputs");
    let (result, session, sent) = replay(case, |s| s.setup_outputs(&OUTPUTS, 125.0));
    assert!(result.is_ok());
    assert_eq!(
        sent,
        hex(field(case, "sent").as_str().unwrap()),
        "frequency and names"
    );
    let recipe = session.output.expect("an output recipe");
    let expected = field(case, "result");
    assert_eq!(
        recipe.identifier as f64,
        field(expected, "id").as_f64().unwrap()
    );
    assert_eq!(recipe.types, strings(field(expected, "types")));

    let case = exchange(&vectors, "setup_inputs");
    let (result, session, sent) = replay(case, |s| s.setup_inputs(&INPUTS));
    assert!(result.is_ok());
    assert_eq!(sent, hex(field(case, "sent").as_str().unwrap()));
    assert_eq!(
        session.input.expect("an input recipe").types,
        strings(field(field(case, "result"), "types"))
    );

    let case = exchange(&vectors, "setup_outputs_in_use");
    let (result, _, _) = replay(case, |s| s.setup_outputs(&OUTPUTS, 125.0));
    let error = result.expect_err("IN_USE is a refusal");
    let python = field(case, "error").as_str().unwrap();
    assert!(python.contains("actual_current") && error.to_string().contains("actual_current"));
    assert!(error
        .to_string()
        .starts_with("the controller answered `IN_USE` for output variable(s) actual_current"));
}

#[test]
fn the_exchange_starts_or_is_refused() {
    let vectors = vectors();
    let case = exchange(&vectors, "start");
    let (result, _, sent) = replay(case, |s| s.start());
    assert!(result.is_ok());
    assert_eq!(sent, hex(field(case, "sent").as_str().unwrap()));
    let case = exchange(&vectors, "start_refused");
    let (result, _, _) = replay(case, |s| s.start());
    assert!(result
        .expect_err("refused")
        .to_string()
        .starts_with("the controller refused to start the data exchange"));
}

#[test]
fn setpoint_packages_are_the_python_bytes() {
    let vectors = vectors();
    for write in field(&vectors, "writes").as_arr().unwrap() {
        let setpoint: Vec<f64> = field(write, "setpoint")
            .as_arr()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect();
        let setpoint: [f64; 6] = setpoint.try_into().expect("six joints");
        let recipe = field(write, "recipe").as_f64().unwrap() as u8;
        let gate = field(write, "gate").as_f64().unwrap() as i32;
        assert_eq!(
            rtde::setpoint_package(recipe, &setpoint, gate).to_vec(),
            hex(field(write, "sent").as_str().unwrap()),
            "{}",
            field(write, "name").as_str().unwrap()
        );
    }
    assert_eq!(control::CMD_SERVO, 1);
    assert_eq!(control::CMD_HOLD, 0);
}

#[test]
fn the_drain_keeps_the_newest_sample() {
    let vectors = vectors();
    let stream = field(&vectors, "stream");
    let mut session = Session::new(Scripted::new(&hex(field(stream, "bytes")
        .as_str()
        .unwrap())));
    // The output recipe as the controller answered it for this driver.
    let setup = exchange(&vectors, "setup_outputs");
    let (_, set_up, _) = replay(setup, |s| s.setup_outputs(&OUTPUTS, 125.0));
    session.output = set_up.output;
    let values = session.newest().expect("the drain").expect("a sample");
    let newest = field(stream, "newest");
    let six = |name: &str| -> Vec<f64> {
        field(newest, name)
            .as_arr()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect()
    };
    assert_eq!(values[0], Value::Vec6(six("actual_q")));
    assert_eq!(values[1], Value::Vec6(six("actual_qd")));
    assert_eq!(values[2], Value::Vec6(six("actual_current")));
    assert_eq!(
        values[4],
        Value::I32(field(newest, "safety_status").as_f64().unwrap() as i32)
    );
    assert_eq!(session.messages, strings(field(stream, "messages")));
    let sample = bridge::Sample::from_values(&values).expect("the fields read");
    assert_eq!(sample.robot_mode, 7);
}

#[test]
fn the_program_is_the_python_program() {
    let vectors = vectors();
    assert_eq!(
        control::SERVO_PROGRAM,
        field(&vectors, "program").as_str().unwrap()
    );
    assert_eq!(
        control::SERVO_PERIOD_S,
        field(&vectors, "period_s").as_f64().unwrap()
    );
}

#[test]
fn every_descriptor_is_the_python_descriptor_as_a_value() {
    let vectors = vectors();
    let cases = field(&vectors, "descriptors").as_arr().unwrap();
    assert_eq!(
        cases.len(),
        14,
        "no model and six models, simulated and not"
    );
    for case in cases {
        let model = field(case, "model").as_str();
        let simulator = field(case, "simulator").as_bool().unwrap();
        let ours = Json::parse(&bridge::describe(model, simulator).expect("renders")).unwrap();
        let theirs = Json::parse(field(case, "descriptor").as_str().unwrap()).unwrap();
        assert_eq!(ours, theirs, "{model:?}, simulator {simulator}");
    }
}

#[test]
fn a_packet_split_where_the_drain_runs_dry_is_completed_next_time() {
    // F75's case: a data package delivered in two halves with "would block" between.
    let vectors = vectors();
    let stream = hex(field(field(&vectors, "stream"), "bytes").as_str().unwrap());
    let setup = exchange(&vectors, "setup_outputs");
    let (_, set_up, _) = replay(setup, |s| s.setup_outputs(&OUTPUTS, 125.0));
    let first_size = u16::from_be_bytes([stream[0], stream[1]]) as usize;
    let transport = Scripted::new(&stream[..first_size - 7]);
    let controller = Rc::clone(&transport.replies);
    let mut session = Session::new(transport);
    session.output = set_up.output;
    assert_eq!(
        session.newest().unwrap(),
        None,
        "half a package is not a sample"
    );
    // The rest arrives, and the next drain completes the package rather than reading its tail
    // as a header.
    controller
        .borrow_mut()
        .extend(stream[first_size - 7..first_size].iter().copied());
    assert!(
        session.newest().unwrap().is_some(),
        "and the next drain completes it"
    );
}

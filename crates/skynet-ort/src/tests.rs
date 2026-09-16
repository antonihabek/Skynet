//! `skynet-ort`'s suite, in two halves.
//!
//! **The half that needs nothing** covers every refusal reachable before ONNX Runtime is loaded —
//! the artefact, the `inputs` block, the model path. Those run on every `cargo test`.
//!
//! **The half that needs a real library** is `#[ignore]`d, the same way the driver-loading suites
//! are: it cannot pass on a machine with no ONNX Runtime, and a test that quietly turns itself into
//! a pass when its subject is absent is the thing §15 calls the most damaging component this
//! platform could ship. Run it with the library on the path or named:
//!
//! ```text
//! SKYNET_ORT_LIBRARY=/path/to/libonnxruntime.so cargo test -p skynet-ort -- --ignored
//! ```
//!
//! What the second half is *for* is worth stating, because "it loaded and did not crash" would be a
//! weak test of a transcribed ABI. Every model in `tests/fixtures` has an output that is arithmetic
//! the test predicts — `setpoint[0] = in[0] + 0.5`, `setpoint[1] = 2 * in[1] - 1` — so a passing run
//! means the right function was called with the right tensor and the numbers came back right. A
//! misaligned call table does not produce those numbers.

use std::path::{Path, PathBuf};

use skynet_infer::session::Session;

use super::*;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
}

/// Write an artefact into a temporary directory whose `model` points at a committed fixture.
///
/// The fixture is named by absolute path so the artefact can live anywhere; the relative-path
/// resolution is covered by its own test below.
fn artefact(name: &str, body: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("skynet-ort-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("policy.json");
    std::fs::write(&path, body).expect("write artefact");
    (dir, path)
}

fn load(name: &str, body: &str) -> Result<(Onnx, Inputs), String> {
    let (dir, path) = artefact(name, body);
    let text = std::fs::read_to_string(&path).expect("read back");
    let result =
        Onnx::load(&path.display().to_string(), &text, &dir).map_err(|error| error.to_string());
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// The library under test, or a panic that says how to provide one.
///
/// Only reached from `#[ignore]`d tests, so it cannot fail a default `cargo test`.
fn library() -> String {
    if let Ok(named) = std::env::var("SKYNET_ORT_LIBRARY") {
        if !named.is_empty() {
            return named;
        }
    }
    default_library().to_string()
}

fn model(file: &str) -> String {
    let path = fixtures().join(file);
    assert!(
        path.exists(),
        "fixture {} is missing — run `python crates/skynet-ort/tests/make-fixtures.py`",
        path.display()
    );
    path.display().to_string().replace('\\', "\\\\")
}

// ── The half that needs no ONNX Runtime ─────────────────────────────────────

#[test]
fn a_document_that_is_not_json_says_so_rather_than_reporting_a_missing_field() {
    let why = load("not-json", "{ this is not json").unwrap_err();
    assert!(why.contains("is not JSON"), "{why}");
    assert!(why.contains("not a tensor dump"), "{why}");
}

/// The shared preamble is `skynet-infer`'s, and this is the check that it is reached from here.
///
/// An `mlp` artefact handed to this runner must be refused **by name**, and the message must name
/// the runner that does read it — otherwise the two runners are one bag of bytes with two parsers.
#[test]
fn another_runners_bytes_are_refused_by_name_and_the_message_names_both_runners() {
    let why = load(
        "wrong-kind",
        r#"{"kind":"mlp","version":1,"inputs":{"channels":1,"reads":["position"]},"model":"x.onnx"}"#,
    )
    .unwrap_err();
    assert!(why.contains("declares kind `mlp`"), "{why}");
    assert!(why.contains("this runner reads `onnx`"), "{why}");
    // The vocabulary, so an operator sees what was available rather than "invalid model".
    assert!(why.contains("skynet-ort"), "{why}");
    assert!(why.contains("feed-forward"), "{why}");
}

#[test]
fn a_later_artefact_version_is_refused_rather_than_read_for_the_familiar_parts() {
    let why = load(
        "later-version",
        r#"{"kind":"onnx","version":2,"inputs":{"channels":1,"reads":["position"]},"model":"x.onnx"}"#,
    )
    .unwrap_err();
    assert!(why.contains("version 2"), "{why}");
    assert!(why.contains("reads version 1"), "{why}");
}

/// `reads` has no default here either, and the reason is the body rather than the parser — F17.
#[test]
fn an_artefact_that_does_not_say_what_it_reads_is_refused_before_a_library_is_touched() {
    let why = load(
        "no-reads",
        r#"{"kind":"onnx","version":1,"inputs":{"channels":1},"model":"x.onnx"}"#,
    )
    .unwrap_err();
    assert!(why.contains("declares no `reads`"), "{why}");
    assert!(why.contains("hold on every cycle"), "{why}");
}

#[test]
fn an_artefact_with_no_model_field_names_the_field_and_what_it_is_for() {
    let why = load(
        "no-model",
        r#"{"kind":"onnx","version":1,"inputs":{"channels":1,"reads":["position"]}}"#,
    )
    .unwrap_err();
    assert!(why.contains("declares no `model`"), "{why}");
    assert!(why.contains("relative to this artefact"), "{why}");
}

#[test]
fn a_model_file_that_is_not_there_names_the_path_it_resolved_to() {
    let why = load(
        "missing-model",
        r#"{"kind":"onnx","version":1,"inputs":{"channels":1,"reads":["position"]},
            "model":"nowhere/absent.onnx"}"#,
    )
    .unwrap_err();
    assert!(why.contains("could not be read"), "{why}");
    // The *resolved* path, not the field's text: "absent.onnx is missing" sends nobody anywhere.
    assert!(why.contains("absent.onnx"), "{why}");
    assert!(why.contains("nowhere"), "{why}");
}

/// A library that is not one fails as a library, and the message says what to do about it.
///
/// Needs no ONNX Runtime because the point is the *absence* of one: this is the refusal an operator
/// on a machine without it actually meets.
#[test]
fn a_library_that_cannot_be_loaded_says_where_to_point_the_variable() {
    let dir = std::env::temp_dir().join(format!("skynet-ort-nolib-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let fake = dir.join("not-onnxruntime.dll");
    std::fs::write(&fake, b"this is not a shared object").expect("write");
    let error = Ort::open(&fake.display().to_string()).unwrap_err();
    let why = error.to_string();
    assert!(why.contains("could not be loaded"), "{why}");
    assert!(why.contains("SKYNET_ORT_LIBRARY"), "{why}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The environment beats the artefact, because the artefact may be signed (§8).
///
/// Against the pure function rather than the process, so this cannot race the `#[ignore]`d tests that
/// read the same variable — see [`resolve_library`].
#[test]
fn the_environment_names_the_library_over_the_artefact() {
    let doc = Json::parse(r#"{"library":"from-artefact.so"}"#).expect("json");
    // Nothing in the environment: the artefact's field is used.
    assert_eq!(resolve_library(None, &doc), "from-artefact.so");
    // Set: it wins.
    assert_eq!(
        resolve_library(Some("from-environment.so"), &doc),
        "from-environment.so"
    );
    // Empty is not a name, and must not shadow the artefact.
    assert_eq!(resolve_library(Some(""), &doc), "from-artefact.so");
    // Neither: the platform's own name, resolved by the system loader.
    let bare = Json::parse("{}").expect("json");
    assert_eq!(resolve_library(None, &bare), default_library());
    assert_eq!(resolve_library(Some(""), &bare), default_library());
    // And an artefact whose field is present but blank falls through the same way.
    let blank = Json::parse(r#"{"library":""}"#).expect("json");
    assert_eq!(resolve_library(None, &blank), default_library());
}

/// The transcription's own arithmetic: the prefix has to be exactly as long as the header says.
///
/// Catches an **inserted or removed** member, which is the mistake that shifts every field after it.
/// It deliberately does not claim to catch a *reorder* — two swapped members of the same width leave
/// the size identical, and what catches that is [`Ort::open`]'s round-trip against a real library.
/// Between them the two cover the transcription: this one needs nothing installed and runs on every
/// commit; that one needs ONNX Runtime and runs where there is one.
#[test]
fn the_call_table_prefix_is_the_length_the_header_says_it_is() {
    assert_eq!(
        std::mem::size_of::<OrtApi>(),
        101 * std::mem::size_of::<*const u8>(),
        "api.rs transcribes members 0..=100 of OrtApi, so the struct is 101 pointers wide. If this \
         moved, a member was inserted or removed — and every field after it now names a different \
         function, because the field order *is* the ABI. Re-derive the list from \
         onnxruntime_c_api.h for the version in API_VERSION and diff it against api.rs."
    );
    // And the two-pointer struct the exported symbol hands back.
    assert_eq!(
        std::mem::size_of::<OrtApiBase>(),
        2 * std::mem::size_of::<*const u8>()
    );
}

// ── The half that needs a real ONNX Runtime ─────────────────────────────────

/// The load-time round-trip, on a real library.
///
/// Proves the error path works while nothing is wrong — see [`Ort::open`] for why that is not
/// circular. If members 0, 1, 2 or 93 were misplaced, this is where it surfaces.
#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn a_real_library_serves_the_api_version_and_passes_the_round_trip() {
    let ort = Ort::open(&library()).expect("ONNX Runtime loads and passes its round-trip");
    // The library reports a version, and it is not the placeholder.
    assert!(!ort.version().is_empty());
    assert_ne!(ort.version(), "unreported", "GetVersionString said nothing");
}

/// The one that makes this crate worth having: a command that is a function of the input.
#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn a_real_model_returns_the_arithmetic_the_fixture_encodes() {
    let (mut session, inputs) = load(
        "affine",
        &format!(
            r#"{{"kind":"onnx","version":1,
                 "inputs":{{"channels":2,"reads":["position","velocity"]}},
                 "model":"{}"}}"#,
            model("affine-4-2.onnx")
        ),
    )
    .expect("the fixture loads");

    assert_eq!(inputs.channels, 2);
    assert_eq!(session.input_width(), 4);
    assert_eq!(session.output_width(), 2);
    assert!(
        session.describe().contains("onnx[4->2]"),
        "{}",
        session.describe()
    );
    assert!(
        session.describe().contains("ONNX Runtime"),
        "{}",
        session.describe()
    );

    // setpoint[0] = in[0] + 0.5 ; setpoint[1] = 2 * in[1] - 1.0
    let mut out = [0.0f32; 2];
    session
        .run(&[0.25, 3.0, 0.0, 0.0], &mut out)
        .expect("a pass");
    assert!((out[0] - 0.75).abs() < 1e-6, "{out:?}");
    assert!((out[1] - 5.0).abs() < 1e-6, "{out:?}");

    // Again, with different numbers, because a session that returned a cached answer would pass the
    // assertion above.
    session
        .run(&[-1.5, 0.5, 9.0, 9.0], &mut out)
        .expect("a pass");
    assert!((out[0] - -1.0).abs() < 1e-6, "{out:?}");
    assert!((out[1] - 0.0).abs() < 1e-6, "{out:?}");
}

/// A symbolic batch is what a real export produces, and the width still comes off the last dimension.
#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn a_dynamic_batch_dimension_is_accepted_and_the_width_is_the_last_one() {
    let (mut session, _) = load(
        "dynamic-batch",
        &format!(
            r#"{{"kind":"onnx","version":1,
                 "inputs":{{"channels":2,"reads":["position","velocity"]}},
                 "model":"{}"}}"#,
            model("affine-4-2-dynamic-batch.onnx")
        ),
    )
    .expect("a symbolic batch loads");
    assert_eq!(session.input_width(), 4);
    let mut out = [0.0f32; 2];
    session
        .run(&[1.0, 1.0, 0.0, 0.0], &mut out)
        .expect("a pass");
    assert!((out[0] - 1.5).abs() < 1e-6, "{out:?}");
    assert!((out[1] - 1.0).abs() < 1e-6, "{out:?}");
}

/// A dynamic *width* is refused where a dynamic batch is accepted. The asymmetry is the point.
#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn a_dynamic_last_dimension_is_refused_because_no_body_could_check_it() {
    let why = load(
        "dynamic-width",
        &format!(
            r#"{{"kind":"onnx","version":1,
                 "inputs":{{"channels":2,"reads":["position","velocity"]}},
                 "model":"{}"}}"#,
            model("dynamic-width.onnx")
        ),
    )
    .unwrap_err();
    assert!(why.contains("dynamic last dimension"), "{why}");
    assert!(why.contains("checked against no body"), "{why}");
}

#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn a_model_that_is_not_float32_is_refused_by_name_rather_than_reinterpreted() {
    let why = load(
        "double",
        &format!(
            r#"{{"kind":"onnx","version":1,
                 "inputs":{{"channels":2,"reads":["position","velocity"]}},
                 "model":"{}"}}"#,
            model("double-input.onnx")
        ),
    )
    .unwrap_err();
    assert!(why.contains("element type"), "{why}");
    assert!(why.contains("float32"), "{why}");
    assert!(why.contains("a different number"), "{why}");
}

#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn a_model_with_two_inputs_is_refused_rather_than_fed_the_first() {
    let why = load(
        "two-inputs",
        &format!(
            r#"{{"kind":"onnx","version":1,
                 "inputs":{{"channels":4,"reads":["position"]}},
                 "model":"{}"}}"#,
            model("two-inputs.onnx")
        ),
    )
    .unwrap_err();
    assert!(why.contains("2 input(s)"), "{why}");
    assert!(why.contains("exactly one of each"), "{why}");
}

/// The width the model declares and the width the layout assembles have to agree, and
/// `skynet_infer::Policy` is what refuses when they do not. This is the join between the two crates.
#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn a_model_wider_than_the_layout_is_refused_by_the_policy_not_by_a_cycle() {
    let (session, inputs) = load(
        "width-clash",
        &format!(
            r#"{{"kind":"onnx","version":1,
                 "inputs":{{"channels":2,"reads":["position"]}},
                 "model":"{}"}}"#,
            model("affine-4-2.onnx")
        ),
    )
    .expect("the model itself is fine");
    // 2 channels x position = 2 floats assembled, and the model reads 4.
    let why = skynet_infer::Policy::new(session, inputs).unwrap_err();
    assert!(why.contains("reads 4 float(s)"), "{why}");
    assert!(why.contains("this layout assembles 2"), "{why}");
}

/// A vision layout, end to end: the pooled frame is part of the width the model is checked against.
#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn a_pooled_frame_counts_towards_the_width_and_reaches_the_model() {
    let (session, inputs) = load(
        "vision",
        &format!(
            r#"{{"kind":"onnx","version":1,
                 "inputs":{{"channels":1,"reads":["position"],
                            "vision":{{"width":4,"height":4,"components":3,"pool":2}}}},
                 "model":"{}"}}"#,
            model("affine-5-1.onnx")
        ),
    )
    .expect("1 channel of position plus a 2x2 pool is 5 floats");
    assert_eq!(inputs.width(), 5);
    assert_eq!(session.input_width(), 5);

    let mut policy = skynet_infer::Policy::new(session, inputs).expect("widths agree");
    // The fixture sums its five inputs. A mid-grey frame pools to 0.5 in every cell, so a position
    // of 1.0 plus four cells of 0.5 is 3.0 — a number that is wrong if the frame does not arrive.
    let pixels = vec![128u8; 4 * 4 * 3];
    let decision = policy.decide(
        Some(skynet_infer::Reading {
            positions: &[1.0],
            velocities: &[f32::NAN],
            efforts: &[f32::NAN],
        }),
        Some(&pixels),
    );
    match decision {
        skynet_infer::Decision::Command(values) => {
            let expected = 1.0 + 4.0 * (128.0 / 255.0);
            assert!(
                (values[0] - expected).abs() < 1e-3,
                "got {values:?}, expected about {expected}"
            );
        }
        other => panic!("expected a command, got {other:?}"),
    }
}

/// A buffer of the wrong width is a caller defect, and is reported rather than read past.
#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn buffers_of_the_wrong_width_are_refused_before_the_library_is_called() {
    let (mut session, _) = load(
        "shape",
        &format!(
            r#"{{"kind":"onnx","version":1,
                 "inputs":{{"channels":2,"reads":["position","velocity"]}},
                 "model":"{}"}}"#,
            model("affine-4-2.onnx")
        ),
    )
    .expect("loads");
    let mut out = [0.0f32; 2];
    let error = session.run(&[1.0, 2.0], &mut out).unwrap_err();
    assert_eq!(
        error,
        skynet_infer::SessionError::Shape {
            expected: (4, 2),
            given: (2, 2)
        }
    );
}

/// `model` is resolved relative to the artefact, so a policy directory can be moved or mounted.
#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn a_relative_model_path_resolves_beside_the_artefact_rather_than_beside_the_process() {
    let dir = std::env::temp_dir().join(format!("skynet-ort-rel-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    std::fs::copy(fixtures().join("affine-4-2.onnx"), dir.join("weights.onnx"))
        .expect("copy fixture");
    let text = r#"{"kind":"onnx","version":1,
                   "inputs":{"channels":2,"reads":["position","velocity"]},
                   "model":"weights.onnx"}"#;
    let (session, _) = Onnx::load("policy.json", text, &dir).expect("resolves beside the artefact");
    assert_eq!(session.input_width(), 4);
    drop(session);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two sessions at once, and one dropped while the other runs.
///
/// The drop order in [`Onnx::drop`] is hand-written, and this is what would catch it being wrong:
/// releasing an environment before its session, or unloading the library before either, tends to
/// fault rather than to return an error.
#[test]
#[ignore = "needs a real ONNX Runtime; see the module documentation"]
fn a_session_dropped_beside_a_live_one_leaves_the_live_one_working() {
    let artefact = format!(
        r#"{{"kind":"onnx","version":1,
             "inputs":{{"channels":2,"reads":["position","velocity"]}},
             "model":"{}"}}"#,
        model("affine-4-2.onnx")
    );
    let (first, _) = load("pair-a", &artefact).expect("first loads");
    let (mut second, _) = load("pair-b", &artefact).expect("second loads");
    drop(first);
    let mut out = [0.0f32; 2];
    second
        .run(&[0.25, 3.0, 0.0, 0.0], &mut out)
        .expect("still runs");
    assert!((out[0] - 0.75).abs() < 1e-6, "{out:?}");
}

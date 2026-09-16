//! Every refusal in this crate, and the arithmetic underneath it.
//!
//! The point of the seam is that all of this runs with no model file, no library and no robot — so
//! the failure paths a production runner will need are written and exercised before it exists.

use super::*;
use crate::session::SessionError;

/// A session that answers with whatever a closure computes.
///
/// The seam's own control. It is not a stand-in for a model: what it is for is reaching
/// [`Hold::SessionRefused`] and [`Hold::OutputNotFinite`], which no honest [`Mlp`] produces and
/// which a real runner produces on a bad day.
type Answer = Box<dyn FnMut(&[f32], &mut [f32]) -> Result<(), SessionError>>;

struct Canned {
    input_width: usize,
    output_width: usize,
    answer: Answer,
    name: String,
}

// By hand, because a boxed closure is not `Debug` and `Policy` derives it. What a refusal message
// needs from this is the widths, which are what a build-time mismatch is about.
impl std::fmt::Debug for Canned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Canned({} -> {})", self.input_width, self.output_width)
    }
}

impl Session for Canned {
    fn input_width(&self) -> usize {
        self.input_width
    }
    fn output_width(&self) -> usize {
        self.output_width
    }
    fn run(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), SessionError> {
        (self.answer)(input, output)
    }
    fn describe(&self) -> &str {
        &self.name
    }
}

fn canned(
    input_width: usize,
    output_width: usize,
    answer: impl FnMut(&[f32], &mut [f32]) -> Result<(), SessionError> + 'static,
) -> Canned {
    Canned {
        input_width,
        output_width,
        answer: Box::new(answer),
        name: "canned".to_string(),
    }
}

/// Doubles every position, so a test can see which input landed where.
///
/// Positions are the first `channels` floats, because they are the first quantity
/// [`position_and_velocity`] names — the grouping [`Policy::decide`] documents.
fn echo(channels: usize) -> Canned {
    canned(2 * channels, channels, move |input, output| {
        for channel in 0..output.len() {
            output[channel] = input[channel] * 2.0;
        }
        Ok(())
    })
}

/// What most joint-space models declare. In the suite rather than in the crate, because every
/// production caller builds an [`Inputs`] by reading a model file.
fn position_and_velocity() -> Reads {
    Reads::of(&[Quantity::Position, Quantity::Velocity]).expect("two distinct quantities")
}

fn proprioceptive(channels: usize) -> Inputs {
    Inputs {
        channels,
        reads: position_and_velocity(),
        vision: None,
    }
}

/// A delivered reading, as the runtime would hand one over. Efforts are unmeasured, which is what a
/// body that does not report them looks like and what every model here declines to read.
fn at<'a>(positions: &'a [f32], velocities: &'a [f32]) -> Option<Reading<'a>> {
    Some(Reading {
        positions,
        velocities,
        efforts: &[f32::NAN; 8],
    })
}

// ── the decision layer ───────────────────────────────────────────────────────

#[test]
fn a_body_reading_becomes_a_command_and_the_inputs_arrive_grouped_by_quantity() {
    let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorded = seen.clone();
    let session = canned(4, 2, move |input, output| {
        recorded.borrow_mut().clear();
        recorded.borrow_mut().extend_from_slice(input);
        output[0] = 1.0;
        output[1] = -1.0;
        Ok(())
    });
    let mut policy = Policy::new(session, proprioceptive(2)).expect("a policy");

    let decision = policy.decide(at(&[0.5, 0.25], &[-1.0, 2.0]), None);
    assert_eq!(decision, Decision::Command(&[1.0, -1.0][..]));
    assert_eq!(
        *seen.borrow(),
        vec![0.5, 0.25, -1.0, 2.0],
        "every channel of the first declared quantity, then every channel of the second"
    );
    assert_eq!(policy.counts().commanded, 1);
    assert_eq!(policy.counts().held(), 0);
}

/// The two ways a policy can be given nothing to run on, and they are not one word.
#[test]
fn no_reading_and_a_reading_too_narrow_are_different_holds() {
    let mut policy = Policy::new(echo(3), proprioceptive(3)).expect("a policy");

    // Nothing delivered: every reason the runtime has for that arrives as one fact here.
    assert_eq!(policy.decide(None, None), Decision::Hold(Hold::NoBody));

    // Delivered, and it is a body with fewer joints than the model reads. A policy running on the
    // wrong robot, which is not the same as a policy running on no robot.
    assert_eq!(
        policy.decide(at(&[0.1, 0.2], &[0.0, 0.0, 0.0]), None),
        Decision::Hold(Hold::BodyTooNarrow {
            joints: 2,
            channels: 3
        })
    );
    assert_eq!(policy.counts().no_body, 1);
    assert_eq!(policy.counts().body_too_narrow, 1);
    assert_eq!(policy.counts().held(), 2);
}

/// The whole pass holds, and the joint *and* the quantity are named. See [`Hold::InputNotFinite`] for
/// why a network cannot do what `Approach` does with a masked joint.
#[test]
fn an_unmeasured_joint_holds_the_whole_pass_and_says_which_one() {
    let mut policy = Policy::new(echo(3), proprioceptive(3)).expect("a policy");
    assert_eq!(
        policy.decide(at(&[0.1, f32::NAN, 0.3], &[0.0; 3]), None),
        Decision::Hold(Hold::InputNotFinite {
            channel: 1,
            quantity: Quantity::Position
        })
    );
    // A velocity nobody measured is the same *kind* of refusal and names the other quantity, which is
    // the difference between going to look at a joint and going to look at what the driver reports.
    assert_eq!(
        policy.decide(at(&[0.1, 0.2, 0.3], &[0.0, 0.0, f32::INFINITY]), None),
        Decision::Hold(Hold::InputNotFinite {
            channel: 2,
            quantity: Quantity::Velocity
        })
    );
    assert_eq!(policy.counts().input_not_finite, 2);
}

/// The reason [`Inputs::reads`] exists: a body that reports no velocity runs a model that does not
/// read one, and is refused by a model that does.
#[test]
fn a_body_that_measures_only_position_runs_a_model_that_reads_only_position() {
    let position_only = Inputs {
        channels: 2,
        reads: Reads::of(&[Quantity::Position]).expect("one quantity"),
        vision: None,
    };
    let mut policy = Policy::new(
        canned(2, 2, |input, output| {
            output.copy_from_slice(input);
            Ok(())
        }),
        position_only,
    )
    .expect("a policy");

    // The LeRobot bridge's fake: position measured, velocity `NaN` (§24's spelling for unmeasured).
    let reading = Some(Reading {
        positions: &[0.25, -0.5],
        velocities: &[f32::NAN, f32::NAN],
        efforts: &[f32::NAN, f32::NAN],
    });
    assert_eq!(
        policy.decide(reading, None),
        Decision::Command(&[0.25, -0.5][..]),
        "a model that never asked for velocity is not stopped by a body that has none"
    );

    // And the same body under a model that does read velocity holds, naming the quantity.
    let mut both = Policy::new(echo(2), proprioceptive(2)).expect("a policy");
    assert_eq!(
        both.decide(reading, None),
        Decision::Hold(Hold::InputNotFinite {
            channel: 0,
            quantity: Quantity::Velocity
        })
    );
}

#[test]
fn a_quantity_named_twice_or_not_at_all_is_refused() {
    assert!(Reads::of(&[]).is_err());
    assert!(Reads::of(&[Quantity::Position, Quantity::Position]).is_err());
    assert_eq!(
        Reads::of(&[Quantity::Effort, Quantity::Position])
            .expect("two distinct")
            .as_slice(),
        &[Quantity::Effort, Quantity::Position],
        "declared order is kept, because it is the order the floats arrive in"
    );
}

#[test]
fn a_session_that_refuses_holds_and_the_first_refusal_is_kept() {
    let mut policy = Policy::new(
        canned(2, 1, |_, _| {
            Err(SessionError::Refused("no execution provider".to_string()))
        }),
        proprioceptive(1),
    )
    .expect("a policy");

    assert_eq!(
        policy.decide(at(&[0.0], &[0.0]), None),
        Decision::Hold(Hold::SessionRefused)
    );
    assert_eq!(
        policy.decide(at(&[0.0], &[0.0]), None),
        Decision::Hold(Hold::SessionRefused)
    );
    assert_eq!(policy.counts().session_refused, 2);
    assert_eq!(
        policy.first_refusal(),
        Some("the model runner refused: no execution provider"),
        "one message, not one per cycle at the control rate"
    );
}

#[test]
fn an_output_that_is_not_a_number_is_never_commanded() {
    // One channel, two outputs: refused when the policy is built rather than on a cycle.
    assert!(Policy::new(
        canned(2, 2, |_, output| {
            output[0] = 0.5;
            output[1] = f32::NAN;
            Ok(())
        }),
        proprioceptive(1),
    )
    .is_err());

    let mut policy = Policy::new(
        canned(2, 1, |_, output| {
            output[0] = f32::NAN;
            Ok(())
        }),
        proprioceptive(1),
    )
    .expect("a policy");
    assert_eq!(
        policy.decide(at(&[0.1], &[0.0]), None),
        Decision::Hold(Hold::OutputNotFinite { index: 0 })
    );
    assert_eq!(policy.counts().output_not_finite, 1);
}

#[test]
fn a_model_whose_width_disagrees_with_its_layout_is_refused_at_build_time() {
    let refusal = Policy::new(echo(2), proprioceptive(3)).expect_err("widths disagree");
    assert!(
        refusal.contains("reads 4") && refusal.contains("assembles 6"),
        "the refusal names both numbers: {refusal}"
    );
}

// ── vision ───────────────────────────────────────────────────────────────────

fn seeing(channels: usize, width: usize, height: usize, pool: usize) -> Inputs {
    Inputs {
        channels,
        reads: position_and_velocity(),
        vision: Some(Vision {
            width,
            height,
            components: 3,
            pool,
        }),
    }
}

#[test]
fn a_vision_model_with_no_frame_holds_and_it_is_not_the_same_as_having_no_camera() {
    let inputs = seeing(1, 4, 4, 2);
    let mut policy = Policy::new(
        canned(inputs.width(), 1, |_, o| {
            o[0] = 1.0;
            Ok(())
        }),
        inputs,
    )
    .expect("a policy");

    assert_eq!(
        policy.decide(at(&[0.0], &[0.0]), None),
        Decision::Hold(Hold::NoFrame)
    );
    assert_eq!(policy.counts().no_frame, 1);

    // A proprioceptive model given the same absent frame commands, because it never looks.
    let mut blind = Policy::new(echo(1), proprioceptive(1)).expect("a policy");
    assert!(matches!(
        blind.decide(at(&[0.25], &[0.0]), None),
        Decision::Command(_)
    ));
    assert_eq!(blind.counts().no_frame, 0);
}

/// §47.6's pool carries no pixel geometry, so the shape is the descriptor's — and a frame that is not
/// that shape is refused rather than read at whatever stride happens to fit.
#[test]
fn a_frame_of_the_wrong_length_is_refused_by_name() {
    let inputs = seeing(1, 4, 4, 2);
    let mut policy = Policy::new(
        canned(inputs.width(), 1, |_, o| {
            o[0] = 1.0;
            Ok(())
        }),
        inputs,
    )
    .expect("a policy");

    let short = vec![0u8; 4 * 4 * 3 - 3];
    assert_eq!(
        policy.decide(at(&[0.0], &[0.0]), Some(&short)),
        Decision::Hold(Hold::FrameShape {
            given: 45,
            declared: 48
        })
    );
    assert_eq!(policy.counts().frame_shape, 1);
}

#[test]
fn pooling_is_the_mean_of_every_byte_in_a_cell_over_255() {
    let vision = Vision {
        width: 2,
        height: 2,
        components: 1,
        pool: 1,
    };
    let mut out = [0.0f32; 1];
    pool_into(&vision, &[0, 255, 0, 255], &mut out);
    assert!((out[0] - 0.5).abs() < 1e-6, "{}", out[0]);

    // Four cells, one pixel each, so each cell is its own pixel.
    let vision = Vision {
        width: 2,
        height: 2,
        components: 1,
        pool: 2,
    };
    let mut out = [0.0f32; 4];
    pool_into(&vision, &[0, 255, 51, 204], &mut out);
    assert_eq!(out[0], 0.0);
    assert!((out[1] - 1.0).abs() < 1e-6);
    assert!((out[2] - 0.2).abs() < 1e-3, "{}", out[2]);
    assert!((out[3] - 0.8).abs() < 1e-3, "{}", out[3]);
}

/// The remainder rows belong to a cell. A pool that stepped by `height / pool` would drop them, and
/// nothing downstream could tell.
#[test]
fn a_grid_that_does_not_divide_the_frame_still_covers_every_row() {
    let vision = Vision {
        width: 2,
        height: 5,
        components: 1,
        pool: 2,
    };
    vision
        .check()
        .expect("two cells over five rows is a shape this pools");

    // Rows 0..2 in the top cells, rows 2..5 in the bottom ones — five rows, none left out. The last
    // row is the one a stepping implementation would drop, so it is the one that is different: a
    // bottom cell covering rows 2..5 means 2/3, and one covering rows 2..4 would mean 1.
    let mut out = [0.0f32; 4];
    pool_into(&vision, &[0, 0, 0, 0, 255, 255, 255, 255, 0, 0], &mut out);
    assert_eq!(out[0], 0.0, "top left, rows 0..2");
    assert_eq!(out[1], 0.0, "top right, rows 0..2");
    for (cell, value) in [(2, out[2]), (3, out[3])] {
        assert!(
            (value - 2.0 / 3.0).abs() < 1e-6,
            "cell {cell} covers rows 2, 3 and 4, so it is 2/3 and not 1: {value}"
        );
    }
}

#[test]
fn a_pool_coarser_than_the_frame_is_refused() {
    let refusal = Policy::new(
        canned(2 + 25, 1, |_, o| {
            o[0] = 0.0;
            Ok(())
        }),
        seeing(1, 4, 4, 5),
    )
    .expect_err("a 5x5 grid over a 4x4 frame");
    assert!(refusal.contains("narrower than a pixel"), "{refusal}");
}

// ── the mlp runner ───────────────────────────────────────────────────────────

/// One layer, identity, so the arithmetic is visible: `out = W x + b`.
const ONE_LAYER: &str = r#"{
  "kind": "mlp",
  "version": 1,
  "inputs": { "channels": 1, "reads": ["position", "velocity"] },
  "layers": [ { "activation": "identity", "weights": [[2.0, 0.5]], "bias": [0.25] } ]
}"#;

#[test]
fn the_mlp_computes_weights_times_input_plus_bias() {
    let (session, inputs) = Mlp::load("one.json", ONE_LAYER).expect("loads");
    assert_eq!(inputs, proprioceptive(1));
    let mut policy = Policy::new(session, inputs).expect("a policy");

    // 2.0 * 0.5 + 0.5 * 4.0 + 0.25 = 3.25
    assert_eq!(
        policy.decide(at(&[0.5], &[4.0]), None),
        Decision::Command(&[3.25][..])
    );
}

#[test]
fn two_layers_compose_and_relu_clamps_at_zero() {
    let text = r#"{
      "kind": "mlp",
      "version": 1,
      "inputs": { "channels": 1, "reads": ["position", "velocity"] },
      "layers": [
        { "activation": "relu", "weights": [[1.0, 0.0], [-1.0, 0.0]], "bias": [0.0, 0.0] },
        { "activation": "identity", "weights": [[1.0, 1.0]], "bias": [0.0] }
      ]
    }"#;
    let (session, inputs) = Mlp::load("two.json", text).expect("loads");
    let mut policy = Policy::new(session, inputs).expect("a policy");

    // relu(1.0) + relu(-1.0) = 1.0 + 0.0
    assert_eq!(
        policy.decide(at(&[1.0], &[0.0]), None),
        Decision::Command(&[1.0][..])
    );
    // relu(-2.0) + relu(2.0) = 0.0 + 2.0
    assert_eq!(
        policy.decide(at(&[-2.0], &[0.0]), None),
        Decision::Command(&[2.0][..])
    );
}

#[test]
fn the_same_weights_and_input_give_the_same_bits_every_time() {
    let (session, inputs) = Mlp::load("one.json", ONE_LAYER).expect("loads");
    let mut policy = Policy::new(session, inputs).expect("a policy");
    let mut seen = Vec::new();
    for _ in 0..8 {
        match policy.decide(at(&[0.1], &[0.3]), None) {
            Decision::Command(values) => seen.push(values[0].to_bits()),
            other => panic!("held: {other:?}"),
        }
    }
    assert!(
        seen.windows(2).all(|pair| pair[0] == pair[1]),
        "a forward pass is a function of its input: {seen:?}"
    );
}

#[test]
fn a_vision_model_loads_its_frame_shape_out_of_the_same_file() {
    let text = r#"{
      "kind": "mlp",
      "version": 1,
      "inputs": { "channels": 1, "reads": ["position", "velocity"],
                   "vision": { "width": 2, "height": 2, "components": 1, "pool": 1 } },
      "layers": [ { "activation": "identity", "weights": [[0.0, 0.0, 1.0]], "bias": [0.0] } ]
    }"#;
    let (session, inputs) = Mlp::load("seeing.json", text).expect("loads");
    assert_eq!(inputs.width(), 3, "two proprioceptive plus one pooled cell");
    let mut policy = Policy::new(session, inputs).expect("a policy");

    // The one weight that is not zero is the pooled cell, so the command *is* the mean intensity.
    assert_eq!(
        policy.decide(at(&[0.0], &[0.0]), Some(&[0, 255, 0, 255])),
        Decision::Command(&[0.5][..])
    );
}

// ── everything the loader refuses ────────────────────────────────────────────

/// Each refusal names the field. A model file is edited by hand at least once, and `invalid model`
/// is a bug report waiting to happen — `CONTRIBUTING.md` makes that a rule rather than a preference.
#[test]
fn every_malformed_model_is_refused_with_the_field_named() {
    let cases: &[(&str, &str, &str)] = &[
        ("not json", "{", "is not JSON"),
        (
            "another runner's bytes",
            r#"{"kind":"onnx","version":1,"inputs":{"channels":1,"reads":["position","velocity"]},"layers":[]}"#,
            "§23.3",
        ),
        (
            "no kind",
            r#"{"version":1,"inputs":{"channels":1,"reads":["position","velocity"]},"layers":[]}"#,
            "declares no `kind`",
        ),
        (
            "a later version",
            r#"{"kind":"mlp","version":2,"inputs":{"channels":1,"reads":["position","velocity"]},"layers":[]}"#,
            "reads version 1",
        ),
        (
            "no inputs block",
            r#"{"kind":"mlp","version":1,"layers":[]}"#,
            "declares no `inputs`",
        ),
        (
            "zero channels",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":0,"reads":["position","velocity"]},"layers":[]}"#,
            "commands nothing",
        ),
        (
            "no layers",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":1,"reads":["position","velocity"]},"layers":[]}"#,
            "declares no layers",
        ),
        (
            "no reads",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":1},"layers":[]}"#,
            "declares no `reads`",
        ),
        (
            "a quantity no body reports",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":1,"reads":["temperature"]},"layers":[]}"#,
            "is not a quantity a body reports",
        ),
        (
            "an unknown activation",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":1,"reads":["position","velocity"]},
                "layers":[{"activation":"gelu","weights":[[1.0,0.0]],"bias":[0.0]}]}"#,
            "not an activation this runner has",
        ),
        (
            "a ragged matrix",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":1,"reads":["position","velocity"]},
                "layers":[{"activation":"relu","weights":[[1.0,0.0],[1.0]],"bias":[0.0,0.0]}]}"#,
            "ragged weight matrix",
        ),
        (
            "a weight that is not finite",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":1,"reads":["position","velocity"]},
                "layers":[{"activation":"relu","weights":[["x",0.0]],"bias":[0.0]}]}"#,
            "is not a number",
        ),
        (
            "a bias short of a row",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":1,"reads":["position","velocity"]},
                "layers":[{"activation":"relu","weights":[[1.0,0.0],[1.0,0.0]],"bias":[0.0]}]}"#,
            "bias value(s)",
        ),
        (
            "layers that do not meet",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":1,"reads":["position","velocity"]},
                "layers":[{"activation":"relu","weights":[[1.0,0.0]],"bias":[0.0]},
                          {"activation":"identity","weights":[[1.0,1.0]],"bias":[0.0]}]}"#,
            "feeding one that reads",
        ),
        (
            "a first layer that disagrees with the inputs block",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":2,"reads":["position","velocity"]},
                "layers":[{"activation":"identity","weights":[[1.0,0.0],[0.0,1.0]],"bias":[0.0,0.0]}]}"#,
            "and its first layer reads",
        ),
        (
            "a last layer that commands the wrong width",
            r#"{"kind":"mlp","version":1,"inputs":{"channels":1,"reads":["position","velocity"]},
                "layers":[{"activation":"identity","weights":[[1.0,0.0],[0.0,1.0]],"bias":[0.0,0.0]}]}"#,
            "one setpoint per commanded channel",
        ),
        (
            "a vision block missing a field",
            r#"{"kind":"mlp","version":1,
                "inputs":{"channels":1,"reads":["position","velocity"],"vision":{"width":2,"height":2,"components":1}},
                "layers":[{"activation":"identity","weights":[[0.0,0.0,1.0]],"bias":[0.0]}]}"#,
            "declares no `pool`",
        ),
    ];

    for (what, text, expected) in cases {
        let refusal = Mlp::load("model.json", text)
            .map(|_| ())
            .expect_err(&format!("{what} should be refused"));
        assert!(
            refusal.contains(expected),
            "{what}: expected a refusal mentioning {expected:?}, got {refusal:?}"
        );
    }
}

/// The seam's shape check, from the session's side rather than the policy's.
#[test]
fn a_session_given_the_wrong_buffers_says_both_widths() {
    let (mut session, _) = Mlp::load("one.json", ONE_LAYER).expect("loads");
    let mut output = [0.0f32; 1];
    let error = session
        .run(&[0.0, 0.0, 0.0], &mut output)
        .expect_err("three floats into a two-float model");
    assert_eq!(
        error,
        SessionError::Shape {
            expected: (2, 1),
            given: (3, 1)
        }
    );
    assert!(error.to_string().contains("reads 2 and writes 1"));
}

#[test]
fn describe_names_the_runner_and_the_artefact_and_claims_nothing_else() {
    let (session, _) = Mlp::load("policy.json", ONE_LAYER).expect("loads");
    assert_eq!(session.describe(), "mlp[1x2 identity] from policy.json");
}

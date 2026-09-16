//! A feed-forward network, and the file it is read out of.
//!
//! # Why this exists rather than a binding to an inference library
//!
//! §23.3 names ONNX Runtime or LibTorch for the in-process backend, and that is still the production
//! answer — see this crate's own documentation for the two routes to it and what each costs. This is
//! the runner that could be finished: a dense multilayer perceptron, arithmetic only, no dependency,
//! no `unsafe`, and bit-identical on every machine the workspace builds for.
//!
//! It is not a stand-in for a large model and it does not pretend to be one. What it is genuinely
//! good for is the shape of policy a small robot actually runs — a few thousand parameters mapping
//! joint state and a pooled camera view onto setpoints — and, more to the point here, it is what lets
//! **every refusal in [`crate::Policy`] be exercised** without a GPU, a package registry or a
//! network. §15's rule cuts both ways: a decision layer whose failure paths have never run is a
//! decision layer nobody has tested.
//!
//! # Determinism, and why it is not an accident
//!
//! Each dot product is summed in index order in `f32`, and Rust does not contract `a * b + c` into a
//! fused multiply-add — there is no `-ffp-contract` and no fast-math. That is the same property
//! `editions/{c,cpp,go,zig}` each check for with a scan and a flag, for the reason §31.2 gives: a
//! fused triple rounds once where two operations round twice, and the results differ in the last
//! bits. So the same weights and the same input give the same command on x86-64 and on aarch64,
//! which is what makes a black-box replay of a run comparable to the run.
//!
//! # The file
//!
//! ```json
//! {
//!   "kind": "mlp",
//!   "version": 1,
//!   "inputs": {
//!     "channels": 2,
//!     "reads": ["position", "velocity"],
//!     "vision": { "width": 8, "height": 8, "components": 3, "pool": 2 }
//!   },
//!   "layers": [
//!     { "activation": "relu", "weights": [[0.1, 0.2, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]], "bias": [0.0] },
//!     { "activation": "tanh", "weights": [[1.0], [1.0]], "bias": [0.0, 0.0] }
//!   ]
//! }
//! ```
//!
//! `weights` is row-major: one row per output, one column per input. `inputs` is in the same file as
//! the weights **on purpose** — a model and the layout it eats are one artefact, and the alternative
//! is two flags an operator can set inconsistently, which produces plausible numbers in the wrong
//! order rather than an error.

use skynet_json::Json;

use crate::session::{Session, SessionError};
use crate::Inputs;

/// What a layer puts its sum through.
///
/// Three, and no more until something needs a fourth. An unknown name is refused by name rather than
/// defaulting to identity: a model whose `gelu` layer was silently run as a linear one produces
/// numbers, and they are somebody else's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// No activation. The usual choice for an output layer commanding a position.
    Identity,
    /// `max(0, x)`.
    Relu,
    /// `tanh(x)`, which bounds the output into `(-1, 1)`.
    ///
    /// Worth naming for what it is **not**: a bound a policy applies to itself is not a safety limit.
    /// §7's guards are the only thing that clamps a command, and they are on the other side of the
    /// ring in a process this one cannot reach. A `tanh` here is a modelling choice.
    Tanh,
}

impl Activation {
    fn apply(self, x: f32) -> f32 {
        match self {
            Activation::Identity => x,
            Activation::Relu => {
                if x > 0.0 {
                    x
                } else {
                    0.0
                }
            }
            Activation::Tanh => x.tanh(),
        }
    }

    fn parse(name: &str) -> Result<Activation, String> {
        match name {
            "identity" | "linear" => Ok(Activation::Identity),
            "relu" => Ok(Activation::Relu),
            "tanh" => Ok(Activation::Tanh),
            other => Err(format!(
                "`{other}` is not an activation this runner has. It has identity, relu and tanh; \
                 running an unknown one as identity would produce numbers from a model nobody \
                 trained"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Activation::Identity => "identity",
            Activation::Relu => "relu",
            Activation::Tanh => "tanh",
        }
    }
}

#[derive(Debug, Clone)]
struct Layer {
    rows: usize,
    cols: usize,
    /// Row-major, `rows * cols` long.
    weights: Vec<f32>,
    bias: Vec<f32>,
    activation: Activation,
}

/// A dense feed-forward network.
#[derive(Debug, Clone)]
pub struct Mlp {
    layers: Vec<Layer>,
    describe: String,
    /// Two buffers, swapped between layers, sized once at load to the widest layer.
    front: Vec<f32>,
    back: Vec<f32>,
}

impl Mlp {
    /// Read a model and the input layout it declares out of one JSON document.
    ///
    /// `label` is what [`Session::describe`] will name — a path, usually — and is never read as a
    /// filename by this function. Loading takes text rather than a path so that the whole of this
    /// crate stays a function of its arguments.
    ///
    /// Every refusal names the field and what it should have been. A model file is a thing an
    /// operator edits by hand at least once, and `invalid model` is a bug report waiting to happen.
    pub fn load(label: &str, text: &str) -> Result<(Mlp, Inputs), String> {
        let doc = Json::parse(text).map_err(|error| {
            format!(
                "{label} is not JSON: {error:?}. A policy file is a document, not a tensor dump"
            )
        })?;

        // `kind`, `version` and the `inputs` block are every runner's, and are read by
        // `crate::read_artefact` so that this runner and `skynet-ort` cannot come to disagree about
        // what a layout means. What is left below is the part that is genuinely this runner's: the
        // weights.
        let inputs = crate::read_artefact(label, &doc, "mlp")?;

        let declared = doc.arr_or_empty("layers");
        if declared.is_empty() {
            return Err(format!(
                "{label} declares no layers. A network of no layers is a policy that commands \
                 nothing, and it would hold for ever while looking loaded"
            ));
        }

        let mut layers: Vec<Layer> = Vec::with_capacity(declared.len());
        for (index, entry) in declared.iter().enumerate() {
            let at = format!("{label} layer {index}");
            let activation = Activation::parse(
                entry
                    .get("activation")
                    .and_then(Json::as_str)
                    .ok_or_else(|| format!("{at} declares no `activation`"))?,
            )
            .map_err(|why| format!("{at}: {why}"))?;

            let rows_json = entry.get("weights").and_then(Json::as_arr).ok_or_else(|| {
                format!("{at} declares no `weights`, which is an array of rows, one per output")
            })?;
            if rows_json.is_empty() {
                return Err(format!("{at} has no rows, so it produces nothing"));
            }
            let cols = rows_json[0]
                .as_arr()
                .ok_or_else(|| format!("{at} row 0 is not an array"))?
                .len();
            if cols == 0 {
                return Err(format!("{at} row 0 has no columns, so it reads nothing"));
            }

            let mut weights = Vec::with_capacity(rows_json.len() * cols);
            for (row_index, row) in rows_json.iter().enumerate() {
                let row = row
                    .as_arr()
                    .ok_or_else(|| format!("{at} row {row_index} is not an array"))?;
                if row.len() != cols {
                    // Refused rather than padded. A ragged matrix is a file that was assembled by
                    // something that lost track, and every number after the short row would be read
                    // against the wrong input.
                    return Err(format!(
                        "{at} row {row_index} has {} column(s) and row 0 has {cols}. A ragged \
                         weight matrix is refused rather than padded: a padded row multiplies real \
                         inputs by zeros nobody chose",
                        row.len()
                    ));
                }
                for (col_index, value) in row.iter().enumerate() {
                    let value = value.as_f64().ok_or_else(|| {
                        format!("{at} row {row_index} column {col_index} is not a number")
                    })? as f32;
                    if !value.is_finite() {
                        return Err(format!(
                            "{at} row {row_index} column {col_index} is not finite. One such weight \
                             makes every output of this layer not finite, so the policy would hold \
                             on every cycle for ever and the reason would be in the last layer \
                             rather than in the file"
                        ));
                    }
                    weights.push(value);
                }
            }

            let bias_json = entry.get("bias").and_then(Json::as_arr).ok_or_else(|| {
                format!("{at} declares no `bias`, which is one number per output")
            })?;
            if bias_json.len() != rows_json.len() {
                return Err(format!(
                    "{at} has {} row(s) and {} bias value(s). One bias per output, and a missing one \
                     is not a zero — a file that lost a number should not be run as though the \
                     number were the identity",
                    rows_json.len(),
                    bias_json.len()
                ));
            }
            let mut bias = Vec::with_capacity(bias_json.len());
            for (bias_index, value) in bias_json.iter().enumerate() {
                let value = value
                    .as_f64()
                    .ok_or_else(|| format!("{at} bias {bias_index} is not a number"))?
                    as f32;
                if !value.is_finite() {
                    return Err(format!("{at} bias {bias_index} is not finite"));
                }
                bias.push(value);
            }

            layers.push(Layer {
                rows: rows_json.len(),
                cols,
                weights,
                bias,
                activation,
            });
        }

        // The layers agree with each other, and the ends agree with what the file said it eats and
        // commands. Checked here, once, because every one of these is a fact about the artefact — and
        // a shape error found on a robot's first cycle is a shape error found too late.
        for pair in layers.windows(2) {
            if pair[0].rows != pair[1].cols {
                return Err(format!(
                    "{label} has a layer producing {} value(s) feeding one that reads {}",
                    pair[0].rows, pair[1].cols
                ));
            }
        }
        let expected_input = inputs.width();
        if layers[0].cols != expected_input {
            return Err(format!(
                "{label} declares inputs {} float(s) wide and its first layer reads {}. The \
                 `inputs` block and the weights describe the same thing and have to agree",
                expected_input, layers[0].cols
            ));
        }
        let last = layers.last().expect("checked non-empty above");
        if last.rows != inputs.channels {
            return Err(format!(
                "{label} declares {} channel(s) and its last layer produces {} value(s). The output \
                 of a policy is one setpoint per commanded channel",
                inputs.channels, last.rows
            ));
        }

        let widest = layers.iter().map(|layer| layer.rows).max().unwrap_or(0);
        let shapes = layers
            .iter()
            .map(|layer| {
                format!(
                    "{}x{} {}",
                    layer.rows,
                    layer.cols,
                    layer.activation.as_str()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        Ok((
            Mlp {
                describe: format!("mlp[{shapes}] from {label}"),
                front: vec![0.0; widest.max(expected_input)],
                back: vec![0.0; widest.max(expected_input)],
                layers,
            },
            inputs,
        ))
    }
}

impl Session for Mlp {
    fn input_width(&self) -> usize {
        self.layers[0].cols
    }

    fn output_width(&self) -> usize {
        self.layers.last().expect("never empty").rows
    }

    fn run(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), SessionError> {
        let expected = (self.input_width(), self.output_width());
        if input.len() != expected.0 || output.len() != expected.1 {
            return Err(SessionError::Shape {
                expected,
                given: (input.len(), output.len()),
            });
        }

        self.front[..input.len()].copy_from_slice(input);
        for index in 0..self.layers.len() {
            let layer = &self.layers[index];
            for row in 0..layer.rows {
                // Index order, in f32, and no fused multiply-add — see the module documentation.
                let base = row * layer.cols;
                let mut sum = layer.bias[row];
                for col in 0..layer.cols {
                    sum += layer.weights[base + col] * self.front[col];
                }
                self.back[row] = layer.activation.apply(sum);
            }
            core::mem::swap(&mut self.front, &mut self.back);
        }
        // The last layer's width is `output.len()`, which `Policy::new` checked against
        // `output_width()` before a single cycle ran.
        output.copy_from_slice(&self.front[..expected.1]);
        Ok(())
    }

    fn describe(&self) -> &str {
        &self.describe
    }
}

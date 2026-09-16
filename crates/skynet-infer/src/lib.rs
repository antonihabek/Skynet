//! §23.3's in-process inference backend: the layer that turns a body and a frame into a command.
//!
//! `docs/status.md` has said, since §47's capture half landed, that *"what is not built is the
//! inference backend §23.3 leaves vacant, so nothing turns a frame into a command"*. This crate is
//! that sentence's subject. It is deliberately **two things, not one**:
//!
//! | | What it is | State |
//! |---|---|---|
//! | [`Policy`] | the decision layer — assemble the input, refuse what cannot be run, turn an output into a command | **built, and every refusal is exercised** |
//! | [`Session`] | the seam a model runner sits behind | **built** |
//! | [`Mlp`] | a runner: dense feed-forward, arithmetic only, no dependency | **built** |
//! | an ONNX Runtime session | §23.3's *production* runner | **built, in `skynet-ort`** |
//!
//! # Where the production runner is, and why it is not here
//!
//! §23.3's table sends an `onnx`, `safetensors` or `torchscript` load plan to an in-process runner
//! and names **ONNX Runtime**. That session exists and lives in `skynet-ort`, one crate *above* this
//! one, for a reason worth stating: reaching ONNX Runtime means `dlopen` and a transcribed C API, so
//! it needs `unsafe` — and this crate, the one that decides what a joint is told to do, stays
//! `#![forbid(unsafe_code)]`. The seam below is what makes that separation possible rather than
//! aspirational.
//!
//! `DRY-COVERAGE-PLAN.md` *Decisions* 12 records why the route is `dlopen` rather than a Cargo
//! dependency: `CONTRIBUTING.md` forbids third-party Rust dependencies, and loading the library at
//! run time keeps that rule a check instead of narrowing it until it admits a transitive tree.
//!
//! [`Mlp`] is not a lesser substitute for it and was not written as a placeholder. It needs no
//! library, it is bit-identical across targets, and it is what lets **every refusal in [`Policy`] be
//! exercised** on a machine with nothing installed. A file declaring `kind: "onnx"` is still refused
//! by [`Mlp`] by name — the two runners each check [`read_artefact`]'s `kind` for themselves, so
//! bytes cannot reach the wrong one.
//!
//! # Inference is never in the real-time path, and this crate cannot put it there
//!
//! §23.3: *"a design where a 40 ms inference spike delays a torque command is a design that falls
//! over."* Structurally guaranteed rather than promised — this code runs in `skynetp`, on the
//! producer side of §25's ring. The kernel reads the ring at the joint rate whatever is happening
//! here; a slow pass produces a stale command, the watchdog sees a sequence that has not moved, and
//! the robot stops in a controlled way. `skynet_policy_host::Summary::overruns` counts the passes
//! that took longer than the control period, and counting is all anything on this side does with it.
//!
//! # What it will not do
//!
//! It never clamps. A policy's output goes to the ring as the policy produced it, and §7's guards on
//! the far side are the only thing that narrows a command. An [`Activation::Tanh`] on an output layer
//! bounds a number, and that is a modelling choice rather than a limit — if it reads as a safety
//! property, that is the confusion §7 exists to prevent.

#![forbid(unsafe_code)]

pub mod mlp;
pub mod session;

#[cfg(test)]
mod tests;

pub use mlp::{Activation, Mlp};
pub use session::{Session, SessionError};

// There is no `position_and_velocity()` convenience here, and a first draft had one.
//
// Every production caller builds an [`Inputs`] by **reading a model file**, which is the whole point
// of [`Inputs::reads`]: the artefact says what it eats. A named default in this crate would have had
// exactly one kind of caller — a test — and `tools/check-production-callers.mjs` said so on the run
// that added it. The suite has its own helper, inside `#[cfg(test)]` where it belongs.

/// One of the three things §24 lets a body report per joint.
///
/// Named rather than implied, because a driver measures some of them and not others: §24 has a driver
/// write `NaN` for a quantity it does not measure, and a model that reads a quantity this body does
/// not have can never run on it. Which ones a model reads is therefore part of what it *is*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quantity {
    /// Measured joint position.
    Position,
    /// Measured joint velocity.
    Velocity,
    /// Measured joint effort — torque or force, in the driver's units.
    Effort,
}

impl Quantity {
    fn parse(name: &str) -> Result<Quantity, String> {
        match name {
            "position" => Ok(Quantity::Position),
            "velocity" => Ok(Quantity::Velocity),
            "effort" => Ok(Quantity::Effort),
            other => Err(format!(
                "`{other}` is not a quantity a body reports. §24 has three per joint: position, \
                 velocity and effort"
            )),
        }
    }
}

/// Which quantities a model reads, in the order they reach it.
///
/// Order is part of the declaration and not a convention this crate picks, because the numbers arrive
/// in one flat vector and a model trained on `position, velocity` fed `velocity, position` produces
/// setpoints rather than an error. Fixed capacity, so [`Inputs`] stays `Copy` and nothing on a control
/// cycle allocates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reads {
    quantities: [Quantity; 3],
    count: usize,
}

impl Reads {
    /// The quantities in `list`, in that order.
    ///
    /// Refuses an empty list, a repeat, and more than the three that exist. A repeat is refused
    /// rather than deduplicated: a file that names position twice is a file somebody generated
    /// wrongly, and running it against a narrower input than it asked for is the shape of failure
    /// that produces numbers.
    pub fn of(list: &[Quantity]) -> Result<Reads, String> {
        if list.is_empty() {
            return Err(
                "a model that reads no quantity reads nothing about the body. Name at least one of \
                 position, velocity or effort"
                    .to_string(),
            );
        }
        if list.len() > 3 {
            return Err(format!(
                "{} quantities named and §24 has three per joint",
                list.len()
            ));
        }
        for (index, quantity) in list.iter().enumerate() {
            if list[..index].contains(quantity) {
                return Err(format!("{quantity:?} is named twice"));
            }
        }
        let mut quantities = [Quantity::Position; 3];
        quantities[..list.len()].copy_from_slice(list);
        Ok(Reads {
            quantities,
            count: list.len(),
        })
    }

    /// The quantities, in declared order.
    pub fn as_slice(&self) -> &[Quantity] {
        &self.quantities[..self.count]
    }
}

/// What the model reads, declared by the artefact it is read out of.
///
/// In the model file rather than on a command line, because a model and its input layout are one
/// thing. Two flags an operator sets separately can disagree, and the failure when they do is not an
/// error: it is the same floats read in a different order, which produces a robot moving somewhere
/// nobody asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Inputs {
    /// How many joints feed in, and how many setpoints come out.
    ///
    /// One number for both, because a policy commands what it observes. A model that reads twelve
    /// joints and commands three is expressible by masking, and this crate does not: §24's mask is
    /// the ring's, and a policy that wanted one would be asking this layer to form an opinion about
    /// which joints matter.
    pub channels: usize,
    /// Which of §24's three per-joint quantities reach the model, and in what order.
    ///
    /// Declared rather than fixed at position-and-velocity, and the reason is a body rather than a
    /// preference: the LeRobot bridge's fake reports position and writes `NaN` for velocity, so a
    /// model that read both could never run on it — it would hold on every cycle with
    /// [`Hold::InputNotFinite`], correctly and uselessly. A model that says it reads position runs
    /// there; one that says it reads velocity is refused by a body that does not measure it, which is
    /// the same information arriving at the right time.
    pub reads: Reads,
    /// The camera, when the model takes one.
    ///
    /// `None` is a **proprioceptive** policy, which is a real and common thing rather than a
    /// degenerate one — a joint-space controller needs no image. It is not the same as a vision
    /// policy whose camera is missing: that one is [`Hold::NoFrame`] on every cycle, and the two
    /// send a person to different places.
    pub vision: Option<Vision>,
}

impl Inputs {
    /// How many floats [`Policy`] will assemble: one per channel per declared quantity, plus the
    /// pooled frame.
    pub fn width(&self) -> usize {
        self.channels * self.reads.as_slice().len()
            + self.vision.map_or(0, |vision| vision.pool * vision.pool)
    }
}

/// Which runner an artefact is asking for, without loading it.
///
/// For a caller that has to choose between runners — `skynetp` is the one — and cannot ask each in
/// turn, because asking involves loading a library. Deliberately shallow: it reads one field and
/// forms no opinion about the rest, and every runner still checks `kind` itself when handed the same
/// text. Two checks of one fact, and that is the right number here: this one picks, and that one
/// refuses. A file that reached the wrong runner by any route is still refused by it.
///
/// Lives here rather than in the binary so that the artefact vocabulary and the parser that reads it
/// stay in one crate. [`KINDS`] is the list; this is how a caller finds out which entry applies.
pub fn kind_of(label: &str, text: &str) -> Result<String, String> {
    use skynet_json::Json;

    let doc = Json::parse(text).map_err(|error| {
        format!("{label} is not JSON: {error:?}. A policy file is a document, not a tensor dump")
    })?;
    match doc.get("kind").and_then(Json::as_str) {
        Some(kind) if KINDS.iter().any(|(name, _)| *name == kind) => Ok(kind.to_string()),
        Some(other) => {
            let known = KINDS
                .iter()
                .map(|(name, what)| format!("`{name}` — {what}"))
                .collect::<Vec<_>>()
                .join("; ");
            Err(format!(
                "{label} declares kind `{other}`, which is not a runner this build has. It has: \
                 {known}"
            ))
        }
        None => Err(format!(
            "{label} declares no `kind`. A model file that does not say what it is cannot be \
             refused by the runner that cannot read it, which is how bytes for one runner get run \
             by another"
        )),
    }
}

/// The artefact kinds this build can run, and what runs each.
///
/// One list, because two would disagree. A file naming a kind that is not here is refused by
/// [`read_artefact`] with this vocabulary in the message, so an operator who typed `onxx` reads what
/// was available rather than "invalid model".
pub const KINDS: [(&str, &str); 2] = [
    (
        "mlp",
        "a dense feed-forward network, in `skynet-infer` itself",
    ),
    (
        "onnx",
        "an ONNX Runtime session, in `skynet-ort` (§23.3's production runner)",
    ),
];

/// The part of a model artefact every runner shares: `kind`, `version` and the `inputs` block.
///
/// `kind` is the caller's own — a runner asks for the one it implements and is told when the file is
/// somebody else's, which is the check that stops bytes for one runner being read by another.
///
/// Shared rather than written once per runner for §78's reason. The `inputs` block is a contract
/// between the artefact and [`Policy`], and two parsers for it would agree until the day one of them
/// gained a field — at which point a model would load with a layout the other runner reads
/// differently, and the numbers would be plausible.
pub fn read_artefact(label: &str, doc: &skynet_json::Json, kind: &str) -> Result<Inputs, String> {
    use skynet_json::Json;

    match doc.get("kind").and_then(Json::as_str) {
        Some(found) if found == kind => {}
        Some(other) => {
            let known = KINDS
                .iter()
                .map(|(name, what)| format!("`{name}` — {what}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(format!(
                "{label} declares kind `{other}` and this runner reads `{kind}`. This build has: \
                 {known}"
            ));
        }
        None => {
            return Err(format!(
                "{label} declares no `kind`. A model file that does not say what it is cannot be \
                 refused by the runner that cannot read it, which is how bytes for one runner get \
                 run by another"
            ))
        }
    }
    match doc.get("version").and_then(Json::as_usize) {
        Some(1) => {}
        Some(other) => {
            return Err(format!(
                "{label} is version {other} and this build reads version 1. A file from a later \
                 build may declare a field that changes what an existing one means, so it is \
                 refused rather than read for the parts that look familiar"
            ))
        }
        None => return Err(format!("{label} declares no `version`")),
    }
    read_inputs(label, doc)
}

/// The `inputs` block: how many channels, which quantities, and the camera when there is one.
fn read_inputs(label: &str, doc: &skynet_json::Json) -> Result<Inputs, String> {
    use skynet_json::Json;

    let block = doc
        .get("inputs")
        .ok_or_else(|| format!("{label} declares no `inputs` block"))?;
    let channels = block.get("channels").and_then(Json::as_usize).ok_or_else(|| {
        format!("{label} inputs declares no `channels`, which is how many joints feed in and are commanded out")
    })?;
    if channels == 0 {
        return Err(format!(
            "{label} declares zero channels, so it commands nothing. A policy that commands nothing \
             is `--backend hold`, which needs no model"
        ));
    }

    // Required, with no default. `["position", "velocity"]` is the obvious one to assume and it is
    // the wrong thing to assume: a body that reports no velocity would then load a model it can never
    // run, and the reason would arrive as a hold on every cycle rather than as a refusal here.
    let named = block.get("reads").and_then(Json::as_arr).ok_or_else(|| {
        format!(
            "{label} inputs declares no `reads`, which is the quantities the model takes per joint \
             — an array of `position`, `velocity` and `effort`, in the order they reach it. There is \
             no default: a model loaded against a body that does not measure what it reads would \
             hold on every cycle instead of being refused here"
        )
    })?;
    let mut quantities = Vec::with_capacity(named.len());
    for (index, entry) in named.iter().enumerate() {
        let name = entry
            .as_str()
            .ok_or_else(|| format!("{label} inputs.reads[{index}] is not a string"))?;
        quantities
            .push(Quantity::parse(name).map_err(|why| format!("{label} inputs.reads: {why}"))?);
    }
    let reads = Reads::of(&quantities).map_err(|why| format!("{label} inputs.reads: {why}"))?;

    let vision = match block.get_present("vision") {
        None => None,
        Some(v) => {
            let field = |name: &str| -> Result<usize, String> {
                v.get(name)
                    .and_then(Json::as_usize)
                    .ok_or_else(|| format!("{label} inputs.vision declares no `{name}`"))
            };
            let vision = Vision {
                width: field("width")?,
                height: field("height")?,
                components: field("components")?,
                pool: field("pool")?,
            };
            vision
                .check()
                .map_err(|why| format!("{label} inputs.vision: {why}"))?;
            Some(vision)
        }
    };

    Ok(Inputs {
        channels,
        reads,
        vision,
    })
}

/// The frame's shape, and how it is reduced before the model sees it.
///
/// # The shape comes from the descriptor, not from the pool
///
/// §47.6's pool header carries a stream name, a slot count and a slot size — and **no pixel
/// geometry**. So a policy handed the pool's bytes cannot decode them from the pool alone, which is
/// not an omission: the shape is the *body's*, and §44's embodiment descriptor is where a body
/// describes itself. The MuJoCo driver declares `width`, `height` and `modality` per sensor there,
/// and a policy resolved against that embodiment is resolved against those numbers.
///
/// What this crate owes in return is a refusal when they disagree. [`Policy::decide`] checks the
/// frame's byte length against `width * height * components` and holds with [`Hold::FrameShape`]
/// rather than reading whatever is there — because a frame read at the wrong stride is not an error,
/// it is a picture of something else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Vision {
    /// Pixels per row, as the descriptor declares.
    pub width: usize,
    /// Rows.
    pub height: usize,
    /// Bytes per pixel: 3 for the `RGB8` every RHAL sensor reports today (§24's `format` 0).
    pub components: usize,
    /// The side of the square grid the frame is average-pooled into.
    ///
    /// `pool * pool` floats reach the model, each the mean of every byte in its cell divided by 255.
    /// Cells are row-major: the top row left to right, then the next.
    ///
    /// # Why pooled rather than whole
    ///
    /// A whole `128 x 96` RGB frame is 36,864 inputs, and a first layer over it is a weight matrix
    /// bigger than every other artefact in this repository put together. Pooling is what a small
    /// policy does, it makes the input width independent of the camera, and — the part that matters
    /// for a robot — it is *declared* rather than implied, so a model trained against one grid cannot
    /// be run against another. A policy that wants the whole frame wants a convolution, which is a
    /// runner this crate does not have and a reason to reach for §23.3's production one.
    pub pool: usize,
}

impl Vision {
    /// Bytes one frame of this shape occupies.
    pub fn frame_bytes(&self) -> usize {
        self.width * self.height * self.components
    }

    fn check(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 || self.components == 0 {
            return Err("a frame of no width, no height or no components carries no pixels".into());
        }
        if self.pool == 0 {
            return Err("a pool of 0 reduces the frame to nothing".into());
        }
        if self.pool > self.width || self.pool > self.height {
            return Err(format!(
                "a pool of {} over a {}x{} frame asks for cells narrower than a pixel. Averaging \
                 nothing yields nothing, and a cell that rounded to empty would feed the model a \
                 zero indistinguishable from a black patch",
                self.pool, self.width, self.height
            ));
        }
        Ok(())
    }
}

/// What a policy decided this cycle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision<'a> {
    /// Command these values — one per channel, in the mode the lease resolved.
    Command(&'a [f32]),
    /// Command nothing, for this reason.
    ///
    /// Never an error and never silence: the caller publishes a hold, so the watchdog keeps seeing a
    /// live producer while the robot does not move. §25.3's answer for a policy that cannot decide.
    Hold(Hold),
}

/// The measured joint state a policy runs on.
///
/// Slices rather than a fixed array, because the width is the **body's** — a driver declares how many
/// joints it measures — and this crate's job is to notice when that is not the width the model reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reading<'a> {
    /// Measured positions, one per joint the body declares.
    pub positions: &'a [f32],
    /// Measured velocities, the same width. `NaN` where a joint does not measure it, which is
    /// [`Hold::InputNotFinite`] and not a zero — and which is why [`Inputs::reads`] exists.
    pub velocities: &'a [f32],
    /// Measured efforts, the same width, `NaN` where unmeasured.
    pub efforts: &'a [f32],
}

impl<'a> Reading<'a> {
    fn of(&self, quantity: Quantity) -> &'a [f32] {
        match quantity {
            Quantity::Position => self.positions,
            Quantity::Velocity => self.velocities,
            Quantity::Effort => self.efforts,
        }
    }

    /// The narrowest of the three, which is how many joints this reading can actually answer for.
    fn joints(&self) -> usize {
        self.positions
            .len()
            .min(self.velocities.len())
            .min(self.efforts.len())
    }
}

/// Why a cycle produced no command.
///
/// Seven reasons, and they are separate because each sends somebody somewhere different — a missing
/// camera is a wiring problem, a frame of the wrong length is a descriptor that disagrees with a
/// driver, a body narrower than the model is a policy running on the wrong robot, and an output that
/// is not finite is a model. Folding them into one would make the count [`Counts`] keeps
/// unactionable, which is the whole reason it is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// No body reading was delivered, so there was nothing to run on.
    ///
    /// Every reason `skynet_policy_host::Body` has for not being `Fresh` arrives here as one word,
    /// and that is correct at this level: an open-loop deployment, a feed that has not started and a
    /// torn slot are different situations for the *runtime*, which keeps them apart and counts them
    /// separately. To a model they are the same fact — there is no body — and re-deriving the
    /// distinction here would be a second opinion about a thing already reported.
    NoBody,
    /// A reading arrived, and it is narrower than the model reads.
    ///
    /// A policy resolved against a body with more joints than this one has: not a missing reading and
    /// not a transient. §6's transport plan is what should have refused it, and this is the last line
    /// before a model would be fed somebody else's joint in the slot for its third.
    BodyTooNarrow {
        /// What the driver declares it measures.
        joints: usize,
        /// What the model reads.
        channels: usize,
    },
    /// The model declares a camera and no frame was shown this cycle.
    NoFrame,
    /// A frame arrived whose length is not the shape the model declares.
    FrameShape {
        /// What arrived.
        given: usize,
        /// What `width * height * components` comes to.
        declared: usize,
    },
    /// A joint the model reads is not measured, so the input cannot be assembled.
    ///
    /// # Why the whole pass holds, where a proportional controller masks one joint
    ///
    /// `skynet_policy_host::Approach` masks out a joint whose position is `NaN` and commands the
    /// rest, because each of its outputs is a function of exactly one input. A network is not
    /// separable: every output depends on every input, so a `NaN` anywhere makes every setpoint a
    /// `NaN`, and substituting a zero would command from a position nobody measured. Holding is the
    /// only honest answer, and the two fields say which joint and which quantity to go and look at.
    ///
    /// A body that never measures the quantity produces this on every cycle, which is loud and is
    /// still the wrong place to learn it. [`Inputs::reads`] is where a model says what it needs, and a
    /// model that only reads position runs on a body that only measures it.
    InputNotFinite {
        /// The channel whose reading could not be used.
        channel: usize,
        /// Which of §24's three it was.
        quantity: Quantity,
    },
    /// The session refused the pass.
    SessionRefused,
    /// The session produced a value that is not finite.
    OutputNotFinite {
        /// The first output that was not a number.
        index: usize,
    },
}

// There is deliberately no `Hold::as_str` here, and a first draft had one.
//
// `skynetp` prints the seven counts with the seven words beside them, so a fixed word per variant
// would be the same vocabulary written twice — §78's shape, and the thing this repository keeps
// finding in itself. When something that is not a command-line summary needs these words (§26's
// black box is the candidate) it can have them; building them for a consumer that does not exist is
// what `tier_profile_to_json` was nearly deleted for.

/// How a run went. Counts of things that happened, and no ratios.
///
/// The same rule `skynet_policy_host::Summary` states for itself: a rate computed over a window this
/// struct cannot see belongs to the party that knows the window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Cycles that produced a command.
    pub commanded: u64,
    /// Cycles held because no body reading was delivered.
    pub no_body: u64,
    /// Cycles held because the reading was narrower than the model reads.
    pub body_too_narrow: u64,
    /// Cycles held because the model wants a frame and none was shown.
    pub no_frame: u64,
    /// Cycles held because a frame was not the declared shape.
    pub frame_shape: u64,
    /// Cycles held because a joint the model reads is not measured.
    pub input_not_finite: u64,
    /// Cycles held because the session refused.
    pub session_refused: u64,
    /// Cycles held because the session produced something that is not a number.
    pub output_not_finite: u64,
}

impl Counts {
    /// Every cycle that produced no command.
    pub fn held(&self) -> u64 {
        self.no_body
            + self.body_too_narrow
            + self.no_frame
            + self.frame_shape
            + self.input_not_finite
            + self.session_refused
            + self.output_not_finite
    }

    fn record(&mut self, hold: &Hold) {
        match hold {
            Hold::NoBody => self.no_body += 1,
            Hold::BodyTooNarrow { .. } => self.body_too_narrow += 1,
            Hold::NoFrame => self.no_frame += 1,
            Hold::FrameShape { .. } => self.frame_shape += 1,
            Hold::InputNotFinite { .. } => self.input_not_finite += 1,
            Hold::SessionRefused => self.session_refused += 1,
            Hold::OutputNotFinite { .. } => self.output_not_finite += 1,
        }
    }
}

/// A loaded model, and everything decided around it.
///
/// Holds its own input and output buffers and reuses them every cycle, for the reason
/// [`Session::run`] takes an output slice: this is a per-control-period path.
#[derive(Debug)]
pub struct Policy<S: Session> {
    session: S,
    inputs: Inputs,
    input: Vec<f32>,
    output: Vec<f32>,
    counts: Counts,
    first_refusal: Option<String>,
}

impl<S: Session> Policy<S> {
    /// Build a policy over a session, for the layout its artefact declared.
    ///
    /// The session's widths are checked against the layout **here**, once, rather than per cycle. A
    /// model one float wider than the body it was given is a configuration mistake, and a robot's
    /// first cycle is too late to find it.
    pub fn new(session: S, inputs: Inputs) -> Result<Policy<S>, String> {
        if inputs.channels == 0 {
            return Err("a policy that commands no channel commands nothing".to_string());
        }
        if let Some(vision) = inputs.vision {
            vision.check()?;
        }
        let width = inputs.width();
        if session.input_width() != width {
            return Err(format!(
                "the model reads {} float(s) and this layout assembles {width}: {} channel(s) of \
                 {:?}{}. The two describe the same input and have to agree",
                session.input_width(),
                inputs.channels,
                inputs.reads.as_slice(),
                match inputs.vision {
                    Some(vision) => format!(", plus a {0}x{0} pooled frame", vision.pool),
                    None => String::new(),
                }
            ));
        }
        if session.output_width() != inputs.channels {
            return Err(format!(
                "the model writes {} value(s) and the layout commands {} channel(s). A policy's \
                 output is one setpoint per commanded channel",
                session.output_width(),
                inputs.channels
            ));
        }
        Ok(Policy {
            input: vec![0.0; width],
            output: vec![0.0; inputs.channels],
            session,
            inputs,
            counts: Counts::default(),
            first_refusal: None,
        })
    }

    /// What the layout is.
    pub fn inputs(&self) -> Inputs {
        self.inputs
    }

    /// What the session says it is.
    pub fn describe(&self) -> &str {
        self.session.describe()
    }

    /// Every cycle so far, by what it did.
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// The first thing the session refused, if it ever did.
    ///
    /// One, and not a log per cycle: a session that refuses tends to refuse identically at the
    /// control rate, and the second thousand copies of a message tell nobody anything the first did
    /// not. The same reason `skynet_kernel_host`'s camera keeps one.
    pub fn first_refusal(&self) -> Option<&str> {
        self.first_refusal.as_deref()
    }

    /// Decide from a body reading and, when the model takes one, a frame.
    ///
    /// `body` is `None` when nothing was delivered this cycle, for any of the reasons a runtime has;
    /// see [`Hold::NoBody`]. `pixels` is the newest frame, or `None` when there is no camera or
    /// nothing has arrived — [`Hold::NoFrame`] tells that apart from a model that never wanted one,
    /// because a model with no `vision` block never looks.
    ///
    /// # What reaches the model, exactly
    ///
    /// Each quantity [`Inputs::reads`] names, in that order, all `channels` of it before the next —
    /// `p0 … pn, v0 … vn` for a model reading position then velocity — in the units the driver
    /// reported them in, unscaled. Then the pooled frame, `pool * pool` means in `[0, 1]`, row-major.
    /// Unscaled because a scale is a property of the training run and inventing one here would be
    /// this layer deciding what a model meant.
    pub fn decide(&mut self, body: Option<Reading<'_>>, pixels: Option<&[u8]>) -> Decision<'_> {
        let channels = self.inputs.channels;
        let Some(reading) = body else {
            return self.hold(Hold::NoBody);
        };
        let joints = reading.joints();
        if joints < channels {
            return self.hold(Hold::BodyTooNarrow { joints, channels });
        }

        let mut at = 0;
        for quantity in self.inputs.reads.as_slice() {
            for (channel, value) in reading.of(*quantity).iter().take(channels).enumerate() {
                if !value.is_finite() {
                    return self.hold(Hold::InputNotFinite {
                        channel,
                        quantity: *quantity,
                    });
                }
                self.input[at] = *value;
                at += 1;
            }
        }

        if let Some(vision) = self.inputs.vision {
            let Some(pixels) = pixels else {
                return self.hold(Hold::NoFrame);
            };
            let declared = vision.frame_bytes();
            if pixels.len() != declared {
                return self.hold(Hold::FrameShape {
                    given: pixels.len(),
                    declared,
                });
            }
            pool_into(&vision, pixels, &mut self.input[at..]);
        }

        // Three disjoint fields of `self`, which is what lets the session take its receiver mutably
        // while the input it reads is a borrow of the buffer beside it.
        if let Err(error) = self.session.run(&self.input, &mut self.output) {
            let message = error.to_string();
            if self.first_refusal.is_none() {
                self.first_refusal = Some(message);
            }
            return self.hold(Hold::SessionRefused);
        }
        if let Some(index) = self.output.iter().position(|value| !value.is_finite()) {
            return self.hold(Hold::OutputNotFinite { index });
        }

        self.counts.commanded += 1;
        Decision::Command(&self.output)
    }

    fn hold(&mut self, hold: Hold) -> Decision<'_> {
        self.counts.record(&hold);
        Decision::Hold(hold)
    }
}

/// Average-pool `pixels` into `out`, one mean in `[0, 1]` per cell, row-major.
///
/// Cell boundaries are computed by multiplication rather than by a step, so a grid that does not
/// divide the frame evenly still covers every pixel exactly once: cell `i` of `pool` spans
/// `[i * n / pool, (i + 1) * n / pool)`. A step of `n / pool` would leave the remainder rows out of
/// every cell, which on a `96`-row frame pooled by `7` is nine rows of the image the model never
/// sees — and nothing would say so.
fn pool_into(vision: &Vision, pixels: &[u8], out: &mut [f32]) {
    for cell_y in 0..vision.pool {
        let y0 = cell_y * vision.height / vision.pool;
        let y1 = (cell_y + 1) * vision.height / vision.pool;
        for cell_x in 0..vision.pool {
            let x0 = cell_x * vision.width / vision.pool;
            let x1 = (cell_x + 1) * vision.width / vision.pool;
            let mut sum = 0.0f64;
            let mut counted = 0usize;
            for y in y0..y1 {
                let row = y * vision.width * vision.components;
                for x in x0..x1 {
                    let at = row + x * vision.components;
                    for component in 0..vision.components {
                        sum += f64::from(pixels[at + component]);
                        counted += 1;
                    }
                }
            }
            // `counted` is never zero: `Vision::check` refuses a pool coarser than the frame, so
            // every span is at least one pixel wide and one row tall.
            out[cell_y * vision.pool + cell_x] = (sum / counted as f64 / 255.0) as f32;
        }
    }
}

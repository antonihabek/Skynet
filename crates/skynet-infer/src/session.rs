//! The seam a model runner sits behind.
//!
//! One trait, three widths and a name. Everything a [`crate::Policy`] does around it — building the
//! input, refusing an input nobody measured, turning an output into a command — is testable against
//! a session that computes something trivial, which is the property that lets §23.3's decision layer
//! be finished before its production runner exists.

/// A loaded model, ready to be asked.
///
/// # Why the output buffer is the caller's
///
/// A forward pass happens once per control period for as long as a robot is moving, and a `Vec`
/// returned per call is an allocation on that path. §24 makes the same argument for the command ring
/// and `skynet-policy-host` makes it for its observation buffer; this is the same rule one layer
/// further out. An implementation that internally allocates is free to, and this signature does not
/// force the choice on the caller.
///
/// # What a session may not do
///
/// It may not read a clock, touch the ring, or decide what to command. It is handed numbers and
/// writes numbers. That is what makes [`crate::Policy`]'s refusals the only place a *decision* is
/// made, and it is the same narrowing `skynet_policy_host::Backend` applies one level up.
pub trait Session {
    /// How many floats this model reads. Fixed for the life of the session.
    ///
    /// Read once, when a [`crate::Policy`] is built, and checked against what the policy will
    /// actually assemble. A mismatch is refused there rather than per cycle: a model whose input is
    /// one float wider than the body it was given is a configuration error, and finding it on the
    /// first cycle of a robot's first run is finding it too late.
    fn input_width(&self) -> usize;

    /// How many floats it writes.
    fn output_width(&self) -> usize;

    /// Run one forward pass.
    ///
    /// `input.len()` is [`Self::input_width`] and `output.len()` is [`Self::output_width`]; a caller
    /// that passes anything else is a defect, and an implementation says so with
    /// [`SessionError::Shape`] rather than reading past an end or writing part of an answer.
    ///
    /// Every value in `input` is finite. That is the policy's guarantee, not this one's — an
    /// implementation is not obliged to check it, and [`crate::Policy`] holds rather than passing a
    /// `NaN` down. See its documentation for why a network cannot be given one channel's worth of
    /// nothing the way a proportional controller can.
    fn run(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), SessionError>;

    /// What produced these numbers, for a log line and for the black box.
    ///
    /// A statement about the runner and the artefact — *"a 3-layer mlp from policy.json"* — and never
    /// a claim about what the policy does. §7.4's rule about not shortening the safety claim is the
    /// same instinct: a name is a name, and a robot describing its own behaviour is a robot making a
    /// claim nobody sanctioned.
    fn describe(&self) -> &str;
}

/// Why a forward pass produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The caller's buffers are not the widths this session declared.
    Shape {
        /// What the session expects: input then output.
        expected: (usize, usize),
        /// What it was given.
        given: (usize, usize),
    },
    /// The runner refused, in its own words.
    ///
    /// A `String` because the runner is the only party that knows what went wrong, and a fixed
    /// vocabulary here would flatten every message an inference library produces into one word.
    Refused(String),
}

impl core::fmt::Display for SessionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SessionError::Shape { expected, given } => write!(
                f,
                "this session reads {} and writes {} float(s), and was given buffers of {} and {}",
                expected.0, expected.1, given.0, given.1
            ),
            SessionError::Refused(why) => write!(f, "the model runner refused: {why}"),
        }
    }
}

impl std::error::Error for SessionError {}

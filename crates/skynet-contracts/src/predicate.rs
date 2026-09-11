//! Declarative predicate evaluation, and §54.2's three-valued refinement.
//!
//! The Rust edition of `reference/typescript/predicate.ts`. Read that file for the design; this one
//! records what reproducing it in a language without `undefined` costs, because those are the
//! places an edition drifts.
//!
//! # Why the robot needs this at all
//!
//! §54.2 is the decision that matters most in Part VI:
//!
//! > `preconditions` in `SkillManifest` is a list of strings — human prose describing what must
//! > hold before the skill is safe. Prose is documentation, and documentation does not stop a
//! > robot. So preconditions become **predicates**, and they are evaluated by the OS before the
//! > skill is allowed to start.
//!
//! The Hub has evaluated predicates since long before that sentence was written — for
//! compatibility rules, over a static context. The skill runtime evaluates the *same predicate
//! shape* over a **live** one. Two implementations of one predicate language is the §31.2 failure,
//! so there is one definition, in `reference/typescript/predicate.ts`, and this is the second
//! edition of it, held to the first by `corpus/predicate/`.
//!
//! # Three values, not two
//!
//! [`evaluate`] answers `satisfied`, `unsatisfied` or `unevaluable`, and [`evaluate_bool`] is
//! `satisfied(…)` of it — never a separate computation. §54.2 requires the third value:
//!
//! > A precondition that cannot be evaluated refuses the skill, **naming the path that was
//! > unresolvable**. Not "assume true".
//!
//! # Reproducing JavaScript, deliberately
//!
//! * **`undefined` is a value here**, spelled [`Value::Undefined`]. JSON has no such thing, so it
//!   arises in exactly two ways: a path that resolved to nothing, and a predicate with no `value`
//!   key at all. Both are the reference's `undefined`, and `undefined === undefined` being *true*
//!   is why `{"op":"eq"}` with no path used to pass.
//! * **`===` on two objects is reference equality**, which an edition holding a parsed document
//!   cannot reproduce by comparing pointers. The reference states the rule instead — two
//!   non-primitive operands are identical when they were read from the same path of the same
//!   context — and [`identical`] is that rule.
//! * **Paths address members only.** An array segment must be a canonical decimal index, so
//!   `capabilities.length` resolves to nothing. `length` is a property of a JavaScript array and
//!   not a member of the document, and an edition without JavaScript arrays could not reproduce it.
//! * **`Number(string)`** is [`js_number`], hex prefixes and all, because a battery reported as
//!   `"17"` must compare the same way on both sides of the platform.
//!
//! # Allocation
//!
//! Operand paths are `String`, and comparing two collection elements builds their paths rather than
//! comparing a base and an index. That is a deliberate trade: this runs at install time and, for
//! the `hold` subset of §54.2, once per cycle over a handful of members — never in the kernel's
//! command path — and the structural version of the comparison was three times the code for an
//! allocation nobody can measure.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use skynet_json::Json;

use crate::js::js_trim;
use crate::semver::{is_valid_range, is_valid_version, satisfies};

/// What an evaluation concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The predicate holds.
    Satisfied,
    /// The predicate does not hold.
    Unsatisfied,
    /// The predicate could not be decided, which refuses whatever it was guarding.
    Unevaluable,
}

impl Outcome {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Satisfied => "satisfied",
            Outcome::Unsatisfied => "unsatisfied",
            Outcome::Unevaluable => "unevaluable",
        }
    }
}

/// The three outcomes, in the reference's declaration order.
pub const OUTCOMES: [Outcome; 3] = [
    Outcome::Satisfied,
    Outcome::Unsatisfied,
    Outcome::Unevaluable,
];

/// Why a predicate could not be decided.
///
/// A closed vocabulary: §54.2 requires the refusal to *name* something, and a free-text reason is
/// the same nothing as a free-text precondition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnevaluableReason {
    /// Not an object, or `op` is not a string. There is no predicate here.
    Malformed,
    /// An operator this edition does not implement.
    UnknownOp,
    /// The predicate names no path to read.
    MissingPath,
    /// `not` with nothing to negate.
    MissingOperand,
    /// A path that names nothing in this context. This is the one §54.2 is about.
    UnresolvedPath,
    /// An operand is present but is not a finite number, and the operator compares numbers.
    NotANumber,
    /// An operand is present but is not a version, or not a range.
    NotAVersion,
}

impl UnevaluableReason {
    /// The wire spelling.
    pub fn as_str(&self) -> &'static str {
        match self {
            UnevaluableReason::Malformed => "malformed",
            UnevaluableReason::UnknownOp => "unknown_op",
            UnevaluableReason::MissingPath => "missing_path",
            UnevaluableReason::MissingOperand => "missing_operand",
            UnevaluableReason::UnresolvedPath => "unresolved_path",
            UnevaluableReason::NotANumber => "not_a_number",
            UnevaluableReason::NotAVersion => "not_a_version",
        }
    }
}

/// Every reason, in the reference's declaration order.
pub const UNEVALUABLE_REASONS: [UnevaluableReason; 7] = [
    UnevaluableReason::Malformed,
    UnevaluableReason::UnknownOp,
    UnevaluableReason::MissingPath,
    UnevaluableReason::MissingOperand,
    UnevaluableReason::UnresolvedPath,
    UnevaluableReason::NotANumber,
    UnevaluableReason::NotAVersion,
];

/// Every operator this edition implements, in the reference's declaration order.
pub const PREDICATE_OPS: [&str; 15] = [
    "and",
    "or",
    "not",
    "eq",
    "neq",
    "in",
    "gte",
    "lte",
    "gt",
    "lt",
    "includes",
    "includes_all",
    "includes_any",
    "exists",
    "semver_satisfies",
];

/// One evaluation's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PredicateOutcome {
    /// What was concluded.
    pub outcome: Outcome,
    /// Why, when `unevaluable`. `None` otherwise.
    pub reason: Option<UnevaluableReason>,
    /// The path that could not be resolved or read, when there is one.
    pub path: Option<String>,
    /// The operator that could not decide. `None` when there was no readable operator at all.
    pub op: Option<String>,
}

impl PredicateOutcome {
    /// Whether this outcome permits whatever it was guarding. Only `satisfied` does.
    pub fn satisfied(&self) -> bool {
        self.outcome == Outcome::Satisfied
    }
}

fn decided(value: bool) -> PredicateOutcome {
    PredicateOutcome {
        outcome: if value {
            Outcome::Satisfied
        } else {
            Outcome::Unsatisfied
        },
        reason: None,
        path: None,
        op: None,
    }
}

fn unevaluable(
    reason: UnevaluableReason,
    path: Option<String>,
    op: Option<&str>,
) -> PredicateOutcome {
    PredicateOutcome {
        outcome: Outcome::Unevaluable,
        reason: Some(reason),
        path,
        op: op.map(ToString::to_string),
    }
}

/// A JavaScript value, which is a JSON value plus `undefined`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Value<'a> {
    /// Nothing was there. A path that named nothing, or an absent `value` key.
    Undefined,
    /// A JSON value, `null` included.
    Present(&'a Json),
}

impl<'a> Value<'a> {
    /// The reference's `value === null || typeof value !== 'object'`.
    fn primitive(&self) -> bool {
        !matches!(
            self,
            Value::Present(Json::Arr(_)) | Value::Present(Json::Obj(_))
        )
    }

    /// `===` between two primitives.
    fn primitive_eq(&self, other: &Value<'a>) -> bool {
        match (self, other) {
            (Value::Undefined, Value::Undefined) => true,
            (Value::Present(a), Value::Present(b)) => match (a, b) {
                (Json::Null, Json::Null) => true,
                (Json::Bool(a), Json::Bool(b)) => a == b,
                (Json::Num(a), Json::Num(b)) => a == b,
                (Json::Str(a), Json::Str(b)) => a == b,
                _ => false,
            },
            _ => false,
        }
    }

    /// The reference's `toArray`: an array is itself, `null` and `undefined` are empty, anything
    /// else is a single-element list.
    fn as_array(self) -> Vec<Value<'a>> {
        match self {
            Value::Undefined | Value::Present(Json::Null) => Vec::new(),
            Value::Present(Json::Arr(items)) => items.iter().map(Value::Present).collect(),
            other => vec![other],
        }
    }
}

/// One side of a comparison.
struct Operand<'a> {
    value: Value<'a>,
    /// Where the value came from, or `None` for a literal.
    path: Option<String>,
    /// False when the path named nothing. A literal is always resolved, even a literal `null`.
    resolved: bool,
}

/// Read `a.b.c` out of a context.
///
/// Returns [`Value::Undefined`] for any missing link. Array segments must be canonical decimal
/// indices — see the module note on members.
pub fn read_path<'a>(context: &'a Json, path: &str) -> Option<&'a Json> {
    match read(context, path) {
        Value::Present(value) => Some(value),
        Value::Undefined => None,
    }
}

fn read<'a>(context: &'a Json, path: &str) -> Value<'a> {
    if path.is_empty() {
        return Value::Undefined;
    }
    let mut current = context;
    for segment in path.split('.') {
        match current {
            Json::Obj(map) => match map.get(segment) {
                Some(next) => current = next,
                None => return Value::Undefined,
            },
            Json::Arr(items) => match canonical_index(segment) {
                Some(index) => match items.get(index) {
                    Some(next) => current = next,
                    None => return Value::Undefined,
                },
                None => return Value::Undefined,
            },
            // `null` and every scalar: the reference stops at `typeof current !== 'object'`.
            _ => return Value::Undefined,
        }
    }
    Value::Present(current)
}

/// `0`, or a digit string with no leading zero — how a JSON array member is named.
fn canonical_index(segment: &str) -> Option<usize> {
    if segment.is_empty() || !segment.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if segment.len() > 1 && segment.starts_with('0') {
        return None;
    }
    segment.parse().ok()
}

/// The path a rule value references, or `None` if it is a literal.
fn referenced_path(value: Option<&Json>) -> Option<&str> {
    let Some(Json::Obj(map)) = value else {
        return None;
    };
    if let Some(Json::Str(reference)) = map.get("__ref") {
        return Some(reference);
    }
    // `{ path: … }` is accepted as an alias so hand-written rules read naturally.
    if map.len() == 1 {
        if let Some(Json::Str(reference)) = map.get("path") {
            return Some(reference);
        }
    }
    None
}

/// The predicate's own path, or `None` when it has none this edition can read.
fn path_of(predicate: &Json) -> Option<&str> {
    match predicate.get("path") {
        Some(Json::Str(path)) if !path.is_empty() => Some(path),
        _ => None,
    }
}

/// The children of a composite, or `None` when `predicates` is present and is not a list.
///
/// Absent and non-array are deliberately different answers. Absent is *no children*, and a
/// conjunction over no children is satisfied. A `predicates` that is a string is a **broken rule**,
/// and answering "no children" there would turn it into a pass.
fn children_of(predicate: &Json) -> Option<&[Json]> {
    match predicate.get("predicates") {
        None | Some(Json::Null) => Some(&[]),
        Some(Json::Arr(items)) => Some(items),
        Some(_) => None,
    }
}

fn from_path<'a>(context: &'a Json, path: &str) -> Operand<'a> {
    let value = read(context, path);
    Operand {
        resolved: value != Value::Undefined,
        value,
        path: Some(path.to_string()),
    }
}

fn from_value<'a>(value: Option<&'a Json>, context: &'a Json) -> Operand<'a> {
    match referenced_path(value) {
        Some(reference) => from_path(context, reference),
        None => Operand {
            value: match value {
                Some(json) => Value::Present(json),
                // An absent `value` key is the reference's `undefined` literal, and it is
                // *resolved*: nothing failed to resolve, there was simply nothing written.
                None => Value::Undefined,
            },
            path: None,
            resolved: true,
        },
    }
}

/// An element of a collection operand, which is a member of the context in its own right.
fn element<'a>(collection: &Operand<'a>, item: Value<'a>, index: usize) -> Operand<'a> {
    Operand {
        value: item,
        path: collection
            .path
            .as_ref()
            .map(|path| format!("{path}.{index}")),
        resolved: true,
    }
}

/// The first operand that named nothing, as a refusal. `None` when they all resolved.
fn first_unresolved(op: &str, operands: [&Operand<'_>; 2]) -> Option<PredicateOutcome> {
    for operand in operands {
        if !operand.resolved {
            return Some(unevaluable(
                UnevaluableReason::UnresolvedPath,
                operand.path.clone(),
                Some(op),
            ));
        }
    }
    None
}

/// The reference's `===`, stated rather than delegated to a host language.
///
/// Two operands are identical when both are primitives and equal, or when both were read from the
/// same path of the same context — which is the only way two non-primitive operands can be one
/// object. A literal object or array in a rule is therefore never equal to anything.
fn identical(left: &Operand<'_>, right: &Operand<'_>) -> bool {
    if left.value.primitive() && right.value.primitive() {
        return left.value.primitive_eq(&right.value);
    }
    match (&left.path, &right.path) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// Membership, with [`identical`]'s rule applied element by element.
fn contains(collection: &Operand<'_>, needle: &Operand<'_>) -> bool {
    collection
        .value
        .as_array()
        .into_iter()
        .enumerate()
        .any(|(index, item)| identical(&element(collection, item, index), needle))
}

/// `Number(value)` for the operands of a numeric comparison, `None` when it is not a finite number.
///
/// The reference accepts a number or a non-blank string, which means this has to be
/// `Number(string)`: decimal and exponent forms, the `0x` / `0o` / `0b` prefixes, and `Infinity`
/// and `NaN` rejected because they are not finite. Rust's own `f64` parser accepts `inf`, `NaN` and
/// `infinity`, which JavaScript does not, so those spellings are refused before it is reached.
///
/// One documented limit: a radix literal wider than 2^53 is accumulated digit by digit in `f64`
/// rather than converted exactly, so it can differ from the reference by one unit in the last
/// place. Every corpus case stays inside the exactly-representable range, and a version or a
/// battery percentage written as an eighteen-digit hexadecimal is not a case worth the arithmetic.
pub fn js_number(value: &Json) -> Option<f64> {
    match value {
        Json::Num(number) if number.is_finite() => Some(*number),
        Json::Str(text) => {
            if js_trim(text).is_empty() {
                return None;
            }
            let parsed = js_string_to_number(js_trim(text))?;
            if parsed.is_finite() {
                Some(parsed)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn js_string_to_number(text: &str) -> Option<f64> {
    for (lower, upper, radix) in [("0x", "0X", 16u32), ("0o", "0O", 8), ("0b", "0B", 2)] {
        let matched = text
            .strip_prefix(lower)
            .or_else(|| text.strip_prefix(upper));
        if let Some(digits) = matched {
            if digits.is_empty() {
                return None;
            }
            let mut accumulated = 0.0f64;
            for character in digits.chars() {
                let digit = character.to_digit(radix)?;
                accumulated = accumulated * f64::from(radix) + f64::from(digit);
            }
            return Some(accumulated);
        }
    }
    // JavaScript's `Number` has no `inf`/`nan` spellings, and `Infinity` is not finite anyway.
    let lowered = text.trim_start_matches(['+', '-']);
    if lowered.eq_ignore_ascii_case("inf")
        || lowered.eq_ignore_ascii_case("infinity")
        || lowered.eq_ignore_ascii_case("nan")
    {
        return None;
    }
    text.parse().ok()
}

/// Evaluate a predicate against a context.
///
/// `predicate` is an [`Option`] because a missing child is the reference's `undefined`, which is
/// malformed rather than an error.
pub fn evaluate(predicate: Option<&Json>, context: &Json) -> PredicateOutcome {
    let Some(predicate) = predicate else {
        return unevaluable(UnevaluableReason::Malformed, None, None);
    };
    // `!predicate` in the reference: `null` is malformed, and so is anything with no string `op`.
    let Some(op) = predicate.get("op").and_then(Json::as_str) else {
        return unevaluable(UnevaluableReason::Malformed, None, None);
    };

    match op {
        // Order is part of the answer for all three: a refusal names *a* predicate, and which one
        // it names must not depend on how an edition iterates.
        "and" => {
            let Some(children) = children_of(predicate) else {
                return unevaluable(UnevaluableReason::Malformed, None, Some(op));
            };
            for child in children {
                let outcome = evaluate(Some(child), context);
                if outcome.outcome != Outcome::Satisfied {
                    return outcome;
                }
            }
            // Nothing was asked for. A skill declaring no preconditions is not one that fails them.
            decided(true)
        }

        "or" => {
            let Some(children) = children_of(predicate) else {
                return unevaluable(UnevaluableReason::Malformed, None, Some(op));
            };
            let mut first_unevaluable: Option<PredicateOutcome> = None;
            for child in children {
                let outcome = evaluate(Some(child), context);
                if outcome.outcome == Outcome::Satisfied {
                    return outcome;
                }
                if outcome.outcome == Outcome::Unevaluable && first_unevaluable.is_none() {
                    first_unevaluable = Some(outcome);
                }
            }
            // An alternative that held would have returned already; an alternative nobody could
            // read means this disjunction is not known to be false, only unread.
            first_unevaluable.unwrap_or_else(|| decided(false))
        }

        "not" => {
            let Some(children) = children_of(predicate) else {
                return unevaluable(UnevaluableReason::Malformed, None, Some(op));
            };
            let child = predicate
                .get_present("predicate")
                .or_else(|| children.first());
            let Some(child) = child else {
                return unevaluable(UnevaluableReason::MissingOperand, None, Some(op));
            };
            let outcome = evaluate(Some(child), context);
            // Ignorance does not negate into knowledge.
            if outcome.outcome == Outcome::Unevaluable {
                return outcome;
            }
            decided(outcome.outcome == Outcome::Unsatisfied)
        }

        "exists" => {
            let Some(path) = path_of(predicate) else {
                return unevaluable(UnevaluableReason::MissingPath, None, Some(op));
            };
            // The one operator that answers *about* resolution, so it never refuses for lack of it.
            match read(context, path) {
                Value::Undefined | Value::Present(Json::Null) => decided(false),
                Value::Present(Json::Arr(items)) => decided(!items.is_empty()),
                Value::Present(Json::Str(text)) => decided(!js_trim(text).is_empty()),
                _ => decided(true),
            }
        }

        "eq" | "neq" => {
            let Some(path) = path_of(predicate) else {
                return unevaluable(UnevaluableReason::MissingPath, None, Some(op));
            };
            let left = from_path(context, path);
            let right = from_value(predicate.get("value"), context);
            if let Some(refused) = first_unresolved(op, [&left, &right]) {
                return refused;
            }
            let equal = identical(&left, &right);
            decided(if op == "eq" { equal } else { !equal })
        }

        "in" => {
            let Some(path) = path_of(predicate) else {
                return unevaluable(UnevaluableReason::MissingPath, None, Some(op));
            };
            let needle = from_path(context, path);
            let collection = from_value(predicate.get("value"), context);
            if let Some(refused) = first_unresolved(op, [&needle, &collection]) {
                return refused;
            }
            decided(contains(&collection, &needle))
        }

        // The context value is a collection that must contain the rule's value.
        "includes" => {
            let Some(path) = path_of(predicate) else {
                return unevaluable(UnevaluableReason::MissingPath, None, Some(op));
            };
            let collection = from_path(context, path);
            let needle = from_value(predicate.get("value"), context);
            if let Some(refused) = first_unresolved(op, [&collection, &needle]) {
                return refused;
            }
            decided(contains(&collection, &needle))
        }

        "includes_all" | "includes_any" => {
            let Some(path) = path_of(predicate) else {
                return unevaluable(UnevaluableReason::MissingPath, None, Some(op));
            };
            let collection = from_path(context, path);
            let wanted = from_value(predicate.get("value"), context);
            if let Some(refused) = first_unresolved(op, [&collection, &wanted]) {
                return refused;
            }
            let items = wanted.value.as_array();
            let each: Vec<bool> = items
                .iter()
                .enumerate()
                .map(|(index, item)| contains(&collection, &element(&wanted, *item, index)))
                .collect();
            // An empty requirement is vacuously satisfied — nothing was asked for. Note that this
            // is reachable only from a requirement that *is* empty, never from one nobody read.
            if op == "includes_any" {
                decided(items.is_empty() || each.iter().any(|held| *held))
            } else {
                decided(each.iter().all(|held| *held))
            }
        }

        "gte" | "lte" | "gt" | "lt" => {
            let Some(path) = path_of(predicate) else {
                return unevaluable(UnevaluableReason::MissingPath, None, Some(op));
            };
            let left_operand = from_path(context, path);
            let right_operand = from_value(predicate.get("value"), context);
            if let Some(refused) = first_unresolved(op, [&left_operand, &right_operand]) {
                return refused;
            }
            let Some(left) = number_of(&left_operand) else {
                return unevaluable(
                    UnevaluableReason::NotANumber,
                    left_operand.path.clone(),
                    Some(op),
                );
            };
            let Some(right) = number_of(&right_operand) else {
                return unevaluable(
                    UnevaluableReason::NotANumber,
                    right_operand.path.clone(),
                    Some(op),
                );
            };
            decided(match op {
                "gte" => left >= right,
                "lte" => left <= right,
                "gt" => left > right,
                _ => left < right,
            })
        }

        "semver_satisfies" => {
            let Some(path) = path_of(predicate) else {
                return unevaluable(UnevaluableReason::MissingPath, None, Some(op));
            };
            let version_operand = from_path(context, path);
            let range_operand = from_value(predicate.get("value"), context);
            if let Some(refused) = first_unresolved(op, [&version_operand, &range_operand]) {
                return refused;
            }
            // A version this edition cannot parse is a fact about the declaration, not a version
            // that fell outside the window — so it is named rather than counted as a miss.
            let version = match version_operand.value {
                Value::Present(Json::Str(text)) if is_valid_version(text) => text,
                _ => {
                    return unevaluable(
                        UnevaluableReason::NotAVersion,
                        version_operand.path.clone(),
                        Some(op),
                    )
                }
            };
            let range = match range_operand.value {
                Value::Present(Json::Str(text)) if is_valid_range(text) => text,
                _ => {
                    return unevaluable(
                        UnevaluableReason::NotAVersion,
                        range_operand.path.clone(),
                        Some(op),
                    )
                }
            };
            decided(satisfies(version, range))
        }

        _ => unevaluable(
            UnevaluableReason::UnknownOp,
            path_of(predicate).map(ToString::to_string),
            Some(op),
        ),
    }
}

fn number_of(operand: &Operand<'_>) -> Option<f64> {
    match operand.value {
        Value::Present(json) => js_number(json),
        Value::Undefined => None,
    }
}

/// The compatibility engine's answer: does this rule hold?
///
/// Derived, never a second computation.
pub fn evaluate_bool(predicate: Option<&Json>, context: &Json) -> bool {
    evaluate(predicate, context).satisfied()
}

/// The wire shape of a [`PredicateOutcome`], with the boolean beside it.
///
/// Every field present, `null` where there is no value: §5's nullable rule, and it keeps a
/// corpus case readable as one line rather than four shapes of the same answer.
///
/// `satisfied` is taken as an argument rather than derived here, and that is the point: the corpus
/// case is produced by calling *both* entry points, so an edition that computed the boolean
/// separately from the outcome would be caught disagreeing with itself.
pub fn outcome_to_json(outcome: &PredicateOutcome, satisfied: bool) -> Json {
    let mut out = Json::obj();
    out.set("satisfied", Json::Bool(satisfied))
        .set("outcome", Json::Str(outcome.outcome.as_str().to_string()))
        .set(
            "reason",
            match &outcome.reason {
                Some(reason) => Json::Str(reason.as_str().to_string()),
                None => Json::Null,
            },
        )
        .set(
            "path",
            match &outcome.path {
                Some(path) => Json::Str(path.clone()),
                None => Json::Null,
            },
        )
        .set(
            "op",
            match &outcome.op {
                Some(op) => Json::Str(op.clone()),
                None => Json::Null,
            },
        );
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    fn context() -> Json {
        Json::parse(
            r#"{
              "robot": {
                "tier": "host",
                "runtime": "1.4.2",
                "capabilities": ["manipulation.grasp", "sensing.camera.depth"],
                "guards": { "active": ["workspace", "velocity"] },
                "state": { "mode": "idle", "battery_percent": 62, "reported": "17",
                           "nothing": null, "empty": [], "blank": "   " }
              },
              "skill": { "needs": ["manipulation.grasp"], "also": ["manipulation.grasp"] }
            }"#,
        )
        .expect("the fixture parses")
    }

    fn run(source: &str) -> PredicateOutcome {
        let predicate = Json::parse(source).expect("the predicate parses");
        evaluate(Some(&predicate), &context())
    }

    fn outcome_of(source: &str) -> Outcome {
        run(source).outcome
    }

    /// The property the whole design rests on: the boolean is a view of the outcome, never a
    /// second computation. Asserted rather than assumed, because the day somebody optimises
    /// `evaluate_bool` into its own switch is the day the two can disagree.
    #[test]
    fn the_boolean_is_the_outcome() {
        for source in [
            r#"{"op":"eq","path":"robot.state.mode","value":"idle"}"#,
            r#"{"op":"eq","path":"robot.state.absent","value":"idle"}"#,
            r#"{"op":"not","predicate":{"op":"eq","path":"robot.state.absent","value":"x"}}"#,
            r#"{"op":"and","predicates":[]}"#,
            r#"{"op":"nonsense"}"#,
        ] {
            let predicate = Json::parse(source).unwrap();
            let outcome = evaluate(Some(&predicate), &context());
            assert_eq!(
                outcome.satisfied(),
                evaluate_bool(Some(&predicate), &context()),
                "{source}"
            );
        }
    }

    /// §54.2: a precondition that cannot be evaluated refuses, **naming the path**.
    #[test]
    fn an_unresolved_path_refuses_and_is_named() {
        let outcome = run(r#"{"op":"gte","path":"robot.state.payload_kg","value":5}"#);
        assert_eq!(outcome.outcome, Outcome::Unevaluable);
        assert_eq!(outcome.reason, Some(UnevaluableReason::UnresolvedPath));
        assert_eq!(outcome.path.as_deref(), Some("robot.state.payload_kg"));
        assert_eq!(outcome.op.as_deref(), Some("gte"));
        assert!(!outcome.satisfied());
    }

    /// The five inputs whose answer changed when the boolean became a view of the outcome. Every
    /// one of them used to be `true`, and every one of them was ignorance reported as a pass.
    #[test]
    fn ignorance_no_longer_passes() {
        for source in [
            // `not` over something unreadable.
            r#"{"op":"not","predicate":{"op":"eq","path":"robot.state.absent","value":"estop"}}"#,
            // `not` with nothing to negate.
            r#"{"op":"not"}"#,
            // `neq` against a path that names nothing.
            r#"{"op":"neq","path":"robot.state.absent","value":"estop"}"#,
            // `eq` where neither side resolves: `undefined === undefined` was true.
            r#"{"op":"eq","path":"robot.state.absent"}"#,
            // The vacuous conjunction over an unreadable requirement.
            r#"{"op":"includes_all","path":"robot.capabilities","value":{"__ref":"skill.absent"}}"#,
        ] {
            let predicate = Json::parse(source).unwrap();
            let outcome = evaluate(Some(&predicate), &context());
            assert_eq!(outcome.outcome, Outcome::Unevaluable, "{source}");
        }
    }

    /// And the idiom that must survive it: `exists` answers *about* absence, so negating it is
    /// meaningful and `not(exists(p))` still means what its author meant.
    #[test]
    fn exists_is_the_escape_hatch() {
        assert_eq!(
            outcome_of(r#"{"op":"exists","path":"robot.state.absent"}"#),
            Outcome::Unsatisfied
        );
        assert_eq!(
            outcome_of(r#"{"op":"not","predicate":{"op":"exists","path":"robot.state.absent"}}"#),
            Outcome::Satisfied
        );
        // Null, an empty list and a blank string are all "no value".
        assert_eq!(
            outcome_of(r#"{"op":"exists","path":"robot.state.nothing"}"#),
            Outcome::Unsatisfied
        );
        assert_eq!(
            outcome_of(r#"{"op":"exists","path":"robot.state.empty"}"#),
            Outcome::Unsatisfied
        );
        assert_eq!(
            outcome_of(r#"{"op":"exists","path":"robot.state.blank"}"#),
            Outcome::Unsatisfied
        );
    }

    /// `===` on two objects, stated as a rule rather than borrowed from a host language.
    #[test]
    fn object_equality_is_path_identity() {
        // A literal array is never equal to anything.
        assert_eq!(
            outcome_of(r#"{"op":"eq","path":"skill.needs","value":["manipulation.grasp"]}"#),
            Outcome::Unsatisfied
        );
        // Two structurally identical arrays at different paths are two objects.
        assert_eq!(
            outcome_of(r#"{"op":"eq","path":"skill.needs","value":{"__ref":"skill.also"}}"#),
            Outcome::Unsatisfied
        );
        // The same path twice is one object.
        assert_eq!(
            outcome_of(r#"{"op":"eq","path":"skill.needs","value":{"__ref":"skill.needs"}}"#),
            Outcome::Satisfied
        );
    }

    /// A collection's member and the same member addressed directly are one object, which is what
    /// makes membership work for non-primitives at all.
    #[test]
    fn a_member_is_the_same_object_as_its_element() {
        assert_eq!(
            outcome_of(r#"{"op":"eq","path":"robot.guards.active.0","value":"workspace"}"#),
            Outcome::Satisfied
        );
        assert_eq!(
            outcome_of(
                r#"{"op":"includes","path":"robot.guards.active","value":{"__ref":"robot.guards.active.1"}}"#
            ),
            Outcome::Satisfied
        );
    }

    /// Paths address members of the document. `length` is a property of a JavaScript array, and an
    /// edition that resolved it would be one the reference could not be held to.
    #[test]
    fn paths_address_members_only() {
        assert_eq!(
            read_path(&context(), "robot.guards.active.1").and_then(Json::as_str),
            Some("velocity")
        );
        assert!(read_path(&context(), "robot.guards.active.length").is_none());
        assert!(read_path(&context(), "robot.guards.active.01").is_none());
        assert!(read_path(&context(), "robot.guards.active.9").is_none());
        assert!(read_path(&context(), "robot.constructor").is_none());
        // A path cannot continue through a scalar, or through an explicit null.
        assert!(read_path(&context(), "robot.tier.length").is_none());
        assert!(read_path(&context(), "robot.state.nothing.deep").is_none());
        // An empty segment names nothing, and neither does an empty path.
        assert!(read_path(&context(), "robot..tier").is_none());
        assert!(read_path(&context(), "").is_none());
    }

    /// `Number(string)`, which is what the reference reaches for and therefore what this must be.
    #[test]
    fn numbers_are_read_the_way_javascript_reads_them() {
        let number = |source: &str| js_number(&Json::parse(source).unwrap());
        assert_eq!(number("62"), Some(62.0));
        assert_eq!(number(r#""17""#), Some(17.0));
        assert_eq!(number(r#""21.5""#), Some(21.5));
        assert_eq!(number(r#""1e3""#), Some(1000.0));
        assert_eq!(number(r#""0x1f""#), Some(31.0));
        assert_eq!(number(r#""0X1F""#), Some(31.0));
        assert_eq!(number(r#""0o17""#), Some(15.0));
        assert_eq!(number(r#""0b101""#), Some(5.0));
        assert_eq!(number(r#""  12  ""#), Some(12.0));
        assert_eq!(number(r#""+5""#), Some(5.0));
        assert_eq!(number(r#""-5""#), Some(-5.0));
        // Not finite, not a number, or not read at all.
        assert_eq!(number(r#""Infinity""#), None);
        assert_eq!(number(r#""-Infinity""#), None);
        assert_eq!(number(r#""abc""#), None);
        assert_eq!(number(r#""   ""#), None);
        assert_eq!(number("null"), None);
        assert_eq!(number("true"), None);
        assert_eq!(number("[1]"), None);
        // Rust's own parser accepts these three spellings and JavaScript does not.
        assert_eq!(number(r#""inf""#), None);
        assert_eq!(number(r#""infinity""#), None);
        assert_eq!(number(r#""NaN""#), None);
        assert_eq!(number(r#""0x""#), None);
        assert_eq!(number(r#""0xzz""#), None);
    }

    /// Rules are operator-supplied rows in a database. A malformed one refuses; it does not take
    /// the evaluation down, and it does not pass.
    #[test]
    fn malformed_rules_refuse_rather_than_panic() {
        for source in [
            "null",
            "42",
            r#""a string""#,
            r#"{"path":"robot.tier"}"#,
            r#"{"op":5}"#,
            r#"{"op":"approximately","path":"robot.state.battery_percent","value":60}"#,
            r#"{"op":"eq","path":5,"value":"idle"}"#,
            r#"{"op":"and","predicates":"nope"}"#,
            r#"{"op":"or","predicates":{"a":1}}"#,
            r#"{"op":"not","predicates":"ab"}"#,
            r#"{"op":"exists","path":["a"]}"#,
        ] {
            let predicate = Json::parse(source).unwrap();
            let outcome = evaluate(Some(&predicate), &context());
            assert_eq!(outcome.outcome, Outcome::Unevaluable, "{source}");
            assert!(!outcome.satisfied(), "{source}");
        }
        // An absent predicate is the reference's `undefined`, and is malformed the same way.
        assert_eq!(evaluate(None, &context()).outcome, Outcome::Unevaluable);
    }

    /// Absent children and broken children are different facts. A conjunction over nothing is
    /// satisfied — a skill declaring no preconditions does not fail them — and a `predicates` that
    /// is a string is a broken rule, which must not become a pass.
    #[test]
    fn no_children_is_not_the_same_as_unreadable_children() {
        assert_eq!(
            outcome_of(r#"{"op":"and","predicates":[]}"#),
            Outcome::Satisfied
        );
        assert_eq!(outcome_of(r#"{"op":"and"}"#), Outcome::Satisfied);
        assert_eq!(
            outcome_of(r#"{"op":"or","predicates":[]}"#),
            Outcome::Unsatisfied
        );
        assert_eq!(
            run(r#"{"op":"and","predicates":"nope"}"#).reason,
            Some(UnevaluableReason::Malformed)
        );
    }

    /// Which predicate a refusal names must not depend on how an edition iterates.
    #[test]
    fn composites_report_in_order() {
        let first_false = run(r#"{"op":"and","predicates":[
                 {"op":"eq","path":"robot.state.mode","value":"estop"},
                 {"op":"gte","path":"robot.state.absent","value":1}]}"#);
        assert_eq!(first_false.outcome, Outcome::Unsatisfied);

        let first_unreadable = run(r#"{"op":"and","predicates":[
                 {"op":"gte","path":"robot.state.absent","value":1},
                 {"op":"eq","path":"robot.state.mode","value":"estop"}]}"#);
        assert_eq!(first_unreadable.outcome, Outcome::Unevaluable);
        assert_eq!(first_unreadable.path.as_deref(), Some("robot.state.absent"));

        // A disjunction with something that holds never had to read the rest.
        let holds = run(r#"{"op":"or","predicates":[
                 {"op":"gte","path":"robot.state.absent","value":1},
                 {"op":"eq","path":"robot.state.mode","value":"idle"}]}"#);
        assert_eq!(holds.outcome, Outcome::Satisfied);
    }

    /// The version half of a precondition, which is why `semver` is in this crate at all.
    #[test]
    fn semver_satisfies_names_an_unreadable_declaration() {
        assert_eq!(
            outcome_of(r#"{"op":"semver_satisfies","path":"robot.runtime","value":"^1.0.0"}"#),
            Outcome::Satisfied
        );
        assert_eq!(
            outcome_of(r#"{"op":"semver_satisfies","path":"robot.runtime","value":">=2.0.0"}"#),
            Outcome::Unsatisfied
        );
        let bad_version =
            run(r#"{"op":"semver_satisfies","path":"robot.state.mode","value":"^1.0.0"}"#);
        assert_eq!(bad_version.reason, Some(UnevaluableReason::NotAVersion));
        assert_eq!(bad_version.path.as_deref(), Some("robot.state.mode"));

        let bad_range =
            run(r#"{"op":"semver_satisfies","path":"robot.runtime","value":"garbage"}"#);
        assert_eq!(bad_range.reason, Some(UnevaluableReason::NotAVersion));
        // A literal range has no path to name, and the outcome says so rather than inventing one.
        assert_eq!(bad_range.path, None);
    }

    #[test]
    fn the_wire_shape_carries_both_answers() {
        let outcome = run(r#"{"op":"gte","path":"robot.state.absent","value":1}"#);
        let json = outcome_to_json(&outcome, outcome.satisfied());
        assert_eq!(
            json.to_canonical().unwrap(),
            // Canonical JSON sorts keys, so this is the order every edition must emit.
            r#"{"op":"gte","outcome":"unevaluable","path":"robot.state.absent","reason":"unresolved_path","satisfied":false}"#
                .to_string()
        );
    }
}

//! Declarative predicate evaluation — `reference/typescript/predicate.ts`.
//!
//! Compatibility rules are data, not code, so a third party can register a robot or a hardware constraint
//! without shipping a frontend change. The module is pure and **total**: it never fails on a malformed
//! rule, and a rule it cannot decide does not pass.
//!
//! # The two answers, and why the boolean is derived
//!
//! §54.2 asks a question a boolean cannot answer: *a precondition that cannot be evaluated refuses the
//! skill, naming the path that was unresolvable.* `false` refuses, which is the right direction, but it
//! collapses "the world is not ready" and "this manifest names something this robot does not expose" into
//! one answer — a battery that needs charging and a skill that will never run here, whose publisher needs
//! telling.
//!
//! So there is one evaluator, and the boolean is `outcome == .satisfied` of it. Two functions computing the
//! same thing by two routes is the drift this repository exists to prevent, and a refinement that cannot
//! disagree with the thing it refines has nothing to drift from.
//!
//! # Where ignorance used to pass
//!
//! Five inputs answered `true` before the outcome existed, all for the same reason — JavaScript's
//! `undefined` arriving at `!`, `!==` and `[].every(...)` — and all now refuse. The one that mattered:
//! `includes_all(robot.capabilities, {__ref: subject.required_capabilities})` **passed when the policy's
//! capability list was missing.** A rule that passes because the fact is absent is worse than one that
//! fails, because nobody goes looking for it.
//!
//! `exists` is the escape hatch and always was: it is the one operator whose job is to ask whether a path
//! resolves, so it answers unsatisfied rather than unevaluable for an absent path.

const std = @import("std");
const Allocator = std.mem.Allocator;
const value = @import("../value.zig");
const js = @import("../js.zig");
const semver = @import("semver.zig");
const Value = value.Value;

pub const Kind = enum {
    satisfied,
    unsatisfied,
    unevaluable,

    pub fn name(self: Kind) []const u8 {
        return @tagName(self);
    }
};

/// Why a predicate could not be decided.
///
/// A closed vocabulary, because §54.2 requires the refusal to *name* something and a free-text reason is the
/// same nothing as a free-text precondition. `corpus/vocabulary/closed-sets.json` holds every edition to
/// this list.
pub const Reason = enum {
    /// Not an object, or `op` is not a string. There is no predicate here.
    malformed,
    /// An operator this edition does not implement.
    unknown_op,
    /// The predicate names no path to read.
    missing_path,
    /// `not` with nothing to negate.
    missing_operand,
    /// A path that names nothing in this context. This is the one §54.2 is about.
    unresolved_path,
    /// An operand is present but is not a finite number, and the operator compares numbers.
    not_a_number,
    /// An operand is present but is not a version, or not a range.
    not_a_version,

    pub fn name(self: Reason) []const u8 {
        return @tagName(self);
    }
};

pub const Outcome = struct {
    outcome: Kind,
    reason: ?Reason = null,
    /// The path that could not be resolved or read, when there is one.
    path: ?[]const u8 = null,
    /// The operator that could not decide. Null when there was no readable operator at all.
    op: ?[]const u8 = null,
};

fn decided(truth: bool) Outcome {
    return .{ .outcome = if (truth) .satisfied else .unsatisfied };
}

fn unevaluable(reason: Reason, path: ?[]const u8, op: ?[]const u8) Outcome {
    return .{ .outcome = .unevaluable, .reason = reason, .path = path, .op = op };
}

/// `^(0|[1-9]\d*)$` — a canonical decimal index, which is what a JSON array member is named. `01` and `1.0`
/// and `-1` are not members of anything.
fn canonicalIndex(segment: []const u8) ?usize {
    if (segment.len == 0) return null;
    if (segment[0] == '0') return if (segment.len == 1) 0 else null;
    if (segment[0] < '1' or segment[0] > '9') return null;
    var out: usize = 0;
    for (segment) |byte| {
        if (byte < '0' or byte > '9') return null;
        // A path naming an index beyond any array this document could hold resolves to nothing, which is
        // the same answer as an index past the end. Bounded so the multiplication cannot wrap.
        out = std.math.mul(usize, out, 10) catch return null;
        out = std.math.add(usize, out, byte - '0') catch return null;
    }
    return out;
}

/// Read `a.b.c` out of a context. Null — the reference's `undefined` — for any missing link.
///
/// **Members only.** An array segment must be a canonical decimal index, so `capabilities.length` does not
/// resolve: `length` is a property of a JavaScript array rather than a member of the JSON document, and an
/// edition without JavaScript arrays could not reproduce it. The same restriction keeps a path off the
/// prototype chain, where a path composed by somebody else has no business going.
pub fn readPath(context: ?*const Value, path: []const u8) ?*const Value {
    if (path.len == 0) return null;
    var current = context;
    var segments = std.mem.splitScalar(u8, path, '.');
    while (segments.next()) |segment| {
        const held = current orelse return null;
        // `current === null || current === undefined` and `typeof current !== 'object'` both answer
        // undefined: a path cannot descend into a string, a number or a boolean.
        switch (held.kind) {
            .array => {
                const index = canonicalIndex(segment) orelse return null;
                // Past the end is `undefined`, which is null here — not a null member.
                current = value.at(held, index);
            },
            .object => {
                // `hasOwnProperty`, so a member present and null resolves to null and a member that is
                // absent resolves to undefined. The two are different facts throughout.
                current = value.get(held, segment) orelse return null;
            },
            else => return null,
        }
    }
    return current;
}

/// One side of a comparison.
///
/// `path` is where the value came from, or null for a literal, and it is load-bearing twice: it is what a
/// refusal names, and it is how `identical` decides object equality without a reference to compare.
const Operand = struct {
    held: ?*const Value,
    path: ?[]const u8,
    /// False when the path named nothing. A literal is always resolved, even a literal null.
    resolved: bool,
};

/// The path a rule value references, or null when it is a literal.
fn referencedPath(held: ?*const Value) ?[]const u8 {
    const v = held orelse return null;
    if (v.kind != .object) return null;
    if (value.get(v, "__ref")) |ref| {
        if (ref.kind == .string) return ref.text;
    }
    // `{ path: ... }` is accepted as an alias so hand-written rules read naturally — and only when it is
    // the *whole* object, so a predicate carrying both a path and an op is not one.
    if (value.get(v, "path")) |alias| {
        if (alias.kind == .string and value.len(v) == 1) return alias.text;
    }
    return null;
}

fn fromPath(context: ?*const Value, path: []const u8) Operand {
    const held = readPath(context, path);
    // `value !== undefined` — a path that resolved to an explicit null *is* resolved.
    return .{ .held = held, .path = path, .resolved = held != null };
}

fn fromValue(held: ?*const Value, context: ?*const Value) Operand {
    const ref = referencedPath(held) orelse return .{ .held = held, .path = null, .resolved = true };
    return fromPath(context, ref);
}

/// An element of a collection operand, which is a member of the context in its own right — and so has a
/// path of its own, which is what makes two elements of one array distinguishable to `identical`.
fn element(a: Allocator, collection: Operand, item: ?*const Value, index: usize) !Operand {
    return .{
        .held = item,
        .resolved = true,
        .path = if (collection.path) |path| try std.fmt.allocPrint(a, "{s}.{d}", .{ path, index }) else null,
    };
}

fn primitive(held: ?*const Value) bool {
    const v = held orelse return true;
    return v.kind != .array and v.kind != .object;
}

/// `===` over two primitives, including the two absences: undefined equals undefined, null equals null, and
/// the two are not each other. NaN equals nothing, itself included.
fn strictEqual(left: ?*const Value, right: ?*const Value) bool {
    if (left == null or right == null) return left == null and right == null;
    const a = left.?;
    const b = right.?;
    if (a.kind != b.kind) return false;
    return switch (a.kind) {
        .null => true,
        .bool => a.truth == b.truth,
        // `NaN === NaN` is false, which `==` on floats already gives; written as a comparison rather than a
        // bit compare for exactly that reason, and because `0 === -0` is true.
        .number => a.number == b.number,
        .string => std.mem.eql(u8, a.text, b.text),
        .array, .object => false,
    };
}

/// JavaScript's `===` on two objects is reference equality, and the reference is the specification, so the
/// rule other editions must reproduce is written here rather than left to the host language.
///
/// Two operands are identical when both are primitives and equal, or when both were read from the **same
/// path of the same context**, which is the only way two non-primitive operands can be one object. A literal
/// object or array in a rule is therefore never equal to anything — which is exactly what `===` does and is
/// worth knowing before writing `{"op":"eq","value":["a"]}` and wondering why it never fires.
fn identical(left: Operand, right: Operand) bool {
    if (primitive(left.held) and primitive(right.held)) return strictEqual(left.held, right.held);
    const left_path = left.path orelse return false;
    const right_path = right.path orelse return false;
    return std.mem.eql(u8, left_path, right_path);
}

/// `toArray`: an array is itself, an absence is empty, and anything else is a list of one.
fn operandLength(held: ?*const Value) usize {
    const v = held orelse return 0;
    if (v.kind == .array) return v.items.items.len;
    if (v.kind == .null) return 0;
    return 1;
}

fn operandAt(held: ?*const Value, index: usize) ?*const Value {
    const v = held orelse return null;
    if (v.kind == .array) return value.at(v, index);
    return v;
}

/// Membership, with `identical`'s rule applied element by element.
fn contains(a: Allocator, collection: Operand, needle: Operand) !bool {
    for (0..operandLength(collection.held)) |index| {
        const item = try element(a, collection, operandAt(collection.held, index), index);
        if (identical(item, needle)) return true;
    }
    return false;
}

/// `toNumber`: a finite number is itself, a non-blank string goes through `Number(...)`, and everything
/// else — a boolean, an array, an absence, a blank string — is not a number.
fn toNumber(held: ?*const Value) ?f64 {
    const v = held orelse return null;
    if (v.kind == .number) return if (std.math.isFinite(v.number)) v.number else null;
    if (v.kind == .string) {
        // A blank string converts to zero, and the reference refuses it before converting: a robot that
        // reported `"   "` for its battery has not reported a number.
        if (js.trim(v.text).len == 0) return null;
        const parsed = js.stringToNumber(v.text);
        return if (std.math.isFinite(parsed)) parsed else null;
    }
    return null;
}

/// The predicate's own path, or null when it has none this edition can read. An empty path is none.
fn pathOf(predicate: ?*const Value) ?[]const u8 {
    const path = value.get(predicate, "path") orelse return null;
    if (path.kind != .string or path.text.len == 0) return null;
    return path.text;
}

const Children = union(enum) {
    /// Absent is *no children*, and a conjunction over no children is satisfied — a skill declaring no
    /// preconditions does not fail them.
    none,
    list: *const Value,
    /// A `predicates` that is a string is a **broken rule**, and answering "no children" there would turn
    /// it into a pass, which is the failure this module exists to stop.
    broken,
};

fn childrenOf(predicate: ?*const Value) Children {
    const held = value.get(predicate, "predicates") orelse return .none;
    if (held.kind == .null) return .none;
    if (held.kind != .array) return .broken;
    return .{ .list = held };
}

pub fn evaluateOutcome(a: Allocator, predicate: ?*const Value, context: ?*const Value) anyerror!Outcome {
    // `!predicate || typeof predicate !== 'object' || typeof predicate.op !== 'string'` — there is no
    // predicate here, and this is the line that makes the module total.
    const held = predicate orelse return unevaluable(.malformed, null, null);
    if (held.kind != .object) return unevaluable(.malformed, null, null);
    const op_value = value.get(held, "op") orelse return unevaluable(.malformed, null, null);
    if (op_value.kind != .string) return unevaluable(.malformed, null, null);
    const op = op_value.text;
    const path = pathOf(held);

    const eql = std.mem.eql;
    if (eql(u8, op, "and") or eql(u8, op, "or")) {
        const children = childrenOf(held);
        if (children == .broken) return unevaluable(.malformed, null, op);
        const list: ?*const Value = if (children == .list) children.list else null;
        const conjunction = eql(u8, op, "and");
        // Order is part of the answer: a refusal names *a* predicate, and which one it names must not depend
        // on how an edition iterates.
        var first_unevaluable: ?Outcome = null;
        for (0..value.len(list)) |index| {
            const outcome = try evaluateOutcome(a, value.at(list, index), context);
            if (conjunction) {
                if (outcome.outcome != .satisfied) return outcome;
                continue;
            }
            if (outcome.outcome == .satisfied) return outcome;
            if (outcome.outcome == .unevaluable and first_unevaluable == null) first_unevaluable = outcome;
        }
        // Nothing was asked for; or, for a disjunction, an alternative that held would have returned
        // already and an alternative nobody could read means this is not known to be false, only unread.
        if (conjunction) return decided(true);
        return first_unevaluable orelse decided(false);
    }

    if (eql(u8, op, "not")) {
        const children = childrenOf(held);
        if (children == .broken) return unevaluable(.malformed, null, op);
        const list: ?*const Value = if (children == .list) children.list else null;
        const child = value.get(held, "predicate") orelse value.at(list, 0) orelse
            return unevaluable(.missing_operand, null, op);
        const outcome = try evaluateOutcome(a, child, context);
        // Ignorance does not negate into knowledge, and this is the line that used to say it did.
        if (outcome.outcome == .unevaluable) return outcome;
        return decided(outcome.outcome == .unsatisfied);
    }

    if (eql(u8, op, "exists")) {
        const held_path = path orelse return unevaluable(.missing_path, null, op);
        // The one operator that answers *about* resolution, so it never refuses for lack of it.
        const found = readPath(context, held_path);
        if (value.nullish(found)) return decided(false);
        const v = found.?;
        if (v.kind == .array) return decided(v.items.items.len > 0);
        if (v.kind == .string) return decided(js.trim(v.text).len != 0);
        return decided(true);
    }

    const equality = eql(u8, op, "eq") or eql(u8, op, "neq");
    const membership = eql(u8, op, "in") or eql(u8, op, "includes");
    const coverage = eql(u8, op, "includes_all") or eql(u8, op, "includes_any");
    const numeric = eql(u8, op, "gte") or eql(u8, op, "lte") or eql(u8, op, "gt") or eql(u8, op, "lt");
    const version_range = eql(u8, op, "semver_satisfies");

    if (equality or membership or coverage or numeric or version_range) {
        const held_path = path orelse return unevaluable(.missing_path, null, op);
        const left = fromPath(context, held_path);
        const right = fromValue(value.get(held, "value"), context);

        // The order the two operands are checked in is the order the reference passes them, and it decides
        // which path a refusal names when both are unresolved.
        if (!left.resolved) return unevaluable(.unresolved_path, left.path, op);
        if (!right.resolved) return unevaluable(.unresolved_path, right.path, op);

        if (equality) {
            const equal = identical(left, right);
            return decided(if (eql(u8, op, "eq")) equal else !equal);
        }
        if (membership) {
            // `in` reads the path as the needle and the value as the collection; `includes` is the other way
            // round. Two operators, one comparison.
            if (eql(u8, op, "in")) return decided(try contains(a, right, left));
            return decided(try contains(a, left, right));
        }
        if (coverage) {
            const count = operandLength(right.held);
            var any = false;
            var all = true;
            for (0..count) |index| {
                const wanted = try element(a, right, operandAt(right.held, index), index);
                if (try contains(a, left, wanted)) any = true else all = false;
            }
            // An empty requirement is vacuously satisfied — nothing was asked for. Reachable only from a
            // requirement that *is* empty, never from one nobody could read.
            if (eql(u8, op, "includes_any")) return decided(count == 0 or any);
            return decided(all);
        }
        if (numeric) {
            const left_number = toNumber(left.held) orelse return unevaluable(.not_a_number, left.path, op);
            const right_number = toNumber(right.held) orelse return unevaluable(.not_a_number, right.path, op);
            if (eql(u8, op, "gte")) return decided(left_number >= right_number);
            if (eql(u8, op, "lte")) return decided(left_number <= right_number);
            if (eql(u8, op, "gt")) return decided(left_number > right_number);
            return decided(left_number < right_number);
        }

        // A version this edition cannot parse is a fact about the declaration, not a version that fell
        // outside the window — so it is named rather than counted as a miss.
        const version = left.held orelse return unevaluable(.not_a_version, left.path, op);
        if (version.kind != .string or !semver.validVersion(version.text)) {
            return unevaluable(.not_a_version, left.path, op);
        }
        const range = right.held orelse return unevaluable(.not_a_version, right.path, op);
        if (range.kind != .string or !try semver.validRange(a, range.text)) {
            return unevaluable(.not_a_version, right.path, op);
        }
        return decided(try semver.satisfies(a, version.text, range.text));
    }

    return unevaluable(.unknown_op, path, op);
}

pub fn evaluateCase(a: Allocator, input: ?*const Value) anyerror!*const Value {
    const outcome = try evaluateOutcome(a, value.get(input, "predicate"), value.get(input, "context"));

    // The boolean and the outcome in one document, deliberately. `evaluatePredicate` is defined as
    // `satisfied(...)` of the outcome, so an edition that computed the two separately would be free to
    // disagree with itself — and the whole argument for the third value is that it refines the boolean
    // rather than offering a second opinion. Freezing both makes that checkable.
    const out = try value.object(a);
    try value.set(a, out, "satisfied", value.boolean(outcome.outcome == .satisfied));
    try value.set(a, out, "outcome", try value.string(a, outcome.outcome.name()));
    try value.set(a, out, "reason", if (outcome.reason) |reason|
        try value.string(a, reason.name())
    else
        value.nul());
    try value.set(a, out, "path", if (outcome.path) |path| try value.string(a, path) else value.nul());
    try value.set(a, out, "op", if (outcome.op) |op| try value.string(a, op) else value.nul());
    return out;
}

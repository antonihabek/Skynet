//! §27's operating-state machine — `reference/typescript/opstate.ts`.
//!
//! Pure, clockless and total: every combination of conditions produces a state, including combinations
//! nobody anticipated, which is the property §33.3 asks of anything that converges.
//!
//! Three corpus files, and the two small ones are the interesting ones. `evaluate` is thirty-eight scripted
//! conditions. `properties` freezes `safe_state`, `commands` and `sticky` for **every** state, and `warning`
//! freezes the tone for **every ordered pair** of states — forty-nine of them — so an edition cannot get one
//! transition right by accident and the rest wrong.

const std = @import("std");

const value = @import("../value.zig");

const Allocator = std.mem.Allocator;
const Value = value.Value;

/// §27's five states plus §48.1's two, **ordered least capable first** — the order `derive` descends through
/// its rules, and the order `properties` and `warning` iterate in. Sorting this would still pass every
/// `evaluate` case and would produce two arrays in the wrong order.
/// Exposed so the self-check can hold this list and the `operating_states` entry of `vocabulary.zig` to each
/// other: the same seven names live in both files, and nothing in the corpus compares them — each is measured
/// against the oracle separately, so two lists that agreed with the oracle at different moments would both
/// still pass while disagreeing with each other.
pub const states = operating_states;

const operating_states = [_][]const u8{
    "safe",
    "stopping",
    "post",
    "unhomed",
    "hold",
    "degraded",
    "running",
};

/// A derived state and the reason for it, before stickiness. `reason` is null only for a stop whose cause
/// this edition does not know — see `stoppingReason`.
const Derived = struct {
    state: []const u8,
    reason: ?[]const u8,
};

/// Whether a state is one §33.4's barrier may open at.
///
/// `stopping` is not one, and it is the interesting entry: a robot mid-ramp is the state that most looks like
/// it is about to be safe and most certainly is not. Swapping a driver during the ramp would change the
/// meaning of the deceleration in flight.
fn isSafeState(state: []const u8) bool {
    const eql = std.mem.eql;
    return eql(u8, state, "post") or eql(u8, state, "unhomed") or eql(u8, state, "hold") or
        eql(u8, state, "safe");
}

/// Whether a policy is commanding the body in this state.
///
/// `running` and `degraded`, and the second is the one that had been getting this wrong: §27's `DEGRADED` row
/// says the robot *runs*, reports, and refuses any package whose grants depend on the failed guard — it is the
/// impaired running state, not a third resting one. Which is why the degraded rule sits below the policy rules
/// in `derive`.
fn commandsAutonomously(state: []const u8) bool {
    return std.mem.eql(u8, state, "running") or std.mem.eql(u8, state, "degraded");
}

/// Whether a state requires an explicit human action to leave (§27). Exactly one does.
fn isSticky(state: []const u8) bool {
    return std.mem.eql(u8, state, "safe");
}

/// §49.1's tone, phrased over `commandsAutonomously` rather than over the literal `running`.
///
/// The under-inclusive version compared two strings to `"running"`, and `hold -> degraded` is a policy
/// beginning to command a body without being a transition into `running` — so the tone did not sound, and a
/// robot whose guard set is imperfect (§45.5's ordinary case) would have begun autonomous motion silently.
/// "Resumption after a stop" falls out of this spelling: `stopping` and `safe` command nothing, so leaving
/// either for a commanding state sounds.
fn warningRequired(from: []const u8, to: []const u8) bool {
    return commandsAutonomously(to) and !commandsAutonomously(from);
}

/// A clearance is a named human action. An empty name is not one — a caller that had nobody to name should
/// not have one.
fn cleared(conditions: ?*const Value) bool {
    const by = value.get(conditions, "cleared_by") orelse return false;
    return by.kind == .string and by.text.len > 0;
}

/// `(conditions.field ?? fallback) == literal`. A field that is absent, null or not a string takes the
/// fallback, which is the reference's `??`.
fn defaultedIs(conditions: ?*const Value, field: []const u8, fallback: []const u8, literal: []const u8) bool {
    const held = value.get(conditions, field);
    if (held != null and held.?.kind == .string) return std.mem.eql(u8, held.?.text, literal);
    return std.mem.eql(u8, fallback, literal);
}

/// `conditions.field === true`. Only a boolean true counts — a field carrying the string "true" is not a
/// guard asking for a hold.
fn strictlyTrue(conditions: ?*const Value, field: []const u8) bool {
    const held = value.get(conditions, field) orelse return false;
    return held.kind == .bool and held.truth;
}

/// The reason a stop is in progress, by cause.
///
/// A table rather than a composed string, so a cause nobody declared is *not* answered with a plausible
/// reason. The reference is a `switch` over a closed union with no default, which in TypeScript returns
/// `undefined` for a cause outside it — and rule 5 omits an undefined field. So this answers null and the
/// caller omits `reason` rather than inventing `stopping_<whatever>`, which would be this edition claiming a
/// vocabulary member that `corpus/vocabulary/closed-sets.json` does not hold. The corpus never asks: all
/// thirty-eight of its cases name a declared cause.
fn stoppingReason(cause: []const u8) ?[]const u8 {
    const table = [_]struct { cause: []const u8, reason: []const u8 }{
        .{ .cause = "watchdog", .reason = "stopping_watchdog" },
        .{ .cause = "proximity", .reason = "stopping_proximity" },
        .{ .cause = "balance", .reason = "stopping_balance" },
        .{ .cause = "operator", .reason = "stopping_operator" },
        .{ .cause = "estop", .reason = "stopping_estop" },
    };
    for (table) |row| {
        if (std.mem.eql(u8, cause, row.cause)) return row.reason;
    }
    return null;
}

/// The state and reason the conditions imply, before stickiness. The rule order *is* the specification; each
/// numbered comment is the §27 row it implements.
fn derive(conditions: ?*const Value) Derived {
    const stop = value.get(conditions, "stop");
    // Absent *and* null both mean no stop is commanded: the reference's `conditions.stop ?? null`.
    const stopping = stop != null and stop.?.kind == .string;

    // 1. The kernel lost the body. §27 puts this in the `SAFE` row beside a completed stop, and it is first
    //    because it is the condition under which nothing else can be known: a robot whose RHAL is gone has
    //    no observed joint state, no estop reading and no guard that can evaluate.
    if (defaultedIs(conditions, "rhal", "lost", "lost")) return .{ .state = "safe", .reason = "rhal_lost" };

    // 2. A failed self-test. §48.2: `SAFE`, not `DEGRADED`, "because a robot that failed its own self-test
    //    should not be talked into running by a reconcile loop".
    if (defaultedIs(conditions, "post", "incomplete", "failed")) {
        return .{ .state = "safe", .reason = "post_failed" };
    }

    // 3. A stop, complete or in progress. Above POST because an operator stop is honoured whatever the robot
    //    is doing, including booting.
    //
    //    `stop_settled` is read only here, which is deliberate: a robot that is not stopping has not
    //    completed a stop, and a caller leaving the flag true between stops would otherwise pin the machine
    //    in `SAFE`. For `estop` it is not observable on any edition that exists — §24 gives the hardware
    //    chain one bit, which says the chain is open and says nothing about whether the body has come to
    //    rest — so absent derives `stopping`, which can hold a robot and can never release one.
    if (stopping) {
        if (strictlyTrue(conditions, "stop_settled")) {
            return .{ .state = "safe", .reason = "stop_complete" };
        }
        return .{ .state = "stopping", .reason = stoppingReason(stop.?.text) };
    }

    // 4. The self-test has not established that this robot may run. Not a fault: §48.4's `post_incomplete`,
    //    which is a robot waiting on the rest of the operating system.
    if (!defaultedIs(conditions, "post", "incomplete", "passed")) {
        return .{ .state = "post", .reason = "post_incomplete" };
    }

    // 5. §48.1. Joint positions are unknown, forward kinematics is meaningless, and every pose-dependent
    //    guard is unavailable — so a policy must not run, and homing is the only motion permitted.
    if (defaultedIs(conditions, "homing", "pending", "pending")) {
        return .{ .state = "unhomed", .reason = "homing_pending" };
    }

    // 6. §27: "a robot with a policy and no kernel must not exist, so the runtime refuses to start if the
    //    kernel is not live". Refusing to start is `HOLD` — the safe resting state — rather than `SAFE`:
    //    nothing has gone wrong with the robot, and a sticky state needing a human would be the wrong answer
    //    for a kernel that is merely still starting.
    if (!defaultedIs(conditions, "kernel", "down", "live")) {
        return .{ .state = "hold", .reason = "kernel_down" };
    }

    // 7. §27's `HOLD` row, first half. A guard asking for a hold outranks a policy that would like to run,
    //    which is the whole point of a guard.
    if (strictlyTrue(conditions, "guard_hold")) return .{ .state = "hold", .reason = "guard_hold" };

    // 8. §27's `HOLD` row, second half: "no policy". Split in two, because "there is nothing to run" and
    //    "there is something and it has not got a lease" are different sentences and the second is the one
    //    with a remedy — §25.2's arbitration is where an operator goes to find out who else is holding the
    //    arm.
    if (defaultedIs(conditions, "policy", "none", "none")) return .{ .state = "hold", .reason = "no_policy" };
    if (!defaultedIs(conditions, "policy", "none", "ready")) {
        return .{ .state = "hold", .reason = "policy_not_ready" };
    }

    // 9. §27's `DEGRADED` row, below the policy rules deliberately: reaching here means a policy is ready to
    //    command, so `degraded` is the impaired running state rather than a resting one.
    if (strictlyTrue(conditions, "guards_unavailable")) {
        return .{ .state = "degraded", .reason = "guards_unavailable" };
    }
    if (defaultedIs(conditions, "rhal", "lost", "degraded")) {
        return .{ .state = "degraded", .reason = "rhal_degraded" };
    }

    return .{ .state = "running", .reason = "nominal" };
}

pub fn evaluateCase(a: Allocator, input: ?*const Value) anyerror!*const Value {
    // `previous` is a state name and is not validated against the list: the reference takes an
    // `OperatingState` and TypeScript erases it, so a caller passing a name nobody declared gets that name
    // back in `from` — and reproducing that is the requirement, not improving on it.
    const previous = value.get(input, "previous");
    const named = previous != null and previous.?.kind == .string;
    const from: []const u8 = if (named) previous.?.text else "";
    const conditions = value.get(input, "conditions");
    const held = derive(conditions);

    // §27's stickiness, applied after the derivation rather than instead of it. A robot in `safe` whose
    // conditions would put it back in `safe` reports the *condition's* reason rather than `safe_sticky`,
    // because "the estop is still pressed" is what an operator needs and "you have not cleared it yet" is
    // not.
    const stuck = named and std.mem.eql(u8, from, "safe") and !cleared(conditions) and
        !std.mem.eql(u8, held.state, "safe");
    const to: []const u8 = if (stuck) "safe" else held.state;
    const reason: ?[]const u8 = if (stuck) "safe_sticky" else held.reason;

    const out = try value.object(a);
    // A `previous` that is not a string is the reference's `undefined` arriving in `from`, which rule 5
    // omits — and `to !== undefined` is then true, so `changed` is true. The corpus never asks: all
    // thirty-eight of its cases name a previous state.
    if (named) try value.set(a, out, "from", previous.?);
    try value.set(a, out, "to", try value.string(a, to));
    // Omitted rather than written when the cause was unreadable: the reference's `undefined`, which rule 5
    // omits. See `stoppingReason`.
    if (reason) |held_reason| try value.set(a, out, "reason", try value.string(a, held_reason));
    // `changed` is `to !== previous`, named rather than left to the caller: the reconciler runs this every
    // pass and most passes change nothing, and a caller filtering black box events on a comparison it wrote
    // itself is a caller that will eventually write `!=` where it meant `==`.
    try value.set(a, out, "changed", value.boolean(!named or !std.mem.eql(u8, from, to)));
    // `commandsAutonomously` of an absent `from` is false, which is what a string comparison against
    // `undefined` gives in the reference.
    try value.set(a, out, "warning", value.boolean(commandsAutonomously(to) and
        !(named and commandsAutonomously(from))));
    return out;
}

pub fn propertiesCase(a: Allocator, input: ?*const Value) anyerror!*const Value {
    _ = input;
    const out = try value.array(a);
    for (operating_states) |state| {
        const row = try value.object(a);
        try value.set(a, row, "state", try value.string(a, state));
        try value.set(a, row, "safe_state", value.boolean(isSafeState(state)));
        try value.set(a, row, "commands", value.boolean(commandsAutonomously(state)));
        try value.set(a, row, "sticky", value.boolean(isSticky(state)));
        try value.push(a, out, row);
    }
    return out;
}

pub fn warningCase(a: Allocator, input: ?*const Value) anyerror!*const Value {
    _ = input;
    // Every ordered pair, forty-nine of them, so no edition can get one transition right by accident and the
    // rest wrong. The pairs where `from == to` are in the list too: a state re-entering itself never sounds,
    // and that is a fact rather than a gap.
    const out = try value.array(a);
    for (operating_states) |from| {
        for (operating_states) |to| {
            const row = try value.object(a);
            try value.set(a, row, "from", try value.string(a, from));
            try value.set(a, row, "to", try value.string(a, to));
            try value.set(a, row, "warning", value.boolean(warningRequired(from, to)));
            try value.push(a, out, row);
        }
    }
    return out;
}

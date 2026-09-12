// §27's operating-state machine — `reference/typescript/opstate.ts`.
//
// Pure, clockless and total: every combination of conditions produces a state, including combinations
// nobody anticipated, which is the property §33.3 asks of anything that converges.
//
// Three corpus files, and the two small ones are the interesting ones. `evaluate` is thirty-eight scripted
// conditions. `properties` freezes `safe_state`, `commands` and `sticky` for **every** state, and `warning`
// freezes the tone for **every ordered pair** of states — forty-nine of them — so an edition cannot get one
// transition right by accident and the rest wrong.

using System;

namespace Skynet.Os.Contracts;

internal static class OpState
{
    /// <summary>
    /// §27's five states plus §48.1's two, <b>ordered least capable first</b> — the order <c>Derive</c>
    /// descends through its rules, and the order <c>PropertiesCase</c> and <c>WarningCase</c> iterate in.
    /// Sorting this would still pass every <c>evaluate</c> case and would produce two arrays in the wrong
    /// order.
    /// </summary>
    internal static readonly string[] OperatingStates =
    {
        "safe", "stopping", "post", "unhomed", "hold", "degraded", "running",
    };

    /// <summary>
    /// A derived state and the reason for it, before stickiness. <c>Reason</c> is null only for a stop whose
    /// cause this edition does not know — see <c>StoppingReason</c>.
    /// </summary>
    private readonly record struct Derived(string State, string? Reason);

    /// <summary>
    /// Whether a state is one §33.4's barrier may open at.
    /// <para>
    /// <c>stopping</c> is not one, and it is the interesting entry: a robot mid-ramp is the state that most
    /// looks like it is about to be safe and most certainly is not. Swapping a driver during the ramp would
    /// change the meaning of the deceleration in flight.
    /// </para>
    /// </summary>
    internal static bool IsSafeState(string state) =>
        string.Equals(state, "post", StringComparison.Ordinal)
        || string.Equals(state, "unhomed", StringComparison.Ordinal)
        || string.Equals(state, "hold", StringComparison.Ordinal)
        || string.Equals(state, "safe", StringComparison.Ordinal);

    /// <summary>
    /// Whether a policy is commanding the body in this state.
    /// <para>
    /// <c>running</c> and <c>degraded</c>, and the second is the one that had been getting this wrong: §27's
    /// <c>DEGRADED</c> row says the robot <i>runs</i>, reports, and refuses any package whose grants depend
    /// on the failed guard — it is the impaired running state, not a third resting one. Which is why the
    /// degraded rule sits below the policy rules in <c>Derive</c>.
    /// </para>
    /// </summary>
    internal static bool CommandsAutonomously(string state) =>
        string.Equals(state, "running", StringComparison.Ordinal)
        || string.Equals(state, "degraded", StringComparison.Ordinal);

    /// <summary>Whether a state requires an explicit human action to leave (§27). Exactly one does.</summary>
    internal static bool IsSticky(string state) => string.Equals(state, "safe", StringComparison.Ordinal);

    /// <summary>
    /// §49.1's tone, phrased over <c>CommandsAutonomously</c> rather than over the literal <c>running</c>.
    /// <para>
    /// The under-inclusive version compared two strings to <c>"running"</c>, and <c>hold -&gt; degraded</c>
    /// is a policy beginning to command a body without being a transition into <c>running</c> — so the tone
    /// did not sound, and a robot whose guard set is imperfect (§45.5's ordinary case) would have begun
    /// autonomous motion silently. "Resumption after a stop" falls out of this spelling: <c>stopping</c> and
    /// <c>safe</c> command nothing, so leaving either for a commanding state sounds.
    /// </para>
    /// </summary>
    internal static bool WarningRequired(string from, string to) =>
        CommandsAutonomously(to) && !CommandsAutonomously(from);

    /// <summary>
    /// A clearance is a named human action. An empty name is not one — a caller that had nobody to name
    /// should not have one.
    /// </summary>
    private static bool Cleared(Value? conditions)
    {
        Value? by = Value.Get(conditions, "cleared_by");
        return by is not null && by.ValueKind == Kind.String
            && Value.AsString(by, string.Empty).Length != 0;
    }

    /// <summary>
    /// <c>(conditions.field ?? fallback) === literal</c>. A field that is absent, null or not a string takes
    /// the fallback, which is the reference's <c>??</c>.
    /// </summary>
    private static bool DefaultedIs(Value? conditions, string field, string fallback, string literal)
    {
        Value? held = Value.Get(conditions, field);
        if (held is not null && held.ValueKind == Kind.String)
        {
            return string.Equals(Value.AsString(held, string.Empty), literal, StringComparison.Ordinal);
        }
        return string.Equals(fallback, literal, StringComparison.Ordinal);
    }

    /// <summary>
    /// <c>conditions.field === true</c>. Only a boolean true counts — a field carrying the string "true" is
    /// not a guard asking for a hold.
    /// </summary>
    private static bool StrictlyTrue(Value? conditions, string field)
    {
        Value? held = Value.Get(conditions, field);
        return held is not null && held.ValueKind == Kind.Bool && Value.AsBool(held, false);
    }

    /// <summary>
    /// The reason a stop is in progress, by cause.
    /// <para>
    /// A table rather than a composed string, so a cause nobody declared is <i>not</i> answered with a
    /// plausible reason. The reference is a <c>switch</c> over a closed union with no default, which in
    /// TypeScript returns <c>undefined</c> for a cause outside it — and rule 5 omits an undefined field. So
    /// this returns null and the caller omits <c>reason</c> rather than inventing
    /// <c>stopping_&lt;whatever&gt;</c>, which would be this edition claiming a vocabulary member that
    /// <c>corpus/vocabulary/closed-sets.json</c> does not hold. The corpus never asks: all thirty-eight of
    /// its cases name a declared cause.
    /// </para>
    /// </summary>
    private static string? StoppingReason(string cause) => cause switch
    {
        "watchdog" => "stopping_watchdog",
        "proximity" => "stopping_proximity",
        "balance" => "stopping_balance",
        "operator" => "stopping_operator",
        "estop" => "stopping_estop",
        _ => null,
    };

    /// <summary>
    /// The state and reason the conditions imply, before stickiness. The rule order <i>is</i> the
    /// specification; each numbered comment is the §27 row it implements.
    /// </summary>
    private static Derived Derive(Value? conditions)
    {
        Value? stop = Value.Get(conditions, "stop");
        // Absent *and* null both mean no stop is commanded: the reference's `conditions.stop ?? null`.
        bool stopping = stop is not null && stop.ValueKind == Kind.String;

        // 1. The kernel lost the body. §27 puts this in the `SAFE` row beside a completed stop, and it is
        //    first because it is the condition under which nothing else can be known: a robot whose RHAL is
        //    gone has no observed joint state, no estop reading and no guard that can evaluate.
        if (DefaultedIs(conditions, "rhal", "lost", "lost"))
        {
            return new Derived("safe", "rhal_lost");
        }

        // 2. A failed self-test. §48.2: `SAFE`, not `DEGRADED`, "because a robot that failed its own
        //    self-test should not be talked into running by a reconcile loop".
        if (DefaultedIs(conditions, "post", "incomplete", "failed"))
        {
            return new Derived("safe", "post_failed");
        }

        // 3. A stop, complete or in progress. Above POST because an operator stop is honoured whatever the
        //    robot is doing, including booting.
        //
        //    `stop_settled` is read only here, which is deliberate: a robot that is not stopping has not
        //    completed a stop, and a caller leaving the flag true between stops would otherwise pin the
        //    machine in `SAFE`. For `estop` it is not observable on any edition that exists — §24 gives the
        //    hardware chain one bit, which says the chain is open and says nothing about whether the body
        //    has come to rest — so absent derives `stopping`, which can hold a robot and never release one.
        if (stopping)
        {
            if (StrictlyTrue(conditions, "stop_settled"))
            {
                return new Derived("safe", "stop_complete");
            }
            return new Derived("stopping", StoppingReason(Value.AsString(stop, string.Empty)));
        }

        // 4. The self-test has not established that this robot may run. Not a fault: §48.4's
        //    `post_incomplete`, which is a robot waiting on the rest of the operating system.
        if (!DefaultedIs(conditions, "post", "incomplete", "passed"))
        {
            return new Derived("post", "post_incomplete");
        }

        // 5. §48.1. Joint positions are unknown, forward kinematics is meaningless, and every pose-dependent
        //    guard is unavailable — so a policy must not run, and homing is the only motion permitted.
        if (DefaultedIs(conditions, "homing", "pending", "pending"))
        {
            return new Derived("unhomed", "homing_pending");
        }

        // 6. §27: "a robot with a policy and no kernel must not exist, so the runtime refuses to start if
        //    the kernel is not live". Refusing to start is `HOLD` — the safe resting state — rather than
        //    `SAFE`: nothing has gone wrong with the robot, and a sticky state needing a human would be the
        //    wrong answer for a kernel that is merely still starting.
        if (!DefaultedIs(conditions, "kernel", "down", "live"))
        {
            return new Derived("hold", "kernel_down");
        }

        // 7. §27's `HOLD` row, first half. A guard asking for a hold outranks a policy that would like to
        //    run, which is the whole point of a guard.
        if (StrictlyTrue(conditions, "guard_hold"))
        {
            return new Derived("hold", "guard_hold");
        }

        // 8. §27's `HOLD` row, second half: "no policy". Split in two, because "there is nothing to run" and
        //    "there is something and it has not got a lease" are different sentences and the second is the
        //    one with a remedy — §25.2's arbitration is where an operator goes to find out who else is
        //    holding the arm.
        if (DefaultedIs(conditions, "policy", "none", "none"))
        {
            return new Derived("hold", "no_policy");
        }
        if (!DefaultedIs(conditions, "policy", "none", "ready"))
        {
            return new Derived("hold", "policy_not_ready");
        }

        // 9. §27's `DEGRADED` row, below the policy rules deliberately: reaching here means a policy is
        //    ready to command, so `degraded` is the impaired running state rather than a resting one.
        if (StrictlyTrue(conditions, "guards_unavailable"))
        {
            return new Derived("degraded", "guards_unavailable");
        }
        if (DefaultedIs(conditions, "rhal", "lost", "degraded"))
        {
            return new Derived("degraded", "rhal_degraded");
        }

        return new Derived("running", "nominal");
    }

    internal static Value EvaluateCase(Value? input)
    {
        // `previous` is a state name and is not validated against the list: the reference takes an
        // `OperatingState` and TypeScript erases it, so a caller passing a name nobody declared gets that
        // name back in `from` — and reproducing that is the requirement, not improving on it.
        Value? previous = Value.Get(input, "previous");
        bool named = previous is not null && previous.ValueKind == Kind.String;
        string from = named ? Value.AsString(previous, string.Empty) : string.Empty;
        Value? conditions = Value.Get(input, "conditions");
        Derived held = Derive(conditions);

        // §27's stickiness, applied after the derivation rather than instead of it. A robot in `safe` whose
        // conditions would put it back in `safe` reports the *condition's* reason rather than `safe_sticky`,
        // because "the estop is still pressed" is what an operator needs and "you have not cleared it yet"
        // is not.
        bool stuck = named && string.Equals(from, "safe", StringComparison.Ordinal) && !Cleared(conditions)
            && !string.Equals(held.State, "safe", StringComparison.Ordinal);
        string to = stuck ? "safe" : held.State;
        string? reason = stuck ? "safe_sticky" : held.Reason;

        Value out_ = Value.Object();
        // A `previous` that is not a string is the reference's `undefined` arriving in `from`, which rule 5
        // omits — and `to !== undefined` is then true, so `changed` is true. The corpus never asks: all
        // thirty-eight of its cases name a previous state.
        if (named)
        {
            out_.Set("from", previous!);
        }
        out_.Set("to", Value.Of(to));
        // Omitted rather than written when the cause was unreadable: the reference's `undefined`, which rule
        // 5 omits. See `StoppingReason`.
        if (reason is not null)
        {
            out_.Set("reason", Value.Of(reason));
        }
        // `changed` is `to !== previous`, named rather than left to the caller: the reconciler runs this
        // every pass and most passes change nothing, and a caller filtering black box events on a comparison
        // it wrote itself is a caller that will eventually write `!=` where it meant `==`.
        out_.Set("changed", Value.Of(!named || !string.Equals(from, to, StringComparison.Ordinal)));
        // `CommandsAutonomously` of an absent `from` is false, which is what a string comparison against
        // `undefined` gives in the reference.
        out_.Set("warning", Value.Of(CommandsAutonomously(to) && !(named && CommandsAutonomously(from))));
        return out_;
    }

    internal static Value PropertiesCase(Value? input)
    {
        _ = input;
        Value out_ = Value.Array();
        foreach (string state in OperatingStates)
        {
            out_.Push(Value.Object()
                .Set("state", Value.Of(state))
                .Set("safe_state", Value.Of(IsSafeState(state)))
                .Set("commands", Value.Of(CommandsAutonomously(state)))
                .Set("sticky", Value.Of(IsSticky(state))));
        }
        return out_;
    }

    internal static Value WarningCase(Value? input)
    {
        _ = input;
        // Every ordered pair, forty-nine of them, so no edition can get one transition right by accident and
        // the rest wrong. The pairs where `from == to` are in the list too: a state re-entering itself never
        // sounds, and that is a fact rather than a gap.
        Value out_ = Value.Array();
        foreach (string from in OperatingStates)
        {
            foreach (string to in OperatingStates)
            {
                out_.Push(Value.Object()
                    .Set("from", Value.Of(from))
                    .Set("to", Value.Of(to))
                    .Set("warning", Value.Of(WarningRequired(from, to))));
            }
        }
        return out_;
    }
}

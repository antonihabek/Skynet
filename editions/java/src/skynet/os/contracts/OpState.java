// §27's operating-state machine — `reference/typescript/opstate.ts`.
//
// Pure, clockless and total: every combination of conditions produces a state, including combinations
// nobody anticipated, which is the property §33.3 asks of anything that converges.
//
// Three corpus files, and the two small ones are the interesting ones. `evaluate` is thirty-eight scripted
// conditions. `properties` freezes `safe_state`, `commands` and `sticky` for **every** state, and `warning`
// freezes the tone for **every ordered pair** of states — forty-nine of them — so an edition cannot get one
// transition right by accident and the rest wrong.

package skynet.os.contracts;

import skynet.os.Value;

public final class OpState {
    private OpState() {}

    /**
     * §27's five states plus §48.1's two, <b>ordered least capable first</b> — the order {@code derive}
     * descends through its rules, and the order {@code properties} and {@code warning} iterate in. Sorting
     * this would still pass every {@code evaluate} case and would produce two arrays in the wrong order.
     *
     * <p>Public so {@code SelfCheck} can hold this list and the {@code operating_states} entry of
     * {@code Vocabulary} to each other: the same seven names live in both classes, and nothing in the corpus
     * compares them — each is measured against the oracle separately, so two lists that agreed with the
     * oracle at different moments would both still pass while disagreeing with each other.
     */
    public static final String[] OPERATING_STATES = {
        "safe", "stopping", "post", "unhomed", "hold", "degraded", "running",
    };

    /**
     * A derived state and the reason for it, before stickiness. {@code reason} is null only for a stop whose
     * cause this edition does not know — see {@code stoppingReason}.
     */
    private record Derived(String state, String reason) {}

    /**
     * Whether a state is one §33.4's barrier may open at.
     *
     * <p>{@code stopping} is not one, and it is the interesting entry: a robot mid-ramp is the state that
     * most looks like it is about to be safe and most certainly is not. Swapping a driver during the ramp
     * would change the meaning of the deceleration in flight.
     */
    public static boolean isSafeState(String state) {
        return state.equals("post") || state.equals("unhomed") || state.equals("hold")
                || state.equals("safe");
    }

    /**
     * Whether a policy is commanding the body in this state.
     *
     * <p>{@code running} and {@code degraded}, and the second is the one that had been getting this wrong:
     * §27's {@code DEGRADED} row says the robot <i>runs</i>, reports, and refuses any package whose grants
     * depend on the failed guard — it is the impaired running state, not a third resting one. Which is why
     * the degraded rule sits below the policy rules in {@code derive}.
     */
    public static boolean commandsAutonomously(String state) {
        return state.equals("running") || state.equals("degraded");
    }

    /** Whether a state requires an explicit human action to leave (§27). Exactly one does. */
    public static boolean isSticky(String state) {
        return state.equals("safe");
    }

    /**
     * §49.1's tone, phrased over {@code commandsAutonomously} rather than over the literal {@code running}.
     *
     * <p>The under-inclusive version compared two strings to {@code "running"}, and
     * {@code hold -> degraded} is a policy beginning to command a body without being a transition into
     * {@code running} — so the tone did not sound, and a robot whose guard set is imperfect (§45.5's
     * ordinary case) would have begun autonomous motion silently. "Resumption after a stop" falls out of
     * this spelling: {@code stopping} and {@code safe} command nothing, so leaving either for a commanding
     * state sounds.
     */
    public static boolean warningRequired(String from, String to) {
        return commandsAutonomously(to) && !commandsAutonomously(from);
    }

    /**
     * A clearance is a named human action. An empty name is not one — a caller that had nobody to name
     * should not have one.
     */
    private static boolean cleared(Value conditions) {
        Value by = Value.get(conditions, "cleared_by");
        return by != null && by.kind() == Value.Kind.STRING && !Value.asString(by, "").isEmpty();
    }

    /**
     * {@code (conditions.field ?? fallback) === literal}. A field that is absent, null or not a string takes
     * the fallback, which is the reference's {@code ??}.
     */
    private static boolean defaultedIs(Value conditions, String field, String fallback, String literal) {
        Value held = Value.get(conditions, field);
        if (held != null && held.kind() == Value.Kind.STRING) return Value.asString(held, "").equals(literal);
        return fallback.equals(literal);
    }

    /**
     * {@code conditions.field === true}. Only a boolean true counts — a field carrying the string "true" is
     * not a guard asking for a hold.
     */
    private static boolean strictlyTrue(Value conditions, String field) {
        Value held = Value.get(conditions, field);
        return held != null && held.kind() == Value.Kind.BOOL && Value.asBool(held, false);
    }

    /**
     * The reason a stop is in progress, by cause.
     *
     * <p>A table rather than a composed string, so a cause nobody declared is <i>not</i> answered with a
     * plausible reason. The reference is a {@code switch} over a closed union with no default, which in
     * TypeScript returns {@code undefined} for a cause outside it — and rule 5 omits an undefined field. So
     * this returns null and the caller omits {@code reason} rather than inventing
     * {@code stopping_<whatever>}, which would be this edition claiming a vocabulary member that
     * {@code corpus/vocabulary/closed-sets.json} does not hold. The corpus never asks: all thirty-eight of
     * its cases name a declared cause.
     */
    private static String stoppingReason(String cause) {
        return switch (cause) {
            case "watchdog" -> "stopping_watchdog";
            case "proximity" -> "stopping_proximity";
            case "balance" -> "stopping_balance";
            case "operator" -> "stopping_operator";
            case "estop" -> "stopping_estop";
            default -> null;
        };
    }

    /**
     * The state and reason the conditions imply, before stickiness. The rule order <i>is</i> the
     * specification; each numbered comment is the §27 row it implements.
     */
    private static Derived derive(Value conditions) {
        Value stop = Value.get(conditions, "stop");
        // Absent *and* null both mean no stop is commanded: the reference's `conditions.stop ?? null`.
        boolean stopping = stop != null && stop.kind() == Value.Kind.STRING;

        // 1. The kernel lost the body. §27 puts this in the `SAFE` row beside a completed stop, and it is
        //    first because it is the condition under which nothing else can be known: a robot whose RHAL is
        //    gone has no observed joint state, no estop reading and no guard that can evaluate.
        if (defaultedIs(conditions, "rhal", "lost", "lost")) return new Derived("safe", "rhal_lost");

        // 2. A failed self-test. §48.2: `SAFE`, not `DEGRADED`, "because a robot that failed its own
        //    self-test should not be talked into running by a reconcile loop".
        if (defaultedIs(conditions, "post", "incomplete", "failed")) {
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
        if (stopping) {
            if (strictlyTrue(conditions, "stop_settled")) return new Derived("safe", "stop_complete");
            return new Derived("stopping", stoppingReason(Value.asString(stop, "")));
        }

        // 4. The self-test has not established that this robot may run. Not a fault: §48.4's
        //    `post_incomplete`, which is a robot waiting on the rest of the operating system.
        if (!defaultedIs(conditions, "post", "incomplete", "passed")) {
            return new Derived("post", "post_incomplete");
        }

        // 5. §48.1. Joint positions are unknown, forward kinematics is meaningless, and every pose-dependent
        //    guard is unavailable — so a policy must not run, and homing is the only motion permitted.
        if (defaultedIs(conditions, "homing", "pending", "pending")) {
            return new Derived("unhomed", "homing_pending");
        }

        // 6. §27: "a robot with a policy and no kernel must not exist, so the runtime refuses to start if
        //    the kernel is not live". Refusing to start is `HOLD` — the safe resting state — rather than
        //    `SAFE`: nothing has gone wrong with the robot, and a sticky state needing a human would be the
        //    wrong answer for a kernel that is merely still starting.
        if (!defaultedIs(conditions, "kernel", "down", "live")) {
            return new Derived("hold", "kernel_down");
        }

        // 7. §27's `HOLD` row, first half. A guard asking for a hold outranks a policy that would like to
        //    run, which is the whole point of a guard.
        if (strictlyTrue(conditions, "guard_hold")) return new Derived("hold", "guard_hold");

        // 8. §27's `HOLD` row, second half: "no policy". Split in two, because "there is nothing to run" and
        //    "there is something and it has not got a lease" are different sentences and the second is the
        //    one with a remedy — §25.2's arbitration is where an operator goes to find out who else is
        //    holding the arm.
        if (defaultedIs(conditions, "policy", "none", "none")) return new Derived("hold", "no_policy");
        if (!defaultedIs(conditions, "policy", "none", "ready")) {
            return new Derived("hold", "policy_not_ready");
        }

        // 9. §27's `DEGRADED` row, below the policy rules deliberately: reaching here means a policy is
        //    ready to command, so `degraded` is the impaired running state rather than a resting one.
        if (strictlyTrue(conditions, "guards_unavailable")) {
            return new Derived("degraded", "guards_unavailable");
        }
        if (defaultedIs(conditions, "rhal", "lost", "degraded")) {
            return new Derived("degraded", "rhal_degraded");
        }

        return new Derived("running", "nominal");
    }

    public static Value evaluateCase(Value input) {
        // `previous` is a state name and is not validated against the list: the reference takes an
        // `OperatingState` and TypeScript erases it, so a caller passing a name nobody declared gets that
        // name back in `from` — and reproducing that is the requirement, not improving on it.
        Value previous = Value.get(input, "previous");
        boolean named = previous != null && previous.kind() == Value.Kind.STRING;
        String from = named ? Value.asString(previous, "") : "";
        Value conditions = Value.get(input, "conditions");
        Derived held = derive(conditions);

        // §27's stickiness, applied after the derivation rather than instead of it. A robot in `safe` whose
        // conditions would put it back in `safe` reports the *condition's* reason rather than `safe_sticky`,
        // because "the estop is still pressed" is what an operator needs and "you have not cleared it yet"
        // is not.
        boolean stuck = named && from.equals("safe") && !cleared(conditions)
                && !held.state().equals("safe");
        String to = stuck ? "safe" : held.state();
        String reason = stuck ? "safe_sticky" : held.reason();

        Value out = Value.object();
        // A `previous` that is not a string is the reference's `undefined` arriving in `from`, which rule 5
        // omits — and `to !== undefined` is then true, so `changed` is true. The corpus never asks: all
        // thirty-eight of its cases name a previous state.
        if (named) out.set("from", previous);
        out.set("to", Value.of(to));
        // Omitted rather than written when the cause was unreadable: the reference's `undefined`, which rule
        // 5 omits. See `stoppingReason`.
        if (reason != null) out.set("reason", Value.of(reason));
        // `changed` is `to !== previous`, named rather than left to the caller: the reconciler runs this
        // every pass and most passes change nothing, and a caller filtering black box events on a comparison
        // it wrote itself is a caller that will eventually write `!=` where it meant `==`.
        out.set("changed", Value.of(!named || !from.equals(to)));
        // `commandsAutonomously` of an absent `from` is false, which is what a string comparison against
        // `undefined` gives in the reference.
        out.set("warning", Value.of(commandsAutonomously(to) && !(named && commandsAutonomously(from))));
        return out;
    }

    public static Value propertiesCase(Value input) {
        Value out = Value.array();
        for (String state : OPERATING_STATES) {
            out.push(Value.object()
                    .set("state", Value.of(state))
                    .set("safe_state", Value.of(isSafeState(state)))
                    .set("commands", Value.of(commandsAutonomously(state)))
                    .set("sticky", Value.of(isSticky(state))));
        }
        return out;
    }

    public static Value warningCase(Value input) {
        // Every ordered pair, forty-nine of them, so no edition can get one transition right by accident and
        // the rest wrong. The pairs where `from == to` are in the list too: a state re-entering itself never
        // sounds, and that is a fact rather than a gap.
        Value out = Value.array();
        for (String from : OPERATING_STATES) {
            for (String to : OPERATING_STATES) {
                out.push(Value.object()
                        .set("from", Value.of(from))
                        .set("to", Value.of(to))
                        .set("warning", Value.of(warningRequired(from, to))));
            }
        }
        return out;
    }
}

#include "skynet_os/opstate.h"

#include <string.h>

/*
 * §27's five states plus §48.1's two, **ordered least capable first** — the order `derive` descends through
 * its rules, and the order `properties` and `warning` iterate in. Sorting this would still pass every
 * `evaluate` case and would produce two arrays in the wrong order, which is why the order is written down
 * here rather than left to whoever reads the list.
 */
static const char *const OPERATING_STATES[] = {
    "safe", "stopping", "post", "unhomed", "hold", "degraded", "running",
};
#define SK_STATE_COUNT (sizeof OPERATING_STATES / sizeof OPERATING_STATES[0])

const char *const *sk_opstate_states(size_t *count)
{
    *count = SK_STATE_COUNT;
    return OPERATING_STATES;
}

/* A derived state and the reason for it, before stickiness. `reason` is NULL only for a stop whose cause
 * this edition does not know — see `stopping_reason`. */
typedef struct {
    const char *state;
    const char *reason;
} derived;

/* Whether a state is one §33.4's barrier may open at.
 *
 * `stopping` is not one, and it is the interesting entry: a robot mid-ramp is the state that most looks
 * like it is about to be safe and most certainly is not. Swapping a driver during the ramp would change
 * the meaning of the deceleration in flight. */
static bool is_safe_state(const char *state)
{
    return strcmp(state, "post") == 0 || strcmp(state, "unhomed") == 0
           || strcmp(state, "hold") == 0 || strcmp(state, "safe") == 0;
}

/* Whether a policy is commanding the body in this state.
 *
 * `running` and `degraded`, and the second is the one that had been getting this wrong: §27's `DEGRADED`
 * row says the robot *runs*, reports, and refuses any package whose grants depend on the failed guard — it
 * is the impaired running state, not a third resting one. Which is why the degraded rule sits below the
 * policy rules in `derive`. */
static bool commands_autonomously(const char *state)
{
    return strcmp(state, "running") == 0 || strcmp(state, "degraded") == 0;
}

/* Whether a state requires an explicit human action to leave (§27). Exactly one does. */
static bool is_sticky(const char *state)
{
    return strcmp(state, "safe") == 0;
}

/* §49.1's tone, phrased over `commands_autonomously` rather than over the literal `running`.
 *
 * The under-inclusive version compared two strings to `"running"`, and `hold -> degraded` is a policy
 * beginning to command a body without being a transition into `running` — so the tone did not sound, and a
 * robot whose guard set is imperfect (§45.5's ordinary case) would have begun autonomous motion silently.
 * "Resumption after a stop" falls out of this spelling: `stopping` and `safe` command nothing, so leaving
 * either for a commanding state sounds. */
static bool warning_required(const char *from, const char *to)
{
    return commands_autonomously(to) && !commands_autonomously(from);
}

/* A clearance is a named human action. An empty name is not one — a caller that had nobody to name should
 * not have one. */
static bool cleared(const sk_value *conditions)
{
    const sk_value *by = sk_get(conditions, "cleared_by");
    if (sk_kind_of(by) != SK_STRING) {
        return false;
    }
    size_t length = 0;
    (void) sk_as_string(by, &length, "");
    return length > 0;
}

/*
 * `(conditions.field ?? fallback) === literal`.
 *
 * One helper rather than a `string_or` returning `const char *`, because a string in this edition carries
 * an explicit length and may hold U+0000 — so `strcmp` over its bytes is not the reference's `===`. Every
 * comparison here goes through `sk_string_is`, which compares the length too. A field that is absent, null
 * or not a string takes the fallback, which is the reference's `??` and is why the fallback is compared as
 * a C literal: this edition's own tokens hold no NUL.
 */
static bool defaulted_is(const sk_value *conditions, const char *field, const char *fallback,
                         const char *literal)
{
    const sk_value *held = sk_get(conditions, field);
    if (sk_kind_of(held) == SK_STRING) {
        return sk_string_is(held, literal);
    }
    return strcmp(fallback, literal) == 0;
}

/* `conditions.field === true`. Only a boolean true counts, which is what the reference's `=== true`
 * says — a field carrying the string "true" is not a guard asking for a hold. */
static bool strictly_true(const sk_value *conditions, const char *field)
{
    const sk_value *held = sk_get(conditions, field);
    return sk_kind_of(held) == SK_BOOL && sk_as_bool(held, false);
}

/*
 * The reason a stop is in progress, by cause.
 *
 * A table rather than a composed string, so a cause nobody declared is *not* answered with a plausible
 * reason. The reference is a `switch` over a closed union with no default, which in TypeScript returns
 * `undefined` for a cause outside it — and rule 5 omits an undefined field. So this returns NULL and the
 * caller omits `reason` rather than inventing `stopping_<whatever>`, which would be this edition claiming
 * a vocabulary member that `corpus/vocabulary/closed-sets.json` does not hold. The corpus never asks: all
 * thirty-eight of its cases name a declared cause.
 */
static const char *stopping_reason(const sk_value *cause)
{
    static const struct {
        const char *cause;
        const char *reason;
    } REASONS[] = {
        {"watchdog", "stopping_watchdog"},
        {"proximity", "stopping_proximity"},
        {"balance", "stopping_balance"},
        {"operator", "stopping_operator"},
        {"estop", "stopping_estop"},
    };
    for (size_t at = 0; at < sizeof REASONS / sizeof REASONS[0]; at += 1) {
        if (sk_string_is(cause, REASONS[at].cause)) {
            return REASONS[at].reason;
        }
    }
    return NULL;
}

/* The state and reason the conditions imply, before stickiness. The rule order *is* the specification;
 * each numbered comment is the §27 row it implements. */
static derived derive(const sk_value *conditions)
{
    const sk_value *stop = sk_get(conditions, "stop");
    /* Absent *and* null both mean no stop is commanded: the reference's `conditions.stop ?? null`. */
    bool stopping = sk_kind_of(stop) == SK_STRING;

    /* 1. The kernel lost the body. §27 puts this in the `SAFE` row beside a completed stop, and it is
     *    first because it is the condition under which nothing else can be known: a robot whose RHAL is
     *    gone has no observed joint state, no estop reading and no guard that can evaluate. */
    if (defaulted_is(conditions, "rhal", "lost", "lost")) {
        return (derived){"safe", "rhal_lost"};
    }

    /* 2. A failed self-test. §48.2: `SAFE`, not `DEGRADED`, "because a robot that failed its own
     *    self-test should not be talked into running by a reconcile loop". */
    if (defaulted_is(conditions, "post", "incomplete", "failed")) {
        return (derived){"safe", "post_failed"};
    }

    /* 3. A stop, complete or in progress. Above POST because an operator stop is honoured whatever the
     *    robot is doing, including booting.
     *
     *    `stop_settled` is read only here, which is deliberate: a robot that is not stopping has not
     *    completed a stop, and a caller leaving the flag true between stops would otherwise pin the
     *    machine in `SAFE`. For `estop` it is not observable on any edition that exists — §24 gives the
     *    hardware chain one bit, which says the chain is open and says nothing about whether the body has
     *    come to rest — so absent derives `stopping`, which can hold a robot and can never release one. */
    if (stopping) {
        if (strictly_true(conditions, "stop_settled")) {
            return (derived){"safe", "stop_complete"};
        }
        return (derived){"stopping", stopping_reason(stop)};
    }

    /* 4. The self-test has not established that this robot may run. Not a fault: §48.4's
     *    `post_incomplete`, which is a robot waiting on the rest of the operating system. */
    if (!defaulted_is(conditions, "post", "incomplete", "passed")) {
        return (derived){"post", "post_incomplete"};
    }

    /* 5. §48.1. Joint positions are unknown, forward kinematics is meaningless, and every pose-dependent
     *    guard is unavailable — so a policy must not run, and homing is the only motion permitted. */
    if (defaulted_is(conditions, "homing", "pending", "pending")) {
        return (derived){"unhomed", "homing_pending"};
    }

    /* 6. §27: "a robot with a policy and no kernel must not exist, so the runtime refuses to start if the
     *    kernel is not live". Refusing to start is `HOLD` — the safe resting state — rather than `SAFE`:
     *    nothing has gone wrong with the robot, and a sticky state needing a human would be the wrong
     *    answer for a kernel that is merely still starting. */
    if (!defaulted_is(conditions, "kernel", "down", "live")) {
        return (derived){"hold", "kernel_down"};
    }

    /* 7. §27's `HOLD` row, first half. A guard asking for a hold outranks a policy that would like to
     *    run, which is the whole point of a guard. */
    if (strictly_true(conditions, "guard_hold")) {
        return (derived){"hold", "guard_hold"};
    }

    /* 8. §27's `HOLD` row, second half: "no policy". Split in two, because "there is nothing to run" and
     *    "there is something and it has not got a lease" are different sentences and the second is the one
     *    with a remedy — §25.2's arbitration is where an operator goes to find out who else is holding
     *    the arm. */
    if (defaulted_is(conditions, "policy", "none", "none")) {
        return (derived){"hold", "no_policy"};
    }
    if (!defaulted_is(conditions, "policy", "none", "ready")) {
        return (derived){"hold", "policy_not_ready"};
    }

    /* 9. §27's `DEGRADED` row, below the policy rules deliberately: reaching here means a policy is ready
     *    to command, so `degraded` is the impaired running state rather than a resting one. */
    if (strictly_true(conditions, "guards_unavailable")) {
        return (derived){"degraded", "guards_unavailable"};
    }
    if (defaulted_is(conditions, "rhal", "lost", "degraded")) {
        return (derived){"degraded", "rhal_degraded"};
    }

    return (derived){"running", "nominal"};
}

const sk_value *sk_opstate_evaluate_case(sk_arena *arena, const sk_value *input, const char **error)
{
    (void) error;
    /* `previous` is a state name and is not validated against the list: the reference takes an
     * `OperatingState` and TypeScript erases it, so a caller passing a name nobody declared gets that name
     * back in `from` — and reproducing that is the requirement, not improving on it. Held as a value rather
     * than as a C string so that `from` echoes exactly what arrived, NULs and all. */
    const sk_value *previous = sk_get(input, "previous");
    bool named = sk_kind_of(previous) == SK_STRING;
    const sk_value *conditions = sk_get(input, "conditions");
    derived held = derive(conditions);

    /* §27's stickiness, applied after the derivation rather than instead of it. A robot in `safe` whose
     * conditions would put it back in `safe` reports the *condition's* reason rather than `safe_sticky`,
     * because "the estop is still pressed" is what an operator needs and "you have not cleared it yet"
     * is not. */
    bool stuck = named && sk_string_is(previous, "safe") && !cleared(conditions)
                 && strcmp(held.state, "safe") != 0;
    const char *to = stuck ? "safe" : held.state;
    const char *reason = stuck ? "safe_sticky" : held.reason;

    sk_value *out = sk_object(arena);
    /* A `previous` that is not a string is the reference's `undefined` arriving in `from`, which rule 5
     * omits — and `to !== undefined` is then true, so `changed` is true. The corpus never asks: all
     * thirty-eight of its cases name a previous state. */
    if (named) {
        sk_set(arena, out, "from", previous);
    }
    sk_set(arena, out, "to", sk_string(arena, to));
    /* Omitted rather than written when the cause was unreadable: the reference's `undefined`, which rule 5
     * omits. See `stopping_reason`. */
    if (reason != NULL) {
        sk_set(arena, out, "reason", sk_string(arena, reason));
    }
    /* `changed` is `to !== previous`, named rather than left to the caller: the reconciler runs this every
     * pass and most passes change nothing, and a caller filtering black box events on a comparison it
     * wrote itself is a caller that will eventually write `!=` where it meant `==`. */
    sk_set(arena, out, "changed", sk_bool(!named || !sk_string_is(previous, to)));
    /* `commands_autonomously` of an absent `from` is false, which is what a string comparison against
     * `undefined` gives in the reference. */
    sk_set(arena, out, "warning",
           sk_bool(commands_autonomously(to)
                   && !(named && (sk_string_is(previous, "running") || sk_string_is(previous, "degraded")))));
    return out;
}

const sk_value *sk_opstate_properties_case(sk_arena *arena, const sk_value *input, const char **error)
{
    (void) input;
    (void) error;
    sk_value *out = sk_array(arena);
    for (size_t at = 0; at < SK_STATE_COUNT; at += 1) {
        const char *state = OPERATING_STATES[at];
        sk_value *row = sk_object(arena);
        sk_set(arena, row, "state", sk_string(arena, state));
        sk_set(arena, row, "safe_state", sk_bool(is_safe_state(state)));
        sk_set(arena, row, "commands", sk_bool(commands_autonomously(state)));
        sk_set(arena, row, "sticky", sk_bool(is_sticky(state)));
        sk_push(arena, out, row);
    }
    return out;
}

const sk_value *sk_opstate_warning_case(sk_arena *arena, const sk_value *input, const char **error)
{
    (void) input;
    (void) error;
    /* Every ordered pair, forty-nine of them, so no edition can get one transition right by accident and
     * the rest wrong. The pairs where `from == to` are in the list too: a state re-entering itself never
     * sounds, and that is a fact rather than a gap. */
    sk_value *out = sk_array(arena);
    for (size_t from = 0; from < SK_STATE_COUNT; from += 1) {
        for (size_t to = 0; to < SK_STATE_COUNT; to += 1) {
            sk_value *row = sk_object(arena);
            sk_set(arena, row, "from", sk_string(arena, OPERATING_STATES[from]));
            sk_set(arena, row, "to", sk_string(arena, OPERATING_STATES[to]));
            sk_set(arena, row, "warning",
                   sk_bool(warning_required(OPERATING_STATES[from], OPERATING_STATES[to])));
            sk_push(arena, out, row);
        }
    }
    return out;
}

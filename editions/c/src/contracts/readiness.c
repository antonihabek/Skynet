#include "skynet_os/readiness.h"

#include <string.h>

#include "skynet_os/js.h"
#include "skynet_os/write.h"

/* Tiers are cumulative, and this order is what makes the walk below mean anything. */
static const char *const TIER_ORDER[] = {"guest", "host", "certified"};
#define SK_TIER_COUNT (sizeof TIER_ORDER / sizeof TIER_ORDER[0])

static const char *const READINESS_CHECKS[] = {
    "adapter",     "kinematics",     "control_modes", "control_rate",
    "envelope",    "envelope_source", "watchdog",      "joint_ratings",
    "estop_software", "estop_hardware", "onboard_compute",
};
#define SK_CHECK_COUNT (sizeof READINESS_CHECKS / sizeof READINESS_CHECKS[0])

typedef struct {
    const char *check;
    bool ok;
    /* The lowest tier that needs this. A failed `guest` check blocks everything above it too. */
    const char *required_for;
    /* English, for logs and API clients. A UI translates by `check`. */
    const char *detail;
} check_result;

/* `caps.has(...)` over a declared capability list. A list that is absent or is not a list holds
 * nothing, which is the reference's `new Set(subject.capabilities ?? [])`. */
static bool has_capability(const sk_value *capabilities, const char *wanted)
{
    for (size_t at = 0; at < sk_len(capabilities); at += 1) {
        if (sk_string_is(sk_at(capabilities, at), wanted)) {
            return true;
        }
    }
    return false;
}

/* `typeof x === 'number' && x > 0`, which is the test every `ok` here uses — and which is *not* the
 * test the matching `detail` uses. The reference's details are written with truthiness, so a declared
 * zero reads as absent in the sentence and a declared negative reads as present; both are reproduced
 * rather than tidied, because the sentence is part of the frozen output. */
static bool positive_number(const sk_value *value)
{
    return sk_kind_of(value) == SK_NUMBER && sk_as_number(value, 0) > 0;
}

/* JavaScript truthiness of a number: present, not null, not zero, not NaN. */
static bool truthy_number(const sk_value *value)
{
    if (sk_kind_of(value) != SK_NUMBER) {
        return false;
    }
    double number = sk_as_number(value, 0);
    return number != 0 && number == number;
}

static bool truthy_string(const sk_value *value)
{
    size_t len = 0;
    (void)sk_as_string(value, &len, "");
    return sk_kind_of(value) == SK_STRING && len != 0;
}

static const char *text_of(sk_arena *arena, const sk_value *value)
{
    return sk_js_to_string(arena, value, value != NULL);
}

/* `array.join(', ')`. */
static const char *join_comma(sk_arena *arena, const sk_value *array)
{
    sk_buf out;
    sk_buf_init(&out, arena);
    for (size_t at = 0; at < sk_len(array); at += 1) {
        if (at > 0) {
            sk_buf_append_str(&out, ", ");
        }
        const sk_value *element = sk_at(array, at);
        if (sk_nullish(element)) {
            continue;
        }
        sk_buf_append_str(&out, sk_js_to_string(arena, element, true));
    }
    return out.bytes == NULL ? "" : out.bytes;
}

static const char *concat3(sk_arena *arena, const char *a, const char *b, const char *c)
{
    sk_buf out;
    sk_buf_init(&out, arena);
    sk_buf_append_str(&out, a);
    sk_buf_append_str(&out, b);
    sk_buf_append_str(&out, c);
    return out.bytes == NULL ? "" : out.bytes;
}

static const char *concat5(sk_arena *arena, const char *a, const char *b, const char *c,
                          const char *d, const char *e)
{
    sk_buf out;
    sk_buf_init(&out, arena);
    sk_buf_append_str(&out, a);
    sk_buf_append_str(&out, b);
    sk_buf_append_str(&out, c);
    sk_buf_append_str(&out, d);
    sk_buf_append_str(&out, e);
    return out.bytes == NULL ? "" : out.bytes;
}

/*
 * Evaluate one robot.
 *
 * Pure. Every check is a question about a declared value, and a check whose input is absent **fails**
 * rather than passing — the same rule certification applies to a missing test result, for the same
 * reason. Absence is not evidence of compliance.
 */
static size_t evaluate(sk_arena *arena, const sk_value *subject, check_result *checks)
{
    const sk_value *capabilities = sk_get(subject, "capabilities");
    /* Null and `{}` are deliberately different for `limits`: nobody looked, versus somebody looked and
     * found nothing. Only the first blocks, and the message says which. */
    const sk_value *limits = sk_get(subject, "limits");
    bool limits_declared = sk_kind_of(limits) == SK_OBJECT;
    const sk_value *compute = sk_get(subject, "compute");
    const sk_value *control_modes = sk_get(subject, "control_modes");
    const sk_value *dof = sk_get(subject, "dof");
    const sk_value *control_hz = sk_get(subject, "control_hz");
    const sk_value *adapter_count = sk_get(subject, "adapter_count");

    const sk_value *source = limits_declared ? sk_get(limits, "source") : NULL;
    /* §5.3: absent reads `declared` — `??`, not truthiness, which is why this is the nullish
       bit and not the string. */
    const sk_value *basis = limits_declared ? sk_get(limits, "basis") : NULL;
    const char *basis_word = sk_nullish(basis) ? "declared" : text_of(arena, basis);
    const sk_value *watchdog = limits_declared ? sk_get(limits, "watchdog_ms") : NULL;
    const sk_value *velocity = limits_declared ? sk_get(limits, "joint_velocity_max_rad_s") : NULL;
    const sk_value *torque = limits_declared ? sk_get(limits, "joint_torque_max_nm") : NULL;
    const sk_value *estop = limits_declared ? sk_get(limits, "estop") : NULL;

    double adapters = sk_as_number(adapter_count, 0);
    size_t count = 0;

    checks[count].check = "adapter";
    checks[count].ok = adapters > 0;
    checks[count].required_for = "guest";
    checks[count].detail =
        adapters > 0
            ? concat3(arena, sk_js_number_to_string(arena, adapters), " published adapter",
                      adapters == 1 ? "." : "s.")
            : "No published adapter, so nothing can talk to this robot yet.";
    count += 1;

    checks[count].check = "kinematics";
    checks[count].ok = positive_number(dof);
    checks[count].required_for = "guest";
    checks[count].detail = truthy_number(dof)
                               ? concat3(arena, text_of(arena, dof), " degrees of freedom declared.", "")
                               : "No degree-of-freedom count declared.";
    count += 1;

    checks[count].check = "control_modes";
    checks[count].ok = sk_len(control_modes) > 0;
    checks[count].required_for = "guest";
    checks[count].detail = sk_len(control_modes) > 0
                               ? concat3(arena, "Accepts ", join_comma(arena, control_modes), ".")
                               : "No control modes declared, so no policy can be matched to it.";
    count += 1;

    checks[count].check = "control_rate";
    checks[count].ok = positive_number(control_hz);
    checks[count].required_for = "host";
    checks[count].detail =
        truthy_number(control_hz)
            ? concat3(arena, "Control loop runs at ", text_of(arena, control_hz), " Hz.")
            : "No control rate declared, so loop stability cannot be checked against a policy.";
    count += 1;

    checks[count].check = "envelope";
    checks[count].ok = limits_declared;
    checks[count].required_for = "host";
    checks[count].detail = limits_declared
                               ? "A safety envelope has been declared."
                               : "No safety envelope declared. There is nothing for the kernel to "
                                 "enforce.";
    count += 1;

    checks[count].check = "envelope_source";
    checks[count].ok = truthy_string(source);
    checks[count].required_for = "host";
    checks[count].detail = truthy_string(source)
                               ? concat5(arena, "Limits cite ", text_of(arena, source), " (",
                                         basis_word, ").")
                               : "Limits cite no datasheet. A safety limit without provenance is a "
                                 "rumour.";
    count += 1;

    checks[count].check = "watchdog";
    checks[count].ok = positive_number(watchdog);
    checks[count].required_for = "host";
    checks[count].detail =
        truthy_number(watchdog)
            ? concat3(arena, "Controller faults after ", text_of(arena, watchdog),
                      " ms without a command.")
            : "No watchdog window declared, so the watchdog guard cannot run.";
    count += 1;

    checks[count].check = "joint_ratings";
    checks[count].ok = sk_kind_of(velocity) == SK_NUMBER && sk_kind_of(torque) == SK_NUMBER;
    checks[count].required_for = "host";
    checks[count].detail =
        truthy_number(velocity) && truthy_number(torque)
            ? concat5(arena, "Rated to ", text_of(arena, velocity), " rad/s and ",
                      text_of(arena, torque), " Nm.")
            : "Velocity and torque ceilings are not both declared, so commands cannot be clamped.";
    count += 1;

    bool software_estop = has_capability(capabilities, "safety.estop");
    checks[count].check = "estop_software";
    checks[count].ok = software_estop;
    checks[count].required_for = "guest";
    checks[count].detail = software_estop ? "Exposes a software emergency stop."
                                          : "No software emergency stop capability declared.";
    count += 1;

    bool hardware_estop = sk_kind_of(estop) == SK_BOOL && sk_as_bool(estop, false);
    checks[count].check = "estop_hardware";
    checks[count].ok = hardware_estop;
    checks[count].required_for = "certified";
    checks[count].detail = hardware_estop
                               ? "A hardware emergency stop is fitted."
                               : "No hardware emergency stop declared. The certified tier needs a "
                                 "stop path off the application processor.";
    count += 1;

    bool onboard = has_capability(capabilities, "platform.onboard_compute") ||
                   (sk_kind_of(compute) == SK_OBJECT && sk_len(compute) > 0);
    checks[count].check = "onboard_compute";
    checks[count].ok = onboard;
    checks[count].required_for = "host";
    checks[count].detail = onboard ? "Runs inference onboard."
                                   : "No onboard compute declared, so the OS cannot own the control "
                                     "loop here.";
    count += 1;

    return count;
}

/*
 * Walk upward and stop at the first tier with an unmet requirement.
 *
 * Stopping rather than skipping matters: a robot that satisfies every certified-tier check but lacks a
 * watchdog window is not "certified with a caveat", it is a robot that cannot attempt the host tier.
 */
static const char *highest_attainable(const check_result *checks, size_t count)
{
    const char *best = NULL;
    for (size_t tier = 0; tier < SK_TIER_COUNT; tier += 1) {
        for (size_t at = 0; at < count; at += 1) {
            if (strcmp(checks[at].required_for, TIER_ORDER[tier]) == 0 && !checks[at].ok) {
                return best;
            }
        }
        best = TIER_ORDER[tier];
    }
    return best;
}

static size_t tier_index(const char *tier)
{
    for (size_t at = 0; at < SK_TIER_COUNT; at += 1) {
        if (strcmp(TIER_ORDER[at], tier) == 0) {
            return at;
        }
    }
    return SK_TIER_COUNT;
}

static const sk_value *shape_check(sk_arena *arena, const check_result *check)
{
    sk_value *out = sk_object(arena);
    sk_set(arena, out, "check", sk_string(arena, check->check));
    sk_set(arena, out, "ok", sk_bool(check->ok));
    sk_set(arena, out, "required_for", sk_string(arena, check->required_for));
    sk_set(arena, out, "detail", sk_string(arena, check->detail));
    return out;
}

/* The blockers, in tier order — the vendor's to-do list.
 *
 * A **stable** insertion sort, because `Array.prototype.sort` has been stable since ES2019 and the
 * reference sorts by tier alone: two guest-tier blockers keep the order the checks were built in, and
 * an unstable sort would reorder them on any input with two blockers at one tier. */
static void sort_blockers(check_result *blockers, size_t count)
{
    for (size_t at = 1; at < count; at += 1) {
        check_result held = blockers[at];
        size_t held_tier = tier_index(held.required_for);
        size_t where = at;
        while (where > 0 && tier_index(blockers[where - 1].required_for) > held_tier) {
            blockers[where] = blockers[where - 1];
            where -= 1;
        }
        blockers[where] = held;
    }
}

static const sk_value *report_of(sk_arena *arena, const sk_value *subject, check_result *checks,
                                 size_t *out_count, check_result *blockers, size_t *out_blockers,
                                 const char **out_attainable)
{
    size_t count = evaluate(arena, subject, checks);
    const char *attainable = highest_attainable(checks, count);

    size_t blocker_count = 0;
    size_t passing = 0;
    for (size_t at = 0; at < count; at += 1) {
        if (checks[at].ok) {
            passing += 1;
        } else {
            blockers[blocker_count] = checks[at];
            blocker_count += 1;
        }
    }
    sort_blockers(blockers, blocker_count);

    sk_value *check_list = sk_array(arena);
    for (size_t at = 0; at < count; at += 1) {
        sk_push(arena, check_list, shape_check(arena, &checks[at]));
    }
    sk_value *blocker_list = sk_array(arena);
    for (size_t at = 0; at < blocker_count; at += 1) {
        sk_push(arena, blocker_list, shape_check(arena, &blockers[at]));
    }

    sk_value *out = sk_object(arena);
    sk_set(arena, out, "slug", sk_get(subject, "slug") == NULL ? sk_null() : sk_get(subject, "slug"));
    sk_set(arena, out, "name", sk_get(subject, "name") == NULL ? sk_null() : sk_get(subject, "name"));
    sk_set(arena, out, "attainable",
           attainable == NULL ? sk_null() : sk_string(arena, attainable));
    sk_set(arena, out, "checks", check_list);
    sk_set(arena, out, "blockers", blocker_list);
    /* A share, not a score anyone should quote — and a plain division, so eight of eleven is the
     * double nearest 8/11 and rule 3 writes its shortest round-trip. */
    sk_set(arena, out, "completeness", sk_number(arena, (double)passing / (double)count));

    *out_count = count;
    *out_blockers = blocker_count;
    *out_attainable = attainable;
    return out;
}

const sk_value *sk_readiness_robot_case(sk_arena *arena, const sk_value *input, const char **error)
{
    (void)error;
    check_result checks[SK_CHECK_COUNT];
    check_result blockers[SK_CHECK_COUNT];
    size_t count = 0;
    size_t blocker_count = 0;
    const char *attainable = NULL;
    return report_of(arena, input, checks, &count, blockers, &blocker_count, &attainable);
}

/*
 * Aggregate across the registry.
 *
 * `common_blockers` is the number worth putting on a screen: it says whether the ecosystem's problem is
 * missing adapters or missing datasheets, and those imply completely different work.
 */
const sk_value *sk_readiness_fleet_case(sk_arena *arena, const sk_value *input, const char **error)
{
    (void)error;
    const sk_value *subjects = sk_get(input, "subjects");
    size_t total = sk_len(subjects);

    /* Robots are counted at their highest tier only. */
    double none = 0;
    double by_tier[SK_TIER_COUNT] = {0};
    /* Counted in the order the checks are declared, which is the insertion order of the reference's
     * Map and therefore the tie-break of the sort below. */
    double blocker_counts[SK_CHECK_COUNT] = {0};
    size_t blocker_order[SK_CHECK_COUNT];
    size_t distinct = 0;

    for (size_t at = 0; at < total; at += 1) {
        check_result checks[SK_CHECK_COUNT];
        check_result blockers[SK_CHECK_COUNT];
        size_t count = 0;
        size_t blocker_count = 0;
        const char *attainable = NULL;
        (void)report_of(arena, sk_at(subjects, at), checks, &count, blockers, &blocker_count,
                        &attainable);
        if (attainable == NULL) {
            none += 1;
        } else {
            by_tier[tier_index(attainable)] += 1;
        }
        for (size_t which = 0; which < blocker_count; which += 1) {
            for (size_t index = 0; index < SK_CHECK_COUNT; index += 1) {
                if (strcmp(READINESS_CHECKS[index], blockers[which].check) != 0) {
                    continue;
                }
                if (blocker_counts[index] == 0) {
                    blocker_order[distinct] = index;
                    distinct += 1;
                }
                blocker_counts[index] += 1;
                break;
            }
        }
    }

    /* Descending by robots, stable — so two checks failing equally often stay in the order they were
     * first seen, which is the reference's Map iteration. */
    for (size_t at = 1; at < distinct; at += 1) {
        size_t held = blocker_order[at];
        size_t where = at;
        while (where > 0 && blocker_counts[blocker_order[where - 1]] < blocker_counts[held]) {
            blocker_order[where] = blocker_order[where - 1];
            where -= 1;
        }
        blocker_order[where] = held;
    }

    sk_value *tiers = sk_object(arena);
    sk_set(arena, tiers, "none", sk_number(arena, none));
    for (size_t at = 0; at < SK_TIER_COUNT; at += 1) {
        sk_set(arena, tiers, TIER_ORDER[at], sk_number(arena, by_tier[at]));
    }

    sk_value *common = sk_array(arena);
    for (size_t at = 0; at < distinct; at += 1) {
        sk_value *row = sk_object(arena);
        sk_set(arena, row, "check", sk_string(arena, READINESS_CHECKS[blocker_order[at]]));
        sk_set(arena, row, "robots", sk_number(arena, blocker_counts[blocker_order[at]]));
        sk_push(arena, common, row);
    }

    sk_value *out = sk_object(arena);
    sk_set(arena, out, "total", sk_number(arena, (double)total));
    sk_set(arena, out, "by_tier", tiers);
    sk_set(arena, out, "common_blockers", common);
    return out;
}

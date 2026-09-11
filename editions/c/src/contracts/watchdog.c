#include "skynet_os/watchdog.h"

#include <math.h>
#include <string.h>

#include "skynet_os/js.h"
#include "skynet_os/write.h"

/* §46.2's vocabulary, in the order the specification lists it. */
static const char *const COMMAND_LOSS_BEHAVIOURS[] = {"hold", "brake", "fault", "coast"};

/* The chain, as data (§46.1). */
typedef struct {
    double layer;
    const char *name;
    const char *catches;
    const char *response;
    const char *required_from;
    const char *petted_by;
} watchdog_layer;

static const watchdog_layer CHAIN[] = {
    {1, "kernel_guard", "the policy stopped producing commands", "stop_cat2", "guest",
     "the policy runtime, every cycle"},
    {2, "supervisor", "the supervisor's reconcile loop hung", "restart skynetd", "guest",
     "skynetd, from inside the reconcile loop"},
    /* §46.1's sentence, and the reason layer 3 means anything. */
    {3, "soc_wdt", "the application processor wedged entirely", "reset the SoC", "host",
     "the Safety Kernel, not skynetd"},
    {4, "safety_mcu", "the AP wedged and the reset did not recover it", "cut drive enable",
     "certified", "an independent timer on the safety MCU"},
};

/* The four windows §46.3 nests, in order, innermost first. */
static const char *const WINDOWS[] = {
    "kernel_cycle_worst_case_ms",
    "envelope_watchdog_ms",
    "soc_wdt_ms",
    "drive_command_timeout_ms",
};

#define SK_WINDOW_COUNT (sizeof WINDOWS / sizeof WINDOWS[0])

/*
 * Whether a body may attempt the host tier, given what it says its drives do (§46.2). NULL when it may.
 *
 * Three refusals and they are one refusal: **the recovery path has not been established to be
 * survivable.** A body that coasts has one that demonstrably is not; a body that declared nothing has
 * one nobody has characterised, and treating silence as `hold` would be inventing the most convenient
 * answer to the only question layer 3 turns on.
 */
static const char *host_blocked(sk_arena *arena, const sk_value *on_command_loss)
{
    if (sk_string_is(on_command_loss, "coast")) {
        return "this body's drives coast when commands stop, and layer 3 of the watchdog chain "
               "recovers by resetting the processor — which on this machine is a fall. The sound "
               "configuration is the certified tier, where layer 4 cuts drive enable independently of "
               "the processor (§46.2)";
    }
    if (sk_nullish(on_command_loss)) {
        return "this body does not declare what its drives do when commands stop, and layer 3 of the "
               "watchdog chain recovers by resetting the processor. Silence is not `hold`: it is a "
               "recovery path nobody has characterised (§46.2)";
    }
    size_t len = 0;
    const char *declared = sk_as_string(on_command_loss, &len, "");
    for (size_t at = 0; at < sizeof COMMAND_LOSS_BEHAVIOURS / sizeof COMMAND_LOSS_BEHAVIOURS[0];
         at += 1) {
        if (sk_string_is(on_command_loss, COMMAND_LOSS_BEHAVIOURS[at])) {
            return NULL;
        }
    }
    /* Rendered through ToString, because the reference interpolates the declaration into the message
     * and a declaration that is not a string is what a bad descriptor looks like. */
    sk_buf message;
    sk_buf_init(&message, arena);
    sk_buf_append_str(&message, "this body declares `on_command_loss: ");
    if (sk_kind_of(on_command_loss) == SK_STRING) {
        sk_buf_append(&message, declared, len);
    } else {
        sk_buf_append_str(&message, sk_js_to_string(arena, on_command_loss, true));
    }
    sk_buf_append_str(&message,
                      "`, which is not one of hold, brake, fault or coast — so what it does while its "
                      "processor resets is not something anybody can read off the descriptor (§46.2)");
    return message.bytes;
}

static const char *nesting_code(size_t inner, size_t outer)
{
    if (inner == 0 && outer == 1) {
        return "cycle_exceeds_watchdog";
    }
    if (inner == 1 && outer == 2) {
        return "wdt_pre_empts_kernel";
    }
    if (inner == 2 && outer == 3) {
        return "drive_faults_first";
    }
    return "windows_not_nested";
}

static const char *nesting_consequence(size_t inner, size_t outer)
{
    if (inner == 0 && outer == 1) {
        return "The kernel cannot complete a cycle inside its own command deadline, so the watchdog "
               "guard trips on a robot that is working correctly.";
    }
    if (inner == 1 && outer == 2) {
        return "The processor resets before the kernel executes its controlled stop, so the harsher "
               "recovery pre-empts the gentler one and the robot never performs its best available "
               "behaviour (§46.3).";
    }
    if (inner == 2 && outer == 3) {
        return "The drive faults on its own before the hardware watchdog fires, so layer 3 is present, "
               "configured, and never reached (§46.3).";
    }
    return "The windows must nest innermost-first for every layer above to be reachable.";
}

/* `present`: declared, not null, and finite. A window a descriptor wrote as a non-finite value is one
 * nobody can nest, which is the same answer as not declaring it. */
static bool window_present(const sk_value *value, double *out)
{
    if (sk_kind_of(value) != SK_NUMBER) {
        return false;
    }
    double number = sk_as_number(value, 0);
    if (!isfinite(number)) {
        return false;
    }
    *out = number;
    return true;
}

const sk_value *sk_watchdog_case(sk_arena *arena, const sk_value *input, const char **error)
{
    (void)error;
    const sk_value *on_command_loss = sk_get(input, "on_command_loss");
    const sk_value *windows = sk_get(input, "windows");

    sk_value *chain = sk_array(arena);
    for (size_t at = 0; at < sizeof CHAIN / sizeof CHAIN[0]; at += 1) {
        sk_value *layer = sk_object(arena);
        sk_set(arena, layer, "layer", sk_number(arena, CHAIN[at].layer));
        sk_set(arena, layer, "name", sk_string(arena, CHAIN[at].name));
        sk_set(arena, layer, "catches", sk_string(arena, CHAIN[at].catches));
        sk_set(arena, layer, "response", sk_string(arena, CHAIN[at].response));
        sk_set(arena, layer, "required_from", sk_string(arena, CHAIN[at].required_from));
        sk_set(arena, layer, "petted_by", sk_string(arena, CHAIN[at].petted_by));
        sk_push(arena, chain, layer);
    }

    /*
     * §46.3's nesting, over **adjacent declared pairs only**.
     *
     * Adjacent because comparing every pair would report one inversion up to three times and bury the
     * one an operator has to fix. Declared-only because a gap in the middle does not stop the walk:
     * with no `envelope_watchdog_ms`, the kernel cycle and the SoC window are still worth comparing,
     * since that inversion is real whatever sits between them.
     */
    double values[SK_WINDOW_COUNT];
    size_t declared[SK_WINDOW_COUNT];
    size_t declared_count = 0;
    sk_value *declared_out = sk_array(arena);
    sk_value *missing = sk_array(arena);
    for (size_t at = 0; at < SK_WINDOW_COUNT; at += 1) {
        const sk_value *held = sk_get(windows, WINDOWS[at]);
        double value = 0;
        bool have = window_present(held, &value);
        /* An array, not an object: canonical JSON sorts an object's keys, and these four windows mean
         * nothing except in order. */
        sk_value *row = sk_object(arena);
        sk_set(arena, row, "window", sk_string(arena, WINDOWS[at]));
        sk_set(arena, row, "ms", sk_nullish(held) ? sk_null() : held);
        sk_push(arena, declared_out, row);
        if (have) {
            values[declared_count] = value;
            declared[declared_count] = at;
            declared_count += 1;
        } else {
            sk_push(arena, missing, sk_string(arena, WINDOWS[at]));
        }
    }

    sk_value *faults = sk_array(arena);
    sk_value *checked = sk_array(arena);
    size_t fault_count = 0;
    for (size_t index = 0; index + 1 < declared_count; index += 1) {
        size_t inner = declared[index];
        size_t outer = declared[index + 1];
        sk_value *pair = sk_array(arena);
        sk_push(arena, pair, sk_string(arena, WINDOWS[inner]));
        sk_push(arena, pair, sk_string(arena, WINDOWS[outer]));
        sk_push(arena, checked, pair);
        /* **Strictly less than.** Two watchdogs with the same window is a race whose winner is decided
         * by interrupt latency, and a chain whose order depends on which timer fires first is not a
         * chain. */
        if (values[index] < values[index + 1]) {
            continue;
        }
        sk_buf detail;
        sk_buf_init(&detail, arena);
        sk_buf_append_str(&detail, WINDOWS[inner]);
        sk_buf_append_str(&detail, " is ");
        sk_buf_append_str(&detail, sk_js_number_to_string(arena, values[index]));
        sk_buf_append_str(&detail, " ms and ");
        sk_buf_append_str(&detail, WINDOWS[outer]);
        sk_buf_append_str(&detail, " is ");
        sk_buf_append_str(&detail, sk_js_number_to_string(arena, values[index + 1]));
        sk_buf_append_str(&detail, " ms, so the outer window is not outside the inner one. ");
        sk_buf_append_str(&detail, nesting_consequence(inner, outer));

        sk_value *fault = sk_object(arena);
        sk_set(arena, fault, "code", sk_string(arena, nesting_code(inner, outer)));
        sk_set(arena, fault, "inner", sk_string(arena, WINDOWS[inner]));
        sk_set(arena, fault, "outer", sk_string(arena, WINDOWS[outer]));
        sk_set(arena, fault, "detail", sk_string(arena, detail.bytes));
        sk_push(arena, faults, fault);
        fault_count += 1;
    }

    sk_value *nesting = sk_object(arena);
    sk_set(arena, nesting, "declared", declared_out);
    sk_set(arena, nesting, "faults", faults);
    sk_set(arena, nesting, "missing", missing);
    /* `ordered` is every pair that could be compared nesting. **Not** the same as "the chain is
     * sound", which is `complete`. */
    sk_set(arena, nesting, "ordered", sk_bool(fault_count == 0));
    sk_set(arena, nesting, "complete",
           sk_bool(fault_count == 0 && sk_len(missing) == 0));

    const char *blocked = host_blocked(arena, on_command_loss);
    sk_value *out = sk_object(arena);
    sk_set(arena, out, "chain", chain);
    sk_set(arena, out, "on_command_loss",
           sk_nullish(on_command_loss) ? sk_null() : on_command_loss);
    sk_set(arena, out, "host_blocked", blocked == NULL ? sk_null() : sk_string(arena, blocked));
    sk_set(arena, out, "nesting", nesting);
    return out;
}

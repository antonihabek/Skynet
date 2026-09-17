#include "skynet_os/watchdog.h"

#include <math.h>
#include <stdlib.h>
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

/*
 * ── Per chain (§87.3) ───────────────────────────────────────────────────────
 *
 * §53 carried the gap from the moment Part V was written: *"a humanoid may brake on the legs and coast
 * on the arms. The field is currently one value per body and probably needs to be per chain."* §87.3
 * makes it per chain, keyed by §5.1's vocabulary, and the decision it settles is what a **missing**
 * chain means — refused, not inherited, because inheriting a body-wide value makes a partially declared
 * humanoid look fully declared.
 *
 * `resolve_command_loss` is the one place that answers *"may this body attempt host"*, whichever way
 * the body declared, and `host_blocked` above stays what it was: the body-wide rules, called by the
 * resolver rather than duplicated beside it. §78 is the record of what two spellings of one fact cost.
 */

/* Whether `chains` — the chains this body's joints are on — carries `name`. */
static bool chain_listed(const sk_value *chains, const char *name, size_t name_len)
{
    for (size_t at = 0; at < sk_len(chains); at += 1) {
        size_t len = 0;
        const char *text = sk_as_string(sk_at(chains, at), &len, NULL);
        if (text != NULL && len == name_len && memcmp(text, name, len) == 0) {
            return true;
        }
    }
    return false;
}

static int compare_chain_names(const void *left, const void *right)
{
    return strcmp(*(const char *const *)left, *(const char *const *)right);
}

/*
 * §87.3's resolution: what each chain does when commands stop, and whether host is available.
 *
 * `per_chain_out` receives the document `shapeWatchdog` renders — `declared`, `chains` and
 * `unknown_chains` — and the return is the one host refusal, or NULL.
 *
 * The refusal order is the substance, and two of its steps were found by building it:
 *
 *   1. **Both declared** — §84.2's rule about two action spaces, for the same reason: two descriptions
 *      of one fact, redundant when they agree and a contradiction when they do not.
 *   2. **A map on a body with no chains** — a body whose joints carry no parseable role is on *no*
 *      chain, so a complete-looking map covers nothing. §5.1 makes a role optional and an unparseable
 *      one is not an error, so the remedy is roles rather than behaviours.
 *   3. **A chain the map does not cover** — refused, not inherited.
 *   4. **An unrecognised behaviour, before a `coast`.** `coast` is characterised: §46.2 knows what it
 *      means and the remedy is the certified tier. An unrecognised word is *uncharacterised*, which is
 *      strictly worse and does not rule out coasting either, so it is named first.
 *   5. **`coast` on any chain** — refused, with every coasting chain named. A humanoid whose arms coast
 *      falls with its arms.
 */
static const char *resolve_command_loss(sk_arena *arena, const sk_value *body,
                                        const sk_value *by_chain, const sk_value *chains,
                                        sk_value **per_chain_out)
{
    bool declared = !sk_nullish(by_chain);

    /* Over-declaration, not a fault: a vendor copying a descriptor between two robots is the way it
     * happens, and naming it is cheaper than a silent no-op. Sorted, as `Object.keys(...).sort()`. */
    sk_value *unknown = sk_array(arena);
    if (declared && sk_kind_of(by_chain) == SK_OBJECT) {
        size_t count = 0;
        for (const sk_member *member = sk_members(by_chain); member != NULL; member = member->next) {
            if (!chain_listed(chains, member->key, member->key_len)) {
                count += 1;
            }
        }
        if (count > 0) {
            const char **names = sk_arena_alloc_array(arena, count, sizeof *names);
            size_t at = 0;
            for (const sk_member *member = sk_members(by_chain); member != NULL;
                 member = member->next) {
                if (!chain_listed(chains, member->key, member->key_len)) {
                    names[at] = member->key;
                    at += 1;
                }
            }
            qsort(names, count, sizeof *names, compare_chain_names);
            for (at = 0; at < count; at += 1) {
                sk_push(arena, unknown, sk_string(arena, names[at]));
            }
        }
    }

    /* One row per chain **this body has**, in the order it was given, because canonical JSON sorts an
     * object's keys and a chain list read alphabetically puts a body's legs between its hands and its
     * neck. `null` is a chain the body has and the declaration does not cover. */
    sk_value *rows = sk_array(arena);
    for (size_t at = 0; at < sk_len(chains); at += 1) {
        size_t len = 0;
        const char *name = sk_as_string(sk_at(chains, at), &len, "");
        const sk_value *behaviour = sk_null();
        if (declared) {
            const sk_value *held = sk_kind_of(by_chain) == SK_OBJECT ? sk_get(by_chain, name) : NULL;
            if (held != NULL && sk_kind_of(held) == SK_STRING) {
                behaviour = held;
            }
        } else if (sk_kind_of(body) == SK_STRING) {
            behaviour = body;
        }
        sk_value *row = sk_object(arena);
        sk_set(arena, row, "chain", sk_string_len(arena, name, len));
        sk_set(arena, row, "behaviour", behaviour);
        sk_push(arena, rows, row);
    }

    sk_value *per_chain = sk_object(arena);
    sk_set(arena, per_chain, "declared", sk_bool(declared));
    sk_set(arena, per_chain, "chains", rows);
    sk_set(arena, per_chain, "unknown_chains", unknown);
    *per_chain_out = per_chain;

    if (declared && sk_kind_of(body) == SK_STRING) {
        return "this body declares both `on_command_loss` and `on_command_loss_by_chain`, which are "
               "two descriptions of one fact — redundant where they agree and a contradiction where "
               "they do not. Delete the one this body does not mean (§87.3)";
    }

    if (!declared) {
        return host_blocked(arena, body);
    }

    if (sk_len(chains) == 0) {
        return "this body declares what its drives do per chain, and none of its joints carries a role "
               "this vocabulary parses — so the declaration covers no joint at all. §5.1 permits a body "
               "the vocabulary does not describe; what it cannot do is declare per chain. Give the "
               "joints roles, or declare `on_command_loss` for the body (§87.3)";
    }

    for (size_t at = 0; at < sk_len(rows); at += 1) {
        const sk_value *row = sk_at(rows, at);
        if (!sk_nullish(sk_get(row, "behaviour"))) {
            continue;
        }
        sk_buf message;
        sk_buf_init(&message, arena);
        sk_buf_append_str(&message, "this body has a ");
        sk_buf_append_str(&message, sk_as_string(sk_get(row, "chain"), NULL, ""));
        sk_buf_append_str(&message, " chain and its per-chain declaration does not cover it. A missing "
                                    "chain is refused rather than inheriting a body-wide value, because "
                                    "inheriting makes a partially declared body look fully declared "
                                    "(§87.3)");
        return message.bytes;
    }

    for (size_t at = 0; at < sk_len(rows); at += 1) {
        const sk_value *row = sk_at(rows, at);
        const sk_value *behaviour = sk_get(row, "behaviour");
        bool known = false;
        for (size_t which = 0; which < sizeof COMMAND_LOSS_BEHAVIOURS / sizeof COMMAND_LOSS_BEHAVIOURS[0];
             which += 1) {
            if (sk_string_is(behaviour, COMMAND_LOSS_BEHAVIOURS[which])) {
                known = true;
                break;
            }
        }
        if (known) {
            continue;
        }
        sk_buf message;
        sk_buf_init(&message, arena);
        sk_buf_append_str(&message, "this body declares `");
        sk_buf_append_str(&message, sk_as_string(sk_get(row, "chain"), NULL, ""));
        sk_buf_append_str(&message, ": ");
        sk_buf_append_str(&message, sk_as_string(behaviour, NULL, ""));
        sk_buf_append_str(&message, "`, which is not one of hold, brake, fault or coast — so what that "
                                    "chain does while its processor resets is not something anybody can "
                                    "read off the descriptor. An unrecognised behaviour is refused "
                                    "before a `coast` is, because `coast` is characterised and this is "
                                    "not (§46.2, §87.3)");
        return message.bytes;
    }

    sk_buf coasting;
    sk_buf_init(&coasting, arena);
    size_t coasting_count = 0;
    for (size_t at = 0; at < sk_len(rows); at += 1) {
        const sk_value *row = sk_at(rows, at);
        if (!sk_string_is(sk_get(row, "behaviour"), "coast")) {
            continue;
        }
        if (coasting_count > 0) {
            sk_buf_append_str(&coasting, ", ");
        }
        sk_buf_append_str(&coasting, sk_as_string(sk_get(row, "chain"), NULL, ""));
        coasting_count += 1;
    }
    if (coasting_count > 0) {
        sk_buf message;
        sk_buf_init(&message, arena);
        sk_buf_append_str(&message, "this body's ");
        sk_buf_append_str(&message, coasting.bytes);
        sk_buf_append_str(&message, " drives coast when commands stop, and layer 3 of the watchdog chain "
                                    "recovers by resetting the processor — which for those chains is a "
                                    "fall. The sound configuration is the certified tier, where layer 4 "
                                    "cuts drive enable independently of the processor (§46.2)");
        return message.bytes;
    }

    return NULL;
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
    const sk_value *by_chain = sk_get(input, "on_command_loss_by_chain");
    const sk_value *chains = sk_get(input, "chains");
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

    /* One answer about host, whichever way the body declared (§87.3). A document over a body with no
     * map is byte-identical to the one this function produced before §87. */
    sk_value *per_chain = NULL;
    const char *blocked = resolve_command_loss(arena, on_command_loss, by_chain, chains, &per_chain);

    sk_value *out = sk_object(arena);
    sk_set(arena, out, "chain", chain);
    sk_set(arena, out, "on_command_loss",
           sk_nullish(on_command_loss) ? sk_null() : on_command_loss);
    /* Present on every document, because *"this body declares one behaviour"* and *"this body declares
     * one behaviour per chain"* are two states a reader has to be able to tell apart without knowing
     * which fields the descriptor happened to carry (§87.3). */
    sk_set(arena, out, "per_chain", per_chain);
    sk_set(arena, out, "host_blocked", blocked == NULL ? sk_null() : sk_string(arena, blocked));
    sk_set(arena, out, "nesting", nesting);
    return out;
}

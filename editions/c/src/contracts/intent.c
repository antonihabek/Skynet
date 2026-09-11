#include "skynet_os/intent.h"

#include <string.h>

#include "skynet_os/js.h"
#include "skynet_os/write.h"

/* `value ?? fallback`: null and absent take the fallback, and nothing else does. The reference's
 * fields are optional numbers, so this is the whole coalescing rule it uses. */
static double or_zero(const sk_value *value)
{
    if (sk_nullish(value)) {
        return 0;
    }
    return sk_as_number(value, 0);
}

/* `(value ?? '').trim()`, which is how both digests and the decode reason are compared. `trim()` is
 * JavaScript's, so a digest wrapped in byte-order marks is the same digest and one behind a U+0085 is
 * a different document — two corpus cases, one line apart. */
static const char *trimmed_or_empty(sk_arena *arena, const sk_value *value, size_t *out_len)
{
    if (sk_kind_of(value) != SK_STRING) {
        *out_len = 0;
        return "";
    }
    size_t len = 0;
    const char *text = sk_as_string(value, &len, "");
    return sk_js_trim(arena, text, len, out_len);
}

static void push_fault(sk_arena *arena, sk_value *faults, const char *code, const char *severity,
                       const char *message)
{
    sk_value *fault = sk_object(arena);
    sk_set(arena, fault, "code", sk_string(arena, code));
    sk_set(arena, fault, "message", sk_string(arena, message));
    sk_set(arena, fault, "severity", sk_string(arena, severity));
    sk_push(arena, faults, fault);
}

static const sk_value *verdict(sk_arena *arena, bool adopt, bool changed, double generation,
                               sk_value *faults)
{
    sk_value *out = sk_object(arena);
    sk_set(arena, out, "adopt", sk_bool(adopt));
    sk_set(arena, out, "changed", sk_bool(changed));
    sk_set(arena, out, "faults", faults);
    sk_set(arena, out, "generation", sk_number(arena, generation));
    return out;
}

const sk_value *sk_intent_offer_case(sk_arena *arena, const sk_value *input, const char **error)
{
    (void)error;
    const sk_value *offer = sk_get(input, "offer");
    const sk_value *held = sk_get(input, "held");

    sk_value *faults = sk_array(arena);
    double held_generation = or_zero(sk_get(held, "generation"));
    size_t held_digest_len = 0;
    const char *held_digest = trimmed_or_empty(arena, sk_get(held, "digest"), &held_digest_len);
    double offered = or_zero(sk_get(offer, "generation"));
    size_t digest_len = 0;
    const char *digest = trimmed_or_empty(arena, sk_get(offer, "digest"), &digest_len);

    /* `offer.decoded !== true` — strict, so an absent field, a null and a truthy string all mean this
     * is not a desired state. Decoding arrives decided, because a second implementation of it here
     * would be a second answer to a question already answered. */
    if (!(sk_kind_of(sk_get(offer, "decoded")) == SK_BOOL && sk_as_bool(sk_get(offer, "decoded"), false))) {
        size_t why_len = 0;
        const char *why = trimmed_or_empty(arena, sk_get(offer, "decode_error"), &why_len);
        sk_buf message;
        sk_buf_init(&message, arena);
        sk_buf_append_str(&message, "This response is not a desired state");
        if (why_len != 0) {
            sk_buf_append_str(&message, ": ");
            sk_buf_append(&message, why, why_len);
        }
        sk_buf_append_str(&message,
                          ". It has not been written to disk, and this robot carries on reconciling "
                          "against what it already has — which is what it does whenever it cannot "
                          "reach the Hub at all (§34.1), so nothing about the robot is worse for it.");
        push_fault(arena, faults, "undecodable", "error", message.bytes);
        return verdict(arena, false, false, held_generation, faults);
    }

    /*
     * The reused generation, and the reason it refuses rather than adopts.
     *
     * §11 rests idempotence on the counter. A Hub serving two documents under one generation has
     * broken that, and the robot is the only party that can see it. Adopting would have the robot
     * running new software while reporting the old generation as applied, and a fleet console would
     * show it converged — the claim §17 forbids the platform from making, invisible because every
     * robot looks right. Refusing produces a visible, actionable state instead.
     */
    if (offered == held_generation && digest_len != 0 && held_digest_len != 0 &&
        (digest_len != held_digest_len || memcmp(digest, held_digest, digest_len) != 0)) {
        sk_buf message;
        sk_buf_init(&message, arena);
        sk_buf_append_str(&message, "The Hub is serving a different document under generation ");
        sk_buf_append_str(&message, sk_js_number_to_string(arena, offered));
        sk_buf_append_str(&message,
                          ", which this robot has already applied. §11 rests idempotence on that "
                          "counter, so adopting this would have the robot running new software while "
                          "reporting the old generation as applied — and a fleet console would show "
                          "it converged. Bump the generation on the Hub and this robot will take it "
                          "on the next poll.");
        push_fault(arena, faults, "generation_reused", "error", message.bytes);
        return verdict(arena, false, false, held_generation, faults);
    }

    /* The opposite call: a generation that goes backwards is a Hub restored from a backup, and a robot
     * that refused to follow one would be a fleet nobody could recover — permanently, with no remedy
     * short of visiting every machine. So it is adopted, and reported once. */
    if (offered < held_generation) {
        sk_buf message;
        sk_buf_init(&message, arena);
        sk_buf_append_str(&message, "This robot has applied generation ");
        sk_buf_append_str(&message, sk_js_number_to_string(arena, held_generation));
        sk_buf_append_str(&message, " and the Hub is asking for ");
        sk_buf_append_str(&message, sk_js_number_to_string(arena, offered));
        sk_buf_append_str(&message,
                          ". Intent has gone backwards, which is what a Hub restored from a backup "
                          "looks like. It is adopted — a robot that refused to follow one would be a "
                          "fleet nobody could recover — and it is reported, because it is worth "
                          "somebody knowing once.");
        push_fault(arena, faults, "generation_went_backwards", "warning", message.bytes);
    }

    double running = or_zero(sk_get(held, "running"));
    if (or_zero(sk_get(offer, "packages")) == 0 && running > 0) {
        sk_buf message;
        sk_buf_init(&message, arena);
        sk_buf_append_str(&message, "This desired state names no packages and this robot is running ");
        sk_buf_append_str(&message, sk_js_number_to_string(arena, running));
        sk_buf_append_str(&message,
                          ". That is what an operator emptying a robot looks like, and it is also "
                          "what a Hub serving a stub looks like. It is adopted, because stopping is "
                          "never the dangerous direction (§33.4) — and it is said out loud, because "
                          "the two are indistinguishable from here.");
        push_fault(arena, faults, "empties_the_robot", "warning", message.bytes);
    }

    bool changed = digest_len != held_digest_len || memcmp(digest, held_digest, digest_len) != 0;
    return verdict(arena, true, changed, offered, faults);
}

#include "skynet_os/embodiment.h"

#include <stdlib.h>
#include <string.h>

#include "skynet_os/js.h"
#include "skynet_os/write.h"

/*
 * The canonical chains.
 *
 * Twelve, and `aux` and `tail` are in the list so that a body nobody anticipated — a snake robot's
 * segments, a second mast, a tail-mounted sensor boom — can still declare a role that parses, rather
 * than being pushed into a wrong-but-close chain because the vocabulary had no honest slot for it.
 */
static const char *const CANONICAL_CHAINS[] = {
    "left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand",
    "torso",    "neck",      "head",     "base",      "tail",      "aux",
};

/* Axis of motion, when a link has more than one degree of freedom. A three-DoF shoulder is three
 * joints in every real driver, and the axis suffix is what keeps `left_arm.shoulder.pitch` from
 * colliding with `left_arm.shoulder.roll`. */
static const char *const CANONICAL_AXES[] = {
    "pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip",
};

static bool in_list(const char *const *list, size_t count, const char *text, size_t len)
{
    for (size_t at = 0; at < count; at += 1) {
        if (strlen(list[at]) == len && memcmp(list[at], text, len) == 0) {
            return true;
        }
    }
    return false;
}

static bool lower_or_underscore(char byte)
{
    return (byte >= 'a' && byte <= 'z') || byte == '_';
}

static bool link_byte(char byte)
{
    return lower_or_underscore(byte) || (byte >= '0' && byte <= '9');
}

static bool lower(char byte)
{
    return byte >= 'a' && byte <= 'z';
}

static bool digit(char byte)
{
    return byte >= '0' && byte <= '9';
}

static size_t run_of(const char *text, size_t len, size_t at, bool (*member)(char))
{
    size_t end = at;
    while (end < len && member(text[end])) {
        end += 1;
    }
    return end - at;
}

sk_role sk_role_parse_text(sk_arena *arena, const char *role, size_t len)
{
    sk_role out;
    memset(&out, 0, sizeof out);

    size_t normalised_len = 0;
    const char *trimmed = sk_js_trim(arena, role, len, &normalised_len);
    /* `role.trim().toLowerCase()`, in that order and with the engine's full mapping: the corpus holds
     * `necK.link.pitch`, which is a chain only after lower-casing, and a role wrapped in U+FEFF, which
     * is whitespace only to JavaScript. */
    const char *text = sk_js_lower(arena, trimmed, normalised_len, &normalised_len);

    /* `^([a-z_]+)\.([a-z0-9_]+)(?:\.([a-z]+))?(?:#(\d+))?$`. Every character class here is disjoint
     * from the separator that follows it, so a greedy run is the only run that can match and there is
     * nothing to backtrack. */
    size_t at = 0;
    size_t chain_len = run_of(text, normalised_len, at, lower_or_underscore);
    if (chain_len == 0) {
        return out;
    }
    size_t chain_at = at;
    at += chain_len;
    if (at >= normalised_len || text[at] != '.') {
        return out;
    }
    at += 1;
    size_t link_len = run_of(text, normalised_len, at, link_byte);
    if (link_len == 0) {
        return out;
    }
    size_t link_at = at;
    at += link_len;

    size_t axis_at = 0;
    size_t axis_len = 0;
    if (at < normalised_len && text[at] == '.') {
        size_t after = at + 1;
        axis_len = run_of(text, normalised_len, after, lower);
        if (axis_len == 0) {
            /* The axis group is optional, but a point with nothing readable after it cannot be
             * consumed by anything else either, so the whole role fails — `a.b.3` and `a.b.` both. */
            return out;
        }
        axis_at = after;
        at = after + axis_len;
    }

    bool has_index = false;
    size_t index_at = 0;
    size_t index_len = 0;
    if (at < normalised_len && text[at] == '#') {
        size_t after = at + 1;
        index_len = run_of(text, normalised_len, after, digit);
        if (index_len == 0) {
            return out;
        }
        index_at = after;
        has_index = true;
        at = after + index_len;
    }
    if (at != normalised_len) {
        return out;
    }

    if (!in_list(CANONICAL_CHAINS, sizeof CANONICAL_CHAINS / sizeof CANONICAL_CHAINS[0],
                 text + chain_at, chain_len)) {
        return out;
    }
    if (axis_len != 0 && !in_list(CANONICAL_AXES, sizeof CANONICAL_AXES / sizeof CANONICAL_AXES[0],
                                  text + axis_at, axis_len)) {
        return out;
    }

    out.valid = true;
    out.chain = sk_arena_copy(arena, text + chain_at, chain_len);
    out.link = sk_arena_copy(arena, text + link_at, link_len);
    out.axis = axis_len == 0 ? NULL : sk_arena_copy(arena, text + axis_at, axis_len);
    out.has_index = has_index;
    if (has_index) {
        out.index = strtod(sk_arena_copy(arena, text + index_at, index_len), NULL);
    }
    return out;
}

sk_role sk_role_parse(sk_arena *arena, const sk_value *role)
{
    /* `if (!role) return null` — a null, an absent field and an empty string are all falsy, and all
     * three mean the same thing here: no role was declared. */
    size_t len = 0;
    const char *text = sk_as_string(role, &len, "");
    if (sk_kind_of(role) != SK_STRING || len == 0) {
        sk_role out;
        memset(&out, 0, sizeof out);
        return out;
    }
    return sk_role_parse_text(arena, text, len);
}

const char *sk_role_key(sk_arena *arena, const sk_role *role)
{
    sk_buf out;
    sk_buf_init(&out, arena);
    sk_buf_append_str(&out, role->chain);
    sk_buf_append_char(&out, '.');
    sk_buf_append_str(&out, role->link);
    if (role->axis != NULL) {
        sk_buf_append_char(&out, '.');
        sk_buf_append_str(&out, role->axis);
    }
    if (role->has_index) {
        sk_buf_append_char(&out, '#');
        sk_buf_append_str(&out, sk_js_number_to_string(arena, role->index));
    }
    return out.bytes == NULL ? "" : out.bytes;
}

const sk_value *sk_roles_vocabulary_case(sk_arena *arena, const sk_value *input, const char **error)
{
    (void)error;
    sk_role parsed = sk_role_parse(arena, sk_get(input, "role"));
    if (!parsed.valid) {
        return sk_null();
    }
    sk_value *out = sk_object(arena);
    sk_set(arena, out, "chain", sk_string(arena, parsed.chain));
    sk_set(arena, out, "link", sk_string(arena, parsed.link));
    sk_set(arena, out, "axis", parsed.axis == NULL ? sk_null() : sk_string(arena, parsed.axis));
    sk_set(arena, out, "index",
           parsed.has_index ? sk_number(arena, parsed.index) : sk_null());
    sk_set(arena, out, "key", sk_string(arena, sk_role_key(arena, &parsed)));
    return out;
}

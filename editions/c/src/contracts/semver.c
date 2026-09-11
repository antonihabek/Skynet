#include "skynet_os/semver.h"

#include <stdlib.h>
#include <string.h>

#include "skynet_os/js.h"
#include "skynet_os/write.h"

/* A slice of somebody else's bytes. The reference works in JavaScript strings and slices them
 * freely; this is the same thing without a copy per slice. */
typedef struct {
    const char *bytes;
    size_t len;
} slice;

static slice of(const char *bytes, size_t len)
{
    slice out = {bytes, len};
    return out;
}

static bool ascii_digit(char byte)
{
    return byte >= '0' && byte <= '9';
}

/* `[0-9A-Za-z.-]`, the prerelease character class. */
static bool prerelease_byte(char byte)
{
    return ascii_digit(byte) || (byte >= 'A' && byte <= 'Z') || (byte >= 'a' && byte <= 'z') ||
           byte == '.' || byte == '-';
}

/* A maximal run of ASCII digits at `at`, or zero length. `\d` in the reference's regex has no `u`
 * flag and no Unicode property, so it is ASCII 0-9 and nothing else — which is what makes the corpus
 * case written in Arabic-Indic digits unreadable rather than a version. */
static size_t digit_run(slice text, size_t at)
{
    size_t end = at;
    while (end < text.len && ascii_digit(text.bytes[end])) {
        end += 1;
    }
    return end - at;
}

/* `Number(...)` over a run of ASCII digits: the double nearest the value the digits spell. */
static double number_of(sk_arena *arena, slice text, size_t at, size_t len)
{
    char *copy = sk_arena_copy(arena, text.bytes + at, len);
    return strtod(copy, NULL);
}

sk_version sk_semver_parse(sk_arena *arena, const char *text, size_t len)
{
    sk_version out;
    memset(&out, 0, sizeof out);

    size_t trimmed_len = 0;
    const char *trimmed = sk_js_trim(arena, text, len, &trimmed_len);
    slice input = of(trimmed, trimmed_len);

    /* `^(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?$`, anchored at both ends. Digits and the point are
     * disjoint character classes, so a greedy run followed by a required point needs no backtracking:
     * the maximal run is the only run that can be followed by one. */
    size_t at = 0;
    size_t major_len = digit_run(input, at);
    if (major_len == 0) {
        return out;
    }
    size_t major_at = at;
    at += major_len;
    if (at >= input.len || input.bytes[at] != '.') {
        return out;
    }
    at += 1;
    size_t minor_len = digit_run(input, at);
    if (minor_len == 0) {
        return out;
    }
    size_t minor_at = at;
    at += minor_len;
    if (at >= input.len || input.bytes[at] != '.') {
        return out;
    }
    at += 1;
    size_t patch_len = digit_run(input, at);
    if (patch_len == 0) {
        return out;
    }
    size_t patch_at = at;
    at += patch_len;

    const char *prerelease = NULL;
    if (at < input.len) {
        if (input.bytes[at] != '-') {
            return out;
        }
        at += 1;
        size_t start = at;
        while (at < input.len && prerelease_byte(input.bytes[at])) {
            at += 1;
        }
        if (at == start || at != input.len) {
            return out;
        }
        prerelease = sk_arena_copy(arena, input.bytes + start, at - start);
    }

    out.valid = true;
    out.major = number_of(arena, input, major_at, major_len);
    out.minor = number_of(arena, input, minor_at, minor_len);
    out.patch = number_of(arena, input, patch_at, patch_len);
    out.prerelease = prerelease;
    out.raw = sk_arena_copy(arena, trimmed, trimmed_len);
    return out;
}

bool sk_semver_valid_version(sk_arena *arena, const char *text, size_t len)
{
    return sk_semver_parse(arena, text, len).valid;
}

/* Is every byte of this identifier an ASCII digit? The reference's `/^\d+$/` over a `.`-separated
 * part, which decides whether the part compares numerically or as a string. */
static bool all_digits(slice part)
{
    if (part.len == 0) {
        return false;
    }
    for (size_t at = 0; at < part.len; at += 1) {
        if (!ascii_digit(part.bytes[at])) {
            return false;
        }
    }
    return true;
}

/* `a.split('.')[index]`, with `found` false past the end — the reference's `undefined`, which the
 * comparison reads as "this side ran out of identifiers". */
static slice dot_part(slice text, size_t index, bool *found)
{
    size_t at = 0;
    size_t which = 0;
    while (true) {
        size_t start = at;
        while (at < text.len && text.bytes[at] != '.') {
            at += 1;
        }
        if (which == index) {
            *found = true;
            return of(text.bytes + start, at - start);
        }
        if (at >= text.len) {
            *found = false;
            return of(text.bytes, 0);
        }
        at += 1;
        which += 1;
    }
}

static size_t dot_count(slice text)
{
    size_t parts = 1;
    for (size_t at = 0; at < text.len; at += 1) {
        if (text.bytes[at] == '.') {
            parts += 1;
        }
    }
    return parts;
}

/* JavaScript's `<` over two strings, which compares UTF-16 code units. Every byte a prerelease may
 * hold is ASCII, so a byte comparison is the same comparison here — and that is a property of the
 * grammar rather than a convenience, which is why it is written down. */
static int string_order(slice a, slice b)
{
    size_t shared = a.len < b.len ? a.len : b.len;
    int order = memcmp(a.bytes, b.bytes, shared);
    if (order != 0) {
        return order < 0 ? -1 : 1;
    }
    if (a.len == b.len) {
        return 0;
    }
    return a.len < b.len ? -1 : 1;
}

/* Prerelease identifiers per semver §11: numeric below alphanumeric, and a longer list wins a tie. */
static int compare_prerelease(sk_arena *arena, const char *a, const char *b)
{
    if (a == NULL && b == NULL) {
        return 0;
    }
    /* A version without a prerelease outranks one with it. */
    if (a == NULL) {
        return 1;
    }
    if (b == NULL) {
        return -1;
    }
    slice left = of(a, strlen(a));
    slice right = of(b, strlen(b));
    if (left.len == right.len && memcmp(left.bytes, right.bytes, left.len) == 0) {
        return 0;
    }

    size_t left_parts = dot_count(left);
    size_t right_parts = dot_count(right);
    size_t length = left_parts > right_parts ? left_parts : right_parts;
    for (size_t index = 0; index < length; index += 1) {
        bool left_found = false;
        bool right_found = false;
        slice ai = dot_part(left, index, &left_found);
        slice bi = dot_part(right, index, &right_found);
        if (!left_found) {
            return -1;
        }
        if (!right_found) {
            return 1;
        }
        bool left_numeric = all_digits(ai);
        bool right_numeric = all_digits(bi);
        if (left_numeric && right_numeric) {
            double difference = number_of(arena, ai, 0, ai.len) - number_of(arena, bi, 0, bi.len);
            if (difference != 0) {
                return difference < 0 ? -1 : 1;
            }
            continue;
        }
        if (left_numeric) {
            return -1;
        }
        if (right_numeric) {
            return 1;
        }
        int order = string_order(ai, bi);
        if (order != 0) {
            return order;
        }
    }
    return 0;
}

int sk_semver_compare(const sk_version *a, const sk_version *b)
{
    /* An unparseable side compares equal, which is the reference's answer and is the reason
     * `package.ts` keeps its own comparator: a strict parser in that position would silently admit a
     * package onto an older OS. */
    if (!a->valid || !b->valid) {
        return 0;
    }
    if (a->major != b->major) {
        return a->major < b->major ? -1 : 1;
    }
    if (a->minor != b->minor) {
        return a->minor < b->minor ? -1 : 1;
    }
    if (a->patch != b->patch) {
        return a->patch < b->patch ? -1 : 1;
    }
    /* An arena is not needed for the numeric comparison of digit runs this reaches, but the signature
     * is the reference's: `comparePrerelease` converts identifiers with `Number(...)`. */
    sk_arena *scratch = sk_arena_new();
    int order = compare_prerelease(scratch, a->prerelease, b->prerelease);
    sk_arena_free(scratch);
    return order;
}

/* ── Ranges ────────────────────────────────────────────────────────────────── */

typedef enum { OP_EQ, OP_GTE, OP_GT, OP_LTE, OP_LT, OP_CARET, OP_TILDE } comparator_op;

typedef struct {
    comparator_op op;
    sk_version version;
} comparator;

/*
 * The operator at `at`, and its width.
 *
 * The alternation order is the reference's and it decides two corpus cases: `>=` is tried before `>`,
 * so `>=1.0.0` is one comparator rather than `>` applied to `=1.0.0`. A token with no operator is an
 * equality comparator.
 */
static bool operator_at(slice text, size_t at, comparator_op *op, size_t *width)
{
    static const struct {
        const char *spelling;
        comparator_op op;
    } OPERATORS[] = {
        {">=", OP_GTE}, {"<=", OP_LTE}, {">", OP_GT}, {"<", OP_LT},
        {"^", OP_CARET}, {"~", OP_TILDE}, {"=", OP_EQ},
    };
    for (size_t which = 0; which < sizeof OPERATORS / sizeof OPERATORS[0]; which += 1) {
        size_t len = strlen(OPERATORS[which].spelling);
        if (text.len - at >= len && memcmp(text.bytes + at, OPERATORS[which].spelling, len) == 0) {
            *op = OPERATORS[which].op;
            *width = len;
            return true;
        }
    }
    return false;
}

/* `\s*` — JavaScript's whitespace class, which is the same set `trim()` removes and includes U+FEFF.
 * A range written with one before its version is a corpus case. */
static size_t skip_space(slice text, size_t at)
{
    while (at < text.len) {
        size_t next = at;
        if (!sk_js_is_space(sk_utf8_next(text.bytes, text.len, &next))) {
            break;
        }
        at = next;
    }
    return at;
}

/*
 * One token of the reference's tokenizer:
 *
 *   /(?:>=|<=|>|<|\^|~|=)?\s*\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?/g
 *
 * Returns false when nothing matches at `at`, which is when the caller advances one byte and tries
 * again — the way a global regex walks a string. Tokenizing on the operator rather than on whitespace
 * is what makes `>= 1.2.0` and `>=1.2.0` the same range, and `>=1.0.0<2.0.0` two comparators.
 */
static bool token_at(slice text, size_t at, size_t *end)
{
    size_t scan = at;
    comparator_op op = OP_EQ;
    size_t width = 0;
    if (operator_at(text, scan, &op, &width)) {
        scan += width;
    }
    scan = skip_space(text, scan);
    size_t major = digit_run(text, scan);
    if (major == 0) {
        return false;
    }
    scan += major;
    if (scan >= text.len || text.bytes[scan] != '.') {
        return false;
    }
    scan += 1;
    size_t minor = digit_run(text, scan);
    if (minor == 0) {
        return false;
    }
    scan += minor;
    if (scan >= text.len || text.bytes[scan] != '.') {
        return false;
    }
    scan += 1;
    size_t patch = digit_run(text, scan);
    if (patch == 0) {
        return false;
    }
    scan += patch;
    if (scan < text.len && text.bytes[scan] == '-') {
        size_t start = scan + 1;
        size_t run = start;
        while (run < text.len && prerelease_byte(text.bytes[run])) {
            run += 1;
        }
        /* The prerelease group is optional, so a lone `-` with nothing readable after it simply is
         * not part of the token rather than failing it. */
        if (run > start) {
            scan = run;
        }
    }
    *end = scan;
    return true;
}

/* `parseComparator`: the token's own operator, then `parseVersion` over the rest. */
static bool parse_comparator(sk_arena *arena, slice token, comparator *out)
{
    size_t trimmed_len = 0;
    const char *trimmed_bytes = sk_js_trim(arena, token.bytes, token.len, &trimmed_len);
    slice trimmed = of(trimmed_bytes, trimmed_len);

    size_t at = 0;
    comparator_op op = OP_EQ;
    size_t width = 0;
    if (operator_at(trimmed, at, &op, &width)) {
        at += width;
    }
    at = skip_space(trimmed, at);
    /* `(.+)` requires at least one character, and `parseVersion` trims again. */
    if (at >= trimmed.len) {
        return false;
    }
    sk_version version = sk_semver_parse(arena, trimmed.bytes + at, trimmed.len - at);
    if (!version.valid) {
        return false;
    }
    out->op = op;
    out->version = version;
    return true;
}

static bool satisfies_comparator(const sk_version *target, const comparator *against)
{
    int order = sk_semver_compare(target, &against->version);
    switch (against->op) {
    case OP_GTE:
        return order >= 0;
    case OP_GT:
        return order > 0;
    case OP_LTE:
        return order <= 0;
    case OP_LT:
        return order < 0;
    case OP_EQ:
        return order == 0;
    case OP_CARET: {
        /* Caret: compatible within the leftmost non-zero component. */
        if (order < 0) {
            return false;
        }
        const sk_version *base = &against->version;
        if (base->major > 0) {
            return target->major == base->major;
        }
        if (base->minor > 0) {
            return target->major == 0 && target->minor == base->minor;
        }
        return target->major == 0 && target->minor == 0 && target->patch == base->patch;
    }
    case OP_TILDE:
        /* Tilde: patch-level changes within the same minor. */
        if (order < 0) {
            return false;
        }
        return target->major == against->version.major && target->minor == against->version.minor;
    }
    return false;
}

/*
 * A range, parsed into its `||` groups.
 *
 * `wildcard` true means every version satisfies it and there is nothing to compare against. A group
 * that does not parse is marked unreadable rather than failing the whole range, which is what keeps
 * one unreadable alternative from invalidating the readable ones — and `sk_semver_valid_range` is the
 * caller that wants the stricter answer, which it gets by asking about every group.
 */
#define SK_MAX_GROUPS 64
#define SK_MAX_COMPARATORS 64

typedef struct {
    bool readable;
    comparator comparators[SK_MAX_COMPARATORS];
    size_t count;
} range_group;

typedef struct {
    bool wildcard;
    range_group groups[SK_MAX_GROUPS];
    size_t count;
    /* A range with more groups or comparators than the arrays hold. No corpus case reaches it and no
     * range a publisher would write does; it is reported rather than truncated, because a truncated
     * range is a *different* range that would answer satisfaction wrongly and silently. */
    bool overflowed;
} parsed_range;

static parsed_range parse_range(sk_arena *arena, const char *range, size_t range_len)
{
    parsed_range out;
    memset(&out, 0, sizeof out);

    size_t trimmed_len = 0;
    const char *trimmed_bytes = sk_js_trim(arena, range, range_len, &trimmed_len);
    slice trimmed = of(trimmed_bytes, trimmed_len);

    if (trimmed.len == 0 || (trimmed.len == 1 && trimmed.bytes[0] == '*')) {
        out.wildcard = true;
        return out;
    }
    size_t lowered_len = 0;
    const char *lowered = sk_js_lower(arena, trimmed.bytes, trimmed.len, &lowered_len);
    if (lowered_len == 3 && memcmp(lowered, "any", 3) == 0) {
        out.wildcard = true;
        return out;
    }

    /* `split('||')`, which yields one more piece than there are separators and can yield empty ones. */
    size_t group_start = 0;
    size_t at = 0;
    while (true) {
        bool last = false;
        size_t group_end = trimmed.len;
        while (at + 1 < trimmed.len) {
            if (trimmed.bytes[at] == '|' && trimmed.bytes[at + 1] == '|') {
                group_end = at;
                break;
            }
            at += 1;
        }
        if (at + 1 >= trimmed.len) {
            group_end = trimmed.len;
            last = true;
        }

        if (out.count >= SK_MAX_GROUPS) {
            out.overflowed = true;
            return out;
        }
        range_group *group = &out.groups[out.count];
        out.count += 1;
        group->readable = false;
        group->count = 0;

        size_t piece_len = 0;
        const char *piece_bytes = sk_js_trim(arena, trimmed.bytes + group_start,
                                            group_end - group_start, &piece_len);
        slice piece = of(piece_bytes, piece_len);

        /* The global tokenizer: at each position, match or advance one byte. */
        size_t scan = 0;
        bool unreadable = false;
        while (scan < piece.len) {
            size_t end = 0;
            if (!token_at(piece, scan, &end)) {
                scan += 1;
                continue;
            }
            if (group->count >= SK_MAX_COMPARATORS) {
                out.overflowed = true;
                return out;
            }
            if (!parse_comparator(arena, of(piece.bytes + scan, end - scan),
                                  &group->comparators[group->count])) {
                /* A token the tokenizer produced and the comparator parser refused makes the whole
                 * group unreadable, exactly as `comparators.some(c => c === null)` does. */
                unreadable = true;
                break;
            }
            group->count += 1;
            scan = end;
        }
        group->readable = !unreadable && group->count > 0;

        if (last) {
            break;
        }
        at += 2;
        group_start = at;
    }
    return out;
}

bool sk_semver_satisfies(sk_arena *arena, const char *version, size_t version_len, const char *range,
                         size_t range_len)
{
    sk_version target = sk_semver_parse(arena, version, version_len);
    if (!target.valid) {
        return false;
    }
    parsed_range groups = parse_range(arena, range, range_len);
    if (groups.wildcard) {
        return true;
    }
    if (groups.overflowed) {
        return false;
    }
    for (size_t which = 0; which < groups.count; which += 1) {
        const range_group *group = &groups.groups[which];
        if (!group->readable) {
            continue;
        }
        bool all = true;
        for (size_t index = 0; index < group->count; index += 1) {
            if (!satisfies_comparator(&target, &group->comparators[index])) {
                all = false;
                break;
            }
        }
        if (all) {
            return true;
        }
    }
    return false;
}

bool sk_semver_valid_range(sk_arena *arena, const char *range, size_t range_len)
{
    parsed_range groups = parse_range(arena, range, range_len);
    if (groups.wildcard) {
        return true;
    }
    if (groups.overflowed) {
        return false;
    }
    for (size_t which = 0; which < groups.count; which += 1) {
        if (!groups.groups[which].readable) {
            return false;
        }
    }
    return true;
}

/* ── The shapes, from `reference/typescript/shape.ts` ──────────────────────── */

static const sk_value *shape_version(sk_arena *arena, const sk_version *version)
{
    if (!version->valid) {
        return sk_null();
    }
    sk_value *out = sk_object(arena);
    sk_set(arena, out, "major", sk_number(arena, version->major));
    sk_set(arena, out, "minor", sk_number(arena, version->minor));
    sk_set(arena, out, "patch", sk_number(arena, version->patch));
    sk_set(arena, out, "prerelease",
           version->prerelease == NULL ? sk_null() : sk_string(arena, version->prerelease));
    sk_set(arena, out, "raw", sk_string(arena, version->raw));
    return out;
}

const sk_value *sk_semver_satisfies_case(sk_arena *arena, const sk_value *input, const char **error)
{
    (void)error;
    size_t version_len = 0;
    size_t range_len = 0;
    const char *version = sk_as_string(sk_get(input, "version"), &version_len, "");
    const char *range = sk_as_string(sk_get(input, "range"), &range_len, "");

    sk_value *out = sk_object(arena);
    sk_set(arena, out, "version", sk_string_len(arena, version, version_len));
    sk_set(arena, out, "range", sk_string_len(arena, range, range_len));
    sk_set(arena, out, "satisfies",
           sk_bool(sk_semver_satisfies(arena, version, version_len, range, range_len)));
    /* Validity beside satisfaction, because the two are different facts: pinning only the boolean
     * would let an edition agree about every satisfaction and disagree about which ranges it could
     * read at all — the difference between refusing a skill and telling its publisher why. */
    sk_set(arena, out, "valid_version", sk_bool(sk_semver_valid_version(arena, version, version_len)));
    sk_set(arena, out, "valid_range", sk_bool(sk_semver_valid_range(arena, range, range_len)));
    return out;
}

const sk_value *sk_semver_compare_case(sk_arena *arena, const sk_value *input, const char **error)
{
    (void)error;
    size_t a_len = 0;
    size_t b_len = 0;
    const char *a_text = sk_as_string(sk_get(input, "a"), &a_len, "");
    const char *b_text = sk_as_string(sk_get(input, "b"), &b_len, "");
    sk_version a = sk_semver_parse(arena, a_text, a_len);
    sk_version b = sk_semver_parse(arena, b_text, b_len);

    sk_value *out = sk_object(arena);
    sk_set(arena, out, "compare", sk_number(arena, (double)sk_semver_compare(&a, &b)));
    sk_set(arena, out, "a", shape_version(arena, &a));
    sk_set(arena, out, "b", shape_version(arena, &b));
    return out;
}

#include "skynet_os/js.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "skynet_os/write.h"

/* No full case mapping in Unicode is longer than this, and `tools/unicode-case.ts` throws rather
 * than truncating above it. */
#define SK_CASE_WIDTH 3

typedef struct {
    uint32_t first;
    uint32_t last;
} sk_code_range;

#include "unicode_case.inc"

const char *sk_js_unicode_version(void)
{
    return SK_UNICODE_VERSION;
}

/* ── UTF-8 ─────────────────────────────────────────────────────────────────── */

uint32_t sk_utf8_next(const char *text, size_t len, size_t *at)
{
    unsigned char lead = (unsigned char)text[*at];
    size_t remaining = len - *at;
    if (lead < 0x80) {
        *at += 1;
        return lead;
    }
    size_t width = 0;
    uint32_t code = 0;
    if ((lead & 0xE0) == 0xC0) {
        width = 2;
        code = lead & 0x1Fu;
    } else if ((lead & 0xF0) == 0xE0) {
        width = 3;
        code = lead & 0x0Fu;
    } else if ((lead & 0xF8) == 0xF0) {
        width = 4;
        code = lead & 0x07u;
    } else {
        *at += 1;
        return 0xFFFD;
    }
    if (remaining < width) {
        *at += 1;
        return 0xFFFD;
    }
    for (size_t which = 1; which < width; which += 1) {
        unsigned char continuation = (unsigned char)text[*at + which];
        if ((continuation & 0xC0) != 0x80) {
            *at += 1;
            return 0xFFFD;
        }
        code = (code << 6) | (continuation & 0x3Fu);
    }
    *at += width;
    return code;
}

size_t sk_utf8_encode(uint32_t code, char *out)
{
    if (code < 0x80) {
        out[0] = (char)code;
        return 1;
    }
    if (code < 0x800) {
        out[0] = (char)(0xC0 | (code >> 6));
        out[1] = (char)(0x80 | (code & 0x3F));
        return 2;
    }
    if (code < 0x10000) {
        out[0] = (char)(0xE0 | (code >> 12));
        out[1] = (char)(0x80 | ((code >> 6) & 0x3F));
        out[2] = (char)(0x80 | (code & 0x3F));
        return 3;
    }
    out[0] = (char)(0xF0 | (code >> 18));
    out[1] = (char)(0x80 | ((code >> 12) & 0x3F));
    out[2] = (char)(0x80 | ((code >> 6) & 0x3F));
    out[3] = (char)(0x80 | (code & 0x3F));
    return 4;
}

/* ── Whitespace ────────────────────────────────────────────────────────────── */

bool sk_js_is_space(uint32_t code)
{
    /*
     * ECMAScript's WhiteSpace production plus LineTerminator, which together are what `trim()`
     * removes. Written out rather than derived from a Unicode property, because the set is the
     * *language's* and not `\p{White_Space}`: U+FEFF is whitespace to JavaScript and is not
     * White_Space in Unicode, and U+0085 is the other way round.
     */
    switch (code) {
    case 0x0009: /* tab */
    case 0x000A: /* line feed */
    case 0x000B: /* vertical tab */
    case 0x000C: /* form feed */
    case 0x000D: /* carriage return */
    case 0x0020: /* space */
    case 0x00A0: /* no-break space */
    case 0x1680: /* ogham space mark */
    case 0x2028: /* line separator */
    case 0x2029: /* paragraph separator */
    case 0x202F: /* narrow no-break space */
    case 0x205F: /* medium mathematical space */
    case 0x3000: /* ideographic space */
    case 0xFEFF: /* zero width no-break space */
        return true;
    default:
        /* U+2000 to U+200A, the en quad through the hair space. */
        return code >= 0x2000 && code <= 0x200A;
    }
}

const char *sk_js_trim(sk_arena *arena, const char *text, size_t len, size_t *out_len)
{
    size_t start = 0;
    while (start < len) {
        size_t next = start;
        if (!sk_js_is_space(sk_utf8_next(text, len, &next))) {
            break;
        }
        start = next;
    }
    /*
     * The end is found by walking forward from `start` and remembering the last position that was not
     * the beginning of a run of whitespace. Walking backwards through UTF-8 is possible but needs its
     * own rules about continuation bytes, and a second decoder is a second chance to disagree with
     * the first.
     */
    size_t end = start;
    size_t at = start;
    while (at < len) {
        size_t next = at;
        uint32_t code = sk_utf8_next(text, len, &next);
        if (!sk_js_is_space(code)) {
            end = next;
        }
        at = next;
    }
    if (out_len != NULL) {
        *out_len = end - start;
    }
    return sk_arena_copy(arena, text + start, end - start);
}

/* ── Case ──────────────────────────────────────────────────────────────────── */

static bool in_ranges(const sk_code_range *ranges, size_t count, uint32_t code)
{
    size_t low = 0;
    size_t high = count;
    while (low < high) {
        size_t middle = low + (high - low) / 2;
        if (code < ranges[middle].first) {
            high = middle;
        } else if (code > ranges[middle].last) {
            low = middle + 1;
        } else {
            return true;
        }
    }
    return false;
}

static bool is_cased(uint32_t code)
{
    return in_ranges(cased_ranges, sizeof cased_ranges / sizeof cased_ranges[0], code);
}

static bool is_case_ignorable(uint32_t code)
{
    return in_ranges(case_ignorable_ranges,
                     sizeof case_ignorable_ranges / sizeof case_ignorable_ranges[0], code);
}

/* The mapping for `code`, or NULL when it maps to itself. `from` is ascending, so this is a binary
 * search; a table that lost its order would still answer most lookups, which is why the self-check
 * asserts the order rather than trusting it. */
static const uint32_t *lookup(const uint32_t *from, const uint32_t (*to)[SK_CASE_WIDTH], size_t count,
                              uint32_t code)
{
    size_t low = 0;
    size_t high = count;
    while (low < high) {
        size_t middle = low + (high - low) / 2;
        if (code < from[middle]) {
            high = middle;
        } else if (code > from[middle]) {
            low = middle + 1;
        } else {
            return to[middle];
        }
    }
    return NULL;
}

static void append_code(sk_buf *out, uint32_t code)
{
    char encoded[4];
    size_t width = sk_utf8_encode(code, encoded);
    sk_buf_append(out, encoded, width);
}

static void append_mapping(sk_buf *out, const uint32_t *mapping)
{
    for (size_t which = 0; which < SK_CASE_WIDTH && mapping[which] != 0; which += 1) {
        append_code(out, mapping[which]);
    }
}

bool sk_js_tables_sorted(void)
{
    for (size_t at = 1; at < sizeof upper_from / sizeof upper_from[0]; at += 1) {
        if (upper_from[at - 1] >= upper_from[at]) {
            return false;
        }
    }
    for (size_t at = 1; at < sizeof lower_from / sizeof lower_from[0]; at += 1) {
        if (lower_from[at - 1] >= lower_from[at]) {
            return false;
        }
    }
    for (size_t at = 1; at < sizeof cased_ranges / sizeof cased_ranges[0]; at += 1) {
        if (cased_ranges[at - 1].last >= cased_ranges[at].first) {
            return false;
        }
    }
    for (size_t at = 1; at < sizeof case_ignorable_ranges / sizeof case_ignorable_ranges[0]; at += 1) {
        if (case_ignorable_ranges[at - 1].last >= case_ignorable_ranges[at].first) {
            return false;
        }
    }
    return true;
}

const char *sk_js_upper(sk_arena *arena, const char *text, size_t len, size_t *out_len)
{
    sk_buf out;
    sk_buf_init(&out, arena);
    size_t at = 0;
    while (at < len) {
        uint32_t code = sk_utf8_next(text, len, &at);
        const uint32_t *mapping =
            lookup(upper_from, upper_to, sizeof upper_from / sizeof upper_from[0], code);
        if (mapping == NULL) {
            append_code(&out, code);
        } else {
            append_mapping(&out, mapping);
        }
    }
    if (out_len != NULL) {
        *out_len = out.len;
    }
    return out.bytes == NULL ? "" : out.bytes;
}

#define SK_CAPITAL_SIGMA 0x03A3u
#define SK_SMALL_SIGMA 0x03C3u
#define SK_FINAL_SIGMA 0x03C2u

/*
 * Unicode's Final_Sigma condition, which is the one context rule `toLowerCase` applies.
 *
 * Before: a cased character, skipping case-ignorable ones. After: *not* a cased character, skipping
 * case-ignorable ones. A character that is both cased and case-ignorable is **skipped**, because the
 * condition skips Case_Ignorable first — and that is the class `tools/unicode-case.ts` picks
 * representatives for, so the vectors are what says whether this reading is the engine's.
 */
static bool scan_cased(const char *text, size_t len, size_t from, bool forward)
{
    if (forward) {
        size_t at = from;
        while (at < len) {
            uint32_t code = sk_utf8_next(text, len, &at);
            if (is_case_ignorable(code)) {
                continue;
            }
            return is_cased(code);
        }
        return false;
    }
    /*
     * Backwards, without a backwards decoder: walk forward from the start recording the last
     * non-ignorable code point before `from`. Same reason `sk_js_trim` walks forward — one decoder,
     * one set of rules about continuation bytes.
     */
    bool found = false;
    bool cased = false;
    size_t at = 0;
    while (at < from && at < len) {
        uint32_t code = sk_utf8_next(text, len, &at);
        if (at > from) {
            break;
        }
        if (is_case_ignorable(code)) {
            continue;
        }
        found = true;
        cased = is_cased(code);
    }
    return found && cased;
}

const char *sk_js_lower(sk_arena *arena, const char *text, size_t len, size_t *out_len)
{
    sk_buf out;
    sk_buf_init(&out, arena);
    size_t at = 0;
    while (at < len) {
        size_t start = at;
        uint32_t code = sk_utf8_next(text, len, &at);
        if (code == SK_CAPITAL_SIGMA) {
            bool before = scan_cased(text, len, start, false);
            bool after = scan_cased(text, len, at, true);
            append_code(&out, before && !after ? SK_FINAL_SIGMA : SK_SMALL_SIGMA);
            continue;
        }
        const uint32_t *mapping =
            lookup(lower_from, lower_to, sizeof lower_from / sizeof lower_from[0], code);
        if (mapping == NULL) {
            append_code(&out, code);
        } else {
            append_mapping(&out, mapping);
        }
    }
    if (out_len != NULL) {
        *out_len = out.len;
    }
    return out.bytes == NULL ? "" : out.bytes;
}

/* ── Numbers and ToString ──────────────────────────────────────────────────── */

const char *sk_js_number_to_string(sk_arena *arena, double number)
{
    if (isnan(number)) {
        return "NaN";
    }
    if (number == 0.0) {
        return "0";
    }
    if (isinf(number)) {
        return number < 0 ? "-Infinity" : "Infinity";
    }

    /*
     * ECMAScript's Number::toString, which is stated over three integers: `s`, `k` and `n`, where `s`
     * has `k` digits, no trailing zero when `k > 1`, and `s * 10^(n-k)` is the value. The shortest
     * `%.*e` that reads back as this double gives all three at once — its digits are `s`, their count
     * is `k`, and its exponent is `n - 1`.
     */
    char rendered[SK_NUMBER_MAX];
    int found = 17;
    for (int precision = 1; precision <= 17; precision += 1) {
        (void)snprintf(rendered, sizeof rendered, "%.*e", precision - 1, fabs(number));
        if (strtod(rendered, NULL) == fabs(number)) {
            found = precision;
            break;
        }
    }
    (void)snprintf(rendered, sizeof rendered, "%.*e", found - 1, fabs(number));

    char digits[SK_NUMBER_MAX];
    size_t k = 0;
    const char *marker = strchr(rendered, 'e');
    for (const char *scan = rendered; scan < marker; scan += 1) {
        if (*scan != '.') {
            digits[k] = *scan;
            k += 1;
        }
    }
    /* `%.*e` pads with zeros where a shorter rendering would have done; ECMAScript's `s` carries no
     * trailing zero, and the rules below count digits. */
    while (k > 1 && digits[k - 1] == '0') {
        k -= 1;
    }
    digits[k] = '\0';
    long n = strtol(marker + 1, NULL, 10) + 1;

    sk_buf out;
    sk_buf_init(&out, arena);
    if (number < 0) {
        sk_buf_append_char(&out, '-');
    }
    if ((long)k <= n && n <= 21) {
        /* The digits, then `n - k` zeros. */
        sk_buf_append(&out, digits, k);
        for (long zero = 0; zero < n - (long)k; zero += 1) {
            sk_buf_append_char(&out, '0');
        }
    } else if (0 < n && n <= 21) {
        /* A point after `n` digits. */
        sk_buf_append(&out, digits, (size_t)n);
        sk_buf_append_char(&out, '.');
        sk_buf_append(&out, digits + n, k - (size_t)n);
    } else if (-6 < n && n <= 0) {
        sk_buf_append_str(&out, "0.");
        for (long zero = 0; zero < -n; zero += 1) {
            sk_buf_append_char(&out, '0');
        }
        sk_buf_append(&out, digits, k);
    } else {
        /* Exponential — which is the whole reason `canonical.ts` does not use this function. */
        sk_buf_append(&out, digits, 1);
        if (k > 1) {
            sk_buf_append_char(&out, '.');
            sk_buf_append(&out, digits + 1, k - 1);
        }
        char exponent[32];
        (void)snprintf(exponent, sizeof exponent, "e%s%ld", n - 1 >= 0 ? "+" : "-",
                       n - 1 >= 0 ? n - 1 : -(n - 1));
        sk_buf_append_str(&out, exponent);
    }
    return out.bytes == NULL ? "" : out.bytes;
}

/* A run of digits in `base`, or a negative count when there is none. */
static bool digits_in_base(const char *text, size_t len, int base, double *out)
{
    if (len == 0) {
        return false;
    }
    double value = 0;
    for (size_t at = 0; at < len; at += 1) {
        char byte = text[at];
        int digit = 0;
        if (byte >= '0' && byte <= '9') {
            digit = byte - '0';
        } else if (byte >= 'a' && byte <= 'z') {
            digit = byte - 'a' + 10;
        } else if (byte >= 'A' && byte <= 'Z') {
            digit = byte - 'A' + 10;
        } else {
            return false;
        }
        if (digit >= base) {
            return false;
        }
        value = value * (double)base + (double)digit;
    }
    *out = value;
    return true;
}

double sk_js_string_to_number(sk_arena *arena, const char *text, size_t len)
{
    size_t trimmed_len = 0;
    const char *trimmed = sk_js_trim(arena, text, len, &trimmed_len);
    /* StringNumericLiteral of nothing is zero, which is the answer that surprises people and the
     * reason the reference's `toNumber` tests for a blank string before converting. */
    if (trimmed_len == 0) {
        return 0.0;
    }

    /*
     * A non-decimal integer literal carries no sign in the grammar, so `-0x10` is NaN where `-16` is
     * -16. This is checked before the sign is consumed, which is what makes that true.
     */
    if (trimmed_len > 2 && trimmed[0] == '0') {
        int base = 0;
        switch (trimmed[1]) {
        case 'x': case 'X': base = 16; break;
        case 'o': case 'O': base = 8; break;
        case 'b': case 'B': base = 2; break;
        default: base = 0; break;
        }
        if (base != 0) {
            double value = 0;
            if (digits_in_base(trimmed + 2, trimmed_len - 2, base, &value)) {
                return value;
            }
            return (double)NAN;
        }
    }

    size_t at = 0;
    double sign = 1.0;
    if (trimmed[at] == '+' || trimmed[at] == '-') {
        sign = trimmed[at] == '-' ? -1.0 : 1.0;
        at += 1;
    }

    /* Spelled with that capital and no other: `Infinity`, never `inf` or `INFINITY`, both of which
     * `strtod` accepts and the engine does not. */
    static const char INFINITY_WORD[] = "Infinity";
    size_t word_len = sizeof INFINITY_WORD - 1;
    if (trimmed_len - at == word_len && memcmp(trimmed + at, INFINITY_WORD, word_len) == 0) {
        return sign * (double)INFINITY;
    }

    /*
     * StrUnsignedDecimalLiteral: digits with an optional fraction and an optional exponent, or a
     * fraction with no integer part. Validated here and converted by `strtod`, which is the correctly
     * rounded conversion the engine also performs — the validation is what keeps `strtod`'s wider
     * grammar (`inf`, `nan`, a hex float, a leading space) out.
     */
    size_t scan = at;
    size_t integer_digits = 0;
    while (scan < trimmed_len && trimmed[scan] >= '0' && trimmed[scan] <= '9') {
        scan += 1;
        integer_digits += 1;
    }
    size_t fraction_digits = 0;
    if (scan < trimmed_len && trimmed[scan] == '.') {
        scan += 1;
        while (scan < trimmed_len && trimmed[scan] >= '0' && trimmed[scan] <= '9') {
            scan += 1;
            fraction_digits += 1;
        }
    }
    if (integer_digits == 0 && fraction_digits == 0) {
        return (double)NAN;
    }
    if (scan < trimmed_len && (trimmed[scan] == 'e' || trimmed[scan] == 'E')) {
        scan += 1;
        if (scan < trimmed_len && (trimmed[scan] == '+' || trimmed[scan] == '-')) {
            scan += 1;
        }
        size_t exponent_digits = 0;
        while (scan < trimmed_len && trimmed[scan] >= '0' && trimmed[scan] <= '9') {
            scan += 1;
            exponent_digits += 1;
        }
        if (exponent_digits == 0) {
            return (double)NAN;
        }
    }
    if (scan != trimmed_len) {
        return (double)NAN;
    }
    char *copy = sk_arena_copy(arena, trimmed, trimmed_len);
    return strtod(copy, NULL);
}

const char *sk_js_to_string(sk_arena *arena, const sk_value *value, bool present)
{
    if (!present) {
        return "undefined";
    }
    switch (sk_kind_of(value)) {
    case SK_NULL:
        return "null";
    case SK_BOOL:
        return sk_as_bool(value, false) ? "true" : "false";
    case SK_NUMBER:
        return sk_js_number_to_string(arena, sk_as_number(value, 0));
    case SK_STRING: {
        size_t len = 0;
        const char *text = sk_as_string(value, &len, "");
        return sk_arena_copy(arena, text, len);
    }
    case SK_ARRAY: {
        /* Array.prototype.join: a null or undefined element is the empty string rather than "null",
         * which is the one place ToString's answer for null is not the word. */
        sk_buf out;
        sk_buf_init(&out, arena);
        for (size_t at = 0; at < sk_len(value); at += 1) {
            if (at > 0) {
                sk_buf_append_char(&out, ',');
            }
            const sk_value *element = sk_at(value, at);
            if (sk_nullish(element)) {
                continue;
            }
            sk_buf_append_str(&out, sk_js_to_string(arena, element, true));
        }
        return out.bytes == NULL ? "" : out.bytes;
    }
    case SK_OBJECT:
        return "[object Object]";
    }
    return "undefined";
}

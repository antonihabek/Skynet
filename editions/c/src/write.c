#include "skynet_os/write.h"

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* 2^53, the point above which an integral double is no longer written as an integer. */
#define SK_INT_SAFE 9007199254740992.0

void sk_buf_init(sk_buf *buf, sk_arena *arena)
{
    buf->arena = arena;
    buf->bytes = NULL;
    buf->len = 0;
    buf->capacity = 0;
}

void sk_buf_append(sk_buf *buf, const char *bytes, size_t len)
{
    if (len == 0) {
        return;
    }
    if (buf->len + len + 1 > buf->capacity) {
        size_t capacity = buf->capacity == 0 ? 256 : buf->capacity;
        while (capacity < buf->len + len + 1) {
            capacity *= 2;
        }
        char *grown = sk_arena_alloc(buf->arena, capacity);
        if (buf->len != 0) {
            memcpy(grown, buf->bytes, buf->len);
        }
        buf->bytes = grown;
        buf->capacity = capacity;
    }
    memcpy(buf->bytes + buf->len, bytes, len);
    buf->len += len;
    buf->bytes[buf->len] = '\0';
}

void sk_buf_append_str(sk_buf *buf, const char *text)
{
    sk_buf_append(buf, text, strlen(text));
}

void sk_buf_append_char(sk_buf *buf, char byte)
{
    sk_buf_append(buf, &byte, 1);
}

/*
 * Turn `-4.4e-01` into `-0.44`.
 *
 * The reference's `exponentialToPositional`, over the one shape `%e` produces: an optional sign, one
 * digit, an optional point and fraction, `e`, a sign, and at least two exponent digits. The exponent
 * is read as a number, so C's two-digit minimum and JavaScript's one-digit rendering are the same
 * input to this.
 */
static void exponential_to_positional(const char *rendered, char *out, size_t out_size)
{
    const char *marker = strchr(rendered, 'e');
    if (marker == NULL) {
        (void)snprintf(out, out_size, "%s", rendered);
        return;
    }

    const char *mantissa = rendered;
    bool negative = false;
    if (*mantissa == '-') {
        negative = true;
        mantissa += 1;
    }

    /* The significant digits, with the point removed. */
    char digits[SK_NUMBER_MAX];
    size_t digit_count = 0;
    for (const char *at = mantissa; at < marker; at += 1) {
        if (*at != '.' && digit_count + 1 < sizeof digits) {
            digits[digit_count] = *at;
            digit_count += 1;
        }
    }
    digits[digit_count] = '\0';

    long exponent = strtol(marker + 1, NULL, 10);
    /* The first digit carries weight 10^exponent, so the point falls after `exponent + 1` digits. */
    long point = exponent + 1;

    size_t at = 0;
    if (negative && at + 1 < out_size) {
        out[at] = '-';
        at += 1;
    }
    if (point <= 0) {
        if (at + 2 < out_size) {
            out[at] = '0';
            out[at + 1] = '.';
            at += 2;
        }
        for (long zero = 0; zero < -point && at + 1 < out_size; zero += 1) {
            out[at] = '0';
            at += 1;
        }
        for (size_t digit = 0; digit < digit_count && at + 1 < out_size; digit += 1) {
            out[at] = digits[digit];
            at += 1;
        }
    } else if ((size_t)point >= digit_count) {
        for (size_t digit = 0; digit < digit_count && at + 1 < out_size; digit += 1) {
            out[at] = digits[digit];
            at += 1;
        }
        for (size_t zero = digit_count; zero < (size_t)point && at + 1 < out_size; zero += 1) {
            out[at] = '0';
            at += 1;
        }
    } else {
        for (size_t digit = 0; digit < (size_t)point && at + 1 < out_size; digit += 1) {
            out[at] = digits[digit];
            at += 1;
        }
        if (at + 1 < out_size) {
            out[at] = '.';
            at += 1;
        }
        for (size_t digit = (size_t)point; digit < digit_count && at + 1 < out_size; digit += 1) {
            out[at] = digits[digit];
            at += 1;
        }
    }
    out[at] = '\0';
}

bool sk_format_number(double number, char *out, size_t out_size)
{
    if (!isfinite(number)) {
        return false;
    }
    if (number == 0.0) {
        /* Covers -0.0, which rule 3 writes as `0`. Comparing against 0.0 rather than testing the
         * sign bit is deliberate: -0.0 == 0.0 is true, and that is the whole rule. */
        (void)snprintf(out, out_size, "0");
        return true;
    }
    if (number == floor(number) && fabs(number) < SK_INT_SAFE) {
        /* `%.0f` on an integral double below 2^53 is exact — every such value is representable and
         * printf renders it without an exponent, which is what rule 3 asks for. */
        (void)snprintf(out, out_size, "%.0f", number);
        return true;
    }

    /*
     * The reference's search: the shortest rendering that reads back as the same double.
     *
     * `%.*e` with `precision - 1` fraction digits is `toExponential(precision - 1)`, and `strtod` is
     * `Number(...)`. Both are correctly rounded in the C library this edition is built against, so
     * the two searches accept at the same precision — with one property worth naming rather than
     * assuming: they break an exact tie differently. ECMA-262 rounds a halfway case to the larger
     * mantissa and `printf` to the even one. A tie can only change the *accepted* rendering where
     * both candidates read back as this same double, which is a precision at which a shorter one
     * would already have been accepted. What holds that claim rather than this paragraph is the
     * `roundtrip` step in the Makefile: every number the reference wrote into `corpus/` is parsed
     * and re-serialised here, and a disagreement about any of them fails the build.
     */
    char rendered[SK_NUMBER_MAX];
    for (int precision = 1; precision <= 17; precision += 1) {
        (void)snprintf(rendered, sizeof rendered, "%.*e", precision - 1, number);
        if (strtod(rendered, NULL) == number) {
            exponential_to_positional(rendered, out, out_size);
            return true;
        }
    }
    (void)snprintf(rendered, sizeof rendered, "%.16e", number);
    exponential_to_positional(rendered, out, out_size);
    return true;
}

/*
 * Rule 4: escape the quote, the backslash and the C0 controls, and nothing else.
 *
 * Bytes above 0x7F are copied through untouched, which is the rule read literally and is also the
 * only reading that produces the reference's bytes: `JSON.stringify` emits UTF-8 for every
 * non-control character, and an edition that escaped non-ASCII would produce a shorter-looking
 * document that differs from the corpus on every case carrying a degree sign.
 */
static void write_string(const char *text, size_t len, sk_buf *out)
{
    sk_buf_append_char(out, '"');
    for (size_t at = 0; at < len; at += 1) {
        unsigned char byte = (unsigned char)text[at];
        switch (byte) {
        case '"': sk_buf_append_str(out, "\\\""); break;
        case '\\': sk_buf_append_str(out, "\\\\"); break;
        case '\b': sk_buf_append_str(out, "\\b"); break;
        case '\f': sk_buf_append_str(out, "\\f"); break;
        case '\n': sk_buf_append_str(out, "\\n"); break;
        case '\r': sk_buf_append_str(out, "\\r"); break;
        case '\t': sk_buf_append_str(out, "\\t"); break;
        default:
            if (byte < 0x20) {
                char escape[7];
                (void)snprintf(escape, sizeof escape, "\\u%04x", byte);
                sk_buf_append_str(out, escape);
            } else {
                sk_buf_append_char(out, (char)byte);
            }
            break;
        }
    }
    sk_buf_append_char(out, '"');
}

/* Rule 2: keys ascending by their UTF-8 byte sequence. A shorter key that is a prefix of a longer
 * one sorts first, which `memcmp` over the shared length does not decide, so the length breaks the
 * tie — exactly as the reference's `compareUtf8` returns the length difference. */
static int compare_keys(const void *left, const void *right)
{
    const sk_member *a = *(const sk_member *const *)left;
    const sk_member *b = *(const sk_member *const *)right;
    size_t shared = a->key_len < b->key_len ? a->key_len : b->key_len;
    int order = memcmp(a->key, b->key, shared);
    if (order != 0) {
        return order;
    }
    if (a->key_len == b->key_len) {
        return 0;
    }
    return a->key_len < b->key_len ? -1 : 1;
}

static bool write_value(sk_arena *arena, const sk_value *value, sk_buf *out, const char **error)
{
    if (value == NULL || value->kind == SK_NULL) {
        sk_buf_append_str(out, "null");
        return true;
    }
    switch (value->kind) {
    case SK_BOOL:
        sk_buf_append_str(out, value->truth ? "true" : "false");
        return true;
    case SK_NUMBER: {
        char rendered[SK_NUMBER_MAX];
        if (!sk_format_number(value->number, rendered, sizeof rendered)) {
            if (error != NULL) {
                *error = "cannot serialise a non-finite number";
            }
            return false;
        }
        sk_buf_append_str(out, rendered);
        return true;
    }
    case SK_STRING:
        write_string(value->text, value->text_len, out);
        return true;
    case SK_ARRAY:
        sk_buf_append_char(out, '[');
        for (size_t at = 0; at < value->count; at += 1) {
            if (at > 0) {
                sk_buf_append_char(out, ',');
            }
            if (!write_value(arena, value->items[at], out, error)) {
                return false;
            }
        }
        sk_buf_append_char(out, ']');
        return true;
    case SK_OBJECT: {
        const sk_member **sorted = sk_arena_alloc_array(arena, value->count + 1, sizeof *sorted);
        size_t count = 0;
        for (const sk_member *member = value->members; member != NULL; member = member->next) {
            sorted[count] = member;
            count += 1;
        }
        if (count > 1) {
            qsort(sorted, count, sizeof *sorted, compare_keys);
        }
        sk_buf_append_char(out, '{');
        for (size_t at = 0; at < count; at += 1) {
            if (at > 0) {
                sk_buf_append_char(out, ',');
            }
            write_string(sorted[at]->key, sorted[at]->key_len, out);
            sk_buf_append_char(out, ':');
            if (!write_value(arena, sorted[at]->value, out, error)) {
                return false;
            }
        }
        sk_buf_append_char(out, '}');
        return true;
    }
    case SK_NULL:
        break;
    }
    sk_buf_append_str(out, "null");
    return true;
}

bool sk_write(sk_arena *arena, const sk_value *value, sk_buf *out, const char **error)
{
    return write_value(arena, value, out, error);
}

const char *sk_encode(sk_arena *arena, const sk_value *value, size_t *len, const char **error)
{
    sk_buf out;
    sk_buf_init(&out, arena);
    if (!sk_write(arena, value, &out, error)) {
        return NULL;
    }
    if (len != NULL) {
        *len = out.len;
    }
    return out.bytes == NULL ? "" : out.bytes;
}

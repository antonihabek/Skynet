#include "skynet_os/parse.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "skynet_os/write.h"

typedef struct {
    sk_arena *arena;
    const char *bytes;
    size_t len;
    size_t at;
    sk_parse_error *error;
    bool failed;
    /* Bounds the nesting a document may reach. §7.5: bound recursion on untrusted input. 512 is
     * deeper than any corpus document and shallow enough that the recursion cannot reach a stack
     * this edition is built with. */
    int depth;
} parser;

#define SK_MAX_DEPTH 512

static bool fail(parser *p, const char *message)
{
    if (!p->failed) {
        p->failed = true;
        if (p->error != NULL) {
            p->error->message = message;
            p->error->offset = p->at;
            /* Counted from the start rather than tracked as we go: an error is produced once and a
             * line counter maintained on every byte is a cost paid on every byte. */
            int line = 1;
            int column = 1;
            for (size_t scan = 0; scan < p->at && scan < p->len; scan += 1) {
                if (p->bytes[scan] == '\n') {
                    line += 1;
                    column = 1;
                } else {
                    column += 1;
                }
            }
            p->error->line = line;
            p->error->column = column;
        }
    }
    return false;
}

static bool at_end(const parser *p)
{
    return p->at >= p->len;
}

static char peek(const parser *p)
{
    return at_end(p) ? '\0' : p->bytes[p->at];
}

static void skip_space(parser *p)
{
    while (!at_end(p)) {
        char byte = p->bytes[p->at];
        if (byte == ' ' || byte == '\t' || byte == '\n' || byte == '\r') {
            p->at += 1;
        } else {
            break;
        }
    }
}

static bool literal(parser *p, const char *word)
{
    size_t word_len = strlen(word);
    if (p->len - p->at < word_len || memcmp(p->bytes + p->at, word, word_len) != 0) {
        return fail(p, "not a JSON value");
    }
    p->at += word_len;
    return true;
}

/* Append one code point as UTF-8. */
static void append_utf8(sk_buf *out, unsigned long code)
{
    if (code < 0x80) {
        sk_buf_append_char(out, (char)code);
    } else if (code < 0x800) {
        sk_buf_append_char(out, (char)(0xC0 | (code >> 6)));
        sk_buf_append_char(out, (char)(0x80 | (code & 0x3F)));
    } else if (code < 0x10000) {
        sk_buf_append_char(out, (char)(0xE0 | (code >> 12)));
        sk_buf_append_char(out, (char)(0x80 | ((code >> 6) & 0x3F)));
        sk_buf_append_char(out, (char)(0x80 | (code & 0x3F)));
    } else {
        sk_buf_append_char(out, (char)(0xF0 | (code >> 18)));
        sk_buf_append_char(out, (char)(0x80 | ((code >> 12) & 0x3F)));
        sk_buf_append_char(out, (char)(0x80 | ((code >> 6) & 0x3F)));
        sk_buf_append_char(out, (char)(0x80 | (code & 0x3F)));
    }
}

static bool hex4(parser *p, unsigned long *out)
{
    if (p->len - p->at < 4) {
        return fail(p, "a truncated \\u escape");
    }
    unsigned long value = 0;
    for (int digit = 0; digit < 4; digit += 1) {
        char byte = p->bytes[p->at + (size_t)digit];
        value *= 16;
        if (byte >= '0' && byte <= '9') {
            value += (unsigned long)(byte - '0');
        } else if (byte >= 'a' && byte <= 'f') {
            value += (unsigned long)(byte - 'a') + 10;
        } else if (byte >= 'A' && byte <= 'F') {
            value += (unsigned long)(byte - 'A') + 10;
        } else {
            return fail(p, "a \\u escape with a non-hex digit");
        }
    }
    p->at += 4;
    *out = value;
    return true;
}

/* The bytes of a string, unescaped. `p->at` is on the opening quote. */
static bool parse_string_bytes(parser *p, const char **bytes, size_t *len)
{
    if (peek(p) != '"') {
        return fail(p, "expected a string");
    }
    p->at += 1;

    sk_buf out;
    sk_buf_init(&out, p->arena);
    while (true) {
        if (at_end(p)) {
            return fail(p, "an unterminated string");
        }
        unsigned char byte = (unsigned char)p->bytes[p->at];
        if (byte == '"') {
            p->at += 1;
            break;
        }
        if (byte < 0x20) {
            return fail(p, "an unescaped control character in a string");
        }
        if (byte != '\\') {
            sk_buf_append_char(&out, (char)byte);
            p->at += 1;
            continue;
        }
        p->at += 1;
        if (at_end(p)) {
            return fail(p, "a trailing backslash in a string");
        }
        char escape = p->bytes[p->at];
        p->at += 1;
        switch (escape) {
        case '"': sk_buf_append_char(&out, '"'); break;
        case '\\': sk_buf_append_char(&out, '\\'); break;
        case '/': sk_buf_append_char(&out, '/'); break;
        case 'b': sk_buf_append_char(&out, '\b'); break;
        case 'f': sk_buf_append_char(&out, '\f'); break;
        case 'n': sk_buf_append_char(&out, '\n'); break;
        case 'r': sk_buf_append_char(&out, '\r'); break;
        case 't': sk_buf_append_char(&out, '\t'); break;
        case 'u': {
            unsigned long code = 0;
            if (!hex4(p, &code)) {
                return false;
            }
            /*
             * A surrogate pair is two escapes for one code point, and JSON.stringify writes one for
             * every character above the BMP. A lone surrogate is kept as the code point it is
             * rather than refused: JavaScript strings admit one, so the reference can hold one, and
             * an edition that refused it would fail a document the oracle produced.
             */
            if (code >= 0xD800 && code <= 0xDBFF && p->len - p->at >= 6 && p->bytes[p->at] == '\\' &&
                p->bytes[p->at + 1] == 'u') {
                size_t before = p->at;
                p->at += 2;
                unsigned long low = 0;
                if (!hex4(p, &low)) {
                    return false;
                }
                if (low >= 0xDC00 && low <= 0xDFFF) {
                    code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                } else {
                    p->at = before;
                }
            }
            append_utf8(&out, code);
            break;
        }
        default:
            return fail(p, "an unknown escape in a string");
        }
    }

    *bytes = out.bytes == NULL ? "" : out.bytes;
    *len = out.len;
    return true;
}

static bool parse_number(parser *p, double *out)
{
    size_t start = p->at;
    if (peek(p) == '-') {
        p->at += 1;
    }
    /* JSON forbids a leading zero before another digit, and accepting one would be accepting a
     * document this edition's writer cannot produce. */
    if (peek(p) == '0') {
        p->at += 1;
        if (peek(p) >= '0' && peek(p) <= '9') {
            return fail(p, "a number with a leading zero");
        }
    } else if (peek(p) >= '1' && peek(p) <= '9') {
        while (peek(p) >= '0' && peek(p) <= '9') {
            p->at += 1;
        }
    } else {
        return fail(p, "a number with no digits");
    }
    if (peek(p) == '.') {
        p->at += 1;
        if (!(peek(p) >= '0' && peek(p) <= '9')) {
            return fail(p, "a number with no digits after the point");
        }
        while (peek(p) >= '0' && peek(p) <= '9') {
            p->at += 1;
        }
    }
    if (peek(p) == 'e' || peek(p) == 'E') {
        p->at += 1;
        if (peek(p) == '+' || peek(p) == '-') {
            p->at += 1;
        }
        if (!(peek(p) >= '0' && peek(p) <= '9')) {
            return fail(p, "a number with no digits in its exponent");
        }
        while (peek(p) >= '0' && peek(p) <= '9') {
            p->at += 1;
        }
    }

    /* strtod over a NUL-terminated copy: the run of bytes has been validated as a JSON number
     * above, so this converts rather than parses, and it is the same correctly rounded conversion
     * the reference's `Number(...)` performs. */
    char *text = sk_arena_copy(p->arena, p->bytes + start, p->at - start);
    *out = strtod(text, NULL);
    return true;
}

static const sk_value *parse_value(parser *p);

static const sk_value *parse_array(parser *p)
{
    p->at += 1; /* [ */
    sk_value *array = sk_array(p->arena);
    skip_space(p);
    if (peek(p) == ']') {
        p->at += 1;
        return array;
    }
    while (true) {
        const sk_value *item = parse_value(p);
        if (item == NULL) {
            return NULL;
        }
        sk_push(p->arena, array, item);
        skip_space(p);
        if (peek(p) == ',') {
            p->at += 1;
            skip_space(p);
            /* A trailing comma is the one JSON5 habit a hand-edited corpus file would pick up, and
             * it is named separately so the message says what is wrong rather than "not a value". */
            if (peek(p) == ']') {
                (void)fail(p, "a trailing comma in an array");
                return NULL;
            }
            continue;
        }
        if (peek(p) == ']') {
            p->at += 1;
            return array;
        }
        (void)fail(p, "expected a comma or a closing bracket");
        return NULL;
    }
}

static const sk_value *parse_object(parser *p)
{
    p->at += 1; /* { */
    sk_value *object = sk_object(p->arena);
    skip_space(p);
    if (peek(p) == '}') {
        p->at += 1;
        return object;
    }
    while (true) {
        skip_space(p);
        const char *key = NULL;
        size_t key_len = 0;
        if (!parse_string_bytes(p, &key, &key_len)) {
            return NULL;
        }
        skip_space(p);
        if (peek(p) != ':') {
            (void)fail(p, "expected a colon after a key");
            return NULL;
        }
        p->at += 1;
        const sk_value *value = parse_value(p);
        if (value == NULL) {
            return NULL;
        }
        /* Set through the length-aware path rather than `sk_set`, because a key may contain a NUL
         * and `sk_set` takes a C string. A duplicate key replaces the earlier one in place, which is
         * what the reference's object literal does. */
        sk_value *holder = object;
        char *copy = sk_arena_copy(p->arena, key, key_len);
        if (key_len == strlen(copy)) {
            sk_set(p->arena, holder, copy, value);
        } else {
            /* A key with an embedded NUL cannot be spelled as a C string, and no document the
             * oracle produces holds one. Refused rather than silently truncated. */
            (void)fail(p, "a key containing U+0000");
            return NULL;
        }
        skip_space(p);
        if (peek(p) == ',') {
            p->at += 1;
            skip_space(p);
            if (peek(p) == '}') {
                (void)fail(p, "a trailing comma in an object");
                return NULL;
            }
            continue;
        }
        if (peek(p) == '}') {
            p->at += 1;
            return object;
        }
        (void)fail(p, "expected a comma or a closing brace");
        return NULL;
    }
}

static const sk_value *parse_value(parser *p)
{
    if (p->depth >= SK_MAX_DEPTH) {
        (void)fail(p, "a document nested deeper than this parser will go");
        return NULL;
    }
    skip_space(p);
    if (at_end(p)) {
        (void)fail(p, "the document ended where a value was expected");
        return NULL;
    }
    char byte = peek(p);
    const sk_value *out = NULL;
    p->depth += 1;
    switch (byte) {
    case '{':
        out = parse_object(p);
        break;
    case '[':
        out = parse_array(p);
        break;
    case '"': {
        const char *text = NULL;
        size_t text_len = 0;
        if (parse_string_bytes(p, &text, &text_len)) {
            out = sk_string_len(p->arena, text, text_len);
        }
        break;
    }
    case 't':
        out = literal(p, "true") ? sk_bool(true) : NULL;
        break;
    case 'f':
        out = literal(p, "false") ? sk_bool(false) : NULL;
        break;
    case 'n':
        out = literal(p, "null") ? sk_null() : NULL;
        break;
    default: {
        double number = 0.0;
        if (parse_number(p, &number)) {
            out = sk_number(p->arena, number);
        }
        break;
    }
    }
    p->depth -= 1;
    return out;
}

const sk_value *sk_parse(sk_arena *arena, const char *bytes, size_t len, sk_parse_error *error)
{
    parser p = {
        .arena = arena,
        .bytes = bytes,
        .len = len,
        .at = 0,
        .error = error,
        .failed = false,
        .depth = 0,
    };
    if (error != NULL) {
        error->message = NULL;
        error->offset = 0;
        error->line = 0;
        error->column = 0;
    }
    /* A UTF-8 byte-order mark, which an editor on Windows will add to a file it saves. Skipped
     * rather than refused, and skipped only at the start. */
    if (len >= 3 && (unsigned char)bytes[0] == 0xEF && (unsigned char)bytes[1] == 0xBB &&
        (unsigned char)bytes[2] == 0xBF) {
        p.at = 3;
    }
    const sk_value *value = parse_value(&p);
    if (value == NULL) {
        return NULL;
    }
    skip_space(&p);
    if (!at_end(&p)) {
        (void)fail(&p, "trailing bytes after the document");
        return NULL;
    }
    return value;
}

const char *sk_parse_error_text(sk_arena *arena, const sk_parse_error *error)
{
    char rendered[256];
    (void)snprintf(rendered, sizeof rendered, "%d:%d: %s", error->line, error->column,
                   error->message == NULL ? "no error" : error->message);
    return sk_arena_copy(arena, rendered, strlen(rendered));
}

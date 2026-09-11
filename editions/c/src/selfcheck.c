/*
 * The C edition checking itself, over the things the corpus cannot reach.
 *
 *   skynet-selfcheck-c [--corpus-root corpus]
 *
 * Four groups, and each exists because a corpus run would be green without it:
 *
 * 1. **Rule 3 at its edges.** The corpus holds the numbers the reference happened to produce. It does
 *    not hold 2^53, or the value whose shortest rendering is exponential in every library and
 *    positional here, or negative zero. Those are where the format is decided.
 * 2. **Rules 2, 4 and 5 as facts rather than as consequences.** A key that is a prefix of another, an
 *    absent field beside a null one, a string holding U+0000.
 * 3. **What the parser refuses.** A trailing comma, `NaN`, a leading zero, an unescaped control
 *    character. A parser that accepted these would read documents the oracle cannot write, and no
 *    corpus file would say so.
 * 4. **The runner's own accounting**, over synthetic corpora with known answers. The four buckets are
 *    what every other number in this edition is reported through, and a runner that counted a
 *    declined file as a pass would report a finished edition.
 */
#include <math.h>
#include <stdio.h>
#include <uchar.h>
#include <string.h>

#include "skynet_os/arena.h"
#include "skynet_os/corpus.h"
#include "skynet_os/js.h"
#include "skynet_os/parse.h"
#include "skynet_os/value.h"
#include "skynet_os/write.h"

static int failures = 0;
static int checks = 0;

static void expect_text(const char *what, const char *actual, const char *wanted)
{
    checks += 1;
    if (actual == NULL || strcmp(actual, wanted) != 0) {
        failures += 1;
        (void)printf("  %s: expected \"%s\", got \"%s\"\n", what, wanted,
                     actual == NULL ? "(null)" : actual);
    }
}

static void expect_true(const char *what, bool truth)
{
    checks += 1;
    if (!truth) {
        failures += 1;
        (void)printf("  %s: expected true\n", what);
    }
}

static void expect_int(const char *what, long actual, long wanted)
{
    checks += 1;
    if (actual != wanted) {
        failures += 1;
        (void)printf("  %s: expected %ld, got %ld\n", what, wanted, actual);
    }
}

/* ── 1. Rule 3 ─────────────────────────────────────────────────────────────── */

static void check_number(const char *what, double number, const char *wanted)
{
    char rendered[SK_NUMBER_MAX];
    if (!sk_format_number(number, rendered, sizeof rendered)) {
        checks += 1;
        failures += 1;
        (void)printf("  %s: refused a finite number\n", what);
        return;
    }
    expect_text(what, rendered, wanted);
}

static void numbers(void)
{
    (void)printf("rule 3 — numbers\n");
    check_number("zero", 0.0, "0");
    /* -0 as 0 is the rule, and it is the one a naive `%g` gets wrong: it prints `-0`. */
    check_number("negative zero", -0.0, "0");
    check_number("one", 1.0, "1");
    check_number("minus one", -1.0, "-1");
    /* Integral and below 2^53 is written as an integer — including the largest such value, which a
     * formatter that switched to exponential at some width would render as 9.007...e15. */
    check_number("2^53 - 2", 9007199254740990.0, "9007199254740990");
    /* At and above 2^53 the integer branch stops applying and the search takes over. The shortest
     * decimal that reads back as 2^53 is its own digits, positionally. */
    check_number("2^53", 9007199254740992.0, "9007199254740992");
    check_number("a tenth", 0.1, "0.1");
    check_number("two tenths", 0.2, "0.2");
    /* 0.1 + 0.2 is the canonical demonstration that a shortest-round-trip search is not the same as
     * printing seventeen digits: the value is not 0.3 and its shortest rendering says so. */
    check_number("a tenth and two", 0.1 + 0.2, "0.30000000000000004");
    check_number("minus four tenths and a bit", -0.44, "-0.44");
    /* The reference's own example of why this function exists rather than `JSON.stringify`: the
     * engine writes `1e-7` and the format requires a positional rendering. */
    check_number("a ten-millionth", 1e-7, "0.0000001");
    /* The smallest positive double. Every library renders this with an exponent; rule 3
     * requires a positional rendering, and this is the case that says so. The digits are the
     * oracle's, read out of `formatNumber` rather than counted by hand. */
    check_number("a very small number", 5e-324,
                 "0.00000000000000000000000000000000000000000000000000"
                 "00000000000000000000000000000000000000000000000000"
                 "00000000000000000000000000000000000000000000000000"
                 "00000000000000000000000000000000000000000000000000"
                 "00000000000000000000000000000000000000000000000000"
                 "00000000000000000000000000000000000000000000000000"
                 "000000000000000000000005");
    /* Positional above 2^53 too, where every library reaches for an exponent. */
    check_number("ten to the twenty-first", 1e21, "1000000000000000000000");
    check_number("a third", 1.0 / 3.0, "0.3333333333333333");
    check_number("two thirds", 2.0 / 3.0, "0.6666666666666666");
    check_number("a large mixed value", 1234567890.1234567, "1234567890.1234567");

    checks += 1;
    char rendered[SK_NUMBER_MAX];
    if (sk_format_number((double)NAN, rendered, sizeof rendered)) {
        failures += 1;
        (void)printf("  NaN: was formatted rather than refused. A document that wrote a non-finite "
                     "value as `null` would claim a measurement was absent when it was not.\n");
    }
    checks += 1;
    if (sk_format_number((double)INFINITY, rendered, sizeof rendered)) {
        failures += 1;
        (void)printf("  infinity: was formatted rather than refused\n");
    }
}

/* ── 2. Rules 2, 4 and 5 ───────────────────────────────────────────────────── */

static const char *encode(sk_arena *arena, const sk_value *value)
{
    const char *error = NULL;
    const char *encoded = sk_encode(arena, value, NULL, &error);
    return encoded == NULL ? error : encoded;
}

static void writer(sk_arena *arena)
{
    (void)printf("rules 2, 4 and 5 — keys, strings and absence\n");

    /* Rule 2 over a key that is a prefix of another: `a` sorts before `ab`, which a comparison over
     * the shared bytes alone cannot decide. */
    sk_value *prefixes = sk_object(arena);
    sk_set(arena, prefixes, "ab", sk_number(arena, 2));
    sk_set(arena, prefixes, "a", sk_number(arena, 1));
    sk_set(arena, prefixes, "abc", sk_number(arena, 3));
    expect_text("a key that is a prefix of another", encode(arena, prefixes),
                "{\"a\":1,\"ab\":2,\"abc\":3}");

    /* Ascending by UTF-8 bytes, which is not ascending by code unit for anything outside the BMP and
     * is not case-insensitive: `Z` (0x5A) sorts before `a` (0x61). */
    sk_value *bytes = sk_object(arena);
    sk_set(arena, bytes, "a", sk_number(arena, 1));
    sk_set(arena, bytes, "Z", sk_number(arena, 2));
    expect_text("uppercase before lowercase", encode(arena, bytes), "{\"Z\":2,\"a\":1}");

    /* Rule 5: an absent field is omitted and a null one is written, and they are different facts. */
    sk_value *absence = sk_object(arena);
    sk_set(arena, absence, "declared", sk_null());
    sk_set_optional(arena, absence, "undeclared", NULL);
    expect_text("absent beside null", encode(arena, absence), "{\"declared\":null}");

    /* Rule 4: the quote, the backslash and the C0 controls, and nothing else. A tab is `\t` and not
     * `\u0009`; a degree sign is its UTF-8 bytes and not an escape. */
    sk_value *escapes = sk_object(arena);
    sk_set(arena, escapes, "quote", sk_string(arena, "a\"b"));
    sk_set(arena, escapes, "backslash", sk_string(arena, "a\\b"));
    sk_set(arena, escapes, "tab", sk_string(arena, "a\tb"));
    sk_set(arena, escapes, "unit separator", sk_string_len(arena, "a\037b", 3));
    sk_set(arena, escapes, "degree", sk_string(arena, "20\xc2\xb0"));
    expect_text("rule 4", encode(arena, escapes),
                "{\"backslash\":\"a\\\\b\",\"degree\":\"20\xc2\xb0\",\"quote\":\"a\\\"b\","
                "\"tab\":\"a\\tb\",\"unit separator\":\"a\\u001fb\"}");

    /* A string carrying U+0000 keeps its length, which a model built on C strings would lose at
     * exactly that byte while passing every other case here. */
    sk_value *embedded = sk_object(arena);
    sk_set(arena, embedded, "nul", sk_string_len(arena, "a\0b", 3));
    expect_text("a string holding U+0000", encode(arena, embedded), "{\"nul\":\"a\\u0000b\"}");

    /* A non-finite number is where writing fails, and it fails as itself. */
    sk_value *broken = sk_object(arena);
    sk_set(arena, broken, "measured", sk_number(arena, (double)NAN));
    const char *error = NULL;
    checks += 1;
    if (sk_encode(arena, broken, NULL, &error) != NULL) {
        failures += 1;
        (void)printf("  a document holding NaN was serialised\n");
    }

    /* An empty object and an empty array are documents in their own right. */
    expect_text("an empty object", encode(arena, sk_object(arena)), "{}");
    expect_text("an empty array", encode(arena, sk_array(arena)), "[]");

    /* Assigning a field twice keeps its first position, which is what the reference's object literal
     * does — and matters because the order a contract inserted in is what a failure prints. */
    sk_value *reassigned = sk_object(arena);
    sk_set(arena, reassigned, "b", sk_number(arena, 1));
    sk_set(arena, reassigned, "a", sk_number(arena, 2));
    sk_set(arena, reassigned, "b", sk_number(arena, 3));
    expect_text("a field assigned twice", encode(arena, reassigned), "{\"a\":2,\"b\":3}");
}

/* ── 3. What the parser refuses ────────────────────────────────────────────── */

static void refuses(sk_arena *arena, const char *what, const char *document)
{
    sk_parse_error error;
    checks += 1;
    if (sk_parse(arena, document, strlen(document), &error) != NULL) {
        failures += 1;
        (void)printf("  %s: `%s` was accepted. A document the oracle cannot write is one this "
                     "edition must not read.\n",
                     what, document);
    }
}

static void accepts(sk_arena *arena, const char *what, const char *document, const char *canonical)
{
    sk_parse_error error;
    const sk_value *parsed = sk_parse(arena, document, strlen(document), &error);
    if (parsed == NULL) {
        checks += 1;
        failures += 1;
        (void)printf("  %s: `%s` was refused: %s\n", what, document,
                     sk_parse_error_text(arena, &error));
        return;
    }
    expect_text(what, encode(arena, parsed), canonical);
}

static void parser(sk_arena *arena)
{
    (void)printf("the parser\n");
    refuses(arena, "a trailing comma in an array", "[1,2,]");
    refuses(arena, "a trailing comma in an object", "{\"a\":1,}");
    refuses(arena, "NaN", "{\"a\":NaN}");
    refuses(arena, "infinity", "{\"a\":Infinity}");
    refuses(arena, "a leading zero", "{\"a\":01}");
    refuses(arena, "a leading plus", "{\"a\":+1}");
    refuses(arena, "a bare word", "{\"a\":undefined}");
    refuses(arena, "a single-quoted string", "{'a':1}");
    refuses(arena, "an unterminated object", "{\"a\":1");
    refuses(arena, "two documents", "{} {}");
    refuses(arena, "an unescaped newline in a string", "{\"a\":\"one\ntwo\"}");
    refuses(arena, "a comment", "{\"a\":1} // a comment");

    /* The corpus is written by `JSON.stringify`, so what the parser must accept is that shape:
     * whitespace, unsorted keys, and an escape for every character above the BMP. */
    accepts(arena, "whitespace and unsorted keys", "{ \"b\" : 1 ,\n  \"a\" : 2 }",
            "{\"a\":2,\"b\":1}");
    accepts(arena, "a surrogate pair", "[\"\\ud834\\udd1e\"]", "[\"\xf0\x9d\x84\x9e\"]");
    accepts(arena, "an escaped solidus", "[\"a\\/b\"]", "[\"a/b\"]");
    accepts(arena, "an escaped BMP character", "[\"\\u00b0\"]", "[\"\xc2\xb0\"]");
    accepts(arena, "an exponent", "[1e3]", "[1000]");
    accepts(arena, "a negative exponent", "[1e-7]", "[0.0000001]");
    accepts(arena, "nesting", "[[[[1]]]]", "[[[[1]]]]");
}

/* ── 4. The runner's accounting ────────────────────────────────────────────── */

static const sk_value *answers_one(sk_arena *arena, const sk_value *input, const char **error)
{
    (void)error;
    return sk_number(arena, sk_as_number(input, 0) + 1);
}

static const sk_value *always_wrong(sk_arena *arena, const sk_value *input, const char **error)
{
    (void)input;
    (void)error;
    return sk_number(arena, -1);
}

static const sk_value *declines(sk_arena *arena, const sk_value *input, const char **error)
{
    (void)arena;
    (void)input;
    *error = "this case is not answered";
    return NULL;
}

/* One file of `count` cases whose input is `n` and whose expectation is `n + 1`. */
static sk_file synthetic(sk_arena *arena, const char *corpus_name, const char *name, size_t count)
{
    sk_file file;
    file.path = name;
    file.corpus = corpus_name;
    file.name = name;
    char key[128];
    (void)snprintf(key, sizeof key, "%s/%s", corpus_name, name);
    file.key = sk_arena_copy(arena, key, strlen(key));
    file.count = count;
    file.cases = sk_arena_alloc_array(arena, count + 1, sizeof *file.cases);
    for (size_t at = 0; at < count; at += 1) {
        char expected[64];
        (void)snprintf(expected, sizeof expected, "%zu", at + 1);
        file.cases[at].id = "case";
        file.cases[at].note = "a synthetic case";
        file.cases[at].input = sk_number(arena, (double)at);
        file.cases[at].expected = sk_arena_copy(arena, expected, strlen(expected));
        file.cases[at].expected_len = strlen(expected);
    }
    return file;
}

static void accounting(sk_arena *arena)
{
    (void)printf("the runner's accounting\n");

    sk_file files[4];
    files[0] = synthetic(arena, "answered", "whole", 3);
    files[1] = synthetic(arena, "partly", "handled", 2);
    files[2] = synthetic(arena, "partly", "declined", 4);
    files[3] = synthetic(arena, "absent", "nothing", 5);

    const sk_dispatch_entry dispatch[] = {
        {"answered/whole", answers_one},
        {"partly/handled", answers_one},
    };
    sk_report report = sk_corpus_run(arena, files, 4, dispatch, 2);

    expect_int("passed", report.passed, 5);
    expect_int("failed", report.failed, 0);
    /* The four declined cases and the five with no handler at all are both *not implemented*, and a
     * runner that let either read as a pass would report a finished edition. */
    expect_int("not implemented", report.unanswered, 9);
    expect_int("total", sk_report_total(&report), 14);
    expect_int("complete corpora", (long)report.complete_count, 1);
    expect_true("`answered` is complete", report.complete_count == 1 &&
                                              strcmp(report.complete[0], "answered") == 0);
    expect_int("partial corpora", (long)report.partial_count, 1);
    expect_true("`partly` is partial",
                report.partial_count == 1 && strcmp(report.partial[0], "partly") == 0);
    expect_int("corpora with no handler", (long)report.missing_count, 1);
    expect_true("`absent` has no handler",
                report.missing_count == 1 && strcmp(report.missing_handlers[0], "absent") == 0);
    /* A corpus with no handler at all is named once rather than once per file, so the declined list
     * holds only the file of the corpus that is partly answered. */
    expect_int("declined files", (long)report.declined_count, 1);
    expect_true("the declined file is named",
                report.declined_count == 1 && strcmp(report.declined_files[0], "partly/declined") == 0);

    /* A handler that answers wrongly is a failure, and its corpus is partial rather than complete
     * even though every file has a handler. */
    const sk_dispatch_entry wrong[] = {{"answered/whole", always_wrong}};
    sk_report red = sk_corpus_run(arena, files, 1, wrong, 1);
    expect_int("a wrong answer fails", red.failed, 3);
    expect_int("and passes nothing", red.passed, 0);
    expect_int("and its corpus is not complete", (long)red.complete_count, 0);
    expect_int("its corpus is partial", (long)red.partial_count, 1);

    /* A handler that declines a case it was given is a *failure*, not an unanswered case: the file
     * has a handler, so silence there would be a contract quietly opting out of its own cases. */
    const sk_dispatch_entry declining[] = {{"answered/whole", declines}};
    sk_report refused = sk_corpus_run(arena, files, 1, declining, 1);
    expect_int("a declined case fails", refused.failed, 3);
    expect_int("and is not counted as unimplemented", refused.unanswered, 0);
}

/* ── 5. The case tables, against whole strings the engine converted ────────── */

typedef struct {
    bool upper;
    const char32_t *in;
    const char32_t *out;
} sk_case_vector;

#include "../tests/case_vectors.inc"

/* A char32_t string as UTF-8, which is what `sk_js_upper` and `sk_js_lower` take. */
static const char *utf8_of(sk_arena *arena, const char32_t *points, size_t *out_len)
{
    sk_buf out;
    sk_buf_init(&out, arena);
    for (size_t at = 0; points[at] != 0; at += 1) {
        char encoded[4];
        size_t width = sk_utf8_encode((uint32_t)points[at], encoded);
        sk_buf_append(&out, encoded, width);
    }
    *out_len = out.len;
    return out.bytes == NULL ? "" : out.bytes;
}

static void case_tables(sk_arena *arena)
{
    (void)printf("the case tables\n");

    /* The tables live in `src/unicode_case.inc`, which only `src/js.c` includes, and the vectors here
     * carry their own version stamp. Two files generated under different Unicode versions would
     * disagree about characters neither of them is wrong about, and the disagreement would read as
     * hundreds of defects — so the two stamps are compared across the file boundary. */
    expect_text("the tables and the vectors are one Unicode version", sk_js_unicode_version(),
                SK_VECTORS_UNICODE_VERSION);

    /*
     * The mapping tables are binary-searched, so their order is load-bearing: a table that lost it
     * would still answer most lookups, which is the failure that passes a corpus run and breaks on
     * one character nobody tested.
     */
    expect_true("the upper table is ascending", sk_js_tables_sorted());

    /*
     * 219 whole strings the engine converted. The tables are single code points and cannot be wrong
     * about a string on their own; the code that walks a string with them, and applies the sigma
     * rule, can — and these are what would say so (finding 163).
     */
    size_t vector_count = sizeof case_vectors / sizeof case_vectors[0];
    int wrong = 0;
    for (size_t at = 0; at < vector_count; at += 1) {
        const sk_case_vector *vector = &case_vectors[at];
        size_t in_len = 0;
        size_t wanted_len = 0;
        const char *in = utf8_of(arena, vector->in, &in_len);
        const char *wanted = utf8_of(arena, vector->out, &wanted_len);
        size_t got_len = 0;
        const char *got = vector->upper ? sk_js_upper(arena, in, in_len, &got_len)
                                        : sk_js_lower(arena, in, in_len, &got_len);
        if (got_len != wanted_len || memcmp(got, wanted, got_len) != 0) {
            wrong += 1;
            if (wrong <= 5) {
                (void)printf("  vector %zu (%s): expected \"%s\", got \"%s\"\n", at,
                             vector->upper ? "upper" : "lower", wanted, got);
            }
        }
    }
    checks += 1;
    if (wrong != 0) {
        failures += 1;
        (void)printf("  %d of %zu case vectors disagree with the engine\n", wrong, vector_count);
    }

    /* The three mappings the header says a C library cannot do, named so a reader can see them. */
    size_t len = 0;
    expect_text("a sharp s upper-cases to two letters",
                sk_js_upper(arena, "\xc3\x9f", 2, &len), "SS");
    expect_text("the Kelvin sign lower-cases to an ASCII k",
                sk_js_lower(arena, "\xe2\x84\xaa", 3, &len), "k");
    /* A capital sigma at the end of a word is a *different* letter from one in the middle, which is
     * the whole reason the ranges are in the table beside the mappings. */
    expect_text("a final sigma", sk_js_lower(arena, "\xce\x9f\xce\xa3", 4, &len),
                "\xce\xbf\xcf\x82");
    expect_text("and a medial one", sk_js_lower(arena, "\xce\x9f\xce\xa3\xce\x9f", 6, &len),
                "\xce\xbf\xcf\x83\xce\xbf");

    /* `trim` removes ECMAScript's whitespace, which is not `isspace`'s: the semver corpus holds a
     * range written with a U+FEFF before the version, and the reference trims it. */
    expect_text("trim removes a zero-width no-break space",
                sk_js_trim(arena, "\xef\xbb\xbf 1.2.3 \xef\xbb\xbf", 13, &len), "1.2.3");
    expect_text("trim leaves an interior space",
                sk_js_trim(arena, "  a b  ", 7, &len), "a b");
    expect_text("trim of nothing but space", sk_js_trim(arena, "   ", 3, &len), "");

    /* Number::toString is not the canonical writer, and the two differ in both directions. */
    expect_text("a ten-millionth is exponential here", sk_js_number_to_string(arena, 1e-7), "1e-7");
    expect_text("and so is ten to the twenty-first",
                sk_js_number_to_string(arena, 1e21), "1e+21");
    expect_text("a millionth is not", sk_js_number_to_string(arena, 1e-6), "0.000001");
    expect_text("ten to the twentieth is not",
                sk_js_number_to_string(arena, 1e20), "100000000000000000000");
    expect_text("an integer", sk_js_number_to_string(arena, 100), "100");
    expect_text("a negative", sk_js_number_to_string(arena, -1.5), "-1.5");
    expect_text("negative zero is zero here too", sk_js_number_to_string(arena, -0.0), "0");
    /* Where the canonical writer refuses, this renders the word — and the difference matters because
     * these end up inside frozen message strings. */
    expect_text("NaN is a word", sk_js_number_to_string(arena, (double)NAN), "NaN");
    expect_text("and so is infinity", sk_js_number_to_string(arena, (double)INFINITY), "Infinity");

    /* ToString over a value, which is what a template literal does. */
    expect_text("an absent field", sk_js_to_string(arena, NULL, false), "undefined");
    expect_text("a null field", sk_js_to_string(arena, sk_null(), true), "null");
    sk_value *array = sk_array(arena);
    sk_push(arena, array, sk_number(arena, 1));
    sk_push(arena, array, sk_null());
    sk_push(arena, array, sk_string(arena, "a"));
    expect_text("an array joins with commas and writes null as nothing",
                sk_js_to_string(arena, array, true), "1,,a");
    expect_text("an object", sk_js_to_string(arena, sk_object(arena), true), "[object Object]");
}

int main(int argc, char **argv)
{
    for (int at = 1; at < argc; at += 1) {
        if (strcmp(argv[at], "--corpus-root") == 0 && at + 1 < argc) {
            /* Accepted and unused: `make test` passes the same arguments to both programs, and a
             * self-check that rejected them would make the Makefile's two lines differ for no
             * reason a reader could see. */
            at += 1;
        } else {
            (void)fprintf(stderr, "usage: skynet-selfcheck-c [--corpus-root DIR]\n");
            return 2;
        }
    }

    sk_arena *arena = sk_arena_new();
    numbers();
    writer(arena);
    parser(arena);
    accounting(arena);
    case_tables(arena);
    sk_arena_free(arena);

    (void)printf("self check  %d checks, %d failed\n", checks, failures);
    return failures == 0 ? 0 : 1;
}

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
#include <string.h>

#include "skynet_os/arena.h"
#include "skynet_os/corpus.h"
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
    sk_arena_free(arena);

    (void)printf("self check  %d checks, %d failed\n", checks, failures);
    return failures == 0 ? 0 : 1;
}

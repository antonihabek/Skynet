/*
 * Run the conformance corpus against the C edition.
 *
 *   skynet-corpus-c [--corpus-root corpus] [--verbose] [--json] [--min-passed N]
 *
 * Exit code 0 only when every case in every corpus **that has a handler** passed, and the summary
 * always says how many cases are not implemented — split into the corpora with no handler at all and
 * the individual files a handler has not been written for. Those facts together are the honest
 * sentence about this edition, and none of them is useful without the others: §31.4 says an edition
 * that has not passed the corpus is not an edition, so "8 of 33 corpora, all passing" has to be as
 * visible as the zero failures.
 *
 * `--min-passed` is the ratchet, and it exists because everything above is a **floor of zero**. An
 * edition whose handler was deleted answers nothing, fails nothing, and exits 0: the cases move from
 * *passed* into *not implemented*, which is the bucket that reads as honest progress. That is the one
 * regression this runner's own design cannot see, so the number a build passes here is the number
 * this edition has reached, and lowering it is a decision somebody makes in a diff.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "skynet_os/arena.h"
#include "skynet_os/contracts.h"
#include "skynet_os/corpus.h"
#include "skynet_os/write.h"

static void print_list(const char *heading, const char **values, size_t count)
{
    if (count == 0) {
        return;
    }
    (void)printf("%s ", heading);
    for (size_t at = 0; at < count; at += 1) {
        (void)printf("%s%s", at == 0 ? "" : ", ", values[at]);
    }
    (void)printf("\n");
}

static const sk_value *strings(sk_arena *arena, const char **values, size_t count)
{
    sk_value *array = sk_array(arena);
    for (size_t at = 0; at < count; at += 1) {
        sk_push(arena, array, sk_string(arena, values[at]));
    }
    return array;
}

int main(int argc, char **argv)
{
    const char *root_argument = NULL;
    bool verbose = false;
    bool as_json = false;
    long min_passed = -1;

    for (int at = 1; at < argc; at += 1) {
        if (strcmp(argv[at], "--corpus-root") == 0 && at + 1 < argc) {
            root_argument = argv[at + 1];
            at += 1;
        } else if (strcmp(argv[at], "--verbose") == 0) {
            verbose = true;
        } else if (strcmp(argv[at], "--json") == 0) {
            as_json = true;
        } else if (strcmp(argv[at], "--min-passed") == 0 && at + 1 < argc) {
            min_passed = strtol(argv[at + 1], NULL, 10);
            at += 1;
        } else {
            (void)fprintf(stderr,
                          "usage: skynet-corpus-c [--corpus-root DIR] [--verbose] [--json] "
                          "[--min-passed N]\n");
            return 2;
        }
    }

    sk_arena *arena = sk_arena_new();
    const char *root = sk_corpus_root(arena, root_argument);
    if (root == NULL) {
        (void)fprintf(stderr, "no corpus found: pass --corpus-root, or set SKYNET_CORPUS\n");
        sk_arena_free(arena);
        return 2;
    }

    const char *error = NULL;
    size_t file_count = 0;
    sk_file *files = sk_corpus_load(arena, root, &file_count, &error);
    if (files == NULL) {
        (void)fprintf(stderr, "%s\n", error);
        sk_arena_free(arena);
        return 2;
    }
    if (file_count == 0) {
        (void)fprintf(stderr, "no corpus files under %s\n", root);
        sk_arena_free(arena);
        return 2;
    }

    size_t dispatch_count = 0;
    const sk_dispatch_entry *dispatch = sk_dispatch(&dispatch_count);
    sk_report report = sk_corpus_run(arena, files, file_count, dispatch, dispatch_count);
    int total = sk_report_total(&report);

    if (as_json) {
        /* Deliberately the same fields the human report prints, and no derived ones: a summary that
         * computed a percentage here and a page that computed it differently would be two answers to
         * one question. */
        sk_value *out = sk_object(arena);
        sk_set(arena, out, "passed", sk_number(arena, (double)report.passed));
        sk_set(arena, out, "failed", sk_number(arena, (double)report.failed));
        sk_set(arena, out, "unimplemented", sk_number(arena, (double)report.unanswered));
        sk_set(arena, out, "total", sk_number(arena, (double)total));
        sk_set(arena, out, "complete", strings(arena, report.complete, report.complete_count));
        sk_set(arena, out, "partial", strings(arena, report.partial, report.partial_count));
        sk_set(arena, out, "missing_handlers",
               strings(arena, report.missing_handlers, report.missing_count));
        sk_set(arena, out, "declined_files",
               strings(arena, report.declined_files, report.declined_count));
        const char *write_error = NULL;
        const char *encoded = sk_encode(arena, out, NULL, &write_error);
        if (encoded == NULL) {
            (void)fprintf(stderr, "the report could not be serialised: %s\n", write_error);
            sk_arena_free(arena);
            return 2;
        }
        (void)printf("%s\n", encoded);
        int code = report.failed == 0 ? 0 : 1;
        sk_arena_free(arena);
        return code;
    }

    (void)printf("corpus     %s\n", root);
    (void)printf("result     %d passed, %d failed, %d not implemented, of %d\n", report.passed,
                 report.failed, report.unanswered, total);
    print_list("complete  ", report.complete, report.complete_count);
    print_list("no handler", report.missing_handlers, report.missing_count);
    print_list("declined  ", report.declined_files, report.declined_count);
    /* Named separately from `complete` on purpose: these are corpora this edition answers part of,
     * and a reader who saw only the passing files would take them for finished ones. */
    print_list("partial   ", report.partial, report.partial_count);

    if (report.failure_count > 0) {
        (void)printf("\n");
        size_t shown = report.failure_count;
        if (!verbose && shown > 10) {
            shown = 10;
        }
        for (size_t at = 0; at < shown; at += 1) {
            (void)printf("%s\n\n", sk_failure_describe(arena, &report.failures[at]));
        }
        if (shown < report.failure_count) {
            (void)printf("... and %zu more (pass --verbose)\n", report.failure_count - shown);
        }
    }

    if (min_passed >= 0 && report.passed < min_passed) {
        (void)printf("\nregression: %d passed, and this edition had reached %ld. Cases that stop "
                     "being answered are counted as *not implemented* rather than as failures, so "
                     "nothing above this line went red — which is why the floor is checked "
                     "separately from the failures.\n",
                     report.passed, min_passed);
        sk_arena_free(arena);
        return 1;
    }
    int code = report.failed == 0 ? 0 : 1;
    sk_arena_free(arena);
    return code;
}

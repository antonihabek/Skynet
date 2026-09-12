/*
 * Reading and running the conformance corpus.
 *
 * Split from the contracts because two programs read the corpus and only one of them computes
 * anything: `roundtrip` re-serialises every `expected` string in it before a single contract exists,
 * and `corpus` runs the contracts against it. One reader means the two cannot disagree about which
 * files the corpus has.
 *
 * # The four buckets, and why none of them is useful alone
 *
 * §31.4: an edition that has not passed the corpus is not an edition. A runner that printed only
 * failures would report an edition with one handler as flawless, so a report here says how many
 * cases *passed*, how many *failed*, and how many are *not implemented* — split into the corpora
 * with no handler at all and the individual files a handler has not been written for.
 *
 * The floor those three share is zero, which is the one regression this design cannot see: delete a
 * handler and its cases move from passed into not-implemented, nothing goes red, and the runner
 * exits 0. `--min-passed` is the ratchet that closes it, and the number a build passes is the number
 * this edition has reached.
 */
#ifndef SKYNET_OS_CORPUS_H
#define SKYNET_OS_CORPUS_H

#include <stdbool.h>
#include <stddef.h>

#include "skynet_os/arena.h"
#include "skynet_os/value.h"

/* One vector: an input, the bytes the reference produced from it, and the note that says what it is
 * for. `expected` is a string and not a tree, because the requirement is byte-identical output and
 * the bytes are what the file holds. */
typedef struct {
    const char *id;
    const char *note;
    const sk_value *input;
    const char *expected;
    size_t expected_len;
} sk_case;

/* One corpus file, and the `corpus/name` key a dispatch is registered under. */
typedef struct {
    const char *path;
    const char *corpus;
    const char *name;
    const char *key;
    sk_case *cases;
    size_t count;
} sk_file;

/*
 * A contract, as the runner sees it.
 *
 * Returns the value the contract produced; the runner encodes it and compares bytes. Returning NULL
 * with `*error` set is a case this edition could not answer *at all* — not one it answered wrongly.
 * The two are counted separately and neither may hide in the other.
 *
 * The arena belongs to the case and is reset after it, so a handler allocates freely and frees
 * nothing.
 */
typedef const sk_value *(*sk_handler)(sk_arena *arena, const sk_value *input, const char **error);

typedef struct {
    const char *key;
    sk_handler handler;
} sk_dispatch_entry;

/* One case that was answered and answered wrongly, or could not be answered. */
typedef struct {
    const char *file;
    const char *id;
    const char *note;
    const char *expected;
    const char *actual;
    const char *error;
} sk_failure;

typedef struct {
    int passed;
    int failed;
    int unanswered;
    const char **complete;
    size_t complete_count;
    const char **partial;
    size_t partial_count;
    const char **missing_handlers;
    size_t missing_count;
    const char **declined_files;
    size_t declined_count;
    sk_failure *failures;
    size_t failure_count;
} sk_report;

/*
 * The corpus directory: an explicit path, else `$SKYNET_CORPUS`, else the nearest `corpus/` at or
 * above the working directory. NULL when there is none, which the caller reports rather than
 * defaulting to something plausible — a runner that measured an empty corpus would exit 0 having
 * proved nothing.
 */
const char *sk_corpus_root(sk_arena *arena, const char *explicit_root);

/* Every JSON file one directory below `root`, sorted, so a run is reproducible. NULL on failure. */
sk_file *sk_corpus_load(sk_arena *arena, const char *root, size_t *count, const char **error);

/* Measure `dispatch` against `files`. */
sk_report sk_corpus_run(sk_arena *arena, const sk_file *files, size_t file_count,
                        const sk_dispatch_entry *dispatch, size_t dispatch_count);

int sk_report_total(const sk_report *report);
/* `file :: case`, the note, and where the two documents part company — a diff of two four-kilobyte
 * documents is not readable without that last line. */
const char *sk_failure_describe(sk_arena *arena, const sk_failure *failure);

#endif /* SKYNET_OS_CORPUS_H */

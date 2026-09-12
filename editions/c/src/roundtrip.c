/*
 * The canonical writer, proven before anything is built on it.
 *
 *   skynet-roundtrip-c [--corpus-root corpus]
 *
 * Every `expected` string in the corpus is parsed and re-serialised, and the bytes must be
 * identical. That is a stronger exam than it looks, and it is the reason this program runs before
 * the corpus runner rather than after:
 *
 * * **Rule 3 over every number the reference ever wrote.** The shortest-round-trip search and the
 *   exponential-to-positional conversion are the part of this edition with no natural test — a
 *   contract that computes the wrong pose fails a case, but a formatter that renders `1e-7` where
 *   the reference renders `0.0000001` fails every case that carries a small number, for a reason
 *   that looks like arithmetic. Here it fails as itself, naming the file and the case.
 * * **Rule 2 over keys nobody chose.** The corpus holds documents whose keys collide on their first
 *   bytes and differ in length, which is the comparison a `memcmp` over the shared prefix gets
 *   wrong.
 * * **Rule 4 over strings the generator escaped.** The corpus is written by `JSON.stringify`, which
 *   escapes non-ASCII as `\uXXXX`; this edition emits UTF-8. A file that round-trips has had both
 *   halves — the parser's escape decoding and the writer's pass-through — agree.
 *
 * A round trip that fails is a defect in this edition and never in the corpus, because the corpus is
 * the oracle's output and canonical by construction.
 */
#include <stdio.h>
#include <string.h>

#include "skynet_os/arena.h"
#include "skynet_os/corpus.h"
#include "skynet_os/parse.h"
#include "skynet_os/write.h"

int main(int argc, char **argv)
{
    const char *root_argument = NULL;
    for (int at = 1; at < argc; at += 1) {
        if (strcmp(argv[at], "--corpus-root") == 0 && at + 1 < argc) {
            root_argument = argv[at + 1];
            at += 1;
        } else {
            (void)fprintf(stderr, "usage: skynet-roundtrip-c [--corpus-root DIR]\n");
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

    int checked = 0;
    int failed = 0;
    for (size_t at = 0; at < file_count; at += 1) {
        const sk_file *file = &files[at];
        for (size_t index = 0; index < file->count; index += 1) {
            const sk_case *vector = &file->cases[index];
            sk_arena *scratch = sk_arena_new();
            sk_parse_error parse_error;
            const sk_value *parsed =
                sk_parse(scratch, vector->expected, vector->expected_len, &parse_error);
            if (parsed == NULL) {
                failed += 1;
                (void)printf("%s :: %s\n  the expectation is not JSON: %s\n", file->key, vector->id,
                             sk_parse_error_text(scratch, &parse_error));
                sk_arena_free(scratch);
                continue;
            }
            const char *write_error = NULL;
            size_t encoded_len = 0;
            const char *encoded = sk_encode(scratch, parsed, &encoded_len, &write_error);
            if (encoded == NULL) {
                failed += 1;
                (void)printf("%s :: %s\n  the expectation cannot be re-serialised: %s\n", file->key,
                             vector->id, write_error);
                sk_arena_free(scratch);
                continue;
            }
            if (encoded_len != vector->expected_len ||
                memcmp(encoded, vector->expected, encoded_len) != 0) {
                failed += 1;
                sk_failure failure = {
                    .file = file->key,
                    .id = vector->id,
                    .note = "a canonical document did not survive a parse and a write",
                    .expected = vector->expected,
                    .actual = encoded,
                    .error = NULL,
                };
                (void)printf("%s\n\n", sk_failure_describe(scratch, &failure));
                sk_arena_free(scratch);
                continue;
            }
            checked += 1;
            sk_arena_free(scratch);
        }
    }

    (void)printf("round trip  %d of %d expectations re-serialised identically\n", checked,
                 checked + failed);
    sk_arena_free(arena);
    return failed == 0 ? 0 : 1;
}

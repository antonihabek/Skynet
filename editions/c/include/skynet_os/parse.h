/*
 * A JSON parser, for the two things this edition reads: the corpus files, and the `expected` strings
 * inside them that the `roundtrip` gate re-serialises.
 *
 * Deliberately a parser of JSON rather than of canonical JSON. The corpus files are written by
 * `tools/corpus-gen/generate.ts` through `JSON.stringify` with indentation, so they carry whitespace,
 * unsorted keys and `\uXXXX` escapes for anything the writer here would emit as UTF-8 — and a parser
 * that only accepted the canonical form could not read the corpus it exists to read.
 *
 * What it refuses is everything outside JSON proper: a trailing comma, a comment, `NaN`, `Infinity`,
 * a bare word, a leading `+`, a leading zero, a control character inside a string. Each of those is
 * a file this edition cannot have produced, and accepting one would be reading a document nobody
 * wrote. §3.9's rule applies here rather than being softened because the input is a checked-in file:
 * all input from outside the process is untrusted, and a corpus path is an argument.
 */
#ifndef SKYNET_OS_PARSE_H
#define SKYNET_OS_PARSE_H

#include <stdbool.h>
#include <stddef.h>

#include "skynet_os/arena.h"
#include "skynet_os/value.h"

/* Where a parse stopped and why. `message` points at a static string or into `arena`. */
typedef struct {
    const char *message;
    size_t offset;
    int line;
    int column;
} sk_parse_error;

/*
 * Parse `len` bytes. Returns NULL on failure with `error` filled in.
 *
 * Trailing whitespace is allowed; trailing anything else is not, so a file holding two documents
 * fails rather than silently reporting the first.
 */
const sk_value *sk_parse(sk_arena *arena, const char *bytes, size_t len, sk_parse_error *error);

/* Render an error as `line:column: message`, into `arena`. */
const char *sk_parse_error_text(sk_arena *arena, const sk_parse_error *error);

#endif /* SKYNET_OS_PARSE_H */

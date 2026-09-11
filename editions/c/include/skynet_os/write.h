/*
 * The canonical writer, and the growable buffer it writes into.
 *
 * corpus/README.md's five rules, applied here and nowhere else in this edition. The one that needs
 * a header comment is rule 3, because it is the one a platform library will not do for you:
 *
 *   An integral value below 2^53 as a decimal integer, -0 as 0; anything else as the shortest
 *   decimal that round-trips, **always positional, never exponential**.
 *
 * `printf("%.17g")` is none of those three things: it is not shortest, it is exponential above and
 * below a threshold it picks, and `%g` strips trailing zeros in a way that depends on the value. So
 * the search the reference runs — the shortest precision whose rendering reads back as the same
 * double — is run here too, and the exponential form it produces is converted to a positional one.
 *
 * # Failure
 *
 * `sk_write` returns false on a non-finite number and says so in `error`. This is the reference's
 * behaviour: `formatNumber` throws rather than emitting `null`, because a document that wrote NaN as
 * `null` would be claiming a measurement was absent when in fact it was not finite, and §24 spends a
 * rule on that distinction. It is the only way writing a value can fail.
 */
#ifndef SKYNET_OS_WRITE_H
#define SKYNET_OS_WRITE_H

#include <stdbool.h>
#include <stddef.h>

#include "skynet_os/arena.h"
#include "skynet_os/value.h"

/* A growable byte buffer. `bytes` is NUL-terminated for printf's benefit and `len` is the truth. */
typedef struct {
    sk_arena *arena;
    char *bytes;
    size_t len;
    size_t capacity;
} sk_buf;

void sk_buf_init(sk_buf *buf, sk_arena *arena);
void sk_buf_append(sk_buf *buf, const char *bytes, size_t len);
void sk_buf_append_str(sk_buf *buf, const char *text);
void sk_buf_append_char(sk_buf *buf, char byte);

/*
 * Format one number in canonical form into `out`, which must hold at least SK_NUMBER_MAX bytes.
 * Returns false for a non-finite value, having written nothing.
 */
#define SK_NUMBER_MAX 512
bool sk_format_number(double number, char *out, size_t out_size);

/*
 * Serialise `value` in canonical form.
 *
 * `error` is filled with the reason on failure and left alone otherwise; pass NULL to discard it.
 */
bool sk_write(sk_arena *arena, const sk_value *value, sk_buf *out, const char **error);

/* `sk_write` into a fresh buffer, returning the bytes and their length. NULL on failure. */
const char *sk_encode(sk_arena *arena, const sk_value *value, size_t *len, const char **error);

#endif /* SKYNET_OS_WRITE_H */

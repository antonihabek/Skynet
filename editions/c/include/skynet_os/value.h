/*
 * The canonical JSON value model — this edition's half of `reference/typescript/canonical.ts`.
 *
 * §31.4 requires byte-identical output, and corpus/README.md states the form: no insignificant
 * whitespace, object keys sorted by their UTF-8 bytes, numbers finite and positional, only the
 * quote, the backslash and the C0 controls escaped, absent fields omitted and `null` written. This
 * header is the value tree those rules are applied to; write.h applies them.
 *
 * # Strings carry a length
 *
 * A JSON string may contain U+0000, which rule 4 says is written as an escape rather than dropped.
 * A model built on NUL-terminated C strings would truncate exactly there and pass every other case,
 * so a string here is bytes plus a length, and the NUL `sk_arena_copy` writes past the end is a
 * convenience for printf rather than a terminator anything relies on.
 *
 * # Numbers are doubles, including the ones that are not finite
 *
 * corpus/README.md's rule about a `null` in an input: where the reference's type is a bare `number`,
 * the null is how JSON spells a non-finite value and reads as NaN. So NaN is representable here, and
 * the writer is where it stops — `sk_write` fails on a non-finite number rather than emitting
 * `null`, which is the reference's behaviour and is the difference between "this was not measured"
 * and "this is absent".
 *
 * # Objects keep insertion order and are written sorted
 *
 * The two are not in tension: rule 2 sorts at write time, and the order a contract inserted in is
 * what a reader of a failure sees. Where an *array's* order follows an object's iteration —
 * `clamped` in a cycle verdict — the corpus generator sorts the input's keys, which
 * corpus/README.md records as a rule of the format, so iteration here is insertion order and
 * matches the reference's.
 */
#ifndef SKYNET_OS_VALUE_H
#define SKYNET_OS_VALUE_H

#include <stdbool.h>
#include <stddef.h>

#include "skynet_os/arena.h"

typedef enum {
    /* Zero on purpose: an arena hands back zeroed memory, so an uninitialised value is null rather
     * than a tag no switch has a case for. */
    SK_NULL = 0,
    SK_BOOL,
    SK_NUMBER,
    SK_STRING,
    SK_ARRAY,
    SK_OBJECT
} sk_kind;

typedef struct sk_value sk_value;

/* One member of an object, in the order it was inserted. */
typedef struct sk_member {
    const char *key;
    size_t key_len;
    const sk_value *value;
    struct sk_member *next;
} sk_member;

struct sk_value {
    sk_kind kind;
    bool truth;    /* SK_BOOL */
    double number; /* SK_NUMBER */
    const char *text;
    size_t text_len; /* SK_STRING */
    /* SK_ARRAY: a grown vector, because an array is built by appending and read by index.
     * SK_OBJECT: `members` and `last`, a singly linked list with a tail pointer, because an object
     * is built by appending and read by key. */
    const sk_value **items;
    size_t count;
    size_t capacity;
    sk_member *members;
    sk_member *last;
};

/* ── Constructors. None of these returns NULL; see arena.h's invariant. ─────────────────────── */

const sk_value *sk_null(void);
const sk_value *sk_bool(bool truth);
const sk_value *sk_number(sk_arena *arena, double number);
/* `text` is copied into the arena, so a caller may pass a stack buffer. */
const sk_value *sk_string(sk_arena *arena, const char *text);
const sk_value *sk_string_len(sk_arena *arena, const char *text, size_t len);

sk_value *sk_array(sk_arena *arena);
void sk_push(sk_arena *arena, sk_value *array, const sk_value *item);

sk_value *sk_object(sk_arena *arena);
/*
 * Append `key` to `object`.
 *
 * A key already present is replaced in place, keeping its original position, which is what
 * assigning to a field twice does in the reference. Aborts on a NULL value: `sk_set(o, "k", NULL)`
 * is a caller that meant `sk_null()` and got a tree with a hole in it, and rule 5's whole point is
 * that absent and null are different facts — so the third one, a C null pointer, is a bug rather
 * than a third fact.
 */
void sk_set(sk_arena *arena, sk_value *object, const char *key, const sk_value *value);
/* `sk_set` unless `value` is NULL, in which case the field stays absent — the reference's
 * `undefined`, which rule 5 omits. This is the one place a NULL value is meaningful, and it is
 * spelled differently from `sk_set` so that it is a decision at the call site. */
void sk_set_optional(sk_arena *arena, sk_value *object, const char *key, const sk_value *value);

/* ── Reading. Every accessor is total: a kind mismatch answers the fallback rather than trapping,
 * because a contract reading a document reads whatever the document holds, exactly as the
 * reference does when a field's declared type and its actual value disagree. ───────────────── */

sk_kind sk_kind_of(const sk_value *value);
bool sk_is_null(const sk_value *value);
/* True when the field is absent (a C NULL) or present and null — the reference's `x == null`,
 * which is the loose comparison it uses throughout and which is true of both. */
bool sk_nullish(const sk_value *value);

bool sk_as_bool(const sk_value *value, bool fallback);
double sk_as_number(const sk_value *value, double fallback);
/* The bytes and length of a string, or `fallback` and its length for anything else. Never NULL. */
const char *sk_as_string(const sk_value *value, size_t *len, const char *fallback);
/* A string compared against a C literal. False for a non-string, and length-aware, so a value
 * holding a NUL in the middle does not compare equal to the bytes before it. */
bool sk_string_is(const sk_value *value, const char *literal);

size_t sk_len(const sk_value *value);
/* Element `index`, or NULL past the end — which `sk_nullish` reads as the reference's `undefined`. */
const sk_value *sk_at(const sk_value *value, size_t index);
/* Member `key`, or NULL when absent. */
const sk_value *sk_get(const sk_value *value, const char *key);
/* The members of an object in insertion order, for the contracts that iterate one. NULL for
 * anything else, which is an empty iteration. */
const sk_member *sk_members(const sk_value *value);

/* True when `value` is an object with a member `key`, present even if null — the reference's
 * `key in object`, which is a different question from `object[key] != null`. */
bool sk_has(const sk_value *value, const char *key);

#endif /* SKYNET_OS_VALUE_H */

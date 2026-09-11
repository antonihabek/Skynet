/*
 * The arena every value in this edition is allocated from.
 *
 * # Why an arena rather than malloc and free
 *
 * §31.3 gives C a Certified ceiling, and the build that ceiling exists for is a MISRA C safety-MCU
 * with no dynamic allocation after init. This edition is not that build — it is §42's L2, the
 * contracts, running on a host with a corpus to read — but the allocation strategy is the one
 * decision an L2 edition can make in the direction of the L4 one rather than away from it. The
 * contracts here allocate from a region, never individually, and free nothing: a corpus case is
 * one region, reset between cases, and no contract function contains a `free`.
 *
 * That is also what makes the contract code readable. The reference is TypeScript, where every
 * intermediate object is garbage; transcribing it into C with per-object ownership would put a
 * lifetime decision in every line of a file whose job is to compute the same bytes.
 *
 * # The invariant, written down because it is the one that would bite
 *
 * `sk_arena_alloc` never returns NULL. It aborts the process instead, naming the request it could
 * not serve. This is deliberate and it is the loud option rather than the quiet one: the
 * alternative is a NULL check at every one of several thousand construction sites, which in
 * practice means a missing one somewhere, and a contract that reads a NULL value computes a wrong
 * answer instead of stopping. A corpus runner that dies saying it is out of memory has told the
 * truth; one that emits a plausible document has not.
 *
 * Callers may therefore treat every constructor in value.h as infallible. Nothing else in this
 * edition may assume that about anything.
 *
 * # Blocks, and why pointers are stable
 *
 * The arena is a list of blocks. A block is never moved, never grown in place and never freed until
 * the whole arena is, so a pointer handed out stays valid for the arena's life. Contracts hold
 * pointers into values they built earlier; a bump allocator that reallocated would be a
 * use-after-free in every one of them.
 */
#ifndef SKYNET_OS_ARENA_H
#define SKYNET_OS_ARENA_H

#include <stddef.h>

typedef struct sk_arena sk_arena;

/* A new, empty arena. Aborts rather than returning NULL (see above). */
sk_arena *sk_arena_new(void);

/*
 * `size` bytes, aligned for any type this edition uses, zeroed.
 *
 * Zeroed because a value model with a tag and a union has exactly one safe default — the tag's
 * zero, which value.h fixes as SK_NULL — and reading an uninitialised tag is the bug this removes
 * for the cost of a memset the corpus cannot measure.
 */
void *sk_arena_alloc(sk_arena *arena, size_t size);

/* `count * size` bytes, with the multiplication checked. Aborts on overflow. */
void *sk_arena_alloc_array(sk_arena *arena, size_t count, size_t size);

/* A copy of `len` bytes from `bytes`, with one NUL past the end so it can also be read as a C
 * string. The length is what callers use; the NUL is a convenience for printf and nothing more,
 * and is not a terminator this edition relies on — a JSON string may contain U+0000. */
char *sk_arena_copy(sk_arena *arena, const char *bytes, size_t len);

/* Release every block. Every pointer from this arena is dangling afterwards. */
void sk_arena_free(sk_arena *arena);

/* Total bytes handed out, for the self-check's accounting. */
size_t sk_arena_used(const sk_arena *arena);

#endif /* SKYNET_OS_ARENA_H */

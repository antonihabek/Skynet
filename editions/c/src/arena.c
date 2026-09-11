#include "skynet_os/arena.h"

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/*
 * 64 KiB, doubling to a ceiling. Small enough that a run of the roles corpus does not reserve a
 * megabyte per case, large enough that the sealed MCAP files in `blackbox/` do not allocate a block
 * per record.
 */
#define SK_BLOCK_FIRST ((size_t)64 * 1024)
#define SK_BLOCK_MAX ((size_t)8 * 1024 * 1024)

/* Every allocation is aligned for the widest type this edition stores: a double or a pointer. */
#define SK_ALIGN (sizeof(void *) > sizeof(double) ? sizeof(void *) : sizeof(double))

typedef struct sk_block {
    struct sk_block *next;
    size_t size;
    size_t used;
    /* Flexible array rather than a second allocation: one malloc per block, and the header is in
     * the same cache line as the first bytes handed out of it. */
    unsigned char bytes[];
} sk_block;

struct sk_arena {
    sk_block *current;
    size_t next_block;
    size_t used;
};

/*
 * The one place this edition gives up.
 *
 * Named `die` rather than folded into the caller because every path that reaches it is a condition
 * no contract can act on, and a reader looking for "what happens when allocation fails" should find
 * one function rather than a scattering of `exit` calls. Written to stderr and flushed: a message
 * buffered behind a runner's stdout is a message nobody sees.
 */
static void die(const char *what, size_t size)
{
    (void)fflush(stdout);
    (void)fprintf(stderr, "skynet-corpus-c: out of memory %s %zu bytes\n", what, size);
    (void)fflush(stderr);
    abort();
}

sk_arena *sk_arena_new(void)
{
    sk_arena *arena = calloc(1, sizeof *arena);
    if (arena == NULL) {
        die("allocating an arena of", sizeof *arena);
    }
    arena->next_block = SK_BLOCK_FIRST;
    return arena;
}

static size_t round_up(size_t size)
{
    size_t remainder = size % SK_ALIGN;
    if (remainder == 0) {
        return size;
    }
    if (size > SIZE_MAX - (SK_ALIGN - remainder)) {
        die("aligning", size);
    }
    return size + (SK_ALIGN - remainder);
}

static void add_block(sk_arena *arena, size_t least)
{
    size_t size = arena->next_block;
    while (size < least) {
        if (size > SIZE_MAX / 2) {
            size = least;
            break;
        }
        size *= 2;
    }
    sk_block *block = malloc(sizeof *block + size);
    if (block == NULL) {
        die("growing an arena by", size);
    }
    block->next = arena->current;
    block->size = size;
    block->used = 0;
    arena->current = block;
    if (arena->next_block < SK_BLOCK_MAX) {
        arena->next_block *= 2;
    }
}

void *sk_arena_alloc(sk_arena *arena, size_t size)
{
    size_t wanted = round_up(size == 0 ? SK_ALIGN : size);
    if (arena->current == NULL || arena->current->size - arena->current->used < wanted) {
        add_block(arena, wanted);
    }
    unsigned char *out = arena->current->bytes + arena->current->used;
    arena->current->used += wanted;
    arena->used += wanted;
    memset(out, 0, wanted);
    return out;
}

void *sk_arena_alloc_array(sk_arena *arena, size_t count, size_t size)
{
    if (size != 0 && count > SIZE_MAX / size) {
        die("allocating an array of", count);
    }
    return sk_arena_alloc(arena, count * size);
}

char *sk_arena_copy(sk_arena *arena, const char *bytes, size_t len)
{
    if (len == SIZE_MAX) {
        die("copying", len);
    }
    char *out = sk_arena_alloc(arena, len + 1);
    if (len != 0) {
        memcpy(out, bytes, len);
    }
    out[len] = '\0';
    return out;
}

void sk_arena_free(sk_arena *arena)
{
    if (arena == NULL) {
        return;
    }
    sk_block *block = arena->current;
    while (block != NULL) {
        sk_block *next = block->next;
        free(block);
        block = next;
    }
    free(arena);
}

size_t sk_arena_used(const sk_arena *arena)
{
    return arena == NULL ? 0 : arena->used;
}

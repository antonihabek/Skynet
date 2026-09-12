/*
 * The contracts, and the table that routes a corpus file to one.
 *
 * Every function declared here is written from the modules under `reference/typescript` and from nothing else —
 * never from `corpus/`, which would agree by construction, and never from the Python, C++ or Go
 * editions, which would prove only that one hand did both. What holds them honest is the corpus, in
 * the one direction that means something: this edition reproducing what the oracle produced, byte
 * for byte.
 *
 * One file under `src/contracts/` per module there, named the same, so a reader holding the reference
 * open can find the C beside it. The dispatch is the only file with no counterpart: the reference has
 * no such table, because in TypeScript the corpus generator calls the functions directly.
 */
#ifndef SKYNET_OS_CONTRACTS_H
#define SKYNET_OS_CONTRACTS_H

#include "skynet_os/corpus.h"

/*
 * Every corpus file this edition answers, keyed by `corpus/file`.
 *
 * A file absent from this table is counted as *not implemented* rather than as passing, and the
 * runner prints the two separately — §31.4's "an edition that has not passed the corpus is not an
 * edition" is not satisfied by a table with fewer entries in it.
 */
const sk_dispatch_entry *sk_dispatch(size_t *count);

#endif /* SKYNET_OS_CONTRACTS_H */

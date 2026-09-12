/*
 * The closed string sets — `corpus/vocabulary/closed-sets.json`, and the only contract in this edition
 * with no module of its own in `reference/typescript`.
 *
 * Every other corpus holds an edition to a *computation*: give it this input, emit these bytes. This one
 * holds it to a **vocabulary**, and the generator's own comment says why it exists: `capability_reserved`
 * was added to the Rust `PackageRejectionCode` and not to the reference's, and nothing could see it — the
 * mirror pins guard the reference, and every corpus covers the cases it holds, none of which had ever
 * mentioned a token invented ten minutes earlier. A missing member is now a corpus failure on the first
 * run, with the set named.
 *
 * Seventy-four sets, and **order is part of the agreement** for several of them: `os_tiers` and
 * `scts_areas` are ranked, `operating_states` and `state_reasons` run in the order `evaluate` descends
 * through its rules, and `skill_phases` runs in diagram order so that "never moves backwards through this
 * list" is a property somebody can assert. An edition that sorted any of those alphabetically would pass
 * every other corpus and would have lost the property.
 */
#ifndef SKYNET_OS_VOCABULARY_H
#define SKYNET_OS_VOCABULARY_H

#include "skynet_os/corpus.h"

const sk_value *sk_vocabulary_closed_sets_case(sk_arena *arena, const sk_value *input,
                                               const char **error);

/* Every set name, in declaration order, for the self-check to walk. `count` is written with the number
 * of sets; the returned array is NULL-terminated as well, so neither reader has to trust the other. */
const char *const *sk_vocabulary_set_names(size_t *count);

#endif /* SKYNET_OS_VOCABULARY_H */

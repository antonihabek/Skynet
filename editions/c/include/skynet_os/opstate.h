/*
 * §27's operating-state machine — `reference/typescript/opstate.ts`.
 *
 * Pure, clockless and total: every combination of conditions produces a state, including combinations
 * nobody anticipated, which is the property §33.3 asks of anything that converges.
 *
 * Three corpus files, and the two small ones are the interesting ones. `evaluate` is thirty-eight scripted
 * conditions. `properties` freezes `safe_state`, `commands` and `sticky` for **every** state, and `warning`
 * freezes the tone for **every ordered pair** of states — forty-nine of them — so an edition cannot get one
 * transition right by accident and the rest wrong.
 */
#ifndef SKYNET_OS_OPSTATE_H
#define SKYNET_OS_OPSTATE_H

#include "skynet_os/corpus.h"

const sk_value *sk_opstate_evaluate_case(sk_arena *arena, const sk_value *input, const char **error);
const sk_value *sk_opstate_properties_case(sk_arena *arena, const sk_value *input, const char **error);
const sk_value *sk_opstate_warning_case(sk_arena *arena, const sk_value *input, const char **error);

/* §27's states in rule order. Exposed so the self-check can hold this list and the `operating_states` entry
 * of `src/contracts/vocabulary.c` to each other: the same seven names live in both files, and nothing in the
 * corpus compares them — each is measured against the oracle separately, and two lists that agreed with the
 * oracle at different times would still both pass. `count` is written with the number of states. */
const char *const *sk_opstate_states(size_t *count);

#endif /* SKYNET_OS_OPSTATE_H */

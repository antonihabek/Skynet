/*
 * Declarative predicate evaluation — `reference/typescript/predicate.ts`.
 *
 * Compatibility rules are data, not code, so a third party can register a robot or a hardware
 * constraint without shipping a frontend change. The module is pure and **total**: it never fails on
 * a malformed rule, and a rule it cannot decide does not pass.
 *
 * # The two answers, and why the boolean is derived
 *
 * §54.2 asks a question a boolean cannot answer: *a precondition that cannot be evaluated refuses the
 * skill, naming the path that was unresolvable.* `false` refuses, which is the right direction, but it
 * collapses "the world is not ready" and "this manifest names something this robot does not expose"
 * into one answer — a battery that needs charging and a skill that will never run here, whose
 * publisher needs telling.
 *
 * So there is one evaluator, and the boolean is `outcome == SK_SATISFIED` of it. Two functions
 * computing the same thing by two routes is the drift this repository exists to prevent, and a
 * refinement that cannot disagree with the thing it refines has nothing to drift from.
 *
 * # Where ignorance used to pass
 *
 * Five inputs answered `true` before the outcome existed, all for the same reason — JavaScript's
 * `undefined` arriving at `!`, `!==` and `[].every(...)` — and all now refuse. The one that mattered:
 * `includes_all(robot.capabilities, {__ref: subject.required_capabilities})` **passed when the
 * policy's capability list was missing.** A rule that passes because the fact is absent is worse than
 * one that fails, because nobody goes looking for it.
 *
 * `exists` is the escape hatch and always was: it is the one operator whose job is to ask whether a
 * path resolves, so it answers unsatisfied rather than unevaluable for an absent path, and
 * `not(exists(p))` still means what its author meant.
 */
#ifndef SKYNET_OS_PREDICATE_H
#define SKYNET_OS_PREDICATE_H

#include <stdbool.h>
#include <stddef.h>

#include "skynet_os/corpus.h"

typedef enum { SK_SATISFIED, SK_UNSATISFIED, SK_UNEVALUABLE } sk_outcome_kind;

/*
 * Why a predicate could not be decided.
 *
 * A closed vocabulary, because §54.2 requires the refusal to *name* something and a free-text reason
 * is the same nothing as a free-text precondition. `corpus/vocabulary/closed-sets.json` holds every
 * edition to this list.
 */
typedef enum {
    SK_REASON_NONE,
    /* Not an object, or `op` is not a string. There is no predicate here. */
    SK_MALFORMED,
    /* An operator this edition does not implement. */
    SK_UNKNOWN_OP,
    /* The predicate names no path to read. */
    SK_MISSING_PATH,
    /* `not` with nothing to negate. */
    SK_MISSING_OPERAND,
    /* A path that names nothing in this context. This is the one §54.2 is about. */
    SK_UNRESOLVED_PATH,
    /* An operand is present but is not a finite number, and the operator compares numbers. */
    SK_NOT_A_NUMBER,
    /* An operand is present but is not a version, or not a range. */
    SK_NOT_A_VERSION
} sk_unevaluable_reason;

typedef struct {
    sk_outcome_kind outcome;
    sk_unevaluable_reason reason;
    /* The path that could not be resolved or read, when there is one. NULL otherwise. */
    const char *path;
    /* The operator that could not decide. NULL when there was no readable operator at all. */
    const char *op;
} sk_predicate_outcome;

sk_predicate_outcome sk_predicate_evaluate_outcome(sk_arena *arena, const sk_value *predicate,
                                                   const sk_value *context);
/* Whether an outcome permits whatever it was guarding. Only satisfied does. */
bool sk_predicate_satisfied(const sk_predicate_outcome *outcome);
bool sk_predicate_evaluate(sk_arena *arena, const sk_value *predicate, const sk_value *context);

/*
 * Read `a.b.c` out of a context. NULL — the reference's `undefined` — for any missing link.
 *
 * **Members only.** An array segment must be a canonical decimal index, so `capabilities.length` does
 * not resolve: `length` is a property of a JavaScript array rather than a member of the JSON
 * document, and an edition without JavaScript arrays could not reproduce it. The same restriction
 * keeps a path off the prototype chain, where a path composed by somebody else has no business going.
 */
const sk_value *sk_read_path(const sk_value *context, const char *path, size_t len);

const char *sk_outcome_name(sk_outcome_kind kind);
const char *sk_reason_name(sk_unevaluable_reason reason);

const sk_value *sk_predicate_evaluate_case(sk_arena *arena, const sk_value *input,
                                            const char **error);

#endif /* SKYNET_OS_PREDICATE_H */

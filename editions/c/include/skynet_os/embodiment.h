/*
 * The embodiment descriptor and the role vocabulary — `reference/typescript/embodiment.ts`.
 *
 * The role vocabulary is the part of this module the whole system leans on: a role is how two bodies
 * from two vendors are recognised as having the same shoulder, and §6.3's transport plan is built out
 * of the match. The rule that keeps it honest is that an unrecognised role never *silently* maps to
 * something plausible — `sk_role_parse` reports that it could not read one, and the caller treats
 * that as "this joint can only be matched by exact name". A body the vocabulary does not describe is
 * still usable; it just cannot take part in cross-body remapping, which is the truthful outcome,
 * because nobody has established what its joints mean.
 */
#ifndef SKYNET_OS_EMBODIMENT_H
#define SKYNET_OS_EMBODIMENT_H

#include <stdbool.h>
#include <stddef.h>

#include "skynet_os/corpus.h"

typedef struct {
    bool valid;
    const char *chain;
    const char *link;
    /* NULL when the role named no axis, which two segments always mean: `torso.yaw` is a link called
     * `yaw`, not an axis. */
    const char *axis;
    /* An ordinal for repeated links. `has_index` false is the reference's null, and it is a separate
     * field because `base.wheel#0` is a valid role whose index is zero — a sentinel would read it as
     * absent, which is the corpus case `ordinal-zero` exists for. */
    bool has_index;
    double index;
} sk_role;

/* Parse a canonical role, after trimming and lower-casing. `valid` false for anything the vocabulary
 * does not describe, including an absent or empty role. */
sk_role sk_role_parse(sk_arena *arena, const sk_value *role);
sk_role sk_role_parse_text(sk_arena *arena, const char *role, size_t len);
/* The canonical string form, so two parsed roles can be compared as keys. */
const char *sk_role_key(sk_arena *arena, const sk_role *role);

const sk_value *sk_roles_vocabulary_case(sk_arena *arena, const sk_value *input, const char **error);

#endif /* SKYNET_OS_EMBODIMENT_H */

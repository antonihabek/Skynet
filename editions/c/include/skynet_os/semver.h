/*
 * Minimal semver parsing, comparison and range satisfaction — `reference/typescript/semver.ts`.
 *
 * It is a contract rather than a utility because §54.2 made `semver_satisfies` an operator the robot
 * evaluates before a skill may start: a robot deciding whether its runtime satisfies `^1.2.0`
 * differently from the Hub that sold it the skill is the §31.2 failure, *a format with two
 * implementations of its own definition is not a format*.
 *
 * Note what is deliberately **not** shared with it. `package.ts` compares OS versions with its own
 * numeric-segment comparator, because `min_os_version` is matched against whatever an image reports,
 * which is `2.0` as often as `2.0.0`. Two comparators with two jobs.
 */
#ifndef SKYNET_OS_SEMVER_H
#define SKYNET_OS_SEMVER_H

#include <stdbool.h>
#include <stddef.h>

#include "skynet_os/corpus.h"

/*
 * A parsed version. `valid` false is the reference's `null` return, and the other fields are then
 * meaningless.
 *
 * The three components are doubles because the reference's are: `Number(match[1])` over a digit run
 * with no length limit, so a version whose major is twenty digits long parses to the double nearest
 * it and is written back out that way. An `int` here would answer a different question from the
 * oracle's on exactly that input.
 */
typedef struct {
    bool valid;
    double major;
    double minor;
    double patch;
    /* NULL when there is no prerelease, which is a different fact from an empty one — the grammar
     * cannot produce an empty prerelease, and the comparison treats absence as outranking. */
    const char *prerelease;
    const char *raw;
} sk_version;

/* Parse, after trimming. `valid` false when the input is not `major.minor.patch[-prerelease]`. */
sk_version sk_semver_parse(sk_arena *arena, const char *text, size_t len);
bool sk_semver_valid_version(sk_arena *arena, const char *text, size_t len);

/* -1 if a < b, 0 if equal, 1 if a > b — and 0 for anything either side could not parse, which is the
 * reference's answer and is why `package.ts` does not use this one. */
int sk_semver_compare(const sk_version *a, const sk_version *b);

bool sk_semver_satisfies(sk_arena *arena, const char *version, size_t version_len, const char *range,
                         size_t range_len);
/*
 * Is `range` a range this edition can read at all?
 *
 * `satisfies` answers false both for "this version is outside that window" and for "that is not a
 * window", and §54.2 needs those apart: a skill whose precondition names an unreadable range is a
 * skill nobody can ever run here, and its publisher is the one who has to hear about it.
 */
bool sk_semver_valid_range(sk_arena *arena, const char *range, size_t range_len);

/* The corpus handlers. */
const sk_value *sk_semver_satisfies_case(sk_arena *arena, const sk_value *input, const char **error);
const sk_value *sk_semver_compare_case(sk_arena *arena, const sk_value *input, const char **error);

#endif /* SKYNET_OS_SEMVER_H */

#include "skynet_os/contracts.h"

#include "skynet_os/embodiment.h"
#include "skynet_os/predicate.h"
#include "skynet_os/semver.h"

/*
 * The table. Entries are added as contract files are written, and a corpus file absent from it is
 * counted as not implemented — which is the only reason this edition can be built and run before it
 * is finished without the runner reporting something untrue.
 */
static const sk_dispatch_entry ENTRIES[] = {
    {"predicate/evaluate", sk_predicate_evaluate_case},
    {"roles/vocabulary", sk_roles_vocabulary_case},
    {"semver/compare", sk_semver_compare_case},
    {"semver/satisfies", sk_semver_satisfies_case},
    {NULL, NULL},
};

const sk_dispatch_entry *sk_dispatch(size_t *count)
{
    /* The trailing sentinel is not an entry. */
    *count = (sizeof ENTRIES / sizeof ENTRIES[0]) - 1;
    return ENTRIES;
}

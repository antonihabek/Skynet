#include "skynet_os/contracts.h"

#include "skynet_os/embodiment.h"
#include "skynet_os/intent.h"
#include "skynet_os/opstate.h"
#include "skynet_os/predicate.h"
#include "skynet_os/readiness.h"
#include "skynet_os/semver.h"
#include "skynet_os/vocabulary.h"
#include "skynet_os/watchdog.h"

/*
 * The table. Entries are added as contract files are written, and a corpus file absent from it is
 * counted as not implemented — which is the only reason this edition can be built and run before it
 * is finished without the runner reporting something untrue.
 */
static const sk_dispatch_entry ENTRIES[] = {
    {"intent/offer", sk_intent_offer_case},
    {"opstate/evaluate", sk_opstate_evaluate_case},
    {"opstate/properties", sk_opstate_properties_case},
    {"opstate/warning", sk_opstate_warning_case},
    {"predicate/evaluate", sk_predicate_evaluate_case},
    {"readiness/fleet", sk_readiness_fleet_case},
    {"readiness/robot", sk_readiness_robot_case},
    {"roles/vocabulary", sk_roles_vocabulary_case},
    {"semver/compare", sk_semver_compare_case},
    {"semver/satisfies", sk_semver_satisfies_case},
    {"vocabulary/closed-sets", sk_vocabulary_closed_sets_case},
    {"watchdog/chains", sk_watchdog_case},
    {"watchdog/command-loss", sk_watchdog_case},
    {"watchdog/nesting", sk_watchdog_case},
    {NULL, NULL},
};

const sk_dispatch_entry *sk_dispatch(size_t *count)
{
    /* The trailing sentinel is not an entry. */
    *count = (sizeof ENTRIES / sizeof ENTRIES[0]) - 1;
    return ENTRIES;
}

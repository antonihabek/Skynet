/*
 * OS readiness — `reference/typescript/readiness.ts`.
 *
 * The certification suite awards a tier from *measurements*: an estop latency taken at the actuator, a
 * joint that refused 150% of its rating, a rollback that recovered unattended. None of those exist
 * until somebody runs a bench suite against real hardware. This module answers the question that can
 * be answered now, from the registry alone: **given what has been declared about this robot, which
 * tier could it even attempt?**
 *
 * Collapsing the two would produce the exact lie the platform is built to avoid — a page reading "host
 * tier" over a robot nobody has ever measured — so the vocabulary is deliberately different. This
 * returns an **attainable** tier and a list of **blockers**; certification returns an *awarded* tier.
 * Attainable means "the paperwork is in order and you may now go and measure it", and it is never
 * evidence of anything about the machine.
 *
 * The distinction has a pleasant side effect: the blockers are a to-do list. A vendor reading "declares
 * no watchdog window" knows precisely what to publish next, which is more useful than a red cross.
 */
#ifndef SKYNET_OS_READINESS_H
#define SKYNET_OS_READINESS_H

#include "skynet_os/corpus.h"

const sk_value *sk_readiness_robot_case(sk_arena *arena, const sk_value *input, const char **error);
const sk_value *sk_readiness_fleet_case(sk_arena *arena, const sk_value *input, const char **error);

#endif /* SKYNET_OS_READINESS_H */

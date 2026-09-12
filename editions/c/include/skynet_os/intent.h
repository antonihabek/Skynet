/*
 * §11's pull — what a robot does with a desired state it has just been handed.
 * `reference/typescript/intent.ts`.
 *
 * > **The Hub stores intent. The robot reconciles.** Never push.
 *
 * The document arrives over a network, which makes it the least trusted input in the system that is
 * also the most consequential: it decides what software runs on a machine that moves. §33.8 says what
 * a robot does with a corrupt file on its own disk; this answers the question one step earlier — may
 * this response become that file at all?
 *
 * Refusing is not ignoring. Two of the four faults refuse the offer, and in both the robot carries on
 * reconciling against what it already has — which §34.1 makes the *ordinary* state rather than a
 * failure mode, because a robot is offline most of the time. A refused offer costs nothing that being
 * offline does not.
 */
#ifndef SKYNET_OS_INTENT_H
#define SKYNET_OS_INTENT_H

#include "skynet_os/corpus.h"

const sk_value *sk_intent_offer_case(sk_arena *arena, const sk_value *input, const char **error);

#endif /* SKYNET_OS_INTENT_H */

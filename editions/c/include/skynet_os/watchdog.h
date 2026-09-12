/*
 * The watchdog chain (§46) — `reference/typescript/watchdog.ts`.
 *
 * Four layers, each catching a failure the one above it cannot see. Layer 3 is petted by the **Safety
 * Kernel**, not by `skynetd`, and that is what makes it mean anything: the hardware watchdog is
 * satisfied only while the thing that actually enforces the envelope is running its cycle. A watchdog
 * petted by a supervisor tells you the supervisor is alive, which is not the question anybody asked.
 *
 * # A reset is not a stop
 *
 * §46.2's uncomfortable part. When the SoC resets, the drives lose their command source, and what they
 * then do is a property of the *hardware*. Some hold, some brake, some fault — and **some coast**, on
 * which body layer 3's own recovery path is a fall.
 *
 * So the refusal has an unusual and correct shape: **the middle rung is the unsafe one.** Guest is
 * fine, because enforcement is advisory and nothing relies on a hardware watchdog. Certified is fine,
 * because layer 4 cuts drive enable independently of the processor. Only host, where the OS owns the
 * control loop and layer 3 is the last line, is the configuration where a reset means a fall.
 */
#ifndef SKYNET_OS_WATCHDOG_H
#define SKYNET_OS_WATCHDOG_H

#include "skynet_os/corpus.h"

const sk_value *sk_watchdog_case(sk_arena *arena, const sk_value *input, const char **error);

#endif /* SKYNET_OS_WATCHDOG_H */

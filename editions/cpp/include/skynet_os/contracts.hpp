// Every contract this edition implements, as one declaration per corpus kind.
//
// One header rather than one per contract, because the dispatch table is the thing that has to
// stay in step with the corpus and a table assembled from thirty-three headers is a table nobody
// reads. Each function takes the corpus file's label — several kinds span several files with
// different inputs — and throws `Unimplemented` for a label it has not transcribed.

#ifndef SKYNET_OS_CONTRACTS_HPP
#define SKYNET_OS_CONTRACTS_HPP

#include "skynet_os/json.hpp"

#include <string>

namespace skynet::contracts {

/// §5's canonical role vocabulary: what a joint means, across two vendors' spelling of it.
[[nodiscard]] Json roles(const std::string& label, const Json& input);

/// The closed sets — every enumeration the wire admits, and what an unrecognised word becomes.
[[nodiscard]] Json vocabulary(const std::string& label, const Json& input);

/// Semantic versions: ordering, and whether one satisfies a range.
[[nodiscard]] Json semver(const std::string& label, const Json& input);

/// §54.2's declarative preconditions: three outcomes, because a boolean cannot name a path.
[[nodiscard]] Json predicate(const std::string& label, const Json& input);

/// §44.3's frame graph: well formed on its own, and against the joint list of the body it describes.
[[nodiscard]] Json frames(const std::string& label, const Json& input);

/// §5's `detects_people`, derived off a whole descriptor.
[[nodiscard]] Json embodiment_descriptor(const std::string& label, const Json& input);

/// §44's forward kinematics: every frame's pose, and the ones that have none.
[[nodiscard]] Json kinematics_solved(const std::string& label, const Json& input);

/// §7.6's resolved ceilings — the set an operator is shown before anything runs.
[[nodiscard]] Json limits(const std::string& label, const Json& input);

/// §7's per-cycle verdict: what the Safety Kernel lets reach the actuators.
[[nodiscard]] Json cycle(const std::string& label, const Json& input);

/// §25.1's observation assembly: the frame, and the instant every stream can answer for.
[[nodiscard]] Json observation(const std::string& label, const Json& input);

/// §6's transport plan: how a policy's action space reaches a body, or why it cannot.
[[nodiscard]] Json transport(const std::string& label, const Json& input);

/// The `fuzz` corpus, which is three families of generated vectors for three contracts above.
[[nodiscard]] Json fuzz(const std::string& label, const Json& input);

/// §8's version digest: one identity for a version, or precisely why it has none.
[[nodiscard]] Json digest(const std::string& label, const Json& input);

/// §47.6's frame pool: a handle names one generation of one slot, and nothing else.
[[nodiscard]] Json media(const std::string& label, const Json& input);

/// §27's operating state machine, its three predicates, and §49.1's warning matrix.
[[nodiscard]] Json opstate(const std::string& label, const Json& input);

/// §46's watchdog chain: four layers, the host-tier refusal, and the nesting report.
[[nodiscard]] Json watchdog(const std::string& label, const Json& input);

/// §11's pull: may this response become the desired state?
[[nodiscard]] Json intent(const std::string& label, const Json& input);

/// §31.5's readiness: what a robot has declared, and the tier it could attempt.
[[nodiscard]] Json readiness(const std::string& label, const Json& input);

/// §54.2's live precondition context, where absent is never null.
[[nodiscard]] Json live(const std::string& label, const Json& input);

/// §56's provenance: what an on-robot result is worth, and whether two roll-ups of it agree.
[[nodiscard]] Json provenance(const std::string& label, const Json& input);

/// §15's certification award: the tier the evidence supports, never the tier claimed.
[[nodiscard]] Json certification(const std::string& label, const Json& input);

/// §50's birth certificate: what it establishes about this machine, and what it cannot.
[[nodiscard]] Json birth(const std::string& label, const Json& input);

/// §55.2's cell descriptor: what an operator sees on save, and what a precondition reads after.
[[nodiscard]] Json cell(const std::string& label, const Json& input);

/// §74's telemetry: one window, and what a set of them covers.
[[nodiscard]] Json telemetry(const std::string& label, const Json& input);

/// §12, §34.3 and §79's transfers: fetching an image, sending a window, and checking both.
[[nodiscard]] Json transfer(const std::string& label, const Json& input);

/// §36's power and thermal assessment: the state, and whether its number can be believed.
[[nodiscard]] Json power(const std::string& label, const Json& input);

/// §87.4's relay: the session's own verdict, and the whole permission table beside it.
[[nodiscard]] Json relay(const std::string& label, const Json& input);

/// §88: what admits a motion request on a program-level body, and whether one in flight is overdue.
[[nodiscard]] Json execute(const std::string& label, const Json& input);

/// §45's calibration validity, and the guards an expired set takes away.
[[nodiscard]] Json calibration(const std::string& label, const Json& input);

/// §10, §19 and §50's identity: a pairing code, a credential's state, and what is running.
[[nodiscard]] Json identity(const std::string& label, const Json& input);

/// §12's OTA: may this image be installed, and is the slot it booted into good.
[[nodiscard]] Json ota(const std::string& label, const Json& input);

/// §11 and §17's check-in: what a robot may claim, its health, and the document it sends.
[[nodiscard]] Json checkin(const std::string& label, const Json& input);

/// §8.2's package admission: every finding, in order, and what will be granted.
[[nodiscard]] Json admission(const std::string& label, const Json& input);

/// §54's skills: preconditions, success, timeout, composition and the lifecycle.
[[nodiscard]] Json skill(const std::string& label, const Json& input);

/// §13 and §26's black box: the ring's records and header, the MCAP schemas, and a sealed file.
[[nodiscard]] Json blackbox(const std::string& label, const Json& input);

}  // namespace skynet::contracts

#endif  // SKYNET_OS_CONTRACTS_HPP

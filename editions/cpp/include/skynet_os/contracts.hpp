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

}  // namespace skynet::contracts

#endif  // SKYNET_OS_CONTRACTS_HPP

// §10's identity, shared. See `src/contracts/identity.cpp` for the reasoning.
//
// Exposed because §17's check-in carries the attestation block, and a second rendering of §10's
// sentence would be two answers to "what is this robot running".

#ifndef SKYNET_OS_IDENTITY_HPP
#define SKYNET_OS_IDENTITY_HPP

#include "skynet_os/json.hpp"

#include <string>
#include <vector>

namespace skynet::identity {

struct Finding {
  std::string code;
  std::string severity;
  std::string message;
  std::vector<std::string> subjects;
};

/// §10's sentence, and which of its three slots nobody read.
struct Attestation {
  bool complete = false;
  std::string sentence;
  std::vector<std::string> unmeasured;
};

/// §10's sentence, which **cannot fail** — see the implementation for why that matters.
[[nodiscard]] Attestation attest(const Json& attestation);

/// One measure, with the digest kept rather than rendered away and gated on the state.
[[nodiscard]] Json measure_block(const Json& measure);

/// The attestation as §10 asks for it: the sentence **and** the three digests it is made of.
[[nodiscard]] Json attestation_block(const Json& attestation);

}  // namespace skynet::identity

#endif  // SKYNET_OS_IDENTITY_HPP

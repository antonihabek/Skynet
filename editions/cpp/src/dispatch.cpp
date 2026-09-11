// The dispatch table: corpus kind to the contract that answers it.
//
// Keyed on the kind and, where one kind spans several files with different inputs, the file label
// — which the contract itself switches on. The Rust runner learned the reason for the second half
// the hard way: a catch-all arm meant a new fuzz family arrived not as an unsupported corpus but as
// forty-eight vectors dispatched into an arm never written for them. Every entry here says which
// kind it is for, and a kind absent from this table is reported as having no handler rather than
// quietly passing through something that happens to compile.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"

#include <map>
#include <string>

namespace skynet {
namespace {

using Handler = Json (*)(const std::string& label, const Json& input);

const std::map<std::string, Handler>& handlers() {
  static const std::map<std::string, Handler> table = {
      {"roles", &contracts::roles},
      {"vocabulary", &contracts::vocabulary},
      {"semver", &contracts::semver},
      {"predicate", &contracts::predicate},
      {"frames", &contracts::frames},
      {"embodiment", &contracts::embodiment_descriptor},
      {"kinematics", &contracts::kinematics_solved},
      {"limits", &contracts::limits},
      {"cycle", &contracts::cycle},
      {"observation", &contracts::observation},
      {"transport", &contracts::transport},
      {"fuzz", &contracts::fuzz},
      {"digest", &contracts::digest},
      {"media", &contracts::media},
      {"opstate", &contracts::opstate},
      {"watchdog", &contracts::watchdog},
      {"intent", &contracts::intent},
      {"readiness", &contracts::readiness},
      {"live", &contracts::live},
      {"provenance", &contracts::provenance},
      {"certification", &contracts::certification},
      {"birth", &contracts::birth},
      {"cell", &contracts::cell},
      {"telemetry", &contracts::telemetry},
      {"transfer", &contracts::transfer},
      {"power", &contracts::power},
      {"calibration", &contracts::calibration},
      {"identity", &contracts::identity},
      {"ota", &contracts::ota},
      {"checkin", &contracts::checkin},
      {"admission", &contracts::admission},
      {"skill", &contracts::skill},
      {"blackbox", &contracts::blackbox},
  };
  return table;
}

}  // namespace

bool has_handler(const std::string& corpus) { return handlers().count(corpus) != 0; }

Json dispatch(const std::string& corpus, const std::string& label, const Json& input) {
  const auto found = handlers().find(corpus);
  if (found == handlers().end()) {
    // Unreachable through `run_corpus`, which asks `has_handler` first. Loud rather than a default
    // value, because the alternative is a corpus silently answered by nothing.
    throw Unimplemented(corpus, label, "no handler is registered for this corpus kind");
  }
  return found->second(label, input);
}

}  // namespace skynet

// The `vocabulary` corpus: every closed set, as the reference edition declares it.
//
// The table itself is in `src/vocabulary.cpp`, not here, because the sets are needed by the
// contracts that classify against them as well as by this corpus. One table, one place — §3.3 of
// the coding standard, and the reason the Python edition keeps its own in a module rather than in
// its corpus handler.

#include "skynet_os/contracts.hpp"
#include "skynet_os/vocabulary.hpp"

#include <string>

namespace skynet::contracts {

Json vocabulary(const std::string& /*label*/, const Json& input) {
  const std::optional<std::string> name = input["set"].string_value();
  if (!name.has_value()) {
    // Loud rather than `[]`. An empty vocabulary is a legitimate answer for some sets, so a case
    // with no `set` named that serialised to `[]` would pass by accident.
    throw JsonError("no set named");
  }
  Json out = Json::array();
  for (const std::string_view member : skynet::vocabulary::members(*name)) {
    out.push(Json::string(std::string(member)));
  }
  return out;
}

}  // namespace skynet::contracts

// The `fuzz` corpus — generated vectors, dispatched to the contract each family is about.
//
// One kind spanning three files, which is the case the runner's `Unimplemented` mechanism exists
// for: `cycle`, `observation` and `transport` are the same three contracts the curated corpora
// exercise, fed inputs nobody chose by hand. A catch-all arm here would send a fourth family, the
// day one is added, into whichever contract happened to be first — the Rust runner learned that
// the hard way, with forty-eight vectors dispatched into an arm never written for them.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"

#include <string>

namespace skynet::contracts {

Json fuzz(const std::string& label, const Json& input) {
  if (label == "cycle") {
    return cycle(label, input);
  }
  if (label == "observation") {
    return observation(label, input);
  }
  if (label == "transport") {
    return transport(label, input);
  }
  throw Unimplemented("fuzz", label);
}

}  // namespace skynet::contracts

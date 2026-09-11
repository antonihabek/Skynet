// Reading the conformance corpus.
//
// Split out from the runner because two programs read it: the corpus runner itself and the
// canonical-JSON round trip, which runs before any contract exists. One reader means the two
// cannot disagree about which files are in the corpus — the kind of drift §31.2 keeps formats
// single-source to prevent, applied to this edition's own tooling.

#ifndef SKYNET_OS_CORPUS_FILES_HPP
#define SKYNET_OS_CORPUS_FILES_HPP

#include "skynet_os/json.hpp"

#include <string>
#include <vector>

namespace skynet {

/// One case: an input document, and the exact bytes an edition must produce from it.
struct Case {
  std::string id;
  std::string note;
  Json input;
  /// The oracle's canonical JSON, compared byte for byte — never parsed and compared field-wise.
  std::string expected;
};

/// One corpus file: its declared kind, the label taken from the path, and its cases.
struct CorpusFile {
  std::string path;
  std::string corpus;
  /// The file stem, which the dispatch uses to tell `semver/compare` from `semver/satisfies`.
  std::string label;
  std::vector<Case> cases;
};

/// Where the corpus is, when nobody said: `$SKYNET_CORPUS`, else the nearest `corpus/` directory at
/// or above the working directory. Returns an empty string when there is none — the caller reports
/// that, because a runner that silently found no cases would exit 0 having proved nothing.
[[nodiscard]] std::string default_corpus_root();

/// Read every `*/*.json` under `root`, sorted so a run is reproducible.
[[nodiscard]] std::vector<CorpusFile> load_corpus(const std::string& root);

}  // namespace skynet

#endif  // SKYNET_OS_CORPUS_FILES_HPP

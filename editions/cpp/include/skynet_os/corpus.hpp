// The conformance corpus runner, C++ edition.
//
//     An edition that has not passed the corpus is not an edition. It cannot carry the name,
//     appear in the matrix, or be installed by `skynetd`. (§31.4)
//
// This is that mechanism for the C++ edition. It reads `corpus/**/*.json`, runs each case's input
// through this edition's contracts, serialises the result in canonical form, and compares it to
// the case's `expected` **string** — byte for byte, not field by field.
//
// # An unimplemented corpus is reported, never skipped
//
// This edition is incomplete, and a runner that quietly passed 4 of 33 corpora and printed "OK"
// would be the single most misleading artifact in this repository. So every corpus with no handler
// is counted as `unimplemented` against the total, and the honest sentence about where this
// edition stands is always available as a number.
//
// A corpus is not the unit, though. `fuzz` is one kind spanning three files — cycle, observation
// and transport — so the day one of them is answered the other two arrive at a handler never
// written for them. Counting those as **failures** would be a different claim from the true one,
// and a louder one: 148 cases nobody has transcribed yet would read as 148 cases this edition gets
// wrong. So a handler may decline a file by throwing `Unimplemented`, the runner counts it in the
// same bucket as a corpus with no handler at all, and a corpus holding a declined file is never
// named `complete`.
//
// Declining is a property of a **file**. A handler that declines some of a file's cases and
// answers others is a defect in this edition rather than a gap in it, and the runner reports those
// as failures for that reason.

#ifndef SKYNET_OS_CORPUS_HPP
#define SKYNET_OS_CORPUS_HPP

#include "skynet_os/corpus_files.hpp"
#include "skynet_os/json.hpp"

#include <exception>
#include <optional>
#include <string>
#include <vector>

namespace skynet {

/// Thrown by a handler for a corpus file this edition has not transcribed yet. Distinct from a
/// failure, and the distinction is the whole point — see the header comment.
class Unimplemented : public std::exception {
 public:
  Unimplemented(std::string corpus, std::string label, std::string note = {});
  [[nodiscard]] const char* what() const noexcept override { return message_.c_str(); }
  [[nodiscard]] const std::string& corpus() const { return corpus_; }
  [[nodiscard]] const std::string& label() const { return label_; }

 private:
  std::string corpus_;
  std::string label_;
  std::string message_;
};

struct Failure {
  std::string corpus;
  std::string label;
  std::string case_id;
  std::string note;
  std::string expected;
  std::string produced;
  /// Index of the first differing byte, or nothing when the case produced no output at all.
  std::optional<std::size_t> diverged_at;
  /// Set instead of the byte comparison when the handler threw.
  std::string reason;

  [[nodiscard]] std::string describe() const;
};

struct Report {
  int passed = 0;
  int failed = 0;
  /// Cases in corpora this edition has no handler for at all. Counted, never hidden.
  int no_handler_cases = 0;
  /// Cases in files a handler exists for and declined by name. The same bucket, kept separate
  /// because the two say different things about where the edition stands.
  int declined_cases = 0;
  std::vector<Failure> failures;
  /// Corpus kinds with no handler.
  std::vector<std::string> missing_handlers;
  /// `corpus/label` for every file a handler declined.
  std::vector<std::string> declined_files;
  /// Corpus kinds where every case passed **and** no file was declined.
  std::vector<std::string> complete;
  /// Corpus kinds holding at least one declined file. Never named complete, however many of their
  /// other files pass — a corpus is complete or it is not.
  std::vector<std::string> partial;

  [[nodiscard]] int unimplemented() const { return no_handler_cases + declined_cases; }
  [[nodiscard]] int total() const { return passed + failed + unimplemented(); }
  [[nodiscard]] std::string summary() const;
};

/// Run every case in every file.
[[nodiscard]] Report run_corpus(const std::vector<CorpusFile>& files);

/// Answer one case. Declared here so the dispatch table has one definition and the tests can reach
/// it; defined in `src/dispatch.cpp`, which is the only file that knows the whole contract set.
[[nodiscard]] Json dispatch(const std::string& corpus, const std::string& label, const Json& input);

/// Whether any handler at all exists for a corpus kind. Kept separate from `dispatch` so a kind
/// with no handler is reported once per corpus rather than once per case.
[[nodiscard]] bool has_handler(const std::string& corpus);

}  // namespace skynet

#endif  // SKYNET_OS_CORPUS_HPP

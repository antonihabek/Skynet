#include "skynet_os/corpus.hpp"

#include <algorithm>
#include <map>
#include <set>

namespace skynet {
namespace {

/// Index of the first byte at which two strings differ; the shorter length when one is a prefix of
/// the other. A byte offset is what a reader needs — in a 400-character canonical document,
/// "these differ" is not a finding.
std::size_t first_difference(const std::string& expected, const std::string& produced) {
  const std::size_t shared = std::min(expected.size(), produced.size());
  for (std::size_t at = 0; at < shared; ++at) {
    if (expected[at] != produced[at]) {
      return at;
    }
  }
  return shared;
}

/// What happened to one case while the file was being evaluated. The file is evaluated in full
/// before anything is recorded, because declining is a property of a *file* and cannot be acted on
/// one case at a time: whether a declined case is *not yet written* or *an inconsistency in the
/// handler* depends on what the file's other cases did.
enum class Outcome { Produced, Declined, Raised };

struct Evaluated {
  Outcome outcome = Outcome::Produced;
  const Case* subject = nullptr;
  std::string produced;
  std::string error;
};

void add_once(std::vector<std::string>& into, const std::string& value) {
  if (std::find(into.begin(), into.end(), value) == into.end()) {
    into.push_back(value);
  }
}

}  // namespace

Unimplemented::Unimplemented(std::string corpus, std::string label, std::string note)
    : corpus_(std::move(corpus)), label_(std::move(label)) {
  message_ = corpus_ + "/" + label_ + " is not implemented in this edition";
  if (!note.empty()) {
    message_ += " - " + note;
  }
}

std::string Failure::describe() const {
  if (!reason.empty()) {
    return corpus + "/" + label + " :: " + case_id + " - " + reason;
  }
  std::string marker;
  if (diverged_at.has_value()) {
    marker = " (first difference at byte " + std::to_string(*diverged_at) + ")";
  }
  return corpus + "/" + label + " :: " + case_id + marker + "\n" +  //
         "    note     " + note + "\n" +                            //
         "    expected " + expected + "\n" +                        //
         "    produced " + produced;
}

std::string Report::summary() const {
  return std::to_string(passed) + " passed, " + std::to_string(failed) + " failed, " +
         std::to_string(unimplemented()) + " not implemented (" +
         std::to_string(no_handler_cases) + " in corpora with no handler, " +
         std::to_string(declined_cases) + " in files a handler declined), " +
         std::to_string(total()) + " total";
}

Report run_corpus(const std::vector<CorpusFile>& files) {
  Report report;
  /// Per corpus kind: whether every case of every file of that kind passed. A kind is the unit
  /// `complete` is a statement about, and one kind's files need not be adjacent in path order.
  std::map<std::string, bool> all_passed;

  for (const CorpusFile& file : files) {
    if (!has_handler(file.corpus)) {
      report.no_handler_cases += static_cast<int>(file.cases.size());
      add_once(report.missing_handlers, file.corpus);
      continue;
    }

    std::vector<Evaluated> evaluated;
    evaluated.reserve(file.cases.size());
    int declined_count = 0;
    for (const Case& one : file.cases) {
      Evaluated entry;
      entry.subject = &one;
      try {
        entry.produced = dispatch(file.corpus, file.label, one.input).canonical();
        entry.outcome = Outcome::Produced;
      } catch (const Unimplemented&) {
        entry.outcome = Outcome::Declined;
        ++declined_count;
      } catch (const std::exception& error) {
        // A raising case is a failing case, reported rather than propagated: one contract that
        // throws must not stop the other thirty-two being measured.
        entry.outcome = Outcome::Raised;
        entry.error = error.what();
      }
      evaluated.push_back(std::move(entry));
    }

    const bool whole_file_declined =
        declined_count > 0 && declined_count == static_cast<int>(evaluated.size());
    if (whole_file_declined) {
      report.declined_cases += declined_count;
      add_once(report.declined_files, file.corpus + "/" + file.label);
      add_once(report.partial, file.corpus);
      continue;
    }

    bool& kind_all_passed = all_passed.try_emplace(file.corpus, true).first->second;

    for (const Evaluated& entry : evaluated) {
      Failure failure;
      failure.corpus = file.corpus;
      failure.label = file.label;
      failure.case_id = entry.subject->id;
      failure.note = entry.subject->note;
      failure.expected = entry.subject->expected;

      if (entry.outcome == Outcome::Declined) {
        // Some of this file answered and some declined. Counting the declined ones as *not
        // implemented* would hide a live inconsistency behind a number that reads as honest
        // progress, which is the one thing this runner exists to prevent.
        failure.reason = "handler declined " + std::to_string(declined_count) + " of " +
                         std::to_string(evaluated.size()) +
                         " cases in this file - Unimplemented is a property of a file, not of a "
                         "case";
      } else if (entry.outcome == Outcome::Raised) {
        failure.reason = entry.error;
      } else if (entry.produced == entry.subject->expected) {
        ++report.passed;
        continue;
      } else {
        failure.produced = entry.produced;
        failure.diverged_at = first_difference(entry.subject->expected, entry.produced);
      }

      ++report.failed;
      kind_all_passed = false;
      report.failures.push_back(std::move(failure));
    }
  }

  for (const auto& [kind, passed] : all_passed) {
    if (passed && std::find(report.partial.begin(), report.partial.end(), kind) ==
                      report.partial.end()) {
      report.complete.push_back(kind);
    }
  }

  std::sort(report.complete.begin(), report.complete.end());
  std::sort(report.partial.begin(), report.partial.end());
  std::sort(report.declined_files.begin(), report.declined_files.end());
  std::sort(report.missing_handlers.begin(), report.missing_handlers.end());
  return report;
}

}  // namespace skynet

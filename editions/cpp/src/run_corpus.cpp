// Run the conformance corpus against the C++ edition.
//
//     skynet-corpus-cpp [--corpus-root corpus] [--verbose] [--json] [--min-passed N]
//
// Exit code 0 only when every case in every corpus **that has a handler** passed, and the summary
// always says how many cases are not implemented — split into the corpora with no handler at all
// and the individual files a handler declined. Those facts together are the honest sentence about
// this edition, and none of them is useful without the others: §31.4 says an edition that has not
// passed the corpus is not an edition, so "8 of 33 corpora, all passing" has to be as visible as
// the zero failures, and a corpus this edition answers *part* of has to be visibly neither.
//
// `--min-passed` is the ratchet, and it exists because everything above is a **floor of zero**. An
// edition whose handler was deleted answers nothing, fails nothing, and exits 0: the cases move
// from *passed* into *not implemented*, which is the bucket that reads as honest progress. That is
// the one regression this runner's own design cannot see, so the number a CI step passes here is
// the number this edition has reached, and lowering it is a decision somebody makes in a diff.

#include "skynet_os/corpus.hpp"

#include <cstdio>
#include <cstdlib>
#include <string>
#include <vector>

namespace {

void print_list(const char* heading, const std::vector<std::string>& values) {
  if (values.empty()) {
    return;
  }
  std::string joined;
  for (std::size_t at = 0; at < values.size(); ++at) {
    if (at > 0) {
      joined += ", ";
    }
    joined += values[at];
  }
  std::printf("%s %s\n", heading, joined.c_str());
}

skynet::Json as_json_array(const std::vector<std::string>& values) {
  skynet::Json out = skynet::Json::array();
  for (const std::string& value : values) {
    out.push(skynet::Json::string(value));
  }
  return out;
}

}  // namespace

int main(int argc, char** argv) {
  std::string root;
  bool verbose = false;
  bool as_json = false;
  long min_passed = -1;

  for (int at = 1; at < argc; ++at) {
    const std::string argument = argv[at];
    if (argument == "--verbose") {
      verbose = true;
    } else if (argument == "--json") {
      as_json = true;
    } else if (argument == "--corpus-root" && at + 1 < argc) {
      root = argv[++at];
    } else if (argument == "--min-passed" && at + 1 < argc) {
      min_passed = std::strtol(argv[++at], nullptr, 10);
    } else {
      std::fprintf(stderr, "unknown argument: %s\n", argument.c_str());
      return 2;
    }
  }

  if (root.empty()) {
    root = skynet::default_corpus_root();
  }
  if (root.empty()) {
    std::fprintf(stderr,
                 "no corpus found. Pass --corpus-root, or set SKYNET_CORPUS, or run from inside "
                 "the repository.\n");
    return 2;
  }

  std::vector<skynet::CorpusFile> files;
  try {
    files = skynet::load_corpus(root);
  } catch (const std::exception& error) {
    std::fprintf(stderr, "could not read the corpus at %s: %s\n", root.c_str(), error.what());
    return 2;
  }
  if (files.empty()) {
    std::fprintf(stderr, "no corpus at %s\n", root.c_str());
    return 2;
  }

  const skynet::Report report = skynet::run_corpus(files);

  if (as_json) {
    // Deliberately the same fields the human report prints, and no derived ones: a summary that
    // computed a percentage here and a page that computed it differently would be two answers to
    // one question.
    skynet::Json out = skynet::Json::object();
    out.set("passed", skynet::Json::number(report.passed));
    out.set("failed", skynet::Json::number(report.failed));
    out.set("unimplemented", skynet::Json::number(report.unimplemented()));
    out.set("total", skynet::Json::number(report.total()));
    out.set("complete", as_json_array(report.complete));
    out.set("partial", as_json_array(report.partial));
    out.set("missing_handlers", as_json_array(report.missing_handlers));
    out.set("declined_files", as_json_array(report.declined_files));
    std::printf("%s\n", out.canonical().c_str());
    return report.failed != 0 ? 1 : 0;
  }

  std::printf("corpus     %s\n", root.c_str());
  std::printf("result     %s\n", report.summary().c_str());
  print_list("complete  ", report.complete);
  print_list("no handler", report.missing_handlers);
  print_list("declined  ", report.declined_files);
  // Named separately from `complete` on purpose: these are corpora this edition answers part of,
  // and a reader who saw only the passing files would take them for finished ones.
  print_list("partial   ", report.partial);

  if (!report.failures.empty()) {
    std::printf("\n");
    const std::size_t shown = verbose ? report.failures.size()
                                      : std::min<std::size_t>(report.failures.size(), 10);
    for (std::size_t at = 0; at < shown; ++at) {
      std::printf("%s\n\n", report.failures[at].describe().c_str());
    }
    if (shown < report.failures.size()) {
      std::printf("... and %zu more (pass --verbose)\n", report.failures.size() - shown);
    }
  }

  if (min_passed >= 0 && report.passed < min_passed) {
    std::printf(
        "\nregression: %d passed, and this edition had reached %ld. Cases that stop being "
        "answered are counted as *not implemented* rather than as failures, so nothing above this "
        "line went red - which is why the floor is checked separately from the failures.\n",
        report.passed, min_passed);
    return 1;
  }

  return report.failed != 0 ? 1 : 0;
}

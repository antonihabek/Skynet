// The canonical-JSON round trip, over the corpus itself.
//
// Every `expected` string in `corpus/**/*.json` is canonical JSON produced by the TypeScript
// oracle. So parsing one and re-serialising it must return the same bytes, and that single
// property exercises the three things an edition's writer can get wrong — number formatting, key
// ordering and string escaping — against 1,148 real values rather than against examples somebody
// chose while writing the writer.
//
// This runs before any contract is implemented, deliberately. A canonical writer that disagrees
// with the oracle makes every later corpus failure look like a contract bug, and §7.6 of the
// coding standard asks for the differential test against the reference *first*.

#include "skynet_os/corpus_files.hpp"
#include "skynet_os/json.hpp"

#include <cstdio>
#include <string>
#include <vector>

namespace {

int failures = 0;
int checked = 0;

void check_roundtrip(const std::string& where, const std::string& canonical_text) {
  ++checked;
  try {
    const std::string again = skynet::Json::parse(canonical_text).canonical();
    if (again != canonical_text) {
      ++failures;
      std::size_t at = 0;
      while (at < again.size() && at < canonical_text.size() && again[at] == canonical_text[at]) {
        ++at;
      }
      std::printf("  FAIL %s (first difference at byte %zu)\n", where.c_str(), at);
      std::printf("    oracle   %s\n", canonical_text.c_str());
      std::printf("    produced %s\n", again.c_str());
    }
  } catch (const skynet::JsonError& error) {
    ++failures;
    std::printf("  FAIL %s — %s\n", where.c_str(), error.what());
  }
}

}  // namespace

int main(int argc, char** argv) {
  const std::string root = argc > 1 ? argv[1] : skynet::default_corpus_root();
  std::vector<skynet::CorpusFile> files;
  try {
    files = skynet::load_corpus(root);
  } catch (const std::exception& error) {
    std::printf("could not read the corpus at %s — %s\n", root.c_str(), error.what());
    return 2;
  }
  if (files.empty()) {
    std::printf("no corpus at %s\n", root.c_str());
    return 2;
  }

  for (const skynet::CorpusFile& file : files) {
    for (const skynet::Case& one : file.cases) {
      check_roundtrip(file.corpus + "/" + file.label + " :: " + one.id, one.expected);
    }
  }

  // The number formatter's own edges, which the corpus does not necessarily contain. Each is a
  // rule from the canonical spec rather than a value somebody liked the look of.
  struct NumberCase {
    double value;
    const char* expected;
  };
  const NumberCase numbers[] = {
      {0.0, "0"},        {-0.0, "0"},          {1.0, "1"},          {-1.0, "-1"},
      {1.5, "1.5"},      {-0.44, "-0.44"},     {0.1, "0.1"},        {1e21, "1000000000000000000000"},
      {1e-7, "0.0000001"}, {9007199254740991.0, "9007199254740991"},
      {0.30000000000000004, "0.30000000000000004"},
  };
  for (const NumberCase& one : numbers) {
    ++checked;
    const std::string produced = skynet::format_number(one.value);
    if (produced != one.expected) {
      ++failures;
      std::printf("  FAIL number %s — produced %s\n", one.expected, produced.c_str());
    }
  }

  // Rule 5: an absent field is not `null`, and the two must not render the same way.
  ++checked;
  skynet::Json absent = skynet::Json::object();
  skynet::Json explicit_null = skynet::Json::object();
  explicit_null.set("declared", skynet::Json::null());
  if (absent.canonical() != "{}" || explicit_null.canonical() != R"({"declared":null})") {
    ++failures;
    std::printf("  FAIL rule 5 — absent %s, null %s\n", absent.canonical().c_str(),
                explicit_null.canonical().c_str());
  }

  // Strictly-true, not truthy. Every edition's self-check turns on this distinction.
  ++checked;
  if (skynet::Json::number(1).is_true() || skynet::Json::string("yes").is_true() ||
      !skynet::Json::boolean(true).is_true()) {
    ++failures;
    std::printf("  FAIL is_true() accepted something that is not `true`\n");
  }

  std::printf("json round trip: %d checked, %d failed\n", checked, failures);
  return failures == 0 ? 0 : 1;
}

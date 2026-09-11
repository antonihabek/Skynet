#include "skynet_os/corpus_files.hpp"

#include <algorithm>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <sstream>

namespace skynet {
namespace {

namespace fs = std::filesystem;

/// A corpus directory is one holding the corpus README beside its kind directories. Checking for
/// that rather than for the bare name stops a run picking up some unrelated `corpus/` on the way
/// up — a wrong corpus that loaded would report figures about nothing.
bool looks_like_corpus(const fs::path& candidate) {
  std::error_code ignored;
  return fs::is_directory(candidate, ignored) && fs::exists(candidate / "README.md", ignored);
}

std::string read_file(const fs::path& path) {
  std::ifstream stream(path, std::ios::binary);
  if (!stream) {
    throw JsonError("cannot open " + path.string());
  }
  std::ostringstream buffer;
  buffer << stream.rdbuf();
  return buffer.str();
}

}  // namespace

std::string default_corpus_root() {
  if (const char* named = std::getenv("SKYNET_CORPUS"); named != nullptr && *named != '\0') {
    return named;
  }
  std::error_code ignored;
  fs::path here = fs::current_path(ignored);
  for (int depth = 0; depth < 12 && !here.empty(); ++depth) {
    const fs::path candidate = here / "corpus";
    if (looks_like_corpus(candidate)) {
      return candidate.string();
    }
    const fs::path parent = here.parent_path();
    if (parent == here) {
      break;
    }
    here = parent;
  }
  return {};
}

std::vector<CorpusFile> load_corpus(const std::string& root) {
  std::vector<fs::path> paths;
  std::error_code ignored;
  for (const fs::directory_entry& kind : fs::directory_iterator(root, ignored)) {
    if (!kind.is_directory()) {
      continue;
    }
    for (const fs::directory_entry& file : fs::directory_iterator(kind.path(), ignored)) {
      if (file.is_regular_file() && file.path().extension() == ".json") {
        paths.push_back(file.path());
      }
    }
  }
  // Sorted on the generic (forward-slash) spelling so a run on Windows visits files in the same
  // order as a run on Linux. Order does not change any verdict; a report that reordered itself by
  // platform would still be one nobody could diff.
  std::sort(paths.begin(), paths.end(), [](const fs::path& left, const fs::path& right) {
    return left.generic_string() < right.generic_string();
  });

  std::vector<CorpusFile> files;
  files.reserve(paths.size());
  for (const fs::path& path : paths) {
    const Json document = Json::parse(read_file(path));
    CorpusFile file;
    file.path = path.generic_string();
    file.corpus = document["corpus"].string_or("");
    file.label = path.stem().string();
    if (file.corpus.empty()) {
      throw JsonError(file.path + " declares no corpus kind");
    }
    for (const Json& one : document["cases"].items()) {
      Case parsed;
      parsed.id = one["id"].string_or("<unnamed>");
      parsed.note = one["note"].string_or("");
      parsed.input = one["input"];
      if (!one["expected"].is_string()) {
        throw JsonError(file.path + " :: " + parsed.id + " has no expected string");
      }
      parsed.expected = one["expected"].string_or("");
      file.cases.push_back(std::move(parsed));
    }
    files.push_back(std::move(file));
  }
  return files;
}

}  // namespace skynet

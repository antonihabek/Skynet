#include "skynet_os/text.hpp"

namespace skynet::text {
namespace {

bool is_ascii_space(char ch) {
  return ch == ' ' || ch == '\t' || ch == '\n' || ch == '\r' || ch == '\f' || ch == '\v';
}

}  // namespace

std::string ascii_lower(std::string_view value) {
  std::string out(value);
  for (char& ch : out) {
    if (is_ascii_upper(ch)) {
      ch = static_cast<char>(ch - 'A' + 'a');
    }
  }
  return out;
}

std::string strip(std::string_view value) {
  std::size_t first = 0;
  while (first < value.size() && is_ascii_space(value[first])) {
    ++first;
  }
  std::size_t last = value.size();
  while (last > first && is_ascii_space(value[last - 1])) {
    --last;
  }
  return std::string(value.substr(first, last - first));
}

std::vector<std::string> split(std::string_view value, char separator) {
  std::vector<std::string> out;
  std::size_t start = 0;
  for (;;) {
    const std::size_t at = value.find(separator, start);
    if (at == std::string_view::npos) {
      out.emplace_back(value.substr(start));
      return out;
    }
    out.emplace_back(value.substr(start, at - start));
    start = at + 1;
  }
}

bool starts_with(std::string_view value, std::string_view prefix) {
  return value.size() >= prefix.size() && value.compare(0, prefix.size(), prefix) == 0;
}

}  // namespace skynet::text

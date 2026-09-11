#include "skynet_os/text.hpp"

namespace skynet::text {

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

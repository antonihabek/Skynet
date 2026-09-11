// Version digests (§8) — the C++ edition of `reference/typescript/digest.ts`.
//
//     sha256(
//       "skynet.version.v1" \n
//       kind                \n
//       version             \n
//       canonical(manifest) \n
//       "<path>\0<sha256>"  \n … per file, sorted by path
//     )
//
// The separator between a path and its hash is a **NUL** rather than a space: it is the one byte a
// path may not contain, which `check_path` enforces, and that is what makes the framing unambiguous
// rather than merely unlikely to collide. It is written as a named constant for a reason worth
// repeating — the Hub's copy holds the raw byte, a terminal renders it as nothing, and that is how
// the TypeScript implementation came to be written against a space and found out from a digest that
// did not match.
//
// # The two rules that exist so a robot and the Hub agree or neither computes an identity
//
// A manifest carrying a number at or above 1e21 or below 1e-6 is **refused**, because that is where
// `JSON.stringify` reaches for an exponent and §31.4's canonical rule forbids one. So is a manifest
// whose object *keys* carry a character above U+FFFF, because the Hub orders keys by UTF-16 code
// unit and the canonical rule orders them by UTF-8 byte; those two orders agree throughout the
// basic multilingual plane and can disagree above it.
//
// Refusing beats guessing, for the same reason a missing envelope refuses an install: a package
// that does not install and a person reading a reason is a better outcome than a package whose
// identity is one thing on the robot and another in the registry.
//
// # Where C++ is not JavaScript
//
// **Sorting.** A byte-wise `std::string` comparison is by UTF-8 byte, and `sorted()` in most hosts
// is by code point. The reference sorts with `<`, which is by **UTF-16 code unit**, and the three
// disagree for characters above U+FFFF: as a surrogate pair such a character begins at 0xD800 and
// therefore sorts *before* U+E000…U+FFFF, while its code point sorts after. A file list containing
// an emoji and a private-use character would hash in a different order in a naive edition —
// producing a different, silently wrong digest rather than an error.

#include "skynet_os/contracts.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/sha256.hpp"
#include "skynet_os/text.hpp"

#include <algorithm>
#include <cmath>

namespace skynet::contracts {
namespace {

/// Present so a digest over a version cannot collide with a digest over anything else.
constexpr std::string_view kVersionDigestDomain = "skynet.version.v1";

constexpr double kJsMinPlain = 1e-6;
constexpr double kJsMaxPlain = 1e21;

/// The separator between a path and its hash, named rather than written inline — see the header.
const std::string& nul() {
  static const std::string value(1, '\0');
  return value;
}

/// Whether a string is a lowercase hex sha256.
bool is_hex_sha256(const Json& value) {
  if (!value.is_string()) {
    return false;
  }
  const std::string_view text = value.string_or("");
  if (text.size() != 64) {
    return false;
  }
  return std::all_of(text.begin(), text.end(), [](char ch) {
    return text::is_ascii_digit(ch) || (ch >= 'a' && ch <= 'f');
  });
}

/// Whether a relative path may be used as one, and if not, in what way.
///
/// The file list is both what is hashed and what is written to disk, and every rule here closes a
/// way for the second to leave the package's own directory or to name two things on two
/// filesystems. Checked before hashing rather than at unpack time, because a digest computed over a
/// traversal is a digest that attests to one.
std::optional<std::string> check_path(const std::string& path) {
  if (path.empty()) {
    return "empty";
  }
  if (path.size() > 1024) {
    return "longer than 1024 bytes";
  }
  if (path.front() == '/') {
    return "absolute; paths are relative to the package root";
  }
  if (path.find('\\') != std::string::npos) {
    return "contains a backslash, which is a separator on one filesystem and an ordinary filename "
           "character on another";
  }
  for (const char raw : path) {
    const auto byte = static_cast<unsigned char>(raw);
    if (byte < 0x20 || byte == 0x7F) {
      return "contains a control character";
    }
  }
  for (const std::string& component : text::split(path, '/')) {
    if (component.empty()) {
      return "has an empty component, so two spellings name one file";
    }
    if (component == ".") {
      return "has a `.` component";
    }
    if (component == "..") {
      return "has a `..` component, which leaves the package root";
    }
    if (component.back() == ' ' || component.back() == '.') {
      return "has a component ending in a space or a dot, which some filesystems silently strip";
    }
  }
  return std::nullopt;
}

struct ScanFault {
  std::string kind;
  std::string subject;
};

/// The first value in a manifest whose serialisation the two canonicalisers could disagree on.
std::optional<ScanFault> scan(const Json& value, const std::string& at) {
  // Before the number branch: a JSON boolean is not a number.
  if (value.is_bool()) {
    return std::nullopt;
  }
  if (value.is_number()) {
    const double magnitude = std::fabs(value.number_or(0));
    const bool diverges = !std::isfinite(magnitude) || magnitude >= kJsMaxPlain ||
                          (magnitude != 0 && magnitude < kJsMinPlain);
    if (diverges) {
      return ScanFault{"number_out_of_range", at.empty() ? "(root)" : at};
    }
    return std::nullopt;
  }
  if (value.is_array()) {
    const JsonArray& items = value.items();
    for (std::size_t index = 0; index < items.size(); ++index) {
      if (const auto found = scan(items[index], at + "[" + std::to_string(index) + "]");
          found.has_value()) {
        return found;
      }
    }
    return std::nullopt;
  }
  if (value.is_object()) {
    for (const auto& entry : value.entries()) {
      for (const std::uint16_t unit : js::utf16_units(entry.first)) {
        // A surrogate is half a character above U+FFFF, which is exactly what this refuses.
        if (unit >= 0xD800 && unit <= 0xDBFF) {
          return ScanFault{"key_outside_bmp",
                           (at.empty() ? "(root)" : at) + " :: " + entry.first};
        }
      }
      if (const auto found = scan(entry.second, at.empty() ? entry.first : at + "." + entry.first);
          found.has_value()) {
        return found;
      }
    }
  }
  return std::nullopt;
}

Json error_json(std::string kind, Json subject, Json detail) {
  Json error = Json::object();
  error.set("kind", Json::string(std::move(kind)));
  error.set("subject", std::move(subject));
  error.set("detail", std::move(detail));
  Json out = Json::object();
  out.set("digest", Json::null());
  out.set("error", error);
  return out;
}

}  // namespace

Json digest(const std::string& /*label*/, const Json& input) {
  const std::string kind(input["kind"].string_or(""));
  const std::string version(input["version"].string_or(""));
  // An absent or null manifest hashes as the empty object, which is what the reference's
  // `manifest ?? {}` does — not as the four characters `null`.
  static const Json kEmptyManifest = Json::object();
  const Json& manifest = input["manifest"].is_null() ? kEmptyManifest : input["manifest"];
  const JsonArray& files = input["files"].items();

  // `shapeDigest` — the digest, or the error with whatever it is *about*. The subject is a path for
  // three of the six error kinds, a field for one, a location and a key for another, and nothing
  // for the sixth. Flattening them into one field is what lets a reader see which input to go and
  // fix without knowing the error taxonomy.
  if (const auto bad = scan(manifest, ""); bad.has_value()) {
    return error_json(bad->kind, Json::string(bad->subject), Json::null());
  }

  for (const Json& file : files) {
    const std::string path(file["path"].string_or(""));
    if (const auto detail = check_path(path); detail.has_value()) {
      return error_json("path", Json::string(path), Json::string(*detail));
    }
    if (!is_hex_sha256(file["sha256"])) {
      return error_json("sha256", Json::string(path), Json::null());
    }
  }

  std::vector<const Json*> ordered;
  ordered.reserve(files.size());
  for (const Json& file : files) {
    ordered.push_back(&file);
  }
  std::stable_sort(ordered.begin(), ordered.end(), [](const Json* left, const Json* right) {
    return js::utf16_less((*left)["path"].string_or(""), (*right)["path"].string_or(""));
  });
  for (std::size_t index = 1; index < ordered.size(); ++index) {
    if ((*ordered[index])["path"].string_or("") == (*ordered[index - 1])["path"].string_or("")) {
      return error_json("duplicate_path", (*ordered[index])["path"], Json::null());
    }
  }

  std::string body;
  body += kVersionDigestDomain;
  body += "\n";
  body += kind;
  body += "\n";
  body += version;
  body += "\n";
  body += manifest.canonical();
  body += "\n";
  for (std::size_t index = 0; index < ordered.size(); ++index) {
    if (index > 0) {
      body += "\n";
    }
    body += std::string((*ordered[index])["path"].string_or(""));
    body += nul();
    body += std::string((*ordered[index])["sha256"].string_or(""));
  }

  Json out = Json::object();
  out.set("digest", Json::string(sha256_hex(body)));
  out.set("error", Json::null());
  return out;
}

}  // namespace skynet::contracts

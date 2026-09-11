// The canonical role vocabulary (§5).
//
// A role is how two different robots' joints are recognised as the same joint.
// `left_arm.shoulder.pitch` on a Unitree H1 and on a Figure 02 are different wire names for the
// same degree of freedom, and the role is what lets a policy trained on one be transported onto
// the other.
//
// The extensibility rule is the load-bearing one, and it is a refusal: **an unrecognised role
// parses to nothing, never to a nearby guess.** A body the vocabulary does not describe is still
// usable — it just cannot take part in cross-body remapping, which is the truthful outcome,
// because nobody has established what its joints mean. A vocabulary that quietly mapped
// `tentacle.segment.flex` onto the nearest arm would produce a plan that looks complete and moves
// the wrong thing.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/roles.hpp"
#include "skynet_os/text.hpp"

#include <algorithm>
#include <array>
#include <optional>
#include <string>
#include <string_view>

namespace skynet::roles {
namespace {

/// The chains a role may name. `aux` and `tail` are in the list so that a body the specification
/// did not anticipate can still declare something that parses, rather than being pushed into a
/// wrong-but-close chain because the vocabulary had no honest slot for it.
constexpr std::array<std::string_view, 12> kCanonicalChains = {
    "left_arm", "right_arm", "left_leg", "right_leg", "left_hand", "right_hand",
    "torso",    "neck",      "head",     "base",      "tail",      "aux",
};

/// Axis of motion, for a link with more than one degree of freedom. A three-DoF shoulder is three
/// joints in every real driver, and the suffix is what keeps `left_arm.shoulder.pitch` from
/// colliding with `left_arm.shoulder.roll` while both stay recognisable as the same shoulder.
constexpr std::array<std::string_view, 8> kCanonicalAxes = {
    "pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate", "grip",
};

bool is_chain(std::string_view value) {
  return std::find(kCanonicalChains.begin(), kCanonicalChains.end(), value) !=
         kCanonicalChains.end();
}

bool is_axis(std::string_view value) {
  return std::find(kCanonicalAxes.begin(), kCanonicalAxes.end(), value) != kCanonicalAxes.end();
}

}  // namespace

/// The grammar, spelled out rather than left to a regular-expression engine.
///
///     ^([a-z_]+)\.([a-z0-9_]+)(?:\.([a-z]+))?(?:#(\d+))?$
///
/// Hand-written because the same pattern must mean the same thing in four editions, and two
/// regular-expression flavours agreeing today is not the kind of guarantee a conformance corpus is
/// held together by. The one place it needs care is the optional third segment: `[a-z0-9_]+` would
/// happily swallow `shoulder.pitch` if the dot were not excluded from it, so the segments are cut
/// on dots first and classified afterwards.
std::optional<ParsedRole> parse(const std::string& role) {
  if (role.empty()) {
    return std::nullopt;
  }

  const std::string text = text::ascii_lower(text::strip(role));
  if (text.empty()) {
    return std::nullopt;
  }

  // The ordinal comes off the end first: `#` cannot appear in any segment, so splitting on it
  // cannot take a character another rule wanted.
  std::string body = text;
  std::optional<double> index;
  if (const std::size_t hash = body.find('#'); hash != std::string::npos) {
    const std::string digits = body.substr(hash + 1);
    if (digits.empty() || !std::all_of(digits.begin(), digits.end(), text::is_ascii_digit)) {
      return std::nullopt;
    }
    index = static_cast<double>(std::strtoll(digits.c_str(), nullptr, 10));
    body = body.substr(0, hash);
  }

  const std::vector<std::string> segments = text::split(body, '.');
  if (segments.size() < 2 || segments.size() > 3) {
    return std::nullopt;
  }

  ParsedRole parsed;
  parsed.chain = segments[0];
  parsed.link = segments[1];
  parsed.index = index;
  if (segments.size() == 3) {
    parsed.axis = segments[2];
  }

  // Each segment's own character class. An empty segment fails here, which is what makes
  // `torso..yaw` and a trailing dot both parse to nothing.
  const auto chain_ok = [](char ch) { return text::is_ascii_lower(ch) || ch == '_'; };
  const auto link_ok = [](char ch) {
    return text::is_ascii_lower(ch) || text::is_ascii_digit(ch) || ch == '_';
  };
  const auto axis_ok = [](char ch) { return text::is_ascii_lower(ch); };
  if (parsed.chain.empty() || !std::all_of(parsed.chain.begin(), parsed.chain.end(), chain_ok)) {
    return std::nullopt;
  }
  if (parsed.link.empty() || !std::all_of(parsed.link.begin(), parsed.link.end(), link_ok)) {
    return std::nullopt;
  }
  if (parsed.axis.has_value() &&
      (parsed.axis->empty() || !std::all_of(parsed.axis->begin(), parsed.axis->end(), axis_ok))) {
    return std::nullopt;
  }

  // Three ways to fail, and all three return the same nothing: the string does not match the
  // grammar, the chain is not in the vocabulary, or the axis is not.
  if (!is_chain(parsed.chain)) {
    return std::nullopt;
  }
  if (parsed.axis.has_value() && !is_axis(*parsed.axis)) {
    return std::nullopt;
  }
  return parsed;
}

std::optional<ParsedRole> parse(const Json& role_field) {
  const std::optional<std::string> raw = role_field.string_value();
  if (!raw.has_value()) {
    return std::nullopt;
  }
  return parse(*raw);
}

std::string key(const ParsedRole& parsed) {
  std::string out = parsed.chain + "." + parsed.link;
  if (parsed.axis.has_value()) {
    out += "." + *parsed.axis;
  }
  if (parsed.index.has_value()) {
    out += "#" + format_number(*parsed.index);
  }
  return out;
}

}  // namespace skynet::roles

namespace skynet::contracts {

Json roles(const std::string& /*label*/, const Json& input) {
  const std::optional<skynet::roles::ParsedRole> parsed = skynet::roles::parse(input["role"]);
  if (!parsed.has_value()) {
    return Json::null();  // The corpus shape: `null` for a role that did not parse.
  }
  Json out = Json::object();
  out.set("chain", Json::string(parsed->chain));
  out.set("link", Json::string(parsed->link));
  // `axis` and `index` are written as `null` rather than omitted: an absent axis is a fact the
  // consumer reads, not a field nobody filled in.
  out.set("axis", parsed->axis.has_value() ? Json::string(*parsed->axis) : Json::null());
  out.set("index", parsed->index.has_value() ? Json::number(*parsed->index) : Json::null());
  out.set("key", Json::string(skynet::roles::key(*parsed)));
  return out;
}

}  // namespace skynet::contracts

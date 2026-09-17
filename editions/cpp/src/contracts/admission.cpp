// §8.2's package admission — may this package be installed on this robot. C++ edition.
//
// Transcribed from `reference/typescript/package.ts` and `shapeAdmission` in
// `reference/typescript/shape.ts`. Every rule and message string is read from those files, never
// from `corpus/`.
//
// Pure, and shared: the Hub runs it to tell an operator what will happen before they deploy, and a
// robot runs it at install time, offline. Every check returns a finding rather than stopping, so an
// operator sees all the reasons at once — fixing four problems one deployment at a time is how
// people end up granting everything.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// The signature `switch` has no `default`
//     `verified` adds nothing, the other four named states add their finding, and a word the
//     reference's type does not contain falls through every case and adds nothing either. That is
//     reproduced rather than tightened here: tightening it is a specification change, and it
//     belongs in the reference and the corpus before it belongs in an edition.
//
// `subjects`, when a finding has none
//     `optional('subjects', …)` omits the key. A finding with no subjects has no `subjects` on the
//     wire, which is a different document from one with an empty list.
//
// `executes_on_load && !trust_remote_code`
//     Truthiness, `js::truthy`, not `=== true`.
//
// `budget.vram_mb !== undefined && budget.vram_mb > (capacity.vram_mb ?? 0)`
//     Presence, then a comparison in which NaN loses both ways. `js::bare_number` gives the NaN a
//     null in a case's input stands for, and C++'s `>` agrees with JavaScript's that NaN is neither
//     greater nor smaller than anything.
//
// `segmentValue`
//     The leading run of ASCII digits read with `Number()`, 0 when there is none — the reference's
//     fail-closed reading of a version segment that is not a number. See the reference for the
//     three readings and why two of them are wrong.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/text.hpp"
#include "skynet_os/vocabulary.hpp"

#include <algorithm>
#include <cstdlib>
#include <limits>
#include <optional>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

/// §49.1's audible warning channel, which no grant can confer: a policy that could claim it could
/// silence its own warning.
constexpr std::string_view kWarningChannel = "hri.warning";

struct Finding {
  std::string code;
  std::string severity;
  std::string message;
  std::optional<std::vector<std::string>> subjects;
};

std::vector<std::string> strings(const Json& list) {
  std::vector<std::string> out;
  for (const Json& item : list.items()) {
    out.push_back(js::interpolate(&item));
  }
  return out;
}

bool contains(const std::vector<std::string>& list, const std::string& item) {
  return std::find(list.begin(), list.end(), item) != list.end();
}

/// `RESERVED_CAPABILITIES.includes(token)`.
bool is_reserved(const std::string& token) { return token == kWarningChannel; }

// ── §85, the capability token vocabulary ─────────────────────────────────────

/// A domain's canonical capabilities. `sensing` is §5's modality list, read from this edition's one
/// copy of it rather than retyped — the reference imports it for the same reason.
std::vector<std::string> canonical_capabilities(const std::string& domain) {
  if (domain == "manipulation") return {"arm", "grasp", "tool", "motion"};
  if (domain == "locomotion") return {"drive", "walk", "fly"};
  if (domain == "sensing") {
    const vocabulary::Set& modalities = vocabulary::members("embodiment_modalities");
    return {modalities.begin(), modalities.end()};
  }
  if (domain == "hri") return {"speech", "light", "warning"};
  if (domain == "compute") return {"accelerator"};
  if (domain == "network") return {"outbound", "lan"};
  if (domain == "storage") return {"shared"};
  return {};
}

/// Dot-separated segments, each an ASCII lowercase letter then ASCII lowercase, digits or `_`.
/// Byte-wise: `<cctype>`'s classifiers depend on the locale, which this edition refuses by scan.
bool well_formed(const std::string& token) {
  const std::vector<std::string> segments = text::split(token, '.');
  if (segments.size() < 2) return false;
  for (const std::string& segment : segments) {
    if (segment.empty() || segment[0] < 'a' || segment[0] > 'z') return false;
    for (std::size_t at = 1; at < segment.size(); ++at) {
      const char c = segment[at];
      if (!((c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '_')) return false;
    }
  }
  return true;
}

/// §85.1's four classes. Exact: nothing trimmed, nothing folded, and a canonical domain admits
/// exactly two segments.
std::string classify_token(const std::string& token) {
  if (!well_formed(token)) return "malformed";
  const std::vector<std::string> segments = text::split(token, '.');
  if (!vocabulary::contains("capability_domains", segments[0])) return "vendor";
  if (segments.size() == 2 && contains(canonical_capabilities(segments[0]), segments[1])) {
    return "canonical";
  }
  return "unknown";
}

/// First-appearance order, each once: one problem named twice reads as two.
std::vector<std::string> distinct(const std::vector<std::string>& tokens) {
  std::vector<std::string> out;
  for (const std::string& token : tokens) {
    if (!contains(out, token)) out.push_back(token);
  }
  return out;
}

/// The value of one dotted segment: its leading run of ASCII digits, or 0.
double segment_value(const std::string& segment) {
  std::size_t digits = 0;
  while (digits < segment.size() && text::is_ascii_digit(segment[digits])) {
    ++digits;
  }
  return digits == 0 ? 0 : std::strtod(segment.substr(0, digits).c_str(), nullptr);
}

/// Numeric-segment comparison, sufficient for `major.minor.patch` OS versions.
int compare_versions(const std::string& left_text, const std::string& right_text) {
  std::vector<double> left;
  std::vector<double> right;
  for (const std::string& segment : text::split(left_text, '.')) left.push_back(segment_value(segment));
  for (const std::string& segment : text::split(right_text, '.')) right.push_back(segment_value(segment));
  for (std::size_t at = 0; at < std::max(left.size(), right.size()); ++at) {
    const double difference = (at < left.size() ? left[at] : 0) - (at < right.size() ? right[at] : 0);
    if (difference != 0) {
      return difference < 0 ? -1 : 1;
    }
  }
  return 0;
}

std::vector<Finding> check_budget(const Json& budget, const Json& capacity) {
  constexpr double kUndefined = std::numeric_limits<double>::quiet_NaN();
  std::vector<std::string> shortfalls;

  const double ram_needed = js::bare_number(budget, "ram_mb", kUndefined);
  const double ram_held = js::bare_number(capacity, "ram_mb", kUndefined);
  if (ram_needed > ram_held) {
    shortfalls.push_back("RAM: needs " + js::number_to_string(ram_needed) + " MB, has " +
                         js::number_to_string(ram_held) + " MB");
  }

  const auto optional_shortfall = [&](const char* field, const std::string& label,
                                      const std::string& unit, const std::string& held_unit) {
    if (!budget.has(field)) {
      return;
    }
    const double needed = js::bare_number(budget, field, kUndefined);
    const double held = js::bare_number(capacity, field, 0);
    if (needed > held) {
      shortfalls.push_back(label + ": needs " + js::number_to_string(needed) + unit + ", has " +
                           js::number_to_string(held) + held_unit);
    }
  };
  optional_shortfall("vram_mb", "VRAM", " MB", " MB");
  optional_shortfall("disk_mb", "Disk", " MB", " MB");
  optional_shortfall("cpu_cores", "CPU", " cores", "");

  if (shortfalls.empty()) {
    return {};
  }
  return {{"budget_exceeded", "error",
           "This robot cannot supply the resources the package declares it needs.", shortfalls}};
}

Json admit_package(const Json& params) {
  const Json& manifest = params["manifest"];
  const Json& capacity = params["capacity"];
  const Json& embodiment = manifest["embodiment"];
  std::vector<Finding> findings;

  // `failed` is its own code because it is the one state that says something went wrong rather
  // than something is missing.
  const std::string signature(params["signature"].string_or(""));
  if (signature == "failed") {
    findings.push_back({"signature_failed", "error",
                        "A release signature was present and did not verify. The bytes are not the "
                        "ones the signer attested to — treat this as tampering until somebody "
                        "proves otherwise.",
                        std::nullopt});
  } else if (signature == "untrusted") {
    findings.push_back({"signature_untrusted", "error",
                        "A release signature verified, and the key that made it is not one this "
                        "robot trusts. Nothing is wrong with the bytes; what is missing is the "
                        "operator's decision that this signer may publish here.",
                        std::nullopt});
  } else if (signature == "absent") {
    findings.push_back({"unsigned", "error",
                        "No publisher signature accompanied this package. The bytes may be intact "
                        "and nobody has attested to who produced them.",
                        std::nullopt});
  } else if (signature == "unverifiable") {
    findings.push_back({"signature_unverifiable", "error",
                        "This platform cannot verify a release signature, so nothing has "
                        "established who produced these bytes. That is a gap in the operating "
                        "system rather than in the package.",
                        std::nullopt});
  }

  if (params["observed_digest"].canonical() != manifest["digest"].canonical() ||
      params.has("observed_digest") != manifest.has("digest")) {
    findings.push_back({"digest_mismatch", "error",
                        "The package contents do not hash to the digest in its manifest.",
                        std::vector<std::string>{js::interpolate(&manifest["digest"]),
                                                 js::interpolate(&params["observed_digest"])}});
  }

  const std::string kind = js::interpolate(manifest.has("kind") ? &manifest["kind"] : nullptr);
  const bool is_driver = manifest["kind"].string_value() == std::optional<std::string>("driver");
  if (!is_driver && manifest["isolation"].string_value() == std::optional<std::string>("native")) {
    findings.push_back({"isolation_violation", "error",
                        "A " + kind +
                            " package may not run unsandboxed. Only drivers touch hardware directly.",
                        std::nullopt});
  }

  // §55.1: an environment describes a simulated world, and only a simulator driver can load one.
  // A warning — installing it is not dangerous — and never silence.
  if (manifest["kind"].string_value() == std::optional<std::string>("environment") &&
      !capacity["simulator"].is_true()) {
    findings.push_back({"environment_inert", "warning",
                        "An environment describes a simulated world, and no simulator driver is "
                        "loaded on this robot. It will install, and nothing will ever load it.",
                        std::nullopt});
  }

  for (Finding& finding : check_budget(manifest["budget"], capacity)) {
    findings.push_back(std::move(finding));
  }

  const std::vector<std::string> requests = strings(manifest["requests"]);
  const std::vector<std::string> declared = strings(embodiment["capabilities"]);
  const std::vector<std::string> declared_extensions = strings(embodiment["extensions"]);

  // A package must not request more than it declared it needs.
  std::vector<std::string> over_requested;
  for (const std::string& token : requests) {
    if (!contains(declared, token)) over_requested.push_back(token);
  }
  if (!over_requested.empty()) {
    findings.push_back({"over_request", "error",
                        "The package requests permissions it never declared a need for.",
                        over_requested});
  }

  // §85. A token that is not a capability can never match one, and is refused by name rather than
  // read as a robot that lacks it.
  std::vector<std::string> not_canonical;
  for (const std::string& token : declared) {
    if (classify_token(token) != "canonical") not_canonical.push_back(token);
  }
  for (const std::string& token : requests) {
    if (classify_token(token) != "canonical") not_canonical.push_back(token);
  }
  const std::vector<std::string> unknown = distinct(not_canonical);
  if (!unknown.empty()) {
    findings.push_back({"capability_unknown", "error",
                        "These are not capabilities this platform defines (§85). A token in a "
                        "canonical domain must be one §85 lists, so check the spelling; a vendor's "
                        "own token belongs under `embodiment.extensions`.",
                        unknown});
  }

  std::vector<std::string> not_vendor;
  for (const std::string& token : declared_extensions) {
    if (classify_token(token) != "vendor") not_vendor.push_back(token);
  }
  const std::vector<std::string> misfiled = distinct(not_vendor);
  if (!misfiled.empty()) {
    findings.push_back({"capability_unknown", "error",
                        "These extensions are not vendor tokens (§85). A token in a canonical "
                        "domain belongs under `embodiment.capabilities`, where it is checked; an "
                        "extension is dot-separated lowercase segments whose first is not a "
                        "canonical domain.",
                        misfiled});
  }

  // Two spellings of one fact, and admission enforces one of them (§8.2). A subset test in one
  // direction only: declaring more in `embodiment` than on the card is the Hub's to catch.
  std::vector<std::string> unenforced;
  for (const std::string& token : strings(manifest["required_capabilities"])) {
    if (!contains(declared, token)) unenforced.push_back(token);
  }
  for (const std::string& token : strings(manifest["required_extensions"])) {
    if (!contains(declared_extensions, token)) unenforced.push_back(token);
  }
  if (!unenforced.empty()) {
    findings.push_back({"embodiment_undeclared", "error",
                        "This package declares requirements its embodiment block does not, so "
                        "admission checked nothing for them. §8.2 enforces `embodiment.capabilities` "
                        "and `embodiment.extensions`.",
                        unenforced});
  }

  // §49.1, before the operator's denials: a reserved token is not something an operator declined,
  // it is something nobody can grant. De-duplicated — one problem, named once.
  std::vector<std::string> reserved;
  std::vector<std::string> asked = requests;
  asked.insert(asked.end(), declared.begin(), declared.end());
  for (const std::string& token : asked) {
    if (is_reserved(token) && !contains(reserved, token)) reserved.push_back(token);
  }
  if (!reserved.empty()) {
    findings.push_back({"capability_reserved", "error",
                        "The audible warning before autonomous motion is a system function and no "
                        "grant confers it. A package that could claim this channel could silence its "
                        "own warning, which removes a control the operator's risk assessment relied "
                        "on (§49.1).",
                        reserved});
  }

  const std::vector<std::string> denied = strings(capacity["denied"]);
  std::vector<std::string> explicitly_denied;
  for (const std::string& token : requests) {
    if (contains(denied, token)) explicitly_denied.push_back(token);
  }
  if (!explicitly_denied.empty()) {
    findings.push_back({"capability_denied", "error",
                        "The operator profile withholds capabilities this package requires.",
                        explicitly_denied});
  }

  std::vector<std::string> provided = strings(capacity["capabilities"]);
  for (const std::string& token : strings(capacity["extensions"])) provided.push_back(token);
  std::vector<std::string> required = declared;
  required.insert(required.end(), declared_extensions.begin(), declared_extensions.end());
  std::vector<std::string> unmet;
  for (const std::string& token : required) {
    if (!contains(provided, token)) unmet.push_back(token);
  }
  if (!unmet.empty()) {
    findings.push_back({"capability_ungranted", "error",
                        "This robot does not provide capabilities the package requires.", unmet});
  }

  const std::vector<std::string> granted = strings(capacity["granted"]);
  std::vector<std::string> ungranted;
  for (const std::string& token : requests) {
    if (contains(provided, token) && !contains(granted, token) && !contains(denied, token)) {
      ungranted.push_back(token);
    }
  }
  if (!ungranted.empty()) {
    // A warning: the Android runtime-permission moment. The install proceeds and the operator is
    // prompted; the mode gate enforces the missing grant at every cycle.
    findings.push_back({"capability_ungranted", "warning",
                        "These capabilities are awaiting an operator grant. The package installs "
                        "and cannot use them until it is given.",
                        ungranted});
  }

  std::vector<std::string> installed;
  for (const Json& entry : capacity["installed"].items()) {
    installed.push_back(entry["id"].canonical());
  }
  for (const Json& dependency : manifest["depends_on"].items()) {
    if (!contains(installed, dependency["id"].canonical())) {
      const std::string id = js::interpolate(dependency.has("id") ? &dependency["id"] : nullptr);
      findings.push_back(
          {"missing_dependency", "error",
           "Requires " + id + " " +
               js::interpolate(dependency.has("range") ? &dependency["range"] : nullptr) +
               ", which is not installed.",
           std::vector<std::string>{id}});
    }
  }

  const Json& minimum = manifest["min_os_version"];
  if (js::truthy(minimum)) {
    const std::string running = js::interpolate(&capacity["os_version"]);
    if (compare_versions(running, js::interpolate(&minimum)) < 0) {
      findings.push_back({"os_too_old", "error",
                          "Requires Skynet OS " + js::interpolate(&minimum) +
                              " or later; this robot runs " + running + ".",
                          std::nullopt});
    }
  }

  if (params["transport_verdict"].string_value() == std::optional<std::string>("refused")) {
    findings.push_back({"transport_refused", "error",
                        "The policy's action space cannot be transported onto this body.",
                        std::nullopt});
  }

  if (js::truthy(params["executes_on_load"]) && !js::truthy(params["trust_remote_code"])) {
    findings.push_back({"executes_on_load_untrusted", "error",
                        "This package contains files that execute code when read, and the operator "
                        "has not accepted that. Install with trust_remote_code once you have decided "
                        "the publisher is trustworthy.",
                        std::nullopt});
  }

  const bool admitted = std::none_of(findings.begin(), findings.end(),
                                     [](const Finding& finding) { return finding.severity == "error"; });

  Json effective = Json::array();
  if (admitted) {
    for (const std::string& token : requests) {
      if (contains(provided, token) && contains(granted, token)) {
        effective.push(Json::string(token));
      }
    }
  }

  // `shapeAdmission`.
  Json findings_json = Json::array();
  for (const Finding& finding : findings) {
    Json row = Json::object();
    row.set("code", Json::string(finding.code));
    row.set("severity", Json::string(finding.severity));
    row.set("message", Json::string(finding.message));
    if (finding.subjects.has_value()) {
      Json subjects = Json::array();
      for (const std::string& subject : *finding.subjects) {
        subjects.push(Json::string(subject));
      }
      row.set("subjects", subjects);
    }
    findings_json.push(row);
  }

  Json out = Json::object();
  out.set("admitted", Json::boolean(admitted));
  out.set("findings", findings_json);
  out.set("effective_grants", effective);
  return out;
}

}  // namespace

Json admission(const std::string& label, const Json& input) {
  if (label == "install" || label == "vocabulary") {
    return admit_package(input);
  }
  if (label == "tokens") {
    Json out = Json::object();
    out.set("class", Json::string(classify_token(std::string(input["token"].string_or("")))));
    return out;
  }
  throw Unimplemented("admission", label);
}

}  // namespace skynet::contracts

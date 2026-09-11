// §50's birth certificate, C++ edition.
//
// Transcribed from `reference/typescript/birth.ts`. Every finding, sentence and ordering decision is
// read from that file, never from `corpus/`.
//
// §50.2 keeps two facts apart, and this contract exists to hold the line between them: **the
// certificate proves what this robot is, and pairing proves whose it is.** A vendor who put a
// namespace in a birth certificate would be asserting a fact they cannot know — so the field is
// read rather than discarded at parse, and reported, because silently ignoring it means the next
// vendor adds it too.
//
// Every observed field is optional because a robot early in boot has seen some and not others, and
// §7.2's rule applies here as everywhere: **an absent input makes a check unavailable, never
// passed.** An absent value never satisfies a comparison.
//
// ── The two findings that void a certificate, and the many that do not ─────
//
// `attests` is false only for `unattested` and a mismatched device key. The others *narrow* what is
// attested; those two remove it. And note what `attests` is **not**: it is not "may the robot run".
// §50 establishes identity and does not gate motion, and a robot whose certificate is void is one
// with an unattested identity — the state every robot was in before §50 existed.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `const [first = '', second = ''] = finding.subjects`
//     Array destructuring with defaults: a missing element becomes `''`, and so does an element
//     that is explicitly `undefined`. Indexing a short list would be a crash in the function whose
//     whole job is to explain a malformed finding.
//
// `observed === null || observed === undefined`
//     Both, and only those. An observed value of `''` is a reading, not an absence — so an empty
//     string is *compared* and mismatches, rather than being reported unchecked.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"

#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

struct Finding {
  std::string code;
  std::vector<std::string> subjects;
};

/// `const [a = '', b = ''] = subjects` — a missing element is the empty string, not an error.
std::string at(const std::vector<std::string>& subjects, std::size_t index) {
  return index < subjects.size() ? subjects[index] : "";
}

/// Whether this finding means the certificate establishes nothing at all.
bool voids_the_certificate(const Finding& finding) {
  return finding.code == "unattested" || finding.code == "device_key_mismatch";
}

/// The sentence an operator gets, naming the remedy. Ordered by how much each costs, and each names
/// its own remedy because the remedies are genuinely different: a re-issue, a resync, a trust-store
/// change, or a call.
std::string explain(const Finding& finding) {
  const std::string first = at(finding.subjects, 0);
  const std::string second = at(finding.subjects, 1);

  if (finding.code == "unattested") {
    return "The birth certificate's signature is " + first +
           ", so it attests nothing: everything it says about this machine is currently a claim the "
           "machine makes about itself, which is the situation a birth certificate exists to end. "
           "Register the vendor's key with this robot's trust store.";
  }
  if (finding.code == "device_key_mismatch") {
    return "This certificate was issued over device key " + first +
           " and the secure element holds " + second +
           ". The document is about a different machine. If the mainboard was replaced this is the "
           "expected result and not a fault — §50.4 — and the remedy is a vendor-signed re-issue, "
           "which also invalidates the calibrations a board swap could have disturbed.";
  }
  if (finding.code == "serial_mismatch") {
    return "The certificate names serial \"" + first + "\" and this machine reports \"" + second +
           "\". Ask the vendor to re-issue.";
  }
  if (finding.code == "descriptor_mismatch") {
    return "The certificate attests descriptor " + first + " and this robot is running " + second +
           ". The joint ratings in force are ones nobody has signed for this machine. A vendor who "
           "has published an updated descriptor should re-issue the certificate over it; until "
           "then the envelope rests on the descriptor's own signature rather than on the vendor's "
           "attestation that it belongs to this body.";
  }
  if (finding.code == "hardware_revision_mismatch") {
    return "The certificate names hardware revision \"" + first + "\" and this machine reports \"" +
           second + "\". Either the machine was modified or the certificate is stale; both are a "
                    "vendor re-issue.";
  }
  if (finding.code == "ownership_asserted") {
    std::string joined;
    for (std::size_t index = 0; index < finding.subjects.size(); ++index) {
      if (index > 0) {
        joined += ", ";
      }
      joined += finding.subjects[index];
    }
    return "This certificate asserts ownership (" + joined +
           "), and ownership is not the vendor's to assert. §50.2 keeps two facts apart: the "
           "certificate proves what this robot is, and pairing proves whose it is. The assertion is "
           "ignored.";
  }
  if (finding.code == "unobservable") {
    return "The certificate binds " + first +
           " and this robot could not read its own. The binding is neither confirmed nor "
           "contradicted — it is unchecked, which is not the same as matching.";
  }
  if (finding.code == "incomplete") {
    return "The certificate does not bind " + first +
           ", which §50.2 requires it to. It is not a complete birth certificate and whatever it "
           "does bind is all it attests.";
  }
  throw JsonError("unknown finding " + finding.code);
}

void compare(const Json& certificate, const char* certificate_key, const Json& observed,
             const char* observed_key, const char* field, const char* mismatch,
             std::vector<Finding>& findings) {
  const std::string declared(certificate[certificate_key].string_or(""));
  if (declared.empty()) {
    findings.push_back({"incomplete", {field}});
    return;
  }
  // Null and undefined, and only those: an observed `''` is a reading rather than an absence, so it
  // is compared and mismatches instead of being reported unchecked.
  if (!observed.has(observed_key) || observed[observed_key].is_null()) {
    findings.push_back({"unobservable", {field}});
    return;
  }
  const std::string seen(observed[observed_key].string_or(""));
  if (seen == declared) {
    return;
  }
  findings.push_back({mismatch, {declared, seen}});
}

}  // namespace

Json birth(const std::string& /*label*/, const Json& input) {
  // The order of the findings is the order of their cost, and the first is deliberately about the
  // *document* rather than the machine: comparing fields against a certificate nobody signed is
  // comparing the robot to a story it told itself.
  const Json& certificate = input["certificate"];
  const Json& observed = input["observed"];

  std::vector<Finding> findings;

  if (certificate["signature"].string_or("") != "verified") {
    findings.push_back({"unattested", {js::interpolate(&certificate["signature"])}});
  }

  compare(certificate, "device_key", observed, "device_key", "the device key", "device_key_mismatch",
          findings);
  compare(certificate, "serial", observed, "serial", "a serial number", "serial_mismatch", findings);
  compare(certificate, "descriptor_digest", observed, "descriptor_digest",
          "an embodiment descriptor digest", "descriptor_mismatch", findings);
  compare(certificate, "hardware_revision", observed, "hardware_revision", "a hardware revision",
          "hardware_revision_mismatch", findings);

  if (certificate["model"].string_or("").empty()) {
    // Bound but not observable: a model is what the vendor calls this design, and there is nothing
    // on the machine to compare it against. Its absence is still incomplete.
    findings.push_back({"incomplete", {"a model"}});
  }

  const JsonArray& ownership = certificate["claims_ownership"].items();
  if (!ownership.empty()) {
    Finding asserted{"ownership_asserted", {}};
    for (const Json& claim : ownership) {
      asserted.subjects.emplace_back(claim.string_or(""));
    }
    findings.push_back(std::move(asserted));
  }

  bool attests = true;
  Json unchecked = Json::array();
  Json rows = Json::array();
  for (const Finding& finding : findings) {
    if (voids_the_certificate(finding)) {
      attests = false;
    }
    if (finding.code == "unobservable") {
      unchecked.push(Json::string(at(finding.subjects, 0)));
    }
    Json row = Json::object();
    row.set("code", Json::string(finding.code));
    row.set("message", Json::string(explain(finding)));
    Json subjects = Json::array();
    for (const std::string& subject : finding.subjects) {
      subjects.push(Json::string(subject));
    }
    row.set("subjects", subjects);
    rows.push(row);
  }

  Json out = Json::object();
  out.set("attests", Json::boolean(attests));
  out.set("findings", rows);
  out.set("fully_confirmed", Json::boolean(findings.empty()));
  out.set("unchecked", unchecked);
  return out;
}

}  // namespace skynet::contracts

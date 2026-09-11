// §10, §19 and §50's identity, C++ edition.
//
// Transcribed from `reference/typescript/identity.ts`. Every rule, code and message string is read
// from that file, never from `corpus/`.
//
// Three questions in one contract: what somebody typed when they meant a pairing code, what state
// this robot's credential is in, and what it is actually running.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `String.prototype.length`, and why this edition does not reproduce it
//     It counts **UTF-16 code units**, and code points disagree with it for every character above
//     U+FFFF. Eight astral characters used to be a code of length **16** to the reference — refused
//     as `too_long`, a sentence that is simply false to somebody who typed eight — and of length 8
//     to the Rust edition, which refused them one rule later as `not_in_alphabet` at position 1.
//     Two faults, two sentences, one typo. §80 recorded it as the rare divergence where the
//     *reference* was the edition that should change, and on 2026-09-05 it did. Code points on all
//     sides, so there is nothing left here to reproduce.
//
// `Array.prototype.sort`
//     Orders by UTF-16 code unit, which is neither byte order nor code-point order. The
//     unrecognised-scope list is sorted that way.
//
// `value === true`
//     `credential`, `birth_certificate` and `binding` are all tested strictly, and the last one is
//     the important one: *absent is "nobody looked"*. A robot that has not been asked whether it
//     has a secure element has not established that it lacks one, and §10's
//     `security.identity.attested` is a measurement rather than an assumption.
//
// `String.prototype.trim`
//     Strips a specific set the ECMAScript specification names, and leaves the separators
//     U+001C–U+001F and U+0085 in place. A namespace of "\x1c" is non-empty to the reference.
//
// `toUpperCase`, `toLowerCase`
//     Full Unicode mappings, which can land inside an ASCII alphabet: `ß` is `SS` and U+212A is
//     `k`. `js::to_upper` and `js::to_lower`, never an ASCII shortcut — see the normaliser below
//     for what the shortcut cost.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/identity.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/text.hpp"

#include <algorithm>
#include <string>
#include <vector>

// File-local, and opened at `skynet` scope rather than inside `skynet::identity`: both halves of this
// file read these — the shared attestation and the contract after it — and an unnamed namespace is
// scoped to the namespace it is opened in, not to the file. The first version opened it one level
// down and the contract could see none of it.
namespace skynet {
namespace {

/// Crockford's alphabet: no I, L, O or U. The excluded letters are not rejected on input — they are
/// **folded**, because an operator who types the letter they saw should get the code they saw.
constexpr std::string_view kPairingAlphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Eight. Forty bits, which is a large number of guesses against a code that lives for minutes
/// behind a Hub that rate-limits, and a small number of characters for somebody reading them off a
/// screen — which is the trade the length is actually making.
constexpr std::size_t kPairingCodeLength = 8;

/// How a pairing code is written for a person to read: two groups of four.
constexpr std::size_t kPairingCodeGroup = 4;

/// The prefix on a Hub device credential (§10), here so a credential pasted into `skynet pair` is
/// recognised as one and refused by name.
constexpr std::string_view kDeviceTokenPrefix = "skd_";

/// §17's and §57's device-facing routes, one scope each. **The direction is §19's and the
/// vocabulary is not**: this is an allow-list because a deny-list of §19's three phrases would be
/// got past by a credential that spelled repository read any other way, and the one thing §19 says
/// about this credential is that it is *assumed lost*.
bool is_device_scope(std::string_view scope) {
  return scope == "device:calibration" || scope == "device:checkin" ||
         scope == "device:conformance" || scope == "device:desired" || scope == "device:episodes" ||
         scope == "device:evaluations" || scope == "device:incidents" || scope == "device:releases";
}

/// Crockford's folding: the letters excluded from the alphabet map to the digits they resemble.
char32_t fold(char32_t character) {
  if (character == U'O') return U'0';
  if (character == U'I' || character == U'L') return U'1';
  return character;
}

std::string trimmed(const Json& value) {
  return value.is_string() ? js::trim(value.string_or("")) : "";
}

identity::Finding finding(std::string code, std::string severity, std::string message,
                          std::vector<std::string> subjects = {}) {
  return identity::Finding{std::move(code), std::move(severity), std::move(message),
                           std::move(subjects)};
}

/// One clause of §10's sentence, **including its own label**.
///
/// The label belongs to the clause rather than to the template, and that is not a stylistic choice:
/// a template reading `running image ${…}` produces "running image `sha256:aa`" for a measured image
/// and "running image an unmeasured OS image" for one that is not. The first version did exactly
/// that, and it was found by opening the console rather than by any assertion — the bytes were
/// correct, the shape was correct, and the sentence was unreadable.
std::string render(const Json& measure, const std::string& label, const std::string& noun) {
  const std::string state(measure["state"].string_or(""));
  if (state == "known") {
    const std::string digest = trimmed(measure["digest"]);
    // A `known` measure with nothing in it is a caller bug, rendered as the absence it is rather
    // than as an empty pair of backticks somebody reads as a digest of zero length.
    return digest.empty() ? "an unnamed " + noun : label + "`" + digest + "`";
  }
  if (state == "nothing") {
    return "no " + noun;
  }
  const std::string why = trimmed(measure["why"]);
  return why.empty() ? "an unmeasured " + noun : "an unmeasured " + noun + " (" + why + ")";
}

}  // namespace
}  // namespace skynet

namespace skynet::identity {

Attestation attest(const Json& attestation) {
  // §10's sentence, which **cannot fail**.
  //
  // §13's rule for a recording with a hole in it — seal it, and say it is incomplete — is the same
  // rule here, because the reason is the same: the report most likely to have a hole in it is the
  // one covering whatever made the hole. A check-in withheld because a digest could not be read is
  // a robot the fleet believes is *offline*, at the exact moment somebody needs to know what it is
  // running, and it converts a named gap into an indistinguishable silence.
  Attestation out;
  for (const char* slot : {"os_image", "policy", "embodiment"}) {
    if (attestation[slot]["state"].string_or("") == "unmeasured") {
      out.unmeasured.emplace_back(slot);
    }
  }
  std::sort(out.unmeasured.begin(), out.unmeasured.end(),
            [](const std::string& left, const std::string& right) {
              return js::utf16_less(left, right);
            });

  const std::string device = trimmed(attestation["device_id"]);
  const std::string subject = device.empty() ? "This robot" : device;

  out.complete = out.unmeasured.empty();
  out.sentence = subject + " is running " + render(attestation["os_image"], "image ", "OS image") +
                 ", " + render(attestation["policy"], "policy ", "policy") + ", against " +
                 render(attestation["embodiment"], "body ", "body") + ".";
  return out;
}

Json measure_block(const Json& measure) {
  // **The digest is gated on the state**, which is the line worth reading twice. A measure that says
  // `unmeasured` and carries a digest is a contradiction — the state says nobody read the slot and
  // the field carries a reading of it — and §32.1 puts a fleet column on exactly this question:
  // *which robots are running image `sha256:aa`*. Passing the digest through would answer that
  // query with a robot whose image was never measured, which is a false positive on the one query an
  // incident is run through. So a digest survives only on a `known` measure.
  std::string state(measure["state"].string_or(""));
  if (state != "known" && state != "nothing" && state != "unmeasured") {
    // An unrecognised word is `unmeasured`, never `known`. Every default in this edition points the
    // same way: a document that did not say has not established that a slot was read.
    state = "unmeasured";
  }
  const std::string digest = trimmed(measure["digest"]);
  const std::string why = trimmed(measure["why"]);

  Json out = Json::object();
  out.set("digest", state == "known" && !digest.empty() ? Json::string(digest) : Json::null());
  out.set("state", Json::string(state));
  out.set("why", why.empty() ? Json::null() : Json::string(why));
  return out;
}

Json attestation_block(const Json& attestation) {
  // A sentence is not a record. §10 asks the Hub to record *"robot X is running image `sha256:…`,
  // policy `sha256:…`, against body `sha256:…`"*, and `attest` produces exactly that — but ask a
  // fleet *"which robots are running image `sha256:aa`"*, the question §32.1 puts a column on, and
  // the only way to answer it from the sentence is to parse English back out of prose that is
  // deliberately not machine-readable.
  //
  // So both go on the wire. The sentence, because it is what somebody reads at 3 a.m.; the measures,
  // because they are what somebody queries at 3 a.m.
  const Attestation report = attest(attestation);
  Json unmeasured = Json::array();
  for (const std::string& name : report.unmeasured) {
    unmeasured.push(Json::string(name));
  }

  Json out = Json::object();
  out.set("complete", Json::boolean(report.complete));
  out.set("sentence", Json::string(report.sentence));
  out.set("unmeasured", unmeasured);
  out.set("embodiment", measure_block(attestation["embodiment"]));
  out.set("os_image", measure_block(attestation["os_image"]));
  out.set("policy", measure_block(attestation["policy"]));
  return out;
}

}  // namespace skynet::identity

namespace skynet::contracts {
namespace {

using identity::Finding;

Json findings_json(const std::vector<Finding>& findings) {
  Json out = Json::array();
  for (const Finding& one : findings) {
    Json row = Json::object();
    row.set("code", Json::string(one.code));
    row.set("message", Json::string(one.message));
    row.set("severity", Json::string(one.severity));
    Json subjects = Json::array();
    for (const std::string& subject : one.subjects) {
      subjects.push(Json::string(subject));
    }
    row.set("subjects", subjects);
    out.push(row);
  }
  return out;
}

}  // namespace

Json identity(const std::string& label, const Json& input) {
  if (label.find("pairing") != std::string::npos) {
    // The order of the checks is the order of usefulness, and it is not the order they would fall
    // out of a parser. A credential pasted here is caught **before** anything is stripped, because
    // `skd_` contains an underscore and would otherwise be reported as an unusable character at
    // position four — true, useless, and it would have the operator hunting for a typo in a string
    // that has none. Length is checked before the alphabet for the same reason: somebody who typed
    // seven characters needs to be told they typed seven.
    const std::string typed(input["input"].string_or(""));

    const auto refuse = [](std::string code, std::string detail, Json position) {
      Json fault = Json::object();
      fault.set("code", Json::string(std::move(code)));
      fault.set("detail", Json::string(std::move(detail)));
      fault.set("position", std::move(position));
      Json out = Json::object();
      out.set("code", Json::null());
      out.set("fault", fault);
      return out;
    };

    // `trim()` and `toLowerCase()`, both the reference's: a byte-order mark in front of a pasted
    // credential is trimmed away, and U+212A KELVIN SIGN lower-cases to the `k` in `skd_`.
    if (text::starts_with(js::to_lower(js::trim(typed)), kDeviceTokenPrefix)) {
      return refuse("looks_like_a_credential",
                    "This is a device credential — it begins `" + std::string(kDeviceTokenPrefix) +
                        "` — and pairing takes the short code the Hub showed you, not the "
                        "credential it hands back afterwards. Nothing has been sent. Treat the "
                        "credential as exposed if it has been on a screen somebody else could see.",
                    Json::null());
    }

    // Two loops, both over code points — the reference's shape, for its reason. `toUpperCase()` on
    // one code point can return several, and `fold` answers a question about each character of the
    // result rather than about the group: the `fi` ligature upper-cases to `FI`, and folding each
    // character gives `F1`, both in the alphabet.
    //
    // The first version of this upper-cased ASCII only, with a comment explaining why that was
    // exact: every character the alphabet admits is ASCII, so nothing else could matter. The corpus
    // disagreed on its first run — `ß` upper-cases to `SS`, two ASCII letters that are both in the
    // alphabet, so `ß234567` is the eight-character code `SS234567` and was refused here as seven.
    // A full mapping can land *inside* an ASCII alphabet, and that was the whole case.
    std::u32string normalized;
    for (const char32_t character : js::code_points(typed)) {
      if (character == U'-' || character == U' ' || character == U'\t' || character == U'\n' ||
          character == U'\r') {
        continue;
      }
      for (const char32_t upper : js::to_upper(character)) {
        normalized.push_back(fold(upper));
      }
    }

    if (normalized.empty()) {
      return refuse("empty",
                    "No code was given. `skynet pair` takes the eight-character code the Hub showed "
                    "you, written as " + std::string(kPairingCodeGroup, 'X') + "-" +
                        std::string(kPairingCodeGroup, 'X') + ".",
                    Json::null());
    }

    // Counted in code points, not bytes and not UTF-16 units — see the header.
    if (normalized.size() != kPairingCodeLength) {
      return refuse(normalized.size() < kPairingCodeLength ? "too_short" : "too_long",
                    "A pairing code is " + std::to_string(kPairingCodeLength) +
                        " characters and this is " + std::to_string(normalized.size()) +
                        ". Hyphens and spaces do not count towards it.",
                    Json::null());
    }

    std::string code;
    for (std::size_t index = 0; index < normalized.size(); ++index) {
      const char32_t character = normalized[index];
      if (character > 0x7F ||
          kPairingAlphabet.find(static_cast<char>(character)) == std::string_view::npos) {
        return refuse("not_in_alphabet",
                      "Character " + std::to_string(index + 1) + " of the " +
                          std::to_string(kPairingCodeLength) +
                          " is not one a pairing code uses. Codes are digits and upper-case "
                          "letters, without I, L, O or U — if you read one of those off the screen "
                          "it was a 1 or a 0, and typing it that way works too.",
                      Json::number(static_cast<double>(index + 1)));
      }
      code.push_back(static_cast<char>(character));
    }

    Json out = Json::object();
    out.set("code", Json::string(code));
    out.set("fault", Json::null());
    return out;
  }

  if (label.find("attestation") != std::string::npos) {
    const identity::Attestation report = identity::attest(input);
    Json unmeasured = Json::array();
    for (const std::string& name : report.unmeasured) {
      unmeasured.push(Json::string(name));
    }
    Json out = Json::object();
    out.set("complete", Json::boolean(report.complete));
    out.set("sentence", Json::string(report.sentence));
    out.set("unmeasured", unmeasured);
    return out;
  }

  if (label.find("credential") == std::string::npos) {
    throw Unimplemented("identity", label);
  }

  // What state this robot's identity is in, and everything wrong with it.
  //
  // Every check runs and the result carries all of them: an operator standing in front of a robot
  // fixing one problem per attempt is the failure a finding list exists to prevent, and pairing is a
  // job somebody drove to a warehouse to do.
  //
  // **Not paired is not a fault.** §50.2 ships a robot with a birth certificate and no namespace, so
  // an unpaired robot is at the correct point in its lifecycle and is reported as a warning naming
  // the command rather than as something broken.
  const Json& identity_doc = input["identity"];
  const Json& observed = input["observed"];

  std::vector<Finding> findings;
  const bool credential = identity_doc["credential"].is_true();
  const std::string namespace_name = trimmed(identity_doc["namespace"]);

  if (!credential) {
    findings.push_back(finding(
        "not_paired", "warning",
        "This robot holds no device credential, so it belongs to nobody and reports to nothing. "
        "That is the state a robot ships in (§50.2). Pair it with `skynet pair` and the code the "
        "Hub showed you."));
  } else {
    if (namespace_name.empty()) {
      findings.push_back(finding(
          "namespace_missing", "error",
          "This robot holds a credential and names no namespace. §10 pairs a robot to a namespace, "
          "so a credential without one has bound it to nothing — and every report it sends is "
          "addressed to a fleet nobody can name."));
    }

    const Json& prefix_field = identity_doc["credential_prefix"];
    const std::string prefix = prefix_field.is_null() ? "" : std::string(prefix_field.string_or(""));
    if (prefix != kDeviceTokenPrefix) {
      const std::string shown =
          prefix.empty() ? "with nothing recognisable" : "`" + prefix + "`";
      findings.push_back(finding(
          "not_a_device_credential", "error",
          "A device credential begins `" + std::string(kDeviceTokenPrefix) + "` and this one begins " +
              shown +
              ". §10 puts the narrowest credential class in the system on the robot on purpose, "
              "because a credential physically present in the field is the easiest one to lose. A "
              "wider one here is a wider one lost."));
    }

    const Json& scopes = identity_doc["scopes"];
    if (scopes.is_null()) {
      findings.push_back(finding(
          "scope_undeclared", "warning",
          "This credential declares no scopes, so nothing on the robot can check that it is as "
          "narrow as §19 requires. That is the ordinary shape of an opaque bearer token and is not "
          "a fault — it means `security.credential.scope` is answered on the Hub and not here."));
    } else {
      std::vector<std::string> unrecognised;
      for (const Json& scope : scopes.items()) {
        const std::string name(scope.string_or(""));
        if (!is_device_scope(name) &&
            std::find(unrecognised.begin(), unrecognised.end(), name) == unrecognised.end()) {
          unrecognised.push_back(name);
        }
      }
      std::sort(unrecognised.begin(), unrecognised.end(),
                [](const std::string& left, const std::string& right) {
                  return js::utf16_less(left, right);
                });
      if (!unrecognised.empty()) {
        std::string joined;
        for (std::size_t at = 0; at < unrecognised.size(); ++at) {
          if (at > 0) {
            joined += ", ";
          }
          joined += unrecognised[at];
        }
        findings.push_back(finding(
            "scope_unrecognised", "error",
            "This credential carries " + joined +
                ", which is not a capability a device credential is recognised to need. §19 assumes "
                "this credential is lost, and a capability nobody has established is narrow, on a "
                "credential everybody assumes is stolen, is refused rather than allowed.",
            unrecognised));
      }
    }
  }

  // ── §10's second addition: what the key is bound to ──────────────────────
  const std::string key = trimmed(identity_doc["device_key"]);
  const Json& binding = identity_doc["binding"];
  const std::string birth_key = trimmed(observed["birth_key"]);

  if (binding.is_null()) {
    findings.push_back(finding(
        "key_binding_unknown", "warning",
        "Nothing has established whether this key lives in a secure element. "
        "`security.identity.attested` is a measurement, so an unmeasured robot is reported "
        "unmeasured rather than counted as either answer."));
  } else if (binding.string_or("") != "hardware") {
    findings.push_back(finding(
        "key_not_hardware_bound", "warning",
        "This device key is a file rather than a secret inside a secure element, so it leaves with "
        "a copy of the filesystem and identifies whatever machine that copy is put on. That is the "
        "ordinary state at guest tier and it is the ceiling §10 puts on "
        "`security.identity.attested`."));
  }

  const bool has_birth_certificate = observed["birth_certificate"].is_true();
  if (!has_birth_certificate) {
    findings.push_back(finding(
        "no_birth_certificate", "warning",
        "This robot holds no trusted birth certificate, so there is nothing to check the device key "
        "against. §50.3: without one, what this machine is remains a claim it makes about itself."));
  } else if (!birth_key.empty() && !key.empty() && birth_key != key) {
    findings.push_back(finding(
        "key_not_attested", "error",
        "The key this robot was paired on is not the key its vendor attested at manufacture. §50.2 "
        "separates what it is from whose it is, and this is the seam between them coming apart: "
        "either the mainboard was replaced — §50.4, expected, and the remedy is a vendor-signed "
        "re-issue — or this robot was paired on a key nobody signed.",
        {birth_key}));
  }

  // Both halves. A hardware key nobody attested is a key the robot generated for itself, and an
  // attested key sitting in a file is an attestation that copies to any machine somebody puts the
  // file on.
  const bool attested = !binding.is_null() && binding.string_or("") == "hardware" &&
                        has_birth_certificate && !birth_key.empty() && !key.empty() &&
                        birth_key == key;

  Json out = Json::object();
  out.set("attested", Json::boolean(attested));
  out.set("findings", findings_json(findings));
  out.set("paired", Json::boolean(credential && !namespace_name.empty()));
  return out;
}

}  // namespace skynet::contracts

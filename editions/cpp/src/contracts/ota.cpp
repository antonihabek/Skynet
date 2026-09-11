// §12's three rules, as two decisions — C++ edition.
//
// Transcribed from `reference/typescript/ota.ts`. Every rule and message string is read from that
// file, never from `corpus/`.
//
// Rule 1 — never apply mid-task — is not here, for the reference's reason: the reconciler already
// enforces it structurally. Rule 3 is `admit_bundle`, and it is the one §12 calls the single most
// important test in the suite: an update that changes the Safety Kernel or the envelope is refused
// without a fresh bench pass **for these exact bytes**. Rule 2 is `confirm_boot`.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// An unrecognised component is gated
//     The whole anti-gaming argument in one line of `gated`: a publisher shipping a kernel change
//     past the gate would write `core` or `rt`, not `safety_kernel`. A matcher that knew two words
//     would wave it through.
//
// `bundle.digest ?? ''`, and where it is trimmed
//     Only the `undigested` check trims. The bench comparison and the sentence use the digest as
//     it arrived, and `digest === ''` beside the comparison is the raw emptiness — so a digest of
//     whitespace is refused as undigested and then also compared, byte for byte, to the pass.
//
// `[...new Set(...)].sort()`
//     De-duplicated, then sorted by UTF-16 code unit — `js::utf16_less`, not byte order.
//
// `evidence.elapsed_ms ?? 0`
//     `elapsed_ms` is a bare `number` in the reference, so a null in a case's input is how the
//     corpus spells NaN (corpus/README.md) — and `NaN ?? 0` is NaN, which renders as `NaN ms` and
//     is never an expired budget. Only an absent field takes the default.
//
// template literals
//     Every millisecond count goes through `js::number_to_string`.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"

#include <algorithm>
#include <array>
#include <cmath>
#include <string>
#include <string_view>
#include <vector>

namespace skynet::contracts {
namespace {

/// The components an OS bundle may declare that it changes — the stack §3 draws and nothing more.
constexpr std::array<std::string_view, 9> kOsComponents = {
    "bootloader", "kernel_image", "rhal_abi",       "rhal_driver", "safety_kernel",
    "envelope",   "skynetd",      "policy_runtime", "userspace",
};

/// The components §12's third rule gates by name. **Not the whole rule** — `gated` is.
constexpr std::array<std::string_view, 2> kCertificationGated = {"safety_kernel", "envelope"};

/// §12's default boot budget: ten minutes, a default an operator may narrow.
constexpr double kDefaultBootBudgetMs = 600000;

template <std::size_t N>
bool listed(const std::array<std::string_view, N>& list, std::string_view word) {
  return std::find(list.begin(), list.end(), word) != list.end();
}

/// Whether changing this component requires a fresh bench pass: §12's two names, and anything this
/// edition does not recognise.
bool gated(std::string_view component) {
  if (listed(kCertificationGated, component)) {
    return true;
  }
  return !listed(kOsComponents, component);
}

struct Fault {
  std::string code;
  std::string severity;
  std::string message;
  std::vector<std::string> subjects;
};

/// An array of strings out of a document, `?? []` for anything absent.
std::vector<std::string> strings(const Json& list) {
  std::vector<std::string> out;
  for (const Json& item : list.items()) {
    out.push_back(js::interpolate(&item));
  }
  return out;
}

/// `Array.prototype.join(', ')`.
std::string joined(const std::vector<std::string>& items) {
  std::string out;
  for (std::size_t at = 0; at < items.size(); ++at) {
    if (at > 0) {
      out += ", ";
    }
    out += items[at];
  }
  return out;
}

/// `value ?? ''` for a field the reference types as a string.
std::string text_or_empty(const Json& value) {
  return value.is_null() ? "" : js::interpolate(&value);
}

std::string certification_message(const std::vector<std::string>& gated_components,
                                  const std::vector<std::string>& unrecognised) {
  std::string message = "This update changes " + joined(gated_components) +
                        ", and §12 refuses one that changes the safety kernel or the envelope "
                        "without a fresh bench pass for these exact bytes.";
  if (!unrecognised.empty()) {
    message += " " + joined(unrecognised) +
               " is not a component this edition recognises, and an unrecognised component is "
               "gated for the same reason the two named ones are: nobody has established what it "
               "changes.";
  }
  message += " This robot holds no bench pass at all.";
  return message;
}

/// `displayDigest` — the empty string reads as `nothing`.
std::string display_digest(const std::string& digest) { return digest.empty() ? "nothing" : digest; }

/// The same, for the bench's `image_digest`, which the reference reads with no `?? ''`: an absent
/// one reaches the template as `undefined`, and that is what it says.
std::string display_digest(const Json* digest) {
  if (digest != nullptr && digest->is_string()) {
    return display_digest(std::string(digest->string_or("")));
  }
  return js::interpolate(digest);
}

Json admit_bundle(const Json& bundle, const Json& subject) {
  std::vector<Fault> faults;
  const std::string version = text_or_empty(bundle["version"]);
  const std::string digest = text_or_empty(bundle["digest"]);
  const std::string channel = text_or_empty(bundle["channel"]);
  const std::vector<std::string> compatible = strings(bundle["compatible"]);
  const std::vector<std::string> changes = strings(bundle["changes"]);

  if (js::trim(version).empty()) {
    faults.push_back({"unversioned", "error",
                      "This bundle names no version, so nothing can say what the robot would be "
                      "running afterwards. Rebuild it with a version.",
                      {}});
  }

  // Before the certification gate, because that gate binds a bench pass to this digest and a bundle
  // with no digest cannot be bound to anything.
  if (js::trim(digest).empty()) {
    faults.push_back({"undigested", "error",
                      "This bundle names no digest. §12's certification gate binds a bench pass to "
                      "the exact bytes it was taken against, and bytes with no name cannot be bound "
                      "to.",
                      {}});
  }

  // §37.1's five outcomes, kept apart: four different facts with four different people at the end.
  const std::string signature(subject["signature"].string_or(""));
  if (signature == "failed") {
    faults.push_back({"signature_failed", "error",
                      "A signature was present and did not verify. The bytes are attested to "
                      "somebody other than the manifest claims, which is what tampering looks like — "
                      "do not install this, and find out where it came from.",
                      {}});
  } else if (signature == "untrusted") {
    faults.push_back({"signature_untrusted", "error",
                      "The signature verifies against a key this robot has not been told to believe. "
                      "Add the key with `skynetd trust add`, or find out why a stranger signed an OS "
                      "image for this fleet.",
                      {}});
  } else if (signature == "absent") {
    faults.push_back({"signature_absent", "error",
                      "Nobody signed this bundle. §12 signs images with the same Ed25519 machinery "
                      "as releases, and an unsigned root filesystem is a root filesystem anybody on "
                      "the path could have written.",
                      {}});
  } else if (signature != "verified") {
    faults.push_back({"signature_unverifiable", "error",
                      "This platform cannot check a signature at all, so it cannot establish that "
                      "this image is the one the fleet published. That is the operating system's "
                      "problem rather than the publisher's.",
                      {}});
  }

  // Compatibility. Declaring nothing is a warning: §12 does not require the field.
  const Json& board_field = subject["board"];
  const bool has_board = !board_field.is_null();
  const std::string board = has_board ? js::interpolate(&board_field) : "";
  if (compatible.empty()) {
    faults.push_back({"compatibility_undeclared", "warning",
                      "This bundle declares no compatible machines, so nothing here can tell whether "
                      "it is built for this one.",
                      {}});
  } else if (has_board && std::find(compatible.begin(), compatible.end(), board) == compatible.end()) {
    faults.push_back({"incompatible", "error",
                      "This bundle is built for " + joined(compatible) + ", and this machine is `" +
                          board + "`.",
                      compatible});
  } else if (!has_board) {
    // A robot that cannot say what board it is has not established a mismatch.
    faults.push_back({"compatibility_undeclared", "warning",
                      "This machine does not report what it is, so the bundle's compatibility with " +
                          joined(compatible) + " could not be checked.",
                      compatible});
  }

  const std::string wanted = text_or_empty(subject["channel"]);
  if (!wanted.empty() && !channel.empty() && wanted != channel) {
    faults.push_back({"channel_mismatch", "error",
                      "The desired state asks for the `" + wanted + "` channel and this bundle is `" +
                          channel +
                          "`. A robot pulled onto another channel by an image rather than by an "
                          "operator is a robot nobody moved.",
                      {}});
  }

  // ── §12's third rule ───────────────────────────────────────────────────────
  std::vector<std::string> gated_components;
  for (const std::string& component : changes) {
    if (gated(component)) {
      gated_components.push_back(component);
    }
  }
  std::sort(gated_components.begin(), gated_components.end(),
            [](const std::string& left, const std::string& right) {
              return js::utf16_less(left, right);
            });
  gated_components.erase(std::unique(gated_components.begin(), gated_components.end()),
                         gated_components.end());

  if (!gated_components.empty()) {
    std::vector<std::string> unrecognised;
    for (const std::string& component : gated_components) {
      if (!listed(kOsComponents, component)) {
        unrecognised.push_back(component);
      }
    }
    const Json& bench = subject["bench"];
    const Json* held_digest = bench.has("image_digest") ? &bench["image_digest"] : nullptr;
    const bool same_bytes = held_digest != nullptr && held_digest->is_string() &&
                            held_digest->string_or("") == digest;
    if (bench.is_null()) {
      faults.push_back({"certification_required", "error",
                        certification_message(gated_components, unrecognised), gated_components});
    } else if (!same_bytes || digest.empty()) {
      faults.push_back({"bench_pass_stale", "error",
                        "The bench pass this robot holds was taken against `" +
                            display_digest(held_digest) + "` and this bundle is `" +
                            display_digest(digest) +
                            "`. Evidence that is not bound to what it is evidence of is not "
                            "evidence — book a fresh bench pass for these bytes.",
                        gated_components});
    } else if (!bench["passed"].is_true()) {
      faults.push_back({"bench_pass_failed", "error",
                        "A bench pass exists for these bytes and it records a failure. This update "
                        "changes " +
                            joined(gated_components) +
                            ", and §12 refuses one that has not passed rather than one that has not "
                            "been tried.",
                        gated_components});
    }
  }

  const std::string running = text_or_empty(subject["running_version"]);
  if (!running.empty() && running == version) {
    faults.push_back({"already_running", "warning",
                      "This robot is already running `" + version +
                          "`. Staging it again is harmless and applying it costs a reboot for "
                          "nothing.",
                      {}});
  }

  bool admitted = true;
  Json faults_json = Json::array();
  for (const Fault& fault : faults) {
    admitted = admitted && fault.severity != "error";
    Json subjects = Json::array();
    for (const std::string& one : fault.subjects) {
      subjects.push(Json::string(one));
    }
    Json row = Json::object();
    row.set("code", Json::string(fault.code));
    row.set("severity", Json::string(fault.severity));
    row.set("message", Json::string(fault.message));
    row.set("subjects", subjects);
    faults_json.push(row);
  }
  Json gated_json = Json::array();
  for (const std::string& one : gated_components) {
    gated_json.push(Json::string(one));
  }

  Json out = Json::object();
  out.set("admitted", Json::boolean(admitted));
  out.set("faults", faults_json);
  out.set("gated_components", gated_json);
  return out;
}

Json boot_verdict(std::string decision, std::string detail, const std::vector<std::string>& missing) {
  Json list = Json::array();
  for (const std::string& one : missing) {
    list.push(Json::string(one));
  }
  Json out = Json::object();
  out.set("decision", Json::string(std::move(decision)));
  out.set("detail", Json::string(std::move(detail)));
  out.set("missing", list);
  return out;
}

Json confirm_boot(const Json& evidence) {
  // A robot that is not pending decides nothing: marking a slot bad on an ordinary boot is a
  // rollback of an update nobody made.
  if (!evidence["pending"].is_true()) {
    return boot_verdict("not_pending",
                        "This boot is not the first into a new slot, so there is nothing to confirm "
                        "and nothing to roll back.",
                        {});
  }

  const double elapsed = js::bare_number(evidence, "elapsed_ms", 0);
  const double budget = js::bare_number(evidence, "budget_ms", kDefaultBootBudgetMs);
  const std::string elapsed_text = js::number_to_string(elapsed);
  const std::string budget_text = js::number_to_string(budget);

  // Absence of evidence is not health: every check is optional and absent never confirms.
  std::vector<std::string> missing;
  if (!evidence["post_passed"].is_true()) missing.emplace_back("post");
  if (!evidence["packages_healthy"].is_true()) missing.emplace_back("packages");
  if (!evidence["converged"].is_true()) missing.emplace_back("converged");
  std::sort(missing.begin(), missing.end());

  if (missing.empty()) {
    return boot_verdict("confirm",
                        "The self-test passed, every wanted package is running and the reconciler "
                        "converged, " +
                            elapsed_text + " ms into a " + budget_text + " ms budget. The slot is good.",
                        missing);
  }

  // A budget that is not a positive number is no budget, and no budget waits rather than rolling
  // back: the patient direction is the safe one when the budget itself is misconfigured.
  const bool expired = std::isfinite(budget) && budget > 0 && std::isfinite(elapsed) &&
                       elapsed >= budget;
  if (expired) {
    return boot_verdict("roll_back",
                        elapsed_text + " ms have passed and the budget is " + budget_text +
                            " ms, and " + joined(missing) + " still " +
                            (missing.size() == 1 ? "has" : "have") +
                            " not passed. Marking the slot bad; the bootloader returns to the "
                            "previous one.",
                        missing);
  }

  return boot_verdict("wait",
                      elapsed_text + " ms into a " + budget_text + " ms budget, with " +
                          joined(missing) +
                          " still to pass. Saying nothing to the bootloader until they do or the "
                          "budget expires.",
                      missing);
}

}  // namespace

Json ota(const std::string& label, const Json& input) {
  if (label == "bundle") {
    return admit_bundle(input["bundle"], input["subject"]);
  }
  if (label == "boot") {
    return confirm_boot(input);
  }
  throw Unimplemented("ota", label);
}

}  // namespace skynet::contracts

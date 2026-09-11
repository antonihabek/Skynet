// §12, §34.3 and §79's transfers, C++ edition.
//
// Transcribed from `reference/typescript/bundle.ts` (fetching an OS image, and checking what
// arrived) and `reference/typescript/upload.ts` (sending a sealed black box window, and checking
// what the receiver ended up with). Every rule and message string is read from those files, never
// from `corpus/`.
//
// The two are mirror images and the whole of the difference is one sentence from §79.2:
//
//     §75's fetch has a rule that makes progress free: *"the partial's length is the progress"*. An
//     upload cannot have that rule, because **the partial is on the other machine.**
//
// Three consequences follow, and they are why an upload is a separate function rather than a flag:
//
//   1. **There is a probe.** A decision this side cannot reach without asking. An edition that
//      folded it into "start" would re-send whole windows.
//   2. **The offset is a claim, not evidence.** It places the next byte and never concludes a
//      transfer; that is the digest's job.
//   3. **A failure cannot discard anything.** The bundle check deletes a file the robot owns; the
//      upload check can only stop believing an offset and start again at zero. The robot has no
//      authority over the receiver's disk, and a contract that pretended otherwise would be one
//      whose remedy nothing could carry out.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `Math.trunc`
//     Truncates **toward zero**, where a flooring division would send `-1.5` to `-2`. Both are then
//     clamped at zero here, so the difference is invisible on every input that reaches this file —
//     which is exactly the kind of agreement that stops being true when somebody changes the clamp.
//
// `Number.isFinite`
//     False for a boolean, and the size guards test it before comparing. A declared size of `true`
//     is not a size.
//
// template literals
//     `String(1024)` is `"1024"`. Every byte count in every sentence here goes through
//     `js::number_to_string` for that reason; the corpus compares the sentences byte for byte.

#include "skynet_os/contracts.hpp"
#include "skynet_os/corpus.hpp"
#include "skynet_os/js.hpp"

#include <cmath>
#include <string>

namespace skynet::contracts {
namespace {

/// `Number.isFinite` — false for a boolean.
bool is_finite(const Json& value) {
  return value.is_number() && std::isfinite(value.number_or(0));
}

/// `Math.max(0, Math.trunc(value ?? 0))`.
double at_least_zero(const Json& value) {
  if (!is_finite(value)) {
    return 0;
  }
  return std::max(0.0, std::trunc(value.number_or(0)));
}

/// `(value ?? '').trim()`.
std::string text_of(const Json& value) {
  return value.is_null() ? "" : js::trim(js::interpolate(&value));
}

/// `bearer.name ?? '?'` — a bearer that will not name itself still gets a sentence.
std::string bearer_name(const Json& bearer) {
  return bearer["name"].is_null() ? "?" : js::interpolate(&bearer["name"]);
}

Json plan(std::string decision, double from, std::string detail) {
  Json out = Json::object();
  out.set("decision", Json::string(std::move(decision)));
  out.set("from", Json::number(from));
  out.set("detail", Json::string(std::move(detail)));
  return out;
}

Json bundle_check(std::string code, bool ok, bool discard, std::string detail) {
  Json out = Json::object();
  out.set("code", Json::string(std::move(code)));
  out.set("ok", Json::boolean(ok));
  out.set("discard", Json::boolean(discard));
  out.set("detail", Json::string(std::move(detail)));
  return out;
}

Json upload_check(std::string code, bool ok, bool restart, std::string detail) {
  Json out = Json::object();
  out.set("code", Json::string(std::move(code)));
  out.set("ok", Json::boolean(ok));
  out.set("restart", Json::boolean(restart));
  out.set("detail", Json::string(std::move(detail)));
  return out;
}

/// What to do about one OS image.
///
/// The order of the tests is **the order of cost**. Having it already is free; refusing costs
/// nothing; waiting for a link costs nothing; and only then is a transfer considered. A version
/// that checked metering first would report a robot as waiting for an unmetered bearer to fetch an
/// image it already has.
Json plan_fetch(const Json& subject) {
  const std::string digest = text_of(subject["digest"]);
  const double partial = at_least_zero(subject["partial"]);
  const Json& size = subject["size"];
  const bool sized = !size.is_null();

  if (subject["have"].is_true()) {
    return plan("have", 0,
                "This bundle is already on the disk and has been checked against the manifest. "
                "Nothing to fetch.");
  }

  if (digest.empty()) {
    return plan("refuse", 0,
                "This release names no bundle digest, so there is nothing to check the bytes "
                "against. §12 binds a bench pass to the exact bytes it was taken against, and bytes "
                "with no name cannot be bound to — so they are not fetched either.");
  }

  const Json& bearer = subject["bearer"];
  if (bearer.is_null()) {
    return plan("no_bearer", 0,
                "No bearer is up, so there is nothing to fetch over. That is the ordinary state for "
                "most of a robot's life (§34.1) and nothing about the update is lost by it — the "
                "release manifest is already here and the transfer resumes when a link returns.");
  }

  if (bearer["metered"].is_true()) {
    const std::string declared = bearer["metering_declared"].is_true() ? "declared" : "assumed";
    return plan("wait_for_unmetered", 0,
                "An OS image is the largest bulk transfer this system has, and `" +
                    bearer_name(bearer) + "` is " + declared +
                    " metered. It waits for an unmetered link (§34.3). Nothing is lost by waiting, "
                    "and a fleet that downloaded hundreds of megabytes per robot over cellular "
                    "would find out at the end of the month.");
  }

  // A partial at least as large as the whole thing is not a partial of this bundle. Resuming from
  // it would append this image's tail to something else's, and the mismatch would only surface
  // after the entire transfer — which on a poor link is hours.
  if (sized && is_finite(size) && size.number_or(0) > 0 && partial >= size.number_or(0)) {
    return plan("discard_and_fetch", 0,
                "There are " + js::number_to_string(partial) +
                    " bytes here for a bundle the manifest says is " + js::interpolate(&size) +
                    ". A partial cannot be larger than what it is part of, so this is left over "
                    "from something else. It is discarded and the transfer starts again.");
  }

  if (partial > 0) {
    const std::string of_size = sized ? " of " + js::interpolate(&size) : "";
    return plan("resume", partial,
                "Resuming at " + js::number_to_string(partial) + " bytes" + of_size +
                    ". The partial is keyed to this bundle's digest, so it cannot be a different "
                    "image's tail.");
  }

  const std::string what = sized ? js::interpolate(&size) + " bytes" : "the bundle";
  return plan("fetch", 0, "Fetching " + what + " over `" + bearer_name(bearer) + "`.");
}

/// Are these the bytes the signed manifest describes (§12)?
///
/// Size is checked before the digest, and the reason is **the sentence rather than the verdict** —
/// both refuse. A file short of its declared size is a transfer that stopped, which is ordinary and
/// resumable; a file of exactly the right size whose digest is wrong is a different set of bytes.
/// Reporting the first as a digest mismatch would send an engineer looking for tampering every time
/// a link dropped.
///
/// `short` keeps the file, because that is exactly what a resume is for. Everything else deletes
/// it: bytes that will never hash correctly are bytes a later resume would append to for ever.
Json check_bundle(const Json& arrived, const Json& expected) {
  const std::string want = text_of(expected["digest"]);
  const std::string got = text_of(arrived["digest"]);
  const Json& size = expected["size"];
  const bool sized = !size.is_null();
  const double actual = arrived["size"].is_null() ? 0 : arrived["size"].number_or(0);

  if (sized && is_finite(size) && actual < size.number_or(0)) {
    return bundle_check("short", false, false,
                        js::number_to_string(actual) + " of " + js::interpolate(&size) +
                            " bytes arrived. The transfer stopped early, which is what a link "
                            "dropping looks like. The partial is kept and the next attempt resumes "
                            "from it.");
  }

  if (sized && is_finite(size) && actual > size.number_or(0)) {
    return bundle_check("long", false, true,
                        js::number_to_string(actual) +
                            " bytes arrived for a bundle the manifest says is " +
                            js::interpolate(&size) +
                            ". More than was declared is not a transfer that stopped — it is a "
                            "different artifact, or a proxy that served something else. Discarded.");
  }

  if (want.empty() || got.empty() || want != got) {
    return bundle_check(
        "digest_mismatch", false, true,
        "These bytes do not hash to what the signed release manifest says this bundle is. §12 binds "
        "every gate it applies to that digest, so bytes that are not it are not this release — do "
        "not install them, and find out where they came from. Discarded, because bytes that will "
        "never verify are bytes a resume would append to for ever.");
  }

  return bundle_check("match", true, false, "These bytes are the release the manifest describes.");
}

/// What to do about one sealed window (§79, §34.3).
///
/// The order of the tests is the order of cost, as in the fetch. Having a receipt is free; refusing
/// costs nothing; waiting for a link costs nothing; asking costs one small request; and only then
/// are megabytes considered.
Json plan_upload(const Json& subject) {
  const std::string digest = text_of(subject["digest"]);
  const double size = at_least_zero(subject["size"]);

  if (subject["receipt"].is_true()) {
    return plan("done", 0,
                "The receiver has acknowledged this window by digest and the receipt is on the "
                "disk. Nothing to send.");
  }

  if (digest.empty()) {
    return plan("refuse", 0,
                "This window carries no digest, so there is no address to send it to and nothing to "
                "check the far side's answer against. §79.1 makes the digest the route, and bytes "
                "with no name have nowhere to go.");
  }

  if (size == 0) {
    return plan("refuse", 0,
                "This window is empty. A seal that produced no bytes is a fact about the recorder "
                "worth reporting (§77.2) and is not something to upload — a zero-length object "
                "under a digest would be an artifact an investigator could open and mistake for the "
                "window.");
  }

  const Json& bearer = subject["bearer"];
  if (bearer.is_null()) {
    return plan("no_bearer", 0,
                "No bearer is up, so there is nothing to send over. That is the ordinary state for "
                "most of a robot's life (§34.1) and nothing is lost by it — the window is sealed on "
                "the disk and the incident naming it is already queued.");
  }

  if (bearer["metered"].is_true()) {
    const std::string declared = bearer["metering_declared"].is_true() ? "declared" : "assumed";
    return plan("wait_for_unmetered", 0,
                "A sealed window is tens of megabytes and `" + bearer_name(bearer) + "` is " +
                    declared +
                    " metered, so it waits for an unmetered link (§34.3). The alarm does not wait: "
                    "§79.3 puts the incident on any bearer that is up, so an operator already knows "
                    "this recording exists and what it hashes to.");
  }

  const Json& held = subject["held"];
  if (held.is_null()) {
    return plan("probe", 0,
                "The receiver has not been asked how much of this window it holds. §79.2: the "
                "partial is on the other machine, so an upload has a step a download does not — and "
                "a robot that skipped it would re-send the whole window every pass.");
  }

  const double bytes_held = at_least_zero(held["bytes"]);
  const std::string held_digest = text_of(held["digest"]);

  if (bytes_held >= size) {
    if (bytes_held == size && !held_digest.empty() && held_digest == digest) {
      return plan("done", 0,
                  "The receiver holds all " + js::number_to_string(size) +
                      " bytes and says they hash to this window's digest. Nothing is sent and a "
                      "receipt is written — which is the state a robot reaches after an upload "
                      "whose acknowledgement was lost.");
    }
    const std::string because =
        bytes_held == size
            ? ", and does not say what they hash to. A complete object nobody will name is not "
              "evidence this window arrived, and writing a receipt for it would record an upload "
              "that may never have happened"
            : ". A partial cannot be larger than what it is part of, so what is over there is not "
              "part of this window";
    return plan("restart", 0,
                "The receiver claims " + js::number_to_string(bytes_held) + " bytes for a window of " +
                    js::number_to_string(size) + because + ". It is sent again from the beginning.");
  }

  if (bytes_held > 0) {
    return plan("resume", bytes_held,
                "Resuming at " + js::number_to_string(bytes_held) + " of " +
                    js::number_to_string(size) +
                    " bytes. The offset is the receiver's claim and is used to place the next byte, "
                    "never to conclude the window arrived — that is the digest's job (§79.2).");
  }

  return plan("start", 0,
              "Sending " + js::number_to_string(size) + " bytes over `" + bearer_name(bearer) + "`.");
}

/// Did the window arrive (§79.2, §79.4)?
///
/// The same four outcomes as the bundle check and one different remedy, which is the asymmetry
/// worth naming: a bundle that fails its check is **deleted**, because the robot owns those bytes.
/// A window that fails this check cannot be deleted from anywhere — the robot can only decline to
/// trust the offset it was given and begin again. `restart` is therefore about what *this* robot
/// will do next, never about what the receiver should do with what it has.
Json check_upload(const Json& reported, const Json& window) {
  const std::string want = text_of(window["digest"]);
  const std::string got = text_of(reported["digest"]);
  const double size = at_least_zero(window["size"]);
  const double bytes_held = at_least_zero(reported["bytes"]);

  if (bytes_held < size) {
    return upload_check("short", false, false,
                        "The receiver holds " + js::number_to_string(bytes_held) + " of " +
                            js::number_to_string(size) +
                            " bytes. The transfer stopped early, which is what a link dropping "
                            "looks like, and the next pass resumes from what is there.");
  }

  if (bytes_held > size) {
    return upload_check("long", false, true,
                        "The receiver holds " + js::number_to_string(bytes_held) +
                            " bytes for a window of " + js::number_to_string(size) +
                            ". More than was sent is not a transfer that stopped — it is another "
                            "object under this digest, or a proxy answering for one. The next "
                            "attempt begins at zero; this robot cannot delete what is over there "
                            "and does not pretend to.");
  }

  if (want.empty() || got.empty() || want != got) {
    return upload_check(
        "digest_mismatch", false, true,
        "The receiver holds the right number of bytes and they are not this window's. The digest "
        "was recorded at the seal, before any transfer existed (§77.1), so it is the half of this "
        "comparison that is evidence. No receipt is written and the next attempt begins at zero.");
  }

  return upload_check(
      "complete", true, false,
      "The receiver holds the whole window and it hashes to what was sealed. A receipt is written "
      "beside the recording; the recording itself is kept, because §39.1's local route is for the "
      "operator standing next to the robot (§79.4).");
}

}  // namespace

Json transfer(const std::string& label, const Json& input) {
  if (label == "fetch") {
    return plan_fetch(input["subject"]);
  }
  if (label == "arrival") {
    return check_bundle(input["arrived"], input["expected"]);
  }
  if (label == "upload") {
    return plan_upload(input["subject"]);
  }
  if (label == "receipt") {
    return check_upload(input["reported"], input["window"]);
  }
  throw Unimplemented("transfer", label);
}

}  // namespace skynet::contracts

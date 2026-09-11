// Provenance classes for on-robot evaluation results — §56, C++ edition.
//
// Transcribed from `reference/typescript/provenance.ts`. Every rule and message string is read from
// that file, never from `corpus/`.
//
// §56.1 states the problem in one sentence and the whole design follows from it:
//
//     **A leaderboard fed by self-reported on-robot numbers is trivially gameable** — a publisher
//     evaluating their own policy on their own robot, choosing their own seeds, and reporting their
//     own score is not producing evidence.
//
//     So an on-robot evaluation result is only worth as much as its attestation.
//
// ── The three classes, and why the worst one is accepted ───────────────────
//
//   field        A real robot, valid birth certificate, attested image   Eligible
//   simulated    A simulator RHAL driver, which must declare itself      Eligible, separately ranked
//   unattested   No device signature, or a device the Hub does not know  Accepted and never ranked
//
// §56.2: *"`unattested` is accepted rather than refused because a researcher on a bench robot has a
// legitimate result worth recording — and it is never ranked, because it is not evidence."* That
// distinction — **recorded** and **not ranked** — is the whole posture.
//
// ── The order of the checks is the anti-gaming argument ────────────────────
//
// Only one of the three classes is worth lying to obtain, and it is `field`. So an untrustworthy
// device signature is `unattested` **before anything else is looked at**, because a bundle nobody
// can attribute makes claims about its own simulator flag, its own seeds and its own birth
// certificate, and none of those claims is evidence either. A *declared* simulator is `simulated`
// and can never be `field`, however perfect the rest of it is.
//
// ── A `null` score is a `NaN` score ────────────────────────────────────────
//
// `scores.on_robot` is typed `number` in the reference — not `number | null` — so a `null` in the
// case file did not come from a caller. It came from the generator, which writes every input
// through `JSON.stringify`, and `JSON.stringify(NaN)` is `null`.
//
// That makes the reading forced rather than a choice: in a field the reference declares as a bare
// `number`, a `null` can only be a non-finite value the format cannot spell. The rule does **not**
// generalise to every null — where the reference's own type admits one, `Frame.worstSkewUs:
// number | null` being the clearest, a `null` means null. The type is what decides, and
// `corpus/README.md` says so.

#include "skynet_os/contracts.hpp"
#include "skynet_os/js.hpp"

#include <cmath>
#include <limits>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

struct Reason {
  std::string code;
  std::vector<std::string> subjects;
};

/// `reason.subjects[n] ?? ''` — an out-of-range index is `undefined`, not an exception.
std::string subject(const Reason& reason, std::size_t index) {
  return index < reason.subjects.size() ? reason.subjects[index] : "";
}

/// Whether this reason is why the bundle cannot be `field`.
///
/// Everything except `not_replayable`: a result can be perfectly attested and unreplayable.
bool blocks_field(const Reason& reason) { return reason.code != "not_replayable"; }

/// The sentence a publisher gets.
std::string explain(const Reason& reason) {
  if (reason.code == "device_signature") {
    return "The device's signature over this bundle is " + subject(reason, 0) +
           ", so nothing in it is attributable to a machine. The result is recorded and is not "
           "ranked: a number nobody can attribute is not evidence, which is the whole of §56.1.";
  }
  if (reason.code == "device_unknown") {
    return "The signature verifies and this Hub does not know the device that made it. Nothing is "
           "wrong with the bytes; what is missing is a registered machine to attribute them to. "
           "Register the device, then re-submit.";
  }
  if (reason.code == "simulator_declared") {
    return "The RHAL driver declared itself a simulator (§35.3), so this is a simulated result and "
           "is ranked on the simulated table. This is the driver meeting its obligation rather "
           "than a fault — and a simulated run and a field run are not comparable numbers, which "
           "is why the tables are separate.";
  }
  if (reason.code == "birth_unattested") {
    return "This robot's birth certificate does not attest it (§50), so \"a real robot of this "
           "model with these joint ratings\" is a claim the machine makes about itself. The result "
           "is recorded and not ranked as a field result.";
  }
  if (reason.code == "image_unattested") {
    return "The OS image running during this evaluation was not attested, so what produced these "
           "numbers is not established. Recorded, not ranked as a field result.";
  }
  if (reason.code == "seeds_not_supplied") {
    // The counts rather than the seeds: a benchmark can carry hundreds, and a sentence that pasted
    // both lists would be one nobody reads.
    return "This run used seeds the benchmark did not supply (" + subject(reason, 0) + " given, " +
           subject(reason, 1) +
           " used). §56.3: seeds come from the benchmark and are never generated by the robot, "
           "because a robot that picks its own seeds picks easy ones — and it need not do so "
           "dishonestly for the number to be worthless. This is not a run of that benchmark.";
  }
  if (reason.code == "benchmark_mismatch") {
    return "This bundle is a run of benchmark " + subject(reason, 1) + " and is being ranked "
           "against " + subject(reason, 0) +
           ". Two benchmarks are two questions, and a score against one says nothing about the "
           "other.";
  }
  return "No black box segment accompanies this run, so a disputed result cannot be replayed "
         "(§56.4). The class is unaffected — a result can be perfectly attested and unreplayable — "
         "but a number with a recording behind it is a different class of claim from a number in a "
         "table.";
}

bool finite(const Json& value) {
  return value.is_number() && std::isfinite(value.number_or(0));
}

/// A score off the wire. A `null` in a field typed `number` is a NaN — see the header.
double score_of(const Json& value) {
  return finite(value) ? value.number_or(0) : std::numeric_limits<double>::quiet_NaN();
}

}  // namespace

Json provenance(const std::string& label, const Json& input) {
  if (label.find("scores") != std::string::npos) {
    // §56.5's roll-up comparison. The sentence travels beside the boolean rather than being left to
    // the caller, because §56.5's whole point is that a disagreement is a defect to investigate —
    // and a bare `false` is the shape of a thing somebody quietly widens a tolerance around.
    const double on_robot = score_of(input["scores"]["on_robot"]);
    const double on_hub = score_of(input["scores"]["on_hub"]);
    // An equality test with a tolerance, and the tolerance exists only for the floating-point
    // **representation** rather than for the arithmetic: 1e-9, which no genuine scoring difference
    // can hide under. A non-finite score is not a score, and two of them are not an agreement —
    // returning true there would report a robot and a Hub that both failed to compute anything as
    // being in perfect accord.
    const bool agree = std::isfinite(on_robot) && std::isfinite(on_hub) &&
                       std::fabs(on_robot - on_hub) <= 1e-9;

    Json out = Json::object();
    out.set("agree", Json::boolean(agree));
    out.set("detail",
            agree ? Json::null()
                  : Json::string(
                        "The robot rolled this run up to " + js::number_to_string(on_robot) +
                        " and the Hub to " + js::number_to_string(on_hub) +
                        ". The Hub's number is authoritative, and the difference is a defect "
                        "rather than a rounding artefact: both sides run the same scoring contract "
                        "over the same per-episode outcomes, so they disagreeing means the "
                        "implementations diverged — which is precisely what the conformance corpus "
                        "(§31.4) exists to catch."));
    return out;
  }

  // §56.2's class, and every reason it is not a better one.
  //
  // `expected_benchmark` and `expected_seeds` come from the benchmark manifest, **not** from the
  // bundle. Passing the bundle's own values would make §56.3 a check a robot performs on itself,
  // which is the shape of every problem in §56.1.
  const Json& bundle = input["bundle"];
  const Json& expected_benchmark = input["expected_benchmark"];
  const JsonArray& expected_seeds = input["expected_seeds"].items();

  std::vector<Reason> reasons;

  // §56.4, first because it is the one observation that never changes the class. Collected up front
  // so the reason list reads in the order a person would ask the questions.
  //
  // Null and absent, and only those two: an **empty string** is a segment somebody recorded as
  // empty rather than a missing one, and it does not raise `not_replayable`.
  if (!bundle.has("blackbox_segment") || bundle["blackbox_segment"].is_null()) {
    reasons.push_back({"not_replayable", {}});
  }

  const Json& signature = bundle["device_signature"];
  const bool verified = signature.is_string() && signature.string_or("") == "verified";
  if (!verified) {
    reasons.push_back({"device_signature", {js::interpolate(&signature)}});
  } else if (!bundle["device_known"].is_true()) {
    reasons.push_back({"device_unknown", {}});
  }

  const Json& claimed_benchmark = bundle["benchmark_digest"];
  const bool benchmark_matches =
      claimed_benchmark.kind() == expected_benchmark.kind() &&
      claimed_benchmark.canonical() == expected_benchmark.canonical();
  if (!benchmark_matches) {
    reasons.push_back({"benchmark_mismatch",
                       {js::interpolate(&expected_benchmark), js::interpolate(&claimed_benchmark)}});
  }

  // Order and membership both. Two runs over the same seeds in a different order are two different
  // sequences of episodes, and a benchmark that fixes its seeds is fixing the sequence.
  const JsonArray& seeds = bundle["seeds"].items();
  bool same_seeds = seeds.size() == expected_seeds.size();
  if (same_seeds) {
    for (std::size_t at = 0; at < seeds.size(); ++at) {
      if (seeds[at].kind() != expected_seeds[at].kind() ||
          seeds[at].canonical() != expected_seeds[at].canonical()) {
        same_seeds = false;
        break;
      }
    }
  }
  if (!same_seeds) {
    reasons.push_back({"seeds_not_supplied",
                       {std::to_string(expected_seeds.size()), std::to_string(seeds.size())}});
  }

  const Json& simulator = bundle["simulator"];
  const bool simulated = simulator.is_true() ||
                         (simulator.is_number() && simulator.number_or(0) != 0) ||
                         (simulator.is_string() && !simulator.string_or("").empty()) ||
                         simulator.is_array() || simulator.is_object();
  if (simulated) {
    reasons.push_back({"simulator_declared", {}});
  } else {
    if (!bundle["birth_attested"].is_true()) {
      reasons.push_back({"birth_unattested", {}});
    }
    if (!bundle["image_attested"].is_true()) {
      reasons.push_back({"image_unattested", {}});
    }
  }

  // The order of these two branches is the anti-gaming argument, and it is the one thing in this
  // file that must not be rearranged for tidiness.
  //
  // A bundle nobody can attribute is `unattested` **before** its simulator flag is consulted,
  // because that flag is one of its own claims. And a *declared* simulator is `simulated` even when
  // everything else about it is perfect, because the one result this scheme exists to prevent is a
  // simulator on the field leaderboard.
  const bool attributable = verified && bundle["device_known"].is_true();

  std::string provenance_class;
  if (!attributable) {
    provenance_class = "unattested";
  } else if (simulated) {
    provenance_class = "simulated";
  } else {
    bool blocked = false;
    for (const Reason& reason : reasons) {
      if (blocks_field(reason)) {
        blocked = true;
        break;
      }
    }
    provenance_class = blocked ? "unattested" : "field";
  }

  bool replayable = true;
  Json reason_rows = Json::array();
  for (const Reason& reason : reasons) {
    if (reason.code == "not_replayable") {
      replayable = false;
    }
    Json row = Json::object();
    row.set("blocks_field", Json::boolean(blocks_field(reason)));
    row.set("code", Json::string(reason.code));
    row.set("message", Json::string(explain(reason)));
    Json subjects = Json::array();
    for (const std::string& one : reason.subjects) {
      subjects.push(Json::string(one));
    }
    row.set("subjects", subjects);
    reason_rows.push(row);
  }

  Json out = Json::object();
  out.set("class", Json::string(provenance_class));
  // §56.2 says *"eligible, separately ranked"* and the separation is not a courtesy. A simulated run
  // and a field run are not comparable numbers, and putting them in one table would make the
  // leaderboard a worse instrument than no leaderboard.
  out.set("rankable",
          Json::boolean(provenance_class == "field" || provenance_class == "simulated"));
  out.set("ranked_separately", Json::boolean(provenance_class == "simulated"));
  out.set("reasons", reason_rows);
  out.set("replayable", Json::boolean(replayable));
  return out;
}

}  // namespace skynet::contracts

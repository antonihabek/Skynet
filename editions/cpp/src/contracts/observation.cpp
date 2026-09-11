// Observation assembly (§25.1), C++ edition.
//
// Transcribed from `reference/typescript/observation.ts`. Every rule and every message string is
// read from that file, never from `corpus/`.
//
// §25.1 calls this "one of the harder parts of the whole system", and the reason is one sentence:
//
//     A policy is trained on synchronised frames: image and joint state as of one instant. A real
//     robot has three cameras at 30 Hz with 20-60 ms of pipeline latency, joint state at 1 kHz with
//     1 ms, and an IMU at 200 Hz — none of them aligned. Feed a policy the most recent value from
//     each and it receives **a frame that never existed**, in which the arm is where it is *now*
//     and the image shows where it was two frames ago.
//
// The last clause is the expensive part: a skewed frame does not announce itself. It presents as a
// policy that grasps slightly late, or oscillates on contact, or works on the bench and not on the
// robot — every one of which sends somebody to retrain a model that was fine. This contract does
// not synchronise anything, and §25.1 is explicit that it should not try. It makes the skew
// **measured**.
//
// ── A divergence in the reference itself ────────────────────────────────────
//
// `roundHalfAway` is defined **twice** in the reference — here and in `kinematics.ts` — with one
// name and one stated contract, and the two disagree: at 0.49999999999999994 `kinematics.ts`
// answers 1 and this file answers 0, and on a non-finite input the first propagates it while this
// one answers 0. `kinematics.ts` computes `trunc(v ± 0.5)`, and at the largest double below a half
// that sum rounds up to exactly 1.0.
//
// This edition transcribes **this file's** version here, because a transcription that quietly
// unified them would be an edition disagreeing with the reference in order to be right. Neither
// input is reachable from a real latency, and the two rounders are still one function with two
// behaviours, which is the shape of defect §7.4's redundant channel exists to catch.

#include "skynet_os/contracts.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/observation.hpp"

#include <algorithm>
#include <cmath>

namespace skynet::observation {
namespace {

constexpr double kNsPerMs = 1000000.0;

/// Seventy-one minutes of skew. Not a real case; it is here because the Rust edition's alternative
/// in a release build is a wrap, and a frame recorded as four microseconds skewed when it was four
/// thousand seconds skewed is worse than one recorded at the ceiling.
constexpr double kU32Max = 4294967295.0;

/// Round half away from zero — **not** `Math.round`, which sends -0.5 to -0.
///
/// The Rust edition cannot use `f64::round` either: it is unavailable under `no_std` without a
/// maths library, and §31.2's rule about platform maths applies with full force to a rounding
/// inside a latency correction. Two editions rounding a half-nanosecond differently is a
/// one-nanosecond disagreement today and a *different chosen sample* the day two candidates are
/// equidistant, so every edition writes the same four lines out.
double round_half_away(double value) {
  if (!std::isfinite(value)) {
    return 0;
  }
  const double truncated = std::trunc(value);
  const double fraction = value - truncated;
  if (fraction >= 0.5) {
    return truncated + 1;
  }
  if (fraction <= -0.5) {
    return truncated - 1;
  }
  return truncated;
}

/// Nanoseconds as milliseconds. One division of an exact integer, so every edition performing it
/// gets the same bits: IEEE-754 division is correctly rounded everywhere.
double milliseconds(double nanos) { return nanos / kNsPerMs; }

/// Milliseconds as whole microseconds, saturating at zero and at the top of a `u32`.
double micros(double milliseconds_value) {
  const double value = round_half_away(milliseconds_value * 1000);
  if (value <= 0) {
    return 0;
  }
  if (value >= kU32Max) {
    return kU32Max;
  }
  return value;
}

/// The instant a sample describes: its stamp, less the declared latency, in nanoseconds.
double effective_ns(double stamp_ns, const Json* latency_ms) {
  const double latency_ns =
      latency_ms == nullptr || latency_ms->is_null()
          ? 0
          : round_half_away(latency_ms->number_or(0) * kNsPerMs);
  return stamp_ns - latency_ns;
}

/// `value ?? null`. Absent and null are the same thing to every rule in this contract.
const Json* nullish(const Json& holder, const char* key) {
  if (!holder.has(key) || holder[key].is_null()) {
    return nullptr;
  }
  return &holder[key];
}

struct Chosen {
  double stamp_ns = 0;
  const Json* seq = nullptr;
  double effective_ns = 0;
  double age_ms = 0;
  bool over_budget = false;
  std::size_t considered = 0;
};

struct StreamFrame {
  std::string name;
  std::string modality;
  const Json* latency_ms = nullptr;
  const Json* uncertainty_ms = nullptr;
  const Json* budget_ms = nullptr;
  std::optional<Chosen> chosen;
};

struct Hold {
  std::string stream;
  std::string reason;
  std::string detail;
};

}  // namespace

Frame assemble(double target_ns, const Json& streams) {
  Frame frame;
  frame.target_ns = target_ns;

  std::vector<StreamFrame> rows;
  std::vector<Hold> holds;

  for (const Json& stream : streams.items()) {
    StreamFrame row;
    row.name = std::string(stream["name"].string_or(""));
    row.modality = std::string(stream["modality"].string_or(""));
    row.latency_ms = nullish(stream, "latency_ms");
    row.uncertainty_ms = nullish(stream, "uncertainty_ms");
    row.budget_ms = nullish(stream, "budget_ms");
    if (row.latency_ms == nullptr) {
      frame.undeclared_latency.push_back(row.name);
    }
    if (row.budget_ms == nullptr) {
      frame.undeclared_budget.push_back(row.name);
    }

    // Nearest is by absolute distance, so a sample *after* the target can be chosen when it is
    // genuinely closer — impossible on a live robot, correct in replay. **A tie takes the earlier
    // sample**: two samples equidistant either side of the target are equally good by the distance
    // rule and are not equally good in any other sense, since the earlier describes something that
    // had already happened at the instant the frame claims to describe. The tie-break also has to
    // exist for a duller reason — without it, two editions iterating in a different order would
    // pick different samples and the corpus would diverge on an input where nothing is wrong.
    const JsonArray& samples = stream["samples"].items();
    const Json* best_sample = nullptr;
    double best_effective = 0;
    for (const Json& sample : samples) {
      const double effective = effective_ns(sample["stamp_ns"].number_or(0), row.latency_ms);
      const double gap = std::fabs(effective - target_ns);
      bool better = best_sample == nullptr;
      if (!better) {
        const double best_gap = std::fabs(best_effective - target_ns);
        better = gap < best_gap || (gap == best_gap && effective < best_effective);
      }
      if (better) {
        best_sample = &sample;
        best_effective = effective;
      }
    }

    if (best_sample == nullptr) {
      holds.push_back(Hold{
          row.name, "no_sample",
          row.name + " (" + row.modality +
              ") offered no sample for this instant, so there is no frame to assemble. A stream "
              "the policy declared and the pipeline is not producing is a capture path that is not "
              "running."});
    } else {
      Chosen chosen;
      chosen.stamp_ns = (*best_sample)["stamp_ns"].number_or(0);
      chosen.seq = &(*best_sample)["seq"];
      chosen.effective_ns = best_effective;
      chosen.age_ms = milliseconds(target_ns - best_effective);
      chosen.considered = samples.size();
      // One-sided, and deliberately so. The budget bounds how far *behind* the target a sample may
      // be; a sample from ahead of it is a clock fault rather than a stale one, it is visible in
      // the signed age, and reporting it as staleness would send somebody to look at a capture
      // pipeline that is working.
      chosen.over_budget = row.budget_ms != nullptr && chosen.age_ms > row.budget_ms->number_or(0);
      if (chosen.over_budget) {
        holds.push_back(Hold{
            row.name, "stale",
            row.name + " (" + row.modality + ") is " + js::number_to_string(chosen.age_ms) +
                " ms behind the frame's instant and the policy's budget for it is " +
                js::number_to_string(row.budget_ms == nullptr ? 0 : row.budget_ms->number_or(0)) +
                " ms. §25.1: exceeding it is a hold, not a silently older frame."});
      }
      row.chosen = chosen;
    }

    rows.push_back(std::move(row));
  }

  // The spread across the streams, which is the frame's own internal inconsistency.
  std::optional<double> lowest_instant;
  std::optional<double> highest_instant;
  double lowest_uncertainty = 0;
  double highest_uncertainty = 0;
  std::size_t contributors = 0;
  for (const StreamFrame& row : rows) {
    if (!row.chosen.has_value()) {
      continue;
    }
    ++contributors;
    const double uncertainty =
        row.uncertainty_ms == nullptr ? 0 : row.uncertainty_ms->number_or(0);
    const double instant = row.chosen->effective_ns;
    if (!lowest_instant.has_value() || instant < *lowest_instant) {
      lowest_instant = instant;
      lowest_uncertainty = uncertainty;
    }
    if (!highest_instant.has_value() || instant > *highest_instant) {
      highest_instant = instant;
      highest_uncertainty = uncertainty;
    }
  }
  // One instant has no spread, and reporting zero would let a frame with a single working stream
  // look perfectly synchronised — the presentation §25.1 is written against.
  if (contributors >= 2 && lowest_instant.has_value() && highest_instant.has_value()) {
    const double measured = milliseconds(*highest_instant - *lowest_instant);
    frame.skew_ms = measured;
    if (frame.undeclared_latency.empty()) {
      frame.skew_upper_ms = measured + lowest_uncertainty + highest_uncertainty;
    }
    // Otherwise the widening is unbounded: §47.2's "larger uncertainty" has no largest.
  }

  // §25.1: exceeding a staleness budget "is a hold, not a silently older frame". So is an empty
  // stream. Everything else — an undeclared latency, an undeclared budget, an unbounded skew — is
  // reported and does not block, because none of them is evidence that the frame is wrong. They are
  // evidence that nobody can say how wrong it is, a different sentence.
  frame.deliverable = holds.empty();
  frame.verdict = holds.empty() ? "ok" : "hold";

  // The shapes, assembled here so the frame owns its own rendering and the two corpora that read it
  // cannot drift.
  frame.streams = Json::array();
  Json record_streams = Json::array();
  double streams_absent = 0;
  for (const StreamFrame& row : rows) {
    Json entry = Json::object();
    entry.set("name", Json::string(row.name));
    entry.set("modality", Json::string(row.modality));
    entry.set("latency_ms", row.latency_ms == nullptr ? Json::null() : *row.latency_ms);
    entry.set("uncertainty_ms", row.uncertainty_ms == nullptr ? Json::null() : *row.uncertainty_ms);
    entry.set("budget_ms", row.budget_ms == nullptr ? Json::null() : *row.budget_ms);
    entry.set("age_ms",
              row.chosen.has_value() ? Json::number(row.chosen->age_ms) : Json::null());
    entry.set("considered",
              Json::number(row.chosen.has_value()
                               ? static_cast<double>(row.chosen->considered)
                               : 0));
    entry.set("over_budget",
              Json::boolean(row.chosen.has_value() && row.chosen->over_budget));
    entry.set("seq", row.chosen.has_value() ? *row.chosen->seq : Json::null());
    entry.set("stamp_ns",
              row.chosen.has_value() ? Json::number(row.chosen->stamp_ns) : Json::null());
    frame.streams.push(entry);

    if (!row.chosen.has_value()) {
      // Distinct from the record's `streams_omitted`, which is "streams the record could not fit" —
      // a limit of the format. This is a limit of the robot, and an investigation reading one as
      // the other would look for a bigger record instead of a dead camera.
      streams_absent += 1;
      continue;
    }
    Json recorded = Json::object();
    recorded.set("name", Json::string(row.name));
    // The record's field is unsigned, so a chosen sample from *after* the target instant saturates
    // at zero. It cannot arise on a live robot — every target a runtime picks is at or before now,
    // and CLOCK_MONOTONIC_RAW does not step — which is why the saturation is acceptable rather than
    // merely convenient. The signed figure stays on the frame.
    recorded.set("age_us", Json::number(micros(row.chosen->age_ms)));
    recorded.set("stale", Json::boolean(row.chosen->over_budget));
    record_streams.push(recorded);
  }

  frame.holds = Json::array();
  for (const Hold& one : holds) {
    Json entry = Json::object();
    entry.set("stream", Json::string(one.stream));
    entry.set("reason", Json::string(one.reason));
    entry.set("detail", Json::string(one.detail));
    frame.holds.push(entry);
  }

  frame.record = Json::object();
  frame.record.set("target_ns", Json::number(target_ns));
  // Null when the skew is not measurable, because zero would make a frame with one working stream
  // and three dead capture paths read, months later, as perfectly synchronised.
  frame.record.set("worst_skew_us",
                   frame.skew_ms.has_value() ? Json::number(micros(*frame.skew_ms)) : Json::null());
  frame.record.set("streams_absent", Json::number(streams_absent));
  frame.record.set("streams", record_streams);

  return frame;
}

std::optional<double> latest_common_instant(double now_ns, const Json& streams) {
  // A live control loop has to choose a target, and choosing badly defeats the whole section: aim
  // at *now* and the slowest camera is always the stalest stream in every frame, so every frame is
  // skewed by that camera's latency and the assembler reports a problem the loop created.
  //
  // Offered rather than imposed: a runtime may target an older instant deliberately. What this
  // exists to prevent is two editions inventing two different defaults and disagreeing about frames
  // neither of them assembled wrongly.
  std::optional<double> earliest;

  for (const Json& stream : streams.items()) {
    const Json* latency = nullish(stream, "latency_ms");
    std::optional<double> latest;
    for (const Json& sample : stream["samples"].items()) {
      const double effective = effective_ns(sample["stamp_ns"].number_or(0), latency);
      if (effective > now_ns) {
        continue;
      }
      if (!latest.has_value() || effective > *latest) {
        latest = effective;
      }
    }
    // A stream with nothing at or before `now` cannot contribute to any frame at or before `now`,
    // so there is no common instant at all — not a later one, and not one that quietly drops it.
    if (!latest.has_value()) {
      return std::nullopt;
    }
    if (!earliest.has_value() || *latest < *earliest) {
      earliest = latest;
    }
  }

  // No streams at all: every instant is common to all of them.
  const double answer = earliest.has_value() ? *earliest : now_ns;
  // A declared latency larger than the stamp itself. The correction is still right and the instant
  // it names is before the monotonic clock started, which no `u64` can express.
  return answer < 0 ? std::nullopt : std::optional<double>(answer);
}

}  // namespace skynet::observation

namespace skynet::contracts {

Json observation(const std::string& label, const Json& input) {
  if (label.find("target") != std::string::npos) {
    const std::optional<double> instant =
        skynet::observation::latest_common_instant(input["now_ns"].number_or(0), input["streams"]);
    Json out = Json::object();
    out.set("target_ns", instant.has_value() ? Json::number(*instant) : Json::null());
    return out;
  }

  const skynet::observation::Frame frame =
      skynet::observation::assemble(input["target_ns"].number_or(0), input["streams"]);

  Json out = Json::object();
  out.set("target_ns", Json::number(frame.target_ns));
  out.set("streams", frame.streams);
  out.set("skew_ms", frame.skew_ms.has_value() ? Json::number(*frame.skew_ms) : Json::null());
  out.set("skew_upper_ms",
          frame.skew_upper_ms.has_value() ? Json::number(*frame.skew_upper_ms) : Json::null());
  Json undeclared_latency = Json::array();
  for (const std::string& name : frame.undeclared_latency) {
    undeclared_latency.push(Json::string(name));
  }
  out.set("undeclared_latency", undeclared_latency);
  Json undeclared_budget = Json::array();
  for (const std::string& name : frame.undeclared_budget) {
    undeclared_budget.push(Json::string(name));
  }
  out.set("undeclared_budget", undeclared_budget);
  out.set("holds", frame.holds);
  out.set("verdict", Json::string(frame.verdict));
  out.set("deliverable", Json::boolean(frame.deliverable));
  // The projection onto §26's record, inside the frame rather than beside it. It is derived from
  // everything above, and pinning it in a second corpus would let an edition agree about a frame
  // and disagree about the only version of it that survives the day.
  out.set("record", frame.record);
  return out;
}

}  // namespace skynet::contracts

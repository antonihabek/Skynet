// §74 — telemetry, as windows rather than samples. C++ edition.
//
// Transcribed from `reference/typescript/telemetry.ts`. Every rule and message string is read from
// that file, never from `corpus/`.
//
// ── A window, not a sample and not a total ─────────────────────────────────
//
// An item covers a **half-open interval** and carries counts over it. The three obvious
// alternatives each break a rule that already exists:
//
//   * **A series of samples** — a week offline produces ten thousand items, which is the state
//     §34.4 collapses check-ins to avoid.
//   * **A running total since boot** — a drop becomes invisible, because the next total absorbs it
//     and the series stays tidy. That is precisely what §47.4 forbids.
//   * **Counts since the last successful send** — makes delivery load-bearing for correctness, and
//     a retry that succeeded twice would double-count with no way for the robot to know.
//
// Counts over a self-describing window have none of those properties, and they buy §47.4's rule
// from the *shape of the data*: dropping one loses that window and nothing else, and the loss is
// visible because the windows no longer tile.
//
// ── Two clocks, and both of them ───────────────────────────────────────────
//
// §35.1 assigns them: `CLOCK_REALTIME` is for *"correlation with the Hub"* and
// `CLOCK_MONOTONIC_RAW` is for anything measured. A window does both jobs, so it carries both.
// Their disagreement is **reported rather than reconciled**: the skew is a fact about the robot and
// not a reason to discard the counts.
//
// ── Where C++ is quietly not JavaScript, here ──────────────────────────────
//
// `Math.trunc(skew / 1000)`
//     Truncation toward zero, and the reference's own comment says why it is not `Math.round`: that
//     is half toward positive infinity, the Rust helper is half away from zero, and the two
//     disagree on exactly the negative halves a backwards clock produces. An edition that reached
//     for a flooring division would put the disagreement back, one second off, in a sentence on a
//     fleet page.
//
// `Number.isFinite`
//     A count of `true` is not a count of one.
//
// `count === null || !Number.isFinite(count) || count < 0`
//     Three ways to be unmeasured and one of them is a **negative** count, which is a counter that
//     wrapped or a field somebody defaulted to `-1`. It lands in `unmeasured` rather than in a rate,
//     because a negative rate on a fleet page is a number nobody can read.

#include "skynet_os/contracts.hpp"
#include "skynet_os/js.hpp"
#include "skynet_os/vocabulary.hpp"

#include <algorithm>
#include <cmath>
#include <string>
#include <vector>

namespace skynet::contracts {
namespace {

/// `typeof value === 'number' && Number.isFinite(value)`.
bool finite(const Json& value) {
  return value.is_number() && std::isfinite(value.number_or(0));
}

Json fault(std::string code, std::string severity, std::string message) {
  Json out = Json::object();
  out.set("code", Json::string(std::move(code)));
  out.set("message", Json::string(std::move(message)));
  out.set("severity", Json::string(std::move(severity)));
  return out;
}

struct WindowReport {
  bool usable = true;
  double elapsed_ms = 0;
  double skew_ms = 0;
  Json rates = Json::object();
  std::vector<std::string> unmeasured;
  Json faults = Json::array();
};

/// What one window says, and whether it says anything (§74.2, §74.3).
///
/// Every check runs and the result carries all of them, for admission's reason: a fleet operator
/// looking at a robot whose telemetry is being refused should get one list rather than one problem
/// per window.
///
/// **An unusable window is refused rather than partly counted.** A window with no duration cannot
/// produce a rate, and a window whose bounds are inverted is a robot whose clock moved under it.
/// Counting either would put a number on a fleet page that is not a rate of anything — and the
/// counts are still on the record, because the *item* is kept whether or not the arithmetic is.
WindowReport assess_window(const Json& window) {
  WindowReport report;
  const Json& from_ms = window["from_ms"];
  const Json& to_ms = window["to_ms"];
  const Json& elapsed = window["elapsed_ms"];

  const bool bounded = finite(from_ms) && finite(to_ms);
  if (!bounded) {
    report.faults.push(fault(
        "unbounded", "error",
        "This window does not say when it was. §35.1 makes wall time the clock a Hub correlates "
        "on, so a window without both bounds cannot be placed beside another robot's — and the gap "
        "between windows is how a dropped one is noticed at all (§34.4, §47.4)."));
  } else if (to_ms.number_or(0) < from_ms.number_or(0)) {
    report.faults.push(fault(
        "inverted", "error",
        "This window ends before it starts. That is a clock that stepped backwards mid-window "
        "rather than a counting error, and the counts in it are over an interval nobody can name."));
  }

  const bool measured = finite(elapsed) && elapsed.number_or(0) > 0;
  if (!measured) {
    report.faults.push(fault(
        "unmeasured_duration", "error",
        "This window carries no monotonic duration, so there is nothing to divide the counts by. "
        "§35.1 keeps rates off wall time on purpose: an NTP step would otherwise turn four hundred "
        "guard trips into a plausible number and nothing would look wrong."));
  }

  const double span = bounded ? to_ms.number_or(0) - from_ms.number_or(0) : 0;
  const double skew = (bounded && measured) ? span - elapsed.number_or(0) : 0;

  // A minute of disagreement over a five-minute window is a machine that was suspended, an NTP
  // correction, or a clock nobody set. It is a fact about the robot and not a reason to throw the
  // counts away, so it is a warning with the number in it.
  if (bounded && measured && (skew > 60000 || skew < -60000)) {
    report.faults.push(fault(
        "implausible_duration", "warning",
        "Wall time and the monotonic clock disagree by " + js::number_to_string(std::trunc(skew / 1000)) +
            " s over this window. The counts are kept and the rates are computed from the monotonic "
            "duration (§35.1); the disagreement is worth one line on a fleet page, because it is "
            "what a suspended machine and a corrected clock both look like."));
  }

  const Json& counters = window["counters"];
  for (const std::string_view name_view : vocabulary::members("telemetry_counters")) {
    const std::string name(name_view);
    const Json& count = counters[name];
    if (!finite(count) || count.number_or(0) < 0) {
      report.unmeasured.push_back(name);
      continue;
    }
    if (measured) {
      report.rates.set(name, Json::number(count.number_or(0) / (elapsed.number_or(0) / 1000)));
    }
  }
  std::sort(report.unmeasured.begin(), report.unmeasured.end(),
            [](const std::string& left, const std::string& right) {
              return js::utf16_less(left, right);
            });

  for (const Json& entry : report.faults.items()) {
    if (entry["severity"].string_or("") == "error") {
      report.usable = false;
    }
  }
  report.elapsed_ms = measured ? elapsed.number_or(0) : 0;
  report.skew_ms = skew;
  return report;
}

Json string_array(const std::vector<std::string>& values) {
  Json out = Json::array();
  for (const std::string& value : values) {
    out.push(Json::string(value));
  }
  return out;
}

}  // namespace

Json telemetry(const std::string& label, const Json& input) {
  if (label.find("window") != std::string::npos) {
    // `rates` is an object rather than a list, and the counters that were *not* measured are absent
    // from it rather than present as zero — which is the whole of §74.3 in one shape. A consumer
    // iterating the counters and reading `rates[name] or 0` would be committing the mistake the
    // section exists to prevent; one that reads `unmeasured` first cannot.
    const WindowReport report = assess_window(input);
    Json out = Json::object();
    out.set("elapsed_ms", Json::number(report.elapsed_ms));
    out.set("faults", report.faults);
    out.set("rates", report.rates);
    out.set("skew_ms", Json::number(report.skew_ms));
    out.set("unmeasured", string_array(report.unmeasured));
    out.set("usable", Json::boolean(report.usable));
    return out;
  }

  // What a set of windows actually covers (§74.1, §74.5).
  //
  // This is the function §15's field conformance has to be computed through, and the reason is one
  // line: **the denominator is the time that was observed, not the time that elapsed.** A robot
  // that sent windows for one hour out of a day has not demonstrated anything about the other
  // twenty-three, and a Hub that divided by twenty-four hours would report a conformance figure
  // that is arithmetic over a period it did not observe.
  //
  // Overlapping windows are merged rather than summed for the purpose of the span, and their counts
  // are summed anyway — an overlap is a robot that restarted and re-reported, and the honest
  // reading of two counts over overlapping intervals is that both events happened.
  struct Usable {
    double from = 0;
    double to = 0;
    const Json* window = nullptr;
  };
  std::vector<Usable> usable;
  double unusable = 0;
  double observed = 0;

  for (const Json& window : input["windows"].items()) {
    const WindowReport report = assess_window(window);
    if (!report.usable) {
      unusable += 1;
      continue;
    }
    observed += report.elapsed_ms;
    usable.push_back({window["from_ms"].number_or(0), window["to_ms"].number_or(0), &window});
  }

  Json totals = Json::object();
  std::vector<std::string> measured_names;
  for (const Usable& entry : usable) {
    const Json& counters = (*entry.window)["counters"];
    for (const std::string_view name_view : vocabulary::members("telemetry_counters")) {
      const std::string name(name_view);
      const Json& count = counters[name];
      if (!finite(count) || count.number_or(0) < 0) {
        continue;
      }
      const double running = totals.has(name) ? totals[name].number_or(0) : 0;
      totals.set(name, Json::number(running + count.number_or(0)));
      if (std::find(measured_names.begin(), measured_names.end(), name) == measured_names.end()) {
        measured_names.push_back(name);
      }
    }
  }

  // A stable sort on `from`, as `Array.prototype.sort` is: two windows starting at the same instant
  // keep the order they were reported in, which is the order a robot wrote them.
  std::stable_sort(usable.begin(), usable.end(),
                   [](const Usable& left, const Usable& right) { return left.from < right.from; });
  Json gaps = Json::array();
  double spanned = 0;
  if (!usable.empty()) {
    double reach = usable.front().to;
    for (std::size_t index = 1; index < usable.size(); ++index) {
      if (usable[index].from > reach) {
        Json gap = Json::object();
        gap.set("from_ms", Json::number(reach));
        gap.set("to_ms", Json::number(usable[index].from));
        gaps.push(gap);
      }
      reach = std::max(reach, usable[index].to);
    }
    // The last window may end before the latest end seen, when windows overlap out of order.
    spanned = std::max(0.0, reach - usable.front().from);
  }

  std::vector<std::string> unmeasured;
  for (const std::string_view name_view : vocabulary::members("telemetry_counters")) {
    const std::string name(name_view);
    if (std::find(measured_names.begin(), measured_names.end(), name) == measured_names.end()) {
      unmeasured.push_back(name);
    }
  }
  std::sort(unmeasured.begin(), unmeasured.end(),
            [](const std::string& left, const std::string& right) {
              return js::utf16_less(left, right);
            });

  // §15's field conformance, over what was actually observed. Null when nothing counted cycles,
  // which is not the same as 1 and is what the award takes for *"no fleet data exists yet"*: a Hub
  // that substituted 1 here would be awarding a certified tier on the strength of a robot that has
  // never reported.
  Json conformance = Json::null();
  if (totals.has("cycles") && finite(totals["cycles"])) {
    const double cycles = totals["cycles"].number_or(0);
    // Absent interventions read as zero *only once there are cycles*, and that is not the usual
    // rule. §74.3 keeps unmeasured and zero apart everywhere else — but a window that counted
    // cycles and did not count interventions is a robot whose kernel intervened in none of them,
    // because the counter that would have recorded one is the counter that recorded the cycles.
    const double interventions = totals.has("interventions") ? totals["interventions"].number_or(0) : 0;
    if (cycles > 0) {
      // Clamped rather than trusted. More interventions than cycles is a robot whose counters
      // disagree with each other, and the honest floor for that is zero rather than a negative
      // figure §15 has no reading for. Above 1 is the dangerous direction and must never appear.
      conformance = Json::number(std::max(0.0, std::min(1.0, (cycles - interventions) / cycles)));
    }
  }

  Json out = Json::object();
  out.set("conformance", conformance);
  // `gaps` is the field this shape exists for. §47.4's rule — a gap annotated as a gap is honest, a
  // gap that looks like a quiet period is not — is obtained here from the data rather than from an
  // annotation, and a shape that dropped the gaps would leave a fleet page unable to tell a robot
  // that reported nothing from one whose telemetry was discarded under §34.4's drop policy.
  out.set("gaps", gaps);
  out.set("observed_ms", Json::number(observed));
  out.set("spanned_ms", Json::number(spanned));
  out.set("totals", totals);
  out.set("unmeasured", string_array(unmeasured));
  out.set("unusable", Json::number(unusable));
  return out;
}

}  // namespace skynet::contracts

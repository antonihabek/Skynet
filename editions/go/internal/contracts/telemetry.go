package contracts

import (
	"math"
	"sort"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// Telemetry windows (§74).
//
// # Two clocks, and the reason there are two
//
// Wall time places a window so a Hub can correlate it with another robot's; monotonic time measures
// how long it actually lasted. Using wall time for the *duration* would let an NTP step change a
// rate — a clock that jumps forward five minutes mid-window turns four hundred guard trips into a
// plausible number instead of an alarming one, and nothing anywhere would look wrong. Using
// monotonic time for the *bounds* would make two robots' windows incomparable. Their disagreement
// is **reported rather than reconciled**.
//
// # `null` is not zero
//
// §74.3, and it has more force here than anywhere: "no measurement of guard trips" and "no guard
// trips" render identically as a zero, and the second is the number an operator relaxes on.

// WindowFault is one thing wrong with a telemetry window.
type WindowFault struct {
	Code     string
	Severity string
	Message  string
}

// WindowReport is what one window says, and whether it says anything.
type WindowReport struct {
	Usable    bool
	ElapsedMS float64
	// SkewMS is `(to − from) − elapsed`. Positive is wall time running ahead of the robot's own
	// measurement — a forward NTP step, or a machine that was suspended. Reported, never corrected.
	SkewMS float64
	// Rates are per-second, for every counter that was measured. Held as parallel slices in
	// declaration order rather than a map, because the writer sorts keys and the *set* of names is
	// what varies; a Go map would also randomise any future iteration of it.
	RateNames  []string
	RateValues []float64
	// Unmeasured are counters nobody measured, sorted. Not the same set as the ones that were zero.
	Unmeasured []string
	Faults     []WindowFault
}

// finiteNumber is the reference's `finite()`: a number, and a finite one. JSON cannot spell a
// non-finite value, so the second half is unreachable from a corpus case and is written anyway —
// this function is also called on totals the edition computed itself.
func finiteNumber(value canonical.Value) (float64, bool) {
	number, ok := value.Number()
	return number, ok && canonical.Finite(number)
}

// AssessWindow is §74.2 and §74.3.
//
// Every check runs and the result carries all of them, for admitPackage's reason: a fleet operator
// looking at a robot whose telemetry is being refused should get one list rather than one problem
// per window.
//
// An unusable window is refused rather than partly counted. A window with no duration cannot
// produce a rate, and one whose bounds are inverted is a robot whose clock moved under it. Counting
// either would put a number on a fleet page that is not a rate of anything.
func AssessWindow(window canonical.Value) WindowReport {
	faults := []WindowFault{}
	from, hasFrom := finiteNumber(window.Field("from_ms"))
	to, hasTo := finiteNumber(window.Field("to_ms"))
	elapsed, hasElapsed := finiteNumber(window.Field("elapsed_ms"))

	bounded := hasFrom && hasTo
	if !bounded {
		faults = append(faults, WindowFault{
			Code: "unbounded", Severity: "error",
			Message: "This window does not say when it was. §35.1 makes wall time the clock a Hub correlates on, " +
				"so a window without both bounds cannot be placed beside another robot's — and the gap " +
				"between windows is how a dropped one is noticed at all (§34.4, §47.4).",
		})
	} else if to < from {
		faults = append(faults, WindowFault{
			Code: "inverted", Severity: "error",
			Message: "This window ends before it starts. That is a clock that stepped backwards mid-window rather " +
				"than a counting error, and the counts in it are over an interval nobody can name.",
		})
	}

	measured := hasElapsed && elapsed > 0
	if !measured {
		faults = append(faults, WindowFault{
			Code: "unmeasured_duration", Severity: "error",
			Message: "This window carries no monotonic duration, so there is nothing to divide the counts by. " +
				"§35.1 keeps rates off wall time on purpose: an NTP step would otherwise turn four hundred " +
				"guard trips into a plausible number and nothing would look wrong.",
		})
	}

	span := 0.0
	if bounded {
		span = to - from
	}
	skew := 0.0
	if bounded && measured {
		skew = span - elapsed
	}

	// A minute of disagreement over a five-minute window is a machine that was suspended, an NTP
	// correction, or a clock nobody set. It is a fact about the robot and not a reason to throw the
	// counts away, so it is a warning with the number in it.
	//
	// Rendered with truncation toward zero and **not** with a rounding: js.Round is half toward +∞
	// and the Rust edition's helper is half away from zero, and the two disagree on exactly the
	// negative halves a backwards clock produces. Truncation is the one reading every edition can
	// spell identically without a shared helper.
	if bounded && measured && (skew > 60000 || skew < -60000) {
		faults = append(faults, WindowFault{
			Code: "implausible_duration", Severity: "warning",
			Message: "Wall time and the monotonic clock disagree by " + js.NumberToString(math.Trunc(skew/1000)) +
				" s over this window. " +
				"The counts are kept and the rates are computed from the monotonic duration (§35.1); the " +
				"disagreement is worth one line on a fleet page, because it is what a suspended machine and " +
				"a corrected clock both look like.",
		})
	}

	report := WindowReport{ElapsedMS: 0, SkewMS: skew, Faults: faults}
	if measured {
		report.ElapsedMS = elapsed
	}
	counters := window.Field("counters")
	unmeasured := []string{}
	for _, name := range vocabulary.TelemetryCounters {
		count, isCount := finiteNumber(counters.Field(name))
		if !isCount || count < 0 {
			unmeasured = append(unmeasured, name)
			continue
		}
		if measured {
			report.RateNames = append(report.RateNames, name)
			report.RateValues = append(report.RateValues, count/(elapsed/1000))
		}
	}
	sort.Strings(unmeasured)
	report.Unmeasured = unmeasured

	report.Usable = true
	for _, entry := range faults {
		if entry.Severity == "error" {
			report.Usable = false
		}
	}
	return report
}

// Gap is an interval inside a coverage span that no window covers.
type Gap struct {
	FromMS float64
	ToMS   float64
}

// Coverage is what a set of windows actually covers (§74.1, §74.5).
type Coverage struct {
	ObservedMS  float64
	SpannedMS   float64
	Gaps        []Gap
	Unusable    float64
	TotalNames  []string
	TotalValues []float64
	Unmeasured  []string
}

// CoverageOf is the function §15's field conformance has to be computed through, and the reason is
// one line: **the denominator is the time that was observed, not the time that elapsed.**
//
// A robot that sent windows for one hour out of a day has not demonstrated anything about the other
// twenty-three, and a Hub that divided by twenty-four hours would report a conformance figure that
// is arithmetic over a period it did not observe. §15 puts a floor of 0.999 on that figure and
// suspends an award below it, so the difference between the two denominators is the difference
// between suspending a robot and not.
//
// Overlapping windows are merged rather than summed for the purpose of the span, and their counts
// are summed anyway — an overlap is a robot that restarted and re-reported, and the honest reading
// of two counts over overlapping intervals is that both events happened.
func CoverageOf(windows []canonical.Value) Coverage {
	type usableWindow struct {
		from, to float64
		window   canonical.Value
	}
	usable := []usableWindow{}
	coverage := Coverage{}

	for _, window := range windows {
		report := AssessWindow(window)
		if !report.Usable {
			coverage.Unusable++
			continue
		}
		coverage.ObservedMS += report.ElapsedMS
		from, _ := window.Field("from_ms").Number()
		to, _ := window.Field("to_ms").Number()
		usable = append(usable, usableWindow{from: from, to: to, window: window})
	}

	// Totals in declaration order, so the names accumulate the way the reference's object does.
	totals := map[string]float64{}
	measured := map[string]bool{}
	for _, entry := range usable {
		counters := entry.window.Field("counters")
		for _, name := range vocabulary.TelemetryCounters {
			count, isCount := finiteNumber(counters.Field(name))
			if !isCount || count < 0 {
				continue
			}
			totals[name] += count
			measured[name] = true
		}
	}
	for _, name := range vocabulary.TelemetryCounters {
		if measured[name] {
			coverage.TotalNames = append(coverage.TotalNames, name)
			coverage.TotalValues = append(coverage.TotalValues, totals[name])
		}
	}

	sorted := make([]usableWindow, len(usable))
	copy(sorted, usable)
	sort.SliceStable(sorted, func(left, right int) bool { return sorted[left].from < sorted[right].from })

	coverage.Gaps = []Gap{}
	if len(sorted) > 0 {
		reach := sorted[0].to
		for _, entry := range sorted[1:] {
			if entry.from > reach {
				coverage.Gaps = append(coverage.Gaps, Gap{FromMS: reach, ToMS: entry.from})
			}
			reach = math.Max(reach, entry.to)
		}
		// The last window may end before the latest end seen, when windows overlap out of order.
		coverage.SpannedMS = math.Max(0, reach-sorted[0].from)
	}

	coverage.Unmeasured = []string{}
	for _, name := range vocabulary.TelemetryCounters {
		if !measured[name] {
			coverage.Unmeasured = append(coverage.Unmeasured, name)
		}
	}
	sort.Strings(coverage.Unmeasured)
	return coverage
}

// FieldConformance is §15's number.
//
// Clamped rather than trusted. More interventions than cycles is a robot whose counters disagree
// with each other, and the honest floor for that is zero rather than a negative conformance figure
// §15 has no reading for — which matters because §15 checks this against a floor, and a negative one
// sails under it into `suspended` for the right reason by accident. Above 1 is the dangerous
// direction and is the one this must never produce.
func FieldConformance(cycles, interventions float64) (float64, bool) {
	if !canonical.Finite(cycles) || cycles <= 0 {
		return 0, false
	}
	return math.Max(0, math.Min(1, (cycles-interventions)/cycles)), true
}

// ConformanceOfCoverage is §15's field conformance over what was actually observed.
//
// Absent when nothing counted cycles, which is not the same as 1 and is what the award takes for
// *"no fleet data exists yet"*. A Hub that substituted 1 here would be awarding a certified tier on
// the strength of a robot that has never reported.
func ConformanceOfCoverage(coverage Coverage) (float64, bool) {
	cycles, hasCycles := totalOf(coverage, "cycles")
	if !hasCycles || !canonical.Finite(cycles) {
		return 0, false
	}
	// Absent interventions read as zero *only once there are cycles*, and that is not the usual
	// rule. §74.3 keeps unmeasured and zero apart everywhere else — but a window that counted
	// cycles and did not count interventions is a robot whose kernel intervened in none of them,
	// because the counter that would have recorded one is the counter that recorded the cycles.
	interventions, _ := totalOf(coverage, "interventions")
	return FieldConformance(cycles, interventions)
}

func totalOf(coverage Coverage, name string) (float64, bool) {
	for at, candidate := range coverage.TotalNames {
		if candidate == name {
			return coverage.TotalValues[at], true
		}
	}
	return 0, false
}

// ── the corpus ──────────────────────────────────────────────────────────────

func namedNumbers(names []string, values []float64) canonical.Value {
	out := canonical.Object()
	for at, name := range names {
		out = out.Set(name, canonical.Number(values[at]))
	}
	return out
}

func telemetryWindow(input canonical.Value) (canonical.Value, error) {
	// The case's input *is* the window — `telemetry/coverage` wraps its windows in a field and this
	// one does not, which is a difference worth one line rather than a shared reader that has to
	// guess which shape it was handed.
	report := AssessWindow(input)
	faults := canonical.Array()
	for _, fault := range report.Faults {
		faults = faults.Push(canonical.Object().
			Set("code", canonical.String(fault.Code)).
			Set("message", canonical.String(fault.Message)).
			Set("severity", canonical.String(fault.Severity)))
	}
	return canonical.Object().
		Set("elapsed_ms", canonical.Number(report.ElapsedMS)).
		Set("faults", faults).
		Set("rates", namedNumbers(report.RateNames, report.RateValues)).
		Set("skew_ms", canonical.Number(report.SkewMS)).
		Set("unmeasured", canonical.Strings(report.Unmeasured)).
		Set("usable", canonical.Bool(report.Usable)), nil
}

func telemetryCoverage(input canonical.Value) (canonical.Value, error) {
	coverage := CoverageOf(input.Field("windows").Items())
	gaps := canonical.Array()
	for _, gap := range coverage.Gaps {
		gaps = gaps.Push(canonical.Object().
			Set("from_ms", canonical.Number(gap.FromMS)).
			Set("to_ms", canonical.Number(gap.ToMS)))
	}
	conformance, hasConformance := ConformanceOfCoverage(coverage)
	return canonical.Object().
		Set("conformance", optionalNumber(conformance, hasConformance)).
		Set("gaps", gaps).
		Set("observed_ms", canonical.Number(coverage.ObservedMS)).
		Set("spanned_ms", canonical.Number(coverage.SpannedMS)).
		Set("totals", namedNumbers(coverage.TotalNames, coverage.TotalValues)).
		Set("unmeasured", canonical.Strings(coverage.Unmeasured)).
		Set("unusable", canonical.Number(coverage.Unusable)), nil
}

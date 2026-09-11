package contracts

import (
	"sort"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// The check-in (§17, §33).
//
// Pure: no clock, no network. Ages arrive as durations rather than as timestamps, for §35.2's
// reason.

// ClaimVerdict is what this robot is entitled to claim.
type ClaimVerdict struct {
	Claim string
	// Applied is §17's field. True for `applied` and for nothing else.
	Applied bool
	// Generation is what this check-in reports having applied. **Never the desired one** — echoing
	// the desired generation would mark every deployment applied the moment the robot heard about
	// it, which is precisely the claim the Hub is forbidden from making.
	Generation float64
	Detail     string
	// Blocking is the stages with work left, sorted and deduplicated. Empty on `applied`.
	Blocking []string
}

// ClaimFor is §17, §33.3 and §33.4.
//
// The order of the tests is the order of severity, and the last is the interesting one. An **empty
// plan with disagreeing generations** is a reconciler that computed a plan closing nothing: the
// robot is not converged and nothing is going to converge it. Reporting that as `applied` would
// hide a supervisor bug behind a green tick on a fleet console, which is the one place nobody would
// go looking for it. It gets a name.
func ClaimFor(progress canonical.Value) ClaimVerdict {
	observed := numberOr(progress.Field("observed_generation"), 0)
	desired := numberOr(progress.Field("desired_generation"), 0)
	pending := progress.Field("pending").Items()

	// Deduplicated and sorted, which is what `[...new Set(...)].sort()` does — and the sort is by
	// UTF-16 code unit there and by UTF-8 byte here. Stage names are ASCII and the two agree; a
	// wider vocabulary would need utf16Less, as digest.go's path sort does.
	seen := map[string]bool{}
	blocking := []string{}
	for _, entry := range pending {
		stage, _ := entry.Field("stage").Str()
		if seen[stage] {
			continue
		}
		seen[stage] = true
		blocking = append(blocking, stage)
	}
	sort.Strings(blocking)

	if len(pending) > 0 {
		deferred := true
		for _, entry := range pending {
			if !isTrue(entry.Field("deferred")) {
				deferred = false
			}
		}
		if deferred {
			return ClaimVerdict{
				Claim: "deferred", Applied: false, Generation: observed,
				Detail: "Everything left to do is waiting for a safe state (§33.4): " +
					strings.Join(blocking, ", ") + ". " +
					"Nothing here converges until the robot is put in one, which is an operator action " +
					"rather than a matter of time.",
				Blocking: blocking,
			}
		}
		return ClaimVerdict{
			Claim: "converging", Applied: false, Generation: observed,
			Detail: "Still reconciling generation " + js.NumberToString(desired) + ", with work left at " +
				strings.Join(blocking, ", ") + ". This robot is running generation " +
				js.NumberToString(observed) + ".",
			Blocking: blocking,
		}
	}

	if observed != desired {
		return ClaimVerdict{
			Claim: "inconsistent", Applied: false, Generation: observed,
			Detail: "The plan is empty and this robot has applied generation " + js.NumberToString(observed) +
				" against a desired " + js.NumberToString(desired) + ". An empty plan that closes nothing is " +
				"a supervisor that cannot see the difference it is meant to close, and reporting it as " +
				"applied would put a green tick over it.",
			Blocking: blocking,
		}
	}

	return ClaimVerdict{
		Claim: "applied", Applied: true, Generation: observed,
		Detail:   "Generation " + js.NumberToString(observed) + " is applied and the plan is empty.",
		Blocking: blocking,
	}
}

// PackageHealthEntry is one package's reading, with every default filled in.
type PackageHealthEntry struct {
	ID        string
	Live      bool
	Readiness string
	AgeMS     float64
}

// HealthField is one of §33.6's five named aggregates, dated.
type HealthField struct {
	Value   float64
	AgeMS   float64
	Present bool
}

// HealthReport is §33.6 rolled up for a check-in.
type HealthReport struct {
	// Degraded is whether anything here should stop this robot being handed new work.
	Degraded bool
	// Stalled are packages that are alive and producing nothing — the finding §33.6 exists for.
	Stalled []string
	// Unmeasured are fields nobody measured, sorted.
	Unmeasured []string
	// Undated are fields that carried a value and no age, sorted. Reported as unmeasured, and named
	// apart: "nobody measured this" and "somebody measured this and lost track of when" send
	// different people to different places.
	Undated  []string
	Packages []PackageHealthEntry
	Fields   []HealthField
}

// SummarizeHealth rolls health up for a check-in (§33.6).
//
// Three rules, and the second is the one this function was written for:
//
//   - **A stalled package is degraded and an unmeasured field is not.** "We could not measure the
//     battery" is not "the battery is fine", and it is also not "the robot is unhealthy" — it is a
//     gap, and reporting a gap as a fault is how an operator learns to ignore the health screen.
//   - **A value with no age is dropped and named.** §33.6 requires an age on every field, so a
//     reading that cannot be dated is reported as unmeasured. Passing the number through would be
//     worse than dropping it: a console renders whatever it is given as current.
//   - **`unknown` readiness is not a stall.** A policy in its first cycle has legitimately produced
//     nothing, and reporting that as a fault would make every start look like one.
func SummarizeHealth(health canonical.Value) HealthReport {
	report := HealthReport{Packages: []PackageHealthEntry{}, Stalled: []string{}}

	for _, entry := range health.Field("packages").Items() {
		id, _ := entry.Field("id").Str()
		readiness := stringOr(entry.Field("readiness"), "unknown")
		report.Packages = append(report.Packages, PackageHealthEntry{
			ID:        id,
			Live:      isTrue(entry.Field("live")),
			Readiness: readiness,
			AgeMS:     numberOr(entry.Field("age_ms"), 0),
		})
		if readiness == "stalled" {
			report.Stalled = append(report.Stalled, id)
		}
	}
	sort.Strings(report.Stalled)

	report.Unmeasured = []string{}
	report.Undated = []string{}
	report.Fields = make([]HealthField, len(vocabulary.HealthFields))
	for at, name := range vocabulary.HealthFields {
		measured := health.Field(name)
		value, hasValue := finiteNumber(measured.Field("value"))
		if !hasValue {
			report.Unmeasured = append(report.Unmeasured, name)
			continue
		}
		age, hasAge := finiteNumber(measured.Field("age_ms"))
		if !hasAge || age < 0 {
			report.Undated = append(report.Undated, name)
			continue
		}
		report.Fields[at] = HealthField{Value: value, AgeMS: age, Present: true}
	}
	sort.Strings(report.Unmeasured)
	sort.Strings(report.Undated)

	report.Degraded = len(report.Stalled) > 0
	return report
}

// ── the corpus ──────────────────────────────────────────────────────────────

func checkinClaim(input canonical.Value) (canonical.Value, error) {
	verdict := ClaimFor(input)
	return canonical.Object().
		Set("applied", canonical.Bool(verdict.Applied)).
		Set("blocking", canonical.Strings(verdict.Blocking)).
		Set("claim", canonical.String(verdict.Claim)).
		Set("detail", canonical.String(verdict.Detail)).
		Set("generation", canonical.Number(verdict.Generation)), nil
}

func checkinHealth(input canonical.Value) (canonical.Value, error) {
	report := SummarizeHealth(input)

	// Every name in HealthFields, present and null where there is no dated reading. Load-bearing
	// rather than tidy: a console iterating the five names §33.6 lists must not have to discover
	// which of them a particular robot happened to measure, and a key that came and went would make
	// "unmeasured" a question about key presence.
	fields := canonical.Object()
	for at, name := range vocabulary.HealthFields {
		held := report.Fields[at]
		if !held.Present {
			fields = fields.Set(name, canonical.Null())
			continue
		}
		fields = fields.Set(name, canonical.Object().
			Set("age_ms", canonical.Number(held.AgeMS)).
			Set("value", canonical.Number(held.Value)))
	}

	packages := canonical.Array()
	for _, entry := range report.Packages {
		packages = packages.Push(canonical.Object().
			Set("age_ms", canonical.Number(entry.AgeMS)).
			Set("id", canonical.String(entry.ID)).
			Set("live", canonical.Bool(entry.Live)).
			Set("readiness", canonical.String(entry.Readiness)))
	}

	return canonical.Object().
		Set("degraded", canonical.Bool(report.Degraded)).
		Set("fields", fields).
		Set("packages", packages).
		Set("stalled", canonical.Strings(report.Stalled)).
		Set("undated", canonical.Strings(report.Undated)).
		Set("unmeasured", canonical.Strings(report.Unmeasured)), nil
}

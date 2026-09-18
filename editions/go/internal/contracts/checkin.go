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

// ── the check-in document ───────────────────────────────────────────────────

// pendingAction is one reconcile action still open, as the document carries it. The values are the
// input's own, defaulted the way the reference defaults them: `deferred` to false and `reason` to the
// empty string, each only when it is absent or null.
type pendingAction struct {
	stage     canonical.Value
	hasStage  bool
	stageText string
	deferred  canonical.Value
	reason    canonical.Value
}

// CheckIn builds the document a robot sends (§11, §17), on the wire.
//
// It cannot fail, for Attest's reason: the robot most in need of a check-in is the one that cannot
// measure everything. Two editions must produce the same bytes for the same robot, or a fleet console
// shows two robots that differ only in who compiled their supervisor.
//
// The limits are carried through rather than derived. What the guards are and what they lack is the
// kernel's answer for this body, and a builder that recomputed any of it would be a second
// implementation of §7.3 living in the reporting path — which is also why this file never waited on
// resolveLimits, whatever the eighty-first session wrote. It waited on Attest.
func CheckIn(input canonical.Value) canonical.Value {
	progress := input.Field("progress")
	pending := []pendingAction{}
	for _, entry := range progress.Field("pending").Items() {
		stage, hasStage := entry.Get("stage")
		stageText, _ := stage.Str()
		deferred, present := presentNonNull(entry, "deferred")
		if !present {
			deferred = canonical.Bool(false)
		}
		reason, present := presentNonNull(entry, "reason")
		if !present {
			reason = canonical.String("")
		}
		pending = append(pending, pendingAction{
			stage: stage, hasStage: hasStage, stageText: stageText, deferred: deferred, reason: reason,
		})
	}
	// Sorted by stage, stably, comparing as JavaScript's `<` does — by UTF-16 code unit. Sorted rather
	// than merely stable because §34.4 collapses check-ins, and two documents differing only in the
	// order of a list are two documents a queue cannot tell are duplicates.
	sort.SliceStable(pending, func(left, right int) bool {
		return utf16Less(pending[left].stageText, pending[right].stageText)
	})

	pendingShaped := canonical.Array()
	for _, entry := range pending {
		pendingShaped = pendingShaped.Push(canonical.Object().
			Set("deferred", entry.deferred).
			Set("reason", entry.reason).
			SetIf("stage", entry.stage, entry.hasStage))
	}

	claim := ClaimFor(progress)
	return canonical.Object().
		Set("agent", canonical.String(trimmedField(input, "agent"))).
		Set("applied", canonical.Bool(claim.Applied)).
		Set("attestation", shapeAttestation(input.Field("attestation"))).
		Set("blocking", canonical.Strings(claim.Blocking)).
		Set("claim", canonical.String(claim.Claim)).
		Set("contracts", canonical.String(trimmedField(input, "contracts"))).
		Set("desired_digest", canonical.String(trimmedField(input, "desired_digest"))).
		Set("detail", canonical.String(claim.Detail)).
		Set("device_id", canonical.String(trimmedField(input, "device_id"))).
		Set("generation", canonical.Number(claim.Generation)).
		Set("health", shapeHealthReport(SummarizeHealth(input.Field("health")))).
		Set("limits", shapeGuardReport(input.Field("limits"))).
		Set("machine", shapeMachine(input.Field("machine"))).
		Set("pending", pendingShaped).
		Set("state", canonical.String(trimmedField(input, "state")))
}

// shapeMachine is §23.5's machine readings as a check-in carries them: the robot's own values,
// copied field by field, or null when the edition reads none — present-and-null, so a consumer
// can tell "this edition does not read the machine" from "this machine backs no claim". Readings
// inside the block that are null are unestablished, not stock.
func shapeMachine(block canonical.Value) canonical.Value {
	if block.IsNull() {
		return canonical.Null()
	}
	return canonical.Object().
		Set("preempt_rt", block.Field("preempt_rt")).
		Set("swap_kb", block.Field("swap_kb"))
}

// shapeGuardReport is §7.6's guard report as a check-in carries it: the robot's own values, copied
// field by field, or null when the robot resolved none — present-and-null, so a consumer can tell
// "this edition does not report them" from "this robot resolved none". A field absent from the report
// stays absent: a copy that filled one in would be this builder deciding a value the kernel did not.
func shapeGuardReport(report canonical.Value) canonical.Value {
	if report.IsNull() {
		return canonical.Null()
	}
	copied := func(out canonical.Value, key string) canonical.Value {
		value, present := report.Get(key)
		return out.SetIf(key, value, present)
	}
	out := canonical.Object().Set("active", canonical.Array(report.Field("active").Items()...))
	for _, key := range []string{
		"base_velocity_max_m_s", "enforced", "enforcement", "joint_torque_max_nm",
		"joint_velocity_max_rad_s", "max_delta_per_cycle", "self_collision_margin_m", "min_control_hz",
		"resolved_at", "source",
	} {
		out = copied(out, key)
	}

	notices := canonical.Array()
	for _, notice := range report.Field("notices").Items() {
		code, present := notice.Get("code")
		notices = notices.Push(canonical.Object().
			SetIf("code", code, present).
			Set("subjects", canonical.Array(notice.Field("subjects").Items()...)))
	}
	unavailable := canonical.Array()
	for _, entry := range report.Field("unavailable").Items() {
		guard, present := entry.Get("guard")
		unavailable = unavailable.Push(canonical.Object().
			SetIf("guard", guard, present).
			Set("missing", canonical.Array(entry.Field("missing").Items()...)))
	}
	// §80's third state. Always present, empty included: a field a robot omits where it is empty would
	// let an edition that never emits it pass this frozen document (§31.6).
	inert := canonical.Array()
	for _, entry := range report.Field("inert").Items() {
		guard, guardPresent := entry.Get("guard")
		reading, readingPresent := entry.Get("reading")
		inert = inert.Push(canonical.Object().
			SetIf("guard", guard, guardPresent).
			SetIf("reading", reading, readingPresent))
	}
	return out.Set("notices", notices).Set("unavailable", unavailable).Set("inert", inert)
}

func shapeClaimVerdict(verdict ClaimVerdict) canonical.Value {
	return canonical.Object().
		Set("applied", canonical.Bool(verdict.Applied)).
		Set("blocking", canonical.Strings(verdict.Blocking)).
		Set("claim", canonical.String(verdict.Claim)).
		Set("detail", canonical.String(verdict.Detail)).
		Set("generation", canonical.Number(verdict.Generation))
}

// shapeHealthReport carries every name in HealthFields, present and null where there is no dated
// reading. Load-bearing rather than tidy: a console iterating the five names §33.6 lists must not have
// to discover which of them a particular robot happened to measure, and a key that came and went
// would make "unmeasured" a question about key presence.
func shapeHealthReport(report HealthReport) canonical.Value {
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
		Set("unmeasured", canonical.Strings(report.Unmeasured))
}

// ── the corpus ──────────────────────────────────────────────────────────────

func checkinClaim(input canonical.Value) (canonical.Value, error) {
	return shapeClaimVerdict(ClaimFor(input)), nil
}

func checkinHealth(input canonical.Value) (canonical.Value, error) {
	return shapeHealthReport(SummarizeHealth(input)), nil
}

func checkinDocument(input canonical.Value) (canonical.Value, error) {
	return CheckIn(input), nil
}

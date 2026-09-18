package contracts

import (
	"sort"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// Tier readiness.
//
// Pure. Every check is a question about a declared value, and a check whose input is absent **fails
// rather than passing** — the same rule certification applies to a missing test result, for the
// same reason. Absence is not evidence of compliance.

// CheckResult is one check's answer.
type CheckResult struct {
	Check string
	OK    bool
	// RequiredFor is the lowest tier that needs this. A failed `guest` check blocks everything
	// above it too.
	RequiredFor string
	// Detail is English, for logs and API clients. A UI translates by Check.
	Detail string
}

// ReadinessReport is one robot's answer.
type ReadinessReport struct {
	Slug string
	Name string
	// Attainable is the highest tier this robot could *attempt*, given what has been declared.
	// Never a claim about the machine. Absent means it cannot yet attempt even the guest tier,
	// which in practice means no adapter exists.
	Attainable    string
	HasAttainable bool
	Checks        []CheckResult
	// Blockers is the failed checks, in tier order — the vendor's to-do list.
	Blockers []CheckResult
	// Completeness is the share of checks passing, for sorting a fleet-wide view. Not a score
	// anyone should quote.
	Completeness float64
}

// EvaluateReadiness evaluates one robot.
func EvaluateReadiness(subject canonical.Value) ReadinessReport {
	capabilities := subject.Field("capabilities")
	limits, hasLimits := subject.Get("limits")
	// `limits: SafetyEnvelope | null` — null and `{}` are deliberately different: nobody looked,
	// versus somebody looked and found nothing. Only the first blocks.
	declaredEnvelope := hasLimits && !limits.IsNull()

	adapters := numberOr(subject.Field("adapter_count"), 0)
	dof, dofIsNumber := subject.Field("dof").Number()
	controlModes := subject.Field("control_modes")
	controlHz, hzIsNumber := subject.Field("control_hz").Number()
	compute := subject.Field("compute")

	source, _ := limits.Field("source").Str()
	// §5.3: absent reads `declared`, and a present-but-empty word reads as written — `??`,
	// not truthiness, which is why this is the presence bit and not the string.
	basis, basisPresent := limits.Field("basis").Str()
	if !basisPresent {
		basis = "declared"
	}
	watchdog, watchdogIsNumber := limits.Field("watchdog_ms").Number()
	velocity, velocityIsNumber := limits.Field("joint_velocity_max_rad_s").Number()
	torque, torqueIsNumber := limits.Field("joint_torque_max_nm").Number()
	hardwareEstop := isTrue(limits.Field("estop"))

	hasEstopCapability := hasCapability(capabilities, "safety.estop")
	hasOnboardCompute := hasCapability(capabilities, "platform.onboard_compute") || compute.Len() > 0

	checks := []CheckResult{
		{
			Check: "adapter", OK: adapters > 0, RequiredFor: "guest",
			Detail: ifElse(adapters > 0,
				js.NumberToString(adapters)+" published adapter"+plural(adapters)+".",
				"No published adapter, so nothing can talk to this robot yet."),
		},
		{
			Check: "kinematics", OK: dofIsNumber && dof > 0, RequiredFor: "guest",
			// The detail tests truthiness where `ok` tests the type and the sign: a declared zero
			// is falsy, so both agree here and would not on a negative count. The reference is
			// written that way and this follows it rather than tidying it.
			Detail: ifElse(dofIsNumber && js.TruthyNumber(dof),
				js.NumberToString(dof)+" degrees of freedom declared.",
				"No degree-of-freedom count declared."),
		},
		{
			Check: "control_modes", OK: controlModes.Len() > 0, RequiredFor: "guest",
			Detail: ifElse(controlModes.Len() > 0,
				"Accepts "+joinStrings(controlModes, ", ")+".",
				"No control modes declared, so no policy can be matched to it."),
		},
		{
			Check: "control_rate", OK: hzIsNumber && controlHz > 0, RequiredFor: "host",
			Detail: ifElse(hzIsNumber && js.TruthyNumber(controlHz),
				"Control loop runs at "+js.NumberToString(controlHz)+" Hz.",
				"No control rate declared, so loop stability cannot be checked against a policy."),
		},
		{
			Check: "envelope", OK: declaredEnvelope, RequiredFor: "host",
			Detail: ifElse(declaredEnvelope,
				"A safety envelope has been declared.",
				"No safety envelope declared. There is nothing for the kernel to enforce."),
		},
		{
			Check: "envelope_source", OK: js.Truthy(source), RequiredFor: "host",
			Detail: ifElse(js.Truthy(source),
				"Limits cite "+source+" ("+basis+").",
				"Limits cite no datasheet. A safety limit without provenance is a rumour."),
		},
		{
			Check: "watchdog", OK: watchdogIsNumber && watchdog > 0, RequiredFor: "host",
			Detail: ifElse(watchdogIsNumber && js.TruthyNumber(watchdog),
				"Controller faults after "+js.NumberToString(watchdog)+" ms without a command.",
				"No watchdog window declared, so the watchdog guard cannot run."),
		},
		{
			Check: "joint_ratings", OK: velocityIsNumber && torqueIsNumber, RequiredFor: "host",
			Detail: ifElse(velocityIsNumber && js.TruthyNumber(velocity) && torqueIsNumber && js.TruthyNumber(torque),
				"Rated to "+js.NumberToString(velocity)+" rad/s and "+js.NumberToString(torque)+" Nm.",
				"Velocity and torque ceilings are not both declared, so commands cannot be clamped."),
		},
		{
			Check: "estop_software", OK: hasEstopCapability, RequiredFor: "guest",
			Detail: ifElse(hasEstopCapability,
				"Exposes a software emergency stop.",
				"No software emergency stop capability declared."),
		},
		{
			Check: "estop_hardware", OK: hardwareEstop, RequiredFor: "certified",
			Detail: ifElse(hardwareEstop,
				"A hardware emergency stop is fitted.",
				"No hardware emergency stop declared. The certified tier needs a stop path off the application processor."),
		},
		{
			Check: "onboard_compute", OK: hasOnboardCompute, RequiredFor: "host",
			Detail: ifElse(hasOnboardCompute,
				"Runs inference onboard.",
				"No onboard compute declared, so the OS cannot own the control loop here."),
		},
	}

	blockers := []CheckResult{}
	passing := 0
	for _, check := range checks {
		if check.OK {
			passing++
			continue
		}
		blockers = append(blockers, check)
	}
	// Stable, because Array.prototype.sort is stable and two blockers required for the same tier
	// keep the order the checks list gives them.
	sort.SliceStable(blockers, func(left, right int) bool {
		return vocabulary.IndexOf(vocabulary.OSTiers, blockers[left].RequiredFor) <
			vocabulary.IndexOf(vocabulary.OSTiers, blockers[right].RequiredFor)
	})

	attainable, hasAttainable := highestAttainable(checks)
	return ReadinessReport{
		Slug:          stringOr(subject.Field("slug"), ""),
		Name:          stringOr(subject.Field("name"), ""),
		Attainable:    attainable,
		HasAttainable: hasAttainable,
		Checks:        checks,
		Blockers:      blockers,
		Completeness:  float64(passing) / float64(len(checks)),
	}
}

// highestAttainable walks upward and stops at the first tier with an unmet requirement.
//
// Stopping rather than skipping matters: a robot that satisfies every certified-tier check but
// lacks a watchdog window is not "certified with a caveat", it is a robot that cannot attempt the
// host tier. Tiers are cumulative and the walk enforces that.
func highestAttainable(checks []CheckResult) (string, bool) {
	best, haveBest := "", false
	for _, tier := range vocabulary.OSTiers {
		for _, check := range checks {
			if check.RequiredFor == tier && !check.OK {
				return best, haveBest
			}
		}
		best, haveBest = tier, true
	}
	return best, haveBest
}

func hasCapability(capabilities canonical.Value, token string) bool {
	for _, entry := range capabilities.Items() {
		if text, ok := entry.Str(); ok && text == token {
			return true
		}
	}
	return false
}

func joinStrings(array canonical.Value, separator string) string {
	parts := make([]string, 0, array.Len())
	for _, entry := range array.Items() {
		text, _ := entry.Str()
		parts = append(parts, text)
	}
	return strings.Join(parts, separator)
}

func ifElse(condition bool, whenTrue, whenFalse string) string {
	if condition {
		return whenTrue
	}
	return whenFalse
}

// plural is the reference's `${n === 1 ? "" : "s"}`: nothing for exactly one, and an s otherwise.
func plural(count float64) string {
	if count == 1 {
		return ""
	}
	return "s"
}

// ── the corpus ──────────────────────────────────────────────────────────────

func shapeCheck(check CheckResult) canonical.Value {
	return canonical.Object().
		Set("check", canonical.String(check.Check)).
		Set("ok", canonical.Bool(check.OK)).
		Set("required_for", canonical.String(check.RequiredFor)).
		Set("detail", canonical.String(check.Detail))
}

func shapeReadiness(report ReadinessReport) canonical.Value {
	checks := canonical.Array()
	for _, check := range report.Checks {
		checks = checks.Push(shapeCheck(check))
	}
	blockers := canonical.Array()
	for _, blocker := range report.Blockers {
		blockers = blockers.Push(shapeCheck(blocker))
	}
	return canonical.Object().
		Set("slug", canonical.String(report.Slug)).
		Set("name", canonical.String(report.Name)).
		Set("attainable", optionalString(report.Attainable, report.HasAttainable)).
		Set("checks", checks).
		Set("blockers", blockers).
		Set("completeness", canonical.Number(report.Completeness))
}

func readinessRobot(input canonical.Value) (canonical.Value, error) {
	return shapeReadiness(EvaluateReadiness(input)), nil
}

// readinessFleet aggregates across the registry.
//
// `common_blockers` is the number worth putting on a screen: it says whether the ecosystem's
// problem is missing adapters or missing datasheets, and those imply completely different work.
func readinessFleet(input canonical.Value) (canonical.Value, error) {
	byTier := map[string]float64{"none": 0, "guest": 0, "host": 0, "certified": 0}
	// Insertion order, because the reference counts into a Map and then sorts by count — and
	// Array.prototype.sort is stable, so ties keep the order a check was *first* seen as a blocker.
	// A Go map here would randomise exactly that.
	order := []string{}
	counts := map[string]float64{}
	total := 0

	for _, subject := range input.Field("subjects").Items() {
		report := EvaluateReadiness(subject)
		total++
		tier := "none"
		if report.HasAttainable {
			tier = report.Attainable
		}
		byTier[tier]++
		for _, blocker := range report.Blockers {
			if _, seen := counts[blocker.Check]; !seen {
				order = append(order, blocker.Check)
			}
			counts[blocker.Check]++
		}
	}

	sort.SliceStable(order, func(left, right int) bool {
		return counts[order[left]] > counts[order[right]]
	})
	blockers := canonical.Array()
	for _, check := range order {
		blockers = blockers.Push(canonical.Object().
			Set("check", canonical.String(check)).
			Set("robots", canonical.Number(counts[check])))
	}

	return canonical.Object().
		Set("total", canonical.Number(float64(total))).
		Set("by_tier", canonical.Object().
			Set("none", canonical.Number(byTier["none"])).
			Set("guest", canonical.Number(byTier["guest"])).
			Set("host", canonical.Number(byTier["host"])).
			Set("certified", canonical.Number(byTier["certified"]))).
		Set("common_blockers", blockers), nil
}

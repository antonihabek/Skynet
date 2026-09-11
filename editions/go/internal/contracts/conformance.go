package contracts

import (
	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// SCTS and the certification award (§15).

// BenchValidityDays is how long a bench result stands before it must be re-measured.
//
// A year, because bench testing a humanoid is expensive and a shorter window would make
// certification something vendors avoid rather than seek. Field conformance is what carries the
// claim in between — it is measured continuously and can revoke the award long before the year
// is up.
const BenchValidityDays = 365

// FieldConformanceFloor is the figure below which an award is suspended.
//
// 0.999 rather than 1.0: a fleet of a hundred robots running at 100 Hz produces tens of millions of
// cycles a day, and demanding literal perfection would suspend every honest vendor on sensor noise
// while telling operators nothing. What this threshold catches is a systematic drift — a robot
// whose real envelope has moved away from its declared one.
const FieldConformanceFloor = 0.999

// SCTSTest is one row of the suite: what it is, and the lowest tier that requires it.
//
// Only the two fields the award reads are carried. The suite's `measures` prose, its `suite` and
// its `area` belong to the reference and to the Hub that renders them; an edition transcribing
// thirty-six paragraphs it never consults would be carrying a second copy of a document to be wrong
// about (§3.3).
type SCTSTest struct {
	ID           string
	RequiredFrom string
}

// SCTSTests is the suite, in declaration order — which is the order `missing` and `failures` are
// reported in, so it is part of the agreement rather than an accident.
var SCTSTests = []SCTSTest{
	{"rhal.abi.version", "guest"},
	{"rhal.state.completeness", "guest"},
	{"rhal.state.rate", "host"},
	{"rhal.command.latency", "host"},
	{"rhal.command.rejection", "guest"},
	{"rhal.sensor.latency", "host"},
	{"embodiment.descriptor.valid", "guest"},
	{"embodiment.descriptor.signed", "host"},
	{"embodiment.limits.sourced", "host"},
	{"embodiment.limits.honest", "certified"},
	{"embodiment.roles.coverage", "host"},
	{"embodiment.capabilities.declared", "host"},
	{"embodiment.frames.complete", "host"},
	{"safety.kernel.parity", "guest"},
	{"safety.watchdog.fires", "host"},
	{"safety.envelope.clamps", "host"},
	{"safety.estop.latency", "host"},
	{"safety.estop.independent", "certified"},
	{"safety.ratelimit.enforced", "host"},
	{"safety.command_loss.behaviour", "host"},
	{"safety.homing.envelope", "host"},
	{"safety.policy.isolation", "guest"},
	{"package.signature.required", "guest"},
	{"package.budget.enforced", "host"},
	{"package.grants.gate", "guest"},
	{"ota.rollback.works", "host"},
	{"ota.safe_state.respected", "host"},
	{"ota.safety_gate", "host"},
	{"telemetry.blackbox.complete", "host"},
	{"telemetry.replay.deterministic", "host"},
	{"telemetry.blackbox.survives", "certified"},
	{"offline.control.independent", "guest"},
	{"offline.reconcile.resumes", "host"},
	{"security.identity.attested", "host"},
	{"security.credential.scope", "guest"},
	{"power.safe_state.reachable", "host"},
}

// TestsRequiredFor is a tier's own tests plus every tier below it.
func TestsRequiredFor(tier string) []SCTSTest {
	ceiling := vocabulary.IndexOf(vocabulary.OSTiers, tier)
	required := []SCTSTest{}
	for _, test := range SCTSTests {
		if vocabulary.IndexOf(vocabulary.OSTiers, test.RequiredFrom) <= ceiling {
			required = append(required, test)
		}
	}
	return required
}

// CertificationVerdict is what the evidence supports.
type CertificationVerdict struct {
	// Awarded is never above what was claimed, often below.
	Awarded string
	// Failures are tests required for the claimed tier that did not pass.
	Failures []string
	// Missing are required tests with no result at all. Absence is not a pass.
	Missing []string
	// Reasons are why the award landed where it did, for the badge's tooltip and the audit log.
	Reasons []string
	// ExpiresInDays is whether the award needs a re-run to stay valid, and by when.
	ExpiresInDays    float64
	HasExpiresInDays bool
}

// AwardCertification awards a tier from the evidence.
//
// Two rules make this worth having. **Absence is not a pass** — a required test with no result is a
// `missing`, never a silent success, which is the loophole every self-certification scheme
// eventually dies of. And **the award is capped by the evidence, not by the claim** — a vendor
// claiming the certified tier whose independent stop path was never measured is awarded `host`,
// with the reason recorded, rather than refused outright. Downgrading tells the truth and keeps the
// vendor in the programme; refusing sends them to go write "ROS 2 compatible" on a web page
// instead, which helps nobody.
func AwardCertification(input canonical.Value) CertificationVerdict {
	reasons := []string{}
	best := "none"
	failures := []string{}
	missing := []string{}

	outcomes := map[string]string{}
	present := map[string]bool{}
	for _, result := range input.Field("results").Items() {
		id, _ := result.Field("test_id").Str()
		outcome, _ := result.Field("outcome").Str()
		outcomes[id], present[id] = outcome, true
	}

	claimed, _ := input.Field("claimed").Str()
	claimedIndex := vocabulary.IndexOf(vocabulary.OSTiers, claimed)

	// Walk upward and keep the highest tier fully satisfied, so the evidence sets the ceiling.
	for at := 0; at <= claimedIndex; at++ {
		tier := vocabulary.OSTiers[at]
		tierFailures := []string{}
		tierMissing := []string{}

		for _, test := range TestsRequiredFor(tier) {
			outcome, found := outcomes[test.ID], present[test.ID]
			if !found || outcome == "skipped" {
				tierMissing = append(tierMissing, test.ID)
			} else if outcome == "fail" {
				tierFailures = append(tierFailures, test.ID)
			}
			// `not_applicable` passes: the balance guard on a wheeled base is genuinely
			// inapplicable, and forcing a fail there would push vendors to fake a result rather
			// than declare a shape.
		}

		if len(tierFailures) == 0 && len(tierMissing) == 0 {
			best = tier
			failures = []string{}
			missing = []string{}
			continue
		}
		failures = tierFailures
		missing = tierMissing
		reasons = append(reasons, tier+": "+js.NumberToString(float64(len(tierFailures)))+
			" failing, "+js.NumberToString(float64(len(tierMissing)))+" unmeasured.")
		break
	}

	if best != "none" && claimed != best {
		reasons = append(reasons, "Claimed "+claimed+"; the evidence supports "+best+".")
	}

	// §35.3 and §70: a simulator RHAL driver declares itself one, and the declaration caps the
	// award at guest. Not because a simulated run is worthless — the sim half of SCTS is
	// load-bearing infrastructure — but because no simulated run may ever be presented as bench
	// evidence, and a `host` badge on a body that does not exist is exactly that.
	//
	// Applied *after* the evidence walk rather than by refusing to walk, so the verdict still
	// records which tier the tests would have supported.
	if isTrue(input.Field("simulator")) && (best == "host" || best == "certified") {
		reasons = append(reasons,
			"The body is simulated (§35.3), so the award is capped at guest; the evidence supports "+best+".")
		best = "guest"
	}

	// Field conformance suspends an award rather than lowering it. A robot whose commands have been
	// leaving its envelope is not a lower-tier robot — it is one whose certificate no longer
	// describes it, and the honest state is suspended pending re-measurement.
	conformance, hasConformance := input.Field("field_conformance").Number()
	if hasConformance && conformance < FieldConformanceFloor && best != "none" {
		reasons = append(reasons, "Field conformance "+js.ToFixed(conformance*100, 3)+
			"% is below the "+js.ToFixed(FieldConformanceFloor*100, 1)+
			"% floor; the award is suspended pending re-measurement.")
		best = "none"
	}

	verdict := CertificationVerdict{Awarded: best, Failures: failures, Missing: missing, Reasons: reasons}
	benchAge, hasBenchAge := input.Field("bench_age_days").Number()
	if best != "none" && hasBenchAge {
		verdict.ExpiresInDays, verdict.HasExpiresInDays = BenchValidityDays-benchAge, true
		if verdict.ExpiresInDays <= 0 {
			verdict.Reasons = append(verdict.Reasons, "Bench results are older than a year; the award has lapsed.")
			verdict.Awarded = "none"
			verdict.ExpiresInDays = 0
		}
	}
	return verdict
}

// ── the corpus ──────────────────────────────────────────────────────────────

func certificationAward(input canonical.Value) (canonical.Value, error) {
	verdict := AwardCertification(input)
	return canonical.Object().
		Set("awarded", canonical.String(verdict.Awarded)).
		Set("failures", canonical.Strings(verdict.Failures)).
		Set("missing", canonical.Strings(verdict.Missing)).
		Set("reasons", canonical.Strings(verdict.Reasons)).
		Set("expires_in_days", optionalNumber(verdict.ExpiresInDays, verdict.HasExpiresInDays)), nil
}

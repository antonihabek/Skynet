package contracts

import (
	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// The `.sky` package's admission gate (§8, §37.1): whether a package may be installed on this robot.
//
// Pure and shared — the Hub runs it to tell an operator what will happen before they deploy, and the
// OS runs it at install time, offline. Every check produces a finding rather than stopping at the
// first, so an operator sees all the reasons an install failed: fixing four problems one deployment
// at a time is how people end up granting everything.

// PackageFinding is one reason, or one warning. `error` refuses the install.
type PackageFinding struct {
	Code        string
	Severity    string
	Message     string
	Subjects    []string
	HasSubjects bool
}

// AdmissionResult is the gate's answer.
type AdmissionResult struct {
	Admitted bool
	Findings []PackageFinding
	// EffectiveGrants are the capabilities that will be active: requested, provided and granted.
	EffectiveGrants []string
}

// signatureFindings are the four signature states that are findings, each its own code, because
// each has a different owner: a publisher to chase, a trust store to look at, an OS to upgrade, or
// bytes to treat as tampered. `verified` has none, and a state outside the five has none either —
// the reference's switch adds nothing for a state its type does not contain, and this reproduces
// that rather than tightening it, which would be a specification change made in one edition.
var signatureFindings = map[string]PackageFinding{
	"failed": {Code: "signature_failed", Severity: "error",
		Message: "A release signature was present and did not verify. The bytes are not the ones the signer attested to — treat this as tampering until somebody proves otherwise."},
	"untrusted": {Code: "signature_untrusted", Severity: "error",
		Message: "A release signature verified, and the key that made it is not one this robot trusts. Nothing is wrong with the bytes; what is missing is the operator's decision that this signer may publish here."},
	"absent": {Code: "unsigned", Severity: "error",
		Message: "No publisher signature accompanied this package. The bytes may be intact and nobody has attested to who produced them."},
	"unverifiable": {Code: "signature_unverifiable", Severity: "error",
		Message: "This platform cannot verify a release signature, so nothing has established who produced these bytes. That is a gap in the operating system rather than in the package."},
}

// AdmitPackage decides whether a package may be installed.
func AdmitPackage(params canonical.Value) AdmissionResult {
	manifest, capacity := params.Field("manifest"), params.Field("capacity")
	embodiment := manifest.Field("embodiment")
	findings := []PackageFinding{}
	add := func(code, severity, message string) {
		findings = append(findings, PackageFinding{Code: code, Severity: severity, Message: message})
	}
	addAbout := func(code, severity, message string, subjects []string) {
		findings = append(findings, PackageFinding{Code: code, Severity: severity, Message: message, Subjects: subjects, HasSubjects: true})
	}

	if signature, _ := params.Field("signature").Str(); signature != "" {
		if finding, found := signatureFindings[signature]; found {
			findings = append(findings, finding)
		}
	}

	manifestDigest, hasManifestDigest := manifest.Get("digest")
	observedDigest, hasObservedDigest := params.Get("observed_digest")
	if !sameField(observedDigest, hasObservedDigest, manifestDigest, hasManifestDigest) {
		addAbout("digest_mismatch", "error", "The package contents do not hash to the digest in its manifest.",
			[]string{js.ToString(manifestDigest, hasManifestDigest), js.ToString(observedDigest, hasObservedDigest)})
	}

	kind, hasKind := manifest.Get("kind")
	kindText, _ := kind.Str()
	if isolation, _ := manifest.Field("isolation").Str(); kindText != "driver" && isolation == "native" {
		add("isolation_violation", "error",
			"A "+js.ToString(kind, hasKind)+" package may not run unsandboxed. Only drivers touch hardware directly.")
	}

	// §55.1: an environment describes a simulated world and only a simulator driver can load one. A
	// warning rather than an error, because installing it is not dangerous — and silence is not
	// acceptable, because a package that will never do anything looks exactly like one that works.
	if kindText == "environment" && !isTrue(capacity.Field("simulator")) {
		add("environment_inert", "warning",
			"An environment describes a simulated world, and no simulator driver is loaded on this robot. It will install, and nothing will ever load it.")
	}

	if shortfalls := budgetShortfalls(manifest.Field("budget"), capacity); len(shortfalls) > 0 {
		addAbout("budget_exceeded", "error",
			"This robot cannot supply the resources the package declares it needs.", shortfalls)
	}

	requests := stringsOf(manifest.Field("requests"))
	capabilities := stringsOf(embodiment.Field("capabilities"))
	extensions := stringsOf(embodiment.Field("extensions"))

	// A package must not request more than it declared it needs.
	if over := without(requests, capabilities); len(over) > 0 {
		addAbout("over_request", "error", "The package requests permissions it never declared a need for.", over)
	}

	// §85. A token that is not a capability can never match one, and is refused by name rather than
	// read as a robot that lacks it.
	notCanonical := []string{}
	for _, token := range append(append([]string{}, capabilities...), requests...) {
		if ClassifyToken(token) != "canonical" {
			notCanonical = append(notCanonical, token)
		}
	}
	if unknown := distinct(notCanonical); len(unknown) > 0 {
		addAbout("capability_unknown", "error",
			"These are not capabilities this platform defines (§85). A token in a canonical domain must "+
				"be one §85 lists, so check the spelling; a vendor's own token belongs under "+
				"`embodiment.extensions`.",
			unknown)
	}
	notVendor := []string{}
	for _, token := range extensions {
		if ClassifyToken(token) != "vendor" {
			notVendor = append(notVendor, token)
		}
	}
	if misfiled := distinct(notVendor); len(misfiled) > 0 {
		addAbout("capability_unknown", "error",
			"These extensions are not vendor tokens (§85). A token in a canonical domain belongs under "+
				"`embodiment.capabilities`, where it is checked; an extension is dot-separated lowercase "+
				"segments whose first is not a canonical domain.",
			misfiled)
	}

	// Two spellings of one fact, and admission enforces one of them (§8.2). Checked in one direction:
	// a card declaring more than `embodiment` is a package whose requirements were never checked.
	unenforced := append(
		without(stringsOf(manifest.Field("required_capabilities")), capabilities),
		without(stringsOf(manifest.Field("required_extensions")), extensions)...,
	)
	if len(unenforced) > 0 {
		addAbout("embodiment_undeclared", "error",
			"This package declares requirements its embodiment block does not, so admission checked "+
				"nothing for them. §8.2 enforces `embodiment.capabilities` and `embodiment.extensions`.",
			unenforced)
	}

	// §49.1, before the operator's denials on purpose: a reserved token is not something an operator
	// declined, it is something nobody can grant. Deduplicated, because a package that both declares
	// and requests the channel has one problem.
	reserved := []string{}
	for _, token := range append(append([]string{}, requests...), capabilities...) {
		if vocabulary.Contains(vocabulary.ReservedCapabilities, token) && !containsString(reserved, token) {
			reserved = append(reserved, token)
		}
	}
	if len(reserved) > 0 {
		addAbout("capability_reserved", "error",
			"The audible warning before autonomous motion is a system function and no grant confers "+
				"it. A package that could claim this channel could silence its own warning, which "+
				"removes a control the operator's risk assessment relied on (§49.1).", reserved)
	}

	denied := stringsOf(capacity.Field("denied"))
	if explicitlyDenied := within(requests, denied); len(explicitlyDenied) > 0 {
		addAbout("capability_denied", "error",
			"The operator profile withholds capabilities this package requires.", explicitlyDenied)
	}

	provided := append(stringsOf(capacity.Field("capabilities")), stringsOf(capacity.Field("extensions"))...)
	required := append(append([]string{}, capabilities...), extensions...)
	if unmet := without(required, provided); len(unmet) > 0 {
		addAbout("capability_ungranted", "error",
			"This robot does not provide capabilities the package requires.", unmet)
	}

	granted := stringsOf(capacity.Field("granted"))
	awaiting := []string{}
	for _, token := range requests {
		if containsString(provided, token) && !containsString(granted, token) && !containsString(denied, token) {
			awaiting = append(awaiting, token)
		}
	}
	if len(awaiting) > 0 {
		// A warning: the Android runtime-permission moment. The install proceeds, and the mode gate
		// enforces the missing grant at every cycle until the operator gives it.
		addAbout("capability_ungranted", "warning",
			"These capabilities are awaiting an operator grant. The package installs and cannot use them until it is given.",
			awaiting)
	}

	installed := capacity.Field("installed").Items()
	for _, dependency := range manifest.Field("depends_on").Items() {
		id, hasID := dependency.Get("id")
		found := false
		for _, entry := range installed {
			other, hasOther := entry.Get("id")
			if sameField(other, hasOther, id, hasID) {
				found = true
				break
			}
		}
		if !found {
			rangeText, hasRange := dependency.Get("range")
			addAbout("missing_dependency", "error",
				"Requires "+js.ToString(id, hasID)+" "+js.ToString(rangeText, hasRange)+", which is not installed.",
				[]string{js.ToString(id, hasID)})
		}
	}

	minimum, _ := manifest.Field("min_os_version").Str()
	running, hasRunning := capacity.Get("os_version")
	runningText, _ := running.Str()
	if js.Truthy(minimum) && compareOSVersions(runningText, minimum) < 0 {
		add("os_too_old", "error",
			"Requires Skynet OS "+minimum+" or later; this robot runs "+js.ToString(running, hasRunning)+".")
	}

	if verdict, _ := params.Field("transport_verdict").Str(); verdict == "refused" {
		add("transport_refused", "error", "The policy's action space cannot be transported onto this body.")
	}

	if truthyValue(params.Field("executes_on_load")) && !truthyValue(params.Field("trust_remote_code")) {
		add("executes_on_load_untrusted", "error",
			"This package contains files that execute code when read, and the operator has not accepted that. "+
				"Install with trust_remote_code once you have decided the publisher is trustworthy.")
	}

	admitted := true
	for _, finding := range findings {
		if finding.Severity == "error" {
			admitted = false
			break
		}
	}
	effective := []string{}
	if admitted {
		for _, token := range requests {
			if containsString(provided, token) && containsString(granted, token) {
				effective = append(effective, token)
			}
		}
	}
	return AdmissionResult{Admitted: admitted, Findings: findings, EffectiveGrants: effective}
}

// without is the members of values that are not in excluded, in order, repeats kept.
func without(values, excluded []string) []string {
	out := []string{}
	for _, value := range values {
		if !containsString(excluded, value) {
			out = append(out, value)
		}
	}
	return out
}

// within is the members of values that are in included, in order, repeats kept.
func within(values, included []string) []string {
	out := []string{}
	for _, value := range values {
		if containsString(included, value) {
			out = append(out, value)
		}
	}
	return out
}

// budgetShortfalls is every resource this robot cannot supply, named with both figures.
func budgetShortfalls(budget, capacity canonical.Value) []string {
	shortfalls := []string{}
	ram, capacityRAM := bareNumber(budget, "ram_mb"), bareNumber(capacity, "ram_mb")
	if ram > capacityRAM {
		shortfalls = append(shortfalls, "RAM: needs "+js.NumberToString(ram)+" MB, has "+js.NumberToString(capacityRAM)+" MB")
	}
	// `budget.x !== undefined && budget.x > (capacity.x ?? 0)`.
	orZero := func(key string) float64 {
		if _, present := presentNonNull(capacity, key); !present {
			return 0
		}
		return bareNumber(capacity, key)
	}
	for _, resource := range []struct{ key, label, unit, trailer string }{
		{"vram_mb", "VRAM", " MB", " MB"},
		{"disk_mb", "Disk", " MB", " MB"},
		{"cpu_cores", "CPU", " cores", ""},
	} {
		needed := numberField(budget, resource.key)
		if !needed.present {
			continue
		}
		if has := orZero(resource.key); needed.value > has {
			shortfalls = append(shortfalls, resource.label+": needs "+js.NumberToString(needed.value)+resource.unit+
				", has "+js.NumberToString(has)+resource.trailer)
		}
	}
	return shortfalls
}

// segmentValue is one dotted segment's leading run of ASCII digits, or 0 — fail-closed, because a
// segment read as NaN made a robot with one non-numeric segment satisfy every `min_os_version` there
// has ever been. `"10-rc1"` is 10, `"rc1"` is 0, `""` is 0.
func segmentValue(segment string) float64 {
	digits := 0
	for digits < len(segment) && segment[digits] >= '0' && segment[digits] <= '9' {
		digits++
	}
	if digits == 0 {
		return 0
	}
	return js.StringToNumber(segment[:digits])
}

// compareOSVersions is the numeric-segment comparison §8.2's gate uses. It does not defer to
// semver.go, whose parser requires three segments and would read `"2.0"` — which an image reports as
// often as `"2.0.0"` — as nothing at all.
func compareOSVersions(left, right string) int {
	leftSegments, rightSegments := splitOn(left, '.'), splitOn(right, '.')
	count := len(leftSegments)
	if len(rightSegments) > count {
		count = len(rightSegments)
	}
	for at := 0; at < count; at++ {
		var a, b float64
		if at < len(leftSegments) {
			a = segmentValue(leftSegments[at])
		}
		if at < len(rightSegments) {
			b = segmentValue(rightSegments[at])
		}
		if difference := a - b; difference != 0 {
			if difference < 0 {
				return -1
			}
			return 1
		}
	}
	return 0
}

func shapeAdmission(result AdmissionResult) canonical.Value {
	findings := canonical.Array()
	for _, finding := range result.Findings {
		findings = findings.Push(canonical.Object().
			Set("code", canonical.String(finding.Code)).
			Set("severity", canonical.String(finding.Severity)).
			Set("message", canonical.String(finding.Message)).
			SetIf("subjects", canonical.Strings(finding.Subjects), finding.HasSubjects))
	}
	return canonical.Object().
		Set("admitted", canonical.Bool(result.Admitted)).
		Set("findings", findings).
		Set("effective_grants", canonical.Strings(result.EffectiveGrants))
}

func admissionInstall(input canonical.Value) (canonical.Value, error) {
	return shapeAdmission(AdmitPackage(input)), nil
}

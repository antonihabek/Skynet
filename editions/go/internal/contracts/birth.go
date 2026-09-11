package contracts

import (
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
)

// Birth certificates (§50).
//
// §7.2's rule applies here as everywhere: an absent input makes a check **unavailable**, never
// passed. An absent value never satisfies a comparison, which is why `unobservable` is a finding
// rather than a silence.

// BirthFinding is one thing wrong, or one thing that could not be checked.
//
// `Subjects` is uniform and the order within it is load-bearing: for a mismatch it is **what the
// certificate says, then what the machine reports**, in that order and never the reverse. A reader
// who has to work out which of two hex strings came from the document is a reader who will get it
// wrong once.
type BirthFinding struct {
	Code     string
	Subjects []string
}

// VoidsTheCertificate reports whether this finding means the certificate establishes nothing at all.
//
// True only for `unattested` and a mismatched device key. The others narrow what is attested; these
// two remove it.
func VoidsTheCertificate(finding BirthFinding) bool {
	return finding.Code == "unattested" || finding.Code == "device_key_mismatch"
}

// ExplainBirth is the sentence an operator gets, naming the remedy.
func ExplainBirth(finding BirthFinding) string {
	first, second := subjectAt(finding.Subjects, 0), subjectAt(finding.Subjects, 1)
	switch finding.Code {
	case "unattested":
		return "The birth certificate's signature is " + first + ", so it attests nothing: everything it " +
			"says about this machine is currently a claim the machine makes about itself, which is " +
			"the situation a birth certificate exists to end. Register the vendor's key with this " +
			"robot's trust store."
	case "device_key_mismatch":
		return "This certificate was issued over device key " + first + " and the secure element holds " +
			second + ". The document is about a different machine. If the mainboard was replaced " +
			"this is the expected result and not a fault — §50.4 — and the remedy is a vendor-signed " +
			"re-issue, which also invalidates the calibrations a board swap could have disturbed."
	case "serial_mismatch":
		return "The certificate names serial \"" + first + "\" and this machine reports \"" + second + "\". Ask " +
			"the vendor to re-issue."
	case "descriptor_mismatch":
		return "The certificate attests descriptor " + first + " and this robot is running " + second + ". " +
			"The joint ratings in force are ones nobody has signed for this machine. A vendor who " +
			"has published an updated descriptor should re-issue the certificate over it; until then " +
			"the envelope rests on the descriptor's own signature rather than on the vendor's " +
			"attestation that it belongs to this body."
	case "hardware_revision_mismatch":
		return "The certificate names hardware revision \"" + first + "\" and this machine reports " +
			"\"" + second + "\". Either the machine was modified or the certificate is stale; both are a " +
			"vendor re-issue."
	case "ownership_asserted":
		return "This certificate asserts ownership (" + strings.Join(finding.Subjects, ", ") + "), and ownership " +
			"is not the vendor's to assert. §50.2 keeps two facts apart: the certificate proves " +
			"what this robot is, and pairing proves whose it is. The assertion is ignored."
	case "unobservable":
		return "The certificate binds " + first + " and this robot could not read its own. The binding is " +
			"neither confirmed nor contradicted — it is unchecked, which is not the same as " +
			"matching."
	case "incomplete":
		return "The certificate does not bind " + first + ", which §50.2 requires it to. It is not a " +
			"complete birth certificate and whatever it does bind is all it attests."
	}
	return ""
}

// subjectAt is the reference's destructuring default `const [first = "", second = ""] = subjects`:
// a subject the list does not have reads as the empty string.
func subjectAt(subjects []string, at int) string {
	if at < 0 || at >= len(subjects) {
		return ""
	}
	return subjects[at]
}

// BirthAttested is what checking one certificate against one machine concluded.
type BirthAttested struct {
	Findings []BirthFinding
}

// BirthAttests reports whether this certificate attests anything at all about this machine.
//
// Note what this is **not**: it is not "may the robot run". §50 establishes identity; it does not
// gate motion, and a robot whose certificate is void is a robot with an unattested identity, which
// is exactly the state every robot was in before §50 existed.
func BirthAttests(result BirthAttested) bool {
	for _, finding := range result.Findings {
		if VoidsTheCertificate(finding) {
			return false
		}
	}
	return true
}

// BirthFullyConfirmed reports whether every binding the certificate makes was checked and matched.
func BirthFullyConfirmed(result BirthAttested) bool { return len(result.Findings) == 0 }

// BirthUnchecked is the bindings that could not be checked, as their field names.
func BirthUnchecked(result BirthAttested) []string {
	out := []string{}
	for _, finding := range result.Findings {
		if finding.Code == "unobservable" {
			out = append(out, subjectAt(finding.Subjects, 0))
		}
	}
	return out
}

// compareBinding is the reference's `compare`: empty is incomplete, absent is unobservable, and a
// difference is the named mismatch. The three are deliberately distinct answers.
func compareBinding(
	certificate string, observed canonical.Value, field, mismatch string, findings []BirthFinding,
) []BirthFinding {
	if certificate == "" {
		return append(findings, BirthFinding{Code: "incomplete", Subjects: []string{field}})
	}
	value, isString := observed.Str()
	if !isString {
		// `observed === null || observed === undefined`. A field of another type cannot arise from
		// the reference's type and would compare unequal rather than absent; the corpus has none.
		return append(findings, BirthFinding{Code: "unobservable", Subjects: []string{field}})
	}
	if value == certificate {
		return findings
	}
	return append(findings, BirthFinding{Code: mismatch, Subjects: []string{certificate, value}})
}

// CheckBirth checks a birth certificate against what the robot observes about itself (§50).
//
// The order of the findings is the order of their cost, and the first is deliberately about the
// *document* rather than the machine: comparing fields against a certificate nobody signed is
// comparing the robot to a story it told itself.
func CheckBirth(certificate, observed canonical.Value) BirthAttested {
	findings := []BirthFinding{}

	signature, _ := certificate.Field("signature").Str()
	if signature != "verified" {
		findings = append(findings, BirthFinding{Code: "unattested", Subjects: []string{signature}})
	}

	findings = compareBinding(stringOr(certificate.Field("device_key"), ""),
		observed.Field("device_key"), "the device key", "device_key_mismatch", findings)
	findings = compareBinding(stringOr(certificate.Field("serial"), ""),
		observed.Field("serial"), "a serial number", "serial_mismatch", findings)
	findings = compareBinding(stringOr(certificate.Field("descriptor_digest"), ""),
		observed.Field("descriptor_digest"), "an embodiment descriptor digest", "descriptor_mismatch", findings)
	findings = compareBinding(stringOr(certificate.Field("hardware_revision"), ""),
		observed.Field("hardware_revision"), "a hardware revision", "hardware_revision_mismatch", findings)

	if stringOr(certificate.Field("model"), "") == "" {
		// Bound but not observable: a model is what the vendor calls this design, and there is
		// nothing on the machine to compare it against. Its absence is still incomplete.
		findings = append(findings, BirthFinding{Code: "incomplete", Subjects: []string{"a model"}})
	}

	ownership := stringsOf(certificate.Field("claims_ownership"))
	if len(ownership) > 0 {
		findings = append(findings, BirthFinding{Code: "ownership_asserted", Subjects: ownership})
	}

	return BirthAttested{Findings: findings}
}

// ── the corpus ──────────────────────────────────────────────────────────────

func birthCertificate(input canonical.Value) (canonical.Value, error) {
	result := CheckBirth(input.Field("certificate"), input.Field("observed"))
	findings := canonical.Array()
	for _, finding := range result.Findings {
		findings = findings.Push(canonical.Object().
			Set("code", canonical.String(finding.Code)).
			Set("message", canonical.String(ExplainBirth(finding))).
			Set("subjects", canonical.Strings(finding.Subjects)))
	}
	return canonical.Object().
		Set("attests", canonical.Bool(BirthAttests(result))).
		Set("findings", findings).
		Set("fully_confirmed", canonical.Bool(BirthFullyConfirmed(result))).
		Set("unchecked", canonical.Strings(BirthUnchecked(result))), nil
}

package contracts

import (
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// §10 — identity, pairing and attestation. Three decisions answered by three different parties:
// whether eight typed characters are a pairing code (the robot, before any network exists), whether
// the credential that came back is the class §10 asked for (the robot, against the credential), and
// what this robot is running (the sentence an incident investigation needs).
//
// No refusal here ever echoes the code. A pairing code is a bearer secret for the length of a
// pairing, so the faults carry a position and never a character.

// PairingAlphabet is Crockford's base32: no I, L, O or U. The first three are folded rather than
// rejected, because an operator who types the letter they saw should get the code they saw.
const PairingAlphabet = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"

// PairingCodeLength is §18's `skynet pair ABCD-1234`: eight characters, hyphens and spacing removed.
const PairingCodeLength = 8

// pairingCodeGroup is how a code is written for a person: two groups of four.
const pairingCodeGroup = 4

// DeviceTokenPrefix begins a Hub device credential, and is here so one pasted into `skynet pair` is
// recognised and refused by name.
const DeviceTokenPrefix = "skd_"

// CodeFault is why a string is not a pairing code. Position is 1-based and present only for
// `not_in_alphabet`.
type CodeFault struct {
	Code        string
	Detail      string
	Position    int
	HasPosition bool
}

// fold is Crockford's folding: the letters excluded from the alphabet map to the digits they
// resemble.
func fold(character rune) rune {
	switch character {
	case 'O':
		return '0'
	case 'I', 'L':
		return '1'
	}
	return character
}

// NormalizePairingCode turns what somebody typed into a pairing code, or says why it is not one.
//
// The order is the order of usefulness. A credential is caught before anything is stripped, because
// `skd_` contains an underscore and would otherwise be reported as an unusable character at position
// four — true, and useless. Length comes before the alphabet: somebody who typed seven characters
// needs to be told they typed seven.
//
// Every count and position is in **code points**, as the reference's `Array.from` and `for … of`
// are. A person who typed one astral character is not told they typed two.
func NormalizePairingCode(input string) (string, CodeFault, bool) {
	if strings.HasPrefix(js.ToLower(js.Trim(input)), DeviceTokenPrefix) {
		return "", CodeFault{
			Code: "looks_like_a_credential",
			Detail: "This is a device credential — it begins `" + DeviceTokenPrefix + "` — and pairing " +
				"takes the short code the Hub showed you, not the credential it hands back afterwards. " +
				"Nothing has been sent. Treat the credential as exposed if it has been on a screen " +
				"somebody else could see.",
		}, false
	}

	// Two loops, both over code points. `toUpperCase()` on one code point can return several — `ß`
	// is `SS`, `ﬁ` is `FI` — and fold answers a question about a character, so it is asked about each
	// character of the upper-cased result. Folding the result as a string was a real divergence: `ﬁ`
	// stayed `FI` in the reference and became `F1` in the Rust edition, both in the alphabet.
	var normalized strings.Builder
	for _, character := range input {
		switch character {
		case '-', ' ', '\t', '\n', '\r':
			continue
		}
		for _, upper := range js.ToUpper(string(character)) {
			normalized.WriteRune(fold(upper))
		}
	}
	code := normalized.String()
	length := utf8.RuneCountInString(code)

	if code == "" {
		group := strings.Repeat("X", pairingCodeGroup)
		return "", CodeFault{
			Code: "empty",
			Detail: "No code was given. `skynet pair` takes the eight-character code the Hub showed " +
				"you, written as " + group + "-" + group + ".",
		}, false
	}

	if length != PairingCodeLength {
		fault := "too_long"
		if length < PairingCodeLength {
			fault = "too_short"
		}
		return "", CodeFault{
			Code: fault,
			Detail: "A pairing code is " + js.NumberToString(PairingCodeLength) + " characters and this is " +
				js.NumberToString(float64(length)) + ". Hyphens and spaces do not count towards it.",
		}, false
	}

	position := 0
	for _, character := range code {
		position++
		if !strings.ContainsRune(PairingAlphabet, character) {
			return "", CodeFault{
				Code: "not_in_alphabet",
				Detail: "Character " + js.NumberToString(float64(position)) + " of the " +
					js.NumberToString(PairingCodeLength) + " is not one a pairing code uses. Codes are digits " +
					"and upper-case letters, without I, L, O or U — if you read one of those off the screen " +
					"it was a 1 or a 0, and typing it that way works too.",
				Position: position, HasPosition: true,
			}, false
		}
	}
	return code, CodeFault{}, true
}

// ── §19's device credential ─────────────────────────────────────────────────

// IdentityFinding is one thing wrong with, or unestablished about, a robot's identity.
type IdentityFinding struct {
	Code     string
	Severity string
	Message  string
	Subjects []string
}

// IdentityVerdict is §10's answer about one robot's identity.
type IdentityVerdict struct {
	// Paired is whether the robot holds a device credential and knows whose it is.
	Paired bool
	// Attested is §10's `security.identity.attested`, and it takes both halves: a hardware key nobody
	// attested says nothing about the machine, and an attested key in a file copies to any machine.
	Attested bool
	Findings []IdentityFinding
}

// trimmedField is the reference's `(x ?? "").trim()` — a field absent or null reads as the empty
// string, and whatever is there is trimmed with ECMAScript's whitespace set.
func trimmedField(object canonical.Value, key string) string {
	text, _ := object.Field(key).Str()
	return js.Trim(text)
}

// CheckIdentity is what state a robot's identity is in, and everything wrong with it — every check
// runs, because an operator fixing one problem per attempt is the failure a finding list exists to
// prevent. Not paired is not a fault: §50.2 ships a robot belonging to nobody.
func CheckIdentity(identity, observed canonical.Value) IdentityVerdict {
	findings := []IdentityFinding{}
	add := func(code, severity, message string, subjects []string) {
		findings = append(findings, IdentityFinding{Code: code, Severity: severity, Message: message, Subjects: subjects})
	}
	credential := isTrue(identity.Field("credential"))
	namespace := trimmedField(identity, "namespace")

	if !credential {
		add("not_paired", "warning",
			"This robot holds no device credential, so it belongs to nobody and reports to nothing. That "+
				"is the state a robot ships in (§50.2). Pair it with `skynet pair` and the code the Hub "+
				"showed you.", []string{})
	} else {
		if namespace == "" {
			add("namespace_missing", "error",
				"This robot holds a credential and names no namespace. §10 pairs a robot to a namespace, "+
					"so a credential without one has bound it to nothing — and every report it sends is "+
					"addressed to a fleet nobody can name.", []string{})
		}

		// `credential_prefix ?? ''`, compared exactly: no trim and no case folding, because a prefix is
		// either the device class's or it is not.
		prefix, _ := identity.Field("credential_prefix").Str()
		if prefix != DeviceTokenPrefix {
			shown := "with nothing recognisable"
			if prefix != "" {
				shown = "`" + prefix + "`"
			}
			add("not_a_device_credential", "error",
				"A device credential begins `"+DeviceTokenPrefix+"` and this one begins "+shown+". §10 puts "+
					"the narrowest credential class in the system on the robot on purpose, because a "+
					"credential physically present in the field is the easiest one to lose. A wider one "+
					"here is a wider one lost.", []string{})
		}

		if scopes, declared := presentNonNull(identity, "scopes"); !declared {
			add("scope_undeclared", "warning",
				"This credential declares no scopes, so nothing on the robot can check that it is as "+
					"narrow as §19 requires. That is the ordinary shape of an opaque bearer token and is not "+
					"a fault — it means `security.credential.scope` is answered on the Hub and not here.",
				[]string{})
		} else {
			// `[...new Set(…)].sort()`: deduplicated, then sorted by UTF-16 code unit as a JavaScript
			// sort compares strings.
			seen := map[string]bool{}
			unrecognised := []string{}
			for _, scope := range stringsOf(scopes) {
				if vocabulary.Contains(vocabulary.DeviceScopes, scope) || seen[scope] {
					continue
				}
				seen[scope] = true
				unrecognised = append(unrecognised, scope)
			}
			sort.SliceStable(unrecognised, func(left, right int) bool {
				return utf16Less(unrecognised[left], unrecognised[right])
			})
			if len(unrecognised) > 0 {
				add("scope_unrecognised", "error",
					"This credential carries "+strings.Join(unrecognised, ", ")+", which is not a capability "+
						"a device credential is recognised to need. §19 assumes this credential is lost, and a "+
						"capability nobody has established is narrow, on a credential everybody assumes is "+
						"stolen, is refused rather than allowed.", unrecognised)
			}
		}
	}

	// §10's second addition: what the key is bound to.
	key := trimmedField(identity, "device_key")
	binding, bound := presentNonNull(identity, "binding")
	birthKey := trimmedField(observed, "birth_key")
	bindingText, _ := binding.Str()

	if !bound {
		add("key_binding_unknown", "warning",
			"Nothing has established whether this key lives in a secure element. "+
				"`security.identity.attested` is a measurement, so an unmeasured robot is reported "+
				"unmeasured rather than counted as either answer.", []string{})
	} else if bindingText != "hardware" {
		add("key_not_hardware_bound", "warning",
			"This device key is a file rather than a secret inside a secure element, so it leaves with a "+
				"copy of the filesystem and identifies whatever machine that copy is put on. That is the "+
				"ordinary state at guest tier and it is the ceiling §10 puts on "+
				"`security.identity.attested`.", []string{})
	}

	certificate := isTrue(observed.Field("birth_certificate"))
	if !certificate {
		add("no_birth_certificate", "warning",
			"This robot holds no trusted birth certificate, so there is nothing to check the device key "+
				"against. §50.3: without one, what this machine is remains a claim it makes about itself.",
			[]string{})
	} else if birthKey != "" && key != "" && birthKey != key {
		add("key_not_attested", "error",
			"The key this robot was paired on is not the key its vendor attested at manufacture. §50.2 "+
				"separates what it is from whose it is, and this is the seam between them coming apart: "+
				"either the mainboard was replaced — §50.4, expected, and the remedy is a vendor-signed "+
				"re-issue — or this robot was paired on a key nobody signed.", []string{birthKey})
	}

	attested := bound && bindingText == "hardware" && certificate && birthKey != "" && key != "" && birthKey == key
	return IdentityVerdict{Paired: credential && namespace != "", Attested: attested, Findings: findings}
}

// ── §10's third addition: image attestation ─────────────────────────────────

// AttestationReport is §10's sentence, and what is missing from it.
type AttestationReport struct {
	Complete   bool
	Sentence   string
	Unmeasured []string
}

// renderMeasure is one clause of §10's sentence, including its own label — `known` carries the label
// and the other two do not, because *an unmeasured OS image* already says what it is.
func renderMeasure(measure canonical.Value, label, noun string) string {
	state, _ := measure.Field("state").Str()
	switch state {
	case "known":
		// A `known` measure with nothing in it is a caller bug, rendered as the absence it is rather
		// than as an empty pair of backticks somebody reads as a digest of zero length.
		if digest := trimmedField(measure, "digest"); digest != "" {
			return label + "`" + digest + "`"
		}
		return "an unnamed " + noun
	case "nothing":
		return "no " + noun
	}
	if why := trimmedField(measure, "why"); why != "" {
		return "an unmeasured " + noun + " (" + why + ")"
	}
	return "an unmeasured " + noun
}

// Attest is §10's sentence. It cannot fail, and that is the design: a check-in withheld because a
// digest could not be read is a robot the fleet believes is offline, at the moment somebody needs to
// know what it is running. An attestation with three holes in it says so three times.
func Attest(attestation canonical.Value) AttestationReport {
	unmeasured := []string{}
	for _, name := range []string{"os_image", "policy", "embodiment"} {
		if state, _ := attestation.Field(name).Field("state").Str(); state == "unmeasured" {
			unmeasured = append(unmeasured, name)
		}
	}
	sort.Strings(unmeasured)

	subject := trimmedField(attestation, "device_id")
	if subject == "" {
		subject = "This robot"
	}
	sentence := subject + " is running " + renderMeasure(attestation.Field("os_image"), "image ", "OS image") + ", " +
		renderMeasure(attestation.Field("policy"), "policy ", "policy") + ", " +
		"against " + renderMeasure(attestation.Field("embodiment"), "body ", "body") + "."
	return AttestationReport{Complete: len(unmeasured) == 0, Sentence: sentence, Unmeasured: unmeasured}
}

// ── the wire ────────────────────────────────────────────────────────────────

// shapeCodeResult carries `code` and `fault` both present and both nullable, and a caller reads
// exactly one of them.
func shapeCodeResult(code string, fault CodeFault, ok bool) canonical.Value {
	if ok {
		return canonical.Object().Set("code", canonical.String(code)).Set("fault", canonical.Null())
	}
	position := canonical.Null()
	if fault.HasPosition {
		position = canonical.Number(float64(fault.Position))
	}
	return canonical.Object().
		Set("code", canonical.Null()).
		Set("fault", canonical.Object().
			Set("code", canonical.String(fault.Code)).
			Set("detail", canonical.String(fault.Detail)).
			Set("position", position))
}

func shapeIdentityVerdict(verdict IdentityVerdict) canonical.Value {
	findings := canonical.Array()
	for _, entry := range verdict.Findings {
		findings = findings.Push(canonical.Object().
			Set("code", canonical.String(entry.Code)).
			Set("message", canonical.String(entry.Message)).
			Set("severity", canonical.String(entry.Severity)).
			Set("subjects", canonical.Strings(entry.Subjects)))
	}
	return canonical.Object().
		Set("attested", canonical.Bool(verdict.Attested)).
		Set("findings", findings).
		Set("paired", canonical.Bool(verdict.Paired))
}

func shapeAttestationReport(report AttestationReport) canonical.Value {
	return canonical.Object().
		Set("complete", canonical.Bool(report.Complete)).
		Set("sentence", canonical.String(report.Sentence)).
		Set("unmeasured", canonical.Strings(report.Unmeasured))
}

// trimmedOrNull is the reference's `trimmedOrNull`: the trimmed text, or null when nothing is left.
func trimmedOrNull(object canonical.Value, key string) canonical.Value {
	if text := trimmedField(object, key); text != "" {
		return canonical.String(text)
	}
	return canonical.Null()
}

// shapeMeasure keeps the digest rather than rendering it away, because a digest that survives only
// inside a sentence is one a change to the sentence silently deletes. A digest on a measure that is
// not `known` is dropped: a robot whose slot was never read must not answer the fleet's "which robots
// are running image X".
func shapeMeasure(measure canonical.Value) canonical.Value {
	state, statePresent := measure.Get("state")
	digest := canonical.Null()
	if text, _ := state.Str(); text == "known" {
		digest = trimmedOrNull(measure, "digest")
	}
	return canonical.Object().
		Set("digest", digest).
		SetIf("state", state, statePresent).
		Set("why", trimmedOrNull(measure, "why"))
}

// shapeAttestation is the sentence **and** the three digests it is made of: the sentence is what
// somebody reads at 3 a.m., and the measures are what somebody queries.
func shapeAttestation(attestation canonical.Value) canonical.Value {
	report := Attest(attestation)
	return shapeAttestationReport(report).
		Set("embodiment", shapeMeasure(attestation.Field("embodiment"))).
		Set("os_image", shapeMeasure(attestation.Field("os_image"))).
		Set("policy", shapeMeasure(attestation.Field("policy")))
}

// ── the corpus ──────────────────────────────────────────────────────────────

func identityPairing(input canonical.Value) (canonical.Value, error) {
	text, _ := input.Field("input").Str()
	return shapeCodeResult(NormalizePairingCode(text)), nil
}

func identityCredential(input canonical.Value) (canonical.Value, error) {
	return shapeIdentityVerdict(CheckIdentity(input.Field("identity"), input.Field("observed"))), nil
}

func identityAttestation(input canonical.Value) (canonical.Value, error) {
	return shapeAttestationReport(Attest(input)), nil
}

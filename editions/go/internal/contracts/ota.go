package contracts

import (
	"sort"
	"strings"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
	"github.com/antonihabek/Skynet/editions/go/internal/vocabulary"
)

// OS updates (§12).

// DefaultBootBudgetMS is §12's default boot budget.
//
// Ten minutes. Long enough for a robot that boots slowly, fetches a desired state over a poor link
// and starts a driver that probes a bus; short enough that a warehouse at 3 a.m. is not a warehouse
// with a brick in it until morning. The document does not give a number, so this is a default an
// operator may narrow rather than a constant anybody derived.
const DefaultBootBudgetMS = 600000

// Gated reports whether changing this component requires a fresh bench pass.
//
// True for §12's two names, and true for anything this edition does not recognise: a component
// nobody can classify is one nobody has established is safe to change, and the two readings fail in
// opposite directions.
func Gated(component string) bool {
	if containsString(vocabulary.CertificationGated, component) {
		return true
	}
	return !containsString(vocabulary.OSComponents, component)
}

// BundleFault is one thing wrong with an update bundle.
type BundleFault struct {
	Code     string
	Severity string
	Message  string
	Subjects []string
}

// BundleVerdict is whether an image may be installed.
type BundleVerdict struct {
	Admitted bool
	Faults   []BundleFault
	// GatedComponents are the declared components that require a bench pass, sorted. Reported
	// whether or not one was held, because *"this update touches the safety kernel"* is a fact an
	// operator wants on the screen before they go and book bench time.
	GatedComponents []string
}

func displayDigest(digest string) string {
	if digest == "" {
		return "nothing"
	}
	return digest
}

func certificationMessage(gatedComponents, unrecognised []string) string {
	message := "This update changes " + strings.Join(gatedComponents, ", ") + ", and §12 refuses one that " +
		"changes the safety kernel or the envelope without a fresh bench pass for these exact bytes."
	if len(unrecognised) > 0 {
		message += " " + strings.Join(unrecognised, ", ") + " is not a component this edition recognises, and an " +
			"unrecognised component is gated for the same reason the two named ones are: nobody has " +
			"established what it changes."
	}
	return message + " This robot holds no bench pass at all."
}

// AdmitBundle is §12's third rule, and everything else that decides whether an image may be
// installed.
//
// Every check runs, and the result carries all of them. An operator fixing four problems one
// attempt at a time is the failure admitPackage already refuses to cause, and an OS update is the
// case where each attempt costs a reboot.
func AdmitBundle(bundle, subject canonical.Value) BundleVerdict {
	faults := []BundleFault{}
	version := stringOr(bundle.Field("version"), "")
	digest := stringOr(bundle.Field("digest"), "")
	channel := stringOr(bundle.Field("channel"), "")
	compatible := stringsOf(bundle.Field("compatible"))
	changes := stringsOf(bundle.Field("changes"))

	if js.Trim(version) == "" {
		faults = append(faults, BundleFault{Code: "unversioned", Severity: "error", Subjects: []string{},
			Message: "This bundle names no version, so nothing can say what the robot would be running " +
				"afterwards. Rebuild it with a version."})
	}

	// Before the certification gate, because that gate binds a bench pass to this digest and a
	// bundle with no digest cannot be bound to anything. Reported separately so the two refusals do
	// not read as one problem.
	if js.Trim(digest) == "" {
		faults = append(faults, BundleFault{Code: "undigested", Severity: "error", Subjects: []string{},
			Message: "This bundle names no digest. §12's certification gate binds a bench pass to the exact bytes " +
				"it was taken against, and bytes with no name cannot be bound to."})
	}

	// §37.1's five outcomes, kept apart here for the reason they are kept apart there: four
	// different facts with four different people at the end of them.
	signature, _ := subject.Field("signature").Str()
	switch signature {
	case "verified":
	case "failed":
		faults = append(faults, BundleFault{Code: "signature_failed", Severity: "error", Subjects: []string{},
			Message: "A signature was present and did not verify. The bytes are attested to somebody other than " +
				"the manifest claims, which is what tampering looks like — do not install this, and find " +
				"out where it came from."})
	case "untrusted":
		faults = append(faults, BundleFault{Code: "signature_untrusted", Severity: "error", Subjects: []string{},
			Message: "The signature verifies against a key this robot has not been told to believe. Add the key " +
				"with `skynetd trust add`, or find out why a stranger signed an OS image for this fleet."})
	case "absent":
		faults = append(faults, BundleFault{Code: "signature_absent", Severity: "error", Subjects: []string{},
			Message: "Nobody signed this bundle. §12 signs images with the same Ed25519 machinery as releases, " +
				"and an unsigned root filesystem is a root filesystem anybody on the path could have " +
				"written."})
	default:
		faults = append(faults, BundleFault{Code: "signature_unverifiable", Severity: "error", Subjects: []string{},
			Message: "This platform cannot check a signature at all, so it cannot establish that this image is " +
				"the one the fleet published. That is the operating system's problem rather than the " +
				"publisher's."})
	}

	// Compatibility. A bundle that declares nothing is a **warning**, because §12 does not require
	// the field and refusing on it would stop every image built before it existed — but it is said
	// out loud, because "this image does not know what it is for" is exactly what somebody wants to
	// have been told after flashing the wrong board.
	board, hasBoard := subject.Field("board").Str()
	switch {
	case len(compatible) == 0:
		faults = append(faults, BundleFault{Code: "compatibility_undeclared", Severity: "warning", Subjects: []string{},
			Message: "This bundle declares no compatible machines, so nothing here can tell whether it is built " +
				"for this one."})
	case hasBoard && !containsString(compatible, board):
		faults = append(faults, BundleFault{Code: "incompatible", Severity: "error", Subjects: compatible,
			Message: "This bundle is built for " + strings.Join(compatible, ", ") + ", and this machine is `" + board + "`."})
	case !hasBoard:
		// A robot that cannot say what board it is has not established a mismatch, and refusing
		// would make an unknown machine unupdatable — which is the state a robot is in exactly when
		// it most needs an update.
		faults = append(faults, BundleFault{Code: "compatibility_undeclared", Severity: "warning", Subjects: compatible,
			Message: "This machine does not report what it is, so the bundle's compatibility with " +
				strings.Join(compatible, ", ") + " could not be checked."})
	}

	wanted := stringOr(subject.Field("channel"), "")
	if wanted != "" && channel != "" && wanted != channel {
		faults = append(faults, BundleFault{Code: "channel_mismatch", Severity: "error", Subjects: []string{},
			Message: "The desired state asks for the `" + wanted + "` channel and this bundle is `" + channel + "`. A " +
				"robot pulled onto another channel by an image rather than by an operator is a robot nobody " +
				"moved."})
	}

	// ── §12's third rule ───────────────────────────────────────────────────
	seen := map[string]bool{}
	gatedComponents := []string{}
	for _, component := range changes {
		if !Gated(component) || seen[component] {
			continue
		}
		seen[component] = true
		gatedComponents = append(gatedComponents, component)
	}
	sort.Strings(gatedComponents)

	if len(gatedComponents) > 0 {
		unrecognised := []string{}
		for _, component := range gatedComponents {
			if !containsString(vocabulary.OSComponents, component) {
				unrecognised = append(unrecognised, component)
			}
		}
		bench, hasBench := subject.Get("bench")
		switch {
		case !hasBench || bench.IsNull():
			faults = append(faults, BundleFault{Code: "certification_required", Severity: "error",
				Message: certificationMessage(gatedComponents, unrecognised), Subjects: gatedComponents})
		default:
			benchDigest := stringOr(bench.Field("image_digest"), "")
			if benchDigest != digest || digest == "" {
				faults = append(faults, BundleFault{Code: "bench_pass_stale", Severity: "error", Subjects: gatedComponents,
					Message: "The bench pass this robot holds was taken against `" + displayDigest(benchDigest) + "` " +
						"and this bundle is `" + displayDigest(digest) + "`. Evidence that is not bound to what it " +
						"is evidence of is not evidence — book a fresh bench pass for these bytes."})
			} else if !isTrue(bench.Field("passed")) {
				faults = append(faults, BundleFault{Code: "bench_pass_failed", Severity: "error", Subjects: gatedComponents,
					Message: "A bench pass exists for these bytes and it records a failure. This update changes " +
						strings.Join(gatedComponents, ", ") + ", and §12 refuses one that has not passed rather than one " +
						"that has not been tried."})
			}
		}
	}

	running := stringOr(subject.Field("running_version"), "")
	if running != "" && running == version {
		faults = append(faults, BundleFault{Code: "already_running", Severity: "warning", Subjects: []string{},
			Message: "This robot is already running `" + version + "`. Staging it again is harmless and applying it " +
				"costs a reboot for nothing."})
	}

	admitted := true
	for _, entry := range faults {
		if entry.Severity == "error" {
			admitted = false
		}
	}
	return BundleVerdict{Admitted: admitted, Faults: faults, GatedComponents: gatedComponents}
}

// BootVerdict is what to tell the bootloader.
type BootVerdict struct {
	Decision string
	Detail   string
	// Missing are the named checks that have not passed, sorted. Empty on a confirm.
	Missing []string
}

// ConfirmBoot is §12's second rule — health-gated rollback.
//
//	The new slot must pass a health check within a boot budget or the bootloader returns to the
//	previous slot unattended.
//
// Three properties, and each is the reason a rule of this shape is written down rather than left to
// a script:
//
//   - **Absence of evidence is not health.** Every check is optional and absent never confirms. A
//     supervisor that marked a slot good because it had not yet been told otherwise would replace
//     §12's unattended recovery with a race between the health check and the confirmation.
//   - **The budget is what makes it unattended.** Without it a robot that never becomes healthy
//     waits for ever, in the state the rollback exists to leave. With it, the worst case is a reboot.
//   - **A robot that is not pending decides nothing.** Marking a slot bad on an ordinary boot is a
//     rollback of an update nobody made.
func ConfirmBoot(evidence canonical.Value) BootVerdict {
	if !isTrue(evidence.Field("pending")) {
		return BootVerdict{
			Decision: "not_pending",
			Detail: "This boot is not the first into a new slot, so there is nothing to confirm and " +
				"nothing to roll back.",
			Missing: []string{},
		}
	}

	elapsed := numberOr(evidence.Field("elapsed_ms"), 0)
	budget := numberOr(evidence.Field("budget_ms"), DefaultBootBudgetMS)

	missing := []string{}
	if !isTrue(evidence.Field("post_passed")) {
		missing = append(missing, "post")
	}
	if !isTrue(evidence.Field("packages_healthy")) {
		missing = append(missing, "packages")
	}
	if !isTrue(evidence.Field("converged")) {
		missing = append(missing, "converged")
	}
	sort.Strings(missing)

	if len(missing) == 0 {
		return BootVerdict{
			Decision: "confirm",
			Detail: "The self-test passed, every wanted package is running and the reconciler converged, " +
				js.NumberToString(elapsed) + " ms into a " + js.NumberToString(budget) + " ms budget. The slot is good.",
			Missing: missing,
		}
	}

	// A budget that is not a positive number is no budget, and no budget means waiting for ever
	// rather than rolling back on the first pass. The safe direction here is the patient one: an
	// unattended rollback triggered by a misconfigured budget is a working robot rebooted into an
	// older image for no reason.
	expired := canonical.Finite(budget) && budget > 0 && canonical.Finite(elapsed) && elapsed >= budget

	if expired {
		verb := "have"
		if len(missing) == 1 {
			verb = "has"
		}
		return BootVerdict{
			Decision: "roll_back",
			Detail: js.NumberToString(elapsed) + " ms have passed and the budget is " + js.NumberToString(budget) + " ms, " +
				"and " + strings.Join(missing, ", ") + " still " + verb + " not passed. " +
				"Marking the slot bad; the bootloader returns to the previous one.",
			Missing: missing,
		}
	}

	return BootVerdict{
		Decision: "wait",
		Detail: js.NumberToString(elapsed) + " ms into a " + js.NumberToString(budget) + " ms budget, with " +
			strings.Join(missing, ", ") + " still to pass. Saying nothing to the bootloader until they do or the " +
			"budget expires.",
		Missing: missing,
	}
}

// ── the corpus ──────────────────────────────────────────────────────────────

func otaBundle(input canonical.Value) (canonical.Value, error) {
	verdict := AdmitBundle(input.Field("bundle"), input.Field("subject"))
	faults := canonical.Array()
	for _, fault := range verdict.Faults {
		faults = faults.Push(canonical.Object().
			Set("code", canonical.String(fault.Code)).
			Set("severity", canonical.String(fault.Severity)).
			Set("message", canonical.String(fault.Message)).
			Set("subjects", canonical.Strings(fault.Subjects)))
	}
	return canonical.Object().
		Set("admitted", canonical.Bool(verdict.Admitted)).
		Set("faults", faults).
		Set("gated_components", canonical.Strings(verdict.GatedComponents)), nil
}

func otaBoot(input canonical.Value) (canonical.Value, error) {
	// The case's input *is* the evidence, as `telemetry/window`'s is the window. The two shapes in
	// this corpus differ — `ota/bundle` wraps its two documents in named fields — and reading the
	// wrong one produced `not_pending` for every case, which is the answer a boot with no evidence
	// gets and therefore the one failure that looks like a decision.
	verdict := ConfirmBoot(input)
	return canonical.Object().
		Set("decision", canonical.String(verdict.Decision)).
		Set("detail", canonical.String(verdict.Detail)).
		Set("missing", canonical.Strings(verdict.Missing)), nil
}

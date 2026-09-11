// The contracts, and the table that routes a corpus file to one.
//
// Every function under this package is written from `reference/typescript/*` and from nothing else
// — never from `corpus/`, which would agree by construction, and never from another edition, which
// would prove only that one hand did both. What holds them honest is the corpus, in the one
// direction that means something: this edition reproducing what the oracle produced, byte for byte.
//
// One file here per module there, named the same, so a reader holding the reference open can find
// the Go beside it. The dispatch is the only file with no counterpart: the reference has no such
// table, because in TypeScript the corpus generator calls the functions directly.
package contracts

import (
	"github.com/antonihabek/Skynet/editions/go/internal/corpus"
)

// Dispatch is every corpus file this edition answers, keyed by `corpus/file`.
//
// A file absent from this table is counted as *not implemented* rather than as passing, and the
// runner prints the two separately — §31.4's "an edition that has not passed the corpus is not an
// edition" is not satisfied by a table with fewer entries in it.
func Dispatch() corpus.Dispatch {
	return corpus.Dispatch{
		"admission/install":      admissionInstall,
		"birth/certificate":      birthCertificate,
		"blackbox/header":        blackboxHeader,
		"blackbox/records":       blackboxRecords,
		"blackbox/schemas":       blackboxSchemas,
		"blackbox/seal":          blackboxSeal,
		"calibration/events":     calibrationCase,
		"calibration/expiry":     calibrationCase,
		"calibration/provenance": calibrationCase,
		"calibration/sets":       calibrationSets,
		"cell/context":           cellContext,
		"cell/validate":          cellValidate,
		"certification/award":    certificationAward,
		"checkin/claim":          checkinClaim,
		"checkin/document":       checkinDocument,
		"checkin/health":         checkinHealth,
		"cycle/advisory":         cycleEvaluate,
		"cycle/collision":        cycleEvaluate,
		"cycle/empty":            cycleEvaluate,
		"cycle/guards":           cycleEvaluate,
		"cycle/mode-gate":        cycleEvaluate,
		"cycle/proximity":        cycleEvaluate,
		"cycle/workspace":        cycleEvaluate,
		"digest/version":         versionDigest,
		"embodiment/detection":   embodimentDetection,
		"frames/bindings":        framesBindings,
		"frames/validate":        framesValidate,
		// The fuzz files are the curated corpora's contracts fed inputs nobody chose by hand, so each
		// is registered to the handler its family already has. Named one by one rather than routed by
		// a prefix: a fourth family added to the generator must arrive here as a file with no
		// handler, not fall into whichever contract happened to match.
		"fuzz/cycle":             cycleEvaluate,
		"fuzz/observation":       observationFrames,
		"fuzz/transport":         transportResolve,
		"identity/attestation":   identityAttestation,
		"identity/credential":    identityCredential,
		"identity/pairing":       identityPairing,
		"intent/offer":           intentOffer,
		"kinematics/solve":       kinematicsSolve,
		"limits/resolution":      limitsResolution,
		"live/context":           liveContextCase,
		"media/pool":             mediaPool,
		"observation/frames":     observationFrames,
		"observation/target":     observationTarget,
		"opstate/evaluate":       opstateEvaluate,
		"opstate/properties":     opstateProperties,
		"opstate/warning":        opstateWarning,
		"ota/boot":               otaBoot,
		"ota/bundle":             otaBundle,
		"power/brownout":         powerCase,
		"power/budget":           powerCase,
		"power/state":            powerCase,
		"power/thermal":          powerCase,
		"power/unmetered":        powerCase,
		"predicate/evaluate":     predicateEvaluate,
		"provenance/classify":    provenanceClassify,
		"provenance/scores":      provenanceScores,
		"readiness/fleet":        readinessFleet,
		"readiness/robot":        readinessRobot,
		"roles/vocabulary":       rolesVocabulary,
		"semver/compare":         semverCompare,
		"semver/satisfies":       semverSatisfies,
		"skill/bounded-deadline": skillBoundedDeadline,
		"skill/composition":      skillComposition,
		"skill/lifecycle":        skillLifecycle,
		"skill/preconditions":    skillPreconditions,
		"skill/success":          skillSuccess,
		"skill/timeout":          skillTimeout,
		"telemetry/coverage":     telemetryCoverage,
		"telemetry/window":       telemetryWindow,
		"transfer/arrival":       transferArrival,
		"transfer/fetch":         transferFetch,
		"transfer/receipt":       transferReceipt,
		"transfer/upload":        transferUpload,
		"transport/binding":      transportResolve,
		"transport/declaration":  transportResolve,
		"transport/non-joint":    transportResolve,
		"transport/observation":  transportResolve,
		"transport/rate":         transportResolve,
		"transport/retarget":     transportResolve,
		"transport/scaling":      transportResolve,
		"vocabulary/closed-sets": closedSets,
		"watchdog/command-loss":  watchdogCase,
		"watchdog/nesting":       watchdogCase,
	}
}

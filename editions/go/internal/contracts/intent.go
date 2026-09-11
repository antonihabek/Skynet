package contracts

import (
	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

// May this response become the desired state (§11, §17, §33.8)?
//
// Pure: no network, no filesystem. Whether the document decoded arrives **decided**, the way a
// signature state does in §37.1, so the same function runs on the robot and on a Hub checking what
// it just served — a second implementation of the decoder here would be a second answer to a
// question already answered.

// OfferFault is one thing wrong with a desired-state offer.
type OfferFault struct {
	Code     string
	Severity string
	Message  string
}

// OfferVerdict is what the robot decided about the document it was handed.
type OfferVerdict struct {
	// Adopt is whether this document may become the desired state on disk.
	Adopt bool
	// Changed is whether adopting it changes anything. False for the ordinary poll, which is most
	// of them: §11's loop asks the same question every few seconds and the answer is usually the
	// document it already has. Separated from Adopt so a caller can skip the write without
	// comparing digests itself — and so a log line exists for "the Hub answered and nothing
	// changed", which is different from "the Hub did not answer".
	Changed bool
	// Generation is what the robot would be reconciling towards. Its own when the offer is refused.
	Generation float64
	Faults     []OfferFault
}

// AssessOffer runs every check and carries all of them, for admitPackage's reason: a fleet operator
// diagnosing why a robot will not take an update should get one list rather than one problem per
// poll.
func AssessOffer(
	decoded bool, decodeError string, offeredGeneration float64, offeredDigest string, packages float64,
	heldGeneration float64, heldDigest string, running float64,
) OfferVerdict {
	faults := []OfferFault{}
	heldDigest = js.Trim(heldDigest)
	digest := js.Trim(offeredDigest)

	if !decoded {
		why := js.Trim(decodeError)
		message := "This response is not a desired state"
		if why != "" {
			message += ": " + why
		}
		message += ". It has not been written to disk, and this robot carries on reconciling against what it " +
			"already has — which is what it does whenever it cannot reach the Hub at all (§34.1), so " +
			"nothing about the robot is worse for it."
		faults = append(faults, OfferFault{Code: "undecodable", Severity: "error", Message: message})
		return OfferVerdict{Adopt: false, Changed: false, Generation: heldGeneration, Faults: faults}
	}

	// The reused generation. §11 rests idempotence on that counter, so adopting a *different*
	// document under a generation already applied would have the robot running new software while
	// reporting the old generation as applied — and a fleet console would show it converged.
	if offeredGeneration == heldGeneration && digest != "" && heldDigest != "" && digest != heldDigest {
		faults = append(faults, OfferFault{
			Code: "generation_reused", Severity: "error",
			Message: "The Hub is serving a different document under generation " +
				js.NumberToString(offeredGeneration) + ", which this robot has " +
				"already applied. §11 rests idempotence on that counter, so adopting this would have the " +
				"robot running new software while reporting the old generation as applied — and a fleet " +
				"console would show it converged. Bump the generation on the Hub and this robot will take " +
				"it on the next poll.",
		})
		return OfferVerdict{Adopt: false, Changed: false, Generation: heldGeneration, Faults: faults}
	}

	// A generation that goes backwards is a Hub restored from a backup, and a robot that refused to
	// follow one would be a fleet an operator could not recover — permanently, with no remedy short
	// of visiting every machine. So it is adopted, and reported.
	if offeredGeneration < heldGeneration {
		faults = append(faults, OfferFault{
			Code: "generation_went_backwards", Severity: "warning",
			Message: "This robot has applied generation " + js.NumberToString(heldGeneration) +
				" and the Hub is asking for " + js.NumberToString(offeredGeneration) + ". " +
				"Intent has gone backwards, which is what a Hub restored from a backup looks like. It is " +
				"adopted — a robot that refused to follow one would be a fleet nobody could recover — and " +
				"it is reported, because it is worth somebody knowing once.",
		})
	}

	if packages == 0 && running > 0 {
		faults = append(faults, OfferFault{
			Code: "empties_the_robot", Severity: "warning",
			Message: "This desired state names no packages and this robot is running " +
				js.NumberToString(running) + ". That is what an " +
				"operator emptying a robot looks like, and it is also what a Hub serving a stub looks like. " +
				"It is adopted, because stopping is never the dangerous direction (§33.4) — and it is said " +
				"out loud, because the two are indistinguishable from here.",
		})
	}

	return OfferVerdict{
		Adopt:      true,
		Changed:    digest != heldDigest,
		Generation: offeredGeneration,
		Faults:     faults,
	}
}

// ── the corpus ──────────────────────────────────────────────────────────────

func intentOffer(input canonical.Value) (canonical.Value, error) {
	offer := input.Field("offer")
	held := input.Field("held")

	// `decoded?: boolean` compared with `!== true`, so absent, null and any non-boolean are all
	// "did not decode". isTrue is that comparison.
	verdict := AssessOffer(
		isTrue(offer.Field("decoded")),
		stringOr(offer.Field("decode_error"), ""),
		numberOr(offer.Field("generation"), 0),
		stringOr(offer.Field("digest"), ""),
		numberOr(offer.Field("packages"), 0),
		numberOr(held.Field("generation"), 0),
		stringOr(held.Field("digest"), ""),
		numberOr(held.Field("running"), 0),
	)

	faults := canonical.Array()
	for _, fault := range verdict.Faults {
		faults = faults.Push(canonical.Object().
			Set("code", canonical.String(fault.Code)).
			Set("severity", canonical.String(fault.Severity)).
			Set("message", canonical.String(fault.Message)))
	}
	return canonical.Object().
		Set("adopt", canonical.Bool(verdict.Adopt)).
		Set("changed", canonical.Bool(verdict.Changed)).
		Set("generation", canonical.Number(verdict.Generation)).
		Set("faults", faults), nil
}

// stringOr is the reference's `x ?? fallback` for a string field: absent and null both take the
// fallback, and so does a value of another type, which no case supplies and which would otherwise
// read as an empty string either way.
func stringOr(value canonical.Value, fallback string) string {
	text, ok := value.Str()
	if !ok {
		return fallback
	}
	return text
}

// numberOr is the same for a number.
func numberOr(value canonical.Value, fallback float64) float64 {
	number, ok := value.Number()
	if !ok {
		return fallback
	}
	return number
}

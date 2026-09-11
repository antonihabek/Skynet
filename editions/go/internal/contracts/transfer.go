package contracts

import (
	"math"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

// Bulk transfer, both directions (§12, §34.3, §79).
//
// Nothing here transfers anything: no network, no filesystem, no clock. The bearer's facts arrive
// as values, the size of whatever is on the disk arrives as a number, and the digest of what
// arrived arrives already computed — so the same function runs on the robot and on a Hub asking
// whether a robot could have fetched an image.

// TransferPlan is a decision and where to start from.
type TransferPlan struct {
	Decision string
	// From is the byte offset to start at. Zero unless the decision is `resume`.
	From   float64
	Detail string
}

// TransferCheck is what arrived, judged.
type TransferCheck struct {
	Code string
	// OK is whether these bytes may be used — installed, or receipted.
	OK bool
	// Act is `discard` for a fetch and `restart` for an upload. The two words name the same
	// judgement about two different machines' bytes; see CheckUpload.
	Act    bool
	Detail string
}

// bearerName is the reference's `bearer.name ?? '?'`.
func bearerName(bearer canonical.Value) string {
	return stringOr(bearer.Field("name"), "?")
}

// meteringWord is `metering_declared === true ? 'declared' : 'assumed'`.
func meteringWord(bearer canonical.Value) string {
	if isTrue(bearer.Field("metering_declared")) {
		return "declared"
	}
	return "assumed"
}

// wholeBytes is the reference's `Math.max(0, Math.trunc(x ?? 0))`: a byte count is never negative
// and never fractional, and clamping here means no later arithmetic has to ask.
func wholeBytes(value canonical.Value) float64 {
	number, ok := value.Number()
	if !ok || math.IsNaN(number) {
		return 0
	}
	return math.Max(0, math.Trunc(number))
}

// PlanFetch is what to do about one OS image (§12, §34.3).
//
// The order of the tests is the order of cost. Having it already is free; refusing costs nothing;
// waiting for a link costs nothing; and only then is a transfer considered. A version that checked
// metering first would report a robot as waiting for an unmetered bearer to fetch an image it
// already has.
func PlanFetch(subject canonical.Value) TransferPlan {
	digest := js.Trim(stringOr(subject.Field("digest"), ""))
	partial := wholeBytes(subject.Field("partial"))
	size, hasSize := subject.Field("size").Number()

	if isTrue(subject.Field("have")) {
		return TransferPlan{Decision: "have", Detail: "This bundle is already on the disk and has been checked against the manifest. " +
			"Nothing to fetch."}
	}

	if digest == "" {
		return TransferPlan{Decision: "refuse", Detail: "This release names no bundle digest, so there is nothing to check the bytes " +
			"against. §12 binds a bench pass to the exact bytes it was taken against, and bytes with " +
			"no name cannot be bound to — so they are not fetched either."}
	}

	bearer, hasBearer := subject.Get("bearer")
	if !hasBearer || bearer.IsNull() {
		return TransferPlan{Decision: "no_bearer", Detail: "No bearer is up, so there is nothing to fetch over. That is the ordinary state for " +
			"most of a robot's life (§34.1) and nothing about the update is lost by it — " +
			"the release manifest is already here and the transfer resumes when a link returns."}
	}

	if isTrue(bearer.Field("metered")) {
		return TransferPlan{Decision: "wait_for_unmetered", Detail: "An OS image is the largest bulk transfer this system has, and `" + bearerName(bearer) + "` " +
			"is " + meteringWord(bearer) + " metered. It waits for an " +
			"unmetered link (§34.3). Nothing is lost by waiting, and a fleet that downloaded " +
			"hundreds of megabytes per robot over cellular would find out at the end of the month."}
	}

	// A partial at least as large as the whole thing is not a partial of this bundle. Resuming from
	// it would append this image's tail to something else's, and the mismatch would only surface
	// after the entire transfer — which on a poor link is hours.
	if hasSize && canonical.Finite(size) && size > 0 && partial >= size {
		return TransferPlan{Decision: "discard_and_fetch", Detail: "There are " + js.NumberToString(partial) + " bytes here for a bundle the manifest says is " +
			js.NumberToString(size) + ". A partial " +
			"cannot be larger than what it is part of, so this is left over from something else. It " +
			"is discarded and the transfer starts again."}
	}

	if partial > 0 {
		ofSize := ""
		if hasSize {
			ofSize = " of " + js.NumberToString(size)
		}
		return TransferPlan{Decision: "resume", From: partial, Detail: "Resuming at " + js.NumberToString(partial) + " bytes" + ofSize + ". The partial is " +
			"keyed to this bundle's digest, so it cannot be a different image's tail."}
	}

	what := "the bundle"
	if hasSize {
		what = js.NumberToString(size) + " bytes"
	}
	return TransferPlan{Decision: "fetch", Detail: "Fetching " + what + " over `" + bearerName(bearer) + "`."}
}

// CheckBundle asks whether these are the bytes the signed manifest describes (§12).
//
// Size is checked before the digest, and the reason is the sentence rather than the verdict — both
// refuse. A file short of its declared size is a transfer that stopped, which is ordinary and
// resumable; a file of exactly the right size whose digest is wrong is a different set of bytes,
// which is a build somebody re-cut or a transfer somebody interfered with. Reporting the first as a
// digest mismatch would send an engineer looking for tampering every time a link dropped.
//
// `short` keeps the file, because that is exactly what a resume is for. Everything else deletes it:
// bytes that will never hash correctly are bytes a later resume would append to for ever.
func CheckBundle(arrived, expected canonical.Value) TransferCheck {
	want := js.Trim(stringOr(expected.Field("digest"), ""))
	got := js.Trim(stringOr(arrived.Field("digest"), ""))
	size, hasSize := expected.Field("size").Number()
	actual := numberOr(arrived.Field("size"), 0)

	if hasSize && canonical.Finite(size) && actual < size {
		return TransferCheck{Code: "short", Detail: js.NumberToString(actual) + " of " + js.NumberToString(size) +
			" bytes arrived. The transfer stopped early, which is what a link " +
			"dropping looks like. The partial is kept and the next attempt resumes from it."}
	}

	if hasSize && canonical.Finite(size) && actual > size {
		return TransferCheck{Code: "long", Act: true, Detail: js.NumberToString(actual) + " bytes arrived for a bundle the manifest says is " +
			js.NumberToString(size) + ". More than was " +
			"declared is not a transfer that stopped — it is a different artifact, or a proxy that " +
			"served something else. Discarded."}
	}

	if want == "" || got == "" || want != got {
		return TransferCheck{Code: "digest_mismatch", Act: true, Detail: "These bytes do not hash to what the signed release manifest says this bundle is. " +
			"§12 binds every gate it applies to that digest, so bytes that are not it are not " +
			"this release — do not install them, and find out where they came from. Discarded, " +
			"because bytes that will never verify are bytes a resume would append to for ever."}
	}

	return TransferCheck{Code: "match", OK: true, Detail: "These bytes are the release the manifest describes."}
}

// PlanUpload is what to do about one sealed window (§79, §34.3).
//
// The order of the tests is the order of cost, as in PlanFetch. Having a receipt is free; refusing
// costs nothing; waiting for a link costs nothing; asking costs one small request; and only then
// are megabytes considered.
func PlanUpload(subject canonical.Value) TransferPlan {
	digest := js.Trim(stringOr(subject.Field("digest"), ""))
	size := wholeBytes(subject.Field("size"))

	if isTrue(subject.Field("receipt")) {
		return TransferPlan{Decision: "done", Detail: "The receiver has acknowledged this window by digest and the receipt is on the " +
			"disk. Nothing to send."}
	}

	if digest == "" {
		return TransferPlan{Decision: "refuse", Detail: "This window carries no digest, so there is no address to send it to and nothing to " +
			"check the far side's answer against. §79.1 makes the digest the route, and bytes with " +
			"no name have nowhere to go."}
	}

	if size == 0 {
		return TransferPlan{Decision: "refuse", Detail: "This window is empty. A seal that produced no bytes is a fact about the recorder " +
			"worth reporting (§77.2) and is not something to upload — a zero-length object under a " +
			"digest would be an artifact an investigator could open and mistake for the window."}
	}

	bearer, hasBearer := subject.Get("bearer")
	if !hasBearer || bearer.IsNull() {
		return TransferPlan{Decision: "no_bearer", Detail: "No bearer is up, so there is nothing to send over. That is the ordinary state for " +
			"most of a robot's life (§34.1) and nothing is lost by it — the window is sealed on the " +
			"disk and the incident naming it is already queued."}
	}

	if isTrue(bearer.Field("metered")) {
		return TransferPlan{Decision: "wait_for_unmetered", Detail: "A sealed window is tens of megabytes and `" + bearerName(bearer) + "` is " +
			meteringWord(bearer) + " metered, so it waits for " +
			"an unmetered link (§34.3). The alarm does not wait: §79.3 puts the incident on any " +
			"bearer that is up, so an operator already knows this recording exists and what it " +
			"hashes to."}
	}

	held, hasHeld := subject.Get("held")
	if !hasHeld || held.IsNull() {
		return TransferPlan{Decision: "probe", Detail: "The receiver has not been asked how much of this window it holds. §79.2: the " +
			"partial is on the other machine, so an upload has a step a download does not — and a " +
			"robot that skipped it would re-send the whole window every pass."}
	}

	bytes := wholeBytes(held.Field("bytes"))
	heldDigest := js.Trim(stringOr(held.Field("digest"), ""))

	if bytes >= size {
		if bytes == size && heldDigest != "" && heldDigest == digest {
			return TransferPlan{Decision: "done", Detail: "The receiver holds all " + js.NumberToString(size) +
				" bytes and says they hash to this window's digest. " +
				"Nothing is sent and a receipt is written — which is the state a robot reaches after " +
				"an upload whose acknowledgement was lost."}
		}
		why := ". A partial cannot be larger than what it is part of, so what is over there is not " +
			"part of this window"
		if bytes == size {
			why = ", and does not say what they hash to. A complete object nobody will name is not " +
				"evidence this window arrived, and writing a receipt for it would record an upload " +
				"that may never have happened"
		}
		return TransferPlan{Decision: "restart", Detail: "The receiver claims " + js.NumberToString(bytes) + " bytes for a window of " +
			js.NumberToString(size) + why + ". It is sent again from the beginning."}
	}

	if bytes > 0 {
		return TransferPlan{Decision: "resume", From: bytes, Detail: "Resuming at " + js.NumberToString(bytes) + " of " + js.NumberToString(size) +
			" bytes. The offset is the receiver's claim and is " +
			"used to place the next byte, never to conclude the window arrived — that is the " +
			"digest's job (§79.2)."}
	}

	return TransferPlan{Decision: "start", Detail: "Sending " + js.NumberToString(size) + " bytes over `" + bearerName(bearer) + "`."}
}

// CheckUpload asks whether the window arrived (§79.2, §79.4).
//
// The same four outcomes as CheckBundle and one different remedy, which is the asymmetry worth
// naming: a bundle that fails its check is **deleted**, because the robot owns those bytes. A
// window that fails this check cannot be deleted from anywhere — the robot can only decline to
// trust the offset it was given and begin again. `restart` is therefore about what this robot will
// do next, never about what the receiver should do with what it has.
func CheckUpload(reported, window canonical.Value) TransferCheck {
	want := js.Trim(stringOr(window.Field("digest"), ""))
	got := js.Trim(stringOr(reported.Field("digest"), ""))
	size := wholeBytes(window.Field("size"))
	bytes := wholeBytes(reported.Field("bytes"))

	if bytes < size {
		return TransferCheck{Code: "short", Detail: "The receiver holds " + js.NumberToString(bytes) + " of " + js.NumberToString(size) +
			" bytes. The transfer stopped early, which is " +
			"what a link dropping looks like, and the next pass resumes from what is there."}
	}

	if bytes > size {
		return TransferCheck{Code: "long", Act: true, Detail: "The receiver holds " + js.NumberToString(bytes) + " bytes for a window of " +
			js.NumberToString(size) + ". More than was sent is " +
			"not a transfer that stopped — it is another object under this digest, or a proxy " +
			"answering for one. The next attempt begins at zero; this robot cannot delete what is " +
			"over there and does not pretend to."}
	}

	if want == "" || got == "" || want != got {
		return TransferCheck{Code: "digest_mismatch", Act: true, Detail: "The receiver holds the right number of bytes and they are not this window's. The " +
			"digest was recorded at the seal, before any transfer existed (§77.1), so it is the " +
			"half of this comparison that is evidence. No receipt is written and the next attempt " +
			"begins at zero."}
	}

	return TransferCheck{Code: "complete", OK: true, Detail: "The receiver holds the whole window and it hashes to what was sealed. A receipt is " +
		"written beside the recording; the recording itself is kept, because §39.1's local route " +
		"is for the operator standing next to the robot (§79.4)."}
}

// ── the corpus ──────────────────────────────────────────────────────────────

func shapePlan(plan TransferPlan) canonical.Value {
	return canonical.Object().
		Set("decision", canonical.String(plan.Decision)).
		Set("from", canonical.Number(plan.From)).
		Set("detail", canonical.String(plan.Detail))
}

func transferFetch(input canonical.Value) (canonical.Value, error) {
	return shapePlan(PlanFetch(input.Field("subject"))), nil
}

func transferUpload(input canonical.Value) (canonical.Value, error) {
	return shapePlan(PlanUpload(input.Field("subject"))), nil
}

func transferArrival(input canonical.Value) (canonical.Value, error) {
	check := CheckBundle(input.Field("arrived"), input.Field("expected"))
	return canonical.Object().
		Set("code", canonical.String(check.Code)).
		Set("ok", canonical.Bool(check.OK)).
		Set("discard", canonical.Bool(check.Act)).
		Set("detail", canonical.String(check.Detail)), nil
}

func transferReceipt(input canonical.Value) (canonical.Value, error) {
	check := CheckUpload(input.Field("reported"), input.Field("window"))
	return canonical.Object().
		Set("code", canonical.String(check.Code)).
		Set("ok", canonical.Bool(check.OK)).
		Set("restart", canonical.Bool(check.Act)).
		Set("detail", canonical.String(check.Detail)), nil
}

package contracts

import (
	"math"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

// Observation assembly (§25.1).
//
// §25.1 calls this "one of the harder parts of the whole system", and the reason is worth keeping
// in front of you:
//
//	Feed a policy the most recent value from each stream and it receives **a frame that never
//	existed**, in which the arm is where it is *now* and the image shows where it was two frames
//	ago. Policies degrade in ways that look like bad training.
//
// The last clause is the expensive part. A skewed frame does not announce itself: it presents as a
// policy that grasps slightly late, or oscillates on contact, or works on the bench and not on the
// robot — every one of which sends somebody to retrain a model that was fine. This module does not
// synchronise anything and §25.1 is explicit that it should not try. It makes the skew **measured
// and visible** instead of silently folded into the policy's behaviour.
//
// # Arithmetic
//
// The Rust edition computes this in i128, exactly, at any stamp a u64 can hold. The oracle computes
// it in `number`, exact for integers below 2^53 — about 104 days of monotonic uptime in nanoseconds
// — and this edition follows the oracle, in float64, because agreeing with the oracle is the
// requirement. No case can reach the boundary: JSON.parse has already rounded any stamp above 2^53
// before the generator writes it.

const nsPerMs = 1000000.0

// Sample is one sample from one stream.
type Sample struct {
	// StampNS is CLOCK_MONOTONIC_RAW at **acquisition**, never at return or arrival (§24, §47.2).
	StampNS float64
	Seq     float64
}

// Stream is one stream the policy asked for, and what is available on it.
type Stream struct {
	Name     string
	Modality string
	// Latency is the declared per-sensor latency (§52) in milliseconds. Absent means nobody
	// declared it — neither a failure nor a zero, which is why presence is carried separately.
	Latency    float64
	HasLatency bool
	// Uncertainty is how wrong Latency could be (§47.2). Supplied by the capture pipeline, not by
	// the descriptor: a backend that stamps at capture knows the figure precisely, one that
	// estimates knows it badly.
	Uncertainty    float64
	HasUncertainty bool
	// Budget is how old a sample may be before this stream is unusable. The same camera is fine at
	// 200 ms for a mapping policy and useless at 40 ms for a contact-rich manipulation one.
	Budget    float64
	HasBudget bool
	Samples   []Sample
}

// Chosen is the sample the assembler picked, and what accepting it cost.
type Chosen struct {
	StampNS     float64
	Seq         float64
	EffectiveNS float64
	// AgeMS is how far behind the target instant that is. **Signed** — negative means the sample
	// describes an instant after the one the frame claims to be, which cannot happen live and
	// happens routinely in replay.
	AgeMS      float64
	OverBudget bool
	Considered int
}

// Hold is why a frame cannot be delivered.
type Hold struct {
	Stream string
	Reason string
	Detail string
}

// StreamFrame is what one stream contributed.
type StreamFrame struct {
	Stream    Stream
	Chosen    Chosen
	HasChosen bool
}

// ObservationFrame is one assembled observation frame.
type ObservationFrame struct {
	TargetNS float64
	Streams  []StreamFrame
	// SkewMS is the spread between the earliest and latest instants actually in the frame. Absent
	// when fewer than two streams contributed: the spread of one instant is not zero, it is
	// undefined, and zero would let a half-empty frame look perfectly synchronised.
	SkewMS  float64
	HasSkew bool
	// SkewUpperMS is the spread once each stream's uncertainty is taken at its worst. Absent when
	// any latency is undeclared: the widening is then unbounded, and §47.2's "larger uncertainty"
	// has no largest.
	SkewUpperMS       float64
	HasSkewUpper      bool
	UndeclaredLatency []string
	UndeclaredBudget  []string
	Holds             []Hold
	Verdict           string
	Deliverable       bool
}

// roundHalfAway rounds half away from zero.
//
// **Not js.Round**, which rounds half toward +∞. The Rust edition cannot use f64::round either,
// because it is unavailable under no_std without a maths library, and §31.2's rule about platform
// maths applies with full force to a rounding inside a latency correction: two editions rounding a
// half-nanosecond differently is a one-nanosecond disagreement today and a *different chosen
// sample* the day two candidates are equidistant. So every edition writes the same four lines out.
func roundHalfAway(value float64) float64 {
	if !canonical.Finite(value) {
		return 0
	}
	truncated := math.Trunc(value)
	fraction := value - truncated
	if fraction >= 0.5 {
		return truncated + 1
	}
	if fraction <= -0.5 {
		return truncated - 1
	}
	return truncated
}

// micros is milliseconds as whole microseconds, saturating at zero and at the top of a u32.
//
// The ceiling is not a real case — seventy-one minutes of skew — and it is here because the Rust
// edition's alternative in a release build is a wrap, and a frame recorded as four microseconds
// skewed when it was four thousand seconds skewed is worse than one recorded at the ceiling.
func micros(milliseconds float64) float64 {
	value := roundHalfAway(milliseconds * 1000)
	if value <= 0 {
		return 0
	}
	if value >= 4294967295 {
		return 4294967295
	}
	return value
}

// effectiveNS is the instant a sample describes: its stamp, less the declared latency.
func effectiveNS(stampNS float64, latency float64, hasLatency bool) float64 {
	if !hasLatency {
		return stampNS
	}
	return stampNS - roundHalfAway(latency*nsPerMs)
}

// milliseconds converts nanoseconds. One division of an exact integer, so every edition performing
// it gets the same bits: IEEE-754 division is correctly rounded everywhere, which is the property
// §31.2 leans on for the kernel's sin and cos.
func milliseconds(nanos float64) float64 { return nanos / nsPerMs }

// AssembleObservation assembles one observation frame for the given instant (§25.1).
//
// For each stream: correct every sample for the declared latency, pick the one nearest the target,
// and measure how far behind the target it is. Then measure the spread across the streams, which is
// the frame's own internal inconsistency.
//
// # Nearest, and what happens on a tie
//
// Nearest is by absolute distance, so a sample *after* the target can be chosen when it is
// genuinely closer — impossible on a live robot, correct in replay.
//
// A tie takes the **earlier** sample. Two samples equidistant either side of the target are equally
// good by the distance rule and are not equally good in any other sense: the earlier describes
// something that had already happened at the instant the frame claims to describe, and the later
// does not. The tie-break also has to exist for a duller reason — without it, two editions
// iterating in different orders would pick different samples and the corpus would diverge on an
// input where nothing is wrong.
func AssembleObservation(targetNS float64, streams []Stream) ObservationFrame {
	frames := make([]StreamFrame, 0, len(streams))
	holds := []Hold{}
	undeclaredLatency := []string{}
	undeclaredBudget := []string{}

	for _, stream := range streams {
		if !stream.HasLatency {
			undeclaredLatency = append(undeclaredLatency, stream.Name)
		}
		if !stream.HasBudget {
			undeclaredBudget = append(undeclaredBudget, stream.Name)
		}

		var bestEffective float64
		var bestSample Sample
		haveBest := false
		for _, sample := range stream.Samples {
			effective := effectiveNS(sample.StampNS, stream.Latency, stream.HasLatency)
			distance := math.Abs(effective - targetNS)
			better := !haveBest
			if haveBest {
				bestDistance := math.Abs(bestEffective - targetNS)
				// Strictly nearer wins; on a tie the earlier instant wins. Never "the one seen
				// first", which would make the answer depend on the order the capture ring
				// happened to be walked.
				better = distance < bestDistance || (distance == bestDistance && effective < bestEffective)
			}
			if better {
				bestEffective, bestSample, haveBest = effective, sample, true
			}
		}

		entry := StreamFrame{Stream: stream}
		if !haveBest {
			holds = append(holds, Hold{
				Stream: stream.Name,
				Reason: "no_sample",
				Detail: stream.Name + " (" + stream.Modality + ") offered no sample for this instant, so there " +
					"is no frame to assemble. A stream the policy declared and the pipeline is not " +
					"producing is a capture path that is not running.",
			})
		} else {
			age := milliseconds(targetNS - bestEffective)
			// One-sided, and deliberately so. The budget bounds how far *behind* the target a
			// sample may be; a sample from ahead of it is a clock fault rather than a stale one, it
			// is visible in the signed age, and reporting it as staleness would send somebody to
			// look at a capture pipeline that is working.
			overBudget := stream.HasBudget && age > stream.Budget
			if overBudget {
				budget := stream.Budget
				if !stream.HasBudget {
					budget = 0
				}
				holds = append(holds, Hold{
					Stream: stream.Name,
					Reason: "stale",
					Detail: stream.Name + " (" + stream.Modality + ") is " + js.NumberToString(age) +
						" ms behind the frame's instant and the policy's budget for it is " +
						js.NumberToString(budget) + " ms. §25.1: exceeding it is a hold, not a " +
						"silently older frame.",
				})
			}
			entry.Chosen = Chosen{
				StampNS:     bestSample.StampNS,
				Seq:         bestSample.Seq,
				EffectiveNS: bestEffective,
				AgeMS:       age,
				OverBudget:  overBudget,
				Considered:  len(stream.Samples),
			}
			entry.HasChosen = true
		}
		frames = append(frames, entry)
	}

	frame := ObservationFrame{
		TargetNS:          targetNS,
		Streams:           frames,
		UndeclaredLatency: undeclaredLatency,
		UndeclaredBudget:  undeclaredBudget,
		Holds:             holds,
		// §25.1: exceeding a staleness budget "is a hold, not a silently older frame". So is an
		// empty stream. Everything else — an undeclared latency, an undeclared budget, an unbounded
		// skew — is reported and does not block, because none of them is evidence that the frame is
		// wrong. They are evidence that nobody can say how wrong it is, a different sentence.
		Verdict:     "ok",
		Deliverable: len(holds) == 0,
	}
	if len(holds) != 0 {
		frame.Verdict = "hold"
	}
	frame.SkewMS, frame.HasSkew, frame.SkewUpperMS, frame.HasSkewUpper = measureSkew(frames, undeclaredLatency)
	return frame
}

func measureSkew(frames []StreamFrame, undeclaredLatency []string) (float64, bool, float64, bool) {
	var lowestInstant, lowestUncertainty, highestInstant, highestUncertainty float64
	haveLowest, haveHighest := false, false
	contributors := 0

	for _, frame := range frames {
		if !frame.HasChosen {
			continue
		}
		contributors++
		uncertainty := 0.0
		if frame.Stream.HasUncertainty {
			uncertainty = frame.Stream.Uncertainty
		}
		if !haveLowest || frame.Chosen.EffectiveNS < lowestInstant {
			lowestInstant, lowestUncertainty, haveLowest = frame.Chosen.EffectiveNS, uncertainty, true
		}
		if !haveHighest || frame.Chosen.EffectiveNS > highestInstant {
			highestInstant, highestUncertainty, haveHighest = frame.Chosen.EffectiveNS, uncertainty, true
		}
	}

	// One instant has no spread, and reporting zero would let a frame with a single working stream
	// look perfectly synchronised — the presentation §25.1 is written against.
	if contributors < 2 || !haveLowest || !haveHighest {
		return 0, false, 0, false
	}

	measured := milliseconds(highestInstant - lowestInstant)
	if len(undeclaredLatency) > 0 {
		return measured, true, 0, false
	}
	return measured, true, measured + lowestUncertainty + highestUncertainty, true
}

// LatestCommonInstant is the most recent instant at which every stream has a sample.
//
// A live control loop has to choose a target, and choosing badly defeats the whole section: aim at
// *now* and the slowest camera is always the stalest stream in every frame, so every frame is
// skewed by that camera's latency and the assembler reports a problem the loop created.
//
// Absent when any stream has no sample at or before the given instant — the same fact
// AssembleObservation reports as a `no_sample` hold. Offered rather than imposed: a runtime may
// target an older instant deliberately. What this exists to prevent is two editions inventing two
// different defaults and disagreeing about frames neither of them assembled wrongly.
func LatestCommonInstant(nowNS float64, streams []Stream) (float64, bool) {
	var earliest float64
	haveEarliest := false

	for _, stream := range streams {
		var latest float64
		haveLatest := false
		for _, sample := range stream.Samples {
			effective := effectiveNS(sample.StampNS, stream.Latency, stream.HasLatency)
			if effective > nowNS {
				continue
			}
			if !haveLatest || effective > latest {
				latest, haveLatest = effective, true
			}
		}
		// A stream with nothing at or before `now` cannot contribute to any frame at or before
		// `now`, so there is no common instant at all — not a later one, and not one that quietly
		// drops it.
		if !haveLatest {
			return 0, false
		}
		if !haveEarliest || latest < earliest {
			earliest, haveEarliest = latest, true
		}
	}

	// No streams at all: every instant is common to all of them.
	answer := nowNS
	if haveEarliest {
		answer = earliest
	}
	// A declared latency larger than the stamp itself. The correction is still right and the
	// instant it names is before the monotonic clock started, which no u64 can express.
	if answer < 0 {
		return 0, false
	}
	return answer, true
}

// ── the corpus ──────────────────────────────────────────────────────────────

func readStreams(value canonical.Value) []Stream {
	streams := make([]Stream, 0, value.Len())
	for _, entry := range value.Items() {
		stream := Stream{}
		stream.Name, _ = entry.Field("name").Str()
		stream.Modality, _ = entry.Field("modality").Str()
		// `latency_ms?: number | null` — the type admits null, so a null is a declared absence
		// rather than corpus/README.md's NaN reading. Reading it as a number is what tells them
		// apart, and the `has` flag carries the difference the whole module turns on.
		stream.Latency, stream.HasLatency = entry.Field("latency_ms").Number()
		stream.Uncertainty, stream.HasUncertainty = entry.Field("uncertainty_ms").Number()
		stream.Budget, stream.HasBudget = entry.Field("budget_ms").Number()
		for _, sample := range entry.Field("samples").Items() {
			stamp, _ := sample.Field("stamp_ns").Number()
			seq, _ := sample.Field("seq").Number()
			stream.Samples = append(stream.Samples, Sample{StampNS: stamp, Seq: seq})
		}
		streams = append(streams, stream)
	}
	return streams
}

func shapeObservation(frame ObservationFrame) canonical.Value {
	streams := canonical.Array()
	for _, entry := range frame.Streams {
		shaped := canonical.Object().
			Set("name", canonical.String(entry.Stream.Name)).
			Set("modality", canonical.String(entry.Stream.Modality)).
			Set("latency_ms", optionalNumber(entry.Stream.Latency, entry.Stream.HasLatency)).
			Set("uncertainty_ms", optionalNumber(entry.Stream.Uncertainty, entry.Stream.HasUncertainty)).
			Set("budget_ms", optionalNumber(entry.Stream.Budget, entry.Stream.HasBudget)).
			Set("age_ms", optionalNumber(entry.Chosen.AgeMS, entry.HasChosen)).
			Set("considered", canonical.Number(float64(entry.Chosen.Considered))).
			Set("over_budget", canonical.Bool(entry.HasChosen && entry.Chosen.OverBudget)).
			Set("seq", optionalNumber(entry.Chosen.Seq, entry.HasChosen)).
			Set("stamp_ns", optionalNumber(entry.Chosen.StampNS, entry.HasChosen))
		if !entry.HasChosen {
			// `considered` is 0 rather than null for a stream that offered nothing: the count of
			// what was available is a number either way, and the reference writes it as one.
			shaped = shaped.Set("considered", canonical.Number(0))
		}
		streams = streams.Push(shaped)
	}

	holds := canonical.Array()
	for _, hold := range frame.Holds {
		holds = holds.Push(canonical.Object().
			Set("stream", canonical.String(hold.Stream)).
			Set("reason", canonical.String(hold.Reason)).
			Set("detail", canonical.String(hold.Detail)))
	}

	return canonical.Object().
		Set("target_ns", canonical.Number(frame.TargetNS)).
		Set("streams", streams).
		Set("skew_ms", optionalNumber(frame.SkewMS, frame.HasSkew)).
		Set("skew_upper_ms", optionalNumber(frame.SkewUpperMS, frame.HasSkewUpper)).
		Set("undeclared_latency", canonical.Strings(frame.UndeclaredLatency)).
		Set("undeclared_budget", canonical.Strings(frame.UndeclaredBudget)).
		Set("holds", holds).
		Set("verdict", canonical.String(frame.Verdict)).
		Set("deliverable", canonical.Bool(frame.Deliverable)).
		// The projection onto §26's record, inside the frame rather than beside it. It is derived
		// from everything above, and pinning it in a second corpus would let an edition agree about
		// a frame and disagree about the only version of it that survives the day.
		Set("record", shapeObservationRecord(frame))
}

func shapeObservationRecord(frame ObservationFrame) canonical.Value {
	streams := canonical.Array()
	absent := 0
	for _, entry := range frame.Streams {
		if !entry.HasChosen {
			absent++
			continue
		}
		streams = streams.Push(canonical.Object().
			Set("name", canonical.String(entry.Stream.Name)).
			Set("age_us", canonical.Number(micros(entry.Chosen.AgeMS))).
			Set("stale", canonical.Bool(entry.Chosen.OverBudget)))
	}
	worstSkew := canonical.Null()
	if frame.HasSkew {
		worstSkew = canonical.Number(micros(frame.SkewMS))
	}
	return canonical.Object().
		Set("target_ns", canonical.Number(frame.TargetNS)).
		Set("worst_skew_us", worstSkew).
		Set("streams_absent", canonical.Number(float64(absent))).
		Set("streams", streams)
}

func observationFrames(input canonical.Value) (canonical.Value, error) {
	target, _ := input.Field("target_ns").Number()
	return shapeObservation(AssembleObservation(target, readStreams(input.Field("streams")))), nil
}

func observationTarget(input canonical.Value) (canonical.Value, error) {
	now, _ := input.Field("now_ns").Number()
	target, ok := LatestCommonInstant(now, readStreams(input.Field("streams")))
	return canonical.Object().Set("target_ns", optionalNumber(target, ok)), nil
}

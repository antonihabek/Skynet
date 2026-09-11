package contracts

import (
	"math"

	"github.com/antonihabek/Skynet/editions/go/internal/canonical"
	"github.com/antonihabek/Skynet/editions/go/internal/js"
)

// One stream's fixed frame pool (§47.6). Pure bookkeeping: no clocks, no I/O, no mapping.

// FrameIdentity is what a published frame is.
type FrameIdentity struct {
	Stream  string
	StampNS float64
	// Sequence is monotonic within the stream. Gaps are drops, and drops are counted.
	Sequence float64
}

// FrameHandle names a slot and the generation it was issued against.
type FrameHandle struct {
	Stream     string
	Slot       int
	Generation float64
}

// FrameRefusal is a pool saying no, in one of §47.6's six words.
type FrameRefusal struct {
	Code    string
	Message string
}

type poolSlot struct {
	generation  float64
	identity    FrameIdentity
	hasIdentity bool
	acquired    bool
}

// FramePool is one stream's pool.
type FramePool struct {
	Stream    string
	Capacity  int
	slots     []poolSlot
	published float64
	dropped   float64
	acquired  float64
	released  float64
}

// NewFramePool builds a pool of the given capacity.
func NewFramePool(stream string, capacity int) *FramePool {
	return &FramePool{Stream: stream, Capacity: capacity, slots: make([]poolSlot, capacity)}
}

// oldestReleased is which slot the next publish takes: the released one holding the oldest frame.
//
// This is the entire mechanism by which a pool is a pool, and getting it wrong costs §47.6 its
// reason to exist. The obvious implementation — the first slot that is not currently acquired — is
// wrong in a way no single-publish test can see: with nothing acquired it returns slot 0 every
// time, so a pool of any declared capacity uses one slot and the others are never written, which
// makes it a duplicate of §47.4's one-slot latest-wins channel.
//
// The cost lands on the reader, not on capture, which is why it presents as somebody else's bug. A
// policy holds a frame by calling Latest and then Acquire, and between those two calls capture is
// free to publish. Taking the first free slot means that publish lands on the slot Latest just
// named, so the acquire is refused `stale_handle` on nearly every cycle and the pool reports a
// reader that cannot keep up.
//
// Empty slots go first because there is nothing in them to destroy, which is also what makes a
// declared capacity mean what it says: a pool fills before it starts overwriting. After that,
// oldest is by the frame's **sequence** and never by the slot's index — slot order and arrival
// order stop agreeing the first time a reader holds one and lets it go. Ties break to the lower
// index so that two editions cannot disagree about a pool nothing has published into yet.
//
// Returns -1 when every slot is acquired, which is `pool_exhausted`.
func (pool *FramePool) oldestReleased() int {
	chosen := -1
	chosenSequence := math.Inf(1)
	for index := range pool.slots {
		slot := &pool.slots[index]
		if slot.acquired {
			continue
		}
		if !slot.hasIdentity {
			return index
		}
		if slot.identity.Sequence < chosenSequence {
			chosenSequence = slot.identity.Sequence
			chosen = index
		}
	}
	return chosen
}

// Publish stores a frame, returning its handle or the refusal.
func (pool *FramePool) Publish(frame FrameIdentity) (FrameHandle, FrameRefusal, bool) {
	if frame.Stream != pool.Stream {
		return FrameHandle{}, FrameRefusal{
			Code:    "unknown_stream",
			Message: "This pool holds " + pool.Stream + " and was handed a frame from " + frame.Stream + ".",
		}, false
	}
	index := pool.oldestReleased()
	if index < 0 {
		pool.dropped++
		return FrameHandle{}, FrameRefusal{
			Code: "pool_exhausted",
			Message: "Every slot is acquired, so frame " + js.NumberToString(frame.Sequence) +
				" was dropped and counted rather than stalling capture.",
		}, false
	}
	slot := &pool.slots[index]
	slot.generation++
	slot.identity, slot.hasIdentity = frame, true
	slot.acquired = false
	pool.published++
	return FrameHandle{Stream: pool.Stream, Slot: index, Generation: slot.generation}, FrameRefusal{}, true
}

// Latest is the newest published frame's handle, or absent when nothing has been published.
func (pool *FramePool) Latest() (FrameHandle, bool) {
	var best FrameHandle
	haveBest := false
	bestSequence := -1.0
	for index := range pool.slots {
		slot := &pool.slots[index]
		if slot.hasIdentity && slot.identity.Sequence > bestSequence {
			bestSequence = slot.identity.Sequence
			best, haveBest = FrameHandle{Stream: pool.Stream, Slot: index, Generation: slot.generation}, true
		}
	}
	return best, haveBest
}

// Acquire takes a read loan on the named frame. The slot is pinned until Release.
func (pool *FramePool) Acquire(handle FrameHandle) (FrameIdentity, FrameRefusal, bool) {
	slot, ok := pool.slotFor(handle)
	if !ok {
		return FrameIdentity{}, FrameRefusal{
			Code: "unknown_slot", Message: "The handle names a slot this pool does not hold.",
		}, false
	}
	if !slot.hasIdentity || handle.Generation != slot.generation {
		return FrameIdentity{}, FrameRefusal{
			Code: "stale_handle",
			Message: "The slot has been reused since this handle was issued, so the handle names a " +
				"frame that is no longer there.",
		}, false
	}
	if slot.acquired {
		return FrameIdentity{}, FrameRefusal{
			Code:    "already_acquired",
			Message: "This handle is already checked out; a handle is one loan, not a share.",
		}, false
	}
	slot.acquired = true
	pool.acquired++
	return slot.identity, FrameRefusal{}, true
}

// Release returns a loan. Refuses unknown, stale and already-released handles by name.
func (pool *FramePool) Release(handle FrameHandle) (FrameRefusal, bool) {
	slot, ok := pool.slotFor(handle)
	if !ok {
		return FrameRefusal{
			Code: "unknown_slot", Message: "The handle names a slot this pool does not hold.",
		}, false
	}
	if !slot.hasIdentity || handle.Generation != slot.generation {
		return FrameRefusal{
			Code:    "stale_handle",
			Message: "The slot has been reused since this handle was issued; there is nothing of it left to release.",
		}, false
	}
	if !slot.acquired {
		return FrameRefusal{
			Code:    "double_release",
			Message: "This handle was already released; releasing it twice would unpin a later loan.",
		}, false
	}
	slot.acquired = false
	pool.released++
	return FrameRefusal{}, true
}

// slotFor is the reference's `this.slots[handle.slot]` plus its `!slot` guard. JavaScript indexing
// out of range yields undefined; Go panics, so the bound is checked rather than relied upon.
func (pool *FramePool) slotFor(handle FrameHandle) (*poolSlot, bool) {
	if handle.Stream != pool.Stream || handle.Slot < 0 || handle.Slot >= len(pool.slots) {
		return nil, false
	}
	return &pool.slots[handle.Slot], true
}

// ── the corpus ──────────────────────────────────────────────────────────────

func shapeHandle(handle FrameHandle, present bool) canonical.Value {
	if !present {
		return canonical.Null()
	}
	return canonical.Object().
		Set("stream", canonical.String(handle.Stream)).
		Set("slot", canonical.Number(float64(handle.Slot))).
		Set("generation", canonical.Number(handle.Generation))
}

func shapeRefusal(refusal FrameRefusal) canonical.Value {
	return canonical.Object().
		Set("code", canonical.String(refusal.Code)).
		Set("message", canonical.String(refusal.Message))
}

func (pool *FramePool) shapeStats() canonical.Value {
	return canonical.Object().
		Set("published", canonical.Number(pool.published)).
		Set("dropped", canonical.Number(pool.dropped)).
		Set("acquired", canonical.Number(pool.acquired)).
		Set("released", canonical.Number(pool.released))
}

func mediaPool(input canonical.Value) (canonical.Value, error) {
	stream, _ := input.Field("stream").Str()
	capacity, _ := input.Field("capacity").Number()
	pool := NewFramePool(stream, int(capacity))

	// The handle each step produced, so a later step can name one by its index in `ops` — which is
	// what makes a case readable top to bottom instead of counting only the steps that yielded one.
	handles := []FrameHandle{}
	handlePresent := []bool{}
	steps := canonical.Array()

	for _, op := range input.Field("ops").Items() {
		kind, _ := op.Field("op").Str()
		step := canonical.Object().Set("op", canonical.String(kind))
		var produced FrameHandle
		producedPresent := false

		switch kind {
		case "publish":
			frameStream := stream
			if named, ok := op.Field("stream").Str(); ok {
				frameStream = named
			}
			stamp, _ := op.Field("stamp_ns").Number()
			sequence, _ := op.Field("sequence").Number()
			handle, refusal, ok := pool.Publish(FrameIdentity{
				Stream: frameStream, StampNS: stamp, Sequence: sequence,
			})
			if ok {
				step = step.Set("handle", shapeHandle(handle, true))
				produced, producedPresent = handle, true
			} else {
				step = step.Set("refusal", shapeRefusal(refusal))
			}
		case "latest":
			handle, ok := pool.Latest()
			step = step.Set("handle", shapeHandle(handle, ok))
			produced, producedPresent = handle, ok
		case "acquire", "release":
			named, ok := namedHandle(op, handles, handlePresent, pool.Stream)
			if !ok {
				// The reference throws here, and a generator that produced such a case would have
				// failed rather than written it. Reported as an error so a case that somehow
				// carries one is a red case naming itself, not a silent pass.
				return canonical.Null(), errNoHandle(kind)
			}
			if kind == "acquire" {
				identity, refusal, okAcquire := pool.Acquire(named)
				if okAcquire {
					step = step.Set("identity", canonical.Object().
						Set("stream", canonical.String(identity.Stream)).
						Set("stamp_ns", canonical.Number(identity.StampNS)).
						Set("sequence", canonical.Number(identity.Sequence)))
				} else {
					step = step.Set("refusal", shapeRefusal(refusal))
				}
			} else {
				refusal, okRelease := pool.Release(named)
				if okRelease {
					step = step.Set("released", canonical.Bool(true))
				} else {
					step = step.Set("refusal", shapeRefusal(refusal))
				}
			}
		case "stats":
			step = step.Set("stats", pool.shapeStats())
		}

		handles = append(handles, produced)
		handlePresent = append(handlePresent, producedPresent)
		steps = steps.Push(step)
	}

	latest, haveLatest := pool.Latest()
	return canonical.Object().
		Set("steps", steps).
		Set("final", canonical.Object().
			Set("latest", shapeHandle(latest, haveLatest)).
			Set("stats", pool.shapeStats())), nil
}

// namedHandle is the reference's `op.handle ?? handles[op.from ?? -1] ?? null`.
func namedHandle(op canonical.Value, handles []FrameHandle, present []bool, stream string) (FrameHandle, bool) {
	if literal, ok := op.Get("handle"); ok && literal.Kind() == canonical.KindObject {
		handleStream, _ := literal.Field("stream").Str()
		if handleStream == "" {
			handleStream = stream
		}
		slot, _ := literal.Field("slot").Number()
		generation, _ := literal.Field("generation").Number()
		return FrameHandle{Stream: handleStream, Slot: int(slot), Generation: generation}, true
	}
	from, ok := op.Field("from").Number()
	if !ok {
		return FrameHandle{}, false
	}
	index := int(from)
	if index < 0 || index >= len(handles) || !present[index] {
		return FrameHandle{}, false
	}
	return handles[index], true
}

type noHandleError string

func (e noHandleError) Error() string { return string(e) }

func errNoHandle(kind string) error {
	return noHandleError(kind + " names no handle, which the reference throws on rather than answering")
}

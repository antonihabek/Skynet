/**
 * Frame handles for the sensor and media pipeline (§47.6).
 *
 * §47.4 gives the policy a latest-wins slot and the black box a bounded queue, but never says
 * how a frame crosses from OS-owned capture memory into a sandboxed policy in another process.
 * Copying megapixel frames through a ring sized for joint vectors is not the answer (§80), so
 * frames stay where capture put them and what crosses is a *handle*: enough to map the frame
 * read-only, and nothing that can name a different frame.
 *
 * The contract, in full:
 *
 * - The pool owns a fixed number of slots per stream. Fixed, because §47.5 charges capture
 *   memory to the OS budget and an unbounded pool is a slow leak presenting as a policy OOM.
 * - `publish` writes into the oldest *released* slot. A slot still acquired by a reader is never
 *   reused under it — reusing it would mutate a frame somebody is inferring on.
 * - When every slot is acquired, the frame is **dropped and the drop is counted**. Capture is
 *   never stalled (§47.4): starving the policy to protect the recording is precisely backwards,
 *   and starving capture to protect a slow reader is the same error one step over.
 * - Every slot carries a **generation** that advances on each reuse. A handle names
 *   (stream, slot, generation), so a reader holding yesterday's handle onto a reused slot is
 *   refused rather than shown somebody else's frame. ABA protection is not optional here: slot
 *   reuse is the normal case, not the edge.
 * - Handles are single-owner loans, not reference counts. `release` on an unknown, stale or
 *   already-released handle is refused and named — a reader that cannot track its own loans is
 *   a reader whose next frame cannot be trusted either.
 * - An empty pool reads as "no frame yet" (`latest()` returns null), never as a frame of zeros.
 *   A zero frame is a measurement nobody took.
 *
 * This module is the bookkeeping half. The mapping half — read-only `mmap` of the named slot in
 * the policy's address space — is per-OS code that cannot be expressed here; what it must honour
 * is that a handle grants read access to exactly one generation of one slot, and that the pool
 * may reuse the slot the moment it is released.
 */

export interface FrameIdentity {
  /** The stream this frame arrived on, e.g. `head_rgb`. */
  stream: string
  /** Acquisition timestamp, nanoseconds, from the sensor — never arrival (§47.2). */
  stamp_ns: number
  /** Monotonic sequence within the stream. Gaps are drops, and drops are counted. */
  sequence: number
}

export interface FrameHandle {
  stream: string
  /** Index into the pool's slots. */
  slot: number
  /** The generation of the slot this handle was issued against. */
  generation: number
}

/**
 * Everything a frame pool can refuse, in the order a reader meets them.
 *
 * A runtime array rather than a bare union, because §31.4 freezes the closed vocabularies as
 * corpus cases and an edition that spelled one of these differently would be refusing a reader in
 * a word nothing else in the system knows. The last member is the odd one and is deliberately in
 * the same set: `pool_exhausted` refuses *capture*, not the reader, and it is the only one of the
 * six that is a normal operating condition rather than a defect somewhere.
 */
export const FRAME_REFUSAL_CODES = [
  'unknown_stream',
  'unknown_slot',
  'stale_handle',
  'already_acquired',
  'double_release',
  'pool_exhausted',
] as const

export type FrameRefusalCode = (typeof FRAME_REFUSAL_CODES)[number]

export interface FrameRefusal {
  code: FrameRefusalCode
  message: string
}

export interface FramePoolStats {
  published: number
  /** Frames dropped because every slot was acquired. Recorded, never silent (§47.4). */
  dropped: number
  acquired: number
  released: number
}

interface Slot {
  generation: number
  identity: FrameIdentity | null
  acquired: boolean
}

/**
 * One stream's fixed frame pool. Pure bookkeeping: no clocks, no I/O, no mapping.
 */
export class FramePool {
  readonly stream: string
  readonly capacity: number
  private readonly slots: Slot[]
  private publishedCount = 0
  private droppedCount = 0
  private acquiredCount = 0
  private releasedCount = 0

  constructor(stream: string, capacity: number) {
    if (!stream) throw new Error('a frame pool is for one named stream')
    if (!Number.isInteger(capacity) || capacity < 1) {
      throw new Error('a frame pool holds at least one slot')
    }
    this.stream = stream
    this.capacity = capacity
    this.slots = Array.from({ length: capacity }, () => ({
      generation: 0,
      identity: null,
      acquired: false,
    }))
  }

  /**
   * Store a frame, returning its handle.
   *
   * Reuses the released slot holding the **oldest** frame — see `oldestReleased`, where the whole
   * argument is — and when no slot is released the frame is dropped and counted rather than
   * stalling capture.
   */
  publish(frame: FrameIdentity): { handle: FrameHandle } | { refusal: FrameRefusal } {
    if (frame.stream !== this.stream) {
      return {
        refusal: {
          code: 'unknown_stream',
          message: `This pool holds ${this.stream} and was handed a frame from ${frame.stream}.`,
        },
      }
    }
    const slotIndex = this.oldestReleased()
    if (slotIndex < 0) {
      this.droppedCount += 1
      return {
        refusal: {
          code: 'pool_exhausted',
          message:
            `Every slot is acquired, so frame ${frame.sequence} was dropped and counted ` +
            'rather than stalling capture.',
        },
      }
    }
    const slot = this.slots[slotIndex]
    slot.generation += 1
    slot.identity = frame
    slot.acquired = false
    this.publishedCount += 1
    return { handle: { stream: this.stream, slot: slotIndex, generation: slot.generation } }
  }

  /**
   * Which slot the next publish takes: the released one holding the oldest frame.
   *
   * This is the entire mechanism by which a pool is a pool, and getting it wrong costs the
   * section its reason to exist. The obvious implementation — the first slot that is not
   * currently acquired — is wrong in a way that no single-publish test can see, and the six that
   * were written with this file did not see it: with nothing acquired it returns slot 0 every
   * time, so a pool of any declared capacity uses one slot and the others are never written. §47.4
   * already has a one-slot latest-wins channel, and a §47.6 that degenerated into it would be a
   * duplicate of the section above rather than the thing that section defers to.
   *
   * The cost lands on the reader, not on capture, which is why it presents as somebody else's
   * bug. A policy holds a frame by calling `latest()` and then `acquire()`, and between those two
   * calls capture is free to publish — at any frame rate above the reader's turnaround it *will*.
   * Taking the first free slot means that publish lands on the slot `latest()` just named, so the
   * acquire is refused `stale_handle` on nearly every cycle and the pool reports a reader that
   * cannot keep up. Taking the oldest means the publish lands somewhere with less to lose and the
   * reader gets its frame.
   *
   * Empty slots go first because there is nothing in them to destroy, which is also what makes a
   * declared capacity mean what it says: a pool fills before it starts overwriting. After that,
   * oldest is by the frame's **sequence** and never by the slot's index — slot order and arrival
   * order stop agreeing the first time a reader holds one and lets it go, which is the case
   * `released-slots-go-oldest-first` exists for. Ties break to the lower index so that two
   * editions cannot disagree about a pool nothing has published into yet.
   *
   * Returns `-1` when every slot is acquired, which is `pool_exhausted` and the one refusal here
   * that is not a defect anywhere.
   */
  private oldestReleased(): number {
    let chosen = -1
    let chosenSequence = Number.POSITIVE_INFINITY
    for (let index = 0; index < this.slots.length; index += 1) {
      const slot = this.slots[index]
      if (slot.acquired) continue
      // An empty slot has nothing to lose, so it wins outright and the search stops.
      if (slot.identity === null) return index
      if (slot.identity.sequence < chosenSequence) {
        chosenSequence = slot.identity.sequence
        chosen = index
      }
    }
    return chosen
  }

  /** The newest published frame's handle, or null when nothing has been published yet. */
  latest(): FrameHandle | null {
    let best: FrameHandle | null = null
    let bestSequence = -1
    for (let index = 0; index < this.slots.length; index += 1) {
      const slot = this.slots[index]
      if (slot.identity !== null && slot.identity.sequence > bestSequence) {
        bestSequence = slot.identity.sequence
        best = { stream: this.stream, slot: index, generation: slot.generation }
      }
    }
    return best
  }

  /**
   * Take a read loan on the named frame. The slot is pinned until `release`: publishing
   * will reuse an older released slot first, and drop rather than reuse this one.
   */
  acquire(handle: FrameHandle): { identity: FrameIdentity } | { refusal: FrameRefusal } {
    const slot = this.slots[handle.slot]
    if (handle.stream !== this.stream || !slot) {
      return {
        refusal: {
          code: 'unknown_slot',
          message: 'The handle names a slot this pool does not hold.',
        },
      }
    }
    if (slot.identity === null || handle.generation !== slot.generation) {
      return {
        refusal: {
          code: 'stale_handle',
          message:
            'The slot has been reused since this handle was issued, so the handle names a ' +
            'frame that is no longer there.',
        },
      }
    }
    if (slot.acquired) {
      return {
        refusal: {
          code: 'already_acquired',
          message: 'This handle is already checked out; a handle is one loan, not a share.',
        },
      }
    }
    slot.acquired = true
    this.acquiredCount += 1
    return { identity: slot.identity }
  }

  /** Return a loan. Refuses unknown, stale and already-released handles by name. */
  release(handle: FrameHandle): { released: true } | { refusal: FrameRefusal } {
    const slot = this.slots[handle.slot]
    if (handle.stream !== this.stream || !slot) {
      return {
        refusal: {
          code: 'unknown_slot',
          message: 'The handle names a slot this pool does not hold.',
        },
      }
    }
    if (slot.identity === null || handle.generation !== slot.generation) {
      return {
        refusal: {
          code: 'stale_handle',
          message: 'The slot has been reused since this handle was issued; there is nothing of it left to release.',
        },
      }
    }
    if (!slot.acquired) {
      return {
        refusal: {
          code: 'double_release',
          message: 'This handle was already released; releasing it twice would unpin a later loan.',
        },
      }
    }
    slot.acquired = false
    this.releasedCount += 1
    return { released: true }
  }

  stats(): FramePoolStats {
    return {
      published: this.publishedCount,
      dropped: this.droppedCount,
      acquired: this.acquiredCount,
      released: this.releasedCount,
    }
  }
}

"""Frame handles for the sensor and media pipeline (§47.6).

The Python edition of ``reference/typescript/media.ts``. Read that file for the design; this one
records the port.

§47.4 gives the policy a latest-wins slot and the black box a bounded queue, and says nothing about
how a frame reaches a policy **in another process**. §80 rules out the obvious answer: §25's ring
has 4 KiB slots sized for joint vectors, a frame does not fit in one, and widening the ring would
put a megabyte of copy in the path a control loop runs. So frames stay where capture put them and
what crosses is a *handle* — ``(stream, slot, generation)``, enough to map exactly one frame
read-only and not enough to name a different one.

── What the transcription had to be careful about ────────────────────────────

``sequence`` is compared, never arithmetic. The reference orders slots by ``identity.sequence`` and
Python compares ``int`` exactly where JavaScript compares doubles, so the two agree for every
sequence a capture ring can produce; nothing here adds or subtracts one, which is where the two
number models would start to differ.

``Number.POSITIVE_INFINITY`` as the initial "oldest so far" becomes ``None``, tested explicitly.
``float("inf")`` would work and would invite a later reader to compare it against an ``int``
sequence, which is exactly the mixed-type comparison the rest of this edition avoids.

The refusal **messages** are transcribed, not paraphrased. §64 makes an error a design surface and
the corpus compares the sentence byte for byte, so a message reworded here is a divergence in the
one field an operator actually reads.

── The bookkeeping half ──────────────────────────────────────────────────────

Every function here is arithmetic over slot state. The **mapping** half — a read-only mapping of
the named slot in the policy's address space — is per-OS code that cannot be expressed in a
contract. What it must honour is the property this module exists to guarantee: a handle grants read
access to exactly one generation of one slot, and the pool may reuse that slot the moment it is
released.
"""

from __future__ import annotations

from typing import Any

#: Everything a frame pool can refuse, in the order a reader meets them.
#:
#: The last member is the odd one and belongs in the set anyway: ``pool_exhausted`` refuses
#: *capture*, not the reader, and it is the only one of the six that is a normal operating
#: condition rather than a defect somewhere.
FRAME_REFUSAL_CODES: tuple[str, ...] = (
    "unknown_stream",
    "unknown_slot",
    "stale_handle",
    "already_acquired",
    "double_release",
    "pool_exhausted",
)


class FramePool:
    """One stream's fixed frame pool. Pure bookkeeping: no clocks, no I/O, no mapping."""

    def __init__(self, stream: str, capacity: int) -> None:
        if not stream:
            raise ValueError("a frame pool is for one named stream")
        if not isinstance(capacity, int) or isinstance(capacity, bool) or capacity < 1:
            raise ValueError("a frame pool holds at least one slot")
        self.stream = stream
        self.capacity = capacity
        self._slots: list[dict[str, Any]] = [
            {"generation": 0, "identity": None, "acquired": False} for _ in range(capacity)
        ]
        self._published = 0
        self._dropped = 0
        self._acquired = 0
        self._released = 0

    # ── The selection rule, which is the whole mechanism ─────────────────────

    def _oldest_released(self) -> int:
        """Which slot the next publish takes: the released one holding the oldest frame.

        The obvious implementation — the first slot that is not currently acquired — is wrong in a
        way no single-publish test can see. With nothing acquired it returns slot 0 every time, so
        a pool of any declared capacity uses one slot, and §47.4 already has a one-slot latest-wins
        channel. The cost lands on the *reader*: a policy calls :meth:`latest` and then
        :meth:`acquire`, capture publishes in between, and taking the first free slot means that
        publish lands on the slot ``latest`` just named.

        Empty slots go first because there is nothing in them to destroy. After that, oldest is by
        the frame's ``sequence`` and never by the slot's index — the two stop agreeing the first
        time a reader holds a frame and lets it go. Ties break to the lower index.

        Returns ``-1`` when every slot is acquired.
        """
        chosen = -1
        chosen_sequence: int | None = None
        for index, slot in enumerate(self._slots):
            if slot["acquired"]:
                continue
            identity = slot["identity"]
            if identity is None:
                return index
            if chosen_sequence is None or identity["sequence"] < chosen_sequence:
                chosen_sequence = identity["sequence"]
                chosen = index
        return chosen

    # ── The four operations ──────────────────────────────────────────────────

    def publish(self, frame: dict[str, Any]) -> dict[str, Any]:
        """Store a frame, returning ``{"handle": …}`` or ``{"refusal": …}``.

        Reuses the released slot holding the oldest frame, and when no slot is released the frame
        is dropped and counted rather than stalling capture.
        """
        if frame["stream"] != self.stream:
            return {
                "refusal": {
                    "code": "unknown_stream",
                    "message": (
                        f"This pool holds {self.stream} and was handed a frame from "
                        f"{frame['stream']}."
                    ),
                }
            }
        index = self._oldest_released()
        if index < 0:
            self._dropped += 1
            return {
                "refusal": {
                    "code": "pool_exhausted",
                    "message": (
                        f"Every slot is acquired, so frame {frame['sequence']} was dropped and "
                        "counted rather than stalling capture."
                    ),
                }
            }
        slot = self._slots[index]
        slot["generation"] += 1
        slot["identity"] = frame
        slot["acquired"] = False
        self._published += 1
        return {
            "handle": {"stream": self.stream, "slot": index, "generation": slot["generation"]}
        }

    def latest(self) -> dict[str, Any] | None:
        """The newest published frame's handle, or ``None`` when nothing has been published.

        Never a frame of zeros: a zero frame is a measurement nobody took, and a policy handed one
        cannot tell it from a dark room.
        """
        best: dict[str, Any] | None = None
        best_sequence = -1
        for index, slot in enumerate(self._slots):
            identity = slot["identity"]
            if identity is not None and identity["sequence"] > best_sequence:
                best_sequence = identity["sequence"]
                best = {"stream": self.stream, "slot": index, "generation": slot["generation"]}
        return best

    def acquire(self, handle: dict[str, Any]) -> dict[str, Any]:
        """Take a read loan on the named frame.

        The slot is pinned until :meth:`release`: publishing reuses an older released slot first,
        and drops rather than reusing this one.
        """
        located = self._locate(handle)
        if "refusal" in located:
            return located
        slot = self._slots[located["slot"]]
        if slot["identity"] is None or handle["generation"] != slot["generation"]:
            return {
                "refusal": {
                    "code": "stale_handle",
                    "message": (
                        "The slot has been reused since this handle was issued, so the handle "
                        "names a frame that is no longer there."
                    ),
                }
            }
        if slot["acquired"]:
            return {
                "refusal": {
                    "code": "already_acquired",
                    "message": (
                        "This handle is already checked out; a handle is one loan, not a share."
                    ),
                }
            }
        slot["acquired"] = True
        self._acquired += 1
        return {"identity": slot["identity"]}

    def release(self, handle: dict[str, Any]) -> dict[str, Any]:
        """Return a loan. Refuses unknown, stale and already-released handles by name."""
        located = self._locate(handle)
        if "refusal" in located:
            return located
        slot = self._slots[located["slot"]]
        if slot["identity"] is None or handle["generation"] != slot["generation"]:
            return {
                "refusal": {
                    "code": "stale_handle",
                    "message": (
                        "The slot has been reused since this handle was issued; there is nothing "
                        "of it left to release."
                    ),
                }
            }
        if not slot["acquired"]:
            return {
                "refusal": {
                    "code": "double_release",
                    "message": (
                        "This handle was already released; releasing it twice would unpin a later "
                        "loan."
                    ),
                }
            }
        slot["acquired"] = False
        self._released += 1
        return {"released": True}

    def stats(self) -> dict[str, int]:
        """What the pool has done, which is the only place a drop is visible."""
        return {
            "published": self._published,
            "dropped": self._dropped,
            "acquired": self._acquired,
            "released": self._released,
        }

    # ── Shared ───────────────────────────────────────────────────────────────

    def _locate(self, handle: dict[str, Any]) -> dict[str, Any]:
        """The stream and slot checks both :meth:`acquire` and :meth:`release` open with."""
        slot = handle["slot"]
        if (
            handle["stream"] != self.stream
            or not isinstance(slot, int)
            or isinstance(slot, bool)
            or slot < 0
            or slot >= len(self._slots)
        ):
            return {
                "refusal": {
                    "code": "unknown_slot",
                    "message": "The handle names a slot this pool does not hold.",
                }
            }
        return {"slot": slot}


# ── The corpus script ────────────────────────────────────────────────────────


def script_to_json(case_input: dict[str, Any]) -> Any:
    """Run one §47.6 script and shape every step's outcome.

    A case is a script rather than one input because a pool's whole subject is what the fourth
    publish does given what the first three did. ``from`` names an earlier step's handle by its
    index in ``ops``, which is how a reader actually holds one: ``latest``, then ``acquire`` on
    what it named. Steps that produce no handle still occupy an index.
    """
    stream = case_input["stream"]
    pool = FramePool(stream, case_input["capacity"])
    handles: list[dict[str, Any] | None] = []
    steps: list[dict[str, Any]] = []

    for op in case_input["ops"]:
        name = op["op"]
        produced: dict[str, Any] | None = None
        step: dict[str, Any] = {"op": name}
        if name == "publish":
            result = pool.publish(
                {
                    "stream": op.get("stream", stream),
                    "stamp_ns": op.get("stamp_ns", 0),
                    "sequence": op.get("sequence", 0),
                }
            )
            if "handle" in result:
                step["handle"] = result["handle"]
                produced = result["handle"]
            else:
                step["refusal"] = result["refusal"]
        elif name == "latest":
            produced = pool.latest()
            step["handle"] = produced
        elif name in ("acquire", "release"):
            named = op.get("handle")
            if named is None:
                named = handles[op["from"]]
            if named is None:
                raise ValueError(f"{name} names no handle")
            result = pool.acquire(named) if name == "acquire" else pool.release(named)
            if "refusal" in result:
                step["refusal"] = result["refusal"]
            elif name == "acquire":
                step["identity"] = result["identity"]
            else:
                step["released"] = True
        elif name == "stats":
            step["stats"] = pool.stats()
        else:
            raise ValueError(f"unknown pool op `{name}`")
        handles.append(produced)
        steps.append(step)

    return {"steps": steps, "final": {"latest": pool.latest(), "stats": pool.stats()}}

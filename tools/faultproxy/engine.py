"""The fault engine: what comes out of a link, when, and in what state, for each message that goes into it.

Nothing here touches a socket. A relay (`relays.py`) reads a message off a transport, asks `Engine.offer` what the link does with
it, and delivers whatever comes back at the times it says. Keeping the two apart is what lets `selftest.py` assert exact
behaviour with a clock it owns — a profile that says "drop everything" drops everything, a duplicate is delivered twice, a bit is
flipped once — instead of sampling a real network and accepting "about 3%".

DRY-COVERAGE-PLAN.md item 12.C.4. What this is for: a driver that has only ever met a substrate that answers at once, in order,
once, intact, has never run its own timeout, its sequence check, its reconnect or its stop path. Each fault below is one way a
real link is worse than that, and each is meant to be shown against a *naive* client first (`selftest.py`) so that a profile
that injects nothing cannot be mistaken for a driver that survives it.

## A profile

A comma-separated list of faults, each `name=arg:arg...`, optionally scoped with `up:` (driver to substrate) or `down:`
(substrate to driver); unscoped faults apply both ways. A duration is `250ms` or `2s`; a probability is `3%`.

    delay=40ms                    every message is held this long
    jitter=15ms                   and this much more or less, uniformly
    loss=3%                       dropped, independently
    burst=2%:30%                  dropped in runs: a good link turns bad with the first probability per message and
                                  recovers with the second (mean run length is one over the second)
    duplicate=5%[:gap]            delivered twice, the second `gap` later (default 0)
    reorder=10%[:extra]           held `extra` longer so that later messages overtake it (default: twice the delay, at least 20ms)
    truncate=2%[:keep]            cut short: `keep` is a fraction (0.5) or a byte count (4); default half
    corrupt=1%[:bits[:from-to]]   `bits` bits flipped (default 1), within bytes from..to-1 of the message if given
    slow=2000                     a link of this many bytes a second: each message waits for the ones before it
    stall=2s@1s                   from 1 s after the first message, everything is held for 2 s and then let go at once
                                  — what the driver sees when its host's clock steps or its scheduler stops (the host-side
                                  version of that is 12.C.5's, on a real-time kernel)
    blackhole=5s@1s               from 1 s after the first message, messages are accepted and never delivered: a half-open
                                  connection, a cable pulled on a hub, a peer that hangs
    reset@3s                      at 3 s, every connection is reset (TCP), which is what a peer that crashed looks like
    kill=1s@2s                    at 2 s, the supervised substrate is killed with SIGKILL and started again 1 s later

`@` times count from the first message after the profile was set, so a driver that takes a while to start does not spend the
fault before it arrives. The empty profile is a clean link.

Every decision is drawn from a generator seeded from `seed` and the direction, so the same messages in the same order meet the
same faults, and a failing run can be reproduced.
"""

from __future__ import annotations

import heapq
import random
import re
import threading
from dataclasses import dataclass, field

UP = "up"
DOWN = "down"
DIRECTIONS = (UP, DOWN)


class ProfileError(ValueError):
    """A profile that could not be read. The message says which fault, what was wrong and what a valid one looks like."""


@dataclass(frozen=True)
class Fault:
    kind: str
    scope: str = "both"
    args: tuple = ()

    def applies(self, direction: str) -> bool:
        return self.scope == "both" or self.scope == direction


_DURATION = re.compile(r"^(\d+(?:\.\d+)?)(ms|s)$")
_PERCENT = re.compile(r"^(\d+(?:\.\d+)?)%$")


def _duration(text: str, fault: str) -> float:
    match = _DURATION.match(text)
    if not match:
        raise ProfileError(f"`{fault}` wants a duration like 250ms or 2s and was given `{text}`")
    value = float(match.group(1))
    return value / 1000.0 if match.group(2) == "ms" else value


def _probability(text: str, fault: str) -> float:
    match = _PERCENT.match(text)
    if not match:
        raise ProfileError(f"`{fault}` wants a percentage like 3% and was given `{text}`")
    value = float(match.group(1)) / 100.0
    if value > 1.0:
        raise ProfileError(f"`{fault}` wants a percentage up to 100% and was given `{text}`")
    return value


def _window(text: str, fault: str) -> tuple[float, float]:
    duration, _, at = text.partition("@")
    if not at:
        raise ProfileError(f"`{fault}` wants DURATION@START, like `{fault}=2s@1s`, and was given `{text}`")
    return _duration(duration, fault), _duration(at, fault)


def parse_profile(spec: str) -> list[Fault]:
    """A profile's faults, in the order the engine applies them. Refuses what it cannot read, naming the fault."""
    faults: list[Fault] = []
    for raw in (part.strip() for part in spec.split(",")):
        if not raw:
            continue
        scope = "both"
        for prefix in (UP, DOWN):
            if raw.startswith(prefix + ":"):
                scope, raw = prefix, raw[len(prefix) + 1 :]
        if raw.startswith("reset@"):
            faults.append(Fault("reset", scope, (_duration(raw[len("reset@") :], "reset"),)))
            continue
        name, _, argtext = raw.partition("=")
        args = argtext.split(":") if argtext else []
        try:
            faults.append(Fault(name, scope, _arguments(name, args, argtext)))
        except ProfileError:
            raise
        except IndexError:
            raise ProfileError(f"`{name}` was given too few arguments: `{raw}`. {_SHAPES.get(name, '')}") from None
        except ValueError:
            raise ProfileError(f"`{name}` could not be read from `{raw}`. {_SHAPES.get(name, '')}") from None
    return faults


_SHAPES = {
    "delay": "It is `delay=40ms`.",
    "jitter": "It is `jitter=15ms`.",
    "loss": "It is `loss=3%`.",
    "burst": "It is `burst=2%:30%` — the chance a good link turns bad, then the chance a bad one recovers.",
    "duplicate": "It is `duplicate=5%` or `duplicate=5%:10ms`.",
    "reorder": "It is `reorder=10%` or `reorder=10%:80ms`.",
    "truncate": "It is `truncate=2%`, `truncate=2%:0.5` (a fraction) or `truncate=2%:4` (bytes).",
    "corrupt": "It is `corrupt=1%`, `corrupt=1%:2` (bits) or `corrupt=1%:1:6-10` (within bytes 6 to 9).",
    "slow": "It is `slow=2000`, bytes a second.",
    "stall": "It is `stall=2s@1s`.",
    "blackhole": "It is `blackhole=5s@1s`.",
    "kill": "It is `kill=1s@2s`: restart after 1 s, at 2 s.",
}


def _arguments(name: str, args: list[str], argtext: str) -> tuple:
    if name == "delay" or name == "jitter":
        return (_duration(args[0], name),)
    if name == "loss":
        return (_probability(args[0], name),)
    if name == "burst":
        return (_probability(args[0], name), _probability(args[1], name))
    if name == "duplicate":
        return (_probability(args[0], name), _duration(args[1], name) if len(args) > 1 else 0.0)
    if name == "reorder":
        return (_probability(args[0], name), _duration(args[1], name) if len(args) > 1 else None)
    if name == "truncate":
        keep: float | int = 0.5
        if len(args) > 1:
            keep = float(args[1]) if "." in args[1] else int(args[1])
        return (_probability(args[0], name), keep)
    if name == "corrupt":
        bits = int(args[1]) if len(args) > 1 else 1
        span = None
        if len(args) > 2:
            low, _, high = args[2].partition("-")
            span = (int(low), int(high))
        return (_probability(args[0], name), bits, span)
    if name == "slow":
        rate = float(args[0])
        if rate <= 0:
            raise ProfileError(f"`slow` is bytes a second and must be above zero: `{argtext}`")
        return (rate,)
    if name in ("stall", "blackhole", "kill"):
        return _window(argtext, name)
    raise ProfileError(
        f"`{name}` is not a fault. The faults are delay, jitter, loss, burst, duplicate, reorder, truncate, corrupt, slow, stall, "
        "blackhole, reset and kill (see `tools/faultproxy/engine.py`)."
    )


@dataclass
class Counters:
    """What the link did, so a test can say a fault was really injected and not merely configured."""

    seen: int = 0
    bytes: int = 0
    delivered: int = 0
    lost: int = 0
    burst_lost: int = 0
    blackholed: int = 0
    duplicated: int = 0
    reordered: int = 0
    truncated: int = 0
    corrupted: int = 0
    stalled: int = 0

    def as_dict(self) -> dict:
        return dict(self.__dict__)


@dataclass
class _Direction:
    rng: random.Random
    counters: Counters = field(default_factory=Counters)
    bad: bool = False  # the burst model's state
    last_delivery: float = 0.0
    link_free_at: float = 0.0


class Engine:
    """One link's faults. Thread-safe: a relay's two directions call `offer` from their own threads."""

    def __init__(self, spec: str = "", seed: int = 1, ordered: bool = False) -> None:
        self._lock = threading.Lock()
        self._seed = seed
        #: A stream (TCP, a serial line) delivers in the order it was written unless a `reorder` says otherwise; a datagram
        #: transport is allowed to let jitter reorder, as a network does.
        self.ordered = ordered
        self._set(spec)

    def _set(self, spec: str) -> None:
        self.spec = spec
        self.faults = parse_profile(spec)
        self._t0: float | None = None
        self._fired: set[int] = set()
        self._dirs = {
            direction: _Direction(random.Random(f"{self._seed}/{direction}")) for direction in DIRECTIONS
        }

    def set_profile(self, spec: str) -> None:
        """Change what the link does from now on, keeping the counters: the way a test heals a link or makes it worse."""
        with self._lock:
            old = {direction: self._dirs[direction].counters for direction in DIRECTIONS}
            self._set(spec)
            for direction in DIRECTIONS:
                self._dirs[direction].counters = old[direction]

    def start(self, now: float) -> None:
        """Start the `@` clock at `now` instead of at the first message. A link that carries nothing until something needs it (a dashboard, a stop's own request) would otherwise have its clock started
        by that request, and the fault would begin after the request it was meant to be tested against: the stop check of the chaos contract could not fail for such a flow (found on the UR bridge, whose
        stop asks the dashboard). A cable failure takes every flow down together, so a runner that knows when the fault begins says so."""
        with self._lock:
            if self._t0 is None:
                self._t0 = now

    def started_at(self) -> float | None:
        """When the `@` clock started (a monotonic time), or None before any message or `start`. The kernel relay, which sees no messages, follows the profile's windows by it."""
        with self._lock:
            return self._t0

    def current(self) -> tuple[str, list[Fault], float | None]:
        """The profile as one view: its text, its faults and when its `@` clock started. For a relay that follows the engine by polling and must not read the text of one profile and the faults of another, which
        two attributes read one after the other can give it while `set_profile` is between the two."""
        with self._lock:
            return self.spec, list(self.faults), self._t0

    def counters(self) -> dict:
        with self._lock:
            return {direction: self._dirs[direction].counters.as_dict() for direction in DIRECTIONS}

    def _of(self, direction: str, kind: str) -> list[Fault]:
        return [fault for fault in self.faults if fault.kind == kind and fault.applies(direction)]

    def offer(self, direction: str, data: bytes, now: float) -> list[tuple[float, bytes]]:
        """What the link does with `data` arriving at `now`: a list of (time to deliver, bytes), empty if it is lost."""
        with self._lock:
            return self._offer(direction, data, now)

    def _offer(self, direction: str, data: bytes, now: float) -> list[tuple[float, bytes]]:
        side = self._dirs[direction]
        counters = side.counters
        counters.seen += 1
        counters.bytes += len(data)
        if self._t0 is None:
            self._t0 = now
        elapsed = now - self._t0
        rng = side.rng

        for fault in self._of(direction, "blackhole"):
            length, start = fault.args
            if start <= elapsed < start + length:
                counters.blackholed += 1
                return []
        for fault in self._of(direction, "burst"):
            enter, leave = fault.args
            if side.bad:
                if rng.random() < leave:
                    side.bad = False
            elif rng.random() < enter:
                side.bad = True
            if side.bad:
                counters.burst_lost += 1
                return []
        for fault in self._of(direction, "loss"):
            if rng.random() < fault.args[0]:
                counters.lost += 1
                return []

        for fault in self._of(direction, "truncate"):
            probability, keep = fault.args
            if rng.random() < probability:
                cut = int(len(data) * keep) if isinstance(keep, float) else keep
                if cut < len(data):
                    data = data[:cut]
                    counters.truncated += 1
        for fault in self._of(direction, "corrupt"):
            probability, bits, span = fault.args
            if rng.random() < probability and data:
                low, high = (0, len(data)) if span is None else (min(span[0], len(data)), min(span[1], len(data)))
                if low < high:
                    mutable = bytearray(data)
                    for place in rng.sample(range(low * 8, high * 8), min(bits, (high - low) * 8)):
                        mutable[place // 8] ^= 1 << (place % 8)
                    data = bytes(mutable)
                    counters.corrupted += 1

        delay = sum(fault.args[0] for fault in self._of(direction, "delay"))
        jitter = sum(fault.args[0] for fault in self._of(direction, "jitter"))
        when = now + delay + (rng.uniform(-jitter, jitter) if jitter else 0.0)
        when = max(when, now)
        for fault in self._of(direction, "reorder"):
            probability, extra = fault.args
            if rng.random() < probability:
                when += extra if extra is not None else max(2 * (delay + jitter), 0.02)
                counters.reordered += 1
        for fault in self._of(direction, "slow"):
            start = max(when, side.link_free_at)
            side.link_free_at = start + len(data) / fault.args[0]
            when = side.link_free_at
        for fault in self._of(direction, "stall"):
            length, start = fault.args
            if start <= elapsed < start + length:
                when = max(when, self._t0 + start + length)
                counters.stalled += 1
        # A stream is in order unless a reorder was asked for: a later write never arrives before an earlier one.
        if self.ordered and not self._of(direction, "reorder"):
            when = max(when, side.last_delivery)
        side.last_delivery = max(side.last_delivery, when)

        out = [(when, data)]
        for fault in self._of(direction, "duplicate"):
            probability, gap = fault.args
            if rng.random() < probability:
                out.append((when + gap, data))
                counters.duplicated += 1
        counters.delivered += len(out)
        return out

    def due_events(self, now: float) -> list[tuple[str, float]]:
        """Connection-level faults whose time has come, each returned once: ('reset', 0) or ('kill', restart_after)."""
        with self._lock:
            if self._t0 is None:
                return []
            elapsed = now - self._t0
            due: list[tuple[str, float]] = []
            for index, fault in enumerate(self.faults):
                if index in self._fired:
                    continue
                if fault.kind == "reset" and elapsed >= fault.args[0]:
                    self._fired.add(index)
                    due.append(("reset", 0.0))
                elif fault.kind == "kill" and elapsed >= fault.args[1]:
                    self._fired.add(index)
                    due.append(("kill", fault.args[0]))
            return due


class Delivery:
    """A heap of (time, bytes) for one direction of one link, for a relay's delivery thread to drain as the times come."""

    def __init__(self, cap_bytes: int = 1 << 20) -> None:
        self._heap: list[tuple[float, int, object]] = []
        self._count = 0
        self._queued = 0
        self._cap = cap_bytes
        self._cond = threading.Condition()
        self.closed = False

    def put(self, when: float, item: bytes, meta: object = None, block: bool = True) -> None:
        """Queue `item`. Blocks while more than the cap is queued — which is how a slow reader pushes back on the writer."""
        with self._cond:
            while block and self._queued >= self._cap and not self.closed:
                self._cond.wait(0.05)
            heapq.heappush(self._heap, (when, self._count, (item, meta)))
            self._count += 1
            self._queued += len(item)
            self._cond.notify_all()

    def get(self, now_fn, timeout: float = 0.05):
        """The next (bytes, meta) that is due, or None after `timeout`."""
        with self._cond:
            if not self._heap:
                self._cond.wait(timeout)
            if self._heap and self._heap[0][0] <= now_fn():
                _, _, (item, meta) = heapq.heappop(self._heap)
                self._queued -= len(item)
                self._cond.notify_all()
                return item, meta
            if self._heap:
                self._cond.wait(min(timeout, max(self._heap[0][0] - now_fn(), 0.0005)))
            return None

    def pending(self) -> int:
        with self._cond:
            return len(self._heap)

    def clear(self) -> None:
        with self._cond:
            self._heap.clear()
            self._queued = 0
            self._cond.notify_all()

    def close(self) -> None:
        with self._cond:
            self.closed = True
            self._cond.notify_all()

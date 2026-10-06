#!/usr/bin/env python3
"""What the fault proxy does, shown — each fault against a client that has never met it, then against one that has.

    python3 tools/faultproxy/selftest.py            # the engine, the relays on loopback, and every fault against two clients
    python3 tools/faultproxy/selftest.py --serve udp --port 9001 [--session 7]   # the substrate the scenarios talk to

DRY-COVERAGE-PLAN.md item 12.C.4 asks that each profile be **shown failing against a deliberately naive driver first**: a fault
that injects nothing would look exactly like a driver that survives it, and a proxy nobody has watched break something proves
only that it runs. So every scenario below has three parts — the fault is seen to happen (the proxy's own counters, not the
configuration), a naive client fails in the way the fault is for (a hang, a wrong value, a crash, a false link-down, a stop that
blocks, a stale sample used as fresh), and a careful client, which has a timeout, a checksum, a sequence number, a session check,
a bounded send and a sample age, either gets through or fails *loudly and in a named way* where the fault makes success
impossible.

The substrate is a servo whose position moves by the amount it is told (`MOVE`), answers every request with its position, a
sequence number and its boot's session id under a CRC, and, on `HELLO`, streams stamped samples. It is deliberately small: it is the
fault that is under test, not the protocol. **It is not any driver's substrate** — `drivers/*/chaos.*` point this proxy at those.

Three parts: the engine with a clock the test owns (exact), the relays on loopback and pseudo-terminals (and SocketCAN and raw
Ethernet where the machine has the interfaces: `SKYNET_FAULTPROXY_REQUIRE=1` makes a machine that cannot a failure, and
`drivers/canopen/vcan.sh` is how a runner gets them without root; the kernel's own `netem` queue likewise, on a loopback the process owns, which
`tools/seed-defects/in-netns.sh` makes), and the scenarios.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import random
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "bindings", "python"))

from engine import Engine, ProfileError, parse_profile  # noqa: E402
from relays import CanRelay, NetemRelay, NetemUnsupported, RawRelay, SerialRelay, Supervised, TcpRelay, UdpRelay, netem_arguments  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))

# ── the substrate's protocol ────────────────────────────────────────────────

MAGIC = 0xA5
MOVE, READ, ACK, HELLO, STATE, STOP = 1, 2, 3, 4, 5, 6
FRAME = struct.Struct(">BBIiIH")  # magic, kind, seq, value, session, crc: sixteen bytes
BODY = struct.Struct(">BBIiI")


def crc16(data: bytes) -> int:
    """CRC-16/CCITT-FALSE."""
    crc = 0xFFFF
    for byte in data:
        crc ^= byte << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def pack(kind: int, seq: int, value: int = 0, session: int = 0) -> bytes:
    body = BODY.pack(MAGIC, kind, seq, value, session)
    return body + struct.pack(">H", crc16(body))


def unpack(raw: bytes) -> tuple[int, int, int, int]:
    """(kind, seq, value, session), or ValueError saying why the bytes are not a frame."""
    if len(raw) != FRAME.size:
        raise ValueError(f"a frame is {FRAME.size} bytes and this is {len(raw)}")
    magic, kind, seq, value, session, crc = FRAME.unpack(raw)
    if magic != MAGIC:
        raise ValueError("not a frame: wrong magic")
    if crc != crc16(raw[: BODY.size]):
        raise ValueError("a frame with a wrong checksum")
    return kind, seq, value, session


class Substrate:
    """The servo. `history` is every MOVE it applied, so a test can say what really happened."""

    def __init__(self, transport: str, port: int = 0, session: int | None = None) -> None:
        self.transport = transport
        self.session = session if session is not None else random.SystemRandom().randrange(1, 1 << 31)
        self.x = 0
        self.history: list[int] = []
        self._answers: dict[int, bytes] = {}
        self._stop = threading.Event()
        kind = socket.SOCK_STREAM if transport == "tcp" else socket.SOCK_DGRAM
        self.socket = socket.socket(socket.AF_INET, kind)
        self.socket.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.socket.bind(("127.0.0.1", port))
        self.address = self.socket.getsockname()
        if transport == "tcp":
            self.socket.listen(8)
        self._lock = threading.Lock()
        self._threads: list[threading.Thread] = []

    def answer(self, raw: bytes) -> bytes | None:
        try:
            kind, seq, value, _ = unpack(raw)
        except ValueError:
            return None
        with self._lock:
            if kind == MOVE:
                # A request that is sent again is answered again and not applied again.
                if seq not in self._answers:
                    self.x += value
                    self.history.append(self.x)
                    self._answers[seq] = pack(ACK, seq, self.x, self.session)
                return self._answers[seq]
            if kind in (READ, STOP):
                return pack(ACK, seq, self.x, self.session)
        return None

    def start(self) -> "Substrate":
        target = self._serve_tcp if self.transport == "tcp" else self._serve_udp
        thread = threading.Thread(target=target, daemon=True)
        thread.start()
        self._threads.append(thread)
        return self

    def stop(self) -> None:
        self._stop.set()
        try:
            self.socket.close()
        except OSError:
            pass

    def _serve_udp(self) -> None:
        self.socket.settimeout(0.05)
        while not self._stop.is_set():
            try:
                raw, source = self.socket.recvfrom(2048)
            except socket.timeout:
                continue
            except OSError:
                return
            try:
                kind = unpack(raw)[0]
            except ValueError:
                continue
            if kind == HELLO:
                threading.Thread(target=self._stream, args=(source,), daemon=True).start()
                continue
            reply = self.answer(raw)
            if reply is not None:
                self.socket.sendto(reply, source)

    def _stream(self, client: tuple) -> None:
        """Samples every 10 ms for as long as the substrate lives: the value is the time it was taken, in milliseconds."""
        seq = 0
        while not self._stop.is_set():
            seq += 1
            stamp = int(time.monotonic() * 1000) & 0x7FFFFFFF
            try:
                self.socket.sendto(pack(STATE, seq, stamp, self.session), client)
            except OSError:
                return
            time.sleep(0.01)

    def _serve_tcp(self) -> None:
        self.socket.settimeout(0.05)
        while not self._stop.is_set():
            try:
                connection, _ = self.socket.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            thread = threading.Thread(target=self._connection, args=(connection,), daemon=True)
            thread.start()
            self._threads.append(thread)

    def _connection(self, connection: socket.socket) -> None:
        buffer = b""
        connection.settimeout(0.05)
        with connection:
            while not self._stop.is_set():
                try:
                    chunk = connection.recv(4096)
                except socket.timeout:
                    continue
                except OSError:
                    return
                if not chunk:
                    return
                buffer += chunk
                while len(buffer) >= FRAME.size:
                    reply = self.answer(buffer[: FRAME.size])
                    buffer = buffer[FRAME.size :]
                    if reply is not None:
                        try:
                            connection.sendall(reply)
                        except OSError:
                            return


# ── clients ─────────────────────────────────────────────────────────────────


class LinkError(Exception):
    """The careful client gave up, and says so."""


class PeerRestarted(Exception):
    """The substrate answered with another boot's session id: what it held is gone."""


class Link:
    """A client's end of a transport. `stream` links carry bytes; datagram links carry frames."""

    def __init__(self, transport: str, address: tuple[str, int], buffer: int | None = None) -> None:
        self.transport, self.address, self.buffer = transport, address, buffer
        self.stream = transport == "tcp"
        self.socket: socket.socket | None = None
        self.connect()

    def connect(self) -> None:
        self.close()
        if self.stream:
            sock = socket.create_connection(self.address, timeout=2)
            sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            if self.buffer:
                sock.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, self.buffer)
        else:
            sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            sock.connect(self.address)
        self.socket = sock

    def send(self, data: bytes) -> None:
        assert self.socket is not None
        self.socket.send(data) if not self.stream else self.socket.sendall(data)

    def recv(self, timeout: float) -> bytes | None:
        assert self.socket is not None
        self.socket.settimeout(max(timeout, 0.001))
        try:
            data = self.socket.recv(4096)
        except (socket.timeout, BlockingIOError):
            return None
        if not data and self.stream:
            raise ConnectionResetError("the substrate closed the connection")
        return data

    def close(self) -> None:
        if self.socket is not None:
            try:
                self.socket.close()
            except OSError:
                pass
            self.socket = None


class Naive:
    """What a driver written against a link that never misbehaved does: send, read sixteen bytes, believe them. No timeout unless
    told one (a hard-coded deadline is the naive driver's other mistake), no checksum, no sequence number, no session check."""

    def __init__(self, link: Link, timeout: float | None = None) -> None:
        self.link, self.seq = link, 0
        assert link.socket is not None
        link.socket.settimeout(timeout)

    def request(self, kind: int, value: int = 0) -> int:
        self.seq += 1
        assert self.link.socket is not None
        self.link.send(pack(kind, self.seq, value))
        raw = self.link.socket.recv(FRAME.size)
        if not raw:
            raise ConnectionResetError("the substrate closed the connection")
        _magic, _kind, _seq, got, _session, _crc = FRAME.unpack(raw)  # struct.error on a short frame
        return got


class Careful:
    """A client that has a timeout and a retry, checks length, magic and checksum, matches the sequence number, tells a duplicate or
    a stale reply from the answer, reconnects when the connection goes, reads the session id and refuses to carry on as if a
    substrate that restarted had not — and gives up, in a named way, when the link cannot be made to work."""

    def __init__(self, link: Link, per_try: float = 0.12, tries: int = 6, deadline: float = 3.0) -> None:
        self.link, self.per_try, self.tries, self.deadline = link, per_try, tries, deadline
        self.seq = 0
        self.session: int | None = None
        self.discarded = 0
        self._buffer = b""

    def _frames(self, chunk: bytes):
        if not self.link.stream:
            yield chunk
            return
        self._buffer += chunk
        while len(self._buffer) >= FRAME.size:
            head = self._buffer.find(bytes([MAGIC]))
            if head < 0:
                self._buffer = b""
                return
            if head:
                self._buffer = self._buffer[head:]
                continue
            candidate = self._buffer[: FRAME.size]
            try:
                unpack(candidate)
            except ValueError:
                self._buffer = self._buffer[1:]  # a false magic: look for the next
                continue
            self._buffer = self._buffer[FRAME.size :]
            yield candidate

    def request(self, kind: int, value: int = 0) -> int:
        self.seq += 1
        started = time.monotonic()
        last = "no reply"
        for attempt in range(self.tries):
            if time.monotonic() - started > self.deadline:
                break
            try:
                self.link.send(pack(kind, self.seq, value))
                end = time.monotonic() + self.per_try
                while time.monotonic() < end:
                    chunk = self.link.recv(end - time.monotonic())
                    if chunk is None:
                        break
                    for raw in self._frames(chunk):
                        try:
                            got_kind, got_seq, got_value, session = unpack(raw)
                        except ValueError as error:
                            self.discarded += 1
                            last = str(error)
                            continue
                        if got_kind != ACK or got_seq != self.seq:
                            self.discarded += 1  # a duplicate, or an answer to an earlier try
                            continue
                        if self.session is not None and session != self.session:
                            old, self.session = self.session, session
                            raise PeerRestarted(f"the substrate restarted: session {old} became {session}")
                        self.session = session
                        return got_value
            except (ConnectionError, OSError) as error:
                last = f"{type(error).__name__}"
                time.sleep(0.05)
                try:
                    self.link.connect()
                    self._buffer = b""
                except OSError:
                    pass
        raise LinkError(f"no valid reply to request {self.seq} in {self.tries} tries ({last})")


# ── the checks ──────────────────────────────────────────────────────────────


class Checks:
    def __init__(self) -> None:
        self.count = 0
        self.failed = 0

    def check(self, condition: bool, what: str) -> bool:
        self.count += 1
        if condition:
            print(f"  ok    {what}", flush=True)
        else:
            self.failed += 1
            print(f"  FAIL  {what}", flush=True)
        return bool(condition)

    def section(self, title: str) -> None:
        print(f"\n{title}", flush=True)


def wait_until(condition, timeout: float = 3.0) -> bool:
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if condition():
            return True
        time.sleep(0.01)
    return bool(condition())


# ── part 0: the runner's own words for the ABI ───────────────────────────────


def runner_flag_checks(t: Checks) -> None:
    """`chaos.py` says "the driver said it was faulted or degraded" by a bit, and the bit has to be the ABI's (F196).

    It was written with `FLAG_FAULT = 0x0004` and `FLAG_DEGRADED = 0x0010`, and the ABI's are 0x0002 and 0x0004: the first was
    DEGRADED's bit and the second is the low bit of the homing field, which every driver that answers `not_required` sets on every
    read. So "outage reported" held for xArm and CANopen whatever they said — a check that could not fail — and "recovers" was
    asking about DEGRADED while believing it asked about FAULT.
    """
    import chaos  # noqa: PLC0415
    import skynet_rhal as rhal  # noqa: PLC0415

    t.section("the runner's flags are the ABI's, not a neighbour's (F196)")
    t.check(chaos.FLAG_FAULT == rhal.FLAG_FAULT, f"FLAG_FAULT is the ABI's bit ({chaos.FLAG_FAULT:#06x}, the ABI says {rhal.FLAG_FAULT:#06x})")
    t.check(chaos.FLAG_DEGRADED == rhal.FLAG_DEGRADED, f"FLAG_DEGRADED is the ABI's bit ({chaos.FLAG_DEGRADED:#06x}, the ABI says {rhal.FLAG_DEGRADED:#06x})")
    # What a driver that has said nothing at all, and answers `not_required` to the homing question, reads as: healthy.
    silent = rhal.HOMING_NOT_REQUIRED
    t.check(not chaos.unhealthy(silent), "a driver that says nothing but its homing answer has not said it is faulted or degraded")
    t.check(chaos.unhealthy(rhal.FLAG_FAULT) and chaos.unhealthy(rhal.FLAG_DEGRADED), "a fault flag and a degraded flag each count as saying so")
    t.check(not chaos.unhealthy(rhal.FLAG_ESTOP | rhal.FLAG_BRAKED), "and a stop or a brake alone does not: the questions about the stop are asked elsewhere")


# ── part 1: the engine, with a clock the test owns ──────────────────────────


def engine_checks(t: Checks) -> None:
    t.section("the engine, with a clock the test owns")

    def deliver(spec: str, payloads, seed: int = 1, direction: str = "up", ordered: bool = False, start: float = 100.0, step: float = 0.0):
        engine = Engine(spec, seed, ordered)
        out = []
        for index, payload in enumerate(payloads):
            out.append(engine.offer(direction, payload, start + index * step))
        return engine, out

    engine, out = deliver("", [b"abc"], start=5.0)
    t.check(out == [[(5.0, b"abc")]], "a clean profile is a wire: the message, at once, unchanged")

    _, out = deliver("delay=40ms", [b"x"], start=10.0)
    t.check(abs(out[0][0][0] - 10.04) < 1e-9, "delay holds every message exactly that long")

    _, out = deliver("delay=100ms,jitter=30ms", [b"x"] * 500, start=0.0)
    offsets = [entry[0][0] for entry in out]
    t.check(min(offsets) >= 0.07 - 1e-9 and max(offsets) <= 0.13 + 1e-9, "jitter stays within delay ± jitter")
    t.check(max(offsets) - min(offsets) > 0.048, "and uses the range, not one end of it")

    engine, out = deliver("loss=100%", [b"x"] * 50)
    t.check(all(entry == [] for entry in out) and engine.counters()["up"]["lost"] == 50, "loss at 100% drops everything and counts it")
    engine, out = deliver("loss=0%", [b"x"] * 50)
    t.check(all(len(entry) == 1 for entry in out), "loss at 0% drops nothing")
    engine, out = deliver("loss=30%", [b"x"] * 4000)
    rate = engine.counters()["up"]["lost"] / 4000
    t.check(0.27 < rate < 0.33, f"loss at 30% drops about 30% ({rate:.3f}) — the same 30% for the same seed")
    engine2, _ = deliver("loss=30%", [b"x"] * 4000)
    t.check(engine2.counters() == engine.counters(), "the same seed meets the same faults")
    engine3, _ = deliver("loss=30%", [b"x"] * 4000, seed=2)
    t.check(engine3.counters() != engine.counters(), "and another seed meets others")

    engine, out = deliver("burst=100%:0%", [b"x"] * 30)
    t.check(engine.counters()["up"]["burst_lost"] == 30, "a link that turns bad at once and never recovers drops every message")
    engine, out = deliver("burst=0%:50%", [b"x"] * 300)
    t.check(engine.counters()["up"]["burst_lost"] == 0, "a link that never turns bad drops nothing")
    engine, out = deliver("burst=10%:25%", [b"x"] * 20000)
    runs, run = [], 0
    for entry in out:
        if not entry:
            run += 1
        elif run:
            runs.append(run)
            run = 0
    mean = sum(runs) / len(runs)
    t.check(3.0 < mean < 5.0, f"a burst model recovering at 25% loses runs of about four ({mean:.2f}), not independent singles")

    engine, out = deliver("duplicate=100%:10ms", [b"x"], start=1.0)
    t.check([round(w, 6) for w, _ in out[0]] == [1.0, 1.01] and engine.counters()["up"]["duplicated"] == 1, "a duplicate arrives twice, the second when the gap says")

    _, out = deliver("reorder=100%:50ms", [b"x"], start=1.0)
    t.check(abs(out[0][0][0] - 1.05) < 1e-9, "a reordered message is held for the extra time")
    _, out = deliver("reorder=50%:80ms", [bytes([i]) for i in range(200)], step=0.001)
    order = [entry[0][1][0] for entry in sorted(out, key=lambda e: e[0][0])]
    t.check(order != sorted(order), "later messages overtake the held ones")

    _, out = deliver("truncate=100%:0.5", [b"0123456789"])
    t.check(out[0][0][1] == b"01234", "truncate keeps a fraction")
    _, out = deliver("truncate=100%:4", [b"0123456789"])
    t.check(out[0][0][1] == b"0123", "or a byte count")
    engine, out = deliver("truncate=100%:4", [b"012"])
    t.check(out[0][0][1] == b"012" and engine.counters()["up"]["truncated"] == 0, "and leaves a message that is already short alone, uncounted")

    def bits(a: bytes, b: bytes) -> int:
        return sum(bin(x ^ y).count("1") for x, y in zip(a, b))

    original = bytes(range(16))
    _, out = deliver("corrupt=100%", [original])
    t.check(bits(original, out[0][0][1]) == 1, "corrupt flips one bit by default")
    _, out = deliver("corrupt=100%:3", [original])
    t.check(bits(original, out[0][0][1]) == 3, "or as many as asked")
    _, out = deliver("corrupt=100%:2:6-10", [original] * 40)
    differing = {i for entry in out for i, (x, y) in enumerate(zip(original, entry[0][1])) if x != y}
    t.check(differing and differing <= {6, 7, 8, 9}, "and only inside the bytes it was told to")

    _, out = deliver("slow=1000", [bytes(500), bytes(500)], start=0.0)
    t.check([round(entry[0][0], 6) for entry in out] == [0.5, 1.0], "a slow link makes a message wait for the ones before it")

    engine = Engine("stall=1s@0.5s", 1, ordered=True)
    times = [engine.offer("up", b"x", moment)[0][0] for moment in (0.0, 0.6, 1.4, 2.0)]
    t.check([round(x, 6) for x in times] == [0.0, 1.5, 1.5, 2.0], "a stall holds everything in its window and lets it go at once")

    engine = Engine("blackhole=1s@0.5s", 1)
    kept = [bool(engine.offer("up", b"x", moment)) for moment in (0.0, 0.4, 0.5, 1.2, 1.5, 2.0)]
    t.check(kept == [True, True, False, False, True, True], "a blackhole swallows messages inside its window only")
    t.check(engine.counters()["up"]["blackholed"] == 2, "and counts them")

    # The `@` clock starts at the first message unless a runner starts it. A link that carries nothing until a stop asks for it would otherwise be blackholed 0.3 s after the stop's own request, which is a stop
    # that was never tested against the fault (the UR bridge's stop asks its dashboard, and no read ever does).
    engine = Engine("blackhole=60s@0.3s", 1)
    late = bool(engine.offer("up", b"stop", 5.0))
    engine = Engine("blackhole=60s@0.3s", 1)
    engine.start(4.0)
    engine.start(4.9)
    started = bool(engine.offer("up", b"stop", 5.0))
    t.check(late and not started, "an engine's clock can be started by the runner, so a link with no traffic yet is already past a fault's onset when its first message comes (and a second start does not move it)")
    engine = Engine("blackhole=60s@0.3s", 1)
    engine.start(4.0)
    engine.set_profile("blackhole=60s@0.3s")
    t.check(bool(engine.offer("up", b"stop", 5.0)), "and setting a profile again forgets it: the clock is the profile's")

    # A relay that follows the engine by polling (the kernel's) reads the profile as one view: the text, the faults and the clock together, which two attributes read one after the other would not give it
    # while `set_profile` is between them.
    engine = Engine("delay=10ms", 1)
    spec, faults, began = engine.current()
    engine.start(7.0)
    engine.set_profile("loss=5%,blackhole=1s@0.3s")
    spec_after, faults_after, began_after = engine.current()
    engine.start(8.0)
    t.check(
        spec == "delay=10ms" and [f.kind for f in faults] == ["delay"] and began is None
        and spec_after == "loss=5%,blackhole=1s@0.3s" and [f.kind for f in faults_after] == ["loss", "blackhole"] and began_after is None
        and engine.current()[2] == 8.0,
        "an engine gives its profile's text, its faults and its clock as one view, and a new profile has no clock until it is started",
    )

    engine = Engine("down:loss=100%", 1)
    t.check(bool(engine.offer("up", b"x", 0.0)) and not engine.offer("down", b"x", 0.0), "a fault scoped down does not touch up")
    engine = Engine("up:delay=10ms", 1)
    t.check(engine.offer("down", b"x", 0.0)[0][0] == 0.0, "and one scoped up does not touch down")

    _, out = deliver("delay=10ms,jitter=40ms", [b"x"] * 300, ordered=True, step=0.001)
    times = [entry[0][0] for entry in out]
    t.check(times == sorted(times), "a stream is delivered in the order it was written, jitter or not")
    _, out = deliver("delay=10ms,jitter=40ms", [b"x"] * 300, ordered=False, step=0.001)
    times = [entry[0][0] for entry in out]
    t.check(times != sorted(times), "a datagram link lets jitter reorder, as a network does")

    engine = Engine("loss=100%", 1)
    engine.offer("up", b"x", 0.0)
    engine.set_profile("")
    t.check(bool(engine.offer("up", b"x", 1.0)) and engine.counters()["up"]["lost"] == 1, "healing a link keeps what was counted")

    engine = Engine("reset@1s,kill=2s@3s", 1)
    t.check(engine.due_events(0.0) == [], "no event is due before the first message")
    engine.offer("up", b"x", 10.0)
    t.check(engine.due_events(10.5) == [] and engine.due_events(11.0) == [("reset", 0.0)] and engine.due_events(11.5) == [], "a reset is due once, at its time")
    t.check(engine.due_events(13.0) == [("kill", 2.0)] and engine.due_events(14.0) == [], "and so is a kill, with the restart delay it carries")

    # The profiles the chaos runner uses are written in this language, and a typo in one would be a run that injected nothing.
    with open(os.path.join(HERE, "profiles.json"), encoding="utf-8") as handle:
        table = json.load(handle)["profiles"]
    bad = []
    for name, entry in table.items():
        try:
            parse_profile(entry["spec"])
        except ProfileError as error:
            bad.append(f"{name}: {error}")
        if not set(entry["applies"]) <= {"tcp", "udp", "can", "raw", "serial", "netem"}:
            bad.append(f"{name}: applies to a transport the relays do not have")
        if "netem" in entry["applies"]:
            # a profile marked for the kernel's relay is one it can carry: a subject that lists it would otherwise find out in the middle of a run
            try:
                NetemRelay.accepts(entry["spec"])
            except ValueError as error:
                bad.append(f"{name}: marked for netem and the kernel cannot carry it ({error})")
    t.check(not bad and len(table) >= 10, f"every profile in profiles.json reads, and applies to transports the relays have ({len(table)} profiles)" + ("" if not bad else f": {bad}"))

    for spec, needle in [
        ("loss=lots", "percentage"),
        ("loss=200%", "up to 100%"),
        ("delay=fast", "duration"),
        ("teleport=1", "not a fault"),
        ("delay", "too few"),
        ("stall=2s", "DURATION@START"),
        ("slow=0", "above zero"),
        ("corrupt=1%:x", "could not be read"),
    ]:
        try:
            parse_profile(spec)
            ok = False
        except (ProfileError, ValueError) as error:
            ok = needle is None or needle in str(error)
        t.check(ok, f"`{spec}` is refused" + (f", and the message says `{needle}`" if needle else ""))


# ── part 2: the relays ──────────────────────────────────────────────────────


def echo_server(transport: str):
    """An echo substrate on loopback: (address, stop)."""
    kind = socket.SOCK_STREAM if transport == "tcp" else socket.SOCK_DGRAM
    sock = socket.socket(socket.AF_INET, kind)
    sock.bind(("127.0.0.1", 0))
    stop = threading.Event()
    if transport == "tcp":
        sock.listen(4)

    def serve_tcp() -> None:
        sock.settimeout(0.05)
        while not stop.is_set():
            try:
                connection, _ = sock.accept()
            except (socket.timeout, OSError):
                continue

            def echo(conn=connection) -> None:
                conn.settimeout(0.05)
                with conn:
                    while not stop.is_set():
                        try:
                            data = conn.recv(65536)
                        except socket.timeout:
                            continue
                        except OSError:
                            return
                        if not data:
                            return
                        try:
                            conn.sendall(data)
                        except OSError:
                            return

            threading.Thread(target=echo, daemon=True).start()

    def serve_udp() -> None:
        sock.settimeout(0.05)
        while not stop.is_set():
            try:
                data, source = sock.recvfrom(65535)
            except (socket.timeout, OSError):
                continue
            sock.sendto(data, source)

    threading.Thread(target=serve_tcp if transport == "tcp" else serve_udp, daemon=True).start()

    def close() -> None:
        stop.set()
        sock.close()

    return sock.getsockname(), close


def relay_checks(t: Checks) -> None:
    t.section("the relays, on loopback")

    # TCP: a clean relay is a wire, in order, whole, and passes an orderly close both ways.
    address, close = echo_server("tcp")
    engine = Engine("", 1, ordered=True)
    relay = TcpRelay(engine, ("127.0.0.1", 0), address).start()
    client = socket.create_connection(relay.listen_address, timeout=3)
    payload = bytes(random.Random(7).randrange(256) for _ in range(300_000))
    sender = threading.Thread(target=lambda: client.sendall(payload), daemon=True)
    sender.start()
    got = b""
    client.settimeout(3)
    try:
        while len(got) < len(payload):
            chunk = client.recv(65536)
            if not chunk:
                break
            got += chunk
    except socket.timeout:
        pass
    t.check(got == payload, "tcp: 300 kB through a clean relay arrive whole and in order")
    client.shutdown(socket.SHUT_WR)
    client.settimeout(3)
    try:
        tail = client.recv(10)
    except socket.timeout:
        tail = None
    t.check(tail == b"", "tcp: the client's orderly close reaches the substrate and comes back as the substrate's own")
    client.close()
    relay.stop()
    close()

    # TCP with delay: every byte is held, both ways.
    address, close = echo_server("tcp")
    relay = TcpRelay(Engine("delay=60ms", 1, ordered=True), ("127.0.0.1", 0), address).start()
    client = socket.create_connection(relay.listen_address, timeout=3)
    started = time.monotonic()
    client.sendall(b"ping")
    client.settimeout(3)
    data = client.recv(16)
    took = time.monotonic() - started
    t.check(data == b"ping" and 0.115 <= took < 0.6, f"tcp: delay=60ms each way makes a round trip take at least 120 ms ({took * 1000:.0f} ms)")
    client.close()
    relay.stop()
    close()

    # TCP reset: what a crashed peer looks like.
    address, close = echo_server("tcp")
    relay = TcpRelay(Engine("reset@0.3s", 1, ordered=True), ("127.0.0.1", 0), address).start()
    client = socket.create_connection(relay.listen_address, timeout=3)
    client.sendall(b"a")
    client.settimeout(3)
    client.recv(1)
    try:
        time.sleep(0.6)
        client.sendall(b"b")
        client.settimeout(2)
        data = client.recv(1)
        reset = data == b""
    except (ConnectionError, OSError):
        reset = True
    t.check(reset, "tcp: reset@0.3s ends the connection the way a crashed peer does")
    client.close()
    relay.stop()
    close()

    # UDP
    address, close = echo_server("udp")
    relay = UdpRelay(Engine("", 1), ("127.0.0.1", 0), address).start()
    client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    client.settimeout(2)
    client.sendto(b"hello", relay.listen_address)
    data, _ = client.recvfrom(100)
    t.check(data == b"hello", "udp: a clean relay returns the datagram")
    relay.stop()
    close()

    address, close = echo_server("udp")
    relay = UdpRelay(Engine("down:duplicate=100%", 1), ("127.0.0.1", 0), address).start()
    client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    client.settimeout(0.5)
    client.sendto(b"one", relay.listen_address)
    copies = 0
    try:
        while True:
            client.recvfrom(100)
            copies += 1
    except socket.timeout:
        pass
    t.check(copies == 2, "udp: duplicate=100% on the way down delivers the reply twice")
    relay.stop()
    close()

    address, close = echo_server("udp")
    relay = UdpRelay(Engine("up:loss=100%", 1), ("127.0.0.1", 0), address).start()
    client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    client.settimeout(0.3)
    client.sendto(b"one", relay.listen_address)
    try:
        client.recvfrom(100)
        silent = False
    except socket.timeout:
        silent = True
    t.check(silent, "udp: loss=100% on the way up and the substrate hears nothing")
    relay.stop()
    close()

    # A serial line: bytes written at one end are read at the other, through the engine.
    relay = SerialRelay(Engine("", 1, ordered=True)).start()
    driver = os.open(relay.driver_path, os.O_RDWR | os.O_NOCTTY)
    substrate = os.open(relay.substrate_path, os.O_RDWR | os.O_NOCTTY)
    os.write(driver, b"\x01\x02move\xff")
    got = b""
    end = time.monotonic() + 2
    while len(got) < 7 and time.monotonic() < end:
        if wait_until(lambda: _readable(substrate), 0.2):
            got += os.read(substrate, 64)
    t.check(got == b"\x01\x02move\xff", "serial: bytes written at the driver's end are read at the substrate's, raw")
    relay.stop()
    os.close(driver)
    os.close(substrate)

    relay = SerialRelay(Engine("up:corrupt=100%:1", 1, ordered=True)).start()
    driver = os.open(relay.driver_path, os.O_RDWR | os.O_NOCTTY)
    substrate = os.open(relay.substrate_path, os.O_RDWR | os.O_NOCTTY)
    os.write(driver, b"\x00" * 8)
    wait_until(lambda: _readable(substrate), 2)
    got = os.read(substrate, 64)
    t.check(len(got) == 8 and sum(bin(b).count("1") for b in got) == 1, "serial: corrupt flips a bit in what the line carried")
    relay.stop()
    os.close(driver)
    os.close(substrate)

    # A socket closed under a pump (a `reset` tears a connection down while the other direction's thread is waiting on it) has file descriptor -1, and `select` says so with a ValueError, not an OSError: the
    # pump's thread used to die with a traceback and the end of the connection was never reported (seen as fourteen tracebacks in one run of the xArm subject's `reset` profile).
    from relays import _Pump

    class ClosedUnderIt:
        def fileno(self) -> int:
            return -1

    import select as _select

    def closed_source() -> object:
        _select.select([ClosedUnderIt()], [], [], 0.01)
        return None

    ended: list[bool] = []
    escaped: list[object] = []
    saved_hook = threading.excepthook
    threading.excepthook = lambda args: escaped.append(args.exc_type)
    try:
        pump = _Pump("closed-socket", closed_source, Engine("", 1), "up", None, threading.Event(), ended.append)
        pump.start()
        pump.join(2)
    finally:
        threading.excepthook = saved_hook
    t.check(ended == [True] and not escaped and not pump.is_alive(), f"a pump whose socket was closed under it ends the connection as an error, and does not die with a traceback ({escaped})")

    # SocketCAN and raw Ethernet need interfaces this process may not be able to make. Said by name when they cannot.
    require = os.environ.get("SKYNET_FAULTPROXY_REQUIRE") == "1"
    for label, names in (("can", ("vcan0", "vcan1")), ("raw", ("fpa0", "fpa1", "fpb0", "fpb1"))):
        _kernel_relay(t, label, names, require)
    _netem_relay(t, require)


def _readable(fd: int) -> bool:
    import select

    return bool(select.select([fd], [], [], 0)[0])


def _has_interface(name: str) -> bool:
    """By `ip`, not /sys/class/net: an unprivileged network namespace shares the machine's sysfs, which lists the machine's devices."""
    try:
        return subprocess.run(["ip", "link", "show", name], capture_output=True).returncode == 0
    except OSError:
        return False


def _make_interfaces(label: str, names) -> bool:
    """Make the interfaces in this namespace if the machine will let the process; False when it will not."""
    commands = []
    if label == "can":
        commands += [["ip", "link", "add", "dev", n, "type", "vcan"] for n in names if not _has_interface(n)]
        # an MTU of 72 is what lets a CAN-FD frame through a virtual interface; a classic frame crosses one just as well
        commands += [["ip", "link", "set", n, "mtu", "72"] for n in names]
    else:
        # two veth pairs, so that the relay joins two segments: (a0 a1) and (b0 b1), the relay on a1 and b0
        for first, second in ((names[0], names[1]), (names[2], names[3])):
            if not _has_interface(first):
                commands.append(["ip", "link", "add", first, "type", "veth", "peer", "name", second])
    commands += [["ip", "link", "set", "up", n] for n in names]
    for command in commands:
        try:
            if subprocess.run(command, capture_output=True).returncode != 0:
                return False
        except OSError:
            return False
    return True


def _kernel_relay(t: Checks, label: str, names, require: bool) -> None:
    if not hasattr(socket, "AF_CAN" if label == "can" else "AF_PACKET") or not _make_interfaces(label, names):
        if require:
            t.check(False, f"{label}: the kernel-interface half was asked for (SKYNET_FAULTPROXY_REQUIRE=1) and this machine cannot make {names}")
        else:
            print(f"  SKIPPED  {label}: the kernel-interface half did not run — this process cannot make {names} (run under `sh drivers/canopen/vcan.sh`, or `unshare -rn`)", flush=True)
        return
    if label == "can":
        # A vcan is one bus: a frame the test sends on vcan0 is heard by the relay's socket there and sent on vcan1, where the
        # test's other socket hears it. A raw CAN socket does not hear its own transmissions.
        relay = CanRelay(Engine("", 1), names[0], names[1]).start()
        a = socket.socket(socket.PF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
        a.bind((names[0],))
        b = socket.socket(socket.PF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
        b.bind((names[1],))
        b.settimeout(2)
        frame = struct.pack("=IB3x8s", 0x123, 3, bytes([1, 2, 3]).ljust(8, bytes(1)))
        a.send(frame)
        try:
            raw = b.recv(16)
        except socket.timeout:
            raw = b""
        t.check(len(raw) == 16 and struct.unpack("=IB3x8s", raw)[0] == 0x123 and raw[8:11] == bytes([1, 2, 3]), "can: a frame sent on one bus is on the other, id and data intact")
        relay.engine.set_profile("truncate=100%:1")
        a.send(frame)
        raw = b.recv(16)
        t.check(struct.unpack("=IB3x8s", raw)[1] == 1, "can: truncate shortens the DLC")
        relay.engine.set_profile("loss=100%")
        a.send(frame)
        try:
            b.recv(16)
            silent = False
        except socket.timeout:
            silent = True
        t.check(silent, "can: loss=100% and the other bus hears nothing")
        relay.stop()
        a.close()
        b.close()

        # CAN-FD: the same two interfaces, a relay that carries 64-byte frames, 29-bit identifiers and the flags with them (a moteus's frames are all three)
        fd_layout = struct.Struct("=IBB2x64s")
        fd_relay = CanRelay(Engine("", 1), names[0], names[1], fd=True).start()
        sockets = []
        for name in names[:2]:
            s = socket.socket(socket.PF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
            s.setsockopt(socket.SOL_CAN_RAW, socket.CAN_RAW_FD_FRAMES, 1)
            s.bind((name,))
            sockets.append(s)
        fa, fb = sockets
        fb.settimeout(2)
        payload = bytes(range(20))
        eff = 0x80000000
        fa.send(fd_layout.pack(0x8001 | eff, len(payload), 1, payload.ljust(64, bytes(1))))
        try:
            raw = fb.recv(72)
        except socket.timeout:
            raw = b""
        got = fd_layout.unpack(raw) if len(raw) == 72 else (0, 0, 0, b"")
        t.check(len(raw) == 72 and got[0] == 0x8001 | eff and got[1] == 20 and got[3][:20] == payload and got[2] & 1,
                "can-fd: a 20-byte frame with a 29-bit identifier and the bit-rate switch crosses the relay intact, flags and all")
        fd_relay.engine.set_profile("truncate=100%:5")
        fa.send(fd_layout.pack(0x8001 | eff, len(payload), 1, payload.ljust(64, bytes(1))))
        got = fd_layout.unpack(fb.recv(72))
        t.check(got[1] == 5 and got[0] == 0x8001 | eff, "can-fd: truncate shortens the length of a long frame, identifier kept")
        fd_relay.engine.set_profile("corrupt=100%:1")
        fa.send(fd_layout.pack(0x0100, len(payload), 1, payload.ljust(64, bytes(1))))
        got = fd_layout.unpack(fb.recv(72))
        t.check(got[1] == 20 and sum(bin(x ^ y).count("1") for x, y in zip(got[3][:20], payload)) == 1, "can-fd: corrupt flips one bit of a long frame's data")
        fd_relay.engine.set_profile("")
        fa.send(struct.pack("=IB3x8s", 0x123, 3, bytes([1, 2, 3]).ljust(8, bytes(1))))
        raw = fb.recv(72)
        t.check(len(raw) == 16 and struct.unpack("=IB3x8s", raw)[0] == 0x123, "can-fd: a classic frame sent through an FD relay is delivered classic")
        fd_relay.engine.set_profile("loss=100%")
        fa.send(fd_layout.pack(0x8001 | eff, len(payload), 1, payload.ljust(64, bytes(1))))
        try:
            fb.recv(72)
            silent = False
        except socket.timeout:
            silent = True
        t.check(silent, "can-fd: loss=100% and the other bus hears nothing")
        fd_relay.stop()
        fa.close()
        fb.close()
    else:
        relay = RawRelay(Engine("", 1), names[1], names[2], ethertype=0x88B5).start()
        sender = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(0x88B5))
        sender.bind((names[0], 0x88B5))
        receiver = socket.socket(socket.AF_PACKET, socket.SOCK_RAW, socket.htons(0x88B5))
        receiver.bind((names[3], 0x88B5))
        receiver.settimeout(2)
        frame = bytes([0xFF] * 6) + bytes([0x02, 0, 0, 0, 0, 0x01]) + bytes([0x88, 0xB5]) + b"skynet-faultproxy".ljust(46, bytes(1))
        sender.send(frame)
        try:
            raw = receiver.recv(1518)
        except socket.timeout:
            raw = b""
        t.check(raw == frame, "raw: a frame sent on one segment crosses the relay and arrives on the other, byte for byte")
        relay.engine.set_profile("corrupt=100%:1")
        sender.send(frame)
        raw = receiver.recv(1518)
        t.check(len(raw) == len(frame) and sum(bin(x ^ y).count("1") for x, y in zip(raw, frame)) == 1, "raw: corrupt flips one bit of the frame")
        relay.stop()
        sender.close()
        receiver.close()


def _netem_unavailable() -> str:
    """Why this process cannot put a queue on its own loopback, or '' when it can: the loopback is this process's and not the machine's, `tc` is there, and the kernel has `sch_netem`."""
    import relays  # noqa: PLC0415

    if not relays.private_namespace():
        return "this process's network is the machine's, and the queue would be on the machine's own loopback (run under `sh tools/seed-defects/in-netns.sh`)"
    if shutil.which("tc") is None:
        return "`tc` (iproute2) is not installed"
    try:
        subprocess.run(["ip", "link", "set", "lo", "up"], capture_output=True)
        probe = subprocess.run(["tc", "qdisc", "replace", "dev", "lo", "root", "netem", "delay", "1ms"], capture_output=True, text=True)
    except OSError as error:
        return f"cannot run `ip` and `tc`: {error}"
    if probe.returncode != 0:
        return f"the kernel refused a netem queue on lo ({(probe.stderr or probe.stdout).strip()}): it needs sch_netem and CAP_NET_ADMIN over the namespace"
    subprocess.run(["tc", "qdisc", "del", "dev", "lo", "root"], capture_output=True)
    return ""


def _qdisc(device: str = "lo") -> str:
    return subprocess.run(["tc", "qdisc", "show", "dev", device], capture_output=True, text=True).stdout.strip()


def _netem_relay(t: Checks, require: bool) -> None:
    """The kernel's link emulation. What a profile becomes is checked wherever this runs; what the kernel then does with it only on a loopback this process owns, and said when it is not."""
    t.section("the kernel's link emulation (netem): what a profile becomes")
    carried = (
        ("", "limit 10000"),
        ("delay=40ms", "limit 10000 delay 40ms"),
        ("delay=30ms,jitter=25ms", "limit 10000 delay 30ms 25ms"),
        ("loss=3%", "limit 10000 loss 3%"),
        ("burst=5%:15%", "limit 10000 loss gemodel 5% 15%"),
        ("reorder=20%:40ms", "limit 10000 delay 40ms reorder 80%"),
        ("duplicate=5%", "limit 10000 duplicate 5%"),
        ("corrupt=2%", "limit 10000 corrupt 2%"),
        ("slow=4000", "limit 10000 rate 32000bit"),
        ("delay=40ms,loss=3%,duplicate=1%", "limit 10000 delay 40ms loss 3% duplicate 1%"),
        # the window is a matter of time, which the relay follows by its own clock: what stands on the interface the rest of the time is the rest of the profile
        ("blackhole=1s@0.3s", "limit 10000"),
        ("delay=20ms,blackhole=1s@0.3s", "limit 10000 delay 20ms"),
    )
    wrong = [(spec, netem_arguments(parse_profile(spec))) for spec, want in carried if netem_arguments(parse_profile(spec)) != want]
    t.check(not wrong, f"netem: {len(carried)} profiles become the `tc` arguments they should" + ("" if not wrong else f" — not: {wrong}"))

    refused = (
        ("truncate=50%:4", "truncate"),
        ("up:loss=5%", "one direction"),
        ("down:delay=40ms", "one direction"),
        ("stall=1s@0.5s", "stall"),
        ("reset@1s", "connection's or a process's"),
        ("kill=1s@2s", "connection's or a process's"),
        ("corrupt=5%:3", "3 bits"),
        ("corrupt=5%:1:2-4", "bytes 2 to 4"),
        ("duplicate=5%:20ms", "gap"),
        ("reorder=20%:40ms,delay=10ms", "of its own"),
        ("loss=5%,burst=5%:15%", "one loss model"),
    )
    for spec, needle in refused:
        try:
            got = netem_arguments(parse_profile(spec))
            t.check(False, f"netem: `{spec}` is refused (it became `{got}`)")
        except NetemUnsupported as error:
            t.check(needle in str(error), f"netem: `{spec}` is refused, and the message says `{needle}`")

    # a profile that is refused is refused before it changes anything, by the runner and by the command line
    import chaos  # noqa: PLC0415

    class Stub:
        name = "stub"
        flows = [("netem", "lo")]
        stopped = 0

        def start(self) -> None:
            pass

        def stop(self) -> None:
            self.stopped += 1

    stub = Stub()
    try:
        chaos.Session(stub, "truncate=50%:4")
        said = ""
    except chaos.Unsupported as error:
        said = str(error)
    t.check("truncate" in said and stub.stopped == 1, "netem: the runner refuses a profile the subject's kernel link cannot carry, and stops the substrate it had started")
    noise = io.StringIO()
    outcome = chaos.Result()
    with contextlib.redirect_stdout(noise):
        chaos.run_profile(stub, "truncate-5pct", {"spec": "truncate=5%"}, outcome, None)
    t.check(outcome.count == 1 and outcome.failed == 1 and "nothing was run" in noise.getvalue(), "netem: and it is a failed check naming the profile, not a run on a clean link that passes")

    import relays  # noqa: PLC0415

    saved = relays.private_namespace
    relays.private_namespace = lambda: False
    try:
        NetemRelay(Engine("", 1), "lo").start()
        said = ""
    except SystemExit as error:
        said = str(error)
    finally:
        relays.private_namespace = saved
    t.check("refusing" in said and "in-netns.sh" in said, "netem: on the machine's own network the relay refuses to change an interface's queue, and says how to get a namespace of its own")

    why = _netem_unavailable()
    if why:
        if require:
            t.check(False, f"netem: the kernel half was asked for (SKYNET_FAULTPROXY_REQUIRE=1) and {why}")
        else:
            print(f"  SKIPPED  netem: the kernel half did not run — {why}", flush=True)
        return

    t.section("the kernel's link emulation (netem): what the kernel does with it, on this process's own loopback")
    address, close = echo_server("udp")

    def round_trip(timeout: float = 1.0) -> float | None:
        """A datagram to the echo and back, in milliseconds; None when nothing came back in `timeout`."""
        client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        client.settimeout(timeout)
        began = time.monotonic()
        try:
            client.sendto(b"x", address)
            client.recvfrom(100)
        except socket.timeout:
            return None
        finally:
            client.close()
        return (time.monotonic() - began) * 1000

    def shown(took: float | None) -> str:
        return "no answer" if took is None else f"{took:.1f} ms"

    engine = Engine("", 1)
    relay = NetemRelay(engine, "lo").start()
    try:
        took = round_trip()
        t.check(took is not None and took < 20, f"netem: with a clean profile the echo is as quick as the loopback ({shown(took)})")

        engine.set_profile("delay=40ms")
        relay.refresh()
        took = round_trip()
        t.check(took is not None and 75 <= took < 400, f"netem: delay=40ms is 40 ms each way, on one queue that carries both ({shown(took)})")

        engine.set_profile("loss=100%")
        relay.refresh()
        t.check(round_trip(0.3) is None, "netem: loss=100% and the echo never comes back")

        # the window: the clock starts when the runner says, and not at a first message, which a kernel queue never sees
        engine.set_profile("blackhole=1s@0.3s")
        began = time.monotonic()
        engine.start(began)
        relay.refresh()
        before = round_trip(0.2)
        time.sleep(max(0.0, began + 0.6 - time.monotonic()))
        during = round_trip(0.2)
        time.sleep(max(0.0, began + 1.6 - time.monotonic()))
        after = round_trip(0.2)
        t.check(before is not None, f"netem: blackhole=1s@0.3s: before the window the link is as it was ({shown(before)})")
        t.check(during is None, f"netem: blackhole=1s@0.3s: inside the window the echo does not come back ({shown(during)})")
        t.check(after is not None, f"netem: blackhole=1s@0.3s: after the window the link is back ({shown(after)})")

        engine.set_profile("")
        relay.refresh()
        took = round_trip()
        t.check(took is not None and took < 20, f"netem: a healed link is the loopback again ({shown(took)})")

        # given behind the relay's back (the runner and the command line ask `accepts` first), a profile the kernel cannot carry must not become a clean link that the run takes for a fault
        engine.set_profile("truncate=50%:4")
        t.check(wait_until(lambda: relay.failure is not None, 1.0), "netem: a profile the kernel cannot carry, given to the engine directly, is noticed by the relay")
        try:
            relay.refresh()
            raised = False
        except NetemUnsupported:
            raised = True
        t.check(raised, "netem: and `refresh` says so, instead of leaving the last queue on the interface as if it were the profile")
    finally:
        relay.stop()
    t.check("netem" not in _qdisc(), "netem: stopping the relay takes its queue off the interface")

    try:
        NetemRelay(Engine("", 1), "nosuchdev0").start()
        said = ""
    except SystemExit as error:
        said = str(error)
    t.check("nosuchdev0" in said and "failed" in said, f"netem: a `tc` that fails is a stop that says so, and not a link that is clean ({said[:90]})")

    # the command line: the same relay, its control socket, and a profile it refuses
    directory = tempfile.mkdtemp(prefix="faultproxy-netem-")
    control = os.path.join(directory, "control")
    process = subprocess.Popen(
        [sys.executable, os.path.join(HERE, "faultproxy.py"), "netem", "--profile", "delay=40ms", "--control", control],
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        if t.check(process.stdout.readline().strip() == "ready netem lo", "netem: `faultproxy.py netem` says it is ready, and on which interface"):
            took = round_trip()
            t.check(took is not None and 75 <= took < 400, f"netem: and carries the profile it was started with ({shown(took)})")

            def ask(line: str) -> str:
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as sock:
                    sock.settimeout(5)
                    sock.connect(control)
                    sock.sendall(line.encode() + b"\n")
                    return sock.makefile().readline().strip()

            wait_until(lambda: os.path.exists(control), 3)
            reply = ask("set truncate=50%:4")
            t.check(reply.startswith("error") and "truncate" in reply, f"netem: a profile the kernel cannot carry is an error on the control socket ({reply[:70]})")
            took = round_trip()
            t.check(took is not None and 75 <= took < 400, f"netem: and the refused profile changed nothing ({shown(took)})")
            t.check(ask("set loss=100%") == "ok" and round_trip(0.3) is None, "netem: `set` puts a new profile on the interface before it answers")
            t.check(ask("heal") == "ok" and (round_trip() or 1e9) < 20, "netem: and `heal` makes the loopback a loopback again")
    finally:
        process.terminate()
        try:
            tail, _ = process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            tail, _ = process.communicate()
        shutil.rmtree(directory, ignore_errors=True)
    t.check("stats {" in tail and "netem" not in _qdisc(), "netem: ended by a signal, it prints its counters and takes its queue off the interface")
    close()


# ── part 3: each fault against a naive client and a careful one ──────────────


class Bench:
    """A substrate, a relay with a profile in front of it, and a way to make clients that talk through it."""

    def __init__(self, transport: str, profile: str, seed: int = 1, buffer: int | None = None, supervised: bool = False, cap: int = 1 << 20) -> None:
        self.transport = transport
        self.buffer = buffer
        self.server: Substrate | None = None
        self.links: list[Link] = []
        supervisor = None
        if supervised:
            # The substrate as a process of its own, so that `kill` has something to kill with SIGKILL.
            holder = socket.socket()
            holder.bind(("127.0.0.1", 0))
            port = holder.getsockname()[1]
            holder.close()
            upstream = ("127.0.0.1", port)
            supervisor = Supervised([sys.executable, os.path.abspath(__file__), "--serve", transport, "--port", str(port)], _tcp_ready(upstream))
        else:
            self.server = Substrate(transport).start()
            upstream = self.server.address[:2]
        self.engine = Engine(profile, seed, ordered=transport == "tcp")
        if transport == "tcp":
            self.relay = TcpRelay(self.engine, ("127.0.0.1", 0), upstream, supervisor, buffer=buffer, cap_bytes=cap)
        else:
            self.relay = UdpRelay(self.engine, ("127.0.0.1", 0), upstream)
        self.relay.start()

    def link(self) -> Link:
        link = Link(self.transport, self.relay.listen_address, self.buffer)
        self.links.append(link)
        return link

    def counters(self, direction: str) -> dict:
        return self.engine.counters()[direction]

    def injected(self, *names: str) -> int:
        """How many times the proxy did any of these to a message, in either direction: what makes a scenario a scenario."""
        return sum(self.counters(direction)[name] for direction in ("up", "down") for name in names)

    def close(self) -> None:
        for link in self.links:
            link.close()
        self.relay.stop()
        if self.server:
            self.server.stop()


def _tcp_ready(target):
    def ready() -> bool:
        try:
            with socket.create_connection(target, timeout=0.2):
                return True
        except OSError:
            return False

    return ready


class Outcome:
    def __init__(self) -> None:
        self.values: list[int] = []
        self.error: BaseException | None = None
        self.hung = False
        self.errors: list[BaseException] = []


def run(work, budget: float) -> Outcome:
    """`work(outcome)` on a thread, for at most `budget` seconds: a client that has not finished by then is hung."""
    outcome = Outcome()

    def body() -> None:
        try:
            work(outcome)
        except BaseException as error:  # noqa: BLE001 — the point is to see what a client dies of
            outcome.error = error

    thread = threading.Thread(target=body, daemon=True)
    thread.start()
    thread.join(budget)
    outcome.hung = thread.is_alive()
    return outcome


def moves(client, count: int, pause: float = 0.0):
    def work(outcome: Outcome) -> None:
        for _ in range(count):
            outcome.values.append(client.request(MOVE, 1))
            if pause:
                time.sleep(pause)

    return work


def clean(outcome: Outcome, count: int) -> bool:
    """Every one of `count` moves answered, in order, with the position the substrate was really at."""
    return outcome.error is None and not outcome.hung and outcome.values == list(range(1, count + 1))


def scenarios(t: Checks) -> None:
    t.section("every fault, against a client that has never met it and one that has")

    # 1. delay: a hard-coded deadline shorter than the link's latency reads a healthy link as a dead one.
    bench = Bench("tcp", "delay=120ms")
    out = run(moves(Naive(bench.link(), timeout=0.1), 4), 3)
    t.check(bench.counters("up")["delivered"] > 0, "delay: the proxy carried the traffic, each message 120 ms late (its own count)")
    t.check(isinstance(out.error, socket.timeout), "delay: a naive client with a 100 ms deadline calls a healthy 240 ms link down")
    bench.close()
    bench = Bench("tcp", "delay=120ms")
    out = run(moves(Careful(bench.link(), per_try=0.15, tries=6, deadline=3.0), 4), 8)
    t.check(clean(out, 4), f"delay: a careful client resends, takes the late answer for its own request, and gets through ({out.values})")
    bench.close()

    # 2. jitter read as a stall (F171): a cyclic read declared faulted the first time one answer is a period late.
    period = 0.05

    def cycle(link: Link, tolerate: int):
        def work(outcome: Outcome) -> None:
            late = 0
            for sequence in range(1, 61):
                started = time.monotonic()
                link.send(pack(READ, sequence))
                got = False
                while time.monotonic() - started < period:
                    chunk = link.recv(period - (time.monotonic() - started))
                    if chunk is None:
                        break
                    try:
                        kind, seq, _, _ = unpack(chunk)
                    except ValueError:
                        continue
                    if kind == ACK and seq == sequence:
                        got = True
                        break
                late = 0 if got else late + 1
                if late > tolerate:
                    raise LinkError(f"stalled after {late} late cycle(s) in a row")
                remaining = period - (time.monotonic() - started)
                if remaining > 0:
                    time.sleep(remaining)
                outcome.values.append(1)

        return work

    bench = Bench("udp", "delay=15ms,jitter=15ms")
    out = run(cycle(bench.link(), 0), 6)
    t.check(bench.counters("up")["delivered"] > 0 and isinstance(out.error, LinkError), "jitter: a cyclic client that faults on one late answer declares a stall within a few cycles (F171)")
    bench.close()
    bench = Bench("udp", "delay=15ms,jitter=15ms")
    out = run(cycle(bench.link(), 2), 6)
    t.check(out.error is None and len(out.values) == 60, "jitter: one that allows two late cycles in a row rides it out, all 60 cycles")
    bench.close()

    # 3. loss: a read with no timeout waits for ever for an answer that was dropped.
    bench = Bench("udp", "down:loss=40%")
    out = run(moves(Naive(bench.link()), 20), 1.5)
    t.check(bench.counters("down")["lost"] > 0 and out.hung, "loss: a naive client with no timeout hangs on the first answer the link drops")
    bench.close()
    bench = Bench("udp", "down:loss=40%")
    out = run(moves(Careful(bench.link()), 20), 15)
    t.check(clean(out, 20) and bench.server.history == list(range(1, 21)), "loss: a careful client resends and reads 1..20 exactly, each move applied once (the resend is answered, not applied again)")
    bench.close()

    # 4. burst loss: a long run exhausts a retry budget. The careful client says so, and is back when the link is.
    bench = Bench("udp", "burst=30%:12%", seed=3)
    out = run(moves(Naive(bench.link()), 60), 1.5)
    t.check(bench.injected("burst_lost") > 0 and out.hung, "burst: a naive client with no timeout hangs in the first run of losses")
    bench.close()
    bench = Bench("udp", "burst=30%:12%", seed=3)
    careful = Careful(bench.link(), per_try=0.03, tries=4, deadline=2.0)
    progress = {"failed": 0, "wrong": []}

    def burst_work(outcome: Outcome) -> None:
        for _ in range(60):
            try:
                value = careful.request(MOVE, 1)
            except LinkError:
                progress["failed"] += 1
                continue
            if value > bench.server.x or (outcome.values and value < outcome.values[-1]):
                progress["wrong"].append(value)
            outcome.values.append(value)

    out = run(burst_work, 20)
    t.check(bench.injected("burst_lost") > 0, "burst: the proxy lost runs, not singles (its own count)")
    t.check(not out.hung and out.error is None and progress["failed"] > 0 and out.values, f"burst: a long run costs the careful client some requests, loudly ({progress['failed']} LinkError) — and it carries on when the run ends ({len(out.values)} answered)")
    t.check(not progress["wrong"], "burst: and it never reports a position the substrate was not at, or one that goes backwards")
    bench.close()

    # 5. duplicate: a reply delivered twice is read as the answer to the next request.
    bench = Bench("udp", "down:duplicate=100%")
    out = run(moves(Naive(bench.link()), 6), 3)
    t.check(bench.counters("down")["duplicated"] > 0 and not out.hung and out.values != list(range(1, 7)), f"duplicate: a naive client reads the copy as the next answer and is one behind ({out.values})")
    bench.close()
    bench = Bench("udp", "down:duplicate=100%")
    careful = Careful(bench.link())
    out = run(moves(careful, 6), 5)
    t.check(clean(out, 6) and careful.discarded > 0, "duplicate: a careful client matches the sequence number and throws the copies away")
    bench.close()

    # 6. reorder: back-to-back requests whose answers come out of order.
    def pipelined(link: Link, by_sequence: bool):
        def work(outcome: Outcome) -> None:
            base = 100
            for index in range(6):
                link.send(pack(MOVE, base + index, 1))
            got: dict[int, int] = {}
            order: list[int] = []
            end = time.monotonic() + 1.5
            while len(order) < 6 and time.monotonic() < end:
                chunk = link.recv(end - time.monotonic())
                if chunk is None:
                    continue
                _kind, seq, value, _ = unpack(chunk)
                got[seq] = value
                order.append(value)
            outcome.values = [got[base + i] for i in range(6)] if by_sequence else order

        return work

    bench = Bench("udp", "down:reorder=50%:60ms")
    out = run(pipelined(bench.link(), False), 4)
    t.check(bench.counters("down")["reordered"] > 0 and out.values != list(range(1, 7)), f"reorder: a client that takes answers in arrival order assigns them to the wrong requests ({out.values})")
    bench.close()
    bench = Bench("udp", "down:reorder=50%:60ms")
    out = run(pipelined(bench.link(), True), 4)
    t.check(out.values == list(range(1, 7)), "reorder: one that files each answer under its sequence number is not fooled")
    bench.close()

    # 7. truncate: a short datagram read as a whole one.
    bench = Bench("udp", "down:truncate=50%:6")
    out = run(moves(Naive(bench.link(), timeout=1.0), 20), 4)
    t.check(bench.counters("down")["truncated"] > 0 and isinstance(out.error, struct.error), "truncate: a naive client unpacks a six-byte answer as sixteen and dies of it")
    bench.close()
    bench = Bench("udp", "down:truncate=50%:6")
    careful = Careful(bench.link())
    out = run(moves(careful, 20), 15)
    t.check(clean(out, 20) and careful.discarded > 0, "truncate: a careful client checks the length, discards the stump and asks again")
    bench.close()

    # 8. corrupt: a flipped bit in the value, believed.
    bench = Bench("udp", "down:corrupt=50%:1:6-10")
    out = run(moves(Naive(bench.link(), timeout=1.0), 20), 4)
    t.check(bench.counters("down")["corrupted"] > 0 and not out.hung and out.values != list(range(1, 21)), "corrupt: a naive client believes a position with a bit flipped in it")
    bench.close()
    bench = Bench("udp", "down:corrupt=50%:1:6-10")
    careful = Careful(bench.link())
    out = run(moves(careful, 20), 15)
    t.check(clean(out, 20) and careful.discarded > 0, "corrupt: a careful client's checksum refuses it, and the resend gets the true one")
    bench.close()

    # 9. half-open: the connection stays up and nothing comes back.
    bench = Bench("tcp", "blackhole=700ms@300ms")
    naive = Naive(bench.link())
    first = run(moves(naive, 1), 2)
    time.sleep(0.4)  # the first message started the clock; the window is open by now
    out = run(moves(naive, 1), 1.5)
    t.check(first.values == [1] and out.hung, "half-open: a naive client with no timeout waits for ever on a connection that is open and silent")
    t.check(bench.injected("blackholed") > 0, "half-open: the proxy swallowed traffic on an open connection (its own count)")
    bench.close()
    bench = Bench("tcp", "blackhole=700ms@300ms")
    careful = Careful(bench.link(), per_try=0.15, tries=12, deadline=4.0)
    out = run(moves(careful, 10, pause=0.12), 12)
    t.check(bench.injected("blackholed") > 0 and clean(out, 10), f"half-open: a careful client times out, resends, and finishes all ten requests once the window closes ({len(out.values)})")
    bench.close()

    # 10. kill -9 and restart: a client that does not reconnect, and one that does and notices what it lost.
    bench = Bench("tcp", "kill=300ms@400ms", supervised=True)
    naive = Naive(bench.link(), timeout=1.0)

    def kill_naive(outcome: Outcome) -> None:
        for _ in range(12):
            try:
                outcome.values.append(naive.request(MOVE, 1))
            except (OSError, struct.error) as error:
                outcome.errors.append(error)
            time.sleep(0.1)

    out = run(kill_naive, 10)
    t.check(bench.relay.supervised.restarts == 1, "kill: the proxy killed the substrate with SIGKILL and started it again (its own count)")
    t.check(len(out.errors) >= 4 and 12 not in out.values, f"kill: a naive client never reconnects: every request after the death fails ({len(out.errors)} errors)")
    bench.close()
    bench = Bench("tcp", "kill=300ms@400ms", supervised=True)
    careful = Careful(bench.link(), per_try=0.2, tries=40, deadline=8.0)
    restarted: list[str] = []
    after: list[int] = []

    def kill_careful(outcome: Outcome) -> None:
        for _ in range(12):
            try:
                value = careful.request(MOVE, 1)
                outcome.values.append(value)
                if restarted:
                    after.append(value)
            except PeerRestarted as error:
                restarted.append(str(error))
            time.sleep(0.1)

    out = run(kill_careful, 16)
    t.check(out.error is None and len(restarted) == 1 and len(after) >= 3, f"kill: a careful client reconnects, is told once that the substrate restarted, and goes on ({len(restarted)} restart noticed, {len(after)} answers after)")
    t.check(bool(after) and after[0] <= 3 and all(b == a + 1 for a, b in zip(after, after[1:])), f"kill: and what it reads after is the new session's own count from the start ({after[:4]}), not a continuation of the dead one's")
    bench.close()

    # 11. slow reader: a blocking send that holds the lock the stop path needs.
    for label, careful_side in (("naive", False), ("careful", True)):
        bench = Bench("tcp", "slow=400", buffer=2048, cap=256)
        link = Link("tcp", bench.relay.listen_address, 2048)
        bench.links.append(link)
        lock = threading.Lock()
        stop_time: list[float] = []
        assert link.socket is not None
        if careful_side:
            link.socket.setblocking(False)

        done = threading.Event()
        # A command with a trajectory in it: four kilobytes, which at 400 bytes a second is ten seconds of a blocking send.
        bulk = bytes(4080)

        def writer(link=link, lock=lock, careful_side=careful_side, done=done) -> None:
            for index in range(6000):
                if done.is_set():
                    return
                with lock:
                    try:
                        if careful_side:
                            try:
                                link.socket.send(pack(MOVE, 1000 + index, 1) + bulk)  # type: ignore[union-attr]
                            except (BlockingIOError, InterruptedError):
                                pass  # the outbox is full: this command is dropped, and nothing waits
                        else:
                            link.socket.sendall(pack(MOVE, 1000 + index, 1) + bulk)  # type: ignore[union-attr]
                    except (OSError, AttributeError):
                        return
                if careful_side:
                    time.sleep(0.0005)

        def stop(link=link, lock=lock, careful_side=careful_side, stop_time=stop_time) -> None:
            started = time.monotonic()
            if careful_side:
                try:
                    link.socket.send(pack(STOP, 1))  # type: ignore[union-attr]
                except (BlockingIOError, InterruptedError, OSError):
                    pass  # would block: say so at once rather than wait (SKR_EIO)
            else:
                try:
                    with lock:  # the stop path takes the same lock and the same blocking send
                        link.socket.sendall(pack(STOP, 1))  # type: ignore[union-attr]
                except (OSError, AttributeError):
                    pass  # the bench was closed under it, which is how a stuck stop ends
            stop_time.append(time.monotonic() - started)

        threading.Thread(target=writer, daemon=True).start()
        time.sleep(0.8)  # long enough for every buffer on the way to fill at 400 bytes a second
        stopper = threading.Thread(target=stop, daemon=True)
        stopper.start()
        stopper.join(1.0)
        if label == "naive":
            t.check(stopper.is_alive(), "slow reader: a naive stop waits behind a blocked send for as long as the substrate is slow (here: still waiting after a second)")
        else:
            t.check(bool(stop_time) and stop_time[0] < 0.05, f"slow reader: a stop with a bounded, non-blocking send returns at once ({stop_time[0] * 1000 if stop_time else -1:.1f} ms)")
        done.set()
        bench.close()

    # 12. a clock step / stall: a burst of old samples read as fresh ones.
    def consume(link: Link, check_age: bool):
        def work(outcome: Outcome) -> None:
            link.send(pack(HELLO, 1))
            fresh = stale = 0
            end = time.monotonic() + 1.4
            while time.monotonic() < end:
                chunk = link.recv(0.05)
                if chunk is None:
                    continue
                try:
                    kind, _seq, stamp, _ = unpack(chunk)
                except ValueError:
                    continue
                if kind != STATE:
                    continue
                age_ms = ((int(time.monotonic() * 1000) & 0x7FFFFFFF) - stamp) & 0x7FFFFFFF
                if check_age and age_ms > 100:
                    stale += 1
                    continue
                if age_ms > 100:
                    outcome.errors.append(ValueError(f"used a sample {age_ms} ms old as fresh"))
                fresh += 1
            outcome.values = [fresh, stale]

        return work

    bench = Bench("udp", "down:stall=500ms@250ms")
    out = run(consume(bench.link(), False), 4)
    t.check(bench.counters("down")["stalled"] > 0 and len(out.errors) >= 10, f"stall: a client that trusts arrival time uses a burst of old samples as fresh ({len(out.errors)} of them, up to 500 ms old)")
    bench.close()
    bench = Bench("udp", "down:stall=500ms@250ms")
    out = run(consume(bench.link(), True), 4)
    t.check(not out.errors and out.values[1] >= 10 and out.values[0] >= 20, f"stall: one that reads each sample's own stamp discards the burst ({out.values[1]} stale) and uses only fresh ones ({out.values[0]})")
    bench.close()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--serve", choices=["tcp", "udp"], help="run the substrate, for the scenarios that kill it")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--session", type=int)
    parser.add_argument("--only", choices=["engine", "relays", "scenarios"], help="one part")
    options = parser.parse_args(argv)
    if options.serve:
        substrate = Substrate(options.serve, options.port, options.session).start()
        print(f"serving {options.serve} {substrate.address[1]} session {substrate.session}", flush=True)
        while True:
            time.sleep(3600)
    t = Checks()
    if options.only in (None, "engine"):
        runner_flag_checks(t)
        engine_checks(t)
    if options.only in (None, "relays"):
        relay_checks(t)
    if options.only in (None, "scenarios"):
        scenarios(t)
    print(f"\n{t.count} checks, {t.failed} failed", flush=True)
    return 1 if t.failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

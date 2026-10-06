#!/usr/bin/env python3
"""The link-chaos contract: what a driver must do when the link to its substrate is worse than the one it was written on.

DRY-COVERAGE-PLAN.md item 12.C.4. `selftest.py` shows the proxy's faults against a toy; this runs them against a *driver*. A
driver supplies a **subject** (`drivers/<name>/chaos.py`): how to start its substrate, what the driver's configuration looks
like when it is pointed at relays instead, and the driver's four entry points. The runner puts a relay on every flow between the
two, opens the driver over a clean link, sets a profile, and asks the same few questions of every driver:

  1. **bounded** — no entry point blocks past its bound whatever the link does: a read two seconds, a write a quarter of one (it is
     the control loop's, and the loop waits for it), the emergency stop 50 ms (the RHAL's "never blocks").
  2. **stop** — `emergency_stop` while the link is dead or crawling returns at once, and the latch holds: the next command is
     refused. A stop that waited for the link would be a stop that waited.
  3. **outage reported** — when the link stops carrying data (a blackhole, a reset, a stall, a link so slow the substrate's
     stream backs up) the driver says so within its staleness bound: a read that returns anything but OK, sets a fault or
     degraded flag, or — for a subject whose `read()` gives its sequence as a fourth value — **holds its sequence**, which is
     §24's own word for a stalled producer (`seq` MUST advance; the watchdog treats a repeat as one). A read that keeps returning
     OK with frozen data and a sequence that advances is a robot that looks alive.
  4. **recovers** — when the link is healed the driver reads OK again within a few seconds, or refuses in a *named* way that
     a re-open clears. It does not stay wedged, and it does not come back with the latch of a stop nobody asked for.
  5. **fresh** — an OK read's data is no older than the driver's own staleness bound, measured against the substrate's truth
     (the subject says how fast its arm moves, so a lag is an age). Data that arrives late and is believed to be fresh is the
     failure `slow` and `stall` are for.
  6. **effective** — for a subject whose substrate can say whether it acted on a command (`commands_taken`): once the link is
     healed and the driver is commanded again, the substrate acts on what it is sent. A datagram nothing answers is a command the
     driver's return code calls taken as soon as the socket has it, so a machine that stopped listening during the outage — a
     robot that takes its legs back after a second without a command — and is never told to listen again is a driver whose every
     write is OK and whose arm does not move.

It says nothing about whether the driver's protocol survives a *corrupt* or *truncated* byte on a TCP stream — real TCP does not
deliver one; those faults belong to the datagram transports, where a subject lists them.

A subject is a class with these members (see `drivers/xarm/chaos.py`):

    name, flows                 [("tcp", (host, port)) | ("udp", (host, port)) | ("can", (driver_if, substrate_if)) | ("canfd", (driver_if, substrate_if)) | ("serial", None) | ("netem", "lo") ...]
                                a `canfd` flow is a `can` one whose relay carries CAN-FD frames (64 bytes, 29-bit identifiers, the flags with them), over interfaces made with `mtu 72`
                                a `netem` flow is the kernel's own link emulation on an interface (`NetemRelay`), for a substrate no relay can stand in front of (DDS, ROS 2): everything on the interface meets the
                                profile, both directions at once, so the substrate and the driver share it, a subject's `profiles` leave out what `netem` cannot do (`truncate`, `stall`, a scoped `slow-link`, `reset`: a profile it lists
                                that the kernel cannot carry is a failed check and nothing is run, `profiles.json` marks the ones it can with `netem`), and the
                                arm's position and the count of commands acted on come through a pipe and not a socket
    start() / stop()            the substrate, and the addresses it listens on (filled into `flows` by `start`)
    attach(addresses)           optional: called once the relays exist and before `config`, for a substrate that has to open its own end of a relay
                                (a serial flow's address is `(the driver's pty path, the substrate's pty path)`)
    config(addresses)           the driver's JSON configuration, pointed at the relays' addresses, one per flow
    open(config) -> rc          and read() -> (rc, flags, positions[, seq]), write(positions) -> rc, stop_call() -> rc, release(), close()
                                (an entry point that *raises* is a failed check naming the exception, said once per profile — F257 — and the run goes on)
    target(i) -> positions      two far poses, i = 0 and 1, valid for the body, so that it is always on its way from one to the other
    truth() -> positions        where the substrate's arm really is
    commands_taken() -> int     optional: how many commands the substrate has acted on, counting none that it received while it was not listening (question 6)
    speed                       how fast the body moves when told to go somewhere else, in the unit `read()` and `truth()` report positions in per
                                second (rad/s for an arm, m/s for a vehicle whose pose they are; 0 if it does not move)
    stale_s                     the driver's staleness bound in seconds
    write_bound_s               optional: how long `write` may take. A joint command is the control loop's and a quarter of a second is its bound (the default); a
                                body moved by motions whose call waits for the vehicle's answer says how long its own motion may
    read_bound_s                optional: how long `read` may take, where two seconds (the default) is the control loop's. A body whose state is a cloud's (no loop is closed over
                                the link) says what its own idempotent reads are given; a request that creates something is never shortened to meet it
    profiles                    the profile names that apply to this subject's transports
    expect                      optional {profile name: expectations} that replace a profile's own for this subject, and say why in the subject: a read that is several messages which must all
                                arrive (a servo bus's sync read is a request and six replies) cannot meet `min_ok_fraction` on a link that loses one message in ten, and the subject states what it can, by arithmetic

    python3 tools/faultproxy/chaos.py drivers/xarm/chaos.py            # the driver's own subject, every profile it lists
    python3 tools/faultproxy/chaos.py drivers/xarm/chaos.py --only blackhole-2s
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import math
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.join(HERE, "..", "..", "bindings", "python"))

import skynet_rhal as rhal  # noqa: E402
from engine import Engine  # noqa: E402
from relays import CanRelay, NetemRelay, RawRelay, SerialRelay, TcpRelay, UdpRelay  # noqa: E402

RC_OK = 0
#: The ABI's own bits, imported and not retyped (F196): this file once spelled them `0x0004` and `0x0010`, which are DEGRADED and the
#: homing field's low bit — set by every driver that answers `not_required` — so "the driver said it was degraded" held on every read.
FLAG_FAULT = rhal.FLAG_FAULT
FLAG_DEGRADED = rhal.FLAG_DEGRADED


def unhealthy(flags: int) -> bool:
    """Whether a driver, by its flags, has said it is faulted or degraded. A stop or a brake is a different question."""
    return bool(flags & (FLAG_FAULT | FLAG_DEGRADED))


def load_profiles() -> dict:
    with open(os.path.join(HERE, "profiles.json"), encoding="utf-8") as handle:
        return json.load(handle)["profiles"]


class Result:
    def __init__(self) -> None:
        self.count = 0
        self.failed = 0

    def check(self, condition: bool, what: str) -> bool:
        self.count += 1
        print(f"  {'ok  ' if condition else 'FAIL'}  {what}", flush=True)
        if not condition:
            self.failed += 1
        return bool(condition)


def timed(call):
    started = time.monotonic()
    value = call()
    return value, time.monotonic() - started


class Unsupported(ValueError):
    """A profile one of the subject's relays cannot carry: the kernel's link emulation cannot do everything the engine does (`relays.netem_arguments` says what, for each fault)."""


class Session:
    """One profile, one fresh substrate and driver, the relays between them. With `kernel`, every TCP and UDP flow is carried by the kernel's own link emulation on the namespace's loopback instead of by a relay:
    the driver is given the substrate's own addresses, and one queue carries everything (see `--kernel`)."""

    def __init__(self, subject, spec: str, seed: int = 1, kernel: bool = False) -> None:
        self.subject = subject
        subject.start()
        if kernel:
            foreign = [kind for kind, _ in subject.flows if kind not in ("tcp", "udp", "netem")]
            if foreign:
                subject.stop()
                raise Unsupported(f"the kernel's link emulation carries IP, and this subject has a `{foreign[0]}` flow")
        if kernel or any(kind == "netem" for kind, _ in subject.flows):
            try:
                NetemRelay.accepts(spec)
            except ValueError as error:
                subject.stop()
                raise Unsupported(str(error)) from error
        self.engines: list[Engine] = []
        self.relays = []
        addresses = []
        shared: NetemRelay | None = None
        for kind, target in subject.flows:
            if kernel and kind in ("tcp", "udp"):
                if shared is None:
                    engine = Engine("", seed)
                    self.engines.append(engine)
                    shared = NetemRelay(engine, "lo").start()
                    self.relays.append(shared)
                addresses.append(target)
                continue
            engine = Engine("", seed, ordered=kind in ("tcp", "serial"))
            self.engines.append(engine)
            if kind == "tcp":
                relay = TcpRelay(engine, ("127.0.0.1", 0), target).start()
                addresses.append(relay.listen_address)
            elif kind == "udp":
                relay = UdpRelay(engine, ("127.0.0.1", 0), target).start()
                addresses.append(relay.listen_address)
            elif kind in ("can", "canfd"):
                relay = CanRelay(engine, *target, fd=kind == "canfd").start()
                addresses.append(target[0])
            elif kind == "raw":
                relay = RawRelay(engine, *target).start()
                addresses.append(target[0])
            elif kind == "netem":
                relay = NetemRelay(engine, target).start()
                addresses.append(target)
            elif kind == "serial":
                # Two pseudo-terminals with the relay between them. The address is (the driver's end, the substrate's end): the driver is configured with the first, and a substrate that has to open
                # its own end does it in `attach`, below, because it cannot until the relay exists.
                relay = SerialRelay(engine).start()
                addresses.append((relay.driver_path, relay.substrate_path))
            else:
                raise SystemExit(f"chaos: a flow of kind `{kind}` is not one this runner relays")
            self.relays.append(relay)
        attach = getattr(subject, "attach", None)
        if attach is not None:
            attach(addresses)
        self.config = subject.config(addresses)
        self.spec = spec

    def apply(self, spec: str) -> None:
        """Set the profile on every flow and start every flow's clock now, so a fault's `@` onset is the same instant on a flow that carries traffic all the time and on one that carries none until a stop asks for it."""
        started = time.monotonic()
        for engine in self.engines:
            engine.set_profile(spec)
            if spec:
                engine.start(started)
        for relay in self.relays:
            # A relay that follows the profile by a clock of its own (the kernel's) is made to have it now, and says if it could not.
            refresh = getattr(relay, "refresh", None)
            if refresh is not None:
                refresh()

    def heal(self) -> None:
        self.apply("")

    def close(self) -> None:
        try:
            self.subject.close()
        except Exception:  # noqa: BLE001 — closing a body whose link is broken is still closing it
            pass
        for relay in self.relays:
            relay.stop()
        self.subject.stop()


def settle(subject, seconds: float) -> int:
    """Keep the arm moving — a new far target every tick — while time passes, so a stale reading is a wrong one. Returns the
    last command's status: a harness whose commands are refused over a clean link would be measuring nothing."""
    end = time.monotonic() + seconds
    flip = 0
    rc = RC_OK
    while time.monotonic() < end:
        rc = subject.write(subject.target(flip))
        flip ^= 1
        time.sleep(0.05)
    return rc


RC_EIO = -5


def run_profile(subject, name: str, profile: dict, result: Result, options) -> None:
    spec = profile["spec"]
    print(f"\n{name}: {spec}", flush=True)
    try:
        session = Session(subject, spec, kernel=getattr(options, "kernel", False))
    except Unsupported as error:
        # A subject that lists a profile its own link cannot carry is a finding about the subject, and a clean link run under that name would be a pass for nothing.
        result.check(False, f"{name}: the subject's kernel link cannot carry this profile, so nothing was run — {error}")
        return
    raised: set[str] = set()

    def trapped(entry: str, call, otherwise):
        """An entry point that raises has failed in a way a return code cannot say (F257: an `http.client.IncompleteRead` out of `read_state` and `emergency_stop`). It is one failed check naming the
        exception, said once per entry point and profile, and the run goes on with `otherwise` as the answer, so what is behind it is still measured."""
        try:
            return call()
        except Exception as error:  # noqa: BLE001 — what a driver lets escape is the finding
            if entry not in raised:
                raised.add(entry)
                result.check(False, f"{name}: {entry} raised {type(error).__name__} out of the entry point — {error}")
            return otherwise

    def read():
        """`subject.read()` as (rc, flags, positions, seq): the sequence is None for a subject that does not give one."""
        got = trapped("read_state", subject.read, (RC_EIO, 0, [], None))
        return got if len(got) == 4 else (*got, None)

    def write(values) -> int:
        return trapped("write_command", lambda: subject.write(values), RC_EIO)

    last_seq = None

    def held(seq) -> bool:
        """Whether this read repeated the sequence of the one before it: §24's stalled producer, a driver saying the stream stopped without a flag. Remembers the sequence for the next."""
        nonlocal last_seq
        repeated = seq is not None and last_seq is not None and seq == last_seq
        if seq is not None:
            last_seq = seq
        return repeated

    try:
        rc, took = timed(lambda: subject.open(session.config))
        if not result.check(rc == RC_OK, f"{name}: the driver opens over a clean relay ({took:.2f} s)"):
            return
        if not result.check(settle(subject, 0.6) == RC_OK, f"{name}: a command is taken over a clean relay"):
            return
        session.apply(spec)

        bounds = {"read": profile.get("read_bound_s", getattr(subject, "read_bound_s", 2.0)), "write": profile.get("write_bound_s", getattr(subject, "write_bound_s", 0.25))}
        worst = {"read": 0.0, "write": 0.0}
        oks = 0
        reads = 0
        ages = []
        end = time.monotonic() + profile.get("duration_s", 3.0)
        flip = 0
        while time.monotonic() < end:
            wrc, wtook = timed(lambda: write(subject.target(flip)))
            worst["write"] = max(worst["write"], wtook)
            flip ^= 1
            (rrc, flags, positions, seq), rtook = timed(read)
            stalled = held(seq)
            worst["read"] = max(worst["read"], rtook)
            reads += 1
            if rrc == RC_OK:
                oks += 1
                # A pose that is NaN is a driver that says it has none, which is the honest answer to a link that stopped carrying data and not a stale value believed fresh. A read that held its sequence says the same thing
                # in §24's words and does not claim to be fresh either.
                if not stalled and subject.speed > 0 and positions and not any(math.isnan(value) for value in positions):
                    lag = max(abs(a - b) for a, b in zip(positions, subject.truth()))
                    ages.append(lag / subject.speed)
            time.sleep(0.02)
        result.check(worst["read"] <= bounds["read"] and worst["write"] <= bounds["write"], f"{name}: bounded — the slowest read took {worst['read'] * 1000:.0f} ms and the slowest write {worst['write'] * 1000:.0f} ms (bounds {bounds['read']:.2f} s and {bounds['write']:.2f} s)")

        expect = {**profile.get("expect", {}), **getattr(subject, "expect", {}).get(name, {})}
        if "min_ok_fraction" in expect:
            fraction = oks / max(reads, 1)
            result.check(fraction >= expect["min_ok_fraction"], f"{name}: the link is poor and not dead, so reads mostly succeed ({oks} of {reads} OK, at least {expect['min_ok_fraction']:.0%} wanted)")
        if expect.get("outage"):
            # The last reads of a dead link must not be OK with data that stopped moving.
            tail = []
            for _ in range(10):
                rrc, flags, positions, seq = read()
                tail.append((rrc, flags, held(seq)))
                time.sleep(0.05)
            said = any(rrc != RC_OK or unhealthy(flags) or stalled for rrc, flags, stalled in tail)
            holding = sum(1 for _, _, stalled in tail if stalled)
            result.check(said, f"{name}: outage reported — within the staleness bound ({subject.stale_s:.1f} s) a read says the link is not carrying data ({sum(1 for rrc, _, _ in tail if rrc != RC_OK)} of {len(tail)} reads not OK" + (f", {holding} holding their sequence" if holding else "") + ")")
        if ages and subject.speed > 0:
            limit = subject.stale_s + profile.get("age_margin_s", 0.5)
            worst_age = max(ages)
            result.check(worst_age <= limit, f"{name}: fresh — the oldest OK read was {worst_age:.2f} s behind the arm, against a staleness bound of {subject.stale_s:.1f} s")

        if expect.get("stop"):
            # A profile's `@` times count from the first message on a flow after it is set (`engine.py`), so setting it and stopping at once puts the stop's own request ahead of the fault: a stop made of
            # one request was sent down a link that had not yet gone wrong, and this check could not fail for it (found on Fourier, whose stop is a synchronous HTTP call: 3.8 ms "with the link like this"
            # against a blackhole that had not started). `apply` starts every flow's clock, which a read no longer has to do for the one flow it touches (the UR bridge's stop asks the dashboard, which no read
            # ever does), and the stop is made 0.6 s later, inside a stall (0.5 s to 2 s) and past a blackhole's onset (0.3 s).
            primed = time.monotonic()
            session.apply(spec)
            read()
            time.sleep(max(0.0, 0.6 - (time.monotonic() - primed)))
            rc, took = timed(lambda: trapped("emergency_stop", subject.stop_call, RC_EIO))
            result.check(took <= 0.05, f"{name}: stop — emergency_stop returned in {took * 1000:.1f} ms with the link like this (50 ms wanted)")
            result.check(write(subject.target(0)) != RC_OK, "  and the latch holds: the command after the stop is refused")
            trapped("release_stop", subject.release, None)

        session.heal()
        recovered = False
        give_up = time.monotonic() + profile.get("recover_s", 6.0)
        reopened = False
        while time.monotonic() < give_up:
            rrc, flags, positions, seq = read()
            if rrc == RC_OK and not unhealthy(flags) and not held(seq):
                wrc = write(subject.target(0))
                if wrc == RC_OK:
                    recovered = True
                    break
            time.sleep(0.1)
        if not recovered:
            # A driver may answer a dead link by refusing until it is opened again. That is allowed; staying wedged is not.
            trapped("close", subject.close, None)
            last_seq = None
            rc, _ = timed(lambda: trapped("open", lambda: subject.open(session.config), RC_EIO))
            reopened = rc == RC_OK
            if reopened:
                rrc, _, _, _ = read()
                reopened = rrc == RC_OK
        result.check(recovered or reopened, f"{name}: recovers — after the link is healed the driver " + ("reads and is commanded again" if recovered else ("is wedged until it is opened again, and opens and reads then" if reopened else "neither recovers nor opens again: it is wedged")))

        taken_by = getattr(subject, "commands_taken", None)
        if taken_by is not None and (recovered or reopened):
            # A command to a substrate nothing answers is OK as soon as the socket has it, so the return code above cannot say whether the machine is listening. Ask it: after the link is healed and
            # the driver is commanded for a while, the substrate must have acted on something. Found on Lite3, whose motion host takes the legs back after a second without a command and which
            # the driver only re-took control from when *its own* sends had been quiet (F276).
            before = taken_by()
            until = time.monotonic() + 1.5
            tick = 0
            while time.monotonic() < until:
                write(subject.target(tick))
                tick ^= 1
                read()
                time.sleep(0.05)
            acted = taken_by() - before
            result.check(acted > 0, f"{name}: effective — after the link is healed the substrate acts on the driver's commands ({acted} taken in 1.5 s)")
    finally:
        session.close()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("subject", help="a Python file defining `subject()`, e.g. drivers/xarm/chaos.py")
    parser.add_argument("--only", action="append", help="a profile by name; repeatable")
    options = parser.parse_args(argv)
    spec = importlib.util.spec_from_file_location("chaos_subject", options.subject)
    module = importlib.util.module_from_spec(spec)
    sys.path.insert(0, os.path.dirname(os.path.abspath(options.subject)))
    spec.loader.exec_module(module)
    profiles = load_profiles()
    result = Result()
    subject = module.subject()
    print(f"link chaos: {subject.name}")
    for name in subject.profiles:
        if options.only and name not in options.only:
            continue
        run_profile(subject, name, profiles[name], result, options)
    print(f"\n{result.count} checks, {result.failed} failed", flush=True)
    return 1 if result.failed else 0


if __name__ == "__main__":
    raise SystemExit(main())

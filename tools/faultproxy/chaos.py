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
     stream backs up) the driver says so within its staleness bound: a read that returns anything but OK, or sets a fault or
     degraded flag. A read that keeps returning OK with frozen data is a robot that looks alive.
  4. **recovers** — when the link is healed the driver reads OK again within a few seconds, or refuses in a *named* way that
     a re-open clears. It does not stay wedged, and it does not come back with the latch of a stop nobody asked for.
  5. **fresh** — an OK read's data is no older than the driver's own staleness bound, measured against the substrate's truth
     (the subject says how fast its arm moves, so a lag is an age). Data that arrives late and is believed to be fresh is the
     failure `slow` and `stall` are for.

It says nothing about whether the driver's protocol survives a *corrupt* or *truncated* byte on a TCP stream — real TCP does not
deliver one; those faults belong to the datagram transports, where a subject lists them.

A subject is a class with these members (see `drivers/xarm/chaos.py`):

    name, flows                 [("tcp", (host, port)) | ("udp", (host, port)) | ("can", (driver_if, substrate_if)) | ("serial", None) ...]
    start() / stop()            the substrate, and the addresses it listens on (filled into `flows` by `start`)
    attach(addresses)           optional: called once the relays exist and before `config`, for a substrate that has to open its own end of a relay
                                (a serial flow's address is `(the driver's pty path, the substrate's pty path)`)
    config(addresses)           the driver's JSON configuration, pointed at the relays' addresses, one per flow
    open(config) -> rc          and read() -> (rc, flags, positions), write(positions) -> rc, stop_call() -> rc, release(), close()
    target(i) -> positions      two far poses, i = 0 and 1, valid for the body, so that it is always on its way from one to the other
    truth() -> positions        where the substrate's arm really is
    speed                       how fast the body moves when told to go somewhere else, in the unit `read()` and `truth()` report positions in per
                                second (rad/s for an arm, m/s for a vehicle whose pose they are; 0 if it does not move)
    stale_s                     the driver's staleness bound in seconds
    write_bound_s               optional: how long `write` may take. A joint command is the control loop's and a quarter of a second is its bound (the default); a
                                body moved by motions whose call waits for the vehicle's answer says how long its own motion may
    read_bound_s                optional: how long `read` may take, where two seconds (the default) is the control loop's. A body whose state is a cloud's (no loop is closed over
                                the link) says what its own idempotent reads are given; a request that creates something is never shortened to meet it
    profiles                    the profile names that apply to this subject's transports

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
from relays import CanRelay, RawRelay, SerialRelay, TcpRelay, UdpRelay  # noqa: E402

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


class Session:
    """One profile, one fresh substrate and driver, the relays between them."""

    def __init__(self, subject, spec: str, seed: int = 1) -> None:
        self.subject = subject
        subject.start()
        self.engines: list[Engine] = []
        self.relays = []
        addresses = []
        for kind, target in subject.flows:
            engine = Engine("", seed, ordered=kind in ("tcp", "serial"))
            self.engines.append(engine)
            if kind == "tcp":
                relay = TcpRelay(engine, ("127.0.0.1", 0), target).start()
                addresses.append(relay.listen_address)
            elif kind == "udp":
                relay = UdpRelay(engine, ("127.0.0.1", 0), target).start()
                addresses.append(relay.listen_address)
            elif kind == "can":
                relay = CanRelay(engine, *target).start()
                addresses.append(target[0])
            elif kind == "raw":
                relay = RawRelay(engine, *target).start()
                addresses.append(target[0])
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
        for engine in self.engines:
            engine.set_profile(spec)

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


def run_profile(subject, name: str, profile: dict, result: Result, options) -> None:
    spec = profile["spec"]
    print(f"\n{name}: {spec}", flush=True)
    session = Session(subject, spec)
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
            (wrc, _), wtook = timed(lambda: (subject.write(subject.target(flip)), None))
            worst["write"] = max(worst["write"], wtook)
            flip ^= 1
            (rrc, flags, positions), rtook = timed(subject.read)
            worst["read"] = max(worst["read"], rtook)
            reads += 1
            if rrc == RC_OK:
                oks += 1
                # A pose that is NaN is a driver that says it has none, which is the honest answer to a link that stopped carrying data and not a stale value believed fresh.
                if subject.speed > 0 and positions and not any(math.isnan(value) for value in positions):
                    lag = max(abs(a - b) for a, b in zip(positions, subject.truth()))
                    ages.append(lag / subject.speed)
            time.sleep(0.02)
        result.check(worst["read"] <= bounds["read"] and worst["write"] <= bounds["write"], f"{name}: bounded — the slowest read took {worst['read'] * 1000:.0f} ms and the slowest write {worst['write'] * 1000:.0f} ms (bounds {bounds['read']:.2f} s and {bounds['write']:.2f} s)")

        expect = profile.get("expect", {})
        if "min_ok_fraction" in expect:
            fraction = oks / max(reads, 1)
            result.check(fraction >= expect["min_ok_fraction"], f"{name}: the link is poor and not dead, so reads mostly succeed ({oks} of {reads} OK, at least {expect['min_ok_fraction']:.0%} wanted)")
        if expect.get("outage"):
            # The last reads of a dead link must not be OK with data that stopped moving.
            tail = []
            for _ in range(10):
                rrc, flags, positions = subject.read()
                tail.append((rrc, flags))
                time.sleep(0.05)
            said = any(rrc != RC_OK or unhealthy(flags) for rrc, flags in tail)
            result.check(said, f"{name}: outage reported — within the staleness bound ({subject.stale_s:.1f} s) a read says the link is not carrying data ({sum(1 for rrc, _ in tail if rrc != RC_OK)} of {len(tail)} reads not OK)")
        if ages and subject.speed > 0:
            limit = subject.stale_s + profile.get("age_margin_s", 0.5)
            worst_age = max(ages)
            result.check(worst_age <= limit, f"{name}: fresh — the oldest OK read was {worst_age:.2f} s behind the arm, against a staleness bound of {subject.stale_s:.1f} s")

        if expect.get("stop"):
            session.apply(spec)
            # A profile's `@` times count from the first message after it is set (`engine.py`), so setting it and stopping at once puts the stop's own request ahead of the fault: a stop made of one
            # request was sent down a link that had not yet gone wrong, and this check could not fail for it (found on Fourier, whose stop is a synchronous HTTP call: 3.8 ms "with the link like this"
            # against a blackhole that had not started). One read starts the clock, and the stop is made 0.6 s later, inside a stall (0.5 s to 2 s) and past a blackhole's onset (0.3 s).
            primed = time.monotonic()
            subject.read()
            time.sleep(max(0.0, 0.6 - (time.monotonic() - primed)))
            rc, took = timed(subject.stop_call)
            result.check(took <= 0.05, f"{name}: stop — emergency_stop returned in {took * 1000:.1f} ms with the link like this (50 ms wanted)")
            result.check(subject.write(subject.target(0)) != RC_OK, "  and the latch holds: the command after the stop is refused")
            subject.release()

        session.heal()
        recovered = False
        give_up = time.monotonic() + profile.get("recover_s", 6.0)
        reopened = False
        while time.monotonic() < give_up:
            rrc, flags, positions = subject.read()
            if rrc == RC_OK and not unhealthy(flags):
                wrc = subject.write(subject.target(0))
                if wrc == RC_OK:
                    recovered = True
                    break
            time.sleep(0.1)
        if not recovered:
            # A driver may answer a dead link by refusing until it is opened again. That is allowed; staying wedged is not.
            subject.close()
            rc, _ = timed(lambda: subject.open(session.config))
            reopened = rc == RC_OK
            if reopened:
                rrc, _, _ = subject.read()
                reopened = rrc == RC_OK
        result.check(recovered or reopened, f"{name}: recovers — after the link is healed the driver " + ("reads and is commanded again" if recovered else ("is wedged until it is opened again, and opens and reads then" if reopened else "neither recovers nor opens again: it is wedged")))
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

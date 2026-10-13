"""A controller that stops answering, seen through the `ros2_control` bridge and a vendor's own hardware plugin (DRY-COVERAGE-PLAN.md item 12.C.4, the last body the link-chaos contract had not reached).

    make -C drivers/ros2_control frozen-ursim-docker     # UR's plugin behind URSim
    make -C drivers/ros2_control frozen-drcf-docker      # Doosan's behind DRCF
    make -C drivers/ros2_control frozen-rsi-docker       # KUKA's behind its RSI simulator (stopped with SIGSTOP: it shares the plugin's container)
    make -C drivers/ros2_control frozen-arcs-docker      # AUBO's behind ARCS

`tools/faultproxy/chaos.py` puts faults on a link the driver reaches its robot by, and the kernel's queue on a loopback cannot be used here: URSim's own processes talk to each other over the
loopback it shares with the plugin, so a delay or a loss there is a fault inside the simulator and not on the line to it. What the bridge most needs to survive is simpler and more usual than a
bad link: **the controller stops answering and keeps its connections open** (it crashed, hung, was paused for a debugger, lost its power with a switch between it and the host, or sits behind a cable
somebody pulled). Docker's `pause` is that: the cgroup freezer stops every process of the simulator at once, nothing is reset, the TCP connections stay up and nothing is sent back.

This script is the driver's side, run inside the ROS image in the controller's network namespace. `frozen-host.sh` is the host's, with one `run-frozen.sh` per vendor to start the controller: it waits for this
script to say `ready`, pauses the controller's container, waits for `measured`, lets it go again and waits for `done`. The two talk through files in a directory both can see, because the container that holds this script has no Docker.

What is asked, in the order of the contract's questions (`tools/faultproxy/chaos.py`):

* **bounded**: no read takes longer than 2 s and no write longer than 0.25 s while the controller is frozen;
* **outage reported**: a read says the controller is not answering (a result other than OK, or the fault flag) within the time the maker's own plugin takes to give up, which is measured, and printed;
* **stop**: `emergency_stop` returns inside the 50 ms the RHAL allows and the latch it sets holds (the command after it is refused);
* **recovers**: after the controller is let go, a body closed and opened again reads and is commanded again (a plugin that failed stays down until then, F142).

`FROZEN_LIB`, `FROZEN_CONFIG`, `FROZEN_DIR`, `FROZEN_JOINTS` and `FROZEN_WAKE` (a command that brings the simulator to a state the body can be opened in) say where everything is.

`FROZEN_OUTAGE_REPORTED=no` records a measured gap (F332): the four makers' plugins (Universal Robots', Doosan's, KUKA's and AUBO's) return OK from `read()` with the values they last had when their controller stops answering, so *outage reported* cannot pass on any of them whatever the bridge does. That target then asks the
question the other way round: it passes while the plugin stays silent and fails the day it says the controller is gone, so the gap cannot close without someone being told. The other four questions are asked as they are.

`FROZEN_WRITE_BOUNDED=no` does the same for *bounded* (F333): AUBO's `write()` sends its command by an RPC that waits a second for the controller's reply, so with the controller frozen every write takes a second, four times the 0.25 s the contract allows. That
check then passes while a write waits (and none waits longer than 2 s), and fails the day none does.
"""

from __future__ import annotations

import math
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))

from rhal_ctypes import Driver, SKR_OK  # noqa: E402

#: `bindings/c/skynet_rhal.h`'s bits; the fault and degraded flags are the two a bridge sets when a plugin's read or write fails.
SKR_FLAG_ESTOP = 1 << 0
SKR_FLAG_FAULT = 1 << 1
SKR_FLAG_DEGRADED = 1 << 2

#: How long the controller is left frozen, and how often the driver is called meanwhile (the 50 Hz of the commanding contract: every write follows a read, F98).
FROZEN_FOR_S = 6.0
ROUND_S = 0.02
#: The contract's bounds.
READ_BOUND_S = 2.0
WRITE_BOUND_S = 0.25
STOP_BOUND_S = 0.05

failures = 0
checks = 0


def check(name: str, what: str, ok: bool) -> bool:
    global failures, checks
    checks += 1
    if not ok:
        failures += 1
    print(f"  {'ok  ' if ok else 'FAIL'}  {name}: {what}", flush=True)
    return ok


def finite(values) -> bool:
    return all(math.isfinite(v) for v in values)


def say(message: str) -> None:
    print(message, flush=True)


def handshake(directory: str, say_word: str, wait_for: str | None, timeout_s: float = 120.0) -> None:
    open(os.path.join(directory, say_word), "w", encoding="utf-8").close()
    if wait_for is None:
        return
    end = time.monotonic() + timeout_s
    path = os.path.join(directory, wait_for)
    while not os.path.exists(path):
        if time.monotonic() > end:
            raise SystemExit(f"frozen: the host never said `{wait_for}` in {timeout_s:.0f} s. Is run-frozen.sh running beside this?")
        time.sleep(0.05)


def open_body(driver: Driver, config: str, joints: int):
    """Open and read until a sample with finite positions arrives; returns that sample's positions."""
    rc = driver.open(config)
    if rc != SKR_OK:
        raise SystemExit(f"frozen: the body did not open (rc {rc}). The simulator must be awake before this script starts: FROZEN_WAKE.")
    end = time.monotonic() + 20.0
    while time.monotonic() < end:
        rc, flags, positions, _ = driver.read(joints, with_seq=True)
        if rc == SKR_OK and finite(positions):
            return positions
        time.sleep(ROUND_S)
    raise SystemExit("frozen: the body opened and never read a finite position in 20 s.")


def main() -> int:
    lib = os.environ["FROZEN_LIB"]
    config = os.environ["FROZEN_CONFIG"]
    directory = os.environ["FROZEN_DIR"]
    joints = int(os.environ.get("FROZEN_JOINTS", "6"))
    wake = os.environ.get("FROZEN_WAKE")
    #: `FROZEN_OUTAGE_REPORTED=no` is a plugin measured not to say a frozen controller is frozen (F332); the question is then asked the other way round, so the day a plugin does say it the check fails and the note is removed.
    known_gap = os.environ.get("FROZEN_KNOWN_GAP", "F332") if os.environ.get("FROZEN_OUTAGE_REPORTED", "yes") == "no" else None
    #: `FROZEN_WRITE_BOUNDED=no` is a plugin measured to wait for its controller's reply inside `write()` (AUBO's: an RPC with a one-second timeout whose failure is swallowed, F333): the bound is then asked the other way round, for the write only.
    write_gap = os.environ.get("FROZEN_WRITE_GAP", "F333") if os.environ.get("FROZEN_WRITE_BOUNDED", "yes") == "no" else None
    if wake:
        subprocess.run(wake, shell=True, check=True)

    driver = Driver(lib)
    say(f"frozen controller: {driver.name} ({joints} joints), frozen for {FROZEN_FOR_S:.0f} s")
    pose = open_body(driver, config, joints)

    # --- healthy: a hold at the live pose, 50 Hz, a second of it. -------------------------------------------------------------------------------------------------------------
    worst_read = worst_write = 0.0
    bad = 0
    last_seq = 0
    end = time.monotonic() + 1.0
    rounds = 0
    healthy_poses = set()
    while time.monotonic() < end:
        began = time.monotonic()
        rc, flags, positions, seq = driver.read(joints, with_seq=True)
        worst_read = max(worst_read, time.monotonic() - began)
        if rc != SKR_OK or (flags & SKR_FLAG_FAULT) or not finite(positions):
            bad += 1
        else:
            pose = positions
            healthy_poses.add(tuple(positions))
        began = time.monotonic()
        wrc = driver.write(pose)
        worst_write = max(worst_write, time.monotonic() - began)
        if wrc != SKR_OK:
            bad += 1
        rounds += 1
        time.sleep(ROUND_S)
    say(f"  info  over those {rounds} rounds the body read {len(healthy_poses)} distinct poses (a still arm that never changes a bit between reads cannot be told from a stalled stream by its values)")
    check("healthy", f"{rounds} rounds of a read and a write at the live pose, none refused or faulted (slowest read {worst_read * 1000:.0f} ms, write {worst_write * 1000:.0f} ms)", bad == 0 and rounds >= 20)

    # --- the controller is frozen by the host. -------------------------------------------------------------------------------------------------------------------------------------
    handshake(directory, "ready", "frozen")
    froze_at = time.monotonic()
    reads_late = writes_late = 0
    slowest_read = slowest_write = 0.0
    first_not_ok = first_fault = None
    not_ok = 0
    frozen_poses = set()
    seqs = []
    rounds = 0
    ok_after_report = 0
    while time.monotonic() - froze_at < FROZEN_FOR_S:
        began = time.monotonic()
        rc, flags, positions, seq = driver.read(joints, with_seq=True)
        took = time.monotonic() - began
        slowest_read = max(slowest_read, took)
        if took > READ_BOUND_S:
            reads_late += 1
        frozen_poses.add(tuple(positions))
        seqs.append(seq)
        reported = rc != SKR_OK or bool(flags & (SKR_FLAG_FAULT | SKR_FLAG_DEGRADED))
        if reported:
            not_ok += 1
            if first_not_ok is None:
                first_not_ok = time.monotonic() - froze_at
        elif first_not_ok is not None:
            ok_after_report += 1
        if flags & SKR_FLAG_FAULT and first_fault is None:
            first_fault = time.monotonic() - froze_at
        began = time.monotonic()
        driver.write(pose)
        took = time.monotonic() - began
        slowest_write = max(slowest_write, took)
        if took > WRITE_BOUND_S:
            writes_late += 1
        rounds += 1
        time.sleep(ROUND_S)
    took = f"the slowest read took {slowest_read * 1000:.0f} ms and the slowest write {slowest_write * 1000:.0f} ms over {rounds} rounds"
    if write_gap and writes_late == 0:
        check("bounded", f"KNOWN GAP {write_gap} HAS CLOSED: {took}, and no write waited for the controller longer than {WRITE_BOUND_S:.2f} s. Remove FROZEN_WRITE_BOUNDED=no from this target and the note it feeds", False)
    elif write_gap:
        check("bounded", f"KNOWN GAP {write_gap}: {took}; {writes_late} of {rounds} writes waited for the controller longer than the {WRITE_BOUND_S:.2f} s the contract allows (none longer than {READ_BOUND_S:.2f} s, and no read late); this check fails when a write stops waiting", reads_late == 0 and slowest_write <= READ_BOUND_S)
    else:
        check("bounded", f"{took} (bounds {READ_BOUND_S:.2f} s and {WRITE_BOUND_S:.2f} s)", reads_late == 0 and writes_late == 0)
    say(f"  info  while the controller was frozen the body read {len(frozen_poses)} distinct poses over {rounds} reads, and its sequence went from {seqs[0]} to {seqs[-1]} (it counts calls, not samples)")
    if first_not_ok is None:
        if known_gap:
            check("outage reported", f"KNOWN GAP {known_gap}: no read said the controller was not answering in {FROZEN_FOR_S:.0f} s ({rounds} reads, all OK and unflagged), as the maker's plugin returns its last values with OK; this check fails when a plugin starts to say it", True)
        else:
            check("outage reported", f"a read said the controller was not answering within {FROZEN_FOR_S:.0f} s (never: {rounds} reads, all OK and unflagged)", False)
    else:
        say(f"  info  the first read that was not OK came {first_not_ok:.2f} s after the freeze; {not_ok} of {rounds} reads were not OK" + (f", the fault flag {first_fault:.2f} s after" if first_fault is not None else ", the fault flag never"))
        if known_gap:
            check("outage reported", f"KNOWN GAP {known_gap} HAS CLOSED: a read said the controller was not answering {first_not_ok:.2f} s after the freeze. Remove FROZEN_OUTAGE_REPORTED=no from this target and the `unreached` note it feeds, and put the plugin's figure where the gap was recorded", False)
        else:
            check("outage reported", f"a read said the controller was not answering ({first_not_ok:.2f} s after the freeze, and it stayed said: {ok_after_report} OK reads after the first report)", ok_after_report == 0)

    # --- the stop, with the controller still frozen. -------------------------------------------------------------------------------------------------------------------------------
    began = time.monotonic()
    src = driver.emergency_stop()
    stop_took = time.monotonic() - began
    check("stop", f"emergency_stop returned in {stop_took * 1000:.1f} ms with the controller like this ({STOP_BOUND_S * 1000:.0f} ms wanted)", stop_took <= STOP_BOUND_S)
    wrc = driver.write(pose)
    check("stop", "and the latch holds: the command after the stop is refused", wrc != SKR_OK)
    driver.release_stop()

    # --- the controller is let go; the body is closed and opened again, which is what a plugin that failed asks for (F142). ------------------------------------------------------
    handshake(directory, "measured", "thawed")
    driver.close()
    reopened = None
    end = time.monotonic() + 60.0
    while time.monotonic() < end:
        if wake:
            # A simulator that was stopped for six seconds may have protective-stopped on its way back; waking it again is what a person at the pendant would do, and is a no-op when it is running.
            subprocess.run(wake, shell=True, check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        driver = Driver(lib)
        try:
            reopened = open_body(driver, config, joints)
            break
        except SystemExit as stopped:
            say(f"  info  not yet: {stopped}")
            driver.close()
            time.sleep(1.0)
    ok = reopened is not None
    check("recovers", "after the controller is let go, a body closed and opened again reads a finite pose" if ok else "after the controller is let go the body would not open and read in 60 s", ok)
    if ok:
        rc = driver.write(reopened)
        check("recovers", "and is commanded again", rc == SKR_OK)
        driver.emergency_stop()
        driver.release_stop()
        driver.close()

    handshake(directory, "done", None)
    say(f"\n{checks} checks, {failures} failed")
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())

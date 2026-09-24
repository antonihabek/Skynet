"""skynet-franka against franka-sim: an FCI server nobody in this repository wrote.

`DRY-COVERAGE-PLAN.md` Phase 8, item 8.2, and F120. Every other check on this driver meets
`skynet_franka/sim.py`, which was written by the same hand as the client — F15's *"two copies of one
misunderstanding agree"*. `franka-sim` (PyPI, Apache-2.0, a third party's) serves libfranka's wire
protocol at server version 10 with MuJoCo physics and Franka's FR3 model behind it, and it polices
the FCI's own motion-generator limits and lost command cycles the way a Control box does. So:

* the handshake is a version-10 `Connect` answered by an independent implementation;
* the state layout is read back from somebody else's encoder, and the motion checks close the loop
  through it — a field at the wrong offset would read as a joint that never arrives;
* position, velocity and torque motions switch the FCI's motion generator the way a Control box
  requires (`StopMove`, then a `Move` in the new mode);
* §24's hold and the emergency stop are judged by where the simulated arm actually is.

`run-franka-sim.sh` starts the server, hands this script its log, and counts what the server itself
flagged. What this cannot say: timing on a real Control box, the arm's own reflexes on hardware, or
anything the FR3 model in MuJoCo does not model.
"""

from __future__ import annotations

import argparse
import math
import sys
import time

import skynet_rhal as rhal
from skynet_rhal import Command, State

from skynet_franka.bridge import JOINT_LIMITS, FrankaBridge

CHECKS = 0
FAILURES: list[str] = []

#: The FCI's own period. A command answers the newest state's message id, so every cycle below reads
#: before it writes — which is what `skynetk` does, and what a Control box counts as a live client.
PERIOD_S = 0.001

#: The elbow, and how far it is sent. Joint 4's travel (-3.04..-0.15 rad) excludes zero, so a state
#: decoded at the wrong offset is unlikely to land inside it by accident.
ELBOW = 3
ELBOW_TRAVEL_RAD = 0.3


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}", flush=True)
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}", flush=True)


def section(title: str) -> None:
    print(f"\n{title}", flush=True)


def worst(a: list[float], b: list[float]) -> float:
    return max(abs(x - y) for x, y in zip(a, b))


class Session:
    """The driver behind the RHAL surface, with a cycle that reads before it writes."""

    def __init__(self, bridge: FrankaBridge) -> None:
        self.bridge = bridge
        self.seq = 0
        self.state = State()

    def read(self) -> list[float]:
        self.state = State()
        rc = self.bridge.read_state(self.state, 0)
        if rc != rhal.SKR_OK:
            raise RuntimeError(f"read_state returned {rc}")
        return list(self.state.position_rad)

    def drive(self, seconds: float, mode: int, frame) -> int:
        """One read and one command per FCI period for `seconds`. `frame(t)` returns the values and
        the mask for the elapsed time. The first refusal ends the window and is returned."""
        start = time.monotonic()
        next_tick = start
        while True:
            elapsed = time.monotonic() - start
            if elapsed >= seconds:
                return rhal.SKR_OK
            self.read()
            values, mask = frame(elapsed)
            self.seq += 1
            rc = self.bridge.write_command(Command(seq=self.seq, mode=mode, values=values, mask=mask), 0)
            if rc != rhal.SKR_OK:
                return rc
            next_tick += PERIOD_S
            delay = next_tick - time.monotonic()
            if delay > 0:
                time.sleep(delay)


def quintic(t: float, duration: float) -> float:
    """0 to 1 with zero velocity and acceleration at both ends — the kind of reference the FCI's
    discontinuity limits are written for."""
    tau = min(max(t / duration, 0.0), 1.0)
    return 10 * tau**3 - 15 * tau**4 + 6 * tau**5


def run(host: str, port: int) -> int:
    bridge = FrankaBridge()
    section("the handshake, answered by an implementation nobody here wrote")
    rc = bridge.open(f'{{"host":"{host}","port":{port},"simulator":true}}')
    check(rc == rhal.SKR_OK,
          "open() — a version-10 Connect accepted by franka-sim, and its RobotState stream decoded")
    if rc != rhal.SKR_OK:
        return 1
    session = Session(bridge)

    start = session.read()
    check(len(start) == 7 and all(math.isfinite(q) for q in start), "seven finite joint positions")
    inside = all(low <= q <= high for q, (low, high) in zip(start, JOINT_LIMITS))
    check(inside, "every joint inside its published travel — joint 4's excludes zero, so a state read "
                  f"at the wrong offset would not land there by accident (elbow {start[ELBOW]:.3f} rad)")
    check(session.state.flags & rhal.FLAG_FAULT == 0, "no fault latched at the start")

    section("a joint-position motion, through the FCI's motion generator")
    target = list(start)
    target[ELBOW] += ELBOW_TRAVEL_RAD
    duration = 3.0
    rc = session.drive(duration + 0.5, rhal.MODE_POSITION, lambda t: (
        [a + (b - a) * quintic(t, duration) for a, b in zip(start, target)], [1] * 7))
    check(rc == rhal.SKR_OK, "every command of the motion was accepted")
    reached = session.read()
    check(abs(reached[ELBOW] - target[ELBOW]) < 0.01,
          f"the elbow went where it was sent: {reached[ELBOW]:.4f} rad against {target[ELBOW]:.4f}")
    others = [i for i in range(7) if i != ELBOW]
    check(max(abs(reached[i] - start[i]) for i in others) < 0.01,
          "and nothing else moved: the command landed on joint 4 and only joint 4")

    section("§24's hold: the all-zero mask the kernel writes when it has nothing to forward")
    before = session.read()
    rc = session.drive(1.5, rhal.MODE_POSITION, lambda t: ([0.0] * 7, [0] * 7))
    check(rc == rhal.SKR_OK, "a second and a half of holds is accepted")
    drift = worst(session.read(), before)
    check(drift < 0.005, f"and the arm did not move (worst {drift:.5f} rad)")

    section("a joint-velocity motion: StopMove, then a Move in the new generator")
    before = session.read()
    duration = 2.0
    peak = 0.2
    rc = session.drive(duration + 0.3, rhal.MODE_VELOCITY, lambda t: (
        [(-peak * math.sin(math.pi * t / duration) if t < duration and i == ELBOW else 0.0)
         for i in range(7)], [1] * 7))
    check(rc == rhal.SKR_OK, "the mode switch and every velocity command were accepted")
    moved = session.read()[ELBOW] - before[ELBOW]
    expected = -peak * 2 * duration / math.pi
    check(abs(moved - expected) < 0.05,
          f"the elbow travelled what the velocity integrates to: {moved:.4f} rad against {expected:.4f}")

    section("torque: the external controller, commanded zero")
    before = session.read()
    rc = session.drive(1.0, rhal.MODE_TORQUE, lambda t: ([0.0] * 7, [1] * 7))
    check(rc == rhal.SKR_OK, "the switch to the external controller and every torque were accepted")
    drift = worst(session.read(), before)
    # The FCI adds gravity compensation under a torque command, so zero is 'float', not 'fall'. A
    # body that fell here would be a server that did not model that — or a driver writing the
    # torque into the wrong field.
    check(drift < 0.05, f"zero torque floats the arm rather than dropping it (worst {drift:.4f} rad)")

    section("the emergency stop: StopMove, and the Control box's idle hold")
    session.read()
    before = session.read()
    check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    time.sleep(1.0)
    after = session.read()
    check(session.state.flags & rhal.FLAG_ESTOP != 0, "the stop is visible in the next state")
    check(worst(after, before) < 0.005,
          f"and the arm is held where it stopped (worst {worst(after, before):.5f} rad)")
    session.seq += 1
    rc = bridge.write_command(Command(seq=session.seq, mode=rhal.MODE_POSITION, values=before,
                                      mask=[1] * 7), 0)
    check(rc == rhal.SKR_EPERM, "and the latch refuses every command after it")
    bridge.close()
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=1337)
    arguments = parser.parse_args()
    result = run(arguments.host, arguments.port)
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed", flush=True)
    for failure in FAILURES:
        print(f"  - {failure}", flush=True)
    if result != 0:
        return result
    return 0 if not FAILURES else 1


if __name__ == "__main__":
    sys.exit(main())

"""drivers/booster, told `model: k1`, against NUbots' MuJoCo simulator of the Booster K1 (DRY-COVERAGE-PLAN.md item 12.D, the `booster` row).

    NUSIM_LIB=/path/libskynet_booster.so python3 nusim_live.py [interface]        (the simulator is already running: `run-nusim.sh` starts it, puts it in CUSTOM mode and calls this)

NUSim is a third party's implementation of the robot's side of the wire this driver speaks: it publishes `rt/low_state` and takes `rt/joint_ctrl` with Booster's SDK, over FastDDS, and tracks a
command in CUSTOM mode with a PD law at 1 kHz against the maker's own K1 model (`BoosterRobotics/booster_assets`, `robots/K1/K1_22dof.xml`). It was written by people who wanted a K1 to walk in,
and it knows nothing of this driver, which is what makes it evidence the fake robot beside the driver is not (F15). What it can show, and what this holds the driver to:

  * the layout: a K1 session opens on a body that publishes 22 serial joints, and reads one number per joint that is that joint's;
  * the **index of every joint**: a position command that moves joint i alone is answered by joint i of the K1 moving, and by no other joint moving much more. The body stands, so a leg joint moved is answered by the other joints too (a knee bent
    is a hip and an ankle moved, and the other leg takes the weight); each joint is therefore measured against a control, the same body held for the same time with nothing moved, and what the move did is the
    difference between the two. The driver's slot i is the SDK's `JointIndexK1` i, the descriptor's name for it is the model's, and the simulator maps the same index to the model's actuator by name, so a table
    slipped in the driver (a joint moved to a neighbour's slot) moves the wrong joint here;
  * the sign and the unit: a positive delta in radians is a positive movement of the joint in radians;
  * the stop: a stop publishes zero gains and zero torques, which under the simulator's law (`tau = kp(q* - q) + kd(dq* - dq) + tau*`) is a body that is commanded to do nothing.

What it cannot show: a K1. The simulator's servo is a PD law on a rigid model with no motor, no backlash, no communication delay and no safety; the gains here are scalar (the driver takes one `kp` and one `kd` for
every joint) and were chosen so that the body stays up, not because they are a K1's. No Booster document is read: the K1 has no manual here.
"""

from __future__ import annotations

import json
import math
import os
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, "..", "..", ".."))
sys.path.insert(0, os.path.join(ROOT, "tools", "faultproxy"))

from rhal_ctypes import Driver  # noqa: E402

JOINTS = 22
#: JointIndexK1's names, from the SDK header (`b1_api_const.hpp`), in order. Written here and not read from the driver.
NAMES = ["head_yaw", "head_pitch", "L_shoulder_pitch", "L_shoulder_roll", "L_elbow_pitch", "L_elbow_yaw", "R_shoulder_pitch", "R_shoulder_roll", "R_elbow_pitch", "R_elbow_yaw",
         "L_hip_pitch", "L_hip_roll", "L_hip_yaw", "L_knee", "L_ankle_pitch", "L_ankle_roll", "R_hip_pitch", "R_hip_roll", "R_hip_yaw", "R_knee", "R_ankle_pitch", "R_ankle_roll"]
#: The maker's K1 model's `range` of each joint, radians, in the same order.
LOW = [-1.0, -0.349, -3.316, -1.74, -2.27, -2.44, -3.316, -1.57, -2.27, 0.0, -3.0, -0.4, -1.0, 0.0, -0.87, -0.345, -3.0, -1.57, -1.0, 0.0, -0.87, -0.345]
HIGH = [1.0, 0.855, 1.22, 1.57, 2.27, 0.0, 1.22, 1.74, 2.27, 2.44, 2.21, 1.57, 1.0, 2.23, 0.345, 0.345, 2.21, 0.4, 1.0, 2.23, 0.345, 0.345]

KP, KD = 150.0, 3.0
STEP_RAD = 0.10
SETTLE_S = 0.6
EARLY_S = 0.15
RATE_HZ = 100.0
#: A body is still when no joint moves faster than this (rad/s) over a tenth of a second, and a body that has not stilled in this long is taken as it is.
STILL_RAD_S = 0.03
STILL_GIVE_UP_S = 3.0
#: What the joint that was asked to move has to have moved, against the most any other joint did (see the loop).
LEAD_MARGIN = 0.6

CHECKS = 0
FAILURES: list[str] = []


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    print(f"  {'ok  ' if held else 'FAIL'}  {what}", flush=True)
    if not held:
        FAILURES.append(what)
        if detail:
            print(f"        {detail}", flush=True)


def section(title: str) -> None:
    print(f"\n{title}", flush=True)


def stream(driver: Driver, target: list[float], seconds: float) -> None:
    """The command, repeated at the control rate for `seconds`: a servo that is told once does not hold."""
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        driver.write(target)
        time.sleep(1.0 / RATE_HZ)


def pose(driver: Driver) -> list[float]:
    rc, _flags, positions = driver.read(JOINTS)
    assert rc == 0, f"read_state answered {rc}"
    return list(positions)


def settle(driver: Driver, target: list[float]) -> list[float]:
    """The command held until the body has stopped moving (or `STILL_GIVE_UP_S` has gone), and where it stands then: a K1 is a standing body, and a step of one leg joint leaves the rest of it moving for a while."""
    end = time.monotonic() + STILL_GIVE_UP_S
    before = pose(driver)
    while time.monotonic() < end:
        stream(driver, target, 0.1)
        now = pose(driver)
        if max(abs(now[i] - before[i]) for i in range(JOINTS)) / 0.1 < STILL_RAD_S:
            return now
        before = now
    return pose(driver)


def trial(driver: Driver, hold: list[float], target: list[float]) -> tuple[list[float], list[float], list[float]]:
    """From a body that has stilled under `hold`, `target` for `SETTLE_S`: where it stood, where it was after `EARLY_S`, and where it was at the end."""
    start = settle(driver, hold)
    stream(driver, target, EARLY_S)
    early = pose(driver)
    stream(driver, target, SETTLE_S - EARLY_S)
    return start, early, pose(driver)


def main() -> int:
    interface = sys.argv[1] if len(sys.argv) > 1 else "lo"
    lib = os.environ.get("NUSIM_LIB", os.path.join(HERE, "..", "libskynet_booster.so"))
    driver = Driver(lib)

    section("a K1 session against NUbots' simulator")
    config = json.dumps({"interface": interface, "model": "k1", "kp": KP, "kd": KD, "simulator": True})
    rc = driver.open(config)
    check(rc == 0, "open() joins DDS and reads the simulator's first `rt/low_state`: 22 serial joints, and no refusal", f"open answered {rc}")
    if rc != 0:
        print("\n  the simulator is not publishing; nothing below can run.")
        return 1
    rc, flags, positions, seq = driver.read(JOINTS, with_seq=True)
    check(rc == 0 and len(positions) == JOINTS, "read_state answers with 22 joints")
    first_seq = seq
    time.sleep(0.2)
    rc, flags, positions, seq = driver.read(JOINTS, with_seq=True)
    check(seq > first_seq, f"and the sequence is the simulator's samples, which advance ({first_seq} to {seq})")
    check(all(math.isfinite(p) for p in positions), "every position is a number")
    inside = [i for i in range(JOINTS) if LOW[i] - 0.05 <= positions[i] <= HIGH[i] + 0.05]
    check(len(inside) == JOINTS, f"and every joint reads inside the maker's model's range for it ({len(inside)} of 22)", str([NAMES[i] for i in range(JOINTS) if i not in inside]))

    section("the body stays up under the driver's command")
    ready = pose(driver)
    stream(driver, ready, 1.5)
    held = pose(driver)
    drift = [abs(held[i] - ready[i]) for i in range(JOINTS)]
    worst = max(range(JOINTS), key=lambda i: drift[i])
    check(max(drift) < 0.12, f"commanded to where it stands, the K1 stays there (worst drift {drift[worst]:.3f} rad at {NAMES[worst]})", str([round(d, 3) for d in drift]))

    section("every joint answers to its own index")
    # A K1 stands, and a standing body answers a move of one joint with moves of others (a knee bent is a hip and an ankle moved, and the other leg takes the weight). So each joint is measured against itself:
    # the body is stilled, held for the same time with no change (the control), stilled again, and held with one joint moved; what the move did is the difference between the two runs, joint by joint.
    base = pose(driver)
    wrong: list[str] = []
    for i in range(JOINTS):
        delta = STEP_RAD if base[i] + STEP_RAD <= HIGH[i] - 0.02 else -STEP_RAD
        target = list(base)
        target[i] = base[i] + delta
        start0, early0, late0 = trial(driver, base, base)
        start1, early1, late1 = trial(driver, base, target)
        effect_early = [(early1[j] - start1[j]) - (early0[j] - start0[j]) for j in range(JOINTS)]
        effect_late = [(late1[j] - start1[j]) - (late0[j] - start0[j]) for j in range(JOINTS)]
        rival = max((j for j in range(JOINTS) if j != i), key=lambda j: abs(effect_early[j]))
        moved = effect_late[i] / delta
        # Its own joint moves the right way, and no other joint moves much more than it does. "Much more" is a margin (a loaded hip is answered more slowly than the knee under it, which the weight of the body
        # pulls along at once), and a table slipped by one slot is nowhere near it: the joint that was meant gets nothing but its neighbour's reaction, and the neighbour gets the whole step.
        ok = effect_early[i] * delta > 0 and abs(effect_early[i]) >= LEAD_MARGIN * abs(effect_early[rival]) and moved > 0.2
        check(ok, f"{NAMES[i]} ({i}): asked {delta:+.2f} rad; it moved {effect_early[i]:+.3f} after {EARLY_S * 1000:.0f} ms and {moved * 100:.0f}% of the step after {SETTLE_S * 1000:.0f} ms, against the control; the most any other joint moved was {NAMES[rival]}, {effect_early[rival]:+.3f}",
              f"early {effect_early[i]:+.3f}, late {effect_late[i]:+.3f}")
        if not ok:
            wrong.append(NAMES[i])
        base = settle(driver, base)
    check(not wrong, "all twenty-two answered to their own index", str(wrong))

    section("the stop")
    rc = driver.emergency_stop()
    check(rc == 0, "emergency_stop returns OK")
    time.sleep(0.1)
    rc3 = driver.write(base)
    check(rc3 != 0, "a command after the stop is refused")
    before = pose(driver)
    time.sleep(1.2)
    after = pose(driver)
    sagged = max(abs(after[i] - before[i]) for i in range(JOINTS))
    check(sagged > 0.15, f"and a body commanded to do nothing does not hold itself up: its joints moved by up to {sagged:.2f} rad in 1.2 s")
    driver.close()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAILED: {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

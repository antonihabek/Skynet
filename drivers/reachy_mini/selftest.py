"""skynet-reachy-mini's own suite: the daemon's shapes, and every refusal the bridge makes.

No test framework, `drivers/xarm/selftest.py`'s shape and emphasis: nearly everything asserted here is a **refusal**, each written beside the nearly identical input that succeeds. A desk robot's failure
modes are a target the daemon clamps without a word, a command it ignores because somebody else's move is running, and a stop that is a line in a log.

    make selftest      # this file, against the simulated daemon
    make check         # and then the whole path through the real ABI

The routes and models are held to the maker's own source by `layout_check.py`, and the live block runs the driver against the maker's own daemon in simulation; this file holds the driver to its own
behaviour, against a daemon written here.
"""

from __future__ import annotations

import contextlib
import io
import json
import math
import sys
import time

import skynet_rhal as rhal

from skynet_reachy_mini import frames
from skynet_reachy_mini.bridge import ReachyMiniBridge
from skynet_reachy_mini.daemon_sim import RANGES, SPEEDS, SimulatedDaemon

FAILURES: list[str] = []
CHECKS = 0
NAMES = [name for name, _ in frames.JOINTS]


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")
        if detail:
            print(f"        what the driver said: {detail!r}")


def section(title: str) -> None:
    print(f"\n{title}")


def until(predicate, timeout: float = 2.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return True
        time.sleep(0.005)
    return bool(predicate())


def settings(daemon: SimulatedDaemon, **extra) -> dict:
    out = {
        "host": "127.0.0.1", "port": daemon.port, "stop_mode": "hold",
        "limits": {name: list(RANGES[name]) for name in ("head_x", "head_y", "head_z", "antenna_right", "antenna_left")},
        "speeds": dict(SPEEDS), "rate_hz": 20,
        "roles": {"head_pitch": "head.pitch", "body_yaw": "torso.yaw"},
    }
    out.update(extra)
    return {k: v for k, v in out.items() if v is not None}


def config(daemon: SimulatedDaemon, **extra) -> str:
    return json.dumps(settings(daemon, **extra))


def cmd(values: list[float], mask: list[int] | None = None, mode: int = rhal.MODE_POSITION) -> rhal.Command:
    return rhal.Command(stamp_ns=1, seq=1, mode=mode, values=values, mask=mask or [1] * len(values))


def opened(daemon: SimulatedDaemon, **extra) -> ReachyMiniBridge:
    bridge = ReachyMiniBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(config(daemon, **extra))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge


def refused(text: str) -> tuple[int, str]:
    bridge = ReachyMiniBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(text)
    bridge.close()
    return code, said.getvalue()


def state(bridge: ReachyMiniBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def write(bridge: ReachyMiniBridge, command: rhal.Command) -> tuple[int, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(command, 0)
    return code, said.getvalue()


def at(**given: float) -> list[float]:
    return [given.get(name, 0.0) for name in NAMES]


def main() -> int:
    print("skynet-reachy-mini selftest\n")

    # ── the daemon's shapes ──────────────────────────────────────────────────
    section("frames — the daemon's models")
    body = frames.target_body([0.01, 0.02, 0.03, 0.1, 0.2, 0.3, 0.4, 0.5, 0.6])
    check(body == {"target_head_pose": {"x": 0.01, "y": 0.02, "z": 0.03, "roll": 0.1, "pitch": 0.2, "yaw": 0.3}, "target_antennas": [0.5, 0.6], "target_body_yaw": 0.4},
          "a target is the head pose as x, y, z, roll, pitch, yaw, the antennas as right then left, and the body yaw")
    check([name for name, _ in frames.JOINTS] == ["head_x", "head_y", "head_z", "head_roll", "head_pitch", "head_yaw", "body_yaw", "antenna_right", "antenna_left"], "the nine joints are in the order a command carries them")
    check([kind for _, kind in frames.JOINTS][:3] == ["prismatic"] * 3 and set(kind for _, kind in frames.JOINTS[3:]) == {"revolute"}, "the head's translation is prismatic and everything else revolute")
    for what, values in (("eight values", [0.0] * 8), ("ten values", [0.0] * 10), ("a NaN", [math.nan] + [0.0] * 8), ("an infinity", [0.0] * 8 + [math.inf])):
        try:
            frames.target_body(values)
            check(False, f"a target of {what} is refused")
        except frames.ProtocolError:
            check(True, f"a target of {what} is refused")
    good = {"head_pose": {"x": 0.0, "y": 0.0, "z": 0.0, "roll": 0.1, "pitch": 0.0, "yaw": 0.0}, "body_yaw": 0.2, "antennas_position": [0.3, 0.4], "control_mode": "enabled"}
    parsed = frames.parse_state(good)
    check(parsed.positions == (0.0, 0.0, 0.0, 0.1, 0.0, 0.0, 0.2, 0.3, 0.4) and parsed.control_mode == "enabled", "a state is read in the same order, antennas right then left")
    for what, broken in (
        ("a pose as a matrix", {**good, "head_pose": {"m": [0.0] * 16}}), ("no pose", {k: v for k, v in good.items() if k != "head_pose"}), ("no body yaw", {k: v for k, v in good.items() if k != "body_yaw"}),
        ("one antenna", {**good, "antennas_position": [0.3]}), ("a pose that is not numbers", {**good, "head_pose": {**good["head_pose"], "x": "a"}}), ("a NaN", {**good, "body_yaw": float("nan")}),
        ("a list", [1]),
    ):
        try:
            frames.parse_state(broken)
            check(False, f"a state with {what} is refused")
        except frames.ProtocolError:
            check(True, f"a state with {what} is refused")
    # Every documented range, both ends: the check this replaces read the pitch's upper end, the yaw's and the body's, said "pitch and roll", and a roll widened to 90 degrees survived the seeding campaign
    # (DRY-COVERAGE-PLAN.md, forty-eighth session).
    documented = {"head_pitch": 40.0, "head_roll": 40.0, "head_yaw": 180.0, "body_yaw": 160.0}
    check(set(frames.DOCUMENTED_LIMITS) == set(documented)
          and all(abs(frames.DOCUMENTED_LIMITS[name][0] + math.radians(degrees)) < 1e-12 and abs(frames.DOCUMENTED_LIMITS[name][1] - math.radians(degrees)) < 1e-12 for name, degrees in documented.items())
          and abs(frames.YAW_DELTA_MAX_RAD - math.radians(65)) < 1e-12,
          "the documented ranges are the maker's, both ends of each: pitch and roll 40 degrees, head yaw 180, body yaw 160, and 65 between the two yaws, and no other joint has one")
    # A pose that arrives as a matrix is what `use_pose_matrix=true` asks for, and the refusal says so: without the type check the same input is still refused, by a TypeError's words about list indices.
    for what, pose in (("a 4x4 matrix as nested lists", [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]]), ("sixteen numbers", [0.0] * 16)):
        try:
            frames.parse_state({**good, "head_pose": pose})
            check(False, f"a pose that is {what} is refused")
        except frames.ProtocolError as error:
            check("matrix" in str(error) and "x, y, z, roll, pitch, yaw" in str(error), f"a pose that is {what} is refused, saying a matrix is what `use_pose_matrix=true` asks for and this driver does not", str(error))

    # ── open ─────────────────────────────────────────────────────────────────
    section("open — what the configuration must say, and what the daemon must say")
    with SimulatedDaemon() as daemon:
        good_settings = settings(daemon)
        check(refused(json.dumps(good_settings))[0] == rhal.SKR_OK, "the configuration that is complete opens")
        for what, mutate, words in (
            ("a configuration with no host", lambda c: c.pop("host"), "host"),
            ("a port that is not one", lambda c: c.update(port=0), "port"),
            ("no stop mode", lambda c: c.pop("stop_mode"), "stop_mode"),
            ("a stop mode that is neither", lambda c: c.update(stop_mode="freeze"), "stop_mode"),
            ("no range for the head's x", lambda c: c["limits"].pop("head_x"), "head_x"),
            ("no range for the right antenna", lambda c: c["limits"].pop("antenna_right"), "antenna_right"),
            ("a range that is not a range", lambda c: c["limits"].update(head_x=[0.01, 0.01]), "not a range"),
            ("a range of one number", lambda c: c["limits"].update(head_x=[0.01]), "head_x"),
            ("a documented range widened", lambda c: c["limits"].update(head_pitch=[-1.0, 1.0]), "wider"),
            ("a limit for a joint that is not one", lambda c: c["limits"].update(head_w=[0, 1]), "limits"),
            ("no speeds", lambda c: c.pop("speeds"), "speeds"),
            ("a speed missing", lambda c: c["speeds"].pop("antenna_left"), "antenna_left"),
            ("a speed of zero", lambda c: c["speeds"].update(head_z=0), "head_z"),
            ("a speed for a joint that is not one", lambda c: c["speeds"].update(head_w=1.0), "speeds"),
            ("a role for a joint that is not one", lambda c: c["roles"].update(head_w="x"), "roles"),
            ("a simulator flag that is not a boolean", lambda c: c.update(simulator="yes"), "simulator"),
            ("a key the driver does not read", lambda c: c.update(speed=1.0), "speed"),
            ("a rate of nothing", lambda c: c.update(rate_hz=0), "rate_hz"),
        ):
            changed = json.loads(json.dumps(good_settings))
            mutate(changed)
            code, said = refused(json.dumps(changed))
            check(code == rhal.SKR_ENODEV and words in said, f"{what} is refused, and the refusal says so")
        narrowed = json.loads(json.dumps(good_settings))
        narrowed["limits"]["head_pitch"] = [-0.2, 0.2]
        check(refused(json.dumps(narrowed))[0] == rhal.SKR_OK, "a documented range may be narrowed")
        check(refused("{not json")[0] == rhal.SKR_EINVAL and refused("[1]")[0] == rhal.SKR_EINVAL, "a configuration that is not a JSON object is EINVAL")
        code, said = refused(json.dumps({**good_settings, "port": 1}))
        check(code == rhal.SKR_EIO and "did not answer" in said, "a port nobody answers on is a transport failure (EIO), not a refusal of the body")

    for what, act, words in (
        ("a daemon that is stopping", lambda d: setattr(d, "state", frames.DaemonState.STOPPING), "stopping"),
        ("a daemon holding an error", lambda d: setattr(d, "error", "motor 3 overheated"), "motor 3 overheated"),
        ("motors that are limp", lambda d: d.set_mode(frames.MotorMode.DISABLED), "disabled"),
        ("motors in gravity compensation", lambda d: d.set_mode(frames.MotorMode.GRAVITY_COMPENSATION), "gravity_compensation"),
        ("a move somebody else started", lambda d: setattr(d, "external_move", "7d1c"), "move"),
        # The maker's own daemon starts asleep (`--no-wake-up-on-start`, or after `goto_sleep`): the head 5 cm down and the antennas folded at 3.05 rad. Every command to a robot outside the range it was told to stay in is
        # refused, and nothing may move it in, because a step is bounded by the speed and a target by the range. So it is refused at open, with the joint, where it is, the range, and what to do.
        ("a robot outside the range it was told to stay in", lambda d: d.place(at(head_z=RANGES["head_z"][0] - 0.05)), "head_z"),
        ("a robot whose antennas are folded past their range", lambda d: d.place(at(antenna_right=-3.5, antenna_left=3.5)), "antenna_right"),
        ("a robot above the range it was told to stay in", lambda d: d.place(at(head_z=RANGES["head_z"][1] + 0.05)), "head_z"),
        ("a robot whose left antenna alone is past its range", lambda d: d.place(at(antenna_left=3.5)), "antenna_left"),
    ):
        with SimulatedDaemon() as daemon:
            act(daemon)
            code, said = refused(config(daemon))
            check(code == rhal.SKR_ENODEV and words in said, f"{what} is refused at open, and the refusal says why")
    with SimulatedDaemon() as daemon:
        daemon.place(at(head_z=RANGES["head_z"][0] - 0.05))
        code, said = refused(config(daemon))
        check(f"{RANGES['head_z'][0] - 0.05:.4f}" in said and f"{RANGES['head_z'][0]:.4f}" in said and "wake_up" in said and "limits" in said,
              "and it says where the joint is, the range it was given and what to do: wake the robot with the maker's own route (the operator's act, not this driver's) or give the range it truly has", said)
    with SimulatedDaemon() as daemon:
        # Sensor jitter at the edge of a range is not a robot outside it: the allowance is one step of that joint's own speed, the amount the driver tolerates anywhere else.
        daemon.place(at(head_z=RANGES["head_z"][1] + SPEEDS["head_z"] * 0.05 * 0.5))
        code, said = refused(config(daemon))
        check(code == rhal.SKR_OK, "a robot a half step past a range's edge is not refused for it", said)
    with SimulatedDaemon() as daemon:
        daemon.simulation_enabled = None
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            bridge = ReachyMiniBridge()
            code = bridge.open(config(daemon))
        check(code == rhal.SKR_OK and "does not say whether it is a simulation" in said.getvalue() and json.loads(bridge.describe())["simulator"] is False,
              "a daemon that does not say whether it is a simulation, and a configuration that does not either, describe hardware, and say so")
        bridge.close()

    # ── the descriptor ───────────────────────────────────────────────────────
    section("the descriptor — the daemon's word on what it is, the configuration's on everything else")
    with SimulatedDaemon() as daemon:
        bridge = opened(daemon, rate_hz=25)
        descriptor = json.loads(bridge.describe())
        check(descriptor["robot_slug"] == "reachy-mini" and descriptor["dof"] == 9 and [j["name"] for j in descriptor["joints"]] == NAMES and descriptor["base"] == "fixed", "nine joints, in order, on a fixed base")
        by_name = {j["name"]: j for j in descriptor["joints"]}
        check(by_name["head_x"]["type"] == "prismatic" and by_name["head_roll"]["type"] == "revolute", "the head's translation is prismatic and its rotation revolute")
        check(by_name["head_pitch"]["limits"]["position_max_rad"] == frames.DOCUMENTED_LIMITS["head_pitch"][1] and by_name["head_x"]["limits"]["position_max_rad"] == RANGES["head_x"][1]
              and by_name["head_z"]["limits"]["velocity_max_rad_s"] == SPEEDS["head_z"], "the documented ranges and the configured ones are the descriptor's, with the configured speeds")
        check(by_name["head_pitch"]["role"] == "head.pitch" and by_name["head_roll"]["role"] is None, "a role is what the configuration said, and a joint it said nothing of has none")
        check(descriptor["simulator"] is True and descriptor["envelope"] is None and descriptor["control"]["rate_hz_max"] == 25.0 and "safety.estop" in descriptor["capabilities"],
              "the simulator flag is the daemon's word, no envelope is invented, the rate is the one asked for, and the stop it has is declared")
        bridge.close()
    with SimulatedDaemon() as daemon:
        daemon.simulation_enabled = False
        bridge = opened(daemon)
        check(json.loads(bridge.describe())["simulator"] is False, "a daemon that says it is not a simulation is believed")
        bridge.close()
        bridge = opened(daemon, simulator=True)
        check(json.loads(bridge.describe())["simulator"] is True, "and a configuration can only make it more cautious")
        bridge.close()
    with SimulatedDaemon() as daemon:
        # The other direction, which the seeding campaign found nobody had asked: a daemon that says it is a simulation, and a configuration that says it is not. Nothing forces `simulator` false.
        bridge = opened(daemon, simulator=False)
        check(json.loads(bridge.describe())["simulator"] is True, "a configuration that says `simulator: false` does not make a daemon that says it is a simulation into hardware")
        bridge.close()
    with SimulatedDaemon() as daemon:
        daemon.simulation_enabled, daemon.mockup_sim_enabled = False, True
        bridge = opened(daemon)
        check(json.loads(bridge.describe())["simulator"] is True, "a daemon in mock-up mode, which has no physics at all, is described as a simulation and never as hardware")
        bridge.close()
    check(ReachyMiniBridge().describe() == "{}", "a bridge that was never opened describes nothing")

    # ── state ────────────────────────────────────────────────────────────────
    section("read_state — what the daemon says, stamped when it arrived")
    with SimulatedDaemon() as daemon:
        daemon.place(at(head_x=0.01, head_pitch=0.2, body_yaw=-0.3, antenna_right=0.5, antenna_left=-0.5))
        bridge = opened(daemon)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        code, first = state(bridge)
        check(code == rhal.SKR_OK and list(first.position_rad) == at(head_x=0.01, head_pitch=0.2, body_yaw=-0.3, antenna_right=0.5, antenna_left=-0.5),
              "the positions are the daemon's own, nine of them, the antennas right then left")
        check(all(math.isnan(v) for v in first.velocity_rad_s) and all(math.isnan(e) for e in first.effort_nm), "no velocity or effort is invented: the daemon reports present positions only (NaN, §24)")
        check(first.flags == rhal.HOMING_NOT_REQUIRED and first.fault_code == 0, "a healthy robot reads clean")
        time.sleep(0.1)
        code, second = state(bridge)
        check(second.seq == first.seq + 1 and second.stamp_ns >= first.stamp_ns, "each read has the next sequence number and a stamp that does not go back")
        daemon.error = "motor 3 overheated"
        until(lambda: state(bridge)[1].fault_code == 1, 3.0)
        code, faulted = state(bridge)
        check(faulted.fault_code == 1 and faulted.flags & rhal.FLAG_FAULT and faulted.flags & rhal.FLAG_DEGRADED, "an error the daemon holds is FAULT and DEGRADED, with a fault word that says there is one")
        daemon.error = None
        until(lambda: state(bridge)[1].fault_code == 0, 3.0)
        check(state(bridge)[1].fault_code == 0 and not state(bridge)[1].flags & rhal.FLAG_FAULT, "and it is not sticky: the driver reads what the daemon says now")
        daemon.state = frames.DaemonState.STOPPING
        until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0)
        check(bool(state(bridge)[1].flags & rhal.FLAG_DEGRADED), "a daemon that is not running is DEGRADED")
        daemon.state = frames.DaemonState.RUNNING
        # Let the last cause clear first: the status is read every half second, so the flag the stopping daemon raised is still up when it is set again, and this check passed on it whatever the driver did about motors
        # (the seeding campaign's survivor `reachy-limp-not-degraded`).
        check(until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0), "a daemon that is running again is not DEGRADED")
        daemon.set_mode(frames.MotorMode.DISABLED)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0)
        check(bool(state(bridge)[1].flags & rhal.FLAG_DEGRADED), "and so are motors that are not enabled")
        daemon.set_mode(frames.MotorMode.ENABLED)
        until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0)
        daemon.drop = True
        time.sleep(0.9)
        code, stale = state(bridge)
        check(code == rhal.SKR_ETIMEDOUT and stale.flags & rhal.FLAG_DEGRADED, "a daemon that has stopped answering is ETIMEDOUT and DEGRADED: silence is not stillness")
        check(0 < stale.stamp_ns < rhal.acquisition_ns() - 400_000_000, "and the stamp is still the sample's own, not the read's: it is the age of the last thing the daemon said")
        daemon.drop = False
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 3.0), "and it recovers when the daemon does")
        until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0)
        daemon.reset_connections()
        seen = []
        deadline = time.monotonic() + 0.45
        while time.monotonic() < deadline:
            seen.append(state(bridge))
            time.sleep(0.02)
        check(any(s.flags & rhal.FLAG_DEGRADED for _, s in seen) and all(c == rhal.SKR_OK for c, _ in seen), "a connection the daemon dropped is said as DEGRADED for a moment, though the next request went through: a peer that crashed and came back was a link that was down")
        check(until(lambda: not state(bridge)[1].flags & rhal.FLAG_DEGRADED, 2.0), "and it is not sticky: the driver reads healthy again once the staleness bound has passed")
        bridge.close()
        check(state(bridge)[0] == rhal.SKR_EPERM, "a closed bridge reads EPERM")
    with SimulatedDaemon() as daemon:
        # A daemon that answers, slowly. No request fails, so nothing is reported dead, and what has been read is older than the staleness bound: silence is not stillness here either. The check above, that a
        # daemon that has gone is ETIMEDOUT, passes on the failures alone (the seeding campaign's survivor `reachy-stale-never`).
        bridge = opened(daemon, stale_ms=200, http_timeout_ms=3000)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        daemon.delay_s = 0.6
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and bool(state(bridge)[1].flags & rhal.FLAG_DEGRADED), 4.0),
              "a daemon that answers every request but takes 0.6 s to, past a staleness bound of 0.2 s, reads ETIMEDOUT and DEGRADED: what was last read is too old to be believed")
        daemon.delay_s = 0.0
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 4.0), "and it reads clean again when the daemon is quick")
        bridge.close()

    # ── commands ─────────────────────────────────────────────────────────────
    section("write_command — the target that goes out, and what is refused before it does")
    with SimulatedDaemon() as daemon:
        bridge = opened(daemon)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        step = SPEEDS["head_pitch"] * 0.05
        before = len(daemon.targets)
        check(write(bridge, cmd(at(head_pitch=step * 0.9)))[0] == rhal.SKR_OK, "a command inside every range and speed is accepted")
        check(len(daemon.targets) == before + 1, "and it is one request to the daemon")
        sent = daemon.targets[-1]
        check(set(sent) == {"target_head_pose", "target_antennas", "target_body_yaw"} and abs(sent["target_head_pose"]["pitch"] - step * 0.9) < 1e-12
              and sent["target_antennas"] == [0.0, 0.0] and sent["target_body_yaw"] == 0.0, "carrying the whole body: the head pose, the antennas and the body yaw")
        check(until(lambda: abs(daemon.present()[4] - step * 0.9) < 1e-3), "which the daemon's robot follows")
        # A stream: each step is measured from the last setpoint this session issued, so five of them in a row are accepted, and not five steps measured from where the session began (the seeding campaign's survivor
        # `reachy-held-not-kept`: every other test sets the last setpoint by hand).
        bridge._held = [0.0] * 9
        codes = [write(bridge, cmd(at(head_pitch=step * 0.9 * n)))[0] for n in range(1, 6)]
        check(codes == [rhal.SKR_OK] * 5, "and five steps in a row, each a step from the last, are all accepted: the speed ceiling is measured from the previous setpoint, not from where the session began", str(codes))
        bridge._held = [0.0] * 9
        check(write(bridge, cmd(at(head_pitch=step * 0.9 * 2)))[0] == rhal.SKR_EINVAL, "while two steps at once is refused")
        bridge._held = [0.0] * 9
        count = len(daemon.targets)
        for index, name in enumerate(NAMES):
            low, high = bridge._limits[name]
            span = SPEEDS[name] * 0.05
            only = [1 if i == index else 0 for i in range(9)]
            # One joint at a time, by mask, a quarter of a step inside its range, and the step beyond it small enough that the speed is never what refuses: the range is, and only the range.
            # The two yaws are held beside each other so that the 65 degrees between them is not what refuses either.
            near_high, near_low = [0.0] * 9, [0.0] * 9
            for held, edge in ((near_high, "high"), (near_low, "low")):
                value = (min(high, 2.7) - span / 4) if edge == "high" and index in (5, 6) else (max(low, -2.7) + span / 4) if index in (5, 6) else (high - span / 4 if edge == "high" else low + span / 4)
                held[index] = value
                if index in (5, 6):
                    held[5] = held[6] = value
            for held, beyond, word in ((near_high, high + 0.001, "above"), (near_low, low - 0.001, "below")):
                bridge._held = list(held)
                values = list(held)
                values[index] = beyond
                check(write(bridge, cmd(values, mask=only))[0] == rhal.SKR_EINVAL, f"a setpoint just {word} {name}'s range is refused")
            bridge._held = list(near_high)
            values = list(near_high)
            values[index] = near_high[index] + span / 8 if near_high[index] + span / 8 <= high else near_high[index] - span / 8
            check(write(bridge, cmd(values, mask=only))[0] == rhal.SKR_OK, "and one inside it, as far from the same edge, is accepted")
        check(len(daemon.targets) == count + 9, "so exactly the nine inside it were sent")
        bridge._held = [0.0] * 9
        for what, values, mode in (
            ("a NaN setpoint", [math.nan] + [0.0] * 8, rhal.MODE_POSITION), ("an infinite setpoint", [0.0] * 8 + [math.inf], rhal.MODE_POSITION),
            ("a step the joint cannot make in one period", at(head_roll=SPEEDS["head_roll"] * 0.05 * 2), rhal.MODE_POSITION), ("a velocity command", [0.0] * 9, rhal.MODE_VELOCITY),
            ("a torque command", [0.0] * 9, rhal.MODE_TORQUE), ("eight joints", [0.0] * 8, rhal.MODE_POSITION), ("ten joints", [0.0] * 10, rhal.MODE_POSITION),
        ):
            check(write(bridge, cmd(values, mode=mode))[0] == rhal.SKR_EINVAL, f"{what} is EINVAL")
        # A refusal says which joint and why. Found against the maker's own daemon (DRY-COVERAGE-PLAN.md, the forty-eighth session): a head that started outside the range it was given had every command answered -22 and not one word.
        # CONTRIBUTING.md: an error names its cause and the next action. Each cause is said once, because a stream at the control rate that is refused for one reason would otherwise say it twenty times a second.
        bridge._held = [0.0] * 9
        code, said = write(bridge, cmd([math.nan] + [0.0] * 8))
        check(code == rhal.SKR_EINVAL and "head_x" in said and "finite" in said, "a setpoint that is not finite is refused naming the joint and saying it is not finite", said)
        code, said = write(bridge, cmd(at(head_roll=SPEEDS["head_roll"] * 0.05 * 2)))
        check(code == rhal.SKR_EINVAL and "head_roll" in said and "one period" in said and "speed" in said, "a step the joint's speed does not allow names the joint and the speed", said)
        pitch_high = frames.DOCUMENTED_LIMITS["head_pitch"][1]
        bridge._held = at(head_pitch=pitch_high - 0.001)
        code, said = write(bridge, cmd(at(head_pitch=pitch_high + 0.001)))
        check(code == rhal.SKR_EINVAL and "head_pitch" in said and f"{pitch_high:.4f}" in said and "range" in said, "a setpoint beyond a range names the joint, the value and the range", said)
        code, again = write(bridge, cmd(at(head_pitch=pitch_high + 0.001)))
        check(code == rhal.SKR_EINVAL and again == "", "and the same refusal is not said twice in a row", again)
        code, other = write(bridge, cmd(at(head_pitch=pitch_high - 0.001, head_roll=SPEEDS["head_roll"] * 0.05 * 2)))
        check(code == rhal.SKR_EINVAL and "head_roll" in other, "while a different cause is said again", other)
        # The same cause as the last one said, with an accepted command between: it is said again. (The first version of this check followed a different cause, so a run that was never ended passed it.)
        bridge._held = at(head_pitch=pitch_high - 0.001)
        code, once = write(bridge, cmd(at(head_pitch=pitch_high + 0.001)))
        check(code == rhal.SKR_EINVAL and "head_pitch" in once, "the range refusal is said, after a different one", once)
        check(write(bridge, cmd(at(head_pitch=pitch_high - 0.002)))[0] == rhal.SKR_OK, "a command that is accepted ends the run")
        bridge._held = at(head_pitch=pitch_high - 0.001)
        code, back = write(bridge, cmd(at(head_pitch=pitch_high + 0.001)))
        check(code == rhal.SKR_EINVAL and "head_pitch" in back, "so the same cause after an accepted command is said again: a person who fixed it and broke it again is told", back)
        bridge._held = [0.0] * 9
        code, said = write(bridge, cmd([0.0] * 9, mode=rhal.MODE_VELOCITY))
        check(code == rhal.SKR_EINVAL and "position" in said, "a velocity command is refused saying the daemon takes a position", said)
        code, said = write(bridge, cmd([0.0] * 8))
        check(code == rhal.SKR_EINVAL and "9" in said and "8" in said, "and eight joints says there are nine", said)
        # The head's yaw may be at most 65 degrees (1.134 rad) from the body's: held at 1.2 against a body at 0, a step away is refused and a step towards is not.
        small = SPEEDS["head_yaw"] * 0.05 * 0.5
        bridge._held = at(head_yaw=1.2, body_yaw=0.0)
        count = len(daemon.targets)
        check(write(bridge, cmd(at(head_yaw=1.2 + small, body_yaw=0.0)))[0] == rhal.SKR_EINVAL, "a head yaw more than 65 degrees from the body's is refused: the daemon documents it and the driver enforces it")
        check(write(bridge, cmd(at(head_yaw=1.2 - small, body_yaw=0.0)))[0] == rhal.SKR_OK, "and one that brings them closer, to 61.5 degrees, is accepted")
        time.sleep(0.1)
        check(len(daemon.targets) == count + 1, "so only the accepted one was sent to a daemon that would have clamped the other without a word")
        bridge._held = [0.0] * 9
        count = len(daemon.targets)
        check(write(bridge, cmd(at(head_pitch=step * 0.5, antenna_left=0.2), mask=[1, 1, 1, 1, 1, 1, 1, 1, 0]))[0] == rhal.SKR_OK, "a masked joint's value is not read")
        check(daemon.targets[-1]["target_antennas"][1] == 0.0 and len(daemon.targets) == count + 1, "and it holds the setpoint this driver last issued for it")
        bridge.close()

    with SimulatedDaemon() as daemon:
        # A daemon that has gone is a failure of the command, and it is said once per run like a refusal: the link-chaos run's blackhole made a 20 Hz stream print "did not answer" at 20 Hz, eleven lines in the
        # window, which buries the cause (DRY-COVERAGE-PLAN.md, forty-eighth session).
        bridge = opened(daemon)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        one = SPEEDS["head_pitch"] * 0.05 * 0.5
        daemon.drop = True
        code, first = write(bridge, cmd(at(head_pitch=one)))
        check(code == rhal.SKR_EIO and "did not answer" in first, "a command to a daemon that has gone is EIO, and says the daemon did not answer", first)
        code, second = write(bridge, cmd(at(head_pitch=one)))
        check(code == rhal.SKR_EIO and second == "", "and the same failure on the next period is not said again", second)
        daemon.drop = False
        until(lambda: write(bridge, cmd(at(head_pitch=one)))[0] == rhal.SKR_OK, 3.0)
        daemon.drop = True
        code, third = write(bridge, cmd(at(head_pitch=one * 2)))
        check(code == rhal.SKR_EIO and "did not answer" in third, "and when the daemon comes back and goes again, it is said again", third)
        daemon.drop = False
        bridge.close()

    with SimulatedDaemon() as daemon:
        bridge = opened(daemon)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        daemon.external_move = "9f3a"
        code, said = write(bridge, cmd(at(head_pitch=0.01)))
        check(code == rhal.SKR_EPERM and "ignored" in said, "a daemon that ignores a target because a move is running says so, and the driver tells the caller the command did not happen")
        code, again = write(bridge, cmd(at(head_pitch=0.01)))
        check(code == rhal.SKR_EPERM and again == "", "and says so once, not on every command of a stream while somebody else's move runs", again)
        daemon.external_move = None
        daemon.drop = True
        began = time.monotonic()
        check(write(bridge, cmd(at(head_pitch=0.01)))[0] == rhal.SKR_EIO, "a daemon that has stopped answering is EIO on a command, not a command that was 'sent'")
        daemon.drop = False
        daemon.delay_s = 3.0
        began = time.monotonic()
        code, _ = write(bridge, cmd(at(head_pitch=0.01)))
        took = time.monotonic() - began
        check(code == rhal.SKR_EIO and took < 0.3, f"and a daemon that is only slow costs a command a fifth of a second and not the half a second a status gets: the loop waits for a command ({took * 1000:.0f} ms)")
        daemon.delay_s = 0.0
        daemon.error = "motor 3 overheated"
        until(lambda: state(bridge)[1].fault_code == 1, 3.0)
        count = len(daemon.targets)
        check(write(bridge, cmd(at(head_pitch=0.01)))[0] == rhal.SKR_EPERM and len(daemon.targets) == count, "a daemon holding an error takes no command, and none is sent")
        daemon.error = None
        until(lambda: state(bridge)[1].fault_code == 0, 3.0)
        daemon.set_mode(frames.MotorMode.DISABLED)
        until(lambda: state(bridge)[1].flags & rhal.FLAG_DEGRADED, 3.0)
        check(write(bridge, cmd(at(head_pitch=0.01)))[0] == rhal.SKR_EPERM, "and neither do motors that are limp")
        bridge.close()

    # ── the stop ─────────────────────────────────────────────────────────────
    section("the stop — latched at once, told to the daemon on a thread of its own")
    with SimulatedDaemon() as daemon:
        daemon.place(at(head_pitch=0.2))
        bridge = opened(daemon)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        daemon.delay_s = 1.0
        began = time.monotonic()
        check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.05, f"the stop returns in {(time.monotonic() - began) * 1000:.1f} ms with the daemon taking a second to answer anything")
        check(write(bridge, cmd(at(head_pitch=0.2)))[0] == rhal.SKR_EPERM, "and the latch is already holding: a command is refused")
        check(bool(state(bridge)[1].flags & rhal.FLAG_ESTOP), "the state says stopped")
        daemon.delay_s = 0.0
        check(until(lambda: any(t.get("target_head_pose", {}).get("pitch") is not None and abs(t["target_head_pose"]["pitch"] - 0.2) < 1e-9 for t in daemon.targets), 6.0), "and the daemon is told to hold where the head is, asked again until it has heard (the first requests waited out the delay)")
        check(daemon.modes == [], "`hold` does not take the motors' torque away")
        bridge.release_stop()
        bridge.close()
    with SimulatedDaemon() as daemon:
        # A request that is lost, as a link loses one: not slow (a slow request still arrives, which is why the delay above does not show a retry) and not a daemon that has gone. The seeding campaign's survivor
        # `reachy-stop-not-retried` was a stop told once.
        daemon.place(at(head_pitch=0.2))
        bridge = opened(daemon)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        daemon.drop_posts = 3
        bridge.emergency_stop()
        check(until(lambda: any(t.get("target_head_pose", {}).get("pitch") is not None and abs(t["target_head_pose"]["pitch"] - 0.2) < 1e-9 for t in daemon.targets), 6.0),
              "a stop whose first three requests were lost on the way is asked again until the daemon has it: the hold arrives")
        check(daemon.drop_posts == 0, "and it was three that were lost, not three that were late")
        bridge.release_stop()
        bridge.close()
    with SimulatedDaemon() as daemon:
        # After a stop the head may be somewhere other than the last setpoint (it coasted, or somebody moved it). The next step is worked out from where it is: the seeding campaign's survivor
        # `reachy-release-held-stale` worked it out from where the loop last asked.
        daemon.place(at(head_pitch=0.2))
        bridge = opened(daemon)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        one_step = SPEEDS["head_pitch"] * 0.05 * 0.9
        check(write(bridge, cmd(at(head_pitch=0.2 + one_step)))[0] == rhal.SKR_OK, "a step is accepted")
        bridge.emergency_stop()
        until(lambda: any(abs(t.get("target_head_pose", {}).get("pitch", 9.0) - 0.2) < 1e-9 for t in daemon.targets), 6.0)
        daemon.place(at(head_pitch=-0.3))
        until(lambda: abs(state(bridge)[1].position_rad[4] + 0.3) < 1e-6, 3.0)
        check(bridge.release_stop() == rhal.SKR_OK, "a release is accepted")
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        check(write(bridge, cmd(at(head_pitch=-0.3 + one_step)))[0] == rhal.SKR_OK, "and the next step is worked out from where the head is, 0.5 rad from the last setpoint, and not from where the loop last asked")
        check(write(bridge, cmd(at(head_pitch=0.2 + one_step)))[0] == rhal.SKR_EINVAL, "while the last setpoint, which the head is no longer at, is a long way from where it can step to")
        bridge.close()
    with SimulatedDaemon() as daemon:
        bridge = opened(daemon, stop_mode="limp")
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        bridge.emergency_stop()
        check(until(lambda: daemon.modes == [frames.MotorMode.DISABLED]), "`limp` takes the motors' torque away")
        check(bridge.release_stop() == rhal.SKR_OK and daemon.mode == frames.MotorMode.ENABLED, "and a release enables them again")
        bridge.close()
        check(frames.MotorMode.DISABLED not in daemon.modes[1:], "a close never lets go of the head, whatever the stop mode is")
    with SimulatedDaemon() as daemon:
        bridge = opened(daemon)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        daemon.external_move = "5b2e"
        bridge.emergency_stop()
        check(until(lambda: daemon.stopped_moves == ["5b2e"]), "a move that is running is cancelled by the stop: a head that is being sent somewhere is not stopped by a target it ignores")
        bridge.close()
    with SimulatedDaemon() as daemon:
        bridge = opened(daemon)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        bridge.emergency_stop()
        until(lambda: any("target_head_pose" in t for t in daemon.targets))
        check(bridge.release_stop() == rhal.SKR_OK and not state(bridge)[1].flags & rhal.FLAG_ESTOP, "release_stop clears the latch when the daemon is healthy")
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        check(write(bridge, cmd(at(head_pitch=0.01)))[0] == rhal.SKR_OK, "and a command is accepted again")
        bridge.emergency_stop()
        daemon.error = "motor 3 overheated"
        until(lambda: state(bridge)[1].fault_code == 1, 3.0)
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.release_stop()
        check(code == rhal.SKR_EIO and "motor 3 overheated" in said.getvalue() and state(bridge)[1].flags & rhal.FLAG_ESTOP, "an error the daemon holds is not cleared by a release: EIO, the stop stays latched, and the error is named")
        bridge.close()
    with SimulatedDaemon() as daemon:
        bridge = opened(daemon)
        daemon.stop()
        time.sleep(0.1)
        check(bridge.emergency_stop() == rhal.SKR_OK, "a stop to a daemon that has gone away is still OK: the latch is what holds")
        bridge.close()
    check(ReachyMiniBridge().emergency_stop() == rhal.SKR_OK, "and a stop to a bridge that was never opened is OK")

    # ── close, and a session ─────────────────────────────────────────────────
    section("close and reopen — a stop belongs to a session")
    with SimulatedDaemon() as daemon:
        daemon.place(at(head_pitch=0.1))
        bridge = opened(daemon)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        before = len(daemon.targets)
        bridge.close()
        check(len(daemon.targets) == before + 1 and abs(daemon.targets[-1]["target_head_pose"]["pitch"] - 0.1) < 1e-9, "closing a session ends it with a target at the present pose: the head is not left going to the last setpoint")
        check(write(bridge, cmd(at(head_pitch=0.01)))[0] == rhal.SKR_EPERM, "and a closed bridge takes no command")
        bridge.emergency_stop()
        said = io.StringIO()
        with contextlib.redirect_stderr(said):
            code = bridge.open(config(daemon))
        check(code == rhal.SKR_OK and not (state(bridge)[1].flags & rhal.FLAG_ESTOP), "it opens again, and the stop latched before the reopen did not survive it (DRY-COVERAGE-PLAN.md F73)")
        check(bridge.read_sensor("camera", rhal.Frame()) == rhal.SKR_ENOSYS, "a sensor is ENOSYS: the daemon's media is not this driver's")
        bridge.close()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  FAIL {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main())

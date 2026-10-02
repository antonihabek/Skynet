"""The bridge against PX4's own firmware, with the reference MAVLink implementation as a second opinion.

    python3 live_px4.py copter udpin:0.0.0.0:14540 udpin:0.0.0.0:14030
    python3 live_px4.py plane  udpin:0.0.0.0:14540 udpin:0.0.0.0:14030
    python3 live_px4.py rover  udpin:0.0.0.0:14540 udpin:0.0.0.0:14030

`sim/run-px4.sh` starts PX4's own SIH build — the firmware's flight stack with its physics simulated in-process, from the image PX4
publishes (`px4io/px4-sitl-sih`) — one vehicle model at a time, and runs this beside it in the same network namespace. PX4 sends MAVLink
to two ports of its own accord: 14540, which this driver listens on, and 14030, a *second instance of its MAVLink module*, which `pymavlink`
listens on. Nothing here is a counterpart written in this repository: every state the driver claims is also read, from the same
firmware, by an implementation that shares no code with it, and every mode it claims to have set is read back from the vehicle's own heartbeat.

Everything is **measured against the firmware, in real time**. `PX4_COMMIT` (the abbreviated hash the image was built from) pins the build when
set; without it — the nightly's run against PX4's newest image — the build is reported and not compared.
"""

from __future__ import annotations

import json
import math
import os
import struct
import sys
import threading
import time

import skynet_rhal as rhal

import selftest as t
from live import Live, Second, _wrap, netem
from skynet_mavlink import frames

EXPECTED_TYPE = {"copter": frames.MavType.QUADROTOR, "plane": frames.MavType.FIXED_WING, "rover": frames.MavType.GROUND_ROVER}
#: PX4's SIH build's simulation-only airframes (`init.d-posix/airframes/10040_sihsim_*`).
SIH_AIRFRAMES = range(10040, 10050)


def px4_mode(second: Second) -> str | None:
    """The reference's own reading of the vehicle's mode, in its names (PX4's hold is its LOITER)."""
    heartbeat = second.get("HEARTBEAT")
    if heartbeat is None:
        return None
    return second.mavutil.interpret_px4_mode(heartbeat.base_mode, heartbeat.custom_mode).lower()


def reference_param(second: Second, name: str) -> float | None:
    """A parameter as the reference reads it: PX4 puts an integer's bytes in the float field, and the reference leaves them there, so this
    undoes that itself with the type that comes with it — not with the driver's decoder."""
    second.m.mav.param_request_read_send(second.m.target_system, second.m.target_component, name.encode(), -1)
    deadline = time.monotonic() + 3.0
    while time.monotonic() < deadline:
        second.pump()
        message = second.m.recv_match(type="PARAM_VALUE", blocking=True, timeout=0.2)
        if message is not None and message.param_id.strip("\x00") == name:
            if message.param_type == 9:
                return message.param_value
            fmt = {1: "<B", 2: "<b", 3: "<H", 4: "<h", 5: "<I", 6: "<i"}[message.param_type]
            raw = struct.pack("<f", message.param_value)
            return float(struct.unpack(fmt, raw[: struct.calcsize(fmt)])[0])
    return None


def run(family: str, link: str, second_link: str | None) -> None:
    print(f"skynet-mavlink live block against PX4: {family}, {link}\n")
    live = Live(family, link, None)
    t.section(f"{family} — open, and what it says it is")
    started = time.monotonic()
    code = live.bridge.open(json.dumps({"link": link, "open_timeout_ms": 30000}))
    t.check(code == rhal.SKR_OK, f"the bridge opens PX4's {family} in {time.monotonic() - started:.1f}s, with no word from the configuration about what it is")
    if code != rhal.SKR_OK:
        return
    second = Second(second_link) if second_link else None
    live.second = second
    descriptor = json.loads(live.bridge.describe())
    code, status = live.ext("status")
    pinned = os.environ.get("PX4_COMMIT", "").strip()
    if pinned:
        t.check(str(status.get("build", "")).startswith(pinned), f"the build hash the vehicle reports ({status.get('build')}) starts with the pinned image's ({pinned}): this is the firmware `sim/run-px4.sh` pulls")
    else:
        print(f"  NOT PINNED: PX4_COMMIT is not set, so the build the vehicle reports ({status.get('build')}) is reported and not compared (the nightly's run against the newest image).")
    t.check(descriptor["simulator"] is True, "the descriptor says it is a simulation, because its airframe exists only in PX4's simulation build")
    t.check(descriptor["base"] == ("wheeled" if family == "rover" else "aerial"), "and the base kind")
    if second is not None:
        second.m.mav.command_long_send(second.m.target_system, second.m.target_component, 512, 0, 148, 0, 0, 0, 0, 0, 0)
        version = None
        for _ in range(40):
            second.pump()
            version = second.latest.get("AUTOPILOT_VERSION")
            if version is not None:
                break
            time.sleep(0.1)
        packed = version.flight_sw_version if version is not None else 0
        reference = f"{packed >> 24}.{(packed >> 16) & 0xFF}.{(packed >> 8) & 0xFF}"
        t.check(version is not None and descriptor["robot_slug"] == f"px4-{family}-{reference}",
                f"the slug names the autopilot, the vehicle and the firmware version the reference reads too ({descriptor['robot_slug']} against {reference})")
        second.pump()
        t.check(second.heartbeat.type == EXPECTED_TYPE[family] and second.heartbeat.autopilot == frames.Autopilot.PX4,
                f"the reference reads MAV_TYPE {second.heartbeat.type} from autopilot {second.heartbeat.autopilot}: a {family} running PX4")
        t.check(status["mode"] == "hold" and px4_mode(second) == "loiter", f"the mode the driver reports ({status['mode']}) is the one the reference reads off the heartbeat (its LOITER: PX4's hold)")
        airframe = reference_param(second, "SYS_AUTOSTART")
        driver = live.ext("param name=SYS_AUTOSTART")[1]
        t.check(airframe is not None and isinstance(driver, dict) and driver["value"] == airframe and int(airframe) in SIH_AIRFRAMES,
                f"the airframe the driver reads (SYS_AUTOSTART {driver.get('value') if isinstance(driver, dict) else driver}) is the reference's own decoding of the same four bytes, and a simulation-only one (F225)")
        gcs = reference_param(second, "NAV_DLL_ACT")
        t.check(gcs is not None and status["link_failsafe"] == gcs, f"and so is the link-loss failsafe (NAV_DLL_ACT {status['link_failsafe']})")

    {"copter": copter, "plane": plane, "rover": rover}[family](live, second)
    t.check(live.bridge.close() == rhal.SKR_OK, "close")


def enu_agrees(second: Second | None, live: Live, tolerance: float = 0.6) -> bool:
    return second is None or t.until(lambda: second.enu() is not None and max(abs(a - b) for a, b in zip(second.enu(), live.state().base_pose[:3])) < tolerance, 4.0)


def background(live: Live, motion: str) -> tuple[threading.Thread, list]:
    """A motion that blocks, run beside the sampling that watches it."""
    outcome: list = []
    worker = threading.Thread(target=lambda: outcome.append(live.ext(motion)), daemon=True)
    worker.start()
    return worker, outcome


def copter(live: Live, second: Second | None) -> None:
    t.section("copter — arm and takeoff")
    code, words = live.ext("takeoff alt=10")
    t.check(code == rhal.SKR_EPERM and "not armed" in str(words), "a takeoff before arming is refused here: PX4 would acknowledge it, enter takeoff mode while disarmed and carry nothing out (F227)")
    if second is not None:
        t.check(px4_mode(second) == "loiter", "and the vehicle is where it was, in hold, as the reference reads it")
    ready, words = live.until_ok("arm", 90)
    t.check(ready, "arm is accepted once PX4 is ready" + ("" if ready else f" — last said: {words}"))
    t.check(second is None or t.until(second.armed, 4.0), "which the reference reads as armed on the vehicle's own heartbeat")
    code, result = live.ext("takeoff alt=10")
    t.check(code == rhal.SKR_OK, "takeoff to 10 m is accepted" + ("" if code == rhal.SKR_OK else f" — {result}"))
    t.check(t.until(lambda: live.state().base_pose[2] > 9.0, 90.0), "and the pose climbs to 10 m")
    t.check(t.until(lambda: live.ext("status")[1].get("mode") == "hold", 40.0), "and PX4 returns itself to hold on arrival: the driver changed no mode")
    state = live.state()
    t.check(abs(state.base_pose[2] - 10.0) < 1.0 and abs(state.base_pose[0]) < 2.0 and abs(state.base_pose[1]) < 2.0,
            f"at 10 m over the point it left: {tuple(round(v, 1) for v in state.base_pose[:3])}")
    t.check(enu_agrees(second, live), "and the reference's position, converted here by explicit rule, agrees to 0.6 m")
    if second is not None:
        driver_yaw = 2 * math.atan2(state.base_pose[5], state.base_pose[6])
        t.check(second.yaw_enu() is not None and abs(_wrap(driver_yaw - second.yaw_enu())) < 0.1, "and so does the yaw, through the whole quaternion path")

    t.section("copter — goto, local and global")
    t.check(live.ext("goto x=15 y=0 z=10")[0] == rhal.SKR_OK, "goto 15 m east, in the local frame")
    t.check(t.until(lambda: abs(live.state().base_pose[0] - 15.0) < 1.0 and abs(live.state().base_pose[1]) < 1.0 and abs(live.state().base_pose[2] - 10.0) < 1.0, 60.0),
            "the vehicle arrives at ENU (15, 0, 10): PX4's own projection puts the latitude and longitude the driver computed where it was asked")
    t.check(enu_agrees(second, live), "and the reference puts it at the same place")
    if second is not None:
        t.check(t.until(lambda: px4_mode(second) == "loiter", 6.0), "in hold, as the reference reads it")
    here = live.state().base_pose
    # Ten metres north of where it is, by the global position the reference reads.
    global_position = second.get("GLOBAL_POSITION_INT") if second is not None else None
    if global_position is not None:
        lat, lon = global_position.lat / 1e7 + math.degrees(10.0 / 6371000.0), global_position.lon / 1e7
        home_alt = (global_position.alt - global_position.relative_alt) / 1000.0
        t.check(live.ext(f"goto lat={lat:.7f} lon={lon:.7f} alt=10")[0] == rhal.SKR_OK, "goto ten metres north, by latitude and longitude")
        t.check(t.until(lambda: abs(live.state().base_pose[1] - (here[1] + 10.0)) < 1.0 and abs(live.state().base_pose[0] - here[0]) < 1.0, 60.0),
                f"and it arrives {live.state().base_pose[1] - here[1]:+.1f} m north of where it was (home is {home_alt:.1f} m above sea level)")

    t.section("copter — velocity is a stream with an end")
    start = live.state().base_pose[1]
    modes_seen: set = set()
    pairs: list[tuple[float, float]] = []
    worker, outcome = background(live, "velocity vx=0 vy=2 vz=0 duration=3")
    peak = 0.0
    while worker.is_alive():
        peak = max(peak, live.state().base_twist[1])
        if second is not None:
            modes_seen.add(px4_mode(second))
            local = second.get("LOCAL_POSITION_NED")
            if local is not None and peak > 1.0:
                pairs.append((live.state().base_twist[1], local.vx))
        time.sleep(0.2)
    code, result = outcome[0]
    moved = live.state().base_pose[1] - start
    t.check(code == rhal.SKR_OK, f"a velocity of 2 m/s north for three seconds returns once it has handed the vehicle back" + ("" if code == rhal.SKR_OK else f" — {result}"))
    t.check(1.5 < peak < 2.6, f"the twist peaks at {peak:.2f} m/s")
    t.check(3.0 < moved < 9.0, f"and the vehicle moved {moved:.1f} m north")
    if second is not None:
        t.check(len(pairs) >= 3 and all(abs(mine - theirs) < 0.5 for mine, theirs in pairs),
                f"and while it did, the reference's north velocity (NED vx) is the driver's ENU vy: {[(round(a, 2), round(b, 2)) for a, b in pairs[-4:]]}")
        t.check("offboard" in {m for m in modes_seen if m}, "the reference saw the vehicle in offboard while it streamed")
        t.check("rtl" not in modes_seen, "and never in return-to-launch: the stream was handed to hold before it stopped (F226)")
    t.check(t.until(lambda: live.ext("status")[1].get("mode") == "hold", 8.0), "the vehicle is in hold afterwards")
    t.check(t.until(lambda: abs(live.state().base_twist[1]) < 0.3, 10.0), "and stops")
    t.check(live.ext("status")[1].get("state") == "ACTIVE", "with no failsafe: its own state is ACTIVE, not CRITICAL")
    yaw0 = 2 * math.atan2(live.state().base_pose[5], live.state().base_pose[6])
    code, _ = live.ext("velocity vx=0 vy=0 vz=0 yaw_rate=0.5 duration=3")
    yaw1 = 2 * math.atan2(live.state().base_pose[5], live.state().base_pose[6])
    t.check(code == rhal.SKR_OK and _wrap(yaw1 - yaw0) > 0.4, f"a yaw rate of +0.5 rad/s is a counter-clockwise turn in ENU: {_wrap(yaw1 - yaw0):+.2f} rad in three seconds")

    impaired(live, second)

    t.section("copter — the stop, in the middle of a stream")
    worker, outcome = background(live, "velocity vx=3 vy=0 vz=0 duration=5")
    t.check(t.until(lambda: live.state().base_twist[0] > 2.0, 10.0), "moving east at 3 m/s")
    began = time.monotonic()
    live.bridge.emergency_stop()
    took = time.monotonic() - began
    t.check(took < 0.5, f"emergency_stop returns in {took * 1000:.0f} ms, while the stream runs")
    if second is not None:
        t.check(t.until(lambda: px4_mode(second) == "loiter", 6.0), f"and the vehicle's own heartbeat says hold within {time.monotonic() - began:.1f} s")
    worker.join(8.0)
    t.check(not worker.is_alive() and outcome and outcome[0][0] == rhal.SKR_EPERM, "the velocity ended, saying the stop took the vehicle")
    t.check(t.until(lambda: abs(live.state().base_twist[0]) < 0.5, 10.0), "and it is stopped within ten seconds")
    t.check(live.state().flags & rhal.FLAG_ESTOP != 0, "the state carries the latch")
    t.check(live.ext("land")[0] == rhal.SKR_EPERM, "a motion is refused while it is latched")
    t.check(live.bridge.release_stop() == rhal.SKR_OK, "release_stop")
    t.check(live.ext("land")[0] == rhal.SKR_OK, "and land is taken")
    t.check(t.until(lambda: live.state().base_pose[2] < 0.5, 120.0), "the vehicle comes down")
    if second is not None:
        t.check(t.until(lambda: not second.armed(), 60.0), "and PX4 disarms itself after landing")


def impaired(live: Live, second: Second | None) -> None:
    """A link that is not loopback: delay, jitter and loss between the driver and the firmware, with a stream that must keep PX4 in offboard."""
    t.section("copter — a link that is not loopback")
    if not netem("delay 80ms 40ms loss 3%"):
        print("  NOT RUN: `tc` cannot impair the loopback here (the container needs NET_ADMIN and iproute2). Reported as not run, never as passed.")
        return
    try:
        modes_seen: set = set()
        worker, outcome = background(live, "velocity vx=2 vy=0 vz=0 duration=3")
        peak = 0.0
        ages: list[float] = []
        while worker.is_alive():
            state = live.state()
            peak = max(peak, state.base_twist[0])
            ages.append((rhal.acquisition_ns() - state.stamp_ns) / 1e9)
            if second is not None:
                modes_seen.add(px4_mode(second))
            time.sleep(0.2)
        ages.sort()
        t.check(outcome[0][0] == rhal.SKR_OK, "a three-second stream through 80 ms +/- 40 ms and 3% loss still hands the vehicle back" + ("" if outcome[0][0] == rhal.SKR_OK else f" — {outcome[0][1]}"))
        t.check(peak > 1.4, f"the vehicle reaches {peak:.2f} m/s of the 2 asked for")
        t.check("rtl" not in modes_seen, "and PX4 never flew its offboard-loss failsafe: the setpoints that arrived were enough to keep it")
        t.check(ages[int(0.95 * (len(ages) - 1))] < 1.5, f"the state a read returns is fresh: the 95th percentile of its age is {ages[int(0.95 * (len(ages) - 1))]:.2f} s")
        started = time.monotonic()
        code, words = live.ext("mode name=hold")
        t.check(code == rhal.SKR_OK, f"an acknowledged command still lands, in {time.monotonic() - started:.1f}s" + ("" if code == rhal.SKR_OK else f" ({words})"))
        t.check(netem("delay 250ms 120ms loss 40%"), "the link is made very bad: 250 ms +/- 120 ms and 40% loss")
        bounded = True
        results = []
        for motion in ("status", "mode name=hold", "param name=NAV_DLL_ACT", "hold"):
            started = time.monotonic()
            code, _ = live.ext(motion)
            results.append((motion, code, time.monotonic() - started))
            bounded = bounded and time.monotonic() - started < 20.0
        t.check(bounded, "no request hangs on it: " + ", ".join(f"{m.split()[0]} {c} in {d:.1f}s" for m, c, d in results))
    finally:
        netem(None)
    t.check(t.until(lambda: live.bridge.read_state(rhal.State(), 0) == rhal.SKR_OK, 15.0), "when the link recovers, reads are OK again")
    ready, words = live.until_ok("mode name=hold", 30, every=1.0)
    t.check(ready, "and a command lands" + ("" if ready else f" — last said: {words}"))


def plane(live: Live, second: Second | None) -> None:
    t.section("plane — arm, takeoff, hold")
    ready, words = live.until_ok("arm", 90)
    t.check(ready, "arm is accepted once PX4 is ready" + ("" if ready else f" — last said: {words}"))
    t.check(live.ext("takeoff alt=40")[0] == rhal.SKR_OK, "takeoff is a command to PX4's fixed wing, and is accepted")
    t.check(t.until(lambda: live.state().base_pose[2] > 25.0, 120.0), "and it climbs")
    t.check(enu_agrees(second, live, 3.0), "to an altitude the reference reads the same, to within a wing's sampling")
    live.bridge.emergency_stop()
    if second is not None:
        t.check(t.until(lambda: px4_mode(second) == "loiter", 6.0), "emergency_stop: hold, on the vehicle's own heartbeat — which for a wing is a circle")
    code, status = live.ext("status")
    t.check(status["stop_latched"] is True and status["armed"] is True and status["landed"] != "on_ground", "and it is still flying, which the status says: the stop is a circle and not a landing")
    live.bridge.release_stop()


def rover(live: Live, second: Second | None) -> None:
    t.section("rover — arm, goto, velocity, hold")
    ready, words = live.until_ok("arm", 90)
    t.check(ready, "arm is accepted once PX4 is ready" + ("" if ready else f" — last said: {words}"))
    t.check(live.ext("takeoff alt=5")[0] == rhal.SKR_EINVAL, "a rover has no takeoff")
    t.check(live.ext("goto x=10 y=0")[0] == rhal.SKR_OK, "goto 10 m east, in the local frame, with no altitude")
    t.check(t.until(lambda: abs(live.state().base_pose[0] - 10.0) < 1.5 and abs(live.state().base_pose[1]) < 1.5, 90.0), "the rover drives there")
    t.check(enu_agrees(second, live), "and the reference agrees where it is")
    start = live.state().base_pose[1]
    worker, outcome = background(live, "velocity vx=0 vy=1 vz=0 duration=4")
    worker.join(15.0)
    t.check(outcome and outcome[0][0] == rhal.SKR_OK, "a velocity of 1 m/s north for four seconds returns" + ("" if outcome and outcome[0][0] == rhal.SKR_OK else f" — {outcome}"))
    t.check(live.state().base_pose[1] - start > 1.0, f"and the rover moved north: {live.state().base_pose[1] - start:+.1f} m")
    live.bridge.emergency_stop()
    if second is not None:
        t.check(t.until(lambda: px4_mode(second) == "loiter", 6.0), "emergency_stop: hold, on the vehicle's own heartbeat")
    t.check(t.until(lambda: abs(live.state().base_twist[1]) < 0.2 and abs(live.state().base_twist[0]) < 0.2, 20.0), "and the rover is stationary")
    live.bridge.release_stop()


def main(argv: list[str]) -> int:
    if len(argv) < 2 or argv[0] not in EXPECTED_TYPE:
        print(__doc__)
        return 2
    run(argv[0], argv[1], argv[2] if len(argv) > 2 else None)
    return t.finish()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

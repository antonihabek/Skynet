"""The bridge against ArduPilot's own firmware, with the reference MAVLink implementation as a second opinion.

    python3 live.py copter tcp:127.0.0.1:5760 tcp:127.0.0.1:5762
    python3 live.py rover  tcp:127.0.0.1:5770 tcp:127.0.0.1:5772
    python3 live.py plane  tcp:127.0.0.1:5780 tcp:127.0.0.1:5782
    python3 live.py boat   tcp:127.0.0.1:5790 tcp:127.0.0.1:5792   # a rover built as a boat: base `surface` (§97)
    python3 live.py sub    tcp:127.0.0.1:5800 tcp:127.0.0.1:5802   # ArduSub: opens only with a `stop_mode`, and is driven by sticks

`sim/run-sim.sh` starts one vehicle of each family (`arducopter`, `ardurover`, `arduplane`, `ardusub`, and a
rover built as a boat) and runs this against each. The second link is a *different serial port of the same
firmware* (SERIAL1, five ports up from SERIAL0), read by `pymavlink` — which shares no code with this driver —
so that every state the driver claims is also read by something that was not written here, and every mode it
claims to have set is read back from the vehicle's own heartbeat and not from the driver's memory.

Everything here is **measured against the firmware, in real time**: an EKF that takes twenty seconds to
converge, a takeoff that climbs at the vehicle's own rate, a velocity setpoint the vehicle stops following on
its own timeout. Nothing is scripted to agree.
"""

from __future__ import annotations

import json
import math
import os
import select
import subprocess
import sys
import threading
import time

import skynet_rhal as rhal

import selftest as t
from skynet_mavlink import frames
from skynet_mavlink.bridge import TOKEN, MavlinkBridge

EXPECTED_TYPE = {"copter": frames.MavType.QUADROTOR, "rover": frames.MavType.GROUND_ROVER, "plane": frames.MavType.FIXED_WING,
                 "boat": frames.MavType.SURFACE_BOAT, "sub": frames.MavType.SUBMARINE}


class Second:
    """`pymavlink` on the vehicle's second serial port: the second opinion."""

    def __init__(self, link: str) -> None:
        from pymavlink import mavutil  # noqa: PLC0415

        self.mavutil = mavutil
        self.m = mavutil.mavlink_connection(link, source_system=254, source_component=191)
        self.latest: dict = {}
        #: Every PARAM_VALUE the vehicle has sent, by name: several may arrive in one pump, and `latest` keeps one message of each kind.
        self.params: dict = {}
        #: Every mode a heartbeat of the vehicle announced, in order: a mode the vehicle leaves again within a second is in `latest` only until the next heartbeat.
        self.modes: list[int] = []
        self._beat = 0.0
        # A serial port of the firmware says nothing until it has heard something: the ground station speaks first.
        deadline = time.monotonic() + 30.0
        self.heartbeat = None
        while self.heartbeat is None and time.monotonic() < deadline:
            self._heartbeat()
            beat = self.m.recv_match(type="HEARTBEAT", blocking=True, timeout=1.0)
            self.heartbeat = beat if beat is not None and self._of_the_vehicle(beat) else None
        if self.heartbeat is None:
            raise SystemExit(f"nothing answered on {link} in thirty seconds")
        self.latest["HEARTBEAT"] = self.heartbeat  # the one just read is the vehicle's latest until the next one arrives
        target = (self.m.target_system, self.m.target_component)
        for message_id, hz in ((31, 20), (32, 20), (30, 20), (33, 5)):
            self.m.mav.command_long_send(*target, 511, 0, message_id, 1e6 / hz, 0, 0, 0, 0, 0)

    def _heartbeat(self) -> None:
        if time.monotonic() - self._beat >= 1.0:
            self._beat = time.monotonic()
            self.m.mav.heartbeat_send(6, 8, 0, 0, 4)  # a ground station, as ArduPilot expects to be spoken to

    def _of_the_vehicle(self, message) -> bool:
        """A heartbeat is the vehicle's only if a ground station did not send it. PX4's second MAVLink instance relays the *driver's own*
        heartbeat (system 255, a GCS) to this port, measured on 2026-10-03 against `px4io/px4-sitl-sih`: reading "the latest HEARTBEAT" took
        the driver for the vehicle about half the time, and the mode, the arming bit and the state it read were the driver's (F229)."""
        return message.type != self.mavutil.mavlink.MAV_TYPE_GCS

    def pump(self) -> None:
        """Read until the socket is empty, so that `latest` is the newest message and not the newest one before a quiet spell.

        `recv_match(blocking=False)` returns `None` for a datagram that yields no message as well as for an empty socket, so stopping at the first
        `None` left the rest of a backlog unread: measured on 2026-10-03 against PX4, one pump after twenty seconds unread reached a position stamped
        12 s when the newest was 32.5 s, and a goto aimed at the reference's idea of where the vehicle was went seven metres from where it was (F230)."""
        self._heartbeat()
        budget = time.monotonic() + 5.0  # forty seconds unread is about six thousand messages from PX4, and a Python parser takes over a second to read them
        while time.monotonic() < budget:
            message = self.m.recv_match(blocking=False)
            if message is None:
                if not select.select([self.m.port], [], [], 0)[0]:
                    return
                continue
            if message.get_type() == "HEARTBEAT" and not self._of_the_vehicle(message):
                continue
            if message.get_type() == "PARAM_VALUE":
                self.params[message.param_id.strip("\x00")] = message
            if message.get_type() == "HEARTBEAT":
                self.modes.append(message.custom_mode)
            self.latest[message.get_type()] = message

    def request(self, message_id: int, hz: float) -> None:
        """Ask the vehicle for a message at a rate, on this link: the reference reads what the driver does not ask for."""
        self.m.mav.command_long_send(self.m.target_system, self.m.target_component, 511, 0, message_id, 1e6 / hz, 0, 0, 0, 0, 0)

    def get(self, kind: str):
        self.pump()
        return self.latest.get(kind)

    def mode(self) -> int | None:
        heartbeat = self.get("HEARTBEAT")
        return None if heartbeat is None else heartbeat.custom_mode

    def armed(self) -> bool:
        heartbeat = self.get("HEARTBEAT")
        return bool(heartbeat and heartbeat.base_mode & 128)

    def enu(self) -> tuple[float, float, float] | None:
        """Position in ENU from the reference's own LOCAL_POSITION_NED, converted here by explicit rule."""
        message = self.get("LOCAL_POSITION_NED")
        return None if message is None else (message.y, message.x, -message.z)

    def yaw_enu(self) -> float | None:
        """ENU yaw (0 = east, counter-clockwise) from ATTITUDE's NED yaw (0 = north, clockwise)."""
        message = self.get("ATTITUDE")
        return None if message is None else _wrap(math.pi / 2 - message.yaw)


def _wrap(angle: float) -> float:
    return (angle + math.pi) % (2 * math.pi) - math.pi


class Live:
    def __init__(self, family: str, link: str, second: Second | None) -> None:
        self.family = family
        self.link = link
        self.second = second
        self.bridge = MavlinkBridge()

    def ext(self, text: str) -> tuple[int, dict | str]:
        code, out = self.bridge.extension(TOKEN, text.encode())
        body = out.decode()
        try:
            return code, json.loads(body)
        except json.JSONDecodeError:
            return code, body

    def state(self) -> rhal.State:
        state = rhal.State()
        self.bridge.read_state(state, 0)
        return state

    def until_ok(self, motion: str, timeout: float, every: float = 2.0) -> tuple[bool, str]:
        """Repeat a motion until the vehicle accepts it: the firmware's own readiness, in the firmware's own time."""
        deadline = time.monotonic() + timeout
        words = ""
        while time.monotonic() < deadline:
            code, out = self.ext(motion)
            if code == rhal.SKR_OK:
                return True, ""
            words = str(out)
            time.sleep(every)
        return False, words


def run(family: str, link: str, second_link: str | None) -> None:
    print(f"skynet-mavlink live block: {family}, {link}\n")
    if family == "sub":
        needs_a_stop(link)
    live = Live(family, link, None)
    t.section(f"{family} — open, and what it says it is")
    started = time.monotonic()
    # A submarine's stop is the operator's to name, so it has no default and the configuration says it. `alt_hold` keeps the depth, which is the one stop that needs no position source.
    code = live.bridge.open(json.dumps({"link": link, "open_timeout_ms": 30000, **({"stop_mode": "alt_hold"} if family == "sub" else {})}))
    t.check(code == rhal.SKR_OK, f"the bridge opens ArduPilot's {family} in {time.monotonic() - started:.1f}s")
    if code != rhal.SKR_OK:
        return
    # The reference comes second: a SITL vehicle holds its boot until its first serial port has a client, and
    # its other ports only speak once that one does.
    second = Second(second_link) if second_link else None
    live.second = second
    descriptor = json.loads(live.bridge.describe())
    expected_commit = _pinned_commit()
    code, status = live.ext("status")
    t.check(status.get("build") == expected_commit[:8], f"the build hash the vehicle reports ({status.get('build')}) is the pinned commit's ({expected_commit[:8]}): this is the firmware in sim/Dockerfile")
    t.check(descriptor["simulator"] is True, "the descriptor says it is a simulation, because the firmware has SIM_ parameters")
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
        t.check(version is not None and descriptor["robot_slug"] == f"ardupilot-{ {'sub': 'submarine'}.get(family, family) }-{reference}",
                f"the slug names the family and the firmware version the reference reads too ({descriptor['robot_slug']} against {reference})")
    t.check(descriptor["base"] == {"rover": "wheeled", "boat": "surface", "sub": "submerged"}.get(family, "aerial"), "and the base kind (§97: a hull is `surface`, a body under the water `submerged`, and not `wheeled`)")
    if second is not None:
        second.pump()
        t.check(second.heartbeat.type == EXPECTED_TYPE[family], f"the reference reads MAV_TYPE {second.heartbeat.type} from the same firmware, which is a {family}")
        t.check(status["mode"] == _mode_name(family, second.mode()), f"the mode the driver reports ({status['mode']}) is the one the reference reads off the heartbeat")

    if family == "copter":
        copter(live, second)
    elif family in ("rover", "boat"):
        rover(live, second)
    elif family == "sub":
        submarine(live, second)
    else:
        plane(live, second)
    t.check(live.bridge.close() == rhal.SKR_OK, "close")


def needs_a_stop(link: str) -> None:
    t.section("sub — it opens only when told what stopping it means")
    bridge = MavlinkBridge()
    code = bridge.open(json.dumps({"link": link, "open_timeout_ms": 30000}))
    t.check(code == rhal.SKR_ENODEV, "ArduSub's own firmware, with no `stop_mode` in the configuration, is refused (ENODEV): the stop is the operator's decision, and nothing was sent to the vehicle")
    code = bridge.open(json.dumps({"link": link, "open_timeout_ms": 30000, "stop_mode": "guided"}))
    t.check(code == rhal.SKR_ENODEV, "and so is one whose stop is a mode that goes on moving")
    bridge.close()


def copter(live: Live, second: Second | None) -> None:
    t.section("copter — the firmware's own readiness")
    code, words = live.ext("arm")
    t.check(code == rhal.SKR_EPERM and words != "", f"arming at once is refused, in the firmware's words: {str(words)[-90:]}")
    ready, words = live.until_ok("mode name=guided", 120)
    t.check(ready, "guided is accepted")
    code, words = live.ext("takeoff alt=10")
    t.check(code == rhal.SKR_EPERM, "takeoff before arming is refused here, before the firmware can ignore it")
    ready, words = live.until_ok("arm", 150)
    t.check(ready, "and once the estimate converges, arm is accepted" + ("" if ready else f" — last said: {words}"))
    t.check(second is None or t.until(second.armed, 4.0), "which the reference reads as SAFETY_ARMED on the vehicle's own heartbeat")
    t.check(live.ext("takeoff alt=10")[0] == rhal.SKR_OK, "takeoff to 10 m is accepted")
    t.check(t.until(lambda: live.state().base_pose[2] > 9.4, 45.0), "and the pose climbs to 10 m at the vehicle's own rate")
    state = live.state()
    t.check(abs(state.base_pose[2] - 10.0) < 0.8 and abs(state.base_pose[0]) < 1.0 and abs(state.base_pose[1]) < 1.0, "over the point it left")
    if second is not None:
        opinion = second.enu()
        t.check(opinion is not None and max(abs(a - b) for a, b in zip(opinion, state.base_pose[:3])) < 0.5, "and the reference's position, converted here by explicit rule, agrees to half a metre")
        driver_yaw = 2 * math.atan2(state.base_pose[5], state.base_pose[6])
        t.check(second.yaw_enu() is not None and abs(_wrap(driver_yaw - second.yaw_enu())) < 0.1, "and so does the yaw, through the whole quaternion path")

    t.section("copter — goto and velocity")
    t.check(live.ext("goto x=15 y=0 z=10")[0] == rhal.SKR_OK, "goto 15 m east")
    t.check(t.until(lambda: 14.0 < live.state().base_pose[0] < 16.0, 30.0), "the pose arrives at ENU x = 15")
    state = live.state()
    t.check(abs(state.base_pose[1]) < 1.0 and abs(state.base_pose[2] - 10.0) < 1.0, "with y and z held")
    if second is not None:
        t.check(t.until(lambda: second.enu() is not None and abs(second.enu()[0] - state.base_pose[0]) < 0.6, 3.0), "and the reference puts it at the same east")
        t.check(t.until(lambda: abs(_wrap(2 * math.atan2(live.state().base_pose[5], live.state().base_pose[6]) - second.yaw_enu())) < 0.1, 3.0), "with the same yaw, which after flying east is the vehicle facing east")
    start = live.state().base_pose[1]
    stop_at = time.monotonic() + 4.0
    peak = 0.0
    pairs: list[tuple[float, float]] = []
    while time.monotonic() < stop_at:
        live.ext("velocity vx=0 vy=2 vz=0")
        peak = max(peak, live.state().base_twist[1])
        time.sleep(0.4)
        if second is not None and second.get("LOCAL_POSITION_NED") is not None and time.monotonic() > stop_at - 2.0:
            pairs.append((live.state().base_twist[1], second.get("LOCAL_POSITION_NED").vx))
    moved = live.state().base_pose[1] - start
    t.check(1.6 < peak < 2.4, f"a velocity of 2 m/s north peaks at {peak:.2f} m/s in the twist")
    t.check(4.0 < moved < 9.0, f"and moves the vehicle {moved:.1f} m north in four seconds")
    if second is not None:
        t.check(len(pairs) >= 3 and all(abs(mine - theirs) < 0.5 for mine, theirs in pairs),
                f"and while it does, the reference's north velocity (NED vx) is the driver's ENU vy: {[(round(a, 2), round(b, 2)) for a, b in pairs]}")
    t.check(t.until(lambda: abs(live.state().base_twist[1]) < 0.3, 10.0), "and when the setpoints stop the vehicle stops following them, on its own timeout")

    yaw0 = 2 * math.atan2(live.state().base_pose[5], live.state().base_pose[6])
    ned0 = second.get("ATTITUDE").yaw if second is not None else None
    stop_at = time.monotonic() + 3.0
    while time.monotonic() < stop_at:
        live.ext("velocity vx=0 vy=0 vz=0 yaw_rate=0.5")
        time.sleep(0.4)
    yaw1 = 2 * math.atan2(live.state().base_pose[5], live.state().base_pose[6])
    turned = _wrap(yaw1 - yaw0)
    t.check(turned > 0.4, f"a yaw rate of +0.5 rad/s is a counter-clockwise turn in ENU: {turned:+.2f} rad in three seconds")
    if second is not None and ned0 is not None:
        t.check(_wrap(second.get("ATTITUDE").yaw - ned0) < -0.4, "which the reference reads as the same turn with the opposite sign, because NED's yaw is clockwise")

    impaired(live, second)

    t.section("copter — the stop")
    stop_at = time.monotonic() + 3.0
    while time.monotonic() < stop_at:
        live.ext("velocity vx=3 vy=0 vz=0")
        time.sleep(0.4)
    t.check(live.state().base_twist[0] > 2.0, "moving east at 3 m/s")
    started = time.monotonic()
    live.bridge.emergency_stop()
    if second is not None:
        t.check(t.until(lambda: second.mode() == frames.COPTER_MODES["brake"], 4.0), f"emergency_stop: the vehicle's own heartbeat says BRAKE within {time.monotonic() - started:.1f}s")
    t.check(t.until(lambda: abs(live.state().base_twist[0]) < 0.5, 8.0), "and it is stopped within eight seconds")
    t.check(live.state().flags & rhal.FLAG_ESTOP != 0, "the state carries the latch")
    t.check(live.ext("land")[0] == rhal.SKR_EPERM, "a motion is refused while it is latched")
    t.check(live.bridge.release_stop() == rhal.SKR_OK, "release_stop")
    t.check(live.ext("mode name=land")[0] == rhal.SKR_OK, "and land is taken")
    if second is not None:
        t.check(t.until(lambda: second.mode() == frames.COPTER_MODES["land"], 4.0), "as the vehicle's heartbeat confirms")
    t.check(t.until(lambda: live.state().base_pose[2] < 0.3, 90.0), "the vehicle comes down")
    if second is not None:
        t.check(t.until(lambda: not second.armed(), 40.0), "and the firmware disarms itself after landing")


def netem(rule: str | None) -> bool:
    """Impair (or, with `None`, restore) the container's loopback, which every link in this block crosses."""
    command = ["tc", "qdisc", "del", "dev", "lo", "root"] if rule is None else ["tc", "qdisc", "replace", "dev", "lo", "root", "netem", *rule.split()]
    return subprocess.run(command, capture_output=True, text=True).returncode == 0


def impaired(live: Live, second: Second | None) -> None:
    """A link that is not loopback. Delay, jitter and loss on the wire between the driver and the firmware."""
    t.section("copter — a link that is not loopback")
    if not netem("delay 80ms 40ms loss 3%"):
        print("  NOT RUN: `tc` cannot impair the loopback here (the container needs NET_ADMIN and iproute2). Reported as not run, never as passed.")
        return
    try:
        # 80 ms of delay with 40 ms of jitter and 3% loss, each way: a poor wireless link to a companion computer.
        ages: list[float] = []
        peak = 0.0
        stop_at = time.monotonic() + 6.0
        while time.monotonic() < stop_at:
            live.ext("velocity vx=2 vy=0 vz=0")
            state = live.state()
            peak = max(peak, state.base_twist[0])
            ages.append((rhal.acquisition_ns() - state.stamp_ns) / 1e9)
            time.sleep(0.25)
        ages.sort()
        t.check(peak > 1.4, f"setpoints still get through 80 ms +/- 40 ms and 3% loss: the vehicle reaches {peak:.2f} m/s of the 2 asked for")
        t.check(ages[int(0.95 * (len(ages) - 1))] < 1.5, f"and the state a read returns is fresh: the 95th percentile of its age is {ages[int(0.95 * (len(ages) - 1))]:.2f} s")
        started = time.monotonic()
        code, words = live.ext("mode name=loiter")
        t.check(code == rhal.SKR_OK, f"an acknowledged command still lands, in {time.monotonic() - started:.1f}s" + ("" if code == rhal.SKR_OK else f" ({words})"))
        t.check(live.ext("mode name=guided")[0] == rhal.SKR_OK, "and so does the one after it")

        started = time.monotonic()
        live.bridge.emergency_stop()
        if second is not None:
            t.check(t.until(lambda: second.mode() == frames.COPTER_MODES["brake"], 8.0), f"emergency_stop over that link: BRAKE on the vehicle's heartbeat after {time.monotonic() - started:.1f}s")
        live.bridge.release_stop()

        t.check(netem("delay 250ms 120ms loss 40%"), "the link is made very bad: 250 ms +/- 120 ms and 40% loss")
        bounded = True
        results = []
        for motion in ("status", "mode name=guided", "param name=FS_GCS_ENABLE", "hold"):
            started = time.monotonic()
            code, _ = live.ext(motion)
            results.append((motion, code, time.monotonic() - started))
            bounded = bounded and time.monotonic() - started < 20.0
        t.check(bounded, "no request hangs on it: " + ", ".join(f"{m.split()[0]} {c} in {d:.1f}s" for m, c, d in results))
        state = live.state()
        t.check(state.stamp_ns > 0, "and a read still returns")
    finally:
        netem(None)
    t.check(t.until(lambda: live.bridge.read_state(rhal.State(), 0) == rhal.SKR_OK, 15.0), "when the link recovers, reads are OK again")
    ready, words = live.until_ok("mode name=guided", 30, every=1.0)
    t.check(ready, "and a command lands" + ("" if ready else f" — last said: {words}"))
    if second is not None:
        t.check(t.until(lambda: second.enu() is not None and max(abs(a - b) for a, b in zip(second.enu(), live.state().base_pose[:3])) < 1.0, 10.0), "with the pose back in agreement with the reference")


def rover(live: Live, second: Second | None) -> None:
    t.section("rover — guided, goto, velocity, hold")
    ready, words = live.until_ok("mode name=guided", 150)
    t.check(ready, "guided is accepted" + ("" if ready else f" — last said: {words}"))
    ready, words = live.until_ok("arm", 150)
    t.check(ready, "arm is accepted" + ("" if ready else f" — last said: {words}"))
    t.check(t.until(lambda: not math.isnan(live.state().base_pose[0]), 30.0), "the pose is localised")
    t.check(live.ext("takeoff alt=5")[0] == rhal.SKR_EINVAL, "a rover has no takeoff")
    t.check(live.ext("goto x=10 y=0")[0] == rhal.SKR_OK, "goto 10 m east")
    t.check(t.until(lambda: live.state().base_pose[0] > 8.5, 90.0), "the rover drives there")
    state = live.state()
    if second is not None:
        t.check(t.until(lambda: second.enu() is not None and abs(second.enu()[0] - live.state().base_pose[0]) < 0.6, 3.0), "and the reference agrees where it is")
    t.check(abs(state.base_pose[2]) < 0.5, "on the ground")
    start = live.state().base_pose[1]
    stop_at = time.monotonic() + 9.0
    while time.monotonic() < stop_at:
        live.ext("velocity vx=0 vy=1")
        time.sleep(0.4)
    state = live.state()
    heading = 2 * math.atan2(state.base_pose[5], state.base_pose[6])
    t.check(state.base_pose[1] - start > 2.0, f"a velocity of 1 m/s north moves it north: {state.base_pose[1] - start:+.1f} m in nine seconds")
    t.check(abs(_wrap(heading - math.pi / 2)) < math.radians(35), f"and the rover turned toward it: heading {math.degrees(heading):.0f} degrees against 90 for north (F179: with a yaw rate of zero it drives straight along its old heading)")
    live.bridge.emergency_stop()
    if second is not None:
        t.check(t.until(lambda: second.mode() == frames.ROVER_MODES["hold"], 4.0), "emergency_stop: HOLD, on the vehicle's own heartbeat")
    if live.family == "boat":
        # HOLD stops the thrust and not the hull: a boat has no brake and comes to rest as the water allows (§97).
        # So the bound is the water's, measured, and the check says how long it took rather than only that it did.
        began = time.monotonic()
        rested = t.until(lambda: abs(live.state().base_twist[1]) < 0.2, 90.0)
        t.check(rested, f"and the boat comes to rest: {time.monotonic() - began:.0f} s from HOLD to under 0.2 m/s, with no brake (HOLD stops the thrust, not the hull)")
    else:
        t.check(t.until(lambda: abs(live.state().base_twist[1]) < 0.2, 10.0), "and the rover is stationary")
    live.bridge.release_stop()


def plane(live: Live, second: Second | None) -> None:
    t.section("plane — arm, takeoff by mode, hold")
    t.check("velocity" not in [m["name"] for m in json.loads(live.bridge.describe())["motions"]], "no velocity setpoint is offered: a wing cannot fly one")
    ready, words = live.until_ok("arm", 150)
    t.check(ready, "arm is accepted" + ("" if ready else f" — last said: {words}"))
    t.check(live.ext("mode name=takeoff")[0] == rhal.SKR_OK, "takeoff is a mode, and is selected")
    if second is not None:
        t.check(t.until(lambda: second.mode() == frames.PLANE_MODES["takeoff"], 4.0), "as the vehicle's heartbeat confirms")
    t.check(t.until(lambda: live.state().base_pose[2] > 30.0, 90.0), "and it climbs")
    if second is not None:
        t.check(t.until(lambda: second.enu() is not None and abs(second.enu()[2] - live.state().base_pose[2]) < 3.0, 3.0), "to an altitude the reference reads the same, to within a wing's sampling")
    live.bridge.emergency_stop()
    if second is not None:
        t.check(t.until(lambda: second.mode() == frames.PLANE_MODES["loiter"], 4.0), "emergency_stop: LOITER, because a fixed wing cannot stand still")
    code, status = live.ext("status")
    t.check(status["stop_latched"] is True and status["armed"] is True, "and it is still flying, which the status says: the stop is a circle and not a landing")
    live.bridge.release_stop()


def drive_and_watch(live: Live, text: str) -> tuple[tuple[int, dict | str], float, float, rhal.State, rhal.State]:
    """A drive in a thread, with the twist watched while it runs: the reply, the fastest it went (m/s) and the fastest it turned (rad/s), and the state either side of it."""
    before = live.state()
    outcome: list = []
    worker = threading.Thread(target=lambda: outcome.append(live.ext(text)))
    worker.start()
    fastest = turned = 0.0
    while worker.is_alive():
        twist = live.state().base_twist
        fastest, turned = max(fastest, math.hypot(twist[0], twist[1])), max(turned, abs(twist[5]))
        time.sleep(0.1)
    worker.join()
    return outcome[0], fastest, turned, before, live.state()


def _along(before: rhal.State, after: rhal.State, bearing: float) -> float:
    """How far the pose moved along `bearing` (radians, ENU, counter-clockwise from east)."""
    return (after.base_pose[0] - before.base_pose[0]) * math.cos(bearing) + (after.base_pose[1] - before.base_pose[1]) * math.sin(bearing)


def submarine(live: Live, second: Second | None) -> None:
    t.section("sub — arm, and the position estimate that arrives late")
    # ArduSub reports MAV_STATE_CRITICAL for the first seconds after boot, which this driver reads as a failsafe and refuses a motion by: the retry is the firmware's own readiness.
    ready, words = live.until_ok("arm", 120, every=2.0)
    t.check(ready, "arm is accepted" + ("" if ready else f" — last said: {words}"))
    t.check(second is None or t.until(second.armed, 4.0), "which the reference reads as SAFETY_ARMED on the vehicle's own heartbeat")
    t.check(live.ext("goto x=1 y=0 z=0")[0] == rhal.SKR_EPERM, "a goto in manual, which ignores a setpoint without a word, is refused here and says so")
    t.check(t.until(lambda: not math.isnan(live.state().base_pose[0]), 150.0), "the pose is localised (ArduSub's estimate waits for a position fix, about forty seconds after boot in this simulator)")
    state = live.state()
    if second is not None:
        opinion = second.enu()
        t.check(opinion is not None and max(abs(a - b) for a, b in zip(opinion, state.base_pose[:3])) < 0.5, "and the reference's position, converted here by explicit rule, agrees to half a metre")
    t.check(live.ext("mode name=manual")[0] == rhal.SKR_OK, "manual, the mode ArduSub boots in, is taken")

    t.section("sub — drive: the sticks, against the firmware")
    heading = second.yaw_enu() if second is not None else None
    reply, fastest, turned, before, after = drive_and_watch(live, "drive surge=0.8 duration=3")
    t.check(reply[0] == rhal.SKR_OK and reply[1]["frames"] >= 40, f"a three-second surge returns OK after {reply[1].get('frames') if isinstance(reply[1], dict) else reply[1]} frames")
    if heading is not None:
        forward = _along(before, after, heading)
        sideways = _along(before, after, heading + math.pi / 2)
        t.check(forward > 0.3 and abs(sideways) < 0.5 * forward, f"surge is the way the vehicle faces: {forward:.2f} m along the reference's heading and {sideways:+.2f} m across it")
        t.check(0.15 < fastest < 1.0, f"at a speed ArduSub's own gain decides, {fastest:.2f} m/s at the fastest, which is a fraction of a stick and not a velocity")
    t.check(t.until(lambda: math.hypot(*live.state().base_twist[:2]) < 0.25 * max(fastest, 0.2), 2.5), "and the vehicle slows at once when the drive returns: the neutral frames replaced the override, which the vehicle would otherwise have kept for about three seconds")
    t.check(t.until(lambda: math.hypot(*live.state().base_twist[:2]) < 0.1, 8.0), "and comes to rest")

    heading = second.yaw_enu() if second is not None else None
    reply, fastest, turned, before, after = drive_and_watch(live, "drive sway=0.8 duration=3")
    if heading is not None:
        left = _along(before, after, heading + math.pi / 2)
        forward = _along(before, after, heading)
        t.check(reply[0] == rhal.SKR_OK and left > 0.3 and abs(forward) < 0.5 * left, f"sway to the left is to the vehicle's left in the world, which the reference's heading says: {left:+.2f} m left, {forward:+.2f} m forward (MAVLink's y is right: the driver flips it)")
    t.check(t.until(lambda: math.hypot(*live.state().base_twist[:2]) < 0.1, 8.0), "and comes to rest")

    ned_before = second.get("ATTITUDE").yaw if second is not None and second.get("ATTITUDE") is not None else None
    yaw_before = 2 * math.atan2(live.state().base_pose[5], live.state().base_pose[6])
    reply, fastest, turned, before, after = drive_and_watch(live, "drive yaw=0.3 duration=3")
    yaw_after = 2 * math.atan2(after.base_pose[5], after.base_pose[6])
    t.check(reply[0] == rhal.SKR_OK and _wrap(yaw_after - yaw_before) > 0.4, f"a positive yaw is a counter-clockwise turn in ENU: {_wrap(yaw_after - yaw_before):+.2f} rad in three seconds (MAVLink's r is clockwise: the driver flips it)")
    if second is not None and ned_before is not None:
        t.check(_wrap(second.get("ATTITUDE").yaw - ned_before) < -0.3, "which the reference reads as the opposite sign, because NED's yaw is clockwise")
    t.check(t.until(lambda: abs(live.state().base_twist[5]) < 0.05, 8.0), "and the turn ends")

    if second is not None:
        # What a stick does to the vehicle is physics, and this simulator's depth is not a measurement (its pressure sensor reads negative absolute pressure and its depth
        # swings by metres in seconds with nothing commanded), so the vertical axis cannot be checked by where the vehicle goes. What can be checked is what the firmware reads
        # off each stick: ArduSub's own RC channels, in ArduSub's own documented directions (channel 5 forward, 6 lateral, 3 throttle up, 4 yaw clockwise, each above 1500).
        t.section("sub — what the vehicle itself reads off each stick")
        second.request(65, 10)
        time.sleep(1.0)
        for text, channel, direction, what in (("drive surge=0.8 duration=2", 5, +1, "surge forward"), ("drive sway=0.8 duration=2", 6, -1, "sway to the left"),
                                               ("drive heave=0.8 duration=2", 3, +1, "heave up"), ("drive yaw=0.8 duration=2", 4, -1, "a counter-clockwise turn")):
            worker = threading.Thread(target=lambda text=text: live.ext(text))
            worker.start()
            time.sleep(1.0)
            rc = second.get("RC_CHANNELS")
            worker.join()
            raw = None if rc is None else getattr(rc, f"chan{channel}_raw")
            others = [] if rc is None else [getattr(rc, f"chan{other}_raw") for other in (3, 4, 5, 6) if other != channel]
            t.check(raw is not None and (raw - 1500) * direction > 100 and all(abs(value - 1500) < 20 for value in others),
                    f"{what}: the vehicle reads channel {channel} at {raw}, {'above' if direction > 0 else 'below'} its neutral 1500, and the other three at neutral ({others})")
            t.check(t.until(lambda: second.get("RC_CHANNELS") is not None and all(abs(getattr(second.get("RC_CHANNELS"), f"chan{other}_raw") - 1500) < 20 for other in (3, 4, 5, 6)), 1.5),
                    "and when the drive has returned every channel is back at 1500: it ended in neutral, which is what the vehicle reads")
        t.check(t.until(lambda: math.hypot(*live.state().base_twist[:2]) < 0.1 and abs(live.state().base_twist[5]) < 0.05, 10.0), "and the vehicle comes to rest")

    t.section("sub — the modes, and the one the stop names")
    ready, words = live.until_ok("mode name=alt_hold", 30)
    t.check(ready, "alt_hold is taken" + ("" if ready else f" — last said: {words}"))
    if second is not None:
        t.check(t.until(lambda: second.mode() == frames.SUB_MODES["alt_hold"], 4.0), "as the reference's heartbeat confirms")
    t.check(live.ext("drive surge=0.5 duration=0.5")[0] == rhal.SKR_OK, "a drive is taken in it")
    t.check(live.ext("mode name=acro")[0] == rhal.SKR_EPERM, "acro is not selected without `allow_modes`")
    ready, words = live.until_ok("mode name=guided", 60)
    t.check(ready, "guided is taken once there is a position estimate" + ("" if ready else f" — last said: {words}"))
    t.check(live.ext("drive surge=0.5 duration=0.5")[0] == rhal.SKR_EPERM, "and a drive is refused in it, which does not take the sticks")
    start = live.state().base_pose
    stop_at = time.monotonic() + 5.0
    while time.monotonic() < stop_at:
        live.ext("velocity vx=0.4 vy=0 vz=0")
        time.sleep(0.4)
    state = live.state()
    t.check(state.base_pose[0] - start[0] > 0.3, f"a velocity of 0.4 m/s east moves it east: {state.base_pose[0] - start[0]:+.2f} m in five seconds")
    t.check(t.until(lambda: math.hypot(*live.state().base_twist[:2]) < 0.1, 15.0), "and it stops following the setpoint when they stop, on the firmware's own timeout")
    start = live.state().base_pose
    t.check(live.ext("goto x=%.2f y=%.2f z=0" % (start[0] + 1.0, start[1]))[0] == rhal.SKR_OK, "a goto one metre east, at the surface")
    t.check(t.until(lambda: abs(live.state().base_pose[0] - (start[0] + 1.0)) < 0.5, 90.0), "is reached, at the speed ArduSub's position controller has")
    mark = len(second.modes) if second is not None else 0
    t.check(live.ext("surface")[0] == rhal.SKR_OK, "surface is a mode, and it is taken")
    if second is not None:
        # ArduSub ends surface mode itself, in alt_hold, the moment the vehicle is at the surface, and this one is: the mode is on the heartbeat it sends when it changes and not for long after.
        t.check(t.until(lambda: (second.pump(), frames.SUB_MODES["surface"] in second.modes[mark:])[1], 4.0), "as the reference's heartbeats confirm: surface was announced, however briefly (the vehicle was already at the surface, and ArduSub ends the mode itself)")
    ready, words = live.until_ok("mode name=manual", 30)
    t.check(ready, "back in manual")

    t.section("sub — the stop")
    outcome: list = []
    worker = threading.Thread(target=lambda: outcome.append(live.ext("drive surge=1 duration=5")))
    worker.start()
    t.check(t.until(lambda: math.hypot(*live.state().base_twist[:2]) > 0.25, 5.0), "a drive is moving the vehicle")
    started = time.monotonic()
    live.bridge.emergency_stop()
    stopped_in = time.monotonic() - started
    t.check(stopped_in < 0.3, f"the stop returns in {stopped_in * 1000:.0f} ms, never waiting behind the stream")
    if second is not None:
        t.check(t.until(lambda: second.mode() == frames.SUB_MODES["alt_hold"], 4.0), f"the vehicle's own heartbeat says alt_hold, the mode that was named as the stop, {time.monotonic() - started:.1f} s after")
    worker.join(2.0)
    t.check(not worker.is_alive() and outcome and outcome[0][0] == rhal.SKR_EPERM, "the drive ends at once, saying the stop took the vehicle")
    t.check(t.until(lambda: math.hypot(*live.state().base_twist[:2]) < 0.2, 4.0), "and the vehicle is nearly at rest within four seconds: the stop sent neutral, so the last stick is not obeyed for the override's three")
    t.check(t.until(lambda: math.hypot(*live.state().base_twist[:2]) < 0.1, 8.0), "and at rest")
    t.check(live.state().flags & rhal.FLAG_ESTOP != 0, "the state carries the latch")
    t.check(live.ext("drive surge=0.5 duration=0.2")[0] == rhal.SKR_EPERM, "a motion is refused while it is latched")
    t.check(live.bridge.release_stop() == rhal.SKR_OK, "release_stop")
    mark = len(second.modes) if second is not None else 0
    t.check(live.ext("surface")[0] == rhal.SKR_OK, "and the way out is open: surface")
    if second is not None:
        t.check(t.until(lambda: (second.pump(), frames.SUB_MODES["surface"] in second.modes[mark:])[1], 4.0), "as the vehicle's heartbeats confirm")
    t.check(live.ext("disarm")[0] == rhal.SKR_OK, "disarm, which a submarine does without being landed")
    if second is not None:
        t.check(t.until(lambda: not second.armed(), 10.0), "and the reference reads it disarmed")



def _mode_name(family: str, number: int | None) -> str | None:
    # A boat is ArduRover with a boat frame: it shares Rover's modes.
    table = frames.FAMILY_MODES["rover" if family == "boat" else family]
    for name, value in table.items():
        if value == number:
            return name
    return None


def _pinned_commit() -> str:
    for path in ("/opt/ardupilot/commit",):
        if os.path.exists(path):
            return open(path).read().strip()
    raise SystemExit("no /opt/ardupilot/commit: this block runs inside sim/Dockerfile's image")


def main(argv: list[str]) -> int:
    if len(argv) < 2:
        print(__doc__)
        return 2
    run(argv[0], argv[1], argv[2] if len(argv) > 2 else None)
    return t.finish()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

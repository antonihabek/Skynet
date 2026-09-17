"""The RHAL driver: a Universal Robots arm over RTDE.

§4's five entry points over :mod:`skynet_ur.rtde`, with the dashboard for the one transition RTDE
cannot make and a URScript loop for the one it cannot command.

Verified against **URSim**, Universal Robots' own controller software, running in a container. That
is the substrate `DRY-COVERAGE-PLAN.md`'s Phase 2 table asks for — *"URSim (UR's own controller,
containerised) … Almost everything, against real vendor software"* — and it is what separates this
driver from the thing F15 refused: the protocol is checked against the vendor's implementation
rather than against a fake written from the same memory as the driver.

# What is read, and what is refused

The output recipe is the body: `actual_q`, `actual_qd`, `actual_current`, `robot_mode`,
`safety_status`, `runtime_state`. RTDE answers `IN_USE` for a variable this controller does not
have — rather than refusing the recipe — so the client checks the returned types and refuses by
name, because one unrecognised variable would shift every field after it.

`actual_current` is **motor current in amps, not joint torque in newton-metres**, and the descriptor
says so by declaring no torque at all rather than converting: the constant that turns one into the
other is a torque constant and a gear ratio the controller does not publish, and §5.2 forbids
inventing it. `effort_nm` is `NaN` on every cycle, which is §24's value for a quantity the driver
does not measure.

# What it will not do

It will not clear a protective stop. `unlock protective stop` exists on the dashboard and this
driver does not send it: a protective stop is the machine saying a person or a collision happened,
and a driver that cleared it on the way up would erase the one signal that says so. `open()` refuses
with the safety status it read.
"""

from __future__ import annotations

import json
import math
import socket
import time

import skynet_rhal as rhal

from .control import CMD_HOLD, CMD_SERVO, SERVO_PERIOD_S, SERVO_PROGRAM
from .dashboard import Dashboard, DashboardError
from .rtde import Rtde, RtdeError

__all__ = ["UrBridge", "OUTPUTS", "INPUTS", "JOINT_NAMES", "JOINT_ROLES"]

#: The body, read every cycle.
OUTPUTS = [
    "actual_q",
    "actual_qd",
    "actual_current",
    "robot_mode",
    "safety_status",
    "runtime_state",
]

#: The setpoint, written every cycle. Six doubles and the gate the control program reads.
INPUTS = [f"input_double_register_{index}" for index in range(6)] + ["input_int_register_0"]

#: A UR's six joints, in the order `actual_q` reports them, with the canonical roles §5.1 gives
#: them. The names are the vendor's own and the roles are this driver's reading of the kinematics —
#: a six-axis arm with a spherical wrist, which is what every UR is.
JOINT_NAMES = ["shoulder_pan", "shoulder_lift", "elbow", "wrist_1", "wrist_2", "wrist_3"]
JOINT_ROLES = [
    "right_arm.shoulder.yaw",
    "right_arm.shoulder.pitch",
    "right_arm.elbow.flex",
    "right_arm.wrist.pitch",
    "right_arm.wrist.yaw",
    "right_arm.wrist.roll",
]

#: `robot_mode` values this driver acts on. The controller publishes more.
MODE_RUNNING = 7
#: `safety_status` values. 1 is NORMAL; everything above it is a person's problem.
SAFETY_NORMAL = 1
SAFETY_NAMES = {
    1: "NORMAL",
    2: "REDUCED",
    3: "PROTECTIVE_STOP",
    4: "RECOVERY",
    5: "SAFEGUARD_STOP",
    6: "SYSTEM_EMERGENCY_STOP",
    7: "ROBOT_EMERGENCY_STOP",
    8: "VIOLATION",
    9: "FAULT",
}


class UrBridge:
    """A UR arm as one Skynet body."""

    def __init__(self) -> None:
        self._rtde: Rtde | None = None
        self._dashboard: Dashboard | None = None
        self._script: socket.socket | None = None
        self._descriptor: str | None = None
        self._seq = 0
        self._estop = False
        self._setpoint = [0.0] * 6
        self._last: dict | None = None
        self._simulator = True
        #: The controller's publication period, from the frequency this session negotiated.
        self._period_s = 1.0 / 125.0
        #: Cycles on which nothing new had arrived from the controller. Counted rather than hidden:
        #: it is the number that says whether this machine is keeping up with a 125 Hz stream.
        self.stale_cycles = 0

    # ── §4's entry points ────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError as error:
            _say(f"the configuration is not JSON: {error}")
            return rhal.SKR_EINVAL
        host = settings.get("host") or ""
        if not host:
            _say(
                "no `host` in the configuration. There is no default: a driver that guessed "
                '192.168.1.100 would eventually connect to somebody else\'s robot. Give '
                '`{"host": "192.168.56.101"}` — or `127.0.0.1` for a URSim container.'
            )
            return rhal.SKR_ENODEV
        # §35.3's direction of safety: declaring `simulated` about a real arm only caps the tier,
        # and declaring `real` about a simulator is the one that lets a simulated run be presented
        # as evidence. So the default is `true` and a real integration says so deliberately.
        self._simulator = bool(settings.get("simulator", True))
        frequency = float(settings.get("frequency_hz", 125.0))
        if not 1.0 <= frequency <= 500.0:
            _say(f"`frequency_hz` of {frequency} is outside what RTDE will negotiate (1 to 500).")
            return rhal.SKR_EINVAL
        self._period_s = 1.0 / frequency

        try:
            self._connect(host, frequency, settings)
        except (RtdeError, DashboardError) as refusal:
            _say(str(refusal))
            self._shutdown()
            return rhal.SKR_ENODEV
        except OSError as error:
            _say(f"{host}: {error}")
            self._shutdown()
            return rhal.SKR_EIO
        return rhal.SKR_OK

    def _connect(self, host: str, frequency: float, settings: dict) -> None:
        self._dashboard = Dashboard(host)
        safety = self._dashboard.safety_status()
        if safety != "NORMAL":
            raise DashboardError(
                f"this robot's safety status is `{safety}`. This driver does not clear it: a "
                "protective stop is the machine saying a person or a collision happened, and a "
                "driver that unlocked it on the way up would erase the one signal that says so. "
                "Clear it at the machine."
            )
        self._dashboard.bring_up()

        self._rtde = Rtde(host)
        self._rtde.negotiate(2)
        version = self._rtde.controller_version()
        _say(f"URControl {'.'.join(str(part) for part in version)} on {host}")
        self._rtde.setup_outputs(OUTPUTS, frequency)
        self._rtde.setup_inputs(INPUTS)
        self._rtde.start()

        # The control program, once. Sent on the secondary interface because that is where a
        # program goes; everything cyclic afterwards is RTDE.
        self._script = socket.create_connection((host, 30002), timeout=5.0)
        self._script.sendall(SERVO_PROGRAM.encode())

        sample = self._await_sample(timeout_s=float(settings.get("startup_timeout_s", 10.0)))
        self._setpoint = list(sample["actual_q"])
        self._descriptor = self._build_descriptor(sample)

    def _await_sample(self, timeout_s: float) -> dict:
        assert self._rtde is not None
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            sample = self._rtde.newest()
            if sample is not None:
                self._last = sample
                return sample
            time.sleep(0.01)
        raise RtdeError(
            f"no data package arrived within {timeout_s:.0f}s of starting the exchange. The "
            "controller says: "
            + (self._rtde.messages[-1] if self._rtde.messages else "nothing")
        )

    def close(self) -> int:
        self._shutdown()
        return rhal.SKR_OK

    def _shutdown(self) -> None:
        if self._rtde is not None:
            try:
                self._rtde.write(self._setpoint + [CMD_HOLD])
                self._rtde.pause()
            except (RtdeError, OSError):
                pass
            self._rtde.close()
            self._rtde = None
        if self._script is not None:
            try:
                self._script.close()
            except OSError:
                pass
            self._script = None
        if self._dashboard is not None:
            self._dashboard.close()
            self._dashboard = None

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if self._rtde is None:
            return rhal.SKR_EPERM
        # Wait for a **new** sample, bounded by the controller's own publication period.
        #
        # This is the first driver here whose source has a clock of its own. `drivers/mock` computes
        # a sample when asked, MuJoCo steps when asked, the LeRobot bridge reads when asked — all of
        # them are faster than whatever is calling. RTDE publishes at 125 Hz whatever the caller
        # does, so two `read_state` calls microseconds apart genuinely have one sample between them.
        #
        # Returning the previous one immediately is truthful and reads, to §15's
        # `rhal.state.completeness`, as *"two consecutive reads returned seq=0 and seq=0"* — a
        # stalled producer, which is exactly what §24 says a repeated sequence means. So the driver
        # waits instead, and the bound is not a guess: it is the period this session negotiated. A
        # caller faster than the robot is slowed to the robot, which is the only honest thing a
        # driver in that position can do.
        #
        # §4's rule 1 — wait-free on the real-time path — is relaxed for a Python driver by the
        # `skynet_rhal` protocol itself, on the grounds that it is Guest tier and not on that path.
        deadline = time.monotonic() + self._period_s
        sample = self._rtde.newest()
        while sample is None and time.monotonic() < deadline:
            time.sleep(0.0005)
            sample = self._rtde.newest()
        if sample is None:
            # A whole period with nothing published. That is a stalled producer for real, and it is
            # reported as one: the previous sample with its sequence held.
            self.stale_cycles += 1
            if self._last is None:
                return rhal.SKR_ETIMEDOUT
            sample = self._last
        else:
            self._last = sample
            self._seq += 1

        out.stamp_ns = time.monotonic_ns()
        out.seq = self._seq
        out.fault_code = 0
        # A UR has absolute encoders and is mastered at the factory; there is no homing routine an
        # operator runs, and reporting `complete` would claim one had been.
        out.flags = rhal.HOMING_NOT_REQUIRED
        safety = int(sample["safety_status"])
        mode = int(sample["robot_mode"])
        if self._estop or safety in (6, 7):
            out.flags |= rhal.FLAG_ESTOP
        if safety != SAFETY_NORMAL:
            out.flags |= rhal.FLAG_DEGRADED
            out.fault_code = safety
        if mode != MODE_RUNNING:
            out.flags |= rhal.FLAG_DEGRADED

        out.position_rad = list(sample["actual_q"])
        out.velocity_rad_s = list(sample["actual_qd"])
        # **NaN.** `actual_current` is motor current in amps; joint torque needs a torque constant
        # and a gear ratio this controller does not publish, and §5.2 forbids inventing one.
        out.effort_nm = [math.nan] * 6
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if self._rtde is None:
            return rhal.SKR_EPERM
        if cmd.joint_count != 6:
            return rhal.SKR_EINVAL
        if cmd.mode != rhal.MODE_POSITION:
            # `servoj` is a joint-position statement. Velocity is `speedj` and torque is not
            # reachable from URScript at all, and wiring either against a controller nobody has run
            # them on is the shape F15 refused.
            return rhal.SKR_EINVAL
        if self._estop:
            return rhal.SKR_EPERM

        for index in range(6):
            applied = cmd.mask[index] if index < len(cmd.mask) else 1
            if not applied:
                continue  # hold: the previous setpoint stands, which is what the mask means
            value = cmd.values[index]
            if not math.isfinite(value):
                return rhal.SKR_EINVAL
            self._setpoint[index] = float(value)
        try:
            self._rtde.write(self._setpoint + [CMD_SERVO])
        except (RtdeError, OSError) as error:
            _say(f"the setpoint did not reach the controller: {error}")
            return rhal.SKR_EIO
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        """The strongest stop reachable from here, and it is not a rated one.

        Three things, in the order that matters: the control program is told to hold, so `servoj`
        stops being fed; the dashboard is told to stop the program; and the flag is latched so that
        every later command is refused. A **rated** stop is the emergency circuit, and no frame on
        any of these three sockets reaches it — §15's `safety.estop.independent` is a bench test
        that this body cannot pass, and the descriptor declares no `estop` rather than `true`.
        """
        self._estop = True
        if self._rtde is not None:
            try:
                self._rtde.write(self._setpoint + [CMD_HOLD])
            except (RtdeError, OSError):
                pass
        if self._dashboard is not None:
            try:
                self._dashboard.command("stop")
            except OSError:
                pass
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── the descriptor ───────────────────────────────────────────────────────

    def _build_descriptor(self, sample: dict) -> str:
        joints = []
        for index, (name, role) in enumerate(zip(JOINT_NAMES, JOINT_ROLES)):
            del index
            joints.append(
                {
                    "name": name,
                    "role": role,
                    # Every UR joint is continuous over ±2π; the *software* limits are set in the
                    # installation and are not published over RTDE, so what is declared is the
                    # mechanical range and nothing narrower.
                    "type": "continuous",
                    "limits": {
                        "position_min_rad": -2.0 * math.pi,
                        "position_max_rad": 2.0 * math.pi,
                    },
                    "control_modes": ["position"],
                }
            )
        descriptor = {
            "schema_version": 1,
            "robot_slug": "universal-robots-arm",
            "digest": None,
            "verified": False,
            "simulator": self._simulator,
            "base": "fixed",
            "dof": 6,
            "joints": joints,
            "end_effectors": [],
            "sensors": [],
            "control": {
                "modes": ["position"],
                # The controller's own rate. `servoj` covers 8 ms per statement, so this is the
                # rate the driver must be fed at rather than a maximum it would like.
                "rate_hz_max": round(1.0 / SERVO_PERIOD_S),
                "rate_hz_min": 1,
            },
            # Null. A UR's joint torque ratings are published in a data sheet, not by the
            # controller, and a driver that typed them in would be putting a design value where a
            # sourced safety rating belongs (§63.4).
            "envelope": None,
            "capabilities": [],
            "extensions": [],
            "solvers": [],
        }
        del sample
        return json.dumps(descriptor, separators=(",", ":"))


def _say(message: str) -> None:
    import sys

    print(f"skynet-ur: {message}", file=sys.stderr, flush=True)

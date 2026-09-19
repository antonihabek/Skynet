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
from .rtde import RegistersInUse, Rtde, RtdeError

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

#: Joint ratings per e-Series model, in `JOINT_NAMES` order (base to wrist 3): maximum joint
#: speed in degrees per second, maximum joint torque in newton-metres.
#:
#: Speeds are from each model's own tech sheet, axis-movement table
#: (`universal-robots.com/manuals`, `{model}_techsheet_en.pdf`): the UR3e runs its wrists at
#: 360 °/s and everything else at 180; the UR5e and UR7e run everything at 180; the UR10e,
#: UR12e and UR16e run base and shoulder at 120 and the rest at 180. Torques are from UR's
#: support article "Max. joint torques CB3 and e-Series", which tabulates joint sizes 0–4 at
#: 9/28/54/150/330 Nm and the size each model's each joint carries.
#:
#: The controller never says which model it is — neither RTDE nor the dashboard publishes one —
#: so the model arrives in `open()`'s config from the operator, who knows which arm is bolted to
#: their floor. An absent or untranscribed model (CB3, UR20, UR30) means no envelope: the ratings
#: differ per model and the narrowest-across-the-range single ceiling would brick a UR16e at its
#: wrists' 9 Nm. Transcribing per model with the citation beside the figures is what §63.4's
#: table means by a datasheet safety rating, as opposed to the CAD design value it refuses.
MODEL_RATINGS = {
    # model: ([deg/s base..wrist3], [Nm base..wrist3]).
    "UR3E": ([180, 180, 180, 360, 360, 360], [54, 54, 28, 9, 9, 9]),
    "UR5E": ([180, 180, 180, 180, 180, 180], [150, 150, 150, 28, 28, 28]),
    "UR7E": ([180, 180, 180, 180, 180, 180], [150, 150, 150, 28, 28, 28]),
    "UR10E": ([120, 120, 180, 180, 180, 180], [330, 330, 150, 54, 54, 54]),
    "UR12E": ([120, 120, 180, 180, 180, 180], [330, 330, 150, 54, 54, 54]),
    "UR16E": ([120, 120, 180, 180, 180, 180], [330, 330, 150, 54, 54, 54]),
}

#: The watchdog window, in milliseconds: ten `servoj` periods at the period this driver feeds.
#: Ten missed feeds is a dead policy rather than jitter, and §46.3's nesting wants the window
#: comfortably above the worst-case cycle — which, at 125 Hz, 80 ms is.
WATCHDOG_MS = 10 * SERVO_PERIOD_S * 1000.0

#: How long bring-up waits for the RTDE registers a closed session held, and how often it asks.
#:
#: The registers belong to one session at a time, and URSim 5.26 frees a cleanly closed session's
#: about 60 ms after its socket goes — measured three times in three, 2026-09-19. A second session
#: opened at once, which is what `skynet driver check` now does after its first (F76), was answered
#: `IN_USE` and refused, and this driver passed only because every `open` through the Python host
#: starts an interpreter first, which takes longer than the release. Two seconds is thirty times the
#: measurement and still a bounded refusal when another client really holds them.
REGISTER_RELEASE_S = 2.0
REGISTER_POLL_S = 0.05

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
        #: The e-Series model the operator declared (`UR5e`), canonicalised, or `None`. The
        #: ratings differ per model and the controller never says which it is, so without this
        #: there is no envelope — see `MODEL_RATINGS`.
        self._model: str | None = None
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
        # The stop's latch belongs to the session it was asserted in: `open` is a new one, and the
        # control program is uploaded afresh below. It used to outlive `close`, so a bridge reopened
        # after a stop refused every command for the life of the process (F76).
        self._estop = False
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
        # The model, canonicalised. "ur5e" and "UR5E" are the same arm; "UR5" is not "UR5e" —
        # CB3 speeds differ — so the normalisation is case and whitespace only, never a guess.
        raw_model = settings.get("model", "")
        model = str(raw_model).strip().upper() if raw_model else ""
        self._model = model if model in MODEL_RATINGS else None
        if raw_model and self._model is None:
            _say(
                f'model "{raw_model}" has no transcribed ratings (this driver knows '
                + ", ".join(sorted(MODEL_RATINGS))
                + "); the envelope is null. The arm still runs — position control is the same "
                "servoj on every model — but nothing here is clamped to its ratings."
            )
        elif not raw_model:
            _say(
                'no `model` in the configuration, so the envelope is null. Declare the arm — '
                '`{"host": ..., "model": "UR5e"}` — and the descriptor carries its published '
                "ratings with the citation beside them."
            )
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

        self._rtde = self._exchange(host, frequency)
        self._rtde.start()

        # The control program, once. Sent on the secondary interface because that is where a
        # program goes; everything cyclic afterwards is RTDE.
        self._script = socket.create_connection((host, 30002), timeout=5.0)
        self._script.sendall(SERVO_PROGRAM.encode())

        sample = self._await_sample(timeout_s=float(settings.get("startup_timeout_s", 10.0)))
        self._setpoint = list(sample["actual_q"])
        self._descriptor = self._build_descriptor(sample)

    def _exchange(self, host: str, frequency: float) -> Rtde:
        """An RTDE session with both recipes set, waiting out registers a closed session held.

        `IN_USE` is the one refusal retried, and only until `REGISTER_RELEASE_S`: a session this
        process closed a moment ago frees its registers in tens of milliseconds, while a variable
        this controller does not have, or another client that keeps them, answers `IN_USE` for the
        whole window and is refused with the same sentence it always was.
        """
        deadline = time.monotonic() + REGISTER_RELEASE_S
        while True:
            session = Rtde(host)
            try:
                session.negotiate(2)
                version = session.controller_version()
                session.setup_outputs(OUTPUTS, frequency)
                session.setup_inputs(INPUTS)
            except RegistersInUse:
                session.close()
                if time.monotonic() >= deadline:
                    raise
                time.sleep(REGISTER_POLL_S)
                continue
            except BaseException:
                session.close()
                raise
            _say(f"URControl {'.'.join(str(part) for part in version)} on {host}")
            return session

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

        out.stamp_ns = rhal.acquisition_ns()
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

        # The whole command is checked before any of it is stored (DRY-COVERAGE-PLAN.md F74). It
        # used to be stored joint by joint as the refusal was being decided, so the first joint of
        # a command refused on its second was kept — and went out, as that joint's "held" value,
        # with the next command that masked it.
        setpoint = list(self._setpoint)
        for index in range(6):
            applied = cmd.mask[index] if index < len(cmd.mask) else 1
            if not applied:
                continue  # hold: the previous setpoint stands, which is what the mask means
            value = cmd.values[index]
            if not math.isfinite(value):
                return rhal.SKR_EINVAL
            setpoint[index] = float(value)
        self._setpoint = setpoint
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
        ratings = MODEL_RATINGS.get(self._model or "")
        joints = []
        for index, (name, role) in enumerate(zip(JOINT_NAMES, JOINT_ROLES)):
            limits = {
                # Every UR joint is continuous over ±2π; the *software* limits are set in the
                # installation and are not published over RTDE, so what is declared is the
                # mechanical range and nothing narrower.
                "position_min_rad": -2.0 * math.pi,
                "position_max_rad": 2.0 * math.pi,
            }
            if ratings is not None:
                # The sheet speaks degrees per second and the descriptor speaks radians; the
                # conversion is in the open so the numbers beside it stay the sheet's.
                limits["velocity_max_rad_s"] = math.radians(ratings[0][index])
                limits["effort_max_nm"] = float(ratings[1][index])
            joints.append(
                {
                    "name": name,
                    "role": role,
                    "type": "continuous",
                    "limits": limits,
                    "control_modes": ["position"],
                }
            )
        envelope = None
        if ratings is not None and self._model is not None:
            speeds, torques = ratings
            envelope = {
                # The narrowest across the arm, in the kernel's tradition: the per-joint
                # figures above do the real clamping, and this is the fallback for a consumer
                # that reads only the envelope.
                "joint_velocity_max_rad_s": math.radians(min(speeds)),
                "joint_torque_max_nm": float(min(torques)),
                "watchdog_ms": WATCHDOG_MS,
                "source": (
                    f"Universal Robots {self._model} tech sheet, axis-movement table "
                    f"({self._model}_techsheet_en.pdf); joint torques from UR support article "
                    "'Max. joint torques CB3 and e-Series'; watchdog ten servoj periods per "
                    "driver design"
                ),
                # Transcribed from documentation, stated rather than defaulted (§5.3): the
                # default would say the same, but a reader of this file should see the kind.
                "basis": "declared",
            }
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
            # The transcribed ratings when the operator declared the model, null otherwise. An
            # earlier version of this driver refused to type datasheet figures in at all, citing
            # §63.4 — but §63.4's table refuses CAD design values precisely because they are not
            # datasheet safety ratings, and these, with the citation beside them, are.
            "envelope": envelope,
            # `emergency_stop` holds the control program, stops it at the dashboard, latches,
            # and refuses every later command — the suite covers all four — so the software stop
            # the driver actually has is the capability it declares. The *hardware* stop stays
            # undeclared for `emergency_stop`'s reason: no frame on any socket reaches the rated
            # circuit.
            "capabilities": ["safety.estop"],
            "extensions": [],
            "solvers": [],
        }
        del sample
        return json.dumps(descriptor, separators=(",", ":"))


def _say(message: str) -> None:
    import sys

    print(f"skynet-ur: {message}", file=sys.stderr, flush=True)

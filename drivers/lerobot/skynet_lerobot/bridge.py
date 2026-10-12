"""The LeRobot RHAL bridge — §16 and §67.

> **A LeRobot RHAL driver.** ``Robot.connect/get_observation/send_action`` is very nearly the
> simplest possible RHAL. Wrapping it means **every robot LeRobot supports becomes a Guest-tier
> Skynet robot on day one**, for the cost of one driver. This is the single fastest path from zero
> to a real device fleet, and it should be built first.

The mapping, from §67.1:

| RHAL | LeRobot |
|---|---|
| ``open(config)`` | ``robot.connect()`` |
| ``read_state`` | ``robot.get_observation()`` — proprioception split from camera frames |
| ``write_command`` | ``robot.send_action()`` |
| ``close`` | ``robot.disconnect()`` |
| ``describe`` | Built from ``observation_features`` / ``action_features`` plus the motor bus config |
| ``read_sensor`` | The camera entries of ``get_observation()`` |
| ``emergency_stop`` | Torque-disable on the motor bus |

# The ceiling, stated plainly

Python caps this at **Guest** (§67.3). The bridge cannot host an enforcing kernel and never claims
to. What it still delivers is most of why anyone would adopt early: the package system, skills with
enforced preconditions, capability grants, fleet reconciliation, the black box, episode capture, and
advisory violation reporting that tells an operator what a policy *would* have commanded before it
is trusted with anything.

# Four things this bridge does not know, and says so rather than inventing

1. **Joint ratings.** LeRobot does not publish position, velocity or effort limits through the
   ``Robot`` interface — they live in the motor bus configuration and the calibration file, in raw
   encoder units, not in radians and newton-metres. So ``joints[].limits`` is **empty**, and the
   transport planner reports every channel bound here as ``unscalable_channel`` rather than scaling
   against a guess. That is the correct outcome and it is worth understanding before it surprises
   somebody: a body with no ratings is a body a policy cannot be safely scaled onto.
2. **The safety envelope.** Null, always, for the same reason it is null everywhere else.
3. **Acquisition time.** ``get_observation()`` supplies none, so the bridge stamps at read (§67.1,
   §47.2 — the estimated-latency path, reported honestly). That timestamp is therefore *after*
   acquisition by an unknown amount, which inflates every downstream latency budget in the safe
   direction. The descriptor's provenance records it.
4. **Units.** LeRobot's ``.pos`` values are normalised or degrees depending on the robot and its
   calibration; the RHAL is radians. The bridge converts only when told the unit, and otherwise
   passes values through and **says in the descriptor that it did**. Silently treating degrees as
   radians is a 57× error on every command, and it is exactly the kind of thing that looks like a
   tuning problem for a day.

# Written from documentation, and now run against LeRobot

§62.2 is explicit that this is how these drivers get written: from public documentation, without
waiting for a vendor. It was exercised against a faithful fake (:mod:`skynet_lerobot.fake`) for a long
time and **not** against LeRobot itself, and the fake had the names and units the tables assumed, so it
could not show where they were wrong. ``selftest_wire.py`` is the run against the library: LeRobot's own
SO-101 class on a bus of emulated Feetech servos (``sim/feetech_bus.py``). It found that the real class
is called ``so_follower`` (F219), that its gripper is in percent while its arm is in degrees (F220), and
that :func:`open_bridge` could not open a robot at all (F221, F222). It is still a counterpart written here
on the servo side, not a servo, and the bridge is still Guest tier whatever it passes.
"""

from __future__ import annotations

import json
import math
import queue
import sys
import threading
import time
from typing import Any

import skynet_rhal as rhal
from skynet_rhal import Command, Frame, State

from .roles import ROBOT_DEFAULTS, roles_for

__all__ = ["LeRobotBridge", "open_bridge", "BridgeConfig", "NeedsConfiguration"]


class NeedsConfiguration(Exception):
    """A robot this bridge can describe only once a person says something it cannot know (`SKR_EINVAL`)."""

#: What a LeRobot feature key's suffix says about the control mode it belongs to.
_SUFFIX_MODE = {".pos": "position", ".vel": "velocity", ".effort": "torque", ".torque": "torque"}

_MODE_SUFFIX = {
    rhal.MODE_POSITION: ".pos",
    rhal.MODE_VELOCITY: ".vel",
    rhal.MODE_TORQUE: ".effort",
}

#: Units the bridge will convert from. Anything else is passed through unconverted and declared.
_UNIT_SCALE = {"rad": 1.0, "radians": 1.0, "deg": math.pi / 180.0, "degrees": math.pi / 180.0}

#: Units a joint may be *declared* in without being converted: LeRobot's `RANGE_0_100` (a percentage of the calibrated travel) and
#: `RANGE_M100_100` are normalisations of a travel the bridge is not told, so there is no honest factor to radians. The value goes
#: through as LeRobot gave it, and the descriptor says so for that joint.
_PASSTHROUGH_UNITS = ("percent", "normalised")

#: The register a servo family keeps its error bits in, by LeRobot's own control-table name: ROBOTIS's `Hardware_Error_Status` and Feetech's `Status`. A bus whose
#: every motor has one of them is a bus whose health the bridge can ask for (F232).
_HEALTH_REGISTERS = ("Hardware_Error_Status", "Status")
#: How often the servos are asked. A servo that has shut itself down on an overload lets go of its arm, so the answer is wanted within a tenth of a second; one extra
#: sync read of a byte is a fraction of a millisecond on the bus, so it is not asked more often than that.
_HEALTH_PERIOD_S = 0.1
#: The fault word's lower half is the servo's own error bits, verbatim. A servo whose torque is off while the bridge has not stopped it has no bit of its own, so it is
#: reported as this one, which neither family's error register uses (the largest is 0x20).
_TORQUE_OFF = 0x100
#: The fault bit for a servo that reads torque on after a stop was sent: the stop is one broadcast that waits for nobody (`_stop_torque`), so this is where a servo that did not take it is found.
_TORQUE_ON = 0x200
#: How long after a stop is sent the health poll gives the servos to take it before it holds them to it: a broadcast is on the wire when the call returns, and a servo applies it a moment after.
_STOP_SETTLE_S = 0.05
#: How long a call into a robot class that can block for good (`bounded` in `roles.ROBOT_DEFAULTS`) is given before the read or the write that made it fails with `SKR_ETIMEDOUT`: a tenth of a
#: second is ten periods of the 100 Hz stream Pollen's SDK asks its server for. And the stop's, which the RHAL wants back inside 50 ms and which must not wait on a robot that has gone quiet.
_CALL_DEADLINE_S = 0.1
_STOP_DEADLINE_S = 0.04
#: How long a read waits for the robot's own clock (`update_clock` in `roles.ROBOT_DEFAULTS`) to move before it calls the robot quiet and returns the state it had, with the stamp and the sequence it had.
_UPDATE_WAIT_S = 0.1


class Unanswered(Exception):
    """A call into the robot class that did not return in time, or could not be made because the one before it has not returned: the robot has gone quiet."""


class _Bounded:
    """Makes calls into a robot class on a thread of its own, so that one that never returns costs its caller the deadline and not the caller.

    Pollen's SDK makes blocking gRPC calls with no deadline of their own, and LeRobot's `Reachy2Robot` makes three of them in every `get_observation()`: against a core that has stopped answering (a
    cable out, a process frozen, a link that drops packets without resetting) the caller waits for TCP to give up, which is minutes (F330). The thread is the deadline.

    One call at a time, always: a call that is still inside the robot class when its deadline passes keeps the thread, and every call after it is refused at once until it returns, because two threads in one
    robot class are the race a gRPC stub or a serial bus is not built for. Its late result is dropped.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._busy = False
        self._work: queue.SimpleQueue = queue.SimpleQueue()
        threading.Thread(target=self._run, name="lerobot-bridge-calls", daemon=True).start()

    def call(self, function: Any, deadline_s: float) -> Any:
        with self._lock:
            if self._busy:
                raise Unanswered("is still waiting on the call before it: the robot has not answered it")
            self._busy = True
        box: dict[str, Any] = {}
        done = threading.Event()
        self._work.put((function, box, done))
        if not done.wait(deadline_s):
            raise Unanswered(f"did not return in {deadline_s * 1000:.0f} ms: the robot has not answered")
        if "error" in box:
            raise box["error"]
        return box.get("value")

    def stop(self) -> None:
        """The thread ends when the call it is in, if it is in one, returns."""
        self._work.put(None)

    def _run(self) -> None:
        while True:
            job = self._work.get()
            if job is None:
                return
            function, box, done = job
            try:
                box["value"] = function()
            except BaseException as error:  # noqa: BLE001 — whatever the robot class raised is the caller's to handle, on the caller's thread
                box["error"] = error
            finally:
                with self._lock:
                    self._busy = False
                done.set()


class BridgeConfig:
    """What the operator supplies that LeRobot does not.

    Parsed from the JSON string the OS hands to ``open()``. Every field is optional; an absent one
    means the bridge declares less rather than assuming more.
    """

    def __init__(self, raw: str | None = None) -> None:
        data: dict[str, Any] = {}
        if raw and raw.strip():
            try:
                parsed = json.loads(raw)
                if isinstance(parsed, dict):
                    data = parsed
            except json.JSONDecodeError as error:
                raise ValueError(
                    f"the driver configuration is not JSON: {error}. It is an object, and every "
                    'field in it is optional: {"robot_class": "so101_follower", "unit": "deg", '
                    '"role_overrides": {"shoulder_pan": "left_arm.shoulder.yaw"}}'
                ) from error

        #: Which mapping table to use. Defaults to the robot's own class name.
        self.robot_class: str | None = data.get("robot_class")
        #: Motor name to canonical role, overriding the table. This is how a bimanual rig fixes the
        #: side that §67.2's table assumes.
        self.role_overrides: dict[str, str] = dict(data.get("role_overrides") or {})
        #: The unit LeRobot's position values are in. Absent means "unknown", and the bridge then
        #: passes values through and declares that it did.
        self.unit: str | None = data.get("unit")
        #: Per-motor units, over `unit`. A joint named here in `percent` or `normalised` is passed through unconverted and declared as
        #: such; one named in `deg`/`rad` is converted with that unit whatever `unit` says. A body with a gripper in percent and an arm
        #: in degrees (LeRobot's SO class) needs this: one `unit` for the whole body mis-scales one of the two.
        self.units: dict[str, str] = dict(data.get("units") or {})
        #: Action features that are not joints, each with the value that leaves it still. LeRobot's
        #: `send_action` wants every feature it declared (Reachy 2's mobile base velocities), the bridge has
        #: no joint to drive them with, and sending nothing is a `KeyError`. Zero is right for a velocity and a
        #: guess for anything else, so it is written down per robot class (`roles.ROBOT_DEFAULTS`) or by the
        #: operator here, and a feature with neither refuses the robot at `open`.
        self.aux_actions: dict[str, float] = {k: float(v) for k, v in dict(data.get("aux_actions") or {}).items()}
        #: Slug for the descriptor. Defaults to the robot class.
        self.robot_slug: str | None = data.get("robot_slug")
        #: Whether to call `robot.calibrate()` semantics via connect(calibrate=True).
        self.calibrate: bool = bool(data.get("calibrate", True))
        #: Whether the body is simulated. The fake implements the same `Robot` interface as
        #: hardware, so the bridge cannot tell them apart — the operator does, and §35.3's
        #: direction of safety makes the default `True`: declaring simulated about real caps
        #: the tier, and the reverse presents sim as evidence.
        self.simulator: bool = bool(data.get("simulator", True))

        for motor, unit in self.units.items():
            if unit not in _UNIT_SCALE and unit not in _PASSTHROUGH_UNITS:
                raise ValueError(
                    f"units[{motor!r}] is {unit!r}, which is not one this bridge knows. Convertible: "
                    f"{', '.join(sorted(_UNIT_SCALE))}. Passed through and declared: {', '.join(_PASSTHROUGH_UNITS)}."
                )
        if self.unit is not None and self.unit not in _UNIT_SCALE:
            raise ValueError(
                f"unit {self.unit!r} is not one this bridge converts. Known: "
                f"{', '.join(sorted(_UNIT_SCALE))}. Leave it out to pass values through unconverted "
                "— which is declared in the descriptor rather than assumed."
            )


class LeRobotBridge:
    """A :class:`skynet_rhal.Driver` over any object with LeRobot's ``Robot`` interface."""

    def __init__(self, robot: Any, config: BridgeConfig | None = None) -> None:
        self._robot = robot
        self._config = config or BridgeConfig()
        self._opened = False
        self._estopped = False
        self._seq = 0
        self._descriptor: str | None = None

        #: Motor names, in the order they appear in `action_features` — which is the order the
        #: RHAL's joint arrays use, and the order `describe()` lists them in. One order, decided
        #: once, or a joint index means two different things on two code paths.
        self._motors: list[str] = []
        self._modes: list[str] = []
        self._roles: dict[str, str | None] = {}
        self._role_note = ""
        self._cameras: list[str] = []
        self._aux: dict[str, float] = {}
        #: Per motor, the factor to radians (1.0 where the value is passed through) and the unit it was declared in.
        self._scales: list[float] = []
        self._unit_names: list[str] = []
        self._last_position: list[float | None] = []
        self._setpoint: list[float | None] = []
        self._hold: list[float | None] = []
        self._last_frames: dict[str, Frame] = {}
        #: The servos' own health (F232): the name of the register their error bits are in, or `None` when this robot class has no bus the bridge can ask. And whether
        #: their torque is held to be on: a servo that lets go by itself has no bit of its own, and an arm that is limp while the bridge believes it is held is the
        #: failure that looks like success.
        self._robot_class = ""
        #: For a robot class whose calls can block for good: the thread they are made on, with a deadline (F330). `None` for one whose calls cannot (a serial bus times out by itself).
        self._bounded: _Bounded | None = None
        #: For a robot class that keeps a clock of its own, a function of the robot that returns a number that moves when its state does (F331); `None` for one that reads its robot at the call.
        self._update_clock: Any = None
        self._last_clock: int | None = None
        self._clock_stamp = 0
        self._health: str | None = None
        self._watch_torque = False
        self._health_next = 0.0
        #: When the last stop was sent, on the monotonic clock: the health poll holds the servos to it once `_STOP_SETTLE_S` has passed.
        self._stopped_at = 0.0
        #: `(servo id, error bits)` of the first servo in trouble at the last poll, or `None`.
        self._fault: tuple[int, int] | None = None
        #: What the last poll saw when it differed from `_fault`, until a second poll sees the same: `()` for "every servo reads clear". A reading is a fault, or the end of one, when the servos say the same thing twice
        #: (see `_poll_health`); what each entry point last said is in `_said`.
        self._candidate: tuple | None = None
        self._said: dict[str, str] = {}

    # ── Lifecycle ────────────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            self._config = BridgeConfig(config)
        except ValueError as error:
            print(f"lerobot-bridge: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_EINVAL

        try:
            if not getattr(self._robot, "is_connected", False):
                self._robot.connect()
        except Exception as error:  # noqa: BLE001 — a bus that did not answer is not our bug
            print(f"lerobot-bridge: connect() failed: {error!r}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO

        try:
            self._introspect()
        except NeedsConfiguration as error:
            print(f"lerobot-bridge: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_EINVAL
        except Exception as error:  # noqa: BLE001
            print(
                f"lerobot-bridge: could not read this robot's feature description: {error!r}. "
                "The bridge builds the embodiment descriptor from `action_features` and "
                "`observation_features`; a robot class that does not expose them needs an adapter.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EPROTO

        if not self._motors:
            print(
                "lerobot-bridge: this robot declares no actuated features, so there is no body to "
                "describe. `action_features` was empty.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_ENODEV

        self._fault, self._health_next = None, 0.0
        self._candidate, self._said = None, {}
        self._health = self._find_health()
        self._watch_torque = False
        if self._health is not None:
            try:
                self._watch_torque = self._torque_is_on()
            except Exception as error:  # noqa: BLE001 — a bus that would not answer a one-byte read is not one to open
                print(f"lerobot-bridge: could not read the servos' torque at open: {error!r}", file=sys.stderr, flush=True)
                return rhal.SKR_EIO
        # The descriptor says whether the servos' health is read, which is only known now.
        self._descriptor = self._build_descriptor(self._robot_class)

        self._opened = True
        self._estopped = False
        # No reading yet: `None`, never zero, so a hold before the first read is refused rather
        # than sent as a command to go to zero radians.
        self._last_position = [None] * len(self._motors)
        # §24's hold, per joint: the last position setpoint sent, and the latched hold target.
        self._setpoint = [None] * len(self._motors)
        self._hold = [None] * len(self._motors)
        return rhal.SKR_OK

    def close(self) -> int:
        self._opened = False
        try:
            self._robot.disconnect()
        except Exception as error:  # noqa: BLE001
            print(f"lerobot-bridge: disconnect() failed: {error!r}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        finally:
            if self._bounded is not None:
                self._bounded.stop()   # a call that is stuck ends with the disconnect; a session opened again starts a thread of its own
                self._bounded = None
        return rhal.SKR_OK

    def _introspect(self) -> None:
        """Work out the body from what LeRobot declares about itself."""
        # A session's body is worked out afresh. `open()` after `close()` on the same bridge appended the same
        # motors again, so a reopened bridge described twelve joints for a six-joint arm and refused every command
        # for a count that no longer matched (F188, found by the seeded-defect run's reopen check).
        self._motors, self._modes, self._cameras, self._aux = [], [], [], {}
        self._last_frames = {}
        action = _features(self._robot, "action_features")
        observation = _features(self._robot, "observation_features")

        for key in action:
            motor, suffix = _split_feature(key)
            if suffix is None:
                continue
            self._motors.append(motor)
            self._modes.append(_SUFFIX_MODE[suffix])

        # Cameras are the observation entries that are not scalar motor readings. LeRobot describes
        # them with a shape tuple; anything with a shape is a frame, anything without is a number.
        for key, spec in observation.items():
            _, suffix = _split_feature(key)
            if suffix is not None:
                continue
            if isinstance(spec, tuple) or isinstance(spec, list):
                self._cameras.append(key)

        robot_class = (
            self._config.robot_class
            or getattr(self._robot, "robot_type", None)
            or type(self._robot).__name__
        )
        defaults = ROBOT_DEFAULTS.get(str(robot_class).lower(), {})
        if defaults.get("bounded") and self._bounded is None:
            self._bounded = _Bounded()   # one thread for the life of the bridge: a session opened again does not start another
        elif not defaults.get("bounded"):
            self._bounded = None
        self._update_clock = defaults.get("update_clock")
        self._last_clock, self._clock_stamp = None, 0
        # A class default is an assumption about how the robot was configured, so it yields when the robot says otherwise: the SO
        # class reports degrees only while `use_degrees` is on, and with it off its joints are normalised to -100..100.
        degrees_off = getattr(getattr(self._robot, "config", None), "use_degrees", True) is False
        if self._config.unit is None and defaults.get("unit") and not (defaults["unit"] == "deg" and degrees_off):
            self._config.unit = defaults["unit"]
        if degrees_off and defaults.get("unit") == "deg":
            for motor in self._motors:
                self._config.units.setdefault(motor, "normalised")
        else:
            for motor, unit in (defaults.get("units") or {}).items():
                self._config.units.setdefault(motor, unit)
        aux_wanted = [key for key in action if _split_feature(key)[1] is None]
        self._aux = {}
        for key in aux_wanted:
            if key in self._config.aux_actions:
                self._aux[key] = self._config.aux_actions[key]
            elif key in defaults.get("aux_actions", {}):
                self._aux[key] = float(defaults["aux_actions"][key])
            else:
                raise NeedsConfiguration(
                    f"this robot's `send_action` takes `{key}`, which is not a joint, and the bridge has no value that "
                    f"leaves it still. Say what it is in the configuration, e.g. {{\"aux_actions\": {{\"{key}\": 0.0}}}}, "
                    "if zero is still for it — the bridge will not guess that for a feature it has not been told about."
                )
        self._unit_names = [self._config.units.get(motor) or self._config.unit or "" for motor in self._motors]
        self._scales = [_UNIT_SCALE.get(name, 1.0) for name in self._unit_names]
        self._roles, self._role_note = roles_for(
            str(robot_class), self._motors, self._config.role_overrides
        )
        self._robot_class = str(robot_class)
        self._descriptor = self._build_descriptor(self._robot_class)

    # ── The servos' own health (F232) ────────────────────────────────────────

    def _health_bus(self) -> Any | None:
        """The robot's motor bus when it is one this bridge can ask a register of: LeRobot's `MotorsBus`, which has the motors, their control tables and `sync_read`."""
        bus = getattr(self._robot, "bus", None)
        motors = getattr(bus, "motors", None)
        tables = getattr(bus, "model_ctrl_table", None)
        if bus is None or not hasattr(bus, "sync_read") or not isinstance(motors, dict) or not motors or not isinstance(tables, dict):
            return None
        return bus

    def _find_health(self) -> str | None:
        bus = self._health_bus()
        if bus is None:
            return None
        for register in _HEALTH_REGISTERS:
            if all(register in bus.model_ctrl_table.get(motor.model, {}) for motor in bus.motors.values()):
                return register
        return None

    def _torque_is_on(self) -> bool:
        """Whether every servo has its torque on now. Held to be so from here only if it is at open: a robot class that leaves torque off is not faulted for it."""
        bus = self._health_bus()
        if bus is None or not all("Torque_Enable" in bus.model_ctrl_table.get(motor.model, {}) for motor in bus.motors.values()):
            return False
        return all(int(value) == 1 for value in bus.sync_read("Torque_Enable", normalize=False).values())

    def _set_torque(self, bus: Any, *, on: bool) -> None:
        """Ask every servo to hold (`on`) or to let go, trying all of them whatever one of them says, and raise naming the ones that did not.

        LeRobot's `disable_torque()` and `enable_torque()` write one servo after another and raise at the first reply that carries a hardware alert. On a bus where one
        servo has shut itself down that is the stop aborting at that servo, with the rest of the arm still under power, at the one time a stop matters most (F232). The write
        has been carried out by then, so the answer is the register: after every servo has been tried the torque of all of them is read back, and a servo is reported only
        if it does not read as asked."""
        call = bus.enable_torque if on else bus.disable_torque
        motors = getattr(bus, "motors", None)
        if (
            not isinstance(motors, dict)
            or not motors
            or self._health_bus() is None
            or not all("Torque_Enable" in bus.model_ctrl_table.get(motor.model, {}) for motor in motors.values())
        ):
            call()  # a bus the bridge cannot enumerate or read back keeps its own all-at-once call, and its own failure
            return
        refused: dict[str, str] = {}
        for name in motors:
            try:
                call(name)
            except Exception as error:  # noqa: BLE001 — read back below: a reply with the alert flag is an error here though the write was made
                refused[name] = repr(error)
        wanted = 1 if on else 0
        try:
            held = bus.sync_read("Torque_Enable", normalize=False)
        except Exception as error:  # noqa: BLE001
            raise RuntimeError(f"torque was asked for on every servo and could not be read back: {error!r}") from error
        wrong = {name: f"reads {value}" for name, value in held.items() if int(value) != wanted}
        failed = {name: refused.get(name, why) for name, why in wrong.items()}
        if failed:
            raise RuntimeError(
                f"torque did not {'return' if on else 'go off'} on " + ", ".join(f"{name} ({why})" for name, why in sorted(failed.items())) + "."
            )

    def _say_once(self, entry: str, message: str) -> None:
        """Say `message` unless this entry point's last words were the same: a failure that goes on is one sentence, and a different cause, or the same one after the entry point worked, is said again."""
        if self._said.get(entry) != message:
            self._said[entry] = message
            print(message, file=sys.stderr, flush=True)

    def _call(self, function: Any, deadline_s: float) -> Any:
        """A call into the robot class: made here for a class whose calls cannot block, and on the bridge's own thread, with a deadline, for one whose calls can (F330). Raises `Unanswered` when the deadline passes."""
        if self._bounded is None:
            return function()
        return self._bounded.call(function, deadline_s)

    def _wait_for_update(self) -> int:
        """The robot's own clock, once it has moved since the last read; or the same value again when it has not moved in `_UPDATE_WAIT_S`, which is the robot gone quiet (F331)."""
        clock = int(self._update_clock(self._robot))
        deadline = time.monotonic() + _UPDATE_WAIT_S
        while clock == self._last_clock and time.monotonic() < deadline:
            time.sleep(0.002)
            clock = int(self._update_clock(self._robot))
        return clock

    def _stop_torque(self, bus: Any) -> None:
        """Torque off on every servo, in one broadcast `SYNC_WRITE` that waits for nobody.

        The stop used to be `_set_torque`: one write per servo, each waiting for its reply, and then every servo read back. Against a line that carried nothing that was the SDK's receive timeout seven times over (351.8 ms
        measured by the link-chaos contract, against the 50 ms the RHAL's "never blocks" wants), and it held the stop while it waited. A broadcast has no replies to wait for, so it costs a dead line what it costs a live one, it
        reaches all of the servos in one frame whatever one of them says (F232's concern, which the per-servo version answered by trying them all and reading back), and what became of it is for the next health poll to say.
        A bus that cannot be enumerated or has no `sync_write` keeps `_set_torque`, and its own failure."""
        motors = getattr(bus, "motors", None)
        if (
            isinstance(motors, dict)
            and motors
            and callable(getattr(bus, "sync_write", None))
            and all("Torque_Enable" in bus.model_ctrl_table.get(motor.model, {}) for motor in motors.values())
        ):
            bus.sync_write("Torque_Enable", {name: 0 for name in motors}, normalize=False)
            return
        self._set_torque(bus, on=False)

    def _poll_health(self) -> int:
        """Ask the servos how they are, at most every `_HEALTH_PERIOD_S`: `SKR_OK`, or `SKR_EIO` when the bus would not say.

        LeRobot's observation is positions only, and its bus code reads the error byte of every reply and throws it away, so a servo that has shut itself down on an
        overload (which ROBOTIS's factory Shutdown register tells it to do, and which lets the arm fall) reads as a healthy one that is not following its goal. The
        register is asked for directly, and any bit in it is a fault: an unlisted one is not read as less than a listed one."""
        now = time.monotonic()
        if self._health is None or now < self._health_next:
            return rhal.SKR_OK
        bus = self._health_bus()
        if bus is None:
            return rhal.SKR_OK
        # Torque is read in both directions: held to be on while the bridge has not stopped, and, once a stop was sent and the servos have had a moment to take it, held to be off (the stop waits for nobody, so this is
        # where a servo that did not take it is found).
        settled = self._estopped and now - self._stopped_at >= _STOP_SETTLE_S
        try:
            errors = bus.sync_read(self._health, normalize=False)
            torque = bus.sync_read("Torque_Enable", normalize=False) if (self._watch_torque and not self._estopped) or settled else {}
        except Exception as error:  # noqa: BLE001
            self._say_once("health", f"lerobot-bridge: the servos' health could not be read: {error!r}")
            return rhal.SKR_EIO
        self._said.pop("health", None)
        self._health_next = now + _HEALTH_PERIOD_S
        fault = None
        for name, motor in bus.motors.items():
            bits = int(errors.get(name, 0))
            if bits:
                fault = (motor.id, bits)
                break
            if torque.get(name) is not None and settled and int(torque[name]) == 1:
                fault = (motor.id, _TORQUE_ON)
                break
            if torque.get(name) is not None and not self._estopped and int(torque[name]) == 0:
                fault = (motor.id, _TORQUE_OFF)
                break
        # A reading is a fault, or the end of one, when the servos say the same thing twice, a period apart. A servo's reply to the poll is matched to the request by its id and a checksum and by nothing that says which register
        # it answers, so a late reply to an earlier request (a position, on a line that stalled or lost a frame) is taken for the answer and reads as error bits that are different every time: 0x06, 0xfd, 0xf9 in one run of
        # the link-chaos contract (12.C.4), each a FAULT and every command refused while it stood. A real fault does not change between two looks.
        seen = fault if fault is not None else ()
        if fault == self._fault:
            self._candidate = None
        elif seen != self._candidate:
            self._candidate = seen
            fault = self._fault
        else:
            self._candidate = None
        if fault != self._fault:
            print(
                "lerobot-bridge: " + (f"servo {fault[0]} {_fault_words(fault[1])}." if fault else "every servo reports no error and holds its torque again."),
                file=sys.stderr,
                flush=True,
            )
        self._fault = fault
        if self._estopped and fault is not None and fault[1] == _TORQUE_ON:
            try:
                self._stop_torque(bus)  # the stop is sent again, every health period, until every servo reads torque off: the broadcast is idempotent
            except Exception as error:  # noqa: BLE001
                print(f"lerobot-bridge: the stop could not be sent again: {error!r}", file=sys.stderr, flush=True)
        return rhal.SKR_OK

    # ── The real-time-ish path ───────────────────────────────────────────────

    def read_state(self, out: State, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM

        # A robot class with a clock of its own (F331): its state is a cache a stream fills, so a stream that has stalled leaves a cache that reads as well as it ever did. Wait, no longer than the window, for the
        # clock to move; a read that is still looking at the clock it last saw says so by returning the state it had, with the stamp and the sequence it had (§24: a repeated sequence is a stalled producer).
        try:
            clock = self._wait_for_update() if self._update_clock is not None else None
        except Exception as error:  # noqa: BLE001
            self._say_once("read", f"lerobot-bridge: the robot's own clock could not be read: {error!r}")
            return rhal.SKR_EIO
        try:
            observation = self._call(self._robot.get_observation, _CALL_DEADLINE_S)
        except Unanswered as error:
            self._say_once("read", f"lerobot-bridge: get_observation() {error}.")
            return rhal.SKR_ETIMEDOUT
        except Exception as error:  # noqa: BLE001
            # Once per run of the same cause: a loop at 50 Hz said it fifty times a second (53 identical lines in one chaos run).
            self._say_once("read", f"lerobot-bridge: get_observation() failed: {error!r}")
            return rhal.SKR_EIO
        self._said.pop("read", None)
        if not isinstance(observation, dict):
            return rhal.SKR_EPROTO
        health = self._poll_health()
        if health != rhal.SKR_OK:
            return health

        if clock is None:
            # §67.1: LeRobot does not supply acquisition times, so the stamp is taken at read. It is
            # therefore *later* than acquisition by an unknown amount, which inflates every downstream
            # latency budget — in the safe direction, and the descriptor says so.
            stamp = rhal.acquisition_ns()
            self._seq += 1
        else:
            # The robot's clock moved: this is a state the last read did not return, and the first time it is seen is the nearest this bridge gets to when it arrived. It did not move: the
            # state is the last one again, under its stamp and its sequence.
            if clock != self._last_clock:
                self._last_clock, self._clock_stamp = clock, rhal.acquisition_ns()
                self._seq += 1
            stamp = self._clock_stamp

        positions: list[float] = []
        velocities: list[float] = []
        efforts: list[float] = []

        for index, motor in enumerate(self._motors):
            scale = self._scales[index]
            positions.append(_number(observation.get(f"{motor}.pos")) * scale)
            # NaN where the robot does not report it. Zero would be a measurement, and this is not
            # one — the distinction is the whole reason the RHAL has three arrays instead of one.
            velocities.append(_optional(observation.get(f"{motor}.vel"), scale))
            efforts.append(_optional(observation.get(f"{motor}.effort")))

        self._last_position = list(positions)

        # Camera frames are kept for read_sensor rather than pushed into the state, which is the
        # split §67.1 asks for: proprioception to `rhal_state`, images to `read_sensor`.
        for name in self._cameras:
            value = observation.get(name)
            if value is not None:
                self._last_frames[name] = _to_frame(value, stamp)

        out.stamp_ns = stamp
        out.seq = self._seq
        out.flags = rhal.FLAG_ESTOP if self._estopped else 0
        # §24's homing field, and this bridge is the one place in the driver set where the answer
        # is genuinely unknown. LeRobot's robot classes expose no homing or referencing state
        # through this interface: a follower arm may be at its calibrated zero or may have been
        # power-cycled with the leader somewhere else, and nothing in the observation says which.
        # Reporting `complete` would be inventing it and reporting `not_required` would be worse —
        # these are relative encoders. So the field is left `unreported`, which is the state §24
        # added it to be able to express, and the robot is held.
        # §24: a servo in trouble is FAULT, and DEGRADED with it because §27 reads that bit and not FAULT. The word is `servo id << 16 | the servo's own error bits`, the
        # bits verbatim, so the black box holds what the maker's register said and not a count.
        if self._fault is not None:
            out.flags |= rhal.FLAG_FAULT | rhal.FLAG_DEGRADED
            out.fault_code = (self._fault[0] << 16) | self._fault[1]
        else:
            out.fault_code = 0
        out.position_rad = positions
        out.velocity_rad_s = velocities
        out.effort_nm = efforts
        out.base_pose = [math.nan] * 7  # LeRobot does not localise the base through this interface
        out.base_twist = [0.0] * 6
        return rhal.SKR_OK

    def write_command(self, cmd: Command, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        if cmd.joint_count != len(self._motors) or len(cmd.mask) != cmd.joint_count:
            return rhal.SKR_EINVAL
        if cmd.mode not in _MODE_SUFFIX:
            return rhal.SKR_EINVAL
        if self._estopped:
            return rhal.SKR_EPERM
        if self._fault is not None:
            self._say_once(
                "refused",
                f"lerobot-bridge: refused: servo {self._fault[0]} {_fault_words(self._fault[1])}. A servo in that state ignores a goal, so answering the command "
                "with success would report a move the arm will not make. Clear the cause and reboot the servo; the next read lifts this.",
            )
            return rhal.SKR_EPERM

        suffix = _MODE_SUFFIX[cmd.mode]
        wanted = rhal.MODE_NAMES[cmd.mode]

        # A mode this body does not accept is refused rather than approximated. Sending a position
        # to a joint expecting a velocity is not a degraded command, it is a different one.
        for index, motor in enumerate(self._motors):
            if self._modes[index] != wanted:
                return rhal.SKR_EINVAL

        # A masked joint must *hold*, and holding has a correct realisation in position and velocity
        # and none in torque: holding a torque-controlled joint means gravity compensation, which
        # this bridge cannot compute. Refusing is the only honest answer — commanding zero torque
        # would drop the arm, which is the exact failure the mask exists to prevent.
        if cmd.mode == rhal.MODE_TORQUE and not all(cmd.mask):
            print(
                "lerobot-bridge: a torque command masks a joint, and 'hold' has no correct value "
                "in torque — zero is limp, not held. Refused rather than approximated.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EINVAL

        # §24: a masked joint is held at its current setpoint. In position that is the target
        # latched when the hold began — the last position setpoint sent, or the last reading when
        # there is none — and not re-read: commanding a servo to wherever it reads each cycle
        # zeroes its position error, and a joint carrying load then creeps downhill one read at a
        # time (DRY-COVERAGE-PLAN.md F121, carried as 8.13). Resolved on copies and committed only
        # once the action is sent.
        hold = list(self._hold)
        action: dict[str, float] = {}
        for index, motor in enumerate(self._motors):
            inverse = 1.0 / self._scales[index]
            value = cmd.values[index]
            if not math.isfinite(value):
                return rhal.SKR_EINVAL
            if cmd.mask[index]:
                action[f"{motor}{suffix}"] = value * inverse
                hold[index] = None
            elif cmd.mode == rhal.MODE_POSITION:
                if hold[index] is None:
                    hold[index] = (
                        self._setpoint[index]
                        if self._setpoint[index] is not None
                        else self._last_position[index]
                    )
                if hold[index] is None or not math.isfinite(hold[index]):
                    print(
                        f"lerobot-bridge: {motor} is masked, and there is nothing to hold it at: "
                        "no position was commanded and none has been read that is a number. "
                        "Read state before commanding a hold.",
                        file=sys.stderr,
                        flush=True,
                    )
                    return rhal.SKR_EIO
                action[f"{motor}{suffix}"] = hold[index] * inverse
            else:
                action[f"{motor}{suffix}"] = 0.0  # hold, in velocity, is zero velocity

        # The features that are not joints, each at the value that leaves it still (see `BridgeConfig.aux_actions`).
        action.update(self._aux)
        try:
            self._call(lambda: self._robot.send_action(action), _CALL_DEADLINE_S)
        except Unanswered as error:
            # Whether the action was taken is not known: it may still be inside the robot class. The holds are not committed, as for any send that did not succeed.
            self._say_once("write", f"lerobot-bridge: send_action() {error}.")
            return rhal.SKR_ETIMEDOUT
        except Exception as error:  # noqa: BLE001
            self._say_once("write", f"lerobot-bridge: send_action() failed: {error!r}")
            return rhal.SKR_EIO
        self._said.pop("write", None)
        self._said.pop("refused", None)
        self._hold = hold
        if cmd.mode == rhal.MODE_POSITION:
            for index in range(len(self._motors)):
                if cmd.mask[index]:
                    self._setpoint[index] = float(cmd.values[index])
        else:
            self._setpoint = [None] * len(self._motors)
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        """Torque-disable on the motor bus (§67.1), latched.

        The latch is what makes the stop observable in the next ``read_state``, which is the half of
        ``safety.estop.latency`` that can be measured without hardware. The *latency* is a bench
        measurement and this driver does not claim one.
        """
        self._estopped = True
        self._stopped_at = time.monotonic()
        bus = getattr(self._robot, "bus", None)
        try:
            if bus is not None and hasattr(bus, "disable_torque"):
                self._stop_torque(bus)
            elif hasattr(self._robot, "disable_torque"):
                self._robot.disable_torque()
            else:
                # No torque-disable on this robot class. Cutting torque on a humanoid's arms would drop them
                # in any case, so the stop is a *freeze*: every joint is told to stay where it was last read
                # and every non-joint feature to stay still, then the latch refuses everything after. Nothing
                # was cut, and saying so is the point: an operator reading the black box must not conclude
                # that power was removed.
                frozen = self._freeze()
                print(
                    "lerobot-bridge: this robot exposes no torque-disable, so the stop is "
                    + ("a freeze in place (every joint told to stay where it reads) and " if frozen else "")
                    + "a software latch. Commands are refused from here; the drives were not cut."
                    + ("" if frozen else " The freeze itself could not be sent."),
                    file=sys.stderr,
                    flush=True,
                )
        except Exception as error:  # noqa: BLE001
            print(f"lerobot-bridge: torque-disable failed: {error!r}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        return rhal.SKR_OK

    def _freeze(self) -> bool:
        """The freeze, within the time the stop has: on a robot that has gone quiet it is not made, and the latch is all there is (F330)."""
        try:
            return bool(self._call(self._freeze_now, _STOP_DEADLINE_S))
        except Unanswered as error:
            print(f"lerobot-bridge: the freeze {error}.", file=sys.stderr, flush=True)
            return False

    def _freeze_now(self) -> bool:
        """Tell every joint to stay where it reads and every non-joint feature to stay still. Best effort.

        A position joint's target is its own reading, a velocity joint's is zero. A torque joint has no
        correct answer (zero is limp, not held), so a body with one is not frozen and the caller says so.
        """
        try:
            observation = self._robot.get_observation()
            action: dict[str, float] = {}
            for motor, mode in zip(self._motors, self._modes):
                if mode == "position":
                    value = _number(observation.get(f"{motor}.pos"))
                    if not math.isfinite(value):
                        return False
                    action[f"{motor}.pos"] = value
                elif mode == "velocity":
                    action[f"{motor}.vel"] = 0.0
                else:
                    return False
            action.update(self._aux)
            self._robot.send_action(action)
        except Exception as error:  # noqa: BLE001
            print(f"lerobot-bridge: the freeze failed: {error!r}", file=sys.stderr, flush=True)
            return False
        return True

    def release_stop(self) -> int:
        """Re-enable torque on the motor bus and clear the latch (§48.2).

        The mirror of the stop: the bus or the robot gets ``enable_torque`` where it
        got ``disable_torque``, and a robot class with neither keeps the software
        latch only, said aloud. An enable that fails re-latches and answers EIO —
        the drives never returned, the body is not commandable, and a cleared latch
        would claim otherwise. Nothing resumes: torque returns, motion waits for the
        next command. Idempotent.
        """
        self._estopped = False
        self._health_next = 0.0  # the servos are asked afresh: their torque was off for the stop, and is what is held to be on again
        bus = getattr(self._robot, "bus", None)
        try:
            if bus is not None and hasattr(bus, "enable_torque"):
                self._set_torque(bus, on=True)
            elif hasattr(self._robot, "enable_torque"):
                self._robot.enable_torque()
            else:
                print(
                    "lerobot-bridge: this robot exposes no torque-enable, so the release is a "
                    "software latch only. Commands are accepted from here; the drives were never "
                    "cut, and were never returned.",
                    file=sys.stderr,
                    flush=True,
                )
        except Exception as error:  # noqa: BLE001
            print(f"lerobot-bridge: torque-enable failed: {error!r}", file=sys.stderr, flush=True)
            self._estopped = True
            return rhal.SKR_EIO
        return rhal.SKR_OK

    def read_sensor(self, name: str, out: Frame) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        if name not in self._cameras:
            return rhal.SKR_ENODEV
        frame = self._last_frames.get(name)
        if frame is None:
            # Read but never populated: the camera is declared and has produced nothing yet.
            return rhal.SKR_EIO
        out.stamp_ns = frame.stamp_ns
        out.width = frame.width
        out.height = frame.height
        out.stride = frame.stride
        out.format = frame.format
        out.data = frame.data
        return rhal.SKR_OK

    # ── The body description ─────────────────────────────────────────────────

    def describe(self) -> str:
        if self._descriptor is None:
            raise RuntimeError("describe() before open()")
        return self._descriptor

    def _build_descriptor(self, robot_class: str) -> str:
        joints = []
        for index, motor in enumerate(self._motors):
            joints.append(
                {
                    "name": motor,
                    "role": self._roles.get(motor),
                    "type": "revolute",
                    # Empty, and this is the honest state: LeRobot does not publish joint ratings
                    # through the Robot interface. They live in the motor bus configuration and the
                    # calibration file, in raw encoder units. A channel bound here is reported
                    # `unscalable_channel` rather than scaled against a guess.
                    "limits": {},
                    "control_modes": [self._modes[index]],
                }
            )

        sensors = [
            {
                "name": name,
                "role": None,
                "modality": "rgb",
                "width": None,
                "height": None,
                "fps": None,
                "frame": None,
            }
            for name in self._cameras
        ]

        modes = sorted(set(self._modes))
        unit = self._config.unit
        descriptor = {
            "schema_version": 1,
            "robot_slug": self._config.robot_slug or _slug(robot_class),
            "digest": None,
            "verified": False,
            "simulator": self._config.simulator,
            # LeRobot's Robot interface says nothing about how the body is carried. `fixed` is the
            # conservative reading — it makes base-velocity channels refuse rather than command
            # motion — and LeKiwi-style bases override it through configuration.
            "base": "fixed",
            "dof": len(self._motors),
            "joints": joints,
            # LeRobot has no notion of an end effector: a gripper is a motor like any other. A
            # policy's `gripper` channel binds to an end effector, so it is refused until a person
            # declares one with its kind, dof and grip force.
            "end_effectors": [],
            "sensors": sensors,
            "control": {
                "modes": modes,
                "rate_hz_max": None,
                "rate_hz_min": None,
                # The number that decides whether a policy trained at 50 Hz is stable on this body
                # (§5.2). LeRobot does not publish it and this bridge has not measured it.
                "command_latency_ms": None,
            },
            "envelope": None,
            # `emergency_stop` torque-disables on the motor bus where the bus offers it, latches,
            # and refuses every later command — the software stop the driver actually has, in the
            # tradition `drivers/ur_rtde` sets: declared, while the rated stop stays undeclared.
            "capabilities": ["safety.estop"],
            "solvers": [],
            "provenance": {
                "tool": "skynet-lerobot bridge",
                "driver_ceiling": "guest",
                "ceiling_reason": (
                    "Python. The GIL and the garbage collector cannot hold a real-time path "
                    "(§31.3, §67.3), so this driver is an observer with a body attached: no "
                    "enforcing kernel, and it never claims one."
                ),
                "robot_class": robot_class,
                "roles": self._role_note,
                "limits": (
                    "Not declared. LeRobot does not publish joint ratings through the Robot "
                    "interface; they live in the motor bus configuration and the calibration file "
                    "in raw encoder units. Every transport channel bound to this body is therefore "
                    "reported unscalable rather than scaled against a guess."
                ),
                "envelope": (
                    "Not declared. A sourced safety figure comes from the vendor's documentation or "
                    "a risk assessment, and this bridge has neither."
                ),
                "timestamps": (
                    "Stamped when the bridge first saw each state the robot's own clock reported "
                    "(the clock moved, F331), and a state it sees again is returned under the same stamp "
                    "and the same sequence: the stamp is after acquisition by the stream's latency, which "
                    "inflates every latency budget in the safe direction, and a repeated sequence is a "
                    "stalled producer (§24)."
                    if self._update_clock is not None
                    else "Stamped at read, not at acquisition: LeRobot supplies no acquisition time "
                    "(§67.1, §47.2). Every latency measured against this stamp is therefore an "
                    "over-estimate, which is the safe direction."
                ),
                "calls": (
                    f"Calls into the robot class are made on a thread of the bridge's own, with a deadline "
                    f"({_CALL_DEADLINE_S * 1000:.0f} ms, {_STOP_DEADLINE_S * 1000:.0f} ms for the stop's freeze): "
                    "one that does not return fails the read or the write with ETIMEDOUT, and the class is not "
                    "called again until it has (F330)."
                    if self._bounded is not None
                    else "Made on the caller's thread: this robot class's calls end by themselves."
                ),
                "health": (
                    f"The servos' own error register (`{self._health}`) is read through the robot's bus every {_HEALTH_PERIOD_S * 1000:.0f} ms, and a bit in it is a fault: "
                    "FAULT and DEGRADED in the state, the servo's id and its bits in the fault word, and every command refused until it clears."
                    + (" A servo whose torque is off while the bridge has not stopped it is a fault too." if self._watch_torque else " Torque was not on at open, so it is not held to be.")
                    if self._health is not None
                    else "Not read. This robot class has no motor bus the bridge knows how to ask for a servo's error register, and LeRobot's observation carries "
                    "positions only, so a servo that shuts itself down is not seen: the fault word is always 0."
                ),
                "units": (
                    f"Position values converted from {unit} to radians."
                    if unit
                    else "Passed through unconverted. LeRobot's `.pos` values are normalised or in "
                    "degrees depending on the robot and its calibration, and this bridge was not "
                    "told which. Set `unit` in the driver configuration once you know; treating "
                    "degrees as radians is a 57x error on every command."
                )
                + "".join(
                    f" {motor}: {name}, passed through unconverted — LeRobot normalises it against a calibrated travel the bridge is not "
                    "told, so there is no factor to radians, and a value here is not an angle."
                    for motor, name in zip(self._motors, self._unit_names)
                    if name in _PASSTHROUGH_UNITS
                ),
            },
        }
        return json.dumps(descriptor, sort_keys=True, separators=(",", ":"))


# ── Helpers ──────────────────────────────────────────────────────────────────


def _fault_words(bits: int) -> str:
    """What a servo's fault bits say, for a person: the torque case in words, and any other as the register's own number."""
    if bits == _TORQUE_ON:
        return "still has its torque on after the stop was sent (it did not take it; the stop is sent again every health period)"
    if bits == _TORQUE_OFF:
        return "has its torque off although nothing here stopped it (a shutdown, a reboot or a power dip)"
    return f"reports error bits {bits:#04x} in its error register"


def _features(robot: Any, attribute: str) -> dict[str, Any]:
    value = getattr(robot, attribute, None)
    if value is None:
        raise AttributeError(
            f"this robot has no `{attribute}`. The bridge builds the descriptor from it, and "
            "there is nothing to fall back on that would not be a guess about the body."
        )
    if not isinstance(value, dict):
        raise TypeError(f"`{attribute}` is a {type(value).__name__}, and a mapping was expected.")
    return value


def _split_feature(key: str) -> tuple[str, str | None]:
    """``"shoulder_pan.pos"`` becomes ``("shoulder_pan", ".pos")``; a camera name gets ``None``."""
    for suffix in _SUFFIX_MODE:
        if key.endswith(suffix):
            return key[: -len(suffix)], suffix
    return key, None


def _number(value: Any) -> float:
    try:
        return float(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return math.nan


def _optional(value: Any, scale: float = 1.0) -> float:
    """A reading the robot may not report. Absent is NaN, never zero."""
    if value is None:
        return math.nan
    return _number(value) * scale


def _to_frame(value: Any, stamp_ns: int) -> Frame:
    """Wrap whatever LeRobot handed back as a camera frame.

    Handles a numpy array without importing numpy — anything with ``shape`` and ``tobytes`` — and
    falls back to raw bytes. The format is declared RGB8 because that is what LeRobot's camera
    interface produces; a driver for a depth camera would say otherwise.
    """
    shape = getattr(value, "shape", None)
    if shape is not None and len(shape) >= 2:
        height, width = int(shape[0]), int(shape[1])
        channels = int(shape[2]) if len(shape) > 2 else 1
        raw = value.tobytes() if hasattr(value, "tobytes") else bytes(value)
        return Frame(
            stamp_ns=stamp_ns,
            width=width,
            height=height,
            stride=width * channels,
            format=0,
            data=raw,
        )
    raw = bytes(value) if isinstance(value, (bytes, bytearray)) else b""
    return Frame(stamp_ns=stamp_ns, width=0, height=0, stride=0, format=0, data=raw)


def _slug(text: str) -> str:
    out = []
    pending = False
    for ch in text:
        if ch.isalnum():
            if pending and out:
                out.append("-")
            pending = False
            out.append(ch.lower())
        else:
            pending = True
    return "".join(out) or "lerobot-robot"


def open_bridge(config: str) -> LeRobotBridge:
    """Construct a bridge over a real LeRobot robot named in ``config``.

    ``config`` is the JSON the OS hands to ``open()``, and needs one more field than
    :class:`BridgeConfig` uses:

    ```json
    {"robot": {"type": "so101_follower", "port": "/dev/ttyACM0", "id": "arm"},
     "unit": "deg",
     "role_overrides": {"shoulder_pan": "left_arm.shoulder.yaw"}}
    ```

    Raises with the fix named when LeRobot is not installed, rather than an ``ImportError`` a driver
    author has to interpret (§64).
    """
    try:
        from lerobot.robots import make_robot_from_config  # type: ignore
        from lerobot.robots import RobotConfig  # type: ignore
    except ImportError as error:
        raise RuntimeError(
            "LeRobot is not installed, so there is no robot to bridge to. "
            "`pip install lerobot`, or construct LeRobotBridge yourself around any object with "
            "LeRobot's Robot interface — the bridge does not import lerobot except here."
        ) from error

    data = json.loads(config) if config and config.strip() else {}
    spec = data.get("robot")
    if not isinstance(spec, dict) or "type" not in spec:
        raise ValueError(
            'the configuration needs a `robot` object naming the LeRobot robot to open, e.g. '
            '{"robot": {"type": "so101_follower", "port": "/dev/ttyACM0"}}.'
        )

    # LeRobot's configs are a registry of subclasses named by `type`, and the base class takes no arguments: the
    # class for a type is looked up, and `type` is not one of its fields. (The first version of this called
    # `RobotConfig(**spec)`, which is what the library's documentation looked like it said and fails on every
    # release the bridge has been run against, found when it first met one — F182.)
    fields = {key: value for key, value in spec.items() if key != "type"}
    lookup = getattr(RobotConfig, "get_choice_class", None)
    if lookup is None:
        raise RuntimeError(
            "this LeRobot's RobotConfig has no `get_choice_class`, which is how a robot type name becomes a "
            "configuration class. Pin the LeRobot release this bridge was run against (drivers/lerobot/sim/Dockerfile)."
        )
    try:
        config_class = lookup(spec["type"])
    except KeyError:
        # LeRobot registers a robot's configuration class when the robot's package is *imported*, and `lerobot.robots` imports none of them
        # (its factory imports one lazily, after the configuration exists). So a type name no module has been imported for is not in the
        # registry, whatever the library supports — F221, found by constructing a real SO-101 the way this function says it does.
        # Import the packages that import cleanly, and say which did not if the name is still not there.
        config_class = lookup_after_registering(spec["type"], lookup)
    # `config_class(**fields)` hands the dataclass raw JSON: a `calibration_dir` arrives as a `str` where LeRobot's code calls `.mkdir()` on a
    # `Path`, and `cameras` as dicts where it wants camera configs (F222, found by the same construction). LeRobot's own configuration parser
    # is `draccus`, which is what its command line uses to turn text into these dataclasses, so it is what turns this JSON into one.
    try:
        import draccus  # type: ignore
    except ImportError:
        configuration = config_class(**fields)
    else:
        configuration = draccus.decode(config_class, fields)
    robot = make_robot_from_config(configuration)
    return LeRobotBridge(robot, BridgeConfig(config))


def lookup_after_registering(name: str, lookup: Any) -> Any:
    """LeRobot's configuration class for robot `name`, after importing every `lerobot.robots.*` package that will import.

    A package whose optional dependencies are missing (a vendor SDK) is skipped and named; the type being asked for is the only one
    that matters, and if it is among the skipped, the error says what its import raised.
    """
    import importlib
    import pkgutil

    import lerobot.robots as package  # type: ignore

    skipped: dict[str, str] = {}
    for info in pkgutil.iter_modules(package.__path__):
        if not info.ispkg:
            continue
        try:
            importlib.import_module(f"{package.__name__}.{info.name}")
        except Exception as error:  # noqa: BLE001 — a robot whose SDK is not installed is not this robot
            skipped[info.name] = repr(error)
    try:
        return lookup(name)
    except KeyError:
        raise RuntimeError(
            f"LeRobot has no robot type {name!r}. Its robot packages that imported are the types this LeRobot can open; "
            + (f"these would not import: {skipped}. " if skipped else "")
            + "The type is the name a `RobotConfig` subclass registers (`so101_follower`, `koch_follower`, `lekiwi`, …)."
        ) from None

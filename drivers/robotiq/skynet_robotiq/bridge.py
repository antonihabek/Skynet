"""The RHAL driver: a Robotiq 2F-85 or Hand-E gripper over Modbus RTU (DRY-COVERAGE-PLAN.md item 12.E.8).

§4's five mandatory entry points over :mod:`skynet_robotiq.frames` and :mod:`skynet_robotiq.link`. It links nothing of the maker's: Robotiq publish a C++ SDK (`github.com/Robotiq/grippers`, BSD-3-Clause), the wire was read
out of its headers, and `layout_check.py` runs the maker's own client and holds this driver's bytes to what it writes. The substrate the maker ships is `makeFakeGripper()`, a register model behind a Modbus RTU server,
which `sim/` serves on a pseudo-terminal: rung 2, a maker's simulator and not a gripper.

# The body

One prismatic joint, **`jaw`**: the opening between the fingers, in metres, from `min_opening` to `max_opening` of the model's profile. The profile is the maker's *measured* one (`device_profile.hpp`: the register
a gripper reads with nothing between the fingers at rPR 0 and rPR 255) for the **2F-85** and the **Hand-E**; nothing the gripper says identifies its model, so `model` is the operator's claim and has no default. Any
other gripper (a 2F-140, or either with other fingers, which move the measured endpoints) is driven with `model: "custom"` and a `profile` the operator measured, and the driver says it is theirs. A configuration may
narrow `limits` and never widen them. `speed` (m/s) and `effort` (0 to 1, the maker's `effortToRegister`) are the operator's: a position request says neither how fast the fingers go nor how hard they close on what they meet.

# The one process image

The maker's own design, followed because Robotiq proved it on hardware: **one command image**, sent in **every exchange cycle** (Modbus function 0x17: write the command block, read the status block, one transaction),
whatever has or has not changed. A gripper that hears nothing for one second raises fault 0x09, so the exchange cycle is also the keep-alive, and a command is not a transaction of its own: `write_command` changes
the image and waits for the exchange that carried it. The image is **seeded from the gripper's own echoes** (rACT, rGTO and rPR from gACT, gGTO and gPR), so that connecting never disturbs a gripper that is already
running; its speed and force are the operator's, and a gripper that was holding something is told them from the first exchange.

# What it refuses to do by itself

* **Activate.** Activation (and a reset after a major fault) is a calibration sweep that *moves the fingers through their full range* (`recoverFromFault`'s own warning). A gripper that is not activated opens, reports
  `HOMING_PENDING` and is refused commands, naming the `activate` motion that does it on purpose; a major fault is cleared by the `recover` motion, the maker's unconditional handshake.
* **Decide what a stop is.** `stop_mode` has no default and accepts only `halt`: clear rGTO and go on exchanging, which takes the go-to action away and leaves the gripper activated. It does **not** release a held
  workpiece. rATR is Robotiq's *emergency release*, which opens or closes the fingers all the way and leaves the gripper in a major fault until it is reset; whether a pinched finger or a held part makes release the safer
  stop is the integrator's risk assessment, and a configuration that asks for it is refused with that said, where a quiet `halt` would be a stop nobody chose. It is a **software stop**: the gripper's own e-stop is a circuit.
* **Send a late command.** A command that was not carried within the budget (a quarter of a second is the control loop's, F256) is `EIO` now, and the image is put back, so that it is not written when the line returns.

# The stop, and the image

`emergency_stop` sets the latch and clears rGTO in the image **under the lock `write_command` changes the image under**, so a command cannot put the go-to back after a stop; the loop is woken, and since the image is sent
every cycle a lost frame is a frame sent again one period later, with no thread of its own to start. While latched, no command is accepted and no motion runs. `release_stop` clears the latch only once an exchange
after it shows the gripper has taken the halt (gGTO, rGTO's echo, clear); a fault is not the latch's business, and commands are refused while a major one is shown.
"""

from __future__ import annotations

import json
import math
import sys
import threading
import time

import skynet_rhal as rhal

from . import frames, link
from .frames import ACT, GTO, Command, Status

__all__ = ["BridgeRefusal", "RobotiqBridge", "TOKEN"]

TOKEN = "vendor.robotiq.task"
JOINT = "jaw"

_RATE_HZ = 50.0
#: One Modbus transaction's budget. A gripper answers in about 4 ms (the maker's own figure at 115200 baud, `connection_config.hpp`), and two transactions have to fit in a command's budget.
_TRANSACTION_S = 0.08
_OPEN_TIMEOUT_S = 2.0
_STALE_S = 0.5
#: How long a command may take to be carried: the control loop waits for it, and a quarter of a second is the contract's bound (`tools/faultproxy/chaos.py`, F256).
_COMMAND_BUDGET_S = 0.2
#: The fastest the exchange cycle runs, whoever asks: "with the default baudrate of the gripper the maximum achievable communication frequency is 250Hz" (the maker's README).
_MAX_RATE_HZ = 250.0
#: Consecutive failed exchanges before the link is called dead: the maker's `kFaultThreshold`.
_FAULT_THRESHOLD = 3
_INITIAL_READ_ATTEMPTS = 3
_ACTIVATION_TIMEOUT_S = 15.0
#: A line that has failed is opened again no more often than this.
_REOPEN_S = 0.5
_ECHO_GRACE_S = 0.5
_KNOWN_KEYS = {"device", "baud", "host", "port", "slave", "model", "profile", "simulator", "stop_mode", "speed", "effort", "limits", "roles", "rate_hz", "timeout_ms", "stale_ms", "open_timeout_ms"}
_PROFILE_KEYS = ("min_speed", "max_speed", "min_opening", "max_opening", "open_position", "closed_position")
_MOTIONS = ("status", "activate", "recover", "move", "grip", "release")
_FLAGS_OF_CLASS = {"ok": 0, "degraded": rhal.FLAG_DEGRADED, "fault": rhal.FLAG_FAULT | rhal.FLAG_DEGRADED, "estop": rhal.FLAG_ESTOP | rhal.FLAG_DEGRADED}


class BridgeRefusal(Exception):
    """A reason this body did not come up or a command did not happen, in the terms an integrator acts on."""

    def __init__(self, message: str, code: int = rhal.SKR_ENODEV) -> None:
        super().__init__(message)
        self.code = code


class Unreachable(BridgeRefusal):
    """The gripper did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""

    def __init__(self, message: str) -> None:
        super().__init__(message, rhal.SKR_EIO)


class RobotiqBridge:
    """A body made of one gripper's fingers."""

    #: A stop written to the host while a motion runs is read at once: `emergency_stop` is safe from another thread (python-host's `stop_during_extension`).
    stop_during_extension = True

    def __init__(self) -> None:
        self._opened = False
        self._estop = False
        self._descriptor: str | None = None
        self._seq = 0
        self._simulated = True
        self._profile: frames.Profile = frames.PROFILES["2F-85"]
        self._model = ""
        self._slave = frames.SLAVE_ADDRESS
        self._limits = (0.0, 0.0)
        self._speed = 0.0
        self._effort = 0.0
        self._speed_register = 0
        self._effort_register = 0
        self._roles: dict[str, str | None] = {}
        self._rate_hz = _RATE_HZ
        self._stale_s = _STALE_S
        self._timeout = _TRANSACTION_S
        self._link: link.Link | None = None
        self._loop: threading.Thread | None = None
        self._closing = threading.Event()
        self._kick = threading.Event()
        self._lock = threading.Lock()
        self._changed = threading.Condition(self._lock)
        # Everything below is guarded by `_lock`.
        self._image = Command()
        self._image_seq = 0
        self._carried_seq = 0
        self._epoch = 0
        self._latest: Status | None = None
        self._stamp_ns = 0
        self._exchanges = 0
        self._failures = 0
        self._dead: str | None = None
        self._commanded_at = 0.0
        self._last_refusal: str | None = None

    # ── §4's entry points ────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError as error:
            _say(f"the configuration is not JSON: {error}")
            return rhal.SKR_EINVAL
        if not isinstance(settings, dict):
            _say("the configuration is not a JSON object.")
            return rhal.SKR_EINVAL
        self._teardown()
        self._estop = False
        self._descriptor = None
        self._last_refusal = None
        try:
            self._open(settings)
        except BridgeRefusal as refusal:
            self._teardown()
            _say(str(refusal))
            return rhal.SKR_EIO if isinstance(refusal, Unreachable) else rhal.SKR_ENODEV
        except (OSError, frames.ProtocolError, link.LinkError) as error:
            self._teardown()
            _say(f"the gripper could not be read: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _open(self, settings: dict) -> None:
        unknown = sorted(set(settings) - _KNOWN_KEYS)
        if unknown:
            raise BridgeRefusal(f"the configuration has {', '.join(map(repr, unknown))}, which this driver does not read. The keys it reads are {', '.join(sorted(_KNOWN_KEYS))}.")
        device, host = settings.get("device"), settings.get("host")
        if (device is None) == (host is None):
            raise BridgeRefusal("the configuration names neither a serial `device` nor a `host` and `port`, or names both. This driver does not scan for a gripper, and one that guessed a port would command whichever "
                                "device answered first: give the gripper's RS-485 adapter (`device`: /dev/ttyUSB0) or the RS-485 gateway that forwards its line raw (`host` and `port`).")
        baud = settings.get("baud", 115200)
        if device is not None:
            if not isinstance(device, str) or not device:
                raise BridgeRefusal(f"`device` of {device!r} is not a path.")
            if isinstance(baud, bool) or not isinstance(baud, int) or baud not in (9600, 19200, 38400, 57600, 115200, 230400, 460800, 500000, 921600, 1000000):
                raise BridgeRefusal(f"`baud` of {baud!r} is not a speed the gripper's line can be set to: it must match the gripper's persisted setting, 115200 from the factory.")
            self._link = link.Link(device=device, baud=baud)
        else:
            port = settings.get("port")
            if not isinstance(host, str) or not host or isinstance(port, bool) or not isinstance(port, int) or not 0 < port < 65536:
                raise BridgeRefusal(f"`host` of {host!r} and `port` of {port!r} do not name a gateway: a host name and a port from 1 to 65535.")
            self._link = link.Link(host=host, port=port)
        slave = settings.get("slave", frames.SLAVE_ADDRESS)
        if isinstance(slave, bool) or not isinstance(slave, int) or not 1 <= slave <= 247:
            raise BridgeRefusal(f"`slave` of {slave!r} is not a Modbus address from 1 to 247 (the gripper's factory setting is {frames.SLAVE_ADDRESS}).")
        self._slave = slave
        self._profile = self._load_profile(settings)
        stop = settings.get("stop_mode")
        if stop != "halt":
            raise BridgeRefusal(
                "`stop_mode` is not in the configuration, or is not `halt`. What stopping a gripper means is a person's decision and this driver will not make it: `halt` clears rGTO, which takes the go-to action "
                "away and leaves the gripper activated, and does not release a held workpiece. The other thing a gripper can do, rATR (Robotiq's emergency release), opens or closes the fingers all the way and "
                "leaves the gripper in a major fault until it is reset; whether that is the safer stop (a pinched finger) or the dangerous one (a dropped part) is the integrator's risk assessment, and it is "
                "not offered.")
        claim = settings.get("simulator")
        if not isinstance(claim, bool):
            raise BridgeRefusal("`simulator` is not in the configuration, or is not true or false. Nothing the gripper says tells a real one from a simulation, so the descriptor cannot be left to guess: a bench run "
                                "described as a simulation costs a tier it could have had, and a simulation described as a bench run is a false claim.")
        self._simulated = claim
        profile = self._profile
        self._speed = _required_number(settings, "speed", f"in m/s, from {profile.min_speed:g} to {profile.max_speed:g}: the register that says how fast the fingers close is the operator's, and nothing "
                                                          "in a position request says it", profile.min_speed, profile.max_speed)
        self._effort = _required_number(settings, "effort", "from 0 to 1, the fraction of the gripper's force range the fingers close on what they meet with (the maker's `effortToRegister`): how hard "
                                                            "to hold is the operator's, and a held part is told it from the first exchange", 0.0, 1.0)
        speed_register, effort_register = frames.speed_to_register(self._speed, profile), frames.effort_to_register(self._effort)
        if speed_register is None or effort_register is None:
            raise BridgeRefusal("the speed or the effort has no register value.")
        self._speed_register, self._effort_register = speed_register, effort_register
        self._limits = self._load_limits(settings.get("limits"), profile)
        roles = settings.get("roles") or {}
        if not isinstance(roles, dict) or set(roles) - {JOINT}:
            raise BridgeRefusal(f"`roles` is a mapping from a joint's name to its role, and the one joint is {JOINT!r}.")
        self._roles = {JOINT: roles.get(JOINT)}
        self._rate_hz = _bounded(settings, "rate_hz", _RATE_HZ, 1.0, _MAX_RATE_HZ)
        self._timeout = _bounded(settings, "timeout_ms", _TRANSACTION_S * 1000, 10.0, 5000.0) / 1000.0
        self._stale_s = _bounded(settings, "stale_ms", _STALE_S * 1000, 20.0, 30000.0) / 1000.0
        open_s = _bounded(settings, "open_timeout_ms", _OPEN_TIMEOUT_S * 1000, 100.0, 60000.0) / 1000.0

        assert self._link is not None
        self._link.open(timeout=min(open_s, 3.0))
        status = self._first_status(open_s)
        with self._lock:
            self._latest, self._stamp_ns = status, rhal.acquisition_ns()
            self._exchanges, self._failures, self._dead = 0, 0, None
            self._image_seq = self._carried_seq = 0
            # The maker's `initializeImage`: rACT, rGTO and rPR from the gripper's own echoes, so that connecting never disturbs one that is already running. Speed and force are the operator's.
            action = (ACT if status.activated else 0) | (GTO if status.go_to else 0)
            self._image = Command(action, status.position_echo, self._speed_register, self._effort_register)
        cls = frames.fault_class(status)
        if cls in ("fault", "estop"):
            _say(f"the gripper reports {self._describe_fault(status)}: opened, and commands are refused until the `recover` motion has reset it (clearing rACT and setting it again, which is a calibration sweep through the fingers' "
                 "full range: do it where the gripper is).")
        elif not status.activated or status.activation_state != frames.STA_COMPLETE:
            _say(f"the gripper is not activated ({frames.STA_NAMES[status.activation_state]}): opened, and commands are refused until the `activate` motion has run, which is a calibration sweep through the "
                 "fingers' full range and so a decision for a person.")
        self._closing.clear()
        self._kick.clear()
        self._loop = threading.Thread(target=self._exchange_loop, daemon=True, name="robotiq-exchange")
        self._loop.start()
        self._descriptor = self._build_descriptor()

    def _load_profile(self, settings: dict) -> frames.Profile:
        model = settings.get("model")
        given = settings.get("profile")
        if model in frames.PROFILES:
            if given is not None:
                raise BridgeRefusal(f"`profile` was given with `model` {model!r}, whose profile is the maker's measured one. Use `model: \"custom\"` for a profile you measured yourself.")
            self._model = model
            return frames.PROFILES[model]
        if model != "custom":
            raise BridgeRefusal(f"`model` of {model!r} is not in the configuration, or is not one of {', '.join(map(repr, [*frames.PROFILES, 'custom']))}. Nothing the gripper says identifies its model, and the opening a "
                                "register means depends on it: say which gripper this is. The maker's SDK carries a measured profile for the 2F-85 and the Hand-E and none for the 2F-140, so that one is `custom`, "
                                "with a `profile` measured as `device_profile.hpp` says (command rPR 0 and read gPO, then rPR 255 and read gPO, with nothing between the fingers).")
        if not isinstance(given, dict) or set(given) != set(_PROFILE_KEYS):
            raise BridgeRefusal(f"`model: \"custom\"` needs a `profile` with exactly {', '.join(_PROFILE_KEYS)}: the opening range and speed range in metres and m/s, and the registers gPO reads after rPR 0 and rPR 255 "
                                "with nothing between the fingers.")
        for key in _PROFILE_KEYS:
            value = given[key]
            if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
                raise BridgeRefusal(f"`profile.{key}` of {value!r} is not a number.")
        if not (0 <= given["open_position"] < given["closed_position"] <= 255 and float(given["open_position"]).is_integer() and float(given["closed_position"]).is_integer()):
            raise BridgeRefusal(f"`profile` has open_position {given['open_position']!r} and closed_position {given['closed_position']!r}: two registers, 0 to 255, the open one below the closed one.")
        if not 0 < given["min_speed"] < given["max_speed"] or not 0 <= given["min_opening"] < given["max_opening"]:
            raise BridgeRefusal("`profile` needs 0 < min_speed < max_speed and 0 <= min_opening < max_opening, in m/s and metres.")
        self._model = "custom"
        return frames.Profile("custom", float(given["min_speed"]), float(given["max_speed"]), float(given["min_opening"]), float(given["max_opening"]), int(given["open_position"]), int(given["closed_position"]))

    def _load_limits(self, given: object, profile: frames.Profile) -> tuple[float, float]:
        low, high = profile.min_opening, profile.max_opening
        if given is None:
            return (low, high)
        if not (isinstance(given, (list, tuple)) and len(given) == 2 and all(isinstance(v, (int, float)) and not isinstance(v, bool) and math.isfinite(v) for v in given)):
            raise BridgeRefusal("`limits` is [min, max] in metres, as two numbers.")
        lo, hi = float(given[0]), float(given[1])
        if not lo < hi:
            raise BridgeRefusal(f"`limits` of [{lo}, {hi}] is not a range.")
        if lo < low - 1e-9 or hi > high + 1e-9:
            raise BridgeRefusal(f"`limits` of [{lo:.6f}, {hi:.6f}] m is wider than the opening range of the profile, [{low:.6f}, {high:.6f}]: a configuration may narrow it and never widen it.")
        return (lo, hi)

    def _first_status(self, budget: float) -> Status:
        """The maker's `initializeImage`: up to three reads of the status block (function 0x03) before the gripper is called absent."""
        assert self._link is not None
        deadline = time.monotonic() + budget
        last: Exception | None = None
        for attempt in range(1, _INITIAL_READ_ATTEMPTS + 1):
            try:
                data = self._link.transact(frames.request_read(self._slave, frames.STATUS_ADDRESS, frames.BLOCK_REGISTERS), frames.FC_READ_HOLDING, self._slave,
                                           max(0.01, min(self._timeout * 4, deadline - time.monotonic())))
                return frames.parse_status(data)
            except (link.LinkError, frames.ProtocolError) as error:
                last = error
                if time.monotonic() >= deadline:
                    break
        raise Unreachable(f"no gripper answered the status read at slave {self._slave} on {self._link.target} ({last}). Is it powered, and do the baud rate and the Modbus slave address match the gripper's "
                          "(115200 and 9 from the factory)?")

    def close(self) -> int:
        # A close writes nothing. The gripper keeps the last command it was given: fingers that were closing go on closing and hold what they meet, and a gripper nobody talks to raises fault 0x09 after a second,
        # which the maker's own SDK does the same way. What a stop does to a held workpiece is the integrator's to decide (see `stop_mode`), and never a close's.
        self._teardown()
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened:
            return rhal.SKR_EPERM
        now = rhal.acquisition_ns()
        with self._lock:
            status, stamp, dead = self._latest, self._stamp_ns, self._dead
            image, commanded_at = self._image, self._commanded_at
        self._seq += 1
        out.seq = self._seq
        out.stamp_ns = stamp or now
        out.flags = rhal.HOMING_PENDING
        out.fault_code = 0
        out.position_rad = [math.nan]
        out.velocity_rad_s = [math.nan]
        out.effort_nm = [math.nan]
        if status is None:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        opening = frames.opening_from_register(status.position, self._profile)
        if opening is not None:
            out.position_rad = [opening]
        if status.activation_state == frames.STA_COMPLETE:
            out.flags = (out.flags & ~rhal.HOMING_MASK) | rhal.HOMING_COMPLETE
        else:
            out.flags |= rhal.FLAG_DEGRADED
        if self._estop:
            out.flags |= rhal.FLAG_ESTOP
        out.flags |= _FLAGS_OF_CLASS[frames.fault_class(status)]
        out.fault_code = status.fault_byte
        # F175: a command the gripper acknowledged on the wire and then did not take. It echoes the position it was asked for (gPR) and whether it is going there (gGTO), a cycle or so after the write.
        if image.action & GTO and not self._estop and status.activation_state == frames.STA_COMPLETE and time.monotonic() - commanded_at > _ECHO_GRACE_S \
                and (not status.go_to or status.position_echo != image.position):
            out.flags |= rhal.FLAG_DEGRADED
        age = (rhal.acquisition_ns() - stamp) / 1e9
        if dead or age > self._stale_s:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._link is None:
            return rhal.SKR_EPERM
        if cmd.joint_count != 1:
            return self._refused("count", f"the command has {cmd.joint_count} joints and this body has one, {JOINT}: nothing was sent.")
        if cmd.mode != rhal.MODE_POSITION:
            return self._refused("mode", f"the command is in mode {cmd.mode}, and this gripper takes a position: its target is the opening between the fingers, in metres, and there is no velocity or torque route. "
                                         "Send positions.")
        if self._estop:
            return rhal.SKR_EPERM
        applied = cmd.mask[0] if cmd.mask else 1
        if not applied:
            return rhal.SKR_OK
        value = cmd.values[0]
        low, high = self._limits
        if not math.isfinite(value):
            return self._refused("finite", f"the jaw was commanded to {value!r}, which is not finite: the command was refused and nothing was sent.")
        if not low - 1e-9 <= value <= high + 1e-9:
            return self._refused("range", f"the jaw was commanded to {value:.6f} m, outside its opening [{low:.6f}, {high:.6f}]: the command was refused whole and nothing was sent.")
        register = frames.opening_to_register(min(max(value, low), high), self._profile)
        if register is None:
            return self._refused("register", f"an opening of {value:.6f} m has no register in this profile: nothing was sent.")
        try:
            self._gate()
            self._send(lambda image: Command(ACT | GTO, register, self._speed_register, self._effort_register))
        except BridgeRefusal as error:
            return self._refused("command:" + str(error.code) + ":" + error.args[0][:40], str(error), error.code)
        except frames.ProtocolError as error:
            return self._refused("protocol", str(error))
        self._last_refusal = None
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        with self._lock:
            self._estop = True
            image = self._image
            self._image = Command(image.action & ~GTO, image.position, image.speed, image.force)
            self._image_seq += 1
            self._changed.notify_all()
        self._kick.set()
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch (§48.2) once the gripper has been *seen* to take the stop: an exchange after the call shows rGTO's echo, gGTO, clear. A fault is not the latch's business (commands are refused while a
        major fault is shown, whatever the latch says), and neither is activation: a latch that waited for them would leave the operator unable to run `activate` or `recover`, which a latched stop refuses."""
        if self._link is None or self._loop is None:
            self._estop = False
            return rhal.SKR_OK
        try:
            self._wait_for_status(lambda s: not s.go_to, _COMMAND_BUDGET_S * 4 + self._timeout * 4, honour_stop=False)
        except BridgeRefusal as error:
            _say(f"the stop stays latched: no exchange has shown the gripper take it ({error}) Fix the line, or close and open the driver, which is a new session.")
            return rhal.SKR_EIO
        self._estop = False
        self._last_refusal = None
        return rhal.SKR_OK

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── the image and the exchange cycle ─────────────────────────────────────

    def _gate(self) -> None:
        """What a command waits for: a gripper that has been read, that is activated and that holds no major fault."""
        with self._lock:
            status = self._latest
        if status is None:
            raise BridgeRefusal("the gripper has not been read yet.", rhal.SKR_ETIMEDOUT)
        cls = frames.fault_class(status)
        if cls in ("fault", "estop"):
            raise BridgeRefusal(f"the gripper reports {self._describe_fault(status)}, and a gripper in a major fault takes nothing: the `recover` motion resets it (a calibration sweep through the fingers' full range). "
                                "The command was not sent.", rhal.SKR_EPERM)
        if status.activation_state != frames.STA_COMPLETE:
            raise BridgeRefusal(f"the gripper is not activated ({frames.STA_NAMES[status.activation_state]}) and takes no motion command: the `activate` motion does it, and is a calibration sweep through the "
                                "fingers' full range. The command was not sent.", rhal.SKR_EPERM)

    def _send(self, make, *, budget: float = _COMMAND_BUDGET_S) -> None:
        """Change the image to what `make(old)` says and wait for the exchange that carried it. **Under the lock a stop takes**, so a command cannot put the go-to back after one. A command that was not carried
        in time is put back and is `EIO` now, and is not written later (F256)."""
        deadline = time.monotonic() + budget
        with self._lock:
            if self._estop:
                raise BridgeRefusal("the stop is latched: nothing is written.", rhal.SKR_EPERM)
            previous = self._image
            new = make(previous)
            new.block()    # a block the wire does not allow is refused here, before it is the image
            if new == previous:
                return
            self._image = new
            self._commanded_at = time.monotonic()
            self._image_seq += 1
            mine, epoch = self._image_seq, self._epoch
        self._kick.set()
        with self._changed:
            while self._carried_seq < mine:
                if self._estop:
                    raise BridgeRefusal("the stop was latched while the command was being made.", rhal.SKR_EPERM)
                if self._epoch != epoch:
                    raise BridgeRefusal("the gripper's line was lost and found again while the command was waiting: it was not sent, and the image is the gripper's own again.", rhal.SKR_EIO)
                remaining = deadline - time.monotonic()
                if remaining <= 0 or self._closing.is_set():
                    if self._image_seq == mine:
                        self._image = previous
                        self._image_seq += 1
                    raise BridgeRefusal(f"the gripper did not carry the command in the {budget:.2f} s a command is given (an exchange takes {self._timeout * 1000:.0f} ms at most, and the last said: "
                                        f"{self._dead or 'nothing wrong'}): it was not sent, and the image was put back. A slow or dead line, not a refusal by the gripper.", rhal.SKR_EIO)
                self._changed.wait(min(remaining, 0.05))

    def _exchange_loop(self) -> None:
        """The maker's exchange thread: the image out and the status in, once a period, forever."""
        assert self._link is not None
        link_ = self._link
        period = 1.0 / self._rate_hz
        spacing = 1.0 / _MAX_RATE_HZ
        last = 0.0
        reopened = 0.0
        open_error: str | None = None
        needs_seed = False
        due = time.monotonic()
        while not self._closing.is_set():
            woken = self._kick.wait(max(0.0, due - time.monotonic()))
            if self._closing.is_set():
                break
            if woken:
                self._kick.clear()
                with self._lock:
                    latched = self._estop
                if not latched:
                    wait = spacing - (time.monotonic() - last)
                    if wait > 0:
                        time.sleep(wait)
            last = time.monotonic()
            try:
                if needs_seed:
                    # The maker's `initializeImage`, again. A line that was dead is not a gripper that was left as this driver last asked: it may have been power-cycled, or stopped, or have finished a motion. So the first thing
                    # sent after it is a status read, and the image is made from what the gripper says it is doing, as it is when the driver opens. The image this driver held (rACT set) would otherwise activate, with a
                    # calibration sweep nobody asked for, a gripper that lost its activation with the power.
                    self._reseed(frames.parse_status(link_.transact(frames.request_read(self._slave, frames.STATUS_ADDRESS, frames.BLOCK_REGISTERS), frames.FC_READ_HOLDING, self._slave, self._timeout)))
                    needs_seed = False
                with self._lock:
                    image, seq = self._image, self._image_seq
                data = link_.transact(frames.request_exchange(self._slave, image.block()), frames.FC_READ_WRITE, self._slave, self._timeout)
                status = frames.parse_status(data)
            except (link.LinkError, frames.ProtocolError) as error:
                reason = str(error)
                if not link_.is_open:
                    if time.monotonic() - reopened >= _REOPEN_S:
                        # A gateway that went away, or an adapter that was pulled out: the line is opened again, at most twice a second, and what it says is what is reported while it stays shut.
                        reopened = time.monotonic()
                        try:
                            link_.open(timeout=min(self._timeout, 0.5))
                            open_error = None
                        except link.LinkError as again:
                            open_error = str(again)
                    reason = open_error or reason
                with self._lock:
                    self._failures += 1
                    if self._failures >= _FAULT_THRESHOLD:
                        self._dead = reason
                        needs_seed = True
                    self._changed.notify_all()
            else:
                with self._lock:
                    self._latest, self._stamp_ns = status, rhal.acquisition_ns()
                    self._exchanges += 1
                    self._failures, self._dead = 0, None
                    self._carried_seq = max(self._carried_seq, seq)
                    self._changed.notify_all()
            due = max(due + period, time.monotonic()) if not woken else time.monotonic() + period

    def _reseed(self, status: Status) -> None:
        """Make the image from the gripper's own echoes, as `open` does. A latched stop keeps rGTO clear, whatever the gripper says. A command that was waiting to be carried is not: the epoch moves, and it is EIO."""
        with self._changed:
            old = self._image
            action = ACT if status.activated else 0
            if self._estop:
                position = old.position
            else:
                action |= GTO if status.go_to else 0
                position = status.position_echo
            self._image = Command(action, position, self._speed_register, self._effort_register)
            self._image_seq += 1
            self._epoch += 1
            self._latest, self._stamp_ns = status, rhal.acquisition_ns()
            self._changed.notify_all()

    def _wait_for_status(self, predicate, timeout: float, *, honour_stop: bool = True) -> Status:
        """The first status after now that `predicate` holds for: the maker's `waitFor`, on the exchange that shows it."""
        deadline = time.monotonic() + timeout
        with self._changed:
            seen = self._exchanges
            while True:
                if self._closing.is_set():
                    raise BridgeRefusal("the driver was closed.", rhal.SKR_EPERM)
                if honour_stop and self._estop:
                    raise BridgeRefusal("the motion ended: the stop was latched.", rhal.SKR_EPERM)
                if self._exchanges > seen and self._latest is not None and predicate(self._latest):
                    return self._latest
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise BridgeRefusal(f"no exchange showed what was waited for in {timeout:.1f} s ({self._dead or 'the line is answering'}).", rhal.SKR_EIO)
                self._changed.wait(min(remaining, 0.05))

    # ── plumbing ─────────────────────────────────────────────────────────────

    def _describe_fault(self, status: Status) -> str:
        parts = []
        if status.gripper_fault:
            text, severity = frames.GRIPPER_FAULTS.get(status.gripper_fault, ("a code the maker does not name", "major"))
            parts.append(f"gripper fault 0x{status.gripper_fault:02X} ({text}; {severity})")
        if status.controller_fault:
            text, severity = frames.CONTROLLER_FAULTS.get(status.controller_fault, ("a code the maker does not name", "major"))
            parts.append(f"controller fault 0x{status.controller_fault:02X} ({text}; {severity})")
        return " and ".join(parts) or "no fault"

    def _refused(self, cause: str, message: str, code: int = rhal.SKR_EINVAL) -> int:
        """`code` and the reason, said once for a run of the same cause: a stream at the control rate that fails for one reason would otherwise say it every period."""
        if cause != self._last_refusal:
            self._last_refusal = cause
            _say(message)
        return code

    def _teardown(self) -> None:
        self._closing.set()
        self._kick.set()
        with self._changed:
            self._changed.notify_all()
        if self._loop is not None:
            self._loop.join(timeout=2.0)
        self._loop = None
        if self._link is not None:
            self._link.close()
        self._link = None
        with self._lock:
            self._latest = None

    def _build_descriptor(self) -> str:
        low, high = self._limits
        joint = {"name": JOINT, "role": self._roles.get(JOINT), "type": "prismatic",
                 "limits": {"position_min_rad": low, "position_max_rad": high, "velocity_max_rad_s": self._speed}, "control_modes": ["position"]}
        effector = {"name": "gripper", "role": self._roles.get(JOINT), "kind": "gripper", "dof": 1, "max_grip_force_n": None, "max_opening_m": high - low}
        descriptor = {
            "schema_version": 1, "robot_slug": "robotiq-" + self._model.lower(), "digest": None, "verified": False, "simulator": self._simulated, "base": "fixed", "dof": 1,
            "joints": [joint], "end_effectors": [effector], "sensors": [],
            "control": {"modes": ["position"], "rate_hz_max": self._rate_hz, "rate_hz_min": 1.0},
            # Null: the opening and speed come from a measured profile and none of it is a risk assessment (§5.2); and no force in newtons, because the gripper takes a fraction of a range this driver cannot see.
            "envelope": None, "capabilities": ["safety.estop"], "motions": [_DESCRIPTORS[name] for name in _MOTIONS], "extensions": [TOKEN], "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))

    # ── the extension: motions ───────────────────────────────────────────────

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != TOKEN:
            return rhal.SKR_ENOSYS, f"unknown token {token!r}. This driver serves {TOKEN}.".encode()
        if not self._opened or self._link is None:
            return rhal.SKR_EPERM, b"the driver is not open."
        try:
            words = data.decode("utf-8").split()
        except UnicodeDecodeError:
            return rhal.SKR_EINVAL, b"the request is text: `<motion> k=v k=v` in UTF-8."
        if not words:
            return rhal.SKR_EINVAL, f"empty request. Motions: {', '.join(_MOTIONS)}.".encode()
        motion, args = words[0], {}
        for word in words[1:]:
            key, sep, value = word.partition("=")
            if not sep or not key:
                return rhal.SKR_EINVAL, f"malformed argument {word!r}: wanted k=v.".encode()
            args[key] = value
        if motion not in _MOTIONS:
            return rhal.SKR_EINVAL, f"unknown motion {motion!r}. Motions: {', '.join(_MOTIONS)}.".encode()
        if self._estop and motion != "status":
            return rhal.SKR_EPERM, b"refused: the stop is latched. Only `status` is answered until it is released."
        try:
            return rhal.SKR_OK, json.dumps(getattr(self, f"_motion_{motion}")(args), separators=(",", ":")).encode()
        except BridgeRefusal as refusal:
            return refusal.code, str(refusal).encode()
        except frames.ProtocolError as error:
            return rhal.SKR_EINVAL, f"the request does not fit the wire: {error}".encode()

    def _number(self, args: dict, key: str, *, required: bool = False, minimum: float | None = None, maximum: float | None = None) -> float | None:
        if key not in args:
            if required:
                raise BridgeRefusal(f"`{key}` is required.", rhal.SKR_EINVAL)
            return None
        try:
            value = float(args.pop(key))
        except ValueError:
            raise BridgeRefusal(f"`{key}` is not a number.", rhal.SKR_EINVAL) from None
        if not math.isfinite(value) or (minimum is not None and value < minimum) or (maximum is not None and value > maximum):
            raise BridgeRefusal(f"`{key}` of {value:g} is outside {minimum if minimum is not None else '-inf'} to {maximum if maximum is not None else 'inf'}.", rhal.SKR_EINVAL)
        return value

    def _no_arguments(self, motion: str, args: dict) -> None:
        if args:
            raise BridgeRefusal(f"`{motion}` does not take {', '.join(sorted(args))}.", rhal.SKR_EINVAL)

    def _opening_of(self, status: Status) -> float:
        """Where the fingers are, in metres, from the register the gripper reports (the profile was validated at open, so the maker's conversion always has an answer)."""
        opening = frames.opening_from_register(status.position, self._profile)
        assert opening is not None
        return opening

    def _present(self) -> Status:
        with self._lock:
            status = self._latest
        if status is None:
            raise BridgeRefusal("the gripper has not been read yet.", rhal.SKR_ETIMEDOUT)
        return status

    def _motion_status(self, args: dict) -> dict:
        self._no_arguments("status", args)
        status = self._present()
        with self._lock:
            image, exchanges, dead = self._image, self._exchanges, self._dead
        opening = frames.opening_from_register(status.position, self._profile)
        return {"model": self._model, "slave": self._slave, "link": self._link.target if self._link else None, "opening_m": opening, "limits_m": list(self._limits), "speed_m_s": self._speed, "effort": self._effort,
                "activation": frames.STA_NAMES[status.activation_state], "activated": status.activated, "go_to": status.go_to, "object": frames.OBJ_NAMES[status.object_detection],
                "gpo": status.position, "gpr": status.position_echo, "current_a": status.current * frames.AMPERES_PER_STEP,
                "fault": f"0x{status.fault_byte:02X}", "fault_class": frames.fault_class(status), "fault_text": self._describe_fault(status),
                "image": {"action": f"0x{image.action:02X}", "position": image.position, "speed": image.speed, "force": image.force},
                "exchanges": exchanges, "link_dead": dead, "stop_latched": self._estop, "simulator": self._simulated}

    def _handshake(self, timeout: float) -> dict:
        """The maker's `runActivationHandshake`: clear rACT (a reset, which clears the fault status), wait for the gripper to say it is not activated, set it, wait for gSTA to say it is complete. rGTO is clear throughout."""
        started = time.monotonic()
        self._send(lambda old: Command(0, old.position, self._speed_register, self._effort_register), budget=_COMMAND_BUDGET_S * 2)
        self._wait_for_status(lambda s: not s.activated, max(0.1, timeout / 2))
        self._send(lambda old: Command(ACT, old.position, self._speed_register, self._effort_register), budget=_COMMAND_BUDGET_S * 2)
        status = self._wait_for_status(lambda s: s.activation_state == frames.STA_COMPLETE, max(0.1, timeout - (time.monotonic() - started)))
        return {"result": "activated", "opening_m": frames.opening_from_register(status.position, self._profile), "seconds": round(time.monotonic() - started, 3)}

    def _motion_activate(self, args: dict) -> dict:
        timeout = self._number(args, "timeout", minimum=1.0, maximum=120.0) or _ACTIVATION_TIMEOUT_S
        self._no_arguments("activate", args)
        status = self._present()
        if frames.fault_class(status) in ("fault", "estop"):
            raise BridgeRefusal(f"the gripper reports {self._describe_fault(status)}: a major fault is not activated away. The `recover` motion is the maker's reset handshake and moves the fingers through their full range.", rhal.SKR_EPERM)
        if status.activation_state == frames.STA_COMPLETE:
            return {"result": "already_active"}
        if status.activation_state == frames.STA_IN_PROGRESS:
            self._wait_for_status(lambda s: s.activation_state == frames.STA_COMPLETE, timeout)
            return {"result": "activated"}
        return self._handshake(timeout)

    def _motion_recover(self, args: dict) -> dict:
        timeout = self._number(args, "timeout", minimum=1.0, maximum=120.0) or _ACTIVATION_TIMEOUT_S
        self._no_arguments("recover", args)
        self._present()
        return self._handshake(timeout)

    def _run_to(self, register: int, speed: float, effort: float, seconds: float) -> Status:
        speed_register, effort_register = frames.speed_to_register(speed, self._profile), frames.effort_to_register(effort)
        if speed_register is None or effort_register is None:
            raise BridgeRefusal("the speed or the effort has no register value.", rhal.SKR_EINVAL)
        self._gate()
        self._send(lambda old: Command(ACT | GTO, register, speed_register, effort_register))
        # The status must show the request taken (gGTO and gPR) and the fingers no longer moving: the maker's `waitForMotionEnd`, judged on the exchange that shows it.
        return self._wait_for_status(lambda s: s.go_to and s.position_echo == register and s.object_detection != frames.OBJ_MOVING or frames.fault_class(s) in ("fault", "estop"), seconds)

    def _motion_move(self, args: dict) -> dict:
        low, high = self._limits
        position = self._number(args, "position", required=True, minimum=low, maximum=high)
        profile = self._profile
        speed = self._number(args, "speed", minimum=profile.min_speed, maximum=profile.max_speed) or self._speed
        effort = self._number(args, "effort", minimum=0.0, maximum=1.0)
        self._no_arguments("move", args)
        assert position is not None
        register = frames.opening_to_register(position, profile)
        if register is None:
            raise BridgeRefusal(f"an opening of {position:.6f} m has no register in this profile.", rhal.SKR_EINVAL)
        travel = abs(position - self._opening_of(self._present()))
        status = self._run_to(register, speed, self._effort if effort is None else effort, travel / speed + 3.0)
        if frames.fault_class(status) in ("fault", "estop"):
            raise BridgeRefusal(f"the gripper reports {self._describe_fault(status)} during the move.", rhal.SKR_EIO)
        return {"object": frames.OBJ_NAMES[status.object_detection], "opening_m": frames.opening_from_register(status.position, profile)}

    def _motion_grip(self, args: dict) -> dict:
        profile = self._profile
        effort = self._number(args, "effort", required=True, minimum=0.0, maximum=1.0)
        speed = self._number(args, "speed", minimum=profile.min_speed, maximum=profile.max_speed) or self._speed
        self._no_arguments("grip", args)
        assert effort is not None
        low, _ = self._limits
        register = frames.opening_to_register(low, profile)
        assert register is not None
        travel = abs(self._opening_of(self._present()) - low)
        status = self._run_to(register, speed, effort, travel / speed + 3.0)
        if frames.fault_class(status) in ("fault", "estop"):
            raise BridgeRefusal(f"the gripper reports {self._describe_fault(status)} during the grip.", rhal.SKR_EIO)
        result = {frames.OBJ_CLOSING: "gripped", frames.OBJ_AT_POSITION: "closed_on_nothing", frames.OBJ_OPENING: "object_detected_opening"}.get(status.object_detection, "moving")
        return {"result": result, "opening_m": frames.opening_from_register(status.position, profile)}

    def _motion_release(self, args: dict) -> dict:
        profile = self._profile
        speed = self._number(args, "speed", minimum=profile.min_speed, maximum=profile.max_speed) or self._speed
        self._no_arguments("release", args)
        _, high = self._limits
        register = frames.opening_to_register(high, profile)
        assert register is not None
        travel = abs(high - self._opening_of(self._present()))
        status = self._run_to(register, speed, self._effort, travel / speed + 3.0)
        if frames.fault_class(status) in ("fault", "estop"):
            raise BridgeRefusal(f"the gripper reports {self._describe_fault(status)} during the release.", rhal.SKR_EIO)
        return {"result": "released" if status.object_detection == frames.OBJ_AT_POSITION else frames.OBJ_NAMES[status.object_detection], "opening_m": frames.opening_from_register(status.position, profile)}


#: One entry per motion name (§84.1).
_DESCRIPTORS: dict[str, dict] = {
    "status": {"name": "status", "parameters": []},
    "activate": {"name": "activate", "parameters": [{"name": "timeout", "unit": "s", "required": False}]},
    "recover": {"name": "recover", "parameters": [{"name": "timeout", "unit": "s", "required": False}]},
    "move": {"name": "move", "parameters": [{"name": "position", "unit": "m", "required": True}, {"name": "speed", "unit": "m/s", "required": False}, {"name": "effort", "required": False}]},
    "grip": {"name": "grip", "parameters": [{"name": "effort", "required": True, "min": 0.0}, {"name": "speed", "unit": "m/s", "required": False}]},
    "release": {"name": "release", "parameters": [{"name": "speed", "unit": "m/s", "required": False}]},
}


def _required_number(settings: dict, key: str, what: str, low: float, high: float) -> float:
    value = settings.get(key)
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value) or not low <= value <= high:
        raise BridgeRefusal(f"no `{key}` in the configuration, or {value!r} is not a number {what}.")
    return float(value)


def _bounded(settings: dict, key: str, default: float, low: float, high: float) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not low <= value <= high:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number from {low:g} to {high:g}.")
    return float(value)


def _say(message: str) -> None:
    print(f"skynet-robotiq: {message}", file=sys.stderr, flush=True)

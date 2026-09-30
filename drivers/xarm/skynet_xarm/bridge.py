"""The RHAL driver: a UFACTORY xArm, 5, 6 or 7 axes, over the controller's own wire.

§4's five mandatory entry points over :mod:`skynet_xarm.frames`. It links nothing of the vendor's:
UFACTORY publish their SDK's source (BSD-3-Clause), so every port, register and field offset was read
out of their own files, and `layout_check.py` opens them and requires the text to still be there.

# What it reads off the controller rather than out of a file

| From | Report field | Used for |
|---|---|---|
| the rich report (30002) | `robot_type`, `robot_axis`, `version` | which arm, and how many joints |
| the rich report | `reduced_joint_limits` | the descriptor's travel |
| the rich report | `joint_max_speed` | the descriptor's speed ceiling |
| the rich report | `is_simulation_robot` | the descriptor's `simulator` |
| the rich report | `error_code`, `motor_enable_status` | whether to come up at all |
| the fast report (30003) | `state_mode`, `actual_joint_angle` | every read |

**Travel is the controller's own configured range**, which is the narrowest the arm will be asked to
go: the controller's `reduced_joint_limits` are what it holds whether or not reduced mode is on, and
a range an operator narrowed is a range this driver must respect. The suite compares them with the
vendor's own `XCONF.Robot.JOINT_LIMITS` table for the same arm and refuses a disagreement.

**Whether it is a simulation is the controller's word.** `is_simulation_robot` is a field the
controller's firmware sets, and UFACTORY's simulator image sets it: a CiA 402 drive and a simulator
of one answer identically, and a controller that says which it is is the only such bus here that can.
`{"simulator": true}` in the configuration forces the answer the safe way; nothing forces it false.

# What it refuses to bring up

* **A latched error.** `open()` reads the rich report's error code first and refuses a controller that
  has one, for `drivers/canopen`'s reason: clearing a fault silently on every restart erases the one
  piece of evidence about why the robot stopped. The operator clears it at the controller.
* **An arm somebody else is moving.** State 1 at open is another client's motion.
* **Motors that will not enable.** `MOTION_EN` is sent only when the enable byte says an axis is off
  (the simulator never answers it, and its byte is already full), and the byte is read back rather than
  the reply trusted.
* **A stream that is not there.** No fast or rich report within the open timeout is a controller this
  driver has no state from, and a body with no state is a body that cannot be stopped by what it says.

# What a command does, and the one thing it will not send

`write_command` sends `MOVE_SERVOJ` in servo mode (mode 1) and reads the reply's state byte. A setpoint
outside a joint's range is **refused before it is sent**: the controller acknowledges such a target
with a clean reply, starts to move, and stops with an error a moment later (measured on the 2.4.0
firmware, DRY-COVERAGE-PLAN.md F175), so the reply cannot be what tells the OS the command was bad.
A masked joint holds the setpoint this driver last issued — not the measured angle, which sags — and
the first command of a session starts from the measurement.

# The stop

`emergency_stop` latches, and sends `SET_STATE 4` on a socket of its own opened at `open()`, so a stop
never waits for a command in flight and never interleaves its bytes with one. It is a software stop
and the descriptor declares it as one. It is not the controller's emergency-stop input, which is a
circuit; `safety.estop.independent` is a bench test nobody passes from here.
"""

from __future__ import annotations

import json
import math
import socket
import sys
import threading
import time

import skynet_rhal as rhal

from .frames import (
    CONTROL_PORT,
    VENDOR_JOINT_LIMITS,
    REPORT_REAL_PORT,
    REPORT_RICH_PORT,
    FastReport,
    Fun,
    ProtocolError,
    ReportStream,
    RichReport,
    State,
    parse_fast,
    parse_reply,
    parse_rich,
    request,
    servoj_data,
)

__all__ = ["XArmBridge", "BridgeRefusal"]

#: How long `open()` waits for each report stream's first frame.
_OPEN_TIMEOUT_S = 3.0
#: How long a control request waits for its reply.
_CONTROL_TIMEOUT_S = 1.0
#: How long the control loop's own request — a servo target — waits for its reply, and for a new connection if the last one was
#: lost: five periods at the stream's 100 Hz. Everything else (`open`, a state change, a release) may take the second above;
#: a loop that waited a second for one command would miss a hundred cycles behind a cable that came out (F194).
_CYCLIC_TIMEOUT_S = 0.05
#: How old the newest fast frame may be before a read says the controller has gone quiet. Ten frames at
#: 100 Hz: a stream that skipped a frame or two is a busy network, and one that skipped ten is not.
_STALE_S = 0.1
#: How long `MOTION_EN` and `release_stop` wait for the controller's own state to say they worked.
_SETTLE_S = 5.0
#: The fixed transaction id the stop frame carries; nothing reads the reply.
_STOP_TRANSACTION = 0xFFF0


class BridgeRefusal(Exception):
    """A reason this body did not come up, in the terms an integrator acts on."""


class Unreachable(BridgeRefusal):
    """The controller did not answer: a transport failure (`SKR_EIO`), not a body the driver refused."""


class _Reader(threading.Thread):
    """One report socket, read for as long as it lives, keeping the newest frame and when it arrived."""

    def __init__(self, sock: socket.socket, parse, name: str) -> None:
        super().__init__(daemon=True, name=name)
        self._sock = sock
        self._parse = parse
        self._stream = ReportStream()
        self._lock = threading.Lock()
        self._latest = None
        self._stamp_ns = 0
        self.dead: str | None = None
        self.frames = 0
        self._stopping = False

    def run(self) -> None:
        self._sock.settimeout(0.25)
        while not self._stopping:
            try:
                data = self._sock.recv(65536)
            except socket.timeout:
                continue
            except OSError as error:
                self.dead = f"the socket failed: {error}"
                return
            if not data:
                self.dead = "the controller closed the report socket"
                return
            acquired = rhal.acquisition_ns()
            try:
                frames = self._stream.feed(data)
                parsed = [self._parse(frame) for frame in frames]
            except ProtocolError as error:
                self.dead = str(error)
                return
            if parsed:
                with self._lock:
                    self._latest = parsed[-1]
                    self._stamp_ns = acquired
                    self.frames += len(parsed)

    def latest(self):
        with self._lock:
            return self._latest, self._stamp_ns

    def close(self) -> None:
        self._stopping = True
        try:
            self._sock.close()
        except OSError:
            pass


class XArmBridge:
    """A body made of one xArm's joints."""

    def __init__(self) -> None:
        self._simulated = True
        self._axis = 0
        self._descriptor: str | None = None
        self._seq = 0
        self._estop = False
        self._opened = False
        self._control: socket.socket | None = None
        self._control_address: tuple[str, int] | None = None
        #: The control socket timed out, lost its framing or failed: the next call opens a new one (F193).
        self._control_broken = False
        self._stop_socket: socket.socket | None = None
        self._transaction = 0
        self._control_lock = threading.Lock()
        self._fast: _Reader | None = None
        self._rich: _Reader | None = None
        self._limits: list[tuple[float, float]] = []
        self._joint_entries: list[dict] = []
        self._held: list[float] = []
        self._holding = False
        self._stop_frame = request(_STOP_TRANSACTION, Fun.SET_STATE, bytes([State.STOP]))
        self._control_timeout_s = _CONTROL_TIMEOUT_S
        self._stale_s = _STALE_S

    # ── §4's entry points ────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except json.JSONDecodeError as error:
            _say(f"the configuration is not JSON: {error}")
            return rhal.SKR_EINVAL
        # A body, and the stop latched on it, belong to one session (DRY-COVERAGE-PLAN.md F73, F76).
        self._teardown()
        self._estop = False
        self._descriptor = None
        try:
            self._open(settings)
        except Unreachable as refusal:
            self._teardown()
            _say(str(refusal))
            return rhal.SKR_EIO
        except BridgeRefusal as refusal:
            self._teardown()
            _say(str(refusal))
            return rhal.SKR_ENODEV
        except OSError as error:
            self._teardown()
            _say(f"the controller could not be reached: {error}")
            return rhal.SKR_EIO
        self._opened = True
        return rhal.SKR_OK

    def _open(self, settings: dict) -> None:
        host = settings.get("host")
        if not isinstance(host, str) or not host:
            raise BridgeRefusal(
                "no `host` in the configuration. This driver does not scan for a controller, and one "
                "that guessed an address would command whichever arm answered first. Give "
                '`{"host": "192.168.1.100", "joints": [{"name": "joint1", "role": "…"}, …]}`.'
            )
        ports = settings.get("ports") or {}
        control_port = int(ports.get("control", CONTROL_PORT))
        fast_port = int(ports.get("fast", REPORT_REAL_PORT))
        rich_port = int(ports.get("rich", REPORT_RICH_PORT))
        self._control_timeout_s = _bounded_seconds(settings, "control_timeout_ms", _CONTROL_TIMEOUT_S)
        self._stale_s = _bounded_seconds(settings, "stale_ms", _STALE_S)
        joints = settings.get("joints") or []
        if not joints:
            raise BridgeRefusal(
                "no joints in the configuration. The controller knows how many axes it has and nothing "
                "on it says what a joint is called or what it does for a policy: a role is a human's "
                'assertion (§63.4). Give `{"joints": [{"name": "joint1", "role": "right_arm.shoulder.yaw"}, …]}`.'
            )

        self._fast = self._report(host, fast_port, parse_fast, "fast (30003)")
        self._rich = self._report(host, rich_port, parse_rich, "rich (30002)")
        rich: RichReport = self._first(self._rich, "rich")
        self._first(self._fast, "fast")

        if rich.axis not in (5, 6, 7):
            raise BridgeRefusal(
                f"the controller reports {rich.axis} axes. This driver is written for the xArm's 5, 6 "
                "and 7-axis arms, whose reports it has read; another count is another robot."
            )
        if len(joints) != rich.axis:
            raise BridgeRefusal(
                f"the configuration names {len(joints)} joints and the controller reports "
                f"{rich.axis} axes. A joint the OS names and the arm does not have is a channel no "
                "policy could ever move."
            )
        for entry in joints:
            if not entry.get("name"):
                raise BridgeRefusal("a joint has no `name`; a joint the OS cannot name is one no policy channel can bind to.")
        self._axis = rich.axis

        limits = list(rich.joint_limits[: rich.axis])
        for index, (low, high) in enumerate(limits):
            if not (math.isfinite(low) and math.isfinite(high) and low < high):
                raise BridgeRefusal(
                    f"the controller reports {low}..{high} rad as joint {index + 1}'s range. That is not "
                    "a range, and a driver that made one up would bound the arm with a number nobody "
                    "measured (§5.2)."
                )
        self._limits = self._cross_check_limits(rich, limits)
        if not (math.isfinite(rich.joint_max_speed) and rich.joint_max_speed > 0):
            raise BridgeRefusal(f"the controller reports a joint speed ceiling of {rich.joint_max_speed} rad/s.")
        self._simulated = bool(settings.get("simulator", False)) or rich.is_simulation
        self._joint_entries = joints

        self._control = self._connect(host, control_port, "control")
        self._control_address = (host, control_port)
        self._control_broken = False
        self._stop_socket = self._connect(host, control_port, "stop")
        # Send-only, and never blocking: a stop that waited for buffer space would be a stop that waited.
        self._stop_socket.setblocking(False)
        self._check_version(rich)

        if rich.error_code:
            raise BridgeRefusal(
                f"the controller holds error {rich.error_code} (warning {rich.warning_code}). This body "
                "does not come up over a latched error: clear it at the controller deliberately, after "
                "reading why — a driver that cleaned it here would restart a machine whose reason for "
                "stopping nobody had looked at, and would do it again on the next power cycle."
            )
        if rich.state == State.MOVING:
            raise BridgeRefusal(
                "the arm is moving and this session did not move it. Somebody else is commanding the "
                "controller, and two masters is two setpoint streams into one arm."
            )
        self._enable_motors(rich)
        self._enter_servo_mode()

        fast: FastReport = self._first(self._fast, "fast")
        self._held = list(fast.angles[: self._axis])
        self._holding = False
        self._descriptor = self._build_descriptor(rich)

    def _cross_check_limits(self, rich: RichReport, reported: list[tuple[float, float]]) -> list[tuple[float, float]]:
        """The controller's ranges, held against UFACTORY's own table for the same arm; the narrower of the two wins.

        The controller says what range it holds, and the vendor's SDK says what range a client may command for this
        arm (`VENDOR_JOINT_LIMITS`). They agree on a real arm configured as shipped, and they do not always: the
        xArm 5, Lite 6, 850 and xArm 7 firmwares in UFACTORY's simulator image all report the xArm 6's ranges, the
        wrist bend on the wrong joint (F181). A driver that believed the controller would allow joint four of an xArm 5
        the full turn its vendor's table gives to joint five, and the firmware acknowledges a target beyond a joint's
        range and faults afterwards (F175). So travel is the intersection: the arm is never asked for more than either
        source allows, an operator's narrower reduced mode still narrows it, and the disagreement is said aloud.

        An arm the SDK's table does not list has one source, which is said too. Two sources with nothing in common
        are a controller this driver has misread, and it will not command what it has misread.
        """
        vendor = VENDOR_JOINT_LIMITS.get((rich.axis, rich.robot_type))
        if vendor is None:
            _say(
                f"UFACTORY's SDK has no joint table for an arm of {rich.axis} axes and type {rich.robot_type}, so the "
                "controller's own ranges are the only source and nothing cross-checks them."
            )
            return reported
        merged: list[tuple[float, float]] = []
        wider: list[str] = []
        for index, ((low, high), (vendor_low, vendor_high)) in enumerate(zip(reported, vendor), start=1):
            # The firmware rounds what it reports to a milliradian (F176); a difference that small is the rounding.
            if low < vendor_low - 1e-3 or high > vendor_high + 1e-3:
                wider.append(f"joint {index}: the controller allows [{low:.3f}, {high:.3f}] and the vendor's table [{vendor_low:.3f}, {vendor_high:.3f}]")
            narrow = (max(low, vendor_low), min(high, vendor_high))
            if not narrow[0] < narrow[1]:
                raise BridgeRefusal(
                    f"joint {index}'s range is [{low:.3f}, {high:.3f}] on the controller and [{vendor_low:.3f}, {vendor_high:.3f}] in "
                    f"UFACTORY's own table for an xArm of {rich.axis} axes and type {rich.robot_type}, and the two have nothing in common. "
                    "One of them is about another arm, and this driver will not command an arm it may have misread."
                )
            merged.append(narrow)
        if wider:
            _say(
                f"the controller's joint ranges are wider than UFACTORY's own table for an xArm of {rich.axis} axes and type "
                f"{rich.robot_type} on {len(wider)} joint(s), so the narrower of the two is the travel: " + "; ".join(wider) + "."
            )
        return merged

    def close(self) -> int:
        if self._control is not None:
            try:
                # A session ends with the arm stopped where it is, not still taking servo targets: the
                # controller keeps its motors on and holds, and the next `open` runs it again.
                self._call(Fun.SET_STATE, bytes([State.STOP]))
            except (BridgeRefusal, OSError):
                pass  # closing a body whose controller has gone away is still closing it
        self._teardown()
        self._opened = False
        return rhal.SKR_OK

    def read_state(self, out: rhal.State, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._fast is None or self._rich is None:
            return rhal.SKR_EPERM
        fast, stamp = self._fast.latest()
        rich, _ = self._rich.latest()
        self._seq += 1
        out.seq = self._seq
        out.flags = rhal.HOMING_NOT_REQUIRED
        out.fault_code = 0
        if fast is None:
            return rhal.SKR_ETIMEDOUT
        # §24: stamped at acquisition, which for a pushed stream is the arrival of the newest frame.
        out.stamp_ns = stamp
        out.position_rad = list(fast.angles[: self._axis])
        # **NaN, not zero.** The fast report carries no joint velocity and no measured torque — its
        # `estimated_joint_torque` is an estimate the controller derives from motor current, which a
        # simulator has no motor to derive — so neither is a quantity this driver measures (§24).
        out.velocity_rad_s = [math.nan] * self._axis
        out.effort_nm = [math.nan] * self._axis
        if self._estop or fast.state == State.STOPPED:
            out.flags |= rhal.FLAG_ESTOP
        if rich is not None and rich.error_code:
            out.flags |= rhal.FLAG_FAULT
            out.fault_code = rich.error_code
        age = (rhal.acquisition_ns() - stamp) / 1e9
        if self._fast.dead or age > self._stale_s:
            out.flags |= rhal.FLAG_DEGRADED
            return rhal.SKR_ETIMEDOUT
        return rhal.SKR_OK

    def write_command(self, cmd: rhal.Command, deadline_ns: int) -> int:
        del deadline_ns
        if not self._opened or self._fast is None:
            return rhal.SKR_EPERM
        if cmd.joint_count != self._axis:
            return rhal.SKR_EINVAL
        if cmd.mode != rhal.MODE_POSITION:
            # Servo mode takes joint angles. Velocity and torque are other modes of the controller,
            # unmeasured here, and shipping them untested is how a torque command becomes a position one.
            return rhal.SKR_EINVAL
        if self._estop:
            return rhal.SKR_EPERM
        fast, _ = self._fast.latest()
        if fast is not None and fast.state == State.STOPPED:
            return rhal.SKR_EPERM

        # Every setpoint is checked before any frame goes out, and nothing is stored until all of them
        # pass (F73's shape). The range check is the one that matters: see the module docstring.
        targets = list(self._held)
        for index in range(self._axis):
            applied = cmd.mask[index] if index < len(cmd.mask) else 1
            if not applied:
                continue
            value = cmd.values[index]
            low, high = self._limits[index]
            if not math.isfinite(value) or not low <= value <= high:
                return rhal.SKR_EINVAL
            targets[index] = value
        try:
            reply = self._call(Fun.MOVE_SERVOJ, servoj_data(targets + [0.0] * (7 - self._axis)), timeout=_CYCLIC_TIMEOUT_S)
        except BridgeRefusal as refusal:
            _say(str(refusal))
            return rhal.SKR_EIO
        except OSError as error:
            _say(f"the control socket failed: {error}")
            return rhal.SKR_EIO
        if reply.error:
            return rhal.SKR_EIO
        if reply.invalid:
            return rhal.SKR_EINVAL
        if reply.not_ready:
            return rhal.SKR_EPERM
        self._held = targets
        self._holding = True
        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        self._estop = True
        sock = self._stop_socket
        if sock is None:
            return rhal.SKR_OK
        try:
            # Never waits, and never on the control socket's lock: a command in flight cannot delay it
            # and its bytes cannot land in the middle of one.
            sock.send(self._stop_frame)
        except OSError:
            pass  # the latch is set; a socket that failed here is one `read_state` will report on
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch and run the arm again (§48.2), or say it will not run.

        The stop left the controller in state 4, where it refuses servo targets — so clearing the latch
        alone would accept commands the controller never obeys. `SET_STATE 0` runs it, and the state is
        read back off the stream rather than trusted from the reply. An error the controller itself
        holds is never cleared here: it is the evidence of why the arm stopped. A failure re-latches
        and answers EIO, so a cleared latch never claims a body that is not commandable.
        """
        self._estop = False
        if self._control is None or self._fast is None or self._rich is None:
            return rhal.SKR_OK
        rich, _ = self._rich.latest()
        if rich is not None and rich.error_code:
            self._estop = True
            _say(
                f"the controller holds error {rich.error_code}, which a session's stop does not clear. "
                "Read it, clear it at the controller, and open a new session."
            )
            return rhal.SKR_EIO
        try:
            self._call(Fun.SET_STATE, bytes([State.RUN]))
        except (BridgeRefusal, OSError) as error:
            self._estop = True
            _say(f"the controller did not take SET_STATE 0: {error}")
            return rhal.SKR_EIO
        deadline = time.monotonic() + _SETTLE_S
        while time.monotonic() < deadline:
            fast, _ = self._fast.latest()
            if fast is not None and fast.state in (State.READY, State.MOVING) and fast.mode == 1:
                self._held = list(fast.angles[: self._axis])
                self._holding = False
                return rhal.SKR_OK
            time.sleep(0.01)
        self._estop = True
        _say("the controller did not report a runnable state after the stop was released.")
        return rhal.SKR_EIO

    def describe(self) -> str:
        return self._descriptor or "{}"

    def read_sensor(self, name: str, out: rhal.Frame) -> int:
        del name, out
        return rhal.SKR_ENOSYS

    # ── the controller ───────────────────────────────────────────────────────

    def _connect(self, host: str, port: int, what: str, timeout: float = _OPEN_TIMEOUT_S) -> socket.socket:
        try:
            sock = socket.create_connection((host, port), timeout=timeout)
        except OSError as error:
            raise Unreachable(
                f"the {what} port ({host}:{port}) did not answer: {error}. A controller that is off, "
                "one on another address and one whose firewall drops the port look the same from here."
            ) from error
        sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        return sock

    def _report(self, host: str, port: int, parse, what: str) -> _Reader:
        reader = _Reader(self._connect(host, port, what), parse, f"xarm-{what}")
        reader.start()
        return reader

    def _first(self, reader: _Reader, what: str):
        deadline = time.monotonic() + _OPEN_TIMEOUT_S
        while time.monotonic() < deadline:
            frame, _ = reader.latest()
            if frame is not None:
                return frame
            if reader.dead:
                raise Unreachable(f"the {what} report stream ended before its first frame: {reader.dead}")
            time.sleep(0.005)
        raise Unreachable(
            f"no {what} report frame arrived within {_OPEN_TIMEOUT_S:.0f}s. A body with no state from its "
            "controller is one nothing can be measured or stopped by."
        )

    def _call(self, function: int, data: bytes = b"", timeout: float | None = None):
        if self._control is None:
            raise BridgeRefusal("the control socket is not open")
        # The configured timeout is a ceiling for the slow calls; the control loop's asks for less.
        wait = self._control_timeout_s if timeout is None else min(timeout, self._control_timeout_s)
        with self._control_lock:
            if self._control_broken:
                self._reconnect_control(wait)
            sock = self._control
            assert sock is not None
            self._transaction = self._transaction % 0xFFEF + 1
            transaction = self._transaction
            sock.settimeout(wait)
            try:
                sock.sendall(request(transaction, function, data))
                header = _read_exactly(sock, 6)
                (length,) = (int.from_bytes(header[4:6], "big"),)
                body = _read_exactly(sock, length)
                return parse_reply(header, body, transaction=transaction, function=function)
            except socket.timeout as error:
                self._control_broken = True
                raise BridgeRefusal(
                    f"the controller did not answer register {function} within "
                    f"{wait:.2f}s. The firmware answers nothing it does not know, and "
                    "nothing at all while it is busy with something it cannot interrupt. The next call "
                    "uses a new connection, because the answer may still come and would be read as the next one's."
                ) from error
            except ProtocolError as error:
                self._control_broken = True
                raise BridgeRefusal(f"register {function}: {error}") from error
            except OSError:
                self._control_broken = True
                raise

    def _reconnect_control(self, wait: float) -> None:
        """Replace a control socket that timed out, lost its framing or failed, rather than read on from it (F193).

        A reply that arrives after its request timed out is still in the stream, and so is the rest of a frame that was half
        read. Reading on would take that for the answer to the *next* request, and every request after it would be one reply
        behind for the rest of the session: each call would consume somebody else's reply and leave its own unread. A new
        connection has no such history. The controller's mode and state are the controller's, not the connection's, so
        nothing is lost by it.
        """
        assert self._control_address is not None
        try:
            if self._control is not None:
                self._control.close()
        except OSError:
            pass
        self._control = self._connect(*self._control_address, "control", timeout=wait)
        self._control_broken = False

    def _check_version(self, rich: RichReport) -> None:
        reply = self._call(Fun.GET_VERSION)
        text = reply.payload.split(b"\0", 1)[0].decode("ascii", "replace")
        fields = [part.strip() for part in text.split(",")]
        if len(fields) < 5 or fields[0] != str(rich.axis) or fields[-1] != rich.version:
            raise BridgeRefusal(
                f"the control port says `{text}` and the rich report says {rich.axis} axes on "
                f"`{rich.version}`. Two answers about one controller that disagree are a controller "
                "this driver has misread, and it will not command what it has misread."
            )

    def _enable_motors(self, rich: RichReport) -> None:
        wanted = (1 << rich.axis) - 1
        if rich.motors_enabled & wanted == wanted:
            return  # already on: the simulator never answers MOTION_EN, and a real arm need not be asked
        try:
            self._call(Fun.MOTION_EN, bytes([8, 1]))
        except BridgeRefusal:
            pass  # the byte is what says, below — a reply that never came is not a refusal by itself
        deadline = time.monotonic() + _SETTLE_S
        while time.monotonic() < deadline:
            current, _ = self._rich.latest()  # type: ignore[union-attr]
            if current is not None and current.motors_enabled & wanted == wanted:
                return
            time.sleep(0.02)
        raise BridgeRefusal(
            f"the controller's motor enable byte is {rich.motors_enabled:#04x} and stayed short of "
            f"{wanted:#04x} for {_SETTLE_S:.0f}s after every axis was asked to enable. An arm whose "
            "motors are off takes servo targets and does nothing with them."
        )

    def _enter_servo_mode(self) -> None:
        self._call(Fun.SET_MODE, bytes([1]))
        self._call(Fun.SET_STATE, bytes([State.RUN]))
        deadline = time.monotonic() + _SETTLE_S
        assert self._fast is not None
        while time.monotonic() < deadline:
            fast, _ = self._fast.latest()
            if fast is not None and fast.mode == 1 and fast.state in (State.READY, State.MOVING):
                return
            time.sleep(0.01)
        fast, _ = self._fast.latest()
        raise BridgeRefusal(
            f"the controller was asked for servo mode and reports mode {getattr(fast, 'mode', '?')} in "
            f"state {getattr(fast, 'state', '?')}. Servo mode is the one where a target is followed as "
            "it arrives; a profile mode plans its own trajectory between targets, which is a second "
            "interpolator that does not know the Safety Kernel exists."
        )

    def _teardown(self) -> None:
        for reader in (self._fast, self._rich):
            if reader is not None:
                reader.close()
        for sock in (self._control, self._stop_socket):
            if sock is not None:
                try:
                    sock.close()
                except OSError:
                    pass
        self._fast = self._rich = None
        self._control = self._stop_socket = None

    # ── the descriptor ───────────────────────────────────────────────────────

    def _build_descriptor(self, rich: RichReport) -> str:
        joints = []
        for entry, (low, high) in zip(self._joint_entries, self._limits):
            joints.append(
                {
                    "name": entry["name"],
                    "role": entry.get("role"),
                    "type": "revolute",
                    "limits": {
                        "position_min_rad": low,
                        "position_max_rad": high,
                        "velocity_max_rad_s": rich.joint_max_speed,
                    },
                    "control_modes": ["position"],
                }
            )
        descriptor = {
            "schema_version": 1,
            "robot_slug": f"xarm{rich.axis}-type{rich.robot_type}",
            "digest": None,
            "verified": False,
            "simulator": self._simulated,
            "base": "fixed",
            "dof": rich.axis,
            "joints": joints,
            "end_effectors": [],
            "sensors": [],
            "control": {"modes": ["position"], "rate_hz_max": 100, "rate_hz_min": 1},
            # Null, not an empty object: nobody has declared a safety envelope for this body, and the
            # controller's own ranges are the controller's rather than a risk assessment's. No torque
            # rating either — the controller publishes none this driver has read.
            "envelope": None,
            "capabilities": ["safety.estop"],
            "extensions": [],
            "solvers": [],
        }
        return json.dumps(descriptor, separators=(",", ":"))


def _read_exactly(sock: socket.socket, count: int) -> bytes:
    buffer = b""
    while len(buffer) < count:
        chunk = sock.recv(count - len(buffer))
        if not chunk:
            raise OSError("the controller closed the control socket")
        buffer += chunk
    return buffer


def _bounded_seconds(settings: dict, key: str, default: float) -> float:
    value = settings.get(key)
    if value is None:
        return default
    if isinstance(value, bool) or not isinstance(value, (int, float)) or not 0 < value <= 30000:
        raise BridgeRefusal(f"`{key}` of {value!r} is not a number of milliseconds above zero and at most 30000.")
    return float(value) / 1000.0


def _say(message: str) -> None:
    print(f"skynet-xarm: {message}", file=sys.stderr, flush=True)

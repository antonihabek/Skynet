"""A Boston Dynamics Spot over its gRPC SDK, through `python-host`.

`DRY-COVERAGE-PLAN.md`'s Phase 7 quadruped row: `bosdyn-client` 5.2.0 is public on PyPI and
there is no vendor simulator, so the substrate is faked gRPC services with the vendor's client
unmodified (the Pudu shape) — `fake_spot.py` beside this file. Spot is §59.2's genuine class:
the vendor controller owns the servo loop and the API is command-level, so this driver reads
twelve joints and moves nothing but task motions. Guest ceiling (§31.3), and the class's own.

What the driver implements, service by service:

    open() bring-up  Auth PgAuthToken, then directory sync, an estop endpoint with a
                     keepalive, a body lease with a keepalive, background time sync, and
                     one state read (the joint names the descriptor is built from)
    read_state       RobotState: twelve joint positions/velocities by name, vision-frame
                     body pose and twist when published, the fault count, the estop flag
    power_on/off     PowerCommand REQUEST_ON/OFF_MOTORS with the lease, feedback polled once
    stand/sit        synchronized mobility commands, vendor builders, vendor defaults
    stop/selfright   full-body stop / self-right, same
    emergency_stop   estop CUT, keepalives stopped, latched locally

`write_command` refuses everything with EPERM after the shape checks: there is no joint-level
command access and §59.2 says there never will be in software. The only route that moves this
robot is `extension` under `vendor.spot.task`.

Three limitations are the vendor's, not this driver's, and all three are stated wherever they
bite:

    * Boston Dynamics publishes no joint travel table for Spot, so the descriptor carries no
      per-joint limits. A limit this driver invented would be §63.4's exact violation.
    * `JointState.load` has no documented unit, so effort reads NaN (§24's rule that an
      unmeasured quantity is NaN and never zero). Position and velocity are radians.
    * Faults are reported, never cleared. Clearing a behaviour fault is an operator act on a
      real machine (`ClearBehaviorFault` exists and this driver does not call it). What the
      state says is read in the four classes of `drivers/fault-vocabulary.json` (`_judge`):
      an informational system fault is nothing, a warning is DEGRADED, and a critical or
      ungraded one, a behaviour fault, motors in error or an estop that is not released are
      not-doing-what-it-is-told or stopped, with DEGRADED beside FAULT and ESTOP because §27
      reads that bit. The fault word is a system fault's own `code`, a behaviour fault's cause
      under the top bit, a motor power state under the next. The motions that bring the body
      up or take it somewhere new (`power_on`, `stand`, `selfright`) are refused while it is
      faulted, and every motion while its estop is not released (`_gate`).

The SDK's own licence is proprietary (Boston Dynamics' Development Kit Licence,
`Other/Proprietary License` in the package metadata). Nothing of it is copied into this tree:
the driver imports it the way an application imports any vendor SDK, the fake imports only its
message shapes, and the suite refuses without the project environment that carries it. That
refusal is the licence boundary made executable — CI has no network and no SDK, so CI does not
run this row.
"""

from __future__ import annotations

import json
import math
import sys
import threading
import time

import skynet_rhal as rhal

TOKEN = "vendor.spot.task"

#: The joint order this driver reads and describes in: the vendor's enumeration, front-left,
#: front-right, hind-left, hind-right, each hip-x, hip-y, knee. Mapping is strictly by name
#: (the suite shuffles the fake's serving order against it); this order is the driver's own
#: canonical choice and the descriptor's.
JOINTS = (
    "fl.hx", "fl.hy", "fl.kn",
    "fr.hx", "fr.hy", "fr.kn",
    "hl.hx", "hl.hy", "hl.kn",
    "hr.hx", "hr.hy", "hr.kn",
)

#: §5.1 roles in the Go2's words (`drivers/unitree`): side leg, hip/thigh/calf, the `#`
#: ordinal separating front from hind. The vocabulary has two legs and the robot four;
#: ordinals are what the grammar gives for that, and the Go2 already spent them.
ROLES = (
    "left_leg.hip.abduct#0", "left_leg.thigh.pitch#0", "left_leg.calf.flex#0",
    "right_leg.hip.abduct#0", "right_leg.thigh.pitch#0", "right_leg.calf.flex#0",
    "left_leg.hip.abduct#1", "left_leg.thigh.pitch#1", "left_leg.calf.flex#1",
    "right_leg.hip.abduct#1", "right_leg.thigh.pitch#1", "right_leg.calf.flex#1",
)

#: The estop endpoint's timeout, seconds. The keepalive checks in at a third of this; a
#: silent endpoint estops the robot past it (the deadman the fake models, the robot owns).
_ESTOP_TIMEOUT_S = 5.0

#: One RPC's budget while a session is being brought up or torn down. Loopback answers in
#: milliseconds and a real Spot on the dock network in tens of them; ten seconds is the
#: network-is-dead verdict, not a wait. It is not the budget of the state path, of a motion or of
#: the stop, which are below: the link-chaos contract (DRY-COVERAGE-PLAN.md 12.C.4) measured a read
#: that waited this out, a motion that waited it out twice (the gate's state read and the command),
#: and a stop that returned after twenty-nine seconds.
_RPC_TIMEOUT_S = 10.0

#: A read changes nothing, so one that is given up is asked again by the next. A motion's own state
#: read (the gate) has the same budget, and its command has three seconds on a LAN that answers in
#: tens of milliseconds: `stand`, `sit` and `stop` are idempotent, and a command that is not
#: accepted inside it is the robot not answering.
_READ_TIMEOUT_S = 1.5
_COMMAND_TIMEOUT_S = 3.0

#: The CUT is told to the robot from the stop's own thread, so `emergency_stop` returns with the latch
#: set and does not wait for the link it is stopping through. A CUT that was lost is sent again, and
#: the silent endpoint behind it is the deadman either way.
_CUT_TIMEOUT_S = 3.0
_CUT_ATTEMPTS = 3
_CUT_RETRY_S = 0.2
_STOP_JOIN_S = 6.0


class SpotError(Exception):
    """An SDK call that failed, with the words the driver answers in."""


class SpotDenied(SpotError):
    """The robot refused a motion its state forbids (unpowered, faulted): EPERM-shaped,
    because the state forbids it rather than the wire failing it."""


def _need_sdk():
    """The vendor client, imported late so the module's absence fails at `open()` with a
    sentence rather than at `import` with a traceback."""
    try:
        import bosdyn.client
        import bosdyn.client.estop
        import bosdyn.client.lease
        import bosdyn.client.power
        import bosdyn.client.robot_command
        import bosdyn.client.robot_state
        return bosdyn.client
    except ImportError as error:
        raise SpotError(
            "bosdyn-client is not importable. This row needs the project environment: "
            "see the README.") from error


class SpotRobot:
    """One Spot's SDK session: bring-up order, calls, teardown. The driver owns one and
    never touches the SDK past it."""

    def __init__(self, host: str, port: int, username: str, password: str,
                 ca_pem: str | None, estop_timeout: float = _ESTOP_TIMEOUT_S) -> None:
        self._host = host
        self._port = port
        self._estop_timeout = estop_timeout
        self._username = username
        self._password = password
        self._ca_pem = ca_pem
        self._robot = None
        self._estop_endpoint = None
        self._estop_alive = None
        self._lease = None
        self._lease_client = None
        self._lease_alive = None
        self._commands = None
        self._states = None
        self._power = None

    def connect(self) -> None:
        sdk_mod = _need_sdk()
        from bosdyn.client.estop import EstopClient, EstopEndpoint, EstopKeepAlive
        from bosdyn.client.lease import LeaseClient, LeaseKeepAlive
        from bosdyn.client.power import PowerClient
        from bosdyn.client.robot_command import RobotCommandClient
        from bosdyn.client.robot_state import RobotStateClient

        # Every piece is kept the moment it exists, not when bring-up is over: a failure
        # half way (a service the robot does not list, a clock that will not sync) must
        # find the keepalives, the lease and the endpoint to let go of. Assigned last, they
        # were unreachable from the failure path, which returned nothing and left the lease
        # held and both keepalives running for a driver that reported it never opened (F190).
        try:
            sdk = sdk_mod.create_standard_sdk("skynet-spot",
                                              cert_resource_glob=self._ca_pem)
            self._robot = sdk.create_robot(self._host)
            self._robot.update_secure_channel_port(self._port)
            self._robot.authenticate(self._username, self._password,
                                     timeout=_RPC_TIMEOUT_S)
            self._robot.sync_with_directory()
            estop = self._robot.ensure_client(EstopClient.default_service_name)
            self._estop_endpoint = EstopEndpoint(estop, "skynet-spot", self._estop_timeout)
            self._estop_endpoint.force_simple_setup()
            self._estop_alive = EstopKeepAlive(self._estop_endpoint)
            self._lease_client = self._robot.ensure_client(LeaseClient.default_service_name)
            self._lease = self._lease_client.acquire("body")
            self._lease_alive = LeaseKeepAlive(self._lease_client)
            # Time sync before the command client exists: the client inherits the
            # endpoint at creation, and a command client born without one refuses
            # every command with NoTimeSyncError.
            self._robot.time_sync.wait_for_sync(timeout_sec=_RPC_TIMEOUT_S)
            self._commands = self._robot.ensure_client(RobotCommandClient.default_service_name)
            self._states = self._robot.ensure_client(RobotStateClient.default_service_name)
            self._power = self._robot.ensure_client(PowerClient.default_service_name)
        except SpotError:
            self.shutdown()
            raise
        except Exception as error:
            self.shutdown()
            raise SpotError(f"bring-up failed: {error}") from error

    def state(self):
        try:
            return self._states.get_robot_state(timeout=_READ_TIMEOUT_S)
        except Exception as error:
            raise SpotError(f"state read failed: {error}") from error

    def power(self, on: bool) -> None:
        from bosdyn.api import power_pb2
        want = (power_pb2.PowerCommandRequest.REQUEST_ON_MOTORS if on else
                power_pb2.PowerCommandRequest.REQUEST_OFF_MOTORS)
        try:
            issued = self._power.power_command(want, lease=self._lease.lease_proto,
                                               timeout=_COMMAND_TIMEOUT_S)
            answered = self._power.power_command_feedback(issued.power_command_id,
                                                          timeout=_COMMAND_TIMEOUT_S)
            # Polled once, and the answer is read: in progress is a motor coming up and
            # success is one that is up, and everything else is the robot saying it will
            # not (faulted, estopped, no battery, no licence) — a driver that took the ack
            # for the outcome would report a powered body that is not (F175's shape; F191).
            if answered not in (power_pb2.STATUS_SUCCESS, power_pb2.STATUS_IN_PROGRESS):
                raise SpotError(f"the robot answered "
                                f"{power_pb2.PowerCommandStatus.Name(answered)}")
        except Exception as error:
            raise SpotError(f"power {'on' if on else 'off'} refused: {error}") from error

    def _motion(self, kind: str, command) -> int:
        from bosdyn.client.robot_command import BehaviorFaultError, NotPoweredOnError
        try:
            return self._commands.robot_command(
                command, lease=self._lease.lease_proto,
                timesync_endpoint=self._commands.timesync_endpoint,
                timeout=_COMMAND_TIMEOUT_S)
        except NotPoweredOnError as error:
            raise SpotDenied(f"{kind} refused: the motors are off; power_on first."
                             ) from error
        except BehaviorFaultError as error:
            raise SpotDenied(f"{kind} refused: the robot is faulted and clearing it "
                             f"is an operator act.") from error
        except Exception as error:
            raise SpotError(f"{kind} refused: {error}") from error

    def stand(self) -> int:
        from bosdyn.client.robot_command import RobotCommandBuilder
        return self._motion("stand", RobotCommandBuilder.synchro_stand_command())

    def sit(self) -> int:
        from bosdyn.client.robot_command import RobotCommandBuilder
        return self._motion("sit", RobotCommandBuilder.synchro_sit_command())

    def stop(self) -> int:
        from bosdyn.client.robot_command import RobotCommandBuilder
        return self._motion("stop", RobotCommandBuilder.stop_command())

    def selfright(self) -> int:
        from bosdyn.client.robot_command import RobotCommandBuilder
        return self._motion("selfright", RobotCommandBuilder.selfright_command())

    def cut(self) -> None:
        """Best-effort CUT, sent a few times if it is lost. Allowed to fail: the caller latches
        regardless, and the deadman behind the stopped keepalives is the second stop."""
        endpoint = self._estop_endpoint
        for attempt in range(_CUT_ATTEMPTS):
            try:
                endpoint.stop(timeout=_CUT_TIMEOUT_S)
                return
            except Exception as error:
                if attempt == _CUT_ATTEMPTS - 1:
                    print(f"skynet-spot: the CUT did not land in {_CUT_ATTEMPTS} tries, the last "
                          f"{error}. The latch holds and the deadman behind the stopped "
                          f"keepalives stops the robot.", file=sys.stderr, flush=True)
                    return
                time.sleep(_CUT_RETRY_S)

    def drop_liveness(self) -> None:
        """Stop the keepalives and leave everything else: reads continue, motions do
        not (the driver's latch), and the silent endpoint is the deadman behind the
        CUT. `shutdown()` repeats two of these calls; both shutdowns are idempotent."""
        if self._estop_alive is not None:
            self._estop_alive.shutdown()
            self._estop_alive = None
        if self._lease_alive is not None:
            self._lease_alive.shutdown()
            self._lease_alive = None

    def shutdown(self) -> None:
        self.drop_liveness()
        if self._lease is not None and self._lease_client is not None:
            try:
                self._lease_client.return_lease(self._lease)
            except Exception as error:
                print(f"skynet-spot: returning the lease failed: {error}. It expires "
                      f"unretained.", file=sys.stderr, flush=True)
            self._lease = None
        if self._estop_endpoint is not None:
            # Deregister only with the motors off: the robot refuses config surgery
            # while powered (STATUS_MOTORS_ON), and a silent-but-registered endpoint
            # is the deadman that safes the machine, not litter.
            powered = False
            try:
                powered = _motor_on(self.state())
            except SpotError:
                pass
            if not powered:
                try:
                    self._estop_endpoint.deregister()
                except Exception as error:
                    print(f"skynet-spot: deregistering the endpoint failed: {error}.",
                          file=sys.stderr, flush=True)
            else:
                print("skynet-spot: the endpoint stays registered and silent with the "
                      "motors on: its timeout, not this process, stops the robot.",
                      file=sys.stderr, flush=True)
            self._estop_endpoint = None
        self._teardown()

    def _teardown(self) -> None:
        if self._robot is not None:
            try:
                self._robot.stop_time_sync()
            except Exception:
                pass
            self._robot = None
        self._commands = self._states = self._power = None
        self._lease_client = None


def _motor_on(state) -> bool:
    from bosdyn.api import robot_state_pb2
    values = robot_state_pb2.PowerState.DESCRIPTOR.fields_by_name[
        "motor_power_state"].enum_type.values_by_name
    on = values["STATE_ON"].number
    return state.power_state.motor_power_state == on


#: What a robot's own state says about itself, in the four classes of `drivers/fault-vocabulary.json` (DRY-COVERAGE-PLAN.md 12.C.6). §27 leaves
#: `present` only on RHAL_DEGRADED and treats only RHAL_ESTOP as a stop, so what a driver reports a state *as* is what the state does to the OS.
_OK, _DEGRADED, _FAULT, _ESTOP = range(4)
_FLAGS = {
    _OK: 0,
    _DEGRADED: rhal.FLAG_DEGRADED,
    _FAULT: rhal.FLAG_FAULT | rhal.FLAG_DEGRADED,
    _ESTOP: rhal.FLAG_ESTOP | rhal.FLAG_FAULT | rhal.FLAG_DEGRADED,
}
#: `SystemFault.Severity`: unknown, info ("No hardware problem"), warn ("Robot performance may be degraded"), critical. A value not here is
#: read as the worst the maker defines.
_SEVERITY_CLASS = {0: _FAULT, 1: _OK, 2: _DEGRADED, 3: _FAULT}
#: `PowerState.MotorPowerState`: unknown, off, on, powering on, powering off, error. Only the error state (and a value nobody has described)
#: is a fault.
_MOTOR_CLASS = {0: _FAULT, 1: _OK, 2: _OK, 3: _OK, 4: _OK, 5: _FAULT}
#: A fault word has one number to say which fault: a system fault's own `code` in the low thirty bits; for a behaviour fault, which has no code
#: of the maker's, the cause under the top bit; a motor power state under the next one down.
_CODE_MASK = 0x3FFFFFFF
_BEHAVIOR_MARK = 0x80000000
_MOTOR_MARK = 0x40000000
#: The motions that bring the body up or take it somewhere new, which the driver does not send while the robot says it is not doing what it is
#: told. `stop`, `sit` and `power_off` are the way out of a fault and stay available (the robot refuses them itself when it must: a behaviour
#: fault blocks every robot command).
_NEW_MOTION = ("power_on", "stand", "selfright")
_MOTIONS = ("power_on", "power_off", "stand", "sit", "stop", "selfright")


class _Health:
    """One reading of a robot's state: the class, the fault word that says which fault, and a sentence for a refusal."""

    __slots__ = ("klass", "word", "because")

    def __init__(self, klass: int, word: int, because: str) -> None:
        self.klass, self.word, self.because = klass, word, because


def _enum_name(enum, number: int) -> str:
    try:
        return enum.Name(number)
    except ValueError:
        return f"value {number}"


def _judge(state) -> _Health:
    """What the robot's own report says about it. Pure, so the suite can hold it to the table without a robot."""
    from bosdyn.api import robot_state_pb2 as pb
    candidates: list[tuple[int, int, str]] = []
    for fault in state.behavior_fault_state.faults:
        cause = _enum_name(pb.BehaviorFault.Cause, fault.cause)
        candidates.append((_FAULT, _BEHAVIOR_MARK | (fault.cause & _CODE_MASK),
                           f"a behaviour fault ({cause}), which blocks robot commands until an operator clears it"))
    power = state.power_state.motor_power_state
    klass = _MOTOR_CLASS.get(power, _FAULT)
    if klass != _OK:
        name = _enum_name(pb.PowerState.MotorPowerState, power)
        candidates.append((klass, _MOTOR_MARK | (power & _CODE_MASK),
                           f"its motor power in the state {name}, which has to be powered off before it is powered again"))
    for fault in state.system_fault_state.faults:
        klass = _SEVERITY_CLASS.get(fault.severity, _FAULT)
        if klass != _OK:
            severity = _enum_name(pb.SystemFault.Severity, fault.severity)
            candidates.append((klass, fault.code & _CODE_MASK,
                               f"a system fault, {fault.name!r} ({severity}, code {fault.code})"))
    # The first of the worst class: `max` keeps the first of equals, so behaviour faults, then motor power, then system faults.
    worst = max(candidates, key=lambda candidate: candidate[0], default=(_OK, 0, ""))
    released = pb.EStopState.State.Value("STATE_NOT_ESTOPPED")
    stopped = [slot for slot in state.estop_states if slot.state != released]
    if stopped:
        slot = stopped[0]
        name = _enum_name(pb.EStopState.State, slot.state)
        word = worst[1] if worst[0] >= _DEGRADED else 0
        return _Health(_ESTOP, word, f"its emergency stop {slot.name!r} is not released ({name})")
    return _Health(*worst)


class SpotDriver:
    """The RHAL side: twelve readable joints, six task motions, one real estop."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._opened = False
        self._estopped = False
        self._seq = 0
        self._robot: SpotRobot | None = None
        self._stop_threads: list[threading.Thread] = []
        self._simulator = True
        self._descriptor = self._describe()

    # -- open ------------------------------------------------------------
    def open(self, config_json: str) -> int:
        with self._lock:
            if self._opened:
                return rhal.SKR_EPERM
            try:
                config = json.loads(config_json or "{}")
            except json.JSONDecodeError:
                return rhal.SKR_EINVAL
            host = config.get("host", "")
            username = config.get("username", "")
            password = config.get("password", "")
            if not host:
                print("skynet-spot: no `host` in the config. Spot is addressed by "
                      "hostname or IP.", file=sys.stderr, flush=True)
                return rhal.SKR_EINVAL
            if not username or not password:
                print("skynet-spot: no `username`/`password` in the config. Every call "
                      "but the token mint carries the token.", file=sys.stderr, flush=True)
                return rhal.SKR_EINVAL
            try:
                port = int(config.get("port", 443))
                if not 1 <= port <= 65535:
                    raise ValueError
            except (TypeError, ValueError):
                print("skynet-spot: `port` is 1-65535 (default 443).",
                      file=sys.stderr, flush=True)
                return rhal.SKR_EINVAL
            try:
                estop_timeout = float(config.get("estop_timeout", _ESTOP_TIMEOUT_S))
                if not estop_timeout > 0:
                    raise ValueError
            except (TypeError, ValueError):
                print("skynet-spot: `estop_timeout` is seconds, positive (default 5).",
                      file=sys.stderr, flush=True)
                return rhal.SKR_EINVAL
            self._simulator = bool(config.get("simulator", True))
            robot = SpotRobot(host, port, username, password, config.get("ca"),
                              estop_timeout)
        # Fail fast outside the lock: bring-up does network I/O, and a blocked open
        # must not wedge the stop path behind the same mutex.
        try:
            robot.connect()
            state = robot.state()
        except SpotError as error:
            print(f"skynet-spot: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        names = {joint.name for joint in state.kinematic_state.joint_states}
        missing = [name for name in JOINTS if name not in names]
        if missing:
            robot.shutdown()
            print(f"skynet-spot: the robot answers without {missing}. A twelve-joint "
                  f"body answering eleven is a different body.", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        with self._lock:
            self._robot = robot
            self._opened = True
            self._descriptor = self._describe()
            return rhal.SKR_OK

    def close(self) -> int:
        # A stop that was just asked for is told before the session goes: the host closes straight after a stop.
        with self._lock:
            threads = list(self._stop_threads)
        for thread in threads:
            thread.join(timeout=_STOP_JOIN_S)
        with self._lock:
            robot, self._robot = self._robot, None
            self._opened = False
        if robot is not None:
            robot.shutdown()
        return rhal.SKR_OK

    # -- read_state ------------------------------------------------------
    def read_state(self, out, deadline_ns: int) -> int:
        _ = deadline_ns
        with self._lock:
            if not self._opened or self._robot is None:
                return rhal.SKR_EPERM
            robot, estopped = self._robot, self._estopped
        try:
            state = robot.state()
        except SpotError as error:
            print(f"skynet-spot: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        measured = {joint.name: joint
                    for joint in state.kinematic_state.joint_states}
        if any(name not in measured for name in JOINTS):
            print("skynet-spot: a joint went missing mid-session. Refusing the read "
                  "rather than reporting eleven joints as twelve.", file=sys.stderr,
                  flush=True)
            return rhal.SKR_EIO
        with self._lock:
            self._seq += 1
            out.stamp_ns = rhal.acquisition_ns()
            out.seq = self._seq
            out.position_rad = [measured[name].position.value for name in JOINTS]
            out.velocity_rad_s = [measured[name].velocity.value for name in JOINTS]
            # Effort is NaN, not load: `JointState.load` has no documented unit, and an
            # unmeasured quantity is NaN, never zero (§24).
            out.effort_nm = [math.nan] * len(JOINTS)
            health = _judge(state)
            out.fault_code = health.word
            out.flags = _FLAGS[health.klass]
            if estopped:
                out.flags |= rhal.FLAG_ESTOP
            edge = state.kinematic_state.transforms_snapshot.child_to_parent_edge_map.get(
                "body")
            if edge is not None and edge.parent_frame_name == "vision":
                pose = edge.parent_tform_child
                out.base_pose = [pose.position.x, pose.position.y, pose.position.z,
                                 pose.rotation.x, pose.rotation.y, pose.rotation.z,
                                 pose.rotation.w]
            else:
                out.base_pose = [math.nan] * 7
            twist = state.kinematic_state.velocity_of_body_in_vision
            out.base_twist = [twist.linear.x, twist.linear.y, twist.linear.z,
                              twist.angular.x, twist.angular.y, twist.angular.z]
            return rhal.SKR_OK

    # -- write_command ---------------------------------------------------
    def write_command(self, cmd, deadline_ns: int) -> int:
        _ = deadline_ns
        with self._lock:
            if not self._opened:
                return rhal.SKR_EPERM
        # The shape before the permission, the way the pendant checks it.
        if cmd.joint_count != len(JOINTS):
            return rhal.SKR_EINVAL
        if cmd.mode not in (rhal.MODE_POSITION, rhal.MODE_VELOCITY, rhal.MODE_TORQUE):
            return rhal.SKR_EINVAL
        if cmd.modes and len(cmd.modes) != len(JOINTS):
            return rhal.SKR_EINVAL
        print("skynet-spot: joint commands are refused: Spot's API is command-level "
              "(§59.2) and the only route that moves it is extension under "
              "vendor.spot.task.", file=sys.stderr, flush=True)
        return rhal.SKR_EPERM

    # -- extension -------------------------------------------------------
    def _gate(self, robot: SpotRobot, motion: str) -> tuple[int, bytes] | None:
        """Refuse a motion the robot's own state says it will not do (the table in `drivers/fault-vocabulary.json`), or None.

        The state is read at the moment of the request and not taken from the last `read_state`: the answer to "may I stand it" is the robot's
        now. A robot whose estop is not released is refused everything. One that is not doing what it is told (a critical system fault, a
        behaviour fault, motors in error) is refused the motions that bring the body up or take it somewhere new, and still sent `stop`, `sit`
        and `power_off`: the way out of a fault is not refused because the robot has reported one.
        """
        try:
            health = _judge(robot.state())
        except SpotError as error:
            return (rhal.SKR_EIO, f"{motion} was not sent: the robot's state could not be read to say it may move ({error}). "
                                  f"Check the link, then ask again.".encode("utf-8"))
        if health.klass == _ESTOP:
            return (rhal.SKR_EPERM, f"{motion} refused: the robot reports {health.because}. Release the stop on the robot, then ask again.".encode("utf-8"))
        if health.klass == _FAULT and motion in _NEW_MOTION:
            return (rhal.SKR_EPERM, f"{motion} refused: the robot reports {health.because}. `stop`, `sit` and `power_off` are still sent; "
                                    f"clearing the fault is an operator's act on the robot.".encode("utf-8"))
        return None

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != TOKEN:
            return (rhal.SKR_ENOSYS,
                    f"unknown token {token!r}. This driver serves {TOKEN}.".encode("utf-8"))
        with self._lock:
            if self._estopped:
                return (rhal.SKR_EPERM,
                        b"refused: the emergency stop is latched. It clears only in a "
                        b"new driver instance; the CUT already went out.")
            if not self._opened or self._robot is None:
                return (rhal.SKR_EPERM, b"the driver is not open.")
            robot = self._robot
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            return (rhal.SKR_EINVAL, b"the request is text: a motion name in UTF-8.")
        words = text.split()
        if not words:
            return (rhal.SKR_EINVAL, b"empty request. Motions: power_on, power_off, "
                                     b"stand, sit, stop, selfright.")
        motion, rest = words[0], words[1:]
        if rest:
            return (rhal.SKR_EINVAL,
                    f"{motion} takes no arguments.".encode("utf-8"))
        if motion in _MOTIONS:
            refusal = self._gate(robot, motion)
            if refusal is not None:
                return refusal
        try:
            if motion == "power_on":
                robot.power(True)
                return (rhal.SKR_OK, b"{\"powered\":true}")
            if motion == "power_off":
                robot.power(False)
                return (rhal.SKR_OK, b"{\"powered\":false}")
            if motion == "stand":
                command_id = robot.stand()
            elif motion == "sit":
                command_id = robot.sit()
            elif motion == "stop":
                command_id = robot.stop()
            elif motion == "selfright":
                command_id = robot.selfright()
            else:
                return (rhal.SKR_EINVAL,
                        f"unknown motion {motion!r}. Motions: power_on, power_off, "
                        f"stand, sit, stop, selfright.".encode("utf-8"))
            return (rhal.SKR_OK, json.dumps({"command_id": command_id}).encode("utf-8"))
        except SpotDenied as error:
            return (rhal.SKR_EPERM, f"{error}".encode("utf-8"))
        except SpotError as error:
            return (rhal.SKR_EIO, f"the robot refused: {error}".encode("utf-8"))

    # -- emergency_stop ---------------------------------------------------
    def emergency_stop(self) -> int:
        """CUT the motors, stop the keepalives, latch. The CUT is best-effort; the latch
        is not, and behind the stopped keepalives stands the deadman: a silent endpoint
        estops the robot at its timeout whether the CUT landed or not. The session stays
        up so later reads report the stop rather than refusing to report at all."""
        with self._lock:
            robot = self._robot
            self._estopped = True
            if robot is not None:
                # Two threads, because neither waits for the other: a CUT on a link that has gone is a wait of its own, and the keepalives' shutdown joins threads that may be inside a call.
                threads = [threading.Thread(target=target, daemon=True, name=f"spot-stop-{target.__name__}") for target in (robot.cut, robot.drop_liveness)]
                self._stop_threads = [t for t in self._stop_threads if t.is_alive()] + threads
                for thread in threads:
                    thread.start()
        return rhal.SKR_OK

    def release_stop(self) -> int:
        """No software release: ENOSYS, honestly.

        The stop CUT the motors and dropped the keepalives, and returning from that
        is re-establishing an estop endpoint, re-taking the lease, powering on and
        standing — a bring-up, not a latch clear, and standing is a motion no
        self-test performs on its own. Only a new session returns this body:
        close tears the endpoint down, open brings a live one up, and power_on and
        stand stay the operator's motions under vendor.spot.task. The OS renews the
        session instead, which is what ENOSYS here asks it to do.
        """
        return rhal.SKR_ENOSYS

    # -- describe ----------------------------------------------------------
    def _describe(self) -> str:
        joints = [{"name": name, "role": role, "type": "revolute"}
                  for name, role in zip(JOINTS, ROLES)]
        return json.dumps({
            "schema_version": 1, "robot_slug": "spot", "digest": None,
            "verified": False, "simulator": self._simulator, "base": "quadruped",
            "dof": len(JOINTS), "joints": joints, "end_effectors": [], "sensors": [],
            # No commandable modes: `write_command` refuses everything (§59.2's genuine
            # class). Motion goes through the six task verbs below.
            "control": {"modes": [], "rate_hz_max": None, "rate_hz_min": None,
                        "command_latency_ms": None},
            # No envelope and no per-joint travel: Boston Dynamics publishes neither,
            # and the envelope is not where a guess goes.
            "envelope": None,
            # The estop is real: CUT cuts motor power through the vendor's own estop
            # system, which is what this capability names. Independence from the
            # command path (§15's `safety.estop.independent`) is unclaimed — the CUT
            # travels the same TLS the commands do.
            "capabilities": ["safety.estop"],
            # §84.1's motion list. No parameters: the builders' defaults are the
            # vendor's, and a parameter this driver invented would be a second vendor.
            "motions": [
                {"name": "power_on", "parameters": []},
                {"name": "power_off", "parameters": []},
                {"name": "stand", "parameters": []},
                {"name": "sit", "parameters": []},
                {"name": "stop", "parameters": []},
                {"name": "selfright", "parameters": []},
            ],
            "extensions": [TOKEN], "solvers": [],
        })

    def describe(self) -> str:
        with self._lock:
            return self._descriptor

    def read_sensor(self, name: str, out) -> int:
        _ = (name, out)
        return rhal.SKR_ENOSYS

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
      real machine (`ClearBehaviorFault` exists and this driver does not call it), so the
      fault word counts active system and behaviour faults and FLAG_FAULT rides with it.

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

#: One RPC's budget, both ways. Loopback answers in milliseconds and a real Spot on the
#: dock network in tens of them; ten seconds is the network-is-dead verdict, not a wait.
_RPC_TIMEOUT_S = 10.0


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

        try:
            sdk = sdk_mod.create_standard_sdk("skynet-spot",
                                              cert_resource_glob=self._ca_pem)
            robot = sdk.create_robot(self._host)
            robot.update_secure_channel_port(self._port)
            robot.authenticate(self._username, self._password,
                               timeout=_RPC_TIMEOUT_S)
            robot.sync_with_directory()
            estop = robot.ensure_client(EstopClient.default_service_name)
            endpoint = EstopEndpoint(estop, "skynet-spot", self._estop_timeout)
            endpoint.force_simple_setup()
            estop_alive = EstopKeepAlive(endpoint)
            lease_client = robot.ensure_client(LeaseClient.default_service_name)
            lease = lease_client.acquire("body")
            lease_alive = LeaseKeepAlive(lease_client)
            # Time sync before the command client exists: the client inherits the
            # endpoint at creation, and a command client born without one refuses
            # every command with NoTimeSyncError.
            robot.time_sync.wait_for_sync(timeout_sec=_RPC_TIMEOUT_S)
            self._commands = robot.ensure_client(RobotCommandClient.default_service_name)
            self._states = robot.ensure_client(RobotStateClient.default_service_name)
            self._power = robot.ensure_client(PowerClient.default_service_name)
        except SpotError:
            raise
        except Exception as error:
            self._teardown()
            raise SpotError(f"bring-up failed: {error}") from error
        self._robot = robot
        self._estop_endpoint = endpoint
        self._estop_alive = estop_alive
        self._lease = lease
        self._lease_client = lease_client
        self._lease_alive = lease_alive

    def state(self):
        try:
            return self._states.get_robot_state(timeout=_RPC_TIMEOUT_S)
        except Exception as error:
            raise SpotError(f"state read failed: {error}") from error

    def power(self, on: bool) -> None:
        from bosdyn.api import power_pb2
        want = (power_pb2.PowerCommandRequest.REQUEST_ON_MOTORS if on else
                power_pb2.PowerCommandRequest.REQUEST_OFF_MOTORS)
        try:
            issued = self._power.power_command(want, lease=self._lease.lease_proto)
            self._power.power_command_feedback(issued.power_command_id,
                                               timeout=_RPC_TIMEOUT_S)
        except Exception as error:
            raise SpotError(f"power {'on' if on else 'off'} refused: {error}") from error

    def _motion(self, kind: str, command) -> int:
        from bosdyn.client.robot_command import BehaviorFaultError, NotPoweredOnError
        try:
            return self._commands.robot_command(
                command, lease=self._lease.lease_proto,
                timesync_endpoint=self._commands.timesync_endpoint,
                timeout=_RPC_TIMEOUT_S)
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
        """Best-effort CUT. Allowed to fail: the caller latches regardless, and the
        deadman behind the stopped keepalives is the second stop."""
        try:
            self._estop_endpoint.stop(timeout=_RPC_TIMEOUT_S)
        except Exception as error:
            print(f"skynet-spot: the CUT did not land: {error}. The latch holds and "
                  f"the deadman behind the stopped keepalives stops the robot.",
                  file=sys.stderr, flush=True)

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


def _robot_estopped(state) -> bool:
    from bosdyn.api import robot_state_pb2
    values = robot_state_pb2.EStopState.DESCRIPTOR.fields_by_name[
        "state"].enum_type.values_by_name
    stopped = values["STATE_ESTOPPED"].number
    return any(slot.state == stopped for slot in state.estop_states)


class SpotDriver:
    """The RHAL side: twelve readable joints, six task motions, one real estop."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._opened = False
        self._estopped = False
        self._seq = 0
        self._robot: SpotRobot | None = None
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
            faults = (len(state.system_fault_state.faults) +
                      len(state.behavior_fault_state.faults))
            out.fault_code = faults
            robot_stopped = _robot_estopped(state)
            out.flags = 0
            if estopped or robot_stopped:
                out.flags |= rhal.FLAG_ESTOP
            if faults or robot_stopped:
                out.flags |= rhal.FLAG_FAULT
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
            robot.cut()
            robot.drop_liveness()
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

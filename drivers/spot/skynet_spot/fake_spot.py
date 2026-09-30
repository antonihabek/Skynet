"""A Spot on loopback: the eight gRPC services the driver speaks, over TLS.

`DRY-COVERAGE-PLAN.md`'s Phase 7 quadruped row for Boston Dynamics Spot: `bosdyn-client`
5.2.0 is public on PyPI and there is no vendor simulator, so the substrate is faked gRPC
services with the vendor's client unmodified (the Pudu shape). One TLS port serves all eight
services — Auth, Directory, Estop, Lease, RobotState, RobotCommand, Power, TimeSync — the way
one port serves them on the robot, distinguished by authority.

What the fake verifies, on every call but `GetAuthToken`: the `authorization: Bearer` token
the SDK's metadata plugin attaches. A wrong or missing token is UNAUTHENTICATED, not data.
What it enforces: lease discipline (power and motion commands without the live lease are
refused with a lease-use verdict), power discipline (motion while unpowered is refused),
timesync discipline (motion without an established clock is refused), and the estop
challenge/response (a wrong echo is refused after the seeding check-in). What it records:
every command by kind, every refusal by cause, and the final counts as one JSON line.

What is scripted: the twelve joint angles (stand and sit poses are this file's invention, not
vendor figures), the body height that follows them, the battery at 95%, power transitions
completing immediately (a real Spot takes seconds; the fake answers SUCCESS at once and says
so). The simplifications are three and all stated: one lease holder (a second acquirer is
refused until the lease is returned or taken), estop resolved per endpoint with last
check-in winning (a real system ANDs its endpoints), and no behaviour faults (system faults
inject through the harness API only).

TLS uses the test-only fixtures in `../certs/`: a CA whose key was deleted after signing, and
a server certificate carrying every authority this fake lists. The SDK is pointed at the CA
explicitly (`cert_resource_glob`), so the TLS path is the real one with a test trust root —
nothing here disables verification.
"""

from __future__ import annotations

import json
import signal
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor

import grpc
from google.protobuf import timestamp_pb2

from bosdyn.api import (
    auth_pb2,
    auth_service_pb2_grpc,
    directory_pb2,
    directory_service_pb2_grpc,
    estop_pb2,
    estop_service_pb2_grpc,
    geometry_pb2,
    header_pb2,
    lease_pb2,
    lease_service_pb2_grpc,
    power_pb2,
    power_service_pb2_grpc,
    robot_command_pb2,
    robot_command_service_pb2_grpc,
    robot_state_pb2,
    robot_state_service_pb2_grpc,
    time_sync_pb2,
    time_sync_service_pb2_grpc,
)

CERTS_DIR = __file__.rsplit("/", 2)[0] + "/certs"

#: The vendor's joint enumeration, transcribed from the SDK's examples: front-left,
#: front-right, hind-left, hind-right, each hip-x, hip-y, knee. The driver maps by name
#: and the suite shuffles this order against it, so the transcription is load-bearing
#: nowhere — but it is the order a real Spot answers in.
JOINTS = (
    "fl.hx", "fl.hy", "fl.kn",
    "fr.hx", "fr.hy", "fr.kn",
    "hl.hx", "hl.hy", "hl.kn",
    "hr.hx", "hr.hy", "hr.kn",
)

#: Scripted poses. Invention, not vendor figures: distinct enough that a stand-after-sit
#: reads unambiguously in the joint angles.
STAND = {"hx": 0.0, "hy": -0.75, "kn": 1.50}
SIT = {"hx": 0.0, "hy": -0.30, "kn": 2.30}

#: (service name, service type, authority). The directory's values are this fake's, following
#: the SDK's own bootstrap pattern (`<service>.spot.robot`); directory itself answers as
#: `api.spot.robot`, which is the authority the SDK bootstraps it under.
SERVICES = (
    ("auth", "bosdyn.api.AuthService", "auth.spot.robot"),
    ("directory", "bosdyn.api.DirectoryService", "api.spot.robot"),
    ("estop", "bosdyn.api.EstopService", "estop.spot.robot"),
    ("lease", "bosdyn.api.LeaseService", "lease.spot.robot"),
    ("robot-state", "bosdyn.api.RobotStateService", "robot-state.spot.robot"),
    ("robot-command", "bosdyn.api.RobotCommandService", "robot-command.spot.robot"),
    ("power", "bosdyn.api.PowerService", "power.spot.robot"),
    ("time-sync", "bosdyn.api.TimeSyncService", "time-sync.spot.robot"),
)

_TOKEN = "skynet-fake-spot-token"


def _status(message_cls, name: str) -> int:
    """A status enum value by name, failing loud at first use when the SDK has no such
    value. Every status this fake answers was read off the installed `bosdyn.api`
    descriptors; a name that stops existing must break the fake, not silently answer 0."""
    values = message_cls.DESCRIPTOR.fields_by_name["status"].enum_type.values_by_name
    if name not in values:
        raise AssertionError(f"{message_cls.DESCRIPTOR.full_name} has no {name}")
    return values[name].number


def _timeout_s(duration) -> float:
    """An estop timeout as seconds. Both halves: a 0.5 s deadman arrives as zero seconds
    and five hundred million nanos, and reading seconds alone would silently run it as
    five."""
    total = duration.seconds + duration.nanos / 1_000_000_000
    return total if total > 0 else 5.0


def _now() -> timestamp_pb2.Timestamp:
    stamp = timestamp_pb2.Timestamp()
    stamp.FromSeconds(int(time.time()))
    return stamp


def _header(request) -> header_pb2.ResponseHeader:
    # The error code is load-bearing: the SDK's `common_header_errors` reads it before
    # any per-RPC status, and an unset code fails the call as `UnsetStatusError` no
    # matter what the RPC status says.
    header = header_pb2.ResponseHeader(
        request_header=request.header, request_received_timestamp=_now(),
        response_timestamp=_now())
    header.error.code = header_pb2.CommonError.CODE_OK
    return header


class SpotWorld:
    """The scripted robot all eight services share. One lock; every servicer takes it."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self.joints = {name: (SIT[name.rsplit(".", 1)[1]], 0.0) for name in JOINTS}
        self.body_z = 0.35
        self.powered = False
        self.estopped = False
        self.lease: lease_pb2.Lease | None = None
        self.lease_owner = ""
        self.epoch = 0
        self.config_id = "config-1"
        self.config_seq = 1
        self.endpoints: dict[str, dict] = {}
        self.endpoint_seq = 0
        self.challenge_seq = 0
        self.command_seq = 0
        self.power_seq = 0
        self.clocks: set[str] = set()
        self.faults: list[tuple[str, str]] = []
        self.last_command = "none"
        # Levers the harness pulls (in-process suites only): what the robot says and does
        # when something is wrong with it. Each one exists because a seeded defect
        # (`tools/seed-defects`, DRY-COVERAGE-PLAN.md 12.C.3) survived without it.
        self.credentials = ("admin", "password")
        self.body_xy = (0.0, 0.0)
        self.body_rotation = (0.0, 0.0, 0.0, 1.0)     # x, y, z, w
        self.body_twist = ((0.0, 0.0, 0.0), (0.0, 0.0, 0.0))  # linear, angular
        self.body_frame = "vision"
        self.hw_estop = False               # a button pressed on the robot, not by the driver
        self.hidden_joints: set[str] = set()
        self.hidden_services: set[str] = set()   # services the directory does not list
        self.behavior_faults: list[str] = []
        self.state_fails = False            # GetRobotState is unavailable
        self.cut_fails = False              # the CUT never lands
        self.cut_delay = 0.0                # seconds the CUT takes to be answered
        self.command_status: str | None = None   # a status every accepted-looking command gets
        self.power_feedback = "STATUS_SUCCESS"
        self.timesync_delay = 0.0           # seconds each TimeSyncUpdate takes to be answered
        self.counts = {
            "requests": 0, "bad_auth": 0, "lease_refused": 0, "unpowered_refused": 0,
            "no_timesync_refused": 0, "challenge_refused": 0, "power_on": 0,
            "power_off": 0, "stand": 0, "sit": 0, "stop": 0, "selfright": 0,
            "estop_cut": 0, "estop_timeout": 0, "checkins": 0, "deregister_refused": 0,
            "bad_login": 0, "retains": 0,
        }
        self._watchdog_stop = threading.Event()
        self._watchdog = threading.Thread(target=self._deadman, daemon=True)
        self._watchdog.start()

    def _deadman(self) -> None:
        """The estop timeout, which is the deadman switch: a registered endpoint that
        goes silent past its timeout estops the robot and cuts motor power. This is what
        makes abandoning the endpoint safe — dropping the deadman stops the machine —
        and what lets a fresh `open()` reconfigure estop after a dirty `close()`."""
        while not self._watchdog_stop.wait(0.2):
            with self._lock:
                now = time.monotonic()
                for endpoint in self.endpoints.values():
                    if endpoint["seeded"] and now - endpoint["last"] > endpoint["timeout"]:
                        endpoint["seeded"] = False
                        endpoint["level"] = "CUT"
                        if not self.estopped:
                            self.counts["estop_timeout"] += 1
                        self.estopped = True
                        self.powered = False

    def halt(self) -> None:
        self._watchdog_stop.set()
        self._watchdog.join(timeout=2.0)

    def inject_fault(self, name: str, severity: str = "SEVERITY_WARN") -> None:
        """Harness API (in-process suites only): hang a system fault on the robot."""
        with self._lock:
            self.faults.append((name, severity))

    def clear_faults(self) -> None:
        with self._lock:
            self.faults = []

    def set_body(self, xy=None, z=None, rotation=None, twist=None, frame=None) -> None:
        """Harness API: where the body is and how it moves, in whatever frame is named. A
        robot that has not localised names another frame than `vision`."""
        with self._lock:
            if xy is not None:
                self.body_xy = tuple(xy)
            if z is not None:
                self.body_z = z
            if rotation is not None:
                self.body_rotation = tuple(rotation)
            if twist is not None:
                self.body_twist = (tuple(twist[0]), tuple(twist[1]))
            if frame is not None:
                self.body_frame = frame

    def set_joint(self, name: str, position: float, velocity: float) -> None:
        """Harness API: one joint's position and velocity."""
        with self._lock:
            self.joints[name] = (position, velocity)

    def hide_joint(self, name: str) -> None:
        """Harness API: the robot stops reporting a joint — a body of eleven answering as twelve."""
        with self._lock:
            self.hidden_joints.add(name)

    def inject_behavior_fault(self, name: str) -> None:
        """Harness API: a behaviour fault, the kind that refuses motion and is cleared by an operator."""
        with self._lock:
            self.behavior_faults.append(name)

    def shuffle_joints(self) -> None:
        """Harness API: serve the joints in reverse enumeration order, so the suite can
        prove the driver maps by name rather than by position."""
        with self._lock:
            items = list(self.joints.items())[::-1]
            self.joints = dict(items)

    def report(self) -> str:
        with self._lock:
            return json.dumps({"counts": self.counts, "last_command": self.last_command,
                               "powered": self.powered, "estopped": self.estopped})


def _bearer(context: grpc.ServicerContext) -> str | None:
    for key, value in context.invocation_metadata():
        if key == "authorization" and value.startswith("Bearer "):
            return value[len("Bearer "):]
    return None


class _Guarded:
    """Token discipline shared by the seven services that require it."""

    def __init__(self, world: SpotWorld) -> None:
        self._world = world

    def _admit(self, context: grpc.ServicerContext) -> bool:
        with self._world._lock:
            self._world.counts["requests"] += 1
        if _bearer(context) != _TOKEN:
            with self._world._lock:
                self._world.counts["bad_auth"] += 1
            context.abort(grpc.StatusCode.UNAUTHENTICATED, "unknown or missing user token")
            return False
        return True


class _Auth(auth_service_pb2_grpc.AuthServiceServicer):
    def __init__(self, world: SpotWorld) -> None:
        self._world = world

    def GetAuthToken(self, request, context):
        with self._world._lock:
            self._world.counts["requests"] += 1
        response = auth_pb2.GetAuthTokenResponse(header=_header(request))
        if request.token:
            # A re-authentication on an existing token: the robot mints a longer-lived
            # one. The fake's token does not expire, so it reissues itself.
            if request.token != _TOKEN:
                response.status = _status(auth_pb2.GetAuthTokenResponse,
                                          "STATUS_INVALID_TOKEN")
                return response
            response.token = _TOKEN
        elif (request.username, request.password) == self._world.credentials:
            response.token = _TOKEN
        else:
            with self._world._lock:
                self._world.counts["bad_login"] += 1
            response.status = _status(auth_pb2.GetAuthTokenResponse, "STATUS_INVALID_LOGIN")
            return response
        response.status = _status(auth_pb2.GetAuthTokenResponse, "STATUS_OK")
        return response


class _Directory(_Guarded, directory_service_pb2_grpc.DirectoryServiceServicer):
    def ListServiceEntries(self, request, context):
        if not self._admit(context):
            return directory_pb2.ListServiceEntriesResponse()
        response = directory_pb2.ListServiceEntriesResponse(header=_header(request))
        for name, service_type, authority in SERVICES:
            if name in self._world.hidden_services:
                continue
            response.service_entries.add(
                name=name, type=service_type, authority=authority,
                last_update=_now(), user_token_required=(name != "auth"))
        return response


class _Estop(_Guarded, estop_service_pb2_grpc.EstopServiceServicer):
    def GetEstopConfig(self, request, context):
        if not self._admit(context):
            return estop_pb2.GetEstopConfigResponse()
        response = estop_pb2.GetEstopConfigResponse(header=_header(request))
        response.request.CopyFrom(request)
        with self._world._lock:
            response.active_config.unique_id = self._world.config_id
            for unique_id, endpoint in self._world.endpoints.items():
                slot = response.active_config.endpoints.add()
                slot.role = endpoint["role"]
                slot.name = endpoint["name"]
                slot.unique_id = unique_id
                slot.timeout.FromNanoseconds(int(endpoint["timeout"] * 1_000_000_000))
        return response

    def RegisterEstopEndpoint(self, request, context):
        if not self._admit(context):
            return estop_pb2.RegisterEstopEndpointResponse()
        response = estop_pb2.RegisterEstopEndpointResponse(header=_header(request))
        response.request.CopyFrom(request)
        with self._world._lock:
            if request.target_config_id != self._world.config_id:
                response.status = _status(estop_pb2.RegisterEstopEndpointResponse,
                                          "STATUS_CONFIG_MISMATCH")
                return response
            self._world.endpoint_seq += 1
            unique_id = f"ep-{self._world.endpoint_seq}"
            self._world.endpoints[unique_id] = {
                "role": request.new_endpoint.role, "name": request.new_endpoint.name,
                "timeout": _timeout_s(request.new_endpoint.timeout),
                "challenge": 0, "level": "NONE", "seeded": False, "last": time.monotonic(),
            }
            response.new_endpoint.CopyFrom(request.new_endpoint)
            response.new_endpoint.unique_id = unique_id
            response.status = _status(estop_pb2.RegisterEstopEndpointResponse,
                                      "STATUS_SUCCESS")
            return response

    def DeregisterEstopEndpoint(self, request, context):
        if not self._admit(context):
            return estop_pb2.DeregisterEstopEndpointResponse()
        response = estop_pb2.DeregisterEstopEndpointResponse(header=_header(request))
        response.request.CopyFrom(request)
        with self._world._lock:
            if request.target_config_id != self._world.config_id:
                response.status = _status(estop_pb2.DeregisterEstopEndpointResponse,
                                          "STATUS_CONFIG_MISMATCH")
                return response
            if request.target_endpoint.unique_id not in self._world.endpoints:
                response.status = _status(estop_pb2.DeregisterEstopEndpointResponse,
                                          "STATUS_ENDPOINT_MISMATCH")
                return response
            if self._world.powered:
                self._world.counts["deregister_refused"] += 1
                response.status = _status(estop_pb2.DeregisterEstopEndpointResponse,
                                          "STATUS_MOTORS_ON")
                return response
            del self._world.endpoints[request.target_endpoint.unique_id]
            response.status = _status(estop_pb2.DeregisterEstopEndpointResponse,
                                      "STATUS_SUCCESS")
            return response

    def SetEstopConfig(self, request, context):
        if not self._admit(context):
            return estop_pb2.SetEstopConfigResponse()
        response = estop_pb2.SetEstopConfigResponse(header=_header(request))
        response.request.CopyFrom(request)
        with self._world._lock:
            if request.target_config_id != self._world.config_id:
                response.status = _status(estop_pb2.SetEstopConfigResponse,
                                          "STATUS_INVALID_ID")
                return response
            if self._world.powered:
                response.status = _status(estop_pb2.SetEstopConfigResponse,
                                          "STATUS_MOTORS_ON")
                return response
            # The configuration is the endpoint set: replace it, minting ids for the
            # endpoints that arrived without one.
            self._world.endpoints = {}
            for endpoint in request.config.endpoints:
                unique_id = endpoint.unique_id
                if not unique_id:
                    self._world.endpoint_seq += 1
                    unique_id = f"ep-{self._world.endpoint_seq}"
                self._world.endpoints[unique_id] = {
                    "role": endpoint.role, "name": endpoint.name,
                    "timeout": _timeout_s(endpoint.timeout),
                    "challenge": 0, "level": "NONE", "seeded": False, "last": time.monotonic(),
                }
            self._world.config_seq += 1
            self._world.config_id = f"config-{self._world.config_seq}"
            response.active_config.unique_id = self._world.config_id
            for unique_id, endpoint in self._world.endpoints.items():
                slot = response.active_config.endpoints.add()
                slot.role = endpoint["role"]
                slot.name = endpoint["name"]
                slot.unique_id = unique_id
                slot.timeout.FromNanoseconds(int(endpoint["timeout"] * 1_000_000_000))
            response.status = _status(estop_pb2.SetEstopConfigResponse, "STATUS_SUCCESS")
            return response

    def EstopCheckIn(self, request, context):
        if not self._admit(context):
            return estop_pb2.EstopCheckInResponse()
        if request.stop_level == estop_pb2.ESTOP_LEVEL_CUT:
            # A CUT that is slow to be answered, or never lands: the driver's latch must not
            # wait on either.
            if self._world.cut_delay:
                time.sleep(self._world.cut_delay)
            if self._world.cut_fails:
                context.abort(grpc.StatusCode.UNAVAILABLE, "the harness dropped the CUT")
                return estop_pb2.EstopCheckInResponse()
        response = estop_pb2.EstopCheckInResponse(header=_header(request))
        response.request.CopyFrom(request)
        with self._world._lock:
            self._world.counts["checkins"] += 1
            endpoint = self._world.endpoints.get(request.endpoint.unique_id)
            if endpoint is None:
                response.status = _status(estop_pb2.EstopCheckInResponse,
                                          "STATUS_ENDPOINT_UNKNOWN")
                return response
            # The response is the challenge's bitwise negation (`response_from_challenge`
            # in the vendor client), proving the endpoint received the last reply. The
            # seeding check-in carries no meaningful response and is accepted.
            want = (~endpoint["challenge"]) & 0xFFFFFFFFFFFFFFFF
            if endpoint["seeded"] and request.response != want:
                self._world.counts["challenge_refused"] += 1
                response.status = _status(
                    estop_pb2.EstopCheckInResponse,
                    "STATUS_INCORRECT_CHALLENGE_RESPONSE")
                response.challenge = endpoint["challenge"]
                return response
            endpoint["seeded"] = True
            endpoint["last"] = time.monotonic()
            self._world.challenge_seq += 1
            endpoint["challenge"] = self._world.challenge_seq
            response.challenge = endpoint["challenge"]
            level = request.stop_level
            cut = level == estop_pb2.ESTOP_LEVEL_CUT
            settle = level == estop_pb2.ESTOP_LEVEL_SETTLE_THEN_CUT
            if cut or settle:
                endpoint["level"] = "CUT"
                if not self._world.estopped:
                    self._world.counts["estop_cut"] += 1
                self._world.estopped = True
                self._world.powered = False
                for name in self._world.joints:
                    pose = SIT[name.rsplit(".", 1)[1]]
                    self._world.joints[name] = (pose, 0.0)
                self._world.body_z = 0.35
            else:
                endpoint["level"] = "NONE"
                if all(e["level"] == "NONE" for e in self._world.endpoints.values()):
                    self._world.estopped = False
            response.status = _status(estop_pb2.EstopCheckInResponse, "STATUS_OK")
            return response


def _lease_use(world: SpotWorld, presented: lease_pb2.Lease) -> lease_pb2.LeaseUseResult:
    """The verdict a real Spot renders on a presented lease. The sequence is a path, not
    a number: a presented lease that extends the current one (`[1, 0]` against `[1]`,
    which is what the SDK's keepalive retains with) is a live sublease and valid; a
    strict prefix is older; anything else is not this lease at all."""
    verdict = lease_pb2.LeaseUseResult()
    current = world.lease
    if current is None:
        verdict.status = _status(lease_pb2.LeaseUseResult, "STATUS_UNMANAGED")
        return verdict
    verdict.attempted_lease.CopyFrom(presented)
    verdict.latest_known_lease.CopyFrom(current)
    if presented.resource != current.resource:
        verdict.status = _status(lease_pb2.LeaseUseResult, "STATUS_INVALID_LEASE")
        return verdict
    if presented.epoch != current.epoch:
        verdict.status = _status(lease_pb2.LeaseUseResult, "STATUS_WRONG_EPOCH")
        return verdict
    have = list(presented.sequence)
    want = list(current.sequence)
    if have[:len(want)] == want:
        verdict.status = _status(lease_pb2.LeaseUseResult, "STATUS_OK")
    elif want[:len(have)] == have:
        verdict.status = _status(lease_pb2.LeaseUseResult, "STATUS_OLDER")
    else:
        verdict.status = _status(lease_pb2.LeaseUseResult, "STATUS_INVALID_LEASE")
    return verdict


class _Lease(_Guarded, lease_service_pb2_grpc.LeaseServiceServicer):
    def AcquireLease(self, request, context):
        if not self._admit(context):
            return lease_pb2.AcquireLeaseResponse()
        response = lease_pb2.AcquireLeaseResponse(header=_header(request))
        with self._world._lock:
            if request.resource != "body":
                response.status = _status(lease_pb2.AcquireLeaseResponse,
                                          "STATUS_INVALID_RESOURCE")
                return response
            if self._world.lease is not None:
                response.status = _status(lease_pb2.AcquireLeaseResponse,
                                          "STATUS_RESOURCE_ALREADY_CLAIMED")
                response.lease.CopyFrom(self._world.lease)
                response.lease_owner.client_name = self._world.lease_owner
                return response
            self._world.epoch += 1
            held = lease_pb2.Lease(resource="body", epoch=f"epoch-{self._world.epoch}")
            held.sequence.append(1)
            held.client_names.append(request.header.client_name or "unnamed")
            self._world.lease = held
            self._world.lease_owner = request.header.client_name or "unnamed"
            response.lease.CopyFrom(held)
            response.lease_owner.client_name = self._world.lease_owner
            response.status = _status(lease_pb2.AcquireLeaseResponse, "STATUS_OK")
            return response

    def TakeLease(self, request, context):
        if not self._admit(context):
            return lease_pb2.TakeLeaseResponse()
        response = lease_pb2.TakeLeaseResponse(header=_header(request))
        with self._world._lock:
            if request.resource != "body":
                response.status = _status(lease_pb2.TakeLeaseResponse,
                                          "STATUS_INVALID_RESOURCE")
                return response
            self._world.epoch += 1
            held = lease_pb2.Lease(resource="body", epoch=f"epoch-{self._world.epoch}")
            held.sequence.append(1)
            held.client_names.append(request.header.client_name or "unnamed")
            self._world.lease = held
            self._world.lease_owner = request.header.client_name or "unnamed"
            response.lease.CopyFrom(held)
            response.lease_owner.client_name = self._world.lease_owner
            response.status = _status(lease_pb2.TakeLeaseResponse, "STATUS_OK")
            return response

    def RetainLease(self, request, context):
        if not self._admit(context):
            return lease_pb2.RetainLeaseResponse()
        response = lease_pb2.RetainLeaseResponse(header=_header(request))
        with self._world._lock:
            self._world.counts["retains"] += 1
            response.lease_use_result.CopyFrom(_lease_use(self._world, request.lease))
            return response

    def ReturnLease(self, request, context):
        if not self._admit(context):
            return lease_pb2.ReturnLeaseResponse()
        response = lease_pb2.ReturnLeaseResponse(header=_header(request))
        with self._world._lock:
            verdict = _lease_use(self._world, request.lease)
            ok = _status(lease_pb2.LeaseUseResult, "STATUS_OK")
            if verdict.status != ok:
                response.status = _status(lease_pb2.ReturnLeaseResponse,
                                          "STATUS_NOT_ACTIVE_LEASE")
                return response
            self._world.lease = None
            self._world.lease_owner = ""
            response.status = _status(lease_pb2.ReturnLeaseResponse, "STATUS_OK")
            return response

    def ListLeases(self, request, context):
        if not self._admit(context):
            return lease_pb2.ListLeasesResponse()
        response = lease_pb2.ListLeasesResponse(header=_header(request))
        with self._world._lock:
            if self._world.lease is not None:
                slot = response.resources.add()
                slot.resource = "body"
                slot.lease.CopyFrom(self._world.lease)
                slot.lease_owner.client_name = self._world.lease_owner
            return response


class _Power(_Guarded, power_service_pb2_grpc.PowerServiceServicer):
    def PowerCommand(self, request, context):
        if not self._admit(context):
            return power_pb2.PowerCommandResponse()
        response = power_pb2.PowerCommandResponse(header=_header(request))
        with self._world._lock:
            verdict = _lease_use(self._world, request.lease)
            response.lease_use_result.CopyFrom(verdict)
            ok = _status(lease_pb2.LeaseUseResult, "STATUS_OK")
            if verdict.status != ok:
                self._world.counts["lease_refused"] += 1
                response.status = _status(power_pb2.PowerCommandResponse, "STATUS_UNKNOWN")
                return response
            if self._world.estopped:
                response.status = _status(power_pb2.PowerCommandResponse, "STATUS_ESTOPPED")
                return response
            want = request.request
            if want == power_pb2.PowerCommandRequest.REQUEST_ON_MOTORS:
                self._world.powered = True
                self._world.counts["power_on"] += 1
            elif want == power_pb2.PowerCommandRequest.REQUEST_OFF_MOTORS:
                self._world.powered = False
                self._world.counts["power_off"] += 1
            else:
                # The fake's slice is motors power. Whole-robot, payload-port and radio
                # requests are real RPCs a real robot answers; here they are out of
                # slice, refused loudly rather than pretended.
                response.status = _status(power_pb2.PowerCommandResponse, "STATUS_UNKNOWN")
                return response
            self._world.power_seq += 1
            response.power_command_id = self._world.power_seq
            response.status = _status(power_pb2.PowerCommandResponse, "STATUS_SUCCESS")
            return response

    def PowerCommandFeedback(self, request, context):
        if not self._admit(context):
            return power_pb2.PowerCommandFeedbackResponse()
        # Power transitions complete immediately in the fake (a real Spot takes seconds and
        # this RPC is the poll). The driver still polls it once, so the path is run.
        response = power_pb2.PowerCommandFeedbackResponse(header=_header(request))
        response.status = _status(power_pb2.PowerCommandFeedbackResponse,
                                  self._world.power_feedback)
        return response


class _TimeSync(_Guarded, time_sync_service_pb2_grpc.TimeSyncServiceServicer):
    def TimeSyncUpdate(self, request, context):
        if not self._admit(context):
            return time_sync_pb2.TimeSyncUpdateResponse()
        if self._world.timesync_delay:
            time.sleep(self._world.timesync_delay)
        response = time_sync_pb2.TimeSyncUpdateResponse(header=_header(request))
        with self._world._lock:
            if request.clock_identifier:
                self._world.clocks.add(request.clock_identifier)
            else:
                request_clock = f"clock-{len(self._world.clocks) + 1}"
                self._world.clocks.add(request_clock)
                response.clock_identifier = request_clock
        previous = request.previous_round_trip
        response.previous_estimate.round_trip_time.FromNanoseconds(
            max(0, previous.client_rx.seconds - previous.client_tx.seconds) * 1_000_000_000)
        response.previous_estimate.clock_skew.FromNanoseconds(0)
        response.state.best_estimate.CopyFrom(response.previous_estimate)
        response.state.status = _status(time_sync_pb2.TimeSyncState, "STATUS_OK")
        response.state.measurement_time.CopyFrom(_now())
        return response


def _command_kind(command: robot_command_pb2.RobotCommand) -> str:
    """Which motion the command carries, by the oneof the builder set. Unknown shapes
    are `unknown`, counted and refused — a fake that executed what it could not name
    would be lying about a robot."""
    if command.HasField("synchronized_command"):
        mobility = command.synchronized_command.mobility_command
        if mobility.HasField("stand_request"):
            return "stand"
        if mobility.HasField("sit_request"):
            return "sit"
        if mobility.HasField("stop_request"):
            return "stop"
    if command.HasField("full_body_command"):
        if command.full_body_command.HasField("stop_request"):
            return "stop"
        if command.full_body_command.HasField("selfright_request"):
            return "selfright"
    return "unknown"


class _RobotCommand(_Guarded, robot_command_service_pb2_grpc.RobotCommandServiceServicer):
    def RobotCommand(self, request, context):
        if not self._admit(context):
            return robot_command_pb2.RobotCommandResponse()
        response = robot_command_pb2.RobotCommandResponse(header=_header(request))
        with self._world._lock:
            verdict = _lease_use(self._world, request.lease)
            response.lease_use_result.CopyFrom(verdict)
            ok = _status(lease_pb2.LeaseUseResult, "STATUS_OK")
            if verdict.status != ok:
                self._world.counts["lease_refused"] += 1
                response.status = _status(robot_command_pb2.RobotCommandResponse,
                                          "STATUS_UNKNOWN")
                return response
            if not request.clock_identifier or request.clock_identifier not in self._world.clocks:
                self._world.counts["no_timesync_refused"] += 1
                response.status = _status(robot_command_pb2.RobotCommandResponse,
                                          "STATUS_NO_TIMESYNC")
                return response
            if not self._world.powered:
                self._world.counts["unpowered_refused"] += 1
                response.status = _status(robot_command_pb2.RobotCommandResponse,
                                          "STATUS_NOT_POWERED_ON")
                return response
            if self._world.command_status is not None:
                response.status = _status(robot_command_pb2.RobotCommandResponse,
                                          self._world.command_status)
                return response
            kind = _command_kind(request.command)
            if kind == "unknown":
                response.status = _status(robot_command_pb2.RobotCommandResponse,
                                          "STATUS_INVALID_REQUEST")
                return response
            self._world.command_seq += 1
            response.robot_command_id = self._world.command_seq
            response.status = _status(robot_command_pb2.RobotCommandResponse, "STATUS_OK")
            self._world.last_command = kind
            self._world.counts[kind] += 1
            if kind == "stand":
                for name in self._world.joints:
                    self._world.joints[name] = (STAND[name.rsplit(".", 1)[1]], 0.0)
                self._world.body_z = 0.55
            elif kind == "sit":
                for name in self._world.joints:
                    self._world.joints[name] = (SIT[name.rsplit(".", 1)[1]], 0.0)
                self._world.body_z = 0.35
            elif kind == "selfright":
                for name in self._world.joints:
                    self._world.joints[name] = (STAND[name.rsplit(".", 1)[1]], 0.0)
                self._world.body_z = 0.55
            return response

    def RobotCommandFeedback(self, request, context):
        if not self._admit(context):
            return robot_command_pb2.RobotCommandFeedbackResponse()
        response = robot_command_pb2.RobotCommandFeedbackResponse(header=_header(request))
        with self._world._lock:
            verdict = _lease_use(self._world, request.lease)
            response.lease_use_result.CopyFrom(verdict)
            return response


class _RobotState(_Guarded, robot_state_service_pb2_grpc.RobotStateServiceServicer):
    def GetRobotState(self, request, context):
        if not self._admit(context):
            return robot_state_pb2.RobotStateResponse()
        if self._world.state_fails:
            context.abort(grpc.StatusCode.UNAVAILABLE, "the harness took the state away")
            return robot_state_pb2.RobotStateResponse()
        response = robot_state_pb2.RobotStateResponse(header=_header(request))
        state = response.robot_state
        with self._world._lock:
            motor_state = state.power_state.DESCRIPTOR.fields_by_name[
                "motor_power_state"].enum_type.values_by_name
            state.power_state.motor_power_state = motor_state[
                "STATE_ON" if self._world.powered else "STATE_OFF"].number
            battery = state.battery_states.add()
            battery.identifier = "fake-battery"
            battery.charge_percentage.value = 95.0
            for unique_id, endpoint in self._world.endpoints.items():
                slot = state.estop_states.add()
                slot.name = endpoint["name"]
                slot.type = robot_state_pb2.EStopState.TYPE_SOFTWARE
                slot.state = robot_state_pb2.EStopState.STATE_ESTOPPED \
                    if self._world.estopped else \
                    robot_state_pb2.EStopState.STATE_NOT_ESTOPPED
            if self._world.hw_estop:
                # A button on the robot: a second slot, stopped, beside the driver's own.
                slot = state.estop_states.add()
                slot.name = "hardware-button"
                slot.type = robot_state_pb2.EStopState.TYPE_HARDWARE
                slot.state = robot_state_pb2.EStopState.STATE_ESTOPPED
            for name, (position, velocity) in self._world.joints.items():
                if name in self._world.hidden_joints:
                    continue
                joint = state.kinematic_state.joint_states.add()
                joint.name = name
                joint.position.value = position
                joint.velocity.value = velocity
            twist = state.kinematic_state.velocity_of_body_in_vision
            (twist.linear.x, twist.linear.y, twist.linear.z) = self._world.body_twist[0]
            (twist.angular.x, twist.angular.y, twist.angular.z) = self._world.body_twist[1]
            edge = state.kinematic_state.transforms_snapshot.child_to_parent_edge_map["body"]
            edge.parent_frame_name = self._world.body_frame
            edge.parent_tform_child.position.x = self._world.body_xy[0]
            edge.parent_tform_child.position.y = self._world.body_xy[1]
            edge.parent_tform_child.position.z = self._world.body_z
            (edge.parent_tform_child.rotation.x, edge.parent_tform_child.rotation.y,
             edge.parent_tform_child.rotation.z,
             edge.parent_tform_child.rotation.w) = self._world.body_rotation
            for name in self._world.behavior_faults:
                behavior = state.behavior_fault_state.faults.add()
                behavior.behavior_fault_id = len(state.behavior_fault_state.faults)
                behavior.cause = robot_state_pb2.BehaviorFault.CAUSE_FALL
            for name, severity in self._world.faults:
                fault = state.system_fault_state.faults.add()
                fault.name = name
                fault.severity = robot_state_pb2.SystemFault.SEVERITY_WARN \
                    if severity == "SEVERITY_WARN" else \
                    robot_state_pb2.SystemFault.SEVERITY_CRITICAL
                fault.error_message = f"injected harness fault: {name}"
            return response


def _server_credentials() -> grpc.ServerCredentials:
    with open(f"{CERTS_DIR}/server-key.pem", "rb") as handle:
        key = handle.read()
    with open(f"{CERTS_DIR}/server.pem", "rb") as handle:
        chain = handle.read()
    return grpc.ssl_server_credentials(((key, chain),))


def serve(port: int, world: SpotWorld | None = None) -> tuple[grpc.Server, SpotWorld, int]:
    """Start the fake on 127.0.0.1:`port` (0 for ephemeral). Returns the server, the world
    it serves, and the bound port."""
    world = world if world is not None else SpotWorld()
    server = grpc.server(ThreadPoolExecutor(max_workers=16))
    auth_service_pb2_grpc.add_AuthServiceServicer_to_server(_Auth(world), server)
    directory_service_pb2_grpc.add_DirectoryServiceServicer_to_server(
        _Directory(world), server)
    estop_service_pb2_grpc.add_EstopServiceServicer_to_server(_Estop(world), server)
    lease_service_pb2_grpc.add_LeaseServiceServicer_to_server(_Lease(world), server)
    robot_state_service_pb2_grpc.add_RobotStateServiceServicer_to_server(
        _RobotState(world), server)
    robot_command_service_pb2_grpc.add_RobotCommandServiceServicer_to_server(
        _RobotCommand(world), server)
    power_service_pb2_grpc.add_PowerServiceServicer_to_server(_Power(world), server)
    time_sync_service_pb2_grpc.add_TimeSyncServiceServicer_to_server(
        _TimeSync(world), server)
    bound = server.add_secure_port(f"127.0.0.1:{port}", _server_credentials())
    server.start()
    return server, world, bound


def main(argv: list[str]) -> int:
    port = int(argv[1]) if len(argv) > 1 else 0
    server, world, bound = serve(port)
    print(f"skynet-spot fake listening on 127.0.0.1:{bound}", file=sys.stderr, flush=True)
    stopped = threading.Event()

    def _halt(_signum, _frame) -> None:
        stopped.set()

    signal.signal(signal.SIGTERM, _halt)
    signal.signal(signal.SIGINT, _halt)
    stopped.wait()
    server.stop(grace=2).wait()
    world.halt()
    print(world.report(), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))

"""A Fourier GR-1 humanoid over RoCS, through `python-host`.

`DRY-COVERAGE-PLAN.md`'s Phase 7 item 3: the buyable humanoids after the Unitrees.
Fourier publishes the interface as an MIT-licensed Python SDK (`fftai/rocs_client_py`):
an HTTP+WebSocket server on the robot, joint commands as WebSocket `move_joint`
messages addressed by `(no, orientation)`, per-motor position/velocity/current over
HTTP. No simulator ships with it, so the substrate is a loopback fake of the
documented shapes — the Pudu arrangement, one class up: joints this time, not tasks.

The driver is Python because the wire is HTTP+WebSocket: `urllib` and `socket` are
stdlib, and the one framing the stdlib lacks (RFC 6455) is `wscodec.py`, pinned to
the RFC's own test vectors. Python's tier ceiling is Guest (§31.3); a humanoid whose
commands cross Wi-Fi to an onboard server was never going to hold a real-time path
either, and the driver says so in `describe`.

What the driver implements, endpoint by endpoint (paths from the vendor SDK):

    open() probe   GET /robot/motor/limit/list   the joint set + live angle ranges
    read_state     POST /robot/motor/pvc          position per motor (degrees, strings)
    write_command  WS move_joint                  angles in degrees, pre-clamped
    emergency_stop POST /robot/stop               the vendor's documented emergency path

Three limitations are the vendor's, not this driver's, and all three are stated
wherever they bite:

    * RoCS publishes no joint-velocity or joint-torque *command*. `move_joint`
      moves to angles; velocity and torque modes are refused, not emulated.
    * The PVC velocity has no published unit and the current has no published
      torque conversion, so both read NaN (§24's rule that an unmeasured
      quantity is NaN and never zero). Position is degrees-to-radians, exact.
    * The motor inventory is runtime-only. The SDK publishes no name↔(no,
      orientation) map and no motor list, so joints are named by their motor
      address (`1-left`) and discovered live. Nothing here names a joint the
      robot did not report.

The envelope carries a torque ceiling only, the Unitree precedent with the same
hedge: 230 N·m is the GR series' published peak across the body (RoCS docs, "About
GR series robots"), not a per-joint rating, and velocity is unpublished anywhere —
so `joint_ratings` stays unmet and the `source` says so (F64). The watchdog window
is driver design, stated as such: the RoCS rate is unpublished.
"""

from __future__ import annotations

import http.client
import json
import math
import socket
import ssl
import sys
import threading
import time
import urllib.error
import urllib.request

import skynet_rhal as rhal
from skynet_fourier.wscodec import WsError, client_handshake
from skynet_fourier.wscodec import close as ws_close
from skynet_fourier.wscodec import send_text as ws_send

#: The GR series' published peak joint torque, N·m — across the body, not per
#: joint. RoCS docs, "About GR series robots": "up to 40 FSA joints, capable of
#: delivering a peak joint torque of 230 N.m", repeated in the spec table.
PEAK_JOINT_TORQUE_NM = 230.0

#: The watchdog window, in milliseconds. The RoCS publish rate is unpublished,
#: so unlike a bus rate this is driver design everywhere, stated as such in the
#: envelope: six sequential HTTP reads plus a WebSocket write fit it with room,
#: and a dead robot fails the read instead of the deadline machinery around it.
WATCHDOG_MS = 500.0

#: One HTTP call's budget while a session is being opened. RoCS answers on the
#: LAN, not a datacentre, so this is tighter than the Pudu cloud's. It is not
#: the budget of the state path or of the stop, which are below: the link-chaos
#: contract (DRY-COVERAGE-PLAN.md 12.C.4) measured a read that waited this out
#: on every one of its joints (6 joints at 3 s: 18 s) and a stop that waited it
#: out before it returned.
_HTTP_TIMEOUT_S = 5.0

#: What one joint's answer may take on the state path, and what all of them may
#: take together. A state is the whole body at one moment: one that needs more
#: than a second and a half to assemble is older than the control loop's two
#: seconds by the time it is complete, so it is EIO and not a late state. The
#: price is stated: a body of 40 joints (the GR series' most) has this much for
#: 40 round trips, which is 37 ms each.
_READ_TIMEOUT_S = 0.5
_READ_BUDGET_S = 1.5
_MIN_REQUEST_S = 0.01

#: A stop is told to the robot from its own thread, so `emergency_stop` returns
#: with the latch set and does not wait for a link it is stopping through. A
#: request that was lost is asked again, a few times, and then `read_state`
#: says what the robot does.
_STOP_TIMEOUT_S = 1.0
_STOP_ATTEMPTS = 5
_STOP_RETRY_S = 0.1

_LIMIT_LIST = "/robot/motor/limit/list"
_PVC = "/robot/motor/pvc"
_STOP = "/robot/stop"

#: §5.1's canonical chains and axes, mirrored from `skynet-contracts`' roles
#: table so config-supplied roles are validated the way `parse_role` will read
#: them — including its trim-and-lowercase normalization.
_CHAINS = frozenset({"left_arm", "right_arm", "left_leg", "right_leg",
                     "left_hand", "right_hand", "torso", "neck", "head",
                     "base", "tail", "aux"})
_AXES = frozenset({"pitch", "yaw", "roll", "slide", "flex", "abduct", "rotate",
                   "grip"})


def _normalise_role(role: object) -> str | None:
    """The canonical form of a §5.1 role, or None when it does not parse."""
    if not isinstance(role, str):
        return None
    text = role.strip().lower()
    if not text:
        return None
    head, _, tail = text.partition("#")
    index: str | None = None
    if "#" in text:
        # ASCII digits only (`str.isdigit` admits superscripts), and at most
        # ten of them — a u32, the way `parse_role` bounds it.
        if not tail or len(tail) > 10 or not tail.isascii() \
                or not tail.isdigit() or int(tail) > 0xFFFFFFFF:
            return None
        index = str(int(tail))
    parts = head.split(".")
    if len(parts) not in (2, 3):
        return None
    chain, link = parts[0], parts[1]
    axis = parts[2] if len(parts) == 3 else None
    if not chain or any(c not in "abcdefghijklmnopqrstuvwxyz_" for c in chain):
        return None
    if not link or any(c not in "abcdefghijklmnopqrstuvwxyz0123456789_"
                       for c in link):
        return None
    if axis is not None and (not axis or not axis.isalpha() or
                             not axis.islower()):
        return None
    if chain not in _CHAINS:
        return None
    if axis is not None and axis not in _AXES:
        return None
    out = f"{chain}.{link}"
    if axis is not None:
        out += f".{axis}"
    if index is not None:
        out += f"#{index}"
    return out


class RocsError(Exception):
    """A RoCS call that failed, with the words the driver answers in."""


class RocsClient:
    """The vendor SDK's wire, without the vendor SDK: JSON HTTP calls with the
    `{code, msg, data}` envelope, and one WebSocket for `move_joint`."""

    def __init__(self, host: str, port: int, secure: bool = False) -> None:
        self._host = host
        self._port = port
        scheme = "https" if secure else "http"
        self._base = f"{scheme}://{host}:{port}"
        self._secure = secure
        self._ws: socket.socket | None = None
        self._ws_lock = threading.Lock()

    def _call(self, method: str, path: str,
              payload: dict | None = None,
              timeout: float = _HTTP_TIMEOUT_S) -> object:
        body = json.dumps(payload or {}).encode("utf-8") if method == "POST" else None
        request = urllib.request.Request(
            self._base + path, data=body, method=method,
            headers={"Content-Type": "application/json",
                     "Accept": "application/json"})
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                raw = response.read().decode("utf-8")
                status = response.status
        except urllib.error.HTTPError as error:
            raise RocsError(f"{method} {path} returned HTTP {error.code}") from error
        except (urllib.error.URLError, TimeoutError, OSError, http.client.HTTPException) as error:
            # `http.client.HTTPException` is not an OSError: a reply cut off half way (`IncompleteRead`) or a status line that never came (`BadStatusLine`) is the robot not answering, and an entry point
            # returns a code and never raises: this one escaped `read_state` and `emergency_stop` alike (found by the link-chaos contract, DRY-COVERAGE-PLAN.md 12.C.4).
            raise RocsError(f"{method} {path} did not answer: {error}") from error
        if status != 200:
            raise RocsError(f"{method} {path} returned HTTP {status}")
        try:
            decoded = json.loads(raw)
        except json.JSONDecodeError as error:
            raise RocsError(f"{method} {path} answered unparsable JSON") from error
        # Success is HTTP 200 AND code 0 — the SDK's rule, both halves. The
        # SDK's own client ignores `code` on most calls; the driver does not.
        if not isinstance(decoded, dict) or decoded.get("code") != 0:
            detail = decoded.get("msg", raw[:120]) if isinstance(decoded, dict) \
                else raw[:120]
            raise RocsError(f"{method} {path} refused: {detail}")
        return decoded.get("data", {})

    def post(self, path: str, payload: dict,
             timeout: float = _HTTP_TIMEOUT_S) -> dict:
        data = self._call("POST", path, payload, timeout)
        if not isinstance(data, dict):
            raise RocsError(f"POST {path} answered no object")
        return data

    def get_list(self, path: str) -> list:
        """GET where `data` is a list (`limit/list`), not a dict."""
        data = self._call("GET", path)
        if not isinstance(data, list):
            raise RocsError(f"GET {path} answered no list")
        return data

    def ws_connect(self) -> None:
        """Open the command WebSocket. Exactly one, held for the session."""
        with self._ws_lock:
            if self._ws is not None:
                return
            sock: socket.socket | None = None
            try:
                sock = socket.create_connection((self._host, self._port),
                                                timeout=_HTTP_TIMEOUT_S)
                if self._secure:
                    context = ssl.create_default_context()
                    sock = context.wrap_socket(sock, server_hostname=self._host)
                sock.settimeout(_HTTP_TIMEOUT_S)
                client_handshake(sock, self._host, self._port)
            except (OSError, WsError) as error:
                if sock is not None:
                    try:
                        sock.close()
                    except OSError:
                        pass
                raise RocsError(f"the command channel did not open: {error}") \
                    from error
            self._ws = sock

    def ws_command(self, command: dict) -> None:
        """Send one `{command, data}` message. A dead channel is EIO's words,
        and the channel stays dead: reconnecting mid-session would be a driver
        deciding the robot is back, which is the operator's call."""
        with self._ws_lock:
            sock = self._ws
            if sock is None:
                raise RocsError("the command channel is not open")
            try:
                ws_send(sock, json.dumps(command), client=True)
            except (OSError, WsError) as error:
                raise RocsError(f"the command channel broke: {error}") from error

    def ws_close(self) -> None:
        with self._ws_lock:
            sock, self._ws = self._ws, None
        if sock is not None:
            ws_close(sock, client=True)
            try:
                sock.close()
            except OSError:
                pass


class FourierDriver:
    """The RHAL side: position joints over RoCS, discovered live."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._opened = False
        self._estopped = False
        self._stop_threads: list[threading.Thread] = []
        self._seq = 0
        self._client: RocsClient | None = None
        #: (name, no, orientation, min_rad, max_rad), in limit/list order.
        self._joints: list[tuple[str, str, str, float, float]] = []
        #: Operator-supplied canonical roles by joint name (§5.1). Never
        #: inferred: RoCS publishes no motor map, so a driver that guessed
        #: which motor is whose elbow would be making the remapping it enables
        #: a lie. Absent a human, joints carry no role and match by exact name.
        self._roles: dict[str, str] = {}
        self._simulator = True

    # -- open ------------------------------------------------------------
    def open(self, config_json: str) -> int:
        with self._lock:
            if self._opened:
                return rhal.SKR_EPERM
            try:
                config = json.loads(config_json or "{}")
            except json.JSONDecodeError:
                return rhal.SKR_EINVAL
            host = config.get("host", "127.0.0.1")
            port = config.get("port", 8001)
            secure = bool(config.get("ssl", False))
            if not isinstance(host, str) or not host:
                print("skynet-fourier: `host` names the robot's RoCS server "
                      "(default 127.0.0.1).", file=sys.stderr, flush=True)
                return rhal.SKR_EINVAL
            if not isinstance(port, int) or not 1 <= port <= 65535:
                print("skynet-fourier: `port` is 1-65535 (default 8001).",
                      file=sys.stderr, flush=True)
                return rhal.SKR_EINVAL
            self._simulator = bool(config.get("simulator", True))
            client = RocsClient(host, port, secure)
            # Fail fast, twice: the joint set proves the HTTP side and names
            # the body, the WebSocket proves the command side — distinguishing
            # "config wrong" (now) from "robot died" (later, EIO on the call).
            try:
                limits = client.get_list(_LIMIT_LIST)
                joints = _parse_limits(limits)
            except RocsError as error:
                print(f"skynet-fourier: the limit probe failed: {error}. Check "
                      f"host/port and that RoCS is serving.", file=sys.stderr,
                      flush=True)
                return rhal.SKR_EIO
            if not joints:
                print("skynet-fourier: the robot reported no motors. A body "
                      "with no joints is a misconfiguration, not a Fourier.",
                      file=sys.stderr, flush=True)
                return rhal.SKR_EIO
            roles = config.get("roles", {})
            if not isinstance(roles, dict):
                print("skynet-fourier: `roles` maps joint names to §5.1 "
                      "canonical roles.", file=sys.stderr, flush=True)
                return rhal.SKR_EINVAL
            known = {name for name, _, _, _, _ in joints}
            normalised: dict[str, str] = {}
            for name, role in roles.items():
                if name not in known:
                    print(f"skynet-fourier: roles names {name!r}, which this "
                          f"robot did not report. Typo'd motor addresses fail "
                          f"here, not silently.", file=sys.stderr, flush=True)
                    return rhal.SKR_EINVAL
                parsed = _normalise_role(role)
                if parsed is None:
                    print(f"skynet-fourier: {role!r} is not a §5.1 canonical "
                          f"role (chain.link[.axis][#index]).",
                          file=sys.stderr, flush=True)
                    return rhal.SKR_EINVAL
                normalised[name] = parsed
            try:
                client.ws_connect()
            except RocsError as error:
                print(f"skynet-fourier: {error}.", file=sys.stderr, flush=True)
                return rhal.SKR_EIO
            self._client = client
            self._joints = joints
            self._roles = normalised
            self._opened = True
            return rhal.SKR_OK

    def close(self) -> int:
        # A stop that was just asked for is told before the session goes: the host closes straight after a stop.
        with self._lock:
            threads = list(self._stop_threads)
        for thread in threads:
            thread.join(timeout=_STOP_TIMEOUT_S)
        with self._lock:
            client, self._client = self._client, None
            self._opened = False
            self._joints = []
            self._roles = {}
        if client is not None:
            client.ws_close()
        return rhal.SKR_OK

    # -- read_state ------------------------------------------------------
    def read_state(self, out, deadline_ns: int) -> int:
        _ = deadline_ns
        with self._lock:
            if not self._opened or self._client is None:
                return rhal.SKR_EPERM
            client, joints, estopped = self._client, list(self._joints), \
                self._estopped
        positions: list[float] = []
        began = time.monotonic()
        for _, no, orientation, _, _ in joints:
            left = _READ_BUDGET_S - (time.monotonic() - began)
            if left < _MIN_REQUEST_S:
                print(f"skynet-fourier: state read gave up: the robot did not answer for all {len(joints)} joints in "
                      f"{_READ_BUDGET_S:.1f} s ({len(positions)} were read). A slow link or a robot busy elsewhere, not a "
                      f"refusal; the next read tries again.", file=sys.stderr, flush=True)
                return rhal.SKR_EIO
            try:
                pvc = client.post(_PVC, {"no": no, "orientation": orientation},
                                  timeout=min(_READ_TIMEOUT_S, left))
            except RocsError as error:
                print(f"skynet-fourier: state read failed: {error}",
                      file=sys.stderr, flush=True)
                return rhal.SKR_EIO
            try:
                # Degrees, as strings ("85.00") — the docstring's shape. The
                # velocity rides along unread and the current too: no unit for
                # one, no torque conversion for the other, so both stay NaN
                # below rather than inheriting a neighbour's meaning.
                positions.append(math.radians(float(pvc["position"])))
            except (KeyError, TypeError, ValueError):
                print(f"skynet-fourier: motor {no}-{orientation} answered no "
                      f"position.", file=sys.stderr, flush=True)
                return rhal.SKR_EIO
        with self._lock:
            self._seq += 1
            out.stamp_ns = rhal.acquisition_ns()
            out.seq = self._seq
            out.flags = rhal.FLAG_ESTOP if estopped else 0
            out.fault_code = 0
            out.position_rad = positions
            out.velocity_rad_s = [math.nan] * len(positions)
            out.effort_nm = [math.nan] * len(positions)
            # Base unlocalised: the IMU the spec table ticks is not on this
            # wire, and the pushed `basestate` is an undocumented frame — §24.
            out.base_pose = [math.nan] * 7
            out.base_twist = [0.0] * 6
            return rhal.SKR_OK

    # -- write_command ---------------------------------------------------
    def write_command(self, cmd, deadline_ns: int) -> int:
        _ = deadline_ns
        with self._lock:
            if not self._opened or self._client is None:
                return rhal.SKR_EPERM
            if cmd.joint_count != len(self._joints):
                return rhal.SKR_EINVAL
            if cmd.mode not in (rhal.MODE_POSITION, rhal.MODE_VELOCITY,
                                rhal.MODE_TORQUE):
                return rhal.SKR_EINVAL
            if cmd.modes and len(cmd.modes) != len(self._joints):
                return rhal.SKR_EINVAL
            for entry in cmd.modes:
                # §86.1's transcription error, refused where it lands: a bare
                # MODE_* in this array is not a mode at all. The host refuses
                # it too — a transport is not where a malformed command stops
                # being malformed.
                if entry not in (rhal.JMODE_INHERIT, rhal.JMODE_POSITION,
                                 rhal.JMODE_VELOCITY, rhal.JMODE_TORQUE):
                    print(f"skynet-fourier: not a per-joint mode: {entry:#04x} "
                          f"(§86.1).", file=sys.stderr, flush=True)
                    return rhal.SKR_EINVAL
            if self._estopped:
                return rhal.SKR_EPERM
            client, joints = self._client, list(self._joints)
        targets: list[dict] = []
        for index, value in cmd.applied():
            if index >= len(joints):
                return rhal.SKR_EINVAL
            if cmd.mode_of(index) != rhal.MODE_POSITION:
                print(f"skynet-fourier: joint {index} is commanded in a mode "
                      f"this body does not move in. RoCS publishes no "
                      f"joint-velocity or joint-torque command — position "
                      f"only.", file=sys.stderr, flush=True)
                return rhal.SKR_EPERM
            _, no, orientation, lo, hi = joints[index]
            # Clamp to the live limits, the vendor client's `_cover_param`
            # behavior: the robot takes angles inside its range, and the
            # degree round-trip is exact to the fake's formats.
            degrees = min(max(math.degrees(value), math.degrees(lo)),
                          math.degrees(hi))
            targets.append({"no": no, "orientation": orientation,
                            "angle": degrees})
        if not targets:
            # Every joint masked: hold, by sending nothing. `move_joint` moves
            # the motors it names, so the unnamed ones keep their setpoint —
            # which is what a mask means.
            return rhal.SKR_OK
        try:
            client.ws_command({"command": "move_joint",
                               "data": {"command": targets}})
        except RocsError as error:
            print(f"skynet-fourier: command failed: {error}", file=sys.stderr,
                  flush=True)
            return rhal.SKR_EIO
        return rhal.SKR_OK

    # -- emergency_stop --------------------------------------------------
    def emergency_stop(self) -> int:
        """POST the vendor's documented emergency path, then latch regardless.

        `/robot/stop` "takes precedence over other commands" and is what the
        SDK recommends "in emergency situations or when an immediate stop is
        necessary" — but it is software over HTTP to the onboard server, not an
        independent circuit, so the descriptor claims no `safety.estop` and the
        latch is what refuses later motion. The latch holds even if the POST
        failed: a stop that depends on the network it is stopping through is a
        suggestion.
        """
        with self._lock:
            client = self._client
            self._estopped = True
            if client is not None:
                thread = threading.Thread(target=self._tell_robot_to_stop, args=(client,),
                                          daemon=True, name="fourier-stop")
                self._stop_threads = [t for t in self._stop_threads if t.is_alive()] + [thread]
                thread.start()
        return rhal.SKR_OK

    def _tell_robot_to_stop(self, client: RocsClient) -> None:
        """The stop call, from its own thread: the latch is already holding, so this is the robot being told."""
        for attempt in range(_STOP_ATTEMPTS):
            try:
                client.post(_STOP, {}, timeout=_STOP_TIMEOUT_S)
                return
            except RocsError as error:
                if attempt == _STOP_ATTEMPTS - 1:
                    print(f"skynet-fourier: the stop call failed {_STOP_ATTEMPTS} times, the last ({error}); "
                          f"the latch holds anyway.", file=sys.stderr, flush=True)
                    return
                with self._lock:
                    if self._client is not client:
                        return
                time.sleep(_STOP_RETRY_S)

    def release_stop(self) -> int:
        """Clear the session latch for §48.2's power-on self-test.

        The latch is what this driver holds: whether `/robot/stop` latches anything
        server-side is undocumented, and the SDK names no resume, so there is no call
        that provably returns the server. The verifier's hold command is the proof
        that commands flow again — accepted here, and executed or refused where the
        server says. Idempotent.
        """
        with self._lock:
            self._estopped = False
            return rhal.SKR_OK

    # -- describe ----------------------------------------------------------
    def describe(self) -> str:
        with self._lock:
            joints = list(self._joints)
            roles = dict(self._roles)
            simulator = self._simulator
        described = []
        for name, _, _, lo, hi in joints:
            joint: dict[str, object] = {
                "name": name, "type": "revolute",
                "control_modes": ["position"],
                "limits": {"position_min_rad": lo, "position_max_rad": hi}}
            if name in roles:
                joint["role"] = roles[name]
            described.append(joint)
        return json.dumps({
            "schema_version": 1, "robot_slug": "fourier-gr1", "digest": None,
            "verified": False, "simulator": simulator, "base": "legged",
            "dof": len(joints),
            "joints": described,
            "end_effectors": [], "sensors": [],
            "control": {"modes": ["position"], "per_joint_modes": True,
                        "rate_hz_max": None, "rate_hz_min": None,
                        "command_latency_ms": None},
            # Torque ceiling only, the Unitree hedge: 230 N·m is the published
            # peak across the GR body, not a per-joint rating, and no joint
            # velocity is published anywhere — so `joint_ratings` stays unmet
            # and this source says so (F64) rather than inventing one. The
            # watchdog window is driver design: the RoCS rate is unpublished.
            "envelope": {
                "joint_torque_max_nm": PEAK_JOINT_TORQUE_NM,
                "watchdog_ms": WATCHDOG_MS,
                "source": ("Fourier RoCS docs, 'About GR series robots': up to "
                           "40 FSA joints, peak joint torque 230 N·m (the "
                           "body's max, not a per-joint rating); joint "
                           "velocities unpublished, so joint_ratings is unmet; "
                           "the watchdog window is driver design")},
            "capabilities": [],
            "extensions": [], "solvers": [],
        })

    def read_sensor(self, name: str, out) -> int:
        _ = (name, out)
        return rhal.SKR_ENOSYS

    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        _ = data
        return (rhal.SKR_ENOSYS,
                f"unknown token {token!r}. This driver serves no extension: "
                f"the joints are the interface.".encode("utf-8"))


def _parse_limits(limits: list) -> list[tuple[str, str, str, float, float]]:
    """The limit/list entries into joints. Entries that do not parse are
    skipped, not guessed: a joint the robot described ambiguously is a joint
    this driver does not move."""
    joints = []
    for entry in limits:
        if not isinstance(entry, dict):
            continue
        try:
            no = str(entry["no"])
            orientation = str(entry["orientation"])
            lo = math.radians(float(entry["min_angle"]))
            hi = math.radians(float(entry["max_angle"]))
        except (KeyError, TypeError, ValueError):
            continue
        if not no or not orientation or not math.isfinite(lo) \
                or not math.isfinite(hi) or lo >= hi:
            continue
        joints.append((f"{no}-{orientation}", no, orientation, lo, hi))
    return joints

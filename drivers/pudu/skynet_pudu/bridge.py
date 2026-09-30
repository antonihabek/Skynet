"""A Pudu service robot over the Pudu OpenAPI cloud, through `python-host`.

`DRY-COVERAGE-PLAN.md`'s Phase 7 item 4: one service-robot driver on the execute path, for a class
that today has nothing. Pudu, Temi, Keenon and Bear share a body plan from this OS's side — a
wheeled base with no joints the OS can see, moved by task calls — and of the four, Pudu is the one
whose task API is published as machine-readable OpenAPI (pudu-robotics/skills on GitHub): delivery,
transport, errand, cruise and call endpoints with an HMAC-SHA1 request scheme. Keenon publishes no
public API at all; Temi's is an Android SDK, which is not a driver shape. So this driver speaks
Pudu's cloud, and the class gets its shape from the one vendor that documents it.

The driver is Python because the cloud is HTTPS: `urllib` and `hmac` are stdlib, and a C driver
would need a TLS library the drivers here do not take on. Python's tier ceiling is Guest (§31.3),
and the service class is Guest ceiling anyway — a policy that dispatches deliveries over the
internet was never going to hold a real-time path.

What the driver implements, endpoint by endpoint (paths from the skill's OpenAPI assets):

    read_state      GET .../v1/robot/get_position      base pose (x, y, z; no yaw published)
                    GET .../v1/robot/task/state/get    (read for transport errors; the task
                                                       vocabulary is undocumented, so no fault
                                                       word is derived from it)
    deliver         POST .../v1/delivery_task           one tray, N destinations by map point
    cruise          POST .../v1/cruise_task             by cruise-line name
    call            POST .../v1/custom_call             by map point
    cancel_call     POST .../v1/custom_call/cancel      by task id
    open() probe    GET .../v2/status/get_by_sn         reachability + credentials, fail fast

`write_command` refuses everything with EPERM: there are no joints, and the pendant's shape (§4)
is the honest one — the only route that moves this robot is `extension` under `vendor.pudu.task`.

Two limitations are the vendor's, not this driver's, and both are stated wherever they bite:

    * Pudu publishes no way to stop a delivery. No cancel endpoint exists for delivery,
      transport, errand or cruise tasks — only calls cancel — so `emergency_stop` latches
      locally, refuses every later motion, and says so in words. It cannot recall the cloud's
      task, and it does not claim to.
    * Pudu publishes no robot fault vocabulary. Task states are free strings and no error
      enumeration is documented, so the fault word stays 0. A transport failure arrives as EIO
      on the call that observed it, not as a latched fault.

The signing scheme is transcribed from the skill's `request-sdk.md` (my own code, from the
documented scheme — the skill repo carries no licence, so nothing is copied from it): GMT date,
Content-MD5 as base64-of-hex for POST, the six-line string-to-sign, HMAC-SHA1, `hmac` auth
scheme. The fake cloud re-verifies every signature, and `run-check.sh` additionally runs the
vendor's own `pudu-request.py` against the fake — if the fake accepts the vendor's signature and
the suite accepts the fake, the driver's signing matches the vendor's construction through two
independent implementations.
"""

from __future__ import annotations

import base64
import datetime
import hashlib
import hmac
import json
import math
import re
import sys
import threading
import urllib.error
import urllib.parse
import urllib.request

import skynet_rhal as rhal

TOKEN = "vendor.pudu.task"

#: Production clusters, hostnames from the vendor's own request tool.
CLUSTERS = {
    "cn": "https://open-platform.pudutech.com",
    "sea": "https://css-open-platform.pudutech.com",
    "de": "https://csg-open-platform.pudutech.com",
    "us": "https://csu-open-platform.pudutech.com",
}

_GET_POSITION = "/open-platform-service/v1/robot/get_position"
_GET_TASK_STATE = "/open-platform-service/v1/robot/task/state/get"
_GET_STATUS = "/open-platform-service/v2/status/get_by_sn"
_POST_DELIVERY = "/open-platform-service/v1/delivery_task"
_POST_CRUISE = "/open-platform-service/v1/cruise_task"
_POST_CALL = "/open-platform-service/v1/custom_call"
_POST_CALL_CANCEL = "/open-platform-service/v1/custom_call/cancel"

#: One HTTP call's budget, both ways. The cloud is not a bus: ten seconds is generous for a
#: datacentre round trip and short enough that a dead network fails the read instead of the
#: deadline machinery around it.
_HTTP_TIMEOUT_S = 10.0


class PuduError(Exception):
    """A cloud call that failed, with the words the driver answers in."""


def _gmt_date() -> str:
    # `Date.toUTCString` shape, e.g. "Sat, 20 Sep 2026 19:00:00 GMT" — the scheme signs this
    # exact string, so the format is load-bearing.
    return datetime.datetime.now(datetime.timezone.utc).strftime("%a, %d %b %Y %H:%M:%S GMT")


def _content_md5(body: bytes) -> str:
    # Base64 of the HEX digest — not of the raw digest. The vendor's tool hexes first and the
    # fake re-verifies, so either reading would fail loudly against the other.
    return base64.b64encode(hashlib.md5(body).hexdigest().encode("ascii")).decode("ascii")


def _sign_path(path: str) -> str:
    # Environment prefixes are stripped before signing (`/release`, `/test`, `/prepub`); any
    # other prefix (`/prod`) is kept. Transcribed from the vendor's request tool, which is the
    # only place the rule is written down.
    for prefix in ("/release", "/test", "/prepub"):
        if path == prefix:
            return "/"
        if path.startswith(prefix + "/"):
            return path[len(prefix):]
    return path or "/"


def sign_request(
    method: str, path: str, query: str, body: bytes, app_key: str, app_secret: str,
) -> tuple[str, str, str]:
    """The skill's signing scheme, returning `(date, content_md5, authorization)`.

    `query` is the sorted `k=v&...` string for GET, empty for POST. `path` excludes the host;
    the signed path strips the environment prefixes above and appends the DECODED query. Pure
    function of its arguments, so the fake cloud can recompute it byte for byte.
    """
    date = _gmt_date()
    md5 = _content_md5(body) if method == "POST" else ""
    base = _sign_path(path)
    sign_path = base if not query else base + "?" + urllib.parse.unquote(query)
    signing = "\n".join(["x-date: " + date, method, "application/json", "application/json",
                         md5, sign_path])
    digest = hmac.new(app_secret.encode("utf-8"), signing.encode("utf-8"),
                      hashlib.sha1).digest()
    signature = base64.b64encode(digest).decode("ascii")
    auth = (f'hmac id="{app_key}", algorithm="hmac-sha1", '
            f'headers="x-date", signature="{signature}"')
    return date, md5, auth


class PuduCloud:
    """One robot's slice of the Pudu OpenAPI: signed calls, SUCCESS-or-raise."""

    def __init__(self, base_url: str, sn: str, app_key: str, app_secret: str) -> None:
        self._base = base_url.rstrip("/")
        self._sn = sn
        self._key = app_key
        self._secret = app_secret

    def _call(self, method: str, path: str, params: dict | None = None,
              payload: dict | None = None) -> dict:
        query = ""
        body = b""
        if method == "GET" and params:
            query = "&".join(f"{k}={urllib.parse.quote(str(v), safe='')}"
                             for k, v in sorted(params.items()))
        if method == "POST":
            # `ensure_ascii=False`, exactly as the vendor's tool serialises: non-ASCII bytes
            # unescaped. The MD5 — and therefore the signature — is over these bytes, so a
            # map point in CJK would sign differently under the default escaping. The
            # cross-check against the vendor's own script pins this with a CJK payload.
            body = json.dumps(payload or {}, separators=(",", ":"),
                              ensure_ascii=False).encode("utf-8")
        # The signature is over the URL's path, and the base URL is part of that path: an
        # environment lives in the base (`https://host/prod`), and `/prod` is signed where
        # `/release` is not. Signing the endpoint's own path alone left `_sign_path`'s rule
        # nothing to apply it to, and every call under a `/prod` base failed authentication (F189,
        # found by the seeded-defect run: no check signed under anything but the bare endpoint).
        signed = urllib.parse.urlparse(self._base).path.rstrip("/") + path
        date, md5, auth = sign_request(method, signed, query, body, self._key, self._secret)
        url = self._base + path + ("?" + query if query else "")
        request = urllib.request.Request(
            url, data=body if method == "POST" else None, method=method,
            headers={"Content-Type": "application/json", "Accept": "application/json",
                     "Content-MD5": md5, "x-date": date, "Authorization": auth})
        try:
            with urllib.request.urlopen(request, timeout=_HTTP_TIMEOUT_S) as response:
                raw = response.read().decode("utf-8")
                status = response.status
        except urllib.error.HTTPError as error:
            raise PuduError(f"{method} {path} returned HTTP {error.code}") from error
        except (urllib.error.URLError, TimeoutError, OSError) as error:
            raise PuduError(f"{method} {path} did not answer: {error}") from error
        if status != 200:
            raise PuduError(f"{method} {path} returned HTTP {status}")
        try:
            decoded = json.loads(raw)
        except json.JSONDecodeError as error:
            raise PuduError(f"{method} {path} answered unparsable JSON") from error
        # Success is HTTP 200 AND the envelope's message — the skill's rule, both halves.
        if decoded.get("message") != "SUCCESS":
            detail = decoded.get("message", raw[:120])
            raise PuduError(f"{method} {path} refused: {detail}")
        return decoded.get("data", {})

    def get(self, path: str, params: dict | None = None) -> dict:
        return self._call("GET", path, params=params)

    def post(self, path: str, payload: dict) -> dict:
        return self._call("POST", path, payload=payload)


class PuduDriver:
    """The RHAL side: no joints, task motions under `vendor.pudu.task`, pose from the cloud."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._opened = False
        self._estopped = False
        self._seq = 0
        self._cloud: PuduCloud | None = None
        self._sn = ""
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
            sn = config.get("sn", "")
            app_key = config.get("app_key", "")
            app_secret = config.get("app_secret", "")
            base_url = config.get("base_url", "")
            if not base_url:
                cluster = config.get("cluster", "")
                if cluster not in CLUSTERS:
                    print("skynet-pudu: name the cloud: `cluster` (cn/sea/de/us) or `base_url` "
                          "for a test endpoint.", file=sys.stderr, flush=True)
                    return rhal.SKR_EINVAL
                base_url = CLUSTERS[cluster]
            if not sn:
                print("skynet-pudu: no `sn` in the config. The cloud addresses robots by "
                      "serial number and no call can be made without one.", file=sys.stderr,
                      flush=True)
                return rhal.SKR_EINVAL
            if re.fullmatch(r"[0-9A-Fa-f]{2}(:[0-9A-Fa-f]{2}){5}", sn):
                # The vendor's tool refuses a MAC in the sn field — a robot serial is not a
                # network address, and the cloud would fail the call less legibly.
                print(f"skynet-pudu: the sn {sn!r} looks like a MAC address. Pass the robot "
                      f"serial, not a network address.", file=sys.stderr, flush=True)
                return rhal.SKR_EINVAL
            if not app_key or not app_secret:
                print("skynet-pudu: no `app_key`/`app_secret` in the config. Every call is "
                      "signed and an unsigned driver would fail auth on each one.",
                      file=sys.stderr, flush=True)
                return rhal.SKR_EINVAL
            self._simulator = bool(config.get("simulator", True))
            cloud = PuduCloud(base_url, sn, app_key, app_secret)
            # Fail fast: one status read proves the URL resolves, the credentials sign, and
            # the robot exists — distinguishing "config wrong" (now) from "cloud died"
            # (later, EIO on the call).
            try:
                cloud.get(_GET_STATUS, {"sn": sn})
            except PuduError as error:
                print(f"skynet-pudu: the cloud probe failed: {error}. Check base_url, the "
                      f"credentials, and that {sn} exists.", file=sys.stderr, flush=True)
                return rhal.SKR_EIO
            self._cloud = cloud
            self._sn = sn
            self._opened = True
            return rhal.SKR_OK

    def close(self) -> int:
        with self._lock:
            self._opened = False
            self._cloud = None
            return rhal.SKR_OK

    # -- read_state ------------------------------------------------------
    def read_state(self, out, deadline_ns: int) -> int:
        _ = deadline_ns
        with self._lock:
            if not self._opened or self._cloud is None:
                return rhal.SKR_EPERM
            cloud, estopped = self._cloud, self._estopped
        try:
            position = cloud.get(_GET_POSITION, {"sn": self._sn})
            cloud.get(_GET_TASK_STATE, {"sn": self._sn})
        except PuduError as error:
            print(f"skynet-pudu: state read failed: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO
        with self._lock:
            self._seq += 1
            out.stamp_ns = rhal.acquisition_ns()
            out.seq = self._seq
            out.flags = rhal.FLAG_ESTOP if estopped else 0
            # The fault word stays 0 — see the module docstring. And the joint lists stay
            # empty: `joint_count` is their length, so there is nothing else to set.
            out.fault_code = 0
            out.position_rad = []
            out.velocity_rad_s = []
            out.effort_nm = []
            xyz = position.get("position", {}) if isinstance(position, dict) else {}
            try:
                x, y, z = float(xyz["x"]), float(xyz["y"]), float(xyz.get("z", 0.0))
            except (KeyError, TypeError, ValueError):
                return rhal.SKR_EIO
            # Position known, orientation unknown: the cloud publishes no yaw, so the
            # quaternion is NaN rather than zeros (§24's rule).
            out.base_pose = [x, y, z, math.nan, math.nan, math.nan, math.nan]
            out.base_twist = [0.0] * 6
            return rhal.SKR_OK

    # -- write_command ---------------------------------------------------
    def write_command(self, cmd, deadline_ns: int) -> int:
        _ = deadline_ns
        with self._lock:
            if not self._opened:
                return rhal.SKR_EPERM
        # The shape before the permission, the way the pendant checks it: a malformed command
        # is EINVAL even from a driver that would refuse a well-formed one, because a body
        # that refused everything with one code would hide a real defect behind a true
        # statement.
        if cmd.joint_count != 0:
            return rhal.SKR_EINVAL
        if cmd.mode not in (rhal.MODE_POSITION, rhal.MODE_VELOCITY, rhal.MODE_TORQUE):
            return rhal.SKR_EINVAL
        print("skynet-pudu: joint commands are refused: this body has no joints, and the only "
              "route that moves it is extension under vendor.pudu.task.", file=sys.stderr,
              flush=True)
        return rhal.SKR_EPERM

    # -- extension -------------------------------------------------------
    def extension(self, token: str, data: bytes) -> tuple[int, bytes]:
        if token != TOKEN:
            return (rhal.SKR_ENOSYS,
                    f"unknown token {token!r}. This driver serves {TOKEN}.".encode("utf-8"))
        with self._lock:
            if not self._opened or self._cloud is None:
                return (rhal.SKR_EPERM, b"the driver is not open.")
            if self._estopped:
                return (rhal.SKR_EPERM,
                        b"refused: the emergency stop is latched. It cannot recall the cloud's "
                        b"in-flight task (no API for that exists) but it issues no more.")
            cloud = self._cloud
        try:
            text = data.decode("utf-8")
        except UnicodeDecodeError:
            return (rhal.SKR_EINVAL, b"the request is text: `<motion> k=v k=v` in UTF-8.")
        words = text.split()
        if not words:
            return (rhal.SKR_EINVAL, b"empty request. Motions: deliver, cruise, call, "
                                     b"cancel_call.")
        motion, args = words[0], {}
        for word in words[1:]:
            if "=" not in word:
                return (rhal.SKR_EINVAL,
                        f"malformed argument {word!r}: wanted k=v.".encode("utf-8"))
            key, _, value = word.partition("=")
            args[key] = value
        try:
            if motion == "deliver":
                return self._deliver(cloud, args)
            if motion == "cruise":
                return self._cruise(cloud, args)
            if motion == "call":
                return self._call(cloud, args)
            if motion == "cancel_call":
                return self._cancel_call(cloud, args)
            return (rhal.SKR_EINVAL,
                    f"unknown motion {motion!r}. Motions: deliver, cruise, call, "
                    f"cancel_call.".encode("utf-8"))
        except PuduError as error:
            return (rhal.SKR_EIO, f"the cloud refused: {error}".encode("utf-8"))

    def _deliver(self, cloud: PuduCloud, args: dict) -> tuple[int, bytes]:
        points = [p for p in args.get("points", "").split(",") if p]
        if not points:
            return (rhal.SKR_EINVAL, b"deliver needs points=A01,B02: map points, comma-separated.")
        body = {"sn": self._sn,
                "payload": {"trays": [{"destinations": [{"points": p} for p in points]}]}}
        answer = cloud.post(_POST_DELIVERY, body)
        return (rhal.SKR_OK, json.dumps(
            {"taskId": answer.get("taskId"), "taskStatus": answer.get("taskStatus"),
             "queueNo": answer.get("queueNo")}).encode("utf-8"))

    def _cruise(self, cloud: PuduCloud, args: dict) -> tuple[int, bytes]:
        line = args.get("line", "")
        if not line:
            return (rhal.SKR_EINVAL, b"cruise needs line=NAME: the cruise line's name.")
        answer = cloud.post(_POST_CRUISE, {"sn": self._sn,
                                           "payload": {"mapCruiseName": line}})
        return (rhal.SKR_OK, json.dumps(
            {"taskId": answer.get("taskId"), "taskStatus": answer.get("taskStatus")}).encode())

    def _call(self, cloud: PuduCloud, args: dict) -> tuple[int, bytes]:
        point = args.get("point", "")
        if not point:
            return (rhal.SKR_EINVAL, b"call needs point=A01: the map point to come to.")
        answer = cloud.post(_POST_CALL, {"sn": self._sn, "point": point})
        return (rhal.SKR_OK, json.dumps({"taskId": answer.get("taskId")}).encode("utf-8"))

    def _cancel_call(self, cloud: PuduCloud, args: dict) -> tuple[int, bytes]:
        task_id = args.get("id", "")
        if not task_id:
            return (rhal.SKR_EINVAL, b"cancel_call needs id=TASK: the call's task id.")
        cloud.post(_POST_CALL_CANCEL, {"sn": self._sn, "taskId": task_id})
        return (rhal.SKR_OK, b"{\"cancelled\":true}")

    # -- emergency_stop ---------------------------------------------------
    def emergency_stop(self) -> int:
        """Latch locally. Pudu publishes no stop for a delivery — no cancel endpoint exists
        for delivery, transport, errand or cruise — so there is no cloud call that recalls
        the in-flight task. What latches is this driver: no more motions, and the flag in
        every later state. Returning OK for a latch that held, with the limitation in the
        extension's words and in the README."""
        with self._lock:
            self._estopped = True
            return rhal.SKR_OK

    def release_stop(self) -> int:
        """Clear the session latch for §48.2's power-on self-test.

        The latch is all the stop holds — there was no cloud call that recalled the
        in-flight task, so there is none that returns it either. Clearing permits
        new motions; the task that was flying when the stop landed is the cloud's
        to finish or abandon. Idempotent.
        """
        with self._lock:
            self._estopped = False
            return rhal.SKR_OK

    # -- describe ----------------------------------------------------------
    def describe(self) -> str:
        with self._lock:
            simulator, sn = self._simulator, self._sn or "unbound"
        return json.dumps({
            "schema_version": 1, "robot_slug": f"pudu-{sn}", "digest": None,
            "verified": False, "simulator": simulator, "base": "wheeled", "dof": 0,
            "joints": [], "end_effectors": [], "sensors": [],
            "control": {"modes": [], "rate_hz_max": None, "rate_hz_min": None,
                        "command_latency_ms": None},
            # No envelope: nothing rated, nothing invented. A delivery robot's limits are
            # map speed and battery, neither of which Pudu publishes per call — and the
            # envelope is not where a guess goes.
            "envelope": None, "capabilities": [],
            # §84.1's motion list: the four task verbs, each with the parameter the
            # controller refuses the motion without. No `motion` slot — deliver is not a
            # move_linear and claiming a canonical shape would be the divergent kind of
            # truth. No units or bounds either: map points are names, not quantities.
            "motions": [
                {"name": "deliver", "parameters": [{"name": "points", "required": True}]},
                {"name": "cruise", "parameters": [{"name": "line", "required": True}]},
                {"name": "call", "parameters": [{"name": "point", "required": True}]},
                {"name": "cancel_call", "parameters": [{"name": "id", "required": True}]},
            ],
            "extensions": [TOKEN], "solvers": [],
        })

    def read_sensor(self, name: str, out) -> int:
        _ = (name, out)
        return rhal.SKR_ENOSYS
"""skynet-pudu's own suite: the signing, the motions, the refusals, the host path.

Two halves. The first drives `PuduDriver` directly against `fake_cloud` on a thread — real HTTP
over loopback, so the bytes the driver sends are the bytes the fake verifies. The second speaks
the `python-host` pipe protocol to `python -m skynet_pudu` as a subprocess — open, describe,
read_state, a `T` motion, close — which is the driver's half of the `T` verb end to end (the
host's half is `check-extension`'s). `run-check.sh` adds the third: the vendor's own
`pudu-request.py` against the fake, and `driver check` through the real ABI.
"""

from __future__ import annotations

import base64
import json
import math
import os
import subprocess
import sys
import threading

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..",
                               "bindings", "python"))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import skynet_rhal as rhal
from skynet_pudu import PuduDriver
from skynet_pudu.fake_cloud import TEST_KEY, TEST_SECRET, serve

CHECKS = 0
FAILURES: list[str] = []


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    print(("  ok    " if held else "  FAIL  ") + what, flush=True)
    if not held:
        FAILURES.append(what)


def section(title: str) -> None:
    print(f"\n{title}", flush=True)


def config(server_port: int, **overrides) -> str:
    base = {"base_url": f"http://127.0.0.1:{server_port}", "sn": "SN-TEST-1",
            "app_key": TEST_KEY, "app_secret": TEST_SECRET}
    base.update(overrides)
    return json.dumps(base)


def main() -> int:
    server, fake = serve(0)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    port = server.server_port

    section("open: fail fast on config, probe the cloud")
    driver = PuduDriver()
    check(driver.open(config(port)) == rhal.SKR_OK, "open() with credentials probes the cloud")
    check(driver.open(config(port)) == rhal.SKR_EPERM, "a second open() is refused")
    check(PuduDriver().open(config(port, sn="")) == rhal.SKR_EINVAL,
          "no serial number is refused: no call can be made without one")
    check(PuduDriver().open(config(port, app_secret="")) == rhal.SKR_EINVAL,
          "no secret is refused: every call would fail auth")
    check(PuduDriver().open(config(port, base_url="http://127.0.0.1:1")) == rhal.SKR_EIO,
          "an unreachable cloud fails the probe, not a later call")
    check(PuduDriver().open(config(port, app_secret="wrong")) == rhal.SKR_EIO,
          "wrong credentials fail the probe — the fake answers 401")
    check(PuduDriver().open("not json") == rhal.SKR_EINVAL, "malformed config is refused")
    check(PuduDriver().open(config(port, sn="14:80:CC:89:27:A6")) == rhal.SKR_EINVAL,
          "a MAC in the sn field is refused: a serial is not a network address")

    section("the body, and what it refuses to declare")
    descriptor = json.loads(driver.describe())
    check(descriptor["robot_slug"] == "pudu-SN-TEST-1", "the slug names the bound robot")
    check(descriptor["dof"] == 0 and descriptor["joints"] == [],
          "zero joints: a service robot has none the OS can see")
    check(descriptor["base"] == "wheeled", "the base is wheeled")
    check(descriptor["control"]["modes"] == [], "and no joint control mode is claimed")
    check(descriptor["extensions"] == ["vendor.pudu.task"],
          "the task hatch is listed under extensions")
    motions = {m["name"]: [p["name"] for p in m["parameters"]]
               for m in descriptor["motions"]}
    check(motions == {"deliver": ["points"], "cruise": ["line"], "call": ["point"],
                      "cancel_call": ["id"]},
          "the §84.1 motion list names the four task verbs and their parameters")
    check(descriptor["envelope"] is None, "no envelope: nothing rated, nothing invented")
    check(descriptor["capabilities"] == [],
          "no estop capability: the latch cannot stop the cloud's task")
    check(descriptor["simulator"] is True, "declared simulated by default (§35.3)")
    real = PuduDriver()
    check(real.open(config(port, simulator=False)) == rhal.SKR_OK, "a declared-real session opens")
    check(json.loads(real.describe())["simulator"] is False,
          "and declares real when the operator says so")
    real.close()

    section("read_state: pose from the cloud, nothing else")
    state = rhal.State()
    check(driver.read_state(state, 0) == rhal.SKR_OK, "read_state succeeds")
    check(state.joint_count == 0, "with zero joints written")
    first_seq = state.seq
    check(state.base_pose[0] == 1.5 and state.base_pose[1] == 2.5
          and state.base_pose[2] == 0.0,
          "the map position reaches the base pose")
    check(all(math.isnan(v) for v in state.base_pose[3:]),
          "and the orientation is NaN — the cloud publishes no yaw, and zeros would be a claim")
    check(state.fault_code == 0, "the fault word stays 0: no fault vocabulary is documented")
    state2 = rhal.State()
    driver.read_state(state2, 0)
    check(state2.seq > first_seq, "the sequence advances between reads")
    check(driver.read_state(rhal.State(), 0) == rhal.SKR_OK, "reads keep succeeding")

    section("extension: the only route that moves it")
    rc, reply = driver.extension("vendor.pudu.task", b"deliver points=A01,B02")
    check(rc == rhal.SKR_OK, "deliver dispatches")
    answer = json.loads(reply.decode("utf-8"))
    check(answer["taskId"].startswith("task-") and answer["taskStatus"] == "RUNNING",
          "and the cloud's task id comes back in the reply")
    check(driver.extension("vendor.pudu.task", b"cruise line=Loop-A")[0] == rhal.SKR_OK,
          "cruise dispatches by line name")
    check(driver.extension("vendor.pudu.task", "deliver points=吧台,A01".encode("utf-8"))[0]
          == rhal.SKR_OK,
          "non-ASCII points dispatch: the body serialises unescaped, as the vendor signs")
    rc, reply = driver.extension("vendor.pudu.task", b"call point=A01")
    check(rc == rhal.SKR_OK, "call dispatches by map point")
    call_id = json.loads(reply.decode("utf-8"))["taskId"]
    check(driver.extension("vendor.pudu.task", f"cancel_call id={call_id}".encode())[0]
          == rhal.SKR_OK, "a call cancels by task id")
    check(driver.extension("vendor.pudu.other", b"deliver points=A01")[0] == rhal.SKR_ENOSYS,
          "any other token is absent, not refused")
    check(driver.extension("vendor.pudu.task", b"deliver")[0] == rhal.SKR_EINVAL,
          "a motion without its points is malformed")
    check(driver.extension("vendor.pudu.task", b"teleport points=A01")[0] == rhal.SKR_EINVAL,
          "an unknown motion is malformed, with the list beside it")
    check(driver.extension("vendor.pudu.task", b"")[0] == rhal.SKR_EINVAL,
          "an empty request is malformed, not a default motion")
    before = dict(fake.counts())
    check(driver.extension("vendor.pudu.task", b"deliver points=\xff\xfe")[0]
          == rhal.SKR_EINVAL, "non-text bytes are malformed before they reach the cloud")
    check(fake.counts()["requests"] == before["requests"],
          "and malformed input never becomes an HTTP request")

    section("write_command refuses everything (§4's shape)")
    cmd = rhal.Command(stamp_ns=0, seq=1, mode=rhal.MODE_POSITION, values=[], mask=[])
    check(driver.write_command(cmd, 0) == rhal.SKR_EPERM,
          "a joint command to a jointless body is refused")
    cmd = rhal.Command(stamp_ns=0, seq=1, mode=rhal.MODE_TORQUE, values=[], mask=[])
    check(driver.write_command(cmd, 0) == rhal.SKR_EPERM, "in every mode")
    cmd = rhal.Command(stamp_ns=0, seq=1, mode=rhal.MODE_POSITION, values=[1.0], mask=[1])
    check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL,
          "but a count this body does not have is malformed, not merely refused")
    cmd = rhal.Command(stamp_ns=0, seq=1, mode=200, values=[], mask=[])
    check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL,
          "and a mode the ABI does not define is malformed too — the shape before the "
          "permission, the way the pendant checks it")

    section("emergency_stop: a latch, honestly limited")
    check(driver.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    check(driver.emergency_stop() == rhal.SKR_OK, "and latches: twice is the same as once")
    rc, reply = driver.extension("vendor.pudu.task", b"deliver points=A01")
    check(rc == rhal.SKR_EPERM, "motions are refused while the latch is set")
    check(b"cannot recall" in reply, "with the limitation in words, not just a code")
    stopped = rhal.State()
    driver.read_state(stopped, 0)
    check(stopped.flags & rhal.FLAG_ESTOP != 0, "and the stop is visible in the next state")
    driver.close()
    reopened = PuduDriver()
    check(reopened.open(config(port)) == rhal.SKR_OK, "a new session opens clean")
    check(reopened.extension("vendor.pudu.task", b"deliver points=A01")[0] == rhal.SKR_OK,
          "and the latch does not carry into it")
    reopened.close()

    seeded_gaps(fake, port)

    section("what the fake cloud counted")
    counts = fake.counts()
    print(f"  {counts}", flush=True)
    check(counts["requests"] > 0, "the driver spoke HTTP for every one of these")
    # The wrong-secret probe above is MEANT to fail auth; anything else failing is a defect.
    check(counts["bad_auth"] == 1, "exactly one 401: the wrong-secret probe, nothing else")
    check(counts["unknown_endpoint"] == 0, "every path the driver sent exists")
    check(counts["malformed"] == 0, "every body the driver sent parsed and carried its fields")

    pipe_checks(port)
    server.shutdown()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed", flush=True)
    for failure in FAILURES:
        print(f"  - {failure}", flush=True)
    return 0 if not FAILURES else 1


def seeded_gaps(fake, port: int) -> None:
    """What `tools/seed-defects` found this suite could not see (DRY-COVERAGE-PLAN.md 12.C.3): each
    block is a defect of a kind that has happened to a driver here, seeded into this one and let
    through by the checks above. Every block was shown failing against its defect before it was
    kept."""
    import hashlib
    import hmac
    import re
    import urllib.parse

    from skynet_pudu import bridge as pudu

    section("seeded gaps: the signature, computed a second way from the scheme's own words")
    saved = pudu._gmt_date
    pudu._gmt_date = lambda: "Tue, 15 Nov 1994 08:12:31 GMT"
    try:
        for method, path, query, body in (
            ("POST", "/open-platform-service/v1/delivery_task", "", b'{"sn":"S1"}'),
            ("GET", "/open-platform-service/v1/robot/get_position", "sn=S%201%26x", b""),
            ("GET", "/release/open-platform-service/v2/status/get_by_sn", "sn=S1", b""),
            ("GET", "/prod/open-platform-service/v2/status/get_by_sn", "sn=S1", b""),
            ("POST", "/test/open-platform-service/v1/cruise_task", "", b'{"sn":"S1"}'),
        ):
            date, md5, auth = pudu.sign_request(method, path, query, body, "the-key", "the-secret")
            signed_path = re.sub(r"^/(release|test|prepub)(?=/)", "", path)
            if query:
                signed_path += "?" + urllib.parse.unquote(query)
            md5_expected = (
                base64.b64encode(hashlib.md5(body).hexdigest().encode()).decode()
                if method == "POST" else ""
            )
            signing = "\n".join(["x-date: Tue, 15 Nov 1994 08:12:31 GMT", method,
                                 "application/json", "application/json", md5_expected, signed_path])
            digest = hmac.new(b"the-secret", signing.encode(), hashlib.sha1).digest()
            want = (f'hmac id="the-key", algorithm="hmac-sha1", headers="x-date", '
                    f'signature="{base64.b64encode(digest).decode()}"')
            check(date == "Tue, 15 Nov 1994 08:12:31 GMT" and md5 == md5_expected and auth == want,
                  f"{method} {path}{'?' + query if query else ''} signs as the scheme says: "
                  f"the hex digest's base64, x-date, the decoded query, the environment prefix "
                  f"stripped and /prod kept")
    finally:
        pudu._gmt_date = saved
    check(re.fullmatch(r"(Mon|Tue|Wed|Thu|Fri|Sat|Sun), \d\d (Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) \d{4} \d\d:\d\d:\d\d GMT", pudu._gmt_date()) is not None,
          "and the date it signs is an HTTP date (toUTCString's spelling), not ISO 8601")

    section("seeded gaps: every environment, and a serial number that needs escaping")
    for prefix in ("/release", "/test", "/prepub", "/prod"):
        bad = fake.counts()["bad_auth"]
        env = PuduDriver()
        check(env.open(config(port, base_url=f"http://127.0.0.1:{port}{prefix}")) == rhal.SKR_OK
              and fake.counts()["bad_auth"] == bad,
              f"a base URL under {prefix} reaches the cloud and signs: "
              f"{'the prefix is part of the signature' if prefix == '/prod' else 'the prefix is not'}")
        env.close()
    awkward = PuduDriver()
    bad = fake.counts()["bad_auth"]
    check(awkward.open(config(port, sn="SN 1&x=吧")) == rhal.SKR_OK
          and fake.counts()["bad_auth"] == bad,
          "a serial number with a space, an ampersand and a CJK character signs over its decoded "
          "form: what the cloud verifies is the query it parsed, not the one that was sent")
    awkward.close()

    section("seeded gaps: what reaches the cloud, and what comes back")
    driver = PuduDriver()
    driver.open(config(port))
    driver.extension("vendor.pudu.task", b"deliver points=A01,B02,C03")
    trays = fake.last_post["payload"]["trays"]
    check([d["points"] for t in trays for d in t["destinations"]] == ["A01", "B02", "C03"],
          "a delivery to three points sends all three, in order")
    driver.extension("vendor.pudu.task", b"cruise line=Loop-A")
    check(fake.last_post["payload"]["mapCruiseName"] == "Loop-A", "a cruise names its line")
    rc, reply = driver.extension("vendor.pudu.task", b"call point=A01")
    task_id = json.loads(reply)["taskId"]
    driver.extension("vendor.pudu.task", f"cancel_call id={task_id}".encode())
    check(fake.last_post["taskId"] == task_id and fake.last_post["sn"] == "SN-TEST-1",
          "a cancel names the task it cancels and the robot it is for, not the robot twice")
    rc, reply = driver.extension("vendor.pudu.task", b"cancel_call id=task-9999")
    check(rc == rhal.SKR_EIO and b"refused" in reply,
          "a cancel the cloud refuses (no such task) is EIO with its words, never success")
    fake.omit_z = True
    state = rhal.State()
    driver.read_state(state, 0)
    check(state.base_pose[2] == 0.0 and state.base_pose[0] == 1.5,
          "a position with no z is at height zero, not one metre: the cloud publishes what it publishes")
    fake.omit_z = False
    fake.fail_reads = True
    check(driver.read_state(rhal.State(), 0) == rhal.SKR_EIO,
          "a state read the cloud answers with HTTP 500 is EIO, not a success over stale output")
    fake.fail_reads = False

    section("seeded gaps: release_stop, and the latch it clears")
    driver.emergency_stop()
    check(driver.extension("vendor.pudu.task", b"deliver points=A01")[0] == rhal.SKR_EPERM,
          "a stopped driver refuses motions")
    check(driver.release_stop() == rhal.SKR_OK, "release_stop clears the latch")
    check(driver.extension("vendor.pudu.task", b"deliver points=A01")[0] == rhal.SKR_OK,
          "and motions are accepted again: §48.2's power-on self-test releases a stop it asserted")
    stopped = rhal.State()
    driver.read_state(stopped, 0)
    check(stopped.flags & rhal.FLAG_ESTOP == 0, "and the state no longer says stopped")
    driver.close()


def pipe_checks(port: int) -> None:
    """The driver's half of `T`, through the real pipe protocol as a subprocess."""
    section("through the pipe: serve() carries T to this driver")
    root = os.path.dirname(os.path.abspath(__file__))
    env = dict(os.environ)
    env["PYTHONPATH"] = (os.path.join(root, "..", "..", "bindings", "python")
                         + os.pathsep + root)
    child = subprocess.Popen(
        [sys.executable, "-m", "skynet_pudu"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL, text=True, bufsize=1, cwd=root, env=env)

    def send(line: str) -> None:
        assert child.stdin is not None
        child.stdin.write(line + "\n")
        child.stdin.flush()

    def recv() -> str:
        assert child.stdout is not None
        line = child.stdout.readline()
        if not line:
            raise RuntimeError("the child went quiet")
        return line.rstrip("\n")

    try:
        hello = recv()
        # The ABI and the host protocol are the binding's to say, not this file's to restate: this line read
        # `H 1 2` until the ABI went to 2 and failed unnoticed, because nothing ran this driver on push (F183).
        check(hello.startswith(f"H {rhal.ABI_VERSION} {rhal._PROTOCOL_VERSION} guest skynet-pudu "),
              f"the child announces itself: {hello}")
        cfg = config(port)
        child.stdin.write(f"O {len(cfg)}\n{cfg}\n")
        child.stdin.flush()
        check(recv() == "R 0", "open() through the pipe probes the cloud")
        send("D")
        headline = recv()
        check(headline.startswith("D "), "describe() answers with a length")
        body = child.stdout.read(int(headline.split()[1]))
        child.stdout.readline()
        check('"vendor.pudu.task"' in body, "and the descriptor lists the task hatch")
        send("S 0")
        state = recv()
        check(state.startswith("S 0 "), "read_state answers through the pipe")
        send("C 0 1 0 0")
        check(recv() == f"R {rhal.SKR_EPERM}", "write_command refuses through the pipe")
        payload = base64.b64encode(b"deliver points=A01").decode("ascii")
        send(f"T vendor.pudu.task {payload}")
        answer = recv()
        parts = answer.split()
        check(len(parts) == 3 and parts[0] == "R" and parts[1] == "0",
              "a T motion answers in-frame with rc 0")
        reply = base64.b64decode(recv().encode("ascii")).decode("utf-8")
        check("taskId" in reply, "and the cloud's task id comes back over the pipe")
        send("T vendor.pudu.task")
        answer = recv()
        check(answer.split()[1] == str(rhal.SKR_EINVAL),
              "an empty T payload is malformed through the pipe")
        recv()  # the empty second line stays in frame
        send("X")
        check(recv() == "R 0", "close() stops the child")
    finally:
        child.stdin.close()
        child.wait(timeout=10)


if __name__ == "__main__":
    raise SystemExit(main())

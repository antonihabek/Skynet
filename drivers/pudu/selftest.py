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
        check(hello.startswith("H 1 2 guest skynet-pudu "),
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

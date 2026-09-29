"""skynet-fourier's own suite: the codec, the joints, the refusals, the pipe path.

Three halves. The first pins `wscodec` to RFC 6455's own worked examples — the
handshake accept pair and a masked frame's bytes — because a codec that agrees
with itself proves nothing. The second drives `FourierDriver` directly against
`fake_rocs` on a thread: real HTTP and real WebSocket over loopback, so the bytes
the driver sends are the bytes the fake verifies. The third speaks the
`python-host` pipe protocol to `python -m skynet_fourier` as a subprocess — open,
describe, read_state, a command, close. `run-check.sh` adds the fourth: the
vendor's own `rocs_client` against the fake, and `driver check` through the real
ABI.
"""

from __future__ import annotations

import base64
import json
import math
import os
import socket
import subprocess
import sys
import threading
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..",
                               "bindings", "python"))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import skynet_rhal as rhal
from skynet_fourier import FourierDriver
from skynet_fourier.fake_rocs import MOTORS, serve
from skynet_fourier.wscodec import WsError, accept_key, recv_text, send_text

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
    base = {"host": "127.0.0.1", "port": server_port}
    base.update(overrides)
    return json.dumps(base)


def wait_moves(fake, count: int, timeout_s: float = 30.0) -> bool:
    """Wait for the fake's WS thread to apply `count` more moves. `write_command`
    returns when the bytes are on the wire, not when the peer acts on them —
    asserting immediately would be asserting about thread scheduling. Delivery
    itself is milliseconds (30/30 under 41 ms, measured); the deadline only
    fires on a machine stall. Fifteen seconds fired twice on this box — both
    moves applied within milliseconds *after* the wait expired, the signature
    of a full-process descheduling with the monotonic clock still ticking —
    so the deadline is thirty, and firing on a stall would be failing the
    driver for the scheduler."""
    with fake.lock:
        start = len(fake.move_joints)
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        with fake.lock:
            if len(fake.move_joints) >= start + count:
                return True
        time.sleep(0.01)
    print(f"  (still waiting: {fake.counts()})", flush=True)
    return False


def codec_checks() -> None:
    section("the codec, against the RFC's own examples")
    # RFC 6455 §1.2's handshake pair, byte for byte.
    check(accept_key("dGhlIHNhbXBsZSBub25jZQ==") == "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=",
          "the §1.2 accept pair: the key hashes to the RFC's value")
    # RFC 6455 §1.3's masked "Hello", sent through a socketpair and read back.
    ours, theirs = socket.socketpair()
    try:
        ours.sendall(bytes([0x81, 0x85, 0x37, 0xFA, 0x21, 0x3D,
                            0x7F, 0x9F, 0x4D, 0x51, 0x58]))
        check(recv_text(theirs, client=False) == "Hello",
              "the §1.3 masked frame decodes to Hello")
    finally:
        ours.close()
        theirs.close()
    # A round trip both directions, then the refusals.
    a, b = socket.socketpair()
    try:
        send_text(a, "move it", client=True)
        check(recv_text(b, client=False) == "move it",
              "a masked client frame crosses to the server")
        send_text(b, "standing by", client=False)
        check(recv_text(a, client=True) == "standing by",
              "an unmasked server frame crosses to the client")
        b.sendall(bytes([0x81, 0x05]) + b"Hello")
        try:
            recv_text(a, client=False)
            masked_ok = False
        except WsError:
            masked_ok = True
        check(masked_ok, "an unmasked client frame is refused (§5.1)")
    finally:
        a.close()
        b.close()
    c, d = socket.socketpair()
    try:
        c.sendall(bytes([0x82, 0x83, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07]))
        try:
            recv_text(d, client=False)
            binary_ok = False
        except WsError:
            binary_ok = True
        check(binary_ok, "a binary frame is refused: this wire is JSON text")
    finally:
        c.close()
        d.close()


def main() -> int:
    codec_checks()
    server, fake = serve(0)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    port = server.server_address[1]

    section("open: fail fast on config, discover the joints live")
    driver = FourierDriver()
    check(driver.open(config(port)) == rhal.SKR_OK,
          "open() discovers six motors off limit/list")
    check(driver.open(config(port)) == rhal.SKR_EPERM, "a second open() is refused")
    check(FourierDriver().open("not json") == rhal.SKR_EINVAL,
          "malformed config is refused")
    check(FourierDriver().open(config(port, host="")) == rhal.SKR_EINVAL,
          "an empty host is refused")
    check(FourierDriver().open(config(port, port=99999)) == rhal.SKR_EINVAL,
          "a port outside 1-65535 is refused")
    check(FourierDriver().open(config(1)) == rhal.SKR_EIO,
          "an unreachable RoCS fails the probe, not a later call")
    aux = {m: f"aux.motor#{i}" for i, m in
           enumerate(["1-left", "1-right", "2-left", "2-right", "3-left", "3-right"])}
    probed = FourierDriver()
    check(probed.open(config(port, roles=aux)) == rhal.SKR_OK,
          "operator roles are accepted: aux, the honest slot for unmapped motors")
    probed.close()
    check(FourierDriver().open(config(port, roles={"1-left": "elbow"}))
          == rhal.SKR_EINVAL,
          "an unparseable role fails at open, not silently")
    check(FourierDriver().open(config(port, roles={"9-up": "aux.motor#0"}))
          == rhal.SKR_EINVAL,
          "a role for a motor the robot never reported fails at open too")

    section("the body, and what it refuses to declare")
    descriptor = json.loads(driver.describe())
    check(descriptor["robot_slug"] == "fourier-gr1", "the slug names the body")
    check(descriptor["dof"] == 6 and len(descriptor["joints"]) == 6,
          "six joints, discovered — not a list kept beside the code")
    names = [j["name"] for j in descriptor["joints"]]
    check(names == ["1-left", "1-right", "2-left", "2-right", "3-left", "3-right"],
          "joints are named by motor address, in the robot's order")
    first = descriptor["joints"][0]["limits"]
    check(math.isclose(first["position_max_rad"], math.radians(150.0)) and
          math.isclose(first["position_min_rad"], math.radians(-150.0)),
          "travel is the live limit/list range, in radians")
    check(all(set(j["limits"]) == {"position_min_rad", "position_max_rad"}
              for j in descriptor["joints"]),
          "no per-joint velocity or effort: unpublished, so undeclared")
    check(all("role" not in j for j in descriptor["joints"]),
          "no roles without an operator: never inferred (§63.4's spirit)")
    roled = FourierDriver()
    roled.open(config(port, roles=aux))
    carried = [j.get("role") for j in json.loads(roled.describe())["joints"]]
    check(carried == [f"aux.motor#{i}" for i in range(6)],
          "with an operator, the roles ride the descriptor")
    roled.close()
    check(all(j["control_modes"] == ["position"] for j in descriptor["joints"]),
          "position only, on every joint")
    check(descriptor["base"] == "legged", "the base is legged")
    check(descriptor["control"]["modes"] == ["position"],
          "and no other control mode is claimed")
    check(descriptor["control"]["rate_hz_max"] is None,
          "no control rate: the RoCS rate is unpublished")
    envelope = descriptor["envelope"]
    check(envelope["joint_torque_max_nm"] == 230.0,
          "the envelope carries the published 230 N·m peak")
    check("not a per-joint rating" in envelope["source"],
          "with the hedge beside the figure, in words")
    check("unpublished" in envelope["source"],
          "and the missing velocity beside it (joint_ratings unmet)")
    check("driver design" in envelope["source"] and envelope["watchdog_ms"] == 500.0,
          "the watchdog window is driver design, stated as such")
    check(descriptor["capabilities"] == [],
          "no estop capability: /robot/stop is software, not a circuit")

    section("read_state: positions measured, the rest NaN")
    state = rhal.State()
    check(driver.read_state(state, 0) == rhal.SKR_OK, "read_state answers")
    starts = [math.radians(m[4]) for m in MOTORS]
    check(all(math.isclose(p, s, abs_tol=1e-9)
              for p, s in zip(state.position_rad, starts)),
          "positions are the fake's starts, degrees to radians")
    check(all(math.isnan(v) for v in state.velocity_rad_s),
          "velocities are NaN: the PVC unit is unpublished")
    check(all(math.isnan(e) for e in state.effort_nm),
          "efforts are NaN: amps are not newton-metres")
    check(all(math.isnan(p) for p in state.base_pose),
          "base pose all NaN: unlocalised (§24)")
    check(state.seq == 1, "the first sample is seq 1")
    state2 = rhal.State()
    driver.read_state(state2, 0)
    check(state2.seq == 2 and state2.stamp_ns >= state.stamp_ns,
          "seq advances and the stamp does not go back")
    check(driver.read_state(rhal.State(), 0) == rhal.SKR_OK,
          "reads repeat without reopening")

    section("write_command: one move_joint, pre-clamped")
    before = dict(fake.positions)
    cmd = rhal.Command(stamp_ns=0, seq=1, mode=rhal.MODE_POSITION,
                       values=[0.1, -0.2, 0.3, -0.4, 0.5, -0.6],
                       mask=[1, 1, 1, 1, 1, 1])
    check(driver.write_command(cmd, 0) == rhal.SKR_OK,
          "six positions are accepted")
    check(wait_moves(fake, 6), "the move_joint lands on the fake")
    with fake.lock:
        applied = [fake.positions[(m[0], m[1])] for m in MOTORS]
    check(all(math.isclose(a, math.degrees(v), abs_tol=1e-9)
              for a, v in zip(applied, cmd.values)),
          "the fake's motors moved, radians to degrees")
    check(all(fake.positions[k] != before[k] for k in before),
          "...every one of them, none left at its start")
    with fake.lock:
        sent = list(fake.move_joints)
    check(len(sent) >= 6 and all(set(m) == {"no", "orientation", "angle"}
                                 for m in sent[-6:]),
          "the wire shape is the vendor's: no/orientation/angle")
    check(fake.counts()["clamped"] == 0,
          "nothing clamped: the values were inside every range")
    # Beyond the range: clamped to the live limit, the vendor client's rule.
    cmd = rhal.Command(stamp_ns=0, seq=2, mode=rhal.MODE_POSITION,
                       values=[10.0] * 6, mask=[1] * 6)
    check(driver.write_command(cmd, 0) == rhal.SKR_OK,
          "beyond-range values are clamped, the _cover_param rule")
    check(wait_moves(fake, 6), "the clamped move_joint lands too")
    with fake.lock:
        clamped = [fake.positions[(m[0], m[1])] for m in MOTORS]
    check(all(math.isclose(c, e, abs_tol=1e-9)
              for c, e in zip(clamped, [150.0, 150.0, 120.0, 120.0, 90.0, 90.0])),
          "clamped per motor to its own live max")
    check(fake.counts()["clamped"] == 0,
          "the driver pre-clamped: the fake never had to")
    # Masked joints hold: send nothing for them.
    with fake.lock:
        snapshot = dict(fake.positions)
    cmd = rhal.Command(stamp_ns=0, seq=3, mode=rhal.MODE_POSITION,
                       values=[0.0] * 6, mask=[0, 0, 1, 0, 0, 0])
    check(driver.write_command(cmd, 0) == rhal.SKR_OK, "a masked command holds")
    check(wait_moves(fake, 1), "one motor named, one motor moved")
    with fake.lock:
        after = dict(fake.positions)
        check(after[("2", "left")] == 0.0 and
              all(after[k] == snapshot[k] for k in snapshot
                  if k != ("2", "left")),
              "the named joint moved and the masked five did not")
    cmd = rhal.Command(stamp_ns=0, seq=4, mode=rhal.MODE_POSITION,
                       values=[0.0] * 6, mask=[0] * 6)
    with fake.lock:
        moves_before = len(fake.move_joints)
    check(driver.write_command(cmd, 0) == rhal.SKR_OK,
          "an all-masked command holds everything, sending nothing")
    time.sleep(0.2)
    with fake.lock:
        check(len(fake.move_joints) == moves_before,
              "...no move_joint at all")

    section("write_command: the refusals")
    cmd = rhal.Command(stamp_ns=0, seq=5, mode=rhal.MODE_POSITION,
                       values=[0.0] * 5, mask=[1] * 5)
    check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL,
          "a count this body does not have is malformed")
    cmd = rhal.Command(stamp_ns=0, seq=6, mode=200, values=[0.0] * 6,
                       mask=[1] * 6)
    check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL,
          "a mode the ABI does not define is malformed too")
    cmd = rhal.Command(stamp_ns=0, seq=7, mode=rhal.MODE_VELOCITY,
                       values=[0.0] * 6, mask=[1] * 6)
    check(driver.write_command(cmd, 0) == rhal.SKR_EPERM,
          "velocity is refused: RoCS publishes no joint-velocity command")
    cmd = rhal.Command(stamp_ns=0, seq=8, mode=rhal.MODE_TORQUE,
                       values=[0.0] * 6, mask=[1] * 6)
    check(driver.write_command(cmd, 0) == rhal.SKR_EPERM,
          "torque is refused: RoCS publishes no joint-torque command")
    cmd = rhal.Command(stamp_ns=0, seq=9, mode=rhal.MODE_POSITION,
                       values=[0.0] * 6, mask=[1] * 6, modes=[2] * 6)
    check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL,
          "a bare MODE_TORQUE in modes[] is malformed (§86.1), not torque")
    cmd = rhal.Command(stamp_ns=0, seq=10, mode=rhal.MODE_POSITION,
                       values=[0.0] * 6, mask=[1] * 6,
                       modes=[rhal.JMODE_POSITION] * 6)
    check(driver.write_command(cmd, 0) == rhal.SKR_OK,
          "spelled per-joint modes cross: JMODE_POSITION moves")
    check(wait_moves(fake, 6), "the per-joint move lands too")
    cmd = rhal.Command(stamp_ns=0, seq=11, mode=rhal.MODE_POSITION,
                       values=[0.0] * 6, mask=[1] * 6,
                       modes=[rhal.JMODE_TORQUE] + [rhal.JMODE_INHERIT] * 5)
    check(driver.write_command(cmd, 0) == rhal.SKR_EPERM,
          "one torque joint among inheriting ones refuses the frame")
    check(driver.read_sensor("imu", rhal.Frame()) == rhal.SKR_ENOSYS,
          "no sensors: ENOSYS, not an empty frame")
    rc, _ = driver.extension("vendor.anything", b"")
    check(rc == rhal.SKR_ENOSYS, "no extension: the joints are the interface")

    section("emergency_stop: the vendor's path, then the latch")
    check(driver.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    check(fake.counts()["stops"] == 1, "POST /robot/stop went out")
    check(driver.emergency_stop() == rhal.SKR_OK,
          "and latches: twice is the same as once")
    cmd = rhal.Command(stamp_ns=0, seq=9, mode=rhal.MODE_POSITION,
                       values=[0.0] * 6, mask=[1] * 6)
    check(driver.write_command(cmd, 0) == rhal.SKR_EPERM,
          "commands are refused while the latch is set")
    stopped = rhal.State()
    driver.read_state(stopped, 0)
    check(stopped.flags & rhal.FLAG_ESTOP != 0,
          "and the stop is visible in the next state")
    driver.close()
    check(driver.read_state(rhal.State(), 0) == rhal.SKR_EPERM,
          "closed reads are refused")
    reopened = FourierDriver()
    check(reopened.open(config(port)) == rhal.SKR_OK, "a new session opens clean")
    cmd = rhal.Command(stamp_ns=0, seq=1, mode=rhal.MODE_POSITION,
                       values=[0.0] * 6, mask=[1] * 6)
    check(reopened.write_command(cmd, 0) == rhal.SKR_OK,
          "and the latch does not carry into it")
    reopened.close()

    section("what the fake counted")
    counts = fake.counts()
    print(f"  {counts}", flush=True)
    check(counts["requests"] > 0, "the driver spoke HTTP for every one of these")
    check(counts["ws_commands"] > 0, "and WebSocket for the commands")
    check(counts["unknown_endpoint"] == 0, "every path the driver sent exists")
    check(counts["malformed"] == 0, "every message the driver sent parsed")
    check(counts["clamped"] == 0, "the fake never clamped: pre-clamped always")

    pipe_checks(port)
    server.shutdown()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed", flush=True)
    for failure in FAILURES:
        print(f"  - {failure}", flush=True)
    return 0 if not FAILURES else 1


def pipe_checks(port: int) -> None:
    """The driver's half of the pipe, through the real protocol as a subprocess."""
    section("through the pipe: serve() carries joints to this driver")
    root = os.path.dirname(os.path.abspath(__file__))
    env = dict(os.environ)
    env["PYTHONPATH"] = (os.path.join(root, "..", "..", "bindings", "python")
                         + os.pathsep + root)
    child = subprocess.Popen(
        [sys.executable, "-m", "skynet_fourier"], stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, bufsize=1,
        cwd=root, env=env)

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
        check(hello.startswith(f"H {rhal.ABI_VERSION} {rhal._PROTOCOL_VERSION} guest skynet-fourier "),
              f"the child announces itself: {hello}")
        cfg = config(port)
        child.stdin.write(f"O {len(cfg)}\n{cfg}\n")
        child.stdin.flush()
        check(recv() == "R 0", "open() through the pipe discovers the joints")
        send("D")
        headline = recv()
        check(headline.startswith("D "), "describe() answers with a length")
        body = child.stdout.read(int(headline.split()[1]))
        child.stdout.readline()
        check('"fourier-gr1"' in body and '"1-left"' in body,
              "and the descriptor names the body and its motors")
        send("S 0")
        state = recv().split()
        check(len(state) == 7 + 3 * 6 and state[0] == "S" and state[1] == "0"
              and state[6] == "6",
              "read_state answers six joints through the pipe")
        check(all(v == "nan" for v in state[13:19]),
              "velocities cross the pipe as NaN, not zero")
        send("C 0 1 0 6 0.1 0.2 0.3 0.4 0.5 0.6 1 1 1 1 1 1")
        check(recv() == "R 0", "write_command moves through the pipe")
        send("C 0 2 0 6 0 0 0 0 0 0 1 1 1 1 1 1 16 16 16 16 16 16")
        check(recv() == "R 0", "per-joint modes ride the C line (0x10 moves)")
        send("C 0 3 0 6 0 0 0 0 0 0 1 1 1 1 1 1 2 2 2 2 2 2")
        check(recv() == f"R {rhal.SKR_EINVAL}",
              "a bare MODE_TORQUE in the C line refuses through the pipe")
        send("C 0 4 0 6 0 0 0 0 0 0 1 1 1 1 1 1 16 16")
        check(recv() == f"R {rhal.SKR_EPROTO}",
              "a short modes array is a framing error, not a short array")
        send("E")
        check(recv() == "R 0", "emergency_stop answers through the pipe")
        send("C 0 2 0 6 0 0 0 0 0 0 1 1 1 1 1 1")
        check(recv() == f"R {rhal.SKR_EPERM}",
              "commands refuse after it, through the pipe too")
        send("X")
        check(recv() == "R 0", "close() stops the child")
    finally:
        child.stdin.close()
        child.wait(timeout=10)


if __name__ == "__main__":
    raise SystemExit(main())

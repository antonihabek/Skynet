"""skynet-spot's own suite: bring-up, the joints, the motions, the refusals, the pipe.

Two halves. The first drives `SpotDriver` directly against `fake_spot` in-process — real TLS
and real gRPC over loopback, so the bytes the driver sends are the bytes the fake verifies.
The second speaks the `python-host` pipe protocol to `python -m skynet_spot` as a subprocess.
`run-check.sh` adds the third and fourth: the vendor's own `bosdyn-client` against the fake,
and `driver check` through the real ABI.

Needs the project environment (`.venv` at the repo root): the wire is gRPC and TLS, and no
stdlib speaks either. Without it the suite refuses with the install line rather than running
half of itself.
"""

from __future__ import annotations

import base64
import json
import math
import os
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "bindings", "python"))
sys.path.insert(0, HERE)

try:
    import grpc  # noqa: F401
    import bosdyn.client  # noqa: F401
except ImportError:
    print("::error::skynet-spot's suite needs the project environment: bosdyn-client and "
          "grpcio are not importable. See drivers/spot/README.md.", flush=True)
    sys.exit(1)

import skynet_rhal as rhal
from skynet_spot import JOINTS, ROLES, TOKEN, SpotDriver
from skynet_spot.fake_spot import SIT, STAND, SpotWorld, serve

CERTS = os.path.join(HERE, "certs")

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
    base = {"host": "127.0.0.1", "port": server_port, "username": "admin",
            "password": "password", "ca": f"{CERTS}/ca.pem"}
    base.update(overrides)
    return json.dumps(base)


def motion(driver: SpotDriver, text: str):
    return driver.extension(TOKEN, text.encode("utf-8"))


def main() -> int:
    server, world, port = serve(0)

    try:
        section("open: fail fast on config, bring the robot up")
        driver = SpotDriver()
        check(driver.open(config(port)) == rhal.SKR_OK,
              "open() authenticates, syncs, and brings up estop, lease and time sync")
        check(driver.open(config(port)) == rhal.SKR_EPERM, "a second open() is refused")
        check(SpotDriver().open(config(port, host="")) == rhal.SKR_EINVAL,
              "no host is refused")
        check(SpotDriver().open(config(port, password="")) == rhal.SKR_EINVAL,
              "no password is refused: every call but the token mint carries the token")
        check(SpotDriver().open(config(port, port=0)) == rhal.SKR_EINVAL,
              "a bad port is refused")
        check(SpotDriver().open(config(port, estop_timeout=0)) == rhal.SKR_EINVAL,
              "a non-positive estop timeout is refused")
        check(SpotDriver().open("not json") == rhal.SKR_EINVAL,
              "malformed config is refused")
        check(SpotDriver().open(config(1)) == rhal.SKR_EIO,
              "an unreachable robot fails bring-up, not a later call")

        section("the body, and what it refuses to declare")
        descriptor = json.loads(driver.describe())
        check(descriptor["robot_slug"] == "spot", "the slug is the robot")
        check(descriptor["dof"] == 12, "twelve joints")
        check([j["name"] for j in descriptor["joints"]] == list(JOINTS),
              "named the vendor's enumeration")
        check([j["role"] for j in descriptor["joints"]] == list(ROLES),
              "roled in the Go2's words, front #0 and hind #1")
        check(all(set(j) == {"name", "role", "type"} for j in descriptor["joints"]),
              "with no travel: Boston Dynamics publishes no joint-limit table (§63.4)")
        check(descriptor["base"] == "quadruped", "the base is quadruped")
        check(descriptor["control"]["modes"] == [],
              "and no joint control mode is claimed (§59.2's genuine class)")
        check(descriptor["envelope"] is None, "and no envelope: nothing rated")
        check(descriptor["capabilities"] == ["safety.estop"],
              "but the CUT it has — through the vendor's own estop — is declared")
        check([m["name"] for m in descriptor["motions"]] ==
              ["power_on", "power_off", "stand", "sit", "stop", "selfright"],
              "six task motions, vendor defaults taking no arguments")
        check(descriptor["extensions"] == [TOKEN], "under the one vendor token")

        section("read_state: twelve joints by name, pose, faults")
        out = rhal.State()
        check(driver.read_state(out, 0) == rhal.SKR_OK, "the first read answers")
        check(len(out.position_rad) == 12 and all(math.isfinite(p)
              for p in out.position_rad), "twelve finite positions")
        check(all(math.isfinite(v) for v in out.velocity_rad_s),
              "twelve finite velocities")
        check(all(math.isnan(e) for e in out.effort_nm),
              "effort all NaN: load has no documented unit (§24)")
        check(len(out.base_pose) == 7 and all(math.isfinite(p) for p in out.base_pose),
              "a vision-frame body pose")
        check(out.base_pose[2] == 0.35, "sitting height, the fake's script")
        check(out.base_twist == [0.0] * 6, "and a zero twist standing still")
        check(out.fault_code == 0 and out.flags == 0, "no faults, no flags")
        first_seq, first_stamp = out.seq, out.stamp_ns
        check(driver.read_state(out, 0) == rhal.SKR_OK and out.seq == first_seq + 1,
              "the sequence advances")
        check(out.stamp_ns >= first_stamp and out.stamp_ns > 0,
              "and the acquisition stamp with it")

        section("the mapping is by name, not by position")
        world.shuffle_joints()
        check(driver.read_state(out, 0) == rhal.SKR_OK, "the fake serves reversed")
        check(out.position_rad[0] == SIT["hx"] and out.position_rad[2] == SIT["kn"],
              "and the driver still reads fl.hx first, fl.kn third")
        world.shuffle_joints()

        section("write_command: shaped, then refused")
        cmd = rhal.Command(mode=rhal.MODE_POSITION, values=[0.0] * 12,
                           mask=[1] * 12)
        check(driver.write_command(cmd, 0) == rhal.SKR_EPERM,
              "a well-formed joint command is refused: command-level API (§59.2)")
        cmd.values = []
        check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL,
              "a shapeless one is EINVAL first, the pendant's order")
        cmd.values = [0.0] * 12
        cmd.mode = 99
        check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL,
              "as is an unknown mode")
        cmd.mode = rhal.MODE_POSITION
        cmd.modes = [0] * 3
        check(driver.write_command(cmd, 0) == rhal.SKR_EINVAL,
              "as is a per-joint array of the wrong length")

        section("extension: shape, power discipline, motions that move")
        rc, _ = driver.extension("vendor.nope", b"stand")
        check(rc == rhal.SKR_ENOSYS, "an unknown token is not this driver's")
        rc, _ = motion(driver, "")
        check(rc == rhal.SKR_EINVAL, "an empty request names no motion")
        rc, _ = motion(driver, "dance")
        check(rc == rhal.SKR_EINVAL, "an unknown motion is refused")
        rc, _ = motion(driver, "stand height=1")
        check(rc == rhal.SKR_EINVAL, "motions take no arguments")
        rc, reply = motion(driver, "stand")
        check(rc == rhal.SKR_EPERM and b"power_on" in reply,
              "stand while unpowered is refused with the remedy")
        rc, reply = motion(driver, "power_on")
        check(rc == rhal.SKR_OK and json.loads(reply)["powered"] is True,
              "power_on answers powered")
        rc, reply = motion(driver, "stand")
        check(rc == rhal.SKR_OK and "command_id" in json.loads(reply),
              "stand answers a command id")
        driver.read_state(out, 0)
        check(out.position_rad[1] == STAND["hy"] and out.position_rad[2] == STAND["kn"]
              and out.base_pose[2] == 0.55,
              "and the joints stand, hips and knees with the body height")
        rc, _ = motion(driver, "sit")
        driver.read_state(out, 0)
        check(rc == rhal.SKR_OK and out.position_rad[2] == SIT["kn"],
              "sit sits")
        rc, _ = motion(driver, "stop")
        check(rc == rhal.SKR_OK, "stop stops")
        rc, _ = motion(driver, "selfright")
        driver.read_state(out, 0)
        check(rc == rhal.SKR_OK and out.position_rad[2] == STAND["kn"],
              "selfright rights to stand")
        rc, _ = motion(driver, "power_off")
        check(rc == rhal.SKR_OK, "power_off answers")
        rc, _ = motion(driver, "stand")
        check(rc == rhal.SKR_EPERM, "and stand is refused again unpowered")

        section("faults: reported, counted, never cleared here")
        world.inject_fault("harness-warning")
        driver.read_state(out, 0)
        check(out.fault_code == 1 and out.flags & rhal.FLAG_FAULT,
              "one injected fault reads one with the flag")
        world.inject_fault("harness-critical", "SEVERITY_CRITICAL")
        driver.read_state(out, 0)
        check(out.fault_code == 2, "two read two")
        world.clear_faults()
        driver.read_state(out, 0)
        check(out.fault_code == 0 and not (out.flags & rhal.FLAG_FAULT),
              "cleared faults read clear")

        section("emergency_stop: CUT, latch, recovery in a new instance")
        check(driver.emergency_stop() == rhal.SKR_OK, "the stop latches")
        check(world.estopped, "the CUT went out")
        rc, reply = motion(driver, "stand")
        check(rc == rhal.SKR_EPERM and b"latched" in reply,
              "later motions are refused with the word")
        check(driver.read_state(out, 0) == rhal.SKR_OK
              and (out.flags & rhal.FLAG_ESTOP) != 0,
              "later reads report the stop in the flags, not a refusal")
        check(driver.close() == rhal.SKR_OK, "close after stop is quiet")
        risen = SpotDriver()
        check(risen.open(config(port)) == rhal.SKR_OK, "a new instance recovers")
        rc, _ = motion(risen, "power_on")
        check(rc == rhal.SKR_OK, "and powers on")
        rc, _ = motion(risen, "stand")
        check(rc == rhal.SKR_OK, "and stands")
        rc, _ = motion(risen, "power_off")
        check(rc == rhal.SKR_OK, "and powers off before closing, so its endpoint "
              "deregisters instead of dying silent")
        risen.close()

        section("the deadman: a silent endpoint stops the machine")
        # The lease is free now (every driver above returned it), so a short-deadman
        # driver opens, powers on, and closes dirty: keepalives stopped, endpoint
        # registered and silent, motors on.
        quick = SpotDriver()
        check(quick.open(config(port, estop_timeout=0.5)) == rhal.SKR_OK,
              "a short-deadman driver opens")
        motion(quick, "power_on")
        quick.close()
        time.sleep(2.0)
        check(world.estopped and not world.powered,
              "its abandoned endpoint times out, estops, and cuts power")
        # The reopen races nothing: the robot is safe (motors off), so estop
        # reconfigures and the keepalive releases the stop.
        recovery = SpotDriver()
        check(recovery.open(config(port)) == rhal.SKR_OK,
              "a fresh driver reopens on the safed robot")
        rc, _ = motion(recovery, "power_on")
        check(rc == rhal.SKR_OK, "powers it, releasing the stop")
        recovery.read_state(out, 0)
        check(not (out.flags & rhal.FLAG_ESTOP) and out.fault_code == 0,
              "and reads no flags on the recovered robot")
        motion(recovery, "power_off")
        recovery.close()
    finally:
        server.stop(grace=2).wait()
        world.halt()

    section("the pipe half: python-host's verbs against a subprocess")
    server, world, port = serve(0)
    try:
        env = dict(os.environ)
        env["PYTHONPATH"] = (os.path.join(HERE, "..", "..", "bindings", "python") +
                             os.pathsep + HERE + os.pathsep +
                             env.get("PYTHONPATH", ""))
        child = subprocess.Popen(
            [sys.executable, "-m", "skynet_spot"], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1,
            env=env, cwd=HERE)

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

        hello = recv()
        check(hello.startswith("H ") and "skynet-spot" in hello,
              f"the child announces itself: {hello}")
        body = config(port)
        assert child.stdin is not None
        child.stdin.write(f"O {len(body)}\n{body}\n")
        child.stdin.flush()
        check(recv() == "R 0", "O opens through the pipe")
        send("D")
        answer = recv()
        check(answer.startswith("D "), "D describes")
        described = child.stdout.read(int(answer.split()[1]))
        child.stdout.readline()
        check(json.loads(described)["dof"] == 12, "twelve joints over the pipe")
        send("S 0")
        fields = recv().split()
        check(fields[0] == "S" and fields[1] == "0" and fields[6] == "12",
              "S reads twelve joints")
        stand = base64.b64encode(b"stand").decode("ascii")
        send(f"T {TOKEN} {stand}")
        answer = recv()
        check(answer.startswith("R "), "T motions")
        parts = answer.split()
        if len(parts) == 3:
            reply = base64.b64decode(recv().encode("ascii")).decode("utf-8")
            check(parts[1] == "-1" and "power_on" in reply,
                  "stand unpowered is EPERM with the remedy, over the pipe")
        else:
            check(False, "the T reply carries its payload length")
        power = base64.b64encode(b"power_on").decode("ascii")
        send(f"T {TOKEN} {power}")
        parts = recv().split()
        if len(parts) == 3:
            recv()
            check(parts[1] == "0", "power_on answers OK over the pipe")
        else:
            check(False, "the power reply carries its payload length")
        child.stdin.write("X\n")
        child.stdin.flush()
        child.wait(timeout=15)
        check(child.returncode == 0, "X closes the child cleanly")
        child.stderr.close()
    finally:
        server.stop(grace=2).wait()
        world.halt()

    seeded_gaps()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed", flush=True)
    return 1 if FAILURES else 0


#: The joint names and roles, written out here a second time. The suite used to compare the
#: descriptor with the driver's own tables, which is the driver agreeing with itself: a
#: front-left leg given the front-right's role passed. These are transcribed from the Go2's
#: roles in `drivers/unitree`, which the driver's header names as its source.
EXPECTED_JOINTS = ["fl.hx", "fl.hy", "fl.kn", "fr.hx", "fr.hy", "fr.kn",
                   "hl.hx", "hl.hy", "hl.kn", "hr.hx", "hr.hy", "hr.kn"]
EXPECTED_ROLES = ["left_leg.hip.abduct#0", "left_leg.thigh.pitch#0", "left_leg.calf.flex#0",
                  "right_leg.hip.abduct#0", "right_leg.thigh.pitch#0", "right_leg.calf.flex#0",
                  "left_leg.hip.abduct#1", "left_leg.thigh.pitch#1", "left_leg.calf.flex#1",
                  "right_leg.hip.abduct#1", "right_leg.thigh.pitch#1", "right_leg.calf.flex#1"]


def approx(got, want, tolerance: float = 1e-9) -> bool:
    return len(got) == len(want) and all(abs(a - b) <= tolerance for a, b in zip(got, want))


def fresh():
    """A robot of its own for one scenario: a stop, a dropped keepalive or a hidden joint
    leaves a world no later scenario should have to know about."""
    return serve(0)


def checked_in(world) -> list:
    """The estop endpoints that have checked in, which are the ones the deadman watches. The
    fake also keeps the endpoint a `SetEstopConfig` minted and nobody checked in: whether a
    real Spot strands one the same way is not something a fake can say."""
    return [name for name, endpoint in world.endpoints.items() if endpoint["seeded"]]


def release(server, world) -> None:
    server.stop(grace=2).wait()
    world.halt()


def seeded_gaps() -> None:
    """What `tools/seed-defects` found the suite could not see (DRY-COVERAGE-PLAN.md 12.C.3):
    each block here is a defect of a kind that has happened to a driver in this repository,
    seeded into this one, and let through by the checks above. Every block was shown failing
    against its defect before it was kept."""
    out = rhal.State()

    section("seeded gaps: the body's identity is written out, not read back from the driver")
    server, world, port = fresh()
    try:
        driver = SpotDriver()
        check(driver.open(config(port)) == rhal.SKR_OK, "a driver opens on a fresh robot")
        descriptor = json.loads(driver.describe())
        check([j["name"] for j in descriptor["joints"]] == EXPECTED_JOINTS,
              "the joints are the vendor's twelve, named by a transcription of their own")
        check([j["role"] for j in descriptor["joints"]] == EXPECTED_ROLES,
              "and each leg has its own side and its own ordinal (front #0, hind #1)")
        check(descriptor["verified"] is False, "nobody has run this against a robot: not verified")
        check(descriptor["simulator"] is True,
              "and simulated unless the operator says otherwise (section 35.3's direction of safety)")
        check(driver.read_sensor("front-camera", None) == rhal.SKR_ENOSYS,
              "a sensor read is ENOSYS: the driver serves none")
        check(driver.release_stop() == rhal.SKR_ENOSYS,
              "release_stop is ENOSYS, honestly: a stopped body is renewed by a new session")
        driver.close()
    finally:
        release(server, world)

    section("seeded gaps: a robot that says it is not real, and one that says it is")
    server, world, port = fresh()
    try:
        driver = SpotDriver()
        check(driver.open(config(port, simulator=False)) == rhal.SKR_OK,
              "a driver opens when told the robot is real")
        check(json.loads(driver.describe())["simulator"] is False,
              "and says so: the flag reaches the descriptor")
        driver.close()
        check(SpotDriver().open(config(port, username="admin", password="wrong")) == rhal.SKR_EIO,
              "a wrong password is refused by the robot, not accepted by the driver")
        check(world.counts["bad_login"] >= 1, "and the robot saw it")
    finally:
        release(server, world)

    section("seeded gaps: what the state carries, value for value")
    server, world, port = fresh()
    try:
        driver = SpotDriver()
        driver.open(config(port))
        world.set_body(xy=(1.5, -2.5), z=0.35, rotation=(0.1, 0.2, 0.3, 0.9),
                       twist=((0.4, 0.5, 0.6), (0.7, 0.8, 0.9)))
        world.set_joint("fl.hx", 0.11, 0.21)
        check(driver.read_state(out, 0) == rhal.SKR_OK, "the read answers")
        check(approx(out.base_pose, [1.5, -2.5, 0.35, 0.1, 0.2, 0.3, 0.9]),
              "the pose is x, y, z and then the quaternion x, y, z, w")
        check(approx(out.base_twist, [0.4, 0.5, 0.6, 0.7, 0.8, 0.9]),
              "the twist is the linear velocity and then the angular")
        check(out.position_rad[0] == 0.11 and out.velocity_rad_s[0] == 0.21,
              "a joint's velocity is its own, not its position")
        world.set_body(frame="odom")
        driver.read_state(out, 0)
        check(all(math.isnan(p) for p in out.base_pose),
              "a body the robot has not placed in the vision frame has no pose: NaN, not the origin")
        world.set_body(frame="vision")

        world.inject_fault("harness-warning")
        world.inject_behavior_fault("fall")
        driver.read_state(out, 0)
        check(out.fault_code == 2 and out.flags & rhal.FLAG_FAULT,
              "a system fault and a behaviour fault are both counted, with the flag")
        world.clear_faults()
        world.behavior_faults.clear()

        world.hw_estop = True
        driver.read_state(out, 0)
        check(out.flags & rhal.FLAG_ESTOP and out.flags & rhal.FLAG_FAULT,
              "a button pressed on the robot shows as the stop and as a fault although "
              "this driver latched nothing")
        world.hw_estop = False
        driver.read_state(out, 0)
        check(not out.flags & rhal.FLAG_ESTOP, "and clears with the button")

        world.hide_joint("fr.kn")
        check(driver.read_state(out, 0) == rhal.SKR_EIO,
              "a joint gone mid-session refuses the read rather than reporting eleven as twelve")
        world.hidden_joints.clear()
        world.state_fails = True
        check(driver.read_state(out, 0) == rhal.SKR_EIO,
              "a state the robot will not give is EIO, not a success over stale arrays")
        world.state_fails = False
        driver.close()
    finally:
        release(server, world)

    section("seeded gaps: a body of eleven joints is refused whole, and let go of")
    server, world, port = fresh()
    try:
        world.hide_joint("hl.hy")
        check(SpotDriver().open(config(port)) == rhal.SKR_EIO,
              "a robot answering eleven joints as twelve is refused at open")
        check(world.lease is None and not checked_in(world),
              "and the refused session is let go of: the lease returned, its estop endpoint gone")
    finally:
        release(server, world)

    section("seeded gaps: a bring-up that fails half way lets go of what it took")
    server, world, port = fresh()
    try:
        world.hidden_services.add("power")
        check(SpotDriver().open(config(port)) == rhal.SKR_EIO,
              "a robot that does not list its power service is refused at open")
        check(world.lease is None and not checked_in(world),
              "and the lease it had taken is back, and its estop endpoint gone: a failed open "
              "leaves nothing held, so the next attempt is not told the body is claimed")
        time.sleep(0.6)
        before = world.counts["checkins"]
        time.sleep(0.6)
        check(world.counts["checkins"] == before,
              "and no keepalive of the refused session is still checking in")
        world.hidden_services.clear()
        retry = SpotDriver()
        check(retry.open(config(port)) == rhal.SKR_OK, "and a retry after the robot is fixed opens")
        retry.close()
    finally:
        release(server, world)

    section("seeded gaps: the request's shape, before and after the robot")
    server, world, port = fresh()
    try:
        cold = SpotDriver()
        cmd = rhal.Command(mode=rhal.MODE_POSITION, values=[], mask=[])
        check(cold.write_command(cmd, 0) == rhal.SKR_EPERM,
              "a command to a driver that was never opened is EPERM before its shape is looked at")
        driver = SpotDriver()
        driver.open(config(port))
        rc, reply = driver.extension(TOKEN, b"\xff\xfe")
        check(rc == rhal.SKR_EINVAL and b"UTF-8" in reply,
              "a request that is not text says it must be UTF-8")
        rc, reply = motion(driver, "power_on")
        check(rc == rhal.SKR_OK, "the robot powers on")
        for name, kind in (("stand", "stand"), ("sit", "sit"), ("stop", "stop"),
                           ("selfright", "selfright")):
            rc, reply = motion(driver, name)
            check(rc == rhal.SKR_OK and world.last_command == kind,
                  f"'{name}' sends the vendor's {kind} and nothing else")
        rc, reply = motion(driver, "stand")
        check(json.loads(reply)["command_id"] == world.command_seq and world.command_seq > 0,
              "the command id in the reply is the robot's own")
        world.command_status = "STATUS_BEHAVIOR_FAULT"
        rc, reply = motion(driver, "stand")
        check(rc == rhal.SKR_EPERM and b"faulted" in reply,
              "a behaviour fault forbids the motion: EPERM, the state forbids it, the wire did not fail")
        world.command_status = "STATUS_INVALID_REQUEST"
        rc, reply = motion(driver, "stand")
        check(rc == rhal.SKR_EIO and b"refused" in reply,
              "a request the robot refuses is EIO with its words, never success")
        world.command_status = None
        rc, _ = motion(driver, "power_off")
        check(rc == rhal.SKR_OK, "the robot powers off")
        world.power_feedback = "STATUS_FAULTED"
        rc, reply = motion(driver, "power_on")
        check(rc == rhal.SKR_EIO and b"FAULTED" in reply,
              "power_on reads the robot's answer, not the ack: a faulted answer is EIO")
        world.power_feedback = "STATUS_SUCCESS"
        driver.close()
    finally:
        release(server, world)

    section("seeded gaps: closing a session, powered and not")
    server, world, port = fresh()
    try:
        driver = SpotDriver()
        driver.open(config(port))
        driver.close()
        check(not checked_in(world) and world.lease is None,
              "close with the motors off deregisters the endpoint and returns the lease")
        driver = SpotDriver()
        driver.open(config(port))
        motion(driver, "power_on")
        driver.close()
        check(len(checked_in(world)) == 1 and world.counts["deregister_refused"] == 0,
              "close with the motors on leaves the endpoint registered and silent, and does not "
              "ask the robot to deregister it: its timeout, not this process, stops the robot")
    finally:
        release(server, world)

    section("seeded gaps: the stop when the CUT does not land")
    server, world, port = fresh()
    try:
        driver = SpotDriver()
        driver.open(config(port, estop_timeout=0.3))
        motion(driver, "power_on")
        world.cut_fails = True
        try:
            rc = driver.emergency_stop()
        except Exception:  # noqa: BLE001
            rc = None
        check(rc == rhal.SKR_OK, "a CUT that could not reach the robot does not stop the stop")
        driver.read_state(out, 0)
        check(out.flags & rhal.FLAG_ESTOP and not world.estopped,
              "the latch shows in the state though the robot never heard the CUT")
        rc, reply = motion(driver, "stand")
        check(rc == rhal.SKR_EPERM and b"latched" in reply, "and motions are refused with the word")
        check(driver.release_stop() == rhal.SKR_ENOSYS and
              motion(driver, "stand")[0] == rhal.SKR_EPERM,
              "release_stop does not clear it: only a new session returns this body")
        driver.close()
        time.sleep(0.7)
        check(world.estopped and not world.powered,
              "the endpoint this driver left silent stops the robot at its timeout: the second stop")
        world.cut_fails = False
        check(driver.open(config(port)) == rhal.SKR_OK, "the same instance opens again")
        rc, reply = motion(driver, "stand")
        check(rc == rhal.SKR_EPERM and b"latched" in reply,
              "and the latch is still set: it clears only in a new driver instance")
        driver.close()
    finally:
        release(server, world)

    section("seeded gaps: the stop stops the keepalives, and the latch does not wait for the CUT")
    server, world, port = fresh()
    try:
        driver = SpotDriver()
        driver.open(config(port, estop_timeout=0.3))
        motion(driver, "power_on")
        check(driver.emergency_stop() == rhal.SKR_OK, "the stop latches")
        time.sleep(0.7)
        before = world.counts["checkins"]
        time.sleep(0.7)
        check(world.counts["checkins"] == before,
              "and the estop keepalive is stopped: the silent endpoint is the deadman behind the CUT")
        driver.close()
    finally:
        release(server, world)
    server, world, port = fresh()
    try:
        driver = SpotDriver()
        driver.open(config(port))
        motion(driver, "power_on")
        world.cut_delay = 0.8
        import threading
        stopper = threading.Thread(target=driver.emergency_stop)
        stopper.start()
        time.sleep(0.3)
        rc, reply = motion(driver, "stand")
        check(rc == rhal.SKR_EPERM and b"latched" in reply,
              "a motion asked for while the CUT is still on the wire is refused as latched: "
              "the latch is set first, and never waits for the robot")
        stopper.join()
        driver.close()
    finally:
        release(server, world)

    # The lease keepalive is the other liveness the stop must end (the estop keepalive is checked above): a lease
    # a stopped driver goes on retaining is a body that never lets go of the robot it has just stopped.
    section("seeded gaps: the stop ends the lease keepalive too, and open() waits for time sync")
    server, world, port = fresh()
    try:
        driver = SpotDriver()
        driver.open(config(port))
        time.sleep(2.6)  # the vendor's keepalive retains every two seconds
        check(world.counts["retains"] >= 1, "the lease keepalive is retaining the lease while the body is up (so the counter below counts)")
        motion(driver, "power_on")
        check(driver.emergency_stop() == rhal.SKR_OK, "the stop latches")
        time.sleep(0.4)  # a retain already on the wire
        before = world.counts["retains"]
        time.sleep(2.6)
        check(world.counts["retains"] == before, "and the lease keepalive is stopped with it")
        driver.close()
    finally:
        release(server, world)

    # A clock exchange that takes time: open() has to wait for it, because the command client is born with
    # the endpoint it inherits and one born before the sync refuses every motion.
    server, world, port = fresh()
    try:
        world.timesync_delay = 0.4
        driver = SpotDriver()
        check(driver.open(config(port)) == rhal.SKR_OK, "open() comes up against a robot whose clock exchange is slow")
        rc, _ = motion(driver, "power_on")
        check(rc == rhal.SKR_OK, "and the first motion is taken, not refused for want of time sync")
        rc, _ = motion(driver, "stand")
        check(rc == rhal.SKR_OK and world.counts["no_timesync_refused"] == 0, "so is the second, and the robot refused none for it")
        driver.close()
    finally:
        release(server, world)


if __name__ == "__main__":
    raise SystemExit(main())

"""Sim conformance for the LeRobot bridge, in the terms of §15.

The Python counterpart of ``drivers/mock/selftest.c``: every check names the SCTS id it stands for,
so a failure here reads the same way a failure on the bench does.

    python selftest.py

Two things it is careful about, because the whole value of the exercise is in them.

**It is run against a fake, and says so.** The bridge is written from LeRobot's public documentation
(§62.2), and passing here means the adapter is internally consistent — not that its field names
match the library's on the day you run it. That claim needs LeRobot installed and is not made here.

**The failures it forces are the ones a real robot cannot be asked for safely** (§63.2): a stalled
bus, a joint reporting nothing, a driver that raises mid-cycle, a stop path with nothing behind it.
"""

from __future__ import annotations

import json
import math
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "..", "bindings", "python"))
sys.path.insert(0, os.path.dirname(__file__))

import skynet_rhal as rhal  # noqa: E402
from skynet_rhal import Command, Frame, State  # noqa: E402
from skynet_lerobot.bridge import LeRobotBridge  # noqa: E402
from skynet_lerobot.fake import FakeCameraArm, FakeSO101  # noqa: E402
from skynet_lerobot.roles import roles_for  # noqa: E402

_PASSED = 0
_FAILED: list[str] = []


def check(scts_id: str, what: str, held: bool, detail: str = "") -> None:
    global _PASSED
    if held:
        _PASSED += 1
        print(f"ok   {scts_id:<28} {what}")
    else:
        _FAILED.append(f"{scts_id}: {what}")
        print(f"FAIL {scts_id:<28} {what}")
        if detail:
            print(f"     {detail}")


def opened(robot=None, config: str = "") -> LeRobotBridge:
    bridge = LeRobotBridge(robot or FakeSO101())
    assert bridge.open(config) == rhal.SKR_OK
    return bridge


def read(bridge: LeRobotBridge) -> tuple[int, State]:
    state = State()
    return bridge.read_state(state, 0), state


def command(bridge: LeRobotBridge, values, mask=None, mode=rhal.MODE_POSITION, seq=1) -> int:
    mask = mask if mask is not None else [1] * len(values)
    return bridge.write_command(
        Command(stamp_ns=1, seq=seq, mode=mode, values=list(values), mask=list(mask)), 0
    )


# ── rhal.abi.version ─────────────────────────────────────────────────────────

def abi_checks() -> None:
    rhal.assert_abi_layout()
    check("rhal.abi.version", "the ctypes ABI view matches the C header", True)

    robot = FakeSO101()
    bridge = LeRobotBridge(robot)
    check("rhal.abi.version", "opens", bridge.open("") == rhal.SKR_OK)
    check("rhal.abi.version", "connects the robot once", robot.connect_calls == 1)
    check("rhal.abi.version", "closes", bridge.close() == rhal.SKR_OK)
    check("rhal.abi.version", "disconnects the robot", robot.disconnect_calls == 1)

    # Every mandatory entry point exists and is callable. A Python driver cannot have a null one, so
    # what is checked instead is that none of them is the inherited Protocol stub.
    missing = [
        name
        for name in ("open", "close", "read_state", "write_command", "emergency_stop", "describe")
        if not callable(getattr(bridge, name, None))
    ]
    check("rhal.abi.version", "implements every mandatory entry point", not missing, str(missing))

    check(
        "rhal.abi.version",
        "refuses a configuration that is not JSON",
        LeRobotBridge(FakeSO101()).open("{not json") == rhal.SKR_EINVAL,
    )
    check(
        "rhal.abi.version",
        "refuses a robot with no actuated features",
        LeRobotBridge(_Bodyless()).open("") == rhal.SKR_ENODEV,
    )


class _Bodyless:
    robot_type = "nothing"
    is_connected = False
    action_features: dict = {}
    observation_features: dict = {}

    def connect(self, calibrate: bool = True) -> None:
        self.is_connected = True

    def disconnect(self) -> None:
        self.is_connected = False


# ── rhal.state.completeness ──────────────────────────────────────────────────

def state_checks() -> None:
    bridge = opened()
    rc, first = read(bridge)
    check("rhal.state.completeness", "read_state succeeds", rc == rhal.SKR_OK)
    check("rhal.state.completeness", "joint count matches the body", first.joint_count == 6)
    check("rhal.state.completeness", "timestamps are non-zero", first.stamp_ns != 0)
    check(
        "rhal.state.completeness",
        "every joint reports a finite position",
        all(math.isfinite(v) for v in first.position_rad),
    )
    # LeRobot's SO-101 reports position only. NaN is the honest value for the other two; zero would
    # be a measurement, and the distinction is why the RHAL has three arrays instead of one.
    check(
        "rhal.state.completeness",
        "an unreported velocity is NaN, not zero",
        all(math.isnan(v) for v in first.velocity_rad_s),
    )
    check(
        "rhal.state.completeness",
        "an unreported effort is NaN, not zero",
        all(math.isnan(v) for v in first.effort_nm),
    )

    _, second = read(bridge)
    check("rhal.state.completeness", "the sequence advances", second.seq == first.seq + 1)
    check(
        "rhal.state.completeness",
        "the base is declared unlocalised rather than at the origin",
        all(math.isnan(v) for v in first.base_pose),
    )

    # A dead encoder. The bridge must carry the NaN through rather than smooth it away.
    dead = opened(FakeSO101(fault="nan"))
    _, state = read(dead)
    check(
        "rhal.state.completeness",
        "a dead encoder reports NaN",
        math.isnan(state.position_rad[2]) and all(math.isfinite(v) for i, v in
                                                  enumerate(state.position_rad) if i != 2),
    )

    # A driver that raises mid-cycle reports an I/O failure. It does not propagate, and it does not
    # return a state that looks fine.
    broken = opened(FakeSO101(fault="raise"))
    rc, _ = read(broken)
    check("rhal.state.completeness", "a raising robot reports SKR_EIO", rc == rhal.SKR_EIO)

    check(
        "rhal.state.completeness",
        "reading before open is refused",
        LeRobotBridge(FakeSO101()).read_state(State(), 0) == rhal.SKR_EPERM,
    )

    # A stalled bus. The bridge stamps at read, so the timestamp keeps moving — the only evidence is
    # that the values stopped, which is what a real stall looks like and why the watchdog guard
    # watches the command sequence rather than the clock.
    stalled = opened(FakeSO101(fault="stall"))
    command(stalled, [0.5] * 6)
    _, a = read(stalled)
    command(stalled, [0.9] * 6, seq=2)
    _, b = read(stalled)
    check(
        "safety.watchdog.fires",
        "a stalled bus repeats its readings while the stamp advances",
        a.position_rad == b.position_rad and b.stamp_ns >= a.stamp_ns,
    )


# ── rhal.command.rejection ───────────────────────────────────────────────────

def command_checks() -> None:
    bridge = opened()
    read(bridge)

    check("rhal.command.rejection", "accepts a well-formed command",
          command(bridge, [0.1] * 6) == rhal.SKR_OK)
    check("rhal.command.rejection", "rejects a wrong joint count",
          command(bridge, [0.1] * 5) == rhal.SKR_EINVAL)
    check("rhal.command.rejection", "rejects an unknown mode",
          command(bridge, [0.1] * 6, mode=200) == rhal.SKR_EINVAL)
    check("rhal.command.rejection", "rejects a non-finite value",
          command(bridge, [0.1, float("nan"), 0.1, 0.1, 0.1, 0.1]) == rhal.SKR_EINVAL)
    check(
        "rhal.command.rejection",
        "rejects a mode this body does not accept",
        command(bridge, [0.1] * 6, mode=rhal.MODE_VELOCITY) == rhal.SKR_EINVAL,
    )
    check(
        "rhal.command.rejection",
        "commanding before open is refused",
        LeRobotBridge(FakeSO101()).write_command(
            Command(values=[0.0] * 6, mask=[1] * 6), 0) == rhal.SKR_EPERM,
    )

    # The mask, which is the field worth reading twice. A masked joint holds — it is commanded to
    # where it already is — and it does not go to zero.
    robot = FakeSO101()
    held = opened(robot)
    command(held, [0.4] * 6)
    read(held)
    robot.sent.clear()
    command(held, [0.9] * 6, mask=[1, 0, 0, 0, 0, 0], seq=2)
    sent = robot.sent[-1]
    check(
        "rhal.command.rejection",
        "an unmasked joint is applied",
        abs(sent["shoulder_pan.pos"] - 0.9) < 1e-9,
        str(sent),
    )
    check(
        "rhal.command.rejection",
        "a masked joint is held, not zeroed",
        abs(sent["shoulder_lift.pos"] - 0.4) < 1e-9,
        str(sent),
    )

    # Held *there*: the target is latched when the hold begins, not re-read. The joint is moved
    # under the hold (the fake is told so) and the next command still asks for 0.4 — commanding a
    # servo to wherever it reads every cycle zeroes its position error and lets a loaded joint
    # creep (DRY-COVERAGE-PLAN.md F121, carried as 8.13).
    robot._position["shoulder_lift"] = 0.55
    read(held)
    robot.sent.clear()
    command(held, [0.9] * 6, mask=[1, 0, 0, 0, 0, 0], seq=3)
    check(
        "rhal.command.rejection",
        "the hold target is latched, not re-read",
        abs(robot.sent[-1]["shoulder_lift.pos"] - 0.4) < 1e-9,
        str(robot.sent[-1]),
    )

    # And never zero for want of a reading: a hold frame before the first read has nothing to hold
    # at, and it used to be sent as a command to go to zero radians on every joint.
    cold_robot = FakeSO101()
    cold = opened(cold_robot)
    cold_robot.sent.clear()
    check(
        "rhal.command.rejection",
        "a hold before anything was read or commanded is refused, not sent as zeros",
        command(cold, [0.0] * 6, mask=[0] * 6) == rhal.SKR_EIO and not cold_robot.sent,
        str(cold_robot.sent),
    )

    failing = opened(FakeSO101(fault="send_fails"))
    read(failing)
    check("rhal.command.rejection", "a bus that will not accept reports SKR_EIO",
          command(failing, [0.1] * 6) == rhal.SKR_EIO)


# ── safety.estop.latency ─────────────────────────────────────────────────────

def estop_checks() -> None:
    robot = FakeSO101()
    bridge = opened(robot)
    read(bridge)
    check("safety.estop.latency", "emergency_stop returns", bridge.emergency_stop() == rhal.SKR_OK)
    check("safety.estop.latency", "torque is disabled on the bus", robot.bus.disable_calls == 1)

    _, state = read(bridge)
    check(
        "safety.estop.latency",
        "an asserted estop is observable in the next state",
        state.flags & rhal.FLAG_ESTOP != 0,
    )
    check(
        "safety.estop.latency",
        "commands are refused while stopped",
        command(bridge, [0.1] * 6, seq=9) == rhal.SKR_EPERM,
    )

    # No torque-disable behind the latch. The stop still holds — commands are refused — and the
    # driver must not let anyone conclude that power was cut.
    latch_only = opened(FakeSO101(fault="no_torque_disable"))
    read(latch_only)
    check(
        "safety.estop.latency",
        "a latch-only stop still returns and still refuses commands",
        latch_only.emergency_stop() == rhal.SKR_OK
        and command(latch_only, [0.1] * 6, seq=9) == rhal.SKR_EPERM,
    )


# ── embodiment.* ─────────────────────────────────────────────────────────────

def descriptor_checks() -> None:
    bridge = opened()
    descriptor = json.loads(bridge.describe())

    check("embodiment.descriptor.valid", "describe returns parseable JSON", True)
    check("embodiment.descriptor.valid", "names this body",
          descriptor["robot_slug"] == "so101-follower", descriptor["robot_slug"])
    check("embodiment.descriptor.valid", "dof agrees with the joint list",
          descriptor["dof"] == len(descriptor["joints"]) == 6)
    check("embodiment.descriptor.valid", "declares the modes the features imply",
          descriptor["control"]["modes"] == ["position"], str(descriptor["control"]["modes"]))

    check(
        "embodiment.limits.sourced",
        "declares no unsourced safety envelope",
        descriptor["envelope"] is None,
    )
    check(
        "embodiment.limits.sourced",
        "declares no joint ratings it does not have",
        all(joint["limits"] == {} for joint in descriptor["joints"]),
    )
    check(
        "embodiment.descriptor.signed",
        "does not claim to be signed",
        descriptor["verified"] is False and descriptor["digest"] is None,
    )
    check(
        "embodiment.descriptor.valid",
        "declares simulated: the robot on the other end is the fake (§35.3)",
        descriptor["simulator"] is True,
    )
    real = opened(config='{"simulator": false}')
    check(
        "embodiment.descriptor.valid",
        "and declares real when the operator says so",
        json.loads(real.describe())["simulator"] is False,
    )
    check(
        "safety.estop",
        "declares the software stop it has, and no rated stop",
        descriptor["capabilities"] == ["safety.estop"]
        and descriptor["envelope"] is None,
        str(descriptor.get("capabilities")),
    )

    # §67.2's table, which is the whole reason this bridge is worth more than a wrapper.
    roles = {joint["name"]: joint["role"] for joint in descriptor["joints"]}
    expected = {
        "shoulder_pan": "right_arm.shoulder.yaw",
        "shoulder_lift": "right_arm.shoulder.pitch",
        "elbow_flex": "right_arm.elbow.flex",
        "wrist_flex": "right_arm.wrist.flex",
        "wrist_roll": "right_arm.wrist.roll",
        "gripper": "right_hand.gripper.grip",
    }
    check("embodiment.roles.coverage", "every SO-101 motor carries its canonical role",
          roles == expected, str(roles))

    provenance = descriptor["provenance"]
    check("embodiment.descriptor.valid", "declares its tier ceiling",
          provenance["driver_ceiling"] == "guest")
    check("embodiment.descriptor.valid", "records that timestamps are taken at read",
          "acquisition" in provenance["timestamps"])
    check("embodiment.descriptor.valid", "records that units were not converted",
          "unconverted" in provenance["units"])

    # An unknown robot class gets no roles rather than plausible ones.
    unknown = FakeSO101()
    unknown.robot_type = "somebody_elses_arm"
    stranger = json.loads(opened(unknown).describe())
    check(
        "embodiment.roles.coverage",
        "an unmapped robot class gets no roles rather than plausible ones",
        all(joint["role"] is None for joint in stranger["joints"]),
    )

    # An override wins, which is how a bimanual rig fixes the side the table assumes.
    overridden = json.loads(
        opened(config='{"role_overrides": {"shoulder_pan": "left_arm.shoulder.yaw"}}').describe()
    )
    check(
        "embodiment.roles.coverage",
        "a configured override beats the table",
        overridden["joints"][0]["role"] == "left_arm.shoulder.yaw",
    )

    mapped, note = roles_for("so101_follower", FakeSO101.MOTORS, {"nonexistent": "base.wheel#0"})
    check(
        "embodiment.roles.coverage",
        "an override naming a motor this body lacks is ignored and reported",
        "nonexistent" not in mapped and "Ignored" in note,
        note,
    )

    # Unit conversion, when an operator supplies the unit.
    degrees = opened(config='{"unit": "deg"}')
    _, state = read(degrees)
    check("embodiment.descriptor.valid", "a declared unit is converted", state.position_rad[0] == 0.0)
    check(
        "embodiment.descriptor.valid",
        "and an undeclared one is refused rather than guessed",
        LeRobotBridge(FakeSO101()).open('{"unit": "furlongs"}') == rhal.SKR_EINVAL,
    )


# ── The camera split ─────────────────────────────────────────────────────────

def sensor_checks() -> None:
    plain = opened()
    check("embodiment.descriptor.valid", "a body with no cameras declares no sensors",
          json.loads(plain.describe())["sensors"] == [])
    check("embodiment.descriptor.valid", "and read_sensor reports no such device",
          plain.read_sensor("wrist", Frame()) == rhal.SKR_ENODEV)

    camera = opened(FakeCameraArm())
    descriptor = json.loads(camera.describe())
    check("embodiment.descriptor.valid", "a camera appears as a sensor, not as a joint",
          len(descriptor["sensors"]) == 1 and descriptor["dof"] == 6, str(descriptor["sensors"]))

    frame = Frame()
    check("embodiment.descriptor.valid", "a camera read before any observation reports SKR_EIO",
          camera.read_sensor("wrist", frame) == rhal.SKR_EIO)

    rc, state = read(camera)
    check("rhal.state.completeness", "the camera does not leak into the joint arrays",
          rc == rhal.SKR_OK and state.joint_count == 6)
    check("embodiment.descriptor.valid", "and is readable afterwards",
          camera.read_sensor("wrist", frame) == rhal.SKR_OK)
    check("embodiment.descriptor.valid", "with its real dimensions",
          (frame.width, frame.height, frame.stride) == (32, 24, 96),
          f"{frame.width}x{frame.height} stride {frame.stride}")
    check("embodiment.descriptor.valid", "and a stamp",
          frame.stamp_ns == state.stamp_ns)


# ── The wire protocol ────────────────────────────────────────────────────────

def protocol_checks() -> None:
    """Drive ``serve()`` through in-memory pipes.

    ``drivers/python-host`` is C and is only exercised where a C compiler is, so the protocol itself
    is held here — where every platform runs it. The failure this is really watching for is a frame
    that leaves the stream one behind, which does not present at the point it happens.
    """
    import io

    class _Pipe(io.StringIO):
        """A StringIO the writer can append to while the reader is partway through it."""

        def __init__(self) -> None:
            super().__init__()
            self._read_at = 0

        def feed(self, text: str) -> None:
            here = self.tell()
            self.seek(0, io.SEEK_END)
            super().write(text)
            self.seek(here)

    inbox = _Pipe()
    outbox = io.StringIO()

    config = '{"role_overrides": {"gripper": "left_hand.gripper.grip"}}'
    script = [
        f"O {len(config)}",
        config,
        "D",
        "S 0",
        "C 1 1 0 6 0.1 0.1 0.1 0.1 0.1 0.1 1 1 1 1 1 1",
        "F wrist",
        "E",
        "X",
    ]
    inbox.feed("\n".join(script) + "\n")

    driver = LeRobotBridge(FakeCameraArm())
    rhal.serve(driver, name="test", version="0", stdin=inbox, stdout=outbox)

    lines = outbox.getvalue().split("\n")
    # ABI 1 and whatever host protocol this repository's harness speaks: the ABI is the vendor
    # contract and is pinned, the protocol is private to `drivers/python-host` and moves with it
    # (§89.1 took it to 2), so restating the number here would be a second record of it.
    check("rhal.abi.version", "the handshake comes first and declares guest",
          lines[0].startswith(f"H 1 {rhal._PROTOCOL_VERSION} guest "), lines[0])
    check("rhal.abi.version", "open is acknowledged", lines[1] == "R 0", lines[1])

    # The descriptor is length-prefixed and the body is exactly that many bytes, or the host reads
    # into the next frame.
    declared = int(lines[2].split()[1])
    check("embodiment.descriptor.valid", "the descriptor's length prefix matches its body",
          len(lines[3].encode("utf-8")) == declared, f"{declared} vs {len(lines[3])}")
    check("embodiment.roles.coverage", "an override reaches the descriptor over the wire",
          '"left_hand.gripper.grip"' in lines[3])

    state = lines[4].split()
    check("rhal.state.completeness", "the state frame carries three arrays of the joint count",
          state[0] == "S" and state[1] == "0" and int(state[6]) == 6 and len(state) == 7 + 18,
          lines[4])
    check("rhal.state.completeness", "an unmeasured channel crosses the wire as nan",
          state[13:19] == ["nan"] * 6, str(state[13:19]))

    check("rhal.command.rejection", "a command is acknowledged", lines[5] == "R 0", lines[5])

    frame = lines[6].split()
    check("embodiment.descriptor.valid", "a frame declares its size and the payload matches",
          frame[0] == "F" and frame[1] == "0" and int(frame[7]) == 24 * 32 * 3, lines[6])
    import base64
    check("embodiment.descriptor.valid", "the frame payload decodes to exactly that many bytes",
          len(base64.b64decode(lines[7])) == int(frame[7]))

    check("safety.estop.latency", "the stop is acknowledged", lines[8] == "R 0", lines[8])
    check("rhal.abi.version", "close is acknowledged and ends the loop", lines[9] == "R 0", lines[9])

    # A driver that raises must answer rather than leave the host waiting on a line that never
    # comes, which the host would read as a hung bus.
    inbox2, outbox2 = _Pipe(), io.StringIO()
    inbox2.feed("O 0\n\nS 0\nX\n")
    rhal.serve(LeRobotBridge(FakeSO101(fault="raise")), name="t", version="0",
               stdin=inbox2, stdout=outbox2)
    replies = outbox2.getvalue().split("\n")
    check("rhal.state.completeness", "a raising driver answers rather than hanging the host",
          replies[2].startswith("S -5"), replies[2])


def main() -> int:
    print("skynet-lerobot — sim conformance against a fake robot\n")
    abi_checks()
    state_checks()
    command_checks()
    estop_checks()
    descriptor_checks()
    sensor_checks()
    protocol_checks()

    print(f"\n{_PASSED + len(_FAILED)} checks, {len(_FAILED)} failed")
    if _FAILED:
        for failure in _FAILED:
            print(f"  {failure}")
        return 1

    print(
        "\nGuest tier is the ceiling and this run does not change that (§67.3). What it shows is "
        "that the adapter is internally consistent; that its field names match LeRobot's needs the "
        "library installed, and that claim is not made here."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

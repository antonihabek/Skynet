"""skynet-ur's own suite, in two halves that are kept apart on purpose.

**The first half needs nothing.** Framing, recipe decoding, the refusals — pure functions over
bytes, and they run on every machine that builds this repository.

**The second half needs URSim**, Universal Robots' own controller software, and it is the half that
matters. `DRY-COVERAGE-PLAN.md` F15 is the argument for arranging it that way: a protocol
transcribed from memory and tested against a fake written from the same memory proves that two
copies of one misunderstanding agree. Every byte this driver sends was measured against the vendor's
implementation, and this is where that measurement is kept.

    make selftest                        # the first half
    SKYNET_UR_HOST=127.0.0.1 make check  # both

Without `SKYNET_UR_HOST` the second half is **skipped by name and the run says so**, rather than
passing quietly — a suite that reported green having exercised the protocol against nothing would be
§15's own worst shape.

Bringing URSim up:

    docker run -d --name ursim -p 29999:29999 -p 30001:30001 -p 30002:30002 \\
      -p 30003:30003 -p 30004:30004 universalrobots/ursim_e-series:latest
"""

from __future__ import annotations

import math
import os
import struct
import sys
import time

import skynet_rhal as rhal

from skynet_ur import control, rtde
from skynet_ur.bridge import INPUTS, OUTPUTS, UrBridge

FAILURES: list[str] = []
CHECKS = 0


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")


def section(title: str) -> None:
    print(f"\n{title}")


def pure() -> None:
    section("rtde — the wire, with no controller")

    check(rtde.width_of("VECTOR6D") == 48, "a VECTOR6D is six doubles")
    check(rtde.width_of("INT32") == 4, "an INT32 is four bytes")

    # `IN_USE` is what a controller answers for a variable it does not have, and the refusal is the
    # whole reason the types are checked: one unrecognised name shifts every field after it.
    try:
        rtde.width_of("IN_USE")
        check(False, "IN_USE is refused")
    except rtde.RtdeError as error:
        check("does not have the variable" in str(error), "IN_USE is refused, and explains itself")

    types = ["VECTOR6D", "INT32", "DOUBLE"]
    payload = struct.pack(">6d", *range(6)) + struct.pack(">i", -7) + struct.pack(">d", 1.5)
    values = rtde.decode(types, payload)
    check(values[0] == [0.0, 1, 2, 3, 4, 5], "a vector decodes big-endian, in order")
    check(values[1] == -7, "a negative INT32 decodes as one")
    check(values[2] == 1.5, "and a double after it is at the right offset")
    check(rtde.encode(types, values) == payload, "and the inverse returns the same bytes")

    # A recipe and a package that disagree is a client reading one variable's bytes as another's.
    try:
        rtde.decode(types, payload[:-1])
        check(False, "a short package is refused")
    except rtde.RtdeError as error:
        check("disagree" in str(error), "a package that does not match its recipe is refused")

    # F75: a packet that straddles the moment the socket runs dry. The drain is non-blocking, so
    # the first half of a packet can be in hand when the socket has nothing more; the header used
    # to be consumed before the body was known to have arrived, and the rest of the stream was
    # then read from the wrong offset.
    class Trickle:
        """A socket that delivers scripted chunks, and says `would block` between them."""

        def __init__(self, chunks: list) -> None:
            self._chunks = list(chunks)

        def settimeout(self, _timeout: float) -> None:
            pass

        def recv(self, _size: int) -> bytes:
            if not self._chunks:
                raise BlockingIOError
            chunk = self._chunks.pop(0)
            if chunk is None:
                raise BlockingIOError
            return chunk

    def data_package(recipe: int, value: float) -> bytes:
        body = bytes([recipe]) + struct.pack(">d", value)
        return struct.pack(">HB", 3 + len(body), rtde.DATA_PACKAGE) + body

    first, second, third = data_package(1, 1.0), data_package(1, 2.0), data_package(1, 3.0)
    session = rtde.Rtde.__new__(rtde.Rtde)
    session._socket = Trickle([first + second[:5], None, second[5:] + third, None])
    session._buffer = b""
    session.messages = []
    session.output = rtde.Recipe(1, ["x"], ["DOUBLE"])
    try:
        seen = [session.newest(), session.newest()]
    except Exception as error:  # the failure this check exists for raises, so it is caught
        seen = [f"{type(error).__name__}: {error}"]
    check(
        seen == [{"x": 1.0}, {"x": 3.0}],
        "a packet split across the moment the socket ran dry is completed on the next drain, "
        "not read from the wrong offset (F75)",
    )

    section("control — the program that reads the registers")
    program = control.SERVO_PROGRAM
    check("servoj(" in program, "the program servos")
    check(
        f"read_input_integer_register({control.COMMAND_REGISTER})" in program,
        "and reads its gate from the integer register the driver writes",
    )
    check(
        program.count("read_input_float_register") == 6,
        "and six float registers, one per joint",
    )
    # The gate is what makes a hold expressible: without it a loop that stopped writing would leave
    # `servoj` tracking the last setpoint for ever.
    check("sync()" in program, "and yields a cycle rather than servoing when the gate is clear")

    section("descriptor — the transcribed ratings, with no controller")
    # `_build_descriptor` reads only the bridge's own fields, so the whole model matrix runs
    # here, against no arm at all. The figures are UR's tech sheets and the max-torques support
    # article — a second person verifies them against those documents, not against this file.
    import json

    def described(model: str | None) -> dict:
        bridge = UrBridge()
        bridge._model = model
        return json.loads(bridge._build_descriptor({}))

    expected = {
        # model: (envelope velocity rad/s, envelope torque Nm).
        "UR3E": (math.pi, 9.0),
        "UR5E": (math.pi, 28.0),
        "UR7E": (math.pi, 28.0),
        "UR10E": (2.0 * math.pi / 3.0, 54.0),
        "UR12E": (2.0 * math.pi / 3.0, 54.0),
        "UR16E": (2.0 * math.pi / 3.0, 54.0),
    }
    for model, (velocity, torque) in expected.items():
        document = described(model)
        envelope = document["envelope"]
        check(envelope is not None, f"{model} has an envelope")
        check(
            abs(envelope["joint_velocity_max_rad_s"] - velocity) < 1e-9,
            f"{model} velocity is the narrowest across its joints",
        )
        check(
            envelope["joint_torque_max_nm"] == torque,
            f"{model} torque is the narrowest across its joints",
        )
        check(envelope["watchdog_ms"] == 80.0, f"{model} watchdog is ten servoj periods")
        check(model in envelope["source"], f"{model} names its own tech sheet as the source")
        check(
            envelope["basis"] == "declared",
            f"{model} states the kind beside the citation (§5.3)",
        )
    wrist = described("UR3E")["joints"][5]["limits"]
    check(
        abs(wrist["velocity_max_rad_s"] - 2.0 * math.pi) < 1e-9
        and wrist["effort_max_nm"] == 9.0,
        "and a per-joint figure differs from the envelope's where the arm does (UR3e wrist)",
    )
    check(
        "safety.estop" in described("UR5E")["capabilities"],
        "the software stop the driver has is the capability it declares",
    )
    check(described(None)["envelope"] is None, "no model, no envelope")
    check(
        "velocity_max_rad_s" not in described(None)["joints"][0]["limits"],
        "and no per-joint figures either",
    )
    check(described("UR30")["envelope"] is None, "an untranscribed model runs unclamped, not wrong")
    check(
        "safety.estop" in described(None)["capabilities"],
        "but the stop capability needs no model",
    )

    section("bridge — a refused command stores nothing, with no controller")

    class Recorder:
        """What the bridge would write to the controller, in place of an RTDE session."""

        def __init__(self) -> None:
            self.writes: list[list[float]] = []

        def write(self, values: list) -> None:
            self.writes.append(list(values))

    bridge = UrBridge()
    recorder = Recorder()
    bridge._rtde = recorder
    refused = bridge.write_command(
        rhal.Command(mode=rhal.MODE_POSITION, values=[0.5, math.nan, 0.0, 0.0, 0.0, 0.0], mask=[1] * 6),
        0,
    )
    accepted = bridge.write_command(
        rhal.Command(mode=rhal.MODE_POSITION, values=[9.0, 0.1, 0.0, 0.0, 0.0, 0.0], mask=[0, 1, 1, 1, 1, 1]),
        0,
    )
    # F74: the setpoint was stored joint by joint while the refusal was still being decided, so the
    # first joint of a command refused on its second was kept — and sent, as the "held" value, with
    # the next command that masked that joint out.
    check(
        refused == rhal.SKR_EINVAL
        and accepted == rhal.SKR_OK
        and len(recorder.writes) == 1
        and recorder.writes[0][0] == 0.0,
        "a command refused on its second joint stores nothing, so a later hold holds where the arm "
        "was told to be (F74)",
    )


def against_ursim(host: str) -> None:
    section(f"bridge — against URSim at {host}")

    bridge = UrBridge()
    import json

    rc = bridge.open(json.dumps({"host": host, "simulator": True}))
    check(rc == rhal.SKR_OK, "the arm comes up: powered, brakes released, exchange started")
    if rc != rhal.SKR_OK:
        return

    descriptor = json.loads(bridge.describe())
    check(descriptor["dof"] == 6, "six degrees of freedom")
    check(len(descriptor["joints"]) == 6, "and six joints in the descriptor")
    check(descriptor["simulator"] is True, "declared simulated (§35.3)")
    check(
        descriptor["envelope"] is None,
        "and no safety envelope — no model was declared, so no ratings were transcribed",
    )

    state = rhal.State()
    check(bridge.read_state(state, 0) == rhal.SKR_OK, "read_state answers")
    check(len(state.position_rad) == 6, "with six positions")
    check(all(math.isfinite(value) for value in state.position_rad), "every one of them finite")
    check(
        all(math.isnan(value) for value in state.effort_nm),
        "torque is NaN — `actual_current` is amps, and the constant that converts it is not "
        "published",
    )
    check(all(math.isfinite(v) for v in state.velocity_rad_s), "velocity is measured and finite")

    start = list(state.position_rad)
    target = list(start)
    target[0] = start[0] + 0.20

    # Fed at the rate the control program's `servoj` covers. A loop slower than that decelerates the
    # arm, which is the honest behaviour and is why this is the thing that must not be late.
    command = rhal.Command(mode=rhal.MODE_POSITION, values=target, mask=[1, 0, 0, 0, 0, 0])
    check(bridge.write_command(command, 0) == rhal.SKR_OK, "a setpoint is accepted")
    deadline = time.monotonic() + 20.0
    reached = False
    while time.monotonic() < deadline and not reached:
        bridge.write_command(command, 0)
        bridge.read_state(state, 0)
        reached = abs(state.position_rad[0] - target[0]) < 0.01
        time.sleep(control.SERVO_PERIOD_S)
    check(reached, f"and the arm reaches it (joint 0: {start[0]:+.3f} → {state.position_rad[0]:+.3f})")
    check(
        abs(state.position_rad[1] - start[1]) < 0.01,
        "while a masked-out joint stays where it was",
    )

    check(
        bridge.write_command(rhal.Command(mode=rhal.MODE_POSITION, values=[0.0] * 3, mask=[1] * 3), 0)
        == rhal.SKR_EINVAL,
        "a command with the wrong joint count is refused",
    )
    check(
        bridge.write_command(rhal.Command(mode=rhal.MODE_TORQUE, values=[0.0] * 6, mask=[1] * 6), 0)
        == rhal.SKR_EINVAL,
        "and a mode `servoj` does not speak is refused",
    )
    check(
        bridge.write_command(
            rhal.Command(mode=rhal.MODE_POSITION, values=[math.nan] * 6, mask=[1] * 6), 0
        )
        == rhal.SKR_EINVAL,
        "and a setpoint that is not a number is refused",
    )

    check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
    bridge.read_state(state, 0)
    check(state.flags & rhal.FLAG_ESTOP != 0, "and the next state says the stop is asserted")
    check(
        bridge.write_command(command, 0) == rhal.SKR_EPERM,
        "and a command after it is refused",
    )
    print(
        f"  ----  {bridge.stale_cycles} cycle(s) had no new sample. That is this machine keeping "
        "up\n        with a 125 Hz stream, and it is a property of the machine rather than of the "
        "driver."
    )
    check(bridge.close() == rhal.SKR_OK, "close")

    section("bridge — what it refuses")
    check(UrBridge().open("{}") == rhal.SKR_ENODEV, "a configuration with no host is refused")
    check(UrBridge().open("not json") == rhal.SKR_EINVAL, "and one that is not JSON is refused")
    # The control for the two above, and it needs a retry — which is the finding rather than a
    # flake. RTDE's general-purpose registers are allocated to **one session at a time**, and a
    # controller holds them for a second or two after a client's socket goes. The suite met this as
    # `IN_USE` on a recipe that had worked moments earlier, and the refusal it produced named the
    # wrong cause ("this controller has no such variable"); both the message and this retry exist
    # because of that run.
    deadline = time.monotonic() + 15.0
    rc = rhal.SKR_EIO
    while time.monotonic() < deadline:
        rc = UrBridge().open('{"host": "%s", "frequency_hz": 125}' % host)
        if rc == rhal.SKR_OK:
            break
        time.sleep(1.0)
    check(rc == rhal.SKR_OK, "and the same host comes up again once the registers are released")


def main() -> int:
    print("skynet-ur selftest\n")
    pure()

    host = os.environ.get("SKYNET_UR_HOST", "").strip()
    if host:
        against_ursim(host)
    else:
        section("bridge — not exercised")
        print(
            "  ----  SKYNET_UR_HOST is not set, so nothing above the wire format was measured.\n"
            "        Every byte this driver sends was verified against URSim — Universal Robots'\n"
            "        own controller software — and none of that verification ran here. Start one\n"
            "        and set the variable; the command is in this file's header. A suite that\n"
            "        reported green having talked to nothing is §15's own worst shape."
        )

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    for failure in FAILURES:
        print(f"  - {failure}")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())

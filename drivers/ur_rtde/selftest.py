"""skynet-ur's own suite, in two halves that are kept apart on purpose.

**The first half needs nothing.** Framing, recipe decoding, the refusals — pure functions over
bytes — and the bridge's own decisions (what a sample becomes, what a stop does, what `open` will
not do), run against a scripted controller and a clock the test owns. They run on every machine that
builds this repository. What they cannot say is that the *bytes* are the controller's: a scripted
controller agrees with whoever scripted it, which is F15's point and the reason the second half exists.

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

import contextlib
import io
import math
import os
import re
import struct
import sys
import threading
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


class Clock:
    """A clock the test owns: `sleep` moves it, so a wait bounded by two periods is exactly two periods."""

    def __init__(self) -> None:
        self.now = 1000.0

    def monotonic(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.now += seconds


class ScriptedRtde:
    """The controller's RTDE side as the bridge sees it: a queue of samples that become readable at a time.

    It stands in for `skynet_ur.rtde.Rtde` so the bridge's own decisions — what it makes of a sample, what it writes, what a
    stop does — can be held with no controller and no clock. It says nothing about the wire: the frames are held above, and
    against URSim below. `fail` names the step that raises, as the real session does when the controller refuses it.
    """

    made: list["ScriptedRtde"] = []
    clock: Clock
    fail: str | None = None
    #: The sample every session made is born holding, so `open`'s wait for the first one is answered.
    first: dict | None = None

    def __init__(self, host: str, port: int = 30004) -> None:
        self.host = host
        self.port = port
        self.queue: list[tuple[float, dict]] = []
        self.writes: list[list[float]] = []
        self.calls: list[str] = []
        self.messages: list[str] = []
        self.closed = False
        self.write_fails = False
        #: What `newest` raises while it is set: the controller closing the stream, or a package that is not one.
        self.newest_raises: Exception | None = None
        #: The bound `pause` was asked to wait under, where it was given one.
        self.pause_timeout: float | None = None
        ScriptedRtde.made.append(self)
        if ScriptedRtde.first is not None:
            self.publish(ScriptedRtde.first)
        self._step("connect")

    def _step(self, name: str) -> None:
        self.calls.append(name)
        if ScriptedRtde.fail == name:
            raise rtde.RtdeError(f"the scripted controller refused `{name}`")

    def negotiate(self, version: int = 2) -> None:
        self._step(f"negotiate {version}")

    def controller_version(self) -> tuple[int, int, int, int]:
        self._step("version")
        return (5, 26, 0, 0)

    def setup_outputs(self, names: list, frequency_hz: float) -> None:
        self.outputs, self.frequency = list(names), frequency_hz
        self._step("setup_outputs")

    def setup_inputs(self, names: list) -> None:
        self.inputs = list(names)
        self._step("setup_inputs")

    def start(self) -> None:
        self._step("start")

    def pause(self, timeout_s: float | None = None) -> None:
        self.pause_timeout = timeout_s
        self._step("pause")

    def write(self, values: list) -> None:
        if self.write_fails:
            raise OSError("the scripted socket is gone")
        self.writes.append(list(values))

    def publish(self, sample: dict, after_s: float = 0.0) -> None:
        self.queue.append((ScriptedRtde.clock.now + after_s, sample))

    def newest(self) -> dict | None:
        """Latest-wins, as the real drain is: everything readable now is consumed, the last one is returned."""
        if self.newest_raises is not None:
            raise self.newest_raises
        ready = [sample for at, sample in self.queue if at <= ScriptedRtde.clock.now]
        self.queue = [(at, sample) for at, sample in self.queue if at > ScriptedRtde.clock.now]
        return ready[-1] if ready else None

    def close(self) -> None:
        self.closed = True


class ScriptedDashboard:
    """The dashboard as the bridge sees it: what safety status it answers, and every command it was sent."""

    made: list["ScriptedDashboard"] = []
    safety = "NORMAL"
    fail_on: str | None = None
    #: Real seconds a command takes to be answered: a dashboard that is slow and does answer.
    delay_s = 0.0
    #: While an event is set here, a command named `hang_on` waits for it and then fails as a socket that timed out does: a dashboard that has gone quiet.
    hang: threading.Event | None = None
    hang_on = "stop"
    #: How many times each command was tried, whether or not it was answered.
    tries: dict[str, int] = {}

    def __init__(self, host: str, port: int = 29999) -> None:
        self.port = port
        self.commands: list[str] = []
        #: The timeout each command was asked with, where it was given one.
        self.timeouts: dict[str, float | None] = {}
        self.brought_up = False
        self.closed = False
        ScriptedDashboard.made.append(self)

    def safety_status(self) -> str:
        return ScriptedDashboard.safety

    def bring_up(self) -> None:
        self.brought_up = True

    def command(self, text: str, timeout_s: float | None = None) -> str:
        ScriptedDashboard.tries[text] = ScriptedDashboard.tries.get(text, 0) + 1
        self.timeouts[text] = timeout_s
        if ScriptedDashboard.hang is not None and text == ScriptedDashboard.hang_on:
            ScriptedDashboard.hang.wait(10.0)
            raise TimeoutError("timed out")
        if ScriptedDashboard.delay_s and text == ScriptedDashboard.hang_on:
            # Only the command named in `hang_on` is slow, so that a `play` asked for while a `stop` is still being answered does not sleep alongside it and land in whichever order the threads start in.
            time.sleep(ScriptedDashboard.delay_s)
        if ScriptedDashboard.fail_on == text:
            raise OSError("the scripted dashboard is gone")
        self.commands.append(text)
        return "ok"

    def close(self) -> None:
        self.closed = True


class ScriptedScript:
    """The secondary interface's socket: whatever program was sent."""

    def __init__(self, address: tuple) -> None:
        self.address, self.sent, self.closed = address, b"", False

    def sendall(self, data: bytes) -> None:
        self.sent += data

    def close(self) -> None:
        self.closed = True


def sample_of(q: list, qd: list, safety: int = 1, mode: int = 7) -> dict:
    """One output package, with joints that differ from each other and from zero, so a sign, an order or a home pose shows."""
    return {
        "actual_q": list(q),
        "actual_qd": list(qd),
        "actual_current": [0.5] * 6,
        "robot_mode": mode,
        "safety_status": safety,
        "runtime_state": 2,
    }


Q0 = [0.11, -0.22, 0.33, -0.44, 0.55, -0.66]
QD0 = [0.01, -0.02, 0.03, -0.04, 0.05, -0.06]


def bridge_logic() -> None:
    """What the bridge decides, against a scripted controller and a clock the test owns (DRY-COVERAGE-PLAN.md 12.C.3).

    `against_ursim` is the half that matters and it is a container away; this is the half a review can run. Both were needed:
    seeded defects (`tools/seed-defects/`) found that twenty of this driver's twenty-seven realistic defects — a mirrored base
    joint, a stop that no longer tells the program to hold, a latch that outlives its session, a protective stop the driver
    would clear — left this file green without URSim, because everything above the wire format was only ever run against it.
    """
    import json

    from skynet_ur import bridge as bridge_module

    section("bridge — its own decisions, against a scripted controller")

    clock = Clock()
    ScriptedRtde.clock = clock
    saved = (bridge_module.Rtde, bridge_module.Dashboard, bridge_module.socket, bridge_module.time)

    class Sockets:
        """`socket` as the bridge names it, so the program upload goes where this test can read it."""

        made: list[ScriptedScript] = []

        @staticmethod
        def create_connection(address: tuple, timeout: float = 0.0) -> ScriptedScript:
            Sockets.made.append(ScriptedScript(address))
            return Sockets.made[-1]

    bridge_module.Rtde, bridge_module.Dashboard = ScriptedRtde, ScriptedDashboard
    bridge_module.socket, bridge_module.time = Sockets, clock

    def reset() -> None:
        ScriptedRtde.made.clear()
        ScriptedRtde.fail = None
        ScriptedRtde.first = None
        ScriptedDashboard.made.clear()
        ScriptedDashboard.safety = "NORMAL"
        ScriptedDashboard.fail_on = None
        ScriptedDashboard.delay_s = 0.0
        ScriptedDashboard.hang = None
        ScriptedDashboard.tries = {}
        Sockets.made.clear()

    def opened(config: dict | None = None, sample: dict | None = None) -> tuple[UrBridge, int]:
        """A bridge opened against the scripted controller, which has one sample ready."""
        reset()
        ScriptedRtde.first = sample or sample_of(Q0, QD0)
        bridge = UrBridge()
        rc = bridge.open(json.dumps({"host": "10.9.8.7", "model": "UR5e", **(config or {})}))
        return bridge, rc

    def read(bridge: UrBridge, after: dict | None = None) -> tuple[int, rhal.State]:
        if after is not None:
            bridge._rtde.publish(after)  # type: ignore[union-attr]
        state = rhal.State()
        return bridge.read_state(state, 0), state

    def position(values: list, mask: list | None = None) -> rhal.Command:
        return rhal.Command(mode=rhal.MODE_POSITION, values=list(values), mask=mask if mask is not None else [1] * 6)

    def told_to_stop(bridge: UrBridge) -> None:
        """Wait for the thread that tells the dashboard: the stop's second half runs on its own, so that the stop does not wait for the link it is stopping through (12.C.4)."""
        for thread in list(getattr(bridge, "_stop_threads", [])):
            thread.join(5.0)

    try:
        # ── bring-up ────────────────────────────────────────────────────────
        bridge, rc = opened()
        session = ScriptedRtde.made[0]
        check(rc == rhal.SKR_OK, "a bridge opens against a controller that answers every step")
        check(session.host == "10.9.8.7" and ScriptedDashboard.made[0].brought_up, "the arm is brought up over the dashboard before the exchange starts")
        check(
            session.outputs == OUTPUTS and session.inputs == INPUTS and abs(session.frequency - 125.0) < 1e-9,
            "the recipes it asks for are the ones the module names, at the default 125 Hz",
        )
        check(
            session.calls == ["connect", "negotiate 2", "version", "setup_outputs", "setup_inputs", "start"],
            f"and the exchange is negotiated in the order RTDE needs: {session.calls}",
        )
        check(
            len(Sockets.made) == 1 and Sockets.made[0].address == ("10.9.8.7", 30002) and Sockets.made[0].sent == control.SERVO_PROGRAM.encode(),
            "the control program goes to the secondary interface, whole, once",
        )
        check(
            session.port == 30004 and ScriptedDashboard.made[0].port == 29999,
            "with no `ports` the sockets are the vendor's own: RTDE 30004, the dashboard 29999, and the secondary interface 30002 above",
        )
        opened({"ports": {"rtde": 41004, "script": 41002, "dashboard": 41999}})
        check(
            ScriptedRtde.made[0].port == 41004 and ScriptedDashboard.made[0].port == 41999 and Sockets.made[0].address == ("10.9.8.7", 41002),
            "`ports` moves each of the three sockets, for a controller behind a forward or a relay",
        )
        opened({"ports": {"rtde": 41004}})
        check(
            ScriptedRtde.made[0].port == 41004 and ScriptedDashboard.made[0].port == 29999 and Sockets.made[0].address == ("10.9.8.7", 30002),
            "and a socket it does not name keeps the vendor's own",
        )
        for bad, words in (
            ({"ports": [30004]}, ("`ports`", "not an object")),
            ({"ports": {"rtd": 30004}}, ("`rtd`", "not one of this controller's sockets")),
            ({"ports": {"rtde": 0}}, ("`ports.rtde`", "not a TCP port", "30004")),
            ({"ports": {"rtde": 70000}}, ("`ports.rtde`", "not a TCP port")),
            ({"ports": {"rtde": True}}, ("`ports.rtde`", "not a TCP port")),
            ({"ports": {"dashboard": "29999"}}, ("`ports.dashboard`", "not a TCP port", "29999")),
        ):
            with contextlib.redirect_stderr(io.StringIO()) as said:
                _, rc = opened(bad)
            check(
                rc == rhal.SKR_EINVAL and not ScriptedRtde.made and not Sockets.made and all(word in said.getvalue() for word in words),
                f"{bad} is refused before any socket is opened, saying what `ports` takes: {said.getvalue().strip()[:110]}",
            )
        described = json.loads(bridge.describe())
        check(described["envelope"] is not None and "UR5E" in described["envelope"]["source"], "a declared model is canonicalised and its ratings carried")
        check(json.loads(opened({"model": " ur5e "})[0].describe())["envelope"] is not None, "case and spaces in the model do not change which arm it is")
        for stranger in ("UR5", "UR20", "UR30", "cb3", ""):
            check(
                json.loads(opened({"model": stranger})[0].describe())["envelope"] is None,
                f"model {stranger!r} has no transcribed ratings and is given none — not a neighbour's",
            )

        # ── what a sample becomes ───────────────────────────────────────────
        bridge, _ = opened()
        rc, state = read(bridge, sample_of(Q0, QD0))
        check(rc == rhal.SKR_OK and state.position_rad == Q0, f"positions are the controller's, joint for joint and sign for sign: {state.position_rad}")
        check(state.velocity_rad_s == QD0, f"and so are velocities, each in its own joint's slot: {state.velocity_rad_s}")
        check(
            len(state.effort_nm) == 6 and all(math.isnan(v) for v in state.effort_nm),
            "torque is NaN on every joint: current is not torque and a zero would be a measurement (§24)",
        )
        check(state.flags == rhal.HOMING_NOT_REQUIRED and state.fault_code == 0, f"a running arm at a normal safety status carries no flag but its homing: {state.flags:#x}")
        first_seq = state.seq
        rc, state = read(bridge, sample_of(Q0, QD0))
        check(state.seq == first_seq + 1, "a new sample advances the sequence by one")
        period = 1.0 / 125.0
        started = clock.now
        rc, state = read(bridge)
        waited = clock.now - started
        check(
            rc == rhal.SKR_OK and state.seq == first_seq + 1 and bridge.stale_cycles == 1,
            "two whole periods with nothing new reports the last sample with its sequence held, and counts it (§24)",
        )
        check(state.position_rad == Q0, "the held sample is the last one read, not zeros")
        check(2.0 * period <= waited < 2.0 * period + 0.002, f"and the wait was two periods, no shorter (jitter) and no longer (a loop kept waiting): {waited * 1000:.1f} ms")
        held_seq = state.seq
        bridge._rtde.publish(sample_of(Q0, QD0), after_s=1.5 * period)  # type: ignore[union-attr]
        rc, state = read(bridge)
        check(
            state.seq == held_seq + 1 and bridge.stale_cycles == 1,
            "a sample that arrives a period and a half late is still fresh: jitter is not a stalled producer (F171)",
        )
        reset()
        lonely = UrBridge()
        lonely._rtde = ScriptedRtde("x")
        check(lonely.read_state(rhal.State(), 0) == rhal.SKR_ETIMEDOUT, "a read before any sample has ever arrived is a timeout, not a zero pose")
        check(UrBridge().read_state(rhal.State(), 0) == rhal.SKR_EPERM, "and a read on a bridge that was never opened is refused")

        # The stream closing under a read (12.C.4, `reset`): the controller's socket gave EOF and `newest` raised `RtdeError` out of `read_state`, as an HTTPException had out of two other drivers' (F257). An entry point
        # that cannot say what happened with a return code has no business raising something the host did not ask for. It is `EIO`, named, and said once per run of the same cause (F247: not every period).
        bridge, _ = opened()
        session = ScriptedRtde.made[0]
        read(bridge, sample_of(Q0, QD0))
        session.newest_raises = rtde.RtdeError("the controller closed the connection")
        with contextlib.redirect_stderr(io.StringIO()) as said:
            try:
                codes = [bridge.read_state(rhal.State(), 0) for _ in range(5)]
                raised = None
            except Exception as error:  # noqa: BLE001 — what escapes is the finding
                codes, raised = [], error
        check(raised is None and codes == [rhal.SKR_EIO] * 5, f"a stream the controller closed is EIO, and not an exception out of read_state ({raised!r})")
        check(said.getvalue().count("the controller closed the connection") == 1 and "opened again" in said.getvalue(), f"said once, with what to do: {said.getvalue().strip()[:200]}")
        session.newest_raises = None
        rc, state = read(bridge, sample_of(Q0, QD0))
        check(rc == rhal.SKR_OK and state.position_rad == Q0, "and a stream that comes back is read again")
        session.newest_raises = rtde.RtdeError("a data package carried 7 bytes and this recipe is 133")
        with contextlib.redirect_stderr(io.StringIO()) as said:
            rc = bridge.read_state(rhal.State(), 0)
        check(rc == rhal.SKR_EIO and "7 bytes" in said.getvalue(), "a package that is not the recipe's is EIO too, and says what it was")

        # A setpoint the socket cannot carry says so once per run of the same cause: a control loop at 50 Hz would otherwise print it fifty times a second (found as 98 identical lines in one chaos run).
        aim = [0.5, 0.4, 0.3, 0.2, 0.1, 0.05]
        bridge, _ = opened()
        session = ScriptedRtde.made[0]
        session.write_fails = True
        with contextlib.redirect_stderr(io.StringIO()) as said:
            codes = [bridge.write_command(position(aim), 0) for _ in range(20)]
        check(codes == [rhal.SKR_EIO] * 20 and said.getvalue().count("did not reach the controller") == 1, f"twenty setpoints the socket cannot carry are twenty EIOs and one message ({said.getvalue().count('did not reach the controller')})")
        session.write_fails = False
        check(bridge.write_command(position(aim), 0) == rhal.SKR_OK, "and a setpoint that is carried again is accepted")
        session.write_fails = True
        with contextlib.redirect_stderr(io.StringIO()) as said:
            bridge.write_command(position(aim), 0)
        check(said.getvalue().count("did not reach the controller") == 1, "a failure after a success is said again: it is a new run")

        # Every safety status and both kinds of mode: which flag each raises, and never the wrong one.
        for safety in range(1, 10):
            for mode in (7, 5):
                bridge, _ = opened(sample=sample_of(Q0, QD0, safety=safety, mode=mode))
                _, state = read(bridge, sample_of(Q0, QD0, safety=safety, mode=mode))
                at = f"safety {safety} ({bridge_module.SAFETY_NAMES[safety]}), robot_mode {mode}"
                check(bool(state.flags & rhal.FLAG_ESTOP) == (safety in (6, 7)), f"{at}: the emergency-stop flag is up exactly for the two emergency stops")
                check(bool(state.flags & rhal.FLAG_DEGRADED) == (safety != 1 or mode != 7), f"{at}: degraded exactly when the arm is not running normally")
                check(state.fault_code == (0 if safety == 1 else safety), f"{at}: the controller's own safety code is surfaced")

        # ── commands ────────────────────────────────────────────────────────
        bridge, _ = opened()
        session = ScriptedRtde.made[0]
        target = [0.5, 0.4, 0.3, 0.2, 0.1, 0.05]
        check(bridge.write_command(position(target), 0) == rhal.SKR_OK, "a position command is accepted")
        check(
            session.writes[-1] == target + [control.CMD_SERVO],
            f"and goes out as six setpoints and the gate open: {session.writes[-1]}",
        )
        check(
            bridge.write_command(position([9.0] * 6, mask=[0] * 6), 0) == rhal.SKR_OK and session.writes[-1] == target + [control.CMD_SERVO],
            "a command that masks every joint out holds where the last one asked",
        )
        fresh, _ = opened()
        fresh_session = ScriptedRtde.made[0]
        check(
            fresh.write_command(position([9.0] * 6, mask=[0] * 6), 0) == rhal.SKR_OK and fresh_session.writes[-1][:6] == Q0,
            f"and a hold as the very first command holds where the arm *is* (its first read), not at zero: {fresh_session.writes[-1][:6]}",
        )
        sent = len(session.writes)
        for what, command, code in (
            ("the wrong joint count", rhal.Command(mode=rhal.MODE_POSITION, values=[0.0] * 3, mask=[1] * 3), rhal.SKR_EINVAL),
            ("a velocity command", rhal.Command(mode=rhal.MODE_VELOCITY, values=[0.0] * 6, mask=[1] * 6), rhal.SKR_EINVAL),
            ("a torque command", rhal.Command(mode=rhal.MODE_TORQUE, values=[0.0] * 6, mask=[1] * 6), rhal.SKR_EINVAL),
            ("a NaN setpoint", position([0.0, 0.0, math.nan, 0.0, 0.0, 0.0]), rhal.SKR_EINVAL),
            ("an infinite setpoint", position([math.inf, 0.0, 0.0, 0.0, 0.0, 0.0]), rhal.SKR_EINVAL),
        ):
            check(bridge.write_command(command, 0) == code, f"{what} is refused")
        check(len(session.writes) == sent, "and none of the refused commands reached the controller")
        session.write_fails = True
        check(bridge.write_command(position(target), 0) == rhal.SKR_EIO, "a setpoint the socket cannot carry is an I/O error, not a success")
        session.write_fails = False
        check(UrBridge().write_command(position(target), 0) == rhal.SKR_EPERM, "a command to a bridge that was never opened is refused")

        # ── the stop ────────────────────────────────────────────────────────
        bridge, _ = opened()
        session, dashboard = ScriptedRtde.made[0], ScriptedDashboard.made[0]
        bridge.write_command(position(target), 0)
        check(bridge.emergency_stop() == rhal.SKR_OK, "emergency_stop returns")
        check(
            session.writes[-1] == target + [control.CMD_HOLD],
            f"and the control program was told to hold, with the last setpoint beside it: {session.writes[-1]}",
        )
        told_to_stop(bridge)
        check("stop" in dashboard.commands, f"and the dashboard was told to stop the program: {dashboard.commands}")
        _, state = read(bridge, sample_of(Q0, QD0))
        check(bool(state.flags & rhal.FLAG_ESTOP), "the next state says the stop is asserted, though the controller's own status is normal")
        sent = len(session.writes)
        check(bridge.write_command(position(target), 0) == rhal.SKR_EPERM and len(session.writes) == sent, "and every later command is refused before it is written")
        check(bridge.emergency_stop() == rhal.SKR_OK, "a second stop is absorbed")
        session.write_fails = True
        ScriptedDashboard.fail_on = "stop"
        with contextlib.redirect_stderr(io.StringIO()):
            check(bridge.emergency_stop() == rhal.SKR_OK, "a stop whose sockets are gone still returns, and the latch stays")
            told_to_stop(bridge)
        session.write_fails = False
        ScriptedDashboard.fail_on = None

        check(bridge.release_stop() == rhal.SKR_OK, "release_stop returns")
        check("play" in dashboard.commands, f"and restarts the program the stop halted: {dashboard.commands}")
        check(bridge.write_command(position(target), 0) == rhal.SKR_OK, "the latch is cleared: commands are accepted again")
        _, state = read(bridge, sample_of(Q0, QD0))
        check(not state.flags & rhal.FLAG_ESTOP, "and the state no longer says a stop is asserted")
        bridge.emergency_stop()
        ScriptedDashboard.fail_on = "play"
        check(bridge.release_stop() == rhal.SKR_EIO, "a restart that fails is an I/O error")
        check(bridge.write_command(position(target), 0) == rhal.SKR_EPERM, "and re-latches: the program never came back, so the body is not commandable")
        ScriptedDashboard.fail_on = None

        # ── the stop, on a controller that has gone quiet (12.C.4) ──────────
        # The link-chaos contract found a stop that returned after 5,005 ms against a blackholed link: it asked the dashboard for `stop` and waited for the answer on a socket with a five-second timeout.
        # §4: the emergency stop never blocks. The latch and the hold are set before it returns; the dashboard is told from a thread of its own.
        bridge, _ = opened()
        session, dashboard = ScriptedRtde.made[0], ScriptedDashboard.made[0]
        bridge.write_command(position(target), 0)
        ScriptedDashboard.delay_s = 0.4
        started = time.monotonic()
        rc = bridge.emergency_stop()
        took = time.monotonic() - started
        check(rc == rhal.SKR_OK and took < 0.05, f"a stop returns at once whatever the dashboard is doing ({took * 1000:.1f} ms against a dashboard that takes 400 ms to answer)")
        check(session.writes[-1] == target + [control.CMD_HOLD] and bridge.write_command(position(target), 0) == rhal.SKR_EPERM, "with the hold on the wire and the latch set when it returns")
        check("stop" not in dashboard.commands, "and the dashboard is told afterwards, from the stop's own thread")
        bridge.close()
        check("stop" in dashboard.commands, "close waits for a stop that was asked for, so a host that closes straight after one still tells the controller")
        check(
            dashboard.timeouts.get("stop") is not None and dashboard.timeouts["stop"] <= 1.0,
            f"and the stop's request carries a timeout of its own, a second at most, and not the session's five: {dashboard.timeouts.get('stop')}",
        )
        check(
            session.pause_timeout is not None and session.pause_timeout <= 1.0,
            f"and a close does not wait out the session's five seconds for the controller to answer the pause of the exchange it is leaving: {session.pause_timeout}",
        )

        bridge, _ = opened()
        dashboard = ScriptedDashboard.made[0]
        ScriptedDashboard.delay_s = 0.3
        bridge.emergency_stop()
        check(bridge.release_stop() == rhal.SKR_OK and dashboard.commands[-2:] == ["stop", "play"], f"a release waits for a stop still being told, so the program is stopped and then played and never the other way round: {dashboard.commands}")

        reset()
        bridge, _ = opened()
        ScriptedDashboard.hang = threading.Event()
        started = time.monotonic()
        with contextlib.redirect_stderr(io.StringIO()) as said:
            rc = bridge.emergency_stop()
            took = time.monotonic() - started
            check(rc == rhal.SKR_OK and took < 0.05 and bridge.write_command(position(target), 0) == rhal.SKR_EPERM, f"a dashboard that never answers does not hold the stop ({took * 1000:.1f} ms), and the latch holds")
            ScriptedDashboard.hang.set()
            told_to_stop(bridge)
        check(ScriptedDashboard.tries["stop"] == 3, f"a stop the dashboard does not answer is asked three times: {ScriptedDashboard.tries}")
        check(said.getvalue().count("did not take `stop`") == 1 and "latch holds" in said.getvalue(), f"and said once, with what still holds: {said.getvalue().strip()[:200]}")
        reset()

        # F76, with no controller: the latch belonged to the session it was asserted in.
        bridge, _ = opened()
        bridge.emergency_stop()
        bridge.close()
        reset()
        ScriptedRtde.first = sample_of(Q0, QD0)
        rc = bridge.open(json.dumps({"host": "10.9.8.7"}))
        check(rc == rhal.SKR_OK and bridge.write_command(position(target), 0) == rhal.SKR_OK, "a stop does not outlive its session: the same bridge reopens and commands (F76)")

        # The same family from the controller's side (12.C.6): an arm a safety status had stopped is the session's reading of it, and the next
        # session has not read anything yet. A bridge that remembered would refuse the first command of a session opened on an arm that has
        # since been recovered, for no reason it could name.
        bridge, _ = opened(sample=sample_of(Q0, QD0, safety=3))
        read(bridge, sample_of(Q0, QD0, safety=3))
        check(bridge.write_command(position(target), 0) == rhal.SKR_EPERM, "a command is refused while the last read said the safety system has the arm stopped")
        bridge.close()
        reset()
        ScriptedRtde.first = sample_of(Q0, QD0)
        rc = bridge.open(json.dumps({"host": "10.9.8.7"}))
        check(rc == rhal.SKR_OK and bridge.write_command(position(target), 0) == rhal.SKR_OK, "and the next session, opened on an arm that has recovered, is not refused for the last one's reading of it")

        bridge, _ = opened()
        session, dashboard, script = ScriptedRtde.made[0], ScriptedDashboard.made[0], Sockets.made[0]
        bridge.write_command(position(target), 0)
        check(bridge.close() == rhal.SKR_OK, "close returns")
        check(
            session.writes[-1] == target + [control.CMD_HOLD] and "pause" in session.calls and session.closed and dashboard.closed and script.closed,
            "and leaves the program holding, the exchange paused, and every socket closed",
        )

        # ── what open refuses ───────────────────────────────────────────────
        reset()
        ScriptedDashboard.safety = "PROTECTIVE_STOP"
        bridge = UrBridge()
        rc = bridge.open(json.dumps({"host": "10.9.8.7"}))
        check(rc == rhal.SKR_ENODEV, "a protective stop is refused at open, with the status it read")
        check(
            not ScriptedDashboard.made[0].brought_up and not ScriptedRtde.made and not Sockets.made,
            "and nothing is powered, negotiated or uploaded: the driver does not clear what a person or a collision set",
        )
        check(ScriptedDashboard.made[0].closed, "the dashboard session it opened is closed again")
        reset()
        check(UrBridge().open("{}") == rhal.SKR_ENODEV and not ScriptedDashboard.made, "no host is refused before any connection is made")
        check(UrBridge().open("not json") == rhal.SKR_EINVAL and not ScriptedDashboard.made, "a configuration that is not JSON is refused the same way")
        for frequency, allowed in ((0.5, False), (1.0, True), (125.0, True), (500.0, True), (500.5, False), (100000.0, False), (-3.0, False)):
            reset()
            rc = UrBridge().open(json.dumps({"host": "10.9.8.7", "frequency_hz": frequency}))
            check(
                (rc == rhal.SKR_EINVAL and not ScriptedRtde.made) if not allowed else (rc != rhal.SKR_EINVAL and ScriptedRtde.made[0].frequency == frequency),
                f"a rate of {frequency} Hz is {'passed to the controller' if allowed else 'refused before the wire: RTDE will not negotiate it'}",
            )
        for step in ("negotiate 2", "setup_outputs", "start"):
            reset()
            ScriptedRtde.fail = step
            rc = UrBridge().open(json.dumps({"host": "10.9.8.7"}))
            check(
                rc == rhal.SKR_ENODEV and ScriptedRtde.made[-1].closed and ScriptedDashboard.made[0].closed,
                f"a controller that refuses `{step}` is a refusal (ENODEV) and leaves no socket open",
            )

        # ── the ratings, transcribed per model ──────────────────────────────
        # UR's tech sheets and the max-joint-torque article, read a second time: a change to any figure is a change to a
        # datasheet claim and should be made by reading the sheet, not by editing one cell to match its neighbour.
        sheet = {
            "UR3E": ([180, 180, 180, 360, 360, 360], [54, 54, 28, 9, 9, 9]),
            "UR5E": ([180] * 6, [150, 150, 150, 28, 28, 28]),
            "UR7E": ([180] * 6, [150, 150, 150, 28, 28, 28]),
            "UR10E": ([120, 120, 180, 180, 180, 180], [330, 330, 150, 54, 54, 54]),
            "UR12E": ([120, 120, 180, 180, 180, 180], [330, 330, 150, 54, 54, 54]),
            "UR16E": ([120, 120, 180, 180, 180, 180], [330, 330, 150, 54, 54, 54]),
        }
        check(set(sheet) == set(bridge_module.MODEL_RATINGS), "the driver transcribes exactly the models the sheet has here, no more")
        for model, (speeds, torques) in sheet.items():
            joints = json.loads(_with_model(model)._build_descriptor({}))["joints"]
            got_speed = [round(math.degrees(j["limits"]["velocity_max_rad_s"])) for j in joints]
            got_torque = [j["limits"]["effort_max_nm"] for j in joints]
            check(got_speed == speeds and got_torque == [float(t) for t in torques], f"{model}: every joint carries its own speed and torque rating: {got_speed} / {got_torque}")
    finally:
        bridge_module.Rtde, bridge_module.Dashboard, bridge_module.socket, bridge_module.time = saved

    section("rtde — framing and interleaved text, against a socket that says what it was sent")

    class Loop:
        """A socket whose receive side is scripted and whose send side is kept."""

        def __init__(self, incoming: list) -> None:
            self.incoming, self.sent = list(incoming), b""

        def settimeout(self, _timeout: float) -> None:
            pass

        def sendall(self, data: bytes) -> None:
            self.sent += data

        def recv(self, _size: int) -> bytes:
            if not self.incoming:
                return b""
            return self.incoming.pop(0)

    def frame(packet_type: int, payload: bytes = b"") -> bytes:
        return struct.pack(">HB", 3 + len(payload), packet_type) + payload

    def session_over(incoming: list) -> tuple:
        wire = Loop(incoming)
        session = rtde.Rtde.__new__(rtde.Rtde)
        session._socket, session._buffer, session.messages = wire, b"", []
        return session, wire

    for payload in (b"", b"\x00\x02", b"x" * 40):
        session, wire = session_over([frame(rtde.START, b"\x01")])
        session.transact(rtde.START, payload)
        size, kind = struct.unpack(">HB", wire.sent[:3])
        check(
            size == len(wire.sent) == 3 + len(payload) and kind == rtde.START and wire.sent[3:] == payload,
            f"a request with {len(payload)} payload byte(s) declares its own length, header included ({size})",
        )
    session, _ = session_over([frame(rtde.TEXT_MESSAGE, b"Safety Setup has not been confirmed") + frame(rtde.START, b"\x01")])
    check(session.transact(rtde.START) == b"\x01", "a reply is read past a text message the controller put in front of it")
    check(len(session.messages) == 1 and "Safety Setup" in session.messages[0], "and the text is kept, not dropped")
    session, _ = session_over([frame(rtde.TEXT_MESSAGE, b"one") + frame(rtde.PAUSE, b"\x01")])
    try:
        session.transact(rtde.START)
        check(False, "a reply of the wrong type is refused")
    except rtde.RtdeError as error:
        check("asked for" in str(error) and "answered" in str(error), "a reply of a type that was not asked for is refused, by name")
    session, _ = session_over([frame(rtde.TEXT_MESSAGE, b"chatter")] * 70)
    try:
        session.transact(rtde.START)
        check(False, "a controller that only talks is given up on")
    except rtde.RtdeError as error:
        check("nothing but text" in str(error), "a controller that sends nothing but text is given up on, not waited for forever")
    session, _ = session_over([struct.pack(">HB", 2, rtde.START)])
    try:
        session._packet()
        check(False, "a packet that cannot hold its own header is refused")
    except rtde.RtdeError as error:
        check("cannot hold its own header" in str(error), "a packet claiming fewer bytes than its own header is refused")

    # A setpoint is sent from the control loop, so it is bounded: a controller that has stopped reading fills the socket's buffer, and a send that waited out the socket's five seconds would hold the loop for them.
    # A real socket pair with small buffers and nobody reading one end; the socket's own timeout here (one second) is only what keeps a driver without the bound from hanging this suite.
    import socket as real_socket

    sender, receiver = real_socket.socketpair()
    sender.setsockopt(real_socket.SOL_SOCKET, real_socket.SO_SNDBUF, 4096)
    receiver.setsockopt(real_socket.SOL_SOCKET, real_socket.SO_RCVBUF, 4096)
    sender.settimeout(1.0)
    session = rtde.Rtde.__new__(rtde.Rtde)
    session._socket, session._buffer, session.messages, session._timeout_s = sender, b"", [], 1.0
    session.input = rtde.Recipe(2, list(INPUTS), ["DOUBLE"] * 6 + ["INT32"])
    worst, sent, failure = 0.0, 0, None
    give_up = time.monotonic() + 8.0
    while failure is None and time.monotonic() < give_up:
        began = time.monotonic()
        try:
            session.write([0.1] * 6 + [1])
            sent += 1
        except (rtde.RtdeError, OSError) as error:
            failure = error
        worst = max(worst, time.monotonic() - began)
    check(
        failure is not None and worst < 0.2,
        f"a setpoint sent into a socket nobody reads fails within a bound once its buffer is full ({sent} packets went; the slowest send took {worst * 1000:.0f} ms), where it waited out the socket's own timeout",
    )
    try:
        session.write([0.1] * 6 + [1])
        refused_by = "it was sent"
    except (rtde.RtdeError, OSError) as error:
        refused_by = str(error)
    check("out of step" in refused_by, f"and the session refuses setpoints from then on, by name, because a packet that was only partly sent leaves the stream out of step: {refused_by[:90]}")
    sender.close()
    receiver.close()

    section("control — the program reads the registers the driver writes")
    written = sorted(int(name.rsplit("_", 1)[1]) for name in INPUTS if name.startswith("input_double_register_"))
    read_floats = sorted(int(n) for n in re.findall(r"read_input_float_register\((\d+)\)", control.SERVO_PROGRAM))
    check(read_floats == written == list(range(6)), f"the program reads float registers {read_floats}, the driver writes {written}")
    gate = [name for name in INPUTS if name.startswith("input_int_register_")]
    check(
        gate == [f"input_int_register_{control.COMMAND_REGISTER}"]
        and re.findall(r"read_input_integer_register\((\d+)\)", control.SERVO_PROGRAM) == [str(control.COMMAND_REGISTER)],
        "and its gate is the integer register the driver writes last",
    )
    check(INPUTS[-1] == gate[0] and INPUTS[:6] == [f"input_double_register_{i}" for i in range(6)], "in the order the write puts them: six setpoints, then the gate")


def _with_model(model: str) -> UrBridge:
    bridge = UrBridge()
    bridge._model = model
    return bridge


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

    # F76: the latch outlived its session, so the same bridge opened again after a stop refused
    # every command for the life of the process. Opened **at once**, with no retry here: the
    # registers the closed session held are the driver's to wait out (`REGISTER_RELEASE_S`), and
    # this suite waited them out for it — a second a try, for fifteen — until `skynet driver check`
    # began opening a second session and the Rust port, which has no interpreter to start first, was
    # refused on it (F81).
    rc = bridge.open(json.dumps({"host": host, "simulator": True}))
    check(rc == rhal.SKR_OK, "the same bridge opens again at once after the stop and a close")
    hold = rhal.Command(mode=rhal.MODE_POSITION, values=[0.0] * 6, mask=[0] * 6)
    check(
        bridge.write_command(hold, 0) == rhal.SKR_OK,
        "and commands again: the latch belonged to the session it was asserted in (F76)",
    )
    check(bridge.close() == rhal.SKR_OK, "close again")

    section("bridge — what it refuses")
    check(UrBridge().open("{}") == rhal.SKR_ENODEV, "a configuration with no host is refused")
    check(UrBridge().open("not json") == rhal.SKR_EINVAL, "and one that is not JSON is refused")
    # The control for the two above. RTDE's general-purpose registers are allocated to **one session
    # at a time**, and a controller frees a closed session's a moment after its socket goes — about
    # 60 ms on URSim 5.26. The suite first met this as `IN_USE` on a recipe that had worked moments
    # earlier, and the refusal named the wrong cause ("this controller has no such variable"); the
    # message was fixed then, and the wait moved into the driver when a second session opened at
    # once was refused (F81). So this opens once, straight after the session above closed.
    rc = UrBridge().open('{"host": "%s", "frequency_hz": 125}' % host)
    check(rc == rhal.SKR_OK, "and the same host comes up again at once, the registers waited out")


def main() -> int:
    print("skynet-ur selftest\n")
    pure()
    bridge_logic()

    host = os.environ.get("SKYNET_UR_HOST", "").strip()
    if host:
        against_ursim(host)
    else:
        section("bridge — not exercised")
        print(
            "  ----  SKYNET_UR_HOST is not set, so the bridge ran against a scripted controller, not a real one.\n"
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

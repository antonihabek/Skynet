"""Golden RTDE vectors for `drivers/ur_rtde-rs`, generated from the Python driver.

The Python driver is the executable specification (DRY-COVERAGE-PLAN.md 6.6), and it was itself
measured against URSim, so the Rust port is held to what *it* sends and parses. The Python session
is driven here through a scripted socket — the requests it writes are recorded, and the replies it
reads are the controller's, framed as URSim frames them — so every request below is the bytes
`skynet_ur.rtde` actually produced, and every parsed value is what it actually read:

    cd drivers/ur_rtde-rs/tests
    PYTHONPATH=../../ur_rtde:../../../bindings/python python3 gen_vectors.py > vectors.json

Nothing is typed in as an expected byte.
"""

from __future__ import annotations

import json
import struct

from skynet_ur import control, rtde
from skynet_ur.bridge import INPUTS, MODEL_RATINGS, OUTPUTS, UrBridge

#: The types URSim 5.26 answers for this driver's recipes — what the controller says, not what
#: the driver asks for.
OUTPUT_TYPES = "VECTOR6D,VECTOR6D,VECTOR6D,INT32,INT32,UINT32"
INPUT_TYPES = "DOUBLE,DOUBLE,DOUBLE,DOUBLE,DOUBLE,DOUBLE,INT32"

#: What a fresh URSim says in the middle of the handshake.
SAFETY_TEXT = b"Safety Setup has not been confirmed yet, therefore no data will be send yet"


class Scripted:
    """A socket that records what is sent and replays one scripted byte stream."""

    def __init__(self, replies: bytes) -> None:
        self.sent = bytearray()
        self._replies = bytearray(replies)

    def sendall(self, data: bytes) -> None:
        self.sent += data

    def settimeout(self, _timeout: float) -> None:
        pass

    def recv(self, size: int) -> bytes:
        if not self._replies:
            raise BlockingIOError
        chunk = bytes(self._replies[:size])
        del self._replies[:size]
        return chunk


def packet(packet_type: int, body: bytes = b"") -> bytes:
    return struct.pack(">HB", 3 + len(body), packet_type) + body


def session(replies: bytes) -> rtde.Rtde:
    s = rtde.Rtde.__new__(rtde.Rtde)
    s._socket = Scripted(replies)
    s._buffer = b""
    s.output = None
    s.input = None
    s.messages = []
    return s


def exchange(name: str, replies: bytes, act) -> dict:
    s = session(replies)
    try:
        result = act(s)
        error = None
    except rtde.RtdeError as refusal:
        result = None
        error = str(refusal)
    return {
        "name": name,
        "replies": replies.hex(),
        "sent": bytes(s._socket.sent).hex(),
        "result": result,
        "error": error,
        "messages": s.messages,
    }


def main() -> None:
    out: dict = {
        "note": (
            "Golden vectors from skynet_ur (rtde.py, control.py, bridge.py). Regenerate with "
            "tests/gen_vectors.py; never edit by hand."
        ),
        "outputs": OUTPUTS,
        "inputs": INPUTS,
        "exchanges": [],
    }

    def negotiate(s: rtde.Rtde):
        s.negotiate(2)
        return True

    def version(s: rtde.Rtde):
        return list(s.controller_version())

    def outputs(s: rtde.Rtde):
        recipe = s.setup_outputs(OUTPUTS, 125.0)
        return {"id": recipe.identifier, "types": recipe.types}

    def inputs(s: rtde.Rtde):
        recipe = s.setup_inputs(INPUTS)
        return {"id": recipe.identifier, "types": recipe.types}

    def start(s: rtde.Rtde):
        s.start()
        return True

    exchanges = out["exchanges"]
    exchanges.append(
        exchange(
            "negotiate_past_a_text_message",
            packet(rtde.TEXT_MESSAGE, SAFETY_TEXT) + packet(rtde.REQUEST_PROTOCOL_VERSION, b"\x01"),
            negotiate,
        )
    )
    exchanges.append(
        exchange("negotiate_refused", packet(rtde.REQUEST_PROTOCOL_VERSION, b"\x00"), negotiate)
    )
    exchanges.append(
        exchange(
            "controller_version",
            packet(rtde.GET_URCONTROL_VERSION, struct.pack(">IIII", 5, 26, 1, 0)),
            version,
        )
    )
    exchanges.append(
        exchange(
            "setup_outputs",
            packet(rtde.SETUP_OUTPUTS, bytes([1]) + OUTPUT_TYPES.encode()),
            outputs,
        )
    )
    exchanges.append(
        exchange(
            "setup_outputs_in_use",
            packet(
                rtde.SETUP_OUTPUTS,
                bytes([1]) + b"VECTOR6D,VECTOR6D,IN_USE,INT32,INT32,UINT32",
            ),
            outputs,
        )
    )
    exchanges.append(
        exchange(
            "setup_inputs",
            packet(rtde.SETUP_INPUTS, bytes([2]) + INPUT_TYPES.encode()),
            inputs,
        )
    )
    exchanges.append(exchange("start", packet(rtde.START, b"\x01"), start))
    exchanges.append(exchange("start_refused", packet(rtde.START, b"\x00"), start))

    # The setpoint writes: a servo command and the hold `emergency_stop` and `close` send.
    setpoint = [0.1, -1.2, 1.5, -0.3, 1.57, -3.0]
    writes = []
    for name, gate in (("servo", control.CMD_SERVO), ("hold", control.CMD_HOLD)):
        s = session(b"")
        s.input = rtde.Recipe(2, INPUTS, INPUT_TYPES.split(","))
        s.write(setpoint + [gate])
        writes.append(
            {"name": name, "recipe": 2, "setpoint": setpoint, "gate": gate,
             "sent": bytes(s._socket.sent).hex()}
        )
    out["writes"] = writes

    # The output stream: two data packages with a text message between, and the drain keeps the
    # newest — the second.
    types = OUTPUT_TYPES.split(",")
    samples = []
    for step in (0, 1):
        samples.append(
            [
                [0.1 * step + j for j in range(6)],
                [0.01 * step - j for j in range(6)],
                [0.5, -0.5, 0.25, 0.0, 1.0, -1.0],
                7,
                1 + step,
                2,
            ]
        )
    stream = b"".join(
        [
            packet(rtde.DATA_PACKAGE, bytes([1]) + rtde.encode(types, samples[0])),
            packet(rtde.TEXT_MESSAGE, b"a message between samples"),
            packet(rtde.DATA_PACKAGE, bytes([1]) + rtde.encode(types, samples[1])),
            packet(rtde.DATA_PACKAGE, bytes([9]) + b"\x00"),  # another recipe's, skipped
        ]
    )
    s = session(stream)
    s.output = rtde.Recipe(1, OUTPUTS, types)
    newest = s.newest()
    out["stream"] = {"bytes": stream.hex(), "recipe": 1, "newest": newest, "messages": s.messages}

    out["program"] = control.SERVO_PROGRAM
    out["period_s"] = control.SERVO_PERIOD_S

    descriptors = []
    for model in [None, *MODEL_RATINGS]:
        for simulator in (True, False):
            bridge = UrBridge()
            bridge._model = model
            bridge._simulator = simulator
            descriptors.append(
                {"model": model, "simulator": simulator, "descriptor": bridge._build_descriptor({})}
            )
    out["descriptors"] = descriptors

    print(json.dumps(out, indent=2))


if __name__ == "__main__":
    main()

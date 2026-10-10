"""The Tello's wire: the text commands of the SDK 2.0 User Guide (V1.0, 2018.11) and the state string it streams (DRY-COVERAGE-PLAN.md item 12.E.11).

Pure functions and tables, standard library only. **The guide's units are centimetres, degrees, centimetres a second and percent**, and so are this module's; the bridge converts at its edge. A command the wire cannot say is
refused here and never rounded: every number of a command is a whole number (the guide gives every range as whole numbers), and a request for 45.5 cm is not a request for 45 or 46.
"""

from __future__ import annotations

from dataclasses import dataclass, field

__all__ = [
    "ProtocolError", "COMMAND_PORT", "STATE_PORT", "FACTORY_HOST", "DIRECTIONS", "FLIPS", "MOVE_CM", "ROTATE_DEG", "SPEED_CM_S", "RC_RANGE", "READS", "Reply", "State", "STATE_INTS", "STATE_FLOATS",
    "command", "takeoff", "land", "emergency", "stop", "move", "rotate", "flip", "speed", "rc", "read", "parse_reply", "parse_state", "encode_state",
]

COMMAND_PORT = 8889
STATE_PORT = 8890
#: "Tello IP: 192.168.10.1" (Send Command & Receive Response).
FACTORY_HOST = "192.168.10.1"

DIRECTIONS = ("up", "down", "left", "right", "forward", "back")
FLIPS = ("l", "r", "f", "b")
#: The ranges the guide gives: `up x` ... `back x` 20 to 500 cm, `cw x` and `ccw x` 1 to 360 degrees, `speed x` 10 to 100 cm/s, `rc a b c d` -100 to 100 each.
MOVE_CM = (20, 500)
ROTATE_DEG = (1, 360)
SPEED_CM_S = (10, 100)
RC_RANGE = (-100, 100)

#: The read commands of the guide, by what they read: "Obtain current speed (cm/s)", "...battery percentage", "...flight time", "Wi-Fi SNR", "the Tello SDK version", "the serial number".
READS = {"speed": "speed?", "battery": "battery?", "time": "time?", "wifi": "wifi?", "sdk": "sdk?", "serial": "sn?"}


class ProtocolError(Exception):
    """A command the guide does not allow, or a reply or a state string that is not the Tello's."""


def _whole(name: str, value: object, low: int, high: int) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise ProtocolError(f"{name} of {value!r} is not a whole number: the Tello takes whole numbers and a request is not rounded.")
    if not low <= value <= high:
        raise ProtocolError(f"{name} of {value} is not from {low} to {high}.")
    return value


# ── the commands ─────────────────────────────────────────────────────────────────────────────────

def command() -> str:
    """`command`: "Enter SDK mode", the first thing sent, before which the Tello takes no other."""
    return "command"


def takeoff() -> str:
    return "takeoff"


def land() -> str:
    return "land"


def emergency() -> str:
    """`emergency`: "Stop motors immediately": the motors are cut, and a Tello in the air falls."""
    return "emergency"


def stop() -> str:
    """`stop`: "Hovers in the air. Note: works at any time."."""
    return "stop"


def move(direction: str, cm: int) -> str:
    """`up x`, `down x`, `left x`, `right x`, `forward x` or `back x`: "x" from 20 to 500 cm."""
    if direction not in DIRECTIONS:
        raise ProtocolError(f"{direction!r} is not a direction ({', '.join(DIRECTIONS)}).")
    return f"{direction} {_whole('a distance', cm, *MOVE_CM)}"


def rotate(clockwise: bool, degrees: int) -> str:
    """`cw x` or `ccw x`: "x" from 1 to 360 degrees."""
    return f"{'cw' if clockwise else 'ccw'} {_whole('an angle', degrees, *ROTATE_DEG)}"


def flip(direction: str) -> str:
    """`flip x`: l, r, f or b."""
    if direction not in FLIPS:
        raise ProtocolError(f"{direction!r} is not a flip ({', '.join(FLIPS)}).")
    return f"flip {direction}"


def speed(cm_s: int) -> str:
    """`speed x`: "Set speed to x cm/s", 10 to 100."""
    return f"speed {_whole('a speed', cm_s, *SPEED_CM_S)}"


def rc(left_right: int, forward_back: int, up_down: int, yaw: int) -> str:
    """`rc a b c d`: left/right, forward/backward, up/down and yaw, each from -100 to 100."""
    values = [_whole(name, value, *RC_RANGE) for name, value in (("left/right", left_right), ("forward/backward", forward_back), ("up/down", up_down), ("yaw", yaw))]
    return "rc " + " ".join(str(value) for value in values)


def read(what: str) -> str:
    if what not in READS:
        raise ProtocolError(f"{what!r} is not something the guide's read commands read ({', '.join(READS)}).")
    return READS[what]


# ── what the Tello answers ───────────────────────────────────────────────────────────────────────

@dataclass(frozen=True)
class Reply:
    kind: str       # "ok", "error" or "value"
    text: str       # the reply, without its line end


def parse_reply(data: bytes) -> Reply:
    """A reply datagram. `ok` is the guide's "Returns ok if the command was successful"; `error` and anything that says an error or an unknown command is the guide's "error or an informational result code"; the rest is the value of a read
    command. Never raises: bytes that are not text are an error reply."""
    try:
        text = data.decode("utf-8").rstrip("\r\n").strip()
    except UnicodeDecodeError:
        return Reply("error", "a reply that is not text")
    lowered = text.lower()
    if lowered == "ok":
        return Reply("ok", text)
    if lowered.startswith("error") or "unknown command" in lowered or "out of range" in lowered:
        return Reply("error", text)
    return Reply("value", text)


# ── the state string ─────────────────────────────────────────────────────────────────────────────

#: "pitch:%d;roll:%d;yaw:%d;vgx:%d;vgy%d;vgz:%d;templ:%d;temph:%d;tof:%d;h:%d;bat:%d;baro:%.2f; time:%d;agx:%.2f;agy:%.2f;agz:%.2f;\r\n". **The guide's `vgy%d` has no colon; the string the Tello sends does**
#: (every other client reads `vgy:`), and this parser reads the colon.
STATE_INTS = ("pitch", "roll", "yaw", "vgx", "vgy", "vgz", "templ", "temph", "tof", "h", "bat", "time")
STATE_FLOATS = ("baro", "agx", "agy", "agz")
_PAD_INTS = ("mid", "x", "y", "z")


@dataclass(frozen=True)
class State:
    pitch: int          # degrees
    roll: int           # degrees
    yaw: int            # degrees
    vgx: int            # "the speed of x axis": the guide gives no unit
    vgy: int
    vgz: int
    templ: int          # the lowest temperature, degrees Celsius
    temph: int          # the highest temperature, degrees Celsius
    tof: int            # "the time of flight distance in cm"
    h: int              # "the height in cm"
    bat: int            # percent
    baro: float         # "the barometer measurement in cm"
    time: int           # "the amount of time the motor has been used"
    agx: float          # accelerations: the guide gives no unit
    agy: float
    agz: float
    pad: tuple[int, int, int, int] | None = None        # mid, x, y, z, with a mission pad's detection on
    extra: dict = field(default_factory=dict)           # fields of a firmware newer than the guide (`mpry`, ...), as text


def parse_state(data: bytes | str) -> State:
    """One state string, or ProtocolError for text that is not one: a field the guide lists that is absent or is not a number, or a string with no fields at all."""
    try:
        text = data.decode("ascii") if isinstance(data, (bytes, bytearray)) else data
    except UnicodeDecodeError as error:
        raise ProtocolError("a state string that is not ASCII.") from error
    fields: dict[str, str] = {}
    for part in text.strip().split(";"):
        key, sep, value = part.partition(":")
        if sep:
            fields[key.strip()] = value.strip()
    if not fields:
        raise ProtocolError(f"{text.strip()[:60]!r} is not a state string.")
    numbers: dict[str, int | float] = {}
    for name in STATE_INTS + STATE_FLOATS:
        if name not in fields:
            raise ProtocolError(f"the state string has no `{name}`.")
        try:
            numbers[name] = int(fields.pop(name)) if name in STATE_INTS else float(fields.pop(name))
        except ValueError:
            raise ProtocolError(f"the state string's `{name}` is not a number.") from None
    pad = None
    if "mid" in fields:
        try:
            pad = tuple(int(fields.pop(name)) for name in _PAD_INTS)
        except (KeyError, ValueError):
            raise ProtocolError("the state string's mission pad fields are not whole numbers.") from None
    return State(pad=pad, extra=fields, **numbers)    # type: ignore[arg-type]


def encode_state(state: State) -> str:
    """The string the Tello streams, for the simulator and the tests."""
    head = "" if state.pad is None else "mid:%d;x:%d;y:%d;z:%d;" % state.pad
    return (head + "pitch:%d;roll:%d;yaw:%d;vgx:%d;vgy:%d;vgz:%d;templ:%d;temph:%d;tof:%d;h:%d;bat:%d;baro:%.2f;time:%d;agx:%.2f;agy:%.2f;agz:%.2f;\r\n"
            % (state.pitch, state.roll, state.yaw, state.vgx, state.vgy, state.vgz, state.templ, state.temph, state.tof, state.h, state.bat, state.baro, state.time, state.agx, state.agy, state.agz))

"""CiA 402's drive profile: the state machine, the object dictionary this driver touches, and units.

Pure, like :mod:`skynet_canopen.frames`. The state machine is the part of CANopen motion control
that everybody re-implements and nobody agrees about, so it is written once here and used by both
ends — the bridge that drives a real motor and the simulator that stands in for one. Two copies of
this table is a drive that the driver believes is enabled and that is actually in `switch on
disabled`, which presents as a joint that reports its position perfectly and does not move.

# The states, and how a statusword is read back to one

CiA 402 encodes eight states in six bits of the statusword (`0x6041`), and the encoding is **not** a
plain enumeration: two of the states are matched with bit 5 *masked out*, because quick-stop is
inactive-high and a drive in `fault` has no meaningful value for it.

| State | Mask | Value |
|---|---|---|
| Not ready to switch on | `0x4F` | `0x00` |
| Switch on disabled | `0x4F` | `0x40` |
| Ready to switch on | `0x6F` | `0x21` |
| Switched on | `0x6F` | `0x23` |
| Operation enabled | `0x6F` | `0x27` |
| Quick stop active | `0x6F` | `0x07` |
| Fault reaction active | `0x4F` | `0x0F` |
| Fault | `0x4F` | `0x08` |

Order matters when matching, because the narrower masks would swallow the wider ones: `0x4F & 0x27`
is `0x07`, so `operation enabled` must be tested before `fault reaction active`. :func:`state_of`
tests the `0x6F` rows first for that reason, and the selftest asserts every row round-trips.

# Units, and the number this driver refuses to guess

CiA 402 works in **increments**. A position of 131072 means nothing until you know the encoder's
resolution and the gear ratio, and the objects that carry them — `0x608F` and `0x6091` — are
optional in the profile. A driver that assumed 65536 counts per revolution because most drives use
it would report a joint angle that is wrong by whatever factor the real drive uses, in radians, to a
Safety Kernel that bounds travel with it.

So :func:`increments_to_rad` takes the resolution as an argument and there is no default. The bridge
reads both objects at open and **refuses the body** if a drive will not report them, which is §5.2's
rule applied one layer down: a limit with no provenance is a rumour, and so is a unit.
"""

from __future__ import annotations

from dataclasses import dataclass
from enum import Enum

__all__ = [
    "State",
    "state_of",
    "statusword_for",
    "Command",
    "command_of",
    "next_state",
    "controlword_for",
    "Mode",
    "OD",
    "increments_to_rad",
    "rad_to_increments",
    "FAULT_REACTION_STATES",
]


class State(Enum):
    """CiA 402's eight, in the profile's own words."""

    NOT_READY_TO_SWITCH_ON = "not_ready_to_switch_on"
    SWITCH_ON_DISABLED = "switch_on_disabled"
    READY_TO_SWITCH_ON = "ready_to_switch_on"
    SWITCHED_ON = "switched_on"
    OPERATION_ENABLED = "operation_enabled"
    QUICK_STOP_ACTIVE = "quick_stop_active"
    FAULT_REACTION_ACTIVE = "fault_reaction_active"
    FAULT = "fault"


#: The two states in which the drive is bringing itself to a stop or has faulted. A command is not
#: applied in either, and the distinction from `switch on disabled` is that a person has to clear
#: these before anything moves.
FAULT_REACTION_STATES = (State.FAULT_REACTION_ACTIVE, State.FAULT)

#: `(mask, value, state)`, in the order they must be tested. See the module header.
_DECODE: tuple[tuple[int, int, State], ...] = (
    (0x6F, 0x27, State.OPERATION_ENABLED),
    (0x6F, 0x23, State.SWITCHED_ON),
    (0x6F, 0x21, State.READY_TO_SWITCH_ON),
    (0x6F, 0x07, State.QUICK_STOP_ACTIVE),
    (0x4F, 0x0F, State.FAULT_REACTION_ACTIVE),
    (0x4F, 0x08, State.FAULT),
    (0x4F, 0x40, State.SWITCH_ON_DISABLED),
    (0x4F, 0x00, State.NOT_READY_TO_SWITCH_ON),
)

#: One statusword per state, for a device that has to *produce* one. Bit 4 (voltage enabled) is set
#: wherever the profile allows it, and bit 5 (quick stop, inactive-high) is set everywhere quick stop
#: is *not* active — the inversion that makes the decode table above need two masks.
_ENCODE: dict[State, int] = {
    State.NOT_READY_TO_SWITCH_ON: 0x0000,
    State.SWITCH_ON_DISABLED: 0x0040,
    State.READY_TO_SWITCH_ON: 0x0021,
    State.SWITCHED_ON: 0x0023,
    State.OPERATION_ENABLED: 0x0027,
    State.QUICK_STOP_ACTIVE: 0x0007,
    State.FAULT_REACTION_ACTIVE: 0x000F,
    State.FAULT: 0x0008,
}


def state_of(statusword: int) -> State:
    """Which state a statusword reports."""
    for mask, value, state in _DECODE:
        if statusword & mask == value:
            return state
    # Unreachable: the last row masks to zero and matches everything the rows above did not.
    raise ValueError(f"statusword {statusword:#06x} matches no CiA 402 state")


def statusword_for(state: State, *, voltage: bool = True, target_reached: bool = False) -> int:
    """A statusword a device in ``state`` would publish.

    ``voltage`` is bit 4, and it is an argument rather than a constant because a drive with the DC
    bus down reports `switch on disabled` **without** it — which is the difference between "somebody
    disabled this axis" and "the contactor is open", and an integrator chasing the second one at
    three in the morning will want the bit.
    """
    word = _ENCODE[state]
    if voltage and state not in FAULT_REACTION_STATES:
        word |= 0x0010
    if target_reached:
        word |= 0x0400
    return word


class Command(Enum):
    """The six controlword commands, plus the reset that is an edge rather than a value."""

    SHUTDOWN = "shutdown"
    SWITCH_ON = "switch_on"
    DISABLE_VOLTAGE = "disable_voltage"
    QUICK_STOP = "quick_stop"
    DISABLE_OPERATION = "disable_operation"
    ENABLE_OPERATION = "enable_operation"
    FAULT_RESET = "fault_reset"


#: `(mask, value, command)`. Tested in order, and the order is again load-bearing: `disable voltage`
#: masks only two bits and matches almost everything, so it is tested last.
_COMMANDS: tuple[tuple[int, int, Command], ...] = (
    (0x8F, 0x0F, Command.ENABLE_OPERATION),
    (0x8F, 0x07, Command.SWITCH_ON),  # also `disable operation`; see `next_state`
    (0x87, 0x06, Command.SHUTDOWN),
    (0x86, 0x02, Command.QUICK_STOP),
    (0x82, 0x00, Command.DISABLE_VOLTAGE),
)

_CONTROLWORDS: dict[Command, int] = {
    Command.SHUTDOWN: 0x0006,
    Command.SWITCH_ON: 0x0007,
    Command.DISABLE_OPERATION: 0x0007,
    Command.ENABLE_OPERATION: 0x000F,
    Command.DISABLE_VOLTAGE: 0x0000,
    Command.QUICK_STOP: 0x0002,
    Command.FAULT_RESET: 0x0080,
}


def command_of(controlword: int, previous: int) -> Command | None:
    """Which command a controlword carries, given the one before it.

    ``previous`` is needed for exactly one of the seven. **Fault reset is a rising edge of bit 7**,
    not a level: a master that held bit 7 set would clear every fault the instant it appeared, which
    turns a latched fault into a drive that restarts itself in a loop with nobody told.
    """
    if controlword & 0x80 and not previous & 0x80:
        return Command.FAULT_RESET
    for mask, value, command in _COMMANDS:
        if controlword & mask == value:
            return command
    return None


def next_state(state: State, command: Command) -> State:
    """The profile's transition table. A command with no transition from here leaves the state alone.

    `switch on` and `disable operation` are the same controlword (`0x0007`) and differ only by where
    they are issued from, which is why this takes the state as well and why :func:`command_of` does
    not try to tell them apart.
    """
    if command is Command.FAULT_RESET:
        return State.SWITCH_ON_DISABLED if state is State.FAULT else state
    if command is Command.DISABLE_VOLTAGE:
        return State.SWITCH_ON_DISABLED if state not in FAULT_REACTION_STATES else state
    if command is Command.QUICK_STOP:
        if state in (State.READY_TO_SWITCH_ON, State.SWITCHED_ON, State.OPERATION_ENABLED):
            return State.QUICK_STOP_ACTIVE
        return state
    if command is Command.SHUTDOWN:
        if state in (
            State.SWITCH_ON_DISABLED,
            State.SWITCHED_ON,
            State.OPERATION_ENABLED,
            State.READY_TO_SWITCH_ON,
        ):
            return State.READY_TO_SWITCH_ON
        return state
    if command is Command.SWITCH_ON:
        if state is State.READY_TO_SWITCH_ON:
            return State.SWITCHED_ON
        if state is State.OPERATION_ENABLED:
            return State.SWITCHED_ON  # the same word read as `disable operation`
        return state
    if command is Command.ENABLE_OPERATION:
        if state in (State.SWITCHED_ON, State.OPERATION_ENABLED):
            return State.OPERATION_ENABLED
        return state
    return state


def controlword_for(command: Command) -> int:
    """The controlword that carries a command. `FAULT_RESET` is bit 7 and needs an edge."""
    return _CONTROLWORDS[command]


class Mode(Enum):
    """Modes of operation (`0x6060`), limited to the three this driver maps to §24's.

    Cyclic synchronous, not profile. §7's loop produces a setpoint every cycle and the profile modes
    would have the drive generate its own trajectory between them — two interpolators, one of which
    does not know the Safety Kernel exists.
    """

    CSP = 8
    CSV = 9
    CST = 10


@dataclass(frozen=True)
class Object:
    """One entry of the object dictionary, with the width an expedited SDO needs."""

    index: int
    subindex: int
    size: int
    signed: bool
    name: str


class OD:
    """The objects this driver touches, and nothing else."""

    CONTROLWORD = Object(0x6040, 0, 2, False, "controlword")
    STATUSWORD = Object(0x6041, 0, 2, False, "statusword")
    MODES_OF_OPERATION = Object(0x6060, 0, 1, True, "modes of operation")
    MODES_DISPLAY = Object(0x6061, 0, 1, True, "modes of operation display")
    POSITION_ACTUAL = Object(0x6064, 0, 4, True, "position actual value")
    VELOCITY_ACTUAL = Object(0x606C, 0, 4, True, "velocity actual value")
    TORQUE_ACTUAL = Object(0x6077, 0, 2, True, "torque actual value")
    TARGET_POSITION = Object(0x607A, 0, 4, True, "target position")
    TARGET_VELOCITY = Object(0x60FF, 0, 4, True, "target velocity")
    TARGET_TORQUE = Object(0x6071, 0, 2, True, "target torque")
    MOTOR_RATED_TORQUE = Object(0x6076, 0, 4, False, "motor rated torque")
    POSITION_LIMIT_MIN = Object(0x607D, 1, 4, True, "software position limit, minimum")
    POSITION_LIMIT_MAX = Object(0x607D, 2, 4, True, "software position limit, maximum")
    MAX_MOTOR_SPEED = Object(0x6080, 0, 4, False, "max motor speed")
    ENCODER_INCREMENTS = Object(0x608F, 1, 4, False, "position encoder resolution, increments")
    ENCODER_REVOLUTIONS = Object(0x608F, 2, 4, False, "position encoder resolution, revolutions")
    GEAR_MOTOR_REVOLUTIONS = Object(0x6091, 1, 4, False, "gear ratio, motor revolutions")
    GEAR_SHAFT_REVOLUTIONS = Object(0x6091, 2, 4, False, "gear ratio, shaft revolutions")
    #: What the master maps into TPDO2/RPDO2, read back to check the drive agrees (see `frames`).
    TPDO2_MAPPING_COUNT = Object(0x1A01, 0, 1, False, "TPDO2 mapping, entry count")
    TPDO2_MAPPING_1 = Object(0x1A01, 1, 4, False, "TPDO2 mapping, first entry")
    TPDO2_MAPPING_2 = Object(0x1A01, 2, 4, False, "TPDO2 mapping, second entry")
    RPDO2_MAPPING_COUNT = Object(0x1601, 0, 1, False, "RPDO2 mapping, entry count")
    RPDO2_MAPPING_1 = Object(0x1601, 1, 4, False, "RPDO2 mapping, first entry")
    RPDO2_MAPPING_2 = Object(0x1601, 2, 4, False, "RPDO2 mapping, second entry")


def _counts_per_shaft_turn(
    increments: int, revolutions: int, gear_motor: int, gear_shaft: int
) -> float:
    if revolutions <= 0 or gear_shaft <= 0:
        raise ValueError(
            "a zero in an encoder resolution or a gear ratio denominator is not a ratio. "
            "Refusing rather than dividing: the result would be a joint angle nobody measured."
        )
    return (increments / revolutions) * (gear_motor / gear_shaft)


def increments_to_rad(
    increments_value: int, increments: int, revolutions: int, gear_motor: int, gear_shaft: int
) -> float:
    """Encoder increments to radians of the **output shaft**.

    Both ratios, both from the drive. `0x608F` gives increments per motor revolution and `0x6091`
    gives motor revolutions per shaft revolution, and a driver that used only the first reports the
    motor's angle as the joint's — wrong by the gearbox, which on an industrial axis is a factor of
    fifty to a hundred.
    """
    per_turn = _counts_per_shaft_turn(increments, revolutions, gear_motor, gear_shaft)
    import math

    return (increments_value / per_turn) * 2.0 * math.pi


def rad_to_increments(
    radians: float, increments: int, revolutions: int, gear_motor: int, gear_shaft: int
) -> int:
    """The inverse, rounded half away from zero.

    Rounded rather than truncated, and away from zero rather than to even, because a truncation
    biases every setpoint toward zero by up to one count and a control loop integrates a bias.
    """
    import math

    per_turn = _counts_per_shaft_turn(increments, revolutions, gear_motor, gear_shaft)
    exact = radians / (2.0 * math.pi) * per_turn
    return int(math.floor(exact + 0.5)) if exact >= 0 else int(math.ceil(exact - 0.5))

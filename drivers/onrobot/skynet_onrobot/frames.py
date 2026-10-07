"""The OnRobot register map: what the RG2, RG6, 2FG7 and 2FG14 keep in their holding registers, how a register becomes a physical quantity and back, and the fault bits. Pure: no I/O, no clock.

OnRobot publish no register table for these grippers in the repository this was written against. They publish a closed shared library (`libonrobot_tool_api`, BSD-3-Clause, release 1.2.0, the maker's own client)
and its documentation; every address, scale, bit and rule below was **read off that library by running it**: against a Modbus server that logs every request and answers from a table of registers the experiment
sets, one register at a time (`layout_check.py` repeats the experiments and holds this module to them; README.md says how). Where the library validates something before it writes, the same validation is here.

* RG2 and RG6: product code 32 and 33 at 1536; firmware at 1540 and 1541; a state block of twenty registers at 258..277; the control block of three registers at 0..2.
* 2FG7 and 2FG14: product code 192 and 193 at 1536; identity at 1536..1543; the device's own limits at 259..262 and 1028..1031; a state block of twenty registers at 256..275; the control block of four registers at 0..3.

Both reach the same unit address, 65, with the Quick Changer's own address for a second tool at 66 and 67.
"""

from __future__ import annotations

import math
from dataclasses import dataclass

PRODUCT_CODE_ADDRESS = 1536
FIRMWARE_ADDRESS = 1540                 # two registers: major << 8 | minor, then the build

# ── RG2 and RG6 ──────────────────────────────────────────────────────────────

RG_CONTROL_ADDRESS = 0                  # force (0.1 N), width (0.1 mm), control
RG_STATUS_ADDRESS = 258
RG_STATUS_COUNT = 20                    # 258..277
RG_CONFIG_ADDRESS = 1025                # the long-term power limit, 0.1 W
RG_CONTROL_GRIP = 16                    # "grip with the fingertip offset": the width is the task aperture at the fitted fingertips
RG_CONTROL_STOP = 40

# Status block offsets from RG_STATUS_ADDRESS (258).
_RG_OFFSET, _RG_VOLTAGE, _RG_CURRENT, _RG_DEPTH, _RG_RELATIVE_DEPTH, _RG_ANGLE, _RG_ACTUAL_WIDTH, _RG_STATUS = 0, 1, 2, 5, 6, 7, 9, 10
_RG_TEMPERATURE, _RG_WIDTH, _RG_ANGULAR_VELOCITY, _RG_TASK_VELOCITY = 15, 17, 18, 19

# The status word's bits: busy, grip detected, the two safety channels' switch and trigger, the safety DC error, and the three-bit error code at bits 7..9.
RG_BUSY, RG_GRIP, RG_S1_PUSHED, RG_S1_TRIGGERED, RG_S2_PUSHED, RG_S2_TRIGGERED, RG_DC_ERROR = 1 << 0, 1 << 1, 1 << 2, 1 << 3, 1 << 4, 1 << 5, 1 << 6
RG_ERROR_SHIFT, RG_ERROR_MASK = 7, 0b111

# ── 2FG7 and 2FG14 ───────────────────────────────────────────────────────────

FG_CONTROL_ADDRESS = 0                  # width (0.1 mm), force (N), speed (%), type
FG_STATUS_ADDRESS = 256
FG_STATUS_COUNT = 20                    # 256..275
FG_IDENTITY_ADDRESS = 1536
FG_IDENTITY_COUNT = 8                   # product code, revisions, firmware, CRC
FG_LIMITS_ADDRESS = 259                 # minimum external, maximum external, minimum internal, maximum internal: 0.1 mm
FG_POWER_ADDRESS = 1028                 # supply power (W), maximum conventional force (N), (reserved), maximum realtime force (N)
FG_POWER_COUNT = 4
FG_TYPE_EXTERNAL, FG_TYPE_INTERNAL, FG_TYPE_STOP = 1, 2, 3
FG_POWER_MIN_W, FG_POWER_MAX_W = 14, 48

# Status block offsets from FG_STATUS_ADDRESS (256).
_FG_STATUS, _FG_EXTERNAL, _FG_INTERNAL, _FG_FORCE, _FG_LINEAR, _FG_MOTOR = 0, 1, 2, 7, 9, 10
FG_BUSY, FG_GRIP, FG_NOT_CALIBRATED, FG_LINEAR_SENSOR_ERROR = 1 << 0, 1 << 1, 1 << 3, 1 << 4


class ProtocolError(Exception):
    """A value the registers cannot carry, or a block that is not the length the map says."""


@dataclass(frozen=True)
class Model:
    name: str
    product_code: int
    family: str                         # "rg" or "fg"
    #: The most the maker's library accepts: the RG's absolute width and conventional force (its documentation: 110 mm and 40 N for the RG2, 160 mm and 120 N for the RG6). None for the 2FG, which reports its own.
    max_width_mm: float | None
    max_force_n: float | None
    #: The least nonzero conventional force (2FG): the library refuses below it ("nonzero conventional force is below the model minimum").
    min_force_n: float | None


MODELS: dict[str, Model] = {
    "RG2": Model("RG2", 32, "rg", 110.0, 40.0, None),
    "RG6": Model("RG6", 33, "rg", 160.0, 120.0, None),
    "2FG7": Model("2FG7", 192, "fg", None, None, 20.0),
    "2FG14": Model("2FG14", 193, "fg", None, None, 40.0),
}


# ── registers and quantities ─────────────────────────────────────────────────

def lround(value: float) -> int:
    """C's `lround`: halves away from zero. Python's `round` takes halves to even, which is a register off for every 0.05 mm."""
    return int(math.floor(value + 0.5)) if value >= 0 else -int(math.floor(-value + 0.5))


def signed(register: int) -> int:
    return register - 0x10000 if register & 0x8000 else register


def _check_block(registers: list[int] | tuple[int, ...], count: int, what: str) -> None:
    if len(registers) != count:
        raise ProtocolError(f"the {what} is {count} registers, not {len(registers)}.")
    for register in registers:
        if isinstance(register, bool) or not isinstance(register, int) or not 0 <= register <= 0xFFFF:
            raise ProtocolError(f"{register!r} is not a sixteen-bit register.")


def _finite(*values: float) -> None:
    for value in values:
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
            raise ProtocolError(f"{value!r} is not a finite number.")


# ── RG ───────────────────────────────────────────────────────────────────────

@dataclass(frozen=True)
class RgState:
    busy: bool
    grip_detected: bool
    safety_1_pushed: bool
    safety_1_triggered: bool
    safety_2_pushed: bool
    safety_2_triggered: bool
    safety_dc_error: bool
    error_code: int
    raw_status: int
    fingertip_offset_mm: float
    motor_voltage_v: float
    motor_current_a: float
    depth_mm: float
    relative_depth_mm: float
    angle_rad: float
    actual_width_mm: float
    temperature_c: float
    width_mm: float                     # the compensated width: the task aperture
    angular_velocity_rad_s: float
    task_velocity_mm_s: float


def decode_rg_state(registers: list[int] | tuple[int, ...]) -> RgState:
    """The twenty registers 258..277. Scales and signs are the maker's library's: widths, depths and the offset are signed tenths of a millimetre, the angle and the angular velocity signed thousandths, the
    voltage and current unsigned thousandths, the temperature unsigned hundredths of a degree, the task velocity signed tenths of a millimetre a second."""
    _check_block(registers, RG_STATUS_COUNT, "RG state block")
    status = registers[_RG_STATUS]
    return RgState(
        busy=bool(status & RG_BUSY), grip_detected=bool(status & RG_GRIP), safety_1_pushed=bool(status & RG_S1_PUSHED), safety_1_triggered=bool(status & RG_S1_TRIGGERED),
        safety_2_pushed=bool(status & RG_S2_PUSHED), safety_2_triggered=bool(status & RG_S2_TRIGGERED), safety_dc_error=bool(status & RG_DC_ERROR),
        error_code=(status >> RG_ERROR_SHIFT) & RG_ERROR_MASK, raw_status=status,
        fingertip_offset_mm=signed(registers[_RG_OFFSET]) / 10.0, motor_voltage_v=registers[_RG_VOLTAGE] / 1000.0, motor_current_a=registers[_RG_CURRENT] / 1000.0,
        depth_mm=signed(registers[_RG_DEPTH]) / 10.0, relative_depth_mm=signed(registers[_RG_RELATIVE_DEPTH]) / 10.0, angle_rad=signed(registers[_RG_ANGLE]) / 1000.0,
        actual_width_mm=signed(registers[_RG_ACTUAL_WIDTH]) / 10.0, temperature_c=registers[_RG_TEMPERATURE] / 100.0, width_mm=signed(registers[_RG_WIDTH]) / 10.0,
        angular_velocity_rad_s=signed(registers[_RG_ANGULAR_VELOCITY]) / 1000.0, task_velocity_mm_s=signed(registers[_RG_TASK_VELOCITY]) / 10.0)


def rg_error_text(state: RgState) -> str | None:
    """What a state says is wrong, or None. The library gives no table of error codes and nor does this repository's source; any nonzero code, and any safety trigger, is reported as it is."""
    parts = []
    if state.error_code:
        parts.append(f"error code {state.error_code}")
    if state.safety_dc_error:
        parts.append("safety DC error (a power cycle after the equipment is made safe clears it; software cannot)")
    if state.safety_1_triggered:
        parts.append("safety switch 1 triggered")
    if state.safety_2_triggered:
        parts.append("safety switch 2 triggered")
    if state.safety_1_pushed:
        parts.append("safety switch 1 pushed")
    if state.safety_2_pushed:
        parts.append("safety switch 2 pushed")
    return ", ".join(parts) if parts else None


def encode_rg_move(width_mm: float, force_n: float, minimum_mm: float, maximum_mm: float, model: Model) -> list[int]:
    """The control block of a conventional grip: `[force, width, 16]`, force in tenths of a newton and width in tenths of a millimetre, both rounded as the maker's library rounds (`lround`). The checks are the
    library's, made on the numbers as given and before rounding: a width outside the installation's limits, a force above the model's, a force of zero. One thing is stricter here: a force that rounds to zero
    (below 0.05 N) is refused, where the library writes a register of zero and a gripper that is told to close with no force."""
    _finite(width_mm, force_n)
    assert model.max_width_mm is not None and model.max_force_n is not None
    if not 0.0 <= minimum_mm < maximum_mm <= model.max_width_mm:
        raise ProtocolError(f"the width limits [{minimum_mm}, {maximum_mm}] mm are not within 0 to {model.max_width_mm} mm for an {model.name}.")
    if not minimum_mm <= width_mm <= maximum_mm:
        raise ProtocolError(f"a width of {width_mm} mm is outside the configured limits [{minimum_mm}, {maximum_mm}] mm.")
    if not 0.0 < force_n <= model.max_force_n:
        raise ProtocolError(f"a force of {force_n} N is not above 0 and at most {model.max_force_n} N for an {model.name}.")
    force = lround(force_n * 10.0)
    if force < 1:
        raise ProtocolError(f"a force of {force_n} N is below the 0.1 N the register resolves: it would be written as zero.")
    return [force, lround(width_mm * 10.0), RG_CONTROL_GRIP]


def rg_stop_block() -> list[int]:
    return [0, 0, RG_CONTROL_STOP]


# ── 2FG ──────────────────────────────────────────────────────────────────────

@dataclass(frozen=True)
class FgState:
    busy: bool
    grip_detected: bool
    not_calibrated: bool
    linear_sensor_error: bool
    raw_status: int
    external_width_mm: float
    internal_width_mm: float
    force_n: float                      # signed: closing into an object is negative
    linear_position_mm: float
    motor_width_mm: float


@dataclass(frozen=True)
class FgLimits:
    min_external_mm: float
    max_external_mm: float
    min_internal_mm: float
    max_internal_mm: float
    supply_power_w: int
    max_force_n: int
    max_realtime_force_n: int


def decode_fg_state(registers: list[int] | tuple[int, ...]) -> FgState:
    """The twenty registers 256..275: the status word, the external and internal widths (signed tenths of a millimetre), the signed force in newtons, the linear mechanism position and the motor width
    (signed hundredths of a millimetre)."""
    _check_block(registers, FG_STATUS_COUNT, "2FG state block")
    status = registers[_FG_STATUS]
    return FgState(
        busy=bool(status & FG_BUSY), grip_detected=bool(status & FG_GRIP), not_calibrated=bool(status & FG_NOT_CALIBRATED), linear_sensor_error=bool(status & FG_LINEAR_SENSOR_ERROR), raw_status=status,
        external_width_mm=signed(registers[_FG_EXTERNAL]) / 10.0, internal_width_mm=signed(registers[_FG_INTERNAL]) / 10.0, force_n=float(signed(registers[_FG_FORCE])),
        linear_position_mm=signed(registers[_FG_LINEAR]) / 100.0, motor_width_mm=signed(registers[_FG_MOTOR]) / 100.0)


def decode_fg_limits(limits: list[int] | tuple[int, ...], power: list[int] | tuple[int, ...]) -> FgLimits:
    _check_block(limits, 4, "2FG limits block")
    _check_block(power, FG_POWER_COUNT, "2FG power block")
    return FgLimits(min_external_mm=limits[0] / 10.0, max_external_mm=limits[1] / 10.0, min_internal_mm=limits[2] / 10.0, max_internal_mm=limits[3] / 10.0,
                    supply_power_w=power[0], max_force_n=power[1], max_realtime_force_n=power[3])


def fg_error_text(state: FgState) -> str | None:
    parts = []
    if state.not_calibrated:
        parts.append("not calibrated")
    if state.linear_sensor_error:
        parts.append("linear sensor error")
    return ", ".join(parts) if parts else None


def encode_fg_grip(width_mm: float, force_n: float, speed_percent: float, limits: FgLimits, model: Model) -> list[int]:
    """The control block of an external grip: `[width, force, speed, 1]`, width in tenths of a millimetre, force in whole newtons, speed in whole percent, rounded with `lround`. The library's checks, made on
    the numbers as given: the width inside the device's own external limits, a force of zero (the device minimum) or from the model's minimum to the device's conventional ceiling, a speed of 1 to 100."""
    _finite(width_mm, force_n, speed_percent)
    assert model.min_force_n is not None
    if not limits.min_external_mm <= width_mm <= limits.max_external_mm:
        raise ProtocolError(f"a width of {width_mm} mm is outside the device's external limits [{limits.min_external_mm}, {limits.max_external_mm}] mm.")
    if force_n < 0 or force_n > limits.max_force_n:
        raise ProtocolError(f"a force of {force_n} N is outside 0 to the device's conventional ceiling of {limits.max_force_n} N.")
    if force_n != 0 and force_n < model.min_force_n:
        raise ProtocolError(f"a nonzero force of {force_n} N is below the {model.name}'s minimum of {model.min_force_n} N (zero selects the device minimum).")
    if not 1.0 <= speed_percent <= 100.0:
        raise ProtocolError(f"a speed of {speed_percent} % is not 1 to 100.")
    return [lround(width_mm * 10.0), lround(force_n), lround(speed_percent), FG_TYPE_EXTERNAL]


def fg_stop_block() -> list[int]:
    return [0, 0, 0, FG_TYPE_STOP]


def firmware_text(registers: list[int] | tuple[int, ...]) -> str:
    _check_block(registers, 2, "firmware block")
    return f"{registers[0] >> 8}.{registers[0] & 0xFF}.{registers[1]}"

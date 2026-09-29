"""MAVLink's frames and Skynet's, and the arithmetic between them.

MAVLink speaks NED for the world (x north, y east, z down) and FRD for a body (x forward, y right, z
down). Skynet speaks REP-103 (§44.4): ENU for the world (x east, y north, z up) and FLU for a body (x
forward, y left, z up), quaternions ordered `xyzw`. A sign error between them moves a vehicle the wrong
way, so the conversion is here, small, and held by `selftest.py` against explicit rotation matrices.

    p_enu   = M · p_ned          M = [[0,1,0],[1,0,0],[0,0,-1]]       (a proper rotation: 180° about (1,1,0)/√2)
    p_frd   = N · p_flu          N = diag(1,-1,-1)                    (180° about x)
    R_enu_flu = M · R_ned_frd · N
"""

from __future__ import annotations

import math

__all__ = [
    "ned_to_enu",
    "enu_to_ned",
    "frd_to_flu",
    "quat_ned_frd_to_enu_flu",
    "quat_from_euler_ned_frd",
    "rotation_matrix",
]

_S = math.sqrt(0.5)
#: M as a quaternion (wxyz): 180° about (1,1,0)/√2.
_Q_M = (0.0, _S, _S, 0.0)
#: N as a quaternion (wxyz): 180° about x.
_Q_N = (0.0, 1.0, 0.0, 0.0)


def ned_to_enu(x: float, y: float, z: float) -> tuple[float, float, float]:
    """A world-frame vector or point: north-east-down to east-north-up."""
    return (y, x, -z)


def enu_to_ned(x: float, y: float, z: float) -> tuple[float, float, float]:
    """The same map back; it is its own inverse."""
    return (y, x, -z)


def frd_to_flu(x: float, y: float, z: float) -> tuple[float, float, float]:
    """A body-frame vector (a rate, say): forward-right-down to forward-left-up."""
    return (x, -y, -z)


def _mul(a: tuple[float, float, float, float], b: tuple[float, float, float, float]) -> tuple[float, float, float, float]:
    aw, ax, ay, az = a
    bw, bx, by, bz = b
    return (
        aw * bw - ax * bx - ay * by - az * bz,
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
    )


def quat_ned_frd_to_enu_flu(w: float, x: float, y: float, z: float) -> tuple[float, float, float, float]:
    """MAVLink's attitude quaternion (wxyz, FRD to NED) as Skynet's base orientation (`xyzw`, FLU to ENU).

    Normalised, with `w >= 0`: `q` and `-q` are one rotation and a state that flips sign between samples
    reads as a spin to anything that differences it.
    """
    norm = math.sqrt(w * w + x * x + y * y + z * z)
    if not math.isfinite(norm) or norm < 1e-6:
        return (math.nan,) * 4
    q = (w / norm, x / norm, y / norm, z / norm)
    rw, rx, ry, rz = _mul(_mul(_Q_M, q), _Q_N)
    if rw < 0:
        rw, rx, ry, rz = -rw, -rx, -ry, -rz
    return (rx, ry, rz, rw)


def quat_from_euler_ned_frd(roll: float, pitch: float, yaw: float) -> tuple[float, float, float, float]:
    """`(w, x, y, z)` for MAVLink's aerospace Euler angles (yaw, then pitch, then roll)."""
    cr, sr = math.cos(roll / 2), math.sin(roll / 2)
    cp, sp = math.cos(pitch / 2), math.sin(pitch / 2)
    cy, sy = math.cos(yaw / 2), math.sin(yaw / 2)
    return (
        cr * cp * cy + sr * sp * sy,
        sr * cp * cy - cr * sp * sy,
        cr * sp * cy + sr * cp * sy,
        cr * cp * sy - sr * sp * cy,
    )


def rotation_matrix(w: float, x: float, y: float, z: float) -> list[list[float]]:
    """The 3x3 matrix of a unit quaternion (wxyz), for the tests that hold the conversion to matrices."""
    return [
        [1 - 2 * (y * y + z * z), 2 * (x * y - w * z), 2 * (x * z + w * y)],
        [2 * (x * y + w * z), 1 - 2 * (x * x + z * z), 2 * (y * z - w * x)],
        [2 * (x * z - w * y), 2 * (y * z + w * x), 1 - 2 * (x * x + y * y)],
    ]

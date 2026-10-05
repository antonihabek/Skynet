"""A driver's shared object, called through its RHAL vtable from Python — so that `chaos.py` can put a fault-injecting link in front of a
Rust or C driver as it does in front of a Python one (DRY-COVERAGE-PLAN.md item 12.C.4).

    driver = Driver("target/release/libskynet_canopen_rs.so")
    rc = driver.open('{"interface": "vcan0", ...}')
    rc, flags, positions = driver.read(joints=3)
    driver.write([0.1, 0.2, 0.3])

The layouts are `bindings/c/skynet_rhal.h`'s, field for field (`skr_state`, `skr_command`, the vtable). Nothing here is a second
definition of the ABI that could drift quietly: the symbol, the vtable's `abi_version` and its size are read back and refused if
they are not what this file was written against.
"""

from __future__ import annotations

import ctypes
import os

ABI_VERSION = 2
SKR_OK = 0
MODE_POSITION = 0


class SkrState(ctypes.Structure):
    _fields_ = [
        ("stamp_ns", ctypes.c_uint64),
        ("seq", ctypes.c_uint32),
        ("joint_count", ctypes.c_uint16),
        ("flags", ctypes.c_uint16),
        ("position_rad", ctypes.POINTER(ctypes.c_float)),
        ("velocity_rad_s", ctypes.POINTER(ctypes.c_float)),
        ("effort_nm", ctypes.POINTER(ctypes.c_float)),
        ("base_pose", ctypes.c_float * 7),
        ("base_twist", ctypes.c_float * 6),
        ("fault_code", ctypes.c_uint32),
    ]


class SkrCommand(ctypes.Structure):
    _fields_ = [
        ("stamp_ns", ctypes.c_uint64),
        ("seq", ctypes.c_uint32),
        ("mode", ctypes.c_uint8),
        ("flags", ctypes.c_uint8),
        ("_pad", ctypes.c_uint8 * 2),
        ("joint_count", ctypes.c_uint16),
        ("values", ctypes.POINTER(ctypes.c_float)),
        ("mask", ctypes.POINTER(ctypes.c_uint8)),
        ("modes", ctypes.POINTER(ctypes.c_uint8)),
    ]


_OPEN = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_char_p, ctypes.POINTER(ctypes.c_void_p))
_CTX = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p)
_READ = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.POINTER(SkrState), ctypes.c_uint64)
_WRITE = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.POINTER(SkrCommand), ctypes.c_uint64)
_DESCRIBE = ctypes.CFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_char_p, ctypes.c_size_t)
_ANY = ctypes.c_void_p


class Vtable(ctypes.Structure):
    _fields_ = [
        ("abi_version", ctypes.c_uint32),
        ("driver_name", ctypes.c_char_p),
        ("driver_version", ctypes.c_char_p),
        ("open", _OPEN),
        ("close", _CTX),
        ("read_state", _READ),
        ("write_command", _WRITE),
        ("emergency_stop", _CTX),
        ("describe", _DESCRIBE),
        ("read_sensor", _ANY),
        ("extension", _ANY),
        ("release_stop", _CTX),
    ]


class Driver:
    """One loaded driver and, once `open` has succeeded, its context."""

    def __init__(self, path: str) -> None:
        if not os.path.exists(path):
            raise SystemExit(f"chaos: no driver at {path}. Build it first (`cargo build --release -p <crate>` puts it in target/release).")
        self._library = ctypes.CDLL(os.path.abspath(path))
        entry = self._library.skynet_rhal_entry
        entry.restype = ctypes.POINTER(Vtable)
        self._table = entry().contents
        if self._table.abi_version != ABI_VERSION:
            raise SystemExit(f"chaos: {path} speaks RHAL ABI {self._table.abi_version} and this loader was written for {ABI_VERSION}.")
        self.name = (self._table.driver_name or b"?").decode()
        self._ctx = ctypes.c_void_p()
        self._seq = 0

    def open(self, config: str) -> int:
        self._ctx = ctypes.c_void_p()
        return int(self._table.open(config.encode(), ctypes.byref(self._ctx)))

    def read(self, joints: int, with_seq: bool = False):
        """(rc, flags, positions), or (rc, flags, positions, seq) for a subject whose stalled stream is a held sequence (§24)."""
        positions = (ctypes.c_float * joints)()
        velocities = (ctypes.c_float * joints)()
        efforts = (ctypes.c_float * joints)()
        state = SkrState()
        state.joint_count = joints
        state.position_rad = ctypes.cast(positions, ctypes.POINTER(ctypes.c_float))
        state.velocity_rad_s = ctypes.cast(velocities, ctypes.POINTER(ctypes.c_float))
        state.effort_nm = ctypes.cast(efforts, ctypes.POINTER(ctypes.c_float))
        rc = int(self._table.read_state(self._ctx, ctypes.byref(state), 0))
        if with_seq:
            return rc, int(state.flags), list(positions), int(state.seq)
        return rc, int(state.flags), list(positions)

    def write(self, values) -> int:
        self._seq += 1
        count = len(values)
        array = (ctypes.c_float * count)(*values)
        mask = (ctypes.c_uint8 * count)(*([1] * count))
        command = SkrCommand()
        command.stamp_ns = 1
        command.seq = self._seq
        command.mode = MODE_POSITION
        command.joint_count = count
        command.values = ctypes.cast(array, ctypes.POINTER(ctypes.c_float))
        command.mask = ctypes.cast(mask, ctypes.POINTER(ctypes.c_uint8))
        return int(self._table.write_command(self._ctx, ctypes.byref(command), 0))

    def emergency_stop(self) -> int:
        return int(self._table.emergency_stop(self._ctx))

    def release_stop(self) -> int:
        return int(self._table.release_stop(self._ctx)) if self._table.release_stop else 0

    def close(self) -> int:
        return int(self._table.close(self._ctx))

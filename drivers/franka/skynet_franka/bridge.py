"""The RHAL driver: a Franka arm over the FCI.

§24's five entry points onto the two transports in :mod:`skynet_franka.protocol`. What this driver
is careful about is the three places where the FCI's shape and §24's shape disagree.

**A Franka is commanded only while a motion is running.** The UDP stream carries a `RobotCommand`
every tick, and the Control box reads the field belonging to the motion generator the `Move` command
started. Outside a motion the stream still runs and nothing in it is read. So §24's `write_command`
cannot simply write: the first command in a mode starts a motion in that mode, a command in a
different mode stops the running one and starts another, and a body with no motion running reports
its state and refuses commands. That is the state machine, and `mode_of` is where it lives.

**`emergency_stop` is `StopMove` and nothing else.** §24's second rule is that the stop must work
from a signal handler and from a dying process — so it is one `send` of twelve bytes on an already
open socket, with no reply waited for, no lock taken and no allocation. It is not
`AutomaticErrorRecovery`, it is not a torque of zero, and it does not wait for the robot to confirm:
a stop path that waits is not a stop path. What it is **not** is the hardware stop. Franka's own
stop is a button wired to the Control box, and this driver cannot press it — the README says so
under what a bench session has to measure.

**The 41 error bits are one fault, surfaced verbatim.** §24 says a vendor fault code is passed
through and not interpreted, so a latched error sets `SKR_FLAG_FAULT` and `fault_code` carries the
index of the first bit. A driver that mapped bits onto meanings would be a second copy of Franka's
own error table, going stale on the day they add a forty-second.

## What the descriptor says, and what it refuses to say

The joint **travel** limits are published by Franka and are a property of the arm, so they are here.
The **torque ratings are not declared**: §63.4's rule is that a design value from a datasheet is not
a safety rating unless the vendor says it is one, and `envelope` is null for the same reason
`skynet-urdf` leaves it null for a URDF's `<limit effort>`. `embodiment.limits.sourced` is a `host`
test and this body is guest; declaring a ceiling out of a brochure would put a number in front of
the Safety Kernel that nobody measured.
"""

from __future__ import annotations

import json
import os
import socket
import struct
import sys
import threading
import time

import skynet_rhal as rhal
from skynet_rhal import Command, Driver, Frame, State

from . import protocol as fci

#: Joint travel, radians, from Franka Robotics' published joint-limit table for the FR3/Panda arm.
#: A travel limit is a *kinematic* fact about the arm and is what §5.2 asks a descriptor to carry;
#: it is not a safety rating, which is why `envelope` below is null.
JOINT_LIMITS = [
    (-2.7437, 2.7437),
    (-1.7837, 1.7837),
    (-2.9007, 2.9007),
    (-3.0421, -0.1518),
    (-2.8065, 2.8065),
    (0.5445, 4.5169),
    (-3.0159, 3.0159),
]

#: §5.1 roles, one per joint, naming the **axis each one actually turns about** rather than where
#: it sits in the list. A seven-axis Franka's chain is fixed by the arm — joint 4 is the elbow on
#: every one ever built — and the axis suffixes are what let a skill written for another arm reach
#: this one: `right_arm.elbow.flex` means the same thing on a UR and on a Franka, and
#: `right_arm.joint4` would mean nothing to either.
#:
#: The link names (`upper_arm`, `forearm`) are vendor-chosen within §5.1's charset, which is what
#: the vocabulary permits: the *chain* and the *axis* are the closed sets and the link is not.
ROLES = [
    "right_arm.shoulder.yaw",
    "right_arm.shoulder.pitch",
    "right_arm.upper_arm.roll",
    "right_arm.elbow.flex",
    "right_arm.forearm.roll",
    "right_arm.wrist.pitch",
    "right_arm.wrist.roll",
]


class FrankaBridge(Driver):
    """One arm, over one FCI connection."""

    def __init__(self) -> None:
        self._tcp: socket.socket | None = None
        self._udp: socket.socket | None = None
        self._command_id = 0
        self._opened = False
        self._estopped = False
        self._seq = 0
        self._state: fci.RobotState | None = None
        self._motion_mode: int | None = None
        self._host = "127.0.0.1"
        self._port = fci.COMMAND_PORT
        self._slug = "franka-fr3"
        self._buffer = b""
        self._peer: tuple[str, int] | None = None
        self._lock = threading.Lock()

    # ── open / close ─────────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            settings = json.loads(config) if config.strip() else {}
        except ValueError:
            return rhal.SKR_EINVAL
        self._host = settings.get("host", os.environ.get("SKYNET_FRANKA_HOST", self._host))
        # Config first, then the environment, then the FCI's own port. `skynet driver check`
        # supplies the config and a `make` target supplies the environment, and an operator's
        # explicit answer beats both — which is the order every other setting in this repository
        # resolves in.
        self._port = int(settings.get("port", os.environ.get("SKYNET_FRANKA_PORT", self._port)))
        self._slug = settings.get("robot_slug", self._slug)

        try:
            self._udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            self._udp.bind((settings.get("bind", "127.0.0.1"), 0))
            self._udp.settimeout(1.0)
            udp_port = self._udp.getsockname()[1]

            self._tcp = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            self._tcp.settimeout(float(settings.get("timeout_s", 3.0)))
            # Nagle off before anything is sent. A command that waits for a full segment is a
            # command that arrives late, and the FCI's command port is request/response.
            self._tcp.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
            self._tcp.connect((self._host, self._port))
        except OSError as error:
            print(f"skynet-franka: could not reach {self._host}:{self._port}: {error!r}",
                  file=sys.stderr, flush=True)
            self._shutdown()
            return rhal.SKR_EIO

        header, payload = self._transact(
            fci.COMMAND_CONNECT, fci.CONNECT_REQUEST.pack(fci.VERSION, udp_port)
        )
        if header is None:
            self._shutdown()
            return rhal.SKR_EIO
        status = payload[0] if payload else fci.CONNECT_INCOMPATIBLE_LIBRARY_VERSION
        if status != fci.CONNECT_SUCCESS:
            theirs = struct.unpack_from("<H", payload, 1)[0] if len(payload) >= 3 else 0
            print(
                f"skynet-franka: this Control box speaks FCI version {theirs} and this driver "
                f"speaks {fci.VERSION}. Every message after the handshake is a fixed-width struct "
                f"whose meaning is the version's, so the connection is refused rather than read.",
                file=sys.stderr,
                flush=True,
            )
            self._shutdown()
            return rhal.SKR_EPROTO

        # The first state, waited for rather than assumed. A driver that returned success here and
        # let the first `read_state` discover the stream was dead would have reported a body it
        # never heard from.
        if self._receive(timeout=2.0) is None:
            print("skynet-franka: connected, and no RobotState arrived within two seconds.",
                  file=sys.stderr, flush=True)
            self._shutdown()
            return rhal.SKR_EIO

        self._opened = True
        return rhal.SKR_OK

    def close(self) -> int:
        if self._opened and self._motion_mode is not None:
            self._transact(fci.COMMAND_STOP_MOVE)
        self._shutdown()
        return rhal.SKR_OK

    def _shutdown(self) -> None:
        self._opened = False
        self._motion_mode = None
        for sock in (self._tcp, self._udp):
            try:
                if sock is not None:
                    sock.close()
            except OSError:
                pass
        self._tcp = self._udp = None

    # ── The command port ─────────────────────────────────────────────────────

    def _transact(self, command: int, payload: bytes = b"") -> tuple[fci.Header | None, bytes]:
        """One request and its reply, matched by command id.

        The id is what makes this safe: the Control box answers a `Move` twice — once when the
        motion starts and once when it finishes — so a client that took "the next message" as its
        reply would read a motion's completion as the answer to whatever it asked next. Replies
        that are not this exchange's are dropped here, and named.
        """
        if self._tcp is None:
            return (None, b"")
        self._command_id += 1
        mine = self._command_id
        try:
            self._tcp.sendall(fci.pack_command(command, mine, payload))
        except OSError:
            return (None, b"")
        deadline = time.monotonic() + 3.0
        while time.monotonic() < deadline:
            message = self._read_message()
            if message is None:
                return (None, b"")
            header, body = message
            if header.command_id == mine:
                return (header, body)
        return (None, b"")

    def _read_message(self) -> tuple[fci.Header, bytes] | None:
        while True:
            if len(self._buffer) >= fci.HEADER_SIZE:
                try:
                    header = fci.Header.unpack(self._buffer)
                except fci.ProtocolError:
                    return None
                if len(self._buffer) >= header.size:
                    message = self._buffer[: header.size]
                    self._buffer = self._buffer[header.size :]
                    return (header, message[fci.HEADER_SIZE :])
            try:
                chunk = self._tcp.recv(4096)
            except OSError:
                return None
            if not chunk:
                return None
            self._buffer += chunk

    # ── The state stream ─────────────────────────────────────────────────────

    def _receive(self, timeout: float = 0.05) -> fci.RobotState | None:
        """The **newest** state in the socket, never the oldest.

        F27's lesson, arriving from a second vendor: a stream at 1 kHz fills a socket buffer with
        seconds of history, and a client that reads one datagram per cycle reads the past. Every
        pending datagram is drained and the last complete one wins, which is §47.6's latest-wins
        rule one layer down.
        """
        if self._udp is None:
            return None
        newest = None
        self._udp.settimeout(timeout)
        while True:
            try:
                raw, peer = self._udp.recvfrom(4096)
            except socket.timeout:
                break
            except OSError:
                # Whatever was drained before the socket failed is still the newest thing there is,
                # so it is decoded below rather than thrown away with a bare return — which is what
                # this did first, and it handed the caller a `bytes` where a state was expected.
                break
            if len(raw) == fci.ROBOT_STATE_SIZE:
                newest = raw
                # Commands go back to wherever the state came from, refreshed on every read rather
                # than cached at open: a Control box that restarts its stream is followed rather
                # than shouted at, and a driver that has heard nothing has nowhere to send.
                self._peer = peer
            self._udp.settimeout(0)
        if newest is None:
            return self._state
        try:
            self._state = fci.RobotState.unpack(newest)
        except fci.ProtocolError as error:
            print(f"skynet-franka: {error}", file=sys.stderr, flush=True)
            return None
        return self._state

    # ── §24 ──────────────────────────────────────────────────────────────────

    def read_state(self, out: State, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        state = self._receive()
        if state is None:
            return rhal.SKR_EIO
        self._seq += 1
        flags = 0
        if self._estopped or state.robot_mode == fci.ROBOT_MODE_USER_STOPPED:
            flags |= rhal.FLAG_ESTOP
        if state.faulted() or state.robot_mode == fci.ROBOT_MODE_REFLEX:
            flags |= rhal.FLAG_FAULT
        # §24's homing field. A Franka's joints are absolute and there is nothing to home, which is
        # a property of the arm rather than a guess about this one.
        flags |= rhal.HOMING_NOT_REQUIRED
        out.stamp_ns = time.monotonic_ns()
        out.seq = self._seq
        out.flags = flags
        out.fault_code = state.fault_code()
        out.position_rad = list(state.q)
        out.velocity_rad_s = list(state.dq)
        # Measured link-side torque, which the arm does report — so this is one of the few bodies
        # here where `effort_nm` is a measurement rather than §24's NaN.
        out.effort_nm = list(state.tau_J)
        return rhal.SKR_OK

    def write_command(self, cmd: Command, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        if self._estopped:
            return rhal.SKR_EPERM
        if cmd.mode not in (rhal.MODE_POSITION, rhal.MODE_VELOCITY, rhal.MODE_TORQUE):
            return rhal.SKR_EINVAL
        if len(cmd.values) != fci.JOINTS or len(cmd.mask) != fci.JOINTS:
            return rhal.SKR_EINVAL
        state = self._state
        if state is None:
            return rhal.SKR_EIO
        if state.faulted():
            # Refused rather than commanded into the fault. The recovery is
            # `AutomaticErrorRecovery`, which an operator asks for — a driver that cleared a latched
            # error on its own would erase why the robot stopped, which is `drivers/canopen`'s F26
            # arriving at a second vendor.
            return rhal.SKR_EPERM

        wanted = self._motion_for(cmd.mode)
        if self._motion_mode != wanted:
            if self._motion_mode is not None:
                self._transact(fci.COMMAND_STOP_MOVE)
                self._motion_mode = None
            header, payload = self._transact(
                fci.COMMAND_MOVE,
                self._move_payload(cmd.mode),
            )
            if header is None:
                return rhal.SKR_EIO
            status = payload[0] if payload else fci.MOVE_ABORTED
            if status not in (fci.MOVE_STARTED, fci.MOVE_SUCCESS):
                print(
                    f"skynet-franka: Move refused — {fci.MOVE_STATUS_NAMES.get(status, status)}",
                    file=sys.stderr,
                    flush=True,
                )
                return rhal.SKR_EPERM
            self._motion_mode = wanted

        # §24's mask: a zero holds the joint. For position that is the joint's own last reading,
        # and for velocity and torque it is zero — which is the difference between holding a pose
        # and going limp, and is why the mask is read per mode rather than once.
        held = state.q if cmd.mode == rhal.MODE_POSITION else [0.0] * fci.JOINTS
        values = [
            float(cmd.values[i]) if cmd.mask[i] else float(held[i]) for i in range(fci.JOINTS)
        ]
        if cmd.mode == rhal.MODE_POSITION:
            message = fci.pack_robot_command(state.message_id, q_c=values)
        elif cmd.mode == rhal.MODE_VELOCITY:
            message = fci.pack_robot_command(state.message_id, dq_c=values)
        else:
            message = fci.pack_robot_command(state.message_id, tau_J_d=values)
        if self._peer is None:
            return rhal.SKR_EIO
        try:
            self._udp.sendto(message, self._peer)
        except OSError:
            return rhal.SKR_EIO
        return rhal.SKR_OK


    def emergency_stop(self) -> int:
        """§24's rule 2: callable from a signal handler and from a dying process.

        One `send` of a twelve-byte header on a socket that is already open. No reply is waited
        for, no lock is taken, nothing is allocated that was not allocated at open, and the latch
        is set first so that a command racing this one is refused even if the send fails.
        """
        self._estopped = True
        if self._tcp is None:
            return rhal.SKR_OK
        try:
            self._command_id += 1
            self._tcp.send(fci.pack_command(fci.COMMAND_STOP_MOVE, self._command_id))
        except OSError:
            # The stop is a latch first and a message second. A box that is unreachable is a box
            # this driver cannot stop, and saying SKR_OK would be claiming otherwise.
            return rhal.SKR_EIO
        self._motion_mode = None
        return rhal.SKR_OK

    def read_sensor(self, name: str, out: Frame) -> int:
        # A Franka arm publishes no images. §24 reports the capability as absent rather than
        # answering an empty frame, and the descriptor declares no sensors to match.
        return rhal.SKR_ENODEV

    # ── The body ─────────────────────────────────────────────────────────────

    def describe(self) -> str:
        joints = []
        for index in range(fci.JOINTS):
            low, high = JOINT_LIMITS[index]
            joints.append(
                {
                    "name": f"panda_joint{index + 1}",
                    "role": ROLES[index],
                    "type": "revolute",
                    # Travel only. Velocity and torque ceilings are ratings, and §63.4 is why they
                    # are not read out of a brochure and put in front of the Safety Kernel.
                    "limits": {
                        "position_min_rad": low,
                        "position_max_rad": high,
                    },
                    "control_modes": ["position", "velocity", "torque"],
                }
            )
        return json.dumps(
            {
                "schema_version": 1,
                "robot_slug": self._slug,
                "digest": None,
                "verified": False,
                "simulator": False,
                "base": "fixed",
                "dof": fci.JOINTS,
                "joints": joints,
                "end_effectors": [],
                "sensors": [],
                "control": {
                    "modes": ["position", "velocity", "torque"],
                    # The FCI's own rate, and the only one it has: a Control box reads one command
                    # per millisecond and counts the ones that do not arrive.
                    "rate_hz_max": 1000,
                    "rate_hz_min": 1000,
                    "command_latency_ms": None,
                },
                "envelope": None,
                "solvers": [],
            }
        )

    # ── helpers ──────────────────────────────────────────────────────────────

    @staticmethod
    def _motion_for(mode: int) -> int:
        return {
            rhal.MODE_POSITION: fci.MOVE_MOTION_JOINT_POSITION,
            rhal.MODE_VELOCITY: fci.MOVE_MOTION_JOINT_VELOCITY,
            rhal.MODE_TORQUE: fci.MOVE_MOTION_NONE,
        }[mode]

    def _move_payload(self, mode: int) -> bytes:
        controller = (
            fci.MOVE_CONTROLLER_EXTERNAL
            if mode == rhal.MODE_TORQUE
            else fci.MOVE_CONTROLLER_JOINT_IMPEDANCE
        )
        return fci.pack_move_payload(controller, self._motion_for(mode))

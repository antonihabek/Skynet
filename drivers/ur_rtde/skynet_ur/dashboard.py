"""The dashboard server on port 29999 — the one interface that can power a UR on.

Line-oriented text, one command per line, one line back. It is the smallest interface Universal
Robots ship and the only one that reaches the parts of the controller RTDE cannot: the brakes, the
power, and whether a protective stop is latched.

**This is not a control interface and this driver does not use it as one.** Everything cyclic goes
over RTDE; what is here is the handful of transitions that happen once, at open, and that a person
would otherwise do at the teach pendant. Each of them is a *state change on a machine*, so each is
verified by reading the mode back rather than by the reply `Powering on` — which the controller
sends before it has done anything.
"""

from __future__ import annotations

import socket
import threading
import time

__all__ = ["Dashboard", "DashboardError"]


class DashboardError(Exception):
    """The dashboard refused, or did not reach the state it was asked for."""


class Dashboard:
    """A dashboard session."""

    def __init__(self, host: str, port: int = 29999, timeout_s: float = 5.0) -> None:
        self._socket = socket.create_connection((host, port), timeout=timeout_s)
        self._socket.settimeout(timeout_s)
        self._timeout_s = timeout_s
        #: One command at a time: a reply carries no tag, so a stop asked for from its own thread while another command waits would have each read the other's answer.
        self._lock = threading.Lock()
        self.banner = self._recv()

    def _recv(self) -> str:
        return self._socket.recv(4096).decode("utf-8", errors="replace").strip()

    def command(self, text: str, timeout_s: float | None = None) -> str:
        """One line out, one line back. `timeout_s` is this command's own bound where the session's is too long for it: a stop is asked with a short one, because it is not worth waiting for a controller that is not there."""
        with self._lock:
            if timeout_s is not None:
                self._socket.settimeout(timeout_s)
            try:
                self._socket.sendall(text.encode() + b"\n")
                return self._recv()
            finally:
                if timeout_s is not None:
                    self._socket.settimeout(self._timeout_s)

    def robot_mode(self) -> str:
        """`POWER_OFF`, `IDLE`, `RUNNING`, … — the word after the colon."""
        return self.command("robotmode").split(":")[-1].strip()

    def safety_status(self) -> str:
        return self.command("safetystatus").split(":")[-1].strip()

    def bring_up(self, timeout_s: float = 30.0) -> None:
        """Power on and release the brakes, waiting for `RUNNING` rather than for a reply.

        `power on` is answered with `Powering on` immediately and the contactor takes seconds; a
        driver that continued on the reply would send its first setpoint into a robot that is not
        energised, and the symptom is a robot that reports its joints perfectly and does not move.
        So each step waits for the *mode* — and gives up with the mode it actually reached, which is
        what tells an operator whether to look at the estop, the brakes or the power.
        """
        mode = self.robot_mode()
        if mode == "RUNNING":
            return
        if mode in ("POWER_OFF", "BOOTING"):
            self.command("power on")
            mode = self._await_mode({"IDLE", "RUNNING"}, timeout_s)
        if mode == "IDLE":
            self.command("brake release")
            mode = self._await_mode({"RUNNING"}, timeout_s)
        if mode != "RUNNING":
            raise DashboardError(
                f"the robot reached `{mode}` rather than `RUNNING`, with safety "
                f"`{self.safety_status()}`. `PROTECTIVE_STOP` and `EMERGENCY_STOP` are cleared at "
                "the machine and not from here; `FAULT` wants the log on the pendant."
            )

    def _await_mode(self, wanted: set[str], timeout_s: float) -> str:
        deadline = time.monotonic() + timeout_s
        mode = self.robot_mode()
        while mode not in wanted and time.monotonic() < deadline:
            time.sleep(0.25)
            mode = self.robot_mode()
        return mode

    def close(self) -> None:
        try:
            self._socket.close()
        except OSError:
            pass

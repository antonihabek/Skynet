"""Wake URSim as the suite's live block does: close the popups, power on, release the brakes, and wait for RUNNING (`drivers/ros2_control/selftest.cpp`, the ur5e_live block).

    python3 wake.py [host]

Standard library only. Exits 0 when the controller says RUNNING, 1 with what it last said.
"""

from __future__ import annotations

import socket
import sys
import time


def dashboard(host: str, command: str) -> str:
    with socket.create_connection((host, 29999), timeout=5) as connection:
        connection.settimeout(5)
        connection.recv(4096)
        connection.sendall(command.encode() + b"\n")
        return connection.recv(4096).decode("utf-8", errors="replace").strip()


def main() -> int:
    host = sys.argv[1] if len(sys.argv) > 1 else "127.0.0.1"
    for command in ("close popup", "close safety popup", "power on"):
        dashboard(host, command)
    time.sleep(4)
    dashboard(host, "brake release")
    time.sleep(3)
    said = ""
    for _ in range(20):
        said = dashboard(host, "robotmode")
        if "RUNNING" in said:
            print(f"URSim: {said}")
            return 0
        time.sleep(1)
    print(f"URSim never reached RUNNING; it said: {said}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())

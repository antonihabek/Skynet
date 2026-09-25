"""Wait until URSim's controller answers, not merely until its port is open.

URSim opens the dashboard port while URControl is still booting, and a dashboard with no controller
behind it answers ``robotmode`` with an error rather than a mode. A suite started on the open port
alone powers on a controller that is not there yet, and fails in a way that reads as a driver
defect. So this waits for a mode: ``POWER_OFF`` is the first word a booted controller says.

    python3 ursim_wait.py <host> [timeout-seconds]

Exits 0 when a mode is read, 1 with the last thing the dashboard said when the time runs out.
Standard library only: it runs on a CI runner and inside the ROS image alike.
"""

from __future__ import annotations

import socket
import sys
import time

# Every mode a booted controller reports (the dashboard server's `robotmode`). `NO_CONTROLLER` and
# `DISCONNECTED` are the two that mean it is not booted yet, so they are not in the list.
MODES = {
    "CONFIRM_SAFETY",
    "BOOTING",
    "POWER_OFF",
    "POWER_ON",
    "IDLE",
    "BACKDRIVE",
    "RUNNING",
}


def robot_mode(host: str) -> str:
    with socket.create_connection((host, 29999), timeout=5) as connection:
        connection.settimeout(5)
        connection.recv(4096)
        connection.sendall(b"robotmode\n")
        return connection.recv(4096).decode("utf-8", errors="replace").strip()


def main() -> int:
    if len(sys.argv) not in (2, 3):
        print("usage: ursim_wait.py <host> [timeout-seconds]", file=sys.stderr)
        return 2
    host = sys.argv[1]
    timeout_s = float(sys.argv[2]) if len(sys.argv) == 3 else 240.0
    deadline = time.monotonic() + timeout_s
    last = "nothing: the dashboard port never accepted a connection"
    while time.monotonic() < deadline:
        try:
            reply = robot_mode(host)
        except OSError as err:
            last = f"no dashboard session ({err})"
        else:
            last = f"`{reply}`"
            if reply.split(":")[-1].strip() in MODES:
                print(f"URSim's controller answers on {host}: {reply}")
                return 0
        time.sleep(1)
    print(
        f"::error::URSim's controller did not answer on {host}:29999 within {timeout_s:.0f} s. "
        f"The last reply was {last}. `docker logs` on the URSim container says why.",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    sys.exit(main())

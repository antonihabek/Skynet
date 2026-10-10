"""The bridge, run once through against the maker's own Virtual Control Box (DRY-COVERAGE-PLAN.md item 12.E.14, F118's rung 1).

    sh sim/run-vm.sh <the converted disk>        # boots the box with its command and data ports on 127.0.0.1:15000 and :15001
    PYTHONPATH=../../bindings/python:. python3 vm_check.py 127.0.0.1:15000

The Virtual Control Box is Rainbow Robotics' own control-box software in a virtual machine (RBVirtualSimulator, an OVA the maker publishes to its customers; no address that stays still, so nothing here fetches it and no workflow
runs this). It is the maker's firmware's own answer to this driver's bytes, which `selftest.py` and `layout_check.py` cannot give: the controller they run against was written here. **It is a simulation of an arm and not an arm**:
the box is in its simulation mode, where only the internal reference moves, so nothing here says what a real arm does with a command, a stop or an error.

What is asked of it is only what this driver says to any control box: open, read the state, stream a target, stop, be released, move, and be asked for what it reports. The data port is the control box's own packet, and the
driver's decoding of it is the thing under test. Exit status 1 if any check fails; 2 if the box could not be reached.
"""

from __future__ import annotations

import json
import math
import socket
import sys
import time

import skynet_rhal as rhal

from skynet_rainbow import bridge as bridge_module
from skynet_rainbow.bridge import RainbowBridge

LIMITS = [[-6.0, 6.0], [-3.0, 3.0], [-2.5, 2.5], [-6.0, 6.0], [-6.0, 6.0], [-6.0, 6.0]]
CHECKS = 0
FAILURES: list[str] = []


def check(condition: bool, what: str, detail: str = "") -> bool:
    global CHECKS
    CHECKS += 1
    print(f"  {'ok  ' if condition else 'FAIL'}  {what}" + (f"   [{detail}]" if detail and not condition else ""), flush=True)
    if not condition:
        FAILURES.append(what)
    return bool(condition)


def read(bridge: RainbowBridge) -> tuple[int, rhal.State]:
    state = rhal.State()
    return bridge.read_state(state, 0), state


def command(seq: int, value: float) -> rhal.Command:
    return rhal.Command(stamp_ns=1, seq=seq, mode=rhal.MODE_POSITION, values=[value] + [0.0] * 5, mask=[1] * 6)


def main(argv: list[str]) -> int:
    address = argv[1] if len(argv) > 1 else "127.0.0.1:15000"
    host, _, port = address.rpartition(":")
    command_port = int(port)
    try:
        socket.create_connection((host, command_port), timeout=3).close()
        socket.create_connection((host, command_port + 1), timeout=3).close()
    except OSError as error:
        print(f"vm_check: the Virtual Control Box is not reachable at {address} and {host}:{command_port + 1} ({error}). `sh sim/run-vm.sh` boots it.")
        return 2
    print(f"the bridge against the Virtual Control Box at {address}", flush=True)
    bridge = RainbowBridge()
    config = json.dumps({"host": host, "port": command_port, "data_port": command_port + 1, "model": "RB5-850", "simulator": True, "limits": LIMITS, "rate_hz": 50})
    check(bridge.open(config) == rhal.SKR_OK, "opens over the box's two ports, in the mode `simulator: true` says")
    try:
        until = time.monotonic() + 3.0
        rc, state = read(bridge)
        while rc != rhal.SKR_OK and time.monotonic() < until:
            time.sleep(0.05)
            rc, state = read(bridge)
        check(rc == rhal.SKR_OK and all(math.isfinite(p) for p in state.position_rad), "a state packet is read, with six joint positions that are numbers", f"rc {rc}, {list(state.position_rad)}")
        start = state.position_rad[0]
        code, body = bridge.extension(bridge_module.TOKEN, b"status")
        status = json.loads(body) if code == rhal.SKR_OK else {}
        check(code == rhal.SKR_OK and status.get("mode_name") == "simulation", "`status` says the box is in its simulation mode", str(body)[:200])
        print(f"        software version {status.get('software_version')}, task state {status.get('task_state')}, robot state {status.get('robot_state')}", flush=True)

        taken = 0
        for i in range(40):
            if bridge.write_command(command(i, 0.01 * i), 0) == rhal.SKR_OK:
                taken += 1
            time.sleep(0.02)
        check(taken == 40, "forty `move_servo_j` targets are all taken", f"{taken} of 40")
        time.sleep(0.4)
        rc, state = read(bridge)
        check(rc == rhal.SKR_OK and abs(state.position_rad[0] - start) > 0.05, "the reference joint follows the stream (read back through the packet)", f"{start:.4f} → {state.position_rad[0]:.4f} rad")

        check(bridge.emergency_stop() == rhal.SKR_OK, "the stop is written and returns at once")
        time.sleep(0.3)
        check(bridge.write_command(command(99, 0.1), 0) != rhal.SKR_OK, "a command after the stop is refused")
        check(bridge.release_stop() == rhal.SKR_OK, "the stop is released once a reading shows the arm not driving a motion")
        check(bridge.write_command(command(100, 0.1), 0) == rhal.SKR_OK, "a command after the release is taken")

        # What the box does with a `move_j` is its own: the box as it boots has no power on its arm (`information_chunk_1` is zero, and it says `info[code][0]`, the maker's "Power Down Detected", beside `halt`), shows
        # robot_state 3 for as long as it is watched and does not move the reference, and Rainbow's own client sees the same. So the check is not that the move arrives but that the driver never says it did when the
        # packet does not show it: an error after its wait, or an end that is not "arrived", or "arrived" with the reference at the goal.
        goal = 0.2
        code, body = bridge.extension(bridge_module.TOKEN, f"move j1={goal} speed=100 accel=100 timeout=4".encode())
        result = json.loads(body) if code == rhal.SKR_OK else {}
        rc, state = read(bridge)
        honest = code != rhal.SKR_OK or result.get("result") != "arrived" or abs(state.position_rad[0] - goal) < 0.01
        check(honest, "`move` never says it arrived when the packet does not show the arm at the goal", f"{code}: {str(body)[:200]}; the reference is {state.position_rad[0]:.4f} rad")
        print(f"        move: {code} {result or str(body)[:160]}", flush=True)

        code, body = bridge.extension(bridge_module.TOKEN, b"clear_faults")
        check(code == rhal.SKR_OK, "`clear_faults` is answered", str(body)[:200])
        rc, state = read(bridge)
        check(rc == rhal.SKR_OK and not state.flags & (rhal.FLAG_ESTOP | rhal.FLAG_FAULT), "the box reads healthy at the end (no stop and no fault flag)", f"flags {state.flags}")
    finally:
        bridge.close()
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed", flush=True)
    return 1 if FAILURES else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

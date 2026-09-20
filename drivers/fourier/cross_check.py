"""The wire cross-check: the vendor's own `rocs_client` against this fake.

`run-check.sh` runs this with the SDK repo's path. It imports Fourier's
`rocs_client` unmodified (plus `websocket-client`, its own dependency, unpacked
beside it — neither is vendored here), points both client classes at the fake,
and asserts the shapes the driver relies on: the limit list, the string-typed
PVC, a `move_joint` that lands, the joint-limit doc example, a pushed debug
state. The vendor's client prints as it goes; the noise is theirs.

If the vendor's construction drives this fake, and the suite drives this driver
against the fake, the driver's wire matches the vendor's through two independent
implementations — including the WebSocket framing, where the fake's codec meets
a real third client. Either half failing is a defect in this tree, not in theirs.
"""

from __future__ import annotations

import asyncio
import os
import sys
import threading
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..",
                               "bindings", "python"))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from skynet_fourier.fake_rocs import serve

CHECKS = 0
FAILURES: list[str] = []


def check(held: bool, what: str) -> None:
    global CHECKS
    CHECKS += 1
    print(("  ok    " if held else "  FAIL  ") + what, flush=True)
    if not held:
        FAILURES.append(what)


def main() -> int:
    sdk = sys.argv[1] if len(sys.argv) > 1 else ""
    wsclient = sys.argv[2] if len(sys.argv) > 2 else ""
    package = os.path.join(sdk, "rocs_client", "__init__.py")
    if not os.path.isfile(package):
        print(f"cross-check: no vendor SDK at {sdk}. Clone it:\n"
              f"  git clone --depth 1 https://github.com/fftai/rocs_client_py <dir>\n"
              f"and pass <dir> (plus an unpacked websocket-client).", flush=True)
        return 2
    sys.path.insert(0, sdk)
    sys.path.insert(0, wsclient)
    try:
        from rocs_client import Human, Motor  # noqa: E402 (path set above)
    except ImportError as error:
        print(f"cross-check: the vendor SDK did not import: {error}. It needs "
              f"`requests` and `websocket-client` importable.", flush=True)
        return 2

    # Port 8001, not an ephemeral one: the vendor's `Motor.__init__` takes `port`
    # and then drops it — `super().__init__(ssl, host)` — so their Motor speaks
    # only to 8001. Their bug, worked around by serving where they listen; the
    # driver under test takes its port from config and the suite proves it on
    # an ephemeral one.
    try:
        server, fake = serve(8001)
    except OSError as error:
        print(f"cross-check: cannot serve the fake on 127.0.0.1:8001 ({error}) — "
              f"something else is there.", flush=True)
        return 2
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()

    print("\nthe vendor's Motor, against this fake", flush=True)
    motor = Motor(host="127.0.0.1")
    check(len(motor.limits) == 6, "limit/list: six motors, the fake's set")
    first = motor.limits[0]
    check(set(("no", "orientation", "min_angle", "max_angle")) <= set(first),
          "limit entries carry no/orientation/min/max_angle")
    pvc = motor.get_motor_pvc("1", "left")
    check(pvc["code"] == 0 and pvc["data"]["position"] == "10.00",
          "pvc: the position as a string, two decimals")
    check(pvc["data"]["velocity"] == "0.0000" and
          pvc["data"]["current"] == "0.12000",
          "pvc: velocity and current in the docstring's shapes")
    motor.move_motor("1", "left", 45.0)
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        with fake.lock:
            if fake.positions[("1", "left")] == 45.0:
                break
        time.sleep(0.01)
    with fake.lock:
        check(fake.positions[("1", "left")] == 45.0,
              "move_motor lands: their framing, this fake's motors")
    motor.enable_motor("1", "left")
    motor.disable_motor("1", "left")
    time.sleep(0.3)
    with fake.lock:
        check(fake.enabled[("1", "left")] is False,
              "disable_motor lands too, and is tracked")
    hand = motor.get_hand_position()
    check(hand["code"] == 0, "the hand state answers")
    motor.exit()

    print("\nthe vendor's Human, against this fake", flush=True)
    pushed: list[str] = []

    async def on_message(message: str) -> None:
        pushed.append(message)

    async def on_connected() -> None:
        pass

    human = Human(host="127.0.0.1", port=8001, on_connected=on_connected,
                  on_message=on_message)
    limits = human.get_joint_limit()
    names = [j["name"] for j in limits["data"]["data"]["jointlimit"]]
    check(names[:2] == ["left_hip_roll", "left_hip_yaw"],
          "joint_limit serves the doc's leg joints")
    states = human.get_joint_states()
    check(states["data"]["data"]["bodyandlegstate"]["currentstatus"] ==
          "StartComplete", "joint_states serves the doc's status shape")
    check(human.stand()["code"] == 0, "stand() is accepted")
    status = human._control_svr_status()
    check(status["code"] == 0, "the control-server status answers")
    human.enable_debug_state()
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline and not pushed:
        time.sleep(0.01)
    check(len(pushed) > 0 and "jointStates" in pushed[0],
          "the pushed debug state arrives on their on_message")
    human.disable_debug_state()
    human.walk(10.0, 0.2)
    time.sleep(0.3)
    with fake.lock:
        check(fake.last_move == {"angle": 10.0, "speed": 0.2},
              "walk lands with angle and speed")
    human.head(1.0, 2.0, 3.0)
    time.sleep(0.3)
    with fake.lock:
        check(fake.last_head == {"roll": 1.0, "pitch": 2.0, "yaw": 3.0},
              "head lands with roll/pitch/yaw")
    human.exit()

    counts = fake.counts()
    print(f"\n  {counts}", flush=True)
    check(counts["malformed"] == 0, "the vendor's bytes all parsed here")
    check(counts["unknown_endpoint"] == 0, "and hit endpoints this fake serves")
    server.shutdown()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed", flush=True)
    return 0 if not FAILURES else 1


if __name__ == "__main__":
    # `asyncio` is imported for the reader: the vendor calls the callbacks with
    # `asyncio.run`, so they must be coroutines, not plain functions.
    _ = asyncio
    raise SystemExit(main())

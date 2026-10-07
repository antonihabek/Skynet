"""This driver's registers, held to OnRobot's own client, which is run (DRY-COVERAGE-PLAN.md item 12.E.8).

    ONROBOT_ORACLE=/path/to/onrobot-oracle PYTHONPATH=../../bindings/python:. python3 layout_check.py

`ONROBOT_ORACLE` is `sim/oracle.cpp` built against the maker's own packages (`sh sim/fetch.sh DIR && sh sim/build.sh DIR/sdk OUT`; `make build-oracle`): OnRobot's `libonrobot_tool_api` 1.2.0, a closed
library the maker publishes with its headers and documentation (BSD-3-Clause) and **no register table**. It is the only statement of the register map there is, so the map in `frames.py` was read off it by running it, and this
check repeats the experiments and holds the driver to what they found. What is compared is what the maker's client does against the simulated gripper written here:

* **what it reads and writes**: every register range this driver reads is inside one the maker's client reads, and every write is one it makes, so that nothing is touched that the maker's own code does not touch;
* **what it decodes**: random state blocks (and the signed and unsigned edges of every register) put into the simulated gripper, decoded by the maker's client and by `frames.py`, field by field;
* **what it writes for a command, and what it refuses before it writes**: random grips, valid and not, against the maker's `RgGripper::move` and `TwoFingerGripper::grip`, the same accept or refuse and the same registers;
* **the stop** and a 2FG's **supply budget**;
* **the two together**: the maker's client moves the simulated gripper and this driver reads where it went, and this driver moves it and the maker's client reads where it went.

It cannot show that a real gripper answers any of this: the gripper is the simulated one (F15), written from what this library was seen to do, and it is accepted by the library's identity checks because it was made to be.
"""

from __future__ import annotations

import json
import os
import random
import subprocess
import sys
import time

import skynet_rhal as rhal

from skynet_onrobot import frames, modbus
from skynet_onrobot.bridge import OnRobotBridge, TOKEN
from skynet_onrobot.gripper_sim import SimulatedGripper

CHECKS = 0
FAILURES: list[str] = []
ORACLE = os.environ.get("ONROBOT_ORACLE", "")


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    if held:
        print(f"  ok    {what}")
    else:
        FAILURES.append(what)
        print(f"  FAIL  {what}")
        if detail:
            print(f"        {detail}")


def section(title: str) -> None:
    print(f"\n{title}")


def until(predicate, timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # noqa: BLE001 - not true yet
            pass
        time.sleep(0.005)
    return bool(predicate())


def oracle(model: str, port: int, *calls: str) -> dict[str, str]:
    """What the maker's client printed for these calls, as the last value of each key."""
    run = subprocess.run([ORACLE, model, str(port), *calls], capture_output=True, text=True, timeout=20)
    out: dict[str, str] = {}
    for line in run.stdout.splitlines():
        key, sep, value = line.partition("=")
        if sep:
            out[key] = value
    return out


def close(a: float, b: float) -> bool:
    return abs(a - b) <= 1e-6 * max(1.0, abs(a), abs(b))


def covered(read: tuple[int, int], by: list[tuple[int, int]]) -> bool:
    return any(address <= read[0] and read[0] + read[1] <= address + count for address, count in by)


def configuration(gripper: SimulatedGripper, model_name: str | None = None, **extra) -> str:
    model = frames.MODELS[model_name or gripper.model.name]
    settings: dict[str, object] = {"model": model.name, "host": "127.0.0.1", "port": gripper.port, "stop_mode": "stop", "simulator": True, "force_n": max(30.0, model.min_force_n or 0.0), "rate_hz": 50, "timeout_ms": 120}
    if model.family == "fg":
        settings.update({"speed": 80, "supply_power_w": 48})
    else:
        settings["limits"] = [0.0, float(model.max_width_mm or 0.0) / 1000.0]
    settings.update(extra)
    return json.dumps(settings)


def run_bridge(gripper: SimulatedGripper, **extra) -> OnRobotBridge:
    bridge = OnRobotBridge()
    assert bridge.open(configuration(gripper, **extra)) == rhal.SKR_OK
    return bridge


RG_FIELDS = (("busy", "busy", False), ("grip_detected", "grip_detected", False), ("safety_1_pushed", "safety_1_pushed", False), ("safety_1_triggered", "safety_1_triggered", False), ("safety_2_pushed", "safety_2_pushed", False),
             ("safety_2_triggered", "safety_2_triggered", False), ("safety_dc_error", "safety_dc_error", False), ("error_code", "error_code", False), ("raw_status", "raw_status", False),
             ("fingertip_offset_mm", "fingertip_offset_mm", True), ("motor_voltage_v", "motor_voltage_v", True), ("motor_current_a", "motor_current_a", True), ("actual_depth_mm", "depth_mm", True),
             ("actual_relative_depth_mm", "relative_depth_mm", True), ("temperature_c", "temperature_c", True), ("width_mm", "width_mm", True), ("actual_width_mm", "actual_width_mm", True),
             ("mechanism_angular_position_rad", "angle_rad", True), ("mechanism_angular_velocity_rad_s", "angular_velocity_rad_s", True), ("task_velocity_mm_s", "task_velocity_mm_s", True))
FG_FIELDS = (("busy", "busy", False), ("grip_detected", "grip_detected", False), ("not_calibrated", "not_calibrated", False), ("linear_sensor_error", "linear_sensor_error", False), ("raw_status", "raw_status", False),
             ("external_width_mm", "external_width_mm", True), ("internal_width_mm", "internal_width_mm", True), ("linear_mechanism_position_mm", "linear_position_mm", True), ("raw_motor_width_mm", "motor_width_mm", True),
             ("force_n", "force_n", True))


def agree(theirs: dict[str, str], ours: object, fields) -> list[str]:
    wrong = []
    for their_key, our_key, is_float in fields:
        want = getattr(ours, our_key)
        got = theirs.get(their_key)
        if got is None:
            wrong.append(f"{their_key}: the maker's client printed nothing")
        elif is_float and not close(float(got), float(want)):
            wrong.append(f"{their_key}: the maker's client says {got}, this driver {want}")
        elif not is_float and int(float(got)) != int(want):
            wrong.append(f"{their_key}: the maker's client says {got}, this driver {int(want)}")
    return wrong


def main() -> int:
    if not ORACLE or not os.path.exists(ORACLE):
        print("ONROBOT_ORACLE names no built oracle (sh sim/fetch.sh DIR && sh sim/build.sh DIR/sdk OUT, then ONROBOT_ORACLE=OUT/onrobot-oracle). Nothing was compared.")
        return 2
    rng = random.Random(59)
    for name in ("RG2", "RG6", "2FG7", "2FG14"):
        model = frames.MODELS[name]
        gripper = SimulatedGripper(name).start()
        try:
            # ── identity ──
            section(f"{name} — the maker's client accepts the simulated gripper, and this driver and it read the same identity")
            theirs = oracle(name, gripper.port, "state")
            check(theirs.get("open_error") is None and theirs.get("product_code") == str(model.product_code), f"the maker's client opens a simulated {name}: product code {model.product_code}, which is the one it demands", str(theirs))
            check(theirs.get("firmware", "").startswith("1.1.33"), f"and reads its firmware as {gripper.firmware[0]}.{gripper.firmware[1]}.{gripper.firmware[2]}", theirs.get("firmware", ""))
            others = [m for m in ("RG2", "RG6", "2FG7", "2FG14") if m != name]
            wrong = oracle(others[0], gripper.port, "state")
            check("product code does not match" in wrong.get("open_error", ""), f"and refuses it as a {others[0]}, as this driver does", str(wrong))
            bridge = OnRobotBridge()
            code = bridge.open(configuration(gripper, others[0]))
            bridge.close()
            check(code == rhal.SKR_ENODEV, "this driver refuses it as that model too, by name, and commands nothing")

            # ── what is read ──
            section(f"{name} — every register this driver reads is one the maker's client reads, and every write one it makes")
            gripper.requests.clear()
            oracle(name, gripper.port, "state", *(("limits", "power") if model.family == "fg" else ("configuration",)))
            their_reads = [(address, count) for function, address, count in gripper.requests if function == modbus.FC_READ_HOLDING]
            gripper.requests.clear()
            gripper.writes.clear()
            bridge = run_bridge(gripper)
            time.sleep(0.3)
            bridge.write_command(rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[(model.max_width_mm or 100.0) / 2000.0], mask=[1]), 0)
            time.sleep(0.2)
            bridge.emergency_stop()
            until(lambda: any(address == 0 and len(values) in (3, 4) and values[-1] in (frames.RG_CONTROL_STOP, frames.FG_TYPE_STOP) for _, address, values in gripper.writes), 2)
            our_reads = sorted({(address, count) for function, address, count in gripper.requests if function == modbus.FC_READ_HOLDING})
            stray = [r for r in our_reads if not covered(r, their_reads)]
            check(not stray, f"this driver reads {len(our_reads)} ranges ({', '.join(f'{a}..{a + c - 1}' for a, c in our_reads)}) and every one is inside a range the maker's client reads", f"not read by the maker's client: {stray}; it reads {sorted(set(their_reads))}")
            bridge.close()

            # ── what is decoded ──
            section(f"{name} — random state blocks, decoded by the maker's client and by this driver")
            gripper.frozen = True
            base = frames.RG_STATUS_ADDRESS if model.family == "rg" else frames.FG_STATUS_ADDRESS
            count = frames.RG_STATUS_COUNT if model.family == "rg" else frames.FG_STATUS_COUNT
            blocks = [[0] * count, [0xFFFF] * count, [0x8000] * count, [0x7FFF] * count, [i + 1 for i in range(count)]]
            blocks += [[rng.choice((0, 1, 0x7FFF, 0x8000, 0xFFFF, rng.randrange(0x10000), rng.randrange(0x10000))) for _ in range(count)] for _ in range(60)]
            wrong_blocks = []
            for block in blocks:
                gripper.set_registers(base, block)
                theirs = oracle(name, gripper.port, "state")
                ours = frames.decode_rg_state(block) if model.family == "rg" else frames.decode_fg_state(block)
                problems = agree(theirs, ours, RG_FIELDS if model.family == "rg" else FG_FIELDS)
                if problems:
                    wrong_blocks.append((block, problems[:3]))
            check(not wrong_blocks, f"{len(blocks)} state blocks, {len(blocks) * len(RG_FIELDS if model.family == 'rg' else FG_FIELDS)} fields: the maker's client and this driver decode every one the same", str(wrong_blocks[:2]))
            gripper.frozen = False
            if model.family == "fg":
                gripper.frozen = True
                wrong_limits = []
                for _ in range(40):
                    limit_block = [rng.randrange(0x10000) for _ in range(4)]
                    power_block = [rng.randrange(0x10000), rng.randrange(0x10000), rng.randrange(0x10000), rng.randrange(0x10000)]
                    gripper.set_registers(frames.FG_LIMITS_ADDRESS, limit_block)
                    gripper.set_registers(frames.FG_POWER_ADDRESS, power_block)
                    theirs = oracle(name, gripper.port, "limits")
                    ours = frames.decode_fg_limits(limit_block, power_block)
                    for their_key, value in (("minimum_external_width_mm", ours.min_external_mm), ("maximum_external_width_mm", ours.max_external_mm), ("minimum_internal_width_mm", ours.min_internal_mm),
                                             ("maximum_internal_width_mm", ours.max_internal_mm), ("supply_power_w", ours.supply_power_w), ("maximum_force_n", ours.max_force_n), ("maximum_realtime_force_n", ours.max_realtime_force_n)):
                        if not close(float(theirs.get(their_key, "nan")), float(value)):
                            wrong_limits.append((their_key, theirs.get(their_key), value))
                check(not wrong_limits, "40 random limit and power blocks: the maker's client and this driver read every limit, the supply budget and both force ceilings the same", str(wrong_limits[:3]))
                gripper.frozen = False
                gripper.power_cycle()
            else:
                wrong_power = []
                for value in (0, 1, 399, 400, 65000, 65535):
                    gripper.set_registers(frames.RG_CONFIG_ADDRESS, [value])
                    theirs = oracle(name, gripper.port, "configuration")
                    if not close(float(theirs.get("long_term_power_limit_w", "nan")), value / 10.0):
                        wrong_power.append((value, theirs))
                check(not wrong_power, "the RG's long-term power limit is read in tenths of a watt, as this driver's map says", str(wrong_power))

            # ── what is written ──
            section(f"{name} — random grips: the same accept or refuse, and the same registers, as the maker's client")
            gripper.place(model.max_width_mm / 2 if model.max_width_mm else 40.0)
            if model.family == "fg":
                limits = frames.decode_fg_limits([gripper._memory[frames.FG_LIMITS_ADDRESS + i] for i in range(4)], [gripper._memory[frames.FG_POWER_ADDRESS + i] for i in range(4)])  # noqa: SLF001
            disagreements = []
            different_on_purpose = []
            for index in range(150 if model.family == "rg" else 200):
                if model.family == "rg":
                    width = rng.choice((0.0, model.max_width_mm, rng.uniform(-5.0, model.max_width_mm + 5.0), round(rng.uniform(0, model.max_width_mm), 2), 50.05, 50.04))
                    force = rng.choice((0.0, 0.04, 0.05, model.max_force_n, rng.uniform(-1.0, model.max_force_n + 2.0), round(rng.uniform(0, model.max_force_n), 2), 20.05, 20.04))
                    call = f"move:{width!r}:{force!r}"
                else:
                    width = rng.choice((limits.min_external_mm, limits.max_external_mm, rng.uniform(limits.min_external_mm - 2, limits.max_external_mm + 2), round(rng.uniform(limits.min_external_mm, limits.max_external_mm), 2), 50.05, 50.04))
                    force = rng.choice((0.0, model.min_force_n, model.min_force_n - 0.5, limits.max_force_n, limits.max_force_n + 0.5, rng.uniform(0, limits.max_force_n + 5), round(rng.uniform(model.min_force_n, limits.max_force_n), 1)))
                    speed = rng.choice((1.0, 100.0, 0.4, 100.4, 60.5, rng.uniform(0.0, 101.0), 99.5))
                    call = f"grip:{width!r}:{force!r}:{speed!r}"
                gripper.writes.clear()
                theirs = oracle(name, gripper.port, call)
                their_block = next((list(values) for _, address, values in gripper.writes if address == 0), None)
                accepted = theirs.get("move") == "ok" or theirs.get("grip") == "ok"
                try:
                    if model.family == "rg":
                        ours = frames.encode_rg_move(width, force, 0.0, float(model.max_width_mm), model)
                    else:
                        ours = frames.encode_fg_grip(width, force, speed, limits, model)
                except frames.ProtocolError as error:
                    ours = None
                    reason = str(error)
                if accepted != (ours is not None) or (accepted and their_block != ours):
                    if accepted and ours is None and model.family == "rg" and their_block and their_block[0] == 0 and 0.0 < force < 0.05:
                        different_on_purpose.append(call)
                    else:
                        disagreements.append((call, their_block, ours, theirs.get("move_error") or theirs.get("grip_error")))
            check(not disagreements, f"{150 if model.family == 'rg' else 200} random {'moves' if model.family == 'rg' else 'grips'}: the maker's client and this driver accept the same and write the same registers", str(disagreements[:3]))
            if model.family == "rg":
                check(len(different_on_purpose) >= 1, f"the one difference there is, pinned (F300): a force under 0.05 N the maker's client writes as a register of zero (a gripper told to close with no force), and this driver refuses ({len(different_on_purpose)} drawn)")
            gripper.writes.clear()
            oracle(name, gripper.port, "stop")
            their_stop = next((list(values) for _, address, values in gripper.writes if address == 0), None)
            check(their_stop == (frames.rg_stop_block() if model.family == "rg" else frames.fg_stop_block()), f"the maker's stop is the block this driver writes: {their_stop}")

            # ── 2FG power ──
            if model.family == "fg":
                section(f"{name} — the supply budget: what the maker's client accepts, and what it writes")
                bad = []
                for watts in range(10, 52):
                    gripper.power_cycle()
                    gripper.writes.clear()
                    theirs = oracle(name, gripper.port, f"setpower:{watts}")
                    accepted = "setpower_error" not in theirs
                    written = [values for _, address, values in gripper.writes if address == frames.FG_POWER_ADDRESS]
                    ours_accepts = frames.FG_POWER_MIN_W <= watts <= frames.FG_POWER_MAX_W
                    if accepted != ours_accepts or (accepted and written != [(watts,)]):
                        bad.append((watts, theirs, written))
                check(not bad, "42 supply budgets from 10 to 51 W: the maker's client accepts 14 to 48 as this driver does, and writes the one register", str(bad[:3]))
                gripper.power_cycle()
                gripper.forget_power = True
                theirs = oracle(name, gripper.port, "setpower:30")
                check("did not retain" in theirs.get("setpower_error", ""), "a gripper that does not keep the budget it was told is refused by the maker's client, as it is by this driver", str(theirs))
                gripper.forget_power = False
                gripper.power_cycle()

            # ── the two together ──
            section(f"{name} — the maker's client moves the gripper and this driver reads where it went, and the other way round")
            target_mm = (model.max_width_mm or 0.0) / 2.0 if model.family == "rg" else (limits.min_external_mm + limits.max_external_mm) / 2.0
            gripper.place(target_mm + 10.0)
            bridge = run_bridge(gripper)
            check(until(lambda: bridge.read_state(rhal.State(), 0) == rhal.SKR_OK, 3), "this driver reads the gripper")
            call = f"move:{target_mm}:20" if model.family == "rg" else f"grip:{target_mm}:40:100"
            oracle(name, gripper.port, call)
            check(until(lambda: not gripper.moving and abs(gripper.width_mm - target_mm) < 0.11, 5), f"the maker's client moves the gripper to {target_mm} mm")
            out = rhal.State()
            until(lambda: bridge.read_state(out, 0) == rhal.SKR_OK and abs(out.position_rad[0] * 1000.0 - target_mm) < 0.11, 3)
            theirs = oracle(name, gripper.port, "state")
            theirs_mm = float(theirs.get("width_mm") or theirs.get("external_width_mm", "nan"))
            check(abs(out.position_rad[0] * 1000.0 - theirs_mm) < 0.11 and abs(theirs_mm - target_mm) < 0.11, f"and this driver reads {out.position_rad[0] * 1000.0:.2f} mm where the maker's client reads {theirs_mm:.2f}")
            goal = target_mm + 7.0
            code = bridge.write_command(rhal.Command(stamp_ns=2, seq=2, mode=rhal.MODE_POSITION, values=[goal / 1000.0], mask=[1]), 0)
            check(code == rhal.SKR_OK and until(lambda: not gripper.moving and abs(gripper.width_mm - goal) < 0.11, 5), f"this driver moves it to {goal} mm")
            theirs = oracle(name, gripper.port, "state")
            theirs_mm = float(theirs.get("width_mm") or theirs.get("external_width_mm", "nan"))
            check(abs(theirs_mm - goal) < 0.11, f"and the maker's client reads {theirs_mm:.2f} mm")
            bridge.close()
        finally:
            gripper.stop()

    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())

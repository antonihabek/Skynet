"""The driver against Robotiq's own fake gripper, with the maker's own client reading the same gripper (DRY-COVERAGE-PLAN.md item 12.E.8).

    ROBOTIQ_SIM=/path/to/built/harnesses PYTHONPATH=../../bindings/python:. python3 live.py

`sim/build.sh` builds the maker's SDK unmodified and, against it, `robotiq-fake-server` (the maker's own `fake::RegisterModel` behind its own `fake::GripperServer`, on a TCP port and a pseudo-terminal) and
`robotiq-second-opinion` (the maker's own `GripperModbusClient`, reading the status block every 50 ms and writing nothing). This block runs this driver against the first with the second looking on, and holds three things
to the pair: what the driver reports against what the maker's client reads, the bytes the maker's server was handed against what the driver says it sent, and what the driver does about a state staged in the maker's model
(`RegisterModel::status()` and `setStatus()`, the way its header says a caller staging one does).

**What the maker's fake is, and is not:** the register map and the activation and fault-latch rules of a gripper, with nothing physical: activation completes instantly, the fingers are wherever they were told, there is no
motion, no object detection and no fault of its own. Rung 2 of F118's ladder: a maker's simulator, and not a gripper.
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import subprocess
import sys
import tempfile
import threading
import time

ROOT = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(ROOT, "..", "..", "bindings", "python"))
sys.path.insert(0, ROOT)

import skynet_rhal as rhal  # noqa: E402

from skynet_robotiq import frames  # noqa: E402
from skynet_robotiq.bridge import _FLAGS_OF_CLASS, RobotiqBridge, TOKEN  # noqa: E402

CHECKS = 0
FAILURES: list[str] = []
HERE = os.environ.get("ROBOTIQ_SIM", "")
SPEED, EFFORT = 0.1, 0.5
P85 = frames.PROFILES["2F-85"]
SPEED_REGISTER = frames.speed_to_register(SPEED, P85)
EFFORT_REGISTER = frames.effort_to_register(EFFORT)


def check(held: bool, what: str, detail: str = "") -> None:
    global CHECKS
    CHECKS += 1
    print(("  ok    " if held else "  FAIL  ") + what, flush=True)
    if not held:
        FAILURES.append(what)
        if detail:
            print(f"        {detail}", flush=True)


def section(title: str) -> None:
    print(f"\n{title}", flush=True)


def until(predicate, timeout: float = 5.0) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if predicate():
                return True
        except Exception:  # noqa: BLE001 - not true yet
            pass
        time.sleep(0.01)
    return bool(predicate())


class Server:
    """The maker's fake gripper as a process, on a TCP port and a pseudo-terminal, with its own record of every request and reply."""

    def __init__(self, port: int = 0, log: str | None = None) -> None:
        self.log = log or tempfile.mktemp(prefix="robotiq-frames-", suffix=".jsonl")
        self.process = subprocess.Popen([os.path.join(HERE, "robotiq-fake-server"), "--tcp", str(port), "--pty", "--log", self.log], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
        self.lines: list[str] = []
        self._lock = threading.Lock()
        self.port = 0
        self.pty = ""
        threading.Thread(target=self._read, daemon=True).start()
        until(lambda: self.port and self.pty, 5.0)

    def _read(self) -> None:
        assert self.process.stdout
        for line in self.process.stdout:
            line = line.strip()
            if line.startswith("tcp "):
                self.port = int(line.rsplit(":", 1)[1])
            elif line.startswith("pty "):
                self.pty = line.split(" ", 1)[1]
            else:
                with self._lock:
                    self.lines.append(line)

    def tell(self, line: str, expect: str) -> str:
        assert self.process.stdin
        with self._lock:
            mark = len(self.lines)
        self.process.stdin.write(line + "\n")
        self.process.stdin.flush()
        until(lambda: any(l.startswith(expect) for l in self.lines[mark:]), 3.0)
        return next((l for l in self.lines[mark:] if l.startswith(expect)), "")

    def fault(self, byte: int) -> None:
        self.tell(f"fault {byte:02x}", "ok fault")

    def exchanges(self) -> list[dict]:
        out = []
        for line in open(self.log, encoding="utf-8"):
            try:
                out.append(json.loads(line))
            except json.JSONDecodeError:
                pass
        return out

    def stop(self) -> None:
        self.process.terminate()
        try:
            self.process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.process.kill()


class Opinion:
    """The maker's client, reading the status block every 50 ms."""

    def __init__(self, port: int, seconds: float = 300.0) -> None:
        self.process = subprocess.Popen([os.path.join(HERE, "robotiq-second-opinion"), "127.0.0.1", str(port), "9", "50", str(seconds)], stdout=subprocess.PIPE, text=True, bufsize=1)
        self.reads: list[dict] = []
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self) -> None:
        assert self.process.stdout
        for line in self.process.stdout:
            try:
                self.reads.append(json.loads(line))
            except json.JSONDecodeError:
                pass

    def latest(self) -> dict | None:
        for read in reversed(self.reads):
            if "status" in read:
                return read["status"]
        return None

    def sees(self, predicate, timeout: float = 3.0) -> bool:
        return until(lambda: (status := self.latest()) is not None and predicate(status), timeout)

    def stop(self) -> None:
        self.process.terminate()


def settings(**extra) -> dict:
    out = {"model": "2F-85", "stop_mode": "halt", "speed": SPEED, "effort": EFFORT, "simulator": True, "roles": {"jaw": "right_hand.gripper.grip"}, "rate_hz": 50, "timeout_ms": 100}
    out.update(extra)
    return out


def opened(**extra) -> tuple[RobotiqBridge, str]:
    bridge = RobotiqBridge()
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.open(json.dumps(settings(**extra)))
    assert code == rhal.SKR_OK, said.getvalue()
    return bridge, said.getvalue()


def state(bridge: RobotiqBridge) -> tuple[int, rhal.State]:
    out = rhal.State()
    return bridge.read_state(out, 0), out


def write(bridge: RobotiqBridge, value: float) -> tuple[int, str]:
    said = io.StringIO()
    with contextlib.redirect_stderr(said):
        code = bridge.write_command(rhal.Command(stamp_ns=1, seq=1, mode=rhal.MODE_POSITION, values=[value], mask=[1]), 0)
    return code, said.getvalue()


def ext(bridge: RobotiqBridge, text: str) -> tuple[int, object]:
    code, body = bridge.extension(TOKEN, text.encode())
    try:
        return code, json.loads(body)
    except json.JSONDecodeError:
        return code, body.decode()


def register(opening: float) -> int:
    value = frames.opening_to_register(opening, P85)
    assert value is not None
    return value


def decoded(server: Server) -> list[dict]:
    """The driver's requests as the maker's server logged them: the function, the two ranges, and the command block."""
    out = []
    for entry in server.exchanges():
        request = bytes.fromhex(entry["request"])
        reply = bytes.fromhex(entry["reply"]) if entry["reply"] else b""
        row = {"t": entry["t"], "line": entry["line"], "peer": entry.get("peer", 0), "function": request[1] if len(request) > 1 else None, "answered": bool(reply) and not reply[1] & 0x80, "crc": len(request) > 3 and frames.crc16(request[:-2]) == int.from_bytes(request[-2:], "little")}
        if len(request) > 1 and request[1] == frames.FC_READ_HOLDING and len(request) == 8:
            row.update(read=(int.from_bytes(request[2:4], "big"), int.from_bytes(request[4:6], "big")))
        if len(request) > 1 and request[1] == frames.FC_READ_WRITE and len(request) == 19:
            row.update(read=(int.from_bytes(request[2:4], "big"), int.from_bytes(request[4:6], "big")), write=(int.from_bytes(request[6:8], "big"), int.from_bytes(request[8:10], "big")), block=request[11:17])
        out.append(row)
    return out


def main() -> int:
    if not HERE or not os.path.exists(os.path.join(HERE, "robotiq-fake-server")):
        print("::error::ROBOTIQ_SIM names no directory of built harnesses (sim/build.sh builds them from a checkout of Robotiq/grippers). Nothing was run.")
        return 1
    print("skynet-robotiq live — against Robotiq's own fake gripper, with the maker's own client looking on\n")
    server = Server()
    opinion = Opinion(server.port)
    try:
        # ── a gripper that is not activated, as the maker's fake starts ───────
        section("a gripper that is not activated — as the maker's own fake starts, and as the maker's client reads it")
        check(opinion.sees(lambda s: s["gACT"] == 0 and s["gSTA"] == 0), "the maker's client reads gACT 0 and gSTA 0: not activated")
        bridge, said = opened(host="127.0.0.1", port=server.port)
        check("not activated" in said and "calibration sweep" in said, "the driver opens, and says it is not activated and that activating is a sweep of the fingers' range", said)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK), "and reads it")
        rc, out = state(bridge)
        check((out.flags & rhal.HOMING_MASK) == rhal.HOMING_PENDING and out.flags & rhal.FLAG_DEGRADED, "HOMING_PENDING and DEGRADED: its position means nothing yet")
        rc, said = write(bridge, 0.04)
        check(rc == rhal.SKR_EPERM and "activate" in said, "a command is refused EPERM, naming the motion that activates", said)
        mark = len(decoded(server))
        rc, body = ext(bridge, "activate timeout=10")
        check(rc == rhal.SKR_OK and body["result"] == "activated", "the activate motion runs the handshake against the maker's model, and it completes", str(body))
        rows = [row for row in decoded(server)[mark:] if "block" in row]
        actions = [row["block"][0] for row in rows]
        check(0x00 in actions and actions[-1] == frames.ACT and actions.index(0x00) < actions.index(frames.ACT), "the maker's server was handed rACT clear, and then rACT set: the reset and the sweep, in that order", str(actions[:6]))
        check(all(a & frames.GTO == 0 for a in actions), "with rGTO clear throughout: finishing an activation starts no motion")
        check(opinion.sees(lambda s: s["gACT"] == 1 and s["gSTA"] == 3), "and the maker's client reads gACT 1 and gSTA 3: activated")
        rc, out = state(bridge)
        check((out.flags & rhal.HOMING_MASK) == rhal.HOMING_COMPLETE and not out.flags & (rhal.FLAG_DEGRADED | rhal.FLAG_FAULT | rhal.FLAG_ESTOP), "the driver says HOMING_COMPLETE with no flag")

        # ── a command, and what the maker's client sees ───────────────────────
        section("a command — the driver's state against what the maker's client reads of the same gripper")
        for target in (0.04, 0.0, 0.085, 0.0425, 0.02):
            code, said = write(bridge, target)
            want = register(target)
            check(code == rhal.SKR_OK and opinion.sees(lambda s, want=want: s["gPR"] == want and s["gPO"] == want and s["gGTO"] == 1), f"a go-to {target} m: the maker's client reads gPR and gPO at register {want} with gGTO set", said)
            until(lambda: abs(state(bridge)[1].position_rad[0] - (frames.opening_from_register(want, P85) or 0)) < 1e-9)
            rc, out = state(bridge)
            theirs = opinion.latest()
            check(rc == rhal.SKR_OK and theirs is not None and abs(out.position_rad[0] - (frames.opening_from_register(theirs["gPO"], P85) or 0)) < 1e-9, f"and the driver's state is the opening of that register: {out.position_rad[0]:.5f} m")
        block = next(row["block"] for row in reversed(decoded(server)) if "block" in row)
        check(block == bytes([frames.ACT | frames.GTO, 0, 0, register(0.02), SPEED_REGISTER, EFFORT_REGISTER]), "the last command block the maker's server was handed is rACT and rGTO, the target register, the operator's speed and force", block.hex())

        # ── the stop ──────────────────────────────────────────────────────────
        section("the stop — the maker's client reads the go-to taken away")
        began = time.monotonic()
        check(bridge.emergency_stop() == rhal.SKR_OK and time.monotonic() - began < 0.01, "emergency_stop returns at once")
        check(opinion.sees(lambda s: s["gGTO"] == 0 and s["gACT"] == 1), "and the maker's client reads gGTO clear with gACT still set: halted, and still activated")
        check(write(bridge, 0.05)[0] == rhal.SKR_EPERM, "a command is refused while latched")
        check(bridge.release_stop() == rhal.SKR_OK, "release_stop clears the latch once the gripper has shown it took the halt")
        check(write(bridge, 0.05)[0] == rhal.SKR_OK and opinion.sees(lambda s: s["gGTO"] == 1 and s["gPR"] == register(0.05)), "and the next command goes out, and the maker's client reads it")

        # ── motions ───────────────────────────────────────────────────────────
        section("motions — move, grip and release, against a model with no motion and no objects")
        rc, body = ext(bridge, "move position=0.03")
        check(rc == rhal.SKR_OK and body["object"] == "at the requested position" and abs(body["opening_m"] - 0.03) < 0.001, "a move is at the requested position", str(body))
        mark = len(decoded(server))
        rc, body = ext(bridge, "grip effort=0.4")
        check(rc == rhal.SKR_OK and body["result"] == "closed_on_nothing", "a grip closes to the end and says it met nothing: the maker's fake models no object, and the driver does not pretend", str(body))
        check(any(row["block"][5] == frames.effort_to_register(0.4) and row["block"][3] == register(0.0) for row in decoded(server)[mark:] if "block" in row), "and the maker's server was handed the effort asked and the closed register")
        rc, body = ext(bridge, "release")
        check(rc == rhal.SKR_OK and body["result"] == "released" and abs(body["opening_m"] - 0.085) < 0.001, "a release opens to the end", str(body))

        # ── a state staged in the maker's own model ───────────────────────────
        section("faults — a state staged in the maker's own register model, which keeps it across commands until rACT falls")
        server.fault(0x0E)
        check(opinion.sees(lambda s: s["gFLT"] == 14), "the maker's client reads the overcurrent the model was staged with")
        until(lambda: state(bridge)[1].fault_code == 0x0E)
        rc, out = state(bridge)
        check(out.flags & rhal.FLAG_FAULT and out.flags & rhal.FLAG_DEGRADED and not out.flags & rhal.FLAG_ESTOP and out.fault_code == 0x0E, "the driver reports FAULT with DEGRADED, and the word is the fault byte")
        rc, said = write(bridge, 0.04)
        check(rc == rhal.SKR_EPERM and "recover" in said and "0x0E" in said, "a command is refused, naming the code and the motion that resets it", said)
        rc, body = ext(bridge, "activate")
        check(rc == rhal.SKR_EPERM and "recover" in str(body), "and activate is refused: the maker's own `activate` returns FaultLatched, and so does this driver's")
        mark = len(decoded(server))
        rc, body = ext(bridge, "recover timeout=10")
        check(rc == rhal.SKR_OK and body["result"] == "activated", "recover runs the maker's reset handshake: rACT falls, which clears the fault in the model, and rises", str(body))
        check(opinion.sees(lambda s: s["gFLT"] == 0 and s["gSTA"] == 3), "and the maker's client reads no fault and an activated gripper")
        rc, out = state(bridge)
        check(out.fault_code == 0 and not out.flags & (rhal.FLAG_FAULT | rhal.FLAG_DEGRADED) and write(bridge, 0.04)[0] == rhal.SKR_OK, "the driver reads healthy and commands again")
        classes_seen: dict[str, int] = {}
        wrong = []
        for byte in [0x00, 0x05, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E, 0x0F, 0x03, 0x40, 0x50, 0x90, 0xC0, 0xE0, 0x10, 0x0E | 0xC0, 0x08 | 0x40]:
            server.fault(byte)
            opinion.sees(lambda s, byte=byte: s["gFLT"] == byte & 0x0F and s["kFLT"] == byte >> 4)
            want_class = frames.fault_class(frames.parse_status(bytes([0xB9, 0, byte, 0, 0, 0])))
            until(lambda byte=byte: state(bridge)[1].fault_code == byte, 2.0)
            rc, out = state(bridge)
            expected = _FLAGS_OF_CLASS[want_class]
            accepted = write(bridge, 0.045 if byte % 2 else 0.035)[0] == rhal.SKR_OK
            classes_seen[want_class] = classes_seen.get(want_class, 0) + 1
            if out.fault_code != byte or (out.flags & (rhal.FLAG_ESTOP | rhal.FLAG_FAULT | rhal.FLAG_DEGRADED)) != expected or accepted != (want_class in ("ok", "degraded")):
                wrong.append((hex(byte), want_class, hex(out.flags), out.fault_code, accepted))
        check(not wrong, f"twenty fault bytes staged one after another, in all four classes {sorted(classes_seen)}: the flags, the word and whether a command is accepted are what the maker's severity says", str(wrong[:3]))
        server.fault(0)
        ext(bridge, "recover timeout=10")
        check(until(lambda: state(bridge)[1].fault_code == 0 and write(bridge, 0.05)[0] == rhal.SKR_OK), "and one recover clears whatever was staged last")

        # ── the wire, as the maker's server logged it ─────────────────────────
        section("the wire — what the maker's server was handed")
        bridge.close()
        rows = decoded(server)
        theirs_peer = rows[0]["peer"]       # the maker's client connected first and read at once: its connection is the first one in the record
        mine_rows = [row for row in rows if row["line"] == "tcp" and row["peer"] != theirs_peer]
        check(all(row["crc"] and row["answered"] for row in rows), f"every one of {len(rows)} requests had a valid CRC and was answered by the maker's server without an exception")
        exchanges = [row for row in rows if row["function"] == frames.FC_READ_WRITE]
        check(bool(exchanges) and all(row["read"] == (frames.STATUS_ADDRESS, 3) and row["write"] == (frames.COMMAND_ADDRESS, 3) for row in exchanges), "every exchange is function 0x17 reading three registers at 2000 and writing three at 1000")
        check(all(row["function"] in (frames.FC_READ_HOLDING, frames.FC_READ_WRITE) and row["read"] == (frames.STATUS_ADDRESS, 3) for row in rows), "and every request of all, the maker's client's and the driver's, read the status block and nothing else: no other address was ever touched")
        check(all(not row["block"][0] & frames.ACTION_RESERVED and row["block"][1] == 0 and row["block"][2] == 0 for row in exchanges), "and none set a reserved bit or a reserved byte")
        check(bool(mine_rows) and mine_rows[0]["function"] == frames.FC_READ_HOLDING and mine_rows[0]["read"] == (frames.STATUS_ADDRESS, 3), "the first thing the driver sent, on its own connection, was a status read: the maker's own `initializeImage`")
        steady = [row["t"] for row in exchanges][-60:]
        gaps = sorted(b - a for a, b in zip(steady, steady[1:]))
        check(len(gaps) > 20 and 0.012 < gaps[len(gaps) // 2] < 0.04, f"and the exchanges were one per period, 50 Hz, whatever was being asked: median gap {1000 * gaps[len(gaps) // 2]:.1f} ms")
        last = exchanges[-1]["t"]
        time.sleep(0.4)
        check([row["t"] for row in decoded(server) if row["function"] == frames.FC_READ_WRITE][-1] == last, "after the close nothing more is written: the maker's client goes on reading, and the driver sends no exchange")

        # ── the same gripper on the pseudo-terminal ───────────────────────────
        section("the pseudo-terminal — the gripper's USB adapter, the same frames")
        bridge, said = opened(device=server.pty)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK) and abs(state(bridge)[1].position_rad[0] - (frames.opening_from_register(register(0.05), P85) or 0)) < 1e-9, "the driver opens the maker's fake on a serial device and reads where it was left")
        check(write(bridge, 0.01)[0] == rhal.SKR_OK and opinion.sees(lambda s: s["gPO"] == register(0.01)), "commands it, and the maker's client (on the gateway) reads the fingers go there")
        bridge.emergency_stop()
        check(opinion.sees(lambda s: s["gGTO"] == 0) and bridge.release_stop() == rhal.SKR_OK, "stops it and releases")
        bridge.close()
        check(any(row["line"] == "pty" for row in decoded(server)) and all(row["crc"] and row["answered"] for row in decoded(server) if row["line"] == "pty"), "every request on the serial device was answered by the maker's server")

        # ── a gateway that goes away, and a gripper that comes back un-activated ──
        section("a gateway that goes away — and a gripper that comes back with no activation")
        bridge, said = opened(host="127.0.0.1", port=server.port, stale_ms=300)
        until(lambda: state(bridge)[0] == rhal.SKR_OK)
        port = server.port
        opinion.stop()
        server.stop()
        check(until(lambda: state(bridge)[0] == rhal.SKR_ETIMEDOUT and state(bridge)[1].flags & rhal.FLAG_DEGRADED, 5.0), "the maker's server killed: the state is ETIMEDOUT and DEGRADED")
        rc, said = write(bridge, 0.03)
        check(rc == rhal.SKR_EIO, "and a command EIO")
        server = Server(port)
        opinion = Opinion(server.port)
        check(until(lambda: state(bridge)[0] == rhal.SKR_OK, 8.0), "a new instance on the same port: the driver connects again by itself")
        rc, out = state(bridge)
        check((out.flags & rhal.HOMING_MASK) == rhal.HOMING_PENDING and out.flags & rhal.FLAG_DEGRADED, "and it reads a gripper that has lost its activation (a fresh model is one that was power-cycled): HOMING_PENDING and DEGRADED")
        rc, said = write(bridge, 0.03)
        check(rc == rhal.SKR_EPERM and "activate" in said, "and a command is refused again, naming the motion that activates", said)
        check(ext(bridge, "activate timeout=10")[0] == rhal.SKR_OK and write(bridge, 0.03)[0] == rhal.SKR_OK, "which activates it, and the command is carried")
        bridge.close()
    finally:
        opinion.stop()
        server.stop()
    print(f"\n{CHECKS} checks, {len(FAILURES)} failed")
    return 1 if FAILURES else 0


if __name__ == "__main__":
    raise SystemExit(main())

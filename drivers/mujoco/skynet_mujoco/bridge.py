"""The MuJoCo simulator RHAL driver — §70.

> MuJoCo and Isaac Sim, and they are not a lesser path — **the sim half of SCTS cannot run without
> them, so they are load-bearing infrastructure.**

| RHAL | MuJoCo |
|---|---|
| `open(config)` | `MjModel.from_xml_path` + `MjData`, and the renderer probe |
| `read_state` | **advance one control period**, then read `qpos`, `qvel`, `actuator_force` |
| `write_command` | write `ctrl` |
| `close` | drop the model |
| `describe` | built from the scene — :mod:`skynet_mujoco.describe` |
| `read_sensor` | render a scene camera from a snapshot, on the driver's own thread — :mod:`skynet_mujoco.render` |
| `emergency_stop` | zero every `ctrl` and latch |

# Who advances time, and why it is the reader

The one decision everything else in this file follows from.

§35.3 makes the simulator's clock *the kernel's* clock, so that a simulation running at a fifth of
wall speed or ten times it is simply the time the whole system is measured in — watchdogs, rate
limits and deadlines included. Without it every simulated run would trip the watchdog in a few
cycles, and the usual workaround, disabling the watchdog in simulation, means the thing being tested
is not the thing that ships.

That leaves the question of what makes time move. **`read_state` does**: each call integrates one
control period with whatever `ctrl` currently holds, and then reports the state that resulted. Not
`write_command`, and the difference matters more than it looks:

* A **stalled policy still has time passing.** The OS keeps reading state at the joint rate, the last
  command keeps being applied by the drives, the state's stamp keeps advancing, and the watchdog
  fires exactly as it would on hardware. Had the world advanced only on a command, a stalled policy
  would freeze the universe and `safety.watchdog.fires` would be untestable in simulation — which is
  half the point of having a simulator driver at all.
* It is what a real robot does. `read_state` on hardware returns a stamp that has moved whether or
  not anybody commanded anything, because the world does not wait.

The consequence, stated so nobody has to derive it from a surprising number: a command written at
cycle *N* is integrated during cycle *N+1*, so `command_latency_ms` is exactly one control period.
That is the one descriptor field a simulator can fill in with more confidence than any hardware
vendor, and it is filled in by construction rather than by measurement.

# What this driver is, and is not, evidence of

It is a **Guest**-tier driver twice over: Python cannot hold a real-time path (§31.3), and a
simulator is forced to Guest by §35.3 so that no simulated run is ever presented as bench evidence.
The descriptor declares `simulator: true` and the flag rides on every check-in and every evaluation
result, where §56.2 turns it into the `simulated` provenance class — eligible, separately ranked,
and never confusable with a field result.

What it *is* evidence of is that a policy, a package, a set of grants and a transport plan compose
into something that runs. That is most of what a driver author needs before there is hardware, and
it is the difference §63.3 draws between turning up to a bench session with the sim suite green and
turning up to discover the shape of the problem.
"""

from __future__ import annotations

import hashlib
import json
import math
import os
import sys
from typing import Any

import skynet_rhal as rhal
from skynet_rhal import Command, Frame, State

from . import describe as describe_mod
from .render import probe_cameras, snapshot_of

__all__ = ["MujocoBridge", "BridgeConfig", "TOOL_VERSION"]

TOOL_VERSION = "0.1.0"

#: Control rate when the configuration names none. 100 Hz is a rate a manipulation policy is
#: routinely trained at and is a whole number of physics steps at MuJoCo's default timestep, which
#: keeps the period exact rather than accumulating a rounding error into the clock.
DEFAULT_RATE_HZ = 100.0

_MODE_NAMES = {
    rhal.MODE_POSITION: "position",
    rhal.MODE_VELOCITY: "velocity",
    rhal.MODE_TORQUE: "torque",
}


class BridgeConfig:
    """What the operator supplies that the scene does not.

    Parsed from the JSON string the OS hands to `open()`:

    ```json
    {"scene": "scenes/so_arm_shaped.xml",
     "rate_hz": 100,
     "base": "wheeled",
     "role_overrides": {"shoulder_pan": "right_arm.shoulder.yaw"},
     "render": {"width": 128, "height": 96},
     "keyframe": "home",
     "hold_gains": {"kp": 40.0, "kd": 2.0}}
    ```

    `hold_gains` is how a torque joint holds (§25: *"a zero mask holds position"*, on a torque joint
    too). One `{"kp", "kd"}` for every torque joint, or a map from joint name to one. They are the
    operator's, as `drivers/unitree`'s are, and never guessed: with none, a masked torque joint is
    refused rather than answered with a free body (`DRY-COVERAGE-PLAN.md` F121, F143).

    Every field is optional except the scene, which may also arrive in `SKYNET_MUJOCO_SCENE` — the
    environment variable exists because `skynet driver check` opens a driver with an empty
    configuration and there is no scene it could sensibly default to.
    """

    def __init__(self, raw: str | None = None) -> None:
        data: dict[str, Any] = {}
        if raw and raw.strip():
            try:
                parsed = json.loads(raw)
                if isinstance(parsed, dict):
                    data = parsed
            except json.JSONDecodeError as error:
                raise ValueError(
                    f"the driver configuration is not JSON: {error}. It is an object and every "
                    'field but `scene` is optional: {"scene": "arm.xml", "rate_hz": 100, '
                    '"base": "wheeled"}'
                ) from error

        self.scene: str | None = data.get("scene") or os.environ.get("SKYNET_MUJOCO_SCENE") or None
        # `or` would be shorter and would turn `"rate_hz": 0` into the default, which is a
        # configuration error silently becoming a working robot at a rate nobody asked for. Absent
        # and zero are different facts and only one of them has a default.
        rate = data.get("rate_hz")
        self.rate_hz: float = DEFAULT_RATE_HZ if rate is None else float(rate)
        self.base: str | None = data.get("base")
        self.role_overrides: dict[str, str] = dict(data.get("role_overrides") or {})
        self.robot_slug: str | None = data.get("robot_slug")
        self.keyframe: str | None = data.get("keyframe")
        self.hold_gains: Any = data.get("hold_gains")

        render = data.get("render") or {}
        if render.get("width") and render.get("height"):
            self.render_size: tuple[int, int] | None = (int(render["width"]), int(render["height"]))
        else:
            self.render_size = None

        if self.rate_hz <= 0:
            raise ValueError(
                f"rate_hz is {self.rate_hz}, and a control rate is a positive number of cycles per "
                "second. It is the unit every deadline in the system is measured in (§35.3)."
            )
        for name, gains in _gain_entries(self.hold_gains):
            kp = gains.get("kp") if isinstance(gains, dict) else None
            kd = gains.get("kd", 0.0) if isinstance(gains, dict) else None
            if not (isinstance(kp, (int, float)) and math.isfinite(kp) and kp > 0.0) or not (
                isinstance(kd, (int, float)) and math.isfinite(kd) and kd >= 0.0
            ):
                raise ValueError(
                    f"hold_gains{'' if name is None else f'[{name!r}]'} is {gains!r}. A hold gain is "
                    '{"kp": <positive>, "kd": <zero or more>}: a stiffness of zero is a free joint, '
                    "which is the thing a hold exists not to be."
                )
        if self.base is not None and self.base not in describe_mod.BASE_KINDS:
            raise ValueError(
                f"base {self.base!r} is not one of {', '.join(describe_mod.BASE_KINDS)}."
            )


class MujocoBridge:
    """A :class:`skynet_rhal.Driver` over a MuJoCo model."""

    def __init__(self, mujoco: Any | None = None) -> None:
        self._mujoco = mujoco
        self._model: Any = None
        self._data: Any = None
        self._config = BridgeConfig()
        self._opened = False
        self._estopped = False
        self._seq = 0
        self._descriptor: str | None = None
        self._describe: Any = None
        self._joints: list[Any] = []
        self._modes: set[str] = set()
        self._cameras: Any = None
        #: The newest state a frame may be rendered from, replaced whole after every step. Written
        #: only by the thread that steps and read by whichever thread asks for a frame, which is the
        #: arrangement §89.1's `concurrent_read` is a promise about — see :mod:`skynet_mujoco.render`.
        self._snapshot: Any = None
        self._substeps = 1
        self._notes: list[Any] = []
        self._warnings: list[int] = []
        self._degraded_reported = False

    # ── Lifecycle ────────────────────────────────────────────────────────────

    def open(self, config: str) -> int:
        try:
            self._config = BridgeConfig(config)
        except ValueError as error:
            print(f"skynet-mujoco: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_EINVAL

        if self._mujoco is None:
            try:
                import mujoco  # type: ignore
            except ImportError:
                print(
                    "skynet-mujoco: MuJoCo is not installed, so there is no simulation to drive. "
                    "`pip install mujoco`. Nothing about this driver works without it and nothing "
                    "it could report instead would be true.",
                    file=sys.stderr,
                    flush=True,
                )
                return rhal.SKR_ENODEV
            except Exception as error:  # noqa: BLE001 — MuJoCo loads a GL backend when imported
                # Found on a machine with Mesa's EGL and no libOSMesa: MuJoCo 3.13 resolves the
                # backend MUJOCO_GL names while it is being imported, so a missing library is not
                # a renderer that fails to probe — which render.py turns into "no cameras" — but a
                # module that never loads, and it arrived here as a PyOpenGL AttributeError out of
                # open() rather than as anything an operator could act on.
                print(
                    f"skynet-mujoco: MuJoCo is installed and would not import "
                    f"({type(error).__name__}: {error}). It loads the OpenGL backend MUJOCO_GL "
                    f"names as it is imported (MUJOCO_GL={os.environ.get('MUJOCO_GL') or 'unset'}), "
                    "so a backend whose library is missing fails here, before any scene is read. "
                    "Install that backend's library — libosmesa6 for osmesa, libegl1 for egl — or "
                    "name one that is installed; unset, MuJoCo tries GLFW and this driver declares "
                    "no cameras if that cannot render.",
                    file=sys.stderr,
                    flush=True,
                )
                return rhal.SKR_ENODEV
            self._mujoco = mujoco

        _assert_enums(self._mujoco)

        if not self._config.scene:
            print(
                "skynet-mujoco: no scene. Set `scene` in the driver configuration or "
                "SKYNET_MUJOCO_SCENE in the environment. There is no default: a simulator driver "
                "with no scene has no body, and inventing one would describe a robot nobody asked "
                "for.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EINVAL

        try:
            with open(self._config.scene, "rb") as handle:
                source = handle.read()
        except OSError as error:
            print(f"skynet-mujoco: cannot read {self._config.scene}: {error}", file=sys.stderr, flush=True)
            return rhal.SKR_ENODEV

        try:
            self._model = self._mujoco.MjModel.from_xml_path(self._config.scene)
            self._data = self._mujoco.MjData(self._model)
        except Exception as error:  # noqa: BLE001 — MuJoCo raises its own error type
            print(
                f"skynet-mujoco: MuJoCo would not compile {self._config.scene}: {error}",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EINVAL

        if self._config.keyframe is not None:
            if not self._load_keyframe(self._config.keyframe):
                return rhal.SKR_EINVAL

        self._mujoco.mj_forward(self._model, self._data)

        # A body opened twice keeps one render thread, not two: the first one's context is freed
        # on the thread that made it before the second is created.
        self._close_cameras()
        cameras = probe_cameras(
            self._mujoco, self._model, self._data, size=self._config.render_size
        )
        self._cameras = cameras
        self._snapshot = snapshot_of(self._data) if cameras.available else None

        result = describe_mod.build_descriptor(
            self._mujoco,
            self._model,
            robot_slug=self._config.robot_slug or _slug(self._model_name()),
            rate_hz=self._config.rate_hz,
            base=self._config.base,
            role_overrides=self._config.role_overrides,
            cameras=cameras.names,
            camera_size=(cameras.width, cameras.height) if cameras.available else None,
            concurrent_read=cameras.concurrent_read,
            scene_source=os.path.basename(self._config.scene),
            scene_sha256=hashlib.sha256(source).hexdigest(),
            tool_version=TOOL_VERSION,
            renderer_note=cameras.note,
        )
        self._describe = result
        self._joints = result.joints
        self._modes = {binding.mode for binding in result.joints}
        self._notes = result.notes

        for note in result.notes:
            print(f"skynet-mujoco: [{note.level}] {note.message}", file=sys.stderr, flush=True)

        if not self._joints:
            print(
                "skynet-mujoco: this scene has no actuator the RHAL can carry, so there is no body "
                "to drive. The notes above name each one and why.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_ENODEV

        if result.free_root_joint is not None and self._config.base is None:
            # The URDF importer's rule, for the same reason: a free-floating root says the machine
            # moves and says nothing about how, and opening it as `fixed` would refuse every
            # base-velocity channel rather than driving it — which presents as a broken policy.
            print(
                "skynet-mujoco: refusing to open. The scene's root body is free-jointed, so this "
                "body moves, and how it moves is not in the file. Set `base`.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EINVAL

        if result.free_root_joint is None and self._config.base not in (None, "fixed"):
            # The converse, and DRY-COVERAGE-PLAN.md F145: the descriptor used to take the declared
            # base verbatim here and its own provenance said "the base is fixed. Nothing was
            # assumed" beside `base: legged` — a body the kernel would ask about balance and the
            # physics bolts to the world. Two statements about one base, and one of them false.
            print(
                f"skynet-mujoco: refusing to open. `base` is {self._config.base!r} and the scene's "
                "root body is fixed to the world, so nothing about this body moves the way the "
                "declaration says. Drop `base`, or give the root a free joint.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EINVAL

        period = 1.0 / self._config.rate_hz
        timestep = float(self._model.opt.timestep)
        self._substeps = max(1, int(round(period / timestep)))
        actual = self._substeps * timestep
        if abs(actual - period) > 1e-12:
            print(
                f"skynet-mujoco: the control period ({period * 1e3:.4f} ms) is not a whole number "
                f"of physics steps ({timestep * 1e3:.4f} ms), so each cycle advances "
                f"{actual * 1e3:.4f} ms instead. The clock stays exact — it is the simulator's — "
                "but the rate is not the one you asked for. Set rate_hz to a divisor of "
                f"{1.0 / timestep:.1f} Hz, or change the scene's timestep.",
                file=sys.stderr,
                flush=True,
            )

        self._warnings = self._warning_counts()
        self._descriptor = result.json()
        self._opened = True
        self._estopped = False
        self._seq = 0
        # §24's hold, per joint: the last position setpoint written, and the target a masked joint
        # is held at (None while the joint is being commanded). Nothing carries into a new session.
        self._position_setpoint: list[float | None] = [None] * len(self._joints)
        self._hold: list[float | None] = [None] * len(self._joints)
        #: Per joint, `(target, kp, kd)` while a torque joint is held, applied before every step.
        self._torque_hold: list[tuple[float, float, float] | None] = [None] * len(self._joints)
        return rhal.SKR_OK

    def close(self) -> int:
        self._opened = False
        self._close_cameras()
        self._model = None
        self._data = None
        self._snapshot = None
        return rhal.SKR_OK

    def _close_cameras(self) -> None:
        if self._cameras is not None:
            self._cameras.close()
            self._cameras = None

    # ── The cycle ────────────────────────────────────────────────────────────

    def read_state(self, out: State, deadline_ns: int) -> int:
        """Advance one control period, then report what resulted.

        The advance is first, deliberately, so the stamp a caller receives is the time of the state
        it is holding rather than the time of the state before it. See the module docstring for why
        the reader is what moves the clock.
        """
        if not self._opened:
            return rhal.SKR_EPERM

        mujoco = self._mujoco
        try:
            for _ in range(self._substeps):
                self._apply_torque_holds()
                mujoco.mj_step(self._model, self._data)
        except Exception as error:  # noqa: BLE001
            print(f"skynet-mujoco: mj_step failed: {error!r}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO

        diverged, degraded = self._check_warnings()
        if diverged is not None:
            # A diverged simulation is the simulator's equivalent of a bus that has stopped making
            # sense, and `SKR_EPROTO` is the RHAL's word for "it answered, and the answer was not
            # valid".
            #
            # Checking the warning counters rather than the numbers is the whole point. **MuJoCo
            # resets itself when it diverges**, so by the time a driver looks at `qpos` it is finite
            # again — the robot appears to have teleported to its home pose, at full speed, with no
            # error anywhere. A driver that only tests for non-finite values reports that as a
            # perfectly good state, and the policy above it is then reacting to a body that jumped.
            print(
                f"skynet-mujoco: the simulation diverged ({diverged}). MuJoCo has reset the state, "
                "so the numbers are finite again and mean nothing; this cycle is not a measurement. "
                "Usually a timestep too large for the scene's stiffest contact, or a command far "
                "outside the model's range.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EPROTO

        if not self._finite_state():
            print(
                "skynet-mujoco: a joint position is not finite and MuJoCo raised no warning. "
                "Nothing past this point is a measurement.",
                file=sys.stderr,
                flush=True,
            )
            return rhal.SKR_EPROTO

        self._seq += 1
        data = self._data
        # One reference replaced, after the state is known to be a measurement and before it is
        # reported — so a frame is only ever rendered from a state some caller could have received.
        if self._snapshot is not None:
            self._snapshot = snapshot_of(data)

        out.stamp_ns = int(round(float(data.time) * 1e9))
        out.seq = self._seq
        out.flags = rhal.FLAG_ESTOP if self._estopped else 0
        # §24's homing field. A simulator's joint positions come straight out of `data.qpos`, so
        # there is nothing whose zero could have been lost and no homing routine to run — the
        # honest answer is `not required` rather than `complete`, which would claim a routine ran.
        out.flags |= rhal.HOMING_NOT_REQUIRED
        # A full contact or constraint buffer means the solver silently dropped contacts: the body
        # may have passed through something. Running, and something is wrong, which is exactly what
        # `degraded` is for.
        if degraded is not None:
            out.flags |= rhal.FLAG_DEGRADED
            if not self._degraded_reported:
                self._degraded_reported = True
                print(
                    f"skynet-mujoco: {degraded}. Contacts were dropped, so collisions in this run "
                    "are not reliable. Raise the scene's buffer sizes.",
                    file=sys.stderr,
                    flush=True,
                )
        out.fault_code = 0
        out.position_rad = [float(data.qpos[b.qpos_adr]) for b in self._joints]
        out.velocity_rad_s = [float(data.qvel[b.dof_adr]) for b in self._joints]
        # `actuator_force` is the scalar force the actuator produced this step, which for a joint
        # transmission is the joint torque. It is a *measurement* of what the simulation applied,
        # not a copy of what was asked for, so a clamped or force-limited actuator reports the
        # smaller number — which is what an effort reading is for.
        out.effort_nm = [float(data.actuator_force[b.actuator_id]) * b.gear for b in self._joints]
        out.base_pose = self._base_pose()
        out.base_twist = self._base_twist()
        return rhal.SKR_OK

    def write_command(self, cmd: Command, deadline_ns: int) -> int:
        if not self._opened:
            return rhal.SKR_EPERM
        if self._estopped:
            return rhal.SKR_EPERM
        if cmd.joint_count != len(self._joints) or len(cmd.mask) != cmd.joint_count:
            return rhal.SKR_EINVAL
        if cmd.mode not in _MODE_NAMES:
            return rhal.SKR_EINVAL

        # §86.1: each joint's mode, the per-joint array resolved against the frame's. An entry the
        # ABI does not define — a bare `MODE_*` written into the wrong array — is refused, never read
        # as the nearest thing it could mean.
        try:
            modes = [cmd.mode_of(index) for index in range(cmd.joint_count)]
        except ValueError:
            return rhal.SKR_EINVAL
        # A mode a joint does not accept is refused rather than approximated. A `<position>`
        # actuator's `ctrl` is a setpoint and a `<motor>`'s is a force; writing one into the other
        # is not a degraded command, it is a different one, and it would be applied silently. Only
        # a joint the command *applies* is asked: a masked joint is held, in its own mode, and a
        # frame's mode says nothing about it — which until F150 made every body with joints in two
        # modes refuse even the kernel's hold.
        for index, binding in enumerate(self._joints):
            if cmd.mask[index] and _MODE_NAMES.get(modes[index]) != binding.mode:
                return rhal.SKR_EINVAL

        # Every value checked before any `ctrl` is written: a command half applied is worse than
        # one refused, and this loop used to write joint by joint and refuse on the first
        # non-finite value, leaving the joints before it commanded.
        for value in cmd.values:
            if not math.isfinite(value):
                return rhal.SKR_EINVAL
        if any(not cmd.mask[i] and b.mode == "torque" for i, b in enumerate(self._joints)):
            # Mask zero means **hold**, and in torque zero is limp, not held. §25 says a zero mask
            # holds position on a torque joint, and a torque joint holds position the way an
            # impedance drive does: a stiffness about a latched target, which is the operator's
            # `hold_gains` — never a default, because a stiffness nobody chose in front of a
            # Safety Kernel that cannot see it is F121's lesson from `drivers/unitree`. With none
            # configured the hold is refused rather than approximated, as it always was.
            unheld = [
                binding.name
                for index, binding in enumerate(self._joints)
                if not cmd.mask[index] and binding.mode == "torque" and self._gains_for(binding.name) is None
            ]
            if unheld:
                print(
                    "skynet-mujoco: a command masks the torque joint(s) "
                    f"{', '.join(unheld)}, and a torque joint holds only with the operator's "
                    "`hold_gains` — zero torque is limp, not held. Refused rather than approximated.",
                    file=sys.stderr,
                    flush=True,
                )
                return rhal.SKR_EINVAL

        for index, binding in enumerate(self._joints):
            if cmd.mask[index]:
                self._data.ctrl[binding.actuator_id] = _to_ctrl(binding, modes[index], cmd.values[index])
                self._hold[index] = None
                self._torque_hold[index] = None
                if modes[index] == rhal.MODE_POSITION:
                    self._position_setpoint[index] = cmd.values[index]
                continue
            # Held, in the joint's own mode: its actuator decides what a hold is, not the frame.
            if binding.mode == "velocity":
                # A velocity actuator holds by commanding zero; there is no position to latch.
                self._data.ctrl[binding.actuator_id] = 0.0
                continue
            if binding.mode == "torque":
                # Latched once, where the joint was when the hold began: a torque joint has no
                # position setpoint, and a target re-read every cycle holds nothing (F121).
                if self._torque_hold[index] is None:
                    kp, kd = self._gains_for(binding.name)
                    self._torque_hold[index] = (float(self._data.qpos[binding.qpos_adr]), kp, kd)
                continue
            # §24: "0 = hold this joint at its current setpoint". A position actuator's `ctrl`
            # *is* its setpoint and persists between writes, so the setpoint the joint was last
            # commanded to is still there. The target is latched when the hold begins — the last
            # position setpoint, or where the joint is when it never had one — and not re-read:
            # writing `qpos` into `ctrl` every cycle zeroes the stiffness term, and a loaded joint
            # then sags a little further each cycle, which is DRY-COVERAGE-PLAN.md F121 on a
            # simulated body (carried here as item 8.13).
            if self._hold[index] is None:
                setpoint = self._position_setpoint[index]
                self._hold[index] = (
                    setpoint if setpoint is not None else float(self._data.qpos[binding.qpos_adr])
                )
            self._data.ctrl[binding.actuator_id] = _to_ctrl(binding, rhal.MODE_POSITION, self._hold[index])

        return rhal.SKR_OK

    def emergency_stop(self) -> int:
        """Zero every actuator command and latch.

        And then the sentence that matters more than the code: **a simulated stop is not a stop.**
        Nothing is cut, no brake engages, no contactor opens, and the arm does not fall or hold
        under gravity because a real one would. What this demonstrates is that the *path* works —
        the latch is observable in the next state, and every later command is refused —
        which is the half of `safety.estop.latency` that can be measured without hardware.
        `safety.estop.independent` asks whether the stop works with the computer switched off, and
        no simulator will ever answer it.
        """
        self._estopped = True
        # A held torque joint is a controller running every step; the stop ends it before the
        # command is zeroed, or the next step would write the hold straight back.
        self._torque_hold = [None] * len(self._joints)
        if self._data is not None:
            for index in range(int(self._model.nu)):
                self._data.ctrl[index] = 0.0
        return rhal.SKR_OK

    def read_sensor(self, name: str, out: Frame) -> int:
        """Render the named camera from the newest snapshot, on the render thread.

        Callable while `read_state` and `write_command` run on another thread — the descriptor
        declares it (§89.1) — because nothing here touches the `MjData` they use: the snapshot is a
        copy, the render thread has a data of its own, and the two references below are read once.
        """
        # Read once each: `close` on the control thread may clear either while this runs.
        cameras = self._cameras
        snapshot = self._snapshot
        if not self._opened:
            return rhal.SKR_EPERM
        if cameras is None or not cameras.available or snapshot is None:
            return rhal.SKR_ENOSYS
        if name not in cameras.names:
            return rhal.SKR_ENODEV

        try:
            payload = cameras.render(name, snapshot)
        except Exception as error:  # noqa: BLE001
            print(f"skynet-mujoco: rendering {name} failed: {error!r}", file=sys.stderr, flush=True)
            return rhal.SKR_EIO

        # The snapshot's stamp, not the clock's now: the pixels are that state, and the control
        # thread may have stepped since the request was made.
        out.stamp_ns = snapshot.stamp_ns
        out.width = cameras.width
        out.height = cameras.height
        out.stride = cameras.width * 3
        out.format = 0  # RGB8
        out.data = payload
        return rhal.SKR_OK

    def describe(self) -> str:
        if self._descriptor is None:
            raise RuntimeError("describe() before open()")
        return self._descriptor

    # ── The parts of the body the scene answers directly ─────────────────────

    def _gains_for(self, joint: str) -> tuple[float, float] | None:
        """The operator's hold gains for one joint, or `None` when none were given for it."""
        for name, gains in _gain_entries(self._config.hold_gains):
            if name is None or name == joint:
                return float(gains["kp"]), float(gains.get("kd", 0.0))
        return None

    def _apply_torque_holds(self) -> None:
        """One step of every torque hold: `kp·(target − q) − kd·q̇`, inside the actuator's range."""
        model, data = self._model, self._data
        for index, held in enumerate(self._torque_hold):
            if held is None:
                continue
            binding = self._joints[index]
            target, kp, kd = held
            torque = kp * (target - float(data.qpos[binding.qpos_adr])) - kd * float(data.qvel[binding.dof_adr])
            torque = _to_ctrl(binding, rhal.MODE_TORQUE, torque)
            if bool(model.actuator_ctrllimited[binding.actuator_id]):
                low, high = model.actuator_ctrlrange[binding.actuator_id]
                torque = min(max(torque, float(low)), float(high))
            data.ctrl[binding.actuator_id] = torque

    def _base_pose(self) -> list[float]:
        """`x y z qx qy qz qw` for the root body, from the simulator's ground truth.

        Exact, noiseless, and available whether or not the base is free — which no physical robot
        has. The provenance block says so, because a policy that quietly learns to depend on a
        perfect base pose has learned something it will not have on hardware, and the day it finds
        out is the day it is on hardware.
        """
        body = self._describe.root_body
        position = [float(v) for v in self._data.xpos[body]]
        w, x, y, z = (float(v) for v in self._data.xquat[body])
        return position + [x, y, z, w]

    def _base_twist(self) -> list[float]:
        """`vx vy vz wx wy wz`.

        From the root's free joint when it has one. A fixed base does not move, and reporting zeros
        for it is a measurement rather than a placeholder. A base that moves by some other
        arrangement — a slide joint, a mocap weld — reports zeros too, and that is a limitation
        rather than a fact about the body; the scene would have to say more for this to be better.
        """
        free = self._describe.free_root_joint
        if free is None:
            return [0.0] * 6
        adr = int(self._model.jnt_dofadr[free])
        return [float(self._data.qvel[adr + i]) for i in range(6)]

    def _warning_counts(self) -> list[int]:
        return [int(self._data.warning[i].number) for i in range(int(self._mujoco.mjtWarning.mjNWARNING))]

    def _check_warnings(self) -> tuple[str | None, str | None]:
        """What MuJoCo complained about during the step just taken.

        Returns `(diverged, degraded)`, either of which may be `None`. Counters rather than flags:
        MuJoCo accumulates warnings for the lifetime of the data, so the question is always "did
        this go up during my step", never "is it set".
        """
        mujoco = self._mujoco
        counts = self._warning_counts()
        previous, self._warnings = self._warnings, counts

        diverged: str | None = None
        degraded: str | None = None
        for index, (was, now) in enumerate(zip(previous, counts)):
            if now <= was:
                continue
            name = mujoco.mjtWarning(index).name
            if index in (
                int(mujoco.mjtWarning.mjWARN_BADQPOS),
                int(mujoco.mjtWarning.mjWARN_BADQVEL),
                int(mujoco.mjtWarning.mjWARN_BADQACC),
                int(mujoco.mjtWarning.mjWARN_BADCTRL),
            ):
                diverged = name
            else:
                degraded = name
        return diverged, degraded

    def _finite_state(self) -> bool:
        for binding in self._joints:
            if not math.isfinite(float(self._data.qpos[binding.qpos_adr])):
                return False
        return True

    def _load_keyframe(self, name: str) -> bool:
        index = self._mujoco.mj_name2id(self._model, self._mujoco.mjtObj.mjOBJ_KEY, name)
        if index < 0:
            available = [
                self._mujoco.mj_id2name(self._model, self._mujoco.mjtObj.mjOBJ_KEY, i)
                for i in range(int(self._model.nkey))
            ]
            print(
                f"skynet-mujoco: the scene has no keyframe named {name!r}. It has: "
                f"{', '.join(n for n in available if n) or '(none)'}.",
                file=sys.stderr,
                flush=True,
            )
            return False
        self._mujoco.mj_resetDataKeyframe(self._model, self._data, index)
        return True

    def _model_name(self) -> str:
        """The scene's `<mujoco model=...>` name.

        MuJoCo packs every name in the model into one null-separated block and the model's own is
        first. Read defensively: the slug is a convenience and never a correctness input, and a
        driver that refused to open because it could not name a robot would be absurd.
        """
        try:
            names = self._model.names
            raw = bytes(names) if not isinstance(names, bytes) else names
            return raw.split(b"\x00", 1)[0].decode("ascii", "replace") or "mujoco-scene"
        except Exception:  # noqa: BLE001
            return "mujoco-scene"

    # ── For the harness ──────────────────────────────────────────────────────

    @property
    def notes(self) -> list[Any]:
        """The generator's report, for a caller that wants to print it its own way."""
        return list(self._notes)

    @property
    def joint_names(self) -> list[str]:
        return [binding.name for binding in self._joints]


def _assert_enums(mujoco: Any) -> None:
    """Check the enum values :mod:`skynet_mujoco.describe` writes out against the library's.

    Those constants are written out so the module can be read without MuJoCo installed. Written-out
    constants are exactly the thing that rots silently, and a hinge read as a slide is a joint whose
    limits are in the wrong unit — so they are checked once, at open, against the library actually
    loaded.
    """
    expected = {
        "mjJNT_FREE": describe_mod.JNT_FREE,
        "mjJNT_BALL": describe_mod.JNT_BALL,
        "mjJNT_SLIDE": describe_mod.JNT_SLIDE,
        "mjJNT_HINGE": describe_mod.JNT_HINGE,
    }
    for name, value in expected.items():
        actual = int(getattr(mujoco.mjtJoint, name))
        if actual != value:
            raise RuntimeError(
                f"MuJoCo's {name} is {actual} and this driver was written against {value}. "
                "Joint kinds have been renumbered; the constants in skynet_mujoco.describe must be "
                "updated before anything here can be trusted."
            )
    pairs = [
        ("mjtTrn", "mjTRN_JOINT", describe_mod.TRN_JOINT),
        ("mjtTrn", "mjTRN_JOINTINPARENT", describe_mod.TRN_JOINTINPARENT),
        ("mjtBias", "mjBIAS_NONE", describe_mod.BIAS_NONE),
        ("mjtBias", "mjBIAS_AFFINE", describe_mod.BIAS_AFFINE),
        ("mjtDyn", "mjDYN_NONE", describe_mod.DYN_NONE),
    ]
    for enum_name, member, value in pairs:
        actual = int(getattr(getattr(mujoco, enum_name), member))
        if actual != value:
            raise RuntimeError(
                f"MuJoCo's {member} is {actual} and this driver was written against {value}."
            )


def _to_ctrl(binding: Any, mode: int, value: float) -> float:
    """A joint-unit value as the actuator's `ctrl`: through the gear, which is what `ctrl` is in.

    A joint transmission acts on `gear · q`, so a position or velocity setpoint is multiplied by the
    gear and a torque divided by it. Gear one — every scene written here, and most of the menagerie
    — is the identity, which is how every geared body in the sweep was commanded off by its gear
    for as long as this was missing (F148).
    """
    if mode == rhal.MODE_TORQUE:
        return value / binding.gear
    return value * binding.gear


def _gain_entries(raw: Any) -> list[tuple[str | None, Any]]:
    """`hold_gains` as `(joint or None for every joint, gains)` pairs."""
    if raw is None:
        return []
    if isinstance(raw, dict) and ("kp" in raw or "kd" in raw):
        return [(None, raw)]
    if isinstance(raw, dict):
        return [(str(name), gains) for name, gains in raw.items()]
    return [(None, raw)]


def _slug(text: str) -> str:
    out: list[str] = []
    pending = False
    for ch in text:
        if ch.isalnum():
            if pending and out:
                out.append("-")
            pending = False
            out.append(ch.lower())
        else:
            pending = True
    return "".join(out) or "mujoco-scene"

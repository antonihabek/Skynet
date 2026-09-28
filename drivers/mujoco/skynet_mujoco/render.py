"""Camera frames: the probe that decides whether to declare any, and the thread that renders them.

MuJoCo renders through OpenGL, and a headless machine may have no context to render into. That is
not a rare edge: a CI runner, a container, and a robot's own compute are all likely to be exactly
that machine.

**So the driver probes before it declares.** If a renderer can be constructed and a frame comes back,
the scene's cameras are declared as sensors. If it cannot, the descriptor declares **no sensors** and
`read_sensor` reports `SKR_ENOSYS`, and the reason goes to stderr where an operator will see it.

The alternative — declaring the cameras and failing the reads — has already been tried in this
repository, by accident, in the LeRobot bridge, and `skynet driver check` caught it with the sentence
that is now the rule: *a declared stream that cannot be read binds a policy's observation to
nothing*. A policy admitted against an observation space it will never receive is worse than one
refused at install time, because the refusal happens on a workstation and the starvation happens on
a robot.

The probe is done once, at `open`, and never repeated for that body. A renderer that worked at open
and fails mid-run is a read error, not a re-negotiation of the body: the descriptor is the source of
truth for what this body is (§4) and it does not change while the body is open.

**And a failed probe is never retried in the same process at all**, which was found the hard way.
MuJoCo's GL context is process-global: the first attempt on a machine with no context raises, which
is fine, and the *second* one aborts the interpreter from inside the native library — no exception,
no traceback, SIGABRT. A supervisor restarting a body, or a harness opening two of them, would take
the whole process down with it. So whether this machine can render is decided once and remembered:
it is a property of the process, not of the body.

# Rendered on a thread of the driver's own, from a snapshot — and so declared `concurrent_read`

`DRY-COVERAGE-PLAN.md` F24 measured what rendering on the control thread costs: 18.2 ms a frame
through a software rasteriser, a 20 ms period, and one overrun for every frame read. §89.1 gave a
driver the word for the fix — `concurrent_read`, the promise that `read_sensor` may run on another
thread while `read_state` and `write_command` do, without making them wait — and
`docs/issues/sensor-reads-off-the-control-thread.md` said what this driver would need before it could
make it: *"rendering while another thread steps is a data race inside MuJoCo. It would need to
render from a scene snapshot."* This module is that.

* **The control thread publishes a :class:`Snapshot` and never reads anything back.** After every
  step `read_state` copies the handful of arrays a picture depends on and replaces one reference —
  a single store, so there is no lock for it to wait on and nothing the render side can hold.
* **The render thread owns everything it touches.** Its own `MjData`, into which a snapshot is
  written and through which `mj_forward` recomputes the geometry, and the GL context, which is
  created, made current, used and freed on that thread and no other. An `MjModel` is read-only
  during simulation, so sharing it is MuJoCo's own threading arrangement rather than an assumption.
* **Nothing but rendering happens there**, and `read_sensor` is its only client. A frame is
  requested, rendered from the newest snapshot, and handed back stamped with that snapshot's time,
  so the stamp is the state the pixels show — the property the on-control-thread version had, kept.

**What makes the promise honest in Python is a measurement, not the design.** Two threads in one
CPython process do not run bytecode at once, and F46 recorded that §89.1's *"without making them
wait"* is honest only for a read that spends its time in native code that releases the GIL. A
MuJoCo render does: measured 2026-09-18 on a WSL2 machine, a render on this thread took 14.4 ms
through llvmpipe and raised the slowest ten-substep `mj_step` burst on the control thread from
0.49 ms to 2.1 ms — the Python between the native calls, not the render. A future binding that held
the GIL through `mjr_render` would make this declaration false, and `check-loop-mujoco`'s
zero-overrun assertion is what would say so.
"""

from __future__ import annotations

import queue
import threading
from typing import Any

__all__ = ["CameraSet", "Snapshot", "probe_cameras", "snapshot_of"]

#: What to render at when the scene's camera declares no resolution of its own. Small on purpose —
#: this is a pipe to a C host (§4's Python path), not a capture pipeline, and §47.2 is where a real
#: camera belongs.
DEFAULT_SIZE = (128, 96)

#: The largest RGB8 frame the RHAL pipe carries, in bytes. The host reads the base64 payload into
#: a 64 KiB line (`SKPH_LINE_MAX`), so 65,535 characters decode to 49,149 bytes, and the shim
#: refuses anything past it before it is framed (`skynet_rhal._serve_frame`), with the stream left
#: in sync (§95). This bound is what a scene resolution is clamped to; the shim stays the backstop,
#: so a second spelling of the number here cannot desynchronise anything if it ever disagrees.
_RHAL_FRAME_BYTES_MAX = (65535 // 4) * 3

#: How long `open` waits for the render thread to build a context and render its first frame. A
#: context that has not answered in this long is one this driver does not declare a camera on: a
#: machine that hangs creating a GL context is a machine that cannot render, and `open` returning is
#: worth more than the camera.
PROBE_TIMEOUT_S = 30.0

#: How long one read waits for its frame before the camera path is declared broken for the rest of
#: the body's life. Far above any render this driver has measured (18.2 ms in software), so it is
#: never a latency budget — it is the bound on a hang, because a read that never returns would stall
#: the runtime's capture thread and, through it, a clean shutdown (§89.3).
RENDER_TIMEOUT_S = 5.0

#: Modules whose renderer has already failed in this process, keyed by identity and holding a strong
#: reference so the identity cannot be reused. Keyed by module rather than global so that a test
#: driving a stand-in cannot decide the answer for the real library, or the reverse.
_PROBE_FAILED: dict[int, tuple[Any, str]] = {}


class Snapshot:
    """The state one frame is rendered from, copied off the control thread's `MjData`.

    Immutable once built. The copies are what separate the two threads: the physics keeps writing
    its own arrays, and a snapshot taken between two steps is never torn by the next one.
    """

    __slots__ = ("stamp_ns", "time", "qpos", "mocap_pos", "mocap_quat")

    def __init__(self, stamp_ns: int, time: float, qpos: Any, mocap_pos: Any, mocap_quat: Any) -> None:
        self.stamp_ns = stamp_ns
        self.time = time
        self.qpos = qpos
        self.mocap_pos = mocap_pos
        self.mocap_quat = mocap_quat


def snapshot_of(data: Any) -> Snapshot:
    """Copy what a picture of `data` depends on: the pose, and the mocap bodies that have one.

    Everything a render reads — body, geom, camera and light placement — is recomputed from `qpos`
    and the mocap poses by `mj_forward` on the render thread, so these three arrays and the clock are
    the whole of it. Velocities, forces and controls change nothing a camera sees.
    """
    time = float(data.time)
    return Snapshot(
        stamp_ns=int(round(time * 1e9)),
        time=time,
        qpos=data.qpos.copy(),
        mocap_pos=data.mocap_pos.copy(),
        mocap_quat=data.mocap_quat.copy(),
    )


class CameraSet:
    """The cameras this driver will actually serve, and why it is not serving the others."""

    def __init__(
        self,
        *,
        names: list[str],
        renderer: _RenderThread | None,
        width: int,
        height: int,
        note: str,
    ) -> None:
        self.names = names
        self.renderer = renderer
        self.width = width
        self.height = height
        self.note = note
        #: Serialises `render`. §89.1 promises a driver that `read_sensor` is never called
        #: concurrently with itself, and this is the driver not relying on that for the pairing of
        #: a request with its reply — a promise kept by somebody else is not an invariant here.
        self._lock = threading.Lock()
        #: Set once a read has timed out; every later read is refused with it rather than queued
        #: behind a thread that stopped answering.
        self.broken: str | None = None

    @property
    def available(self) -> bool:
        return self.renderer is not None and bool(self.names)

    @property
    def concurrent_read(self) -> bool:
        """§89.1's declaration, and it is true exactly when there is a render thread to make it so."""
        return self.available

    def render(self, name: str, snapshot: Snapshot) -> bytes:
        """One RGB8 frame from the named camera, rendered from `snapshot` on the render thread.

        Raises on a failed render, and on one that did not come back within
        :data:`RENDER_TIMEOUT_S` — after which every read raises the same thing.
        """
        assert self.renderer is not None
        with self._lock:
            if self.broken is not None:
                raise RuntimeError(self.broken)
            try:
                return self.renderer.render(name, snapshot, RENDER_TIMEOUT_S)
            except TimeoutError:
                self.broken = (
                    f"the render thread did not return a frame within {RENDER_TIMEOUT_S:.0f} s, so "
                    "the camera is not read again for the life of this body. A read that never "
                    "returns would stall the runtime's capture thread; a refused one is a gap it "
                    "can annotate (§47.4)."
                )
                raise RuntimeError(self.broken) from None

    def close(self) -> None:
        if self.renderer is not None:
            self.renderer.stop()
            self.renderer = None


class _RenderThread:
    """The one thread that ever touches the GL context, and the `MjData` frames are rendered from.

    A GL context is current on one thread at a time and MuJoCo's `Renderer` makes its context
    current on whichever thread calls it, so the context is created here, used here and freed here —
    and a caller on any other thread asks rather than renders.
    """

    def __init__(self, mujoco: Any, model: Any, width: int, height: int) -> None:
        self._mujoco = mujoco
        self._model = model
        self._width = width
        self._height = height
        self._requests: queue.SimpleQueue = queue.SimpleQueue()
        self._replies: queue.SimpleQueue = queue.SimpleQueue()
        self._thread = threading.Thread(
            target=self._run, name="skynet-mujoco render", daemon=True
        )

    def start(self, timeout_s: float) -> None:
        """Start the thread and wait for its renderer. Raises whatever constructing it raised."""
        self._thread.start()
        try:
            ready = self._replies.get(timeout=timeout_s)
        except queue.Empty:
            raise TimeoutError(
                f"no GL context within {timeout_s:.0f} s — the render thread is still trying"
            ) from None
        if isinstance(ready, BaseException):
            raise ready

    def render(self, name: str, snapshot: Snapshot, timeout_s: float) -> bytes:
        self._requests.put((name, snapshot))
        try:
            reply = self._replies.get(timeout=timeout_s)
        except queue.Empty:
            raise TimeoutError from None
        if isinstance(reply, BaseException):
            raise reply
        return reply

    def stop(self) -> None:
        """Ask the thread to free its context and end, and wait for it if it is answering.

        Not waited on for longer than a render could take: a thread that is hung inside the native
        library cannot be interrupted from here, and `close` returning is what a supervisor needs.
        It is a daemon, so a hung one does not hold the interpreter open either.
        """
        if self._thread.is_alive():
            self._requests.put(None)
            self._thread.join(timeout=RENDER_TIMEOUT_S)

    def _run(self) -> None:
        mujoco = self._mujoco
        try:
            renderer = mujoco.Renderer(self._model, self._height, self._width)
        except Exception as error:  # noqa: BLE001 — every GL failure mode arrives as a different type
            self._replies.put(error)
            return
        try:
            scratch = mujoco.MjData(self._model)
        except Exception as error:  # noqa: BLE001
            renderer.close()
            self._replies.put(error)
            return
        self._replies.put(None)

        try:
            while True:
                request = self._requests.get()
                if request is None:
                    return
                name, snapshot = request
                try:
                    scratch.time = snapshot.time
                    scratch.qpos[:] = snapshot.qpos
                    scratch.mocap_pos[:] = snapshot.mocap_pos
                    scratch.mocap_quat[:] = snapshot.mocap_quat
                    # The render thread's own `MjData`, so this recomputes geometry the physics is
                    # not reading — never the control thread's, which is the data race §89.1's
                    # declaration would otherwise be a promise about.
                    mujoco.mj_forward(self._model, scratch)
                    renderer.update_scene(scratch, camera=name)
                    image = renderer.render()
                    self._replies.put(
                        bytes(memoryview(image).tobytes() if hasattr(image, "tobytes") else image)
                    )
                except Exception as error:  # noqa: BLE001 — reported to the reader, who owns the rc
                    self._replies.put(error)
        finally:
            renderer.close()


def probe_cameras(
    mujoco: Any, model: Any, data: Any, *, size: tuple[int, int] | None
) -> CameraSet:
    """Start a render thread and render one frame on it. Declare cameras only if that worked."""
    names: list[str] = []
    first = 0
    for index in range(model.ncam):
        name = mujoco.mj_id2name(model, mujoco.mjtObj.mjOBJ_CAMERA, index)
        if name:
            if not names:
                first = index
            names.append(name)

    if not names:
        return CameraSet(
            names=[],
            renderer=None,
            width=0,
            height=0,
            note="The scene declares no cameras, so this body has no sensors the RHAL can carry.",
        )

    (width, height), sized_note = _rhal_size(size, model, first)

    remembered = _PROBE_FAILED.get(id(mujoco))
    if remembered is not None:
        return CameraSet(names=[], renderer=None, width=0, height=0, note=remembered[1])

    thread = _RenderThread(mujoco, model, width, height)
    try:
        thread.start(PROBE_TIMEOUT_S)
        thread.render(names[0], snapshot_of(data), PROBE_TIMEOUT_S)
    except Exception as error:  # noqa: BLE001 — every GL failure mode arrives as a different type
        thread.stop()
        note = (
            f"The scene declares {len(names)} camera(s) and none is declared as a sensor: this "
            f"machine could not render one ({type(error).__name__}: {error}). MuJoCo needs an "
            "OpenGL context — set MUJOCO_GL=osmesa or =egl and install the matching library. "
            "Declaring a stream nothing can read would bind a policy's observation to nothing, "
            "so the descriptor declares none and read_sensor reports ENOSYS."
        )
        # Remembered, and not retried. A second attempt after this one aborts the process from
        # inside MuJoCo rather than raising.
        _PROBE_FAILED[id(mujoco)] = (mujoco, note)
        return CameraSet(names=[], renderer=None, width=0, height=0, note=note)

    note = (
        f"{len(names)} camera(s), rendered on demand at {width}x{height} RGB8 by MuJoCo's "
        "offscreen renderer, on a thread of this driver's own and from a snapshot of the exact "
        "state the frame is stamped with — so each is declared `concurrent_read` (§89.1). A "
        "simulated frame has no acquisition latency and no noise, and a policy tuned against "
        "one has been tuned against neither."
    )
    if sized_note is not None:
        note = f"{note} {sized_note}"
    return CameraSet(
        names=names,
        renderer=thread,
        width=width,
        height=height,
        note=note,
    )


def _rhal_size(
    size: tuple[int, int] | None, model: Any, index: int
) -> tuple[tuple[int, int], str | None]:
    """The size to render at, and a note when the pipe decided it rather than the scene.

    A scene resolution whose RGB8 frame the RHAL pipe cannot carry is rendered at
    :data:`DEFAULT_SIZE` instead, and the note names both sizes — a small readable frame beats a
    declared stream nothing can read (DRY-COVERAGE-PLAN.md F172). An explicit `render` size is the
    operator's word and is honoured as given; when it does not fit, the note says reads will be
    refused, and the shim refuses them with the stream left in sync.

    Pure — no GL — so the self-test holds it without a context.
    """
    if size is not None:
        if size[0] * size[1] * 3 <= _RHAL_FRAME_BYTES_MAX:
            return size, None
        return size, (
            f"The configured render size is {size[0]}x{size[1]}, whose frame this pipe cannot "
            "carry, so reads will be refused rather than framed."
        )
    width, height = _scene_size(model, index)
    if width * height * 3 <= _RHAL_FRAME_BYTES_MAX:
        return (width, height), None
    return DEFAULT_SIZE, (
        f"The scene's camera resolves at {width}x{height}, whose RGB8 frame this pipe cannot "
        f"carry, so it is rendered at {DEFAULT_SIZE[0]}x{DEFAULT_SIZE[1]}."
    )


def _scene_size(model: Any, index: int) -> tuple[int, int]:
    """The camera's own resolution when the scene declares one, else :data:`DEFAULT_SIZE`.

    MuJoCo writes `[1, 1]` for a camera with no `resolution` attribute, which is not a resolution
    anybody meant. One size serves every camera in the scene: the renderer is allocated once, and
    two cameras at two sizes would need two of them.
    """
    try:
        width, height = (int(v) for v in model.cam_resolution[index])
    except Exception:  # noqa: BLE001 — an older MuJoCo without cam_resolution
        return DEFAULT_SIZE
    if width > 1 and height > 1:
        return width, height
    return DEFAULT_SIZE

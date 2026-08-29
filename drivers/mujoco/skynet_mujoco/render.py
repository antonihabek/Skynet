"""Camera frames, and the probe that decides whether to declare any.

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
"""

from __future__ import annotations

from typing import Any

__all__ = ["CameraSet", "probe_cameras"]

#: What to render at when the scene's camera declares no resolution of its own. Small on purpose —
#: this is a pipe to a C host (§4's Python path), not a capture pipeline, and §47.2 is where a real
#: camera belongs.
DEFAULT_SIZE = (128, 96)

#: Modules whose renderer has already failed in this process, keyed by identity and holding a strong
#: reference so the identity cannot be reused. Keyed by module rather than global so that a test
#: driving a stand-in cannot decide the answer for the real library, or the reverse.
_PROBE_FAILED: dict[int, tuple[Any, str]] = {}


class CameraSet:
    """The cameras this driver will actually serve, and why it is not serving the others."""

    def __init__(
        self,
        *,
        names: list[str],
        renderer: Any | None,
        width: int,
        height: int,
        note: str,
    ) -> None:
        self.names = names
        self.renderer = renderer
        self.width = width
        self.height = height
        self.note = note

    @property
    def available(self) -> bool:
        return self.renderer is not None and bool(self.names)

    def render(self, mujoco: Any, model: Any, data: Any, name: str) -> bytes:
        """One RGB8 frame from the named camera, at the state `data` currently holds.

        The scene is updated from `data` on every call rather than cached, because a cached scene
        renders the robot where it *was*, and an observation that lags the state it is paired with is
        the failure §25.1 spends a page on.
        """
        assert self.renderer is not None
        self.renderer.update_scene(data, camera=name)
        image = self.renderer.render()
        return bytes(memoryview(image).tobytes() if hasattr(image, "tobytes") else image)


def probe_cameras(mujoco: Any, model: Any, data: Any, *, size: tuple[int, int] | None) -> CameraSet:
    """Try to build a renderer and render one frame. Declare cameras only if that worked."""
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

    width, height = size if size else _scene_size(model, first)

    remembered = _PROBE_FAILED.get(id(mujoco))
    if remembered is not None:
        return CameraSet(names=[], renderer=None, width=0, height=0, note=remembered[1])

    try:
        renderer = mujoco.Renderer(model, height, width)
        renderer.update_scene(data, camera=names[0])
        renderer.render()
    except Exception as error:  # noqa: BLE001 — every GL failure mode arrives as a different type
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

    return CameraSet(
        names=names,
        renderer=renderer,
        width=width,
        height=height,
        note=(
            f"{len(names)} camera(s), rendered on demand at {width}x{height} RGB8 by MuJoCo's "
            "offscreen renderer, from the exact state the frame is stamped with. A simulated frame "
            "has no acquisition latency and no noise, and a policy tuned against one has been "
            "tuned against neither."
        ),
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

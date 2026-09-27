"""A URDF becomes a body the MuJoCo driver can open — DRY-COVERAGE-PLAN.md Phase 9.

A URDF describes a robot's kinematics and inertia and says nothing about how it is driven: there is
no actuator in the format. `drivers/mujoco` (§70) needs one per joint it serves, because the RHAL's
arrays pair a joint with the thing that commands it. So the sweep writes the missing half, once,
here, by a rule that is the same for every model and is stated on every report:

* **A critically damped position servo on every actuated joint**, tuned from the model's own mass
  matrix at its zero pose — `kp = I·ω²`, `kv = 2·I·ω`, `ω = 2π·SERVO_HZ` — so a finger and a hip
  are the same servo relative to what they move. Nothing about it is the vendor's controller.
* **Its force limit is the URDF's own `effort`**, where the file gives one, so a model whose
  declared effort cannot move its own link says so as motion that does not happen.
* **Gravity compensated**, every body, because every controller on this list compensates gravity
  and a servo sagging under a load it was never tuned for is a statement about this file's gains,
  not about the robot.
* **No contacts.** A URDF's collision meshes routinely interpenetrate at the zero pose, and a solver
  resolving that is a simulation fault the robot does not have. Geometry is kept, so the descriptor
  still carries collision shapes; nothing collides with anything.
* **The root is fixed.** A URDF with no floating joint is fixed to the world by MuJoCo, and that is
  how a humanoid's or a quadruped's file is simulated here: a body on a stand, every joint free to
  be commanded. A sweep that balanced a biped would be measuring a balance controller.
* **A `<mimic>` joint gets no actuator**, and follows its driver through the equality MuJoCo's URDF
  parser writes for it — the same reading `crates/skynet-urdf` gives it: a linkage, not a degree of
  freedom.

What the file cannot supply is dropped and named, never guessed: a mesh MuJoCo cannot read (Collada,
or a path nothing on disk answers) loses its geometry, and every visual is discarded. Two defects in
published files are repaired rather than refused, because each is local to one link and the repair
is stated on the report row: an inertia tensor that is not positive definite — which no rigid body
has — is replaced by the magnitudes of its diagonal, and a primitive with a zero or negative size
loses its geometry. Everything else MuJoCo refuses is refused, in MuJoCo's words.

The output is an MJCF the driver loads unmodified. It is written to disk rather than handed over in
memory because `skynet driver check` and `skynetk` open the driver in another process, which is the
point — the body crosses the real ABI.
"""

from __future__ import annotations

import math
import os
import re
import xml.etree.ElementTree as ET
from typing import Any

import numpy

__all__ = ["SERVO_HZ", "BuildError", "build_from_urdf", "package_index", "resolve_mesh"]

#: The servo bandwidth every generated actuator is tuned to. Eight hertz at a 2 ms step with the
#: implicit integrator is stable for every mass matrix on the list and fast enough that a 0.3 rad
#: step settles inside a second — which is all the sweep asks of it.
SERVO_HZ = 8.0

#: Mesh formats MuJoCo reads. Collada (`.dae`) is the common one it does not.
READABLE_MESHES = (".stl", ".obj", ".msh")

#: MuJoCo's STL decoder refuses a mesh with more faces than this, and the whole model with it.
MAX_FACES = 200_000


class BuildError(Exception):
    """The file could not be made into a body, and why — carried to the report verbatim."""


def package_index(root: str) -> dict[str, str]:
    """Every ROS package under `root`, by the name its `package.xml` declares, then by directory.

    `package://name/path` is how a URDF names a mesh, and `name` is a *package* name, which is not
    always the directory it lives in. The declared name wins; the directory name is the fallback for
    the repositories that ship a description without a manifest.
    """
    by_manifest: dict[str, str] = {}
    by_directory: dict[str, str] = {}
    for current, dirs, files in os.walk(root):
        dirs[:] = [d for d in dirs if not d.startswith(".")]
        by_directory.setdefault(os.path.basename(current), current)
        if "package.xml" in files:
            try:
                text = open(os.path.join(current, "package.xml"), encoding="utf-8").read()
            except OSError:
                continue
            found = re.search(r"<name>\s*([^<\s]+)\s*</name>", text)
            if found:
                by_manifest.setdefault(found.group(1), current)
    return {**by_directory, **by_manifest}


def resolve_mesh(filename: str, urdf_dir: str, root: str, packages: dict[str, str]) -> str | None:
    """The file on disk a URDF mesh reference means, or `None` when nothing answers it.

    Tried in order: the package the URI names; the path relative to the URDF; the path relative to
    the repository; and last, any file under the repository whose path ends with the reference's
    tail. The last is a search rather than a resolution, and it exists because a large share of the
    public files name packages by a directory that was renamed when they were published.
    """
    if filename.startswith("file://"):
        filename = filename[len("file://"):]
    candidates: list[str] = []
    tail = filename
    if filename.startswith("package://"):
        rest = filename[len("package://"):]
        package, _, tail = rest.partition("/")
        if package in packages:
            candidates.append(os.path.join(packages[package], tail))
    else:
        candidates.append(os.path.join(urdf_dir, filename))
        candidates.append(os.path.join(root, filename))
    for candidate in candidates:
        if os.path.isfile(candidate):
            return os.path.abspath(candidate)
    parts = [p for p in tail.split("/") if p]
    for depth in (3, 2, 1):
        if len(parts) < depth:
            continue
        suffix = os.sep + os.path.join(*parts[-depth:])
        for current, dirs, files in os.walk(root):
            dirs[:] = [d for d in dirs if not d.startswith(".")]
            if parts[-1] in files and os.path.join(current, parts[-1]).endswith(suffix):
                return os.path.abspath(os.path.join(current, parts[-1]))
    return None


def _prepared_urdf(urdf_path: str, root: str) -> tuple[str, list[str]]:
    """The URDF with its visuals gone and every collision mesh resolved or removed."""
    notes: list[str] = []
    try:
        tree = ET.parse(urdf_path)
    except ET.ParseError as error:
        raise BuildError(f"the URDF is not well-formed XML: {error}") from None
    robot = tree.getroot()
    if robot.tag != "robot":
        raise BuildError(f"the root element is <{robot.tag}>, not <robot>")
    packages = package_index(root)
    urdf_dir = os.path.dirname(os.path.abspath(urdf_path))
    dropped: dict[str, int] = {}
    for link in robot.iter("link"):
        inertial = link.find("inertial")
        mass = inertial.find("mass") if inertial is not None else None
        if inertial is not None and mass is not None and 0.0 <= _number(mass.get("value")) < 1e-9:
            # A placeholder: a sensor frame given a mass of femtograms so a tool would accept it.
            # Removed, so the link is massless — which is what it is — rather than a tensor whose
            # eigenvalues MuJoCo cannot resolve from rounding error.
            link.remove(inertial)
            notes.append(f"link `{link.get('name')}` declares a mass below a nanogram; treated as massless")
            inertial = None
        inertia = inertial.find("inertia") if inertial is not None else None
        if inertia is not None and not _physical(inertia):
            for key in ("ixy", "ixz", "iyz"):
                inertia.set(key, "0")
            for key in ("ixx", "iyy", "izz"):
                inertia.set(key, repr(max(abs(_number(inertia.get(key))), 1e-8)))
            notes.append(
                f"link `{link.get('name')}` declares an inertia with a non-positive eigenvalue, which "
                "no rigid body has; its diagonal magnitudes were used"
            )
        for collision in list(link.findall("collision")):
            primitive = collision.find("geometry")
            shape = primitive[0] if primitive is not None and len(primitive) else None
            if shape is not None and shape.tag in ("box", "cylinder", "sphere"):
                sizes = [_number(v) for v in (shape.get("size") or "").split()] or [
                    _number(shape.get(key)) for key in ("radius", "length") if shape.get(key) is not None
                ]
                if not sizes or any(size <= 0.0 for size in sizes):
                    link.remove(collision)
                    dropped["a primitive of zero size"] = dropped.get("a primitive of zero size", 0) + 1
        for visual in list(link.findall("visual")):
            link.remove(visual)
        for collision in list(link.findall("collision")):
            mesh = collision.find("geometry/mesh")
            if mesh is None:
                continue
            reference = mesh.get("filename", "")
            resolved = resolve_mesh(reference, urdf_dir, root, packages) if reference else None
            reason = None
            if resolved is None:
                reason = "not found"
            elif not resolved.lower().endswith(READABLE_MESHES):
                reason = os.path.splitext(resolved)[1].lower() or "no extension"
            elif _faces(resolved) > MAX_FACES:
                reason = f"over MuJoCo's {MAX_FACES:,} faces"
            if reason is not None:
                link.remove(collision)
                dropped[reason] = dropped.get(reason, 0) + 1
                continue
            mesh.set("filename", resolved)
    for reason, count in sorted(dropped.items()):
        notes.append(f"{count} collision geometr{'y' if count == 1 else 'ies'} dropped ({reason}); each link keeps its inertia")
    # The mujoco extension element: absolute mesh paths, every link its own body so the frame graph
    # the driver builds is the URDF's own, and the bounds that let a link with no <inertial> compile.
    for existing in robot.findall("mujoco"):
        robot.remove(existing)
    extension = ET.SubElement(robot, "mujoco")
    ET.SubElement(
        extension,
        "compiler",
        {
            "strippath": "false",
            "fusestatic": "false",
            "discardvisual": "true",
            "balanceinertia": "true",
            "boundmass": "1e-4",
            "boundinertia": "1e-8",
        },
    )
    return ET.tostring(robot, encoding="unicode"), notes


def _faces(path: str) -> int:
    """How many triangles an STL declares, or 0 for anything else — the only format with a limit."""
    if not path.lower().endswith(".stl"):
        return 0
    try:
        with open(path, "rb") as handle:
            head = handle.read(84)
            if head[:5].lower() == b"solid" and b"facet" in handle.read(4096):
                handle.seek(0)
                return sum(line.lstrip().startswith(b"facet") for line in handle)
            return int.from_bytes(head[80:84], "little") if len(head) == 84 else 0
    except OSError:
        return 0


def _number(text: str | None) -> float:
    try:
        return float(text) if text is not None else 0.0
    except ValueError:
        return 0.0


def _physical(inertia: ET.Element) -> bool:
    """Whether a URDF inertia tensor is positive definite — what MuJoCo requires of a rigid body."""
    xx, yy, zz = (_number(inertia.get(k)) for k in ("ixx", "iyy", "izz"))
    xy, xz, yz = (_number(inertia.get(k)) for k in ("ixy", "ixz", "iyz"))
    tensor = numpy.array([[xx, xy, xz], [xy, yy, yz], [xz, yz, zz]])
    if not numpy.all(numpy.isfinite(tensor)):
        return False
    if not numpy.any(tensor):
        # All zeros is a link that declares no inertia, which the compiler's bounds handle; it is
        # not a tensor with a wrong sign in it.
        return True
    # Positive, and not by rounding error: MuJoCo refuses a tensor whose smallest principal moment
    # is a billionth of its largest, which a rod-shaped link exported in single precision can be.
    values = numpy.linalg.eigvalsh(tensor)
    return bool(values[0] > 1e-15 and values[0] > 1e-9 * values[-1])


def _urdf_facts(urdf_path: str) -> dict[str, dict[str, Any]]:
    """Per joint: type, whether it mimics another, and the effort the file declares."""
    facts: dict[str, dict[str, Any]] = {}
    for joint in ET.parse(urdf_path).getroot().iter("joint"):
        name = joint.get("name", "")
        limit = joint.find("limit")
        effort = None
        if limit is not None and limit.get("effort") not in (None, ""):
            try:
                effort = float(limit.get("effort"))
            except ValueError:
                effort = None
        mimic = joint.find("mimic")
        facts[name] = {
            "type": joint.get("type", ""),
            "mimics": mimic.get("joint") if mimic is not None else None,
            "effort": effort,
        }
    return facts


def build_from_urdf(mujoco: Any, urdf_path: str, root: str, out_path: str) -> dict[str, Any]:
    """Write a driver-ready MJCF for `urdf_path` to `out_path`, and say what was done to get it.

    Raises :class:`BuildError` when MuJoCo will not compile the file at all; the message is MuJoCo's
    own, because it names the element.
    """
    text, notes = _prepared_urdf(urdf_path, root)
    facts = _urdf_facts(urdf_path)
    try:
        spec = mujoco.MjSpec.from_string(text)
    except Exception as error:  # noqa: BLE001 - MuJoCo raises ValueError with its own sentence
        raise BuildError(f"MuJoCo will not parse it: {error}") from None
    # Set on the spec as well as in the URDF's own extension element, because `to_xml` writes the
    # spec's compiler: a body that compiled here and not where the driver reloads it is no body.
    spec.compiler.balanceinertia = True
    spec.compiler.boundmass = 1e-4
    spec.compiler.boundinertia = 1e-8
    spec.option.integrator = mujoco.mjtIntegrator.mjINT_IMPLICITFAST
    spec.option.timestep = 0.002
    for geom in spec.geoms:
        geom.contype = 0
        geom.conaffinity = 0
    for body in spec.bodies:
        body.gravcomp = 1.0
    # A travel past a thousand radians is a URDF's way of writing "continuous" (F149), and MuJoCo
    # would hold it as a range; the joint is left unlimited instead, as the importer leaves it.
    for joint in spec.joints:
        if len(joint.range) == 2 and max(abs(float(joint.range[0])), abs(float(joint.range[1]))) >= 1e3:
            joint.limited = mujoco.mjtLimited.mjLIMITED_FALSE
            notes.append(f"joint `{joint.name}` declares a travel past a thousand radians; simulated as continuous")
    try:
        bare = spec.compile()
    except Exception as error:  # noqa: BLE001
        raise BuildError(f"MuJoCo will not compile it: {error}") from None

    # The effective inertia each joint sees at the zero pose, from the full mass matrix.
    data = mujoco.MjData(bare)
    mujoco.mj_forward(bare, data)
    dense = numpy.zeros((bare.nv, bare.nv))
    # MuJoCo changed this call's signature (a model, a data and the destination, since the mass
    # matrix stopped being one array); the older form takes the matrix itself.
    try:
        mujoco.mj_fullM(bare, data, dense)
    except TypeError:
        mujoco.mj_fullM(bare, dense, data.qM)
    followers = {
        mujoco.mj_id2name(bare, mujoco.mjtObj.mjOBJ_JOINT, int(bare.eq_obj1id[i]))
        for i in range(bare.neq)
        if int(bare.eq_type[i]) == int(mujoco.mjtEq.mjEQ_JOINT)
    }
    omega = 2.0 * math.pi * SERVO_HZ
    actuated: list[str] = []
    skipped: dict[str, str] = {}
    for index in range(bare.njnt):
        name = mujoco.mj_id2name(bare, mujoco.mjtObj.mjOBJ_JOINT, index) or f"joint{index}"
        kind = int(bare.jnt_type[index])
        if kind not in (int(mujoco.mjtJoint.mjJNT_HINGE), int(mujoco.mjtJoint.mjJNT_SLIDE)):
            skipped[name] = "free or ball joint"
            continue
        if name in followers or (facts.get(name) or {}).get("mimics"):
            skipped[name] = f"mimics {(facts.get(name) or {}).get('mimics') or 'another joint'}"
            continue
        dof = int(bare.jnt_dofadr[index])
        inertia = max(float(dense[dof, dof]), 1e-6)
        kp, kv = inertia * omega * omega, 2.0 * inertia * omega
        actuator = spec.add_actuator()
        actuator.name = name
        actuator.target = name
        actuator.trntype = mujoco.mjtTrn.mjTRN_JOINT
        actuator.gaintype = mujoco.mjtGain.mjGAIN_FIXED
        actuator.biastype = mujoco.mjtBias.mjBIAS_AFFINE
        gain = [0.0] * len(actuator.gainprm)
        gain[0] = kp
        actuator.gainprm = gain
        bias = [0.0] * len(actuator.biasprm)
        bias[1], bias[2] = -kp, -kv
        actuator.biasprm = bias
        effort = (facts.get(name) or {}).get("effort")
        # The importer's sentinel rule (F149): a million newton-metres is "unlimited", not a rating.
        if effort is not None and 0.0 < effort < 1e6:
            actuator.forcelimited = mujoco.mjtLimited.mjLIMITED_TRUE
            actuator.forcerange = [-effort, effort]
        actuated.append(name)
    if not actuated:
        raise BuildError("the file has no joint a servo could drive (every joint is fixed or mimicked)")
    try:
        spec.compile()
    except Exception as error:  # noqa: BLE001
        raise BuildError(f"MuJoCo will not compile it with its servos: {error}") from None
    text = spec.to_xml()
    # MuJoCo's writer drops `balanceinertia` from the compiler element, and an inertia written at
    # six significant digits can then sit exactly on the triangle inequality it enforces.
    if "balanceinertia" not in text:
        text = text.replace("<compiler ", '<compiler balanceinertia="true" ', 1)
    with open(out_path, "w", encoding="utf-8") as out:
        out.write(text)
    # The file is what the driver opens, in another process, so the file is what is checked.
    try:
        mujoco.MjModel.from_xml_path(out_path)
    except Exception as error:  # noqa: BLE001
        raise BuildError(f"the written body does not load back: {error}") from None
    return {
        "actuated": actuated,
        "skipped": skipped,
        "notes": notes,
        "servo_hz": SERVO_HZ,
    }

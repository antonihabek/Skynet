#!/usr/bin/env python3
"""Regenerate one Doosan arm's URDFs and provisioned descriptor, inside `doosan/Dockerfile`'s image.

    docker run --rm -v "$PWD":/src skynet-doosan-drcf \
      python3 /src/drivers/ros2_control/doosan/generate.py <slug> [--check]

writes `urdf/doosan_<slug>_live.urdf`, `urdf/doosan_<slug>_mock.urdf` and
`urdf/doosan_<slug>.provisioned.json` under `drivers/ros2_control`, or with `--check` compares what it
would write to what is there and writes nothing. The two URDFs are the vendor's own `xacro` output,
emitted by the vendor's xacro at the commit the image pins, with this repository's provenance comment
after the XML declaration; the mock differs from the live file by the one `<plugin>` line its own
header says. What is written into each comment is taken from the committed `doosan_m0609_*` files, the
first of the three arms added under Phase 10 item 10.6, with the model name and the effort figures
substituted — so the words that say why `host` is 127.0.0.1 and why `update_rate` is 10 exist once.

The provisioned descriptor's limits are the URDF's, read as the degrees Doosan means (a joint of
`upper="6.2832"` is 360°, and 360° is written as `math.radians(360)`, which is what the descriptor's
first four arms carry): a limit that is not within a fiftieth of a degree of a whole one is kept as
the URDF prints it, and the descriptor's source says so. No torque is declared: `<limit effort>` is a
design value in every vendor's URDF and is declined here as everywhere (§63.4).

`--check` is what says a committed file was edited by hand: `make check-doosan-urdfs` runs it for every
arm the recipe wrote, in the image. It is not in CI's `invariants` job, which has no image; the
nightly substrate run rebuilds the image at the pin, which is where this recipe is true.
"""

from __future__ import annotations

import json
import math
import os
import re
import subprocess
import sys
from pathlib import Path

WORKSPACE = Path("/opt/doosan_ws")
DESCRIPTION = WORKSPACE / "install/share/dsr_description2"
ROLES = (
    "right_arm.shoulder.yaw",
    "right_arm.shoulder.pitch",
    "right_arm.elbow.flex",
    "right_arm.wrist.pitch",
    "right_arm.wrist.yaw",
    "right_arm.wrist.roll",
)
TEMPLATE = "m0609"
LIVE_PLUGIN = "<plugin>dsr_hardware2/DRHWInterface</plugin>"
MOCK_PLUGIN = "<plugin>mock_components/GenericSystem</plugin>"


def xacro(slug: str, *arguments: str) -> str:
    """The vendor's xacro, run from the description's own directory as its comment records."""
    # A fixed path, because it is written into the file: the header names it.
    stub = Path("/tmp/ament_dsr")
    # The description's Gazebo macro looks `dsr_controller2` up for a path it writes only into
    # the `<gazebo>` element, which nothing here reads: an ament index entry answers it.
    (stub / "share/ament_index/resource_index/packages").mkdir(parents=True, exist_ok=True)
    (stub / "share/ament_index/resource_index/packages/dsr_controller2").write_text("")
    (stub / "share/dsr_controller2").mkdir(exist_ok=True)
    environment = dict(os.environ)
    environment["AMENT_PREFIX_PATH"] = f"{stub}:{environment.get('AMENT_PREFIX_PATH', '')}"
    done = subprocess.run(
        ["xacro", f"xacro/{slug}.urdf.xacro", *arguments],
        cwd=DESCRIPTION,
        env=environment,
        capture_output=True,
        text=True,
    )
    if done.returncode != 0:
        raise SystemExit(f"generate: the vendor's xacro refused {slug}:\n{done.stderr}")
    return done.stdout


def header_of(text: str) -> str:
    start = text.index("<!--\n  Skynet")
    return text[start : text.index("-->\n", start) + 4]


def limits_of(urdf: str) -> list[tuple[float, float, float, float]]:
    """`(effort, lower, upper, velocity)` of every revolute joint, in file order."""
    found = re.findall(
        r'<limit effort="([^"]+)" lower="([^"]+)" upper="([^"]+)" velocity="([^"]+)"', urdf
    )
    return [tuple(float(value) for value in row) for row in found]  # type: ignore[misc]


def whole_degrees(radians: float) -> float | None:
    degrees = math.degrees(radians)
    return float(round(degrees)) if abs(degrees - round(degrees)) < 0.02 else None


def descriptor_of(slug: str, urdf: str, commit: str) -> dict:
    rows = limits_of(urdf)
    if len(rows) != len(ROLES):
        raise SystemExit(
            f"generate: {slug} has {len(rows)} limited joints and this table names {len(ROLES)}. "
            "Doosan's six-axis arms are what it is for; an arm of another count needs its own roles."
        )
    joints = []
    parts = []
    for index, (_effort, lower, upper, velocity) in enumerate(rows):
        span = whole_degrees(upper)
        speed = whole_degrees(velocity)
        exact = span is not None and speed is not None and abs(lower + upper) < 1e-3
        travel = math.radians(span) if span is not None else upper
        rate = math.radians(speed) if speed is not None else velocity
        joints.append(
            {
                "control_modes": ["position"],
                "limits": {
                    "position_max_rad": travel,
                    "position_min_rad": -travel if abs(lower + upper) < 1e-3 else lower,
                    "velocity_max_rad_s": rate,
                },
                "name": f"joint_{index + 1}",
                "role": ROLES[index],
                "type": "revolute",
            }
        )
        parts.append(
            (f"±{span:g}°" if exact else f"{lower:g}..{upper:g} rad", f"{speed:g}°/s" if exact else f"{velocity:g} rad/s")
        )
    groups: list[list] = []
    for index, part in enumerate(parts):
        if groups and groups[-1][1] == part:
            groups[-1][2] = index
        else:
            groups.append([index, part, index])
    said = ", ".join(
        f"J{a + 1}{'–J' + str(b + 1) if b != a else ''} {rng}/{spd}" for a, (rng, spd), b in groups
    )
    return {
        "base": "fixed",
        "capabilities": ["safety.estop"],
        "control": {"modes": ["position"], "per_joint_modes": True, "rate_hz_max": 1000, "rate_hz_min": 1},
        "digest": None,
        "dof": len(joints),
        "end_effectors": [],
        "envelope": {
            "basis": "declared",
            "joint_velocity_max_rad_s": min(j["limits"]["velocity_max_rad_s"] for j in joints),
            "source": (
                f"dsr_description2's generated URDF joint limits at {commit[:7]} (Doosan's own "
                f"data): {said}. Joint torques not tabulated: the description publishes range and "
                "speed only, and `<limit effort>` is a design value declined like every URDF's"
            ),
        },
        "extensions": [],
        "joints": joints,
        "robot_slug": slug,
        "schema_version": 1,
        "sensors": [],
        "simulator": True,
        "solvers": [],
        "verified": False,
    }


def main(argv: list[str]) -> int:
    check = "--check" in argv
    slugs = [word for word in argv if not word.startswith("--")]
    if len(slugs) != 1 or not re.fullmatch(r"[a-z]\d{4}", slugs[0]):
        print(__doc__, file=sys.stderr)
        return 2
    slug = slugs[0]
    urdf_dir = Path(__file__).resolve().parents[1] / "urdf"
    commit = (WORKSPACE / "doosan-robot2.commit").read_text().strip()

    template_live = header_of((urdf_dir / f"doosan_{TEMPLATE}_live.urdf").read_text(encoding="utf-8"))
    template_mock = header_of((urdf_dir / f"doosan_{TEMPLATE}_mock.urdf").read_text(encoding="utf-8"))
    if commit not in template_live:
        raise SystemExit(
            f"generate: the image builds doosan-robot2 at {commit} and the committed headers name "
            "another commit. Regenerate every Doosan file, starting with the m0609 template, "
            "before adding an arm at a new pin."
        )

    live = xacro(
        slug,
        "mode:=virtual",
        "host:=127.0.0.1",
        "rt_host:=127.0.0.1",
        "port:=12345",
        f"model:={slug}",
        "update_rate:=10",
    )
    mock = xacro(slug).replace(LIVE_PLUGIN, MOCK_PLUGIN)
    if MOCK_PLUGIN not in mock or LIVE_PLUGIN not in live:
        raise SystemExit(f"generate: {slug}'s URDF has no `{LIVE_PLUGIN}` line to configure.")

    efforts = "/".join(f"{effort:g}" for effort, *_ in limits_of(live))
    mock_header = re.sub(r"\((\d+(?:/\d+){5}) Nm\)", f"({efforts} Nm)", template_mock)
    outputs = {}
    for kind, text, template in (("live", live, template_live), ("mock", mock, mock_header)):
        declaration, _, rest = text.partition("\n")
        outputs[f"doosan_{slug}_{kind}.urdf"] = (
            declaration + "\n" + template.replace(TEMPLATE, slug) + rest
        )
    outputs[f"doosan_{slug}.provisioned.json"] = json.dumps(
        descriptor_of(slug, live, commit), sort_keys=True
    )

    status = 0
    for name, text in outputs.items():
        path = urdf_dir / name
        if check:
            if name.endswith(".json"):
                # A provisioned descriptor's *source* is a person's sentence, and the first four
                # arms' cite catalogues; what the recipe holds is the numbers under it.
                if not path.exists():
                    same = False
                else:
                    have, want = json.loads(path.read_text(encoding="utf-8")), json.loads(text)
                    same = have["joints"] == want["joints"] and (
                        have["envelope"]["joint_velocity_max_rad_s"]
                        == want["envelope"]["joint_velocity_max_rad_s"]
                    )
            else:
                same = path.exists() and path.read_text(encoding="utf-8") == text
            if not same:
                print(f"generate: {name} is not what the recipe writes")
                if path.exists() and not name.endswith(".json"):
                    import difflib

                    for line in list(
                        difflib.unified_diff(
                            path.read_text(encoding="utf-8").splitlines(),
                            text.splitlines(),
                            "committed",
                            "recipe",
                            lineterm="",
                            n=0,
                        )
                    )[:24]:
                        print("  " + line)
                status = 1
        else:
            path.write_text(text, encoding="utf-8", newline="\n")
            print(f"wrote {path.relative_to(urdf_dir.parent)}")
    return status


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

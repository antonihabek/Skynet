"""The accepted role tables can refuse — item 10.8.

`fleet/roles.py` applies a role table to a body only after the geometry agrees with it. A check that
has never failed has proved nothing, so most of this file is the ways a table must be refused: the
Jaco whose wrist is tilted 30°, a body missing a joint the table lists, joints out of order, and a
table for the wrong kind of arm. It needs no MuJoCo — `judge` takes the axes — so it runs in the
`invariants` job.

    python3 fleet/test_roles.py
"""

from __future__ import annotations

import os
import sys
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import robots  # noqa: E402
import roles  # noqa: E402

Z = (0.0, 0.0, 1.0)
Y = (0.0, 1.0, 0.0)
X = (1.0, 0.0, 0.0)


def body(*joints: tuple[str, tuple[float, float, float]]) -> dict[str, tuple[int, tuple[float, float, float]]]:
    """The zero-pose axes of a body: names in tree order, as `zero_pose_axes` returns them."""
    return {name: (index, axis) for index, (name, axis) in enumerate(joints)}


UR5E = body(
    ("shoulder_pan_joint", Z), ("shoulder_lift_joint", Y), ("elbow_joint", Y),
    ("wrist_1_joint", Y), ("wrist_2_joint", (0.0, 0.0, -1.0)), ("wrist_3_joint", Y),
)
XARM6 = body(
    ("joint1", Z), ("joint2", Y), ("joint3", Y), ("joint4", (0.0, 0.0, -1.0)), ("joint5", Y), ("joint6", (0.0, 0.0, -1.0)),
)


class Accepted(unittest.TestCase):
    def test_a_ur_holds_the_parallel_wrist_table(self) -> None:
        verdict = roles.judge("ur5e_description", UR5E)
        self.assertEqual(verdict["outcome"], "accepted", verdict)
        self.assertEqual(verdict["table"], "six_parallel_wrist")
        self.assertEqual(
            verdict["_roles"],
            {
                "shoulder_pan_joint": roles.YAW,
                "shoulder_lift_joint": roles.PITCH,
                "elbow_joint": roles.ELBOW,
                "wrist_1_joint": roles.WRIST_FLEX,
                "wrist_3_joint": roles.WRIST_ROLL,
            },
        )
        self.assertEqual(verdict["without_counterpart"], ["wrist_2_joint"])

    def test_a_numbered_industrial_arm_holds_the_roll_mid_table(self) -> None:
        verdict = roles.judge("xarm6_description", XARM6)
        self.assertEqual(verdict["outcome"], "accepted", verdict)
        self.assertEqual(verdict["_roles"]["joint5"], roles.WRIST_FLEX)
        self.assertEqual(verdict["_roles"]["joint6"], roles.WRIST_ROLL)
        self.assertNotIn("joint4", verdict["_roles"])


class Refused(unittest.TestCase):
    def test_a_tilted_wrist_is_refused_by_name(self) -> None:
        # The JACO2 with a 60° wrist: its fifth axis is 30° off perpendicular to its fourth.
        tilted = (0.0, -0.87, -0.5)
        jaco = body(
            ("j2n6s200_joint_1", (0.0, 0.0, -1.0)), ("j2n6s200_joint_2", (0.0, -1.0, 0.0)), ("j2n6s200_joint_3", Y),
            ("j2n6s200_joint_4", (0.0, 0.0, -1.0)), ("j2n6s200_joint_5", tilted), ("j2n6s200_joint_6", (0.0, 0.0, -1.0)),
        )
        verdict = roles.judge("j2n6s200_description", jaco)
        self.assertEqual(verdict["outcome"], "refused", verdict)
        self.assertIn("j2n6s200_joint_5", verdict["detail"])
        self.assertIn("30° from perpendicular", verdict["detail"])
        self.assertNotIn("_roles", verdict)

    def test_the_straight_wrist_variant_of_the_same_arm_is_accepted(self) -> None:
        straight = body(
            ("j2s6s200_joint_1", (0.0, 0.0, -1.0)), ("j2s6s200_joint_2", (0.0, -1.0, 0.0)), ("j2s6s200_joint_3", Y),
            ("j2s6s200_joint_4", (0.0, 0.0, -1.0)), ("j2s6s200_joint_5", (0.0, -1.0, 0.0)), ("j2s6s200_joint_6", Z),
        )
        self.assertEqual(roles.judge("j2s6s200_description", straight)["outcome"], "accepted")

    def test_a_table_for_a_joint_the_body_does_not_have_is_refused(self) -> None:
        # KUKA's iiwa 7 as the sweep builds it has no joint 4: the table lists seven and the body six.
        iiwa7 = body(
            ("iiwa_joint_1", Z), ("iiwa_joint_2", Y), ("iiwa_joint_3", Z),
            ("iiwa_joint_5", Z), ("iiwa_joint_6", Y), ("iiwa_joint_7", Z),
        )
        verdict = roles.judge("iiwa7_description", iiwa7)
        self.assertEqual(verdict["outcome"], "refused", verdict)
        self.assertIn("iiwa_joint_4", verdict["detail"])

    def test_joints_out_of_order_are_refused(self) -> None:
        swapped = body(*[(name, UR5E[name][1]) for name in list(UR5E)[:2] + ["wrist_1_joint", "elbow_joint", "wrist_2_joint", "wrist_3_joint"]])
        verdict = roles.judge("ur5e_description", swapped)
        self.assertEqual(verdict["outcome"], "refused", verdict)
        self.assertIn("not in order", verdict["detail"])

    def test_an_arm_of_the_wrong_kind_is_refused(self) -> None:
        # Asserted as a UR — three parallel axes — an xArm 6 has a roll where the first wrist should be.
        original = roles.ARMS.get("xarm6_description")
        roles.ARMS["xarm6_description"] = ("six_parallel_wrist", ["joint1", "joint2", "joint3", "joint4", "joint5", "joint6"])
        try:
            verdict = roles.judge("xarm6_description", XARM6)
        finally:
            roles.ARMS["xarm6_description"] = original
        self.assertEqual(verdict["outcome"], "refused", verdict)
        self.assertIn("joint4", verdict["detail"])
        self.assertIn("parallel", verdict["detail"])

    def test_a_joint_the_table_skips_is_refused(self) -> None:
        # A body with a movable joint between the arm's first and last that the table does not list.
        extra = body(
            ("shoulder_pan_joint", Z), ("shoulder_lift_joint", Y), ("elbow_joint", Y), ("wrist_1_joint", Y),
            ("wrist_2_joint", (0.0, 0.0, -1.0)), ("stowaway_joint", Y), ("wrist_3_joint", Y),
        )
        verdict = roles.judge("ur5e_description", extra)
        self.assertEqual(verdict["outcome"], "refused", verdict)
        self.assertIn("stowaway_joint", verdict["detail"])

    def test_a_description_with_no_table_is_not_applicable(self) -> None:
        self.assertEqual(roles.judge("go2_description", {})["outcome"], "not_applicable")


class TheTablesThemselves(unittest.TestCase):
    def test_every_table_names_a_pattern_that_has_a_place_for_each_joint(self) -> None:
        for description, (pattern, joints) in roles.ARMS.items():
            self.assertIn(pattern, roles.PATTERNS, description)
            self.assertLessEqual(len(joints), len(roles.PATTERNS[pattern]), description)
            self.assertEqual(len(joints), len(set(joints)), f"{description} lists a joint twice")

    def test_a_pattern_gives_no_role_twice(self) -> None:
        for name, pattern in roles.PATTERNS.items():
            given = [role for role, _ in pattern if role is not None]
            self.assertEqual(len(given), len(set(given)), name)

    def test_a_relation_only_refers_to_a_role_placed_before_it(self) -> None:
        for name, pattern in roles.PATTERNS.items():
            placed: set[str] = set()
            for role, relation in pattern:
                if role is not None:
                    placed.add(role)
                if relation and ":" in relation:
                    self.assertIn(relation.split(":", 1)[1], placed, f"{name}: {relation}")

    def test_every_description_named_is_one_the_fleet_runs(self) -> None:
        for description in roles.ARMS:
            self.assertIn(description, robots.ROBOTS, f"{description} is in roles.ARMS and not in robots.ROBOTS")


if __name__ == "__main__":
    unittest.main(verbosity=2)

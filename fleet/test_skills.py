"""What the `skill` stage reads, and the ways it must refuse — item 12.F.7.

`fleet/skills.py` turns the robot's own `skynet skill check` report and a skill manifest into the three
questions the stage asks (binds, names, supplied), finds the Hub's skills, and decides which joint a
skill's gripper channel means. A reader that cannot fail proves nothing, so most of this file is the
inputs it must not accept: a report with no transport verdict, a gripper skill on a body with no joint
carrying its role, a hand declared on the wrong side, a manifest that commands nothing. It needs
neither MuJoCo nor the Hub, so it runs in the `invariants` job.

    python3 fleet/test_skills.py
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import skills  # noqa: E402

# Lines as `skynet skill check` prints them: two spaces of indent under a section, then the verdict word.
REPORT = """
  skynet-demo/park 1.0.0

  package
    pass  package.manifest.present         manifest.json at the package root
    miss  package.entrypoint.runs          the entry point is executable on the robot
          not measurable here: the store makes the declared entry point the one executable
  skill
    pass  skill.requests.declared          every requested token is one the package declared it needs
    FAIL  skill.context.supplied           every context path a precondition or success criterion reads is one the daemon fills
          success criterion [1] The gripper is parked open: `robot.state.gripper`
    FAIL  skill.context.on_body            every joint a precondition or success criterion names is a joint of this body
    n/a   skill.composition.acyclic        the declared call graph is acyclic and within the depth limit
    pass  skill.transport.resolves         the action space reaches this body (§6, §37.1)

  18 passed, 2 failed, 1 not measured, 1 not applicable
"""

PARK = {
    "id": "skynet-demo/park",
    "version": "1.0.0",
    "task": "Fold the arm.",
    "expected_duration_seconds": 12,
    "embodiment": {"verified_on": []},
    "source_embodiment": {
        "robot_slug": "so101-follower",
        "action_channels": [
            {"index": 0, "kind": "joint_position", "role": "right_arm.shoulder.yaw", "joint": "shoulder_pan"},
            {"index": 1, "kind": "gripper", "role": "right_hand.gripper.grip", "joint": "gripper"},
        ],
    },
    "runtime": {
        "entrypoint": "bin/run",
        "args": ["--hz", "50", "--channels", "2", "--mode", "position", "--cycles", "600", "--backend", "trajectory",
                 "--setpoints", "0:0,1;60:0.5,1;120:0.5,0"],
    },
}


class ReadingTheReport(unittest.TestCase):
    def test_every_verdict_word_is_read(self) -> None:
        checks = skills.parse_checks(REPORT)
        self.assertEqual(checks["skill.transport.resolves"], "pass")
        self.assertEqual(checks["skill.context.supplied"], "FAIL")
        self.assertEqual(checks["package.entrypoint.runs"], "miss")
        self.assertEqual(checks["skill.composition.acyclic"], "n/a")

    def test_explanatory_lines_are_not_checks(self) -> None:
        # The indented prose under a verdict is not a verdict: a line that happens to begin with `pass` would be.
        text = REPORT + "\n          pass  skill.context.on_body this is a sentence inside a refusal\n"
        self.assertEqual(skills.parse_checks(text)["skill.context.on_body"], "FAIL")

    def test_a_report_without_the_transport_verdict_is_not_a_pass(self) -> None:
        checks = skills.parse_checks("  18 passed, 0 failed\n")
        self.assertEqual(checks, {})
        self.assertEqual(skills.verdict(checks), {"binds": False, "names": False, "supplied": False})

    def test_the_three_questions_are_separate(self) -> None:
        verdict = skills.verdict(skills.parse_checks(REPORT))
        self.assertEqual(verdict, {"binds": True, "names": False, "supplied": False})
        # Asked in order: the action space, then the joints the clauses name, then what the daemon supplies.
        self.assertEqual(skills.first_refusal(skills.parse_checks(REPORT)), "skill.context.on_body")

    def test_first_refusal_is_none_only_when_all_three_pass(self) -> None:
        passing = {check: "pass" for check in skills.CHECKS}
        self.assertIsNone(skills.first_refusal(passing))
        self.assertEqual(skills.first_refusal({**passing, "skill.context.on_body": "FAIL"}), "skill.context.on_body")
        self.assertEqual(skills.first_refusal({**passing, "skill.transport.resolves": "n/a"}), "skill.transport.resolves")


class ReadingTheManifest(unittest.TestCase):
    def test_the_taught_path(self) -> None:
        shape = skills.trajectory(PARK)
        assert shape is not None
        self.assertEqual((shape["hz"], shape["cycles"], shape["mode"], shape["channels"], shape["rows"]), (50, 600, "position", 2, 3))

    def test_a_package_that_commands_nothing_has_no_trajectory(self) -> None:
        dwell = {"runtime": {"entrypoint": "bin/wait", "args": ["--seconds", "10"]}, "expected_duration_seconds": 10}
        self.assertIsNone(skills.trajectory(dwell))
        without_embodiment = {**PARK}
        without_embodiment.pop("source_embodiment")
        self.assertIsNone(skills.trajectory(without_embodiment))

    def test_a_table_with_no_rate_or_bound_is_not_driven(self) -> None:
        stripped = json.loads(json.dumps(PARK))
        stripped["runtime"]["args"] = ["--setpoints", "0:0,1"]
        self.assertIsNone(skills.trajectory(stripped))

    def test_the_summary_says_what_the_package_commands(self) -> None:
        summary = skills.skill_summary({"name": "park", "dir": "/x", "manifest": PARK})
        self.assertEqual((summary["name"], summary["commands"], summary["rows"], summary["source"]), ("park", True, 3, "so101-follower"))


def body(*joints: tuple[str, str | None]) -> dict:
    return {"joints": [{"name": name, "role": role} for name, role in joints]}


class DeclaringTheHand(unittest.TestCase):
    def test_the_joint_carrying_the_channels_role_is_the_hand(self) -> None:
        served = body(("1", "right_arm.shoulder.yaw"), ("6", "right_hand.gripper.grip"))
        self.assertEqual(skills.hand_for(PARK, served), {"joint": "6", "kind": "gripper", "role": "right_hand.gripper.grip", "by": "role"})

    def test_a_body_with_no_such_role_and_no_such_joint_has_no_hand_declared(self) -> None:
        # The stage must not invent one: a body whose joints carry no gripper role and none named as the skill names its own keeps the planner's refusal.
        self.assertIsNone(skills.hand_for(PARK, body(("1", "right_arm.shoulder.yaw"), ("2", None))))

    def test_the_skills_own_joint_name_is_the_second_ground(self) -> None:
        # PARK's gripper channel says `"joint": "gripper"`. A body with a joint of exactly that name and no role for it: the skill's word, by name.
        hand = skills.hand_for(PARK, body(("shoulder_pan", None), ("gripper", None)))
        assert hand is not None
        self.assertEqual((hand["joint"], hand["by"], hand["role"]), ("gripper", "name", None))

    def test_a_role_beats_a_name(self) -> None:
        hand = skills.hand_for(PARK, body(("gripper", None), ("6", "right_hand.gripper.grip")))
        assert hand is not None
        self.assertEqual((hand["joint"], hand["by"]), ("6", "role"))

    def test_a_joint_that_merely_resembles_the_name_is_not_a_hand(self) -> None:
        # `Jaw`, `gripper_joint_1`, `finger` are hands to a person and not the skill's word: only the exact name counts.
        self.assertIsNone(skills.hand_for(PARK, body(("Jaw", None), ("gripper_joint_1", None), ("finger", None))))

    def test_the_other_arms_hand_is_not_declared(self) -> None:
        # The planner binds a gripper channel to the first effector of the kind, so declaring the left hand
        # for a right-arm skill would route it to the wrong hand. Only the side the skill names is declared.
        both = body(("l", "left_hand.gripper.grip"), ("r", "right_hand.gripper.grip"))
        self.assertEqual(skills.hand_for(PARK, both)["joint"], "r")  # type: ignore[index]
        left_only = body(("l", "left_hand.gripper.grip"), ("a", "right_arm.shoulder.yaw"))
        self.assertIsNone(skills.hand_for(PARK, left_only))

    def test_a_skill_with_no_gripper_channel_declares_nothing(self) -> None:
        five = json.loads(json.dumps(PARK))
        five["source_embodiment"]["action_channels"] = five["source_embodiment"]["action_channels"][:1]
        self.assertIsNone(skills.hand_for(five, body(("6", "right_hand.gripper.grip"))))

    def test_the_copy_declares_the_effector_and_leaves_the_original_alone(self) -> None:
        served = body(("6", "right_hand.gripper.grip"))
        served["end_effectors"] = []
        declared = skills.with_hand(served, {"joint": "6", "kind": "gripper", "role": "right_hand.gripper.grip"})
        self.assertEqual(declared["end_effectors"], [{"name": "6", "role": "right_hand.gripper.grip", "kind": "gripper", "dof": 1.0}])
        self.assertEqual(served["end_effectors"], [])


class WhereATaughtPathExceedsABody(unittest.TestCase):
    TABLE = "0:0,-0.4,1;60:0.5,-1.5,0.2;120:0,-1.7,1"

    def test_each_channels_lowest_and_highest_ask(self) -> None:
        self.assertEqual(skills.taught_ranges(self.TABLE), [(0.0, 0.5), (-1.7, -0.4), (0.2, 1.0)])

    def test_an_empty_table_asks_nothing(self) -> None:
        self.assertEqual(skills.taught_ranges(""), [])

    def plan(self, travel: dict) -> dict:
        return {
            "bindings": [
                {"source_index": 0, "target": "a"},
                {"source_index": 1, "target": "b"},
                {"source_index": 2, "target": "hand"},
            ],
            "clamp": {"travel": travel},
        }

    def test_a_joint_asked_past_either_end_of_its_travel_is_named(self) -> None:
        ranges = skills.taught_ranges(self.TABLE)
        self.assertEqual(skills.past_travel(self.plan({"a": [-1, 1], "b": [-1.6, 0]}), ranges), ["b: taught -1.7 past -1.6"])
        self.assertEqual(skills.past_travel(self.plan({"a": [-1, 0.4], "b": [-3, 3]}), ranges), ["a: taught 0.5 past 0.4"])

    def test_a_path_inside_the_travel_exceeds_nothing(self) -> None:
        self.assertEqual(skills.past_travel(self.plan({"a": [-1, 1], "b": [-3, 3]}), skills.taught_ranges(self.TABLE)), [])

    def test_a_channel_with_no_travel_in_the_plan_is_not_compared(self) -> None:
        # A hand bound to an effector has no travel in the plan; the check must not invent one or crash on it.
        self.assertEqual(skills.past_travel(self.plan({}), skills.taught_ranges(self.TABLE)), [])

    def test_a_column_the_table_does_not_have_is_skipped(self) -> None:
        plan = {"bindings": [{"source_index": 9, "target": "a"}], "clamp": {"travel": {"a": [0, 0]}}}
        self.assertEqual(skills.past_travel(plan, skills.taught_ranges(self.TABLE)), [])

    def test_a_blocked_binding_has_no_target(self) -> None:
        plan = {"bindings": [{"source_index": 0, "target": None}], "clamp": {"travel": {"a": [0, 0]}}}
        self.assertEqual(skills.past_travel(plan, skills.taught_ranges(self.TABLE)), [])


class WhatTheHostCannotRun(unittest.TestCase):
    def test_a_plan_that_leaves_a_joint_out(self) -> None:
        text = "skynetk: the plan binds 5 channel(s) and the body declares 6 joint(s). This loop commands\n every joint on every cycle."
        self.assertEqual(skills.host_cannot(text), "leaves a joint out")

    def test_a_plan_that_reorders_joints(self) -> None:
        text = "the plan binds channel 1 to `x` and this loop writes channel 1 to joint 1, which is `y`. A plan that reorders joints needs a channel map\n this host does not have yet; refused."
        self.assertEqual(skills.host_cannot(text), "reorders joints")

    def test_any_other_refusal_is_not_the_hosts_known_limit(self) -> None:
        # A plan the planner itself refuses, or a driver that would not open, is a failure and must stay one.
        self.assertIsNone(skills.host_cannot("skynetk: the plan is refused: This body has no gripper and the policy requires one."))
        self.assertIsNone(skills.host_cannot(""))


class FindingTheLibrary(unittest.TestCase):
    def test_the_hub_names_are_the_ones_the_other_tools_use(self) -> None:
        names = skills.hub_names(REPO)
        self.assertIn("Robot Overflow", names)
        self.assertEqual(len(names), len(set(names)))

    def test_an_explicit_directory_is_used_and_a_missing_one_is_not_replaced_by_a_guess(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            self.assertEqual(skills.find_skills(REPO, directory), directory)
            self.assertIsNone(skills.find_skills(REPO, os.path.join(directory, "nowhere")))

    def test_packages_are_the_directories_that_hold_a_manifest(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            os.makedirs(os.path.join(directory, "park"))
            os.makedirs(os.path.join(directory, "not-a-package"))
            with open(os.path.join(directory, "park", "manifest.json"), "w", encoding="utf-8") as handle:
                json.dump(PARK, handle)
            found = skills.packages(directory)
            self.assertEqual([package["name"] for package in found], ["park"])


if __name__ == "__main__":
    unittest.main(verbosity=1)
